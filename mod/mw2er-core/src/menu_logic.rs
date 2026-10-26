//! "Is an Elden Ring menu open?" from the `CSMenuManImp.ui_states` visible bits.
//!
//! Which index is which screen is unknown (CS2 note: ui_states works as a signal, the mapping is
//! not documented), so the rule is relative: indices visible in normal gameplay are allowed
//! (configured `menu.hud_indices`, or the set captured when MW2 mode was enabled); any other
//! visible index means a menu is open. Transitions are always logged to build the index map.

/// Number of UI state slots (`ui_states: [UIState; 0x46]`).
pub const UI_STATE_COUNT: usize = 0x46;

/// Bit mask of indices.
pub fn mask_from_indices(indices: &[u32]) -> u128 {
    indices.iter().filter(|&&i| i < 128).fold(0u128, |m, &i| m | (1u128 << i))
}

/// Indices set in a mask, ascending.
pub fn indices_of(mask: u128) -> Vec<u32> {
    (0..128u32).filter(|&i| mask & (1u128 << i) != 0).collect()
}

/// Mask built from one visible flag per slot.
pub fn mask_from_flags(visible: &[bool]) -> u128 {
    visible.iter().take(128).enumerate().fold(0u128, |m, (i, &v)| if v { m | (1u128 << i) } else { m })
}

/// The allow-list in force: configured indices if any, else the baseline captured at enable.
pub fn effective_allow(hud_indices: &[u32], baseline: Option<u128>) -> u128 {
    if hud_indices.is_empty() { baseline.unwrap_or(0) } else { mask_from_indices(hud_indices) }
}

/// The menu-open decision. `use_ui_states = false` ignores the mask (uncalibrated indices flip
/// during normal play), leaving the cursor as the only signal.
pub fn menu_open(visible_mask: u128, allow: u128, use_ui_states: bool, use_show_cursor: bool, show_cursor: bool) -> bool {
    (use_ui_states && (visible_mask & !allow) != 0) || (use_show_cursor && show_cursor)
}

/// `menu_open`, or any of `menu_mask`'s indices visible (menus that show no cursor: ER's pause menu on a pad).
pub fn menu_open_or_indices(visible_mask: u128, menu_mask: u128, allow: u128, use_ui_states: bool, use_show_cursor: bool, show_cursor: bool) -> bool {
    visible_mask & menu_mask != 0 || menu_open(visible_mask, allow, use_ui_states, use_show_cursor, show_cursor)
}

/// How long after the AC-130 gives ER's HUD back (`hud_state` restored) a hidden HUD index is still ours: ER shows the
/// HUD again one UI frame later (2026-10-21 12:58:43.218 restore -> 43.236 idx 7 back).
pub const HUD_OURS_GRACE_S: f64 = 0.25;

/// A `[menu] hud_off_indices` index that was visible when MW2 mode was enabled (`baseline`) and is hidden now: ER took
/// its HUD away for a full-screen menu that shows no cursor on a pad (2026-10-21: a site of grace; idx 7 is hidden from
/// ~1.7 s after the sit until the menu closes, never while moving or shooting in any logged session). Never while we
/// hid the HUD ourselves (`hud_ours`: an AC-130 ride, see `HudOwner`); never for an index that was not in the baseline
/// (HUD switched off in ER's options, or MW2 mode enabled at a grace); never with MW2 mode off (the baseline is then the
/// current mask).
pub fn hud_gone(visible_mask: u128, hud_off_mask: u128, baseline: Option<u128>, hud_ours: bool) -> bool {
    !hud_ours && hud_off_mask & baseline.unwrap_or(0) & !visible_mask != 0
}

/// "We hid ER's HUD" (`CSFeManImp.hud_state` written by the AC-130), held `HUD_OURS_GRACE_S` past the restore.
#[derive(Clone, Copy, Debug, Default)]
pub struct HudOwner {
    last_t: Option<f64>,
}

impl HudOwner {
    /// `ours`: the HUD write is held (or its restore pending) as of this tick's menu read.
    pub fn step(&mut self, now: f64, ours: bool) -> bool {
        if ours {
            self.last_t = Some(now);
        }
        self.last_t.is_some_and(|t| now - t <= HUD_OURS_GRACE_S)
    }
}

/// The bit mask of `indices` (indices >= 128 are ignored; the config rejects them).
pub fn mask_of(indices: &[u32]) -> u128 {
    indices.iter().filter(|&&i| i < 128).fold(0, |m, &i| m | 1u128 << i)
}

/// Extra refusal when F5 would enable MW2 mode while `menu.hud_indices` is not calibrated yet.
///
/// Without configured indices the visible set at F5 becomes the "HUD" baseline, so a menu that is
/// open at that moment would count as HUD for the whole mode session. The cursor signal
/// (`CSMouseMan.show_cursor`, unverified) is therefore checked at enable time even when
/// `menu.use_show_cursor` is off; `menu.cursor_gate_at_enable = false` turns this off.
pub fn enable_refusal(hud_indices: &[u32], cursor_gate: bool, show_cursor: Option<bool>) -> Option<&'static str> {
    (hud_indices.is_empty() && cursor_gate && show_cursor == Some(true))
        .then_some("cursor visible - close the menu (or set menu.cursor_gate_at_enable = false)")
}

/// Visibility changes between two masks as `(index, now_visible)`.
pub fn transitions(prev: u128, cur: u128) -> Vec<(u32, bool)> {
    let changed = prev ^ cur;
    indices_of(changed).into_iter().map(|i| (i, cur & (1u128 << i) != 0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_indices_open_without_cursor() {
        let m = mask_of(&[28]);
        assert!(menu_open_or_indices(1 << 28, m, 0, false, true, false), "pad pause menu: idx 28, no cursor");
        assert!(!menu_open_or_indices(1 << 23, m, 0, false, true, false), "another index alone is not a menu");
        assert!(menu_open_or_indices(0, m, 0, false, true, true), "the cursor still counts");
        assert!(!menu_open_or_indices(1 << 28, 0, 0, false, true, false), "empty list = the old behaviour");
        assert_eq!(mask_of(&[0, 127, 128]), 1 | 1u128 << 127);
    }

    #[test]
    fn hud_gone_grace_menu_on_a_pad_blocks_the_mw2_actions() {
        // Regression (2026-10-21 13:02:01.544, pad, sitting at a grace after "Rest"): visible [5, 8] (idx 7, ER's HUD,
        // hidden), cursor false, MW2 baseline [5, 7, 8, 23] -> D-pad down armed the nuke. Same at 2026-10-20 19:39:00.697.
        let hud_off = mask_of(&[7]);
        let menu = mask_of(&[28]);
        let base = Some(mask_of(&[5, 7, 8, 23]));
        let grace = mask_of(&[5, 8]);
        let allow = effective_allow(&[], base);
        assert!(!menu_open_or_indices(grace, menu, allow, false, true, false), "the bug: no cursor, no idx 28");
        assert!(hud_gone(grace, hud_off, base, false), "the grace menu");
        assert!(hud_gone(mask_of(&[4, 5, 8]), hud_off, base, false), "the sit itself ([4, 5, 8] at 13:01:59.511)");
        // normal play (idx 7 visible, prompts coming and going): never
        for vis in [&[5, 7, 8][..], &[5, 7, 8, 19], &[5, 7, 8, 9, 19], &[5, 7, 8, 18, 23], &[3, 5, 7, 8]] {
            assert!(!hud_gone(mask_of(vis), hud_off, base, false), "{vis:?}");
        }
        // the AC-130 hides ER's HUD itself (12:58:03-12:58:43 visible [5, 8, 23]): the ride must not read it as a menu
        assert!(!hud_gone(mask_of(&[5, 8, 23]), hud_off, base, true));
        // not in the baseline (HUD off in ER's options, or F5 at a grace): disarmed, MW2 mode keeps working
        assert!(!hud_gone(grace, hud_off, Some(mask_of(&[5, 8])), false));
        assert!(!hud_gone(grace, hud_off, None, false));
        // MW2 mode off: task.rs passes the current mask as the baseline -> never
        assert!(!hud_gone(grace, hud_off, Some(grace), false));
        // empty list = the old behaviour
        assert!(!hud_gone(grace, 0, base, false));
    }

    #[test]
    fn hud_owner_holds_past_the_ac130_restore() {
        let mut o = HudOwner::default();
        assert!(!o.step(0.0, false), "never hidden by us");
        assert!(o.step(1.0, true), "ride: the HUD write held");
        assert!(o.step(41.0, true));
        // restore at 41.0 (12:58:43.218); ER shows idx 7 again one UI frame later (43.236)
        assert!(o.step(41.018, false), "the frame before ER shows the HUD again");
        assert!(o.step(41.0 + HUD_OURS_GRACE_S, false));
        assert!(!o.step(41.3, false), "then a hidden HUD is ER's again (a grace)");
    }

    #[test]
    fn menu_baseline_allowlist() {
        let baseline = mask_from_indices(&[3, 40]);
        let allow = effective_allow(&[], Some(baseline));
        assert!(!menu_open(baseline, allow, true, false, false));
        assert!(!menu_open(mask_from_indices(&[3]), allow, true, false, false), "a HUD element hiding is not a menu");
        assert!(menu_open(mask_from_indices(&[3, 40, 12]), allow, true, false, false), "a new index means a menu");
        assert!(menu_open(baseline, allow, true, true, true), "cursor signal when enabled");
        assert!(!menu_open(baseline, allow, true, false, true));
    }

    #[test]
    fn menu_ui_states_off_uses_cursor_only() {
        // Regression (first in-game run): a HUD index appearing must not count as a menu.
        let allow = mask_from_indices(&[5, 7, 8]);
        assert!(!menu_open(mask_from_indices(&[5, 7, 8, 13, 23]), allow, false, true, false));
        assert!(menu_open(mask_from_indices(&[5, 7, 8]), allow, false, true, true), "cursor visible = menu");
    }

    #[test]
    fn menu_configured_indices_win() {
        let allow = effective_allow(&[5], Some(mask_from_indices(&[5, 6])));
        assert!(menu_open(mask_from_indices(&[5, 6]), allow, true, false, false));
        assert_eq!(effective_allow(&[], None), 0);
    }

    #[test]
    fn menu_enable_gate_with_cursor() {
        // uncalibrated + cursor visible: refuse, whatever use_show_cursor says
        assert!(enable_refusal(&[], true, Some(true)).is_some());
        assert!(enable_refusal(&[], true, Some(false)).is_none());
        assert!(enable_refusal(&[], true, None).is_none(), "no CSMouseMan: cannot tell, allow");
        // calibrated indices: menu_open() already decides
        assert!(enable_refusal(&[3, 40], true, Some(true)).is_none());
        // gate switched off by config
        assert!(enable_refusal(&[], false, Some(true)).is_none());
    }

    #[test]
    fn menu_transitions_listed() {
        let a = mask_from_indices(&[1, 2, 69]);
        let b = mask_from_indices(&[2, 9, 69]);
        assert_eq!(transitions(a, b), vec![(1, false), (9, true)]);
        assert!(transitions(a, a).is_empty());
        let flags: Vec<bool> = (0..UI_STATE_COUNT).map(|i| i == 0 || i == 69).collect();
        assert_eq!(indices_of(mask_from_flags(&flags)), vec![0, 69]);
    }
}
