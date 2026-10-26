//! The movement controller's game fields (docs/M5_DESIGN.md section 4.1), read once per task tick into plain data.
//! Same rules as the rest of `game`: non-faulting reads, pointers read as integers and checked, enums as integers.
//! Every offset the design names by number is pinned with a compile-time assert against the vendored crate.

use std::mem::offset_of;

use eldenring::cs::{
    CSChrActionFlagModule, CSChrActionRequestModule, CSChrDataModule, CSChrPhysicsModule, ChrCtrl, ChrCtrlModifier,
    ChrCtrlModifierData, ChrIns, ChrInsModuleContainer, ChrPhysicsMaterialInfo, PlayerIns, WorldChrMan,
};
use mw2er_core::vec3::V3;

use super::{plausible, read, read_ptr, singleton_addr};

// docs/M5_DESIGN.md table 4.1 (compiled with offset_of! against fromsoftware-rs 59fbd3b)
pub const PHYS_POSITION: usize = offset_of!(CSChrPhysicsModule, position);
pub const PHYS_LAST_UPDATE: usize = offset_of!(CSChrPhysicsModule, last_update_position);
pub const PHYS_PROXY_REQ: usize = offset_of!(CSChrPhysicsModule, chr_proxy_pos_update_requested);
pub const PHYS_STANDING: usize = offset_of!(CSChrPhysicsModule, standing_on_solid_ground);
pub const PHYS_TOUCHING: usize = offset_of!(CSChrPhysicsModule, touching_solid_ground);
pub const PHYS_MOTION_MULT: usize = offset_of!(CSChrPhysicsModule, motion_multiplier);
pub const PHYS_GRAVITY_MULT: usize = offset_of!(CSChrPhysicsModule, gravity_multiplier);
pub const PHYS_IS_FALLING: usize = offset_of!(CSChrPhysicsModule, is_falling);
pub const PHYS_IS_TOUCHING_GROUND: usize = offset_of!(CSChrPhysicsModule, is_touching_ground);
pub const PHYS_GRAVITY_DISABLED: usize = offset_of!(CSChrPhysicsModule, gravity_disabled);
pub const PHYS_HIT_HEIGHT: usize = offset_of!(CSChrPhysicsModule, hit_height);
pub const PHYS_HIT_RADIUS: usize = offset_of!(CSChrPhysicsModule, hit_radius);
pub const PHYS_MATERIAL_NORMAL: usize =
    offset_of!(CSChrPhysicsModule, material_info) + offset_of!(ChrPhysicsMaterialInfo, normal_vector);
/// `root_motion` is private in the crate (unk); it sits right after `adjust_to_hi_collision` (+0xcc) and its 3 pad
/// bytes. Probe read only.
pub const PHYS_ROOT_MOTION: usize = 0xd0;
pub const CHR_CHUNK_POSITION: usize = offset_of!(ChrIns, chunk_position);
pub const PLAYER_BLOCK_POSITION: usize = offset_of!(PlayerIns, block_position);
pub const CHR_CTRL_PROXY_FLAGS: usize = offset_of!(ChrCtrl, chr_proxy_flags);
pub const DATA_HP: usize = offset_of!(CSChrDataModule, hp);
pub const DATA_MAX_HP: usize = offset_of!(CSChrDataModule, max_hp);

const _: () = assert!(PHYS_POSITION == 0x70);
const _: () = assert!(PHYS_LAST_UPDATE == 0x80);
const _: () = assert!(PHYS_PROXY_REQ == 0x91);
const _: () = assert!(PHYS_STANDING == 0x92);
const _: () = assert!(PHYS_TOUCHING == 0x93);
const _: () = assert!(offset_of!(CSChrPhysicsModule, adjust_to_hi_collision) == 0xcc);
const _: () = assert!(PHYS_MOTION_MULT == 0x1c4);
const _: () = assert!(PHYS_GRAVITY_MULT == 0x1cc);
const _: () = assert!(PHYS_IS_FALLING == 0x1d0);
const _: () = assert!(PHYS_IS_TOUCHING_GROUND == 0x1d1);
const _: () = assert!(PHYS_GRAVITY_DISABLED == 0x1d5);
const _: () = assert!(PHYS_HIT_HEIGHT == 0x2f0);
const _: () = assert!(PHYS_HIT_RADIUS == 0x2f4);
const _: () = assert!(PHYS_MATERIAL_NORMAL == 0x1f0 + 0x40);
const _: () = assert!(CHR_CHUNK_POSITION == 0x80);
const _: () = assert!(PLAYER_BLOCK_POSITION == 0x6c0);
const _: () = assert!(CHR_CTRL_PROXY_FLAGS == 0xfc);
const _: () = assert!(DATA_HP == 0x138);
const _: () = assert!(offset_of!(ChrIns, chr_ctrl) == 0x58);
const _: () = assert!(offset_of!(ChrIns, modules) == 0x190);
const _: () = assert!(offset_of!(ChrInsModuleContainer, physics) == 0x68);
const _: () = assert!(offset_of!(ChrCtrl, modifier) == 0xc8);
const _: () = assert!(offset_of!(ChrCtrlModifierData, movement_limit) == 0x30);
const _: () = assert!(offset_of!(CSChrActionFlagModule, animation_action_flags) == 0x10);
const _: () = assert!(offset_of!(CSChrActionFlagModule, action_modifiers_flags) == 0x40);
const _: () = assert!(offset_of!(CSChrActionRequestModule, action_requests) == 0x10);
// the position write keeps w, and the optional 32-byte write covers position + last_update_position
const _: () = assert!(PHYS_LAST_UPDATE == PHYS_POSITION + 16);

/// One tick's read of everything the controller needs. `Err(field)` = a required read failed.
#[derive(Clone, Copy, Debug)]
pub struct MoveRead {
    pub player: usize,
    pub physics: usize,
    pub chr_ctrl: usize,
    pub data_module: usize,
    /// physics.position (metres, Havok frame) and its w.
    pub pos: V3,
    pub pos_w: f32,
    pub is_touching_ground: bool,
    pub is_falling: bool,
    pub standing_on_solid_ground: bool,
    pub touching_solid_ground: bool,
    pub gravity_mult: f32,
    pub gravity_disabled: u8,
    pub hit_radius: f32,
    pub hit_height: f32,
    pub hp: i32,
    pub max_hp: i32,
    pub chunk: Option<V3>,
    pub block: Option<[f32; 4]>,
    // probes (optional)
    pub material_normal: Option<V3>,
    pub motion_mult: Option<f32>,
    pub root_motion: Option<[f32; 4]>,
    pub movement_limit: Option<u32>,
    pub anim_flags: Option<u64>,
    pub mod_flags: Option<u64>,
    pub action_requests: Option<u64>,
    pub proxy_flags: Option<u32>,
}

/// `Ok(None)` = not in world.
pub fn read_move() -> Result<Option<MoveRead>, &'static str> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return Ok(None);
    }
    // SAFETY (all reads): main_player read as an integer and checked; every module pointer checked before use;
    // integers and floats only.
    unsafe {
        let player = read_ptr(wcm + offset_of!(WorldChrMan, main_player));
        if player == 0 {
            return Ok(None);
        }
        if !plausible(player) {
            return Err("main_player");
        }
        let modules = read_ptr(player + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return Err("modules");
        }
        let physics = read_ptr(modules + offset_of!(ChrInsModuleContainer, physics));
        if !plausible(physics) {
            return Err("physics_module");
        }
        let data_module = read_ptr(modules + offset_of!(ChrInsModuleContainer, data));
        if !plausible(data_module) {
            return Err("data_module");
        }
        let p = read::<[f32; 4]>(physics + PHYS_POSITION).ok_or("physics.position")?;
        let pos = super::v3(p).ok_or("physics.position")?;
        let gravity_mult = read::<f32>(physics + PHYS_GRAVITY_MULT).ok_or("gravity_multiplier")?;
        let gravity_disabled = read::<u8>(physics + PHYS_GRAVITY_DISABLED).ok_or("gravity_disabled")?;
        let is_touching_ground = read::<u8>(physics + PHYS_IS_TOUCHING_GROUND).ok_or("is_touching_ground")? != 0;
        let is_falling = read::<u8>(physics + PHYS_IS_FALLING).ok_or("is_falling")? != 0;
        let standing_on_solid_ground = read::<u8>(physics + PHYS_STANDING).ok_or("standing_on_solid_ground")? != 0;
        let touching_solid_ground = read::<u8>(physics + PHYS_TOUCHING).ok_or("touching_solid_ground")? != 0;
        let hit_radius = read::<f32>(physics + PHYS_HIT_RADIUS).ok_or("hit_radius")?;
        let hit_height = read::<f32>(physics + PHYS_HIT_HEIGHT).ok_or("hit_height")?;
        let hp = read::<i32>(data_module + DATA_HP).ok_or("hp")?;
        let max_hp = read::<i32>(data_module + DATA_MAX_HP).ok_or("max_hp")?;
        let chunk = read::<[f32; 4]>(player + CHR_CHUNK_POSITION).and_then(super::v3);
        let block = read::<[f32; 4]>(player + PLAYER_BLOCK_POSITION);
        let material_normal = read::<[f32; 4]>(physics + PHYS_MATERIAL_NORMAL).and_then(super::v3);
        let motion_mult = read::<f32>(physics + PHYS_MOTION_MULT);
        let root_motion = read::<[f32; 4]>(physics + PHYS_ROOT_MOTION);
        let chr_ctrl = read_ptr(player + offset_of!(ChrIns, chr_ctrl));
        let (movement_limit, proxy_flags) = if plausible(chr_ctrl) {
            let m = read_ptr(chr_ctrl + offset_of!(ChrCtrl, modifier));
            let ml = if plausible(m) {
                read::<u32>(m + offset_of!(ChrCtrlModifier, data) + offset_of!(ChrCtrlModifierData, movement_limit))
            } else {
                None
            };
            (ml, read::<u32>(chr_ctrl + CHR_CTRL_PROXY_FLAGS))
        } else {
            (None, None)
        };
        let af = read_ptr(modules + offset_of!(ChrInsModuleContainer, action_flag));
        let (anim_flags, mod_flags) = if plausible(af) {
            (
                read::<u64>(af + offset_of!(CSChrActionFlagModule, animation_action_flags)),
                read::<u64>(af + offset_of!(CSChrActionFlagModule, action_modifiers_flags)),
            )
        } else {
            (None, None)
        };
        let ar = read_ptr(modules + offset_of!(ChrInsModuleContainer, action_request));
        let action_requests =
            if plausible(ar) { read::<u64>(ar + offset_of!(CSChrActionRequestModule, action_requests)) } else { None };
        Ok(Some(MoveRead {
            player,
            physics,
            chr_ctrl: if plausible(chr_ctrl) { chr_ctrl } else { 0 },
            data_module,
            pos,
            pos_w: p[3],
            is_touching_ground,
            is_falling,
            standing_on_solid_ground,
            touching_solid_ground,
            gravity_mult,
            gravity_disabled,
            hit_radius,
            hit_height,
            hp,
            max_hp,
            chunk,
            block,
            material_normal,
            motion_mult,
            root_motion,
            movement_limit,
            anim_flags,
            mod_flags,
            action_requests,
            proxy_flags,
        }))
    }
}

/// The gravity fields of a module, read fresh right before a restore. `None` = unreadable.
pub fn read_gravity(physics: usize) -> Option<(f32, u8)> {
    if !plausible(physics) {
        return None;
    }
    // SAFETY: non-faulting reads of an f32 and a byte.
    unsafe { Some((read::<f32>(physics + PHYS_GRAVITY_MULT)?, read::<u8>(physics + PHYS_GRAVITY_DISABLED)?)) }
}

/// main_player's physics module address now (0 = none), for the deferred-restore address check.
pub fn current_physics() -> usize {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return 0;
    }
    // SAFETY: integer reads, each pointer checked.
    unsafe {
        let player = read_ptr(wcm + offset_of!(WorldChrMan, main_player));
        if !plausible(player) {
            return 0;
        }
        let modules = read_ptr(player + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return 0;
        }
        let p = read_ptr(modules + offset_of!(ChrInsModuleContainer, physics));
        if plausible(p) { p } else { 0 }
    }
}
