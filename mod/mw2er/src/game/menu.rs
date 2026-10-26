//! Menu signals: `CSMenuManImp.ui_states` visible bits (bit 1) and `CSMouseMan.show_cursor`.

use std::mem::offset_of;

use eldenring::cs::{CSMenuManImp, CSMouseMan, UIState};
use mw2er_core::menu_logic::UI_STATE_COUNT;

use super::{read, singleton_addr};

const _: () = assert!(size_of::<UIState>() == 1);
const VISIBLE_BIT: u8 = 1 << 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct MenuSample {
    /// Bit i set = ui_states[i] visible.
    pub mask: u128,
    pub show_cursor: Option<bool>,
}

pub fn read_menu() -> Option<MenuSample> {
    let mm = singleton_addr::<CSMenuManImp>();
    if mm == 0 {
        return None;
    }
    // SAFETY: live CSMenuManImp; ui_states is a [u8-sized bitfield; 0x46] array.
    let states = unsafe { read::<[u8; UI_STATE_COUNT]>(mm + offset_of!(CSMenuManImp, ui_states)) }?;
    let mask = states
        .iter()
        .enumerate()
        .fold(0u128, |m, (i, &s)| if s & VISIBLE_BIT != 0 { m | (1u128 << i) } else { m });
    let mouse = singleton_addr::<CSMouseMan>();
    // SAFETY: mouse is 0 or the live CSMouseMan; the bool is read as a byte.
    let show_cursor = unsafe { read::<u8>(if mouse == 0 { 0 } else { mouse + offset_of!(CSMouseMan, show_cursor) }) }
        .map(|b| b != 0);
    Some(MenuSample { mask, show_cursor })
}
