//! C3 PASS C (docs/C3_DESIGN.md part 3): seeing each other in co-op. Runs on the game's main thread inside the task,
//! after the peer watch (`coop.rs`) and before the nuke (it hands the nuke a peer's marker).
//!
//! - **Twins out** ([`CoopVis::send_twin`]): beside a damaging spawn (which never gets bit 3) a no-attack twin row is
//!   spawned through `spawn::spawn_synced` (bit 3, ER sends it to the peers), only while a peer is in the session,
//!   rate-limited per gun, lowest priority in the emitter watchdog. `COOP twin sent ...` every `log_every_s`.
//! - **Peer effects in**: the peers' twins in our bullet list (owner not us) become muzzle / impact / explosion events
//!   (`mw2er_core::coop_vis::RemoteTracker`): the gun's third-person sound at its position, a tracer, an explosion
//!   sound + flash. Nothing here touches damage. `COOP twin seen ...`, `COOP peer fx ...`.
//! - **Markers**: our synced marker SpEffects (MW2 mode = ER's weapon hidden, the gun in hand, the AC-130 ride, the nuke)
//!   applied / refreshed / removed through `ac130::apply_speffect_synced` / `remove_speffect`; the peers' lists are read
//!   at 4 Hz (`COOP marker sent / seen ...`). A peer's nuke marker (SpEffect or bullet) is handed to the nuke.
//! - **World guns** (stage A): for every peer with a known gun (and ourselves in third person) the MW2 world model is
//!   placed on the character's model frame and published for the renderer; hidden when three map rays from the camera
//!   are all blocked. `COOP worldgun ...` on every change.
//! Reads are non-faulting; the only engine calls are the gated spawn / SpEffect wrappers.

use std::sync::Arc;

use mw2er_core::config::{Config, YawAxisCfg};
use mw2er_core::coop::PeerSlot;
use mw2er_core::coop_vis::{
    AIM_HOLD_S, FxEvent, FxStats, HIDE_FIELDS, HideRow, MARKER_RENEW_MARGIN_S, MarkerAct, MarkerCtl, NukeMarkerRx, PeerMarkers, RemoteTracker, SP_MODE, Twin,
    TwinKind, TwinLimiter, aim_pitch, bone_anchor, c3_markers, decode_twin, hide_row_state, marker_scope, peer_gain, peer_gun, peer_markers, tracer_alpha,
    twin_wanted, wanted_markers, world_gun_lod, world_gun_matrix,
};
use mw2er_core::emitter_guard::Kind as SpawnKind;
use mw2er_core::tracer_occl::{CLIP_RANGE_M, FLASH_TOL_M, PathIn, TracerOccl, point_visible};
use mw2er_core::fp_math::{Axis, forward_of, horizontal_forward, rotate};
use mw2er_core::hold::{
    AxesVote, DEFAULT_HAND_BONE, FallbackLog, FbEvent, HAND_BONE_NAMES, HAND_MAX_M, HandIn, HandOk, HandTrack, Reason, StaleGuard, WEAPON_AXES_RIG, WEAPON_NEAR_M,
    WeaponIn, ads_blend_step, axes_auto_accept, blend_mat34, floats_text, grip_matrix, gun_from_hand, hand_axes_world, learn_grip, match_flver_any, parse_axes,
    probe_chunk, reasons_text, translation, v3_text, validate_hand, weapon_world,
};
use mw2er_core::body_occl::{self, BODY_SLOTS, Capsule};
use mw2er_core::logfmt;
use mw2er_core::vec3::V3;
use mw2er_core::xform::{Mat34, m_mul, m_point};

use crate::arsenal_rt::ArsenalData;
use crate::game::ac130_read::{main_player, speffect_ids};
use crate::game::player::PlayerSample;
use crate::game::{bullets, havok};
use crate::log::{log_i, log_w};
use crate::{shared, spawn, spawn_guard};

/// The peers' SpEffect lists are read this often.
const MARKER_READ_S: f64 = 0.25;
/// A world gun line per peer at most this often (plus on every gun / source change).
const WORLDGUN_LOG_S: f64 = 30.0;
/// A peer explosion flash lasts this long.
const FLASH_S: f64 = 0.35;
/// Review C3 stage B: the bone probe runs this long after MW2 mode turns on, a sample every BONE_PROBE_EVERY_S.
const BONE_PROBE_S: f64 = 10.0;
const BONE_PROBE_EVERY_S: f64 = 2.0;
/// A hand bone farther than this from the feet (world) / the origin (local) is not plausible (stage A fallback).
const BONE_MAX_M: f32 = 2.5;
/// C6: the `HOLD axes` lines: every AXES_EVERY_S for AXES_S after hand mode first succeeds (once per session).
const AXES_S: f64 = 20.0;
const AXES_EVERY_S: f64 = 2.0;
/// C6: the name lookup is retried this often, this many times, before `HOLD bone name=none`.
const BONE_LOOKUP_EVERY_S: f64 = 1.0;
const BONE_LOOKUP_TRIES: u32 = 20;
/// C6 attach 2: the `HOLD weapon probe` lines: every WPROBE_EVERY_S for WPROBE_S once the stance first holds the weapon
/// (or hand mode first draws without the stance), once per session, with `bone_probe`.
const WPROBE_S: f64 = 10.0;
const WPROBE_EVERY_S: f64 = 0.5;
/// C6 attach 2: a player's `HOLD src` line at most this often (on a change).
const SRC_LOG_S: f64 = 2.0;
/// C6 attach 2: learned hand -> weapon grips kept per weapon row (session).
const G_HAND_MAX: usize = 16;
/// C6 attach 2: `weapon_axes = "auto"` votes only after the stance has held the weapon this long (ER's blend into the
/// crossbow idle is over).
const POSE_SETTLE_S: f64 = 1.0;
/// C6 pose 3: the `HOLD axes` window re-arms this many times when the stance settles again (after a chord).
const AXES_REARMS: u32 = 3;
/// C6 part 17 review: at most this many `HOLD guard align_clamped / align_unclamped` lines per guard episode.
const GUARD_CLAMP_LINES: u32 = 4;
/// C6 pose 3: the left hand (`L_Hand`, FLVER 29 in the 17:32 bone probe) for `lh_d=` (left hand to ER's weapon origin).
const LEFT_HAND_BONE: u32 = 29;
/// C6 depth fallback: the body-bone name lookup is retried this often, this many times.
const BODY_LOOKUP_EVERY_S: f64 = 1.0;
const BODY_LOOKUP_TRIES: u32 = 20;

/// One world gun for the renderer: the arsenal slot of the gun on THIS PC, the LOD, model (inches) -> ER world (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldGunDraw {
    pub slot: u16,
    pub lod: u8,
    pub model: Mat34,
}

/// What the task hands in every tick.
pub struct CoopVisIn<'a> {
    pub now: f64,
    pub cfg: &'a Config,
    pub in_world: bool,
    pub mode_on: bool,
    pub peers: &'a [PeerSlot],
    pub player: Option<&'a PlayerSample>,
    /// The stable gun index g of the gun in hand (None without the arsenal).
    pub gun_g: Option<u32>,
    pub ac130_active: bool,
    /// Our nuke wants its synced marker on (the caller of an owned co-op nuke, before the window ends).
    pub nuke_marker: bool,
    pub fp_on: bool,
    /// First person is latched off by a loading screen / cutscene: no world guns (the camera is not the player's).
    pub fp_latched: bool,
    /// ER's camera forward (row 2) this tick: our own world gun's aim yaw in third person.
    pub cam_fwd: Option<V3>,
    /// The eye the HUD projects from (FP eye or ER's camera): listener for sounds, origin of the occlusion rays.
    pub view_pos: V3,
    /// C6: we ride Torrent (`hold_mounted = "hide"` hides our world gun in hand mode).
    pub mounted: bool,
    /// C6: the MW2 ADS fraction (0..1) for `tp_ads_hold = "aim"`.
    pub ads: f32,
    /// C6 scope peek: first person is on only for a third-person scoped ADS (it counts as third person for the
    /// solo hide marker; the self world gun still follows the real `fp_on`).
    pub scope_peek: bool,
    /// C6 attach 2: the crossbow stance holds the weapon (`ErStance::posed`, last tick): `weapon_axes = "auto"` votes.
    pub stance_posed: bool,
    /// C6 pose 3: our grip (`arm_style`) as the stance read it last tick (0 = unknown / stance off): the `HOLD axes` line.
    pub stance_arm: u32,
    /// C6 aim 4: ER's crossbow aim is confirmed (last tick): the gun turns about its grip onto the camera's aim line.
    pub stance_aim_on: bool,
    /// C6 aim 4: an aim episode runs (last tick): `tp_ads_hold = "aim"` stays out of it.
    pub stance_aim_active: bool,
    /// C6 part 16 review: the real crossbow carrier is on (last tick): ER's genuine aim raises the arms, so the round-7
    /// `hold_fallback` never shoulders the gun along the camera (it stays on ER's weapon frame, `aim_align` only).
    pub stance_carrier: bool,
    /// C6 part 17 (docs 17.5): a third-person guard episode holds ER's block input (last tick); its guard pose was
    /// detected (the gun turns onto the aim line, at most `tp_guard_align_max_deg`); it was not detected in time (the
    /// gun raise with `tp_ads_hold = "aim"`).
    pub stance_guard_active: bool,
    pub stance_guard_align: bool,
    pub stance_guard_fallback: bool,
    /// C6 part 17 addendum: the guard is held standing still (last tick): `guard_axes = "auto"` learns only then.
    pub stance_guard_standing: bool,
}

/// C6 attach 2: one player's hand-path state (ours or a peer's): the stage-A jump guard / fallback log of the final
/// matrix, the weapon-model miss log, its stale guard and the last logged source.
#[derive(Clone, Debug, Default)]
struct HoldPlayer {
    track: HandTrack,
    fb: FallbackLog,
    wfb: FallbackLog,
    stale: StaleGuard,
    src: Option<&'static str>,
    src_log_t: f64,
    /// C6 depth fallback: the last logged body-capsule state ("ok" or a reason) of this player.
    body: Option<&'static str>,
}

impl HoldPlayer {
    fn reset(&mut self) {
        *self = HoldPlayer::default();
    }
}

/// C6 attach 2: the four vtables of the weapon-model chain (live addresses).
#[derive(Clone, Copy, Debug)]
struct WeaponVts {
    asm: usize,
    parts: usize,
    ent: usize,
    arr: usize,
}

/// C6: what the hand path needs this tick, shared by every player.
struct HoldCtx {
    bone: Result<u32, Reason>,
    vts: Option<(usize, usize)>,
    wvts: Option<WeaponVts>,
    weapon: bool,
    hand_axes: Option<Mat34>,
    weapon_axes: Mat34,
    /// C6 part 16.7: the carrier crossbow's weapon-frame map (`weapon_axes_xbow`), used on a carrier row.
    weapon_axes_xbow: Mat34,
}

/// C6 attach 2: one player's frame of the hand path.
struct HoldFrame {
    /// The gun matrix and the `HOLD hand ok` detail, or why stage A is drawn.
    res: Result<(Mat34, String), Reason>,
    /// ER's weapon pose (world, orthonormal) this frame: its model, else the hand bone x the learned grip.
    weapon: Option<Mat34>,
    /// The validated hand bone (world).
    hand: Option<HandOk>,
    /// C6 part 17 addendum: the active right weapon row (0 = unreadable): the guard frame is learned per row.
    row: u32,
}

/// C6 hide: the in-memory patch of the MW2-mode marker row.
#[derive(Default)]
struct HidePatch {
    /// We wrote the patch (restore it when no longer wanted).
    patched: bool,
    /// A foreign row / failed write: never touched again this session.
    refused: Option<&'static str>,
}

/// C6 (docs/C6_HOLD_DESIGN.md 4-6): the hand path's per-session state.
#[derive(Default)]
struct HoldState {
    cfg_line: String,
    axes_src: String,
    axes: Option<mw2er_core::xform::Mat34>,
    /// C6 attach 2: the explicit `weapon_axes` (parsed, by its text), the map learned in game, its vote.
    waxes_src: String,
    waxes: Option<Mat34>,
    learned: Option<(String, Mat34)>,
    vote: AxesVote,
    /// Since when the stance holds the weapon (the first POSE_SETTLE_S are ER's blend into the crossbow idle: no votes).
    posed_t0: Option<f64>,
    /// Learned hand -> weapon grips per weapon row (the bone keeps the sword's pose when the model is not usable).
    g_hand: Vec<(u32, Mat34)>,
    self_hp: HoldPlayer,
    wprobe_t0: Option<f64>,
    wprobe_next: f64,
    wprobe_done: bool,
    hide: HidePatch,
    /// The name lookup's result (cached per session: the same c0000 FLVER on every PC).
    bone_auto: Option<Result<u32, &'static str>>,
    bone_tries: u32,
    bone_next: f64,
    axes_t0: Option<f64>,
    axes_next: f64,
    axes_done: bool,
    ads_u: f32,
    last_now: Option<f64>,
    mounted_logged: bool,
    /// C6 grip 3: the last `HOLD grip` line (gun, values) of our own gun.
    grip_logged: String,
    /// C6 grip 3: `weapon_axes = "auto"` voted a map that is not the rig map (kept the rig map).
    auto_rejected: bool,
    /// C6 pose 3: the `HOLD axes` window re-arms on each new settled stance (at most AXES_REARMS).
    axes_rearms: u32,
    prev_posed: bool,
    /// C6 depth fallback: the FLVER index of every `body_occl::BODY_SLOTS` name (cached per session), the lookup tries.
    body_bones: Option<Result<Vec<Option<u32>>, &'static str>>,
    body_tries: u32,
    body_next: f64,
    /// C6 aim 4: the aim-align blend (0..1), this episode's `HOLD aim align` / `align_clamped` lines logged.
    aim_u: f32,
    aim_logged: bool,
    aim_clamp_logged: bool,
    /// C6 round 7: ER's aim pose did not turn the weapon this episode (the align clamped): with `tp_ads_hold = "aim"` the
    /// gun shoulders along the camera instead (`HOLD aim hold_fallback` once per episode).
    aim_nopose: bool,
    /// C6 part 17: the guard episode's align over `tp_guard_align_max_deg` (review: debounced and un-latching,
    /// `hold::GuardClamp`: the gun raise while set), its `HOLD guard align_clamped / align_unclamped` lines this episode,
    /// and the last settled align of this frame (deg, clamped) for the stance's `HOLD guard episode` line.
    guard_clamp: mw2er_core::hold::GuardClamp,
    guard_clamp_lines: u32,
    guard_align: Option<(f32, bool)>,
    /// C6 part 17 addendum (docs 17.12): the guard-frame blend (0..1, in over the detection, out at the release), the
    /// frame in use (kept for the blend out), its source this frame (the episode line's `corr=`), the `HOLD guard frame`
    /// line once per episode, and the `guard_axes = "auto"` learning (per weapon row, session).
    guard_u: f32,
    guard_last: Option<(Mat34, mw2er_core::hold::GuardFrameSrc)>,
    guard_corr: Option<&'static str>,
    guard_frame_logged: bool,
    guard_learn: mw2er_core::hold::GuardLearn,
}

/// What the task takes out.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoopVisOut {
    /// A peer's nuke marker was seen this tick (peer handle, "speffect" | "bullet").
    pub nuke_from: Option<(u64, &'static str)>,
}

#[derive(Clone, Debug)]
struct PeerVis {
    handle: u64,
    addr: usize,
    first_seen: f64,
    markers: PeerMarkers,
    last_twin: Option<(u32, f64)>,
    seen_sp: Vec<i32>,
    gun_logged: Option<(u32, &'static str)>,
    gun_log_t: f64,
    /// Review C3: the peer's havok frame this tick (None = pose unreadable).
    frame: Option<f32>,
    /// Review C3: the direction of the peer's last shot (its aim) and when.
    aim: Option<(V3, f64)>,
    /// Review C3: the last `worldgun skip` reason logged for this peer.
    skip_logged: Option<&'static str>,
    /// C6: the hand path's per-player state.
    hp: HoldPlayer,
}

#[derive(Default)]
struct Window {
    t0: f64,
    sent: u32,
    sent_row: i32,
    sent_gun: u32,
    seen: u32,
    seen_row: i32,
    seen_peer: u64,
    seen_known: bool,
    fx: FxStats,
    refused: u32,
    other_frame: u32,
}

/// Review 2026-10-24: our launcher streaks hidden behind map geometry (task.rs `publish`), for the 5 s line.
pub static PROJ_HIDDEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub struct CoopVis {
    markers: MarkerCtl,
    tracker: RemoteTracker,
    nuke_rx: NukeMarkerRx,
    limiter: TwinLimiter,
    peers: Vec<PeerVis>,
    next_marker_read: f64,
    own_ids: Option<Vec<i32>>,
    win: Window,
    in_world: bool,
    cfg_line: String,
    /// Peer explosions for the HUD flash (position, time, size m, visible from the camera: None = no ray yet).
    flashes: Vec<(V3, f64, f32, Option<bool>)>,
    /// Review 2026-10-24: the peers' tracers / flashes behind map geometry (map rays, budgeted, cached per tracer).
    tocc: TracerOccl,
    flashes_hidden: u32,
    pub world_guns: Vec<WorldGunDraw>,
    /// C6 depth fallback: this tick's body capsules for the world-gun pass (`[coop] world_gun_body_occlusion`).
    pub world_occl: Vec<Capsule>,
    /// C6 depth 5: our own Spine2 bone this tick (world metres; third person, body capsules resolved): the chest pixel of
    /// probe P-S2's self check.
    pub self_chest: Option<[f32; 3]>,
    /// The twin / marker rows are live (PARAMS); `None` = not checked in this world yet.
    rows_ok: Option<bool>,
    rng: mw2er_core::rng::Rng,
    /// Review C3: the last known local player handle (never treat it as a peer's while a sample is missing).
    last_local: u64,
    /// Review C3 stage B: the bone probe (start time; done once per session) and the last `worldgun bone` source.
    probe_t0: Option<f64>,
    probe_next: f64,
    probe_done: bool,
    prev_mode_on: bool,
    bone_logged: Option<&'static str>,
    /// Review C3: the last peer 105 mm impact near us (time, amplitude 0..1, noise seed) for the view shake.
    quake: Option<(f64, f32, u32)>,
    /// C6: the gun on the hand bone.
    hold: HoldState,
}

impl CoopVis {
    pub fn new(seed: u64) -> Self {
        CoopVis {
            markers: MarkerCtl::default(),
            tracker: RemoteTracker::default(),
            nuke_rx: NukeMarkerRx::default(),
            limiter: TwinLimiter::default(),
            peers: Vec::new(),
            next_marker_read: 0.0,
            own_ids: None,
            win: Window::default(),
            in_world: false,
            cfg_line: String::new(),
            flashes: Vec::new(),
            tocc: TracerOccl::default(),
            flashes_hidden: 0,
            world_guns: Vec::new(),
            world_occl: Vec::new(),
            self_chest: None,
            rows_ok: None,
            rng: mw2er_core::rng::Rng::new(seed ^ 0xC0_0B_515),
            last_local: 0,
            probe_t0: None,
            probe_next: 0.0,
            probe_done: false,
            prev_mode_on: false,
            bone_logged: None,
            quake: None,
            hold: HoldState::default(),
        }
    }

    /// C6 part 17: the gun's aim align this frame inside a guard episode (deg to the aim line, clamped), for the stance's
    /// `HOLD guard episode` line; None outside one / no align computed.
    pub fn guard_align(&self) -> Option<(f32, bool)> {
        self.hold.guard_align
    }

    /// C6 part 17 addendum: the guard frame our gun used this frame ("builtin" | "auto" | "fixed"; None = none).
    pub fn guard_corr(&self) -> Option<&'static str> {
        self.hold.guard_corr
    }

    /// C6 PASS H2: the peers the stance module may patch for (handle, PlayerIns address, carries the MW2-mode marker).
    pub fn peer_states(&self) -> Vec<(u64, usize, bool)> {
        self.peers.iter().map(|p| (p.handle, p.addr, p.markers.mode)).collect()
    }

    fn rows_ok(&mut self) -> bool {
        if self.rows_ok.is_none() {
            let r = crate::arsenal_rt::ARSENAL.get().and_then(|a| a.coop.clone());
            self.rows_ok = Some(match r.as_ref().map(|c| crate::game::params::check_coop(c)) {
                Some(Ok(s)) => {
                    log_i!("COOP", "rows ok {s}");
                    true
                }
                Some(Err(e)) => {
                    log_w!("COOP", "WARN rows {e}: twins and markers off (rebuild / redeploy the regulation on both PCs)");
                    false
                }
                None => {
                    log_w!("COOP", "WARN rows.json has no coop block: twins and markers off (rebuild the regulation)");
                    false
                }
            });
        }
        self.rows_ok == Some(true)
    }

    /// C6 hide (work/c6/hide.md 4 A): patches the MW2-mode marker row 79001090 into an Unseen Blade-style weapon hide
    /// (`HIDE_FIELDS` through `game::write`, at the address of a read-only `SoloParamRepository` lookup) while `want`,
    /// and writes the packaged values back when no longer wanted. Bytes that are neither the package's nor the patch
    /// are never written (`HOLD hide foreign`, refused for the session). Returns true when the row changed this call
    /// (a present marker is then re-applied so ER evaluates the new row). Nothing is read while not wanted and never
    /// patched (the defaults stay byte-identical).
    fn hide_tick(&mut self, want: bool) -> bool {
        let h = &mut self.hold.hide;
        if h.refused.is_some() || (!want && !h.patched) {
            return false;
        }
        let Some(addr) = sp_row_addr(SP_MODE as u32) else {
            h.refused = Some("row_missing");
            log_w!("HOLD", "hide refused sp={SP_MODE} reason=row_missing (the weapon stays visible)");
            return false;
        };
        let Some(cur) = read_hide(addr) else {
            h.refused = Some("read");
            log_w!("HOLD", "hide refused sp={SP_MODE} reason=read (the weapon stays visible)");
            return false;
        };
        let state = hide_row_state(&cur);
        match (want, state) {
            (true, HideRow::Package) => {
                let ok = write_hide(addr, true);
                if ok && read_hide(addr).is_some_and(|v| hide_row_state(&v) == HideRow::Patched) {
                    h.patched = true;
                    log_i!("HOLD", "hide row patched sp={SP_MODE} state=184 vow=0xffff wep_param_change=1 rates=1.0 motion_interval=-1 (Unseen Blade hide; spCategory stays 0)");
                    true
                } else {
                    // a partial write is put back where possible; never retried
                    let back = write_hide(addr, false);
                    h.refused = Some("write");
                    log_w!("HOLD", "hide refused sp={SP_MODE} reason=write ok={} restored={} (the weapon stays visible)", ok as u8, back as u8);
                    true
                }
            }
            (true, HideRow::Patched) => {
                if !h.patched {
                    h.patched = true;
                    log_i!("HOLD", "hide row already patched sp={SP_MODE} (kept)");
                }
                false
            }
            (false, HideRow::Patched) => {
                let ok = write_hide(addr, false) && read_hide(addr).is_some_and(|v| hide_row_state(&v) == HideRow::Package);
                h.patched = false;
                if ok {
                    log_i!("HOLD", "hide row restored sp={SP_MODE} (package values)");
                } else {
                    h.refused = Some("restore");
                    log_w!("HOLD", "hide restore failed sp={SP_MODE}");
                }
                true
            }
            (false, HideRow::Package) => {
                h.patched = false;
                false
            }
            (_, HideRow::Foreign) => {
                h.refused = Some("foreign");
                h.patched = false;
                log_w!(
                    "HOLD",
                    "hide foreign sp={SP_MODE} fields={} (neither the package nor the patch: nothing written, the weapon stays visible)",
                    HIDE_FIELDS.iter().zip(&cur).map(|(f, v)| format!("{}:0x{v:x}", f.name)).collect::<Vec<_>>().join(",")
                );
                false
            }
        }
    }

    /// A world leave: every local state is dropped. Markers still on our character are not forgotten for good: the
    /// next marker tick removes any C3 marker it finds present but unwanted (review C3, `MarkerCtl` orphans).
    pub fn reset(&mut self) {
        self.markers.clear();
        self.tracker.clear();
        self.tocc.clear();
        self.peers.clear();
        self.flashes.clear();
        self.world_guns.clear();
        self.world_occl.clear();
        self.self_chest = None;
        self.own_ids = None;
        self.probe_t0 = None;
        // C6: the hand path's per-player state belongs to the old world (the bone index, the learned axes / grips and the
        // row patch stay: same FLVERs, same param rows)
        self.hold.self_hp.reset();
        self.hold.ads_u = 0.0;
        self.hold.last_now = None;
    }

    /// Review C3: the panic path. Removes every C3 marker SpEffect still on our character (MW2 mode's 8060 would keep
    /// ER's weapon invisible, a nuke marker could re-trigger a peer), then resets. Gated wrappers; reads non-faulting.
    pub fn strip_markers(&mut self) {
        let me = main_player();
        let ids = speffect_ids(me).map(|v| c3_markers(&v)).unwrap_or_default();
        for id in ids {
            // SAFETY: the live local player read just now, game thread; gated (version gate, marker ids) inside.
            let ok = unsafe { crate::ac130::remove_speffect(me, id) };
            log_i!("COOP", "marker removed sp={id} call_ok={} why=panic", ok as u8);
        }
        self.reset();
    }

    /// C3 PASS C: the twin of a damaging spawn `src_rid` (no-op solo, with `visual_twins` off, for rows without a twin,
    /// or rate-limited). Never touches the damaging spawn; the twin has no attack.
    #[allow(clippy::too_many_arguments)]
    pub fn send_twin(&mut self, now: f64, cfg: &Config, peers: u32, p: &PlayerSample, src_rid: i32, origin: V3, dir: V3) {
        let Some(id) = twin_wanted(peers, cfg.coop.visual_twins, src_rid) else { return };
        if !self.rows_ok() {
            return;
        }
        let Some(tw) = decode_twin(id) else { return };
        if matches!(tw.kind, TwinKind::Gun | TwinKind::Launch) && !self.limiter.allow(now, tw.g, cfg.coop.twin_min_interval_ms as f64 / 1000.0) {
            return;
        }
        if !spawn_guard::admit(now, SpawnKind::Twin, id) {
            self.win.refused += 1;
            return;
        }
        match spawn::spawn_synced(p.field_ins_handle, id, origin, p.chr.pos_w, dir) {
            Ok(_) => {
                self.win.sent += 1;
                self.win.sent_row = id;
                self.win.sent_gun = tw.g;
            }
            Err(e) => {
                self.win.refused += 1;
                if self.win.refused <= 3 {
                    log_w!("COOP", "WARN twin spawn id={id} src={src_rid} {e}");
                }
            }
        }
    }

    /// One tick. Returns the peer's nuke marker when one arrived.
    pub fn tick(&mut self, i: &CoopVisIn, ar: Option<&Arc<ArsenalData>>) -> CoopVisOut {
        let mut out = CoopVisOut::default();
        let c = &i.cfg.coop;
        if i.in_world != self.in_world {
            self.in_world = i.in_world;
            self.reset();
            self.rows_ok = None;
        }
        let line = format!(
            "vis cfg twins={} interval_ms={} effects={} sounds={} tracers={} flashes={} markers={} hide_er_weapon={} world_guns={} self_tp={} offset=[{:.2},{:.2},{:.2}] lod1_m={:.0} occlusion={} peer_drop_guard={} factor={:.1} hand_bone={} bone_probe={} tracer_occl={} tracer_rays={}",
            c.visual_twins as u8,
            c.twin_min_interval_ms,
            c.peer_effects as u8,
            c.peer_sounds as u8,
            c.peer_tracers as u8,
            c.peer_flashes as u8,
            c.markers as u8,
            c.hide_er_weapon as u8,
            c.world_guns as u8,
            c.world_gun_self_tp as u8,
            c.world_gun_offset_m[0],
            c.world_gun_offset_m[1],
            c.world_gun_offset_m[2],
            c.world_gun_lod1_m,
            c.world_gun_occlusion as u8,
            c.peer_drop_guard as u8,
            c.peer_drop_factor,
            c.hand_bone,
            c.bone_probe as u8,
            c.tracer_occlusion as u8,
            c.tracer_occlusion_rays
        );
        if line != self.cfg_line {
            log_i!("COOP", "{line}");
            self.cfg_line = line;
        }
        // C6: one line at config load / change (the only HOLD line with the defaults)
        let (vt43, vt44) = own_vtables_rva();
        let hline = format!(
            "cfg hold_mode={} hand_bone={} auto={} axes={} grip=[{:.3},{:.3},{:.3}] rot=[{:.1},{:.1},{:.1}] smooth={:.2} grips={} mounted={} tp_ads_hold={} er_aim={} er_stance={} refresh={} min_switch_s={:.1} peers={} peer_delay_s={:.1} vt43={} vt44={}",
            c.hold_mode,
            c.hand_bone,
            c.hand_bone_auto as u8,
            c.hand_axes,
            c.grip_offset_m[0],
            c.grip_offset_m[1],
            c.grip_offset_m[2],
            c.grip_rot_deg[0],
            c.grip_rot_deg[1],
            c.grip_rot_deg[2],
            c.hand_smooth,
            c.grip.len(),
            c.hold_mounted,
            c.tp_ads_hold,
            c.tp_ads_er_aim as u8,
            c.er_stance,
            c.stance_refresh,
            c.stance_min_switch_s,
            c.stance_peers as u8,
            c.stance_peer_delay_s,
            vt43.map_or("none".to_string(), |v| format!("0x{v:x}")),
            vt44.map_or("none".to_string(), |v| format!("0x{v:x}"))
        ) + &format!(
            " source={} weapon_axes={} hide_patch={} vtw={} weapon_rake={:.1} grip_table={} grip_cfg={} exit_defer_s={:.1} body_occl={} probe_er_depth={} ads_pose={} ads_patch={} probe_er_aim={} er_depth={} depth_state={} depth_sample={} bias={:.3}+{:.4}z max_turn={:.1} aim_cam={}",
            c.hold_source,
            c.weapon_axes,
            c.hide_state_patch as u8,
            weapon_vtables_rva().map_or("none".to_string(), |v| format!("0x{:x}/0x{:x}/0x{:x}", v.0, v.1, v.2)),
            c.weapon_rake_deg,
            mw2er_core::hold::GRIP_IN.len(),
            c.grip.len(),
            c.stance_exit_defer_s,
            c.world_gun_body_occlusion as u8,
            c.probe_er_depth as u8,
            // C6 aim 4: the effective pose (the old tp_ads_er_aim key is its alias; needs the crossbow stance); C6 part 17:
            // guard (needs the polearm stance)
            if c.ads_pose_on() {
                "er_aim"
            } else if c.guard_pose_on() {
                "guard"
            } else {
                "off"
            },
            c.tp_ads_patch,
            c.probe_er_aim as u8,
            // C6 depth 5
            c.world_gun_er_depth as u8,
            c.probe_er_depth_state as u8,
            c.probe_er_depth_sample as u8,
            c.er_depth_bias_m,
            c.er_depth_bias_rel,
            c.er_depth_max_turn_deg,
            // C6 round 6: ER's aim camera / reticle during the aim pose (keep = masked, er = the escape hatch)
            c.tp_aim_cam
        ) + &format!(
            // C6 part 16: the real crossbow carrier (effective: needs the stance), the give, its weapon-frame map
            " carrier={} give={} axes_xbow={} load={}",
            c.carrier_mode().label(),
            c.carrier_give as u8,
            c.weapon_axes_xbow,
            // C6 part 16.16: the loaded crossbow
            c.carrier_load as u8
        ) + &format!(
            // C6 part 17: the guard's body yaw (effective: needs the guard) and its align limit
            " guard_yaw={} guard_max={:.0}",
            c.guard_yaw_on() as u8,
            c.tp_guard_align_max_deg
        ) + &format!(
            // C6 part 17 addendum: the polearm carry grips and the guard frame (effective: the polearm stance / the guard)
            " polearm_grip={} long_min={:.2} guard_axes={} guard_rot={}",
            c.polearm_grip_on() as u8,
            c.polearm_long_min_m,
            if c.guard_frame_on() { c.guard_axes.as_str() } else { "off" },
            floats_text(&c.guard_rot_deg)
        );
        if hline != self.hold.cfg_line {
            log_i!("HOLD", "{hline}");
            self.hold.cfg_line = hline;
        }
        if !i.in_world {
            return out;
        }
        let now = i.now;
        // the peer list follows the debounced watch
        self.peers.retain(|p| i.peers.iter().any(|s| s.handle == p.handle));
        for s in i.peers {
            match self.peers.iter_mut().find(|p| p.handle == s.handle) {
                Some(p) => p.addr = s.addr,
                None => self.peers.push(PeerVis {
                    handle: s.handle,
                    addr: s.addr,
                    first_seen: now,
                    markers: PeerMarkers::default(),
                    last_twin: None,
                    seen_sp: Vec::new(),
                    gun_logged: None,
                    gun_log_t: f64::NEG_INFINITY,
                    frame: None,
                    aim: None,
                    skip_logged: None,
                    hp: HoldPlayer::default(),
                }),
            }
        }
        let n_peers = self.peers.len() as u32;
        // review C3: every peer's havok frame this tick (positions of a peer in another tile are not in our frame)
        for p in self.peers.iter_mut() {
            // SAFETY: the peer's ChrIns from the player set (re-read by the watch at 4 Hz); non-faulting reads.
            p.frame = unsafe { crate::game::chars::read_pose(p.addr) }.map(|x| x.2);
        }
        if let Some(pl) = i.player {
            self.last_local = pl.chr.handle;
        }
        self.bone_probe(i);

        // ---- markers: ours (applied / refreshed / removed) and the peers' (4 Hz) ----
        if now >= self.next_marker_read {
            self.next_marker_read = now + MARKER_READ_S;
            let me = main_player();
            self.own_ids = speffect_ids(me);
            // markers matter with a peer (a peer joining later gets them within 0.25 s); solo only the MW2-mode marker,
            // and only while C6 draws the gun in our hand / poses the hidden weapon (`hold_mode = "hand"` or the stance,
            // third person): it hides ER's own weapon, which nothing else hides solo. Defaults: solo applies nothing.
            // C6 scope peek: a peek is third person here (the marker stays; the render bit hides the body anyway)
            let solo_hide = (c.hold_hand() || c.stance_on()) && (!i.fp_on || i.scope_peek);
            // C6 hide: the marker row becomes a real weapon hide (stateInfo 184) before it is applied; restored when off
            let rows_ok = self.rows_ok();
            let reapply = self.hide_tick(c.hide_patch_wanted() && rows_ok);
            let want = if (n_peers > 0 || solo_hide) && rows_ok {
                marker_scope(n_peers > 0, solo_hide, wanted_markers(c.markers, i.mode_on, c.hide_er_weapon, i.gun_g, i.ac130_active, i.nuke_marker))
            } else {
                Vec::new()
            };
            let mut acts = Vec::new();
            // the row changed under a present marker: it is removed now and re-applied (fresh) on the next read
            if reapply && self.own_ids.as_ref().is_some_and(|v| v.contains(&SP_MODE)) {
                acts.push(MarkerAct::Remove(SP_MODE));
            }
            // C6: renew a present marker 3 s before its endurance ends (no 0.25 s gap every 30 s: the hidden sword blinked)
            let renew = if c.hold_hand() || c.stance_on() {
                crate::arsenal_rt::ARSENAL.get().and_then(|a| a.coop.as_ref()).map_or(0.0, |r| (r.marker_endurance_s - MARKER_RENEW_MARGIN_S).max(0.0))
            } else {
                0.0
            };
            self.markers.tick_renew(now, &want, self.own_ids.as_deref(), c.marker_refresh_s as f64, renew, &mut acts);
            let mut k = 0;
            while k < acts.len() {
                match acts[k] {
                    MarkerAct::Remove(id) if acts.get(k + 1) == Some(&MarkerAct::Apply(id)) => {
                        // SAFETY: the live local player read just now, game thread; gated (version gate, marker ids) inside.
                        let ok_r = unsafe { crate::ac130::remove_speffect(me, id) };
                        // SAFETY: as above.
                        let ok_a = unsafe { crate::ac130::apply_speffect_synced(me, id) };
                        log_i!("COOP", "marker renewed sp={id} remove_ok={} apply_ok={}", ok_r as u8, ok_a as u8);
                        k += 2;
                        continue;
                    }
                    MarkerAct::Apply(id) => {
                        // SAFETY: the live local player read just now, game thread; gated (version gate, marker ids) inside.
                        let ok = unsafe { crate::ac130::apply_speffect_synced(me, id) };
                        let present = self.own_ids.as_ref().is_some_and(|v| v.contains(&id));
                        if !present {
                            log_i!("COOP", "marker sent sp={id} call_ok={} peers={n_peers}", ok as u8);
                        }
                    }
                    MarkerAct::Remove(id) => {
                        // SAFETY: as above.
                        let ok = unsafe { crate::ac130::remove_speffect(me, id) };
                        log_i!("COOP", "marker removed sp={id} call_ok={}", ok as u8);
                    }
                }
                k += 1;
            }
            for p in self.peers.iter_mut() {
                let Some(ids) = speffect_ids(p.addr) else { continue };
                let m = peer_markers(&ids);
                for id in ids.iter().copied().filter(|id| mw2er_core::coop_vis::syncable_speffect(*id)) {
                    if !p.seen_sp.contains(&id) && p.seen_sp.len() < 64 {
                        p.seen_sp.push(id);
                        log_i!("COOP", "marker seen peer={} sp={id} dt={:.0}ms", logfmt::handle(p.handle), (now - p.first_seen) * 1000.0);
                    }
                }
                p.seen_sp.retain(|id| ids.contains(id) || *id == mw2er_core::coop_vis::SP_NUKE);
                if m.nuke && self.nuke_rx.seen(now, p.handle) {
                    out.nuke_from = Some((p.handle, "speffect"));
                }
                p.markers = m;
            }
        }

        // ---- the peers' twins: tracers, sounds, explosions (cosmetic only) ----
        // review C3: only with a known local handle (a missing player sample must not class our own twins / nuke marker
        // as a peer's); peers in another havok frame give no positions (their twins are dropped, counted)
        if n_peers > 0 && c.peer_effects && let Some(pl) = i.player {
            let local = pl.chr.handle;
            let pw = pl.chr.pos_w;
            let other: Vec<u64> = self.peers.iter().filter(|p| p.frame.is_some_and(|w| w != pw)).map(|p| p.handle).collect();
            let list: Vec<_> = bullets::read_remote(local).into_iter().filter(|b| b.owner != self.last_local).collect();
            let mut ev = Vec::new();
            self.tracker.update(now, &list, local, &other, &mut ev);
            self.win.other_frame += self.tracker.take_other_frame();
            for e in &ev {
                self.win.fx.add(e);
                self.on_event(now, i, ar, e, &mut out);
            }
            self.occlusion_tick(now, i, pl);
        } else if n_peers == 0 && self.tracker.tracked() > 0 {
            self.tracker.clear();
            self.tocc.clear();
        }
        self.flashes.retain(|(_, t, _, _)| now - *t <= FLASH_S);

        // ---- world guns ----
        self.world_guns.clear();
        self.world_occl.clear();
        self.self_chest = None;
        if c.world_guns && !i.fp_latched {
            self.world_guns_tick(i, ar);
        }

        // ---- the 5 s lines ----
        if self.win.t0 == 0.0 {
            self.win.t0 = now;
        }
        if now - self.win.t0 >= c.log_every_s as f64 {
            let w = std::mem::take(&mut self.win);
            self.win.t0 = now;
            if w.sent > 0 || w.refused > 0 {
                let gun = ar.and_then(|a| a.guns.iter().find(|g| g.spec.g == w.sent_gun)).map_or("ac130".to_string(), |g| g.name.clone());
                log_i!("COOP", "twin sent gun={gun} row={} n={}/{:.0}s refused={} peers={n_peers}", w.sent_row, w.sent, c.log_every_s, w.refused);
            }
            if w.seen > 0 {
                log_i!(
                    "COOP",
                    "twin seen peer={} row={} n={}/{:.0}s known_peer={}",
                    logfmt::handle(w.seen_peer),
                    w.seen_row,
                    w.seen,
                    c.log_every_s,
                    w.seen_known as u8
                );
            }
            if w.other_frame > 0 {
                log_i!("COOP", "peer fx skipped other_frame={}/{:.0}s (a peer in another map tile: positions not in our frame)", w.other_frame, c.log_every_s);
            }
            let o = self.tocc.take_stats();
            let fh = std::mem::take(&mut self.flashes_hidden);
            let ph = PROJ_HIDDEN.swap(0, std::sync::atomic::Ordering::Relaxed);
            if o.rays > 0 || o.no_rays > 0 || o.hidden + o.partial > 0 || fh > 0 || ph > 0 {
                log_i!(
                    "COOP",
                    "tracer occl draws shown={} partial={} hidden={} clipped={} rays={} budget_out={} no_rays={} flashes_hidden={} streaks_hidden={}/{:.0}s",
                    o.shown,
                    o.partial,
                    o.hidden,
                    o.clip_hits,
                    o.rays,
                    o.budget_out,
                    o.no_rays,
                    fh,
                    ph,
                    c.log_every_s
                );
            }
            if w.fx.any() {
                log_i!(
                    "COOP",
                    "peer fx muzzle={} impact={} expl={} sounds={} nuke_markers={}/{:.0}s",
                    w.fx.muzzle,
                    w.fx.impact,
                    w.fx.explosion,
                    w.fx.sounds,
                    w.fx.nuke_markers,
                    c.log_every_s
                );
            }
        }
        out
    }

    fn on_event(&mut self, now: f64, i: &CoopVisIn, ar: Option<&Arc<ArsenalData>>, e: &FxEvent, out: &mut CoopVisOut) {
        let c = &i.cfg.coop;
        let seen = |w: &mut Window, owner: u64, row: i32, peers: &[PeerVis]| {
            w.seen += 1;
            w.seen_row = row;
            w.seen_peer = owner;
            w.seen_known = peers.iter().any(|p| p.handle == owner);
        };
        match *e {
            FxEvent::Muzzle { owner, row, twin, pos, sound, dir } => {
                seen(&mut self.win, owner, row, &self.peers);
                if let Some(p) = self.peers.iter_mut().find(|p| p.handle == owner) {
                    p.last_twin = Some((twin.g, now));
                    // review C3: the shot direction is the peer's aim (its gun points there, not along its body)
                    if twin.kind == TwinKind::Gun {
                        p.aim = Some((dir, now));
                    }
                }
                if sound && c.peer_sounds {
                    let d = (pos - i.view_pos).len();
                    let g = peer_gain(d, c.sound_min_m, c.sound_max_m, 0.0) * c.peer_gain;
                    if g > 0.01
                        && let Some(alias) = ar.and_then(|a| a.guns.iter().find(|x| x.spec.g == twin.g)).and_then(|x| x.roles.get("fire_npc").copied())
                    {
                        shared::play_gun_at(alias, g);
                    }
                }
            }
            FxEvent::Impact { .. } => {}
            FxEvent::Explosion { owner, row, twin, pos } => {
                seen(&mut self.win, owner, row, &self.peers);
                let size = match twin.kind {
                    TwinKind::Ac130 { gun: 0, .. } => 6.0,
                    TwinKind::Ac130 { gun: 1, .. } => 2.5,
                    TwinKind::Ac130 { .. } => 0.8,
                    _ => 3.0,
                };
                if self.flashes.len() < 32 {
                    self.flashes.push((pos, now, size, None));
                }
                if c.peer_sounds {
                    self.explosion_sound(i, twin, pos);
                }
                // review C3: a peer's 105 mm shakes our view with distance (the AC-130's own impact shake, scaled)
                if let TwinKind::Ac130 { gun: 0, tier: 1 } = twin.kind {
                    let d = (pos - i.view_pos).len();
                    let k = (1.0 - (d / 120.0)).clamp(0.0, 1.0);
                    if k > 0.05 {
                        self.quake = Some((now, k, (now * 1000.0) as u32));
                    }
                }
            }
            FxEvent::NukeMarker { owner, pos } => {
                seen(&mut self.win, owner, mw2er_core::coop_vis::NUKE_MARKER_BULLET, &self.peers);
                if self.nuke_rx.seen(now, owner) {
                    out.nuke_from = Some((owner, "bullet"));
                    log_i!("COOP", "nuke marker bullet from={} at={}", logfmt::handle(owner), logfmt::v3(pos));
                }
            }
        }
    }

    /// The explosion sound of a peer's launcher / AC-130 shell (the AC-130 cues; the 25 mm uses the 40 mm one quieter).
    fn explosion_sound(&mut self, i: &CoopVisIn, twin: Twin, pos: V3) {
        use mw2er_core::ac130_audio::{Cue, pick};
        let (cue, extra) = match twin.kind {
            TwinKind::Ac130 { gun: 0, .. } => (Cue::Exp105, 1.0),
            TwinKind::Ac130 { gun: 1, .. } => (Cue::Exp40, 1.0),
            TwinKind::Ac130 { .. } => (Cue::Exp40, 0.4),
            _ => (Cue::Exp40, 1.0),
        };
        let c = &i.cfg.coop;
        let Some(d) = crate::data::AC_DATA.get() else { return };
        let Some(sc) = d.sounds.cues.get(cue.key()) else { return };
        let dist = (pos - i.view_pos).len();
        // a shell heard from far keeps a floor (the AC-130 is loud); never louder than the gunship hears its own
        let g = peer_gain(dist, c.sound_min_m * 3.0, c.sound_max_m * 4.0, c.explosion_floor) * c.peer_gain * extra * i.cfg.ac130.audio.explosion_gain;
        if g > 0.01 {
            shared::play_ac(pick(cue, sc, &mut self.rng, g, 0));
        }
    }

    /// Peers with a known gun (marker, else the last twin) and ourselves in third person. C6: with `hold_mode = "hand"`
    /// each player's gun sits on its right-hand bone (any failed read / check draws stage A for that player that frame).
    fn world_guns_tick(&mut self, i: &CoopVisIn, ar: Option<&Arc<ArsenalData>>) {
        let c = &i.cfg.coop;
        let Some(ar) = ar else { return };
        let axis = match i.cfg.first_person.yaw_forward_axis {
            YawAxisCfg::PlusZ => Axis::PlusZ,
            _ => Axis::MinusZ,
        };
        let slot_of = |g: u32| ar.guns.iter().position(|x| x.spec.g == g);
        let now = i.now;
        let dt = self.hold.last_now.map_or(0.0, |t| (now - t).clamp(0.0, 0.25)) as f32;
        self.hold.last_now = Some(now);
        let player_ins = i.player.map_or(0, |p| p.player_ins);
        let pw = i.player.map(|p| p.chr.pos_w);
        let mut bone_src: Option<&'static str> = None;
        // C6: the hand path's inputs this tick (bone index, the vtables, the axis maps)
        let hand = c.hold_hand();
        let ctx = if hand { Some(self.hold_ctx(c, now)) } else { None };
        // C6 depth fallback: the body capsules (third person only: first person stays exactly as before)
        let body = if c.world_gun_body_occlusion && !i.fp_on { self.body_bones(now) } else { None };
        let vt43 = own_vtables().map(|v| v.0);
        for p in self.peers.iter_mut() {
            let Some((g, src)) = peer_gun(&p.markers, p.last_twin, now, c.twin_gun_fallback_s as f64) else { continue };
            if p.markers.ac130 {
                continue; // riding the gunship: no gun on the character
            }
            let Some(slot) = slot_of(g) else { continue };
            // SAFETY: the peer's ChrIns from the player set read this tick (re-read by the watch at 4 Hz); non-faulting.
            let Some((feet, q, w)) = (unsafe { crate::game::chars::read_pose(p.addr) }) else { continue };
            // review C3: a peer in another havok frame (tile) has frame-relative coordinates: never drawn here
            if pw != Some(w) {
                if p.skip_logged != Some("other_frame") {
                    p.skip_logged = Some("other_frame");
                    log_i!("COOP", "worldgun skip peer={} reason=other_frame w={w} ours={}", logfmt::handle(p.handle), pw.map_or("?".to_string(), |x| x.to_string()));
                }
                continue;
            }
            if p.skip_logged.is_some() {
                p.skip_logged = None;
                log_i!("COOP", "worldgun skip peer={} reason=none (same frame again)", logfmt::handle(p.handle));
            }
            let dist = (feet - i.view_pos).len();
            if !(dist <= c.world_gun_max_m) {
                continue;
            }
            // review C3: aim = the direction of the peer's last shot (1.5 s), else its body facing
            let aim = p.aim.filter(|(_, t)| now - *t <= AIM_HOLD_S).map(|(d, _)| d);
            let Some(fwd) = aim.and_then(horizontal_forward).or_else(|| horizontal_forward(forward_of(q, axis))) else { continue };
            let pitch = aim.map_or(0.0, aim_pitch);
            let stage_b = if hand { -1 } else { c.hand_bone };
            let (anchor, offset, bsrc) = hand_anchor(stage_b, p.addr, feet, q, c.world_gun_offset_m);
            if stage_b >= 0 {
                bone_src = Some(bsrc);
            }
            let Some(mut m) = world_gun_matrix(anchor, fwd, offset, pitch) else { continue };
            if let Some(ctx) = ctx.as_ref() {
                let who = format!("peer={}", logfmt::handle(p.handle));
                let gun = &ar.guns[slot].name;
                let f = hold_frame(ctx, p.addr, feet, grips(c, gun), &mut p.hp, &mut self.hold.g_hand, now, &who);
                if let Some(wg) = hand_frame(&mut p.hp, now, &who, f.res, c.hand_smooth) {
                    m = wg;
                }
            }
            let lod = world_gun_lod(dist, c.world_gun_lod1_m);
            let mut occl = false;
            if c.world_gun_occlusion && pw == Some(w) {
                let pts = [[15.0, 0.0, 0.0], [-15.0, 0.0, 0.0], [0.0, 0.0, 0.0]].map(|x| m_point(&m, &x));
                let blocked = pts.map(|t| {
                    let tv = V3::new(t[0], t[1], t[2]);
                    let d = tv - i.view_pos;
                    let len = d.len();
                    match d.normalize() {
                        Some(dir) if len > 0.2 => havok::map_ray(i.view_pos, w, dir, len, player_ins).is_some_and(|hit| hit < len - 0.15),
                        _ => false,
                    }
                });
                occl = mw2er_core::coop_vis::occluded(blocked);
            }
            if p.gun_logged != Some((g, src)) || now - p.gun_log_t >= WORLDGUN_LOG_S {
                p.gun_logged = Some((g, src));
                p.gun_log_t = now;
                log_i!("COOP", "worldgun peer={} gun={} g={g} src={src} lod={lod} occl={} dist={dist:.1}m", logfmt::handle(p.handle), ar.guns[slot].name, occl as u8);
            }
            if !occl {
                self.world_guns.push(WorldGunDraw { slot: slot as u16, lod, model: m });
                if let (Some(b), Some(vt)) = (body.as_deref(), vt43) {
                    let who = format!("peer={}", logfmt::handle(p.handle));
                    let _ = body_capsules(b, vt, p.addr, feet, horizontal_forward(forward_of(q, axis)), &mut p.hp, &who, &mut self.world_occl);
                }
            }
        }
        // ourselves in third person (the MW2-mode marker hides ER's weapon on every PC, ours included, with a peer
        // present; solo only with hold_mode = "hand" or er_stance on, else ER's weapon stays visible next to the gun)
        // C6 part 17: this frame's guard align (set below when our gun is drawn)
        self.hold.guard_align = None;
        if c.world_gun_self_tp
            && i.mode_on
            && !i.fp_on
            && !i.ac130_active
            && let (Some(pl), Some(g)) = (i.player, i.gun_g)
            && let Some(slot) = slot_of(g)
            && let Some(fwd) = i.cam_fwd.and_then(horizontal_forward)
        {
            // C6: on Torrent with `hold_mounted = "hide"` (hand mode only) our world gun is not drawn
            let hide_mounted = hand && i.mounted && c.hold_mounted == "hide";
            if hide_mounted != self.hold.mounted_logged {
                self.hold.mounted_logged = hide_mounted;
                log_i!("HOLD", "mounted hide={}", hide_mounted as u8);
            }
            // SAFETY: our own ChrIns (main player) this tick; non-faulting reads.
            let q = unsafe { crate::game::chars::read_pose(main_player()) }.map_or([0.0, 0.0, 0.0, 1.0], |x| x.1);
            let stage_b = if hand { -1 } else { c.hand_bone };
            let (anchor, offset, bsrc) = hand_anchor(stage_b, main_player(), pl.chr.pos, q, c.world_gun_offset_m);
            if stage_b >= 0 {
                bone_src = Some(bsrc);
            }
            if !hide_mounted && let Some(mut m) = world_gun_matrix(anchor, fwd, offset, 0.0) {
                if let Some(ctx) = ctx.as_ref() {
                    let me = main_player();
                    let hs = &mut self.hold;
                    let gun = &ar.guns[slot].name;
                    let gr = grips(c, gun);
                    // C6 grip 3: one line per gun / value change (which grip the weapon frame uses)
                    let gl = format!("gun={gun} src={} offset={} rot={} rake={:.1}", gr.2, floats_text(&gr.0.0), floats_text(&gr.0.1), c.weapon_rake_deg);
                    if c.weapon_source() && gl != hs.grip_logged {
                        log_i!("HOLD", "grip who=self {gl} (weapon frame; [coop.grip.<gun>] keys override)");
                        hs.grip_logged = gl;
                    }
                    let f = hold_frame(ctx, me, pl.chr.pos, gr, &mut hs.self_hp, &mut hs.g_hand, now, "self");
                    let h_axes = f.hand.map(|h| h.h);
                    // C6 attach 2: `weapon_axes = "auto"` learns the map while the stance holds ER's weapon (self only)
                    hs.posed_t0 = if i.stance_posed { Some(hs.posed_t0.unwrap_or(now)) } else { None };
                    if c.weapon_axes == "auto"
                        && hs.learned.is_none()
                        && hs.posed_t0.is_some_and(|t| now - t >= POSE_SETTLE_S)
                        && let Some(w) = f.weapon
                        && let Some(body) = horizontal_forward(forward_of(q, axis))
                    {
                        hs.vote.observe(&w, body, mw2er_core::vec3::UP);
                        if let Some((t, n)) = hs.vote.result()
                            && let Ok(a) = parse_axes(&t)
                        {
                            // C6 grip 3: only the rig map is ever used (the vote alone could learn a backwards map)
                            let ok = axes_auto_accept(&t);
                            log_i!(
                                "HOLD",
                                "weapon axes auto axes={t} votes={n}/{} accepted={} (the rig map is {WEAPON_AXES_RIG}; {})",
                                hs.vote.total(),
                                ok as u8,
                                if ok { "confirmed" } else { "rejected: kept the rig map" }
                            );
                            hs.auto_rejected = !ok;
                            hs.learned = Some(if ok { (t, a) } else { (WEAPON_AXES_RIG.to_string(), parse_axes(WEAPON_AXES_RIG).unwrap_or(a)) });
                        }
                    }
                    weapon_probe(hs, c, i, me, &f);
                    // C6 pose 3: the HOLD axes window re-arms each time the stance settles again (after its chords)
                    if i.stance_posed && !hs.prev_posed && hs.axes_done && hs.axes_rearms < AXES_REARMS {
                        hs.axes_rearms += 1;
                        hs.axes_done = false;
                        hs.axes_t0 = None;
                        hs.axes_next = 0.0;
                    }
                    hs.prev_posed = i.stance_posed;
                    // C6 part 17 addendum: the guard frame only replaces a hand-path gun (never stage A)
                    let mut hand_drawn = false;
                    if let Some(wg) = hand_frame(&mut hs.self_hp, now, "self", f.res, c.hand_smooth) {
                        m = wg;
                        hand_drawn = true;
                        // the HOLD axes lines (once per session, with the probe on): each hand axis in world
                        if c.bone_probe && !hs.axes_done && let Some(h) = h_axes {
                            let t0 = *hs.axes_t0.get_or_insert(now);
                            if now - t0 > AXES_S {
                                hs.axes_done = true;
                            } else if now >= hs.axes_next {
                                hs.axes_next = now + AXES_EVERY_S;
                                let [hx, hy, hz] = hand_axes_world(&h);
                                // C6 pose 3: the grip and the left hand's distance to ER's weapon origin (both hands on
                                // the weapon: ~0.3 m; a one-hand carry: ~0.5 m)
                                // SAFETY: our own ChrIns this tick; non-faulting reads only.
                                let lh = unsafe { crate::game::chars::read_hand(me, LEFT_HAND_BONE) }
                                    .and_then(|r| mw2er_core::hold::hand_world(&r.e, &r.m, pl.chr.pos).ok())
                                    .map(|(lh, _)| translation(&lh));
                                let lh_d = match (lh, f.weapon) {
                                    (Some(l), Some(w)) => format!("{:.3}", (l - translation(&w)).len()),
                                    _ => "?".to_string(),
                                };
                                log_i!(
                                    "HOLD",
                                    "axes hx={} hy={} hz={} cam_fwd={} ads={} arm={} lh_d={lh_d} posed={} rearm={}",
                                    v3_text(hx),
                                    v3_text(hy),
                                    v3_text(hz),
                                    i.cam_fwd.map_or("?".to_string(), v3_text),
                                    (i.ads >= 0.5) as u8,
                                    i.stance_arm,
                                    i.stance_posed as u8,
                                    hs.axes_rearms
                                );
                            }
                        }
                    }
                    // C6 part 17 addendum (docs 17.12): in a DETECTED guard the gun takes the guard frame on ER's weapon pose
                    // (the barrel along the body's forward from the right fist, the pistol grip still on the fist), blended
                    // in over the detection and out at the release; the align below keeps only the residual (over the max:
                    // the raise, as before). `guard_axes = "auto"` learns the row's frame from guards held standing.
                    hs.guard_corr = None;
                    if !i.stance_guard_active {
                        hs.guard_frame_logged = false;
                    }
                    let body = horizontal_forward(forward_of(q, axis));
                    if c.guard_frame_on()
                        && c.guard_axes == "auto"
                        && i.stance_guard_align
                        && i.stance_guard_standing
                        && hs.guard_clamp.settled(now)
                        && let (Some(w), Some(bf)) = (f.weapon, body)
                        && let Some((lf, n, spread)) = hs.guard_learn.observe(now, f.row, &w, bf, mw2er_core::vec3::UP)
                    {
                        let (pa, pr) = mw2er_core::hold::frame_pin(&lf);
                        log_i!(
                            "HOLD",
                            "guard frame auto row={} axes={pa} rot={} n={n} spread={spread:.1} vs_builtin={:.1} (learned from the detected guards held standing; set [coop] guard_axes = \"{pa}\" and guard_rot_deg = {} to keep it)",
                            f.row,
                            floats_text(&pr),
                            mw2er_core::hold::frame_diff_deg(&lf, &mw2er_core::hold::guard_frame_builtin()),
                            floats_text(&pr)
                        );
                    }
                    let gframe = if c.guard_frame_on() && i.stance_guard_align {
                        use mw2er_core::hold::GuardFrameSrc;
                        let fr = match c.guard_frame_fixed() {
                            Some(x) => (x, GuardFrameSrc::Fixed),
                            None => hs.guard_learn.frame(f.row).map_or((mw2er_core::hold::guard_frame_builtin(), GuardFrameSrc::Builtin), |x| (x, GuardFrameSrc::Auto)),
                        };
                        hs.guard_last = Some(fr);
                        Some(fr)
                    } else {
                        None
                    };
                    hs.guard_u = ads_blend_step(hs.guard_u, gframe.is_some() && f.weapon.is_some() && hand_drawn, dt);
                    if hs.guard_u <= 0.0 {
                        hs.guard_last = None;
                    }
                    if hs.guard_u > 0.0
                        && hand_drawn
                        && let (Some(w), Some((gf, gsrc))) = (f.weapon, gframe.or(hs.guard_last))
                    {
                        let (go, gr) = mw2er_core::hold::frame_grip(mw2er_core::hold::grip_point_in(gun), &gf, &ctx.weapon_axes);
                        let mg = gun_from_hand(&w, &grip_matrix(go, gr, &ctx.weapon_axes));
                        m = blend_mat34(&m, &mg, hs.guard_u);
                        hs.guard_corr = Some(gsrc.label());
                        if gframe.is_some() && !hs.guard_frame_logged {
                            hs.guard_frame_logged = true;
                            log_i!(
                                "HOLD",
                                "guard frame src={} gun={gun} row={} offset={} rot={} fwd_err={} (the barrel along the body's forward from the right fist on ER's guard; aim_align keeps the residual, max {:.0})",
                                gsrc.label(),
                                f.row,
                                floats_text(&go),
                                floats_text(&gr),
                                body.and_then(|bf| mw2er_core::hold::frame_fwd_err_deg(&w, &gf, bf)).map_or("?".to_string(), |d| format!("{d:.1}")),
                                c.tp_guard_align_max_deg
                            );
                        }
                    }
                    // C6 aim 4 (aim4.md 3.3): in ER's crossbow aim the gun turns about its pistol grip (in the fist) onto
                    // the camera's aim point, blended in / out over 0.12 s, at most 20 deg (else ER's weapon frame as is)
                    // C6 part 17 (docs 17.5): in a third-person guard episode the gun turns onto the aim line once ER's
                    // guard pose is detected (at most tp_guard_align_max_deg); the two poses are exclusive by config
                    let guard_cfg = c.guard_pose_on();
                    let align_on = i.stance_aim_on || i.stance_guard_align;
                    let align_max = if guard_cfg { c.tp_guard_align_max_deg } else { mw2er_core::hold::AIM_ALIGN_MAX_DEG };
                    hs.aim_u = ads_blend_step(hs.aim_u, align_on, dt);
                    if !align_on {
                        hs.aim_logged = false;
                        hs.aim_clamp_logged = false;
                    }
                    if !i.stance_guard_active {
                        hs.guard_clamp = mw2er_core::hold::GuardClamp::default();
                        hs.guard_clamp_lines = 0;
                    }
                    hs.guard_align = None;
                    // C6 round 7 review: shoulder on ER's confirmed aim, not on the 20 deg align clamp (ER plays no arm pose)
                    let was_nopose = hs.aim_nopose;
                    hs.aim_nopose = if guard_cfg {
                        // C6 part 17: the guard raises the gun only when its pose was not detected in time or does not align
                        mw2er_core::hold::guard_nopose(c.tp_ads_hold == "aim", i.stance_guard_active, i.stance_guard_fallback, hs.guard_clamp.clamped)
                    } else {
                        mw2er_core::hold::aim_nopose_step(hs.aim_nopose, c.tp_ads_hold == "aim", i.stance_aim_on, i.stance_aim_active, hs.aim_u, i.stance_carrier)
                    };
                    if hs.aim_nopose && !was_nopose {
                        if guard_cfg {
                            log_i!(
                                "HOLD",
                                "guard hold_fallback gun={gun} why={} (tp_ads_hold = \"aim\" raises our gun along the camera)",
                                if hs.guard_clamp.clamped { "clamp" } else { "not_detected" }
                            );
                        } else {
                            log_i!(
                                "HOLD",
                                "aim hold_fallback gun={gun} (ER's aim plays no arm pose: tp_ads_hold = \"aim\" shoulders our gun along the camera)"
                            );
                        }
                    }
                    // C6 part 17 review: this frame's guard align (Some(true) = over the limit) for the debounced clamp
                    let mut guard_over: Option<(f32, bool)> = None;
                    if hs.aim_u > 0.0
                        && let Some(cf) = i.cam_fwd.and_then(|v| v.normalize())
                    {
                        let pivot = mw2er_core::hold::gun_point(&m, mw2er_core::hold::grip_point_in(gun));
                        let target = i.view_pos + cf * mw2er_core::hold::AIM_ALIGN_DIST_M;
                        match mw2er_core::hold::aim_align(&m, pivot, target, hs.aim_u, align_max) {
                            Ok((aligned, deg)) => {
                                m = aligned;
                                if i.stance_guard_align {
                                    guard_over = Some((deg, false));
                                }
                                if i.stance_guard_align && !hs.aim_logged && hs.guard_clamp.settled(now) {
                                    hs.aim_logged = true;
                                    log_i!(
                                        "HOLD",
                                        "guard align gun={gun} deg={deg:.1} max={align_max:.0} rake={:.1} corr={} (the barrel vs the camera's aim point on ER's guard pose; turned about the grip)",
                                        c.weapon_rake_deg,
                                        hs.guard_corr.unwrap_or("off")
                                    );
                                }
                                if i.stance_aim_on && !hs.aim_logged {
                                    hs.aim_logged = true;
                                    log_i!(
                                        "HOLD",
                                        "aim align gun={gun} deg={deg:.1} rake={:.1} (the barrel vs the camera's aim point at {:.0} m on ER's aim pose; turned about the grip)",
                                        c.weapon_rake_deg,
                                        mw2er_core::hold::AIM_ALIGN_DIST_M
                                    );
                                }
                            }
                            Err(deg) => {
                                // C6 part 17: a detected guard over the limit feeds the debounced clamp (below)
                                if i.stance_guard_align {
                                    guard_over = Some((deg, true));
                                }
                                if !i.stance_guard_align && !hs.aim_clamp_logged {
                                    hs.aim_clamp_logged = true;
                                    log_i!(
                                        "HOLD",
                                        "aim align_clamped gun={gun} deg={deg:.1} max={:.0} ({})",
                                        mw2er_core::hold::AIM_ALIGN_MAX_DEG,
                                        if hs.aim_nopose { "the gun shoulders along the camera: tp_ads_hold = aim" } else { "drawn on ER's weapon frame as is" }
                                    );
                                }
                            }
                        }
                    }
                    // C6 part 17 review: the clamp waits GUARD_ALIGN_SETTLE_S after detection (ER's raise in motion), sets
                    // after GUARD_CLAMP_ON_S over the limit, clears after GUARD_CLAMP_OFF_S under it (a hit reaction)
                    if i.stance_guard_active {
                        let was = hs.guard_clamp.clamped;
                        let now_c = hs.guard_clamp.step(now, i.stance_guard_align, guard_over.map(|g| g.1));
                        if hs.guard_clamp.settled(now)
                            && let Some((deg, _)) = guard_over
                        {
                            hs.guard_align = Some((deg, now_c));
                        }
                        if now_c != was && hs.guard_clamp_lines < GUARD_CLAMP_LINES {
                            hs.guard_clamp_lines += 1;
                            let deg = guard_over.map_or("?".to_string(), |g| format!("{:.1}", g.0));
                            if now_c {
                                log_i!(
                                    "HOLD",
                                    "guard align_clamped gun={gun} deg={deg} max={align_max:.0} ({})",
                                    if c.tp_ads_hold == "aim" { "the gun raises along the camera: tp_ads_hold = aim" } else { "drawn on ER's weapon frame as is" }
                                );
                            } else {
                                log_i!("HOLD", "guard align_unclamped gun={gun} deg={deg} max={align_max:.0} (back on ER's guard pose)");
                            }
                        }
                    }
                    // C6 (docs 6): `tp_ads_hold = "aim"`: blend to the stage-A pose aimed along the camera while MW2 ADS
                    // (C6 aim 4: not during ER's crossbow aim episode; round 7: inside one when ER kept the carry)
                    // C6 part 17: a guard episode counts like an aim episode (the raise only as its fallback)
                    if mw2er_core::hold::ads_hold_aim(c.tp_ads_hold == "aim", i.stance_aim_active || i.stance_guard_active, hs.aim_u, hs.aim_nopose) {
                        hs.ads_u = ads_blend_step(hs.ads_u, i.ads >= 0.5, dt);
                        if hs.ads_u > 0.0
                            && let Some(aimed) = world_gun_matrix(pl.chr.pos, fwd, c.world_gun_offset_m, i.cam_fwd.map_or(0.0, aim_pitch))
                        {
                            m = blend_mat34(&m, &aimed, hs.ads_u);
                        }
                    } else {
                        hs.ads_u = 0.0;
                    }
                }
                let lod = world_gun_lod((pl.chr.pos - i.view_pos).len(), c.world_gun_lod1_m);
                self.world_guns.push(WorldGunDraw { slot: slot as u16, lod, model: m });
            }
        }
        // C6 depth fallback: our own body in third person (it hides peers' guns behind us and our own gun from behind)
        if let (Some(b), Some(vt), Some(pl)) = (body.as_deref(), vt43, i.player)
            && i.mode_on
            && !i.ac130_active
            && !self.world_guns.is_empty()
        {
            let me = main_player();
            // SAFETY: our own ChrIns (main player) this tick; non-faulting reads.
            let q = unsafe { crate::game::chars::read_pose(me) }.map(|x| x.1);
            let facing = q.and_then(|q| horizontal_forward(forward_of(q, axis)));
            self.self_chest = body_capsules(b, vt, me, pl.chr.pos, facing, &mut self.hold.self_hp, "self", &mut self.world_occl).map(|v| [v.x, v.y, v.z]);
        }
        if bone_src.is_some() && bone_src != self.bone_logged {
            self.bone_logged = bone_src;
            log_i!("COOP", "worldgun bone={} src={}", c.hand_bone, bone_src.unwrap_or("?"));
        }
    }

    /// C6: this tick's shared hand-path inputs (the axis maps are parsed once per text).
    fn hold_ctx(&mut self, c: &mw2er_core::config::CoopCfg, now: f64) -> HoldCtx {
        let bone = self.bone_index(c, now);
        let hs = &mut self.hold;
        if hs.axes_src != c.hand_axes {
            hs.axes_src = c.hand_axes.clone();
            hs.axes = parse_axes(&c.hand_axes).ok();
        }
        if hs.waxes_src != c.weapon_axes {
            hs.waxes_src = c.weapon_axes.clone();
            hs.waxes = parse_axes(&c.weapon_axes).ok();
        }
        // C6 grip 3: an explicit map > "auto" (only ever the rig map: confirmed by the vote or kept) > the rig map
        let weapon_axes = if c.weapon_axes == "auto" { hs.learned.as_ref().map(|x| x.1) } else { hs.waxes }
            .or_else(|| parse_axes(WEAPON_AXES_RIG).ok())
            .unwrap_or(mw2er_core::xform::MAT34_IDENTITY);
        let weapon_axes_xbow = parse_axes(&c.weapon_axes_xbow).or_else(|_| parse_axes(WEAPON_AXES_RIG)).unwrap_or(mw2er_core::xform::MAT34_IDENTITY);
        HoldCtx { bone, vts: own_vtables(), wvts: own_weapon_vtables(), weapon: c.weapon_source(), hand_axes: hs.axes, weapon_axes, weapon_axes_xbow }
    }

    /// C6 depth fallback: the FLVER index of every `BODY_SLOTS` name (havok names matched to the FLVER entries by their
    /// model-pose translation, as the hand bone; cached per session, retried every second, `HOLD body bones none` after
    /// BODY_LOOKUP_TRIES misses). `None` = not known (yet): no capsules, the guns draw as before.
    fn body_bones(&mut self, now: f64) -> Option<Vec<Option<u32>>> {
        let hs = &mut self.hold;
        match &hs.body_bones {
            Some(Ok(v)) => return Some(v.clone()),
            Some(Err(_)) => return None,
            None => {}
        }
        if now < hs.body_next {
            return None;
        }
        hs.body_next = now + BODY_LOOKUP_EVERY_S;
        hs.body_tries += 1;
        let me = main_player();
        // SAFETY: our own ChrIns this tick; non-faulting reads only.
        let r = unsafe { crate::game::chars::find_skeleton_bones(me, &BODY_SLOTS) }.and_then(|(found, all)| {
            // SAFETY: as above.
            let (_, e) = unsafe { crate::game::chars::read_palette(me, mw2er_core::hold::N_MAX as usize) }.ok_or("array")?;
            // SAFETY: as above.
            let chr = unsafe { crate::game::chars::read_chr_matrix(me) }.map(|x| x.2);
            let idx: Vec<Option<u32>> = found
                .iter()
                // SAFETY: as above.
                .map(|k| k.and_then(|k| unsafe { crate::game::chars::read_model_pose_t(me, k) }).and_then(|t| match_flver_any(t, &e, chr.as_ref())).map(|x| x.0 as u32))
                .collect();
            if idx[0].is_none() {
                return Err(if found[0].is_none() { "no_pelvis_name" } else { "no_pelvis_match" });
            }
            let txt: Vec<String> = BODY_SLOTS.iter().zip(&idx).map(|(n, i)| format!("{n}:{}", i.map_or("-".to_string(), |i| i.to_string()))).collect();
            log_i!("HOLD", "body bones found={}/{} flver={} (skeleton n={})", idx.iter().flatten().count(), BODY_SLOTS.len(), txt.join(","), all.len());
            Ok((idx, all))
        });
        match r {
            Ok((idx, all)) => {
                // the whole name list once (bone_probe): the next run can fix a name that did not match
                if self.probe_t0.is_some() || self.hold.body_tries == 1 {
                    for (k, chunk) in all.chunks(40).enumerate() {
                        let from = k * 40;
                        let names: Vec<String> = chunk.iter().enumerate().map(|(j, n)| format!("{}:{n}", from + j)).collect();
                        log_i!("HOLD", "body names {}", names.join(" "));
                    }
                }
                self.hold.body_bones = Some(Ok(idx.clone()));
                Some(idx)
            }
            Err(why) => {
                if self.hold.body_tries >= BODY_LOOKUP_TRIES {
                    self.hold.body_bones = Some(Err(why));
                    log_w!("HOLD", "body bones none reason={why} tries={} (no body capsules this session: world guns draw as before)", self.hold.body_tries);
                }
                None
            }
        }
    }

    /// C6 (docs 4.2): the FLVER index of the hand bone: `hand_bone` when set, else the name lookup (`hand_bone_auto`:
    /// R_Weapon / R_Hand in the havok skeleton, matched to the FLVER entry by its model-pose translation; cached per session,
    /// retried every second, `HOLD bone name=none` after BONE_LOOKUP_TRIES misses). C6 attach 2: while the lookup runs or
    /// after it found nothing, the right hand of the 2026-10-21 probe (`DEFAULT_HAND_BONE` 57, the same c0000 FLVER on
    /// every PC) instead of no bone.
    fn bone_index(&mut self, c: &mw2er_core::config::CoopCfg, now: f64) -> Result<u32, Reason> {
        if c.hand_bone >= 0 {
            return Ok(c.hand_bone as u32);
        }
        if !c.hand_bone_auto {
            return Ok(DEFAULT_HAND_BONE);
        }
        match self.hold.bone_auto {
            Some(Ok(i)) => return Ok(i),
            Some(Err(_)) => return Ok(DEFAULT_HAND_BONE),
            None => {}
        }
        if now < self.hold.bone_next {
            return Ok(DEFAULT_HAND_BONE);
        }
        self.hold.bone_next = now + BONE_LOOKUP_EVERY_S;
        self.hold.bone_tries += 1;
        let me = main_player();
        // SAFETY: our own ChrIns this tick; non-faulting reads only.
        let r = unsafe { crate::game::chars::find_skeleton_bone(me, &HAND_BONE_NAMES) }.and_then(|(k, name, cnt_off, n)| {
            // SAFETY: as above.
            let t = unsafe { crate::game::chars::read_model_pose_t(me, k) }.ok_or("pose")?;
            // SAFETY: as above.
            let (_, e) = unsafe { crate::game::chars::read_palette(me, mw2er_core::hold::N_MAX as usize) }.ok_or("array")?;
            // SAFETY: as above.
            let chr = unsafe { crate::game::chars::read_chr_matrix(me) }.map(|x| x.2);
            let (fi, err, space) = match_flver_any(t, &e, chr.as_ref()).ok_or("match")?;
            log_i!("HOLD", "bone name={name} hk={k} flver={fi} err={:.1}mm src={space} n_hk={n} cnt_off=0x{cnt_off:x} n_flver={}", err * 1000.0, e.len());
            Ok(fi as u32)
        });
        match r {
            Ok(i) => {
                self.hold.bone_auto = Some(Ok(i));
                Ok(i)
            }
            Err(why) => {
                if self.hold.bone_tries >= BONE_LOOKUP_TRIES {
                    self.hold.bone_auto = Some(Err(why));
                    log_w!(
                        "HOLD",
                        "bone name=none reason={why} tries={} using flver={DEFAULT_HAND_BONE} (the probe's right hand; set [coop] hand_bone to override)",
                        self.hold.bone_tries
                    );
                }
                Ok(DEFAULT_HAND_BONE)
            }
        }
    }

    /// Review C3 stage B / C6 (docs 4.2): logs ER's bone array of our own character for BONE_PROBE_S after MW2 mode turns
    /// on (once per session, `[coop] bone_probe`), so the right-hand index can be read off the log: the header (count,
    /// both vtables checked, the chr matrix translation, the feet), then per bone the model-space T and the row lengths.
    /// Read-only.
    fn bone_probe(&mut self, i: &CoopVisIn) {
        let on_edge = i.mode_on && !self.prev_mode_on;
        self.prev_mode_on = i.mode_on;
        if !i.cfg.coop.bone_probe || self.probe_done {
            return;
        }
        if on_edge && self.probe_t0.is_none() {
            self.probe_t0 = Some(i.now);
            self.probe_next = i.now;
        }
        let Some(t0) = self.probe_t0 else { return };
        if i.now - t0 > BONE_PROBE_S {
            self.probe_done = true;
            log_i!("COOP", "bone probe done (set [coop] hand_bone to the right-hand index for hold_mode = \"hand\" if HOLD bone found none)");
            return;
        }
        if i.now < self.probe_next {
            return;
        }
        self.probe_next = i.now + BONE_PROBE_EVERY_S;
        let me = main_player();
        // SAFETY: our own ChrIns this tick; non-faulting reads only.
        let (pal, pose, chr) = unsafe {
            (
                crate::game::chars::read_palette(me, crate::game::chars::PALETTE_MAX),
                crate::game::chars::read_pose(me),
                crate::game::chars::read_chr_matrix(me),
            )
        };
        let Some((a, e)) = pal else {
            log_i!("COOP", "bone probe t={:.1} unreadable (model item / bone array pointer)", i.now - t0);
            return;
        };
        let feet = pose.map_or(V3::ZERO, |x| x.0);
        let base = crate::game::c3_read::exe_base();
        let (vt43, vt44) = own_vtables_rva();
        let ok = |want: Option<usize>, got: usize| match want {
            Some(v) if got == base + v => "ok",
            Some(_) => "bad",
            None => "unknown",
        };
        let chr_vt = chr.map_or("unreadable", |(_, vt, _)| ok(vt44, vt));
        let mt = chr.map_or("?".to_string(), |(_, _, m)| v3_text(V3::new(m[12], m[13], m[14])));
        log_i!(
            "COOP",
            "bone probe t={:.1} n={} arr=0x{:x} vt={} data=0x{:x} chr_vt={chr_vt} M_T={mt} feet={}",
            i.now - t0,
            a.n,
            a.arr,
            ok(vt43, a.vt),
            a.data,
            v3_text(feet)
        );
        for from in (0..e.len()).step_by(32) {
            log_i!("COOP", "bone probe bones={}-{} {}", from, (from + 31).min(e.len() - 1), probe_chunk(&e, from, from + 32));
        }
    }

    /// Review C3: the view shake of a peer's 105 mm impact (distance-scaled, the AC-130's `[ac130.shake]` look; added
    /// to the view-fx angles like the nuke's shake). Zero with `[ac130.shake] enabled = false`.
    pub fn shake(&self, now: f64, cfg: &Config) -> mw2er_core::ac130_shake::ShakeOut {
        use mw2er_core::ac130_shake::{QuakeSample, ShakeOut, sample};
        let Some((t0, amp, seed)) = self.quake else { return ShakeOut::ZERO };
        if !cfg.ac130.shake.enabled || !cfg.coop.peer_effects {
            return ShakeOut::ZERO;
        }
        let p = cfg.ac130.shake.params(1.0);
        sample(&[QuakeSample { amp: 0.5 * amp, duration_s: 0.8, age_s: (now - t0) as f32, seed }], &p)
    }

    /// Tracer segments (a, b, alpha, head dot) for the HUD. Review 2026-10-24: with `tracer_occlusion` only the parts
    /// the map rays found visible from the camera, clipped at the shot's first map hit (`tracer_occl`); a tracer the
    /// rays have not reached yet draws whole, as before.
    pub fn segments(&self, now: f64, cfg: &Config) -> Vec<([f32; 3], [f32; 3], f32, bool)> {
        if !cfg.coop.peer_tracers || !cfg.coop.peer_effects {
            return Vec::new();
        }
        let a3 = |v: V3| [v.x, v.y, v.z];
        let mut out = Vec::new();
        for s in self.tracker.segments(now).into_iter().take(64) {
            let alpha = tracer_alpha(s.age);
            if alpha <= 0.0 {
                continue;
            }
            if !cfg.coop.tracer_occlusion {
                out.push((a3(s.a), a3(s.b), alpha, true));
                continue;
            }
            let pc = self.tocc.pieces(&PathIn { key: s.key, a: s.a, b: s.b, dir: s.dir });
            let n = pc.parts.len();
            for (k, (x, y)) in pc.parts.into_iter().enumerate() {
                out.push((a3(x), a3(y), alpha, pc.tip && k + 1 == n));
            }
        }
        out
    }

    /// Review 2026-10-24: this tick's map rays for the peers' tracers and flashes (from the HUD's eye, in our havok
    /// frame; only peers in our frame give tracers). No physics world = no rays: everything draws as before.
    fn occlusion_tick(&mut self, now: f64, i: &CoopVisIn, pl: &PlayerSample) {
        let c = &i.cfg.coop;
        if !c.tracer_occlusion {
            self.tocc.clear();
            return;
        }
        let segs: Vec<PathIn> = if c.peer_tracers {
            self.tracker.segments(now).into_iter().take(64).map(|s| PathIn { key: s.key, a: s.a, b: s.b, dir: s.dir }).collect()
        } else {
            Vec::new()
        };
        let (w, ins) = (pl.chr.pos_w, pl.player_ins);
        let rays_ok = ins != 0 && havok::phys_world() != 0;
        let mut ray = |o: V3, d: V3, r: f32| havok::map_ray(o, w, d, r, ins);
        self.tocc.update(i.view_pos, &segs, c.tracer_occlusion_rays, CLIP_RANGE_M, if rays_ok { Some(&mut ray) } else { None });
        for s in &segs {
            let pc = self.tocc.pieces(s);
            self.tocc.note_drawn(s, &pc);
        }
        // a flash: one ray at its first tick (it lives FLASH_S)
        if rays_ok && c.peer_flashes {
            for f in self.flashes.iter_mut().filter(|f| f.3.is_none()) {
                let vis = point_visible(i.view_pos, f.0, FLASH_TOL_M.max(0.5 * f.2), &mut ray);
                self.tocc.stats.rays += 1;
                if !vis {
                    self.flashes_hidden += 1;
                }
                f.3 = Some(vis);
            }
        }
    }

    /// Explosion flashes (position, alpha, size m) for the HUD.
    pub fn flashes(&self, now: f64, cfg: &Config) -> Vec<([f32; 3], f32, f32)> {
        // review C3: off by default (ER draws the explosion twin's own SFX here; the flash would stack on it)
        if !cfg.coop.peer_effects || !cfg.coop.peer_flashes {
            return Vec::new();
        }
        self.flashes
            .iter()
            .filter(|f| !(cfg.coop.tracer_occlusion && f.3 == Some(false)))
            .map(|(p, t, s, _)| ([p.x, p.y, p.z], (1.0 - (now - t) / FLASH_S).clamp(0.0, 1.0) as f32, *s))
            .collect()
    }
}

/// C6: this build's bone-array / chr-matrix vtable RVAs (`None` without a verified value or before the gate).
fn own_vtables_rva() -> (Option<usize>, Option<usize>) {
    crate::version_gate::own().map_or((None, None), |o| (o.mtx43_array_vtable, o.mtx44_chr_vtable))
}

/// C6: both vtables as live addresses (`None` = hand mode falls back with `reason=exe`).
fn own_vtables() -> Option<(usize, usize)> {
    let (a, b) = own_vtables_rva();
    let base = crate::game::c3_read::exe_base();
    match (a, b) {
        (Some(a), Some(b)) if base != 0 => Some((base + a, base + b)),
        _ => None,
    }
}

/// C6 hide: the address of an SpEffectParam row (read-only lookup: a pointer turned into an integer, never a `&mut`).
fn sp_row_addr(id: u32) -> Option<usize> {
    use eldenring::cs::{SoloParamRepository, SpEffectParam};
    use fromsoftware_shared::FromStatic;
    // SAFETY: main thread after the boot gate (as `game::params::check_coop`); the repository lives for the process.
    let repo = unsafe { SoloParamRepository::instance() }.ok()?;
    repo.get::<SpEffectParam>(id).map(|r| r as *const _ as usize)
}

const _: () = assert!(std::mem::size_of::<eldenring::param::SP_EFFECT_PARAM_ST>() == mw2er_core::coop_vis::SP_ROW_SIZE);

/// C6 hide: the HIDE_FIELDS of the row as raw bits (non-faulting).
fn read_hide(addr: usize) -> Option<Vec<u32>> {
    HIDE_FIELDS
        .iter()
        .map(|f| {
            // SAFETY: non-faulting reads of plain integers inside a live param row.
            unsafe {
                match f.width {
                    1 => crate::game::read::<u8>(addr + f.off).map(u32::from),
                    2 => crate::game::read::<u16>(addr + f.off).map(u32::from),
                    _ => crate::game::read::<u32>(addr + f.off),
                }
            }
        })
        .collect()
}

/// C6 hide: writes the patch (`to`) or the package values (`from`) through `game::write`; true when every write went in.
fn write_hide(addr: usize, patch: bool) -> bool {
    let mut ok = true;
    for f in HIDE_FIELDS {
        let v = if patch { f.to } else { f.from };
        // SAFETY: a field of a live SpEffectParam row (heap); game::write checks the page, never faults, and is version-gated.
        // The game reads an effect's row when it evaluates it (the marker is re-applied after a change).
        ok &= unsafe {
            match f.width {
                1 => crate::game::write::<u8>(addr + f.off, v as u8),
                2 => crate::game::write::<u16>(addr + f.off, v as u16),
                _ => crate::game::write::<u32>(addr + f.off, v),
            }
        };
    }
    ok
}

/// C6 attach 2: the weapon-model vtable RVAs (asm, parts, location) of this build.
fn weapon_vtables_rva() -> Option<(usize, usize, usize)> {
    let o = crate::version_gate::own().ok()?;
    Some((o.chr_asm_model_vtable?, o.parts_model_vtable?, o.mtx44_entity_vtable?))
}

/// C6 attach 2: the four vtables of the weapon-model chain as live addresses (`None` = the weapon model is not read).
fn own_weapon_vtables() -> Option<WeaponVts> {
    let (asm, parts, ent) = weapon_vtables_rva()?;
    let arr = own_vtables_rva().0?;
    let base = crate::game::c3_read::exe_base();
    (base != 0).then_some(WeaponVts { asm: base + asm, parts: base + parts, ent: base + ent, arr: base + arr })
}

/// C6 attach 2: ER's active right weapon model of `chr` -> its world matrix (orthonormal), checked against the hand bone
/// (`near`: the hand within WEAPON_NEAR_M, else the feet within HAND_MAX_M when the hand did not read).
fn weapon_pose(chr: usize, wvts: Option<WeaponVts>, near: V3, max_m: f32) -> Result<(Mat34, mw2er_core::hold::WeaponSrc, f32), Reason> {
    let v = wvts.ok_or(Reason::WeaponVtable)?;
    // SAFETY: a live PlayerIns of this tick (ours or a peer's from the player set); non-faulting reads only.
    let r = match unsafe { crate::game::chars::read_weapon_model(chr, crate::game::chars::ASM_SLOT_RIGHT0) } {
        Ok(r) => r,
        Err("none") => return Err(Reason::NoWeapon),
        Err(_) => return Err(Reason::WeaponRead),
    };
    let w = WeaponIn {
        vt_ok: r.asm_vt == v.asm && r.parts_vt == v.parts && r.owner_ok,
        root: r.root.filter(|_| r.ent_vt == v.ent),
        e0: r.e0.filter(|_| r.arr_vt == v.arr),
        n: r.n,
    };
    weapon_world(&w, near, max_m)
}

/// C6 grip 3: (weapon-frame grip, hand-bone grip, the weapon grip's source "table" | "config" | "mixed"); C6 part 16.7:
/// the grip on the carrier crossbow's weapon frame (its own table, `[coop.grip.<gun>.xbow]`).
type Grips = (([f32; 3], [f32; 3]), ([f32; 3], [f32; 3]), &'static str, ([f32; 3], [f32; 3]));

fn grips(c: &mw2er_core::config::CoopCfg, gun: &str) -> Grips {
    // C6 part 17 addendum (docs 17.12): the polearm stance carries its own grips (long along the shaft, short ahead)
    let (wo, wr, src) = if c.polearm_grip_on() { c.weapon_grip_polearm(gun) } else { c.weapon_grip_for(gun) };
    let (xo, xr, _) = c.weapon_grip_xbow(gun);
    ((wo, wr), c.grip_for(gun), src, (xo, xr))
}

/// C6 depth fallback: one character's body capsules (appended to `out`, at most `body_occl::MAX_CAPSULES` in all) and
/// its rate-limited `HOLD body ok|miss` line on a change. Any miss adds nothing (that player's guns draw as before).
#[allow(clippy::too_many_arguments)]
/// Returns the Spine2 bone (`body_occl::BODY_SLOTS[3]`) when the capsules were built (C6 depth 5: the self check's chest).
fn body_capsules(idx: &[Option<u32>], vt43: usize, chr: usize, feet: V3, facing: Option<V3>, hp: &mut HoldPlayer, who: &str, out: &mut Vec<Capsule>) -> Option<V3> {
    // SAFETY: a live ChrIns of this tick (ours or a peer's from the player set); non-faulting reads only.
    let r = match unsafe { crate::game::chars::read_body_points(chr, idx, vt43, feet) } {
        None => Err("read"),
        Some(pts) => body_occl::capsules(&pts, feet, facing).map_err(body_occl::reject_label).map(|c| (c, pts)),
    };
    let state = match &r {
        Ok(_) => "ok",
        Err(w) => w,
    };
    if hp.body != Some(state) {
        hp.body = Some(state);
        match &r {
            Ok((c, pts)) => log_i!("HOLD", "body ok who={who} capsules={} bones={}", c.len(), body_occl::resolved_text(pts)),
            Err(w) => log_i!("HOLD", "body miss who={who} reason={w} (no capsules: its gun draws as before)"),
        }
    }
    match r {
        Ok((c, pts)) => {
            let room = body_occl::MAX_CAPSULES.saturating_sub(out.len());
            out.extend(c.into_iter().take(room));
            pts.get(3).copied().flatten()
        }
        Err(_) => None,
    }
}

/// C6 attach 2 (work/c6/attach2.md 5): one player's frame of the hand path. Order: ER's own weapon model (exactly where
/// ER draws the sword) -> the hand bone with the hand -> weapon grip learned while both read -> the hand bone with
/// `hand_axes` -> stage A (the caller). The gun is `W * T(offset) * R(rot) * A(weapon_axes) * S(in->m)`.
#[allow(clippy::too_many_arguments)]
fn hold_frame(
    ctx: &HoldCtx,
    chr: usize,
    phys: V3,
    grip: Grips,
    hp: &mut HoldPlayer,
    g_hand: &mut Vec<(u32, Mat34)>,
    now: f64,
    who: &str,
) -> HoldFrame {
    // C6 grip 3: the weapon-frame grip (the model and the learned bone grip) and the hand-bone grip (`hand_axes`)
    let ((wo, wr), (o, r), _, (xo, xr)) = grip;
    let bone = ctx.bone;
    let hand = ctx.vts.ok_or(Reason::Exe).and_then(|(vt43, vt44)| {
        let b = bone?;
        // SAFETY: a live ChrIns of this tick (ours or a peer's from the player set); non-faulting reads only.
        let rd = unsafe { crate::game::chars::read_hand(chr, b) }.ok_or(Reason::Read)?;
        validate_hand(&HandIn { vt43_ok: rd.vt43 == vt43, vt44_ok: rd.vt44 == vt44, n: rd.n, bone: b, e: rd.e, m: rd.m, phys })
    });
    // SAFETY: as above (the ChrAsm of a PlayerIns).
    let row = unsafe { crate::game::chars::read_arm(chr) }.and_then(|a| mw2er_core::er_stance::row_of(a.right_id)).unwrap_or(0);
    let mut weapon = None;
    if ctx.weapon {
        let (near, max_m) = match &hand {
            Ok(h) => (translation(&h.h), WEAPON_NEAR_M),
            Err(_) => (phys, HAND_MAX_M),
        };
        let wres = weapon_pose(chr, ctx.wvts, near, max_m).and_then(|x| if hp.stale.step(&x.0, near) { Err(Reason::WeaponStale) } else { Ok(x) });
        let mut ev = Vec::new();
        hp.wfb.frame(now, wres.as_ref().map(|_| ()).map_err(|e| *e), &mut ev);
        for e in ev {
            match e {
                FbEvent::Reason(x) => log_i!("HOLD", "weapon miss who={who} reason={} row={row}", x.label()),
                FbEvent::HandOk => {
                    if let Ok((_, src, d)) = &wres {
                        log_i!("HOLD", "weapon ok who={who} src={} d={d:.3} row={row}", src.label());
                    }
                }
                FbEvent::Summary { reasons, bad, total } => {
                    log_i!("HOLD", "weapon miss who={who} reasons={} frames={bad}/{total} 30s", reasons_text(&reasons))
                }
            }
        }
        weapon = wres.ok();
    }
    let bone_txt = bone.map_or(-1, |b| b as i64);
    // C6 part 16.7: a carrier crossbow row (ours or a peer's) uses the crossbow frame's grip / axes; the sword's stay
    let gw = if mw2er_core::carrier::carrier_rank(row).is_some() { grip_matrix(xo, xr, &ctx.weapon_axes_xbow) } else { grip_matrix(wo, wr, &ctx.weapon_axes) };
    // the weapon pose drawn this frame: the model, else the hand bone x the learned grip (the axes vote uses either)
    let mut wpose = weapon.as_ref().map(|x| x.0);
    let (res, src): (Result<(Mat34, String), Reason>, &'static str) = match (&weapon, &hand) {
        (Some((w, ws, d)), _) => {
            // only a pose that changed since the last frame teaches the grip: a repeated one may already be frozen while
            // the hand moves under the stale threshold (review: inv(H_moved) * W_frozen is not the real offset)
            if let Ok(h) = &hand
                && hp.stale.changed()
            {
                let g = learn_grip(&h.h, w);
                match g_hand.iter().position(|(k, _)| *k == row) {
                    Some(k) => g_hand[k].1 = g,
                    None if g_hand.len() < G_HAND_MAX => g_hand.push((row, g)),
                    None => {}
                }
            }
            (Ok((gun_from_hand(w, &gw), format!("src=weapon_{} d={d:.3} bone={bone_txt} row={row}", ws.label()))), "weapon")
        }
        (None, Ok(h)) => {
            let learned = g_hand.iter().find(|(k, _)| *k == row).or(g_hand.last()).map(|x| x.1);
            match (learned, ctx.hand_axes) {
                (Some(g), _) => (
                    Ok((gun_from_hand(wpose.insert(m_mul(&h.h, &g)), &gw), format!("src=bone_learned space={} bone={bone_txt} row={row}", h.space.label()))),
                    "bone_learned",
                ),
                (None, Some(a)) => (
                    Ok((
                        gun_from_hand(&h.h, &grip_matrix(o, r, &a)),
                        format!("src=bone space={} bone={bone_txt} scale={:.3} det={:.3}", h.space.label(), h.scale, h.det),
                    )),
                    "bone",
                ),
                (None, None) => (Err(Reason::Exe), "offset"),
            }
        }
        (None, Err(e)) => (Err(*e), "offset"),
    };
    if hp.src != Some(src) && now - hp.src_log_t >= SRC_LOG_S {
        log_i!("HOLD", "src who={who} src={src} was={}", hp.src.unwrap_or("none"));
        hp.src = Some(src);
        hp.src_log_t = now;
    }
    HoldFrame { res, weapon: wpose, hand: hand.ok(), row }
}

/// C6 attach 2: the `HOLD weapon probe` lines (with `bone_probe`, once per session for WPROBE_S from the first frame the
/// stance holds the weapon, or the first hand frame when the stance is off): both weapon matrices, the array count, the
/// right hand 57 / 58 and the palm pair 73 / 74 in full, so the next run can confirm which matrix ER refreshes.
fn weapon_probe(hs: &mut HoldState, c: &mw2er_core::config::CoopCfg, i: &CoopVisIn, me: usize, f: &HoldFrame) {
    if !c.bone_probe || hs.wprobe_done || !c.weapon_source() {
        return;
    }
    let now = i.now;
    if hs.wprobe_t0.is_none() {
        if !(i.stance_posed || !c.stance_on()) {
            return;
        }
        hs.wprobe_t0 = Some(now);
        hs.wprobe_next = now;
    }
    let t0 = hs.wprobe_t0.unwrap_or(now);
    if now - t0 > WPROBE_S {
        hs.wprobe_done = true;
        log_i!("HOLD", "weapon probe done (HOLD weapon ok / miss say which source was used)");
        return;
    }
    if now < hs.wprobe_next {
        return;
    }
    hs.wprobe_next = now + WPROBE_EVERY_S;
    let (vasm, vparts, vent) = weapon_vtables_rva().unwrap_or((0, 0, 0));
    let base = crate::game::c3_read::exe_base();
    let ok = |want: usize, got: usize| if want != 0 && got == base + want { "ok" } else { "bad" };
    // SAFETY: our own PlayerIns this tick; non-faulting reads only.
    let w = unsafe { crate::game::chars::read_weapon_model(me, crate::game::chars::ASM_SLOT_RIGHT0) };
    // C6 pose 3: + the left palm pair 29 / 45 (is the left hand on the weapon?)
    let bones: Vec<String> = [57u32, 58, 73, 74, 29, 45]
        .iter()
        // SAFETY: as above.
        .map(|&b| format!("b{b}={}", unsafe { crate::game::chars::read_hand(me, b) }.map_or("?".to_string(), |r| floats_text(&r.e))))
        .collect();
    match w {
        Ok(r) => log_i!(
            "HOLD",
            "weapon probe t={:.1} posed={} parts=0x{:x} vt=asm:{},parts:{},ent:{},arr:{} own={} n={} root={} e0={} used={} {}",
            now - t0,
            i.stance_posed as u8,
            r.parts,
            ok(vasm, r.asm_vt),
            ok(vparts, r.parts_vt),
            ok(vent, r.ent_vt),
            ok(own_vtables_rva().0.unwrap_or(0), r.arr_vt),
            r.owner_ok as u8,
            r.n,
            r.root.map_or("?".to_string(), |m| floats_text(&m)),
            r.e0.map_or("?".to_string(), |m| floats_text(&m)),
            f.weapon.map_or("none".to_string(), |m| v3_text(translation(&m))),
            bones.join(" ")
        ),
        Err(e) => log_i!("HOLD", "weapon probe t={:.1} posed={} model={e} {}", now - t0, i.stance_posed as u8, bones.join(" ")),
    }
}

/// C6: the jump guard / smoothing and the rate-limited `HOLD fallback` / `HOLD hand ok` lines of one player; `Some` =
/// draw this hand matrix, `None` = draw stage A.
fn hand_frame(hp: &mut HoldPlayer, now: f64, who: &str, res: Result<(Mat34, String), Reason>, smooth: f32) -> Option<Mat34> {
    let (r, info) = match res {
        Ok((m, s)) => (Ok(m), Some(s)),
        Err(e) => (Err(e), None),
    };
    let r = hp.track.step(now, r, smooth);
    let mut ev = Vec::new();
    hp.fb.frame(now, r.map(|_| ()), &mut ev);
    for e in ev {
        match e {
            FbEvent::Reason(x) => log_i!("HOLD", "fallback who={who} reason={}", x.label()),
            FbEvent::HandOk => log_i!("HOLD", "hand ok who={who} {}", info.as_deref().unwrap_or("src=held")),
            FbEvent::Summary { reasons, bad, total } => {
                log_i!("HOLD", "fallback who={who} reasons={} frames={bad}/{total} 30s", reasons_text(&reasons))
            }
        }
    }
    r.ok()
}

/// Review C3 stage B: the world gun's anchor and offset in OFFSET mode with `hand_bone >= 0`: the bone's position (no
/// offset) when the entry is plausible ("world_a/b" | "local_a/b"); else stage A: the feet + `offset_m` ("stage_a" /
/// "fallback"). C6: the entry is now read through the fixed chain (`chars::read_hand`).
fn hand_anchor(hand_bone: i32, chr: usize, feet: V3, q: [f32; 4], offset_m: [f32; 3]) -> (V3, [f32; 3], &'static str) {
    if hand_bone < 0 {
        return (feet, offset_m, "stage_a");
    }
    // SAFETY: a live ChrIns of this tick (ours or a peer's from the player set); non-faulting reads only.
    let r = unsafe { crate::game::chars::read_hand(chr, hand_bone as u32) };
    match r.filter(|r| (hand_bone as u32) < r.n) {
        Some(r) => match bone_anchor(feet, |v| rotate(q, v), &r.e, BONE_MAX_M) {
            Some((p, src)) => (p, [0.0; 3], src),
            None => (feet, offset_m, "fallback"),
        },
        None => (feet, offset_m, "fallback"),
    }
}

#[cfg(test)]
mod tests {
    use mw2er_core::coop_vis::{HIDE_FIELDS, STATE_INFO_HIDE_WEAPON};

    /// C6 hide: the raw offsets / widths / values of the row patch are the vendored SP_EFFECT_PARAM_ST fields (set through
    /// the accessors to the Unseen Blade values, read back as bytes), and nothing outside them changes.
    #[test]
    fn c6_hide_offsets_match_vendored_layout() {
        // SAFETY: SP_EFFECT_PARAM_ST is plain data (integers, floats, bitfield bytes): all-zero is a valid value.
        let mut r: eldenring::param::SP_EFFECT_PARAM_ST = unsafe { std::mem::zeroed() };
        r.set_motion_interval(-1.0);
        r.set_fall_damage_rate(1.0);
        r.set_soul_rate(1.0);
        r.set_equip_weight_change_rate(1.0);
        r.set_all_item_weight_change_rate(1.0);
        r.set_soul_steal_rate(1.0);
        r.set_life_reduction_rate(1.0);
        r.set_hp_recover_rate(1.0);
        r.set_state_info(STATE_INFO_HIDE_WEAPON as u16);
        r.set_wep_param_change(1);
        for f in [
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type0,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type1,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type2,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type3,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type4,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type5,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type6,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type7,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type8,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type9,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type10,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type11,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type12,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type13,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type14,
            eldenring::param::SP_EFFECT_PARAM_ST::set_vow_type15,
        ] {
            f(&mut r, true);
        }
        // SAFETY: reading the bytes of our own local value.
        let b = unsafe { std::slice::from_raw_parts(&r as *const _ as *const u8, std::mem::size_of_val(&r)) };
        let mut covered = vec![false; b.len()];
        for f in HIDE_FIELDS {
            let mut v = [0u8; 4];
            v[..f.width].copy_from_slice(&b[f.off..f.off + f.width]);
            assert_eq!(u32::from_le_bytes(v), f.to, "{} at +0x{:x}", f.name, f.off);
            covered[f.off..f.off + f.width].iter_mut().for_each(|c| *c = true);
        }
        assert!(b.iter().zip(&covered).all(|(x, c)| *c || *x == 0), "a setter wrote outside HIDE_FIELDS");
    }
}
