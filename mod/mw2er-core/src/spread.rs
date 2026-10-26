//! Hip-fire spread (IW-style, reconstructed; ADS is M4).
//!
//! A scale `s` in [0, 1] rises by `hipSpreadFireAdd` per shot and with movement, and decays at
//! `hipSpreadDecayRate` per second. The cone half-angle is
//! `hipSpreadStandMin + (hipSpreadMax - hipSpreadStandMin) * s` degrees. Bullets are perturbed
//! inside that cone, so they stay inside the drawn crosshair. Behaviour is inference
//! (docs/M2_DESIGN.md section 10.4).

use crate::rng::Rng;
use crate::vec3::{UP, V3};

/// Planar speed (m/s) above which movement adds spread.
pub const MOVE_THRESHOLD_MPS: f32 = 0.5;
/// Planar speed (m/s) at which the movement add is at full strength.
pub const MOVE_FULL_MPS: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpreadParams {
    pub min_deg: f32,
    pub max_deg: f32,
    pub fire_add: f32,
    pub decay_per_s: f32,
    pub move_add: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpreadState {
    pub s: f32,
}

impl SpreadState {
    /// Per-tick update: movement adds, standing decays.
    pub fn tick(&mut self, p: &SpreadParams, dt: f32, planar_speed: f32, use_move: bool) {
        let dt = dt.max(0.0);
        if use_move && planar_speed > MOVE_THRESHOLD_MPS {
            self.s += p.move_add * (planar_speed / MOVE_FULL_MPS).clamp(0.0, 1.0) * dt;
        } else {
            self.s -= p.decay_per_s * dt;
        }
        self.s = self.s.clamp(0.0, 1.0);
    }

    pub fn on_shot(&mut self, p: &SpreadParams) {
        self.s = (self.s + p.fire_add).clamp(0.0, 1.0);
    }

    /// Current cone half-angle in degrees.
    pub fn deg(&self, p: &SpreadParams) -> f32 {
        p.min_deg + (p.max_deg - p.min_deg) * self.s.clamp(0.0, 1.0)
    }
}

/// M5: the stance values of the weapon file (UMP45: ducked 1.75-4.5, prone 1.5-4; ducked/prone decay x1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StanceSpread {
    pub ducked: [f32; 2],
    pub prone: [f32; 2],
    /// `hipSpreadDuckedDecay` / `hipSpreadProneDecay`: multipliers of the decay rate [I].
    pub ducked_decay: f32,
    pub prone_decay: f32,
    /// `bg_aimSpreadMoveSpeedThreshold` (m/s; MW2 11 in/s).
    pub move_threshold_mps: f32,
    /// `jump_spreadAdd` / 255.
    pub jump_add: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpreadStance {
    Stand,
    Ducked,
    Prone,
}

impl SpreadState {
    /// M5 per-tick update while the controller drives: the move add above MW2's 11 in/s threshold, otherwise the
    /// decay scaled per stance.
    pub fn tick_stance(&mut self, p: &SpreadParams, ss: &StanceSpread, st: SpreadStance, dt: f32, planar_speed: f32, use_move: bool) {
        let dt = dt.max(0.0);
        if use_move && planar_speed > ss.move_threshold_mps {
            self.s += p.move_add * (planar_speed / MOVE_FULL_MPS).clamp(0.0, 1.0) * dt;
        } else {
            let k = match st {
                SpreadStance::Stand => 1.0,
                SpreadStance::Ducked => ss.ducked_decay,
                SpreadStance::Prone => ss.prone_decay,
            };
            self.s -= p.decay_per_s * k * dt;
        }
        self.s = self.s.clamp(0.0, 1.0);
    }

    /// A jump adds `jump_spreadAdd` (64/255) to the scale.
    pub fn on_jump(&mut self, ss: &StanceSpread) {
        self.s = (self.s + ss.jump_add).clamp(0.0, 1.0);
    }

    /// The cone of the stance (the target stance during a transition).
    pub fn deg_stance(&self, p: &SpreadParams, ss: &StanceSpread, st: SpreadStance) -> f32 {
        let (lo, hi) = match st {
            SpreadStance::Stand => (p.min_deg, p.max_deg),
            SpreadStance::Ducked => (ss.ducked[0], ss.ducked[1]),
            SpreadStance::Prone => (ss.prone[0], ss.prone[1]),
        };
        lo + (hi - lo) * self.s.clamp(0.0, 1.0)
    }
}

/// Right/up basis for a forward vector (`right = fwd x Y`, `up = right x fwd`).
/// `None` when looking (almost) straight up or down.
pub fn basis(fwd: V3) -> Option<(V3, V3)> {
    let right = fwd.cross(UP).normalize()?;
    let up = right.cross(fwd).normalize()?;
    Some((right, up))
}

/// Random direction inside a cone of half-angle `spread_deg` around `fwd` (IW style: random
/// angle, radius uniform in [0, 1], so the pattern is centre-biased; deviation <= spread_deg).
pub fn perturb(fwd: V3, right: V3, up: V3, spread_deg: f32, rng: &mut Rng) -> V3 {
    let theta = std::f32::consts::TAU * rng.next_f32();
    let r = rng.next_f32();
    let offset = spread_deg.clamp(0.0, 89.0).to_radians().tan() * r;
    let d = fwd + right * (offset * theta.cos()) + up * (offset * theta.sin());
    d.normalize().unwrap_or(fwd)
}

/// M6 (section 4.4 step 3): the cone while aiming: the hip cone blended toward the gun's ADS spread (0 for most rifles,
/// 5.5 for the shotguns) by the ADS fraction (M2-M5 used the hip cone whatever the ADS state).
pub fn ads_cone(hip_deg: f32, ads_spread_deg: f32, ads_frac: f32) -> f32 {
    let a = ads_frac.clamp(0.0, 1.0);
    (hip_deg + (ads_spread_deg - hip_deg) * a).max(0.0)
}

/// M6 pellets (section 6.4): `n` independent directions inside the cone (the IW perturbation per pellet [I]).
pub fn pellet_dirs(fwd: V3, spread_deg: f32, n: u32, rng: &mut Rng, out: &mut Vec<V3>) {
    out.clear();
    match basis(fwd) {
        Some((right, up)) => {
            for _ in 0..n {
                out.push(perturb(fwd, right, up, spread_deg, rng));
            }
        }
        None => out.extend(std::iter::repeat_n(fwd, n as usize)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pellets_inside_cone_and_mean_radius() {
        let fwd = V3::new(0.0, 0.0, 1.0);
        let mut rng = Rng::new(42);
        let mut v = Vec::new();
        let deg = 7.0f32;
        let mut sum = 0.0f64;
        let mut count = 0;
        for _ in 0..1250 {
            pellet_dirs(fwd, deg, 8, &mut rng, &mut v);
            assert_eq!(v.len(), 8);
            for d in &v {
                let a = d.angle_deg(fwd);
                assert!(a <= deg + 1e-3, "pellet {a} outside the {deg} cone");
                sum += (a.to_radians().tan() / deg.to_radians().tan()) as f64;
                count += 1;
            }
        }
        // uniform radius in [0, 1]: mean 0.5 (+-5 %) over 10k pellets
        let mean = sum / count as f64;
        assert!((mean - 0.5).abs() <= 0.025, "mean radius {mean}");
    }

    #[test]
    fn pellets_ads_cone_lerp() {
        assert_eq!(ads_cone(7.0, 5.5, 0.0), 7.0);
        assert_eq!(ads_cone(7.0, 5.5, 1.0), 5.5);
        assert!((ads_cone(4.0, 0.0, 0.5) - 2.0).abs() < 1e-6);
        assert_eq!(ads_cone(4.0, 0.0, 2.0), 0.0, "clamped");
    }

    fn ump() -> SpreadParams {
        SpreadParams { min_deg: 2.0, max_deg: 5.0, fire_add: 0.52, decay_per_s: 4.0, move_add: 4.0 }
    }

    #[test]
    fn spread_min_at_rest_and_add_per_shot() {
        let p = ump();
        let mut s = SpreadState::default();
        assert_eq!(s.deg(&p), 2.0);
        s.on_shot(&p);
        assert!((s.s - 0.52).abs() < 1e-6);
        s.on_shot(&p);
        assert_eq!(s.s, 1.0, "clamped at 1");
        assert_eq!(s.deg(&p), 5.0);
    }

    #[test]
    fn spread_decays_to_min_within_quarter_second() {
        let p = ump();
        let mut s = SpreadState { s: 1.0 };
        for _ in 0..16 {
            s.tick(&p, 1.0 / 60.0, 0.0, true);
        }
        assert!(s.s <= 1e-5, "s = {}", s.s);
        assert!((s.deg(&p) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn spread_sustained_fire_stays_in_range() {
        let p = ump();
        let mut s = SpreadState::default();
        let mut t = 0.0f32;
        let mut next = 0.0f32;
        let mut seen_min = f32::MAX;
        for _ in 0..600 {
            s.tick(&p, 1.0 / 60.0, 0.0, true);
            if t >= next {
                s.on_shot(&p);
                next += 0.09;
            }
            let d = s.deg(&p);
            assert!((2.0..=5.0).contains(&d));
            if t > 1.0 {
                seen_min = seen_min.min(s.s);
            }
            t += 1.0 / 60.0;
        }
        assert!(seen_min > 0.5, "sustained fire keeps the cone wide ({seen_min})");
    }

    #[test]
    fn spread_movement_adds() {
        let p = ump();
        let mut s = SpreadState::default();
        s.tick(&p, 0.1, 6.0, true);
        assert!((s.s - 0.4).abs() < 1e-5);
        let mut s2 = SpreadState::default();
        s2.tick(&p, 0.1, 6.0, false);
        assert_eq!(s2.s, 0.0);
    }

    #[test]
    fn spread_perturb_stays_inside_cone() {
        let fwd = V3::new(0.3, -0.2, 0.9).normalize().unwrap();
        let (right, up) = basis(fwd).unwrap();
        assert!(right.dot(fwd).abs() < 1e-5 && up.dot(fwd).abs() < 1e-5);
        let mut rng = Rng::new(7);
        let mut max_dev = 0.0f32;
        for _ in 0..10_000 {
            let d = perturb(fwd, right, up, 5.0, &mut rng);
            max_dev = max_dev.max(d.angle_deg(fwd));
        }
        assert!(max_dev <= 5.0 + 1e-3, "max deviation {max_dev}");
        assert!(max_dev > 4.5, "the cone is actually used ({max_dev})");
        assert!(basis(V3::new(0.0, 1.0, 0.0)).is_none());
    }

    fn ss() -> StanceSpread {
        StanceSpread { ducked: [1.75, 4.5], prone: [1.5, 4.0], ducked_decay: 1.0, prone_decay: 1.0, move_threshold_mps: 11.0 * 0.0254, jump_add: 64.0 / 255.0 }
    }

    #[test]
    fn spread_stance_min_max_per_stance() {
        let p = ump();
        let mut s = SpreadState::default();
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Stand), 2.0);
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Ducked), 1.75);
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Prone), 1.5);
        s.s = 1.0;
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Stand), 5.0);
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Ducked), 4.5);
        assert_eq!(s.deg_stance(&p, &ss(), SpreadStance::Prone), 4.0);
    }

    #[test]
    fn spread_stance_jump_add_and_move_threshold() {
        let p = ump();
        let mut s = SpreadState::default();
        s.on_jump(&ss());
        assert!((s.s - 64.0 / 255.0).abs() < 1e-6);
        for _ in 0..4 {
            s.on_jump(&ss());
        }
        assert_eq!(s.s, 1.0, "clamped");
        // 10 in/s is below the 11 in/s threshold: decays; 12 in/s adds
        let mut a = SpreadState { s: 0.5 };
        a.tick_stance(&p, &ss(), SpreadStance::Stand, 0.1, 10.0 * 0.0254, true);
        assert!(a.s < 0.5);
        let mut b = SpreadState { s: 0.5 };
        b.tick_stance(&p, &ss(), SpreadStance::Stand, 0.1, 12.0 * 0.0254, true);
        assert!(b.s > 0.5);
        // ducked decay multiplier
        let half = StanceSpread { ducked_decay: 0.5, ..ss() };
        let mut c = SpreadState { s: 1.0 };
        c.tick_stance(&p, &half, SpreadStance::Ducked, 0.1, 0.0, true);
        assert!((c.s - 0.8).abs() < 1e-5);
    }
}
