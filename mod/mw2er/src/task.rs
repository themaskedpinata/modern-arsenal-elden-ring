//! The recurring `CSTask` closure: all gameplay, once per frame on the game's main thread
//! (group `ChrIns_PostPhysics`: this frame's physics positions are final and `CameraStep` has run).
//!
//! Order per tick (docs/M2_DESIGN.md section 8): clock -> player -> PARAMS check on world entry
//! -> camera -> menu -> foreground -> F5 toggles -> blanking flags -> characters -> spread ->
//! weapon sim -> per-shot firing pipeline -> bullet diagnostics -> HP poll -> HUD snapshot.
//!
//! A panic anywhere in the tick is caught: blanking is cleared first (LMB/R go back to the game),
//! gameplay is switched off for the session and every later tick returns immediately.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use eldenring::cs::{CSTaskGroupIndex, CSTaskImp, FieldInsHandle, FieldInsType};
use eldenring::fd4::FD4TaskData;
use fromsoftware_shared::SharedTaskImpExt;
use mw2er_core::aim::{
    CamSignTracker, SignUpdate, bullet_origin, cam_dist_plausible, check_shot_fp, estimate_target_distance, view_forward,
};
use mw2er_core::cam_gate::{CamGate, CamSample, GateParams, Why, det3, ortho_err};
use mw2er_core::config::{CameraSource, Config, ShotLog};
use mw2er_core::falloff::Falloff;
use mw2er_core::hit_logic::{ChrSample, HitContext, HitEvent, HitTracker, ImpactSample, Via, prune_impacts};
use mw2er_core::input_logic::{CounterReader, MOUSE_POLL_MAX_AGE_MS, fresh};
use mw2er_core::logfmt::{self, HitAttr, ShotFields};
use mw2er_core::vm_anim::{VmAnim, VmEvent, VmInputs, VmParams, VmState};
use mw2er_core::menu_logic::{HudOwner, effective_allow, enable_refusal, hud_gone, indices_of, mask_of, menu_open_or_indices, transitions};
use mw2er_core::rng::Rng;
use mw2er_core::spread::{SpreadParams, SpreadState, StanceSpread, basis, perturb};
use mw2er_core::vec3::V3;
use mw2er_core::weapon_sim::{CueFractions, ReloadIgnored, SegPhase, SfxCue, SimEvent, SimParams, WeaponSim};
use mw2er_core::spawn_cause::{Breaker, Cause, PoolSnap, classify, pellets_allowed};
use mw2er_core::emitter_guard::Kind as SpawnKind;
use mw2er_core::killstreak::KillSrc;
use mw2er_core::spread::{ads_cone, pellet_dirs};
use mw2er_core::switch_logic::SwitchEvent;
use mw2er_core::projectile::{Guidance, Projectile, javelin_apex_m, self_blast_distance};
use mw2er_core::arsenal::LockKind;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::arsenal_task::{ArsenalCtl, LBlast};

use crate::ac130::{Ac130Ctl, Ac130In};
use crate::data::GameData;
use crate::game::camera::{CameraSample, read_camera, read_pers_cam1_17, sample_from17};
use crate::game::chars::{ChrRead, read_nearby};
use crate::game::menu::MenuSample;
use crate::game::player::{PlayerSample, read_player};
use crate::game::{bullets, havok, menu, params};
use crate::guard::guard;
use crate::log::{log_e, log_i, log_w, set_thread_role};
use crate::shared::{
    self, GAME_HWND, GAMEPLAY_DISABLED, HudSnapshot, INPUT_HOOK_OK, LAST_GDS_MOUSE_MS, MODE_ON, PRESSES_RELOAD,
    PRESSES_TOGGLE, RAW_ADS_DOWN, RAW_FIRE_DOWN, SNAPSHOT, TASK_HEARTBEAT_MS, WANT_BLANK_ADS, WANT_BLANK_FIRE,
    WANT_BLANK_RELOAD, now_ms_if_started, now_s,
};
use crate::first_person::{FpController, TickIn};
use crate::movement::{MoveController, MoveIn, MoveOut};
use crate::{cam_hook, spawn, spawn_guard, viewmodel};

/// Consecutive spawn failures that open the breaker. Run 6: ER refused 4 spawns (code 4) in ~4 s mid-fight with
/// good shots in between, and the old session-long breaker at 3 disabled firing for good.
const SPAWN_BREAKER: u32 = 8;
/// The breaker closes again after this long (firing retries; a persistent fault re-opens it).
const SPAWN_BREAKER_COOLDOWN_S: f64 = 3.0;
/// Bullet diagnostics run only this long after the last shot.
const BULLET_POLL_AFTER_SHOT_S: f64 = 2.0;
/// IMPACT counts as "expired" (end of life mid-air) within this distance of speed * life.
const EXPIRED_TOLERANCE_M: f32 = 2.0;
/// No shot during the first ticks after entering the world (the character and camera settle).
const FIRE_WARMUP_TICKS: u64 = 30;
/// Our impact markers are kept this long for hit attribution (M3).
const IMPACT_KEEP_S: f64 = 1.0;
const SHOT_HIST: usize = 32;
/// CAMFRAME diagnostics: the first ticks after world entry, then every few seconds and on validity changes.
const CAMFRAME_FIRST_TICKS: u64 = 10;
const CAMFRAME_EVERY_S: f64 = 5.0;
/// A player or camera move larger than this in one tick is a Havok re-base / teleport, not movement.
const REBASE_JUMP_M: f32 = 5.0;

/// The bullet owner must be a real character handle: not the empty handle, selector type Chr.
/// (The crate's `FieldInsType` values; that the player's selector reads as Chr is inferred and is
/// logged on "world entered".)
fn owner_check(h: FieldInsHandle) -> Result<(), String> {
    if h.is_empty() {
        return Err("owner handle is empty".into());
    }
    match h.selector.field_ins_type() {
        Some(FieldInsType::Chr) => Ok(()),
        t => Err(format!("owner handle type {t:?} is not Chr ({h:?})")),
    }
}

#[derive(Clone, Copy, Debug)]
struct ShotInfo {
    t: f64,
    band_id: i32,
    mw2: i32,
    d: f32,
    origin: V3,
    /// M6: the arsenal slot that fired (u16::MAX = the M2-M5 single gun).
    gun: u16,
}

struct TaskState {
    data: Option<GameData>,
    cfg: Arc<Config>,
    first_tick: bool,
    perf: Perf,
    last_t: Option<f64>,
    in_world: bool,
    world_ticks: u64,
    params_ok: Option<bool>,
    sim: Option<WeaponSim>,
    spread: SpreadState,
    spread_p: Option<SpreadParams>,
    falloff: Option<Falloff>,
    rng: Rng,
    cam_sign: CamSignTracker,
    last_fov_raw: f32,
    prev_menu_mask: u128,
    prev_show_cursor: Option<bool>,
    menu_baseline: Option<u128>,
    /// 2026-10-21: the AC-130's own HUD hide (excluded from `hud_gone`) and the last `hud_gone` value (its MENU line).
    hud_owner: HudOwner,
    prev_hud_gone: bool,
    toggle_reader: CounterReader,
    reload_reader: CounterReader,
    /// 2026-10-21: the heal presses (keyboard pulses / refusals, pad pulses) for the `HEAL` lines.
    heal_readers: [CounterReader; 3],
    hit: HitTracker,
    our_ids: HashSet<i32>,
    seen_markers: HashSet<u64>,
    first_band_bullet_logged: bool,
    first_spawn_done: bool,
    spawn_failures: u32,
    breaker_open: bool,
    breaker_until: f64,
    last_shot: Option<ShotInfo>,
    /// The last few shots, so a HIT resolved after `impact_lag_s` names the shot current at its drop.
    shot_hist: VecDeque<ShotInfo>,
    last_hit_t: f64,
    last_hit_kill: bool,
    shots: u64,
    hits: u64,
    prev_pos: Option<V3>,
    planar_speed: f32,
    weapon_warned: bool,
    events: Vec<SimEvent>,
    // M3
    cam_gate: CamGate,
    cam_valid: bool,
    cam_why: Why,
    /// C6 round 6: when the player last entered the world (NaN before the first enter: the stance's spawn window).
    world_enter_t: f64,
    last_camframe_t: f64,
    camsign_warned: bool,
    impacts: Vec<ImpactSample>,
    vm: Option<VmAnim>,
    vm_state: VmState,
    vm_events: Vec<VmEvent>,
    // M3b
    fp: FpController,
    fp_eye: Option<V3>,
    /// C6 part 18: the third-person over-the-shoulder camera (`[third_person]`).
    shoulder: crate::shoulder_cam::ShoulderRt,
    // M4
    ac130: Ac130Ctl,
    /// The post-panic AC-130 cleanup still runs (cleared if it panics itself).
    ac130_disabled_ok: bool,
    /// Bumped on every config change (the movement controller rebuilds its config-derived lines / masks then).
    cfg_gen: u64,
    // M5
    movement: MoveController,
    /// The post-panic deferred gravity restore still runs (cleared if it panics itself).
    move_disabled_ok: bool,
    mv: MoveOut,
    /// Reload presses held back until the sprint-out time is over (MW2: the reload starts after sprint_out).
    pending_reloads: u32,
    stance_spread: Option<StanceSpread>,
    // M6
    /// The arsenal (None = the M2-M5 single gun: `[arsenal] enabled = false` or its data failed).
    ars: Option<ArsenalCtl>,
    /// Spawn breaker counted per shot (a pellet shot fails only if all its pellets fail; bug 2).
    breaker: Breaker,
    /// Engine bullets spawned since the world was entered (the bug-2 pool counters).
    spawns_world: u32,
    pool_logged_at: u32,
    /// Bug 2: one retry of a failed spawn whose cause was the owner (band id, origin, dir).
    retry: Option<(i32, V3, V3)>,
    marker_probe: Option<bool>,
    scope: f32,
    lock_ui: (u8, Option<V3>, bool),
    /// The pellet directions of a shot (reused).
    pellets: Vec<V3>,
    // M7
    /// An AC-130 ride was Active last tick (its start cancels a reload / rechamber, A3).
    ac130_was_active: bool,
    /// The MW2 Tactical Nuke (killstreak counter, sequence, kills, player death).
    nuke: crate::nuke::NukeCtl,
    /// Co-op C2: the read-only peer watch (`COOP session peers=<n>` lines; the nuke's co-op rules).
    coop: crate::coop::CoopCtl,
    /// C3 PASS B: the gamepad's task half (wishes for the XInput detour, PAD lines, the deg/count calibration).
    pad: crate::pad::PadTask,
    /// The nuke's camera shake of this tick (added to the view-fx angles).
    nuke_shake: mw2er_core::ac130_shake::ShakeOut,
    // M8
    /// The gun lowered by a mantle row >= 33 in or a ladder (MW2 quick drop): no fire, ADS or reload until raised.
    wgate: mw2er_core::mantle::WeaponGate,
    /// 2026-10-24 (user: "can still shoot after death"): no MW2 fire of any kind from the death until the player is
    /// alive again after the respawn, first and third person (`SHOT dead gate on/off`, `SHOT refused why=dead`).
    death_gate: mw2er_core::fp_logic::DeathGate,
    /// The player is on a ladder (`CSChrLadderModule.state != -1`), MW2 mode on.
    on_ladder: bool,
    // C3
    /// The UI yield (docs/C3_DESIGN.md 2.3) and its last published frac / bits.
    ui_yield: mw2er_core::ui_yield::UiYield,
    ui_yield_out: (f32, u32),
    /// C5 `mode = "cutout"`: this tick's regions (strongest first) and how many; the last `UI cutout` line's signal
    /// set and time (rate limit).
    ui_cut: ([mw2er_core::ui_yield::CutRect; mw2er_core::ui_yield::MAX_CUT_RECTS], usize),
    ui_cut_log: (u32, f64),
    /// `[ui_yield] learn`: per-index open time, durations and the signal bits seen with it (60 s windows).
    ui_learn: UiLearn,
    /// Floating damage numbers (docs/C3_DESIGN.md 2.6).
    dmg: mw2er_core::dmgnum::DmgNums,
    /// The `HUD dmgnum cfg` line was logged for this config generation.
    dmg_cfg_gen: Option<u64>,
    /// The eye the HUD projects from this tick (FP eye or ER's camera).
    view_pos: [f32; 3],
    /// The Scaleform probe's 5 s lines (only while it is installed).
    ui_probe_log: crate::ui_probe::ProbeLog,
    /// C6 depth probe P-D (read-only; `[coop] probe_er_depth`).
    depth_probe: crate::er_depth::DepthProbe,
    /// C6 depth 5: the barrier probe's lines / verdict, the sample, the live gate (`[coop] probe_er_depth_state`).
    depth_state: crate::er_depth_state::StateLog,
    /// C4: the injection's going live / hot disable / trips / PSO worker / 5 s line (only while installed).
    ui_inject_log: crate::ui_inject::InjectLog,
    // C3 PASS C
    /// Co-op visibility: twins out, peer effects in, markers, world guns (docs/C3_DESIGN.md part 3).
    coopvis: crate::coop_vis::CoopVis,
    /// A panic in the co-op visibility tick turned it off for the session (nothing to restore: markers expire).
    coopvis_off: bool,
    /// C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance of the hidden ER weapon (`[coop] er_stance`).
    stance: crate::er_stance::ErStance,
    /// C6 part 16 (docs/C6_HOLD_DESIGN.md 16.3-16.5): the real crossbow carrier (`[coop] er_carrier`); ticked before the
    /// stance, off for the session after a panic (its journal stays on disk: the next world enter of a later session
    /// restores).
    carrier: crate::carrier::Carrier,
    carrier_off: bool,
    stance_off: bool,
    /// `HIT peer_drop` lines this session (docs/C3_DESIGN.md 3.7).
    peer_drops: u32,
}

/// Review C3: the `ui_states` indices the C2 logs show as short, frequent opens (toast candidates: 15/24/66 and the
/// unconfirmed 17/19/65); `UI yield on ... vis=` lists the visible ones.
const UI_WATCH_IDX: [u32; 6] = [15, 17, 19, 24, 65, 66];

/// C3 `[ui_yield] learn`: how long each `ui_states` index stays open, and with which signals.
struct UiLearn {
    t0: f64,
    open_since: [Option<f64>; 128],
    durs: Vec<(u8, f64, u32)>,
}

fn sim_params(d: &GameData, cfg: &Config) -> SimParams {
    let c = &cfg.audio.reload_cues;
    SimParams::from_spec(
        &d.weapon,
        cfg.gameplay.auto_reload_on_empty,
        cfg.gameplay.max_shots_per_tick,
        CueFractions { lift: c.lift, clipout: c.clipout, clipin: c.clipin, chamber: c.chamber },
    )
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    t ^ ((std::process::id() as u64) << 32) ^ (now_s().to_bits())
}

impl TaskState {
    fn new(data: Option<GameData>) -> Self {
        let cfg = shared::config();
        // M6: the arsenal in place of the single gun when its data loaded (arsenal_rt::load_logged at boot)
        let ars = crate::arsenal_rt::ARSENAL.get().cloned().map(|d| ArsenalCtl::new(d, &cfg));
        let mut sim = data.as_ref().map(|d| WeaponSim::new(sim_params(d, &cfg)));
        let mut spread_p = data.as_ref().map(|d| SpreadParams {
            min_deg: d.weapon.spread_min_deg,
            max_deg: d.weapon.spread_max_deg,
            fire_add: d.weapon.spread_fire_add,
            decay_per_s: d.weapon.spread_decay_per_s,
            move_add: d.weapon.spread_move_add,
        });
        let mut falloff = data.as_ref().map(|d| Falloff {
            damage: d.weapon.damage,
            min_damage: d.weapon.min_damage,
            max_damage_range_m: d.weapon.max_damage_range_m,
            min_damage_range_m: d.weapon.min_damage_range_m,
        });
        let mut our_ids: HashSet<i32> = data.as_ref().map(|d| d.rows.all_ids().into_iter().collect()).unwrap_or_default();
        if let Some(a) = ars.as_ref() {
            sim = Some(a.new_sim(a.cur, &cfg));
            spread_p = Some(a.spread_params(a.cur));
            falloff = Some(a.spec().falloff);
            our_ids.extend(a.data.rows.ids());
            our_ids.insert(a.data.marker_id);
            a.publish_move_class(a.cur);
        }
        let fp = FpController::new(&cfg.first_person);
        TaskState {
            data,
            cfg,
            first_tick: true,
            perf: Perf::default(),
            last_t: None,
            in_world: false,
            world_ticks: 0,
            params_ok: None,
            sim,
            spread: SpreadState::default(),
            spread_p,
            falloff,
            rng: Rng::new(seed()),
            cam_sign: CamSignTracker::default(),
            last_fov_raw: 0.0,
            prev_menu_mask: 0,
            prev_show_cursor: None,
            menu_baseline: None,
            hud_owner: HudOwner::default(),
            prev_hud_gone: false,
            toggle_reader: CounterReader::starting_at(PRESSES_TOGGLE.load(Ordering::SeqCst)),
            reload_reader: CounterReader::starting_at(PRESSES_RELOAD.load(Ordering::SeqCst)),
            heal_readers: [
                CounterReader::starting_at(shared::HEAL_KEY_PULSES.load(Ordering::SeqCst)),
                CounterReader::starting_at(shared::HEAL_KEY_REFUSED.load(Ordering::SeqCst)),
                CounterReader::starting_at(crate::pad::heal_pulses()),
            ],
            hit: HitTracker::default(),
            our_ids,
            seen_markers: HashSet::new(),
            first_band_bullet_logged: false,
            first_spawn_done: false,
            spawn_failures: 0,
            breaker_open: false,
            breaker_until: 0.0,
            last_shot: None,
            shot_hist: VecDeque::with_capacity(SHOT_HIST),
            last_hit_t: f64::NEG_INFINITY,
            last_hit_kill: false,
            shots: 0,
            hits: 0,
            prev_pos: None,
            planar_speed: 0.0,
            weapon_warned: false,
            events: Vec::with_capacity(16),
            cam_gate: CamGate::default(),
            cam_valid: false,
            cam_why: Why::Settling,
            world_enter_t: f64::NAN,
            last_camframe_t: f64::NEG_INFINITY,
            camsign_warned: false,
            impacts: Vec::with_capacity(64),
            vm: None,
            vm_state: VmState::hidden(),
            vm_events: Vec::with_capacity(16),
            fp,
            fp_eye: None,
            shoulder: crate::shoulder_cam::ShoulderRt::default(),
            ac130: Ac130Ctl::new(seed()),
            ac130_disabled_ok: true,
            cfg_gen: 0,
            movement: MoveController::new(),
            move_disabled_ok: true,
            mv: MoveOut::idle(),
            pending_reloads: 0,
            stance_spread: None,
            ars,
            breaker: Breaker::default(),
            spawns_world: 0,
            pool_logged_at: 0,
            retry: None,
            marker_probe: None,
            scope: 0.0,
            lock_ui: (0, None, false),
            pellets: Vec::with_capacity(16),
            ac130_was_active: false,
            nuke: crate::nuke::NukeCtl::new(seed()),
            coop: crate::coop::CoopCtl::default(),
            pad: crate::pad::PadTask::new(),
            nuke_shake: mw2er_core::ac130_shake::ShakeOut::ZERO,
            wgate: mw2er_core::mantle::WeaponGate::default(),
            death_gate: mw2er_core::fp_logic::DeathGate::default(),
            on_ladder: false,
            ui_yield: mw2er_core::ui_yield::UiYield::default(),
            ui_yield_out: (0.0, 0),
            ui_cut: ([mw2er_core::ui_yield::CutRect::default(); mw2er_core::ui_yield::MAX_CUT_RECTS], 0),
            ui_cut_log: (0, f64::NEG_INFINITY),
            ui_learn: UiLearn { t0: 0.0, open_since: [None; 128], durs: Vec::new() },
            dmg: mw2er_core::dmgnum::DmgNums::default(),
            dmg_cfg_gen: None,
            view_pos: [0.0; 3],
            ui_probe_log: crate::ui_probe::ProbeLog::new(),
            depth_probe: crate::er_depth::DepthProbe::default(),
            depth_state: crate::er_depth_state::StateLog::default(),
            ui_inject_log: crate::ui_inject::InjectLog::new(),
            coopvis: crate::coop_vis::CoopVis::new(seed()),
            coopvis_off: false,
            stance: crate::er_stance::ErStance::new(),
            carrier: crate::carrier::Carrier::default(),
            carrier_off: false,
            stance_off: false,
            peer_drops: 0,
        }
    }

    /// M7 A3: cancels a reload / segmented reload / rechamber of the gun in hand and logs each cancel with its reason
    /// (`RELOAD cancelled (<reason>)`, `RECHAMBER cancelled gun= reason=`). `vm`: the viewmodel leaves its reload clip.
    /// 2026-10-21: the `HEAL` lines (the detours pressed ER's use-item, or refused a key press while MW2 mode was on).
    fn log_heal(&mut self, er_use_item_key: &str, menu_open: bool, in_world: bool, foreground: bool) {
        let fp = self.fp_eye.is_some() as u8;
        let n = self.heal_readers[0].take(shared::HEAL_KEY_PULSES.load(Ordering::SeqCst));
        if n > 0 {
            log_i!("HEAL", "press src=key n={n} synth={er_use_item_key} fp={fp}");
        }
        let n = self.heal_readers[1].take(shared::HEAL_KEY_REFUSED.load(Ordering::SeqCst));
        if n > 0 {
            let why = if menu_open {
                "menu"
            } else if !in_world {
                "not_in_world"
            } else if !foreground {
                "unfocused"
            } else {
                "not_driving"
            };
            log_i!("HEAL", "refused src=key n={n} why={why}");
        }
        let n = self.heal_readers[2].take(crate::pad::heal_pulses());
        if n > 0 {
            log_i!("HEAL", "press src=pad n={n} synth={} fp={fp}", crate::pad::er_use_item_name());
        }
    }

    fn cancel_sim_vm(&mut self, reason: &str, vm: bool) {
        let mut ev = Vec::new();
        if let Some(sim) = self.sim.as_mut() {
            sim.cancel_reload(&mut ev);
        }
        self.log_cancel(&ev, reason, vm);
    }

    fn cancel_sim(&mut self, reason: &str) {
        self.cancel_sim_vm(reason, false);
    }

    /// M8: the gun in hand's quickDropTime / quickRaiseTime (movement.json mantle block; MW2 0.25 / 0.55 s).
    fn quick_times(&self) -> (f32, f32) {
        let gun = self.gun_name();
        crate::movement::move_data().and_then(|d| d.mantle.as_ref().map(|m| m.quick_times(&gun))).unwrap_or((0.25, 0.55))
    }

    /// M8 review: MW2's quick clips of the gun in hand (its weapon file, V) and whether the viewmodel plays that
    /// very xanim (`exact`) or the gun's putaway / pullout at the quick time (`putaway` / `pullout`, I: the dedicated
    /// `*_quick` xanims of 14 guns are not converted).
    fn quick_clip_text(&self) -> String {
        let gun = self.gun_name();
        let Some(q) = crate::movement::move_data().and_then(|d| d.mantle.as_ref().and_then(|m| m.quick_clips(&gun).cloned())) else {
            return " mw2_clips=?".into();
        };
        format!(
            " mw2_drop={} vm_drop={} mw2_raise={} vm_raise={}",
            q.drop_anim,
            if q.drop_exact { "exact" } else { "putaway" },
            q.raise_anim,
            if q.raise_exact { "exact" } else { "pullout" }
        )
    }

    fn log_cancel(&mut self, ev: &[SimEvent], reason: &str, vm: bool) {
        if ev.contains(&SimEvent::ReloadCancelled) {
            if vm {
                self.vm_events.push(VmEvent::ReloadCancelled);
            }
            if reason == "mantle" || reason == "ladder" {
                // M8: the gun quick-drops (MW2 switches to no weapon)
                log_i!("RELOAD", "cancelled reason={reason} gun={}", self.gun_name());
            } else {
                log_i!("RELOAD", "cancelled ({})", if reason == "sprint" { "sprint start" } else if reason == "world_left" { "world left" } else if reason == "mode_off" { "mode off" } else { reason });
            }
        }
        if ev.contains(&SimEvent::RechamberCancelled) {
            log_i!("RECHAMBER", "cancelled gun={} reason={reason}", self.gun_name());
        }
    }

    /// The aim's chest reference: feet + chest_height, or (M5, while the controller drives) a constant distance below
    /// the stance eye so crouched / prone shots keep the standing geometry (docs/M5_DESIGN.md section 8.5).
    fn chest(&self, p: &PlayerSample) -> V3 {
        match self.mv.chest {
            Some(c) if self.mv.driving => c,
            _ => p.chr.pos + V3::new(0.0, self.cfg.aim.chest_height_m, 0.0),
        }
    }

    /// The stance values of the spread (from the converted movement data, M5).
    fn stance_spread(&mut self) -> Option<StanceSpread> {
        if let Some(a) = self.ars.as_ref() {
            if self.stance_spread.is_none() {
                self.stance_spread = a.stance_spread(a.cur);
            }
            return self.stance_spread;
        }
        if self.stance_spread.is_none()
            && let Some(d) = crate::movement::move_data()
        {
            let w = &d.weapon;
            self.stance_spread = Some(StanceSpread {
                ducked: w.spread_ducked,
                prone: w.spread_prone,
                ducked_decay: w.spread_ducked_decay,
                prone_decay: w.spread_prone_decay,
                move_threshold_mps: d.mw2.aim_spread_move_threshold * 0.0254,
                jump_add: d.mw2.jump_spread_add / 255.0,
            });
        }
        self.stance_spread
    }

    /// The hip spread cone now: the stance values while the controller drives (M5), else the M2 standing cone.
    fn spread_deg(&self, sp: &SpreadParams) -> f32 {
        match (self.mv.driving, self.stance_spread) {
            (true, Some(ss)) => self.spread.deg_stance(sp, &ss, self.mv.spread_stance),
            _ => self.spread.deg(sp),
        }
    }

    fn refresh_config(&mut self) {
        let cfg = shared::config();
        if Arc::ptr_eq(&cfg, &self.cfg) {
            return;
        }
        self.cfg = cfg;
        self.cfg_gen += 1;
        if let Some(a) = self.ars.as_ref() {
            let cur = a.cur;
            if let (Some(vm), Some(p)) = (self.vm.as_mut(), a.vm_params(cur, &self.cfg)) {
                vm.set_params(p);
            }
            let p = a.sim_params(cur, &self.cfg);
            if let Some(sim) = self.sim.as_mut() {
                sim.set_params(p);
            }
            return;
        }
        if let (Some(vm), Some(a)) = (self.vm.as_mut(), viewmodel::assets()) {
            vm.set_params(VmParams::from_assets(&a, self.cfg.viewmodel.crossfade_s, self.cfg.viewmodel.sprint_speed_mps));
        }
        if let (Some(d), Some(sim)) = (self.data.as_ref(), self.sim.as_mut()) {
            sim.set_params(sim_params(d, &self.cfg));
            if self.cfg.gameplay.weapon != d.weapon.name && !self.weapon_warned {
                self.weapon_warned = true;
                log_w!("CONFIG", "WARN gameplay.weapon={} ignored: M2 loads one weapon at boot ({}); restart to switch", self.cfg.gameplay.weapon, d.weapon.name);
            }
        }
    }

    fn tick(&mut self) {
        // Heartbeat first: the input detour honours WANT_BLANK_* only while this is fresh.
        TASK_HEARTBEAT_MS.store(now_ms_if_started(), Ordering::Relaxed);
        // (cleared here: the M5 movement step may queue a sprint-start reload cancel before the weapon sim runs)
        self.vm_events.clear();
        let now = now_s();
        let dt = self.last_t.map_or(0.0, |t| (now - t).clamp(0.0, 0.25));
        self.last_t = Some(now);
        self.refresh_config();
        let cfg = self.cfg.clone();

        // Player: None = not in world (title, loading, modules not populated).
        let player = read_player();
        let in_world = player.is_some();
        if in_world != self.in_world {
            self.in_world = in_world;
            self.cam_gate.reset();
            self.cam_valid = false;
            self.camsign_warned = false;
            self.impacts.clear();
            if in_world {
                self.world_ticks = 0;
                // C6 round 6: the spawn window of the stance's chords starts here
                self.world_enter_t = now;
                self.params_ok = None;
                self.ac130.reset_rows();
                // M6 bug 2: the pool counters restart with the world (a world reload empties ER's pools)
                self.spawns_world = 0;
                self.pool_logged_at = u32::MAX;
                self.marker_probe = None;
                self.retry = None;
                spawn_guard::world_entered();
                log_i!(
                    "TASK",
                    "world entered (player handle {} owner_check={})",
                    logfmt::handle(player.map_or(0, |p| p.chr.handle)),
                    player.map_or("n/a".to_string(), |p| owner_check(p.field_ins_handle).err().unwrap_or_else(|| "ok".into()))
                );
            } else {
                log_i!("TASK", "world left (loading / title)");
                // C3: numbers belong to the old world
                self.dmg.clear();
                // C6: every patched EquipParamWeapon row back to its original (no chords: the grip is the player's again)
                if guard("stance.world_leave", || self.stance.restore_all("world_leave")).is_none() {
                    self.stance_off = true;
                }
                // C3 PASS C: the peers' effects and our marker state belong to the old world
                self.coopvis.reset();
                // loading frees and reuses heap pages: every write page is checked again in the next world
                crate::game::flush_write_cache();
                self.cancel_sim("world_left");
                self.hit.clear();
                self.cam_sign.reset();
                self.prev_pos = None;
                if let Some(a) = self.ars.as_mut() {
                    a.sw.reset();
                    if let Some(vm) = self.vm.as_mut() {
                        vm.cancel_switch();
                    }
                    a.projs.clear();
                    a.reset_lock();
                }
            }
        }
        if in_world {
            self.world_ticks += 1;
            if self.params_ok.is_none() {
                self.check_params();
            }
        }

        // Planar speed from the position delta (jumps > 5 m are map re-basing, not movement).
        self.planar_speed = 0.0;
        if let Some(p) = &player {
            if let Some(prev) = self.prev_pos {
                let d = (p.chr.pos - prev).planar_len();
                if d <= REBASE_JUMP_M && dt > 0.0 {
                    self.planar_speed = d / dt as f32;
                } else if d > REBASE_JUMP_M {
                    // Havok re-base / teleport: stale impacts and the camera history are meaningless now.
                    self.impacts.clear();
                    self.cam_gate.reset();
                    // C3: the damage numbers move with the world (a teleport only shifts a few live numbers)
                    let dv = p.chr.pos - prev;
                    self.dmg.rebase([dv.x, dv.y, dv.z]);
                    // review M6: launcher projectiles live in the old frame (a Javelin / M79 up to 30 s)
                    if let Some(a) = self.ars.as_mut()
                        && !a.projs.is_empty()
                    {
                        a.expire_projectiles("rebase");
                    }
                }
            }
            self.prev_pos = Some(p.chr.pos);
        }

        // Camera.
        let mut cam = read_camera(cfg.gameplay.camera_source);
        // M4 camera mode 2: pers_cam_1 still holds the gunship frame the detour wrote last frame; the gate, CAMFRAME,
        // the UMP45 aim and the AC-130's own theta0 must see ER's camera (docs/M4_DESIGN.md section 4.5).
        if cfg.gameplay.camera_source == CameraSource::PersCam1
            && let Some(c) = cam.as_mut()
            && let Some((_, m17)) = read_pers_cam1_17()
            && let Some(er) = cam_hook::er_view_for_task(&m17)
            && let Some(s) = sample_from17(&er, c)
        {
            *c = s;
        }
        // M3b hook mode: the camera-step detour already moved pers_cam_1 to the eye this frame; the gate, CAMFRAME
        // and aim must keep seeing ER's own camera (docs/M3B_DESIGN.md section 2.3).
        if let Some(c) = cam.as_mut()
            && let Some(er) = cam_hook::er_view_pos(c.pos)
        {
            c.pos = er;
        }
        // M3b task mode: if ER skipped a camera step, pers_cam_1 still holds last tick's eye; the gate gets ER's last
        // own position for this tick instead (review M3b minor 6).
        if let Some(c) = cam.as_mut()
            && let Some(er) = self.fp.er_view_override(c.pos)
        {
            c.pos = er;
        }
        // M6: while scoped, pers_cam_1's fov holds our zoomed write (a sniper reaches ~0.18 rad, below the gate's
        // 0.2 rad floor); the gate sees ER's own fov, the HUD keeps the zoomed one it is drawn with.
        let shown_fov = cam.map(|c| c.fov_raw);
        if let Some(c) = cam.as_mut()
            && let Some(er) = cam_hook::viewfx_er_fov(c.fov_raw)
        {
            c.fov_raw = er;
        }
        if in_world && self.world_ticks <= 3 {
            self.log_camera_probe(player.as_ref());
        }
        self.update_cam_gate(now, cam, player.as_ref());
        if let (Some(c), Some(p)) = (cam, player.as_ref()) {
            self.last_fov_raw = shown_fov.unwrap_or(c.fov_raw);
            let chest = self.chest(p);
            match self.cam_sign.update(c.row2, c.pos, chest) {
                SignUpdate::Resolved(s) | SignUpdate::Flipped(s) => {
                    let dist = (chest - c.pos).len();
                    log_i!(
                        "CAMERA",
                        "sign={:+} cam_player_dist={:.2}m fov_raw={} source={}",
                        s as i32,
                        dist,
                        c.fov_raw,
                        cfg.gameplay.camera_source.label()
                    );
                    if self.cam_valid && s < 0.0 && !self.camsign_warned {
                        self.camsign_warned = true;
                        log_w!("CAMERA", "WARN camsign: the M2 sign tracker resolved -1 while the camera gate is valid; aiming uses +1 (M3)");
                    }
                    if !cam_dist_plausible(dist) {
                        log_w!(
                            "CAMERA",
                            "odd cam_player_dist={dist:.2}m (camera and player may be in different spaces); shots are skipped while it lasts"
                        );
                    }
                }
                _ => {}
            }
        }

        // Menus: always log visibility transitions (builds the ui_states index map).
        let m = menu::read_menu().unwrap_or_default();
        if m.mask != self.prev_menu_mask {
            for (idx, vis) in transitions(self.prev_menu_mask, m.mask) {
                log_i!("MENU", "idx={idx} {}", if vis { "0->1" } else { "1->0" });
            }
            self.prev_menu_mask = m.mask;
        }
        // The cursor is now the default menu signal: log its transitions so a run proves it.
        if m.show_cursor != self.prev_show_cursor {
            log_i!("MENU", "cursor {:?} -> {:?}", self.prev_show_cursor, m.show_cursor);
            self.prev_show_cursor = m.show_cursor;
        }
        let mode_on = MODE_ON.load(Ordering::SeqCst);
        // The baseline is captured when MW2 mode is enabled (docs/M2_DESIGN.md section 19). While the
        // mode is off and no menu.hud_indices are configured, the current visible set counts as HUD, so
        // only the optional cursor signal can block F5 (a stale earlier baseline must not).
        let baseline = if mode_on { self.menu_baseline } else { Some(m.mask) };
        let allow = effective_allow(&cfg.menu.hud_indices, baseline);
        // 2026-10-21: a site of grace on a pad shows no cursor and no idx 28, but hides ER's HUD (idx 7): menu open, so the
        // pad / 4 / F7 MW2 actions yield and the D-pad drives ER's menu (not while the AC-130 hid the HUD itself).
        let hud_ours = self.hud_owner.step(now, self.ac130.hides_er_hud());
        let hud_off = hud_gone(m.mask, mask_of(&cfg.menu.hud_off_indices), baseline, hud_ours);
        if hud_off != self.prev_hud_gone {
            self.prev_hud_gone = hud_off;
            log_i!(
                "MENU",
                "hud_off {} hidden={:?} ui_states={:?} cursor={:?}",
                if hud_off { "0->1 (ER's HUD hidden: a full-screen menu, e.g. a site of grace)" } else { "1->0" },
                indices_of(mask_of(&cfg.menu.hud_off_indices) & !m.mask),
                indices_of(m.mask),
                m.show_cursor
            );
        }
        let menu_is_open = hud_off
            || menu_open_or_indices(m.mask, mask_of(&cfg.menu.menu_indices), allow, cfg.menu.use_ui_states, cfg.menu.use_show_cursor, m.show_cursor.unwrap_or(false));

        // Foreground: blanking only applies while the game window is focused.
        let hwnd = GAME_HWND.load(Ordering::Relaxed);
        // SAFETY: no preconditions.
        let foreground = hwnd == 0 || unsafe { GetForegroundWindow() }.0 as usize == hwnd;

        // F5 presses since the last tick.
        let toggles = self.toggle_reader.take(PRESSES_TOGGLE.load(Ordering::SeqCst));
        for _ in 0..toggles.min(4) {
            self.toggle(player.as_ref(), menu_is_open, foreground, m);
        }
        let mode_on = MODE_ON.load(Ordering::SeqCst);

        // Blanking: LMB and R are hidden from the game exactly while we use them.
        let blank = mode_on && in_world && !menu_is_open && foreground && !GAMEPLAY_DISABLED.load(Ordering::SeqCst);
        WANT_BLANK_FIRE.store(blank, Ordering::SeqCst);
        WANT_BLANK_RELOAD.store(blank, Ordering::SeqCst);
        let ads_enabled = cfg.keys.ads_mouse_button != 255;
        WANT_BLANK_ADS.store(blank && ads_enabled, Ordering::SeqCst);
        let reloads = self.reload_reader.take(PRESSES_RELOAD.load(Ordering::SeqCst));
        let reloads = if blank { reloads } else { 0 };

        let chars = if in_world { read_nearby(cfg.hit.max_chars) } else { Vec::new() };
        let mouse_polled = fresh(now_ms_if_started(), LAST_GDS_MOUSE_MS.load(Ordering::Relaxed), MOUSE_POLL_MAX_AGE_MS);

        // The first-person signal reads, ONCE per tick for both controllers (review M4 minor 6; timed as fp_us).
        let fp_read_t0 = std::time::Instant::now();
        let fp_player = if in_world { crate::game::fp_read::read_player_fp() } else { Ok(None) };
        let fp_world = if in_world { Some(crate::game::fp_read::read_world_fp()) } else { None };
        let fp_read_us = fp_read_t0.elapsed().as_secs_f64() * 1e6;
        // 2026-10-24 (user: "can still shoot after death"; logs/coop/host_c6_2026-10-24.log 13:27:52-13:30:44: killed in
        // third person, 40 SHOT lines while hp <= 0: 25 skipped, 15 real bullets at 13:30:40-41): the death gate refuses every MW2
        // fire path (hitscan, launchers, ADS, reload, the AC-130 call, the nuke key, the melee / heal pulses) from the
        // death until a player read was alive for DEATH_REARM_S after the respawn, first and third person alike (first
        // person's own `FP blocked reason=death` only hands the camera back). A tick without a read decides nothing.
        let death_cam = fp_world.and_then(|w| w.camera_type);
        let death_read = fp_player.ok().flatten();
        match self.death_gate.update(now, death_read.map(|p| mw2er_core::fp_logic::dead(p.hp, p.flags1c5, death_cam))) {
            Some(mw2er_core::fp_logic::DeathEv::On) => {
                let (hp, flags) = death_read.map_or((0, 0), |p| (p.hp, p.flags1c5));
                log_i!(
                    "SHOT",
                    "dead gate on hp={hp} death_flag={} camtype={death_cam:?} fp={} mode={} (no MW2 fire / ADS / reload / AC-130 / nuke / melee / heal until alive after the respawn)",
                    (flags & mw2er_core::fp_logic::DEATH_FLAG != 0) as u8,
                    self.fp_eye.is_some() as u8,
                    mode_on as u8
                );
            }
            Some(mw2er_core::fp_logic::DeathEv::Off { dead_s, refused }) => log_i!(
                "SHOT",
                "dead gate off dead_s={dead_s:.1} refused={refused} (alive {:.1}s after the respawn)",
                mw2er_core::fp_logic::DEATH_REARM_S
            ),
            None => {}
        }
        let dead_block = self.death_gate.blocked();
        shared::WANT_DEAD_BLOCK.store(dead_block, Ordering::SeqCst);
        // M8 (ladder audit, B1/B2): on a ladder ER drives in third person; the MW2 gun is lowered (MW2 quick-drops on
        // ladders too), the AC-130 and the nuke refuse. LMB stays hidden from ER unless ladder_er_attacks.
        let on_ladder = mode_on && fp_player.ok().flatten().is_some_and(|p| mw2er_core::fp_logic::on_ladder(p.ladder));
        self.on_ladder = on_ladder;
        if on_ladder && cfg.movement.ladder_er_attacks {
            WANT_BLANK_FIRE.store(false, Ordering::SeqCst);
        }

        // M4 AC-130: before first person, so FP hands back (reason ac130) in the same tick the ride starts.
        let ac = self.ac130.tick(&Ac130In {
            now,
            cfg: &cfg,
            mode_on,
            in_world,
            world_ticks: self.world_ticks,
            menu_open: menu_is_open,
            foreground,
            cam_valid: self.cam_valid,
            cam,
            player: player.as_ref(),
            chars: &chars,
            fp_latched: self.fp.latched(),
            mouse_polled,
            fp_on: self.fp_eye.is_some(),
            fp_player,
            fp_world,
            nuke_busy: self.nuke.busy(),
            player_dead: dead_block,
        });
        self.perf.ac130 |= ac.active;
        // M7: the Tactical Nuke (after the AC-130: a ride blocks the key; before movement / first person: its player-death
        // prep hands both back in the same tick)
        // co-op C2: the peer watch (read-only, 4 Hz); a panic in it only skips this tick's read
        let _ = guard("coop", || self.coop.tick(now, in_world));
        let coop_peers = self.coop.peers();
        // C3 PASS C (docs/C3_DESIGN.md part 3): co-op visibility (markers, the peers' twins, world guns); hands the nuke a
        // peer's marker. Cosmetic only: a panic turns it off for the session, nothing else.
        let mut nuke_from = None;
        if !self.coopvis_off {
            let slots = self.coop.peer_slots();
            let gun_g = self.ars.as_ref().map(|a| a.spec().g);
            let view = self.fp_eye.or(cam.map(|c| c.pos)).unwrap_or(V3::ZERO);
            let nuke_marker = self.nuke.marker_wanted(now, &cfg);
            let ar = crate::arsenal_rt::ARSENAL.get().cloned();
            let fp_on = self.fp_eye.is_some();
            let r = guard("coopvis", || {
                self.coopvis.tick(
                    &crate::coop_vis::CoopVisIn {
                        now,
                        cfg: &cfg,
                        in_world,
                        mode_on,
                        peers: &slots,
                        player: player.as_ref(),
                        gun_g,
                        ac130_active: ac.active,
                        nuke_marker,
                        fp_on,
                        fp_latched: self.fp.latched().0 || self.fp.latched().1,
                        cam_fwd: cam.map(|c| c.row2),
                        view_pos: view,
                        mounted: fp_player.ok().flatten().is_some_and(|p| p.is_mounted),
                        ads: self.vm_state.ads_at(now),
                        scope_peek: self.fp.peeking(),
                        stance_posed: self.stance.posed(),
                        stance_arm: self.stance.arm(),
                        stance_aim_on: self.stance.aim_on(),
                        stance_aim_active: self.stance.aim_active(),
                        // C6 part 17: the guard episode (last tick): held, detected (align), not detected in time (raise)
                        stance_guard_active: self.stance.guard_active(),
                        stance_guard_align: self.stance.guard_aligned(),
                        stance_guard_fallback: self.stance.guard_fallback(),
                        // C6 part 17 addendum: held standing (the guard frame learns only then)
                        stance_guard_standing: self.stance.guard_standing(),
                        // C6 part 16 review: the carrier's verified crossbow (last tick; ticked after this)
                        stance_carrier: !self.carrier_off && self.carrier.on(),
                    },
                    ar.as_ref(),
                )
            });
            match r {
                Some(o) => nuke_from = o.nuke_from,
                None => {
                    self.coopvis_off = true;
                    // review C3: our C3 markers are removed before the reset (MW2 mode's 8060 would keep ER's weapon
                    // invisible; a nuke marker could re-trigger a peer); guarded on its own
                    if guard("coopvis_strip", || self.coopvis.strip_markers()).is_none() {
                        self.coopvis.reset();
                    }
                    // C6: the strip path restores the stance's rows too (the peer states are gone with the reset)
                    let _ = guard("stance.strip", || self.stance.restore_all("coopvis_panic"));
                    log_e!("COOP", "FAIL panic in the co-op visibility tick; twins, peer effects, markers and world guns are off for this session");
                }
            }
            // the AC-130 shells of this tick get their twins (the explosion rows stay unsynced)
            if let Some(p) = player.as_ref() {
                for (src, o, d) in self.ac130.take_twins() {
                    self.coopvis.send_twin(now, &cfg, coop_peers, p, src, o, d);
                }
            }
        }
        let coop = &self.coop;
        let role = || coop.role();
        let nk = self.nuke.tick(&crate::nuke::NukeIn {
            now,
            cfg: &cfg,
            mode_on,
            in_world,
            loading: !in_world || self.fp.latched().0,
            world_ticks: self.world_ticks,
            player: player.as_ref(),
            ac130_busy: ac.fp_block,
            ladder: on_ladder,
            cam_fwd: cam.map(|c| c.row2),
            blank: blank && !ac.active,
            coop_peers,
            coop_role: &role,
            remote_nuke: nuke_from,
            player_dead: dead_block,
        });
        self.nuke_shake = nk.shake;
        if nk.cancel_reload {
            self.cancel_sim_vm("nuke", true);
        }
        let reloads = if nk.fire_block { 0 } else { reloads };
        let reloads = if ac.ump_block { 0 } else { reloads };
        let reloads = if dead_block { 0 } else { reloads };
        // M6: the weapon keys and the wheel belong to the arsenal while MW2 mode drives (section 5.1), never during an
        // AC-130 ride: its gun keys 1/2 are the weapon keys (review M6; the input hook also checks the gunship flag)
        shared::WANT_BLANK_WEAPON.store(blank && self.ars.is_some() && !ac.active, Ordering::SeqCst);
        shared::WANT_WHEEL.store(blank && self.ars.is_some() && cfg.arsenal.wheel && !ac.active, Ordering::SeqCst);

        // M5 movement: after the AC-130 (it suspends the controller in the same tick a ride starts), before first person
        // (which takes the controller's feet and stance eye) and before any shot (sprint gates fire, ADS and reload).
        // C3 PASS B (docs/C3_DESIGN.md 4.2): the pad's wishes for the XInput detour (MW2 mode drives, no AC-130 ride),
        // its PAD lines and the calibration of ER's degrees per mouse count; RT / LT are the pad's fire / ADS
        let _ = guard("pad", || {
            self.pad.tick(&crate::pad::PadIn {
                now,
                cfg: &cfg,
                cfg_gen: self.cfg_gen,
                want: blank && !ac.active,
                ads_frac: self.vm_state.ads_at(now),
                in_world,
                cam_fwd: cam.map(|c| c.row2),
                gunship: ac.active,
                er_prompt: m.mask & mask_of(&cfg.gamepad.prompt_indices) != 0,
            })
        });
        // 2026-10-21 (user: heal pots in MW2 mode): Q / D-pad right press ER's own use-item key / button once (the
        // selected quick item, the flask) while MW2 mode drives, first or third person, never in the AC-130 or a menu;
        // the detours do it, the task gives the wish (the pad's is `want` above) and logs
        shared::WANT_HEAL.store(blank && !ac.active, Ordering::SeqCst);
        self.log_heal(&cfg.keys.er_use_item, menu_is_open, in_world, foreground);
        let fire_in = (mouse_polled && RAW_FIRE_DOWN.load(Ordering::SeqCst)) || crate::pad::fire_down();
        let ads_in = (ads_enabled && mouse_polled && RAW_ADS_DOWN.load(Ordering::SeqCst)) || crate::pad::ads_down();
        if let Some(n) = self.death_gate.press(blank && fire_in)
            && (n == 1 || n % 10 == 0)
        {
            log_i!("SHOT", "refused why=dead n={n} fp={} (until alive after the respawn)", self.fp_eye.is_some() as u8);
        }
        let fire_raw = blank && fire_in && !ac.ump_block && !nk.fire_block && !dead_block;
        let ads_raw = blank && ads_in && !ac.ump_block && !nk.fire_block && !dead_block;
        // C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance (inert with `[coop] er_stance = "off"`); a panic
        // restores every patched row and turns it off for the session
        // C6 part 16: the inputs the carrier and the stance share this tick
        let st_fp = self.fp_eye.is_some() && !self.fp.peeking();
        let st_dead = fp_player.ok().flatten().is_some_and(|p| mw2er_core::fp_logic::dead(p.hp, p.flags1c5, fp_world.and_then(|w| w.camera_type)));
        let st_settling = mw2er_core::er_stance::spawn_settling(
            now - self.world_enter_t,
            fp_world.and_then(|w| mw2er_core::fp_logic::fade_max(&w.fade)),
            self.cam_why == Why::Dist,
        );
        let st_latched = self.fp.latched().0 || self.fp.latched().1;
        // C6 part 16 (docs 16.3): the real crossbow carrier, before the stance (it reads the carrier's view). Inert with
        // `er_carrier = "off"` except one journal-file check per world enter (a crash's swap is restored whatever the
        // flags). A panic turns it off for the session; the journal stays for the next session.
        let carrier_view = if self.carrier_off {
            // after a panic nothing is known: treated as equipped (no chord RB) until the session ends
            crate::carrier::CarrierView { equipped: true, ..Default::default() }
        } else {
            let (busy, orig_ranged, stance_dis) = self.stance.carrier_feed();
            let ci = crate::carrier::CarrierIn {
                now,
                mode: cfg.coop.carrier_mode(),
                give: cfg.coop.carrier_give,
                mode_on,
                in_world,
                fp: st_fp,
                ac130: ac.active || ac.fp_block,
                nuke: nk.fp_block,
                dead: st_dead,
                settling: st_settling,
                menu: menu_is_open || !foreground || st_latched,
                busy,
                player: crate::game::ac130_read::main_player(),
                stance_off: stance_dis || self.stance_off,
                orig_ranged,
                probe: cfg.coop.probe_er_aim,
                // C6 part 16.16: the loaded crossbow (code default false)
                load: cfg.coop.carrier_load,
            };
            if guard("carrier", || self.carrier.tick(&ci)).is_none() {
                self.carrier_off = true;
                log_e!("HOLD", "FAIL panic in the carrier tick; the carrier is off for this session (its journal stays: the next world enter of a new session restores)");
            }
            self.carrier.view(crate::game::ac130_read::main_player())
        };
        if !self.stance_off {
            let peers = if self.coopvis_off { Vec::new() } else { self.coopvis.peer_states() };
            // C6 part 17: the guard's weapon-frame carry is body-relative too
            let body_fwd = if cfg.coop.ads_pose_on() || cfg.coop.guard_pose_on() {
                let axis = match cfg.first_person.yaw_forward_axis {
                    mw2er_core::config::YawAxisCfg::PlusZ => mw2er_core::fp_math::Axis::PlusZ,
                    _ => mw2er_core::fp_math::Axis::MinusZ,
                };
                // SAFETY: our own ChrIns (main player) this tick; non-faulting reads.
                unsafe { crate::game::chars::read_pose(crate::game::ac130_read::main_player()) }.map(|x| mw2er_core::fp_math::forward_of(x.1, axis))
            } else {
                None
            };
            let si = crate::er_stance::StanceIn {
                now,
                cfg: &cfg,
                in_world,
                mode_on,
                // C6 scope peek: a third-person scoped ADS is still third person for the stance (no exit chord per scope-in)
                fp_on: st_fp,
                ac130: ac.active || ac.fp_block,
                nuke: nk.fp_block,
                latched: st_latched,
                menu_open: menu_is_open || !foreground,
                er_prompt: m.mask & mask_of(&cfg.gamepad.prompt_indices) != 0,
                player: crate::game::ac130_read::main_player(),
                handle: player.as_ref().map_or(0, |p| p.chr.handle),
                pos: player.as_ref().map(|p| p.chr.pos),
                peers,
                ads_held: ads_raw,
                fire_held: fire_raw,
                cam_fwd: cam.map(|c| c.row2),
                body_fwd,
                // C6 aim 4: the death refusal, ER's aim state and the aim pose's gates (sprint / lowered gun: last tick)
                dead: st_dead,
                flags1c5: fp_player.ok().flatten().map(|p| p.flags1c5),
                ads_aim: ads_raw && !self.mv.fire_blocked && !self.wgate.blocked(now),
                scoped: cfg.first_person.tp_scope_peek
                    && cfg.first_person.enabled
                    && cfg.viewmodel.enabled
                    && self.ars.as_ref().is_some_and(|a| a.spec().ads.overlay.is_some()),
                mounted: fp_player.ok().flatten().is_some_and(|p| p.is_mounted),
                camera_type: fp_world.and_then(|w| w.camera_type),
                // C6 round 6: ER still spawns the player (the 21:25:30 refresh fail: both chords during the respawn)
                settling: st_settling,
                carrier: carrier_view,
                // C6 part 17: the guard's body-yaw write (tp_guard_yaw) and the gun's align as coop_vis drew it this tick
                fp: fp_player.ok().flatten(),
                guard_align: if self.coopvis_off { None } else { self.coopvis.guard_align() },
                // C6 part 17 addendum: the guard frame the gun used (the episode line's corr=)
                guard_corr: if self.coopvis_off { None } else { self.coopvis.guard_corr() },
            };
            if guard("stance", || self.stance.tick(&si)).is_none() {
                self.stance_off = true;
                // C6 round 6: the aim-camera filter off before anything else (restore_all disarms too)
                crate::aim_cam::disarm();
                let _ = guard("stance.panic", || self.stance.restore_all("panic"));
                log_e!("HOLD", "FAIL panic in the stance tick; every patched row restored, er_stance off for this session");
            }
        }
        self.mv = self.movement.tick(&MoveIn {
            now,
            dt,
            cfg: &cfg,
            mode_on,
            in_world,
            menu_open: menu_is_open,
            blank_ok: blank,
            fp_on: self.fp_eye.is_some(),
            fp_peek: self.fp.peeking(),
            ac130_block: ac.fp_block,
            nuke_block: nk.fp_block,
            fp_player,
            world: fp_world,
            fp_cutscene_latched: self.fp.latched().1,
            cam_row2: cam.map(|c| c.row2),
            chars: &chars,
            cfg_gen: self.cfg_gen,
            player_handle: player.as_ref().map_or(0, |p| p.chr.handle),
            fire_held: fire_raw,
            ads_held: ads_raw,
            reload_press: reloads > 0,
        });
        if self.mv.sprint_started {
            self.cancel_sim_vm("sprint", true);
        }
        // M7 A3: an AC-130 ride starting cancels a reload / rechamber cleanly (logged), like a switch
        if ac.active && !self.ac130_was_active {
            self.cancel_sim_vm("ac130", true);
        }
        self.ac130_was_active = ac.active;
        // M8: the weapon gate (a mantle row >= 33 in, a ladder): the gun quick-drops, no fire / ADS / reload until the
        // quick raise ended; a running reload or rechamber is cancelled at the drop
        let raise_s = self.quick_times().1 as f64;
        match self.wgate.update(now, self.mv.mantle_lowered, on_ladder, raise_s) {
            Some(mw2er_core::mantle::GateEvent::Lowered(why)) => {
                log_i!("ARSENAL", "lowered why={why} gun={} quick_drop_s={:.2}{}", self.gun_name(), self.quick_times().0, self.quick_clip_text());
                self.cancel_sim_vm(why, true);
            }
            Some(mw2er_core::mantle::GateEvent::RaiseStart(why)) => {
                log_i!("ARSENAL", "raise why={why} gun={} quick_raise_s={raise_s:.2}{}", self.gun_name(), self.quick_clip_text());
            }
            Some(mw2er_core::mantle::GateEvent::Raised(why)) => log_i!("ARSENAL", "raised why={why} gun={}", self.gun_name()),
            None => {}
        }
        let gate_block = self.wgate.blocked(now);
        let reloads = if gate_block { 0 } else { reloads };
        // MW2: a reload pressed during the sprint (or its out time) starts after sprint_out
        let reloads = if self.mv.fire_blocked {
            self.pending_reloads = (self.pending_reloads + reloads).min(1);
            0
        } else {
            reloads + std::mem::take(&mut self.pending_reloads)
        };

        // M3b first person: after the gate (which saw ER's camera) and the toggles, before any shot.
        let chest = player.as_ref().map(|p| self.chest(p));
        // C6 scope peek (work/c6/scope3p.md): third-person MW2 ADS with a scoped gun runs first person while it is held
        let scope_peek = mw2er_core::fp_logic::peek_wanted(&mw2er_core::fp_logic::PeekIn {
            flag: cfg.first_person.tp_scope_peek,
            fp_enabled: cfg.first_person.enabled,
            viewmodel: cfg.viewmodel.enabled,
            overlay_gun: self.ars.as_ref().is_some_and(|a| a.spec().ads.overlay.is_some()),
            switch_ready: self.ars.as_ref().is_some_and(|a| a.sw.is_ready()),
            live: blank && mode_on && in_world,
            ac130: ac.active || ac.fp_block,
            nuke: nk.fp_block,
            ads_held: ads_raw && !self.mv.fire_blocked && !gate_block,
        });
        let fp_t0 = std::time::Instant::now();
        self.fp_eye = self.fp.tick(&TickIn {
            now,
            dt: dt as f32,
            cfg: &cfg.first_person,
            mode_on,
            in_world,
            menu_open: menu_is_open,
            cam_valid: self.cam_valid,
            cam_why: self.cam_why,
            er_pos: cam.map(|c| c.pos),
            chest,
            planar_speed: self.planar_speed,
            ac130_block: ac.fp_block,
            nuke_block: nk.fp_block,
            player_fp: fp_player,
            world: fp_world,
            move_feet: self.mv.feet_m,
            move_eye_h: self.mv.eye_h_m,
            ui_mask: m.mask,
            scope_peek,
        });
        self.perf.add_fp(fp_t0.elapsed().as_secs_f64() * 1e6 + fp_read_us);
        // C6 part 18 (docs/C6_HOLD_DESIGN.md part 18): the over-the-shoulder camera, after first person (an FP / peek
        // on this tick cuts / holds it) and before any shot (the shots aim from this tick's camera read, which already
        // holds the offset the detour wrote this frame: the crosshair ray). Off with both offsets zero.
        if guard("shoulder", || {
            self.shoulder.tick(&crate::shoulder_cam::ShoulderIn {
                now,
                dt: dt as f32,
                cfg: &cfg,
                gate: mw2er_core::shoulder::GateIn {
                    in_world,
                    latched: self.fp.latched().0 || self.fp.latched().1 || self.fp.latched().2,
                    ac130: ac.active || ac.fp_block,
                    nuke: nk.fp_block,
                    peek: self.fp.peeking(),
                    fp: self.fp_eye.is_some(),
                    mode_on,
                    dead: st_dead,
                    menu: menu_is_open,
                    mounted: fp_player.ok().flatten().is_some_and(|p| p.is_mounted),
                    cam_valid: self.cam_valid,
                    ..Default::default()
                },
                ads_w: if self.stance.guard_active() || self.stance.aim_on() { 1.0 } else { self.vm_state.ads_at(now) },
                cam,
                player: player.as_ref(),
            })
        })
        .is_none()
        {
            self.shoulder.emergency_off();
        }

        if let Some(sp) = self.spread_p {
            match (self.mv.driving, self.stance_spread()) {
                (true, Some(ss)) => {
                    self.spread.tick_stance(&sp, &ss, self.mv.spread_stance, dt as f32, self.mv.speed_mps, cfg.gameplay.use_move_spread);
                    if self.mv.jumped {
                        self.spread.on_jump(&ss);
                    }
                }
                _ => self.spread.tick(&sp, dt as f32, self.planar_speed, cfg.gameplay.use_move_spread),
            }
        }

        // M6 weapon switching (docs/M6_DESIGN.md section 5): keys 1/2 and the wheel; never during an AC-130 ride
        let mut switching = false;
        // review M6: the gun in hand whose viewmodel bundle failed (no VmAnim, no ADS) hands over to the first usable
        // gun, as a gun with failed rows does in check_params
        if let Some(a) = self.ars.as_ref()
            && a.sw.is_ready()
            && crate::arsenal_rt::vm_failed(a.cur)
            && let Some(i) = a.enabled().iter().position(|e| *e)
        {
            log_w!("ARSENAL", "start gun disabled gun={} why=assets; using {}", a.cur_gun().name, a.gun(i).name);
            self.select_gun(i, true);
        }
        if self.ars.is_some() {
            let live = blank && mode_on && in_world && !ac.active;
            let busy = self.sim.as_ref().is_some_and(|s| s.burst_in_flight());
            let evs = match self.ars.as_mut() {
                Some(a) => {
                    let req = a.take_request(live, cfg.arsenal.wheel, cfg.arsenal.wheel_invert);
                    a.switch_tick(now, req, ac.active, mode_on, busy)
                }
                None => Vec::new(),
            };
            for e in evs {
                self.on_switch_event(e);
            }
            switching = self.ars.as_ref().is_some_and(|a| !a.sw.is_ready());
            // review M6 safety net: the machine is Ready but the viewmodel still waits for a raise that is not coming
            // (no SwitchRaise queued): end the viewmodel switch so it raises the gun in hand instead of staying hidden
            let raise_queued = self.vm_events.iter().any(|e| matches!(e, VmEvent::SwitchRaise { .. }));
            if !switching && !raise_queued && self.vm.as_ref().is_some_and(|vm| vm.switching()) {
                if let Some(vm) = self.vm.as_mut() {
                    vm.cancel_switch();
                }
                log_w!("SWITCH", "vm_unstuck gun={} why=ready_without_raise", self.gun_name());
            }
        }
        // Weapon simulation and its events. The raw fire button is a sticky last value: it only
        // counts while the game is still polling the mouse.
        if self.breaker.poll(shared::now_s()) {
            self.breaker_open = false;
            self.spawn_failures = 0;
            log_i!("SPAWN", "breaker closed after {SPAWN_BREAKER_COOLDOWN_S} s; firing retries");
        }
        let can_fire = blank && self.params_ok == Some(true) && !self.breaker.is_open(shared::now_s()) && self.world_ticks >= FIRE_WARMUP_TICKS && !dead_block;
        // M4: the UMP45 ignores LMB during an AC-130 ride and until LMB was released after it
        // M5: no shot and no ADS while sprinting and during sprintOutTime after it
        // M8: and nothing while the weapon gate holds the gun down (mantle rows >= 33 in, ladders)
        let mut trigger = can_fire && fire_raw && !self.mv.fire_blocked && !gate_block;
        let ads_held = ads_raw && !self.mv.fire_blocked && !gate_block;
        // M6: a lock-on launcher without a lock does not fire (MW2 requireLockonToFire; `[launchers] lock`)
        if trigger
            && let Some(a) = self.ars.as_mut()
            && a.spec().is_launcher()
            && a.lock_refuses(&cfg)
        {
            if now - a.lock_refused_t > 0.5 {
                log_i!("LAUNCH", "refused gun={} why=no_lock", a.cur_gun().name);
            }
            a.lock_refused_t = now;
            trigger = false;
        }
        let mut events = std::mem::take(&mut self.events);
        events.clear();
        // review M6: a burst in flight stops when the gun may no longer fire (menu, focus, AC-130 start, breaker)
        if !(can_fire && !ac.ump_block)
            && let Some(sim) = self.sim.as_mut()
        {
            let mut ev = Vec::new();
            if sim.cancel_burst(&mut ev) {
                let why = if ac.ump_block { "ac130" } else if dead_block { "dead" } else if !blank { "blocked" } else { "gate" };
                log_i!("SHOT", "burst cancelled why={why}");
            }
        }
        if mode_on
            && in_world
            && let Some(sim) = self.sim.as_mut()
        {
            // M6: no shot and no rechamber start while switching or sprinting (a cancelled rechamber resumes after)
            sim.set_hold(switching || self.mv.fire_blocked || gate_block || dead_block);
            sim.step(now, trigger, reloads, &mut events);
        }
        for ev in events.iter().copied() {
            match ev {
                SimEvent::Shot { mag_after } => {
                    self.vm_events.push(if mag_after == 0 && self.ars.is_some() { VmEvent::LastShot } else { VmEvent::Shot });
                    if let Some(p) = player.as_ref() {
                        self.fire(now, p, cam, &chars, mag_after);
                    }
                }
                SimEvent::RechamberStart { duration_s } => {
                    let ads = self.vm_state.ads_at(now) >= 0.5;
                    self.vm_events.push(VmEvent::Rechamber { ads });
                    log_i!("RECHAMBER", "start gun={} t={duration_s:.3} ads={}", self.gun_name(), ads as u8);
                }
                SimEvent::RechamberDone => log_i!("RECHAMBER", "done gun={}", self.gun_name()),
                // a reload started over a running rechamber (the reload chambers the gun)
                SimEvent::RechamberCancelled => log_i!("RECHAMBER", "cancelled gun={} reason=reload", self.gun_name()),
                SimEvent::Segment { phase, duration_s } => {
                    self.vm_events.push(match phase {
                        SegPhase::Start => VmEvent::SegStart,
                        SegPhase::Loop => VmEvent::SegLoop,
                        SegPhase::End => VmEvent::SegEnd,
                    });
                    log_i!("RELOAD", "segment gun={} phase={} dur={duration_s:.3}", self.gun_name(), phase.label());
                }
                SimEvent::ShellAdded { added, mag, reserve } => {
                    log_i!("RELOAD", "shell gun={} added={added} mag={mag} reserve={reserve}", self.gun_name())
                }
                SimEvent::BurstDone { fired, span_s } => log_i!("BURST", "gun={} n={fired} t_ms={:.0}", self.gun_name(), span_s * 1000.0),
                SimEvent::ReloadDeferred => log_i!("RELOAD", "deferred gun={} (burst in flight)", self.gun_name()),
                SimEvent::ReloadStart { empty, duration_s } => {
                    self.vm_events.push(VmEvent::ReloadStart { empty });
                    let mag = self.sim.as_ref().map_or(0, |s| s.mag());
                    log_i!("RELOAD", "start empty={} dur={duration_s} mag={mag} gun={}", empty as u8, self.gun_name());
                }
                SimEvent::ReloadDone { added, mag, reserve } => {
                    self.vm_events.push(VmEvent::ReloadDone);
                    log_i!("RELOAD", "done mag={mag} reserve={reserve} added={added} gun={}", self.gun_name());
                }
                SimEvent::ReloadCancelled => {
                    self.vm_events.push(VmEvent::ReloadCancelled);
                    log_i!("RELOAD", "cancelled")
                }
                SimEvent::ReloadIgnored(r) => log_i!(
                    "RELOAD",
                    "ignored ({})",
                    match r {
                        ReloadIgnored::Full => "full",
                        ReloadIgnored::NoReserve => "no reserve",
                    }
                ),
                SimEvent::DryFire => log_i!("AMMO", "empty (dry fire) mag=0 reserve=0"),
                SimEvent::Cue(c) => shared::play(c),
            }
        }
        self.events = events;
        spawn_guard::tick(now);
        self.step_viewmodel(now, mode_on, in_world, menu_is_open, ads_held);
        // M6: notetrack sounds, view kick / sway / zoom, lock-on, projectiles, the bug-2 pool line and owner retry
        if self.ars.is_some() {
            self.arsenal_tick(now, dt as f32, mode_on && in_world && blank && !ac.active, cam, player.as_ref(), &chars);
        }

        if let Some(p) = player.as_ref() {
            self.poll_bullets(now, p);
            self.poll_hp(now, p, &chars);
        }

        // C3 (docs/C3_DESIGN.md 2.3): the UI yield (render-only: it reads ER's message signals, never blocks a shot)
        let _ = guard("ui_yield", || self.ui_yield_tick(now, in_world && mode_on, m.mask, fire_raw || ads_raw));
        self.ui_probe_log.tick(now);
        let _ = guard("depth_probe", || self.depth_probe.tick(now, in_world));
        let _ = guard("depth_state", || self.depth_state.tick(now, in_world, &cfg));
        let _ = guard("ui_inject", || self.ui_inject_log.tick(now));
        self.dmg.prune(now, cfg.hud.damage_numbers_s as f64);
        self.view_pos = match (self.fp_eye, cam) {
            (Some(e), _) => [e.x, e.y, e.z],
            (None, Some(c)) => [c.pos.x, c.pos.y, c.pos.z],
            _ => [0.0; 3],
        };
        if self.dmg_cfg_gen != Some(self.cfg_gen) {
            self.dmg_cfg_gen = Some(self.cfg_gen);
            let h = &cfg.hud;
            log_i!(
                "HUD",
                "dmgnum cfg on={} life={:.2} merge={:.2} value={} head_guess={} max={} scale={:.2} suppress_er_tag={}",
                h.damage_numbers as u8,
                h.damage_numbers_s,
                h.damage_numbers_merge_s,
                h.damage_numbers_value,
                h.damage_numbers_head_guess as u8,
                h.damage_numbers_max,
                h.damage_numbers_scale,
                h.damage_numbers_suppress_with_er_tag as u8
            );
        }

        self.publish(now, in_world, mode_on, menu_is_open, foreground, &chars, player.as_ref());
    }

    fn check_params(&mut self) {
        let Some(d) = self.data.clone() else {
            self.params_ok = Some(false);
            return;
        };
        // M4: the AC-130 rows are checked separately (PARAMS OK n= stays the M2 row count)
        if self.cfg.ac130.enabled {
            self.ac130.check_rows();
        }
        // M6 (docs/M6_DESIGN.md 9.7): every arsenal gun's rows; a mismatch disables that gun only
        if let Some(a) = self.ars.as_mut() {
            let (mut ok, mut rows) = (0, 0);
            for i in 0..a.data.guns.len() {
                let g = &a.data.guns[i];
                match params::check_arsenal_gun(&a.data.rows, &g.rows, a.data.marker_id, a.data.bullet_speed_mps) {
                    Ok(n) => {
                        ok += 1;
                        rows += n;
                        a.params_ok[i] = Some(true);
                    }
                    Err(e) => {
                        a.params_ok[i] = Some(false);
                        log_e!("PARAMS", "FAIL gun={} {e}; that gun is disabled", g.name);
                        log_e!("ARSENAL", "disable gun={} why=params", g.name);
                    }
                }
            }
            log_i!("PARAMS", "OK guns={ok} rows={rows} mode={} s={} d_ref={} bullet_sfx={}", a.data.rows.mode, a.data.rows.s, a.data.rows.d_ref, a.data.rows.bullet_sfx);
            if ok == 0 {
                self.params_ok = Some(false);
                log_e!("PARAMS", "FAIL no arsenal gun usable; firing disabled");
                shared::toast("mw2er: regulation rows missing/stale (launched without the package?)", 10.0, true);
                return;
            }
            // the gun in hand must be usable
            if a.params_ok[a.cur] == Some(false)
                && let Some(i) = (0..a.data.guns.len()).find(|&i| a.params_ok[i] == Some(true))
            {
                log_w!("ARSENAL", "the start gun {} is disabled; using {}", a.data.guns[a.cur].name, a.data.guns[i].name);
                self.select_gun(i, true);
            }
            self.params_ok = Some(true);
            return;
        }
        match params::check(&d.rows, &d.weapon.name) {
            Ok(n) => {
                self.params_ok = Some(true);
                log_i!("PARAMS", "OK n={n}");
            }
            Err(e) => {
                self.params_ok = Some(false);
                log_e!("PARAMS", "FAIL {e}; firing disabled");
                shared::toast("mw2er: regulation rows missing/stale (launched without the package?)", 10.0, true);
            }
        }
    }

    fn toggle(&mut self, player: Option<&PlayerSample>, menu_is_open: bool, foreground: bool, m: MenuSample) {
        let mask = m.mask;
        if MODE_ON.load(Ordering::SeqCst) {
            // M7: not while a Tactical Nuke sequence runs (the sequence needs MW2 mode until it ends)
            if self.nuke.busy() {
                log_i!("MODE", "toggle blocked reason=nuke");
                shared::toast("MW2 mode stays on until the Tactical Nuke sequence ends", 3.0, true);
                return;
            }
            // Turning off is always allowed: it only gives control back to the game.
            MODE_ON.store(false, Ordering::SeqCst);
            WANT_BLANK_FIRE.store(false, Ordering::SeqCst);
            WANT_BLANK_RELOAD.store(false, Ordering::SeqCst);
            shared::WANT_HEAL.store(false, Ordering::SeqCst);
            self.cancel_sim("mode_off");
            log_i!("MODE", "off");
            shared::toast("MW2 mode OFF", 1.5, false);
            return;
        }
        let refuse = if GAMEPLAY_DISABLED.load(Ordering::SeqCst) {
            Some("gameplay disabled after an error (see log)")
        } else if self.data.is_none() && self.ars.is_none() {
            Some("data not loaded (DATA FAIL in the log)")
        } else if !INPUT_HOOK_OK.load(Ordering::SeqCst) {
            Some("input hook failed (the game would attack too)")
        } else if player.is_none() {
            Some("not in world")
        } else if self.params_ok != Some(true) {
            Some("regulation rows missing/stale (PARAMS FAIL)")
        } else if menu_is_open {
            Some("a menu is open")
        } else if let Some(r) = enable_refusal(&self.cfg.menu.hud_indices, self.cfg.menu.cursor_gate_at_enable, m.show_cursor) {
            Some(r)
        } else if !foreground {
            Some("game window not focused")
        } else {
            None
        };
        if let Some(r) = refuse {
            log_i!(
                "MODE",
                "toggle ignored ({r}) ui_states={:?} show_cursor={}",
                indices_of(mask),
                m.show_cursor.map_or("?", |c| if c { "1" } else { "0" })
            );
            shared::toast(format!("MW2 mode refused: {r}"), 4.0, true);
            return;
        }
        self.menu_baseline = Some(mask);
        if self.cfg.gameplay.refill_on_mode_enable {
            match self.ars.as_mut() {
                // M6: every gun refilled (reserve per `[arsenal] reserve_source`), the parked ones too
                Some(a) => {
                    let src = self.cfg.arsenal.reserve_source;
                    for i in 0..a.data.guns.len() {
                        let r = a.data.guns[i].spec.refill_reserve(src);
                        if let Some(s) = a.parked[i].sim.as_mut() {
                            s.refill_with(r);
                        }
                    }
                    let r = a.spec().refill_reserve(src);
                    if let Some(sim) = self.sim.as_mut() {
                        sim.refill_with(r);
                    }
                }
                None => {
                    if let Some(sim) = self.sim.as_mut() {
                        sim.refill();
                    }
                }
            }
        }
        if let Some(a) = self.ars.as_mut() {
            a.sw.reset();
            if let Some(vm) = self.vm.as_mut() {
                vm.cancel_switch();
            }
        }
        MODE_ON.store(true, Ordering::SeqCst);
        let (mag, reserve) = self.sim.as_ref().map_or((0, 0), |s| (s.mag(), s.reserve()));
        let weapon = match self.ars.as_ref() {
            Some(a) => a.cur_gun().name.as_str(),
            None => self.data.as_ref().map_or("?", |d| d.weapon.name.as_str()),
        };
        log_i!(
            "MODE",
            "on weapon={weapon} mag={mag}/{reserve} menu_baseline={:?} hud_watch={:?} show_cursor={}",
            indices_of(mask),
            indices_of(mask_of(&self.cfg.menu.hud_off_indices) & mask),
            m.show_cursor.map_or("?", |c| if c { "1" } else { "0" })
        );
        shared::toast("MW2 mode ON", 1.5, false);
    }

    fn fire(&mut self, now: f64, p: &PlayerSample, cam: Option<CameraSample>, chars: &[ChrRead], mag_after: u32) {
        self.shots += 1;
        // M6: an arsenal gun (pellets, launchers, kick, the per-shot breaker)
        if self.ars.is_some() {
            self.fire_arsenal(now, p, cam, chars, mag_after);
            return;
        }
        let cfg = self.cfg.clone();
        let (Some(d), Some(sp), Some(falloff)) = (self.data.clone(), self.spread_p, self.falloff) else { return };
        let reserve = self.sim.as_ref().map_or(0, |s| s.reserve());
        shared::play(SfxCue::Fire);
        let Some(c) = cam else {
            log_w!("SHOT", "n={} skipped: no camera", self.shots);
            return;
        };
        // M3: the forward sign is fixed at +1 (every valid sample agreed; the -1 resolutions came from the
        // placeholder camera right after loading) and shots wait for the camera gate. The sign tracker only logs
        // disagreements now; `aim.camera_gate = false` restores the M2 behaviour.
        let fwd = if cfg.aim.camera_gate {
            if !self.cam_valid {
                log_w!("SHOT", "n={} skipped: camera settling ({})", self.shots, self.cam_why.label());
                return;
            }
            view_forward(c.row2, 1.0)
        } else {
            self.cam_sign.sign().and_then(|s| view_forward(c.row2, s))
        };
        let Some(fwd) = fwd else {
            log_w!("SHOT", "n={} skipped: camera direction unresolved", self.shots);
            return;
        };
        if let Err(e) = owner_check(p.field_ins_handle) {
            log_w!("SHOT", "n={} skipped: {e}", self.shots);
            return;
        }
        let chest = self.chest(p);
        // M3b: in first person the screen centre is the eye ray, so the bullet leaves from the eye; the validity
        // check below still uses ER's third-person camera position.
        let origin = bullet_origin(self.fp_eye.unwrap_or(c.pos), fwd, chest, cfg.aim.origin_ahead_m);
        let spread_deg = self.spread_deg(&sp);
        let dir = match (cfg.gameplay.apply_spread_to_bullets, basis(fwd)) {
            (true, Some((right, up))) => perturb(fwd, right, up, spread_deg, &mut self.rng),
            _ => fwd,
        };
        // Everything below goes into engine calls (ray cast, spawn_bullet): sanity-check it first.
        if let Err(r) = check_shot_fp(c.pos, chest, origin, dir, cfg.aim.origin_ahead_m, cfg.aim.max_range_m, self.fp_eye.is_some()) {
            log_w!("SHOT", "n={} skipped: {} origin={} dir={}", self.shots, r.reason(), logfmt::v3(origin), logfmt::dir3(dir));
            return;
        }

        // Falloff band from the distance to the first expected hit (character near the ray, or map).
        let chests: Vec<V3> = chars
            .iter()
            .filter(|ch| {
                ch.handle != p.chr.handle
                    && ch.hp > 0
                    && !cfg.hit.exclude_character_ids.contains(&ch.character_id)
                    && (ch.pos - p.chr.pos).len() <= cfg.aim.max_range_m
            })
            .map(|ch| ch.pos + V3::new(0.0, cfg.aim.chest_height_m, 0.0))
            .collect();
        let map_t = havok::map_ray(origin, p.chr.pos_w, dir, cfg.aim.max_range_m, p.player_ins);
        let (dist, src) =
            estimate_target_distance(origin, dir, &chests, map_t, cfg.aim.max_range_m, cfg.aim.char_radius_m);
        let mw2 = falloff.damage_at(dist as f64);
        let Some(band) = d.rows_weapon.band_for_damage(mw2).copied() else { return };

        if cfg.gameplay.spawn_bullets && !self.breaker_open && spawn_guard::admit(now, SpawnKind::Gun, band.id) {
            if !self.first_spawn_done {
                log_i!(
                    "SPAWN",
                    "try id={} owner={} origin={} w={} dir={} (first spawn of the session)",
                    band.id,
                    logfmt::handle(p.chr.handle),
                    logfmt::v3(origin),
                    p.chr.pos_w,
                    logfmt::dir3(dir)
                );
            }
            match spawn::spawn(p.field_ins_handle, band.id, origin, p.chr.pos_w, dir) {
                Ok(_) => {
                    if !self.first_spawn_done {
                        log_i!("SPAWN", "ok (first spawn)");
                    }
                    self.first_spawn_done = true;
                    self.spawn_failures = 0;
                    self.spawns_world += 2;
                    self.breaker.record(now, true, SPAWN_BREAKER, SPAWN_BREAKER_COOLDOWN_S);
                }
                Err(e) => {
                    self.first_spawn_done = true;
                    self.spawn_failures += 1;
                    // M6 bug 2: the failure names its cause and the pool counters
                    let name = d.weapon.name.clone();
                    self.on_spawn_fail(&e, band.id, p, origin, dir, &name);
                    if self.breaker.record(now, false, SPAWN_BREAKER, SPAWN_BREAKER_COOLDOWN_S) {
                        self.on_breaker_open(p, origin, dir);
                    }
                }
            }
        }
        if cfg.log.shots == ShotLog::All {
            log_i!(
                "SHOT",
                "{}",
                logfmt::shot_msg(&ShotFields {
                    n: self.shots,
                    mag: mag_after,
                    reserve,
                    row_id: band.id,
                    mw2,
                    d_m: dist,
                    src: src.label(),
                    spread_deg,
                    origin,
                    dir,
                })
            );
        }
        self.push_shot(ShotInfo { t: now, band_id: band.id, mw2, d: dist, origin, gun: u16::MAX });
        self.spread.on_shot(&sp);
    }

    fn poll_bullets(&mut self, now: f64, p: &PlayerSample) {
        let Some(shot) = self.last_shot else { return };
        if now - shot.t > BULLET_POLL_AFTER_SHOT_S || self.our_ids.is_empty() {
            self.seen_markers.clear();
            return;
        }
        let Some(d) = self.data.as_ref() else { return };
        let marker = d.rows.marker_id;
        let expected = (d.rows.bullet.speed_mps * d.rows.bullet.life_s) as f32;
        let list = bullets::read_ours(p.chr.handle, &self.our_ids);
        let mut present = HashSet::with_capacity(list.len());
        for b in &list {
            present.insert(b.handle);
            if b.param_id == marker {
                if self.seen_markers.insert(b.handle) {
                    let dist = (b.pos - shot.origin).len();
                    let kind = if (dist - expected).abs() <= EXPIRED_TOLERANCE_M { "expired" } else { "hit" };
                    if kind == "hit" {
                        self.impacts.push(ImpactSample { t: now, pos: [b.pos.x, b.pos.y, b.pos.z], used: false });
                    }
                    log_i!(
                        "IMPACT",
                        "pos={} dist_from_origin={:.1}m t={:.3}s kind={kind}",
                        logfmt::v3(b.pos),
                        dist,
                        now - shot.t
                    );
                }
            } else if !self.first_band_bullet_logged {
                self.first_band_bullet_logged = true;
                log_i!(
                    "BULLET",
                    "id={} pos={} origin={} speed={:.1} m/s alive={:.3}s",
                    b.param_id,
                    logfmt::v3(b.pos),
                    logfmt::v3(shot.origin),
                    b.vel.len(),
                    b.time_alive
                );
            }
        }
        self.seen_markers.retain(|h| present.contains(h));
    }

    fn poll_hp(&mut self, now: f64, p: &PlayerSample, chars: &[ChrRead]) {
        let samples: Vec<ChrSample> = chars
            .iter()
            .map(|c| ChrSample {
                handle: c.handle,
                character_id: c.character_id,
                hp: c.hp,
                max_hp: c.max_hp,
                last_hit_by: c.last_hit_by,
                pos: [c.pos.x, c.pos.y, c.pos.z],
            })
            .collect();
        prune_impacts(&mut self.impacts, now, IMPACT_KEEP_S);
        // M4: an AC-130 explosion also opens the HP-drop window (its drops are attributed below)
        let last_shot_t = match (self.last_shot.map(|s| s.t), self.ac130.last_explosion()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let ctx = HitContext {
            player_handle: p.chr.handle,
            now,
            last_shot_t,
            window_s: self.cfg.hit.window_s,
            exclude_character_ids: &self.cfg.hit.exclude_character_ids,
            impact_window_s: self.cfg.hit.impact_window_s,
            impact_lag_s: self.cfg.hit.impact_lag_s,
            impact_radius_m: self.cfg.hit.impact_radius_m,
            capsule_height_m: self.cfg.hit.capsule_height_m,
        };
        let mut evs = Vec::new();
        self.hit.update(&samples, &ctx, &mut self.impacts, &mut evs);
        for e in evs {
            match e {
                HitEvent::Hit { character_id, handle, hp_before, hp_after, via, n, kill, t_drop, impact_pos } => {
                    use mw2er_core::dmgnum::DmgSrc;
                    // review C3: a peer's nuke runs here: its kills (our lethal bullet on this PC) are not our hits
                    if self.nuke.remote_running() {
                        self.peer_drops += 1;
                        log_i!("HIT", "peer_drop chr=c{character_id:04} hp={hp_before}->{hp_after} dmg={} kill={} src=remote_nuke n={}", hp_before - hp_after, kill as u8, self.peer_drops);
                        continue;
                    }
                    let chr_pos = chars.iter().find(|c| c.handle == handle).map(|c| c.pos);
                    // Drops resolve up to impact_lag_s late: report the shot that was current at the drop.
                    let shot = self.shot_hist.iter().rev().find(|s| s.t <= t_drop + 1e-6).copied().or(self.last_shot);
                    // Run 8: ER sets last_hit_by for our AC-130 explosion bullets too (two kills were logged as
                    // UMP45 band 0). A last_hit_by drop with no UMP45 impact goes to the AC-130 attribution first
                    // when an explosion sphere covers the character at the drop time.
                    // M6: a launcher explosion covering the character at the drop time
                    if let (Some(a), Some(pos)) = (self.ars.as_ref(), chars.iter().find(|c| c.handle == handle).map(|c| c.pos))
                        && shot.is_none_or(|s| s.d == 0.0 || via == Via::LastHitBy)
                        && let Some(b) = a.attribute(pos, t_drop, self.cfg.hit.capsule_height_m)
                    {
                        let dmg = hp_before - hp_after;
                        // review C3: the peer-drop guard on the launcher path too (all tiers + the impact row as the bound)
                        if Self::peer_drop_explosive(&self.cfg, self.coop.peers(), &mut self.peer_drops, character_id, hp_before, hp_after, kill, launcher_max_atk(a, b.gun), "launcher") {
                            continue;
                        }
                        log_i!(
                            "HIT",
                            "chr={character_id} hp={hp_before}->{hp_after} dmg={dmg} src=launcher gun={} tier={} kill={}",
                            a.gun(b.gun).name,
                            ["core", "ring", "single"][b.tier.min(2) as usize],
                            kill as u8
                        );
                        self.on_hit(now, handle, chr_pos, dmg, kill, DmgSrc::Launcher, impact_pos, None);
                        if kill {
                            self.ks_kill(now, handle, KillSrc::Launcher, p);
                        }
                        continue;
                    }
                    if via == Via::LastHitBy {
                        let pos = chars.iter().find(|c| c.handle == handle).map(|c| c.pos);
                        let newer_than_shot = shot.is_none_or(|s| self.ac130.last_explosion().is_some_and(|x| x >= s.t));
                        let ac = pos.filter(|_| newer_than_shot).and_then(|pos| {
                            self.ac130.attribute(now, character_id, hp_before, hp_after, t_drop, pos, self.cfg.hit.capsule_height_m)
                        });
                        if let Some((msg, kill_msg)) = ac {
                            // review C3: the peer-drop guard on the AC-130 path (the largest gun's tiers as the bound)
                            if Self::peer_drop_explosive(&self.cfg, self.coop.peers(), &mut self.peer_drops, character_id, hp_before, hp_after, kill, ac130_max_atk(), "ac130") {
                                continue;
                            }
                            log_i!("HIT", "{msg}");
                            if let Some(k) = &kill_msg {
                                log_i!("AC130", "{k}");
                            }
                            self.on_hit(now, handle, chr_pos, hp_before - hp_after, kill, DmgSrc::Ac130, impact_pos, None);
                            if kill_msg.is_some() {
                                self.ks_kill(now, handle, KillSrc::Ac130, p);
                            }
                            continue;
                        }
                    }
                    let (band, mw2, d) = shot.map_or((0, 0, 0.0), |s| (s.band_id, s.mw2, s.d));
                    // C3 PASS C (docs/C3_DESIGN.md 3.7): in a session, a drop our shot cannot have dealt (more than the
                    // row's flat attack x pellets x factor) is a peer's: no kill credit, no streak point, no number
                    if self.cfg.coop.peer_drop_guard
                        && let (Some(a), Some(s)) = (self.ars.as_ref(), shot)
                        && (s.gun as usize) < a.data.guns.len()
                    {
                        let g = a.gun(s.gun as usize);
                        let atk = g.rows.bands.iter().find(|b| b.id == band).map(|b| b.atk);
                        let peers = self.coop.peers();
                        let dmg = hp_before - hp_after;
                        if mw2er_core::coop_vis::peer_drop(peers, dmg, atk, g.spec.pellets.max(1), self.cfg.coop.peer_drop_factor) {
                            self.peer_drops += 1;
                            log_i!(
                                "HIT",
                                "peer_drop chr=c{character_id:04} hp={hp_before}->{hp_after} dmg={dmg} kill={} band={band} atk={} pellets={} factor={:.1} peers={peers} via={} n={}",
                                kill as u8,
                                atk.unwrap_or(0),
                                g.spec.pellets.max(1),
                                self.cfg.coop.peer_drop_factor,
                                via.label(),
                                self.peer_drops
                            );
                            continue;
                        }
                    }
                    let attr = HitAttr { via: via.label(), n, kill };
                    let gun = match (self.ars.as_ref(), shot) {
                        (Some(a), Some(s)) if (s.gun as usize) < a.data.guns.len() => format!(" gun={}", a.gun(s.gun as usize).name),
                        _ => String::new(),
                    };
                    log_i!("HIT", "{}{gun}", logfmt::hit_msg(character_id, hp_before, hp_after, band, mw2, d, attr));
                    self.on_hit(now, handle, chr_pos, hp_before - hp_after, kill, DmgSrc::Gun, impact_pos, Some(mw2));
                    if kill {
                        self.ks_kill(now, handle, KillSrc::Gun, p);
                    }
                }
                HitEvent::Kill { character_id, .. } => {
                    log_i!("KILL", "{}", logfmt::kill_msg(character_id));
                    self.last_hit_kill = true;
                }
                HitEvent::HpDrop { character_id, handle, hp_before, hp_after, last_hit_by, t_drop } => {
                    let pos = chars.iter().find(|c| c.handle == handle).map(|c| c.pos);
                    // review C3: no attribution while a peer's nuke runs here
                    if self.nuke.remote_running() {
                        log_i!("HPDROP", "{} (remote nuke)", logfmt::hpdrop_msg(character_id, hp_before, hp_after, last_hit_by));
                        continue;
                    }
                    if let (Some(a), Some(pp)) = (self.ars.as_ref(), pos)
                        && let Some(b) = a.attribute(pp, t_drop, self.cfg.hit.capsule_height_m)
                    {
                        if Self::peer_drop_explosive(&self.cfg, self.coop.peers(), &mut self.peer_drops, character_id, hp_before, hp_after, hp_after <= 0, launcher_max_atk(a, b.gun), "launcher") {
                            continue;
                        }
                        log_i!(
                            "HIT",
                            "chr={character_id} hp={hp_before}->{hp_after} dmg={} src=launcher gun={} tier={} kill={}",
                            hp_before - hp_after,
                            a.gun(b.gun).name,
                            ["core", "ring", "single"][b.tier.min(2) as usize],
                            (hp_after <= 0) as u8
                        );
                        self.on_hit(now, handle, pos, hp_before - hp_after, hp_after <= 0, mw2er_core::dmgnum::DmgSrc::Launcher, None, None);
                        if hp_after <= 0 {
                            self.ks_kill(now, handle, KillSrc::Launcher, p);
                        }
                        continue;
                    }
                    let ac = pos.and_then(|pos| {
                        self.ac130.attribute(now, character_id, hp_before, hp_after, t_drop, pos, self.cfg.hit.capsule_height_m)
                    });
                    match ac {
                        Some(_) if Self::peer_drop_explosive(&self.cfg, self.coop.peers(), &mut self.peer_drops, character_id, hp_before, hp_after, hp_after <= 0, ac130_max_atk(), "ac130") => {}
                        Some((msg, kill_msg)) => {
                            log_i!("HIT", "{msg}");
                            if let Some(k) = &kill_msg {
                                log_i!("AC130", "{k}");
                            }
                            self.on_hit(now, handle, pos, hp_before - hp_after, kill_msg.is_some(), mw2er_core::dmgnum::DmgSrc::Ac130, None, None);
                            if kill_msg.is_some() {
                                self.ks_kill(now, handle, KillSrc::Ac130, p);
                            }
                        }
                        None => log_i!("HPDROP", "{}", logfmt::hpdrop_msg(character_id, hp_before, hp_after, last_hit_by)),
                    }
                }
            }
        }
    }

    /// Review C3: the peer-drop guard of the explosive paths (launcher, AC-130): in a session a drop above `factor` x
    /// the largest damage our explosion can deal is a peer's (no credit, number or streak point). Logs `HIT peer_drop`.
    #[allow(clippy::too_many_arguments)]
    fn peer_drop_explosive(cfg: &Config, peers: u32, peer_drops: &mut u32, character_id: u32, hp_before: i32, hp_after: i32, kill: bool, max_atk: Option<i32>, src: &str) -> bool {
        if !cfg.coop.peer_drop_guard {
            return false;
        }
        let dmg = hp_before - hp_after;
        if !mw2er_core::coop_vis::peer_drop(peers, dmg, max_atk, 1, cfg.coop.peer_drop_factor) {
            return false;
        }
        *peer_drops += 1;
        log_i!(
            "HIT",
            "peer_drop chr=c{character_id:04} hp={hp_before}->{hp_after} dmg={dmg} kill={} src={src} atk={} factor={:.1} peers={peers} n={}",
            kill as u8,
            max_atk.unwrap_or(0),
            cfg.coop.peer_drop_factor,
            *peer_drops
        );
        true
    }

    /// M7: one kill attributed to an MW2 weapon counts towards the Tactical Nuke streak.
    fn ks_kill(&mut self, now: f64, handle: u64, src: KillSrc, p: &PlayerSample) {
        let cfg = self.cfg.clone();
        self.nuke.on_kill(now, handle, src, p.chr.hp > 0, MODE_ON.load(Ordering::SeqCst), &cfg);
    }

    fn log_camera_probe(&self, player: Option<&PlayerSample>) {
        let fmt = |c: Option<CameraSample>| {
            c.map_or("none".to_string(), |c| {
                format!("pos={} row2={} fov={}", logfmt::v3(c.pos), logfmt::dir3(c.row2), c.fov_raw)
            })
        };
        log_i!(
            "CAMERA",
            "probe tick={} pers_cam_1[{}] chr_cam[{}] player={}",
            self.world_ticks,
            fmt(read_camera(CameraSource::PersCam1)),
            fmt(read_camera(CameraSource::ChrCam)),
            player.map_or("none".to_string(), |p| logfmt::v3(p.chr.pos))
        );
    }

    /// Camera validity gate (docs/M3_DESIGN.md section 9) + CAMFRAME diagnostics.
    fn update_cam_gate(&mut self, now: f64, cam: Option<CameraSample>, player: Option<&PlayerSample>) {
        // run 18: while first person drives the render camera, ER's camera pulled next to the chest by its collision
        // is not a space mismatch (only the upper bound and the re-base grace still apply)
        let close_ok = MODE_ON.load(Ordering::SeqCst) && self.fp.close_cam_ok(&self.cfg.first_person);
        let gp = GateParams { det_sign: self.cfg.aim.camera_det_sign as f32, close_ok, ..GateParams::default() };
        let sample = match (cam, player) {
            (Some(c), Some(p)) => {
                let chest = self.chest(p);
                Some(CamSample {
                    rows: c.rows,
                    pos: [c.pos.x, c.pos.y, c.pos.z],
                    fov: c.fov_raw,
                    chest: [chest.x, chest.y, chest.z],
                    player: [p.chr.pos.x, p.chr.pos.y, p.chr.pos.z],
                })
            }
            _ => None,
        };
        let was = self.cam_valid;
        let (valid, why) = self.cam_gate.update(sample.as_ref(), &gp);
        self.cam_valid = valid && player.is_some();
        self.cam_why = why;
        let due = (player.is_some() && self.world_ticks <= CAMFRAME_FIRST_TICKS) || now - self.last_camframe_t >= CAMFRAME_EVERY_S;
        if player.is_some() && (due || was != self.cam_valid) {
            self.last_camframe_t = now;
            let (pos, fwd, det, oe, fov, aspect, near, far, dist) = match (cam, sample) {
                (Some(c), Some(s)) => (
                    logfmt::v3(c.pos),
                    logfmt::dir3(c.row2),
                    det3(&c.rows),
                    ortho_err(&c.rows),
                    c.fov_raw,
                    c.aspect,
                    c.near,
                    c.far,
                    ((s.pos[0] - s.chest[0]).powi(2) + (s.pos[1] - s.chest[1]).powi(2) + (s.pos[2] - s.chest[2]).powi(2)).sqrt(),
                ),
                _ => ("none".into(), "none".into(), f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN),
            };
            log_i!(
                "CAMFRAME",
                "tick={} src={} valid={} why={} pos={pos} fwd={fwd} det={det:+.3} ortho_err={oe:.1e} fov={fov:.4}({:.1}deg) aspect={aspect:.4} near={near:.4} far={far:.1} cam_chest={dist:.2}m close_ok={}",
                self.world_ticks,
                self.cfg.gameplay.camera_source.label(),
                self.cam_valid as u8,
                why.label(),
                fov.to_degrees(),
                close_ok as u8
            );
        }
    }

    /// Steps the viewmodel animation (M3); the state is published in the snapshot for the present thread.
    fn step_viewmodel(&mut self, now: f64, mode_on: bool, in_world: bool, menu_open: bool, ads_held: bool) {
        if self.vm.is_none() {
            match self.ars.as_ref() {
                // M6: the gun in hand's clips at its weapon times (VmParams::from_gun_assets)
                Some(a) => {
                    if let Some(p) = a.vm_params(a.cur, &self.cfg) {
                        let mut vm = VmAnim::new(p);
                        vm.set_gun(a.cur as u16);
                        self.vm = Some(vm);
                        log_i!("VM", "animation ready gun={} (arsenal slot {})", a.cur_gun().name, a.cur);
                    }
                }
                None => {
                    if let Some(a) = viewmodel::assets() {
                        let v = &self.cfg.viewmodel;
                        self.vm = Some(VmAnim::new(VmParams::from_assets(&a, v.crossfade_s, v.sprint_speed_mps)));
                        log_i!("VM", "animation ready ({} clips)", a.clips.len());
                    }
                }
            }
        }
        let qt = self.quick_times();
        let Some(vm) = self.vm.as_mut() else { return };
        let inputs = VmInputs {
            now,
            mode_on,
            in_world,
            menu_open,
            ads_held,
            planar_speed_mps: self.planar_speed,
            events: &self.vm_events,
            // M8: `vm = "sprint"`: the sprint pose stands in for the mantle's quick drop
            sprinting: if self.mv.mantle_vm_sprint { Some(true) } else { self.mv.sprinting },
            // M8: its own input (never the weapon-switch path): mantle rows >= 33 in and ladders
            lowered: self.wgate.lowered() && !self.mv.mantle_vm_sprint,
            quick_drop_s: qt.0,
            quick_raise_s: qt.1,
        };
        if let Some(t) = vm.step(&inputs) {
            let st = vm.state();
            log_i!(
                "VM",
                "state {}->{} clip={} rate={:.3} ads={:.2} seq={}",
                t.from.label(),
                t.to.label(),
                st.base.clip.name(),
                st.base.rate,
                st.ads_at(now),
                st.seq
            );
        }
        self.vm_state = vm.state();
    }

    #[allow(clippy::too_many_arguments)]
    fn publish(&self, now: f64, in_world: bool, mode_on: bool, menu_is_open: bool, foreground: bool, chars_list: &[ChrRead], player: Option<&PlayerSample>) {
        let chars = chars_list.len();
        let mut ac130 = self.ac130.hud(now, &self.cfg);
        ac130.markers = self.ac130.markers(chars_list, player, &self.cfg);
        let (mag, reserve, reloading, progress) = self.sim.as_ref().map_or((0, 0, false, 0.0), |s| {
            (s.mag(), s.reserve(), s.is_reloading(), s.reload_progress(now).unwrap_or(0.0))
        });
        // M6: the gun, the switch list, the lock and the projectiles
        let (gun_name, gun_tag, switch_names, switch_until, low, reticle) = match self.ars.as_ref() {
            Some(a) => (a.spec().display.clone(), a.fire_tag(), a.switch_names(), a.switch_until, a.spec().low_ammo_count(), a.spec().reticle_side),
            None => (
                String::new(),
                "",
                [String::new(), String::new(), String::new()],
                f64::NEG_INFINITY,
                self.data.as_ref().map_or(0, |d| d.weapon.low_ammo_count()),
                self.data.as_ref().map_or(8.0, |d| d.weapon.reticle_side_size),
            ),
        };
        let (cam_pos, cam_rows, er_nf) = match read_camera(self.cfg.gameplay.camera_source) {
            Some(c) => ([c.pos.x, c.pos.y, c.pos.z], c.rows, (c.near, c.far)),
            None => ([0.0; 3], [[0.0; 3]; 3], (0.0, 0.0)),
        };
        // review 2026-10-24 (`[coop] tracer_occlusion`): a streak whose head the map hides from the HUD's camera is not
        // drawn (ImGui has no depth test); one map ray per projectile, at most 8; no physics world = drawn as before
        let streak_eye = V3::new(cam_pos[0], cam_pos[1], cam_pos[2]);
        let streak_rays = self.cfg.coop.tracer_occlusion && cam_pos != [0.0; 3] && havok::phys_world() != 0;
        let projectiles: Vec<([f32; 3], [f32; 3])> = self.ars.as_ref().map_or(Vec::new(), |a| {
            a.projs
                .iter()
                .enumerate()
                .filter(|(k, p)| {
                    let Some(pl) = player.filter(|_| streak_rays && *k < 8) else { return true };
                    let vis = mw2er_core::tracer_occl::point_visible(streak_eye, p.pos, mw2er_core::tracer_occl::HIT_TOL_M, &mut |o, d, r| {
                        havok::map_ray(o, pl.chr.pos_w, d, r, pl.player_ins)
                    });
                    if !vis {
                        crate::coop_vis::PROJ_HIDDEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    vis
                })
                .map(|(_, p)| {
                    let tail = p.pos - p.vel * 0.04;
                    ([p.pos.x, p.pos.y, p.pos.z], [tail.x, tail.y, tail.z])
                })
                .collect()
        });
        // C6 depth 5: ER's camera planes, probe P-S2's chest pixel (our Spine2 bone in ER's camera, third person) and the
        // camera turn
        crate::er_depth_state::note_camera(er_nf.0, er_nf.1, &self.cfg.coop);
        crate::er_depth_state::publish_chest(
            if self.coopvis_off || !in_world || self.fp_eye.is_some() { None } else { self.coopvis.self_chest },
            cam_pos,
            cam_rows,
            self.last_fov_raw,
            self.cfg.hud.fallback_vfov_deg,
        );
        let spread_now = match (self.ars.as_ref(), self.spread_p) {
            (Some(a), Some(sp)) => ads_cone(self.spread_deg(&sp), a.spec().spread.ads_spread, self.vm_state.ads_at(now)),
            (None, Some(sp)) => self.spread_deg(&sp),
            _ => 0.0,
        };
        let snap = HudSnapshot {
            weapon_lowered: self.wgate.blocked(now),
            // C6 aim 4: ER's crossbow aim in third person (no scope): a small centre dot instead of the faded crosshair
            // C6 part 17: the guard ADS keeps the MW2 crosshair too (third person only)
            tp_aim_dot: (self.stance.aim_on() || self.stance.guard_active()) && self.fp_eye.is_none(),
            fp_on: self.fp_eye.is_some(),
            vm_draw_tp: self.cfg.viewmodel.draw_in_third_person,
            t: now,
            in_world,
            mode_on,
            menu_open: menu_is_open,
            foreground,
            mag,
            reserve,
            low_ammo_count: low,
            reloading,
            reload_progress: progress,
            spread_deg: spread_now,
            reticle_side: reticle,
            vfov_raw: self.last_fov_raw,
            last_hit_t: self.last_hit_t,
            last_hit_kill: self.last_hit_kill,
            shots: self.shots,
            hits: self.hits,
            chars: chars as u32,
            vm: self.vm_state,
            ads_frac: self.vm_state.ads_at(now),
            cam_valid: self.cam_valid,
            ac130,
            stance: if self.mv.driving { self.mv.stance } else { 255 },
            vm_offset_in: {
                // M6 gun kick: the viewmodel pushed back along the barrel (renderer x = forward)
                let back = self.ars.as_ref().map_or(0.0, |a| a.gunkick.back_in);
                let base = if self.mv.driving { self.mv.vm_offset_in } else { [0.0; 3] };
                [base[0] - back, base[1], base[2]]
            },
            gun_name,
            gun_tag,
            switch_names,
            switch_until,
            scope: self.scope,
            lock: self.lock_ui.0,
            lock_target: self.lock_ui.1.map(|v| [v.x, v.y, v.z]),
            lock_point: self.lock_ui.2,
            cam_pos,
            cam_rows,
            projectiles,
            nuke: self.nuke.hud(now, &self.cfg, mode_on, self.ac130_was_active),
            ui_yield: self.ui_yield_out.0,
            ui_yield_why: self.ui_yield_out.1,
            ui_cut: self.ui_cut.0,
            ui_cut_n: self.ui_cut.1 as u8,
            dmg_nums: self.dmg.to_vec(),
            view_pos: self.view_pos,
            er_tag_handles: if self.cfg.hud.damage_numbers && self.cfg.hud.damage_numbers_suppress_with_er_tag && !self.dmg.is_empty() {
                crate::game::c3_read::read_enemy_tag_handles()
            } else {
                [0; 8]
            },
            peer_segs: if self.coopvis_off { Vec::new() } else { self.coopvis.segments(now, &self.cfg) },
            peer_flashes: if self.coopvis_off { Vec::new() } else { self.coopvis.flashes(now, &self.cfg) },
            world_guns: if self.coopvis_off || !in_world { Vec::new() } else { self.coopvis.world_guns.clone() },
            world_occl: if self.coopvis_off || !in_world { Vec::new() } else { self.coopvis.world_occl.clone() },
            er_near: if er_nf.0.is_finite() { er_nf.0 } else { 0.0 },
            er_far: if er_nf.1.is_finite() { er_nf.1 } else { 0.0 },
        };
        *shared::lock(&SNAPSHOT) = snap;
    }
}

// ---- C3: the UI yield and the damage numbers (docs/C3_DESIGN.md part 2) ------------------------------------------

impl TaskState {
    /// One UI-yield tick: ER's message signals (non-faulting reads) -> `UiYield` -> `ui_yield_out` + transition lines.
    fn ui_yield_tick(&mut self, now: f64, active: bool, ui_mask: u128, fire_or_ads: bool) {
        use mw2er_core::ui_yield::{SIG_INDEX, YieldEvent, cut_bits, cutout_rects, index_bits, why_label};
        let y = self.cfg.ui_yield.clone();
        if !y.enabled || y.mode == "off" || !active {
            if self.ui_yield.frac() > 0.0 {
                log_i!("UI", "yield off dur=? why=inactive");
            }
            self.ui_yield.reset();
            self.ui_yield_out = (0.0, 0);
            self.ui_cut.1 = 0;
            if self.ui_cut_log.0 != 0 {
                log_i!("UI", "cutout off why=inactive");
                self.ui_cut_log.0 = 0;
            }
            return;
        }
        let sig = crate::game::c3_read::read_ui_signals(y.use_popup, y.use_announce, y.use_status, y.use_summon, y.use_blink);
        let mut bits = sig.bits;
        if index_bits(ui_mask, &y.indices) {
            bits |= SIG_INDEX;
        }
        if y.learn {
            self.ui_learn_tick(now, ui_mask, bits);
        }
        let out = self.ui_yield.update(now, bits, fire_or_ads, &y.params());
        match out.event {
            Some(YieldEvent::On { why }) => {
                let idx: Vec<u32> = y.indices.iter().copied().filter(|&i| i < 128 && ui_mask & (1u128 << i) != 0).collect();
                // review C3: the C2 toast candidates visible right now (pairs the yield with the MENU idx lines)
                let vis: Vec<u32> = UI_WATCH_IDX.iter().copied().filter(|&i| ui_mask & (1u128 << i) != 0).collect();
                log_i!(
                    "UI",
                    "yield on reason={} idx={:?} vis={:?} popups={} announce={} status={}/{} mode={}",
                    why_label(why),
                    idx,
                    vis,
                    sig.popups,
                    sig.announce,
                    sig.status_rw.0,
                    sig.status_rw.1,
                    y.mode
                );
            }
            Some(YieldEvent::Off { dur, cancelled }) => {
                log_i!("UI", "yield off dur={dur:.2}s why={}", if cancelled { "fire_or_ads" } else { "clear" });
            }
            Some(YieldEvent::Stuck { bit }) => {
                log_w!("UI", "WARN yield signal stuck bit={} for {:.0} s: ignored until it clears", why_label(bit), y.stuck_s);
            }
            None => {}
        }
        self.ui_yield_out = (out.frac, out.why);
        // C5 `mode = "cutout"`: the message regions at the yield's own timing (render-only: firing / ADS still cancel
        // the yield, which brings the whole gun back)
        self.ui_cut = if y.cutout() { cutout_rects(out.frac, &out.bit_frac, &y.cutout_regions()) } else { (Default::default(), 0) };
        let (rects, n) = self.ui_cut;
        let bits = cut_bits(&rects, n);
        // one line when the cut regions' signal set changes, at most one per second (a flickering prompt stays quiet)
        if bits != self.ui_cut_log.0 && (bits == 0 || now - self.ui_cut_log.1 >= 1.0) {
            if bits != 0 {
                let list: Vec<String> = rects[..n]
                    .iter()
                    .map(|c| format!("{}:[{:.2},{:.2},{:.2},{:.2}]@{:.2}", why_label(c.bit), c.rect[0], c.rect[1], c.rect[2], c.rect[3], c.strength))
                    .collect();
                log_i!("UI", "cutout on signal={} regions={} {} feather={:.3}", why_label(bits), n, list.join(" "), y.cutout_feather);
                self.ui_cut_log.1 = now;
            } else {
                log_i!("UI", "cutout off");
            }
            self.ui_cut_log.0 = bits;
        }
    }

    /// `[ui_yield] learn`: `UI learn idx=<n> open=<k> med=<s> max=<s> with=<signal bits>` every 60 s.
    fn ui_learn_tick(&mut self, now: f64, mask: u128, bits: u32) {
        let l = &mut self.ui_learn;
        if l.t0 == 0.0 {
            l.t0 = now;
        }
        for i in 0..128usize {
            let open = mask & (1u128 << i) != 0;
            match (open, l.open_since[i]) {
                (true, None) => l.open_since[i] = Some(now),
                (false, Some(t)) => {
                    l.open_since[i] = None;
                    if l.durs.len() < 4096 {
                        l.durs.push((i as u8, now - t, bits));
                    }
                }
                _ => {}
            }
        }
        if now - l.t0 < 60.0 {
            return;
        }
        l.t0 = now;
        let mut by: std::collections::BTreeMap<u8, (Vec<f64>, u32)> = std::collections::BTreeMap::new();
        for (i, d, b) in l.durs.drain(..) {
            let e = by.entry(i).or_default();
            e.0.push(d);
            e.1 |= b;
        }
        for (i, (mut ds, b)) in by {
            ds.sort_by(|a, c| a.total_cmp(c));
            log_i!(
                "UI",
                "learn idx={i} open={} med={:.2} max={:.2} with={}",
                ds.len(),
                ds[ds.len() / 2],
                ds[ds.len() - 1],
                mw2er_core::ui_yield::why_label(b)
            );
        }
    }

    /// C3 (docs/C3_DESIGN.md 2.6): one of our attributed hits (gun, launcher, AC-130): the counters every hit site
    /// keeps, plus a damage number at the impact (or above the character). Never called for an HPDROP.
    #[allow(clippy::too_many_arguments)]
    fn on_hit(&mut self, now: f64, handle: u64, chr_pos: Option<V3>, dmg: i32, kill: bool, src: mw2er_core::dmgnum::DmgSrc, impact: Option<[f32; 3]>, mw2_dmg: Option<i32>) {
        self.hits += 1;
        self.last_hit_t = now;
        self.last_hit_kill = kill;
        let h = &self.cfg.hud;
        if !h.damage_numbers {
            return;
        }
        let Some(cp) = chr_pos.map(|p| [p.x, p.y, p.z]).or(impact) else { return };
        let cap = self.cfg.hit.capsule_height_m;
        let (at, via) = mw2er_core::dmgnum::anchor(impact, cp, cap);
        let head = h.damage_numbers_head_guess && chr_pos.is_some() && mw2er_core::dmgnum::head_guess(impact, cp, cap);
        let value = if h.damage_numbers_value == "mw2" { mw2_dmg.filter(|d| *d > 0).unwrap_or(dmg) } else { dmg };
        let merged = self.dmg.push(now, handle, at, value, kill, head, src, h.damage_numbers_merge_s as f64, h.damage_numbers_max as usize);
        log_i!(
            "DMGNUM",
            "n={} handle={} dmg={value} kill={} head={} via={} src={}{}",
            self.dmg.len(),
            logfmt::handle(handle),
            kill as u8,
            head as u8,
            via.label(),
            src.label(),
            if merged { " merge=1" } else { "" }
        );
    }
}

// ---- M6: the arsenal (docs/M6_DESIGN.md) -----------------------------------------------------------------------

impl TaskState {
    /// The gun in hand (the M2-M5 weapon name without the arsenal).
    fn gun_name(&self) -> String {
        match self.ars.as_ref() {
            Some(a) => a.cur_gun().name.clone(),
            None => self.data.as_ref().map_or("?".to_string(), |d| d.weapon.name.clone()),
        }
    }

    /// Puts gun `i` in hand: the old gun's sim / spread are parked (ammo kept), gun i's are taken (a fresh full gun
    /// the first time), its spread / falloff / movement class / viewmodel params apply. `immediate`: no switch anim.
    fn select_gun(&mut self, i: usize, immediate: bool) {
        let cfg = self.cfg.clone();
        let Some(a) = self.ars.as_mut() else { return };
        if i >= a.data.guns.len() {
            return;
        }
        let old = a.cur;
        if i != old {
            // M7 A3: the outgoing gun is never parked mid-reload / mid-rechamber (run 14 parked the stinger, rpg,
            // m79, at4 and cheytac with a reload that restarted during the drop)
            let mut ev = Vec::new();
            if let Some(sim) = self.sim.as_mut() {
                sim.cancel_reload(&mut ev);
            }
            if ev.contains(&SimEvent::ReloadCancelled) {
                log_i!("RELOAD", "cancelled (park) gun={}", a.data.guns[old].name);
            }
            if ev.contains(&SimEvent::RechamberCancelled) {
                log_i!("RECHAMBER", "cancelled gun={} reason=switch", a.data.guns[old].name);
            }
            a.parked[old].sim = self.sim.take();
            a.parked[old].spread = std::mem::take(&mut self.spread);
            a.cur = i;
            let sim = a.parked[i].sim.take().unwrap_or_else(|| a.new_sim(i, &cfg));
            self.sim = Some(sim);
            self.spread = std::mem::take(&mut a.parked[i].spread);
        }
        self.spread_p = Some(a.spread_params(i));
        self.falloff = Some(a.spec().falloff);
        self.stance_spread = a.stance_spread(i);
        a.publish_move_class(i);
        if let Some(vm) = self.vm.as_mut() {
            if let Some(p) = a.vm_params(i, &cfg) {
                vm.set_params(p);
            }
            vm.set_gun(i as u16);
        }
        if immediate {
            a.sw.force(i);
        }
    }

    fn on_switch_event(&mut self, e: SwitchEvent) {
        match e {
            SwitchEvent::Start { from, .. } | SwitchEvent::Reversed { from, .. } => {
                // section 5.2: a reload / rechamber / segmented reload in progress is cancelled (shells kept); M7 A3: a
                // rechamber cancel is logged too, and the sim stepped during the drop (hold) never restarts it
                self.cancel_sim("switch");
                self.vm_events.push(VmEvent::SwitchDrop);
                if let Some(a) = self.ars.as_mut() {
                    a.reset_lock();
                    a.play_role(from, "putaway");
                }
            }
            SwitchEvent::RaiseStart { gun, first, .. } => {
                self.select_gun(gun, false);
                self.vm_events.push(VmEvent::SwitchRaise { gun: gun as u16, first });
                if let Some(a) = self.ars.as_ref() {
                    let role = if first && a.gun(gun).roles.contains_key("first_raise") { "first_raise" } else { "raise" };
                    a.play_role(gun, role);
                }
            }
            SwitchEvent::Raised { .. } | SwitchEvent::Retarget { .. } | SwitchEvent::Blocked(_) | SwitchEvent::Waiting => {}
        }
    }

    /// Per tick after the viewmodel: notetrack sounds, view effects, lock-on, projectiles, owner retry, pool line.
    fn arsenal_tick(&mut self, now: f64, dt: f32, live: bool, cam: Option<CameraSample>, player: Option<&PlayerSample>, chars: &[ChrRead]) {
        let cfg = self.cfg.clone();
        let vm = self.vm_state;
        let ads = vm.ads_at(now);
        // 2026-10-20: the pad's sprint (L3) holds breath like Shift
        let sprint_key = (shared::RAW_KEYS.load(Ordering::Relaxed) | crate::pad::move_levels()) & (1 << mw2er_core::move_input::PRESS_SPRINT) != 0;
        let Some(a) = self.ars.as_mut() else { return };
        a.play_notes(&vm, now);
        let scoped = live && a.spec().ads.overlay.is_some() && ads >= 0.999 && vm.phase != mw2er_core::vm_anim::VmPhase::Rechamber;
        let (pitch, yaw, ratio) = a.view_fx(&cfg, now, dt, ads, scoped, sprint_key);
        // M7: the nuke's camera shake rides on the view-fx angles (degrees); review C3: so does a peer's 105 mm impact
        let peer = if self.coopvis_off { mw2er_core::ac130_shake::ShakeOut::ZERO } else { self.coopvis.shake(now, &cfg) };
        let (pitch, yaw) = (pitch + self.nuke_shake.pitch_deg + peer.pitch_deg, yaw + self.nuke_shake.yaw_deg + peer.yaw_deg);
        // the RPG's rocket part: hidden while the magazine is empty (shown again reloadShowRocketTime into the reload)
        let mag = self.sim.as_ref().map_or(1, |s| s.mag());
        let reload_t = self.sim.as_ref().and_then(|s| s.reload_progress(now));
        let show_rocket_s = a.spec().show_rocket_s;
        let reload_s = match a.spec().reload {
            mw2er_core::arsenal::ReloadKind::Mag { reload_s, .. } => reload_s,
            _ => 1.0,
        };
        let hide_rocket = a.spec().has_rocket_part && mag == 0 && reload_t.is_none_or(|f| (f as f64) * reload_s < show_rocket_s);
        if let Some(v) = self.vm.as_mut() {
            v.set_mag_empty(mag == 0);
            v.set_hide_rocket(hide_rocket);
        }
        self.scope = if scoped && cfg.viewmodel.enabled { 1.0 } else { 0.0 };
        // view effects for the camera-step detour (third person only with [kick] camera_tp)
        let fx_on = live && (cfg.kick.camera_tp || self.fp_eye.is_some());
        shared::VIEWFX_ON.store(fx_on, Ordering::Relaxed);
        shared::VIEWFX_PITCH_BITS.store(if fx_on { pitch } else { 0.0 }.to_bits(), Ordering::Relaxed);
        shared::VIEWFX_YAW_BITS.store(if fx_on { yaw } else { 0.0 }.to_bits(), Ordering::Relaxed);
        shared::VIEWFX_FOV_RATIO_BITS.store(if fx_on { ratio } else { 1.0 }.to_bits(), Ordering::Relaxed);
        shared::SENS_SCALE_BITS.store(if fx_on && cfg.arsenal.ads_sens_scale { ratio } else { 1.0 }.to_bits(), Ordering::Relaxed);
        if fx_on && let Some((addr, _)) = crate::game::camera::read_pers_cam1_17() {
            cam_hook::VIEWFX_CAM.store(addr, Ordering::Relaxed);
            cam_hook::VIEWFX_CSCAMERA.store(crate::game::fp_read::cscamera_addr(), Ordering::Relaxed);
        }
        // lock-on (Stinger / Javelin, `[launchers] lock`)
        self.lock_ui = (0, None, false);
        let lk = a.spec().projectile.map(|p| p.lock);
        if cfg.launchers.lock
            && matches!(lk, Some(LockKind::Required | LockKind::RequiredTopAttack))
            && let (Some(c), Some(p)) = (cam, player)
        {
            let ads_ok = live && ads >= 0.95;
            let fwd = c.row2.normalize().unwrap_or(c.row2);
            let vfov = if c.fov_raw.is_finite() && c.fov_raw > 0.05 { c.fov_raw } else { 0.8 };
            // the MW2 reticle circle (65 of 240 half-height) [V gsc], scaled to this fov
            let cone = ((vfov * 0.5).tan() * (65.0 / 240.0)).atan();
            let mut cand: Option<(u64, V3, f32)> = None;
            let mut aim: Option<(V3, f32, Option<u64>)> = None;
            for ch in chars {
                if ch.handle == p.chr.handle || ch.hp <= 0 || cfg.hit.exclude_character_ids.contains(&ch.character_id) {
                    continue;
                }
                let chest = ch.pos + V3::new(0.0, cfg.aim.chest_height_m, 0.0);
                let d = chest - c.pos;
                let dist = d.len();
                if !(dist > 1.0 && dist < cfg.launchers.max_ray_m) {
                    continue;
                }
                let ang = fwd.angle_deg(d).to_radians();
                if ang <= cone && cand.is_none_or(|(_, _, a)| ang < a) {
                    let los = d.normalize().map(|dn| havok::map_ray(c.pos, p.chr.pos_w, dn, dist, p.player_ins).is_none_or(|t| t >= dist - 0.75));
                    if los == Some(true) {
                        cand = Some((ch.handle, chest, ang));
                    }
                }
                // the Javelin follows a character on the aim line (within the aim's character radius)
                let along = d.dot(fwd);
                if along > 0.0 && (d - fwd * along).len() <= cfg.aim.char_radius_m && aim.is_none_or(|(_, t, _)| along < t) {
                    aim = Some((chest, along, Some(ch.handle)));
                }
            }
            if aim.is_none()
                && let Some(t) = havok::map_ray(c.pos, p.chr.pos_w, fwd, cfg.launchers.max_ray_m, p.player_ins)
            {
                aim = Some((c.pos + fwd * t, t, None));
            }
            let (st, pt) = a.lock_tick(now, ads_ok, cand.map(|(h, ch, _)| (h, ch)), aim);
            self.lock_ui = (st, pt, lk == Some(LockKind::RequiredTopAttack));
        } else if lk.is_some() {
            a.reset_lock();
        }
        if now - a.lock_refused_t < 1.0 && self.lock_ui.0 == 0 {
            self.lock_ui.0 = 3;
        }
        // projectiles
        if let Some(p) = player {
            self.tick_projectiles(now, dt, p, chars);
        }
        // bug 2: the owner retry and the periodic pool line
        if let (Some((id, o, d)), Some(p)) = (self.retry.take(), player) {
            let ok = spawn_guard::admit(now, SpawnKind::Gun, id) && spawn::spawn(p.field_ins_handle, id, o, p.chr.pos_w, d).is_ok();
            if ok {
                self.spawns_world += 1;
            }
            log_i!("SPAWN", "retry cause=owner ok={} id={id} owner={}", ok as u8, logfmt::handle(p.chr.handle));
        }
        let every = cfg.arsenal.pool_log_every.max(1);
        if let Some(p) = player
            && (self.pool_logged_at == u32::MAX || self.spawns_world >= self.pool_logged_at.saturating_add(every))
        {
            self.pool_logged_at = self.spawns_world;
            if let Some(ps) = bullets::pool_snapshot(p.chr.handle, &self.our_ids, self.spawns_world) {
                log_i!("SPAWN", "pool {}", pool_fields(&ps));
            }
            // M7 A1: the read-only emitter walk next to every pool line (and once on world entry)
            spawn_guard::log_walk(p.chr.handle, &self.our_ids);
        }
    }

    /// Launcher projectiles: one step each, one map ray + the characters' capsules along the step (section 6.6).
    fn tick_projectiles(&mut self, now: f64, dt: f32, p: &PlayerSample, chars: &[ChrRead]) {
        let cfg = self.cfg.clone();
        let Some(a) = self.ars.as_mut() else { return };
        // review M6: player HP 0.6 s after each launcher explosion (the analyzer FAILs on a loss next to a blast)
        let mut i = 0;
        while i < a.blast_hp.len() {
            let (t, hp0, gun, id) = a.blast_hp[i];
            if now - t >= 0.6 {
                log_i!("PROJ", "selfcheck gun={} id={id} hp_before={hp0} hp_after={} lost={}", a.gun(gun).name, p.chr.hp, (hp0 - p.chr.hp).max(0));
                a.blast_hp.remove(i);
            } else {
                i += 1;
            }
        }
        if a.projs.is_empty() {
            return;
        }
        // review M6: projectiles launched in another havok frame (pos_w changed) are expired, never exploded here
        if a.proj_w.is_some_and(|w| w != p.chr.pos_w) {
            a.expire_projectiles("frame");
            return;
        }
        let feet: Vec<V3> = chars
            .iter()
            .filter(|c| c.handle != p.chr.handle && c.hp > 0 && !cfg.hit.exclude_character_ids.contains(&c.character_id))
            .map(|c| c.pos)
            .collect();
        let handles: Vec<u64> = chars
            .iter()
            .filter(|c| c.handle != p.chr.handle && c.hp > 0 && !cfg.hit.exclude_character_ids.contains(&c.character_id))
            .map(|c| c.handle)
            .collect();
        let locked = a.lock.locked();
        let mut out: Vec<(usize, V3, V3, &'static str, bool)> = Vec::new();
        for (k, pr) in a.projs.iter_mut().enumerate() {
            // guided rockets follow a locked character's chest
            if let (Guidance::Pn { .. }, Some(h)) = (pr.guidance, locked)
                && let Some(c) = chars.iter().find(|c| c.handle == h)
            {
                pr.guidance = Guidance::Pn { target: c.pos + V3::new(0.0, cfg.aim.chest_height_m, 0.0) };
            }
            let st = pr.step(dt);
            let seg = st.to - st.from;
            let len = seg.len();
            if len > 1e-4
                && let Some(dir) = seg.normalize()
            {
                let map_t = havok::map_ray(st.from, p.chr.pos_w, dir, len, p.player_ins);
                let hit = mw2er_core::ac130_aim::first_hit(st.from, dir, map_t, &feet, cfg.aim.char_radius_m * 0.6, cfg.hit.capsule_height_m, len);
                match hit.via {
                    mw2er_core::ac130_aim::Via::None => {}
                    mw2er_core::ac130_aim::Via::Map => {
                        out.push((k, st.from + dir * hit.t, dir, "map", false));
                        continue;
                    }
                    mw2er_core::ac130_aim::Via::Chr(i) => {
                        let _ = handles.get(i);
                        out.push((k, st.from + dir * hit.t, dir, "chr", true));
                        continue;
                    }
                }
            }
            if st.expired {
                out.push((k, pr.pos, pr.vel.normalize().unwrap_or(V3::new(0.0, 0.0, 1.0)), "expire", false));
            }
        }
        let mut remove = Vec::new();
        for (k, at, dir, kind, chr) in out {
            let pr = a.projs[k];
            remove.push(k);
            let d = (at - pr.start).len();
            if kind == "expire" {
                log_i!("PROJ", "hit kind=expire gun={} id={} d={d:.1} t={:.2}", a.gun(pr.gun).name, pr.id, pr.t);
                continue;
            }
            let armed = pr.armed();
            log_i!("PROJ", "hit kind={} gun={} id={} d={d:.1} t={:.2} armed={}", if armed { kind } else { "dud" }, a.gun(pr.gun).name, pr.id, pr.t, armed as u8);
            let Some(l) = a.gun(pr.gun).rows.launcher else { continue };
            let mut ids = Vec::new();
            // a direct character hit: the impact row (MW2 1000 / the M79's 135) [I]
            if chr
                && (cfg.launchers.rocket_impact || !armed)
                && spawn_guard::admit(now, SpawnKind::Launcher, l.impact.id)
                && spawn::spawn(p.field_ins_handle, l.impact.id, at - dir * 0.3, p.chr.pos_w, dir).is_ok()
            {
                ids.push(l.impact.id.to_string());
            }
            if armed {
                let tiers: Vec<(u8, mw2er_core::rows::ArsenalExplRow)> = match (cfg.launchers.explosion_tiers, l.core, l.ring) {
                    (2, Some(c), Some(r)) => vec![(0, c), (1, r)],
                    _ => vec![(2, l.single)],
                };
                // review M6 self guard: ER's handling of selfTarget 0 on these cloned rows is not verified; a blast
                // whose sphere reaches the player's capsule is a dud
                let r_max = tiers.iter().map(|(_, r)| r.radius_m as f32).fold(0.0f32, f32::max);
                if cfg.launchers.self_guard
                    && let Some(dp) = self_blast_distance(p.chr.pos, cfg.hit.capsule_height_m, at, r_max, cfg.launchers.self_guard_margin_m)
                {
                    log_w!("PROJ", "hit kind=dud gun={} id={} why=self_radius d_player={dp:.1}m radius={r_max:.1}m", a.gun(pr.gun).name, pr.id);
                    self.spawns_world += ids.len() as u32;
                    continue;
                }
                a.blast_hp.push((now, p.chr.hp, pr.gun, pr.id));
                for (tier, r) in tiers {
                    // M7 watchdog: the ring tier is low priority (dropped first when the emitter pool is filling)
                    let kind = if tier == 1 { SpawnKind::Ring } else { SpawnKind::Launcher };
                    if !spawn_guard::admit(now, kind, r.id) {
                        continue;
                    }
                    match spawn::spawn(p.field_ins_handle, r.id, at - dir * 0.3, p.chr.pos_w, dir) {
                        Ok(_) => {
                            ids.push(r.id.to_string());
                            a.blasts.push(LBlast { t: now, pos: at, radius_m: r.radius_m as f32, gun: pr.gun, tier });
                        }
                        Err(e) => {
                            // M7: every failure names its cause
                            let pool = bullets::pool_snapshot(p.chr.handle, &self.our_ids, self.spawns_world);
                            log_e!("PROJ", "explode gun={} id={} SPAWN FAIL {e} cause={}", a.gun(pr.gun).name, r.id, classify(pool.as_ref(), None).label())
                        }
                    }
                }
                // C3 PASS C: peers see / hear the explosion (a no-attack synced twin of the first tier)
                if let Some(first) = ids.iter().find_map(|s| s.parse::<i32>().ok()).filter(|id| *id != l.impact.id)
                    && !self.coopvis_off
                {
                    let peers = self.coop.peers();
                    self.coopvis.send_twin(now, &cfg, peers, p, first, at - dir * 0.3, dir);
                }
                // the AC-130 40 mm explosion sound (MW2's streamed launcher explosion is not in the dumps)
                if let Some(d) = crate::data::AC_DATA.get()
                    && let Some(c) = d.sounds.cues.get(mw2er_core::ac130_audio::Cue::Exp40.key())
                {
                    let mut rng = mw2er_core::rng::Rng::new(pr.id as u64 ^ 0x5eed);
                    shared::play_ac(mw2er_core::ac130_audio::pick(mw2er_core::ac130_audio::Cue::Exp40, c, &mut rng, cfg.ac130.audio.explosion_gain, 0));
                }
                log_i!("PROJ", "explode gun={} tiers={} ids={} at={}", a.gun(pr.gun).name, if l.core.is_some() && cfg.launchers.explosion_tiers == 2 { "core+ring" } else { "single" }, ids.join("+"), logfmt::v3(at));
            }
            self.spawns_world += ids.len() as u32;
        }
        remove.sort_unstable();
        remove.dedup();
        for k in remove.into_iter().rev() {
            a.projs.remove(k);
        }
        a.blasts.retain(|b| now - b.t <= 3.0);
    }

    /// The shot pipeline of an arsenal gun (section 4.4): sound, aim (the kicked camera), cone (hip -> ADS spread),
    /// hitscan pellets with the pool gate and the per-shot breaker, or a launcher projectile.
    fn fire_arsenal(&mut self, now: f64, p: &PlayerSample, cam: Option<CameraSample>, chars: &[ChrRead], mag_after: u32) {
        let cfg = self.cfg.clone();
        let Some(a) = self.ars.as_ref() else { return };
        let gun = a.cur;
        let spec = a.spec().clone();
        let rows = a.cur_gun().rows.clone();
        let gname = a.cur_gun().name.clone();
        let role = if mag_after == 0 && a.cur_gun().roles.contains_key("last_shot") { "last_shot" } else { "fire" };
        a.play_role(gun, role);
        let (Some(sp), Some(falloff)) = (self.spread_p, self.falloff) else { return };
        let reserve = self.sim.as_ref().map_or(0, |s| s.reserve());
        let Some(c) = cam else {
            log_w!("SHOT", "n={} skipped: no camera", self.shots);
            return;
        };
        let fwd = if cfg.aim.camera_gate {
            if !self.cam_valid {
                log_w!("SHOT", "n={} skipped: camera settling ({})", self.shots, self.cam_why.label());
                return;
            }
            view_forward(c.row2, 1.0)
        } else {
            self.cam_sign.sign().and_then(|s| view_forward(c.row2, s))
        };
        let Some(fwd) = fwd else {
            log_w!("SHOT", "n={} skipped: camera direction unresolved", self.shots);
            return;
        };
        if let Err(e) = owner_check(p.field_ins_handle) {
            log_w!("SHOT", "n={} skipped: {e}", self.shots);
            return;
        }
        let chest = self.chest(p);
        let origin = bullet_origin(self.fp_eye.unwrap_or(c.pos), fwd, chest, cfg.aim.origin_ahead_m);
        let ads = self.vm_state.ads_at(now);
        let hip = self.spread_deg(&sp);
        let cone = if cfg.gameplay.apply_spread_to_bullets { ads_cone(hip, spec.spread.ads_spread, ads) } else { 0.0 };
        if let Err(r) = check_shot_fp(c.pos, chest, origin, fwd, cfg.aim.origin_ahead_m, cfg.aim.max_range_m, self.fp_eye.is_some()) {
            log_w!("SHOT", "n={} skipped: {} origin={} dir={}", self.shots, r.reason(), logfmt::v3(origin), logfmt::dir3(fwd));
            return;
        }
        let (kp, ky) = self.ars.as_ref().map_or((0.0, 0.0), |a| a.kick.angles());
        if let Some(a) = self.ars.as_mut() {
            if cfg.kick.enabled {
                a.kick.on_shot(&spec.kick, ads, cfg.kick.scale, &mut self.rng);
            }
            if cfg.kick.gun_kick {
                a.gunkick.on_shot(&spec.kick, ads, cfg.kick.gun_scale, &mut self.rng);
            }
        }
        self.spread.on_shot(&sp);

        // launchers: a projectile flown by the DLL (section 6.6)
        if let Some(pj) = spec.projectile {
            let dir = match basis(fwd) {
                Some((r, u)) => perturb(fwd, r, u, cone, &mut self.rng),
                None => fwd,
            };
            let Some(a) = self.ars.as_mut() else { return };
            if a.projs.len() >= cfg.launchers.max_live as usize {
                let old = a.projs.remove(0);
                log_i!("PROJ", "hit kind=expire gun={} id={} (max_live {})", a.gun(old.gun).name, old.id, cfg.launchers.max_live);
            }
            let locked_chr = a.lock.locked().and_then(|h| chars.iter().find(|c| c.handle == h)).map(|c| c.pos + V3::new(0.0, cfg.aim.chest_height_m, 0.0));
            let point = a.plock.point;
            let (guidance, lock) = match (pj.lock, cfg.launchers.lock) {
                _ if pj.ballistic => (Guidance::Ballistic, "none"),
                (LockKind::Required, true) => match locked_chr {
                    Some(t) => (Guidance::Pn { target: t }, "chr"),
                    None => (Guidance::Straight, "none"),
                },
                (LockKind::RequiredTopAttack, true) => match point {
                    Some(t) => {
                        let range = (t - origin).planar_len();
                        (Guidance::TopAttack { target: t, apex_y: t.y.max(origin.y) + javelin_apex_m(range), diving: false }, "point")
                    }
                    None => (Guidance::Straight, "none"),
                },
                _ => (Guidance::Straight, "none"),
            };
            let speed = pj.speed_mps();
            let up = if pj.ballistic || cfg.launchers.rocket_speed_up == "add" { pj.speed_up_in * 0.0254 } else { 0.0 };
            let id = a.next_proj;
            a.next_proj += 1;
            let pr = Projectile::launch(id, gun, origin, dir, speed, up, pj.ignition_s, pj.lifetime_s, pj.arm_m(), pj.max_accel_mps2().max(1.0), guidance);
            // review M6: the Javelin boosts to its cruise speed after ignition [I]
            let pr = if matches!(guidance, Guidance::TopAttack { .. }) || pj.lock == LockKind::RequiredTopAttack {
                pr.with_boost(cfg.launchers.javelin_cruise_mps, cfg.launchers.javelin_accel_mps2)
            } else {
                pr
            };
            let pr = if matches!(guidance, Guidance::Ballistic) {
                // M79: upward speed added along world up (MW2 projectileSpeedUp)
                let mut q = pr;
                q.vel = dir * speed + V3::new(0.0, up, 0.0);
                q
            } else {
                pr
            };
            a.proj_w = Some(p.chr.pos_w);
            a.projs.push(pr);
            if !cfg.launchers.lock && matches!(pj.lock, LockKind::Required | LockKind::RequiredTopAttack) && id == 1 {
                log_i!("LAUNCH", "lock=off dumbfire=1 ([launchers] lock = false)");
            }
            log_i!("LAUNCH", "gun={gname} id={id} v={speed:.1} v0={:.1} cruise={:.1} lock={lock} mag={mag_after}/{reserve} origin={} dir={}", pr.vel.len(), if pr.boost_accel > 0.0 { pr.cruise } else { speed }, logfmt::v3(origin), logfmt::dir3(dir));
            self.push_shot(ShotInfo { t: now, band_id: rows.launcher.map_or(0, |l| l.impact.id), mw2: spec.falloff.damage, d: 0.0, origin, gun: gun as u16 });
            // C3 PASS C: peers hear the launcher (a no-attack twin of the impact row's launch slot, synced)
            if let Some(l) = rows.launcher
                && !self.coopvis_off
            {
                let peers = self.coop.peers();
                self.coopvis.send_twin(now, &cfg, peers, p, l.impact.id, origin, dir);
            }
            return;
        }

        // hitscan: one map ray + one chest proximity per shot (pellets share it, `pellet_bands = "shot"`)
        let chests: Vec<V3> = chars
            .iter()
            .filter(|ch| {
                ch.handle != p.chr.handle
                    && ch.hp > 0
                    && !cfg.hit.exclude_character_ids.contains(&ch.character_id)
                    && (ch.pos - p.chr.pos).len() <= cfg.aim.max_range_m
            })
            .map(|ch| ch.pos + V3::new(0.0, cfg.aim.chest_height_m, 0.0))
            .collect();
        let map_t = havok::map_ray(origin, p.chr.pos_w, fwd, cfg.aim.max_range_m, p.player_ins);
        let (dist, src) = estimate_target_distance(origin, fwd, &chests, map_t, cfg.aim.max_range_m, cfg.aim.char_radius_m);
        let mw2 = falloff.damage_at(dist as f64);
        let Some(band) = rows.band_for_damage(mw2).copied() else { return };
        let n = spec.pellets.max(1);
        let capped = spec.is_shotgun() && cfg.arsenal.shotgun_range_cap && (dist as f64) > falloff.min_damage_range_m && dist < cfg.aim.max_range_m;
        let pool = if n > 1 && cfg.arsenal.pellet_pool_gate { bullets::pool_snapshot(p.chr.handle, &self.our_ids, self.spawns_world) } else { None };
        let allowed = if n > 1 && cfg.arsenal.pellet_pool_gate { pellets_allowed(pool.as_ref(), n) } else { n };
        let mut dirs = std::mem::take(&mut self.pellets);
        if n == 1 {
            dirs.clear();
            dirs.push(match basis(fwd) {
                Some((r, u)) if cone > 0.0 => perturb(fwd, r, u, cone, &mut self.rng),
                _ => fwd,
            });
        } else {
            pellet_dirs(fwd, cone, n, &mut self.rng, &mut dirs);
        }
        let mut bands_used: Vec<i32> = Vec::with_capacity(n as usize);
        let (mut tried, mut ok) = (0u32, 0u32);
        if cfg.gameplay.spawn_bullets && !capped && !self.breaker.is_open(now) {
            for (k, d) in dirs.iter().enumerate() {
                if k as u32 >= allowed {
                    break;
                }
                let id = if cfg.arsenal.pellet_bands == "pellet" && n > 1 {
                    let (dp, _) = estimate_target_distance(origin, *d, &chests, map_t, cfg.aim.max_range_m, cfg.aim.char_radius_m);
                    rows.band_for_damage(falloff.damage_at(dp as f64)).map_or(band.id, |b| b.id)
                } else {
                    band.id
                };
                // M7 watchdog: the first pellet is a normal shot, the others are low priority
                if !spawn_guard::admit(now, if k == 0 { SpawnKind::Gun } else { SpawnKind::Pellet }, id) {
                    continue;
                }
                tried += 1;
                if !self.first_spawn_done {
                    log_i!("SPAWN", "try id={id} owner={} origin={} w={} dir={} (first spawn of the session)", logfmt::handle(p.chr.handle), logfmt::v3(origin), p.chr.pos_w, logfmt::dir3(*d));
                }
                match spawn::spawn(p.field_ins_handle, id, origin, p.chr.pos_w, *d) {
                    Ok(_) => {
                        if !self.first_spawn_done {
                            log_i!("SPAWN", "ok (first spawn)");
                        }
                        self.first_spawn_done = true;
                        ok += 1;
                        self.spawns_world += 2; // the bullet and its impact marker
                        bands_used.push(id);
                    }
                    Err(e) => {
                        self.first_spawn_done = true;
                        self.on_spawn_fail(&e, id, p, origin, *d, &gname);
                    }
                }
            }
            if tried > 0 && self.breaker.record(now, ok > 0, SPAWN_BREAKER, SPAWN_BREAKER_COOLDOWN_S) {
                self.on_breaker_open(p, origin, fwd);
            }
            // C3 PASS C: one no-attack synced twin per shot (the centre pellet) for the peers' tracer and sound; the
            // damaging pellets above never carry bit 3
            if ok > 0 && !self.coopvis_off {
                let peers = self.coop.peers();
                let d0 = dirs.first().copied().unwrap_or(fwd);
                self.coopvis.send_twin(now, &cfg, peers, p, band.id, origin, d0);
            }
        }
        self.pellets = dirs;
        if cfg.log.shots == ShotLog::All {
            let mut b: Vec<(i32, u32)> = Vec::new();
            for id in &bands_used {
                match b.iter_mut().find(|x| x.0 == *id) {
                    Some(x) => x.1 += 1,
                    None => b.push((*id, 1)),
                }
            }
            let bands = if b.is_empty() { "-".to_string() } else { b.iter().map(|(i, c)| format!("{i}x{c}")).collect::<Vec<_>>().join(",") };
            log_i!(
                "SHOT",
                "{} gun={gname} pellets={n} spawned={ok} dropped={} bands={bands} ads={ads:.2} cone={cone:.2} kick=({kp:.2},{ky:.2}) capped={}",
                logfmt::shot_msg(&ShotFields { n: self.shots, mag: mag_after, reserve, row_id: band.id, mw2, d_m: dist, src: src.label(), spread_deg: cone, origin, dir: dirs_first(&self.pellets, fwd) }),
                n.saturating_sub(allowed),
                capped as u8
            );
        }
        self.push_shot(ShotInfo { t: now, band_id: band.id, mw2, d: dist, origin, gun: gun as u16 });
    }

    fn push_shot(&mut self, s: ShotInfo) {
        self.last_shot = Some(s);
        if self.shot_hist.len() >= SHOT_HIST {
            self.shot_hist.pop_front();
        }
        self.shot_hist.push_back(s);
    }

    /// Bug 2: every failure names its cause and the pool counters; an `owner` cause is retried once next tick.
    fn on_spawn_fail(&mut self, e: &spawn::SpawnError, id: i32, p: &PlayerSample, origin: V3, dir: V3, gun: &str) {
        let pool = bullets::pool_snapshot(p.chr.handle, &self.our_ids, self.spawns_world);
        let cause = classify(pool.as_ref(), self.marker_probe);
        log_e!(
            "SPAWN",
            "FAIL {e} cause={} {} marker_probe={} id={id} gun={gun}",
            cause.label(),
            pool.as_ref().map_or("pool=unreadable".to_string(), pool_fields),
            match self.marker_probe {
                Some(true) => "ok",
                Some(false) => "fail",
                None => "-",
            }
        );
        if cause == Cause::Owner && self.cfg.arsenal.owner_retry && self.retry.is_none() {
            self.retry = Some((id, origin, dir));
        }
    }

    /// Bug 2: when the breaker opens, one diagnostic spawn of the inert marker with the same owner tells whether the
    /// owner can spawn at all; the classified cause and the pool counters are logged.
    fn on_breaker_open(&mut self, p: &PlayerSample, origin: V3, dir: V3) {
        let marker = self.ars.as_ref().map_or(79_000_900, |a| a.data.marker_id);
        let probe = spawn_guard::admit(shared::now_s(), SpawnKind::Gun, marker) && spawn::spawn(p.field_ins_handle, marker, origin, p.chr.pos_w, dir).is_ok();
        self.marker_probe = Some(probe);
        let pool = bullets::pool_snapshot(p.chr.handle, &self.our_ids, self.spawns_world);
        let cause = classify(pool.as_ref(), Some(probe));
        self.breaker_open = true;
        self.breaker_until = self.breaker.open_until.unwrap_or(0.0);
        log_e!(
            "SPAWN",
            "breaker open fails={SPAWN_BREAKER} cause={} marker_probe={} {}",
            cause.label(),
            if probe { "ok" } else { "fail" },
            pool.as_ref().map_or("pool=unreadable".to_string(), pool_fields)
        );
        log_e!("ERROR", "spawn breaker open after {SPAWN_BREAKER} consecutive failures; firing paused {SPAWN_BREAKER_COOLDOWN_S} s");
        shared::toast("mw2er: bullet spawns failing - firing paused (see log)", 5.0, true);
    }
}

/// The pool counters as log fields (bug 2).
fn pool_fields(p: &PoolSnap) -> String {
    format!(
        "bullets={}/{} sfx={}/{} sfx_empty_spot={} unk40={}/{} counter={} live_ours={} live_all={} spawns_world={}",
        p.bullets.allocated,
        p.bullets.heap_cap,
        p.sfx.allocated,
        p.sfx.heap_cap,
        p.sfx.empty_spot as u8,
        p.unk40.allocated,
        p.unk40.heap_cap,
        p.bullets.counter,
        p.live_ours,
        p.live_all,
        p.spawns_world
    )
}

fn dirs_first(v: &[V3], fwd: V3) -> V3 {
    v.first().copied().unwrap_or(fwd)
}

/// Per-tick cost of our task on the game's main thread (run 4: the game fell to ~15 FPS with M3b).
/// Logs `PERF ticks_per_s=.. task_us p50/max .. fp_us avg/max ..` every 5 s.
#[derive(Default)]
struct Perf {
    t0: f64,
    task_us: Vec<f64>,
    fp_sum: f64,
    fp_max: f64,
    fp_n: u32,
    /// M4: an AC-130 ride was Active during this window (`ac130=1`).
    ac130: bool,
}

impl Perf {
    fn add_fp(&mut self, us: f64) {
        self.fp_sum += us;
        self.fp_max = self.fp_max.max(us);
        self.fp_n += 1;
    }

    fn add_task(&mut self, us: f64, now: f64, extra: impl FnOnce() -> String) {
        if self.t0 == 0.0 {
            self.t0 = now;
        }
        if self.task_us.len() < 4096 {
            self.task_us.push(us);
        }
        let span = now - self.t0;
        if span >= 5.0 && !self.task_us.is_empty() {
            let n = self.task_us.len();
            self.task_us.sort_by(|a, b| a.total_cmp(b));
            let p50 = self.task_us[n / 2];
            let p95 = self.task_us[(n * 95 / 100).min(n - 1)];
            let max = self.task_us[n - 1];
            let fp_avg = if self.fp_n > 0 { self.fp_sum / self.fp_n as f64 } else { 0.0 };
            // Game-memory write cost (run 5: ~35 ms per write = the FPS collapse). writes/page_checks are
            // cumulative; write_us_max/page_check_ms_max are the maxima of this 5 s window.
            let w = crate::game::write_stats();
            log_i!(
                "PERF",
                "ticks_per_s={:.1} task_us p50={p50:.0} p95={p95:.0} max={max:.0} fp_us avg={fp_avg:.0} max={:.0} writes={} write_us_max={:.1} slow_writes={} page_checks={} page_check_ms_max={:.2} page_check_ms_total={:.1} cached_pages={} ac130={} {}",
                n as f64 / span,
                self.fp_max,
                w.writes,
                w.write_us_max,
                w.slow_writes,
                w.page_checks,
                w.page_check_ms_max,
                w.page_check_ms_total,
                w.cached_pages,
                self.ac130 as u8,
                extra()
            );
            *self = Perf { t0: now, ..Perf::default() };
        }
    }
}

fn run_tick(st: &mut TaskState) {
    if GAMEPLAY_DISABLED.load(Ordering::Relaxed) {
        // review M4 major 2: after a task panic the protection / ER HUD cleanup keeps running (guarded, cheap)
        if st.ac130_disabled_ok && guard("ac130.disabled", || st.ac130.disabled_tick()).is_none() {
            st.ac130_disabled_ok = false;
        }
        // review M5 minor 6: a gravity restore deferred by the panic path is retried until it resolves
        if st.move_disabled_ok && guard("move.disabled", || st.movement.disabled_tick()).is_none() {
            st.move_disabled_ok = false;
        }
        return;
    }
    if st.first_tick {
        st.first_tick = false;
        set_thread_role("task");
        log_i!("TASK", "first tick on the game's main thread");
    }
    let t0 = std::time::Instant::now();
    let ok = guard("task", || st.tick()).is_some();
    // M6 PERF fields (docs/M6_DESIGN.md 12.1): view-fx frames that still held our write (kick_stale), live launcher
    // projectiles, bullets spawned in this world; formatted only when the 5 s line is written
    let m6 = st.ars.as_ref().map(|a| (a.projs.len(), st.spawns_world));
    let mv = &mut st.movement;
    st.perf.add_task(t0.elapsed().as_secs_f64() * 1e6, shared::now_s(), || match m6 {
        Some((proj_live, spawns_world)) => format!(
            "{} kick_stale={} viewfx_writes={} proj_live={proj_live} spawns_world={spawns_world}",
            mv.perf_take(),
            shared::VIEWFX_STALE.load(Ordering::Relaxed),
            shared::VIEWFX_WRITES.load(Ordering::Relaxed)
        ),
        None => mv.perf_take(),
    });
    if !ok {
        shared::disable_gameplay(); // clears the blanking flags first
        // M5: gravity back, feet unpublished, blank wishes cleared (before the AC-130 and first-person paths)
        let _ = guard("move.emergency", || st.movement.emergency_off(shared::now_s()));
        // M4: end an AC-130 ride first (camera mode 2, input capture, ER HUD, protection), under its own guard.
        let cfg = st.cfg.clone();
        let _ = guard("ac130.emergency", || st.ac130.emergency_off(shared::now_s(), &cfg));
        // M3b: give the camera, the body and the facing back (fresh reads, under its own guard).
        let _ = guard("fp.emergency", || st.fp.emergency_off(shared::now_s()));
        // C6 part 18: the shoulder offset disarmed (ER rebuilds the camera next frame)
        let _ = guard("shoulder.emergency", || st.shoulder.emergency_off());
        // M7: a running nuke sequence ends (sounds stop; the post pass ends with the stale snapshot)
        let _ = guard("nuke.emergency", || st.nuke.emergency_off());
        // C6: every patched EquipParamWeapon row back to its original, no R3 block, no chord
        let _ = guard("stance.emergency", || st.stance.restore_all("panic"));
        log_e!("ERROR", "task disabled: gameplay off for this session; LMB/R returned to the game");
        shared::toast("mw2er: gameplay task failed - see logs\\mw2er.log", 15.0, true);
    }
}

/// Waits for the task system and registers the per-frame closure (boot thread, after the gate).
pub fn register(data: Option<GameData>) -> Result<(), String> {
    let cs_task = CSTaskImp::wait_for_instance(Duration::from_secs(60)).map_err(|e| format!("CSTask: {e}"))?;
    let mut st = TaskState::new(data);
    let handle = cs_task.run_recurring(
        move |_: &FD4TaskData| run_tick(&mut st),
        CSTaskGroupIndex::ChrIns_PostPhysics,
    );
    // Dropping the handle would cancel the task; it lives for the process lifetime.
    std::mem::forget(handle);
    Ok(())
}

/// Review C3: the most one launcher shot of `gun` can deal (impact row + every explosion tier; a bound, not a model).
fn launcher_max_atk(a: &crate::arsenal_task::ArsenalCtl, gun: usize) -> Option<i32> {
    if gun >= a.data.guns.len() {
        return None;
    }
    let l = a.gun(gun).rows.launcher?;
    Some(l.impact.atk + l.core.map_or(0, |r| r.atk) + l.ring.map_or(0, |r| r.atk) + l.single.atk)
}

/// Review C3: the most one AC-130 shell can deal (the largest gun's tiers summed; a bound).
fn ac130_max_atk() -> Option<i32> {
    let d = crate::data::AC_DATA.get()?;
    d.rows.guns.values().map(|g| g.core.map_or(0, |r| r.atk_phys) + g.ring.map_or(0, |r| r.atk_phys) + g.single.atk_phys).max()
}
