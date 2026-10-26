//! C3 (docs/C3_DESIGN.md 2.4) + C4 (docs/C4_UI_DESIGN.md 2): the Scaleform UI-pass probe. OFF by default
//! (`[ui] probe_scaleform`, cold).
//!
//! ER's UI is Scaleform GFx on a D3D12 HAL [V exe]: BeginScene 0x14115e6b0 (vtable slot 59, prologue
//! `40 57 48 83 ec 20 8b 41 58 48 8b f9 a8 02 75 08`) and EndScene 0x14115e810 (slot 60, prologue
//! `41 55 41 56 48 83 ec 48`) on exe 2.7.1.0; 0x14115fda0 / 0x14115ff00 with the same prologues on 2.7.2.0 (port,
//! [V exe file]); per build in `mw2er_core::exe_build::OwnAddrs`. C3 only logged what an injection would need:
//!
//! - MinHook detours on both functions, each installed only if its prologue bytes match (cam_hook.rs pattern);
//!   a mismatch logs `UI probe skip why=prologue fn=.. got=..` and installs nothing.
//! - The detours count calls, note the calling thread and `this` / the HAL flags (`this+0x58`), then call the
//!   original. Once per 5 s BeginScene scans `this[0..0x800]` (non-faulting reads) for a pointer whose first qword is
//!   the vtable of hudhook's `ID3D12GraphicsCommandList` (published by the present callback) and records the offset.
//! - No allocation, lock or log in a detour: the task logs the atomics every 5 s (`UI probe scenes/s=..`).
//!
//! C4 adds (2.7.2.0 only: `OwnAddrs::scaleform_ui`; its sites are checked on the loaded image first):
//!
//! - detours on ER's UiBegin 0x140d80900 / UiEnd 0x140d80a70 (16-byte prologue checks). They call the ORIGINAL FIRST,
//!   then read the HAL, the owner, `S` and the selected target non-faulting into [`UiFacts`]; with the probe on they
//!   fill the probe's atomics, with `[ui] inject_viewmodel` they hand the facts to `ui_inject` (the same install path
//!   serves both; installed when either key is on at boot).
//! - hudhook patch 0002's ExecuteCommandLists callback ([`ecl_hook`]): the probe counts per phase / queue whether the
//!   list UiBegin armed (`[hal+0x1420]` after the original) is submitted first on the HAL queue before UiEnd
//!   (zero tolerance); the injection's splice runs in the same callback.
//! - once per 5 s a read-only `GetDesc` of the selected target (only if its vtable lies in the D3D12 runtime module
//!   of the back buffer hudhook handed us), the `UI probe ...` lines of design 2.2 and the in-DLL verdict
//!   (`mw2er_core::c4::verdict`, published for `ui_inject`).
//! - every hook target is stored ([`HOOKED`]) and disabled by `ui_inject::teardown` (never removed: trampolines kept) (no more `forget()`); [`ACTIVE`]
//!   false turns every detour into a passthrough first, [`INFLIGHT`] counts threads inside one.
//! - No game memory is written. Nothing is drawn into ER's command lists.
//!
//! C4.1 probe P41 (docs/C4_1_DESIGN.md 2; with `[ui] probe_scaleform`, after `exe_build::scaleform_ui41_sites` passed
//! on the loaded image): the same detours also read ER's render-thread id (`[[G+0x6d0]+0x14]`, `byte [G+0x11c]`), the
//! pinned read-only chain to BOTH candidate textures (`mw2er_core::c4::chain41`; ER's getter is never called), the
//! `[owner+0xDD0]` rtData cross-check and the composite's inputs, and count `ahead` (UiBegin) / `present_in_ui`
//! (Present) / the `between` ECL's frame. Atomics only in the detours; the resource's module check runs on the task
//! thread (cached per vtable), so the render thread only ever calls `GetDesc` through `desc_if_vtable`. The task logs
//! the `UI probe41 ...` lines and `c4::verdict41`. The layer sample P2 (`[ui] probe_layer_sample`, cold, off by
//! default; `UI probe41 on sample=1`) lives in `ui_sample.rs`: UiBegin hands it the layer's chain, the ECL callback
//! splices its list when the injection did not, UiEnd cancels an unsubmitted one, and its convention is the verdict's
//! `alpha` row (docs/C4_1_DESIGN.md 2.4).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::hooks::dx12::pre_imgui::{ECL_HOOK, PRESENT_SEQ, PRESENT_TID as HH_PRESENT_TID, PreImguiCtx, Splice};
use hudhook::mh::{MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::c4::{self, Chain41, EclTotals, Output41, Phase, ProbeStats, ProbeStats41, RtData41, TexDesc, Thread41, UiPath};
use mw2er_core::exe_build::ScaleformUi;
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::Interface;

use crate::game::read;
use crate::log::{log_i, log_w};

// BeginScene / EndScene RVAs and prologues: `version_gate::own()` (the accepted exe build's `OwnAddrs`).
const SCAN_BYTES: usize = 0x800;
const SCAN_EVERY_MS: u64 = 5_000;

type SceneFn = unsafe extern "C" fn(*mut c_void) -> u64;

static BEGIN_TRAMP: AtomicUsize = AtomicUsize::new(0);
static END_TRAMP: AtomicUsize = AtomicUsize::new(0);
pub static INSTALLED_BEGIN: AtomicBool = AtomicBool::new(false);
pub static INSTALLED_END: AtomicBool = AtomicBool::new(false);
static BEGINS: AtomicU64 = AtomicU64::new(0);
static ENDS: AtomicU64 = AtomicU64::new(0);
static LAST_THIS: AtomicUsize = AtomicUsize::new(0);
static LAST_FLAGS: AtomicU32 = AtomicU32::new(0);
static LAST_TID: AtomicU32 = AtomicU32::new(0);
/// The present thread (set by the viewmodel's pre-ImGui callback) and hudhook's command-list vtable.
pub static PRESENT_TID: AtomicU32 = AtomicU32::new(0);
pub static CL_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// The offset in `this` of a pointer to a command list (u32::MAX = none found yet) and the last scan time.
static CL_OFF: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_SCAN_MS: AtomicU64 = AtomicU64::new(0);

// ---- C4: the hook table, passthrough switch and in-flight counter (design S1) ----------------------------------------

/// Hook targets (absolute addresses, 0 = not hooked) in install order: BeginScene, EndScene, UiBegin, UiEnd.
pub static HOOKED: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
pub const H_BEGIN_SCENE: usize = 0;
pub const H_END_SCENE: usize = 1;
pub const H_UI_BEGIN: usize = 2;
pub const H_UI_END: usize = 3;
/// false = every detour and the ECL callback are pure passthrough (teardown step 1).
pub static ACTIVE: AtomicBool = AtomicBool::new(true);
/// Threads inside a detour or the ECL callback right now.
pub static INFLIGHT: AtomicU32 = AtomicU32::new(0);
/// `[ui] probe_scaleform` at boot (the C4 counters run only with it).
pub static PROBE_ON: AtomicBool = AtomicBool::new(false);
/// The in-DLL verdict (design 2.3): set once the probe judged READY (`ui_inject` reads it).
pub static PROBE_READY: AtomicBool = AtomicBool::new(false);
/// The verdict was computed at least once (60 s after the first UiBegin).
pub static PROBE_JUDGED: AtomicBool = AtomicBool::new(false);
/// C4.1 (docs/C4_1_DESIGN.md 4): the in-DLL C4.1 verdict for `ui_inject` (judged at 60 s, then every 60 s): ready, the
/// only failing row was `alpha`, and its alpha (0 unknown, 1 transmit, 2 premul).
pub static PROBE41_JUDGED: AtomicBool = AtomicBool::new(false);
pub static PROBE41_READY: AtomicBool = AtomicBool::new(false);
pub static PROBE41_ONLY_ALPHA: AtomicBool = AtomicBool::new(false);
pub static PROBE41_ALPHA: AtomicU8 = AtomicU8::new(0);
/// C4.1 PASS I41: `[ui] inject_ui_thread` with the C4.1 sites passed on the loaded image (boot): UiBegin's detour
/// reads the C4.1 facts ([`Facts41`]) for the injection.
pub static INJ41: AtomicBool = AtomicBool::new(false);

/// The C4.1 probe's reads are on (`[ui] probe_scaleform` + the C4.1 sites): its verdict gates the C4.1 injection.
pub fn probe41_on() -> bool {
    P41_ON.load(Ordering::Relaxed)
}

/// `LayerAlpha` <-> the atomic code (0 = unknown).
pub fn alpha_code(a: Option<c4::LayerAlpha>) -> u8 {
    match a {
        None => 0,
        Some(c4::LayerAlpha::Transmit) => 1,
        Some(c4::LayerAlpha::Premul) => 2,
    }
}

pub fn alpha_of(code: u8) -> Option<c4::LayerAlpha> {
    match code {
        1 => Some(c4::LayerAlpha::Transmit),
        2 => Some(c4::LayerAlpha::Premul),
        _ => None,
    }
}

// ---- C4: UiBegin / UiEnd -----------------------------------------------------------------------------------------------

type UiFn = unsafe extern "C" fn(*mut c_void) -> u64;

static UI_BEGIN_TRAMP: AtomicUsize = AtomicUsize::new(0);
static UI_END_TRAMP: AtomicUsize = AtomicUsize::new(0);
pub static INSTALLED_UI_BEGIN: AtomicBool = AtomicBool::new(false);
pub static INSTALLED_UI_END: AtomicBool = AtomicBool::new(false);
static ECL_OK: AtomicBool = AtomicBool::new(false);
static SKIPS: AtomicU32 = AtomicU32::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static SF: OnceLock<ScaleformUi> = OnceLock::new();

/// Published by the present callback (hudhook's device / queue, the back buffer's vtable and desc).
pub static PRESENT_DEVICE: AtomicUsize = AtomicUsize::new(0);
pub static PRESENT_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static BB_VTABLE: AtomicUsize = AtomicUsize::new(0);
pub static BB_SIZE: AtomicU64 = AtomicU64::new(0);

/// Everything UiBegin's detour reads after the original returned (non-faulting reads; 0 = unreadable / none).
#[derive(Clone, Copy, Debug, Default)]
pub struct UiFacts {
    pub hal: usize,
    pub movies: u64,
    pub vt: usize,
    pub vt_ok: bool,
    pub flags: u32,
    pub rtid: u32,
    pub tid: u32,
    pub present_tid: u32,
    /// `[hal+0x1420]` AFTER the original: the list holding ER's `0xC0 -> 4` barrier.
    pub list: usize,
    pub queue: usize,
    pub device: usize,
    pub b13c: u8,
    pub b13e: u8,
    pub scene: usize,
    pub layer: usize,
    pub path: UiPath,
    /// The selected texture object, the getter's address and its first 16 bytes, the chained resource.
    pub sel: usize,
    pub getter_addr: usize,
    pub getter: [u8; 16],
    pub getter_read: bool,
    pub res: usize,
    /// The other candidate's resource (logged by the probe).
    pub other_res: usize,
    pub seq: u64,
}


/// Resolves a texture object's resource through its (plain) getter; returns (getter addr, bytes, read ok, res).
fn resolve(sel: usize, l: &mw2er_core::exe_build::UiLayout) -> (usize, [u8; 16], bool, usize) {
    if sel == 0 {
        return (0, [0; 16], false, 0);
    }
    let this = sel + l.obj;
    // SAFETY: non-faulting reads of the object's vtable, its slot 1 and the slot's first 16 code bytes.
    let vtbl = unsafe { read::<usize>(this) }.unwrap_or(0);
    let addr = if vtbl == 0 { 0 } else { unsafe { read::<usize>(vtbl + 8) }.unwrap_or(0) };
    let bytes = if addr == 0 { None } else { unsafe { read::<[u8; 16]>(addr) } };
    let res = bytes
        .and_then(|b| c4::parse_getter(&b))
        // SAFETY: non-faulting qword reads of the chain (the getter is plain: no call).
        .and_then(|g| c4::chain_res(this, g, l.wrap, l.res, &|a| unsafe { read::<usize>(a) }))
        .unwrap_or(0);
    (addr, bytes.unwrap_or([0; 16]), bytes.is_some(), res)
}

fn read_facts(owner: usize, sf: &ScaleformUi, base: usize) -> UiFacts {
    let l = &sf.layout;
    // SAFETY (whole fn): non-faulting reads of ER's UI owner, its HAL and the render global; nothing is written.
    let hal = unsafe { read::<usize>(owner + l.owner_hal) }.unwrap_or(0);
    let movies = unsafe { read::<u64>(owner + l.owner_movies) }.unwrap_or(0);
    let mut f = UiFacts { hal, movies, seq: PRESENT_SEQ.load(Ordering::SeqCst), ..UiFacts::default() };
    // SAFETY: GetCurrentThreadId has no preconditions.
    f.tid = unsafe { GetCurrentThreadId() };
    f.present_tid = HH_PRESENT_TID.load(Ordering::Relaxed);
    if hal != 0 {
        f.vt = unsafe { read::<usize>(hal) }.unwrap_or(0);
        f.vt_ok = f.vt == base + sf.fs_hal_vtable;
        f.flags = unsafe { read::<u32>(hal + l.hal_flags) }.unwrap_or(0);
        f.rtid = unsafe { read::<u32>(hal + l.hal_rtid) }.unwrap_or(0);
        f.list = unsafe { read::<usize>(hal + l.hal_list) }.unwrap_or(0);
        f.queue = unsafe { read::<usize>(hal + l.hal_queue) }.unwrap_or(0);
        f.device = unsafe { read::<usize>(hal + l.hal_device) }.unwrap_or(0);
    }
    let g = unsafe { read::<usize>(base + sf.g_render) }.unwrap_or(0);
    let s = if g == 0 { 0 } else { unsafe { read::<usize>(g + l.s_off) }.unwrap_or(0) };
    if s != 0 {
        f.b13c = unsafe { read::<u8>(s + l.hdr_a) }.unwrap_or(0);
        f.b13e = unsafe { read::<u8>(s + l.hdr_b) }.unwrap_or(0);
        f.layer = unsafe { read::<usize>(s + l.layer) }.unwrap_or(0);
        f.scene = unsafe { read::<usize>(s + l.scene) }.unwrap_or(0);
    }
    f.path = c4::select_path(f.b13c, f.b13e, f.layer);
    f.sel = if f.path == UiPath::Layer { f.layer } else { f.scene };
    (f.getter_addr, f.getter, f.getter_read, f.res) = resolve(f.sel, l);
    let other = if f.path == UiPath::Layer { f.scene } else { f.layer };
    f.other_res = resolve(other, l).3;
    f
}

// ---- C4 probe counters (the task logs them every 5 s) -------------------------------------------------------------------

static P_BEGINS: AtomicU64 = AtomicU64::new(0);
static P_ENDS: AtomicU64 = AtomicU64::new(0);
static P_NOMOVIES: AtomicU64 = AtomicU64::new(0);
static P_TID_OK: AtomicU64 = AtomicU64::new(0);
static P_ARMED: AtomicU64 = AtomicU64::new(0);
static P_VT_BAD: AtomicU32 = AtomicU32::new(0);
static P_DEV_BAD: AtomicU32 = AtomicU32::new(0);
static P_THREAD_BAD: AtomicU32 = AtomicU32::new(0);
/// The last facts as plain atomics (hal line / mode line / getter line).
static P_VT: AtomicUsize = AtomicUsize::new(0);
static P_VT_OK: AtomicBool = AtomicBool::new(false);
static P_DEV: AtomicUsize = AtomicUsize::new(0);
static P_QUEUE: AtomicUsize = AtomicUsize::new(0);
static P_RTID: AtomicU32 = AtomicU32::new(0);
static P_B13C: AtomicU8 = AtomicU8::new(0);
static P_B13E: AtomicU8 = AtomicU8::new(0);
static P_LAYER: AtomicUsize = AtomicUsize::new(0);
static P_GETTER_ADDR: AtomicUsize = AtomicUsize::new(0);
static P_GETTER: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static P_RES: AtomicUsize = AtomicUsize::new(0);
/// UiBegins per Present: the count of the current seq, and the window's min / max.
static P_SEQ_LAST: AtomicU64 = AtomicU64::new(u64::MAX);
static P_PER_COUNT: AtomicU32 = AtomicU32::new(0);
static P_PER_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
static P_PER_MAX: AtomicU32 = AtomicU32::new(0);
/// The `GetDesc` sample: the task sets DUE every 5 s, the detour fills both candidates' descs once.
static P_DESC_DUE: AtomicBool = AtomicBool::new(false);
static P_DESC: [[AtomicU64; 4]; 2] = [const { [const { AtomicU64::new(0) }; 4] }; 2];
static P_DESC_RES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P_DESC_GEN: AtomicU64 = AtomicU64::new(0);
/// The frame's UI phase for the ECL counting: the seq of the last UiBegin / UiEnd, the armed list and its flags.
static F_BEGIN_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static F_END_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static F_ARMED: AtomicUsize = AtomicUsize::new(0);
static F_ARMED_SEEN: AtomicBool = AtomicBool::new(false);
static F_HIT: AtomicBool = AtomicBool::new(false);
static F_FIRST: AtomicBool = AtomicBool::new(false);
static F_OTHER: AtomicBool = AtomicBool::new(false);
static F_HAL_QUEUE: AtomicUsize = AtomicUsize::new(0);
static F_RTID: AtomicU32 = AtomicU32::new(0);
/// `EclTotals` field by field.
static E: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];
const E_TOTAL: usize = 0;
const E_HALQ: usize = 1;
const E_OTHERQ: usize = 2;
const E_PRE: usize = 3;
const E_UI: usize = 4;
const E_POST: usize = 5;
const E_ARMED_HIT: usize = 7;
const E_ARMED_FIRST: usize = 8;
const E_IDX_MAX: usize = 9;
const E_N_MAX: usize = 10;
const E_TID_OK: usize = 11;
const E_BETWEEN: usize = 12;
const E_LATE: usize = 13;

// ---- C4.1 probe P41 counters (detours store, the task logs) ------------------------------------------------------------

/// The C4.1 sites passed and the probe is on (boot).
static P41_ON: AtomicBool = AtomicBool::new(false);
/// UiBegins with a HAL (the `rt_ok` denominator and the `between` frame index), and those on ER's render thread.
static P41_HAL_BEGINS: AtomicU64 = AtomicU64::new(0);
static P41_RT_OK: AtomicU64 = AtomicU64::new(0);
/// UiBegins that read G, and those with `byte [G+0x11c] == 0` (single-threaded mode).
static P41_MT_SEEN: AtomicU64 = AtomicU64::new(0);
static P41_MT_BAD: AtomicU64 = AtomicU64::new(0);
static P41_RT_TID: AtomicU32 = AtomicU32::new(0);
static P41_HAL_TID: AtomicU32 = AtomicU32::new(0);
static P41_BEGIN_TID: AtomicU32 = AtomicU32::new(0);
static P41_RTID_CHANGES: AtomicU32 = AtomicU32::new(0);
static P41_AHEAD: AtomicU64 = AtomicU64::new(0);
static P41_PRESENT_IN_UI: AtomicU64 = AtomicU64::new(0);
/// Per candidate (0 = layer `[S+0x100]`, 1 = scene `[S+0xF0]`): the object, the chain (pin bits, count, res, ER's
/// RTV), the resource's vtable (the task checks its module: `vt | ok` in P41_VT_OK) and the res changes.
static P41_SEL: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_BITS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static P41_COUNT: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static P41_RES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_RTV: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_RES_VT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_VT_OK: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_RES_CHANGES: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
/// `[owner+0xDD0]`, its rtData, rtData's format / RTV, and the per-frame cross-check (layer selected, HAL up).
static P41_RT: AtomicUsize = AtomicUsize::new(0);
static P41_RTD: AtomicUsize = AtomicUsize::new(0);
static P41_RTD_FMT: AtomicU32 = AtomicU32::new(0);
static P41_RTD_RTV: AtomicUsize = AtomicUsize::new(0);
static P41_RTD_N: AtomicU64 = AtomicU64::new(0);
static P41_RTD_OK: AtomicU64 = AtomicU64::new(0);
/// The composite's inputs: `b13c | b13e << 8 | eb9 << 16 | shaders << 24`, the render scale (f32 bits x | y << 32).
static P41_OUT: AtomicU32 = AtomicU32::new(0);
static P41_SCALE: AtomicU64 = AtomicU64::new(0);
static P41_OUT_SEEN: AtomicBool = AtomicBool::new(false);
/// The 5 s GetDesc sample of both candidates (only through a vtable the task found in the D3D12 module).
static P41_DESC_DUE: AtomicBool = AtomicBool::new(false);
static P41_DESC: [[AtomicU64; 4]; 2] = [const { [const { AtomicU64::new(0) }; 4] }; 2];
static P41_DESC_RES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static P41_DESC_GEN: AtomicU64 = AtomicU64::new(0);
/// The last sampled layer format (0 = none yet), for the per-frame rtData cross-check.
static P41_LAYER_FMT: AtomicU32 = AtomicU32::new(0);
/// The first 4 `between_other` ECLs: frame index + 1 (0 = not written yet) and the first list of the call.
static P41_BETWEEN_N: AtomicU32 = AtomicU32::new(0);
static P41_BETWEEN_FRAME: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static P41_BETWEEN_LIST: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

/// A non-faulting read of 1 / 4 / 8 bytes for `c4::chain41` / `c4::rtdata41`.
fn rd41(a: usize, len: usize) -> Option<u64> {
    // SAFETY: non-faulting reads; nothing is written.
    unsafe {
        match len {
            1 => read::<u8>(a).map(u64::from),
            4 => read::<u32>(a).map(u64::from),
            _ => read::<u64>(a),
        }
    }
}

/// C4.1 PASS I41: what the injection's gate reads in UiBegin's detour besides [`UiFacts`] (non-faulting reads only;
/// ER's getter is never called).
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts41 {
    /// `byte [G+0x11c] != 0` (ER's multithreaded mode) and `[[G+0x6d0]+0x14]` (ER's render thread).
    pub mt: bool,
    pub rt_tid: u32,
    /// The layer candidate `[S+0x100]`'s pinned chain and the `[owner+0xDD0]` rtData cross-check.
    pub chain: Chain41,
    pub rtd: RtData41,
    /// The composite's inputs.
    pub out: Output41,
}

/// The C4.1 reads of the injection (render thread, after the original; the same reads as `probe41_begin`, for the
/// layer only).
fn read41(owner: usize, f: &UiFacts, sf: &ScaleformUi, base: usize) -> Facts41 {
    let u = &sf.ui41;
    let rd: c4::Rd = &rd41;
    let mut x = Facts41::default();
    // SAFETY (reads below): non-faulting reads of ER's render global, its render-thread object and the composite's
    // inputs; nothing is written.
    let g = unsafe { read::<usize>(base + sf.g_render) }.unwrap_or(0);
    if g != 0 {
        x.mt = unsafe { read::<u8>(g + u.g_mt) }.unwrap_or(0) != 0;
        let rto = unsafe { read::<usize>(g + u.g_rt) }.unwrap_or(0);
        if rto != 0 {
            x.rt_tid = unsafe { read::<u32>(rto + u.rt_tid) }.unwrap_or(0);
        }
        let sh = unsafe { read::<usize>(g + u.g_shaders) }.unwrap_or(0);
        x.out = Output41 {
            b13c: f.b13c,
            b13e: f.b13e,
            eb9: unsafe { read::<u8>(g + u.g_eb9) }.unwrap_or(0),
            scale_x: unsafe { read::<f32>(g + u.g_scale) }.unwrap_or(0.0),
            scale_y: unsafe { read::<f32>(g + u.g_scale + 4) }.unwrap_or(0.0),
            shaders: sh != 0 && unsafe { read::<u8>(sh + u.shaders_ready) }.unwrap_or(0) != 0,
        };
    }
    x.chain = c4::chain41(f.layer, base, u, &sf.layout, rd);
    x.rtd = c4::rtdata41(owner, u, rd);
    x
}

/// UiBegin's detour, after the original and `probe_begin` (render thread; atomics and non-faulting reads only, plus
/// one `GetDesc` per candidate every 5 s through a vtable the task already checked).
fn probe41_begin(owner: usize, f: &UiFacts, sf: &ScaleformUi, base: usize) {
    let u = &sf.ui41;
    let rd: c4::Rd = &rd41;
    // SAFETY (reads below): non-faulting reads of ER's render global, its render-thread object and the composite's
    // inputs; nothing is written.
    let g = unsafe { read::<usize>(base + sf.g_render) }.unwrap_or(0);
    let mut rt_tid = 0;
    let mut mt = false;
    if g != 0 {
        P41_MT_SEEN.fetch_add(1, Ordering::Relaxed);
        mt = unsafe { read::<u8>(g + u.g_mt) }.unwrap_or(0) != 0;
        if !mt {
            P41_MT_BAD.fetch_add(1, Ordering::Relaxed);
        }
        let rto = unsafe { read::<usize>(g + u.g_rt) }.unwrap_or(0);
        if rto != 0 {
            rt_tid = unsafe { read::<u32>(rto + u.rt_tid) }.unwrap_or(0);
        }
        let eb9 = unsafe { read::<u8>(g + u.g_eb9) }.unwrap_or(0);
        let sx = unsafe { read::<f32>(g + u.g_scale) }.unwrap_or(0.0);
        let sy = unsafe { read::<f32>(g + u.g_scale + 4) }.unwrap_or(0.0);
        let sh = unsafe { read::<usize>(g + u.g_shaders) }.unwrap_or(0);
        let ready = sh != 0 && unsafe { read::<u8>(sh + u.shaders_ready) }.unwrap_or(0) != 0;
        P41_OUT.store(f.b13c as u32 | (f.b13e as u32) << 8 | (eb9 as u32) << 16 | (ready as u32) << 24, Ordering::Relaxed);
        P41_SCALE.store(sx.to_bits() as u64 | (sy.to_bits() as u64) << 32, Ordering::Relaxed);
        P41_OUT_SEEN.store(true, Ordering::Relaxed);
    }
    P41_BEGIN_TID.store(f.tid, Ordering::Relaxed);
    P41_HAL_TID.store(f.rtid, Ordering::Relaxed);
    let prev = P41_RT_TID.swap(rt_tid, Ordering::Relaxed);
    if prev != 0 && rt_tid != 0 && prev != rt_tid {
        P41_RTID_CHANGES.fetch_add(1, Ordering::Relaxed);
    }
    let rt_ok = c4::rt_ok41(f.tid, f.rtid, rt_tid);
    if f.hal != 0 {
        P41_HAL_BEGINS.fetch_add(1, Ordering::Relaxed);
        if rt_ok {
            P41_RT_OK.fetch_add(1, Ordering::Relaxed);
        }
    }
    let rtd = c4::rtdata41(owner, u, rd);
    P41_RT.store(rtd.rt, Ordering::Relaxed);
    P41_RTD.store(rtd.data, Ordering::Relaxed);
    P41_RTD_FMT.store(rtd.fmt, Ordering::Relaxed);
    P41_RTD_RTV.store(rtd.rtv, Ordering::Relaxed);
    let mut chains = [Chain41::default(); 2];
    for (i, sel) in [(0, f.layer), (1, f.scene)] {
        let c = c4::chain41(sel, base, u, &sf.layout, rd);
        P41_SEL[i].store(sel, Ordering::Relaxed);
        P41_BITS[i].store(c.bits() as u32, Ordering::Relaxed);
        P41_COUNT[i].store(c.count, Ordering::Relaxed);
        P41_RTV[i].store(c.rtv, Ordering::Relaxed);
        let prev = P41_RES[i].swap(c.res, Ordering::Relaxed);
        if prev != 0 && prev != c.res {
            P41_RES_CHANGES[i].fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: non-faulting read of the resource's vtable pointer (the task checks its module).
        let vt = if c.res != 0 { unsafe { read::<usize>(c.res) }.unwrap_or(0) } else { 0 };
        P41_RES_VT[i].store(vt, Ordering::Relaxed);
        chains[i] = c;
    }
    // the cross-check: [owner+0xDD0] was built from the selection, so it describes the layer when UiBegin uses it
    if f.hal != 0 && f.path == UiPath::Layer && chains[0].ok() {
        P41_RTD_N.fetch_add(1, Ordering::Relaxed);
        if c4::rtdata_match(&chains[0], &rtd, P41_LAYER_FMT.load(Ordering::Relaxed)) {
            P41_RTD_OK.fetch_add(1, Ordering::Relaxed);
        }
    }
    if P41_DESC_DUE.swap(false, Ordering::Relaxed) {
        for (i, c) in chains.iter().enumerate() {
            let vt = P41_RES_VT[i].load(Ordering::Relaxed);
            let d = if c.res != 0 && vt != 0 && P41_VT_OK[i].load(Ordering::Relaxed) == vt | 1 { desc_if_vtable(c.res, vt) } else { None };
            P41_DESC_RES[i].store(if d.is_some() { c.res } else { 0 }, Ordering::Relaxed);
            let v = d.as_ref().map_or([0; 4], pack_desc);
            for (k, x) in v.into_iter().enumerate() {
                P41_DESC[i][k].store(x, Ordering::Relaxed);
            }
            if let (0, Some(d)) = (i, d) {
                P41_LAYER_FMT.store(d.format, Ordering::Relaxed);
            }
        }
        P41_DESC_GEN.fetch_add(1, Ordering::Release);
    }
    // P2: the layer sample (a due frame only; it gates itself: c4::sample_gate41)
    let vt = P41_RES_VT[0].load(Ordering::Relaxed);
    let res_mod = vt != 0 && P41_VT_OK[0].load(Ordering::Relaxed) == vt | 1;
    crate::ui_sample::on_ui_begin(f, &chains[0], vt, res_mod, mt, rt_ok);
}

pub(crate) fn pack_desc(d: &TexDesc) -> [u64; 4] {
    [d.width, (d.height as u64) | ((d.format as u64) << 32), (d.dimension as u64) | ((d.mips as u64) << 32) | ((d.array as u64) << 48), (d.samples as u64) | ((d.flags as u64) << 32)]
}

pub(crate) fn unpack_desc(v: [u64; 4]) -> TexDesc {
    TexDesc {
        width: v[0],
        height: v[1] as u32,
        format: (v[1] >> 32) as u32,
        dimension: v[2] as u32,
        mips: (v[2] >> 32) as u16,
        array: (v[2] >> 48) as u16,
        samples: v[3] as u32,
        flags: (v[3] >> 32) as u32,
    }
}

/// The module (HMODULE) holding an address, without touching its reference count (0 = none).
pub fn module_of(addr: usize) -> usize {
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW};
    if addr == 0 {
        return 0;
    }
    let mut h = windows::Win32::Foundation::HMODULE::default();
    // SAFETY: FROM_ADDRESS + UNCHANGED_REFCOUNT: a lookup only; the address is passed as the "name" pointer.
    let ok = unsafe {
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, windows::core::PCWSTR(addr as *const u16), &mut h)
    };
    if ok.is_ok() { h.0 as usize } else { 0 }
}

/// `res->GetDesc()` on ER's resource, only when its vtable lies in the module of `own_vtable` (a D3D12 runtime object
/// of ours / hudhook's): a read-only COM call through a borrowed (non-owning) pointer.
pub fn desc_of(res: usize, own_vtable: usize) -> Option<TexDesc> {
    if res == 0 || own_vtable == 0 {
        return None;
    }
    // SAFETY: non-faulting read of the object's vtable pointer.
    let vt = unsafe { read::<usize>(res) }?;
    let m = module_of(vt);
    if m == 0 || m != module_of(own_vtable) {
        return None;
    }
    desc_raw(res)
}

/// Review C4: `res->GetDesc()` when the object's vtable still equals `vt`, a vtable the caller already found in the
/// D3D12 runtime's module (no module lookup here: no loader lock on the render thread). Read-only, borrowed.
pub fn desc_if_vtable(res: usize, vt: usize) -> Option<TexDesc> {
    if res == 0 || vt == 0 {
        return None;
    }
    // SAFETY: non-faulting read of the object's vtable pointer.
    if unsafe { read::<usize>(res) }? != vt {
        return None;
    }
    desc_raw(res)
}

fn desc_raw(res: usize) -> Option<TexDesc> {
    let raw = res as *mut c_void;
    // SAFETY: callers checked the vtable is the D3D12 runtime's; borrowed: no AddRef / Release.
    let r = unsafe { ID3D12Resource::from_raw_borrowed(&raw) }?;
    // SAFETY: GetDesc is a pure query.
    let d = unsafe { r.GetDesc() };
    Some(TexDesc {
        dimension: d.Dimension.0 as u32,
        width: d.Width,
        height: d.Height,
        array: d.DepthOrArraySize,
        mips: d.MipLevels,
        format: d.Format.0 as u32,
        samples: d.SampleDesc.Count,
        flags: d.Flags.0 as u32,
    })
}

fn probe_begin(f: &UiFacts) {
    P_BEGINS.fetch_add(1, Ordering::Relaxed);
    if f.movies == 0 {
        P_NOMOVIES.fetch_add(1, Ordering::Relaxed);
    }
    if f.tid == f.rtid && f.tid == f.present_tid {
        P_TID_OK.fetch_add(1, Ordering::Relaxed);
    }
    // UiBegins per Present (a skipped seq = a Present without a UiBegin)
    let last = P_SEQ_LAST.swap(f.seq, Ordering::Relaxed);
    // C4.1 `ahead`: this UiBegin read the previous UiBegin's seq (J2)
    if c4::ahead41(last, f.seq) {
        P41_AHEAD.fetch_add(1, Ordering::Relaxed);
    }
    if last != f.seq {
        if last != u64::MAX {
            let n = P_PER_COUNT.load(Ordering::Relaxed);
            P_PER_MIN.fetch_min(if f.seq > last + 1 { 0 } else { n }, Ordering::Relaxed);
            P_PER_MAX.fetch_max(n, Ordering::Relaxed);
        }
        P_PER_COUNT.store(1, Ordering::Relaxed);
    } else {
        P_PER_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if f.hal == 0 {
        return;
    }
    if !f.vt_ok {
        P_VT_BAD.fetch_add(1, Ordering::Relaxed);
    }
    let pd = PRESENT_DEVICE.load(Ordering::Relaxed);
    if pd != 0 && f.device != pd {
        P_DEV_BAD.fetch_add(1, Ordering::Relaxed);
    }
    if f.present_tid != 0 && f.tid != f.present_tid {
        P_THREAD_BAD.fetch_add(1, Ordering::Relaxed);
    }
    P_VT.store(f.vt, Ordering::Relaxed);
    P_VT_OK.store(f.vt_ok, Ordering::Relaxed);
    P_DEV.store(f.device, Ordering::Relaxed);
    P_QUEUE.store(f.queue, Ordering::Relaxed);
    P_RTID.store(f.rtid, Ordering::Relaxed);
    P_B13C.store(f.b13c, Ordering::Relaxed);
    P_B13E.store(f.b13e, Ordering::Relaxed);
    P_LAYER.store(f.layer, Ordering::Relaxed);
    if f.getter_read {
        P_GETTER_ADDR.store(f.getter_addr, Ordering::Relaxed);
        P_GETTER[0].store(u64::from_le_bytes(f.getter[..8].try_into().unwrap_or([0; 8])), Ordering::Relaxed);
        P_GETTER[1].store(u64::from_le_bytes(f.getter[8..].try_into().unwrap_or([0; 8])), Ordering::Relaxed);
    }
    P_RES.store(f.res, Ordering::Relaxed);
    // the frame's UI phase for the ECL counting (the armed list = [hal+0x1420] after the original)
    F_ARMED.store(if f.movies > 0 { f.list } else { 0 }, Ordering::SeqCst);
    F_ARMED_SEEN.store(false, Ordering::SeqCst);
    F_HIT.store(false, Ordering::SeqCst);
    F_FIRST.store(false, Ordering::SeqCst);
    F_OTHER.store(false, Ordering::SeqCst);
    F_HAL_QUEUE.store(f.queue, Ordering::SeqCst);
    F_RTID.store(f.rtid, Ordering::SeqCst);
    F_BEGIN_SEQ.store(f.seq, Ordering::SeqCst);
    // once per 5 s: GetDesc of both candidates (the module check first)
    if P_DESC_DUE.swap(false, Ordering::Relaxed) {
        let own = BB_VTABLE.load(Ordering::Relaxed);
        for (i, r) in [f.res, f.other_res].into_iter().enumerate() {
            let d = desc_of(r, own);
            P_DESC_RES[i].store(if d.is_some() { r } else { 0 }, Ordering::Relaxed);
            let v = d.as_ref().map_or([0; 4], pack_desc);
            for (k, x) in v.into_iter().enumerate() {
                P_DESC[i][k].store(x, Ordering::Relaxed);
            }
        }
        P_DESC_GEN.fetch_add(1, Ordering::Release);
    }
}

/// UiEnd (after the original, whose EndFrame may submit): the frame's armed / hit / first are counted here together,
/// so a sample taken in the middle of a UI frame never sees a hit without its armed frame.
fn probe_end(seq: u64) {
    P_ENDS.fetch_add(1, Ordering::Relaxed);
    if F_BEGIN_SEQ.load(Ordering::SeqCst) == seq && F_ARMED.load(Ordering::SeqCst) != 0 {
        P_ARMED.fetch_add(1, Ordering::Relaxed);
        if F_HIT.load(Ordering::SeqCst) {
            E[E_ARMED_HIT].fetch_add(1, Ordering::Relaxed);
        }
        if F_FIRST.load(Ordering::SeqCst) {
            E[E_ARMED_FIRST].fetch_add(1, Ordering::Relaxed);
        }
    }
    F_END_SEQ.store(seq, Ordering::SeqCst);
}

/// The ECL counting (any thread; atomics only).
fn probe_ecl(queue: usize, lists: &[usize], seq: u64) {
    E[E_TOTAL].fetch_add(1, Ordering::Relaxed);
    let halq = queue != 0 && queue == F_HAL_QUEUE.load(Ordering::SeqCst);
    E[if halq { E_HALQ } else { E_OTHERQ }].fetch_add(1, Ordering::Relaxed);
    let begin = F_BEGIN_SEQ.load(Ordering::SeqCst);
    let end = F_END_SEQ.load(Ordering::SeqCst);
    let phase = if begin == seq && end != seq {
        Phase::Ui
    } else if begin == seq {
        Phase::Post
    } else {
        Phase::Pre
    };
    E[match phase {
        Phase::Pre => E_PRE,
        Phase::Ui => E_UI,
        Phase::Post => E_POST,
    }]
    .fetch_add(1, Ordering::Relaxed);
    let armed = F_ARMED.load(Ordering::SeqCst);
    let idx = c4::find_armed(lists, armed);
    let seen = F_ARMED_SEEN.load(Ordering::SeqCst);
    // the armed list of an older Present is not this frame's (late is counted only within the same Present)
    let has = idx.is_some() && begin == seq;
    let c = c4::count_ecl(phase, halq, has, seen, F_OTHER.load(Ordering::SeqCst));
    if c.armed_hit {
        F_ARMED_SEEN.store(true, Ordering::SeqCst);
        F_HIT.store(true, Ordering::SeqCst);
        E[E_IDX_MAX].fetch_max(idx.unwrap_or(0) as u64, Ordering::Relaxed);
        E[E_N_MAX].fetch_max(lists.len() as u64, Ordering::Relaxed);
        // SAFETY: GetCurrentThreadId has no preconditions.
        if unsafe { GetCurrentThreadId() } == F_RTID.load(Ordering::Relaxed) {
            E[E_TID_OK].fetch_add(1, Ordering::Relaxed);
        }
    }
    if c.armed_first {
        F_FIRST.store(true, Ordering::SeqCst);
    }
    if c.between_other {
        E[E_BETWEEN].fetch_add(1, Ordering::Relaxed);
        // C4.1 `between`: the first 4 events' frame index (UiBegins with a HAL so far) and list
        if P41_ON.load(Ordering::Relaxed) {
            let k = P41_BETWEEN_N.fetch_add(1, Ordering::SeqCst) as usize;
            if k < P41_BETWEEN_FRAME.len() {
                P41_BETWEEN_LIST[k].store(lists.first().copied().unwrap_or(0), Ordering::SeqCst);
                P41_BETWEEN_FRAME[k].store(P41_HAL_BEGINS.load(Ordering::Relaxed) + 1, Ordering::SeqCst);
            }
        }
    }
    if c.late {
        F_ARMED_SEEN.store(true, Ordering::SeqCst);
        E[E_LATE].fetch_add(1, Ordering::Relaxed);
    }
    if halq && phase == Phase::Ui && !c.armed_hit {
        F_OTHER.store(true, Ordering::SeqCst);
    }
}

unsafe extern "C" fn ui_begin_detour(owner: *mut c_void) -> u64 {
    INFLIGHT.fetch_add(1, Ordering::SeqCst);
    let f = UI_BEGIN_TRAMP.load(Ordering::Acquire);
    // SAFETY: the MinHook trampoline of UiBegin, called FIRST with the game's own argument (design 3.3 step 1).
    let r = unsafe { std::mem::transmute::<usize, UiFn>(f)(owner) };
    if ACTIVE.load(Ordering::Relaxed) && catch_unwind(AssertUnwindSafe(|| after_ui_begin(owner as usize))).is_err() {
        crate::ui_inject::trip(c4::Miss::Panic);
    }
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    r
}

unsafe extern "C" fn ui_end_detour(owner: *mut c_void) -> u64 {
    INFLIGHT.fetch_add(1, Ordering::SeqCst);
    // C4.1: note that UiEnd's original is running (its EndFrame submits ER's last UI list; atomics only)
    if ACTIVE.load(Ordering::Relaxed) && catch_unwind(crate::ui_inject::before_ui_end).is_err() {
        crate::ui_inject::trip(c4::Miss::Panic);
    }
    let f = UI_END_TRAMP.load(Ordering::Acquire);
    // SAFETY: the MinHook trampoline of UiEnd (the original first: its EndFrame may still submit the armed list).
    let r = unsafe { std::mem::transmute::<usize, UiFn>(f)(owner) };
    if ACTIVE.load(Ordering::Relaxed) && catch_unwind(AssertUnwindSafe(after_ui_end)).is_err() {
        crate::ui_inject::trip(c4::Miss::Panic);
    }
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    r
}

fn after_ui_begin(owner: usize) {
    let Some(sf) = SF.get() else { return };
    let base = BASE.load(Ordering::Relaxed);
    let f = read_facts(owner, sf, base);
    if PROBE_ON.load(Ordering::Relaxed) {
        probe_begin(&f);
        if P41_ON.load(Ordering::Relaxed) {
            probe41_begin(owner, &f, sf, base);
        }
    }
    if INJ41.load(Ordering::Relaxed) {
        let x = read41(owner, &f, sf, base);
        crate::ui_inject::on_ui_begin41(&f, &x);
    } else {
        crate::ui_inject::on_ui_begin(&f);
    }
    // review C6 depth: the depth probe's GetDesc sample, here on ER's render thread (a due frame only); C6 depth 5: the
    // watched resource and the copy / sample record
    crate::er_depth::on_ui_begin(base, sf, &f);
}

fn after_ui_end() {
    let seq = PRESENT_SEQ.load(Ordering::SeqCst);
    if PROBE_ON.load(Ordering::Relaxed) {
        probe_end(seq);
        if P41_ON.load(Ordering::Relaxed) {
            crate::ui_sample::on_ui_end();
        }
    }
    crate::ui_inject::on_ui_end(seq);
    // C6 depth 5: an armed copy / sample this UI frame never spliced (its list never ran)
    crate::er_depth_state::on_ui_end(seq);
}

/// hudhook patch 0002's ExecuteCommandLists callback (every ECL of the process; atomics only, no alloc/log/lock).
pub fn ecl_hook(queue: usize, lists: &[usize]) -> Option<Splice> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    INFLIGHT.fetch_add(1, Ordering::SeqCst);
    let r = catch_unwind(AssertUnwindSafe(|| {
        let seq = PRESENT_SEQ.load(Ordering::SeqCst);
        if PROBE_ON.load(Ordering::Relaxed) {
            probe_ecl(queue, lists, seq);
        }
        // C6 depth 5: the barrier fold of this call in queue order, BEFORE any splice (the state our list would see)
        crate::er_depth_state::fold_ecl(queue, lists, seq);
        // C4.1: an ECL on the HAL queue from ER's render thread while UiEnd's original runs (its EndFrame submit)
        crate::ui_inject::note_ecl(queue);
        // the injection's splice first, then the C6 ER-depth copy / sample, then the P2 sample (one splice per call)
        match crate::ui_inject::on_ecl(queue, lists, seq) {
            Some(s) => Some(s),
            None => match crate::er_depth_state::on_ecl(queue, lists, seq) {
                Some(s) => Some(s),
                None if P41_ON.load(Ordering::Relaxed) => crate::ui_sample::on_ecl(queue, lists, seq),
                None => None,
            },
        }
    }));
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    r.unwrap_or_else(|_| {
        crate::ui_inject::trip(c4::Miss::Panic);
        None
    })
}

/// Present callback (hudhook's thread): what the probe compares ER's HAL with (atomics only).
pub fn on_present(ctx: &PreImguiCtx<'_>) {
    // SAFETY: GetCurrentThreadId has no preconditions; the first qword of a COM object is its vtable (non-faulting).
    PRESENT_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    let vt = unsafe { read::<usize>(ctx.command_list.as_raw() as usize) }.unwrap_or(0);
    CL_VTABLE.store(vt, Ordering::Relaxed);
    // C4.1 `present_in_ui`: this Present's callback runs inside an open UI frame (before hudhook's PRESENT_SEQ++)
    if P41_ON.load(Ordering::Relaxed)
        && c4::present_in_ui41(F_BEGIN_SEQ.load(Ordering::SeqCst), F_END_SEQ.load(Ordering::SeqCst), PRESENT_SEQ.load(Ordering::SeqCst))
    {
        P41_PRESENT_IN_UI.fetch_add(1, Ordering::Relaxed);
    }
    if INSTALLED_UI_BEGIN.load(Ordering::Relaxed) {
        PRESENT_DEVICE.store(ctx.device.as_raw() as usize, Ordering::Relaxed);
        PRESENT_QUEUE.store(ctx.command_queue.as_raw() as usize, Ordering::Relaxed);
        // SAFETY: as above.
        BB_VTABLE.store(unsafe { read::<usize>(ctx.render_target.as_raw() as usize) }.unwrap_or(0), Ordering::Relaxed);
        BB_SIZE.store(ctx.width as u64 | (ctx.height as u64) << 24 | (ctx.rtv_format.0 as u64) << 48, Ordering::Relaxed);
    }
}

/// Either probe generation is installed (the present callback publishes then).
pub fn any_installed() -> bool {
    INSTALLED_BEGIN.load(Ordering::Relaxed) || INSTALLED_UI_BEGIN.load(Ordering::Relaxed)
}

/// The body of both detours (no allocation, no lock, no log).
fn note(this: *mut c_void, begin: bool) {
    let t = this as usize;
    if begin {
        BEGINS.fetch_add(1, Ordering::Relaxed);
    } else {
        ENDS.fetch_add(1, Ordering::Relaxed);
    }
    LAST_THIS.store(t, Ordering::Relaxed);
    // SAFETY: GetCurrentThreadId has no preconditions.
    LAST_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    // SAFETY: non-faulting read of the HAL flags (u32).
    if let Some(f) = unsafe { read::<u32>(t + 0x58) } {
        LAST_FLAGS.store(f, Ordering::Relaxed);
    }
    if !begin {
        return;
    }
    let now = now_ms();
    let vt = CL_VTABLE.load(Ordering::Relaxed);
    if vt == 0 || now.saturating_sub(LAST_SCAN_MS.load(Ordering::Relaxed)) < SCAN_EVERY_MS {
        return;
    }
    LAST_SCAN_MS.store(now, Ordering::Relaxed);
    let mut found = u32::MAX;
    for off in (0..SCAN_BYTES).step_by(8) {
        // SAFETY: non-faulting reads of qwords of the HAL object and of what they point at.
        let p = unsafe { read::<usize>(t + off) }.unwrap_or(0);
        if !crate::game::plausible(p) {
            continue;
        }
        if unsafe { read::<usize>(p) } == Some(vt) {
            found = off as u32;
            break;
        }
    }
    CL_OFF.store(found, Ordering::Relaxed);
}

fn now_ms() -> u64 {
    crate::shared::now_ms_if_started()
}

unsafe extern "C" fn begin_detour(this: *mut c_void) -> u64 {
    INFLIGHT.fetch_add(1, Ordering::SeqCst);
    if ACTIVE.load(Ordering::Relaxed) {
        let _ = catch_unwind(AssertUnwindSafe(|| note(this, true)));
    }
    let f = BEGIN_TRAMP.load(Ordering::Acquire);
    // SAFETY: the MinHook trampoline of BeginScene, called with the game's own argument (installed only when set).
    let r = unsafe { std::mem::transmute::<usize, SceneFn>(f)(this) };
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    r
}

unsafe extern "C" fn end_detour(this: *mut c_void) -> u64 {
    INFLIGHT.fetch_add(1, Ordering::SeqCst);
    if ACTIVE.load(Ordering::Relaxed) {
        let _ = catch_unwind(AssertUnwindSafe(|| note(this, false)));
    }
    let f = END_TRAMP.load(Ordering::Acquire);
    // SAFETY: the MinHook trampoline of EndScene.
    let r = unsafe { std::mem::transmute::<usize, SceneFn>(f)(this) };
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    r
}

fn hex16(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Prologue check + MinHook create/enable. The target is stored in [`HOOKED`] for the teardown (the MhHook value has
/// no Drop: dropping it releases nothing; MH_DisableHook takes the target address).
fn hook(target: usize, prologue: &[u8], detour: *mut c_void, tramp: &AtomicUsize, name: &str, slot: usize) -> Result<(), String> {
    let got = code_bytes(target, prologue.len()).ok_or_else(|| format!("why=prologue fn={name} got=unreadable at 0x{target:x}"))?;
    if got != prologue {
        return Err(format!("why=prologue fn={name} got={}", hex16(&got)));
    }
    // SAFETY: idempotent.
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        s => return Err(format!("why=mh_init fn={name} {s:?}")),
    }
    // SAFETY: the prologue matched; the detour has the (this) -> rax shape of the hooked functions.
    let h = unsafe { MhHook::new(target as *mut c_void, detour) }.map_err(|s| format!("why=mh_create fn={name} {s:?}"))?;
    tramp.store(h.trampoline() as usize, Ordering::Release);
    HOOKED[slot].store(target, Ordering::SeqCst);
    // SAFETY: created above, trampoline stored before enabling.
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => Ok(()),
        s => Err(format!("why=mh_enable fn={name} {s:?}")),
    }
}

/// Reads `n` code bytes at `addr` (non-faulting).
fn code_bytes(addr: usize, n: usize) -> Option<Vec<u8>> {
    (0..n).map(|i| unsafe { read::<u8>(addr + i) }).collect()
}

/// Boot thread, after the version gate, only with `[ui] probe_scaleform = true`.
pub fn install() {
    // SAFETY: `None` = the exe module.
    let base = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h.0 as usize,
        Err(e) => {
            log_w!("UI", "probe skip why=module {e}");
            return;
        }
    };
    let own = match crate::version_gate::own() {
        Ok(o) => o,
        Err(e) => {
            log_w!("UI", "probe skip why=build {e}");
            return;
        }
    };
    PROBE_ON.store(true, Ordering::SeqCst);
    match hook(base + own.begin_scene, own.begin_scene_prologue, begin_detour as SceneFn as *mut c_void, &BEGIN_TRAMP, "begin_scene", H_BEGIN_SCENE) {
        Ok(()) => {
            INSTALLED_BEGIN.store(true, Ordering::SeqCst);
            log_i!("UI", "probe hook ok fn=begin_scene addr=0x{:x} prologue=ok (read-only; logs only)", base + own.begin_scene);
        }
        Err(e) => {
            SKIPS.fetch_add(1, Ordering::Relaxed);
            log_w!("UI", "probe skip {e}");
        }
    }
    match hook(base + own.end_scene, own.end_scene_prologue, end_detour as SceneFn as *mut c_void, &END_TRAMP, "end_scene", H_END_SCENE) {
        Ok(()) => {
            INSTALLED_END.store(true, Ordering::SeqCst);
            log_i!("UI", "probe hook ok fn=end_scene addr=0x{:x} prologue=ok (read-only; logs only)", base + own.end_scene);
        }
        Err(e) => {
            SKIPS.fetch_add(1, Ordering::Relaxed);
            log_w!("UI", "probe skip {e}");
        }
    }
}

/// C4: boot thread, after the version gate, when `[ui] probe_scaleform` or `[ui] inject_viewmodel` is on. Checks the
/// build's C4 sites on the loaded image, hooks UiBegin / UiEnd and registers the ECL callback. Returns true when both
/// hooks and the callback are in place. `tag` names the lines (`probe` with the probe on, else `inject`). `sample` =
/// `[ui] probe_layer_sample` (C4.1 P2; only with the probe and the C4.1 sites). `inject41` = `[ui] inject_ui_thread`
/// (with `inject_viewmodel`): the C4.1 sites are checked for the injection too ([`INJ41`]).
pub fn install_ui(probe: bool, sample: bool, inject41: bool) -> bool {
    let tag = if probe { "probe" } else { "inject" };
    let skip = |why: &str, name: &str, got: &str| {
        SKIPS.fetch_add(1, Ordering::Relaxed);
        log_w!("UI", "{tag} skip why={why} fn={name} got={got}");
    };
    let Some(build) = crate::version_gate::build() else {
        skip("build", "ui_begin", "none");
        return false;
    };
    let Some(sf) = build.own().scaleform_ui else {
        skip("build", "ui_begin", "none");
        log_i!("UI", "{tag} n/a: exe table {} has no ScaleformUi addresses (C4 is 2.7.2.0 only)", build.table_name());
        return false;
    };
    // SAFETY: `None` = the exe module.
    let base = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h.0 as usize,
        Err(e) => {
            skip("module", "ui_begin", &e.to_string());
            return false;
        }
    };
    match crate::version_gate::check_sites(&mw2er_core::exe_build::scaleform_ui_sites(build)) {
        Ok(pf) if pf.passed() => log_i!("UI", "{tag} sites {} ({})", pf.summary(), build.table_name()),
        Ok(pf) => {
            for f in &pf.failures {
                let name = f.split(':').next().unwrap_or("?").trim_start_matches("scaleform_");
                let why = if name == "ui_begin" || name == "ui_end" { "prologue" } else { "build" };
                let got = code_bytes(base + if name == "ui_end" { sf.ui_end } else { sf.ui_begin }, 16).map_or("unreadable".into(), |b| hex16(&b));
                skip(why, name, &got);
            }
            return false;
        }
        Err(e) => {
            skip("build", "ui_begin", &e);
            return false;
        }
    }
    // C4.1 P41 (probe only): the chain's vtables and code on the loaded image; a failure keeps the C4 probe running
    // without the C4.1 reads (never counted as a C4 skip)
    if probe || inject41 {
        match crate::version_gate::check_sites(&mw2er_core::exe_build::scaleform_ui41_sites(build)) {
            Ok(pf) if pf.passed() => {
                P41_ON.store(probe, Ordering::SeqCst);
                INJ41.store(inject41, Ordering::SeqCst);
                log_i!("UI", "probe41 sites {} ({}; probe={} inject_ui_thread={})", pf.summary(), build.table_name(), probe as u8, inject41 as u8);
            }
            Ok(pf) => log_w!("UI", "probe41 skip why=sites {} (the C4 probe still runs; no C4.1 reads; no C4.1 injection)", pf.failures.join("; ")),
            Err(e) => log_w!("UI", "probe41 skip why=build {e}"),
        }
    }
    let _ = SF.set(sf);
    BASE.store(base, Ordering::SeqCst);
    let mut ok = true;
    for (name, rva, prologue, detour, tramp, slot, flag) in [
        ("ui_begin", sf.ui_begin, sf.ui_begin_prologue, ui_begin_detour as UiFn as *mut c_void, &UI_BEGIN_TRAMP, H_UI_BEGIN, &INSTALLED_UI_BEGIN),
        ("ui_end", sf.ui_end, sf.ui_end_prologue, ui_end_detour as UiFn as *mut c_void, &UI_END_TRAMP, H_UI_END, &INSTALLED_UI_END),
    ] {
        match hook(base + rva, prologue, detour, tramp, name, slot) {
            Ok(()) => {
                flag.store(true, Ordering::SeqCst);
                log_i!("UI", "{tag} hook ok fn={name} addr=0x{:x} prologue=ok", base + rva);
            }
            Err(e) => {
                SKIPS.fetch_add(1, Ordering::Relaxed);
                log_w!("UI", "{tag} skip {e}");
                ok = false;
            }
        }
    }
    let ecl = ECL_HOOK.set(ecl_hook).is_ok();
    ECL_OK.store(ecl, Ordering::SeqCst);
    log_i!("UI", "{tag} ecl hook={}", if ecl { "ok" } else { "missing" });
    if P41_ON.load(Ordering::SeqCst) {
        // P2 (the layer sample): without it the alpha row stays FAIL "P2 off"
        log_i!("UI", "{}", c4::fmt41_on(crate::ui_sample::boot(sample)));
    }
    ok && ecl
}

/// Task thread, every 5 s while installed: the C3 `UI probe scenes/s=..` line and the C4 lines of design 2.2.
pub struct ProbeLog {
    last_t: f64,
    last_begins: u64,
    last_ends: u64,
    // C4
    c4_t: f64,
    first_begin_t: f64,
    verdict_t: f64,
    hal_logged: Option<(usize, bool, bool, bool, u32, u32)>,
    mode_logged: Option<(u8, u8, bool)>,
    getter_logged: Option<(usize, [u8; 16])>,
    getter_pin: Option<(usize, [u8; 16])>,
    desc_gen: u64,
    rt_seen: Vec<usize>,
    target_size: Option<(u64, u32)>,
    stats: ProbeStats,
    last_present: u64,
    // C4.1
    c41_t: f64,
    v41_t: f64,
    pins41_logged: [Option<(u32, u32, usize, bool, usize, u32, usize)>; 2],
    pins41_n: u32,
    out41_logged: Option<(u32, u64)>,
    out41_n: u32,
    last41: [(usize, usize); 2],
    last41_rtd: usize,
    recreate41_n: u32,
    between41_logged: [bool; 4],
    desc41_gen: u64,
    layer_res41: Vec<usize>,
    s41: ProbeStats41,
    /// C4.1 P2.
    sample: crate::ui_sample::SampleLog,
}

/// Lines of one kind logged on change at most this often per session (a flapping value never floods the log).
const LINES41_MAX: u32 = 64;

/// The packed `P41_OUT` / `P41_SCALE` values as the composite's inputs.
fn out41(out: u32, scale: u64) -> Output41 {
    Output41 {
        b13c: out as u8,
        b13e: (out >> 8) as u8,
        eb9: (out >> 16) as u8,
        scale_x: f32::from_bits(scale as u32),
        scale_y: f32::from_bits((scale >> 32) as u32),
        shaders: (out >> 24) & 1 != 0,
    }
}

impl ProbeLog {
    pub const fn new() -> Self {
        ProbeLog {
            last_t: 0.0,
            last_begins: 0,
            last_ends: 0,
            c4_t: 0.0,
            first_begin_t: 0.0,
            verdict_t: 0.0,
            hal_logged: None,
            mode_logged: None,
            getter_logged: None,
            getter_pin: None,
            desc_gen: 0,
            rt_seen: Vec::new(),
            target_size: None,
            stats: ProbeStats {
                hook_begin: false,
                hook_end: false,
                ecl_hook: false,
                skips: 0,
                hal_seen: false,
                hal_vt_bad: 0,
                hal_dev_bad: 0,
                thread_bad: 0,
                begins: 0,
                begin_tid_ok: 0,
                per_present_bad_windows: 0,
                windows: 0,
                getter_seen: false,
                getter_plain: false,
                getter_changes: 0,
                target_seen: false,
                target_bad: 0,
                armed: 0,
                armed_hit: 0,
                armed_first: 0,
                between_other: 0,
                late: 0,
            },
            last_present: 0,
            c41_t: 0.0,
            v41_t: 0.0,
            pins41_logged: [None; 2],
            pins41_n: 0,
            out41_logged: None,
            out41_n: 0,
            last41: [(0, 0); 2],
            last41_rtd: 0,
            recreate41_n: 0,
            between41_logged: [false; 4],
            desc41_gen: 0,
            layer_res41: Vec::new(),
            s41: ProbeStats41 {
                hooks_c4: false,
                on: false,
                thread_lines: 0,
                mt_bad_lines: 0,
                begins: 0,
                rt_ok: 0,
                rtid_changes: 0,
                present_tid: 0,
                ahead: 0,
                present_in_ui: 0,
                late: 0,
                armed: 0,
                armed_hit: 0,
                armed_idx_max: 0,
                armed_n_max: 0,
                between_other: 0,
                between_frame_max: 0,
                pins_lines: 0,
                pins_bad: 0,
                res_distinct: 0,
                layer_lines: 0,
                layer_bad: 0,
                output_seen: false,
                output: Output41 { b13c: 0, b13e: 0, eb9: 0, scale_x: 0.0, scale_y: 0.0, shaders: false },
                alpha: None,
            },
            sample: crate::ui_sample::SampleLog::new(),
        }
    }

    /// C4.1 (docs/C4_1_DESIGN.md 2.2): pins / output / recreate / between first + on change (each tick), thread and
    /// layer / scene every 5 s, `verdict41` 60 s after the first UiBegin and every 60 s after. Task thread only.
    fn tick_c41(&mut self, now: f64) {
        if !P41_ON.load(Ordering::Relaxed) {
            return;
        }
        if self.v41_t == 0.0 {
            self.v41_t = now + 60.0;
        }
        self.sample.tick(now);
        // the resources' module check, cached per vtable (here, not on the render thread: no loader lock there)
        let own = BB_VTABLE.load(Ordering::Relaxed);
        let mut res_mod = [false; 2];
        let mut vts = [0usize; 2];
        for i in 0..2 {
            let vt = P41_RES_VT[i].load(Ordering::Relaxed);
            vts[i] = vt;
            if vt == 0 || own == 0 {
                continue;
            }
            if P41_VT_OK[i].load(Ordering::Relaxed) & !1 != vt {
                let m = module_of(vt);
                P41_VT_OK[i].store(vt | (m != 0 && m == module_of(own)) as usize, Ordering::Relaxed);
            }
            res_mod[i] = P41_VT_OK[i].load(Ordering::Relaxed) == vt | 1;
        }
        let rtd = RtData41 {
            rt: P41_RT.load(Ordering::Relaxed),
            data: P41_RTD.load(Ordering::Relaxed),
            fmt: P41_RTD_FMT.load(Ordering::Relaxed),
            rtv: P41_RTD_RTV.load(Ordering::Relaxed),
        };
        let cands = [(0usize, UiPath::Layer), (1, UiPath::Scene)];
        // pins (first + on change; only once the texture exists and a Present published our D3D12 module)
        for (i, which) in cands {
            if P41_SEL[i].load(Ordering::Relaxed) == 0 || own == 0 {
                continue;
            }
            let c = Chain41::from_bits(
                P41_BITS[i].load(Ordering::Relaxed) as u8,
                P41_COUNT[i].load(Ordering::Relaxed),
                0,
                P41_RES[i].load(Ordering::Relaxed),
                P41_RTV[i].load(Ordering::Relaxed),
            );
            let key = (c.bits() as u32, c.count, c.res, res_mod[i], c.rtv, rtd.fmt, rtd.rtv);
            if self.pins41_logged[i] == Some(key) || self.pins41_n >= LINES41_MAX {
                continue;
            }
            self.pins41_logged[i] = Some(key);
            self.pins41_n += 1;
            log_i!("UI", "{}", c4::fmt41_pins(which, &c, res_mod[i], &rtd));
            if i == 0 {
                self.s41.pins_lines += 1;
                if !c4::pins_line_ok41(&c, res_mod[i], &rtd) {
                    self.s41.pins_bad += 1;
                }
                if c.res != 0 && !self.layer_res41.contains(&c.res) && self.layer_res41.len() < 64 {
                    self.layer_res41.push(c.res);
                }
            }
        }
        // recreate (each change of a texture object, its resource, or the HAL render target's rtData)
        for (i, which) in cands {
            let (sel, res) = (P41_SEL[i].load(Ordering::Relaxed), P41_RES[i].load(Ordering::Relaxed));
            let (ls, lr) = self.last41[i];
            for (what, old, new) in [("sel", ls, sel), ("res", lr, res)] {
                if old != 0 && new != old && self.recreate41_n < LINES41_MAX {
                    self.recreate41_n += 1;
                    log_i!("UI", "{}", c4::fmt41_recreate(what, which, old, new, now));
                }
            }
            self.last41[i] = (sel, res);
        }
        if self.last41_rtd != 0 && rtd.data != self.last41_rtd && self.recreate41_n < LINES41_MAX {
            self.recreate41_n += 1;
            log_i!("UI", "{}", c4::fmt41_recreate("rtdata", UiPath::Layer, self.last41_rtd, rtd.data, now));
        }
        self.last41_rtd = rtd.data;
        // output (first + on change)
        if P41_OUT_SEEN.load(Ordering::Relaxed) {
            let key = (P41_OUT.load(Ordering::Relaxed), P41_SCALE.load(Ordering::Relaxed));
            let o = out41(key.0, key.1);
            if self.out41_logged != Some(key) && self.out41_n < LINES41_MAX {
                self.out41_logged = Some(key);
                self.out41_n += 1;
                log_i!("UI", "{}", c4::fmt41_output(&o));
            }
            self.s41.output_seen = true;
            self.s41.output = o;
        }
        // between (the first 4 events)
        for k in 0..P41_BETWEEN_FRAME.len() {
            let fr = P41_BETWEEN_FRAME[k].load(Ordering::SeqCst);
            if fr != 0 && !self.between41_logged[k] {
                self.between41_logged[k] = true;
                self.s41.between_frame_max = self.s41.between_frame_max.max(fr - 1);
                log_i!("UI", "{}", c4::fmt41_between(k as u32 + 1, fr - 1, P41_BETWEEN_LIST[k].load(Ordering::SeqCst)));
            }
        }
        if now - self.c41_t >= 5.0 {
            self.c41_t = now;
            let t = Thread41 {
                mt: P41_MT_SEEN.load(Ordering::Relaxed) > 0 && P41_MT_BAD.load(Ordering::Relaxed) == 0,
                rt_tid: P41_RT_TID.load(Ordering::Relaxed),
                hal_tid: P41_HAL_TID.load(Ordering::Relaxed),
                begin_tid: P41_BEGIN_TID.load(Ordering::Relaxed),
                present_tid: HH_PRESENT_TID.load(Ordering::Relaxed),
                rt_ok: P41_RT_OK.load(Ordering::Relaxed),
                begins: P41_HAL_BEGINS.load(Ordering::Relaxed),
                rtid_changes: P41_RTID_CHANGES.load(Ordering::Relaxed),
                ahead: P41_AHEAD.load(Ordering::Relaxed),
                present_in_ui: P41_PRESENT_IN_UI.load(Ordering::Relaxed),
            };
            log_i!("UI", "{}", c4::fmt41_thread(&t));
            let s = &mut self.s41;
            s.thread_lines += 1;
            s.mt_bad_lines += !t.mt as u32;
            (s.begins, s.rt_ok, s.rtid_changes, s.present_tid, s.ahead, s.present_in_ui) = (t.begins, t.rt_ok, t.rtid_changes, t.present_tid, t.ahead, t.present_in_ui);
            // layer / scene: the GetDesc sample the detour took since the last window
            let desc_gen = P41_DESC_GEN.load(Ordering::Acquire);
            if desc_gen != self.desc41_gen {
                self.desc41_gen = desc_gen;
                let bb = BB_SIZE.load(Ordering::Relaxed);
                let bb = ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32, (bb >> 48) as u32);
                for (i, which) in cands {
                    let res = P41_DESC_RES[i].load(Ordering::Relaxed);
                    if res == 0 {
                        let cur = P41_RES[i].load(Ordering::Relaxed);
                        // the layer without a desc: no chained resource, or the module check (done) refused it
                        if i == 0 && P41_SEL[0].load(Ordering::Relaxed) != 0 && (cur == 0 || P41_VT_OK[0].load(Ordering::Relaxed) & !1 == vts[0]) {
                            log_i!("UI", "probe41 layer res=0x{cur:x} desc=none (no chained resource, or its vtable is outside the D3D12 runtime module)");
                            self.s41.layer_lines += 1;
                            self.s41.layer_bad += 1;
                        }
                        continue;
                    }
                    let d = unpack_desc([0, 1, 2, 3].map(|k| P41_DESC[i][k].load(Ordering::Relaxed)));
                    let rtdata = (P41_RTD_OK.load(Ordering::Relaxed), P41_RTD_N.load(Ordering::Relaxed));
                    log_i!("UI", "{}", c4::fmt41_target(which, res, &d, bb, P41_RES_CHANGES[i].load(Ordering::Relaxed), (i == 0).then_some(rtdata)));
                    if i == 0 {
                        self.s41.layer_lines += 1;
                        if !c4::layer_ok41(&d, bb, rtdata) {
                            self.s41.layer_bad += 1;
                        }
                        // the pins row: rtData's format is the layer's
                        if rtd.fmt != d.format {
                            self.s41.pins_bad += 1;
                        }
                    }
                }
            }
            P41_DESC_DUE.store(true, Ordering::Relaxed);
        }
        if now >= self.v41_t {
            self.v41_t = now + 60.0;
            let s = &mut self.s41;
            s.hooks_c4 = INSTALLED_UI_BEGIN.load(Ordering::Relaxed)
                && INSTALLED_UI_END.load(Ordering::Relaxed)
                && ECL_OK.load(Ordering::Relaxed)
                && SKIPS.load(Ordering::Relaxed) == 0;
            s.on = true;
            s.late = E[E_LATE].load(Ordering::Relaxed);
            s.armed = P_ARMED.load(Ordering::Relaxed);
            s.armed_hit = E[E_ARMED_HIT].load(Ordering::Relaxed);
            s.armed_idx_max = E[E_IDX_MAX].load(Ordering::Relaxed);
            s.armed_n_max = E[E_N_MAX].load(Ordering::Relaxed);
            s.between_other = E[E_BETWEEN].load(Ordering::Relaxed);
            s.res_distinct = self.layer_res41.len() as u32;
            // P2: the convention of every sample so far (None without P2, undecided, or after a D3D12 failure)
            s.alpha = self.sample.alpha();
            if self.sample.count() == 0 {
                log_i!("UI", "probe41 sample none yet ({})", self.sample.misses());
            }
            // C4.1 PASS I41: the verdict gates the UI-thread injection (ui_inject reads these)
            let (ready, why, _, alpha) = c4::verdict41(s);
            PROBE41_ALPHA.store(alpha_code(alpha), Ordering::SeqCst);
            PROBE41_ONLY_ALPHA.store(why == ["alpha"], Ordering::SeqCst);
            PROBE41_READY.store(ready, Ordering::SeqCst);
            PROBE41_JUDGED.store(true, Ordering::SeqCst);
            log_i!("UI", "{}", c4::fmt41_verdict(s));
        }
    }

    pub fn tick(&mut self, now: f64) {
        self.tick_c4(now);
        if !(INSTALLED_BEGIN.load(Ordering::Relaxed) || INSTALLED_END.load(Ordering::Relaxed)) || now - self.last_t < 5.0 {
            return;
        }
        let dt = if self.last_t > 0.0 { now - self.last_t } else { 5.0 };
        self.last_t = now;
        let b = BEGINS.load(Ordering::Relaxed);
        let e = ENDS.load(Ordering::Relaxed);
        let tid = LAST_TID.load(Ordering::Relaxed);
        let present = PRESENT_TID.load(Ordering::Relaxed);
        let off = CL_OFF.load(Ordering::Relaxed);
        log_i!(
            "UI",
            "probe scenes/s={:.1} ends/s={:.1} thread={} this=0x{:x} flags=0x{:x} cl_off={}",
            (b - self.last_begins) as f64 / dt,
            (e - self.last_ends) as f64 / dt,
            if tid == 0 { "none" } else if tid == present { "present" } else { "other" },
            LAST_THIS.load(Ordering::Relaxed),
            LAST_FLAGS.load(Ordering::Relaxed),
            if off == u32::MAX { "none".to_string() } else { format!("0x{off:x}") }
        );
        self.last_begins = b;
        self.last_ends = e;
    }

    /// The C4 lines (design 2.2): hal / mode / getter first + on change, rt / frame / ecl every 5 s, the verdict at
    /// 60 s after the first UiBegin and every 60 s after.
    fn tick_c4(&mut self, now: f64) {
        if !PROBE_ON.load(Ordering::Relaxed) || !INSTALLED_UI_BEGIN.load(Ordering::Relaxed) {
            return;
        }
        let begins = P_BEGINS.load(Ordering::Relaxed);
        if begins == 0 {
            return;
        }
        if self.first_begin_t == 0.0 {
            self.first_begin_t = now;
            self.verdict_t = now + 60.0;
        }
        self.tick_c41(now);
        let present_tid = HH_PRESENT_TID.load(Ordering::Relaxed);
        let vt = P_VT.load(Ordering::Relaxed);
        if vt != 0 {
            let dev = P_DEV.load(Ordering::Relaxed);
            let pdev = PRESENT_DEVICE.load(Ordering::Relaxed);
            let rtid = P_RTID.load(Ordering::Relaxed);
            let hal = (vt, P_VT_OK.load(Ordering::Relaxed), dev != 0 && dev == pdev, P_QUEUE.load(Ordering::Relaxed) == PRESENT_QUEUE.load(Ordering::Relaxed), rtid, present_tid);
            if self.hal_logged != Some(hal) {
                self.hal_logged = Some(hal);
                self.stats.hal_seen = true;
                log_i!("UI", "{}", c4::fmt_hal(hal.0, hal.1, hal.2, hal.3, hal.4, hal.5));
            }
            let mode = (P_B13C.load(Ordering::Relaxed), P_B13E.load(Ordering::Relaxed), P_LAYER.load(Ordering::Relaxed) != 0);
            if self.mode_logged != Some(mode) {
                self.mode_logged = Some(mode);
                log_i!("UI", "{}", c4::fmt_mode(mode.0, mode.1, mode.2 as usize));
            }
            let ga = P_GETTER_ADDR.load(Ordering::Relaxed);
            if ga != 0 {
                let mut gb = [0u8; 16];
                gb[..8].copy_from_slice(&P_GETTER[0].load(Ordering::Relaxed).to_le_bytes());
                gb[8..].copy_from_slice(&P_GETTER[1].load(Ordering::Relaxed).to_le_bytes());
                if self.getter_logged != Some((ga, gb)) {
                    self.getter_logged = Some((ga, gb));
                    log_i!("UI", "{}", c4::fmt_getter(ga, &gb));
                    match self.getter_pin {
                        None => {
                            self.getter_pin = Some((ga, gb));
                            self.stats.getter_seen = true;
                            self.stats.getter_plain = c4::parse_getter(&gb).is_some();
                        }
                        Some(p) if p != (ga, gb) => self.stats.getter_changes += 1,
                        Some(_) => {}
                    }
                }
            }
        }
        if now - self.c4_t < 5.0 {
            return;
        }
        self.c4_t = now;
        // rt: the GetDesc sample the detour took since the last window, then ask for the next one
        let desc_gen = P_DESC_GEN.load(Ordering::Acquire);
        if desc_gen != self.desc_gen {
            self.desc_gen = desc_gen;
            let bb = BB_SIZE.load(Ordering::Relaxed);
            let bb = ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32, (bb >> 48) as u32);
            let mode = self.mode_logged.unwrap_or((0, 0, false));
            let sel = c4::select_path(mode.0, mode.1, mode.2 as usize);
            for (i, which) in [(0, sel), (1, if sel == UiPath::Layer { UiPath::Scene } else { UiPath::Layer })] {
                let res = P_DESC_RES[i].load(Ordering::Relaxed);
                if res == 0 {
                    if i == 0 {
                        log_i!("UI", "probe rt which={} res=0x{:x} desc=none (no target, or its vtable is outside the D3D12 runtime module)", which.label(), P_RES.load(Ordering::Relaxed));
                        self.stats.target_bad += 1;
                    }
                    continue;
                }
                let d = unpack_desc([0, 1, 2, 3].map(|k| P_DESC[i][k].load(Ordering::Relaxed)));
                if i == 0 && !self.rt_seen.contains(&res) && self.rt_seen.len() < 64 {
                    self.rt_seen.push(res);
                }
                log_i!("UI", "{}", c4::fmt_rt(which, res, &d, bb, self.rt_seen.len() as u32));
                if i == 0 {
                    self.stats.target_seen = true;
                    let fmt_ok = c4::target_rule(&d, which, true).is_ok();
                    let size_ok = match self.target_size {
                        None => {
                            self.target_size = Some((d.width, d.height));
                            true
                        }
                        Some(s) => s == (d.width, d.height) || (d.width, d.height) == (bb.0 as u64, bb.1),
                    };
                    if !fmt_ok || !size_ok {
                        self.stats.target_bad += 1;
                    }
                }
            }
        }
        P_DESC_DUE.store(true, Ordering::Relaxed);
        // frame
        let ends = P_ENDS.load(Ordering::Relaxed);
        let nomovies = P_NOMOVIES.load(Ordering::Relaxed);
        let tid_ok = P_TID_OK.load(Ordering::Relaxed);
        let present = PRESENT_SEQ.load(Ordering::Relaxed);
        let (mn, mx) = (P_PER_MIN.swap(u32::MAX, Ordering::Relaxed), P_PER_MAX.swap(0, Ordering::Relaxed));
        let mn = if mn == u32::MAX { 0 } else { mn };
        if present > self.last_present {
            self.stats.windows += 1;
            if mx > 1 {
                self.stats.per_present_bad_windows += 1;
            }
        }
        self.last_present = present;
        log_i!("UI", "{}", c4::fmt_frame(present, begins, ends, nomovies, mn, mx, tid_ok));
        // ecl
        let t = EclTotals {
            total: E[E_TOTAL].load(Ordering::Relaxed),
            halq: E[E_HALQ].load(Ordering::Relaxed),
            otherq: E[E_OTHERQ].load(Ordering::Relaxed),
            pre: E[E_PRE].load(Ordering::Relaxed),
            ui: E[E_UI].load(Ordering::Relaxed),
            post: E[E_POST].load(Ordering::Relaxed),
            armed: P_ARMED.load(Ordering::Relaxed),
            armed_hit: E[E_ARMED_HIT].load(Ordering::Relaxed),
            armed_first: E[E_ARMED_FIRST].load(Ordering::Relaxed),
            armed_idx_max: E[E_IDX_MAX].load(Ordering::Relaxed),
            armed_n_max: E[E_N_MAX].load(Ordering::Relaxed),
            armed_tid_ok: E[E_TID_OK].load(Ordering::Relaxed),
            between_other: E[E_BETWEEN].load(Ordering::Relaxed),
            late: E[E_LATE].load(Ordering::Relaxed),
        };
        log_i!("UI", "{}", c4::fmt_ecl(&t));
        // verdict (60 s after the first UiBegin, then every 60 s)
        if now >= self.verdict_t {
            self.verdict_t = now + 60.0;
            let s = &mut self.stats;
            s.hook_begin = INSTALLED_UI_BEGIN.load(Ordering::Relaxed);
            s.hook_end = INSTALLED_UI_END.load(Ordering::Relaxed);
            s.ecl_hook = ECL_OK.load(Ordering::Relaxed);
            s.skips = SKIPS.load(Ordering::Relaxed);
            s.hal_vt_bad = P_VT_BAD.load(Ordering::Relaxed);
            s.hal_dev_bad = P_DEV_BAD.load(Ordering::Relaxed);
            s.thread_bad = P_THREAD_BAD.load(Ordering::Relaxed);
            s.begins = begins;
            s.begin_tid_ok = tid_ok;
            s.armed = t.armed;
            s.armed_hit = t.armed_hit;
            s.armed_first = t.armed_first;
            s.between_other = t.between_other;
            s.late = t.late;
            let (ready, _) = c4::verdict(s);
            PROBE_READY.store(ready, Ordering::SeqCst);
            PROBE_JUDGED.store(true, Ordering::SeqCst);
            log_i!("UI", "{}", c4::fmt_verdict(s));
        }
    }
}
