//! C3 PASS B (docs/C3_DESIGN.md part 4): the XInput gamepad.
//!
//! A MinHook detour on `xinput1_4!XInputGetState` (the system DLL's export, found with GetModuleHandleW +
//! GetProcAddress; eldenring.exe's IAT and code are never touched, ER's PadDevice is never written). The detour calls
//! the original, then, only on success (return 0) and only for the selected slot:
//! - records levels, press edges (into the SAME counters the keyboard detour bumps: F5 toggle, F6, reload, weapon
//!   keys, nuke, AC-130 keys, the movement press counters) and the sticks into atomics;
//! - hides the bound buttons / triggers / sticks from ER while MW2 mode drives (heartbeat rule, per-button release
//!   latch), everything but Start during an AC-130 ride, and synthesizes ER's interact button while X is held (at
//!   ER's action prompt from the press, 2026-10-21; for at least INTERACT_PULSE_MS) (and,
//!   2026-10-21, ER's use-item button for a moment after a heal press, D-pad right: the flask).
//!
//! The caller's `XINPUT_STATE` is the only memory written (the `input_hook.rs` precedent; not game memory). When MW2
//! mode is off, a menu is open, the window is unfocused or the task heartbeat is stale the state passes through
//! unchanged (apart from the toggle chord, hidden like F5 with `[keys] blank_toggle_key`).
//!
//! The detour runs on ER's pad thread: no allocation, no locks, no logging, no panics (`catch_unwind` around the body).
//! The task half (`PadTask`) mirrors the wishes, logs (`PAD ...`) and calibrates ER's mouse degrees per count.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::mh::{MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::config::Config;
use mw2er_core::input_logic::{effective_want, fresh};
use mw2er_core::move_input::{PRESS_INTERACT, PRESS_JUMP, PRESS_SPRINT, PRESS_STANCE_HOLD};
use mw2er_core::pad_logic::{
    self, ACT_AC130, ACT_ADS, ACT_FIRE, ACT_FP, ACT_GS_EXIT, ACT_GS_FIRE, ACT_GS_NEXT, ACT_GS_THERMAL, ACT_HEAL, ACT_INTERACT,
    ACT_JUMP, ACT_NUKE, ACT_RELOAD, ACT_SPRINT, ACT_STANCE, ACT_TOGGLE, ACT_WEAPON_NEXT, ACT_WEAPON_PREV, Binds, Calib,
    LookParams, LookState, N_ACT, Raw, StepIn, StepOut, StepState, TRIGGER_RELEASE, apply_buttons, look_counts, look_rates,
    reshape_for_er, step,
};
use mw2er_core::vec3::V3;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::core::{s, w};

use crate::log::{log_i, log_w};
use crate::shared::{
    AC_KEY_CALL, AC_KEY_EXIT, AC_KEY_NEXT, AC_KEY_THERMAL, AC_PRESSES, BLANK_TOGGLE_KEY, LAST_GDS_MOUSE_MS, MOUSE_DX,
    MOUSE_DY, PRESSES_FP, PRESSES_MOVE, PRESSES_NUKE, PRESSES_RELOAD, PRESSES_TOGGLE, PRESSES_WEAPON, TASK_HEARTBEAT_MS,
    WANT_GUNSHIP_INPUT, now_ms_if_started, now_us_if_started,
};

// ---- config mirror (task / config watcher -> detours) ----------------------------------------------------------------

static ENABLED: AtomicBool = AtomicBool::new(false);
static PAD_INDEX: AtomicI32 = AtomicI32::new(-1);
static MASKS: [AtomicU32; N_ACT] = [const { AtomicU32::new(0) }; N_ACT];
static BLANK_MASK: AtomicU32 = AtomicU32::new(0);
static BLANK_RS: AtomicBool = AtomicBool::new(false);
static BLANK_LS: AtomicBool = AtomicBool::new(false);
static ER_INTERACT: AtomicU32 = AtomicU32::new(pad_logic::XI_Y);
static ER_MELEE: AtomicU32 = AtomicU32::new(pad_logic::XI_RB);
static ER_USE_ITEM: AtomicU32 = AtomicU32::new(pad_logic::XI_X);
static HOLD_MS: AtomicU32 = AtomicU32::new(300);
static TRIG_THR: AtomicU8 = AtomicU8::new(30);
static ROUTE_MOUSE: AtomicBool = AtomicBool::new(true);
static LS_DZ_BITS: AtomicU32 = AtomicU32::new(0x3e4c_cccd); // 0.20
static LS_OUTER_BITS: AtomicU32 = AtomicU32::new(0x3f7a_e148); // 0.98
/// Look parameters (f32 bits): yaw, pitch, yaw_ads, pitch_ads, sens, accel_lerp, accel_max_yaw, dz, outer.
static LOOK_F: [AtomicU32; 9] = [const { AtomicU32::new(0) }; 9];
/// bit 0 invert_y, bit 1 accel; graph index in bits 8..15.
static LOOK_FLAGS: AtomicU32 = AtomicU32::new(0x0302);
/// ER's degrees per mouse count in use (f32 bits; calibrated or configured) and the AC-130 counts per stick degree.
static DEG_PER_COUNT_BITS: AtomicU32 = AtomicU32::new(0x3da3_d70a); // 0.08 provisional [I]
static GS_COUNTS_PER_DEG_BITS: AtomicU32 = AtomicU32::new(0);

/// Provisional degrees per count until `PAD calib` measured ER's [I].
const PROVISIONAL_DEG_PER_COUNT: f32 = 0.08;
/// The stick's look goes through the mouse path only while ER polled the mouse this recently.
const MOUSE_FRESH_MS: u64 = 100;
/// Pad levels count for the task only while the selected slot was read this recently.
const PAD_FRESH_MS: u64 = 150;

// ---- task -> detours --------------------------------------------------------------------------------------------------

/// MW2 mode drives (mode on, in world, no menu, focused, not in the AC-130): the detour's blank / action wish.
static WANT_PAD: AtomicBool = AtomicBool::new(false);
/// 2026-10-21: ER's action prompt is on screen (`[gamepad] prompt_indices`): an X press is ER's interact at once.
static WANT_PROMPT: AtomicBool = AtomicBool::new(false);
/// The left stick is hidden from ER (the W/A/S/D rule of movement, `move_input::wasd_blanked`).
static WANT_BLANK_LS: AtomicBool = AtomicBool::new(false);
static ADS_FRAC_BITS: AtomicU32 = AtomicU32::new(0);
/// C6 (docs/C6_HOLD_DESIGN.md 3.4-3.5, 6): the task's two-hand chord request counter, the stance-active melee block and
/// the held ER aim probe (both under the heartbeat rule).
static CHORD_REQ: AtomicU32 = AtomicU32::new(0);
/// C6: when the last chord request was made (ms): a request the detour sees later than CHORD_STALE_MS (the pad was not
/// read meanwhile; the task gave it up) is consumed without a chord.
static CHORD_REQ_MS: AtomicU64 = AtomicU64::new(0);
const CHORD_STALE_MS: u64 = 500;
static WANT_MELEE_OFF: AtomicBool = AtomicBool::new(false);
/// C6 part 16.4 guard 2: R3 melee is also blocked while the real crossbow carrier is equipped (RB is its shot), set by
/// the carrier itself (independent of the stance: a stance panic / exit never re-enables it while the crossbow is on).
static WANT_MELEE_OFF_CARRIER: AtomicBool = AtomicBool::new(false);
static WANT_ER_AIM: AtomicBool = AtomicBool::new(false);

// ---- detour state and outputs -----------------------------------------------------------------------------------------

static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static SELECTED: AtomicI32 = AtomicI32::new(-1);
static ST_PREV: AtomicU32 = AtomicU32::new(0);
static ST_LATCHED: AtomicU32 = AtomicU32::new(0);
static ST_CHORD: AtomicU32 = AtomicU32::new(0);
static ST_HOLD_T0: AtomicU64 = AtomicU64::new(0);
static ST_HOLD_FIRED: AtomicBool = AtomicBool::new(false);
static ST_MELEE_T0: AtomicU64 = AtomicU64::new(0);
static ST_HEAL_T0: AtomicU64 = AtomicU64::new(0);
static ST_INTERACT_T0: AtomicU64 = AtomicU64::new(0);
static ST_CHORD_T0: AtomicU64 = AtomicU64::new(0);
static ST_CHORD_SEQ: AtomicU32 = AtomicU32::new(0);
/// C6: the last chord event: request seq (bits 16..31), `ChordEv::code` (8..15), reason code (0..7).
static CHORD_EV: AtomicU32 = AtomicU32::new(0);
/// C6: R3 presses suppressed while the stance was active.
static MELEE_SUPPRESSED: AtomicU32 = AtomicU32::new(0);
/// Review C3: `process` and `mouse_look` are serialized by try-lock flags (XInputGetState may be called from two
/// threads, e.g. ER's pad thread and an overlay); a contended call passes the state through with the last read's
/// hidden / synthesized buttons and stick blanks (no step, no edge, no state change).
static STEP_BUSY: AtomicBool = AtomicBool::new(false);
static LOOK_BUSY: AtomicBool = AtomicBool::new(false);
static LAST_HIDE: AtomicU32 = AtomicU32::new(0);
static LAST_SYNTH: AtomicU32 = AtomicU32::new(0);
/// Bit 0: the last processed read zeroed the left stick; bit 1: the right stick.
static LAST_STICKS: AtomicU32 = AtomicU32::new(0);
static CONTENDED: AtomicU64 = AtomicU64::new(0);

/// Releases a try-lock flag on drop (also on an unwind inside the guarded body).
struct Busy(&'static AtomicBool);

impl Busy {
    fn try_take(f: &'static AtomicBool) -> Option<Busy> {
        f.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok().then_some(Busy(f))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The number of contended detour calls so far (`PAD stat ... contended=`).
pub fn contended() -> u64 {
    CONTENDED.load(Ordering::Relaxed)
}
static HELD: AtomicU32 = AtomicU32::new(0);
static LAST_OK_MS: AtomicU64 = AtomicU64::new(0);
static PKT: AtomicU32 = AtomicU32::new(0);
/// Sticks as packed i16 pairs (x low, y high).
static RAW_LS: AtomicU32 = AtomicU32::new(0);
static RAW_RS: AtomicU32 = AtomicU32::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static SEEN_SLOTS: AtomicU32 = AtomicU32::new(0);
static BLANKED_READS: AtomicU64 = AtomicU64::new(0);
static INTERACT_HOLDS: AtomicU32 = AtomicU32::new(0);
/// 2026-10-21: the interacts among INTERACT_HOLDS that were a press at ER's prompt (not a hold).
static INTERACT_TAPS: AtomicU32 = AtomicU32::new(0);
/// 2026-10-21: heal presses that pulsed ER's use-item button (`HEAL press src=pad`).
static HEAL_PULSES: AtomicU32 = AtomicU32::new(0);
/// The stick turned ER's camera through the mouse path in the last read / mouse reads were stale (route fallback).
static LOOK_VIA_MOUSE: AtomicBool = AtomicBool::new(false);
static LOOK_FALLBACK_READS: AtomicU64 = AtomicU64::new(0);
// mouse look path
static LAST_LOOK_US: AtomicU64 = AtomicU64::new(0);
static LOOK_EXTRA_BITS: AtomicU32 = AtomicU32::new(0);
static LOOK_CARRY: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];
static LOOK_COUNTS: AtomicU64 = AtomicU64::new(0);
/// Counts delivered to ER on lX (the user's mouse after the ADS scaling + ours), for the calibration.
static DELIVERED_DX: AtomicI64 = AtomicI64::new(0);
// gunship path
static LAST_GS_US: AtomicU64 = AtomicU64::new(0);
static GS_EXTRA_BITS: AtomicU32 = AtomicU32::new(0);
static GS_CARRY: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];

#[repr(C)]
struct XState {
    packet: u32,
    pad: [u8; 12],
}

type GetStateFn = unsafe extern "system" fn(u32, *mut XState) -> u32;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

fn f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

fn pack(x: i16, y: i16) -> u32 {
    (x as u16 as u32) | ((y as u16 as u32) << 16)
}

fn unpack(v: u32) -> (i16, i16) {
    (v as u16 as i16, (v >> 16) as u16 as i16)
}

fn load_binds() -> Binds {
    let mut b = Binds::default();
    for (i, m) in MASKS.iter().enumerate() {
        b.masks[i] = m.load(Ordering::Relaxed);
    }
    b.blank = BLANK_MASK.load(Ordering::Relaxed);
    b.blank_rs = BLANK_RS.load(Ordering::Relaxed);
    b.blank_ls = BLANK_LS.load(Ordering::Relaxed);
    b.er_interact = ER_INTERACT.load(Ordering::Relaxed);
    b.er_melee = ER_MELEE.load(Ordering::Relaxed);
    b.er_use_item = ER_USE_ITEM.load(Ordering::Relaxed);
    b
}

fn load_look() -> LookParams {
    let fl = LOOK_FLAGS.load(Ordering::Relaxed);
    LookParams {
        yaw: f(&LOOK_F[0]),
        pitch: f(&LOOK_F[1]),
        yaw_ads: f(&LOOK_F[2]),
        pitch_ads: f(&LOOK_F[3]),
        sens: f(&LOOK_F[4]),
        accel_lerp: f(&LOOK_F[5]),
        accel_max_yaw: f(&LOOK_F[6]),
        dz: f(&LOOK_F[7]),
        outer: f(&LOOK_F[8]),
        invert_y: fl & 1 != 0,
        accel: fl & 2 != 0,
        graph: (fl >> 8) as u8,
    }
}

/// Mirrors `[gamepad]` for the detours (called by `shared::set_config` at boot and on every hot reload). A bind table
/// that does not resolve cannot reach here (the parser rejects it); defensively it disables the pad.
pub fn mirror_config(cfg: &Config) {
    let g = &cfg.gamepad;
    let binds = g.binds();
    ENABLED.store(g.enabled && binds.is_ok(), Ordering::SeqCst);
    let b = binds.unwrap_or_default();
    for (i, m) in MASKS.iter().enumerate() {
        m.store(b.masks[i], Ordering::SeqCst);
    }
    BLANK_MASK.store(b.blank, Ordering::SeqCst);
    BLANK_RS.store(b.blank_rs, Ordering::SeqCst);
    BLANK_LS.store(b.blank_ls, Ordering::SeqCst);
    ER_INTERACT.store(b.er_interact, Ordering::SeqCst);
    ER_MELEE.store(b.er_melee, Ordering::SeqCst);
    ER_USE_ITEM.store(b.er_use_item, Ordering::SeqCst);
    PAD_INDEX.store(g.pad_index, Ordering::SeqCst);
    HOLD_MS.store(g.hold_ms, Ordering::SeqCst);
    TRIG_THR.store(g.trigger_threshold, Ordering::SeqCst);
    ROUTE_MOUSE.store(g.route_mouse(), Ordering::SeqCst);
    LS_DZ_BITS.store(g.ls_deadzone.to_bits(), Ordering::SeqCst);
    LS_OUTER_BITS.store(g.ls_outer.to_bits(), Ordering::SeqCst);
    let l = g.look();
    for (i, v) in [l.yaw, l.pitch, l.yaw_ads, l.pitch_ads, l.sens, l.accel_lerp, l.accel_max_yaw, l.dz, l.outer].into_iter().enumerate() {
        LOOK_F[i].store(v.to_bits(), Ordering::SeqCst);
    }
    LOOK_FLAGS.store(l.invert_y as u32 | (l.accel as u32) << 1 | (l.graph as u32) << 8, Ordering::SeqCst);
    if g.mouse_deg_per_count > 0.0 {
        DEG_PER_COUNT_BITS.store(g.mouse_deg_per_count.to_bits(), Ordering::SeqCst);
    }
    let gs = if g.gunship_counts_per_deg > 0.0 {
        g.gunship_counts_per_deg
    } else {
        1.0 / (cfg.ac130.mouse_deg_per_count * cfg.ac130.mouse_sens).max(1e-4)
    };
    GS_COUNTS_PER_DEG_BITS.store(gs.to_bits(), Ordering::SeqCst);
}

/// Panic path / gameplay disabled: nothing hidden any more (held buttons stay hidden only through their latch).
pub fn clear_wants() {
    WANT_PAD.store(false, Ordering::SeqCst);
    WANT_BLANK_LS.store(false, Ordering::SeqCst);
    WANT_PROMPT.store(false, Ordering::SeqCst);
    WANT_MELEE_OFF.store(false, Ordering::SeqCst);
    WANT_MELEE_OFF_CARRIER.store(false, Ordering::SeqCst);
    WANT_ER_AIM.store(false, Ordering::SeqCst);
}

/// C6 part 16.4: the carrier's own melee block (true while our crossbow is in the slot).
pub fn set_carrier_melee_off(on: bool) {
    WANT_MELEE_OFF_CARRIER.store(on, Ordering::Relaxed);
}

/// C6: the stance's wishes for the detour: R3 melee blocked, ER's aim button held (C6 aim 4: the third-person ADS
/// episode of `tp_ads_pose = "er_aim"`, only while it holds the input).
pub fn set_stance_wants(melee_off: bool, er_aim: bool) {
    WANT_MELEE_OFF.store(melee_off, Ordering::Relaxed);
    WANT_ER_AIM.store(er_aim, Ordering::Relaxed);
}

/// C6: asks the detour for one two-hand chord; returns its request number (the event carries it back).
pub fn chord_request() -> u32 {
    CHORD_REQ_MS.store(now_ms_if_started(), Ordering::SeqCst);
    CHORD_REQ.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}

/// C6: the last chord event as (request seq & 0xFFFF, event code, reason).
pub fn chord_event() -> (u32, u32, &'static str) {
    let v = CHORD_EV.load(Ordering::Relaxed);
    (v >> 16, (v >> 8) & 0xff, pad_logic::CHORD_REASONS.get((v & 0xff) as usize).copied().unwrap_or(""))
}

/// C6: R3 presses suppressed so far (the stance was active).
pub fn melee_suppressed() -> u32 {
    MELEE_SUPPRESSED.load(Ordering::Relaxed)
}

/// Movement's W/A/S/D blank rule, applied to the left stick.
pub fn set_blank_ls(on: bool) {
    WANT_BLANK_LS.store(on, Ordering::Relaxed);
}

// ---- getters for the task / movement / first person ---------------------------------------------------------------

/// 2026-10-21: heal presses that pulsed ER's use-item button so far, and that button's name (for `HEAL` lines).
pub fn heal_pulses() -> u32 {
    HEAL_PULSES.load(Ordering::Relaxed)
}

pub fn er_use_item_name() -> String {
    pad_logic::mask_name(ER_USE_ITEM.load(Ordering::Relaxed))
}

/// The selected pad was read recently (its levels count).
pub fn pad_fresh() -> bool {
    ENABLED.load(Ordering::Relaxed) && fresh(now_ms_if_started(), LAST_OK_MS.load(Ordering::Relaxed), PAD_FRESH_MS)
}

fn held(act: usize) -> bool {
    pad_fresh() && HELD.load(Ordering::Relaxed) & (1 << act) != 0
}

/// RT (fire) held; in the AC-130 the gunship fire binding.
pub fn fire_down() -> bool {
    held(ACT_FIRE)
}

pub fn gunship_fire_down() -> bool {
    held(ACT_GS_FIRE)
}

pub fn ads_down() -> bool {
    held(ACT_ADS)
}

/// Movement levels in `RAW_KEYS` layout (bit `PRESS_*`): sprint, jump, stance hold.
pub fn move_levels() -> u32 {
    if !pad_fresh() {
        return 0;
    }
    let h = HELD.load(Ordering::Relaxed);
    let mut l = 0u32;
    if h & (1 << ACT_SPRINT) != 0 {
        l |= 1 << PRESS_SPRINT;
    }
    if h & (1 << ACT_JUMP) != 0 {
        l |= 1 << PRESS_JUMP;
    }
    if h & (1 << ACT_STANCE) != 0 {
        l |= 1 << PRESS_STANCE_HOLD;
    }
    l
}

fn left_stick() -> (i16, i16) {
    unpack(RAW_LS.load(Ordering::Relaxed))
}

/// The left stick as W/A/S/D bits (`shared::MOVE_W` ..) for the keyboard-level checks.
pub fn move_bits() -> u8 {
    if !pad_fresh() {
        return 0;
    }
    let (x, y) = left_stick();
    pad_logic::stick_move_bits(x, y, f(&LS_DZ_BITS), f(&LS_OUTER_BITS))
}

/// The left stick's analog usercmd (forwardmove, rightmove) outside its deadzone.
pub fn analog() -> Option<(f32, f32)> {
    if !pad_fresh() {
        return None;
    }
    let (x, y) = left_stick();
    pad_logic::analog_cmd(x, y, f(&LS_DZ_BITS), f(&LS_OUTER_BITS))
}

// ---- the XInputGetState detour ----------------------------------------------------------------------------------------

fn bump(presses: u32) {
    let c = |a: usize, ctr: &AtomicU32| {
        if presses & (1 << a) != 0 {
            ctr.fetch_add(1, Ordering::Relaxed);
        }
    };
    c(ACT_TOGGLE, &PRESSES_TOGGLE);
    c(ACT_FP, &PRESSES_FP);
    c(ACT_RELOAD, &PRESSES_RELOAD);
    c(ACT_INTERACT, &PRESSES_MOVE[PRESS_INTERACT]);
    c(ACT_INTERACT, &INTERACT_HOLDS);
    c(ACT_WEAPON_NEXT, &PRESSES_WEAPON[0]);
    c(ACT_WEAPON_PREV, &PRESSES_WEAPON[1]);
    c(ACT_JUMP, &PRESSES_MOVE[PRESS_JUMP]);
    c(ACT_STANCE, &PRESSES_MOVE[PRESS_STANCE_HOLD]);
    c(ACT_SPRINT, &PRESSES_MOVE[PRESS_SPRINT]);
    c(ACT_AC130, &AC_PRESSES[AC_KEY_CALL]);
    c(ACT_NUKE, &PRESSES_NUKE);
    c(ACT_GS_NEXT, &AC_PRESSES[AC_KEY_NEXT]);
    c(ACT_GS_THERMAL, &AC_PRESSES[AC_KEY_THERMAL]);
    c(ACT_GS_EXIT, &AC_PRESSES[AC_KEY_EXIT]);
    c(ACT_HEAL, &HEAL_PULSES);
}

/// The body of the detour: atomics and the caller's 16-byte state only.
///
/// # Safety
/// `st` is the caller's XINPUT_STATE (checked for null) after a call of the original.
unsafe fn process(idx: u32, ret: u32, st: *mut XState) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    if !ENABLED.load(Ordering::Relaxed) || idx > 3 {
        return;
    }
    let cfg_slot = PAD_INDEX.load(Ordering::Relaxed);
    if ret != 0 {
        // that slot is not connected: a lost selected pad is reselected later
        if cfg_slot < 0 && SELECTED.load(Ordering::Relaxed) == idx as i32 {
            SELECTED.store(-1, Ordering::Relaxed);
        }
        return;
    }
    if st.is_null() {
        return;
    }
    SEEN_SLOTS.fetch_or(1 << idx, Ordering::Relaxed);
    // SAFETY: XInputGetState succeeded, so the caller's 16-byte XINPUT_STATE is filled.
    let (packet, bytes) = unsafe { ((*st).packet, (*st).pad) };
    let raw = Raw::from_bytes(&bytes);
    let sel = if cfg_slot >= 0 {
        cfg_slot
    } else {
        let s = SELECTED.load(Ordering::Relaxed);
        if s >= 0 {
            s
        } else if raw.active() {
            SELECTED.store(idx as i32, Ordering::Relaxed);
            idx as i32
        } else {
            return;
        }
    };
    if sel != idx as i32 {
        return;
    }
    // review C3: one reader at a time; a concurrent call gets the last read's hiding without stepping
    let o = match Busy::try_take(&STEP_BUSY) {
        Some(_busy) => step_selected(raw, packet),
        None => {
            CONTENDED.fetch_add(1, Ordering::Relaxed);
            contended_read(raw)
        }
    };
    if o != raw {
        BLANKED_READS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's buffer, filled by the original call above.
        unsafe { (*st).pad = o.to_bytes() };
    }
}

/// Review C3: a contended read: the last processed read's hidden / synthesized buttons and stick blanks, no step.
fn contended_read(raw: Raw) -> Raw {
    let mut o = raw;
    apply_buttons(&mut o, &StepOut { hide: LAST_HIDE.load(Ordering::Relaxed), synth: LAST_SYNTH.load(Ordering::Relaxed), ..StepOut::default() });
    let sticks = LAST_STICKS.load(Ordering::Relaxed);
    if sticks & 1 != 0 {
        (o.lx, o.ly) = (0, 0);
    }
    if sticks & 2 != 0 {
        (o.rx, o.ry) = (0, 0);
    }
    o
}

/// The selected pad's read (under STEP_BUSY): the step, the presses, the look; returns the edited gamepad.
fn step_selected(raw: Raw, packet: u32) -> Raw {
    let now = now_ms_if_started();
    let now_us = now_us_if_started();
    LAST_OK_MS.store(now, Ordering::Relaxed);
    PKT.store(packet, Ordering::Relaxed);
    RAW_LS.store(pack(raw.lx, raw.ly), Ordering::Relaxed);
    RAW_RS.store(pack(raw.rx, raw.ry), Ordering::Relaxed);
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    let gunship = effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, hb);
    let mw2 = !gunship && effective_want(WANT_PAD.load(Ordering::Relaxed), now, hb);
    let binds = load_binds();
    // C6: a chord request older than CHORD_STALE_MS is consumed silently (the task timed it out and moved on)
    let chord_req = CHORD_REQ.load(Ordering::SeqCst);
    if chord_req != ST_CHORD_SEQ.load(Ordering::Relaxed) && now.saturating_sub(CHORD_REQ_MS.load(Ordering::SeqCst)) > CHORD_STALE_MS {
        ST_CHORD_SEQ.store(chord_req, Ordering::Relaxed);
    }
    let mut s = StepState {
        prev: ST_PREV.load(Ordering::Relaxed),
        latched: ST_LATCHED.load(Ordering::Relaxed),
        chord_hide: ST_CHORD.load(Ordering::Relaxed),
        hold_t0: ST_HOLD_T0.load(Ordering::Relaxed),
        hold_fired: ST_HOLD_FIRED.load(Ordering::Relaxed),
        melee_t0: ST_MELEE_T0.load(Ordering::Relaxed),
        heal_t0: ST_HEAL_T0.load(Ordering::Relaxed),
        interact_t0: ST_INTERACT_T0.load(Ordering::Relaxed),
        chord_t0: ST_CHORD_T0.load(Ordering::Relaxed),
        chord_seq: ST_CHORD_SEQ.load(Ordering::Relaxed),
    };
    let out = step(
        &mut s,
        &binds,
        &StepIn {
            levels: raw.levels(TRIG_THR.load(Ordering::Relaxed)),
            levels_lo: raw.levels(TRIGGER_RELEASE),
            now_ms: now,
            mw2,
            gunship,
            hold_ms: HOLD_MS.load(Ordering::Relaxed),
            blank_toggle: BLANK_TOGGLE_KEY.load(Ordering::Relaxed),
            er_prompt: effective_want(WANT_PROMPT.load(Ordering::Relaxed), now, hb),
            chord_req,
            melee_off: effective_want(WANT_MELEE_OFF.load(Ordering::Relaxed) || WANT_MELEE_OFF_CARRIER.load(Ordering::Relaxed), now, hb),
            er_aim: effective_want(WANT_ER_AIM.load(Ordering::Relaxed), now, hb),
            // 2026-10-24: the task's death gate (no melee / heal pulse while dead, until alive after the respawn)
            dead: effective_want(crate::shared::WANT_DEAD_BLOCK.load(Ordering::Relaxed), now, hb),
        },
    );
    ST_PREV.store(s.prev, Ordering::Relaxed);
    ST_LATCHED.store(s.latched, Ordering::Relaxed);
    ST_CHORD.store(s.chord_hide, Ordering::Relaxed);
    ST_HOLD_T0.store(s.hold_t0, Ordering::Relaxed);
    ST_HOLD_FIRED.store(s.hold_fired, Ordering::Relaxed);
    ST_MELEE_T0.store(s.melee_t0, Ordering::Relaxed);
    ST_HEAL_T0.store(s.heal_t0, Ordering::Relaxed);
    ST_INTERACT_T0.store(s.interact_t0, Ordering::Relaxed);
    ST_CHORD_T0.store(s.chord_t0, Ordering::Relaxed);
    ST_CHORD_SEQ.store(s.chord_seq, Ordering::Relaxed);
    if out.chord != pad_logic::ChordEv::None {
        CHORD_EV.store((s.chord_seq & 0xffff) << 16 | out.chord.code() << 8 | pad_logic::chord_reason_code(out.chord.reason()), Ordering::Relaxed);
    }
    if out.melee_suppressed {
        MELEE_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
    }
    LAST_HIDE.store(out.hide, Ordering::Relaxed);
    LAST_SYNTH.store(out.synth, Ordering::Relaxed);
    HELD.store(out.held, Ordering::Relaxed);
    bump(out.presses);
    if out.prompt_tap {
        INTERACT_TAPS.fetch_add(1, Ordering::Relaxed);
    }

    let mut o = raw;
    apply_buttons(&mut o, &out);
    let lp = load_look();
    let route_mouse = ROUTE_MOUSE.load(Ordering::Relaxed);
    let mouse_fresh = fresh(now, LAST_GDS_MOUSE_MS.load(Ordering::Relaxed), MOUSE_FRESH_MS);
    let look_mouse = mw2 && route_mouse && mouse_fresh;
    LOOK_VIA_MOUSE.store(look_mouse, Ordering::Relaxed);
    if gunship {
        // the AC-130 aims with the stick through the gunship camera's mouse counters (MW2 rates, hip)
        let prev = LAST_GS_US.swap(now_us, Ordering::Relaxed);
        let dt = if prev == 0 { 0.0 } else { (now_us.saturating_sub(prev) as f32 * 1e-6).min(0.05) };
        let mut ls = LookState { extra: f(&GS_EXTRA_BITS) };
        let (yaw, pitch) = look_rates(&mut ls, &lp, raw.rx, raw.ry, 0.0, dt);
        GS_EXTRA_BITS.store(ls.extra.to_bits(), Ordering::Relaxed);
        let cpd = f(&GS_COUNTS_PER_DEG_BITS);
        if cpd > 0.0 {
            let mut carry = [GS_CARRY[0].load(Ordering::Relaxed), GS_CARRY[1].load(Ordering::Relaxed)];
            let (dx, dy) = look_counts(yaw, pitch, dt, 1.0 / cpd, &mut carry);
            GS_CARRY[0].store(carry[0], Ordering::Relaxed);
            GS_CARRY[1].store(carry[1], Ordering::Relaxed);
            MOUSE_DX.fetch_add(dx, Ordering::Relaxed);
            MOUSE_DY.fetch_add(dy, Ordering::Relaxed);
        }
        (o.lx, o.ly, o.rx, o.ry) = (0, 0, 0, 0);
    } else {
        LAST_GS_US.store(0, Ordering::Relaxed);
        if mw2 {
            if look_mouse {
                if binds.blank_rs {
                    (o.rx, o.ry) = (0, 0);
                }
            } else {
                // route er, or the mouse is not polled (fallback): ER keeps the stick, reshaped by the MW2 graph
                if route_mouse && !mouse_fresh && (raw.rx != 0 || raw.ry != 0) {
                    LOOK_FALLBACK_READS.fetch_add(1, Ordering::Relaxed);
                }
                (o.rx, o.ry) = reshape_for_er(raw.rx, raw.ry, &lp);
            }
        }
        if binds.blank_ls && effective_want(WANT_BLANK_LS.load(Ordering::Relaxed), now, hb) {
            (o.lx, o.ly) = (0, 0);
        }
    }
    let ls_zero = (raw.lx != 0 || raw.ly != 0) && (o.lx, o.ly) == (0, 0);
    let rs_zero = (raw.rx != 0 || raw.ry != 0) && (o.rx, o.ry) == (0, 0);
    LAST_STICKS.store(ls_zero as u32 | (rs_zero as u32) << 1, Ordering::Relaxed);
    o
}

unsafe extern "system" fn detour(idx: u32, st: *mut XState) -> u32 {
    let orig = TRAMPOLINE.load(Ordering::Acquire);
    if orig == 0 {
        return ERROR_DEVICE_NOT_CONNECTED; // cannot happen once the hook is enabled
    }
    // SAFETY: orig is the MinHook trampoline of XInputGetState (same signature).
    let orig: GetStateFn = unsafe { std::mem::transmute(orig) };
    // SAFETY: forwarding the caller's own arguments.
    let ret = unsafe { orig(idx, st) };
    // Defensive: the body has no panicking calls; an unwind must not cross this frame.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: after the original call; `process` checks ret / null.
        unsafe { process(idx, ret, st) }
    }));
    ret
}

// ---- the mouse look route (called by input_hook's mouse branch) --------------------------------------------------------

/// Adds the right stick's MW2 look to ER's mouse read: called AFTER the ADS sensitivity scaling of the user's own
/// counts (so ADS is never scaled twice), only while the stick's look goes through the mouse (MW2 mode drives, route
/// mouse, the pad fresh). Also sums the counts delivered on lX for the calibration.
///
/// # Safety
/// `data` must point to the game's DIMOUSESTATE(2) buffer (16 or 20 bytes) of a successful GetDeviceState.
pub unsafe fn mouse_look(data: *mut u8) {
    // review C3: serialized like `process` (the look carry / accel state is read-modify-write)
    let Some(_busy) = Busy::try_take(&LOOK_BUSY) else {
        CONTENDED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let now = now_ms_if_started();
    let now_us = now_us_if_started();
    let prev = LAST_LOOK_US.swap(now_us, Ordering::Relaxed);
    let xp = data as *mut i32;
    // SAFETY: lX / lY are the i32s at bytes 0..8 of the caller's buffer.
    let (lx, ly) = unsafe { (xp.read_unaligned(), xp.add(1).read_unaligned()) };
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    let active = ENABLED.load(Ordering::Relaxed)
        && LOOK_VIA_MOUSE.load(Ordering::Relaxed)
        && effective_want(WANT_PAD.load(Ordering::Relaxed), now, hb)
        && fresh(now, LAST_OK_MS.load(Ordering::Relaxed), MOUSE_FRESH_MS);
    let mut dx = 0;
    if active {
        let dt = if prev == 0 { 0.0 } else { (now_us.saturating_sub(prev) as f32 * 1e-6).min(0.05) };
        let lp = load_look();
        let (rx, ry) = unpack(RAW_RS.load(Ordering::Relaxed));
        let mut ls = LookState { extra: f(&LOOK_EXTRA_BITS) };
        let (yaw, pitch) = look_rates(&mut ls, &lp, rx, ry, f(&ADS_FRAC_BITS), dt);
        LOOK_EXTRA_BITS.store(ls.extra.to_bits(), Ordering::Relaxed);
        let mut carry = [LOOK_CARRY[0].load(Ordering::Relaxed), LOOK_CARRY[1].load(Ordering::Relaxed)];
        let (cx, cy) = look_counts(yaw, pitch, dt, f(&DEG_PER_COUNT_BITS), &mut carry);
        LOOK_CARRY[0].store(carry[0], Ordering::Relaxed);
        LOOK_CARRY[1].store(carry[1], Ordering::Relaxed);
        if cx != 0 || cy != 0 {
            dx = cx;
            // SAFETY: as above.
            unsafe {
                xp.write_unaligned(lx.saturating_add(cx));
                xp.add(1).write_unaligned(ly.saturating_add(cy));
            }
            LOOK_COUNTS.fetch_add((cx.unsigned_abs() + cy.unsigned_abs()) as u64, Ordering::Relaxed);
        }
    } else {
        LOOK_EXTRA_BITS.store(0, Ordering::Relaxed);
    }
    DELIVERED_DX.fetch_add(lx as i64 + dx as i64, Ordering::Relaxed);
}

// ---- install ----------------------------------------------------------------------------------------------------------

/// Installs the detour on hudhook's MinHook. Returns the `PAD hook ok ...` details.
pub fn install() -> Result<String, String> {
    // SAFETY: MH_Initialize is idempotent (ALREADY_INITIALIZED).
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        s => return Err(format!("MH_Initialize: {s:?}")),
    }
    // SAFETY: by name; ER imports xinput1_4.dll, so this normally only finds it.
    let module = match unsafe { GetModuleHandleW(w!("xinput1_4.dll")) } {
        Ok(m) => m,
        // SAFETY: loading a system DLL by name (the game loads it anyway).
        Err(_) => unsafe { LoadLibraryW(w!("xinput1_4.dll")) }.map_err(|e| format!("LoadLibraryW(xinput1_4): {e}"))?,
    };
    // SAFETY: a valid module handle and a static export name.
    let target = unsafe { GetProcAddress(module, s!("XInputGetState")) }.ok_or("GetProcAddress(XInputGetState) failed")? as usize;
    // SAFETY: reading the first byte of an exported function of a loaded module (code pages are readable).
    let first = unsafe { *(target as *const u8) };
    // SAFETY: by name only, no load.
    let overlay = unsafe { GetModuleHandleW(w!("GameOverlayRenderer64.dll")) }.is_ok();
    // SAFETY: target is XInputGetState; the detour has the same signature.
    let hook = unsafe { MhHook::new(target as *mut c_void, detour as *mut c_void) }.map_err(|s| format!("MH_CreateHook 0x{target:x}: {s:?}"))?;
    TRAMPOLINE.store(hook.trampoline() as usize, Ordering::Release);
    // SAFETY: the hook was created above; the trampoline is stored before enabling.
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => {}
        s => return Err(format!("MH_EnableHook 0x{target:x}: {s:?}")),
    }
    std::mem::forget(hook); // MhHook has no Drop; the hook lives for the process lifetime (the DLL has no unload)
    HOOKED.store(true, Ordering::SeqCst);
    Ok(format!("xinput1_4=0x{target:x} first_byte=0x{first:02X} overlay=GameOverlayRenderer64:{}", if overlay { "yes" } else { "no" }))
}

static HOOKED: AtomicBool = AtomicBool::new(false);

// ---- the task half ----------------------------------------------------------------------------------------------------

pub struct PadIn<'a> {
    pub now: f64,
    pub cfg: &'a Config,
    pub cfg_gen: u64,
    /// MW2 mode drives (`blank`) and no AC-130 ride.
    pub want: bool,
    pub ads_frac: f32,
    pub in_world: bool,
    /// ER's camera forward this tick (for the calibration), the gunship flag.
    pub cam_fwd: Option<V3>,
    pub gunship: bool,
    /// 2026-10-21: ER's action prompt is visible (a `[gamepad] prompt_indices` index in `ui_states`).
    pub er_prompt: bool,
}

pub struct PadTask {
    cfg_gen: Option<u64>,
    next_stat: f64,
    boot_t: Option<f64>,
    none_logged: bool,
    connected_logged: bool,
    prev_fallback: u64,
    fallback_t: f64,
    prev_holds: u32,
    prev_taps: u32,
    prev_prompt: bool,
    prev_yaw_fwd: Option<(f32, f32)>,
    prev_dx: i64,
    calib: Calib,
    calib_logged: Option<f32>,
    calib_log_t: f64,
}

impl PadTask {
    pub fn new() -> PadTask {
        PadTask {
            cfg_gen: None,
            next_stat: 0.0,
            boot_t: None,
            none_logged: false,
            connected_logged: false,
            prev_fallback: LOOK_FALLBACK_READS.load(Ordering::Relaxed),
            fallback_t: f64::NEG_INFINITY,
            prev_holds: INTERACT_HOLDS.load(Ordering::Relaxed),
            prev_taps: INTERACT_TAPS.load(Ordering::Relaxed),
            prev_prompt: false,
            prev_yaw_fwd: None,
            prev_dx: DELIVERED_DX.load(Ordering::Relaxed),
            calib: Calib::default(),
            calib_logged: None,
            calib_log_t: f64::NEG_INFINITY,
        }
    }

    pub fn tick(&mut self, i: &PadIn) {
        let g = &i.cfg.gamepad;
        WANT_PAD.store(i.want && g.enabled, Ordering::Relaxed);
        WANT_PROMPT.store(i.er_prompt, Ordering::Relaxed);
        ADS_FRAC_BITS.store(i.ads_frac.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        if self.cfg_gen != Some(i.cfg_gen) {
            self.cfg_gen = Some(i.cfg_gen);
            let l = g.look();
            let dpc = if g.mouse_deg_per_count > 0.0 { format!("{:.4}", g.mouse_deg_per_count) } else { "auto".into() };
            let b = g.binds().unwrap_or_default();
            log_i!(
                "PAD",
                "cfg enabled={} hooked={} route={} graph={} rates={}/{}/{}/{} sens={:.2} accel={}({}/{}) dz={:.2}/{:.2} trig={} hold_ms={} deg_per_count={dpc} interact_synth={} prompt_idx={:?} melee_synth={} heal={}->{} toggle={} fp={} blank_rs={} blank_ls={} rumble=0",
                g.enabled as u8,
                HOOKED.load(Ordering::Relaxed) as u8,
                g.look_route,
                l.graph,
                l.yaw,
                l.pitch,
                l.yaw_ads,
                l.pitch_ads,
                l.sens,
                l.accel as u8,
                l.accel_lerp,
                l.accel_max_yaw,
                g.rs_deadzone,
                g.ls_deadzone,
                g.trigger_threshold,
                g.hold_ms,
                pad_logic::mask_name(b.er_interact),
                g.prompt_indices,
                pad_logic::mask_name(b.er_melee),
                pad_logic::mask_name(b.masks[ACT_HEAL]),
                pad_logic::mask_name(b.er_use_item),
                pad_logic::mask_name(b.masks[ACT_TOGGLE]),
                pad_logic::mask_name(b.masks[ACT_FP]),
                b.blank_rs as u8,
                b.blank_ls as u8
            );
        }
        if !HOOKED.load(Ordering::Relaxed) || !g.enabled {
            return;
        }
        let boot_t = *self.boot_t.get_or_insert(i.now);
        let seen = SEEN_SLOTS.load(Ordering::Relaxed);
        if seen != 0 && !self.connected_logged {
            self.connected_logged = true;
            log_i!("PAD", "connected slots=0x{seen:x} selected={}", SELECTED.load(Ordering::Relaxed));
        }
        if seen == 0 && !self.none_logged && i.now - boot_t >= 60.0 {
            self.none_logged = true;
            log_w!(
                "PAD",
                "none seen in 60s (enable Steam Input for PlayStation pads) calls={}",
                CALLS.load(Ordering::Relaxed)
            );
        }
        // the interact hold (X held): a line per hold, so the analyzer can attribute `MOVE suspend reason=interact`;
        // 2026-10-21: a press at ER's prompt is its own `interact tap` line (X interacted at once, no reload)
        let holds = INTERACT_HOLDS.load(Ordering::Relaxed);
        let taps = INTERACT_TAPS.load(Ordering::Relaxed);
        if holds != self.prev_holds {
            let n_tap = taps.wrapping_sub(self.prev_taps);
            let n_hold = holds.wrapping_sub(self.prev_holds).saturating_sub(n_tap);
            let synth = pad_logic::mask_name(ER_INTERACT.load(Ordering::Relaxed));
            if n_tap > 0 {
                log_i!("PAD", "interact tap n={n_tap} synth={synth} prompt=1 pulse_ms={}", pad_logic::INTERACT_PULSE_MS);
            }
            if n_hold > 0 {
                log_i!("PAD", "interact hold n={n_hold} synth={synth} prompt={}", i.er_prompt as u8);
            }
            self.prev_holds = holds;
        }
        self.prev_taps = taps;
        // the prompt signal's transitions while MW2 mode drives (proves the X-at-prompt rule had its signal)
        let prompt = i.er_prompt && i.want;
        if prompt != self.prev_prompt {
            self.prev_prompt = prompt;
            log_i!("PAD", "er_prompt {} (X {})", if prompt { "0->1" } else { "1->0" }, if prompt { "= ER interact" } else { "tap = reload, hold = interact" });
        }
        let fb = LOOK_FALLBACK_READS.load(Ordering::Relaxed);
        if fb != self.prev_fallback {
            if i.now - self.fallback_t >= 30.0 {
                self.fallback_t = i.now;
                log_w!("PAD", "look route fallback=er why=mouse_stale reads={} (ER did not poll the mouse within {MOUSE_FRESH_MS} ms)", fb - self.prev_fallback);
            }
            self.prev_fallback = fb;
        }
        self.calibrate(i);
        if i.now >= self.next_stat {
            self.next_stat = i.now + g.log_every_s as f64;
            let sel = SELECTED.load(Ordering::Relaxed);
            let slot = if g.pad_index >= 0 { g.pad_index } else { sel };
            log_i!(
                "PAD",
                "stat calls={} slot={slot} connected={} pkt={} blanked={} look_counts={} deg_per_count={:.4} src={} drive={} via_mouse={} contended={}",
                CALLS.load(Ordering::Relaxed),
                pad_fresh() as u8,
                PKT.load(Ordering::Relaxed),
                BLANKED_READS.load(Ordering::Relaxed),
                LOOK_COUNTS.load(Ordering::Relaxed),
                f(&DEG_PER_COUNT_BITS),
                if g.mouse_deg_per_count > 0.0 { "cfg" } else if self.calib_logged.is_some() { "calib" } else { "provisional" },
                WANT_PAD.load(Ordering::Relaxed) as u8,
                LOOK_VIA_MOUSE.load(Ordering::Relaxed) as u8,
                contended()
            );
        }
    }

    /// ER's degrees per mouse count: |camera yaw change| / |counts delivered on lX| over ticks with enough counts
    /// (the user's mouse or the stick), running median [I: ER turns linearly with counts].
    fn calibrate(&mut self, i: &PadIn) {
        let total = DELIVERED_DX.load(Ordering::Relaxed);
        let counts = total - self.prev_dx;
        self.prev_dx = total;
        let fwd = i.cam_fwd.filter(|_| i.in_world && !i.gunship).and_then(|v| {
            let h = (v.x * v.x + v.z * v.z).sqrt();
            (h > 0.2).then_some((v.x / h, v.z / h))
        });
        if let (Some(prev), Some(now)) = (self.prev_yaw_fwd, fwd) {
            self.calib.add(counts, pad_logic::yaw_delta_deg(prev, now));
        }
        self.prev_yaw_fwd = fwd;
        if i.cfg.gamepad.mouse_deg_per_count > 0.0 {
            return;
        }
        let Some(est) = self.calib.estimate() else {
            if self.calib_logged.is_none() {
                DEG_PER_COUNT_BITS.store(PROVISIONAL_DEG_PER_COUNT.to_bits(), Ordering::Relaxed);
            }
            return;
        };
        DEG_PER_COUNT_BITS.store(est.to_bits(), Ordering::Relaxed);
        let changed = self.calib_logged.is_none_or(|l| (est - l).abs() > 0.15 * l);
        if changed && i.now - self.calib_log_t >= 20.0 {
            self.calib_log_t = i.now;
            self.calib_logged = Some(est);
            log_i!("PAD", "calib deg_per_count={est:.4} n={} (ER camera yaw / counts delivered, median)", self.calib.n());
        }
    }
}
