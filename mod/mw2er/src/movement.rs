//! M5 MW2 movement (docs/M5_DESIGN.md): the DLL side of the controller, run once per task tick on the game's main
//! thread inside the existing `ChrIns_PostPhysics` task (no new hook).
//!
//! Per tick: key counters -> key probe (MW2 mode off) -> deferred gravity restore -> one read (`game::move_read`) ->
//! handback triggers -> `mw2er_core::move_logic::MoveCtl` -> on `On`: resync + save/own gravity; on a release: exact
//! restores -> while driving: rebase/warp, push-out, the pure controller (`mw2er_core::move_sim::Player`) against
//! Havok map rays, then the writes of section 4.2 (position + proxy request, gravity re-assert, fall damage) and the
//! published feet / eye height for the camera-step detour -> the blank masks for the input detour -> log lines.
//!
//! Writes go through `game::write` only (version-gated, page-checked, non-faulting) and are logged once per episode
//! with field, address, old and new value. Gravity is saved in `GravityOwner` BEFORE its first write and restored on
//! every exit path (F5, config, every handback, AC-130, world left (deferred), panic: `emergency_off`). The HP write of
//! a damaging landing is the only write that is never restored (section 4.4).

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use mw2er_core::config::{
    Config, CrouchMode, FallDamageCfg, GostandFromProne, GroundNormalCfg, LethalFall, SprintModeCfg, StanceEye, TpAnim,
    WeaponMoveClass,
};
use mw2er_core::fall::{self, FallDamage};
use mw2er_core::fp_logic;
use mw2er_core::input_logic::CounterReader;
use mw2er_core::logfmt;
use mw2er_core::move_aim;
use mw2er_core::move_collide::RayWorld;
use mw2er_core::move_input::{
    BlankCtx, KeyMask, MoveKeys, PRESS_CROUCH, PRESS_INTERACT, PRESS_JUMP, PRESS_KEYS, PRESS_PRONE, PRESS_SPRINT,
    PRESS_STANCE_HOLD, PROBE_KEYS, blank_masks, er_bit_names, mask_count, mask_has,
};
use mw2er_core::move_logic::{
    CtlEvent, ErDelta, FieldPlan, GravityOwner, Hold, Jump, MoveCtl, POS_WRITE_MAX_M, PushParams, Reason, ReleasePlan,
    Triggers, char_pushout, classify_jump, er_pushout, position_write_ok, remove_into, rm_compensate,
};
use mw2er_core::ladder_log::{LadderLog, LadderSample};
use mw2er_core::mantle::{AbortWhy, REARM_BLOCK_S, Refuse};
use mw2er_core::move_logic::{Assist, AssistEvent, AssistIn, InteractFlap, Unstick, UnstickEvent, UnstickIn, er_confirmed};
use mw2er_core::collision_layers::{self, CastSrc};
use mw2er_core::move_sim::{Input, MAX_OTHERS, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::MoveData;
use mw2er_core::pmove::WeaponClass;
use mw2er_core::spread::SpreadStance;
use mw2er_core::sprint::{SprintEvent, SprintMode};
use mw2er_core::stance::{StanceEvent, StanceKind};
use mw2er_core::vec3::V3;
use mw2er_core::vm_pose::{StancePose, StancePoseIn, StancePoseParams, offset_from_fru};

use crate::cam_hook;
use crate::game::chars::ChrRead;
use crate::game::fp_read::{FpPlayer, FpWorld};
use crate::game::havok::{MAP_FILTER, OBJECT_FILTER, RAY_NS_MAX, map_ray_point, phys_world};
use crate::game::move_read::{
    CHR_CTRL_PROXY_FLAGS, DATA_HP, MoveRead, PHYS_GRAVITY_DISABLED, PHYS_GRAVITY_MULT, PHYS_POSITION, PHYS_PROXY_REQ,
    current_physics, read_gravity, read_move,
};
use crate::game::{LAST_WRITE_REFUSAL, refusal_label, write};
use crate::log::{log_i, log_w};
use crate::paths::Paths;
use crate::shared::{
    MOVE_A, MOVE_BLANK_LATCH, MOVE_BLANK_PLAIN, MOVE_BLANKED_READS, MOVE_D, MOVE_LATCHED, MOVE_S, MOVE_W, PRESSES_MOVE,
    PROBE_PRESSES, RAW_KEYS, RAW_MOVE_KEYS,
};

/// IW4 inches -> metres.
const M_PER_IN: f32 = 0.0254;
const IN_PER_M: f32 = 1.0 / M_PER_IN;
/// A required read failing this many ticks in a row is `read_fail`.
const READ_FAIL_TICKS: u32 = 3;
/// Hold windows (seconds) of the inferred edge signals.
const LOAD_HOLD_S: f64 = 2.0;
const TALK_HOLD_S: f64 = 1.5;
const MASK_HOLD_S: f64 = 1.0;
/// ER moved the player more than this in a tick: an interaction / knockback is still running.
const ER_MOVING_M: f32 = 0.05;
/// After a lethal HP write, ER's own death must show within this long.
const LETHAL_WATCH_S: f64 = 2.0;
const KEYPROBE_WINDOW_S: f64 = 0.3;
const ORDER_TICKS: u32 = 600;
const ER_DELTA_LOG_S: f64 = 5.0;
const STATE_LINE_MIN_S: f64 = 0.25;
const PROBE_LINE_MIN_S: f64 = 0.5;
const DISAGREE_STREAK: u32 = 30;
/// Velocity seed clamp on take-over (in/s, the MW2 sprint speed).
const SEED_MAX_IPS: f32 = 285.0;
/// Review M5 major 1: our ground ray missed while ER reported ground for this many ticks in a row (0.5 s at 60 Hz):
/// ER drives (`ground_disagree`) for `GROUND_DISAGREE_HOLD_S`.
const GROUND_DISAGREE_TICKS: u32 = 30;
const GROUND_DISAGREE_HOLD_S: f64 = 3.0;
/// Review M5 minor 7: ER's action flags keep the interact hold alive at most this long after the press.
const INTERACT_MAX_S: f64 = 8.0;
/// M8 P1: a position jump within this long of a load-counter change is never a chunk re-base.
const REBASE_LOAD_GRACE_S: f64 = 1.0;
/// M8: a `MOVE mantle check ok=0` line per refusal reason at most this often.
const MANTLE_REFUSE_LOG_S: f64 = 0.5;
/// M8 review: `why=no_face` (no face within reach of a held jump) is sampled at most this often, with its count.
const NO_FACE_LOG_S: f64 = 5.0;

// ---- data (loaded once at boot; `[movement] data` is cold) ------------------------------------------------------

static MOVE_DATA: OnceLock<Option<Arc<MoveData>>> = OnceLock::new();

/// Loads `converted\movement.json` (boot thread). A failure keeps the controller `Off(no_data)`.
pub fn load_data(paths: &Paths, cfg: &Config) {
    let p = paths.rel(&cfg.movement.data);
    let d = match std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display())).and_then(|t| MoveData::from_json(&t)) {
        Ok(d) => {
            log_i!("MOVE", "data {}", d.summary());
            Some(Arc::new(d))
        }
        Err(e) => {
            log_w!("MOVE", "blocked reason=no_data ({e}); run tools\\scripts\\convert_movement.py");
            None
        }
    };
    let _ = MOVE_DATA.set(d);
}

pub fn move_data() -> Option<Arc<MoveData>> {
    MOVE_DATA.get().cloned().flatten()
}

// ---- the game's rays for the pure controller ----------------------------------------------------------------------

/// `RayWorld` over `CSPhysWorld::cast_ray`: the controller works in inches, the game in metres. Run 18: one or two
/// collision filters (`[movement.rays]`: the map filter and the object / asset filter); with two, the nearer hit
/// wins and its source is counted (`hits`: map nearer or alone, object nearer or alone, both the same point).
struct HavokRays {
    world: usize,
    player_ins: usize,
    w: f32,
    filters: [u32; 2],
    n_filters: usize,
    hits: [u32; 3],
}

impl RayWorld for HavokRays {
    fn cast(&mut self, origin: V3, delta: V3) -> Option<V3> {
        let o = origin * M_PER_IN;
        let mut d = delta * M_PER_IN;
        // (point, distance, source: 0 map / first filter, 1 object / second filter, 2 both within 1 cm)
        let mut best: Option<(V3, f32, usize)> = None;
        for (k, f) in self.filters.iter().take(self.n_filters.clamp(1, 2)).enumerate() {
            if let Some(h) = map_ray_point(self.world, self.player_ins, o, self.w, d, *f) {
                let t = (h - o).len();
                best = match best {
                    None => Some((h, t, k)),
                    Some((bh, bt, _)) if (t - bt).abs() < 0.01 => Some((bh, bt, 2)),
                    Some(b) if t < b.1 => Some((h, t, k)),
                    b => b,
                };
                // the next filter only has to look 2 cm past this hit (a same-surface hit still counts as "both")
                if let Some(u) = d.normalize() {
                    d = u * (t + 0.02);
                }
            }
        }
        let (h, _, src) = best?;
        self.hits[src] += 1;
        Some(h * IN_PER_M)
    }
}

// ---- tick in/out -------------------------------------------------------------------------------------------------

pub struct MoveIn<'a> {
    pub now: f64,
    pub dt: f64,
    pub cfg: &'a Config,
    pub mode_on: bool,
    pub in_world: bool,
    pub menu_open: bool,
    /// The M2 blanking gates hold (mode on, in world, no menu, focused, gameplay enabled).
    pub blank_ok: bool,
    /// First person is On (last tick's result).
    pub fp_on: bool,
    /// C6 scope peek: that first person is only a third-person scoped ADS (`fp_only` / `tp_anim = er` keep their
    /// third-person choice through it, so the controller never flips mid-ADS).
    pub fp_peek: bool,
    pub ac130_block: bool,
    /// M7: the nuke's player-death prep (move_logic reason `nuke`).
    pub nuke_block: bool,
    pub fp_player: Result<Option<FpPlayer>, &'static str>,
    pub world: Option<FpWorld>,
    /// FP's latched cutscene signal (camera mask / fade), shared.
    pub fp_cutscene_latched: bool,
    /// ER's camera forward (row 2), never the gunship's.
    pub cam_row2: Option<V3>,
    pub chars: &'a [ChrRead],
    /// Bumped by the task on every config change (config-derived lines and masks are rebuilt only then).
    pub cfg_gen: u64,
    pub player_handle: u64,
    pub fire_held: bool,
    pub ads_held: bool,
    pub reload_press: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MoveOut {
    pub driving: bool,
    /// The controller's feet (metres, this tick's write) and eye height above them (metres).
    pub feet_m: Option<V3>,
    pub eye_h_m: Option<f32>,
    /// The aim's chest reference while driving (section 8.5).
    pub chest: Option<V3>,
    pub sprinting: Option<bool>,
    /// Fire / ADS / reload wait: sprinting or within sprintOutTime after it.
    pub fire_blocked: bool,
    pub sprint_started: bool,
    pub jumped: bool,
    /// 0 stand, 1 crouch, 2 prone; 255 = not driving.
    pub stance: u8,
    pub spread_stance: SpreadStance,
    pub speed_mps: f32,
    pub vm_offset_in: [f32; 3],
    /// M8: a mantle runs this tick.
    pub mantling: bool,
    /// M8: the mantle's row lowers the gun (rows >= 33 in, MW2 quick drop): the task's weapon gate.
    pub mantle_lowered: bool,
    /// M8: `[movement.mantle] vm = "sprint"`: the viewmodel shows the sprint pose instead of the drop.
    pub mantle_vm_sprint: bool,
}

impl MoveOut {
    pub fn idle() -> MoveOut {
        MoveOut {
            mantling: false,
            mantle_lowered: false,
            mantle_vm_sprint: false,
            driving: false,
            feet_m: None,
            eye_h_m: None,
            chest: None,
            sprinting: None,
            fire_blocked: false,
            sprint_started: false,
            jumped: false,
            stance: 255,
            spread_stance: SpreadStance::Stand,
            speed_mps: 0.0,
            vm_offset_in: [0.0; 3],
        }
    }
}

#[derive(Default)]
struct MovePerf {
    us: Vec<f64>,
    rays_sum: u64,
    rays_max: u32,
    ticks: u32,
    writes: u64,
    driving: bool,
    // M8: the mantle budget, reported separately from the main rays
    mantle_checks: u32,
    mantle_rays_max: u32,
    /// M8 review: the largest per-tick mantle budget (max_rays + 4 per extra substep) and deferred starts.
    mantle_budget_max: u32,
    mantle_deferred: u32,
    mantle_us_max: f64,
    mantles: u32,
    mantle_refusals: u32,
}

#[derive(Default)]
struct ErDeltaStats {
    samples: Vec<f32>,
    applied: u32,
    handbacks: u32,
    budget_hits: u32,
    disagree_streaks: u32,
    /// Ticks in which the ground ray missed and ER's ground kept the feet (review M5 major 1).
    ground_held_ticks: u32,
}

/// The probe line's fields, compared as values before any formatting (review M5 minor 8).
#[derive(Clone, Copy, PartialEq)]
struct ProbeVals {
    movement_limit: Option<u32>,
    anim_flags: u64,
    mod_flags: u64,
    motion_mult: Option<u32>,
    root_motion: Option<[u32; 3]>,
    /// M8: CSChrLadderModule.state (-1 = none).
    ladder: i8,
}

impl ProbeVals {
    fn of(r: &MoveRead, ladder: i8) -> ProbeVals {
        ProbeVals {
            ladder,
            movement_limit: r.movement_limit,
            anim_flags: r.anim_flags.unwrap_or(0),
            mod_flags: r.mod_flags.unwrap_or(0),
            // the line prints 2 decimals: compare what it would print
            motion_mult: r.motion_mult.map(|m| (m * 100.0).round().to_bits()),
            root_motion: r.root_motion.map(|v| [(v[0] * 100.0).round().to_bits(), (v[1] * 100.0).round().to_bits(), (v[2] * 100.0).round().to_bits()]),
        }
    }
}

/// The `MOVE state` change key (no allocation).
type StateKey = (StanceKind, bool, bool, bool);

/// C3 PASS C: W/A/S/D (and the left stick) reach ER while the controller drives (tp / fp hybrid): its run root motion
/// is expected in its displacement (written by the blank step every tick).
static WASD_TO_ER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// C3 PASS C: first person with `fp_anim = "hybrid"` drives: ER steers the body for A/S/D again, so first person's yaw
/// gate pauses on them as before M5 (read by first_person.rs).
pub static FP_HYBRID_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct MoveController {
    ctl: MoveCtl,
    gravity: GravityOwner,
    player: Option<Player>,
    presses: [CounterReader; PRESS_KEYS],
    probe_presses: [CounterReader; mw2er_core::move_input::N_PROBE],
    // episode / owner
    owner: usize,
    physics: usize,
    logged: HashSet<&'static str>,
    /// The config generation (and data presence) the cfg line was built for.
    cfg_seen: Option<(u64, bool)>,
    cfg_line: String,
    /// The movement keys and ER blank list of a config generation.
    keys_cache: Option<(u64, MoveKeys, Vec<u8>)>,
    /// Reused per-tick buffers (no allocation per tick).
    others: Vec<V3>,
    events: Vec<SimEvent>,
    // trigger state
    read_fail_streak: u32,
    read_fail_field: &'static str,
    pending_write_fail: Option<&'static str>,
    freefall: bool,
    interact: Hold,
    /// The press time of the running interact hold and ER's action flags while we last drove (its baseline).
    interact_t0: f64,
    flags_driving: Option<(u64, u64)>,
    er_move: Hold,
    ground_disagree: Hold,
    /// Consecutive ticks in which the feet were kept on ER's ground signal only.
    ground_hold_streak: u32,
    load_hold: Hold,
    talk_hold: Hold,
    mask_hold: Hold,
    last_load: Option<i32>,
    last_talk: Option<i32>,
    /// 2026-10-21: the interact gate of the talk handback (fp_logic::TalkGate).
    talk_gate: fp_logic::TalkGate,
    last_mask: Option<u32>,
    er_move_flap: fp_logic::FlapGuard,
    load_flap: fp_logic::FlapGuard,
    // positions
    last_written: Option<V3>,
    prev_er: Option<V3>,
    prev_chunk: Option<V3>,
    er_vel: V3,
    world_entered: bool,
    // lethal fall watch
    lethal_t: Option<f64>,
    lethal_warned: bool,
    // presentation
    pose: StancePose,
    blank_seen: Option<(KeyMask, KeyMask)>,
    last_state_t: f64,
    last_state_key: Option<StateKey>,
    ticks_driving: u64,
    last_probe: Option<ProbeVals>,
    last_probe_t: f64,
    stance_blocked_t: f64,
    // order probe
    order_n: u32,
    order_m: u32,
    last_detour_calls: u64,
    order_logged: bool,
    // key probe
    probe_pending: [(f64, u64); mw2er_core::move_input::N_PROBE],
    probe_logged: [bool; mw2er_core::move_input::N_PROBE],
    // stats
    er_delta: ErDeltaStats,
    last_er_delta_log: f64,
    disagree: u32,
    perf: MovePerf,
    /// The reason of the last release (a `menu` keeps the stance).
    last_release: Option<Reason>,
    /// The optional stance-hold key: press time and whether the long hold fired.
    hold_t0: Option<f64>,
    hold_fired: bool,
    // ---- M8 ----
    /// The last load-counter change (the re-base rule: no re-base within 1 s of one).
    last_load_change_t: f64,
    /// No mantle check before this (a re-arm: now + 0.3 s).
    mantle_rearm_until: f64,
    /// The last `MOVE mantle check ok=0` per refusal reason (rate limit) and the count since.
    refuse_log_t: [f64; 15],
    refuse_count: [u32; 15],
    /// B3: interact handbacks count in the flap guard only when nothing ER confirms took over.
    interact_flap_m8: InteractFlap,
    /// B4: the interact hold was cleared for this ladder episode.
    interact_cleared: bool,
    /// The ladder audit lines.
    ladder_log: LadderLog,
    /// Consecutive failed first-person reads (`fp:<field>` read_fail after READ_FAIL_TICKS).
    fp_fail_streak: u32,
    fp_fail_field: &'static str,
    /// Ticks since a mantle end, until ER reports ground (the `MOVE mantle settle` line).
    mantle_settle: Option<u32>,
    // ---- run 18 ----
    /// The unstick detector and its handback hold (the hold length of the last fire).
    unstick: Unstick,
    unstick_hold: Hold,
    unstick_hold_s: f64,
    unsticks: u32,
    /// Ray hits by source since the last state line / er_delta line: [map, object, both] (`[movement.rays]`).
    hits_state: [u32; 3],
    hits_period: [u32; 3],
    /// This tick's hits (the mantle check line).
    hits_tick: [u32; 3],
    // ---- C3 (docs/C3_DESIGN.md 1.3-1.6) ----
    /// The `player` ray mode's cast filter (the live layer or the fallback) and where it came from.
    player_cast: u32,
    cast_src: CastSrc,
    /// The last read of the live filter word (re-read every PLAYER_CAST_REFRESH_S while driving).
    cast_read_t: f64,
    /// The `MOVE filter live` line was logged for this rays mode (first `MOVE on`, and on a mode change).
    filter_logged_mode: Option<String>,
    /// The one-shot live table dump ran.
    table_logged: bool,
    /// `MOVE assist`: the detector, its hold, the hold length of the last fire, the count.
    assist: Assist,
    assist_hold: Hold,
    assist_hold_s: f64,
    assists: u32,
    /// A mantle refusal (no_face / path_blocked / edge) happened this tick (the assist's shortcut).
    refused_tick: bool,
    /// `MOVE blocked wish=..` episodes: the start of the current stall and the last line (rate limit 1/s).
    blocked_since: Option<f64>,
    blocked_log_t: f64,
}

/// C3: the live filter word is read again this often while driving (s).
const PLAYER_CAST_REFRESH_S: f64 = 5.0;
/// C3: a stall (direction held, speed < 0.15 m/s) this long is a blocked episode (`MOVE blocked wish=` line).
const BLOCKED_EPISODE_S: f64 = 0.4;

impl MoveController {
    pub fn new() -> Self {
        let rd = |c: &std::sync::atomic::AtomicU32| CounterReader::starting_at(c.load(Ordering::SeqCst));
        MoveController {
            ctl: MoveCtl::default(),
            gravity: GravityOwner::default(),
            player: None,
            presses: std::array::from_fn(|i| rd(&PRESSES_MOVE[i])),
            probe_presses: std::array::from_fn(|i| rd(&PROBE_PRESSES[i])),
            owner: 0,
            physics: 0,
            logged: HashSet::new(),
            cfg_seen: None,
            cfg_line: String::new(),
            keys_cache: None,
            others: Vec::with_capacity(64),
            events: Vec::with_capacity(16),
            read_fail_streak: 0,
            read_fail_field: "",
            pending_write_fail: None,
            freefall: false,
            interact: Hold::default(),
            interact_t0: f64::NEG_INFINITY,
            flags_driving: None,
            er_move: Hold::default(),
            ground_disagree: Hold::default(),
            ground_hold_streak: 0,
            load_hold: Hold::default(),
            talk_hold: Hold::default(),
            mask_hold: Hold::default(),
            last_load: None,
            last_talk: None,
            talk_gate: fp_logic::TalkGate::default(),
            last_mask: None,
            er_move_flap: fp_logic::FlapGuard::default(),
            load_flap: fp_logic::FlapGuard::default(),
            last_written: None,
            prev_er: None,
            prev_chunk: None,
            er_vel: V3::ZERO,
            world_entered: false,
            lethal_t: None,
            lethal_warned: false,
            pose: StancePose::default(),
            blank_seen: None,
            last_state_t: f64::NEG_INFINITY,
            last_state_key: None,
            ticks_driving: 0,
            last_probe: None,
            last_probe_t: f64::NEG_INFINITY,
            stance_blocked_t: f64::NEG_INFINITY,
            order_n: 0,
            order_m: 0,
            last_detour_calls: 0,
            order_logged: false,
            probe_pending: [(f64::NEG_INFINITY, 0); mw2er_core::move_input::N_PROBE],
            probe_logged: [false; mw2er_core::move_input::N_PROBE],
            er_delta: ErDeltaStats::default(),
            last_er_delta_log: 0.0,
            disagree: 0,
            perf: MovePerf::default(),
            last_release: None,
            hold_t0: None,
            hold_fired: false,
            last_load_change_t: f64::NEG_INFINITY,
            mantle_rearm_until: f64::NEG_INFINITY,
            refuse_log_t: [f64::NEG_INFINITY; 15],
            refuse_count: [0; 15],
            interact_flap_m8: InteractFlap::default(),
            interact_cleared: false,
            ladder_log: LadderLog::default(),
            fp_fail_streak: 0,
            fp_fail_field: "",
            mantle_settle: None,
            unstick: Unstick::default(),
            unstick_hold: Hold::default(),
            unstick_hold_s: 1.0,
            unsticks: 0,
            hits_state: [0; 3],
            hits_period: [0; 3],
            hits_tick: [0; 3],
            player_cast: 0x3,
            cast_src: CastSrc::Fallback,
            cast_read_t: f64::NEG_INFINITY,
            filter_logged_mode: None,
            table_logged: false,
            assist: Assist::default(),
            assist_hold: Hold::default(),
            assist_hold_s: 0.8,
            assists: 0,
            refused_tick: false,
            blocked_since: None,
            blocked_log_t: f64::NEG_INFINITY,
        }
    }

    // ---- config / data lines -------------------------------------------------------------------------------------

    fn sim_cfg(cfg: &Config, d: &MoveData, hit_radius: f32) -> SimCfg {
        let m = &cfg.movement;
        let mut s = SimCfg::defaults(d);
        let r_m = if m.hull_radius_m > 0.0 { m.hull_radius_m } else { hit_radius.clamp(0.2, 0.6) };
        s.radius = (if r_m.is_finite() { r_m } else { 0.4 }) * IN_PER_M;
        s.skin = m.skin_m * IN_PER_M;
        s.max_substep_s = m.max_substep_s;
        s.max_bumps = m.max_bumps;
        s.max_rays = m.max_rays_per_tick;
        s.v_scale = match m.stance_eye {
            StanceEye::Ratio => cfg.first_person.eye_height_m / (d.mw2.eye_stand * M_PER_IN),
            StanceEye::Mw2 => 1.0,
        };
        s.walkable_y = m.walkable_normal_y;
        s.jump_slowdown = m.jump_slowdown;
        s.sprint.marathon = m.marathon;
        s.sprint.mode = match m.sprint_mode {
            SprintModeCfg::Press => SprintMode::Press,
            SprintModeCfg::Hold => SprintMode::Hold,
        };
        s.speed_scale = m.speed_scale;
        s.class_scale = match m.weapon_move_class {
            WeaponMoveClass::Smg => WeaponClass::Smg,
            WeaponMoveClass::Rifle => WeaponClass::Rifle,
            WeaponMoveClass::Mg => WeaponClass::Mg,
            WeaponMoveClass::Launcher => WeaponClass::Launcher,
            // M6 (docs/M6_DESIGN.md section 7): the class of the gun in hand, published by the task at each raise
            WeaponMoveClass::Auto => match crate::shared::ARSENAL_MOVE_CLASS.load(std::sync::atomic::Ordering::Relaxed) {
                1 => WeaponClass::Rifle,
                2 => WeaponClass::Mg,
                3 => WeaponClass::Launcher,
                _ => WeaponClass::Smg,
            },
        }
        .scale(&d.mw2);
        s.lightweight = m.lightweight;
        s.crouch_hold = m.crouch_mode == CrouchMode::Hold;
        s.gostand_from_prone = match m.gostand_from_prone {
            GostandFromProne::Stand => StanceKind::Stand,
            GostandFromProne::Crouch => StanceKind::Crouch,
        };
        s.freefall_in = m.freefall_handback_m * IN_PER_M;
        // M8: [movement.mantle] over MW2's numbers; enabled only with the data's mantle block
        let mut mt = m.mantle.sim();
        mt.enabled = mt.enabled && d.mantle.as_ref().is_some_and(|x| x.enable);
        mt.er_push_abort_in = m.er_move_handback_m * IN_PER_M;
        mt.end_speed_max = d.mw2.g_speed;
        s.mantle = mt;
        // run 18: the waist probe and the front body column
        s.body_probes = m.rays.body_probes;
        s
    }

    /// The cfg line, rebuilt only when the config generation or the data presence changed (review M5 minor 8).
    fn log_cfg(&mut self, cfg: &Config, cfg_gen: u64, d: Option<&MoveData>) {
        if self.cfg_seen == Some((cfg_gen, d.is_some())) {
            return;
        }
        self.cfg_seen = Some((cfg_gen, d.is_some()));
        let m = &cfg.movement;
        let line = match d {
            Some(d) => {
                let s = Self::sim_cfg(cfg, d, 0.4);
                let eye = |e: f32| e * s.v_scale * M_PER_IN;
                let mt = &m.mantle;
                format!(
                    "enabled={} tp_anim={} sprint={} crouch={} jump_slowdown={} fall={} lethal_fall={} eye={:.3}/{:.3}/{:.3} hull={:.3}/{:.3}/{:.3} r={} rays_max={} data={} sha={} mantle={} key={} max_h={:.2}m rise_max={:.0}in angle={:.0} over={} vm={} rays={} ladder_er_attacks={} filters={} body_probes={} depen={:.2}m unstick={} assist={} fp_anim={} rm_mps={:.1}",
                    m.enabled as u8,
                    if m.tp_anim == TpAnim::Hybrid { "hybrid" } else { "er" },
                    if m.sprint_mode == SprintModeCfg::Press { "press" } else { "hold" },
                    if m.crouch_mode == CrouchMode::Toggle { "toggle" } else { "hold" },
                    m.jump_slowdown as u8,
                    if m.fall_damage == FallDamageCfg::Mw2 { "mw2" } else { "off" },
                    if m.lethal_fall == LethalFall::Handback { "handback" } else { "zero" },
                    eye(d.mw2.eye_stand),
                    eye(d.mw2.eye_crouch),
                    eye(d.mw2.eye_prone),
                    eye(d.mw2.hull_stand),
                    eye(d.mw2.hull_crouch),
                    eye(d.mw2.hull_prone),
                    if m.hull_radius_m > 0.0 { format!("{:.2}", m.hull_radius_m) } else { "er".into() },
                    m.max_rays_per_tick,
                    m.data,
                    &d.exe_sha256[..d.exe_sha256.len().min(12)],
                    s.mantle.enabled as u8,
                    mt.key_mode,
                    mt.max_height_m,
                    mt.max_rise_in,
                    mt.check_angle_deg,
                    mt.over,
                    mt.vm,
                    mt.max_rays,
                    m.ladder_er_attacks as u8,
                    if m.rays.is_player() {
                        // C3: one cast with the player's own proxy layer (live; the fallback until the first read)
                        format!(
                            "player(cast=0x{:x} fallback=0x{:x} layers={:?} src={})",
                            self.player_cast,
                            m.rays.player_fallback,
                            m.rays.player_layers,
                            self.cast_src.label()
                        )
                    } else {
                        let (f, n) = m.rays.filters();
                        // the exe's own constants: the map filter of cast_ray's only caller, the follow camera's filter
                        let name = |x: u32| match x {
                            MAP_FILTER => "map",
                            OBJECT_FILTER => "object_cam",
                            _ if x == m.rays.map_filter => "map",
                            _ => "object",
                        };
                        let tags: Vec<String> = f[..n].iter().map(|x| format!("{}:0x{x:x}", name(*x))).collect();
                        format!("{}({}) rollback=1", m.rays.mode, tags.join("+"))
                    },
                    m.rays.body_probes as u8,
                    mt.depenetrate_m,
                    if m.unstick.enabled {
                        format!("{:.1}s/{:.2}m/{:.0}%/hold{:.1}-{:.1}s/max{}", m.unstick.window_s, m.unstick.progress_m, m.unstick.evidence_frac * 100.0, m.unstick.hold_s, m.unstick.max_hold_s, m.unstick.max_per_window)
                    } else {
                        "0".into()
                    },
                    if m.assist.enabled {
                        format!(
                            "{:.2}mps/{:.1}s/hold{:.1}s/max{}/refusal{}",
                            m.assist.stall_speed,
                            m.assist.stall_s,
                            m.assist.hold_s,
                            m.assist.max_per_min,
                            m.assist.after_refusal as u8
                        )
                    } else {
                        "0".into()
                    },
                    // C3 PASS C (docs/C3_DESIGN.md 3.6): ER sees W/A/S/D in first person too (walk / run animations)
                    m.fp_anim.label(),
                    m.fp_anim_rm_mps
                )
            }
            None => format!("enabled={} data={} (missing: controller off)", m.enabled as u8, m.data),
        };
        // a reload that changes nothing here logs nothing
        if line != self.cfg_line {
            log_i!("MOVE", "cfg {line}");
            self.cfg_line = line;
        }
    }

    fn log_once(&mut self, key: &'static str, msg: impl FnOnce() -> String) {
        if self.logged.insert(key) {
            log_i!("MOVE", "{}", msg());
        }
    }

    // ---- the tick ------------------------------------------------------------------------------------------------

    pub fn tick(&mut self, i: &MoveIn) -> MoveOut {
        let t0 = std::time::Instant::now();
        let d = move_data();
        self.log_cfg(i.cfg, i.cfg_gen, d.as_deref());
        // key counters every tick (never replayed later)
        let mut presses = [0u32; PRESS_KEYS];
        for (k, p) in presses.iter_mut().enumerate() {
            *p = self.presses[k].take(PRESSES_MOVE[k].load(Ordering::Relaxed));
        }
        let mut probe = [0u32; mw2er_core::move_input::N_PROBE];
        for (k, p) in probe.iter_mut().enumerate() {
            *p = self.probe_presses[k].take(PROBE_PRESSES[k].load(Ordering::Relaxed));
        }
        let rd = if i.in_world { read_move() } else { Ok(None) };
        // review M5 major 2: without a physics world every ray reads "no hit" (no walls, no floor): count it as a
        // failed read (no sim, no write this tick; `read_fail` handback after READ_FAIL_TICKS)
        let phys = if matches!(rd, Ok(Some(_))) { phys_world() } else { 0 };
        let rd = match rd {
            Ok(Some(_)) if phys == 0 => Err("CSHavokMan.phys_world"),
            x => x,
        };
        let r = match rd {
            Ok(r) => {
                self.read_fail_streak = 0;
                r
            }
            Err(f) => {
                self.read_fail_streak += 1;
                self.read_fail_field = f;
                None
            }
        };
        self.keyprobe(i, &probe, r.as_ref());
        self.deferred_gravity(i.in_world);

        // world entry / exit
        let in_world = i.in_world && (r.is_some() || self.read_fail_streak > 0);
        if !in_world {
            self.world_entered = false;
            self.prev_er = None;
            self.prev_chunk = None;
            self.last_load = None;
        }

        // ---- triggers ----
        let p = i.fp_player.ok().flatten();
        let w = i.world;
        let mut t = Triggers::default();
        let owner_changed = self.ctl.driving() && r.as_ref().is_some_and(|r| r.player != self.owner);
        t.set(Reason::WorldLeft, !in_world || owner_changed);
        // loading: the load counter changed (held a little)
        if let Some(lc) = w.and_then(|w| w.load_counter) {
            if self.last_load.is_some_and(|l| l != lc) {
                // M8 P1: a load seen (even with the handback signal muted) rules out a re-base for a while
                self.last_load_change_t = i.now;
            }
            if self.last_load.is_some_and(|l| l != lc) && !self.load_flap.muted() {
                self.load_hold.trigger(i.now, LOAD_HOLD_S);
                if self.ctl.driving() && self.load_flap.record(i.now) {
                    log_w!("MOVE", "WARN signal load_counter muted for this session after {} handbacks in {:.0} s (it changes during normal play)", fp_logic::FLAP_MAX, fp_logic::FLAP_WINDOW_S);
                }
            }
            self.last_load = Some(lc);
        }
        t.set(Reason::Loading, self.load_hold.active(i.now, false, LOAD_HOLD_S));
        // M8 (ladder audit): a first-person read that keeps failing would read the ladder, death and Torrent signals
        // as "no": hand back (read_fail field=fp:<name>) instead of driving blind
        match i.fp_player {
            Err(f) if i.in_world => {
                self.fp_fail_streak += 1;
                self.fp_fail_field = f;
            }
            _ => self.fp_fail_streak = 0,
        }
        if self.fp_fail_streak >= READ_FAIL_TICKS && self.read_fail_streak < READ_FAIL_TICKS {
            self.read_fail_field = match self.fp_fail_field {
                "main_player" => "fp:main_player",
                "modules" => "fp:modules",
                "ladder_module" => "fp:ladder_module",
                "ladder.state" => "fp:ladder.state",
                "hp" => "fp:hp",
                "ride_module" => "fp:ride_module",
                _ => "fp:other",
            };
        }
        t.set(Reason::ReadFail, self.read_fail_streak >= READ_FAIL_TICKS || self.fp_fail_streak >= READ_FAIL_TICKS);
        let write_fail = self.pending_write_fail.take();
        t.set(Reason::WriteFail, write_fail.is_some());
        t.set(Reason::NoData, d.is_none());
        let dead = p.is_some_and(|p| fp_logic::dead(p.hp, p.flags1c5, w.and_then(|w| w.camera_type)));
        t.set(Reason::Death, dead);
        let fp_cfg = &i.cfg.first_person;
        let fade = w.and_then(|w| fp_logic::fade_max(&w.fade)).is_some_and(|a| fp_cfg.use_fade_signal && a > fp_cfg.fade_alpha_max);
        if let Some(mask) = w.and_then(|w| w.camera_mask) {
            if fp_cfg.use_cam_mask_signal && self.last_mask.is_some_and(|m| m != mask) {
                self.mask_hold.trigger(i.now, MASK_HOLD_S);
            }
            self.last_mask = Some(mask);
        }
        t.set(Reason::Cutscene, fade || i.fp_cutscene_latched || self.mask_hold.active(i.now, false, MASK_HOLD_S));
        t.set(Reason::Torrent, p.is_some_and(|p| fp_logic::riding(p.ride_state, p.is_mounting, fp_cfg.use_ride_state_signal)));
        t.set(Reason::Ladder, p.is_some_and(|p| fp_logic::on_ladder(p.ladder)));
        t.set(Reason::Throw, p.is_some_and(|p| fp_logic::in_throw(p.throw_state)));
        // 2026-10-21: like first person, a talk id change counts only shortly after an interact press (a boss voice
        // line mid-fight suspended movement for 1.5 s)
        self.talk_gate.interacted(presses[PRESS_INTERACT], i.now);
        if let Some(tid) = w.and_then(|w| w.talk_id) {
            if fp_cfg.use_talk_signal
                && self.last_talk.is_some_and(|l| l != tid)
                && self.talk_gate.is_dialog(i.now, fp_cfg.talk_interact_window_s as f64)
            {
                self.talk_hold.trigger(i.now, TALK_HOLD_S);
            }
            self.last_talk = Some(tid);
        }
        // C3 (docs/C3_DESIGN.md 1.7): TALK_HOLD_S stays the minimum; while first person's talk latch holds, movement
        // holds too (C2: movement re-armed after 1.5 s and MW2 mode ran in third person until the latch let go)
        let fp_talk = fp_cfg.use_talk_signal && crate::shared::TALK_LATCHED.load(Ordering::Relaxed);
        t.set(Reason::Talk, self.talk_hold.active(i.now, false, TALK_HOLD_S) || fp_talk);
        // ER's own displacement of the player this tick (interactions, knockback)
        let er_step = match (r.as_ref(), self.prev_er) {
            (Some(r), Some(prev)) => V3::new(r.pos.x - prev.x, 0.0, r.pos.z - prev.z).len(),
            _ => 0.0,
        };
        // ...only when the user is not walking himself (W/A/S/D released): an interaction animation or a knockback,
        // not ER's normal movement during the suspension (that would extend the hold forever) [I]
        let er_moving = !self.ctl.driving()
            && er_step > ER_MOVING_M
            && er_step < 5.0
            && (RAW_MOVE_KEYS.load(Ordering::Relaxed) | crate::pad::move_bits()) & (MOVE_W | MOVE_A | MOVE_S | MOVE_D) == 0;
        let ml_interact = i.cfg.movement.use_movement_limit_signal && r.as_ref().is_some_and(|r| r.movement_limit == Some(3));
        if presses[PRESS_INTERACT] > 0 && i.mode_on && !self.interact_flap_m8.muted() {
            self.interact.trigger(i.now, i.cfg.movement.interact_hold_s as f64);
            self.interact_t0 = i.now;
            self.interact_cleared = false;
        }
        // review M5 minor 7: an interaction that does not move the player (lever, chest, grace) keeps ER's action /
        // modifier flags away from what they were while we drove; while they differ (W/A/S/D released, at most
        // INTERACT_MAX_S after the press) the hold is extended [I: which bits mean "interacting" is unverified]
        let flags_now = r.as_ref().map(|r| (r.anim_flags.unwrap_or(0), r.mod_flags.unwrap_or(0)));
        let flags_busy = !self.ctl.driving()
            && i.now - self.interact_t0 < INTERACT_MAX_S
            && (RAW_MOVE_KEYS.load(Ordering::Relaxed) | crate::pad::move_bits()) & (MOVE_W | MOVE_A | MOVE_S | MOVE_D) == 0
            && matches!((flags_now, self.flags_driving), (Some(a), Some(b)) if a != b);
        let mut interact = self.interact.active(i.now, er_moving || ml_interact || flags_busy, i.cfg.movement.interact_hold_s as f64);
        // M8 B4: once ER shows the ladder the E hold has done its job: clear it, so the re-arm after the dismount
        // waits only for the ladder (and the grounded ticks), never for a stale interact hold
        if interact && t.has(Reason::Ladder) && !self.interact_cleared {
            self.interact.clear();
            self.interact_cleared = true;
            interact = false;
            log_i!("MOVE", "interact cleared why=ladder state={}", p.map_or(-1, |p| p.ladder));
        }
        t.set(Reason::Interact, interact);
        // M8 B3: an E press that became a ladder / talk / cutscene / throw / Torrent episode is never a flap
        if self.interact_flap_m8.tick(i.now, er_confirmed(&t), interact) {
            log_w!(
                "MOVE",
                "WARN signal interact muted for this session after {} unconfirmed handbacks in {:.0} s",
                fp_logic::FLAP_MAX,
                fp_logic::FLAP_WINDOW_S
            );
        }
        t.set(Reason::ErMove, self.er_move.active(i.now, er_moving, i.cfg.movement.er_move_hold_s as f64));
        t.set(Reason::GroundDisagree, self.ground_disagree.active(i.now, false, GROUND_DISAGREE_HOLD_S));
        // run 18: the unstick hold (ER's physics resolves the overlap; the hold doubles per repeat, see Unstick)
        t.set(Reason::Unstick, self.unstick_hold.active(i.now, false, self.unstick_hold_s));
        // C3: the assist window (ER walks with W/A/S/D; first person stays on, the camera follows the character)
        t.set(Reason::Assist, self.assist_hold.active(i.now, false, self.assist_hold_s));
        if self.freefall && r.as_ref().is_some_and(|r| r.is_touching_ground) && !self.ctl.driving() {
            self.freefall = false;
        }
        t.set(Reason::Freefall, self.freefall);
        t.set(Reason::Menu, i.menu_open);
        t.set(Reason::Ac130, i.ac130_block);
        t.set(Reason::Nuke, i.nuke_block);
        let fp_user = i.fp_on && !i.fp_peek;
        t.set(Reason::FpOnly, i.cfg.movement.fp_only && !fp_user);
        t.set(Reason::TpEr, i.cfg.movement.tp_anim == TpAnim::Er && !fp_user);
        t.set(Reason::F5, !i.mode_on);
        t.set(Reason::Config, !i.cfg.movement.enabled);
        // a lethal landing must be followed by ER's own death
        if let Some(lt) = self.lethal_t {
            if dead {
                self.lethal_t = None;
            } else if i.now - lt > LETHAL_WATCH_S {
                self.lethal_t = None;
                if !self.lethal_warned {
                    self.lethal_warned = true;
                    log_w!("MOVE", "WARN lethal_fall no_death_seen (HP was written 0 {LETHAL_WATCH_S} s ago; consider [movement] lethal_fall = \"handback\")");
                }
            }
        }

        let er_grounded = r.as_ref().is_some_and(|r| r.is_touching_ground);
        let ev = self.ctl.step(&t, er_grounded, i.cfg.movement.rearm_ticks);
        let mut out = MoveOut::idle();
        match ev {
            Some(CtlEvent::On) => {
                if let (Some(r), Some(d)) = (r.as_ref(), d.as_deref()) {
                    self.enter(i, r, d);
                } else {
                    self.ctl.emergency_off();
                }
            }
            Some(CtlEvent::Release(reason)) => {
                self.release(reason, i, r.as_ref(), write_fail);
                if reason == Reason::Interact {
                    self.interact_flap_m8.released(i.now);
                }
            }
            Some(CtlEvent::Blocked(reason)) => {
                if i.mode_on && i.in_world {
                    if reason == Reason::Ladder {
                        log_i!("MOVE", "blocked reason=ladder state={}", p.map_or(-1, |p| p.ladder));
                    } else {
                        log_i!("MOVE", "blocked reason={}", reason.label());
                    }
                }
            }
            None => {}
        }

        if self.ctl.driving()
            && let (Some(r), Some(d)) = (r.as_ref(), d.as_deref())
        {
            out = self.drive(i, r, d, &presses, phys);
            // the interact baseline: ER's flags while we drive (no interaction running)
            self.flags_driving = Some((r.anim_flags.unwrap_or(0), r.mod_flags.unwrap_or(0)));
        }

        // M8 ladder audit: the LADDER lines from the same first-person read (ER third person, nothing written)
        if let Some(pp) = p
            && i.mode_on
        {
            let move_state = match self.ctl.state() {
                mw2er_core::move_logic::State::Driving => "driving",
                mw2er_core::move_logic::State::Arming(_) => "arming",
                mw2er_core::move_logic::State::Suspended(_) => "suspended",
                mw2er_core::move_logic::State::Off(_) => "off",
            };
            let s = LadderSample {
                now: i.now,
                state: pp.ladder,
                pos: pp.pos,
                top: pp.ladder_top,
                bottom: pp.ladder_bottom,
                er_ground: r.as_ref().is_some_and(|r| r.is_touching_ground),
                move_state,
                fp_on: i.fp_on,
            };
            for line in self.ladder_log.feed(&s) {
                log_i!("LADDER", "{}", line.text());
            }
        }

        // blank masks for the input detour (only while driving and the M2 gates hold)
        self.publish_blank(i, out.driving);
        if !out.driving {
            cam_hook::unpublish_feet();
        }
        if let Some(r) = r.as_ref() {
            if let Some(prev) = self.prev_er
                && i.dt > 0.0
                && (r.pos - prev).len() < 5.0
            {
                self.er_vel = (r.pos - prev) * (1.0 / i.dt as f32);
            }
            self.prev_er = Some(r.pos);
            self.prev_chunk = r.chunk;
            self.probe_line(i.now, r, p.map_or(-1, |p| p.ladder));
        }
        let us = t0.elapsed().as_secs_f64() * 1e6;
        if self.perf.us.len() < 4096 {
            self.perf.us.push(us);
        }
        self.perf.driving |= out.driving;
        out
    }

    /// Takes over: resync to ER's position, own gravity (save first), episode logs.
    fn enter(&mut self, i: &MoveIn, r: &MoveRead, d: &MoveData) {
        self.logged.clear();
        self.owner = r.player;
        self.physics = r.physics;
        self.ticks_driving = 0;
        self.freefall = false;
        self.ground_hold_streak = 0;
        // the velocity seed: ER's last planar displacement / dt, clamped; zero if ER is airborne
        let seed = if r.is_touching_ground && i.dt > 0.0 {
            let v = V3::new(self.er_vel.x, 0.0, self.er_vel.z) * IN_PER_M;
            let l = v.len();
            if l > SEED_MAX_IPS { v * (SEED_MAX_IPS / l) } else { v }
        } else {
            V3::ZERO
        };
        let pos_in = r.pos * IN_PER_M;
        let keep = self.player.take();
        let mut pl = Player::new(d, pos_in);
        if let Some(old) = keep {
            // the sprint meter is time based and survives everything; the stance only across a menu
            pl.sprint = old.sprint;
            if self.last_release.is_some_and(|x| x.keeps_stance()) {
                pl.stance = old.stance;
            }
        }
        pl.resync(pos_in, seed, true);
        // M8: a stale mantle path never resumes after a re-arm, and no check for 0.3 s
        debug_assert!(pl.mantle.is_none());
        if pl.mantle.is_some() {
            pl.mantle = None;
        }
        self.mantle_rearm_until = i.now + REARM_BLOCK_S;
        pl.mantle_block_until = i.now + REARM_BLOCK_S;
        self.unstick.reset_window();
        self.player = Some(pl);
        let why = if self.world_entered { "handback" } else { "world_enter" };
        self.world_entered = true;
        log_i!("MOVE", "resync why={why} d=0.00m chunk_d=0.00m");
        log_i!(
            "MOVE",
            "on pos={} vel0={:.1} er_ground={}/{}/{} falling={} grav_mult={:.3} grav_dis={} r={:.2} h={:.2} fp={}",
            logfmt::v3(r.pos),
            seed.len(),
            r.is_touching_ground as u8,
            r.standing_on_solid_ground as u8,
            r.touching_solid_ground as u8,
            r.is_falling as u8,
            r.gravity_mult,
            r.gravity_disabled,
            r.hit_radius,
            r.hit_height,
            i.fp_on as u8
        );
        self.own_gravity(r);
        self.last_written = None;
        self.assist.reset();
        self.blocked_since = None;
        // C3: the player's own filter layer (read-only), logged at the first MOVE on and on a mode change
        self.refresh_player_cast(i, r.physics, true);
    }

    /// C3 (docs/C3_DESIGN.md 1.3 / 1.6): reads `physics+0x338` (never written), picks the `player` mode's cast filter
    /// (layer only, accepted layers, else the fallback) and logs `MOVE filter live` (+ the one-shot table dump).
    fn refresh_player_cast(&mut self, i: &MoveIn, physics: usize, at_on: bool) {
        let rc = &i.cfg.movement.rays;
        let words = crate::game::c3_read::read_filter_words(physics);
        let (cast, src) = collision_layers::player_cast(words.main, &rc.player_layers, rc.player_fallback);
        let changed = cast != self.player_cast || src != self.cast_src;
        self.player_cast = cast;
        self.cast_src = src;
        self.cast_read_t = i.now;
        let mode_seen = self.filter_logged_mode.as_deref() == Some(rc.mode.as_str());
        if (at_on && !mode_seen) || changed {
            self.filter_logged_mode = Some(rc.mode.clone());
            let hx = |w: Option<u32>| w.map_or("?".to_string(), |v| format!("0x{v:x}"));
            let line = format!(
                "filter live phys338={} phys348={} physc8={} layer={} cast=0x{cast:x} src={} mode={} base2e8={} base2f8={} base_layer={}",
                hx(words.main),
                hx(words.second),
                hx(words.c8),
                words.main.map_or("?".to_string(), |v| format!("{}", collision_layers::layer_of(v))),
                src.label(),
                rc.mode,
                hx(words.base),
                hx(words.base2),
                words.base.map_or("?".to_string(), |v| format!("{}", collision_layers::layer_of(v)))
            );
            if src == CastSrc::Fallback && rc.is_player() {
                log_w!("MOVE", "WARN {line} (the live word is unreadable or its layer is not in player_layers {:?})", rc.player_layers);
            } else {
                log_i!("MOVE", "{line}");
            }
        }
        if at_on && rc.log_live_table && !self.table_logged {
            self.table_logged = true;
            let mut layers = vec![collision_layers::layer_of(MAP_FILTER), collision_layers::layer_of(OBJECT_FILTER)];
            let cl = collision_layers::layer_of(cast);
            if !layers.contains(&cl) {
                layers.push(cl);
            }
            for l in layers {
                match crate::game::c3_read::read_table_row(l) {
                    Ok(b) => {
                        let live = collision_layers::row_from_bytes(&b);
                        let same = live == collision_layers::row(l);
                        log_i!(
                            "MOVE",
                            "filter table row=0x{l:02x} live={} static={}",
                            collision_layers::row_hex(live),
                            if same { "match".to_string() } else { format!("DIFF want={}", collision_layers::row_hex(collision_layers::row(l))) }
                        );
                    }
                    Err(why) => {
                        log_i!("MOVE", "filter table row=0x{l:02x} skip why={why}");
                    }
                }
            }
        }
    }

    /// Gravity: the save is registered in `GravityOwner` BEFORE either field is written (section 4.2 #4/#5).
    fn own_gravity(&mut self, r: &MoveRead) {
        let (mult, dis) = self.gravity.own(r.physics, r.gravity_mult, r.gravity_disabled);
        // SAFETY: the player's physics module (read this tick); an f32 the engine sets per TAE / per frame.
        let ok1 = unsafe { write(r.physics + PHYS_GRAVITY_MULT, mult) };
        // SAFETY: as above; a bool (TAE ChrActionFlag 27).
        let ok2 = unsafe { write(r.physics + PHYS_GRAVITY_DISABLED, dis) };
        self.perf.writes += 2;
        log_i!(
            "MOVE",
            "write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=0x{:x} old={:.3} new={:.3} ok={}",
            r.physics + PHYS_GRAVITY_MULT,
            r.gravity_mult,
            mult,
            ok1 as u8
        );
        log_i!(
            "MOVE",
            "write field=CSChrPhysicsModule.gravity_disabled(+0x1d5) addr=0x{:x} old={} new={} ok={}",
            r.physics + PHYS_GRAVITY_DISABLED,
            r.gravity_disabled,
            dis,
            ok2 as u8
        );
        if !(ok1 && ok2) {
            self.pending_write_fail = Some("CSChrPhysicsModule.gravity");
        }
    }

    /// Restores (or defers) the gravity fields per the `GravityOwner` plan, with the log lines.
    fn restore_gravity(&mut self, in_world: bool) {
        let module = if in_world { current_physics() } else { 0 };
        let cur = if module != 0 { read_gravity(module) } else { None };
        let plan = self.gravity.release(in_world, module, cur);
        self.apply_gravity_plan(plan, module);
    }

    fn deferred_gravity(&mut self, in_world: bool) {
        if !self.gravity.deferred() || !in_world {
            return;
        }
        let module = current_physics();
        let cur = if module != 0 { read_gravity(module) } else { None };
        if cur.is_none() {
            // still unreadable: stay deferred quietly (no plan, no line every tick)
            return;
        }
        let plan = self.gravity.resolve_deferred(module, cur);
        self.apply_gravity_plan(plan, module);
    }

    /// After a task panic (task.rs runs this every tick while gameplay is disabled, guarded): a gravity restore the
    /// panic path had to defer (no physics module / unreadable fields at that moment) is retried until it resolves
    /// (review M5 minor 6). Writes only through the same `GravityOwner` plan as every other restore.
    pub fn disabled_tick(&mut self) {
        if self.gravity.deferred() {
            self.deferred_gravity(current_physics() != 0);
        }
    }

    fn apply_gravity_plan(&mut self, plan: ReleasePlan, module: usize) {
        match plan {
            ReleasePlan::Nothing => {}
            ReleasePlan::Deferred => log_i!("MOVE", "restore field=gravity_multiplier deferred (not in world; restored on return at the same module only)"),
            ReleasePlan::Stale(s) => log_i!(
                "MOVE",
                "restore field=gravity_multiplier addr=0x{:x} skipped=stale (another physics module 0x{module:x} now; dropped, never written)",
                s.module + PHYS_GRAVITY_MULT
            ),
            ReleasePlan::Restore { save, mult, disabled } => {
                match mult {
                    FieldPlan::Write { cur, new } => {
                        // SAFETY: the same physics module as the save (checked by GravityOwner), holding our 0.0.
                        let ok = unsafe { write(save.module + PHYS_GRAVITY_MULT, new) };
                        self.perf.writes += 1;
                        log_i!("MOVE", "restore field=gravity_multiplier addr=0x{:x} old={cur:.3} new={new:.3} ok={}", save.module + PHYS_GRAVITY_MULT, ok as u8);
                    }
                    FieldPlan::Skipped { cur } => log_i!(
                        "MOVE",
                        "restore field=gravity_multiplier addr=0x{:x} skipped=changed (ER set {cur:.3}; saved {:.3})",
                        save.module + PHYS_GRAVITY_MULT,
                        save.mult
                    ),
                }
                match disabled {
                    FieldPlan::Write { cur, new } => {
                        // SAFETY: as above; a bool holding our 1.
                        let ok = unsafe { write(save.module + PHYS_GRAVITY_DISABLED, new as u8) };
                        self.perf.writes += 1;
                        log_i!("MOVE", "restore field=gravity_disabled addr=0x{:x} old={} new={} ok={}", save.module + PHYS_GRAVITY_DISABLED, cur as u8, new as u8, ok as u8);
                    }
                    FieldPlan::Skipped { cur } => log_i!(
                        "MOVE",
                        "restore field=gravity_disabled addr=0x{:x} skipped=changed (ER set {}; saved {})",
                        save.module + PHYS_GRAVITY_DISABLED,
                        cur as u8,
                        save.disabled
                    ),
                }
            }
        }
    }

    /// Every handback: the restores, then the reason line. One match over every reason (no wildcard arm).
    fn release(&mut self, reason: Reason, i: &MoveIn, r: Option<&MoveRead>, write_fail: Option<&'static str>) {
        let in_world = i.in_world && r.is_some();
        let p = i.fp_player.ok().flatten();
        let detail = match reason {
            Reason::Panic => "task panic".to_string(),
            Reason::WorldLeft => if in_world { "main_player changed".into() } else { "world left".into() },
            Reason::Loading => format!("load_counter {:?}", self.last_load),
            Reason::ReadFail => format!("field={}", self.read_fail_field),
            Reason::WriteFail => format!("field={} why={}", write_fail.unwrap_or("?"), refusal_label(LAST_WRITE_REFUSAL.load(Ordering::Relaxed))),
            Reason::NoData => "converted\\movement.json missing or invalid".into(),
            Reason::Death => p.map_or(String::new(), |p| format!("hp={} death_flag={}", p.hp, (p.flags1c5 & fp_logic::DEATH_FLAG != 0) as u8)),
            Reason::Cutscene => format!("cam_mask={:?}", self.last_mask),
            Reason::Torrent => p.map_or(String::new(), |p| format!("ride_state={:?} mounting={}", p.ride_state, p.is_mounting as u8)),
            Reason::Ladder => p.map_or(String::new(), |p| format!("state={}", p.ladder)),
            Reason::Throw => p.map_or(String::new(), |p| format!("throw_state={:?}", p.throw_state)),
            Reason::Talk => format!("talk_id={:?}", self.last_talk),
            Reason::Interact => format!("E pressed (hold {:.1} s)", i.cfg.movement.interact_hold_s),
            Reason::ErMove => format!("er_delta > {:.2} m (hold {:.1} s)", i.cfg.movement.er_move_handback_m, i.cfg.movement.er_move_hold_s),
            Reason::GroundDisagree => format!(
                "ground ray missed {GROUND_DISAGREE_TICKS} ticks while ER reported ground (hold {GROUND_DISAGREE_HOLD_S:.0} s, held_ticks={})",
                self.er_delta.ground_held_ticks
            ),
            Reason::Unstick => format!(
                "no progress while ER pushed out / a probe was inside geometry (hold {:.1} s, {} of max {} within {:.0} s; gravity back to ER)",
                self.unstick_hold_s,
                self.unstick.recent(i.now, &i.cfg.movement.unstick.params()),
                i.cfg.movement.unstick.max_per_window,
                i.cfg.movement.unstick.backoff_window_s
            ),
            Reason::Assist => format!(
                "our controller stalled with a direction held (hold {:.1} s, {} of max {} within 60 s; ER's own movement walks)",
                self.assist_hold_s,
                self.assist.recent(i.now),
                i.cfg.movement.assist.max_per_min
            ),
            Reason::Freefall => format!("no ground for {:.0} m", i.cfg.movement.freefall_handback_m),
            Reason::Menu => "cursor".into(),
            Reason::Ac130 => "gunship".into(),
            Reason::Nuke => "tactical nuke: the player's death".into(),
            Reason::FpOnly => "fp_only and first person off".into(),
            Reason::TpEr => "tp_anim = er and first person off".into(),
            Reason::F5 => "MW2 mode off".into(),
            Reason::Config => "[movement] enabled = false".into(),
        };
        // M8: a running mantle is aborted BEFORE the gravity restore (ER gets the last validated position, already
        // written last tick, and drops it at most ~2 m with its own gravity)
        if let Some(pl) = self.player.as_mut()
            && let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = pl.mantle_abort(AbortWhy::Handback(reason.label()))
        {
            log_i!("MOVE", "mantle abort why=handback:{} t_frac={t_frac:.2} pos={} last_valid=1", reason.label(), logfmt::v3(pos * M_PER_IN));
        }
        if let Some(pl) = self.player.as_mut()
            && let Some(d) = move_data()
        {
            let cfg = Self::sim_cfg(i.cfg, &d, 0.4);
            if let Some(SprintEvent::End { why, dur, meter }) = pl.end_sprint_handback(&cfg, i.now) {
                log_i!("MOVE", "sprint end why={} dur={dur:.2}s meter={meter:.2}s", why.label());
            }
            if !reason.keeps_stance() && pl.stance.goal() != StanceKind::Stand {
                pl.stance.reset();
                log_i!("MOVE", "stance reset to=stand (reason={})", reason.label());
            }
            pl.vel = V3::ZERO;
        }
        cam_hook::unpublish_feet();
        self.restore_gravity(in_world);
        self.last_release = Some(reason);
        self.unstick.reset_window();
        self.assist.reset();
        self.blocked_since = None;
        self.er_delta.handbacks += 1;
        // M8: ER's position at the handback (the analyzer's ladder-signal row compares it with the re-arm position)
        let at = r.map_or("?".to_string(), |r| logfmt::v3(r.pos));
        match reason.kind() {
            mw2er_core::move_logic::Kind::Off => log_i!("MOVE", "off reason={} {detail} ticks={} pos={at}", reason.label(), self.ticks_driving),
            mw2er_core::move_logic::Kind::Suspend => {
                log_i!("MOVE", "suspend reason={} {detail} ticks={} pos={at}", reason.label(), self.ticks_driving)
            }
        }
    }

    /// Task panic (task.rs runs this before FP's and the AC-130's emergency paths): gravity back, feet unpublished,
    /// blank wishes cleared.
    pub fn emergency_off(&mut self, now: f64) {
        let was = self.ctl.emergency_off();
        if let Some(pl) = self.player.as_mut() {
            pl.mantle_abort(AbortWhy::Handback("panic"));
        }
        cam_hook::unpublish_feet();
        crate::shared::clear_move_blank();
        let module = current_physics();
        self.restore_gravity(module != 0);
        log_i!("MOVE", "off reason=panic was_driving={} t={now:.2}", was as u8);
    }

    /// One driving tick (section 1.2 steps 3-7).
    fn drive(&mut self, i: &MoveIn, r: &MoveRead, d: &MoveData, presses: &[u32; PRESS_KEYS], phys: usize) -> MoveOut {
        // review M5 major 2: no rays without the physics world and the player they ignore (tick() already turns a
        // missing world into a failed read; this keeps the invariant local)
        if phys == 0 || r.player == 0 {
            self.read_fail_streak += 1;
            self.read_fail_field = "CSHavokMan.phys_world";
            // M8: no rays, no re-validation: a running mantle stops here (nothing written this tick)
            if let Some(pl) = self.player.as_mut()
                && let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = pl.mantle_abort(AbortWhy::ReadFail)
            {
                log_i!("MOVE", "mantle abort why=read_fail t_frac={t_frac:.2} pos={} last_valid=1", logfmt::v3(pos * M_PER_IN));
            }
            return MoveOut::idle();
        }
        let cfg = i.cfg;
        let mc = &cfg.movement;
        let scfg = Self::sim_cfg(cfg, d, r.hit_radius);
        self.ticks_driving += 1;
        let Some(mut pl) = self.player.take() else { return MoveOut::idle() };
        // this tick's push-out (inches, planar): goes through the controller's collide-and-slide, never straight
        // into the position (review M5 major 3)
        let mut push_in = V3::ZERO;
        // M8: ER's planar displacement this tick (inches), for the mantle's er_push rule
        let mut mantle_er_in = 0.0f32;
        // run 18: ER's push-out applied this tick (the unstick detector's evidence)
        let mut er_pushed = false;

        // ---- resync: rebase / warp ----
        if let Some(last) = self.last_written {
            let e = r.pos - last;
            let ep = V3::new(e.x, 0.0, e.z).len();
            // M8 P1 (B5): the chunk jump as a vector; ER re-bases move it with the position (either sign accepted)
            let chunk_v = match (r.chunk, self.prev_chunk) {
                (Some(a), Some(b)) => Some(a - b),
                _ => None,
            };
            let load_age = i.now - self.last_load_change_t;
            let jv = classify_jump(e, chunk_v, pl.vel.len() * M_PER_IN * i.dt as f32, load_age < REBASE_LOAD_GRACE_S);
            let vtxt = |v: Option<V3>| v.map_or("?".to_string(), logfmt::v3);
            match jv.jump {
                Jump::Rebase => {
                    pl.translate(e * IN_PER_M);
                    let sign = if jv.sign == Some(-1) { "-" } else { "+" };
                    log_i!(
                        "MOVE",
                        "resync why=rebase d={ep:.2}m chunk_d={:.2}m e={} c={} block={:?} sign={sign} load_age={load_age:.1}s",
                        jv.chunk,
                        logfmt::v3(e),
                        vtxt(chunk_v),
                        r.block
                    );
                    if let Some(m) = pl.mantle.as_ref() {
                        log_i!("MOVE", "mantle rebase d={ep:.2}m sign={sign} t_frac={:.2} (the planned path moved with the player)", m.frac());
                    }
                }
                Jump::Warp => {
                    if let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = pl.mantle_abort(AbortWhy::Warp) {
                        log_i!("MOVE", "mantle abort why=warp t_frac={t_frac:.2} pos={} last_valid=1", logfmt::v3(pos * M_PER_IN));
                    }
                    if let Some(SprintEvent::End { why, dur, meter }) = pl.end_sprint_handback(&scfg, i.now) {
                        log_i!("MOVE", "sprint end why={} dur={dur:.2}s meter={meter:.2}s", why.label());
                    }
                    pl.resync(r.pos * IN_PER_M, V3::ZERO, r.is_touching_ground);
                    self.unstick.reset_window();
                    log_i!(
                        "MOVE",
                        "resync why=warp d={ep:.2}m chunk_d={:.2}m e={} c={} block={:?} sign=none load_age={load_age:.1}s",
                        jv.chunk,
                        logfmt::v3(e),
                        vtxt(chunk_v),
                        r.block
                    );
                }
                Jump::None => {
                    self.er_delta.samples.push(ep);
                    // M8: during a mantle ER's displacement is measured, never applied; a large one aborts it in the
                    // controller (er_push) and starts the existing ErMove handback. C3 PASS C: with W/A/S/D reaching ER
                    // (tp / fp hybrid) its expected run root motion along our motion is not ER pushing (M5 5.6)
                    let rm_max = if WASD_TO_ER.load(Ordering::Relaxed) { mc.fp_anim_rm_mps * i.dt as f32 } else { 0.0 };
                    let ec = rm_compensate(e, pl.last_dir, rm_max);
                    let ep = V3::new(ec.x, 0.0, ec.z).len();
                    mantle_er_in = ep * IN_PER_M;
                    if pl.mantling() && ep > mc.er_move_handback_m && !self.er_move_flap.muted() {
                        self.er_move.trigger(i.now, mc.er_move_hold_s as f64);
                    }
                    // ER's own proxy push-out (things our map rays do not see)
                    if mc.er_pushout && !pl.mantling() {
                        let pp = PushParams { max_m: mc.er_pushout_max_m, handback_m: mc.er_move_handback_m, min_m: 0.02 };
                        match er_pushout(e, pl.last_dir, &pp) {
                            ErDelta::Apply(v) => {
                                push_in = push_in + V3::new(v.x, 0.0, v.z) * IN_PER_M;
                                if let Some(n) = V3::new(v.x, 0.0, v.z).normalize() {
                                    pl.vel = remove_into(pl.vel, n);
                                }
                                self.er_delta.applied += 1;
                                er_pushed = true;
                            }
                            ErDelta::ErMove => {
                                if !self.er_move_flap.muted() {
                                    self.er_move.trigger(i.now, mc.er_move_hold_s as f64);
                                    if self.er_move_flap.record(i.now) {
                                        log_w!("MOVE", "WARN signal er_move muted for this session after {} handbacks", fp_logic::FLAP_MAX);
                                    }
                                }
                            }
                            ErDelta::Ignore => {}
                        }
                    }
                }
            }
        }
        // characters (circle push-out)
        self.others.clear();
        self.others.extend(i.chars.iter().filter(|c| c.handle != i.player_handle && c.hp > 0).map(|c| c.pos));
        let push = if pl.mantling() { V3::ZERO } else { char_pushout(pl.pos * M_PER_IN, scfg.radius * M_PER_IN, &self.others, mc.char_push_radius_m, 0.2) };
        // M8: the nearest characters (inches) for the mantle's `occupied` rule
        let mut others_in = [V3::ZERO; MAX_OTHERS];
        let mut n_others = 0usize;
        for c in self.others.iter() {
            if n_others < MAX_OTHERS && (*c - pl.pos * M_PER_IN).len() < 5.0 {
                others_in[n_others] = *c * IN_PER_M;
                n_others += 1;
            }
        }
        if push.len() > 0.0 {
            push_in = push_in + push * IN_PER_M;
            if let Some(n) = push.normalize() {
                pl.vel = remove_into(pl.vel, n);
            }
        }

        // ---- input ----
        // C3 PASS B: the pad's sprint / jump / stance levels and its left stick (analog; W/A/S/D win)
        let lv = RAW_KEYS.load(Ordering::Relaxed) | crate::pad::move_levels();
        let wasd = RAW_MOVE_KEYS.load(Ordering::Relaxed);
        let analog = crate::pad::analog();
        let (fwd_h, right_h) = i.cam_row2.and_then(|f| move_aim::axes(f, mc.strafe_sign)).unwrap_or((V3::new(0.0, 0.0, 1.0), V3::new(1.0, 0.0, 0.0)));
        let hold_pressed = presses[PRESS_STANCE_HOLD];
        let inp = Input {
            w: wasd & MOVE_W != 0,
            s: wasd & MOVE_S != 0,
            a: wasd & MOVE_A != 0,
            d: wasd & MOVE_D != 0,
            fwd_h,
            right_h,
            sprint_presses: presses[PRESS_SPRINT],
            sprint_held: lv & (1 << PRESS_SPRINT) != 0,
            jump_presses: presses[PRESS_JUMP],
            crouch_presses: presses[PRESS_CROUCH],
            crouch_held: lv & (1 << PRESS_CROUCH) != 0,
            prone_presses: presses[PRESS_PRONE],
            hold_tap: self.stance_hold_tap(i.now, hold_pressed, lv & (1 << PRESS_STANCE_HOLD) != 0, d, false),
            hold_long: self.stance_hold_tap(i.now, 0, lv & (1 << PRESS_STANCE_HOLD) != 0, d, true),
            fire: i.fire_held,
            ads: i.ads_held,
            reload: i.reload_press,
            er_ground: Some(r.is_touching_ground),
            push: push_in,
            // M8: the jump level (a press edge counts as held) and the mantle gate's DLL part
            jump_held: lv & (1 << PRESS_JUMP) != 0 || presses[PRESS_JUMP] > 0,
            mantle_block: !mc.mantle.enabled || i.now < self.mantle_rearm_until,
            er_delta_in: mantle_er_in,
            others: others_in,
            n_others: n_others as u8,
            analog,
        };

        // ---- the controller ----
        // rays ignore the same main_player whose physics module is written (review M5 major 2)
        // C3: mode "player" casts once with the player's own proxy layer (re-read every 5 s, never written)
        if mc.rays.is_player() && i.now - self.cast_read_t >= PLAYER_CAST_REFRESH_S {
            self.refresh_player_cast(i, r.physics, false);
        }
        let (filters, n_filters) = mc.rays.filters_with(self.player_cast);
        let mut world = HavokRays { world: phys, player_ins: r.player, w: r.pos_w, filters, n_filters, hits: [0; 3] };
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        let tick_t0 = std::time::Instant::now();
        let st = pl.tick(d, &scfg, &inp, i.now, i.dt, &mut world, &mut events);
        // run 18: which filter the rays hit (map / object / both), for the state, er_delta and mantle lines
        self.hits_tick = world.hits;
        for k in 0..3 {
            self.hits_state[k] += world.hits[k];
            self.hits_period[k] += world.hits[k];
        }
        if st.mantle_checks > 0 || st.mantling {
            self.perf.mantle_us_max = self.perf.mantle_us_max.max(tick_t0.elapsed().as_secs_f64() * 1e6);
        }
        self.perf.mantle_checks += st.mantle_checks;
        self.perf.mantle_rays_max = self.perf.mantle_rays_max.max(st.mantle_rays);
        if st.mantle_checks > 0 || st.mantling || st.mantle_rays > 0 {
            self.perf.mantle_budget_max = self.perf.mantle_budget_max.max(st.mantle_budget);
        }
        self.perf.mantle_deferred += st.mantle_deferred;
        // ground kept on ER's signal only: count, hand back if it lasts (review M5 major 1)
        if st.ground_held > 0 {
            self.ground_hold_streak += 1;
            self.er_delta.ground_held_ticks += 1;
            if self.ground_hold_streak == GROUND_DISAGREE_TICKS {
                self.ground_disagree.trigger(i.now, GROUND_DISAGREE_HOLD_S);
                log_i!(
                    "MOVE",
                    "ground_disagree ray_miss_ticks={} er_ground=1 pos={} (ER stands on something our map rays miss; ER drives {GROUND_DISAGREE_HOLD_S:.0} s)",
                    self.ground_hold_streak,
                    logfmt::v3(r.pos)
                );
            }
        } else {
            self.ground_hold_streak = 0;
        }
        self.perf.rays_sum += st.rays as u64;
        self.perf.rays_max = self.perf.rays_max.max(st.rays);
        self.perf.ticks += 1;
        if st.budget_hit {
            self.er_delta.budget_hits += 1;
        }
        let mut out = MoveOut::idle();
        for e in &events {
            self.log_event(i, r, d, &scfg, &pl, e, &mut out);
        }
        // M8: did ER see the player on the top after the end (within 10 ticks)?
        if events.iter().any(|e| matches!(e, SimEvent::MantleEnd { .. })) {
            self.mantle_settle = Some(0);
        } else if let Some(n) = self.mantle_settle {
            if r.is_touching_ground || n + 1 >= 10 || pl.mantling() {
                log_i!("MOVE", "mantle settle er_ground={} ticks={} ground={} pos={}", r.is_touching_ground as u8, n + 1, pl.grounded as u8, logfmt::v3(r.pos));
                self.mantle_settle = None;
            } else {
                self.mantle_settle = Some(n + 1);
            }
        }
        self.events = events;
        // run 18: no progress while W/A/S/D is held and ER keeps pushing the proxy out (or a probe is inside):
        // hand back to ER for a moment (Reason::Unstick, gravity restored), with a doubling hold and a cap
        let ui = UnstickIn {
            now: i.now,
            pos: pl.pos * M_PER_IN,
            wish: inp.wish(),
            pushed: er_pushed,
            inside: st.inside > 0,
            mantling: pl.mantling() || st.mantling,
        };
        match self.unstick.tick(&ui, &mc.unstick.params()) {
            Some(UnstickEvent::Fire(f)) => {
                self.unsticks += 1;
                self.unstick_hold_s = f.hold_s;
                self.unstick_hold.trigger(i.now, f.hold_s);
                log_i!(
                    "MOVE",
                    "unstick n={} hold={:.1}s window={:.2}s moved={:.2}m wish={}/{} pushed={} inside={} pos={} er_pos={} stance={} hits=map:{}/obj:{}/both:{}",
                    f.n,
                    f.hold_s,
                    f.window_s,
                    f.moved_m,
                    f.wish_ticks,
                    f.ticks,
                    f.pushed_ticks,
                    f.inside_ticks,
                    logfmt::v3(pl.pos * M_PER_IN),
                    logfmt::v3(r.pos),
                    pl.stance.effective().label(),
                    self.hits_period[0],
                    self.hits_period[1],
                    self.hits_period[2]
                );
            }
            Some(UnstickEvent::Capped { n, until }) => log_w!(
                "MOVE",
                "WARN unstick capped n={n} within {:.0} s: the detector rests until t={until:.1} (ER push-out keeps the player out meanwhile)",
                mc.unstick.backoff_window_s
            ),
            None => {}
        }
        // C3 (docs/C3_DESIGN.md 1.4 / 1.6): a direction held and no progress in ER's world: the blocked-episode line and
        // the assist hand-back (ER's own movement for a moment, then resync)
        let speed_world = match (self.prev_er, i.dt > 0.0) {
            (Some(prev), true) => V3::new(r.pos.x - prev.x, 0.0, r.pos.z - prev.z).len() / i.dt as f32,
            _ => f32::INFINITY,
        };
        let wish = inp.wish();
        let slope_er = |r: &MoveRead| r.material_normal.map_or("?".to_string(), |n| format!("{:.1}", n.y.clamp(-1.0, 1.0).acos().to_degrees()));
        if wish && pl.grounded && speed_world < mc.assist.stall_speed && !pl.mantling() {
            let since = *self.blocked_since.get_or_insert(i.now);
            if i.now - since >= BLOCKED_EPISODE_S && i.now - self.blocked_log_t >= 1.0 {
                self.blocked_log_t = i.now;
                log_i!(
                    "MOVE",
                    "blocked wish={:.1} speed={speed_world:.2} slope_er={}deg mode={} cast=0x{:x} hits=map:{}/obj:{}/both:{} dt={:.2}s pos={}",
                    pl.last_wish,
                    slope_er(r),
                    mc.rays.mode,
                    self.player_cast,
                    self.hits_period[0],
                    self.hits_period[1],
                    self.hits_period[2],
                    i.now - since,
                    logfmt::v3(r.pos)
                );
            }
        } else {
            self.blocked_since = None;
        }
        let ai = AssistIn {
            now: i.now,
            wish,
            grounded: pl.grounded,
            speed: if speed_world.is_finite() { speed_world } else { 99.0 },
            refusal: std::mem::take(&mut self.refused_tick),
            blocked: pl.mantling() || st.mantling || i.menu_open || i.ac130_block || i.nuke_block,
        };
        match self.assist.tick(&ai, &mc.assist.params()) {
            Some(AssistEvent::Fire { n, hold_s, why, stalled_s }) => {
                self.assists += 1;
                self.assist_hold_s = hold_s;
                self.assist_hold.trigger(i.now, hold_s);
                log_i!(
                    "MOVE",
                    "assist n={n} why={} hold={hold_s:.1}s speed={:.2} slope_er={} stalled={stalled_s:.2}s mode={} cast=0x{:x} pos={}",
                    why.label(),
                    ai.speed,
                    slope_er(r),
                    mc.rays.mode,
                    self.player_cast,
                    logfmt::v3(r.pos)
                );
            }
            Some(AssistEvent::Capped { n, until }) => {
                log_w!("MOVE", "WARN assist capped n={n} within 60 s: the detector rests until t={until:.1}");
            }
            None => {}
        }
        if mc.lethal_fall == LethalFall::Handback && !pl.grounded && pl.air_top_y - pl.pos.y >= d.mw2.fall_max && !self.freefall {
            self.freefall = true;
            log_i!("MOVE", "lethal fall predicted ({:.1} m): ER resolves it (lethal_fall = handback)", (pl.air_top_y - pl.pos.y) * M_PER_IN);
        }

        // ---- writes (section 4.2 #1, #2, #6) ----
        let pos_m = pl.pos * M_PER_IN;
        // review M5 minor 4: never hand Havok a non-finite or far-off position: resync to ER, write nothing this
        // tick and hand back (write_fail) on the next
        if !position_write_ok(pos_m, r.pos, POS_WRITE_MAX_M) {
            log_w!(
                "MOVE",
                "WARN position_insane new={} er={} (max {POS_WRITE_MAX_M:.1} m): not written, resync to ER",
                logfmt::v3(pos_m),
                logfmt::v3(r.pos)
            );
            // M8 review: a running mantle is aborted (and logged) before the resync drops it silently
            if let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = pl.mantle_abort(AbortWhy::Insane) {
                log_i!("MOVE", "mantle abort why=position_insane t_frac={t_frac:.2} pos={} last_valid=1", logfmt::v3(pos * M_PER_IN));
            }
            pl.resync(r.pos * IN_PER_M, V3::ZERO, r.is_touching_ground);
            self.pending_write_fail = Some("CSChrPhysicsModule.position (not finite or > 5 m from ER: skipped)");
            self.player = Some(pl);
            return MoveOut::idle();
        }
        let new = [pos_m.x, pos_m.y, pos_m.z, r.pos_w];
        let ok = if mc.write_last_update {
            // SAFETY: the player's physics module (read this tick): position + last_update_position, 2 x f32x4.
            unsafe { write(r.physics + PHYS_POSITION, [new, new]) }
        } else {
            // SAFETY: the player's physics module (read this tick): position f32x4, w kept.
            unsafe { write(r.physics + PHYS_POSITION, new) }
        };
        // SAFETY: as above: chr_proxy_pos_update_requested, a request flag ER consumes.
        let ok2 = unsafe { write(r.physics + PHYS_PROXY_REQ, 1u8) };
        self.perf.writes += 2;
        if ok {
            self.log_once("pos", || {
                format!(
                    "write field=CSChrPhysicsModule.position(+0x70) addr=0x{:x} old={} new={}{}",
                    r.physics + PHYS_POSITION,
                    logfmt::v3(r.pos),
                    logfmt::v3(pos_m),
                    if mc.write_last_update { " (+ last_update_position +0x80)" } else { "" }
                )
            });
        } else {
            self.pending_write_fail = Some("CSChrPhysicsModule.position");
        }
        if ok2 {
            self.log_once("proxy", || {
                format!("write field=CSChrPhysicsModule.chr_proxy_pos_update_requested(+0x91) addr=0x{:x} old=? new=1", r.physics + PHYS_PROXY_REQ)
            });
        } else {
            self.pending_write_fail = Some("CSChrPhysicsModule.chr_proxy_pos_update_requested");
        }
        if mc.proxy_sync_flag
            && r.chr_ctrl != 0
            && let Some(f) = r.proxy_flags
        {
            // SAFETY: ChrCtrl.chr_proxy_flags (read this tick): bit 0 = position sync request, the rest kept.
            let ok3 = unsafe { write(r.chr_ctrl + CHR_CTRL_PROXY_FLAGS, f | 1) };
            self.perf.writes += 1;
            self.log_once("proxyflags", || format!("write field=ChrCtrl.chr_proxy_flags(+0xfc) bit0 old=0x{f:x} new=0x{:x} ok={}", f | 1, ok3 as u8));
        }
        // gravity re-assert (ER rewrites the TAE flag)
        if self.gravity.reassert(r.physics, r.gravity_disabled) {
            // SAFETY: the owned physics module's gravity_disabled bool.
            let okg = unsafe { write(r.physics + PHYS_GRAVITY_DISABLED, 1u8) };
            self.perf.writes += 1;
            if self.gravity.reasserts == 1 {
                log_i!("MOVE", "gravity reasserted count=1 (first; ER set gravity_disabled={} ok={})", r.gravity_disabled, okg as u8);
            }
        }
        if r.gravity_mult != 0.0 && self.gravity.saved().is_some_and(|s| s.module == r.physics) {
            // SAFETY: as above; the multiplier back to 0 (ER rewrote it).
            unsafe { write(r.physics + PHYS_GRAVITY_MULT, 0.0f32) };
            self.perf.writes += 1;
        }
        self.last_written = Some(pos_m);

        // ---- publish (section 1.6) ----
        let eye_in = pl.eye_height(d, &scfg, i.now, mc.land_dip) + if mc.view_bob { pl.view_bob(d, i.ads_held) } else { 0.0 };
        let eye_h_m = eye_in * M_PER_IN;
        cam_hook::publish_feet(pos_m, mc.publish_max_m);
        let stance = match pl.stance.effective() {
            StanceKind::Stand => 0,
            StanceKind::Crouch => 1,
            StanceKind::Prone => 2,
        };
        let eye = pos_m + V3::new(0.0, eye_h_m, 0.0);
        out.driving = true;
        out.feet_m = Some(pos_m);
        out.eye_h_m = Some(eye_h_m);
        out.chest = Some(move_aim::chest_ref(eye, cfg.first_person.eye_height_m, cfg.aim.chest_height_m));
        out.sprinting = Some(pl.sprint.active());
        out.fire_blocked = pl.sprint.fire_blocked(i.now);
        out.stance = stance;
        out.spread_stance = [SpreadStance::Stand, SpreadStance::Ducked, SpreadStance::Prone][stance as usize];
        out.speed_mps = pl.vel.planar_len() * M_PER_IN;
        // M8: the mantle's weapon rule (rows >= 33 in lower the gun; 27 / 21 keep it, MW2)
        out.mantling = pl.mantling();
        let lower = pl.mantle.as_ref().is_some_and(|m| m.drop_weapon);
        out.mantle_vm_sprint = lower && scfg.mantle.vm == mw2er_core::mantle::VmMode::Sprint;
        out.mantle_lowered = lower;
        if mc.vm_stance_pose {
            let w = &d.weapon;
            let pp = StancePoseParams {
                ducked_ofs: w.ducked_ofs,
                prone_ofs: w.prone_ofs,
                stand_move: w.stand_move,
                ducked_move: w.ducked_move,
                prone_move: w.prone_move,
                strafe_move: w.strafe_move,
                move_rate: w.pos_move_rate,
                prone_move_rate: w.pos_prone_move_rate,
                bob: d.mw2.weap_bob,
                bob_max: d.mw2.weap_bob_max,
                ads_bob_factor: w.ads_bob_factor,
                move_threshold: d.mw2.move_threshold,
            };
            let rm = (inp.d as i32 - inp.a as i32) as f32;
            let pin = StancePoseIn {
                stance,
                speed: pl.vel.planar_len(),
                strafe: rm,
                sprinting: pl.sprint.active(),
                airborne: !pl.grounded,
                ads: if i.ads_held { 1.0 } else { 0.0 },
                walked: pl.walked,
            };
            out.vm_offset_in = offset_from_fru(self.pose.step(&pp, &pin, i.dt as f32));
        }

        // ---- cross-checks and lines ----
        if pl.grounded != r.is_touching_ground {
            self.disagree += 1;
            if self.disagree == DISAGREE_STREAK {
                self.er_delta.disagree_streaks += 1;
            }
        } else {
            self.disagree = 0;
        }
        self.state_line(i, r, d, &pl, st.rays);
        self.er_delta_line(i.now);
        self.order_probe();
        self.player = Some(pl);
        out
    }

    /// Console-style stance key (optional): a tap (< cl_stanceHoldTime) toggles crouch, a hold goes prone.
    fn stance_hold_tap(&mut self, now: f64, presses: u32, held: bool, d: &MoveData, long: bool) -> bool {
        let hold_s = d.mw2.cl_stance_hold_ms as f64 / 1000.0;
        if presses > 0 {
            self.hold_t0 = Some(now);
            self.hold_fired = false;
        }
        match (self.hold_t0, long) {
            (Some(t0), true) if held && !self.hold_fired && now - t0 >= hold_s => {
                self.hold_fired = true;
                true
            }
            (Some(t0), false) if !held && presses == 0 => {
                self.hold_t0 = None;
                !self.hold_fired && now - t0 < hold_s
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn log_event(&mut self, i: &MoveIn, r: &MoveRead, d: &MoveData, scfg: &mw2er_core::move_sim::SimCfg, pl: &Player, e: &SimEvent, out: &mut MoveOut) {
        let mc = &i.cfg.movement;
        match *e {
            SimEvent::Sprint(SprintEvent::Start { meter, from_low_stance }) => {
                out.sprint_started = true;
                log_i!("MOVE", "sprint start meter={meter:.2}s stance_from={}", if from_low_stance { pl.stance.leg().map_or("crouch", |l| l.from.label()) } else { "stand" });
            }
            SimEvent::Sprint(SprintEvent::End { why, dur, meter }) => {
                log_i!("MOVE", "sprint end why={} dur={dur:.2}s meter={meter:.2}s", why.label());
            }
            SimEvent::Sprint(SprintEvent::Refused { why, meter }) => {
                log_i!("MOVE", "sprint refused why={} meter={meter:.2}s", why.label());
            }
            SimEvent::Stance(StanceEvent::Leg { from, to, dur_ms }) => {
                log_i!("MOVE", "stance from={} to={} dur_ms={dur_ms:.0}", from.label(), to.label());
            }
            SimEvent::Stance(StanceEvent::Blocked { to }) => {
                if i.now - self.stance_blocked_t >= 1.0 {
                    self.stance_blocked_t = i.now;
                    log_i!("MOVE", "stance blocked to={} why=headroom", to.label());
                }
            }
            SimEvent::Jump { y0, v0, land_factor } => {
                out.jumped = true;
                log_i!("MOVE", "jump y0={:.3} v0={v0:.1} land_factor={land_factor:.2}", y0 * M_PER_IN);
            }
            SimEvent::Apex { h, t, ceiling } => {
                log_i!("MOVE", "apex h={:.3}m t={t:.3}s{}", h * M_PER_IN, if ceiling { " ceiling=1" } else { "" });
            }
            SimEvent::Land(l) => {
                let hp = self.apply_fall_damage(mc.fall_damage, r, l.dmg_pct, mc.fall_damage_scale, i.now, pl.fall_er_confirmed(), pl.er_ground_air);
                log_i!(
                    "MOVE",
                    "land fall_h={:.1}in ({:.2}m) v_impact={:.1} slow={:.2}*{:.2} dmg_pct={:.1} hp={} land_ms={:.0}",
                    l.fall_h,
                    l.fall_h * M_PER_IN,
                    fall::impact_speed(&d.mw2, l.fall_h),
                    l.slow_a,
                    l.slow_b,
                    l.dmg_pct,
                    hp,
                    l.land_ms
                );
            }
            SimEvent::Freefall { fallen } => {
                self.freefall = true;
                log_i!("MOVE", "freefall fallen={:.1}m (ER resolves it)", fallen * M_PER_IN);
            }
            // ---- M8 mantle lines (docs/M8_DESIGN.md section 9) ----
            SimEvent::MantleStart(m) => {
                self.perf.mantles += 1;
                let dir_deg = m.dir.x.atan2(m.dir.z).to_degrees();
                log_i!(
                    "MOVE",
                    "mantle start kind={} rise={:.2}m row={} ledge_y={:.3} from={} end={} dir_deg={dir_deg:.1} d={:.2}m X={:.2}m dur={:.2}s rays={} trigger={} air={} v0y={:.1} retried={} weapon={}{} depen={:.2}m narrow_top={} line_up={:.2}m hits=map:{}/obj:{}/both:{}",
                    m.kind.label(),
                    m.rise * M_PER_IN,
                    m.row_height,
                    m.ledge_y * M_PER_IN,
                    logfmt::v3(m.from * M_PER_IN),
                    logfmt::v3(m.end * M_PER_IN),
                    m.d * M_PER_IN,
                    m.x * M_PER_IN,
                    m.dur,
                    m.rays,
                    m.trigger.label(),
                    m.air as u8,
                    m.v0y,
                    m.retried as u8,
                    if m.drop_weapon { "drop" } else { "up" },
                    m.drop_y.map_or(String::new(), |y| format!(" drop_to={:.2}m", y * M_PER_IN)),
                    m.depen * M_PER_IN,
                    m.narrow_top as u8,
                    m.line_up * M_PER_IN,
                    self.hits_tick[0],
                    self.hits_tick[1],
                    self.hits_tick[2]
                );
            }
            SimEvent::MantleEnd { dt, kind, pos, dist_end, vel } => {
                log_i!(
                    "MOVE",
                    "mantle end dt={dt:.2}s pos={} dist_end={:.3}m kind={} ground={} er_ground={} vel={}",
                    logfmt::v3(pos * M_PER_IN),
                    dist_end * M_PER_IN,
                    kind.label(),
                    pl.grounded as u8,
                    r.is_touching_ground as u8,
                    logfmt::v3(vel * M_PER_IN)
                );
            }
            SimEvent::MantleRelease { drop_to, pos } => {
                log_i!(
                    "MOVE",
                    "mantle release kind=over pos={} drop_to={}",
                    logfmt::v3(pos * M_PER_IN),
                    drop_to.map_or("?".to_string(), |y| format!("{:.2}m", y * M_PER_IN))
                );
            }
            SimEvent::MantleAbort { why, t_frac, pos } => {
                log_i!("MOVE", "mantle abort why={} t_frac={t_frac:.2} pos={} last_valid=1", why.label(), logfmt::v3(pos * M_PER_IN));
            }
            SimEvent::MantleRefuse { why, rise, rays, fwd } => {
                self.perf.mantle_refusals += 1;
                // C3: the assist's shortcut (only while stalled, see move_assist)
                if matches!(why, Refuse::NoFace | Refuse::PathBlocked | Refuse::Edge) {
                    self.refused_tick = true;
                }
                let k = why.index();
                self.refuse_count[k] += 1;
                // M8 review: no_face is logged too (sampled: it fires on every held jump in the open), with the view
                let every = if why == Refuse::NoFace { NO_FACE_LOG_S } else { MANTLE_REFUSE_LOG_S };
                if mc.mantle.log_refusals && i.now - self.refuse_log_t[k] >= every {
                    log_i!(
                        "MOVE",
                        "mantle check ok=0 why={} rise={:.2}m rays={rays} n={} pos={} fwd=({:.2},{:.2}) hits=map:{}/obj:{}/both:{}",
                        why.label(),
                        rise * M_PER_IN,
                        self.refuse_count[k],
                        logfmt::v3(pl.pos * M_PER_IN),
                        fwd.x,
                        fwd.z,
                        self.hits_tick[0],
                        self.hits_tick[1],
                        self.hits_tick[2]
                    );
                    self.refuse_log_t[k] = i.now;
                    self.refuse_count[k] = 0;
                }
            }
        }
        let _ = scfg;
    }

    /// The fall-damage HP write (section 4.4): the only write that is never restored. At most once per landing,
    /// never above the current HP, only with `fall_damage = "mw2"`. Returns the `hp=a->b` text.
    /// Review M5 major 1: no HP write for a fall ER did not see as one (`er_airborne` false: ER reported ground on
    /// `er_ticks` ticks of it after the grace), since our rays and ER disagree about the floor.
    #[allow(clippy::too_many_arguments)]
    fn apply_fall_damage(&mut self, mode: FallDamageCfg, r: &MoveRead, dmg_pct: f32, scale: f32, now: f64, er_airborne: bool, er_ticks: u32) -> String {
        if mode == FallDamageCfg::Off {
            return format!("{}->{} max_hp={} fall_damage=off", r.hp, r.hp, r.max_hp);
        }
        if dmg_pct > 0.0 && !er_airborne {
            return format!("{}->{} max_hp={} fall_damage=skipped_er_ground er_ground_ticks={er_ticks}", r.hp, r.hp, r.max_hp);
        }
        let Some(new) = fall::hp_after(FallDamage::Mw2, r.hp, r.max_hp, dmg_pct, scale) else {
            return format!("{}->{} max_hp={}", r.hp, r.hp, r.max_hp);
        };
        // SAFETY: the player's CSChrDataModule (read this tick); hp i32, a lower value only (hp_after never raises).
        let ok = unsafe { write(r.data_module + DATA_HP, new) };
        self.perf.writes += 1;
        if ok && new == 0 {
            self.lethal_t = Some(now);
        }
        format!("{}->{} max_hp={}{}", r.hp, new, r.max_hp, if ok { "" } else { " write_failed" })
    }

    fn publish_blank(&mut self, i: &MoveIn, driving: bool) {
        // the keys and the ER blank list are config-derived: built once per config generation (review M5 minor 8)
        if self.keys_cache.as_ref().is_none_or(|c| c.0 != i.cfg_gen) {
            let keys = i.cfg.keys.move_keys().unwrap_or(MoveKeys::mw2_defaults());
            self.keys_cache = Some((i.cfg_gen, keys, i.cfg.movement.blank_er_diks()));
        }
        let Some((_, keys, diks)) = self.keys_cache.as_ref() else { return };
        let ctx = BlankCtx {
            driving: driving && i.blank_ok,
            fp: i.fp_on,
            hybrid: i.cfg.movement.tp_anim == TpAnim::Hybrid,
            // C3 PASS C (docs/C3_DESIGN.md 3.6): `fp_anim = "hybrid"`: W/A/S/D and the left stick reach ER in first
            // person too, so ER plays its walk / run animations (peers see them); our position write discards the root
            // motion as in third person
            fp_hybrid: i.cfg.movement.fp_anim == mw2er_core::config::FpAnim::Hybrid,
        };
        let to_er = ctx.driving && !mw2er_core::move_input::wasd_blanked(ctx);
        WASD_TO_ER.store(to_er, Ordering::Relaxed);
        FP_HYBRID_ACTIVE.store(to_er && i.fp_on, Ordering::Relaxed);
        let (latch, plain) = blank_masks(keys, diks, ctx);
        // C3 PASS B (docs/C3_DESIGN.md 4.5): the pad's left stick follows the same rule as W/A/S/D
        crate::pad::set_blank_ls(mw2er_core::move_input::wasd_blanked(ctx));
        for w in 0..4 {
            MOVE_BLANK_LATCH[w].store(latch[w], Ordering::Relaxed);
            MOVE_BLANK_PLAIN[w].store(plain[w], Ordering::Relaxed);
        }
        if self.blank_seen != Some((latch, plain)) {
            self.blank_seen = Some((latch, plain));
            let names = |m: &KeyMask| -> String {
                let mut v: Vec<String> = Vec::new();
                for k in 0..=255u8 {
                    if mask_has(m, k) {
                        v.push(key_name(k));
                    }
                }
                if v.is_empty() { "none".into() } else { v.join(",") }
            };
            let latched: u32 = (0..4).map(|w| MOVE_LATCHED[w].load(Ordering::Relaxed).count_ones()).sum();
            log_i!(
                "MOVE",
                "keys blank={} wasd={} ({}) latched={latched} blanked_reads={} n={}",
                names(&latch),
                names(&plain),
                if i.fp_on { "fp" } else { "tp" },
                MOVE_BLANKED_READS.load(Ordering::Relaxed),
                mask_count(&latch) + mask_count(&plain)
            );
        }
    }

    fn keyprobe(&mut self, i: &MoveIn, probe: &[u32; mw2er_core::move_input::N_PROBE], r: Option<&MoveRead>) {
        if !i.cfg.movement.keyprobe || i.mode_on || !i.in_world {
            return;
        }
        let bits = r.and_then(|r| r.action_requests).unwrap_or(0);
        for k in 0..9 {
            if self.probe_logged[k] {
                continue;
            }
            if probe[k] > 0 && self.probe_pending[k].0 == f64::NEG_INFINITY {
                self.probe_pending[k] = (i.now, 0);
            }
            let (t0, acc) = self.probe_pending[k];
            if t0 == f64::NEG_INFINITY {
                continue;
            }
            let acc = acc | bits;
            self.probe_pending[k].1 = acc;
            if i.now - t0 >= KEYPROBE_WINDOW_S {
                self.probe_logged[k] = true;
                log_i!("MOVE", "keyprobe key={} er_bits={}", PROBE_KEYS[k].0, er_bit_names(acc));
            }
        }
    }

    fn state_line(&mut self, i: &MoveIn, r: &MoveRead, d: &MoveData, pl: &Player, rays: u32) {
        let m = &d.mw2;
        let speed_world = match (self.prev_er, i.dt > 0.0) {
            (Some(prev), true) => V3::new(r.pos.x - prev.x, 0.0, r.pos.z - prev.z).len() / i.dt as f32,
            _ => 0.0,
        };
        let moving = pl.vel.planar_len() > 1.0;
        let key: StateKey = (pl.stance.effective(), pl.sprint.active(), pl.grounded, moving);
        let periodic = moving && self.ticks_driving % i.cfg.movement.log_every_ticks.max(1) as u64 == 0;
        let changed = self.last_state_key != Some(key) && i.now - self.last_state_t >= STATE_LINE_MIN_S;
        if !(periodic || changed) {
            return;
        }
        self.last_state_t = i.now;
        self.last_state_key = Some(key);
        let slope = |n: V3| n.y.clamp(-1.0, 1.0).acos().to_degrees();
        let ed = self.er_delta.samples.last().copied().unwrap_or(0.0);
        log_i!(
            "MOVE",
            "state t={:.2} stance={} frac={:.2} speed_sim={:.1}in/s speed_world={:.2}m/s wish={:.1} sprint={} meter={:.2}s ground={} er_ground={}/{}/{} slope_rays={:.1}deg slope_er={} rays={rays} er_delta={ed:.3}m pushout={} hits=map:{}/obj:{}/both:{}",
            i.now,
            pl.stance.effective().label(),
            pl.stance.frac(i.now),
            pl.vel.planar_len(),
            speed_world,
            pl.last_wish,
            pl.sprint.active() as u8,
            pl.sprint.meter(&Self::sim_cfg(i.cfg, d, r.hit_radius).sprint, i.now),
            pl.grounded as u8,
            r.is_touching_ground as u8,
            r.standing_on_solid_ground as u8,
            r.touching_solid_ground as u8,
            slope(pl.normal),
            r.material_normal.map_or("?".to_string(), |n| format!("{:.1}deg", slope(n))),
            (self.er_delta.applied > 0 && ed > 0.02) as u8,
            self.hits_state[0],
            self.hits_state[1],
            self.hits_state[2]
        );
        self.hits_state = [0; 3];
        let _ = m;
        let _ = GroundNormalCfg::Rays;
    }

    fn er_delta_line(&mut self, now: f64) {
        if now - self.last_er_delta_log < ER_DELTA_LOG_S {
            return;
        }
        self.last_er_delta_log = now;
        let s = &mut self.er_delta;
        if s.samples.is_empty() {
            return;
        }
        s.samples.sort_by(|a, b| a.total_cmp(b));
        let n = s.samples.len();
        log_i!(
            "MOVE",
            "er_delta p50={:.3} p95={:.3} max={:.3} applied={} handbacks={} budget_hits={} gravity_reasserts={} ground_disagree_streaks={} ground_held_ticks={} unsticks={} hits=map:{}/obj:{}/both:{}",
            s.samples[n / 2],
            s.samples[(n * 95 / 100).min(n - 1)],
            s.samples[n - 1],
            s.applied,
            s.handbacks,
            s.budget_hits,
            self.gravity.reasserts,
            s.disagree_streaks,
            s.ground_held_ticks,
            self.unsticks,
            self.hits_period[0],
            self.hits_period[1],
            self.hits_period[2]
        );
        s.samples.clear();
        self.hits_period = [0; 3];
    }

    fn order_probe(&mut self) {
        if self.order_logged {
            return;
        }
        let calls = cam_hook::DETOUR_CALLS.load(Ordering::Relaxed);
        if self.order_m > 0 && calls > self.last_detour_calls {
            self.order_n += 1;
        }
        self.last_detour_calls = calls;
        self.order_m += 1;
        if self.order_m > ORDER_TICKS {
            self.order_logged = true;
            log_i!(
                "MOVE",
                "order detour_before_task={}/{} (camera-step detour calls between task ticks; feet used={} rejected={})",
                self.order_n,
                self.order_m - 1,
                cam_hook::MOVE_FEET_USED.load(Ordering::Relaxed),
                cam_hook::MOVE_FEET_REJECTED.load(Ordering::Relaxed)
            );
        }
    }

    fn probe_line(&mut self, now: f64, r: &MoveRead, ladder: i8) {
        if now - self.last_probe_t < PROBE_LINE_MIN_S {
            return;
        }
        // compared as plain values; formatted only when they changed (review M5 minor 8)
        let vals = ProbeVals::of(r, ladder);
        if self.last_probe == Some(vals) {
            return;
        }
        self.last_probe = Some(vals);
        self.last_probe_t = now;
        log_i!(
            "MOVE",
            "probe movement_limit={:?} anim_flags=0x{:x} mod_flags=0x{:x} motion_mult={} root_motion={} ladder={ladder}",
            r.movement_limit,
            r.anim_flags.unwrap_or(0),
            r.mod_flags.unwrap_or(0),
            r.motion_mult.map_or("?".into(), |m| format!("{m:.2}")),
            r.root_motion.map_or("?".into(), |v| format!("({:.2},{:.2},{:.2})", v[0], v[1], v[2]))
        );
    }

    /// The `PERF ... move=..` fields of this 5 s window (task.rs appends them to its PERF line).
    pub fn perf_take(&mut self) -> String {
        let p = std::mem::take(&mut self.perf);
        let mut us = p.us;
        us.sort_by(|a, b| a.total_cmp(b));
        let n = us.len();
        let (p50, p95, max) = if n > 0 { (us[n / 2], us[(n * 95 / 100).min(n - 1)], us[n - 1]) } else { (0.0, 0.0, 0.0) };
        let ray_us_max = RAY_NS_MAX.swap(0, Ordering::Relaxed) as f64 / 1000.0;
        format!(
            "move={} move_us_p50={p50:.0} move_us_p95={p95:.0} move_us_max={max:.0} rays_avg={:.1} rays_max={} ray_us_max={ray_us_max:.1} move_writes={} mantle_checks={} mantle_rays_max={} mantle_budget_max={} mantle_deferred={} mantle_us_max={:.0} mantles={} mantle_refusals={} ladder_trips={}",
            p.driving as u8,
            if p.ticks > 0 { p.rays_sum as f64 / p.ticks as f64 } else { 0.0 },
            p.rays_max,
            p.writes,
            p.mantle_checks,
            p.mantle_rays_max,
            p.mantle_budget_max,
            p.mantle_deferred,
            p.mantle_us_max,
            p.mantles,
            p.mantle_refusals,
            self.ladder_log.episodes
        )
    }
}

/// A short name of a DIK code for the `MOVE keys` line.
fn key_name(k: u8) -> String {
    for (n, d) in PROBE_KEYS {
        if d == k {
            return n.to_string();
        }
    }
    match k {
        0x11 => "W".into(),
        0x1E => "A".into(),
        0x1F => "S".into(),
        0x20 => "D".into(),
        0x12 => "E".into(),
        _ => format!("0x{k:02x}"),
    }
}
