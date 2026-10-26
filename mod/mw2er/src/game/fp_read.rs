//! Every first-person signal and field, read into plain data (docs/M3B_DESIGN.md sections 2-5). Same rules as the
//! rest of `game`: non-faulting reads, pointers read as integers and checked, engine enums read as integers.
//!
//! Field offsets come from `offset_of!` on the vendored crate's `#[repr(C)]` types; the ones the design names by
//! number are pinned with compile-time asserts below.

use std::mem::offset_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};

use eldenring::cs::{
    CSCam, CSCamera, CSChrActionFlagModule, CSChrDataModule, CSChrLadderModule, CSChrMenuFlags, CSChrPhysicsModule,
    CSChrRideModule, CSChrThrowModule, CSFD4FadePlate, CSFD4FadePlateColor, CSFade, CSMenuManImp, CSNowLoadingHelper,
    CSPlayerMenuCtrl, CSPopupMenu, CSRideNode, CSThrowNode, ChrCam, ChrIns, ChrInsModuleContainer, PlayerIns,
    WorldChrMan,
};
use mw2er_core::fp_logic::FADE_PLATES;
use mw2er_core::vec3::V3;

use super::{plausible, read, read_ptr, singleton_addr};

// Offsets the design names by number (docs/M3B_DESIGN.md sections 2.1, 3, 4).
const _: () = assert!(offset_of!(PlayerIns, chr_ins) == 0);
const _: () = assert!(offset_of!(ChrIns, chr_flags1c5) == 0x1c5);
const _: () = assert!(offset_of!(ChrIns, base_transparency) == 0x24c);
const _: () = assert!(offset_of!(ChrIns, base_transparency_modifier) == 0x250);
const _: () = assert!(offset_of!(ChrIns, is_locked_on) == 0xc9);
const _: () = assert!(offset_of!(CSChrPhysicsModule, orientation) == 0x50);
const _: () = assert!(offset_of!(CSChrPhysicsModule, interpolated_orientation) == 0x60);
const _: () = assert!(offset_of!(CSChrPhysicsModule, position) == 0x70);
const _: () = assert!(offset_of!(CSCamera, pers_cam_1) == 0x08);
// M8 ladder audit (docs/M8_DESIGN.md 10.2): the ladder signal's offsets, pinned
const _: () = assert!(offset_of!(ChrInsModuleContainer, ladder) == 0x78);
const _: () = assert!(offset_of!(CSChrLadderModule, state) == 0x18);
const _: () = assert!(offset_of!(CSChrLadderModule, top) == 0x20);
const _: () = assert!(offset_of!(CSChrLadderModule, bottom) == 0x30);
const _: () = assert!(offset_of!(CSCamera, camera_mask) == 0x28);
const _: () = assert!(offset_of!(CSCam, matrix) == 0x10);
const _: () = assert!(offset_of!(CSCam, fov) == 0x50);
const _: () = assert!(size_of::<eldenring::rotation::Quaternion>() == 16);
const _: () = assert!(size_of::<fromsoftware_shared::F32ViewMatrix>() == 64);

/// `CSCam` matrix row 3 (position) relative to the CSCam.
pub const CAM_ROW3: usize = offset_of!(CSCam, matrix) + 3 * 16;
pub const CAM_ROWS: usize = offset_of!(CSCam, matrix);
pub const CAM_FOV: usize = offset_of!(CSCam, fov);
pub const CAM_NEAR: usize = offset_of!(CSCam, near_plane);
pub const FLAGS1C5: usize = offset_of!(ChrIns, chr_flags1c5);
pub const BASE_TRANSPARENCY: usize = offset_of!(ChrIns, base_transparency);
pub const BASE_TRANSPARENCY_MOD: usize = offset_of!(ChrIns, base_transparency_modifier);
pub const ORIENTATION: usize = offset_of!(CSChrPhysicsModule, orientation);
pub const INTERP_ORIENTATION: usize = offset_of!(CSChrPhysicsModule, interpolated_orientation);
pub const PHYS_POSITION: usize = offset_of!(CSChrPhysicsModule, position);

// erfps2's camera handling (after run 5). `ChrCam` embeds its own CSPersCam at offset 0, followed by the private
// `ex_follow_cam` pointer (OwnedPtr<ChrExFollowCam> in erfps2's fromsoftware-rs fork @79266c9, camera.rs).
const _: () = assert!(offset_of!(ChrCam, pers_cam) == 0);
const _: () = assert!(size_of::<CSCam>() == 0x60);
const _: () = assert!(offset_of!(ChrCam, request_camera_reset) == 0x78);
/// `ChrCam.pers_cam` matrix row 3 (position) relative to the ChrCam.
pub const CHR_CAM_ROW3: usize = offset_of!(ChrCam, pers_cam) + CAM_ROW3;
/// `ChrCam.ex_follow_cam` (pointer; private in the vendored crate): right after the embedded CSPersCam (0x60 bytes).
pub const CHR_CAM_EX_FOLLOW_CAM: usize = size_of::<CSCam>();
/// `ChrExFollowCam.camera_auto_rotation` (bool): pers_cam 0x60 + chr_cam pointer 8 + 0x1d8 bytes = 0x240, from
/// erfps2's fork (`unk68: [u8; 0x1d8]` before it). Cross-checked on the user's exe: ER's follow-cam update (RVA
/// 0x3b7570) begins with `cmp byte ptr [rcx+0x240], ...`, and its prologue bytes are checked before hooking.
pub const FOLLOW_CAM_AUTO_ROTATION: usize = 0x240;

/// The render camera `CSCamera.pers_cam_1` (address + raw matrix + fov + near).
#[derive(Clone, Copy, Debug)]
pub struct PersCam {
    pub addr: usize,
    pub m: [[f32; 4]; 4],
    pub fov: f32,
    pub near: f32,
}

pub fn read_pers_cam1() -> Option<PersCam> {
    let c = singleton_addr::<CSCamera>();
    if c == 0 {
        return None;
    }
    // SAFETY: live CSCamera; pers_cam_1 is an OwnedPtr read as an integer.
    let addr = unsafe { read_ptr(c + offset_of!(CSCamera, pers_cam_1)) };
    if !plausible(addr) {
        return None;
    }
    // SAFETY: addr is the game's CSCam; the matrix is 4x4 f32, fov/near are f32.
    unsafe {
        let m = read::<[[f32; 4]; 4]>(addr + CAM_ROWS)?;
        let fov = read::<f32>(addr + CAM_FOV)?;
        let near = read::<f32>(addr + CAM_NEAR).unwrap_or(f32::NAN);
        Some(PersCam { addr, m, fov, near })
    }
}

/// The local player's first-person fields. `Err(field)` = a required read failed (handback `read_fail`).
#[derive(Clone, Copy, Debug)]
pub struct FpPlayer {
    /// PlayerIns address (== its ChrIns).
    pub player: usize,
    pub flags1c5: u8,
    pub hp: i32,
    pub physics: usize,
    pub pos: V3,
    pub orientation: [f32; 4],
    pub interpolated: [f32; 4],
    pub locked_on: bool,
    /// `action_modifiers_flags`; `None` when the module is not readable (yaw writes pause).
    pub action_flags: Option<u64>,
    /// `ride_node->ride_state`; `None` when there is no ride node (not riding).
    pub ride_state: Option<u32>,
    pub is_mounting: bool,
    pub is_mounted: bool,
    pub ladder: i8,
    /// M8: the ladder's top / bottom (Havok frame, metres) while on one; probe only (the `LADDER` lines).
    pub ladder_top: Option<V3>,
    pub ladder_bottom: Option<V3>,
    pub throw_state: Option<u32>,
    /// `player_menu_ctrl->chr_menu_flags.flags` bit 3 (pause menu disabled by TAE 54): probe only.
    pub pause_bit: Option<bool>,
}

/// Reads the player's first-person fields. `Ok(None)` = not in world (no main_player).
pub fn read_player_fp() -> Result<Option<FpPlayer>, &'static str> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return Ok(None);
    }
    // SAFETY: live WorldChrMan; main_player read as an integer.
    let player = unsafe { read_ptr(wcm + offset_of!(WorldChrMan, main_player)) };
    if player == 0 {
        return Ok(None);
    }
    if !plausible(player) {
        return Err("main_player");
    }
    // SAFETY (all reads): `player` is the live PlayerIns this tick; every intermediate pointer is checked before it
    // is followed; integers and floats only.
    unsafe {
        let flags1c5 = read::<u8>(player + FLAGS1C5).ok_or("chr_flags1c5")?;
        let locked_on = read::<u8>(player + offset_of!(ChrIns, is_locked_on)).ok_or("is_locked_on")? != 0;
        // The hide fields are read again (fresh) right before any write by read_hide_fields(); here they only keep this
        // read a superset of it (a failing hide read is always a read_fail here too).
        read::<f32>(player + BASE_TRANSPARENCY).ok_or("base_transparency")?;
        read::<f32>(player + BASE_TRANSPARENCY_MOD).ok_or("base_transparency_modifier")?;
        let modules = read_ptr(player + offset_of!(ChrIns, modules));
        if !plausible(modules) {
            return Err("modules");
        }
        let module = |off: usize, name: &'static str| -> Result<usize, &'static str> {
            let p = read_ptr(modules + off);
            if plausible(p) { Ok(p) } else { Err(name) }
        };
        let data = module(offset_of!(ChrInsModuleContainer, data), "data_module")?;
        let physics = module(offset_of!(ChrInsModuleContainer, physics), "physics_module")?;
        let ride = module(offset_of!(ChrInsModuleContainer, ride), "ride_module")?;
        let ladder_m = module(offset_of!(ChrInsModuleContainer, ladder), "ladder_module")?;
        let hp = read::<i32>(data + offset_of!(CSChrDataModule, hp)).ok_or("hp")?;
        let p = read::<[f32; 4]>(physics + PHYS_POSITION).ok_or("physics.position")?;
        let pos = super::v3(p).ok_or("physics.position")?;
        let orientation = read::<[f32; 4]>(physics + ORIENTATION).ok_or("physics.orientation")?;
        let interpolated = read::<[f32; 4]>(physics + INTERP_ORIENTATION).ok_or("physics.interpolated_orientation")?;
        let ride_node = read_ptr(ride + offset_of!(CSChrRideModule, ride_node));
        let ride_state =
            if plausible(ride_node) { Some(read::<u32>(ride_node + offset_of!(CSRideNode, ride_state)).ok_or("ride_state")?) } else { None };
        let is_mounting = read::<u8>(ride + offset_of!(CSChrRideModule, is_mounting)).ok_or("is_mounting")? != 0;
        let is_mounted = read::<u8>(ride + offset_of!(CSChrRideModule, is_mounted)).ok_or("is_mounted")? != 0;
        let ladder = read::<i8>(ladder_m + offset_of!(CSChrLadderModule, state)).ok_or("ladder.state")?;
        // M8: optional (a failing read never fails the whole sample)
        let (ladder_top, ladder_bottom) = if ladder != -1 {
            (
                read::<[f32; 4]>(ladder_m + offset_of!(CSChrLadderModule, top)).and_then(super::v3),
                read::<[f32; 4]>(ladder_m + offset_of!(CSChrLadderModule, bottom)).and_then(super::v3),
            )
        } else {
            (None, None)
        };
        // Optional: the throw and action-flag modules only gate yaw writes / add a trigger.
        let throw_m = read_ptr(modules + offset_of!(ChrInsModuleContainer, throw));
        let throw_node = if plausible(throw_m) { read_ptr(throw_m + offset_of!(CSChrThrowModule, throw_node)) } else { 0 };
        let throw_state =
            if plausible(throw_node) { read::<u32>(throw_node + offset_of!(CSThrowNode, throw_state)) } else { None };
        let af = read_ptr(modules + offset_of!(ChrInsModuleContainer, action_flag));
        let action_flags =
            if plausible(af) { read::<u64>(af + offset_of!(CSChrActionFlagModule, action_modifiers_flags)) } else { None };
        let pmc = read_ptr(player + offset_of!(PlayerIns, player_menu_ctrl));
        let pause_bit = if plausible(pmc) {
            read::<u32>(pmc + offset_of!(CSPlayerMenuCtrl, chr_menu_flags) + offset_of!(CSChrMenuFlags, flags)).map(|f| f & 0x8 != 0)
        } else {
            None
        };
        Ok(Some(FpPlayer {
            player,
            flags1c5,
            hp,
            physics,
            pos,
            orientation,
            interpolated,
            locked_on,
            action_flags,
            ride_state,
            is_mounting,
            is_mounted,
            ladder,
            ladder_top,
            ladder_bottom,
            throw_state,
            pause_bit,
        }))
    }
}

/// Just the fields the body hide/restore touches, read fresh right before a write (review M3b major 2 / minor 4):
/// independent of the data/physics/ride/ladder modules, so a failing module read can never keep the body hidden.
#[derive(Clone, Copy, Debug)]
pub struct HideFields {
    pub player: usize,
    pub flags1c5: u8,
    pub base_transparency: f32,
    pub base_transparency_modifier: f32,
}

/// `None` = not in world, or `main_player` / one of the three fields unreadable.
pub fn read_hide_fields() -> Option<HideFields> {
    let wcm = singleton_addr::<WorldChrMan>();
    if wcm == 0 {
        return None;
    }
    // SAFETY: live WorldChrMan; main_player read as an integer and checked; then a u8 and two f32 of that ChrIns.
    unsafe {
        let player = read_ptr(wcm + offset_of!(WorldChrMan, main_player));
        if !plausible(player) {
            return None;
        }
        Some(HideFields {
            player,
            flags1c5: read::<u8>(player + FLAGS1C5)?,
            base_transparency: read::<f32>(player + BASE_TRANSPARENCY)?,
            base_transparency_modifier: read::<f32>(player + BASE_TRANSPARENCY_MOD)?,
        })
    }
}

/// `CSCamera.pers_cam_1` read fresh (for the camera-step detour: no singleton lookup, the task publishes the
/// CSCamera instance address).
///
/// # Safety
/// None beyond the non-faulting read; `cscamera` may be anything.
pub unsafe fn read_pers_cam1_ptr(cscamera: usize) -> usize {
    // SAFETY: non-faulting read of a pointer-sized integer.
    unsafe { read_ptr(cscamera + offset_of!(CSCamera, pers_cam_1)) }
}

/// The CSCamera singleton instance address (0 = none).
pub fn cscamera_addr() -> usize {
    singleton_addr::<CSCamera>()
}

/// The WorldChrMan singleton instance address (0 = none).
pub fn world_chr_man_addr() -> usize {
    singleton_addr::<WorldChrMan>()
}

/// `WorldChrMan.chr_cam` read fresh (0 = none / unreadable / implausible). For the detours: no singleton lookup.
///
/// # Safety
/// None beyond the non-faulting read; `wcm` may be anything.
pub unsafe fn read_chr_cam_ptr(wcm: usize) -> usize {
    if wcm == 0 {
        return 0;
    }
    // SAFETY: non-faulting read of a pointer-sized integer.
    let p = unsafe { read_ptr(wcm + chr_cam_offset()) };
    if plausible(p) { p } else { 0 }
}

// 2.7.2.0 port: WorldChrMan grew between main_player and chr_cam, so chr_cam and the fields after it sit
// `wcm_tail_shift` bytes later than the vendored layout says. The build's shift is the default; `scan_chr_cam` (present
// thread only) confirms it by ChrCam's vtable and replaces it if the game disagrees.
static TAIL_SHIFT: AtomicIsize = AtomicIsize::new(isize::MIN);
static SCAN_TICK: AtomicU32 = AtomicU32::new(0);
static SCAN_DONE: AtomicBool = AtomicBool::new(false);
const SCAN_SPAN: isize = 0x100;
const SCAN_EVERY: u32 = 60;

/// Bytes to add to a vendored `WorldChrMan` offset at or after `chr_cam`.
pub fn wcm_tail_shift() -> isize {
    match TAIL_SHIFT.load(Ordering::Relaxed) {
        isize::MIN => crate::version_gate::own().map(|o| o.wcm_tail_shift as isize).unwrap_or(0),
        s => s,
    }
}

/// `WorldChrMan.chr_cam`'s offset in this build.
pub fn chr_cam_offset() -> usize {
    (offset_of!(WorldChrMan, chr_cam) as isize + wcm_tail_shift()) as usize
}

/// `WorldChrMan.chr_inses_by_distance`'s offset in this build.
pub fn chr_inses_by_distance_offset() -> usize {
    (offset_of!(WorldChrMan, chr_inses_by_distance) as isize + wcm_tail_shift()) as usize
}

/// Finds `WorldChrMan.chr_cam` by ChrCam's vtable once it exists (rate-limited; logs the result once).
fn scan_chr_cam(wcm: usize) {
    if SCAN_DONE.load(Ordering::Relaxed) || SCAN_TICK.fetch_add(1, Ordering::Relaxed) % SCAN_EVERY != 0 {
        return;
    }
    let Ok(own) = crate::version_gate::own() else { return };
    // SAFETY: `None` = the exe module (loaded for the process lifetime).
    let Ok(m) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return };
    let vmt = m.0 as usize + own.chr_cam_vmt;
    let base = offset_of!(WorldChrMan, chr_cam) as isize;
    let is_cam = |shift: isize| {
        // SAFETY: non-faulting reads of pointer-sized integers.
        let p = unsafe { read_ptr((wcm as isize + base + shift) as usize) };
        plausible(p) && unsafe { read_ptr(p) } == vmt
    };
    let want = own.wcm_tail_shift as isize;
    let found = if is_cam(want) { Some(want) } else { (-SCAN_SPAN..=SCAN_SPAN).step_by(8).find(|&s| is_cam(s)) };
    if let Some(s) = found {
        TAIL_SHIFT.store(s, Ordering::Relaxed);
        SCAN_DONE.store(true, Ordering::Relaxed);
        crate::log::write(
            crate::log::Level::Info,
            "FPREAD",
            &format!("chr_cam found at {:#x} shift={:+#x} build_default={:+#x} {}", base + s, s, want, if s == want { "ok" } else { "OVERRIDE" }),
        );
    }
}

/// `ChrCam.ex_follow_cam` read fresh (0 = none / unreadable / implausible).
///
/// # Safety
/// None beyond the non-faulting read; `chr_cam` may be anything.
pub unsafe fn read_follow_cam_ptr(chr_cam: usize) -> usize {
    if chr_cam == 0 {
        return 0;
    }
    // SAFETY: non-faulting read of a pointer-sized integer.
    let p = unsafe { read_ptr(chr_cam + CHR_CAM_EX_FOLLOW_CAM) };
    if plausible(p) { p } else { 0 }
}

/// World-level signals. Every field is optional: an absent singleton means "signal unavailable", not a failure.
#[derive(Clone, Copy, Debug)]
pub struct FpWorld {
    pub camera_type: Option<u32>,
    pub camera_mask: Option<u32>,
    pub fade: [Option<f32>; FADE_PLATES],
    pub fade_available: bool,
    pub load_counter: Option<i32>,
    pub talk_id: Option<i32>,
}

pub fn read_world_fp() -> FpWorld {
    // SAFETY (all reads): singletons are 0 or live; every pointer is checked; integers and floats only.
    unsafe {
        let wcm = singleton_addr::<WorldChrMan>();
        if wcm != 0 {
            scan_chr_cam(wcm);
        }
        let chr_cam = if wcm == 0 { 0 } else { read_ptr(wcm + chr_cam_offset()) };
        let camera_type = if plausible(chr_cam) { read::<u32>(chr_cam + offset_of!(ChrCam, camera_type)) } else { None };
        let csc = singleton_addr::<CSCamera>();
        let camera_mask = if csc == 0 { None } else { read::<u32>(csc + offset_of!(CSCamera, camera_mask)) };
        let mut fade = [None; FADE_PLATES];
        let f = singleton_addr::<CSFade>();
        if f != 0 {
            for (i, slot) in fade.iter_mut().enumerate() {
                let plate = read_ptr(f + offset_of!(CSFade, fade_plates) + i * size_of::<usize>());
                if plausible(plate) {
                    *slot = read::<f32>(plate + offset_of!(CSFD4FadePlate, current_color) + offset_of!(CSFD4FadePlateColor, a));
                }
            }
        }
        let nl = singleton_addr::<CSNowLoadingHelper>();
        let load_counter = if nl == 0 { None } else { read::<i32>(nl + offset_of!(CSNowLoadingHelper, menu_load_counter)) };
        let mm = singleton_addr::<CSMenuManImp>();
        let popup = if mm == 0 { 0 } else { read_ptr(mm + offset_of!(CSMenuManImp, popup_menu)) };
        let talk_id = if plausible(popup) { read::<i32>(popup + offset_of!(CSPopupMenu, current_talk_id)) } else { None };
        FpWorld { camera_type, camera_mask, fade, fade_available: f != 0, load_counter, talk_id }
    }
}
