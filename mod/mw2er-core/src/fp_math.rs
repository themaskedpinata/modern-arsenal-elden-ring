//! First-person geometry (docs/M3B_DESIGN.md sections 2.5 and 4). Pure; Elden Ring world space (metres, +Y up).
//!
//! - [`eye_position`]: the render camera position (feet + eye height + a small horizontal forward offset).
//! - [`clamp_pitch`]: an FPS-style pitch limit on the camera rows (a no-op while ER's own limit is narrower).
//! - Yaw: [`quat_from_yaw`], [`forward_of`], [`yaw_from_quat`] for the character's facing quaternion (XYZW).
//! - [`YawProbe`]: learns whether the character's forward axis is +Z or -Z from samples taken while W is held.
//! - [`yaw_gate`]: when a facing write is allowed (never while strafing, dodging, turn-locked, locked on, thrown).
//! - [`DriftStats`]: how far the game turned the character between two of our writes.

use crate::vec3::{UP, V3};

/// Below this horizontal length the view is (nearly) vertical and has no horizontal forward.
pub const MIN_PLANAR: f32 = 1e-3;

/// Unit horizontal part of a direction, `None` when the direction is (nearly) vertical or not finite.
pub fn horizontal_forward(fwd: V3) -> Option<V3> {
    let h = V3::new(fwd.x, 0.0, fwd.z);
    if !h.is_finite() || h.len() < MIN_PLANAR {
        return None;
    }
    h.normalize()
}

/// `feet + (0, eye_height_m, 0) + horizontal(fwd) * eye_forward_m`. A vertical view gets no forward offset.
pub fn eye_position(feet: V3, fwd: V3, eye_height_m: f32, eye_forward_m: f32) -> V3 {
    let base = feet + V3::new(0.0, eye_height_m, 0.0);
    match horizontal_forward(fwd) {
        Some(h) => base + h * eye_forward_m,
        None => base,
    }
}

/// Exponential filter on the eye height (stairs). `tau_s <= 0` = off. A jump of more than 1 m resets it.
#[derive(Clone, Copy, Debug, Default)]
pub struct EyeSmoother {
    y: Option<f32>,
}

impl EyeSmoother {
    pub fn reset(&mut self) {
        self.y = None;
    }

    pub fn step(&mut self, target_y: f32, dt_s: f32, tau_s: f32) -> f32 {
        let y = match self.y {
            Some(prev) if tau_s > 0.0 && (target_y - prev).abs() <= 1.0 && dt_s.is_finite() && dt_s >= 0.0 => {
                let a = 1.0 - (-dt_s / tau_s).exp();
                prev + (target_y - prev) * a
            }
            _ => target_y,
        };
        self.y = Some(y);
        y
    }
}

/// Pitch of a forward vector in degrees (positive = up).
pub fn pitch_deg(fwd: V3) -> f32 {
    fwd.normalize().map_or(0.0, |f| f.y.clamp(-1.0, 1.0).asin().to_degrees())
}

fn v(a: [f32; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}

fn arr(a: V3) -> [f32; 3] {
    [a.x, a.y, a.z]
}

/// Camera rows `[right, up, forward]`. Returns new rows when |pitch| exceeds `max_deg`, `None` when no change
/// is needed (or the rows are degenerate). The new rows are orthonormal and keep the sign of each original row,
/// so the determinant keeps its sign.
pub fn clamp_pitch(rows: [[f32; 3]; 3], max_deg: f32) -> Option<[[f32; 3]; 3]> {
    let f = v(rows[2]).normalize()?;
    let p = f.y.clamp(-1.0, 1.0).asin();
    if !(max_deg.is_finite() && max_deg > 0.0) || p.abs().to_degrees() <= max_deg {
        return None;
    }
    let up_row = v(rows[1]);
    // Horizontal heading: from the forward row, or from the up row when the view is exactly vertical
    // (looking up with pitch p: up = (-sin p * h, cos p); so cos p * f_h - sin p * up_h = h).
    let fh = V3::new(f.x, 0.0, f.z);
    let uh = V3::new(up_row.x, 0.0, up_row.z);
    let h = (fh * p.cos() - uh * p.sin()).normalize()?;
    let pc = max_deg.to_radians().copysign(p);
    let nf = h * pc.cos() + UP * pc.sin();
    let mut r = UP.cross(nf).normalize()?;
    if r.dot(v(rows[0])) < 0.0 {
        r = -r;
    }
    let mut u = nf.cross(r).normalize()?;
    if u.dot(up_row) < 0.0 {
        u = -u;
    }
    Some([arr(r), arr(u), arr(nf)])
}

// ---- yaw --------------------------------------------------------------------------------------------------

/// The character's local forward axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    PlusZ,
    MinusZ,
}

impl Axis {
    pub fn label(self) -> &'static str {
        match self {
            Axis::PlusZ => "+z",
            Axis::MinusZ => "-z",
        }
    }

    fn local(self) -> V3 {
        match self {
            Axis::PlusZ => V3::new(0.0, 0.0, 1.0),
            Axis::MinusZ => V3::new(0.0, 0.0, -1.0),
        }
    }
}

/// Yaw (radians, about +Y) that turns the local `axis` onto the horizontal direction `h`.
pub fn yaw_of(h: V3, axis: Axis) -> f32 {
    match axis {
        Axis::PlusZ => h.x.atan2(h.z),
        Axis::MinusZ => (-h.x).atan2(-h.z),
    }
}

/// A yaw-only quaternion, XYZW.
pub fn quat_from_yaw(yaw: f32) -> [f32; 4] {
    let (s, c) = (yaw * 0.5).sin_cos();
    [0.0, s, 0.0, c]
}

/// The 32 bytes the yaw path writes in ONE write: `CSChrPhysicsModule.orientation` (+0x50) followed by
/// `interpolated_orientation` (+0x60), both the same yaw quaternion. Run 5 showed ~70 ms per yaw tick: two separate
/// writes, each paying a region query on ER's multi-GB heap (fixed in the write helper; one write halves the rest).
pub fn yaw_pair(q: [f32; 4]) -> [[f32; 4]; 2] {
    [q, q]
}

/// Rotates `p` by the unit quaternion `q` (XYZW).
pub fn rotate(q: [f32; 4], p: V3) -> V3 {
    let u = V3::new(q[0], q[1], q[2]);
    let w = q[3];
    let t = u.cross(p) * 2.0;
    p + t * w + u.cross(t)
}

/// The world direction of the local forward `axis` under `q`.
pub fn forward_of(q: [f32; 4], axis: Axis) -> V3 {
    rotate(q, axis.local())
}

/// The yaw of a facing quaternion, `None` for a non-finite or degenerate quaternion.
pub fn yaw_from_quat(q: [f32; 4], axis: Axis) -> Option<f32> {
    if !q.iter().all(|x| x.is_finite()) {
        return None;
    }
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if !(0.5..=2.0).contains(&n) {
        return None;
    }
    let qn = [q[0] / n, q[1] / n, q[2] / n, q[3] / n];
    horizontal_forward(forward_of(qn, axis)).map(|h| yaw_of(h, axis))
}

/// Signed difference a - b in degrees, wrapped into -180..180.
pub fn angle_diff_deg(a_rad: f32, b_rad: f32) -> f32 {
    let mut d = (a_rad - b_rad).to_degrees() % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Outcome of a probe attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProbeEvent {
    Resolved { axis: Axis, dot_med: f32, n: usize },
    Failed { dot_med: f32, n: usize, attempt: u32 },
    GaveUp,
}

/// Learns the character's forward axis: while W alone is held and the character runs (ER turns it toward the
/// input, i.e. the camera's horizontal forward), the dot of the camera heading with q*(0,0,+1) is +1 for a +Z
/// character and -1 for a -Z one.
#[derive(Clone, Debug)]
pub struct YawProbe {
    need: usize,
    samples: Vec<f32>,
    attempts: u32,
    result: Option<Axis>,
    gave_up: bool,
}

pub const PROBE_MAX_ATTEMPTS: u32 = 3;
pub const PROBE_DOT_MIN: f32 = 0.9;

impl YawProbe {
    pub fn new(need: usize) -> Self {
        YawProbe { need: need.max(1), samples: Vec::with_capacity(need.max(1)), attempts: 0, result: None, gave_up: false }
    }

    pub fn result(&self) -> Option<Axis> {
        self.result
    }

    pub fn gave_up(&self) -> bool {
        self.gave_up
    }

    pub fn pending(&self) -> usize {
        self.samples.len()
    }

    /// Feeds one sample (camera heading, character quaternion). Returns an event when an attempt completes.
    pub fn feed(&mut self, cam_fwd: V3, q: [f32; 4]) -> Option<ProbeEvent> {
        if self.result.is_some() || self.gave_up {
            return None;
        }
        let (Some(h), Some(c)) = (horizontal_forward(cam_fwd), horizontal_forward(forward_of(q, Axis::PlusZ))) else {
            return None;
        };
        if !q.iter().all(|x| x.is_finite()) {
            return None;
        }
        self.samples.push(h.dot(c));
        if self.samples.len() < self.need {
            return None;
        }
        let n = self.samples.len();
        let dot_med = median(&mut self.samples);
        self.samples.clear();
        self.attempts += 1;
        if dot_med > PROBE_DOT_MIN {
            self.result = Some(Axis::PlusZ);
            Some(ProbeEvent::Resolved { axis: Axis::PlusZ, dot_med, n })
        } else if dot_med < -PROBE_DOT_MIN {
            self.result = Some(Axis::MinusZ);
            Some(ProbeEvent::Resolved { axis: Axis::MinusZ, dot_med, n })
        } else if self.attempts >= PROBE_MAX_ATTEMPTS {
            self.gave_up = true;
            Some(ProbeEvent::GaveUp)
        } else {
            Some(ProbeEvent::Failed { dot_med, n, attempt: self.attempts })
        }
    }
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        f32::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) * 0.5
    }
}

// ---- yaw gate ---------------------------------------------------------------------------------------------

/// `CSChrActionFlagModule.action_modifiers_flags` bits that forbid a facing write (action_flag.rs:166-240).
pub const AF_DODGING: u32 = 1;
pub const AF_DISABLE_TURNING: u32 = 15;
pub const AF_DISABLE_DIRECTION_CHANGE: u32 = 32;
pub const AF_TURN_LOWER_BODY: u32 = 46;

/// Why a facing write was skipped this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YawPause {
    Lateral,
    Dodge,
    TurnLock,
    LockOn,
    Throw,
    /// Moving clearly away from the camera heading with no A/S/D key: a gamepad stick or remapped keys are turning
    /// the character (review M3b minor 9).
    OffHeading,
}

impl YawPause {
    pub fn label(self) -> &'static str {
        match self {
            YawPause::Lateral => "lateral",
            YawPause::Dodge => "dodge",
            YawPause::TurnLock => "turn_lock",
            YawPause::LockOn => "lockon",
            YawPause::Throw => "throw",
            YawPause::OffHeading => "off_heading",
        }
    }

    pub const ALL: [YawPause; 6] =
        [YawPause::Lateral, YawPause::Dodge, YawPause::TurnLock, YawPause::LockOn, YawPause::Throw, YawPause::OffHeading];
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct YawGateIn {
    /// A, S or D held (W alone is fine: it already turns the character toward the camera).
    pub lateral_keys: bool,
    pub action_flags: u64,
    pub locked_on: bool,
    pub throw_active: bool,
    /// [`off_heading`] this tick.
    pub off_heading: bool,
    /// M5: the MW2 movement controller drives in first person. ER no longer sees A/S/D and the strafing is ours, not
    /// ER steering the character, so the lateral and off-heading pauses do not apply (docs/M5_DESIGN.md section 7).
    pub controller_driving: bool,
}

/// Planar speed above which the movement direction is compared with the camera heading.
pub const OFF_HEADING_MIN_SPEED_MPS: f32 = 1.0;
/// Movement further than this from the camera heading means something other than W is steering the character.
pub const OFF_HEADING_MAX_DEG: f32 = 45.0;

/// True when the character moves (planar velocity `vel`, m/s) faster than [`OFF_HEADING_MIN_SPEED_MPS`] in a
/// direction more than [`OFF_HEADING_MAX_DEG`] away from the horizontal camera heading `h`. The raw-key gate only
/// knows W/A/S/D; this catches a gamepad stick or remapped keys, where ER turns the character toward the input and a
/// facing write would fight it.
pub fn off_heading(vel: V3, h: V3) -> bool {
    let v = V3::new(vel.x, 0.0, vel.z);
    if !v.is_finite() || v.planar_len() <= OFF_HEADING_MIN_SPEED_MPS {
        return false;
    }
    v.angle_deg(V3::new(h.x, 0.0, h.z)) > OFF_HEADING_MAX_DEG
}

fn bit(flags: u64, b: u32) -> bool {
    flags >> b & 1 == 1
}

/// `Ok` when a facing write is allowed, else the first reason in a fixed order.
pub fn yaw_gate(i: &YawGateIn) -> Result<(), YawPause> {
    if i.throw_active {
        return Err(YawPause::Throw);
    }
    if bit(i.action_flags, AF_DODGING) {
        return Err(YawPause::Dodge);
    }
    if bit(i.action_flags, AF_DISABLE_TURNING)
        || bit(i.action_flags, AF_DISABLE_DIRECTION_CHANGE)
        || bit(i.action_flags, AF_TURN_LOWER_BODY)
    {
        return Err(YawPause::TurnLock);
    }
    if i.locked_on {
        return Err(YawPause::LockOn);
    }
    if i.lateral_keys && !i.controller_driving {
        return Err(YawPause::Lateral);
    }
    if i.off_heading && !i.controller_driving {
        return Err(YawPause::OffHeading);
    }
    Ok(())
}

/// Absolute yaw drift samples (degrees) between our write and the next tick's read; nearest-rank percentiles.
#[derive(Clone, Debug, Default)]
pub struct DriftStats {
    v: Vec<f32>,
}

impl DriftStats {
    pub fn push(&mut self, deg: f32) {
        if deg.is_finite() && self.v.len() < 100_000 {
            self.v.push(deg.abs());
        }
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    /// (p50, p95, n) and clears; `None` when empty.
    pub fn take(&mut self) -> Option<(f32, f32, usize)> {
        if self.v.is_empty() {
            return None;
        }
        self.v.sort_by(|a, b| a.total_cmp(b));
        let n = self.v.len();
        let rank = |p: f32| self.v[(((p * n as f32).ceil() as usize).max(1) - 1).min(n - 1)];
        let r = (rank(0.5), rank(0.95), n);
        self.v.clear();
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cam_gate::{det3, ortho_err};

    fn close(a: V3, b: V3, eps: f32) -> bool {
        (a - b).len() <= eps
    }

    /// Camera rows for yaw `y` and pitch `p` (radians): right, up, forward with det +1.
    fn rows_for(y: f32, p: f32) -> [[f32; 3]; 3] {
        let h = V3::new(y.sin(), 0.0, y.cos());
        let f = h * p.cos() + UP * p.sin();
        let r = UP.cross(h).normalize().unwrap();
        let u = f.cross(r);
        [arr(r), arr(u), arr(f)]
    }

    #[test]
    fn fp_math_eye_known_answers() {
        let feet = V3::new(10.0, -3.0, 5.0);
        let e = eye_position(feet, V3::new(0.0, 0.0, 1.0), 1.6, 0.1);
        assert!(close(e, V3::new(10.0, -1.4, 5.1), 1e-5), "{e:?}");
        // pitch does not move the eye: only the horizontal heading counts
        let f = V3::new(0.6, -0.64, 0.48).normalize().unwrap();
        let e = eye_position(feet, f, 1.6, 0.1);
        // horizontal (0.6, 0.48) / 0.7684 = (0.7809, 0.6247) -> +0.1 m along it
        assert!(close(e, V3::new(10.07809, -1.4, 5.06247), 1e-4), "{e:?}");
        // straight up / down: no forward offset
        assert!(close(eye_position(feet, V3::new(0.0, 1.0, 0.0), 1.6, 0.1), V3::new(10.0, -1.4, 5.0), 1e-6));
        assert!(close(eye_position(feet, V3::new(0.0, -1.0, 0.0), 1.6, 0.1), V3::new(10.0, -1.4, 5.0), 1e-6));
        // eye-to-chest (chest = feet + 1.2) is ~0.41 m with the defaults: the gate (min 0.3 m) never sees it
        let chest = feet + V3::new(0.0, 1.2, 0.0);
        let d = (eye_position(feet, V3::new(1.0, 0.0, 0.0), 1.6, 0.1) - chest).len();
        assert!((d - 0.4123).abs() < 1e-3, "{d}");
    }

    #[test]
    fn fp_math_eye_smoothing() {
        let mut s = EyeSmoother::default();
        assert_eq!(s.step(1.0, 0.016, 0.1), 1.0, "first sample passes through");
        let y = s.step(1.2, 0.1, 0.1);
        assert!((y - (1.0 + 0.2 * (1.0 - (-1.0f32).exp()))).abs() < 1e-5, "{y}");
        assert_eq!(s.step(5.0, 0.016, 0.1), 5.0, "a jump > 1 m resets");
        assert_eq!(s.step(5.3, 0.016, 0.0), 5.3, "tau 0 = off");
    }

    #[test]
    fn fp_math_pitch_clamp() {
        // below the limit: no change
        for p in [-60.0f32, 0.0, 40.0, 84.9] {
            assert!(clamp_pitch(rows_for(0.7, p.to_radians()), 85.0).is_none(), "{p}");
        }
        // above: rebuilt to exactly the limit, orthonormal, det kept, heading kept
        for (yaw, p) in [(0.7f32, 89.0f32), (-2.0, -88.0), (3.0, 89.9)] {
            let rows = rows_for(yaw, p.to_radians());
            let det0 = det3(&rows);
            let n = clamp_pitch(rows, 85.0).expect("clamped");
            assert!((pitch_deg(v(n[2])) - 85.0f32.copysign(p)).abs() < 1e-3, "{:?}", n);
            assert!(ortho_err(&n) < 1e-5);
            assert!((det3(&n) - det0).abs() < 1e-4, "det {} vs {}", det3(&n), det0);
            let h = horizontal_forward(v(n[2])).unwrap();
            assert!((yaw_of(h, Axis::PlusZ) - yaw).abs() < 1e-3);
        }
        // exactly vertical: the heading comes from the up row
        let rows = rows_for(1.0, 90f32.to_radians());
        let n = clamp_pitch(rows, 85.0).expect("clamped");
        assert!((yaw_of(horizontal_forward(v(n[2])).unwrap(), Axis::PlusZ) - 1.0).abs() < 1e-3);
        // a left-handed (det -1) camera keeps det -1
        let mut lh = rows_for(0.3, 88f32.to_radians());
        lh[0] = [-lh[0][0], -lh[0][1], -lh[0][2]];
        let n = clamp_pitch(lh, 85.0).unwrap();
        assert!((det3(&n) + 1.0).abs() < 1e-4);
    }

    #[test]
    fn fp_math_yaw_quat_round_trip() {
        for axis in [Axis::PlusZ, Axis::MinusZ] {
            for deg in [-179.0f32, -90.0, -30.0, 0.0, 45.0, 135.0, 180.0] {
                let h = V3::new(deg.to_radians().sin(), 0.0, deg.to_radians().cos());
                let q = quat_from_yaw(yaw_of(h, axis));
                assert!(close(forward_of(q, axis), h, 1e-5), "{axis:?} {deg}");
                let back = yaw_from_quat(q, axis).unwrap();
                assert!(angle_diff_deg(back, yaw_of(h, axis)).abs() < 1e-3);
                let n = q.iter().map(|x| x * x).sum::<f32>();
                assert!((n - 1.0).abs() < 1e-5);
            }
        }
        assert!(yaw_from_quat([f32::NAN, 0.0, 0.0, 1.0], Axis::PlusZ).is_none());
        // the single 32-byte yaw write: orientation then interpolated_orientation, both the same quaternion
        let q = quat_from_yaw(0.7);
        assert_eq!(yaw_pair(q), [q, q]);
        assert_eq!(std::mem::size_of_val(&yaw_pair(q)), 32);
        assert!(yaw_from_quat([0.0; 4], Axis::PlusZ).is_none());
        assert!((angle_diff_deg(179f32.to_radians(), (-179f32).to_radians()) + 2.0).abs() < 1e-3);
    }

    #[test]
    fn fp_math_yaw_probe() {
        let cam = V3::new(0.3, -0.2, 0.9).normalize().unwrap();
        let h = horizontal_forward(cam).unwrap();
        for axis in [Axis::PlusZ, Axis::MinusZ] {
            let mut p = YawProbe::new(20);
            let q = quat_from_yaw(yaw_of(h, axis));
            let mut ev = None;
            for _ in 0..20 {
                ev = p.feed(cam, q);
            }
            match ev {
                Some(ProbeEvent::Resolved { axis: a, dot_med, n }) => {
                    assert_eq!(a, axis);
                    assert_eq!(n, 20);
                    assert!(dot_med.abs() > 0.99);
                }
                e => panic!("{e:?}"),
            }
            assert_eq!(p.result(), Some(axis));
            assert_eq!(p.feed(cam, q), None, "resolved probes ignore samples");
        }
        // a character facing sideways (strafing) never resolves; 3 attempts then give up
        let mut p = YawProbe::new(5);
        let side = quat_from_yaw(yaw_of(h, Axis::PlusZ) + std::f32::consts::FRAC_PI_2);
        let mut events = Vec::new();
        for _ in 0..15 {
            if let Some(e) = p.feed(cam, side) {
                events.push(e);
            }
        }
        assert!(matches!(events[0], ProbeEvent::Failed { attempt: 1, .. }), "{events:?}");
        assert!(matches!(events[1], ProbeEvent::Failed { attempt: 2, .. }));
        assert_eq!(events[2], ProbeEvent::GaveUp);
        assert!(p.gave_up() && p.result().is_none());
    }

    #[test]
    fn fp_math_yaw_gate_pauses() {
        assert_eq!(yaw_gate(&YawGateIn::default()), Ok(()));
        let base = YawGateIn::default();
        assert_eq!(yaw_gate(&YawGateIn { lateral_keys: true, ..base }), Err(YawPause::Lateral));
        assert_eq!(yaw_gate(&YawGateIn { action_flags: 1 << AF_DODGING, ..base }), Err(YawPause::Dodge));
        for b in [AF_DISABLE_TURNING, AF_DISABLE_DIRECTION_CHANGE, AF_TURN_LOWER_BODY] {
            assert_eq!(yaw_gate(&YawGateIn { action_flags: 1 << b, ..base }), Err(YawPause::TurnLock), "bit {b}");
        }
        assert_eq!(yaw_gate(&YawGateIn { locked_on: true, ..base }), Err(YawPause::LockOn));
        assert_eq!(yaw_gate(&YawGateIn { throw_active: true, lateral_keys: true, ..base }), Err(YawPause::Throw));
        // unrelated bits do not pause
        assert_eq!(yaw_gate(&YawGateIn { action_flags: 1 << 0 | 1 << 2 | 1 << 33, ..base }), Ok(()));
        assert_eq!(yaw_gate(&YawGateIn { off_heading: true, ..base }), Err(YawPause::OffHeading));
        assert_eq!(yaw_gate(&YawGateIn { off_heading: true, lateral_keys: true, ..base }), Err(YawPause::Lateral));
        // gamepad: running sideways (90 deg) or backwards with no key held pauses; along the heading does not
        let h = V3::new(0.0, 0.0, 1.0);
        assert!(!off_heading(V3::new(0.0, -3.0, 4.0), h), "running along the heading (vertical speed ignored)");
        assert!(!off_heading(V3::new(2.0, 0.0, 3.0), h), "33.7 deg: W while the mouse turns");
        assert!(off_heading(V3::new(4.0, 0.0, 0.0), h), "stick sideways");
        assert!(off_heading(V3::new(0.0, 0.0, -4.0), h), "stick back");
        assert!(!off_heading(V3::new(0.9, 0.0, -0.1), h), "slower than 1 m/s: no verdict");
        assert!(!off_heading(V3::new(f32::NAN, 0.0, 4.0), h));
        assert_eq!(YawPause::ALL.map(|p| p.label()), ["lateral", "dodge", "turn_lock", "lockon", "throw", "off_heading"]);
    }

    #[test]
    fn fp_math_drift_percentiles() {
        let mut d = DriftStats::default();
        assert!(d.take().is_none());
        for i in 1..=100 {
            d.push(-(i as f32));
        }
        d.push(f32::NAN);
        assert_eq!(d.take(), Some((50.0, 95.0, 100)));
        assert!(d.is_empty());
    }

    #[test]
    fn fp_math_yaw_gate_controller_ignores_lateral_but_not_lock_on() {
        let base = YawGateIn { controller_driving: true, ..YawGateIn::default() };
        assert_eq!(yaw_gate(&YawGateIn { lateral_keys: true, ..base }), Ok(()), "A/S/D do not pause while driving");
        assert_eq!(yaw_gate(&YawGateIn { off_heading: true, ..base }), Ok(()));
        assert_eq!(yaw_gate(&YawGateIn { locked_on: true, lateral_keys: true, ..base }), Err(YawPause::LockOn));
        assert_eq!(yaw_gate(&YawGateIn { throw_active: true, ..base }), Err(YawPause::Throw));
        assert_eq!(yaw_gate(&YawGateIn { action_flags: 1 << AF_DODGING, ..base }), Err(YawPause::Dodge));
        assert_eq!(yaw_gate(&YawGateIn { lateral_keys: true, ..YawGateIn::default() }), Err(YawPause::Lateral), "unchanged when not driving");
    }
}
