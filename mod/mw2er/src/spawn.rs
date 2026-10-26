//! Spawning engine bullets (`CSBulletManager::spawn_bullet`) from regulation rows the build added.
//! Main thread only. The spawn data comes from the vendored `BulletSpawnData::new` (patches 0001 + 0003):
//! owner = the player (team rules + `last_hit_by` attribution), target = empty handle (no lock-on
//! homing), behavior/magic/goods/dummy-poly ids = -1, emitter flags `unk44` = 1 (bit 0 auto-finish, M7), every
//! other unknown field zero.
//!
//! M7 (docs/M7_DESIGN.md 2.1): every call creates one bullet emitter in the manager's 64 + 192 emitter pool. Without
//! the auto-finish bit our emitters never finished and the 257th call of a world failed with code 4 (runs 6, 13, 14;
//! the AC-130 stopped dealing damage in run 14). `Ok` carries the emitter id ER returned.

use eldenring::cs::{BulletSpawnData, CSBulletManager, FieldInsHandle};
use fromsoftware_shared::{F32Vector4, FromStatic};
use mw2er_core::vec3::V3;

const _: () = assert!(size_of::<BulletSpawnData>() == 0x110);
/// The emitter-flags word sits at +0x44 (the request copy `movzx eax,byte ptr [rdi+44h]; and al,1` at 0x14038e66a).
const _: () = assert!(std::mem::offset_of!(BulletSpawnData, unk44) == 0x44);
/// Bit 0 = auto-finish. Bit 3 (network sync) is never set on a damaging row: it lands once, on the shooter's PC.
pub const SPAWN_FLAGS: u32 = 1;
/// C3 PASS C (docs/C3_DESIGN.md 3.4): auto-finish + bit 3 network sync (0x1403a2d28 `test byte [rsi+44h],8` [V]):
/// ER sends the spawn to the peers (packet 0x3F). ONLY for the no-attack visual twins and the nuke marker bullet
/// ([`spawn_synced`] refuses every other id).
pub const SPAWN_FLAGS_SYNCED: u32 = 0x9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// CSBulletManager not available.
    NoManager,
    /// Engine error code (3 = invalid bullet id, 4 = construction failed).
    Engine(i32),
    /// C3: a synced spawn of a row that is not a twin / marker (refused before the engine call).
    NotSyncable(i32),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::NoManager => write!(f, "code=none (CSBulletManager unavailable)"),
            SpawnError::Engine(c) => write!(f, "code={c} (3=invalid bullet id, 4=construction failed)"),
            SpawnError::NotSyncable(id) => write!(f, "code=none (row {id} may not be network-synced: only twins / markers)"),
        }
    }
}

/// The spawn data of one call (also used by the byte test).
pub fn spawn_data(owner: FieldInsHandle, bullet_id: i32, origin: V3, origin_w: f32, dir: V3) -> BulletSpawnData {
    let d = BulletSpawnData::new(owner, bullet_id, F32Vector4(origin.x, origin.y, origin.z, origin_w), F32Vector4(dir.x, dir.y, dir.z, 0.0));
    debug_assert_eq!(d.unk44, SPAWN_FLAGS);
    d
}

/// Fires one bullet of row `bullet_id` from `origin` along `dir` (unit vector). `Ok` = the emitter id.
pub fn spawn(owner: FieldInsHandle, bullet_id: i32, origin: V3, origin_w: f32, dir: V3) -> Result<i32, SpawnError> {
    // SAFETY: main thread (CSTask closure) after the boot gate; exclusive access as the crate requires.
    let bm = unsafe { CSBulletManager::instance_mut() }.map_err(|_| SpawnError::NoManager)?;
    let data = spawn_data(owner, bullet_id, origin, origin_w, dir);
    bm.spawn_bullet(&data).map_err(SpawnError::Engine)
}

/// C3 PASS C: the spawn data of a synced spawn (auto-finish + bit 3); `Err` for any row that is not a twin or the nuke
/// marker (`mw2er_core::coop_vis::syncable_bullet`): a damaging row can never be network-synced.
pub fn spawn_data_synced(owner: FieldInsHandle, bullet_id: i32, origin: V3, origin_w: f32, dir: V3) -> Result<BulletSpawnData, SpawnError> {
    if !mw2er_core::coop_vis::syncable_bullet(bullet_id) {
        return Err(SpawnError::NotSyncable(bullet_id));
    }
    let mut d = spawn_data(owner, bullet_id, origin, origin_w, dir);
    d.unk44 = SPAWN_FLAGS_SYNCED;
    Ok(d)
}

/// C3 PASS C: fires one visual twin / nuke marker with network sync (peers' games create it too). `Ok` = the emitter id.
pub fn spawn_synced(owner: FieldInsHandle, bullet_id: i32, origin: V3, origin_w: f32, dir: V3) -> Result<i32, SpawnError> {
    let data = spawn_data_synced(owner, bullet_id, origin, origin_w, dir)?;
    // SAFETY: main thread (CSTask closure) after the boot gate; exclusive access as the crate requires.
    let bm = unsafe { CSBulletManager::instance_mut() }.map_err(|_| SpawnError::NoManager)?;
    bm.spawn_bullet(&data).map_err(SpawnError::Engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/M7_DESIGN.md 2.1.4 oracle 1: the spawn data as bytes: +0x44 = 0x01 (auto-finish only), +0x45..0x48 = 0,
    /// bit 3 (network sync) clear, the ids -1, every other unknown field 0.
    #[test]
    fn spawn_data_bytes_autofinish_only() {
        let d = spawn_data(FieldInsHandle::none(), 79_000_201, V3::new(1.0, 2.0, 3.0), 1.0, V3::new(0.0, 0.0, 1.0));
        // SAFETY: BulletSpawnData is a 0x110-byte plain-data struct (asserted above).
        let b = unsafe { std::mem::transmute::<BulletSpawnData, [u8; 0x110]>(d) };
        assert_eq!(b[0x44], 0x01, "auto-finish bit");
        assert_eq!(&b[0x45..0x48], &[0, 0, 0]);
        assert_eq!(b[0x44] & 0x08, 0, "network sync bit never set");
        assert_eq!(&b[0..8], &[0xff; 8], "owner = empty handle here");
        let i32_at = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!((i32_at(0x08), i32_at(0x0c), i32_at(0x14), i32_at(0x18), i32_at(0x1c)), (-1, -1, 79_000_201, -1, -1));
        assert_eq!(&b[0x20..0x28], &[0xff; 8], "target = empty handle");
        // unknown fields: +0x10, +0x28..0x44 (incl. unk30 vector, unk40), +0x48..0x50, +0x60..0x70, +0x90..0x110
        for (lo, hi) in [(0x10, 0x14), (0x28, 0x44), (0x48, 0x50), (0x60, 0x70), (0x90, 0x110)] {
            assert!(b[lo..hi].iter().all(|&x| x == 0), "unknown bytes {lo:#x}..{hi:#x} must be zero");
        }
        let f32_at = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        assert_eq!((f32_at(0x80), f32_at(0x84), f32_at(0x88), f32_at(0x8c)), (1.0, 2.0, 3.0, 1.0), "position");
        assert_eq!((f32_at(0x70), f32_at(0x74), f32_at(0x78)), (0.0, 0.0, 1.0), "angle");
    }

    /// C3 PASS C (docs/C3_DESIGN.md 3.4): a twin carries bit 3 (+0x44 = 0x09); a damaging row is refused for a synced
    /// spawn and keeps 0x01 on the normal path.
    #[test]
    fn c3_twin_spawn_bytes_bit3_only_on_twins() {
        let d = spawn_data_synced(FieldInsHandle::none(), 79_812_000, V3::new(1.0, 2.0, 3.0), 1.0, V3::new(0.0, 0.0, 1.0)).unwrap();
        // SAFETY: BulletSpawnData is a 0x110-byte plain-data struct (asserted above).
        let b = unsafe { std::mem::transmute::<BulletSpawnData, [u8; 0x110]>(d) };
        assert_eq!(b[0x44], 0x09, "auto-finish + network sync");
        assert_eq!(&b[0x45..0x48], &[0, 0, 0]);
        assert!(spawn_data_synced(FieldInsHandle::none(), mw2er_core::coop_vis::NUKE_MARKER_BULLET, V3::ZERO, 1.0, V3::new(0.0, 0.0, 1.0)).is_ok());
        for damaging in [79_112_000, 79_112_005, 79_000_201, 79_000_961, 79_153_601, 79_000_900] {
            assert_eq!(
                spawn_data_synced(FieldInsHandle::none(), damaging, V3::ZERO, 1.0, V3::new(0.0, 0.0, 1.0)).err(),
                Some(SpawnError::NotSyncable(damaging)),
                "{damaging}"
            );
            let n = spawn_data(FieldInsHandle::none(), damaging, V3::ZERO, 1.0, V3::new(0.0, 0.0, 1.0));
            assert_eq!(n.unk44 & 0x08, 0, "the normal path never syncs");
        }
    }
}
