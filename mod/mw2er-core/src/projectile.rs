//! M6 launcher projectiles simulated in the DLL (docs/M6_DESIGN.md section 6.6) and the simplified lock-on
//! (a user-visible deviation: MW2's Stinger locks only air killstreaks, Elden Ring has none).
//!
//! Units: metres, seconds, world space with +Y up (the DLL's havok frame). The DLL steps each projectile once per
//! tick, casts one map ray along the step segment and tests the characters' capsules (`ac130_aim::first_hit`).
//!
//! Flight models [I]:
//! - Rockets (AT4, RPG, Stinger, Javelin): straight from the muzzle along the aim, no gravity; a projectile with an
//!   ignition delay leaves at 1/10 speed and reaches full speed at ignition.
//! - M79: ballistic, g = 800 in/s^2 = 20.32 m/s^2 [V iw4 constant], v0 = fwd * speed + up * speedUp; it arms after
//!   `projectileActivateDist` from the muzzle (an impact before = dud).
//! - Stinger after lock: proportional navigation (N = 3), lateral acceleration capped at maxSteeringAccel.
//! - Javelin top attack: climb to an apex `h = clamp(0.35 * range, 20, 120)` m above the target, then dive onto it;
//!   direct attack when the sky above the target is blocked within h (the DLL's ray decides). Review M6: the guided
//!   missile does not cruise at its launch projectileSpeed (600 in/s = 15.2 m/s: ~10 s for a 100 m top attack); after
//!   ignition it boosts at `boost_accel` up to `cruise` (`[launchers] javelin_cruise_mps` / `javelin_accel_mps2`) [I]:
//!   MW2's own missile speed / acceleration dvars are engine values not in the dumps.
//! - Self guard: an explosion whose sphere reaches the player's capsule is turned into a dud (`self_blast_distance`).

use crate::vec3::{UP, V3};

pub const GRAVITY_MPS2: f32 = 800.0 * 0.0254;
/// Speed fraction before ignition (eject) [I].
pub const EJECT_FRACTION: f32 = 0.1;
pub const PN_GAIN: f32 = 3.0;
/// Review M6: the tightest turn of the boosted Javelin (m) [I].
pub const JAVELIN_TURN_RADIUS_M: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guidance {
    Straight,
    Ballistic,
    /// Proportional navigation toward a target point the caller updates every tick.
    Pn { target: V3 },
    /// Javelin top attack toward `target`; `apex_y` = the climb height (world y); `diving` once over the apex.
    TopAttack { target: V3, apex_y: f32, diving: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projectile {
    pub id: u32,
    pub gun: usize,
    pub pos: V3,
    pub vel: V3,
    pub start: V3,
    pub t: f32,
    pub speed: f32,
    pub ignition_s: f32,
    pub lifetime_s: f32,
    pub arm_m: f32,
    pub max_accel: f32,
    pub guidance: Guidance,
    /// Review M6: after ignition the speed grows at `boost_accel` (m/s^2) up to `cruise` (m/s); 0 = constant speed.
    pub cruise: f32,
    pub boost_accel: f32,
}

/// What one step decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub from: V3,
    pub to: V3,
    pub expired: bool,
}

impl Projectile {
    #[allow(clippy::too_many_arguments)]
    pub fn launch(id: u32, gun: usize, muzzle: V3, dir: V3, speed: f32, speed_up: f32, ignition_s: f32, lifetime_s: f32, arm_m: f32, max_accel: f32, guidance: Guidance) -> Projectile {
        let d = dir.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
        let v0 = if ignition_s > 0.0 { speed * EJECT_FRACTION } else { speed };
        let mut vel = d * v0;
        if matches!(guidance, Guidance::Ballistic) {
            vel = vel + UP * speed_up;
        }
        Projectile { id, gun, pos: muzzle, vel, start: muzzle, t: 0.0, speed, ignition_s, lifetime_s, arm_m, max_accel, guidance, cruise: 0.0, boost_accel: 0.0 }
    }

    /// A boosted missile (the Javelin): from `speed` after ignition up to `cruise` at `accel` [I].
    pub fn with_boost(mut self, cruise: f32, accel: f32) -> Projectile {
        if cruise > self.speed && accel > 0.0 && cruise.is_finite() && accel.is_finite() {
            self.cruise = cruise;
            self.boost_accel = accel;
        }
        self
    }

    /// Armed (an impact explodes): beyond the arming distance from the muzzle.
    pub fn armed(&self) -> bool {
        self.arm_m <= 0.0 || (self.pos - self.start).len() >= self.arm_m
    }

    /// Advances `dt` seconds (sub-stepped at <= 1/120 s for the guided and ballistic models).
    pub fn step(&mut self, dt: f32) -> Step {
        let from = self.pos;
        let dt = dt.clamp(0.0, 0.25);
        let n = (dt * 120.0).ceil().max(1.0) as usize;
        let h = dt / n as f32;
        for _ in 0..n {
            self.sub(h);
        }
        if !self.pos.is_finite() || !self.vel.is_finite() {
            self.pos = from;
            self.t = self.lifetime_s;
        }
        Step { from, to: self.pos, expired: self.t >= self.lifetime_s - 1e-4 }
    }

    pub fn current_speed(&self) -> f32 {
        if self.ignition_s > 0.0 && self.t < self.ignition_s {
            self.speed * EJECT_FRACTION
        } else if self.boost_accel > 0.0 {
            (self.speed + self.boost_accel * (self.t - self.ignition_s.max(0.0))).min(self.cruise)
        } else {
            self.speed
        }
    }

    fn sub(&mut self, h: f32) {
        self.t += h;
        match self.guidance {
            Guidance::Straight => {
                let d = self.vel.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
                self.vel = d * self.current_speed();
            }
            Guidance::Ballistic => {
                self.vel = self.vel + V3::new(0.0, -GRAVITY_MPS2, 0.0) * h;
            }
            Guidance::Pn { target } => {
                let s = self.current_speed();
                if self.t >= self.ignition_s {
                    let a = pn_accel(self.pos, self.vel, target, V3::ZERO, PN_GAIN, self.max_accel);
                    self.vel = self.vel + a * h;
                }
                self.vel = self.vel.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0)) * s;
            }
            Guidance::TopAttack { target, apex_y, diving } => {
                let s = self.current_speed();
                if self.t >= self.ignition_s {
                    let flat = V3::new(target.x - self.pos.x, 0.0, target.z - self.pos.z);
                    let dive = diving || self.pos.y >= apex_y - 1.0 || flat.len() < (apex_y - target.y).max(1.0) * 0.5;
                    if dive && !diving {
                        self.guidance = Guidance::TopAttack { target, apex_y, diving: true };
                    }
                    let aim = if dive { target } else { V3::new(self.pos.x, apex_y, self.pos.z) + flat * 0.25 };
                    let want = (aim - self.pos).normalize().unwrap_or(self.vel.normalize().unwrap_or(UP));
                    // review M6: with the boost the turn radius v^2/a would grow past the short top-attack arcs (the
                    // missile circles its target); the radius is capped at JAVELIN_TURN_RADIUS_M [I]
                    let acc = self.max_accel.max(s * s / JAVELIN_TURN_RADIUS_M);
                    self.vel = turn_toward(self.vel, want, acc, h);
                }
                self.vel = self.vel.normalize().unwrap_or(UP) * s;
            }
        }
        self.pos = self.pos + self.vel * h;
    }
}

/// Rotates `v` toward the unit direction `want` with at most `max_accel` lateral acceleration over `h`.
pub fn turn_toward(v: V3, want: V3, max_accel: f32, h: f32) -> V3 {
    let s = v.len();
    if s <= 1e-6 {
        return want * 1.0;
    }
    let cur = v * (1.0 / s);
    let ang = cur.dot(want).clamp(-1.0, 1.0).acos();
    let max_ang = (max_accel.max(0.0) * h / s).max(0.0);
    if ang <= max_ang || ang < 1e-6 {
        return want * s;
    }
    let t = max_ang / ang;
    let d = (cur * (1.0 - t) + want * t).normalize().unwrap_or(cur);
    d * s
}

/// Pure proportional navigation: `a = N * (LOS rate x v)` capped at `max_accel` (the lateral part only).
pub fn pn_accel(pos: V3, vel: V3, target: V3, target_vel: V3, n: f32, max_accel: f32) -> V3 {
    let r = target - pos;
    let r2 = r.dot(r);
    if r2 < 1e-6 {
        return V3::ZERO;
    }
    let vr = target_vel - vel;
    let omega = r.cross(vr) * (1.0 / r2);
    let mut a = omega.cross(vel) * n;
    // keep it lateral (no speed change)
    if let Some(vd) = vel.normalize() {
        a = a - vd * a.dot(vd);
    }
    let l = a.len();
    if l > max_accel && l > 0.0 { a * (max_accel / l) } else { a }
}

/// Review M6 self guard: the distance from an explosion at `at` to the player's capsule (feet `feet`, height `h`)
/// when the sphere of `radius_m` + `margin_m` reaches it (the blast would include the player), else None.
pub fn self_blast_distance(feet: V3, h: f32, at: V3, radius_m: f32, margin_m: f32) -> Option<f32> {
    let y = at.y.clamp(feet.y, feet.y + h.max(0.0));
    let d = (at - V3::new(feet.x, y, feet.z)).len();
    (d.is_finite() && d <= radius_m.max(0.0) + margin_m.max(0.0)).then_some(d)
}

/// The Javelin apex height above the target [I]: `clamp(0.35 * range, 20, 120)` m.
pub fn javelin_apex_m(range_m: f32) -> f32 {
    (0.35 * range_m).clamp(20.0, 120.0)
}

// ---- lock-on ------------------------------------------------------------------------------------------------

/// MW2 constants (`_stinger.gsc` / `_javelin.gsc` [V]).
pub const STINGER_LOCK_S: f64 = 1.0;
pub const STINGER_GRACE_S: f64 = 0.5;
pub const JAVELIN_LOCK_S: f64 = 1.15;
pub const JAVELIN_SAMPLES: usize = 12;
pub const JAVELIN_SAMPLE_S: f64 = 0.025;
pub const JAVELIN_RADIUS_M: f32 = 400.0 * 0.0254;
pub const JAVELIN_MIN_DIST_M: f32 = 1100.0 * 0.0254;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LockState {
    Idle,
    Locking { target: u64, t0: f64 },
    Locked { target: u64, lost_since: Option<f64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockEvent {
    Start(u64),
    Acquired(u64),
    /// Lost: `sight` (no LOS / out of the reticle beyond the grace) or `ads` (left the sights).
    Lost { sight: bool },
}

/// Stinger-style lock on a target id (a character handle, or 0 = a Javelin point lock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lock {
    pub state: LockState,
}

impl Default for Lock {
    fn default() -> Self {
        Lock { state: LockState::Idle }
    }
}

impl Lock {
    /// One tick: `ads_ok` = sights up, `candidate` = the target in the reticle with LOS this tick (if any).
    pub fn tick(&mut self, now: f64, ads_ok: bool, candidate: Option<u64>, lock_s: f64, grace_s: f64) -> Option<LockEvent> {
        if !ads_ok {
            let was = !matches!(self.state, LockState::Idle);
            self.state = LockState::Idle;
            return was.then_some(LockEvent::Lost { sight: false });
        }
        match self.state {
            LockState::Idle => {
                if let Some(c) = candidate {
                    self.state = LockState::Locking { target: c, t0: now };
                    return Some(LockEvent::Start(c));
                }
                None
            }
            LockState::Locking { target, t0 } => match candidate {
                Some(c) if c == target => {
                    if now - t0 >= lock_s {
                        self.state = LockState::Locked { target, lost_since: None };
                        return Some(LockEvent::Acquired(target));
                    }
                    None
                }
                Some(c) => {
                    self.state = LockState::Locking { target: c, t0: now };
                    Some(LockEvent::Start(c))
                }
                None => {
                    self.state = LockState::Idle;
                    Some(LockEvent::Lost { sight: true })
                }
            },
            LockState::Locked { target, lost_since } => {
                if candidate == Some(target) {
                    self.state = LockState::Locked { target, lost_since: None };
                    return None;
                }
                let since = lost_since.unwrap_or(now);
                if now - since >= grace_s {
                    self.state = LockState::Idle;
                    return Some(LockEvent::Lost { sight: true });
                }
                self.state = LockState::Locked { target, lost_since: Some(since) };
                None
            }
        }
    }

    pub fn locked(&self) -> Option<u64> {
        match self.state {
            LockState::Locked { target, .. } => Some(target),
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.state = LockState::Idle;
    }
}

/// Javelin point lock: the aim point must stay within `JAVELIN_RADIUS_M` over the last 12 samples (25 ms apart) and be
/// at least `JAVELIN_MIN_DIST_M` away; then the lock timer (1.15 s) runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointLock {
    samples: Vec<V3>,
    last_sample_t: f64,
    pub lock: Lock,
    pub point: Option<V3>,
}

impl PointLock {
    /// `aim` = where the aim ray hits (map or character chest), `dist` its distance; `chr` = a character id when the
    /// ray hit one (the lock then follows it).
    pub fn tick(&mut self, now: f64, ads_ok: bool, aim: Option<(V3, f32, Option<u64>)>) -> Option<LockEvent> {
        if !ads_ok {
            self.samples.clear();
            self.point = None;
            return self.lock.tick(now, false, None, JAVELIN_LOCK_S, 0.0);
        }
        if let Some((p, _, _)) = aim
            && now - self.last_sample_t >= JAVELIN_SAMPLE_S
        {
            self.last_sample_t = now;
            self.samples.push(p);
            if self.samples.len() > JAVELIN_SAMPLES {
                self.samples.remove(0);
            }
        }
        let stable = match aim {
            Some((p, d, _)) => {
                d >= JAVELIN_MIN_DIST_M && self.samples.len() >= JAVELIN_SAMPLES && self.samples.iter().all(|s| (*s - p).len() <= JAVELIN_RADIUS_M)
            }
            None => false,
        };
        let cand = if stable { Some(aim.and_then(|a| a.2).unwrap_or(0)) } else { None };
        if stable {
            self.point = aim.map(|a| a.0);
        }
        // the point lock keeps the last stable point while locked (grace 0: a moved aim breaks the lock)
        let locked = self.lock.locked();
        let cand = if locked.is_some() && cand.is_none() && aim.is_some_and(|(p, _, _)| self.point.is_some_and(|q| (p - q).len() <= JAVELIN_RADIUS_M)) {
            locked
        } else {
            cand
        };
        self.lock.tick(now, true, cand, JAVELIN_LOCK_S, 0.05)
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.point = None;
        self.lock.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectile_rocket_straight_line() {
        let mut p = Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.0, 1.0), 63.5, 12.7, 0.0, 7.0, 0.0, 76.2, Guidance::Straight);
        let mut t = 0.0;
        while t < 1.0 - 1e-6 {
            p.step(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
        assert!((p.pos.z - 63.5).abs() < 0.05 && p.pos.x.abs() < 1e-4 && p.pos.y.abs() < 1e-4, "{:?}", p.pos);
        // ignition: 1/10 speed first
        let mut q = Projectile::launch(1, 0, V3::ZERO, V3::new(1.0, 0.0, 0.0), 100.0, 0.0, 0.5, 7.0, 0.0, 76.2, Guidance::Straight);
        q.step(0.25);
        assert!((q.pos.x - 2.5).abs() < 0.05, "{:?}", q.pos);
    }

    #[test]
    fn projectile_m79_ballistics_apex_range_and_arming() {
        // 45 deg at 60.96 m/s: range v^2/g (~183 m) and apex v^2/(4g) +-0.5 %
        let v = 2400.0 * 0.0254;
        let d = V3::new(1.0, 1.0, 0.0);
        let mut p = Projectile::launch(1, 0, V3::ZERO, d, v, 0.0, 0.0, 30.0, 375.0 * 0.0254, 0.0, Guidance::Ballistic);
        let mut apex: f32 = 0.0;
        let mut prev = p.pos;
        let mut armed_at = None;
        for _ in 0..20_000 {
            prev = p.pos;
            p.step(1.0 / 240.0);
            apex = apex.max(p.pos.y);
            if armed_at.is_none() && p.armed() {
                armed_at = Some((p.pos - p.start).len());
            }
            if p.pos.y < 0.0 {
                break;
            }
        }
        let x = prev.x + (p.pos.x - prev.x) * (prev.y / (prev.y - p.pos.y));
        let range = v * v / GRAVITY_MPS2;
        assert!((x - range).abs() <= 0.005 * range, "range {x} vs {range}");
        let ap = v * v / (4.0 * GRAVITY_MPS2);
        assert!((apex - ap).abs() <= 0.005 * ap, "apex {apex} vs {ap}");
        let a = armed_at.unwrap();
        assert!((a - 9.525).abs() < 0.5, "arms after 9.525 m ({a})");
        // an impact before the arming distance is a dud
        let near = Projectile::launch(1, 0, V3::ZERO, V3::new(1.0, 0.0, 0.0), v, 0.0, 0.0, 30.0, 9.525, 0.0, Guidance::Ballistic);
        assert!(!near.armed());
    }

    #[test]
    fn projectile_lifetime_expiry() {
        let mut p = Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.0, 1.0), 10.0, 0.0, 0.0, 1.0, 0.0, 0.0, Guidance::Straight);
        // (one step is clamped to 0.25 s: a hitch does not teleport the projectile)
        for _ in 0..3 {
            assert!(!p.step(0.25).expired);
        }
        assert!(p.step(0.25).expired);
        assert!((p.pos.z - 10.0).abs() < 1e-3);
    }

    #[test]
    fn projectile_pn_hits_crossing_target_within_accel_cap() {
        let cap = 10000.0 * 0.0254;
        let mut p = Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.0, 1.0), 101.6, 0.0, 0.0, 7.0, 0.0, cap, Guidance::Pn { target: V3::ZERO });
        let mut tgt = V3::new(-30.0, 2.0, 120.0);
        let tv = V3::new(8.0, 0.0, 0.0);
        let dt = 1.0 / 120.0;
        let mut min_d = f32::MAX;
        let mut prev_v = p.vel;
        let mut max_a: f32 = 0.0;
        for _ in 0..1200 {
            tgt = tgt + tv * dt;
            p.guidance = Guidance::Pn { target: tgt };
            p.step(dt);
            let a = (p.vel - prev_v).len() / dt;
            max_a = max_a.max(a);
            prev_v = p.vel;
            min_d = min_d.min((p.pos - tgt).len());
            if min_d < 0.5 {
                break;
            }
        }
        assert!(min_d < 1.0, "miss distance {min_d}");
        assert!(max_a <= cap * 1.05, "lateral accel {max_a} <= {cap}");
    }

    #[test]
    fn projectile_javelin_apex_and_dive() {
        assert_eq!(javelin_apex_m(10.0), 20.0);
        assert_eq!(javelin_apex_m(1000.0), 120.0);
        assert!((javelin_apex_m(100.0) - 35.0).abs() < 1e-4);
        let target = V3::new(0.0, 0.0, 100.0);
        let apex_y = target.y + javelin_apex_m(100.0);
        let mut p = Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.3, 1.0), 40.0, 0.0, 0.0, 30.0, 0.0, 76.2, Guidance::TopAttack { target, apex_y, diving: false });
        let mut top: f32 = 0.0;
        let mut best = f32::MAX;
        for _ in 0..3000 {
            p.step(1.0 / 120.0);
            top = top.max(p.pos.y);
            best = best.min((p.pos - target).len());
            if best < 1.0 {
                break;
            }
        }
        assert!(top > 20.0, "climbs: {top}");
        assert!(best < 1.5, "dives onto the target: {best}");
        assert!(p.vel.y < 0.0, "arrives from above");
    }

    /// Flies a Javelin top attack at `range` m with the given boost; returns (time to within 1.5 m, apex).
    fn javelin_flight(range: f32, cruise: f32, accel: f32) -> (Option<f32>, f32) {
        let target = V3::new(0.0, 0.0, range);
        let apex_y = javelin_apex_m(range);
        let mut p = Projectile::launch(1, 0, V3::new(0.0, 1.5, 0.0), V3::new(0.0, 0.3, 1.0), 600.0 * 0.0254, 0.0, 0.5, 30.0, 0.0, 3000.0 * 0.0254, Guidance::TopAttack { target, apex_y, diving: false })
            .with_boost(cruise, accel);
        let (mut t, mut top) = (0.0f32, 0.0f32);
        for _ in 0..(30 * 120) {
            p.step(1.0 / 120.0);
            t += 1.0 / 120.0;
            top = top.max(p.pos.y);
            if (p.pos - target).len() < 1.5 {
                return (Some(t), top);
            }
        }
        (None, top)
    }

    #[test]
    fn projectile_javelin_time_to_target_bounded() {
        // review M6 fidelity: a 100 m top attack (apex 35 m) arrives in 2..5 s with the default boost (60 m/s,
        // 40 m/s^2) [I]; the old constant 15.2 m/s took ~10 s
        let (t, top) = javelin_flight(100.0, 60.0, 40.0);
        let t = t.expect("the boosted javelin reaches the target");
        assert!((2.0..=5.0).contains(&t), "time to target {t}");
        assert!(top > 25.0, "still a top attack: apex {top}");
        // every range arrives (no circling: the turn radius cap) and a longer shot never arrives sooner
        let mut prev = 0.0;
        for r in [30.0, 60.0, 100.0, 200.0] {
            let t = javelin_flight(r, 60.0, 40.0).0.unwrap_or(f32::MAX);
            assert!(t < 6.0 && t > prev, "range {r}: {t} s");
            prev = t;
        }
        let (slow, _) = javelin_flight(100.0, 0.0, 0.0);
        assert!(slow.is_none_or(|s| s > 7.0), "without a boost it cruises at launch speed: {slow:?}");
        // the speed never exceeds the cruise and is the eject speed before ignition
        let p = Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.0, 1.0), 15.24, 0.0, 0.5, 30.0, 0.0, 76.2, Guidance::Straight).with_boost(60.0, 40.0);
        assert!((p.current_speed() - 1.524).abs() < 1e-3);
        let mut q = p;
        q.t = 10.0;
        assert_eq!(q.current_speed(), 60.0);
        // a cruise below the launch speed is ignored
        assert_eq!(Projectile::launch(1, 0, V3::ZERO, V3::new(0.0, 0.0, 1.0), 63.5, 0.0, 0.0, 7.0, 0.0, 76.2, Guidance::Straight).with_boost(10.0, 40.0).boost_accel, 0.0);
    }

    #[test]
    fn projectile_self_blast_guard() {
        let feet = V3::new(0.0, 0.0, 0.0);
        // an RPG on a wall 2 m ahead (radius 10.2 m): the player is inside -> dud
        assert!(self_blast_distance(feet, 1.8, V3::new(0.0, 1.0, 2.0), 10.16, 0.5).is_some_and(|d| (d - 2.0).abs() < 1e-4));
        // 30 m away: no guard
        assert_eq!(self_blast_distance(feet, 1.8, V3::new(0.0, 1.0, 30.0), 10.16, 0.5), None);
        // the capsule counts: a blast 10.5 m above the head of a 1.8 m capsule with radius 10.16 + 0.5 reaches it
        assert!(self_blast_distance(feet, 1.8, V3::new(0.0, 12.0, 0.0), 10.16, 0.5).is_some());
        assert_eq!(self_blast_distance(feet, 1.8, V3::new(0.0, 12.5, 0.0), 10.16, 0.5), None);
    }

    #[test]
    fn lock_stinger_timer_and_grace() {
        let mut l = Lock::default();
        assert_eq!(l.tick(0.0, true, Some(7), STINGER_LOCK_S, STINGER_GRACE_S), Some(LockEvent::Start(7)));
        assert_eq!(l.tick(0.99, true, Some(7), STINGER_LOCK_S, STINGER_GRACE_S), None);
        assert_eq!(l.tick(1.0, true, Some(7), STINGER_LOCK_S, STINGER_GRACE_S), Some(LockEvent::Acquired(7)));
        assert_eq!(l.locked(), Some(7));
        // out of sight: kept 0.5 s, then lost
        assert_eq!(l.tick(1.1, true, None, STINGER_LOCK_S, STINGER_GRACE_S), None);
        assert_eq!(l.locked(), Some(7));
        assert_eq!(l.tick(1.59, true, None, STINGER_LOCK_S, STINGER_GRACE_S), None);
        assert_eq!(l.tick(1.61, true, None, STINGER_LOCK_S, STINGER_GRACE_S), Some(LockEvent::Lost { sight: true }));
        // leaving ADS drops the lock
        l.tick(2.0, true, Some(3), STINGER_LOCK_S, STINGER_GRACE_S);
        assert_eq!(l.tick(2.1, false, Some(3), STINGER_LOCK_S, STINGER_GRACE_S), Some(LockEvent::Lost { sight: false }));
    }

    #[test]
    fn lock_javelin_point_samples() {
        let mut j = PointLock::default();
        let p = V3::new(0.0, 0.0, 50.0);
        let mut t = 0.0;
        let mut acquired = None;
        while t < 3.0 {
            if let Some(LockEvent::Acquired(_)) = j.tick(t, true, Some((p, 50.0, None))) {
                acquired = Some(t);
                break;
            }
            t += 1.0 / 60.0;
        }
        // 12 samples at 25 ms (~0.3 s) then 1.15 s
        let a = acquired.expect("locks");
        assert!(a >= JAVELIN_LOCK_S + 0.25 && a <= JAVELIN_LOCK_S + 0.45, "{a}");
        // too close: never locks
        let mut j = PointLock::default();
        let mut t = 0.0;
        while t < 3.0 {
            assert!(!matches!(j.tick(t, true, Some((V3::new(0.0, 0.0, 10.0), 10.0, None))), Some(LockEvent::Acquired(_))));
            t += 1.0 / 60.0;
        }
    }
}
