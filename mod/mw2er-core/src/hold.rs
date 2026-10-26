//! C6 (docs/C6_HOLD_DESIGN.md part 4-6): the MW2 world gun on the character's right-hand bone.
//!
//! Pure maths of the hand attachment: the raw Mtx43 bone entry and the Mtx44 chr matrix ER draws the body with, turned
//! into one column-form 3x4 (`xform::Mat34`, `p' = M * [p, 1]`) per player per frame, the per-gun grip, the signed axis
//! map from MW2 model axes onto the hand bone, the validation that sends a frame back to stage A (the fixed offset), the
//! jump guard / 1-frame smoothing, the rate-limited fallback log and the third-person ADS blend.
//!
//! Conventions (bones.md 2, [V] getters 0x140b43c80 / 0x140b43f60):
//! - a bone entry E is 3 x float4 rows `[R row | T]`: already column form (`p_model = R_E p + T_E`);
//! - the chr matrix M is row-major 4x4 with the translation in row 3 (`p' = p M`); its column form is `C = M^T`.
//!
//! Nothing here reads memory; the DLL reads the 48 + 64 bytes per player and calls [`validate_hand`].

use crate::coop_vis::IN_TO_M;
use crate::vec3::V3;
use crate::xform::{Mat34, m_mul};

/// A hand farther than this from the model origin (local) / the feet (world) is not a hand.
pub const HAND_MAX_M: f32 = 2.5;
/// The chr matrix translation must be this close to the physics position (same frame, no mid re-base read).
pub const CHR_MAX_M: f32 = 1.0;
/// Bone rows: equal lengths within this (relative to their mean), normalised |ri . rj| < ORTHO_TOL.
pub const ORTHO_TOL: f32 = 0.05;
/// The uniform scale a hand-bone entry may carry (body-proportion sliders; 1 for a default character).
pub const SCALE_MIN: f32 = 0.5;
pub const SCALE_MAX: f32 = 2.0;
/// det(R_H) must be +1 within this.
pub const DET_TOL: f32 = 0.1;
/// The anchor moving more than this between two good frames of one player is a jump (teleport / torn read).
pub const JUMP_M: f32 = 3.0;
/// The Mtx43 array count a player model can have.
pub const N_MIN: u32 = 8;
pub const N_MAX: u32 = 1024;
/// The fallback summary line every this many seconds while any fallback happens.
pub const FALLBACK_SUMMARY_S: f64 = 30.0;
/// `HOLD bone`: a havok model-pose translation within this of a FLVER entry is that FLVER bone.
pub const NAME_MATCH_M: f32 = 0.02;
/// Third-person ADS blend in / out time (docs 6).
pub const ADS_BLEND_S: f32 = 0.12;

/// Why a hand frame fell back to stage A (docs 5); the label is the log token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    Exe,
    NoBone,
    Read,
    Vtable,
    Range,
    Nan,
    Ortho,
    Space,
    ChrFar,
    Det,
    Jump,
    // C6 attach 2: why ER's own weapon model was not used this frame (the hand bone takes over; logged as `HOLD weapon
    // miss`, never a stage-A fallback by itself)
    NoWeapon,
    WeaponRead,
    WeaponVtable,
    WeaponBad,
    WeaponFar,
    WeaponDet,
    WeaponStale,
}

impl Reason {
    pub const ALL: [Reason; 18] = [
        Reason::Exe,
        Reason::NoBone,
        Reason::Read,
        Reason::Vtable,
        Reason::Range,
        Reason::Nan,
        Reason::Ortho,
        Reason::Space,
        Reason::ChrFar,
        Reason::Det,
        Reason::Jump,
        Reason::NoWeapon,
        Reason::WeaponRead,
        Reason::WeaponVtable,
        Reason::WeaponBad,
        Reason::WeaponFar,
        Reason::WeaponDet,
        Reason::WeaponStale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Reason::Exe => "exe",
            Reason::NoBone => "no_bone",
            Reason::Read => "read",
            Reason::Vtable => "vtable",
            Reason::Range => "range",
            Reason::Nan => "nan",
            Reason::Ortho => "ortho",
            Reason::Space => "space",
            Reason::ChrFar => "chr_far",
            Reason::Det => "det",
            Reason::Jump => "jump",
            Reason::NoWeapon => "no_weapon",
            Reason::WeaponRead => "weapon_read",
            Reason::WeaponVtable => "weapon_vtable",
            Reason::WeaponBad => "weapon_bad",
            Reason::WeaponFar => "weapon_far",
            Reason::WeaponDet => "weapon_det",
            Reason::WeaponStale => "weapon_stale",
        }
    }
}

/// Which space the bone array was in this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Local,
    World,
}

impl Space {
    pub fn label(self) -> &'static str {
        match self {
            Space::Local => "local",
            Space::World => "world",
        }
    }
}

// ---- matrix conversions --------------------------------------------------------------------------------------------

/// A raw Mtx43 entry as a column-form 3x4 (the layout is already `[R row | T]` x 3: a copy).
pub fn entry_to_col(e: &[f32; 12]) -> Mat34 {
    *e
}

/// The row-major, row-vector chr 4x4 (translation in row 3) as a column-form 3x4: `C = M^T`.
pub fn chr_to_col(m: &[f32; 16]) -> Mat34 {
    let mut o = [0.0f32; 12];
    for r in 0..3 {
        for c in 0..3 {
            o[r * 4 + c] = m[c * 4 + r];
        }
        o[r * 4 + 3] = m[12 + r];
    }
    o
}

fn col(m: &Mat34, j: usize) -> V3 {
    V3::new(m[j], m[4 + j], m[8 + j])
}

fn set_col(m: &mut Mat34, j: usize, v: V3) {
    m[j] = v.x;
    m[4 + j] = v.y;
    m[8 + j] = v.z;
}

pub fn translation(m: &Mat34) -> V3 {
    V3::new(m[3], m[7], m[11])
}

/// det of the 3x3 part.
pub fn det3(m: &Mat34) -> f32 {
    col(m, 0).dot(col(m, 1).cross(col(m, 2)))
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// The three rows of the entry's 3x3 are a rotation times a UNIFORM scale: equal lengths (pairwise within
/// `ORTHO_TOL * mean`), the mean inside [`SCALE_MIN`, `SCALE_MAX`] (ER's body-proportion sliders / havok scale
/// inheritance can scale the hand bone), and the normalised rows mutually orthogonal within [`ORTHO_TOL`]. The scale is
/// removed later by `orthonormalize` (recorded in `HandOk.scale`), so the gun keeps its own size.
pub fn rows_orthonormal(e: &[f32; 12]) -> bool {
    let r = [V3::new(e[0], e[1], e[2]), V3::new(e[4], e[5], e[6]), V3::new(e[8], e[9], e[10])];
    let l = r.map(|v| v.len());
    let mean = (l[0] + l[1] + l[2]) / 3.0;
    if !mean.is_finite() || !(SCALE_MIN..=SCALE_MAX).contains(&mean) {
        return false;
    }
    let tol = ORTHO_TOL * mean;
    if (l[0] - l[1]).abs() > tol || (l[0] - l[2]).abs() > tol || (l[1] - l[2]).abs() > tol {
        return false;
    }
    let n = [r[0] * (1.0 / l[0]), r[1] * (1.0 / l[1]), r[2] * (1.0 / l[2])];
    n[0].dot(n[1]).abs() < ORTHO_TOL && n[0].dot(n[2]).abs() < ORTHO_TOL && n[1].dot(n[2]).abs() < ORTHO_TOL
}

/// Gram-Schmidt of the 3x3 columns keeping the handedness of the input (column 2 = +-c0 x c1). Returns the matrix and
/// the mean column length (the scale it carried).
pub fn orthonormalize(m: &Mat34) -> Option<(Mat34, f32)> {
    let (a, b, c) = (col(m, 0), col(m, 1), col(m, 2));
    let scale = (a.len() + b.len() + c.len()) / 3.0;
    let x = a.normalize()?;
    let y = (b - x * b.dot(x)).normalize()?;
    let sign = if det3(m) < 0.0 { -1.0 } else { 1.0 };
    let z = x.cross(y) * sign;
    let mut o = *m;
    set_col(&mut o, 0, x);
    set_col(&mut o, 1, y);
    set_col(&mut o, 2, z);
    (scale.is_finite() && scale > 1e-6).then_some((o, scale))
}

// ---- hand -> world ------------------------------------------------------------------------------------------------

/// One player's raw read (docs 4.1) plus the checks the DLL already made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandIn {
    pub vt43_ok: bool,
    pub vt44_ok: bool,
    pub n: u32,
    pub bone: u32,
    pub e: [f32; 12],
    pub m: [f32; 16],
    /// The character's physics position (feet, the same frame as the draw).
    pub phys: V3,
}

/// A validated hand frame (rotation orthonormal, unit scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandOk {
    pub h: Mat34,
    pub space: Space,
    /// The mean column length of `C * E` before normalisation (I10: ~1 expected).
    pub scale: f32,
    /// det of the normalised-by-scale 3x3 before Gram-Schmidt (~+1).
    pub det: f32,
}

/// `hand -> world` from the entry and the chr matrix: world `E` when the entry already sits near the feet, else local
/// `C * E` (docs 4.3). C6 attach 2 [L 2026-10-21 bone probe, 5 frames]: ER's chr bone array is WORLD space (bone 0 ==
/// the chr matrix translation exactly), so world is tried first; the first build tried local first and would have drawn
/// a player standing within 2.5 m of the tile origin at twice its position.
pub fn hand_world(e: &[f32; 12], m: &[f32; 16], phys: V3) -> Result<(Mat34, Space), Reason> {
    let ec = entry_to_col(e);
    let te = translation(&ec);
    let c = chr_to_col(m);
    let tc = translation(&c);
    if (te - phys).len() < HAND_MAX_M {
        return Ok((ec, Space::World));
    }
    if te.len() < HAND_MAX_M {
        if (tc - phys).len() >= CHR_MAX_M {
            return Err(Reason::ChrFar);
        }
        return Ok((m_mul(&c, &ec), Space::Local));
    }
    Err(Reason::Space)
}

/// The whole per-frame check of docs 5 (`exe`, `no_bone`, `read` are decided by the caller before it has an input).
pub fn validate_hand(i: &HandIn) -> Result<HandOk, Reason> {
    if !i.vt43_ok || !i.vt44_ok {
        return Err(Reason::Vtable);
    }
    if !(N_MIN..=N_MAX).contains(&i.n) || i.bone >= i.n {
        return Err(Reason::Range);
    }
    if !finite(&i.e) || !finite(&i.m) || !i.phys.is_finite() {
        return Err(Reason::Nan);
    }
    if !rows_orthonormal(&i.e) {
        return Err(Reason::Ortho);
    }
    let (h, space) = hand_world(&i.e, &i.m, i.phys)?;
    if !finite(&h) {
        return Err(Reason::Nan);
    }
    let (hn, scale) = orthonormalize(&h).ok_or(Reason::Det)?;
    let det = det3(&h) / (scale * scale * scale);
    if !det.is_finite() || (det - 1.0).abs() > DET_TOL {
        return Err(Reason::Det);
    }
    Ok(HandOk { h: hn, space, scale, det })
}

// ---- the grip -----------------------------------------------------------------------------------------------------

/// The signed permutation `hand_axes` ("+x,+z,+y": MW2 X -> +x, MW2 Y -> +z, MW2 Z -> +y) as the columns of a 3x3 in a
/// Mat34 (no translation). Its det must be -1 (MW2 right-handed -> ER left-handed, as stage A), so the renderer's culling
/// winding is unchanged.
pub fn parse_axes(s: &str) -> Result<Mat34, String> {
    let toks: Vec<&str> = s.split(',').map(str::trim).collect();
    if toks.len() != 3 {
        return Err(format!("hand_axes {s:?} must be three tokens like \"+x,+z,+y\""));
    }
    let mut m = [0.0f32; 12];
    let mut used = [false; 3];
    for (j, t) in toks.iter().enumerate() {
        let b = t.as_bytes();
        if b.len() != 2 {
            return Err(format!("hand_axes token {t:?} must be a sign and an axis (+x, -z, ...)"));
        }
        let sign = match b[0] {
            b'+' => 1.0,
            b'-' => -1.0,
            _ => return Err(format!("hand_axes token {t:?} must start with + or -")),
        };
        let r = match b[1].to_ascii_lowercase() {
            b'x' => 0,
            b'y' => 1,
            b'z' => 2,
            _ => return Err(format!("hand_axes token {t:?} must name x, y or z")),
        };
        if used[r] {
            return Err(format!("hand_axes {s:?} uses an axis twice"));
        }
        used[r] = true;
        m[r * 4 + j] = sign;
    }
    let d = det3(&m);
    if d > 0.0 {
        return Err(format!("hand_axes {s:?} has det +1: it must mirror like stage A (det -1), else the gun is drawn inside out"));
    }
    Ok(m)
}

/// `R = Rz * Ry * Rx` (X applied first, then Y, then Z, in hand-local axes), angles in degrees.
pub fn rot_xyz_deg(d: [f32; 3]) -> Mat34 {
    let (sx, cx) = d[0].to_radians().sin_cos();
    let (sy, cy) = d[1].to_radians().sin_cos();
    let (sz, cz) = d[2].to_radians().sin_cos();
    let rx: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, cx, -sx, 0.0, 0.0, sx, cx, 0.0];
    let ry: Mat34 = [cy, 0.0, sy, 0.0, 0.0, 1.0, 0.0, 0.0, -sy, 0.0, cy, 0.0];
    let rz: Mat34 = [cz, -sz, 0.0, 0.0, sz, cz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    m_mul(&rz, &m_mul(&ry, &rx))
}

/// `G (gun -> hand) = T(offset_m) * R_xyz(rot_deg) * A(axes)`.
pub fn grip_matrix(offset_m: [f32; 3], rot_deg: [f32; 3], axes: &Mat34) -> Mat34 {
    let mut r = rot_xyz_deg(rot_deg);
    r[3] = offset_m[0];
    r[7] = offset_m[1];
    r[11] = offset_m[2];
    m_mul(&r, axes)
}

/// `W_gun = H * G * S(IN_TO_M)`: MW2 model inches -> ER world metres (the 3x4 `WorldGunDraw.model` takes).
pub fn gun_from_hand(h: &Mat34, g: &Mat34) -> Mat34 {
    let mut w = m_mul(h, g);
    for r in 0..3 {
        for c in 0..3 {
            w[r * 4 + c] *= IN_TO_M;
        }
    }
    w
}

// ---- per player: jump guard + smoothing ---------------------------------------------------------------------------

/// One player's last good frame: the 1-frame hold of the jump guard and the optional anchor smoothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandTrack {
    last: Option<Mat34>,
    held: bool,
    last_t: f64,
}

/// A last good frame older than this (the player was not drawn) is forgotten: no jump guard against a stale anchor.
pub const TRACK_STALE_S: f64 = 0.5;

impl HandTrack {
    pub fn reset(&mut self) {
        *self = HandTrack::default();
    }

    /// A validated `W_gun` (or the reason it failed) -> what to draw from the hand path. `smooth` 0..0.9 lerps the
    /// anchor toward the last good one (0 = none). A jump > [`JUMP_M`] draws the last good matrix for one frame; a second
    /// jumping frame falls back (`jump`) and forgets the old anchor, so the next frame starts fresh.
    pub fn step(&mut self, now: f64, r: Result<Mat34, Reason>, smooth: f32) -> Result<Mat34, Reason> {
        if self.last.is_some() && !(now - self.last_t <= TRACK_STALE_S) {
            self.reset();
        }
        let mut w = r?;
        if let Some(last) = self.last {
            let d = (translation(&w) - translation(&last)).len();
            if !(d <= JUMP_M) {
                if !self.held {
                    self.held = true;
                    return Ok(last);
                }
                self.reset();
                return Err(Reason::Jump);
            }
            let k = smooth.clamp(0.0, 0.9);
            if k > 0.0 {
                for i in [3, 7, 11] {
                    w[i] += (last[i] - w[i]) * k;
                }
            }
        }
        self.held = false;
        self.last = Some(w);
        self.last_t = now;
        Ok(w)
    }
}

// ---- the fallback log (docs 5) ------------------------------------------------------------------------------------

/// What one player's frame wants logged.
#[derive(Clone, Debug, PartialEq)]
pub enum FbEvent {
    /// First frame of a new fallback reason.
    Reason(Reason),
    /// First hand frame (at start / after fallbacks).
    HandOk,
    /// Every [`FALLBACK_SUMMARY_S`] while any fallback happened: reasons with counts, bad / total frames.
    Summary { reasons: Vec<(Reason, u32)>, bad: u32, total: u32 },
}

/// Rate-limited fallback accounting for one player (no log per frame).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FallbackLog {
    cur: Option<Reason>,
    ok: bool,
    t0: Option<f64>,
    counts: Vec<(Reason, u32)>,
    bad: u32,
    total: u32,
}

impl FallbackLog {
    pub fn frame(&mut self, now: f64, r: Result<(), Reason>, out: &mut Vec<FbEvent>) {
        let t0 = *self.t0.get_or_insert(now);
        self.total += 1;
        match r {
            Ok(()) => {
                if !self.ok {
                    self.ok = true;
                    out.push(FbEvent::HandOk);
                }
                self.cur = None;
            }
            Err(x) => {
                self.ok = false;
                self.bad += 1;
                match self.counts.iter_mut().find(|(k, _)| *k == x) {
                    Some(c) => c.1 += 1,
                    None => self.counts.push((x, 1)),
                }
                if self.cur != Some(x) {
                    self.cur = Some(x);
                    out.push(FbEvent::Reason(x));
                }
            }
        }
        if now - t0 >= FALLBACK_SUMMARY_S {
            if self.bad > 0 {
                let mut reasons = std::mem::take(&mut self.counts);
                reasons.sort();
                out.push(FbEvent::Summary { reasons, bad: self.bad, total: self.total });
            }
            self.counts.clear();
            self.bad = 0;
            self.total = 0;
            self.t0 = Some(now);
        }
    }
}

/// `reasons=space:12,det:1` for the summary line.
pub fn reasons_text(r: &[(Reason, u32)]) -> String {
    r.iter().map(|(k, n)| format!("{}:{n}", k.label())).collect::<Vec<_>>().join(",")
}

// ---- the name lookup (docs 4.2 step 2) -----------------------------------------------------------------------------

/// The FLVER entry whose translation is within [`NAME_MATCH_M`] of the havok bone's model-pose translation (the nearest
/// one; `None` when none is that close). Returns (index, error m).
pub fn match_flver(t: V3, entries: &[[f32; 12]]) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, e) in entries.iter().enumerate() {
        let d = (V3::new(e[3], e[7], e[11]) - t).len();
        if d.is_finite() && d <= NAME_MATCH_M && best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    best
}

/// The 34 MW2 MP gun names (convert_arsenal.GUN_INDEX order, the stable g of `coop_vis::GUN_G`): what `[coop.grip.<gun>]`
/// accepts when `arsenal.guns` is empty (index.json order).
pub const MW2_GUNS: [&str; 34] = [
    "m4_mp", "famas_mp", "scar_mp", "tavor_mp", "fal_mp", "m16_mp", "masada_mp", "fn2000_mp", "ak47_mp", "mp5k_mp", "ump45_mp", "kriss_mp", "p90_mp",
    "uzi_mp", "sa80_mp", "rpd_mp", "mg4_mp", "aug_mp", "m240_mp", "cheytac_mp", "barrett_mp", "wa2000_mp", "m21_mp", "spas12_mp", "aa12_mp",
    "striker_mp", "ranger_mp", "m1014_mp", "model1887_mp", "at4_mp", "m79_mp", "rpg_mp", "stinger_mp", "javelin_mp",
];

/// [`match_flver`] on the entries as read (model space, I5) and, when that finds nothing, on the entries brought back from
/// world space through the inverse of the chr matrix (a re-based array). Returns (index, error m, "local" | "world").
pub fn match_flver_any(t: V3, entries: &[[f32; 12]], chr: Option<&[f32; 16]>) -> Option<(usize, f32, &'static str)> {
    if let Some((i, e)) = match_flver(t, entries) {
        return Some((i, e, "local"));
    }
    let inv = crate::xform::m_inv_rigid(&orthonormalize(&chr_to_col(chr?))?.0);
    let local: Vec<[f32; 12]> = entries
        .iter()
        .map(|e| {
            let p = crate::xform::m_point(&inv, &[e[3], e[7], e[11]]);
            let mut o = *e;
            (o[3], o[7], o[11]) = (p[0], p[1], p[2]);
            o
        })
        .collect();
    match_flver(t, &local).map(|(i, e)| (i, e, "world"))
}

/// C6 `COOP bone probe bones=a-b` chunk: `i:Tx,Ty,Tz|l0,l1,l2` (the entry's translation and its three row lengths, 2
/// decimals) for the bones `from..to`.
pub fn probe_chunk(entries: &[[f32; 12]], from: usize, to: usize) -> String {
    let mut v = Vec::new();
    for (i, e) in entries.iter().enumerate().take(to.min(entries.len())).skip(from) {
        let l = |k: usize| V3::new(e[k * 4], e[k * 4 + 1], e[k * 4 + 2]).len();
        v.push(format!("{i}:{:.2},{:.2},{:.2}|{:.2},{:.2},{:.2}", e[3], e[7], e[11], l(0), l(1), l(2)));
    }
    v.join(" ")
}

/// `[x,y,z]` with 3 decimals for the HOLD lines.
pub fn v3_text(v: V3) -> String {
    format!("[{:.3},{:.3},{:.3}]", v.x, v.y, v.z)
}

/// The bone name `hand_bone_auto` looks for first, then the fallback.
pub const HAND_BONE_NAMES: [&str; 2] = ["R_Weapon", "R_Hand"];

// ---- C6 attach 2 (work/c6/attach2.md): ER's own right-hand weapon model -------------------------------------------

/// The right hand's FLVER index of the c0000 bone array [L 2026-10-21 probe: the second arm chain, 0.23 m from the
/// elbow]; used when `hand_bone = -1` and the name lookup finds nothing (the same FLVER on every PC).
pub const DEFAULT_HAND_BONE: u32 = 57;
/// ER's drawn weapon must sit this close to the hand bone (a stale / wrong matrix is elsewhere) [attach2.md 2].
pub const WEAPON_NEAR_M: f32 = 0.35;
/// A weapon FLVER bone array has at most this many entries (a sword: 1..few).
pub const WEAPON_N_MAX: u32 = 64;
/// C6 grip 3 (work/c6/grip3.md 1): ER's weapon frame is a rig constant (`e0 == R_Weapon x Rz(180)` in all 21 probe
/// frames [L 17:32]): weapon -Y = the character's facing (the muzzle), +X = the hilt axis through the fist (up), +Z = the
/// character's left. So MW2 X (forward) -> -y, MW2 Y (left) -> +z, MW2 Z (up) -> +x (det -1). The first build's guess
/// "+y,+x,+z" put the barrel on +Y = backward and 8-27 deg down (the round-2 screenshot). It is the shipped
/// `weapon_axes` and the only map `"auto"` may accept ([`axes_auto_accept`]).
pub const WEAPON_AXES_RIG: &str = "-y,+z,+x";
/// C6 grip 3 (grip3.md 4): the grip rake about weapon Z that cancels the measured +18.8 deg muzzle-up of ER's hold (the
/// pistol grip then lies along the hilt axis, the bore ~level); the `[coop] weapon_rake_deg` default.
pub const WEAPON_RAKE_DEG: f32 = -18.0;
/// C6 grip 3 (grip3.md 2): the MW2 world model's right-palm / pistol-grip point G (MW2 world-model inches: X forward,
/// Y left, Z up, origin = the world model's tag_weapon) of every gun, from the world-vs-viewmodel tag fit
/// (`work/c6/grip3/final.json`). The origin is on the handguard / mag-well front: G is ~11 in behind and ~4.4 in below it.
/// Launchers: at4 / rpg / stinger are 1-tag (tag_flash) fits [I]; javelin's fit put the palm off the gun, so it takes
/// [`GRIP_IN_DEFAULT`] [I]. Tune any gun with `[coop.grip.<gun>]`.
pub const GRIP_IN: [(&str, [f32; 3]); 34] = [
    ("m4_mp", [-10.72, 0.0, -4.12]),
    ("famas_mp", [-9.82, 0.0, -4.14]),
    ("scar_mp", [-12.28, 0.0, -4.27]),
    ("tavor_mp", [-9.58, 0.0, -3.94]),
    ("fal_mp", [-10.42, 0.0, -5.03]),
    ("m16_mp", [-10.85, 0.0, -4.13]),
    ("masada_mp", [-11.61, 0.0, -4.39]),
    ("fn2000_mp", [-10.88, 0.0, -4.35]),
    ("ak47_mp", [-10.83, 0.0, -4.42]),
    ("mp5k_mp", [-12.17, 0.0, -3.78]),
    ("ump45_mp", [-11.82, 0.0, -4.50]),
    ("kriss_mp", [-11.52, 0.0, -1.84]),
    ("p90_mp", [-10.62, 0.0, -4.30]),
    ("uzi_mp", [-9.87, 0.0, -4.40]),
    ("sa80_mp", [-7.79, 0.0, -4.30]),
    ("rpd_mp", [-8.39, 0.0, -4.28]),
    ("mg4_mp", [-9.33, 0.0, -4.66]),
    ("aug_mp", [-9.71, 0.0, -4.50]),
    ("m240_mp", [-11.76, 0.0, -5.52]),
    ("cheytac_mp", [-10.90, 0.0, -4.90]),
    ("barrett_mp", [-13.28, 0.0, -5.40]),
    ("wa2000_mp", [-9.48, 0.0, -5.10]),
    ("m21_mp", [-14.29, 0.0, -4.97]),
    ("spas12_mp", [-11.56, 0.0, -5.03]),
    ("aa12_mp", [-11.14, 0.0, -5.13]),
    ("striker_mp", [-11.36, 0.0, -3.70]),
    ("ranger_mp", [-11.26, 0.0, -2.72]),
    ("m1014_mp", [-12.04, 0.0, -5.70]),
    ("model1887_mp", [-11.57, 0.0, -4.42]),
    ("at4_mp", [-17.30, 0.0, 0.41]),
    ("m79_mp", [-11.81, 0.0, -2.01]),
    ("rpg_mp", [-27.92, 0.0, -2.70]),
    ("stinger_mp", [-10.98, 0.0, -4.03]),
    ("javelin_mp", GRIP_IN_DEFAULT),
];
/// The median grip point of the 29 small arms (grip3.md 2): a gun missing from [`GRIP_IN`] (and javelin) uses it.
pub const GRIP_IN_DEFAULT: [f32; 3] = [-11.0, 0.0, -4.4];

/// The grip point G (MW2 inches) of `gun` (full "m4_mp" or short "m4" name): its [`GRIP_IN`] row, else the default.
pub fn grip_point_in(gun: &str) -> [f32; 3] {
    let full = if gun.ends_with("_mp") { gun.to_string() } else { format!("{gun}_mp") };
    GRIP_IN.iter().find(|(n, _)| *n == full).map_or(GRIP_IN_DEFAULT, |(_, g)| *g)
}

/// C6 grip 3 (grip3.md 4): the weapon-mode grip that puts the MW2 grip point `g_in` exactly on ER's weapon origin (the
/// fist) for the axis map `axes` and the rake `rake_deg` about weapon Z: `rot_deg = [0, 0, rake]`,
/// `offset_m = -Rz(rake) * A * G * IN_TO_M` (so `T(o) * Rz * A * S` maps G to 0). Default gun, rig map, -18 deg:
/// `[0.020, -0.300, 0.0]`.
pub fn weapon_grip(g_in: [f32; 3], axes: &Mat34, rake_deg: f32) -> ([f32; 3], [f32; 3]) {
    let rot = [0.0, 0.0, rake_deg];
    (weapon_grip_offset(g_in, axes, rot), rot)
}

/// C6 part 17 addendum: the weapon-frame offset that puts the MW2 grip point `g_in` on ER's weapon origin (the fist) for
/// the map `axes` turned by `rot_deg` (`offset_m = -R_xyz(rot) * A * G * IN_TO_M`; [`weapon_grip`] with any rotation).
pub fn weapon_grip_offset(g_in: [f32; 3], axes: &Mat34, rot_deg: [f32; 3]) -> [f32; 3] {
    let ra = m_mul(&rot_xyz_deg(rot_deg), axes);
    let p = crate::xform::m_point(&ra, &g_in);
    [-p[0] * IN_TO_M, -p[1] * IN_TO_M, -p[2] * IN_TO_M]
}

/// `weapon_axes = "auto"` (now a diagnostic): the vote's map is used only when it is the rig map; anything else (a
/// one-hand sword pose, a mid-chord blend) is rejected and the rig map stays (grip3.md 3: the vote alone could learn a
/// backwards map).
pub fn axes_auto_accept(learned: &str) -> bool {
    matches!((parse_axes(learned), parse_axes(WEAPON_AXES_RIG)), (Ok(a), Ok(b)) if a == b)
}
/// The weapon pose bit-identical for this many frames while the hand moved more than STALE_HAND_MOVE_M: the hidden
/// model stopped updating (`weapon_stale`; the hand bone with the learned grip takes over).
pub const STALE_FRAMES: u32 = 3;
pub const STALE_HAND_MOVE_M: f32 = 0.02;
/// `weapon_axes = "auto"`: a frame votes when one weapon axis is within acos(0.75) of the character's facing and another
/// within acos(0.5) of world up; the map is taken after VOTE_N votes with VOTE_SHARE of them for one map.
pub const VOTE_FWD_MIN: f32 = 0.75;
pub const VOTE_UP_MIN: f32 = 0.5;
pub const VOTE_N: u32 = 45;
pub const VOTE_SHARE: f32 = 0.7;

/// Which of the weapon model's two matrices was used (attach2.md 2: which one ER refreshes per frame is [I]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponSrc {
    /// The weapon FLVER bone 0 entry (`[[parts+0x10]+0x640]`, `[R row | T]`), already in world like the chr array.
    E0,
    /// The model's own location 4x4 (`[parts+0x20]+0x70`, row-major like the chr matrix).
    Root,
    /// Bone 0 relative to the location matrix (an array in model space).
    RootE0,
}

impl WeaponSrc {
    pub fn label(self) -> &'static str {
        match self {
            WeaponSrc::E0 => "e0",
            WeaponSrc::Root => "root",
            WeaponSrc::RootE0 => "root_e0",
        }
    }
}

/// One weapon-model read (the DLL checked the four vtables and the owner back-pointers: `vt_ok`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponIn {
    pub vt_ok: bool,
    pub root: Option<[f32; 16]>,
    pub e0: Option<[f32; 12]>,
    pub n: u32,
}

/// ER's weapon model -> world (orthonormal, unit scale) from the first candidate (e0, root, root x e0) that is finite, a
/// rotation times a uniform scale, proper (det +1) and within `max_m` of `near` (world: the hand bone with
/// [`WEAPON_NEAR_M`]; the caller passes the feet and a wider radius when the hand bone did not read).
/// Returns the matrix, its source and the distance to the hand.
pub fn weapon_world(w: &WeaponIn, near: V3, max_m: f32) -> Result<(Mat34, WeaponSrc, f32), Reason> {
    if !w.vt_ok {
        return Err(Reason::WeaponVtable);
    }
    let e0 = w.e0.filter(|_| (1..=WEAPON_N_MAX).contains(&w.n)).map(|e| entry_to_col(&e));
    let root = w.root.map(|m| chr_to_col(&m));
    let mut cands: Vec<(Mat34, WeaponSrc)> = Vec::with_capacity(3);
    if let Some(e) = e0 {
        cands.push((e, WeaponSrc::E0));
    }
    if let Some(r) = root {
        cands.push((r, WeaponSrc::Root));
        if let Some(e) = e0 {
            cands.push((m_mul(&r, &e), WeaponSrc::RootE0));
        }
    }
    if cands.is_empty() {
        return Err(Reason::WeaponRead);
    }
    let mut why = Reason::WeaponBad;
    for (m, src) in cands {
        if !finite(&m) || !rows_orthonormal(&m) {
            continue;
        }
        let d = (translation(&m) - near).len();
        if !(d < max_m) {
            why = Reason::WeaponFar;
            continue;
        }
        let Some((n, scale)) = orthonormalize(&m) else { continue };
        let det = det3(&m) / (scale * scale * scale);
        if !det.is_finite() || (det - 1.0).abs() > DET_TOL {
            if why != Reason::WeaponFar {
                why = Reason::WeaponDet;
            }
            continue;
        }
        return Ok((n, src, d));
    }
    Err(why)
}

/// The weapon model stopped updating (the hide may freeze it [I]): its pose bit-identical for [`STALE_FRAMES`] frames
/// while the hand bone moved more than [`STALE_HAND_MOVE_M`] since the pose last changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StaleGuard {
    last: Option<Mat34>,
    hand_at_change: Option<V3>,
    n: u32,
    changed: bool,
}

impl StaleGuard {
    pub fn reset(&mut self) {
        *self = StaleGuard::default();
    }

    /// true = stale (do not use this pose).
    pub fn step(&mut self, w: &Mat34, hand: V3) -> bool {
        let same = self.last.is_some_and(|l| l.iter().zip(w).all(|(a, b)| a.to_bits() == b.to_bits()));
        self.changed = !same;
        if !same {
            self.n = 0;
            self.hand_at_change = Some(hand);
        } else if self.hand_at_change.is_some_and(|h| (h - hand).len() > STALE_HAND_MOVE_M) {
            self.n += 1;
        }
        self.last = Some(*w);
        self.n >= STALE_FRAMES
    }

    /// The pose of the last [`step`](Self::step) differed from the one before (the first pose counts as changed). Only
    /// such a pose may teach the grip: a repeated pose may already be frozen while the hand moves under the stale
    /// threshold (C6 review), and `inv(H_moved) * W_frozen` is not the real hand -> weapon offset.
    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// The hand -> weapon grip learned while both read (`inv(H) * W`, both orthonormal): the gun keeps ER's weapon pose on
/// the hand bone when the weapon model is missing / stale / far for a frame.
pub fn learn_grip(h: &Mat34, w: &Mat34) -> Mat34 {
    m_mul(&crate::xform::m_inv_rigid(h), w)
}

/// The six signed axes, in vote-index order.
pub const AXIS_TOKENS: [&str; 6] = ["+x", "-x", "+y", "-y", "+z", "-z"];

fn signed_axis(m: &Mat34, k: usize) -> V3 {
    let c = col(m, k / 2);
    if k % 2 == 0 { c } else { c * -1.0 }
}

/// The `weapon_axes` text whose MW2 X is signed axis `fwd_k`, MW2 Z is `up_k`, and MW2 Y the remaining axis with the sign
/// that mirrors (det -1, as every map here). `None` when both name the same axis.
pub fn axes_text(fwd_k: usize, up_k: usize) -> Option<String> {
    if fwd_k >= 6 || up_k >= 6 || fwd_k / 2 == up_k / 2 {
        return None;
    }
    let rest = 3 - fwd_k / 2 - up_k / 2;
    ["+", "-"].iter().map(|s| format!("{},{}{},{}", AXIS_TOKENS[fwd_k], s, ["x", "y", "z"][rest], AXIS_TOKENS[up_k])).find(|t| parse_axes(t).is_ok())
}

/// `weapon_axes = "auto"`: votes for the map MW2 forward -> the weapon axis along the character's facing, MW2 up -> the
/// axis nearest world up, counted only while the crossbow stance holds the weapon (the DLL decides when).
#[derive(Clone, Debug, PartialEq)]
pub struct AxesVote {
    counts: [u32; 36],
    total: u32,
}

impl Default for AxesVote {
    fn default() -> Self {
        AxesVote { counts: [0; 36], total: 0 }
    }
}

impl AxesVote {
    /// One frame of the weapon's world rotation `w` (orthonormal), the character's horizontal facing and world up.
    /// Returns true when the frame voted.
    pub fn observe(&mut self, w: &Mat34, fwd: V3, up: V3) -> bool {
        let (Some(f), Some(u)) = (fwd.normalize(), up.normalize()) else { return false };
        let best = |skip: Option<usize>, dir: V3| {
            (0..6).filter(|k| skip.is_none_or(|s| k / 2 != s / 2)).map(|k| (k, signed_axis(w, k).dot(dir))).fold((0, f32::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a })
        };
        let (fk, fd) = best(None, f);
        if !(fd >= VOTE_FWD_MIN) {
            return false;
        }
        let (uk, ud) = best(Some(fk), u);
        if !(ud >= VOTE_UP_MIN) {
            return false;
        }
        self.counts[fk * 6 + uk] += 1;
        self.total += 1;
        true
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    /// The winning map once [`VOTE_N`] frames voted and one map holds [`VOTE_SHARE`] of them: (axes text, its votes).
    pub fn result(&self) -> Option<(String, u32)> {
        if self.total < VOTE_N {
            return None;
        }
        let (i, &n) = self.counts.iter().enumerate().max_by_key(|(_, n)| **n)?;
        if (n as f32) < VOTE_SHARE * self.total as f32 {
            return None;
        }
        axes_text(i / 6, i % 6).map(|t| (t, n))
    }
}

/// Floats as `[a,b,...]` with 3 decimals for the `HOLD weapon probe` lines.
pub fn floats_text(v: &[f32]) -> String {
    format!("[{}]", v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(","))
}

// ---- third-person ADS (docs 6) -------------------------------------------------------------------------------------

/// Orthonormalised blend of two gun matrices (columns lerped then re-orthonormalised with `a`'s handedness and the
/// lerped scale; translation lerped). `u` 0 = `a`, 1 = `b`.
pub fn blend_mat34(a: &Mat34, b: &Mat34, u: f32) -> Mat34 {
    let u = u.clamp(0.0, 1.0);
    let mut m = [0.0f32; 12];
    for i in 0..12 {
        m[i] = a[i] + (b[i] - a[i]) * u;
    }
    if u <= 0.0 {
        return *a;
    }
    if u >= 1.0 {
        return *b;
    }
    let sa = (col(a, 0).len() + col(a, 1).len() + col(a, 2).len()) / 3.0;
    let sb = (col(b, 0).len() + col(b, 1).len() + col(b, 2).len()) / 3.0;
    let s = sa + (sb - sa) * u;
    let sign = if det3(a) < 0.0 { -1.0 } else { 1.0 };
    let (Some(x), Some(y0)) = (col(&m, 0).normalize(), col(&m, 1).normalize()) else { return if u < 0.5 { *a } else { *b } };
    let Some(y) = (y0 - x * y0.dot(x)).normalize() else { return if u < 0.5 { *a } else { *b } };
    let z = x.cross(y) * sign;
    set_col(&mut m, 0, x * s);
    set_col(&mut m, 1, y * s);
    set_col(&mut m, 2, z * s);
    m
}

/// The ADS blend factor moving toward `on` over [`ADS_BLEND_S`].
pub fn ads_blend_step(u: f32, on: bool, dt: f32) -> f32 {
    let step = if ADS_BLEND_S > 0.0 { dt.max(0.0) / ADS_BLEND_S } else { 1.0 };
    if on { (u + step).min(1.0) } else { (u - step).max(0.0) }
}

// ---- C6 aim 4 (work/c6/aim4.md 3.3): the gun in ER's crossbow aim ------------------------------------------------

/// The aim-align limit: a larger correction means the pose is not the aim (drawn on ER's weapon frame as is).
pub const AIM_ALIGN_MAX_DEG: f32 = 20.0;
/// The aim point: this far along the camera ray.
pub const AIM_ALIGN_DIST_M: f32 = 30.0;

/// A rotation of `rad` about the unit `axis` (Rodrigues), as a Mat34 without translation.
pub fn rot_axis_angle(axis: V3, rad: f32) -> Mat34 {
    let (s, c) = rad.sin_cos();
    let t = 1.0 - c;
    let (x, y, z) = (axis.x, axis.y, axis.z);
    [
        t * x * x + c,
        t * x * y - s * z,
        t * x * z + s * y,
        0.0,
        t * x * y + s * z,
        t * y * y + c,
        t * y * z - s * x,
        0.0,
        t * x * z - s * y,
        t * y * z + s * x,
        t * z * z + c,
        0.0,
    ]
}

/// The gun's barrel (MW2 model +X, the first column of the drawn matrix) in world, unit.
pub fn gun_barrel(m: &Mat34) -> Option<V3> {
    col(m, 0).normalize()
}

/// A point of the MW2 model (inches, e.g. its pistol grip `GRIP_IN`) in world through the drawn matrix.
pub fn gun_point(m: &Mat34, p_in: [f32; 3]) -> V3 {
    V3::new(
        m[0] * p_in[0] + m[1] * p_in[1] + m[2] * p_in[2] + m[3],
        m[4] * p_in[0] + m[5] * p_in[1] + m[6] * p_in[2] + m[7],
        m[8] * p_in[0] + m[9] * p_in[1] + m[10] * p_in[2] + m[11],
    )
}

/// The angle (deg) between the barrel and the line from `pivot` to `target`; None when either is degenerate.
pub fn aim_error_deg(m: &Mat34, pivot: V3, target: V3) -> Option<f32> {
    let f = gun_barrel(m)?;
    let d = (target - pivot).normalize()?;
    Some(f.dot(d).clamp(-1.0, 1.0).acos().to_degrees())
}

/// C6 aim 4: turns the drawn gun about `pivot` (its pistol grip in the fist) by the fraction `u` of the smallest
/// rotation that points its barrel at `target` (the camera's aim point), so the barrel follows the aim line even if ER's
/// aim pose / the -18 deg rake leave it off. `Ok((matrix, full angle deg))`; `Err(full angle deg)` above `max_deg`
/// (the matrix is then drawn as is). Rigid: scale and handedness (det -1) are kept, the pivot does not move.
pub fn aim_align(m: &Mat34, pivot: V3, target: V3, u: f32, max_deg: f32) -> Result<(Mat34, f32), f32> {
    let (Some(f), Some(d)) = (gun_barrel(m), (target - pivot).normalize()) else { return Ok((*m, 0.0)) };
    let ang = f.dot(d).clamp(-1.0, 1.0).acos();
    let deg = ang.to_degrees();
    if deg > max_deg {
        return Err(deg);
    }
    let u = u.clamp(0.0, 1.0);
    let Some(axis) = f.cross(d).normalize() else { return Ok((*m, deg)) };
    if u <= 0.0 || ang < 1e-5 {
        return Ok((*m, deg));
    }
    let r = rot_axis_angle(axis, ang * u);
    let mut o = m_mul(&r, m);
    let t = translation(m) - pivot;
    let rt = V3::new(r[0] * t.x + r[1] * t.y + r[2] * t.z, r[4] * t.x + r[5] * t.y + r[6] * t.z, r[8] * t.x + r[9] * t.y + r[10] * t.z) + pivot;
    o[3] = rt.x;
    o[7] = rt.y;
    o[11] = rt.z;
    Ok((o, deg))
}

/// C6 round 7 (work/c6/aim7.md 2 / 4 D, the round-6 user addendum "the gun stayed in the low carry"): `tp_ads_hold =
/// "aim"` (`hold_aim`) blends the gun to the camera-aimed pose while MW2 ADS: outside ER's aim episode as before (once
/// the align blend is out, `aim_u <= 0`), and inside one when ER's aim pose did not turn the weapon onto the aim line
/// (`no_pose`: [`aim_nopose_step`], set on ER's confirmed aim; kept until the episode and its align blend ended).
pub fn ads_hold_aim(hold_aim: bool, episode: bool, aim_u: f32, no_pose: bool) -> bool {
    hold_aim && (no_pose || (!episode && aim_u <= 0.0))
}

/// C6 round 7 review (aim7.md 2: ER plays no arm pose for our stance): the `no_pose` latch of [`ads_hold_aim`]. With
/// `tp_ads_hold = "aim"` it sets on ER's confirmed aim (`aim_on`) whatever [`aim_align`] answers (the 22:42 log: after
/// ER's body turn the carry sits only 9.7..18.5 deg off the aim line, so the 20 deg clamp is no "ER kept the carry"
/// test; it only fired on the first frame before the turn), and clears once the episode and its align blend ended.
/// C6 part 16 review: never with the real crossbow carrier (`carrier`): ER's genuine crossbow aim raises the arms and
/// the gun stays on ER's weapon frame (only `aim_align`); the fallback is the patched-sword stance's.
pub fn aim_nopose_step(latched: bool, hold_aim: bool, aim_on: bool, episode: bool, aim_u: f32, carrier: bool) -> bool {
    if carrier || (!episode && aim_u <= 0.0) {
        return false;
    }
    latched || (hold_aim && aim_on)
}

/// C6 part 17 (docs 17.5): the `no_pose` input of [`ads_hold_aim`] in a third-person GUARD episode: with `tp_ads_hold =
/// "aim"` the gun is raised along the camera while the guard is held and either not detected within the confirm time
/// (`fallback`) or detected with the gun more than `tp_guard_align_max_deg` off the aim line (`clamped`, latched for the
/// episode by the caller); a guard that shows (and aligns) draws the gun on ER's weapon frame, turned by `aim_align`.
pub fn guard_nopose(hold_aim: bool, active: bool, fallback: bool, clamped: bool) -> bool {
    hold_aim && active && (fallback || clamped)
}

/// C6 part 17 review: after the guard is detected the align is ignored this long (ER's guard raise is still in motion:
/// detection fires 0.1 s after the frame leaves the carry, partway up).
pub const GUARD_ALIGN_SETTLE_S: f64 = 0.25;
/// C6 part 17 review: the align must stay over `tp_guard_align_max_deg` this long to clamp (the gun raise) ...
pub const GUARD_CLAMP_ON_S: f64 = 0.2;
/// ... and back under it this long to unclamp (a block-hit reaction swings the frame for a frame or two).
pub const GUARD_CLAMP_OFF_S: f64 = 0.15;

/// C6 part 17 review (docs 17.5): the `clamped` input of [`guard_nopose`], debounced and un-latching (it replaced a
/// one-frame latch for the whole episode). Ignores the align for [`GUARD_ALIGN_SETTLE_S`] after detection, clamps after
/// [`GUARD_CLAMP_ON_S`] over the limit, unclamps after [`GUARD_CLAMP_OFF_S`] under it. The caller resets it when the
/// guard episode ends.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GuardClamp {
    det_t: Option<f64>,
    over_t: Option<f64>,
    under_t: Option<f64>,
    pub clamped: bool,
}

impl GuardClamp {
    /// One frame: `detected` = the guard pose was detected in this episode; `over` = this frame's align (Some(true) =
    /// over the limit, Some(false) = within, None = not measured). Returns the clamp.
    pub fn step(&mut self, now: f64, detected: bool, over: Option<bool>) -> bool {
        if !detected {
            *self = Self::default();
            return false;
        }
        let det = *self.det_t.get_or_insert(now);
        if now - det < GUARD_ALIGN_SETTLE_S {
            self.over_t = None;
            self.under_t = None;
            return self.clamped;
        }
        match over {
            Some(true) => {
                self.under_t = None;
                if now - *self.over_t.get_or_insert(now) >= GUARD_CLAMP_ON_S {
                    self.clamped = true;
                }
            }
            Some(false) => {
                self.over_t = None;
                if now - *self.under_t.get_or_insert(now) >= GUARD_CLAMP_OFF_S {
                    self.clamped = false;
                }
            }
            None => {}
        }
        self.clamped
    }

    /// The settle time after detection has passed (the align counts: the episode line's align / clamped).
    pub fn settled(&self, now: f64) -> bool {
        self.det_t.is_some_and(|d| now - d >= GUARD_ALIGN_SETTLE_S)
    }
}

/// `HOLD axes`: the three hand axes in world (unit columns of `H`).
pub fn hand_axes_world(h: &Mat34) -> [V3; 3] {
    [col(h, 0), col(h, 1), col(h, 2)]
}

// ---- C6 part 17 addendum (docs 17.12, the guard-pose test 2026-10-22 11:46-11:51): the guard frame, the polearm carry --
//
// [L] the 11:46-11:51 log (`HOLD axes` b73 = e0 x Rz(180), `HOLD weapon probe` e0 / b29 / b57; row 18080000 with the
// polearm set). ER's weapon frame is R_Weapon (b73) x Rz(180) for any weapon model (grip3.md 1, again here), so these are
// constants of ER's polearm ANIMATIONS, not of the hidden weapon. In weapon-frame coordinates (+X, +Y, +Z = e0's axes):
// - the guard (6 detected, yaw-aligned guards; the body's level forward spread 0.3..3.2 deg): forward = [`GUARD_FWD_W`]
//   (~ -Z), up = [`GUARD_UP_W`] (~ +X), the left hand 0.58 m along -Y = the character's LEFT: the guard holds the shaft
//   ACROSS the chest, so the sword-carry grip (barrel along -Y, rake -18) pointed the barrel left / down: 79.9..96.2 deg
//   off the aim line in every detected guard -> the clamp -> the round-7 raise ("the gun hovers over the right shoulder").
//   The guard frame puts the barrel along the body's forward from the right fist (the pistol grip stays on ER's weapon
//   origin = the fist, no translation): 1.5..8.1 deg off the camera's aim line in 5 of the 6 frames (the camera pitch;
//   53 deg with the camera looking 53 deg down), so `aim_align` keeps only the residual.
// - the standing polearm carry (4 probe frames): the left hand at [`POLEARM_LH_W`] (0.54 m along -Y: the shaft runs from
//   the right fist at the hip forward / up / LEFT across the body), up = [`POLEARM_UP_W`], the body's level forward =
//   [`POLEARM_FWD_W`] (2 estimates 13 deg apart [I]). The sword grip's barrel pointed forward 0.77 / up 0.29 / left 0.57
//   (body frame): neither along the shaft nor ahead.

/// The guard (body-yaw aligned, standing): the body's level forward in ER's weapon frame [L 6 frames].
pub const GUARD_FWD_W: [f32; 3] = [-0.091, -0.048, -0.994];
/// The guard: world up in ER's weapon frame [L].
pub const GUARD_UP_W: [f32; 3] = [0.982, 0.162, -0.097];
/// The standing polearm carry: the left hand (FLVER 29) relative to ER's weapon origin, in its frame [L 4 frames, 0.541 m].
pub const POLEARM_LH_W: [f32; 3] = [0.010, -0.534, 0.090];
/// The standing polearm carry: world up in ER's weapon frame [L].
pub const POLEARM_UP_W: [f32; 3] = [0.777, -0.557, -0.293];
/// The standing polearm carry: the body's level forward in ER's weapon frame [L 2 estimates, 13 deg apart: I].
pub const POLEARM_FWD_W: [f32; 3] = [-0.613, -0.610, -0.501];
/// A gun whose muzzle reaches at least this far ahead of its pistol grip lies along the polearm shaft (the left hand at
/// 0.54 m lands on its fore-end / barrel); a shorter one is held ahead from the right fist (`[coop] polearm_long_min_m`).
pub const POLEARM_LONG_MIN_M: f32 = 0.50;
/// The muzzle's reach ahead of the pistol grip G (m) of each gun (`work/c6/grip3/final.json` `muzzle_ahead_G_m`; javelin's
/// fit put the palm off the tube: absent = long).
pub const MUZZLE_AHEAD_M: [(&str, f32); 33] = [
    ("m4_mp", 0.61),
    ("famas_mp", 0.39),
    ("scar_mp", 0.61),
    ("tavor_mp", 0.41),
    ("fal_mp", 0.70),
    ("m16_mp", 0.70),
    ("masada_mp", 0.61),
    ("fn2000_mp", 0.34),
    ("ak47_mp", 0.64),
    ("mp5k_mp", 0.38),
    ("ump45_mp", 0.41),
    ("kriss_mp", 0.39),
    ("p90_mp", 0.22),
    ("uzi_mp", 0.23),
    ("sa80_mp", 0.66),
    ("rpd_mp", 0.72),
    ("mg4_mp", 0.88),
    ("aug_mp", 0.66),
    ("m240_mp", 0.81),
    ("cheytac_mp", 1.09),
    ("barrett_mp", 0.99),
    ("wa2000_mp", 0.67),
    ("m21_mp", 0.85),
    ("spas12_mp", 0.69),
    ("aa12_mp", 0.62),
    ("striker_mp", 0.58),
    ("ranger_mp", 0.45),
    ("m1014_mp", 0.74),
    ("model1887_mp", 0.83),
    ("at4_mp", 0.54),
    ("m79_mp", 0.46),
    ("rpg_mp", 1.01),
    ("stinger_mp", 0.64),
];

fn v3a(a: [f32; 3]) -> V3 {
    V3::new(a[0], a[1], a[2])
}

/// The 3x3 transpose (translation dropped): the inverse of a rotation / signed permutation.
pub fn transpose3(m: &Mat34) -> Mat34 {
    [m[0], m[4], m[8], 0.0, m[1], m[5], m[9], 0.0, m[2], m[6], m[10], 0.0]
}

/// `w`'s weapon-local coordinates of the world direction `v` (`W^T v`, `w` orthonormal).
pub fn to_local(w: &Mat34, v: V3) -> V3 {
    V3::new(col(w, 0).dot(v), col(w, 1).dot(v), col(w, 2).dot(v))
}

/// A gun frame in weapon coordinates (no translation, det -1 like every map here): MW2 X (the barrel) along `fwd`, MW2 Z
/// (the top) the part of `up` normal to it, MW2 Y = X x Z. None when degenerate.
pub fn gun_frame(fwd: V3, up: V3) -> Option<Mat34> {
    let x = fwd.normalize()?;
    let z = (up - x * up.dot(x)).normalize()?;
    let y = x.cross(z);
    let mut m = [0.0f32; 12];
    set_col(&mut m, 0, x);
    set_col(&mut m, 1, y);
    set_col(&mut m, 2, z);
    Some(m)
}

/// The angles of [`rot_xyz_deg`] for a proper rotation `r` (its inverse; at the gimbal lock y = +-90 deg, x = 0).
pub fn rot_xyz_of(r: &Mat34) -> [f32; 3] {
    let sy = (-r[8]).clamp(-1.0, 1.0);
    let y = sy.asin();
    if sy.abs() > 0.999_99 {
        return [0.0, y.to_degrees(), (-r[1]).atan2(r[5]).to_degrees()];
    }
    [r[9].atan2(r[10]).to_degrees(), y.to_degrees(), r[4].atan2(r[0]).to_degrees()]
}

/// `(offset_m, rot_deg)` of the weapon-frame grip whose `grip_matrix(offset, rot, axes)` is `T(offset) * frame`, with the
/// MW2 grip point `g_in` on ER's weapon origin (the right fist): `rot = R_xyz^-1(frame * A^T)`.
pub fn frame_grip(g_in: [f32; 3], frame: &Mat34, axes: &Mat34) -> ([f32; 3], [f32; 3]) {
    let rot = rot_xyz_of(&m_mul(frame, &transpose3(axes)));
    (weapon_grip_offset(g_in, axes, rot), rot)
}

/// The nearest det -1 signed permutation to `frame` and the [`rot_xyz_deg`] angles that complete it (`R_xyz(rot) * A ==
/// frame`): what `HOLD guard frame auto` prints for `[coop] guard_axes` / `guard_rot_deg`. The rig map when no clean
/// permutation is near.
pub fn frame_pin(frame: &Mat34) -> (String, [f32; 3]) {
    let mut toks = Vec::with_capacity(3);
    let mut used = [false; 3];
    for j in 0..3 {
        let c = col(frame, j);
        let v = [c.x, c.y, c.z];
        let r = (0..3).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap_or(0);
        used[r] = true;
        toks.push(format!("{}{}", if v[r] < 0.0 { "-" } else { "+" }, ["x", "y", "z"][r]));
    }
    let text = toks.join(",");
    let (text, a) = match parse_axes(&text) {
        Ok(a) if used.iter().all(|u| *u) => (text, a),
        _ => (WEAPON_AXES_RIG.to_string(), parse_axes(WEAPON_AXES_RIG).unwrap_or(crate::xform::MAT34_IDENTITY)),
    };
    let rot = rot_xyz_of(&m_mul(frame, &transpose3(&a)));
    (text, rot)
}

/// The polearm carry's two holds (`HOLD grip ... src=polearm_long | polearm_short`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolearmKind {
    /// Along the shaft: the pistol grip in the right fist, the barrel toward the left hand (port arms across the body).
    Long,
    /// Too short for the left hand: the pistol grip in the right fist, the barrel level along the body's forward.
    Short,
}

impl PolearmKind {
    pub fn label(self) -> &'static str {
        match self {
            PolearmKind::Long => "polearm_long",
            PolearmKind::Short => "polearm_short",
        }
    }
}

/// The muzzle's reach ahead of the pistol grip of `gun` (full or short name); None = unknown (javelin, a new gun).
pub fn muzzle_ahead_m(gun: &str) -> Option<f32> {
    let full = if gun.ends_with("_mp") { gun.to_string() } else { format!("{gun}_mp") };
    MUZZLE_AHEAD_M.iter().find(|(n, _)| *n == full).map(|(_, m)| *m)
}

/// Long (the muzzle reaches `long_min_m` or more; unknown = long) or short.
pub fn polearm_kind(gun: &str, long_min_m: f32) -> PolearmKind {
    match muzzle_ahead_m(gun) {
        Some(m) if m < long_min_m => PolearmKind::Short,
        _ => PolearmKind::Long,
    }
}

/// The built-in polearm carry frame (weapon coordinates) of a hold.
pub fn polearm_frame(kind: PolearmKind) -> Mat34 {
    let f = match kind {
        PolearmKind::Long => v3a(POLEARM_LH_W),
        PolearmKind::Short => v3a(POLEARM_FWD_W),
    };
    gun_frame(f, v3a(POLEARM_UP_W)).unwrap_or(crate::xform::MAT34_IDENTITY)
}

/// The built-in guard frame (weapon coordinates): the barrel along the body's level forward, the top up.
pub fn guard_frame_builtin() -> Mat34 {
    gun_frame(v3a(GUARD_FWD_W), v3a(GUARD_UP_W)).unwrap_or(crate::xform::MAT34_IDENTITY)
}

/// Where the guard frame of an episode came from (`HOLD guard frame src=..`, the episode line's `corr=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardFrameSrc {
    /// The 2026-10-22 constants (`guard_axes = "auto"` before this session learned the row's guard).
    Builtin,
    /// Learned this session from the detected guards (`guard_axes = "auto"`).
    Auto,
    /// `guard_axes = "<map>"` + `guard_rot_deg`.
    Fixed,
}

impl GuardFrameSrc {
    pub fn label(self) -> &'static str {
        match self {
            GuardFrameSrc::Builtin => "builtin",
            GuardFrameSrc::Auto => "auto",
            GuardFrameSrc::Fixed => "fixed",
        }
    }
}

/// `guard_axes = "auto"`: samples of a detected, settled guard held standing are taken this often ...
pub const GUARD_LEARN_EVERY_S: f64 = 0.1;
/// ... this many make a frame ...
pub const GUARD_LEARN_N: u32 = 20;
/// ... when the mean of the unit forward and of the unit up (weapon coordinates) are each at least this long (a spread
/// under ~14 deg rms); else the samples are dropped and it starts again.
pub const GUARD_LEARN_MIN_R: f32 = 0.97;

/// `guard_axes = "auto"`: learns the guard frame of the current weapon row from the detected guards (the body's level
/// forward and world up in ER's weapon frame, averaged). Session only; a new row starts again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuardLearn {
    row: Option<u32>,
    f: V3,
    u: V3,
    n: u32,
    next_t: f64,
    done: Option<Mat34>,
}

impl GuardLearn {
    /// One frame of a detected, settled guard held standing: ER's weapon pose `w` (orthonormal), the body's facing, world
    /// up. Returns `(frame, samples, spread deg)` once, when the frame is learned.
    pub fn observe(&mut self, now: f64, row: u32, w: &Mat34, facing: V3, up: V3) -> Option<(Mat34, u32, f32)> {
        if self.row != Some(row) {
            *self = GuardLearn { row: Some(row), ..GuardLearn::default() };
        }
        if self.done.is_some() || now < self.next_t {
            return None;
        }
        self.next_t = now + GUARD_LEARN_EVERY_S;
        let f = V3::new(facing.x, 0.0, facing.z).normalize()?;
        let u = up.normalize()?;
        self.f = self.f + to_local(w, f);
        self.u = self.u + to_local(w, u);
        self.n += 1;
        if self.n < GUARD_LEARN_N {
            return None;
        }
        let n = self.n as f32;
        let r = (self.f.len() / n).min(self.u.len() / n);
        if !(r >= GUARD_LEARN_MIN_R) {
            *self = GuardLearn { row: Some(row), next_t: self.next_t, ..GuardLearn::default() };
            return None;
        }
        let frame = gun_frame(self.f, self.u)?;
        self.done = Some(frame);
        Some((frame, self.n, r.clamp(-1.0, 1.0).acos().to_degrees()))
    }

    /// The learned frame of `row` (None before it is learned / for another row).
    pub fn frame(&self, row: u32) -> Option<Mat34> {
        if self.row == Some(row) { self.done } else { None }
    }
}

/// The rotation angle (deg) between two gun frames of the same handedness (`HOLD guard frame auto ... vs_builtin=`).
pub fn frame_diff_deg(a: &Mat34, b: &Mat34) -> f32 {
    let r = m_mul(&transpose3(a), b);
    ((r[0] + r[5] + r[10] - 1.0) * 0.5).clamp(-1.0, 1.0).acos().to_degrees()
}

/// The barrel's angle (deg) to the level `facing` for a gun frame on ER's weapon pose `w` (the `HOLD guard frame` check:
/// ~0 when the frame fits this pose).
pub fn frame_fwd_err_deg(w: &Mat34, frame: &Mat34, facing: V3) -> Option<f32> {
    let b = m_mul(w, frame);
    let f = V3::new(facing.x, 0.0, facing.z).normalize()?;
    Some(col(&b, 0).normalize()?.dot(f).clamp(-1.0, 1.0).acos().to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coop_vis::world_gun_matrix;
    use crate::xform::{m_point, m_trs, q_normalize};

    fn close(a: &[f32], b: &[f32], eps: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
    }

    /// The row-major chr 4x4 (row-vector) of a column-form rigid transform.
    fn chr_of(c: &Mat34) -> [f32; 16] {
        let mut m = [0.0f32; 16];
        for r in 0..3 {
            for k in 0..3 {
                m[k * 4 + r] = c[r * 4 + k];
            }
            m[12 + r] = c[r * 4 + 3];
        }
        m[15] = 1.0;
        m
    }

    fn input(e: Mat34, chr: Mat34, phys: V3) -> HandIn {
        HandIn { vt43_ok: true, vt44_ok: true, n: 150, bone: 40, e, m: chr_of(&chr), phys }
    }

    #[test]
    fn c6_hold_conversions() {
        // a row-major M with p' = p M: rotation 90 deg about Y and translation (10, 2, -3) in row 3
        let m: [f32; 16] = [0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 10.0, 2.0, -3.0, 1.0];
        let c = chr_to_col(&m);
        // row-vector p M for p = (1, 0, 0): row 0 of M = (0, 0, -1) + T
        let p = m_point(&c, &[1.0, 0.0, 0.0]);
        assert!(close(&p, &[10.0, 2.0, -4.0], 1e-6), "{p:?}");
        let p = m_point(&c, &[0.0, 0.0, 1.0]);
        assert!(close(&p, &[11.0, 2.0, -3.0], 1e-6), "{p:?}");
        // the raw entry keeps its layout: T in floats 3 / 7 / 11
        let e: [f32; 12] = [1.0, 0.0, 0.0, 0.3, 0.0, 1.0, 0.0, 1.2, 0.0, 0.0, 1.0, 0.1];
        assert_eq!(entry_to_col(&e), e);
        assert_eq!(translation(&e), V3::new(0.3, 1.2, 0.1));
    }

    #[test]
    fn c6_hold_hand_world_local_and_world() {
        let q = q_normalize([0.0, 0.38, 0.0, 0.92]);
        let feet = V3::new(120.0, 30.0, -45.0);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q);
        let e = m_trs(&[0.3, 1.2, 0.2], &q_normalize([0.1, 0.2, 0.3, 0.9]));
        let (h, s) = hand_world(&e, &chr_of(&chr), feet).unwrap();
        assert_eq!(s, Space::Local);
        assert!(close(&h, &m_mul(&chr, &e), 1e-5));
        // a re-based array: the entry already in world near the feet
        let ew = m_mul(&chr, &e);
        let (h2, s2) = hand_world(&ew, &chr_of(&chr), feet).unwrap();
        assert_eq!(s2, Space::World);
        assert_eq!(h2, ew);
        // neither near the origin nor near the feet
        let mut far = e;
        far[3] = 40.0;
        assert_eq!(hand_world(&far, &chr_of(&chr), feet), Err(Reason::Space));
    }

    #[test]
    fn c6_hold_parse_axes_all_perms() {
        let axes = ["x", "y", "z"];
        let (mut ok, mut bad) = (0, 0);
        for a in 0..3 {
            for b in 0..3 {
                for c in 0..3 {
                    if a == b || b == c || a == c {
                        assert!(parse_axes(&format!("+{},+{},+{}", axes[a], axes[b], axes[c])).is_err(), "duplicate accepted");
                        continue;
                    }
                    for s in 0..8 {
                        let sg = |k: usize| if s >> k & 1 == 1 { "-" } else { "+" };
                        let txt = format!("{}{},{}{},{}{}", sg(0), axes[a], sg(1), axes[b], sg(2), axes[c]);
                        match parse_axes(&txt) {
                            Ok(m) => {
                                ok += 1;
                                assert!((det3(&m) + 1.0).abs() < 1e-6);
                            }
                            Err(_) => bad += 1,
                        }
                    }
                }
            }
        }
        assert_eq!((ok, bad), (24, 24));
        for junk in ["", "+x,+y", "+x,+y,+z,+x", "x,+y,+z", "+w,+y,+z", "+xx,+y,+z", "*x,+z,+y"] {
            assert!(parse_axes(junk).is_err(), "{junk:?} accepted");
        }
        // the default: MW2 X -> +x, Y -> +z, Z -> +y
        let d = parse_axes("+x,+z,+y").unwrap();
        assert!(close(&m_point(&d, &[1.0, 0.0, 0.0]), &[1.0, 0.0, 0.0], 0.0));
        assert!(close(&m_point(&d, &[0.0, 1.0, 0.0]), &[0.0, 0.0, 1.0], 0.0));
        assert!(close(&m_point(&d, &[0.0, 0.0, 1.0]), &[0.0, 1.0, 0.0], 0.0));
    }

    #[test]
    fn c6_hold_gun_from_identity_hand_is_axes_times_scale() {
        let ident: Mat34 = crate::xform::MAT34_IDENTITY;
        let a = parse_axes("+x,+z,+y").unwrap();
        let g = grip_matrix([0.0; 3], [0.0; 3], &a);
        let w = gun_from_hand(&ident, &g);
        let mut want = a;
        for r in 0..3 {
            for c in 0..3 {
                want[r * 4 + c] *= IN_TO_M;
            }
        }
        assert!(close(&w, &want, 1e-7));
        assert!(det3(&w) < 0.0, "det -1 like stage A");
        // the MW2 muzzle 10 in ahead on the hand's +x
        assert!(close(&m_point(&w, &[10.0, 0.0, 0.0]), &[0.254, 0.0, 0.0], 1e-6));
    }

    #[test]
    fn c6_hold_grip_offset_and_rotation_order() {
        let a = parse_axes("+x,+z,+y").unwrap();
        // offset in hand-local metres
        let g = grip_matrix([0.1, -0.02, 0.03], [0.0; 3], &a);
        assert!(close(&m_point(&g, &[0.0; 3]), &[0.1, -0.02, 0.03], 1e-6));
        // X then Y: rotating +y by 90 about X gives +z, then 90 about Y maps +z to +x
        let r = rot_xyz_deg([90.0, 90.0, 0.0]);
        assert!(close(&m_point(&r, &[0.0, 1.0, 0.0]), &[1.0, 0.0, 0.0], 1e-6));
        // the opposite order would give -y.. so the order is pinned: Y first would map +y -> +y, then X -> +z
        let r2 = m_mul(&rot_xyz_deg([90.0, 0.0, 0.0]), &rot_xyz_deg([0.0, 90.0, 0.0]));
        assert!(close(&m_point(&r2, &[0.0, 1.0, 0.0]), &[0.0, 0.0, 1.0], 1e-6));
        // Z last
        let r3 = rot_xyz_deg([0.0, 0.0, 90.0]);
        assert!(close(&m_point(&r3, &[1.0, 0.0, 0.0]), &[0.0, 1.0, 0.0], 1e-6));
    }

    #[test]
    fn c6_hold_validate_rejects_each_reason() {
        let feet = V3::new(50.0, 10.0, 20.0);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q_normalize([0.0, 0.2, 0.0, 0.98]));
        let e = m_trs(&[0.35, 1.25, 0.15], &q_normalize([0.3, -0.1, 0.2, 0.9]));
        let good = input(e, chr, feet);
        let ok = validate_hand(&good).unwrap();
        assert_eq!(ok.space, Space::Local);
        assert!((ok.scale - 1.0).abs() < 1e-4 && (ok.det - 1.0).abs() < 1e-4);
        // vtable / range
        assert_eq!(validate_hand(&HandIn { vt43_ok: false, ..good }), Err(Reason::Vtable));
        assert_eq!(validate_hand(&HandIn { vt44_ok: false, ..good }), Err(Reason::Vtable));
        assert_eq!(validate_hand(&HandIn { n: 4, ..good }), Err(Reason::Range));
        assert_eq!(validate_hand(&HandIn { n: 2000, ..good }), Err(Reason::Range));
        assert_eq!(validate_hand(&HandIn { bone: 150, ..good }), Err(Reason::Range));
        // NaN
        let mut x = good;
        x.e[5] = f32::NAN;
        assert_eq!(validate_hand(&x), Err(Reason::Nan));
        let mut x = good;
        x.m[13] = f32::INFINITY;
        assert_eq!(validate_hand(&x), Err(Reason::Nan));
        // a row of length 1.2
        let mut x = good;
        for k in 0..3 {
            x.e[k] *= 1.2;
        }
        assert_eq!(validate_hand(&x), Err(Reason::Ortho));
        // a 0.1 cross dot: rows 0 and 1 not orthogonal
        let mut x = good;
        let r0 = V3::new(x.e[0], x.e[1], x.e[2]);
        let r1 = (V3::new(x.e[4], x.e[5], x.e[6]) + r0 * 0.1).normalize().unwrap();
        (x.e[4], x.e[5], x.e[6]) = (r1.x, r1.y, r1.z);
        assert_eq!(validate_hand(&x), Err(Reason::Ortho));
        // local T of 3 m: neither local nor world (feet are 55 m from the origin)
        let mut x = good;
        x.e[7] = 3.0;
        assert_eq!(validate_hand(&x), Err(Reason::Space));
        // the chr matrix 1.5 m off the physics position
        let x = HandIn { phys: feet + V3::new(1.5, 0.0, 0.0), ..good };
        assert_eq!(validate_hand(&x), Err(Reason::ChrFar));
        // det -1: a mirrored chr matrix
        let mut mir = chr;
        for r in 0..3 {
            mir[r * 4] = -mir[r * 4];
        }
        assert_eq!(validate_hand(&input(e, mir, feet)), Err(Reason::Det));
        // a uniform scale far outside 0.5..2: ortho
        let mut x = good;
        for k in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
            x.e[k] *= 3.0;
        }
        assert_eq!(validate_hand(&x), Err(Reason::Ortho));
    }

    #[test]
    fn c6_hold_uniformly_scaled_bone_validates() {
        // review: a body-proportion slider scales the hand bone uniformly (1.15): accepted, the scale recorded and
        // removed, so the gun is still drawn at IN_TO_M (its own size)
        let feet = V3::new(50.0, 10.0, 20.0);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q_normalize([0.0, 0.2, 0.0, 0.98]));
        let mut e = m_trs(&[0.35, 1.25, 0.15], &q_normalize([0.3, -0.1, 0.2, 0.9]));
        for k in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
            e[k] *= 1.15;
        }
        assert!(rows_orthonormal(&e));
        let ok = validate_hand(&input(e, chr, feet)).unwrap();
        assert!((ok.scale - 1.15).abs() < 1e-3, "scale {}", ok.scale);
        assert!((ok.det - 1.0).abs() < 1e-3);
        let a = parse_axes("+x,+z,+y").unwrap();
        let w = gun_from_hand(&ok.h, &grip_matrix([0.0; 3], [0.0; 3], &a));
        for c in 0..3 {
            assert!((col(&w, c).len() - IN_TO_M).abs() < 1e-5, "column {c} {}", col(&w, c).len());
        }
        // non-uniform (one row 1.15, the others 1) is still refused
        let mut x = m_trs(&[0.35, 1.25, 0.15], &q_normalize([0.3, -0.1, 0.2, 0.9]));
        for k in 0..3 {
            x[k] *= 1.15;
        }
        assert!(!rows_orthonormal(&x));
    }

    #[test]
    fn c6_hold_jump_guard_holds_one_frame_then_falls_back() {
        let mut t = HandTrack::default();
        let a = crate::xform::m_from_translation(&[10.0, 0.0, 0.0]);
        let b = crate::xform::m_from_translation(&[14.5, 0.0, 0.0]);
        assert_eq!(t.step(0.0, Ok(a), 0.0), Ok(a));
        // 4 m jump: hold the last good for one frame ...
        assert_eq!(t.step(0.016, Ok(b), 0.0), Ok(a));
        // ... then fall back, and the next frame starts fresh
        assert_eq!(t.step(0.033, Ok(b), 0.0), Err(Reason::Jump));
        assert_eq!(t.step(0.05, Ok(b), 0.0), Ok(b));
        // a failed frame passes its reason through
        assert_eq!(t.step(0.066, Err(Reason::Ortho), 0.0), Err(Reason::Ortho));
        // smoothing 0.5 lerps the anchor halfway to the last good one
        let c = crate::xform::m_from_translation(&[15.5, 0.0, 0.0]);
        let s = t.step(0.083, Ok(c), 0.5).unwrap();
        assert!((s[3] - 15.0).abs() < 1e-6);
        // not drawn for a second: the old anchor is forgotten (a far move is no jump)
        assert_eq!(t.step(2.0, Ok(a), 0.0), Ok(a));
    }

    #[test]
    fn c6_hold_fallback_matrix_is_stage_a() {
        // offset mode / every fallback draws exactly today's world_gun_matrix: nothing in this module touches it
        let feet = V3::new(1.0, 2.0, 3.0);
        let fwd = V3::new(0.0, 0.0, 1.0);
        let m = world_gun_matrix(feet, fwd, [0.45, 0.20, 1.15], 0.0).unwrap();
        assert!(det3(&m) < 0.0);
        let again = world_gun_matrix(feet, fwd, [0.45, 0.20, 1.15], 0.0).unwrap();
        assert_eq!(m, again);
        // the hand path with default axes keeps the same handedness as stage A
        let chr = m_trs(&[feet.x, feet.y, feet.z], &[0.0, 0.0, 0.0, 1.0]);
        let ok = validate_hand(&input(m_trs(&[0.3, 1.2, 0.2], &[0.0, 0.0, 0.0, 1.0]), chr, feet)).unwrap();
        let w = gun_from_hand(&ok.h, &grip_matrix([0.0; 3], [0.0; 3], &parse_axes("+x,+z,+y").unwrap()));
        assert_eq!(det3(&w).signum(), det3(&m).signum());
        assert!((det3(&w) - det3(&m)).abs() < 1e-9);
    }

    #[test]
    fn c6_hold_fallback_log_rate_limits() {
        let mut f = FallbackLog::default();
        let mut ev = Vec::new();
        f.frame(0.0, Ok(()), &mut ev);
        assert_eq!(ev, vec![FbEvent::HandOk]);
        ev.clear();
        for k in 0..10 {
            f.frame(0.1 + k as f64 * 0.01, Err(Reason::Space), &mut ev);
        }
        assert_eq!(ev, vec![FbEvent::Reason(Reason::Space)], "one line for the new reason, none per frame");
        ev.clear();
        f.frame(1.0, Err(Reason::Det), &mut ev);
        f.frame(1.1, Ok(()), &mut ev);
        assert_eq!(ev, vec![FbEvent::Reason(Reason::Det), FbEvent::HandOk]);
        ev.clear();
        f.frame(30.0, Ok(()), &mut ev);
        match &ev[..] {
            [FbEvent::Summary { reasons, bad, total }] => {
                assert_eq!(reasons_text(reasons), "space:10,det:1");
                assert_eq!((*bad, *total), (11, 14));
            }
            other => panic!("{other:?}"),
        }
        ev.clear();
        // a clean window writes no summary
        f.frame(61.0, Ok(()), &mut ev);
        assert!(ev.is_empty());
    }

    #[test]
    fn c6_hold_gun_names_match_gun_g() {
        assert_eq!(MW2_GUNS.len(), crate::coop_vis::GUN_G.len());
        assert!(MW2_GUNS.iter().all(|n| n.ends_with("_mp")));
        let mut s = MW2_GUNS.to_vec();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), 34);
    }

    #[test]
    fn c6_hold_match_flver() {
        let e = |t: [f32; 3]| [1.0, 0.0, 0.0, t[0], 0.0, 1.0, 0.0, t[1], 0.0, 0.0, 1.0, t[2]];
        let entries = [e([0.0, 1.0, 0.0]), e([0.31, 1.2, 0.1]), e([0.3, 1.2, 0.1])];
        assert_eq!(match_flver(V3::new(0.3, 1.205, 0.1), &entries).map(|x| x.0), Some(2));
        assert!(match_flver(V3::new(0.6, 1.2, 0.1), &entries).is_none());
        // a re-based (world) array is matched through the inverse chr matrix
        let feet = V3::new(100.0, 20.0, -30.0);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q_normalize([0.0, 0.5, 0.0, 0.87]));
        let world: Vec<[f32; 12]> = entries.iter().map(|x| m_mul(&chr, x)).collect();
        let m = chr_of(&chr);
        assert_eq!(match_flver_any(V3::new(0.3, 1.205, 0.1), &world, Some(&m)).map(|x| (x.0, x.2)), Some((2, "world")));
        assert_eq!(match_flver_any(V3::new(0.3, 1.205, 0.1), &entries, Some(&m)).map(|x| (x.0, x.2)), Some((2, "local")));
        assert!(match_flver_any(V3::new(0.3, 1.205, 0.1), &world, None).is_none());
        assert_eq!(probe_chunk(&entries, 1, 3), "1:0.31,1.20,0.10|1.00,1.00,1.00 2:0.30,1.20,0.10|1.00,1.00,1.00");
        assert_eq!(v3_text(V3::new(1.0, -0.5, 0.25)), "[1.000,-0.500,0.250]");
    }

    #[test]
    fn c6_attach_hand_world_prefers_world_array() {
        // [L] the chr array is world space: a player 1.5 m from the tile origin must not be read as local (2x position)
        let feet = V3::new(1.0, 0.0, 0.5);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q_normalize([0.0, 0.3, 0.0, 0.95]));
        let ew = m_mul(&chr, &m_trs(&[0.3, 1.2, 0.2], &q_normalize([0.1, 0.2, 0.3, 0.9])));
        let (h, s) = hand_world(&ew, &chr_of(&chr), feet).unwrap();
        assert_eq!((s, h), (Space::World, ew));
    }

    fn weapon_case() -> (Mat34, Mat34, V3) {
        // the hand bone (world) and ER's sword 6 cm from it, rotated against the hand
        let feet = V3::new(40.0, 5.0, -12.0);
        let chr = m_trs(&[feet.x, feet.y, feet.z], &q_normalize([0.0, 0.6, 0.0, 0.8]));
        let hand = m_mul(&chr, &m_trs(&[0.3, 1.1, 0.25], &q_normalize([0.2, 0.1, -0.3, 0.9])));
        let w = m_mul(&hand, &m_trs(&[0.04, 0.0, 0.045], &q_normalize([0.5, 0.0, 0.2, 0.8])));
        (hand, w, feet)
    }

    #[test]
    fn c6_attach_weapon_world_picks_the_drawn_matrix() {
        let (hand, w, _) = weapon_case();
        let ht = translation(&hand);
        let ident = crate::xform::MAT34_IDENTITY;
        // e0 in world next to the hand: e0 (the root is the stale chr copy 1 m away)
        let stale_root = chr_of(&crate::xform::m_from_translation(&[ht.x, ht.y - 1.0, ht.z]));
        let i = WeaponIn { vt_ok: true, root: Some(stale_root), e0: Some(w), n: 1 };
        let (m, src, d) = weapon_world(&i, ht, WEAPON_NEAR_M).unwrap();
        assert_eq!(src, WeaponSrc::E0);
        assert!(close(&m, &w, 1e-5) && d < 0.07);
        // e0 an identity bone at the model origin (model space), the root the weapon's world matrix: root
        let i = WeaponIn { vt_ok: true, root: Some(chr_of(&w)), e0: Some(ident), n: 1 };
        assert_eq!(weapon_world(&i, ht, WEAPON_NEAR_M).unwrap().1, WeaponSrc::Root);
        // e0 a bone relative to a root that sits 2 m away (neither is near the hand alone): root x e0
        let off = crate::xform::m_from_translation(&[0.0, 0.0, 2.0]);
        let bone = m_mul(&crate::xform::m_inv_rigid(&off), &w);
        let i = WeaponIn { vt_ok: true, root: Some(chr_of(&off)), e0: Some(bone), n: 3 };
        let (m, src, _) = weapon_world(&i, ht, WEAPON_NEAR_M).unwrap();
        assert_eq!(src, WeaponSrc::RootE0);
        assert!(close(&m, &w, 1e-4));
        // misses
        assert_eq!(weapon_world(&WeaponIn { vt_ok: false, ..i }, ht, WEAPON_NEAR_M), Err(Reason::WeaponVtable));
        assert_eq!(weapon_world(&WeaponIn { vt_ok: true, root: None, e0: None, n: 0 }, ht, WEAPON_NEAR_M), Err(Reason::WeaponRead));
        assert_eq!(weapon_world(&WeaponIn { vt_ok: true, root: None, e0: Some(w), n: 0 }, ht, WEAPON_NEAR_M), Err(Reason::WeaponRead), "n out of range");
        assert_eq!(weapon_world(&WeaponIn { vt_ok: true, root: None, e0: Some(w), n: 1 }, ht + V3::new(0.5, 0.0, 0.0), WEAPON_NEAR_M), Err(Reason::WeaponFar));
        let mut nan = w;
        nan[0] = f32::NAN;
        assert_eq!(weapon_world(&WeaponIn { vt_ok: true, root: None, e0: Some(nan), n: 1 }, ht, WEAPON_NEAR_M), Err(Reason::WeaponBad));
        let mut mir = w;
        for r in 0..3 {
            mir[r * 4] = -mir[r * 4];
        }
        assert_eq!(weapon_world(&WeaponIn { vt_ok: true, root: None, e0: Some(mir), n: 1 }, ht, WEAPON_NEAR_M), Err(Reason::WeaponDet));
    }

    #[test]
    fn c6_attach_stale_guard_and_learned_grip() {
        let (hand, w, _) = weapon_case();
        let mut g = StaleGuard::default();
        let ht = translation(&hand);
        assert!(!g.step(&w, ht));
        // the hand stands still, the pose repeats: not stale (an idle character)
        for _ in 0..10 {
            assert!(!g.step(&w, ht));
        }
        // the hand moves 5 cm while the pose stays bit-identical: stale on the 3rd frame
        let moved = ht + V3::new(0.05, 0.0, 0.0);
        assert!(!g.step(&w, moved));
        assert!(!g.step(&w, moved));
        assert!(g.step(&w, moved));
        // the pose changes again: fresh
        let mut w2 = w;
        w2[3] += 0.01;
        assert!(!g.step(&w2, moved));
        // the learned grip puts the weapon back on the hand exactly
        let gh = learn_grip(&hand, &w);
        assert!(close(&m_mul(&hand, &gh), &w, 1e-5));
    }

    #[test]
    fn c6_attach_grip_not_learned_from_a_frozen_pose() {
        let (hand, w, _) = weapon_case();
        let mut g = StaleGuard::default();
        let mut grip = None;
        // the policy of coop_vis::hold_frame: learn only on a pose change that is not stale
        let frame = |g: &mut StaleGuard, grip: &mut Option<Mat34>, h: &Mat34, w: &Mat34| -> bool {
            let stale = g.step(w, translation(h));
            if !stale && g.changed() {
                *grip = Some(learn_grip(h, w));
            }
            stale
        };
        assert!(!frame(&mut g, &mut grip, &hand, &w));
        assert!(g.changed());
        let pre = grip.unwrap();
        // the model freezes while the hand walks 2.5 cm a frame (under the threshold at first, then stale)
        let mut stale = false;
        for k in 1..=6 {
            let mut hk = hand;
            hk[3] += 0.025 * k as f32;
            stale |= frame(&mut g, &mut grip, &hk, &w);
            assert!(!g.changed());
        }
        assert!(stale, "the freeze is detected");
        // bone_learned keeps the pre-freeze grip: the weapon follows the moved hand exactly
        assert!(close(&grip.unwrap(), &pre, 1e-6));
        let mut hk = hand;
        hk[3] += 0.15;
        let mut want = w;
        want[3] += 0.15;
        assert!(close(&m_mul(&hk, &grip.unwrap()), &want, 1e-5));
    }

    #[test]
    fn c6_attach_axes_text_and_vote() {
        // every (forward, up) pair of different axes gives a mirrored (det -1) map with X / Z where asked
        let mut n = 0;
        for f in 0..6 {
            for u in 0..6 {
                match axes_text(f, u) {
                    Some(t) => {
                        let a = parse_axes(&t).unwrap();
                        assert!(det3(&a) < 0.0);
                        assert_eq!(&t[..2], AXIS_TOKENS[f]);
                        assert_eq!(&t[6..], AXIS_TOKENS[u]);
                        n += 1;
                    }
                    None => assert_eq!(f / 2, u / 2),
                }
            }
        }
        assert_eq!(n, 24);
        assert!(parse_axes(WEAPON_AXES_RIG).is_ok());
        // a weapon whose -z points along the facing and +x up (the crossbow pose): the vote learns "-z,..,+x"
        let fwd = V3::new(0.6, 0.0, 0.8);
        let up = crate::vec3::UP;
        let mut w = [0.0f32; 12];
        set_col(&mut w, 0, up);
        set_col(&mut w, 2, fwd * -1.0);
        set_col(&mut w, 1, (fwd * -1.0).cross(up));
        assert!((det3(&w) - 1.0).abs() < 1e-5, "a proper rotation");
        let mut v = AxesVote::default();
        for _ in 0..(VOTE_N - 1) {
            assert!(v.observe(&w, fwd, up));
        }
        assert!(v.result().is_none(), "not before VOTE_N votes");
        assert!(!v.observe(&w, crate::vec3::UP, up), "a vertical 'facing' takes the up axis: nothing left near up, no vote");
        assert!(v.observe(&w, fwd, up));
        let (t, votes) = v.result().unwrap();
        assert_eq!((&t[..2], &t[6..], votes), ("-z", "+x", VOTE_N));
        // with the learned map the MW2 muzzle (+X) points along the facing and MW2 up (+Z) up
        let g = gun_from_hand(&w, &grip_matrix([0.0; 3], [0.0; 3], &parse_axes(&t).unwrap()));
        let mz = m_point(&g, &[1.0, 0.0, 0.0]);
        let mu = m_point(&g, &[0.0, 0.0, 1.0]);
        assert!(V3::new(mz[0], mz[1], mz[2]).normalize().unwrap().dot(fwd) > 0.999);
        assert!(V3::new(mu[0], mu[1], mu[2]).normalize().unwrap().dot(up) > 0.999);
        // a split vote (below VOTE_SHARE) gives nothing
        let mut s = AxesVote::default();
        let mut w2 = w;
        set_col(&mut w2, 0, up * -1.0);
        set_col(&mut w2, 1, col(&w, 1) * -1.0);
        for k in 0..VOTE_N * 2 {
            assert!(s.observe(if k % 2 == 0 { &w } else { &w2 }, fwd, up));
        }
        assert!(s.result().is_none());
    }

    /// A weapon frame like the 17:32 probe: -Y = facing tilted `up_deg` above horizontal, +Z = the character's left,
    /// +X = the hilt axis (completes a proper rotation), origin at `o`.
    fn er_weapon_frame(face: V3, up_deg: f32, o: V3) -> Mat34 {
        let f = face.normalize().unwrap();
        let up = crate::vec3::UP;
        let (s, c) = up_deg.to_radians().sin_cos();
        let muzzle = (f * c + up * s).normalize().unwrap();
        // +Z = fwd x up, so that +X = -muzzle x Z comes out up (as the probe's hilt axis)
        let left = f.cross(up).normalize().unwrap();
        let mut w = [0.0f32; 12];
        set_col(&mut w, 1, muzzle * -1.0);
        set_col(&mut w, 2, left);
        let x = (muzzle * -1.0).cross(left);
        set_col(&mut w, 0, x);
        if det3(&w) < 0.0 {
            set_col(&mut w, 0, x * -1.0);
        }
        w[3] = o.x;
        w[7] = o.y;
        w[11] = o.z;
        w
    }

    #[test]
    fn c6_grip_table_matches_the_research_and_lands_in_the_fist() {
        let rig = parse_axes(WEAPON_AXES_RIG).unwrap();
        assert!(det3(&rig) < 0.0);
        // the default rule: [0.020, -0.300, 0.000] / [0, 0, -18]
        let (o, r) = weapon_grip(GRIP_IN_DEFAULT, &rig, WEAPON_RAKE_DEG);
        assert!(close(&o, &[0.020, -0.300, 0.0], 0.0015), "{o:?}");
        assert_eq!(r, [0.0, 0.0, -18.0]);
        // grip3.md 4: the per-gun table (offset_m, 3 decimals)
        for (gun, want) in [
            ("striker", [0.000, -0.304]),
            ("barrett", [0.026, -0.363]),
            ("m21", [0.008, -0.384]),
            ("m1014", [0.043, -0.335]),
            ("scar", [0.007, -0.330]),
            ("m240", [0.041, -0.327]),
            ("mp5k", [-0.004, -0.324]),
            ("ump45", [0.016, -0.321]),
            ("spas12", [0.031, -0.319]),
            ("kriss", [-0.046, -0.293]),
            ("m79", [-0.044, -0.301]),
            ("ranger", [-0.023, -0.293]),
            ("aug", [0.033, -0.270]),
            ("mg4", [0.039, -0.262]),
            ("tavor", [0.020, -0.262]),
            ("uzi", [0.029, -0.273]),
            ("wa2000", [0.049, -0.269]),
            ("rpd", [0.037, -0.236]),
            ("sa80", [0.043, -0.222]),
        ] {
            let (o, _) = weapon_grip(grip_point_in(gun), &rig, WEAPON_RAKE_DEG);
            assert!(close(&o[..2], &want, 0.0015) && o[2].abs() < 1e-6, "{gun}: {o:?} vs {want:?}");
        }
        // every gun of the arsenal has a row; the row names are the arsenal names
        assert_eq!(GRIP_IN.len(), MW2_GUNS.len());
        for g in MW2_GUNS {
            assert!(GRIP_IN.iter().any(|(n, _)| *n == g), "{g} has no grip row");
        }
        assert_eq!(grip_point_in("nosuchgun"), GRIP_IN_DEFAULT);
        assert_eq!(grip_point_in("javelin_mp"), GRIP_IN_DEFAULT);
        // in an ER hold tilted 18.8 deg muzzle-up, every gun's grip point sits in the fist (the weapon origin), the bore
        // is ~level (within 1 deg) along the facing, and the gun's top points up
        let face = V3::new(0.6, 0.0, -0.8);
        let fist = V3::new(-7.4, 7.0, -3.9);
        let w = er_weapon_frame(face, 18.8, fist);
        for (gun, g) in GRIP_IN {
            let (o, r) = weapon_grip(g, &rig, WEAPON_RAKE_DEG);
            let gun_m = gun_from_hand(&w, &grip_matrix(o, r, &rig));
            let p = m_point(&gun_m, &g);
            assert!((V3::new(p[0], p[1], p[2]) - fist).len() < 1e-4, "{gun}: grip {p:?} not in the fist");
            let a = m_point(&gun_m, &[0.0, 0.0, 0.0]);
            let b = m_point(&gun_m, &[10.0, 0.0, 0.0]);
            let bore = (V3::new(b[0], b[1], b[2]) - V3::new(a[0], a[1], a[2])).normalize().unwrap();
            assert!(bore.dot(face.normalize().unwrap()) > 0.999, "{gun}: bore {bore:?}");
            let t = m_point(&gun_m, &[0.0, 0.0, 10.0]);
            assert!((t[1] - a[1]) > 0.24, "{gun}: the gun's top is not up");
        }
    }

    #[test]
    fn c6_grip_auto_axes_only_accepts_the_rig_map() {
        assert!(axes_auto_accept(WEAPON_AXES_RIG));
        assert!(axes_auto_accept(" -y, +z, +x "), "parsed, not compared as text");
        for t in ["+y,+x,+z", "-z,-y,+x", "+x,+z,+y", "-y,-z,-x", "junk"] {
            assert!(!axes_auto_accept(t), "{t}");
        }
        // the rig map in the probe frame: MW2 forward along the facing, MW2 up along the hilt axis (weapon +X)
        let face = V3::new(-0.2, 0.0, 1.0);
        let w = er_weapon_frame(face, 0.0, V3::ZERO);
        let g = gun_from_hand(&w, &grip_matrix([0.0; 3], [0.0; 3], &parse_axes(WEAPON_AXES_RIG).unwrap()));
        let f = m_point(&g, &[1.0, 0.0, 0.0]);
        assert!(V3::new(f[0], f[1], f[2]).normalize().unwrap().dot(face.normalize().unwrap()) > 0.999);
        // the first build's guess pointed the barrel backward (the round-2 screenshot)
        let bad = gun_from_hand(&w, &grip_matrix([0.0; 3], [0.0; 3], &parse_axes("+y,+x,+z").unwrap()));
        let fb = m_point(&bad, &[1.0, 0.0, 0.0]);
        assert!(V3::new(fb[0], fb[1], fb[2]).normalize().unwrap().dot(face.normalize().unwrap()) < -0.999);
    }

    #[test]
    fn c6_ads_blend_endpoints_midpoint_timing() {
        let a = gun_from_hand(&m_trs(&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0, 1.0]), &parse_axes("+x,+z,+y").unwrap());
        let b = gun_from_hand(&m_trs(&[2.0, 2.0, 3.0], &q_normalize([0.0, 0.7, 0.0, 0.7])), &parse_axes("+x,+z,+y").unwrap());
        assert_eq!(blend_mat34(&a, &b, 0.0), a);
        assert_eq!(blend_mat34(&a, &b, 1.0), b);
        let m = blend_mat34(&a, &b, 0.5);
        let (c0, c1, c2) = (col(&m, 0), col(&m, 1), col(&m, 2));
        for c in [c0, c1, c2] {
            assert!((c.len() - IN_TO_M).abs() < 1e-6);
        }
        assert!(c0.dot(c1).abs() < 1e-9 && c0.dot(c2).abs() < 1e-9 && c1.dot(c2).abs() < 1e-9);
        assert!(det3(&m) < 0.0, "handedness kept");
        assert!((translation(&m) - V3::new(1.5, 2.0, 3.0)).len() < 1e-6);
        // 0.12 s in at 60 Hz: 8 frames to full, then out again
        let mut u = 0.0;
        let mut n = 0;
        while u < 1.0 {
            u = ads_blend_step(u, true, 1.0 / 60.0);
            n += 1;
        }
        assert!((7..=8).contains(&n), "{n}");
        assert_eq!(ads_blend_step(1.0, false, 0.12), 0.0);
    }

    /// C6 aim 4 (aim4.md 3.3): the barrel turns about the pistol grip onto the camera's aim point; rigid, det -1 kept,
    /// the grip does not move; half the rotation at u = 0.5; beyond 20 deg nothing is changed (Err with the angle).
    #[test]
    fn c6_aim_align_turns_the_barrel_about_the_grip() {
        // the default gun on a weapon frame 18 deg below the aim line (the rake question)
        let axes = parse_axes(WEAPON_AXES_RIG).unwrap();
        let w = m_trs(&[2.0, 1.4, 5.0], &q_normalize([0.0, 0.0, (-9.0f32).to_radians().sin(), (-9.0f32).to_radians().cos()]));
        let (o, r) = weapon_grip(GRIP_IN_DEFAULT, &axes, WEAPON_RAKE_DEG);
        let m = gun_from_hand(&w, &grip_matrix(o, r, &axes));
        let pivot = gun_point(&m, GRIP_IN_DEFAULT);
        assert!((pivot - translation(&w)).len() < 0.002, "the pistol grip sits on ER's weapon origin");
        let f0 = gun_barrel(&m).unwrap();
        // a target 12 deg off the barrel, 30 m away
        let side = f0.cross(V3::new(0.0, 1.0, 0.0)).normalize().unwrap();
        let dir = (f0 * 12.0f32.to_radians().cos() + side * 12.0f32.to_radians().sin()).normalize().unwrap();
        let target = pivot + dir * AIM_ALIGN_DIST_M;
        assert!((aim_error_deg(&m, pivot, target).unwrap() - 12.0).abs() < 0.05);
        let (a, deg) = aim_align(&m, pivot, target, 1.0, AIM_ALIGN_MAX_DEG).unwrap();
        assert!((deg - 12.0).abs() < 0.05);
        assert!(aim_error_deg(&a, pivot, target).unwrap() < 0.05, "on the aim line");
        assert!((gun_point(&a, GRIP_IN_DEFAULT) - pivot).len() < 1e-4, "the grip stays in the fist");
        assert!(det3(&a) < 0.0 && (col(&a, 0).len() - IN_TO_M).abs() < 1e-6 && col(&a, 0).dot(col(&a, 1)).abs() < 1e-8);
        let (h, _) = aim_align(&m, pivot, target, 0.5, AIM_ALIGN_MAX_DEG).unwrap();
        assert!((aim_error_deg(&h, pivot, target).unwrap() - 6.0).abs() < 0.05, "half way at u = 0.5");
        assert_eq!(aim_align(&m, pivot, target, 0.0, AIM_ALIGN_MAX_DEG).unwrap().0, m);
        // 25 deg off: clamped, the caller draws ER's weapon frame as is
        let far = pivot + (f0 * 25.0f32.to_radians().cos() + side * 25.0f32.to_radians().sin()) * AIM_ALIGN_DIST_M;
        assert!(matches!(aim_align(&m, pivot, far, 1.0, AIM_ALIGN_MAX_DEG), Err(d) if (d - 25.0).abs() < 0.05));
        // degenerate target at the pivot: unchanged
        assert_eq!(aim_align(&m, pivot, pivot, 1.0, AIM_ALIGN_MAX_DEG).unwrap().0, m);
        // the rotation helper is a proper rotation
        let r = rot_axis_angle(V3::new(0.0, 1.0, 0.0), 0.3);
        assert!((det3(&r) - 1.0).abs() < 1e-6);
    }

    /// C6 round 7: `tp_ads_hold = "aim"` shoulders the gun inside an ER aim episode only when ER's pose did not turn the
    /// weapon (the align clamped); "hand" never; outside an episode as before (after the align blend).
    #[test]
    fn c6_ads_hold_aim_inside_an_episode_without_er_pose() {
        assert!(ads_hold_aim(true, false, 0.0, false), "plain MW2 ADS, as before");
        assert!(!ads_hold_aim(true, false, 0.4, false), "the align blend still out");
        assert!(!ads_hold_aim(true, true, 1.0, false), "ER's pose turned the weapon: the align draws it");
        assert!(ads_hold_aim(true, true, 1.0, true), "the round-6 case: ER kept the carry, the gun shoulders");
        assert!(ads_hold_aim(true, false, 0.3, true), "kept through the release blend");
        assert!(!ads_hold_aim(false, true, 1.0, true) && !ads_hold_aim(false, false, 0.0, false), "hand: never");
    }

    /// C6 round 7 review: the carry 10..18 deg off the aim line (the align does not clamp) must still shoulder the gun
    /// inside a third-person episode with `tp_ads_hold = "aim"` (the "gun stays in the same position" report).
    #[test]
    fn c6_aim_nopose_latches_without_the_align_clamp() {
        let axes = parse_axes(WEAPON_AXES_RIG).unwrap();
        let w = m_trs(&[2.0, 1.4, 5.0], &q_normalize([0.0, 0.0, (-9.0f32).to_radians().sin(), (-9.0f32).to_radians().cos()]));
        let (o, r) = weapon_grip(GRIP_IN_DEFAULT, &axes, WEAPON_RAKE_DEG);
        let m = gun_from_hand(&w, &grip_matrix(o, r, &axes));
        let pivot = gun_point(&m, GRIP_IN_DEFAULT);
        let f0 = gun_barrel(&m).unwrap();
        let side = f0.cross(V3::new(0.0, 1.0, 0.0)).normalize().unwrap();
        for d in [9.7f32, 11.1, 17.3, 18.5] {
            let t = pivot + (f0 * d.to_radians().cos() + side * d.to_radians().sin()) * AIM_ALIGN_DIST_M;
            assert!(aim_align(&m, pivot, t, 1.0, AIM_ALIGN_MAX_DEG).is_ok(), "{d} deg: no clamp");
            let l = aim_nopose_step(false, true, true, true, 0.1, false);
            assert!(l && ads_hold_aim(true, true, 1.0, l), "{d} deg: the gun shoulders anyway");
        }
        // kept through the release blend, cleared after it; "hand" never latches
        assert!(aim_nopose_step(true, true, false, false, 0.3, false));
        assert!(!aim_nopose_step(true, true, false, false, 0.0, false));
        assert!(!aim_nopose_step(false, false, true, true, 1.0, false));
        assert!(!aim_nopose_step(false, true, false, true, 0.5, false), "the episode before ER's confirmed aim: not yet");
    }

    /// C6 part 17: the guard episode raises the gun only when the guard pose is not detected or does not align.
    #[test]
    fn c6_guard_nopose_raises_only_without_the_guard_pose() {
        assert!(!guard_nopose(true, true, false, false), "the guard shows and aligns: ER's weapon frame + aim_align");
        assert!(guard_nopose(true, true, true, false), "not detected: the gun raise");
        assert!(guard_nopose(true, true, false, true), "detected but off the aim line beyond the max: the gun raise");
        assert!(!guard_nopose(false, true, true, true), "tp_ads_hold = hand: never");
        assert!(!guard_nopose(true, false, true, true), "no guard episode: the plain ADS path decides");
        // inside a guard episode the plain ADS raise stays out unless no_pose (ads_hold_aim's episode rule)
        assert!(!ads_hold_aim(true, true, 0.0, guard_nopose(true, true, false, false)));
        assert!(ads_hold_aim(true, true, 0.0, guard_nopose(true, true, true, false)));
    }

    /// C6 part 17 review: the guard clamp neither latches on one frame (ER's raise in motion, a block-hit reaction) nor
    /// stays latched once the align is back under the limit.
    #[test]
    fn c6_guard_clamp_debounced_and_unlatching() {
        let dt = 1.0 / 60.0;
        let mut g = GuardClamp::default();
        // detected at t = 0; every frame over the limit while the raise settles: no clamp
        let mut t = 0.0;
        while t < GUARD_ALIGN_SETTLE_S - 1e-6 {
            assert!(!g.step(t, true, Some(true)), "settling at {t}");
            assert!(!g.settled(t));
            t += dt;
        }
        // one frame over the limit after the settle, then in-limit frames: never the raise
        assert!(!g.step(t, true, Some(true)));
        for _ in 0..30 {
            t += dt;
            assert!(!g.step(t, true, Some(false)));
            assert!(!guard_nopose(true, true, false, g.clamped));
        }
        assert!(g.settled(t));
        // over the limit for GUARD_CLAMP_ON_S: the raise
        let t_on = t + dt;
        loop {
            t += dt;
            if g.step(t, true, Some(true)) {
                break;
            }
            assert!(t - t_on < GUARD_CLAMP_ON_S + 2.0 * dt, "clamped in time");
        }
        assert!(t - t_on >= GUARD_CLAMP_ON_S - 1e-9);
        assert!(guard_nopose(true, true, false, g.clamped));
        // a frame that starts clamped and later aligns: back on ER's guard pose (no raise)
        assert!(g.step(t + dt, true, Some(false)), "one in-limit frame does not unclamp");
        let t_off = t + dt;
        t = t_off;
        while g.step(t, true, Some(false)) {
            t += dt;
            assert!(t - t_off < GUARD_CLAMP_OFF_S + 2.0 * dt, "unclamped in time");
        }
        assert!(!guard_nopose(true, true, false, g.clamped));
        // unmeasured frames change nothing; not detected resets (the next detection settles again)
        assert!(!g.step(t + dt, true, None));
        assert!(!g.step(t + 2.0 * dt, false, Some(true)));
        assert!(!g.settled(t + 2.0 * dt));
        assert!(!g.step(t + 3.0 * dt, true, Some(true)), "a new detection settles first");
    }

    // ---- C6 part 17 addendum: the logged 2026-10-22 11:46-11:47 frames as fixtures --------------------------------

    /// ER's weapon pose from a `HOLD axes` line (hx / hy / hz = b73's columns; e0 = b73 x Rz(180) = [-hx, -hy, hz]),
    /// at `t` (orthonormalised: the line has 3 decimals).
    fn e0_from_axes(hx: [f32; 3], hy: [f32; 3], hz: [f32; 3], t: [f32; 3]) -> Mat34 {
        let m: Mat34 = [-hx[0], -hy[0], hz[0], t[0], -hx[1], -hy[1], hz[1], t[1], -hx[2], -hy[2], hz[2], t[2]];
        orthonormalize(&m).unwrap().0
    }

    /// The six `HOLD axes ... ads=1 posed=1` lines of the detected, yaw-aligned guards (hx, hy, hz, cam_fwd).
    const GUARD_AXES_LOG: [([f32; 3], [f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.141, -0.981, 0.137], [-0.962, -0.168, -0.214], [0.233, -0.101, -0.967], [-0.260, -0.126, 0.957]),
        ([-0.136, -0.981, 0.142], [0.208, -0.168, -0.963], [0.969, -0.101, 0.227], [-0.584, -0.798, -0.149]),
        ([-0.069, -0.981, 0.184], [-0.185, -0.168, -0.968], [0.980, -0.101, -0.170], [-0.989, -0.023, 0.146]),
        ([-0.044, -0.990, 0.137], [-0.233, -0.123, -0.965], [0.971, -0.075, -0.226], [-0.978, -0.153, 0.138]),
        ([-0.166, -0.978, 0.129], [0.329, -0.178, -0.928], [0.930, -0.112, 0.351], [-0.934, -0.027, -0.355]),
        ([-0.140, -0.982, 0.130], [0.296, -0.167, -0.941], [0.945, -0.093, 0.314], [-0.935, -0.096, -0.341]),
    ];

    fn gun_on(w: &Mat34, o: [f32; 3], r: [f32; 3], axes: &Mat34) -> Mat34 {
        gun_from_hand(w, &grip_matrix(o, r, axes))
    }

    fn deg(a: V3, b: V3) -> f32 {
        a.normalize().unwrap().dot(b.normalize().unwrap()).clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// C6 part 17 addendum: on every logged guard the sword-carry grip's barrel was 60+ deg off the aim (the 79.9..96.2
    /// clamps); the guard frame puts it within 10 deg of the camera's forward (the camera pitch) and within 7 deg of the
    /// level facing on all six, the pistol grip still on ER's weapon origin (the fist), inside tp_guard_align_max_deg 45.
    #[test]
    fn c6_guard_frame_fixes_the_logged_guards() {
        let rig = parse_axes(WEAPON_AXES_RIG).unwrap();
        let gun = "ump45_mp";
        let g = grip_point_in(gun);
        let (so, sr) = weapon_grip(g, &rig, WEAPON_RAKE_DEG);
        let (go, gr) = frame_grip(g, &guard_frame_builtin(), &rig);
        let origin = [-4.672, 7.213, -4.518];
        let mut near = 0;
        for (hx, hy, hz, cf) in GUARD_AXES_LOG {
            let w = e0_from_axes(hx, hy, hz, origin);
            let cam = v3a(cf);
            let level = V3::new(cf[0], 0.0, cf[2]);
            let old = gun_on(&w, so, sr, &rig);
            assert!(deg(gun_barrel(&old).unwrap(), cam) > 60.0, "the logged clamp: the sword grip is far off");
            let m = gun_on(&w, go, gr, &rig);
            let b = gun_barrel(&m).unwrap();
            assert!(deg(b, level) <= 7.0, "level facing: {}", deg(b, level));
            if cf[1].abs() < 0.2 {
                assert!(deg(b, cam) <= 10.0, "camera forward: {}", deg(b, cam));
                near += 1;
                let pivot = gun_point(&m, g);
                assert!(aim_align(&m, pivot, pivot + cam * AIM_ALIGN_DIST_M, 1.0, 45.0).is_ok(), "inside the guard max");
            }
            // the pistol grip stays in the fist; the top of the gun up; det -1 kept
            assert!((gun_point(&m, g) - translation(&w)).len() < 1e-3);
            assert!(col(&m, 2).normalize().unwrap().y > 0.95);
            assert!(det3(&m) < 0.0);
            assert!(frame_fwd_err_deg(&w, &guard_frame_builtin(), level).unwrap() <= 7.0);
        }
        assert_eq!(near, 5);
        // the pin of the built-in frame: the guard map -z,-y,+x and a small turn
        let (axes, rot) = frame_pin(&guard_frame_builtin());
        assert_eq!(axes, "-z,-y,+x");
        assert!(rot.iter().all(|a| a.abs() < 15.0), "{rot:?}");
        let back = m_mul(&rot_xyz_deg(rot), &parse_axes(&axes).unwrap());
        assert!(close(&back, &guard_frame_builtin(), 1e-4));
    }

    /// C6 part 17 addendum: the polearm carry (the logged standing frames t=0.0 and t=7.1): a long gun's barrel runs from
    /// the right fist to the left hand (along the shaft), a short gun's level along the body's facing; the pistol grip on
    /// ER's weapon origin; the sword grip's barrel was 15+ deg off the shaft.
    #[test]
    fn c6_polearm_carry_from_the_logged_carry() {
        let rig = parse_axes(WEAPON_AXES_RIG).unwrap();
        // (e0, the left hand b29, the body's facing [I: the guard's yaw just before the release for t=7.1])
        let frames: [([f32; 12], [f32; 3], Option<[f32; 3]>); 2] = [
            ([-0.558, -0.400, -0.727, -4.128, 0.778, -0.557, -0.290, 6.832, -0.289, -0.727, 0.622, -5.127], [-3.987, 7.114, -4.688], None),
            ([0.608, 0.749, 0.265, -4.286, 0.789, -0.533, -0.305, 6.795, -0.087, 0.395, -0.915, -4.511], [-4.663, 7.052, -4.805], Some([-0.988, 0.0, 0.156])),
        ];
        for (e, lh, facing) in frames {
            let w = orthonormalize(&e).unwrap().0;
            let shaft = v3a(lh) - translation(&w);
            assert!((shaft.len() - 0.54).abs() < 0.01, "both hands on the shaft: {}", shaft.len());
            for gun in ["m4_mp", "barrett_mp", "ranger_mp", "uzi_mp"] {
                let g = grip_point_in(gun);
                let kind = polearm_kind(gun, POLEARM_LONG_MIN_M);
                let (o, r) = frame_grip(g, &polearm_frame(kind), &rig);
                let m = gun_on(&w, o, r, &rig);
                let b = gun_barrel(&m).unwrap();
                assert!((gun_point(&m, g) - translation(&w)).len() < 1e-3, "{gun}: the pistol grip in the fist");
                assert!(det3(&m) < 0.0);
                match kind {
                    PolearmKind::Long => {
                        assert!(deg(b, shaft) <= 3.0, "{gun} along the shaft: {}", deg(b, shaft));
                        assert!(col(&m, 2).normalize().unwrap().y > 0.75, "{gun}: the top up");
                    }
                    PolearmKind::Short => {
                        assert!(b.y.abs() < 0.1, "{gun} level: {}", b.y);
                        if let Some(f) = facing {
                            assert!(deg(b, v3a(f)) <= 10.0, "{gun} ahead: {}", deg(b, v3a(f)));
                        }
                    }
                }
            }
            let (so, sr) = weapon_grip(grip_point_in("m4_mp"), &rig, WEAPON_RAKE_DEG);
            assert!(deg(gun_barrel(&gun_on(&w, so, sr, &rig)).unwrap(), shaft) > 15.0, "the sword grip was off the shaft");
        }
        // long vs short by the muzzle's reach (unknown = long), the threshold is the config's
        assert_eq!((polearm_kind("m4", 0.5), polearm_kind("ranger_mp", 0.5), polearm_kind("javelin_mp", 0.5)), (PolearmKind::Long, PolearmKind::Short, PolearmKind::Long));
        assert_eq!((polearm_kind("p90_mp", 0.2), polearm_kind("m4_mp", 0.7)), (PolearmKind::Long, PolearmKind::Short));
        assert_eq!(MUZZLE_AHEAD_M.iter().filter(|(_, m)| *m < POLEARM_LONG_MIN_M).count(), 10);
        assert!(MUZZLE_AHEAD_M.iter().all(|(n, _)| MW2_GUNS.contains(n)));
    }

    /// C6 part 17 addendum: rot_xyz_of inverts rot_xyz_deg; frame_grip reproduces any frame with the pistol grip at the
    /// origin on any map; frame_pin reproduces the frame with a permutation.
    #[test]
    fn c6_frame_grip_round_trips() {
        for a in [[0.0, 0.0, -18.0], [88.15, 5.6, 9.37], [-10.6, 26.2, -3.6], [31.5, 16.7, -35.1], [170.0, -80.0, 45.0], [0.0, 90.0, 30.0]] {
            let r = rot_xyz_deg(a);
            let back = rot_xyz_deg(rot_xyz_of(&r));
            assert!(close(&r, &back, 1e-4), "{a:?}");
        }
        let g = grip_point_in("rpg_mp");
        for axes in [WEAPON_AXES_RIG, "+x,+z,+y", "-z,-y,+x"] {
            let a = parse_axes(axes).unwrap();
            for f in [guard_frame_builtin(), polearm_frame(PolearmKind::Long), polearm_frame(PolearmKind::Short)] {
                let (o, r) = frame_grip(g, &f, &a);
                let gm = grip_matrix(o, r, &a);
                assert!(close(&transpose3(&gm), &transpose3(&f), 1e-4), "{axes}");
                // the grip matrix takes metres (gun_from_hand scales the inches after it)
                let p = m_point(&gm, &g.map(|x| x * IN_TO_M));
                assert!(p.iter().all(|x| x.abs() < 1e-4), "the grip point on the origin: {p:?}");
                let (pa, pr) = frame_pin(&f);
                assert!(close(&m_mul(&rot_xyz_deg(pr), &parse_axes(&pa).unwrap()), &f, 1e-4));
            }
        }
        // the sword grip is the special case rot = [0, 0, rake]
        let rig = parse_axes(WEAPON_AXES_RIG).unwrap();
        let (o, r) = weapon_grip(g, &rig, -18.0);
        assert_eq!((weapon_grip_offset(g, &rig, r), r), (o, [0.0, 0.0, -18.0]));
        assert!(gun_frame(V3::new(0.0, 1.0, 0.0), V3::new(0.0, 2.0, 0.0)).is_none());
    }

    /// C6 part 17 addendum: `guard_axes = "auto"` learns the logged guard from body-yawed copies (the built-in frame
    /// within 3 deg), only once per row, a new row starts again, scattered poses never make a frame.
    #[test]
    fn c6_guard_learn_from_the_logged_guards() {
        let yaw = |d: f32| rot_xyz_deg([0.0, d, 0.0]);
        let mut l = GuardLearn::default();
        let mut got = None;
        let mut t = 0.0;
        for k in 0..40 {
            let (hx, hy, hz, cf) = GUARD_AXES_LOG[k % 6];
            let w0 = e0_from_axes(hx, hy, hz, [0.0; 3]);
            // the same pose with the body turned (world yaw about +Y): the facing turns with it
            let r = yaw(k as f32 * 37.0);
            let w = m_mul(&r, &w0);
            let f = V3::new(cf[0], 0.0, cf[2]);
            let fr = m_point(&r, &[f.x, f.y, f.z]);
            if let Some(x) = l.observe(t, 18_080_000, &w, V3::new(fr[0], fr[1], fr[2]), crate::vec3::UP) {
                got = Some(x);
                break;
            }
            assert!(l.frame(18_080_000).is_none());
            t += GUARD_LEARN_EVERY_S;
        }
        let (f, n, spread) = got.expect("learned");
        assert_eq!(n, GUARD_LEARN_N);
        assert!(spread < 5.0, "{spread}");
        for j in 0..3 {
            assert!(deg(col(&f, j), col(&guard_frame_builtin(), j)) < 3.0, "axis {j}");
        }
        assert!(frame_diff_deg(&f, &guard_frame_builtin()) < 3.0 && frame_diff_deg(&f, &f) < 0.1);
        assert!((frame_diff_deg(&guard_frame_builtin(), &m_mul(&rot_xyz_deg([0.0, 0.0, 30.0]), &guard_frame_builtin())) - 30.0).abs() < 0.1);
        assert_eq!(l.frame(18_080_000), Some(f));
        assert!(l.frame(2_000_000).is_none());
        assert!(l.observe(t + 1.0, 18_080_000, &MAT34_IDENTITY_T, V3::new(0.0, 0.0, 1.0), crate::vec3::UP).is_none(), "learned once");
        // samples closer than GUARD_LEARN_EVERY_S count once; a new row starts again; scattered poses never learn
        let mut l = GuardLearn::default();
        for k in 0..200 {
            let w = rot_xyz_deg([k as f32 * 53.0, k as f32 * 31.0, k as f32 * 17.0]);
            assert!(l.observe(k as f64 * 0.11, 2_000_000, &w, V3::new(0.0, 0.0, 1.0), crate::vec3::UP).is_none());
        }
        assert!(l.frame(2_000_000).is_none());
    }

    const MAT34_IDENTITY_T: Mat34 = crate::xform::MAT34_IDENTITY;

    /// C6 part 16 review: with the real crossbow carrier on, ER's confirmed aim never latches the fallback (the gun stays
    /// on ER's weapon frame, `aim_align` only), and a latch from before the carrier is dropped.
    #[test]
    fn c6_aim_nopose_never_with_the_carrier() {
        assert!(!aim_nopose_step(false, true, true, true, 1.0, true), "carrier on + aim_on: nopose stays false");
        assert!(!aim_nopose_step(true, true, true, true, 1.0, true), "a latch is dropped");
        let l = aim_nopose_step(false, true, true, true, 1.0, true);
        assert!(!ads_hold_aim(true, true, 1.0, l), "inside ER's episode the gun is not shouldered along the camera");
    }
}
