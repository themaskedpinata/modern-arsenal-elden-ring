//! HUD via hudhook 0.9.3 DX12 + ImGui (docs/M2_DESIGN.md section 13).
//!
//! The render loop runs on the game's present thread. It reads only the task's snapshot, the
//! toast and the config (all with `try_lock`, keeping the last copy on contention, so Present never
//! blocks), never game memory and never `FromStatic`. Geometry comes from mw2er-core::hud_geom.
//! ImGui never takes mouse/keyboard input: it is a draw-only overlay.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{ConfigFlags, Context, DrawListMut, FontConfig, FontSource, Io, Ui};
use hudhook::{BeforeWndProc, Hudhook, ImguiRenderLoop, MessageFilter, RenderContext};
use mw2er_core::ac130_hud::{blink_alpha, box43, dark_pulse, menu_rect, project, timer_text};
use mw2er_core::ac130_tracer::{MAX_INSTANCES, TracerInstance, TracerView, build_instances};
use mw2er_core::config::Config;
use mw2er_core::font_check::{MAX_FONT_BYTES, check_font};
use mw2er_core::hud_geom::{ammo_anchor, crosshair, hitmarker, mag_is_low, reload_bar, scales, stance_glyph, vfov_rad};

use crate::data::AC_DATA;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};

use crate::guard::guard;
use crate::log::{log_e, log_i, log_w, set_thread_role};
use crate::shared::{self, HUD_DISABLED, HUD_OK, HudSnapshot, SNAPSHOT, TOAST, Toast, now_s, try_lock};

/// Snapshots older than this hide the crosshair (the task stopped ticking: loading, pause...).
const SNAPSHOT_MAX_AGE_S: f64 = 0.5;

const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 0.65];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const RED: [f32; 4] = [0.95, 0.2, 0.15, 0.95];
const BADGE: [f32; 4] = [0.45, 0.62, 0.28, 0.9];

/// Applies the D3D12 hooks (boot thread, before the game creates its swapchain, so hudhook's
/// CreateSwapChain hook captures the command queue). The M3 viewmodel's pre-ImGui callback is registered
/// first (PRE_IMGUI.set happens before Hudhook::builder()), so the hook exists only if the version gate passed.
pub fn apply(hmodule_raw: usize, root: std::path::PathBuf) -> Result<(), String> {
    let _ = guard("vm.install", || crate::viewmodel::install(root));
    let r = guard("hud.apply", || {
        Hudhook::builder()
            .with::<ImguiDx12Hooks>(Mw2Hud::new())
            .with_hmodule(HINSTANCE(hmodule_raw as *mut std::ffi::c_void))
            .build()
            .apply()
    });
    match r {
        Some(Ok(())) => Ok(()),
        Some(Err(status)) => Err(format!("status {status:?}")),
        None => Err("panic".into()),
    }
}

struct Mw2Hud {
    snap: HudSnapshot,
    cfg: Arc<Config>,
    toast: Toast,
    init_t: f64,
    vm_warned: bool,
    /// M7: the dpad_nuke icon uploaded through hudhook's texture loader (once).
    nuke_icon: Option<(hudhook::imgui::TextureId, u32, u32)>,
    nuke_icon_tried: bool,
    /// C3: the larger font of the damage numbers (added at initialize; None = the HUD font).
    dmg_font: Option<usize>,
}

impl Mw2Hud {
    fn new() -> Self {
        Mw2Hud {
            snap: HudSnapshot::empty(),
            cfg: Arc::new(Config::default()),
            toast: Toast { text: String::new(), until: 0.0, error: false },
            init_t: 0.0,
            vm_warned: false,
            nuke_icon: None,
            nuke_icon_tried: false,
            dmg_font: None,
        }
    }

    fn refresh(&mut self) {
        if let Some(s) = try_lock(&SNAPSHOT) {
            self.snap = s.clone();
        }
        if let Some(c) = shared::try_config() {
            self.cfg = c;
        }
        if let Some(t) = try_lock(&TOAST) {
            self.toast = t.clone();
        }
    }

    fn draw(&mut self, ui: &Ui) {
        self.refresh();
        let now = now_s();
        crate::viewmodel::check_no_frames(self.init_t, now, &mut self.vm_warned);
        let [w, h] = ui.io().display_size;
        if !(w > 0.0 && h > 0.0) {
            return;
        }
        let (_, s1080) = scales(h);
        // The ONE background draw list of this frame: imgui-rs allows a single live `DrawListMut` per list and
        // panics on a second `get_background_draw_list()` while this one lives (run 8 PANIC at the AC-130 start).
        // Every helper below takes `&dl`; m2_static_checks `hudwiring` keeps this the only call in the crate.
        let dl = ui.get_background_draw_list();
        let text = |pos: [f32; 2], col: [f32; 4], t: &str| {
            dl.add_text([pos[0] + 1.0, pos[1] + 1.0], BLACK, t);
            dl.add_text(pos, col, t);
        };

        // C3 (docs/C3_DESIGN.md 2.3): while ER shows a message our texts dim (crosshair, hitmarker and ammo stay)
        let dim = if self.cfg.ui_yield.enabled && self.cfg.ui_yield.mode != "off" {
            mw2er_core::ui_yield::hud_alpha(self.snap.ui_yield, self.cfg.ui_yield.hud_dim_alpha)
        } else {
            1.0
        };
        let dimmed = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * dim];
        if now < self.toast.until && !self.toast.text.is_empty() {
            // C3: off ER's HP / FP gauges (was the top-left corner)
            let a = self.cfg.ui_yield.toast_anchor;
            text([a[0] * w, a[1] * h], dimmed(if self.toast.error { RED } else { WHITE }), &self.toast.text);
        }

        let s = &self.snap;
        let cfg = &self.cfg;
        // C3 PASS C (docs/C3_DESIGN.md 3.5): the peers' tracers and explosion flashes (also with our MW2 mode off: they
        // are the peer's shots), on the same draw list, under everything else of ours
        if cfg.hud.enabled && s.in_world && !s.menu_open && !s.ac130.active && now - s.t < SNAPSHOT_MAX_AGE_S && (!s.peer_segs.is_empty() || !s.peer_flashes.is_empty()) {
            let (vf, _) = vfov_rad(s.vfov_raw, cfg.hud.fallback_vfov_deg);
            let yield_dim = 1.0 - 0.65 * s.ui_yield.clamp(0.0, 1.0);
            // review 2026-10-24: only the parts visible from the camera (`[coop] tracer_occlusion`); the head dot only on the
            // part that ends at the bullet
            for (a, b, alpha, tip) in &s.peer_segs {
                if let (Some(pa), Some(pb)) = (project(s.view_pos, s.cam_rows, vf, w, h, *a), project(s.view_pos, s.cam_rows, vf, w, h, *b)) {
                    let al = alpha * yield_dim;
                    dl.add_line(pa, pb, [1.0, 0.82, 0.45, 0.75 * al]).thickness(2.0 * s1080).build();
                    if *tip {
                        dl.add_circle(pb, 1.5 * s1080, [1.0, 0.95, 0.8, 0.9 * al]).filled(true).build();
                    }
                }
            }
            for (p, alpha, size) in &s.peer_flashes {
                if let Some(c) = project(s.view_pos, s.cam_rows, vf, w, h, *p) {
                    let d = ((p[0] - s.view_pos[0]).powi(2) + (p[1] - s.view_pos[1]).powi(2) + (p[2] - s.view_pos[2]).powi(2)).sqrt().max(1.0);
                    // the screen size of a sphere of `size` metres at distance d
                    let r = (size / d / (vf * 0.5).tan() * h * 0.5).clamp(3.0 * s1080, h * 0.4);
                    let al = alpha * yield_dim;
                    dl.add_circle(c, r, [1.0, 0.75, 0.35, 0.45 * al]).filled(true).build();
                    dl.add_circle(c, r * 0.5, [1.0, 0.95, 0.8, 0.6 * al]).filled(true).build();
                }
            }
        }
        if !(cfg.hud.enabled && s.mode_on && s.in_world && !s.menu_open && now - s.t < SNAPSHOT_MAX_AGE_S) {
            return;
        }
        // M7: the nuke countdown, hints and the streak counter (never in the gunship: a ride blocks the nuke)
        if cfg.nuke.enabled && !s.ac130.active {
            draw_nuke(ui, &dl, s, cfg, w, h, now, self.nuke_icon, dim);
        }
        // M4: the gunship HUD replaces the MW2 crosshair / ammo while an AC-130 ride is Active
        if s.ac130.active {
            draw_gunship(ui, &dl, s, cfg, w, h, now);
            return;
        }

        // Crosshair: four lines whose gap follows the hip spread, dark outline first. M3: it fades out while
        // aiming down the sights (MW2 hides it in ADS) and is dimmed while the camera gate blocks shots.
        let (vfov, _) = vfov_rad(s.vfov_raw, cfg.hud.fallback_vfov_deg);
        let c = crosshair(w, h, s.spread_deg, vfov, cfg.hud.min_gap_px, s.reticle_side, cfg.hud.crosshair_line_scale);
        // M8: no crosshair while the gun is down (mantle / ladder quick drop)
        // C6 aim (user 2026-10-21): third-person ADS in ER's crossbow aim keeps the same crosshair as first-person hip
        // (no scope there); its gap follows the ADS spread
        let ads_fade = if s.tp_aim_dot { 1.0 } else { (1.0 - s.ads_frac).clamp(0.0, 1.0) };
        let fade = ads_fade * if s.cam_valid { 1.0 } else { 0.4 } * if s.weapon_lowered { 0.0 } else { 1.0 };
        if fade > 0.01 {
            let outline = [BLACK[0], BLACK[1], BLACK[2], BLACK[3] * fade];
            let col = cfg.hud.crosshair_color;
            let col = [col[0], col[1], col[2], col[3] * fade];
            for seg in c.segs {
                dl.add_line(seg.a, seg.b, outline).thickness(c.thickness + 2.0).build();
            }
            for seg in c.segs {
                dl.add_line(seg.a, seg.b, col).thickness(c.thickness).build();
            }
        }

        // Hitmarker X (red on a kill), fading out.
        if let Some(hm) = hitmarker(w, h, now - s.last_hit_t, cfg.hud.hitmarker_s) {
            let base = if s.last_hit_kill { RED } else { [1.0, 1.0, 1.0, 1.0] };
            let col = [base[0], base[1], base[2], hm.alpha];
            for seg in hm.segs {
                dl.add_line(seg.a, seg.b, col).thickness(hm.thickness).build();
            }
        }

        // C3 (docs/C3_DESIGN.md 2.6): damage numbers at the hits, rising and fading (same draw list, after the
        // hitmarker); projected from the FP eye in first person, ER's camera in third person
        if cfg.hud.damage_numbers && !s.dmg_nums.is_empty() {
            let (vf, _) = vfov_rad(s.vfov_raw, cfg.hud.fallback_vfov_deg);
            let life = cfg.hud.damage_numbers_s as f64;
            let font = self.dmg_font.and_then(|i| ui.fonts().fonts().get(i).copied());
            let _font = font.map(|f| ui.push_font(f));
            for n in &s.dmg_nums {
                if cfg.hud.damage_numbers_suppress_with_er_tag && n.handle != 0 && s.er_tag_handles.contains(&n.handle) {
                    continue;
                }
                let Some((rise_m, rise_u, alpha)) = mw2er_core::dmgnum::style(now - n.t, life) else { continue };
                let Some(px) = project(s.view_pos, s.cam_rows, vf, w, h, [n.anchor[0], n.anchor[1] + rise_m, n.anchor[2]]) else { continue };
                if !(px[0].is_finite() && px[1].is_finite()) || px[0] < 0.0 || px[0] > w || px[1] < 0.0 || px[1] > h {
                    continue;
                }
                let t = n.text();
                let [tw, th] = ui.calc_text_size(&t);
                let a = (alpha * dim).clamp(0.0, 1.0);
                let col = match n.color() {
                    mw2er_core::dmgnum::DmgColor::White => [1.0, 1.0, 1.0, a],
                    mw2er_core::dmgnum::DmgColor::Yellow => [1.0, 0.86, 0.25, a],
                    mw2er_core::dmgnum::DmgColor::Red => [0.95, 0.2, 0.15, a],
                };
                let p = [px[0] - tw * 0.5, px[1] - th * 0.5 - 18.0 * s1080 * rise_u];
                dl.add_text([p[0] + 1.0, p[1] + 1.0], [0.0, 0.0, 0.0, 0.8 * a], &t);
                dl.add_text(p, col, &t);
            }
        }

        // Ammo "32 / 192" bottom-right; the magazine number turns red when low.
        let anchor = ammo_anchor(w, h);
        let mag_txt = s.mag.to_string();
        let rest_txt = format!(" / {}", s.reserve);
        let [mw, th] = ui.calc_text_size(&mag_txt);
        let [rw, _] = ui.calc_text_size(&rest_txt);
        let y = anchor[1] - th;
        let x = anchor[0] - mw - rw;
        text([x, y], if mag_is_low(s.mag, s.low_ammo_count) { RED } else { WHITE }, &mag_txt);
        text([x + mw, y], WHITE, &rest_txt);

        // M6: the gun's name (and its fire-mode tag) left of the ammo counter
        if cfg.hud.weapon_name && !s.gun_name.is_empty() {
            let label = if s.gun_tag.is_empty() { format!("{}   ", s.gun_name) } else { format!("{} [{}]   ", s.gun_name, s.gun_tag) };
            let [lw, _] = ui.calc_text_size(&label);
            text([x - lw, y], WHITE, &label);
        }
        // M6: the switch list (previous  [TARGET]  next) for a moment after a switch
        if cfg.hud.switch_list && now < s.switch_until && !s.switch_names[1].is_empty() {
            let t = format!("{}    [ {} ]    {}", s.switch_names[0], s.switch_names[1], s.switch_names[2]);
            let [tw, _] = ui.calc_text_size(&t);
            text([w * 0.5 - tw * 0.5, h * 0.80], dimmed(WHITE), &t);
        }
        // M6: lock-on brackets and text (Stinger: brackets on the target, Javelin: a box on the point)
        if cfg.hud.lock_ui && s.lock > 0 {
            let (label, col) = match s.lock {
                1 => ("LOCKING", WHITE),
                2 => ("LOCKED", RED),
                _ => ("NO LOCK", RED),
            };
            let [tw, _] = ui.calc_text_size(label);
            text([w * 0.5 - tw * 0.5, h * 0.62], dimmed(col), label);
            if let Some(t) = s.lock_target {
                let (vf, _) = vfov_rad(s.vfov_raw, cfg.hud.fallback_vfov_deg);
                if let Some(px) = project(s.cam_pos, s.cam_rows, vf, w, h, t) {
                    let r = if s.lock_point { 14.0 } else { 26.0 } * s1080;
                    let l = r * 0.45;
                    for (cx, cy, dx, dy) in [(-1.0f32, -1.0f32, 1.0f32, 1.0f32), (1.0, -1.0, -1.0, 1.0), (-1.0, 1.0, 1.0, -1.0), (1.0, 1.0, -1.0, -1.0)] {
                        let c = [px[0] + cx * r, px[1] + cy * r];
                        dl.add_line(c, [c[0] + dx * l, c[1]], col).thickness(2.0 * s1080).build();
                        dl.add_line(c, [c[0], c[1] + dy * l], col).thickness(2.0 * s1080).build();
                    }
                }
            }
        }
        // M6: launcher projectiles in flight (a short hot streak)
        if !s.projectiles.is_empty() {
            let (vf, _) = vfov_rad(s.vfov_raw, cfg.hud.fallback_vfov_deg);
            for (head, tail) in &s.projectiles {
                if let (Some(a), Some(b)) = (project(s.cam_pos, s.cam_rows, vf, w, h, *head), project(s.cam_pos, s.cam_rows, vf, w, h, *tail)) {
                    dl.add_line(b, a, [1.0, 0.85, 0.55, 0.85]).thickness(3.0 * s1080).build();
                    dl.add_circle(a, 3.0 * s1080, [1.0, 0.95, 0.8, 0.95]).filled(true).build();
                }
            }
        }

        // "MW2" mode badge above the ammo counter.
        let [bw, bh] = ui.calc_text_size("MW2");
        let pad = 6.0 * s1080;
        let bmax = [anchor[0], y - pad];
        let bmin = [anchor[0] - bw - 2.0 * pad, y - pad - bh - 2.0 * pad];
        dl.add_rect(bmin, bmax, BADGE).filled(true).rounding(4.0 * s1080).build();
        dl.add_text([bmin[0] + pad, bmin[1] + pad], [0.05, 0.05, 0.05, 1.0], "MW2");

        // M5: the stance figure left of the ammo counter (same draw list), only while the controller drives
        if cfg.hud.stance_icon {
            let size = th * 1.1;
            for (a, b) in stance_glyph(s.stance, x - 8.0 * s1080, y + th, size) {
                dl.add_line(a, b, BLACK).thickness(4.0 * s1080).build();
                dl.add_line(a, b, WHITE).thickness(2.0 * s1080).build();
            }
        }

        // M4: the AC-130 indicator under the ammo counter, and a 2 s READY toast
        if cfg.ac130.enabled && !s.ac130.indicator.is_empty() {
            let t = &s.ac130.indicator;
            let [iw, _] = ui.calc_text_size(t);
            let col = match s.ac130.indicator_kind {
                0 => WHITE,
                1 => [0.7, 0.7, 0.7, 0.9],
                _ => [0.6, 0.6, 0.6, 0.55],
            };
            text([anchor[0] - iw, anchor[1] + 4.0 * s1080], col, t);
        }
        if now - s.ac130.ready_toast_t < 2.0 {
            let t = "AC-130 READY";
            let [tw, _] = ui.calc_text_size(t);
            text([w * 0.5 - tw * 0.5, h * 0.25], WHITE, t);
        }

        // Reload indicator / out of ammo.
        if s.reloading {
            let label = "RELOADING";
            let [lw, lh] = ui.calc_text_size(label);
            let b = reload_bar(w, h, s.reload_progress, lh);
            text([b.text_center[0] - lw * 0.5, b.text_center[1] - lh * 0.5], WHITE, label);
            dl.add_rect(b.outer_min, b.outer_max, BLACK).filled(true).build();
            dl.add_rect(b.outer_min, b.fill_max, WHITE).filled(true).build();
        } else if s.mag == 0 && s.reserve == 0 {
            let label = "NO AMMO";
            let [lw, lh] = ui.calc_text_size(label);
            let b = reload_bar(w, h, 0.0, lh);
            text([b.text_center[0] - lw * 0.5, b.text_center[1] - lh * 0.5], RED, label);
        }
    }
}

/// Reads `hud.font_path` and accepts it only if `font_check` passes; otherwise logs why and returns
/// `None` (the built-in font is used).
fn load_user_font(path: &str) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    // Read at most MAX_FONT_BYTES + 1 so a huge file is rejected without loading all of it.
    let data = std::fs::File::open(path).and_then(|f| {
        let mut buf = Vec::new();
        f.take(MAX_FONT_BYTES as u64 + 1).read_to_end(&mut buf).map(|_| buf)
    });
    match data {
        Err(e) => {
            log_w!("HUD", "WARN hud.font_path {path}: {e}; using the built-in font");
            None
        }
        Ok(d) => match check_font(&d) {
            Ok(kind) => {
                log_i!("HUD", "font {path}: {kind}, {} bytes, structure check passed", d.len());
                Some(d)
            }
            Err(e) => {
                log_w!("HUD", "WARN hud.font_path {path} rejected ({e}); using the built-in font");
                None
            }
        },
    }
}

impl ImguiRenderLoop for Mw2Hud {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _rc: &'a mut dyn RenderContext) {
        set_thread_role("present");
        let ok = guard("hud.initialize", || {
            let cfg = shared::try_config().unwrap_or_else(|| Arc::new(Config::default()));
            let io = ctx.io_mut();
            // ImGui must never own the mouse or keyboard: HUD only.
            io.config_flags.insert(ConfigFlags::NO_MOUSE | ConfigFlags::NO_MOUSE_CURSOR_CHANGE);
            io.config_flags.remove(ConfigFlags::NAV_ENABLE_KEYBOARD | ConfigFlags::NAV_ENABLE_GAMEPAD);
            io.mouse_draw_cursor = false;
            let [dw, dh] = io.display_size;
            // The first font added becomes the default; hudhook builds the atlas after this returns.
            // A user font is structure-checked first: ImGui's asserts are compiled in and abort the
            // game on a font it cannot parse (empty file, wrong format, .ttc, WOFF).
            let px = cfg.hud.font_px;
            let ttf = load_user_font(&cfg.hud.font_path);
            let font = match &ttf {
                Some(data) => {
                    ctx.fonts().add_font(&[FontSource::TtfData { data, size_pixels: px, config: None }]);
                    format!("ttf {}", cfg.hud.font_path)
                }
                None => {
                    ctx.fonts().add_font(&[FontSource::DefaultFontData {
                        config: Some(FontConfig { size_pixels: px, ..FontConfig::default() }),
                    }]);
                    "built-in".to_string()
                }
            };
            // C3: a second, larger font for the damage numbers (the built-in font: the user font may lack digits)
            if cfg.hud.damage_numbers {
                let big = (px * 1.25 * cfg.hud.damage_numbers_scale).clamp(8.0, 160.0);
                ctx.fonts().add_font(&[FontSource::DefaultFontData { config: Some(FontConfig { size_pixels: big, ..FontConfig::default() }) }]);
                // its index in the atlas (FontId is not Send: the render loop must be)
                self.dmg_font = ctx.fonts().fonts().len().checked_sub(1);
            }
            log_i!("HUD", "initialize display={}x{} font={} {}px dmgnum_font={}", dw as u32, dh as u32, font, px, self.dmg_font.is_some() as u8);
            self.init_t = now_s();
        });
        HUD_OK.store(ok.is_some(), Ordering::SeqCst);
        if ok.is_none() {
            HUD_DISABLED.store(true, Ordering::SeqCst);
            log_e!("ERROR", "hud disabled (initialize panicked)");
        }
    }

    fn before_render<'a>(&'a mut self, _ctx: &mut Context, rc: &'a mut dyn RenderContext) {
        // M7: upload the dpad_nuke icon once (RGBA8 from converted\nuke); a failure only drops the icon
        if self.nuke_icon_tried || HUD_DISABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(d) = crate::data::NUKE_DATA.get() else { return };
        self.nuke_icon_tried = true;
        if let Some((w, h, px)) = d.icon.as_ref() {
            match guard("hud.nuke_icon", || rc.load_texture(px, *w, *h)) {
                Some(Ok(id)) => {
                    self.nuke_icon = Some((id, *w, *h));
                    log_i!("HUD", "nuke icon loaded {w}x{h}");
                }
                Some(Err(e)) => log_w!("HUD", "WARN nuke icon not loaded: {e:?}"),
                None => log_w!("HUD", "WARN nuke icon upload panicked"),
            }
        }
    }

    fn render(&mut self, ui: &mut Ui) {
        if HUD_DISABLED.load(Ordering::Relaxed) {
            return;
        }
        if guard("hud.render", || self.draw(ui)).is_none() {
            HUD_DISABLED.store(true, Ordering::SeqCst);
            log_e!("ERROR", "hud disabled (render panicked)");
        }
    }

    /// `Break` skips only ImGui's own message handling (including the WM_CHAR unwrap that panics
    /// on a lone UTF-16 surrogate); the game's WndProc still receives every message.
    fn before_wnd_proc(&self, _h: HWND, _m: u32, _w: WPARAM, _l: LPARAM) -> BeforeWndProc {
        BeforeWndProc::Break
    }

    fn message_filter(&self, _io: &Io) -> MessageFilter {
        MessageFilter::empty() // every window message still reaches the game
    }
}

/// MW2's `S.t` countdown text: `m:ss.t` (secondsascountdown + tenths).
pub fn nuke_timer_text(left_s: f32) -> String {
    let l = left_s.max(0.0);
    let total = (l * 10.0 + 1e-3).floor() as u32;
    let (secs, tenths) = (total / 10, total % 10);
    format!("{}:{:02}.{}", secs / 60, secs % 60, tenths)
}

/// M7 (docs/M7_DESIGN.md 3.3): the countdown at MW2's scorebar item (640x480 rect, sub-left alignment), its
/// `Friendly tactical nuke inbound!` line, the earn hint, refusals and the bottom-right streak counter.
#[allow(clippy::too_many_arguments)]
/// C3: `dim` (the UI yield's HUD alpha) fades the hints, refusals and the co-op note (never the countdown / counter).
fn draw_nuke(ui: &Ui, dl: &DrawListMut<'_>, s: &HudSnapshot, cfg: &Config, w: f32, h: f32, now: f64, icon: Option<(hudhook::imgui::TextureId, u32, u32)>, dim: f32) {
    let dimmed = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * dim];
    let n = &s.nuke;
    let (_, s1080) = scales(h);
    let Some(d) = crate::data::NUKE_DATA.get() else { return };
    let hd = &d.file.hud;
    let text = |pos: [f32; 2], col: [f32; 4], t: &str| {
        dl.add_text([pos[0] + 1.0, pos[1] + 1.0], BLACK, t);
        dl.add_text(pos, col, t);
    };
    let glow_text = |pos: [f32; 2], fg: [f32; 4], glow: [f32; 4], t: &str| {
        let g = [glow[0], glow[1], glow[2], glow[3] * 0.55];
        for (dx, dy) in [(-2.0f32, 0.0f32), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
            dl.add_text([pos[0] + dx * s1080, pos[1] + dy * s1080], g, t);
        }
        dl.add_text(pos, fg, t);
    };
    let key = cfg.keys.nuke.as_str();
    if let Some(left) = n.countdown {
        // MW2 rect x 112 y 42 on the 640x480 screen with sub-left alignment (+48 virtual px) [I]
        let sv = h / 480.0;
        let r = hd.timer_rect;
        let pos = [(r[0] + 48.0) * sv, r[1] * sv];
        glow_text(pos, hd.timer_forecolor, hd.timer_glowcolor, &nuke_timer_text(left));
        if now < n.inbound_until {
            let t = d.file.string("MP_FRIENDLY_TACTICAL_NUKE");
            text([pos[0], pos[1] + 34.0 * s1080], WHITE, t);
        }
    }
    if now < n.hint_until {
        let t = d.file.string("MP_EARNED_NUKE").replace("{+actionslot 4}", key);
        let [tw, _] = ui.calc_text_size(&t);
        glow_text([w * 0.5 - tw * 0.5, h * 0.30], dimmed(WHITE), dimmed(hd.timer_glowcolor), &t);
    }
    if now < n.refusal_until && n.refusal > 0 {
        let t = match n.refusal {
            1 => d.file.string("MP_NUKE_ALREADY_INBOUND").to_string(),
            2 => "Not available in the AC-130".to_string(),
            4 => mw2er_core::coop::COOP_BLOCKED.to_string(),
            _ => format!("{} not earned ({}/{})", d.file.string("MP_TACTICAL_NUKE"), n.streak, n.cost),
        };
        let [tw, _] = ui.calc_text_size(&t);
        text([w * 0.5 - tw * 0.5, h * 0.34], dimmed(RED), &t);
    }
    if now < n.coop_note_until {
        let t = mw2er_core::coop::COOP_NOTE;
        let [tw, _] = ui.calc_text_size(t);
        text([w * 0.5 - tw * 0.5, h * 0.38], dimmed(WHITE), t);
    }
    if n.show_counter {
        let anchor = ammo_anchor(w, h);
        let (label, col) = if n.earned {
            (format!("{}  [{key}]", d.file.string("MP_TACTICAL_NUKE").to_uppercase()), [hd.timer_glowcolor[0], 0.55, 0.1, 1.0])
        } else if n.test {
            (format!("Nuke: READY (test)  [{key}]"), WHITE)
        } else {
            (format!("Nuke: {}/{}", n.streak, n.cost), WHITE)
        };
        let [lw, lh] = ui.calc_text_size(&label);
        // above the MW2 badge (two text lines above the ammo counter)
        let y = anchor[1] - 3.6 * lh - 12.0 * s1080;
        let x = anchor[0] - lw;
        text([x, y], col, &label);
        if cfg.nuke.hud_icon && n.earned && let Some((id, iw, ih)) = icon {
            let ih_px = lh * 1.6;
            let iw_px = ih_px * iw as f32 / ih.max(1) as f32;
            dl.add_image(id, [x - iw_px - 8.0 * s1080, y - (ih_px - lh) * 0.5], [x - 8.0 * s1080, y - (ih_px - lh) * 0.5 + ih_px]).build();
        }
    }
}

/// IW4 `textalign` (MIDDLE_LEFT = 8 ...): low two bits horizontal (left/centre/right), the rest vertical
/// (1 top, 2 middle, 3 bottom) [I: IW4 menudefinition enums; only position, no MW2 code].
fn align_text(r: [f32; 4], tw: f32, th: f32, textalign: u32) -> [f32; 2] {
    let x = match textalign & 3 {
        1 => r[0] + r[2] * 0.5 - tw * 0.5,
        2 => r[0] + r[2] - tw,
        _ => r[0],
    };
    let y = match textalign >> 2 {
        2 => r[1] + r[3] * 0.5 - th * 0.5,
        3 => r[1] + r[3] - th,
        _ => r[1],
    };
    [x, y]
}

/// The gunship HUD (docs/M4_DESIGN.md section 11): the menu text blocks from `hud_layout.json`, gun labels (the
/// selected one bright, a reloading one blinking), countdown + timer bar, WHOT/BHOT, AGL, tracers, the 105 dark
/// pulse, the hitmarker and (debug) target markers. The film / overlay / grain come from the thermal pass.
fn draw_gunship(ui: &Ui, dl: &DrawListMut<'_>, s: &HudSnapshot, cfg: &Config, w: f32, h: f32, now: f64) {
    let a = &s.ac130;
    let (_, s1080) = scales(h);
    let fg = if a.black_hot { [0.0, 0.0, 0.0, 0.95] } else { [1.0, 1.0, 1.0, 0.95] };
    let sh = if a.black_hot { [1.0, 1.0, 1.0, 0.5] } else { BLACK };
    let text = |pos: [f32; 2], col: [f32; 4], t: &str| {
        dl.add_text([pos[0] + 1.0, pos[1] + 1.0], [sh[0], sh[1], sh[2], sh[3] * col[3]], t);
        dl.add_text(pos, col, t);
    };
    let Some(d) = AC_DATA.get() else {
        let t = timer_text(a.duration_s - (now - a.t0));
        text([w * 0.5, 24.0 * s1080], fg, &t);
        return;
    };
    // 105 mm dark pulse first (under the text)
    let p = &d.timing.dark_pulse;
    let pa = dark_pulse(now - a.pulse_t, p.alpha, p.fade_in_s, p.hold_s, p.fade_out_s);
    if pa > 0.001 {
        dl.add_rect([0.0, 0.0], [w, h], [0.0, 0.0, 0.0, pa]).filled(true).build();
    }
    let ms = now * 1000.0;
    let blink = blink_alpha(ms, d.hud.blink_ms, d.hud.blink_amp);
    let ids = mw2er_core::ac130_data::GUN_IDS;
    for item in &d.hud.items {
        let r = menu_rect(item.rect, w, h);
        let mut col = fg;
        let t: String = match item.key.as_str() {
            k if k.starts_with("label_") => {
                let gi = ids.iter().position(|g| k == format!("label_{g}")).unwrap_or(0);
                let label = d.hud.labels.get(ids[gi]).cloned().unwrap_or_default();
                if gi != a.gun {
                    col[3] *= 0.45;
                } else if a.reloading[gi] {
                    col[3] *= blink;
                }
                let clip = if a.clip_size[gi] > 1 { format!(" {}", a.clips[gi]) } else { String::new() };
                if gi == a.gun { format!("> {label}{clip}") } else { format!("  {label}{clip}") }
            }
            "countdown" => timer_text(a.duration_s - (now - a.t0)),
            "reloading" => {
                if !a.reloading[a.gun.min(2)] {
                    continue;
                }
                col[3] *= blink;
                d.hud.string("reloading").to_string()
            }
            "grain" => continue,
            _ => item.text.trim_start_matches('\n').to_string(),
        };
        if t.is_empty() {
            continue;
        }
        let [tw, th] = ui.calc_text_size(&t);
        text(align_text(r, tw, th, item.textalign), col, &t);
    }
    // timer bar under the countdown: remaining fraction of the ride
    let b = box43(w, h);
    let frac = (1.0 - (now - a.t0) / a.duration_s.max(1e-3)).clamp(0.0, 1.0) as f32;
    let bw = 120.0 * s1080;
    let bx = b[0] + b[2] - bw - 16.0 * s1080;
    let by = b[1] + b[3] - 12.0 * s1080;
    dl.add_rect([bx, by], [bx + bw, by + 4.0 * s1080], [0.0, 0.0, 0.0, 0.5]).filled(true).build();
    dl.add_rect([bx, by], [bx + bw * frac, by + 4.0 * s1080], fg).filled(true).build();
    // WHOT / BHOT and AGL in the top right of the box
    let vis = d.hud.string(if a.black_hot { "bhot" } else { "whot" });
    let agl = d.hud.string("agl").replace("&&1", &format!("{:.0}", a.agl_m * 3.28084));
    let [vw, vh] = ui.calc_text_size(vis);
    let rx = b[0] + b[2] - 16.0 * s1080;
    text([rx - vw, b[1] + 16.0 * s1080], fg, vis);
    let [aw, _] = ui.calc_text_size(&agl);
    text([rx - aw, b[1] + 20.0 * s1080 + vh], fg, &agl);
    // tracers (shells in flight) and debug markers
    // run 8: the D3D12 tracer pass draws the rounds (before the film); ImGui lines only when that pass is not running
    if cfg.ac130.tracers.enabled && !crate::thermal_present::TRACERS_GPU.load(std::sync::atomic::Ordering::Relaxed) {
        let view = TracerView { pos: a.cam_pos, rows: a.cam_rows, vfov: a.vfov, width: w, height: h };
        let mut inst = [TracerInstance::default(); MAX_INSTANCES];
        let n = build_instances(&a.tracers, now, &view, &cfg.ac130.tracers.params(), &mut inst);
        for t in &inst[..n] {
            let col = [fg[0], fg[1], fg[2], (t.intensity * 0.9).clamp(0.0, 1.0)];
            if t.kind > 0.5 {
                dl.add_circle(t.p0, t.half_w, col).filled(true).build();
            } else {
                dl.add_line(t.p0, t.p1, col).thickness((2.0 * t.half_w).max(1.0)).build();
            }
        }
    }
    if cfg.ac130.debug_markers {
        for m in &a.markers {
            if let Some(px) = project(a.cam_pos, a.cam_rows, a.vfov, w, h, *m) {
                let r = 8.0 * s1080;
                dl.add_rect([px[0] - r, px[1] - r], [px[0] + r, px[1] + r], [1.0, 0.3, 0.2, 0.8]).thickness(1.5).build();
            }
        }
    }
    if let Some(hm) = hitmarker(w, h, now - a.hit_t, cfg.hud.hitmarker_s) {
        let base = if a.hit_kill { RED } else { fg };
        let col = [base[0], base[1], base[2], hm.alpha];
        for seg in hm.segs {
            dl.add_line(seg.a, seg.b, col).thickness(hm.thickness).build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::lock;

    #[test]
    fn hud_nuke_timer_text_mw2_format() {
        assert_eq!(nuke_timer_text(9.4), "0:09.4");
        assert_eq!(nuke_timer_text(10.0), "0:10.0");
        assert_eq!(nuke_timer_text(0.05), "0:00.0");
        assert_eq!(nuke_timer_text(-1.0), "0:00.0");
        assert_eq!(nuke_timer_text(61.99), "1:01.9");
    }

    /// Run 8 regression: one ImGui frame of the normal MW2 HUD and one of the gunship HUD through `Mw2Hud::draw`
    /// (the code path that panicked with "The DrawListMut instance for the background draw list is already loaded").
    #[test]
    fn hud_gunship_frame_uses_one_background_draw_list() {
        let mut ctx = Context::create();
        ctx.set_ini_filename(None);
        ctx.io_mut().display_size = [1920.0, 1080.0];
        ctx.fonts().add_font(&[FontSource::DefaultFontData { config: None }]);
        ctx.fonts().build_rgba32_texture();
        let mut hud = Mw2Hud::new();
        for active in [false, true, true] {
            {
                let mut s = lock(&SNAPSHOT);
                *s = HudSnapshot::empty();
                s.t = now_s();
                s.in_world = true;
                s.mode_on = true;
                s.ac130.active = active;
                s.ac130.t0 = now_s();
                // M7: a countdown, hint and counter in the same frame (draw_nuke shares the one draw list)
                s.nuke.countdown = Some(9.4);
                s.nuke.show_counter = !active;
                s.nuke.hint_until = now_s() + 1.0;
            }
            let ui = ctx.new_frame();
            hud.draw(ui);
            let _ = ctx.render();
        }
    }
}
