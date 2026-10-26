//! Development only (compiled for tests): renders the wizard's pages on fixture data into PNG files, without a
//! window or a GPU. egui's output (triangles + its font atlas) goes through a small CPU rasterizer that blends like
//! the glow backend (premultiplied alpha in gamma space), so a screenshot looks like the real window.
//!
//! `cargo test -p installer --bin MW2inER-setup shots -- --ignored` with `MW2ER_UI_SHOTS=<dir>` writes one PNG per
//! page state into `<dir>` (default `work\installer\ui\after`; the P2 set: `work\installer\ui\p2`, the release made
//! with the built-in regulation builder, plus the Advanced WitchyBND fallback; the P3 set: `work\installer\ui\p3`, plus
//! the update states 16-22: Update over an older install, the Update card of a DLL-only and of a rows release, the
//! update running, Roll back, the opt-in release check, Done after an update; the P4 set: `work\installer\ui\p4`, the
//! public display name and the update from 0.2.0 to this version). The fixture lives in `work\installer\ui\fixture`
//! (deleted afterwards); every other path shown is made up. Nothing is clicked or applied: no dialog, thread,
//! download or tool starts, and no real game, Steam, me3 or Seamless folder is read.

use super::*;
use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{Color32, Pos2, Rect, TextureId, Vec2, ViewportId, epaint};
use installer::plan::{Step, StepId};
use installer::regulation::Tool;
use installer::seamless::IniState;
use installer::update::{Change, Kind, Summary};

struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

#[derive(Default)]
pub struct Renderer {
    tex: HashMap<TextureId, Tex>,
}

pub struct Shot {
    pub w: usize,
    pub h: usize,
    /// Straight RGBA8.
    pub rgba: Vec<u8>,
}

impl Renderer {
    fn apply(&mut self, d: &egui::TexturesDelta) {
        for (id, deltas) in &d.set {
            for im in deltas {
                let egui::ImageData::Color(c) = &im.image;
                let [w, h] = c.size;
                match im.pos {
                    None => {
                        self.tex.insert(*id, Tex { w, h, px: c.pixels.clone() });
                    }
                    Some([x0, y0]) => {
                        if let Some(t) = self.tex.get_mut(id) {
                            for y in 0..h {
                                for x in 0..w {
                                    if x0 + x < t.w && y0 + y < t.h {
                                        t.px[(y0 + y) * t.w + x0 + x] = c.pixels[y * w + x];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn free(&mut self, d: &egui::TexturesDelta) {
        for id in &d.free {
            self.tex.remove(id);
        }
    }

    fn sample(&self, id: TextureId, uv: Pos2) -> [f32; 4] {
        let Some(t) = self.tex.get(&id) else { return [1.0; 4] };
        // bilinear, clamped
        let fx = (uv.x * t.w as f32 - 0.5).clamp(0.0, (t.w - 1) as f32);
        let fy = (uv.y * t.h as f32 - 0.5).clamp(0.0, (t.h - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(t.w - 1), (y0 + 1).min(t.h - 1));
        let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| {
            let c = t.px[y * t.w + x];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        let mut o = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * ax;
            let bot = c[k] + (d[k] - c[k]) * ax;
            o[k] = (top + (bot - top) * ay) / 255.0;
        }
        o
    }

    /// Rasterizes the primitives into a `w` x `h` premultiplied canvas.
    fn paint(&self, prims: &[epaint::ClippedPrimitive], ppp: f32, w: usize, h: usize) -> Shot {
        let mut cv = vec![[0.0f32, 0.0, 0.0, 1.0]; w * h];
        for p in prims {
            let epaint::Primitive::Mesh(m) = &p.primitive else { continue };
            let c = p.clip_rect;
            let cx0 = (c.min.x * ppp).round().max(0.0) as i64;
            let cy0 = (c.min.y * ppp).round().max(0.0) as i64;
            let cx1 = ((c.max.x * ppp).round() as i64).min(w as i64);
            let cy1 = ((c.max.y * ppp).round() as i64).min(h as i64);
            if cx0 >= cx1 || cy0 >= cy1 {
                continue;
            }
            for t in m.indices.as_chunks::<3>().0 {
                let v = [&m.vertices[t[0] as usize], &m.vertices[t[1] as usize], &m.vertices[t[2] as usize]];
                let mut q = [v[0].pos.to_vec2() * ppp, v[1].pos.to_vec2() * ppp, v[2].pos.to_vec2() * ppp];
                let mut vi = [0usize, 1, 2];
                let edge = |a: Vec2, b: Vec2, p: Vec2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                let mut area = edge(q[0], q[1], q[2]);
                if area == 0.0 {
                    continue;
                }
                if area < 0.0 {
                    q.swap(1, 2);
                    vi.swap(1, 2);
                    area = -area;
                }
                // each edge owned by exactly one of the two triangles that share it (no double blend on seams)
                let owns = |a: Vec2, b: Vec2| {
                    let d = b - a;
                    d.y < 0.0 || (d.y == 0.0 && d.x > 0.0)
                };
                let o = [owns(q[1], q[2]), owns(q[2], q[0]), owns(q[0], q[1])];
                let minx = q.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor() as i64;
                let maxx = q.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil() as i64;
                let miny = q.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor() as i64;
                let maxy = q.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil() as i64;
                let (x0, x1, y0, y1) = (minx.max(cx0), maxx.min(cx1), miny.max(cy0), maxy.min(cy1));
                let col = |k: usize| {
                    let c = v[vi[k]].color;
                    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
                };
                let cols = [col(0), col(1), col(2)];
                let uvs = [v[vi[0]].uv, v[vi[1]].uv, v[vi[2]].uv];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let pc = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = edge(q[1], q[2], pc);
                        let w1 = edge(q[2], q[0], pc);
                        let w2 = edge(q[0], q[1], pc);
                        let inside = |w: f32, own: bool| w > 0.0 || (w == 0.0 && own);
                        if !(inside(w0, o[0]) && inside(w1, o[1]) && inside(w2, o[2])) {
                            continue;
                        }
                        let (b0, b1, b2) = (w0 / area, w1 / area, w2 / area);
                        let uv = Pos2::new(uvs[0].x * b0 + uvs[1].x * b1 + uvs[2].x * b2, uvs[0].y * b0 + uvs[1].y * b1 + uvs[2].y * b2);
                        let s = self.sample(m.texture_id, uv);
                        let d = &mut cv[y as usize * w + x as usize];
                        let src: [f32; 4] = std::array::from_fn(|k| (cols[0][k] * b0 + cols[1][k] * b1 + cols[2][k] * b2) * s[k]);
                        for k in 0..4 {
                            d[k] = src[k] + d[k] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in cv {
            let a = p[3].clamp(1e-6, 1.0);
            for c in &p[..3] {
                rgba.push(((c / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            rgba.push(255);
        }
        Shot { w, h, rgba }
    }
}

/// Runs a few frames of the window's drawing (layout settles, fonts load, a hovered tooltip opens) and paints
/// the last one. `size` in points; `ppp` = the display scale (1.0 = 100 %).
pub fn render(app: &mut App, ctx: &egui::Context, r: &mut Renderer, size: Vec2, ppp: f32, hover: Option<Pos2>) -> Shot {
    let mut last = None;
    for k in 0..6 {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(100.0 + k as f64),
            max_texture_side: Some(8192),
            ..Default::default()
        };
        if let Some(v) = input.viewports.get_mut(&ViewportId::ROOT) {
            v.native_pixels_per_point = Some(ppp);
        }
        if let Some(p) = hover.filter(|_| k == 0) {
            input.events.push(egui::Event::PointerMoved(p));
        }
        let out = ctx.run_ui(input, |ui| app.draw(ui));
        r.apply(&out.textures_delta);
        if k < 5 {
            r.free(&out.textures_delta);
        }
        app.actions.clear(); // drawing only: nothing is ever applied
        if let Some(mut prev) = last.replace(out) {
            prev.textures_delta.clear();
        }
    }
    let mut out = last.unwrap();
    let ppp = out.pixels_per_point;
    let prims = ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
    let (w, h) = ((size.x * ppp).round() as usize, (size.y * ppp).round() as usize);
    let shot = r.paint(&prims, ppp, w, h);
    r.free(&out.textures_delta);
    out.textures_delta.clear();
    shot
}

pub fn save_png(s: &Shot, path: &Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), s.w as u32, s.h as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&s.rgba).unwrap();
}

// ---- fixtures ----------------------------------------------------------------------------------------------------

pub fn ui_dir() -> PathBuf {
    installer::util::normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\work\installer\ui"))
}

const ER: &str = r"C:\Program Files (x86)\Steam\steamapps\common\ELDEN RING\Game";
const MW2: &str = r"D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2";
const ME3: &str = r"C:\Users\Player\AppData\Local\Programs\garyttierney\me3\bin\me3.exe";
const ROOT: &str = r"C:\Users\Player\AppData\Local\MW2inER";

fn ck(id: &'static str, level: Level, msg: &str, fix: Option<Fix>) -> Check {
    Check { id, level, msg: msg.into(), fix }
}

/// Every row of a good PC (messages as installer::detect::evaluate writes them).
fn good_checks() -> Vec<Check> {
    vec![
        ck("er.found", Level::Ok, &format!("Elden Ring: {ER}"), None),
        ck("er.version", Level::Ok, "eldenring.exe 2.7.2.0", None),
        ck("er.build", Level::Ok, "Steam build 25827938", None),
        ck("er.regulation", Level::Ok, "vanilla regulation.bin 3f9a51c0e2d7", None),
        ck("mw2.found", Level::Ok, &format!("MW2: {MW2}"), None),
        ck("mw2.build", Level::Ok, "Steam build 25374375, iw4sp.exe == release", None),
        ck("me3", Level::Ok, &format!("me3 0.13.0: {ME3}"), None),
        ck("seamless", Level::Ok, r"Seamless Co-op 2.0.1 (zip C:\Users\Player\Downloads\Seamless Co-op-510-2-0-1.zip)", None),
        ck("python", Level::Ok, r"Python: the bundled CPython 3.14.7 (python-3.14.7-embed-amd64.zip, sha256-pinned) goes into runtime\python", None),
        ck("disk", Level::Ok, "212 GB free", None),
    ]
}

/// P2 fallback (the Install page's Advanced box): the WitchyBND row is back and the builder row says so.
fn fallback_checks() -> Vec<Check> {
    let mut v = good_checks();
    let at = v.len() - 1;
    v.insert(at, ck("witchy", Level::Warn, "WitchyBND 3.0.1.0 will be downloaded from its official GitHub release (48 MB, sha256-pinned), or choose the zip", Some(Fix::DownloadWitchy)));
    v.insert(
        at + 1,
        ck(
            "regulation.tool",
            Level::Warn,
            "Regulation builder: WitchyBND 3.0.1.0 (fallback) instead of this release's built in (regbuild). The params are checked against the release by content; the regulation file and the co-op id differ from the host's (both players must use the same builder)",
            None,
        ),
    );
    v
}

/// The Games page with problems: Elden Ring out of date (red), MW2 differs (yellow).
fn games_fail_checks() -> Vec<Check> {
    let mut v = good_checks();
    for c in &mut v {
        match c.id {
            "er.version" => {
                *c = ck(
                    "er.version",
                    Level::Fail,
                    "eldenring.exe is 2.6.1.0, this mod needs 2.7.2.0 (Steam build 25827938, the 2026-10-20 update). Update Elden Ring in Steam, or wait for the mod update if the game is newer",
                    Some(Fix::UpdateInSteam(installer::steam::APP_ELDEN_RING)),
                )
            }
            "er.build" => *c = ck("er.build", Level::Warn, "Steam build 24811350, the release was made on 25827938 (the exe version decides)", None),
            "mw2.build" => {
                *c = ck(
                    "mw2.build",
                    Level::Warn,
                    "MW2 differs from the release (build 25374375, iw4sp.exe 9c1e04a2b7d3; release: build 25374375, 5ad0e8f1c2b9). Install continues; the verify step decides",
                    None,
                )
            }
            _ => {}
        }
    }
    v
}

/// The co-op plan of a fresh install; `witchy` = with the WitchyBND step (the P2 fallback).
fn full_plan(running: bool, witchy: bool) -> Plan {
    let skip = |s: StepId| matches!(s, StepId::Payload) && !running;
    Plan {
        root: ROOT.into(),
        mode: Mode::Coop,
        steps: StepId::ALL
            .into_iter()
            .filter(|id| witchy || *id != StepId::Witchy)
            .map(|id| Step {
                id,
                inputs: Vec::new(),
                deps: Vec::new(),
                outputs: Vec::new(),
                clean: Vec::new(),
                policy: installer::plan::SkipPolicy::Never,
                key: String::new(),
                legacy_key: None,
                decision: if skip(id) { Decision::Skip("unchanged".into()) } else { Decision::Run(if id == StepId::Preflight { "always".into() } else { "new".into() }) },
            })
            .collect(),
    }
}

/// An install of this release (repair / Done / Uninstall pages); [`installed_old`] = the 0.2.0 install an update
/// starts from.
fn installed_state() -> InstallState {
    InstallState {
        schema: installer::state::SCHEMA,
        release_version: env!("CARGO_PKG_VERSION").into(),
        coop_id: "5b00e3bc".into(),
        mode: Mode::Coop,
        last_result: "PASS 681/681 verified".into(),
        shortcuts: vec![
            r"C:\Users\Player\Desktop\Modern Arsenal for Elden Ring.lnk".into(),
            r"C:\Users\Player\Desktop\Modern Arsenal for Elden Ring - Co-op.lnk".into(),
            r"C:\Users\Player\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Modern Arsenal for Elden Ring\Modern Arsenal for Elden Ring - Setup.lnk".into(),
        ],
        ..Default::default()
    }
}

/// P4: the install of the last release before this one (0.2.0, old shortcut names) for the update states.
fn installed_old() -> InstallState {
    let mut st = installed_state();
    st.release_version = "0.2.0".into();
    st.coop_id = "2799328d".into();
    st.shortcuts = installer::run::shortcut_names(installer::LEGACY_DISPLAY_NAMES[0]).into_iter().take(2).map(|n| PathBuf::from(r"C:\Users\Player\Desktop").join(n)).collect();
    st
}

/// The fixture: a release with pins only (work\installer\ui\fixture\pkg), an installed-looking root for the
/// Uninstall page, and an Env whose folders all point into the fixture (no registry, no Steam).
pub struct Fixture {
    pub dir: PathBuf,
    env: Env,
    args: Args,
}

impl Fixture {
    pub fn new(tag: &str) -> Fixture {
        let dir = ui_dir().join(format!("fixture-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("manifest")).unwrap();
        std::fs::write(
            pkg.join(release::HOST_PINS),
            r#"{"er_exe_version":"2.7.2.0","er_app_version":"Steam build 25827938, the 2026-10-20 update","me3_version":"0.13.0","seamless_version":"2.0.1","ersc_sha256":"00","witchybnd_zip_sha256":"0000000000000000000000000000000000000000000000000000000000000000","regulation_builder":"regbuild","package_bnd4_sha256":"1111111111111111111111111111111111111111111111111111111111111111","rows_json_core_sha256":"2222222222222222222222222222222222222222222222222222222222222222"}"#,
        )
        .unwrap();
        std::fs::write(pkg.join(release::HOST_MANIFEST), r#"{"files":{}}"#).unwrap();
        // an install for the Uninstall page (only what uninstall::plan reads)
        let root = dir.join("MW2inER");
        for d in ["state", "build", "converted", "logs", "profiles", "runtime", "seamless", "backups", "tools", "manifest"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("mw2er.toml"), "").unwrap();
        std::fs::write(root.join("MW2inER.exe"), "").unwrap();
        let mut st = installed_state();
        st.shortcuts.clear();
        std::fs::write(root.join(installer::state::STATE_FILE), serde_json::to_string(&st).unwrap()).unwrap();
        let env = Env {
            local_appdata: Some(dir.join("Local")),
            appdata: Some(dir.join("Roaming")),
            downloads: Some(dir.join("Downloads")),
            documents: Some(dir.join("Documents")),
            ..Default::default()
        };
        let args = Args { root: Some(dir.join("fresh")), payload: Some(pkg), mode: Some(Mode::Coop), no_register: true, no_shortcuts: true, ..Default::default() };
        Fixture { dir, env, args }
    }

    /// A window on the fixture with a detection result whose rows are `checks`; paths shown are made up.
    pub fn app(&self, checks: Vec<Check>) -> App {
        let mut app = App::new(self.args.clone(), self.env.clone(), Start::Wizard, Default::default(), Default::default());
        assert!(app.release.is_some(), "{:?}", app.payload);
        let mut req = app.request();
        req.probe_python = false;
        let mut s = Session::prepare(self.env.clone(), req).unwrap();
        s.checks = checks;
        s.facts.seamless.copy_ini = None;
        app.session = Some(s);
        app.plan = Some(full_plan(false, false));
        app.root = ROOT.into();
        app.env.steam_path = Some(r"C:\Program Files (x86)\Steam".into());
        app.env.local_appdata = Some(r"C:\Users\Player\AppData\Local".into());
        app.desktop = true;
        app.allow_download = true;
        app.start_menu = true;
        app
    }

    pub fn uninstall_app(&self) -> App {
        let mut app = App::new(self.args.clone(), self.env.clone(), Start::Uninstall, Default::default(), Default::default());
        app.root = self.dir.join("MW2inER");
        app.installed = InstallState::load(&app.root);
        app
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// (file name, the window in that state).
pub fn states(fx: &Fixture) -> Vec<(&'static str, App, Vec2, f32)> {
    let size = Vec2::new(860.0, 660.0);
    let mut v: Vec<(&'static str, App, Vec2, f32)> = Vec::new();

    let mut a = fx.app(good_checks());
    a.page = Page::Welcome;
    a.session = None;
    a.plan = None;
    v.push(("01_welcome", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Welcome;
    a.installed = Some(installed_state());
    a.session = None;
    a.report = Some((
        "Verify".into(),
        "verify C:\\Users\\Player\\AppData\\Local\\MW2inER\nOK   681/681 files match the release (sha256)\nOK   co-op id 5b00e3bc\nPASS".into(),
    ));
    v.push(("02_welcome_installed", a, size, 1.0));

    let mut a = fx.app(games_fail_checks());
    a.page = Page::Games;
    v.push(("03_games_fail", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Games;
    v.push(("04_games_ok", a, size, 1.0));

    let mut a = fx.app(
        good_checks()
            .into_iter()
            .map(|c| if c.id == "me3" { ck("me3", Level::Fail, "me3 0.13.0 (the mod loader) is not installed", Some(Fix::RunMe3Installer(r"C:\Users\Player\Downloads\MW2inER-0.3.0\tools\me3\me3_installer.exe".into()))) } else { c })
            .collect(),
    );
    a.page = Page::Me3;
    if let Some(s) = a.session.as_mut() {
        s.facts.me3.bundled_installer = Some((r"C:\Users\Player\Downloads\MW2inER-0.3.0\tools\me3\me3_installer.exe".into(), true));
    }
    v.push(("05_me3_missing", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Seamless;
    a.password = "tarnished-ac130".into();
    v.push(("06_seamless_password", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Seamless;
    a.mode = Mode::Coop;
    a.pw_only = true;
    a.keep_password = false;
    if let Some(s) = a.session.as_mut() {
        s.facts.seamless.copy_ini = Some(IniState { password_set: true, password_line: true, ..Default::default() });
    }
    v.push(("07_seamless_change_password", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    v.push(("08_install_ready", a, size, 1.0));

    // P2: Advanced opened, the WitchyBND fallback ticked
    let mut a = fx.app(fallback_checks());
    a.page = Page::Install;
    a.witchy_fallback = true;
    if let Some(s) = a.session.as_mut() {
        s.regulation_tool = Tool::Witchy;
    }
    a.plan = Some(full_plan(false, true));
    v.push(("08b_install_witchybnd_fallback", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    a.view.apply(&Progress::Plan(full_plan(true, false).steps.iter().map(|s| (s.id, s.decision.runs())).collect()));
    for id in [StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime, StepId::Seamless] {
        a.view.apply(&Progress::StepStart { id, index: 0, total: 13 });
        a.view.apply(&Progress::Line { id, text: format!("OK   {} done", id.as_str()) });
        a.view.apply(&Progress::StepDone { id, ok: true, secs: 2.0, msg: "ok".into() });
    }
    a.view.apply(&Progress::StepStart { id: StepId::Dumps, index: 5, total: 13 });
    for k in 1..=7 {
        a.view.apply(&Progress::Line { id: StepId::Dumps, text: format!("OK   unlinker run {k}/17 (mp_{k:02}.ff)") });
    }
    a.view.apply(&Progress::Fraction { id: StepId::Dumps, value: 7.0 / 17.0 });
    let (tx, rx) = mpsc::channel();
    std::mem::forget(tx); // keeps the fake run "running" (nothing is ever sent)
    a.run = Some(Running { rx, cancel: Arc::new(AtomicBool::new(false)) });
    v.push(("09_install_progress", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    a.view.apply(&Progress::Plan(full_plan(true, false).steps.iter().map(|s| (s.id, s.decision.runs())).collect()));
    for id in [StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime] {
        a.view.apply(&Progress::StepStart { id, index: 0, total: 13 });
        a.view.apply(&Progress::StepDone { id, ok: true, secs: 2.0, msg: "ok".into() });
    }
    a.view.apply(&Progress::StepStart { id: StepId::Seamless, index: 4, total: 13 });
    a.view.apply(&Progress::Line { id: StepId::Seamless, text: "FAIL the zip has no SeamlessCoop\\ersc.dll".into() });
    a.view.apply(&Progress::StepDone { id: StepId::Seamless, ok: false, secs: 0.4, msg: "the zip has no SeamlessCoop\\ersc.dll".into() });
    a.view.apply(&Progress::Finished { ok: false, msg: "INSTALL FAILED: seamless: the zip has no SeamlessCoop\\ersc.dll".into() });
    v.push(("10_install_failed", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Done;
    a.installed = Some(installed_state());
    v.push(("11_done", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Done;
    a.installed = Some(installed_state());
    a.report = Some((
        "Play co-op".into(),
        "FAIL Steam is not running: start Steam first\n\nNot started: fix the FAIL rows above (Steam must be running; this program never starts it).".into(),
    ));
    v.push(("12_done_report", a, size, 1.0));

    let a = fx.uninstall_app();
    v.push(("13_uninstall", a, size, 1.0));

    // ---- P3: the update path ----
    let upd = |a: &mut App, version: &str| {
        if let Some(r) = a.release.as_mut() {
            r.version = version.into();
            r.update_repo = "owner/MW2inER".into();
        }
    };
    let mut a = fx.app(good_checks());
    a.page = Page::Welcome;
    upd(&mut a, "0.3.0");
    a.installed = Some(installed_old());
    a.session = None;
    a.plan = None;
    a.check_updates = true;
    a.update_check = Some(Ok(Latest { tag: "0.3.1".into(), newer: true, page: "https://github.com/owner/MW2inER/releases/tag/0.3.1".into(), sha256: vec![] }));
    v.push(("16_welcome_update", a, size, 1.0));

    let dll_only = Summary {
        kind: Kind::Update,
        from_version: "0.2.0".into(),
        to_version: "0.3.0".into(),
        from_coop_id: "2799328d".into(),
        to_coop_id: "6c1e0f42".into(),
        changes: vec![Change { what: "The mod (mw2er.dll)".into(), detail: "1 changed: mw2er.dll".into() }],
        steps: vec![StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime, StepId::Seamless, StepId::VerifyConverted, StepId::VerifyAll, StepId::Cleanup, StepId::Profiles, StepId::Finish],
        settings: Some("mw2er.toml merged: 5 of your setting(s) kept".into()),
    };
    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    upd(&mut a, "0.3.0");
    a.installed = Some(installed_old());
    a.update = Some(dll_only.clone());
    v.push(("17_install_update_dll_only", a, size, 1.0));

    let mut rows = dll_only.clone();
    rows.to_version = "0.3.1".into();
    rows.to_coop_id = "91d3a7e0".into();
    rows.changes = vec![Change { what: "Regulation build settings (mw2er.toml)".into(), detail: "[regulation]".into() }];
    rows.steps.insert(6, StepId::Regulation);
    rows.settings = Some("mw2er.toml merged: 5 of your setting(s) kept, 1 new default(s), 1 set by the release".into());
    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    upd(&mut a, "0.3.1");
    a.installed = Some(installed_old());
    a.update = Some(rows);
    v.push(("18_install_update_rows", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Install;
    upd(&mut a, "0.3.0");
    a.update = Some(dll_only.clone());
    let plan: Vec<(StepId, bool)> = StepId::ALL.into_iter().filter(|id| *id != StepId::Witchy).map(|id| (id, dll_only.steps.contains(&id))).collect();
    a.view.apply(&Progress::Plan(plan));
    for id in [StepId::Dumps, StepId::Converters, StepId::Regulation] {
        a.view.apply(&Progress::Skipped { id, reason: "inputs unchanged, outputs present".into() });
    }
    for id in [StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime, StepId::Seamless] {
        a.view.apply(&Progress::StepStart { id, index: 0, total: 13 });
        a.view.apply(&Progress::StepDone { id, ok: true, secs: 0.4, msg: "ok".into() });
    }
    a.view.apply(&Progress::StepStart { id: StepId::VerifyConverted, index: 5, total: 13 });
    let (tx, rx) = mpsc::channel();
    std::mem::forget(tx);
    a.run = Some(Running { rx, cancel: Arc::new(AtomicBool::new(false)) });
    v.push(("19_update_running", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Welcome;
    let mut st = installed_state();
    st.release_version = "0.3.0".into();
    st.coop_id = "6c1e0f42".into();
    a.installed = Some(st);
    a.session = None;
    a.plan = None;
    upd(&mut a, "0.3.0");
    a.rollback = Some(("0.3.0".into(), "0.2.0".into()));
    a.confirm_rollback = true;
    v.push(("20_welcome_rollback", a, size, 1.0));

    let mut a = fx.app(good_checks());
    let mut st = installed_old();
    st.last_result = "ROLLED BACK from 0.3.0 to 0.2.0".into();
    a.installed = Some(st);
    a.note = Some((Level::Ok, "Rolled back to 0.2.0: its files, settings and record are back. The co-op id is 2799328d again: both players must use 0.2.0.".into()));
    a.page = Page::Welcome;
    a.session = None;
    a.plan = None;
    v.push(("21_welcome_rolled_back", a, size, 1.0));

    let mut a = fx.app(good_checks());
    a.page = Page::Done;
    let mut st = installed_state();
    st.release_version = "0.3.0".into();
    st.coop_id = "6c1e0f42".into();
    a.installed = Some(st);
    a.update = Some(dll_only.clone());
    v.push(("22_done_after_update", a, size, 1.0));

    // the smallest window at 150 % display scale
    let mut a = fx.app(games_fail_checks());
    a.page = Page::Games;
    v.push(("14_games_fail_min_150pct", a, Vec2::new(760.0, 560.0), 1.5));
    v
}

/// Writes every state to `MW2ER_UI_SHOTS` (default work\installer\ui\after).
#[test]
#[ignore = "writes PNG files; run on request"]
fn shots() {
    let out = std::env::var_os("MW2ER_UI_SHOTS").map(PathBuf::from).unwrap_or_else(|| ui_dir().join("after"));
    std::fs::create_dir_all(&out).unwrap();
    let fx = Fixture::new("shots");
    for (name, mut app, size, ppp) in states(&fx) {
        let ctx = egui::Context::default();
        style(&ctx);
        let mut r = Renderer::default();
        let s = render(&mut app, &ctx, &mut r, size, ppp, None);
        save_png(&s, &out.join(format!("{name}.png")));
    }
    // after the redesign only: a row's tooltip (the full message) under the mouse
    if out.ends_with("after") || out.ends_with("p2") || out.ends_with("p3") {
        let mut app = fx.app(games_fail_checks());
        app.page = Page::Games;
        let ctx = egui::Context::default();
        style(&ctx);
        let s = render(&mut app, &ctx, &mut Renderer::default(), Vec2::new(860.0, 660.0), 1.0, Some(Pos2::new(120.0, 249.0)));
        save_png(&s, &out.join("15_games_row_tooltip.png"));
    }
}

/// The rasterizer draws something (text and frames) for a page, in memory only.
#[test]
fn renderer_draws_a_page() {
    let fx = Fixture::new("test");
    let mut app = fx.app(games_fail_checks());
    app.page = Page::Games;
    let ctx = egui::Context::default();
    style(&ctx);
    let s = render(&mut app, &ctx, &mut Renderer::default(), Vec2::new(400.0, 300.0), 1.0, None);
    assert_eq!((s.w, s.h), (400, 300));
    let distinct: std::collections::HashSet<[u8; 3]> = s.rgba.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]).collect();
    assert!(distinct.len() > 20, "only {} colours", distinct.len());
    assert!(app.actions.is_empty());
}
