//! Map ray cast for the falloff distance estimate (`CSHavokMan.phys_world.cast_ray`).
//! The filter 0x2000058 hits map collision (CS2 note); the API compiled in the design checks.

use std::mem::offset_of;

use eldenring::cs::{CSHavokMan, CSPhysWorld, PlayerIns};
use eldenring::position::{HavokPosition, PositionDelta};
use mw2er_core::vec3::V3;

use super::{plausible, read_ptr, singleton_addr};

/// Collision filter for map geometry (reported in the CS2 conversion note).
pub const MAP_FILTER: u32 = 0x0200_0058;

/// Distance (metres) along `dir` from `origin` to the first map hit within `range`, if any.
pub fn map_ray(origin: V3, w: f32, dir: V3, range: f32, player_ins: usize) -> Option<f32> {
    ray_t(origin, w, dir, range, player_ins, MAP_FILTER)
}

/// As [`map_ray`] under another collision `filter` (C6 part 18: the shoulder camera's spring arm also casts
/// `OBJECT_FILTER`, the filter ER's own follow camera uses).
pub fn ray_t(origin: V3, w: f32, dir: V3, range: f32, player_ins: usize, filter: u32) -> Option<f32> {
    let hm = singleton_addr::<CSHavokMan>();
    if hm == 0 || !plausible(player_ins) {
        return None;
    }
    // SAFETY: live CSHavokMan; phys_world is an OwnedPtr read as an integer.
    let world = unsafe { read_ptr(hm + offset_of!(CSHavokMan, phys_world)) };
    if !plausible(world) {
        return None;
    }
    let dir = dir.normalize()?;
    let o = HavokPosition(origin.x, origin.y, origin.z, w);
    let d = PositionDelta(dir.x * range, dir.y * range, dir.z * range);
    // SAFETY: both pointers were checked non-null this tick on the main thread; CSPhysWorld is an
    // opaque (zero-sized) type and the PlayerIns reference is only passed through to the game.
    let hit = unsafe {
        let world = &*(world as *const CSPhysWorld);
        let player = &*(player_ins as *const PlayerIns);
        world.cast_ray(filter, &o, d, player)
    }?;
    let h = V3::new(hit.0, hit.1, hit.2);
    let t = (h - origin).len();
    (t.is_finite() && t <= range * 1.01).then_some(t)
}

// ---- M5: map rays for the movement controller (docs/M5_DESIGN.md section 5.1) --------------------------------

use std::sync::atomic::{AtomicU64, Ordering};

/// Rays cast by the controller and their cost (for the PERF line; the maxima reset when read).
pub static RAY_CALLS: AtomicU64 = AtomicU64::new(0);
pub static RAY_NS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static RAY_NS_MAX: AtomicU64 = AtomicU64::new(0);

/// The game's physics world (0 = none), read once per tick by the controller.
pub fn phys_world() -> usize {
    let hm = singleton_addr::<CSHavokMan>();
    if hm == 0 {
        return 0;
    }
    // SAFETY: live CSHavokMan; phys_world is an OwnedPtr read as an integer.
    let w = unsafe { read_ptr(hm + offset_of!(CSHavokMan, phys_world)) };
    if plausible(w) { w } else { 0 }
}

/// The object / asset filter of run 18: the constant ER's follow camera passes to its sphere casts (`mov edx,5Bh` at
/// eldenring.exe 0x1403b746f / 0x1403b8da8 / 0x1403b97be / 0x1403bab31, the same CastParams slot +0x14 that
/// cast_ray fills from its filter argument). The camera collides with wagons; the map filter apparently does not [I].
pub const OBJECT_FILTER: u32 = 0x5b;

/// The first hit point (metres, Havok frame) of the segment `origin .. origin + delta` under the collision `filter`
/// (`MAP_FILTER`, or run 18's `[movement.rays]` filters), if any. Counts and times every call. Only the controller's
/// `RayBudget` calls it (wiring check 4).
pub fn map_ray_point(world: usize, player_ins: usize, origin: V3, w: f32, delta: V3, filter: u32) -> Option<V3> {
    if !plausible(world) || !plausible(player_ins) || !origin.is_finite() || !delta.is_finite() {
        return None;
    }
    let t = std::time::Instant::now();
    let o = HavokPosition(origin.x, origin.y, origin.z, w);
    let d = PositionDelta(delta.x, delta.y, delta.z);
    // SAFETY: both pointers were checked non-null this tick on the main thread (the task); CSPhysWorld is an opaque
    // type and the PlayerIns reference is only passed through to the game, as in map_ray above.
    let hit = unsafe {
        let world = &*(world as *const CSPhysWorld);
        let player = &*(player_ins as *const PlayerIns);
        world.cast_ray(filter, &o, d, player)
    };
    let ns = t.elapsed().as_nanos() as u64;
    RAY_CALLS.fetch_add(1, Ordering::Relaxed);
    RAY_NS_TOTAL.fetch_add(ns, Ordering::Relaxed);
    RAY_NS_MAX.fetch_max(ns, Ordering::Relaxed);
    let h = V3::new(hit?.0, hit?.1, hit?.2);
    h.is_finite().then_some(h)
}
