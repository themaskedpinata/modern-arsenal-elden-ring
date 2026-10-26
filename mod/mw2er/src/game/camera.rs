//! Camera sample: matrix row 2 (forward candidate, sign resolved in mw2er-core::aim), position and
//! the raw FOV value. Source is `CSCamera.pers_cam_1` (default) or `WorldChrMan.chr_cam`.

use std::mem::offset_of;

use eldenring::cs::{CSCam, CSCamera, ChrCam, WorldChrMan};
use mw2er_core::config::CameraSource;
use mw2er_core::vec3::V3;

use super::{plausible, read, read_ptr, singleton_addr, v3};

const _: () = assert!(size_of::<fromsoftware_shared::F32ViewMatrix>() == 64);

#[derive(Clone, Copy, Debug)]
pub struct CameraSample {
    pub row2: V3,
    pub pos: V3,
    pub fov_raw: f32,
    /// M3 CAMFRAME / camera gate: rows 0..2 of the matrix and the never-read projection fields.
    pub rows: [[f32; 3]; 3],
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

/// Reads a `CSCam` at `cam`.
///
/// # Safety
/// `cam` must be 0 or the address of a live `CSCam`.
unsafe fn read_cam(cam: usize) -> Option<CameraSample> {
    if !plausible(cam) {
        return None;
    }
    // SAFETY: live CSCam; the view matrix is four rows of four f32.
    let m = unsafe { read::<[[f32; 4]; 4]>(cam + offset_of!(CSCam, matrix)) }?;
    // SAFETY: as above.
    let fov_raw = unsafe { read::<f32>(cam + offset_of!(CSCam, fov)) }?;
    // SAFETY: as above (aspect_ratio, near_plane, far_plane follow fov in the crate's repr(C) layout).
    let (aspect, near, far) = unsafe {
        (
            read::<f32>(cam + offset_of!(CSCam, aspect_ratio)).unwrap_or(f32::NAN),
            read::<f32>(cam + offset_of!(CSCam, near_plane)).unwrap_or(f32::NAN),
            read::<f32>(cam + offset_of!(CSCam, far_plane)).unwrap_or(f32::NAN),
        )
    };
    let row2 = v3(m[2])?;
    let pos = v3(m[3])?;
    let rows = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    Some(CameraSample { row2, pos, fov_raw, rows, aspect, near, far })
}

pub fn read_camera(src: CameraSource) -> Option<CameraSample> {
    match src {
        CameraSource::PersCam1 => {
            let c = singleton_addr::<CSCamera>();
            if c == 0 {
                return None;
            }
            // SAFETY: live CSCamera; pers_cam_1 is an OwnedPtr read as an integer.
            let pc = unsafe { read_ptr(c + offset_of!(CSCamera, pers_cam_1)) };
            // SAFETY: pc is 0 or the camera the game owns.
            unsafe { read_cam(pc) }
        }
        CameraSource::ChrCam => {
            let w = singleton_addr::<WorldChrMan>();
            if w == 0 {
                return None;
            }
            // SAFETY: live WorldChrMan; chr_cam is an Option<NonNull<ChrCam>> read as an integer.
            let cc = unsafe { read_ptr(w + super::fp_read::chr_cam_offset()) };
            if cc == 0 {
                return None;
            }
            // SAFETY: ChrCam starts with its CSPersCam (= CSCam).
            unsafe { read_cam(cc + offset_of!(ChrCam, pers_cam)) }
        }
    }
}

/// M4: `CSCamera.pers_cam_1`'s address and its 17 floats at +0x10 (rows 0-3 + fov), for the read substitution
/// (docs/M4_DESIGN.md section 4.5) and the exact restore at the end of a ride.
pub fn read_pers_cam1_17() -> Option<(usize, [f32; 17])> {
    let c = singleton_addr::<CSCamera>();
    if c == 0 {
        return None;
    }
    // SAFETY: live CSCamera; pers_cam_1 is an OwnedPtr read as an integer.
    let pc = unsafe { read_ptr(c + offset_of!(CSCamera, pers_cam_1)) };
    if !plausible(pc) {
        return None;
    }
    // SAFETY: the game's CSCam; 17 f32 starting at the matrix (the fov follows row 3 at +0x50).
    unsafe { read::<[f32; 17]>(pc + offset_of!(CSCam, matrix)) }.map(|m| (pc, m))
}

const _: () = assert!(offset_of!(CSCam, fov) == offset_of!(CSCam, matrix) + 64);

/// A camera sample built from 17 floats (ER's own frame), keeping `base`'s aspect / near / far.
pub fn sample_from17(m: &[f32; 17], base: &CameraSample) -> Option<CameraSample> {
    let row2 = v3([m[8], m[9], m[10], m[11]])?;
    let pos = v3([m[12], m[13], m[14], m[15]])?;
    let rows = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    Some(CameraSample { row2, pos, fov_raw: m[16], rows, aspect: base.aspect, near: base.near, far: base.far })
}
