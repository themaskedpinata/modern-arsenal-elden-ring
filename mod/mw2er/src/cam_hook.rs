//! Tier B of the first-person camera (docs/M3B_DESIGN.md section 2.3): MinHooks on Elden Ring's camera-step update
//! and (after run 5) on its follow-camera update, installed when `[first_person] enabled = true` and
//! `camera_mode = "hook"`, or (M4) when `[ac130] enabled = true`: the AC-130 gunship camera is mode 2 of the same
//! camera-step detour (docs/M4_DESIGN.md section 4).
//!
//! Attribution: Dasaav-dsv/erfps2 (MIT OR Apache-2.0), commit 57e92c1378e87670f011cdee4a715e7287995dac, read-only
//! clone, reference only (no code copied):
//! - RVAs for 1.17.1 WW / exe 2.7.1.0 from `src/rva/ww.rs`: `CAMERA_STEP_UPDATE_RVA` 0x3bc5f0,
//!   `UPDATE_FOLLOW_CAM_RVA` 0x3b7570, `CAM_WALL_RECOVERY_RVA` 0x3b19c80 (exe 2.7.2.0: re-found by erfps2's
//!   binary-mapper signatures in the exe file, 0x3bc5f0 / 0x3b7570 / 0x3b1cc80; per build in
//!   `mw2er_core::exe_build::OwnAddrs`, port 2.7.2.0);
//! - the ideas from `src/core.rs` (`update_cs_cam`, `update_follow_cam`) and `src/hooks.rs`: put the eye into BOTH
//!   `CSCamera.pers_cam_1` and `ChrCam.pers_cam`, clear the `CAM_WALL_RECOVERY` global and
//!   `ChrExFollowCam.camera_auto_rotation` before ER's follow-cam update while first person is on;
//! - the `ChrExFollowCam.camera_auto_rotation` offset (+0x240) from erfps2's fromsoftware-rs fork @79266c9.
//!
//! Our own design differs: erfps2 skips the original camera step in first person and owns the whole matrix; this
//! detour always calls it and then only moves the position rows. erfps2 never restores the cleared bytes; we save
//! them on the first clear of an episode and the task writes them back on every handback
//! (`mw2er_core::fp_restore::ClearedByte`, [`handback`]).
//!
//! Safety:
//! - installed after the version gate (2.7.1.0 or 2.7.2.0, every address preflighted) at the accepted build's
//!   addresses only (`version_gate::own()`), and only if the first bytes at each target equal the prologue read from
//!   the user's exe (`OwnAddrs::camera_step_prologue`, `follow_cam_prologue`), checked with a non-faulting read;
//! - the detours write only while the task armed them (`ACTIVE`) and the task heartbeat is fresh; they never
//!   allocate, lock, log or panic (an unwind is caught before it reaches the game's frame);
//! - every object is re-read from its owner every call and must equal the address the task published
//!   (`CSCamera.pers_cam_1`, `WorldChrMan.chr_cam`, `ChrCam.ex_follow_cam`);
//! - the eye must be finite and within `MAX_EYE_DIST_M` of ER's camera, else the frame is skipped (counted);
//! - heap writes go through `game::write` (page-checked, version-gated, non-faulting); the exe global through
//!   `game::write_exe_bool` (that one validated address only, values 0/1).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::mh::{MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::ac130_cam::{
    Aim, AimParams, BaseView, FOV_IDX, FrameIn, Orbit, check_frame, frame, merge_er, restore_plan, substitute_er,
};
use mw2er_core::ac130_shake::{MAX_QUAKES, QuakeSample, ShakeParams, sample as shake_sample};
use mw2er_core::fp_math::eye_position;
use mw2er_core::fp_restore::{ClearedByte, hook_row_restore};
use mw2er_core::vec3::V3;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::game::fp_read::{
    CAM_ROW3, CAM_ROWS, CHR_CAM_ROW3, FOLLOW_CAM_AUTO_ROTATION, PHYS_POSITION, read_chr_cam_ptr, read_follow_cam_ptr,
    read_pers_cam1_ptr,
};
use crate::game::{read, validate_exe_data_bool, write, write_exe_bool};
use crate::shared::{MOUSE_DX, MOUSE_DY, TASK_HEARTBEAT_MS, now_ms_if_started, now_us_if_started};

// The camera-step / follow-cam RVAs, their prologues and the CAM_WALL_RECOVERY RVA are per exe build in
// `mw2er_core::exe_build::{OWN_2710, OWN_2720}` (port 2.7.2.0): camera step 0x3bc5f0 and follow cam 0x3b7570 with the
// same prologues in both builds (follow cam: `mov rax,rsp; push rbx; sub rsp,0xe0; cmp byte ptr [rcx+0x240],..` - the
// last 6 bytes also pin the `camera_auto_rotation` offset 0x240 the detour clears); CAM_WALL_RECOVERY (a bool in the
// exe's `.data`, file value 0x01) 0x3b19c80 on 2.7.1.0, 0x3b1cc80 on 2.7.2.0.

/// The accepted build's camera-step RVA (0 = no build accepted).
pub fn camera_step_rva() -> usize {
    crate::version_gate::own().map_or(0, |o| o.camera_step)
}
/// The accepted build's follow-cam RVA (0 = no build accepted).
pub fn follow_cam_rva() -> usize {
    crate::version_gate::own().map_or(0, |o| o.follow_cam)
}
/// The accepted build's CAM_WALL_RECOVERY RVA (0 = no build accepted).
pub fn cam_wall_recovery_rva() -> usize {
    crate::version_gate::own().map_or(0, |o| o.cam_wall_recovery)
}
/// The eye may be at most this far from ER's camera (third person is ~3.5 m).
pub const MAX_EYE_DIST_M: f32 = 20.0;
const HEARTBEAT_MAX_MS: u64 = 250;
/// Handback waits at most this long for a detour that is mid-call on another thread.
const IDLE_WAIT_MAX_US: u128 = 2_000;

type StepFn = unsafe extern "C" fn(*mut c_void, *const c_void);
type FollowFn = unsafe extern "C" fn(*mut c_void);

static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static FOLLOW_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
pub static INSTALLED: AtomicBool = AtomicBool::new(false);
pub static FOLLOW_INSTALLED: AtomicBool = AtomicBool::new(false);
/// The validated `CAM_WALL_RECOVERY` address (0 = not validated: the global is never touched).
pub static WALL_ADDR: AtomicUsize = AtomicUsize::new(0);
/// Detour calls in flight (the handback waits for 0 after clearing `ACTIVE`).
static IN_DETOUR: AtomicI32 = AtomicI32::new(0);

// Published by the task (first_person.rs) for the detours.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub static CAM_ADDR: AtomicUsize = AtomicUsize::new(0);
/// The CSCamera instance; the detour re-reads `pers_cam_1` from it every frame and requires it to equal `CAM_ADDR`.
pub static CSCAMERA_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static PHYS_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static EYE_H_BITS: AtomicU32 = AtomicU32::new(0);
pub static EYE_FWD_BITS: AtomicU32 = AtomicU32::new(0);
/// WorldChrMan instance and its `chr_cam` (re-read and compared every call).
pub static WCM_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static CHR_CAM_ADDR: AtomicUsize = AtomicUsize::new(0);
/// Episode switches from `[first_person]` (published at FP on).
pub static SYNC_CHR_CAM: AtomicBool = AtomicBool::new(false);
pub static WALL_OFF: AtomicBool = AtomicBool::new(false);
pub static AUTOROT_OFF: AtomicBool = AtomicBool::new(false);
// The world FOV override (if any) is written by the task in both modes, not here.

// M5 (docs/M5_DESIGN.md section 1.6): the movement controller's feet (metres, Havok frame) and a sequence number,
// published by the task after its position write. While `MOVE_ACTIVE`, the detour builds the eye from these feet
// instead of `physics.position` (which already holds ER's next displacement), only when the sequence read twice around
// the feet is unchanged and the feet are within `MOVE_PUBLISH_MAX_BITS` of `physics.position`.
pub static MOVE_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static MOVE_FEET: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
pub static MOVE_SEQ: AtomicU32 = AtomicU32::new(0);
pub static MOVE_PUBLISH_MAX_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0 m
/// Camera-step detour calls (every call, both modes): the task's frame-order probe (`MOVE order`).
pub static DETOUR_CALLS: AtomicU64 = AtomicU64::new(0);
/// Frames whose eye used the published feet / fell back to `physics.position` while `MOVE_ACTIVE`.
pub static MOVE_FEET_USED: AtomicU64 = AtomicU64::new(0);
pub static MOVE_FEET_REJECTED: AtomicU64 = AtomicU64::new(0);

/// The task (the only writer) publishes the controller's feet under a seqlock (review M5 minor 5): the sequence is
/// odd while the three stores run and even again after them, so a reader overlapping the stores sees an odd or a
/// changed sequence and falls back to `physics.position` instead of mixing two ticks.
pub fn publish_feet(feet: V3, max_m: f32) {
    MOVE_PUBLISH_MAX_BITS.store(max_m.to_bits(), Ordering::Relaxed);
    let s = MOVE_SEQ.load(Ordering::Relaxed);
    MOVE_SEQ.store(s.wrapping_add(1) | 1, Ordering::Relaxed);
    std::sync::atomic::fence(Ordering::Release);
    for (i, v) in [feet.x, feet.y, feet.z].into_iter().enumerate() {
        MOVE_FEET[i].store(v.to_bits(), Ordering::Relaxed);
    }
    MOVE_SEQ.store((s | 1).wrapping_add(1), Ordering::Release);
    MOVE_ACTIVE.store(true, Ordering::Release);
}

/// The controller stopped driving: the detour goes back to `physics.position`.
pub fn unpublish_feet() {
    MOVE_ACTIVE.store(false, Ordering::SeqCst);
}

/// The feet the detour should use: the published ones when active, consistent (seq unchanged) and near `phys`.
/// No allocation, lock or log (detour-safe).
fn move_feet(phys: V3) -> V3 {
    if !MOVE_ACTIVE.load(Ordering::Acquire) {
        return phys;
    }
    let s1 = MOVE_SEQ.load(Ordering::Acquire);
    let f = V3::new(
        f32::from_bits(MOVE_FEET[0].load(Ordering::Relaxed)),
        f32::from_bits(MOVE_FEET[1].load(Ordering::Relaxed)),
        f32::from_bits(MOVE_FEET[2].load(Ordering::Relaxed)),
    );
    // the feet loads complete before the second sequence read (seqlock reader)
    std::sync::atomic::fence(Ordering::Acquire);
    let s2 = MOVE_SEQ.load(Ordering::Acquire);
    let max = f32::from_bits(MOVE_PUBLISH_MAX_BITS.load(Ordering::Relaxed));
    // an odd sequence = a publish in progress; a changed one = a publish overlapped the reads
    if s1 & 1 == 0 && s1 == s2 && f.is_finite() && (f - phys).len() <= max {
        MOVE_FEET_USED.fetch_add(1, Ordering::Relaxed);
        f
    } else {
        MOVE_FEET_REJECTED.fetch_add(1, Ordering::Relaxed);
        phys
    }
}

// Published by the detours for the task.
pub static ER_POS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
pub static LAST_EYE: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
pub static WRITES: AtomicU64 = AtomicU64::new(0);
pub static SKIPS: AtomicU64 = AtomicU64::new(0);
/// `ChrCam.pers_cam` row 3: ER's pre-write value and our eye of the latest frame, and whether we wrote this episode.
pub static CHR_ER_POS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
pub static CHR_LAST_EYE: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
pub static CHR_WROTE: AtomicBool = AtomicBool::new(false);
pub static CHR_WRITES: AtomicU64 = AtomicU64::new(0);
pub static CHR_SKIPS: AtomicU64 = AtomicU64::new(0);
/// Frames where ChrCam.pers_cam row 3 still held our eye from the frame before (ER did not rebuild it).
pub static CHR_PERSISTED: AtomicU64 = AtomicU64::new(0);
/// Follow-cam detour: calls that acted, clears, and the `ClearedByte` saves (atomic encoding) + owner.
pub static FOLLOW_CALLS: AtomicU64 = AtomicU64::new(0);
pub static FOLLOW_SKIPS: AtomicU64 = AtomicU64::new(0);
pub static AUTOROT_CLEARS: AtomicU64 = AtomicU64::new(0);
pub static WALL_CLEARS: AtomicU64 = AtomicU64::new(0);
static AUTOROT_SAVE: AtomicU32 = AtomicU32::new(0);
static AUTOROT_OWNER: AtomicUsize = AtomicUsize::new(0);
static WALL_SAVE: AtomicU32 = AtomicU32::new(0);

fn load4(a: &[AtomicU32; 4]) -> [f32; 4] {
    [0, 1, 2, 3].map(|i| f32::from_bits(a[i].load(Ordering::Relaxed)))
}

fn store4(a: &[AtomicU32; 4], v: [f32; 4]) {
    for (i, x) in v.iter().enumerate() {
        a[i].store(x.to_bits(), Ordering::Relaxed);
    }
}

/// ER's pre-write camera position saved by the detour in the current frame, and the eye it wrote.
pub fn er_pos_and_eye() -> ([f32; 4], [f32; 4]) {
    (load4(&ER_POS), load4(&LAST_EYE))
}

/// Counts a detour call in flight for the handback's idle wait.
struct InDetour;

impl InDetour {
    fn enter() -> InDetour {
        IN_DETOUR.fetch_add(1, Ordering::SeqCst);
        InDetour
    }
}

impl Drop for InDetour {
    fn drop(&mut self) {
        IN_DETOUR.fetch_sub(1, Ordering::SeqCst);
    }
}

fn armed() -> bool {
    if !ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    hb != 0 && now_ms_if_started().saturating_sub(hb) <= HEARTBEAT_MAX_MS
}

fn apply() {
    let _g = InDetour::enter();
    if !armed() {
        return;
    }
    let cam = CAM_ADDR.load(Ordering::Relaxed);
    let phys = PHYS_ADDR.load(Ordering::Relaxed);
    // The published CSCam may be up to HEARTBEAT_MAX_MS old: re-read CSCamera.pers_cam_1 now and require the same
    // address (review M3b minor 10), so a recreated camera is never written through a stale pointer.
    // SAFETY: non-faulting read of a pointer-sized integer.
    let fresh = unsafe { read_pers_cam1_ptr(CSCAMERA_ADDR.load(Ordering::Relaxed)) };
    if cam == 0 || fresh != cam {
        SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: non-faulting reads of the addresses the task published this frame (0 or stale reads fail or give
    // values that the checks below reject).
    let (Some(m), Some(p)) = (unsafe { read::<[[f32; 4]; 4]>(cam + CAM_ROWS) }, unsafe { read::<[f32; 4]>(phys + PHYS_POSITION) })
    else {
        SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let fwd = V3::new(m[2][0], m[2][1], m[2][2]);
    let er = V3::new(m[3][0], m[3][1], m[3][2]);
    let feet = move_feet(V3::new(p[0], p[1], p[2]));
    let eye = eye_position(
        feet,
        fwd,
        f32::from_bits(EYE_H_BITS.load(Ordering::Relaxed)),
        f32::from_bits(EYE_FWD_BITS.load(Ordering::Relaxed)),
    );
    if !(eye.is_finite() && er.is_finite() && (eye - er).len() <= MAX_EYE_DIST_M) {
        SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let new = [eye.x, eye.y, eye.z, m[3][3]];
    // SAFETY: `cam` is the CSCam we just read; row 3 is 16 bytes of f32 the engine rebuilds every frame.
    if unsafe { write(cam + CAM_ROW3, new) } {
        store4(&ER_POS, m[3]);
        store4(&LAST_EYE, new);
        WRITES.fetch_add(1, Ordering::Relaxed);
    } else {
        return;
    }
    // erfps2: the same eye into ER's own player camera (ChrCam.pers_cam), so the follow camera and everything that
    // reads the chr cam agree with the render camera.
    if SYNC_CHR_CAM.load(Ordering::Relaxed) {
        let chr = CHR_CAM_ADDR.load(Ordering::Relaxed);
        // SAFETY: non-faulting re-read of WorldChrMan.chr_cam.
        let fresh_chr = unsafe { read_chr_cam_ptr(WCM_ADDR.load(Ordering::Relaxed)) };
        // SAFETY: non-faulting read of ChrCam.pers_cam row 3.
        match (chr != 0 && fresh_chr == chr).then(|| unsafe { read::<[f32; 4]>(chr + CHR_CAM_ROW3) }).flatten() {
            Some(old) if old.iter().all(|x| x.is_finite()) => {
                let new_chr = [eye.x, eye.y, eye.z, old[3]];
                // If ER did not rebuild the row since our last write, the row still holds our eye: keep the last
                // real ER value as the restore value (counted: tells whether ER rebuilds ChrCam.pers_cam every frame).
                let persisted = CHR_WROTE.load(Ordering::Relaxed) && old.map(f32::to_bits) == load4(&CHR_LAST_EYE).map(f32::to_bits);
                if persisted {
                    CHR_PERSISTED.fetch_add(1, Ordering::Relaxed);
                }
                // SAFETY: ChrCam (address re-checked just now); row 3 of its embedded CSPersCam, 16 bytes of f32.
                if unsafe { write(chr + CHR_CAM_ROW3, new_chr) } {
                    if !persisted {
                        store4(&CHR_ER_POS, old);
                    }
                    store4(&CHR_LAST_EYE, new_chr);
                    CHR_WROTE.store(true, Ordering::Relaxed);
                    CHR_WRITES.fetch_add(1, Ordering::Relaxed);
                } else {
                    CHR_SKIPS.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {
                CHR_SKIPS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

unsafe extern "C" fn detour(this: *mut c_void, time: *const c_void) {
    DETOUR_CALLS.fetch_add(1, Ordering::Relaxed);
    let orig = TRAMPOLINE.load(Ordering::Acquire);
    if orig != 0 {
        // SAFETY: the MinHook trampoline of the camera-step update, called with the game's own arguments.
        let f: StepFn = unsafe { std::mem::transmute(orig) };
        // SAFETY: as above.
        unsafe { f(this, time) };
    }
    // An unwind must never cross this frame (neither body has panicking calls). Mode 2 (the AC-130) replaces the
    // first-person path while it is armed; mode 1 behaviour is unchanged.
    if MODE.load(Ordering::SeqCst) == MODE_GUNSHIP {
        if catch_unwind(AssertUnwindSafe(apply_gunship)).is_err() {
            AC_FAULT.store(true, Ordering::SeqCst);
        }
    } else {
        let _ = catch_unwind(AssertUnwindSafe(apply));
        // C6 part 18: the over-the-shoulder offset (third person only) before the view kick (which keeps row 3)
        let _ = catch_unwind(AssertUnwindSafe(apply_shoulder));
        let _ = catch_unwind(AssertUnwindSafe(apply_view_fx));
    }
}

// ---- M6: view kick + ADS sway + ADS zoom (docs/M6_DESIGN.md sections 6.5 and 8) -------------------------------------
//
// After ER's camera step (and the first-person eye write): the camera rows of pers_cam_1 turned by the published kick +
// sway angles and the fov narrowed by the ADS zoom ratio, written with ONE game::write of rows 0-3 + fov (17 floats;
// row 3 is written back as read). ER rebuilds the rows every frame [V M4], so nothing accumulates; a frame where the
// rows still hold our last write (ER skipped its step) is not kicked again (counted as stale). The task aims from the
// rows it reads after this, so the bullets go where the kicked crosshair points.

/// pers_cam_1 and CSCamera published by the task for the view effects (0 = off).
pub static VIEWFX_CAM: AtomicUsize = AtomicUsize::new(0);
pub static VIEWFX_CSCAMERA: AtomicUsize = AtomicUsize::new(0);
static VIEWFX_LAST: [AtomicU32; 17] = [const { AtomicU32::new(0) }; 17];
static VIEWFX_ER: [AtomicU32; 17] = [const { AtomicU32::new(0) }; 17];

/// ER's own fov when `fov_now` is exactly the zoomed fov our last view-fx write left in pers_cam_1 (else None).
pub fn viewfx_er_fov(fov_now: f32) -> Option<f32> {
    let last = VIEWFX_LAST[16].load(Ordering::Relaxed);
    if last == 0 || fov_now.to_bits() != last {
        return None;
    }
    let er = f32::from_bits(VIEWFX_ER[16].load(Ordering::Relaxed));
    (er.is_finite() && er > 0.0).then_some(er)
}

fn apply_view_fx() {
    use crate::shared::{VIEWFX_FOV_RATIO_BITS, VIEWFX_ON, VIEWFX_PITCH_BITS, VIEWFX_STALE, VIEWFX_WRITES, VIEWFX_YAW_BITS};
    if !VIEWFX_ON.load(Ordering::Relaxed) || MODE.load(Ordering::Relaxed) == MODE_GUNSHIP {
        return;
    }
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    if hb == 0 || now_ms_if_started().saturating_sub(hb) > HEARTBEAT_MAX_MS {
        return;
    }
    let pitch = f32::from_bits(VIEWFX_PITCH_BITS.load(Ordering::Relaxed));
    let yaw = f32::from_bits(VIEWFX_YAW_BITS.load(Ordering::Relaxed));
    let ratio = f32::from_bits(VIEWFX_FOV_RATIO_BITS.load(Ordering::Relaxed));
    if pitch == 0.0 && yaw == 0.0 && ratio == 1.0 {
        return;
    }
    let cam = VIEWFX_CAM.load(Ordering::Relaxed);
    // SAFETY: non-faulting read of a pointer-sized integer (the camera must still be the published one).
    let fresh = unsafe { read_pers_cam1_ptr(VIEWFX_CSCAMERA.load(Ordering::Relaxed)) };
    if cam == 0 || fresh != cam {
        return;
    }
    // SAFETY: non-faulting read of rows 0-3 + fov (CSCam+0x10..0x54).
    let Some(f) = (unsafe { read::<[f32; 17]>(cam + CAM_ROWS) }) else { return };
    if !f.iter().all(|x| x.is_finite()) {
        return;
    }
    // ER rebuilt rows and fov every frame in runs 8-10 (AC130CAM persisted=0 fov_persisted=0); should one of them still
    // hold our last write, the saved ER value is used instead, so the kick / zoom never compounds.
    let last = std::array::from_fn::<u32, 17, _>(|i| VIEWFX_LAST[i].load(Ordering::Relaxed));
    let mut f = f;
    if f[..12].iter().zip(&last[..12]).all(|(a, b)| a.to_bits() == *b) {
        VIEWFX_STALE.fetch_add(1, Ordering::Relaxed);
        for (i, v) in f.iter_mut().enumerate().take(12) {
            *v = f32::from_bits(VIEWFX_ER[i].load(Ordering::Relaxed));
        }
    } else {
        for (i, v) in f.iter().enumerate().take(12) {
            VIEWFX_ER[i].store(v.to_bits(), Ordering::Relaxed);
        }
    }
    if f[16].to_bits() == last[16] {
        f[16] = f32::from_bits(VIEWFX_ER[16].load(Ordering::Relaxed));
    } else {
        VIEWFX_ER[16].store(f[16].to_bits(), Ordering::Relaxed);
    }
    let m = [[f[0], f[1], f[2], f[3]], [f[4], f[5], f[6], f[7]], [f[8], f[9], f[10], f[11]], [f[12], f[13], f[14], f[15]]];
    let k = mw2er_core::kick::kick_rows(m, pitch, yaw);
    let fov = mw2er_core::kick::zoom_fov(f[16], ratio);
    let mut out = [0.0f32; 17];
    for r in 0..4 {
        out[r * 4..r * 4 + 4].copy_from_slice(&k[r]);
    }
    out[16] = fov;
    if !out.iter().all(|x| x.is_finite()) {
        return;
    }
    // SAFETY: the CSCam we just re-read; rows 0-3 + fov, 68 bytes of f32 the engine rebuilds every frame.
    if unsafe { write(cam + CAM_ROWS, out) } {
        for (i, v) in out.iter().enumerate() {
            VIEWFX_LAST[i].store(v.to_bits(), Ordering::Relaxed);
        }
        VIEWFX_WRITES.fetch_add(1, Ordering::Relaxed);
    }
}

// ---- C6 part 18: the over-the-shoulder camera (docs/C6_HOLD_DESIGN.md part 18) -----------------------------------
//
// After ER's camera step (first person never runs with it: the detour refuses while `ACTIVE` or in mode 2, counted as
// a conflict): pers_cam_1 row 3 moved by the task's published local offset `[right, up, back]` (already pulled in by
// the task's spring-arm ray), turned into the world with this frame's rows (mw2er_core::shoulder::world_offset). ONE
// game::write of row 3; ChrCam, the follow cam and CAM_WALL_RECOVERY are never touched, so ER's own camera collision,
// auto rotation and lock-on never see the offset. ER rebuilds row 3 every frame [V M4]: nothing is restored; a frame
// where the row still holds our last write (ER skipped its step) is offset from ER's saved position (counted).

/// Armed by the task while the offset applies (with the heartbeat rule).
pub static SH_ON: AtomicBool = AtomicBool::new(false);
pub static SH_CAM: AtomicUsize = AtomicUsize::new(0);
pub static SH_CSCAMERA: AtomicUsize = AtomicUsize::new(0);
static SH_LOCAL: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static SH_ER: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static SH_ER_FWD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static SH_LAST: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static SH_WROTE: AtomicBool = AtomicBool::new(false);
pub static SH_WRITES: AtomicU64 = AtomicU64::new(0);
pub static SH_SKIPS: AtomicU64 = AtomicU64::new(0);
pub static SH_PERSISTED: AtomicU64 = AtomicU64::new(0);
/// Frames where the shoulder was armed while first person or the gunship owned the camera (refused; must stay 0).
pub static SH_CONFLICTS: AtomicU64 = AtomicU64::new(0);

fn load3(a: &[AtomicU32; 3]) -> V3 {
    V3::new(f(&a[0]), f(&a[1]), f(&a[2]))
}

fn store3(a: &[AtomicU32; 3], v: V3) {
    for (i, x) in [v.x, v.y, v.z].into_iter().enumerate() {
        set(&a[i], x);
    }
}

fn apply_shoulder() {
    let _g = InDetour::enter();
    if !SH_ON.load(Ordering::SeqCst) {
        return;
    }
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    if hb == 0 || now_ms_if_started().saturating_sub(hb) > HEARTBEAT_MAX_MS {
        return;
    }
    // first person (incl. the scope peek) and the AC-130 own the camera: never offset theirs
    if ACTIVE.load(Ordering::SeqCst) || MODE.load(Ordering::SeqCst) == MODE_GUNSHIP {
        SH_CONFLICTS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let cam = SH_CAM.load(Ordering::Relaxed);
    // SAFETY: non-faulting read of a pointer-sized integer (the camera must still be the published one).
    let fresh = unsafe { read_pers_cam1_ptr(SH_CSCAMERA.load(Ordering::Relaxed)) };
    if cam == 0 || fresh != cam {
        SH_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: non-faulting read of rows 0-3 (CSCam+0x10..0x50).
    let Some(m) = (unsafe { read::<[[f32; 4]; 4]>(cam + CAM_ROWS) }) else {
        SH_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let pos = V3::new(m[3][0], m[3][1], m[3][2]);
    let last = load3(&SH_LAST);
    let ours = SH_WROTE.load(Ordering::Relaxed)
        && pos.x.to_bits() == last.x.to_bits()
        && pos.y.to_bits() == last.y.to_bits()
        && pos.z.to_bits() == last.z.to_bits();
    // ER did not rebuild row 3 since our last write: offset its saved position (never compound)
    let er = if ours {
        SH_PERSISTED.fetch_add(1, Ordering::Relaxed);
        load3(&SH_ER)
    } else {
        pos
    };
    let fwd = V3::new(m[2][0], m[2][1], m[2][2]);
    let Some(off) = mw2er_core::shoulder::world_offset(fwd, load3(&SH_LOCAL)) else {
        SH_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let new = er + off;
    if !(new.is_finite() && er.is_finite() && m[3][3].is_finite()) {
        SH_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: `cam` is the CSCam re-read above; row 3 is 16 bytes of f32 the engine rebuilds every frame.
    if unsafe { write(cam + CAM_ROW3, [new.x, new.y, new.z, m[3][3]]) } {
        store3(&SH_ER, er);
        store3(&SH_ER_FWD, fwd);
        store3(&SH_LAST, new);
        SH_WROTE.store(true, Ordering::Relaxed);
        SH_WRITES.fetch_add(1, Ordering::Relaxed);
    } else {
        SH_SKIPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Task: arm / update the offset (the local `[right, up, back]` after the spring arm) and the camera it goes into.
pub fn shoulder_publish(local: V3, cam: usize, cscamera: usize) {
    store3(&SH_LOCAL, local);
    SH_CAM.store(cam, Ordering::SeqCst);
    SH_CSCAMERA.store(cscamera, Ordering::SeqCst);
    if !SH_ON.load(Ordering::SeqCst) {
        SH_WROTE.store(false, Ordering::SeqCst);
        SH_ON.store(true, Ordering::SeqCst);
    }
}

/// Task: disarm (nothing to restore: ER rebuilds row 3 every frame), then wait up to 2 ms for a call in flight.
pub fn shoulder_stop() {
    if !SH_ON.swap(false, Ordering::SeqCst) {
        return;
    }
    let t = std::time::Instant::now();
    while IN_DETOUR.load(Ordering::SeqCst) > 0 && t.elapsed().as_micros() < IDLE_WAIT_MAX_US {
        std::hint::spin_loop();
    }
    store3(&SH_LOCAL, V3::ZERO);
}

/// ER's own camera (position, forward) under this frame's shoulder write: when `read_pos` is exactly the position
/// the detour wrote, the position ER built before it (the spring arm's ray starts there). None = no write to undo.
pub fn shoulder_er_view(read_pos: V3) -> Option<(V3, V3)> {
    if !SH_WROTE.load(Ordering::Relaxed) {
        return None;
    }
    let last = load3(&SH_LAST);
    let same = read_pos.x.to_bits() == last.x.to_bits() && read_pos.y.to_bits() == last.y.to_bits() && read_pos.z.to_bits() == last.z.to_bits();
    same.then(|| (load3(&SH_ER), load3(&SH_ER_FWD)))
}

// ---- M4: camera mode 2, the AC-130 gunship (docs/M4_DESIGN.md section 4) ---------------------------------------
//
// After ER's original camera step: save ER's 17 floats (rows 0-3 + fov, CSCam+0x10..0x54), compute the gunship
// frame (mw2er_core::ac130_cam::frame) from the published atomics and write it with ONE game::write. ChrCam, the
// follow cam and CAM_WALL_RECOVERY are never touched in mode 2. The task reads ER's saved matrix instead of ours
// (`er_view_for_task`), and restores ER's frame on exit only if the field still holds our write.

/// 0 = off (first person may use the hook), 2 = gunship.
pub static MODE: AtomicU8 = AtomicU8::new(0);
pub const MODE_GUNSHIP: u8 = 2;
/// Armed by the task for the ride (with the heartbeat rule), cleared by `ac130_stop`.
pub static AC_ACTIVE: AtomicBool = AtomicBool::new(false);
/// The detour caught a panic in mode 2 (the task ends the ride with `panic`).
pub static AC_FAULT: AtomicBool = AtomicBool::new(false);
pub static AC_CAM_ADDR: AtomicUsize = AtomicUsize::new(0);
pub static AC_CSCAMERA_ADDR: AtomicUsize = AtomicUsize::new(0);
static AC_CENTER: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static AC_FEET: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static AC_THETA0: AtomicU32 = AtomicU32::new(0);
static AC_R: AtomicU32 = AtomicU32::new(0);
static AC_H: AtomicU32 = AtomicU32::new(0);
static AC_PERIOD: AtomicU32 = AtomicU32::new(0);
static AC_DIR: AtomicU32 = AtomicU32::new(0);
static AC_BASE_TAG: AtomicBool = AtomicBool::new(false);
static AC_TAG_PITCH: AtomicU32 = AtomicU32::new(0);
static AC_ARC: AtomicU32 = AtomicU32::new(0);
static AC_FOV: AtomicU32 = AtomicU32::new(0);
static AC_DEG_PER_COUNT: AtomicU32 = AtomicU32::new(0);
static AC_ZOOM_SCALE: AtomicU32 = AtomicU32::new(0);
static AC_INVERT: AtomicBool = AtomicBool::new(false);
static AC_MAX_DIST: AtomicU32 = AtomicU32::new(0);
static AC_T0_US: AtomicU64 = AtomicU64::new(0);
static AIM_YAW: AtomicU32 = AtomicU32::new(0);
static AIM_PITCH: AtomicU32 = AtomicU32::new(0);
// After run 9: the earthquake slots (ac130_shake; the task pushes, the detour samples) and the shake look.
static SHAKE_AMP: [AtomicU32; MAX_QUAKES] = [const { AtomicU32::new(0) }; MAX_QUAKES];
static SHAKE_DUR: [AtomicU32; MAX_QUAKES] = [const { AtomicU32::new(0) }; MAX_QUAKES];
static SHAKE_T0_US: [AtomicU64; MAX_QUAKES] = [const { AtomicU64::new(0) }; MAX_QUAKES];
static SHAKE_SEED: [AtomicU32; MAX_QUAKES] = [const { AtomicU32::new(0) }; MAX_QUAKES];
static SHAKE_NEXT: AtomicUsize = AtomicUsize::new(0);
static SHAKE_ROT: AtomicU32 = AtomicU32::new(0);
static SHAKE_ROLL: AtomicU32 = AtomicU32::new(0);
static SHAKE_POS: AtomicU32 = AtomicU32::new(0);
static SHAKE_FREQ: AtomicU32 = AtomicU32::new(0);
static CAM_AIM_POS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static CAM_AIM_FWD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
// Outputs of the detour.
static ER_MAT: [AtomicU32; 17] = [const { AtomicU32::new(0) }; 17];
static LAST_WRITE: [AtomicU32; 17] = [const { AtomicU32::new(0) }; 17];
static CAM_POS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static CAM_FWD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static AC_WROTE: AtomicBool = AtomicBool::new(false);
pub static AC_WRITES: AtomicU64 = AtomicU64::new(0);
pub static AC_SKIPS: AtomicU64 = AtomicU64::new(0);
pub static AC_PERSISTED: AtomicU64 = AtomicU64::new(0);
/// Frames in which the fov (CSCam+0x50) still held our last write (review M4 major 1: ER may rebuild the rows every
/// frame but not the fov; ER's saved fov is then kept per float).
pub static AC_FOV_PERSISTED: AtomicU64 = AtomicU64::new(0);
/// Last bounds rejection (ac130_cam::Reject as u8 + 1; 0 = none) for the log.
pub static AC_LAST_REJECT: AtomicU8 = AtomicU8::new(0);

fn f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

fn set(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

fn load17(a: &[AtomicU32; 17]) -> [f32; 17] {
    let mut m = [0.0f32; 17];
    for (i, x) in m.iter_mut().enumerate() {
        *x = f32::from_bits(a[i].load(Ordering::Relaxed));
    }
    m
}

fn store17(a: &[AtomicU32; 17], m: &[f32; 17]) {
    for (i, x) in m.iter().enumerate() {
        a[i].store(x.to_bits(), Ordering::Relaxed);
    }
}

fn armed_gunship() -> bool {
    if MODE.load(Ordering::SeqCst) != MODE_GUNSHIP || !AC_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    let hb = TASK_HEARTBEAT_MS.load(Ordering::Relaxed);
    hb != 0 && now_ms_if_started().saturating_sub(hb) <= HEARTBEAT_MAX_MS
}

/// The mode-2 detour body: no allocation, lock, log or panic; exactly one game::write.
fn apply_gunship() {
    let _g = InDetour::enter();
    if !armed_gunship() {
        return;
    }
    let cam = AC_CAM_ADDR.load(Ordering::Relaxed);
    // SAFETY: non-faulting read of a pointer-sized integer (the published CSCamera instance).
    let fresh = unsafe { read_pers_cam1_ptr(AC_CSCAMERA_ADDR.load(Ordering::Relaxed)) };
    if cam == 0 || fresh != cam {
        AC_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: non-faulting read of the 17 floats at CSCam+0x10 (rows 0-3 + fov).
    let Some(cur) = (unsafe { read::<[f32; 17]>(cam + CAM_ROWS) }) else {
        AC_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    // Per float: a float ER did not rebuild since our last write keeps the older saved ER value (review M4 major 1).
    let (er, persisted) = merge_er(&load17(&ER_MAT), &cur, &load17(&LAST_WRITE), AC_WROTE.load(Ordering::Relaxed));
    if persisted == (1 << 17) - 1 {
        AC_PERSISTED.fetch_add(1, Ordering::Relaxed);
    }
    if persisted & (1 << FOV_IDX) != 0 {
        AC_FOV_PERSISTED.fetch_add(1, Ordering::Relaxed);
    }
    store17(&ER_MAT, &er);
    let mut aim = Aim { yaw_deg: f(&AIM_YAW), pitch_deg: f(&AIM_PITCH) };
    let ap = AimParams {
        deg_per_count: f(&AC_DEG_PER_COUNT),
        zoom_scale: f(&AC_ZOOM_SCALE),
        invert_y: AC_INVERT.load(Ordering::Relaxed),
        arc_deg: f(&AC_ARC),
    };
    aim.integrate(MOUSE_DX.swap(0, Ordering::Relaxed), MOUSE_DY.swap(0, Ordering::Relaxed), &ap);
    set(&AIM_YAW, aim.yaw_deg);
    set(&AIM_PITCH, aim.pitch_deg);
    let now = now_us_if_started();
    let t = now.saturating_sub(AC_T0_US.load(Ordering::Relaxed)) as f32 / 1e6;
    let mut quakes = [QuakeSample::default(); MAX_QUAKES];
    for (k, q) in quakes.iter_mut().enumerate() {
        *q = QuakeSample {
            amp: f(&SHAKE_AMP[k]),
            duration_s: f(&SHAKE_DUR[k]),
            age_s: now.saturating_sub(SHAKE_T0_US[k].load(Ordering::Relaxed)) as f32 / 1e6,
            seed: SHAKE_SEED[k].load(Ordering::Relaxed),
        };
    }
    let sp = ShakeParams {
        scale: 1.0,
        rot_deg: f(&SHAKE_ROT),
        roll_deg: f(&SHAKE_ROLL),
        pos_m: f(&SHAKE_POS),
        freq_hz: f(&SHAKE_FREQ),
        mw2_falloff: false,
    };
    let fi = FrameIn {
        orbit: Orbit {
            center: V3::new(f(&AC_CENTER[0]), f(&AC_CENTER[1]), f(&AC_CENTER[2])),
            radius: f(&AC_R),
            height: f(&AC_H),
            period_s: f(&AC_PERIOD),
            dir: f(&AC_DIR),
            theta0: f(&AC_THETA0),
        },
        t,
        base: if AC_BASE_TAG.load(Ordering::Relaxed) { BaseView::Mw2Tag } else { BaseView::Center },
        tag_pitch_deg: f(&AC_TAG_PITCH),
        aim,
        shake: shake_sample(&quakes, &sp),
        fov: f(&AC_FOV),
        er_w: [er[3], er[7], er[11], er[15]],
    };
    let Some(out) = frame(&fi) else {
        AC_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let feet = V3::new(f(&AC_FEET[0]), f(&AC_FEET[1]), f(&AC_FEET[2]));
    if let Err(r) = check_frame(&out.m, feet, f(&AC_MAX_DIST)) {
        AC_LAST_REJECT.store(r as u8 + 1, Ordering::Relaxed);
        AC_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: `cam` is the CSCam re-read above; rows 0-3 + fov are 68 bytes of f32 the engine rebuilds every frame.
    if unsafe { write(cam + CAM_ROWS, out.m) } {
        store17(&LAST_WRITE, &out.m);
        for i in 0..3 {
            set(&CAM_POS[i], [out.pos.x, out.pos.y, out.pos.z][i]);
            set(&CAM_FWD[i], [out.fwd.x, out.fwd.y, out.fwd.z][i]);
            set(&CAM_AIM_POS[i], [out.aim_pos.x, out.aim_pos.y, out.aim_pos.z][i]);
            set(&CAM_AIM_FWD[i], [out.aim_fwd.x, out.aim_fwd.y, out.aim_fwd.z][i]);
        }
        AC_WROTE.store(true, Ordering::Relaxed);
        AC_WRITES.fetch_add(1, Ordering::Relaxed);
    } else {
        AC_SKIPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Everything the detour needs for one ride (published once at Active; the fov/zoom change with the gun).
#[derive(Clone, Copy, Debug)]
pub struct GunshipParams {
    pub cam_addr: usize,
    pub cscamera: usize,
    pub orbit: Orbit,
    pub base_tag: bool,
    pub tag_pitch_deg: f32,
    pub arc_deg: f32,
    pub fov: f32,
    pub deg_per_count: f32,
    pub zoom_scale: f32,
    pub invert_y: bool,
    pub max_dist: f32,
}

/// Starts mode 2 (task): publishes the ride, resets the aim and the mouse accumulators, arms the detour.
pub fn ac130_start(p: &GunshipParams, feet: V3) {
    AC_CAM_ADDR.store(p.cam_addr, Ordering::SeqCst);
    AC_CSCAMERA_ADDR.store(p.cscamera, Ordering::SeqCst);
    for (i, v) in [p.orbit.center.x, p.orbit.center.y, p.orbit.center.z].into_iter().enumerate() {
        set(&AC_CENTER[i], v);
    }
    set_feet(feet);
    set(&AC_THETA0, p.orbit.theta0);
    set(&AC_R, p.orbit.radius);
    set(&AC_H, p.orbit.height);
    set(&AC_PERIOD, p.orbit.period_s);
    set(&AC_DIR, p.orbit.dir);
    AC_BASE_TAG.store(p.base_tag, Ordering::SeqCst);
    set(&AC_TAG_PITCH, p.tag_pitch_deg);
    set(&AC_ARC, p.arc_deg);
    set_gun(p.fov, p.zoom_scale);
    set(&AC_DEG_PER_COUNT, p.deg_per_count);
    AC_INVERT.store(p.invert_y, Ordering::SeqCst);
    set(&AC_MAX_DIST, p.max_dist);
    set(&AIM_YAW, 0.0);
    set(&AIM_PITCH, 0.0);
    for a in &SHAKE_AMP {
        set(a, 0.0);
    }
    MOUSE_DX.swap(0, Ordering::SeqCst);
    MOUSE_DY.swap(0, Ordering::SeqCst);
    AC_WROTE.store(false, Ordering::SeqCst);
    AC_FAULT.store(false, Ordering::SeqCst);
    AC_LAST_REJECT.store(0, Ordering::SeqCst);
    AC_T0_US.store(now_us_if_started(), Ordering::SeqCst);
    MODE.store(MODE_GUNSHIP, Ordering::SeqCst);
    AC_ACTIVE.store(true, Ordering::SeqCst);
}

/// Per tick (task): the feet for the bounds check.
pub fn set_feet(feet: V3) {
    for (i, v) in [feet.x, feet.y, feet.z].into_iter().enumerate() {
        set(&AC_FEET[i], v);
    }
}

/// Gun switch: the fov (radians) and the zoom sensitivity scale.
pub fn set_gun(fov: f32, zoom_scale: f32) {
    set(&AC_FOV, fov);
    set(&AC_ZOOM_SCALE, zoom_scale);
}

/// The shake look (task; `[ac130.shake]` is hot, so it is published with every quake).
pub fn set_shake_params(p: &ShakeParams) {
    set(&SHAKE_ROT, p.rot_deg);
    set(&SHAKE_ROLL, p.roll_deg);
    set(&SHAKE_POS, p.pos_m);
    set(&SHAKE_FREQ, p.freq_hz);
}

/// An MW2 earthquake starting now (task): `amp` = MW2 scale x `[ac130.shake]` scale x falloff gain
/// (`ac130_shake::quake_amp`), into the next of the `MAX_QUAKES` slots. The amplitude is cleared first and written
/// last, so the detour never pairs a new amplitude with an old start time.
pub fn push_quake(amp: f32, dur_s: f32, seed: u32) {
    let k = SHAKE_NEXT.fetch_add(1, Ordering::Relaxed) % MAX_QUAKES;
    set(&SHAKE_AMP[k], 0.0);
    set(&SHAKE_DUR[k], dur_s);
    SHAKE_SEED[k].store(seed, Ordering::Relaxed);
    SHAKE_T0_US[k].store(now_us_if_started(), Ordering::Relaxed);
    set(&SHAKE_AMP[k], amp);
}

/// Stops mode 2 (task, end step 1): disarm, then wait up to 2 ms for a detour call in flight.
pub fn ac130_stop() {
    AC_ACTIVE.store(false, Ordering::SeqCst);
    MODE.store(0, Ordering::SeqCst);
    let t = std::time::Instant::now();
    while IN_DETOUR.load(Ordering::SeqCst) > 0 && t.elapsed().as_micros() < IDLE_WAIT_MAX_US {
        std::hint::spin_loop();
    }
    MOUSE_DX.swap(0, Ordering::SeqCst);
    MOUSE_DY.swap(0, Ordering::SeqCst);
}

/// The gunship camera the user sees (the detour's last write): position and forward.
pub fn gunship_view() -> Option<(V3, V3, [[f32; 3]; 3], f32)> {
    if !AC_WROTE.load(Ordering::Relaxed) {
        return None;
    }
    let m = load17(&LAST_WRITE);
    let pos = V3::new(f(&CAM_POS[0]), f(&CAM_POS[1]), f(&CAM_POS[2]));
    let fwd = V3::new(f(&CAM_FWD[0]), f(&CAM_FWD[1]), f(&CAM_FWD[2]));
    let rows = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    Some((pos, fwd, rows, m[16]))
}

/// The unshaken gunship view (position, forward): where the guns point (the shake only moves the picture).
pub fn gunship_aim() -> Option<(V3, V3)> {
    if !AC_WROTE.load(Ordering::Relaxed) {
        return None;
    }
    let pos = V3::new(f(&CAM_AIM_POS[0]), f(&CAM_AIM_POS[1]), f(&CAM_AIM_POS[2]));
    let fwd = V3::new(f(&CAM_AIM_FWD[0]), f(&CAM_AIM_FWD[1]), f(&CAM_AIM_FWD[2]));
    Some((pos, fwd))
}

/// The aim offsets (degrees) for the periodic log.
pub fn aim_offsets() -> (f32, f32) {
    (f(&AIM_YAW), f(&AIM_PITCH))
}

/// ER's own frame of the latest gunship write (rows, position, fov).
pub fn er_frame() -> [f32; 17] {
    load17(&ER_MAT)
}

/// Task-side read substitution (docs/M4_DESIGN.md section 4.5): every float of pers_cam_1 that still holds our last
/// gunship write is replaced by ER's saved value, so the camera gate, CAMFRAME and the UMP45 aim never see the gunship.
pub fn er_view_for_task(read17: &[f32; 17]) -> Option<[f32; 17]> {
    if !AC_WROTE.load(Ordering::Relaxed) {
        return None;
    }
    substitute_er(read17, &load17(&LAST_WRITE), &load17(&ER_MAT))
}

/// What an end-of-ride restore did (for the log).
#[derive(Clone, Copy, Debug)]
pub struct GunshipRestore {
    pub ok: bool,
    pub ours: [f32; 17],
    pub er: [f32; 17],
    /// Floats that still held our write and got ER's saved value back.
    pub floats: u32,
    /// The fov (CSCam+0x50) was among them.
    pub fov: bool,
}

/// End step 2 (task, after `ac130_stop`): per float, ER's saved value goes back into every float that still holds our
/// last write; floats ER rebuilt are left as they are (review M4 major 1). `None` when nothing was written this ride
/// or ER rebuilt all 17 floats.
pub fn restore_er_frame(cur: &[f32; 17], addr: usize) -> Option<GunshipRestore> {
    if !AC_WROTE.swap(false, Ordering::SeqCst) {
        return None;
    }
    let ours = load17(&LAST_WRITE);
    let er = load17(&ER_MAT);
    let (out, floats, fov) = restore_plan(cur, &ours, &er)?;
    // SAFETY: the same pers_cam_1 (the caller re-read it this tick, on the game thread that runs the camera step);
    // rebuilt floats are written with the value just read, the others with ER's saved value.
    let ok = unsafe { write(addr + CAM_ROWS, out) };
    Some(GunshipRestore { ok, ours, er, floats, fov })
}

/// One `ClearedByte` step on an atomically stored save: returns true when the field must be written to 0 now.
fn clear_step(save: &AtomicU32, current: u8) -> bool {
    let mut st = ClearedByte::decode(save.load(Ordering::SeqCst));
    let step = st.step(current);
    if step.save.is_some() {
        save.store(st.encode(), Ordering::SeqCst);
    }
    step.clear
}

/// Before ER's follow-cam update, while first person is on: camera auto rotation and wall recovery off.
fn follow_pre(fc: usize) {
    let _g = InDetour::enter();
    // M4: the follow camera is never touched in mode 2 (the gunship writes CSCamera.pers_cam_1 only)
    if MODE.load(Ordering::SeqCst) == MODE_GUNSHIP || !armed() {
        return;
    }
    // Only the player's follow cam: WorldChrMan.chr_cam (re-read, == published) -> ex_follow_cam (re-read) == this.
    // SAFETY: non-faulting reads of pointer-sized integers.
    let chr = unsafe { read_chr_cam_ptr(WCM_ADDR.load(Ordering::Relaxed)) };
    // SAFETY: as above.
    if chr == 0 || chr != CHR_CAM_ADDR.load(Ordering::Relaxed) || unsafe { read_follow_cam_ptr(chr) } != fc {
        FOLLOW_SKIPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    FOLLOW_CALLS.fetch_add(1, Ordering::Relaxed);
    if AUTOROT_OFF.load(Ordering::Relaxed) {
        let owner = AUTOROT_OWNER.load(Ordering::SeqCst);
        // a save that belongs to another follow-cam object is left for the handback (it drops it)
        if owner == 0 || owner == fc {
            // SAFETY: non-faulting read of one byte of this ChrExFollowCam.
            if let Some(cur) = unsafe { read::<u8>(fc + FOLLOW_CAM_AUTO_ROTATION) } {
                AUTOROT_OWNER.store(fc, Ordering::SeqCst);
                // SAFETY: the player's ChrExFollowCam (checked above); one bool.
                if clear_step(&AUTOROT_SAVE, cur) && unsafe { write(fc + FOLLOW_CAM_AUTO_ROTATION, 0u8) } {
                    AUTOROT_CLEARS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    let wall = WALL_ADDR.load(Ordering::Relaxed);
    if WALL_OFF.load(Ordering::Relaxed) && wall != 0 {
        // SAFETY: non-faulting read of the validated exe bool.
        if let Some(cur) = unsafe { read::<u8>(wall) } {
            // SAFETY: the validated CAM_WALL_RECOVERY bool; 0 is a valid value.
            if clear_step(&WALL_SAVE, cur) && unsafe { write_exe_bool(wall, 0) } {
                WALL_CLEARS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

unsafe extern "C" fn follow_detour(this: *mut c_void) {
    let fc = this as usize;
    // An unwind must never cross this frame (follow_pre has no panicking calls).
    let _ = catch_unwind(AssertUnwindSafe(|| follow_pre(fc)));
    let orig = FOLLOW_TRAMPOLINE.load(Ordering::Acquire);
    if orig != 0 {
        // SAFETY: the MinHook trampoline of the follow-cam update, called with the game's own argument.
        let f: FollowFn = unsafe { std::mem::transmute(orig) };
        // SAFETY: as above.
        unsafe { f(this) };
    }
}

fn exe_base() -> Result<usize, String> {
    // SAFETY: `None` = the exe module (stays loaded for the process lifetime).
    Ok(unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?.0 as usize)
}

/// Prologue check + MinHook create/enable. Returns the hooked address and the trampoline.
fn hook_at(target: usize, prologue: &[u8], detour: *mut c_void) -> Result<usize, String> {
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate().take(prologue.len()) {
        // SAFETY: non-faulting read of one code byte.
        *b = unsafe { read::<u8>(target + i) }.ok_or_else(|| format!("prologue unreadable at 0x{target:x}"))?;
    }
    if bytes[..prologue.len()] != *prologue {
        return Err(format!("prologue mismatch at 0x{target:x}: {:02x?} (want {prologue:02x?})", &bytes[..prologue.len()]));
    }
    // SAFETY: MH_Initialize is idempotent (ALREADY_INITIALIZED once hudhook/input initialised it).
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        s => return Err(format!("MH_Initialize: {s:?}")),
    }
    // SAFETY: target's prologue was checked above; the detour has the target's signature (caller contract).
    let hook = unsafe { MhHook::new(target as *mut c_void, detour) }.map_err(|s| format!("MH_CreateHook 0x{target:x}: {s:?}"))?;
    let tramp = hook.trampoline() as usize;
    std::mem::forget(hook); // lives for the process lifetime; disabling = ACTIVE false
    Ok(tramp)
}

/// Installs the camera-step hook. Must run after the version gate. Returns the hooked address.
pub fn install() -> Result<usize, String> {
    let own = crate::version_gate::own()?;
    let target = exe_base()? + own.camera_step;
    let camera_step_prologue = own.camera_step_prologue;
    // SAFETY: non-faulting read of 10 code bytes.
    let bytes = unsafe { read::<[u8; 10]>(target) }.ok_or_else(|| format!("prologue unreadable at 0x{target:x}"))?;
    if bytes != camera_step_prologue {
        return Err(format!("prologue mismatch at 0x{target:x}: {bytes:02x?} (want {camera_step_prologue:02x?})"));
    }
    let tramp = hook_at(target, camera_step_prologue, detour as StepFn as *mut c_void)?;
    TRAMPOLINE.store(tramp, Ordering::Release);
    // SAFETY: created above; the trampoline is stored before enabling.
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => {}
        s => return Err(format!("MH_EnableHook 0x{target:x}: {s:?}")),
    }
    INSTALLED.store(true, Ordering::SeqCst);
    Ok(target)
}

/// Result of [`install_follow`] for the boot log.
pub struct FollowInstall {
    pub addr: usize,
    /// `Ok((address, current value))` of CAM_WALL_RECOVERY, or why it is left alone.
    pub wall: Result<(usize, u8), String>,
}

/// Installs the follow-cam hook (after [`install`] succeeded) and validates the CAM_WALL_RECOVERY global. Must run
/// after the version gate. A wall-recovery validation failure only disables that part.
pub fn install_follow() -> Result<FollowInstall, String> {
    let own = crate::version_gate::own()?;
    let target = exe_base()? + own.follow_cam;
    let wall = validate_exe_data_bool(own.cam_wall_recovery);
    if let Ok((a, _)) = wall {
        WALL_ADDR.store(a, Ordering::SeqCst);
    }
    let tramp = hook_at(target, own.follow_cam_prologue, follow_detour as FollowFn as *mut c_void)?;
    FOLLOW_TRAMPOLINE.store(tramp, Ordering::Release);
    // SAFETY: created above; the trampoline is stored before enabling.
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => {}
        s => return Err(format!("MH_EnableHook 0x{target:x}: {s:?}")),
    }
    FOLLOW_INSTALLED.store(true, Ordering::SeqCst);
    Ok(FollowInstall { addr: target, wall })
}

/// Hook mode: the task's camera read sees the eye the detour wrote this frame. When the read position is exactly
/// that eye, returns ER's own pre-write position so the camera gate, CAMFRAME and aim keep seeing ER's camera.
pub fn er_view_pos(read_pos: V3) -> Option<V3> {
    if !INSTALLED.load(Ordering::Relaxed) || !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    let (er, eye) = er_pos_and_eye();
    let same = read_pos.x.to_bits() == eye[0].to_bits()
        && read_pos.y.to_bits() == eye[1].to_bits()
        && read_pos.z.to_bits() == eye[2].to_bits();
    same.then_some(V3::new(er[0], er[1], er[2]))
}

/// Start of an episode (task, FP on): publish the switches, clear the per-episode saves/flags.
pub fn begin_episode(sync_chr_cam: bool, wall_off: bool, autorot_off: bool) {
    SYNC_CHR_CAM.store(sync_chr_cam, Ordering::SeqCst);
    WALL_OFF.store(wall_off, Ordering::SeqCst);
    AUTOROT_OFF.store(autorot_off, Ordering::SeqCst);
    CHR_WROTE.store(false, Ordering::SeqCst);
}

/// One restored (or left alone) field of a handback, for the `FP restore` log lines.
pub struct Restored {
    pub field: &'static str,
    pub addr: usize,
    pub old: String,
    pub new: String,
    /// `None` = nothing to write (why in `old`/`new`), `Some(ok)` = written.
    pub ok: Option<bool>,
}

/// Handback (task): stops the detours, waits for one in flight, then restores `ChrCam.pers_cam` row 3, the follow
/// cam's `camera_auto_rotation` and `CAM_WALL_RECOVERY` exactly. `pers_cam_1` is restored by the caller, which has
/// this tick's read of it. Returns what was done, for the log.
pub fn handback() -> Vec<Restored> {
    ACTIVE.store(false, Ordering::SeqCst);
    let t = std::time::Instant::now();
    while IN_DETOUR.load(Ordering::SeqCst) > 0 && t.elapsed().as_micros() < IDLE_WAIT_MAX_US {
        std::hint::spin_loop();
    }
    let mut out = Vec::new();
    // ChrCam.pers_cam row 3
    if CHR_WROTE.swap(false, Ordering::SeqCst) {
        let chr = CHR_CAM_ADDR.load(Ordering::SeqCst);
        // SAFETY: non-faulting re-read of WorldChrMan.chr_cam.
        let fresh = unsafe { read_chr_cam_ptr(WCM_ADDR.load(Ordering::SeqCst)) };
        // SAFETY: non-faulting read.
        let cur = (chr != 0 && fresh == chr).then(|| unsafe { read::<[f32; 4]>(chr + CHR_CAM_ROW3) }).flatten();
        let (er, eye) = (load4(&CHR_ER_POS), load4(&CHR_LAST_EYE));
        let fmt = |v: [f32; 4]| format!("({:.2},{:.2},{:.2})", v[0], v[1], v[2]);
        match cur.and_then(|c| hook_row_restore(c, eye, er, true).map(|v| (c, v))) {
            Some((c, v)) => {
                // SAFETY: the same ChrCam (re-checked); row 3, the exact value ER had before our last write.
                let ok = unsafe { write(chr + CHR_CAM_ROW3, v) };
                out.push(Restored { field: "ChrCam.pers_cam+0x40", addr: chr + CHR_CAM_ROW3, old: fmt(c), new: fmt(v), ok: Some(ok) });
            }
            None => out.push(Restored {
                field: "ChrCam.pers_cam+0x40",
                addr: chr + CHR_CAM_ROW3,
                old: cur.map_or("unreadable/changed object".into(), fmt),
                new: "nothing (ER already rebuilt it, or the chr cam changed)".into(),
                ok: None,
            }),
        }
    }
    // ChrExFollowCam.camera_auto_rotation
    let mut st = ClearedByte::decode(AUTOROT_SAVE.swap(0, Ordering::SeqCst));
    let owner = AUTOROT_OWNER.swap(0, Ordering::SeqCst);
    if st.saved.is_some() {
        // SAFETY: non-faulting re-reads (WorldChrMan.chr_cam -> ex_follow_cam).
        let fc = unsafe { read_follow_cam_ptr(read_chr_cam_ptr(WCM_ADDR.load(Ordering::SeqCst))) };
        let same = owner != 0 && fc == owner;
        // SAFETY: non-faulting read of one byte.
        let cur = if same { unsafe { read::<u8>(owner + FOLLOW_CAM_AUTO_ROTATION) } } else { None };
        let saved = st.saved.unwrap_or(0);
        match cur.and_then(|c| st.restore(c, same).map(|v| (c, v))) {
            Some((c, v)) => {
                // SAFETY: the same follow cam (re-checked); one bool, the value saved before our first clear.
                let ok = unsafe { write(owner + FOLLOW_CAM_AUTO_ROTATION, v) };
                out.push(Restored { field: "ChrExFollowCam.camera_auto_rotation(+0x240)", addr: owner + FOLLOW_CAM_AUTO_ROTATION, old: c.to_string(), new: v.to_string(), ok: Some(ok) });
            }
            None => out.push(Restored {
                field: "ChrExFollowCam.camera_auto_rotation(+0x240)",
                addr: owner + FOLLOW_CAM_AUTO_ROTATION,
                old: cur.map_or(if same { "unreadable".into() } else { "follow cam changed".to_string() }, |c| c.to_string()),
                new: format!("nothing (saved={saved})"),
                ok: None,
            }),
        }
    }
    // CAM_WALL_RECOVERY (exe global)
    let mut st = ClearedByte::decode(WALL_SAVE.swap(0, Ordering::SeqCst));
    let wall = WALL_ADDR.load(Ordering::SeqCst);
    if st.saved.is_some() && wall != 0 {
        let saved = st.saved.unwrap_or(0);
        // SAFETY: non-faulting read of the validated exe bool.
        let cur = unsafe { read::<u8>(wall) };
        match cur.and_then(|c| st.restore(c, true).map(|v| (c, v))) {
            Some((c, v)) => {
                // SAFETY: the validated CAM_WALL_RECOVERY bool; the value saved before our first clear.
                let ok = unsafe { write_exe_bool(wall, v) };
                out.push(Restored { field: "CAM_WALL_RECOVERY(exe .data)", addr: wall, old: c.to_string(), new: v.to_string(), ok: Some(ok) });
            }
            None => out.push(Restored {
                field: "CAM_WALL_RECOVERY(exe .data)",
                addr: wall,
                old: cur.map_or("unreadable".into(), |c| c.to_string()),
                new: format!("nothing (saved={saved})"),
                ok: None,
            }),
        }
    }
    out
}

/// The current values for the periodic FPCAM line: (auto_rotation of the player's follow cam, CAM_WALL_RECOVERY).
pub fn live_bytes() -> (Option<u8>, Option<u8>) {
    let fc = live_follow_cam();
    // SAFETY: non-faulting reads.
    unsafe {
        let a = if fc != 0 { read::<u8>(fc + FOLLOW_CAM_AUTO_ROTATION) } else { None };
        let w = WALL_ADDR.load(Ordering::Relaxed);
        (a, if w != 0 { read::<u8>(w) } else { None })
    }
}

/// The player's `ChrExFollowCam` now (WorldChrMan.chr_cam -> ex_follow_cam), 0 when absent.
pub fn live_follow_cam() -> usize {
    // SAFETY: non-faulting reads of pointer-sized integers.
    unsafe { read_follow_cam_ptr(read_chr_cam_ptr(WCM_ADDR.load(Ordering::Relaxed))) }
}

/// The first value saved this episode (for the one-time write log), `None` before the first detour call.
pub fn saved_bytes() -> (Option<u8>, Option<u8>) {
    (ClearedByte::decode(AUTOROT_SAVE.load(Ordering::Relaxed)).saved, ClearedByte::decode(WALL_SAVE.load(Ordering::Relaxed)).saved)
}

/// The latest ChrCam row-3 write (ER's value, our eye) for the one-time write log.
pub fn chr_er_and_eye() -> ([f32; 4], [f32; 4]) {
    (load4(&CHR_ER_POS), load4(&CHR_LAST_EYE))
}
