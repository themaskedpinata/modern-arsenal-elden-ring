//! C3 (docs/C3_DESIGN.md 1.7): first person stuck off after a talk, F6 during a latch, a fade plate stuck high.
//!
//! [V] root cause (C2 logs): `CSPopupMenu.current_talk_id` keeps its last value after a dialog, so the talk latch only
//! released through `latch_stale_s` (20 s) after the value settled with the cursor closed; F6 pressed meanwhile set
//! `wanted = 0` and kept first person off afterwards (`FP blocked reason=f6`).
//!
//! - The talk latch gets its own stale time (`talk_stale_s`, 2 s) and holds while the dialog's `ui_states` index
//!   (`talk_menu_idx`, 3 in C2) is visible, like an open cursor.
//! - F6 while first person is off for a non-user reason and `wanted == 1` force-releases the talk / fade latches and
//!   keeps `wanted = 1` ([`f6_action`]). C3 review: a loading screen or a camera-mask cutscene is never overridden (their
//!   edge latches would not re-latch while the cutscene / load goes on, and the nuke reads the load latch): F6 there is
//!   deferred (`wanted` stays 1, first person returns when it ends).
//! - The fade latch gets `fade_stale_s` (3 s).

use crate::fp_logic::{LatchParams, Reason};

/// The talk latch: the configured hold-off, stale after `talk_stale_s` (at least 0.5 s) of a stable value with the
/// cursor (and the dialog index) closed.
pub fn talk_params(holdoff_s: f64, talk_stale_s: f64) -> LatchParams {
    LatchParams { holdoff_s, stale_s: if talk_stale_s.is_finite() { talk_stale_s.max(0.5) } else { 2.0 } }
}

/// The fade latch: stale after `fade_stale_s` (at least 0.5 s).
pub fn fade_params(holdoff_s: f64, fade_stale_s: f64) -> LatchParams {
    LatchParams { holdoff_s, stale_s: if fade_stale_s.is_finite() { fade_stale_s.max(0.5) } else { 3.0 } }
}

/// The "menu open" input of the talk latch: the cursor, or the dialog's `ui_states` index (-1 = off) visible.
pub fn talk_menu_open(cursor_menu: bool, ui_mask: u128, talk_menu_idx: i32) -> bool {
    cursor_menu || ((0..128).contains(&talk_menu_idx) && ui_mask & (1u128 << talk_menu_idx) != 0)
}

/// What one F6 press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum F6Action {
    /// First person is On (or the user had turned it off): flip `wanted`.
    Toggle,
    /// First person is off for a non-user reason while wanted: release the talk / fade latches, keep `wanted = 1`.
    Override(Reason),
    /// First person is off for a loading screen / camera-mask cutscene while wanted: nothing is released and `wanted`
    /// stays 1 (it comes back when the load / cutscene ends; F6 then works as a toggle again).
    Defer(Reason),
}

/// Reasons a user F6 press may override: the talk latch (the C2 bug), throw, the camera gate, a menu, and a cutscene
/// held ONLY by a fade plate (a stale plate; `fade_only`). Never a loading screen or a camera-mask cutscene.
pub fn overridable(r: Reason, fade_only: bool) -> bool {
    matches!(r, Reason::Talk | Reason::Menu | Reason::CamGate | Reason::Throw) || (r == Reason::Cutscene && fade_only)
}

/// Reasons where F6 is deferred instead of toggling `wanted` off (C2: F6 during a latch turned first person off for good).
pub fn deferred(r: Reason) -> bool {
    matches!(r, Reason::Loading | Reason::Cutscene)
}

/// One F6 press. `on`: first person is On now; `blocked`: the reason it is off (the machine's last block); `fade_only`:
/// the cutscene reason is held by the fade latch alone (no camera-mask latch).
pub fn f6_action(on: bool, wanted: bool, blocked: Option<Reason>, override_enabled: bool, fade_only: bool) -> F6Action {
    match blocked {
        Some(r) if override_enabled && !on && wanted && overridable(r, fade_only) => F6Action::Override(r),
        Some(r) if override_enabled && !on && wanted && deferred(r) => F6Action::Defer(r),
        _ => F6Action::Toggle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fp_logic::{EdgeLatch, LatchStep};

    /// Replays a talk: the latch is set at `t_set` (baseline -> `v`), the dialog index is open over `menu`, the value
    /// changes at the given times; returns the release time.
    fn replay(p: LatchParams, base: i32, seq: &[(f64, i32)], menu: &[(f64, f64)], t_end: f64) -> Option<f64> {
        let mut l = EdgeLatch::default();
        l.latch(base, Some(seq[0].1), seq[0].0);
        let mut t = seq[0].0;
        let mut v = seq[0].1;
        while t <= t_end {
            for &(ts, nv) in seq {
                if ts <= t {
                    v = nv;
                }
            }
            let m = menu.iter().any(|&(a, b)| t >= a && t < b);
            if let LatchStep::Released(_) = l.step(Some(v), talk_menu_open(false, if m { 1 << 3 } else { 0 }, 3), t, p) {
                return Some(t);
            }
            t += 1.0 / 60.0;
        }
        None
    }

    #[test]
    fn c3_fp_talk_releases_two_seconds_after_the_dialog_closes() {
        let p = talk_params(2.0, 2.0);
        // host 15:16:19 (1000 -> 204101000), dialog index 3 open 0.02..4.1 and 4.2..8.1 (the 17 ms flap at 4.1)
        let r = replay(p, 1000, &[(0.0, 204101000)], &[(0.02, 4.15), (4.17, 8.12)], 30.0).unwrap();
        assert!(r >= 8.12 + 2.0 - 0.02 && r <= 8.12 + 2.5, "released at {r}");
    }

    #[test]
    fn c3_fp_talk_held_while_the_dialog_index_is_open() {
        let p = talk_params(2.0, 2.0);
        let r = replay(p, 1000, &[(0.0, 302001000)], &[(0.0, 15.0)], 30.0).unwrap();
        assert!(r >= 17.0 - 0.02, "{r}");
    }

    /// The host's whole C2 talk sequence (-1 -> 1000 -> 3000 -> 1000 -> 204101000 -> 1000 -> 302001000): each latch
    /// releases within 2.5 s of its dialog closing.
    #[test]
    fn c3_fp_talk_replay_of_the_c2_host_sequence() {
        let p = talk_params(2.0, 2.0);
        let cases: [(i32, &[(f64, i32)], &[(f64, f64)]); 4] = [
            (-1, &[(0.0, 1000), (6.4, 3000), (22.6, 1000)], &[(0.02, 1.75), (22.65, 24.25)]),
            (1000, &[(0.0, 204101000)], &[(0.02, 4.15), (4.17, 8.12)]),
            (1000, &[(0.0, 302001000)], &[(0.02, 4.08)]),
            (302001000, &[(0.0, 302201000)], &[(0.02, 3.65)]),
        ];
        for (base, seq, menu) in cases {
            let close = menu.last().unwrap().1;
            let last_change = seq.last().unwrap().0;
            let r = replay(p, base, seq, menu, 60.0).unwrap();
            assert!(r <= close.max(last_change) + 2.5, "base {base}: released at {r}, closed at {close}");
        }
    }

    /// Mutation: the old general stale time (20 s) must fail the 2.5 s bound.
    #[test]
    fn c3_fp_talk_mutation_stale_20_fails() {
        let p = LatchParams { holdoff_s: 2.0, stale_s: 20.0 };
        let r = replay(p, 1000, &[(0.0, 302001000)], &[(0.02, 4.08)], 60.0).unwrap();
        assert!(r > 4.08 + 2.5, "{r}");
    }

    #[test]
    fn c3_fp_talk_menu_input_and_param_floors() {
        assert!(talk_menu_open(true, 0, -1));
        assert!(talk_menu_open(false, 1 << 3, 3));
        assert!(!talk_menu_open(false, 1 << 3, -1));
        assert!(!talk_menu_open(false, 1 << 3, 200));
        assert_eq!(talk_params(2.0, 0.0).stale_s, 0.5);
        assert_eq!(talk_params(2.0, f64::NAN).stale_s, 2.0);
        assert_eq!(fade_params(2.0, 3.0).stale_s, 3.0);
    }

    #[test]
    fn c3_fp_f6_override_during_latches_keeps_wanted() {
        for r in [Reason::Talk, Reason::Menu, Reason::CamGate, Reason::Throw] {
            assert_eq!(f6_action(false, true, Some(r), true, false), F6Action::Override(r), "{r:?}");
        }
        // a cutscene held only by a (stale) fade plate is overridable; a camera-mask cutscene / loading is deferred
        assert_eq!(f6_action(false, true, Some(Reason::Cutscene), true, true), F6Action::Override(Reason::Cutscene));
        assert_eq!(f6_action(false, true, Some(Reason::Cutscene), true, false), F6Action::Defer(Reason::Cutscene));
        assert_eq!(f6_action(false, true, Some(Reason::Loading), true, true), F6Action::Defer(Reason::Loading));
        assert_eq!(f6_action(false, true, Some(Reason::Loading), true, false), F6Action::Defer(Reason::Loading));
        // On: F6 turns it off
        assert_eq!(f6_action(true, true, None, true, false), F6Action::Toggle);
        // the user already turned it off: F6 turns it on
        assert_eq!(f6_action(false, false, Some(Reason::F6), true, false), F6Action::Toggle);
        assert_eq!(f6_action(false, false, Some(Reason::Talk), true, false), F6Action::Toggle);
        assert_eq!(f6_action(false, false, Some(Reason::Loading), true, false), F6Action::Toggle);
        // switched off in the config: the old toggle
        assert_eq!(f6_action(false, true, Some(Reason::Talk), false, false), F6Action::Toggle);
        // hard reasons never override
        for r in [Reason::Death, Reason::ReadFail, Reason::Ac130, Reason::Nuke, Reason::Torrent, Reason::Ladder, Reason::F5] {
            assert_eq!(f6_action(false, true, Some(r), true, true), F6Action::Toggle, "{r:?}");
        }
    }

    /// Review C3 (mutation guard): the load / camera-mask latches are never overridable (they would not re-latch while
    /// the load / cutscene goes on).
    #[test]
    fn c3_fp_f6_never_overrides_load_or_mask() {
        assert!(!overridable(Reason::Loading, true) && !overridable(Reason::Loading, false));
        assert!(!overridable(Reason::Cutscene, false));
    }
}
