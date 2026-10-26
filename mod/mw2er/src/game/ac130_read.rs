//! M4 AC-130 signals (docs/M4_DESIGN.md sections 6.7, 9): the player's SpEffect list, CSFeManImp.hud_state, HP.
//!
//! Same rules as the rest of `game`: non-faulting reads, pointers read as integers and checked, no references. The
//! protection's two engine calls live in `ac130.rs` (the only place allowed to make them).

use std::mem::offset_of;

use eldenring::cs::{CSFeManImp, ChrIns, SpecialEffect, SpecialEffectEntry, SpecialEffectEntryAccumulatorInfo, WorldChrMan};

use super::{plausible, read, read_ptr, singleton_addr};

// The SpEffect list is walked by offset. `SpecialEffect.head` (private) sits right after the vftable, `next`
// (private) right after the 0x20-byte accumulator info: pinned by the public neighbours below.
const _: () = assert!(offset_of!(SpecialEffect, owner) == 0x10);
const SPECIAL_EFFECT_HEAD: usize = 0x8;
const _: () = assert!(offset_of!(SpecialEffectEntry, param_id) == 0x8);
const _: () = assert!(offset_of!(SpecialEffectEntry, accumulator_info) == 0x10);
const _: () = assert!(size_of::<SpecialEffectEntryAccumulatorInfo>() == 0x20);
const _: () = assert!(offset_of!(SpecialEffectEntry, removal_timer) == 0x40);
const ENTRY_NEXT: usize = 0x30;
/// Never walk more entries than this (a corrupt list ends the scan instead of looping).
const MAX_ENTRIES: usize = 512;
// CSFeManImp.hud_state: u8 at +0x78 (HideAll = 0, Default = 3) [V crate source].
const _: () = assert!(offset_of!(CSFeManImp, hud_state) == 0x78);
pub const HUD_STATE: usize = offset_of!(CSFeManImp, hud_state);
pub const HUD_HIDE_ALL: u8 = 0;

/// The local player's ChrIns (= PlayerIns) address, 0 when not in world.
pub fn main_player() -> usize {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return 0;
    }
    // SAFETY: live WorldChrMan; main_player read as an integer.
    let p = unsafe { read_ptr(wcm + offset_of!(WorldChrMan, main_player)) };
    if plausible(p) { p } else { 0 }
}

/// The param ids of every SpEffect entry on `chr` (`None` = the list is unreadable).
pub fn speffect_ids(chr: usize) -> Option<Vec<i32>> {
    if !plausible(chr) {
        return None;
    }
    // SAFETY: non-faulting reads; every pointer is checked before it is followed; the walk is bounded.
    unsafe {
        let se = read_ptr(chr + offset_of!(ChrIns, special_effect));
        if !plausible(se) {
            return None;
        }
        let mut cur = read::<usize>(se + SPECIAL_EFFECT_HEAD)?;
        let mut out = Vec::new();
        while cur != 0 {
            if !plausible(cur) || out.len() >= MAX_ENTRIES {
                return None;
            }
            out.push(read::<i32>(cur + offset_of!(SpecialEffectEntry, param_id))?);
            cur = read::<usize>(cur + ENTRY_NEXT)?;
        }
        Some(out)
    }
}

/// How many entries of `id` the player carries now (`None` = unreadable).
pub fn speffect_count(chr: usize, id: i32) -> Option<usize> {
    speffect_ids(chr).map(|v| mw2er_core::ac130_protect::count(&v, id))
}

/// The address of `CSFeManImp.hud_state` and its current value (`None` = singleton absent / unreadable).
pub fn hud_state() -> Option<(usize, u8)> {
    let fe = singleton_addr::<CSFeManImp>();
    if fe == 0 {
        return None;
    }
    let addr = fe + HUD_STATE;
    // SAFETY: non-faulting read of one byte of the live CSFeManImp.
    unsafe { read::<u8>(addr) }.map(|v| (addr, v))
}
