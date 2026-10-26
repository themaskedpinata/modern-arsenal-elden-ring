//! M4: the thermal pass inside hudhook's pre-ImGui callback (docs/M4_DESIGN.md section 6), present thread.
//!
//! - [`dispatch`] runs first in the viewmodel's callback (`viewmodel::callback_body`): while an AC-130 ride is Active
//!   it records the run-8 round tracers (`mw2er_render::tracer::TracerPass`, additive, into the back buffer) and then
//!   `mw2er_render::thermal::ThermalPass` into hudhook's list (ER's finished frame + tracers -> MW2 film + overlay
//!   + grain) and returns true so the viewmodel is never drawn in the gunship. It never waits, never blocks
//!   (`try_lock` only) and never allocates per frame (the snapshot fields are copied, the instances go into a fixed
//!   array).
//! - Tracers BEFORE the thermal pass: the film sees a round as a hot object, white in WHOT and black in BHOT like MW2's
//!   thermal view, with the same grain and the gun overlay drawn over it (drawing after the film would put an
//!   always-white streak over a black-hot image).
//! - A loader thread (`mw2er-thload`) creates the tracer pass, reads `converted\ac130\*.vmtex`, creates the thermal
//!   pass on the game's device (PSO + scene copy for the swap chain's format and size) and hands both over.
//! - Any failure (data, D3D, a non-SDR output (PQ / scRGB), a format change, a panic) logs `AC130 FAIL thermal
//!   reason=...` (or `AC130 FAIL tracers reason=...`) and turns that pass off for the session: the gunship continues
//!   with the ImGui HUD only (and ImGui's simple tracer lines).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use hudhook::hooks::dx12::pre_imgui::{LAST_COLOR_SPACE, PreImguiCtx};
use mw2er_core::ac130_data::OVERLAY_FILES;
use mw2er_core::ac130_hud::{blink_alpha, box43};
use mw2er_core::ac130_tracer::{MAX_INSTANCES, TracerInstance, TracerRing, TracerView, build_instances};
use mw2er_core::thermal::{Film, fade_at, film_at, grain_frame, grain_offset};
use mw2er_core::vm_assets::parse_tex;
use mw2er_render::thermal::{BUSY_PREFIX, ThermalParams, ThermalPass, ThermalTarget, scene_formats};
use mw2er_render::tracer::{TracerPass, TracerTarget};
use mw2er_render::{COLORSPACE_PQ, base_format, format_name};
use windows::Win32::Graphics::Direct3D12::ID3D12Device;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::Interface;

use crate::cam_hook;
use crate::data::AC_DATA;
use crate::guard::guard;
use crate::log::{log_e, log_i, log_w, set_thread_role};
use crate::shared::{self, SNAPSHOT, lock, now_s, try_lock};

static PASS: Mutex<Option<ThermalPass>> = Mutex::new(None);
static READY: AtomicBool = AtomicBool::new(false);
pub static DISABLED: AtomicBool = AtomicBool::new(false);
static LOADER: AtomicBool = AtomicBool::new(false);
static DEVICE: AtomicUsize = AtomicUsize::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);
static BUSY_WARNED: AtomicBool = AtomicBool::new(false);
static LAST_GPU_LOG_MS: AtomicU64 = AtomicU64::new(0);
const GPU_LOG_EVERY_MS: u64 = 10_000;
const SNAPSHOT_MAX_AGE_S: f64 = 0.5;

/// Run 8 tracers: the pass + its per-frame instance scratch (fixed array: no allocation on the present thread).
struct Tracers {
    pass: TracerPass,
    scratch: [TracerInstance; MAX_INSTANCES],
}
static TRACERS: Mutex<Option<Tracers>> = Mutex::new(None);
static TRACERS_READY: AtomicBool = AtomicBool::new(false);
static TRACERS_DISABLED: AtomicBool = AtomicBool::new(false);
static TRACER_FRAMES: AtomicU64 = AtomicU64::new(0);
static TRACER_MAX: AtomicUsize = AtomicUsize::new(0);
/// True while the D3D12 tracer pass draws the rounds (the ImGui HUD then draws none; its lines are the fallback).
pub static TRACERS_GPU: AtomicBool = AtomicBool::new(false);

/// Logs one `AC130 FAIL thermal` line and turns the pass off for the session (the gunship continues).
pub fn fail(reason: &str) {
    if !DISABLED.swap(true, Ordering::SeqCst) {
        log_e!("AC130", "FAIL thermal reason={reason}; the gunship continues with the ImGui HUD only");
    }
}

/// Logs one `AC130 FAIL tracers` line and turns the tracer pass off for the session (ImGui lines take over).
pub fn fail_tracers(reason: &str) {
    TRACERS_GPU.store(false, Ordering::SeqCst);
    if !TRACERS_DISABLED.swap(true, Ordering::SeqCst) {
        log_e!("AC130", "FAIL tracers reason={reason}; ImGui draws simple tracer lines instead");
    }
}

/// The snapshot fields one frame needs, copied under the lock (all `Copy`: no allocation per frame).
#[derive(Clone, Copy)]
struct FrameIn {
    t: f64,
    in_world: bool,
    menu_open: bool,
    active: bool,
    thermal: bool,
    gun: usize,
    reloading: [bool; 3],
    grain: bool,
    luma: [f32; 3],
    gamma: f32,
    film_from: Film,
    film_to: Film,
    film_t0: f64,
    film_dur: f64,
    t0: f64,
    fade_s: f64,
    tracers: TracerRing,
}

fn frame_in() -> Option<FrameIn> {
    let s = try_lock(&SNAPSHOT)?;
    let a = &s.ac130;
    Some(FrameIn {
        t: s.t,
        in_world: s.in_world,
        menu_open: s.menu_open,
        active: a.active,
        thermal: a.thermal,
        gun: a.gun,
        reloading: a.reloading,
        grain: a.grain,
        luma: a.luma,
        gamma: a.gamma,
        film_from: a.film_from,
        film_to: a.film_to,
        film_t0: a.film_t0,
        film_dur: a.film_dur,
        t0: a.t0,
        fade_s: a.fade_s,
        tracers: a.tracers,
    })
}

/// True while an AC-130 ride is Active (the viewmodel must not draw); records the tracers and the thermal pass when
/// it can.
pub fn dispatch(ctx: &PreImguiCtx<'_>) -> bool {
    let Some(cfg) = shared::try_config() else { return false };
    if !cfg.ac130.enabled {
        return false;
    }
    if (cfg.ac130.thermal || cfg.ac130.tracers.enabled) && !LOADER.swap(true, Ordering::SeqCst) {
        start_loader(ctx, cfg.ac130.thermal);
    }
    let Some(a) = frame_in() else { return false };
    if !a.active {
        return false;
    }
    let now = now_s();
    // the snapshot gate: stale (loading, a hang), menu or not in world -> ER's image shows (still no viewmodel)
    if now - a.t > SNAPSHOT_MAX_AGE_S || !a.in_world || a.menu_open {
        return true;
    }
    if ctx.device.as_raw() as usize != DEVICE.load(Ordering::Relaxed) {
        fail("d3d: the swap chain's device changed");
        fail_tracers("d3d: the swap chain's device changed");
        return true;
    }
    let slot = ctx.frame_index % ctx.num_frames.max(1);
    // 1. tracers (into ER's frame; the thermal film then sees them as hot rounds)
    if cfg.ac130.tracers.enabled && TRACERS_READY.load(Ordering::Acquire) && !TRACERS_DISABLED.load(Ordering::Relaxed) {
        record_tracers(ctx, &a, &cfg.ac130.tracers.params(), slot, now);
    } else {
        TRACERS_GPU.store(false, Ordering::Relaxed);
    }
    // 2. the thermal pass
    if !(cfg.ac130.thermal && a.thermal) || DISABLED.load(Ordering::Relaxed) || !READY.load(Ordering::Acquire) {
        return true;
    }
    if LAST_COLOR_SPACE.load(Ordering::Relaxed) == COLORSPACE_PQ || scene_formats(ctx.rtv_format).is_none() {
        fail(&format!(
            "output fmt={} colorspace={} is not SDR sRGB (HDR on?)",
            format_name(ctx.rtv_format),
            LAST_COLOR_SPACE.load(Ordering::Relaxed)
        ));
        return true;
    }
    // SAFETY: plain query on the live back buffer hudhook handed us (no reference is kept).
    let live = unsafe { ctx.render_target.GetDesc() }.Format;
    if base_format(live) != base_format(ctx.rtv_format) {
        fail(&format!("back-buffer format changed ({} != hudhook RTV {})", format_name(live), format_name(ctx.rtv_format)));
        return true;
    }
    let Some(d) = AC_DATA.get() else { return true };
    let Some(mut pg) = try_lock(&PASS) else { return true };
    let Some(pass) = pg.as_mut() else { return true };
    let gun = a.gun.min(2);
    let ms = now * 1000.0;
    let fc = d.hud.grain_forecolor;
    let p = ThermalParams {
        film: film_at(&a.film_from, &a.film_to, a.film_t0, a.film_dur, now),
        luma: a.luma,
        gamma: a.gamma,
        fade: fade_at(now - a.t0, a.fade_s),
        box_px: box43(ctx.width as f32, ctx.height as f32),
        overlay: Some(gun),
        ov_alpha: if a.reloading[gun] { blink_alpha(ms, d.hud.blink_ms, d.hud.blink_amp) } else { 1.0 },
        grain: a.grain,
        grain_offset: grain_offset(grain_frame(now), 512),
        grain_scale: [1.0, 1.0],
        grain_tint: [fc[0], fc[1], fc[2]],
        grain_alpha: fc[3],
        timing: true,
        test_unbalanced: false,
    };
    let target = ThermalTarget { resource: ctx.render_target, rtv: ctx.rtv, rtv_format: ctx.rtv_format, width: ctx.width, height: ctx.height };
    match pass.record(ctx.command_list, &target, slot, &p) {
        Ok(()) => {
            for x in pass.take_present_allocs() {
                log_w!("AC130", "WARN created {x} on the present thread (size/format change: one-off hitch)");
            }
            if DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
                log_i!("AC130", "thermal first frame {}x{} fmt={} gun={gun}", ctx.width, ctx.height, format_name(ctx.rtv_format));
            }
            let now_ms = (now * 1000.0) as u64;
            if now_ms.saturating_sub(LAST_GPU_LOG_MS.load(Ordering::Relaxed)) >= GPU_LOG_EVERY_MS {
                LAST_GPU_LOG_MS.store(now_ms, Ordering::Relaxed);
                let mut t = pass.take_timings();
                if !t.is_empty() {
                    t.sort_by(|a, b| a.total_cmp(b));
                    let q = |f: f64| t[((t.len() - 1) as f64 * f).round() as usize];
                    log_i!("AC130", "gpu_ms p50={:.4} p95={:.4} max={:.4} n={}", q(0.5), q(0.95), t[t.len() - 1], t.len());
                }
                drop(pg);
                log_tracer_timing();
            }
        }
        Err(e) if e.to_string().starts_with(BUSY_PREFIX) => {
            if !BUSY_WARNED.swap(true, Ordering::Relaxed) {
                log_w!("AC130", "WARN thermal skipped a frame: {e}");
            }
        }
        Err(e) => {
            drop(pg);
            fail(&format!("record {e}"));
        }
    }
    true
}

/// Projects the rounds through the gunship camera the detour wrote this frame and draws them (no allocation).
fn record_tracers(ctx: &PreImguiCtx<'_>, a: &FrameIn, params: &mw2er_core::ac130_tracer::TracerParams, slot: usize, now: f64) {
    let Some((pos, _, rows, vfov)) = cam_hook::gunship_view() else { return };
    let Some(mut g) = try_lock(&TRACERS) else { return };
    let Some(t) = g.as_mut() else { return };
    let view = TracerView { pos: [pos.x, pos.y, pos.z], rows, vfov, width: ctx.width as f32, height: ctx.height as f32 };
    let n = build_instances(&a.tracers, now, &view, params, &mut t.scratch);
    let target = TracerTarget { resource: ctx.render_target, rtv: ctx.rtv, rtv_format: ctx.rtv_format, width: ctx.width, height: ctx.height };
    match t.pass.record(ctx.command_list, &target, slot, &t.scratch[..n], n > 0) {
        Ok(()) => {
            TRACERS_GPU.store(true, Ordering::Relaxed);
            for x in t.pass.take_present_allocs() {
                log_w!("AC130", "WARN created {x} on the present thread (format change: one-off hitch)");
            }
            if n > 0 {
                TRACER_MAX.fetch_max(n, Ordering::Relaxed);
                if TRACER_FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
                    log_i!("AC130", "tracers first frame instances={n} {}x{} fmt={}", ctx.width, ctx.height, format_name(ctx.rtv_format));
                }
            }
        }
        Err(e) => {
            drop(g);
            fail_tracers(&format!("record {e}"));
        }
    }
}

/// `AC130 tracer gpu_ms ...` next to the thermal line (every 10 s while drawing).
fn log_tracer_timing() {
    let Some(mut g) = try_lock(&TRACERS) else { return };
    let Some(t) = g.as_mut() else { return };
    let mut s = t.pass.take_timings();
    if s.is_empty() {
        return;
    }
    s.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f64| s[((s.len() - 1) as f64 * f).round() as usize];
    log_i!(
        "AC130",
        "tracer gpu_ms p50={:.4} p95={:.4} max={:.4} n={} frames={} max_instances={}",
        q(0.5),
        q(0.95),
        s[s.len() - 1],
        s.len(),
        TRACER_FRAMES.load(Ordering::Relaxed),
        TRACER_MAX.load(Ordering::Relaxed)
    );
}

fn start_loader(ctx: &PreImguiCtx<'_>, thermal: bool) {
    let device: ID3D12Device = ctx.device.clone();
    DEVICE.store(ctx.device.as_raw() as usize, Ordering::Relaxed);
    let fmt = ctx.rtv_format;
    let (w, h) = (ctx.width, ctx.height);
    let num_frames = ctx.num_frames.max(1);
    // SAFETY: plain query on the game's present queue.
    let ts_freq = unsafe { ctx.command_queue.GetTimestampFrequency() }.unwrap_or(0);
    let spawned = std::thread::Builder::new().name("mw2er-thload".into()).spawn(move || {
        set_thread_role("thload");
        match guard("tracer.loader", || load_tracers(&device, fmt, num_frames, ts_freq)) {
            Some(Ok(())) => {}
            Some(Err(e)) => fail_tracers(&e),
            None => fail_tracers("panic in the tracer loader"),
        }
        if !thermal {
            return;
        }
        match guard("thermal.loader", || load(device, fmt, (w, h), num_frames, ts_freq)) {
            Some(Ok(())) => {}
            Some(Err(e)) => fail(&e),
            None => fail("panic in the thermal loader thread"),
        }
    });
    if let Err(e) = spawned {
        fail(&format!("loader thread: {e}"));
        fail_tracers(&format!("loader thread: {e}"));
    }
}

fn load_tracers(device: &ID3D12Device, fmt: DXGI_FORMAT, num_frames: usize, ts_freq: u64) -> Result<(), String> {
    let t0 = Instant::now();
    let mut pass = TracerPass::new(device, num_frames, ts_freq).map_err(|e| e.to_string())?;
    pass.prepare(fmt).map_err(|e| e.to_string())?;
    *lock(&TRACERS) = Some(Tracers { pass, scratch: [TracerInstance::default(); MAX_INSTANCES] });
    TRACERS_READY.store(true, Ordering::Release);
    log_i!(
        "AC130",
        "tracers ready ms={} max_instances={MAX_INSTANCES} fmt={} frames={num_frames} (shaders {}..)",
        t0.elapsed().as_millis(),
        format_name(fmt),
        &mw2er_render::tracer::TRACER_PS_SHA256[..12]
    );
    Ok(())
}

fn load(device: ID3D12Device, fmt: DXGI_FORMAT, size: (u32, u32), num_frames: usize, ts_freq: u64) -> Result<(), String> {
    let t0 = Instant::now();
    let d = AC_DATA.get().ok_or("converted\\ac130 data not loaded (DATA FAIL ac130 in the log)")?;
    let mut texs = Vec::new();
    for f in OVERLAY_FILES {
        let p = d.dir.join(f);
        let b = std::fs::read(&p).map_err(|e| format!("assets: {}: {e}", p.display()))?;
        texs.push(parse_tex(&b).map_err(|e| format!("assets: {f}: {e}"))?);
    }
    let mut pass = ThermalPass::new(&device, num_frames, ts_freq).map_err(|e| e.to_string())?;
    let bytes = pass.upload_overlays(&texs).map_err(|e| e.to_string())?;
    if scene_formats(fmt).is_some() {
        pass.prepare(fmt, size.0, size.1).map_err(|e| e.to_string())?;
    }
    *lock(&PASS) = Some(pass);
    READY.store(true, Ordering::Release);
    log_i!(
        "AC130",
        "thermal ready ms={} overlays=4 bytes={bytes} fmt={} {}x{} ts_freq={ts_freq} (shaders {}..)",
        t0.elapsed().as_millis(),
        format_name(fmt),
        size.0,
        size.1,
        &mw2er_render::thermal::THERMAL_PS_SHA256[..12]
    );
    Ok(())
}
