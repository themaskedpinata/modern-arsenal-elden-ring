//! M6 recoil: view kick, ADS sway and hold breath, gun kick (docs/M6_DESIGN.md sections 6.5 and 8). [I] models
//! with the weapon-file numbers; every scale is a config value.
//!
//! - **View kick** per shot: `vel += (U(pitchMin, pitchMax), U(yawMin, yawMax))` deg/s (hip/ADS lerped by the ADS
//!   fraction; min may exceed max), then per frame a constant recentre deceleration `center_speed` deg/s^2 toward
//!   zero; the angle never crosses zero. One impulse `v` peaks at `v^2 / (2 c)`.
//! - **ADS sway**: a Lissajous wander of amplitude `k_sway * adsIdleAmount` degrees at `adsIdleSpeed`, scaled by the
//!   ADS fraction and the breath scale.
//! - **Hold breath** (scoped guns, the sprint key while fully scoped; iw4sp defaults hold 4.5 s, gasp 1 s, gasp
//!   scale 4.5, lerps 4 / 6 per second): the breath scale goes to ~0 while held, to 4.5 while gasping.
//! - **Gun kick**: the viewmodel pushed back along its barrel and recovering (a damped spring [I]).

use crate::arsenal::KickParams;
use crate::rng::Rng;

/// Per-axis kick: angle (deg) and velocity (deg/s).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KickAxis {
    pub angle: f32,
    pub vel: f32,
}

impl KickAxis {
    /// Constant deceleration toward 0 (magnitude `c` deg/s^2); never crosses zero.
    pub fn tick(&mut self, dt: f32, c: f32) {
        if dt <= 0.0 || (self.angle == 0.0 && self.vel == 0.0) {
            return;
        }
        let toward = if self.angle != 0.0 { -self.angle.signum() } else { -self.vel.signum() };
        let a = toward * c.max(0.0);
        let old = self.angle;
        self.angle += self.vel * dt + 0.5 * a * dt * dt;
        self.vel += a * dt;
        if old != 0.0 && (self.angle.signum() != old.signum() || self.angle == 0.0) {
            self.angle = 0.0;
            self.vel = 0.0;
        }
        if !self.angle.is_finite() || !self.vel.is_finite() {
            *self = KickAxis::default();
        }
    }
}

/// View kick state: pitch (positive = up) and yaw (positive = right) in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewKick {
    pub pitch: KickAxis,
    pub yaw: KickAxis,
    center: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Uniform in [a, b] (either order).
fn uni(a: f32, b: f32, rng: &mut Rng) -> f32 {
    a + (b - a) * rng.next_f32()
}

impl ViewKick {
    pub fn on_shot(&mut self, k: &KickParams, ads: f32, scale: f32, rng: &mut Rng) {
        let p = [0, 1, 2, 3].map(|i| lerp(k.hip[i], k.ads[i], ads));
        self.center = lerp(k.hip_center, k.ads_center, ads);
        self.pitch.vel += uni(p[0], p[1], rng) * scale;
        self.yaw.vel += uni(p[2], p[3], rng) * scale;
    }

    pub fn tick(&mut self, dt: f32) {
        let c = if self.center > 0.0 { self.center } else { 1000.0 };
        self.pitch.tick(dt, c);
        self.yaw.tick(dt, c);
    }

    /// (pitch, yaw) in degrees.
    pub fn angles(&self) -> (f32, f32) {
        (self.pitch.angle, self.yaw.angle)
    }

    pub fn reset(&mut self) {
        *self = ViewKick::default();
    }
}

/// Peak angle of one impulse `v` against recentre `c`: `v^2 / (2 c)`.
pub fn kick_peak(v: f32, c: f32) -> f32 {
    v * v / (2.0 * c.max(1e-6))
}

/// Hold-breath constants (iw4sp defaults [V SP, I MP equal]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BreathParams {
    pub hold_s: f32,
    pub gasp_s: f32,
    pub gasp_scale: f32,
    pub hold_lerp: f32,
    pub gasp_lerp: f32,
}

impl Default for BreathParams {
    fn default() -> Self {
        BreathParams { hold_s: 4.5, gasp_s: 1.0, gasp_scale: 4.5, hold_lerp: 4.0, gasp_lerp: 6.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreathPhase {
    Normal,
    Holding { since_ms: u64 },
    Gasping { since_ms: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreathEvent {
    Hold,
    /// The hold ended: `timeout` (held too long) or `release`.
    Gasp { timeout: bool },
}

/// Hold-breath state machine and the sway amplitude scale it produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Breath {
    pub phase: BreathPhase,
    t: f64,
    phase_t0: f64,
    /// The sway amplitude multiplier (1 normal, ~0 holding, gasp_scale gasping).
    pub scale: f32,
    /// After ADS exit with the key held, a key-up is needed before the next hold (or sprint).
    need_key_up: bool,
}

impl Default for Breath {
    fn default() -> Self {
        Breath { phase: BreathPhase::Normal, t: 0.0, phase_t0: 0.0, scale: 1.0, need_key_up: false }
    }
}

impl Breath {
    /// One tick: `key` = the sprint key held, `scoped` = fully scoped on an overlay gun.
    pub fn tick(&mut self, now: f64, dt: f32, key: bool, scoped: bool, p: &BreathParams) -> Option<BreathEvent> {
        self.t = now;
        let mut ev = None;
        if !key {
            self.need_key_up = false;
        }
        match self.phase {
            BreathPhase::Normal => {
                if scoped && key && !self.need_key_up {
                    self.phase = BreathPhase::Holding { since_ms: 0 };
                    self.phase_t0 = now;
                    ev = Some(BreathEvent::Hold);
                }
            }
            BreathPhase::Holding { .. } => {
                let held = now - self.phase_t0;
                if !scoped || !key {
                    self.phase = BreathPhase::Gasping { since_ms: 0 };
                    self.phase_t0 = now;
                    ev = Some(BreathEvent::Gasp { timeout: false });
                    if !scoped && key {
                        self.need_key_up = true;
                    }
                } else if held >= p.hold_s as f64 {
                    self.phase = BreathPhase::Gasping { since_ms: 0 };
                    self.phase_t0 = now;
                    self.need_key_up = true;
                    ev = Some(BreathEvent::Gasp { timeout: true });
                } else {
                    self.phase = BreathPhase::Holding { since_ms: (held * 1000.0) as u64 };
                }
            }
            BreathPhase::Gasping { .. } => {
                let g = now - self.phase_t0;
                if g >= p.gasp_s as f64 {
                    self.phase = BreathPhase::Normal;
                } else {
                    self.phase = BreathPhase::Gasping { since_ms: (g * 1000.0) as u64 };
                }
            }
        }
        let (target, rate) = match self.phase {
            BreathPhase::Normal => (1.0, p.gasp_lerp),
            BreathPhase::Holding { .. } => (0.0, p.hold_lerp),
            BreathPhase::Gasping { .. } => (p.gasp_scale, p.gasp_lerp),
        };
        let k = 1.0 - (-rate * dt.max(0.0)).exp();
        self.scale += (target - self.scale) * k;
        if !self.scale.is_finite() {
            self.scale = 1.0;
        }
        ev
    }

    /// The sprint key may start a sprint (not consumed by the breath, no key-up pending).
    pub fn key_free_for_sprint(&self) -> bool {
        !self.need_key_up && !matches!(self.phase, BreathPhase::Holding { .. })
    }
}

/// ADS sway: (pitch, yaw) offset in degrees at time `t` for `adsIdleAmount` / `adsIdleSpeed`.
pub fn sway_offset(t: f64, amount: f32, speed: f32, k_deg_per_unit: f32, ads: f32, breath_scale: f32) -> (f32, f32) {
    let a = amount.max(0.0) * k_deg_per_unit * ads.clamp(0.0, 1.0) * breath_scale.max(0.0);
    if a <= 0.0 {
        return (0.0, 0.0);
    }
    let w = (speed.max(0.0) as f64) * std::f64::consts::TAU / 4.0;
    let yaw = a as f64 * (w * t).sin();
    let pitch = 0.5 * a as f64 * (2.0 * w * t + 0.7).sin();
    (pitch as f32, yaw as f32)
}

/// Gun kick: the viewmodel pushed back along the barrel (inches), recovering exponentially [I].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GunKick {
    pub back_in: f32,
    pub pitch_deg: f32,
}

impl GunKick {
    pub fn on_shot(&mut self, k: &KickParams, ads: f32, scale: f32, rng: &mut Rng) {
        let p = [0, 1].map(|i| lerp(k.hip_gun[i], k.ads_gun[i], ads));
        let v = uni(p[0], p[1], rng).abs();
        // [I] 0.01 in of push-back per unit of the gun-kick pitch, capped
        self.back_in = (self.back_in + v * 0.01 * scale).min(3.0);
        self.pitch_deg = (self.pitch_deg + v * 0.02 * scale).min(8.0);
    }

    pub fn tick(&mut self, dt: f32) {
        let k = (-12.0 * dt.max(0.0)).exp();
        self.back_in *= k;
        self.pitch_deg *= k;
    }
}

/// The camera rows (0 right, 1 up, 2 forward; row 3 = position, untouched) turned by `pitch` (positive = toward the
/// world-up side of the up row) and `yaw` (about the camera's up row) in degrees. Used by the camera-step detour
/// after ER's own step (ER rebuilds the rows every frame, so nothing accumulates). Orthonormal in, orthonormal out.
pub fn kick_rows(m: [[f32; 4]; 4], pitch_deg: f32, yaw_deg: f32) -> [[f32; 4]; 4] {
    if !(pitch_deg.is_finite() && yaw_deg.is_finite()) || (pitch_deg == 0.0 && yaw_deg == 0.0) {
        return m;
    }
    let v = |r: usize| [m[r][0], m[r][1], m[r][2]];
    let (mut rt, mut up, mut fw) = (v(0), v(1), v(2));
    // pitch toward "up" on screen: whichever sign of the up row points to world +Y
    let upw = if up[1] >= 0.0 { up } else { [-up[0], -up[1], -up[2]] };
    let (sp, cp) = pitch_deg.to_radians().sin_cos();
    let f2 = [0, 1, 2].map(|k| fw[k] * cp + upw[k] * sp);
    let u2 = [0, 1, 2].map(|k| upw[k] * cp - fw[k] * sp);
    fw = f2;
    up = if up[1] >= 0.0 { u2 } else { [-u2[0], -u2[1], -u2[2]] };
    let (sy, cy) = yaw_deg.to_radians().sin_cos();
    let f3 = [0, 1, 2].map(|k| fw[k] * cy + rt[k] * sy);
    let r3 = [0, 1, 2].map(|k| rt[k] * cy - fw[k] * sy);
    fw = f3;
    rt = r3;
    let mut out = m;
    for (r, row) in [rt, up, fw].into_iter().enumerate() {
        out[r][0] = row[0];
        out[r][1] = row[1];
        out[r][2] = row[2];
    }
    out
}

/// The ADS zoom: `2 atan(tan(fov / 2) * ratio)` where ratio = tan(adsZoomFov / 2) / tan(hip / 2) blended by the ADS
/// fraction (radians in and out).
pub fn zoom_fov(fov: f32, ratio: f32) -> f32 {
    if !(fov.is_finite() && ratio.is_finite()) || ratio <= 0.0 || (ratio - 1.0).abs() < 1e-6 {
        return fov;
    }
    2.0 * ((fov * 0.5).tan() * ratio).atan()
}

/// The tangent ratio of a gun's ADS zoom at ADS fraction `ads` (MW2's hip fov 65 [I]; 1.0 at hip).
pub fn zoom_ratio(ads_zoom_fov_deg: f32, ads: f32, hip_fov_deg: f32) -> f32 {
    if !(ads_zoom_fov_deg > 1.0 && ads_zoom_fov_deg < hip_fov_deg) {
        return 1.0;
    }
    let full = (ads_zoom_fov_deg.to_radians() * 0.5).tan() / (hip_fov_deg.to_radians() * 0.5).tan();
    1.0 + (full - 1.0) * ads.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_rows_orthonormal_and_direction() {
        let m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [5.0, 6.0, 7.0, 1.0]];
        assert_eq!(kick_rows(m, 0.0, 0.0), m);
        let k = kick_rows(m, 2.0, 1.0);
        assert!(k[2][1] > 0.0, "positive pitch raises the view");
        assert_eq!(k[3], m[3], "position untouched");
        for a in 0..3 {
            for b in 0..3 {
                let d: f32 = (0..3).map(|i| k[a][i] * k[b][i]).sum();
                assert!((d - if a == b { 1.0 } else { 0.0 }).abs() < 1e-5);
            }
        }
        let ang = k[2][1].asin().to_degrees();
        assert!((ang - 2.0).abs() < 0.05, "{ang}");
        // an up row pointing down (other handedness): still raises the view
        let n = [[1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0; 4]];
        assert!(kick_rows(n, 2.0, 0.0)[2][1] > 0.0);
    }

    #[test]
    fn kick_zoom_fov_ratio() {
        let r = zoom_ratio(15.0, 1.0, 65.0);
        assert!((r - (7.5f32.to_radians().tan() / 32.5f32.to_radians().tan())).abs() < 1e-5);
        assert_eq!(zoom_ratio(15.0, 0.0, 65.0), 1.0);
        assert_eq!(zoom_ratio(80.0, 1.0, 65.0), 1.0, "no zoom-out");
        let f = zoom_fov(1.0, r);
        assert!(f < 0.3 && f > 0.1, "{f}");
        assert_eq!(zoom_fov(1.0, 1.0), 1.0);
    }

    fn kp(p: [f32; 2], c: f32) -> KickParams {
        KickParams { hip: [p[0], p[1], 0.0, 0.0], ads: [p[0], p[1], 0.0, 0.0], hip_center: c, ads_center: c, hip_gun: [0.0; 4], ads_gun: [0.0; 4] }
    }

    #[test]
    fn kick_peak_matches_formula_returns_and_never_crosses() {
        for (v, c) in [(75.0f32, 1600.0f32), (50.0, 1500.0), (100.0, 500.0), (95.0, 1000.0)] {
            let mut k = ViewKick::default();
            let mut rng = Rng::new(1);
            k.on_shot(&kp([v, v], c), 0.0, 1.0, &mut rng);
            let mut peak: f32 = 0.0;
            let dt = 1.0 / 2000.0;
            let mut t = 0.0;
            while t < 2.0 {
                k.tick(dt);
                peak = peak.max(k.pitch.angle);
                assert!(k.pitch.angle >= 0.0, "never crosses zero");
                t += dt;
            }
            let want = kick_peak(v, c);
            assert!((peak - want).abs() <= 0.01 * want, "peak {peak} vs {want}");
            assert_eq!(k.pitch.angle, 0.0, "returns to rest");
        }
        // a negative impulse mirrors
        let mut k = ViewKick::default();
        k.on_shot(&kp([-40.0, -40.0], 1600.0), 0.0, 1.0, &mut Rng::new(2));
        for _ in 0..1000 {
            k.tick(0.001);
            assert!(k.pitch.angle <= 0.0);
        }
    }

    #[test]
    fn kick_ads_lerp_and_scale() {
        let k = KickParams { hip: [100.0, 100.0, 0.0, 0.0], ads: [20.0, 20.0, 0.0, 0.0], hip_center: 1000.0, ads_center: 2000.0, hip_gun: [0.0; 4], ads_gun: [0.0; 4] };
        let mut v = ViewKick::default();
        v.on_shot(&k, 1.0, 0.5, &mut Rng::new(3));
        assert!((v.pitch.vel - 10.0).abs() < 1e-4, "ADS values x scale");
    }

    #[test]
    fn breath_hold_gasp_and_key_up_rule() {
        let p = BreathParams::default();
        let mut b = Breath::default();
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut evs = Vec::new();
        // hold: amplitude to <= 5 % within 1 s
        for _ in 0..60 {
            t += dt as f64;
            if let Some(e) = b.tick(t, dt, true, true, &p) {
                evs.push(e);
            }
        }
        assert_eq!(evs, vec![BreathEvent::Hold]);
        assert!(b.scale <= 0.05, "{}", b.scale);
        // keep holding: timeout at 4.5 s -> gasp, scale toward 4.5
        while t < 4.6 {
            t += dt as f64;
            if let Some(e) = b.tick(t, dt, true, true, &p) {
                evs.push(e);
            }
        }
        assert_eq!(evs.last(), Some(&BreathEvent::Gasp { timeout: true }));
        for _ in 0..30 {
            t += dt as f64;
            b.tick(t, dt, true, true, &p);
        }
        assert!(b.scale > 3.5, "gasp x4.5: {}", b.scale);
        // still holding the key after the gasp: no new hold until key-up
        for _ in 0..120 {
            t += dt as f64;
            assert_eq!(b.tick(t, dt, true, true, &p), None);
        }
        assert!(!b.key_free_for_sprint());
        b.tick(t + dt as f64, dt, false, true, &p);
        assert!(b.key_free_for_sprint());
        assert_eq!(b.tick(t + 2.0 * dt as f64, dt, true, true, &p), Some(BreathEvent::Hold));
    }

    #[test]
    fn sway_amplitude_and_breath_scale() {
        let mut max_yaw: f32 = 0.0;
        for k in 0..4000 {
            let (_, y) = sway_offset(k as f64 * 0.005, 60.0, 1.5, 1.0 / 60.0, 1.0, 1.0);
            max_yaw = max_yaw.max(y.abs());
        }
        assert!((max_yaw - 1.0).abs() < 0.02, "the Intervention wanders ~1 deg: {max_yaw}");
        assert_eq!(sway_offset(1.0, 60.0, 1.5, 1.0 / 60.0, 0.0, 1.0), (0.0, 0.0), "no sway at hip");
        let (p, y) = sway_offset(1.3, 60.0, 1.5, 1.0 / 60.0, 1.0, 0.05);
        assert!(p.abs() <= 0.05 && y.abs() <= 0.05);
    }
}
