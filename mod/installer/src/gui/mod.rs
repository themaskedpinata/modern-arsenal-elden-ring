//! The setup window (docs/INSTALLER_DESIGN.md 2.3, decision D3): an eframe/egui wizard over the library's session
//! ([`installer::run::Session`]): Welcome -> Games -> me3 -> Seamless Co-op (co-op only) -> Install -> Done, plus
//! the Uninstall page (Apps & Features / the Setup shortcut), Repair / Update (the same pages; unchanged steps are
//! skipped by the plan), Verify and the Play buttons. P3: the setup started over an older install offers
//! "Update 0.2.0 -> 0.2.1"; the Install page then shows the Update card (what changes, the steps that run, the co-op
//! id before / after with "both players must update"); the Installed card has "Roll back to X" after an update, and
//! the Welcome page an opt-in "Check GitHub for a newer release" (only for a release with `update_repo`; off by
//! default; it opens the release page on a click, never downloads).
//!
//! Everything slow runs on a worker thread (detection, the install, verify, the official me3 installer) and talks
//! to the window over channels; the window only draws and reacts. The co-op password lives in the masked field
//! until Install, then moves into a [`Secret`] for the worker (it is written only into the mod's own
//! ersc_settings.ini copy; never logged, shown or stored elsewhere). Nothing here starts Steam or the game: the
//! Steam buttons open store / library pages (steam://nav/...) on the player's click, and Play goes through the
//! launcher's checks before me3 is started.

mod logic;
#[cfg(test)]
mod shots;
mod theme;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Margin, Response, RichText};
use installer::cli::{self, Args};
use installer::detect::{Check, Env, Fix, Input, Level};
use installer::guard::Guard;
use installer::plan::{Decision, Plan};
use installer::regulation::Tool;
use installer::release::{self, Release};
use installer::run::{self, Progress, Request, Session, ShortcutDirs};
use installer::seamless::{self, Secret};
use installer::state::{InstallState, Mode};
use installer::win::{self, BoxIcon, Buttons};
use installer::update::{self, Latest};
use installer::{payload, rollback, uninstall, util};

use logic::{Page, RunView, StepStatus};
use theme::{ACCENT, level_color, style};

/// Height kept for the footer (Close / Back / the primary button) below the page's scroll area.
const FOOTER_H: f32 = 64.0;
/// Width of the label column of the status rows.
const LABEL_W: f32 = 150.0;
/// A report longer than this shows its problems and an "All lines" section.
const REPORT_LINES: usize = 10;

/// Which page the window opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Wizard,
    Uninstall,
}

/// The window title: `installer::DISPLAY_NAME` + " - Setup" (a test keeps them in step).
pub const TITLE: &str = "Modern Arsenal for Elden Ring - Setup";

/// Development only (screenshots and a hands-off end-to-end run of the window; never set for players):
/// `MW2ER_GUI_DEV=page=<games|me3|seamless|install|done|uninstall>[,install][,close]` opens that page (running the
/// detection first), `install` presses Install once the Install page is ready (nothing blocks), `close` closes the
/// window when the install ended. Everything else (checks, guard, pins, options) is the same as by hand.
#[derive(Clone, Debug, Default, PartialEq)]
struct Dev {
    page: Option<Page>,
    install: bool,
    close: bool,
    started: bool,
}

impl Dev {
    fn parse(s: &str) -> Dev {
        let mut d = Dev::default();
        for p in s.split(',').map(str::trim) {
            match p {
                "install" => d.install = true,
                "close" => d.close = true,
                _ => {
                    d.page = match p.strip_prefix("page=") {
                        Some("games") => Some(Page::Games),
                        Some("me3") => Some(Page::Me3),
                        Some("seamless") => Some(Page::Seamless),
                        Some("install") => Some(Page::Install),
                        Some("done") => Some(Page::Done),
                        Some("uninstall") => Some(Page::Uninstall),
                        _ => d.page,
                    }
                }
            }
        }
        d
    }
}

/// Work for after the window closed: the uninstalled MW2inER.exe deletes itself once this process has exited.
#[derive(Default)]
struct AfterExit {
    self_delete: Option<(PathBuf, PathBuf, Guard)>,
}

/// Opens the window; returns the exit code (0 = the last install / uninstall passed or nothing was run).
pub fn run(a: Args, start: Start) -> i32 {
    let after = Arc::new(Mutex::new(AfterExit::default()));
    let code = Arc::new(Mutex::new(0i32));
    let icon = egui::IconData { rgba: crate::icon_art::rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([860.0, 660.0])
            .with_min_inner_size([760.0, 560.0])
            .with_icon(icon),
        ..Default::default()
    };
    let (a2, after2, code2) = (a.clone(), after.clone(), code.clone());
    let r = eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            style(&cc.egui_ctx);
            let mut app = App::new(a2, Env::from_system(), start, after2, code2);
            app.dev = std::env::var("MW2ER_GUI_DEV").map(|s| Dev::parse(&s)).unwrap_or_default();
            Ok(Box::new(app))
        }),
    );
    if let Some((exe, root, g)) = after.lock().ok().and_then(|mut x| x.self_delete.take()) {
        let _ = uninstall::schedule_self_delete(&exe, &root, &g);
    }
    match r {
        Ok(()) => code.lock().map(|c| *c).unwrap_or(1),
        Err(e) => fallback(&a, start, &e.to_string()),
    }
}

/// The window could not open (no OpenGL 2 driver, a remote session ...): say so and offer the console front end.
fn fallback(a: &Args, start: Start, err: &str) -> i32 {
    let text = format!(
        "The setup window could not open:\n{err}\n\nOK = continue in a console window (the same steps as text questions).\nCancel = stop.\n\nFrom a command prompt the same is: MW2inER-setup.exe --cli install [--mode coop]"
    );
    if !win::message_box(TITLE, &text, Buttons::OkCancel, BoxIcon::Error) {
        return 1;
    }
    win::alloc_console();
    let mut argv = vec!["--cli".to_string(), if start == Start::Uninstall { "uninstall" } else { "install" }.to_string()];
    for (k, v) in [("--root", &a.root), ("--payload", &a.payload)] {
        if let Some(v) = v {
            argv.extend([k.to_string(), v.display().to_string()]);
        }
    }
    if let Some(m) = a.mode.filter(|_| start == Start::Wizard) {
        argv.extend(["--mode".to_string(), if m.coop() { "coop" } else { "solo" }.to_string()]);
    }
    let code = cli::main_with(&argv, &mut std::io::stdout());
    eprint!("\nPress Enter to close this window.");
    let _ = std::io::stdin().read_line(&mut String::new());
    code
}

fn level_tag(l: Level) -> &'static str {
    match l {
        Level::Ok => " OK ",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    }
}

/// Everything the player can trigger; collected while drawing, applied after (so drawing only reads).
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Detect,
    Next,
    Back,
    Goto(Page),
    Pick(Input),
    PickPayload { zip: bool },
    DefaultRoot,
    Open(String),
    RunMe3Installer(PathBuf),
    AllowDownload(bool),
    /// P2: the Install page's Advanced box (build the regulation with WitchyBND instead of the built-in builder).
    SetWitchyFallback(bool),
    SetForce(bool),
    StartInstall,
    Cancel,
    Play(Mode),
    Verify,
    NewPassword,
    /// 2026-10-22 (user: the Welcome page's "Change co-op password" did nothing visible): write only the password into the
    /// installed copy's ersc_settings.ini, no reinstall
    SavePassword,
    Uninstall,
    /// P3: show the rollback confirmation (true) / hide it (false).
    AskRollback(bool),
    Rollback,
    /// P3: the opt-in GitHub release check (ticked = ask now).
    CheckUpdates(bool),
    Close,
}

/// The footer's primary button of a page.
struct Primary {
    label: String,
    enabled: bool,
    action: Action,
    /// Uninstall: drawn red.
    danger: bool,
}

enum RunMsg {
    Event(Progress),
    End(Result<(), String>),
}

struct Running {
    rx: Receiver<RunMsg>,
    cancel: Arc<AtomicBool>,
}

type Detected = Result<(Session, Option<Plan>), String>;

struct App {
    args: Args,
    env: Env,
    after: Arc<Mutex<AfterExit>>,
    code: Arc<Mutex<i32>>,
    page: Page,
    root: PathBuf,
    /// The release folder (a zip is staged by payload::resolve).
    payload: Result<PathBuf, String>,
    release: Option<Release>,
    installed: Option<InstallState>,
    mode: Mode,
    over: installer::detect::Overrides,
    note: Option<(Level, String)>,
    actions: Vec<Action>,
    // detection
    detecting: Option<Receiver<Detected>>,
    session: Option<Session>,
    plan: Option<Plan>,
    detect_error: Option<String>,
    // me3
    me3_job: Option<Receiver<String>>,
    // Seamless Co-op
    seamless_note: Option<Result<String, String>>,
    password: String,
    show_password: bool,
    keep_password: bool,
    /// The Welcome page's "Change co-op password" on an installed co-op copy: the Seamless page saves only the password.
    pw_only: bool,
    // install options
    allow_download: bool,
    /// P2 fallback: the regulation is built with WitchyBND (`--regulation-tool witchy` / the Advanced box).
    witchy_fallback: bool,
    backup: bool,
    desktop: bool,
    start_menu: bool,
    force: bool,
    // install run
    run: Option<Running>,
    view: RunView,
    close_after_run: bool,
    // reports (verify / play) on the Welcome and Done pages
    report: Option<(String, String)>,
    report_job: Option<(String, Receiver<String>)>,
    // uninstall
    delete_backups: bool,
    uninstall_done: Option<Result<Vec<String>, String>>,
    /// The Uninstall page found an install to remove (its footer button).
    uninstall_ready: bool,
    /// The footer's right group width last frame (Back + primary; laid out left to right for the Tab order).
    footer_right_w: f32,
    // P3: update / rollback / the release check
    /// What this run does as an update / repair (from the detection's plan).
    update: Option<update::Summary>,
    /// (installed version, the version a rollback restores) when the root keeps a rollback store.
    rollback: Option<(String, String)>,
    confirm_rollback: bool,
    check_updates: bool,
    update_check: Option<Result<Latest, String>>,
    update_job: Option<Receiver<Result<Latest, String>>>,
    dev: Dev,
}

impl App {
    fn new(args: Args, env: Env, start: Start, after: Arc<Mutex<AfterExit>>, code: Arc<Mutex<i32>>) -> App {
        let root = cli::resolve_root(&args, &env);
        let over = installer::detect::Overrides {
            er_game: args.er.clone(),
            mw2_dir: args.mw2.clone(),
            me3: args.me3.clone(),
            seamless: args.seamless.clone(),
            witchy_zip: args.witchy_zip.clone(),
            python: args.python.clone(),
        };
        let mut app = App {
            page: if start == Start::Uninstall { Page::Uninstall } else { Page::Welcome },
            root,
            payload: Err(String::new()),
            release: None,
            installed: None,
            mode: Mode::Solo,
            over,
            note: None,
            actions: Vec::new(),
            detecting: None,
            session: None,
            plan: None,
            detect_error: None,
            me3_job: None,
            seamless_note: None,
            password: String::new(),
            show_password: false,
            keep_password: true,
            pw_only: false,
            allow_download: args.download_witchy,
            witchy_fallback: args.regulation_tool == Some(Tool::Witchy),
            backup: !args.no_backup,
            desktop: !args.no_shortcuts,
            start_menu: !args.no_shortcuts && args.shortcut_dir.is_none(),
            force: args.force,
            run: None,
            view: RunView::default(),
            close_after_run: false,
            report: None,
            report_job: None,
            delete_backups: args.delete_backups,
            uninstall_done: None,
            uninstall_ready: false,
            footer_right_w: 0.0,
            update: None,
            rollback: None,
            confirm_rollback: false,
            check_updates: false,
            update_check: None,
            update_job: None,
            dev: Dev::default(),
            args,
            env,
            after,
            code,
        };
        app.load_root();
        app.mode = app.args.mode.or(app.installed.as_ref().map(|s| s.mode)).unwrap_or(Mode::Solo);
        app
    }

    /// (Re)reads the install record and the release files for the current root.
    fn load_root(&mut self) {
        self.installed = InstallState::load(&self.root);
        let p = match &self.args.payload {
            Some(p) => Ok(p.clone()),
            None => cli::resolve_payload(&self.args, &self.root),
        };
        self.payload = p.and_then(|p| payload::resolve(&p));
        self.release = self.payload.as_ref().ok().and_then(|p| Release::load(p).ok());
        if self.release.is_none() && self.payload.is_ok() {
            self.payload = Err(format!("no release files in {}", self.payload.as_ref().unwrap().display()));
        }
        self.session = None;
        self.plan = None;
        self.update = None;
        self.rollback = rollback::plan(&self.root).ok().map(|p| (p.from, p.to));
        self.confirm_rollback = false;
    }

    /// The Update card's data from the session and its plan.
    fn refresh_update(&mut self) {
        self.update = match (&self.session, &self.plan) {
            (Some(s), Some(p)) => s.update_summary(p),
            _ => None,
        };
    }

    /// The Welcome page's primary label: "Update 0.2.0 -> 0.2.1" over an older install, "Repair / change" over the
    /// same version, "Next" without an install.
    fn welcome_label(&self) -> String {
        match (&self.installed, &self.release) {
            (Some(st), Some(r)) if !st.release_version.is_empty() => {
                let from = if st.update_from.is_empty() { &st.release_version } else { &st.update_from };
                match update::kind(Some(from), &r.version) {
                    update::Kind::Update => format!("Update {from} -> {}", r.version),
                    update::Kind::Downgrade => format!("Install {} (older)", r.version),
                    _ => "Repair / change".to_string(),
                }
            }
            (Some(_), _) => "Repair / change".to_string(),
            _ => "Next".to_string(),
        }
    }

    fn shortcut_dirs(&self) -> ShortcutDirs {
        if let Some(d) = &self.args.shortcut_dir {
            return ShortcutDirs { desktop: self.desktop.then(|| d.clone()), start_menu: None };
        }
        ShortcutDirs {
            desktop: if self.desktop { self.env.desktop.clone() } else { None },
            start_menu: if self.start_menu { self.env.programs.clone() } else { None },
        }
    }

    fn request(&self) -> Request {
        Request {
            root: self.root.clone(),
            payload: self.payload.clone().unwrap_or_default(),
            mode: self.mode,
            over: self.over.clone(),
            allow_download: self.allow_download,
            regulation_tool: self.regulation_tool(),
            python_zip: self.args.python_zip.clone(),
            keep_dumps: self.args.keep_dumps,
            force: self.force,
            shortcuts: self.shortcut_dirs(),
            register: !self.args.no_register,
            self_exe: std::env::current_exe().ok(),
            save_root: if self.backup { self.env.save_root() } else { None },
            backup_dir: self.args.backup_dir.clone(),
            probe_python: true,
        }
    }

    /// The builder the player chose: the Advanced box (WitchyBND), else `--regulation-tool regbuild`, else None (the
    /// release's builder).
    fn regulation_tool(&self) -> Option<Tool> {
        if self.witchy_fallback { Some(Tool::Witchy) } else { self.args.regulation_tool.filter(|t| *t != Tool::Witchy) }
    }

    fn checks(&self) -> Vec<Check> {
        self.session.as_ref().map(|s| s.checks.clone()).unwrap_or_default()
    }

    fn busy(&self) -> bool {
        self.detecting.is_some() || self.run.is_some() || self.me3_job.is_some() || self.report_job.is_some() || self.update_job.is_some()
    }

    /// P3: puts the version from before the last update back (refused while the game runs).
    fn do_rollback(&mut self) {
        self.confirm_rollback = false;
        if let Some(c) = run::running_game_check(win::is_running) {
            self.note = Some((Level::Fail, c.msg));
            return;
        }
        let p = match rollback::plan(&self.root) {
            Ok(p) => p,
            Err(e) => {
                self.note = Some((Level::Fail, e));
                return;
            }
        };
        let st = InstallState::load(&self.root).unwrap_or_default();
        let g = cli::state_guard(&self.env, &st);
        let me = std::env::current_exe().ok();
        match rollback::execute(&p, &g, me.as_deref()) {
            Ok(_) => {
                if !self.args.no_register && !st.registry.is_empty() {
                    use installer::win::RegValue::Str;
                    let _ = win::reg_set_hkcu(run::APP_KEY, &[("Version", Str(&p.to))]);
                    let _ = win::reg_set_hkcu(run::UNINSTALL_KEY, &[("DisplayVersion", Str(&p.to))]);
                }
                let back = InstallState::load(&self.root).map(|s| s.coop_id).unwrap_or_default();
                let note = if back != st.coop_id { format!(" The co-op id is {back} again: both players must use {}.", p.to) } else { String::new() };
                self.note = Some((Level::Ok, format!("Rolled back to {}: its files, settings and record are back.{note}", p.to)));
            }
            Err(e) => self.note = Some((Level::Fail, format!("Roll back failed: {e}"))),
        }
        self.load_root();
    }

    /// P3: the opt-in GitHub release check on a worker thread (only the release's `update_repo` on api.github.com).
    fn check_updates(&mut self, ctx: &egui::Context) {
        let Some(r) = self.release.as_ref().filter(|r| !r.update_repo.is_empty()) else { return };
        let repo = r.update_repo.clone();
        let current = self.installed.as_ref().map(|s| s.release_version.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| r.version.clone());
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check_latest(&repo, &current));
            ctx.request_repaint();
        });
        self.update_check = None;
        self.update_job = Some(rx);
    }

    // ---- background work ----------------------------------------------------------------------------------------

    fn detect(&mut self, ctx: &egui::Context) {
        if self.payload.is_err() || self.run.is_some() {
            return;
        }
        let req = self.request();
        let env = self.env.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = Session::prepare(env, req).map(|s| {
                let p = s.plan();
                (s, p)
            });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.detecting = Some(rx);
        self.detect_error = None;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.detecting {
            match rx.try_recv() {
                Ok(Ok((s, p))) => {
                    self.keep_password = self.keep_password && s.facts.seamless.copy_ini.as_ref().is_some_and(|i| i.password_set);
                    self.allow_download = s.req.allow_download;
                    self.session = Some(s);
                    self.plan = p;
                    self.detecting = None;
                    self.refresh_update();
                }
                Ok(Err(e)) => {
                    self.detect_error = Some(e);
                    self.session = None;
                    self.detecting = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.detect_error = Some("the detection stopped unexpectedly".into());
                    self.detecting = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.me3_job {
            match rx.try_recv() {
                Ok(msg) => {
                    self.note = Some((Level::Ok, msg));
                    self.me3_job = None;
                    self.detect(ctx);
                }
                Err(TryRecvError::Disconnected) => self.me3_job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.update_job {
            match rx.try_recv() {
                Ok(r) => {
                    self.update_check = Some(r);
                    self.update_job = None;
                }
                Err(TryRecvError::Disconnected) => self.update_job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some((title, rx)) = &self.report_job {
            match rx.try_recv() {
                Ok(text) => {
                    self.report = Some((title.clone(), text));
                    self.report_job = None;
                }
                Err(TryRecvError::Disconnected) => self.report_job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut ended = None;
        if let Some(r) = &self.run {
            loop {
                match r.rx.try_recv() {
                    Ok(RunMsg::Event(e)) => self.view.apply(&e),
                    Ok(RunMsg::End(res)) => {
                        ended = Some(res);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        ended = Some(Err("the install stopped unexpectedly; run the setup again to continue (finished steps are kept)".into()));
                        break;
                    }
                }
            }
        }
        if let Some(res) = ended {
            self.run = None;
            if let Err(e) = &res
                && self.view.finished.is_none()
            {
                self.view.finished = Some((false, e.clone()));
            }
            if let Ok(mut c) = self.code.lock() {
                *c = i32::from(res.is_err());
            }
            self.installed = InstallState::load(&self.root);
            self.rollback = rollback::plan(&self.root).ok().map(|p| (p.from, p.to));
            if res.is_ok() {
                self.page = Page::Done;
            }
            if self.close_after_run {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.session.take() else { return };
        s.req.force = self.force;
        s.req.save_root = if self.backup { self.env.save_root() } else { None };
        s.req.shortcuts = self.shortcut_dirs();
        s.set_allow_download(self.allow_download);
        // live: the game may have been started since the detection (Session::install checks again too)
        s.refresh_running();
        if let Some(c) = s.checks.iter().find(|c| c.id == "er.running") {
            self.note = Some((Level::Fail, c.msg.clone()));
            self.session = Some(s);
            return;
        }
        if !s.blocking().is_empty() {
            self.note = Some((Level::Fail, "fix the red rows first".into()));
            self.session = Some(s);
            return;
        }
        let need_pw = s.req.mode.coop() && !(self.keep_password && s.facts.seamless.copy_ini.as_ref().is_some_and(|i| i.password_set));
        let pw = if need_pw {
            match Secret::new(std::mem::take(&mut self.password)) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.note = Some((Level::Fail, format!("{e}: go back to the Seamless Co-op page")));
                    self.session = Some(s);
                    return;
                }
            }
        } else {
            None
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (c2, ctx2) = (cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let ctx3 = ctx2.clone();
            let r = s.install(pw.as_ref(), &c2, &mut |e| {
                let _ = tx2.send(RunMsg::Event(e));
                ctx3.request_repaint();
            });
            drop(pw);
            let _ = tx.send(RunMsg::End(r.map_err(|e| e.to_string())));
            ctx2.request_repaint();
        });
        self.view = RunView::default();
        self.run = Some(Running { rx, cancel });
        self.note = None;
    }

    fn run_me3_installer(&mut self, path: PathBuf, ctx: &egui::Context) {
        let pin = self.release.as_ref().map(|r| r.pins.me3_installer_pin().to_string()).unwrap_or_else(|| release::ME3_INSTALLER_SHA256.into());
        // hashed again right before it runs
        match util::sha256_file(&path) {
            Ok(h) if h.eq_ignore_ascii_case(&pin) => {}
            _ => {
                self.note = Some((Level::Fail, format!("{} does not match the official me3 0.13.0 installer (sha256): not run", path.display())));
                return;
            }
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match std::process::Command::new(&path).spawn().and_then(|mut c| c.wait()) {
                Ok(st) if st.success() => "The me3 installer finished; checking again".to_string(),
                Ok(st) => format!("The me3 installer ended with {st}; checking again"),
                Err(e) => format!("cannot start the me3 installer: {e}"),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        self.me3_job = Some(rx);
    }

    fn verify(&mut self, ctx: &egui::Context) {
        let root = self.root.display().to_string();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            cli::main_with(&["verify".to_string(), "--root".to_string(), root], &mut out);
            let _ = tx.send(String::from_utf8_lossy(&out).into_owned());
            ctx.request_repaint();
        });
        self.report_job = Some(("Verify".into(), rx));
    }

    fn play(&mut self, mode: Mode) {
        let title = format!("Play {}", if mode.coop() { "co-op" } else { "solo" });
        let p = match cli::play_prepare(&self.env, &self.root, mode) {
            Ok(p) => p,
            Err(e) => {
                self.report = Some((title, format!("FAIL {e}")));
                return;
            }
        };
        let mut text = String::new();
        for r in p.rows.iter().filter(|r| r.level != Level::Ok) {
            text.push_str(&format!("{} {}\n", level_tag(r.level), r.msg));
        }
        if !p.can_start() {
            text.push_str("\nNot started: fix the FAIL rows above (Steam must be running; this program never starts it).");
            self.report = Some((title, text));
            return;
        }
        match cli::play_start(&self.env, &self.root, &p) {
            Ok(lines) => {
                for l in lines {
                    text.push_str(&format!("OK   {l}\n"));
                }
                text.push_str("\nme3 is starting Elden Ring; you can close this window.");
            }
            Err(e) => text.push_str(&format!("FAIL {e}\n")),
        }
        self.report = Some((title, text));
    }

    fn uninstall(&mut self) {
        if win::is_running("eldenring.exe") {
            self.note = Some((Level::Fail, "Elden Ring is running: close it first".into()));
            return;
        }
        let me = std::env::current_exe().ok();
        let mut p = match uninstall::plan(&self.root, self.delete_backups, me.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                self.uninstall_done = Some(Err(e));
                return;
            }
        };
        let st = InstallState::load(&self.root).unwrap_or_default();
        let g = cli::state_guard(&self.env, &st);
        let self_exe = p.self_in_root.take();
        let r = uninstall::execute(&p, &g, true).map(|mut lines| {
            if let Some(exe) = self_exe {
                lines.push(format!("{} is deleted when this window closes", exe.display()));
                if let Ok(mut a) = self.after.lock() {
                    a.self_delete = Some((exe, self.root.clone(), g.clone()));
                }
            }
            lines
        });
        if let Ok(mut c) = self.code.lock() {
            *c = i32::from(r.is_err());
        }
        self.uninstall_done = Some(r);
        self.installed = InstallState::load(&self.root);
    }

    fn pick(&mut self, input: Input, ctx: &egui::Context) {
        let steam = self.env.steam_path.clone();
        let picked = match input {
            Input::ErGame => win::pick("Choose the ELDEN RING folder (or its Game folder)", true, &[], steam.as_deref()).map(|p| logic::er_game_choice(&p)),
            Input::Mw2Dir => win::pick("Choose the Call of Duty Modern Warfare 2 folder (with iw4sp.exe)", true, &[], steam.as_deref()),
            Input::Me3 => win::pick("Choose me3.exe", false, &[("me3.exe", "me3.exe"), ("Programs", "*.exe")], None),
            Input::Python => win::pick("Choose python.exe (3.11 or newer)", false, &[("python.exe", "python.exe")], None),
            Input::Seamless => win::pick(
                "Choose the Seamless Co-op zip you downloaded, or SeamlessCoop\\ersc.dll",
                false,
                &[("Seamless Co-op (zip or ersc.dll)", "*.zip;ersc.dll")],
                self.env.downloads.as_deref(),
            ),
            Input::WitchyZip => win::pick("Choose WitchyBND-3.0.1.0-win-x64.zip", false, &[("WitchyBND zip", "*.zip")], self.env.downloads.as_deref()),
            Input::Root => win::pick("Choose the install folder", true, &[], self.root.parent()).map(|p| logic::root_choice(&p)),
        };
        let Some(p) = picked else { return };
        match input {
            Input::ErGame => self.over.er_game = Some(p),
            Input::Mw2Dir => self.over.mw2_dir = Some(p),
            Input::Me3 => self.over.me3 = Some(p),
            Input::Python => self.over.python = Some(p),
            Input::Seamless => {
                let (pin, ver) = self.release.as_ref().map(|r| (r.pins.ersc_sha256.clone(), r.pins.seamless_version.clone())).unwrap_or_default();
                self.seamless_note = Some(logic::seamless_pick_note(&p, &pin, &ver));
                self.over.seamless = Some(p);
            }
            Input::WitchyZip => self.over.witchy_zip = Some(p),
            Input::Root => {
                self.root = p;
                self.load_root();
                if self.page == Page::Welcome {
                    return;
                }
            }
        }
        self.detect(ctx);
    }

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::Detect => self.detect(ctx),
            Action::Next => {
                let coop = self.mode.coop();
                if self.page == Page::Welcome {
                    self.detect(ctx);
                }
                self.page = logic::next_page(self.page, coop);
                self.note = None;
            }
            Action::Back => {
                self.page = logic::prev_page(self.page, self.mode.coop());
                self.note = None;
            }
            Action::Goto(p) => {
                self.page = p;
                self.note = None;
                self.uninstall_done = None;
            }
            Action::Pick(i) => self.pick(i, ctx),
            Action::PickPayload { zip } => {
                let p = if zip {
                    win::pick("Choose the release zip (MW2inER-...zip)", false, &[("Release zip", "*.zip")], self.env.downloads.as_deref())
                } else {
                    win::pick("Choose the unzipped release folder", true, &[], self.env.downloads.as_deref())
                };
                if let Some(p) = p {
                    self.args.payload = Some(p);
                    self.load_root();
                }
            }
            Action::DefaultRoot => {
                if let Some(d) = self.env.default_root() {
                    self.root = d;
                    self.load_root();
                }
            }
            Action::Open(t) => {
                if let Err(e) = win::shell_open(&t) {
                    self.note = Some((Level::Fail, e.to_string()));
                }
            }
            Action::RunMe3Installer(p) => self.run_me3_installer(p, ctx),
            Action::AllowDownload(b) => {
                self.allow_download = b;
                if let Some(s) = self.session.as_mut() {
                    s.set_allow_download(b);
                }
            }
            Action::SetWitchyFallback(b) => {
                self.witchy_fallback = b;
                let tool = self.regulation_tool();
                if let Some(s) = self.session.as_mut() {
                    s.set_regulation_tool(tool);
                    self.plan = s.plan();
                }
                self.refresh_update();
            }
            Action::SetForce(b) => {
                self.force = b;
                if let Some(s) = self.session.as_mut() {
                    s.req.force = b;
                    self.plan = s.plan();
                }
                self.refresh_update();
            }
            Action::StartInstall => self.start_install(ctx),
            Action::Cancel => {
                if let Some(r) = &self.run {
                    r.cancel.store(true, Ordering::SeqCst);
                }
            }
            Action::Play(m) => self.play(m),
            Action::Verify => self.verify(ctx),
            Action::NewPassword => {
                self.mode = Mode::Coop;
                self.keep_password = false;
                // an installed co-op copy: straight to the password field (Save writes it, no reinstall); otherwise the
                // pages add co-op and Install writes it
                let installed_coop = InstallState::load(&self.root).is_some_and(|st| st.mode.coop());
                self.pw_only = installed_coop;
                self.note = Some((Level::Ok, if installed_coop {
                    "Type the new co-op password and click Save password".into()
                } else {
                    "Co-op chosen: Next goes through the pages; the Seamless Co-op page asks for the password and Install writes it (unchanged build steps are skipped)".into()
                }));
                if installed_coop {
                    self.page = Page::Seamless;
                }
            }
            Action::SavePassword => self.save_password_only(),
            Action::Uninstall => self.uninstall(),
            Action::AskRollback(b) => self.confirm_rollback = b,
            Action::Rollback => self.do_rollback(),
            Action::CheckUpdates(b) => {
                self.check_updates = b;
                if b {
                    self.check_updates(ctx);
                } else {
                    self.update_check = None;
                }
            }
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    // ---- drawing ------------------------------------------------------------------------------------------------
    //
    // 2026-10-22 redesign (the user: "less wordy, less in your face"): one title + one sentence per page, compact
    // status rows (dot, label, value; the full message is the row's tooltip; yellow / red rows keep a short hint and
    // their fix button), cards, the step indicator, the primary action bottom-right. Behaviour is unchanged: the
    // same actions, checks and guards as before; long texts moved into tooltips and "More info".

    fn draw(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let bg = ui.visuals().panel_fill;
            ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
            egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 12, bottom: 4 }).show(ui, |ui| self.header(ui));
            ui.add(egui::Separator::default().spacing(0.0));
            let h = (ui.available_height() - FOOTER_H).max(60.0);
            egui::ScrollArea::vertical().id_salt("page").max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 14, bottom: 14 }).show(ui, |ui| {
                    self.note_banner(ui);
                    match self.page {
                        Page::Welcome => self.page_welcome(ui),
                        Page::Games => self.page_games(ui),
                        Page::Me3 => self.page_me3(ui),
                        Page::Seamless => self.page_seamless(ui),
                        Page::Install => self.page_install(ui),
                        Page::Done => self.page_done(ui),
                        Page::Uninstall => self.page_uninstall(ui),
                    }
                });
            });
            ui.add(egui::Separator::default().spacing(0.0));
            // drawn after the page, so Tab goes through the page first, then Back / Next
            egui::Frame::new().inner_margin(Margin::symmetric(24, 10)).show(ui, |ui| self.footer(ui));
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(installer::DISPLAY_NAME).size(17.0).strong().color(ACCENT));
            ui.label(RichText::new(format!("setup {}", env!("CARGO_PKG_VERSION"))).small().weak());
        });
        if self.page != Page::Uninstall {
            let pages = logic::wizard_pages(self.mode.coop());
            let names: Vec<&str> = pages.iter().map(|p| logic::indicator_title(*p)).collect();
            let cur = pages.iter().position(|p| *p == self.page).unwrap_or(0);
            ui.add_space(2.0);
            theme::steps(ui, &names, cur);
        } else {
            ui.add_space(4.0);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let running = self.run.is_some();
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            if theme::secondary(ui, "Close", !running).on_disabled_hover_text("Cancel the install first").clicked() {
                self.actions.push(Action::Close);
            }
            if self.busy() {
                ui.spinner();
            }
            if let Some((l, h)) = self.footer_hint() {
                ui.label(RichText::new(h).small().color(level_color(l)));
            }
            // right group (Back, primary) in Tab order: its width is measured and kept for the next frame
            let gap = ui.spacing().item_spacing.x;
            let space = ui.available_width() - self.footer_right_w - gap;
            if space > 0.0 {
                ui.add_space(space);
            }
            let mut used: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| used = Some(used.map_or(r, |u| u.union(r)));
            if let Some((label, a)) = self.back_action() {
                let r = theme::secondary(ui, label, true);
                grow(r.rect);
                if r.clicked() {
                    if self.pw_only && self.page == Page::Seamless {
                        self.pw_only = false;
                    }
                    self.actions.push(a);
                }
            }
            if let Some(p) = self.primary() {
                let r = if p.danger { theme::danger(ui, &p.label, p.enabled) } else { theme::primary(ui, &p.label, p.enabled) };
                grow(r.rect);
                if r.clicked() {
                    self.actions.push(p.action);
                }
            }
            let w = used.map_or(0.0, |r| r.width());
            if (w - self.footer_right_w).abs() > 0.5 {
                self.footer_right_w = w;
                ui.ctx().request_discard("footer width");
            }
        });
    }

    /// A short reason next to a disabled primary button.
    fn footer_hint(&self) -> Option<(Level, &'static str)> {
        let blocked = |p: Page| self.detecting.is_none() && self.session.as_ref().is_some_and(|s| logic::page_blocked(&s.checks, p));
        match self.page {
            Page::Games | Page::Me3 if blocked(self.page) => Some((Level::Fail, "Fix the red rows first")),
            Page::Seamless if blocked(Page::Seamless) => Some((Level::Fail, "Fix the red rows first")),
            Page::Seamless if self.password.trim().is_empty() && !(self.keep_password && self.password_set()) && self.session.is_some() => {
                Some((Level::Warn, "Type the co-op password"))
            }
            Page::Install if self.run.is_none() && self.view.finished.is_none() && self.session.as_ref().is_some_and(|s| !s.blocking().is_empty()) => {
                Some((Level::Fail, "Not ready: see the red rows"))
            }
            _ => None,
        }
    }

    /// The footer's secondary button left of the primary one: (label, action).
    fn back_action(&self) -> Option<(&'static str, Action)> {
        match self.page {
            Page::Welcome | Page::Done => None,
            Page::Seamless if self.pw_only => Some(("Back", Action::Goto(Page::Welcome))),
            Page::Uninstall if self.uninstall_done.is_none() => Some(("Back", Action::Goto(Page::Welcome))),
            Page::Uninstall => None,
            _ if self.run.is_some() => None,
            _ => Some(("Back", Action::Back)),
        }
    }

    /// The page's primary button (bottom-right).
    fn primary(&self) -> Option<Primary> {
        let ok_page = |p: Page| self.detecting.is_none() && self.session.as_ref().is_some_and(|s| !logic::page_blocked(&s.checks, p));
        let next = |enabled: bool| Some(Primary { label: "Next".into(), enabled, action: Action::Next, danger: false });
        match self.page {
            Page::Welcome => {
                Some(Primary { label: self.welcome_label(), enabled: self.payload.is_ok() && self.run.is_none(), action: Action::Next, danger: false })
            }
            Page::Games | Page::Me3 => next(ok_page(self.page)),
            Page::Seamless if self.pw_only => Some(Primary {
                label: "Save password".into(),
                enabled: !self.password.trim().is_empty() && !self.busy(),
                action: Action::SavePassword,
                danger: false,
            }),
            Page::Seamless => {
                let pw_ok = !self.password.trim().is_empty() || (self.keep_password && self.password_set());
                next(ok_page(Page::Seamless) && pw_ok)
            }
            Page::Install if self.run.is_none() && self.view.finished.is_none() => {
                let ready = self.detecting.is_none() && self.session.as_ref().is_some_and(|s| s.blocking().is_empty()) && self.plan.is_some();
                let label = if self.update.as_ref().is_some_and(|u| u.kind == update::Kind::Update) { "Update" } else { "Install" };
                Some(Primary { label: label.into(), enabled: ready, action: Action::StartInstall, danger: false })
            }
            Page::Uninstall if self.uninstall_done.is_none() && self.uninstall_ready => {
                Some(Primary { label: "Uninstall".into(), enabled: !self.busy(), action: Action::Uninstall, danger: true })
            }
            _ => None,
        }
    }

    /// Writes only the co-op password (and the two fixed keys) into the installed copy's ersc_settings.ini.
    fn save_password_only(&mut self) {
        let ini = self.root.join("seamless").join("SeamlessCoop").join(seamless::INI);
        let st = InstallState::load(&self.root).unwrap_or_default();
        let g = cli::state_guard(&self.env, &st);
        let secret = match seamless::Secret::new(std::mem::take(&mut self.password)) {
            Ok(s) => s,
            Err(e) => {
                self.note = Some((Level::Fail, format!("password not saved: {e}")));
                return;
            }
        };
        match seamless::set_ini_keys(&ini, Some(&secret), &g) {
            Ok(()) => {
                self.pw_only = false;
                self.keep_password = true;
                self.page = Page::Welcome;
                self.note = Some((Level::Ok, "Co-op password saved (the other player must use the same one)".into()));
                self.actions.push(Action::Detect);
            }
            Err(e) => self.note = Some((Level::Fail, format!("could not save the password: {e}"))),
        }
    }

    fn password_set(&self) -> bool {
        self.session.as_ref().and_then(|s| s.facts.seamless.copy_ini.as_ref()).is_some_and(|i| i.password_set)
    }

    fn note_banner(&mut self, ui: &mut egui::Ui) {
        let Some((l, m)) = self.note.clone() else { return };
        let mut close = false;
        theme::tinted_card(ui, l, |ui| {
            ui.horizontal(|ui| {
                theme::dot(ui, level_color(l));
                let w = (ui.available_width() - 34.0).max(50.0);
                ui.allocate_ui_with_layout(egui::vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.add(egui::Label::new(m).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.small_button("x").on_hover_text("Dismiss").clicked();
                });
            });
        });
        if close {
            self.note = None;
        }
        ui.add_space(8.0);
    }

    /// One compact status row: dot, label, value, and on the right an optional widget (a fix button).
    fn status_row(ui: &mut egui::Ui, level: Level, label: &str, value: &str, tip: &str, right: impl FnOnce(&mut egui::Ui)) -> Response {
        let r = ui
            .horizontal(|ui| {
                theme::dot(ui, level_color(level));
                ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(LABEL_W);
                    ui.label(RichText::new(label).weak());
                });
                let v = RichText::new(value).strong();
                ui.add(egui::Label::new(if level == Level::Ok { v } else { v.color(level_color(level)) }).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            })
            .response;
        if tip.is_empty() { r } else { r.on_hover_text(tip) }
    }

    /// The short hint under a yellow / red row.
    fn hint_line(ui: &mut egui::Ui, level: Level, hint: &str) {
        ui.horizontal(|ui| {
            ui.add_space(18.0 + LABEL_W);
            ui.add(egui::Label::new(RichText::new(hint).small().color(level_color(level))).wrap());
        });
    }

    /// One check row with its fix button (yellow / red rows only) and its hint.
    fn row(&mut self, ui: &mut egui::Ui, c: &Check) {
        let t = logic::row_text(c);
        let mut tip = c.msg.clone();
        match &c.fix {
            Some(Fix::VerifyInSteam(_)) if c.level != Level::Ok => tip.push_str("\n\nShow in Steam, then Properties > Installed Files > Verify integrity of game files, and Check again."),
            Some(Fix::UpdateInSteam(_)) if c.level != Level::Ok => tip.push_str("\n\nShow in Steam, let Steam update it, then Check again."),
            _ => {}
        }
        let fix = c.fix.clone().filter(|_| c.level != Level::Ok);
        Self::status_row(ui, c.level, t.label, &t.value, &tip, |ui| {
            if let Some(f) = fix {
                self.fix_button(ui, f);
            }
        });
        if c.level != Level::Ok
            && let Some(h) = &t.hint
        {
            ui.add_space(-5.0);
            Self::hint_line(ui, c.level, h);
        }
    }

    fn fix_button(&mut self, ui: &mut egui::Ui, f: Fix) {
        match f {
            Fix::OpenUrl(u) => {
                let label = if u.contains("nexusmods") {
                    "Open Nexus Mods"
                } else if u.contains("me3") {
                    "me3 release page"
                } else if u.contains("vc_redist") {
                    "Get the VC++ runtime"
                } else {
                    "Open the page"
                };
                if theme::secondary(ui, label, true).on_hover_text(&u).clicked() {
                    self.actions.push(Action::Open(u));
                }
            }
            Fix::VerifyInSteam(id) | Fix::UpdateInSteam(id) => {
                if theme::secondary(ui, "Show in Steam", true).on_hover_text("Opens the game's page in your Steam library (Steam is not started by this setup)").clicked() {
                    self.actions.push(Action::Open(format!("steam://nav/games/details/{id}")));
                }
            }
            Fix::Browse(i) => {
                if theme::secondary(ui, "Choose...", true).clicked() {
                    self.actions.push(Action::Pick(i));
                }
            }
            Fix::RunMe3Installer(p) => {
                if theme::secondary(ui, "Install me3 0.13.0", self.me3_job.is_none())
                    .on_hover_text("Runs the official, unmodified me3 0.13.0 installer included in this release (sha256-checked right before it runs)")
                    .clicked()
                {
                    self.actions.push(Action::RunMe3Installer(p));
                }
            }
            Fix::DownloadWitchy => {
                // the download itself is the checkbox under the row
                if theme::secondary(ui, "Choose the zip...", true).clicked() {
                    self.actions.push(Action::Pick(Input::WitchyZip));
                }
            }
        }
    }

    fn rows(&mut self, ui: &mut egui::Ui, rows: &[Check]) {
        for c in rows {
            self.row(ui, c);
        }
    }

    /// The detection state above a page that needs it (spinner, error, or nothing).
    fn detection_state(&mut self, ui: &mut egui::Ui) -> bool {
        if self.detecting.is_some() {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking your games and tools (read-only) ...");
                });
            });
            return false;
        }
        if let Some(e) = self.detect_error.clone() {
            theme::tinted_card(ui, Level::Fail, |ui| {
                ui.add(egui::Label::new(e).wrap());
                if theme::secondary(ui, "Check again", true).clicked() {
                    self.actions.push(Action::Detect);
                }
            });
            return false;
        }
        if self.session.is_none() {
            if theme::secondary(ui, "Check", true).clicked() {
                self.actions.push(Action::Detect);
            }
            return false;
        }
        true
    }

    /// A row of small secondary buttons under a page's rows.
    fn tools_row(&mut self, ui: &mut egui::Ui, buttons: &[(&str, Action)]) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (label, a) in buttons {
                if theme::secondary(ui, label, true).clicked() {
                    self.actions.push(a.clone());
                }
            }
        });
    }

    /// The Verify / Play result as a dismissible card.
    fn report_card(&mut self, ui: &mut egui::Ui) {
        if let Some((title, _)) = &self.report_job {
            ui.add_space(8.0);
            let title = title.clone();
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("{title} ..."));
                });
            });
        }
        let Some((title, text)) = self.report.clone() else { return };
        let lines = logic::report_lines(&text);
        let level = logic::report_level(&lines).unwrap_or(Level::Ok);
        let mut close = false;
        ui.add_space(8.0);
        theme::tinted_card(ui, level, |ui| {
            ui.horizontal(|ui| {
                theme::dot(ui, level_color(level));
                ui.label(RichText::new(&title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.small_button("x").on_hover_text("Dismiss").clicked();
                });
            });
            let line = |ui: &mut egui::Ui, l: &Option<Level>, t: &str| {
                ui.horizontal(|ui| {
                    match l {
                        Some(l) => {
                            theme::dot(ui, level_color(*l));
                        }
                        None => ui.add_space(18.0),
                    }
                    ui.add(egui::Label::new(RichText::new(t).small()).wrap());
                });
            };
            // a long report: its problems and last line here, everything under "All lines"
            if lines.len() <= REPORT_LINES {
                for (l, t) in &lines {
                    line(ui, l, t);
                }
            } else {
                let mut shown: Vec<&(Option<Level>, String)> = lines.iter().filter(|(l, _)| matches!(l, Some(Level::Warn | Level::Fail))).take(REPORT_LINES - 1).collect();
                shown.extend(lines.last());
                for (l, t) in shown {
                    line(ui, l, t);
                }
                egui::CollapsingHeader::new(RichText::new(format!("All {} lines", lines.len())).small()).id_salt("report_all").default_open(false).show(ui, |ui| {
                    for (l, t) in &lines {
                        line(ui, l, t);
                    }
                });
            }
        });
        if close {
            self.report = None;
        }
    }

    fn page_welcome(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "Welcome", &format!("Installs {} for this Windows user, built from your own copies of Elden Ring and MW2 (2009).", installer::DISPLAY_NAME));
        theme::section(ui, "Setup");
        theme::card(ui, |ui| {
            let root = self.root.display().to_string();
            Self::status_row(ui, Level::Ok, "Install folder", &root, &root, |ui| {
                let default = self.env.default_root();
                if default.as_ref().is_some_and(|d| !util::same_path(d, &self.root)) && theme::secondary(ui, "Default", true).clicked() {
                    self.actions.push(Action::DefaultRoot);
                }
                if theme::secondary(ui, "Change...", self.run.is_none()).clicked() {
                    self.actions.push(Action::Pick(Input::Root));
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(LABEL_W);
                    ui.label(RichText::new("Mode").weak());
                });
                ui.selectable_value(&mut self.mode, Mode::Solo, "Solo");
                ui.selectable_value(&mut self.mode, Mode::Coop, "Solo + co-op");
                theme::info(ui, "Solo + co-op adds Seamless Co-op. You download Seamless Co-op yourself from Nexus Mods; the next pages show where.");
            });
            match (&self.payload, &self.release) {
                (Ok(p), Some(r)) => {
                    let v = format!("{}   co-op id {}", r.version, r.coop_id());
                    Self::status_row(ui, Level::Ok, "Release", &v, &format!("Release files: {}", p.display()), |_| {});
                }
                (Err(e), _) => {
                    let why = if e.is_empty() { "No release files were found next to this program." } else { e.as_str() };
                    Self::status_row(ui, Level::Fail, "Release", "not found", why, |_| {});
                    Self::hint_line(ui, Level::Fail, "Choose the unzipped release folder or the release zip");
                    ui.horizontal(|ui| {
                        ui.add_space(18.0 + LABEL_W);
                        if theme::secondary(ui, "Choose the folder...", true).clicked() {
                            self.actions.push(Action::PickPayload { zip: false });
                        }
                        if theme::secondary(ui, "Choose the zip...", true).clicked() {
                            self.actions.push(Action::PickPayload { zip: true });
                        }
                    });
                }
                _ => {}
            }
        });
        if let Some(st) = self.installed.clone() {
            theme::section(ui, "Installed here");
            theme::card(ui, |ui| {
                let pass = st.last_result.starts_with("PASS");
                ui.horizontal_wrapped(|ui| {
                    theme::dot(ui, if st.last_result.is_empty() || pass { theme::GREEN } else { theme::AMBER });
                    ui.label(RichText::new(format!("Version {}", if st.release_version.is_empty() { "?" } else { &st.release_version })).strong());
                    theme::chip(ui, if st.mode.coop() { "solo + co-op" } else { "solo" }, theme::MUTED);
                    if !st.coop_id.is_empty() {
                        theme::chip(ui, &format!("id {}", st.coop_id), theme::MUTED).on_hover_text("The co-op id: equal ids = byte-identical installs");
                    }
                    if !st.last_result.is_empty() {
                        theme::chip(ui, if pass { "last run passed" } else { "last run had problems" }, if pass { theme::GREEN } else { theme::AMBER })
                            .on_hover_text(format!("Last run: {}", st.last_result));
                    }
                });
                ui.add_space(4.0);
                let idle = !self.busy();
                ui.horizontal_wrapped(|ui| {
                    if st.mode.coop() && ui.add_enabled(idle, egui::Button::new(RichText::new("Play co-op").strong())).clicked() {
                        self.actions.push(Action::Play(Mode::Coop));
                    }
                    if ui.add_enabled(idle, egui::Button::new(RichText::new("Play solo").strong())).clicked() {
                        self.actions.push(Action::Play(Mode::Solo));
                    }
                    ui.add_space(6.0);
                    if theme::secondary(ui, "Verify files", idle).on_hover_text("Re-hashes every installed file against the release").clicked() {
                        self.actions.push(Action::Verify);
                    }
                    let pw_label = if st.mode.coop() { "Change co-op password" } else { "Add co-op" };
                    if theme::secondary(ui, pw_label, idle).clicked() {
                        self.actions.push(Action::NewPassword);
                    }
                    if theme::secondary(ui, "Open the folder", true).clicked() {
                        self.actions.push(Action::Open(self.root.display().to_string()));
                    }
                    if theme::secondary(ui, "Uninstall...", idle).clicked() {
                        self.actions.push(Action::Goto(Page::Uninstall));
                    }
                    if let Some((_, to)) = self.rollback.clone()
                        && theme::secondary(ui, &format!("Roll back to {to}"), idle && !self.confirm_rollback)
                            .on_hover_text("Puts the version from before the last update back: its files, its mw2er.toml and its install record")
                            .clicked()
                    {
                        self.actions.push(Action::AskRollback(true));
                    }
                });
                ui.label(RichText::new("Repair or update with the button below; unchanged steps are skipped.").small().weak());
            });
            if self.confirm_rollback
                && let Some((from, to)) = self.rollback.clone()
            {
                ui.add_space(6.0);
                theme::tinted_card(ui, Level::Warn, |ui| {
                    ui.horizontal(|ui| {
                        theme::dot(ui, theme::AMBER);
                        ui.label(RichText::new(format!("Roll back from {from} to {to}?")).strong());
                    });
                    ui.add(egui::Label::new(RichText::new(format!("The files {from} replaced, your mw2er.toml of {to} and its record come back; nothing is rebuilt. If the co-op id changes, both players must use the same version.")).small().weak()).wrap());
                    ui.horizontal(|ui| {
                        if theme::primary(ui, "Roll back", !self.busy()).clicked() {
                            self.actions.push(Action::Rollback);
                        }
                        if theme::secondary(ui, &format!("Keep {from}"), true).clicked() {
                            self.actions.push(Action::AskRollback(false));
                        }
                    });
                });
            }
        }
        self.update_check_card(ui);
        self.report_card(ui);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            theme::dot(ui, theme::MUTED);
            ui.label(RichText::new("Offline / private play only. Your game folders are never changed; your saves are backed up first.").small().weak());
        });
        egui::CollapsingHeader::new(RichText::new("More info").small()).id_salt("more").default_open(false).show(ui, |ui| {
            for l in [
                "1. Checks Steam, Elden Ring 2.7.2.0, MW2 (2009), me3 and the free disk space (read-only).",
                "2. Copies the mod into the install folder and builds its data from your games: about 5-20 minutes, about 6 GB free space while it runs, about 0.5 GB after.",
                "3. Verifies every file against the release (co-op needs byte-identical installs) and makes the shortcuts.",
                "Never official online mode, and never ersc_launcher.exe for co-op.",
                "The mod uses its own saves: ER0000.mw2er.sl2 (solo) and ER0000.mw2co2 (co-op); the save backup is a read-only copy.",
                "Start Steam yourself before playing; this program never starts Steam or the game on its own.",
            ] {
                ui.add(egui::Label::new(RichText::new(l).small().weak()).wrap());
            }
        });
    }

    /// P3: the opt-in release check (shown only for a release that names its GitHub repository).
    fn update_check_card(&mut self, ui: &mut egui::Ui) {
        let Some(repo) = self.release.as_ref().map(|r| r.update_repo.clone()).filter(|r| !r.is_empty()) else { return };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let mut on = self.check_updates;
            if ui
                .checkbox(&mut on, "Check GitHub for a newer release")
                .on_hover_text(format!("Asks api.github.com for the newest release of {repo} (only while ticked; nothing is downloaded)"))
                .changed()
            {
                self.actions.push(Action::CheckUpdates(on));
            }
            if self.update_job.is_some() {
                ui.spinner();
            }
        });
        let Some(r) = self.update_check.clone() else { return };
        match r {
            Ok(l) if l.newer => {
                theme::tinted_card(ui, Level::Warn, |ui| {
                    ui.horizontal(|ui| {
                        theme::dot(ui, theme::AMBER);
                        ui.label(RichText::new(format!("{} is available", l.tag)).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::secondary(ui, "Open the release page", true).on_hover_text(&l.page).clicked() {
                                self.actions.push(Action::Open(l.page.clone()));
                            }
                        });
                    });
                    ui.label(RichText::new("Download its setup there and run it: it updates this install (both co-op players use the same version).").small().weak());
                });
            }
            Ok(l) => {
                ui.horizontal(|ui| {
                    theme::dot(ui, theme::GREEN);
                    ui.label(RichText::new(format!("You have the newest release ({}).", l.tag)).small().weak());
                });
            }
            Err(e) => {
                ui.horizontal(|ui| {
                    theme::dot(ui, theme::AMBER);
                    ui.add(egui::Label::new(RichText::new(format!("Release check: {e}")).small().color(theme::AMBER)).wrap());
                });
            }
        }
    }

    /// P3: the Update card of the Install page: versions, co-op id before / after, what changes, the steps that run.
    fn update_card(&mut self, ui: &mut egui::Ui, u: &update::Summary) {
        theme::section(ui, match u.kind {
            update::Kind::Update => "Update",
            update::Kind::Downgrade => "Older release",
            _ => "Repair",
        });
        theme::card(ui, |ui| {
            Self::status_row(ui, Level::Ok, "Version", &format!("{}  ->  {}", if u.from_version.is_empty() { "?" } else { &u.from_version }, u.to_version), "", |_| {});
            let to_id = if u.to_coop_id.is_empty() { "after the build".to_string() } else { u.to_coop_id.clone() };
            let lvl = if u.coop_changes() { Level::Warn } else { Level::Ok };
            Self::status_row(ui, lvl, "Co-op id", &format!("{}  ->  {to_id}", if u.from_coop_id.is_empty() { "?" } else { &u.from_coop_id }), "Equal ids = byte-identical installs", |_| {});
            if let Some(n) = u.coop_note() {
                Self::hint_line(ui, Level::Warn, &n);
            }
            ui.add_space(4.0);
            ui.label(RichText::new("What changes").weak());
            if u.changes.is_empty() && u.settings.is_none() {
                ui.label(RichText::new("Nothing in the mod's files: the install is checked and repaired where needed.").small().weak());
            }
            for c in &u.changes {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    theme::dot(ui, ACCENT);
                    ui.label(RichText::new(&c.what).small().strong());
                    if !c.detail.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&c.detail).small().weak()).truncate());
                    }
                });
            }
            if let Some(s) = &u.settings {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    theme::dot(ui, theme::GREEN);
                    ui.label(RichText::new("Your settings").small().strong());
                    ui.label(RichText::new(s.trim_start_matches("mw2er.toml merged: ")).small().weak());
                    theme::info(ui, "Hot settings (keys, sensitivity, HUD, camera offsets ...) keep your values; the build inputs ([damage], [regulation*], the gun list) and the release's safety settings come from the new release. Your old mw2er.toml is backed up in backups\\settings.");
                });
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Steps that run").weak());
                let build = u.build_steps();
                let rebuild: Vec<_> = build.iter().filter(|s| **s != installer::plan::StepId::Payload).collect();
                for s in &build {
                    theme::chip(ui, logic::step_short(*s), if *s == installer::plan::StepId::Payload { theme::GREEN } else { ACCENT }).on_hover_text(s.title());
                }
                theme::chip(ui, "verify + record", theme::MUTED).on_hover_text("The checks, the verification against the release, the profiles and the record always run");
                if rebuild.is_empty() {
                    ui.label(RichText::new("no rebuild").small().weak());
                }
            });
            if u.kind == update::Kind::Update {
                ui.label(RichText::new(format!("{} is kept for Roll back.", u.from_version)).small().weak());
            }
        });
    }

    fn page_games(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "Games", "Your Elden Ring and Modern Warfare 2 installs (read-only check).");
        if !self.detection_state(ui) {
            return;
        }
        let rows = logic::rows_for(&self.checks(), Page::Games).into_iter().cloned().collect::<Vec<_>>();
        let (er, rest): (Vec<Check>, Vec<Check>) = rows.into_iter().partition(|c| c.id.starts_with("er."));
        let (mw2, other): (Vec<Check>, Vec<Check>) = rest.into_iter().partition(|c| c.id.starts_with("mw2."));
        theme::section(ui, "Elden Ring");
        theme::card(ui, |ui| self.rows(ui, &er));
        theme::section(ui, "Call of Duty: Modern Warfare 2 (2009)");
        theme::card(ui, |ui| self.rows(ui, &mw2));
        theme::section(ui, "This PC");
        theme::card(ui, |ui| {
            if let Some(p) = self.env.steam_path.clone().filter(|_| !other.iter().any(|c| c.id == "steam")) {
                Self::status_row(ui, Level::Ok, "Steam", "found", &p.display().to_string(), |_| {});
            }
            self.rows(ui, &other);
        });
        self.tools_row(ui, &[("Choose the Elden Ring folder...", Action::Pick(Input::ErGame)), ("Choose the MW2 folder...", Action::Pick(Input::Mw2Dir)), ("Check again", Action::Detect)]);
    }

    fn page_me3(&mut self, ui: &mut egui::Ui) {
        let pin = self.release.as_ref().map(|r| r.pins.me3_version.clone()).unwrap_or_default();
        theme::page_title(ui, "me3", &format!("The mod loader (me3 {pin}): a separate open-source program with its own uninstaller."));
        if self.me3_job.is_some() {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("The me3 installer is open: finish it there; this page checks again when it closes.");
                });
            });
            ui.add_space(6.0);
        }
        if !self.detection_state(ui) {
            return;
        }
        let rows = logic::rows_for(&self.checks(), Page::Me3).into_iter().cloned().collect::<Vec<_>>();
        let bundled = self.session.as_ref().and_then(|s| s.facts.me3.bundled_installer.clone());
        theme::card(ui, |ui| {
            self.rows(ui, &rows);
            if let Some((p, false)) = &bundled {
                Self::status_row(ui, Level::Fail, "Bundled installer", "not official", &format!("{} does not match the official me3 0.13.0 installer: it is not run.", p.display()), |_| {});
            }
        });
        self.tools_row(ui, &[("me3 release page", Action::Open(release::ME3_RELEASES.into())), ("Choose me3.exe...", Action::Pick(Input::Me3)), ("Check again", Action::Detect)]);
    }

    fn page_seamless(&mut self, ui: &mut egui::Ui) {
        let (ver, pin_ok) = self.release.as_ref().map(|r| (r.pins.seamless_version.clone(), !r.pins.ersc_sha256.is_empty())).unwrap_or_default();
        if self.pw_only {
            theme::page_title(ui, "Co-op password", "Type the new password; both players must use the same one.");
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Seamless Co-op").heading().strong());
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Co-op uses your own download of Seamless Co-op {ver} from Nexus Mods.")).weak());
                theme::info(
                    ui,
                    &format!(
                        "This setup never includes or downloads Seamless Co-op. It is copied into {}; your download and the Elden Ring folder stay unchanged.",
                        self.root.join(seamless::COPY_DIR).display()
                    ),
                );
            });
            ui.add_space(6.0);
        }
        if !pin_ok {
            theme::tinted_card(ui, Level::Fail, |ui| {
                ui.label("This release has no Seamless pin: co-op cannot be installed from it.");
            });
        }
        if !self.detection_state(ui) {
            return;
        }
        if !self.pw_only {
            let rows = logic::rows_for(&self.checks(), Page::Seamless).into_iter().cloned().collect::<Vec<_>>();
            let note = self.seamless_note.clone();
            theme::card(ui, |ui| {
                self.rows(ui, &rows);
                match &note {
                    Some(Ok(m)) => {
                        Self::status_row(ui, Level::Ok, "Chosen file", "right version", m, |_| {});
                    }
                    Some(Err(m)) => {
                        Self::status_row(ui, Level::Fail, "Chosen file", "not usable", m, |_| {});
                        Self::hint_line(ui, Level::Fail, m);
                    }
                    None => {}
                }
            });
            self.tools_row(ui, &[("Open Nexus Mods", Action::Open(release::NEXUS_SEAMLESS.into())), ("Choose the zip or ersc.dll...", Action::Pick(Input::Seamless)), ("Check again", Action::Detect)]);
            theme::section(ui, "Co-op password");
        }
        theme::card(ui, |ui| {
            let set = self.password_set();
            if set {
                ui.checkbox(&mut self.keep_password, "Keep the co-op password that is already set");
            }
            if !set || !self.keep_password {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.password).password(!self.show_password).hint_text("co-op password").desired_width(260.0));
                    ui.checkbox(&mut self.show_password, "Show");
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Both players type the same password; share it privately.").small().weak());
                theme::info(
                    ui,
                    "Written only into the mod's own copy of ersc_settings.ini (Seamless needs it there), never into logs or the install record. That copy also gets allow_invaders = 0 and save_file_extension = mw2co2 (co-op save ER0000.mw2co2).",
                );
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            theme::dot(ui, theme::AMBER);
            ui.label(RichText::new(format!("Never start the game with ersc_launcher.exe: use the \"{} - Co-op\" shortcut.", installer::DISPLAY_NAME)).small().color(theme::AMBER));
        });
    }

    fn page_install(&mut self, ui: &mut egui::Ui) {
        if self.run.is_some() || self.view.finished.is_some() {
            return self.install_progress(ui);
        }
        let upd = self.update.clone().filter(|u| u.kind != update::Kind::Fresh);
        match &upd {
            Some(u) if u.kind == update::Kind::Update => theme::page_title(ui, &u.headline(), "Only what changed is copied or rebuilt; the previous version is kept for Roll back."),
            Some(_) => theme::page_title(ui, "Repair", "Checks every file; only missing or changed parts are rebuilt."),
            None => theme::page_title(ui, "Install", "About 5-20 minutes; about 6 GB free space while it builds, 0.5 GB after."),
        }
        if !self.detection_state(ui) {
            return;
        }
        if let Some(u) = &upd {
            self.update_card(ui, u);
        }
        let r = self.release.clone();
        theme::card(ui, |ui| {
            let root = self.root.display().to_string();
            Self::status_row(ui, Level::Ok, "Install folder", &root, &root, |_| {});
            Self::status_row(ui, Level::Ok, "Mode", if self.mode.coop() { "solo + co-op" } else { "solo" }, "", |_| {});
            if let Some(r) = &r {
                Self::status_row(ui, Level::Ok, "Release", &format!("{}   co-op id {}", r.version, r.coop_id()), "", |_| {});
            }
        });
        // P2: the regulation is built in (no WitchyBND, no download); the WitchyBND card shows only on the fallback
        let tool = self.session.as_ref().map(|s| s.regulation_tool).unwrap_or_default();
        let tool_rows = logic::rows_for(&self.checks(), Page::Install).into_iter().cloned().collect::<Vec<_>>();
        let regulation_runs = self.plan.as_ref().is_some_and(|p| p.runs(installer::plan::StepId::Regulation));
        if (tool == Tool::Witchy && regulation_runs) || tool_rows.iter().any(|c| c.level != Level::Ok) {
            theme::section(ui, "Regulation tool");
            theme::card(ui, |ui| {
                self.rows(ui, &tool_rows);
                if tool == Tool::Witchy {
                    let mut allow = self.allow_download;
                    if ui
                        .checkbox(&mut allow, "Download WitchyBND 3.0.1.0 when needed (48 MB, official GitHub release)")
                        .on_hover_text(format!("Kept only if its sha256 equals the pin.\n{}", release::WITCHY_URL))
                        .changed()
                    {
                        self.actions.push(Action::AllowDownload(allow));
                    }
                }
            });
        }
        theme::section(ui, "Options");
        theme::card(ui, |ui| {
            ui.checkbox(&mut self.backup, "Back up my Elden Ring saves first").on_hover_text("A read-only copy of %APPDATA%\\EldenRing into the install folder's backups");
            ui.horizontal_wrapped(|ui| {
                if let Some(d) = &self.args.shortcut_dir {
                    ui.checkbox(&mut self.desktop, "Shortcuts").on_hover_text(d.display().to_string());
                } else {
                    ui.checkbox(&mut self.desktop, "Desktop shortcuts");
                    ui.checkbox(&mut self.start_menu, "Start menu shortcuts");
                }
            });
            let mut force = self.force;
            if ui.checkbox(&mut force, "Rebuild everything (repair)").on_hover_text("Normally the steps whose inputs did not change are skipped").changed() {
                self.actions.push(Action::SetForce(force));
            }
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("Advanced").small())
            .id_salt("advanced")
            .default_open(self.witchy_fallback)
            .show(ui, |ui| {
                let mut fb = self.witchy_fallback;
                if ui
                    .checkbox(&mut fb, "Build the regulation with WitchyBND (fallback)")
                    .on_hover_text(
                        "The setup builds the regulation itself, no extra tool needed. Only if the host asks: WitchyBND 3.0.1.0 instead (48 MB download or its zip). Both players must use the same builder; the co-op id shows it.",
                    )
                    .changed()
                {
                    self.actions.push(Action::SetWitchyFallback(fb));
                }
            });
        if let Some(p) = self.plan.clone() {
            ui.add_space(4.0);
            let runs = p.steps.iter().filter(|s| s.decision.runs()).count();
            egui::CollapsingHeader::new(RichText::new(format!("Plan: {} step(s) to run, {} skipped", runs, p.steps.len() - runs)).small())
                .id_salt("plan")
                .default_open(false)
                .show(ui, |ui| {
                    for s in &p.steps {
                        let (kind, why) = match &s.decision {
                            Decision::Run(w) => (theme::StepIcon::Pending, w.as_str()),
                            Decision::Skip(w) => (theme::StepIcon::Skipped, w.as_str()),
                        };
                        ui.horizontal(|ui| {
                            theme::step_icon(ui, kind);
                            ui.label(RichText::new(logic::step_short(s.id)).small()).on_hover_text(s.id.title());
                            if !why.is_empty() {
                                ui.label(RichText::new(why).small().weak());
                            }
                        });
                    }
                });
        }
        let blocking: Vec<Check> = self.session.as_ref().map(|s| s.blocking().into_iter().cloned().collect()).unwrap_or_default();
        if !blocking.is_empty() {
            ui.add_space(6.0);
            theme::tinted_card(ui, Level::Fail, |ui| {
                ui.label(RichText::new("Not ready").strong());
                for c in &blocking {
                    let t = logic::row_text(c);
                    Self::status_row(ui, Level::Fail, t.label, &t.value, &c.msg, |_| {});
                }
            });
        }
    }

    fn install_progress(&mut self, ui: &mut egui::Ui) {
        let running = self.run.is_some();
        let updating = self.update.as_ref().is_some_and(|u| u.kind == update::Kind::Update);
        let (title, sentence) = match &self.view.finished {
            _ if running && updating => ("Updating", "Only what changed is copied or rebuilt; the previous version is kept for Roll back."),
            _ if running => ("Installing", "This takes about 5-20 minutes; finished steps are kept if it stops."),
            Some((true, _)) => (if updating { "Updated" } else { "Installed" }, ""),
            _ if updating => ("Update stopped", "Finished steps are kept: fix the problem and continue, or roll back on the start page."),
            _ => ("Install stopped", "Finished steps are kept: fix the problem and continue."),
        };
        theme::page_title(ui, title, sentence);
        theme::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.view.short_headline()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0} %", self.view.overall() * 100.0)).weak());
                });
            });
            ui.add(egui::ProgressBar::new(self.view.overall()).desired_height(10.0).fill(ACCENT).corner_radius(5));
            if let Some(f) = self.view.fraction.filter(|_| running) {
                ui.add(egui::ProgressBar::new(f).desired_height(4.0).fill(theme::MUTED).corner_radius(2));
            }
            ui.add_space(6.0);
            let plan = self.view.plan.clone();
            let half = plan.len().div_ceil(2);
            ui.columns(2, |cols| {
                for (k, (id, runs)) in plan.iter().enumerate() {
                    let ui = &mut cols[if k < half { 0 } else { 1 }];
                    let st = self.view.status.get(id).cloned().unwrap_or(StepStatus::Pending);
                    let (icon, detail) = match &st {
                        StepStatus::Pending if *runs => (theme::StepIcon::Pending, String::new()),
                        StepStatus::Pending => (theme::StepIcon::Skipped, "skipped".to_string()),
                        StepStatus::Running => (theme::StepIcon::Running, String::new()),
                        StepStatus::Done(m) => (theme::StepIcon::Done, m.clone()),
                        StepStatus::Failed(m) => (theme::StepIcon::Failed, m.clone()),
                        StepStatus::Skipped(m) => (theme::StepIcon::Skipped, m.clone()),
                    };
                    let tip = if detail.is_empty() { id.title().to_string() } else { format!("{}\n{detail}", id.title()) };
                    ui.horizontal(|ui| {
                        theme::step_icon(ui, icon);
                        let t = RichText::new(logic::step_short(*id));
                        let t = match icon {
                            theme::StepIcon::Running => t.strong(),
                            theme::StepIcon::Failed => t.color(theme::RED),
                            theme::StepIcon::Pending | theme::StepIcon::Skipped => t.weak(),
                            theme::StepIcon::Done => t,
                        };
                        ui.label(t);
                    })
                    .response
                    .on_hover_text(tip);
                }
            });
        });
        if let Some((false, msg)) = self.view.finished.clone() {
            ui.add_space(8.0);
            theme::tinted_card(ui, Level::Fail, |ui| {
                ui.add(egui::Label::new(msg).wrap());
            });
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if running {
                let stopping = self.run.as_ref().is_some_and(|r| r.cancel.load(Ordering::SeqCst));
                if theme::secondary(ui, if stopping { "Stopping ..." } else { "Cancel" }, !stopping).clicked() {
                    self.actions.push(Action::Cancel);
                }
                theme::info(ui, "Cancel stops after the running tool; run the setup again to continue (finished steps are kept).");
            } else {
                if matches!(self.view.finished, Some((false, _))) && theme::secondary(ui, "Check again and continue", true).clicked() {
                    self.view = RunView::default();
                    self.actions.push(Action::Detect);
                }
                let logs = run::log_dir(&self.root);
                if logs.is_dir() && theme::secondary(ui, "Open the log folder", true).clicked() {
                    self.actions.push(Action::Open(logs.display().to_string()));
                }
            }
        });
        egui::CollapsingHeader::new(RichText::new("Show log").small()).id_salt("log").default_open(false).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("logtail").max_height(200.0).min_scrolled_height(160.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                for l in &self.view.log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        });
    }

    fn page_done(&mut self, ui: &mut egui::Ui) {
        let st = self.installed.clone().unwrap_or_default();
        theme::page_title(ui, "Ready to play", &format!("{} {} is installed. Start Steam first, then press Play.", installer::DISPLAY_NAME, st.release_version));
        if st.mode.coop() {
            theme::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Co-op id").weak());
                    ui.label(RichText::new(&st.coop_id).size(24.0).strong().color(ACCENT));
                    ui.label(RichText::new("must match your friend's").small().weak());
                    theme::info(ui, "Compare it with your friend before playing: equal ids = byte-identical installs.");
                });
                if let Some(n) = self.update.as_ref().filter(|u| u.kind == update::Kind::Update && st.release_version == u.to_version && u.coop_changes()).map(|u| {
                    format!("The co-op id changed ({} -> {}): both players must use {} to play together.", u.from_coop_id, u.to_coop_id, u.to_version)
                }) {
                    ui.horizontal(|ui| {
                        theme::dot(ui, theme::AMBER);
                        ui.add(egui::Label::new(RichText::new(n).small().color(theme::AMBER)).wrap());
                    });
                }
            });
            ui.add_space(10.0);
        }
        let idle = !self.busy();
        ui.horizontal(|ui| {
            if st.mode.coop() && theme::big(ui, "Play co-op", true, idle).clicked() {
                self.actions.push(Action::Play(Mode::Coop));
            }
            if theme::big(ui, "Play solo", !st.mode.coop(), idle).clicked() {
                self.actions.push(Action::Play(Mode::Solo));
            }
        });
        if st.mode.coop() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                theme::dot(ui, theme::AMBER);
                ui.label(RichText::new("Never use ersc_launcher.exe: Play co-op and the Co-op shortcut start it the safe way.").small().color(theme::AMBER));
            });
        }
        self.report_card(ui);
        theme::section(ui, "Shortcuts");
        ui.horizontal_wrapped(|ui| {
            if st.shortcuts.is_empty() {
                theme::chip(ui, "none", theme::MUTED);
            }
            for s in &st.shortcuts {
                theme::chip(ui, &logic::shortcut_chip(s), theme::GREEN).on_hover_text(s.display().to_string());
            }
            theme::info(
                ui,
                &format!("\"{}\" = solo; \"- Co-op\" = with Seamless Co-op (me3 --no-mem-patch); \"- Setup\" = this window (repair, update, verify, uninstall).", installer::DISPLAY_NAME),
            );
        });
        ui.add_space(6.0);
        if theme::secondary(ui, "Open the install folder", true).on_hover_text(self.root.display().to_string()).clicked() {
            self.actions.push(Action::Open(self.root.display().to_string()));
        }
        if st.coop_id.is_empty() || !st.mode.coop() {
            ui.label(RichText::new(format!("Build id {} (the co-op id, if you add co-op later)", st.coop_id)).small().weak());
        }
    }

    fn page_uninstall(&mut self, ui: &mut egui::Ui) {
        const UNTOUCHED: &str = "Never touched: the game folders, your Seamless Co-op download, me3 (it has its own uninstaller) and %APPDATA%\\EldenRing. The mod's saves stay there (ER0000.mw2er.sl2, ER0000.mw2co2); delete them in Explorer if you want.";
        theme::page_title(ui, "Uninstall", &format!("Removes {} from this PC.", installer::DISPLAY_NAME));
        self.uninstall_ready = false;
        if let Some(r) = self.uninstall_done.clone() {
            match r {
                Ok(lines) => {
                    theme::tinted_card(ui, Level::Ok, |ui| {
                        ui.horizontal(|ui| {
                            theme::dot(ui, theme::GREEN);
                            ui.label(RichText::new(format!("{} was removed.", installer::DISPLAY_NAME)).strong());
                        });
                        egui::CollapsingHeader::new(RichText::new("Details").small()).id_salt("udone").default_open(false).show(ui, |ui| {
                            for l in lines {
                                ui.label(RichText::new(l).monospace().small());
                            }
                        });
                    });
                }
                Err(e) => {
                    theme::tinted_card(ui, Level::Fail, |ui| {
                        ui.add(egui::Label::new(format!("Uninstall failed: {e}")).wrap());
                    });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Game folders, saves, Seamless Co-op and me3 were not touched.").small().weak());
                theme::info(ui, UNTOUCHED);
            });
            return;
        }
        let me = std::env::current_exe().ok();
        match uninstall::plan(&self.root, self.delete_backups, me.as_deref()) {
            Err(e) => {
                theme::tinted_card(ui, Level::Fail, |ui| {
                    ui.add(egui::Label::new(e).wrap());
                    if theme::secondary(ui, "Choose the install folder...", true).clicked() {
                        self.actions.push(Action::Pick(Input::Root));
                    }
                });
            }
            Ok(p) => {
                self.uninstall_ready = true;
                theme::card(ui, |ui| {
                    let root = self.root.display().to_string();
                    Self::status_row(ui, Level::Ok, "Install folder", &root, &root, |_| {});
                    let del = p.delete.len() + p.shortcuts.len() + p.registry.len() + usize::from(p.self_in_root.is_some());
                    Self::status_row(ui, Level::Warn, "Deletes", &format!("{del} item(s)"), "", |_| {});
                    Self::status_row(ui, Level::Ok, "Keeps", &format!("{} item(s)", p.keep.len()), "", |_| {});
                    egui::CollapsingHeader::new(RichText::new("Details").small()).id_salt("uplan").default_open(false).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt("uplan_s").max_height(200.0).min_scrolled_height(160.0).show(ui, |ui| {
                            ui.label(RichText::new(p.describe()).monospace().small());
                        });
                    });
                });
                ui.add_space(4.0);
                ui.checkbox(&mut self.delete_backups, "Also delete backups").on_hover_text("backups\\: the save copies made by this setup and the launcher");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Game folders, saves, Seamless Co-op and me3 are never touched.").small().weak());
                    theme::info(ui, UNTOUCHED);
                });
            }
        }
    }
}

impl App {
    fn dev_step(&mut self, ctx: &egui::Context) {
        let Some(page) = self.dev.page else { return };
        if !self.dev.started {
            self.dev.started = true;
            self.page = page;
            if page != Page::Uninstall && page != Page::Done {
                self.detect(ctx);
            }
        }
        let ready = self.page == Page::Install && self.run.is_none() && self.view.finished.is_none() && self.detecting.is_none();
        if self.dev.install && ready && self.session.as_ref().is_some_and(|s| s.blocking().is_empty()) && self.plan.is_some() {
            self.dev.install = false;
            self.actions.push(Action::StartInstall);
        }
        if self.dev.close && self.run.is_none() && self.view.finished.is_some() {
            self.dev.close = false;
            self.actions.push(Action::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.dev_step(&ctx);
        if ctx.input(|i| i.viewport().close_requested())
            && let Some(r) = &self.run
        {
            // closing during the install: stop cleanly first (the window closes when the tool has stopped)
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            r.cancel.store(true, Ordering::SeqCst);
            self.close_after_run = true;
        }
        self.draw(ui);
        for a in std::mem::take(&mut self.actions) {
            self.apply(a, &ctx);
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join("mw2er-installer-tests").join(format!("gui-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A release with pins only (no game, no Steam): every page has red rows to draw.
    fn fixture(t: &Path) -> (Env, Args) {
        let pkg = t.join("pkg");
        std::fs::create_dir_all(pkg.join("manifest")).unwrap();
        std::fs::write(
            pkg.join(release::HOST_PINS),
            r#"{"er_exe_version":"2.7.2.0","er_app_version":"Steam build 25827938","me3_version":"0.13.0","seamless_version":"2.0.1","ersc_sha256":"00","witchybnd_zip_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        )
        .unwrap();
        std::fs::write(pkg.join(release::HOST_MANIFEST), r#"{"files":{}}"#).unwrap();
        let env = Env {
            local_appdata: Some(t.join("Local")),
            appdata: Some(t.join("Roaming")),
            downloads: Some(t.join("Downloads")),
            ..Default::default()
        };
        let args = Args { root: Some(t.join("MW2inER")), payload: Some(pkg), mode: Some(Mode::Coop), no_register: true, no_shortcuts: true, ..Default::default() };
        (env, args)
    }

    fn frame(ctx: &egui::Context, app: &mut App) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.draw(ui));
        out.textures_delta.clear(); // no renderer here
    }

    /// Every page draws (twice: layout passes) on fixture data, with and without a detection result, while
    /// installing and after; nothing is clicked, so no dialog, thread, download or tool starts.
    #[test]
    fn every_page_draws_headless() {
        let t = tmp("pages");
        let (env, args) = fixture(&t);
        let mut app = App::new(args, env.clone(), Start::Wizard, Default::default(), Default::default());
        assert!(app.release.is_some(), "{:?}", app.payload);
        assert_eq!(app.mode, Mode::Coop);
        let ctx = egui::Context::default();
        style(&ctx);
        let pages = [Page::Welcome, Page::Games, Page::Me3, Page::Seamless, Page::Install, Page::Done, Page::Uninstall];
        for p in pages {
            app.page = p;
            frame(&ctx, &mut app);
            frame(&ctx, &mut app);
        }
        let mut req = app.request();
        req.probe_python = false;
        let s = Session::prepare(env, req).unwrap();
        assert!(!s.blocking().is_empty(), "no games in the fixture");
        app.plan = s.plan();
        app.session = Some(s);
        for p in pages {
            app.page = p;
            frame(&ctx, &mut app);
            frame(&ctx, &mut app);
            assert!(app.actions.is_empty(), "drawing alone triggers nothing on {p:?}");
        }
        // Next stays disabled on a page with red rows
        app.page = Page::Games;
        assert!(!app.primary().unwrap().enabled);
        assert_eq!(app.footer_hint().map(|h| h.0), Some(Level::Fail));
        app.page = Page::Seamless;
        assert!(!app.primary().unwrap().enabled);
        // Install is the footer's primary button, disabled while rows are red; Done has the Play buttons instead
        app.page = Page::Install;
        let p = app.primary().unwrap();
        assert_eq!((p.label.as_str(), p.enabled, p.action), ("Install", false, Action::StartInstall));
        app.page = Page::Done;
        assert!(app.primary().is_none() && app.back_action().is_none());
        // no install in the fixture: the Uninstall page offers no Uninstall button
        app.page = Page::Uninstall;
        frame(&ctx, &mut app);
        assert!(!app.uninstall_ready && app.primary().is_none());
        assert_eq!(app.back_action().map(|b| b.1), Some(Action::Goto(Page::Welcome)));
        // changing only the password: Save password replaces Next, Back returns to the start
        app.page = Page::Seamless;
        app.pw_only = true;
        app.password = "pw".into();
        assert_eq!(app.primary().unwrap().action, Action::SavePassword);
        assert_eq!(app.back_action().map(|b| b.1), Some(Action::Goto(Page::Welcome)));
        app.pw_only = false;
        app.password.clear();
        // the progress view
        app.page = Page::Install;
        app.view.apply(&Progress::Plan(vec![(installer::plan::StepId::Preflight, true), (installer::plan::StepId::Dumps, true)]));
        app.view.apply(&Progress::StepStart { id: installer::plan::StepId::Dumps, index: 1, total: 2 });
        app.view.apply(&Progress::Fraction { id: installer::plan::StepId::Dumps, value: 0.4 });
        frame(&ctx, &mut app);
        app.view.apply(&Progress::Finished { ok: false, msg: "INSTALL FAILED: x".into() });
        frame(&ctx, &mut app);
        assert!(app.actions.is_empty());
        let _ = std::fs::remove_dir_all(&t);
    }

    /// P3: over an older install the Welcome page offers "Update 0.2.0 -> 0.2.1"; the Install page shows the Update
    /// card and an Update button; Roll back asks first; the release check is offered only with `update_repo`.
    /// P4: the window title is the display name (internal ids stay MW2inER).
    #[test]
    fn title_is_the_display_name() {
        assert_eq!(TITLE, format!("{} - Setup", installer::DISPLAY_NAME));
        assert!(!TITLE.contains("MW2 in Elden Ring") && !installer::DISPLAY_NAME.contains("MW2"));
        assert_eq!(installer::run::APP_KEY, r"Software\MW2inER");
        assert!(installer::run::UNINSTALL_KEY.ends_with(r"\Uninstall\MW2inER") && installer::run::INSTALLED_EXE == "MW2inER.exe");
    }

    #[test]
    fn the_update_pages() {
        let t = tmp("update");
        let (env, args) = fixture(&t);
        // release 0.2.1 (release.json layout) with an update repository
        let pkg = t.join("pkg");
        let pins: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(pkg.join(release::HOST_PINS)).unwrap()).unwrap();
        let rj = serde_json::json!({"release_version": "0.2.1", "pins": pins, "manifest": {"files": {}}, "update_repo": "owner/MW2inER"});
        std::fs::write(pkg.join(release::RELEASE_JSON), rj.to_string()).unwrap();
        let mut app = App::new(args.clone(), env.clone(), Start::Wizard, Default::default(), Default::default());
        assert_eq!(app.release.as_ref().unwrap().update_repo, "owner/MW2inER");
        assert_eq!(app.welcome_label(), "Next", "no install yet");
        let mut st = InstallState::new();
        st.release_version = "0.2.0".into();
        st.coop_id = "2799328d".into();
        app.installed = Some(st.clone());
        assert_eq!(app.welcome_label(), "Update 0.2.0 -> 0.2.1");
        st.release_version = "0.2.1".into();
        app.installed = Some(st.clone());
        assert_eq!(app.welcome_label(), "Repair / change");
        st.release_version = "0.3.0".into();
        app.installed = Some(st.clone());
        assert_eq!(app.welcome_label(), "Install 0.2.1 (older)");
        // an interrupted update still says what it is
        st.release_version = "0.2.1".into();
        st.update_from = "0.2.0".into();
        app.installed = Some(st);
        assert_eq!(app.welcome_label(), "Update 0.2.0 -> 0.2.1");
        let ctx = egui::Context::default();
        style(&ctx);
        app.page = Page::Welcome;
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert!(app.actions.is_empty() && app.update_job.is_none(), "the release check runs only when ticked");
        // the Update card + the Update button
        let mut req = app.request();
        req.probe_python = false;
        let s = Session::prepare(env.clone(), req).unwrap();
        app.plan = s.plan();
        app.session = Some(s);
        app.update = Some(update::Summary {
            kind: update::Kind::Update,
            from_version: "0.2.0".into(),
            to_version: "0.2.1".into(),
            from_coop_id: "2799328d".into(),
            to_coop_id: "6c1e0f42".into(),
            changes: vec![update::Change { what: "The mod (mw2er.dll)".into(), detail: "1 changed: mw2er.dll".into() }],
            steps: vec![installer::plan::StepId::Preflight, installer::plan::StepId::Payload, installer::plan::StepId::VerifyAll],
            settings: Some("mw2er.toml merged: 4 of your setting(s) kept".into()),
        });
        app.page = Page::Install;
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert_eq!(app.primary().unwrap().label, "Update");
        assert!(app.actions.is_empty());
        // Roll back: a confirmation first, then the store is used
        app.rollback = Some(("0.2.1".into(), "0.2.0".into()));
        app.page = Page::Welcome;
        app.apply(Action::AskRollback(true), &ctx);
        assert!(app.confirm_rollback);
        frame(&ctx, &mut app);
        app.apply(Action::AskRollback(false), &ctx);
        assert!(!app.confirm_rollback);
        app.apply(Action::Rollback, &ctx);
        assert!(app.note.as_ref().is_some_and(|(l, m)| *l == Level::Fail && m.contains("no install in")), "{:?}", app.note);
        // a check result draws (no request is made here)
        app.update_check = Some(Ok(Latest { tag: "0.2.2".into(), newer: true, page: "https://github.com/owner/MW2inER/releases/tag/0.2.2".into(), sha256: vec![] }));
        frame(&ctx, &mut app);
        app.update_check = Some(Err("cannot reach GitHub".into()));
        frame(&ctx, &mut app);
        assert!(app.actions.is_empty());
        let _ = std::fs::remove_dir_all(&t);
    }

    /// P3: Roll back from the window on a real store (temp fixture): the old DLL and record come back.
    #[test]
    fn rollback_from_the_window() {
        let t = tmp("rollback");
        let (env, args) = fixture(&t);
        let root = t.join("MW2inER");
        let g = Guard::default();
        let mut st = InstallState::new();
        st.release_version = "0.2.0".into();
        st.coop_id = "aaaaaaaa".into();
        st.save(&root, &g).unwrap();
        std::fs::create_dir_all(root.join(r"build\natives")).unwrap();
        std::fs::write(root.join(r"build\natives\mw2er.dll"), b"old").unwrap();
        std::fs::write(root.join("mw2er.toml"), "").unwrap();
        let mut store = rollback::Store::begin(&root, &st, "0.2.1", &g).unwrap();
        store.save_file(&root, "build/natives/mw2er.dll", &g).unwrap();
        std::fs::write(root.join(r"build\natives\mw2er.dll"), b"new").unwrap();
        st.release_version = "0.2.1".into();
        st.coop_id = "bbbbbbbb".into();
        st.save(&root, &g).unwrap();
        store.finish("bbbbbbbb", &g).unwrap();
        let mut app = App::new(args, env, Start::Wizard, Default::default(), Default::default());
        assert_eq!(app.rollback, Some(("0.2.1".to_string(), "0.2.0".to_string())));
        let ctx = egui::Context::default();
        app.apply(Action::AskRollback(true), &ctx);
        app.apply(Action::Rollback, &ctx);
        assert!(app.note.as_ref().is_some_and(|(l, m)| *l == Level::Ok && m.contains("Rolled back to 0.2.0") && m.contains("aaaaaaaa")), "{:?}", app.note);
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"old");
        assert_eq!(app.installed.as_ref().unwrap().release_version, "0.2.0");
        assert!(app.rollback.is_none() && !app.confirm_rollback);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn dev_switch_parses() {
        assert_eq!(Dev::parse(""), Dev::default());
        let d = Dev::parse("page=install,install,close");
        assert_eq!((d.page, d.install, d.close), (Some(Page::Install), true, true));
        assert_eq!(Dev::parse("page=nope").page, None);
    }

    /// P2: a release made with regbuild needs no WitchyBND (no row, no step, no download box); the Advanced box
    /// switches to the WitchyBND fallback and back, and `--regulation-tool witchy` starts with it ticked.
    #[test]
    fn the_advanced_box_switches_the_regulation_builder() {
        let t = tmp("regtool");
        let (env, args) = fixture(&t);
        let pins = t.join(r"pkg\manifest\host_pins.json");
        let text = std::fs::read_to_string(&pins).unwrap().replace(
            r#""ersc_sha256":"00""#,
            r#""ersc_sha256":"00","regulation_builder":"regbuild","package_bnd4_sha256":"1111111111111111111111111111111111111111111111111111111111111111","rows_json_core_sha256":"2222222222222222222222222222222222222222222222222222222222222222""#,
        );
        std::fs::write(&pins, text).unwrap();
        let mut app = App::new(args.clone(), env.clone(), Start::Wizard, Default::default(), Default::default());
        let mut req = app.request();
        assert_eq!(req.regulation_tool, None, "the release's builder");
        req.probe_python = false;
        app.session = Some(Session::prepare(env.clone(), req).unwrap());
        let ids = |app: &App| app.checks().iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(app.session.as_ref().unwrap().regulation_tool, Tool::Regbuild);
        assert!(!ids(&app).contains(&"witchy") && !ids(&app).contains(&"regulation.tool"), "{:?}", ids(&app));
        let ctx = egui::Context::default();
        style(&ctx);
        app.page = Page::Install;
        frame(&ctx, &mut app);
        app.apply(Action::SetWitchyFallback(true), &ctx);
        assert_eq!(app.session.as_ref().unwrap().regulation_tool, Tool::Witchy);
        assert_eq!(app.request().regulation_tool, Some(Tool::Witchy));
        assert!(ids(&app).contains(&"witchy") && ids(&app).contains(&"regulation.tool"), "{:?}", ids(&app));
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        app.apply(Action::SetWitchyFallback(false), &ctx);
        assert_eq!(app.session.as_ref().unwrap().regulation_tool, Tool::Regbuild);
        assert!(!ids(&app).contains(&"witchy"));
        // --regulation-tool witchy
        let a2 = Args { regulation_tool: Some(Tool::Witchy), ..args };
        let app2 = App::new(a2, env, Start::Wizard, Default::default(), Default::default());
        assert!(app2.witchy_fallback && app2.request().regulation_tool == Some(Tool::Witchy));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn the_password_moves_into_the_secret_and_out_of_the_field() {
        let t = tmp("pw");
        let (env, args) = fixture(&t);
        let mut app = App::new(args, env.clone(), Start::Wizard, Default::default(), Default::default());
        let mut req = app.request();
        req.probe_python = false;
        app.session = Some(Session::prepare(env, req).unwrap());
        app.page = Page::Seamless;
        app.password = "  ".into();
        assert!(!app.primary().unwrap().enabled, "a blank password is no password");
        app.password = "hunter22".into();
        // blocked (no games): the install does not start and the field keeps what was typed
        let ctx = egui::Context::default();
        app.start_install(&ctx);
        assert!(app.run.is_none() && app.session.is_some());
        assert!(app.note.as_ref().is_some_and(|(l, _)| *l == Level::Fail));
        assert_eq!(app.password.len(), 8);
        // not blocked: the field is emptied into the Secret (the worker is not started here: the session is gone
        // only when it really starts, so this checks the hand-over rule on a cleared session)
        app.session.as_mut().unwrap().checks.clear();
        app.allow_download = true; // the WitchyBND row is re-evaluated at the start: the pinned download is allowed
        app.session.as_mut().unwrap().facts.er.game = None; // plan() = None: install() fails at once on the worker
        app.start_install(&ctx);
        assert!(app.password.is_empty(), "moved out of the text field");
        assert!(app.run.is_some());
        // the worker ends with a preflight failure (no game); nothing was written
        let t0 = std::time::Instant::now();
        while app.run.is_some() && t0.elapsed() < Duration::from_secs(20) {
            app.poll(&ctx);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(app.view.finished, Some((false, _))), "{:?}", app.view.finished);
        assert!(!t.join("MW2inER").exists(), "a refused preflight writes nothing");
        let _ = std::fs::remove_dir_all(&t);
    }
}
