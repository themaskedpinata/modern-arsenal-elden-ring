//! AC-130 gunship camera shake (after run 9; docs/M4_DESIGN.md section 20). Pure: the camera-step detour (mode 2)
//! samples the active quakes every frame and [`crate::ac130_cam::frame`] applies the result to the gunship write.
//!
//! Source: MW2's `_ac130.gsc` (converted to `converted\ac130\timing.json` by convert_ac130.py 1.1):
//! `earthquake(scale, duration, origin, radius)` per gun, fired on the plane (`weaponFiredThread`: 105 0.2 / 1 s,
//! 40 0.1 / 0.5 s, radius 1000 in) and at the impact (`shotFired`: 105 0.4 / 1 s radius 3500 in, 40 0.2 / 0.5 s radius
//! 2000 in); the 25 mm has none. [V numbers]
//!
//! Model [I] (IW's exact earthquake curve is not in the dumps):
//! - intensity `a(t) = scale x cfg.scale x gain x (1 - t / duration)` (linear decay to 0 at `duration`);
//! - `gain` = 1, or with `mw2_falloff` MW2's radius: `max(0, 1 - dist / radius)` from the quake origin to the camera;
//! - overlapping quakes: the strongest current one drives the shake (no stacking of the 40 mm's rapid quakes);
//! - output: yaw/pitch jitter `a x rot_deg`, roll `a x roll_deg`, a camera offset `a x pos_m` metres (right, up,
//!   forward), each a smooth deterministic noise in [-1, 1] at `freq_hz` (two incommensurate sines per axis).

use crate::vec3::V3;

/// Quakes kept at once (the 40 mm fires 4 in 0.9 s; older ones have decayed by then).
pub const MAX_QUAKES: usize = 4;
pub const INCH_M: f32 = 0.0254;

/// `[ac130.shake]` as used by the pure logic (`config::Ac130Shake::params`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShakeParams {
    /// Multiplies MW2's scale; 0 = off.
    pub scale: f32,
    pub rot_deg: f32,
    pub roll_deg: f32,
    pub pos_m: f32,
    pub freq_hz: f32,
    pub mw2_falloff: bool,
}

/// One active quake as the detour sees it: peak amplitude (MW2 scale x cfg scale x gain), duration, age, seed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuakeSample {
    pub amp: f32,
    pub duration_s: f32,
    pub age_s: f32,
    pub seed: u32,
}

/// The shake of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShakeOut {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    /// Camera-space offset in metres: right, up, forward.
    pub offset_m: [f32; 3],
}

impl ShakeOut {
    pub const ZERO: ShakeOut = ShakeOut { yaw_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0, offset_m: [0.0; 3] };
}

/// MW2's radius falloff of a quake at `dist_m` from its origin (`mw2_falloff`), else 1.
pub fn falloff_gain(dist_m: f32, radius_m: f32, mw2_falloff: bool) -> f32 {
    if !mw2_falloff {
        return 1.0;
    }
    if !(radius_m > 0.0) || !dist_m.is_finite() {
        return 0.0;
    }
    (1.0 - dist_m / radius_m).clamp(0.0, 1.0)
}

/// Peak amplitude of a new quake: MW2 scale x cfg scale x falloff gain (0 when off or not finite).
pub fn quake_amp(mw2_scale: f32, p: &ShakeParams, dist_m: f32, radius_m: f32) -> f32 {
    let a = mw2_scale * p.scale * falloff_gain(dist_m, radius_m, p.mw2_falloff);
    if a.is_finite() && a > 0.0 { a } else { 0.0 }
}

/// Linear decay: 1 at the start, 0 from `duration_s` on (and before the start).
pub fn envelope(age_s: f32, duration_s: f32) -> f32 {
    if !(duration_s > 0.0 && age_s >= 0.0 && age_s < duration_s) {
        return 0.0;
    }
    1.0 - age_s / duration_s
}

fn phase(seed: u32, axis: u32) -> f32 {
    // a cheap integer hash -> [0, 2 pi)
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ axis.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65536.0 * std::f32::consts::TAU
}

/// Smooth deterministic noise in [-1, 1]: two incommensurate sines (ratio 2.37) with seed/axis phases.
pub fn noise(seed: u32, axis: u32, t: f32, freq_hz: f32) -> f32 {
    let w = std::f32::consts::TAU * freq_hz;
    ((w * t + phase(seed, axis)).sin() + 0.5 * (2.37 * w * t + phase(seed, axis + 16)).sin()) / 1.5
}

/// The frame's shake from the active quakes (the strongest current one drives it).
pub fn sample(quakes: &[QuakeSample], p: &ShakeParams) -> ShakeOut {
    let mut best: Option<(f32, &QuakeSample)> = None;
    for q in quakes {
        let a = q.amp * envelope(q.age_s, q.duration_s);
        if a.is_finite() && a > 0.0 && best.is_none_or(|(b, _)| a > b) {
            best = Some((a, q));
        }
    }
    let Some((a, q)) = best else { return ShakeOut::ZERO };
    let n = |axis: u32, f: f32| noise(q.seed, axis, q.age_s, p.freq_hz * f);
    ShakeOut {
        yaw_deg: a * p.rot_deg * n(0, 1.0),
        pitch_deg: a * p.rot_deg * n(1, 1.13),
        roll_deg: a * p.roll_deg * n(2, 0.87),
        offset_m: [a * p.pos_m * n(3, 0.93), a * p.pos_m * n(4, 1.07), a * p.pos_m * n(5, 0.71)],
    }
}

/// Applies the roll (about the forward row) and the camera-space offset to camera rows (right, up, forward) and
/// position. The rows stay orthonormal (a rotation in the right/up plane).
pub fn apply_roll_offset(rows: [V3; 3], pos: V3, s: &ShakeOut) -> ([V3; 3], V3) {
    let (sn, cs) = s.roll_deg.to_radians().sin_cos();
    let [r, u, f] = rows;
    let r2 = r * cs + u * sn;
    let u2 = u * cs - r * sn;
    let pos2 = pos + r * s.offset_m[0] + u * s.offset_m[1] + f * s.offset_m[2];
    ([r2, u2, f], pos2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac130_cam::{Aim, BaseView, FrameIn, Orbit, check_frame, det17, er_fov, frame, max_dist};

    const P: ShakeParams = ShakeParams { scale: 1.0, rot_deg: 2.0, roll_deg: 0.6, pos_m: 1.5, freq_hz: 11.0, mw2_falloff: false };

    fn q(amp: f32, dur: f32, age: f32, seed: u32) -> QuakeSample {
        QuakeSample { amp, duration_s: dur, age_s: age, seed }
    }

    #[test]
    fn ac130_shake_envelope_decays_linearly() {
        assert_eq!(envelope(0.0, 1.0), 1.0);
        assert!((envelope(0.25, 1.0) - 0.75).abs() < 1e-6);
        assert!((envelope(0.25, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(envelope(1.0, 1.0), 0.0);
        assert_eq!(envelope(-0.1, 1.0), 0.0);
        assert_eq!(envelope(0.1, 0.0), 0.0);
        // the shake's peak amplitude follows the envelope: max |yaw| over a window shrinks with age
        let peak = |a0: f32, a1: f32| {
            (0..200).map(|k| sample(&[q(0.4, 1.0, a0 + (a1 - a0) * k as f32 / 200.0, 7)], &P).yaw_deg.abs()).fold(0.0f32, f32::max)
        };
        assert!(peak(0.0, 0.2) > peak(0.5, 0.7) && peak(0.5, 0.7) > peak(0.8, 0.99));
        assert_eq!(sample(&[q(0.4, 1.0, 1.0, 7)], &P), ShakeOut::ZERO, "over at the duration");
    }

    #[test]
    fn ac130_shake_bounded_by_amplitude() {
        for k in 0..2000 {
            let t = k as f32 * 0.0005;
            let s = sample(&[q(0.4, 1.0, t, 3)], &P);
            let a = 0.4 * envelope(t, 1.0);
            assert!(s.yaw_deg.abs() <= a * 2.0 + 1e-5 && s.pitch_deg.abs() <= a * 2.0 + 1e-5, "{t} {s:?}");
            assert!(s.roll_deg.abs() <= a * 0.6 + 1e-5);
            assert!(s.offset_m.iter().all(|o| o.abs() <= a * 1.5 + 1e-5));
        }
        // it moves (not a constant offset) and is deterministic per seed
        let a = sample(&[q(0.4, 1.0, 0.10, 3)], &P);
        let b = sample(&[q(0.4, 1.0, 0.13, 3)], &P);
        assert_ne!(a, b);
        assert_eq!(a, sample(&[q(0.4, 1.0, 0.10, 3)], &P));
        assert_ne!(a, sample(&[q(0.4, 1.0, 0.10, 4)], &P), "the seed changes the pattern");
    }

    #[test]
    fn ac130_shake_strongest_quake_wins() {
        // a 105 impact (0.4) while a 40 mm fire quake (0.1) is fresh: the 105 drives the shake
        let big = q(0.4, 1.0, 0.2, 11);
        let small = q(0.1, 0.5, 0.0, 12);
        assert_eq!(sample(&[small, big], &P), sample(&[big], &P));
        // once the 105 has decayed below the fresh one the small one wins
        let old = q(0.4, 1.0, 0.95, 11);
        assert_eq!(sample(&[old, small], &P), sample(&[small], &P));
        // empty / all over = zero
        assert_eq!(sample(&[], &P), ShakeOut::ZERO);
        assert_eq!(sample(&[QuakeSample::default(); MAX_QUAKES], &P), ShakeOut::ZERO);
    }

    #[test]
    fn ac130_shake_mw2_numbers_and_falloff() {
        // timing.json numbers: 105 impact 0.4 / 1 s radius 3500 in, fire 0.2 / 1 s radius 1000 in at the plane
        let r_imp = 3500.0 * INCH_M;
        let r_fire = 1000.0 * INCH_M;
        assert!((r_imp - 88.9).abs() < 1e-3);
        // default (no falloff): every quake at full strength, scale multiplies
        assert_eq!(quake_amp(0.4, &P, 157.8, r_imp), 0.4);
        assert!((quake_amp(0.4, &ShakeParams { scale: 2.5, ..P }, 157.8, r_imp) - 1.0).abs() < 1e-6);
        assert_eq!(quake_amp(0.4, &ShakeParams { scale: 0.0, ..P }, 0.0, r_imp), 0.0, "scale 0 = off");
        // MW2 falloff: the gunship (slant 157.8 m) is outside the 105 impact radius (88.9 m): no shake;
        // the fire quake sits on the plane (distance 0): full strength; half the radius: half
        let f = ShakeParams { mw2_falloff: true, ..P };
        assert_eq!(quake_amp(0.4, &f, 157.8, r_imp), 0.0);
        assert_eq!(quake_amp(0.2, &f, 0.0, r_fire), 0.2);
        assert!((quake_amp(0.4, &f, r_imp / 2.0, r_imp) - 0.2).abs() < 1e-6);
        assert_eq!(falloff_gain(f32::NAN, r_imp, true), 0.0);
        assert_eq!(quake_amp(f32::NAN, &P, 0.0, r_imp), 0.0);
    }

    #[test]
    fn ac130_shake_frame_orthonormal_and_aim_unshaken() {
        let o = Orbit { center: V3::new(10.0, 5.0, -3.0), radius: 116.0, height: 107.0, period_s: 70.0, dir: 1.0, theta0: 0.0 };
        let base = FrameIn {
            orbit: o,
            t: 3.0,
            base: BaseView::Center,
            tag_pitch_deg: 64.5,
            aim: Aim::default(),
            shake: ShakeOut::ZERO,
            fov: er_fov(45.0, true, 16.0 / 9.0),
            er_w: [0.0, 0.0, 0.0, 1.0],
        };
        let calm = frame(&base).unwrap();
        let md = max_dist(o.radius, o.height, 25.0);
        for k in 0..500 {
            let s = sample(&[q(0.4, 1.0, k as f32 * 0.002, 9)], &P);
            let f = frame(&FrameIn { shake: s, ..base }).unwrap();
            assert!((det17(&f.m) - 1.0).abs() < 1e-4, "det {}", det17(&f.m));
            assert!(check_frame(&f.m, o.center, md).is_ok());
            // the shot direction and origin ignore the shake (IW shakes the view, not the weapon angles)
            assert_eq!((f.aim_fwd, f.aim_pos), (calm.aim_fwd, calm.aim_pos));
            assert!((f.pos - calm.pos).len() <= 0.4 * 1.5 * 3f32.sqrt() + 1e-4);
        }
        // a strong shake moves the view: the forward row turns by about the jitter
        let s = ShakeOut { yaw_deg: 1.0, pitch_deg: 0.0, roll_deg: 0.0, offset_m: [0.0; 3] };
        let f = frame(&FrameIn { shake: s, ..base }).unwrap();
        let ang = f.fwd.dot(calm.fwd).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(ang > 0.5 && ang < 1.01, "{ang}");
        // roll keeps the forward row and turns right/up
        let rows = [V3::new(1.0, 0.0, 0.0), V3::new(0.0, 1.0, 0.0), V3::new(0.0, 0.0, 1.0)];
        let (r, p) = apply_roll_offset(rows, V3::ZERO, &ShakeOut { roll_deg: 90.0, offset_m: [1.0, 2.0, 3.0], ..ShakeOut::ZERO });
        assert!((r[0] - V3::new(0.0, 1.0, 0.0)).len() < 1e-6 && (r[1] - V3::new(-1.0, 0.0, 0.0)).len() < 1e-6 && r[2] == rows[2]);
        assert!((p - V3::new(1.0, 2.0, 3.0)).len() < 1e-6);
    }
}
