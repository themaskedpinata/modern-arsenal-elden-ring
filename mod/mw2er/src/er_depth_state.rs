//! C6 depth 5 (docs/C6_HOLD_DESIGN.md part 15): ER's scene depth hides world guns.
//!
//! - **Probe P-S** (`[coop] probe_er_depth_state`, cold, off by default; needs `probe_er_depth` and the UiBegin hook of
//!   `[ui] inject_viewmodel`): the task thread installs a MinHook on d3d12's `ID3D12GraphicsCommandList::ResourceBarrier`
//!   (slot 26 of OUR Present list's vtable, only inside the back buffer's D3D12 module, refused on a foreign `jmp`
//!   prologue). The detour (`mw2er_render::er_depth::resource_barrier_detour`) calls the ORIGINAL FIRST and records the
//!   transitions on the watched resource into a lock-free table; [`fold_ecl`] (hudhook's ECL callback, before any
//!   splice) applies them in queue order and stores the state at the armed UiBegin call. Teardown: recording off first,
//!   then `MH_DisableHook` (a passthrough; the trampoline is kept, never freed in-session: the review-C4 rule).
//! - **The watched resources**: UiBegin's detour (ER's render thread) publishes candidate A's resource only after this
//!   frame's pinned chain (6/6) passed and its vtable equals the one the task proved in the D3D12 module; a new resource
//!   must be seen 2 frames in a row, and bumps the epoch (Q unknown, 2 s not judged). C6 round 6 (work/c6/depth6.md, docs
//!   15.8): candidate B (the scene context's DefDepthStencil `S[0]+0xE8`, its pinned dsb chain + the proven vtable) is
//!   watched the same way with its own table / fold; B's own UiBegin barrier (`plane 1 0xC0 -> 0x10`) is the positive
//!   control; the detour counts every call before any gate and keeps a census of all depth-state traffic; the verdict
//!   is per candidate (`why=hook|idle|control|..`) and the copy / sample read the candidate the fold proved readable.
//! - **Probe P-S2** (`probe_er_depth_sample`) and **the copy** (`world_gun_er_depth`, live only on this session's
//!   verdict): UiBegin's detour records OUR list (`mw2er_render::er_depth::DepthReader`, worker-created) when
//!   `er_depth_state::copy_gate` passes; [`on_ecl`] splices it at index 0 of the armed call only when the fold proved
//!   plane 0 readable there, followed by our fence. **ER's resource is never barriered, bound for writing or written.**
//! - **The prime** ([`present_prime`]): at Present, third person only, a copy of this Present's seq that was spliced
//!   (no Signal failure since) is written into OUR world-gun depth before the capsules and the guns; fast turns skip it.
//! - Any surprise trips for the session (`DEPTH live trip why=..`): the world guns then draw with the capsules only.
//!
//! Render thread / ECL callback: atomics, non-faulting reads, `try_lock`, one GetDesc and the record only (no
//! allocation, no log, no blocking lock). The task thread logs.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::hooks::dx12::pre_imgui::{PRESENT_SEQ, SPLICE_SIGNAL_FAILS, Splice};
use hudhook::mh::{MH_DisableHook, MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::er_depth::{DsbChain, SceneChain};
use mw2er_core::er_depth_state::{
    self as eds, CAND_TAGS, Conv, CopyIn, CopyMiss, Ctl, EclIn, Fold, LiveIn, LiveStep, SAMPLE_N, SLOTS, SLOT_RESOURCE_BARRIER, SelfTally, StateStats,
    Table, Want,
};
use mw2er_core::exe_build::ScaleformUi;
use mw2er_render::er_depth::{CENSUS, DepthReader, PrimeIn, ResourceBarrierFn, TABLE, TABLE_B, WATCH, WATCH_B};
use mw2er_render::VmError;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandList, ID3D12Fence, ID3D12Resource};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::Interface;

use crate::game::read;
use crate::log::{log_e, log_i, log_w};
use crate::shared::{self, lock, try_lock};
use crate::ui_probe::{self, UiFacts};

// ---- boot / config ---------------------------------------------------------------------------------------------------

/// `probe_er_depth_state` / `probe_er_depth_sample` / `world_gun_er_depth` at boot (cold).
static CFG_STATE: AtomicBool = AtomicBool::new(false);
static CFG_SAMPLE: AtomicBool = AtomicBool::new(false);
static CFG_LIVE: AtomicBool = AtomicBool::new(false);
/// `world_gun_er_depth` now (hot off: the task clears it).
static LIVE_CFG_NOW: AtomicBool = AtomicBool::new(false);
/// The hook is installed and enabled; its target; the install was tried.
static HOOK_ON: AtomicBool = AtomicBool::new(false);
static HOOK_TARGET: AtomicUsize = AtomicUsize::new(0);
static HOOK_TRIED: AtomicBool = AtomicBool::new(false);

/// The queue-order folds (one per candidate: 0 = A, 1 = B; C6 round 6).
static FOLDS: [Fold; 2] = [Fold::new(), Fold::new()];

/// Candidate `k`'s barrier table and watched resource (mw2er-render's statics: the detour fills both).
fn table(k: usize) -> &'static Table {
    if k == 0 { &TABLE } else { &TABLE_B }
}

fn watch(k: usize) -> &'static AtomicUsize {
    if k == 0 { &WATCH } else { &WATCH_B }
}

/// The frame facts of the last UiBegin (for the fold's armed call and the splice).
static ARMED_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static ARMED_LIST: AtomicUsize = AtomicUsize::new(0);
static ARMED_QUEUE: AtomicUsize = AtomicUsize::new(0);
static ARMED_TID: AtomicU32 = AtomicU32::new(0);
static RT_TID: AtomicU32 = AtomicU32::new(0);
static LAST_BEGIN_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
/// The watch per candidate: res seen in a row, the epoch and its start, the vtable proven in the D3D12 module.
static PREV_RES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static STABLE_N: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static EPOCH: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static EPOCH_MS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static WATCH_CHANGES: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static PROVEN_VT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
/// C6 round 6: the candidate the live copy reads (set at live on) and the one the pending sample read.
static LIVE_CAND: AtomicU8 = AtomicU8::new(0);
static SAMPLE_CAND: AtomicU8 = AtomicU8::new(0);
/// The last desc of the watched resource (w | h << 32, format) for the `desc` trip.
static DESC_RES: AtomicUsize = AtomicUsize::new(0);
static DESC_KEY: AtomicU64 = AtomicU64::new(0);

// ---- the copy / sample ------------------------------------------------------------------------------------------------

/// Live (15.3) and tripped for the session.
static LIVE: AtomicBool = AtomicBool::new(false);
static TRIPPED: AtomicBool = AtomicBool::new(false);
static TRIP_WHY: AtomicU8 = AtomicU8::new(0);
const TRIPS: [&str; 11] = ["none", "mismatch", "queue", "split", "alias", "table_full", "state", "hresult", "desc", "panic", "self"];
/// The reader exists (worker-created) and its raw pointers are published.
static READER: Mutex<Option<DepthReader>> = Mutex::new(None);
static READER_READY: AtomicBool = AtomicBool::new(false);
static READER_SIZE: AtomicU64 = AtomicU64::new(0);
static READER_STARTED: AtomicBool = AtomicBool::new(false);
static LIST_RAW: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static FENCE_RAW: AtomicUsize = AtomicUsize::new(0);
/// `CUR`: 0 = idle, else Armed: seq + 1 in the low 56 bits, slot in bits 56-57, copy bit 58, sample bit 59.
static CUR: AtomicU64 = AtomicU64::new(0);
static SLOT_VALUE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static NEXT_VALUE: AtomicU64 = AtomicU64::new(0);
/// The last spliced copy: its seq (u64::MAX none), slot and the splice-Signal failure count at its splice.
static LAST_COPY_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_COPY_SLOT: AtomicUsize = AtomicUsize::new(0);
static LAST_COPY_FAILS: AtomicU64 = AtomicU64::new(0);
/// P-S2: the task asks (5 s); a spliced sample's fence value (0 = none pending), its state at UI, chest z and turn.
static SAMPLE_DUE: AtomicBool = AtomicBool::new(false);
static SAMPLE_VALUE: AtomicU64 = AtomicU64::new(0);
static SAMPLE_STATE: AtomicU32 = AtomicU32::new(eds::UNKNOWN);
static SAMPLE_CHEST: AtomicU64 = AtomicU64::new(u64::MAX);
static SAMPLE_CHEST_Z: AtomicU32 = AtomicU32::new(0);
static SAMPLE_TURN: AtomicU32 = AtomicU32::new(0);
/// C6 round 7: the camera move (m per task tick) of the armed sample.
static SAMPLE_MOVE: AtomicU32 = AtomicU32::new(0);
/// The chest pixel (x | y << 32, u64::MAX = none), its view z and the camera turn of the tick (task thread).
static CHEST_PX: AtomicU64 = AtomicU64::new(u64::MAX);
static CHEST_Z: AtomicU32 = AtomicU32::new(0);
static TURN: AtomicU32 = AtomicU32::new(0);
static LAST_TASK_FWD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
/// C6 round 7 (depth7.md 1 item 3): the camera move of the tick ("still" needs < 5 cm) and the last camera position.
static MOVE: AtomicU32 = AtomicU32::new(0);
static LAST_TASK_POS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
/// C6 part 16.8 (depth8.md 1): per candidate, the near the prime linearises with relative to CSCam's (f32 bits, >= 1
/// once a self check passed; 0 = none yet = 1) and the metres added to its bias (a proven line with the surface behind
/// the bone). Never lowered while live: a larger near / bias only pushes ER's surfaces back (under-occludes).
static NEAR_REL: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static BIAS_ADD: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];

/// The live near ratio of candidate k (>= 1).
fn near_rel(k: usize) -> f32 {
    f32::from_bits(NEAR_REL[k & 1].load(Ordering::Relaxed)).max(1.0)
}
/// What Present last wanted (`Want` code) and when (ms).
static WANT: AtomicU8 = AtomicU8::new(0);
static WANT_MS: AtomicU64 = AtomicU64::new(0);
/// Present's previous camera forward (the prime's turn rule).
static LAST_FWD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
/// Counters: per-frame misses (`CopyMiss` order), copies spliced, frames primed, attempts, try_lock failures.
static MISS: [AtomicU64; 17] = [const { AtomicU64::new(0) }; 17];
static COPIES: AtomicU64 = AtomicU64::new(0);
static PRIMED: AtomicU64 = AtomicU64::new(0);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static LOCKED: AtomicU64 = AtomicU64::new(0);
static SAMPLES_SPLICED: AtomicU64 = AtomicU64::new(0);
/// The first D3D12 error (a static `VmError::D3d`: no allocation on the render thread) and the reader's failure.
static ERR: Mutex<Option<VmError>> = Mutex::new(None);
static FAILED: AtomicBool = AtomicBool::new(false);

const WANT_FRESH_MS: u64 = 250;
const ARMED_COPY: u64 = 1 << 58;
const ARMED_SAMPLE: u64 = 1 << 59;
/// C6 round 6: the armed record reads candidate B.
const ARMED_CAND_B: u64 = 1 << 60;
const SEQ_MASK: u64 = (1 << 56) - 1;

fn miss(m: CopyMiss) {
    MISS[m.index()].fetch_add(1, Ordering::Relaxed);
}

fn armed_pack(seq: u64, slot: usize, copy: bool, sample: bool) -> u64 {
    ((seq + 1) & SEQ_MASK) | ((slot as u64 & 3) << 56) | if copy { ARMED_COPY } else { 0 } | if sample { ARMED_SAMPLE } else { 0 }
}

/// The candidate of an Armed `CUR` (0 = A, 1 = B).
fn armed_cand(v: u64) -> usize {
    (v & ARMED_CAND_B != 0) as usize
}

/// (seq, slot, copy, sample) of an Armed `CUR` (None = idle).
fn armed_unpack(v: u64) -> Option<(u64, usize, bool, bool)> {
    let s = v & SEQ_MASK;
    (s != 0).then(|| (s - 1, ((v >> 56) & 3) as usize, v & ARMED_COPY != 0, v & ARMED_SAMPLE != 0))
}

/// Trips the live path for the session (any thread; atomics only). The task logs it.
fn trip(why: &'static str) {
    LIVE.store(false, Ordering::SeqCst);
    if !TRIPPED.swap(true, Ordering::SeqCst) {
        TRIP_WHY.store(TRIPS.iter().position(|t| *t == why).unwrap_or(0) as u8, Ordering::SeqCst);
    }
}

fn fail_err(e: VmError) {
    if let Some(mut g) = try_lock(&ERR)
        && g.is_none()
    {
        *g = Some(e);
    }
    FAILED.store(true, Ordering::SeqCst);
    trip("hresult");
}

/// Boot (`boot.rs`, after the UI hooks and the depth probe): the wanted flags (cold). The hook itself is installed by
/// the task once a Present published our list's vtable.
pub fn boot(state: bool, sample: bool, live: bool) {
    CFG_STATE.store(state, Ordering::SeqCst);
    CFG_SAMPLE.store(state && sample, Ordering::SeqCst);
    CFG_LIVE.store(live, Ordering::SeqCst);
    LIVE_CFG_NOW.store(live, Ordering::SeqCst);
    if !state && !live {
        return;
    }
    if state && !crate::er_depth::on() {
        log_w!("DEPTH", "state hook off why=probe_off (needs [coop] probe_er_depth with its sites)");
        HOOK_TRIED.store(true, Ordering::SeqCst);
    } else if state && !ui_probe::INSTALLED_UI_BEGIN.load(Ordering::Relaxed) {
        log_w!("DEPTH", "state hook off why=no_ui_begin (needs [ui] inject_viewmodel: the fold and the copy run in UiBegin / the ECL callback)");
        HOOK_TRIED.store(true, Ordering::SeqCst);
    } else if !state {
        log_w!("DEPTH", "state hook off why=flag ([coop] probe_er_depth_state = false; world_gun_er_depth needs it: no per-build pin yet)");
        HOOK_TRIED.store(true, Ordering::SeqCst);
    }
}

/// The barrier probe runs (UiBegin's detour feeds it).
pub fn on() -> bool {
    HOOK_ON.load(Ordering::Relaxed)
}

/// The ER-depth prime pass is wanted at boot (the loader prepares it; it may run before [`boot`], so the config too).
pub fn prime_wanted() -> bool {
    CFG_LIVE.load(Ordering::Relaxed) || shared::config().coop.world_gun_er_depth
}

fn rd_u64(a: usize) -> Option<u64> {
    // SAFETY: non-faulting read.
    unsafe { read::<u64>(a) }
}

/// Task thread: the hook install (once our list's vtable and the back buffer's are known).
fn install_hook() -> Result<(usize, String), &'static str> {
    let vt = ui_probe::CL_VTABLE.load(Ordering::Relaxed);
    let bb = ui_probe::BB_VTABLE.load(Ordering::Relaxed);
    if vt == 0 || bb == 0 {
        return Err("no_vtable");
    }
    let target = rd_u64(vt + 8 * SLOT_RESOURCE_BARRIER).unwrap_or(0) as usize;
    let m = ui_probe::module_of(target);
    if target == 0 || m == 0 || m != ui_probe::module_of(bb) {
        return Err("module");
    }
    // SAFETY: non-faulting reads of the function's first bytes.
    let p = [0, 1, 2].map(|k| unsafe { read::<u8>(target + k) }.unwrap_or(0));
    if p[0] == 0xE9 || p[0] == 0xEB || (p[0] == 0xFF && p[1] == 0x25) || (p[0] == 0x48 && p[1] == 0xFF && p[2] == 0x25) {
        return Err("prologue");
    }
    // SAFETY: idempotent.
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        _ => return Err("minhook"),
    }
    let detour = mw2er_render::er_depth::resource_barrier_detour as ResourceBarrierFn as *mut c_void;
    // SAFETY: the target is d3d12's ResourceBarrier (slot 26 of a live list's vtable, in the D3D12 module, no foreign
    // jmp); the detour has its exact signature and calls the trampoline first.
    let h = unsafe { MhHook::new(target as *mut c_void, detour) }.map_err(|_| "minhook")?;
    mw2er_render::er_depth::TRAMP.store(h.trampoline() as usize, Ordering::Release);
    HOOK_TARGET.store(target, Ordering::SeqCst);
    // review 2026-10-21: pin mw2er.dll before enabling (never unmapped by FreeLibrary): ER calls ResourceBarrier from
    // several recording threads all frame, so at an unload some thread is inside the original with a return address in
    // the detour; MH_DisableHook cannot move that, and no in-flight count covers it. Pinned, detach() never runs for a
    // FreeLibrary (only process exit), so the hook stays a passthrough-able detour for the process lifetime.
    if !pin_self() {
        return Err("pin");
    }
    // C6 round 6: recording on with the hook (the counters and the census run before any candidate is watched; a
    // table with no watched resource records nothing)
    mw2er_render::er_depth::ON.store(true, Ordering::SeqCst);
    // SAFETY: created above, the trampoline stored before enabling.
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => {}
        _ => {
            mw2er_render::er_depth::ON.store(false, Ordering::SeqCst);
            return Err("minhook");
        }
    }
    Ok((target, module_name(m)))
}

/// Pins the module holding the barrier detour (GET_MODULE_HANDLE_EX_FLAG_PIN | FROM_ADDRESS): it stays loaded until
/// process exit whatever FreeLibrary calls follow. True when pinned.
fn pin_self() -> bool {
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW};
    let addr = mw2er_render::er_depth::resource_barrier_detour as ResourceBarrierFn as usize;
    let mut h = windows::Win32::Foundation::HMODULE::default();
    // SAFETY: FROM_ADDRESS: the address of our own detour is passed as the "name" pointer; PIN: the module is never
    // unloaded (no handle to release).
    unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, windows::core::PCWSTR(addr as *const u16), &mut h) }.is_ok()
        && !h.is_invalid()
}

fn module_name(m: usize) -> String {
    let mut buf = [0u16; 260];
    // SAFETY: a module handle from GetModuleHandleExW; the buffer is ours.
    let n = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(Some(windows::Win32::Foundation::HMODULE(m as *mut c_void)), &mut buf) } as usize;
    let full = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    full.rsplit(['\\', '/']).next().unwrap_or("?").to_string()
}

/// Teardown (the UI hooks going down; a FreeLibrary only before the hook was installed: once installed the DLL is
/// pinned, [`pin_self`]): recording off first, then the hook disabled (passthrough; the trampoline is kept: a thread
/// preempted inside the detour may still return through it). No logging (loader lock).
pub fn teardown() {
    mw2er_render::er_depth::ON.store(false, Ordering::SeqCst);
    WATCH.store(0, Ordering::SeqCst);
    WATCH_B.store(0, Ordering::SeqCst);
    LIVE.store(false, Ordering::SeqCst);
    HOOK_ON.store(false, Ordering::SeqCst);
    let t = HOOK_TARGET.load(Ordering::SeqCst);
    if t != 0 {
        // SAFETY: a target we hooked (MinHook validates it; a second disable is MH_ERROR_DISABLED).
        let _ = unsafe { MH_DisableHook(t as *mut c_void) };
    }
}

// ---- UiBegin (ER's render thread) ---------------------------------------------------------------------------------------

/// UiBegin's detour (after the original, `er_depth::on_ui_begin`): the frame facts, the watched resources and the
/// copy / sample record. `c` = this frame's candidate-A chain, `b` candidate B's dsb chain (`S[0]+0xE8`), `proven_vt` the
/// vtables the task found in the D3D12 module (A, B).
pub fn on_ui_begin(f: &UiFacts, c: &SceneChain, b: &DsbChain, proven_vt: [usize; 2], base: usize, sf: &ScaleformUi) {
    if !HOOK_ON.load(Ordering::Relaxed) {
        return;
    }
    let u = &sf.ui41;
    // SAFETY (reads below): non-faulting reads of ER's render global and its render-thread object.
    let g = unsafe { read::<usize>(base + sf.g_render) }.unwrap_or(0);
    let mt = g != 0 && unsafe { read::<u8>(g + u.g_mt) }.unwrap_or(0) != 0;
    let rto = if g != 0 { unsafe { read::<usize>(g + u.g_rt) }.unwrap_or(0) } else { 0 };
    let rt_tid = if rto != 0 { unsafe { read::<u32>(rto + u.rt_tid) }.unwrap_or(0) } else { 0 };
    let armed = f.hal != 0 && f.movies > 0 && f.list != 0;
    let not_ahead = !mw2er_core::c4::ahead41(LAST_BEGIN_SEQ.swap(f.seq, Ordering::SeqCst), f.seq);
    if armed {
        ARMED_LIST.store(f.list, Ordering::SeqCst);
        ARMED_QUEUE.store(f.queue, Ordering::SeqCst);
        ARMED_TID.store(f.tid, Ordering::SeqCst);
        RT_TID.store(rt_tid, Ordering::SeqCst);
        ARMED_SEQ.store(f.seq, Ordering::SeqCst);
    }
    // the watched resources: this frame's pinned chain + the proven D3D12 vtable, the same resource 2 frames in a row
    let cand = [(c.ok(), c.a.res), (b.ok(), b.res)];
    let mut res_vt = [0usize; 2];
    let mut chain_ok = [false; 2];
    let mut stable = [false; 2];
    for k in 0..2 {
        let (ok, res) = cand[k];
        // SAFETY: non-faulting read of the resource's vtable pointer.
        res_vt[k] = if ok { unsafe { read::<usize>(res) }.unwrap_or(0) } else { 0 };
        PROVEN_VT[k].store(proven_vt[k], Ordering::Relaxed);
        chain_ok[k] = ok && proven_vt[k] != 0 && res_vt[k] == proven_vt[k];
        if !chain_ok[k] {
            continue;
        }
        let n = if PREV_RES[k].swap(res, Ordering::Relaxed) == res { STABLE_N[k].fetch_add(1, Ordering::Relaxed) + 1 } else {
            STABLE_N[k].store(1, Ordering::Relaxed);
            1
        };
        stable[k] = n >= 2;
        let w = watch(k).load(Ordering::Relaxed);
        if stable[k] && w != res {
            EPOCH_MS[k].store(shared::now_ms_if_started(), Ordering::SeqCst);
            EPOCH[k].fetch_add(1, Ordering::SeqCst);
            watch(k).store(res, Ordering::SeqCst);
            if w != 0 {
                WATCH_CHANGES[k].fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    // the copy / sample
    let live = LIVE.load(Ordering::Relaxed) && !TRIPPED.load(Ordering::Relaxed) && LIVE_CFG_NOW.load(Ordering::Relaxed);
    let sample_due = SAMPLE_DUE.load(Ordering::Relaxed) && SAMPLE_VALUE.load(Ordering::Relaxed) == 0 && !FAILED.load(Ordering::Relaxed);
    if !(live || sample_due) || !READER_READY.load(Ordering::Relaxed) {
        return;
    }
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    // C6 round 6: the candidate: the live one, else the one the fold found readable at the last armed UiBegin
    let watched = [0, 1].map(|k| watch(k).load(Ordering::Relaxed) != 0);
    let Some(k) = eds::pick_cand(live.then(|| LIVE_CAND.load(Ordering::Relaxed) as usize), [0, 1].map(|k| FOLDS[k].last_at_ui()), watched) else {
        miss(CopyMiss::State);
        return;
    };
    let w = watch(k).load(Ordering::Relaxed);
    let res_addr = cand[k].1;
    // an armed list of an older frame never spliced: its list never ran
    let cur = CUR.load(Ordering::SeqCst);
    if let Some((s, _, _, _)) = armed_unpack(cur)
        && s != f.seq
        && CUR.compare_exchange(cur, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    {
        miss(CopyMiss::Busy);
    }
    let idle = CUR.load(Ordering::SeqCst) == 0;
    let Some(mut rg) = try_lock(&READER) else {
        LOCKED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Some(r) = rg.as_mut() else { return };
    let slot = (f.seq % SLOTS as u64) as usize;
    let slot_free = r.completed() >= SLOT_VALUE[slot].load(Ordering::SeqCst);
    let want_fresh = shared::now_ms_if_started().saturating_sub(WANT_MS.load(Ordering::Relaxed)) <= WANT_FRESH_MS;
    // one GetDesc per frame, only when the rows before it can pass
    let desc = if chain_ok[k] && stable[k] && idle && res_addr == w { ui_probe::desc_if_vtable(res_addr, res_vt[k]) } else { None };
    let bb = ui_probe::BB_SIZE.load(Ordering::Relaxed);
    let rs = READER_SIZE.load(Ordering::Relaxed);
    let g = CopyIn {
        live,
        sample_due,
        want: want_fresh.then(|| Want::from_code(WANT.load(Ordering::Relaxed))),
        mt,
        tid: f.tid,
        hal_tid: f.rtid,
        rt_tid,
        queue_ok: f.queue != 0 && f.queue == ui_probe::PRESENT_QUEUE.load(Ordering::Relaxed),
        armed,
        not_ahead,
        chain_ok: chain_ok[k],
        res: res_addr,
        watch: w,
        stable: stable[k],
        desc,
        bb: ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32),
        ready: true,
        reader_size: (rs as u32, (rs >> 32) as u32),
        slot_free,
        idle,
    };
    // the `desc` trip: the same resource with another desc (ER resized it in place: never trusted again)
    if let Some(d) = desc {
        let key = d.width | (d.height as u64) << 32 ^ (d.format as u64) << 56;
        if DESC_RES.swap(res_addr, Ordering::Relaxed) == res_addr {
            if DESC_KEY.swap(key, Ordering::Relaxed) != key && LIVE.load(Ordering::Relaxed) {
                trip("desc");
                return;
            }
        } else {
            DESC_KEY.store(key, Ordering::Relaxed);
        }
    }
    let what = match eds::copy_gate(&g) {
        Ok(w) => w,
        Err(m) => {
            miss(m);
            return;
        }
    };
    let raw = res_addr as *mut c_void;
    // SAFETY: ER's scene depth candidate, validated this UiBegin (its pinned chain, the proven D3D12 vtable, a fresh
    // GetDesc) and used by ER's own lists of this frame; borrowed (no AddRef / Release), used only for the SRV write.
    let Some(res) = (unsafe { ID3D12Resource::from_raw_borrowed(&raw) }) else { return };
    let pts = if what.sample {
        let ch = CHEST_PX.load(Ordering::Relaxed);
        let chest = (ch != u64::MAX).then_some((ch as u32, (ch >> 32) as u32));
        SAMPLE_CHEST.store(ch, Ordering::Relaxed);
        SAMPLE_CHEST_Z.store(CHEST_Z.load(Ordering::Relaxed), Ordering::Relaxed);
        SAMPLE_TURN.store(TURN.load(Ordering::Relaxed), Ordering::Relaxed);
        SAMPLE_MOVE.store(MOVE.load(Ordering::Relaxed), Ordering::Relaxed);
        Some(eds::sample_points(g.bb.0, g.bb.1, chest))
    } else {
        None
    };
    match r.record(slot, res, what.copy, pts.as_ref()) {
        Ok(()) => {
            let v = armed_pack(f.seq, slot, what.copy, what.sample) | if k == 1 { ARMED_CAND_B } else { 0 };
            let _ = CUR.compare_exchange(0, v, Ordering::SeqCst, Ordering::SeqCst);
        }
        Err(e) => {
            drop(rg);
            fail_err(e);
        }
    }
}

/// UiEnd's detour (after the original): a copy / sample still armed this UI frame never ran.
pub fn on_ui_end(seq: u64) {
    if !HOOK_ON.load(Ordering::Relaxed) {
        return;
    }
    let cur = CUR.load(Ordering::SeqCst);
    if let Some((s, _, _, _)) = armed_unpack(cur)
        && s == seq
        && CUR.compare_exchange(cur, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    {
        miss(CopyMiss::Busy);
    }
}

// ---- the ECL callback (any thread) --------------------------------------------------------------------------------------

/// `[[list] + 26 * 8]` (non-faulting).
fn vt_slot26(list: usize) -> Option<usize> {
    let vt = rd_u64(list)? as usize;
    rd_u64(vt + 8 * SLOT_RESOURCE_BARRIER).map(|x| x as usize)
}

/// hudhook's ECL callback, before any splice: the fold of this call for each watched candidate (atomics only).
pub fn fold_ecl(queue: usize, lists: &[usize], seq: u64) {
    if !HOOK_ON.load(Ordering::Relaxed) {
        return;
    }
    let armed_q = ARMED_QUEUE.load(Ordering::SeqCst);
    let armed = seq == ARMED_SEQ.load(Ordering::SeqCst) && queue == armed_q && lists.first().copied() == Some(ARMED_LIST.load(Ordering::SeqCst));
    // SAFETY: GetCurrentThreadId has no preconditions.
    let tid = unsafe { GetCurrentThreadId() };
    let now_ms = shared::now_ms_if_started();
    for k in 0..2 {
        let w = watch(k).load(Ordering::Relaxed);
        if w == 0 {
            continue;
        }
        let i = EclIn {
            queue,
            tid,
            lists,
            watch: w,
            epoch: EPOCH[k].load(Ordering::SeqCst),
            armed,
            seq,
            judged: now_ms.saturating_sub(EPOCH_MS[k].load(Ordering::Relaxed)) >= (eds::WARMUP_S * 1000.0) as u64,
            hal_queue: armed_q,
            rt_tid: RT_TID.load(Ordering::Relaxed),
            target: HOOK_TARGET.load(Ordering::Relaxed),
        };
        let _ = FOLDS[k].ecl(table(k), &i, &vt_slot26);
    }
}

/// hudhook's ECL callback, when the injection did not splice this call: our armed copy / sample at index 0 of the armed
/// call, only when the fold proved plane 0 readable there; our fence Signal follows (patch 0002).
pub fn on_ecl(queue: usize, lists: &[usize], seq: u64) -> Option<Splice> {
    if !HOOK_ON.load(Ordering::Relaxed) || !READER_READY.load(Ordering::Relaxed) {
        return None;
    }
    let cur = CUR.load(Ordering::SeqCst);
    let (aseq, slot, copy, sample) = armed_unpack(cur)?;
    // SAFETY: GetCurrentThreadId has no preconditions.
    if queue != ARMED_QUEUE.load(Ordering::SeqCst) || unsafe { GetCurrentThreadId() } != ARMED_TID.load(Ordering::SeqCst) {
        return None;
    }
    let idx = mw2er_core::c4::find_armed(lists, ARMED_LIST.load(Ordering::SeqCst))?;
    let cancel = |m: CopyMiss| {
        if CUR.compare_exchange(cur, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            miss(m);
        }
    };
    if idx != 0 {
        cancel(CopyMiss::NoFrame);
        return None;
    }
    if aseq != seq {
        cancel(CopyMiss::Ahead);
        return None;
    }
    let k = armed_cand(cur);
    let at = FOLDS[k].at_ui(seq);
    if eds::splice_ok(at).is_err() {
        cancel(CopyMiss::State);
        return None;
    }
    let s = splice_of(slot)?;
    if CUR.compare_exchange(cur, 0, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return None;
    }
    SLOT_VALUE[slot].store(s.value, Ordering::SeqCst);
    if copy {
        LAST_COPY_SLOT.store(slot, Ordering::SeqCst);
        LAST_COPY_FAILS.store(SPLICE_SIGNAL_FAILS.load(Ordering::SeqCst), Ordering::SeqCst);
        LAST_COPY_SEQ.store(seq, Ordering::SeqCst);
        COPIES.fetch_add(1, Ordering::Relaxed);
    }
    if sample {
        SAMPLE_CAND.store(k as u8, Ordering::SeqCst);
        SAMPLE_STATE.store(at.unwrap_or(eds::UNKNOWN), Ordering::SeqCst);
        SAMPLE_VALUE.store(s.value, Ordering::SeqCst);
        SAMPLES_SPLICED.fetch_add(1, Ordering::Relaxed);
    }
    Some(s)
}

/// The splice of `slot`'s list (our list and fence, kept alive by READER for the session; borrowed, then cloned (AddRef)
/// into the splice hudhook executes and releases).
fn splice_of(slot: usize) -> Option<Splice> {
    let (lraw, fraw) = (LIST_RAW[slot].load(Ordering::SeqCst) as *mut c_void, FENCE_RAW.load(Ordering::SeqCst) as *mut c_void);
    // SAFETY: our own list (a graphics list IS a command list) and fence, never released in-session.
    let list = unsafe { ID3D12CommandList::from_raw_borrowed(&lraw) }?.clone();
    // SAFETY: as above.
    let fence = unsafe { ID3D12Fence::from_raw_borrowed(&fraw) }?.clone();
    let value = NEXT_VALUE.fetch_add(1, Ordering::SeqCst) + 1;
    Some(Splice { at: 0, list, fence, value })
}

// ---- Present (hudhook's thread) -------------------------------------------------------------------------------------------

/// Present (`viewmodel::callback_body`, every frame before the world guns): what this frame wants of ER's depth, for the
/// copy gate on ER's render thread (first person / no world gun / a gun). Atomics only.
pub fn note_want(fp_on: bool, world_gun: bool) {
    if !CFG_LIVE.load(Ordering::Relaxed) {
        return;
    }
    let w = if fp_on {
        Want::Fp
    } else if world_gun {
        Want::Gun
    } else {
        Want::NoGun
    };
    WANT.store(w as u8, Ordering::Relaxed);
    WANT_MS.store(shared::now_ms_if_started(), Ordering::Relaxed);
}

/// What the prime of this frame reads (ER's camera, the bias) and the slot of its copy.
pub struct PrimeTicket {
    pub slot: usize,
    pub near: f32,
    pub far: f32,
    pub bias_m: f32,
    pub bias_rel: f32,
}

/// Present's world-gun pass (`viewmodel::draw_world_guns`): publishes what this frame wants (the copy gate's `want`),
/// then decides the prime: live, third person, this Present's copy spliced, not turning fast, ER's near known.
pub fn present_prime(snap: &shared::HudSnapshot, cfg: &mw2er_core::config::Config) -> Option<PrimeTicket> {
    if !CFG_LIVE.load(Ordering::Relaxed) {
        return None;
    }
    let fwd = snap.cam_rows[2];
    let prev = [0, 1, 2].map(|k| f32::from_bits(LAST_FWD[k].swap(fwd[k].to_bits(), Ordering::Relaxed)));
    if !LIVE.load(Ordering::Relaxed) || TRIPPED.load(Ordering::Relaxed) || !cfg.coop.world_gun_er_depth {
        return None;
    }
    if snap.fp_on {
        miss(CopyMiss::Fp);
        return None;
    }
    if eds::turn_deg(prev, fwd) > cfg.coop.er_depth_max_turn_deg {
        miss(CopyMiss::Turn);
        return None;
    }
    let seq = PRESENT_SEQ.load(Ordering::SeqCst);
    let cs = LAST_COPY_SEQ.load(Ordering::SeqCst);
    if !eds::prime_pair((cs != u64::MAX).then_some(cs), seq, SPLICE_SIGNAL_FAILS.load(Ordering::SeqCst), LAST_COPY_FAILS.load(Ordering::SeqCst)) {
        miss(CopyMiss::Pair);
        return None;
    }
    if !(snap.er_near > 0.0) {
        miss(CopyMiss::Desc);
        return None;
    }
    // C6 part 16.8: the self check's effective near (n_eff = near_cam * near_rel, >= near_cam: ER's surfaces only ever
    // pushed back) and its bias addition, never CSCam's near alone (the 23:41 run: ~10 % short = phantom occlusion)
    let k = LIVE_CAND.load(Ordering::Relaxed) as usize & 1;
    Some(PrimeTicket {
        slot: LAST_COPY_SLOT.load(Ordering::SeqCst),
        near: snap.er_near * near_rel(k),
        far: if INF_FAR.load(Ordering::Relaxed) { 0.0 } else { snap.er_far },
        bias_m: cfg.coop.er_depth_bias_m + f32::from_bits(BIAS_ADD[k].load(Ordering::Relaxed)).max(0.0),
        bias_rel: cfg.coop.er_depth_bias_rel,
    })
}

/// C6 round 7: at most this many `DEPTH probe self_s` lines per session.
const SELF_S_MAX: u32 = 32;

/// The self check's far model (true = infinite: the prime ignores CSCam's far).
static INF_FAR: AtomicBool = AtomicBool::new(true);

/// Present: the reader for the prime's texture (try_lock: a contended frame draws with the capsules only).
pub fn reader() -> Option<std::sync::MutexGuard<'static, Option<DepthReader>>> {
    let g = try_lock(&READER);
    if g.is_none() {
        LOCKED.fetch_add(1, Ordering::Relaxed);
    }
    g
}

/// Present, after the record: the prime was recorded this frame.
pub fn note_primed(primed: bool, t: &PrimeTicket) {
    if primed && PRIMED.fetch_add(1, Ordering::Relaxed) == 0 {
        log_i!("DEPTH", "live first prime slot={} near={:.4} far={:.1} bias={:.3}+{:.4}z (ER's depth in our world-gun depth, before the capsules)", t.slot, t.near, t.far, t.bias_m, t.bias_rel);
    }
}

/// The prime's input from a ticket and the reader.
pub fn prime_in<'a>(t: &PrimeTicket, r: &'a DepthReader) -> Option<PrimeIn<'a>> {
    Some(PrimeIn { tex: r.texture(t.slot)?, near: t.near, far: t.far, bias_m: t.bias_m, bias_rel: t.bias_rel })
}

// ---- the task thread ------------------------------------------------------------------------------------------------------

/// Task thread (the HUD snapshot): probe P-S2's chest pixel (our Spine2 bone through ER's camera at our back-buffer
/// size) and the camera turn of this tick.
pub fn publish_chest(chest: Option<[f32; 3]>, cam_pos: [f32; 3], rows: [[f32; 3]; 3], fov_raw: f32, fallback_deg: f32) {
    if !CFG_SAMPLE.load(Ordering::Relaxed) {
        return;
    }
    let fwd = rows[2];
    let prev = [0, 1, 2].map(|k| f32::from_bits(LAST_TASK_FWD[k].swap(fwd[k].to_bits(), Ordering::Relaxed)));
    TURN.store(eds::turn_deg(prev, fwd).to_bits(), Ordering::Relaxed);
    // C6 round 7: the camera's move this tick (a moving camera is not "still": the snapshot vs ER's frame)
    let pp = [0, 1, 2].map(|k| f32::from_bits(LAST_TASK_POS[k].swap(cam_pos[k].to_bits(), Ordering::Relaxed)));
    let mv = (0..3).map(|k| (cam_pos[k] - pp[k]).powi(2)).sum::<f32>().sqrt();
    MOVE.store(if mv.is_finite() { mv } else { f32::MAX }.to_bits(), Ordering::Relaxed);
    let bb = ui_probe::BB_SIZE.load(Ordering::Relaxed);
    let (w, h) = ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32);
    let (vf, _) = mw2er_core::hud_geom::vfov_rad(fov_raw, fallback_deg);
    let px = chest.zip(mw2er_core::coop_vis::world_view_proj(cam_pos, rows, vf, w.max(1), h.max(1), eds::OUR_NEAR, eds::OUR_FAR)).and_then(|(c, (view, vp))| {
        let clip = [0, 1, 2, 3].map(|r| vp[r][0] * c[0] + vp[r][1] * c[1] + vp[r][2] * c[2] + vp[r][3]);
        let z = view[2][0] * c[0] + view[2][1] * c[1] + view[2][2] * c[2] + view[2][3];
        if !(clip[3] > 0.1 && z > 0.3) {
            return None;
        }
        let (nx, ny) = (clip[0] / clip[3], clip[1] / clip[3]);
        if !(nx.abs() < 0.98 && ny.abs() < 0.98) {
            return None;
        }
        let x = ((nx * 0.5 + 0.5) * w as f32) as u32;
        let y = ((0.5 - ny * 0.5) * h as f32) as u32;
        Some((x, y, z))
    });
    match px {
        Some((x, y, z)) if w > 0 => {
            CHEST_Z.store(z.to_bits(), Ordering::Relaxed);
            CHEST_PX.store(x as u64 | (y as u64) << 32, Ordering::Relaxed);
        }
        _ => CHEST_PX.store(u64::MAX, Ordering::Relaxed),
    }
}

/// Task thread: the hook install, the reader worker, the lines, the verdict, the sample, the live gate and the trips.
#[derive(Default)]
pub struct StateLog {
    t_line: f64,
    t_verdict: Option<f64>,
    t_sample: f64,
    t_live: f64,
    paths_logged: [Vec<Vec<u64>>; 2],
    path_gen: [u64; 2],
    /// C6 round 6: per candidate (A, B): the verdict, the sample's convention, the self tally / pass.
    verdict: [Option<bool>; 2],
    samples: u32,
    conv: [Option<Conv>; 2],
    tally: [SelfTally; 2],
    self_ok: [Option<bool>; 2],
    t_census: Option<f64>,
    sample_t0: Option<f64>,
    sample_fails_base: u64,
    /// C6 round 7: `DEPTH probe self_s` lines so far (the first SELF_S_MAX still samples).
    self_s_n: u32,
    off_logged: bool,
    live_logged: Option<&'static str>,
    at_live: Option<StateStats>,
    trip_logged: bool,
    paused: bool,
    reader_size: (u32, u32),
    retired: Vec<DepthReader>,
}

impl StateLog {
    pub fn tick(&mut self, now: f64, in_world: bool, cfg: &mw2er_core::config::Config) {
        if !(CFG_STATE.load(Ordering::Relaxed) || CFG_LIVE.load(Ordering::Relaxed)) {
            return;
        }
        // hot off of the prime (cold on: the merge refuses a hot false -> true)
        if !cfg.coop.world_gun_er_depth && LIVE_CFG_NOW.swap(false, Ordering::Relaxed) && CFG_LIVE.load(Ordering::Relaxed) {
            LIVE.store(false, Ordering::SeqCst);
            log_i!("DEPTH", "live off why=config (world_gun_er_depth = false; capsules only for the session)");
        }
        self.try_install();
        if !HOOK_ON.load(Ordering::Relaxed) {
            // no probe: the live gate says why the prime stays off (once)
            self.live_gate(now);
            return;
        }
        // the UI hooks went down (hot inject off / a C4 trip): the fold no longer runs, so neither does the recording
        if !ui_probe::ACTIVE.load(Ordering::Relaxed) {
            if !self.paused {
                self.paused = true;
                teardown();
                log_i!("DEPTH", "state hook disabled why=ui_hooks (UiBegin / ECL callback passthrough; hook disabled, trampoline kept; capsules only)");
            }
            return;
        }
        self.reader_worker();
        self.check_trips();
        if !in_world {
            return;
        }
        if now - self.t_line >= 5.0 {
            self.t_line = now;
            for k in 0..2 {
                let w = watch(k).load(Ordering::Relaxed);
                if k == 0 || w != 0 {
                    let s = FOLDS[k].stats(table(k));
                    log_i!("DEPTH", "{} cand={}", eds::state_text_line(w, FOLDS[k].q()[0], &s), CAND_TAGS[k]);
                }
                self.paths(k);
            }
            let r = &mw2er_render::er_depth::BARS;
            log_i!(
                "DEPTH",
                "{}",
                eds::control_line(&ctl(), r.load(Ordering::Relaxed), mw2er_render::er_depth::TRANS.load(Ordering::Relaxed), CENSUS.full.load(Ordering::Relaxed))
            );
            if CFG_LIVE.load(Ordering::Relaxed) {
                let m: Vec<(CopyMiss, u64)> = CopyMiss::ALL.iter().map(|k| (*k, MISS[k.index()].load(Ordering::Relaxed))).collect();
                log_i!("DEPTH", "{} attempts={} locked={} state_at_ui={} live={} tripped={}", eds::live_line(ATTEMPTS.load(Ordering::Relaxed), COPIES.load(Ordering::Relaxed), PRIMED.load(Ordering::Relaxed), &m), ATTEMPTS.load(Ordering::Relaxed), LOCKED.load(Ordering::Relaxed), if LIVE.load(Ordering::Relaxed) { eds::state_text(FOLDS[LIVE_CAND.load(Ordering::Relaxed) as usize & 1].at_ui(ARMED_SEQ.load(Ordering::Relaxed)).unwrap_or(eds::UNKNOWN)) } else { "-".to_string() }, LIVE.load(Ordering::Relaxed) as u8, TRIPPED.load(Ordering::Relaxed) as u8);
            }
        }
        // C6 round 6: the census (first at 10 s, then with every verdict) names every resource with depth traffic
        let tc = *self.t_census.get_or_insert(now + 10.0);
        if now >= tc {
            self.t_census = Some(now + 60.0);
            self.census();
        }
        // the per-candidate verdict (60 s): judged once the hook is on (a dead hook = `why=hook`, not a silent `unknown`)
        let tv = *self.t_verdict.get_or_insert(now + 60.0);
        if now >= tv {
            self.t_verdict = Some(now + 60.0);
            let c = ctl();
            for k in 0..2 {
                let s = FOLDS[k].stats(table(k));
                let (ready, why) = eds::verdict_cand(&s, &c);
                self.verdict[k] = Some(ready);
                log_i!("DEPTH", "{}", eds::cand_verdict_line(k, &s, &c));
                if LIVE.load(Ordering::Relaxed) && LIVE_CAND.load(Ordering::Relaxed) as usize == k && !ready {
                    trip("state");
                    log_w!("DEPTH", "live trip why=verdict ({why} cand={}); capsules only for the session", CAND_TAGS[k]);
                    self.trip_logged = true;
                }
            }
        }
        self.sample(now);
        self.live_gate(now);
    }

    fn try_install(&mut self) {
        if HOOK_TRIED.load(Ordering::Relaxed) || !CFG_STATE.load(Ordering::Relaxed) {
            return;
        }
        if ui_probe::CL_VTABLE.load(Ordering::Relaxed) == 0 || ui_probe::BB_VTABLE.load(Ordering::Relaxed) == 0 {
            return;
        }
        HOOK_TRIED.store(true, Ordering::SeqCst);
        match install_hook() {
            Ok((t, m)) => {
                HOOK_ON.store(true, Ordering::SeqCst);
                log_i!(
                    "DEPTH",
                    "state hook on target=0x{t:x} module={m} slot={SLOT_RESOURCE_BARRIER} (read-only: transitions on ER's scene depth counted; original first) sample={} er_depth={}",
                    CFG_SAMPLE.load(Ordering::Relaxed) as u8,
                    CFG_LIVE.load(Ordering::Relaxed) as u8
                );
            }
            Err(why) => log_w!("DEPTH", "state hook off why={why} (nothing hooked; world guns keep the capsules)"),
        }
    }

    /// The reader (copy textures, lists, PSOs) on a worker, once the renderer exists and the back-buffer size is known;
    /// a new size re-prepares (the old reader is retired, never freed in-session).
    fn reader_worker(&mut self) {
        if !(CFG_SAMPLE.load(Ordering::Relaxed) || CFG_LIVE.load(Ordering::Relaxed)) || FAILED.load(Ordering::Relaxed) {
            return;
        }
        if !shared::VM_READY.load(Ordering::Acquire) {
            return;
        }
        let bb = ui_probe::BB_SIZE.load(Ordering::Relaxed);
        let size = ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32);
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if READER_STARTED.load(Ordering::SeqCst) && (self.reader_size == size || !READER_READY.load(Ordering::SeqCst)) {
            return;
        }
        if READER_READY.load(Ordering::SeqCst) {
            // a resize: the old reader stays alive (lists / textures possibly in flight), the new one is made below
            READER_READY.store(false, Ordering::SeqCst);
            if let Some(old) = lock(&READER).take() {
                self.retired.push(old);
            }
            log_i!("DEPTH", "reader resize {}x{} -> {}x{} (re-prepared on a worker; misses `pso` meanwhile)", self.reader_size.0, self.reader_size.1, size.0, size.1);
        }
        READER_STARTED.store(true, Ordering::SeqCst);
        self.reader_size = size;
        let r = std::thread::Builder::new().name("mw2er-erdepth".into()).spawn(move || {
            crate::log::set_thread_role("erdepth");
            let made = crate::guard::guard("erdepth.reader", || {
                let dev = crate::viewmodel::renderer_device().ok_or_else(|| "no renderer".to_string())?;
                // review 2026-10-21: the new fence starts at the last value handed out, so every SLOT_VALUE / a pending
                // SAMPLE_VALUE of the retired reader is already "completed" on it (values stay monotonic across readers)
                DepthReader::new(&dev, size.0, size.1, NEXT_VALUE.load(Ordering::SeqCst)).map_err(|e| e.to_string())
            });
            match made {
                Some(Ok(rd)) => {
                    // review 2026-10-21: a copy / sample armed on the retired reader is cancelled BEFORE the new lists are
                    // published (an on_ecl that read a new LIST_RAW then fails its CUR compare-exchange: never spliced
                    // against the old record); its pending sample is dropped (re-requested by the 5 s tick)
                    if CUR.swap(0, Ordering::SeqCst) != 0 {
                        miss(CopyMiss::Busy);
                    }
                    SAMPLE_VALUE.store(0, Ordering::SeqCst);
                    SAMPLE_DUE.store(false, Ordering::SeqCst);
                    for (k, a) in LIST_RAW.iter().enumerate() {
                        a.store(rd.list_raw(k), Ordering::SeqCst);
                    }
                    FENCE_RAW.store(rd.fence_raw(), Ordering::SeqCst);
                    let dev = rd.device_raw();
                    // a copy of the old reader is never paired with the new one's textures
                    LAST_COPY_SEQ.store(u64::MAX, Ordering::SeqCst);
                    *lock(&READER) = Some(rd);
                    READER_SIZE.store(size.0 as u64 | (size.1 as u64) << 32, Ordering::SeqCst);
                    READER_READY.store(true, Ordering::SeqCst);
                    log_i!(
                        "DEPTH",
                        "reader ready device=0x{dev:x} size={}x{} copy={}x{} slots={SLOTS} (our compute lists: SRV plane 0 of ER's depth, no barrier on it)",
                        size.0,
                        size.1,
                        eds::half(size.0, size.1).0,
                        eds::half(size.0, size.1).1
                    );
                }
                Some(Err(e)) => {
                    log_e!("DEPTH", "reader create failed: {e} (capsules only)");
                    FAILED.store(true, Ordering::SeqCst);
                }
                None => FAILED.store(true, Ordering::SeqCst),
            }
        });
        if let Err(e) = r {
            log_e!("DEPTH", "reader create failed (thread: {e})");
            FAILED.store(true, Ordering::SeqCst);
        }
    }

    /// `DEPTH state path ... cand=..` for each distinct frame path of candidate `k` (at most 20 each).
    fn paths(&mut self, k: usize) {
        let (g, seq, p, at) = FOLDS[k].last_path();
        if g == self.path_gen[k] || g % 2 == 1 || self.paths_logged[k].len() >= 20 {
            return;
        }
        let (g2, _, _, _) = FOLDS[k].last_path();
        if g2 != g {
            return;
        }
        self.path_gen[k] = g;
        let mut key = p.clone();
        key.push(at as u64);
        if !self.paths_logged[k].contains(&key) {
            self.paths_logged[k].push(key);
            log_i!("DEPTH", "{} cand={}", eds::path_line(seq, &p, at), CAND_TAGS[k]);
        }
    }

    /// C6 round 6: `DEPTH census ...` for the top resources with depth-state traffic (task thread; `vt_ok` = a
    /// non-faulting read of the resource's first qword equals a candidate's proven D3D12 resource vtable; no GetDesc
    /// here: a resource outside both pinned chains is never queried).
    fn census(&mut self) {
        let rows = CENSUS.rows();
        let (wa, wb) = (WATCH.load(Ordering::Relaxed), WATCH_B.load(Ordering::Relaxed));
        let pv = [0, 1].map(|k| PROVEN_VT[k].load(Ordering::Relaxed));
        if rows.is_empty() {
            log_i!(
                "DEPTH",
                "census empty calls={} trans={} (no depth-state transition seen by the hook)",
                mw2er_render::er_depth::CALLS.load(Ordering::Relaxed),
                mw2er_render::er_depth::TRANS.load(Ordering::Relaxed)
            );
        }
        for r in rows.iter().take(eds::CENSUS_TOP) {
            let is = if r.res == wa && wa != 0 {
                "A"
            } else if r.res == wb && wb != 0 {
                "B"
            } else {
                "-"
            };
            let vt = rd_u64(r.res).unwrap_or(0) as usize;
            let vt_ok = vt != 0 && pv.contains(&vt);
            log_i!("DEPTH", "{}", eds::census_line(r, is, vt_ok));
        }
        if rows.len() > eds::CENSUS_TOP {
            log_i!("DEPTH", "census more={} full={}", rows.len() - eds::CENSUS_TOP, CENSUS.full.load(Ordering::Relaxed));
        }
    }

    /// P-S2: the 5 s request, the readback after the fence, the sample and self lines.
    fn sample(&mut self, now: f64) {
        if !CFG_SAMPLE.load(Ordering::Relaxed) || !READER_READY.load(Ordering::Relaxed) {
            return;
        }
        if FAILED.load(Ordering::Relaxed) {
            if !self.off_logged {
                self.off_logged = true;
                let err = try_lock(&ERR).and_then(|g| g.as_ref().map(|e| e.to_string())).unwrap_or_default();
                log_e!("DEPTH", "probe sample off why=hresult (samples={} err={err})", self.samples);
            }
            return;
        }
        let v = SAMPLE_VALUE.load(Ordering::SeqCst);
        if v == 0 {
            self.sample_t0 = None;
            self.sample_fails_base = SPLICE_SIGNAL_FAILS.load(Ordering::SeqCst);
            if now - self.t_sample >= 5.0 {
                self.t_sample = now;
                SAMPLE_DUE.store(true, Ordering::Relaxed);
            }
            return;
        }
        SAMPLE_DUE.store(false, Ordering::Relaxed);
        let t0 = *self.sample_t0.get_or_insert(now);
        if SPLICE_SIGNAL_FAILS.load(Ordering::SeqCst) > self.sample_fails_base {
            fail_err(VmError::D3d { call: "Signal(er depth, hudhook splice)", hr: 0 });
            return;
        }
        let Some(g) = try_lock(&READER) else { return };
        let Some(r) = g.as_ref() else { return };
        if let Some(hr) = r.device_removed() {
            drop(g);
            fail_err(VmError::D3d { call: "GetDeviceRemovedReason(er depth)", hr });
            return;
        }
        if r.completed() < v {
            drop(g);
            if now - t0 > 10.0 {
                fail_err(VmError::D3d { call: "fence(er depth sample) never passed", hr: 0 });
            }
            return;
        }
        let px = r.read_sample();
        drop(g);
        SAMPLE_VALUE.store(0, Ordering::SeqCst);
        self.samples += 1;
        let mut grid = [0f32; 16];
        grid.copy_from_slice(&px[..16]);
        let chest_ok = SAMPLE_CHEST.load(Ordering::Relaxed) != u64::MAX;
        let chest_z = f32::from_bits(SAMPLE_CHEST_Z.load(Ordering::Relaxed));
        let near = NEAR_CAM.load(Ordering::Relaxed);
        let near = f32::from_bits(near);
        // C6 round 7: the chest is the median of its 5-texel cross
        let mut d5 = [0f32; eds::CHEST_N];
        d5.copy_from_slice(&px[eds::CHEST_AT..eds::CHEST_AT + eds::CHEST_N]);
        let d_med = eds::chest_median(&d5, near, None).map_or(px[eds::CHEST_AT], |c| c.0);
        let conv = eds::conv_of(&grid, d_med, chest_ok.then_some(chest_z - eds::CHEST_FRONT_M), near);
        // C6 round 6: the sample's candidate (the one the fold found readable)
        let k = SAMPLE_CAND.load(Ordering::SeqCst) as usize & 1;
        if conv != Conv::Unknown {
            self.conv[k] = Some(conv);
        }
        log_i!(
            "DEPTH",
            "{} chest={} d_chest={:.6} d_med={d_med:.6} cand={}",
            eds::sample_line(self.samples, conv, &grid, px[16], SAMPLE_STATE.load(Ordering::Relaxed)),
            chest_ok as u8,
            px[eds::CHEST_AT],
            CAND_TAGS[k]
        );
        if chest_ok {
            let (turn, mv) = (f32::from_bits(SAMPLE_TURN.load(Ordering::Relaxed)), f32::from_bits(SAMPLE_MOVE.load(Ordering::Relaxed)));
            self.tally[k].add5(d5, chest_z, turn, mv);
            let far = f32::from_bits(FAR_CAM.load(Ordering::Relaxed));
            let o = self.tally[k].result(near, far);
            let live_k = LIVE.load(Ordering::Relaxed) && LIVE_CAND.load(Ordering::Relaxed) as usize == k;
            if !LIVE.load(Ordering::Relaxed) || live_k {
                INF_FAR.store(o.far.is_none(), Ordering::Relaxed);
            }
            // C6 round 7 (depth7.md 1): the fitted offset + residuals + edges + the scale (proven, or unproven with the
            // margin the prime then uses); the margin never shrinks while this candidate is live
            let pass = eds::self_pass(&o);
            self.self_ok[k] = Some(pass);
            // C6 part 16.8 step 4: while live, the proven slope's low end beating the live near by > 5 % (or offset /
            // spread over 12 judged) trips; a slope ~1.10 the live near already carries does not
            if live_k && eds::self_trips(&o, near_rel(k)) {
                trip("self");
            }
            if pass {
                let (nr, ba) = (o.near_rel.max(1.0), o.bias_add_m.max(0.0));
                if live_k {
                    NEAR_REL[k].fetch_max(nr.to_bits(), Ordering::Relaxed);
                    BIAS_ADD[k].fetch_max(ba.to_bits(), Ordering::Relaxed);
                } else {
                    NEAR_REL[k].store(nr.to_bits(), Ordering::Relaxed);
                    BIAS_ADD[k].store(ba.to_bits(), Ordering::Relaxed);
                }
            }
            // the first still samples once (offset vs scale readable from the log)
            if let Some(last) = self.tally[k].s.last()
                && last.still()
                && self.self_s_n < SELF_S_MAX
            {
                self.self_s_n += 1;
                log_i!("DEPTH", "{} cand={}", eds::self_s_line(self.self_s_n, last, near, o.far), CAND_TAGS[k]);
            }
            log_i!("DEPTH", "{} cand={}", eds::self_line(&o, near, far), CAND_TAGS[k]);
        }
        const _: () = assert!(SAMPLE_N == 22 && eds::CHEST_AT + eds::CHEST_N == SAMPLE_N);
    }

    /// Design 15.3: live only on this session's verdict + sample + self check; logged once per decision.
    fn live_gate(&mut self, now: f64) {
        if !CFG_LIVE.load(Ordering::Relaxed) || LIVE.load(Ordering::Relaxed) || TRIPPED.load(Ordering::Relaxed) || now - self.t_live < 1.0 {
            return;
        }
        self.t_live = now;
        let pinned = mw2er_core::er_depth_state::STATE_PIN_2720.is_some();
        // C6 round 6: per candidate; the first that may go live is the one the copy reads (A first, then B)
        let steps = [0, 1].map(|k| {
            eds::live_step(&LiveIn {
                cfg_on: LIVE_CFG_NOW.load(Ordering::Relaxed),
                pinned,
                probe_state: CFG_STATE.load(Ordering::Relaxed),
                probe_sample: CFG_SAMPLE.load(Ordering::Relaxed),
                // not tried yet (no Present so far) = pending, not off
                hook_on: HOOK_ON.load(Ordering::Relaxed) || (!HOOK_TRIED.load(Ordering::Relaxed) && CFG_STATE.load(Ordering::Relaxed)),
                ui_hooks: ui_probe::INSTALLED_UI_BEGIN.load(Ordering::Relaxed) && ui_probe::ACTIVE.load(Ordering::Relaxed),
                verdict: self.verdict[k],
                conv: self.conv[k],
                self_ok: self.self_ok[k],
                sample_failed: FAILED.load(Ordering::Relaxed),
            })
        });
        let k = steps.iter().position(|s| *s == LiveStep::On);
        let step = match k {
            Some(_) => LiveStep::On,
            None if steps.contains(&LiveStep::Wait) => LiveStep::Wait,
            // both off: B's reason (the expected candidate, depth6.md 4 item 5)
            None => steps[1],
        };
        match step {
            LiveStep::Wait => {}
            LiveStep::On => {
                if !READER_READY.load(Ordering::Relaxed) {
                    return;
                }
                let k = k.unwrap_or(0);
                LIVE_CAND.store(k as u8, Ordering::SeqCst);
                self.at_live = Some(FOLDS[k].stats(table(k)));
                LIVE.store(true, Ordering::SeqCst);
                let rs = READER_SIZE.load(Ordering::Relaxed);
                log_i!(
                    "DEPTH",
                    "live on res=0x{:x} fmt={} size={}x{} conv=reverse near={:.4} near_cam={:.4} near_rel={:.3} far_model={} bias={:.3}+{:.4}z bias_add_cm={:.1} max_turn={:.1} cand={} (ER's depth primes our world-gun depth in third person with near = n_eff; capsules stay)",
                    watch(k).load(Ordering::Relaxed),
                    mw2er_core::er_depth::FMT_R32G8X24_TYPELESS,
                    rs as u32,
                    (rs >> 32) as u32,
                    f32::from_bits(NEAR_CAM.load(Ordering::Relaxed)) * near_rel(k),
                    f32::from_bits(NEAR_CAM.load(Ordering::Relaxed)),
                    near_rel(k),
                    if INF_FAR.load(Ordering::Relaxed) { "inf" } else { "finite" },
                    LAST_BIAS[0].load(Ordering::Relaxed) as f32 / 1e6,
                    LAST_BIAS[1].load(Ordering::Relaxed) as f32 / 1e6,
                    f32::from_bits(BIAS_ADD[k].load(Ordering::Relaxed)).max(0.0) * 100.0,
                    LAST_BIAS[2].load(Ordering::Relaxed) as f32 / 1e6,
                    CAND_TAGS[k]
                );
            }
            LiveStep::Off(why) => {
                if self.live_logged != Some(why) {
                    self.live_logged = Some(why);
                    log_i!("DEPTH", "live off why={why} (world guns keep the capsules)");
                }
            }
        }
    }

    /// Design 15.3 trips while live (counters against their values at going live) and the logged trip line.
    fn check_trips(&mut self) {
        if LIVE.load(Ordering::Relaxed)
            && let Some(a) = self.at_live
            && let Some(why) = eds::trip_of(&a, &{
                let k = LIVE_CAND.load(Ordering::Relaxed) as usize & 1;
                FOLDS[k].stats(table(k))
            })
        {
            trip(why);
        }
        if mw2er_render::er_depth::PANICS.load(Ordering::Relaxed) > 0 {
            trip("panic");
        }
        if TRIPPED.load(Ordering::Relaxed) && !self.trip_logged && CFG_LIVE.load(Ordering::Relaxed) {
            self.trip_logged = true;
            let why = TRIPS.get(TRIP_WHY.load(Ordering::SeqCst) as usize).copied().unwrap_or("?");
            let err = try_lock(&ERR).and_then(|g| g.as_ref().map(|e| e.to_string())).unwrap_or_default();
            log_w!("DEPTH", "live trip why={why}{} (capsules only for the session; the probe keeps logging)", if err.is_empty() { String::new() } else { format!(" err={err}") });
        }
    }
}

/// ER's near / far (the task's snapshot values, for the sample's linearisation) and the bias (millionths) of the live line.
static NEAR_CAM: AtomicU32 = AtomicU32::new(0);
static FAR_CAM: AtomicU32 = AtomicU32::new(0);
static LAST_BIAS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

/// C6 round 6: the positive control (every detour call; candidate B's armed frames and those with B transitions).
fn ctl() -> Ctl {
    Ctl {
        calls: mw2er_render::er_depth::CALLS.load(Ordering::Relaxed),
        frames: FOLDS[1].armed_frames.load(Ordering::Relaxed),
        hits: FOLDS[1].ctl_hits.load(Ordering::Relaxed),
    }
}

/// Task thread: ER's camera planes and the hot bias / turn values of this tick.
pub fn note_camera(near: f32, far: f32, cfg: &mw2er_core::config::CoopCfg) {
    if near.is_finite() && near > 0.0 {
        NEAR_CAM.store(near.to_bits(), Ordering::Relaxed);
    }
    if far.is_finite() {
        FAR_CAM.store(far.to_bits(), Ordering::Relaxed);
    }
    for (k, v) in [cfg.er_depth_bias_m, cfg.er_depth_bias_rel, cfg.er_depth_max_turn_deg].into_iter().enumerate() {
        LAST_BIAS[k].store((v * 1e6) as u32, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_depth_state_armed_pack_roundtrip() {
        for (seq, slot, c, s) in [(0u64, 0usize, true, false), (12345, 2, false, true), (SEQ_MASK - 2, 1, true, true)] {
            assert_eq!(armed_unpack(armed_pack(seq, slot, c, s)), Some((seq, slot, c, s)));
        }
        assert_eq!(armed_unpack(0), None);
        // C6 round 6: the candidate bit leaves (seq, slot, copy, sample) alone
        let v = armed_pack(41, 2, true, true) | ARMED_CAND_B;
        assert_eq!((armed_unpack(v), armed_cand(v), armed_cand(armed_pack(41, 2, true, true))), (Some((41, 2, true, true)), 1, 0));
        assert_eq!(TRIPS.len(), 11);
        assert!(TRIPS.contains(&"hresult") && TRIPS.contains(&"desc") && TRIPS.contains(&"panic") && TRIPS.contains(&"self"));
    }
}
