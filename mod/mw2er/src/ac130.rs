//! The M4 AC-130 killstreak controller (docs/M4_DESIGN.md). Runs once per task tick on the game's main thread.
//!
//! Per tick: AC-130 keys -> signals -> the pure state machine (`mw2er_core::ac130_logic`) -> act:
//! - F7 in Idle: the start gate (11 ordered checks, the open-sky ray last); on a pass the protection SpEffect is
//!   applied through the engine and the ride waits (Starting) until the effect is seen in the player's list;
//! - Active: camera mode 2 published for the camera-step detour, input captured, ER HUD hidden, thermal on, the
//!   three guns, shots ("Route A": map ray + capsule test from the gunship camera, MW2 flight time), explosions
//!   spawned as the regulation rows at the impact, sounds, periodic logs;
//! - every exit goes through [`Ac130Ctl::end`] in the fixed order of section 2.4, then the protection is removed and
//!   verified absent, then the cooldown runs.
//!
//! Every game write goes through `game::write` (the camera's 17 floats are written by the detour; here: the camera
//! restore and `CSFeManImp.hud_state`), logged once per ride with old/new and restored only if the field still holds
//! our value. The protection's two engine calls ([`apply_speffect`] / [`remove_speffect`]) are the only other path
//! and live only in this file.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use eldenring::cs::{ChrIns, ChrInsExt, FieldInsHandle, FieldInsType};
use mw2er_core::ac130_aim::{Blast, Via, attribute, first_hit, flight_time};
use mw2er_core::ac130_audio::{Cue, explosion_gain, pick};
use mw2er_core::ac130_cam::{Orbit, er_fov, max_dist, theta0_behind};
use mw2er_core::ac130_guns::{GunEvent, GunSpec, Guns, SwitchVia};
use mw2er_core::ac130_hud::{indicator_text, timer_text};
use mw2er_core::ac130_logic::{
    AbortMerge, Ac130Machine, Block, EndPlan, Event, ExitIn, GateIn, Indicator, Params, Phase, Reason, RideEdges, ShellTimes, indicator,
};
use mw2er_core::ac130_shake;
use mw2er_core::ac130_protect::{ACTIVE_CHECK_EVERY_TICKS, RemoveStep, RemoveTracker, calls_allowed, refresh_due};
use mw2er_core::ac130_tracer::{TracerRing, TracerShot, muzzle};
use mw2er_core::config::{BaseViewCfg, Config};
use mw2er_core::fp_logic::{self, FadeWatch};
use mw2er_core::input_logic::{CounterReader, wheel_steps};
use mw2er_core::logfmt;
use mw2er_core::move_input::PRESS_INTERACT;
use mw2er_core::rng::Rng;
use mw2er_core::thermal::{Film, film_at, pack};
use mw2er_core::vec3::V3;

use crate::cam_hook::{self, GunshipParams};
use crate::data::{AC_DATA, Ac130Data};
use crate::game::ac130_read::{HUD_HIDE_ALL, hud_state, main_player, speffect_count};
use crate::game::camera::{CameraSample, read_pers_cam1_17};
use crate::game::chars::ChrRead;
use crate::game::fp_read::{FpPlayer, FpWorld, cscamera_addr};
use crate::game::player::PlayerSample;
use crate::game::{bullets, havok, params, write};
use crate::log::{log_e, log_i, log_w};
use crate::shared::{
    self, AC_KEY_CALL, AC_KEY_EXIT, AC_KEY_GUN1, AC_KEY_NEXT, AC_KEY_THERMAL, AC_KEYS, AC_PRESSES, Ac130Hud, ESC_PRESSES, MOUSE_COUNTS,
    PRESSES_MOVE, RAW_FIRE_DOWN, WANT_GUNSHIP_INPUT, WHEEL, WRITES_ALLOWED,
};
use crate::spawn;

/// Ticks in world before F7 is accepted (the character and camera settle; same as first person).
const ARM_TICKS: u64 = 30;
/// More consecutive detour skips than this while Active end the ride (`cam_fail`).
const CAM_FAIL_SKIPS: u64 = 30;
/// A required read failing this many ticks in a row ends the ride (`read_fail`).
const READ_FAIL_TICKS: u32 = 3;
/// After a task panic (gameplay disabled), the guarded cleanup checks the protection / ER HUD this often (ticks).
const DISABLED_CHECK_EVERY_TICKS: u64 = 30;
/// HUD hide: ER rewriting hud_state this many ticks in a row makes us stop writing it.
const HUD_REWRITE_GIVE_UP: u32 = 10;
/// Explosions are kept this long for HIT attribution; the bullet scan runs this long after one.
const BLAST_KEEP_S: f64 = 2.0;
const SEEN_SCAN_S: f64 = 0.5;
const SPAWN_BREAKER: u32 = 8;
const SPAWN_BREAKER_COOLDOWN_S: f64 = 3.0;
/// HIT attribution window around an explosion and the sphere margin (metres).
const ATTR_BEFORE_S: f64 = 0.6;
const ATTR_AFTER_S: f64 = 0.15;
const ATTR_MARGIN_M: f32 = 0.5;

/// One task tick's inputs (already read by the task).
pub struct Ac130In<'a> {
    pub now: f64,
    pub cfg: &'a Config,
    pub mode_on: bool,
    pub in_world: bool,
    pub world_ticks: u64,
    pub menu_open: bool,
    pub foreground: bool,
    pub cam_valid: bool,
    /// ER's own camera (the gunship frame substituted away).
    pub cam: Option<CameraSample>,
    pub player: Option<&'a PlayerSample>,
    pub chars: &'a [ChrRead],
    /// First person's edge latches (loading, cutscene, talk).
    pub fp_latched: (bool, bool, bool),
    pub mouse_polled: bool,
    pub fp_on: bool,
    /// The first-person signal reads of this tick, read ONCE by the task and shared with first person (review M4
    /// minor 6). `Ok(None)` / `None` when not in world.
    pub fp_player: Result<Option<FpPlayer>, &'static str>,
    pub fp_world: Option<FpWorld>,
    /// M7: a Tactical Nuke sequence runs (F7 is refused).
    pub nuke_busy: bool,
    /// 2026-10-24: the task's death gate holds (`fp_logic::DeathGate`): the call-in is refused (`dead`) until the
    /// player is alive again after the respawn.
    pub player_dead: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Ac130Out {
    /// First person must hand back / stay off (fp_logic reason `ac130`).
    pub fp_block: bool,
    /// The UMP45 must not fire (ride running, or LMB not released since it ended).
    pub ump_block: bool,
    /// A ride is Active (PERF `ac130=1`).
    pub active: bool,
}

#[derive(Clone, Copy, Debug)]
struct Shell {
    gun: usize,
    impact: V3,
    dir: V3,
    w: f32,
    due: f64,
    dist: f32,
}

#[derive(Clone, Copy, Debug)]
struct Removal {
    tracker: RemoveTracker,
    reason: &'static str,
    cleanup: bool,
}

pub struct Ac130Ctl {
    data: Option<Arc<Ac130Data>>,
    machine: Ac130Machine,
    rows_ok: Option<bool>,
    readers: [CounterReader; AC_KEYS],
    esc_reader: CounterReader,
    wheel_acc: i32,
    rng: Rng,
    in_world: bool,
    cleanup_pending: bool,
    cfg_seen: Option<String>,
    // ride
    guns: Option<Guns>,
    center: V3,
    center_w: f32,
    start_camtype: Option<u32>,
    t_active: f64,
    hp_start: i32,
    hp_min: i32,
    moved_m: f32,
    move_warned: bool,
    fp_was_on: bool,
    shells: Vec<Shell>,
    /// Run 8: the rounds of this ride for the tracer pass (fixed ring, copied into the HUD snapshot).
    tracer_ring: TracerRing,
    blasts: Vec<Blast>,
    last_explosion: Option<f64>,
    shots: [u64; 3],
    explosions: u64,
    hits: u64,
    kills: u64,
    /// M6 run-13 bug 1: the exit edges, armed when a start passes the gate and cleared when the ride ends (the run-13
    /// stale load baseline ended every later F7 at once with `reason=loading t_s=0.0`).
    edges: RideEdges,
    /// 2026-10-21: the interact gate of the talk exit (fp_logic::TalkGate: a boss voice line is not a dialog) and its
    /// own reader on `PRESSES_MOVE[PRESS_INTERACT]`.
    talk_gate: fp_logic::TalkGate,
    interact_presses: CounterReader,
    /// Identical aborted starts merged into one line; the back-off line once per back-off.
    abort: AbortMerge,
    backoff_logged_until: f64,
    fade_watch: FadeWatch,
    read_fail_streak: u32,
    ticks_active: u64,
    protect_present: bool,
    last_writes: u64,
    last_skips: u64,
    skip_streak: u64,
    cam_write_logged: bool,
    writes_at_start: u64,
    persisted_at_start: u64,
    fov_persisted_at_start: u64,
    // ER HUD
    hud_saved: Option<(usize, u8)>,
    hud_rewrites: u32,
    hud_gave_up: bool,
    // protection
    removal: Option<Removal>,
    protect_chr: usize,
    /// When the protection was last applied (the finite effectEndurance is refreshed before it runs out).
    protect_applied_t: f64,
    /// ER HUD value still to give back (the end could not read CSFeManImp; retried on the next ticks).
    hud_pending: Option<(usize, u8)>,
    /// After a task panic: the protection still has to be verified absent (run by `disabled_tick`).
    panic_cleanup: bool,
    disabled_ticks: u64,
    // thermal
    black_hot: bool,
    film_from: Film,
    film_to: Film,
    film_t0: f64,
    film_dur: f64,
    // spawn breaker + explosion bullet scan
    spawn_failures: u32,
    breaker_until: f64,
    seen_ids: HashSet<i32>,
    // HUD / misc
    last_indicator_kind: u8,
    ready_toast_t: f64,
    pulse_t: f64,
    hit_t: f64,
    hit_kill: bool,
    agl_m: f32,
    agl_t: f64,
    ump_release_needed: bool,
    /// C3 PASS C: the explosions of this tick whose no-attack synced twin the task sends (row id, origin, dir).
    pending_twins: Vec<(i32, V3, V3)>,
}

/// The bullet owner must be a real character handle (the same rule as the UMP45 shots).
fn owner_ok(h: FieldInsHandle) -> Result<(), String> {
    if h.is_empty() {
        return Err("owner handle is empty".into());
    }
    match h.selector.field_ins_type() {
        Some(FieldInsType::Chr) => Ok(()),
        t => Err(format!("owner handle type {t:?} is not Chr")),
    }
}

/// Applies SpEffect `id` to `chr` through the engine (`dont_sync = true`). Refused before the version gate.
/// The one gated wrapper of the engine call: the AC-130 protection here, the M7 nuke's lethal effect (nuke.rs) on the
/// characters of its census and, last, on the player.
///
/// # Safety
/// `chr` must be a live ChrIns read this tick from the game's own lists (the local player, or an NPC the nuke re-read
/// with the same handle), on the game's main thread (CSTask).
pub(crate) unsafe fn apply_speffect(chr: usize, id: i32) -> bool {
    if !WRITES_ALLOWED.load(Ordering::SeqCst) || !crate::game::plausible(chr) {
        return false;
    }
    // SAFETY: the caller guarantees a live ChrIns on the game thread; the engine call manages its own SpEffect list
    // (vendored fromsoftware-rs ChrInsExt::apply_speffect, RVA 0x3e8dc0 on 2.7.1.0).
    let c = unsafe { &mut *(chr as *mut ChrIns) };
    c.apply_speffect(id, true);
    true
}

/// Removes SpEffect `id` from `chr` through the engine. Refused before the version gate.
///
/// # Safety
/// As [`apply_speffect`].
pub(crate) unsafe fn remove_speffect(chr: usize, id: i32) -> bool {
    if !WRITES_ALLOWED.load(Ordering::SeqCst) || !crate::game::plausible(chr) {
        return false;
    }
    // SAFETY: as above (ChrInsExt::remove_speffect, RVA 0x3ee2e0 on 2.7.1.0).
    let c = unsafe { &mut *(chr as *mut ChrIns) };
    c.remove_speffect(id);
    true
}

/// C3 PASS C (docs/C3_DESIGN.md 3.6 / 3.8): applies one of the co-op marker SpEffects with `dont_sync = false`, so
/// ER replicates it to the peers. Refused for every id that is not a C3 marker (`coop_vis::syncable_speffect`: never
/// the nuke's lethal 79000960, never the protection); [`apply_speffect`] keeps `dont_sync = true` for everything else.
///
/// # Safety
/// As [`apply_speffect`] (the local player only).
pub(crate) unsafe fn apply_speffect_synced(chr: usize, id: i32) -> bool {
    if !WRITES_ALLOWED.load(Ordering::SeqCst) || !crate::game::plausible(chr) || !mw2er_core::coop_vis::syncable_speffect(id) {
        return false;
    }
    // SAFETY: the caller guarantees a live ChrIns on the game thread (ChrInsExt::apply_speffect, RVA 0x3e8dc0).
    let c = unsafe { &mut *(chr as *mut ChrIns) };
    c.apply_speffect(id, false);
    true
}

fn fmt17(m: &[f32; 17]) -> String {
    format!("pos=({:.2},{:.2},{:.2}) fov={:.4}", m[12], m[13], m[14], m[16])
}

impl Ac130Ctl {
    /// C3 PASS C: the explosions waiting for their co-op twin (drained by the task every tick).
    pub fn take_twins(&mut self) -> Vec<(i32, V3, V3)> {
        std::mem::take(&mut self.pending_twins)
    }

    pub fn new(seed: u64) -> Self {
        let p = |i: usize| CounterReader::starting_at(AC_PRESSES[i].load(Ordering::SeqCst));
        Ac130Ctl {
            data: AC_DATA.get().cloned(),
            machine: Ac130Machine::default(),
            rows_ok: None,
            readers: [p(0), p(1), p(2), p(3), p(4), p(5), p(6)],
            esc_reader: CounterReader::starting_at(ESC_PRESSES.load(Ordering::SeqCst)),
            wheel_acc: 0,
            rng: Rng::new(seed ^ 0xAC13_0AC1_30AC_130A),
            in_world: false,
            cleanup_pending: false,
            cfg_seen: None,
            guns: None,
            center: V3::ZERO,
            center_w: 1.0,
            start_camtype: None,
            t_active: 0.0,
            hp_start: 0,
            hp_min: 0,
            moved_m: 0.0,
            move_warned: false,
            fp_was_on: false,
            shells: Vec::new(),
            tracer_ring: TracerRing::new(),
            blasts: Vec::new(),
            last_explosion: None,
            shots: [0; 3],
            explosions: 0,
            hits: 0,
            kills: 0,
            edges: RideEdges::default(),
            talk_gate: fp_logic::TalkGate::default(),
            interact_presses: CounterReader::starting_at(PRESSES_MOVE[PRESS_INTERACT].load(Ordering::SeqCst)),
            abort: AbortMerge::default(),
            backoff_logged_until: f64::NEG_INFINITY,
            fade_watch: FadeWatch::default(),
            read_fail_streak: 0,
            ticks_active: 0,
            protect_present: false,
            last_writes: 0,
            last_skips: 0,
            skip_streak: 0,
            cam_write_logged: false,
            writes_at_start: 0,
            persisted_at_start: 0,
            fov_persisted_at_start: 0,
            hud_saved: None,
            hud_rewrites: 0,
            hud_gave_up: false,
            removal: None,
            protect_chr: 0,
            protect_applied_t: 0.0,
            hud_pending: None,
            panic_cleanup: false,
            disabled_ticks: 0,
            black_hot: false,
            film_from: Film::BLACK,
            film_to: Film::BLACK,
            film_t0: 0.0,
            film_dur: 0.0,
            spawn_failures: 0,
            breaker_until: 0.0,
            seen_ids: HashSet::new(),
            last_indicator_kind: 255,
            ready_toast_t: f64::NEG_INFINITY,
            pulse_t: f64::NEG_INFINITY,
            hit_t: f64::NEG_INFINITY,
            hit_kill: false,
            agl_m: 0.0,
            agl_t: f64::NEG_INFINITY,
            ump_release_needed: false,
            pending_twins: Vec::new(),
        }
    }

    fn params(&self, cfg: &Config) -> Params {
        let a = &cfg.ac130;
        // without the converted data no ride can start (gate: rows_missing), so no MW2 number is typed here
        let mw2 = self.data.as_ref().map_or(0.0, |d| d.timing.duration_s);
        Params {
            duration_s: if a.duration_s > 0.0 { a.duration_s } else { mw2 },
            cooldown_s: a.cooldown_s,
            cooldown_enabled: a.cooldown_enabled,
            verify_ticks: mw2er_core::ac130_logic::PROTECT_VERIFY_TICKS,
            shells_after_exit: a.shells_after_exit,
            start_backoff_s: a.start_backoff_s,
        }
    }

    fn protect_id(&self) -> i32 {
        self.data.as_ref().map_or(0, |d| d.rows.protect_speffect_id)
    }

    /// (radius, altitude, period, arc) from the config or the converted MW2 rig. `None` without the converted data
    /// (review M4 minor 9: no typed MW2 fallbacks; the start gate reports rows_missing).
    fn geometry(&self, cfg: &Config) -> Option<(f32, f32, f32, f32)> {
        let a = &cfg.ac130;
        let d = self.data.as_ref()?;
        let (r_mw2, h_mw2) = d.timing.rig_m();
        let r = if a.radius_m > 0.0 { a.radius_m } else { r_mw2 as f32 } * a.scale;
        let h = if a.altitude_m > 0.0 { a.altitude_m } else { h_mw2 as f32 } * a.scale;
        let period = if a.orbit_period_s > 0.0 { a.orbit_period_s } else { d.timing.orbit_period_s as f32 };
        let arc = if a.aim_arc_deg > 0.0 { a.aim_arc_deg } else { d.timing.arc_deg[0] as f32 };
        Some((r, h, period, arc))
    }

    /// The effective gun FOVs (cg_fov degrees): config, or 0 = the converted `ads_zoom_fov` (review M4 minor 9).
    fn fovs(&self, cfg: &Config) -> Option<[f32; 3]> {
        let d = self.data.as_ref()?;
        Some(cfg.ac130.resolve_fovs([0, 1, 2].map(|k| d.weapons.guns[k].ads_zoom_fov as f32)))
    }

    fn log_cfg(&mut self, cfg: &Config) {
        let a = &cfg.ac130;
        let (r, h, period, arc) = self.geometry(cfg).unwrap_or((f32::NAN, f32::NAN, f32::NAN, f32::NAN));
        let p = self.params(cfg);
        let fv = self.fovs(cfg).unwrap_or([f32::NAN; 3]);
        let v = fv.map(|x| er_fov(x, true, 16.0 / 9.0).to_degrees());
        let line = format!(
            "cfg enabled={} key={} cooldown_s={} cooldown={} duration_s={} R={r:.1} H={h:.1} period_s={period} arc={arc} base={} fov={}/{}/{}(v={:.1}/{:.1}/{:.2}deg) vision={} tiers={} hook={} thermal={} data={}",
            a.enabled as u8,
            cfg.keys.ac130,
            a.cooldown_s,
            a.cooldown_enabled as u8,
            p.duration_s,
            match a.base_view {
                BaseViewCfg::Center => "center",
                BaseViewCfg::Mw2Tag => "mw2_tag",
            },
            fv[0],
            fv[1],
            fv[2],
            v[0],
            v[1],
            v[2],
            a.vision.label(),
            a.explosion_tiers,
            cam_hook::INSTALLED.load(Ordering::SeqCst) as u8,
            a.thermal as u8,
            self.data.is_some() as u8
        );
        if self.cfg_seen.as_deref() != Some(line.as_str()) {
            log_i!("AC130", "{line}");
            if a.sync_chr_cam {
                log_w!("AC130", "WARN sync_chr_cam = true is not built in v1 (fallback 1 of section 4.7); the gunship writes CSCamera.pers_cam_1 only");
            }
            self.cfg_seen = Some(line);
        }
    }

    /// The PARAMS check of the AC-130 rows (first in-world tick after each load).
    /// ER's HUD is hidden by us (`CSFeManImp.hud_state` written for a ride, or its restore still pending): task.rs must
    /// not read the hidden HUD index as an ER menu (`menu_logic::hud_gone`).
    pub fn hides_er_hud(&self) -> bool {
        self.hud_saved.is_some() || self.hud_pending.is_some()
    }

    pub fn check_rows(&mut self) {
        let Some(d) = self.data.clone() else {
            self.rows_ok = Some(false);
            log_w!("AC130", "blocked reason=rows_missing (converted\\ac130 data not loaded)");
            return;
        };
        match params::check_ac130(&d.rows) {
            Ok((b, a, s, fx)) => {
                self.rows_ok = Some(true);
                log_i!("AC130", "rows ok bullet={b} atk={a} speffect={s} protect_id={} {fx}", d.rows.protect_speffect_id);
            }
            Err(e) => {
                self.rows_ok = Some(false);
                log_e!("AC130", "blocked reason=rows_missing ids={e}");
            }
        }
    }

    pub fn reset_rows(&mut self) {
        self.rows_ok = None;
    }

    fn take_keys(&mut self) -> [u32; AC_KEYS] {
        let mut out = [0u32; AC_KEYS];
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.readers[i].take(AC_PRESSES[i].load(Ordering::SeqCst));
        }
        out
    }

    /// One task tick.
    pub fn tick(&mut self, i: &Ac130In) -> Ac130Out {
        if self.data.is_none() {
            self.data = AC_DATA.get().cloned();
        }
        let cfg = i.cfg;
        self.log_cfg(cfg);
        let p = self.params(cfg);
        let keys = self.take_keys();
        let esc = self.esc_reader.take(ESC_PRESSES.load(Ordering::SeqCst)) > 0;
        let interacts = self.interact_presses.take(PRESSES_MOVE[PRESS_INTERACT].load(Ordering::Relaxed));
        self.talk_gate.interacted(interacts, i.now);
        let wheel = WHEEL.swap(0, Ordering::Relaxed);
        let now = i.now;

        // world transitions
        if i.in_world != self.in_world {
            self.in_world = i.in_world;
            if i.in_world {
                self.cleanup_pending = true;
                self.seen_ids.clear();
            } else {
                let st = self.shell_times();
                if let Some(plan) = self.machine.force_end(Reason::WorldLeft, now, &st, &p) {
                    self.end(plan, false);
                }
            }
        }
        if cam_hook::AC_FAULT.swap(false, Ordering::SeqCst) {
            let st = self.shell_times();
            if let Some(plan) = self.machine.force_end(Reason::Panic, now, &st, &p) {
                log_e!("AC130", "FAIL the camera-step detour caught a panic in mode 2; ending the ride");
                self.end(plan, i.in_world);
            }
        }

        // signals (read once by the task, shared with first person)
        let fpp = i.fp_player;
        let world = i.fp_world;
        let player_fp = fpp.as_ref().ok().and_then(|p| *p);
        let chr = main_player();

        match self.machine.phase() {
            Phase::Idle => {
                self.idle_tick(i, &p, chr, keys[AC_KEY_CALL] > 0, player_fp.as_ref(), world.as_ref());
            }
            Phase::Starting { .. } | Phase::Active { .. } => {
                let read_fail = fpp.is_err() || (i.in_world && (player_fp.is_none() || i.player.is_none()));
                self.read_fail_streak = if read_fail { self.read_fail_streak + 1 } else { 0 };
                let exit_key = keys[AC_KEY_CALL] > 0 || keys[AC_KEY_EXIT] > 0;
                if esc {
                    log_i!("AC130", "esc pressed (raw DIK_ESCAPE edge): ending the ride as menu");
                }
                self.ride_tick(i, &p, chr, exit_key, esc, &keys, wheel, player_fp.as_ref(), world.as_ref());
            }
            Phase::Ending { .. } => self.ending_tick(i, chr),
        }
        if !self.machine.is_active() {
            WANT_GUNSHIP_INPUT.store(false, Ordering::SeqCst);
            self.wheel_acc = 0;
        }
        if self.ump_release_needed && !RAW_FIRE_DOWN.load(Ordering::SeqCst) && !crate::pad::fire_down() && !crate::pad::gunship_fire_down() {
            self.ump_release_needed = false;
        }
        self.blasts.retain(|b| now - b.t <= BLAST_KEEP_S);
        if let Some(m) = self.abort.poll(now) {
            log_i!("AC130", "start aborted reason={} x{} in {:.1}s", m.reason.label(), m.count, m.span_s);
        }
        Ac130Out {
            fp_block: self.machine.fp_blocked(),
            ump_block: self.machine.fp_blocked() || self.ump_release_needed,
            active: self.machine.is_active(),
        }
    }

    fn gate(&self, i: &Ac130In, p: Option<&fp_read_types::FpPlayer>, w: Option<&fp_read_types::FpWorld>, sky_ok: bool) -> GateIn {
        let cfg = &i.cfg.ac130;
        let camtype = w.and_then(|w| w.camera_type);
        GateIn {
            enabled: cfg.enabled,
            mw2_on: i.mode_on,
            hook_ok: cam_hook::INSTALLED.load(Ordering::SeqCst),
            // the converted data missing blocks as rows_missing (check_rows logs "convertedc130 data not loaded")
            rows_ok: self.rows_ok == Some(true) && self.data.is_some(),
            settled: i.in_world && i.world_ticks >= ARM_TICKS && i.cam_valid,
            loading: !i.in_world || i.fp_latched.0,
            dead: i.player_dead || p.is_some_and(|p| fp_logic::dead(p.hp, p.flags1c5, camtype)),
            cutscene: i.fp_latched.1,
            talk: i.fp_latched.2,
            menu: i.menu_open,
            camtype_ok: camtype == Some(0),
            torrent: p.is_some_and(|p| fp_logic::riding(p.ride_state, p.is_mounting, i.cfg.first_person.use_ride_state_signal)),
            // M8 B1: never from a ladder (ER third person mid-climb)
            ladder: p.is_some_and(|p| fp_logic::on_ladder(p.ladder)),
            sky_ok,
        }
    }

    fn idle_tick(&mut self, i: &Ac130In, p: &Params, chr: usize, f7: bool, fp: Option<&fp_read_types::FpPlayer>, w: Option<&fp_read_types::FpWorld>) {
        let now = i.now;
        // stray cleanup / a pending verification of a removal
        self.removal_tick(chr);
        self.hud_pending_tick();
        if self.cleanup_pending && i.in_world && i.world_ticks >= 2 && self.rows_ok.is_some() && self.removal.is_none() {
            self.cleanup_pending = false;
            let id = self.protect_id();
            if id != 0 && self.rows_ok == Some(true) {
                match speffect_count(chr, id) {
                    Some(n) if n > 0 => {
                        // SAFETY: the live local player, read this tick on the game thread; version gate checked inside.
                        let ok = unsafe { remove_speffect(chr, id) };
                        log_i!("AC130", "PROTECT cleanup removed={n} id={id} chr=0x{chr:x} call_ok={} (a stray copy: quit during a ride or kept by the save)", ok as u8);
                        self.removal = Some(Removal { tracker: RemoveTracker::default(), reason: "cleanup", cleanup: true });
                    }
                    Some(_) => {}
                    None => self.cleanup_pending = true,
                }
            }
        }
        // indicator (cheap checks; the sky ray only on F7)
        let g = self.gate(i, fp, w, true);
        let block = self.machine.check(now, &g, p).err();
        let ind = indicator(&self.machine, now, p, block);
        let kind = match ind {
            Indicator::Ready => 0,
            Indicator::Cooldown(_) => 1,
            Indicator::Unavailable(_) => 2,
            Indicator::Busy => 3,
        };
        if kind != self.last_indicator_kind {
            match &ind {
                // Run 16: a brief Unavailable blip (camera gate, menu) must not re-announce: only a finished cooldown
                // (1) or the first ready after MW2 mode on (255) does.
                Indicator::Ready if i.mode_on && matches!(self.last_indicator_kind, 1 | 255) => {
                    log_i!("AC130", "ready");
                    self.ready_toast_t = now;
                    if i.cfg.ac130.audio.announce_ready {
                        self.play(Cue::Announcer, 1.0, 0);
                    }
                }
                Indicator::Cooldown(s) => log_i!("AC130", "cooldown remaining_s={s:.1}"),
                _ => {}
            }
            if !(kind == 0 && !i.mode_on) {
                self.last_indicator_kind = kind;
            }
        }
        if f7 {
            // M7: never during a Tactical Nuke sequence
            if i.nuke_busy {
                log_i!("AC130", "blocked reason=nuke (a Tactical Nuke sequence is running)");
            } else {
                self.try_start(i, p, chr, fp, w);
            }
        }
    }

    fn try_start(&mut self, i: &Ac130In, p: &Params, chr: usize, fp: Option<&fp_read_types::FpPlayer>, w: Option<&fp_read_types::FpWorld>) {
        let now = i.now;
        let mut g = self.gate(i, fp, w, true);
        if let Err(b) = self.machine.check(now, &g, p) {
            self.log_block(b, now, p);
            return;
        }
        if i.cfg.ac130.require_open_sky {
            g.sky_ok = self.sky_clear(i);
        }
        if let Err(b) = self.machine.start(now, &g, p) {
            self.log_block(b, now, p);
            return;
        }
        // M6 run-13 bug 1: arm the exit edges from THIS tick's values (a load during Starting is caught against a fresh
        // baseline; nothing is carried over from an earlier ride)
        match w {
            Some(w) => self.edges.arm(w.load_counter, w.camera_mask, w.talk_id),
            None => self.edges.clear(),
        }
        let id = self.protect_id();
        if let Err(e) = calls_allowed(WRITES_ALLOWED.load(Ordering::SeqCst), self.rows_ok == Some(true)) {
            log_e!("AC130", "blocked reason=protect_refused ({e})");
            let st = self.shell_times();
            if let Some(plan) = self.machine.force_end(Reason::ProtectFail, now, &st, p) {
                self.end(plan, true);
            }
            return;
        }
        let before = speffect_count(chr, id).unwrap_or(0);
        if before > 0 {
            // a stray copy first goes (section 9)
            // SAFETY: the live local player read this tick, game thread; version gate checked inside.
            unsafe { remove_speffect(chr, id) };
            log_i!("AC130", "PROTECT cleanup removed={before} id={id} chr=0x{chr:x} (before apply)");
        }
        // SAFETY: as above.
        let called = unsafe { apply_speffect(chr, id) };
        let present = speffect_count(chr, id).unwrap_or(0);
        self.protect_chr = chr;
        self.protect_applied_t = now;
        self.protect_present = present > 0;
        log_i!(
            "AC130",
            "PROTECT apply id={id} chr=0x{chr:x} present_before={before} present={present} ok={} call={}",
            (present == 1) as u8,
            called as u8
        );
        self.hp_start = i.player.map_or(0, |p| p.chr.hp);
        self.hp_min = self.hp_start;
        self.fp_was_on = i.fp_on;
        self.ticks_active = 0;
        self.read_fail_streak = 0;
    }

    fn log_block(&mut self, b: Block, now: f64, p: &Params) {
        if b == Block::Backoff {
            if now >= self.backoff_logged_until {
                self.backoff_logged_until = now + p.start_backoff_s.max(0.1);
                log_i!("AC130", "blocked reason=retry_backoff (an aborted start waits {:.1} s)", p.start_backoff_s);
            }
            return;
        }
        match b {
            Block::Cooldown => log_i!("AC130", "blocked reason=cooldown remaining_s={:.1}", self.machine.cooldown_remaining(now, p)),
            Block::Ladder => log_i!("AC130", "blocked reason=ladder (ER drives the climb; F7 again on the ground)"),
            _ => log_i!("AC130", "blocked reason={}", b.label()),
        }
        shared::toast(format!("AC-130 unavailable: {}", b.label().replace('_', " ")), 2.0, false);
    }

    /// A map ray straight up from the feet + 2 m to the gunship altitude: a hit means no open sky.
    fn sky_clear(&self, i: &Ac130In) -> bool {
        let Some(pl) = i.player else { return false };
        let Some((_, h, _, _)) = self.geometry(i.cfg) else { return false };
        let o = pl.chr.pos + V3::new(0.0, 2.0, 0.0);
        let hit = havok::map_ray(o, pl.chr.pos_w, V3::new(0.0, 1.0, 0.0), h.max(10.0), pl.player_ins);
        if let Some(t) = hit {
            log_i!("AC130", "sky ray hit at {t:.1} m above the feet (+2 m); legacy dungeon / cave / overhang");
        }
        hit.is_none()
    }

    fn shell_times(&self) -> ShellTimes {
        ShellTimes { last_due: self.shells.iter().map(|s| s.due).reduce(f64::max), last_explosion: self.last_explosion }
    }

    #[allow(clippy::too_many_arguments)]
    fn ride_tick(
        &mut self,
        i: &Ac130In,
        p: &Params,
        chr: usize,
        exit_key: bool,
        esc: bool,
        keys: &[u32; AC_KEYS],
        wheel: i32,
        fp: Option<&fp_read_types::FpPlayer>,
        w: Option<&fp_read_types::FpWorld>,
    ) {
        let now = i.now;
        let cfg = &i.cfg.ac130;
        let id = self.protect_id();
        let starting = matches!(self.machine.phase(), Phase::Starting { .. });
        // the protection: every Starting tick, then every 30 ticks while Active
        if starting || self.ticks_active % ACTIVE_CHECK_EVERY_TICKS == 0 {
            self.protect_present = chr == self.protect_chr && speffect_count(chr, id).unwrap_or(0) > 0;
        }
        self.protect_refresh(now, chr);
        let camtype = w.and_then(|w| w.camera_type);
        if let Some(pl) = i.player {
            self.hp_min = self.hp_min.min(pl.chr.hp);
            if !starting {
                let d = (pl.chr.pos - self.center).len();
                self.moved_m = self.moved_m.max(d);
                if d > 1.0 && !self.move_warned {
                    self.move_warned = true;
                    log_w!("AC130", "WARN player_moved={d:.1}m (the character should stand still during the ride)");
                }
            }
        }
        // cam_fail: consecutive detour skips while Active
        let (writes, skips) = (cam_hook::AC_WRITES.load(Ordering::Relaxed), cam_hook::AC_SKIPS.load(Ordering::Relaxed));
        if writes > self.last_writes {
            self.skip_streak = 0;
        } else {
            self.skip_streak += skips.saturating_sub(self.last_skips);
        }
        self.last_writes = writes;
        self.last_skips = skips;
        let fade_fire = if !starting && i.cfg.first_person.use_fade_signal { w.and_then(|w| self.fade_watch.fired(&w.fade, i.cfg.first_person.fade_alpha_max)) } else { None };
        // 2026-10-21: a talk id change with no interact press shortly before is a boss / NPC voice line (c2130
        // 1000 -> 204101000 mid-fight): the ride goes on and the talk edge takes the new id
        if let Some(w) = w
            && let Some((a, b)) = self.edges.ignore_voice_line(w.talk_id, &self.talk_gate, now, i.cfg.first_person.talk_interact_window_s as f64)
        {
            log_i!(
                "AC130",
                "talk ignored talk_id {a}->{b} last_interact={} window={:.1}s (no E / pad interact before it: a voice line, not a dialog)",
                Some(self.talk_gate.since(now)).filter(|s| s.is_finite()).map_or("never".into(), |s| format!("{s:.1}s")),
                i.cfg.first_person.talk_interact_window_s
            );
        }
        let (edge_load, edge_mask, edge_talk) = w.map_or((false, false, false), |w| self.edges.exit_flags(w.load_counter, w.camera_mask, w.talk_id));
        let ex = ExitIn {
            world_left: !i.in_world,
            loading: edge_load,
            dead: fp.is_some_and(|p| fp_logic::dead(p.hp, p.flags1c5, camtype)),
            read_fail: self.read_fail_streak >= READ_FAIL_TICKS,
            cam_fail: !starting && self.skip_streak > CAM_FAIL_SKIPS,
            protect_missing: false,
            cutscene: fade_fire.is_some() || (i.cfg.first_person.use_cam_mask_signal && edge_mask),
            // the pause menu (task signal) or a raw Esc edge (review M4 minor 4: Esc ends the ride even if the menu
            // signal misses it, so the pause menu is never left unusable under the input capture)
            menu: i.menu_open || esc,
            talk: i.cfg.first_person.use_talk_signal && edge_talk,
            camtype_changed: !starting && camtype != self.start_camtype,
            moved: i.player.is_some_and(|pl| !starting && (pl.chr.pos - self.center).len() > cfg.max_player_move_m),
            mw2_off: !i.mode_on,
            exit_key: exit_key && !starting,
            timeout: false,
            disabled: !cfg.enabled,
        };
        let st = self.shell_times();
        match self.machine.tick(now, &ex, self.protect_present, &st, p) {
            Some(Event::Activated) => self.begin_ride(i, w),
            Some(Event::Ended(plan)) => {
                self.end(plan, i.in_world);
                return;
            }
            None => {}
        }
        if self.machine.is_active() {
            self.active_tick(i, keys, wheel);
        }
    }

    fn begin_ride(&mut self, i: &Ac130In, w: Option<&fp_read_types::FpWorld>) {
        let now = i.now;
        let cfg = &i.cfg.ac130;
        let Some(d) = self.data.clone() else { return };
        let Some(pl) = i.player else { return };
        let (Some((r, h, period, arc)), Some(fovs)) = (self.geometry(i.cfg), self.fovs(i.cfg)) else { return };
        self.center = pl.chr.pos;
        self.center_w = pl.chr.pos_w;
        let er_fwd = i.cam.map_or(V3::new(0.0, 0.0, 1.0), |c| c.row2);
        let theta0 = theta0_behind(er_fwd);
        let aspect = i.cam.map_or(16.0 / 9.0, |c| if c.aspect.is_finite() && c.aspect > 0.1 { c.aspect } else { 16.0 / 9.0 });
        let specs = [0, 1, 2].map(|k| {
            let g = &d.weapons.guns[k];
            GunSpec { fire_time_s: g.fire_time_s, clip: g.clip_size, reload_s: g.reload_s }
        });
        let guns = Guns::new(specs, now);
        let fov = er_fov(fovs[0], cfg.fov_is_vertical, aspect);
        let cam = read_pers_cam1_17();
        let gp = GunshipParams {
            cam_addr: cam.map_or(0, |c| c.0),
            cscamera: cscamera_addr(),
            orbit: Orbit { center: self.center, radius: r, height: h, period_s: period, dir: cfg.orbit_dir as f32, theta0 },
            base_tag: cfg.base_view == BaseViewCfg::Mw2Tag,
            tag_pitch_deg: d.timing.tag_pitch_deg as f32,
            arc_deg: arc,
            fov,
            deg_per_count: cfg.mouse_deg_per_count * cfg.mouse_sens,
            zoom_scale: 1.0,
            invert_y: cfg.invert_y,
            max_dist: max_dist(r, h, 25.0),
        };
        self.writes_at_start = cam_hook::AC_WRITES.load(Ordering::Relaxed);
        self.persisted_at_start = cam_hook::AC_PERSISTED.load(Ordering::Relaxed);
        self.fov_persisted_at_start = cam_hook::AC_FOV_PERSISTED.load(Ordering::Relaxed);
        self.last_writes = self.writes_at_start;
        self.last_skips = cam_hook::AC_SKIPS.load(Ordering::Relaxed);
        self.skip_streak = 0;
        self.cam_write_logged = false;
        cam_hook::ac130_start(&gp, pl.chr.pos);
        WHEEL.swap(0, Ordering::SeqCst);
        WANT_GUNSHIP_INPUT.store(true, Ordering::SeqCst);
        self.guns = Some(guns);
        self.t_active = now;
        self.start_camtype = w.and_then(|w| w.camera_type);
        // (the load / mask / talk edges were armed in try_start; M6 run-13 bug 1)
        if let Some(w) = w {
            self.fade_watch.arm(&w.fade, i.cfg.first_person.fade_alpha_max);
        }
        self.shots = [0; 3];
        self.explosions = 0;
        self.hits = 0;
        self.kills = 0;
        self.shells.clear();
        self.tracer_ring.clear();
        self.moved_m = 0.0;
        self.move_warned = false;
        self.ticks_active = 0;
        // thermal (white hot unless start_black_hot), fading in from black
        let (wn, bn) = cfg.vision.names();
        let white = d.vision.visions.get(wn).map(pack).unwrap_or(Film::BLACK);
        let black = d.vision.visions.get(bn).map(pack).unwrap_or(Film::BLACK);
        self.black_hot = cfg.start_black_hot;
        let target = if self.black_hot { black } else { white };
        self.film_from = target;
        self.film_to = target;
        self.film_t0 = now;
        self.film_dur = 0.0;
        // ER HUD hidden (restored only if it still holds our value)
        self.hud_saved = None;
        self.hud_rewrites = 0;
        self.hud_gave_up = false;
        if cfg.hide_er_hud
            && let Some((addr, old)) = hud_state()
            && old != HUD_HIDE_ALL
        {
            // SAFETY: CSFeManImp.hud_state, one u8 of the live singleton (HideAll = 0 is a valid value).
            let ok = unsafe { write(addr, HUD_HIDE_ALL) };
            log_i!("AC130", "write field=CSFeManImp.hud_state(+0x78) addr=0x{addr:x} old={old} new={HUD_HIDE_ALL} ok={}", ok as u8);
            if ok {
                self.hud_saved = Some((addr, old));
            }
        }
        shared::ambient(true, 1.0, 0);
        log_i!(
            "AC130",
            "on center={} theta0_deg={:.1} gun=105 fp_was_on={} camtype={:?} er_fov={:.1}deg gun_fov={:.2}deg R={r:.1} H={h:.1} slant={:.1} listener=render_cam(inferred) hp={}",
            logfmt::v3(self.center),
            theta0.to_degrees(),
            self.fp_was_on as u8,
            self.start_camtype,
            i.cam.map_or(f32::NAN, |c| c.fov_raw).to_degrees(),
            fov.to_degrees(),
            (r * r + h * h).sqrt(),
            self.hp_start
        );
        // after run 9: the effects of this ride (shake, impact flash/bloom); the rows' object damage and SFX are in the
        // 'rows ok' line
        let sh = &cfg.shake;
        let tr = &cfg.tracers;
        log_i!(
            "AC130",
            "effects shake={} scale={:.2} rot_deg={:.2} roll_deg={:.2} pos_m={:.2} freq_hz={:.1} mw2_falloff={} flash_105={:.1}m/{:.2}s gain={:.2} bloom_105={:.1}m",
            (sh.enabled && sh.scale * cfg.shake_scale > 0.0) as u8,
            sh.scale * cfg.shake_scale,
            sh.rot_deg,
            sh.roll_deg,
            sh.pos_m,
            sh.freq_hz,
            sh.mw2_falloff as u8,
            tr.flash_105,
            if tr.flash_s_105 > 0.0 { tr.flash_s_105 } else { tr.flash_s },
            tr.flash_gain_105,
            tr.bloom_105
        );
        if gp.cam_addr == 0 {
            log_w!("AC130", "WARN pers_cam_1 unreadable at start: the detour will skip until it is (cam_fail after {CAM_FAIL_SKIPS} skips)");
        }
    }

    /// One MW2 earthquake (after run 9, `[ac130.shake]`): its amplitude at the gunship camera (`dist_m` from the quake
    /// origin) goes into the detour's quake slots; one `AC130 SHAKE` line per quake (also when the shake is off).
    fn quake(&mut self, cfg: &mw2er_core::config::Ac130, q: &mw2er_core::ac130_data::Quake, dist_m: f32, src: &str, gid: &str) {
        let sp = cfg.shake.params(cfg.shake_scale);
        let radius_m = q.radius_in as f32 * ac130_shake::INCH_M;
        let amp = ac130_shake::quake_amp(q.scale as f32, &sp, dist_m, radius_m);
        cam_hook::set_shake_params(&sp);
        if amp > 0.0 {
            cam_hook::push_quake(amp, q.duration_s as f32, self.rng.next_u64() as u32);
        }
        log_i!(
            "AC130",
            "SHAKE src={src} gun={gid} mw2_scale={:.2} dur_s={:.2} radius_m={radius_m:.1} dist_m={dist_m:.1} gain={:.2} amp={amp:.3} on={}",
            q.scale,
            q.duration_s,
            ac130_shake::falloff_gain(dist_m, radius_m, sp.mw2_falloff),
            (sp.scale > 0.0) as u8
        );
    }

    fn current_film(&self, now: f64) -> Film {
        film_at(&self.film_from, &self.film_to, self.film_t0, self.film_dur, now)
    }

    fn active_tick(&mut self, i: &Ac130In, keys: &[u32; AC_KEYS], wheel: i32) {
        let now = i.now;
        let cfg = &i.cfg.ac130;
        self.ticks_active += 1;
        WANT_GUNSHIP_INPUT.store(true, Ordering::SeqCst);
        if let Some(pl) = i.player {
            cam_hook::set_feet(pl.chr.pos);
        }
        // the first write of the ride (the detour cannot log)
        let writes = cam_hook::AC_WRITES.load(Ordering::Relaxed);
        if !self.cam_write_logged && writes > self.writes_at_start {
            self.cam_write_logged = true;
            let er = cam_hook::er_frame();
            if let Some((pos, _, _, fov)) = cam_hook::gunship_view() {
                log_i!(
                    "AC130CAM",
                    "write field=CSCamera.pers_cam_1+0x10..0x54 addr=0x{:x} old_pos=({:.2},{:.2},{:.2}) new_pos={} old_fov={:.4} new_fov={fov:.4} (camera-step hook mode 2, one 68-byte write per frame)",
                    cam_hook::AC_CAM_ADDR.load(Ordering::Relaxed) + 0x10,
                    er[12],
                    er[13],
                    er[14],
                    logfmt::v3(pos),
                    er[16]
                );
            }
        }
        let Some(d) = self.data.clone() else { return };
        // gun switching: wheel / Q cycle, 1/2/3 direct
        let mut switched = None;
        if let Some(g) = self.guns.as_mut() {
            let steps = wheel_steps(&mut self.wheel_acc, wheel);
            if steps != 0 && g.cycle(steps) {
                switched = Some(SwitchVia::Wheel);
            }
            for _ in 0..keys[AC_KEY_NEXT].min(3) {
                if g.cycle(1) {
                    switched = Some(SwitchVia::Key);
                }
            }
            for k in 0..3 {
                if keys[AC_KEY_GUN1 + k] > 0 && g.select(k) {
                    switched = Some(SwitchVia::Key);
                }
            }
        }
        if let (Some(via), Some(g)) = (switched, self.guns.as_ref()) {
            let k = g.selected();
            let aspect = i.cam.map_or(16.0 / 9.0, |c| if c.aspect.is_finite() && c.aspect > 0.1 { c.aspect } else { 16.0 / 9.0 });
            let fovs = self.fovs(i.cfg).unwrap_or([f32::NAN; 3]);
            let fov = er_fov(fovs[k], cfg.fov_is_vertical, aspect);
            let zoom = if cfg.zoom_sens_scale { fovs[k] / fovs[0].max(1.0) } else { 1.0 };
            cam_hook::set_gun(fov, zoom);
            self.play(Cue::Switch, 1.0, 0);
            log_i!("AC130", "SWITCH gun={} via={} fov={:.2}deg", d.weapons.guns[k].id, via.label(), fov.to_degrees());
        }
        // white hot / black hot (MW2 +activate)
        if keys[AC_KEY_THERMAL] > 0 {
            let (wn, bn) = cfg.vision.names();
            let white = d.vision.visions.get(wn).map(pack).unwrap_or(Film::BLACK);
            let black = d.vision.visions.get(bn).map(pack).unwrap_or(Film::BLACK);
            self.film_from = self.current_film(now);
            self.black_hot = !self.black_hot;
            self.film_to = if self.black_hot { black } else { white };
            self.film_t0 = now;
            self.film_dur = if self.black_hot { d.timing.to_black_hot_s } else { d.timing.to_white_hot_s };
            log_i!("AC130", "THERMAL black_hot={} transition_s={}", self.black_hot as u8, self.film_dur);
        }
        // firing: full auto on the fire button for every gun
        // C3 PASS B: or the pad's gunship fire binding (RT)
        let trigger = ((RAW_FIRE_DOWN.load(Ordering::SeqCst) && i.mouse_polled) || crate::pad::gunship_fire_down()) && i.foreground;
        let mut events = Vec::new();
        if let Some(g) = self.guns.as_mut() {
            g.step(now, trigger, i.cfg.gameplay.max_shots_per_tick.min(2), &mut events);
        }
        for ev in events {
            match ev {
                GunEvent::Shot { gun, clip_after, .. } => self.fire(i, gun, clip_after),
                GunEvent::ReloadStart { gun, until } => {
                    log_i!("AC130", "RELOAD gun={} s={:.2}", d.weapons.guns[gun].id, until - now);
                }
                GunEvent::ReloadDone { gun, clip } => log_i!("AC130", "RELOAD done gun={} clip={clip}", d.weapons.guns[gun].id),
            }
        }
        self.land_shells(i);
        self.scan_seen(i);
        // the ER HUD stays hidden (ER rewriting it 10 ticks in a row: give up, it just shows in thermal)
        if let Some((addr, _)) = self.hud_saved
            && !self.hud_gave_up
            && let Some((a2, cur)) = hud_state()
            && a2 == addr
            && cur != HUD_HIDE_ALL
        {
            self.hud_rewrites += 1;
            if self.hud_rewrites > HUD_REWRITE_GIVE_UP {
                self.hud_gave_up = true;
                log_w!("AC130", "WARN er_hud_rewritten: ER set CSFeManImp.hud_state back {} ticks in a row; no longer writing it", self.hud_rewrites);
            } else {
                // SAFETY: the same CSFeManImp.hud_state byte (address re-checked); HideAll = 0.
                let _ = unsafe { write(addr, HUD_HIDE_ALL) };
            }
        } else {
            self.hud_rewrites = 0;
        }
        // AGL like MW2's readout: camera height over the ground under it, refreshed every 0.75 s
        if now - self.agl_t >= 0.75
            && let (Some((pos, _, _, _)), Some(pl)) = (cam_hook::gunship_view(), i.player)
        {
            self.agl_t = now;
            if let Some(t) = havok::map_ray(pos, pl.chr.pos_w, V3::new(0.0, -1.0, 0.0), cfg.max_ray_m, pl.player_ins) {
                self.agl_m = t;
            }
        }
        if self.ticks_active % cfg.log_every_ticks.max(1) as u64 == 0 {
            self.log_cam(i);
        }
    }

    fn log_cam(&self, i: &Ac130In) {
        let (ay, ap) = cam_hook::aim_offsets();
        let view = cam_hook::gunship_view();
        let feet = i.player.map(|p| p.chr.pos);
        let gun = self.guns.as_ref().map_or(0, |g| g.selected());
        let er_yaw = i.cam.map_or(f32::NAN, |c| c.row2.x.atan2(c.row2.z).to_degrees());
        log_i!(
            "AC130CAM",
            "t={:.1} slant_m={:.1} cam_to_player={} er_cam_to_player={} writes={} skips={} persisted={} fov_persisted={} fov_deg={:.2} gun={} aim_yaw={ay:.1} aim_pitch={ap:.1} mouse_counts={} er_yaw={er_yaw:.1} last_reject={}",
            i.now - self.t_active,
            view.map_or(f32::NAN, |v| (v.0 - self.center).len()),
            match (view, feet) {
                (Some(v), Some(f)) => format!("{:.1}", (v.0 - f).len()),
                _ => "?".into(),
            },
            match (i.cam, feet) {
                (Some(c), Some(f)) => format!("{:.1}", (c.pos - f).len()),
                _ => "?".into(),
            },
            cam_hook::AC_WRITES.load(Ordering::Relaxed) - self.writes_at_start,
            cam_hook::AC_SKIPS.load(Ordering::Relaxed),
            cam_hook::AC_PERSISTED.load(Ordering::Relaxed) - self.persisted_at_start,
            cam_hook::AC_FOV_PERSISTED.load(Ordering::Relaxed) - self.fov_persisted_at_start,
            view.map_or(f32::NAN, |v| v.3.to_degrees()),
            ["105", "40", "25"][gun],
            MOUSE_COUNTS.load(Ordering::Relaxed),
            cam_hook::AC_LAST_REJECT.load(Ordering::Relaxed)
        );
    }

    fn play(&mut self, cue: Cue, extra_gain: f32, delay_ms: u32) {
        let Some(d) = self.data.as_ref() else { return };
        let Some(c) = d.sounds.cues.get(cue.key()) else { return };
        let p = pick(cue, c, &mut self.rng, extra_gain, delay_ms.max(c.startdelay_ms));
        shared::play_ac(p);
    }

    fn fire(&mut self, i: &Ac130In, gun: usize, clip_after: u32) {
        let now = i.now;
        let cfg = &i.cfg.ac130;
        let Some(d) = self.data.clone() else { return };
        self.shots[gun] += 1;
        let n = self.shots[gun];
        let g = &d.weapons.guns[gun];
        self.play(Cue::fire(gun), 1.0, 0);
        if let Some(rc) = Cue::reload(gun) {
            self.play(rc, 1.0, 0);
        }
        if let Some(q) = d.timing.fire_quake.get(&g.id) {
            // MW2 puts the fire quake on the plane model: distance 0 from the gunship camera
            self.quake(cfg, q, 0.0, "fire", &g.id);
        }
        // the round leaves along the unshaken view (the shake only moves the picture); the muzzle uses the rows the
        // user sees, so the tracer streaks in from the visible side
        let (Some((o, f)), Some((_, _, rows, _)), Some(pl)) = (cam_hook::gunship_aim(), cam_hook::gunship_view(), i.player) else {
            log_w!("AC130", "SHOT gun={} n={n} skipped: no gunship camera write yet", g.id);
            return;
        };
        let map_t = havok::map_ray(o, pl.chr.pos_w, f, cfg.max_ray_m, pl.player_ins);
        let bases: Vec<V3> = i
            .chars
            .iter()
            .filter(|c| c.handle != pl.chr.handle && c.hp > 0 && !i.cfg.hit.exclude_character_ids.contains(&c.character_id))
            .map(|c| c.pos)
            .collect();
        let h = first_hit(o, f, map_t, &bases, i.cfg.aim.char_radius_m, i.cfg.hit.capsule_height_m, cfg.max_ray_m);
        let clip = format!("{clip_after}/{}", g.clip_size);
        if h.via == Via::None {
            log_i!("AC130", "SHOT gun={} n={n} clip={clip} o={} dir={} impact=none dist_m=0 via=none flight_s=0", g.id, logfmt::v3(o), logfmt::dir3(f));
            return;
        }
        let impact = o + f * h.t;
        let flight = flight_time(h.t, g.projectile_speed_mps as f32);
        self.shells.push(Shell { gun, impact, dir: f, w: pl.chr.pos_w, due: now + flight as f64, dist: h.t });
        // run 8: the visible round, from the muzzle (camera-space offset at fire time) to the impact over the flight
        self.tracer_ring.push(TracerShot {
            gun: gun as u8,
            muzzle: muzzle([o.x, o.y, o.z], rows, cfg.tracers.muzzle_offset_m),
            impact: [impact.x, impact.y, impact.z],
            t_fire: now,
            flight_s: flight,
        });
        log_i!(
            "AC130",
            "SHOT gun={} n={n} clip={clip} o={} dir={} impact={} dist_m={:.1} via={} flight_s={flight:.2}",
            g.id,
            logfmt::v3(o),
            logfmt::dir3(f),
            logfmt::v3(impact),
            h.t,
            h.via.label()
        );
    }

    fn land_shells(&mut self, i: &Ac130In) {
        let now = i.now;
        let mut due: Vec<Shell> = Vec::new();
        self.shells.retain(|s| {
            if s.due <= now {
                due.push(*s);
                false
            } else {
                true
            }
        });
        for s in due {
            self.explode(i, s);
        }
    }

    fn explode(&mut self, i: &Ac130In, s: Shell) {
        let now = i.now;
        let cfg = &i.cfg.ac130;
        let Some(d) = self.data.clone() else { return };
        let Some(pl) = i.player else { return };
        let gid = d.weapons.guns[s.gun].id.clone();
        let Some(rows) = d.rows.gun(&gid).copied() else { return };
        let tiers: Vec<(u8, mw2er_core::rows::Ac130Row)> = match (cfg.explosion_tiers, rows.core, rows.ring) {
            (2, Some(c), Some(r)) => vec![(0, c), (1, r)],
            _ => vec![(2, rows.single)],
        };
        if let Err(e) = owner_ok(pl.field_ins_handle) {
            log_w!("AC130", "EXPLODE gun={gid} skipped: {e}");
            return;
        }
        if now < self.breaker_until {
            log_w!("AC130", "EXPLODE gun={gid} skipped: spawn breaker open");
            return;
        }
        let origin = s.impact - s.dir * 0.3;
        let mut ids = Vec::new();
        for (tier, r) in &tiers {
            // M7: the AC-130 is a priority spawn (32 emitter slots are kept for it); a refusal is logged by the guard
            if !crate::spawn_guard::admit(now, mw2er_core::emitter_guard::Kind::Ac130, r.id) {
                log_e!("AC130", "EXPLODE gun={gid} id={} SPAWN FAIL code=4 cause=pool_emitters_full (refused by the emitter guard)", r.id);
                continue;
            }
            match spawn::spawn(pl.field_ins_handle, r.id, origin, s.w, s.dir) {
                Ok(_) => {
                    self.spawn_failures = 0;
                    ids.push(r.id.to_string());
                    self.blasts.push(Blast { t: now, pos: s.impact, radius_m: r.radius_m as f32, gun: s.gun, tier: *tier });
                }
                Err(e) => {
                    self.spawn_failures += 1;
                    // M7: every failure names its cause (the pool counters at the failure)
                    let ours: HashSet<i32> = d.rows.ids().into_iter().collect();
                    let pool = bullets::pool_snapshot(pl.chr.handle, &ours, 0);
                    let cause = mw2er_core::spawn_cause::classify(pool.as_ref(), None);
                    log_e!(
                        "AC130",
                        "EXPLODE gun={gid} id={} SPAWN FAIL {e} cause={} emitters={}",
                        r.id,
                        cause.label(),
                        bullets::emitter_upper().map_or("?".to_string(), |n| n.to_string())
                    );
                    if self.spawn_failures >= SPAWN_BREAKER {
                        self.breaker_until = now + SPAWN_BREAKER_COOLDOWN_S;
                        log_e!("AC130", "spawn breaker open after {} consecutive failures; explosions paused {SPAWN_BREAKER_COOLDOWN_S} s", self.spawn_failures);
                    }
                }
            }
        }
        self.explosions += 1;
        self.last_explosion = Some(now);
        // C3 PASS C: the first tier that spawned gets a no-attack synced twin (peers see / hear the shell)
        if let Some(first) = ids.first().and_then(|s| s.parse::<i32>().ok())
            && self.pending_twins.len() < 16
        {
            self.pending_twins.push((first, origin, s.dir));
        }
        log_i!("AC130", "EXPLODE gun={gid} ids={} pos={} dist_m={:.1} late_ms={:.0}", ids.join("+"), logfmt::v3(s.impact), s.dist, (now - s.due) * 1000.0);
        // sound from the gunship, earthquake, dark pulse (105)
        let slant = cam_hook::gunship_view().map_or(s.dist, |v| (v.0 - s.impact).len());
        if let Some(cue) = Cue::explosion(s.gun)
            && let Some(c) = d.sounds.cues.get(cue.key()).cloned()
        {
            let g = explosion_gain(&c, slant, cfg.audio.explosion_floor_gain) * cfg.audio.explosion_gain;
            let p = pick(cue, &c, &mut self.rng, g, 0);
            shared::play_ac(p);
        }
        if let Some(q) = d.timing.impact_quake.get(&gid) {
            self.quake(cfg, q, slant, "impact", &gid);
        }
        if s.gun == 0 {
            self.pulse_t = now;
        }
    }

    /// First sighting per explosion row id per session in the bullet list (the engine created it).
    fn scan_seen(&mut self, i: &Ac130In) {
        let Some(last) = self.last_explosion else { return };
        if i.now - last > SEEN_SCAN_S {
            return;
        }
        let (Some(d), Some(pl)) = (self.data.as_ref(), i.player) else { return };
        let ids: HashSet<i32> = d.rows.ids().into_iter().filter(|id| !self.seen_ids.contains(id)).collect();
        if ids.is_empty() {
            return;
        }
        for b in bullets::read_ours(pl.chr.handle, &ids) {
            if self.seen_ids.insert(b.param_id) {
                log_i!("AC130", "EXPLODE seen id={} pos={} alive={:.3}s", b.param_id, logfmt::v3(b.pos), b.time_alive);
            }
        }
    }

    /// End sequence (docs/M4_DESIGN.md section 2.4), fixed order.
    fn end(&mut self, plan: EndPlan, in_world: bool) {
        // M6 run-13 bug 1: no exit baseline survives the ride
        self.edges.clear();
        self.fade_watch.clear();
        // 1. stop the detour (waits for a call in flight)
        cam_hook::ac130_stop();
        // 2. camera restore: ER's frame back only if pers_cam_1 still holds our last write
        if in_world {
            match read_pers_cam1_17() {
                Some((addr, cur)) if addr == cam_hook::AC_CAM_ADDR.load(Ordering::SeqCst) => match cam_hook::restore_er_frame(&cur, addr) {
                    Some(r) => log_i!(
                        "AC130",
                        "restore field=CSCamera.pers_cam_1+0x10..0x54 addr=0x{:x} old=<{}> new=<{}> floats={} fov_restored={} fov_persisted_frames={} ok={}",
                        addr + 0x10,
                        fmt17(&r.ours),
                        fmt17(&r.er),
                        r.floats,
                        r.fov as u8,
                        cam_hook::AC_FOV_PERSISTED.load(Ordering::Relaxed) - self.fov_persisted_at_start,
                        r.ok as u8
                    ),
                    None => log_i!(
                        "AC130",
                        "restore field=CSCamera.pers_cam_1+0x10..0x54 addr=0x{:x} skipped=er_rebuilt floats=0 fov_persisted_frames={}",
                        addr + 0x10,
                        cam_hook::AC_FOV_PERSISTED.load(Ordering::Relaxed) - self.fov_persisted_at_start
                    ),
                },
                _ => log_i!("AC130", "restore field=CSCamera.pers_cam_1+0x10..0x54 skipped=camera_changed"),
            }
        } else {
            log_i!("AC130", "restore field=CSCamera.pers_cam_1+0x10..0x54 skipped=not_in_world");
        }
        // 3. input released; the UMP45 needs a release edge first
        WANT_GUNSHIP_INPUT.store(false, Ordering::SeqCst);
        WHEEL.swap(0, Ordering::SeqCst);
        self.ump_release_needed = true;
        // 4. ER HUD restored only if it still holds our value; CSFeManImp is a persistent singleton, so this runs in
        //    or out of world (review M4 minor 3). Unreadable now: kept and retried every tick.
        if let Some(saved) = self.hud_saved.take() {
            self.hud_pending = Some(saved);
            self.hud_pending_tick();
            if self.hud_pending.is_some() {
                log_i!("AC130", "restore field=CSFeManImp.hud_state deferred: CSFeManImp unreadable (in_world={}); retried every tick", in_world as u8);
            }
        }
        // 5. thermal off: the snapshot says inactive from this tick on (see hud())
        // 6. sounds
        shared::ambient(false, 0.0, 300);
        if plan.t_s > 0.0 {
            self.play(Cue::Exit, 1.0, 500);
        }
        // 7. first person: the machine no longer blocks it (Ending)
        // shells
        let dropped = if plan.drop_shells { std::mem::take(&mut self.shells).len() } else { 0 };
        if dropped > 0 {
            log_i!("AC130", "SHELL dropped n={dropped} reason={}", plan.reason.label());
        }
        // 9. the off line (8. the protection removal follows in ending_tick); M6: repeated identical aborted starts are
        //    merged into one `start aborted ... x<N> in <T>s` line
        let aborted = plan.t_s <= 0.0 && plan.reason != Reason::ExitKey;
        if aborted {
            let (first, prev) = self.abort.push(plan.reason, shared::now_s());
            if let Some(m) = prev {
                log_i!("AC130", "start aborted reason={} x{} in {:.1}s", m.reason.label(), m.count, m.span_s);
            }
            if first {
                log_i!("AC130", "start aborted reason={} (first of a run; repeats merged)", plan.reason.label());
            }
        }
        log_i!(
            "AC130",
            "off reason={} t_s={:.1} shots=105:{}/40:{}/25:{} explosions={} hits={} kills={} shells_dropped={dropped} player_hp_start={} player_hp_min={} moved_m={:.1} fp_resume={} cooldown_s={}",
            plan.reason.label(),
            plan.t_s,
            self.shots[0],
            self.shots[1],
            self.shots[2],
            self.explosions,
            self.hits,
            self.kills,
            self.hp_start,
            self.hp_min,
            self.moved_m,
            self.fp_was_on as u8,
            plan.cooldown_s
        );
        self.guns = None;
        if !in_world {
            // ER owns new objects after a load: the removal is deferred to the stray cleanup of the next world entry
            log_i!("AC130", "PROTECT restore deferred reason={} (not in world; the stray cleanup removes it on the next world entry)", plan.reason.label());
            self.cleanup_pending = true;
            self.removal = None;
            self.machine.finish();
        }
    }

    fn ending_tick(&mut self, i: &Ac130In, chr: usize) {
        // kept shells still land (timeout / exit_key with shells_after_exit)
        if i.in_world {
            self.land_shells(i);
            self.scan_seen(i);
        } else {
            self.shells.clear();
            self.cleanup_pending = true;
            self.removal = None;
            self.machine.finish();
            return;
        }
        let reason = match self.machine.phase() {
            Phase::Ending { reason, .. } => reason,
            _ => return,
        };
        if self.removal.is_none() {
            // kept shells may still be landing: the protection stays fresh until it is removed
            self.protect_refresh(i.now, chr);
        }
        if self.removal.is_none() && self.machine.protect_due(i.now) {
            let id = self.protect_id();
            // SAFETY: the live local player read this tick on the game thread; version gate checked inside.
            let ok = unsafe { remove_speffect(chr, id) };
            if !ok {
                log_w!("AC130", "PROTECT restore call refused (chr=0x{chr:x}); retried");
            }
            self.removal = Some(Removal { tracker: RemoveTracker::default(), reason: reason.label(), cleanup: false });
            return;
        }
        if self.removal.is_some() {
            self.removal_tick(chr);
        }
    }

    /// Verifies a removal (and retries) one tick after the call.
    fn removal_tick(&mut self, chr: usize) {
        let Some(mut r) = self.removal else { return };
        let id = self.protect_id();
        let Some(n) = speffect_count(chr, id) else { return };
        match r.tracker.step(n > 0) {
            RemoveStep::Done => {
                if r.cleanup {
                    log_i!("AC130", "PROTECT cleanup verified id={id} present=0");
                } else {
                    log_i!("AC130", "PROTECT restore id={id} chr=0x{chr:x} present=0 ok=1 reason={}", r.reason);
                }
                self.removal = None;
                if matches!(self.machine.phase(), Phase::Ending { .. }) {
                    self.machine.finish();
                }
                return;
            }
            RemoveStep::Retry => {
                // SAFETY: as in ending_tick.
                unsafe { remove_speffect(chr, id) };
            }
            RemoveStep::Stuck { first, retry_now } => {
                if first {
                    log_w!("AC130", "WARN protect_stuck id={id} present={n} after a retry; retrying every 60 ticks");
                }
                if retry_now {
                    // SAFETY: as in ending_tick.
                    unsafe { remove_speffect(chr, id) };
                }
            }
        }
        self.removal = Some(r);
    }

    /// Re-applies the protection before its finite effectEndurance runs out (review M4 major 2), logged.
    fn protect_refresh(&mut self, now: f64, chr: usize) {
        let endurance = self.data.as_ref().map_or(0.0, |d| d.rows.protect_endurance_s);
        if self.protect_chr == 0 || chr != self.protect_chr || !refresh_due(now, self.protect_applied_t, endurance) {
            return;
        }
        let id = self.protect_id();
        let before = speffect_count(chr, id).unwrap_or(0);
        // SAFETY: the live local player read this tick on the game thread; version gate checked inside.
        let called = unsafe { apply_speffect(chr, id) };
        let after = speffect_count(chr, id).unwrap_or(0);
        self.protect_applied_t = now;
        log_i!(
            "AC130",
            "PROTECT refresh id={id} chr=0x{chr:x} endurance_s={endurance} present_before={before} present={after} call={} (re-apply semantics [I]: timer reset or a 2nd entry)",
            called as u8
        );
    }

    /// Gives a deferred ER HUD value back once CSFeManImp is readable (only if it still holds our HideAll).
    fn hud_pending_tick(&mut self) {
        let Some((addr, old)) = self.hud_pending else { return };
        match hud_state() {
            Some((a2, cur)) if a2 == addr && cur == HUD_HIDE_ALL => {
                // SAFETY: the same CSFeManImp.hud_state byte (address re-checked); the value it had before our write.
                let ok = unsafe { write(addr, old) };
                log_i!("AC130", "restore field=CSFeManImp.hud_state old={cur} new={old} ok={}", ok as u8);
                if ok {
                    self.hud_pending = None;
                }
            }
            Some((a2, cur)) if a2 == addr => {
                log_i!("AC130", "restore field=CSFeManImp.hud_state skipped: holds {cur} (ER changed it)");
                self.hud_pending = None;
            }
            Some((a2, _)) => {
                log_i!("AC130", "restore field=CSFeManImp.hud_state skipped: CSFeManImp moved (0x{addr:x} -> 0x{a2:x})");
                self.hud_pending = None;
            }
            None => {}
        }
    }

    /// Panic path (no normal tick follows): end the ride and give everything back with fresh reads. What cannot be
    /// done now (protection not verifiable, ER HUD unreadable) is left to [`Self::disabled_tick`].
    pub fn emergency_off(&mut self, now: f64, cfg: &Config) {
        let p = self.params(cfg);
        let st = self.shell_times();
        // a ride (Starting/Active/Ending), a removal not yet verified or a pending stray cleanup: the protection may be on
        let maybe_on = !self.machine.is_idle() || self.removal.is_some() || self.cleanup_pending;
        let ended = self.machine.force_end(Reason::Panic, now, &st, &p);
        cam_hook::ac130_stop();
        WANT_GUNSHIP_INPUT.store(false, Ordering::SeqCst);
        shared::ambient(false, 0.0, 100);
        if let Some(plan) = ended {
            if let Some((addr, cur)) = read_pers_cam1_17()
                && let Some(r) = cam_hook::restore_er_frame(&cur, addr)
            {
                log_i!(
                    "AC130",
                    "restore field=CSCamera.pers_cam_1+0x10..0x54 addr=0x{:x} new=<{}> floats={} fov_restored={} ok={} (panic)",
                    addr + 0x10,
                    fmt17(&r.er),
                    r.floats,
                    r.fov as u8,
                    r.ok as u8
                );
            }
            log_i!("AC130", "off reason={} t_s={:.1} (task panicked)", plan.reason.label(), plan.t_s);
        }
        if let Some(saved) = self.hud_saved.take() {
            self.hud_pending = Some(saved);
        }
        self.hud_pending_tick();
        // the protection: removed now if possible; verified (and retried) by disabled_tick in any case
        self.panic_cleanup = maybe_on && self.protect_id() != 0;
        self.disabled_ticks = 0;
        self.panic_cleanup_step("panic");
    }

    /// One step of the post-panic protection cleanup: remove a present copy, or log the verified absence.
    fn panic_cleanup_step(&mut self, why: &str) {
        if !self.panic_cleanup {
            return;
        }
        let chr = main_player();
        let id = self.protect_id();
        match speffect_count(chr, id) {
            Some(0) => {
                log_i!("AC130", "PROTECT restore id={id} chr=0x{chr:x} present=0 ok=1 reason=panic (verified after {why})");
                self.panic_cleanup = false;
            }
            Some(n) => {
                // SAFETY: the live local player read just now; game thread (CSTask); version gate checked inside.
                let ok = unsafe { remove_speffect(chr, id) };
                log_i!("AC130", "PROTECT restore id={id} chr=0x{chr:x} present={n} call_ok={} reason=panic ({why}; verified on a later tick)", ok as u8);
            }
            // not in world / player unreadable: retried; the finite effectEndurance expires it anyway
            None => {}
        }
    }

    /// Runs every task tick while gameplay is disabled after a panic (the normal tick no longer runs): every 30
    /// ticks, the protection is removed/verified and a deferred ER HUD value given back (review M4 major 2 / minor 3).
    /// Cheap when nothing is pending.
    pub fn disabled_tick(&mut self) {
        if !self.panic_cleanup && self.hud_pending.is_none() {
            return;
        }
        self.disabled_ticks += 1;
        if self.disabled_ticks % DISABLED_CHECK_EVERY_TICKS != 0 {
            return;
        }
        self.hud_pending_tick();
        self.panic_cleanup_step("disabled_tick");
    }

    /// HIT attribution of an HP drop to an AC-130 explosion (called by the task for unattributed drops). Returns the
    /// HIT message and, for a kill, the `AC130 KILL` message (the task logs it right AFTER the HIT line, so the
    /// analyzer can pair each KILL with its `HIT kill=1 src=ac130`).
    pub fn attribute(&mut self, now: f64, cid: u32, hp_before: i32, hp_after: i32, t_drop: f64, chr_pos: V3, capsule_h: f32) -> Option<(String, Option<String>)> {
        let (bi, dist) = attribute(chr_pos, t_drop, &self.blasts, capsule_h, ATTR_BEFORE_S, ATTR_AFTER_S, ATTR_MARGIN_M)?;
        let b = self.blasts[bi];
        let d = self.data.as_ref()?;
        let gid = d.weapons.guns[b.gun].id.clone();
        let rows = d.rows.gun(&gid)?;
        let row = match b.tier {
            0 => rows.core,
            1 => rows.ring,
            _ => Some(rows.single),
        }?;
        let kill = hp_after <= 0 && hp_before > 0;
        self.hits += 1;
        if kill {
            self.kills += 1;
        }
        self.hit_t = now;
        self.hit_kill = kill;
        let tier = ["core", "ring", "single"][b.tier.min(2) as usize];
        let msg = format!(
            "{} src=ac130 gun={gid} tier={tier}",
            logfmt::hit_msg(cid, hp_before, hp_after, row.id, row.mw2.round() as i32, dist, logfmt::HitAttr { via: "ac130", n: 1, kill })
        );
        let kill_msg = kill.then(|| format!("KILL gun={gid} cid=c{cid:04} d_m={dist:.1} tier={tier}"));
        Some((msg, kill_msg))
    }

    /// The last explosion time (the task widens the HP-drop window with it).
    pub fn last_explosion(&self) -> Option<f64> {
        self.last_explosion
    }

    /// The present thread's view of the AC-130 (HUD + thermal).
    pub fn hud(&self, now: f64, cfg: &Config) -> Ac130Hud {
        let mut h = Ac130Hud::empty();
        let p = self.params(cfg);
        let d = self.data.as_ref();
        h.active = self.machine.is_active();
        h.t0 = self.t_active;
        h.duration_s = p.duration_s;
        if let Some(g) = self.guns.as_ref() {
            h.gun = g.selected();
            for k in 0..3 {
                h.clips[k] = g.state(k).clip;
                h.clip_size[k] = g.specs[k].clip;
                h.reloading[k] = g.reloading(k);
            }
        }
        h.thermal = cfg.ac130.thermal;
        h.grain = cfg.ac130.grain;
        // review M4 major 8: the pre-exposure scales the luma weights (exact for desaturation 1, all MW2 visions)
        h.luma = mw2er_core::thermal::exposed_luma(cfg.ac130.luma_weights, cfg.ac130.exposure);
        h.gamma = cfg.ac130.gamma;
        h.film_from = self.film_from;
        h.film_to = self.film_to;
        h.film_t0 = self.film_t0;
        h.film_dur = self.film_dur;
        h.fade_s = d.map_or(1.0, |d| d.timing.fade_in_s);
        h.black_hot = self.black_hot;
        if let Some((pos, _, rows, fov)) = cam_hook::gunship_view() {
            h.cam_pos = [pos.x, pos.y, pos.z];
            h.cam_rows = rows;
            h.vfov = fov;
        }
        if h.active && cfg.ac130.tracers.enabled {
            h.tracers = self.tracer_ring;
        }
        h.pulse_t = self.pulse_t;
        h.hit_t = self.hit_t;
        h.hit_kill = self.hit_kill;
        h.agl_m = self.agl_m;
        let ind = indicator(&self.machine, now, &p, None);
        h.indicator = indicator_text(&ind, &cfg.keys.ac130);
        h.indicator_kind = self.last_indicator_kind.min(3);
        if self.last_indicator_kind == 1 {
            h.indicator = format!("AC-130 {}", timer_text(self.machine.cooldown_remaining(now, &p)));
        }
        h.ready_toast_t = self.ready_toast_t;
        h
    }

    /// Debug markers: nearby enemy chests (FOV-convention oracle), only with `debug_markers`.
    pub fn markers(&self, chars: &[ChrRead], player: Option<&PlayerSample>, cfg: &Config) -> Vec<[f32; 3]> {
        if !(cfg.ac130.debug_markers && self.machine.is_active()) {
            return Vec::new();
        }
        chars
            .iter()
            .filter(|c| player.is_none_or(|p| p.chr.handle != c.handle) && c.hp > 0)
            .map(|c| [c.pos.x, c.pos.y + cfg.aim.chest_height_m, c.pos.z])
            .collect()
    }
}

/// The first-person read types under a short name (the controller reuses `fp_read`'s signals).
mod fp_read_types {
    pub use crate::game::fp_read::{FpPlayer, FpWorld};
}

