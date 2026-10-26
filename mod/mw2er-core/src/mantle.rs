//! M8 MW2 mantling (docs/M8_DESIGN.md sections 3-7): procedural ledge detection with ray casts, the SP mantle path
//! (exe sigmoid rise, then the forward move) and its per-tick re-validation, the weapon gate and the negative cache.
//! Pure: the controller (`move_sim::Player`) runs it against the game's rays (the DLL), the synthetic world (tests,
//! `movebench`).
//!
//! IW4 inches, y up. Every ray goes through [`RayBudget::cast`] on the mantle's own budget (never the main budget's
//! vertical reserve). Every ray whose MISS would mean "free" starts inside the current capsule or on a segment an
//! earlier ray proved free (the start-inside-solid axiom of M5 5.1: such a ray misses, so a bad origin can only turn
//! an accept into a refuse). Rays whose miss refuses (R0b, R3, R4, R5 / O1) may start anywhere; R0's miss leads on
//! to R0b, so R0 starts inside the capsule like the other free-on-miss rays.

use crate::move_collide::{RayBudget, RayKind, RayWorld, normal3};
use crate::movement_data::MantleData;
use crate::vec3::{UP, V3};

/// The lift above the ledge the feet travel at during the forward move (0.05 m).
pub const LIFT_IN: f32 = 0.05 / 0.0254;
/// The end centre may lie at most this far below the lip and still count as the same top (inches): lower is a thin
/// wall with something behind it (the over path) [I].
pub const TOP_STEP_IN: f32 = 4.0;
/// A deeper second try of the top probe on a rounded rock (section 4.3).
pub const RETRY_DEEPER_IN: f32 = 8.0;
/// Characters closer than this to the top (plus the player radius, metres) make it occupied.
pub const OCCUPIED_M: f32 = 0.45;
/// The fraction of the side offset of the feeler / side / width rays (x radius).
pub const SIDE_FRAC: f32 = 0.7;
/// The negative cache re-opens when the feet moved this far (inches, 0.10 m) or the view turned this much (deg).
pub const CACHE_MOVE_IN: f32 = 0.10 / 0.0254;
pub const CACHE_YAW_DEG: f32 = 10.0;
/// After a re-arm, no mantle check for this long (s).
pub const REARM_BLOCK_S: f64 = 0.3;
/// R0 sits this far above the step height (M8 review: at step + lift a 18-20 in top was neither stepped nor
/// mantled; now only 18.0-18.25 in is left to the step's own rounding).
pub const KNEE_ABOVE_STEP_IN: f32 = 0.25;
/// A lip higher than the end centre by more than this is a raised rim (a curb, a coping, a rock rim): the forward
/// move crosses over the rim and sets down only once the axis stands over the probed end centre (M8 review).
pub const RIM_MIN_IN: f32 = 0.25;
/// The set-down after the forward move over a raised rim (s, x `time_scale`) [I].
pub const RIM_SETDOWN_S: f32 = 0.05;
/// The detection's worst case (R0 + R0b, both feelers, the column, R2, R3, the top probes and their deeper retry,
/// R7, R8 / R9 / Rf / R10 / R10'; run 18: + the 2 back rays of a depenetration and the 2 rays of the narrow-top retry;
/// 2026-10-21: + R2b and its face probe R2f over a rising top, still 24: there the deeper retry does not follow the
/// narrow one); `max_rays` below it can refuse `budget` on a deep retry.
pub const DETECT_MAX_RAYS: u32 = 24;
/// Run 18: the narrow-top retry probes the sides at this fraction of the radius (a rock top 0.6 r wide carries the
/// capsule's axis; MW2's own box is 15 in either side of the origin) [I].
pub const NARROW_SIDE_FRAC: f32 = 0.35;
/// Re-validation rays of one running-mantle substep at most (rise: up + head forward + ledge check; forward move:
/// feet / waist / head + ledge check). The per-tick mantle budget is `max_rays + SUBSTEP_RAYS * (substeps - 1)`.
pub const SUBSTEP_RAYS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMode {
    /// MW2 SP: jump held (or pressed).
    Mw2,
    /// Only a jump press.
    Press,
    /// Jump held, or forward input into the face (the MW2 MP automantle perk).
    Auto,
}

impl KeyMode {
    pub fn label(self) -> &'static str {
        match self {
            KeyMode::Mw2 => "mw2",
            KeyMode::Press => "press",
            KeyMode::Auto => "auto",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmMode {
    /// MW2: the gun quick-drops and quick-raises.
    Drop,
    /// The verified sprint pose instead (fallback).
    Sprint,
}

/// The controller's mantle settings (config + MW2 data), inches / seconds. `Copy` (part of `SimCfg`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MantleCfg {
    pub enabled: bool,
    pub key_mode: KeyMode,
    pub require_forward: bool,
    /// Top above the ground the player left (the user's 2.0 m cap, I).
    pub max_height_in: f32,
    /// Top above the current feet (MW2 60, V).
    pub max_rise_in: f32,
    pub check_angle_deg: f32,
    pub check_range_in: f32,
    pub time_scale: f32,
    pub over: bool,
    pub over_max_drop_in: f32,
    pub crouch_fallback: bool,
    pub low_keeps_weapon: bool,
    pub vm: VmMode,
    pub max_rays: u32,
    pub refuse_cache_s: f64,
    /// A planar ER displacement over this (inches) during a mantle aborts it (`er_push`).
    pub er_push_abort_in: f32,
    /// The walk speed the planar end velocity is clamped to (in/s).
    pub end_speed_max: f32,
    /// Run 18: a face nearer than r + s/2 (the capsule already reaches into it) is backed off by up to this much
    /// (inches; 0 = refuse `path_blocked` as in M8).
    pub depenetrate_in: f32,
}

impl MantleCfg {
    /// The design defaults with MW2's numbers from the data (`enabled` only with data).
    pub fn defaults(d: Option<&MantleData>) -> MantleCfg {
        MantleCfg {
            enabled: d.is_some_and(|m| m.enable),
            key_mode: KeyMode::Mw2,
            require_forward: false,
            max_height_in: d.map_or(2.0, |m| m.max_height_m) / 0.0254,
            max_rise_in: d.map_or(60.0, |m| m.max_rise),
            check_angle_deg: d.map_or(60.0, |m| m.check_angle_deg),
            check_range_in: d.map_or(20.0, |m| m.check_range),
            time_scale: 1.0,
            over: true,
            over_max_drop_in: 2.0 / 0.0254,
            crouch_fallback: true,
            low_keeps_weapon: true,
            vm: VmMode::Drop,
            max_rays: 24,
            refuse_cache_s: 0.15,
            er_push_abort_in: 0.30 / 0.0254,
            end_speed_max: 190.0,
            depenetrate_in: 0.30 / 0.0254,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MantleKind {
    Up,
    Crouch,
    Over,
}

impl MantleKind {
    pub fn label(self) -> &'static str {
        match self {
            MantleKind::Up => "up",
            MantleKind::Crouch => "crouch",
            MantleKind::Over => "over",
        }
    }
}

/// Why a detection refused (section 4.3, the fixed set of 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refuse {
    NoFace,
    Angle,
    Ceiling,
    TooHigh,
    TooLow,
    NoLedge,
    Slope,
    Edge,
    Narrow,
    NoHeadroom,
    PathBlocked,
    OverBlocked,
    OverDrop,
    Occupied,
    Budget,
}

impl Refuse {
    pub const ALL: [Refuse; 15] = [
        Refuse::NoFace,
        Refuse::Angle,
        Refuse::Ceiling,
        Refuse::TooHigh,
        Refuse::TooLow,
        Refuse::NoLedge,
        Refuse::Slope,
        Refuse::Edge,
        Refuse::Narrow,
        Refuse::NoHeadroom,
        Refuse::PathBlocked,
        Refuse::OverBlocked,
        Refuse::OverDrop,
        Refuse::Occupied,
        Refuse::Budget,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Refuse::NoFace => "no_face",
            Refuse::Angle => "angle",
            Refuse::Ceiling => "ceiling",
            Refuse::TooHigh => "too_high",
            Refuse::TooLow => "too_low",
            Refuse::NoLedge => "no_ledge",
            Refuse::Slope => "slope",
            Refuse::Edge => "edge",
            Refuse::Narrow => "narrow",
            Refuse::NoHeadroom => "no_headroom",
            Refuse::PathBlocked => "path_blocked",
            Refuse::OverBlocked => "over_blocked",
            Refuse::OverDrop => "over_drop",
            Refuse::Occupied => "occupied",
            Refuse::Budget => "budget",
        }
    }

    pub fn index(self) -> usize {
        Refuse::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// Why a running mantle stopped early (section 5.3 / 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortWhy {
    Blocked,
    LedgeGone,
    ErPush,
    Warp,
    ReadFail,
    Budget,
    /// The DLL's `position_insane` guard resynced to ER (M8 review: logged like the other aborts).
    Insane,
    /// A handback (`move_logic::Reason` label).
    Handback(&'static str),
}

impl AbortWhy {
    pub fn label(self) -> String {
        match self {
            AbortWhy::Blocked => "blocked".into(),
            AbortWhy::LedgeGone => "ledge_gone".into(),
            AbortWhy::ErPush => "er_push".into(),
            AbortWhy::Warp => "warp".into(),
            AbortWhy::ReadFail => "read_fail".into(),
            AbortWhy::Insane => "position_insane".into(),
            AbortWhy::Budget => "budget".into(),
            AbortWhy::Handback(r) => format!("handback:{r}"),
        }
    }
}

/// What a detection sees: the feet, the view, the capsule, the take-off ground and the nearby characters (inches).
#[derive(Clone, Copy, Debug)]
pub struct Query<'a> {
    pub feet: V3,
    pub fwd: V3,
    pub radius: f32,
    pub skin: f32,
    pub hull_stand: f32,
    pub hull_crouch: f32,
    /// The feet height at the last grounded tick (the user's cap is measured from it).
    pub ground_y0: f32,
    pub walkable_y: f32,
    pub others: &'a [V3],
}

/// An accepted ledge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ledge {
    pub kind: MantleKind,
    /// Top above the current feet (inches).
    pub rise: f32,
    pub ledge_y: f32,
    /// The feet height of the forward move: max(lip, end centre) + lift (M8 review: a raised rim is crossed over).
    pub y_cross: f32,
    /// The lip stands more than [`RIM_MIN_IN`] above the end centre (the set-down waits for the end column).
    pub rim: bool,
    /// Planar unit direction into the face.
    pub dir: V3,
    /// Planar distance from the feet to the face plane.
    pub d: f32,
    /// Planar move onto the top (Up/Crouch) or over the wall (Over).
    pub x: f32,
    /// The end feet (Up/Crouch: on the top; Over: the release point at the ledge height + lift).
    pub end: V3,
    /// The top point the ledge_gone checks look at (R3's hit).
    pub top: V3,
    pub row: usize,
    /// Over: the far-side floor height (O1).
    pub drop_y: Option<f32>,
    pub rays: u32,
    pub retried: bool,
    /// Run 18: where the climb starts: the feet, or the feet backed off a face the capsule reached into.
    pub start: V3,
    /// Run 18: how far the start was backed off (inches; 0 = none).
    pub depen: f32,
    /// Run 18: the top's sides were probed at the narrow offset (`NARROW_SIDE_FRAC`).
    pub narrow_top: bool,
    /// 2026-10-21: how far R2b raised the over-the-top line over a top rising away (inches; 0 = not raised).
    pub line_up: f32,
}

/// The pattern's rays, for the origin-mutation tests (section 12.1 `mantle_origin_`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayId {
    R0,
    /// M8 review: the second face ray at chest height when R0 misses (an undercut shelf).
    R0b,
    R1,
    R1b,
    Col,
    R2,
    /// 2026-10-21: the line again, higher, over a top rising away (only when R2 met it inside x + r).
    R2b,
    /// 2026-10-21 review: down from R2b's line just in front of R2's hit: the slope there, not a box's face.
    R2f,
    R3,
    R4a,
    R4b,
    R5,
    R7,
    R8,
    R9,
    /// M8 review: the capsule's front column, down from R9's free line to the knee (rock reaching into the climb).
    Rf,
    R10a,
    R10b,
    O1,
    O2,
    /// Run 18: the two back rays that prove the space behind a depenetration.
    Back,
}

/// A test hook: moves the origin of one ray (`None` = none moved).
pub type OriginHook<'a> = Option<&'a dyn Fn(RayId, V3) -> V3>;

fn planar(v: V3) -> V3 {
    V3::new(v.x, 0.0, v.z)
}

/// The horizontal unit vector to the right of `d` (same convention as move_collide).
pub fn side_of(d: V3) -> V3 {
    V3::new(d.z, 0.0, -d.x)
}

fn normal_from_points(a: V3, b: V3, dir: V3) -> Option<V3> {
    let t = planar(b - a).normalize()?;
    let n = V3::new(-t.z, 0.0, t.x);
    Some(if n.dot(dir) > 0.0 { -n } else { n })
}

struct Caster<'b, 'w, 'h> {
    b: &'b mut RayBudget,
    w: &'w mut dyn RayWorld,
    hook: OriginHook<'h>,
}

impl Caster<'_, '_, '_> {
    fn cast(&mut self, id: RayId, o: V3, delta: V3) -> Result<Option<V3>, Refuse> {
        let o = match self.hook {
            Some(h) => h(id, o),
            None => o,
        };
        self.b.cast(self.w, RayKind::Vertical, o, delta).map_err(|_| Refuse::Budget)
    }
}

/// The ledge detection of section 4 (at most 16 rays at the default budget). `Err((why, rise))`: the refusal and the
/// top height it saw (0 when unknown).
pub fn find_ledge(b: &mut RayBudget, w: &mut dyn RayWorld, q: &Query, c: &MantleCfg, md: &MantleData) -> Result<Ledge, (Refuse, f32)> {
    find_ledge_hooked(b, w, q, c, md, None)
}

pub fn find_ledge_hooked(
    b: &mut RayBudget,
    w: &mut dyn RayWorld,
    q: &Query,
    c: &MantleCfg,
    md: &MantleData,
    hook: OriginHook<'_>,
) -> Result<Ledge, (Refuse, f32)> {
    let used0 = b.used;
    let mut k = Caster { b, w, hook };
    let r = q.radius;
    let s = q.skin;
    let p = q.feet;
    let lift = LIFT_IN;
    let rise_min = md.rise_min;
    let cap_y = q.ground_y0 + c.max_height_in;
    let top_lim = (p.y + c.max_rise_in).min(cap_y);
    let Some(fwd) = planar(q.fwd).normalize() else { return Err((Refuse::NoFace, 0.0)) };
    if top_lim <= p.y + rise_min + 0.5 {
        // the cap leaves no room above a step (falling far below the take-off, or a low cap)
        return Err((Refuse::TooHigh, 0.0));
    }
    let e = |why: Refuse| (why, 0.0f32);
    // ---- R0: the face just above the step height (inside the capsule); R0b at chest height when R0 misses ----
    let knee_y = p.y + rise_min + KNEE_ABOVE_STEP_IN;
    let knee = V3::new(p.x, knee_y, p.z);
    let reach = r + c.check_range_in + s;
    let (face_o, w0) = match k.cast(RayId::R0, knee, fwd * reach).map_err(e)? {
        Some(h) => (knee, h),
        None => {
            // an undercut shelf, a slab or a cart bed with a gap below the lip (MW2's front probe is a box from 0 to
            // 70 in): one more face ray half way up the window, still inside the capsule
            let chest = V3::new(p.x, (0.5 * (knee_y + top_lim - 2.0)).max(knee_y + 1.0), p.z);
            match k.cast(RayId::R0b, chest, fwd * reach).map_err(e)? {
                Some(h) => (chest, h),
                None => return Err(e(Refuse::NoFace)),
            }
        }
    };
    // ---- R1 / R1': the wall normal from two feelers at +-0.7 r (inside the capsule) ----
    let side = side_of(fwd);
    let opp = |n: V3| n.dot(fwd);
    let angle_ok = |n: V3| (-n).angle_deg(fwd) <= c.check_angle_deg + 1e-3;
    let mut normals: Vec<V3> = Vec::with_capacity(2);
    if let Some(h1) = k.cast(RayId::R1, face_o + side * (SIDE_FRAC * r), fwd * reach).map_err(e)?
        && let Some(n) = normal_from_points(w0, h1, fwd)
    {
        normals.push(n);
    }
    // the second feeler only when the first did not settle it (a miss, or a face beyond the view angle: a crease)
    if normals.first().is_none_or(|n| !angle_ok(*n))
        && let Some(h2) = k.cast(RayId::R1b, face_o - side * (SIDE_FRAC * r), fwd * reach).map_err(e)?
        && let Some(n) = normal_from_points(w0, h2, fwd)
    {
        normals.push(n);
    }
    let n = match normals.as_slice() {
        [] => -fwd,
        [a] => *a,
        [a, b2, ..] => {
            if a.dot(*b2) > 0.98 {
                planar(*a + *b2).normalize().unwrap_or(*a)
            } else if opp(*a) <= opp(*b2) {
                *a
            } else {
                *b2
            }
        }
    };
    if !angle_ok(n) {
        return Err(e(Refuse::Angle));
    }
    let dir = planar(-n).normalize().unwrap_or(fwd);
    let mut d = planar(w0 - p).dot(dir).max(0.0);
    // the walk keeps radius + skin from a wall it saw; a face nearer than r + s/2 is rock already reaching into the
    // capsule between the walk's probe heights (M8 review): rising there would drag the capsule through it.
    // Run 18 (34 of 209 refusals, `path_blocked rays=3` against a wagon): when the capsule reaches into the face by
    // at most `depenetrate_in`, back the start off to r + s from it, if two rays from the axis (knee and chest,
    // inside the capsule) prove the space behind free; the rest of the pattern then runs from the backed-off feet
    let mut p = p;
    let mut depen = 0.0f32;
    if d < r + 0.5 * s {
        let need = r + s - d;
        if c.depenetrate_in <= 0.0 || need > c.depenetrate_in {
            return Err(e(Refuse::PathBlocked));
        }
        for hy in [rise_min + KNEE_ABOVE_STEP_IN, q.hull_stand - 4.0] {
            let o = V3::new(p.x, p.y + hy, p.z);
            if k.cast(RayId::Back, o, -dir * (need + r + s)).map_err(e)?.is_some() {
                return Err(e(Refuse::PathBlocked));
            }
        }
        p = p - dir * need;
        depen = need;
        d = r + s;
    }
    let p = p;
    let x = md.forward.max(d + r + s + 1.0);
    // ---- the rise column (inside the capsule upward): R1a + R6 of the design in one ray ----
    let col_lo = p.y + q.hull_stand - 1.0;
    let col_hi = top_lim + q.hull_stand + lift + 1.0;
    let ceil_y = match k.cast(RayId::Col, V3::new(p.x, col_lo, p.z), UP * (col_hi - col_lo)).map_err(e)? {
        Some(h) => h.y,
        None => f32::INFINITY,
    };
    let y2 = top_lim + lift;
    if y2 + 1.0 > col_lo && ceil_y < y2 + 1.0 {
        return Err(e(Refuse::Ceiling));
    }
    // ---- R2: the over-the-top line (from the capsule / the free column) ----
    let line_len = x + RETRY_DEEPER_IN + r + s;
    let mut line_free = match k.cast(RayId::R2, V3::new(p.x, y2, p.z), dir * line_len).map_err(e)? {
        Some(h) => {
            let t = planar(h - p).dot(dir);
            if t < d + 1.0 + s {
                // the face goes on above the reach
                return Err(e(Refuse::TooHigh));
            }
            t - s
        }
        None => line_len,
    };
    // 2026-10-21 (`path_blocked rays=4` x25, a 1.15 m step on a 27 deg hillside): the line runs at the reach height
    // (feet + 60 in), not over the top, so a top rising away at more than ~26 deg met it inside x + r. R2b runs the
    // line again higher, over a walkable slope from R2's hit to the line's end (inside the capsule / the free
    // column); R2's hit must then lie on the top the end centre finds (checked below), else path_blocked as before
    let mut rising: Option<(f32, f32)> = None;
    let y2_reach = y2;
    let mut y2 = y2;
    if x + r > line_free {
        let t = line_free + s;
        let wy = q.walkable_y.clamp(0.05, 1.0);
        let tan_max = (1.0 - wy * wy).max(0.0).sqrt() / wy;
        let y2b = y2 + (line_len - t) * tan_max + 1.0;
        if y2b + 1.0 > ceil_y.min(col_hi) {
            return Err(e(Refuse::PathBlocked));
        }
        line_free = match k.cast(RayId::R2b, V3::new(p.x, y2b, p.z), dir * line_len).map_err(e)? {
            Some(h) => planar(h - p).dot(dir) - s,
            None => line_len,
        };
        if x + r > line_free {
            return Err(e(Refuse::PathBlocked));
        }
        // review: R2's hit must be on a slope, not the vertical face of a crate / a second step on a flat top (the
        // top check below compares one point and passes both): down from the raised line just in front of the hit,
        // the slope is at most (1 + s/2) x tan_max lower; in front of a face the ray falls to the top far below
        let tol = (1.0 + 0.5 * s) * tan_max + 1.0;
        let probe = planar(p) + dir * (t - 0.5 * s - 1.0);
        match k.cast(RayId::R2f, V3::new(probe.x, y2b, probe.z), UP * -(y2b - (y2 - tol))).map_err(e)? {
            Some(h) if h.y <= y2 + 1.0 => {}
            _ => return Err(e(Refuse::PathBlocked)),
        }
        rising = Some((t, y2));
        y2 = y2b;
    }
    let y2 = y2;
    // ---- R3 at the lip (the top of the face itself), down from the line ----
    let span = y2 - (p.y + rise_min);
    // down from the line height at a planar point; a side probe sits half a radius ahead of its centre probe (the
    // centre and the two sides span a triangle for the normal)
    let down_at = |k: &mut Caster, id: RayId, at: V3| -> Result<Option<V3>, (Refuse, f32)> {
        k.cast(id, V3::new(at.x, y2, at.z), UP * -span).map_err(e)
    };
    let along = |xx: f32| planar(p) + dir * xx;
    let down = |k: &mut Caster, id: RayId, xx: f32, sd: f32| -> Result<Option<V3>, (Refuse, f32)> {
        let ahead = if sd != 0.0 { 0.5 * r } else { 0.0 };
        down_at(k, id, along(xx + ahead) + side * sd)
    };
    let window = |y: f32| -> Result<(), (Refuse, f32)> {
        let rise = y - p.y;
        if rise <= rise_min + 0.01 {
            return Err((Refuse::TooLow, rise));
        }
        if rise > c.max_rise_in + 0.05 || y > cap_y + 0.05 || y > top_lim + 0.05 {
            return Err((Refuse::TooHigh, rise));
        }
        Ok(())
    };
    // the lip: just past the face point R0 met (on a corner of a rock that is not on the line from the feet)
    // (M8 review: 1 in + s/2 past it, was s + 2: a rim or coping down to ~1.6 in deep is probed as the lip; a
    // narrower one is left to R8, which refuses it)
    let lip_xz = planar(w0) + dir * (0.5 * s + 1.0);
    let lip = down_at(&mut k, RayId::R3, lip_xz)?.ok_or(e(Refuse::NoLedge))?;
    window(lip.y)?;
    // ---- the end centre (R5 of the design: deep enough to stand on), its sides (R4 / R4'): the slope ----
    let slope_ok = |top: V3, a: Option<V3>, bb: Option<V3>| -> Result<(), (Refuse, f32)> {
        let rise = top.y - p.y;
        let (Some(a), Some(bb)) = (a, bb) else { return Err((Refuse::Edge, rise)) };
        if (a.y - top.y).abs() > rise_min || (bb.y - top.y).abs() > rise_min {
            return Err((Refuse::Edge, rise));
        }
        match normal3(top, a, bb) {
            Some(nn) if nn.y >= q.walkable_y => Ok(()),
            _ => Err((Refuse::Slope, rise)),
        }
    };
    let mut x = x;
    let mut retried = false;
    let mut narrow_top = false;
    // a centre well above the lip: the lip ray met a face leaning back (a rock), the centre is the top (R8 proves the
    // way over the edge); well below it or a miss: the top ends before the end footprint (narrow)
    let mut centre = down(&mut k, RayId::R5, x, 0.0)?;
    let mut deep = centre.is_some_and(|c| c.y >= lip.y - TOP_STEP_IN);
    let top;
    if deep {
        let mut t0 = centre.unwrap_or(lip);
        window(t0.y)?;
        let a = down(&mut k, RayId::R4a, x, SIDE_FRAC * r)?;
        let bb = down(&mut k, RayId::R4b, x, -SIDE_FRAC * r)?;
        let mut first = slope_ok(t0, a, bb);
        let narrow_retry = matches!(first, Err((Refuse::Edge, _)));
        // run 18 (13 `edge` refusals on rocks 0.5-1.4 m): a top narrower than the side probes (a rock crest, a beam)
        // is probed once more at the narrow offset; it must still be level enough to stand on
        if narrow_retry {
            let a2 = down(&mut k, RayId::R4a, x, NARROW_SIDE_FRAC * r)?;
            let b2 = down(&mut k, RayId::R4b, x, -NARROW_SIDE_FRAC * r)?;
            first = slope_ok(t0, a2, b2);
            narrow_top = first.is_ok();
        }
        if let Err((why, rise)) = first {
            // a rounded rock: the shoulder is flatter further in (only if the free line covers it)
            // (2026-10-21 review: over a raised line not after the narrow retry too, which keeps R2f in 24 rays)
            if why == Refuse::Slope && x + RETRY_DEEPER_IN + r <= line_free && !(rising.is_some() && narrow_retry) {
                retried = true;
                x += RETRY_DEEPER_IN;
                centre = down(&mut k, RayId::R5, x, 0.0)?;
                let Some(c2) = centre.filter(|c| c.y >= lip.y - TOP_STEP_IN) else { return Err((Refuse::Slope, rise)) };
                window(c2.y)?;
                let a2 = down(&mut k, RayId::R4a, x, SIDE_FRAC * r)?;
                let b2 = down(&mut k, RayId::R4b, x, -SIDE_FRAC * r)?;
                slope_ok(c2, a2, b2)?;
                t0 = c2;
            } else {
                return Err((why, rise));
            }
        }
        top = t0;
    } else {
        // narrow (a thin wall): its top at the lip must be walkable too
        // the sides 2 in past the lip (a thin wall's top is shallow)
        let a = down_at(&mut k, RayId::R4a, lip_xz + dir * 2.0 + side * (SIDE_FRAC * r))?;
        let bb = down_at(&mut k, RayId::R4b, lip_xz + dir * 2.0 - side * (SIDE_FRAC * r))?;
        slope_ok(lip, a, bb)?;
        top = lip;
        deep = false;
    }
    // 2026-10-21: over a raised line, R2's hit is the top itself: between level and the walkable slope from the end
    // centre to it (1 in either way). A beam, a lip or a second tier above the top is not, and nor is a thin wall
    if let Some((t_hit, y_hit)) = rising {
        let wy = q.walkable_y.clamp(0.05, 1.0);
        let tan_max = (1.0 - wy * wy).max(0.0).sqrt() / wy;
        let dx = t_hit - planar(top - p).dot(dir);
        let up = y_hit - top.y;
        if !deep || up < (dx * tan_max).min(0.0) - 1.0 || up > (dx * tan_max).max(0.0) + 1.0 {
            return Err((Refuse::PathBlocked, top.y - p.y));
        }
    }
    let ledge_y = top.y;
    let rise = ledge_y - p.y;
    let er = |why: Refuse| (why, rise);
    // ---- occupied (0 rays) ----
    let rr = r + OCCUPIED_M / 0.0254;
    if q.others.iter().any(|o| planar(*o - top).len() < rr && (o.y - ledge_y).abs() < 40.0) {
        return Err(er(Refuse::Occupied));
    }
    let max_row = md.rows.first().map_or(57.0, |r| r.height);
    let over = !deep;
    if over && (!c.over || rise > max_row + 3.0) {
        return Err(er(Refuse::Narrow));
    }
    // ---- the end stance: the column, then the end headroom (R7, Up only) ----
    let y_top = ledge_y + lift;
    // the forward move crosses at the higher of the lip and the end centre (M8 review: a rim 2-4 in above the
    // centre, a curb or a wall coping, was crossed at centre + lift, through the rim)
    let rim = !over && lip.y > ledge_y + RIM_MIN_IN;
    let y_cross = lip.y.max(ledge_y) + lift;
    let mut crouch = false;
    if ceil_y < y_cross + q.hull_stand {
        if c.crouch_fallback && !over && ceil_y >= y_cross + q.hull_crouch {
            crouch = true;
        } else {
            return Err(er(Refuse::Ceiling));
        }
    }
    if !over && y2 < y_top + q.hull_stand {
        let q0 = V3::new(top.x, y2, top.z);
        if let Some(h) = k.cast(RayId::R7, q0, UP * (y_top + q.hull_stand - y2)).map_err(er)? {
            if c.crouch_fallback && h.y >= y_top + q.hull_crouch {
                crouch = true;
            } else {
                return Err(er(Refuse::NoHeadroom));
            }
        }
    }
    let h_end = if crouch { q.hull_crouch } else { q.hull_stand };
    // ---- the path: R8 low, R9 high (from the free column / capsule), R10 fan at mid height ----
    let blocked = if over { Refuse::OverBlocked } else { Refuse::PathBlocked };
    let x_over = (md.forward + md.over_forward).max(d + r + r + s);
    let x_move = if over { x_over } else { x };
    // the feet's own path (M8 review: 1 in above it missed a rim between the path and the line)
    let lo = y_cross - 0.25;
    let len = x_move + r + s;
    // the feet line only to the end point: a top rising further in (a rock crown) is ground ahead, not a lip
    if k.cast(RayId::R8, V3::new(p.x, lo, p.z), dir * x_move).map_err(er)?.is_some() {
        return Err(er(blocked));
    }
    let hi = y_cross + h_end - 4.0;
    if k.cast(RayId::R9, V3::new(p.x, hi, p.z), dir * len).map_err(er)?.is_some() {
        return Err(er(blocked));
    }
    // Rf: the capsule's front column through the whole climb, from R9's free line straight down to the knee. One
    // ray sees any rock bulging toward the player between the knee and the head end of the rise (M8 review: a slab
    // 12 in proud of the face at 26-32 in was accepted and the capsule ran through it)
    let front = V3::new(p.x, hi, p.z) + dir * (r - 0.25);
    if k.cast(RayId::Rf, front, UP * -(hi - knee_y)).map_err(er)?.is_some() {
        return Err(er(Refuse::PathBlocked));
    }
    let mid = y_cross + h_end * 0.5;
    let fan = |sgn: f32| -> V3 {
        let t = dir * x_move + side * (sgn * SIDE_FRAC * r);
        let l = t.len();
        t * ((l + s) / l.max(1e-3))
    };
    if k.cast(RayId::R10a, V3::new(p.x, mid, p.z), fan(1.0)).map_err(er)?.is_some() {
        return Err(er(blocked));
    }
    // ---- over: the far side (O1 floor within over_max_drop, O2 room to stand), R10' dropped ----
    let mut drop_y = None;
    let end;
    if over {
        let q_over = V3::new(p.x, lo, p.z) + dir * x_over;
        let o1 = k.cast(RayId::O1, q_over, UP * -(lo - (ledge_y - c.over_max_drop_in))).map_err(er)?;
        let Some(f) = o1 else { return Err(er(Refuse::OverDrop)) };
        drop_y = Some(f.y);
        if k.cast(RayId::O2, q_over, UP * (y_cross + q.hull_stand - lo)).map_err(er)?.is_some() {
            return Err(er(Refuse::OverBlocked));
        }
        end = V3::new(q_over.x, y_cross, q_over.z);
        x = x_over;
    } else {
        if k.cast(RayId::R10b, V3::new(p.x, mid, p.z), fan(-1.0)).map_err(er)?.is_some() {
            return Err(er(blocked));
        }
        end = V3::new(top.x, ledge_y, top.z);
    }
    let kind = if over {
        MantleKind::Over
    } else if crouch {
        MantleKind::Crouch
    } else {
        MantleKind::Up
    };
    Ok(Ledge { kind, rise, ledge_y, y_cross, rim, dir, d, x, end, top, row: md.row_for(rise), drop_y, rays: k.b.used - used0, retried, start: p, depen, narrow_top, line_up: y2 - y2_reach })
}

// ---- curves --------------------------------------------------------------------------------------------------

/// A monotone cubic (Fritsch-Carlson) through samples at `t[i]`, with zero end tangents (C1 into and out of rest).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    t: Vec<f32>,
    y: Vec<f32>,
    m: Vec<f32>,
}

impl Curve {
    /// Uniform knots over 0..1.
    pub fn uniform(y: &[f32]) -> Curve {
        let n = y.len().max(2);
        let t: Vec<f32> = (0..y.len()).map(|i| i as f32 / (n - 1) as f32).collect();
        Curve::new(&t, y)
    }

    pub fn new(t: &[f32], y: &[f32]) -> Curve {
        let n = y.len();
        let mut m = vec![0.0f32; n];
        if n >= 3 {
            let delta: Vec<f32> = (0..n - 1).map(|i| (y[i + 1] - y[i]) / (t[i + 1] - t[i]).max(1e-6)).collect();
            for i in 1..n - 1 {
                m[i] = if delta[i - 1] * delta[i] <= 0.0 { 0.0 } else { (delta[i - 1] + delta[i]) * 0.5 };
            }
            // Fritsch-Carlson limiter (keeps every segment monotone)
            for i in 0..n - 1 {
                if delta[i] == 0.0 {
                    m[i] = 0.0;
                    m[i + 1] = 0.0;
                    continue;
                }
                let a = m[i] / delta[i];
                let b = m[i + 1] / delta[i];
                let s = a * a + b * b;
                if s > 9.0 {
                    let tau = 3.0 / s.sqrt();
                    m[i] = tau * a * delta[i];
                    m[i + 1] = tau * b * delta[i];
                }
            }
        }
        Curve { t: t.to_vec(), y: y.to_vec(), m }
    }

    pub fn at(&self, u: f32) -> f32 {
        let n = self.y.len();
        if n == 0 {
            return u.clamp(0.0, 1.0);
        }
        if n == 1 || u <= self.t[0] {
            return self.y[0];
        }
        if u >= self.t[n - 1] {
            return self.y[n - 1];
        }
        let i = self.t.partition_point(|x| *x <= u).saturating_sub(1).min(n - 2);
        let h = (self.t[i + 1] - self.t[i]).max(1e-6);
        let s = (u - self.t[i]) / h;
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        h00 * self.y[i] + h10 * h * self.m[i] + h01 * self.y[i + 1] + h11 * h * self.m[i + 1]
    }
}

fn smoothstep(k: f32) -> f32 {
    let k = k.clamp(0.0, 1.0);
    k * k * (3.0 - 2.0 * k)
}

// ---- the running mantle ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Rise,
    Cross,
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Held,
    Press,
    Auto,
}

impl Trigger {
    pub fn label(self) -> &'static str {
        match self {
            Trigger::Held => "held",
            Trigger::Press => "press",
            Trigger::Auto => "auto",
        }
    }
}

/// A mantle in progress (section 5.1). Positions are absolute feet (inches); [`Mantle::translate`] follows a re-base.
#[derive(Clone, Debug, PartialEq)]
pub struct Mantle {
    pub kind: MantleKind,
    pub t: f64,
    pub start: V3,
    pub ledge_y: f32,
    /// The feet height of the forward move (the rise ends here): max(lip, end centre) + lift.
    pub y_top: f32,
    /// A raised rim: the forward move stays at `y_top` and the set-down runs at the end column over `t_set`.
    pub rim: bool,
    pub t_set: f32,
    pub end: V3,
    pub top: V3,
    pub dir: V3,
    pub x: f32,
    pub rise: f32,
    pub row: usize,
    pub v0y: f32,
    pub t_rise: f32,
    pub t_cross: f32,
    /// Planar progress at which the axis is past the lip (the descent onto the top may start).
    pub lip: f32,
    pub last_valid: V3,
    pub checked_half: bool,
    pub checked_end: bool,
    pub drop_y: Option<f32>,
    pub trigger: Trigger,
    pub air: bool,
    pub rays: u32,
    pub drop_weapon: bool,
    /// Feet height reached so far (the rise never goes down).
    pub y_max: f32,
    rise_c: Curve,
    cross_c: Curve,
}

impl Mantle {
    /// Plans the path of `l` from the feet `start` (vertical speed `vy`, in/s).
    pub fn plan(l: &Ledge, start: V3, vy: f32, md: &MantleData, c: &MantleCfg, jump_v0: f32, trigger: Trigger, air: bool) -> Mantle {
        let ts = c.time_scale.clamp(0.25, 4.0);
        let row = &md.rows[l.row.min(md.rows.len() - 1)];
        let y_top = l.y_cross;
        // > 60 in by config: the rise takes longer in proportion [I]
        let tall = (l.rise / md.rows.first().map_or(57.0, |r| r.height)).max(1.0);
        let t_rise = md.rise_s * ts * if c.max_rise_in > 60.0 { tall } else { 1.0 };
        // M8 review: MW2's own times (V: 333 ms rise + 167 ms forward = 0.500 s up; + 333 ms over), with the forward
        // curve eased over the full-footprint distance X instead of MW2's 16 in [I: the move is faster, not longer;
        // the earlier 167 ms x X / 16 always hit its 2x cap and made every mantle 0.667 s]
        let (t_cross, cross_c) = if l.kind == MantleKind::Over {
            // MW2: the up anim's 16 in forward, then the over anim's 31 in, renormalized to one move over X
            let up = &row.cross;
            let ov = &md.over_x[row.over.min(2)];
            let total = md.forward + md.over_forward;
            let mut t = Vec::new();
            let mut y = Vec::new();
            let t_up = md.cross_s;
            for (i, v) in up.iter().enumerate() {
                t.push(t_up * i as f32 / (up.len() - 1).max(1) as f32);
                y.push(v * md.forward / total);
            }
            for (i, v) in ov.iter().enumerate().skip(1) {
                t.push(t_up + md.over_cross_s * i as f32 / (ov.len() - 1).max(1) as f32);
                y.push((md.forward + v * md.over_forward) / total);
            }
            let dur = t_up + md.over_cross_s;
            let tn: Vec<f32> = t.iter().map(|x| x / dur).collect();
            (dur * ts, Curve::new(&tn, &y))
        } else {
            (md.cross_s * ts, Curve::uniform(&row.cross))
        };
        let rim = l.rim && l.kind != MantleKind::Over;
        Mantle {
            kind: l.kind,
            t: 0.0,
            start,
            ledge_y: l.ledge_y,
            y_top,
            rim,
            t_set: if rim { RIM_SETDOWN_S * ts } else { 0.0 },
            end: l.end,
            top: l.top,
            dir: l.dir,
            x: l.x,
            rise: l.rise,
            row: l.row,
            v0y: vy.clamp(0.0, jump_v0.max(0.0)),
            t_rise,
            t_cross,
            lip: l.d + 1.0,
            last_valid: start,
            checked_half: false,
            checked_end: false,
            drop_y: l.drop_y,
            trigger,
            air,
            rays: l.rays,
            drop_weapon: row.drop_weapon || !c.low_keeps_weapon,
            y_max: start.y,
            rise_c: Curve::uniform(&row.rise),
            cross_c,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.t_rise + self.t_cross + self.t_set) as f64
    }

    pub fn frac(&self) -> f32 {
        (self.t / self.duration().max(1e-6)).clamp(0.0, 1.0) as f32
    }

    pub fn done(&self) -> bool {
        self.t >= self.duration() - 1e-9
    }

    /// The path point at mantle time `t` (s): (feet, phase). Monotone in y during the rise (with `y_max`).
    pub fn at(&self, t: f64) -> (V3, Phase) {
        let t = t.max(0.0) as f32;
        if t < self.t_rise {
            let u = t / self.t_rise.max(1e-6);
            let delta = self.y_top - self.start.y;
            let hermite = self.v0y * self.t_rise * u * (1.0 - u) * (1.0 - u);
            let y = (self.start.y + delta * self.rise_c.at(u) + hermite).min(self.y_top).max(self.start.y);
            return (V3::new(self.start.x, y, self.start.z), Phase::Rise);
        }
        let t_move = self.t_rise + self.t_cross;
        if self.rim && t >= t_move {
            // over a raised rim: the set-down straight down the end column (R5 proved it free down to the top)
            let k = (t - t_move) / self.t_set.max(1e-6);
            if k >= 1.0 {
                return (self.end, Phase::Cross);
            }
            let y = self.y_top - (self.y_top - self.ledge_y) * smoothstep(k);
            return (V3::new(self.end.x, y, self.end.z), Phase::Cross);
        }
        let v = ((t - self.t_rise) / self.t_cross.max(1e-6)).clamp(0.0, 1.0);
        let prog = self.x * self.cross_c.at(v).clamp(0.0, 1.0);
        let pl = V3::new(self.start.x, 0.0, self.start.z) + self.dir * prog;
        if self.kind == MantleKind::Over {
            return (V3::new(pl.x, self.y_top, pl.z), Phase::Over);
        }
        if self.rim {
            return (V3::new(pl.x, self.y_top, pl.z), Phase::Cross);
        }
        if v >= 1.0 {
            return (self.end, Phase::Cross);
        }
        // the 2 in lift is set down once the axis is past the lip (feet over the top)
        let k = if self.x > self.lip + 0.5 { (prog - self.lip) / (self.x - self.lip) } else { 0.0 };
        let y = self.y_top - (self.y_top - self.ledge_y) * smoothstep(k);
        (V3::new(pl.x, y, pl.z), Phase::Cross)
    }

    /// A re-base: the whole planned path moves with the player.
    pub fn translate(&mut self, by: V3) {
        self.start = self.start + by;
        self.end = self.end + by;
        self.top = self.top + by;
        self.last_valid = self.last_valid + by;
        self.ledge_y += by.y;
        self.y_top += by.y;
        self.y_max += by.y;
        if let Some(d) = self.drop_y.as_mut() {
            *d += by.y;
        }
    }
}

// ---- the negative cache (section 3 rule 7) ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefuseCache {
    until: f64,
    pos: V3,
    fwd: V3,
    pub skipped: u32,
}

impl RefuseCache {
    pub fn refused(&mut self, now: f64, pos: V3, fwd: V3, hold_s: f64) {
        self.until = now + hold_s;
        self.pos = pos;
        self.fwd = fwd;
    }

    /// The cache lets this check through (expired, or the feet / view moved enough).
    pub fn open(&mut self, now: f64, pos: V3, fwd: V3) -> bool {
        if now >= self.until {
            return true;
        }
        if (pos - self.pos).len() > CACHE_MOVE_IN || planar(fwd).angle_deg(planar(self.fwd)) > CACHE_YAW_DEG {
            self.until = 0.0;
            return true;
        }
        self.skipped += 1;
        false
    }

    pub fn translate(&mut self, by: V3) {
        self.pos = self.pos + by;
    }

    pub fn clear(&mut self) {
        self.until = 0.0;
    }
}

// ---- the weapon gate (section 6) -------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateEvent {
    /// The gun goes down (`why` = mantle | ladder): cancel ADS / reload, block fire.
    Lowered(&'static str),
    /// The quick raise starts.
    RaiseStart(&'static str),
    /// The raise ended: fire allowed again.
    Raised(&'static str),
}

/// Rows >= 33 in of a mantle (or a ladder) lower the gun: no fire, ADS or reload until the quick raise ends.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeaponGate {
    why: Option<&'static str>,
    raise_until: Option<f64>,
    raise_why: &'static str,
}

impl WeaponGate {
    /// One tick: `mantle` / `ladder` = the gun must be down now; `raise_s` = the gun's quick raise.
    pub fn update(&mut self, now: f64, mantle: bool, ladder: bool, raise_s: f64) -> Option<GateEvent> {
        let want = if ladder {
            Some("ladder")
        } else if mantle {
            Some("mantle")
        } else {
            None
        };
        match (self.why, want) {
            (None, Some(w)) => {
                self.why = Some(w);
                self.raise_until = None;
                Some(GateEvent::Lowered(w))
            }
            (Some(_), Some(w)) => {
                self.why = Some(w);
                None
            }
            (Some(w), None) => {
                self.why = None;
                self.raise_until = Some(now + raise_s.max(0.0));
                self.raise_why = w;
                Some(GateEvent::RaiseStart(w))
            }
            (None, None) => match self.raise_until {
                Some(t) if now >= t => {
                    self.raise_until = None;
                    Some(GateEvent::Raised(self.raise_why))
                }
                _ => None,
            },
        }
    }

    /// The gun is down (the viewmodel drops / stays down).
    pub fn lowered(&self) -> bool {
        self.why.is_some()
    }

    /// Fire, ADS and reload are blocked (down, or the raise still running).
    pub fn blocked(&self, now: f64) -> bool {
        self.why.is_some() || self.raise_until.is_some_and(|t| now < t)
    }

    pub fn reset(&mut self) {
        *self = WeaponGate::default();
    }
}

#[cfg(test)]
#[path = "mantle_tests.rs"]
mod tests;
