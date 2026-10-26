//! Characters: one `ChrIns` read into plain data, and the `WorldChrMan.chr_inses_by_distance` list.

use std::mem::{offset_of, size_of};

use eldenring::DLVector;
use eldenring::cs::{
    CSChrDataModule, CSChrPhysicsModule, ChrIns, ChrInsDistanceEntry, ChrInsModuleContainer, ChrSet, ChrSetEntry, OpenFieldChrSet, WorldChrMan,
};
use mw2er_core::vec3::V3;

use super::{plausible, read, read_ptr, singleton_addr, v3};

// The vector is read raw: MSVC std::vector with the allocator first (crate feature msvc2015),
// then first / last / end pointers.
const _: () = assert!(size_of::<DLVector<ChrInsDistanceEntry>>() == 32);
const _: () = assert!(offset_of!(DLVector<ChrInsDistanceEntry>, allocator) == 0);
const VEC_FIRST: usize = 8;
const VEC_LAST: usize = 16;
const _: () = assert!(size_of::<ChrInsDistanceEntry>() == 16);
const _: () = assert!(offset_of!(ChrInsDistanceEntry, chr_ins) == 0);

/// One character as plain data.
#[derive(Clone, Copy, Debug)]
pub struct ChrRead {
    pub handle: u64,
    pub character_id: u32,
    pub hp: i32,
    pub max_hp: i32,
    pub last_hit_by: u64,
    /// Havok-space position (metres).
    pub pos: V3,
    pub pos_w: f32,
}

/// Reads one `ChrIns` at `chr`.
///
/// # Safety
/// `chr` must be 0 or the address of a live `ChrIns` (from the game's own lists this tick).
pub(crate) unsafe fn read_chr(chr: usize) -> Option<ChrRead> {
    if !plausible(chr) {
        return None;
    }
    // SAFETY (all reads below): `chr` is a live ChrIns; offsets are the crate's repr(C) layout;
    // every intermediate pointer is null/plausibility-checked before it is followed.
    unsafe {
        let handle = read::<u64>(chr + offset_of!(ChrIns, field_ins_handle))?;
        let character_id = read::<u32>(chr + offset_of!(ChrIns, character_id))?;
        let last_hit_by = read::<u64>(chr + offset_of!(ChrIns, last_hit_by))?;
        let modules = read_ptr(chr + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return None;
        }
        let data = read_ptr(modules + offset_of!(ChrInsModuleContainer, data));
        let physics = read_ptr(modules + offset_of!(ChrInsModuleContainer, physics));
        if !plausible(data) || !plausible(physics) {
            return None;
        }
        let hp = read::<i32>(data + offset_of!(CSChrDataModule, hp))?;
        let max_hp = read::<i32>(data + offset_of!(CSChrDataModule, max_hp))?;
        let p = read::<[f32; 4]>(physics + offset_of!(CSChrPhysicsModule, position))?;
        Some(ChrRead { handle, character_id, hp, max_hp, last_hit_by, pos: v3(p)?, pos_w: p[3] })
    }
}

// ---- M7 nuke census (docs/M7_DESIGN.md 3.11) ----------------------------------------------------------------------

const _: () = assert!(size_of::<ChrSet<ChrIns>>() == size_of::<ChrSet<eldenring::cs::PlayerIns>>());
const _: () = assert!(size_of::<ChrSetEntry<ChrIns>>() == 16);
const _: () = assert!(offset_of!(OpenFieldChrSet, base) == 0);
/// Never read more entries of one set than this.
const SET_MAX: usize = 4096;
/// The runtime team type (`ChrIns.team_type`, +0x6c in the vendored layout; [I] until run 15 logs it).
pub const TEAM_TYPE_OFF: usize = offset_of!(ChrIns, team_type);

/// One character with the fields the nuke's target selection needs.
#[derive(Clone, Copy, Debug)]
pub struct NukeChr {
    pub addr: usize,
    pub c: ChrRead,
    pub npc_param_id: i32,
    pub team: u8,
    pub chr_type: i32,
    pub flags1c5: u8,
    pub flags1c8: u8,
    /// C3 co-op nuke (docs/C3_DESIGN.md 3.8): `ChrSetEntry.chr_update_type` of the set entry holding this character
    /// (0 Local, 4 Remote; `None` = in no set the census walked), `ChrIns.network_authority` and NetChrSyncFlags bit 5
    /// (distance-based network update authority). Logged; the owner rule uses `upd`.
    pub upd: Option<u8>,
    pub net_auth: u32,
    pub dist_auth: bool,
}

impl NukeChr {
    pub fn dead_flag(&self) -> bool {
        self.flags1c5 & 0x80 != 0
    }

    pub fn invincible(&self) -> bool {
        self.flags1c5 & 0x10 != 0
    }

    pub fn active(&self) -> bool {
        self.flags1c8 & 0x10 != 0
    }
}

/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_nuke_chr(addr: usize) -> Option<NukeChr> {
    // SAFETY: the caller's contract; every field is plain data read by offset with non-faulting reads.
    unsafe {
        let c = read_chr(addr)?;
        Some(NukeChr {
            addr,
            c,
            npc_param_id: read::<i32>(addr + offset_of!(ChrIns, npc_param_id))?,
            team: read::<u8>(addr + TEAM_TYPE_OFF)?,
            chr_type: read::<i32>(addr + offset_of!(ChrIns, chr_type))?,
            flags1c5: read::<u8>(addr + offset_of!(ChrIns, chr_flags1c5))?,
            flags1c8: read::<u8>(addr + offset_of!(ChrIns, chr_flags1c8))?,
            upd: None,
            net_auth: read::<u32>(addr + offset_of!(ChrIns, network_authority)).unwrap_or(u32::MAX),
            dist_auth: read::<u8>(addr + offset_of!(ChrIns, net_chr_sync_flags)).is_some_and(|f| f & (1 << 5) != 0),
        })
    }
}

/// C3: every entry of one ChrSet (any load status) as (ChrIns address, chr_update_type) for the owner rule.
fn set_update_types(set: usize, out: &mut std::collections::HashMap<usize, u8>) {
    if !plausible(set) {
        return;
    }
    // SAFETY: non-faulting reads of the set header and its entries; every pointer is checked.
    unsafe {
        let cap = read::<u32>(set + offset_of!(ChrSet<ChrIns>, capacity)).unwrap_or(0) as usize;
        let entries = read_ptr(set + offset_of!(ChrSet<ChrIns>, entries));
        if !plausible(entries) || cap == 0 {
            return;
        }
        let stride = size_of::<ChrSetEntry<ChrIns>>();
        for i in 0..cap.min(SET_MAX) {
            let e = entries + i * stride;
            let chr = read_ptr(e + offset_of!(ChrSetEntry<ChrIns>, chr_ins));
            if chr == 0 || !plausible(chr) {
                continue;
            }
            if let Some(u) = read::<u8>(e + offset_of!(ChrSetEntry<ChrIns>, chr_update_type)) {
                out.insert(chr, u);
            }
        }
    }
}

/// The ChrIns addresses of one ChrSet (`active_only`: entries with chr_load_status Active).
fn set_members(set: usize, active_only: bool, out: &mut Vec<usize>) {
    if !plausible(set) {
        return;
    }
    // SAFETY: non-faulting reads of the set header and its entries; every pointer is checked.
    unsafe {
        let cap = read::<u32>(set + offset_of!(ChrSet<ChrIns>, capacity)).unwrap_or(0) as usize;
        let entries = read_ptr(set + offset_of!(ChrSet<ChrIns>, entries));
        if !plausible(entries) || cap == 0 {
            return;
        }
        let stride = size_of::<ChrSetEntry<ChrIns>>();
        for i in 0..cap.min(SET_MAX) {
            let e = entries + i * stride;
            // the load status first: an inactive entry costs one read, not two
            if active_only && read::<u8>(e + offset_of!(ChrSetEntry<ChrIns>, chr_load_status)) != Some(2) {
                continue;
            }
            let chr = read_ptr(e + offset_of!(ChrSetEntry<ChrIns>, chr_ins));
            if chr == 0 || !plausible(chr) {
                continue;
            }
            out.push(chr);
        }
    }
}

/// Everything the census found: characters (union by address of `chr_inses_by_distance` and every Active entry of
/// the ChrSets + the open-field set), the members of the player / ghost / summon-buddy / debug sets, and the counts.
pub struct Census {
    pub chars: Vec<NukeChr>,
    pub player_sets: std::collections::HashSet<usize>,
    pub by_distance: u32,
    pub chrsets: u32,
    pub main_player: usize,
}

pub fn census(max_by_distance: usize) -> Option<Census> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return None;
    }
    let mut addrs: Vec<usize> = Vec::new();
    // 1. by distance (the list read_nearby uses, up to max)
    let vec = wcm + super::fp_read::chr_inses_by_distance_offset();
    // SAFETY: wcm is the live WorldChrMan; the vector header is 4 pointer-sized words.
    let (first, last) = unsafe { (read_ptr(vec + VEC_FIRST), read_ptr(vec + VEC_LAST)) };
    let mut by_distance = 0u32;
    if first != 0 && last >= first && plausible(first) {
        let stride = size_of::<ChrInsDistanceEntry>();
        let count = ((last - first) / stride).min(max_by_distance).min(SET_MAX);
        for i in 0..count {
            // SAFETY: i < count, inside [first, last).
            let chr = unsafe { read_ptr(first + i * stride + offset_of!(ChrInsDistanceEntry, chr_ins)) };
            if plausible(chr) {
                addrs.push(chr);
                by_distance += 1;
            }
        }
    }
    // 2. every Active entry of the map ChrSets and the open-field set
    let mut sets = Vec::new();
    // C3: the update type (owner) of every set entry, any load status
    let mut upd: std::collections::HashMap<usize, u8> = std::collections::HashMap::new();
    // SAFETY: plain integer reads of the live WorldChrMan.
    let holders = unsafe { read::<u32>(wcm + offset_of!(WorldChrMan, chr_set_holder_count)) }.unwrap_or(0).min(196) as usize;
    for i in 0..holders {
        // SAFETY: i < 196 (array bound); the entry is an Option<OwnedPtr> read as an integer.
        let s = unsafe { read_ptr(wcm + offset_of!(WorldChrMan, chr_sets) + i * size_of::<usize>()) };
        set_members(s, true, &mut sets);
        set_update_types(s, &mut upd);
    }
    set_members(wcm + offset_of!(WorldChrMan, open_field_chr_set), true, &mut sets);
    set_update_types(wcm + offset_of!(WorldChrMan, open_field_chr_set), &mut upd);
    let chrsets = sets.len() as u32;
    addrs.extend(sets);
    addrs.sort_unstable();
    addrs.dedup();
    // 3. the sets that are part of the player (never killed)
    let mut ps = Vec::new();
    for off in [
        offset_of!(WorldChrMan, player_chr_set),
        offset_of!(WorldChrMan, ghost_chr_set),
        offset_of!(WorldChrMan, summon_buddy_chr_set),
        offset_of!(WorldChrMan, debug_chr_set),
    ] {
        set_members(wcm + off, false, &mut ps);
    }
    // SAFETY: main_player read as an integer.
    let main_player = unsafe { read_ptr(wcm + offset_of!(WorldChrMan, main_player)) };
    let chars = addrs
        .into_iter()
        // SAFETY: addresses from the game's own lists this tick (or rejected by plausibility inside).
        .filter_map(|a| unsafe { read_nuke_chr(a) })
        .map(|mut n| {
            n.upd = upd.get(&n.addr).copied();
            n
        })
        .collect();
    Some(Census { chars, player_sets: ps.into_iter().collect(), by_distance, chrsets, main_player })
}

/// One character re-read by address (the kill pass checks its target is still the same live character).
pub fn reread(addr: usize) -> Option<NukeChr> {
    // SAFETY: `addr` came from this tick's census; plausibility and non-faulting reads inside.
    unsafe { read_nuke_chr(addr) }
}

/// Up to `max` characters from `WorldChrMan.chr_inses_by_distance` (nearest first). The entries'
/// `distance` is not metres (CS2 note); callers compute distances from positions.
pub fn read_nearby(max: usize) -> Vec<ChrRead> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return Vec::new();
    }
    let vec = wcm + super::fp_read::chr_inses_by_distance_offset();
    // SAFETY: wcm is the live WorldChrMan; the vector header is 4 pointer-sized words.
    let (first, last) = unsafe { (read_ptr(vec + VEC_FIRST), read_ptr(vec + VEC_LAST)) };
    if first == 0 || last < first || !plausible(first) {
        return Vec::new();
    }
    let stride = size_of::<ChrInsDistanceEntry>();
    let count = ((last - first) / stride).min(max).min(4096);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        // SAFETY: i < count, entries lie in [first, last); the chr_ins pointer is read as an integer.
        let chr = unsafe { read_ptr(first + i * stride + offset_of!(ChrInsDistanceEntry, chr_ins)) };
        // SAFETY: chr comes from the game's own list this tick (or fails plausibility).
        if let Some(c) = unsafe { read_chr(chr) } {
            out.push(c);
        }
    }
    out
}

// ---- co-op C2: the player set and the session role (docs/COOP_PLAN.md; read-only) -----------------------------------

/// The slots of `WorldChrMan.player_chr_set` as plain data, plus the local player's address / handle and the
/// `sinner_hunter` address. `None` without a WorldChrMan or a local player (not in the world).
pub fn player_set_slots() -> Option<(Vec<mw2er_core::coop::PeerSlot>, usize, u64, usize)> {
    use mw2er_core::coop::{MAX_SLOTS, PeerSlot};
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return None;
    }
    // SAFETY: integer reads of the live WorldChrMan (pointers read as integers, checked before use).
    let (main, sinner) = unsafe { (read_ptr(wcm + offset_of!(WorldChrMan, main_player)), read_ptr(wcm + offset_of!(WorldChrMan, sinner_hunter))) };
    if !plausible(main) {
        return None;
    }
    // SAFETY: main is the live local PlayerIns (plausibility-checked); the handle is plain data.
    let main_handle = unsafe { read::<u64>(main + offset_of!(ChrIns, field_ins_handle)) }.unwrap_or(0);
    let set = wcm + offset_of!(WorldChrMan, player_chr_set);
    let mut out = Vec::new();
    // SAFETY: non-faulting reads of the set header and its entries (same layout as ChrSet<ChrIns>, asserted above).
    unsafe {
        let cap = read::<u32>(set + offset_of!(ChrSet<ChrIns>, capacity)).unwrap_or(0) as usize;
        let entries = read_ptr(set + offset_of!(ChrSet<ChrIns>, entries));
        if plausible(entries) {
            let stride = size_of::<ChrSetEntry<ChrIns>>();
            for i in 0..cap.min(MAX_SLOTS) {
                let e = entries + i * stride;
                let chr = read_ptr(e + offset_of!(ChrSetEntry<ChrIns>, chr_ins));
                if chr == 0 || !plausible(chr) {
                    out.push(PeerSlot::default());
                    continue;
                }
                let load_status = read::<u8>(e + offset_of!(ChrSetEntry<ChrIns>, chr_load_status)).unwrap_or(0);
                let update_type = read::<u8>(e + offset_of!(ChrSetEntry<ChrIns>, chr_update_type)).unwrap_or(0);
                let handle = read::<u64>(chr + offset_of!(ChrIns, field_ins_handle)).unwrap_or(0);
                let chr_type = read::<i32>(chr + offset_of!(ChrIns, chr_type)).unwrap_or(-1);
                out.push(PeerSlot { addr: chr, handle, chr_type, load_status, update_type });
            }
        }
    }
    Some((out, main, main_handle, if plausible(sinner) { sinner } else { 0 }))
}

/// `CSSessionManager.lobby_state` (3 Host, 6 Client in the vanilla enum) and the length of its `players` vector.
/// Read as plain integers; `(None, None)` when the singleton is not found. Whether Seamless drives these is [I].
pub fn session_state() -> (Option<u32>, Option<u32>) {
    use eldenring::cs::{CSSessionManager, SessionManagerPlayerEntry};
    let sm = singleton_addr::<CSSessionManager>();
    if !plausible(sm) {
        return (None, None);
    }
    // SAFETY: sm is the live session manager; the enum is read as a u32 (never as a Rust enum), the vector header as
    // integers; nothing is dereferenced beyond them.
    unsafe {
        let lobby = read::<u32>(sm + offset_of!(CSSessionManager, lobby_state));
        let v = sm + offset_of!(CSSessionManager, players);
        let (first, last) = (read_ptr(v + VEC_FIRST), read_ptr(v + VEC_LAST));
        let stride = size_of::<SessionManagerPlayerEntry>();
        let n = if first != 0 && last >= first && stride > 0 && (last - first) % stride == 0 && (last - first) / stride <= 64 {
            Some(((last - first) / stride) as u32)
        } else if first == 0 && last == 0 {
            Some(0)
        } else {
            None
        };
        (lobby, n)
    }
}

/// C3 PASS C (world guns, docs/C3_DESIGN.md 3.6): a character's feet position, its physics orientation (XYZW) and havok
/// frame. Read-only; `None` when anything is unreadable.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_pose(chr: usize) -> Option<(V3, [f32; 4], f32)> {
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; pointers checked before they are followed; non-faulting reads.
    unsafe {
        let modules = read_ptr(chr + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return None;
        }
        let physics = read_ptr(modules + offset_of!(ChrInsModuleContainer, physics));
        if !plausible(physics) {
            return None;
        }
        let p = read::<[f32; 4]>(physics + offset_of!(CSChrPhysicsModule, position))?;
        let q = read::<[f32; 4]>(physics + offset_of!(CSChrPhysicsModule, orientation))?;
        if !q.iter().all(|x| x.is_finite()) {
            return None;
        }
        Some((v3(p)?, q, p[3]))
    }
}
/// Review C3 (docs/C3_DESIGN.md 3.6 stage B): `CSFD4ModelItem.mtx43_array_entity` (+0x640, private in the vendored
/// crate: derived from the public `location_aabb_exporter` at +0x658). C6 [V, work/c6/bones.md 1-2]: a pointer to a
/// `CSFD4LocationMtx43ArrayEntity`, NOT to the matrices (the C3 probe's garbage): count u32 at +0x68, entries at
/// `[+0x70]`, 0x30 bytes each, indexed by the model's FLVER bone.
pub const MODEL_ITEM_PALETTE: usize = offset_of!(eldenring::cs::CSFD4ModelItem, location_aabb_exporter) - 0x18;
const _: () = assert!(MODEL_ITEM_PALETTE == 0x640);
/// C6: `CSFD4LocationMtx43ArrayEntity` count (u32) and entry pointer [V ctor 0x140b43ad0, getters 0x140b43bf0 / c80].
pub const MTX43_COUNT: usize = 0x68;
pub const MTX43_DATA: usize = 0x70;
/// One entry: 3 x float4 `[R row | T]` [V].
pub const PALETTE_STRIDE: usize = 48;
/// Never read more bones than this for the probe.
pub const PALETTE_MAX: usize = 1024;
/// C6: `ChrCtrl.location_mtx44_chr_entity` (+0x2F8, private in the vendored crate: the public `offset_y` at +0x2E0, then
/// `unk2e4: [u8; 0x14]`) [V 0x1403c5046]; its 4x4 (row-major, translation in row 3) at +0x70 [V getter 0x140b43f60].
pub const CHR_CTRL_MTX44: usize = offset_of!(eldenring::cs::ChrCtrl, offset_y) + 4 + 0x14;
const _: () = assert!(CHR_CTRL_MTX44 == 0x2F8);
pub const MTX44_MATRIX: usize = 0x70;
/// C6 (docs 4.2, I9): `ChrIns.hka_pose_importer` (+0x398, private: the public `net_position_synchronized` at +0x364,
/// then `unk368: [u8; 0x20]`, `last_received_packet60: u32`, `unk38c: [u8; 0xc]`) [V fn 0x1403e8240]. Its hkaPose at
/// +0x48: skeleton* (+0x48), model-pose hkArray<hkQsTransform> data (+0x60) / size (+0x68).
pub const CHR_HKA_IMPORTER: usize = offset_of!(ChrIns, net_position_synchronized) + 4 + 0x20 + 4 + 0xc;
const _: () = assert!(CHR_HKA_IMPORTER == 0x398);
pub const IMPORTER_SKELETON: usize = 0x48;
pub const IMPORTER_MODEL_POSE: usize = 0x60;
pub const IMPORTER_MODEL_POSE_N: usize = 0x68;
/// hkaSkeleton bones `{char* name; bool lock}` x 0x10: the hkArray `{data +0x30, size +0x38}` [attach2.md 4: the importer
/// ctor 0x140b4b8e1 reads the bone COUNT at +0x38 [V]; the first build read the data pointer at +0x28, which is the
/// parent-index array's size|capacity word (0x80000000_000000xx): `HOLD bone name=none reason=bones` on all 20 tries].
pub const SKEL_BONES: usize = 0x30;
pub const SKEL_COUNT: [usize; 1] = [0x38];
pub const SKEL_BONE_STRIDE: usize = 0x10;
pub const HK_QS_STRIDE: usize = 0x30;

/// C6: the bone array of a character (read-only; the vtable is compared by the caller).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoneArray {
    pub arr: usize,
    pub vt: usize,
    pub n: u32,
    pub data: usize,
}

/// C6: `[[chr+0x50]+0x10]+0x640` and its header. `None` when a pointer is implausible.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_bone_array(chr: usize) -> Option<BoneArray> {
    use eldenring::cs::CSModelIns;
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; every pointer is read as an integer and checked before it is followed.
    unsafe {
        let model_ins = read_ptr(chr + offset_of!(ChrIns, chr_model_ins));
        if !plausible(model_ins) {
            return None;
        }
        // CSChrModelIns starts with its CSModelIns
        let item = read_ptr(model_ins + offset_of!(CSModelIns, model_item));
        if !plausible(item) {
            return None;
        }
        let arr = read_ptr(item + MODEL_ITEM_PALETTE);
        if !plausible(arr) {
            return None;
        }
        let vt = read_ptr(arr);
        let n = read::<u32>(arr + MTX43_COUNT)?;
        let data = read_ptr(arr + MTX43_DATA);
        if !plausible(data) || data % 16 != 0 {
            return None;
        }
        Some(BoneArray { arr, vt, n, data })
    }
}

/// Review C3 stage B / C6: the bone array of a character as raw 12-float entries (read-only, non-faulting; at most
/// `min(n, max)` entries, stops at the first unreadable one). `None` when the chain is unreadable.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_palette(chr: usize, max: usize) -> Option<(BoneArray, Vec<[f32; 12]>)> {
    // SAFETY: forwarded; non-faulting reads.
    unsafe {
        let a = read_bone_array(chr)?;
        let mut out = Vec::new();
        for k in 0..(a.n as usize).min(max).min(PALETTE_MAX) {
            let Some(m) = read::<[f32; 12]>(a.data + k * PALETTE_STRIDE) else { break };
            out.push(m);
        }
        Some((a, out))
    }
}

/// C6: the chr matrix the body is drawn with (`[[chr+0x58]+0x2F8]+0x70`): (entity, vtable, row-major 4x4).
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_chr_matrix(chr: usize) -> Option<(usize, usize, [f32; 16])> {
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; pointers checked before they are followed; non-faulting reads.
    unsafe {
        let ctrl = read_ptr(chr + offset_of!(ChrIns, chr_ctrl));
        if !plausible(ctrl) {
            return None;
        }
        let ent = read_ptr(ctrl + CHR_CTRL_MTX44);
        if !plausible(ent) {
            return None;
        }
        let vt = read_ptr(ent);
        let m = read::<[f32; 16]>(ent + MTX44_MATRIX)?;
        Some((ent, vt, m))
    }
}

/// C6: one hand read (docs 4.1): the bone entry, the chr matrix, the count and both vtables (112 bytes + 7 pointers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandRead {
    pub e: [f32; 12],
    pub m: [f32; 16],
    pub n: u32,
    pub vt43: usize,
    pub vt44: usize,
}

/// C6: the entry `bone` of a character's bone array and its chr matrix. `None` = a pointer / read failed (`reason=read`);
/// a `bone >= n` reads no entry (zeros; the caller rejects it as `range`).
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_hand(chr: usize, bone: u32) -> Option<HandRead> {
    // SAFETY: forwarded; non-faulting reads.
    unsafe {
        let a = read_bone_array(chr)?;
        let (_, vt44, m) = read_chr_matrix(chr)?;
        let e = if bone < a.n && a.n <= mw2er_core::hold::N_MAX { read::<[f32; 12]>(a.data + bone as usize * PALETTE_STRIDE)? } else { [0.0; 12] };
        Some(HandRead { e, m, n: a.n, vt43: a.vt, vt44 })
    }
}

/// C6 (docs 4.2, I9): the havok skeleton index of the first of `names` found (in `names` order): (index, name, count
/// offset used, count). Err = why not (`importer` / `skeleton` / `bones` / `count` / `none`). Read-only; names are
/// ASCII, at most 64 bytes.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn find_skeleton_bone(chr: usize, names: &[&'static str]) -> Result<(usize, &'static str, usize, u32), &'static str> {
    if !plausible(chr) {
        return Err("chr");
    }
    // SAFETY: the caller's contract; every pointer is checked; non-faulting reads.
    unsafe {
        let imp = read_ptr(chr + CHR_HKA_IMPORTER);
        if !plausible(imp) {
            return Err("importer");
        }
        let skel = read_ptr(imp + IMPORTER_SKELETON);
        if !plausible(skel) {
            return Err("skeleton");
        }
        let bones = read_ptr(skel + SKEL_BONES);
        if !plausible(bones) {
            return Err("bones");
        }
        let Some((cnt_off, n)) = SKEL_COUNT.iter().find_map(|&o| read::<u32>(skel + o).filter(|n| (1..=1024).contains(n)).map(|n| (o, n))) else {
            return Err("count");
        };
        let mut found: Vec<Option<usize>> = vec![None; names.len()];
        for k in 0..n as usize {
            // an hkStringPtr keeps its "owned" flag in bit 0 of the pointer
            let p = read_ptr(bones + k * SKEL_BONE_STRIDE) & !1;
            if !plausible_str(p) {
                continue;
            }
            let Some(name) = read_cstr(p) else { continue };
            if let Some(i) = names.iter().position(|x| *x == name)
                && found[i].is_none()
            {
                found[i] = Some(k);
            }
        }
        for (i, f) in found.iter().enumerate() {
            if let Some(k) = f {
                return Ok((*k, names[i], cnt_off, n));
            }
        }
        Err("none")
    }
}

/// C6 depth fallback: the havok skeleton index of every name in `names` (`None` = not in the skeleton), plus every bone
/// name in skeleton order (for the one-off `HOLD body names` dump). Err as [`find_skeleton_bone`]. Read-only.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn find_skeleton_bones(chr: usize, names: &[&'static str]) -> Result<(Vec<Option<usize>>, Vec<String>), &'static str> {
    if !plausible(chr) {
        return Err("chr");
    }
    // SAFETY: the caller's contract; every pointer is checked; non-faulting reads.
    unsafe {
        let imp = read_ptr(chr + CHR_HKA_IMPORTER);
        if !plausible(imp) {
            return Err("importer");
        }
        let skel = read_ptr(imp + IMPORTER_SKELETON);
        if !plausible(skel) {
            return Err("skeleton");
        }
        let bones = read_ptr(skel + SKEL_BONES);
        if !plausible(bones) {
            return Err("bones");
        }
        let Some(n) = SKEL_COUNT.iter().find_map(|&o| read::<u32>(skel + o).filter(|n| (1..=1024).contains(n))) else {
            return Err("count");
        };
        let mut found: Vec<Option<usize>> = vec![None; names.len()];
        let mut all = Vec::with_capacity(n as usize);
        for k in 0..n as usize {
            let p = read_ptr(bones + k * SKEL_BONE_STRIDE) & !1;
            let name = if plausible_str(p) { read_cstr(p) } else { None };
            if let Some(nm) = &name
                && let Some(i) = names.iter().position(|x| x == nm)
                && found[i].is_none()
            {
                found[i] = Some(k);
            }
            all.push(name.unwrap_or_else(|| "?".to_string()));
        }
        Ok((found, all))
    }
}

/// C6 depth fallback: the world positions of FLVER bones `idx` of a character (`None` per bone not resolved / out of
/// range / unreadable / implausible), through [`mw2er_core::hold::hand_world`] (world-space array first, else model space
/// x the chr matrix). `None` = the array or chr matrix unreadable or its vtable not `vt43`.
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_body_points(chr: usize, idx: &[Option<u32>], vt43: usize, phys: V3) -> Option<Vec<Option<V3>>> {
    // SAFETY: forwarded; non-faulting reads.
    unsafe {
        let a = read_bone_array(chr)?;
        if a.vt != vt43 || a.n > mw2er_core::hold::N_MAX {
            return None;
        }
        let (_, _, m) = read_chr_matrix(chr)?;
        Some(
            idx.iter()
                .map(|i| {
                    let i = (*i)?;
                    if i >= a.n {
                        return None;
                    }
                    let e = read::<[f32; 12]>(a.data + i as usize * PALETTE_STRIDE)?;
                    mw2er_core::hold::hand_world(&e, &m, phys).ok().map(|(h, _)| mw2er_core::hold::translation(&h))
                })
                .collect(),
        )
    }
}

/// A string pointer: any user-mode address (names may be byte-aligned).
fn plausible_str(p: usize) -> bool {
    (0x10000..0x0000_7FFF_FFFF_0000).contains(&p)
}

/// Up to 64 ASCII bytes until NUL, read in 16-byte chunks (non-faulting).
///
/// # Safety
/// Any address (non-faulting reads).
unsafe fn read_cstr(p: usize) -> Option<String> {
    let mut out = Vec::new();
    for c in 0..4 {
        // SAFETY: non-faulting read of 16 plain bytes.
        let b = unsafe { read::<[u8; 16]>(p + c * 16) }?;
        for &x in &b {
            if x == 0 {
                return (!out.is_empty()).then(|| String::from_utf8_lossy(&out).into_owned());
            }
            if !(0x20..0x7f).contains(&x) {
                return None;
            }
            out.push(x);
        }
    }
    None
}

/// C6: the model-pose translation of havok bone `k` (`[importer+0x60] + 0x30*k`, hkQsTransform T at +0); valid only
/// while the pose is in sync [I].
///
/// # Safety
/// As [`read_chr`].
pub(crate) unsafe fn read_model_pose_t(chr: usize, k: usize) -> Option<V3> {
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; pointers checked; non-faulting reads.
    unsafe {
        let imp = read_ptr(chr + CHR_HKA_IMPORTER);
        if !plausible(imp) {
            return None;
        }
        let data = read_ptr(imp + IMPORTER_MODEL_POSE);
        let n = read::<u32>(imp + IMPORTER_MODEL_POSE_N)? as usize;
        if !plausible(data) || k >= n || n > 1024 {
            return None;
        }
        v3(read::<[f32; 4]>(data + k * HK_QS_STRIDE)?)
    }
}

// ---- C6 attach 2 (work/c6/attach2.md 1-2): the model ER draws the right-hand weapon with -----------------------------

/// `PlayerIns+0x648` = the private `chr_asm_model_ins` (CSChrAsmModelIns*), right after `chr_asm` and `chr_asm_model_res`
/// [V 0x1406522a8 `mov [rdi+648h], rax` after its ctor 0x1409ea640].
pub const PLAYER_ASM_MODEL: usize = offset_of!(eldenring::cs::PlayerIns, chr_asm) + 0x10;
const _: () = assert!(PLAYER_ASM_MODEL == 0x648);
/// CSChrAsmModelIns: `+0x08` the owning CSChrModelIns (== `[PlayerIns+0x50]`), `+0x28 + 8*slot` CSPartsModelIns* for 27
/// slots [V ctor 0x1409ea640]; slots 11..14 = models 0..3 of the active RIGHT weapon (kind 7) [V 0x1409ed8fc].
pub const ASM_MODEL_OWNER: usize = 0x08;
pub const ASM_MODEL_PARTS: usize = 0x28;
pub const ASM_MODEL_SLOTS: usize = 27;
pub const ASM_SLOT_RIGHT0: usize = 11;
/// CSPartsModelIns (a CSModelIns): `+0x150` the parent CSChrModelIns [V 0x1409f5151].
pub const PARTS_PARENT: usize = 0x150;

/// C6 attach 2: one read of the active right weapon's model (all non-faulting; the caller compares the vtables).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeaponModelRead {
    pub parts: usize,
    pub asm_vt: usize,
    pub parts_vt: usize,
    pub ent_vt: usize,
    pub arr_vt: usize,
    /// `[asm+8]` and `[parts+0x150]` are the character's own CSChrModelIns.
    pub owner_ok: bool,
    /// The location matrix `[parts+0x20]+0x70` (row-major 4x4) and the weapon FLVER bone 0 entry with the array count.
    pub root: Option<[f32; 16]>,
    pub e0: Option<[f32; 12]>,
    pub n: u32,
}

/// The model of the active right weapon (`slot` = 11 for model 0). `Err("none")` = no model in the slot (fists, or the
/// game dropped it), other `Err` = an unreadable pointer.
///
/// # Safety
/// `player` must be 0 or a live `PlayerIns` (ours or a peer's from the player set this tick).
pub(crate) unsafe fn read_weapon_model(player: usize, slot: usize) -> Result<WeaponModelRead, &'static str> {
    use eldenring::cs::CSModelIns;
    if !plausible(player) || slot >= ASM_MODEL_SLOTS {
        return Err("player");
    }
    // SAFETY: the caller's contract; every pointer is read as an integer and checked before it is followed.
    unsafe {
        let chr_model = read_ptr(player + offset_of!(ChrIns, chr_model_ins));
        let asm = read_ptr(player + PLAYER_ASM_MODEL);
        if !plausible(asm) || !plausible(chr_model) {
            return Err("asm");
        }
        let parts = read_ptr(asm + ASM_MODEL_PARTS + 8 * slot);
        if parts == 0 {
            return Err("none");
        }
        if !plausible(parts) {
            return Err("parts");
        }
        let mut r = WeaponModelRead { parts, asm_vt: read_ptr(asm), parts_vt: read_ptr(parts), ..Default::default() };
        r.owner_ok = read_ptr(asm + ASM_MODEL_OWNER) == chr_model && read_ptr(parts + PARTS_PARENT) == chr_model;
        let ent = read_ptr(parts + offset_of!(CSModelIns, location_entity));
        if plausible(ent) {
            r.ent_vt = read_ptr(ent);
            r.root = read::<[f32; 16]>(ent + MTX44_MATRIX);
        }
        let item = read_ptr(parts + offset_of!(CSModelIns, model_item));
        if plausible(item) {
            let arr = read_ptr(item + MODEL_ITEM_PALETTE);
            if plausible(arr) {
                r.arr_vt = read_ptr(arr);
                r.n = read::<u32>(arr + MTX43_COUNT).unwrap_or(0);
                let data = read_ptr(arr + MTX43_DATA);
                if plausible(data) && data % 16 == 0 && (1..=mw2er_core::hold::WEAPON_N_MAX).contains(&r.n) {
                    r.e0 = read::<[f32; 12]>(data);
                }
            }
        }
        Ok(r)
    }
}

/// C6 (docs 3.3): a player's grip and active weapons from `PlayerIns.chr_asm` (readable for peers too).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArmRead {
    /// The ChrAsm address (`arm_style` is written there by the `stance_refresh = "arm_style"` probe only).
    pub asm: usize,
    pub arm_style: u32,
    pub right_slot: u32,
    /// `equipment_param_ids` of the active right / left weapon slots (-1 = empty).
    pub right_id: i32,
    pub left_id: i32,
    /// All six weapon `equipment_param_ids` (left 0/2/4, right 1/3/5; -1 = empty): the stance never lets a peer's row
    /// or its own touch a row another of our weapons uses.
    pub ids: [i32; 6],
}

/// `ChrAsm.equipment.arm_style` offset (a u32 enum: 0 empty, 1 one-handed, 2 left both, 3 right both) [V vendored layout].
pub const ASM_ARM_STYLE: usize = offset_of!(eldenring::cs::ChrAsm, equipment) + offset_of!(eldenring::cs::ChrAsmEquipment, arm_style);
const _: () = assert!(size_of::<eldenring::cs::ChrAsmArmStyle>() == 4);

/// # Safety
/// `player` must be 0 or a live `PlayerIns` (ours or a peer's from the player set this tick).
pub(crate) unsafe fn read_arm(player: usize) -> Option<ArmRead> {
    use eldenring::cs::{ChrAsm, ChrAsmEquipment, ChrAsmEquipmentSlots, PlayerIns};
    if !plausible(player) {
        return None;
    }
    // SAFETY: the caller's contract; the ChrAsm pointer is checked; plain integers read by offset (the enum as a u32).
    unsafe {
        let asm = read_ptr(player + offset_of!(PlayerIns, chr_asm));
        if !plausible(asm) {
            return None;
        }
        let eq = asm + offset_of!(ChrAsm, equipment);
        let arm_style = read::<u32>(asm + ASM_ARM_STYLE)?;
        let slots = eq + offset_of!(ChrAsmEquipment, selected_slots);
        let right_slot = read::<u32>(slots + offset_of!(ChrAsmEquipmentSlots, right_weapon_slot))?;
        let left_slot = read::<u32>(slots + offset_of!(ChrAsmEquipmentSlots, left_weapon_slot))?;
        if arm_style > 3 || right_slot > 2 || left_slot > 2 {
            return None;
        }
        let ids = asm + offset_of!(ChrAsm, equipment_param_ids);
        let mut all = [-1i32; 6];
        for (k, v) in all.iter_mut().enumerate() {
            *v = read::<i32>(ids + 4 * k)?;
        }
        let right_id = all[1 + 2 * right_slot as usize];
        let left_id = all[2 * left_slot as usize];
        Some(ArmRead { asm, arm_style, right_slot, right_id, left_id, ids: all })
    }
}

/// C6 aim 4 probe (work/c6/aim4.md 1): the animation ER plays now: `CSChrTimeActModule.anim_queue[read_idx].anim_id`
/// (the circular buffer of the TAE module; read-only).
///
/// # Safety
/// `chr` must be 0 or a live `ChrIns` (our PlayerIns this tick).
pub(crate) unsafe fn read_anim_id(chr: usize) -> Option<i32> {
    use eldenring::cs::{CSChrTimeActModule, CSChrTimeActModuleAnim};
    const _: () = assert!(size_of::<CSChrTimeActModuleAnim>() == 16);
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; the module pointers are checked; plain integers read by offset.
    unsafe {
        let modules = read_ptr(chr + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return None;
        }
        let ta = read_ptr(modules + offset_of!(ChrInsModuleContainer, time_act));
        if !plausible(ta) {
            return None;
        }
        let idx = read::<u32>(ta + offset_of!(CSChrTimeActModule, read_idx))? as usize % 10;
        read::<i32>(ta + offset_of!(CSChrTimeActModule, anim_queue) + idx * size_of::<CSChrTimeActModuleAnim>() + offset_of!(CSChrTimeActModuleAnim, anim_id))
    }
}

/// C6 round 7 probe P-A7 (work/c6/aim7.md 4 C): all 10 `anim_queue` ids of the TAE module, starting at `read_idx`
/// (read-only).
///
/// # Safety
/// `chr` must be 0 or a live `ChrIns` (our PlayerIns this tick).
pub(crate) unsafe fn read_anim_queue(chr: usize) -> Option<[i32; 10]> {
    use eldenring::cs::{CSChrTimeActModule, CSChrTimeActModuleAnim};
    if !plausible(chr) {
        return None;
    }
    // SAFETY: the caller's contract; the module pointers are checked; plain integers read by offset.
    unsafe {
        let modules = read_ptr(chr + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return None;
        }
        let ta = read_ptr(modules + offset_of!(ChrInsModuleContainer, time_act));
        if !plausible(ta) {
            return None;
        }
        let idx = read::<u32>(ta + offset_of!(CSChrTimeActModule, read_idx))? as usize;
        let q = ta + offset_of!(CSChrTimeActModule, anim_queue);
        let mut out = [-1i32; 10];
        for (k, v) in out.iter_mut().enumerate() {
            *v = read::<i32>(q + ((idx + k) % 10) * size_of::<CSChrTimeActModuleAnim>() + offset_of!(CSChrTimeActModuleAnim, anim_id))?;
        }
        Some(out)
    }
}

/// C6 round 7 (work/c6/aim7.md 1): `PlayerIns.snipe_mode_draw_alpha_fade_timer` (+0x5D8; 1 = drawn, 0 = ER faded our
/// body out for its precision aim). Read-only: ER rewrites it every frame.
///
/// # Safety
/// `player` must be 0 or a live `PlayerIns` (ours this tick).
pub(crate) unsafe fn read_snipe(player: usize) -> Option<f32> {
    const OFF: usize = offset_of!(eldenring::cs::PlayerIns, snipe_mode_draw_alpha_fade_timer);
    const _: () = assert!(OFF == mw2er_core::aim_cam::SNIPE_FADE_OFF);
    if !plausible(player) {
        return None;
    }
    // SAFETY: the caller's contract; a plain f32 read by offset.
    unsafe { read::<f32>(player + OFF) }.filter(|v| v.is_finite())
}

/// C6 round 7 probe P-A7: the bolt slots' `equipment_param_ids` (Bolt1..3 = ChrAsm slots 7 / 9 / 11; -1 = empty).
///
/// # Safety
/// `player` must be 0 or a live `PlayerIns` (ours this tick).
pub(crate) unsafe fn read_bolts(player: usize) -> Option<[i32; 3]> {
    use eldenring::cs::{ChrAsm, PlayerIns};
    if !plausible(player) {
        return None;
    }
    // SAFETY: the caller's contract; the ChrAsm pointer is checked; plain integers read by offset.
    unsafe {
        let asm = read_ptr(player + offset_of!(PlayerIns, chr_asm));
        if !plausible(asm) {
            return None;
        }
        let ids = asm + offset_of!(ChrAsm, equipment_param_ids);
        Some([read::<i32>(ids + 4 * 7)?, read::<i32>(ids + 4 * 9)?, read::<i32>(ids + 4 * 11)?])
    }
}
