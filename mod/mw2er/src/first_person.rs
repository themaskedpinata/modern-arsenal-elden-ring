//! First-person controller (docs/M3B_DESIGN.md): runs once per task tick on the game's main thread.
//!
//! Per tick: F6 presses -> read every signal (game::fp_read) -> camera self-check -> triggers -> the pure state
//! machine (mw2er_core::fp_logic) -> on `On`: save + hide + log; on `Off(reason)`: exact restore + log; while On:
//! eye write (task mode) or publish to the camera-step hook (hook mode), hide re-check, yaw follow, periodic log.
//!
//! Every write goes through `game::write` (version-gated, non-faulting) and is logged once per episode with its
//! field, address, old and new value; every handback logs `FP off reason=<label> <detail>`.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use mw2er_core::cam_gate::Why;
use mw2er_core::config::{CameraMode, FirstPerson, HideMethod, YawAxisCfg};
use mw2er_core::fp_latch::{self, F6Action};
use mw2er_core::fp_logic::{
    self, EdgeLatch, EdgeWatch, FadeWatch, FlapGuard, FpEvent, FpMachine, LatchParams, LatchRelease, LatchStep, Reason, TalkGate,
    Triggers, fade_max,
};
use mw2er_core::fp_math::{
    AF_DISABLE_TURNING, Axis, DriftStats, EyeSmoother, ProbeEvent, YawGateIn, YawPause, YawProbe, angle_diff_deg, clamp_pitch,
    eye_position, horizontal_forward, off_heading, pitch_deg, quat_from_yaw, yaw_from_quat, yaw_gate, yaw_of, yaw_pair,
};
use mw2er_core::fp_restore::{
    BitTick, FadeHide, FadeRestoring, FadeStep, Observation, PersistWatch, RenderBit, YawRelease, hook_row_restore,
};
use mw2er_core::input_logic::CounterReader;
use mw2er_core::logfmt;
use mw2er_core::move_input::PRESS_INTERACT;
use mw2er_core::vec3::V3;

use crate::cam_hook;
use crate::game::fp_read::{
    BASE_TRANSPARENCY_MOD, CAM_FOV, CAM_ROW3, CAM_ROWS, CHR_CAM_ROW3, FLAGS1C5, FOLLOW_CAM_AUTO_ROTATION, FpPlayer, FpWorld,
    HideFields, INTERP_ORIENTATION, ORIENTATION, PersCam, cscamera_addr, read_chr_cam_ptr, read_hide_fields, read_pers_cam1,
    world_chr_man_addr,
};

// The yaw path writes both facing quaternions with ONE 32-byte write (run 5: every write used to cost ~35 ms).
const _: () = assert!(INTERP_ORIENTATION == ORIENTATION + 16);
use crate::game::{LAST_WRITE_REFUSAL, refusal_label, write};
use crate::log::{log_i, log_w};
use crate::shared::{self, MOVE_A, MOVE_D, MOVE_S, MOVE_W, PRESSES_FP, PRESSES_MOVE, RAW_MOVE_KEYS};

/// The yaw probe samples only while the character actually runs.
const PROBE_MIN_SPEED_MPS: f32 = 1.0;
const DRIFT_LOG_EVERY_S: f64 = 5.0;
const PROBE_LINE_MIN_S: f64 = 0.5;
/// Hook mode: warn when the detour wrote nothing for this many ticks while On.
const HOOK_SILENT_TICKS: u64 = 60;
const FLAP_LOAD: usize = 0;
const FLAP_MASK: usize = 1;
const FLAP_TALK: usize = 2;
const FLAP_FADE: usize = 3;
const FLAP_NAMES: [&str; 4] = ["load_counter", "camera_mask", "talk_id", "fade"];
/// Planar position jumps above this are map re-basing, not movement (same as task.rs).
const REBASE_JUMP_M: f32 = 5.0;
const FLAP_CFG: [&str; 4] = ["(no switch: loading also hands back on world left)", "use_cam_mask_signal", "use_talk_signal", "use_fade_signal"];

/// One tick's inputs from the task (all already computed there).
pub struct TickIn<'a> {
    pub now: f64,
    pub dt: f32,
    pub cfg: &'a FirstPerson,
    pub mode_on: bool,
    pub in_world: bool,
    pub menu_open: bool,
    pub cam_valid: bool,
    pub cam_why: Why,
    /// ER's own (pre-write) camera position, as the gate saw it.
    pub er_pos: Option<V3>,
    /// The player's chest (feet + aim.chest_height_m).
    pub chest: Option<V3>,
    pub planar_speed: f32,
    /// M4: an AC-130 ride is starting or running (fp_logic reason `ac130`).
    pub ac130_block: bool,
    /// M7: the nuke's player-death prep (fp_logic reason `nuke`).
    pub nuke_block: bool,
    /// The signal reads of this tick, read once by the task (shared with the AC-130; review M4 minor 6).
    pub player_fp: Result<Option<FpPlayer>, &'static str>,
    pub world: Option<FpWorld>,
    /// M5: the movement controller drives: its feet (metres, this tick's write) and stance eye height above them.
    pub move_feet: Option<V3>,
    pub move_eye_h: Option<f32>,
    /// C3: `CSMenuManImp.ui_states` visible bits this tick (the talk latch holds while the dialog index is open).
    pub ui_mask: u128,
    /// C6 scope peek (work/c6/scope3p.md): third-person scoped ADS wants first person for as long as it is held
    /// (`fp_logic::peek_wanted`); inert while the user's own first person is on.
    pub scope_peek: bool,
}

enum Hidden {
    Bit(RenderBit),
    Fade(FadeHide),
}

pub struct FpController {
    machine: FpMachine,
    wanted: bool,
    /// C6 scope peek: the running episode began as a third-person scoped ADS (`wanted` 0 at On). Cleared when the
    /// machine is off or the user wants first person (an F6 during a peek makes it the user's own episode).
    peek_episode: bool,
    presses: CounterReader,
    mode: CameraMode,
    cfg_seen: Option<FirstPerson>,
    /// cam_hook::INSTALLED when the cfg line was logged (run 8: the line can precede the hook install by ms).
    cfg_seen_hook: bool,
    // camera
    row3: PersistWatch<[f32; 4]>,
    rows: PersistWatch<[[f32; 4]; 3]>,
    fov: PersistWatch<f32>,
    cam_disabled: bool,
    rewrite_logged: bool,
    pending_fail: Option<&'static str>,
    hook_writes_at_on: u64,
    hook_ok_logged: bool,
    hook_silent_warned: bool,
    // erfps2's camera handling (hook mode, after run 5): detour counters at FP on (for the once-per-episode logs)
    chr_writes_at_on: u64,
    autorot_clears_at_on: u64,
    wall_clears_at_on: u64,
    follow_calls_at_on: u64,
    // body
    hidden: Option<Hidden>,
    /// A hide that could not be restored at handback (not in world); resolved before anything else.
    pending_hidden: Option<Hidden>,
    fade_restoring: Option<FadeRestoring>,
    // yaw
    yaw: YawRelease,
    probe: YawProbe,
    drift: DriftStats,
    last_yaw_written: Option<f32>,
    pauses: [u64; YawPause::ALL.len()],
    paused_now: Option<YawPause>,
    prev_feet: Option<V3>,
    last_drift_log: f64,
    // eye
    smoother: EyeSmoother,
    eye: Option<V3>,
    clamped: bool,
    // edges
    load_edge: EdgeWatch<i32>,
    mask_edge: EdgeWatch<u32>,
    talk_edge: EdgeWatch<i32>,
    /// 2026-10-21: a talk id change hands back only shortly after an interact press (boss voice lines do not).
    talk_gate: TalkGate,
    interact_presses: CounterReader,
    talk_ignored: u32,
    fade_watch: FadeWatch,
    /// Handback latches of the edge signals (review M3b major 1): an edge keeps first person off until its value is
    /// back at the On baseline (+ hold-off) or has gone stale with the cursor closed.
    load_latch: EdgeLatch<i32>,
    mask_latch: EdgeLatch<u32>,
    talk_latch: EdgeLatch<i32>,
    fade_latch: EdgeLatch<bool>,
    fade_latch_plate: usize,
    persist_warned: bool,
    /// Anti-flap of the inferred edge signals: load_counter, camera_mask, talk_id, fade.
    flap: [FlapGuard; 4],
    // episode
    owner: usize,
    ticks_on: u64,
    cam_writes: u64,
    logged: HashSet<&'static str>,
    last_probe: String,
    last_probe_t: f64,
    /// Run 15: world presence of the previous tick (respawn detection).
    was_in_world: bool,
    /// Run 16: consecutive ticks the camera gate was invalid.
    cam_bad_ticks: u32,
}

impl FpController {
    pub fn new(cfg: &FirstPerson) -> Self {
        FpController {
            machine: FpMachine::default(),
            wanted: cfg.enabled,
            peek_episode: false,
            presses: CounterReader::starting_at(PRESSES_FP.load(Ordering::SeqCst)),
            mode: cfg.camera_mode,
            cfg_seen: None,
            cfg_seen_hook: false,
            row3: PersistWatch::default(),
            rows: PersistWatch::default(),
            fov: PersistWatch::default(),
            cam_disabled: false,
            rewrite_logged: false,
            pending_fail: None,
            hook_writes_at_on: 0,
            hook_ok_logged: false,
            hook_silent_warned: false,
            chr_writes_at_on: 0,
            autorot_clears_at_on: 0,
            wall_clears_at_on: 0,
            follow_calls_at_on: 0,
            hidden: None,
            pending_hidden: None,
            fade_restoring: None,
            yaw: YawRelease::default(),
            probe: YawProbe::new(cfg.yaw_probe_samples as usize),
            drift: DriftStats::default(),
            last_yaw_written: None,
            pauses: [0; YawPause::ALL.len()],
            paused_now: None,
            prev_feet: None,
            last_drift_log: 0.0,
            smoother: EyeSmoother::default(),
            eye: None,
            clamped: false,
            load_edge: EdgeWatch::default(),
            mask_edge: EdgeWatch::default(),
            talk_edge: EdgeWatch::default(),
            talk_gate: TalkGate::default(),
            interact_presses: CounterReader::starting_at(PRESSES_MOVE[PRESS_INTERACT].load(Ordering::SeqCst)),
            talk_ignored: 0,
            fade_watch: FadeWatch::default(),
            load_latch: EdgeLatch::default(),
            mask_latch: EdgeLatch::default(),
            talk_latch: EdgeLatch::default(),
            fade_latch: EdgeLatch::default(),
            fade_latch_plate: 0,
            persist_warned: false,
            flap: Default::default(),
            owner: 0,
            ticks_on: 0,
            cam_writes: 0,
            logged: HashSet::new(),
            last_probe: String::new(),
            last_probe_t: f64::NEG_INFINITY,
            was_in_world: false,
            cam_bad_ticks: 0,
        }
    }

    /// M4: which edge-signal latches currently hold (loading, cutscene, talk): the AC-130 start gate reuses them.
    pub fn latched(&self) -> (bool, bool, bool) {
        (self.load_latch.is_latched(), self.mask_latch.is_latched() || self.fade_latch.is_latched(), self.talk_latch.is_latched())
    }

    /// C6 scope peek: first person is on only for a third-person scoped ADS (the user's choice is third person). Callers
    /// that must keep third-person behaviour during a peek (the C6 stance, the solo hide marker) read this.
    pub fn peeking(&self) -> bool {
        self.machine.is_on() && !self.wanted && self.peek_episode
    }

    /// The eye this tick while first person is On (bullets leave from it).
    pub fn eye(&self) -> Option<V3> {
        if self.machine.is_on() { self.eye } else { None }
    }

    /// Task mode: when the camera the task just read is exactly the eye we wrote last tick (ER skipped one camera
    /// step, so our write persisted), ER's last own position for the gate/CAMFRAME/aim this tick (review M3b minor 6).
    pub fn er_view_override(&self, read_pos: V3) -> Option<V3> {
        if !self.machine.is_on() || self.hook_mode() {
            return None;
        }
        let w = self.row3.last_written()?;
        let same = read_pos.x.to_bits() == w[0].to_bits() && read_pos.y.to_bits() == w[1].to_bits() && read_pos.z.to_bits() == w[2].to_bits();
        let er = self.row3.last_er()?;
        same.then_some(V3::new(er[0], er[1], er[2]))
    }

    fn hook_mode(&self) -> bool {
        self.mode == CameraMode::Hook && cam_hook::INSTALLED.load(Ordering::SeqCst)
    }

    fn log_cfg(&mut self, cfg: &FirstPerson) {
        let installed = cam_hook::INSTALLED.load(Ordering::SeqCst);
        if self.cfg_seen.as_ref() == Some(cfg) && self.cfg_seen_hook == installed {
            return;
        }
        self.cfg_seen_hook = installed;
        let mode = if self.mode == CameraMode::Hook && !cam_hook::INSTALLED.load(Ordering::SeqCst) {
            "task (hook requested but not installed)".to_string()
        } else {
            self.mode.label().to_string()
        };
        log_i!(
            "FPCAM",
            "cfg enabled={} mode={mode} eye_h={:.2} eye_fwd={:.2} eye_smooth_s={:.2} pitch_clamp={:.1} hide={} yaw={} world_fov={} arm_ticks={} fade_alpha_max={:.2} signals fade={} cam_mask={} talk={} key={} sync_chr_cam={} wall_recovery_off={} auto_rotation_off={} talk_window={:.1}s{}",
            cfg.enabled as u8,
            cfg.eye_height_m,
            cfg.eye_forward_m,
            cfg.eye_smooth_s,
            cfg.pitch_clamp_deg,
            cfg.hide_method.label(),
            cfg.yaw_follow as u8,
            if cfg.world_fov_deg > 0.0 { format!("{:.1}deg", cfg.world_fov_deg) } else { "off".into() },
            cfg.arm_ticks,
            cfg.fade_alpha_max,
            cfg.use_fade_signal as u8,
            cfg.use_cam_mask_signal as u8,
            cfg.use_talk_signal as u8,
            shared::config().keys.fp_toggle,
            cfg.sync_chr_cam as u8,
            cfg.disable_wall_recovery as u8,
            cfg.disable_auto_rotation as u8,
            cfg.talk_interact_window_s,
            // C6 scope peek (the defaults keep the line as before)
            if cfg.tp_scope_peek { format!(" tp_scope_peek=1 peek_arm={}", cfg.scope_peek_arm_ticks) } else { String::new() }
        );
        // A change of the master switch (hot reload) also resets the F6 preference to it.
        if self.cfg_seen.as_ref().is_some_and(|old| old.enabled != cfg.enabled) {
            self.wanted = cfg.enabled;
        }
        self.cfg_seen = Some(cfg.clone());
    }

    fn handle_f6(&mut self, cfg: &FirstPerson) {
        let n = self.presses.take(PRESSES_FP.load(Ordering::SeqCst));
        for _ in 0..n.min(4) {
            if !cfg.enabled {
                log_i!("FP", "toggle F6 refused (first_person.enabled = false)");
                shared::toast("First person disabled in mw2er.toml", 2.0, false);
                continue;
            }
            // C3 (docs/C3_DESIGN.md 1.7): F6 while first person is off for a non-user reason (a talk / fade / load
            // latch, a menu, the camera gate) brings it back: the edge latches are released and `wanted` stays 1
            // (C2: the user pressed F6 during a stuck talk latch, which turned first person off for good)
            // review C3: only the talk / fade latches are released; a load or a camera-mask cutscene defers the press
            let fade_only = self.fade_latch.is_latched() && !self.mask_latch.is_latched();
            match fp_latch::f6_action(self.machine.is_on(), self.wanted, self.machine.last_block(), cfg.f6_overrides_latches, fade_only) {
                F6Action::Override(r) => {
                    let released = self.release_latches();
                    self.wanted = true;
                    log_i!("FP", "toggle F6 override reason={} released={released} wanted=1", r.label());
                    shared::toast("First person ON", 1.5, false);
                }
                F6Action::Defer(r) => {
                    log_i!("FP", "toggle F6 deferred reason={} wanted=1 (first person returns when it ends)", r.label());
                    shared::toast("First person returns after the cutscene / loading", 1.5, false);
                }
                F6Action::Toggle => {
                    self.wanted = !self.wanted;
                    log_i!("FP", "toggle F6 wanted={}", self.wanted as u8);
                    shared::toast(if self.wanted { "First person ON" } else { "First person OFF" }, 1.5, false);
                }
            }
        }
    }

    /// C3: drops the talk and fade latches (F6 override); returns their names ("none" when nothing was latched).
    /// Review C3: the load and camera-mask latches are never released here (they would not re-latch while the load /
    /// cutscene goes on, and the nuke reads the load latch).
    fn release_latches(&mut self) -> String {
        let mut v: Vec<&str> = Vec::new();
        if self.talk_latch.is_latched() {
            self.talk_latch.reset();
            v.push(FLAP_NAMES[FLAP_TALK]);
        }
        if self.fade_latch.is_latched() {
            self.fade_latch.reset();
            v.push(FLAP_NAMES[FLAP_FADE]);
        }
        shared::TALK_LATCHED.store(false, Ordering::Relaxed);
        if v.is_empty() { "none".into() } else { v.join(",") }
    }

    /// Run 18: first person drives (or is about to drive) the render camera, so the camera gate may accept ER's own
    /// camera next to the chest (its collision pulls it in against a wagon / wall; `[first_person] cam_gate_close_ok`).
    pub fn close_cam_ok(&self, cfg: &FirstPerson) -> bool {
        cfg.cam_gate_close_ok && cfg.enabled && (self.machine.is_on() || self.wanted)
    }

    /// Logs a write once per episode.
    fn log_write_once(&mut self, key: &'static str, msg: impl FnOnce() -> String) {
        if self.logged.insert(key) {
            log_i!("FPCAM", "{}", msg());
        }
    }

    /// One task tick. Returns the eye while first person is On.
    pub fn tick(&mut self, i: &TickIn) -> Option<V3> {
        let cfg = i.cfg;
        self.log_cfg(cfg);
        self.handle_f6(cfg);
        // E edges and pad interact holds (its own reader; movement consumes the same counter with another)
        let interacts = self.interact_presses.take(PRESSES_MOVE[PRESS_INTERACT].load(Ordering::Relaxed));
        self.talk_gate.interacted(interacts, i.now);

        // ---- reads ----
        let (player, mut read_fail) = if i.in_world {
            match i.player_fp {
                Ok(p) => (p, None),
                Err(f) => (None, Some(f)),
            }
        } else {
            (None, None)
        };
        let world = if i.in_world { i.world } else { None };
        let cam = if i.in_world { read_pers_cam1() } else { None };
        if i.in_world && read_fail.is_none() && player.is_some() && cam.is_none() {
            read_fail = Some("pers_cam_1");
        }
        let write_fail = self.pending_fail.take();
        let in_world = i.in_world && (player.is_some() || read_fail.is_some());
        // Run 16: ER re-bases the world coordinates every ~32 m of walking; for one tick the camera is in the old
        // chunk and the player in the new one (cam_chest ~31 m), which tripped cam_gate and flashed third person.
        // Only a gate failure that lasts cam_gate_grace_ticks hands first person back.
        self.cam_bad_ticks = if i.cam_valid { 0 } else { self.cam_bad_ticks.saturating_add(1) };
        // Run 15: a respawn / reload (world left -> entered) brings first person back like MW2, even if F6 had
        // turned it off before the death; F6 still toggles it afterwards.
        if in_world && !self.was_in_world && !self.wanted && cfg.enabled && cfg.reset_on_respawn {
            self.wanted = true;
            log_i!("FP", "respawn: F6 choice reset, first person wanted=1");
        }
        self.was_in_world = in_world;

        // ---- camera self-check (task mode, while On): did ER rebuild pers_cam_1 since our last write? ----
        if self.machine.is_on()
            && let Some(c) = cam
        {
            // The FOV override is written by the task in both modes.
            let _ = self.fov.observe(c.fov);
        }
        if self.machine.is_on()
            && !self.hook_mode()
            && let Some(c) = cam
        {
            let o3 = self.row3.observe(c.m[3]);
            let orows = self.rows.observe([c.m[0], c.m[1], c.m[2]]);
            if self.row3.persists() || self.rows.persists() {
                self.cam_disabled = true;
                if !self.persist_warned {
                    self.persist_warned = true;
                    log_w!(
                        "FPCAM",
                        "WARN er_rewrite=0: pers_cam_1 still holds our write ({}) for {} ticks in a row; ER does not rebuild it here. First person off for this session; try [first_person] camera_mode = \"hook\"",
                        logfmt::v3(V3::new(c.m[3][0], c.m[3][1], c.m[3][2])),
                        self.row3.persist_streak().max(self.rows.persist_streak())
                    );
                }
            } else if o3 == Observation::Persisted || orows == Observation::Persisted {
                let streak = self.row3.persist_streak().max(self.rows.persist_streak());
                self.log_write_once("persist1", || {
                    format!(
                        "persisted sample streak={streak}/{} (ER skipped a camera step; the gate gets ER's last position this tick; logged once per episode)",
                        mw2er_core::fp_restore::PERSIST_STREAK_DISABLE
                    )
                });
            } else if o3 == Observation::Rewritten && !self.rewrite_logged {
                self.rewrite_logged = true;
                log_i!(
                    "FPCAM",
                    "er_rewrite=1: ER rebuilt pers_cam_1 row 3 between two ticks ({} -> {}); handback restore is free",
                    logfmt::v3(self.row3.last_written().map_or(V3::ZERO, |w| V3::new(w[0], w[1], w[2]))),
                    logfmt::v3(V3::new(c.m[3][0], c.m[3][1], c.m[3][2]))
                );
            }
        }

        // ---- triggers ----
        let p = player.as_ref();
        // A deferred hide restore is resolved before the machine may turn On again (enter() would otherwise save
        // the still-hidden byte as the "original"). It uses its own small read of main_player + the hide fields, so
        // a failing module read (read_fail) can never keep the body hidden (review M3b major 2).
        if let Some(h) = self.pending_hidden.take() {
            self.pending_hidden = self.restore_hidden(h, "deferred", i.now, false);
        }
        let w = world.unwrap_or(FpWorld {
            camera_type: None,
            camera_mask: None,
            fade: [None; fp_logic::FADE_PLATES],
            fade_available: false,
            load_counter: None,
            talk_id: None,
        });
        let owner_changed = self.machine.is_on() && p.is_some_and(|p| p.player != self.owner);
        let load_on = !self.flap[FLAP_LOAD].muted();
        let fade_on = cfg.use_fade_signal && !self.flap[FLAP_FADE].muted();
        let mask_on = cfg.use_cam_mask_signal && !self.flap[FLAP_MASK].muted();
        let talk_on = cfg.use_talk_signal && !self.flap[FLAP_TALK].muted();

        // Latches of earlier edge handbacks (they hold the trigger on while the cause lasts).
        let lp = LatchParams { holdoff_s: cfg.latch_holdoff_s as f64, stale_s: cfg.latch_stale_s as f64 };
        // A counter never returns to its old value: it releases once it has been stable (loading over) for the
        // hold-off (at least 2 s) with the cursor closed.
        let lp_load = LatchParams { holdoff_s: lp.holdoff_s, stale_s: lp.holdoff_s.max(2.0) };
        // C3 (docs/C3_DESIGN.md 1.7): the talk id never returns to its old value: its latch releases `talk_stale_s`
        // after it settled with the cursor AND the dialog index (`talk_menu_idx`) closed; a fade plate stuck high
        // releases after `fade_stale_s`
        let lp_talk = fp_latch::talk_params(lp.holdoff_s, cfg.talk_stale_s as f64);
        let lp_fade = fp_latch::fade_params(lp.holdoff_s, cfg.fade_stale_s as f64);
        let talk_menu = fp_latch::talk_menu_open(i.menu_open, i.ui_mask, cfg.talk_menu_idx);
        let fade_now = w.fade[self.fade_latch_plate].filter(|a| a.is_finite()).map(|a| a > cfg.fade_alpha_max);
        let load_hold = latch_tick(&mut self.load_latch, load_on, FLAP_NAMES[FLAP_LOAD], w.load_counter, i.menu_open, i.now, lp_load);
        let mask_hold = latch_tick(&mut self.mask_latch, mask_on, FLAP_NAMES[FLAP_MASK], w.camera_mask, i.menu_open, i.now, lp);
        let talk_hold = latch_tick(&mut self.talk_latch, talk_on, FLAP_NAMES[FLAP_TALK], w.talk_id, talk_menu, i.now, lp_talk);
        let fade_hold = latch_tick(&mut self.fade_latch, fade_on, FLAP_NAMES[FLAP_FADE], fade_now, i.menu_open, i.now, lp_fade);

        // Edges (armed only while On).
        let load_changed = load_on && self.load_edge.changed(w.load_counter);
        let fade_fire = if fade_on { self.fade_watch.fired(&w.fade, cfg.fade_alpha_max) } else { None };
        let mask_changed = mask_on && self.mask_edge.changed(w.camera_mask);
        let talk_moved = talk_on && self.talk_edge.changed(w.talk_id);
        // 2026-10-21: a change with no interact press shortly before is a boss / NPC voice line (c2130 mid-fight):
        // first person stays on and the edge takes the new id as its baseline (a later grace / NPC talk still fires)
        let talk_changed = talk_moved && self.talk_gate.is_dialog(i.now, cfg.talk_interact_window_s as f64);
        if talk_moved && !talk_changed {
            self.talk_ignored += 1;
            log_i!(
                "FP",
                "talk ignored talk_id {:?}->{:?} n={} last_interact={} window={:.1}s (no E / pad interact before it: a voice line, not a dialog)",
                self.talk_edge.baseline(),
                w.talk_id,
                self.talk_ignored,
                Some(self.talk_gate.since(i.now)).filter(|s| s.is_finite()).map_or("never".into(), |s| format!("{s:.1}s")),
                cfg.talk_interact_window_s
            );
            self.talk_edge.arm(w.talk_id);
        }
        if load_changed && let Some(b) = self.load_edge.baseline() {
            self.load_latch.latch(b, w.load_counter, i.now);
            log_i!("FP", "latch set signal=load_counter baseline={b} now={:?}", w.load_counter);
        }
        if mask_changed && let Some(b) = self.mask_edge.baseline() {
            self.mask_latch.latch(b, w.camera_mask, i.now);
            log_i!("FP", "latch set signal=camera_mask baseline=0x{b:x} now={:?}", w.camera_mask.map(|m| format!("0x{m:x}")));
        }
        if talk_changed && let Some(b) = self.talk_edge.baseline() {
            self.talk_latch.latch(b, w.talk_id, i.now);
            log_i!("FP", "latch set signal=talk_id baseline={b} now={:?}", w.talk_id);
        }
        if let Some((plate, a)) = fade_fire {
            self.fade_latch_plate = plate;
            self.fade_latch.latch(false, Some(true), i.now);
            log_i!("FP", "latch set signal=fade plate={plate} a={a:.2} thr={:.2}", cfg.fade_alpha_max);
        }
        // C3: movement's `talk` reason follows this latch
        shared::TALK_LATCHED.store(self.talk_latch.is_latched(), Ordering::Relaxed);
        if self.wanted || !self.machine.is_on() {
            self.peek_episode = false;
        }
        let t = Triggers {
            // an unresolved hide restore also blocks arming (enter() would save the hidden byte as the original)
            read_fail: read_fail.is_some() || write_fail.is_some() || (in_world && self.pending_hidden.is_some()),
            cam_persist: self.cam_disabled,
            loading: !in_world || owner_changed || load_changed || load_hold,
            dead: p.is_some_and(|p| fp_logic::dead(p.hp, p.flags1c5, w.camera_type)),
            ac130: i.ac130_block,
            nuke: i.nuke_block,
            cutscene: fade_fire.is_some() || mask_changed || fade_hold || mask_hold,
            riding: p.is_some_and(|p| fp_logic::riding(p.ride_state, p.is_mounting, cfg.use_ride_state_signal)),
            ladder: p.is_some_and(|p| fp_logic::on_ladder(p.ladder)),
            throw: p.is_some_and(|p| fp_logic::in_throw(p.throw_state)),
            talk: talk_changed || talk_hold,
            menu: i.menu_open,
            cam_invalid: self.cam_bad_ticks >= cfg.cam_gate_grace_ticks,
            mode_off: !i.mode_on,
            // C6 scope peek: a scoped ADS in third person clears the F6 trigger while it is held
            // (only for an episode that began as a peek: F6 out of the user's own first person while scoped turns it off)
            unwanted: !(self.wanted || fp_logic::peek_holds(i.scope_peek, self.peek_episode, self.machine.is_on())),
            disabled: !cfg.enabled,
        };
        let detail = |r: Reason| -> String {
            match r {
                Reason::ReadFail => match (read_fail, write_fail) {
                    (Some(f), _) => format!("field={f}"),
                    (None, None) if self.pending_hidden.is_some() => "field=pending_hide_restore".into(),
                    (None, Some(f)) => {
                        format!("write_fail field={f} why={}", refusal_label(LAST_WRITE_REFUSAL.load(Ordering::Relaxed)))
                    }
                    _ => String::new(),
                },
                Reason::CamPersist => "er_rewrite=0".into(),
                Reason::Loading if !in_world => "world left".into(),
                Reason::Loading if owner_changed => "main_player changed".into(),
                Reason::Loading if load_changed => format!("load_counter {:?}->{:?}", self.load_edge.baseline(), w.load_counter),
                Reason::Loading => format!("latched load_counter {:?}->{:?}", self.load_latch.baseline(), w.load_counter),
                Reason::Death => p.map_or(String::new(), |p| {
                    format!("hp={} death_flag={} camtype={:?}", p.hp, (p.flags1c5 & fp_logic::DEATH_FLAG != 0) as u8, w.camera_type)
                }),
                Reason::Cutscene => match fade_fire {
                    Some((i, a)) => format!("fade plate={i} a={a:.2}"),
                    None if mask_changed => format!(
                        "cam_mask {:?}->{:?}",
                        self.mask_edge.baseline().map(|m| format!("0x{m:x}")),
                        w.camera_mask.map(|m| format!("0x{m:x}"))
                    ),
                    None if mask_hold => format!(
                        "latched cam_mask {:?}->{:?}",
                        self.mask_latch.baseline().map(|m| format!("0x{m:x}")),
                        w.camera_mask.map(|m| format!("0x{m:x}"))
                    ),
                    None => format!("latched fade plate={} above={:?}", self.fade_latch_plate, fade_now),
                },
                Reason::Torrent => p.map_or(String::new(), |p| format!("ride_state={:?} mounting={}", p.ride_state, p.is_mounting as u8)),
                Reason::Ladder => p.map_or(String::new(), |p| format!("state={}", p.ladder)),
                Reason::Throw => p.map_or(String::new(), |p| format!("throw_state={:?}", p.throw_state)),
                Reason::Talk if talk_changed => format!("talk_id {:?}->{:?}", self.talk_edge.baseline(), w.talk_id),
                Reason::Talk => format!("latched talk_id {:?}->{:?}", self.talk_latch.baseline(), w.talk_id),
                Reason::CamGate => format!("why={}", i.cam_why.label()),
                _ => String::new(),
            }
        };

        let arm = fp_logic::arm_ticks_for(i.scope_peek, self.wanted, self.machine.last_block(), cfg.arm_ticks, cfg.scope_peek_arm_ticks);
        match self.machine.step(&t, arm) {
            Some(FpEvent::On) => {
                self.peek_episode = i.scope_peek && !self.wanted;
                if let (Some(p), Some(c)) = (p, cam) {
                    self.enter(i, p, &w, &c);
                } else {
                    // cannot happen (no trigger means both reads succeeded); stay safe
                    self.machine.force_off(Reason::ReadFail);
                }
            }
            Some(FpEvent::Off(r)) => {
                let d = detail(r);
                // Which inferred edge signal (if any) caused it: feeds the anti-flap guard.
                let edge = match r {
                    Reason::Loading if in_world && !owner_changed && load_changed => Some(FLAP_LOAD),
                    Reason::Cutscene if fade_fire.is_some() => Some(FLAP_FADE),
                    Reason::Cutscene if mask_changed => Some(FLAP_MASK),
                    Reason::Talk if talk_changed => Some(FLAP_TALK),
                    _ => None,
                };
                self.leave(r, &d, i.now, in_world, cam.as_ref());
                if let Some(k) = edge
                    && self.flap[k].record(i.now)
                {
                    log_w!(
                        "FP",
                        "WARN signal {} muted for this session after {} handbacks in {:.0} s (it changes during normal play; set [first_person] {} = false to silence it for good)",
                        FLAP_NAMES[k],
                        fp_logic::FLAP_MAX,
                        fp_logic::FLAP_WINDOW_S,
                        FLAP_CFG[k]
                    );
                }
            }
            Some(FpEvent::Blocked(r)) => {
                if i.mode_on {
                    log_i!("FP", "blocked reason={} {}", r.label(), detail(r));
                }
            }
            None => {}
        }

        if self.machine.is_on()
            && let (Some(p), Some(c)) = (p, cam)
        {
            self.apply(i, p, &c);
        } else {
            self.eye = None;
        }
        self.step_fade_restore(i.now);
        if in_world {
            self.probe_line(i.now, p, &w);
        }
        self.eye()
    }

    fn enter(&mut self, i: &TickIn, p: &FpPlayer, w: &FpWorld, c: &PersCam) {
        let cfg = i.cfg;
        self.owner = p.player;
        self.ticks_on = 0;
        self.cam_writes = 0;
        self.logged.clear();
        self.smoother.reset();
        self.yaw.reset();
        let _ = self.drift.take();
        self.last_yaw_written = None;
        self.pauses = [0; YawPause::ALL.len()];
        self.paused_now = None;
        self.prev_feet = None;
        self.last_drift_log = i.now;
        self.row3 = PersistWatch::default();
        self.rows = PersistWatch::default();
        self.fov = PersistWatch::default();
        self.load_edge.arm(w.load_counter);
        self.mask_edge.arm(w.camera_mask);
        self.talk_edge.arm(w.talk_id);
        self.fade_watch.arm(&w.fade, cfg.fade_alpha_max);
        self.hook_writes_at_on = cam_hook::WRITES.load(Ordering::Relaxed);
        self.hook_silent_warned = false;
        self.chr_writes_at_on = cam_hook::CHR_WRITES.load(Ordering::Relaxed);
        self.autorot_clears_at_on = cam_hook::AUTOROT_CLEARS.load(Ordering::Relaxed);
        self.wall_clears_at_on = cam_hook::WALL_CLEARS.load(Ordering::Relaxed);
        self.follow_calls_at_on = cam_hook::FOLLOW_CALLS.load(Ordering::Relaxed);
        if self.hook_mode() {
            // erfps2's camera handling, fixed for the whole episode (a config change applies from the next one).
            cam_hook::begin_episode(cfg.sync_chr_cam, cfg.disable_wall_recovery, cfg.disable_auto_rotation);
        }

        let fwd = V3::new(c.m[2][0], c.m[2][1], c.m[2][2]);
        let eye = eye_position(p.pos, fwd, cfg.eye_height_m, cfg.eye_forward_m);
        log_i!(
            "FP",
            "on eye={} cam_gate=ok er_cam_dist={} eye_to_chest={} hp={} ride={:?} mounted={} ladder={} camtype={:?} mask={} talk={:?} fade_max={} world_fov={:.1}deg near={:.4} mode={} hide={} player=0x{:x}{}",
            logfmt::v3(eye),
            i.er_pos.map_or("?".into(), |e| format!("{:.2}m", (e - eye).len())),
            i.chest.map_or("?".into(), |ch| format!("{:.2}m", (ch - eye).len())),
            p.hp,
            p.ride_state,
            p.is_mounted as u8,
            p.ladder,
            w.camera_type,
            w.camera_mask.map_or("?".into(), |m| format!("0x{m:x}")),
            w.talk_id,
            fade_max(&w.fade).map_or("n/a".into(), |a| format!("{a:.2}")),
            c.fov.to_degrees(),
            c.near,
            if self.hook_mode() { "hook" } else { "task" },
            cfg.hide_method.label(),
            p.player,
            // C6 scope peek: this episode is a third-person scoped ADS (it ends at ADS release, reason f6)
            if i.scope_peek && !self.wanted { " peek=1" } else { "" }
        );
        if !w.fade_available && cfg.use_fade_signal && self.logged.insert("fade_unavailable") {
            log_w!("FP", "WARN CSFade not found: the fade (cutscene) signal is unavailable this episode");
        }

        // Hide the body (local player only). The hide fields are re-read right before the write so a change the
        // engine made earlier in this tick is not undone (review M3b minor 4).
        let restoring = self.fade_restoring.take();
        let fresh = fresh_hide(p.player);
        match (cfg.hide_method, fresh) {
            (HideMethod::None, _) => {}
            (_, None) => log_w!("FP", "WARN hide skipped: main_player 0x{:x} hide fields unreadable right before the write; the body stays visible", p.player),
            (HideMethod::RenderFlag, Some(f)) => {
                let (rb, v) = RenderBit::enter(p.player, f.flags1c5);
                // SAFETY: main_player's ChrIns flags byte (read just now); only bit 3 differs from the current value.
                if unsafe { write(p.player + FLAGS1C5, v) } {
                    log_i!(
                        "FP",
                        "hide write player=0x{:x} field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) addr=0x{:x} old=0x{:02x} new=0x{:02x}",
                        p.player,
                        p.player + FLAGS1C5,
                        f.flags1c5,
                        v
                    );
                    self.hidden = Some(Hidden::Bit(rb));
                } else {
                    log_w!("FP", "WARN hide write failed at 0x{:x} why={}; the body stays visible", p.player + FLAGS1C5, last_refusal());
                }
            }
            (HideMethod::Fade, Some(f)) => {
                // A fade-in still running holds our own +1.0: carry its saved original over (review M3b minor 7).
                let (fh, v) = FadeHide::enter_after(p.player, f.base_transparency, f.base_transparency_modifier, restoring);
                // SAFETY: main_player's base_transparency_modifier f32 (read just now).
                if unsafe { write(p.player + BASE_TRANSPARENCY_MOD, v) } {
                    log_i!(
                        "FP",
                        "hide write player=0x{:x} field=ChrIns.base_transparency_modifier(+0x250) addr=0x{:x} old={} new={} saved={} (base_transparency={})",
                        p.player,
                        p.player + BASE_TRANSPARENCY_MOD,
                        f.base_transparency_modifier,
                        v,
                        fh.saved_modifier,
                        f.base_transparency
                    );
                    self.hidden = Some(Hidden::Fade(fh));
                } else {
                    log_w!("FP", "WARN hide write failed at 0x{:x} why={}; the body stays visible", p.player + BASE_TRANSPARENCY_MOD, last_refusal());
                }
            }
        }
        if !matches!(self.hidden, Some(Hidden::Fade(_))) {
            // no new fade hide took over the field: keep finishing the earlier fade-in
            self.fade_restoring = restoring;
        }
    }

    /// While On: the eye and the camera write (or the hook's inputs), the hide re-check, yaw, periodic log.
    fn apply(&mut self, i: &TickIn, p: &FpPlayer, c: &PersCam) {
        let cfg = i.cfg;
        self.ticks_on += 1;
        let fwd = V3::new(c.m[2][0], c.m[2][1], c.m[2][2]);
        // M5: while the controller drives, the eye sits on its feet at the stance eye height (section 1.1 Fix A)
        let eye_h = i.move_eye_h.unwrap_or(cfg.eye_height_m);
        let mut eye = eye_position(i.move_feet.unwrap_or(p.pos), fwd, eye_h, cfg.eye_forward_m);
        eye.y = self.smoother.step(eye.y, i.dt, cfg.eye_smooth_s);
        self.eye = Some(eye);

        if self.hook_mode() {
            cam_hook::CAM_ADDR.store(c.addr, Ordering::Relaxed);
            cam_hook::CSCAMERA_ADDR.store(cscamera_addr(), Ordering::Relaxed);
            cam_hook::PHYS_ADDR.store(p.physics, Ordering::Relaxed);
            cam_hook::EYE_H_BITS.store(eye_h.to_bits(), Ordering::Relaxed);
            cam_hook::EYE_FWD_BITS.store(cfg.eye_forward_m.to_bits(), Ordering::Relaxed);
            let wcm = world_chr_man_addr();
            cam_hook::WCM_ADDR.store(wcm, Ordering::Relaxed);
            // SAFETY: non-faulting read of WorldChrMan.chr_cam (0 when absent).
            cam_hook::CHR_CAM_ADDR.store(unsafe { read_chr_cam_ptr(wcm) }, Ordering::Relaxed);
            cam_hook::ACTIVE.store(true, Ordering::SeqCst);
            self.log_follow_writes();
            let writes = cam_hook::WRITES.load(Ordering::Relaxed);
            if writes > self.hook_writes_at_on && !self.hook_ok_logged {
                self.hook_ok_logged = true;
                let (er, e) = cam_hook::er_pos_and_eye();
                log_i!(
                    "FPCAM",
                    "write field=CSCamera.pers_cam_1+0x40 addr=0x{:x} old={} new={} (camera-step hook)",
                    c.addr + CAM_ROW3,
                    logfmt::v3(V3::new(er[0], er[1], er[2])),
                    logfmt::v3(V3::new(e[0], e[1], e[2]))
                );
            }
            if writes == self.hook_writes_at_on && self.ticks_on >= HOOK_SILENT_TICKS && !self.hook_silent_warned {
                self.hook_silent_warned = true;
                log_w!("FPHOOK", "WARN no write in {} ticks while On (skips={})", self.ticks_on, cam_hook::SKIPS.load(Ordering::Relaxed));
            }
        } else {
            let new3 = [eye.x, eye.y, eye.z, c.m[3][3]];
            // SAFETY: pers_cam_1 (read this tick); row 3 is the position the engine rebuilds every frame.
            if unsafe { write(c.addr + CAM_ROW3, new3) } {
                self.row3.wrote(new3);
                self.cam_writes += 1;
                let old = c.m[3];
                self.log_write_once("row3", || {
                    format!(
                        "write field=CSCamera.pers_cam_1+0x40 addr=0x{:x} old={} new={}",
                        c.addr + CAM_ROW3,
                        logfmt::v3(V3::new(old[0], old[1], old[2])),
                        logfmt::v3(eye)
                    )
                });
            } else {
                self.pending_fail = Some("CSCamera.pers_cam_1+0x40");
            }
            // FPS pitch clamp (a no-op while ER's own limit is narrower).
            let rows3 = [[c.m[0][0], c.m[0][1], c.m[0][2]], [c.m[1][0], c.m[1][1], c.m[1][2]], [c.m[2][0], c.m[2][1], c.m[2][2]]];
            self.clamped = false;
            if let Some(n) = clamp_pitch(rows3, cfg.pitch_clamp_deg) {
                let new_rows = [0, 1, 2].map(|r| [n[r][0], n[r][1], n[r][2], c.m[r][3]]);
                // SAFETY: rows 0..2 of the same CSCam, 48 bytes of f32.
                if unsafe { write(c.addr + CAM_ROWS, new_rows) } {
                    self.rows.wrote(new_rows);
                    self.clamped = true;
                    let before = pitch_deg(fwd);
                    self.log_write_once("rows", || {
                        format!(
                            "write field=CSCamera.pers_cam_1+0x10..0x40 (pitch clamp) addr=0x{:x} old_pitch={before:.1}deg new_pitch={:.1}deg",
                            c.addr + CAM_ROWS,
                            cfg.pitch_clamp_deg.copysign(before)
                        )
                    });
                } else {
                    self.pending_fail = Some("CSCamera.pers_cam_1+0x10");
                }
            }
        }
        // World FOV override (both modes; default off).
        if cfg.world_fov_deg > 0.0 {
            let v = cfg.world_fov_deg.to_radians();
            // SAFETY: the f32 fov of pers_cam_1 (read this tick).
            if unsafe { write(c.addr + CAM_FOV, v) } {
                self.fov.wrote(v);
                let old = c.fov;
                self.log_write_once("fov", || {
                    format!(
                        "write field=CSCamera.pers_cam_1+0x50 (fov) addr=0x{:x} old={old:.4}({:.1}deg) new={v:.4}({:.1}deg)",
                        c.addr + CAM_FOV,
                        old.to_degrees(),
                        cfg.world_fov_deg
                    )
                });
            } else {
                self.pending_fail = Some("CSCamera.pers_cam_1+0x50");
            }
        }

        self.hide_tick(p);
        self.yaw_step(i, p, fwd);

        if self.ticks_on % cfg.log_every_ticks.max(1) as u64 == 0 {
            // erfps2-style camera handling state (hook mode): chr cam writes, follow-cam hook calls/clears and the
            // live values of the two bytes it keeps at 0 (both must read 0 while On when switched on).
            let (autorot, wall) = cam_hook::live_bytes();
            let byte = |b: Option<u8>| b.map_or("?".to_string(), |v| v.to_string());
            log_i!(
                "FPCAM",
                "tick={} eye_to_er_cam={} eye_to_chest={} pitch={:.1}deg clamp={} near={:.4} world_fov={:.1}deg writes={} mode={} hook_writes={} hook_skips={} hide_reasserts={} chr_cam_writes={} chr_cam_skips={} chr_cam_persisted={} follow_hook={} follow_calls={} follow_skips={} autorot_clears={} autorot={} wall_clears={} wall_recovery={} wall_addr={}",
                self.ticks_on,
                i.er_pos.map_or("?".into(), |e| format!("{:.2}m", (e - eye).len())),
                i.chest.map_or("?".into(), |ch| format!("{:.2}m", (ch - eye).len())),
                pitch_deg(fwd),
                self.clamped as u8,
                c.near,
                c.fov.to_degrees(),
                self.cam_writes,
                if self.hook_mode() { "hook" } else { "task" },
                cam_hook::WRITES.load(Ordering::Relaxed),
                cam_hook::SKIPS.load(Ordering::Relaxed),
                self.reasserts(),
                cam_hook::CHR_WRITES.load(Ordering::Relaxed),
                cam_hook::CHR_SKIPS.load(Ordering::Relaxed),
                cam_hook::CHR_PERSISTED.load(Ordering::Relaxed),
                cam_hook::FOLLOW_INSTALLED.load(Ordering::Relaxed) as u8,
                cam_hook::FOLLOW_CALLS.load(Ordering::Relaxed),
                cam_hook::FOLLOW_SKIPS.load(Ordering::Relaxed),
                cam_hook::AUTOROT_CLEARS.load(Ordering::Relaxed),
                byte(autorot),
                cam_hook::WALL_CLEARS.load(Ordering::Relaxed),
                byte(wall),
                if cam_hook::WALL_ADDR.load(Ordering::Relaxed) != 0 { "ok" } else { "none" }
            );
        }
    }

    /// Hook mode: logs the first write of the episode of each erfps2-style field (the detours cannot log): old = the
    /// value before our first write, new = ours.
    fn log_follow_writes(&mut self) {
        if cam_hook::CHR_WRITES.load(Ordering::Relaxed) > self.chr_writes_at_on {
            let (er, eye) = cam_hook::chr_er_and_eye();
            let chr = cam_hook::CHR_CAM_ADDR.load(Ordering::Relaxed);
            self.log_write_once("chr_row3", || {
                format!(
                    "write field=ChrCam.pers_cam+0x40 addr=0x{:x} old={} new={} (camera-step hook; erfps2: eye in both cameras)",
                    chr + CHR_CAM_ROW3,
                    logfmt::v3(V3::new(er[0], er[1], er[2])),
                    logfmt::v3(V3::new(eye[0], eye[1], eye[2]))
                )
            });
        }
        if cam_hook::FOLLOW_CALLS.load(Ordering::Relaxed) > self.follow_calls_at_on {
            let (a, w) = cam_hook::saved_bytes();
            let fc = cam_hook::live_follow_cam();
            if cam_hook::AUTOROT_CLEARS.load(Ordering::Relaxed) > self.autorot_clears_at_on {
                self.log_write_once("autorot", || {
                    format!(
                        "write field=ChrExFollowCam.camera_auto_rotation(+0x240) addr=0x{:x} old={} new=0 (follow-cam hook; restored on handback)",
                        fc + FOLLOW_CAM_AUTO_ROTATION,
                        a.map_or("?".into(), |v| v.to_string())
                    )
                });
            } else if a == Some(0) {
                self.log_write_once("autorot", || "follow-cam hook: camera_auto_rotation already 0 (ER option off); nothing written".into());
            }
            if cam_hook::WALL_CLEARS.load(Ordering::Relaxed) > self.wall_clears_at_on {
                self.log_write_once("wall", || {
                    format!(
                        "write field=CAM_WALL_RECOVERY(exe+0x{:x}) addr=0x{:x} old={} new=0 (follow-cam hook; restored on handback)",
                        cam_hook::cam_wall_recovery_rva(),
                        cam_hook::WALL_ADDR.load(Ordering::Relaxed),
                        w.map_or("?".into(), |v| v.to_string())
                    )
                });
            } else if w == Some(0) {
                self.log_write_once("wall", || "follow-cam hook: CAM_WALL_RECOVERY already 0; nothing written".into());
            }
        }
    }

    fn reasserts(&self) -> u32 {
        match &self.hidden {
            Some(Hidden::Bit(b)) => b.reasserts,
            Some(Hidden::Fade(f)) => f.reasserts,
            None => 0,
        }
    }

    /// Re-checks the hide with a fresh read of the field right before any re-write (review M3b minor 4: the byte
    /// also holds death_flag b7 and is_invincible b4; only bit 3 is ever changed, from the value read just now).
    fn hide_tick(&mut self, p: &FpPlayer) {
        let Some(f) = fresh_hide(p.player) else { return };
        match self.hidden.as_mut() {
            Some(Hidden::Bit(b)) => {
                if let BitTick::Reassert(v) = b.tick(f.player, f.flags1c5) {
                    // SAFETY: the same saved ChrIns flags byte (owner checked by tick()), read just now.
                    let ok = unsafe { write(f.player + FLAGS1C5, v) };
                    if b.reasserts == 1 {
                        log_i!("FP", "hide reasserted count=1 (first; the engine set enable_render again: 0x{:02x} -> 0x{v:02x}, write ok={})", f.flags1c5, ok as u8);
                    }
                }
            }
            Some(Hidden::Fade(h)) => {
                if let Ok(Some(v)) = h.tick(f.player, f.base_transparency_modifier) {
                    // SAFETY: the same saved ChrIns modifier field, read just now.
                    let ok = unsafe { write(f.player + BASE_TRANSPARENCY_MOD, v) };
                    if h.reasserts == 1 {
                        log_i!("FP", "hide reasserted count=1 (first; base_transparency_modifier was {}, write ok={})", f.base_transparency_modifier, ok as u8);
                    }
                }
            }
            None => {}
        }
    }

    fn yaw_step(&mut self, i: &TickIn, p: &FpPlayer, fwd: V3) {
        if !i.cfg.yaw_follow || self.probe.gave_up() {
            return;
        }
        // C3 PASS B: the pad's left stick counts like W/A/S/D
        let keys = RAW_MOVE_KEYS.load(Ordering::Relaxed) | crate::pad::move_bits();
        // Planar velocity of the feet (this tick's delta); jumps are re-basing, not movement.
        let vel = match self.prev_feet {
            Some(prev) if i.dt > 0.0 && (p.pos - prev).planar_len() <= REBASE_JUMP_M => (p.pos - prev) * (1.0 / i.dt),
            _ => V3::ZERO,
        };
        self.prev_feet = Some(p.pos);
        let Some(h) = horizontal_forward(fwd) else { return };
        let gate = yaw_gate(&YawGateIn {
            lateral_keys: keys & (MOVE_A | MOVE_S | MOVE_D) != 0,
            // An unreadable action-flag module counts as "turn locked": no write.
            action_flags: p.action_flags.unwrap_or(1 << AF_DISABLE_TURNING),
            locked_on: p.locked_on,
            throw_active: fp_logic::in_throw(p.throw_state),
            // gamepad stick / remapped keys: ER is steering the character away from the camera heading
            off_heading: off_heading(vel, h),
            // M5: the controller moves the player; A/S/D and the strafe heading are ours, not ER steering. C3 PASS C:
            // with `fp_anim = "hybrid"` ER sees A/S/D again and turns the body to run that way (peers see it): pause
            controller_driving: i.move_feet.is_some() && !crate::movement::FP_HYBRID_ACTIVE.load(Ordering::Relaxed),
        });
        // M5: a fixed forward axis (`[first_person] yaw_forward_axis`, default -z from run 4) while the probe has not
        // resolved; the probe only learns while ER moves the player (`auto` keeps the M3b probe alone).
        let fixed = match i.cfg.yaw_forward_axis {
            YawAxisCfg::MinusZ => Some(Axis::MinusZ),
            YawAxisCfg::PlusZ => Some(Axis::PlusZ),
            YawAxisCfg::Auto => None,
        };
        let Some(axis) = self.probe.result().or(fixed) else {
            if gate.is_ok() && keys == MOVE_W && i.planar_speed > PROBE_MIN_SPEED_MPS && i.move_feet.is_none() {
                match self.probe.feed(fwd, p.orientation) {
                    Some(ProbeEvent::Resolved { axis, dot_med, n }) => {
                        log_i!("FP", "yaw probe conv={} dot_med={dot_med:.3} n={n}", axis.label())
                    }
                    Some(ProbeEvent::Failed { dot_med, n, attempt }) => {
                        log_w!("FP", "yaw probe FAIL dot_med={dot_med:.3} n={n} attempt={attempt} (retrying)")
                    }
                    Some(ProbeEvent::GaveUp) => log_w!("FP", "yaw probe FAIL: gave up after 3 attempts; yaw follow off for this session"),
                    None => {}
                }
            }
            return;
        };
        match gate {
            Err(pause) => {
                let idx = YawPause::ALL.iter().position(|x| *x == pause).unwrap_or(0);
                self.pauses[idx] += 1;
                self.last_yaw_written = None; // the game may legitimately turn the body now
                if self.paused_now != Some(pause) {
                    self.paused_now = Some(pause);
                }
            }
            Ok(()) => {
                self.paused_now = None;
                if let (Some(last), Some(cur)) = (self.last_yaw_written, yaw_from_quat(p.orientation, axis)) {
                    self.drift.push(angle_diff_deg(cur, last));
                }
                let yaw = yaw_of(h, axis);
                let q = quat_from_yaw(yaw);
                // One 32-byte write covers orientation (+0x50) and interpolated_orientation (+0x60): run 5's ~70 ms
                // yaw ticks were these two writes at ~35 ms each (region query per write, now cached per page).
                // SAFETY: the player's physics module (read this tick); two adjacent XYZW quaternions the game
                // rewrites itself (adjacency asserted at compile time).
                let ok = unsafe { write(p.physics + ORIENTATION, yaw_pair(q)) };
                if ok {
                    if self.yaw.record(p.orientation, p.interpolated) {
                        log_i!(
                            "FP",
                            "yaw write field=CSChrPhysicsModule.orientation addr=0x{:x} old={:?} new={:?} conv={}",
                            p.physics + ORIENTATION,
                            p.orientation,
                            q,
                            axis.label()
                        );
                        log_i!(
                            "FP",
                            "yaw write field=CSChrPhysicsModule.interpolated_orientation addr=0x{:x} old={:?} new={:?}",
                            p.physics + INTERP_ORIENTATION,
                            p.interpolated,
                            q
                        );
                    }
                    self.last_yaw_written = Some(yaw);
                } else {
                    self.pending_fail = Some("CSChrPhysicsModule.orientation");
                }
            }
        }
        if i.now - self.last_drift_log >= DRIFT_LOG_EVERY_S {
            self.last_drift_log = i.now;
            self.log_drift("");
        }
    }

    fn log_drift(&mut self, suffix: &str) {
        let paused: u64 = self.pauses.iter().sum();
        let causes: Vec<String> = YawPause::ALL.iter().zip(self.pauses).map(|(p, n)| format!("{}={n}", p.label())).collect();
        match self.drift.take() {
            Some((p50, p95, n)) => log_i!(
                "FP",
                "yaw drift p50={p50:.2}deg p95={p95:.2}deg n={n} writes={} paused={paused} ({}){suffix}",
                self.yaw.writes,
                causes.join(" ")
            ),
            None if self.yaw.writes > 0 || paused > 0 => {
                log_i!("FP", "yaw drift n=0 writes={} paused={paused} ({}){suffix}", self.yaw.writes, causes.join(" "))
            }
            None => {}
        }
        self.pauses = [0; YawPause::ALL.len()];
    }

    /// Exact restore of everything first person changed, then the reason line.
    fn leave(&mut self, r: Reason, detail: &str, now: f64, in_world: bool, cam: Option<&PersCam>) {
        // Camera. Hook mode: stop the detours (waiting for one in flight), then restore ChrCam row 3, the follow
        // cam's camera_auto_rotation and CAM_WALL_RECOVERY (cam_hook::handback), and pers_cam_1 row 3 here.
        let was_active = cam_hook::ACTIVE.load(Ordering::SeqCst);
        let restored = if cam_hook::INSTALLED.load(Ordering::SeqCst) {
            cam_hook::handback()
        } else {
            cam_hook::ACTIVE.store(false, Ordering::SeqCst);
            Vec::new()
        };
        if was_active && let Some(c) = cam {
            // This frame's camera step already put the eye in; write ER's saved position back now (the frame has
            // not been drawn yet), only if the row still holds our eye.
            let (er, eye) = cam_hook::er_pos_and_eye();
            if let Some(v) = hook_row_restore(c.m[3], eye, er, true) {
                // SAFETY: pers_cam_1 row 3 (read this tick).
                let ok = unsafe { write(c.addr + CAM_ROW3, v) };
                log_i!(
                    "FP",
                    "restore field=CSCamera.pers_cam_1+0x40 old={} new={} ok={} (hook)",
                    logfmt::v3(V3::new(eye[0], eye[1], eye[2])),
                    logfmt::v3(V3::new(v[0], v[1], v[2])),
                    ok as u8
                );
            }
        }
        for x in &restored {
            match x.ok {
                Some(ok) => log_i!("FP", "restore field={} addr=0x{:x} old={} new={} ok={} (hook)", x.field, x.addr, x.old, x.new, ok as u8),
                None => log_i!("FP", "restore field={} addr=0x{:x} skipped: current={} -> {}", x.field, x.addr, x.old, x.new),
            }
        }
        let restores: [(&str, usize); 3] = [("CSCamera.pers_cam_1+0x40", CAM_ROW3), ("CSCamera.pers_cam_1+0x10", CAM_ROWS), ("CSCamera.pers_cam_1+0x50", CAM_FOV)];
        let r3 = self.row3.stop();
        let rr = self.rows.stop();
        let rf = self.fov.stop();
        if let Some(c) = cam.filter(|_| in_world) {
            // SAFETY (all three): fields of pers_cam_1 (read this tick), the exact values ER had before our write.
            if let Some(v) = r3 {
                let ok = unsafe { write(c.addr + restores[0].1, v) };
                log_i!("FP", "restore field={} old={} new={} ok={}", restores[0].0, logfmt::v3(V3::new(c.m[3][0], c.m[3][1], c.m[3][2])), logfmt::v3(V3::new(v[0], v[1], v[2])), ok as u8);
            }
            if let Some(v) = rr {
                let ok = unsafe { write(c.addr + restores[1].1, v) };
                log_i!("FP", "restore field={}..0x40 (pitch clamp rows) ok={}", restores[1].0, ok as u8);
            }
            if let Some(v) = rf {
                let ok = unsafe { write(c.addr + restores[2].1, v) };
                log_i!("FP", "restore field={} old={:.4} new={v:.4} ok={}", restores[2].0, c.fov, ok as u8);
            }
        } else if r3.is_some() || rr.is_some() || rf.is_some() {
            log_w!("FP", "restore of pers_cam_1 skipped: not in world (the next camera is ER's)");
        }

        // Body: its own fresh read of main_player + the hide fields (deferred only when THAT read fails).
        if let Some(h) = self.hidden.take() {
            self.pending_hidden = self.restore_hidden(h, r.label(), now, true);
        }

        // Yaw: release policy (nothing is written back).
        if self.yaw.writes > 0 || self.pauses.iter().any(|n| *n > 0) {
            self.log_drift(" (episode end; release: facing not written back)");
        }
        debug_assert!(self.yaw.restore().is_none());

        self.load_edge.clear();
        self.mask_edge.clear();
        self.talk_edge.clear();
        self.fade_watch.clear();
        self.eye = None;
        log_i!(
            "FP",
            "off reason={} {detail} ticks_on={} cam_writes={} ({})",
            r.label(),
            self.ticks_on,
            self.cam_writes,
            if r.waits_for_user() { "waits for the user" } else { "re-arms when clear" }
        );
    }

    /// Restores the hidden body with a fresh read of `main_player` and the hide fields taken right before the write
    /// (independent of the full first-person read, review M3b major 2). Returns the model back when that read fails
    /// (not in world / unreadable) or the write fails: the next tick retries. A different character in
    /// `main_player` drops it (no write).
    fn restore_hidden(&mut self, h: Hidden, reason: &str, now: f64, log_defer: bool) -> Option<Hidden> {
        let Some(f) = read_hide_fields() else {
            if log_defer {
                let owner = match &h {
                    Hidden::Bit(b) => b.owner,
                    Hidden::Fade(f) => f.owner,
                };
                log_i!("FP", "hide restore deferred player=0x{owner:x}: main_player/hide fields not readable now (reason={reason}); retried every tick");
            }
            return Some(h);
        };
        match h {
            Hidden::Bit(b) => match b.restore(f.player, f.flags1c5) {
                Some(v) => {
                    // SAFETY: the same ChrIns flags byte whose value we saved (owner checked by restore()), read just
                    // now; only bit 3 differs from the current value.
                    let ok = v == f.flags1c5 || unsafe { write(f.player + FLAGS1C5, v) };
                    if !ok {
                        if log_defer {
                            log_w!("FP", "WARN hide restore write failed at 0x{:x} why={}; retried every tick", f.player + FLAGS1C5, last_refusal());
                        }
                        return Some(Hidden::Bit(b));
                    }
                    log_i!(
                        "FP",
                        "hide restore player=0x{:x} field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) old=0x{:02x} new=0x{v:02x} saved=0x{:02x} reasserted={} ok=1 reason={reason}",
                        f.player,
                        f.flags1c5,
                        b.saved,
                        b.reasserts
                    );
                }
                None => log_i!(
                    "FP",
                    "hide restore skipped player=0x{:x}: main_player is now 0x{:x} (a new character; reasserted={} reason={reason}); nothing written",
                    b.owner,
                    f.player,
                    b.reasserts
                ),
            },
            Hidden::Fade(fh) => match fh.begin_restore(f.player, now) {
                Some((fr, v)) => {
                    // SAFETY: the same ChrIns modifier field whose value we saved, read just now.
                    let ok = unsafe { write(f.player + BASE_TRANSPARENCY_MOD, v) };
                    if !ok {
                        if log_defer {
                            log_w!("FP", "WARN hide restore write failed at 0x{:x} why={}; retried every tick", f.player + BASE_TRANSPARENCY_MOD, last_refusal());
                        }
                        return Some(Hidden::Fade(fh));
                    }
                    log_i!(
                        "FP",
                        "hide restore player=0x{:x} field=ChrIns.base_transparency_modifier(+0x250) old={} new={v} (fading in; saved {} follows) reasserted={} ok=1 reason={reason}",
                        f.player,
                        f.base_transparency_modifier,
                        fh.saved_modifier,
                        fh.reasserts
                    );
                    self.fade_restoring = Some(fr);
                }
                None => log_i!(
                    "FP",
                    "hide restore skipped player=0x{:x}: main_player is now 0x{:x} (reason={reason}); nothing written",
                    fh.owner,
                    f.player
                ),
            },
        }
        None
    }

    fn step_fade_restore(&mut self, now: f64) {
        let Some(fr) = self.fade_restoring else { return };
        // Unreadable (loading): keep it and try again next tick; a new character drops it (step() -> Drop).
        let Some(f) = read_hide_fields() else { return };
        match fr.step(f.player, f.base_transparency, now) {
            FadeStep::Wait => {}
            FadeStep::Done(v) => {
                // SAFETY: the same ChrIns modifier field (owner checked by step()), read just now.
                let ok = unsafe { write(f.player + BASE_TRANSPARENCY_MOD, v) };
                log_i!(
                    "FP",
                    "hide restore done field=ChrIns.base_transparency_modifier(+0x250) old={} new={v} base_transparency={} ok={}",
                    f.base_transparency_modifier,
                    f.base_transparency,
                    ok as u8
                );
                self.fade_restoring = None;
            }
            FadeStep::Drop => {
                log_i!("FP", "hide restore dropped: the character changed before the fade-in finished; nothing written");
                self.fade_restoring = None;
            }
        }
    }

    /// Panic path only (no tick follows): a fade-in cannot be waited for. If the body is already visible the saved
    /// modifier goes back now; otherwise the modifier is left at +1.0 so the body still fades in (logged).
    fn finish_fade_restore_now(&mut self) {
        let Some(fr) = self.fade_restoring.take() else { return };
        let Some(f) = fresh_hide(fr.owner) else {
            log_w!("FP", "WARN panic: fade restore of player=0x{:x} not finished (character not readable or changed); nothing written", fr.owner);
            return;
        };
        if f.base_transparency >= mw2er_core::fp_restore::FADE_VISIBLE_AT {
            // SAFETY: the same ChrIns modifier field (owner checked), read just now.
            let ok = unsafe { write(f.player + BASE_TRANSPARENCY_MOD, fr.saved_modifier) };
            log_i!("FP", "hide restore done (panic) field=ChrIns.base_transparency_modifier(+0x250) new={} ok={}", fr.saved_modifier, ok as u8);
        } else {
            log_w!(
                "FP",
                "WARN panic: fade-in still running (base_transparency={}); modifier left at {} so the body fades in; saved {} not written",
                f.base_transparency,
                f.base_transparency_modifier,
                fr.saved_modifier
            );
        }
    }

    /// Raw values of every inferred signal, logged on change (rate-limited) so one run maps them.
    fn probe_line(&mut self, now: f64, p: Option<&FpPlayer>, w: &FpWorld) {
        if now - self.last_probe_t < PROBE_LINE_MIN_S {
            return;
        }
        let s = match p {
            Some(p) => format!(
                "ride={:?} mounting={} mounted={} ladder={} throw={:?} lockon={} hp_le0={} death_flag={} camtype={:?} mask={} talk={:?} load={:?} pause_bit={:?} fade_max={}",
                p.ride_state,
                p.is_mounting as u8,
                p.is_mounted as u8,
                p.ladder,
                p.throw_state,
                p.locked_on as u8,
                (p.hp <= 0) as u8,
                (p.flags1c5 & fp_logic::DEATH_FLAG != 0) as u8,
                w.camera_type,
                w.camera_mask.map_or("?".into(), |m| format!("0x{m:x}")),
                w.talk_id,
                w.load_counter,
                p.pause_bit,
                fade_max(&w.fade).map_or("n/a".into(), |a| format!("{:.1}", a))
            ),
            None => return,
        };
        if s != self.last_probe {
            log_i!("FP", "probe {s}");
            self.last_probe = s;
            self.last_probe_t = now;
        }
    }

    /// Panic path: restore everything with fresh reads (called under its own guard after the task panicked). No tick
    /// follows, so a deferred hide restore and an unfinished fade-in are resolved here too, whatever the machine
    /// state (review M3b minor 3).
    pub fn emergency_off(&mut self, now: f64) {
        if self.machine.force_off(Reason::Panic) {
            let cam = read_pers_cam1();
            let in_world = read_hide_fields().is_some();
            self.leave(Reason::Panic, "task panicked", now, in_world, cam.as_ref());
        } else if cam_hook::INSTALLED.load(Ordering::SeqCst) {
            // not On: still stop the detours and give back anything they changed
            for x in cam_hook::handback() {
                log_i!("FP", "restore field={} addr=0x{:x} old={} new={} ok={:?} (panic)", x.field, x.addr, x.old, x.new, x.ok);
            }
        } else {
            cam_hook::ACTIVE.store(false, Ordering::SeqCst);
        }
        if let Some(h) = self.pending_hidden.take() {
            self.pending_hidden = self.restore_hidden(h, "panic", now, true);
            if self.pending_hidden.is_some() {
                log_w!("FP", "WARN panic: the hidden body could not be restored (main_player not readable); a reload of the area shows it again");
            }
        }
        self.finish_fade_restore_now();
    }
}

/// The hide fields of `main_player`, read now, only if it is still `owner`.
fn fresh_hide(owner: usize) -> Option<HideFields> {
    read_hide_fields().filter(|f| f.player == owner)
}

fn last_refusal() -> &'static str {
    refusal_label(LAST_WRITE_REFUSAL.load(Ordering::Relaxed))
}

/// One tick of an edge latch; logs a release. Returns true while it holds the trigger on.
fn latch_tick<T: Copy + PartialEq + std::fmt::Debug>(
    l: &mut EdgeLatch<T>,
    enabled: bool,
    name: &str,
    current: Option<T>,
    menu_open: bool,
    now: f64,
    p: LatchParams,
) -> bool {
    if !enabled {
        if l.is_latched() {
            l.reset();
            log_i!("FP", "latch dropped signal={name} (switched off in the config or muted)");
        }
        return false;
    }
    let age = l.age(now);
    match l.step(current, menu_open, now, p) {
        LatchStep::Hold => true,
        LatchStep::Released(r) => {
            let why = match r {
                LatchRelease::Returned => "returned",
                // C3: the talk latch's stale release is the dialog closing (cursor and dialog index closed)
                LatchRelease::Stale if name == FLAP_NAMES[FLAP_TALK] => "closed",
                LatchRelease::Stale => "stale",
            };
            log_i!("FP", "latch released signal={name} why={why} after {age:.1}s value={current:?}");
            false
        }
        LatchStep::Idle => false,
    }
}
