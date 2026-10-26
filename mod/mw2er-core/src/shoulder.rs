//! The third-person over-the-shoulder camera of MW2 mode (docs/C6_HOLD_DESIGN.md part 18, `[third_person]`).
//!
//! ER's third-person camera looks over the head of the character, so the screen centre (the MW2 crosshair) sits on
//! the head. While MW2 mode drives in third person the camera-step detour moves `CSCamera.pers_cam_1` row 3 (the
//! render camera's position, nothing else) by a local offset `[right, up, back]` metres: right = the horizontal
//! right of the view, up = world up, back = against the view forward. ER rebuilds the row every frame, so nothing is
//! restored; ER's ChrCam / follow cam (its collision, auto rotation, lock-on) never see the offset.
//!
//! Pure logic here (no game access): the basis ([`world_offset`]), the spring arm ([`arm_frac`] / [`step_arm`]: a ray
//! from ER's camera to the offset position, pulled in at once on a hit minus a margin, let out with the blend), the
//! blend ([`blend_k`]), the gates ([`gate`]) and the controller ([`ShoulderCtl`]).

use crate::vec3::{UP, V3};

/// The offset may never be longer than this (metres; the config range keeps it far below).
pub const MAX_OFFSET_M: f32 = 3.0;
/// Below this length the offset is "zero" (nothing written).
pub const EPS_M: f32 = 1e-3;
/// A spring-arm fraction below this is a pull (logged).
pub const PULL_FRAC: f32 = 0.98;

/// `[third_person] shoulder_side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
}

impl Side {
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "right" => Some(Side::Right),
            "left" => Some(Side::Left),
            _ => None,
        }
    }

    pub fn sign(self) -> f32 {
        match self {
            Side::Right => 1.0,
            Side::Left => -1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Side::Right => "right",
            Side::Left => "left",
        }
    }
}

/// The resolved `[third_person]` shoulder keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShoulderParams {
    /// Hip offset `[right, up, back]` (metres, right side).
    pub offset: [f32; 3],
    /// ADS / guard offset `[right, up, back]`.
    pub ads_offset: [f32; 3],
    pub side: Side,
    /// Blend time constant: ~95 % of a step after this many seconds.
    pub blend_s: f32,
    /// The spring arm stops this far before a hit.
    pub margin_m: f32,
}

impl ShoulderParams {
    /// Off when both offsets are zero (the code default).
    pub fn on(&self) -> bool {
        self.offset.iter().chain(self.ads_offset.iter()).any(|v| v.abs() > EPS_M)
    }

    /// The local target `[right, up, back]` for an ADS weight 0..1 (lerped hip -> ADS), the side applied.
    pub fn target(&self, ads_w: f32) -> V3 {
        let w = if ads_w.is_finite() { ads_w.clamp(0.0, 1.0) } else { 0.0 };
        let l = |k: usize| self.offset[k] + (self.ads_offset[k] - self.offset[k]) * w;
        V3::new(l(0) * self.side.sign(), l(1), l(2))
    }
}

/// The world offset of a local `[right, up, back]` for a camera looking along `fwd`: right is horizontal (to the
/// right of the heading: a camera looking +z has its right at +x, ER's frame), up is world up, back is `-fwd`
/// (pitch included, like a boom). None for a non-finite / vertical forward or a non-finite / too long result.
pub fn world_offset(fwd: V3, local: V3) -> Option<V3> {
    let f = fwd.normalize()?;
    if !local.is_finite() {
        return None;
    }
    let right = UP.cross(V3::new(f.x, 0.0, f.z)).normalize()?;
    let off = right * local.x + UP * local.y - f * local.z;
    (off.is_finite() && off.len() <= MAX_OFFSET_M).then_some(off)
}

/// The spring arm: the fraction (0..1) of an offset `want_m` long that fits before the first hit `hit_t` (metres
/// along the ray from ER's camera), stopping `margin_m` before it. No hit = 1.
pub fn arm_frac(want_m: f32, hit_t: Option<f32>, margin_m: f32) -> f32 {
    if !(want_m.is_finite() && want_m > EPS_M) {
        return 1.0;
    }
    match hit_t {
        Some(t) if t.is_finite() => ((t - margin_m.max(0.0)).max(0.0) / want_m).clamp(0.0, 1.0),
        Some(_) => 0.0,
        None => 1.0,
    }
}

/// Exponential blend factor for one step of `dt` seconds: ~95 % of a step after `blend_s` (0 = at once).
pub fn blend_k(dt: f32, blend_s: f32) -> f32 {
    if !(blend_s.is_finite() && blend_s > 1e-4) {
        return 1.0;
    }
    if !(dt.is_finite() && dt > 0.0) {
        return 0.0;
    }
    1.0 - (-3.0 * dt / blend_s).exp()
}

/// The spring arm's fraction over time: pulled in to `allowed` at once (never into the wall), let out with the blend.
pub fn step_arm(cur: f32, allowed: f32, dt: f32, blend_s: f32) -> f32 {
    let a = if allowed.is_finite() { allowed.clamp(0.0, 1.0) } else { 0.0 };
    if a <= cur || !cur.is_finite() {
        return a;
    }
    let n = cur + (a - cur) * blend_k(dt, blend_s);
    if (a - n).abs() < 1e-3 { a } else { n }
}

/// Why the shoulder offset is not applied this tick (the first that holds, in this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Both offsets zero, or no camera-step hook.
    Cfg,
    NotInWorld,
    /// A loading screen / cutscene / NPC talk latch of first person.
    Cutscene,
    Ac130,
    Nuke,
    /// The C6 scope peek (first person for a scoped ADS): the offset is held and comes back at once after it.
    Peek,
    FirstPerson,
    ModeOff,
    Dead,
    Menu,
    Mounted,
    /// The camera gate is not valid (settling after a load / a re-base) beyond the task's grace ticks.
    Camera,
}

impl Why {
    pub fn label(self) -> &'static str {
        match self {
            Why::Cfg => "cfg",
            Why::NotInWorld => "world",
            Why::Cutscene => "cutscene",
            Why::Ac130 => "ac130",
            Why::Nuke => "nuke",
            Why::Peek => "peek",
            Why::FirstPerson => "fp",
            Why::ModeOff => "mode_off",
            Why::Dead => "dead",
            Why::Menu => "menu",
            Why::Mounted => "mounted",
            Why::Camera => "camera",
        }
    }

    /// What the offset does while this holds.
    pub fn action(self) -> Action {
        match self {
            // another camera owns the screen (or none is valid): the offset is dropped at once
            Why::NotInWorld | Why::Cutscene | Why::Ac130 | Why::Nuke | Why::FirstPerson => Action::Cut,
            // the peek is a short first-person moment of a third-person ADS; a camera-gate blip (re-base) is one frame
            Why::Peek | Why::Camera => Action::Hold,
            Why::Cfg | Why::ModeOff | Why::Dead | Why::Menu | Why::Mounted => Action::BlendOut,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Zero at once.
    Cut,
    /// Kept (not applied).
    Hold,
    /// Blended to zero, applied while it fades.
    BlendOut,
}

/// The gate inputs (one tick).
#[derive(Clone, Copy, Debug, Default)]
pub struct GateIn {
    pub cfg_on: bool,
    pub hook: bool,
    pub in_world: bool,
    pub latched: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub peek: bool,
    pub fp: bool,
    pub mode_on: bool,
    pub dead: bool,
    pub menu: bool,
    pub mounted: bool,
    /// `[third_person] shoulder_mounted`: on Torrent too.
    pub mounted_ok: bool,
    pub cam_valid: bool,
}

pub fn gate(i: &GateIn) -> Result<(), Why> {
    let checks = [
        (!(i.cfg_on && i.hook), Why::Cfg),
        (!i.in_world, Why::NotInWorld),
        (i.latched, Why::Cutscene),
        (i.ac130, Why::Ac130),
        (i.nuke, Why::Nuke),
        (i.peek, Why::Peek),
        (i.fp, Why::FirstPerson),
        (!i.mode_on, Why::ModeOff),
        (i.dead, Why::Dead),
        (i.menu, Why::Menu),
        (i.mounted && !i.mounted_ok, Why::Mounted),
        (!i.cam_valid, Why::Camera),
    ];
    match checks.iter().find(|(c, _)| *c) {
        Some((_, w)) => Err(*w),
        None => Ok(()),
    }
}

/// One controller step's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    /// Write the offset this frame (the detour is armed).
    pub apply: bool,
    /// The blended local offset before the spring arm.
    pub local: V3,
    /// An on / off edge of `apply` this step.
    pub edge: Option<bool>,
}

/// The blend state (task side). `local` = the blended `[right, up, back]`; `frac` = the spring arm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShoulderCtl {
    pub local: V3,
    pub frac: f32,
    pub applying: bool,
    pub last_why: Option<Why>,
}

impl Default for ShoulderCtl {
    fn default() -> Self {
        ShoulderCtl { local: V3::ZERO, frac: 1.0, applying: false, last_why: Some(Why::Cfg) }
    }
}

impl ShoulderCtl {
    /// One tick: the gate result, the local target (`ShoulderParams::target`), `dt`, the blend time.
    pub fn step(&mut self, g: Result<(), Why>, target: V3, dt: f32, blend_s: f32) -> Step {
        let was = self.applying;
        self.last_why = g.err();
        let apply = match g {
            Ok(()) => {
                let k = blend_k(dt, blend_s);
                self.local = self.local + (target - self.local) * k;
                if (target - self.local).len() < EPS_M {
                    self.local = target;
                }
                self.local.len() > EPS_M
            }
            Err(w) => match w.action() {
                Action::Cut => {
                    self.local = V3::ZERO;
                    self.frac = 1.0;
                    false
                }
                Action::Hold => false,
                Action::BlendOut => {
                    let k = blend_k(dt, blend_s);
                    self.local = self.local * (1.0 - k);
                    if self.local.len() < EPS_M {
                        self.local = V3::ZERO;
                    }
                    self.local.len() > EPS_M
                }
            },
        };
        if !self.local.is_finite() {
            self.local = V3::ZERO;
            self.frac = 1.0;
        }
        let apply = apply && self.local.is_finite();
        self.applying = apply;
        Step { apply, local: self.local, edge: (apply != was).then_some(apply) }
    }

    /// The spring arm for this tick's ray result: returns the local offset to publish (`local * frac`).
    pub fn arm(&mut self, allowed: f32, dt: f32, blend_s: f32) -> V3 {
        self.frac = step_arm(self.frac, allowed, dt, blend_s);
        self.local * self.frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> ShoulderParams {
        ShoulderParams { offset: [0.55, 0.25, 0.0], ads_offset: [0.45, 0.15, -0.6], side: Side::Right, blend_s: 0.15, margin_m: 0.2 }
    }

    fn close(a: V3, b: V3) -> bool {
        (a - b).len() < 1e-4
    }

    #[test]
    fn c6_shoulder_basis_right_up_back() {
        // ER's frame: a camera looking +z has its right at +x (ac130_cam, kick rows: 0 right, 1 up, 2 forward)
        let o = world_offset(V3::new(0.0, 0.0, 1.0), V3::new(0.55, 0.25, 0.4)).unwrap();
        assert!(close(o, V3::new(0.55, 0.25, -0.4)), "{o:?}");
        // looking +x: right is -z
        let o = world_offset(V3::new(1.0, 0.0, 0.0), V3::new(1.0, 0.0, 0.0)).unwrap();
        assert!(close(o, V3::new(0.0, 0.0, -1.0)), "{o:?}");
        // pitched 40 deg down: right stays horizontal, up stays world up, back follows the pitch
        let f = V3::new(0.0, -(40f32.to_radians().sin()), 40f32.to_radians().cos());
        let o = world_offset(f, V3::new(0.5, 0.2, 0.0)).unwrap();
        assert!(close(o, V3::new(0.5, 0.2, 0.0)), "{o:?}");
        let b = world_offset(f, V3::new(0.0, 0.0, 1.0)).unwrap();
        assert!(close(b, -f), "{b:?}");
        // left side mirrors the right component only
        let l = ShoulderParams { side: Side::Left, ..p() };
        assert!(close(l.target(0.0), V3::new(-0.55, 0.25, 0.0)));
        // rejects a vertical / non-finite forward and an over-long offset
        assert!(world_offset(V3::new(0.0, 1.0, 0.0), V3::new(0.5, 0.0, 0.0)).is_none());
        assert!(world_offset(V3::new(f32::NAN, 0.0, 1.0), V3::new(0.5, 0.0, 0.0)).is_none());
        assert!(world_offset(V3::new(0.0, 0.0, 1.0), V3::new(3.5, 0.0, 0.0)).is_none());
    }

    #[test]
    fn c6_shoulder_ads_lerp_and_off() {
        let s = p();
        assert!(s.on());
        assert!(close(s.target(0.0), V3::new(0.55, 0.25, 0.0)));
        assert!(close(s.target(1.0), V3::new(0.45, 0.15, -0.6)));
        assert!(close(s.target(0.5), V3::new(0.50, 0.20, -0.3)));
        assert!(close(s.target(f32::NAN), s.target(0.0)));
        let off = ShoulderParams { offset: [0.0; 3], ads_offset: [0.0; 3], ..s };
        assert!(!off.on());
    }

    #[test]
    fn c6_shoulder_spring_arm_pulls_in_with_margin() {
        assert_eq!(arm_frac(0.6, None, 0.2), 1.0);
        // a wall 0.5 m to the right of ER's camera, margin 0.2: 0.3 of 0.6 m fits
        assert!((arm_frac(0.6, Some(0.5), 0.2) - 0.5).abs() < 1e-6);
        // hit beyond the target: still all of it once the margin is outside
        assert_eq!(arm_frac(0.6, Some(5.0), 0.2), 1.0);
        // wall closer than the margin: nothing
        assert_eq!(arm_frac(0.6, Some(0.1), 0.2), 0.0);
        assert_eq!(arm_frac(0.6, Some(f32::NAN), 0.2), 0.0);
        assert_eq!(arm_frac(0.0, Some(0.0), 0.2), 1.0);
        // pull-in is immediate, let-out blends
        assert_eq!(step_arm(1.0, 0.3, 1.0 / 60.0, 0.15), 0.3);
        let n = step_arm(0.3, 1.0, 1.0 / 60.0, 0.15);
        assert!(n > 0.3 && n < 0.5, "{n}");
        let mut f = 0.3;
        for _ in 0..30 {
            f = step_arm(f, 1.0, 1.0 / 60.0, 0.15);
        }
        assert_eq!(f, 1.0);
    }

    #[test]
    fn c6_shoulder_blend_in_out_timing() {
        let s = p();
        let mut c = ShoulderCtl::default();
        let dt = 1.0 / 60.0;
        let first = c.step(Ok(()), s.target(0.0), dt, s.blend_s);
        assert_eq!(first.edge, Some(true));
        let mut t = dt;
        while t < s.blend_s - 1e-6 {
            c.step(Ok(()), s.target(0.0), dt, s.blend_s);
            t += dt;
        }
        // ~95 % after blend_s
        let r = c.local.len() / s.target(0.0).len();
        assert!((0.93..=0.97).contains(&r), "{r}");
        for _ in 0..60 {
            c.step(Ok(()), s.target(0.0), dt, s.blend_s);
        }
        assert!(close(c.local, s.target(0.0)));
        // blend out (mode off): applied while fading, then an off edge
        let mut edges = Vec::new();
        for _ in 0..120 {
            let st = c.step(Err(Why::ModeOff), s.target(0.0), dt, s.blend_s);
            if let Some(e) = st.edge {
                edges.push(e);
            }
        }
        assert_eq!(edges, vec![false]);
        assert_eq!(c.local, V3::ZERO);
        // blend_s 0 = at once
        let st = c.step(Ok(()), s.target(1.0), dt, 0.0);
        assert!(close(st.local, s.target(1.0)));
    }

    #[test]
    fn c6_shoulder_cut_hold_and_gates() {
        let s = p();
        let mut c = ShoulderCtl::default();
        for _ in 0..60 {
            c.step(Ok(()), s.target(0.0), 1.0 / 60.0, s.blend_s);
        }
        // the scope peek holds the offset (not applied), and it is back at once afterwards
        let h = c.step(Err(Why::Peek), s.target(1.0), 1.0 / 60.0, s.blend_s);
        assert!(!h.apply && h.edge == Some(false) && close(h.local, s.target(0.0)));
        let b = c.step(Ok(()), s.target(0.0), 1.0 / 60.0, s.blend_s);
        assert!(b.apply && close(b.local, s.target(0.0)));
        // first person / AC-130 / cutscene cut at once
        for w in [Why::FirstPerson, Why::Ac130, Why::Nuke, Why::Cutscene, Why::NotInWorld] {
            let mut d = c;
            let st = d.step(Err(w), s.target(0.0), 1.0 / 60.0, s.blend_s);
            assert!(!st.apply && st.local == V3::ZERO, "{w:?}");
        }
        let mut d = c;
        let st = d.step(Err(Why::Camera), s.target(0.0), 1.0 / 60.0, s.blend_s);
        assert!(!st.apply && close(st.local, s.target(0.0)), "a camera-gate blip holds");
        // the gate order: AC-130 before first person, the peek before first person (a peek is FP on)
        let base = GateIn { cfg_on: true, hook: true, in_world: true, mode_on: true, cam_valid: true, ..Default::default() };
        assert_eq!(gate(&base), Ok(()));
        assert_eq!(gate(&GateIn { hook: false, ..base }), Err(Why::Cfg));
        assert_eq!(gate(&GateIn { ac130: true, fp: true, ..base }), Err(Why::Ac130));
        assert_eq!(gate(&GateIn { peek: true, fp: true, ..base }), Err(Why::Peek));
        assert_eq!(gate(&GateIn { fp: true, ..base }), Err(Why::FirstPerson));
        assert_eq!(gate(&GateIn { mode_on: false, ..base }), Err(Why::ModeOff));
        assert_eq!(gate(&GateIn { dead: true, ..base }), Err(Why::Dead));
        assert_eq!(gate(&GateIn { menu: true, ..base }), Err(Why::Menu));
        assert_eq!(gate(&GateIn { latched: true, ..base }), Err(Why::Cutscene));
        assert_eq!(gate(&GateIn { mounted: true, ..base }), Err(Why::Mounted));
        assert_eq!(gate(&GateIn { mounted: true, mounted_ok: true, ..base }), Ok(()));
        assert_eq!(gate(&GateIn { cam_valid: false, ..base }), Err(Why::Camera));
    }

    #[test]
    fn c6_shoulder_cfg() {
        use crate::config::Config;
        let d = Config::default().third_person;
        assert!(!d.shoulder().on(), "code default = off");
        assert_eq!((d.shoulder_side.as_str(), d.shoulder_blend_s, d.shoulder_margin_m, d.shoulder_mounted), ("right", 0.15, 0.20, false));
        let c = Config::parse("[third_person]\nshoulder_offset_m = [0.55, 0.25, 0.0]\nshoulder_ads_offset_m = [0.45, 0.15, -0.6]\nshoulder_side = \"left\"\n")
            .unwrap()
            .third_person;
        let s = c.shoulder();
        assert!(s.on() && s.side == Side::Left && close(s.target(1.0), V3::new(-0.45, 0.15, -0.6)));
        assert!(Config::parse("[third_person]\nshoulder_side = \"up\"\n").is_err());
        assert!(Config::parse("[third_person]\nshoulder_offset_m = [2.0, 0.0, 0.0]\n").is_err());
        assert!(Config::parse("[third_person]\nshoulder_offset_m = [0.5, 0.0]\n").is_err());
        assert!(Config::parse("[third_person]\nshoulder_margin_m = -0.1\n").is_err());
        assert!(Config::parse("[third_person]\nshoulder_pitch = 1.0\n").is_err(), "unknown key");
        // the largest accepted offset fits the detour's bound
        assert!(world_offset(V3::new(0.0, 0.0, 1.0), V3::new(1.5, 1.0, 2.0)).is_some());
    }

    #[test]
    fn c6_shoulder_shot_stays_on_the_offset_camera_ray() {
        // the third-person bullet origin is a point ON the camera's centre ray (aim::bullet_origin): with the camera
        // moved over the shoulder (the chest off the ray) the bullet still flies exactly along the crosshair ray
        let er_cam = V3::new(0.0, 1.9, -3.5);
        let fwd = V3::new(0.0, -0.1, 1.0).normalize().unwrap();
        let cam = er_cam + world_offset(fwd, p().target(0.0)).unwrap();
        let chest = V3::new(0.0, 1.3, 0.0);
        let o = crate::aim::bullet_origin(cam, fwd, chest, 0.5);
        let along = (o - cam).dot(fwd);
        let off_ray = (o - cam) - fwd * along;
        assert!(off_ray.len() < 1e-4 && along > 0.0, "{off_ray:?}");
        assert!(crate::aim::check_shot_fp(cam, chest, o, fwd, 0.5, 300.0, false).is_ok());
    }
}
