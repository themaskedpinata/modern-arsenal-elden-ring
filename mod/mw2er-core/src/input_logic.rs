//! Input rules used by the DirectInput `GetDeviceState` detour and the game task.
//!
//! The detour itself lives in the DLL and may only touch atomics; the decisions it makes are the
//! pure functions here, so they are unit-tested.

/// DirectInput keyboard scan code (`DIK_*`) for a key name: F1-F12, A-Z, 0-9 (case-insensitive).
pub fn dik_from_name(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_uppercase();
    let code = match n.as_str() {
        "F1" => 0x3B,
        "F2" => 0x3C,
        "F3" => 0x3D,
        "F4" => 0x3E,
        "F5" => 0x3F,
        "F6" => 0x40,
        "F7" => 0x41,
        "F8" => 0x42,
        "F9" => 0x43,
        "F10" => 0x44,
        "F11" => 0x57,
        "F12" => 0x58,
        "1" => 0x02,
        "2" => 0x03,
        "3" => 0x04,
        "4" => 0x05,
        "5" => 0x06,
        "6" => 0x07,
        "7" => 0x08,
        "8" => 0x09,
        "9" => 0x0A,
        "0" => 0x0B,
        "Q" => 0x10,
        "W" => 0x11,
        "E" => 0x12,
        "R" => 0x13,
        "T" => 0x14,
        "Y" => 0x15,
        "U" => 0x16,
        "I" => 0x17,
        "O" => 0x18,
        "P" => 0x19,
        "A" => 0x1E,
        "S" => 0x1F,
        "D" => 0x20,
        "F" => 0x21,
        "G" => 0x22,
        "H" => 0x23,
        "J" => 0x24,
        "K" => 0x25,
        "L" => 0x26,
        "Z" => 0x2C,
        "X" => 0x2D,
        "C" => 0x2E,
        "V" => 0x2F,
        "B" => 0x30,
        "N" => 0x31,
        "M" => 0x32,
        // M4: the extra AC-130 exit key (DIK_BACK)
        "BACK" | "BACKSPACE" => 0x0E,
        // M5: the MW2 PC movement binds (docs/M5_DESIGN.md section 6.1)
        "SPACE" => 0x39,
        "LSHIFT" | "SHIFT" => 0x2A,
        "RSHIFT" => 0x36,
        "LCTRL" | "LCONTROL" | "CTRL" => 0x1D,
        "RCTRL" | "RCONTROL" => 0x9D,
        "LALT" | "ALT" => 0x38,
        "RALT" => 0xB8,
        "TAB" => 0x0F,
        "CAPSLOCK" => 0x3A,
        _ => return None,
    };
    Some(code)
}

/// `DIK_ESCAPE`: never zeroed by the gunship capture, so ER's pause menu still opens (and ends the ride: `menu`).
pub const DIK_ESCAPE: usize = 0x01;

/// `DIMOUSESTATE(2)` axes: `lX@0`, `lY@4`, `lZ@8` (i32 each), then `rgbButtons@12`.
pub fn mouse_axes(buf: &[u8]) -> Option<(i32, i32, i32)> {
    if buf.len() < 12 {
        return None;
    }
    let rd = |o: usize| i32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    Some((rd(0), rd(4), rd(8)))
}

/// M4 gunship capture of a mouse state buffer (16 or 20 bytes): returns the axes and the raw button bytes' "down"
/// bits, then zeroes the axes and every button so ER neither turns its camera nor attacks. Other sizes: untouched.
pub fn capture_mouse(buf: &mut [u8]) -> Option<((i32, i32, i32), u8)> {
    if !matches!(buf.len(), 16 | 20) {
        return None;
    }
    let axes = mouse_axes(buf)?;
    let mut down = 0u8;
    for (i, b) in buf[12..].iter_mut().enumerate() {
        if *b & 0x80 != 0 {
            down |= 1 << i;
        }
        *b = 0;
    }
    buf[..12].fill(0);
    Some((axes, down))
}

/// M4 gunship capture of a 256-byte keyboard buffer: every key reads released except Esc. The caller records its
/// own key edges BEFORE this call.
pub fn capture_keyboard(buf: &mut [u8]) -> bool {
    if buf.len() != KEYBOARD_STATE_SIZE as usize {
        return false;
    }
    let esc = buf[DIK_ESCAPE];
    buf.fill(0);
    buf[DIK_ESCAPE] = esc;
    true
}

/// Mouse wheel counts -> gun steps: one notch (120) is one step; a remainder is kept for the next call.
pub fn wheel_steps(acc: &mut i32, delta: i32) -> i32 {
    *acc = acc.saturating_add(delta);
    let steps = *acc / 120;
    *acc -= steps * 120;
    steps
}

/// Byte offset of `rgbButtons[button]` inside `DIMOUSESTATE` (cb 16, 4 buttons) or
/// `DIMOUSESTATE2` (cb 20, 8 buttons). `None` for other sizes or out-of-range buttons. The axes
/// (`lX`, `lY`, `lZ`, bytes 0..12) are never addressed: the camera is the aim.
pub fn mouse_button_offset(cb_data: u32, button: u8) -> Option<usize> {
    let buttons = match cb_data {
        16 => 4,
        20 => 8,
        _ => return None,
    };
    ((button as usize) < buttons).then_some(12 + button as usize)
}

/// Size of a DirectInput keyboard state buffer (`c_dfDIKeyboard`).
pub const KEYBOARD_STATE_SIZE: u32 = 256;

/// Release latch: keep blanking a button that is still held after blanking stops being wanted,
/// so an ER attack (or item use) does not leak out when MW2 mode turns off or a menu opens
/// mid-press (Sunless note, gotcha 18). Returns the new "blanking" state.
pub fn latch_next(was_blanking: bool, want: bool, down: bool) -> bool {
    want || (was_blanking && down)
}

/// Stateful wrapper of [`latch_next`] (the DLL keeps the state in an atomic instead).
#[derive(Clone, Copy, Debug, Default)]
pub struct BlankLatch {
    blanking: bool,
}

impl BlankLatch {
    pub fn update(&mut self, want: bool, down: bool) -> bool {
        self.blanking = latch_next(self.blanking, want, down);
        self.blanking
    }
}

/// The game task's blanking request counts only while the task's heartbeat is this fresh: if the
/// task stops ticking (title screen, loading, a hang), LMB and R go back to the game by themselves.
pub const TASK_HEARTBEAT_MAX_AGE_MS: u64 = 250;
/// The task ignores the fire button when the game has not polled the mouse for this long (the raw
/// state is a sticky last value).
pub const MOUSE_POLL_MAX_AGE_MS: u64 = 150;

/// True when `last_ms` (0 = never) is at most `max_age_ms` before `now_ms`. Clock values are
/// milliseconds since boot; a stamp in the future (clock read order between threads) counts as fresh.
pub fn fresh(now_ms: u64, last_ms: u64, max_age_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) <= max_age_ms
}

/// The blanking request the input detour applies: the task's wish, but only while its heartbeat is
/// fresh. The release latch ([`latch_next`]) still covers a button that is held when this drops.
pub fn effective_want(want: bool, now_ms: u64, heartbeat_ms: u64) -> bool {
    want && fresh(now_ms, heartbeat_ms, TASK_HEARTBEAT_MAX_AGE_MS)
}

/// True on a 0 -> 1 transition.
pub fn press_edge(prev_down: bool, down: bool) -> bool {
    down && !prev_down
}

/// 2026-10-21: how long ER sees its use-item key / button after a heal press (one drink of the selected quick item,
/// not a held key; ER reads the key on several frames even at 30 fps).
pub const HEAL_PULSE_MS: u64 = 150;

/// What one keyboard read did with the heal key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealKey {
    None,
    /// A press while MW2 mode drives: ER's use-item key is pressed for [`HEAL_PULSE_MS`].
    Started,
    /// A press while it does not (a menu, not in world, unfocused): nothing reaches ER.
    Refused,
}

/// 2026-10-21: the keyboard heal (`[keys] heal`, default Q) in the detour. A press edge while `ok` (MW2 mode drives:
/// in world, no menu, focused, no AC-130 ride, task heartbeat fresh) starts a pulse of ER's use-item key
/// (`[keys] er_use_item`, ER's R) that lasts [`HEAL_PULSE_MS`] and ends early when `ok` drops. `t0` is the pulse start
/// (ms; 0 = none). Returns (ER's use-item key reads down this read, what the press did). The caller records the reload
/// key's edge BEFORE writing the synthesized key, so the MW2 gun never reloads from it.
pub fn heal_key_step(t0: &mut u64, edge: bool, ok: bool, now_ms: u64) -> (bool, HealKey) {
    let mut ev = HealKey::None;
    if edge {
        if ok {
            *t0 = now_ms.max(1);
            ev = HealKey::Started;
        } else {
            ev = HealKey::Refused;
        }
    }
    if *t0 != 0 {
        if ok && now_ms.saturating_sub(*t0) < HEAL_PULSE_MS {
            return (true, ev);
        }
        *t0 = 0;
    }
    (false, ev)
}

/// Reads a monotonically increasing press counter (written by the detour) as deltas.
#[derive(Clone, Copy, Debug, Default)]
pub struct CounterReader {
    last: u32,
}

impl CounterReader {
    /// Starts at the counter's current value so presses from before are not replayed.
    pub fn starting_at(current: u32) -> Self {
        Self { last: current }
    }

    /// Presses since the previous call (wrapping-safe).
    pub fn take(&mut self, current: u32) -> u32 {
        let d = current.wrapping_sub(self.last);
        self.last = current;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latch_blanks_while_wanted() {
        let mut l = BlankLatch::default();
        assert!(l.update(true, true));
        assert!(l.update(true, false));
        assert!(!l.update(false, false));
        assert!(!l.update(false, true), "a press that starts while not wanted is never blanked");
    }

    #[test]
    fn latch_holds_until_release() {
        let mut l = BlankLatch::default();
        assert!(l.update(true, true)); // firing in MW2 mode
        assert!(l.update(false, true)); // mode turned off / menu opened, LMB still held
        assert!(l.update(false, true));
        assert!(!l.update(false, false)); // released: blanking ends
        assert!(!l.update(false, true)); // a new press reaches the game
    }

    #[test]
    fn latch_pure_function_table() {
        assert!(latch_next(false, true, false));
        assert!(latch_next(true, false, true));
        assert!(!latch_next(true, false, false));
        assert!(!latch_next(false, false, true));
    }

    #[test]
    fn latch_heartbeat_drops_stale_blanking() {
        // the task ticked 100 ms ago: its wish applies
        assert!(effective_want(true, 10_100, 10_000));
        // the task stopped ticking (title screen / loading): blanking ends by itself
        assert!(!effective_want(true, 10_300, 10_000));
        // never ticked
        assert!(!effective_want(true, 10_000, 0));
        assert!(!effective_want(false, 10_000, 10_000));
        // a stamp from slightly in the future still counts as fresh
        assert!(effective_want(true, 9_990, 10_000));
        // a held button keeps being blanked through the latch after the heartbeat goes stale
        let mut blanking = latch_next(false, effective_want(true, 10_000, 10_000), true);
        assert!(blanking);
        blanking = latch_next(blanking, effective_want(true, 20_000, 10_000), true);
        assert!(blanking, "held: still blanked");
        blanking = latch_next(blanking, effective_want(true, 20_016, 10_000), false);
        assert!(!blanking, "released: the game gets the button back");
        assert!(!latch_next(blanking, effective_want(true, 20_032, 10_000), true), "a new press reaches the game");
        // the mouse poll staleness uses the same rule
        assert!(fresh(1_000, 900, MOUSE_POLL_MAX_AGE_MS));
        assert!(!fresh(1_200, 900, MOUSE_POLL_MAX_AGE_MS));
    }

    #[test]
    fn key_names_and_mouse_offsets() {
        assert_eq!(dik_from_name("F5"), Some(0x3F));
        assert_eq!(dik_from_name("r"), Some(0x13));
        assert_eq!(dik_from_name("F12"), Some(0x58));
        assert_eq!(dik_from_name("0"), Some(0x0B));
        assert_eq!(dik_from_name("F13"), None);
        // M5: Space is now a key name (MW2 +gostand)
        assert_eq!(dik_from_name("Space"), Some(0x39));
        assert_eq!(dik_from_name("Spacebar"), None);
        assert_eq!(mouse_button_offset(16, 0), Some(12));
        assert_eq!(mouse_button_offset(16, 4), None);
        assert_eq!(mouse_button_offset(20, 7), Some(19));
        assert_eq!(mouse_button_offset(256, 0), None);
    }

    #[test]
    fn ads_button_offset_and_latch() {
        // RMB = rgbButtons[1]: byte 13 in DIMOUSESTATE (cb 16) and DIMOUSESTATE2 (cb 20)
        assert_eq!(mouse_button_offset(16, 1), Some(13));
        assert_eq!(mouse_button_offset(20, 1), Some(13));
        // 255 = no ADS button: never addressed
        assert_eq!(mouse_button_offset(16, 255), None);
        assert_eq!(mouse_button_offset(20, 255), None);
        // the ADS button uses the same release latch as fire: held through mode-off, released -> back to the game
        let mut l = BlankLatch::default();
        assert!(l.update(true, true));
        assert!(l.update(false, true), "RMB still held after the mode turned off: still blanked");
        assert!(!l.update(false, false));
        assert!(!l.update(false, true), "a new RMB press reaches the game (guard)");
    }

    #[test]
    fn input_gunship_capture() {
        // DIMOUSESTATE2: lX = 5, lY = -3, lZ = 120, LMB + button 4 down
        let mut m = [0u8; 20];
        m[0..4].copy_from_slice(&5i32.to_le_bytes());
        m[4..8].copy_from_slice(&(-3i32).to_le_bytes());
        m[8..12].copy_from_slice(&120i32.to_le_bytes());
        m[12] = 0x80;
        m[16] = 0x80;
        let (axes, down) = capture_mouse(&mut m).unwrap();
        assert_eq!(axes, (5, -3, 120));
        assert_eq!(down, 0b1_0001);
        assert!(m.iter().all(|&b| b == 0), "axes and buttons zeroed");
        let mut short = [7u8; 16];
        assert!(capture_mouse(&mut short).is_some());
        assert!(short.iter().all(|&b| b == 0));
        let mut joy = [7u8; 80];
        assert!(capture_mouse(&mut joy).is_none());
        assert!(joy.iter().all(|&b| b == 7), "a gamepad buffer is never touched");
        // keyboard: everything released except Esc
        let mut k = [0x80u8; 256];
        assert!(capture_keyboard(&mut k));
        assert_eq!(k[DIK_ESCAPE], 0x80);
        assert_eq!(k.iter().filter(|&&b| b != 0).count(), 1);
        let mut k2 = [0u8; 256];
        k2[0x11] = 0x80; // W held
        capture_keyboard(&mut k2);
        assert_eq!(k2[0x11], 0, "a held W reads as released");
        assert!(!capture_keyboard(&mut [0u8; 16]));
        // inert with a stale heartbeat: the detour applies the capture only through effective_want
        assert!(!effective_want(true, 10_300, 10_000));
        // wheel: one notch per step, remainders kept
        let mut acc = 0;
        assert_eq!(wheel_steps(&mut acc, 120), 1);
        assert_eq!(wheel_steps(&mut acc, 60), 0);
        assert_eq!(wheel_steps(&mut acc, 60), 1);
        assert_eq!(wheel_steps(&mut acc, -240), -2);
        assert_eq!(dik_from_name("Back"), Some(0x0E));
    }

    /// 2026-10-21 (user: "a way to use heal pots while in MW2 mode"): Q = one press of ER's use-item key.
    #[test]
    fn heal_key_pulses_er_use_item_once() {
        let mut t0 = 0u64;
        assert_eq!(heal_key_step(&mut t0, true, true, 1000), (true, HealKey::Started));
        assert_eq!(heal_key_step(&mut t0, false, true, 1000 + HEAL_PULSE_MS - 1), (true, HealKey::None), "the pulse lasts");
        assert_eq!(heal_key_step(&mut t0, false, true, 1000 + HEAL_PULSE_MS), (false, HealKey::None), "a held Q is one drink");
        assert_eq!(t0, 0);
        // a menu / not driving: refused, nothing reaches ER
        assert_eq!(heal_key_step(&mut t0, true, false, 2000), (false, HealKey::Refused));
        assert_eq!(heal_key_step(&mut t0, false, true, 2010), (false, HealKey::None), "no late pulse after a refusal");
        // the pulse ends at once when MW2 mode stops driving (menu opened, AC-130 ride started, heartbeat stale)
        assert_eq!(heal_key_step(&mut t0, true, true, 3000), (true, HealKey::Started));
        assert_eq!(heal_key_step(&mut t0, false, false, 3016), (false, HealKey::None));
        assert_eq!(heal_key_step(&mut t0, false, true, 3032), (false, HealKey::None), "not resumed");
        // clock at 0 (not started): the pulse still counts as running
        let mut t0 = 0u64;
        assert_eq!(heal_key_step(&mut t0, true, true, 0), (true, HealKey::Started));
        assert_eq!(t0, 1);
    }

    #[test]
    fn edges_and_counters() {
        assert!(press_edge(false, true));
        assert!(!press_edge(true, true));
        let mut r = CounterReader::starting_at(5);
        assert_eq!(r.take(5), 0);
        assert_eq!(r.take(7), 2);
        let mut w = CounterReader::starting_at(u32::MAX);
        assert_eq!(w.take(1), 2);
    }
}
