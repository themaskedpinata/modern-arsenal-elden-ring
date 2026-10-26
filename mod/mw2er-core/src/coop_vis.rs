//! C3 PASS C (docs/C3_DESIGN.md part 3): seeing each other in co-op. Pure: the DLL reads the bullet list and the peers'
//! SpEffect lists into plain data, calls these rules and acts on their results.
//!
//! - **Visual twins** (3.4): every damaging spawn keeps network-sync bit 3 OFF (it lands once, on the shooter's PC, and
//!   ER replicates the HP change). Beside it the DLL spawns a no-attack "twin" row (`atkId_Bullet = -1`) WITH bit 3, so
//!   ER sends it to the peers. Row ids: `79800000 + (rid - 79100000)` for the M6 rows (hitscan: one twin per gun, the
//!   band-0 slot `g*1000`; launcher launch `g*1000 + 500`; explosion tiers `g*1000 + 601..603`) and
//!   `79800000 + (rid - 79000000)` for the AC-130 explosion rows (`79800201..79800401`).
//! - **Peer effects** (3.5): [`RemoteTracker`] turns the twins other players own into muzzle / impact / explosion
//!   events (sounds, tracers, flashes). Nothing here ever touches damage.
//! - **Markers** (3.6, 3.8): synced SpEffects carry "MW2 mode on" (hides ER's weapon through the vanilla SpEffectVfx
//!   8060), the gun in hand, the AC-130 ride and the nuke. [`MarkerCtl`] applies / refreshes / removes ours;
//!   [`peer_markers`] reads a peer's list; [`NukeMarkerRx`] dedupes the nuke marker per peer.
//! - **Peer-drop guard** (3.7): [`peer_drop`].
//! - **World guns** (3.6 stage A): [`world_gun_matrix`] / [`world_view_proj`] put an MW2 world model (inches, X forward,
//!   Y left, Z up) on a character (ER world metres, Y up) and project it with ER's camera rows.

use crate::vec3::V3;
use crate::xform::{Mat4, Mat34, m4_mul};

// ---- ids -------------------------------------------------------------------------------------------------------

pub const TWIN_BASE: i32 = 79_800_000;
pub const TWIN_MAX: i32 = 79_899_999;
/// The M6 arsenal rows (`79100000 + g*1000 + kind*100 + idx`).
pub const M6_BASE: i32 = 79_100_000;
pub const M6_MAX: i32 = 79_199_999;
/// The AC-130 explosion rows (`79000000 + slot*100 + tier`, slots 2/3/4 = 105/40/25).
pub const AC130_BASE: i32 = 79_000_000;
pub const AC130_ROWS: (i32, i32) = (79_000_200, 79_000_499);
/// The nuke marker bullet (no attack, no model, short life; synced).
pub const NUKE_MARKER_BULLET: i32 = 79_000_963;
/// The synced marker SpEffects.
pub const SP_MODE: i32 = 79_001_090;
pub const SP_GUN_BASE: i32 = 79_001_100;
pub const SP_GUN_SLOTS: i32 = 50;
pub const SP_AC130: i32 = 79_001_150;
pub const SP_NUKE: i32 = 79_001_190;
/// The nuke's lethal SpEffect: never synced (isDisableNetSync 1), never applied through the synced path.
pub const NUKE_LETHAL_SP: i32 = 79_000_960;

// ---- C6 hide (work/c6/hide.md 4): the MW2-mode marker as a real weapon hide ---------------------------------------

/// One field of the in-memory patch of row [`SP_MODE`]: `from` = the packaged value (a clone of the inert template row
/// 0), `to` = vanilla SpEffect 1466000 "Unseen Blade" (the only vanilla effect that hides a weapon model), as raw
/// little-endian bits (f32 as bits). spCategory stays 0 (Unseen Blade's 162 would let a grease replace the marker and
/// our re-apply strip the grease); endurance, net sync, icon, targets and vfx stay the package's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HideField {
    pub name: &'static str,
    pub off: usize,
    pub width: usize,
    pub from: u32,
    pub to: u32,
}

/// SpEffectParam row size [V Paramdex, check_c6_regulation.py].
pub const SP_ROW_SIZE: usize = 912;
/// `stateInfo` 184 = "Hide Weapon" (Paramdex; only Unseen Blade uses it).
pub const STATE_INFO_HIDE_WEAPON: u32 = 184;
pub const OFF_SP_STATE_INFO: usize = 0x156;

/// The patch (offsets / values pinned against Paramdex and the package by check_c6_regulation.py `hide`, against the
/// vendored SP_EFFECT_PARAM_ST setters by the DLL test `c6_hide_offsets_match_vendored_layout`).
pub const HIDE_FIELDS: [HideField; 12] = [
    HideField { name: "motionInterval", off: 0x00C, width: 4, from: 0x00000000, to: 0xBF800000 },
    HideField { name: "fallDamageRate", off: 0x0DC, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "soulRate", off: 0x0E0, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "equipWeightChangeRate", off: 0x0E4, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "allItemWeightChangeRate", off: 0x0E8, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "soulStealRate", off: 0x118, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "lifeReductionRate", off: 0x11C, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "hpRecoverRate", off: 0x120, width: 4, from: 0x00000000, to: 0x3F800000 },
    HideField { name: "stateInfo", off: 0x156, width: 2, from: 0x00000000, to: 0x000000B8 },
    HideField { name: "wepParamChange", off: 0x158, width: 1, from: 0x00000000, to: 0x00000001 },
    HideField { name: "vowType0", off: 0x165, width: 1, from: 0x00000000, to: 0x000000FF },
    HideField { name: "vowType8", off: 0x166, width: 1, from: 0x00000000, to: 0x000000FF },
];

/// What the row holds now (read field by field in [`HIDE_FIELDS`] order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideRow {
    /// Exactly the package values: may be patched.
    Package,
    /// Exactly the patch: may be restored.
    Patched,
    /// Anything else (another tool, a different package): never written (`HOLD hide foreign`).
    Foreign,
}

pub fn hide_row_state(cur: &[u32]) -> HideRow {
    if cur.len() != HIDE_FIELDS.len() {
        return HideRow::Foreign;
    }
    if HIDE_FIELDS.iter().zip(cur).all(|(f, v)| f.from == *v) {
        HideRow::Package
    } else if HIDE_FIELDS.iter().zip(cur).all(|(f, v)| f.to == *v) {
        HideRow::Patched
    } else {
        HideRow::Foreign
    }
}

/// The stable gun indices g of the 34 MW2 MP guns (convert_arsenal.GUN_INDEX), ascending. A gun marker is
/// `SP_GUN_BASE + position in this list`, so both PCs agree whatever their `[arsenal] guns` order.
pub const GUN_G: [u32; 34] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 21, 22, 23, 24, 25, 31, 32, 33, 34, 41, 42, 43, 44, 45, 46, 51, 52, 53, 54, 55];

/// The marker SpEffect of gun `g`.
pub fn gun_marker(g: u32) -> Option<i32> {
    GUN_G.iter().position(|&x| x == g).map(|k| SP_GUN_BASE + k as i32)
}

/// The gun g of a gun-marker SpEffect id.
pub fn gun_of_marker(id: i32) -> Option<u32> {
    let k = id.checked_sub(SP_GUN_BASE)?;
    if !(0..SP_GUN_SLOTS).contains(&k) {
        return None;
    }
    GUN_G.get(k as usize).copied()
}

/// What a twin row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TwinKind {
    /// A hitscan shot of gun g (tracer + the gun's third-person fire sound).
    Gun,
    /// A launcher firing (its fire sound).
    Launch,
    /// A launcher explosion tier 1 core / 2 ring / 3 single (ER plays the row's explosion SFX; we add the sound).
    Expl(u8),
    /// An AC-130 shell explosion: gun 0 = 105, 1 = 40, 2 = 25; tier as above.
    Ac130 { gun: u8, tier: u8 },
}

impl TwinKind {
    pub fn label(self) -> &'static str {
        match self {
            TwinKind::Gun => "gun",
            TwinKind::Launch => "launch",
            TwinKind::Expl(_) => "expl",
            TwinKind::Ac130 { .. } => "ac130",
        }
    }

    /// The twin is an explosion (one event at its first sighting).
    pub fn is_explosion(self) -> bool {
        matches!(self, TwinKind::Expl(_) | TwinKind::Ac130 { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Twin {
    pub kind: TwinKind,
    /// The stable gun index (0 for the AC-130).
    pub g: u32,
}

/// The twin row of a damaging row (`None` = this row gets no twin).
pub fn twin_of(src_rid: i32) -> Option<i32> {
    if (M6_BASE..=M6_MAX).contains(&src_rid) {
        let rest = src_rid - M6_BASE;
        let (g, r) = (rest / 1000, rest % 1000);
        let (kind, idx) = (r / 100, r % 100);
        if g < 1 || gun_marker(g as u32).is_none() {
            return None;
        }
        return match (kind, idx) {
            // every hitscan band shares the gun's one twin (twins carry no damage, so the band does not matter)
            (0, _) => Some(TWIN_BASE + g * 1000),
            (5, 0) => Some(TWIN_BASE + g * 1000 + 500),
            (6, 1..=3) => Some(TWIN_BASE + g * 1000 + 600 + idx),
            _ => None,
        };
    }
    if (AC130_ROWS.0..=AC130_ROWS.1).contains(&src_rid) {
        let rest = src_rid - AC130_BASE;
        let (slot, tier) = (rest / 100, rest % 100);
        if (2..=4).contains(&slot) && (1..=3).contains(&tier) {
            return Some(TWIN_BASE + rest);
        }
    }
    None
}

/// The row a twin id stands for (decoded from the id alone).
pub fn decode_twin(id: i32) -> Option<Twin> {
    if !(TWIN_BASE..=TWIN_MAX).contains(&id) {
        return None;
    }
    let rest = id - TWIN_BASE;
    if rest < 1000 {
        let (slot, tier) = (rest / 100, rest % 100);
        return ((2..=4).contains(&slot) && (1..=3).contains(&tier)).then_some(Twin { kind: TwinKind::Ac130 { gun: (slot - 2) as u8, tier: tier as u8 }, g: 0 });
    }
    let (g, r) = ((rest / 1000) as u32, rest % 1000);
    gun_marker(g)?;
    let kind = match r {
        0 => TwinKind::Gun,
        500 => TwinKind::Launch,
        601..=603 => TwinKind::Expl((r - 600) as u8),
        _ => return None,
    };
    Some(Twin { kind, g })
}

/// The damaging row a twin mirrors (hitscan twins name band 0).
pub fn twin_src(id: i32) -> Option<i32> {
    let t = decode_twin(id)?;
    Some(match t.kind {
        TwinKind::Ac130 { .. } => AC130_BASE + (id - TWIN_BASE),
        _ => M6_BASE + (id - TWIN_BASE),
    })
}

/// A bullet row that may be spawned with network-sync bit 3: a twin or the nuke marker. Never a damaging row.
pub fn syncable_bullet(id: i32) -> bool {
    decode_twin(id).is_some() || id == NUKE_MARKER_BULLET
}

/// A SpEffect that may be applied with `dont_sync = false`: only the C3 markers (never 79000960, the lethal one).
pub fn syncable_speffect(id: i32) -> bool {
    id == SP_MODE || gun_of_marker(id).is_some() || id == SP_AC130 || id == SP_NUKE
}

// ---- twin sending ------------------------------------------------------------------------------------------------

/// `[coop] twin_min_interval_ms` per gun: an SMG at 40 ms sends at most 25 twins per second (explosions are not limited).
#[derive(Clone, Debug, Default)]
pub struct TwinLimiter {
    last: Vec<(u32, f64)>,
}

impl TwinLimiter {
    pub fn allow(&mut self, now: f64, g: u32, min_interval_s: f64) -> bool {
        match self.last.iter_mut().find(|(x, _)| *x == g) {
            Some((_, t)) if now - *t < min_interval_s - 1e-9 => false,
            Some((_, t)) => {
                *t = now;
                true
            }
            None => {
                self.last.push((g, now));
                true
            }
        }
    }
}

/// Twins go out only with a peer in the session, `visual_twins` on, and only for rows that have one.
pub fn twin_wanted(peers: u32, enabled: bool, src_rid: i32) -> Option<i32> {
    if peers == 0 || !enabled {
        return None;
    }
    twin_of(src_rid)
}

// ---- peer effects (receive) -------------------------------------------------------------------------------------

/// One bullet of the bullet list as plain data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemoteBullet {
    pub handle: u64,
    pub owner: u64,
    pub param_id: i32,
    pub pos: V3,
    pub vel: V3,
}

/// A remote bullet worth tracking: a twin or the nuke marker that another character (not the local player) owns.
pub fn classify_remote(b: &RemoteBullet, local_owner: u64) -> Option<RemoteKind> {
    if b.owner == 0 || b.owner == u64::MAX || b.owner == local_owner || !b.pos.is_finite() {
        return None;
    }
    if b.param_id == NUKE_MARKER_BULLET {
        return Some(RemoteKind::NukeMarker);
    }
    decode_twin(b.param_id).map(RemoteKind::Twin)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteKind {
    Twin(Twin),
    NukeMarker,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FxEvent {
    /// A peer fired gun g (first sighting of a Gun / Launch twin). `sound`: the first muzzle of this owner+gun within
    /// [`MUZZLE_MERGE_S`] (a burst of twins in one frame plays one sound).
    Muzzle { owner: u64, row: i32, twin: Twin, pos: V3, dir: V3, sound: bool },
    /// A Gun / Launch twin vanished (hit or end of life) at `pos`.
    Impact { owner: u64, row: i32, twin: Twin, pos: V3, from: V3 },
    /// An explosion twin appeared (launcher tier, AC-130 shell).
    Explosion { owner: u64, row: i32, twin: Twin, pos: V3 },
    /// The nuke marker bullet of a peer.
    NukeMarker { owner: u64, pos: V3 },
}

/// Muzzles of one owner + gun closer than this merge into one sound.
pub const MUZZLE_MERGE_S: f64 = 0.03;
/// A finished tracer stays this long (fading).
pub const TRACER_FADE_S: f64 = 0.15;
/// Never track more remote bullets than this.
pub const MAX_TRACKED: usize = 256;

#[derive(Clone, Copy, Debug)]
struct Tracked {
    handle: u64,
    owner: u64,
    row: i32,
    kind: RemoteKind,
    first: V3,
    last: V3,
    /// The twin's direction at its first read (its velocity), if it moved (review 2026-10-24: the tracer clip ray).
    dir: Option<V3>,
}

/// A tracer segment for the HUD: from `a` to `b`, `age` seconds after the bullet vanished (0 while it flies).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: V3,
    pub b: V3,
    pub age: f64,
    pub g: u32,
    /// Review 2026-10-24 (tracer occlusion): the twin's handle (stable for the tracer's life) and its shot direction.
    pub key: u64,
    pub dir: Option<V3>,
}

#[derive(Clone, Debug, Default)]
pub struct RemoteTracker {
    live: Vec<Tracked>,
    done: Vec<(Seg, f64)>,
    last_muzzle: Vec<(u64, u32, f64)>,
    /// Review C3: remote twins dropped because their owner stands in another havok frame (counted for the log).
    other_frame: u32,
}

impl RemoteTracker {
    pub fn clear(&mut self) {
        self.live.clear();
        self.done.clear();
        self.last_muzzle.clear();
    }

    pub fn tracked(&self) -> usize {
        self.live.len()
    }

    /// Twins dropped for another havok frame since the last call.
    pub fn take_other_frame(&mut self) -> u32 {
        std::mem::take(&mut self.other_frame)
    }

    /// One read of the bullet list (only this tick's remote twins matter; anything else is ignored). Review C3:
    /// `other_frame_owners` = peers standing in another havok frame (tile) than ours: their twins' positions are
    /// frame-relative, so they give no tracer / flash / sound here (the nuke marker still counts: it is global).
    pub fn update(&mut self, now: f64, bullets: &[RemoteBullet], local_owner: u64, other_frame_owners: &[u64], out: &mut Vec<FxEvent>) {
        let mut present: Vec<u64> = Vec::with_capacity(bullets.len().min(MAX_TRACKED));
        for b in bullets.iter().take(4 * MAX_TRACKED) {
            let Some(kind) = classify_remote(b, local_owner) else { continue };
            if matches!(kind, RemoteKind::Twin(_)) && other_frame_owners.contains(&b.owner) {
                if !self.live.iter().any(|t| t.handle == b.handle) {
                    self.other_frame = self.other_frame.saturating_add(1);
                }
                continue;
            }
            present.push(b.handle);
            if let Some(t) = self.live.iter_mut().find(|t| t.handle == b.handle && t.row == b.param_id && t.owner == b.owner) {
                t.last = b.pos;
                if t.dir.is_none() {
                    t.dir = b.vel.normalize();
                }
                continue;
            }
            // a handle reused by another row / owner: the old one ended
            if let Some(i) = self.live.iter().position(|t| t.handle == b.handle) {
                let t = self.live.remove(i);
                self.finish(now, t, out);
            }
            if self.live.len() >= MAX_TRACKED {
                continue;
            }
            match kind {
                RemoteKind::NukeMarker => out.push(FxEvent::NukeMarker { owner: b.owner, pos: b.pos }),
                RemoteKind::Twin(tw) if tw.kind.is_explosion() => out.push(FxEvent::Explosion { owner: b.owner, row: b.param_id, twin: tw, pos: b.pos }),
                RemoteKind::Twin(tw) => {
                    let sound = match self.last_muzzle.iter_mut().find(|(o, g, _)| *o == b.owner && *g == tw.g) {
                        Some((_, _, t)) if now - *t < MUZZLE_MERGE_S => false,
                        Some((_, _, t)) => {
                            *t = now;
                            true
                        }
                        None => {
                            self.last_muzzle.push((b.owner, tw.g, now));
                            true
                        }
                    };
                    let dir = b.vel.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
                    out.push(FxEvent::Muzzle { owner: b.owner, row: b.param_id, twin: tw, pos: b.pos, dir, sound });
                }
            }
            self.live.push(Tracked { handle: b.handle, owner: b.owner, row: b.param_id, kind, first: b.pos, last: b.pos, dir: b.vel.normalize() });
        }
        let mut i = 0;
        while i < self.live.len() {
            if present.contains(&self.live[i].handle) {
                i += 1;
            } else {
                let t = self.live.remove(i);
                self.finish(now, t, out);
            }
        }
        self.done.retain(|(_, t)| now - *t <= TRACER_FADE_S);
        self.last_muzzle.retain(|(_, _, t)| now - *t <= 5.0);
    }

    fn finish(&mut self, now: f64, t: Tracked, out: &mut Vec<FxEvent>) {
        if let RemoteKind::Twin(tw) = t.kind
            && !tw.kind.is_explosion()
        {
            out.push(FxEvent::Impact { owner: t.owner, row: t.row, twin: tw, pos: t.last, from: t.first });
            if self.done.len() < MAX_TRACKED && tw.kind == TwinKind::Gun {
                self.done.push((Seg { a: t.first, b: t.last, age: 0.0, g: tw.g, key: seg_key(t.handle, t.first), dir: t.dir }, now));
            }
        }
    }

    /// The tracer segments to draw now: live hitscan twins (first -> current) and finished ones while they fade.
    pub fn segments(&self, now: f64) -> Vec<Seg> {
        let mut v: Vec<Seg> = self
            .live
            .iter()
            .filter_map(|t| match t.kind {
                RemoteKind::Twin(tw) if tw.kind == TwinKind::Gun => Some(Seg { a: t.first, b: t.last, age: 0.0, g: tw.g, key: seg_key(t.handle, t.first), dir: t.dir }),
                _ => None,
            })
            .collect();
        v.extend(self.done.iter().filter(|(_, t)| now - *t <= TRACER_FADE_S).map(|(s, t)| Seg { age: (now - t).max(0.0), ..*s }));
        v
    }
}

/// A tracer's key for its whole life: the twin's handle mixed with its first position (a handle ER reuses for the
/// next twin while the old tracer still fades gets another key).
pub fn seg_key(handle: u64, first: V3) -> u64 {
    handle ^ ((first.x.to_bits() as u64) << 32 | first.z.to_bits() as u64).rotate_left(17) ^ (first.y.to_bits() as u64).rotate_left(41)
}

/// Tracer alpha: 1 while flying, then linear to 0 over [`TRACER_FADE_S`].
pub fn tracer_alpha(age: f64) -> f32 {
    (1.0 - age / TRACER_FADE_S).clamp(0.0, 1.0) as f32
}

/// Counters behind the `COOP peer fx ...` / `COOP twin seen ...` lines (reset every window).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FxStats {
    pub muzzle: u32,
    pub impact: u32,
    pub explosion: u32,
    pub sounds: u32,
    pub nuke_markers: u32,
}

impl FxStats {
    pub fn add(&mut self, e: &FxEvent) {
        match e {
            FxEvent::Muzzle { sound, .. } => {
                self.muzzle += 1;
                self.sounds += *sound as u32;
            }
            FxEvent::Impact { .. } => self.impact += 1,
            FxEvent::Explosion { .. } => {
                self.explosion += 1;
                self.sounds += 1;
            }
            FxEvent::NukeMarker { .. } => self.nuke_markers += 1,
        }
    }

    pub fn any(&self) -> bool {
        *self != FxStats::default()
    }
}

/// A peer's sound gain at `d_m`: 1 up to `min_m`, linear to 0 at `max_m` (with a small floor for far explosions).
pub fn peer_gain(d_m: f32, min_m: f32, max_m: f32, floor: f32) -> f32 {
    crate::ac130_audio::dist_gain(d_m, min_m, max_m).max(floor.clamp(0.0, 1.0))
}

// ---- markers ---------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerAct {
    Apply(i32),
    Remove(i32),
}

/// Our synced markers: the wanted set -> apply / refresh / remove. A wanted marker is (re-)applied when it is new,
/// when the read list no longer shows it (death, reload) and every `refresh_s` (the rows have a finite endurance, so a
/// marker the DLL could not remove expires on its own).
#[derive(Clone, Debug, Default)]
pub struct MarkerCtl {
    /// (id, last apply, when the entry was created in ER's list: C6 renewal).
    applied: Vec<(i32, f64, f64)>,
    /// Review C3: present markers we do not track (orphans of a reset / panic / world change): last remove time.
    orphan_t: Vec<(i32, f64)>,
}

/// C6 hide (work/c6/hide.md 5): a present marker is renewed (removed + applied in the same tick) this long before its
/// endurance ends: re-applying an id ER still lists does not reset its timer, so it dropped for <= 0.25 s every 30 s
/// (the hidden sword blinked back).
pub const MARKER_RENEW_MARGIN_S: f64 = 3.0;

impl MarkerCtl {
    pub fn applied(&self) -> Vec<i32> {
        self.applied.iter().map(|(i, _, _)| *i).collect()
    }

    /// Forget everything (world left: the character and its SpEffects are gone).
    pub fn clear(&mut self) {
        self.applied.clear();
        self.orphan_t.clear();
    }

    pub fn tick(&mut self, now: f64, want: &[i32], present: Option<&[i32]>, refresh_s: f64, out: &mut Vec<MarkerAct>) {
        self.tick_renew(now, want, present, refresh_s, 0.0, out);
    }

    /// [`MarkerCtl::tick`] plus the C6 renewal: with `renew_after_s > 0` a wanted marker still present `renew_after_s`
    /// after ER created its entry is removed and applied again in the same tick (a fresh endurance, no gap).
    pub fn tick_renew(&mut self, now: f64, want: &[i32], present: Option<&[i32]>, refresh_s: f64, renew_after_s: f64, out: &mut Vec<MarkerAct>) {
        // review C3: a C3 marker on our character that is neither wanted nor tracked (left behind by a reset, a world
        // change or the panic path) is removed too (at most once per ORPHAN_RETRY_S per id); otherwise it would stay
        // up to its 30 s endurance (ER's weapon hidden with MW2 mode off; a stale nuke marker re-triggering a peer)
        if let Some(p) = present {
            self.orphan_t.retain(|(id, _)| p.contains(id));
            for &id in p {
                if !syncable_speffect(id) || want.contains(&id) || self.applied.iter().any(|(x, _, _)| *x == id) {
                    continue;
                }
                match self.orphan_t.iter_mut().find(|(x, _)| *x == id) {
                    Some((_, t)) if now - *t < ORPHAN_RETRY_S => {}
                    Some((_, t)) => {
                        *t = now;
                        out.push(MarkerAct::Remove(id));
                    }
                    None => {
                        self.orphan_t.push((id, now));
                        out.push(MarkerAct::Remove(id));
                    }
                }
            }
        }
        let mut i = 0;
        while i < self.applied.len() {
            let id = self.applied[i].0;
            if want.contains(&id) {
                i += 1;
            } else {
                out.push(MarkerAct::Remove(id));
                self.applied.remove(i);
            }
        }
        for &id in want {
            if !syncable_speffect(id) {
                continue;
            }
            match self.applied.iter_mut().find(|(x, _, _)| *x == id) {
                None => {
                    out.push(MarkerAct::Apply(id));
                    self.applied.push((id, now, now));
                }
                Some((_, t, born)) => {
                    let gone = present.is_some_and(|p| !p.contains(&id));
                    let shown = present.is_some_and(|p| p.contains(&id));
                    if renew_after_s > 0.0 && shown && now - *born >= renew_after_s {
                        // C6: renew before the endurance ends (remove + apply in the same tick)
                        out.push(MarkerAct::Remove(id));
                        out.push(MarkerAct::Apply(id));
                        *t = now;
                        *born = now;
                    } else if (gone && now - *t >= 0.5) || now - *t >= refresh_s {
                        // a missing marker is re-applied at most twice per second; a present one every refresh_s
                        out.push(MarkerAct::Apply(id));
                        *t = now;
                        if !shown {
                            *born = now;
                        }
                    }
                }
            }
        }
    }
}

/// An orphan marker is removed again at most this often while it stays present.
pub const ORPHAN_RETRY_S: f64 = 1.0;

/// The C3 marker SpEffects in a list (the panic path removes every one of them).
pub fn c3_markers(ids: &[i32]) -> Vec<i32> {
    ids.iter().copied().filter(|id| syncable_speffect(*id)).collect()
}

/// The markers we want on our own character this tick.
pub fn wanted_markers(markers_on: bool, mode_on: bool, hide_er_weapon: bool, gun_g: Option<u32>, ac130: bool, nuke: bool) -> Vec<i32> {
    let mut v = Vec::new();
    if !markers_on {
        return v;
    }
    if mode_on && hide_er_weapon {
        v.push(SP_MODE);
    }
    if mode_on && let Some(id) = gun_g.and_then(gun_marker) {
        v.push(id);
    }
    if ac130 {
        v.push(SP_AC130);
    }
    if nuke {
        v.push(SP_NUKE);
    }
    v
}

/// The markers to apply this tick: everything [`wanted_markers`] gives with a peer present; solo only the MW2-mode
/// marker (it hides ER's own weapon) and only while `solo_hide` (C6 review: `hold_mode = "hand"` or the stance on, in
/// third person), else nothing (solo stays as before: no marker).
pub fn marker_scope(peers: bool, solo_hide: bool, all: Vec<i32>) -> Vec<i32> {
    if peers {
        all
    } else if solo_hide {
        all.into_iter().filter(|id| *id == SP_MODE).collect()
    } else {
        Vec::new()
    }
}

/// What a peer's SpEffect list says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerMarkers {
    pub mode: bool,
    pub gun_g: Option<u32>,
    pub ac130: bool,
    pub nuke: bool,
}

pub fn peer_markers(ids: &[i32]) -> PeerMarkers {
    let mut m = PeerMarkers::default();
    for &id in ids {
        if id == SP_MODE {
            m.mode = true;
        } else if id == SP_AC130 {
            m.ac130 = true;
        } else if id == SP_NUKE {
            m.nuke = true;
        } else if let Some(g) = gun_of_marker(id) {
            m.gun_g = Some(g);
        }
    }
    m
}

/// The nuke marker of a peer, seen by SpEffect or by bullet: one trigger per peer. Review C3: the entry lives at least
/// [`NUKE_DEDUPE_S`] from the first sighting AND until the peer's marker has been absent for [`NUKE_ABSENT_S`] (every
/// sighting refreshes it), so a marker that outlives the window (30 s endurance, refreshed until the caller's window
/// ends, orphaned by a reset) never starts a second remote nuke.
#[derive(Clone, Debug, Default)]
pub struct NukeMarkerRx {
    /// (peer, first sighting, last sighting)
    seen: Vec<(u64, f64, f64)>,
}

/// Longer than the marker endurance (30 s) + kill_at + coop_window_s (~19 s).
pub const NUKE_DEDUPE_S: f64 = 75.0;
pub const NUKE_ABSENT_S: f64 = 2.0;

impl NukeMarkerRx {
    /// True = a new nuke from `peer` (start the local sequence).
    pub fn seen(&mut self, now: f64, peer: u64) -> bool {
        self.seen.retain(|(_, first, last)| now - *first < NUKE_DEDUPE_S || now - *last < NUKE_ABSENT_S);
        if let Some(e) = self.seen.iter_mut().find(|(p, _, _)| *p == peer) {
            e.2 = now;
            return false;
        }
        self.seen.push((peer, now, now));
        true
    }
}

// ---- the peer-drop guard (3.7) ---------------------------------------------------------------------------------

/// An HP drop attributed to our shot that our shot cannot have dealt: in a session, more than `factor` x the row's
/// attack x pellets (a flat-damage row deals at most its atkPhys per hit). It gets no kill credit, no streak point and
/// no damage number (`HIT peer_drop`). C2: the friend's DLL credited the host's nuke kill of c2500 (4937) as an AK-47
/// hit (row attack ~150).
pub fn peer_drop(peers: u32, dmg: i32, row_atk: Option<i32>, pellets: u32, factor: f32) -> bool {
    let Some(atk) = row_atk.filter(|a| *a > 0) else { return false };
    peers > 0 && dmg as f64 > atk as f64 * pellets.max(1) as f64 * factor.max(1.0) as f64
}

// ---- world guns (stage A) --------------------------------------------------------------------------------------

/// Inches -> metres.
pub const IN_TO_M: f32 = 0.0254;

/// Model -> ER world (3x4) of a world gun: the MW2 model's tag_weapon origin at `feet + offset` (forward, right, up in
/// metres along the character's horizontal facing `fwd_h`), MW2 X along `fwd_h`, MW2 Y (left) along -right, MW2 Z
/// along +Y; scaled inches -> metres. MW2 is right-handed and ER's world left-handed (det of the camera rows +1), so
/// the map is a reflection, as the viewmodel's (`vm_proj::M_AXIS`): the same culling winding applies.
pub fn world_gun_matrix(feet: V3, fwd_h: V3, offset_m: [f32; 3], pitch_rad: f32) -> Option<Mat34> {
    let f = V3::new(fwd_h.x, 0.0, fwd_h.z).normalize()?;
    let up = V3::new(0.0, 1.0, 0.0);
    let right = up.cross(f);
    let anchor = feet + f * offset_m[0] + right * offset_m[1] + up * offset_m[2];
    // pitch about the right axis (positive = muzzle up)
    let (s, c) = pitch_rad.sin_cos();
    let fx = f * c + up * s;
    let ux = up * c - f * s;
    let left = -right;
    let k = IN_TO_M;
    if !anchor.is_finite() {
        return None;
    }
    // columns: MW2 X -> fx, MW2 Y -> left, MW2 Z -> ux
    Some([
        fx.x * k, left.x * k, ux.x * k, anchor.x,
        fx.y * k, left.y * k, ux.y * k, anchor.y,
        fx.z * k, left.z * k, ux.z * k, anchor.z,
    ])
}

/// ER world -> D3D view (rows = the camera's right, up, forward; left-handed like D3D) and the reversed-Z
/// perspective; `view_proj = proj * view`. Matches `ac130_hud::project` (same rows, vertical fov, aspect).
pub fn world_view_proj(cam_pos: [f32; 3], rows: [[f32; 3]; 3], vfov: f32, w: u32, h: u32, near: f32, far: f32) -> Option<(Mat4, Mat4)> {
    if !(vfov > 0.01 && vfov < 3.0) || w == 0 || h == 0 || !(near > 0.0 && far > near) {
        return None;
    }
    let d = |r: [f32; 3]| -(r[0] * cam_pos[0] + r[1] * cam_pos[1] + r[2] * cam_pos[2]);
    let view: Mat4 = [
        [rows[0][0], rows[0][1], rows[0][2], d(rows[0])],
        [rows[1][0], rows[1][1], rows[1][2], d(rows[1])],
        [rows[2][0], rows[2][1], rows[2][2], d(rows[2])],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let ty = (vfov * 0.5).tan();
    let tx = ty * w as f32 / h as f32;
    let proj = crate::vm_proj::proj_reversed(tx, ty, near, far);
    Some((view, m4_mul(&proj, &view)))
}

/// The LOD of a world gun at `dist_m` (`lod1_m` <= 0 = always lod0).
pub fn world_gun_lod(dist_m: f32, lod1_m: f32) -> u8 {
    if lod1_m > 0.0 && dist_m > lod1_m { 1 } else { 0 }
}

/// The gun a peer holds: its gun marker, else the gun of its last twin for `fallback_s` after it.
pub fn peer_gun(markers: &PeerMarkers, last_twin: Option<(u32, f64)>, now: f64, fallback_s: f64) -> Option<(u32, &'static str)> {
    if let Some(g) = markers.gun_g {
        return Some((g, "marker"));
    }
    match last_twin {
        Some((g, t)) if now - t <= fallback_s && g != 0 => Some((g, "twin")),
        _ => None,
    }
}

// ---- world guns stage B (review C3): the hand bone of ER's bone palette [I] -------------------------------------

/// The two translation candidates of a 12-float palette entry: A = 3 rows of 4 (m[3], m[7], m[11]), B = 4 rows of 3
/// (m[9], m[10], m[11]). Which layout ER uses is what `COOP bone probe` is for [I].
pub fn bone_candidates(m: &[f32; 12]) -> [V3; 2] {
    [V3::new(m[3], m[7], m[11]), V3::new(m[9], m[10], m[11])]
}

/// The world position of a hand bone: a candidate within `max_m` of the feet is taken as world space; else one within
/// `max_m` of the origin as model-local (`rot` = the body orientation, added to the feet). `None` = neither is plausible
/// (the caller falls back to stage A and logs it).
pub fn bone_anchor(feet: V3, rot: impl Fn(V3) -> V3, m: &[f32; 12], max_m: f32) -> Option<(V3, &'static str)> {
    let c = bone_candidates(m);
    let near = |v: V3, o: V3| v.is_finite() && (v - o).len() <= max_m;
    if near(c[0], feet) && feet.len() > max_m * 2.0 {
        return Some((c[0], "world_a"));
    }
    if near(c[1], feet) && feet.len() > max_m * 2.0 {
        return Some((c[1], "world_b"));
    }
    for (k, v) in c.iter().enumerate() {
        if near(*v, V3::ZERO) && v.len() > 0.05 {
            let w = feet + rot(*v);
            if w.is_finite() {
                return Some((w, if k == 0 { "local_a" } else { "local_b" }));
            }
        }
    }
    None
}

/// The muzzle pitch (rad) of an aim direction (ER world, Y up), clamped to +-0.8 rad.
pub fn aim_pitch(dir: V3) -> f32 {
    dir.y.clamp(-1.0, 1.0).asin().clamp(-0.8, 0.8)
}

/// A peer's last shot direction counts as its aim for this long (s); else the body facing.
pub const AIM_HOLD_S: f64 = 1.5;

/// One `COOP bone probe` chunk: `i:ax,ay,az|bx,by,bz` for the bones `from..to` (2 decimals).
pub fn bone_probe_chunk(entries: &[[f32; 12]], from: usize, to: usize) -> String {
    let mut v = Vec::new();
    for (i, m) in entries.iter().enumerate().take(to.min(entries.len())).skip(from) {
        let c = bone_candidates(m);
        v.push(format!("{i}:{:.2},{:.2},{:.2}|{:.2},{:.2},{:.2}", c[0].x, c[0].y, c[0].z, c[1].x, c[1].y, c[1].z));
    }
    v.join(" ")
}

/// Occlusion of a world gun from 3 ray results (camera -> muzzle / stock / centre; `true` = blocked): drawn unless
/// every ray is blocked (binary; C3 has no depth from ER).
pub fn occluded(blocked: [bool; 3]) -> bool {
    blocked.iter().all(|b| *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xform::{m_point, m4_apply};

    #[test]
    fn c3_twin_ids_round_trip() {
        // hitscan: every band maps to the gun's one twin; launcher launch / tiers; AC-130 tiers
        assert_eq!(twin_of(79_112_000), Some(79_812_000));
        assert_eq!(twin_of(79_112_007), Some(79_812_000));
        assert_eq!(twin_of(79_153_500), Some(79_853_500));
        assert_eq!(twin_of(79_153_501), None, "the launcher marker gets no twin");
        assert_eq!(twin_of(79_153_601), Some(79_853_601));
        assert_eq!(twin_of(79_153_603), Some(79_853_603));
        assert_eq!(twin_of(79_000_201), Some(79_800_201));
        assert_eq!(twin_of(79_000_401), Some(79_800_401));
        assert_eq!(twin_of(79_000_900), None, "the impact marker");
        assert_eq!(twin_of(79_000_960), None, "the lethal nuke rows never get a twin");
        assert_eq!(twin_of(79_000_961), None);
        assert_eq!(twin_of(79_110_000), None, "g 10 is not a gun");
        for g in GUN_G {
            let t = twin_of(M6_BASE + g as i32 * 1000 + 3).unwrap();
            assert_eq!(decode_twin(t), Some(Twin { kind: TwinKind::Gun, g }));
            assert_eq!(twin_src(t), Some(M6_BASE + g as i32 * 1000));
            assert!(syncable_bullet(t));
            assert!(!syncable_bullet(M6_BASE + g as i32 * 1000), "a damaging row is never syncable");
        }
        assert_eq!(decode_twin(79_853_602), Some(Twin { kind: TwinKind::Expl(2), g: 53 }));
        assert_eq!(decode_twin(79_800_302), Some(Twin { kind: TwinKind::Ac130 { gun: 1, tier: 2 }, g: 0 }));
        assert_eq!(twin_src(79_800_302), Some(79_000_302));
        assert_eq!(decode_twin(79_800_500), None);
        assert_eq!(decode_twin(79_812_100), None);
        assert!(syncable_bullet(NUKE_MARKER_BULLET));
        assert!(!syncable_bullet(79_000_961) && !syncable_bullet(79_000_900) && !syncable_bullet(79_000_201));
        // every twin id is inside the reserved twin range
        for g in GUN_G {
            for src in [0, 500, 601, 602, 603] {
                if let Some(t) = twin_of(M6_BASE + g as i32 * 1000 + src) {
                    assert!((TWIN_BASE..=TWIN_MAX).contains(&t));
                }
            }
        }
    }

    #[test]
    fn c3_twin_rate_limiter_and_peers() {
        let mut l = TwinLimiter::default();
        let mut n = 0;
        // an SMG at 900 rpm (66.7 ms) and a 40 ms limit: every shot; at 2000 rpm (30 ms): at most 25/s
        for i in 0..100 {
            if l.allow(i as f64 * 0.030, 13, 0.040) {
                n += 1;
            }
        }
        assert!((49..=51).contains(&n), "{n} twins in 3 s at 30 ms");
        assert!(l.allow(0.0, 12, 0.040), "another gun has its own clock");
        assert_eq!(twin_wanted(0, true, 79_112_000), None, "solo: no twin");
        assert_eq!(twin_wanted(1, false, 79_112_000), None, "visual_twins off");
        assert_eq!(twin_wanted(1, true, 79_112_004), Some(79_812_000));
    }

    #[test]
    fn c3_twin_pellets_one_twin_per_shot() {
        // the shotgun sends one twin for the shot (the centre pellet), never one per pellet: the limiter drops the rest
        let mut l = TwinLimiter::default();
        let sent = (0..8).filter(|_| l.allow(1.0, 41, 0.040)).count();
        assert_eq!(sent, 1);
    }

    fn rb(handle: u64, owner: u64, id: i32, pos: V3) -> RemoteBullet {
        RemoteBullet { handle, owner, param_id: id, pos, vel: V3::new(0.0, 0.0, 999.0) }
    }

    /// Review 2026-10-24 (tracer occlusion): a segment carries its shot direction (the twin's velocity, filled later
    /// when the first read had none) and a key that stays the same from flight to fade; a handle reused while the old
    /// tracer fades gets another key.
    #[test]
    fn c3_tracer_seg_key_and_dir() {
        let mut t = RemoteTracker::default();
        let mut ev = Vec::new();
        let still = RemoteBullet { vel: V3::ZERO, ..rb(7, 0x20, 79_812_000, V3::new(0.0, 1.0, 1.0)) };
        t.update(1.0, &[still], 0x10, &[], &mut ev);
        assert_eq!(t.segments(1.0)[0].dir, None);
        t.update(1.016, &[rb(7, 0x20, 79_812_000, V3::new(0.0, 1.0, 17.0))], 0x10, &[], &mut ev);
        let live = t.segments(1.016)[0];
        assert_eq!(live.dir, Some(V3::new(0.0, 0.0, 1.0)));
        t.update(1.033, &[rb(7, 0x20, 79_812_000, V3::new(5.0, 1.0, 1.0))], 0x10, &[], &mut ev);
        // handle 7 is the same row / owner: the tracker keeps it as one twin; a new owner on 7 = a new twin
        t.update(1.050, &[rb(7, 0x30, 79_812_000, V3::new(5.0, 1.0, 1.0))], 0x10, &[], &mut ev);
        let s = t.segments(1.050);
        assert_eq!(s.len(), 2, "{s:?}");
        let faded = s.iter().find(|x| x.age > 0.0 || x.a == V3::new(0.0, 1.0, 1.0)).unwrap();
        let fresh = s.iter().find(|x| x.a == V3::new(5.0, 1.0, 1.0)).unwrap();
        assert_eq!(faded.key, live.key, "flight -> fade keeps the key");
        assert_ne!(faded.key, fresh.key, "a reused handle is another tracer");
    }

    #[test]
    fn c3_remote_classify() {
        let me = 0x10;
        assert_eq!(classify_remote(&rb(1, 0x20, 79_812_000, V3::ZERO), me), Some(RemoteKind::Twin(Twin { kind: TwinKind::Gun, g: 12 })));
        assert_eq!(classify_remote(&rb(1, me, 79_812_000, V3::ZERO), me), None, "our own twin");
        assert_eq!(classify_remote(&rb(1, 0x20, 79_112_000, V3::ZERO), me), None, "a damaging row is never a peer effect");
        assert_eq!(classify_remote(&rb(1, 0, 79_812_000, V3::ZERO), me), None, "no owner");
        assert_eq!(classify_remote(&rb(1, 0x20, NUKE_MARKER_BULLET, V3::ZERO), me), Some(RemoteKind::NukeMarker));
        assert_eq!(classify_remote(&rb(1, 0x20, 10_170_000, V3::ZERO), me), None, "a vanilla bullet");
    }

    #[test]
    fn c3_remote_muzzle_impact_from_frames() {
        let mut t = RemoteTracker::default();
        let mut ev = Vec::new();
        let p = 0x20;
        // frame 1: a hitscan twin appears -> muzzle (with sound)
        t.update(1.0, &[rb(7, p, 79_812_000, V3::new(0.0, 1.0, 1.0))], 0x10, &[], &mut ev);
        assert!(matches!(ev[0], FxEvent::Muzzle { owner: 0x20, sound: true, twin: Twin { kind: TwinKind::Gun, g: 12 }, .. }));
        ev.clear();
        // frame 2: it moved (no event), a tracer from first to current
        t.update(1.016, &[rb(7, p, 79_812_000, V3::new(0.0, 1.0, 17.0))], 0x10, &[], &mut ev);
        assert!(ev.is_empty());
        let s = t.segments(1.016);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].a, s[0].b, s[0].age), (V3::new(0.0, 1.0, 1.0), V3::new(0.0, 1.0, 17.0), 0.0));
        // frame 3: gone -> impact at the last position; the tracer fades
        t.update(1.033, &[], 0x10, &[], &mut ev);
        assert!(matches!(ev[0], FxEvent::Impact { pos, from, .. } if pos == V3::new(0.0, 1.0, 17.0) && from == V3::new(0.0, 1.0, 1.0)));
        let s = t.segments(1.10);
        assert_eq!(s.len(), 1);
        assert!(tracer_alpha(s[0].age) < 1.0 && tracer_alpha(s[0].age) > 0.0);
        assert!(t.segments(1.033 + TRACER_FADE_S + 0.01).is_empty(), "faded out");
        // an explosion twin: one event at its first sighting, no impact when it vanishes
        ev.clear();
        t.update(2.0, &[rb(9, p, 79_800_201, V3::new(5.0, 0.0, 5.0))], 0x10, &[], &mut ev);
        t.update(2.1, &[], 0x10, &[], &mut ev);
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0], FxEvent::Explosion { twin: Twin { kind: TwinKind::Ac130 { gun: 0, tier: 1 }, .. }, .. }));
        // the nuke marker bullet
        ev.clear();
        t.update(3.0, &[rb(11, p, NUKE_MARKER_BULLET, V3::ZERO)], 0x10, &[], &mut ev);
        assert!(matches!(ev[0], FxEvent::NukeMarker { owner: 0x20, .. }));
    }

    #[test]
    fn c3_remote_pellet_grouping_and_handle_reuse() {
        let mut t = RemoteTracker::default();
        let mut ev = Vec::new();
        // three twins of one owner + gun in one frame (a burst / a mis-sent pellet set): one sound
        t.update(1.0, &[rb(1, 0x20, 79_841_000, V3::ZERO), rb(2, 0x20, 79_841_000, V3::ZERO), rb(3, 0x20, 79_841_000, V3::ZERO)], 0x10, &[], &mut ev);
        let sounds = ev.iter().filter(|e| matches!(e, FxEvent::Muzzle { sound: true, .. })).count();
        assert_eq!((ev.len(), sounds), (3, 1));
        // another owner in the same frame has its own sound
        ev.clear();
        t.update(1.001, &[rb(1, 0x20, 79_841_000, V3::ZERO), rb(4, 0x30, 79_841_000, V3::ZERO)], 0x10, &[], &mut ev);
        assert!(ev.iter().any(|e| matches!(e, FxEvent::Muzzle { owner: 0x30, sound: true, .. })));
        // a handle reused by another row: the old bullet ends (impact), the new one starts (muzzle)
        ev.clear();
        t.update(2.0, &[rb(1, 0x20, 79_812_000, V3::ZERO)], 0x10, &[], &mut ev);
        assert!(ev.iter().any(|e| matches!(e, FxEvent::Impact { row: 79_841_000, .. })));
        assert!(ev.iter().any(|e| matches!(e, FxEvent::Muzzle { row: 79_812_000, .. })));
        let mut st = FxStats::default();
        for e in &ev {
            st.add(e);
        }
        assert!(st.any() && st.muzzle >= 1 && st.impact >= 1);
    }

    #[test]
    fn c3_marker_apply_refresh_remove() {
        let mut m = MarkerCtl::default();
        let mut out = Vec::new();
        let want = wanted_markers(true, true, true, Some(12), false, false);
        assert_eq!(want, vec![SP_MODE, gun_marker(12).unwrap()]);
        m.tick(0.0, &want, Some(&[]), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Apply(SP_MODE), MarkerAct::Apply(SP_GUN_BASE + 10)]);
        out.clear();
        // present and fresh: nothing
        m.tick(1.0, &want, Some(&want), 5.0, &mut out);
        assert!(out.is_empty());
        // refresh every 5 s
        m.tick(5.0, &want, Some(&want), 5.0, &mut out);
        assert_eq!(out.len(), 2);
        out.clear();
        // gone from the list (death): re-applied
        m.tick(6.0, &want, Some(&[SP_MODE]), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Apply(SP_GUN_BASE + 10)]);
        out.clear();
        // switch: the old gun marker is removed, the new one applied
        let want2 = wanted_markers(true, true, true, Some(9), false, false);
        m.tick(7.0, &want2, Some(&want), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_GUN_BASE + 10), MarkerAct::Apply(gun_marker(9).unwrap())]);
        out.clear();
        // mode off: everything removed
        m.tick(8.0, &wanted_markers(true, false, true, Some(9), false, false), None, 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_MODE), MarkerAct::Remove(gun_marker(9).unwrap())]);
        assert!(m.applied().is_empty());
        // never anything outside the synced range (the lethal nuke SpEffect is refused)
        out.clear();
        m.tick(9.0, &[NUKE_LETHAL_SP, 79_000_950], None, 5.0, &mut out);
        assert!(out.is_empty());
        assert!(!syncable_speffect(NUKE_LETHAL_SP) && !syncable_speffect(79_000_950));
        assert!(wanted_markers(false, true, true, Some(12), true, true).is_empty(), "markers off");
    }

    #[test]
    fn c6_hide_fields_are_unseen_blade() {
        // inside the row, ascending, no overlap; the packaged values are the inert template's zeros
        let mut end = 0;
        for f in HIDE_FIELDS {
            assert!(f.off >= end && f.off + f.width <= SP_ROW_SIZE && matches!(f.width, 1 | 2 | 4), "{f:?}");
            assert!(f.width == 4 || f.to < 1 << (8 * f.width), "{f:?}");
            end = f.off + f.width;
            assert_eq!(f.from, 0, "{} is 0 in the package", f.name);
        }
        let get = |n: &str| HIDE_FIELDS.iter().find(|f| f.name == n).copied().unwrap();
        assert_eq!((get("stateInfo").off, get("stateInfo").to), (OFF_SP_STATE_INFO, STATE_INFO_HIDE_WEAPON));
        assert_eq!(f32::from_bits(get("motionInterval").to), -1.0);
        let rates = HIDE_FIELDS.iter().filter(|f| f.name.ends_with("Rate")).count();
        assert_eq!(rates, 7);
        assert!(HIDE_FIELDS.iter().filter(|f| f.name.ends_with("Rate")).all(|f| f32::from_bits(f.to) == 1.0), "neutral rates once the row is live");
        assert!(!HIDE_FIELDS.iter().any(|f| f.name == "spCategory" || f.name == "vfxId" || f.name == "effectEndurance"));
        // the row state
        let from: Vec<u32> = HIDE_FIELDS.iter().map(|f| f.from).collect();
        let to: Vec<u32> = HIDE_FIELDS.iter().map(|f| f.to).collect();
        assert_eq!(hide_row_state(&from), HideRow::Package);
        assert_eq!(hide_row_state(&to), HideRow::Patched);
        let mut mixed = from.clone();
        mixed[8] = STATE_INFO_HIDE_WEAPON;
        assert_eq!(hide_row_state(&mixed), HideRow::Foreign);
        assert_eq!(hide_row_state(&from[..3]), HideRow::Foreign);
    }

    #[test]
    fn c6_marker_renewed_before_it_expires() {
        let mut m = MarkerCtl::default();
        let mut out = Vec::new();
        let want = [SP_MODE];
        let renew = 30.0 - MARKER_RENEW_MARGIN_S;
        m.tick_renew(0.0, &want, Some(&[]), 5.0, renew, &mut out);
        assert_eq!(out, vec![MarkerAct::Apply(SP_MODE)]);
        // the 5 s refreshes do not move the creation time (ER keeps the old timer)
        let mut t = 0.25;
        let mut acts = Vec::new();
        while t < 26.9 {
            out.clear();
            m.tick_renew(t, &want, Some(&want), 5.0, renew, &mut out);
            acts.extend(out.iter().copied().filter(|a| matches!(a, MarkerAct::Remove(_))));
            t += 0.25;
        }
        assert!(acts.is_empty(), "no remove before the renewal");
        out.clear();
        m.tick_renew(27.0, &want, Some(&want), 5.0, renew, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_MODE), MarkerAct::Apply(SP_MODE)], "renewed with no gap, 3 s before expiry");
        out.clear();
        m.tick_renew(27.25, &want, Some(&want), 5.0, renew, &mut out);
        assert!(out.is_empty());
        // a marker that vanished (death) is re-applied and its timer restarts
        out.clear();
        m.tick_renew(40.0, &want, Some(&[]), 5.0, renew, &mut out);
        assert_eq!(out, vec![MarkerAct::Apply(SP_MODE)]);
        out.clear();
        m.tick_renew(66.9, &want, Some(&want), 5.0, renew, &mut out);
        assert!(!out.contains(&MarkerAct::Remove(SP_MODE)), "26.9 s after the re-apply: not yet");
        out.clear();
        m.tick_renew(67.0, &want, Some(&want), 5.0, renew, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_MODE), MarkerAct::Apply(SP_MODE)]);
        // renewal off (0): today's behaviour, no remove ever
        let mut z = MarkerCtl::default();
        let mut all = Vec::new();
        for k in 0..400 {
            z.tick(k as f64 * 0.25, &want, Some(if k == 0 { &[] } else { &want }), 5.0, &mut all);
        }
        assert!(!all.iter().any(|a| matches!(a, MarkerAct::Remove(_))));
    }

    #[test]
    fn c6_marker_scope_solo_hides_er_weapon_only() {
        let all = wanted_markers(true, true, true, Some(12), true, false);
        assert_eq!(marker_scope(true, false, all.clone()), all, "with a peer: everything");
        assert_eq!(marker_scope(false, true, all.clone()), vec![SP_MODE], "solo + hand / stance: the hide marker only");
        assert!(marker_scope(false, false, all).is_empty(), "solo defaults: nothing (unchanged)");
        // hide_er_weapon off or mode off: no hide marker solo either
        assert!(marker_scope(false, true, wanted_markers(true, true, false, Some(12), false, false)).is_empty());
        assert!(marker_scope(false, true, wanted_markers(true, false, true, None, false, false)).is_empty());
    }

    #[test]
    fn c3_marker_peer_read_and_gun_ids() {
        let m = peer_markers(&[1540, SP_MODE, gun_marker(53).unwrap(), SP_NUKE]);
        assert_eq!(m, PeerMarkers { mode: true, gun_g: Some(53), ac130: false, nuke: true });
        assert_eq!(peer_markers(&[]), PeerMarkers::default());
        for (k, g) in GUN_G.iter().enumerate() {
            assert_eq!(gun_marker(*g), Some(SP_GUN_BASE + k as i32));
            assert_eq!(gun_of_marker(SP_GUN_BASE + k as i32), Some(*g));
            assert!(SP_GUN_BASE + (k as i32) < SP_AC130);
        }
        assert_eq!(gun_marker(10), None);
        assert_eq!(gun_of_marker(SP_AC130), None);
        assert_eq!(peer_gun(&m, None, 0.0, 3.0), Some((53, "marker")));
        assert_eq!(peer_gun(&PeerMarkers::default(), Some((12, 9.0)), 10.0, 3.0), Some((12, "twin")));
        assert_eq!(peer_gun(&PeerMarkers::default(), Some((12, 5.0)), 10.0, 3.0), None);
        // nuke marker dedupe per peer
        let mut rx = NukeMarkerRx::default();
        assert!(rx.seen(0.0, 0x20));
        assert!(!rx.seen(1.0, 0x20), "the SpEffect and the bullet of one nuke");
        assert!(rx.seen(1.0, 0x30));
        assert!(rx.seen(NUKE_DEDUPE_S + 1.0, 0x20), "a later nuke (the marker was absent)");
    }

    /// Review C3: a marker still present past the dedupe window (orphaned, 30 s endurance + refreshes) never starts a
    /// second remote nuke; after it has been gone for NUKE_ABSENT_S a later nuke does.
    #[test]
    fn c3_nukerx_persistent_marker_never_retriggers() {
        let mut rx = NukeMarkerRx::default();
        assert!(rx.seen(0.0, 0x20));
        let mut t = 0.25;
        while t < 200.0 {
            assert!(!rx.seen(t, 0x20), "retriggered at {t}");
            t += 0.25;
        }
        // gone for 3 s, then a new marker
        assert!(rx.seen(203.0, 0x20));
        // mutation guard: the old 30 s first-sight window would retrigger a 30 s marker refreshed for ~19 s
        assert!(NUKE_DEDUPE_S > 30.0 + 19.0);
    }

    #[test]
    fn c3_marker_orphans_removed() {
        let mut m = MarkerCtl::default();
        let mut out = Vec::new();
        // after a reset nothing is tracked, but our list still shows the MW2-mode and nuke markers; mode is off now
        m.tick(0.0, &[], Some(&[1540, SP_MODE, SP_NUKE]), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_MODE), MarkerAct::Remove(SP_NUKE)]);
        out.clear();
        // still present 0.5 s later: not spammed; after ORPHAN_RETRY_S again
        m.tick(0.5, &[], Some(&[SP_MODE]), 5.0, &mut out);
        assert!(out.is_empty());
        m.tick(1.1, &[], Some(&[SP_MODE]), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Remove(SP_MODE)]);
        out.clear();
        // a wanted marker that is present but untracked is applied (tracked), never removed
        m.tick(2.0, &[SP_MODE], Some(&[SP_MODE]), 5.0, &mut out);
        assert_eq!(out, vec![MarkerAct::Apply(SP_MODE)]);
        // the lethal nuke SpEffect / vanilla ids are never touched
        out.clear();
        m.tick(3.0, &[SP_MODE], Some(&[SP_MODE, NUKE_LETHAL_SP, 1540]), 5.0, &mut out);
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(c3_markers(&[1540, SP_MODE, NUKE_LETHAL_SP, SP_GUN_BASE]), vec![SP_MODE, SP_GUN_BASE]);
    }

    #[test]
    fn c3_remote_other_frame_owner_dropped() {
        let mut t = RemoteTracker::default();
        let mut ev = Vec::new();
        t.update(1.0, &[rb(1, 0x20, 79_812_000, V3::ZERO), rb(2, 0x30, 79_812_000, V3::ZERO), rb(3, 0x20, NUKE_MARKER_BULLET, V3::ZERO)], 0x10, &[0x20], &mut ev);
        assert!(ev.iter().all(|e| !matches!(e, FxEvent::Muzzle { owner: 0x20, .. })), "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, FxEvent::Muzzle { owner: 0x30, .. })));
        assert!(ev.iter().any(|e| matches!(e, FxEvent::NukeMarker { owner: 0x20, .. })), "the nuke marker is global");
        assert_eq!(t.take_other_frame(), 1);
        assert_eq!(t.take_other_frame(), 0);
    }

    #[test]
    fn c3_worldgun_bone_anchor_and_aim() {
        let feet = V3::new(100.0, 10.0, -200.0);
        let id = |v: V3| v;
        // world-space translation (layout A) next to the feet
        let mut m = [0.0f32; 12];
        m[3] = 100.3;
        m[7] = 11.2;
        m[11] = -199.8;
        assert_eq!(bone_anchor(feet, id, &m, 2.5).map(|x| x.1), Some("world_a"));
        // model-local (layout B) near the origin: rotated and added to the feet
        let mut m = [0.0f32; 12];
        m[3] = 50.0; // layout A implausible (m[11] is shared by both layouts)
        m[9] = 0.3;
        m[10] = 1.2;
        m[11] = 0.2;
        let (p, src) = bone_anchor(feet, id, &m, 2.5).unwrap();
        assert_eq!(src, "local_b");
        assert!((p - V3::new(100.3, 11.2, -199.8)).len() < 1e-4);
        // garbage: stage A fallback
        let mut m = [0.0f32; 12];
        m[3] = 5000.0;
        m[7] = 5000.0;
        m[11] = 5000.0;
        m[9] = 900.0;
        m[10] = 900.0;
        assert!(bone_anchor(feet, id, &m, 2.5).is_none());
        // aim: level, up, clamped
        assert!(aim_pitch(V3::new(0.0, 0.0, 1.0)).abs() < 1e-6);
        assert!(aim_pitch(V3::new(0.0, 0.5, 0.866)) > 0.5);
        assert!((aim_pitch(V3::new(0.0, 1.0, 0.0)) - 0.8).abs() < 1e-6);
        let e = [[1.0f32; 12], [2.0f32; 12]];
        assert_eq!(bone_probe_chunk(&e, 1, 9), "1:2.00,2.00,2.00|2.00,2.00,2.00");
    }

    #[test]
    fn c3_peerdrop_guard() {
        // the C2 case: AK-47 row attack ~150, one pellet: the host's nuke kill (4937) is a peer drop
        assert!(peer_drop(1, 4937, Some(150), 1, 1.5));
        assert!(!peer_drop(1, 35, Some(150), 1, 1.5), "a normal hit");
        assert!(!peer_drop(0, 4937, Some(150), 1, 1.5), "solo: never");
        // a shotgun shot: 8 pellets of 40 can deal 320
        assert!(!peer_drop(1, 300, Some(40), 8, 1.5));
        assert!(peer_drop(1, 2000, Some(40), 8, 1.5));
        assert!(!peer_drop(1, 9999, None, 1, 1.5), "unknown row: no decision");
        // the factor never goes below 1
        assert!(!peer_drop(1, 150, Some(150), 1, 0.5));
    }

    #[test]
    fn c3_worldgun_projects_like_the_hud() {
        let rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let cam = [0.0, 1.6, 0.0];
        let (_view, vp) = world_view_proj(cam, rows, 0.8, 1920, 1080, 0.05, 2000.0).unwrap();
        let feet = V3::new(0.5, 0.0, 6.0);
        let m = world_gun_matrix(feet, V3::new(0.0, 0.0, 1.0), [0.3, 0.2, 1.2], 0.0).unwrap();
        // the model origin lands on the anchor; 10 in along MW2 X is 0.254 m forward; MW2 Y (left) is -right
        let o = m_point(&m, &[0.0, 0.0, 0.0]);
        assert!((o[0] - 0.7).abs() < 1e-5 && (o[1] - 1.2).abs() < 1e-5 && (o[2] - 6.3).abs() < 1e-5, "{o:?}");
        let x = m_point(&m, &[10.0, 0.0, 0.0]);
        assert!((x[2] - (6.3 + 0.254)).abs() < 1e-4);
        let l = m_point(&m, &[0.0, 10.0, 0.0]);
        assert!(l[0] < o[0], "MW2 left is ER -right");
        // the projection equals the HUD's
        let c = m4_apply(&vp, &o);
        let ndc = [c[0] / c[3], c[1] / c[3]];
        let px = [(ndc[0] * 0.5 + 0.5) * 1920.0, (0.5 - ndc[1] * 0.5) * 1080.0];
        let hud = crate::ac130_hud::project(cam, rows, 0.8, 1920.0, 1080.0, o).unwrap();
        assert!((px[0] - hud[0]).abs() < 0.05 && (px[1] - hud[1]).abs() < 0.05, "{px:?} {hud:?}");
        // reversed depth: nearer is larger
        let far = m4_apply(&vp, &[0.0, 1.6, 50.0]);
        assert!(c[2] / c[3] > far[2] / far[3]);
        // a reflection (det < 0), like the viewmodel's axis map
        let det = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8]) + m[2] * (m[4] * m[9] - m[5] * m[8]);
        assert!(det < 0.0);
        // pitch up raises the muzzle
        let mp = world_gun_matrix(feet, V3::new(0.0, 0.0, 1.0), [0.3, 0.2, 1.2], 0.3).unwrap();
        assert!(m_point(&mp, &[20.0, 0.0, 0.0])[1] > m_point(&m, &[20.0, 0.0, 0.0])[1]);
        assert!(world_gun_matrix(feet, V3::new(0.0, 1.0, 0.0), [0.0; 3], 0.0).is_none(), "no horizontal facing");
        assert!(world_view_proj(cam, rows, 0.0, 1920, 1080, 0.05, 2000.0).is_none());
        assert_eq!((world_gun_lod(5.0, 10.0), world_gun_lod(12.0, 10.0), world_gun_lod(50.0, 0.0)), (0, 1, 0));
        assert!(occluded([true, true, true]) && !occluded([true, false, true]));
        assert!(peer_gain(5.0, 10.0, 100.0, 0.0) == 1.0 && peer_gain(200.0, 10.0, 100.0, 0.1) == 0.1);
    }
}
