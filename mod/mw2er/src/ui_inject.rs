//! C4 (docs/C4_UI_DESIGN.md 3, 5, S1): the MW2 viewmodel UNDER Elden Ring's UI. OFF by default
//! (`[ui] inject_viewmodel`, cold to turn on, hot to turn off).
//!
//! - UiBegin's detour (ui_probe.rs, after ER's original ran) hands its facts to [`on_ui_begin`]: the gate
//!   (`mw2er_core::c4::gate`, design 3.3 step 2) and, when it passes, the record of world guns + gun (or the scope
//!   overlay) into our own early list (`viewmodel::record_early`, the target barriered `0xC0 -> RT -> 0xC0`). The
//!   frame state becomes Armed with the Scaleform list `[hal+0x1420]` read after the original.
//! - hudhook's ECL callback ([`on_ecl`]): an Armed frame, on the HAL's queue and thread, whose armed list is the FIRST
//!   list of the call, is spliced: hudhook executes ours, signals our fence, then ER's call unchanged. Submitted.
//! - UiEnd ([`on_ui_end`]) cancels a frame still Armed (never executed: its slot stays free).
//! - Present's callback ([`present_frame`]) closes the frame: `true` only when our list was spliced this frame, then
//!   viewmodel.rs skips the world guns, the gun and the overlay. Any other state draws at Present exactly as before.
//!   One `u64` state moved by compare-exchange: never both, never neither (exhaustively tested in mw2er-core).
//! - Expected misses only count (`UI inject early=.. miss=..`); a trip (5.2) turns the injection off for the session
//!   (`UI inject trip why=..`) and the task disables the hooks (unless the probe still uses them).
//! - No detour or callback here allocates, logs or blocks (try_lock only); the task thread logs from atomics.
//! - No game memory is written. ER's and Scaleform's lists are never touched.
//!
//! C4.1 PASS I41 (docs/C4_1_DESIGN.md 3 / 4 / 5; `[ui] inject_ui_thread`, cold, with `inject_viewmodel`): the same
//! hooks, splice, frame state and teardown, with UiBegin on ER's render thread instead of the Present thread
//! ([`on_ui_begin41`], gate `c4::gate41`): the pinned read-only chain gives ER's UI layer `[S+0x100]`, the gun is
//! written into it as opaque UI-layer content in the layer's alpha convention (pinned per build, or the in-DLL C4.1
//! probe verdict with P2's alpha), so ER's own composite puts its UI over it.
//! - Going live: the convention known (`c4::live41`), the early pass ready; then a session canary (3.6): the next
//!   armed frame records one texel at (2, 2) instead of the gun; after UiEnd's original (whose EndFrame submitted ER's
//!   last UI list, which the ECL callback saw), our sampler reads it back through our own ECL on ER's queue (the layer
//!   is in `0xC0` again; no barrier on ER's resource). Cleared -> trip `cleared`. Intact -> the persistence check
//!   (review C4.1): a later armed frame records an empty list (the gun at Present) and the same readback must find the
//!   canary texel empty in the convention (ER clears its layer between UI frames); still the canary -> `canary
//!   persisted` and trip `cleared`; empty -> the gun starts.
//! - Every 60 s the same UiEnd readback re-votes the convention on 16 grid texels; two disagreements trip `alpha`.
//! - `ahead` (a UiBegin that read the previous UiBegin's seq) never arms; a Present inside an open UI frame whose list
//!   was spliced draws at Present anyway and trips `order` (`c4::present41`).
//! - Present skips the world guns, the gun and the overlay only on a Submitted gun frame (canary and check frames
//!   draw at Present). Fallback frames keep the C5 cut-out (`fallback_cut` counts them while a message region is up).
//! - ER's render thread does no allocation, resource creation, logging or blocking lock: the snapshot copy, the pose
//!   scratch, the depth, the PSOs and the readback sampler are prepared by workers (`viewmodel::create_early`, the
//!   PSO / depth workers below); `viewmodel::record_early41` / `record_canary41` only record.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::hooks::dx12::pre_imgui::{PRESENT_SEQ, SPLICE_SIGNAL_FAILS, Splice};
use hudhook::mh::MH_DisableHook;
use mw2er_core::c4::{
    self, CanaryGate, CanaryRead, EarlyMode, EarlyOut, FlapRing, FlapWindow, GateIn, GateIn41, Kind, LayerAlpha, Live41, LiveIn41, Miss, Recheck41, ResPin41,
    SLOTS, SampleState, TexDesc, UiPath, Wants,
};
use mw2er_render::{LayerSampler, VmError};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandList, ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::Interface;

use crate::game::read;
use crate::log::{log_e, log_i, log_w};
use crate::shared::{self, try_lock};
use crate::ui_probe::{self, Facts41, UiFacts};

/// The hooks and the callback are installed for the injection (boot, `[ui] inject_viewmodel = true`).
pub static INSTALLED: AtomicBool = AtomicBool::new(false);
/// Installed, the early pass exists, the config is on, not tripped, the probe verdict (if the probe is on) ready.
static LIVE: AtomicBool = AtomicBool::new(false);
static TRIPPED: AtomicBool = AtomicBool::new(false);
static TRIP_WHY: AtomicUsize = AtomicUsize::new(0);
static TRIP_SEQ: AtomicU64 = AtomicU64::new(0);
static TRIP_ERR: Mutex<String> = Mutex::new(String::new());
/// C4.1: a trip's renderer error, stored as is (no formatting on ER's render thread).
static TRIP_VM: Mutex<Option<VmError>> = Mutex::new(None);
/// `[ui] probe_scaleform` is on too: wait for its verdict before going live.
static WAIT_VERDICT: AtomicBool = AtomicBool::new(false);
static HOT_OFF: AtomicBool = AtomicBool::new(false);
static HOOKS_DOWN: AtomicBool = AtomicBool::new(false);

/// The per-Present state (`mw2er_core::c4::pack`).
static STATE: AtomicU64 = AtomicU64::new(0);
/// The armed frame: Scaleform's list, our reserved fence value, the HAL queue and render thread.
static ARMED_LIST: AtomicUsize = AtomicUsize::new(0);
static ARMED_VALUE: AtomicU64 = AtomicU64::new(0);
static ARMED_QUEUE: AtomicUsize = AtomicUsize::new(0);
static ARMED_RTID: AtomicU32 = AtomicU32::new(0);
/// The fence value of each slot's last SUBMITTED list, the last slot used, the next value to reserve.
static SLOT_VALUE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static LAST_SLOT: AtomicUsize = AtomicUsize::new(SLOTS - 1);
static NEXT_VALUE: AtomicU64 = AtomicU64::new(0);
/// The early pass's COM pointers (kept alive by `viewmodel::EARLY`): lists, fence, device, a resource vtable.
static LIST_RAW: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static FENCE_RAW: AtomicUsize = AtomicUsize::new(0);
static DEVICE_RAW: AtomicUsize = AtomicUsize::new(0);
static OWN_MODULE: AtomicUsize = AtomicUsize::new(0);
static EARLY_READY: AtomicBool = AtomicBool::new(false);
static EARLY_STARTED: AtomicBool = AtomicBool::new(false);
/// PSO sets ready (`format | mode << 16 | 1 << 31`) and the one requested of the worker.
static PSO_READY: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static PSO_REQ: AtomicU32 = AtomicU32::new(0);
static PSO_BUSY: AtomicBool = AtomicBool::new(false);
/// The getter pinned at the first good frame (address + 16 bytes).
static PIN_ADDR: AtomicUsize = AtomicUsize::new(0);
static PIN: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// The target's module check, cached per vtable (`vt | module_ok`); the desc itself is re-read every UiBegin.
static CACHE_VT: AtomicUsize = AtomicUsize::new(0);
/// The previous UiBegin's target passed the target rule (format_flap counts accepted -> refused changes).
static CACHE_ACCEPTED: AtomicBool = AtomicBool::new(false);
static FLAP: Mutex<FlapWindow> = Mutex::new(FlapWindow::new());
/// Counters for `UI inject early=.. present=.. splice=.. cancelled=.. miss=..`.
static EARLY_N: AtomicU64 = AtomicU64::new(0);
static PRESENT_N: AtomicU64 = AtomicU64::new(0);
static SPLICE_N: AtomicU64 = AtomicU64::new(0);
static CANCELLED_N: AtomicU64 = AtomicU64::new(0);
static MISS: [AtomicU64; Miss::ALL.len()] = [const { AtomicU64::new(0) }; Miss::ALL.len()];
static STREAK: AtomicU32 = AtomicU32::new(0);
/// The first armed frame's facts for the `UI inject on` line, and the last target size for the size line.
static ON_INFO: AtomicU64 = AtomicU64::new(0);
static ON_PATH: AtomicU32 = AtomicU32::new(0);
static SIZE_NOW: AtomicU64 = AtomicU64::new(0);

// ---- C4.1 PASS I41 ------------------------------------------------------------------------------------------------------

/// `[ui] inject_ui_thread` with the C4.1 sites passed (boot): the C4.1 path replaces the C4 one.
static MODE41: AtomicBool = AtomicBool::new(false);
/// The session's convention (`ui_probe::alpha_code`; 0 until live).
static CONV: AtomicU8 = AtomicU8::new(0);
/// `ahead`: the seq of the previous UiBegin (every UiBegin, armed or not); the UI frame's open / close seqs.
static LAST_BEGIN_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static UI_BEGIN_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static UI_END_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
/// A frame armed successfully (a thread mismatch / a broken pin trips from then on); the warm-up UiBegin happened.
static HAD_GOOD: AtomicBool = AtomicBool::new(false);
static WARM: AtomicBool = AtomicBool::new(false);
/// `c4::ResPin41` (only the render thread writes it) and the format_flap ring.
static RES_GOOD: AtomicUsize = AtomicUsize::new(0);
static RES_CAND: AtomicUsize = AtomicUsize::new(0);
static RES_CAND_N: AtomicU32 = AtomicU32::new(0);
static FLAP41: Mutex<FlapRing> = Mutex::new(FlapRing::new());
/// The layer resource's vtable (render thread) and its D3D12-module check by the task (`vt | ok`): no module lookup,
/// no loader lock on ER's render thread.
static INJ_RES_VT: AtomicUsize = AtomicUsize::new(0);
static INJ_VT_OK: AtomicUsize = AtomicUsize::new(0);
/// The session canary: Due / InFlight / Passed, then (review C4.1) the persistence check CheckDue / CheckInFlight
/// between InFlight and Passed; the seq of the frame that recorded it, readbacks judged / impossible.
static CANARY: AtomicU8 = AtomicU8::new(CANARY_DUE);
const CANARY_DUE: u8 = 0;
const CANARY_INFLIGHT: u8 = 1;
const CANARY_PASSED: u8 = 2;
const CANARY_CHECK_DUE: u8 = 3;
const CANARY_CHECK_INFLIGHT: u8 = 4;
static CANARY_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static CANARY_NOREAD: AtomicU32 = AtomicU32::new(0);
/// Our depth's size (`w | h << 24`, published by the worker) and a size the gate met without one (the worker's job).
static DEPTH_WH: AtomicU64 = AtomicU64::new(0);
static DEPTH_REQ: AtomicU64 = AtomicU64::new(0);
static DEPTH_BUSY: AtomicBool = AtomicBool::new(false);
/// The frame armed by [`on_ui_begin41`]: its seq, ER's layer, `format | w << 16 | h << 40`, what it drew (1 canary,
/// 2 gun, 3 overlay, 4 the persistence check: nothing) and ER's HDR output (for the `on` line).
static ARMED41_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static ARMED41_RES: AtomicUsize = AtomicUsize::new(0);
static ARMED41_INFO: AtomicU64 = AtomicU64::new(0);
static ARMED41_KIND: AtomicU8 = AtomicU8::new(0);
static OUT_HDR: AtomicBool = AtomicBool::new(false);
/// UiEnd's original is running, and an ECL on the HAL queue from ER's render thread happened inside it (its EndFrame
/// submit: ER's `RENDER_TARGET -> 0xC0` on the layer is then on the queue ahead of our readback).
static UIEND_OPEN: AtomicBool = AtomicBool::new(false);
static ECL_IN_UIEND: AtomicBool = AtomicBool::new(false);
/// The UiEnd readback (our `LayerSampler`, own ECL + Signal on ER's queue): `c4::sample_pack` state, its kind
/// (1 canary, 2 re-check, 4 persistence check), its seq, the next fence value, the re-check request (task, 60 s).
static RB_STATE: AtomicU64 = AtomicU64::new(0);
static RB_KIND: AtomicU8 = AtomicU8::new(0);
static RB_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
static RB_NEXT: AtomicU64 = AtomicU64::new(0);
static RB_DUE: AtomicBool = AtomicBool::new(false);
static SAMPLER: Mutex<Option<LayerSampler>> = Mutex::new(None);
static SAMPLER_READY: AtomicBool = AtomicBool::new(false);
/// Present-drawn (fallback) frames while a C5 cut-out region was up.
static FALLBACK_CUT: AtomicU64 = AtomicU64::new(0);

fn miss(m: Miss) {
    MISS[m.index()].fetch_add(1, Ordering::Relaxed);
}

/// Turns the injection off for the session (any thread; atomics only). The task logs it and disables the hooks.
pub fn trip(m: Miss) {
    if !INSTALLED.load(Ordering::Relaxed) {
        return;
    }
    LIVE.store(false, Ordering::SeqCst);
    if !TRIPPED.swap(true, Ordering::SeqCst) {
        TRIP_WHY.store(m.index() + 1, Ordering::SeqCst);
        TRIP_SEQ.store(PRESENT_SEQ.load(Ordering::Relaxed), Ordering::SeqCst);
    }
}

fn trip_err(m: Miss, e: String) {
    if let Some(mut g) = try_lock(&TRIP_ERR)
        && g.is_empty()
    {
        *g = e;
    }
    trip(m);
}

/// C4.1: a trip with the renderer's error moved in (no formatting, no allocation on the render thread).
fn trip_vm(m: Miss, e: VmError) {
    if let Some(mut g) = try_lock(&TRIP_VM)
        && g.is_none()
    {
        *g = Some(e);
    }
    trip(m);
}

fn pso_key(format: u32, mode: EarlyMode) -> u32 {
    format | mode.code() << 16 | 1 << 31
}

fn pso_ready(format: u32, mode: EarlyMode) -> bool {
    let k = pso_key(format, mode);
    PSO_READY.iter().any(|a| a.load(Ordering::Relaxed) == k)
}

/// The early pass built PSOs for (format, mode).
pub fn mark_pso(format: u32, mode: EarlyMode) {
    let k = pso_key(format, mode);
    if pso_ready(format, mode) {
        return;
    }
    for a in &PSO_READY {
        if a.compare_exchange(0, k, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            return;
        }
    }
}

/// The early pass exists (viewmodel.rs, worker thread): its raw pointers for the detours and the callback.
pub fn publish_early(lists: [usize; SLOTS], fence: usize, device: usize, own_vtable: usize) {
    for (a, l) in LIST_RAW.iter().zip(lists) {
        a.store(l, Ordering::SeqCst);
    }
    FENCE_RAW.store(fence, Ordering::SeqCst);
    DEVICE_RAW.store(device, Ordering::SeqCst);
    OWN_MODULE.store(ui_probe::module_of(own_vtable), Ordering::SeqCst);
    EARLY_READY.store(true, Ordering::SeqCst);
}

fn wh(w: u32, h: u32) -> u64 {
    w as u64 | (h as u64) << 24
}

/// C4.1: the early pass's depth exists for `w` x `h` (worker thread).
pub fn publish_depth(w: u32, h: u32) {
    DEPTH_WH.store(wh(w, h), Ordering::SeqCst);
}

/// C4.1: the UiEnd readback sampler (worker thread; never released in-session).
pub fn publish_sampler(s: LayerSampler) {
    if let Ok(mut g) = SAMPLER.lock() {
        *g = Some(s);
        SAMPLER_READY.store(true, Ordering::SeqCst);
    }
}

fn completed() -> u64 {
    let raw = FENCE_RAW.load(Ordering::Relaxed) as *mut c_void;
    // SAFETY: our fence, alive while the early pass exists (it is never released while the hooks run); borrowed.
    unsafe { ID3D12Fence::from_raw_borrowed(&raw) }.map_or(0, |f| unsafe { f.GetCompletedValue() })
}

/// The getter equals the pinned one (pinned at the first plain one).
fn getter_ok(f: &UiFacts) -> bool {
    if !f.getter_read || c4::parse_getter(&f.getter).is_none() {
        return false;
    }
    let lo = u64::from_le_bytes(f.getter[..8].try_into().unwrap_or([0; 8]));
    let hi = u64::from_le_bytes(f.getter[8..].try_into().unwrap_or([0; 8]));
    if PIN_ADDR.load(Ordering::Relaxed) == 0 {
        PIN[0].store(lo, Ordering::Relaxed);
        PIN[1].store(hi, Ordering::Relaxed);
        PIN_ADDR.store(f.getter_addr, Ordering::SeqCst);
        return true;
    }
    PIN_ADDR.load(Ordering::Relaxed) == f.getter_addr && PIN[0].load(Ordering::Relaxed) == lo && PIN[1].load(Ordering::Relaxed) == hi
}

/// The target's module check and a FRESH desc every UiBegin (review C4: ER may recreate its UI target at the same
/// address with another format or size after a resize / HDR toggle; GetDesc is a cheap read-only query). Only the
/// module check is cached, keyed on the vtable pointer (`vt | ok`; vtables are 8-aligned). Returns (module_ok, desc).
fn target_desc(res: usize, path: UiPath, ui_layer: bool, now: f64) -> (bool, TexDesc) {
    // SAFETY: non-faulting read of the resource's vtable pointer.
    let vt = unsafe { read::<usize>(res) }.unwrap_or(0);
    let cached = CACHE_VT.load(Ordering::Relaxed);
    let module_ok = if vt != 0 && cached & !1 == vt {
        cached & 1 != 0
    } else {
        let own = OWN_MODULE.load(Ordering::Relaxed);
        let ok = vt != 0 && own != 0 && ui_probe::module_of(vt) == own;
        CACHE_VT.store(vt | ok as usize, Ordering::Relaxed);
        ok
    };
    let d = if module_ok { ui_probe::desc_if_vtable(res, vt) } else { None }.unwrap_or_default();
    let accepted = c4::target_rule(&d, path, ui_layer).is_ok();
    // a change to a refused target after a good one, 3 times in 10 s, trips (format_flap)
    let was = CACHE_ACCEPTED.swap(accepted, Ordering::Relaxed);
    if was && !accepted && module_ok && try_lock(&FLAP).is_some_and(|mut w| w.refused_change(now)) {
        trip(Miss::FormatFlap);
    }
    (module_ok, d)
}

/// UiBegin's detour, after the original (render thread). Gate, then record and arm.
pub fn on_ui_begin(f: &UiFacts) {
    if !LIVE.load(Ordering::Relaxed) {
        return;
    }
    let cur = STATE.load(Ordering::SeqCst);
    let first = c4::begin_ok(cur, f.seq);
    if first && c4::unpack(cur).kind == Kind::Armed {
        // an older Present's armed list was never resolved: it never ran (its slot was never committed)
        miss(Miss::Cancelled);
    }
    let now = shared::now_s();
    let ui_layer = shared::try_config().is_some_and(|c| c.ui.ui_layer_effective());
    let (module_ok, desc) = if f.res != 0 { target_desc(f.res, f.path, ui_layer, now) } else { (true, TexDesc::default()) };
    let values: [u64; SLOTS] = [0, 1, 2].map(|k| SLOT_VALUE[k].load(Ordering::SeqCst));
    let slot = c4::pick_slot(&values, completed(), LAST_SLOT.load(Ordering::Relaxed));
    let target = c4::target_rule(&desc, f.path, ui_layer);
    let mode = if f.path == UiPath::Layer { EarlyMode::Premul } else { EarlyMode::Scene };
    let g = GateIn {
        enabled: LIVE.load(Ordering::Relaxed),
        build: true,
        hal: f.hal,
        movies: f.movies,
        vt_ok: f.vt_ok,
        hal_flags: f.flags,
        tid: f.tid,
        hal_tid: f.rtid,
        present_tid: f.present_tid,
        thread_trips: ui_probe::PROBE_READY.load(Ordering::Relaxed),
        first,
        wants: crate::viewmodel::early_wants(),
        getter_ok: f.sel != 0 && getter_ok(f),
        chain_ok: f.res != 0,
        module_ok,
        target,
        device_ok: f.device != 0 && f.device == DEVICE_RAW.load(Ordering::Relaxed),
        slot_free: slot.is_some(),
        pso_ready: pso_ready(desc.format, mode),
    };
    let rule = match c4::gate(&g) {
        Ok(r) => r,
        Err(m) => {
            miss(m);
            if m.is_trip() {
                trip(m);
            }
            if m == Miss::PsoPending {
                let _ = PSO_REQ.compare_exchange(0, pso_key(desc.format, mode), Ordering::SeqCst, Ordering::SeqCst);
            }
            return;
        }
    };
    let Some(slot) = slot else { return };
    let value = NEXT_VALUE.fetch_add(1, Ordering::SeqCst) + 1;
    let req = crate::viewmodel::EarlyReq {
        res: f.res,
        format: desc.format,
        width: desc.width as u32,
        height: desc.height,
        mode,
        out: rule.out,
        slot,
        value,
    };
    match crate::viewmodel::record_early(&req) {
        Ok(()) => {
            // everything the callback reads is stored BEFORE the state says Armed
            ARMED_LIST.store(f.list, Ordering::SeqCst);
            ARMED_VALUE.store(value, Ordering::SeqCst);
            ARMED_QUEUE.store(f.queue, Ordering::SeqCst);
            ARMED_RTID.store(f.rtid, Ordering::SeqCst);
            LAST_SLOT.store(slot, Ordering::Relaxed);
            if STATE.compare_exchange(cur, c4::armed(f.seq, slot as u8), Ordering::SeqCst, Ordering::SeqCst).is_err() {
                miss(Miss::Cancelled);
                return;
            }
            let info = desc.width | (desc.height as u64) << 24 | (desc.format as u64) << 48;
            let _ = ON_INFO.compare_exchange(0, info, Ordering::SeqCst, Ordering::SeqCst);
            ON_PATH.store(if f.path == UiPath::Layer { 2 } else { 1 }, Ordering::Relaxed);
            SIZE_NOW.store(info, Ordering::Relaxed);
        }
        Err(crate::viewmodel::EarlyErr::Locked) => miss(Miss::Locked),
        Err(crate::viewmodel::EarlyErr::NotReady) => miss(Miss::NotReady),
        Err(crate::viewmodel::EarlyErr::Pso) => miss(Miss::PsoPending),
        Err(crate::viewmodel::EarlyErr::D3d(e)) => trip_err(Miss::Hresult, e),
        Err(crate::viewmodel::EarlyErr::Vm(e)) => trip_vm(Miss::Hresult, e),
    }
}

/// C4.1 (docs/C4_1_DESIGN.md 3.3 / 3.5 / 3.6): UiBegin's detour on ER's render thread, after the original and the
/// C4.1 reads. `ahead` and the UI-frame tracking first (every UiBegin, armed or not), then the gate, then the canary or
/// the gun record and Armed. Atomics, try_lock, non-faulting reads, one GetDesc, the record: nothing allocates, logs
/// or waits.
pub fn on_ui_begin41(f: &UiFacts, x: &Facts41) {
    if !INSTALLED.load(Ordering::Relaxed) {
        return;
    }
    let ahead = c4::ahead41(LAST_BEGIN_SEQ.swap(f.seq, Ordering::SeqCst), f.seq);
    UI_BEGIN_SEQ.store(f.seq, Ordering::SeqCst);
    let c = &x.chain;
    // SAFETY: non-faulting read of the layer resource's vtable pointer (the task checks its module).
    let vt = if c.res != 0 { unsafe { read::<usize>(c.res) }.unwrap_or(0) } else { 0 };
    INJ_RES_VT.store(vt, Ordering::Relaxed);
    if !LIVE.load(Ordering::Relaxed) {
        return;
    }
    let cur = STATE.load(Ordering::SeqCst);
    let first = c4::begin_ok(cur, f.seq);
    if first && c4::unpack(cur).kind == Kind::Armed {
        miss(Miss::Cancelled);
    }
    let now = shared::now_s();
    let hdr_ok = shared::try_config().is_some_and(|c| c.ui.ui_layer_effective());
    let checked = INJ_VT_OK.load(Ordering::Relaxed);
    let module = (vt != 0 && checked & !1 == vt).then_some(checked & 1 != 0);
    // a fresh desc every UiBegin, only through a vtable the task found in the D3D12 runtime module
    let desc = if module == Some(true) && c.ok() { ui_probe::desc_if_vtable(c.res, vt) } else { None };
    let bbv = ui_probe::BB_SIZE.load(Ordering::Relaxed);
    let bb = ((bbv & 0xff_ffff) as u32, ((bbv >> 24) & 0xff_ffff) as u32, (bbv >> 48) as u32);
    let target = desc.map_or(Err(Miss::NotReady), |d| c4::layer_rule41(&d, bb));
    // the layer resource: a new / recreated one passes two UiBegins first; 3 changes in 10 s trip format_flap
    let res = if c.ok() && module == Some(true) {
        let p = ResPin41 { good: RES_GOOD.load(Ordering::Relaxed), cand: RES_CAND.load(Ordering::Relaxed), cand_n: RES_CAND_N.load(Ordering::Relaxed) };
        let (n, r, changed) = c4::res_step41(p, c.res, c4::rtdata_match(c, &x.rtd, desc.map_or(0, |d| d.format)));
        RES_GOOD.store(n.good, Ordering::Relaxed);
        RES_CAND.store(n.cand, Ordering::Relaxed);
        RES_CAND_N.store(n.cand_n, Ordering::Relaxed);
        if changed && try_lock(&FLAP41).is_some_and(|mut w| w.note(now)) {
            trip(Miss::FormatFlap);
        }
        r
    } else {
        Err(Miss::NoTarget)
    };
    let conv = ui_probe::alpha_of(CONV.load(Ordering::Relaxed));
    let mode = conv.map_or(EarlyMode::Scene, EarlyMode::of);
    let (fmt, w, h) = target.unwrap_or((0, 0, 0));
    let values: [u64; SLOTS] = [0, 1, 2].map(|k| SLOT_VALUE[k].load(Ordering::SeqCst));
    let slot = c4::pick_slot(&values, completed(), LAST_SLOT.load(Ordering::Relaxed));
    let canary = match CANARY.load(Ordering::SeqCst) {
        CANARY_DUE => CanaryGate::Due,
        CANARY_INFLIGHT | CANARY_CHECK_INFLIGHT => CanaryGate::InFlight,
        CANARY_CHECK_DUE => CanaryGate::Check,
        _ => CanaryGate::Passed,
    };
    let wants = crate::viewmodel::early_wants();
    let g = GateIn41 {
        enabled: LIVE.load(Ordering::Relaxed),
        build: true,
        hal: f.hal,
        movies: f.movies,
        vt_ok: f.vt_ok,
        hal_flags: f.flags,
        mt: x.mt,
        tid: f.tid,
        hal_tid: f.rtid,
        rt_tid: x.rt_tid,
        present_tid: f.present_tid,
        had_good: HAD_GOOD.load(Ordering::Relaxed),
        ahead,
        first,
        wants,
        layer_path: f.path == UiPath::Layer,
        sel: f.layer,
        chain: *c,
        module,
        res,
        output: x.out,
        hdr_ok,
        target,
        alpha: conv,
        canary,
        warm: WARM.load(Ordering::Relaxed),
        device_ok: f.device != 0 && f.device == DEVICE_RAW.load(Ordering::Relaxed),
        slot_free: slot.is_some(),
        depth_ready: w > 0 && DEPTH_WH.load(Ordering::Relaxed) == wh(w, h),
        pso_ready: pso_ready(fmt, mode),
    };
    let e = match c4::gate41(&g) {
        Ok(e) => e,
        Err(m) => {
            miss(m);
            if m.is_trip() {
                trip(m);
            }
            match m {
                Miss::PsoPending => {
                    let _ = PSO_REQ.compare_exchange(0, pso_key(fmt, mode), Ordering::SeqCst, Ordering::SeqCst);
                }
                Miss::Size if w > 0 => {
                    let _ = DEPTH_REQ.compare_exchange(0, wh(w, h), Ordering::SeqCst, Ordering::SeqCst);
                }
                // the first gate-passing UiBegin after going live records nothing (design 3.5)
                Miss::Warmup => WARM.store(true, Ordering::Relaxed),
                _ => {}
            }
            return;
        }
    };
    let (Some(slot), Some(conv)) = (slot, conv) else { return };
    let value = NEXT_VALUE.fetch_add(1, Ordering::SeqCst) + 1;
    let req = crate::viewmodel::EarlyReq {
        res: c.res,
        format: e.format,
        width: e.width,
        height: e.height,
        mode: e.mode,
        // the UNORM layer gets the same sRGB-encoded values as an SDR back buffer (SDR2SDR blends in gamma space [I])
        out: EarlyOut::Srgb,
        slot,
        value,
    };
    // the canary frame writes the canary texel, the persistence-check frame nothing (its readback looks for the canary)
    let r = if e.canary {
        crate::viewmodel::record_canary41(&req, Some(c4::canary_color41(conv)))
    } else if e.check {
        crate::viewmodel::record_canary41(&req, None)
    } else {
        crate::viewmodel::record_early41(&req)
    };
    match r {
        Ok(()) => {
            // everything the callback, UiEnd and Present read is stored BEFORE the state says Armed
            ARMED_LIST.store(f.list, Ordering::SeqCst);
            ARMED_VALUE.store(value, Ordering::SeqCst);
            ARMED_QUEUE.store(f.queue, Ordering::SeqCst);
            ARMED_RTID.store(f.tid, Ordering::SeqCst);
            ARMED41_SEQ.store(f.seq, Ordering::SeqCst);
            ARMED41_RES.store(c.res, Ordering::SeqCst);
            ARMED41_INFO.store(e.format as u64 | (e.width as u64) << 16 | (e.height as u64) << 40, Ordering::SeqCst);
            ARMED41_KIND.store(if e.canary { 1 } else if e.check { 4 } else if wants == Wants::Gun { 2 } else { 3 }, Ordering::SeqCst);
            OUT_HDR.store(x.out.hdr(), Ordering::Relaxed);
            if e.canary || e.check {
                CANARY_SEQ.store(f.seq, Ordering::SeqCst);
                CANARY.store(if e.canary { CANARY_INFLIGHT } else { CANARY_CHECK_INFLIGHT }, Ordering::SeqCst);
            }
            LAST_SLOT.store(slot, Ordering::Relaxed);
            if STATE.compare_exchange(cur, c4::armed(f.seq, slot as u8), Ordering::SeqCst, Ordering::SeqCst).is_err() {
                miss(Miss::Cancelled);
                return;
            }
            HAD_GOOD.store(true, Ordering::Relaxed);
            let info = e.width as u64 | (e.height as u64) << 24 | (e.format as u64) << 48;
            let _ = ON_INFO.compare_exchange(0, info, Ordering::SeqCst, Ordering::SeqCst);
            ON_PATH.store(2, Ordering::Relaxed);
            SIZE_NOW.store(info, Ordering::Relaxed);
        }
        Err(crate::viewmodel::EarlyErr::Locked) => miss(Miss::Locked),
        Err(crate::viewmodel::EarlyErr::NotReady) => miss(Miss::NotReady),
        Err(crate::viewmodel::EarlyErr::Pso) => miss(Miss::PsoPending),
        Err(crate::viewmodel::EarlyErr::D3d(e)) => trip_err(Miss::Hresult, e),
        Err(crate::viewmodel::EarlyErr::Vm(e)) => trip_vm(Miss::Hresult, e),
    }
}

/// hudhook's ECL callback (any thread; atomics only): the splice of an armed frame (design 3.2).
pub fn on_ecl(queue: usize, lists: &[usize], seq: u64) -> Option<Splice> {
    if !LIVE.load(Ordering::Relaxed) {
        return None;
    }
    let cur = STATE.load(Ordering::SeqCst);
    if c4::unpack(cur).kind != Kind::Armed || queue != ARMED_QUEUE.load(Ordering::SeqCst) {
        return None;
    }
    let at = c4::find_armed(lists, ARMED_LIST.load(Ordering::SeqCst))?;
    // SAFETY: GetCurrentThreadId has no preconditions.
    if unsafe { GetCurrentThreadId() } != ARMED_RTID.load(Ordering::Relaxed) {
        return None;
    }
    if !c4::splice_allowed(at) {
        if let Some(n) = c4::ui_end(cur, seq)
            && STATE.compare_exchange(cur, n, Ordering::SeqCst, Ordering::SeqCst).is_ok()
        {
            miss(Miss::Split);
        }
        return None;
    }
    let next = c4::ecl_submit(cur, seq)?;
    let slot = c4::unpack(next).slot as usize;
    let (lraw, fraw) = (LIST_RAW[slot].load(Ordering::SeqCst) as *mut c_void, FENCE_RAW.load(Ordering::SeqCst) as *mut c_void);
    // SAFETY: our own list (a graphics list IS a command list: same COM object, base vtable prefix) and fence, kept
    // alive by the early pass; borrowed, then cloned (AddRef) into the splice hudhook executes and releases. Taken
    // BEFORE the state moves, so a Submitted frame always has its splice.
    let list = unsafe { ID3D12CommandList::from_raw_borrowed(&lraw) }?.clone();
    let fence = unsafe { ID3D12Fence::from_raw_borrowed(&fraw) }?.clone();
    STATE.compare_exchange(cur, next, Ordering::SeqCst, Ordering::SeqCst).ok()?;
    let value = ARMED_VALUE.load(Ordering::SeqCst);
    SLOT_VALUE[slot].store(value, Ordering::SeqCst);
    SPLICE_N.fetch_add(1, Ordering::Relaxed);
    Some(Splice { at, list, fence, value })
}

/// C4.1: every ECL (any thread; atomics only): one on the armed frame's HAL queue from ER's render thread while UiEnd's
/// original runs is its EndFrame submit (design 3.6: the layer's `RENDER_TARGET -> 0xC0` is then queued).
pub fn note_ecl(queue: usize) {
    if UIEND_OPEN.load(Ordering::Relaxed) && queue != 0 && queue == ARMED_QUEUE.load(Ordering::Relaxed) {
        // SAFETY: GetCurrentThreadId has no preconditions.
        if unsafe { GetCurrentThreadId() } == ARMED_RTID.load(Ordering::Relaxed) {
            ECL_IN_UIEND.store(true, Ordering::SeqCst);
        }
    }
}

/// C4.1: UiEnd's detour BEFORE its original (render thread; atomics only).
pub fn before_ui_end() {
    if MODE41.load(Ordering::Relaxed) && INSTALLED.load(Ordering::Relaxed) {
        ECL_IN_UIEND.store(false, Ordering::SeqCst);
        UIEND_OPEN.store(true, Ordering::SeqCst);
    }
}

/// UiEnd's detour, after the original: a frame still Armed was never submitted (cancelled). C4.1: the UI frame is
/// closed, and a Submitted canary / re-check frame gets its readback.
pub fn on_ui_end(seq: u64) {
    if !INSTALLED.load(Ordering::Relaxed) {
        return;
    }
    let cur = STATE.load(Ordering::SeqCst);
    if let Some(n) = c4::ui_end(cur, seq) {
        if STATE.compare_exchange(cur, n, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            cancelled();
        }
    } else if c4::unpack(cur).kind == Kind::Submitted {
        STREAK.store(0, Ordering::Relaxed);
    }
    if MODE41.load(Ordering::Relaxed) {
        UIEND_OPEN.store(false, Ordering::SeqCst);
        UI_END_SEQ.store(seq, Ordering::SeqCst);
        readback41(seq);
    }
}

/// C4.1 (design 3.6): after UiEnd's original, on ER's render thread. A Submitted canary frame (or a gun frame when a
/// re-check is due) records our sampler's list (16 texels of ER's layer through our SRV; no barrier on ER's resource)
/// and executes it with our Signal on ER's queue. Only when the ECL callback saw UiEnd's own EndFrame submit on that
/// queue from this thread (ER's `RENDER_TARGET -> 0xC0` on the layer is then queued ahead of our read [V: UiEnd
/// records the barrier, then EndFrame submits]) and the queue is the one hudhook presents on. No allocation, no log.
fn readback41(seq: u64) {
    if !LIVE.load(Ordering::Relaxed) || !SAMPLER_READY.load(Ordering::Relaxed) {
        return;
    }
    let st = c4::unpack(STATE.load(Ordering::SeqCst));
    if st.kind != Kind::Submitted || st.seq != c4::unpack(c4::pack(Kind::Idle, 0, seq)).seq || ARMED41_SEQ.load(Ordering::SeqCst) != seq {
        return;
    }
    let kind = ARMED41_KIND.load(Ordering::SeqCst);
    // the canary frame and the persistence check both read the canary texel (point 0)
    let canary = kind == 1 || kind == 4;
    let recheck = kind == 2 && RB_DUE.load(Ordering::Relaxed);
    if !(canary || recheck) || c4::sample_unpack(RB_STATE.load(Ordering::SeqCst)) != SampleState::Idle {
        return;
    }
    let queue = ARMED_QUEUE.load(Ordering::SeqCst);
    if !ECL_IN_UIEND.load(Ordering::SeqCst) || queue == 0 || queue != ui_probe::PRESENT_QUEUE.load(Ordering::Relaxed) {
        if canary {
            CANARY_NOREAD.fetch_add(1, Ordering::SeqCst);
        }
        return;
    }
    let Some(mut sg) = try_lock(&SAMPLER) else { return };
    let Some(s) = sg.as_mut() else { return };
    let info = ARMED41_INFO.load(Ordering::SeqCst);
    let (fmt, w, h) = ((info & 0xffff) as u32, ((info >> 16) & 0xff_ffff) as u32, ((info >> 40) & 0xff_ffff) as u32);
    let pts = c4::readback_points41(w, h, canary);
    let (rraw, qraw) = (ARMED41_RES.load(Ordering::SeqCst) as *mut c_void, queue as *mut c_void);
    // SAFETY: ER's layer (validated at this frame's UiBegin; ER's composite of this frame still reads it) and ER's
    // HAL queue (== hudhook's present queue): borrowed, no AddRef / Release.
    let (Some(res), Some(q)) = (unsafe { ID3D12Resource::from_raw_borrowed(&rraw) }, unsafe { ID3D12CommandQueue::from_raw_borrowed(&qraw) }) else { return };
    if let Err(e) = s.record(res, DXGI_FORMAT(fmt as i32), &pts) {
        trip_vm(Miss::Hresult, e);
        return;
    }
    let Ok(list) = s.list().cast::<ID3D12CommandList>() else {
        trip(Miss::Hresult);
        return;
    };
    let value = RB_NEXT.fetch_add(1, Ordering::SeqCst) + 1;
    RB_KIND.store(if canary { kind } else { 2 }, Ordering::SeqCst);
    RB_SEQ.store(seq, Ordering::SeqCst);
    RB_STATE.store(c4::sample_pack(SampleState::Submitted(value)), Ordering::SeqCst);
    if recheck {
        RB_DUE.store(false, Ordering::Relaxed);
    }
    // SAFETY: our closed list and our fence (alive: the sampler is never released in-session) on ER's queue, from ER's
    // render thread, after ER's own last UI submit of this frame.
    unsafe {
        q.ExecuteCommandLists(&[Some(list)]);
        if q.Signal(s.fence(), value).is_err() {
            trip(Miss::Hresult);
        }
    }
}

fn cancelled() {
    CANCELLED_N.fetch_add(1, Ordering::Relaxed);
    miss(Miss::Cancelled);
    let mut s = c4::Streak { cancelled_in_row: STREAK.load(Ordering::Relaxed) };
    let t = s.armed_outcome(false);
    STREAK.store(s.cancelled_in_row, Ordering::Relaxed);
    if t {
        trip(Miss::NeverSubmitted);
    }
}

/// Present's pre-ImGui callback (viewmodel.rs): closes the frame. `true` = our early list drew this frame's world
/// guns + gun / overlay under ER's UI, so Present skips them. One atomic load while the injection is not installed.
/// C4.1: a Present inside an open UI frame (design 3.4) and the canary frames (drawn at Present) as well.
pub fn present_frame() -> bool {
    if !INSTALLED.load(Ordering::Relaxed) {
        return false;
    }
    let seq = PRESENT_SEQ.load(Ordering::SeqCst);
    let m41 = MODE41.load(Ordering::Relaxed);
    let open = m41 && c4::present_in_ui41(UI_BEGIN_SEQ.load(Ordering::SeqCst), UI_END_SEQ.load(Ordering::SeqCst), seq);
    let mut cur = STATE.load(Ordering::SeqCst);
    let (mut d, order) = loop {
        let (d, next, order) = c4::present41(cur, seq, open);
        match STATE.compare_exchange(cur, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break (d, order),
            Err(x) => cur = x,
        }
    };
    if order {
        trip(Miss::Order);
    }
    let cs = CANARY_SEQ.load(Ordering::SeqCst);
    if m41 && cs != u64::MAX && cs <= seq {
        // the canary frame's list drew no gun: it is drawn here
        if cs == seq && d.early {
            d.early = false;
            miss(Miss::Canary);
        }
        // a canary whose readback never went out (cancelled, split, no EndFrame seen; or its Present was skipped by
        // the AC-130): another canary frame
        if RB_SEQ.load(Ordering::SeqCst) != cs {
            let _ = CANARY.compare_exchange(CANARY_INFLIGHT, CANARY_DUE, Ordering::SeqCst, Ordering::SeqCst);
            // the same for the persistence check: another check frame
            let _ = CANARY.compare_exchange(CANARY_CHECK_INFLIGHT, CANARY_CHECK_DUE, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
    if d.cancelled {
        cancelled();
    }
    if d.early {
        EARLY_N.fetch_add(1, Ordering::Relaxed);
    } else if LIVE.load(Ordering::Relaxed) {
        PRESENT_N.fetch_add(1, Ordering::Relaxed);
    }
    if LIVE.load(Ordering::Relaxed) {
        let raw = DEVICE_RAW.load(Ordering::Relaxed) as *mut c_void;
        // SAFETY: the early pass's device (alive); borrowed; a plain query.
        if unsafe { ID3D12Device::from_raw_borrowed(&raw) }.is_some_and(|dv| unsafe { dv.GetDeviceRemovedReason() }.is_err()) {
            trip(Miss::DeviceRemoved);
        }
        if SPLICE_SIGNAL_FAILS.load(Ordering::Relaxed) > 0 {
            trip(Miss::Hresult);
        }
    }
    d.early
}

/// C4.1 `fallback_cut` (viewmodel.rs, present thread): the gun is drawn at Present with a C5 cut-out region up while the
/// UI-thread injection is live.
pub fn note_fallback_cut() {
    if MODE41.load(Ordering::Relaxed) && LIVE.load(Ordering::Relaxed) {
        FALLBACK_CUT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Boot thread, after `ui_probe::install_ui` (`[ui] inject_viewmodel = true`). `ui_thread` = `[ui] inject_ui_thread`
/// (C4.1): it needs the C4.1 sites (`ui_probe::INJ41`), else the injection stays off (never the C4 rule instead).
pub fn boot(hooks_ok: bool, probe: bool, ui_thread: bool) {
    let build = crate::version_gate::build();
    if build.and_then(|b| b.own().scaleform_ui).is_none() {
        log_w!("UI", "inject off why=build ({}: no verified ScaleformUi addresses; the gun stays at Present)", build.map_or("none", |b| b.table_name()));
        return;
    }
    if !hooks_ok {
        log_w!("UI", "inject off why=hooks (UiBegin / UiEnd / the ECL callback not all installed; the gun stays at Present)");
        return;
    }
    if ui_thread && !ui_probe::INJ41.load(Ordering::SeqCst) {
        log_w!("UI", "inject off why=build (inject_ui_thread: the C4.1 sites did not pass on the loaded image; the gun stays at Present)");
        return;
    }
    MODE41.store(ui_thread, Ordering::SeqCst);
    WAIT_VERDICT.store(probe, Ordering::SeqCst);
    INSTALLED.store(true, Ordering::SeqCst);
    log_i!(
        "UI",
        "inject hooks ready mode={} (the early pass is created once the renderer is; probe verdict gate={}; the gun draws at Present until then)",
        if ui_thread { "ui_thread" } else { "present_thread" },
        probe as u8
    );
}

/// Task thread, every frame while installed: going live, hot disable, trips, the PSO worker, the log lines.
pub struct InjectLog {
    last_t: f64,
    on_logged: bool,
    trip_logged: bool,
    size_logged: u64,
    // C4.1
    hooks_t: f64,
    live41_logged: bool,
    canary_tries: u32,
    gone_tries: u32,
    rb_t0: Option<f64>,
    recheck: Recheck41,
    recheck_t: f64,
}

impl InjectLog {
    pub const fn new() -> Self {
        InjectLog {
            last_t: 0.0,
            on_logged: false,
            trip_logged: false,
            size_logged: 0,
            hooks_t: -1.0,
            live41_logged: false,
            canary_tries: 0,
            gone_tries: 0,
            rb_t0: None,
            recheck: Recheck41 { disagree: 0 },
            recheck_t: 0.0,
        }
    }

    pub fn tick(&mut self, now: f64) {
        if !INSTALLED.load(Ordering::Relaxed) {
            return;
        }
        if self.hooks_t < 0.0 {
            self.hooks_t = now;
        }
        let cfg = shared::config();
        let m41 = MODE41.load(Ordering::Relaxed);
        // the early pass, once the renderer is ready (a worker: no D3D creation on the task thread)
        if crate::shared::VM_READY.load(Ordering::Acquire) && !EARLY_STARTED.swap(true, Ordering::SeqCst) {
            let layer = cfg.ui.ui_layer_effective();
            let r = std::thread::Builder::new().name("mw2er-c4early".into()).spawn(move || {
                crate::log::set_thread_role("c4early");
                match crate::guard::guard("c4.early", || crate::viewmodel::create_early(layer, m41)) {
                    Some(Ok(info)) => log_i!("UI", "inject early pass ready {info}"),
                    Some(Err(e)) => {
                        log_e!("UI", "inject off why=early_pass ({e})");
                        trip(Miss::Hresult);
                    }
                    None => trip(Miss::Panic),
                }
            });
            if let Err(e) = r {
                log_e!("UI", "inject off why=early_pass (thread: {e})");
            }
        }
        if !cfg.ui.inject_viewmodel && !HOT_OFF.swap(true, Ordering::SeqCst) {
            LIVE.store(false, Ordering::SeqCst);
            log_i!("UI", "inject off why=config (hot: the gun draws at Present from the next frame)");
            self.hooks_down();
        }
        // C4.1: `inject_ui_thread` turned off hot (the master switch stays on): the injection goes off too
        if m41 && !cfg.ui.inject_ui_thread && !HOT_OFF.swap(true, Ordering::SeqCst) {
            LIVE.store(false, Ordering::SeqCst);
            log_i!("UI", "inject off why=config (hot: inject_ui_thread = false; the gun draws at Present from the next frame)");
            self.hooks_down();
        }
        if TRIPPED.load(Ordering::SeqCst) && !self.trip_logged {
            self.trip_logged = true;
            let why = TRIP_WHY.load(Ordering::SeqCst).checked_sub(1).and_then(|i| Miss::ALL.get(i).copied()).unwrap_or(Miss::Panic);
            let mut err = try_lock(&TRIP_ERR).map(|g| g.clone()).unwrap_or_default();
            if err.is_empty() {
                err = try_lock(&TRIP_VM).and_then(|g| g.as_ref().map(|e| e.to_string())).unwrap_or_default();
            }
            log_e!("UI", "inject trip why={} frame={}{}", why.label(), TRIP_SEQ.load(Ordering::SeqCst), if err.is_empty() { String::new() } else { format!(" err={err}") });
            self.hooks_down();
        }
        // live: the early pass exists, the config is on, nothing tripped, and (probe on) the verdict was READY
        if !LIVE.load(Ordering::Relaxed) && EARLY_READY.load(Ordering::SeqCst) && !TRIPPED.load(Ordering::SeqCst) && !HOT_OFF.load(Ordering::SeqCst) {
            if m41 {
                self.go_live41(now);
            } else if WAIT_VERDICT.load(Ordering::SeqCst) {
                if ui_probe::PROBE_JUDGED.load(Ordering::SeqCst) {
                    WAIT_VERDICT.store(false, Ordering::SeqCst);
                    if ui_probe::PROBE_READY.load(Ordering::SeqCst) {
                        LIVE.store(true, Ordering::SeqCst);
                        log_i!("UI", "inject live (probe verdict ready)");
                    } else {
                        HOT_OFF.store(true, Ordering::SeqCst);
                        log_w!("UI", "inject off why=probe_verdict (see the UI probe verdict line; the gun stays at Present)");
                    }
                }
            } else {
                LIVE.store(true, Ordering::SeqCst);
                log_i!("UI", "inject live");
            }
        }
        if m41 {
            self.tick41(now);
        }
        // the PSO worker (never on the render thread)
        let req = PSO_REQ.load(Ordering::SeqCst);
        if req != 0 && !PSO_BUSY.swap(true, Ordering::SeqCst) {
            let format = req & 0xffff;
            let mode = EarlyMode::from_code(req >> 16 & 0x7fff).unwrap_or(EarlyMode::Scene);
            let r = std::thread::Builder::new().name("mw2er-c4pso".into()).spawn(move || {
                crate::log::set_thread_role("c4pso");
                match crate::guard::guard("c4.pso", || crate::viewmodel::build_early_psos(format, mode)) {
                    Some(Ok(())) => log_i!("UI", "inject PSOs ready fmt={}:{} mode={}", format, c4::format_name(format), mode.label()),
                    Some(Err(e)) => trip_err(Miss::Hresult, format!("PSOs fmt={format}: {e}")),
                    None => trip(Miss::Panic),
                }
                PSO_REQ.store(0, Ordering::SeqCst);
                PSO_BUSY.store(false, Ordering::SeqCst);
            });
            if r.is_err() {
                PSO_BUSY.store(false, Ordering::SeqCst);
            }
        }
        let info = ON_INFO.load(Ordering::Relaxed);
        if info != 0 && !self.on_logged && !m41 {
            self.on_logged = true;
            let (w, h, f) = ((info & 0xff_ffff) as u32, ((info >> 24) & 0xff_ffff) as u32, (info >> 48) as u32);
            log_i!(
                "UI",
                "inject on build={} path={} fmt={} size={w}x{h} slots={SLOTS}",
                crate::version_gate::build().map_or("none".into(), |b| mw2er_core::gate::fmt_quad(b.version())),
                if ON_PATH.load(Ordering::Relaxed) == 2 { "layer" } else { "scene" },
                c4::format_name(f)
            );
        }
        let sz = SIZE_NOW.load(Ordering::Relaxed) & 0xffff_ffff_ffff;
        let bb = ui_probe::BB_SIZE.load(Ordering::Relaxed) & 0xffff_ffff_ffff;
        if sz != 0 && bb != 0 && sz != bb && sz != self.size_logged {
            self.size_logged = sz;
            log_i!("UI", "inject size target={}x{} bb={}x{}", sz & 0xff_ffff, sz >> 24, bb & 0xff_ffff, bb >> 24);
        }
        if now - self.last_t >= 5.0 {
            self.last_t = now;
            crate::viewmodel::early_housekeeping();
            if EARLY_READY.load(Ordering::Relaxed) {
                let m: Vec<u64> = MISS.iter().map(|a| a.load(Ordering::Relaxed)).collect();
                let (early, present, splice, cancelled) =
                    (EARLY_N.load(Ordering::Relaxed), PRESENT_N.load(Ordering::Relaxed), SPLICE_N.load(Ordering::Relaxed), CANCELLED_N.load(Ordering::Relaxed));
                let line = if m41 {
                    c4::fmt_inject41(early, present, splice, cancelled, FALLBACK_CUT.load(Ordering::Relaxed), &m)
                } else {
                    c4::fmt_inject(early, present, splice, cancelled, &m)
                };
                log_i!("UI", "{line}");
            }
        }
    }

    /// C4.1 (design 3.2 / 4): the session's convention from the per-build pin or the in-DLL C4.1 probe verdict.
    fn go_live41(&mut self, now: f64) {
        let l = LiveIn41 {
            pinned: crate::version_gate::build().and_then(|b| b.own().scaleform_ui).and_then(|s| s.layer_alpha),
            probe: ui_probe::probe41_on(),
            judged: ui_probe::PROBE41_JUDGED.load(Ordering::SeqCst),
            ready: ui_probe::PROBE41_READY.load(Ordering::SeqCst),
            probe_alpha: ui_probe::alpha_of(ui_probe::PROBE41_ALPHA.load(Ordering::SeqCst)),
            only_alpha: ui_probe::PROBE41_ONLY_ALPHA.load(Ordering::SeqCst),
            sampling: crate::ui_sample::running(),
            waited_s: now - self.hooks_t,
        };
        match c4::live41(&l) {
            Live41::Go(conv) => {
                CONV.store(ui_probe::alpha_code(Some(conv)), Ordering::SeqCst);
                LIVE.store(true, Ordering::SeqCst);
                log_i!(
                    "UI",
                    "inject live mode=ui_thread alpha={} (from {}; the session canary is next, the gun draws at Present until it passed)",
                    conv.label(),
                    if l.pinned.is_some() { "the per-build pin" } else { "the C4.1 probe verdict" }
                );
            }
            Live41::Wait => {}
            Live41::Off(why) => {
                HOT_OFF.store(true, Ordering::SeqCst);
                log_w!(
                    "UI",
                    "inject off why={why} (C4.1: the layer's alpha convention is not pinned for this build, or the UI probe41 verdict is not ready; the gun stays at Present)"
                );
            }
        }
    }

    /// C4.1, every task tick while installed: the layer's module check, the depth worker, the UiEnd readback (canary /
    /// re-check), the re-check request and the `on` line.
    fn tick41(&mut self, now: f64) {
        // the layer resource's vtable in the D3D12 runtime module (here: no loader lock on ER's render thread)
        let vt = INJ_RES_VT.load(Ordering::Relaxed);
        let own = OWN_MODULE.load(Ordering::Relaxed);
        if vt != 0 && own != 0 && INJ_VT_OK.load(Ordering::Relaxed) & !1 != vt {
            let m = ui_probe::module_of(vt);
            INJ_VT_OK.store(vt | (m != 0 && m == own) as usize, Ordering::Relaxed);
        }
        // the depth worker (never on the render thread)
        let req = DEPTH_REQ.load(Ordering::SeqCst);
        if req != 0 && !DEPTH_BUSY.swap(true, Ordering::SeqCst) {
            let (w, h) = ((req & 0xff_ffff) as u32, ((req >> 24) & 0xff_ffff) as u32);
            let r = std::thread::Builder::new().name("mw2er-c4depth".into()).spawn(move || {
                crate::log::set_thread_role("c4depth");
                match crate::guard::guard("c4.depth", || crate::viewmodel::prepare_early_depth(w, h)) {
                    Some(Ok(())) => log_i!("UI", "inject depth ready {w}x{h} (prepared off ER's render thread)"),
                    Some(Err(e)) => trip_err(Miss::Hresult, format!("depth {w}x{h}: {e}")),
                    None => trip(Miss::Panic),
                }
                DEPTH_REQ.store(0, Ordering::SeqCst);
                DEPTH_BUSY.store(false, Ordering::SeqCst);
            });
            if r.is_err() {
                DEPTH_BUSY.store(false, Ordering::SeqCst);
            }
        }
        let Some(conv) = ui_probe::alpha_of(CONV.load(Ordering::Relaxed)) else { return };
        if !LIVE.load(Ordering::Relaxed) {
            return;
        }
        // a canary that never got its readback (no EndFrame submit seen inside UiEnd) CANARY_TRIES times: no answer
        if CANARY.load(Ordering::SeqCst) != CANARY_PASSED && CANARY_NOREAD.load(Ordering::SeqCst) >= c4::CANARY_TRIES {
            log_e!("UI", "{} (no ECL of ER's on its queue inside UiEnd: the readback cannot be ordered after ER's last UI list)", c4::fmt_canary41(CanaryRead::Unknown, [0; 4]));
            trip(Miss::Cleared);
            return;
        }
        if let SampleState::Submitted(v) = c4::sample_unpack(RB_STATE.load(Ordering::SeqCst)) {
            self.read_back41(now, v, conv);
        }
        if CANARY.load(Ordering::SeqCst) == CANARY_PASSED {
            if !self.live41_logged {
                self.live41_logged = true;
                self.recheck_t = now;
                let info = ON_INFO.load(Ordering::Relaxed);
                let (w, h, f) = ((info & 0xff_ffff) as u32, ((info >> 24) & 0xff_ffff) as u32, (info >> 48) as u32);
                log_i!(
                    "UI",
                    "{}",
                    c4::fmt_inject41_on(
                        &crate::version_gate::build().map_or("none".into(), |b| mw2er_core::gate::fmt_quad(b.version())),
                        OUT_HDR.load(Ordering::Relaxed),
                        conv,
                        f,
                        w,
                        h
                    )
                );
            }
            if now - self.recheck_t >= c4::RECHECK_S {
                self.recheck_t = now;
                RB_DUE.store(true, Ordering::Relaxed);
            }
        }
    }

    /// The UiEnd readback once its fence passed (task thread): the canary verdict or a re-check vote.
    fn read_back41(&mut self, now: f64, v: u64, conv: LayerAlpha) {
        let t0 = *self.rb_t0.get_or_insert(now);
        let Some(g) = try_lock(&SAMPLER) else { return };
        let Some(s) = g.as_ref() else { return };
        if let Some(hr) = s.device_removed() {
            drop(g);
            trip_err(Miss::DeviceRemoved, format!("readback: GetDeviceRemovedReason 0x{:08x}", hr as u32));
            return;
        }
        if !c4::sample_ready41(SampleState::Submitted(v), s.completed()) {
            drop(g);
            if now - t0 > c4::SAMPLE_STUCK_S {
                trip_err(Miss::Hresult, "readback fence never passed".into());
            }
            return;
        }
        let px = s.read().map(c4::unpack_px);
        drop(g);
        self.rb_t0 = None;
        let kind = RB_KIND.load(Ordering::SeqCst);
        RB_STATE.store(c4::sample_pack(SampleState::Idle), Ordering::SeqCst);
        if kind == 1 {
            self.canary_tries += 1;
            let r = c4::canary_read41(px[0], conv);
            match c4::canary_next41(r, self.canary_tries) {
                Some(true) => {
                    log_i!("UI", "{} (the persistence check is next: one UI frame later the texel must be empty again)", c4::fmt_canary41(r, px[0]));
                    // review C4.1: not live yet; a later armed frame draws nothing and reads the texel back
                    CANARY_NOREAD.store(0, Ordering::SeqCst);
                    CANARY.store(CANARY_CHECK_DUE, Ordering::SeqCst);
                }
                Some(false) => {
                    log_e!("UI", "{} (something clears or overdraws ER's UI layer after UiBegin; the gun stays at Present)", c4::fmt_canary41(r, px[0]));
                    trip(Miss::Cleared);
                }
                None => {
                    log_w!("UI", "{} (try {} of {}; another canary frame)", c4::fmt_canary41(r, px[0]), self.canary_tries, c4::CANARY_TRIES);
                    CANARY.store(CANARY_DUE, Ordering::SeqCst);
                }
            }
        } else if kind == 4 {
            // review C4.1: the persistence check (a later UI frame whose list drew nothing): ER must have cleared the
            // canary texel, or every early gun would stay in the layer (trails; a double gun on a frame without UiBegin)
            self.gone_tries += 1;
            let r = c4::canary_gone41(px[0], conv);
            match c4::gone_next41(r, self.gone_tries) {
                Some(true) => {
                    log_i!("UI", "{} (ER clears its layer between UI frames; the gun is drawn early from now on)", c4::fmt_canary_gone41(r, px[0]));
                    CANARY.store(CANARY_PASSED, Ordering::SeqCst);
                }
                Some(false) => {
                    log_e!(
                        "UI",
                        "{} (check; ER does not clear that texel of its UI layer between UI frames: an early gun would stay in it; the gun stays at Present)",
                        c4::fmt_canary_gone41(r, px[0])
                    );
                    trip(Miss::Cleared);
                }
                None => {
                    log_w!("UI", "{} (try {} of {}; another persistence-check frame)", c4::fmt_canary_gone41(r, px[0]), self.gone_tries, c4::CANARY_TRIES);
                    CANARY.store(CANARY_CHECK_DUE, Ordering::SeqCst);
                }
            }
        } else {
            let vote = c4::recheck_vote41(&px);
            let tripped = self.recheck.step(vote, conv);
            let empty = px.iter().filter(|p| c4::empty_alpha(**p).is_some()).count() as u32;
            log_i!("UI", "{}", c4::fmt_recheck41(vote, conv, empty, self.recheck.disagree));
            if tripped {
                trip(Miss::Alpha);
            }
        }
    }

    /// After a hot disable or a trip: the hooks come out unless the probe still uses them (it then keeps running
    /// with the injection as pure passthrough).
    fn hooks_down(&mut self) {
        if ui_probe::PROBE_ON.load(Ordering::Relaxed) || HOOKS_DOWN.swap(true, Ordering::SeqCst) {
            return;
        }
        let r = teardown(false);
        log_i!(
            "UI",
            "inject teardown hooks=disabled (passthrough; trampolines kept, never freed in-session) inflight={} fence=n/a released=0",
            if r.inflight_zero { "0" } else { "busy" }
        );
    }
}

/// What [`teardown`] did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Teardown {
    /// No thread was counted inside a detour after the 1 s wait.
    pub inflight_zero: bool,
    pub fence_ok: bool,
    pub released: bool,
}

/// Design S1, in `mw2er_core::c4::teardown_plan` order: passthrough, MH_DisableHook, wait for the threads inside a
/// detour (1 s), and with `full` (FreeLibrary) the fence wait (2 s) and the release (only if both waits passed;
/// otherwise the objects are leaked, never freed while possibly in use). No logging here: the unload path runs under
/// the loader lock.
///
/// Review C4: MH_RemoveHook is never called, in-session or at unload. INFLIGHT is only incremented after the jump
/// has landed in a detour, so a render thread preempted between the detour's entry and its `fetch_add` is invisible
/// to the wait (MH_DisableHook only moves threads inside the target prologue or the trampoline); freeing the
/// trampoline would let it call freed memory. The trampolines (and `UI_*_TRAMP`) stay: the detours are passthrough.
/// C4.1: the readback sampler is never released (a static): at unload it is leaked, never freed while possibly in use.
pub fn teardown(full: bool) -> Teardown {
    let mut out = Teardown::default();
    LIVE.store(false, Ordering::SeqCst);
    ui_probe::ACTIVE.store(false, Ordering::SeqCst);
    for h in &ui_probe::HOOKED {
        let t = h.load(Ordering::SeqCst);
        if t != 0 {
            // SAFETY: a target we hooked (MinHook validates it; a second disable is MH_ERROR_DISABLED).
            let _ = unsafe { MH_DisableHook(t as *mut c_void) };
        }
    }
    let t0 = std::time::Instant::now();
    while ui_probe::INFLIGHT.load(Ordering::SeqCst) != 0 && t0.elapsed().as_millis() < 1000 {
        std::thread::yield_now();
    }
    let inflight_zero = ui_probe::INFLIGHT.load(Ordering::SeqCst) == 0;
    out.inflight_zero = inflight_zero;
    if full {
        // the last submitted list's value (a reserved but cancelled value is never signalled)
        let want = SLOT_VALUE.iter().map(|a| a.load(Ordering::SeqCst)).max().unwrap_or(0);
        let t1 = std::time::Instant::now();
        while completed() < want && t1.elapsed().as_millis() < 2000 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        out.fence_ok = completed() >= want;
        out.released = crate::viewmodel::release_early(out.fence_ok && inflight_zero);
    }
    out
}
