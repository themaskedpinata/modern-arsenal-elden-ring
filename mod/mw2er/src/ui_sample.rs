//! C4.1 probe P2 (docs/C4_1_DESIGN.md 2.1 / 2.4): the layer sample. OFF by default (`[ui] probe_layer_sample`, cold,
//! only together with `[ui] probe_scaleform` and the C4.1 sites).
//!
//! Every 5 s, on one UI frame whose UiBegin passes `mw2er_core::c4::sample_gate41` (ER's render thread, multithreaded
//! mode, the HAL queue = the present queue, the 7 pins + count + resource module, a fresh GetDesc: 1 mip, single
//! sample, back-buffer sized, R8G8B8A8 / B8G8R8A8 UNORM), UiBegin's detour records OUR compute list
//! (`mw2er_render::LayerSampler`): 16 texels of ER's UI layer through our SRV into our readback buffer. hudhook's ECL
//! callback splices it directly ahead of the armed Scaleform list (`at == 0`, the same rule as the C4 splice) followed
//! by our fence Signal; the task thread reads the texels once the fence passed, votes the alpha convention
//! (`c4::AlphaTally`) and logs `UI probe41 sample ..`.
//!
//! - D3D12 (design 2.4): the armed list's first command is ER's own `0xC0 -> RENDER_TARGET` barrier on the layer, so the
//!   layer is in `0xC0` (PIXEL | NON_PIXEL_SHADER_RESOURCE) when our list runs right before it on the same queue; our
//!   compute read needs NON_PIXEL. Our list never barriers ER's resource and leaves its state as it found it.
//! - Render thread: atomics, try_lock, one GetDesc, one SRV descriptor write and the list record (no allocation, no
//!   resource creation, no log, no wait). Every object is created by a worker (`mw2er-c41sample`) and never released
//!   in-session (statics are never dropped: at unload it is leaked, never freed while the GPU may use it).
//! - Any D3D12 failure (record, Signal, a fence that never passes, device removed) or a layer format the sample does
//!   not read ends P2 for the session (`UI probe41 sample off why=..`); the probe goes on. A P2 that ended on a D3D12
//!   failure never pins a convention (`c4::sample_alpha41`).
//! - No game memory is written; nothing is drawn; ER's lists are never touched.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use hudhook::hooks::dx12::pre_imgui::{SPLICE_SIGNAL_FAILS, Splice};
use mw2er_core::c4::{self, AlphaTally, Chain41, LayerAlpha, SampleEcl, SampleIn, SampleMiss, SampleState};
use mw2er_render::{LayerSampler, VmError};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandList, ID3D12Fence, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::Interface;

use crate::log::{log_e, log_i, log_w};
use crate::shared::{self, lock, try_lock};
use crate::ui_probe::{self, UiFacts};

/// `[ui] probe_layer_sample` with the C4.1 probe on (boot).
static CFG: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
/// The sampler exists (its raw pointers are published).
static READY: AtomicBool = AtomicBool::new(false);
/// P2 ended for the session; why (`OFF_*`).
static OFF: AtomicBool = AtomicBool::new(false);
static OFF_WHY: AtomicU8 = AtomicU8::new(0);
const OFF_GATE: u8 = 1;
const OFF_HRESULT: u8 = 2;
const OFF_PSO: u8 = 3;
/// The task asks for a sample (5 s); the render thread clears it once one is armed.
static DUE: AtomicBool = AtomicBool::new(false);
/// `c4::sample_pack(SampleState)`.
static STATE: AtomicU64 = AtomicU64::new(0);
static NEXT_VALUE: AtomicU64 = AtomicU64::new(0);
/// The armed frame: Scaleform's list, the HAL queue, the render thread.
static ARMED_LIST: AtomicUsize = AtomicUsize::new(0);
static ARMED_QUEUE: AtomicUsize = AtomicUsize::new(0);
static ARMED_RTID: AtomicU32 = AtomicU32::new(0);
/// The sampler's list and fence (kept alive by SAMPLER, never released in-session).
static LIST_RAW: AtomicUsize = AtomicUsize::new(0);
static FENCE_RAW: AtomicUsize = AtomicUsize::new(0);
static SAMPLER: Mutex<Option<LayerSampler>> = Mutex::new(None);
/// The first D3D12 error (an `VmError::D3d`: no allocation on the render thread).
static ERR: Mutex<Option<VmError>> = Mutex::new(None);
/// Per-frame misses of a due sample (`SampleMiss` order), try_lock failures, armed samples cancelled at UiEnd / split.
static MISS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static LOCKED: AtomicU64 = AtomicU64::new(0);
static CANCELLED: AtomicU64 = AtomicU64::new(0);

const MISSES: [SampleMiss; 8] =
    [SampleMiss::NotDue, SampleMiss::Busy, SampleMiss::NoFrame, SampleMiss::Thread, SampleMiss::Queue, SampleMiss::Chain, SampleMiss::Desc, SampleMiss::Format];

fn miss(m: SampleMiss) {
    if let Some(i) = MISSES.iter().position(|x| *x == m) {
        MISS[i].fetch_add(1, Ordering::Relaxed);
    }
}

/// Ends P2 for the session (any thread; atomics only). The task logs it.
fn off(why: u8) {
    if !OFF.swap(true, Ordering::SeqCst) {
        OFF_WHY.store(why, Ordering::SeqCst);
    }
}

fn off_err(e: VmError) {
    if let Some(mut g) = try_lock(&ERR)
        && g.is_none()
    {
        *g = Some(e);
    }
    off(OFF_HRESULT);
}

/// Boot (`ui_probe::install_ui`, the C4.1 sites passed): P2 is wanted. Returns whether it is on (the `on sample=` value).
pub fn boot(requested: bool) -> bool {
    CFG.store(requested, Ordering::SeqCst);
    requested
}

/// P2 is configured and has not ended (C4.1 PASS I41 waits for its convention while this holds).
pub fn running() -> bool {
    CFG.load(Ordering::Relaxed) && !OFF.load(Ordering::Relaxed)
}

/// UiBegin's detour, after the C4.1 reads (ER's render thread). `layer` = the layer candidate's chain, `res_vt` its
/// resource's vtable and `res_mod` whether the task found that vtable in the D3D12 module, `mt` = `byte [G+0x11c] != 0`.
pub fn on_ui_begin(f: &UiFacts, layer: &Chain41, res_vt: usize, res_mod: bool, mt: bool, rt_ok: bool) {
    if !READY.load(Ordering::Relaxed) || OFF.load(Ordering::Relaxed) || !DUE.load(Ordering::Relaxed) {
        return;
    }
    let cur = STATE.load(Ordering::SeqCst);
    let idle = c4::sample_unpack(cur) == SampleState::Idle;
    // the fresh desc only when everything before it in the gate can pass (one GetDesc per due frame)
    let desc = if idle && layer.ok() && res_mod { ui_probe::desc_if_vtable(layer.res, res_vt) } else { None };
    let bb = ui_probe::BB_SIZE.load(Ordering::Relaxed);
    let g = SampleIn {
        ready: true,
        due: true,
        idle,
        hal: f.hal,
        movies: f.movies,
        list: f.list,
        layer_path: f.path == c4::UiPath::Layer,
        mt,
        rt_ok,
        queue_ok: f.queue != 0 && f.queue == ui_probe::PRESENT_QUEUE.load(Ordering::Relaxed),
        chain_ok: layer.ok(),
        res_mod,
        desc,
        bb: ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32, (bb >> 48) as u32),
    };
    let (fmt, w, h) = match c4::sample_gate41(&g) {
        Ok(x) => x,
        Err(SampleMiss::Format) => {
            miss(SampleMiss::Format);
            off(OFF_GATE);
            return;
        }
        Err(m) => {
            miss(m);
            return;
        }
    };
    let Some(mut sg) = try_lock(&SAMPLER) else {
        LOCKED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let Some(s) = sg.as_mut() else { return };
    let raw = layer.res as *mut c_void;
    // SAFETY: ER's layer resource, validated this UiBegin (7 pins, the D3D12 module's vtable, a fresh GetDesc) and
    // referenced by ER's own armed list of this frame; borrowed (no AddRef / Release), used only for the SRV write.
    let Some(res) = (unsafe { ID3D12Resource::from_raw_borrowed(&raw) }) else { return };
    match s.record(res, DXGI_FORMAT(fmt as i32), &c4::sample_points41(w, h)) {
        Ok(()) => {
            // everything the callback reads is stored BEFORE the state says Armed
            ARMED_LIST.store(f.list, Ordering::SeqCst);
            ARMED_QUEUE.store(f.queue, Ordering::SeqCst);
            ARMED_RTID.store(f.tid, Ordering::SeqCst);
            if STATE.compare_exchange(cur, c4::sample_pack(SampleState::Armed(f.seq)), Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                DUE.store(false, Ordering::Relaxed);
            }
        }
        Err(e) => off_err(e),
    }
}

/// hudhook's ECL callback, when the injection did not splice this call (any thread; atomics only).
pub fn on_ecl(queue: usize, lists: &[usize], seq: u64) -> Option<Splice> {
    if !READY.load(Ordering::Relaxed) || OFF.load(Ordering::Relaxed) {
        return None;
    }
    let cur = STATE.load(Ordering::SeqCst);
    let st = c4::sample_unpack(cur);
    if !matches!(st, SampleState::Armed(_)) || queue != ARMED_QUEUE.load(Ordering::SeqCst) {
        return None;
    }
    // SAFETY: GetCurrentThreadId has no preconditions.
    if unsafe { GetCurrentThreadId() } != ARMED_RTID.load(Ordering::Relaxed) {
        return None;
    }
    match c4::sample_ecl41(st, seq, c4::find_armed(lists, ARMED_LIST.load(Ordering::SeqCst))) {
        SampleEcl::Pass => None,
        SampleEcl::Cancel => {
            if STATE.compare_exchange(cur, c4::sample_pack(SampleState::Idle), Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                CANCELLED.fetch_add(1, Ordering::Relaxed);
            }
            None
        }
        SampleEcl::Splice => {
            let (lraw, fraw) = (LIST_RAW.load(Ordering::SeqCst) as *mut c_void, FENCE_RAW.load(Ordering::SeqCst) as *mut c_void);
            // SAFETY: our own list (a graphics list IS a command list) and fence, kept alive by SAMPLER for the
            // session; borrowed, then cloned (AddRef) into the splice hudhook executes and releases. Taken BEFORE the
            // state moves, so a Submitted sample always has its splice.
            let list = unsafe { ID3D12CommandList::from_raw_borrowed(&lraw) }?.clone();
            let fence = unsafe { ID3D12Fence::from_raw_borrowed(&fraw) }?.clone();
            let value = NEXT_VALUE.fetch_add(1, Ordering::SeqCst) + 1;
            STATE.compare_exchange(cur, c4::sample_pack(SampleState::Submitted(value)), Ordering::SeqCst, Ordering::SeqCst).ok()?;
            Some(Splice { at: 0, list, fence, value })
        }
    }
}

/// UiEnd's detour, after the original: a sample still Armed was never submitted (its list never ran).
pub fn on_ui_end() {
    if !READY.load(Ordering::Relaxed) {
        return;
    }
    let cur = STATE.load(Ordering::SeqCst);
    if let Some(n) = c4::sample_ui_end41(c4::sample_unpack(cur))
        && STATE.compare_exchange(cur, c4::sample_pack(n), Ordering::SeqCst, Ordering::SeqCst).is_ok()
    {
        CANCELLED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Task thread (inside `ui_probe::ProbeLog`): the worker, the 5 s request, the readback and the lines.
pub struct SampleLog {
    due_t: f64,
    submitted_t: Option<f64>,
    n: u32,
    tally: AlphaTally,
    off_logged: bool,
    failed: bool,
}

impl SampleLog {
    pub const fn new() -> Self {
        SampleLog { due_t: 0.0, submitted_t: None, n: 0, tally: AlphaTally { empty: 0, a255: 0, a0: 0, other: 0 }, off_logged: false, failed: false }
    }

    /// The convention for `c4::verdict41` (None while undecided, without P2, or after a D3D12 failure).
    pub fn alpha(&self) -> Option<LayerAlpha> {
        c4::sample_alpha41(&self.tally, self.failed)
    }

    /// Samples read so far.
    pub fn count(&self) -> u32 {
        self.n
    }

    /// `miss=<label:n,..> locked=<n> cancelled=<n>` (the 60 s note while no sample was read).
    pub fn misses(&self) -> String {
        let m: Vec<String> = MISSES
            .iter()
            .zip(&MISS)
            .filter(|(_, a)| a.load(Ordering::Relaxed) > 0)
            .map(|(k, a)| format!("{}:{}", k.label(), a.load(Ordering::Relaxed)))
            .collect();
        format!(
            "miss={} locked={} cancelled={}",
            if m.is_empty() { "none".to_string() } else { m.join(",") },
            LOCKED.load(Ordering::Relaxed),
            CANCELLED.load(Ordering::Relaxed)
        )
    }

    pub fn tick(&mut self, now: f64) {
        if !CFG.load(Ordering::Relaxed) {
            return;
        }
        // the sampler, once the renderer exists (a worker: no D3D12 creation on the task or render thread)
        if shared::VM_READY.load(Ordering::Acquire) && !STARTED.swap(true, Ordering::SeqCst) {
            let r = std::thread::Builder::new().name("mw2er-c41sample".into()).spawn(|| {
                crate::log::set_thread_role("c41sample");
                let made = crate::guard::guard("c41.sample", || {
                    let dev = crate::viewmodel::renderer_device().ok_or_else(|| "no renderer".to_string())?;
                    LayerSampler::new(&dev).map_err(|e| e.to_string())
                });
                match made {
                    Some(Ok(s)) => {
                        LIST_RAW.store(s.list_raw(), Ordering::SeqCst);
                        FENCE_RAW.store(s.fence_raw(), Ordering::SeqCst);
                        let dev = s.device_raw();
                        *lock(&SAMPLER) = Some(s);
                        READY.store(true, Ordering::SeqCst);
                        log_i!("UI", "probe41 sample ready device=0x{dev:x} points={} (every 5 s; our compute list, no barrier on ER's layer)", c4::SAMPLE_N);
                    }
                    Some(Err(e)) => {
                        log_e!("UI", "probe41 sample create failed: {e}");
                        off(OFF_PSO);
                    }
                    None => off(OFF_PSO),
                }
            });
            if let Err(e) = r {
                log_e!("UI", "probe41 sample create failed (thread: {e})");
                off(OFF_PSO);
            }
        }
        if OFF.load(Ordering::SeqCst) {
            if !self.off_logged {
                self.off_logged = true;
                let why = OFF_WHY.load(Ordering::SeqCst);
                self.failed = why == OFF_HRESULT;
                let err = try_lock(&ERR).and_then(|g| g.as_ref().map(|e| e.to_string())).unwrap_or_default();
                let label = match why {
                    OFF_GATE => "gate",
                    OFF_HRESULT => "hresult",
                    OFF_PSO => "pso",
                    _ => "config",
                };
                let detail = format!("(samples={} {}{})", self.n, self.misses(), if err.is_empty() { String::new() } else { format!(" err={err}") });
                if self.failed {
                    log_e!("UI", "{} {detail}", c4::fmt41_sample_off(label));
                } else {
                    log_w!("UI", "{} {detail}", c4::fmt41_sample_off(label));
                }
            }
            return;
        }
        if !READY.load(Ordering::Relaxed) {
            return;
        }
        match c4::sample_unpack(STATE.load(Ordering::SeqCst)) {
            SampleState::Idle => {
                self.submitted_t = None;
                if now - self.due_t >= 5.0 {
                    self.due_t = now;
                    DUE.store(true, Ordering::Relaxed);
                }
            }
            SampleState::Armed(_) => {}
            SampleState::Submitted(v) => self.read_back(now, v),
        }
    }

    fn read_back(&mut self, now: f64, v: u64) {
        let t0 = *self.submitted_t.get_or_insert(now);
        if SPLICE_SIGNAL_FAILS.load(Ordering::Relaxed) > 0 {
            off_err(VmError::D3d { call: "Signal(layer sample, hudhook splice)", hr: 0 });
            return;
        }
        let Some(g) = try_lock(&SAMPLER) else { return };
        let Some(s) = g.as_ref() else { return };
        if let Some(hr) = s.device_removed() {
            drop(g);
            off_err(VmError::D3d { call: "GetDeviceRemovedReason(layer sample)", hr });
            return;
        }
        if !c4::sample_ready41(SampleState::Submitted(v), s.completed()) {
            drop(g);
            if now - t0 > c4::SAMPLE_STUCK_S {
                off_err(VmError::D3d { call: "fence(layer sample) never passed", hr: 0 });
            }
            return;
        }
        let px = s.read().map(c4::unpack_px);
        drop(g);
        let mut empty_now = 0;
        for p in px {
            empty_now += c4::empty_alpha(p).is_some() as u32;
            self.tally.add(p);
        }
        self.n += 1;
        self.submitted_t = None;
        log_i!("UI", "{}", c4::fmt41_sample(self.n, empty_now, &self.tally, &px));
        STATE.store(c4::sample_pack(SampleState::Idle), Ordering::SeqCst);
    }
}
