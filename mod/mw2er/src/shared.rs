//! All cross-thread state (docs/M2_DESIGN.md section 4). There is no other shared state.
//!
//! - Atomics: written/read by the input detour (which may not lock or allocate), the game task,
//!   the HUD and the boot thread.
//! - `SNAPSHOT`: written once per tick by the task, read by the HUD with `try_lock`.
//! - `CONFIG`: swapped by the config watcher; readers clone the `Arc`.
//! - `SFX_TX`: the task queues sounds with `try_send` (never blocks).

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Instant;

use mw2er_core::ac130_audio::Play;
use mw2er_core::ac130_tracer::TracerRing;
use mw2er_core::config::Config;
use mw2er_core::input_logic::dik_from_name;
use mw2er_core::thermal::Film;
use mw2er_core::vm_anim::VmState;
use mw2er_core::weapon_sim::SfxCue;

// ---- gameplay / input atomics -------------------------------------------------------------------

/// MW2 mode on (F5).
pub static MODE_ON: AtomicBool = AtomicBool::new(false);
/// The task wants LMB / the reload key hidden from the game (mode on, in world, no menu, focused).
pub static WANT_BLANK_FIRE: AtomicBool = AtomicBool::new(false);
pub static WANT_BLANK_RELOAD: AtomicBool = AtomicBool::new(false);
/// Raw (pre-blank) button states seen by the detour.
pub static RAW_FIRE_DOWN: AtomicBool = AtomicBool::new(false);
pub static RAW_RELOAD_DOWN: AtomicBool = AtomicBool::new(false);
pub static RAW_TOGGLE_DOWN: AtomicBool = AtomicBool::new(false);
/// M3: the ADS mouse button (raw), its blanking wish (= the fire button's) and its release latch.
pub static RAW_ADS_DOWN: AtomicBool = AtomicBool::new(false);
pub static WANT_BLANK_ADS: AtomicBool = AtomicBool::new(false);
pub static ADS_LATCH: AtomicBool = AtomicBool::new(false);
/// `rgbButtons` index of the ADS button; 255 = none.
pub static ADS_BUTTON: AtomicU8 = AtomicU8::new(1);
/// Press edge counters (detour -> task).
pub static PRESSES_TOGGLE: AtomicU32 = AtomicU32::new(0);
pub static PRESSES_RELOAD: AtomicU32 = AtomicU32::new(0);
/// Release-latch state, owned by the detour.
pub static FIRE_LATCH: AtomicBool = AtomicBool::new(false);
pub static RELOAD_LATCH: AtomicBool = AtomicBool::new(false);
/// Key configuration mirrored for the detour.
pub static TOGGLE_DIK: AtomicU8 = AtomicU8::new(0x3F); // F5
pub static RELOAD_DIK: AtomicU8 = AtomicU8::new(0x13); // R
pub static FIRE_BUTTON: AtomicU8 = AtomicU8::new(0); // LMB
pub static BLANK_TOGGLE_KEY: AtomicBool = AtomicBool::new(true);
/// M3b: the first-person toggle key (F6), its raw state and press counter (detour -> task).
pub static FP_TOGGLE_DIK: AtomicU8 = AtomicU8::new(0x40); // F6
pub static RAW_FP_DOWN: AtomicBool = AtomicBool::new(false);
pub static PRESSES_FP: AtomicU32 = AtomicU32::new(0);
/// M3b: raw W/A/S/D state from the keyboard buffer (read only, never blanked): bit 0 W, 1 A, 2 S, 3 D.
pub static RAW_MOVE_KEYS: AtomicU8 = AtomicU8::new(0);
pub const MOVE_W: u8 = 1;
pub const MOVE_A: u8 = 2;
pub const MOVE_S: u8 = 4;
pub const MOVE_D: u8 = 8;
/// M3b: game-memory writes are allowed only after the version gate passed (set once by boot.rs).
pub static WRITES_ALLOWED: AtomicBool = AtomicBool::new(false);
/// Last GetDeviceState call per device kind (ms since boot; 0 = never). The mouse stamp also gates
/// firing: a stale `RAW_FIRE_DOWN` (the game stopped polling the mouse) is ignored.
pub static LAST_GDS_MOUSE_MS: AtomicU64 = AtomicU64::new(0);
pub static LAST_GDS_KBD_MS: AtomicU64 = AtomicU64::new(0);
/// Written by every game-task tick (ms since boot; 0 = never). The input detour applies
/// `WANT_BLANK_*` only while this is fresh, so blanking ends by itself when the task stops ticking.
pub static TASK_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
/// C3 (docs/C3_DESIGN.md 1.7): first person's talk latch holds (the movement controller's `talk` reason follows it,
/// so MW2 mode never comes back in third person after a dialog). Written by first_person.rs every tick.
pub static TALK_LATCHED: AtomicBool = AtomicBool::new(false);

// ---- M5 movement input (detour -> task) ------------------------------------------------------------
//
// The detour records the levels and press edges of the movement keys BEFORE any blanking (docs/M5_DESIGN.md section
// 6.3), then hides the wanted keys: `MOVE_BLANK_LATCH` (latched until physical release) and `MOVE_BLANK_PLAIN`
// (W/A/S/D in first person, never latched), both honoured only through `effective_want` (the heartbeat rule).

/// DIK codes of W/A/S/D (configurable; the M3b `RAW_MOVE_KEYS` bits follow them).
pub static MOVE_WASD_DIK: [AtomicU8; 4] = [AtomicU8::new(0x11), AtomicU8::new(0x1E), AtomicU8::new(0x1F), AtomicU8::new(0x20)];
/// DIK codes of the counted keys in `move_input::PRESS_*` order: sprint, jump, crouch, prone, stance_hold (0 = none), E.
pub static MOVE_PRESS_DIK: [AtomicU8; 6] =
    [AtomicU8::new(0x2A), AtomicU8::new(0x39), AtomicU8::new(0x2E), AtomicU8::new(0x1D), AtomicU8::new(0), AtomicU8::new(0x12)];
/// Press counters of those keys (bumped on every up->down edge, before any blanking).
pub static PRESSES_MOVE: [AtomicU32; 6] = [const { AtomicU32::new(0) }; 6];
pub static MOVE_PRESS_DOWN: [AtomicBool; 6] = [const { AtomicBool::new(false) }; 6];
/// Levels: bit i = `MOVE_PRESS_DIK[i]` held (0..5).
pub static RAW_KEYS: AtomicU32 = AtomicU32::new(0);
/// The task's wanted blank masks (256-bit DIK sets) and the detour's latch state.
pub static MOVE_BLANK_LATCH: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
pub static MOVE_BLANK_PLAIN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
pub static MOVE_LATCHED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// Blank reads that hid at least one key (for `MOVE keys ... latched=`).
pub static MOVE_BLANKED_READS: AtomicU64 = AtomicU64::new(0);
/// Key probe (MW2 mode off): press counters of `move_input::PROBE_KEYS`, always recorded.
pub static PROBE_PRESSES: [AtomicU32; mw2er_core::move_input::N_PROBE] = [const { AtomicU32::new(0) }; mw2er_core::move_input::N_PROBE];
pub static PROBE_DOWN: [AtomicBool; mw2er_core::move_input::N_PROBE] = [const { AtomicBool::new(false) }; mw2er_core::move_input::N_PROBE];

// ---- M6 arsenal input (detour -> task) and view effects (task -> camera-step detour) ------------------------------

/// The weapon keys (next, prev; 0 = unbound), their raw state, press counters and release latches.
pub static WEAPON_DIK: [AtomicU8; 2] = [AtomicU8::new(0x02), AtomicU8::new(0x03)];
pub static WEAPON_RAW_DOWN: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
pub static PRESSES_WEAPON: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
pub static WEAPON_LATCH: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
/// The task wants the weapon keys hidden from ER (MW2 mode drives; heartbeat rule).
pub static WANT_BLANK_WEAPON: AtomicBool = AtomicBool::new(false);
/// The task wants the mouse wheel for switching (counts into `ARSENAL_WHEEL`, lZ zeroed for ER; heartbeat rule).
pub static WANT_WHEEL: AtomicBool = AtomicBool::new(false);
pub static ARSENAL_WHEEL: AtomicI32 = AtomicI32::new(0);
/// Mouse sensitivity scale while aiming (f32 bits; 1.0 = untouched) and the detour's residual carry.
pub static SENS_SCALE_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000);
pub static SENS_CARRY: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];
/// View effects for the camera-step detour: kick + sway (pitch, yaw degrees, f32 bits), the ADS FOV tangent ratio
/// (1.0 = ER's fov) and the on switch. Honoured only with the heartbeat rule; never in the gunship.
pub static VIEWFX_ON: AtomicBool = AtomicBool::new(false);
pub static VIEWFX_PITCH_BITS: AtomicU32 = AtomicU32::new(0);
pub static VIEWFX_YAW_BITS: AtomicU32 = AtomicU32::new(0);
pub static VIEWFX_FOV_RATIO_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000);
/// Frames where ER did not rebuild the camera rows since our last write (the kick was not re-applied).
pub static VIEWFX_STALE: AtomicU64 = AtomicU64::new(0);
pub static VIEWFX_WRITES: AtomicU64 = AtomicU64::new(0);
/// The movement class of the gun in hand for `[movement] weapon_move_class = "auto"` (0 smg, 1 rifle, 2 mg, 3 launcher).
pub static ARSENAL_MOVE_CLASS: AtomicU8 = AtomicU8::new(0);

// ---- M7 nuke input (detour -> task) and post pass / shake state (task -> present thread / camera detour) ---------

/// The nuke key (MW2 `+actionslot 4`, default 4; 0 = unbound), its raw state, press counter and release latch.
pub static NUKE_DIK: AtomicU8 = AtomicU8::new(0x05);
pub static NUKE_RAW_DOWN: AtomicBool = AtomicBool::new(false);
pub static PRESSES_NUKE: AtomicU32 = AtomicU32::new(0);
pub static NUKE_LATCH: AtomicBool = AtomicBool::new(false);
/// The task wants the nuke key hidden from ER (MW2 mode drives; heartbeat rule).
pub static WANT_BLANK_NUKE: AtomicBool = AtomicBool::new(false);

// ---- 2026-10-21 heal (detour -> task) ------------------------------------------------------------------------------

/// The heal key (`[keys] heal`, default Q; 0 = none) and ER's use-item key (`[keys] er_use_item`, default R).
pub static HEAL_DIK: AtomicU8 = AtomicU8::new(0x10);
pub static ER_USE_ITEM_DIK: AtomicU8 = AtomicU8::new(0x13);
pub static HEAL_RAW_DOWN: AtomicBool = AtomicBool::new(false);
pub static HEAL_LATCH: AtomicBool = AtomicBool::new(false);
/// The running pulse of ER's use-item key (ms since boot; 0 = none), owned by the detour.
pub static HEAL_T0: AtomicU64 = AtomicU64::new(0);
/// MW2 mode drives and no AC-130 ride (the task's wish; heartbeat rule): a heal press drinks, the heal key is hidden.
pub static WANT_HEAL: AtomicBool = AtomicBool::new(false);
/// Heal presses that pressed ER's use-item key / that were refused (MW2 mode on, not driving: a menu, unfocused).
pub static HEAL_KEY_PULSES: AtomicU32 = AtomicU32::new(0);
/// 2026-10-24 (user: "can still shoot after death"): the task's death gate (`fp_logic::DeathGate`) holds (heartbeat
/// rule): no heal / melee pulse reaches ER; the heal key stays hidden.
pub static WANT_DEAD_BLOCK: AtomicBool = AtomicBool::new(false);
pub static HEAL_KEY_REFUSED: AtomicU32 = AtomicU32::new(0);

// ---- C6 the keyboard two-hand chord (task -> detour; docs/C6_HOLD_DESIGN.md 3.4) ---------------------------------

/// The keyboard chord's start (ms since boot; 0 = none): ER's E reads down for `CHORD_HOLD_MS`, its LMB for
/// `CHORD_TAP_FROM_MS..CHORD_TAP_TO_MS` (only inside E). Set / cleared by the task (`er_stance.rs`), never by the detour.
pub static KB_CHORD_T0: AtomicU64 = AtomicU64::new(0);
/// ER's two-hand modifier key (E, DIK 0x12) [I4].
pub const KB_CHORD_DIK: usize = 0x12;
/// C6 aim 4 (work/c6/aim4.md 1): without a pad, ER's aim input of a two-handed crossbow is its guard button, the right
/// mouse button on ER's default keyboard layout [I]: held in ER's mouse buffer while the task asks (heartbeat rule, never
/// in the gunship). The MW2 ADS state is recorded before (`RAW_ADS_DOWN`), so MW2 never sees this.
pub static KB_AIM: AtomicBool = AtomicBool::new(false);
/// `rgbButtons` index of ER's guard / aim button (RMB).
pub const KB_AIM_BUTTON: u8 = 1;

/// Clears the movement blank wishes (panic path, mode off): held keys stay hidden only through their latch.
pub fn clear_move_blank() {
    for i in 0..4 {
        MOVE_BLANK_LATCH[i].store(0, Ordering::SeqCst);
        MOVE_BLANK_PLAIN[i].store(0, Ordering::SeqCst);
    }
    // C3 PASS B: the pad's left stick follows W/A/S/D
    crate::pad::set_blank_ls(false);
}

// ---- M4 AC-130 input (detour -> task / camera detour) ---------------------------------------------

/// The task wants the gunship capture (mouse axes/buttons and the keyboard except Esc hidden from ER). Honoured only
/// through `effective_want` (the 250 ms heartbeat rule).
pub static WANT_GUNSHIP_INPUT: AtomicBool = AtomicBool::new(false);
/// Mouse counts accumulated by the input detour while capturing; the camera-step detour swaps them to 0 every frame.
pub static MOUSE_DX: AtomicI32 = AtomicI32::new(0);
pub static MOUSE_DY: AtomicI32 = AtomicI32::new(0);
/// Mouse wheel counts (lZ) for the task's gun switching.
pub static WHEEL: AtomicI32 = AtomicI32::new(0);
/// Total mouse counts seen while capturing (the AC130CAM `mouse_counts=` proof).
pub static MOUSE_COUNTS: AtomicU64 = AtomicU64::new(0);
/// The AC-130 keys (DIK codes, mirrored from the config) and their press counters, in this order:
/// ac130 (F7), exit, next, gun1, gun2, gun3, thermal.
pub const AC_KEYS: usize = 7;
pub const AC_KEY_CALL: usize = 0;
pub const AC_KEY_EXIT: usize = 1;
pub const AC_KEY_NEXT: usize = 2;
pub const AC_KEY_GUN1: usize = 3;
pub const AC_KEY_THERMAL: usize = 6;
pub static AC_DIK: [AtomicU8; AC_KEYS] = [
    AtomicU8::new(0x41),
    AtomicU8::new(0x0E),
    AtomicU8::new(0x10),
    AtomicU8::new(0x02),
    AtomicU8::new(0x03),
    AtomicU8::new(0x04),
    AtomicU8::new(0x21),
];
pub static AC_RAW_DOWN: [AtomicBool; AC_KEYS] = [const { AtomicBool::new(false) }; AC_KEYS];
pub static AC_PRESSES: [AtomicU32; AC_KEYS] = [const { AtomicU32::new(0) }; AC_KEYS];
/// Raw Esc press edges (DIK_ESCAPE), recorded by the input detour on every keyboard poll. A ride ends on an Esc edge
/// directly, so the pause menu never opens under the gunship capture even if the menu signal misses it (review M4
/// minor 4).
pub static ESC_RAW_DOWN: AtomicBool = AtomicBool::new(false);
pub static ESC_PRESSES: AtomicU32 = AtomicU32::new(0);

// ---- subsystem status ---------------------------------------------------------------------------

pub static DATA_OK: AtomicBool = AtomicBool::new(false);
pub static HUD_APPLIED: AtomicBool = AtomicBool::new(false);
pub static HUD_OK: AtomicBool = AtomicBool::new(false);
pub static HUD_DISABLED: AtomicBool = AtomicBool::new(false);
pub static INPUT_HOOK_OK: AtomicBool = AtomicBool::new(false);
pub static AUDIO_OK: AtomicBool = AtomicBool::new(false);
pub static TASK_OK: AtomicBool = AtomicBool::new(false);
pub static GAMEPLAY_DISABLED: AtomicBool = AtomicBool::new(false);
pub static BOOTED: AtomicBool = AtomicBool::new(false);
/// M3 viewmodel: disabled for the session after any failure; ready once the loader handed over the renderer.
pub static VM_DISABLED: AtomicBool = AtomicBool::new(false);
pub static VM_READY: AtomicBool = AtomicBool::new(false);
/// The game's main window (for the foreground check).
pub static GAME_HWND: AtomicUsize = AtomicUsize::new(0);

/// Turns gameplay off for the session after a task panic. Blanking flags are cleared first so
/// the game gets LMB and R back immediately.
pub fn disable_gameplay() {
    WANT_BLANK_FIRE.store(false, Ordering::SeqCst);
    WANT_BLANK_RELOAD.store(false, Ordering::SeqCst);
    WANT_BLANK_ADS.store(false, Ordering::SeqCst);
    WANT_GUNSHIP_INPUT.store(false, Ordering::SeqCst);
    // M6: weapon keys / wheel back to ER, no sensitivity scaling, no view kick / zoom
    WANT_BLANK_WEAPON.store(false, Ordering::SeqCst);
    WANT_WHEEL.store(false, Ordering::SeqCst);
    // M7: the nuke key back to ER
    WANT_BLANK_NUKE.store(false, Ordering::SeqCst);
    // 2026-10-21: the heal key back to ER, no use-item pulse
    WANT_HEAL.store(false, Ordering::SeqCst);
    WANT_DEAD_BLOCK.store(false, Ordering::SeqCst);
    SENS_SCALE_BITS.store(1.0f32.to_bits(), Ordering::SeqCst);
    VIEWFX_ON.store(false, Ordering::SeqCst);
    clear_move_blank();
    // C3 PASS B: the pad's buttons and sticks back to ER
    crate::pad::clear_wants();
    MODE_ON.store(false, Ordering::SeqCst);
    GAMEPLAY_DISABLED.store(true, Ordering::SeqCst);
}

// ---- clock --------------------------------------------------------------------------------------

static CLOCK: OnceLock<Instant> = OnceLock::new();

/// Starts the monotonic clock (boot thread, first thing).
pub fn init_clock() {
    CLOCK.get_or_init(Instant::now);
}

/// Seconds since boot (QPC-backed `Instant`).
pub fn now_s() -> f64 {
    CLOCK.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Milliseconds since boot without initialising anything (safe for the input detour).
pub fn now_ms_if_started() -> u64 {
    CLOCK.get().map_or(0, |c| c.elapsed().as_millis() as u64)
}

/// Microseconds since boot without initialising anything (safe for the camera-step detour).
pub fn now_us_if_started() -> u64 {
    CLOCK.get().map_or(0, |c| c.elapsed().as_micros() as u64)
}

// ---- locks that never poison the game -----------------------------------------------------------

/// Locks, recovering the data if a panicking thread poisoned the mutex.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Non-blocking lock (present thread); `None` when contended.
pub fn try_lock<T>(m: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match m.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

// ---- config -------------------------------------------------------------------------------------

static CONFIG: Mutex<Option<Arc<Config>>> = Mutex::new(None);

/// The current config (defaults until boot loads the file).
pub fn config() -> Arc<Config> {
    lock(&CONFIG).clone().unwrap_or_else(|| Arc::new(Config::default()))
}

/// Non-blocking variant for the present thread.
pub fn try_config() -> Option<Arc<Config>> {
    try_lock(&CONFIG).and_then(|g| g.clone())
}

/// Installs a config and mirrors the key settings for the input detour.
pub fn set_config(cfg: Arc<Config>) {
    TOGGLE_DIK.store(dik_from_name(&cfg.keys.toggle).unwrap_or(0x3F), Ordering::SeqCst);
    RELOAD_DIK.store(dik_from_name(&cfg.keys.reload).unwrap_or(0x13), Ordering::SeqCst);
    FIRE_BUTTON.store(cfg.keys.fire_mouse_button, Ordering::SeqCst);
    ADS_BUTTON.store(cfg.keys.ads_mouse_button, Ordering::SeqCst);
    BLANK_TOGGLE_KEY.store(cfg.keys.blank_toggle_key, Ordering::SeqCst);
    FP_TOGGLE_DIK.store(dik_from_name(&cfg.keys.fp_toggle).unwrap_or(0x40), Ordering::SeqCst);
    let k = &cfg.keys;
    for (i, (name, dflt)) in [
        (&k.ac130, 0x41u8),
        (&k.ac130_exit, 0x0E),
        (&k.ac130_next, 0x10),
        (&k.ac130_gun1, 0x02),
        (&k.ac130_gun2, 0x03),
        (&k.ac130_gun3, 0x04),
        (&k.ac130_thermal, 0x21),
    ]
    .into_iter()
    .enumerate()
    {
        AC_DIK[i].store(dik_from_name(name).unwrap_or(dflt), Ordering::SeqCst);
    }
    // M6: the weapon keys ("" = unbound)
    for (i, name) in [&k.weapon_next, &k.weapon_prev].into_iter().enumerate() {
        WEAPON_DIK[i].store(if name.is_empty() { 0 } else { dik_from_name(name).unwrap_or(0) }, Ordering::SeqCst);
    }
    // M7: the nuke key ("" = unbound; validated by the config parser)
    NUKE_DIK.store(if k.nuke.is_empty() { 0 } else { dik_from_name(&k.nuke).unwrap_or(0) }, Ordering::SeqCst);
    // 2026-10-21: the heal key ("" = none) and ER's use-item key (validated by the config parser)
    HEAL_DIK.store(if k.heal.is_empty() { 0 } else { dik_from_name(&k.heal).unwrap_or(0) }, Ordering::SeqCst);
    ER_USE_ITEM_DIK.store(dik_from_name(&k.er_use_item).unwrap_or(0x13), Ordering::SeqCst);
    // M5: the movement keys for the input detour (validated by the config parser)
    if let Ok(mk) = cfg.keys.move_keys() {
        for (i, d) in [mk.forward, mk.left, mk.back, mk.right].into_iter().enumerate() {
            MOVE_WASD_DIK[i].store(d, Ordering::SeqCst);
        }
        for (i, d) in mk.press_keys().into_iter().enumerate() {
            MOVE_PRESS_DIK[i].store(d, Ordering::SeqCst);
        }
    }
    // C3 PASS B: the gamepad bindings / look parameters for the XInput and mouse detours
    crate::pad::mirror_config(&cfg);
    *lock(&CONFIG) = Some(cfg);
}

// ---- HUD snapshot -------------------------------------------------------------------------------

/// Everything the HUD draws, copied by the task once per tick.
#[derive(Clone, Debug)]
pub struct HudSnapshot {
    /// Time of the tick that wrote it (0 = never written).
    pub t: f64,
    pub in_world: bool,
    pub mode_on: bool,
    pub menu_open: bool,
    pub foreground: bool,
    pub mag: u32,
    pub reserve: u32,
    pub low_ammo_count: u32,
    pub reloading: bool,
    pub reload_progress: f32,
    pub spread_deg: f32,
    pub reticle_side: f32,
    pub vfov_raw: f32,
    pub last_hit_t: f64,
    pub last_hit_kill: bool,
    pub shots: u64,
    pub hits: u64,
    pub chars: u32,
    /// M3: the viewmodel animation state (posed by the present thread at its own clock).
    pub vm: VmState,
    pub ads_frac: f32,
    pub cam_valid: bool,
    /// M4: the AC-130 state for the thermal pass and the HUD.
    pub ac130: Ac130Hud,
    /// Run 15: first person is On this tick (the viewmodel only draws in first person unless configured).
    pub fp_on: bool,
    /// M8: the gun is down (mantle row >= 33 in, ladder): no crosshair until the quick raise ended.
    pub weapon_lowered: bool,
    /// C6 aim 4: ER's crossbow aim runs in third person (`tp_ads_pose = "er_aim"`): a centre dot while ADS hides the
    /// crosshair (there is no scope in third person).
    pub tp_aim_dot: bool,
    /// `[viewmodel] draw_in_third_person`.
    pub vm_draw_tp: bool,
    /// M5: the controller's stance for the HUD icon (0 stand, 1 crouch, 2 prone; 255 = the controller is not driving).
    pub stance: u8,
    /// M5: the viewmodel's stance pose + weapon bob, added to `[viewmodel] offset_in` (inches, renderer axes).
    pub vm_offset_in: [f32; 3],
    /// M6: the gun in hand (the user-facing name), its fire-mode tag ("BURST" / "2x" / ""), the switch list
    /// (previous, target, next) shown until `switch_until`.
    pub gun_name: String,
    pub gun_tag: &'static str,
    pub switch_names: [String; 3],
    pub switch_until: f64,
    /// M6: the scope overlay opacity (0 = none; the viewmodel is hidden while > 0).
    pub scope: f32,
    /// M6 lock-on: 0 none, 1 locking, 2 locked, 3 "NO LOCK" flash; the target point (world) for the brackets.
    pub lock: u8,
    pub lock_target: Option<[f32; 3]>,
    pub lock_point: bool,
    /// M6: the camera this tick (for projecting the lock target and the projectiles in the HUD).
    pub cam_pos: [f32; 3],
    pub cam_rows: [[f32; 3]; 3],
    /// M6: live launcher projectiles (head, tail) for the HUD streaks.
    pub projectiles: Vec<([f32; 3], [f32; 3])>,
    /// M7: the nuke counter / countdown / hints and the post pass of this tick.
    pub nuke: NukeHud,
    /// C3 (docs/C3_DESIGN.md 2.3): the UI yield 0..1 (the gun drops / our HUD dims while ER shows a message) and its
    /// signal bits (`mw2er_core::ui_yield::SIG_*`). Render-only.
    pub ui_yield: f32,
    pub ui_yield_why: u32,
    /// C5 `[ui_yield] mode = "cutout"`: the message regions the viewmodel and the world guns leave alone this tick
    /// (the first `ui_cut_n`, strongest first; screen fractions). Render-only.
    pub ui_cut: [mw2er_core::ui_yield::CutRect; mw2er_core::ui_yield::MAX_CUT_RECTS],
    pub ui_cut_n: u8,
    /// C3 (2.6): live damage numbers (our attributed hits) and the eye they are projected from (the FP eye in first
    /// person, ER's camera otherwise; rows = `cam_rows`).
    pub dmg_nums: Vec<mw2er_core::dmgnum::DmgNum>,
    pub view_pos: [f32; 3],
    /// C3 (2.6, `suppress_with_er_tag`): targets ER's own enemy tags show this tick.
    pub er_tag_handles: [u64; 8],
    /// C3 PASS C: the peers' tracers (a, b, alpha) and explosion flashes (position, alpha, size m) for the HUD, and the
    /// world guns for the renderer (drawn with `view_pos` / `cam_rows` / `vfov_raw`).
    pub peer_segs: Vec<([f32; 3], [f32; 3], f32, bool)>,
    pub peer_flashes: Vec<([f32; 3], f32, f32)>,
    pub world_guns: Vec<crate::coop_vis::WorldGunDraw>,
    /// C6 depth fallback: the body capsules the world-gun pass draws into its own depth first (third person only).
    pub world_occl: Vec<mw2er_core::body_occl::Capsule>,
    /// C6 depth 5: ER's camera near / far plane (CSCam `near_plane` / `far_plane`; 0 = unread): the prime's linearisation.
    pub er_near: f32,
    pub er_far: f32,
}

/// M7: what the HUD and the post pass need of the nuke (all `Copy`: no allocation per frame).
#[derive(Clone, Copy, Debug)]
pub struct NukeHud {
    /// Show `Nuke: n/cost` (MW2 mode on, not in the AC-130, no sequence past the detonation).
    pub show_counter: bool,
    pub streak: u32,
    pub cost: u32,
    pub earned: bool,
    /// `nuke.always_available` (the counter reads READY (test)).
    pub test: bool,
    /// The countdown (seconds left) while counting down.
    pub countdown: Option<f32>,
    /// `Friendly tactical nuke inbound!` until this time.
    pub inbound_until: f64,
    /// The earn hint `Press [4] for TACTICAL NUKE.` until this time.
    pub hint_until: f64,
    /// A refusal line (0 none, 1 already inbound, 2 the AC-130, 3 not earned, 4 co-op session) until `refusal_until`.
    pub refusal: u8,
    pub refusal_until: f64,
    /// Co-op C2: the one-time "Co-op nuke: ..." note until this time.
    pub coop_note_until: f64,
    /// The post pass of this tick (flash / vision amount / aftermath blend) and the blended vision.
    pub post: mw2er_core::nuke_logic::PostState,
    pub vis: mw2er_core::nuke_fx::Vis,
}

impl NukeHud {
    pub const fn empty() -> Self {
        NukeHud {
            show_counter: false,
            streak: 0,
            cost: 25,
            earned: false,
            test: false,
            countdown: None,
            inbound_until: f64::NEG_INFINITY,
            hint_until: f64::NEG_INFINITY,
            refusal: 0,
            refusal_until: f64::NEG_INFINITY,
            coop_note_until: f64::NEG_INFINITY,
            post: mw2er_core::nuke_logic::PostState { flash: 0.0, amount: 0.0, blend: 0.0 },
            vis: mw2er_core::nuke_fx::Vis {
                film: Film::BLACK,
                cutoff: 1.0,
                glow_desat: 0.0,
                intensity: 0.0,
                radius: 1.0,
            },
        }
    }
}

/// What the present thread needs of the AC-130 (copied by the task every tick).
#[derive(Clone, Debug)]
pub struct Ac130Hud {
    /// A ride is Active: thermal + gunship HUD + no viewmodel.
    pub active: bool,
    /// now_s() when the ride became Active.
    pub t0: f64,
    pub duration_s: f64,
    pub gun: usize,
    pub clips: [u32; 3],
    pub clip_size: [u32; 3],
    pub reloading: [bool; 3],
    /// Thermal: on (config + data) and the vision transition (white/black hot) in progress.
    pub thermal: bool,
    pub grain: bool,
    pub luma: [f32; 3],
    /// Mid-tone lift of the film input (`[ac130] gamma`, run 8).
    pub gamma: f32,
    pub film_from: Film,
    pub film_to: Film,
    pub film_t0: f64,
    pub film_dur: f64,
    pub fade_s: f64,
    pub black_hot: bool,
    /// The gunship camera (the detour's last write) for tracers / markers.
    pub cam_pos: [f32; 3],
    pub cam_rows: [[f32; 3]; 3],
    pub vfov: f32,
    /// The rounds of this ride (run 8 tracers; a fixed `Copy` ring) and nearby enemy chests (debug markers).
    pub tracers: TracerRing,
    pub markers: Vec<[f32; 3]>,
    /// Last 105 impact (dark pulse) and the last AC-130 hit (hitmarker).
    pub pulse_t: f64,
    pub hit_t: f64,
    pub hit_kill: bool,
    pub agl_m: f32,
    /// MW2-mode indicator line and its kind (0 ready, 1 cooldown, 2 unavailable, 3 busy) + the ready toast time.
    pub indicator: String,
    pub indicator_kind: u8,
    pub ready_toast_t: f64,
}

impl Ac130Hud {
    pub const fn empty() -> Self {
        Ac130Hud {
            active: false,
            t0: 0.0,
            duration_s: 40.0,
            gun: 0,
            clips: [0; 3],
            clip_size: [0; 3],
            reloading: [false; 3],
            thermal: false,
            grain: false,
            luma: [0.2126, 0.7152, 0.0722],
            gamma: 1.0,
            film_from: Film::BLACK,
            film_to: Film::BLACK,
            film_t0: 0.0,
            film_dur: 0.0,
            fade_s: 1.0,
            black_hot: false,
            cam_pos: [0.0; 3],
            cam_rows: [[0.0; 3]; 3],
            vfov: 0.6,
            tracers: TracerRing::new(),
            markers: Vec::new(),
            pulse_t: f64::NEG_INFINITY,
            hit_t: f64::NEG_INFINITY,
            hit_kill: false,
            agl_m: 0.0,
            indicator: String::new(),
            indicator_kind: 2,
            ready_toast_t: f64::NEG_INFINITY,
        }
    }
}

impl HudSnapshot {
    pub const fn empty() -> Self {
        HudSnapshot {
            weapon_lowered: false,
            tp_aim_dot: false,
            t: 0.0,
            in_world: false,
            mode_on: false,
            menu_open: false,
            foreground: false,
            mag: 0,
            reserve: 0,
            low_ammo_count: 0,
            reloading: false,
            reload_progress: 0.0,
            spread_deg: 0.0,
            reticle_side: 8.0,
            vfov_raw: 0.0,
            last_hit_t: f64::NEG_INFINITY,
            last_hit_kill: false,
            shots: 0,
            hits: 0,
            chars: 0,
            vm: VmState::hidden(),
            ads_frac: 0.0,
            cam_valid: false,
            ac130: Ac130Hud::empty(),
            fp_on: false,
            vm_draw_tp: false,
            stance: 255,
            vm_offset_in: [0.0; 3],
            gun_name: String::new(),
            gun_tag: "",
            switch_names: [String::new(), String::new(), String::new()],
            switch_until: f64::NEG_INFINITY,
            scope: 0.0,
            lock: 0,
            lock_target: None,
            lock_point: false,
            cam_pos: [0.0; 3],
            cam_rows: [[0.0; 3]; 3],
            projectiles: Vec::new(),
            nuke: NukeHud::empty(),
            ui_yield: 0.0,
            ui_yield_why: 0,
            ui_cut: [mw2er_core::ui_yield::CutRect { rect: [0.0; 4], strength: 0.0, bit: 0 }; mw2er_core::ui_yield::MAX_CUT_RECTS],
            ui_cut_n: 0,
            dmg_nums: Vec::new(),
            view_pos: [0.0; 3],
            er_tag_handles: [0; 8],
            peer_segs: Vec::new(),
            peer_flashes: Vec::new(),
            world_guns: Vec::new(),
            world_occl: Vec::new(),
            er_near: 0.0,
            er_far: 0.0,
        }
    }
}

pub static SNAPSHOT: Mutex<HudSnapshot> = Mutex::new(HudSnapshot::empty());

// ---- toast --------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub until: f64,
    pub error: bool,
}

pub static TOAST: Mutex<Toast> = Mutex::new(Toast { text: String::new(), until: 0.0, error: false });

/// Shows a top-left message for `secs` seconds.
pub fn toast(text: impl Into<String>, secs: f64, error: bool) {
    *lock(&TOAST) = Toast { text: text.into(), until: now_s() + secs, error };
}

// ---- audio queue --------------------------------------------------------------------------------

/// One request to the audio thread.
#[derive(Clone, Copy, Debug)]
pub enum AudioMsg {
    /// An M2 weapon cue.
    Sfx(SfxCue),
    /// M4: an AC-130 cue (variant, gain, pitch and delay already picked by the task).
    Ac(Play),
    /// M6: an arsenal sound alias (id into `arsenal_rt::SOUNDS`); `fire` picks the fire gain.
    Gun { alias: u16, fire: bool },
    /// C3 PASS C: a peer's gun (its third-person alias) heard at a distance: `gain` = distance x `[coop] peer_gain`.
    GunAt { alias: u16, gain: f32 },
    /// M7: a nuke alias (index in nuke.json's sound key order; variant, gain, pitch, delay picked by the task).
    Nuke(NukePlay),
}

/// M7: one nuke sound request.
#[derive(Clone, Copy, Debug)]
pub struct NukePlay {
    pub key: u8,
    pub variant: u8,
    pub gain: f32,
    pub speed: f32,
    pub delay_ms: u32,
    /// The sequence generation it belongs to (a cancel bumps `NUKE_AUDIO_GEN` and stops it).
    pub generation: u32,
}

/// M7: the current nuke sound generation (bumped on a cancel / sequence end at loading).
pub static NUKE_AUDIO_GEN: AtomicU32 = AtomicU32::new(0);

/// M7: queues a nuke sound (never blocks).
pub fn play_nuke(p: NukePlay) {
    send(AudioMsg::Nuke(p));
}

// M4 gunship ambient loop: state, not messages (review M4 minor 5). The audio thread polls these every <= 20 ms, so a
// full or contended queue can never leave the loop playing.
/// The ambient should play.
pub static AMBIENT_WANTED: AtomicBool = AtomicBool::new(false);
/// Incremented on every start request (the audio thread starts once per generation).
pub static AMBIENT_GEN: AtomicU32 = AtomicU32::new(0);
/// Start gain (f32 bits) and stop fade (ms).
pub static AMBIENT_GAIN: AtomicU32 = AtomicU32::new(0);
pub static AMBIENT_FADE_MS: AtomicU32 = AtomicU32::new(300);

pub static SFX_TX: OnceLock<SyncSender<AudioMsg>> = OnceLock::new();

fn send(m: AudioMsg) {
    if let Some(tx) = SFX_TX.get() {
        let _ = tx.try_send(m);
    }
}

/// Queues a sound; drops it if audio is off or the queue is full. Never blocks.
pub fn play(cue: SfxCue) {
    send(AudioMsg::Sfx(cue));
}

/// M4: queues an AC-130 sound (never blocks).
pub fn play_ac(p: Play) {
    send(AudioMsg::Ac(p));
}

/// M6: queues an arsenal alias (never blocks).
pub fn play_gun(alias: u16, fire: bool) {
    send(AudioMsg::Gun { alias, fire });
}

/// C3 PASS C: queues a peer's gun alias at `gain` (never blocks).
pub fn play_gun_at(alias: u16, gain: f32) {
    if gain.is_finite() && gain > 0.0 {
        send(AudioMsg::GunAt { alias, gain });
    }
}

/// The gunship ambient on (a new generation) / off (fade-out). Never blocks and is never dropped.
pub fn ambient(on: bool, gain: f32, fade_ms: u32) {
    if on {
        AMBIENT_GAIN.store(gain.to_bits(), Ordering::SeqCst);
        AMBIENT_GEN.fetch_add(1, Ordering::SeqCst);
        AMBIENT_WANTED.store(true, Ordering::SeqCst);
    } else {
        AMBIENT_FADE_MS.store(fade_ms, Ordering::SeqCst);
        AMBIENT_WANTED.store(false, Ordering::SeqCst);
    }
}
