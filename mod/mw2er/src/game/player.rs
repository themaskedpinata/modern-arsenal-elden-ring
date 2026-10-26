//! The local player (`WorldChrMan.main_player`). `None` means "not in world": title screen,
//! loading, or modules not populated yet.

use std::mem::offset_of;

use eldenring::cs::{FieldInsHandle, PlayerIns, WorldChrMan};

use super::chars::{ChrRead, read_chr};
use super::{read, read_ptr, singleton_addr};

const _: () = assert!(size_of::<FieldInsHandle>() == 8);
// Patch 0001: the empty handle is all ones (selector u32::MAX, block -1).
// SAFETY (const eval): FieldInsHandle is two 4-byte integers, no padding.
const _: () = assert!(unsafe { std::mem::transmute::<FieldInsHandle, u64>(FieldInsHandle::none()) } == u64::MAX);

#[derive(Clone, Copy, Debug)]
pub struct PlayerSample {
    pub chr: ChrRead,
    /// The PlayerIns address (cast_ray owner), valid this tick only.
    pub player_ins: usize,
    /// The owner handle for spawned bullets.
    pub field_ins_handle: FieldInsHandle,
}

pub fn read_player() -> Option<PlayerSample> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return None;
    }
    // SAFETY: wcm is the live WorldChrMan; `main_player` is an Option<OwnedPtr> read as an integer.
    let player = unsafe { read_ptr(wcm + offset_of!(WorldChrMan, main_player)) };
    if player == 0 {
        return None;
    }
    let chr_addr = player + offset_of!(PlayerIns, chr_ins);
    // SAFETY: main_player points to a live PlayerIns whose first field is its ChrIns.
    let chr = unsafe { read_chr(chr_addr) }?;
    // SAFETY: same object; FieldInsHandle is plain data (two u32 bitfields).
    let field_ins_handle =
        unsafe { read::<FieldInsHandle>(chr_addr + offset_of!(eldenring::cs::ChrIns, field_ins_handle)) }?;
    Some(PlayerSample { chr, player_ins: player, field_ins_handle })
}
