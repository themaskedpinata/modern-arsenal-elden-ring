//! M7: the nuke post pass inside hudhook's pre-ImGui callback (docs/M7_DESIGN.md 3.7), present thread.
//!
//! [`dispatch`] runs after the viewmodel / AC-130 passes (viewmodel::on_pre_imgui): while the snapshot carries an
//! active post state (flash or vision amount > 0) it records `mw2er_render::nuke::NukePass` into hudhook's list (ER's
//! finished frame -> 1/8 bloom -> MW2 mpnuke / aftermath film + the white flash). It never waits, never blocks
//! (`try_lock` only) and never allocates per frame (the snapshot fields are `Copy`). A loader thread
//! (`mw2er-nukeload`) creates the pass on the game's device ahead of the first sequence.
//!
//! Any failure (D3D, a non-SDR output, a format change, a panic) logs `NUKE FAIL post reason=...` and turns the pass
//! off for the session: the nuke continues without the visual (sounds, kills, the player's death are unaffected).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use hudhook::hooks::dx12::pre_imgui::{LAST_COLOR_SPACE, PreImguiCtx};
use mw2er_core::nuke_fx::constants;
use mw2er_core::nuke_logic::PostState;
use mw2er_render::nuke::{NukePass, NukeTarget};
use mw2er_render::thermal::{BUSY_PREFIX, scene_formats};
use mw2er_render::{COLORSPACE_PQ, base_format, format_name};
use windows::Win32::Graphics::Direct3D12::ID3D12Device;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::Interface;

use crate::guard::guard;
use crate::log::{log_e, log_i, log_w, set_thread_role};
use crate::shared::{self, SNAPSHOT, lock, now_s, try_lock};

static PASS: Mutex<Option<NukePass>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);
pub static DISABLED: AtomicBool = AtomicBool::new(false);
static LOADER: AtomicBool = AtomicBool::new(false);
static DEVICE: AtomicUsize = AtomicUsize::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static BUSY_WARNED: AtomicBool = AtomicBool::new(false);
static LAST_GPU_LOG_MS: AtomicU64 = AtomicU64::new(0);
/// Max GPU ms of the current sequence (logged as `NUKE post gpu_ms=` when the pass goes idle).
static SEQ_MAX_US: AtomicU64 = AtomicU64::new(0);
static SEQ_ACTIVE: AtomicBool = AtomicBool::new(false);
const GPU_LOG_EVERY_MS: u64 = 2_000;
const SNAPSHOT_MAX_AGE_S: f64 = 0.5;

/// Logs one `NUKE FAIL post` line and turns the pass off for the session (the nuke continues).
pub fn fail(reason: &str) {
    if !DISABLED.swap(true, Ordering::SeqCst) {
        log_e!("NUKE", "FAIL post reason={reason}; the nuke continues without the flash / white-out");
    }
}

#[derive(Clone, Copy)]
struct FrameIn {
    t: f64,
    in_world: bool,
    post: PostState,
    vis: mw2er_core::nuke_fx::Vis,
}

fn frame_in() -> Option<FrameIn> {
    let s = try_lock(&SNAPSHOT)?;
    Some(FrameIn { t: s.t, in_world: s.in_world, post: s.nuke.post, vis: s.nuke.vis })
}

/// Records the nuke pass when the snapshot has an active post state. Returns true when it drew.
pub fn dispatch(ctx: &PreImguiCtx<'_>) -> bool {
    if DISABLED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(cfg) = shared::try_config() else { return false };
    if !cfg.nuke.enabled {
        return false;
    }
    if !LOADER.swap(true, Ordering::SeqCst) {
        start_loader(ctx);
    }
    let Some(a) = frame_in() else { return false };
    let now = now_s();
    // the snapshot gate: stale (loading, the death screen's load, a hang) or not in world -> ER's image shows
    if !a.post.active() || now - a.t > SNAPSHOT_MAX_AGE_S || !a.in_world {
        if SEQ_ACTIVE.swap(false, Ordering::Relaxed) {
            let us = SEQ_MAX_US.swap(0, Ordering::Relaxed);
            log_i!("NUKE", "post gpu_ms={:.4} (max of the sequence) frames={}", us as f64 / 1000.0, DRAWN.load(Ordering::Relaxed));
        }
        return false;
    }
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    if ctx.device.as_raw() as usize != DEVICE.load(Ordering::Relaxed) {
        fail("d3d: the swap chain's device changed");
        return false;
    }
    if LAST_COLOR_SPACE.load(Ordering::Relaxed) == COLORSPACE_PQ || scene_formats(ctx.rtv_format).is_none() {
        fail(&format!("output fmt={} colorspace={} is not SDR sRGB (HDR on?)", format_name(ctx.rtv_format), LAST_COLOR_SPACE.load(Ordering::Relaxed)));
        return false;
    }
    // SAFETY: plain query on the live back buffer hudhook handed us (no reference is kept).
    let live = unsafe { ctx.render_target.GetDesc() }.Format;
    if base_format(live) != base_format(ctx.rtv_format) {
        fail(&format!("back-buffer format changed ({} != hudhook RTV {})", format_name(live), format_name(ctx.rtv_format)));
        return false;
    }
    let Some(mut pg) = try_lock(&PASS) else { return false };
    let Some(pass) = pg.as_mut() else { return false };
    let slot = ctx.frame_index % ctx.num_frames.max(1);
    let consts = constants(&a.vis, &a.post, ctx.width, ctx.height);
    let target = NukeTarget { resource: ctx.render_target, rtv: ctx.rtv, rtv_format: ctx.rtv_format, width: ctx.width, height: ctx.height };
    match pass.record(ctx.command_list, &target, slot, &consts, true) {
        Ok(()) => {
            SEQ_ACTIVE.store(true, Ordering::Relaxed);
            for x in pass.take_present_allocs() {
                log_w!("NUKE", "WARN created {x} on the present thread (size/format change: one-off hitch)");
            }
            if DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
                log_i!("NUKE", "post first frame {}x{} fmt={} flash={:.2} amount={:.2}", ctx.width, ctx.height, format_name(ctx.rtv_format), a.post.flash, a.post.amount);
            }
            let t = pass.take_timings();
            for v in &t {
                SEQ_MAX_US.fetch_max((*v as f64 * 1000.0) as u64, Ordering::Relaxed);
            }
            let now_ms = (now * 1000.0) as u64;
            if !t.is_empty() && now_ms.saturating_sub(LAST_GPU_LOG_MS.load(Ordering::Relaxed)) >= GPU_LOG_EVERY_MS {
                LAST_GPU_LOG_MS.store(now_ms, Ordering::Relaxed);
                log_i!("NUKE", "post gpu_ms last={:.4} flash={:.2} amount={:.2} blend={:.2}", t[t.len() - 1], a.post.flash, a.post.amount, a.post.blend);
            }
            true
        }
        Err(e) if e.to_string().starts_with(BUSY_PREFIX) => {
            if !BUSY_WARNED.swap(true, Ordering::Relaxed) {
                log_w!("NUKE", "WARN post skipped a frame: {e}");
            }
            false
        }
        Err(e) => {
            drop(pg);
            fail(&format!("record {e}"));
            false
        }
    }
}

fn start_loader(ctx: &PreImguiCtx<'_>) {
    let device: ID3D12Device = ctx.device.clone();
    DEVICE.store(ctx.device.as_raw() as usize, Ordering::Relaxed);
    let fmt = ctx.rtv_format;
    let (w, h) = (ctx.width, ctx.height);
    let num_frames = ctx.num_frames.max(1);
    // SAFETY: plain query on the game's present queue.
    let ts_freq = unsafe { ctx.command_queue.GetTimestampFrequency() }.unwrap_or(0);
    let spawned = std::thread::Builder::new().name("mw2er-nukeload".into()).spawn(move || {
        set_thread_role("nukeload");
        match guard("nuke.loader", || load(device, fmt, (w, h), num_frames, ts_freq)) {
            Some(Ok(())) => {}
            Some(Err(e)) => fail(&e),
            None => fail("panic in the nuke loader thread"),
        }
    });
    if let Err(e) = spawned {
        fail(&format!("loader thread: {e}"));
    }
}

fn load(device: ID3D12Device, fmt: DXGI_FORMAT, size: (u32, u32), num_frames: usize, ts_freq: u64) -> Result<(), String> {
    let t0 = Instant::now();
    let mut pass = NukePass::new(&device, num_frames, ts_freq).map_err(|e| e.to_string())?;
    if scene_formats(fmt).is_some() {
        pass.prepare(fmt, size.0, size.1).map_err(|e| e.to_string())?;
    }
    *lock(&PASS) = Some(pass);
    READY.store(true, Ordering::Release);
    log_i!(
        "NUKE",
        "post ready ms={} fmt={} {}x{} ts_freq={ts_freq} (shaders {}..)",
        t0.elapsed().as_millis(),
        format_name(fmt),
        size.0,
        size.1,
        &mw2er_render::nuke::NUKE_PS_SHA256[..12]
    );
    Ok(())
}
