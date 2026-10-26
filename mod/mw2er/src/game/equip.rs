//! C6 part 16 (docs/C6_HOLD_DESIGN.md 16.3, work/c6/xbow8.md 2.1): non-faulting reads of the MAIN player's
//! EquipGameData (`[[GameDataMan]+8]+0x2B0`): the equipped slots, their inventory indices, the grip, the bolt slots and
//! the inventory entries. Read-only: the carrier changes the equipment only through ER's own EquipItem (carrier.rs).

use std::mem::offset_of;

use eldenring::cs::{ChrAsm, ChrAsmEquipment, ChrAsmEquipmentSlots, EquipGameData, EquipInventoryData, EquipInventoryDataListEntry, GameDataMan, InventoryItemsData, PlayerGameData, PlayerIns};
use mw2er_core::carrier::InvItem;

use super::{plausible, read, read_bytes, read_ptr};

/// The layout the exe research pinned (xbow8.md 2.1) is the vendored one.
pub const EGD_IN_PGD: usize = offset_of!(PlayerGameData, equipment);
const _: () = assert!(EGD_IN_PGD == 0x2B0);
pub const EGD_IDX_LIST: usize = offset_of!(EquipGameData, equipment_item_idx_list);
const _: () = assert!(EGD_IDX_LIST == 0x8);
pub const EGD_CHR_ASM: usize = offset_of!(EquipGameData, chr_asm);
const _: () = assert!(EGD_CHR_ASM == 0x6C);
pub const EGD_INVENTORY: usize = offset_of!(EquipGameData, equip_inventory_data);
const _: () = assert!(EGD_INVENTORY == 0x158);
pub const EGD_LAST_ADD: usize = offset_of!(EquipGameData, last_add_item_result);
const _: () = assert!(EGD_LAST_ADD == 0x3FC);
const ASM_ARM: usize = offset_of!(ChrAsm, equipment) + offset_of!(ChrAsmEquipment, arm_style);
const ASM_SEL_RIGHT: usize = offset_of!(ChrAsm, equipment) + offset_of!(ChrAsmEquipment, selected_slots) + offset_of!(ChrAsmEquipmentSlots, right_weapon_slot);
const ASM_HANDLES: usize = offset_of!(ChrAsm, gaitem_handles);
const ASM_IDS: usize = offset_of!(ChrAsm, equipment_param_ids);
pub const ASM_BOLT_LOADED: usize = offset_of!(ChrAsm, bolt_loaded_states);
const _: () = assert!(ASM_BOLT_LOADED == mw2er_core::carrier::ASM_BOLT_LOADED_OFF);
const _: () = assert!(EGD_CHR_ASM + ASM_ARM == 0x74&& EGD_CHR_ASM + ASM_HANDLES == 0x90 && EGD_CHR_ASM + ASM_IDS == 0xE8 && EGD_CHR_ASM + ASM_BOLT_LOADED == 0x148);
const INV_ITEMS: usize = offset_of!(EquipInventoryData, items_data);
const ENTRY_SIZE: usize = size_of::<EquipInventoryDataListEntry>();
const _: () = assert!(ENTRY_SIZE == 24);
const ENTRY_HANDLE: usize = offset_of!(EquipInventoryDataListEntry, gaitem_handle);
const ENTRY_ID: usize = offset_of!(EquipInventoryDataListEntry, item_id);
const ENTRY_QTY: usize = offset_of!(EquipInventoryDataListEntry, quantity);
/// The normal inventory is never scanned beyond this many entries (ER's capacity is ~2700).
const NORMAL_MAX: u32 = 8192;

/// One read of the main player's equipment (plain values; nothing outlives the tick).
#[derive(Clone, Debug, Default)]
pub struct EquipRead {
    pub egd: usize,
    pub idx: [i32; 22],
    pub handles: [u32; 22],
    pub ids: [i32; 22],
    pub arm_style: u32,
    pub right_sel: u32,
    pub bolt_loaded: [u8; 12],
    pub key_cap: u32,
    pub key_head: usize,
    pub key_len: u32,
    pub normal_cap: u32,
    pub normal_head: usize,
    pub normal_len: u32,
}

/// The main player's PlayerGameData through the pinned `GameDataMan` global (`base` = the exe image base).
pub fn main_pgd(base: usize, gdm_rva: usize) -> Option<usize> {
    if base == 0 {
        return None;
    }
    // SAFETY: non-faulting reads of the exe's pointer global and the singleton's field (integers, checked).
    let gdm = unsafe { read_ptr(base + gdm_rva) };
    if !plausible(gdm) {
        return None;
    }
    // SAFETY: as above.
    let pgd = unsafe { read_ptr(gdm + offset_of!(GameDataMan, main_player_game_data)) };
    plausible(pgd).then_some(pgd)
}

/// The equipment of `pgd` (None = unreadable / implausible).
pub fn read_equip(pgd: usize) -> Option<EquipRead> {
    let egd = pgd + EGD_IN_PGD;
    // SAFETY: non-faulting reads of plain integers inside the main player's EquipGameData.
    unsafe {
        let mut r = EquipRead { egd, ..Default::default() };
        r.idx = read::<[i32; 22]>(egd + EGD_IDX_LIST)?;
        let asm = egd + EGD_CHR_ASM;
        r.handles = read::<[u32; 22]>(asm + ASM_HANDLES)?;
        r.ids = read::<[i32; 22]>(asm + ASM_IDS)?;
        r.arm_style = read::<u32>(asm + ASM_ARM)?;
        r.right_sel = read::<u32>(asm + ASM_SEL_RIGHT)?;
        r.bolt_loaded = read::<[u8; 12]>(asm + ASM_BOLT_LOADED)?;
        let items = egd + EGD_INVENTORY + INV_ITEMS;
        r.key_cap = read::<u32>(items + offset_of!(InventoryItemsData, key_items_capacity))?;
        r.normal_cap = read::<u32>(items + offset_of!(InventoryItemsData, normal_items_capacity))?;
        r.normal_len = read::<u32>(items + offset_of!(InventoryItemsData, normal_items_len))?;
        r.normal_head = read_ptr(items + offset_of!(InventoryItemsData, normal_items_head));
        // the key list the game reads through (single player: the key items; in a session: the multiplayer copy)
        let acc = items + offset_of!(InventoryItemsData, key_items_accessor);
        r.key_head = read_ptr(acc);
        r.key_len = read::<u32>(read_ptr(acc + 8))?;
        if r.arm_style > 3 || r.right_sel > 2 || !plausible(r.normal_head) || r.normal_len > r.normal_cap || r.normal_cap > NORMAL_MAX {
            return None;
        }
        Some(r)
    }
}

fn entry_at(head: usize, k: usize, index: i32) -> Option<InvItem> {
    let a = head + k * ENTRY_SIZE;
    // SAFETY: non-faulting reads of one inventory entry.
    unsafe {
        Some(InvItem {
            index,
            handle: read::<u32>(a + ENTRY_HANDLE)?,
            item_id: read::<u32>(a + ENTRY_ID)?,
            qty: read::<u32>(a + ENTRY_QTY)?,
        })
    }
}

/// The inventory entry at ER's inventory index (key items first, then `key_cap + k`), None = out of range / unreadable.
pub fn entry(r: &EquipRead, index: i32) -> Option<InvItem> {
    let (key, k) = mw2er_core::carrier::inv_location(index, r.key_cap)?;
    if key {
        (k < r.key_len as usize && plausible(r.key_head)).then(|| entry_at(r.key_head, k, index)).flatten()
    } else {
        (k < r.normal_len as usize).then(|| entry_at(r.normal_head, k, index)).flatten()
    }
}

/// Every non-empty NORMAL inventory entry (weapons live there), read in one non-faulting copy.
pub fn scan_normal(r: &EquipRead) -> Option<Vec<InvItem>> {
    let n = r.normal_len as usize;
    let mut buf = vec![0u8; n * ENTRY_SIZE];
    if n > 0 && !read_bytes(r.normal_head, &mut buf) {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    Some(
        (0..n)
            .map(|k| {
                let o = k * ENTRY_SIZE;
                InvItem { index: (r.key_cap as usize + k) as i32, handle: u32_at(o + ENTRY_HANDLE), item_id: u32_at(o + ENTRY_ID), qty: u32_at(o + ENTRY_QTY) }
            })
            .filter(|e| e.item_id != u32::MAX && e.item_id != 0 && e.handle != 0)
            .collect(),
    )
}

/// The summed quantity of the equipped bolt slots (Bolt1 / Bolt2: ChrAsm slots 7 / 9) through their inventory entries.
pub fn bolt_qty(r: &EquipRead) -> Option<u32> {
    let mut q = 0u32;
    for s in [mw2er_core::carrier::SLOT_BOLT1, mw2er_core::carrier::SLOT_BOLT2] {
        if r.ids[s] <= 0 || r.idx[s] < 0 {
            continue;
        }
        let e = entry(r, r.idx[s])?;
        if e.handle != r.handles[s] {
            return None;
        }
        q = q.saturating_add(e.qty);
    }
    Some(q)
}

/// C6 part 16 addendum (`HOLD aim episode bolts=`): the main player's equipped bolt quantity, with or without the
/// carrier (2.7.2.0 only: the carrier's pinned `GameDataMan`; None elsewhere / unreadable). Read-only.
pub fn main_bolt_qty() -> Option<u32> {
    crate::version_gate::own().ok().and_then(|o| o.carrier)?;
    let pgd = main_pgd(crate::game::c3_read::exe_base(), mw2er_core::carrier::CARRIER_2720.game_data_man)?;
    bolt_qty(&read_equip(pgd)?)
}

/// The character's name (PlayerGameData, UTF-16, at most 16 characters).
pub fn character_name(pgd: usize) -> Option<String> {
    // SAFETY: a non-faulting read of a plain u16 array.
    let w = unsafe { read::<[u16; 17]>(pgd + offset_of!(PlayerGameData, character_name)) }?;
    let n = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    Some(String::from_utf16_lossy(&w[..n]))
}

/// C6 part 16.16: the address of our PlayerIns's ChrAsm (None = no player / implausible). Read-only.
///
/// # Safety
/// `player` must be 0 or our live PlayerIns this tick.
pub unsafe fn player_asm(player: usize) -> Option<usize> {
    if !plausible(player) {
        return None;
    }
    // SAFETY: the caller's contract; a non-faulting pointer read.
    let asm = unsafe { read_ptr(player + offset_of!(PlayerIns, chr_asm)) };
    plausible(asm).then_some(asm)
}

/// The PlayerIns ChrAsm (what ER draws and syncs): the param id in slot `s` and its `bolt_loaded_states[s]`.
///
/// # Safety
/// `player` must be 0 or our live PlayerIns this tick.
pub unsafe fn player_slot(player: usize, s: usize) -> Option<(i32, u32, bool)> {
    if !plausible(player) || s >= 22 {
        return None;
    }
    // SAFETY: the caller's contract; the ChrAsm pointer is checked; plain integers read by offset.
    unsafe {
        let asm = read_ptr(player + offset_of!(PlayerIns, chr_asm));
        if !plausible(asm) {
            return None;
        }
        let id = read::<i32>(asm + ASM_IDS + 4 * s)?;
        let h = read::<u32>(asm + ASM_HANDLES + 4 * s)?;
        let loaded = if s < 12 { read::<u8>(asm + ASM_BOLT_LOADED + s)? != 0 } else { false };
        Some((id, h, loaded))
    }
}
