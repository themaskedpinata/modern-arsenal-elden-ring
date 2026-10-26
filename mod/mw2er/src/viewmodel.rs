//! M3: the MW2 viewmodel drawn inside Elden Ring's frame (docs/M3_DESIGN.md sections 4.2-4.6).
//!
//! - [`install`] registers the pre-ImGui callback of the vendored hudhook (patch 0001) before hudhook is applied.
//! - The callback runs on the present thread inside `D3D12RenderEngine::render`, after hudhook bound the back
//!   buffer and before ImGui draws: it poses the viewmodel from the task's published `VmState` at its own clock
//!   and records it with `mw2er_render::Renderer` into hudhook's command list. It never waits, never blocks
//!   (`try_lock` only) and is wrapped in `guard` (hudhook's patch adds a second `catch_unwind`).
//! - On the first callback a loader thread (`mw2er-vmload`) reads and validates `converted\viewmodel\`, creates
//!   the renderer on the game's device (PSOs for the back-buffer format, its own upload queue) and hands it over.
//! - Any failure (assets, D3D, unknown output format, panic, record error) logs `VM FAIL reason=...`, sets
//!   `VM_DISABLED` and leaves the M2 behaviour (HUD, firing, input) untouched.
//! - C4 (docs/C4_UI_DESIGN.md 3.3, `[ui] inject_viewmodel`): [`record_early`] records the same world guns + gun /
//!   overlay into the early pass's list from ER's UiBegin (ui_inject.rs), and the Present path skips them only on a
//!   frame whose early list was spliced (`ui_inject::present_frame`). With the injection off nothing here changes.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use hudhook::hooks::dx12::pre_imgui::{LAST_COLOR_SPACE, PRE_IMGUI, PRE_IMGUI_PANICS, PreImguiCtx};
use mw2er_core::vm_anim::{DrawGate, vm_should_draw};
use mw2er_core::vm_assets::VmAssets;
use mw2er_core::vm_pose::{PoseScratch, evaluate};
use mw2er_core::vm_proj::{ProjParams, build};
use mw2er_core::c4::{EarlyMode, EarlyOut, Wants};
use mw2er_render::{
    Cutout, DebugMode, EarlyPass, EarlyTarget, FrameParams, LightParams, MAX_PALETTE, MAX_WORLD_DRAWS, OutputMode, Renderer, Target, Variant, VmError, WorldDraw,
    WorldFrame, base_format, format_name, select_output,
};
use windows::Win32::Graphics::Direct3D12::ID3D12Device;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::Interface;

use crate::guard::guard;
use crate::log::{log_e, log_i, log_w, set_thread_role};
use crate::shared::{self, SNAPSHOT, VM_DISABLED, VM_READY, lock, now_s, try_lock};

static RENDERER: Mutex<Option<Renderer>> = Mutex::new(None);
static ASSETS: OnceLock<Arc<VmAssets>> = OnceLock::new();
/// M6: the arsenal's bundles by gun slot (`None` = that gun failed to load).
static GUNS: OnceLock<Vec<Option<Arc<VmAssets>>>> = OnceLock::new();
static ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();
static LOADER_STARTED: AtomicBool = AtomicBool::new(false);
/// The device the renderer was created on (COM pointer value); a different device later disables the viewmodel.
static DEVICE: AtomicUsize = AtomicUsize::new(0);
static OFF_LOGGED: AtomicBool = AtomicBool::new(false);
/// C3: the UI yield's signal bits last seen by the present thread (one `VM yield draw` line per change).
static YIELD_WHY_SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Callbacks seen (for the "no frames" warning) and frames actually drawn.
pub static CALLBACKS: AtomicU64 = AtomicU64::new(0);
pub static DRAWN: AtomicU64 = AtomicU64::new(0);
/// C3 PASS C: frames with world guns drawn, and the world-gun pass turned off after a record error (the viewmodel
/// keeps drawing).
pub static WORLD_DRAWN: AtomicU64 = AtomicU64::new(0);
/// C6 depth fallback: world-gun frames drawn with body capsules (the first one is logged).
pub static WORLD_OCCL_DRAWN: AtomicU64 = AtomicU64::new(0);
static WORLD_DISABLED: AtomicBool = AtomicBool::new(false);
/// Review C4: the early path turned the world guns off (render thread, no logging there): the task logs it once.
static WORLD_EARLY_FAIL: AtomicBool = AtomicBool::new(false);
static WORLD_EARLY_ERR: Mutex<String> = Mutex::new(String::new());
/// C4: the early pass (only with `[ui] inject_viewmodel`; created by [`create_early`] once the renderer is ready).
static EARLY: Mutex<Option<EarlyPass>> = Mutex::new(None);
/// M6 scope overlay uploaded (C4's gate asks without the renderer lock).
static OVERLAY_READY: AtomicBool = AtomicBool::new(false);
/// C4.1 (docs/C4_1_DESIGN.md 3.5): what [`record_early41`] needs of the snapshot, copied into buffers whose capacity
/// the worker reserved ([`create_early`]), and the render thread's own pose scratch (reserved for MAX_PALETTE bones):
/// nothing on ER's render thread allocates.
static EARLY_SNAP: Mutex<EarlySnap> = Mutex::new(EarlySnap::empty());
static EARLY_SCRATCH: Mutex<PoseScratch> = Mutex::new(PoseScratch::empty());
/// C4.1: a world-gun failure on the render thread (stored without formatting; the task logs it).
static WORLD_EARLY_VMERR: Mutex<Option<VmError>> = Mutex::new(None);

/// C4.1: the snapshot fields the early record reads (Copy, plus the world guns in a reserved vector).
struct EarlySnap {
    t: f64,
    in_world: bool,
    menu_open: bool,
    ac130: bool,
    scope: f32,
    vm: mw2er_core::vm_anim::VmState,
    vm_offset_in: [f32; 3],
    vfov_raw: f32,
    view_pos: [f32; 3],
    cam_rows: [[f32; 3]; 3],
    world_guns: Vec<crate::coop_vis::WorldGunDraw>,
}

impl EarlySnap {
    const fn empty() -> Self {
        EarlySnap {
            t: 0.0,
            in_world: false,
            menu_open: false,
            ac130: false,
            scope: 0.0,
            vm: mw2er_core::vm_anim::VmState::hidden(),
            vm_offset_in: [0.0; 3],
            vfov_raw: 0.0,
            view_pos: [0.0; 3],
            cam_rows: [[0.0; 3]; 3],
            world_guns: Vec::new(),
        }
    }

    /// Copies `s` without allocating: at most the reserved capacity of world guns (never more than MAX_WORLD_DRAWS,
    /// the renderer's own limit).
    fn copy_from(&mut self, s: &crate::shared::HudSnapshot) {
        self.t = s.t;
        self.in_world = s.in_world;
        self.menu_open = s.menu_open;
        self.ac130 = s.ac130.active;
        self.scope = s.scope;
        self.vm = s.vm;
        self.vm_offset_in = s.vm_offset_in;
        self.vfov_raw = s.vfov_raw;
        self.view_pos = s.view_pos;
        self.cam_rows = s.cam_rows;
        let n = s.world_guns.len().min(self.world_guns.capacity()).min(MAX_WORLD_DRAWS);
        self.world_guns.clear();
        self.world_guns.extend_from_slice(&s.world_guns[..n]);
    }
}

struct PresentState {
    out: Option<(DXGI_FORMAT, i32, OutputMode)>,
    last_log: f64,
    panics_seen: u64,
}

static PRESENT: Mutex<PresentState> = Mutex::new(PresentState { out: None, last_log: 0.0, panics_seen: 0 });

thread_local! {
    static SCRATCH: RefCell<PoseScratch> = RefCell::new(PoseScratch::default());
}

/// The converted assets once the loader finished (the task builds its `VmAnim` from their clip durations).
pub fn assets() -> Option<Arc<VmAssets>> {
    ASSETS.get().cloned()
}

/// M6: one gun's bundle once the loader finished (`None` while loading, for a failed gun, or without the arsenal).
pub fn gun_assets(slot: usize) -> Option<Arc<VmAssets>> {
    GUNS.get().and_then(|g| g.get(slot).cloned().flatten())
}

/// The bundle the present thread poses for `slot` (the arsenal's, else the M3 single gun).
fn assets_for(slot: usize) -> Option<Arc<VmAssets>> {
    match GUNS.get() {
        Some(g) => g.get(slot).cloned().flatten(),
        None => ASSETS.get().cloned(),
    }
}

/// Logs one `VM FAIL` line and disables the viewmodel for the session (M2 keeps running).
fn fail(reason: &str) {
    if !VM_DISABLED.swap(true, Ordering::SeqCst) {
        log_e!("VM", "FAIL reason={reason}; viewmodel off for this session, MW2 mode unaffected");
    }
}

/// Registers the pre-ImGui callback (boot thread, before `Hudhook::builder()`); `root` resolves `assets_dir`.
pub fn install(root: PathBuf) {
    let cfg = shared::config();
    // M4: the same callback also carries the AC-130 thermal pass, so it is registered when either is enabled.
    if !cfg.viewmodel.enabled && !cfg.ac130.enabled && !cfg.nuke.enabled {
        log_i!("VM", "off (config)");
        OFF_LOGGED.store(true, Ordering::SeqCst);
        return;
    }
    let dir = root.join(&cfg.viewmodel.assets_dir);
    if cfg.viewmodel.enabled {
        let _ = ASSETS_DIR.set(dir.clone());
    }
    if PRE_IMGUI.set(Box::new(on_pre_imgui)).is_err() {
        log_w!("VM", "WARN pre-ImGui callback already set");
    }
    log_i!("VM", "pre-ImGui callback registered; assets {} (viewmodel={} ac130 thermal={})", dir.display(), cfg.viewmodel.enabled as u8, cfg.ac130.enabled as u8);
}

/// hudhook's pre-ImGui hook (present thread).
fn on_pre_imgui(ctx: &PreImguiCtx<'_>) {
    if guard("vm.callback", || callback_body(ctx)).is_none() {
        fail("panic in the present callback");
    }
    // C3 / C4: the UI probe compares ER's Scaleform thread / HAL device / queue with ours (atomics only; read-only)
    if crate::ui_probe::any_installed() {
        let _ = guard("ui.probe_present", || crate::ui_probe::on_present(ctx));
    }
    // C6 depth probe: our back buffer's vtable / size for its module check and desc rule (atomics only)
    if crate::er_depth::on() {
        // SAFETY: the first qword of a COM object is its vtable (non-faulting read).
        let vt = unsafe { crate::game::read::<usize>(windows::core::Interface::as_raw(ctx.render_target) as usize) }.unwrap_or(0);
        crate::er_depth::on_present(vt, ctx.width, ctx.height);
    }
    // M7: the nuke post pass last (over ER's frame, the viewmodel and the tracers), under its own guard: a failure
    // there turns only that pass off (`NUKE FAIL post reason=`).
    if guard("nuke.callback", || crate::nuke_present::dispatch(ctx)).is_none() {
        crate::nuke_present::fail("panic in the present callback");
    }
}

fn callback_body(ctx: &PreImguiCtx<'_>) {
    // M4: while an AC-130 ride is Active the thermal pass owns the frame and the viewmodel is never drawn. Its own
    // guard: a failure there turns the thermal pass off, never the viewmodel.
    match guard("thermal.callback", || crate::thermal_present::dispatch(ctx)) {
        Some(true) => return,
        Some(false) => {}
        None => crate::thermal_present::fail("panic in the present callback"),
    }
    // C4 (docs/C4_UI_DESIGN.md 3.3): this frame's decision (one atomic load while the injection is not installed):
    // `early` = our own list already drew the world guns and the gun / overlay under ER's UI
    let early = crate::ui_inject::present_frame();
    if VM_DISABLED.load(Ordering::Relaxed) {
        return;
    }
    let n = CALLBACKS.fetch_add(1, Ordering::Relaxed);
    let Some(cfg) = shared::try_config() else { return };
    if !cfg.viewmodel.enabled {
        if !OFF_LOGGED.swap(true, Ordering::SeqCst) {
            log_i!("VM", "off (config)");
        }
        return;
    }
    let colorspace = LAST_COLOR_SPACE.load(Ordering::Relaxed);
    if n == 0 {
        log_i!(
            "VM",
            "target fmt={} {}x{} colorspace={} frames={}",
            format_name(ctx.rtv_format),
            ctx.width,
            ctx.height,
            if colorspace < 0 { "none".to_string() } else { colorspace.to_string() },
            ctx.num_frames
        );
    }
    if !LOADER_STARTED.swap(true, Ordering::SeqCst) {
        start_loader(ctx);
        return;
    }
    if !VM_READY.load(Ordering::Acquire) {
        return;
    }
    if ctx.device.as_raw() as usize != DEVICE.load(Ordering::Relaxed) {
        fail("d3d: the swap chain's device changed (adapter switch / device reset); resources belong to the old device");
        return;
    }
    // hudhook fixes its RTV format when its render engine is created and does not refresh it on
    // ResizeBuffers; if ER recreated the swap chain buffers with another format (HDR toggle) the RTV no longer
    // matches the back buffer and the output transform would be chosen from the wrong format. Stop drawing.
    // SAFETY: plain query on the live back buffer hudhook handed us.
    let live = unsafe { ctx.render_target.GetDesc() }.Format;
    if base_format(live) != base_format(ctx.rtv_format) {
        fail(&format!(
            "back-buffer format changed ({} != hudhook RTV {}); restart the game to draw the viewmodel again",
            format_name(live),
            format_name(ctx.rtv_format)
        ));
        return;
    }
    let Some(mut ps) = try_lock(&PRESENT) else { return };
    // Output transform: re-selected when the format or colour space changes.
    let out = match ps.out {
        Some((f, cs, m)) if f == ctx.rtv_format && cs == colorspace => m,
        _ => match select_output(cfg.viewmodel.output, ctx.rtv_format, colorspace) {
            Ok(m) => {
                log_i!("VM", "output {m:?} for fmt={} colorspace={colorspace} (viewmodel.output={:?})", format_name(ctx.rtv_format), cfg.viewmodel.output);
                ps.out = Some((ctx.rtv_format, colorspace, m));
                m
            }
            Err(e) => {
                fail(&format!("output {e}"));
                return;
            }
        },
    };
    let panics = PRE_IMGUI_PANICS.load(Ordering::Relaxed);
    if panics != ps.panics_seen {
        ps.panics_seen = panics;
        fail("hudhook caught a panic around the pre-ImGui callback");
        return;
    }
    let Some(snap) = try_lock(&SNAPSHOT).map(|s| s.clone()) else { return };
    let now = now_s();
    // C3 PASS C (docs/C3_DESIGN.md 3.6): the world guns (peers, ourselves in third person) before the viewmodel, with
    // our own depth; a failure turns only this pass off. Review C3: skipped while ER shows a message (UI yield);
    // C5 `mode = "cutout"`: drawn, with the message regions cut out like the viewmodel's.
    let cut = cutout_of(&cfg, &snap);
    let world_yield = mw2er_core::ui_yield::world_hidden(cfg.ui_yield.active() && !cfg.ui_yield.cutout(), snap.ui_yield);
    // C6 depth 5: what this frame wants of ER's depth (the copy gate's `want` on ER's render thread; atomics only)
    crate::er_depth_state::note_want(snap.fp_on, !world_yield && cfg.coop.world_guns && !snap.world_guns.is_empty() && snap.in_world && !snap.menu_open && !snap.ac130.active);
    if !early
        && cfg.coop.world_guns
        && !world_yield
        && !snap.world_guns.is_empty()
        && snap.in_world
        && !snap.menu_open
        && !snap.ac130.active
        && now - snap.t < 0.5
        && !WORLD_DISABLED.load(Ordering::Relaxed)
    {
        draw_world_guns(ctx, &snap, &cfg, out, cut);
    }
    if early {
        return;
    }
    let g = DrawGate {
        renderer_ready: true,
        vm_disabled: false,
        cfg_enabled: true,
        snapshot_age_s: now - snap.t,
        in_world: snap.in_world,
        menu_open: snap.menu_open,
        mode_on: snap.mode_on,
        visible: snap.vm.visible || snap.scope > 0.0,
        ac130_active: snap.ac130.active,
        fp_on: snap.fp_on,
        draw_in_tp: snap.vm_draw_tp,
    };
    if !vm_should_draw(&g) {
        return;
    }
    // C4.1 (docs/C4_1_DESIGN.md 3.7 / 5): a Present-drawn (fallback) frame of the live UI-thread injection while a
    // message region is cut out (the C5 cut-out still applies here; early frames draw with no cut)
    if cut.n > 0 {
        crate::ui_inject::note_fallback_cut();
    }
    // C3 (docs/C3_DESIGN.md 2.3): `[ui_yield] mode = "hide"`: no gun while ER's message shows (render-only; the
    // task never blocks a shot for it, and firing ends the yield)
    let yield_frac = if cfg.ui_yield.enabled { snap.ui_yield } else { 0.0 };
    // one line per change of the yield's signal set as the present thread sees it (proves the render path)
    if YIELD_WHY_SEEN.swap(snap.ui_yield_why, Ordering::Relaxed) != snap.ui_yield_why && snap.ui_yield_why != 0 {
        log_i!("VM", "yield draw frac={yield_frac:.2} why={} mode={}", mw2er_core::ui_yield::why_label(snap.ui_yield_why), cfg.ui_yield.mode);
    }
    if cfg.ui_yield.mode == "hide" && mw2er_core::ui_yield::hide(yield_frac) {
        return;
    }
    // M6: scoped (a sniper at full ADS): the overlay replaces the viewmodel (MW2's adsFireAnim is the idle: hidden)
    if snap.scope > 0.0 {
        let Some(mut rg) = try_lock(&RENDERER) else { return };
        let Some(r) = rg.as_mut() else { return };
        if r.has_overlay() {
            let target = Target { resource: ctx.render_target, rtv: ctx.rtv, format: ctx.rtv_format, width: ctx.width, height: ctx.height };
            if let Err(e) = r.record_overlay(ctx.command_list, &target, snap.scope) {
                drop(rg);
                fail(&format!("record overlay {e}"));
            }
            return;
        }
    }
    let gun = snap.vm.gun as usize;
    let Some(assets) = assets_for(gun) else { return };
    let assets = &*assets;
    let Some(mut rg) = try_lock(&RENDERER) else { return };
    let Some(r) = rg.as_mut() else { return };
    // C5: every frame (Cutout::NONE without a message: the shader's path of before)
    r.set_cutout(cut);
    let v = &cfg.viewmodel;
    let ads = snap.vm.ads_at(now);
    let res = SCRATCH.with(|sc| {
        let mut sc = sc.borrow_mut();
        evaluate(assets, &snap.vm, now, &mut sc);
        // M5: the stance pose and weapon bob of the movement controller ride on top of [viewmodel] offset_in
        // C3: the UI yield slides the gun down (and back) while ER shows a message
        let drop = if cfg.ui_yield.mode == "lower" { mw2er_core::ui_yield::vm_offset(yield_frac, cfg.ui_yield.drop_in) } else { [0.0; 3] };
        let off = [0, 1, 2].map(|k| v.offset_in[k] + snap.vm_offset_in[k] + drop[k]);
        let pp = ProjParams { hip_fov: v.hip_fov, ads_fov: v.ads_fov, near_in: v.near_in, far_in: v.far_in, offset_in: off };
        let vp = build(&sc.world[assets.skel.tags.view as usize], ctx.width, ctx.height, ads, &pp);
        let fp = FrameParams {
            palette: &sc.palette,
            view_proj: vp.view_proj,
            view: vp.view,
            light: LightParams {
                dir_view: v.light_dir_view,
                diffuse: v.diffuse,
                ambient_sky: v.ambient_sky,
                ambient_ground: v.ambient_ground,
                exposure: v.exposure,
            },
            out,
            paper_white_nits: v.paper_white_nits,
            debug: DebugMode::None,
            variant: Variant::default(),
            timing: v.gpu_timing,
        };
        let target = Target {
            resource: ctx.render_target,
            rtv: ctx.rtv,
            format: ctx.rtv_format,
            width: ctx.width,
            height: ctx.height,
        };
        // M6: the gun's own slot (M3: slot 0); the RPG's rocket part hidden while its magazine is empty
        let hide = if snap.vm.hide_rocket { 1u32 << mw2er_core::vm_assets::PART_ROCKET } else { 0 };
        r.record_gun(ctx.command_list, &target, ctx.frame_index % ctx.num_frames.max(1), if GUNS.get().is_some() { gun } else { 0 }, hide, &fp)
    });
    if let Err(e) = res {
        drop(rg);
        fail(&format!("record {e}"));
        return;
    }
    for a in r.take_present_allocs() {
        log_w!("VM", "WARN created {a} on the present thread (size/format change: one-off hitch)");
    }
    if DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
        log_i!("VM", "first frame drawn {}x{} slot={} out={out:?}", ctx.width, ctx.height, ctx.frame_index);
    }
    if now - ps.last_log >= v.log_interval_s {
        ps.last_log = now;
        let mut t = r.take_timings();
        if !t.is_empty() {
            t.sort_by(|a, b| a.total_cmp(b));
            let q = |f: f64| t[((t.len() - 1) as f64 * f).round() as usize];
            log_i!("VM", "gpu_ms p50={:.4} p95={:.4} max={:.4} n={}", q(0.5), q(0.95), t[t.len() - 1], t.len());
        }
    }
}

/// C5 `[ui_yield] mode = "cutout"`: the snapshot's message regions for the renderer ([`Cutout::NONE`] in any other
/// mode, with the yield disabled, or while no region is up).
fn cutout_of(cfg: &mw2er_core::config::Config, snap: &crate::shared::HudSnapshot) -> Cutout {
    let n = (snap.ui_cut_n as usize).min(mw2er_core::ui_yield::MAX_CUT_RECTS);
    if !cfg.ui_yield.cutout() || n == 0 {
        return Cutout::NONE;
    }
    let mut c = Cutout { n, feather: cfg.ui_yield.cutout_feather, ..Cutout::NONE };
    for (k, r) in snap.ui_cut[..n].iter().enumerate() {
        c.rects[k] = r.rect;
        c.strength[k] = r.strength;
    }
    c
}

/// C3 PASS C: records the snapshot's world guns with ER's camera (the HUD's eye, rows and fov).
fn draw_world_guns(ctx: &PreImguiCtx<'_>, snap: &crate::shared::HudSnapshot, cfg: &mw2er_core::config::Config, out: OutputMode, cut: Cutout) {
    let Some(mut rg) = try_lock(&RENDERER) else { return };
    let Some(r) = rg.as_mut() else { return };
    r.set_cutout(cut);
    let (vf, _) = mw2er_core::hud_geom::vfov_rad(snap.vfov_raw, cfg.hud.fallback_vfov_deg);
    let Some((view, view_proj)) = mw2er_core::coop_vis::world_view_proj(snap.view_pos, snap.cam_rows, vf, ctx.width, ctx.height, 0.05, 3000.0) else { return };
    let draws: Vec<WorldDraw> = snap.world_guns.iter().map(|w| WorldDraw { gun: w.slot as usize, lod: w.lod, model: w.model }).collect();
    let v = &cfg.viewmodel;
    let wf = WorldFrame {
        view,
        view_proj,
        light: LightParams { dir_view: v.light_dir_view, diffuse: v.diffuse, ambient_sky: v.ambient_sky, ambient_ground: v.ambient_ground, exposure: v.exposure },
        out,
        paper_white_nits: v.paper_white_nits,
        variant: Variant::default(),
    };
    let target = Target { resource: ctx.render_target, rtv: ctx.rtv, format: ctx.rtv_format, width: ctx.width, height: ctx.height };
    // C6 depth fallback: the body capsules into OUR depth first, third person only (first person draws exactly as before)
    let caps: &[mw2er_core::body_occl::Capsule] = if cfg.coop.world_gun_body_occlusion && !snap.fp_on { &snap.world_occl } else { &[] };
    // C6 depth 5: ER's scene depth (this Present's copy) primed into OUR depth before the capsules, third person only and
    // only while live; None = the commands of before (first person, flags off, a miss)
    let ticket = crate::er_depth_state::present_prime(snap, cfg);
    let reader = if ticket.is_some() { crate::er_depth_state::reader() } else { None };
    let prime = match (&ticket, reader.as_ref().and_then(|g| g.as_ref())) {
        (Some(t), Some(rd)) => crate::er_depth_state::prime_in(t, rd),
        _ => None,
    };
    match r.record_world_occluded(ctx.command_list, &target, ctx.frame_index % ctx.num_frames.max(1), &draws, &wf, caps, prime.as_ref()) {
        Ok((n, nc)) if n > 0 => {
            if let Some(t) = &ticket {
                crate::er_depth_state::note_primed(r.last_primed(), t);
            }
            if WORLD_DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
                log_i!("COOP", "worldgun first frame drawn n={n} {}x{}", ctx.width, ctx.height);
            }
            if nc > 0 && WORLD_OCCL_DRAWN.fetch_add(1, Ordering::Relaxed) == 0 {
                log_i!("COOP", "worldgun body occlusion first frame capsules={nc} guns={n} (our depth only; ER's depth never bound)");
            }
        }
        Ok(_) => {}
        Err(e) => {
            if !WORLD_DISABLED.swap(true, Ordering::SeqCst) {
                log_e!("COOP", "FAIL worldgun record {e}; world guns off for this session (the viewmodel keeps drawing)");
            }
        }
    }
    for a in r.take_present_allocs() {
        log_w!("VM", "WARN created {a} on the present thread (size/format change: one-off hitch)");
    }
}

fn start_loader(ctx: &PreImguiCtx<'_>) {
    let device: ID3D12Device = ctx.device.clone();
    DEVICE.store(ctx.device.as_raw() as usize, Ordering::Relaxed);
    let fmt = ctx.rtv_format;
    let (width, height) = (ctx.width, ctx.height);
    let num_frames = ctx.num_frames.max(1);
    // SAFETY: plain query on the game's present queue.
    let ts_freq = unsafe { ctx.command_queue.GetTimestampFrequency() }.unwrap_or(0);
    let Some(dir) = ASSETS_DIR.get().cloned() else {
        fail("assets: no assets_dir (install was not called)");
        return;
    };
    let spawned = std::thread::Builder::new().name("mw2er-vmload".into()).spawn(move || {
        set_thread_role("vmload");
        let r = guard("vm.loader", || load(device, fmt, (width, height), num_frames, ts_freq, &dir));
        match r {
            Some(Ok(())) => {}
            Some(Err(e)) => fail(&e),
            None => fail("panic in the loader thread"),
        }
    });
    if let Err(e) = spawned {
        fail(&format!("loader thread: {e}"));
    }
}

fn load(device: ID3D12Device, fmt: DXGI_FORMAT, size: (u32, u32), num_frames: usize, ts_freq: u64, dir: &std::path::Path) -> Result<(), String> {
    let t0 = Instant::now();
    let mut r = Renderer::new(&device, num_frames, ts_freq).map_err(|e| e.to_string())?;
    r.prepare(fmt, Variant::default(), size.0, size.1).map_err(|e| e.to_string())?;
    // C6 depth fallback: the body-capsule pass (our root signature / PSO / ring; created here, never on the present
    // thread). A failure only means no capsules: the world guns draw as before.
    match r.prepare_occluders() {
        Ok(()) => log_i!("COOP", "worldgun occluders ready (depth-only body capsules; used with [coop] world_gun_body_occlusion in third person)"),
        Err(e) => log_w!("COOP", "WARN worldgun occluders unavailable: {e} (world guns draw without body occlusion)"),
    }
    // C6 depth 5: the ER-depth prime pass (our root signature / PSO / SRV heap), only with world_gun_er_depth at boot
    if crate::er_depth_state::prime_wanted() {
        match r.prepare_er_depth() {
            Ok(()) => log_i!("COOP", "worldgun er depth prime ready (depth-only; ER's depth copy primes our world-gun depth in third person once live)"),
            Err(e) => log_w!("COOP", "WARN worldgun er depth prime unavailable: {e} (world guns keep the capsules)"),
        }
    }
    // M6: every gun of the arsenal into its own slot (docs/M6_DESIGN.md 4.3); a gun that fails is marked and skipped
    if let Some(ar) = crate::arsenal_rt::ARSENAL.get() {
        let mut cache = mw2er_core::vm_assets::TextureCache::new();
        let mut guns: Vec<Option<Arc<VmAssets>>> = Vec::with_capacity(ar.guns.len());
        let (mut ok, mut bytes, mut overlay_file) = (0usize, 0u64, None);
        let mut fatal: Option<String> = None;
        for (slot, g) in ar.guns.iter().enumerate() {
            // review M6: after a device-level failure (removed / hung / a 120 s stuck upload) every later gun would
            // fail the same way or block the loader again: they are marked failed without trying
            if let Some(f) = &fatal {
                crate::arsenal_rt::VM_GUN_FAILED.fetch_or(1u64 << slot.min(63), Ordering::SeqCst);
                log_e!("ARSENAL", "disable gun={} why=assets (not tried after the device failure: {f})", g.name);
                guns.push(None);
                continue;
            }
            let one = VmAssets::load_gun_dir(&ar.dir.join(&g.name), &mut cache).and_then(|a| {
                r.upload_gun(slot, &a).map(|b| (a, b)).map_err(|e| {
                    if e.is_device_fatal() {
                        fatal = Some(e.to_string());
                    }
                    e.to_string()
                })
            });
            match one {
                Ok((a, b)) => {
                    ok += 1;
                    bytes += b;
                    if overlay_file.is_none() {
                        overlay_file = g.spec.ads.overlay.as_ref().map(|o| o.file.clone());
                    }
                    crate::arsenal_rt::VM_GUN_READY.fetch_or(1u64 << slot.min(63), Ordering::SeqCst);
                    guns.push(Some(Arc::new(a)));
                }
                Err(e) => {
                    crate::arsenal_rt::VM_GUN_FAILED.fetch_or(1u64 << slot.min(63), Ordering::SeqCst);
                    log_e!("ARSENAL", "disable gun={} why=assets ({e})", g.name);
                    guns.push(None);
                }
            }
        }
        // C3 PASS C (docs/C3_DESIGN.md 3.6): the world models of every gun (not fatal: a gun without one shows no world
        // gun); never after a device-level failure
        if fatal.is_none() {
            let t1 = Instant::now();
            let (mut wg, mut wl, mut wbytes) = (0usize, 0usize, 0u64);
            for (slot, g) in ar.guns.iter().enumerate() {
                match VmAssets::load_gun_world(&ar.dir.join(&g.name), &mut cache) {
                    Ok(list) if list.is_empty() => {}
                    Ok(list) => {
                        let mut any = false;
                        for w in &list {
                            match r.upload_world(slot, w.lod, &w.mesh, &w.textures) {
                                Ok(b) => {
                                    any = true;
                                    wl += 1;
                                    wbytes += b;
                                }
                                Err(e) => {
                                    log_w!("COOP", "WARN worldgun gun={} lod={} upload: {e}", g.name, w.lod);
                                    if e.is_device_fatal() {
                                        fatal = Some(e.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                        wg += any as usize;
                    }
                    Err(e) => log_w!("COOP", "WARN worldgun gun={} assets: {e}; no world gun for it", g.name),
                }
                if fatal.is_some() {
                    break;
                }
            }
            log_i!("COOP", "worldguns loaded guns={wg}/{} lods={wl} bytes={wbytes} ms={}", ar.guns.len(), t1.elapsed().as_millis());
        }
        if let Some(f) = overlay_file {
            let p = ar.dir.join(f.replace('/', "\\"));
            match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|d| mw2er_core::vm_assets::parse_tex(&d)).and_then(|t| r.upload_overlay(&t).map_err(|e| e.to_string())) {
                Ok(srv) => {
                    OVERLAY_READY.store(true, Ordering::SeqCst);
                    log_i!("ARSENAL", "scope overlay ready srv={srv} file={}", p.display())
                }
                Err(e) => log_w!("ARSENAL", "WARN scope overlay {}: {e}; scoped guns show the plain ADS view", p.display()),
            }
        }
        if let Some(f) = fatal {
            return Err(format!("assets: device failure while uploading the arsenal ({f}); {ok} gun(s) were on the GPU"));
        }
        if ok == 0 {
            return Err("assets: no arsenal gun loaded".into());
        }
        log_i!(
            "ARSENAL",
            "loaded n={} ok={ok} ms={} vram_mb={:.1} gun_bytes={bytes} textures_shared={} uploads={} fmt={}",
            ar.guns.len(),
            t0.elapsed().as_millis(),
            r.gpu_bytes() as f64 / (1024.0 * 1024.0),
            cache.len(),
            r.upload_count(),
            format_name(fmt)
        );
        let _ = GUNS.set(guns);
        *lock(&RENDERER) = Some(r);
        VM_READY.store(true, Ordering::Release);
        return Ok(());
    }
    let assets = VmAssets::load_dir(dir).map_err(|e| format!("assets: {e}"))?;
    let bytes = r.upload(&assets).map_err(|e| e.to_string())?;
    let (bones, verts, clips) = (assets.skel.bones.len(), assets.mesh.vertex_count, assets.clips.len());
    let _ = ASSETS.set(Arc::new(assets));
    *lock(&RENDERER) = Some(r);
    VM_READY.store(true, Ordering::Release);
    log_i!(
        "VM",
        "assets ready ms={} bytes={bytes} bones={bones} verts={verts} clips={clips} fmt={} ts_freq={ts_freq}",
        t0.elapsed().as_millis(),
        format_name(fmt)
    );
    Ok(())
}

/// `VM WARN` once when hudhook is up but the callback never ran (called from the HUD render loop).
pub fn check_no_frames(hud_init_t: f64, now: f64, warned: &mut bool) {
    if !*warned && now - hud_init_t > 30.0 && CALLBACKS.load(Ordering::Relaxed) == 0 && PRE_IMGUI.get().is_some() {
        *warned = true;
        log_w!("VM", "WARN no frames 30 s after HUD initialize (pre-ImGui callback never ran)");
    }
}

// ---- C4: the early path (docs/C4_UI_DESIGN.md 3.3, 4.3) -----------------------------------------------------------------

/// What the Present path would draw this frame, without cloning the snapshot (C4's gate: `Wants`).
pub fn early_wants() -> Wants {
    if VM_DISABLED.load(Ordering::Relaxed) || !VM_READY.load(Ordering::Acquire) {
        return Wants::NotReady;
    }
    let Some(cfg) = shared::try_config() else { return Wants::NotReady };
    if !cfg.viewmodel.enabled {
        return Wants::NotReady;
    }
    let Some(s) = try_lock(&SNAPSHOT) else { return Wants::NotReady };
    if s.ac130.active {
        return Wants::Ac130;
    }
    let g = DrawGate {
        renderer_ready: true,
        vm_disabled: false,
        cfg_enabled: true,
        snapshot_age_s: now_s() - s.t,
        in_world: s.in_world,
        menu_open: s.menu_open,
        mode_on: s.mode_on,
        visible: s.vm.visible || s.scope > 0.0,
        ac130_active: s.ac130.active,
        fp_on: s.fp_on,
        draw_in_tp: s.vm_draw_tp,
    };
    if !vm_should_draw(&g) {
        return if s.in_world && s.mode_on && !s.fp_on && !s.vm_draw_tp { Wants::NotFp } else { Wants::Nothing };
    }
    // M6: scoped with the overlay loaded = the overlay; without it Present draws the gun, and so does the early path
    if s.scope > 0.0 && OVERLAY_READY.load(Ordering::Relaxed) {
        return Wants::Overlay;
    }
    // review C4: the gun in hand still loading (or failed) is an expected miss, as Present's silent return
    if assets_for(s.vm.gun as usize).is_none() { Wants::NotReady } else { Wants::Gun }
}

/// One early record (ui_inject.rs fills it from UiBegin's facts and the passed gate).
pub struct EarlyReq {
    /// ER's UI target (`ID3D12Resource*`, in 0xC0) and its desc.
    pub res: usize,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    /// What the PSOs write (C4: `Scene`, or `Premul` on the HDR UI layer; C4.1: the layer's convention).
    pub mode: EarlyMode,
    pub out: EarlyOut,
    pub slot: usize,
    /// The fence value hudhook signals right after our list.
    pub value: u64,
}

pub enum EarlyErr {
    /// A lock was contended (expected miss).
    Locked,
    /// Assets / renderer / early pass not ready (expected miss).
    NotReady,
    /// The PSOs for this target are not built yet (expected miss).
    Pso,
    /// A D3D12 / record failure (trip).
    D3d(String),
    /// C4.1: a D3D12 / record failure as the renderer returned it (trip; formatted on the task thread, never on ER's
    /// render thread).
    Vm(VmError),
}

/// C4: records world guns + gun (or the scope overlay) into the early pass's list of `q.slot` on ER's render thread,
/// with the same decisions as the Present path (the UI yield is not applied: this frame is drawn under ER's UI).
/// try_lock only; never waits; never creates a PSO.
pub fn record_early(q: &EarlyReq) -> Result<(), EarlyErr> {
    let cfg = shared::try_config().ok_or(EarlyErr::Locked)?;
    let snap = try_lock(&SNAPSHOT).map(|s| s.clone()).ok_or(EarlyErr::Locked)?;
    let rg = try_lock(&RENDERER).ok_or(EarlyErr::Locked)?;
    let r = rg.as_ref().ok_or(EarlyErr::NotReady)?;
    let mut eg = try_lock(&EARLY).ok_or(EarlyErr::Locked)?;
    let e = eg.as_mut().ok_or(EarlyErr::NotReady)?;
    let fmt = mw2er_render::early::dxgi(q.format);
    if !e.has_psos(fmt, q.mode) {
        return Err(EarlyErr::Pso);
    }
    let raw = q.res as *mut std::ffi::c_void;
    // SAFETY: ER's live UI target (non-zero, its vtable in the D3D12 runtime module: checked by the gate); borrowed,
    // never released; ER keeps it alive across frames.
    let res = unsafe { windows::Win32::Graphics::Direct3D12::ID3D12Resource::from_raw_borrowed(&raw) }.ok_or(EarlyErr::NotReady)?;
    let t = EarlyTarget { resource: res, format: fmt, width: q.width, height: q.height, mode: q.mode };
    let v = &cfg.viewmodel;
    // the output from the target format (4.1): sRGB OETF into UNORM, linear into an _SRGB view (scRGB at 80 nits = 1.0)
    let (out, paper) = match q.out {
        EarlyOut::Srgb => (OutputMode::Srgb, v.paper_white_nits),
        EarlyOut::Linear => (OutputMode::Scrgb, 80.0),
    };
    // review C4: the gun's bundle loaded and uploaded, checked BEFORE begin (an expected miss: this frame draws at
    // Present, which returns silently for a gun still loading); only real D3D12 failures trip
    let gun = snap.vm.gun as usize;
    let gun_slot = if GUNS.get().is_some() { gun } else { 0 };
    if !(snap.scope > 0.0 && r.has_overlay()) && (assets_for(gun).is_none() || !r.gun_ready(gun_slot)) {
        return Err(EarlyErr::NotReady);
    }
    e.begin(q.slot, q.value, &t).map_err(|x| EarlyErr::D3d(x.to_string()))?;
    let now = now_s();
    let e: &EarlyPass = e;
    let rec = || -> Result<(), String> {
        // C3 PASS C: the world guns first, same conditions as at Present (minus the UI yield)
        if cfg.coop.world_guns && !snap.world_guns.is_empty() && snap.in_world && !snap.menu_open && !snap.ac130.active && now - snap.t < 0.5 && !WORLD_DISABLED.load(Ordering::Relaxed) {
            let (vf, _) = mw2er_core::hud_geom::vfov_rad(snap.vfov_raw, cfg.hud.fallback_vfov_deg);
            if let Some((view, view_proj)) = mw2er_core::coop_vis::world_view_proj(snap.view_pos, snap.cam_rows, vf, t.width, t.height, 0.05, 3000.0) {
                let draws: Vec<WorldDraw> = snap.world_guns.iter().map(|w| WorldDraw { gun: w.slot as usize, lod: w.lod, model: w.model }).collect();
                let wf = WorldFrame {
                    view,
                    view_proj,
                    light: LightParams { dir_view: v.light_dir_view, diffuse: v.diffuse, ambient_sky: v.ambient_sky, ambient_ground: v.ambient_ground, exposure: v.exposure },
                    out,
                    paper_white_nits: paper,
                    variant: Variant::default(),
                };
                // review C4: as at Present, a world-gun failure turns only the world guns off (the task logs it)
                if let Err(x) = r.record_world_early(e, q.slot, &t, &draws, &wf)
                    && !WORLD_DISABLED.swap(true, Ordering::SeqCst)
                {
                    if let Some(mut g) = try_lock(&WORLD_EARLY_ERR) {
                        *g = x.to_string();
                    }
                    WORLD_EARLY_FAIL.store(true, Ordering::SeqCst);
                }
            }
        }
        // M6: scoped: the overlay replaces the gun
        if snap.scope > 0.0 && r.has_overlay() {
            return r.record_overlay_early(e, q.slot, &t, snap.scope).map_err(|x| format!("overlay {x}"));
        }
        let assets = assets_for(gun).ok_or("assets of the gun in hand not loaded")?;
        let assets = &*assets;
        let ads = snap.vm.ads_at(now);
        SCRATCH.with(|sc| {
            let mut sc = sc.borrow_mut();
            evaluate(assets, &snap.vm, now, &mut sc);
            let off = [0, 1, 2].map(|k| v.offset_in[k] + snap.vm_offset_in[k]);
            let pp = ProjParams { hip_fov: v.hip_fov, ads_fov: v.ads_fov, near_in: v.near_in, far_in: v.far_in, offset_in: off };
            // 4.2: projection and ADS from the target's size (the gun looks as on the back buffer)
            let vp = build(&sc.world[assets.skel.tags.view as usize], t.width, t.height, ads, &pp);
            let fp = FrameParams {
                palette: &sc.palette,
                view_proj: vp.view_proj,
                view: vp.view,
                light: LightParams { dir_view: v.light_dir_view, diffuse: v.diffuse, ambient_sky: v.ambient_sky, ambient_ground: v.ambient_ground, exposure: v.exposure },
                out,
                paper_white_nits: paper,
                debug: DebugMode::None,
                variant: Variant::default(),
                timing: false,
            };
            let hide = if snap.vm.hide_rocket { 1u32 << mw2er_core::vm_assets::PART_ROCKET } else { 0 };
            r.record_gun_early(e, q.slot, &t, gun_slot, hide, &fp).map_err(|x| format!("gun {x}"))
        })
    };
    let rec_r = rec();
    let e = eg.as_mut().ok_or(EarlyErr::NotReady)?;
    match rec_r {
        Ok(()) => e.finish(q.slot, &t).map_err(|x| EarlyErr::D3d(x.to_string())),
        Err(x) => {
            e.abandon(q.slot);
            Err(EarlyErr::D3d(x))
        }
    }
}

/// C4: creates the early pass on the renderer's device with PSOs for the back-buffer format (worker thread, once the
/// renderer is ready) and hands its raw pointers to ui_inject. Returns the log detail.
///
/// C4.1 (`ui_thread`, docs/C4_1_DESIGN.md 3.5): no PSO set up front (the gate asks for (layer format, convention) and
/// the PSO worker builds it); the depth for the back-buffer size (= the layer's [V probe]); the render thread's
/// snapshot copy and pose scratch reserved here; and the UiEnd readback sampler (the canary / the convention
/// re-check) created here. Nothing of it is ever created on ER's render thread.
pub fn create_early(ui_layer: bool, ui_thread: bool) -> Result<String, String> {
    let dev = lock(&RENDERER).as_ref().map(|r| r.device().clone()).ok_or("no renderer")?;
    let fmt = lock(&PRESENT).out.map(|o| o.0).unwrap_or(windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R10G10B10A2_UNORM);
    let bb = crate::ui_probe::BB_SIZE.load(Ordering::Relaxed);
    let (w, h) = ((bb & 0xff_ffff) as u32, ((bb >> 24) & 0xff_ffff) as u32);
    let mut e = EarlyPass::new(&dev).map_err(|x| x.to_string())?;
    let b = e.pso_builder();
    let mut sets = Vec::new();
    if !ui_thread {
        sets.push((fmt, EarlyMode::Scene));
        if ui_layer {
            sets.push((fmt, EarlyMode::Premul));
        }
    }
    for (f, m) in &sets {
        e.insert_psos(b.build(*f, *m).map_err(|x| format!("PSOs {}: {x}", format_name(*f)))?);
    }
    if w > 0 && h > 0 {
        e.prepare_depth(w, h).map_err(|x| x.to_string())?;
    }
    let mut sampler = None;
    if ui_thread {
        lock(&EARLY_SNAP).world_guns.reserve(MAX_WORLD_DRAWS);
        lock(&EARLY_SCRATCH).reserve(MAX_PALETTE);
        sampler = Some(mw2er_render::LayerSampler::new(&dev).map_err(|x| format!("readback sampler: {x}"))?);
    }
    let lists = [e.list_raw(0), e.list_raw(1), e.list_raw(2)];
    let (fence, device, own_vt) = (e.fence_raw(), e.device_raw(), e.own_resource_vtable());
    *lock(&EARLY) = Some(e);
    for (f, m) in &sets {
        crate::ui_inject::mark_pso(f.0 as u32, *m);
    }
    if w > 0 && h > 0 {
        crate::ui_inject::publish_depth(w, h);
    }
    if let Some(s) = sampler {
        crate::ui_inject::publish_sampler(s);
    }
    crate::ui_inject::publish_early(lists, fence, device, own_vt);
    Ok(format!(
        "device=0x{device:x} fmt={} depth={w}x{h} slots={} psos={}{}",
        format_name(fmt),
        mw2er_core::c4::SLOTS,
        sets.len(),
        if ui_thread { " mode=ui_thread (PSOs per layer convention on request; readback sampler ready)" } else { "" }
    ))
}

/// C4.1: our depth for the layer's size (worker thread; the gate met a size it had no depth for: `size` until then).
pub fn prepare_early_depth(w: u32, h: u32) -> Result<(), String> {
    let mut g = lock(&EARLY);
    let e = g.as_mut().ok_or("no early pass")?;
    e.prepare_depth(w, h).map_err(|x| x.to_string())?;
    drop(g);
    crate::ui_inject::publish_depth(w, h);
    Ok(())
}

/// C4.1 (docs/C4_1_DESIGN.md 3.5): [`record_early`] on ER's render thread WITHOUT allocation: the snapshot is copied
/// into the reserved [`EARLY_SNAP`], the world guns into a stack array, the pose into the reserved [`EARLY_SCRATCH`];
/// the begin never creates the depth or a PSO (`begin_prepared`); errors stay `VmError`s (formatted by the task).
/// try_lock only; never waits. The UI yield is not applied (the frame is drawn under ER's UI) and the C5 cut-out is
/// not either (`cut_count = 0`: the frame constants of the early path never carry it).
pub fn record_early41(q: &EarlyReq) -> Result<(), EarlyErr> {
    let cfg = shared::try_config().ok_or(EarlyErr::Locked)?;
    let mut es = try_lock(&EARLY_SNAP).ok_or(EarlyErr::Locked)?;
    {
        let s = try_lock(&SNAPSHOT).ok_or(EarlyErr::Locked)?;
        es.copy_from(&s);
    }
    let snap: &EarlySnap = &es;
    let rg = try_lock(&RENDERER).ok_or(EarlyErr::Locked)?;
    let r = rg.as_ref().ok_or(EarlyErr::NotReady)?;
    let mut eg = try_lock(&EARLY).ok_or(EarlyErr::Locked)?;
    let e = eg.as_mut().ok_or(EarlyErr::NotReady)?;
    let fmt = mw2er_render::early::dxgi(q.format);
    if !e.has_psos(fmt, q.mode) {
        return Err(EarlyErr::Pso);
    }
    if !e.has_depth(q.width, q.height) {
        return Err(EarlyErr::NotReady);
    }
    let mut sc = try_lock(&EARLY_SCRATCH).ok_or(EarlyErr::Locked)?;
    let raw = q.res as *mut std::ffi::c_void;
    // SAFETY: ER's live UI layer (the 7 pins, its vtable in the D3D12 runtime module, a fresh GetDesc this UiBegin);
    // borrowed, never released; ER's own armed list of this frame references it.
    let res = unsafe { windows::Win32::Graphics::Direct3D12::ID3D12Resource::from_raw_borrowed(&raw) }.ok_or(EarlyErr::NotReady)?;
    let t = EarlyTarget { resource: res, format: fmt, width: q.width, height: q.height, mode: q.mode };
    let v = &cfg.viewmodel;
    let (out, paper) = match q.out {
        EarlyOut::Srgb => (OutputMode::Srgb, v.paper_white_nits),
        EarlyOut::Linear => (OutputMode::Scrgb, 80.0),
    };
    let gun = snap.vm.gun as usize;
    let gun_slot = if GUNS.get().is_some() { gun } else { 0 };
    let overlay = snap.scope > 0.0 && r.has_overlay();
    let assets = if overlay { None } else { Some(assets_for(gun).ok_or(EarlyErr::NotReady)?) };
    if !overlay && (!r.gun_ready(gun_slot) || assets.as_ref().is_some_and(|a| a.skel.bones.len() > sc.capacity())) {
        return Err(EarlyErr::NotReady);
    }
    e.begin_prepared(q.slot, q.value, &t).map_err(EarlyErr::Vm)?;
    let now = now_s();
    let e: &EarlyPass = e;
    let mut rec = || -> Result<(), VmError> {
        // C3 PASS C: the world guns first, the same conditions as at Present (minus the UI yield)
        if cfg.coop.world_guns && !snap.world_guns.is_empty() && snap.in_world && !snap.menu_open && !snap.ac130 && now - snap.t < 0.5 && !WORLD_DISABLED.load(Ordering::Relaxed) {
            let (vf, _) = mw2er_core::hud_geom::vfov_rad(snap.vfov_raw, cfg.hud.fallback_vfov_deg);
            if let Some((view, view_proj)) = mw2er_core::coop_vis::world_view_proj(snap.view_pos, snap.cam_rows, vf, t.width, t.height, 0.05, 3000.0) {
                let mut draws = [WorldDraw { gun: 0, lod: 0, model: mw2er_core::xform::MAT34_IDENTITY }; MAX_WORLD_DRAWS];
                let n = snap.world_guns.len().min(MAX_WORLD_DRAWS);
                for (d, w) in draws.iter_mut().zip(&snap.world_guns[..n]) {
                    *d = WorldDraw { gun: w.slot as usize, lod: w.lod, model: w.model };
                }
                let wf = WorldFrame {
                    view,
                    view_proj,
                    light: LightParams { dir_view: v.light_dir_view, diffuse: v.diffuse, ambient_sky: v.ambient_sky, ambient_ground: v.ambient_ground, exposure: v.exposure },
                    out,
                    paper_white_nits: paper,
                    variant: Variant::default(),
                };
                // as at Present, a world-gun failure turns only the world guns off (the task logs it)
                if let Err(x) = r.record_world_early(e, q.slot, &t, &draws[..n], &wf)
                    && !WORLD_DISABLED.swap(true, Ordering::SeqCst)
                {
                    if let Some(mut g) = try_lock(&WORLD_EARLY_VMERR) {
                        *g = Some(x);
                    }
                    WORLD_EARLY_FAIL.store(true, Ordering::SeqCst);
                }
            }
        }
        // M6: scoped: the overlay replaces the gun
        if overlay {
            return r.record_overlay_early(e, q.slot, &t, snap.scope);
        }
        let Some(assets) = assets.as_deref() else { return Ok(()) };
        let ads = snap.vm.ads_at(now);
        evaluate(assets, &snap.vm, now, &mut sc);
        let off = [0, 1, 2].map(|k| v.offset_in[k] + snap.vm_offset_in[k]);
        let pp = ProjParams { hip_fov: v.hip_fov, ads_fov: v.ads_fov, near_in: v.near_in, far_in: v.far_in, offset_in: off };
        // the projection and ADS from the layer's size (= the back buffer's: the gun looks as at Present)
        let vp = build(&sc.world[assets.skel.tags.view as usize], t.width, t.height, ads, &pp);
        let fp = FrameParams {
            palette: &sc.palette,
            view_proj: vp.view_proj,
            view: vp.view,
            light: LightParams { dir_view: v.light_dir_view, diffuse: v.diffuse, ambient_sky: v.ambient_sky, ambient_ground: v.ambient_ground, exposure: v.exposure },
            out,
            paper_white_nits: paper,
            debug: DebugMode::None,
            variant: Variant::default(),
            timing: false,
        };
        let hide = if snap.vm.hide_rocket { 1u32 << mw2er_core::vm_assets::PART_ROCKET } else { 0 };
        r.record_gun_early(e, q.slot, &t, gun_slot, hide, &fp)
    };
    let rec_r = rec();
    let e = eg.as_mut().ok_or(EarlyErr::NotReady)?;
    match rec_r {
        Ok(()) => e.finish(q.slot, &t).map_err(EarlyErr::Vm),
        Err(x) => {
            e.abandon(q.slot);
            Err(EarlyErr::Vm(x))
        }
    }
}

/// C4.1 (docs/C4_1_DESIGN.md 3.6): the session canary's list: `0xC0 -> RT`, one texel at `c4::CANARY_XY` of `color`
/// (opaque in the convention), `RT -> 0xC0`. `None` = the persistence check's list (review C4.1): the two barriers
/// only, nothing drawn. No gun is drawn (the frame is a `canary` miss, drawn at Present). No allocation; try_lock only.
pub fn record_canary41(q: &EarlyReq, color: Option<[f32; 4]>) -> Result<(), EarlyErr> {
    let mut eg = try_lock(&EARLY).ok_or(EarlyErr::Locked)?;
    let e = eg.as_mut().ok_or(EarlyErr::NotReady)?;
    let fmt = mw2er_render::early::dxgi(q.format);
    if !e.has_psos(fmt, q.mode) {
        return Err(EarlyErr::Pso);
    }
    if !e.has_depth(q.width, q.height) {
        return Err(EarlyErr::NotReady);
    }
    let raw = q.res as *mut std::ffi::c_void;
    // SAFETY: as in record_early41.
    let res = unsafe { windows::Win32::Graphics::Direct3D12::ID3D12Resource::from_raw_borrowed(&raw) }.ok_or(EarlyErr::NotReady)?;
    let t = EarlyTarget { resource: res, format: fmt, width: q.width, height: q.height, mode: q.mode };
    e.begin_prepared(q.slot, q.value, &t).map_err(EarlyErr::Vm)?;
    let rec = match color {
        Some(c) => e.record_canary(q.slot, &t, mw2er_core::c4::CANARY_XY, c),
        None => Ok(()),
    };
    match rec {
        Ok(()) => e.finish(q.slot, &t).map_err(EarlyErr::Vm),
        Err(x) => {
            e.abandon(q.slot);
            Err(EarlyErr::Vm(x))
        }
    }
}

/// C4.1 probe P2: the renderer's device (ER's device), once the renderer exists (worker thread: the layer sampler).
pub fn renderer_device() -> Option<ID3D12Device> {
    lock(&RENDERER).as_ref().map(|r| r.device().clone())
}

/// C4: the PSOs for a target format the gate met (worker thread; the pass is locked only to clone / insert).
pub fn build_early_psos(format: u32, mode: EarlyMode) -> Result<(), String> {
    let b = lock(&EARLY).as_ref().map(|e| e.pso_builder()).ok_or("no early pass")?;
    let set = b.build(mw2er_render::early::dxgi(format), mode).map_err(|x| x.to_string())?;
    if let Some(e) = lock(&EARLY).as_mut() {
        e.insert_psos(set);
    }
    crate::ui_inject::mark_pso(format, mode);
    Ok(())
}

/// C4: releases retired early depth buffers whose fence passed (task thread, every 5 s; try_lock only).
pub fn early_housekeeping() {
    if WORLD_EARLY_FAIL.swap(false, Ordering::SeqCst) {
        let vm = try_lock(&WORLD_EARLY_VMERR).and_then(|mut g| g.take()).map(|e| e.to_string());
        let err = vm.unwrap_or_else(|| try_lock(&WORLD_EARLY_ERR).map(|g| g.clone()).unwrap_or_default());
        log_e!("COOP", "FAIL worldgun record (early path) {err}; world guns off for this session (the viewmodel keeps drawing)");
    }
    if let Some(mut g) = try_lock(&EARLY)
        && let Some(e) = g.as_mut()
    {
        e.release_retired();
    }
}

/// C4 unload: drops the early pass when `safe` (both teardown waits passed), else leaks it (never freed while the GPU
/// may still use it). Returns whether it was released. try_lock only (the unload path runs under the loader lock).
pub fn release_early(safe: bool) -> bool {
    let Some(mut g) = try_lock(&EARLY) else { return false };
    match g.take() {
        Some(e) if safe => {
            drop(e);
            true
        }
        Some(e) => {
            std::mem::forget(e);
            false
        }
        None => false,
    }
}