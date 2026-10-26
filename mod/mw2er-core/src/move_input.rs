//! Movement keys and what is hidden from Elden Ring while the controller drives (docs/M5_DESIGN.md section 6).
//!
//! The input detour (DLL) only touches atomics and the caller's 256-byte keyboard buffer; the decisions are these
//! pure functions: the key table from the config, the two 256-bit blank masks of a context (latched keys and the
//! never-latched W/A/S/D of first person), the per-key latch step, and the key-probe bit names.

use crate::input_logic::{dik_from_name, latch_next};

/// The movement keys as DIK codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveKeys {
    pub forward: u8,
    pub back: u8,
    pub left: u8,
    pub right: u8,
    pub sprint: u8,
    pub jump: u8,
    pub crouch: u8,
    pub prone: u8,
    /// Optional console-style key (tap = crouch, hold 300 ms = prone); 0 = none.
    pub stance_hold: u8,
}

/// Indices of the press counters the detour keeps (`PRESSES_MOVE`).
pub const PRESS_SPRINT: usize = 0;
pub const PRESS_JUMP: usize = 1;
pub const PRESS_CROUCH: usize = 2;
pub const PRESS_PRONE: usize = 3;
pub const PRESS_STANCE_HOLD: usize = 4;
pub const PRESS_INTERACT: usize = 5;
pub const PRESS_KEYS: usize = 6;

/// ER's interact key (E): never blanked; its press is the `interact` handback.
pub const DIK_E: u8 = 0x12;

impl MoveKeys {
    /// The MW2 PC defaults (`default_mp_controls.cfg`): W/A/S/D, LShift sprint, Space +gostand, C togglecrouch,
    /// LCtrl toggleprone.
    pub fn mw2_defaults() -> MoveKeys {
        MoveKeys { forward: 0x11, back: 0x1F, left: 0x1E, right: 0x20, sprint: 0x2A, jump: 0x39, crouch: 0x2E, prone: 0x1D, stance_hold: 0 }
    }

    /// The keys whose presses are counted, in `PRESS_*` order (0 = unused slot).
    pub fn press_keys(&self) -> [u8; PRESS_KEYS] {
        [self.sprint, self.jump, self.crouch, self.prone, self.stance_hold, DIK_E]
    }

    pub fn wasd(&self) -> [u8; 4] {
        [self.forward, self.back, self.left, self.right]
    }

    /// Parses the `[keys] move_*` names.
    #[allow(clippy::too_many_arguments)]
    pub fn from_names(
        forward: &str,
        back: &str,
        left: &str,
        right: &str,
        sprint: &str,
        jump: &str,
        crouch: &str,
        prone: &str,
        stance_hold: &str,
    ) -> Result<MoveKeys, String> {
        let k = |what: &str, n: &str| dik_from_name(n).ok_or_else(|| format!("keys.{what} = {n:?} is not a known key"));
        Ok(MoveKeys {
            forward: k("move_forward", forward)?,
            back: k("move_back", back)?,
            left: k("move_left", left)?,
            right: k("move_right", right)?,
            sprint: k("move_sprint", sprint)?,
            jump: k("move_jump", jump)?,
            crouch: k("move_crouch", crouch)?,
            prone: k("move_prone", prone)?,
            stance_hold: if stance_hold.trim().is_empty() { 0 } else { k("move_stance_hold", stance_hold)? },
        })
    }

    /// No duplicates among the movement keys, none equal to `others` (toggle, reload, fp_toggle, ac130), and E stays
    /// ER's. The AC-130 in-ride keys may overlap (they are only read while the controller is suspended).
    pub fn validate(&self, others: &[(&str, u8)]) -> Result<(), String> {
        let mut all: Vec<(&str, u8)> = vec![
            ("move_forward", self.forward),
            ("move_back", self.back),
            ("move_left", self.left),
            ("move_right", self.right),
            ("move_sprint", self.sprint),
            ("move_jump", self.jump),
            ("move_crouch", self.crouch),
            ("move_prone", self.prone),
        ];
        if self.stance_hold != 0 {
            all.push(("move_stance_hold", self.stance_hold));
        }
        for (i, a) in all.iter().enumerate() {
            if a.1 == DIK_E {
                return Err(format!("keys.{} must not be E (ER's interact key stays ER's)", a.0));
            }
            for b in &all[i + 1..] {
                if a.1 == b.1 {
                    return Err(format!("keys.{} and keys.{} are the same key", a.0, b.0));
                }
            }
            for (n, k) in others {
                if a.1 == *k {
                    return Err(format!("keys.{} must differ from keys.{n}", a.0));
                }
            }
        }
        Ok(())
    }
}

/// A 256-bit DIK set.
pub type KeyMask = [u64; 4];

pub fn mask_set(m: &mut KeyMask, dik: u8) {
    m[dik as usize >> 6] |= 1u64 << (dik & 63);
}

pub fn mask_has(m: &KeyMask, dik: u8) -> bool {
    m[dik as usize >> 6] & (1u64 << (dik & 63)) != 0
}

pub fn mask_count(m: &KeyMask) -> u32 {
    m.iter().map(|w| w.count_ones()).sum()
}

/// The context of the blank decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlankCtx {
    /// The controller drives this tick (and the usual blanking gates hold: heartbeat, in world, no menu, focused).
    pub driving: bool,
    /// First person is On.
    pub fp: bool,
    /// Third person keeps ER's animation (`tp_anim = "hybrid"`).
    pub hybrid: bool,
    /// C3 PASS C (docs/C3_DESIGN.md 3.6): first person too (`fp_anim = "hybrid"`): ER sees W/A/S/D and the left stick,
    /// plays its walk / run animations (peers see them), and our position write discards its root motion.
    pub fp_hybrid: bool,
}

/// The wanted masks: `latch` = keys hidden from ER and latched until physical release (Space, Shift, C, Ctrl, F, X,
/// Q, LAlt + the movement action keys); `plain` = W/A/S/D in first person, never latched (a handback gives ER a
/// held key at once). Nothing when not driving: a key still held stays blanked by its own latch only.
pub fn blank_masks(keys: &MoveKeys, blank_er: &[u8], ctx: BlankCtx) -> (KeyMask, KeyMask) {
    let mut latch = [0u64; 4];
    let mut plain = [0u64; 4];
    if !ctx.driving {
        return (latch, plain);
    }
    for k in blank_er.iter().copied().chain([keys.sprint, keys.jump, keys.crouch, keys.prone, keys.stance_hold]) {
        if k != 0 && k != DIK_E {
            mask_set(&mut latch, k);
        }
    }
    if wasd_blanked(ctx) {
        for k in keys.wasd() {
            mask_set(&mut plain, k);
        }
    }
    (latch, plain)
}

/// W/A/S/D (and, C3 PASS B, the pad's left stick: docs/C3_DESIGN.md 4.5) are hidden from ER: while the controller
/// drives in first person without `fp_anim = "hybrid"` (C3 PASS C), or in third person without `tp_anim = "hybrid"`.
/// The one rule both inputs follow.
pub fn wasd_blanked(ctx: BlankCtx) -> bool {
    ctx.driving && if ctx.fp { !ctx.fp_hybrid } else { !ctx.hybrid }
}

/// One key in the detour: returns (blank this read, latched state to keep). Levels are recorded BEFORE this.
pub fn key_blank(latched: bool, want_latch: bool, want_plain: bool, down: bool) -> (bool, bool) {
    let l = latch_next(latched, want_latch, down);
    (l || want_plain, l)
}

/// The default `[movement] blank_er` list (section 6.2): ER's dodge, its modifier, crouch keys, jump, lock-on, walk.
pub const BLANK_ER_DEFAULT: [&str; 8] = ["SPACE", "LSHIFT", "C", "LCTRL", "F", "X", "Q", "LALT"];

/// Keys the first-run probe watches (MW2 mode off): their ER action bits are logged once each. M6 adds the weapon
/// keys 1 and 2 (docs/M6_DESIGN.md section 5.1).
pub const N_PROBE: usize = 11;
pub const PROBE_KEYS: [(&str, u8); N_PROBE] = [
    ("SPACE", 0x39),
    ("LSHIFT", 0x2A),
    ("C", 0x2E),
    ("LCTRL", 0x1D),
    ("F", 0x21),
    ("X", 0x2D),
    ("Z", 0x2C),
    ("Q", 0x10),
    ("LALT", 0x38),
    ("1", 0x02),
    ("2", 0x03),
];

/// `ChrActions` bit names (fromsoftware-rs `cs/chr_ins/module/action_request.rs`).
const ACTION_BITS: [(u32, &str); 34] = [
    (0, "r1"),
    (1, "r2"),
    (2, "l1"),
    (3, "l2"),
    (4, "action"),
    (5, "sp_move"),
    (6, "jump"),
    (7, "use_item"),
    (8, "switch_form"),
    (9, "change_weapon_r"),
    (10, "change_weapon_l"),
    (11, "change_item"),
    (12, "r3"),
    (13, "l3"),
    (14, "touch_r"),
    (15, "touch_l"),
    (16, "backstep"),
    (17, "rolling"),
    (19, "magic_r"),
    (20, "magic_l"),
    (21, "gesture"),
    (22, "ladderup"),
    (23, "ladderdown"),
    (24, "guard"),
    (25, "emergencystep"),
    (26, "light_kick"),
    (27, "heavy_kick"),
    (28, "change_style_r"),
    (29, "change_style_l"),
    (30, "rideon"),
    (31, "rideoff"),
    (32, "buddy_disappear"),
    (33, "magic_r2"),
    (34, "magic_l2"),
];

/// `er_bits=` of a `MOVE keyprobe` line: the names of the set bits joined with `|`, or `none`.
pub fn er_bit_names(bits: u64) -> String {
    let names: Vec<&str> = ACTION_BITS.iter().filter(|(b, _)| bits & (1u64 << b) != 0).map(|(_, n)| *n).collect();
    if names.is_empty() { "none".into() } else { names.join("|") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_logic::{CounterReader, effective_want, press_edge};

    fn names(list: &[&str]) -> Vec<u8> {
        list.iter().map(|n| dik_from_name(n).unwrap()).collect()
    }

    #[test]
    fn move_input_dik_names() {
        for (n, k) in [("SPACE", 0x39), ("LSHIFT", 0x2A), ("RSHIFT", 0x36), ("LCTRL", 0x1D), ("LCONTROL", 0x1D), ("RCTRL", 0x9D), ("LALT", 0x38), ("RALT", 0xB8), ("TAB", 0x0F), ("CAPSLOCK", 0x3A), ("C", 0x2E), ("X", 0x2D), ("F", 0x21), ("Q", 0x10), ("E", 0x12), ("Z", 0x2C)] {
            assert_eq!(dik_from_name(n), Some(k), "{n}");
        }
        assert_eq!(MoveKeys::from_names("W", "S", "A", "D", "LSHIFT", "SPACE", "C", "LCTRL", "").unwrap(), MoveKeys::mw2_defaults());
        assert!(MoveKeys::from_names("W", "S", "A", "D", "LSHIFT", "SPACEBAR", "C", "LCTRL", "").is_err());
        assert_eq!(MoveKeys::from_names("W", "S", "A", "D", "LSHIFT", "SPACE", "C", "LCTRL", "V").unwrap().stance_hold, 0x2F);
    }

    #[test]
    fn move_input_duplicate_keys_rejected_and_ac130_overlap_allowed() {
        let k = MoveKeys::mw2_defaults();
        let others = [("toggle", 0x3F), ("reload", 0x13), ("fp_toggle", 0x40), ("ac130", 0x41)];
        k.validate(&others).unwrap();
        let dup = MoveKeys { crouch: k.sprint, ..k };
        assert!(dup.validate(&others).unwrap_err().contains("same key"));
        let clash = MoveKeys { jump: 0x13, ..k };
        assert!(clash.validate(&others).unwrap_err().contains("reload"));
        let e = MoveKeys { prone: DIK_E, ..k };
        assert!(e.validate(&others).is_err());
        // the AC-130 in-ride keys (Q next, F thermal, 1-3 guns) may be blank keys: they are not in `others`
        let (latch, _) = blank_masks(&k, &names(&BLANK_ER_DEFAULT), BlankCtx { driving: true, fp: true, hybrid: true, fp_hybrid: false });
        assert!(mask_has(&latch, 0x10) && mask_has(&latch, 0x21));
    }

    #[test]
    fn move_input_blank_sets_per_context() {
        let k = MoveKeys::mw2_defaults();
        let er = names(&BLANK_ER_DEFAULT);
        // first person driving: latched set + W/A/S/D plain
        let (latch, plain) = blank_masks(&k, &er, BlankCtx { driving: true, fp: true, hybrid: true, fp_hybrid: false });
        for n in BLANK_ER_DEFAULT {
            assert!(mask_has(&latch, dik_from_name(n).unwrap()), "{n}");
        }
        assert!(!mask_has(&latch, DIK_E), "E always reaches ER");
        assert!(!mask_has(&latch, 0x11), "W never latched");
        for d in k.wasd() {
            assert!(mask_has(&plain, d));
        }
        // third person hybrid: ER sees W/A/S/D (it animates)
        let (latch2, plain2) = blank_masks(&k, &er, BlankCtx { driving: true, fp: false, hybrid: true, fp_hybrid: false });
        assert_eq!(latch2, latch);
        assert_eq!(mask_count(&plain2), 0);
        // suspended / arming / off / MW2 mode off: nothing new
        let (l3, p3) = blank_masks(&k, &er, BlankCtx { driving: false, fp: true, hybrid: true, fp_hybrid: false });
        assert_eq!((mask_count(&l3), mask_count(&p3)), (0, 0));
    }

    #[test]
    fn c3_pad_left_stick_follows_the_wasd_rule() {
        let k = MoveKeys::mw2_defaults();
        for driving in [false, true] {
            for fp in [false, true] {
                for hybrid in [false, true] {
                    for fp_hybrid in [false, true] {
                        let ctx = BlankCtx { driving, fp, hybrid, fp_hybrid };
                        let (_, plain) = blank_masks(&k, &[], ctx);
                        assert_eq!(wasd_blanked(ctx), mask_count(&plain) == 4, "{ctx:?}");
                    }
                }
            }
        }
        assert!(!wasd_blanked(BlankCtx { driving: true, fp: false, hybrid: true, fp_hybrid: false }), "third person hybrid: ER animates");
    }

    #[test]
    fn c3_fpanim_blank_sets_hybrid_vs_blank() {
        let k = MoveKeys::mw2_defaults();
        let er = names(&BLANK_ER_DEFAULT);
        // fp_anim = "blank" (C2): W/A/S/D hidden in first person
        let blank = BlankCtx { driving: true, fp: true, hybrid: true, fp_hybrid: false };
        let (l1, p1) = blank_masks(&k, &er, blank);
        assert_eq!(mask_count(&p1), 4);
        // fp_anim = "hybrid": ER sees W/A/S/D (walk / run animations); Space/LShift/C/LCtrl/F/X/Q/LAlt stay latched
        let hy = BlankCtx { fp_hybrid: true, ..blank };
        let (l2, p2) = blank_masks(&k, &er, hy);
        assert_eq!(mask_count(&p2), 0);
        assert_eq!(l2, l1, "the action keys stay hidden");
        for n in BLANK_ER_DEFAULT {
            assert!(mask_has(&l2, dik_from_name(n).unwrap()), "{n}");
        }
        assert!(!wasd_blanked(hy));
        // third person is governed by tp_anim alone
        assert!(wasd_blanked(BlankCtx { driving: true, fp: false, hybrid: false, fp_hybrid: true }));
        assert!(!wasd_blanked(BlankCtx { driving: false, ..blank }), "not driving: nothing");
    }

    #[test]
    fn move_input_space_latched_across_handback_wasd_released() {
        // Space held while driving, then a handback (want drops): Space stays blanked until released
        let (b, l) = key_blank(false, true, false, true);
        assert!(b && l);
        let (b, l) = key_blank(l, false, false, true);
        assert!(b, "still held: still hidden (ER dodges on release)");
        let (b, l) = key_blank(l, false, false, false);
        assert!(!b && !l);
        let (b, _) = key_blank(l, false, false, true);
        assert!(!b, "a new press reaches ER");
        // W in first person: blanked while wanted, given back at once (held) on a handback
        let (b, l) = key_blank(false, false, true, true);
        assert!(b && !l);
        let (b, _) = key_blank(l, false, false, true);
        assert!(!b, "W reaches ER at once");
    }

    #[test]
    fn move_input_press_counters_three_taps_one_tick() {
        // the detour polls ~every frame; three taps between two 33 ms task ticks count 3
        let mut counter = 0u32;
        let mut prev = false;
        for down in [true, false, true, false, true, true, false] {
            if press_edge(prev, down) {
                counter += 1;
            }
            prev = down;
        }
        let mut r = CounterReader::starting_at(0);
        assert_eq!(r.take(counter), 3);
        assert_eq!(r.take(counter), 0);
    }

    #[test]
    fn move_input_inert_with_stale_heartbeat() {
        let want = effective_want(true, 10_300, 10_000);
        assert!(!want);
        let (b, _) = key_blank(false, want, want, true);
        assert!(!b, "a stale task heartbeat hides nothing");
    }

    #[test]
    fn move_input_keyprobe_bit_names() {
        assert_eq!(er_bit_names(0), "none");
        assert_eq!(er_bit_names((1 << 16) | (1 << 17)), "backstep|rolling");
        assert_eq!(er_bit_names(1 << 6), "jump");
        assert_eq!(er_bit_names(1 << 13), "l3");
        assert_eq!(PROBE_KEYS.len(), N_PROBE);
    }
}
