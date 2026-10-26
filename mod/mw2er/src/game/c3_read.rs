//! C3 (docs/C3_DESIGN.md 1.6 / 2.3 / 2.6): read-only, non-faulting reads of
//! - the player's collision filter words in `CSChrPhysicsModule` (+0x338 main proxy, +0x348 second proxy, +0xC8:
//!   u32 words the setter 0x14045ff10 stores [V exe]; the vendored crate's `unkc8: f32` is wrong),
//! - the live layer table rows of `CSCollisionFilter` (`CSHavokMan+0xC0` [I] -> filter, vtable 0x142b94df0 [V],
//!   rows at +0x20, 16 bytes each [V]),
//! - ER's UI message signals (CSFeManImp / CSMenuManImp / CSPopupMenu / FeSystemAnnounceView fields of the vendored
//!   fromsoftware-rs @59fbd3b, offsets by `offset_of!`) for the UI yield,
//! - the handles of ER's enemy tags (damage numbers `suppress_with_er_tag`).
//!
//! Nothing here writes. Every pointer is read as an integer and plausibility-checked; a failed read is `None` / 0.

use std::mem::offset_of;

use eldenring::cs::{CSFeManImp, CSMenuManImp, CSPopupMenu, ChrEnemyTagEntry, FeSystemAnnounceView, FeSystemAnnounceViewModel, FrontEndViewValues};
use mw2er_core::ui_yield::{SIG_ANNOUNCE, SIG_BLINK, SIG_POPUP, SIG_STATUS, SIG_SUMMON};

use super::{plausible, read, read_ptr, singleton_addr};

/// `CSChrPhysicsModule` filter words [V exe: setter 0x14045ff10].
pub const PHYS_FILTER_MAIN: usize = 0x338;
pub const PHYS_FILTER_SECOND: usize = 0x348;
pub const PHYS_FILTER_C8: usize = 0xC8;
/// Review C3 [V exe]: the setter's only caller (0x1403cb1f2, in 0x1403caeb0) builds the +0x338 word from the BASE word at
/// +0x2E8 (getter 0x14045ddc0; +0x2F8 = the second proxy's base, getter 0x14045ddd0): layer = base & 0x7F, remapped by
/// the character's state flags (ChrCtrl? +0xF0 [I]) into the variant layers 0xF..0x1A / 1 / 3. Logged beside +0x338 so a
/// run shows the player's base layer and its live (remapped) layer.
pub const PHYS_FILTER_BASE: usize = 0x2E8;
pub const PHYS_FILTER_BASE2: usize = 0x2F8;
/// `CSHavokMan+0xC0` -> CSCollisionFilter [I]; the table offset [V]. Its vtable rva [V] is per exe build
/// (`OwnAddrs::filter_vtable`: 0x2b94df0 on 2.7.1.0, 0x2b96ee0 on 2.7.2.0, RTTI `.?AVCSCollisionFilter@CS@@`).
pub const HAVOK_FILTER_PTR: usize = 0xC0;
pub const FILTER_TABLE: usize = 0x20;

// C3 (2.1, c3_offsets_): the vendored layouts the UI yield reads; a crate update that moves them fails the build.
const _: () = assert!(offset_of!(CSFeManImp, hud_state) == 0x78);
const _: () = assert!(offset_of!(CSMenuManImp, ui_states) == 0x90);
const _: () = assert!(offset_of!(CSMenuManImp, popup_menu) == 0x80);
const _: () = assert!(offset_of!(CSMenuManImp, system_announce_view_model) == 0x860);
const _: () = assert!(offset_of!(CSFeManImp, frontend_values) == 0x80);

/// The `hud_state` value of an open popup menu (`CSFeManHudState::PopupMenu`), read as a byte.
const HUD_STATE_POPUP: u8 = 2;
/// The size field of a DLDeque (allocator, proxy, map, map size, offset, size) [V design 2.1: deque+0x28].
const DEQUE_SIZE: usize = 0x28;

/// The three filter words of the player's physics module (None = unreadable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FilterWords {
    pub main: Option<u32>,
    pub second: Option<u32>,
    pub c8: Option<u32>,
    /// Review C3: the base words the remap starts from (+0x2E8 / +0x2F8).
    pub base: Option<u32>,
    pub base2: Option<u32>,
}

pub fn read_filter_words(physics: usize) -> FilterWords {
    if !plausible(physics) {
        return FilterWords::default();
    }
    // SAFETY: non-faulting reads of u32 words of the live physics module (any bit pattern is valid).
    unsafe {
        FilterWords {
            main: read::<u32>(physics + PHYS_FILTER_MAIN),
            second: read::<u32>(physics + PHYS_FILTER_SECOND),
            c8: read::<u32>(physics + PHYS_FILTER_C8),
            base: read::<u32>(physics + PHYS_FILTER_BASE),
            base2: read::<u32>(physics + PHYS_FILTER_BASE2),
        }
    }
}

/// A live table row (16 bytes) of `CSCollisionFilter`, or why it could not be read.
pub fn read_table_row(layer: u8) -> Result<[u8; 16], &'static str> {
    let hm = singleton_addr::<eldenring::cs::CSHavokMan>();
    if hm == 0 {
        return Err("no_havok_man");
    }
    // SAFETY: non-faulting pointer read.
    let filter = unsafe { read_ptr(hm + HAVOK_FILTER_PTR) };
    if !plausible(filter) {
        return Err("filter_ptr");
    }
    // SAFETY: as above; the first qword of the filter object is its vtable.
    let vt = unsafe { read_ptr(filter) };
    let Ok(own) = crate::version_gate::own() else {
        return Err("no_build");
    };
    if vt != exe_base() + own.filter_vtable {
        return Err("vtable");
    }
    // SAFETY: 16 plain bytes.
    unsafe { read::<[u8; 16]>(filter + FILTER_TABLE + 16 * (layer & 0x7f) as usize) }.ok_or("row_unreadable")
}

pub(crate) fn exe_base() -> usize {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: `None` = the exe module, loaded for the process lifetime.
    unsafe { GetModuleHandleW(None) }.map(|h| h.0 as usize).unwrap_or(0)
}

/// ER's message signals this tick (`ui_yield::SIG_*` bits; the configured indices are added by the task).
#[derive(Clone, Copy, Debug, Default)]
pub struct UiSignals {
    pub bits: u32,
    /// For the learn / transition lines: the popup deque size, the announce play state, the status ring indices.
    pub popups: u64,
    pub announce: u8,
    pub status_rw: (u32, u32),
}

/// Reads the signals selected by `use_*` (the vendored fields; a deque's map is never walked, only its size read).
pub fn read_ui_signals(use_popup: bool, use_announce: bool, use_status: bool, use_summon: bool, use_blink: bool) -> UiSignals {
    let mut s = UiSignals::default();
    let fe = singleton_addr::<CSFeManImp>();
    let mm = singleton_addr::<CSMenuManImp>();
    if use_popup {
        // SAFETY: non-faulting reads (a byte, a pointer, a u64 size).
        let hud = if fe != 0 { unsafe { read::<u8>(fe + offset_of!(CSFeManImp, hud_state)) } } else { None };
        let popup = if mm != 0 { unsafe { read_ptr(mm + offset_of!(CSMenuManImp, popup_menu)) } } else { 0 };
        let n = if plausible(popup) { unsafe { read::<u64>(popup + offset_of!(CSPopupMenu, popup_messages) + DEQUE_SIZE) }.unwrap_or(0) } else { 0 };
        // a garbage size is not a message
        s.popups = if n <= 64 { n } else { 0 };
        if hud == Some(HUD_STATE_POPUP) || s.popups > 0 {
            s.bits |= SIG_POPUP;
        }
    }
    if use_announce && mm != 0 {
        // SAFETY: non-faulting reads of the view model pointer, its view pointer and two bytes of the view.
        let vm = unsafe { read_ptr(mm + offset_of!(CSMenuManImp, system_announce_view_model)) };
        let view = if plausible(vm) { unsafe { read_ptr(vm + offset_of!(FeSystemAnnounceViewModel, view)) } } else { 0 };
        if plausible(view) {
            let vis = unsafe { read::<u8>(view + offset_of!(FeSystemAnnounceView, is_visible)) }.unwrap_or(0);
            let st = unsafe { read::<u8>(view + offset_of!(FeSystemAnnounceView, announce_play_state)) }.unwrap_or(0);
            s.announce = st;
            if vis != 0 || st != 0 {
                s.bits |= SIG_ANNOUNCE;
            }
        }
    }
    if fe != 0 && (use_status || use_blink || use_summon) {
        if use_status {
            // SAFETY: non-faulting u32 reads (the proc status ring indices).
            let r = unsafe { read::<u32>(fe + offset_of!(CSFeManImp, proc_status_messages_read_index)) };
            let w = unsafe { read::<u32>(fe + offset_of!(CSFeManImp, proc_status_messages_write_index)) };
            if let (Some(r), Some(w)) = (r, w) {
                s.status_rw = (r, w);
                if r != w && r < 6 && w < 6 {
                    s.bits |= SIG_STATUS;
                }
            }
            // the status banner's timer (FrontEndViewValues inside CSFeManImp): running between 0 and 3 s
            let tv = unsafe { read::<f32>(fe + offset_of!(CSFeManImp, frontend_values) + offset_of!(FrontEndViewValues, proc_status_message_timer)) };
            if tv.is_some_and(|t| t > 0.0 && t < 3.0) {
                s.bits |= SIG_STATUS;
            }
        }
        if use_blink {
            // SAFETY: non-faulting reads of i32 / bool fields.
            let blink = unsafe { read::<i32>(fe + offset_of!(CSFeManImp, blinking_message_id)) }.unwrap_or(0);
            let sub = unsafe { read::<i32>(fe + offset_of!(CSFeManImp, subarea_name_popup_message_id)) }.unwrap_or(0);
            let welcome = unsafe { read::<u8>(fe + offset_of!(CSFeManImp, area_welcome_message_request)) }.unwrap_or(0);
            let full = unsafe { read::<i32>(fe + offset_of!(CSFeManImp, frontend_values) + offset_of!(FrontEndViewValues, full_screen_message_request_id)) }
                .unwrap_or(0);
            if blink > 0 || sub > 0 || welcome != 0 || full > 0 {
                s.bits |= SIG_BLINK;
            }
        }
        if use_summon {
            // the summon message queue's current entry priority [I]: non-zero while a summon message plays
            let off = offset_of!(CSFeManImp, summon_msg_queue) + 8 + 8;
            // SAFETY: a non-faulting i16 read.
            let prio = unsafe { read::<i16>(fe + off) }.unwrap_or(0);
            if prio > 0 {
                s.bits |= SIG_SUMMON;
            }
        }
    }
    s
}

/// The handles ER's enemy tags show (0 = empty slot).
pub fn read_enemy_tag_handles() -> [u64; 8] {
    let mut out = [0u64; 8];
    let fe = singleton_addr::<CSFeManImp>();
    if fe == 0 {
        return out;
    }
    let base = fe + offset_of!(CSFeManImp, enemy_chr_tag_displays);
    for (i, o) in out.iter_mut().enumerate() {
        // SAFETY: a non-faulting u64 read of the FieldInsHandle at the start of each entry.
        *o = unsafe { read::<u64>(base + i * size_of::<ChrEnemyTagEntry>() + offset_of!(ChrEnemyTagEntry, field_ins_handle)) }.unwrap_or(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/C3_DESIGN.md 2.7 `c3_offsets_`: the const asserts above compile; this test names them for the checks.
    #[test]
    fn c3_offsets_vendored_ui_fields() {
        assert_eq!(offset_of!(CSFeManImp, hud_state), 0x78);
        assert_eq!(offset_of!(CSMenuManImp, ui_states), 0x90);
        assert_eq!(offset_of!(CSMenuManImp, popup_menu), 0x80);
        assert_eq!(offset_of!(CSMenuManImp, system_announce_view_model), 0x860);
        assert_eq!(offset_of!(CSPopupMenu, current_talk_id), 0x180);
        assert_eq!(PHYS_FILTER_MAIN, 0x338);
        assert_eq!((PHYS_FILTER_BASE, PHYS_FILTER_BASE2), (0x2E8, 0x2F8));
        assert_eq!(mw2er_core::exe_build::OWN_2710.filter_vtable + 0x1_4000_0000, 0x1_42b9_4df0);
        assert_eq!(mw2er_core::exe_build::OWN_2720.filter_vtable + 0x1_4000_0000, 0x1_42b9_6ee0);
    }
}
