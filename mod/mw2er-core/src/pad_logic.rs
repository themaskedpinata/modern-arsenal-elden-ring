//! C3 PASS B (docs/C3_DESIGN.md part 4): the gamepad (XInput) rules.
//!
//! The DLL detours `xinput1_4!XInputGetState` and may only touch atomics and the caller's `XINPUT_STATE`; every
//! decision it makes is a pure function here: button names and bindings (`[gamepad.bind]`, `[gamepad.gunship]`), the
//! per-read step (press edges, the X tap / hold pair with the synthesized ER interact button, the toggle chord, the
//! per-button release latch, the gunship blank-all-but-Start), the radial deadzones, MW2's look graph and turn rates
//! with the accel ramp, the 1/1024 count carry of the mouse route, and the analog usercmd of the left stick.
//!
//! [V] MW2 console look numbers (iw4sp.exe): yaw 260, pitch 90, ADS yaw 90, ADS pitch 55 deg/s; `input_graph` 3;
//! accel lerp 1200; the four `view_input_N.graph` curves (dumps\m5\common_mp\aim_assist, see `GRAPH_SOURCE`).
//! [I] the accel formula (ramp toward `accel_max_yaw` while the stick is at full yaw deflection), ER's mouse degrees
//! per count (calibrated live), ER's interact button on a pad (`er_interact`, default Y).

use crate::input_logic::{latch_next, press_edge};

// ---- XInput buttons ---------------------------------------------------------------------------------------------

pub const XI_DPAD_UP: u32 = 0x0001;
pub const XI_DPAD_DOWN: u32 = 0x0002;
pub const XI_DPAD_LEFT: u32 = 0x0004;
pub const XI_DPAD_RIGHT: u32 = 0x0008;
pub const XI_START: u32 = 0x0010;
pub const XI_BACK: u32 = 0x0020;
pub const XI_L3: u32 = 0x0040;
pub const XI_R3: u32 = 0x0080;
pub const XI_LB: u32 = 0x0100;
pub const XI_RB: u32 = 0x0200;
pub const XI_A: u32 = 0x1000;
pub const XI_B: u32 = 0x2000;
pub const XI_X: u32 = 0x4000;
pub const XI_Y: u32 = 0x8000;
/// The triggers as virtual buttons (above `trigger_threshold`), outside the 16 XInput bits.
pub const VB_LT: u32 = 1 << 16;
pub const VB_RT: u32 = 1 << 17;
/// Every real button bit (XInput's 0x0400 / 0x0800 are unused).
pub const XI_ALL: u32 = 0xF3FF;

/// Button names of the config (case-insensitive).
pub const BUTTONS: [(&str, u32); 16] = [
    ("a", XI_A),
    ("b", XI_B),
    ("x", XI_X),
    ("y", XI_Y),
    ("lb", XI_LB),
    ("rb", XI_RB),
    ("lt", VB_LT),
    ("rt", VB_RT),
    ("l3", XI_L3),
    ("r3", XI_R3),
    ("dpad_up", XI_DPAD_UP),
    ("dpad_down", XI_DPAD_DOWN),
    ("dpad_left", XI_DPAD_LEFT),
    ("dpad_right", XI_DPAD_RIGHT),
    ("start", XI_START),
    ("back", XI_BACK),
];

/// The bit of one button name.
pub fn button_bit(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase();
    let n = match n.as_str() {
        "view" | "select" => "back",
        "menu" | "options" => "start",
        x => x,
    };
    BUTTONS.iter().find(|(b, _)| *b == n).map(|(_, v)| *v)
}

/// The name of a single-button mask (for log lines); chords print as `a+b`.
pub fn mask_name(mask: u32) -> String {
    if mask == 0 {
        return "-".into();
    }
    let names: Vec<&str> = BUTTONS.iter().filter(|(_, b)| mask & b != 0).map(|(n, _)| *n).collect();
    names.join("+")
}

/// A binding: "" = unbound (0), one button, or a chord of exactly two buttons (`l3+r3`). `start` is never allowed.
pub fn parse_binding(what: &str, s: &str) -> Result<u32, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    let parts: Vec<&str> = s.split('+').collect();
    if parts.len() > 2 {
        return Err(format!("gamepad.{what} = {s:?}: chords are two buttons at most"));
    }
    let mut mask = 0u32;
    for p in parts {
        let b = button_bit(p).ok_or_else(|| format!("gamepad.{what} = {s:?}: {p:?} is not a button (a b x y lb rb lt rt l3 r3 dpad_up dpad_down dpad_left dpad_right back)"))?;
        if b == XI_START {
            return Err(format!("gamepad.{what} = {s:?}: start is never bound (it always opens ER's menu)"));
        }
        if mask & b != 0 {
            return Err(format!("gamepad.{what} = {s:?}: the same button twice"));
        }
        mask |= b;
    }
    Ok(mask)
}

// ---- actions and bindings -------------------------------------------------------------------------------------------

/// The press actions (bit `1 << act` in [`StepOut::presses`] / [`StepOut::held`]).
pub const ACT_TOGGLE: usize = 0;
pub const ACT_FP: usize = 1;
pub const ACT_RELOAD: usize = 2;
pub const ACT_INTERACT: usize = 3;
pub const ACT_WEAPON_NEXT: usize = 4;
pub const ACT_WEAPON_PREV: usize = 5;
pub const ACT_JUMP: usize = 6;
pub const ACT_STANCE: usize = 7;
pub const ACT_SPRINT: usize = 8;
pub const ACT_MELEE: usize = 9;
pub const ACT_AC130: usize = 10;
pub const ACT_NUKE: usize = 11;
pub const ACT_FIRE: usize = 12;
pub const ACT_ADS: usize = 13;
pub const ACT_GS_FIRE: usize = 14;
pub const ACT_GS_NEXT: usize = 15;
pub const ACT_GS_THERMAL: usize = 16;
pub const ACT_GS_EXIT: usize = 17;
/// 2026-10-21: drink the selected ER quick item (the flask): ER's use-item button (`er_use_item`) pulsed once.
pub const ACT_HEAL: usize = 18;
pub const N_ACT: usize = 19;

/// The MW2-mode actions in config order (name, index); `interact_hold` may share its button with `reload` only.
pub const MW2_ACTS: [(&str, usize); 15] = [
    ("toggle", ACT_TOGGLE),
    ("fp_toggle", ACT_FP),
    ("fire", ACT_FIRE),
    ("ads", ACT_ADS),
    ("reload", ACT_RELOAD),
    ("interact_hold", ACT_INTERACT),
    ("weapon_next", ACT_WEAPON_NEXT),
    ("weapon_prev", ACT_WEAPON_PREV),
    ("jump", ACT_JUMP),
    ("stance", ACT_STANCE),
    ("sprint", ACT_SPRINT),
    ("melee", ACT_MELEE),
    ("ac130", ACT_AC130),
    ("nuke", ACT_NUKE),
    ("heal", ACT_HEAL),
];
pub const GS_ACTS: [(&str, usize); 4] = [("fire", ACT_GS_FIRE), ("next", ACT_GS_NEXT), ("thermal", ACT_GS_THERMAL), ("exit", ACT_GS_EXIT)];

/// Resolved bindings (all `Copy`: the detour builds one on its stack from atomics, no allocation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Binds {
    /// Button mask per action (0 = unbound).
    pub masks: [u32; N_ACT],
    /// Buttons hidden from ER while MW2 mode drives (`blank_er`, plus every bound MW2 button and the trigger bits).
    pub blank: u32,
    /// `blank_er` lists "rs" (the right stick is ours while the look route is `mouse`) / "ls_fp" (the left stick
    /// follows the W/A/S/D rule).
    pub blank_rs: bool,
    pub blank_ls: bool,
    /// The ER button synthesized while the interact hold fires (ER's event action) [I: Y on ER's default pad layout].
    pub er_interact: u32,
    /// Review C3: the ER button pulsed for [`MELEE_PULSE_MS`] when the melee binding is pressed in MW2 mode (ER's
    /// right-hand light attack, RB on ER's default pad layout [I]); 0 = melee does nothing.
    pub er_melee: u32,
    /// 2026-10-21: the ER button pulsed for [`HEAL_PULSE_MS`] when the heal binding is pressed in MW2 mode (ER's
    /// use-item = the selected quick item, X on ER's default pad layout [I]); 0 = heal does nothing.
    pub er_use_item: u32,
}

/// Review C3: how long ER sees the synthesized melee button after the melee press (one light attack, not a held RB).
pub const MELEE_PULSE_MS: u64 = 120;
/// 2026-10-21: how long ER sees its use-item button after a heal press (one drink, not a held button); the keyboard's
/// pulse of ER's use-item key is the same length.
pub const HEAL_PULSE_MS: u64 = crate::input_logic::HEAL_PULSE_MS;
/// 2026-10-21: the shortest time ER sees its interact button after an interact (a tap at ER's prompt, or a hold released
/// right after `hold_ms`): a human tap's length, several of ER's 60 Hz polls.
pub const INTERACT_PULSE_MS: u64 = 120;

/// The names `blank_er` accepts: every button except start, plus the two sticks.
pub fn blank_er_parse(list: &[String]) -> Result<(u32, bool, bool), String> {
    let mut mask = 0u32;
    let (mut rs, mut ls) = (false, false);
    for n in list {
        match n.trim().to_ascii_lowercase().as_str() {
            "rs" => rs = true,
            "ls_fp" => ls = true,
            other => {
                let b = button_bit(other).ok_or_else(|| format!("gamepad.blank_er: {n:?} is not a button / rs / ls_fp"))?;
                if b == XI_START {
                    return Err("gamepad.blank_er must not list start (ER's menu stays reachable)".into());
                }
                mask |= b;
            }
        }
    }
    Ok((mask, rs, ls))
}

impl Binds {
    /// Resolves and validates the bind tables: unknown names, `start`, duplicates among the MW2 actions (only the
    /// reload / interact_hold tap-hold pair may share a button) and among the gunship actions are rejected.
    pub fn resolve(mw2: &[(&str, &str)], gunship: &[(&str, &str)], blank_er: &[String], er_interact: &str) -> Result<Binds, String> {
        let mut b = Binds::default();
        for (name, value) in mw2 {
            let idx = MW2_ACTS.iter().find(|(n, _)| n == name).map(|(_, i)| *i).ok_or_else(|| format!("gamepad.bind.{name}: unknown action"))?;
            b.masks[idx] = parse_binding(&format!("bind.{name}"), value)?;
        }
        for (name, value) in gunship {
            let idx = GS_ACTS.iter().find(|(n, _)| n == name).map(|(_, i)| *i).ok_or_else(|| format!("gamepad.gunship.{name}: unknown action"))?;
            b.masks[idx] = parse_binding(&format!("gunship.{name}"), value)?;
        }
        for (i, (na, ia)) in MW2_ACTS.iter().enumerate() {
            for (nb, ib) in &MW2_ACTS[i + 1..] {
                let (ma, mb) = (b.masks[*ia], b.masks[*ib]);
                let pair = matches!((*ia, *ib), (ACT_RELOAD, ACT_INTERACT) | (ACT_INTERACT, ACT_RELOAD));
                if ma != 0 && ma == mb && !pair {
                    return Err(format!("gamepad.bind.{na} and gamepad.bind.{nb} are the same button(s) ({})", mask_name(ma)));
                }
            }
        }
        for (i, (na, ia)) in GS_ACTS.iter().enumerate() {
            for (nb, ib) in &GS_ACTS[i + 1..] {
                if b.masks[*ia] != 0 && b.masks[*ia] == b.masks[*ib] {
                    return Err(format!("gamepad.gunship.{na} and gamepad.gunship.{nb} are the same button(s)"));
                }
            }
        }
        let (mask, rs, ls) = blank_er_parse(blank_er)?;
        b.blank = mask;
        b.blank_rs = rs;
        b.blank_ls = ls;
        let ei = parse_binding("er_interact", er_interact)?;
        if ei.count_ones() != 1 || ei & (VB_LT | VB_RT) != 0 {
            return Err(format!("gamepad.er_interact = {er_interact:?} must be one face / shoulder / stick / d-pad button"));
        }
        b.er_interact = ei;
        Ok(b)
    }

    /// Review C3: `[gamepad] er_melee` ("" = off): one face / shoulder button.
    pub fn with_er_melee(mut self, er_melee: &str) -> Result<Binds, String> {
        let m = parse_binding("er_melee", er_melee)?;
        if m != 0 && (m.count_ones() != 1 || m & (VB_LT | VB_RT) != 0 || m == XI_START) {
            return Err(format!("gamepad.er_melee = {er_melee:?} must be \"\" or one face / shoulder / stick / d-pad button"));
        }
        self.er_melee = m;
        Ok(self)
    }

    /// 2026-10-21: `[gamepad] er_use_item` ("" = heal off): one face / shoulder / stick / d-pad button.
    pub fn with_er_use_item(mut self, er_use_item: &str) -> Result<Binds, String> {
        let m = parse_binding("er_use_item", er_use_item)?;
        if m != 0 && (m.count_ones() != 1 || m & (VB_LT | VB_RT) != 0) {
            return Err(format!("gamepad.er_use_item = {er_use_item:?} must be \"\" or one face / shoulder / stick / d-pad button"));
        }
        self.er_use_item = m;
        Ok(self)
    }

    pub fn tap_hold_pair(&self) -> bool {
        self.masks[ACT_RELOAD] != 0 && self.masks[ACT_RELOAD] == self.masks[ACT_INTERACT]
    }
}

// ---- the per-read step of the detour --------------------------------------------------------------------------------

/// The raw `XINPUT_GAMEPAD` (12 bytes after the packet number).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl Raw {
    pub fn from_bytes(b: &[u8; 12]) -> Raw {
        let i = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        Raw { buttons: u16::from_le_bytes([b[0], b[1]]), lt: b[2], rt: b[3], lx: i(4), ly: i(6), rx: i(8), ry: i(10) }
    }

    pub fn to_bytes(self) -> [u8; 12] {
        let mut o = [0u8; 12];
        o[0..2].copy_from_slice(&self.buttons.to_le_bytes());
        o[2] = self.lt;
        o[3] = self.rt;
        o[4..6].copy_from_slice(&self.lx.to_le_bytes());
        o[6..8].copy_from_slice(&self.ly.to_le_bytes());
        o[8..10].copy_from_slice(&self.rx.to_le_bytes());
        o[10..12].copy_from_slice(&self.ry.to_le_bytes());
        o
    }

    /// Button levels with the triggers as virtual buttons above `threshold` (0..255).
    pub fn levels(&self, threshold: u8) -> u32 {
        let mut l = self.buttons as u32 & XI_ALL;
        if self.lt > threshold {
            l |= VB_LT;
        }
        if self.rt > threshold {
            l |= VB_RT;
        }
        l
    }

    /// Any input at all (a button, a trigger past 30, a stick past 0.25): selects the slot with `pad_index = -1`.
    pub fn active(&self) -> bool {
        let big = |v: i16| (v as i32).abs() > 8192;
        self.levels(30) != 0 || big(self.lx) || big(self.ly) || big(self.rx) || big(self.ry)
    }
}

/// The trigger level below which a hidden trigger is released for the latch (ER may react to a lighter press than
/// our fire threshold).
pub const TRIGGER_RELEASE: u8 = 8;

/// The detour's state (the DLL keeps it in atomics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepState {
    pub prev: u32,
    pub latched: u32,
    /// The toggle chord's buttons, hidden until each one is released.
    pub chord_hide: u32,
    /// The interact-hold button's press time (ms; 0 = not timing) and whether the hold fired.
    pub hold_t0: u64,
    pub hold_fired: bool,
    /// Review C3: the melee press time (ms; 0 = no pulse).
    pub melee_t0: u64,
    /// 2026-10-21: the last interact's time (ms; 0 = no pulse): ER sees its interact button for INTERACT_PULSE_MS at least.
    pub interact_t0: u64,
    /// 2026-10-21: the heal press time (ms; 0 = no pulse).
    pub heal_t0: u64,
    /// C6 (docs/C6_HOLD_DESIGN.md 3.4): the two-hand chord's start (ms; 0 = none) and the last request seen.
    pub chord_t0: u64,
    pub chord_seq: u32,
}

/// C6: what happened to the two-hand chord in this read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChordEv {
    #[default]
    None,
    Started,
    Done,
    /// Stopped before the end (both buttons released in the same read): prompt / pulse / mode.
    Aborted(&'static str),
    /// Not started: prompt / pulse / hold / mode.
    Refused(&'static str),
}

impl ChordEv {
    pub fn code(self) -> u32 {
        match self {
            ChordEv::None => 0,
            ChordEv::Started => 1,
            ChordEv::Done => 2,
            ChordEv::Aborted(_) => 3,
            ChordEv::Refused(_) => 4,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            ChordEv::Aborted(r) | ChordEv::Refused(r) => r,
            _ => "",
        }
    }
}

/// C6: the chord's timeline (ms from its start): ER's two-hand modifier (Y) held 0..300, ER's right-hand button (RB)
/// 100..220, so RB is never down without Y [I4: ER's default pad layout].
pub const CHORD_HOLD_MS: u64 = 300;
pub const CHORD_TAP_FROM_MS: u64 = 100;
pub const CHORD_TAP_TO_MS: u64 = 220;
/// C6 probe `tp_ads_er_aim`: ER's aim button with a two-handed crossbow (LB on ER's default pad) [I8].
pub const ER_AIM_BUTTON: u32 = XI_LB;
/// Refusal / abort reasons as small codes for the detour's atomics.
pub const CHORD_REASONS: [&str; 5] = ["", "prompt", "pulse", "hold", "mode"];

pub fn chord_reason_code(r: &str) -> u32 {
    CHORD_REASONS.iter().position(|x| *x == r).unwrap_or(0) as u32
}

/// (hold, tap) buttons of the chord at `elapsed_ms`: the tap only inside the hold.
pub fn chord_bits(elapsed_ms: u64) -> (bool, bool) {
    let hold = elapsed_ms < CHORD_HOLD_MS;
    let tap = hold && (CHORD_TAP_FROM_MS..CHORD_TAP_TO_MS).contains(&elapsed_ms);
    (hold, tap)
}

/// The chord's buttons from the binds: ER's interact (its two-hand modifier, Y) and ER's light attack (RB); ER's
/// default pad buttons when either is configured off.
pub fn chord_buttons(b: &Binds) -> (u32, u32) {
    (if b.er_interact != 0 { b.er_interact } else { XI_Y }, if b.er_melee != 0 { b.er_melee } else { XI_RB })
}

#[derive(Clone, Copy, Debug)]
pub struct StepIn {
    /// Levels (triggers at the fire threshold) and the latch levels (triggers above `TRIGGER_RELEASE`).
    pub levels: u32,
    pub levels_lo: u32,
    pub now_ms: u64,
    /// MW2 mode drives (the task's wish through the heartbeat rule) / the gunship capture (same rule).
    pub mw2: bool,
    pub gunship: bool,
    pub hold_ms: u32,
    /// `[keys] blank_toggle_key`: the toggle chord is hidden from ER even with MW2 mode off (like F5).
    pub blank_toggle: bool,
    /// 2026-10-21: ER's action prompt ("Rest at site of grace", "Open", "Pick up": `[gamepad] prompt_indices`) is on screen.
    pub er_prompt: bool,
    /// C6: the task's chord request counter (a new value = send one two-hand chord).
    pub chord_req: u32,
    /// C6: the stance is active: R3 melee does not pulse `er_melee` (docs 3.5).
    pub melee_off: bool,
    /// C6 probe `tp_ads_er_aim`: hold ER's aim button (a held synth, not a pulse).
    pub er_aim: bool,
    /// 2026-10-24 (user: "can still shoot after death"): the task's death gate (`fp_logic::DeathGate`) holds: no melee
    /// pulse, no heal pulse (a running one is cut); the presses are not counted.
    pub dead: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepOut {
    /// Actions pressed this read (bit `1 << ACT_*`).
    pub presses: u32,
    /// Actions held (every button of the mask down) this read, whatever the mode.
    pub held: u32,
    /// Button / trigger bits hidden from ER this read.
    pub hide: u32,
    /// Button bits ER sees pressed although they are not (the interact hold).
    pub synth: u32,
    /// 2026-10-21: this read's interact was a press at ER's prompt (not a hold), for the `PAD interact tap` line.
    pub prompt_tap: bool,
    /// C6: the chord's event this read.
    pub chord: ChordEv,
    /// C6: a melee press this read was suppressed (stance active).
    pub melee_suppressed: bool,
}

fn held(levels: u32, mask: u32) -> bool {
    mask != 0 && levels & mask == mask
}

/// One read of the selected pad. Pure: the DLL loads `st` from atomics, calls this, stores `st` back and applies the
/// output to the caller's buffer.
pub fn step(st: &mut StepState, b: &Binds, i: &StepIn) -> StepOut {
    let mut out = StepOut::default();
    let lv = i.levels;
    for a in 0..N_ACT {
        if held(lv, b.masks[a]) {
            out.held |= 1 << a;
        }
    }
    let edge = |a: usize| held(lv, b.masks[a]) && !held(st.prev, b.masks[a]);
    // the toggle (chord): always counted (MW2 mode off turns it on), its buttons hidden until released
    let tmask = b.masks[ACT_TOGGLE];
    let chord_now = held(lv, tmask);
    if edge(ACT_TOGGLE) {
        out.presses |= 1 << ACT_TOGGLE;
    }
    if i.blank_toggle && chord_now {
        st.chord_hide |= tmask;
    }
    st.chord_hide &= i.levels_lo;
    // a single button that is part of the chord does not act in the read that completes the chord
    let in_chord = |a: usize| chord_now && tmask.count_ones() > 1 && b.masks[a] & tmask != 0 && b.masks[a] != tmask;
    if i.gunship {
        for a in [ACT_GS_NEXT, ACT_GS_THERMAL, ACT_GS_EXIT] {
            if edge(a) {
                out.presses |= 1 << a;
            }
        }
        st.hold_t0 = 0;
        st.hold_fired = false;
        st.melee_t0 = 0;
        st.heal_t0 = 0;
        st.interact_t0 = 0;
    } else if i.mw2 {
        for a in [ACT_FP, ACT_WEAPON_NEXT, ACT_WEAPON_PREV, ACT_JUMP, ACT_STANCE, ACT_SPRINT, ACT_MELEE, ACT_AC130, ACT_NUKE] {
            if edge(a) && !in_chord(a) {
                out.presses |= 1 << a;
            }
        }
        // review C3: melee = one ER light attack (er_melee pulsed for MELEE_PULSE_MS); C6: never while the crossbow stance
        // is active (RB on the patched bank is unknown territory: no ER attack reaches the patched weapon)
        if edge(ACT_MELEE) && !in_chord(ACT_MELEE) && b.er_melee != 0 && !i.dead {
            if i.melee_off {
                out.melee_suppressed = true;
            } else {
                st.melee_t0 = i.now_ms.max(1);
            }
        }
        if i.melee_off || i.dead {
            st.melee_t0 = 0;
        }
        // 2026-10-21: heal = one ER use-item press (er_use_item pulsed for HEAL_PULSE_MS); the heal button itself stays
        // hidden (D-pad right is ER's right-hand swap), so ER sees only its use-item button
        if edge(ACT_HEAL) && !in_chord(ACT_HEAL) && b.er_use_item != 0 && !i.dead {
            out.presses |= 1 << ACT_HEAL;
            st.heal_t0 = i.now_ms.max(1);
        }
        if i.dead {
            st.heal_t0 = 0;
        }
        // reload tap / interact hold
        let pair = b.tap_hold_pair();
        if !pair && edge(ACT_RELOAD) {
            out.presses |= 1 << ACT_RELOAD;
        }
        let im = b.masks[ACT_INTERACT];
        if im != 0 {
            if edge(ACT_INTERACT) {
                st.hold_t0 = i.now_ms.max(1);
                st.hold_fired = false;
                // 2026-10-21: ER's prompt is up: the press is ER's interact at once (ER's own tap), never a reload
                if i.er_prompt {
                    st.hold_fired = true;
                    st.interact_t0 = i.now_ms.max(1);
                    out.presses |= 1 << ACT_INTERACT;
                    out.prompt_tap = true;
                }
            }
            if held(lv, im) {
                if st.hold_t0 != 0 && !st.hold_fired && i.now_ms.saturating_sub(st.hold_t0) >= i.hold_ms as u64 {
                    st.hold_fired = true;
                    st.interact_t0 = i.now_ms.max(1);
                    out.presses |= 1 << ACT_INTERACT;
                }
            } else {
                if pair && st.hold_t0 != 0 && !st.hold_fired {
                    out.presses |= 1 << ACT_RELOAD;
                }
                st.hold_t0 = 0;
                st.hold_fired = false;
            }
        }
    } else {
        st.hold_t0 = 0;
        st.hold_fired = false;
        st.melee_t0 = 0;
        st.heal_t0 = 0;
        st.interact_t0 = 0;
    }
    if st.heal_t0 != 0 {
        if i.mw2 && !i.gunship && i.now_ms.saturating_sub(st.heal_t0) < HEAL_PULSE_MS {
            out.synth |= b.er_use_item;
        } else {
            st.heal_t0 = 0;
        }
    }
    if st.melee_t0 != 0 {
        if i.mw2 && !i.gunship && i.now_ms.saturating_sub(st.melee_t0) < MELEE_PULSE_MS {
            out.synth |= b.er_melee;
        } else {
            st.melee_t0 = 0;
        }
    }
    // ER interacts while the hold has fired and the button is still held (it sees one press, then a hold); 2026-10-21:
    // and for INTERACT_PULSE_MS after the interact however short the press (a tap at the prompt, a hold let go at
    // hold_ms), also when the prompt closes meanwhile (ER took it)
    let pulse = st.interact_t0 != 0 && i.now_ms.saturating_sub(st.interact_t0) < INTERACT_PULSE_MS;
    if !pulse {
        st.interact_t0 = 0;
    }
    if ((st.hold_fired && held(lv, b.masks[ACT_INTERACT])) || pulse) && !i.gunship {
        out.synth |= b.er_interact;
    }
    // C6 (docs 3.4): the two-hand chord. Started on a new request only outside the gunship, with no prompt, no other pulse
    // and the user not holding either button; aborted (both released together) when any of that changes; the tap is
    // only ever synthesized inside the hold
    let (ch_hold, ch_tap) = chord_buttons(b);
    // (with MW2 mode off the chord still runs: the exit chord back to the saved grip at mode off; the task never asks in a
    // menu; never in the gunship)
    let chord_block = if i.gunship {
        Some("mode")
    } else if i.er_prompt {
        Some("prompt")
    } else if out.synth != 0 || st.hold_t0 != 0 {
        Some("pulse")
    } else {
        None
    };
    if i.chord_req != st.chord_seq {
        st.chord_seq = i.chord_req;
        let user = i.levels_lo & (ch_hold | ch_tap) != 0;
        match chord_block.or(user.then_some("hold")) {
            Some(r) => out.chord = ChordEv::Refused(r),
            None => {
                st.chord_t0 = i.now_ms.max(1);
                out.chord = ChordEv::Started;
            }
        }
    }
    if st.chord_t0 != 0 {
        let el = i.now_ms.saturating_sub(st.chord_t0);
        if let Some(r) = chord_block {
            st.chord_t0 = 0;
            out.chord = ChordEv::Aborted(r);
        } else {
            let (h, t) = chord_bits(el);
            if !h {
                st.chord_t0 = 0;
                if out.chord == ChordEv::None {
                    out.chord = ChordEv::Done;
                }
            } else {
                out.synth |= ch_hold;
                if t {
                    out.synth |= ch_tap;
                }
            }
        }
    }
    // C6 probe: ER's aim button held while asked (released on every exit edge: the wish, the mode, a prompt)
    if i.er_aim && i.mw2 && !i.gunship && !i.er_prompt {
        out.synth |= ER_AIM_BUTTON;
    }
    // hidden buttons: the gunship hides everything but Start; MW2 mode its blank set; each stays hidden until released
    let want = if i.gunship {
        (XI_ALL & !XI_START) | VB_LT | VB_RT
    } else if i.mw2 {
        b.blank
    } else {
        0
    };
    let mut latched = 0u32;
    for bit in 0..18 {
        let m = 1u32 << bit;
        if latch_next(st.latched & m != 0, want & m != 0, i.levels_lo & m != 0) {
            latched |= m;
        }
    }
    st.latched = latched;
    out.hide = (latched | st.chord_hide) & !out.synth;
    st.prev = lv;
    out
}

/// Applies a step's output to the gamepad: hidden buttons cleared, hidden triggers zeroed, synthesized buttons set.
pub fn apply_buttons(raw: &mut Raw, out: &StepOut) {
    let mut b = raw.buttons as u32;
    b &= !(out.hide & XI_ALL);
    b |= out.synth & XI_ALL;
    raw.buttons = b as u16;
    if out.hide & VB_LT != 0 {
        raw.lt = 0;
    }
    if out.hide & VB_RT != 0 {
        raw.rt = 0;
    }
}

// ---- sticks ---------------------------------------------------------------------------------------------------------

/// Radial deadzone: the stick as -1..1 per axis, 0 inside `dz`, rescaled so `outer` reads 1.
pub fn radial(x: i16, y: i16, dz: f32, outer: f32) -> (f32, f32) {
    let vx = (x as f32 / 32767.0).clamp(-1.0, 1.0);
    let vy = (y as f32 / 32767.0).clamp(-1.0, 1.0);
    let m = (vx * vx + vy * vy).sqrt();
    if !(m > dz) || !(outer > dz) {
        return (0.0, 0.0);
    }
    let s = ((m - dz) / (outer - dz)).min(1.0);
    let k = s / m;
    (vx * k, vy * k)
}

/// MW2's `view_input_N.graph` curves (dumps\m5\common_mp\aim_assist; a test compares them with the files).
pub const GRAPH_SOURCE: &str = r"dumps\m5\common_mp\aim_assist\view_input_N.graph";
pub const GRAPH_0: [(f32, f32); 15] = [
    (0.0, 0.0),
    (0.1218, 0.0171),
    (0.1972, 0.0295),
    (0.2612, 0.0451),
    (0.3244, 0.0644),
    (0.3872, 0.0897),
    (0.4543, 0.1281),
    (0.5149, 0.1729),
    (0.5723, 0.2203),
    (0.6263, 0.2771),
    (0.6771, 0.3483),
    (0.7288, 0.4296),
    (0.7781, 0.5157),
    (0.8387, 0.6301),
    (1.0, 1.0),
];
pub const GRAPH_1: [(f32, f32); 13] = [
    (0.0, 0.0),
    (0.1323, 0.0235),
    (0.2102, 0.0469),
    (0.2939, 0.0778),
    (0.3741, 0.1170),
    (0.4555, 0.1724),
    (0.5401, 0.2400),
    (0.6118, 0.3149),
    (0.6726, 0.4018),
    (0.7268, 0.4906),
    (0.7797, 0.5833),
    (0.8386, 0.6926),
    (1.0, 1.0),
];
pub const GRAPH_2: [(f32, f32); 12] = [
    (0.0, 0.0),
    (0.1322, 0.0396),
    (0.2338, 0.0780),
    (0.3235, 0.1267),
    (0.4022, 0.1812),
    (0.4809, 0.2479),
    (0.5561, 0.3254),
    (0.6205, 0.3936),
    (0.6930, 0.4771),
    (0.7819, 0.5943),
    (0.8598, 0.7299),
    (1.0, 1.0),
];
pub const GRAPH_3: [(f32, f32); 7] = [(0.0, 0.0), (0.1242, 0.0423), (0.2238, 0.0885), (0.3210, 0.1553), (0.4118, 0.2419), (0.5116, 0.3508), (1.0, 1.0)];

pub fn graph(index: u8) -> &'static [(f32, f32)] {
    match index {
        0 => &GRAPH_0,
        1 => &GRAPH_1,
        2 => &GRAPH_2,
        _ => &GRAPH_3,
    }
}

/// Linear interpolation between the graph points (input clamped to 0..1).
pub fn graph_eval(g: &[(f32, f32)], x: f32) -> f32 {
    let x = if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    for w in g.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 1.0 };
            return y0 + (y1 - y0) * t;
        }
    }
    g.last().map_or(x, |p| p.1)
}

/// Graph applied per axis, sign kept.
pub fn shaped(v: f32, g: &[(f32, f32)]) -> f32 {
    graph_eval(g, v.abs()).copysign(v)
}

/// The look parameters (`[gamepad]`, mirrored into atomics for the detours).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    pub yaw: f32,
    pub pitch: f32,
    pub yaw_ads: f32,
    pub pitch_ads: f32,
    pub sens: f32,
    pub invert_y: bool,
    pub accel: bool,
    pub accel_lerp: f32,
    pub accel_max_yaw: f32,
    pub dz: f32,
    pub outer: f32,
    pub graph: u8,
}

impl Default for LookParams {
    fn default() -> Self {
        LookParams {
            yaw: 260.0,
            pitch: 90.0,
            yaw_ads: 90.0,
            pitch_ads: 55.0,
            sens: 1.0,
            invert_y: false,
            accel: true,
            accel_lerp: 1200.0,
            accel_max_yaw: 400.0,
            dz: 0.18,
            outer: 0.98,
            graph: 3,
        }
    }
}

/// The accel ramp's state: the extra yaw rate (deg/s) on top of the base rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookState {
    pub extra: f32,
}

/// Right stick -> (yaw, pitch) in deg/s: yaw > 0 turns right, pitch > 0 looks up (`invert_y` flips it). `ads` is the
/// ADS fraction (0 hip .. 1 aimed: the rates blend). Accel [I]: while the shaped yaw is at full deflection (>= 0.99)
/// the extra rate ramps toward `accel_max_yaw` at `accel_lerp` deg/s^2 (hip only), and decays at the same rate.
pub fn look_rates(st: &mut LookState, p: &LookParams, rx: i16, ry: i16, ads: f32, dt: f32) -> (f32, f32) {
    let ads = if ads.is_finite() { ads.clamp(0.0, 1.0) } else { 0.0 };
    let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
    let (nx, ny) = radial(rx, ry, p.dz, p.outer);
    let g = graph(p.graph);
    let (gx, gy) = (shaped(nx, g), shaped(ny, g));
    let lerp = |a: f32, b: f32| a + (b - a) * ads;
    let base_yaw = lerp(p.yaw, p.yaw_ads) * p.sens;
    let base_pitch = lerp(p.pitch, p.pitch_ads) * p.sens;
    let cap = ((p.accel_max_yaw * p.sens - p.yaw * p.sens).max(0.0)) * (1.0 - ads);
    if p.accel && gx.abs() >= 0.99 {
        st.extra = (st.extra + p.accel_lerp * dt).min(cap);
    } else {
        st.extra = (st.extra - p.accel_lerp * dt).max(0.0).min(cap);
    }
    let yaw = gx * (base_yaw + st.extra);
    let pitch = gy * base_pitch * if p.invert_y { -1.0 } else { 1.0 };
    (yaw, pitch)
}

/// Degrees -> whole counts at `deg_per_count`, the residual carried in 1/1024 counts (slow turns are never lost; the
/// sum over many frames is exact to 1/1024 of a count per frame).
pub fn to_counts(deg: f32, deg_per_count: f32, carry: &mut i32) -> i32 {
    if !(deg_per_count > 0.0) || !deg.is_finite() {
        return 0;
    }
    let q = ((deg / deg_per_count) as f64 * 1024.0).round().clamp(-1e12, 1e12) as i64;
    let total = q + *carry as i64;
    let out = total.div_euclid(1024);
    *carry = (total - out * 1024) as i32;
    out.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Mouse counts of one look step: x = yaw (+ right), y = -pitch (DirectInput's +Y moves the view down) [I].
pub fn look_counts(yaw_dps: f32, pitch_dps: f32, dt: f32, deg_per_count: f32, carry: &mut [i32; 2]) -> (i32, i32) {
    let dx = to_counts(yaw_dps * dt, deg_per_count, &mut carry[0]);
    let dy = to_counts(-pitch_dps * dt, deg_per_count, &mut carry[1]);
    (dx, dy)
}

/// Route `er` (or the fallback): ER keeps the right stick, reshaped by the radial deadzone and the MW2 graph [I: ER's
/// own response curve on top is unknown].
pub fn reshape_for_er(rx: i16, ry: i16, p: &LookParams) -> (i16, i16) {
    let (nx, ny) = radial(rx, ry, p.dz, p.outer);
    let g = graph(p.graph);
    let s = |v: f32| (shaped(v, g) * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    (s(nx), s(ny))
}

/// Left stick -> the analog usercmd (forwardmove, rightmove), -127..127, or `None` inside the deadzone. The circle is
/// stretched to the square (a full diagonal is full speed, as with two keys; pmove scales by max(|fm|,|rm|)/127).
pub fn analog_cmd(lx: i16, ly: i16, dz: f32, outer: f32) -> Option<(f32, f32)> {
    let (nx, ny) = radial(lx, ly, dz, outer);
    let m = (nx * nx + ny * ny).sqrt();
    if m <= 0.0 {
        return None;
    }
    let k = m / nx.abs().max(ny.abs());
    let q = |v: f32| (v * k * 127.0).round().clamp(-127.0, 127.0);
    Some((q(ny), q(nx)))
}

/// The left stick as W/A/S/D bits for the keyboard-level checks (wish, yaw gate, interaction holds): an axis counts
/// once its share of the shaped vector is above 0.38 (about 22.5 degrees off the other axis).
pub fn stick_move_bits(lx: i16, ly: i16, dz: f32, outer: f32) -> u8 {
    let (nx, ny) = radial(lx, ly, dz, outer);
    let m = (nx * nx + ny * ny).sqrt();
    if m <= 0.0 {
        return 0;
    }
    let mut b = 0u8;
    if ny / m > 0.38 {
        b |= 1; // W
    }
    if nx / m < -0.38 {
        b |= 2; // A
    }
    if ny / m < -0.38 {
        b |= 4; // S
    }
    if nx / m > 0.38 {
        b |= 8; // D
    }
    b
}

/// The press-mode sprint gate of pmove (forwardmove > 105).
pub const SPRINT_FORWARDMOVE: f32 = 105.0;

pub fn sprint_allowed(fm: f32) -> bool {
    fm > SPRINT_FORWARDMOVE
}

/// The press edge helper re-exported for the detour.
pub fn edge(prev: bool, now: bool) -> bool {
    press_edge(prev, now)
}

/// Running median of ER's degrees per mouse count (the `PAD calib` estimate): ratio samples of |camera yaw change| /
/// |counts delivered| over ticks with enough counts.
#[derive(Clone, Debug, Default)]
pub struct Calib {
    samples: Vec<f32>,
    next: usize,
}

pub const CALIB_N: usize = 31;
pub const CALIB_MIN_N: usize = 15;
pub const CALIB_MIN_COUNTS: i64 = 20;

impl Calib {
    /// Adds a tick (counts delivered to ER, ER's yaw change in degrees); ignored below `CALIB_MIN_COUNTS`, above 90
    /// degrees (a cut / respawn) or without a turn. Only magnitudes are compared (ER's heading sign convention is not
    /// verified); the median drops ticks where ER turned on its own (lock-on, auto rotation).
    pub fn add(&mut self, counts: i64, dyaw_deg: f32) -> bool {
        if counts.abs() < CALIB_MIN_COUNTS || !dyaw_deg.is_finite() || dyaw_deg.abs() > 90.0 || dyaw_deg == 0.0 {
            return false;
        }
        let r = dyaw_deg.abs() / counts.abs() as f32;
        if !(r > 1e-5 && r < 5.0) {
            return false;
        }
        if self.samples.len() < CALIB_N {
            self.samples.push(r);
        } else {
            self.samples[self.next] = r;
        }
        self.next = (self.next + 1) % CALIB_N;
        true
    }

    pub fn n(&self) -> usize {
        self.samples.len()
    }

    /// The median once `CALIB_MIN_N` samples are in.
    pub fn estimate(&self) -> Option<f32> {
        if self.samples.len() < CALIB_MIN_N {
            return None;
        }
        let mut v = self.samples.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }
}

/// Signed heading change (degrees, -180..180) between two forward vectors given as (x, z) [I: which sign is "right"
/// in ER's camera rows is not verified; the calibration only uses the magnitude].
pub fn yaw_delta_deg(prev: (f32, f32), now: (f32, f32)) -> f32 {
    let a0 = prev.0.atan2(prev.1);
    let a1 = now.0.atan2(now.1);
    let mut d = (a1 - a0).to_degrees();
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Binds {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        Binds::resolve(
            &[
                ("toggle", "l3+r3"),
                ("fp_toggle", "dpad_left"),
                ("fire", "rt"),
                ("ads", "lt"),
                ("reload", "x"),
                ("interact_hold", "x"),
                ("weapon_next", "y"),
                ("weapon_prev", "lb"),
                ("jump", "a"),
                ("stance", "b"),
                ("sprint", "l3"),
                ("melee", "r3"),
                ("ac130", "dpad_up"),
                ("nuke", "dpad_down"),
                ("heal", "dpad_right"),
            ],
            &[("fire", "rt"), ("next", "y"), ("thermal", "x"), ("exit", "back")],
            &s(&["a", "b", "x", "y", "lb", "rb", "lt", "rt", "l3", "r3", "dpad_up", "dpad_down", "dpad_left", "dpad_right", "rs", "ls_fp"]),
            "y",
        )
        .unwrap()
        .with_er_melee("rb")
        .unwrap()
        .with_er_use_item("x")
        .unwrap()
    }

    /// 2026-10-21 (user: "a way to use heal pots while in MW2 mode"): D-pad right = one ER use-item press (X pulsed
    /// HEAL_PULSE_MS) while MW2 mode drives; D-pad right itself stays hidden (no right-hand swap), the MW2 gun does not
    /// reload (X is synthesized, not read), nothing in the AC-130 or with MW2 mode off / a menu (`mw2` false).
    #[test]
    fn heal_pad_dpad_right_pulses_er_use_item() {
        let b = defaults();
        assert_eq!(b.masks[ACT_HEAL], XI_DPAD_RIGHT);
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 1000, true));
        assert_eq!(o.presses, 1 << ACT_HEAL);
        assert_eq!(o.synth, XI_X, "ER sees its use-item button");
        assert_eq!(o.hide & XI_DPAD_RIGHT, XI_DPAD_RIGHT, "no right-hand swap");
        assert_eq!(o.hide & XI_X, 0);
        let mut raw = Raw { buttons: XI_DPAD_RIGHT as u16, ..Raw::default() };
        apply_buttons(&mut raw, &o);
        assert_eq!(raw.buttons as u32, XI_X);
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 1000 + HEAL_PULSE_MS - 1, true));
        assert_eq!((o.presses, o.synth), (0, XI_X), "held: still the one press");
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 1000 + HEAL_PULSE_MS, true));
        assert_eq!(o.synth, 0, "a held D-pad right is one drink, not a held X");
        // release: no reload (the synthesized X never reaches our levels), a second press drinks again
        let o = step(&mut st, &b, &sin(0, 1400, true));
        assert_eq!((o.presses, o.synth), (0, 0));
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 1500, true));
        assert_eq!((o.presses, o.synth), (1 << ACT_HEAL, XI_X));
        // the pulse ends when MW2 mode stops driving (a menu opened mid-pulse)
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 1510, false));
        assert_eq!(o.synth, 0);
        // MW2 mode off / a menu: D-pad right is ER's own (nothing synthesized, nothing counted)
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT, 0, false));
        assert_eq!((o.presses, o.synth, o.hide), (0, 0, 0));
        // the AC-130: everything but Start hidden, no heal
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { gunship: true, ..sin(XI_DPAD_RIGHT, 0, false) });
        assert_eq!((o.presses & (1 << ACT_HEAL), o.synth), (0, 0));
        assert_eq!(o.hide & XI_X, XI_X);
        // a heal pulse that started in MW2 mode stops at once when the ride starts
        let mut st = StepState::default();
        step(&mut st, &b, &sin(XI_DPAD_RIGHT, 0, true));
        let o = step(&mut st, &b, &StepIn { gunship: true, ..sin(XI_DPAD_RIGHT, 16, false) });
        assert_eq!(o.synth, 0);
        // X held for an interact while healing: the synthesized X wins over the hidden X for the pulse only
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT | XI_X, 0, true));
        assert_eq!(o.synth & XI_X, XI_X);
        let o = step(&mut st, &b, &sin(XI_X, 200, true));
        assert_eq!(o.synth & XI_X, 0);
        assert_eq!(o.hide & XI_X, XI_X);
        // er_use_item "" = heal off; heal may not share a button with another MW2 action
        let off = defaults().with_er_use_item("").unwrap();
        let mut st = StepState::default();
        assert_eq!(step(&mut st, &off, &sin(XI_DPAD_RIGHT, 0, true)).synth, 0);
        assert!(Binds::default().with_er_use_item("rt").is_err());
        assert!(Binds::default().with_er_use_item("x+y").is_err());
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(Binds::resolve(&[("heal", "dpad_down"), ("nuke", "dpad_down")], &[], &s(&[]), "y").is_err());
    }

    /// Review C3: R3 = one ER light attack (RB pulsed 120 ms, then hidden again); nothing with MW2 mode off.
    #[test]
    fn c3_pad_melee_pulses_er_rb() {
        let b = defaults();
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_R3, 1000, true));
        assert_eq!(o.synth & XI_RB, XI_RB, "pulse on the press");
        assert_eq!(o.hide & XI_R3, XI_R3, "R3 itself stays hidden from ER");
        let o = step(&mut st, &b, &sin(XI_R3, 1100, true));
        assert_eq!(o.synth & XI_RB, XI_RB);
        let o = step(&mut st, &b, &sin(XI_R3, 1130, true));
        assert_eq!(o.synth & XI_RB, 0, "held R3 is one attack, not a held RB");
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_R3, 1000, false));
        assert_eq!(o.synth, 0, "MW2 mode off: ER's own R3");
        assert!(Binds::default().with_er_melee("").is_ok());
        assert!(Binds::default().with_er_melee("start").is_err());
        assert!(Binds::default().with_er_melee("rt").is_err());
    }

    fn sin(levels: u32, now_ms: u64, mw2: bool) -> StepIn {
        StepIn {
            levels,
            levels_lo: levels,
            now_ms,
            mw2,
            gunship: false,
            hold_ms: 300,
            blank_toggle: true,
            er_prompt: false,
            chord_req: 0,
            melee_off: false,
            er_aim: false,
            dead: false,
        }
    }

    #[test]
    fn c3_pad_deadzone_edges() {
        let (dz, outer) = (0.18, 0.98);
        let v = |f: f32| (f * 32767.0) as i16;
        assert_eq!(radial(v(0.17), 0, dz, outer), (0.0, 0.0), "inside the deadzone");
        assert_eq!(radial(v(0.18) - 1, 0, dz, outer), (0.0, 0.0));
        let (x, _) = radial(v(0.19), 0, dz, outer);
        assert!(x > 0.0 && x < 0.02, "just outside rescales from 0: {x}");
        assert_eq!(radial(v(0.981), 0, dz, outer).0, 1.0, "outer reads 1");
        assert!(radial(v(0.97), 0, dz, outer).0 < 1.0);
        assert_eq!(radial(32767, 0, dz, outer).0, 1.0);
        assert_eq!(radial(-32768, 0, dz, outer).0, -1.0);
        // radial, not per axis: a diagonal just past the deadzone is live on both axes
        let (x, y) = radial(v(0.14), v(0.14), dz, outer);
        assert!(x > 0.0 && y > 0.0);
        assert_eq!(analog_cmd(v(0.1), v(0.1), 0.20, 0.98), None);
    }

    #[test]
    fn c3_pad_graph3_points() {
        for (x, y) in GRAPH_3 {
            assert!((graph_eval(&GRAPH_3, x) - y).abs() < 1e-6, "{x} -> {y}");
        }
        assert!((graph_eval(&GRAPH_3, 0.75) - (0.3508 + (1.0 - 0.3508) * (0.75 - 0.5116) / (1.0 - 0.5116))).abs() < 1e-5);
        assert_eq!(graph_eval(&GRAPH_3, 2.0), 1.0);
        assert_eq!(graph_eval(&GRAPH_3, -1.0), 0.0);
        assert_eq!(shaped(-1.0, &GRAPH_3), -1.0);
        assert_eq!(graph(3), &GRAPH_3[..]);
        assert_eq!(LookParams::default().graph, 3);
    }

    #[test]
    fn c3_pad_graphs_equal_the_dumped_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("dumps").join("m5").join("common_mp").join("aim_assist");
        // a checkout without the local MW2 dumps (the public source) has nothing to compare against
        if !root.is_dir() {
            eprintln!("skipped: {} missing (local MW2 dumps)", root.display());
            return;
        }
        for i in 0..4u8 {
            let p = root.join(format!("view_input_{i}.graph"));
            let Ok(t) = std::fs::read_to_string(&p) else { panic!("{} missing", p.display()) };
            let mut lines = t.lines().map(str::trim).filter(|l| !l.is_empty());
            assert_eq!(lines.next(), Some("GRAPH_FLOAT_FILE"));
            let n: usize = lines.next().unwrap().parse().unwrap();
            let pts: Vec<(f32, f32)> = lines
                .map(|l| {
                    let mut it = l.split_whitespace().map(|x| x.parse::<f32>().unwrap());
                    (it.next().unwrap(), it.next().unwrap())
                })
                .collect();
            assert_eq!(pts.len(), n);
            assert_eq!(graph(i), &pts[..], "graph {i}");
        }
    }

    #[test]
    fn c3_pad_rates_mw2() {
        let p = LookParams { accel: false, ..LookParams::default() };
        let mut st = LookState::default();
        assert_eq!(look_rates(&mut st, &p, 32767, 0, 0.0, 0.016), (260.0, 0.0));
        assert_eq!(look_rates(&mut st, &p, 0, 32767, 0.0, 0.016), (0.0, 90.0));
        assert_eq!(look_rates(&mut st, &p, 32767, 0, 1.0, 0.016), (90.0, 0.0));
        assert_eq!(look_rates(&mut st, &p, 0, 32767, 1.0, 0.016), (0.0, 55.0));
        assert_eq!(look_rates(&mut st, &p, -32768, -32768, 0.0, 0.016).0.signum(), -1.0);
        let inv = LookParams { invert_y: true, ..p };
        assert_eq!(look_rates(&mut st, &inv, 0, 32767, 0.0, 0.016), (0.0, -90.0));
        let half = LookParams { sens: 0.5, ..p };
        assert_eq!(look_rates(&mut st, &half, 32767, 0, 0.0, 0.016).0, 130.0);
        // the graph shapes a half deflection (0.5 past the deadzone rescale)
        let (y, _) = look_rates(&mut st, &p, (0.59 * 32767.0) as i16, 0, 0.0, 0.016);
        assert!(y > 0.0 && y < 130.0, "graph 3 is below linear at mid deflection: {y}");
    }

    #[test]
    fn c3_pad_accel_ramp_and_decay() {
        let p = LookParams::default();
        let mut st = LookState::default();
        let dt = 0.01;
        let mut last = 0.0;
        for k in 0..100 {
            let (y, _) = look_rates(&mut st, &p, 32767, 0, 0.0, dt);
            assert!(y >= last, "ramps up");
            last = y;
            if k == 0 {
                assert!((y - (260.0 + 12.0)).abs() < 1e-3, "{y}");
            }
        }
        assert!((last - 400.0).abs() < 1e-3, "capped at accel_max_yaw: {last}");
        // released: decays at accel_lerp
        let _ = look_rates(&mut st, &p, 0, 0, 0.0, 0.05);
        assert!((st.extra - 80.0).abs() < 1e-3, "{}", st.extra);
        for _ in 0..10 {
            let _ = look_rates(&mut st, &p, 0, 0, 0.0, 0.05);
        }
        assert_eq!(st.extra, 0.0);
        // ADS: no accel
        let mut st = LookState::default();
        for _ in 0..100 {
            let _ = look_rates(&mut st, &p, 32767, 0, 1.0, dt);
        }
        assert_eq!(look_rates(&mut st, &p, 32767, 0, 1.0, dt).0, 90.0);
        // off
        let mut st = LookState::default();
        let off = LookParams { accel: false, ..p };
        for _ in 0..100 {
            let _ = look_rates(&mut st, &off, 32767, 0, 0.0, dt);
        }
        assert_eq!(look_rates(&mut st, &off, 32767, 0, 0.0, dt).0, 260.0);
    }

    #[test]
    fn c3_pad_counts_carry_exact() {
        let mut carry = [0i32; 2];
        let (mut sx, mut sy) = (0i64, 0i64);
        let dpc = 0.022;
        for _ in 0..1000 {
            let (x, y) = look_counts(10.0, -5.0, 0.016_667, dpc, &mut carry);
            sx += x as i64;
            sy += y as i64;
        }
        let q = |deg: f32| ((deg / dpc) as f64 * 1024.0).round() as i64;
        assert_eq!(sx * 1024 + carry[0] as i64, 1000 * q(10.0 * 0.016_667));
        assert_eq!(sy * 1024 + carry[1] as i64, 1000 * q(5.0 * 0.016_667));
        assert!(sy > 0, "pitch down (negative pitch rate) is +Y counts");
        // a tiny turn is never lost
        let mut c = 0;
        let mut s = 0;
        for _ in 0..1000 {
            s += to_counts(0.001, 0.022, &mut c);
        }
        assert_eq!(s, (1000.0f64 * 0.001 / 0.022) as i32);
        assert_eq!(to_counts(1.0, 0.0, &mut c), 0, "no scale: nothing");
    }

    #[test]
    fn c3_pad_analog_cmd() {
        assert_eq!(analog_cmd(0, 32767, 0.20, 0.98), Some((127.0, 0.0)), "full forward");
        assert_eq!(analog_cmd(0, -32768, 0.20, 0.98), Some((-127.0, 0.0)));
        assert_eq!(analog_cmd(32767, 0, 0.20, 0.98), Some((0.0, 127.0)), "full right");
        let d = (0.7071 * 32767.0) as i16;
        assert_eq!(analog_cmd(d, d, 0.20, 0.98), Some((127.0, 127.0)), "a full diagonal is full speed");
        let (fm, _) = analog_cmd(0, (0.6 * 32767.0) as i16, 0.20, 0.98).unwrap();
        assert!(fm > 0.0 && fm < 105.0);
        assert!(!sprint_allowed(fm), "half stick never sprints");
        assert!(sprint_allowed(127.0));
        assert!(!sprint_allowed(105.0), "sprint needs forwardmove > 105");
        // 30 degrees off forward at full deflection: fm 127 (square) still sprints, 60 degrees does not
        let a = 30f32.to_radians();
        let (fm, rm) = analog_cmd((a.sin() * 32767.0) as i16, (a.cos() * 32767.0) as i16, 0.20, 0.98).unwrap();
        assert!(fm == 127.0 && rm > 0.0 && rm < 127.0);
        let a = 60f32.to_radians();
        let (fm, _) = analog_cmd((a.sin() * 32767.0) as i16, (a.cos() * 32767.0) as i16, 0.20, 0.98).unwrap();
        assert!(!sprint_allowed(fm));
        assert_eq!(stick_move_bits(0, 32767, 0.2, 0.98), 1);
        assert_eq!(stick_move_bits(-32768, 0, 0.2, 0.98), 2);
        assert_eq!(stick_move_bits(d, d, 0.2, 0.98), 1 | 8);
        assert_eq!(stick_move_bits(1000, 1000, 0.2, 0.98), 0);
    }

    #[test]
    fn c3_pad_latch_rt_held_across_mode_off() {
        let b = defaults();
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(VB_RT, 0, true));
        assert!(o.hide & VB_RT != 0 && o.held & (1 << ACT_FIRE) != 0);
        // MW2 mode off (or a menu) with RT still held: still hidden
        let o = step(&mut st, &b, &sin(VB_RT, 16, false));
        assert!(o.hide & VB_RT != 0, "held RT stays hidden");
        let o = step(&mut st, &b, &sin(0, 32, false));
        assert_eq!(o.hide & VB_RT, 0);
        let o = step(&mut st, &b, &sin(VB_RT, 48, false));
        assert_eq!(o.hide & VB_RT, 0, "a new press reaches ER");
        // a trigger resting just above our fire threshold's release level keeps the latch
        let mut st = StepState::default();
        step(&mut st, &b, &sin(VB_RT, 0, true));
        let o = step(&mut st, &b, &StepIn { levels: 0, levels_lo: VB_RT, ..sin(0, 16, false) });
        assert!(o.hide & VB_RT != 0);
        let mut raw = Raw { buttons: XI_A as u16, rt: 200, lt: 100, ..Raw::default() };
        apply_buttons(&mut raw, &StepOut { hide: VB_RT | XI_A, ..StepOut::default() });
        assert_eq!((raw.buttons, raw.rt, raw.lt), (0, 0, 100));
    }

    #[test]
    fn c3_pad_chord_l3_r3_same_read_hides_both() {
        let b = defaults();
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_L3 | XI_R3, 0, false));
        assert_eq!(o.presses, 1 << ACT_TOGGLE, "toggle with MW2 mode off");
        assert_eq!(o.hide & (XI_L3 | XI_R3), XI_L3 | XI_R3);
        // in MW2 mode the chord neither sprints nor melees
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_L3 | XI_R3, 0, true));
        assert_eq!(o.presses, 1 << ACT_TOGGLE);
        // held: no second toggle; released one by one: hidden until each is up
        let o = step(&mut st, &b, &sin(XI_L3 | XI_R3, 16, false));
        assert_eq!(o.presses, 0);
        let o = step(&mut st, &b, &sin(XI_R3, 32, false));
        assert_eq!(o.hide & (XI_L3 | XI_R3), XI_R3);
        let o = step(&mut st, &b, &sin(0, 48, false));
        assert_eq!(o.hide, 0);
        // L3 alone in MW2 mode is sprint
        let o = step(&mut st, &b, &sin(XI_L3, 64, true));
        assert_eq!(o.presses, 1 << ACT_SPRINT);
        // blank_toggle off: the chord reaches ER with MW2 mode off
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { blank_toggle: false, ..sin(XI_L3 | XI_R3, 0, false) });
        assert_eq!((o.presses, o.hide), (1 << ACT_TOGGLE, 0));
    }

    #[test]
    fn c3_pad_x_tap_vs_hold_synth_interact() {
        let b = defaults();
        // tap: reload on release, no interact
        let mut st = StepState::default();
        assert_eq!(step(&mut st, &b, &sin(XI_X, 1000, true)).presses, 0);
        assert_eq!(step(&mut st, &b, &sin(XI_X, 1200, true)).presses, 0);
        let o = step(&mut st, &b, &sin(0, 1250, true));
        assert_eq!(o.presses, 1 << ACT_RELOAD);
        assert_eq!(o.synth, 0);
        // hold 300 ms: interact once, ER sees its interact button (Y) while X stays held, no reload on release
        let mut st = StepState::default();
        step(&mut st, &b, &sin(XI_X, 1000, true));
        let o = step(&mut st, &b, &sin(XI_X, 1299, true));
        assert_eq!((o.presses, o.synth), (0, 0));
        let o = step(&mut st, &b, &sin(XI_X, 1300, true));
        assert_eq!(o.presses, 1 << ACT_INTERACT);
        assert_eq!(o.synth, XI_Y);
        assert_eq!(o.hide & XI_Y, 0, "the synthesized button is not hidden");
        assert!(o.hide & XI_X != 0, "X itself stays hidden");
        let o = step(&mut st, &b, &sin(XI_X, 1600, true));
        assert_eq!((o.presses, o.synth), (0, XI_Y));
        let o = step(&mut st, &b, &sin(0, 1700, true));
        assert_eq!((o.presses, o.synth), (0, 0), "no reload after a hold");
        let mut raw = Raw { buttons: XI_X as u16, ..Raw::default() };
        apply_buttons(&mut raw, &StepOut { hide: XI_X, synth: XI_Y, ..StepOut::default() });
        assert_eq!(raw.buttons as u32, XI_Y);
        // with MW2 mode off X is ER's (no timing, nothing hidden)
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_X, 0, false));
        let o2 = step(&mut st, &b, &sin(XI_X, 500, false));
        assert_eq!((o.presses | o2.presses, o.hide | o2.hide, o2.synth), (0, 0, 0));
    }

    /// 2026-10-21 (user: "pressing E/use with controller is buggy", 12:54-13:12 session): X taps at ER's prompt (idx 19
    /// visible) were MW2 reloads (`RELOAD ignored (full)` x14, 13:02:47.965 .. 13:07:36.578) and never reached ER; only a
    /// later >= 300 ms hold interacted. ER is polled at 60 Hz (`PAD stat calls` +600 / 10 s): a 17 ms cadence here.
    #[test]
    fn pad_x_tap_at_er_prompt_interacts_once() {
        let b = defaults();
        let poll = |st: &mut StepState, lv: u32, t: u64, prompt: bool| step(st, &b, &StepIn { er_prompt: prompt, ..sin(lv, t, true) });
        // the logged taps: X down 4 polls (~67 ms) at the prompt: ER sees Y from the press, for the whole pulse, once
        let mut st = StepState::default();
        let mut seen = 0;
        let mut presses = 0u32;
        let mut edges = 0;
        let mut prev_y = false;
        for k in 0..20u64 {
            let lv = if k < 4 { XI_X } else { 0 };
            let o = poll(&mut st, lv, 1000 + k * 17, true);
            presses |= o.presses;
            if k == 0 {
                assert_eq!(o.presses, 1 << ACT_INTERACT, "the press interacts at once");
                assert!(o.prompt_tap);
            }
            let y = o.synth & XI_Y != 0;
            seen += y as u32;
            edges += (y && !prev_y) as u32;
            prev_y = y;
            assert!(k >= 4 || o.hide & XI_X != 0, "X itself stays hidden");
        }
        assert_eq!(presses, 1 << ACT_INTERACT, "no reload, no weapon switch (Y is synthesized, never read)");
        assert_eq!(edges, 1, "one press edge for ER");
        assert!(seen as u64 * 17 >= INTERACT_PULSE_MS, "ER sees Y for at least the pulse ({seen} polls)");
        // mashing: a second tap after the pulse is a second interact (ER gets a release, then a new press)
        let o = poll(&mut st, XI_X, 2000, true);
        assert_eq!((o.presses, o.synth & XI_Y), (1 << ACT_INTERACT, XI_Y));
        // held at the prompt: one interact, Y held while X is (ER's hold), no second press at hold_ms, no reload
        let mut st = StepState::default();
        let mut presses = 0u32;
        for k in 0..40u64 {
            let o = poll(&mut st, XI_X, 1000 + k * 17, true);
            presses |= o.presses;
            assert_eq!(o.synth & XI_Y, XI_Y);
        }
        let o = poll(&mut st, 0, 1000 + 40 * 17, true);
        assert_eq!((presses, o.presses), (1 << ACT_INTERACT, 0));
        // the prompt closes as ER takes the interact (idx 19 1->0 0.1-0.8 s later): the pulse is not cut
        let mut st = StepState::default();
        poll(&mut st, XI_X, 1000, true);
        let o = poll(&mut st, 0, 1017, false);
        assert_eq!((o.presses, o.synth & XI_Y), (0, XI_Y));
        // no prompt: the MW2 tap = reload stays (and ER sees nothing)
        let mut st = StepState::default();
        let mut presses = 0u32;
        let mut synth = 0u32;
        for k in 0..10u64 {
            let o = poll(&mut st, if k < 4 { XI_X } else { 0 }, 1000 + k * 17, false);
            presses |= o.presses;
            synth |= o.synth;
        }
        assert_eq!((presses, synth), (1 << ACT_RELOAD, 0));
        // no prompt, a hold released right after hold_ms (one poll of Y before): ER still sees the whole pulse
        let mut st = StepState::default();
        let mut seen = 0;
        for k in 0..40u64 {
            let t = 1000 + k * 17;
            let o = poll(&mut st, if t <= 1310 { XI_X } else { 0 }, t, false);
            assert_eq!(o.presses & (1 << ACT_RELOAD), 0);
            seen += (o.synth & XI_Y != 0) as u32;
        }
        assert!(seen as u64 * 17 >= INTERACT_PULSE_MS, "{seen} polls");
        // MW2 mode off (a menu) / the AC-130: the prompt changes nothing
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { er_prompt: true, ..sin(XI_X, 1000, false) });
        assert_eq!((o.presses, o.synth, o.hide), (0, 0, 0));
        let o = step(&mut st, &b, &StepIn { er_prompt: true, gunship: true, ..sin(XI_X, 1100, false) });
        assert_eq!((o.presses & (1 << ACT_INTERACT), o.synth), (0, 0));
    }

    #[test]
    fn c3_pad_gunship_blanks_all_but_start() {
        let b = defaults();
        let mut st = StepState::default();
        let all = XI_ALL | VB_LT | VB_RT;
        let o = step(&mut st, &b, &StepIn { gunship: true, ..sin(all, 0, false) });
        assert_eq!(o.hide, all & !XI_START);
        assert_eq!(o.presses, (1 << ACT_TOGGLE) | (1 << ACT_GS_NEXT) | (1 << ACT_GS_THERMAL) | (1 << ACT_GS_EXIT));
        assert!(o.held & (1 << ACT_GS_FIRE) != 0);
        let mut raw = Raw { buttons: 0xF3FF, lt: 255, rt: 255, ..Raw::default() };
        apply_buttons(&mut raw, &o);
        assert_eq!((raw.buttons as u32, raw.lt, raw.rt), (XI_START, 0, 0));
        // MW2 mode: Start and Back reach ER; review C3: D-pad right is hidden (ER's right-hand swap under the MW2 gun)
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_DPAD_RIGHT | XI_START | XI_BACK | XI_A, 0, true));
        assert_eq!(o.hide & (XI_DPAD_RIGHT | XI_START | XI_BACK | XI_A), XI_A | XI_DPAD_RIGHT);
        // 2026-10-21: D-pad right is the heal (ER's use-item X pulsed), never ER's swap
        assert_eq!(o.presses, (1 << ACT_JUMP) | (1 << ACT_HEAL));
        assert_eq!(o.synth, XI_X);
    }

    #[test]
    fn c3_pad_edges_once_and_blank_set() {
        let b = defaults();
        assert!(b.blank_rs && b.blank_ls);
        assert_eq!(b.blank & XI_START, 0);
        assert_eq!(b.blank & XI_DPAD_RIGHT, XI_DPAD_RIGHT, "review C3: hidden in MW2 mode");
        let mut st = StepState::default();
        let mut n = 0;
        for (k, lv) in [XI_Y, XI_Y, 0, XI_Y, 0, XI_LB].iter().enumerate() {
            let o = step(&mut st, &b, &sin(*lv, k as u64 * 16, true));
            n += (o.presses >> ACT_WEAPON_NEXT & 1) + (o.presses >> ACT_WEAPON_PREV & 1);
        }
        assert_eq!(n, 3);
        let o = step(&mut st, &b, &sin(XI_B | XI_DPAD_UP, 200, true));
        assert_eq!(o.presses, (1 << ACT_STANCE) | (1 << ACT_AC130));
        let o = step(&mut st, &b, &sin(XI_DPAD_DOWN | XI_DPAD_LEFT, 216, true));
        assert_eq!(o.presses, (1 << ACT_NUKE) | (1 << ACT_FP));
    }

    #[test]
    fn c3_pad_raw_bytes_roundtrip_and_reshape() {
        let r = Raw { buttons: 0x9001, lt: 7, rt: 250, lx: -32768, ly: 32767, rx: 123, ry: -456 };
        assert_eq!(Raw::from_bytes(&r.to_bytes()), r);
        assert_eq!(r.levels(30), 0x9001 | VB_RT);
        assert!(r.active());
        assert!(!Raw::default().active());
        let p = LookParams::default();
        assert_eq!(reshape_for_er(1000, -1000, &p), (0, 0));
        assert_eq!(reshape_for_er(32767, 0, &p), (32767, 0));
    }

    #[test]
    fn c3_pad_calib_median() {
        let mut c = Calib::default();
        assert!(!c.add(5, 1.0), "too few counts");
        assert!(!c.add(100, 0.0), "no turn");
        assert!(!c.add(100, 120.0), "a cut");
        for k in 0..20 {
            let s = if k % 2 == 0 { 1 } else { -1 };
            assert!(c.add(s * (100 + k), -(s as f32) * (100 + k) as f32 * 0.05), "either heading convention");
        }
        assert!(c.add(100, 30.0));
        assert!((c.estimate().unwrap() - 0.05).abs() < 1e-4);
        assert!((yaw_delta_deg((0.0, 1.0), (1.0, 0.0)) - 90.0).abs() < 1e-3);
        assert!((yaw_delta_deg((-0.01, -1.0), (0.01, -1.0)) - (-1.146)).abs() < 0.01, "wraps at 180");
    }

    /// C6 (docs/C6_HOLD_DESIGN.md 3.4): the two-hand chord never shows RB without Y in any read at 30 / 60 / 144 Hz, is
    /// refused at a prompt / during another pulse / while the user holds Y or RB / outside MW2 mode, and aborts both
    /// buttons together.
    #[test]
    fn c6_chord_never_rb_without_y() {
        let b = defaults();
        let (y, rb) = chord_buttons(&b);
        assert_eq!((y, rb), (XI_Y, XI_RB));
        for hz in [30.0f64, 60.0, 144.0] {
            let mut st = StepState::default();
            let dt = 1000.0 / hz;
            let (mut saw_y, mut saw_rb, mut done) = (false, false, false);
            for k in 0..60 {
                let now = 5000 + (k as f64 * dt) as u64;
                let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(0, now, true) });
                if k == 0 {
                    assert_eq!(o.chord, ChordEv::Started);
                }
                if o.synth & rb != 0 {
                    assert_ne!(o.synth & y, 0, "RB without Y at {hz} Hz read {k}");
                    saw_rb = true;
                }
                saw_y |= o.synth & y != 0;
                if o.chord == ChordEv::Done {
                    done = true;
                    assert_eq!(o.synth & (y | rb), 0);
                }
                let mut raw = Raw::default();
                apply_buttons(&mut raw, &o);
                assert!(raw.buttons as u32 & rb == 0 || raw.buttons as u32 & y != 0);
            }
            assert!(saw_y && done, "{hz} Hz");
            // 120 ms of RB: every rate sees it at least once
            assert!(saw_rb, "{hz} Hz never pressed RB");
        }
        for el in 0..400 {
            let (h, t) = chord_bits(el);
            assert!(!t || h, "tap without hold at {el} ms");
        }
    }

    #[test]
    fn c6_chord_refused_and_aborted() {
        let b = defaults();
        // a prompt on screen: refused (Y would interact)
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { chord_req: 1, er_prompt: true, ..sin(0, 100, true) });
        assert_eq!((o.chord, o.synth), (ChordEv::Refused("prompt"), 0));
        // the same request is not retried by the detour: the task sends a new one
        let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(0, 150, true) });
        assert_eq!(o.chord, ChordEv::None);
        // another pulse (melee) running
        let mut st = StepState::default();
        step(&mut st, &b, &sin(XI_R3, 0, true));
        let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(XI_R3, 10, true) });
        assert_eq!(o.chord, ChordEv::Refused("pulse"));
        // the user holds RB or Y
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(XI_RB, 0, true) });
        assert_eq!(o.chord, ChordEv::Refused("hold"));
        // in the gunship: refused; with MW2 mode off (the exit chord at mode off): sent
        let mut st = StepState::default();
        assert_eq!(step(&mut st, &b, &StepIn { chord_req: 1, gunship: true, ..sin(0, 0, false) }).chord, ChordEv::Refused("mode"));
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(0, 0, false) });
        assert_eq!((o.chord, o.synth & XI_Y), (ChordEv::Started, XI_Y));
        // a prompt appearing mid-chord: both released in the same read
        let mut st = StepState::default();
        step(&mut st, &b, &StepIn { chord_req: 1, ..sin(0, 0, true) });
        let o = step(&mut st, &b, &StepIn { chord_req: 1, ..sin(0, 150, true) });
        assert_eq!(o.synth & (XI_Y | XI_RB), XI_Y | XI_RB);
        let o = step(&mut st, &b, &StepIn { chord_req: 1, er_prompt: true, ..sin(0, 160, true) });
        assert_eq!((o.chord, o.synth & (XI_Y | XI_RB)), (ChordEv::Aborted("prompt"), 0));
        // a second request later starts a second chord
        let o = step(&mut st, &b, &StepIn { chord_req: 2, ..sin(0, 600, true) });
        assert_eq!(o.chord, ChordEv::Started);
        assert_eq!(chord_reason_code("pulse"), 2);
        assert_eq!(CHORD_REASONS[chord_reason_code("hold") as usize], "hold");
    }

    /// C6 (docs 3.5): with the stance active R3 never pulses ER's RB; without it melee is unchanged.
    #[test]
    fn c6_melee_suppressed_while_stance() {
        let b = defaults();
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { melee_off: true, ..sin(XI_R3, 0, true) });
        assert!(o.melee_suppressed);
        assert_eq!(o.synth & XI_RB, 0);
        let o = step(&mut st, &b, &StepIn { melee_off: true, ..sin(XI_R3, 50, true) });
        assert_eq!(o.synth & XI_RB, 0);
        // unchanged without the stance
        let mut st = StepState::default();
        let o = step(&mut st, &b, &sin(XI_R3, 0, true));
        assert!(!o.melee_suppressed);
        assert_eq!(o.synth & XI_RB, XI_RB);
        // a pulse running when the stance turns on is cut
        let o = step(&mut st, &b, &StepIn { melee_off: true, ..sin(XI_R3, 30, true) });
        assert_eq!(o.synth & XI_RB, 0);
    }

    /// 2026-10-24 (user: "can still shoot after death"): while the task's death gate holds, R3 melee and the heal button
    /// pulse nothing (a running pulse is cut); the gamepad's MW2 presses of the other actions are unchanged.
    #[test]
    fn c6_dead_no_melee_no_heal_pulse() {
        let b = defaults();
        let mut st = StepState::default();
        let o = step(&mut st, &b, &StepIn { dead: true, ..sin(XI_R3, 0, true) });
        assert_eq!(o.synth & XI_RB, 0, "no melee pulse while dead");
        assert!(!o.melee_suppressed, "not the stance's counter");
        let o = step(&mut st, &b, &StepIn { dead: true, ..sin(XI_DPAD_RIGHT, 16, true) });
        assert_eq!(o.presses & (1 << ACT_HEAL), 0);
        assert_eq!(o.synth & b.er_use_item, 0, "no heal pulse while dead");
        // pulses that started alive are cut on the death tick
        let mut st = StepState::default();
        assert_eq!(step(&mut st, &b, &sin(XI_R3, 0, true)).synth & XI_RB, XI_RB);
        let o = step(&mut st, &b, &StepIn { dead: true, ..sin(0, 16, true) });
        assert_eq!(o.synth & XI_RB, 0);
        let mut st = StepState::default();
        assert_ne!(step(&mut st, &b, &sin(XI_DPAD_RIGHT, 0, true)).synth & b.er_use_item, 0);
        let o = step(&mut st, &b, &StepIn { dead: true, ..sin(0, 16, true) });
        assert_eq!(o.synth & b.er_use_item, 0);
        // alive again: a new press pulses
        let mut st = StepState::default();
        assert_eq!(step(&mut st, &b, &sin(XI_R3, 1000, true)).synth & XI_RB, XI_RB);
    }

    /// C6 probe `tp_ads_er_aim`: a held LB while asked, released on every exit edge (wish off, MW2 mode off, gunship,
    /// prompt).
    #[test]
    fn c6_ads_er_aim_held_and_released() {
        let b = defaults();
        let mut st = StepState::default();
        for t in [0, 100, 2000] {
            assert_eq!(step(&mut st, &b, &StepIn { er_aim: true, ..sin(0, t, true) }).synth & XI_LB, XI_LB);
        }
        assert_eq!(step(&mut st, &b, &sin(0, 2100, true)).synth & XI_LB, 0, "wish off");
        assert_eq!(step(&mut st, &b, &StepIn { er_aim: true, ..sin(0, 2200, false) }).synth & XI_LB, 0, "mode off");
        assert_eq!(step(&mut st, &b, &StepIn { er_aim: true, gunship: true, ..sin(0, 2300, false) }).synth & XI_LB, 0, "gunship");
        assert_eq!(step(&mut st, &b, &StepIn { er_aim: true, er_prompt: true, ..sin(0, 2400, true) }).synth & XI_LB, 0, "prompt");
        // the user's own LB stays hidden while the synth is off
        let o = step(&mut st, &b, &sin(XI_LB, 2500, true));
        assert_eq!(o.hide & XI_LB, XI_LB);
    }
}
