//! DirectInput `GetDeviceState` hook (docs/M2_DESIGN.md section 9).
//!
//! Elden Ring reads mouse and keyboard only through `IDirectInputDevice8::GetDeviceState` on this
//! build (CS2/Sunless notes). The detour lets the game read the real state, records the raw
//! buttons for the task, and zeroes ONLY the fire-button byte and our own key bytes while the task
//! asks for it (MW2 mode on, in world, no menu, focused) AND the task's heartbeat is fresh (it ticked
//! in the last 250 ms), with a release latch. Any other buffer size (gamepads) is never touched.
//!
//! M4 (docs/M4_DESIGN.md section 5): while the AC-130 ride runs (`WANT_GUNSHIP_INPUT`, same heartbeat rule) the mouse
//! axes are accumulated for the gunship camera and zeroed with every mouse button, and the keyboard reads all keys
//! released except Esc. The AC-130 key edges (F7, exit, next, 1/2/3, thermal) are recorded before that, always.
//!
//! 2026-10-21: the one key byte ever SET is ER's use-item key, for `HEAL_PULSE_MS` after a heal press (Q) while MW2
//! mode drives (the flask; R is otherwise our reload and hidden).
//!
//! C6 (docs/C6_HOLD_DESIGN.md 3.4): with no pad connected, the crossbow stance's two-hand chord is ER's E held for
//! `CHORD_HOLD_MS` with its LMB down inside it (`KB_CHORD_T0`, set by the task; [I4]), written after every blank.
//! C6 aim 4: and ER's aim (guard) button, RMB, held while the stance's third-person ADS episode asks (`KB_AIM`).
//!
//! The detour runs on the game's input thread: no allocation, no locks, no logging, no panics.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use hudhook::mh::{MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::input_logic::{
    DIK_ESCAPE, HealKey, KEYBOARD_STATE_SIZE, capture_keyboard, capture_mouse, effective_want, heal_key_step, latch_next,
    mouse_button_offset, press_edge,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    DIRECTINPUT_VERSION, DirectInput8Create, GUID_SysKeyboard, GUID_SysMouse, IDirectInput8A, IDirectInput8W,
    IDirectInputDevice8A, IDirectInputDevice8W,
};
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::core::{GUID, HRESULT, Interface, w};

use crate::shared::{
    ADS_BUTTON, ADS_LATCH, RAW_ADS_DOWN, WANT_BLANK_ADS, BLANK_TOGGLE_KEY, FIRE_BUTTON, FIRE_LATCH, LAST_GDS_KBD_MS, LAST_GDS_MOUSE_MS, PRESSES_RELOAD, PRESSES_TOGGLE,
    RAW_FIRE_DOWN, RAW_RELOAD_DOWN, RAW_TOGGLE_DOWN, RELOAD_DIK, RELOAD_LATCH, TASK_HEARTBEAT_MS, TOGGLE_DIK,
    WANT_BLANK_FIRE, WANT_BLANK_RELOAD, now_ms_if_started,
};
use crate::shared::{FP_TOGGLE_DIK, MOVE_A, MOVE_D, MOVE_S, MOVE_W, PRESSES_FP, RAW_FP_DOWN, RAW_MOVE_KEYS};
use crate::shared::{AC_DIK, AC_KEY_CALL, AC_KEYS, AC_PRESSES, AC_RAW_DOWN, ESC_PRESSES, ESC_RAW_DOWN, MOUSE_COUNTS, MOUSE_DX, MOUSE_DY, WANT_GUNSHIP_INPUT, WHEEL};
use crate::shared::{
    MOVE_BLANK_LATCH, MOVE_BLANK_PLAIN, MOVE_BLANKED_READS, MOVE_LATCHED, MOVE_PRESS_DIK, MOVE_PRESS_DOWN, MOVE_WASD_DIK,
    PRESSES_MOVE, PROBE_DOWN, PROBE_PRESSES, RAW_KEYS,
};
use mw2er_core::move_input::{PROBE_KEYS, key_blank};
use crate::shared::{ARSENAL_WHEEL, PRESSES_WEAPON, SENS_CARRY, SENS_SCALE_BITS, WANT_BLANK_WEAPON, WANT_WHEEL, WEAPON_DIK, WEAPON_LATCH, WEAPON_RAW_DOWN};

/// M6: mouse counts scaled by `s` with the residual carried in 1/1024 counts (no allocation, no float state).
fn scale_counts(v: i32, s: f32, carry: i32) -> (i32, i32) {
    let total = v as i64 * (s * 1024.0) as i64 + carry as i64;
    let out = total.div_euclid(1024);
    let rest = total - out * 1024;
    (out.clamp(i32::MIN as i64, i32::MAX as i64) as i32, rest as i32)
}

/// M5 (docs/M5_DESIGN.md section 6.3): the movement keys' levels and press edges, recorded BEFORE any blanking of this
/// read (a tap shorter than a task tick is never lost: the task consumes counter differences).
///
/// # Safety
/// `data` must point to the game's 256-byte keyboard buffer.
unsafe fn record_move_keys(data: *const u8) {
    let mut levels = 0u32;
    for i in 0..PRESSES_MOVE.len() {
        let k = MOVE_PRESS_DIK[i].load(Ordering::Relaxed) as usize;
        if k == 0 {
            continue;
        }
        // SAFETY: k < 256, inside the buffer.
        let down = unsafe { *data.add(k) } & 0x80 != 0;
        if down {
            levels |= 1 << i;
        }
        if press_edge(MOVE_PRESS_DOWN[i].swap(down, Ordering::Relaxed), down) {
            PRESSES_MOVE[i].fetch_add(1, Ordering::Relaxed);
        }
    }
    RAW_KEYS.store(levels, Ordering::Relaxed);
    for (i, (_, k)) in PROBE_KEYS.iter().enumerate() {
        // SAFETY: k < 256, inside the buffer.
        let down = unsafe { *data.add(*k as usize) } & 0x80 != 0;
        if press_edge(PROBE_DOWN[i].swap(down, Ordering::Relaxed), down) {
            PROBE_PRESSES[i].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// M5: hides the movement keys the task wants hidden: `MOVE_BLANK_LATCH` keys stay hidden until physically released
/// (ER dodges on the release of a short Space press), `MOVE_BLANK_PLAIN` keys (W/A/S/D in first person) only while
/// wanted. Both only while the task's heartbeat is fresh.
///
/// # Safety
/// `data` must point to the game's 256-byte keyboard buffer.
unsafe fn blank_move_keys(data: *mut u8, now: u64, heartbeat: u64) {
    let fresh = effective_want(true, now, heartbeat);
    let mut any = false;
    for w in 0..4 {
        let want_l = if fresh { MOVE_BLANK_LATCH[w].load(Ordering::Relaxed) } else { 0 };
        let want_p = if fresh { MOVE_BLANK_PLAIN[w].load(Ordering::Relaxed) } else { 0 };
        let mut latched = MOVE_LATCHED[w].load(Ordering::Relaxed);
        let mut bits = want_l | want_p | latched;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let k = w * 64 + b;
            // SAFETY: k < 256, inside the buffer.
            let p = unsafe { data.add(k) };
            // SAFETY: inside the buffer.
            let down = unsafe { *p } & 0x80 != 0;
            let (blank, l) = key_blank(latched >> b & 1 == 1, want_l >> b & 1 == 1, want_p >> b & 1 == 1, down);
            if l {
                latched |= 1 << b;
            } else {
                latched &= !(1 << b);
            }
            if blank {
                // SAFETY: inside the buffer.
                unsafe { *p = 0 };
                any = true;
            }
        }
        MOVE_LATCHED[w].store(latched, Ordering::Relaxed);
    }
    if any {
        MOVE_BLANKED_READS.fetch_add(1, Ordering::Relaxed);
    }
}

type GetDeviceStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> HRESULT;

const SLOTS: usize = 4;
static TRAMPOLINES: [AtomicUsize; SLOTS] =
    [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

const E_FAIL: HRESULT = HRESULT(0x8000_4005u32 as i32);

/// Post-processing of a successful GetDeviceState call. Only atomics and the caller's buffer.
///
/// # Safety
/// `data` must point to `cb` writable bytes (the buffer the game passed to GetDeviceState).
unsafe fn process_state(cb: u32, data: *mut u8) {
    let now = now_ms_if_started();
    // The task's wish counts only while its heartbeat is fresh (it may stop ticking on the title
    // screen or in loading screens); the release latch still covers a held button.
    let heartbeat = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    if let Some(off) = mouse_button_offset(cb, FIRE_BUTTON.load(Ordering::Relaxed)) {
        LAST_GDS_MOUSE_MS.store(now, Ordering::Relaxed);
        // SAFETY: off < cb (mouse_button_offset only returns rgbButtons indices inside the struct).
        let p = unsafe { data.add(off) };
        // SAFETY: p is inside the caller's buffer.
        let down = unsafe { *p } & 0x80 != 0;
        RAW_FIRE_DOWN.store(down, Ordering::Relaxed);
        let want = effective_want(WANT_BLANK_FIRE.load(Ordering::Relaxed), now, heartbeat);
        let blank = latch_next(FIRE_LATCH.load(Ordering::Relaxed), want, down);
        FIRE_LATCH.store(blank, Ordering::Relaxed);
        if blank {
            // SAFETY: as above.
            unsafe { *p = 0 };
        }
        // M3: the ADS button (RMB = guard/off-hand in ER) under the same rules and its own release latch.
        if let Some(aoff) = mouse_button_offset(cb, ADS_BUTTON.load(Ordering::Relaxed)) {
            // SAFETY: aoff < cb, as for the fire button.
            let ap = unsafe { data.add(aoff) };
            // SAFETY: ap is inside the caller's buffer.
            let adown = unsafe { *ap } & 0x80 != 0;
            RAW_ADS_DOWN.store(adown, Ordering::Relaxed);
            let awant = effective_want(WANT_BLANK_ADS.load(Ordering::Relaxed), now, heartbeat);
            let ablank = latch_next(ADS_LATCH.load(Ordering::Relaxed), awant, adown);
            ADS_LATCH.store(ablank, Ordering::Relaxed);
            if ablank {
                // SAFETY: as above.
                unsafe { *ap = 0 };
            }
        }
        // M6: the wheel switches guns (lZ counted, then hidden from ER) and aiming scales the mouse look (residual
        // carry, so slow movements are not lost); both only outside the gunship, with the heartbeat rule.
        let gunship = effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, heartbeat);
        if !gunship && (cb == 16 || cb == 20) {
            if effective_want(WANT_WHEEL.load(Ordering::Relaxed), now, heartbeat) {
                // SAFETY: DIMOUSESTATE(2): lZ is the i32 at bytes 8..12, inside the caller's buffer.
                let zp = unsafe { data.add(8) } as *mut i32;
                // SAFETY: as above (unaligned access through read/write_unaligned).
                let dz = unsafe { zp.read_unaligned() };
                if dz != 0 {
                    ARSENAL_WHEEL.fetch_add(dz, Ordering::Relaxed);
                    // SAFETY: as above.
                    unsafe { zp.write_unaligned(0) };
                }
            }
            let s = f32::from_bits(SENS_SCALE_BITS.load(Ordering::Relaxed));
            if s.is_finite() && s > 0.0 && s < 0.999 && effective_want(true, now, heartbeat) {
                for axis in 0..2 {
                    // SAFETY: lX / lY are the i32s at bytes 0..8, inside the caller's buffer.
                    let ap = unsafe { data.add(axis * 4) } as *mut i32;
                    // SAFETY: as above.
                    let v = unsafe { ap.read_unaligned() };
                    let (out, carry) = scale_counts(v, s, SENS_CARRY[axis].load(Ordering::Relaxed));
                    SENS_CARRY[axis].store(carry, Ordering::Relaxed);
                    // SAFETY: as above.
                    unsafe { ap.write_unaligned(out) };
                }
            }
            // C3 PASS B (docs/C3_DESIGN.md 4.4): the pad's right stick turns ER's camera through this mouse read, added
            // AFTER the ADS scaling above (ADS is not scaled twice); inert unless MW2 mode drives with route mouse
            // SAFETY: GetDeviceState succeeded with a 16 / 20-byte DIMOUSESTATE(2) (cb checked above).
            unsafe { crate::pad::mouse_look(data) };
        }
        // C6: the keyboard chord's LMB (only inside its E hold, never in the gunship; the raw fire state was recorded above,
        // so the MW2 gun never sees it)
        let kc = crate::shared::KB_CHORD_T0.load(Ordering::Relaxed);
        if kc != 0
            && !gunship
            && effective_want(true, now, heartbeat)
            && let Some(loff) = mouse_button_offset(cb, 0)
        {
            let (hold, tap) = mw2er_core::pad_logic::chord_bits(now.saturating_sub(kc));
            if hold && tap {
                // SAFETY: loff < cb (an rgbButtons index of this call's buffer).
                unsafe { *data.add(loff) = 0x80 };
            }
        }
        // C6 aim 4: without a pad, ER's aim (guard) button held while the stance's ADS episode asks (after the ADS
        // blank above, whose raw state was recorded first: MW2's ADS never sees it); never in the gunship
        if crate::shared::KB_AIM.load(Ordering::Relaxed)
            && !gunship
            && effective_want(true, now, heartbeat)
            && let Some(roff) = mouse_button_offset(cb, crate::shared::KB_AIM_BUTTON)
        {
            // SAFETY: roff < cb (an rgbButtons index of this call's buffer).
            unsafe { *data.add(roff) = 0x80 };
        }
        // M4 gunship capture (after the raw button states above were recorded): the axes go to the camera-step
        // detour / the task, then axes and every button read as zero for ER (no camera turn, no attack).
        if gunship {
            // SAFETY: GetDeviceState succeeded with `cb` bytes (16 or 20 here: mouse_button_offset matched).
            let buf = unsafe { std::slice::from_raw_parts_mut(data, cb as usize) };
            if let Some(((dx, dy, dz), _)) = capture_mouse(buf) {
                MOUSE_DX.fetch_add(dx, Ordering::Relaxed);
                MOUSE_DY.fetch_add(dy, Ordering::Relaxed);
                WHEEL.fetch_add(dz, Ordering::Relaxed);
                MOUSE_COUNTS.fetch_add((dx.unsigned_abs() + dy.unsigned_abs()) as u64, Ordering::Relaxed);
            }
        }
    } else if cb == KEYBOARD_STATE_SIZE {
        LAST_GDS_KBD_MS.store(now, Ordering::Relaxed);
        // M5: movement key levels and press counters first, before any key of this read is blanked.
        // SAFETY: the 256-byte keyboard buffer of this call.
        unsafe { record_move_keys(data) };
        // Toggle key: raw edge -> counter; hidden from the game if configured.
        let t = TOGGLE_DIK.load(Ordering::Relaxed) as usize;
        // SAFETY: t < 256 (u8) and the keyboard buffer is 256 bytes.
        let tp = unsafe { data.add(t) };
        // SAFETY: inside the buffer.
        let tdown = unsafe { *tp } & 0x80 != 0;
        if press_edge(RAW_TOGGLE_DOWN.swap(tdown, Ordering::Relaxed), tdown) {
            PRESSES_TOGGLE.fetch_add(1, Ordering::Relaxed);
        }
        if BLANK_TOGGLE_KEY.load(Ordering::Relaxed) {
            // SAFETY: inside the buffer.
            unsafe { *tp = 0 };
        }
        // M3b first-person key: raw edge -> counter; hidden like the toggle key.
        let f = FP_TOGGLE_DIK.load(Ordering::Relaxed) as usize;
        // SAFETY: f < 256.
        let fp = unsafe { data.add(f) };
        // SAFETY: inside the buffer.
        let fdown = unsafe { *fp } & 0x80 != 0;
        if press_edge(RAW_FP_DOWN.swap(fdown, Ordering::Relaxed), fdown) {
            PRESSES_FP.fetch_add(1, Ordering::Relaxed);
        }
        if BLANK_TOGGLE_KEY.load(Ordering::Relaxed) {
            // SAFETY: inside the buffer.
            unsafe { *fp = 0 };
        }
        // M3b: W/A/S/D for the yaw gate, read before any blanking (M5: the configured movement keys).
        let mut keys = 0u8;
        for (i, bit) in [MOVE_W, MOVE_A, MOVE_S, MOVE_D].into_iter().enumerate() {
            let dik = MOVE_WASD_DIK[i].load(Ordering::Relaxed) as usize;
            // SAFETY: dik < 256, inside the buffer.
            if unsafe { *data.add(dik) } & 0x80 != 0 {
                keys |= bit;
            }
        }
        RAW_MOVE_KEYS.store(keys, Ordering::Relaxed);
        // Reload key: raw edge -> counter; blanked through the release latch.
        let r = RELOAD_DIK.load(Ordering::Relaxed) as usize;
        // SAFETY: r < 256.
        let rp = unsafe { data.add(r) };
        // SAFETY: inside the buffer.
        let rdown = unsafe { *rp } & 0x80 != 0;
        if press_edge(RAW_RELOAD_DOWN.swap(rdown, Ordering::Relaxed), rdown) {
            PRESSES_RELOAD.fetch_add(1, Ordering::Relaxed);
        }
        let want = effective_want(WANT_BLANK_RELOAD.load(Ordering::Relaxed), now, heartbeat);
        let blank = latch_next(RELOAD_LATCH.load(Ordering::Relaxed), want, rdown);
        RELOAD_LATCH.store(blank, Ordering::Relaxed);
        if blank {
            // SAFETY: inside the buffer.
            unsafe { *rp = 0 };
        }
        // M4: raw edges of the AC-130 keys and of Esc are recorded first (the task uses them only when they apply),
        // BEFORE the M6 weapon-key blanking: the AC-130 gun keys 1/2 share the weapon keys (review M6) ...
        // SAFETY: DIK_ESCAPE < 256, inside the buffer.
        let esc = unsafe { *data.add(DIK_ESCAPE) } & 0x80 != 0;
        if press_edge(ESC_RAW_DOWN.swap(esc, Ordering::Relaxed), esc) {
            ESC_PRESSES.fetch_add(1, Ordering::Relaxed);
        }
        for i in 0..AC_KEYS {
            let k = AC_DIK[i].load(Ordering::Relaxed) as usize;
            // SAFETY: k < 256, inside the buffer.
            let down = unsafe { *data.add(k) } & 0x80 != 0;
            if press_edge(AC_RAW_DOWN[i].swap(down, Ordering::Relaxed), down) {
                AC_PRESSES[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // M6: the weapon keys (next / prev): raw edge -> counter, hidden from ER through a release latch while MW2
        // mode drives (heartbeat rule); never in the gunship (the AC-130 shares 1/2 and reads its own counters).
        let wwant = effective_want(WANT_BLANK_WEAPON.load(Ordering::Relaxed), now, heartbeat)
            && !effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, heartbeat);
        for i in 0..2 {
            let k = WEAPON_DIK[i].load(Ordering::Relaxed) as usize;
            if k == 0 {
                continue;
            }
            // SAFETY: k < 256, inside the buffer.
            let kp = unsafe { data.add(k) };
            // SAFETY: inside the buffer.
            let down = unsafe { *kp } & 0x80 != 0;
            if press_edge(WEAPON_RAW_DOWN[i].swap(down, Ordering::Relaxed), down) {
                PRESSES_WEAPON[i].fetch_add(1, Ordering::Relaxed);
            }
            let blank = latch_next(WEAPON_LATCH[i].load(Ordering::Relaxed), wwant, down);
            WEAPON_LATCH[i].store(blank, Ordering::Relaxed);
            if blank {
                // SAFETY: inside the buffer.
                unsafe { *kp = 0 };
            }
        }
        // M7: the nuke key (MW2 +actionslot 4): raw edge -> counter, hidden from ER through a release latch while MW2
        // mode drives (heartbeat rule), like the weapon keys
        let nk = crate::shared::NUKE_DIK.load(Ordering::Relaxed) as usize;
        if nk != 0 {
            // SAFETY: nk < 256, inside the buffer.
            let np = unsafe { data.add(nk) };
            // SAFETY: inside the buffer.
            let down = unsafe { *np } & 0x80 != 0;
            if press_edge(crate::shared::NUKE_RAW_DOWN.swap(down, Ordering::Relaxed), down) {
                crate::shared::PRESSES_NUKE.fetch_add(1, Ordering::Relaxed);
            }
            let nwant = effective_want(crate::shared::WANT_BLANK_NUKE.load(Ordering::Relaxed), now, heartbeat);
            let blank = latch_next(crate::shared::NUKE_LATCH.load(Ordering::Relaxed), nwant, down);
            crate::shared::NUKE_LATCH.store(blank, Ordering::Relaxed);
            if blank {
                // SAFETY: inside the buffer.
                unsafe { *np = 0 };
            }
        }
        // 2026-10-21: the heal key (default Q): raw edge -> one press of ER's own use-item key (`heal_key_step`) while MW2
        // mode drives (heartbeat rule, never in the gunship, where Q is the next AC-130 gun); the heal key itself is
        // hidden from ER through a release latch (Q is ER's lock-on). The use-item key is written further down, after
        // every blank of this read, and after the reload key's edge was recorded above (the MW2 gun never reloads).
        let gunship_now = effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, heartbeat);
        let heal_ok = effective_want(crate::shared::WANT_HEAL.load(Ordering::Relaxed), now, heartbeat) && !gunship_now;
        let mut heal_edge = false;
        let hk = crate::shared::HEAL_DIK.load(Ordering::Relaxed) as usize;
        if hk != 0 {
            // SAFETY: hk < 256, inside the buffer.
            let hp = unsafe { data.add(hk) };
            // SAFETY: inside the buffer.
            let down = unsafe { *hp } & 0x80 != 0;
            heal_edge = press_edge(crate::shared::HEAL_RAW_DOWN.swap(down, Ordering::Relaxed), down);
            let blank = latch_next(crate::shared::HEAL_LATCH.load(Ordering::Relaxed), heal_ok, down);
            crate::shared::HEAL_LATCH.store(blank, Ordering::Relaxed);
            if blank {
                // SAFETY: inside the buffer.
                unsafe { *hp = 0 };
            }
        }
        let mut heal_t0 = crate::shared::HEAL_T0.load(Ordering::Relaxed);
        // 2026-10-24: dead (the task's death gate, until alive after the respawn): the key stays hidden, no drink
        let heal_dead = effective_want(crate::shared::WANT_DEAD_BLOCK.load(Ordering::Relaxed), now, heartbeat);
        let (heal_synth, heal_ev) = heal_key_step(&mut heal_t0, heal_edge, heal_ok && !heal_dead, now);
        crate::shared::HEAL_T0.store(heal_t0, Ordering::Relaxed);
        match heal_ev {
            HealKey::Started => {
                crate::shared::HEAL_KEY_PULSES.fetch_add(1, Ordering::Relaxed);
            }
            HealKey::Refused if !gunship_now && crate::shared::MODE_ON.load(Ordering::Relaxed) => {
                crate::shared::HEAL_KEY_REFUSED.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        if BLANK_TOGGLE_KEY.load(Ordering::Relaxed) {
            let k = AC_DIK[AC_KEY_CALL].load(Ordering::Relaxed) as usize;
            // SAFETY: k < 256, inside the buffer (the call-in key is hidden like the toggle keys).
            unsafe { *data.add(k) = 0 };
        }
        // M5: the movement keys hidden from ER while the controller drives (after every level/edge above was recorded)
        // SAFETY: the 256-byte keyboard buffer of this call.
        unsafe { blank_move_keys(data, now, heartbeat) };
        // 2026-10-21: the heal pulse: ER reads its use-item key down (after every blank above; never in the gunship)
        if heal_synth {
            let u = crate::shared::ER_USE_ITEM_DIK.load(Ordering::Relaxed) as usize;
            // SAFETY: u < 256, inside the buffer.
            unsafe { *data.add(u) = 0x80 };
        }
        // C6: the keyboard chord's E (ER's two-hand modifier), never in the gunship, only with a fresh task heartbeat
        let kc = crate::shared::KB_CHORD_T0.load(Ordering::Relaxed);
        if kc != 0 && !gunship_now && effective_want(true, now, heartbeat) && mw2er_core::pad_logic::chord_bits(now.saturating_sub(kc)).0 {
            // SAFETY: KB_CHORD_DIK < 256, inside the buffer.
            unsafe { *data.add(crate::shared::KB_CHORD_DIK) = 0x80 };
        }
        // ... then, in the gunship, ER sees every key released except Esc (its pause menu still opens).
        if effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, heartbeat) {
            // SAFETY: GetDeviceState succeeded with the 256-byte keyboard buffer.
            capture_keyboard(unsafe { std::slice::from_raw_parts_mut(data, KEYBOARD_STATE_SIZE as usize) });
        }
    }
    // Any other size (DIJOYSTATE etc.): untouched.
}

#[inline(always)]
unsafe fn detour_body(slot: usize, this: *mut c_void, cb: u32, data: *mut c_void) -> HRESULT {
    let orig = TRAMPOLINES[slot].load(Ordering::Acquire);
    if orig == 0 {
        return E_FAIL; // cannot happen once the hook is enabled
    }
    // SAFETY: orig is the MinHook trampoline for this GetDeviceState target.
    let orig: GetDeviceStateFn = unsafe { std::mem::transmute(orig) };
    // SAFETY: forwarding the game's own arguments.
    let hr = unsafe { orig(this, cb, data) };
    if hr.is_ok() && !data.is_null() {
        // Defensive: the body has no panicking calls; an unwind must not cross this frame.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: GetDeviceState succeeded, so `data` holds `cb` bytes the game owns.
            unsafe { process_state(cb, data as *mut u8) }
        }));
    }
    hr
}

unsafe extern "system" fn detour0(this: *mut c_void, cb: u32, data: *mut c_void) -> HRESULT {
    // SAFETY: called by the game in place of GetDeviceState.
    unsafe { detour_body(0, this, cb, data) }
}
unsafe extern "system" fn detour1(this: *mut c_void, cb: u32, data: *mut c_void) -> HRESULT {
    // SAFETY: as detour0.
    unsafe { detour_body(1, this, cb, data) }
}
unsafe extern "system" fn detour2(this: *mut c_void, cb: u32, data: *mut c_void) -> HRESULT {
    // SAFETY: as detour0.
    unsafe { detour_body(2, this, cb, data) }
}
unsafe extern "system" fn detour3(this: *mut c_void, cb: u32, data: *mut c_void) -> HRESULT {
    // SAFETY: as detour0.
    unsafe { detour_body(3, this, cb, data) }
}

const DETOURS: [GetDeviceStateFn; SLOTS] = [detour0, detour1, detour2, detour3];

/// GetDeviceState addresses of keyboard and mouse devices of one interface flavour.
fn targets_w(hinst: HINSTANCE) -> Result<Vec<usize>, String> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid out pointer; IID matches the requested interface.
    unsafe { DirectInput8Create(hinst, DIRECTINPUT_VERSION, &IDirectInput8W::IID, &mut raw, None) }
        .map_err(|e| format!("DirectInput8Create(W): {e}"))?;
    // SAFETY: DirectInput8Create returned an IDirectInput8W with one reference we now own.
    let di = unsafe { IDirectInput8W::from_raw(raw) };
    let mut out = Vec::new();
    for guid in [&GUID_SysKeyboard, &GUID_SysMouse] {
        let mut dev: Option<IDirectInputDevice8W> = None;
        // SAFETY: valid GUID and out pointer.
        unsafe { di.CreateDevice(guid as *const GUID, &mut dev, None) }.map_err(|e| format!("CreateDevice(W): {e}"))?;
        let dev = dev.ok_or("CreateDevice(W) returned null")?;
        out.push(dev.vtable().GetDeviceState as usize);
    } // devices and the interface are released on drop
    Ok(out)
}

fn targets_a(hinst: HINSTANCE) -> Result<Vec<usize>, String> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: as targets_w.
    unsafe { DirectInput8Create(hinst, DIRECTINPUT_VERSION, &IDirectInput8A::IID, &mut raw, None) }
        .map_err(|e| format!("DirectInput8Create(A): {e}"))?;
    // SAFETY: as targets_w.
    let di = unsafe { IDirectInput8A::from_raw(raw) };
    let mut out = Vec::new();
    for guid in [&GUID_SysKeyboard, &GUID_SysMouse] {
        let mut dev: Option<IDirectInputDevice8A> = None;
        // SAFETY: as targets_w.
        unsafe { di.CreateDevice(guid as *const GUID, &mut dev, None) }.map_err(|e| format!("CreateDevice(A): {e}"))?;
        let dev = dev.ok_or("CreateDevice(A) returned null")?;
        out.push(dev.vtable().GetDeviceState as usize);
    }
    Ok(out)
}

/// Installs the detours on hudhook's MinHook. Returns the hooked addresses.
pub fn install() -> Result<Vec<usize>, String> {
    // MinHook is normally initialised by Hudhook::builder(); if the HUD failed before that, do it here.
    // SAFETY: MH_Initialize is idempotent (ALREADY_INITIALIZED).
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        s => return Err(format!("MH_Initialize: {s:?}")),
    }
    // SAFETY: loading a system DLL by name (the game loads it anyway).
    unsafe { LoadLibraryW(w!("dinput8.dll")) }.map_err(|e| format!("LoadLibraryW(dinput8): {e}"))?;
    // SAFETY: `None` = the exe module.
    let exe = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let hinst = HINSTANCE(exe.0);

    let mut addrs = targets_w(hinst)?;
    match targets_a(hinst) {
        Ok(a) => addrs.extend(a),
        Err(e) => crate::log::log_w!("INPUT", "WARN ANSI interface unavailable ({e}); hooking the wide one only"),
    }
    addrs.sort_unstable();
    addrs.dedup();
    if addrs.len() > SLOTS {
        return Err(format!("{} distinct GetDeviceState targets (max {SLOTS})", addrs.len()));
    }
    for (slot, &target) in addrs.iter().enumerate() {
        // SAFETY: target is GetDeviceState in dinput8.dll; the detour has the same signature.
        let hook = unsafe { MhHook::new(target as *mut c_void, DETOURS[slot] as *mut c_void) }
            .map_err(|s| format!("MH_CreateHook 0x{target:x}: {s:?}"))?;
        TRAMPOLINES[slot].store(hook.trampoline() as usize, Ordering::Release);
        // SAFETY: the hook was created above; the trampoline is stored before enabling.
        match unsafe { MH_EnableHook(target as *mut c_void) } {
            MH_STATUS::MH_OK => {}
            s => return Err(format!("MH_EnableHook 0x{target:x}: {s:?}")),
        }
        std::mem::forget(hook); // MhHook has no Drop; the hook lives for the process lifetime
    }
    Ok(addrs)
}
