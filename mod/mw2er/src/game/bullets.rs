//! Live bullets (`CSBulletManager.bullets` linked list), filtered to ours, as plain data. Used only
//! for the `BULLET` / `IMPACT` diagnostic lines (docs/M2_DESIGN.md section 12.2).

use std::collections::HashSet;
use std::mem::offset_of;

use eldenring::cs::{BufferAndAllocLinkedList, CSBulletIns, CSBulletManager};
use mw2er_core::spawn_cause::{BULLET_HEAP_CAP, ListSnap, PoolSnap, SFX_HEAP_CAP, UNK40_HEAP_CAP};
use mw2er_core::vec3::V3;

use super::{plausible, read, read_ptr, singleton_addr, v3};

type BulletList = BufferAndAllocLinkedList<CSBulletIns, 128, 128>;

/// Hard cap on list walking (the manager holds 128 preallocated + 128 heap bullets).
const MAX_WALK: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct BulletSample {
    pub handle: u64,
    pub param_id: i32,
    pub pos: V3,
    pub vel: V3,
    pub time_alive: f32,
}

// ---- M6 bug 2: read-only pool counters (docs/M6_DESIGN.md section 10.2) ---------------------------------------

/// The layout of one `BufferAndAllocLinkedList` (its counters are private in the vendored crate): read by offset.
#[repr(C)]
struct ListMirror {
    prealloc_buffer: u64,
    head: u64,
    empty_spot: u64,
    allocated_count: u32,
    allocations_counter: u32,
}

const _: () = assert!(size_of::<ListMirror>() == size_of::<BulletList>());
/// The three lists sit back to back at +0x00 / +0x20 / +0x40, then `chr_cam` (public) at +0x60 [V vendored layout].
const _: () = assert!(offset_of!(CSBulletManager, chr_cam) == 3 * size_of::<ListMirror>());
pub const POOL_BULLETS_OFF: usize = 0x00;
pub const POOL_SFX_OFF: usize = 0x20;
pub const POOL_UNK40_OFF: usize = 0x40;

/// One list's counters (no writes; plausibility-checked pointers only).
fn list_snap(bm: usize, off: usize, cap: u32) -> ListSnap {
    // SAFETY: non-faulting reads of plain integers inside the live CSBulletManager.
    unsafe {
        let head = read::<u64>(bm + off + offset_of!(ListMirror, head)).unwrap_or(0);
        let empty = read::<u64>(bm + off + offset_of!(ListMirror, empty_spot)).unwrap_or(0);
        let alloc = read::<u32>(bm + off + offset_of!(ListMirror, allocated_count)).unwrap_or(0);
        let counter = read::<u32>(bm + off + offset_of!(ListMirror, allocations_counter)).unwrap_or(0);
        ListSnap { has_head: head != 0, empty_spot: empty != 0, allocated: alloc, heap_cap: cap, counter }
    }
}

/// The pools of CSBulletManager + live bullets (all, ours) by walking the bullet list. `None` without a manager.
pub fn pool_snapshot(owner: u64, ids: &HashSet<i32>, spawns_world: u32) -> Option<PoolSnap> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return None;
    }
    let mut p = PoolSnap {
        bullets: list_snap(bm, POOL_BULLETS_OFF, BULLET_HEAP_CAP),
        sfx: list_snap(bm, POOL_SFX_OFF, SFX_HEAP_CAP),
        unk40: list_snap(bm, POOL_UNK40_OFF, UNK40_HEAP_CAP),
        live_all: 0,
        live_ours: 0,
        spawns_world,
    };
    // SAFETY: live CSBulletManager; `head` is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + offset_of!(CSBulletManager, bullets) + offset_of!(BulletList, head)) };
    let mut walked = 0;
    while plausible(cur) && walked < MAX_WALK {
        walked += 1;
        p.live_all += 1;
        // SAFETY: cur is a live list node this tick; plain data read by offset.
        let (o, pid) = unsafe {
            (read::<u64>(cur + offset_of!(CSBulletIns, targeting_owner.owner_chr_handle)), read::<i32>(cur + offset_of!(CSBulletIns, fly_state.base.param.param_id)))
        };
        if o == Some(owner) && pid.is_some_and(|i| ids.contains(&i)) {
            p.live_ours += 1;
        }
        // SAFETY: next_bullet is an Option<NonNull> read as an integer.
        cur = unsafe { read_ptr(cur + offset_of!(CSBulletIns, next_bullet)) };
    }
    Some(p)
}

// ---- M7 A1: the bullet-emitter list (docs/M7_DESIGN.md 2.1) ----------------------------------------------------
// The +0x20 list ("sfx" since M6) holds ER's bullet emitters (0x9d0 bytes each, 64 + 192). Node layout [V disassembly
// of 2.7.1.0 unless marked]: next at +0x998 (chaining at 0x1403a5bb7..0x1403a5bd2), +0x320 u32 live bullets [I],
// +0x329 emitting (u8), +0x334 bullet id (i32), +0x340 owner FieldInsHandle (u64), +0x4C4 auto-finish (u8).

/// The emitter list head (`BufferAndAllocLinkedList.head` of the +0x20 list).
pub const EMITTER_HEAD_OFF: usize = POOL_SFX_OFF + offset_of!(ListMirror, head);
const _: () = assert!(EMITTER_HEAD_OFF == 0x28);
const EM_NEXT: usize = 0x998;
const EM_EMITTING: usize = 0x329;
const EM_BULLET_ID: usize = 0x334;
const EM_OWNER: usize = 0x340;
const EM_AUTO_FINISH: usize = 0x4C4;
/// Never walk more nodes than this (the pool holds 256).
const EM_MAX_WALK: usize = 300;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmitterWalk {
    pub live: u32,
    /// Owner = the player and the bullet id one of our rows.
    pub ours: u32,
    /// Ours, still emitting, auto-finish clear: never freed before a reload (must be 0 after patch 0003).
    pub persistent: u32,
    /// The walk hit EM_MAX_WALK or an implausible pointer.
    pub truncated: bool,
}

/// The cheap upper bound of live emitters (2 reads): see `mw2er_core::emitter_guard::live_upper`.
pub fn emitter_upper() -> Option<u32> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return None;
    }
    let s = list_snap(bm, POOL_SFX_OFF, SFX_HEAP_CAP);
    Some(mw2er_core::emitter_guard::live_upper(s.empty_spot, s.allocated))
}

/// Count-only walk of the live emitters (1 read per node, at most EM_MAX_WALK nodes): the guard's correction of the
/// counter bound once it reaches `warn`. `None` when the manager is missing or the walk was truncated (a broken or
/// longer-than-possible list: the bound decides then).
pub fn emitter_count_walk() -> Option<u32> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return None;
    }
    // SAFETY: live CSBulletManager; the head is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + EMITTER_HEAD_OFF) };
    let mut n = 0u32;
    while cur != 0 {
        if !plausible(cur) || n as usize >= EM_MAX_WALK {
            return None;
        }
        n += 1;
        // SAFETY: the next pointer of a live node, read as an integer.
        cur = unsafe { read_ptr(cur + EM_NEXT) };
    }
    Some(n)
}

/// Read-only walk of every live emitter (with each `SPAWN pool` line, not per shot).
pub fn emitter_walk(owner: u64, ids: &HashSet<i32>) -> Option<EmitterWalk> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return None;
    }
    let mut w = EmitterWalk::default();
    // SAFETY: live CSBulletManager; the head is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + EMITTER_HEAD_OFF) };
    while cur != 0 {
        if !plausible(cur) || w.live as usize >= EM_MAX_WALK {
            w.truncated = true;
            break;
        }
        w.live += 1;
        // SAFETY: cur is a live list node this tick; plain data read by offset with non-faulting reads.
        let (o, id, emitting, auto) = unsafe {
            (read::<u64>(cur + EM_OWNER), read::<i32>(cur + EM_BULLET_ID), read::<u8>(cur + EM_EMITTING), read::<u8>(cur + EM_AUTO_FINISH))
        };
        if o == Some(owner) && id.is_some_and(|i| ids.contains(&i)) {
            w.ours += 1;
            if emitting.is_some_and(|e| e != 0) && auto == Some(0) {
                w.persistent += 1;
            }
        }
        // SAFETY: the next pointer of the node, read as an integer.
        cur = unsafe { read_ptr(cur + EM_NEXT) };
    }
    Some(w)
}

/// Bullets owned by `owner` whose param row id is in `ids`.
pub fn read_ours(owner: u64, ids: &HashSet<i32>) -> Vec<BulletSample> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: live CSBulletManager; `head` is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + offset_of!(CSBulletManager, bullets) + offset_of!(BulletList, head)) };
    let mut walked = 0;
    while plausible(cur) && walked < MAX_WALK {
        walked += 1;
        let b = cur;
        // SAFETY: b is a live list node this tick; all fields are plain data read by offset.
        let sample = unsafe {
            let owner_h = read::<u64>(b + offset_of!(CSBulletIns, targeting_owner.owner_chr_handle));
            let param_id = read::<i32>(b + offset_of!(CSBulletIns, fly_state.base.param.param_id));
            match (owner_h, param_id) {
                (Some(o), Some(pid)) if o == owner && ids.contains(&pid) => {
                    let handle = read::<u64>(b + offset_of!(CSBulletIns, field_ins_handle)).unwrap_or(0);
                    let pos = read::<[f32; 4]>(b + offset_of!(CSBulletIns, physics.position)).and_then(v3);
                    let vel = read::<[f32; 4]>(b + offset_of!(CSBulletIns, physics.velocity)).and_then(v3);
                    let time_alive = read::<f32>(b + offset_of!(CSBulletIns, time_alive)).unwrap_or(0.0);
                    match (pos, vel) {
                        (Some(pos), Some(vel)) => Some(BulletSample { handle, param_id: pid, pos, vel, time_alive }),
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        if let Some(s) = sample {
            out.push(s);
        }
        // SAFETY: next_bullet is an Option<NonNull> read as an integer.
        cur = unsafe { read_ptr(b + offset_of!(CSBulletIns, next_bullet)) };
    }
    out
}

/// C3 PASS C (docs/C3_DESIGN.md 3.5): every live bullet of a co-op twin / nuke-marker row that the local player does
/// not own (the peers' synced spawns ER created on this PC), as plain data. Read-only, bounded walk.
pub fn read_remote(local_owner: u64) -> Vec<mw2er_core::coop_vis::RemoteBullet> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: live CSBulletManager; `head` is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + offset_of!(CSBulletManager, bullets) + offset_of!(BulletList, head)) };
    let mut walked = 0;
    while plausible(cur) && walked < MAX_WALK {
        walked += 1;
        let b = cur;
        // SAFETY: b is a live list node this tick; every field is plain data read by offset (non-faulting reads).
        unsafe {
            let pid = read::<i32>(b + offset_of!(CSBulletIns, fly_state.base.param.param_id)).unwrap_or(0);
            if mw2er_core::coop_vis::syncable_bullet(pid) {
                let owner = read::<u64>(b + offset_of!(CSBulletIns, targeting_owner.owner_chr_handle)).unwrap_or(0);
                if owner != local_owner {
                    let handle = read::<u64>(b + offset_of!(CSBulletIns, field_ins_handle)).unwrap_or(0);
                    let pos = read::<[f32; 4]>(b + offset_of!(CSBulletIns, physics.position)).and_then(v3);
                    let vel = read::<[f32; 4]>(b + offset_of!(CSBulletIns, physics.velocity)).and_then(v3).unwrap_or(V3::ZERO);
                    if let Some(pos) = pos {
                        out.push(mw2er_core::coop_vis::RemoteBullet { handle, owner, param_id: pid, pos, vel });
                    }
                }
            }
            // SAFETY: next_bullet is an Option<NonNull> read as an integer.
            cur = read_ptr(b + offset_of!(CSBulletIns, next_bullet));
        }
    }
    out
}

/// C6 (docs/C6_HOLD_DESIGN.md 3.5): every live bullet owned by `owner` whose row is NOT one of the mod's
/// (`er_stance::mod_row`): (bullet handle, row). The stray-bullet guard of the crossbow stance. Read-only, bounded walk.
pub fn read_owned_foreign(owner: u64) -> Vec<(u64, i32)> {
    let bm = singleton_addr::<CSBulletManager>();
    if bm == 0 || owner == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: live CSBulletManager; `head` is an Option<NonNull> read as an integer.
    let mut cur = unsafe { read_ptr(bm + offset_of!(CSBulletManager, bullets) + offset_of!(BulletList, head)) };
    let mut walked = 0;
    while plausible(cur) && walked < MAX_WALK && out.len() < 64 {
        walked += 1;
        let b = cur;
        // SAFETY: b is a live list node this tick; every field is plain data read by offset (non-faulting reads).
        unsafe {
            let pid = read::<i32>(b + offset_of!(CSBulletIns, fly_state.base.param.param_id)).unwrap_or(0);
            if pid > 0 && !mw2er_core::er_stance::mod_row(pid) && read::<u64>(b + offset_of!(CSBulletIns, targeting_owner.owner_chr_handle)) == Some(owner) {
                let handle = read::<u64>(b + offset_of!(CSBulletIns, field_ins_handle)).unwrap_or(0);
                out.push((handle, pid));
            }
            // SAFETY: next_bullet is an Option<NonNull> read as an integer.
            cur = read_ptr(b + offset_of!(CSBulletIns, next_bullet));
        }
    }
    out
}
