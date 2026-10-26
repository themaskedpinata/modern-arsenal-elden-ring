//! IW4 player-move maths (docs/M5_DESIGN.md sections 2.2-2.6), in IW4 units: inches, seconds, y up.
//!
//! These are the pure formulas of MW2's `PM_CmdScale*`, `PM_Friction`, `PM_Accelerate`, `Jump_Start` and
//! `Jump_GetLandFactor` with the constants of `converted\movement.json` (the exe addresses are in that file). The
//! per-substep order and the collision live in [`crate::move_sim`].

use crate::movement_data::{Mw2, WeaponMove};
use crate::vec3::V3;

/// IW4 usercmd axes: -127..127.
pub const CMD_MAX: f32 = 127.0;

/// One tick's movement buttons in IW4 form (keyboard: full scale, as `CL_KeyMove`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cmd {
    pub forwardmove: f32,
    pub rightmove: f32,
}

impl Cmd {
    /// W/S/A/D levels to the usercmd: forward = 127 W - 127 S, right = 127 D - 127 A.
    pub fn from_keys(w: bool, s: bool, a: bool, d: bool) -> Cmd {
        let b = |x: bool| if x { CMD_MAX } else { 0.0 };
        Cmd { forwardmove: b(w) - b(s), rightmove: b(d) - b(a) }
    }
}

/// Weapon class speed scale of MW2 MP's `updateMoveSpeedScale` (`_weapons.gsc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Smg,
    Rifle,
    Mg,
    Launcher,
}

impl WeaponClass {
    pub fn scale(self, m: &Mw2) -> f32 {
        match self {
            WeaponClass::Smg => m.class_smg,
            WeaponClass::Rifle => m.class_rifle,
            WeaponClass::Mg => m.class_mg,
            WeaponClass::Launcher => m.class_launcher,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WeaponClass::Smg => "smg",
            WeaponClass::Rifle => "rifle",
            WeaponClass::Mg => "mg",
            WeaponClass::Launcher => "launcher",
        }
    }
}

/// Everything besides the usercmd that scales the wish speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleIn {
    pub sprinting: bool,
    /// Stance speed scale (1 / 0.65 / 0.15, blended during crouch<->prone).
    pub stance_scale: f32,
    pub ads: bool,
    pub weapon_move: f32,
    pub weapon_ads_move: f32,
    pub class_scale: f32,
    pub lightweight: bool,
    /// The single non-MW2 knob (`[movement] speed_scale`).
    pub speed_scale: f32,
}

impl ScaleIn {
    pub fn plain(w: &WeaponMove) -> ScaleIn {
        ScaleIn {
            sprinting: false,
            stance_scale: 1.0,
            ads: false,
            weapon_move: w.move_speed_scale,
            weapon_ads_move: w.ads_move_speed_scale,
            class_scale: 1.0,
            lightweight: false,
            speed_scale: 1.0,
        }
    }
}

/// The rightmove MW2 actually uses (sprinting scales strafe input by player_sprintStrafeSpeedScale).
pub fn effective_rightmove(m: &Mw2, cmd: Cmd, sprinting: bool) -> f32 {
    if sprinting { cmd.rightmove * m.sprint_strafe_speed_scale } else { cmd.rightmove }
}

/// Wish speed (in/s) of section 2.2: `g_speed * max(|fm*back|, |rm*strafe|)/127 * sprint * stance * weapon * ...`.
/// Diagonal movement is NOT faster than forward (V).
pub fn wishspeed(m: &Mw2, cmd: Cmd, s: &ScaleIn) -> f32 {
    let rm = effective_rightmove(m, cmd, s.sprinting);
    let fm = cmd.forwardmove;
    if fm == 0.0 && rm == 0.0 {
        return 0.0;
    }
    let fmv = (fm * if fm < 0.0 { m.back_speed_scale } else { 1.0 }).abs();
    let rmv = rm.abs() * m.strafe_speed_scale;
    let mut w = m.g_speed * fmv.max(rmv) / CMD_MAX;
    if s.sprinting {
        w *= m.sprint_speed_scale;
    }
    w *= s.stance_scale;
    w *= if s.ads { m.ads_walk_scale * s.weapon_ads_move } else { s.weapon_move };
    w *= s.class_scale;
    if s.lightweight {
        w *= m.lightweight_scale;
    }
    w * s.speed_scale
}

/// Horizontal wish direction from the camera's horizontal forward/right and the usercmd (`None` when no input).
pub fn wishdir(fwd_h: V3, right_h: V3, fm: f32, rm: f32) -> Option<V3> {
    let d = fwd_h * fm + right_h * rm;
    V3::new(d.x, 0.0, d.z).normalize()
}

/// `PM_Friction` on the horizontal velocity: control = max(stopspeed, speed), drop = control * friction * h * scale.
/// Below 1 in/s the velocity is zeroed.
pub fn friction(m: &Mw2, v: V3, h: f32, scale: f32) -> V3 {
    let s = v.planar_len();
    if s < 1.0 {
        return V3::new(0.0, v.y, 0.0);
    }
    let control = s.max(m.stopspeed);
    let drop = control * m.friction * h * scale;
    let k = (s - drop).max(0.0) / s;
    V3::new(v.x * k, v.y, v.z * k)
}

/// `PM_Accelerate`: add = wish - v.dir; v += dir * min(add, accel * h * max(wish, stopspeed)).
pub fn accelerate(m: &Mw2, v: V3, dir: V3, wish: f32, accel: f32, h: f32) -> V3 {
    let cur = v.dot(dir);
    let add = wish - cur;
    if add <= 0.0 {
        return v;
    }
    let a = (accel * h * wish.max(m.stopspeed)).min(add);
    v + dir * a
}

/// Q3 `PM_ClipVelocity`: removes the component into the plane (with the overbounce).
pub fn clip_velocity(v: V3, n: V3, overbounce: f32) -> V3 {
    let back = v.dot(n);
    let back = if back < 0.0 { back * overbounce } else { back / overbounce };
    v - n * back
}

/// MW2 jump take-off speed: sqrt(2 g * 39).
pub fn jump_v0(m: &Mw2) -> f32 {
    (2.0 * m.gravity * m.jump_height).sqrt()
}

/// `Jump_GetLandFactor`: pm_time >= 1700 -> 2.5, else 1 + 1.5 * pm_time / 1700.
pub fn land_factor(m: &Mw2, slow_ms: f32) -> f32 {
    if slow_ms <= 0.0 {
        1.0
    } else if slow_ms >= m.land_factor_full_ms {
        m.land_factor_max
    } else {
        1.0 + (m.land_factor_max - 1.0) * slow_ms / m.land_factor_full_ms
    }
}

/// Take-off speed of a jump made while the landing slowdown runs: v0^2 / landFactor.
pub fn jump_v0_slowed(m: &Mw2, slow_ms: f32) -> f32 {
    (jump_v0(m).powi(2) / land_factor(m, slow_ms)).sqrt()
}

/// Exact constant-gravity step: returns (dy, new vy). The move uses the average velocity, so the jump apex does not
/// depend on the frame rate.
pub fn gravity_step(g: f32, vy: f32, h: f32) -> (f32, f32) {
    let v_end = vy - g * h;
    ((vy + v_end) * 0.5 * h, v_end)
}

/// `Jump_ClampVelocity`: a rising player may not exceed sqrt(2 g (origin + 39 - y)); 0 when within 0.1 in of it.
pub fn jump_clamp_vy(m: &Mw2, vy: f32, y: f32, origin_y: f32) -> f32 {
    if vy <= 0.0 {
        return vy;
    }
    let gap = origin_y + m.jump_height - y;
    let cap = (2.0 * m.gravity * gap.max(0.0)).sqrt();
    // only a velocity that would carry the player above origin + 39 is clamped (with exact integration that happens
    // only after an outside push, e.g. a step-up); within 0.1 in of the cap the rise stops
    if vy <= cap + 1e-3 {
        return vy;
    }
    if gap < 0.1 { 0.0 } else { cap }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement_data::test_data;

    fn ws(cmd: Cmd, f: impl FnOnce(&mut ScaleIn)) -> f32 {
        let d = test_data();
        let mut s = ScaleIn::plain(&d.weapon);
        f(&mut s);
        wishspeed(&d.mw2, cmd, &s)
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn pmove_cmd_scale_table() {
        let w = Cmd::from_keys(true, false, false, false);
        let wa = Cmd::from_keys(true, false, true, false);
        let wd = Cmd::from_keys(true, false, false, true);
        let a = Cmd::from_keys(false, false, true, false);
        let dd = Cmd::from_keys(false, false, false, true);
        let s = Cmd::from_keys(false, true, false, false);
        let sa = Cmd::from_keys(false, true, true, false);
        for c in [w, wa, wd] {
            assert!(close(ws(c, |_| {}), 190.0, 1e-3));
        }
        assert!(close(ws(a, |_| {}), 152.0, 1e-3));
        assert!(close(ws(dd, |_| {}), 152.0, 1e-3));
        assert!(close(ws(s, |_| {}), 133.0, 1e-3));
        assert!(close(ws(sa, |_| {}), 152.0, 1e-3));
        assert!(close(ws(w, |s| s.sprinting = true), 285.0, 1e-3));
        assert!(close(ws(wa, |s| s.sprinting = true), 285.0, 1e-3));
        assert!(close(ws(w, |s| s.stance_scale = 0.65), 123.5, 1e-3));
        assert!(close(ws(a, |s| s.stance_scale = 0.65), 98.8, 1e-3));
        assert!(close(ws(s, |s| s.stance_scale = 0.65), 86.45, 1e-3));
        assert!(close(ws(w, |s| s.stance_scale = 0.15), 28.5, 1e-3));
        assert!(close(ws(w, |s| s.ads = true), 152.0, 1e-3));
        assert!(close(ws(w, |s| { s.ads = true; s.weapon_ads_move = 1.0 }), 76.0, 1e-3));
        assert!(close(ws(w, |s| { s.ads = true; s.stance_scale = 0.65 }), 98.8, 1e-3));
        assert!(close(ws(w, |s| s.lightweight = true), 203.3, 1e-3));
        let d = test_data();
        assert!(close(ws(w, |s| s.class_scale = WeaponClass::Mg.scale(&d.mw2)), 166.25, 1e-3));
        assert_eq!(ws(Cmd::default(), |_| {}), 0.0);
    }

    #[test]
    fn pmove_cmd_scale_sprint_strafe_direction() {
        let d = test_data();
        let wa = Cmd::from_keys(true, false, true, false);
        let rm = effective_rightmove(&d.mw2, wa, true);
        assert!(close(rm, -84.709, 1e-2));
        let fwd = V3::new(0.0, 0.0, 1.0);
        let right = V3::new(1.0, 0.0, 0.0);
        let dir = wishdir(fwd, right, wa.forwardmove, rm).unwrap();
        let ang = dir.x.atan2(dir.z).to_degrees();
        assert!(close(ang, -33.7, 0.05), "{ang}");
    }

    /// Frames 1..11 of a standing W start at 60 Hz (friction then accelerate), design section 2.4.
    #[test]
    fn pmove_accel_known_answers_60hz() {
        let d = test_data();
        let m = &d.mw2;
        let dir = V3::new(0.0, 0.0, 1.0);
        let want = [28.5, 47.833, 67.167, 86.5, 105.833, 124.632, 141.707, 157.217, 171.305, 184.102, 190.0];
        let mut v = V3::ZERO;
        for (i, w) in want.iter().enumerate() {
            v = friction(m, v, 1.0 / 60.0, 1.0);
            v = accelerate(m, v, dir, 190.0, m.accel_stand, 1.0 / 60.0);
            assert!(close(v.planar_len(), *w, 1e-3), "frame {} {} vs {w}", i + 1, v.planar_len());
        }
    }

    #[test]
    fn pmove_accel_air_keeps_run_speed() {
        let d = test_data();
        let m = &d.mw2;
        let dir = V3::new(0.0, 0.0, 1.0);
        let mut v = dir * 190.0;
        for _ in 0..40 {
            v = accelerate(m, v, dir, 190.0, m.air_accel, 1.0 / 60.0);
        }
        assert!(close(v.planar_len(), 190.0, 1.0));
        // air control toward the side is weak: accel 1 * h * 190 per frame
        let side = V3::new(1.0, 0.0, 0.0);
        let v2 = accelerate(m, dir * 190.0, side, 190.0, m.air_accel, 1.0 / 60.0);
        assert!(close(v2.x, 190.0 / 60.0, 1e-3));
    }

    #[test]
    fn pmove_friction_stop_from_run_60hz() {
        let d = test_data();
        let m = &d.mw2;
        let mut v = V3::new(0.0, 0.0, 190.0);
        let want = [172.583, 156.763, 142.393, 129.341, 117.485, 106.715, 96.933];
        let mut stop_frame = 0;
        for f in 1..=40 {
            v = friction(m, v, 1.0 / 60.0, 1.0);
            if f <= want.len() {
                assert!(close(v.planar_len(), want[f - 1], 1e-3), "frame {f}: {}", v.planar_len());
            }
            if v.planar_len() == 0.0 && stop_frame == 0 {
                stop_frame = f;
            }
        }
        assert_eq!(stop_frame, 18, "0 at frame 18 (0.300 s)");
        assert_eq!(friction(m, V3::new(0.5, 3.0, 0.0), 1.0 / 60.0, 1.0), V3::new(0.0, 3.0, 0.0), "< 1 in/s -> 0");
    }

    #[test]
    fn pmove_jump_v0_and_land_factor() {
        let d = test_data();
        let m = &d.mw2;
        assert!(close(jump_v0(m), 249.8, 0.05));
        assert!(close(land_factor(m, 1800.0), 2.5, 1e-6));
        assert!(close(land_factor(m, 850.0), 1.75, 1e-6));
        assert!(close(jump_v0_slowed(m, 1800.0), 158.0, 0.05));
        assert!(close(jump_v0_slowed(m, 850.0), 188.8, 0.05));
        assert_eq!(land_factor(m, 0.0), 1.0);
        // apex of the exact integration at 60 Hz
        let (mut y, mut vy) = (0.0f32, jump_v0(m));
        let mut apex = 0.0f32;
        for _ in 0..60 {
            let (dy, v) = gravity_step(m.gravity, vy, 1.0 / 60.0);
            y += dy;
            vy = jump_clamp_vy(m, v, y, 0.0);
            apex = apex.max(y);
        }
        assert!(close(apex, 39.0, 0.1), "{apex}");
    }

    #[test]
    fn pmove_clip_velocity_removes_normal_component() {
        let v = clip_velocity(V3::new(1.0, 0.0, -1.0), V3::new(0.0, 0.0, 1.0), 1.001);
        assert!(v.z >= 0.0 && close(v.x, 1.0, 1e-6));
    }
}
