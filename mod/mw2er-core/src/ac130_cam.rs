//! AC-130 gunship camera (docs/M4_DESIGN.md section 4.3). Pure: the camera-step detour (mode 2) calls [`frame`]
//! every frame with the published atomics, and the tests below cover every rule.
//!
//! - Orbit: `p(t) = c + (R cos th, H, R sin th)`, `th = th0 + dir * 2 pi * t / period` (MW2's 10 deg ease-in is not
//!   reproduced: the orbit starts at full speed; the 1 s fade from black hides it) [I].
//! - `th0` puts the gunship BEHIND the player's ER camera heading, so the first view looks the way the player looked.
//! - Forward convention `f(yaw, pitch) = (cos p sin y, sin p, cos p cos y)`, pitch positive up.
//! - Base view `center` looks at the call-in point; `mw2_tag` keeps the yaw towards it at MW2's fixed tag pitch.
//! - Aim offsets relative to the rotating base (MW2 `PlayerLinkWeaponviewToDelta`), clamped to +-arc; the absolute
//!   pitch to [-89, -5] deg.
//! - Matrix rows (r, u, f, p) with `r = normalize(UP x f)`, `u = f x r` (det +1, the ER convention proven by CAMFRAME),
//!   w components kept from ER's matrix; the 17th float is the fov (radians).

use crate::ac130_shake::{ShakeOut, apply_roll_offset};
use crate::vec3::{UP, V3};

pub const PITCH_MIN_DEG: f32 = -89.0;
pub const PITCH_MAX_DEG: f32 = -5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseView {
    Center,
    Mw2Tag,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub center: V3,
    pub radius: f32,
    pub height: f32,
    pub period_s: f32,
    /// +1 = yaw increasing (MW2 rotateyaw +360), -1 = reverse.
    pub dir: f32,
    pub theta0: f32,
}

impl Orbit {
    pub fn theta(&self, t: f32) -> f32 {
        self.theta0 + self.dir * std::f32::consts::TAU * t / self.period_s.max(1e-3)
    }

    pub fn pos(&self, t: f32) -> V3 {
        let th = self.theta(t);
        self.center + V3::new(self.radius * th.cos(), self.height, self.radius * th.sin())
    }

    /// Slant distance gunship -> centre.
    pub fn slant(&self) -> f32 {
        (self.radius * self.radius + self.height * self.height).sqrt()
    }
}

/// `th0` that places the gunship behind ER's camera heading `er_fwd` (its horizontal part); 0 for a vertical camera.
pub fn theta0_behind(er_fwd: V3) -> f32 {
    let (x, z) = (er_fwd.x, er_fwd.z);
    if (x * x + z * z).sqrt() < 1e-4 { 0.0 } else { (-z).atan2(-x) }
}

pub fn forward(yaw: f32, pitch: f32) -> V3 {
    V3::new(pitch.cos() * yaw.sin(), pitch.sin(), pitch.cos() * yaw.cos())
}

/// (yaw, pitch) of a direction (radians), the inverse of [`forward`].
pub fn yaw_pitch(f: V3) -> (f32, f32) {
    let n = f.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
    (n.x.atan2(n.z), n.y.clamp(-1.0, 1.0).asin())
}

/// Base yaw/pitch (radians) at gunship position `p` looking at centre `c`.
pub fn base_view(p: V3, c: V3, mode: BaseView, tag_pitch_deg: f32) -> (f32, f32) {
    let (yaw, pitch) = yaw_pitch(c - p);
    match mode {
        BaseView::Center => (yaw, pitch),
        BaseView::Mw2Tag => (yaw, -tag_pitch_deg.to_radians()),
    }
}

/// The mouse aim offsets (degrees) relative to the base view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aim {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimParams {
    /// MW2 m_yaw / m_pitch convention (0.022 deg per count) times the user sensitivity.
    pub deg_per_count: f32,
    /// `fov_current / fov_105` when zoom sensitivity scaling is on, else 1.
    pub zoom_scale: f32,
    pub invert_y: bool,
    pub arc_deg: f32,
}

impl Aim {
    /// Integrates raw mouse counts: +dx turns right, +dy (mouse towards the user) looks down unless `invert_y`.
    pub fn integrate(&mut self, dx: i32, dy: i32, p: &AimParams) {
        let k = p.deg_per_count * p.zoom_scale;
        if !k.is_finite() {
            return;
        }
        let arc = p.arc_deg.abs();
        let dy = if p.invert_y { -dy } else { dy };
        self.yaw_deg = (self.yaw_deg + dx as f32 * k).clamp(-arc, arc);
        self.pitch_deg = (self.pitch_deg - dy as f32 * k).clamp(-arc, arc);
    }

    /// Absolute (yaw, pitch) in radians around `base`, pitch clamped to [-89, -5] deg.
    pub fn apply(&self, base: (f32, f32)) -> (f32, f32) {
        let pitch = (base.1.to_degrees() + self.pitch_deg).clamp(PITCH_MIN_DEG, PITCH_MAX_DEG);
        (base.0 + self.yaw_deg.to_radians(), pitch.to_radians())
    }
}

/// Camera rows (r, u, f) for a forward vector (det +1).
pub fn rows_for(f: V3) -> Option<[V3; 3]> {
    let f = f.normalize()?;
    let r = UP.cross(f).normalize()?;
    let u = f.cross(r);
    Some([r, u, f])
}

/// The 17 floats written at `CSCam + 0x10`: rows 0..3 (each with ER's w) and the fov.
pub fn matrix17(rows: [V3; 3], pos: V3, er_w: [f32; 4], fov: f32) -> [f32; 17] {
    let mut m = [0.0f32; 17];
    for (i, r) in rows.iter().enumerate() {
        m[i * 4] = r.x;
        m[i * 4 + 1] = r.y;
        m[i * 4 + 2] = r.z;
        m[i * 4 + 3] = er_w[i];
    }
    m[12] = pos.x;
    m[13] = pos.y;
    m[14] = pos.z;
    m[15] = er_w[3];
    m[16] = fov;
    m
}

/// det(rows 0..2) of a 17-float frame.
pub fn det17(m: &[f32; 17]) -> f32 {
    let r = |i: usize| V3::new(m[i * 4], m[i * 4 + 1], m[i * 4 + 2]);
    r(0).dot(r(1).cross(r(2)))
}

/// The camera's vertical FOV (radians) for an MW2 cg_fov value (4:3 Hor+), or the horizontal one at `aspect` when
/// ER's fov turns out to be horizontal (`fov_is_vertical = false`).
pub fn er_fov(cg_fov_deg: f32, fov_is_vertical: bool, aspect: f32) -> f32 {
    let v = crate::vm_proj::vfov_from_cg_fov(cg_fov_deg);
    if fov_is_vertical { v } else { 2.0 * ((v * 0.5).tan() * aspect.max(0.1)).atan() }
}

/// Inputs of one detour frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameIn {
    pub orbit: Orbit,
    /// Seconds since the ride started.
    pub t: f32,
    pub base: BaseView,
    pub tag_pitch_deg: f32,
    pub aim: Aim,
    /// The earthquake shake of this frame (`ac130_shake::sample`; `ShakeOut::ZERO` = none).
    pub shake: ShakeOut,
    pub fov: f32,
    /// ER's w column of rows 0..3 (kept as ER wrote it).
    pub er_w: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameOut {
    pub m: [f32; 17],
    /// The written (shaken) camera position and forward row: what the user sees.
    pub pos: V3,
    pub fwd: V3,
    /// The unshaken view: where the gun points (IW shakes the view, not the weapon angles) [I].
    pub aim_pos: V3,
    pub aim_fwd: V3,
}

pub fn frame(i: &FrameIn) -> Option<FrameOut> {
    let pos = i.orbit.pos(i.t);
    let base = base_view(pos, i.orbit.center, i.base, i.tag_pitch_deg);
    let (yaw, pitch) = i.aim.apply(base);
    let aim_fwd = rows_for(forward(yaw, pitch))?[2];
    let pitch = (pitch.to_degrees() + i.shake.pitch_deg).clamp(PITCH_MIN_DEG, PITCH_MAX_DEG).to_radians();
    let yaw = yaw + i.shake.yaw_deg.to_radians();
    let rows = rows_for(forward(yaw, pitch))?;
    let (rows, shaken) = apply_roll_offset(rows, pos, &i.shake);
    Some(FrameOut { m: matrix17(rows, shaken, i.er_w, i.fov), pos: shaken, fwd: rows[2], aim_pos: pos, aim_fwd })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    NotFinite,
    Distance,
    Det,
    Fov,
}

impl Reject {
    pub fn label(self) -> &'static str {
        match self {
            Reject::NotFinite => "not_finite",
            Reject::Distance => "distance",
            Reject::Det => "det",
            Reject::Fov => "fov",
        }
    }
}

/// The detour's sanity check before the write: all finite, `|p - feet| <= max_dist`, det in [0.99, 1.01], fov in
/// (0.01, 2.5) rad.
pub fn check_frame(m: &[f32; 17], feet: V3, max_dist: f32) -> Result<(), Reject> {
    if !m.iter().all(|v| v.is_finite()) || !feet.is_finite() {
        return Err(Reject::NotFinite);
    }
    let p = V3::new(m[12], m[13], m[14]);
    if (p - feet).len() > max_dist {
        return Err(Reject::Distance);
    }
    if !(0.99..=1.01).contains(&det17(m)) {
        return Err(Reject::Det);
    }
    if !(m[16] > 0.01 && m[16] < 2.5) {
        return Err(Reject::Fov);
    }
    Ok(())
}

/// Bounds distance: the gunship stays within `hypot(R, H) + margin` of the feet.
pub fn max_dist(radius: f32, height: f32, margin: f32) -> f32 {
    (radius * radius + height * height).sqrt() + margin
}

/// Index of the fov in the 17 floats (CSCam+0x50).
pub const FOV_IDX: usize = 16;

fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits()
}

/// Per-float save of ER's frame before the gunship write (review M4 major 1). A float that still holds our last write
/// (ER did not rebuild that float this frame) keeps the older saved ER value; every other float is ER's fresh value.
/// Before the first write of a ride (`wrote == false`) the whole read is ER's. Returns the merged frame and a bit mask
/// of the floats that persisted (bit i = float i still held our write).
pub fn merge_er(saved: &[f32; 17], cur: &[f32; 17], last_write: &[f32; 17], wrote: bool) -> ([f32; 17], u32) {
    let mut out = *cur;
    let mut persisted = 0u32;
    if wrote {
        for i in 0..17 {
            if same(cur[i], last_write[i]) {
                out[i] = saved[i];
                persisted |= 1 << i;
            }
        }
    }
    (out, persisted)
}

/// Task-side view of pers_cam_1 while it may hold our write: every float that still equals our last write is replaced
/// by ER's saved value. `None` when no float holds our write (the camera is ER's own).
pub fn substitute_er(read: &[f32; 17], ours: &[f32; 17], er: &[f32; 17]) -> Option<[f32; 17]> {
    let mut out = *read;
    let mut any = false;
    for i in 0..17 {
        if same(read[i], ours[i]) {
            out[i] = er[i];
            any = true;
        }
    }
    any.then_some(out)
}

/// The end-of-ride restore, per float: ER's saved value goes back into every float that still holds our write; a float
/// ER rebuilt is left alone. `None` = nothing holds our write. Otherwise (the 17 floats to write, how many were ours,
/// whether the fov was ours).
pub fn restore_plan(cur: &[f32; 17], ours: &[f32; 17], er: &[f32; 17]) -> Option<([f32; 17], u32, bool)> {
    let mut out = *cur;
    let mut n = 0;
    for i in 0..17 {
        if same(cur[i], ours[i]) && !same(cur[i], er[i]) {
            out[i] = er[i];
            n += 1;
        }
    }
    let fov = same(cur[FOV_IDX], ours[FOV_IDX]) && !same(cur[FOV_IDX], er[FOV_IDX]);
    (n > 0).then_some((out, n, fov))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m17(base: f32) -> [f32; 17] {
        let mut m = [0.0f32; 17];
        for (i, v) in m.iter_mut().enumerate() {
            *v = base + i as f32;
        }
        m
    }

    #[test]
    fn ac130_cam_merge_first_write_takes_all() {
        let (out, p) = merge_er(&m17(0.0), &m17(100.0), &m17(100.0), false);
        assert_eq!(out, m17(100.0));
        assert_eq!(p, 0);
    }

    #[test]
    fn ac130_cam_merge_fov_not_rebuilt_keeps_er_fov() {
        // ER rebuilds rows + position every frame but leaves the fov: our gunship fov must not become "ER's fov"
        let er0 = m17(0.0);
        let ours = m17(500.0);
        let mut cur = m17(1.0); // ER rebuilt rows/pos
        cur[FOV_IDX] = ours[FOV_IDX]; // fov still ours
        let (out, p) = merge_er(&er0, &cur, &ours, true);
        assert_eq!(out[FOV_IDX], er0[FOV_IDX]);
        assert_eq!(out[0], cur[0]);
        assert_eq!(p, 1 << FOV_IDX);
    }

    #[test]
    fn ac130_cam_merge_nothing_rebuilt_keeps_all() {
        let (out, p) = merge_er(&m17(0.0), &m17(500.0), &m17(500.0), true);
        assert_eq!(out, m17(0.0));
        assert_eq!(p, (1 << 17) - 1);
    }

    #[test]
    fn ac130_cam_restore_fov_only() {
        let er = m17(0.0);
        let ours = m17(500.0);
        let mut cur = m17(7.0);
        cur[FOV_IDX] = ours[FOV_IDX];
        let (w, n, fov) = restore_plan(&cur, &ours, &er).unwrap();
        assert_eq!(n, 1);
        assert!(fov);
        assert_eq!(w[FOV_IDX], er[FOV_IDX]);
        assert_eq!(w[0], cur[0]); // rebuilt floats untouched
    }

    #[test]
    fn ac130_cam_restore_all_and_none() {
        let er = m17(0.0);
        let ours = m17(500.0);
        let (w, n, fov) = restore_plan(&ours, &ours, &er).unwrap();
        assert_eq!((w, n, fov), (er, 17, true));
        assert!(restore_plan(&m17(3.0), &ours, &er).is_none());
        // a float equal in ours and ER's (e.g. the w components) is not counted
        let mut ours2 = ours;
        ours2[3] = er[3];
        let (_, n2, _) = restore_plan(&ours2, &ours2, &er).unwrap();
        assert_eq!(n2, 16);
    }

    #[test]
    fn ac130_cam_substitute_per_float() {
        let er = m17(0.0);
        let ours = m17(500.0);
        let mut read = m17(9.0);
        assert!(substitute_er(&read, &ours, &er).is_none());
        read[FOV_IDX] = ours[FOV_IDX];
        let s = substitute_er(&read, &ours, &er).unwrap();
        assert_eq!(s[FOV_IDX], er[FOV_IDX]);
        assert_eq!(s[0], read[0]);
    }

    fn orbit() -> Orbit {
        Orbit { center: V3::new(10.0, 5.0, -3.0), radius: 116.0, height: 107.0, period_s: 70.0, dir: 1.0, theta0: 0.0 }
    }

    fn close(a: V3, b: V3, eps: f32) -> bool {
        (a - b).len() < eps
    }

    #[test]
    fn ac130_cam_orbit_positions() {
        let o = orbit();
        assert!(close(o.pos(0.0), V3::new(126.0, 112.0, -3.0), 1e-3));
        // a quarter turn after 17.5 s: theta = 90 deg
        assert!(close(o.pos(17.5), V3::new(10.0, 112.0, 113.0), 1e-2));
        // half a turn after 35 s
        assert!(close(o.pos(35.0), V3::new(-106.0, 112.0, -3.0), 1e-2));
        let r = Orbit { dir: -1.0, ..o };
        assert!(close(r.pos(17.5), V3::new(10.0, 112.0, -119.0), 1e-2));
        assert!((o.slant() - 157.82).abs() < 0.01);
        // scale knob (radius/altitude x 0.5)
        let s = Orbit { radius: 58.0, height: 53.5, ..o };
        assert!((s.slant() - o.slant() * 0.5).abs() < 0.01);
    }

    #[test]
    fn ac130_cam_theta0_behind_heading() {
        for (fx, fz) in [(0.0f32, 1.0f32), (1.0, 0.0), (-0.6, 0.8), (0.3, -0.95)] {
            let f = V3::new(fx, -0.3, fz);
            let mut o = orbit();
            o.theta0 = theta0_behind(f);
            let p = o.pos(0.0);
            let to_c = (o.center - p).normalize().unwrap();
            let h = V3::new(f.x, 0.0, f.z).normalize().unwrap();
            let th = V3::new(to_c.x, 0.0, to_c.z).normalize().unwrap();
            assert!(th.dot(h) > 0.9999, "the first view looks along ER's heading {f:?}");
        }
        assert_eq!(theta0_behind(V3::new(0.0, -1.0, 0.0)), 0.0);
    }

    #[test]
    fn ac130_cam_base_view_pitch() {
        let o = orbit();
        let (_, p) = base_view(o.pos(0.0), o.center, BaseView::Center, 64.5);
        assert!((p.to_degrees() + 42.68).abs() < 0.02, "{}", p.to_degrees());
        let (_, p) = base_view(o.pos(0.0), o.center, BaseView::Mw2Tag, 64.5);
        assert!((p.to_degrees() + 64.5).abs() < 1e-4);
        let (y, p) = yaw_pitch(forward(0.7, -0.4));
        assert!((y - 0.7).abs() < 1e-5 && (p + 0.4).abs() < 1e-5);
    }

    #[test]
    fn ac130_cam_aim_clamps_hold() {
        let p = AimParams { deg_per_count: 0.022, zoom_scale: 1.0, invert_y: false, arc_deg: 35.0 };
        let mut a = Aim::default();
        a.integrate(1_000_000, -1_000_000, &p);
        assert_eq!((a.yaw_deg, a.pitch_deg), (35.0, 35.0));
        a.integrate(-3_000_000, 3_000_000, &p);
        assert_eq!((a.yaw_deg, a.pitch_deg), (-35.0, -35.0));
        // absolute pitch stays within [-89, -5]
        let (_, pitch) = Aim { yaw_deg: 0.0, pitch_deg: 35.0 }.apply((0.0, (-30.0f32).to_radians()));
        assert!((pitch.to_degrees() + 5.0).abs() < 1e-4);
        let (_, pitch) = Aim { yaw_deg: 0.0, pitch_deg: -35.0 }.apply((0.0, (-80.0f32).to_radians()));
        assert!((pitch.to_degrees() + 89.0).abs() < 1e-4);
    }

    #[test]
    fn ac130_cam_mouse_y_looks_down_and_invert() {
        let p = AimParams { deg_per_count: 0.022, zoom_scale: 1.0, invert_y: false, arc_deg: 35.0 };
        let mut a = Aim::default();
        a.integrate(0, 100, &p);
        assert!(a.pitch_deg < 0.0, "+lY (mouse towards the user) looks down");
        let mut b = Aim::default();
        b.integrate(0, 100, &AimParams { invert_y: true, ..p });
        assert!(b.pitch_deg > 0.0);
        let mut c = Aim::default();
        c.integrate(100, 0, &p);
        assert!((c.yaw_deg - 2.2).abs() < 1e-5, "+lX turns right");
        // the right of a camera looking +z is +x: yaw increasing turns towards +x
        assert!(forward(0.1, 0.0).x > 0.0 && rows_for(V3::new(0.0, 0.0, 1.0)).unwrap()[0].x > 0.99);
    }

    #[test]
    fn ac130_cam_zoom_sensitivity_scaling() {
        let base = AimParams { deg_per_count: 0.022, zoom_scale: 1.0, invert_y: false, arc_deg: 35.0 };
        let zoomed = AimParams { zoom_scale: 5.0 / 45.0, ..base };
        let (mut a, mut b) = (Aim::default(), Aim::default());
        a.integrate(90, 0, &base);
        b.integrate(90, 0, &zoomed);
        assert!((b.yaw_deg - a.yaw_deg * 5.0 / 45.0).abs() < 1e-5);
    }

    #[test]
    fn ac130_cam_rows_orthonormal_sweep() {
        let mut worst_o = 0.0f32;
        let mut worst_d = 0.0f32;
        for i in 0..10_000 {
            let yaw = (i as f32 * 0.6283) % std::f32::consts::TAU - std::f32::consts::PI;
            let pitch = (-89.0 + (i % 85) as f32).to_radians();
            let r = rows_for(forward(yaw, pitch)).unwrap();
            for a in 0..3 {
                worst_o = worst_o.max((r[a].len() - 1.0).abs());
                for b in a + 1..3 {
                    worst_o = worst_o.max(r[a].dot(r[b]).abs());
                }
            }
            let m = matrix17(r, V3::ZERO, [0.0, 0.0, 0.0, 1.0], 0.6);
            worst_d = worst_d.max((det17(&m) - 1.0).abs());
        }
        assert!(worst_o < 1e-5 && worst_d < 1e-5, "ortho {worst_o} det {worst_d}");
    }

    #[test]
    fn ac130_cam_bounds_reject() {
        let o = orbit();
        let i = FrameIn {
            orbit: o,
            t: 3.0,
            base: BaseView::Center,
            tag_pitch_deg: 64.5,
            aim: Aim::default(),
            shake: ShakeOut::ZERO,
            fov: er_fov(45.0, true, 16.0 / 9.0),
            er_w: [0.0, 0.0, 0.0, 1.0],
        };
        let f = frame(&i).unwrap();
        let md = max_dist(o.radius, o.height, 25.0);
        assert!(check_frame(&f.m, o.center, md).is_ok());
        let mut bad = f.m;
        bad[5] = f32::NAN;
        assert_eq!(check_frame(&bad, o.center, md), Err(Reject::NotFinite));
        assert_eq!(check_frame(&f.m, o.center + V3::new(500.0, 0.0, 0.0), md), Err(Reject::Distance));
        let mut flip = f.m;
        for k in 0..3 {
            flip[k] = -flip[k];
        }
        assert_eq!(check_frame(&flip, o.center, md), Err(Reject::Det));
        let mut fov = f.m;
        fov[16] = 3.0;
        assert_eq!(check_frame(&fov, o.center, md), Err(Reject::Fov));
        // the gunship looks at the centre: forward . (c - p) ~ 1
        assert!(f.fwd.dot((o.center - f.pos).normalize().unwrap()) > 0.9999);
    }

    #[test]
    fn ac130_cam_fov_conversion() {
        for (cg, want) in [(45.0f32, 34.52f32), (20.0, 15.06), (5.0, 3.75)] {
            let v = er_fov(cg, true, 16.0 / 9.0).to_degrees();
            assert!((v - want).abs() < 0.05, "{cg} -> {v}");
        }
        let h = er_fov(45.0, false, 16.0 / 9.0).to_degrees();
        assert!(h > 50.0 && h < 60.0, "{h}");
    }

    #[test]
    fn ac130_cam_camframe_forward_rebuilds_right_up() {
        // A real CAMFRAME forward from run 7 (ER's third-person camera, det +1): fwd=(0.612,-0.276,0.741)
        let f = V3::new(0.612, -0.276, 0.741).normalize().unwrap();
        let r = rows_for(f).unwrap();
        // right is horizontal and to the right of the heading, up has a positive y
        assert!(r[0].y.abs() < 1e-6 && r[1].y > 0.9 && r[0].dot(f).abs() < 1e-6);
        let m = matrix17(r, V3::ZERO, [0.0; 4], 0.8);
        assert!((det17(&m) - 1.0).abs() < 1e-5);
    }
}
