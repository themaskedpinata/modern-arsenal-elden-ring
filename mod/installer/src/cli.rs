//! The console front end (`--cli`), also what the shortcuts run (`MW2inER.exe play solo|coop`).
//!
//! ```text
//! MW2inER-setup.exe [--cli] status    [--root DIR] [--payload DIR] [--mode solo|coop] [input options]
//! MW2inER-setup.exe [--cli] install   [--root DIR] [--payload DIR] [--mode solo|coop] [input options]
//!                                     [--regulation-tool regbuild|witchy] [--download-witchy] [--password-stdin]
//!                                     [--keep-dumps] [--force]
//!                                     [--no-backup] [--backup-dir DIR] [--no-shortcuts] [--shortcut-dir DIR]
//!                                     [--no-register] [--dry-run] [--verbose]
//! MW2inER-setup.exe [--cli] dry-run   (= install --dry-run)
//! MW2inER-setup.exe [--cli] verify    [--root DIR] [--quick]
//! MW2inER-setup.exe [--cli] play solo|coop [--root DIR] [--dry-run]
//! MW2inER-setup.exe [--cli] uninstall [--root DIR] [--delete-backups] [--yes] [--dry-run]
//! MW2inER-setup.exe [--cli] rollback  [--root DIR] [--yes] [--dry-run]    (P3: back to the version before the update)
//! MW2inER-setup.exe [--cli] check-update [--root DIR] [--payload DIR]     (P3: opt-in GitHub Releases check)
//! MW2inER-setup.exe --cli regulation-info [--root DIR]   (release step: the builder-independent regulation pins)
//! MW2inER-setup.exe [--gui] [--root DIR] [--payload DIR|ZIP] [--mode solo|coop]     (the setup window)
//! input options: --er DIR --mw2 DIR --me3 EXE --seamless ersc.dll|ZIP --witchy-zip ZIP --python EXE --python-zip ZIP
//! (--payload may be the release folder or its zip)
//! ```
//! Which front end runs is decided by [`front_end`]: `--cli` always means this console front end; no command means
//! the setup window; `play` / `uninstall` started with a console of their own (a shortcut, Apps & Features) get the
//! launcher's message boxes / the window's uninstall page; everything else stays in the console.
//! Exit codes: 0 pass, 1 fail, 2 usage. The co-op password is read masked (or one line from stdin with
//! `--password-stdin`) and goes only into the mod's own ersc_settings.ini copy.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::detect::{self, Check, Env, Fix, Level};
use crate::guard::Guard;
use crate::launch;
use crate::regulation::{self, Tool};
use crate::release::{self, Release};
use crate::rollback;
use crate::run::{self, Progress, Request, Session, ShortcutDirs};
use crate::update;
use crate::seamless::Secret;
use crate::state::{InstallState, Mode};
use crate::uninstall;
use crate::util;
use crate::win;

pub const USAGE: &str = "usage: MW2inER-setup.exe [--cli] <status|install|dry-run|verify|play solo|play coop|uninstall|rollback|check-update|regulation-info> [options]
  --root DIR          install folder (default: the installed copy's folder, else %LOCALAPPDATA%\\MW2inER)
  --payload DIR       release files to install from (default: the release inside this setup exe, else this
                      exe's folder, else the install folder)
  --mode solo|coop    co-op adds Seamless Co-op (default: the last install's mode, else solo)
  --er DIR --mw2 DIR --me3 EXE --seamless (ersc.dll|zip) --witchy-zip ZIP --python EXE   choose inputs by hand
  --regulation-tool regbuild|witchy   the regulation builder (default: the one the release was made with, the
                      built-in regbuild; witchy = the WitchyBND 3.0.1.0 fallback, needs its zip or the download)
  --download-witchy   allow the pinned official WitchyBND 3.0.1.0 download (sha256-checked; fallback only)
  --python-zip ZIP    the official python-3.14.7-embed-amd64.zip (sha256-pinned; else the release's runtime\\python)
  Ctrl+C during the install stops it cleanly; run it again to continue (logs: <root>\\logs\\install)
  --password-stdin    read the co-op password as one line from stdin (otherwise asked, masked)
  --keep-dumps --force --no-backup --backup-dir DIR --no-shortcuts --shortcut-dir DIR --no-register
  --dry-run           show what would happen, change nothing
  --quick (verify)    only the launcher's quick hashes      --delete-backups --yes (uninstall)
  rollback            put the version from before the last update back (files, mw2er.toml, record); --yes --dry-run
  check-update        ask GitHub Releases for a newer release (only when you run it; nothing is downloaded)
  --verbose           print every line of the build tools
  --gui               open the setup window (the default without a command; play / uninstall started from a
                      shortcut use the launcher's message boxes / the window's uninstall page unless --cli is given)";

/// Parsed command line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    pub command: String,
    pub sub: Option<String>,
    pub root: Option<PathBuf>,
    pub payload: Option<PathBuf>,
    pub mode: Option<Mode>,
    pub er: Option<PathBuf>,
    pub mw2: Option<PathBuf>,
    pub me3: Option<PathBuf>,
    pub seamless: Option<PathBuf>,
    pub witchy_zip: Option<PathBuf>,
    pub python: Option<PathBuf>,
    pub python_zip: Option<PathBuf>,
    pub download_witchy: bool,
    /// `--regulation-tool` (None = the release's builder).
    pub regulation_tool: Option<Tool>,
    pub password_stdin: bool,
    pub keep_dumps: bool,
    pub force: bool,
    pub no_backup: bool,
    pub backup_dir: Option<PathBuf>,
    pub no_shortcuts: bool,
    pub shortcut_dir: Option<PathBuf>,
    pub no_register: bool,
    pub dry_run: bool,
    pub quick: bool,
    pub delete_backups: bool,
    pub yes: bool,
    pub verbose: bool,
    pub gui: bool,
}

pub const COMMANDS: &[&str] = &["status", "install", "dry-run", "verify", "play", "launch", "uninstall", "rollback", "check-update", "regulation-info", "help"];

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter().peekable();
    let val = |name: &str, it: &mut std::iter::Peekable<std::slice::Iter<String>>| -> Result<PathBuf, String> {
        it.next().map(PathBuf::from).ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(x) = it.next() {
        match x.as_str() {
            "--cli" => {}
            "--root" => a.root = Some(val(x, &mut it)?),
            "--payload" => a.payload = Some(val(x, &mut it)?),
            "--mode" => {
                let v = val(x, &mut it)?;
                a.mode = Some(Mode::parse(&v.to_string_lossy()).ok_or("--mode is solo or coop")?);
            }
            "--er" => a.er = Some(val(x, &mut it)?),
            "--mw2" => a.mw2 = Some(val(x, &mut it)?),
            "--me3" => a.me3 = Some(val(x, &mut it)?),
            "--seamless" => a.seamless = Some(val(x, &mut it)?),
            "--witchy-zip" => a.witchy_zip = Some(val(x, &mut it)?),
            "--python" => a.python = Some(val(x, &mut it)?),
            "--python-zip" => a.python_zip = Some(val(x, &mut it)?),
            "--backup-dir" => a.backup_dir = Some(val(x, &mut it)?),
            "--shortcut-dir" => a.shortcut_dir = Some(val(x, &mut it)?),
            "--download-witchy" => a.download_witchy = true,
            "--regulation-tool" => {
                let v = val(x, &mut it)?;
                a.regulation_tool = Some(Tool::parse(&v.to_string_lossy()).ok_or("--regulation-tool is regbuild or witchy")?);
            }
            "--password-stdin" => a.password_stdin = true,
            "--keep-dumps" => a.keep_dumps = true,
            "--force" => a.force = true,
            "--no-backup" => a.no_backup = true,
            "--no-shortcuts" => a.no_shortcuts = true,
            "--no-register" => a.no_register = true,
            "--dry-run" => a.dry_run = true,
            "--quick" => a.quick = true,
            "--delete-backups" => a.delete_backups = true,
            "--yes" | "-y" => a.yes = true,
            "--verbose" | "-v" => a.verbose = true,
            "--gui" => a.gui = true,
            "-h" | "--help" | "/?" => a.command = "help".into(),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if a.command.is_empty() => {
                if !COMMANDS.contains(&s) {
                    return Err(format!("unknown command {s}"));
                }
                a.command = s.to_string();
            }
            s if a.sub.is_none() && matches!(a.command.as_str(), "play" | "launch") => {
                a.mode = Some(Mode::parse(s).ok_or("play solo or play coop")?);
                a.sub = Some(s.to_string());
            }
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    if a.command == "dry-run" {
        a.command = "install".into();
        a.dry_run = true;
    }
    if a.command == "launch" {
        a.command = "play".into();
    }
    if a.command == "play" && a.sub.is_none() {
        return Err("play solo or play coop".into());
    }
    Ok(a)
}

/// The front end a command line gets (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontEnd {
    /// This console front end.
    Cli,
    /// The setup window (wizard).
    Wizard,
    /// The setup window on its uninstall page.
    UninstallWindow,
    /// `play` from a shortcut: preflight, a message box on a problem, start me3, exit.
    Launcher,
}

/// `owns_console`: Windows made a console just for this process (Explorer / a shortcut), see `win::owns_console`.
pub fn front_end(argv: &[String], owns_console: bool) -> FrontEnd {
    if argv.iter().any(|a| a == "--cli") {
        return FrontEnd::Cli;
    }
    let Ok(a) = parse(argv) else { return FrontEnd::Cli };
    let windowed = a.gui || owns_console;
    match a.command.as_str() {
        "" => FrontEnd::Wizard,
        "install" if a.gui && !a.dry_run => FrontEnd::Wizard,
        "play" if windowed && !a.dry_run => FrontEnd::Launcher,
        "uninstall" if windowed && !a.dry_run && !a.yes => FrontEnd::UninstallWindow,
        _ => FrontEnd::Cli,
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
}

/// The install root: `--root`, the folder of an installed copy, `HKCU\Software\MW2inER\InstallRoot`, the default.
pub fn resolve_root(a: &Args, env: &Env) -> PathBuf {
    if let Some(r) = &a.root {
        return util::normalize(r);
    }
    if let Some(d) = exe_dir().filter(|d| InstallState::path(d).is_file()) {
        return d;
    }
    if let Some(r) = win::reg_get_string(win::Hive::CurrentUser, run::APP_KEY, "InstallRoot").map(PathBuf::from).filter(|r| InstallState::path(r).is_file()) {
        return r;
    }
    env.default_root().unwrap_or_else(|| PathBuf::from(r"C:\MW2inER"))
}

/// The release folder: `--payload`, the release embedded in this exe (the downloaded `MW2inER-<ver>-setup.exe`,
/// unpacked into %TEMP%), this exe's folder, the install root.
pub fn resolve_payload(a: &Args, root: &Path) -> Result<PathBuf, String> {
    if let Some(p) = &a.payload {
        return Ok(util::normalize(p));
    }
    if let Ok(me) = std::env::current_exe()
        && let Some(stage) = crate::payload::embedded(&me)?
    {
        return Ok(stage);
    }
    if let Some(d) = exe_dir().filter(|d| Release::present(d)) {
        return Ok(d);
    }
    if Release::present(root) {
        return Ok(root.to_path_buf());
    }
    Err(format!(
        "no release files found next to this program or in {} (manifest\\host_pins.json + host_manifest.json, or manifest\\release.json): pass --payload <unzipped release folder>",
        root.display()
    ))
}

fn tag(l: Level) -> &'static str {
    match l {
        Level::Ok => "OK  ",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    }
}

fn fix_hint(f: &Fix) -> String {
    match f {
        Fix::OpenUrl(u) => format!("open {u}"),
        Fix::VerifyInSteam(id) => format!("Steam > Properties > Installed Files > Verify integrity (steam://nav/games/details/{id})"),
        Fix::UpdateInSteam(id) => format!("update the game in Steam (steam://nav/games/details/{id})"),
        Fix::Browse(i) => format!("choose it by hand ({})", match i {
            detect::Input::ErGame => "--er <ELDEN RING\\Game>",
            detect::Input::Mw2Dir => "--mw2 <MW2 folder>",
            detect::Input::Me3 => "--me3 <me3.exe>",
            detect::Input::Seamless => "--seamless <ersc.dll or zip>",
            detect::Input::WitchyZip => "--witchy-zip <zip>",
            detect::Input::Python => "--python <python.exe>",
            detect::Input::Root => "--root <folder>",
        }),
        Fix::RunMe3Installer(p) => format!("run the bundled official me3 installer {}", p.display()),
        Fix::DownloadWitchy => "--download-witchy, or --witchy-zip <zip>".into(),
    }
}

fn print_checks(out: &mut dyn Write, c: &[Check]) {
    for x in c {
        let _ = writeln!(out, "{} {}", tag(x.level), x.msg);
        if x.level != Level::Ok
            && let Some(f) = &x.fix
        {
            let _ = writeln!(out, "     -> {}", fix_hint(f));
        }
    }
}

fn request(a: &Args, env: &Env, root: PathBuf, payload: PathBuf, state: &InstallState) -> Request {
    let mode = a.mode.unwrap_or(if InstallState::path(&root).is_file() { state.mode } else { Mode::Solo });
    let shortcuts = if a.no_shortcuts {
        ShortcutDirs::default()
    } else if let Some(d) = &a.shortcut_dir {
        ShortcutDirs { desktop: Some(d.clone()), start_menu: None }
    } else {
        ShortcutDirs { desktop: env.desktop.clone(), start_menu: env.programs.clone() }
    };
    Request {
        root,
        payload,
        mode,
        over: detect::Overrides {
            er_game: a.er.clone(),
            mw2_dir: a.mw2.clone(),
            me3: a.me3.clone(),
            seamless: a.seamless.clone(),
            witchy_zip: a.witchy_zip.clone(),
            python: a.python.clone(),
        },
        allow_download: a.download_witchy,
        regulation_tool: a.regulation_tool,
        python_zip: a.python_zip.clone(),
        keep_dumps: a.keep_dumps,
        force: a.force,
        shortcuts,
        register: !a.no_register,
        self_exe: std::env::current_exe().ok(),
        save_root: if a.no_backup { None } else { env.save_root() },
        backup_dir: a.backup_dir.clone(),
        probe_python: true,
    }
}

fn ask_yes(q: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{q} [y/N] ");
    let mut l = String::new();
    std::io::stdin().read_line(&mut l).is_ok() && l.trim().eq_ignore_ascii_case("y")
}

fn read_password(a: &Args) -> Result<Secret, String> {
    let line = if a.password_stdin {
        let mut l = String::new();
        std::io::stdin().read_line(&mut l).map_err(|e| e.to_string())?;
        l
    } else {
        if !std::io::stdin().is_terminal() {
            return Err("the co-op password is needed: pass --password-stdin and pipe it in".into());
        }
        eprint!("Co-op password (the same on both PCs; typing is hidden): ");
        win::read_line_masked().map_err(|e| e.to_string())?
    };
    Secret::new(line).map_err(str::to_string)
}

/// Entry point; returns the process exit code.
pub fn main_with(argv: &[String], out: &mut dyn Write) -> i32 {
    let a = match parse(argv) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(out, "{e}\n{USAGE}");
            return 2;
        }
    };
    if a.command.is_empty() || a.command == "help" {
        let _ = writeln!(out, "{} setup {}\n{USAGE}", crate::DISPLAY_NAME, env!("CARGO_PKG_VERSION"));
        return if a.command == "help" { 0 } else { 2 };
    }
    let env = Env::from_system();
    let root = resolve_root(&a, &env);
    match a.command.as_str() {
        "status" | "install" => cmd_install(&a, env, root, out),
        "verify" => cmd_verify(&a, &env, &root, out),
        "play" => cmd_play(&a, &env, &root, out),
        "uninstall" => cmd_uninstall(&a, &env, &root, out),
        "rollback" => cmd_rollback(&a, &env, &root, out, &win::is_running),
        "check-update" => cmd_check_update(&a, &root, out),
        "regulation-info" => cmd_regulation_info(&root, out),
        _ => 2,
    }
}

fn cmd_install(a: &Args, env: Env, root: PathBuf, out: &mut dyn Write) -> i32 {
    let payload = match resolve_payload(a, &root) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            return 1;
        }
    };
    let state = InstallState::load_or_new(&root);
    let req = request(a, &env, root.clone(), payload.clone(), &state);
    let mut s = match Session::prepare(env, req) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            return 1;
        }
    };
    let _ = writeln!(out, "{} {} ({} mode)", crate::DISPLAY_NAME, s.release.version, if s.req.mode.coop() { "co-op" } else { "solo" });
    let _ = writeln!(out, "install folder: {}\nrelease files:  {}", root.display(), payload.display());
    if let Some(st) = InstallState::load(&root) {
        let _ = writeln!(out, "installed:      {} {} (co-op id {}), {}", st.release_version, if st.mode.coop() { "co-op" } else { "solo" }, st.coop_id, st.last_result);
    }
    let _ = writeln!(out, "release co-op id: {}", s.release.coop_id());
    let _ = writeln!(
        out,
        "regulation builder: {}{}",
        s.regulation_tool.label(),
        if regulation::by_content(&s.release.pins, s.regulation_tool) {
            format!(" (the release was made with {}: checked by content, the co-op id will differ)", Tool::of_release(&s.release.pins).label())
        } else {
            String::new()
        }
    );
    print_checks(out, &s.checks);
    let plan = s.plan();
    // P3: update / repair: what changes, what rebuilds, the co-op id before / after
    if let Some(u) = plan.as_ref().and_then(|p| s.update_summary(p)).filter(|u| u.kind != update::Kind::Fresh) {
        let _ = writeln!(out);
        for l in u.lines() {
            let _ = writeln!(out, "{l}");
        }
    }
    if let Some(p) = &plan
        && (a.command == "status" || a.dry_run) {
            let _ = writeln!(out, "\nplan:\n{}", p.describe());
        }
    let blocking = s.blocking().len();
    if a.command == "status" || a.dry_run {
        let _ = writeln!(out, "{}", if blocking == 0 { "READY: nothing blocks the install" } else { "NOT READY: fix the FAIL rows above" });
        if a.dry_run {
            let _ = writeln!(out, "DRY RUN: nothing was written");
        }
        return if blocking == 0 { 0 } else { 1 };
    }
    // WitchyBND: ask before the download when it is needed and not allowed yet
    if s.blocking().iter().any(|c| c.id == "witchy") && s.facts.witchy.zip.is_none() && s.release.witchy_source().is_some()
        && ask_yes(&format!("Download WitchyBND 3.0.1.0 (48 MB) from its official GitHub release ({})? The file is kept only if its sha256 equals the pin.", release::WITCHY_URL)) {
            s.req.allow_download = true;
            s.checks.retain(|c| c.id != "witchy");
        }
    if !s.blocking().is_empty() {
        let _ = writeln!(out, "INSTALL FAILED: fix the FAIL rows above (nothing was written)");
        return 1;
    }
    let mut password = None;
    if s.req.mode.coop() && (a.password_stdin || !s.facts.seamless.copy_ini.as_ref().is_some_and(|i| i.password_set)) {
        match read_password(a) {
            Ok(p) => password = Some(p),
            Err(e) => {
                let _ = writeln!(out, "FAIL {e}");
                return 1;
            }
        }
    }
    let verbose = a.verbose;
    // Ctrl+C: stop cleanly (the library logs every event into <root>\logs\install\install.log)
    static CANCEL: AtomicBool = AtomicBool::new(false);
    win::on_ctrl_c(&CANCEL);
    let r = s.install(password.as_ref(), &CANCEL, &mut |e| {
        let line = match &e {
            Progress::Plan(v) => format!("plan: {} step(s) to run, {} skipped", v.iter().filter(|x| x.1).count(), v.iter().filter(|x| !x.1).count()),
            Progress::StepStart { id, index, total } => format!("STEP {}/{} {}", index + 1, total, id.title()),
            Progress::Line { text, .. } => {
                if !verbose {
                    return;
                }
                format!("     {text}")
            }
            Progress::Fraction { .. } => return,
            Progress::StepDone { ok, secs, msg, .. } => format!("{} {msg} ({secs:.0} s)", if *ok { "OK  " } else { "FAIL" }),
            Progress::Skipped { id, reason } => format!("SKIP {}: {reason}", id.as_str()),
            Progress::Finished { msg, .. } => format!("\n{msg}"),
        };
        let _ = writeln!(out, "{line}");
    });
    match r {
        Ok(()) => {
            let _ = writeln!(out, "Start the game with the shortcut \"{}\" (Steam running), or: \"{}\" play solo", crate::DISPLAY_NAME, root.join(run::INSTALLED_EXE).display());
            0
        }
        Err(_) => 1,
    }
}

/// The guard of an existing install (its recorded game folders, this PC's Steam libraries, %APPDATA%\EldenRing).
pub fn state_guard(env: &Env, st: &InstallState) -> Guard {
    let steam = env.steam();
    Guard::new(st.paths.er_game.as_deref(), st.paths.mw2_dir.as_deref(), &steam.steamapps_dirs(), steam.steam_path.as_deref(), env.appdata.as_deref())
}

fn cmd_verify(a: &Args, env: &Env, root: &Path, out: &mut dyn Write) -> i32 {
    let Some(st) = InstallState::load(root) else {
        let _ = writeln!(out, "FAIL no install in {}", root.display());
        return 1;
    };
    let mut fails = 0;
    for (rel, want) in &st.preflight {
        match util::sha256_file(&root.join(rel.replace('/', "\\"))) {
            Ok(h) if h.eq_ignore_ascii_case(want) => {
                let _ = writeln!(out, "OK   {rel}");
            }
            _ => {
                fails += 1;
                let _ = writeln!(out, "FAIL {rel} changed or missing");
            }
        }
    }
    if !a.quick {
        let rel = match Release::load(root) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(out, "FAIL {e}");
                return 1;
            }
        };
        // the builder of the installed regulation (empty = before P2: WitchyBND); another builder than the release's
        // is checked by content (as the install's verify step does)
        let tool = Tool::parse(&st.regulation_tool).unwrap_or(Tool::Witchy);
        let by_content = regulation::by_content(&rel.pins, tool);
        let manifest = if by_content { regulation::manifest_without_regulation(&rel.manifest) } else { rel.manifest.clone() };
        let v = release::verify_plain(&manifest, root, release::Only::All);
        for k in v.mismatched.iter().chain(&v.missing).take(20) {
            let _ = writeln!(out, "FAIL {k}");
        }
        let _ = writeln!(out, "{}", v.summary());
        if !v.passed() {
            fails += 1;
        }
        let mut manifest_path = root.join(release::HOST_MANIFEST);
        if by_content {
            match regulation::check_content(&rel.pins, root) {
                Ok(m) => {
                    let _ = writeln!(out, "OK   {m}");
                }
                Err(e) => {
                    fails += 1;
                    let _ = writeln!(out, "FAIL {e}");
                }
            }
            manifest_path = root.join(run::VERIFY_MANIFEST);
            let text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
            if state_guard(env, &st).write(&manifest_path, text).is_err() {
                let _ = writeln!(out, "WARN cannot write {}", manifest_path.display());
            }
        }
        if let Some(py) = st.paths.python.as_ref().filter(|p| p.is_file()) {
            let job = crate::proc::python_job(
                "verify_cli",
                py,
                &root.join(r"tools\scripts\coop_manifest.py"),
                &["verify".into(), manifest_path.display().to_string(), root.display().to_string(), "--only".into(), "all".into()],
                &[],
                root,
                5,
                &run::log_dir(root),
            );
            let g = state_guard(env, &st);
            if g.check(&job.log).is_ok() {
                match crate::proc::run(&job, |_| {}) {
                    Ok(r) => {
                        let _ = writeln!(out, "coop_manifest.py: {}", r.last_line);
                        if !r.ok() {
                            fails += 1;
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(out, "WARN cannot run Python ({e}): canonical-JSON entries not checked");
                    }
                }
            }
        } else {
            let _ = writeln!(out, "WARN no Python recorded: rows.json / movement.json (canonical JSON) not checked");
        }
        let sha = util::sha256_file(&root.join(regulation::PACKAGE)).ok();
        let _ = writeln!(out, "co-op id {}", regulation::coop_id(&rel, tool, sha.as_deref()));
    }
    let _ = writeln!(out, "{}", if fails == 0 { "VERIFY PASSED" } else { "VERIFY FAILED: run the setup again (repair)" });
    i32::from(fails > 0)
}

/// What the launcher found before starting the game (shared by `play`, the launcher's message boxes and the
/// wizard's Play buttons).
pub struct PlayPrep {
    pub mode: Mode,
    /// Preflight rows (+ a `me3` row when me3 is missing); starting is allowed when none is `Fail`.
    pub rows: Vec<Check>,
    /// The me3 command (None when me3 was not found).
    pub cmd: Option<launch::LaunchCommand>,
    pub state: InstallState,
}

impl PlayPrep {
    pub fn can_start(&self) -> bool {
        self.cmd.is_some() && detect::worst(&self.rows) != Level::Fail
    }
}

/// The launcher's checks (reads only; nothing is started). Err = no usable install at all.
pub fn play_prepare(env: &Env, root: &Path, mode: Mode) -> Result<PlayPrep, String> {
    let st = InstallState::load(root).ok_or_else(|| format!("no install in {}: run the setup first", root.display()))?;
    if mode.coop() && !st.mode.coop() {
        return Err("this install is solo only: run the setup with co-op to add Seamless Co-op".into());
    }
    let pins = Release::load(root).map(|r| r.pins).unwrap_or_default();
    let er_version = st.paths.er_game.as_ref().and_then(|g| win::pe_version(&g.join("eldenring.exe"))).and_then(|v| v.file_version);
    let w = launch::World { steam_running: win::is_running("steam.exe"), er_running: win::is_running("eldenring.exe"), er_version, pins: &pins };
    let mut rows = launch::preflight(root, mode, &st, &w);
    let me3 = detect::find_me3(env, st.paths.me3.as_deref()).or_else(|| detect::find_me3(env, None));
    if me3.is_none() {
        rows.push(Check { id: "me3", level: Level::Fail, msg: "me3 not found (install me3 0.13.0 with the setup)".into(), fix: None });
    }
    let cmd = me3.map(|m| launch::command(&m, root, mode));
    Ok(PlayPrep { mode, rows, cmd, state: st })
}

/// Starts the game through me3 after the save backup rule (the only place that runs me3). Returns the lines done.
pub fn play_start(env: &Env, root: &Path, p: &PlayPrep) -> Result<Vec<String>, String> {
    let Some(cmd) = p.cmd.as_ref().filter(|_| p.can_start()) else { return Err("not starting: fix the FAIL rows first".into()) };
    let mut lines = Vec::new();
    if let Some(saves) = env.save_root() {
        let l = launch::save_backup_if_needed(&saves, &root.join("backups"), &util::local_folder_stamp(), &state_guard(env, &p.state))
            .map_err(|e| format!("save backup: {e} (not starting)"))?;
        lines.extend(l);
    }
    launch::spawn(cmd).map_err(|e| format!("cannot start me3: {e}"))?;
    lines.push(format!("Started: {}", cmd.display()));
    Ok(lines)
}

fn cmd_play(a: &Args, env: &Env, root: &Path, out: &mut dyn Write) -> i32 {
    let mode = a.mode.unwrap_or_default();
    let p = match play_prepare(env, root, mode) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            return 1;
        }
    };
    print_checks(out, &p.rows);
    if !p.can_start() {
        let _ = writeln!(out, "NOT STARTING: fix the FAIL rows above");
        return 1;
    }
    if a.dry_run {
        let _ = writeln!(out, "DRY RUN: would back up a changed ER0000.sl2, then run: {}", p.cmd.as_ref().map(|c| c.display()).unwrap_or_default());
        return 0;
    }
    match play_start(env, root, &p) {
        Ok(lines) => {
            for l in lines {
                let _ = writeln!(out, "OK   {l}");
            }
            0
        }
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            1
        }
    }
}
/// `regulation-info --root <folder with build\package\regulation.bin + build\rows.json>` (the release step, on the
/// host's build): the builder and the builder-independent pins, as `key=value` lines. Reads only.
fn cmd_regulation_info(root: &Path, out: &mut dyn Write) -> i32 {
    match regulation::content(root) {
        Ok(c) => {
            if !c.rows_package_sha256.eq_ignore_ascii_case(&c.package_sha256) {
                let _ = writeln!(out, "FAIL rows.json package_regulation_sha256 is not the sha256 of {}", root.join(regulation::PACKAGE).display());
                return 1;
            }
            let _ = writeln!(out, "generator={}", c.rows_generator);
            let _ = writeln!(out, "regulation_builder={}", c.tool().as_str());
            let _ = writeln!(out, "package_regulation_sha256={}", c.package_sha256);
            let _ = writeln!(out, "package_bnd4_sha256={}", c.bnd4_sha256);
            let _ = writeln!(out, "rows_json_core_sha256={}", c.rows_core_sha256);
            0
        }
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            1
        }
    }
}

/// `rollback`: the version before the last update back (P3). `is_running` = [`win::is_running`] (tests: a fake).
pub fn cmd_rollback(a: &Args, env: &Env, root: &Path, out: &mut dyn Write, is_running: &dyn Fn(&str) -> bool) -> i32 {
    let p = match rollback::plan(root) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            return 1;
        }
    };
    let _ = write!(out, "{}", p.describe());
    let st = InstallState::load(root).unwrap_or_default();
    let back_id = p.store.old_state().map(|o| o.coop_id).unwrap_or_default();
    let _ = writeln!(out, "co-op id: {} -> {back_id}", st.coop_id);
    if !back_id.is_empty() && back_id != st.coop_id {
        let _ = writeln!(out, "NOTE the co-op id changes back: both players must use {} again to play together.", p.to);
    }
    if a.dry_run {
        let _ = writeln!(out, "DRY RUN: nothing was changed");
        return 0;
    }
    if let Some(c) = run::running_game_check(is_running) {
        let _ = writeln!(out, "FAIL {}", c.msg);
        return 1;
    }
    if !a.yes && !ask_yes(&format!("Roll back to {}?", p.to)) {
        let _ = writeln!(out, "cancelled (pass --yes to roll back without asking)");
        return 1;
    }
    let me = std::env::current_exe().ok();
    match rollback::execute(&p, &state_guard(env, &st), me.as_deref()) {
        Ok(lines) => {
            for l in lines {
                let _ = writeln!(out, "OK   {l}");
            }
            if !a.no_register && !st.registry.is_empty() {
                use crate::win::RegValue::Str;
                let _ = win::reg_set_hkcu(run::APP_KEY, &[("Version", Str(&p.to))]);
                let _ = win::reg_set_hkcu(run::UNINSTALL_KEY, &[("DisplayVersion", Str(&p.to))]);
            }
            let _ = writeln!(out, "ROLLED BACK to {}", p.to);
            0
        }
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            1
        }
    }
}

/// `check-update`: the opt-in "newer release" check (decision D6). Only the release's `update_repo` on
/// api.github.com is asked; nothing is downloaded; the release page is printed for the browser.
fn cmd_check_update(a: &Args, root: &Path, out: &mut dyn Write) -> i32 {
    let rel = resolve_payload(a, root).ok().and_then(|p| crate::payload::resolve(&p).ok()).and_then(|p| Release::load(&p).ok());
    let Some(rel) = rel else {
        let _ = writeln!(out, "FAIL no release files found (pass --payload)");
        return 1;
    };
    if rel.update_repo.is_empty() {
        let _ = writeln!(out, "This release has no update check (no update_repo): the host sends the new setup when there is one.");
        return 0;
    }
    let current = InstallState::load(root).map(|s| s.release_version).filter(|v| !v.is_empty()).unwrap_or(rel.version.clone());
    let _ = writeln!(out, "asking {}/repos/{}/releases/latest (installed: {current}) ...", update::GITHUB_API, rel.update_repo);
    match update::check_latest(&rel.update_repo, &current) {
        Ok(l) => {
            let _ = writeln!(out, "{} {} (installed {current}): {}", if l.newer { "NEWER" } else { "OK  " }, l.tag, l.page);
            for h in &l.sha256 {
                let _ = writeln!(out, "     sha256 listed in the release notes: {h}");
            }
            if l.newer {
                let _ = writeln!(out, "Download the setup from that page yourself (this program never downloads it), then run it: it updates this install.");
            }
            0
        }
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            1
        }
    }
}

fn cmd_uninstall(a: &Args, env: &Env, root: &Path, out: &mut dyn Write) -> i32 {
    let me = std::env::current_exe().ok();
    let p = match uninstall::plan(root, a.delete_backups, me.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            return 1;
        }
    };
    let _ = write!(out, "{}", p.describe());
    let _ = writeln!(out, "Never touched: the game folders, your Seamless download, me3, %APPDATA%\\EldenRing (mod saves ER0000.mw2er.sl2 / ER0000.mw2co2 stay there).");
    if a.dry_run {
        let _ = writeln!(out, "DRY RUN: nothing was deleted");
        return 0;
    }
    if !a.yes && !ask_yes("Uninstall?") {
        let _ = writeln!(out, "cancelled (pass --yes to uninstall without asking)");
        return 1;
    }
    if win::is_running("eldenring.exe") {
        let _ = writeln!(out, "FAIL Elden Ring is running: close it first");
        return 1;
    }
    let st = InstallState::load(root).unwrap_or_default();
    match uninstall::execute(&p, &state_guard(env, &st), true) {
        Ok(lines) => {
            for l in lines {
                let _ = writeln!(out, "OK   {l}");
            }
            let _ = writeln!(out, "UNINSTALLED");
            0
        }
        Err(e) => {
            let _ = writeln!(out, "FAIL {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The release step's `regulation-info`: the builder and the builder-independent pins of a build folder.
    #[test]
    fn regulation_info_prints_the_pins() {
        let t = crate::util::testutil::TempDir::new("cli-reginfo");
        let reg = crate::regulation::fixture::regulation(4, 21);
        let sha = util::sha256_bytes(&reg);
        t.write(r"r\build\package\regulation.bin", &reg);
        t.write(r"r\build\rows.json", crate::regulation::fixture::rows("regbuild 0.2.0", &sha, &"v".repeat(64), 4.0));
        let root = t.0.join("r").display().to_string();
        let mut out = Vec::new();
        assert_eq!(main_with(&s(&["--cli", "regulation-info", "--root", &root]), &mut out), 0);
        let text = String::from_utf8(out).unwrap();
        let c = crate::regulation::content(&t.0.join("r")).unwrap();
        for l in [
            "regulation_builder=regbuild".to_string(),
            format!("package_regulation_sha256={sha}"),
            format!("package_bnd4_sha256={}", c.bnd4_sha256),
            format!("rows_json_core_sha256={}", c.rows_core_sha256),
        ] {
            assert!(text.lines().any(|x| x == l), "{l}\n{text}");
        }
        // rows.json naming another package, or no build at all: refused
        t.write(r"r\build\rows.json", crate::regulation::fixture::rows("build_regulation.py", &"0".repeat(64), &"v".repeat(64), 4.0));
        let mut out = Vec::new();
        assert_eq!(main_with(&s(&["--cli", "regulation-info", "--root", &root]), &mut out), 1);
        let mut out = Vec::new();
        assert_eq!(main_with(&s(&["regulation-info", "--root", &t.0.join("none").display().to_string()]), &mut out), 1);
    }

    #[test]
    fn parses_the_commands() {
        let a = parse(&s(&["--cli", "install", "--mode", "coop", "--root", r"D:\X", "--download-witchy", "--password-stdin"])).unwrap();
        assert_eq!(a.command, "install");
        assert_eq!(a.mode, Some(Mode::Coop));
        assert!(a.download_witchy && a.password_stdin);
        assert_eq!(a.regulation_tool, None);
        assert_eq!(parse(&s(&["install", "--regulation-tool", "witchy"])).unwrap().regulation_tool, Some(Tool::Witchy));
        assert_eq!(parse(&s(&["install", "--regulation-tool", "regbuild"])).unwrap().regulation_tool, Some(Tool::Regbuild));
        assert!(parse(&s(&["install", "--regulation-tool", "oodle"])).is_err());
        assert_eq!(parse(&s(&["regulation-info", "--root", r"D:\Mods\MW2inER"])).unwrap().command, "regulation-info");
        let d = parse(&s(&["dry-run"])).unwrap();
        assert!(d.command == "install" && d.dry_run);
        let p = parse(&s(&["play", "coop"])).unwrap();
        assert_eq!((p.command.as_str(), p.mode), ("play", Some(Mode::Coop)));
        assert_eq!(parse(&s(&["launch", "solo"])).unwrap().command, "play");
        assert!(parse(&s(&["play"])).is_err());
        assert!(parse(&s(&["play", "pvp"])).is_err());
        assert!(parse(&s(&["install", "--root"])).is_err());
        assert!(parse(&s(&["frobnicate"])).is_err());
        assert!(parse(&s(&["status", "--bogus"])).is_err());
        assert_eq!(parse(&s(&["--help"])).unwrap().command, "help");
    }

    #[test]
    fn front_end_choice() {
        use FrontEnd::*;
        let f = |v: &[&str], own: bool| front_end(&s(v), own);
        // double-click / the Setup shortcut / options only: the window
        assert_eq!(f(&[], true), Wizard);
        assert_eq!(f(&[], false), Wizard);
        assert_eq!(f(&["--payload", r"D:\rel", "--root", r"D:\x"], false), Wizard);
        assert_eq!(f(&["--gui", "install"], false), Wizard);
        // --cli always wins, also for the shortcut commands and from a shortcut
        assert_eq!(f(&["--cli"], true), Cli);
        assert_eq!(f(&["--cli", "play", "coop"], true), Cli);
        assert_eq!(f(&["--cli", "uninstall"], true), Cli);
        assert_eq!(f(&["--gui", "--cli"], false), Cli);
        // the shortcuts and Apps & Features (own console): launcher / uninstall page
        assert_eq!(f(&["play", "solo"], true), Launcher);
        assert_eq!(f(&["play", "coop"], true), Launcher);
        assert_eq!(f(&["uninstall"], true), UninstallWindow);
        // from a terminal (shared console): the console, as before
        assert_eq!(f(&["play", "coop"], false), Cli);
        assert_eq!(f(&["uninstall"], false), Cli);
        assert_eq!(f(&["play", "coop", "--dry-run"], true), Cli);
        assert_eq!(f(&["uninstall", "--yes"], true), Cli);
        assert_eq!(f(&["install", "--root", r"D:\x"], true), Cli);
        assert_eq!(f(&["status"], false), Cli);
        // usage errors are printed by the console front end
        assert_eq!(f(&["frobnicate"], true), Cli);
        assert_eq!(f(&["play"], true), Cli);
    }

    /// `rollback`: refused without a store, a dry run changes nothing, refused while the game runs, then restores.
    #[test]
    fn rollback_command() {
        use crate::guard::Guard;
        let t = crate::util::testutil::TempDir::new("cli-rollback");
        let root = t.0.join("root");
        let g = Guard::default();
        let env = Env::default();
        let a = |extra: &[&str]| parse(&s(&[&["rollback", "--root", &root.display().to_string(), "--no-register"], extra].concat())).unwrap();
        let run = |a: &Args, running: bool| {
            let mut out = Vec::new();
            let code = cmd_rollback(a, &env, &root, &mut out, &|e: &str| running && e == "eldenring.exe");
            (code, String::from_utf8(out).unwrap())
        };
        let mut st = InstallState::new();
        st.release_version = "0.2.0".into();
        st.coop_id = "aaaaaaaa".into();
        st.save(&root, &g).unwrap();
        t.write(r"root\build\natives\mw2er.dll", b"old");
        assert!(run(&a(&["--yes"]), false).1.contains("nothing to roll back"));
        let mut store = rollback::Store::begin(&root, &st, "0.2.1", &g).unwrap();
        store.save_file(&root, "build/natives/mw2er.dll", &g).unwrap();
        t.write(r"root\build\natives\mw2er.dll", b"new");
        let mut st2 = st.clone();
        st2.release_version = "0.2.1".into();
        st2.coop_id = "bbbbbbbb".into();
        st2.save(&root, &g).unwrap();
        store.finish("bbbbbbbb", &g).unwrap();
        let (code, text) = run(&a(&["--dry-run"]), false);
        assert_eq!(code, 0);
        assert!(text.contains("from 0.2.1 to 0.2.0") && text.contains("co-op id: bbbbbbbb -> aaaaaaaa") && text.contains("both players"), "{text}");
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"new");
        let (code, text) = run(&a(&["--yes"]), true);
        assert_eq!(code, 1);
        assert!(text.contains("Elden Ring is running"), "{text}");
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"new");
        let (code, text) = run(&a(&["--yes"]), false);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("ROLLED BACK to 0.2.0"));
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"old");
        assert_eq!(InstallState::load(&root).unwrap().coop_id, "aaaaaaaa");
        // check-update on a release without update_repo: no request, a plain answer
        crate::release::fixture::package(&t);
        let mut out = Vec::new();
        assert_eq!(main_with(&s(&["--cli", "check-update", "--payload", &t.0.join("pkg").display().to_string(), "--root", &root.display().to_string()]), &mut out), 0);
        assert!(String::from_utf8(out).unwrap().contains("no update check"));
        assert_eq!(parse(&s(&["rollback", "--yes"])).unwrap().command, "rollback");
        assert_eq!(front_end(&s(&["rollback"]), true), FrontEnd::Cli);
    }

    #[test]
    fn usage_exit_codes() {
        let mut o = Vec::new();
        assert_eq!(main_with(&s(&["--cli"]), &mut o), 2);
        assert_eq!(main_with(&s(&["help"]), &mut o), 0);
        assert_eq!(main_with(&s(&["nope"]), &mut o), 2);
        assert!(String::from_utf8_lossy(&o).contains("usage:"));
    }
}
