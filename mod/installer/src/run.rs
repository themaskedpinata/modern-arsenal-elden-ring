//! The install session: resolve the inputs (detection + the player's choices + the remembered state), build the
//! plan, and execute it step by step with [`Progress`] events (the wizard reads them from a channel, `--cli`
//! prints them). Every write goes through the [`Guard`].
//!
//! The pipeline (docs/INSTALLER_DESIGN.md 2.3 page 6; setup.ps1's steps): preflight, save backup (read-only on the
//! source), payload (the release files, hash-checked, + the release toml policy of [`friend_toml`]), Python runtime
//! (the bundled embeddable CPython, sha256-pinned), Seamless copy + ini (co-op), WitchyBND (only on the P2 fallback:
//! pinned zip or the pinned download; Offline / Flexible=false / Paramdex upgrader as setup.ps1), MW2 dumps
//! (Unlinker, pinned), the 7 converters, verify converted, regulation (P2: the in-process Rust builder by default,
//! [`regulation`]; `build_regulation.py` + WitchyBND on the fallback), verify all (the host-manifest check: Rust
//! re-hash + `coop_manifest.py`; the regulation by content when the builder is not the release's), cleanup, me3
//! profiles (solo + co-op), finish.
//!
//! - Skip-if-unchanged: [`plan::build`] decides from the recorded step keys.
//! - Resume: a step's record is dropped (and saved) before it runs and written only when it passed; the state is
//!   saved after every step, so an interrupted, failed or cancelled install reruns exactly what is unfinished.
//! - Cancel: the flag passed to [`Session::install`] is checked between steps, between copied files, between
//!   download reads and every 50 ms while a tool runs (the tool's whole process tree is killed).
//! - Logs: `<root>\logs\install\install.log` (every event of every session, appended) and one
//!   `<root>\logs\install\<tool>.log` per tool run. The co-op password is never logged: it goes only into the mod's
//!   own ersc_settings.ini copy, and every event is passed through [`Secret::redact`] as well.
//! - External programs go through [`Session::runner`] ([`system_runner`] = [`proc::run_cancellable`]); tests put
//!   fake tools there.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::detect::{self, Check, Env, Facts, Level, Overrides};
use crate::guard::{self, Guard};
use crate::plan::{self, Decision, Plan, PlanInputs, StepId};
use crate::proc;
use crate::regulation::{self, Tool};
use crate::release::{self, Only, Release};
use crate::seamless::{self, Secret};
use crate::state::{InstallState, Mode};
use crate::util;
use crate::rollback::{self, Store};
use crate::{download, friend_toml, launch, payload, toml_merge, update, witchy};

/// Bytes a full build needs while it runs (setup.ps1 asks for 6 GB; docs 1.2: peak ~2.7 GB + WitchyBND).
pub const BUILD_BYTES: u64 = 6 << 30;
pub const APP_KEY: &str = r"Software\MW2inER";
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MW2inER";
pub const INSTALLED_EXE: &str = "MW2inER.exe";
/// The Start-menu folder of the shortcuts (P4: the display name; releases before 0.3.0 used a
/// [`crate::LEGACY_DISPLAY_NAMES`] name, which an update replaces and an uninstall removes).
pub const SHORTCUT_FOLDER: &str = crate::DISPLAY_NAME;

/// The solo, co-op and setup shortcut file names for a display name.
pub fn shortcut_names(name: &str) -> [String; 3] {
    [format!("{name}.lnk"), format!("{name} - Co-op.lnk"), format!("{name} - Setup.lnk")]
}

/// A shortcut file an older release made under a [`crate::LEGACY_DISPLAY_NAMES`] name.
pub fn is_legacy_shortcut(p: &Path) -> bool {
    let Some(f) = p.file_name().map(|f| f.to_string_lossy().to_lowercase()) else { return false };
    crate::LEGACY_DISPLAY_NAMES.iter().any(|n| shortcut_names(n).iter().any(|s| s.to_lowercase() == f))
}

/// A Start-menu folder of an older release (or the current one).
pub fn is_legacy_folder(d: &Path) -> bool {
    d.file_name().is_some_and(|f| crate::LEGACY_DISPLAY_NAMES.iter().chain([&SHORTCUT_FOLDER]).any(|n| f.to_string_lossy().eq_ignore_ascii_case(n)))
}

/// The folder a legacy shortcut was asked for: the Desktop itself, or the Start-menu Programs folder above the
/// old named folder.
fn legacy_shortcut_base(p: &Path) -> Option<PathBuf> {
    let parent = p.parent()?;
    if is_legacy_folder(parent) { parent.parent().map(Path::to_path_buf) } else { Some(parent.to_path_buf()) }
}
/// Root-relative folder of the installer's logs (the mod DLL writes `logs\` itself).
pub const LOG_DIR: &str = r"logs\install";
/// The session log inside [`LOG_DIR`].
pub const SESSION_LOG: &str = "install.log";
/// The manifest `coop_manifest.py verify` gets when the regulation is checked by content (root-relative).
pub const VERIFY_MANIFEST: &str = r"state\verify_manifest.json";
/// friend_dump.py prints one `OK  ` / `FAIL` line per Unlinker run (17), per iwd folder (2) and for the HUD copy.
pub const DUMP_LINES: usize = 20;
/// P3: the release's mw2er.toml as installed last time (root-relative): the base of the next settings merge, so a
/// default the player never changed follows the new release.
pub const RELEASE_TOML: &str = r"state\release_mw2er.toml";
/// P3: the player's mw2er.toml before an update / repair changed it (root-relative folder; kept on uninstall).
pub const SETTINGS_BACKUPS: &str = r"backups\settings";

/// The running-game refusal (an update / repair / rollback replaces files the game has open): a red `er.running`
/// row while `eldenring.exe` runs. `is_running` = [`crate::win::is_running`] (tests: a fake).
pub fn running_game_check(is_running: impl Fn(&str) -> bool) -> Option<Check> {
    is_running("eldenring.exe").then(|| Check {
        id: "er.running",
        level: Level::Fail,
        msg: "Elden Ring is running: close it first (the mod files are in use)".into(),
        fix: None,
    })
}

/// The settings summary's note when no merge base exists (see [`merge_base`]).
pub const NO_BASE: &str = "; the previous release's defaults are not known for this install: every value that differs from the new release is kept as yours";

/// P3: the base of the settings merge ([`toml_merge::merge`]): the release file recorded at the last install
/// ([`RELEASE_TOML`]). An install made before P3 has none: then the player's own file is the base when it is still
/// exactly the installed release's (`installed_sha` = its `payload["mw2er.toml"]`: nothing was changed, so every
/// value follows the new release). None = unknown (every differing hot value counts as the player's).
pub fn merge_base(root: &Path, player: &str, installed_sha: Option<&str>) -> Option<String> {
    if let Ok(b) = std::fs::read_to_string(root.join(RELEASE_TOML)) {
        return Some(b);
    }
    let want = installed_sha.filter(|h| util::is_sha256(h))?;
    let have = util::sha256_file(&root.join("mw2er.toml")).ok()?;
    have.eq_ignore_ascii_case(want).then(|| player.to_string())
}

pub fn log_dir(root: &Path) -> PathBuf {
    root.join(LOG_DIR)
}

/// Events for the front ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// The plan: (step, runs) in order.
    Plan(Vec<(StepId, bool)>),
    StepStart { id: StepId, index: usize, total: usize },
    Line { id: StepId, text: String },
    /// 0..1 inside a step (downloads, Unlinker runs, converters).
    Fraction { id: StepId, value: f32 },
    StepDone { id: StepId, ok: bool, secs: f64, msg: String },
    Skipped { id: StepId, reason: String },
    Finished { ok: bool, msg: String },
}

impl Progress {
    /// One log line (`STEP` / `LINE` / `OK  ` / `FAIL` / `SKIP` / `END `); None for fractions.
    pub fn log_line(&self) -> Option<String> {
        Some(match self {
            Progress::Plan(v) => format!(
                "PLAN {}",
                v.iter().map(|(id, r)| format!("{}{}", if *r { "" } else { "-" }, id.as_str())).collect::<Vec<_>>().join(" ")
            ),
            Progress::StepStart { id, index, total } => format!("STEP {}/{} {} - {}", index + 1, total, id.as_str(), id.title()),
            Progress::Line { id, text } => format!("     [{}] {text}", id.as_str()),
            Progress::Fraction { .. } => return None,
            Progress::StepDone { id, ok, secs, msg } => format!("{} {} ({secs:.1} s): {msg}", if *ok { "OK  " } else { "FAIL" }, id.as_str()),
            Progress::Skipped { id, reason } => format!("SKIP {}: {reason}", id.as_str()),
            Progress::Finished { msg, .. } => format!("END  {msg}"),
        })
    }

    fn redacted(self, pw: Option<&Secret>) -> Progress {
        let Some(pw) = pw else { return self };
        match self {
            Progress::Line { id, text } => Progress::Line { id, text: pw.redact(&text) },
            Progress::StepDone { id, ok, secs, msg } => Progress::StepDone { id, ok, secs, msg: pw.redact(&msg) },
            Progress::Skipped { id, reason } => Progress::Skipped { id, reason: pw.redact(&reason) },
            Progress::Finished { ok, msg } => Progress::Finished { ok, msg: pw.redact(&msg) },
            other => other,
        }
    }
}

/// Runs one external program: (job, cancel flag, line callback) -> result.
pub type Runner = Arc<dyn Fn(&proc::Job, &AtomicBool, &mut dyn FnMut(&str)) -> std::io::Result<proc::JobResult> + Send + Sync>;

/// The real runner: [`proc::run_cancellable`] (no window, timeout, job object, cancel).
pub fn system_runner() -> Runner {
    Arc::new(|job: &proc::Job, cancel: &AtomicBool, line: &mut dyn FnMut(&str)| proc::run_cancellable(job, Some(cancel), line))
}

/// Where the shortcuts go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShortcutDirs {
    pub desktop: Option<PathBuf>,
    pub start_menu: Option<PathBuf>,
}

/// What the player asked for.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub root: PathBuf,
    /// The release files: a folder or a zip (staged by [`payload::resolve`]).
    pub payload: PathBuf,
    pub mode: Mode,
    pub over: Overrides,
    /// The pinned WitchyBND download is allowed (the player clicked "Download" / `--download-witchy`).
    pub allow_download: bool,
    /// The regulation builder chosen by hand (`--regulation-tool`, the wizard's Advanced box); None = the builder the
    /// release was made with ([`Tool::of_release`]: regbuild for P2 releases).
    pub regulation_tool: Option<Tool>,
    /// The official CPython embeddable zip chosen by hand (else looked for in the payload).
    pub python_zip: Option<PathBuf>,
    pub keep_dumps: bool,
    pub force: bool,
    pub shortcuts: ShortcutDirs,
    /// Write `HKCU\Software\MW2inER` and the Apps & Features entry.
    pub register: bool,
    /// Copy this exe into the root as MW2inER.exe.
    pub self_exe: Option<PathBuf>,
    /// `%APPDATA%\EldenRing` (None = no save backup).
    pub save_root: Option<PathBuf>,
    /// Default `<root>\backups`.
    pub backup_dir: Option<PathBuf>,
    /// Probe Python's version (runs python; off in unit tests).
    pub probe_python: bool,
}

/// A prepared session: facts, rows, the guard and the remembered state.
pub struct Session {
    pub env: Env,
    pub req: Request,
    pub release: Release,
    pub facts: Facts,
    pub checks: Vec<Check>,
    pub guard: Guard,
    pub state: InstallState,
    /// The pinned CPython embeddable zip (sha256 == [`payload::PYTHON_EMBED_SHA256`]) the runtime step unpacks.
    pub python_zip: Option<PathBuf>,
    /// Runs the external programs (tests: fake tools).
    pub runner: Runner,
    /// The regulation builder of this session (the request's, else the release's).
    pub regulation_tool: Tool,
    /// The in-process Rust regulation build ([`regulation::system_builder`]; tests: a fake).
    pub regbuilder: regulation::Builder,
    /// P3: the install record as it was when the session was prepared (the old version of an update).
    pub prev_state: InstallState,
    /// P3: this run updates the install from that version (None = a fresh install or a repair of the same version).
    pub update_from: Option<String>,
    /// P3: the rollback store of this update (opened when the install starts).
    pub store: Option<Store>,
    /// Asks whether a process runs ([`crate::win::is_running`]; tests: a fake). The running-game refusal is checked
    /// again when the install starts (the window may have been open for a while: the game started from the Play
    /// shortcut meanwhile).
    pub is_running: fn(&str) -> bool,
    /// The checks step passed in this run: only then does the record take the new version (a refused update leaves
    /// the old record as it was, apart from its last result).
    past_preflight: bool,
}

#[derive(Debug)]
pub struct InstallError {
    pub step: Option<StepId>,
    pub msg: String,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.step {
            Some(s) => write!(f, "{}: {}", s.as_str(), self.msg),
            None => f.write_str(&self.msg),
        }
    }
}

impl InstallError {
    pub fn cancelled(&self) -> bool {
        self.msg == CANCELLED
    }
}

const CANCELLED: &str = "cancelled";

fn err(step: StepId, msg: impl Into<String>) -> InstallError {
    InstallError { step: Some(step), msg: msg.into() }
}

/// Without a zip, an unpacked copy or the player's permission, the WitchyBND row is red (the download needs a click).
fn witchy_row(checks: &mut [Check], facts: &Facts, allow_download: bool) {
    if facts.witchy.zip.is_none() && !facts.witchy.unpacked_ok && !allow_download {
        for c in checks.iter_mut().filter(|c| c.id == "witchy") {
            c.level = Level::Fail;
            c.msg = "WitchyBND 3.0.1.0 zip not found: allow the pinned download (--download-witchy / the Download box) or choose the zip (--witchy-zip / Choose...)".into();
        }
    }
}

/// The rows of the regulation builder: WitchyBND's (only with the WitchyBND fallback) and, when the builder is not
/// the one the release was made with, a `regulation.tool` row (yellow: checked by content; red: the release has no
/// content pins).
fn tool_rows(facts: &Facts, release: &Release, tool: Tool, allow_download: bool) -> Vec<Check> {
    let mut rows: Vec<Check> = if tool == Tool::Witchy {
        detect::evaluate(facts, &release.pins, true).into_iter().filter(|c| c.id == "witchy").collect()
    } else {
        Vec::new()
    };
    witchy_row(&mut rows, facts, allow_download);
    let made_with = Tool::of_release(&release.pins);
    if tool != made_with {
        let pins = &release.pins;
        rows.push(if util::is_sha256(&pins.package_bnd4_sha256) && util::is_sha256(&pins.rows_json_core_sha256) {
            Check {
                id: "regulation.tool",
                level: Level::Warn,
                msg: format!(
                    "Regulation builder: {} instead of this release's {}. The params are checked against the release by content; the regulation file and the co-op id differ from the host's (both players must use the same builder)",
                    tool.label(),
                    made_with.label()
                ),
                fix: None,
            }
        } else {
            Check {
                id: "regulation.tool",
                level: Level::Fail,
                msg: format!(
                    "This release was built with {} and has no builder-independent regulation pins: use --regulation-tool {} (or leave the builder at its default)",
                    made_with.label(),
                    made_with.as_str()
                ),
                fix: None,
            }
        });
    }
    rows
}

/// The session log: lines wait in memory until `<root>\logs\install` exists (the preflight creates it).
struct SessionLog {
    path: PathBuf,
    guard: Guard,
    file: Option<std::fs::File>,
    pending: Vec<String>,
}

impl SessionLog {
    fn new(root: &Path, guard: Guard) -> SessionLog {
        SessionLog { path: log_dir(root).join(SESSION_LOG), guard, file: None, pending: Vec::new() }
    }

    fn line(&mut self, l: String) {
        self.pending.push(format!("{} {l}", util::utc_stamp()));
        if self.file.is_none()
            && self.path.parent().is_some_and(Path::is_dir)
            && self.guard.check(&self.path).is_ok()
        {
            self.file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        if let Some(f) = self.file.as_mut() {
            for p in self.pending.drain(..) {
                let _ = writeln!(f, "{p}");
            }
        }
    }
}

impl Session {
    /// Loads the release (a folder or a zip), the state (remembered paths fill the overrides the player left
    /// empty), detects everything and evaluates the rows. Nothing is written (a zip payload is unpacked into
    /// `%TEMP%`).
    pub fn prepare(env: Env, mut req: Request) -> Result<Session, String> {
        req.payload = payload::resolve(&req.payload)?;
        let release = Release::load(&req.payload).map_err(|e| format!("no release in {}: {e}", req.payload.display()))?;
        let state = InstallState::load_or_new(&req.root);
        let o = &mut req.over;
        let p = &state.paths;
        let keep = |cur: &mut Option<PathBuf>, rem: &Option<PathBuf>| {
            if cur.is_none() {
                *cur = rem.clone().filter(|r| r.exists());
            }
        };
        keep(&mut o.er_game, &p.er_game);
        keep(&mut o.mw2_dir, &p.mw2_dir);
        keep(&mut o.me3, &p.me3);
        keep(&mut o.seamless, &p.seamless_source);
        keep(&mut o.witchy_zip, &p.witchy_zip);
        // Python is not taken from the record: detection finds the bundled runtime / the PC's Python again, and a
        // remembered system Python must not hide the bundled one of a newer release.
        let mut facts = detect::Gather {
            env: &env,
            release: &release,
            root: &req.root,
            payload: Some(&req.payload),
            over: &req.over,
            coop: req.mode.coop(),
            need_bytes: BUILD_BYTES,
        }
        .run();
        if req.probe_python
            && let Some(py) = &facts.python.exe
        {
            facts.python.version = detect::probe_python(py);
        }
        let regulation_tool = req.regulation_tool.unwrap_or_else(|| Tool::of_release(&release.pins));
        let mut checks = detect::evaluate(&facts, &release.pins, false);
        let pos = checks.iter().position(|c| c.id == "vcredist" || c.id == "disk").unwrap_or(checks.len());
        for (k, c) in tool_rows(&facts, &release, regulation_tool, req.allow_download).into_iter().enumerate() {
            checks.insert(pos + k, c);
        }
        // the bundled CPython: a pinned zip (payload or chosen) or an unpacked runtime\python (payload or root)
        let mut python_zip = None;
        match payload::find_python_zip(req.python_zip.as_deref(), &req.root, &req.payload) {
            Some((z, true)) => python_zip = Some(z),
            Some((z, false)) => checks.push(Check {
                id: "python.bundled",
                level: Level::Fail,
                msg: format!("{} is not the official CPython embeddable package {} (sha256 differs): refused", z.display(), payload::PYTHON_EMBED_NAME),
                fix: Some(detect::Fix::Browse(detect::Input::Python)),
            }),
            None => {}
        }
        if req.over.python.is_none() && python_zip.is_some() && !facts.python.bundled {
            for c in checks.iter_mut().filter(|c| c.id == "python") {
                c.level = Level::Ok;
                c.msg = format!("Python: the bundled CPython 3.14.7 ({}, sha256-pinned) goes into runtime\\python", payload::PYTHON_EMBED_NAME);
                c.fix = None;
            }
        }
        let steam = env.steam();
        let mut guard = Guard::new(
            facts.er.game.as_deref(),
            facts.mw2.dir.as_deref(),
            &steam.steamapps_dirs(),
            steam.steam_path.as_deref(),
            env.appdata.as_deref(),
        );
        // the player's own Seamless (its SeamlessCoop folder, or the zip) is only ever read; only the copy inside a
        // recorded install is ours
        let own_copy = req.root.join(seamless::COPY_DIR);
        let recorded = InstallState::path(&req.root).is_file();
        match &facts.seamless.found {
            Some(seamless::Found::Dll(d)) => {
                if let Some(dir) = d.parent()
                    && !(recorded && util::same_path(dir, &own_copy))
                {
                    guard.forbid(dir);
                }
            }
            Some(seamless::Found::Zip(z)) => guard.forbid(z),
            None => {}
        }
        let windir = env.system32.as_ref().and_then(|s| s.parent().map(Path::to_path_buf));
        if let Err(e) = guard::check_root(&req.root, &guard, &guard::program_files_dirs(), windir.as_deref()) {
            checks.push(Check { id: "root", level: Level::Fail, msg: e, fix: Some(detect::Fix::Browse(detect::Input::Root)) });
        } else if let Err(e) = guard::check_root_contents(&req.root, &req.payload) {
            checks.push(Check { id: "root", level: Level::Fail, msg: e, fix: Some(detect::Fix::Browse(detect::Input::Root)) });
        } else if !util::same_path(&req.root, &req.payload) && util::is_under(&req.root, &req.payload) {
            checks.push(Check { id: "root", level: Level::Fail, msg: "the install folder cannot be inside the release folder".into(), fix: None });
        }
        checks.extend(running_game_check(crate::win::is_running));
        let from = if state.update_from.is_empty() { state.release_version.clone() } else { state.update_from.clone() };
        let update_from = (recorded && !from.is_empty() && from != release.version && !util::same_path(&req.root, &req.payload)).then_some(from);
        Ok(Session {
            env,
            req,
            release,
            facts,
            checks,
            guard,
            python_zip,
            runner: system_runner(),
            regulation_tool,
            regbuilder: regulation::system_builder(),
            prev_state: state.clone(),
            update_from,
            store: None,
            is_running: crate::win::is_running,
            past_preflight: false,
            state,
        })
    }

    /// Re-evaluates the regulation builder's rows (`witchy`, `regulation.tool`) after a choice changed.
    fn refresh_tool_rows(&mut self) {
        self.checks.retain(|c| c.id != "witchy" && c.id != "regulation.tool");
        let pos = self.checks.iter().position(|c| c.id == "vcredist" || c.id == "disk").unwrap_or(self.checks.len());
        for (k, c) in tool_rows(&self.facts, &self.release, self.regulation_tool, self.req.allow_download).into_iter().enumerate() {
            self.checks.insert(pos + k, c);
        }
    }

    /// The player allowed (or withdrew) the pinned WitchyBND download after the detection (the wizard's checkbox,
    /// the CLI's question): updates the request and the `witchy` row (shown only with the WitchyBND fallback).
    pub fn set_allow_download(&mut self, allow: bool) {
        self.req.allow_download = allow;
        self.refresh_tool_rows();
    }

    /// The regulation builder chosen after the detection (the wizard's Advanced box; None = the release's).
    pub fn set_regulation_tool(&mut self, tool: Option<Tool>) {
        self.req.regulation_tool = tool;
        self.regulation_tool = tool.unwrap_or_else(|| Tool::of_release(&self.release.pins));
        self.refresh_tool_rows();
    }

    /// The co-op id of this install: the release's, or with a builder other than the release's the id derived from
    /// the regulation file actually built ([`regulation::coop_id`]).
    pub fn coop_id(&self) -> String {
        let sha = util::sha256_file(&self.req.root.join(regulation::PACKAGE)).ok();
        regulation::coop_id(&self.release, self.regulation_tool, sha.as_deref())
    }

    /// Re-checks the running game (the `er.running` row) now: the detection's answer may be stale.
    pub fn refresh_running(&mut self) {
        self.checks.retain(|c| c.id != "er.running");
        self.checks.extend(running_game_check(self.is_running));
    }

    pub fn blocking(&self) -> Vec<&Check> {
        self.checks.iter().filter(|c| c.level == Level::Fail).collect()
    }

    /// The Python the build will run: the chosen / found one, else the bundled runtime the runtime step unpacks.
    fn planned_python(&self) -> Option<PathBuf> {
        self.facts.python.exe.clone().or_else(|| self.python_zip.as_ref().map(|_| payload::runtime_python(&self.req.root)))
    }

    /// The plan (None while a required input is missing).
    pub fn plan(&self) -> Option<Plan> {
        let f = &self.facts;
        let er = f.er.game.as_deref()?;
        let mw2 = f.mw2.dir.as_deref()?;
        let me3 = f.me3.exe.as_deref()?;
        let py = self.planned_python()?;
        let seam = match &f.seamless.found {
            Some(seamless::Found::Dll(p)) | Some(seamless::Found::Zip(p)) => Some(p.as_path()),
            None => None,
        };
        let i = PlanInputs {
            root: &self.req.root,
            payload: &self.req.payload,
            release: &self.release,
            mode: self.req.mode,
            er_game: er,
            mw2_dir: mw2,
            me3,
            python: &py,
            seamless_source: seam,
            witchy_zip: f.witchy.zip.as_deref().filter(|_| f.witchy.zip_ok),
            regulation_tool: self.regulation_tool,
            mw2_buildid: f.mw2.buildid.as_deref().unwrap_or(""),
            iw4sp_sha256: f.mw2.iw4sp_sha256.as_deref().unwrap_or(""),
            vanilla_regulation_sha256: f.er.regulation_sha256.as_deref().unwrap_or(""),
            keep_dumps: self.req.keep_dumps,
            force: self.req.force,
        };
        Some(plan::build(&i, &self.state))
    }

    /// Runs the plan. `password`: the co-op password typed this time (None = keep the one in the copy's ini).
    /// `cancel`: set it (from any thread) to stop; the finished steps are kept and the next run resumes.
    pub fn install(&mut self, password: Option<&Secret>, cancel: &AtomicBool, sink: &mut dyn FnMut(Progress)) -> Result<(), InstallError> {
        // live, not the detection's answer: an update replaces files the game has open
        self.refresh_running();
        let mut log = SessionLog::new(&self.req.root, self.guard.clone());
        log.line(format!(
            "==== MW2inER setup {} : install release {} ({}) root {} payload {}",
            env!("CARGO_PKG_VERSION"),
            self.release.version,
            if self.req.mode.coop() { "co-op" } else { "solo" },
            self.req.root.display(),
            self.req.payload.display()
        ));
        if !self.state.last_result.is_empty() {
            log.line(format!("     previous result: {}", self.state.last_result));
        }
        for c in &self.checks {
            log.line(format!("     check {:?} {}: {}", c.level, c.id, c.msg));
        }
        let mut out = |e: Progress| {
            let e = e.redacted(password);
            if let Some(l) = e.log_line() {
                log.line(l);
            }
            sink(e);
        };
        let blocking: Vec<String> = self.blocking().iter().map(|c| c.msg.clone()).collect();
        let Some(plan) = self.plan() else {
            let e = InstallError {
                step: Some(StepId::Preflight),
                msg: if blocking.is_empty() { "a required game or tool was not found".into() } else { blocking.join("; ") },
            };
            out(Progress::Finished { ok: false, msg: format!("INSTALL FAILED: {e}") });
            return Err(e);
        };
        if let Some(u) = self.update_summary(&plan) {
            for l in u.lines() {
                out(Progress::Line { id: StepId::Preflight, text: l });
            }
        }
        // P3: an update keeps what it replaces in the rollback store (opened before the first file changes)
        if let Some(from) = self.update_from.clone()
            && blocking.is_empty()
            && self.store.is_none()
        {
            let mut old = self.prev_state.clone();
            if old.release_version != from {
                // resumed without a store (the first session stopped before it was made): the files are still X's
                old.release_version = from.clone();
            }
            old.update_from.clear();
            match Store::begin(&self.req.root, &old, &self.release.version, &self.guard) {
                Ok(st) => self.store = Some(st),
                Err(e) => {
                    let e = err(StepId::Preflight, format!("cannot keep the files of {from} for a rollback: {e}"));
                    out(Progress::Finished { ok: false, msg: format!("INSTALL FAILED: {e}") });
                    return Err(e);
                }
            }
        }
        out(Progress::Plan(plan.steps.iter().map(|s| (s.id, s.decision.runs())).collect()));
        let total = plan.steps.len();
        let res = (|| {
            for (index, step) in plan.steps.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    return Err(err(step.id, CANCELLED));
                }
                if let Decision::Skip(r) = &step.decision {
                    if step.migrates(&self.state) {
                        // a record of an earlier setup's key format: the same inputs, recorded the current way
                        self.state.record_step(step.id.as_str(), &step.key);
                    }
                    out(Progress::Skipped { id: step.id, reason: r.clone() });
                    continue;
                }
                out(Progress::StepStart { id: step.id, index, total });
                let t0 = std::time::Instant::now();
                let keyed = !step.key.is_empty();
                if keyed {
                    // resume: no record while it runs, so an interruption reruns it
                    self.state.forget_step(step.id.as_str());
                }
                // what a crash / kill / power cut leaves in the record (the finish step writes PASS)
                self.state.last_result = format!("INTERRUPTED during {} (started {})", step.id.as_str(), util::utc_stamp());
                self.save_state();
                let r = (|| {
                    for c in &step.clean {
                        let p = self.req.root.join(c.replace('/', "\\"));
                        // an update moves the old build output into the rollback store instead of deleting it
                        if let Some(st) = self.store.as_ref().filter(|_| rollback::MOVE_ON_UPDATE.contains(&c.as_str())) {
                            if st.move_away(&self.req.root, c, &self.guard).map_err(|e| err(step.id, e.to_string()))? {
                                out(Progress::Line { id: step.id, text: format!("kept the previous {c} for a rollback") });
                            }
                            continue;
                        }
                        self.guard.remove_dir_all(&p).or_else(|_| self.guard.remove_file(&p)).map_err(|e| err(step.id, e.to_string()))?;
                    }
                    self.run_step(step.id, password, &plan, cancel, &mut out)
                })();
                let secs = t0.elapsed().as_secs_f64();
                match r {
                    Ok(msg) => {
                        if step.id == StepId::Preflight {
                            self.past_preflight = true;
                        }
                        if keyed {
                            self.state.record_step(step.id.as_str(), &step.key);
                        }
                        self.save_state();
                        out(Progress::StepDone { id: step.id, ok: true, secs, msg });
                    }
                    Err(mut e) => {
                        let stopped = cancel.load(Ordering::SeqCst) || e.cancelled();
                        self.state.forget_step(step.id.as_str());
                        if stopped {
                            e.msg = CANCELLED.into();
                        } else {
                            for s in plan::invalidated_by(step.id) {
                                self.state.forget_step(s.as_str());
                            }
                        }
                        self.state.last_result =
                            if stopped { format!("CANCELLED at {} {}", step.id.as_str(), util::utc_stamp()) } else { format!("FAIL {e}") };
                        self.save_state();
                        out(Progress::StepDone { id: step.id, ok: false, secs, msg: e.msg.clone() });
                        return Err(e);
                    }
                }
            }
            Ok(())
        })();
        if res.is_ok() {
            let id = self.coop_id();
            if let Some(st) = self.store.as_mut() {
                let _ = st.finish(&id, &self.guard);
            }
            self.update_from = None;
            self.save_state();
        }
        match &res {
            Ok(()) => out(Progress::Finished { ok: true, msg: format!("INSTALL PASSED: co-op id {}", self.coop_id()) }),
            Err(e) if e.cancelled() => out(Progress::Finished {
                ok: false,
                msg: format!(
                    "INSTALL CANCELLED during {}: run the setup again to continue (finished steps are kept)",
                    e.step.map(StepId::as_str).unwrap_or("?")
                ),
            }),
            Err(e) => out(Progress::Finished { ok: false, msg: format!("INSTALL FAILED: {e} (logs: {})", log_dir(&self.req.root).display()) }),
        }
        res
    }

    /// Saves `state\install.json` (only once the root exists: a refused preflight writes nothing).
    fn save_state(&mut self) {
        if !self.req.root.is_dir() {
            return;
        }
        if !self.past_preflight {
            // refused at the checks: the old record stays (only its last result changes)
            if InstallState::path(&self.req.root).is_file() {
                let mut old = self.prev_state.clone();
                old.last_result = self.state.last_result.clone();
                let _ = old.save(&self.req.root, &self.guard);
            }
            return;
        }
        self.state.release_version = self.release.version.clone();
        self.state.update_from = self.update_from.clone().unwrap_or_default();
        self.state.mode = self.req.mode;
        let f = &self.facts;
        let p = &mut self.state.paths;
        p.er_game = f.er.game.clone();
        p.mw2_dir = f.mw2.dir.clone();
        p.me3 = f.me3.exe.clone();
        p.python = f.python.exe.clone();
        p.payload = Some(self.req.payload.clone());
        if let Some(seamless::Found::Dll(x) | seamless::Found::Zip(x)) = &f.seamless.found {
            p.seamless_source = Some(x.clone());
        }
        if f.witchy.zip_ok {
            p.witchy_zip = f.witchy.zip.clone();
        }
        if self.state.installed_utc.is_empty() {
            self.state.installed_utc = util::utc_stamp();
        }
        self.state.updated_utc = util::utc_stamp();
        let _ = self.state.save(&self.req.root, &self.guard);
    }

    fn log_dir(&self) -> PathBuf {
        log_dir(&self.req.root)
    }

    fn py_env(&self) -> Vec<(String, String)> {
        let f = &self.facts;
        let s = |p: Option<&Path>| p.map(|p| p.display().to_string()).unwrap_or_default();
        vec![
            ("MW2ER_IW4SP".into(), s(f.mw2.dir.as_ref().map(|d| d.join("iw4sp.exe")).as_deref())),
            ("MW2ER_GAME_REGULATION".into(), s(f.er.game.as_ref().map(|g| g.join("regulation.bin")).as_deref())),
            ("MW2ER_SNDCONV".into(), self.req.root.join(r"tools\bin\sndconv.exe").display().to_string()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            // live progress lines through the pipe (stdout is block-buffered otherwise); stdio only, files unchanged
            ("PYTHONUNBUFFERED".into(), "1".into()),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn python(
        &self,
        id: StepId,
        name: &str,
        script: &str,
        args: &[String],
        minutes: u64,
        cancel: &AtomicBool,
        sink: &mut dyn FnMut(Progress),
    ) -> Result<String, InstallError> {
        let py = self.facts.python.exe.clone().ok_or_else(|| err(id, "no Python"))?;
        let script = self.req.root.join(r"tools\scripts").join(script);
        let mut job = proc::python_job(name, &py, &script, args, &self.py_env(), &self.req.root, minutes, &self.log_dir());
        job.args.insert(1, "-B".into());
        self.guard.check(&job.log).map_err(|e| err(id, e.to_string()))?;
        let r = (self.runner)(&job, cancel, &mut |l: &str| sink(Progress::Line { id, text: l.to_string() }))
            .map_err(|e| err(id, format!("cannot start {}: {e}", py.display())))?;
        if r.cancelled || cancel.load(Ordering::SeqCst) {
            Err(err(id, CANCELLED))
        } else if r.ok() {
            Ok(format!("{name} ({:.0} s): {}", r.secs, r.last_line))
        } else if r.timed_out {
            Err(err(id, format!("{name} timed out after {minutes} min ({LOG_DIR}\\{name}.log)")))
        } else {
            Err(err(id, format!("{name} exit {:?}: {} ({LOG_DIR}\\{name}.log)", r.code, r.last_line)))
        }
    }

    /// The host-manifest check: the Rust re-hash of the plain entries AND `coop_manifest.py verify` must pass. With a
    /// regulation builder other than the release's, the two regulation entries are checked by content instead
    /// ([`regulation::check_content`]) and `coop_manifest.py` gets the manifest without them ([`VERIFY_MANIFEST`]).
    fn verify(&self, id: StepId, only: Only, cancel: &AtomicBool, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let by_content = only != Only::Converted && regulation::by_content(&self.release.pins, self.regulation_tool);
        let reduced = by_content.then(|| regulation::manifest_without_regulation(&self.release.manifest));
        let rust = release::verify_plain(reduced.as_ref().unwrap_or(&self.release.manifest), &self.req.root, only);
        sink(Progress::Line { id, text: rust.summary() });
        for k in rust.mismatched.iter().map(|k| ("differs", k)).chain(rust.missing.iter().map(|k| ("missing", k))).take(20) {
            sink(Progress::Line { id, text: format!("{} {}", k.0, k.1) });
        }
        let content = by_content.then(|| regulation::check_content(&self.release.pins, &self.req.root));
        match &content {
            Some(Ok(m)) => sink(Progress::Line { id, text: m.clone() }),
            Some(Err(e)) => sink(Progress::Line { id, text: format!("FAIL {e}") }),
            None => {}
        }
        let manifest = match &reduced {
            Some(m) => {
                let p = self.req.root.join(VERIFY_MANIFEST);
                let text = serde_json::to_string_pretty(m).map_err(|e| err(id, e.to_string()))?;
                self.guard.write(&p, text).map_err(|e| err(id, e.to_string()))?;
                p
            }
            None => self.req.root.join(release::HOST_MANIFEST),
        };
        if !manifest.is_file() {
            // a release.json-only payload: coop_manifest.py reads the host-manifest layout
            let text = serde_json::to_string_pretty(&self.release.manifest).map_err(|e| err(id, e.to_string()))?;
            self.guard.write(&manifest, text).map_err(|e| err(id, e.to_string()))?;
        }
        let args = vec![
            "verify".to_string(),
            manifest.display().to_string(),
            self.req.root.display().to_string(),
            "--only".into(),
            only.arg().into(),
        ];
        let py = self.python(id, id.as_str(), "coop_manifest.py", &args, 5, cancel, sink);
        if py.as_ref().is_err_and(|e| e.cancelled()) {
            return Err(err(id, CANCELLED));
        }
        let n = rust.mismatched.len() + rust.missing.len() + usize::from(matches!(content, Some(Err(_))));
        match (py, rust.passed(), content) {
            (Ok(m), true, None | Some(Ok(_))) => Ok(format!("{} | {m}{}", rust.summary(), if by_content { " | regulation checked by content" } else { "" })),
            (r, _, c) => {
                let detail = [r.err().map(|e| e.msg), c.and_then(Result::err)].into_iter().flatten().collect::<Vec<_>>().join("; ");
                Err(err(
                    id,
                    match only {
                        Only::Converted => format!(
                            "your converted files differ from the release ({n} file(s)); most likely your MW2 is a different build. Send {LOG_DIR}\\ to the host. {detail}"
                        ),
                        _ => format!("{n} file(s) differ from the release: co-op needs byte-identical files. Send {LOG_DIR}\\ to the host. {detail}"),
                    },
                ))
            }
        }
    }

    fn run_step(&mut self, id: StepId, password: Option<&Secret>, plan: &Plan, cancel: &AtomicBool, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let root = self.req.root.clone();
        let g = self.guard.clone();
        match id {
            StepId::Preflight => {
                let b = self.blocking();
                if !b.is_empty() {
                    return Err(err(id, b.iter().map(|c| c.msg.as_str()).collect::<Vec<_>>().join("; ")));
                }
                g.create_dir_all(&root).map_err(|e| err(id, e.to_string()))?;
                g.create_dir_all(&self.log_dir()).map_err(|e| err(id, e.to_string()))?;
                let warns = self.checks.iter().filter(|c| c.level == Level::Warn).count();
                for c in self.checks.iter().filter(|c| c.level == Level::Warn) {
                    sink(Progress::Line { id, text: format!("WARN {}", c.msg) });
                }
                Ok(format!("{} checks, {warns} warning(s)", self.checks.len()))
            }
            StepId::SaveBackup => {
                let Some(src) = self.req.save_root.clone() else { return Ok("no save folder given (skipped)".into()) };
                if !src.is_dir() {
                    return Ok(format!("no {} yet (nothing to back up)", src.display()));
                }
                let dir = self.req.backup_dir.clone().unwrap_or_else(|| root.join("backups"));
                let dest = dir.join(format!("EldenRing-{}", util::local_folder_stamp()));
                g.check(&dest).map_err(|e| err(id, e.to_string()))?;
                let n = util::copy_tree_checked(&src, &dest).map_err(|e| err(id, format!("save backup: {e}")))?;
                Ok(format!("{n} save file(s) copied to {} (hash-checked); keep this folder", dest.display()))
            }
            StepId::Payload => self.copy_payload(cancel, sink),
            StepId::Runtime => self.runtime(sink),
            StepId::Seamless => {
                let src = self.facts.seamless.found.clone().ok_or_else(|| err(id, "Seamless Co-op not found"))?;
                let er_parent = self.facts.er.game.as_ref().and_then(|p| p.parent().map(Path::to_path_buf));
                let pin = self.release.pins.ersc_sha256.clone();
                let dll = seamless::import(&src, &root, &pin, er_parent.as_deref(), &g).map_err(|e| err(id, e))?;
                let ini = dll.with_file_name(seamless::INI);
                seamless::set_ini_keys(&ini, password, &g).map_err(|e| err(id, e))?;
                let st = seamless::read_ini_state(&ini).map_err(|e| err(id, e.to_string()))?;
                if !st.keys_ok() {
                    return Err(err(id, "ersc_settings.ini: the two settings did not stick"));
                }
                if !st.password_set {
                    return Err(err(id, "no co-op password: type the password the host gave you (both PCs use the same one)"));
                }
                Ok(format!("Seamless Co-op copied to {}; allow_invaders = 0, save_file_extension = mw2co2, password set (not shown)", dll.parent().unwrap().display()))
            }
            StepId::Witchy => {
                let pins = self.release.pins.clone();
                let app = root.join(detect::WITCHY_APP);
                let ready = util::sha256_file(&app.join("WitchyBND.exe")).is_ok_and(|h| h.eq_ignore_ascii_case(&pins.witchybnd_exe_sha256))
                    && std::fs::read_to_string(app.join(r"Assets\Paramdex\ER\Upgrader\version.txt")).is_ok_and(|v| v == pins.paramdex_version)
                    && std::fs::read_to_string(app.join("appsettings.override.json")).is_ok_and(|v| v == witchy::OVERRIDE_JSON)
                    && !app.join(r"Assets\Paramdex.zip").exists();
                if ready {
                    return Ok("already unpacked (exe, Paramdex upgrader and settings match)".into());
                }
                let zip = match self.facts.witchy.zip.clone().filter(|_| self.facts.witchy.zip_ok) {
                    Some(z) => z,
                    None => {
                        if !self.req.allow_download {
                            return Err(err(id, "WitchyBND 3.0.1.0 zip not found and the download was not allowed"));
                        }
                        let src = self.release.witchy_source().ok_or_else(|| err(id, "the release has no WitchyBND pin"))?;
                        let dest = root.join("downloads").join(&src.file_name);
                        sink(Progress::Line { id, text: format!("downloading {} (48 MB) from {}", src.file_name, src.url) });
                        let r = download::fetch_pinned_cancellable(&src, &dest, &g, cancel, |n, t| {
                            if let Some(t) = t.filter(|t| *t > 0) {
                                sink(Progress::Fraction { id, value: n as f32 / t as f32 })
                            }
                        });
                        match r {
                            Err(download::DownloadError::Cancelled) => return Err(err(id, CANCELLED)),
                            Err(e) => return Err(err(id, format!("{e} (or choose the zip you downloaded yourself)"))),
                            Ok(_) => {}
                        }
                        self.facts.witchy.zip = Some(dest.clone());
                        self.facts.witchy.zip_ok = true;
                        dest
                    }
                };
                witchy::prepare(&zip, &root, &pins, &g).map_err(|e| err(id, e))?;
                Ok(format!("WitchyBND 3.0.1.0 in tools\\WitchyBND\\app (Offline, Flexible=false, Paramdex upgrader {})", pins.paramdex_version))
            }
            StepId::Dumps => {
                let mw2 = self.facts.mw2.dir.clone().ok_or_else(|| err(id, "no MW2"))?;
                let unlinker = root.join(r"tools\oat\bin\Unlinker.exe");
                let pin = self.release.pins.unlinker_sha256.clone();
                if util::is_sha256(&pin) {
                    let h = util::sha256_file(&unlinker).map_err(|e| err(id, format!("{}: {e}", unlinker.display())))?;
                    if !h.eq_ignore_ascii_case(&pin) {
                        return Err(err(id, format!("tools\\oat\\bin\\Unlinker.exe is not the release's build (sha256 {}, want {}): refused", util::short(&h), util::short(&pin))));
                    }
                }
                let args = vec!["--mw2".to_string(), mw2.display().to_string(), "--unlinker".into(), unlinker.display().to_string()];
                let mut done = 0usize;
                let msg = {
                    let mut count = |e: Progress| {
                        if let Progress::Line { text, .. } = &e
                            && (text.starts_with("OK  ") || text.starts_with("FAIL"))
                        {
                            done += 1;
                            sink(Progress::Fraction { id, value: (done as f32 / DUMP_LINES as f32).min(1.0) });
                        }
                        sink(e);
                    };
                    self.python(id, "dumps", "friend_dump.py", &args, 20, cancel, &mut count)?
                };
                // the resume marker (plan::DUMPS_COMPLETE holds this step's key)
                let key = plan.get(id).map(|s| s.key.clone()).unwrap_or_default();
                g.write(&root.join(plan::DUMPS_COMPLETE.replace('/', "\\")), key).map_err(|e| err(id, e.to_string()))?;
                Ok(msg)
            }
            StepId::Converters => {
                let n = plan::CONVERTERS.len();
                let mut last = String::new();
                for (k, (name, script)) in plan::CONVERTERS.iter().enumerate() {
                    sink(Progress::Fraction { id, value: k as f32 / n as f32 });
                    last = self.python(id, name, script, &[], 10, cancel, sink)?;
                    sink(Progress::Line { id, text: last.clone() });
                }
                sink(Progress::Fraction { id, value: 1.0 });
                Ok(format!("{n} converters: {last}"))
            }
            StepId::VerifyConverted => self.verify(id, Only::Converted, cancel, sink),
            StepId::Regulation => match self.regulation_tool {
                Tool::Regbuild => self.regbuild(cancel, sink),
                Tool::Witchy => {
                    let m = self.python(id, "build_regulation", "build_regulation.py", &[], 15, cancel, sink)?;
                    self.state.regulation_tool = Tool::Witchy.as_str().into();
                    Ok(m)
                }
            },
            StepId::VerifyAll => self.verify(id, Only::All, cancel, sink),
            StepId::Cleanup => {
                for d in ["dumps", "work"] {
                    g.remove_dir_all(&root.join(d)).map_err(|e| err(id, e.to_string()))?;
                }
                self.state.forget_step(StepId::Dumps.as_str());
                Ok("temporary dumps\\ and work\\ removed".into())
            }
            StepId::Profiles => self.write_profiles(),
            StepId::Finish => self.finish(plan, sink),
        }
    }

    /// The regulation step with the in-process Rust builder (P2): reads the game's vanilla regulation.bin (only read;
    /// regbuild checks its sha256 pin), writes build\package\regulation.bin + build\rows.json (both paths checked by
    /// the guard first; the step cleared build\package before) and logs to logs\install\regulation.log.
    fn regbuild(&mut self, cancel: &AtomicBool, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Regulation;
        let root = self.req.root.clone();
        let g = self.guard.clone();
        let game = self.facts.er.game.clone().ok_or_else(|| err(id, "no Elden Ring folder"))?;
        let vanilla = game.join("regulation.bin");
        let log = self.log_dir().join(regulation::LOG_NAME);
        for p in [root.join(regulation::PACKAGE), root.join(regulation::ROWS_JSON), log.clone()] {
            g.check(&p).map_err(|e| err(id, e.to_string()))?;
        }
        g.create_dir_all(&root.join(r"build\package")).map_err(|e| err(id, e.to_string()))?;
        let mut lines = vec![format!("builder: {}", regbuild::BUILDER_ID), format!("vanilla (read only): {}", vanilla.display())];
        for l in &lines {
            sink(Progress::Line { id, text: l.clone() });
        }
        let job = regulation::BuildJob { root, vanilla };
        let r = {
            let mut on_line = |l: &str| {
                // regbuild's phases: reading, planning, compressing (~17 s of ~18), re-reading
                let f = [("planning", 0.05f32), ("compressing", 0.1), ("re-reading", 0.9)].into_iter().find(|(p, _)| l.starts_with(p));
                if let Some((_, v)) = f {
                    sink(Progress::Fraction { id, value: v });
                }
                lines.push(l.to_string());
                sink(Progress::Line { id, text: l.to_string() });
            };
            (self.regbuilder)(&job, cancel, &mut on_line)
        };
        lines.push(match &r {
            Ok(m) => m.clone(),
            Err(e) => format!("FAIL {e}"),
        });
        let _ = g.write(&log, lines.join("\r\n") + "\r\n");
        match r {
            Ok(m) => {
                sink(Progress::Fraction { id, value: 1.0 });
                self.state.regulation_tool = Tool::Regbuild.as_str().into();
                Ok(m)
            }
            Err(_) if cancel.load(Ordering::SeqCst) => Err(err(id, CANCELLED)),
            Err(e) if e == CANCELLED => Err(err(id, CANCELLED)),
            Err(e) => Err(err(id, format!("{e} ({LOG_DIR}\\{})", regulation::LOG_NAME))),
        }
    }

    fn copy_payload(&mut self, cancel: &AtomicBool, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Payload;
        let root = self.req.root.clone();
        let g = self.guard.clone();
        let files = plan::payload_files(&self.req.payload);
        // record the top-level entries before copying (uninstall deletes only what an install created)
        for (rel, _) in &files {
            let top = rel.split('/').next().unwrap_or("").to_lowercase();
            if !top.is_empty() && !self.state.created.contains(&top) {
                self.state.created.push(top);
            }
        }
        self.state.created.sort();
        let n = files.len().max(1);
        let (mut copied, mut same) = (0usize, 0usize);
        let mut merged = None;
        // the installed release's record of its mw2er.toml, read before the payload replaces manifest\release.json
        let old_toml_sha = self.installed_toml_sha();
        for (k, (rel, src)) in files.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                return Err(err(id, CANCELLED));
            }
            if k % 16 == 0 {
                sink(Progress::Fraction { id, value: k as f32 / n as f32 });
            }
            let h = util::sha256_file(src).map_err(|e| err(id, e.to_string()))?;
            if let Some(want) = self.release.payload.get(rel)
                && !h.eq_ignore_ascii_case(want)
            {
                return Err(err(id, format!("payload file {rel} is damaged (sha256 {} != {}): download the release again", util::short(&h), util::short(want))));
            }
            let to = root.join(rel.replace('/', "\\"));
            if rel == "mw2er.toml" && to.is_file() {
                // P3: the player's settings are merged into the release's file (docs 2.8 "Settings")
                merged = Some(self.merge_settings(src, old_toml_sha.as_deref(), sink)?);
                continue;
            }
            // P3 incremental: a file that is already the release's is not touched (a DLL-only release copies the DLL)
            if to.is_file() && util::sha256_file(&to).is_ok_and(|d| d == h) {
                same += 1;
                continue;
            }
            self.keep_for_rollback(rel).map_err(|e| err(id, e))?;
            // atomic: the new file goes in under a temporary name, then replaces the old one in one rename
            let tmp = to.with_file_name(format!("{}.p3new", to.file_name().and_then(|x| x.to_str()).unwrap_or("file")));
            g.copy(src, &tmp).map_err(|e| err(id, format!("copy {rel}: {e}")))?;
            g.rename(&tmp, &to).map_err(|e| err(id, format!("replace {rel}: {e}")))?;
            copied += 1;
        }
        let pin = &self.release.pins.mw2er_dll_sha256;
        if util::is_sha256(pin) {
            let h = util::sha256_file(&root.join(r"build\natives\mw2er.dll")).map_err(|e| err(id, e.to_string()))?;
            if !h.eq_ignore_ascii_case(pin) {
                return Err(err(id, "mw2er.dll does not match the release pin"));
            }
        }
        let policy = self.apply_toml_policy(sink)?;
        // the release's own file: the base of the next merge
        let rel_toml = self.req.payload.join("mw2er.toml");
        if rel_toml.is_file() && !util::same_path(&root, &self.req.payload) {
            // kept for a rollback (or recorded as added): Roll back restores X's base along with X's mw2er.toml
            self.keep_for_rollback("state/release_mw2er.toml").map_err(|e| err(id, e))?;
            let to = root.join(RELEASE_TOML);
            let tmp = to.with_file_name("release_mw2er.toml.p3new");
            g.copy(&rel_toml, &tmp).map_err(|e| err(id, e.to_string()))?;
            g.rename(&tmp, &to).map_err(|e| err(id, e.to_string()))?;
        }
        Ok(format!(
            "{copied} file(s) copied, {same} already up to date (from {}){}; {policy}",
            self.req.payload.display(),
            merged.map(|m: String| format!("; {m}")).unwrap_or_default()
        ))
    }

    /// P3: keeps the root's `rel` in the rollback store before it is replaced (only during an update).
    fn keep_for_rollback(&mut self, rel: &str) -> Result<(), String> {
        let root = self.req.root.clone();
        let g = self.guard.clone();
        if let Some(st) = self.store.as_mut() {
            st.save_file(&root, rel, &g).map_err(|e| format!("cannot keep {rel} for a rollback: {e}"))?;
        }
        Ok(())
    }

    /// P3: merges the installed mw2er.toml (the player's) into the release's ([`toml_merge::merge`]): hot settings
    /// kept, build inputs / policy / layout from the release, the player's old file backed up (and kept in the
    /// rollback store during an update). Returns the summary line.
    fn merge_settings(&mut self, release_toml: &Path, old_toml_sha: Option<&str>, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Payload;
        let root = self.req.root.clone();
        let g = self.guard.clone();
        let to = root.join("mw2er.toml");
        let rel_text = std::fs::read_to_string(release_toml).map_err(|e| err(id, format!("{}: {e}", release_toml.display())))?;
        let player = std::fs::read_to_string(&to).map_err(|e| err(id, format!("{}: {e}", to.display())))?;
        let base = merge_base(&root, &player, old_toml_sha);
        let m = toml_merge::merge(&player, base.as_deref(), &rel_text, &self.release.locked_toml);
        for l in m.lines() {
            sink(Progress::Line { id, text: l });
        }
        let summary = format!("{}{}", m.summary(), if base.is_none() { NO_BASE } else { "" });
        if base.is_none() {
            sink(Progress::Line { id, text: NO_BASE.trim_start_matches("; ").to_string() });
        }
        if m.text == player {
            return Ok(summary);
        }
        self.keep_for_rollback("mw2er.toml").map_err(|e| err(id, e))?;
        let from = self.update_from.clone().unwrap_or_else(|| self.prev_state.release_version.clone());
        let backup = root.join(SETTINGS_BACKUPS).join(format!("mw2er-{}-{}.toml", if from.is_empty() { "old" } else { &from }, util::local_folder_stamp()));
        g.copy(&to, &backup).map_err(|e| err(id, e.to_string()))?;
        sink(Progress::Line { id, text: format!("your previous mw2er.toml is backed up as {}", backup.display()) });
        g.write_atomic(&to, m.text.as_bytes()).map_err(|e| err(id, e.to_string()))?;
        Ok(summary)
    }

    /// `payload["mw2er.toml"]` of the root's `manifest\release.json` (the installed release's file; None for the P0
    /// layout, or when the root is the payload).
    fn installed_toml_sha(&self) -> Option<String> {
        if util::same_path(&self.req.root, &self.req.payload) || !self.req.root.join(release::RELEASE_JSON).is_file() {
            return None;
        }
        Release::load(&self.req.root).ok()?.payload.get("mw2er.toml").cloned()
    }

    /// P3: what this run will do as an update / repair (None while the plan is missing).
    pub fn update_summary(&self, plan: &Plan) -> Option<update::Summary> {
        let recorded = InstallState::path(&self.req.root).is_file();
        let installed = recorded.then(|| self.update_from.clone().unwrap_or_else(|| self.prev_state.release_version.clone()));
        let kind = update::kind(installed.as_deref(), &self.release.version);
        let mut changes = Vec::new();
        let mut settings = None;
        if kind != update::Kind::Fresh {
            if let Some(p) = plan.get(StepId::Payload).filter(|s| s.decision.runs()) {
                changes = update::file_changes(&self.req.root, &p.inputs);
            }
            let rel_text = std::fs::read_to_string(self.req.payload.join("mw2er.toml")).unwrap_or_default();
            if let Ok(player) = std::fs::read_to_string(self.req.root.join("mw2er.toml"))
                && !util::same_path(&self.req.root, &self.req.payload)
            {
                changes.extend(update::build_input_changes(&player, &rel_text));
                let base = merge_base(&self.req.root, &player, self.installed_toml_sha().as_deref());
                let m = toml_merge::merge(&player, base.as_deref(), &rel_text, &self.release.locked_toml).summary();
                settings = Some(format!("{m}{}", if base.is_none() { NO_BASE } else { "" }));
            }
        }
        let to_coop_id = if regulation::by_content(&self.release.pins, self.regulation_tool) {
            String::new()
        } else {
            self.release.coop_id()
        };
        Some(update::Summary {
            kind,
            from_version: installed.unwrap_or_default(),
            to_version: self.release.version.clone(),
            from_coop_id: if recorded { self.prev_state.coop_id.clone() } else { String::new() },
            to_coop_id,
            changes,
            steps: plan.steps.iter().filter(|s| s.decision.runs()).map(|s| s.id).collect(),
            settings,
        })
    }

    /// The release toml policy (friend_toml) on the installed mw2er.toml: probes forced off (rewritten), a missing
    /// tested feature refuses the install. The build-input sections never change.
    fn apply_toml_policy(&self, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Payload;
        let p = self.req.root.join("mw2er.toml");
        let text = std::fs::read_to_string(&p).map_err(|e| err(id, format!("{}: {e}", p.display())))?;
        let a = friend_toml::apply(&text).map_err(|e| err(id, format!("the release's {e}")))?;
        if a.text == text {
            return Ok("mw2er.toml fits the release policy".into());
        }
        if plan::toml_build_sections(&a.text) != plan::toml_build_sections(&text) {
            return Err(err(id, "the toml policy would change a build-input section (refused)"));
        }
        self.guard.write(&p, a.text.as_bytes()).map_err(|e| err(id, e.to_string()))?;
        for c in &a.changed {
            sink(Progress::Line { id, text: format!("mw2er.toml policy: {c}") });
        }
        Ok(format!("mw2er.toml policy: {} test setting(s) forced off", a.changed.len()))
    }

    /// The Python the build runs: the one chosen by hand, else the bundled CPython (unpacked from the pinned zip,
    /// or copied from the payload's runtime\python), else the Python found on this PC (P0 behaviour).
    fn runtime(&mut self, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Runtime;
        let root = self.req.root.clone();
        if let Some(o) = self.req.over.python.clone() {
            return Ok(format!("using the Python chosen by hand: {}", o.display()));
        }
        let rp = payload::runtime_python(&root);
        let mut how = "copied from the release";
        if let Some(zip) = self.python_zip.clone() {
            let current = util::sha256_file(&rp).is_ok_and(|h| h.eq_ignore_ascii_case(payload::PYTHON_EXE_SHA256));
            if current {
                how = "already installed";
            } else {
                if let Some(st) = self.store.as_ref()
                    && st.move_away(&root, "runtime/python", &self.guard).map_err(|e| err(id, e.to_string()))?
                {
                    sink(Progress::Line { id, text: "kept the previous runtime\\python for a rollback".into() });
                }
                sink(Progress::Line { id, text: format!("unpacking {} (sha256-pinned) into {}", zip.display(), payload::RUNTIME_DIR) });
                payload::install_python_zip(&zip, &root, payload::PYTHON_EMBED_SHA256, &self.guard).map_err(|e| err(id, e))?;
                how = "unpacked from the official embeddable package";
            }
        }
        if rp.is_file() {
            self.facts.python = detect::PythonFacts { exe: Some(rp.clone()), bundled: true, version: self.facts.python.version.filter(|_| self.facts.python.bundled) };
            return Ok(format!("bundled CPython ({how}): {}", rp.display()));
        }
        match &self.facts.python.exe {
            Some(p) => Ok(format!(
                "no bundled runtime in this release: using {}{}",
                p.display(),
                self.facts.python.version.map(|v| format!(" (Python {}.{})", v / 100, v % 100)).unwrap_or_default()
            )),
            None => Err(err(id, "no Python: the release has no runtime\\python and none was found on this PC")),
        }
    }

    fn write_profiles(&mut self) -> Result<String, InstallError> {
        let id = StepId::Profiles;
        let root = &self.req.root.clone();
        let dll = root.join(r"build\natives\mw2er.dll").display().to_string();
        let pkg = root.join(r"build\package").display().to_string();
        let ersc = root.join(seamless::COPY_DIR).join("ersc.dll").display().to_string();
        let mut done = Vec::new();
        let mut modes = vec![Mode::Solo];
        if self.req.mode.coop() {
            modes.push(Mode::Coop);
        }
        for m in modes {
            let tpl = root.join(if m.coop() { r"profiles\mw2er-coop.template.me3" } else { r"profiles\mw2er-solo.template.me3" });
            let t = std::fs::read_to_string(&tpl).map_err(|e| err(id, format!("{}: {e}", tpl.display())))?;
            let text = launch::fill_profile_template(&t, &[("@@ERSC@@", &ersc), ("@@DLL@@", &dll), ("@@PACKAGE@@", &pkg)]);
            launch::check_profile_text(&text, m).map_err(|e| err(id, e))?;
            let out = launch::profile_path(root, m);
            if std::fs::read_to_string(&out).ok().as_deref() != Some(text.as_str()) {
                if let Some(rel) = out.strip_prefix(root).ok().map(util::rel_key) {
                    let g = self.guard.clone();
                    if let Some(st) = self.store.as_mut() {
                        st.save_file(root, &rel, &g).map_err(|e| err(id, e.to_string()))?;
                    }
                }
                self.guard.write(&out, text).map_err(|e| err(id, e.to_string()))?;
            }
            done.push(out.display().to_string());
        }
        if let Some(me3) = &self.facts.me3.exe {
            let mut lc = String::new();
            for m in [Mode::Coop, Mode::Solo].into_iter().filter(|m| self.req.mode.coop() || !m.coop()) {
                lc.push_str(&format!("& {}\r\n", launch::command(me3, root, m).display()));
            }
            self.guard.write(&root.join("launch_command.txt"), lc).map_err(|e| err(id, e.to_string()))?;
        }
        Ok(format!("wrote {}", done.join(", ")))
    }

    fn finish(&mut self, _plan: &Plan, sink: &mut dyn FnMut(Progress)) -> Result<String, InstallError> {
        let id = StepId::Finish;
        let root = self.req.root.clone();
        let g = self.guard.clone();
        let exe = root.join(INSTALLED_EXE);
        if let Some(me) = self.req.self_exe.clone()
            && !util::same_path(&me, &exe)
        {
            self.keep_for_rollback(INSTALLED_EXE).map_err(|e| err(id, e))?;
            // without the embedded release (the payload files are in the root now)
            payload::copy_without_overlay(&me, &exe, &g).map_err(|e| err(id, format!("cannot copy the launcher: {e}")))?;
        }
        // preflight hashes for the launcher
        self.state.preflight.clear();
        for rel in launch::preflight_files(self.req.mode) {
            if let Ok(h) = util::sha256_file(&root.join(rel.replace('/', "\\"))) {
                self.state.preflight.insert(rel.to_string(), h);
            }
        }
        self.state.coop_id = self.coop_id();
        // shortcuts
        // the launcher's own icon (resource 1 of MW2inER.exe)
        let icon = Some((exe.clone(), 0));
        let mut made = Vec::new();
        let mut links: Vec<(PathBuf, &str, String)> = Vec::new();
        let n = crate::DISPLAY_NAME;
        // the places asked for: (the folder given, the folder the links go into)
        let places: Vec<(PathBuf, PathBuf)> = [self.req.shortcuts.desktop.clone().map(|d| (d.clone(), d)), self.req.shortcuts.start_menu.clone().map(|s| (s.clone(), s.join(SHORTCUT_FOLDER)))]
            .into_iter()
            .flatten()
            .collect();
        for (_, dir) in &places {
            let [solo, coop, setup] = shortcut_names(n);
            links.push((dir.join(solo), "play solo", format!("{n} alone (own save ER0000.mw2er.sl2). Start Steam first.")));
            if self.req.mode.coop() {
                links.push((dir.join(coop), "play coop", format!("{n} with Seamless Co-op. Start Steam first. Never use ersc_launcher.exe.")));
            }
            links.push((dir.join(setup), "", format!("Repair, update, verify or uninstall {n}")));
        }
        for (lnk, args, desc) in links {
            g.check(&lnk).map_err(|e| err(id, e.to_string()))?;
            let s = crate::win::Shortcut { lnk: lnk.clone(), target: exe.clone(), args: args.into(), workdir: root.clone(), description: desc, icon: icon.clone() };
            match crate::win::write_shortcut(&s) {
                Ok(()) => made.push(lnk),
                Err(e) => sink(Progress::Line { id, text: format!("WARN could not create {}: {e}", lnk.display()) }),
            }
        }
        // P4: the shortcuts an older release made under its old name ("MW2 in Elden Ring - Co-op.lnk", the Start-menu
        // folder "MW2 in Elden Ring") are replaced: removed (and dropped from the record) once a new shortcut was made
        // in the same place. Without new shortcuts there (--no-shortcuts, a box unticked) they stay recorded, so the
        // uninstall still removes them.
        let renewed: Vec<&PathBuf> = places.iter().filter(|(_, dir)| made.iter().any(|m| m.parent().is_some_and(|p| util::same_path(p, dir)))).map(|(base, _)| base).collect();
        let mut kept = Vec::new();
        for old in std::mem::take(&mut self.state.shortcuts) {
            let replace = is_legacy_shortcut(&old) && !made.contains(&old) && legacy_shortcut_base(&old).is_some_and(|b| renewed.iter().any(|r| util::same_path(r, &b)));
            if !replace {
                kept.push(old);
                continue;
            }
            match g.remove_file(&old) {
                Ok(()) => {
                    sink(Progress::Line { id, text: format!("replaced the old shortcut {}", old.display()) });
                    if let Some(d) = old.parent().filter(|d| is_legacy_folder(d)) {
                        let _ = std::fs::remove_dir(d); // only when empty
                    }
                }
                Err(e) => {
                    sink(Progress::Line { id, text: format!("WARN could not remove the old shortcut {}: {e}", old.display()) });
                    kept.push(old);
                }
            }
        }
        self.state.shortcuts = kept;
        for m in &made {
            if !self.state.shortcuts.contains(m) {
                self.state.shortcuts.push(m.clone());
            }
        }
        if self.req.register {
            use crate::win::RegValue::{Dword, Str};
            let r = root.display().to_string();
            // the uninstall window (Apps & Features starts it with its own console: the wizard's uninstall page)
            let uninst = format!("\"{}\" uninstall", exe.display());
            crate::win::reg_set_hkcu(APP_KEY, &[("InstallRoot", Str(&r)), ("Version", Str(&self.release.version))]).map_err(|e| err(id, e.to_string()))?;
            crate::win::reg_set_hkcu(
                UNINSTALL_KEY,
                &[
                    ("DisplayName", Str(crate::DISPLAY_NAME)),
                    ("DisplayVersion", Str(&self.release.version)),
                    ("Publisher", Str(crate::PUBLISHER)),
                    ("InstallLocation", Str(&r)),
                    ("UninstallString", Str(&uninst)),
                    ("DisplayIcon", Str(&exe.display().to_string())),
                    ("NoModify", Dword(1)),
                    ("NoRepair", Dword(1)),
                ],
            )
            .map_err(|e| err(id, e.to_string()))?;
            for k in [APP_KEY, UNINSTALL_KEY] {
                if !self.state.registry.iter().any(|x| x == k) {
                    self.state.registry.push(k.to_string());
                }
            }
        }
        self.state.last_result = format!("PASS install {} (co-op id {})", util::utc_stamp(), self.state.coop_id);
        Ok(format!("co-op id {}; {} shortcut(s)", self.state.coop_id, made.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::sync::Mutex;

    fn env(t: &TempDir, steam: &Path) -> Env {
        Env {
            steam_path: Some(steam.to_path_buf()),
            local_appdata: Some(t.0.join("Local")),
            appdata: Some(t.0.join("Roaming")),
            downloads: Some(t.0.join("Downloads")),
            ..Default::default()
        }
    }

    fn request(t: &TempDir, pkg: PathBuf, mode: Mode) -> Request {
        Request {
            root: t.0.join("MW2inER"),
            payload: pkg,
            mode,
            save_root: Some(t.0.join(r"Roaming\EldenRing")),
            shortcuts: ShortcutDirs { desktop: Some(t.0.join("Desktop")), start_menu: None },
            ..Default::default()
        }
    }

    /// The fixture's facts that cannot come from fixture files (the fixture exes are no PE files).
    fn patch(s: &mut Session) {
        s.facts.er.exe_version = Some("2.7.2.0".into());
        s.facts.me3.version = Some("0.13.0".into());
        s.checks = detect::evaluate(&s.facts, &s.release.pins, false);
        s.refresh_tool_rows();
    }

    /// A full fixture session: everything detected except the real exe version (fixture exe is not a PE), which
    /// the test patches into the facts.
    fn session(t: &TempDir, mode: Mode) -> Session {
        let (steam, _) = crate::steam::fixture::steam(t, "25827938", "25374375");
        let pkg = crate::release::fixture::package(t);
        t.write(r"Local\Programs\garyttierney\me3\bin\me3.exe", b"me3");
        t.write(r"pkg\runtime\python\python.exe", b"py");
        crate::seamless::fixture::zip(t, "Downloads/Seamless Co-op 2.0.1.zip", "", b"ersc");
        t.write(r"Roaming\EldenRing\7656\ER0000.sl2", b"save");
        let mut s = Session::prepare(env(t, &steam), request(t, pkg, mode)).unwrap();
        patch(&mut s);
        s
    }

    // ---- fake tools ---------------------------------------------------------------------------------------------

    #[derive(Clone, Default)]
    struct Fake {
        /// Job names in call order.
        calls: Arc<Mutex<Vec<String>>>,
        /// Job name -> exit 1.
        fail: Option<String>,
        /// Job name -> the player presses Cancel while it runs.
        cancel_in: Option<String>,
        /// convert_weapons writes the wrong bytes (the converted output differs from the release).
        wrong_output: bool,
        /// Job name -> this program dies while it runs (a crash / kill / power cut).
        crash_in: Option<String>,
        /// (regulation.bin, rows.json) the fake regbuild writes (default: the fixture manifest's bytes).
        regbuild_out: Option<Arc<(Vec<u8>, String)>>,
        /// (regulation.bin, rows.json) the fake build_regulation.py (WitchyBND) writes.
        witchy_out: Option<Arc<(Vec<u8>, String)>>,
    }

    fn ok(code: i32, last: &str) -> std::io::Result<proc::JobResult> {
        Ok(proc::JobResult { code: Some(code), timed_out: false, cancelled: false, secs: 0.01, last_line: last.into() })
    }

    impl Fake {
        fn runner(&self) -> Runner {
            let f = self.clone();
            Arc::new(move |job: &proc::Job, cancel: &AtomicBool, line: &mut dyn FnMut(&str)| {
                f.calls.lock().unwrap().push(job.name.clone());
                assert_eq!(job.args[..2], ["-I".to_string(), "-B".to_string()], "python -I -B");
                assert!(job.env.iter().any(|(k, v)| k == "PYTHONUNBUFFERED" && v == "1"));
                assert!(job.log.starts_with(job.cwd.join(LOG_DIR)), "tool logs in logs\\install");
                let root = &job.cwd;
                let w = |rel: &str, data: &[u8]| {
                    let p = root.join(rel);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, data).unwrap();
                };
                if f.cancel_in.as_deref() == Some(job.name.as_str()) {
                    line("working ...");
                    cancel.store(true, Ordering::SeqCst);
                    return Ok(proc::JobResult { code: None, timed_out: false, cancelled: true, secs: 0.01, last_line: "working ...".into() });
                }
                if f.crash_in.as_deref() == Some(job.name.as_str()) {
                    w(r"converted\half_written.bin", b"partial");
                    panic!("simulated crash in {}", job.name);
                }
                if f.fail.as_deref() == Some(job.name.as_str()) {
                    line("Traceback: fake failure");
                    return ok(1, "Traceback: fake failure");
                }
                match job.name.as_str() {
                    "dumps" => {
                        w(r"dumps\common_mp\weapons\m4a1", b"dump");
                        for k in 0..DUMP_LINES {
                            line(&format!("OK   step {k}"));
                        }
                        line("PASS dumps ready");
                        ok(0, "PASS dumps ready")
                    }
                    "convert_weapons" => {
                        w(r"converted\weapons.json", if f.wrong_output { b"{\"x\":1}" } else { b"{}" });
                        w(r"work\m2\tmp.bin", b"t");
                        ok(0, "PASS weapons")
                    }
                    "build_regulation" => {
                        match &f.witchy_out {
                            Some(o) => {
                                w(r"build\package\regulation.bin", &o.0);
                                w(r"build\rows.json", o.1.as_bytes());
                            }
                            None => {
                                w(r"build\package\regulation.bin", b"package regulation");
                                w(r"build\rows.json", b"{}");
                            }
                        }
                        w(r"work\regulation\x.bin", b"t");
                        ok(0, "REGULATION PASS")
                    }
                    "verify_converted" | "verify_all" => ok(0, "PASS (fake coop_manifest.py)"),
                    _ => ok(0, &format!("PASS {}", job.name)),
                }
            })
        }

        /// The fake in-process regulation build ("regbuild" in the calls): checks the job, writes the outputs.
        fn builder(&self) -> regulation::Builder {
            let f = self.clone();
            Arc::new(move |job: &regulation::BuildJob, cancel: &AtomicBool, line: &mut dyn FnMut(&str)| {
                f.calls.lock().unwrap().push("regbuild".into());
                assert!(job.vanilla.ends_with(r"ELDEN RING\Game\regulation.bin"), "{}", job.vanilla.display());
                assert!(job.root.join(r"build\package").is_dir() && std::fs::read_dir(job.root.join(r"build\package")).unwrap().count() == 0, "a clean package folder");
                line("reading the vanilla regulation.bin (read only)");
                line("planning the rows (mw2er.toml + converted data)");
                if f.cancel_in.as_deref() == Some("regbuild") {
                    cancel.store(true, Ordering::SeqCst);
                    return Err("cancelled".into());
                }
                if f.fail.as_deref() == Some("regbuild") {
                    return Err("game regulation changed (sha256 00); the pinned vanilla is b8cd".into());
                }
                line("compressing (zstd level 21, ~20 s)");
                line("re-reading and verifying the output");
                let (reg, rows) = match &f.regbuild_out {
                    Some(o) => (o.0.clone(), o.1.clone()),
                    None => (b"package regulation".to_vec(), "{}".to_string()),
                };
                std::fs::write(job.root.join(regulation::PACKAGE), reg).unwrap();
                std::fs::write(job.root.join(regulation::ROWS_JSON), rows).unwrap();
                Ok("REGULATION PASS (regbuild, 0 s)".into())
            })
        }

        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }
    }

    /// Turns the fixture release (t\pkg) into a P2 release made with `builder`; `content` = its builder-independent
    /// pins (BND4, rows core) and the manifest entry of its regulation package (the host's file).
    fn p2_release(t: &TempDir, builder: Tool, content: Option<(&[u8], &str)>) {
        let pins_p = t.0.join(r"pkg\manifest\host_pins.json");
        let mut pins: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&pins_p).unwrap()).unwrap();
        pins["regulation_builder"] = builder.as_str().into();
        if let Some((reg, rows)) = content {
            t.write(r"hostbuild\build\package\regulation.bin", reg);
            t.write(r"hostbuild\build\rows.json", rows);
            let c = regulation::content(&t.0.join("hostbuild")).unwrap();
            pins["package_bnd4_sha256"] = c.bnd4_sha256.into();
            pins["rows_json_core_sha256"] = c.rows_core_sha256.into();
            pins["package_regulation_sha256"] = c.package_sha256.clone().into();
            let man_p = t.0.join(r"pkg\manifest\host_manifest.json");
            let mut man: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&man_p).unwrap()).unwrap();
            man["files"]["build/package/regulation.bin"] = serde_json::json!({"sha256": c.package_sha256, "size": c.package_len});
            std::fs::write(&man_p, serde_json::to_string_pretty(&man).unwrap()).unwrap();
        }
        std::fs::write(&pins_p, serde_json::to_string_pretty(&pins).unwrap()).unwrap();
    }

    /// A session on a P2 release made with regbuild: no WitchyBND anywhere (not primed, no zip, no permission).
    fn ready_p2(t: &TempDir, mode: Mode, fake: &Fake, content: Option<(&[u8], &str)>) -> Session {
        let mut s = session(t, mode);
        p2_release(t, Tool::Regbuild, content);
        s.release = Release::load(&t.0.join("pkg")).unwrap();
        s.set_regulation_tool(None);
        s.checks.retain(|c| c.id != "disk");
        s.runner = fake.runner();
        s.is_running = |_| false;
        s.regbuilder = fake.builder();
        s
    }

    fn reprepare_p2(t: &TempDir, mode: Mode, fake: &Fake, tool: Option<Tool>) -> Session {
        let steam = t.0.join("Steam");
        let mut req = request(t, t.0.join("pkg"), mode);
        req.regulation_tool = tool;
        let mut s = Session::prepare(env(t, &steam), req).unwrap();
        patch(&mut s);
        s.checks.retain(|c| c.id != "disk");
        s.runner = fake.runner();
        s.is_running = |_| false;
        s.regbuilder = fake.builder();
        s
    }

    /// The P2 tool order: the converters, verify converted, the in-process regbuild, verify all.
    const P2_ORDER: &[&str] = &[
        "dumps",
        "convert_weapons",
        "copy_sounds",
        "convert_viewmodel",
        "convert_ac130",
        "convert_movement",
        "convert_arsenal",
        "convert_nuke",
        "verify_converted",
        "regbuild",
        "verify_all",
    ];

    /// A session ready for a full fake build: WitchyBND already unpacked with the pinned exe, disk check dropped.
    fn ready(t: &TempDir, mode: Mode, fake: &Fake) -> Session {
        let mut s = session(t, mode);
        prime_witchy(t, &s.release.pins);
        s.checks.retain(|c| c.id != "witchy" && c.id != "disk");
        s.runner = fake.runner();
        s.is_running = |_| false;
        s
    }

    fn prime_witchy(t: &TempDir, pins: &release::Pins) {
        t.write(r"MW2inER\tools\WitchyBND\app\WitchyBND.exe", b"witchy exe");
        t.write(r"MW2inER\tools\WitchyBND\app\Assets\Paramdex\ER\Upgrader\version.txt", &pins.paramdex_version);
        t.write(r"MW2inER\tools\WitchyBND\app\appsettings.override.json", witchy::OVERRIDE_JSON);
    }

    fn reprepare(t: &TempDir, mode: Mode, fake: &Fake) -> Session {
        let steam = t.0.join("Steam");
        let mut s = Session::prepare(env(t, &steam), request(t, t.0.join("pkg"), mode)).unwrap();
        patch(&mut s);
        s.checks.retain(|c| c.id != "witchy" && c.id != "disk");
        s.runner = fake.runner();
        s.is_running = |_| false;
        s
    }

    const BUILD_ORDER: &[&str] = &[
        "dumps",
        "convert_weapons",
        "copy_sounds",
        "convert_viewmodel",
        "convert_ac130",
        "convert_movement",
        "convert_arsenal",
        "convert_nuke",
        "verify_converted",
        "build_regulation",
        "verify_all",
    ];

    fn started(ev: &[Progress]) -> Vec<StepId> {
        ev.iter().filter_map(|e| if let Progress::StepStart { id, .. } = e { Some(*id) } else { None }).collect()
    }

    fn skipped(ev: &[Progress]) -> Vec<StepId> {
        ev.iter().filter_map(|e| if let Progress::Skipped { id, .. } = e { Some(*id) } else { None }).collect()
    }

    // ---- tests ----------------------------------------------------------------------------------------------------

    #[test]
    fn prepare_detects_and_plans() {
        let t = TempDir::new("sess");
        let s = session(&t, Mode::Coop);
        assert!(s.blocking().is_empty() || s.blocking().iter().all(|c| c.id == "witchy" || c.id == "disk"), "{:#?}", s.blocking());
        let p = s.plan().unwrap();
        assert!(p.steps.iter().all(|x| x.decision.runs()));
        assert!(s.guard.check(&t.0.join(r"SteamLibrary\steamapps\common\ELDEN RING\Game\x")).is_err());
        assert!(s.guard.check(&t.0.join(r"Roaming\EldenRing\x")).is_err());
        assert!(s.guard.check(&t.0.join(r"MW2inER\x")).is_ok());
    }

    #[test]
    fn the_download_permission_flips_the_witchy_row() {
        let t = TempDir::new("sess-dl");
        let mut s = session(&t, Mode::Solo);
        let ids: Vec<&str> = s.checks.iter().map(|c| c.id).collect();
        let row = |s: &Session| s.checks.iter().find(|c| c.id == "witchy").map(|c| c.level);
        s.set_allow_download(false);
        assert_eq!(row(&s), Some(Level::Fail), "no zip + no permission = red");
        assert!(!s.req.allow_download);
        s.set_allow_download(true);
        assert_eq!(row(&s), Some(Level::Warn), "the pinned download will run");
        assert!(s.req.allow_download);
        assert_eq!(s.checks.iter().map(|c| c.id).collect::<Vec<_>>(), ids, "same rows, same order");
        // a zip of the player's own (wrong hash) stays red whatever the permission
        t.write(r"Downloads\WitchyBND-3.0.1.0-win-x64.zip", b"not the release");
        let mut s = session(&t, Mode::Solo);
        s.set_allow_download(true);
        assert_eq!(row(&s), Some(Level::Fail));
    }

    #[test]
    fn root_inside_a_game_folder_is_refused() {
        let t = TempDir::new("sess-root");
        let (steam, _) = crate::steam::fixture::steam(&t, "1", "2");
        let pkg = crate::release::fixture::package(&t);
        let req = Request { root: t.0.join(r"SteamLibrary\steamapps\common\MW2inER"), payload: pkg, ..Default::default() };
        let s = Session::prepare(env(&t, &steam), req).unwrap();
        assert!(s.blocking().iter().any(|c| c.id == "root"));
    }

    /// `--cli install --root D:\Games` / `--root %USERPROFILE%\Documents`: a folder of the player's own files is
    /// refused (uninstall would delete them), and the player's Seamless download found there is never written.
    #[test]
    fn root_with_the_players_own_files_is_refused() {
        let t = TempDir::new("sess-root-own");
        let (steam, _) = crate::steam::fixture::steam(&t, "1", "2");
        let pkg = crate::release::fixture::package(&t);
        t.write(r"Games\Other Game\game.exe", b"mine");
        let req = Request { root: t.0.join("Games"), payload: pkg.clone(), ..Default::default() };
        let s = Session::prepare(env(&t, &steam), req).unwrap();
        let row = s.blocking().into_iter().find(|c| c.id == "root").expect("root refused");
        assert!(row.msg.contains(r"Games\MW2inER"), "{}", row.msg);
        // Documents holding the player's Seamless (Documents\seamless\SeamlessCoop)
        t.write(r"Documents\seamless\SeamlessCoop\ersc.dll", b"ersc");
        let ini = t.write(r"Documents\seamless\SeamlessCoop\ersc_settings.ini", crate::seamless::fixture::INI_TEXT);
        let docs_env = || Env { documents: Some(t.0.join("Documents")), ..env(&t, &steam) };
        let req = Request { root: t.0.join("Documents"), payload: pkg.clone(), mode: Mode::Coop, ..Default::default() };
        let s = Session::prepare(docs_env(), req).unwrap();
        assert!(s.blocking().iter().any(|c| c.id == "root"));
        assert!(matches!(&s.facts.seamless.found, Some(seamless::Found::Dll(d)) if d.starts_with(t.0.join("Documents"))));
        assert!(s.guard.check(&ini).is_err(), "the player's own Seamless folder is forbidden");
        // a fresh root next to it is fine, and the download stays read-only
        let req = Request { root: t.0.join(r"Documents\MW2inER"), payload: pkg, mode: Mode::Coop, ..Default::default() };
        let s = Session::prepare(docs_env(), req).unwrap();
        assert!(!s.blocking().iter().any(|c| c.id == "root"));
        assert!(s.guard.check(&ini).is_err() && s.guard.check(&t.0.join(r"Documents\MW2inER\seamless\SeamlessCoop\x")).is_ok());
    }

    /// The steps that need no Python / network on fixtures, one by one: preflight, save backup, payload, runtime,
    /// Seamless, cleanup, profiles, finish.
    #[test]
    fn non_python_steps_on_fixtures() {
        let t = TempDir::new("sess-run");
        let mut s = session(&t, Mode::Coop);
        s.checks.retain(|c| c.id != "witchy" && c.id != "disk");
        let plan = s.plan().unwrap();
        let mut events = Vec::new();
        let pw = Secret::new("fixture-pass".into()).unwrap();
        let no = AtomicBool::new(false);
        for id in [StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime, StepId::Seamless, StepId::Cleanup, StepId::Profiles] {
            let r = s.run_step(id, Some(&pw), &plan, &no, &mut |e| events.push(e));
            assert!(r.is_ok(), "{id:?}: {r:?}");
        }
        let root = t.0.join("MW2inER");
        assert!(root.join(r"build\natives\mw2er.dll").is_file() && root.join("setup.ps1").is_file());
        assert!(root.join(LOG_DIR).is_dir());
        // the release toml policy forced the two test settings off, the build inputs are untouched
        let toml = std::fs::read_to_string(root.join("mw2er.toml")).unwrap();
        assert!(toml.contains("always_available     = false") && toml.contains("probe_scaleform      = false"), "{toml}");
        assert_eq!(plan::toml_build_sections(&toml), plan::toml_build_sections(crate::friend_toml::fixture::POLICY_TOML));
        assert!(events.iter().any(|e| matches!(e, Progress::Line { text, .. } if text.contains("[nuke] always_available true -> false"))));
        // the bundled runtime of the payload is the build's Python now
        assert_eq!(s.facts.python.exe.as_deref(), Some(payload::runtime_python(&root).as_path()));
        let backups: Vec<_> = std::fs::read_dir(root.join("backups")).unwrap().flatten().collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].path().join(r"7656\ER0000.sl2").is_file());
        let ini = root.join(r"seamless\SeamlessCoop\ersc_settings.ini");
        let st = seamless::read_ini_state(&ini).unwrap();
        assert!(st.keys_ok() && st.password_set);
        let coop = std::fs::read_to_string(root.join(launch::PROFILE_COOP)).unwrap();
        let coop_t: toml::Table = coop.parse().unwrap();
        assert_eq!(coop_t["natives"][0]["path"].as_str(), Some(root.join(r"seamless\SeamlessCoop\ersc.dll").display().to_string().as_str()));
        assert!(!coop.contains("@@"));
        let solo = std::fs::read_to_string(root.join(launch::PROFILE_SOLO)).unwrap();
        assert!(launch::check_profile_text(&solo, Mode::Solo).is_ok());
        // finish: launcher copy + shortcuts + preflight hashes; no registry in tests
        let me = t.write("fake-setup.exe", b"exe");
        s.req.self_exe = Some(me);
        t.write(r"MW2inER\build\package\regulation.bin", b"package regulation");
        t.write(r"MW2inER\converted\weapons.json", b"{}");
        // P4: the shortcuts of a pre-0.3.0 install (old name): the Desktop ones are replaced; the Start-menu one
        // (no Start-menu shortcuts asked for here) stays recorded for the uninstall
        let old_solo = t.write(r"Desktop\MW2 in Elden Ring.lnk", b"old lnk");
        let old_coop = t.write(r"Desktop\MW2 in Elden Ring - Co-op.lnk", b"old lnk");
        let old_menu = t.write(r"Programs\MW2 in Elden Ring\MW2 in Elden Ring - Setup.lnk", b"old lnk");
        let mine = t.write(r"Desktop\My notes.lnk", b"the player's own");
        s.state.shortcuts = vec![old_solo.clone(), old_coop.clone(), old_menu.clone()];
        s.run_step(StepId::Finish, None, &plan, &no, &mut |e| events.push(e)).unwrap();
        assert!(root.join(INSTALLED_EXE).is_file());
        assert!(!old_solo.exists() && !old_coop.exists(), "old Desktop shortcuts replaced");
        assert!(old_menu.is_file() && mine.is_file());
        assert_eq!(s.state.shortcuts.len(), 4, "{:?}", s.state.shortcuts);
        assert!(s.state.shortcuts.contains(&old_menu) && !s.state.shortcuts.contains(&old_solo));
        for f in shortcut_names(crate::DISPLAY_NAME) {
            assert!(t.0.join("Desktop").join(&f).is_file(), "{f}");
        }
        assert!(t.0.join(r"Desktop\Modern Arsenal for Elden Ring - Co-op.lnk").is_file());
        assert_eq!(s.state.preflight.len(), 4);
        assert!(s.state.registry.is_empty());
        s.past_preflight = true; // run_step directly: the loop sets it after the checks
        s.save_state();
        let text = std::fs::read_to_string(root.join(crate::state::STATE_FILE)).unwrap();
        assert!(!text.contains("fixture-pass") && !text.to_lowercase().contains("password"));
        for e in &events {
            assert!(!format!("{e:?}").contains("fixture-pass"));
        }
        // the player's Seamless zip and save are unchanged
        assert_eq!(std::fs::read(t.0.join(r"Roaming\EldenRing\7656\ER0000.sl2")).unwrap(), b"save");
        // a co-op rerun without a password keeps the one in the copy
        s.run_step(StepId::Seamless, None, &plan, &no, &mut |_| {}).unwrap();
        assert!(seamless::read_ini_state(&ini).unwrap().password_set);
    }

    #[test]
    fn coop_without_password_fails_cleanly() {
        let t = TempDir::new("sess-nopw");
        let mut s = session(&t, Mode::Coop);
        let plan = s.plan().unwrap();
        let e = s.run_step(StepId::Seamless, None, &plan, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.msg.contains("no co-op password"));
    }

    #[test]
    fn install_stops_at_a_blocking_check() {
        let t = TempDir::new("sess-block");
        let mut s = session(&t, Mode::Solo);
        s.facts.er.exe_version = Some("2.7.3.0".into());
        s.checks = detect::evaluate(&s.facts, &s.release.pins, true);
        let mut ev = Vec::new();
        let e = s.install(None, &AtomicBool::new(false), &mut |x| ev.push(x)).unwrap_err();
        assert_eq!(e.step, Some(StepId::Preflight));
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: false, .. })));
        assert!(!t.0.join("MW2inER").exists(), "nothing written, not even the state or a log");
        let cancel = AtomicBool::new(true);
        s.facts.er.exe_version = Some("2.7.2.0".into());
        s.checks = detect::evaluate(&s.facts, &s.release.pins, true);
        s.checks.retain(|c| c.id != "witchy" && c.id != "disk");
        let e = s.install(None, &cancel, &mut |_| {}).unwrap_err();
        assert!(e.cancelled());
    }

    /// The whole pipeline on fake tools: step order, tool order, progress, logs, the password never written
    /// anywhere but the ini, then a rerun that skips every unchanged build step.
    #[test]
    fn full_install_on_fake_tools_then_skip_if_unchanged() {
        let t = TempDir::new("sess-full");
        let fake = Fake::default();
        let mut s = ready(&t, Mode::Coop, &fake);
        let pw = Secret::new("hunter2-fixture".into()).unwrap();
        let mut ev = Vec::new();
        s.install(Some(&pw), &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
        assert_eq!(fake.calls(), BUILD_ORDER);
        let all: Vec<StepId> = s.plan().unwrap().steps.iter().map(|x| x.id).collect();
        assert_eq!(started(&ev), all, "every step, in plan order");
        assert_eq!(all.len(), 14);
        // the dumps progress reaches 1.0 in order; the converters' too
        let fr: Vec<f32> = ev.iter().filter_map(|e| if let Progress::Fraction { id: StepId::Dumps, value } = e { Some(*value) } else { None }).collect();
        assert_eq!(fr.len(), DUMP_LINES);
        assert!(fr.windows(2).all(|w| w[0] < w[1]) && *fr.last().unwrap() == 1.0);
        assert!(ev.iter().any(|e| matches!(e, Progress::Fraction { id: StepId::Converters, value } if *value == 1.0)));
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: true, msg }) if msg.contains(&s.release.coop_id())));
        let root = t.0.join("MW2inER");
        assert!(!root.join("dumps").exists() && !root.join("work").exists(), "cleanup");
        assert!(root.join(launch::PROFILE_SOLO).is_file() && root.join(launch::PROFILE_COOP).is_file());
        let st = InstallState::load(&root).unwrap();
        for k in ["payload", "converters", "regulation"] {
            assert!(st.step_key(k).is_some(), "{k} recorded");
        }
        assert!(st.step_key("dumps").is_none(), "the cleanup removed the dumps");
        assert!(st.last_result.starts_with("PASS"));
        // logs: the session log has every step; no password anywhere in the root except the ini copy
        let log = std::fs::read_to_string(root.join(LOG_DIR).join(SESSION_LOG)).unwrap();
        assert!(log.contains("==== MW2inER setup") && log.contains("STEP 7/14 dumps") && log.contains("END  INSTALL PASSED"), "{log}");
        for f in util::walk_files(&root).unwrap() {
            let b = std::fs::read(&f).unwrap();
            let has = b.windows(15).any(|w| w == b"hunter2-fixture");
            assert_eq!(has, f.ends_with(r"seamless\SeamlessCoop\ersc_settings.ini"), "{}", f.display());
        }
        // rerun (a new session, no password typed): the unchanged build is skipped, only checks + verifies run
        let mut s2 = reprepare(&t, Mode::Coop, &fake);
        let mut ev2 = Vec::new();
        s2.install(None, &AtomicBool::new(false), &mut |e| ev2.push(e)).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "verify_all"]);
        let sk = skipped(&ev2);
        for id in [StepId::Payload, StepId::Witchy, StepId::Dumps, StepId::Converters, StepId::Regulation] {
            assert!(sk.contains(&id), "{id:?} skipped: {sk:?}");
        }
        assert!(std::fs::read_to_string(root.join(LOG_DIR).join(SESSION_LOG)).unwrap().matches("==== MW2inER setup").count() == 2);
    }

    /// A tool fails: the install stops at that step, the later tools never run, and the next run resumes there
    /// (the finished dumps are reused, the failed converters rerun from scratch).
    #[test]
    fn failure_stops_and_the_rerun_resumes() {
        let t = TempDir::new("sess-fail");
        let mut fake = Fake { fail: Some("convert_ac130".into()), ..Default::default() };
        let mut s = ready(&t, Mode::Solo, &fake);
        let mut ev = Vec::new();
        let e = s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap_err();
        assert_eq!(e.step, Some(StepId::Converters));
        assert!(e.msg.contains("convert_ac130 exit Some(1)") && e.msg.contains(LOG_DIR), "{}", e.msg);
        assert_eq!(fake.calls(), &BUILD_ORDER[..5]);
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: false, msg }) if msg.contains("INSTALL FAILED")));
        let root = t.0.join("MW2inER");
        let st = InstallState::load(&root).unwrap();
        assert!(st.step_key("payload").is_some() && st.step_key("dumps").is_some());
        assert!(st.step_key("converters").is_none() && st.step_key("regulation").is_none());
        assert!(st.last_result.starts_with("FAIL converters"));
        assert!(root.join(plan::DUMPS_COMPLETE.replace('/', "\\")).is_file());
        assert!(std::fs::read_to_string(root.join(LOG_DIR).join(SESSION_LOG)).unwrap().contains("FAIL converters"));
        // fixed: resume
        fake.fail = None;
        let mut s2 = reprepare(&t, Mode::Solo, &fake);
        let mut ev2 = Vec::new();
        s2.install(None, &AtomicBool::new(false), &mut |e| ev2.push(e)).unwrap();
        assert_eq!(fake.calls(), &BUILD_ORDER[1..], "dumps reused");
        assert!(skipped(&ev2).contains(&StepId::Dumps) && skipped(&ev2).contains(&StepId::Payload));
    }

    /// Cancel while a tool runs: the step stops, nothing after it runs, finished steps stay recorded, and the next
    /// run continues from the cancelled step.
    #[test]
    fn cancel_mid_step_then_resume() {
        let t = TempDir::new("sess-cancel");
        let mut fake = Fake { cancel_in: Some("convert_viewmodel".into()), ..Default::default() };
        let mut s = ready(&t, Mode::Solo, &fake);
        let flag = AtomicBool::new(false);
        let mut ev = Vec::new();
        let e = s.install(None, &flag, &mut |e| ev.push(e)).unwrap_err();
        assert!(e.cancelled() && e.step == Some(StepId::Converters), "{e}");
        assert_eq!(fake.calls(), &BUILD_ORDER[..4]);
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: false, msg }) if msg.contains("INSTALL CANCELLED during converters")));
        let st = InstallState::load(&t.0.join("MW2inER")).unwrap();
        assert!(st.last_result.starts_with("CANCELLED at converters"));
        assert!(st.step_key("dumps").is_some() && st.step_key("converters").is_none());
        fake.cancel_in = None;
        let mut s2 = reprepare(&t, Mode::Solo, &fake);
        s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), &BUILD_ORDER[1..]);
    }

    /// The program dies in the middle of the converters (after an earlier full install): the record says so, the
    /// half-written step is not trusted (its record was dropped before it ran), and the next run redoes exactly it.
    #[test]
    fn crash_mid_step_then_resume() {
        let t = TempDir::new("sess-crash");
        let mut fake = Fake::default();
        let mut s = ready(&t, Mode::Solo, &fake);
        s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        fake.calls();
        // an update that changes the converter inputs, and the PC dies during convert_movement
        t.write(r"pkg\tools\scripts\convert_weapons.py", "# convert_weapons.py v2\n");
        fake.crash_in = Some("convert_movement".into());
        let mut s2 = reprepare(&t, Mode::Solo, &fake);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s2.install(None, &AtomicBool::new(false), &mut |_| {})));
        assert!(r.is_err(), "crashed");
        assert_eq!(fake.calls(), &BUILD_ORDER[..6]);
        let root = t.0.join("MW2inER");
        let st = InstallState::load(&root).unwrap();
        assert!(st.last_result.starts_with("INTERRUPTED during converters"), "{}", st.last_result);
        assert!(st.step_key("converters").is_none(), "not trusted after the crash");
        assert!(st.step_key("dumps").is_some() && st.step_key("payload").is_some());
        assert!(root.join(r"converted\half_written.bin").is_file());
        let log = std::fs::read_to_string(root.join(LOG_DIR).join(SESSION_LOG)).unwrap();
        let last_session = log.rsplit("==== MW2inER setup").next().unwrap();
        assert!(last_session.contains("STEP 7/13 converters") && !last_session.contains("END "), "{log}");
        // the next run: dumps reused, converters redone from a clean converted\, then the rest
        fake.crash_in = None;
        let mut s3 = reprepare(&t, Mode::Solo, &fake);
        s3.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), &BUILD_ORDER[1..]);
        assert!(!root.join(r"converted\half_written.bin").exists());
        assert!(InstallState::load(&root).unwrap().last_result.starts_with("PASS"));
    }

    /// Hash refusals: converted output that differs from the release (even when the Python verifier is fooled),
    /// a damaged payload file, a replaced Unlinker, a wrong CPython zip.
    #[test]
    fn hash_refusals() {
        // (a) wrong converted bytes: the Rust re-hash refuses; the regulation never runs; the build records drop
        let t = TempDir::new("sess-hash-a");
        let fake = Fake { wrong_output: true, ..Default::default() };
        let mut s = ready(&t, Mode::Solo, &fake);
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::VerifyConverted));
        assert!(e.msg.contains("differ from the release (1 file(s))"), "{}", e.msg);
        assert_eq!(fake.calls(), &BUILD_ORDER[..9]);
        let st = InstallState::load(&t.0.join("MW2inER")).unwrap();
        assert!(st.step_key("converters").is_none() && st.step_key("regulation").is_none());

        // (b) a payload file that does not match release.json's hash: nothing after the payload runs
        let t = TempDir::new("sess-hash-b");
        let fake = Fake::default();
        let mut s = ready(&t, Mode::Solo, &fake);
        s.release.payload.insert("tools/scripts/friend_dump.py".into(), util::sha256_bytes(b"other"));
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::Payload));
        assert!(e.msg.contains("tools/scripts/friend_dump.py is damaged"), "{}", e.msg);
        assert!(fake.calls().is_empty());

        // (c) an Unlinker that is not the pinned build: refused before it runs
        let t = TempDir::new("sess-hash-c");
        let fake = Fake::default();
        let mut s = ready(&t, Mode::Solo, &fake);
        t.write(r"pkg\tools\oat\bin\Unlinker.exe", b"tampered");
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::Dumps));
        assert!(e.msg.contains("Unlinker.exe is not the release's build"), "{}", e.msg);
        assert!(fake.calls().is_empty(), "the Unlinker never ran");

        // (d) a CPython zip in the payload that is not the official one: blocked before anything is written
        let t = TempDir::new("sess-hash-d");
        let (steam, _) = crate::steam::fixture::steam(&t, "25827938", "25374375");
        let pkg = crate::release::fixture::package(&t);
        t.write(r"Local\Programs\garyttierney\me3\bin\me3.exe", b"me3");
        t.write(&format!(r"pkg\runtime\{}", payload::PYTHON_EMBED_NAME), b"not python");
        let mut s = Session::prepare(env(&t, &steam), request(&t, pkg, Mode::Solo)).unwrap();
        assert!(s.blocking().iter().any(|c| c.id == "python.bundled" && c.msg.contains("refused")), "{:#?}", s.checks);
        assert!(s.python_zip.is_none());
        assert!(s.install(None, &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(!t.0.join("MW2inER").exists());
    }

    /// P2: a release made with regbuild installs with the in-process builder: no WitchyBND row, step, download or
    /// zip, no build_regulation.py; its log, record and co-op id; an unchanged rerun skips it.
    #[test]
    fn p2_release_installs_with_the_rust_builder_and_no_witchybnd() {
        let t = TempDir::new("sess-p2");
        let fake = Fake::default();
        let mut s = ready_p2(&t, Mode::Solo, &fake, None);
        assert_eq!(s.regulation_tool, Tool::Regbuild);
        assert!(!s.checks.iter().any(|c| c.id == "witchy" || c.id == "regulation.tool"), "{:#?}", s.checks);
        assert!(s.blocking().is_empty(), "{:#?}", s.blocking());
        assert!(s.plan().unwrap().get(StepId::Witchy).is_none());
        let mut ev = Vec::new();
        s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
        assert_eq!(fake.calls(), P2_ORDER);
        assert_eq!(started(&ev).len(), 12);
        assert!(!started(&ev).contains(&StepId::Witchy));
        let fr: Vec<f32> = ev.iter().filter_map(|e| if let Progress::Fraction { id: StepId::Regulation, value } = e { Some(*value) } else { None }).collect();
        assert_eq!(fr, [0.05, 0.1, 0.9, 1.0]);
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: true, msg }) if msg.contains(&s.release.coop_id())));
        let root = t.0.join("MW2inER");
        assert!(!root.join(r"tools\WitchyBND\app").exists() && !root.join("downloads").exists(), "nothing of WitchyBND");
        let log = std::fs::read_to_string(root.join(LOG_DIR).join(regulation::LOG_NAME)).unwrap();
        assert!(log.contains("builder: regbuild") && log.contains("REGULATION PASS (regbuild"), "{log}");
        let st = InstallState::load(&root).unwrap();
        assert_eq!(st.regulation_tool, "regbuild");
        assert_eq!(st.coop_id, s.release.coop_id());
        assert!(st.step_key("regulation").is_some());
        // rerun: the regulation is skipped
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake, None);
        let mut ev2 = Vec::new();
        s2.install(None, &AtomicBool::new(false), &mut |e| ev2.push(e)).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "verify_all"]);
        assert!(skipped(&ev2).contains(&StepId::Regulation));
    }

    /// P2: a failed or cancelled in-process build stops the install at the regulation; nothing is recorded for it.
    #[test]
    fn p2_regbuild_failure_and_cancel() {
        let t = TempDir::new("sess-p2-fail");
        let mut fake = Fake { fail: Some("regbuild".into()), ..Default::default() };
        let mut s = ready_p2(&t, Mode::Solo, &fake, None);
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::Regulation));
        assert!(e.msg.contains("game regulation changed") && e.msg.contains(regulation::LOG_NAME), "{}", e.msg);
        let root = t.0.join("MW2inER");
        let st = InstallState::load(&root).unwrap();
        assert!(st.step_key("regulation").is_none() && st.step_key("converters").is_some());
        assert!(std::fs::read_to_string(root.join(LOG_DIR).join(regulation::LOG_NAME)).unwrap().contains("FAIL game regulation changed"));
        fake.calls();
        fake.fail = None;
        fake.cancel_in = Some("regbuild".into());
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake, None);
        let e = s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.cancelled() && e.step == Some(StepId::Regulation), "{e}");
        assert_eq!(fake.calls(), ["verify_converted", "regbuild"], "the converted data was reused");
        fake.cancel_in = None;
        reprepare_p2(&t, Mode::Solo, &fake, None).install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "regbuild", "verify_all"]);
    }

    /// P2 fallback: `--regulation-tool witchy` on a regbuild release brings back the WitchyBND row / step and
    /// build_regulation.py; the regulation is verified by content (same params, other bytes) and the co-op id
    /// changes; a release without the content pins refuses the fallback; other params fail the verify.
    #[test]
    fn p2_witchybnd_fallback_is_verified_by_content() {
        let vanilla = util::sha256_bytes(b"vanilla regulation"); // the fixture release's pin
        let host = regulation::fixture::regulation(4, 21);
        let host_rows = regulation::fixture::rows("regbuild 0.2.0", &util::sha256_bytes(&host), &vanilla, 4.0);
        let other = regulation::fixture::regulation(4, 3);
        let other_rows = regulation::fixture::rows("build_regulation.py", &util::sha256_bytes(&other), &vanilla, 4.0);
        // without content pins: refused up front
        let t = TempDir::new("sess-p2-fb0");
        let fake = Fake::default();
        let mut s = ready_p2(&t, Mode::Solo, &fake, None);
        s.set_regulation_tool(Some(Tool::Witchy));
        assert!(s.blocking().iter().any(|c| c.id == "regulation.tool" && c.msg.contains("--regulation-tool regbuild")), "{:#?}", s.checks);
        assert!(s.checks.iter().any(|c| c.id == "witchy"), "the WitchyBND row is back");
        s.set_regulation_tool(None);
        assert!(!s.checks.iter().any(|c| c.id == "witchy" || c.id == "regulation.tool"));

        let t = TempDir::new("sess-p2-fb");
        let fake = Fake { witchy_out: Some(Arc::new((other.clone(), other_rows.clone()))), ..Default::default() };
        let mut s = ready_p2(&t, Mode::Solo, &fake, Some((&host, &host_rows)));
        prime_witchy(&t, &s.release.pins);
        s.set_regulation_tool(Some(Tool::Witchy));
        let row = s.checks.iter().find(|c| c.id == "regulation.tool").expect("the fallback row");
        assert_eq!(row.level, Level::Warn);
        s.checks.retain(|c| c.id != "witchy");
        assert!(s.blocking().is_empty(), "{:#?}", s.blocking());
        let mut ev = Vec::new();
        s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
        assert_eq!(fake.calls(), BUILD_ORDER);
        assert!(started(&ev).contains(&StepId::Witchy));
        assert!(ev.iter().any(|e| matches!(e, Progress::Line { id: StepId::VerifyAll, text } if text.contains("only in its compression"))));
        let root = t.0.join("MW2inER");
        let st = InstallState::load(&root).unwrap();
        assert_eq!(st.regulation_tool, "witchybnd");
        assert_ne!(st.coop_id, s.release.coop_id(), "the fallback install is not byte-identical to the host's");
        assert_eq!(st.coop_id, regulation::coop_id(&s.release, Tool::Witchy, Some(&util::sha256_bytes(&other))));
        assert!(matches!(ev.last(), Some(Progress::Finished { ok: true, msg }) if msg.contains(&st.coop_id)));
        let vm = std::fs::read_to_string(root.join(VERIFY_MANIFEST)).unwrap();
        assert!(!vm.contains("build/package/regulation.bin") && vm.contains("converted/weapons.json"), "{vm}");
        // the default again (no flag): the regulation is rebuilt with regbuild, checked the strict way
        let fake2 = Fake { regbuild_out: Some(Arc::new((host.clone(), host_rows.clone()))), ..Default::default() };
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake2, None);
        s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake2.calls(), ["verify_converted", "regbuild", "verify_all"]);
        assert_eq!(InstallState::load(&root).unwrap().coop_id, s2.release.coop_id());

        // the fallback building other params: verify all fails, the regulation record is dropped
        let t = TempDir::new("sess-p2-fb2");
        let bad = regulation::fixture::regulation(5, 3);
        let bad_rows = regulation::fixture::rows("build_regulation.py", &util::sha256_bytes(&bad), &vanilla, 4.0);
        let fake = Fake { witchy_out: Some(Arc::new((bad, bad_rows))), ..Default::default() };
        let mut s = ready_p2(&t, Mode::Solo, &fake, Some((&host, &host_rows)));
        prime_witchy(&t, &s.release.pins);
        s.set_regulation_tool(Some(Tool::Witchy));
        s.checks.retain(|c| c.id != "witchy");
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::VerifyAll));
        assert!(e.msg.contains("params differ"), "{}", e.msg);
        assert!(InstallState::load(&t.0.join("MW2inER")).unwrap().step_key("regulation").is_none());
    }

    // ---- P3: the update path ---------------------------------------------------------------------------------------

    /// Turns t\pkg into release `version` in the release.json layout (pins + host manifest + the sha256 of every
    /// payload file + the locked toml keys), as make_release writes it.
    fn release_json(t: &TempDir, version: &str) {
        let pkg = t.0.join("pkg");
        let read = |rel: &str| -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(pkg.join(rel)).unwrap()).unwrap() };
        let payload: std::collections::BTreeMap<String, String> = plan::payload_files(&pkg)
            .into_iter()
            .filter(|(rel, _)| rel != "manifest/release.json")
            .map(|(rel, p)| (rel, util::sha256_file(&p).unwrap()))
            .collect();
        let rj = serde_json::json!({
            "what": "test release",
            "format": 1,
            "release_version": version,
            "pins": read(release::HOST_PINS),
            "manifest": read(release::HOST_MANIFEST),
            "payload": payload,
            "locked_toml": ["damage", "regulation*", "arsenal.guns", "launchers.explosion_tiers"],
        });
        t.write(r"pkg\manifest\release.json", serde_json::to_string_pretty(&rj).unwrap());
    }

    /// A new mw2er.dll in t\pkg (its pin and manifest entry follow, as in a real release).
    fn new_dll(t: &TempDir, bytes: &[u8]) {
        t.write(r"pkg\build\natives\mw2er.dll", bytes);
        let edit = |rel: &str, f: &dyn Fn(&mut serde_json::Value)| {
            let p = t.0.join("pkg").join(rel);
            let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            f(&mut v);
            std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        };
        edit(release::HOST_PINS, &|v| v["mw2er_dll_sha256"] = util::sha256_bytes(bytes).into());
        edit(release::HOST_MANIFEST, &|v| v["files"]["build/natives/mw2er.dll"] = serde_json::json!({"sha256": util::sha256_bytes(bytes), "size": bytes.len()}));
    }

    /// The 0.2.0 install the update tests start from (release.json layout, regbuild, solo).
    fn installed_020(t: &TempDir, fake: &Fake) -> PathBuf {
        let _ = ready_p2(t, Mode::Solo, fake, None);
        release_json(t, "0.2.0");
        let mut s = reprepare_p2(t, Mode::Solo, fake, None);
        assert_eq!(s.update_from, None, "a fresh install");
        assert_eq!(s.update_summary(&s.plan().unwrap()).unwrap().kind, update::Kind::Fresh);
        s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), P2_ORDER);
        let root = t.0.join("MW2inER");
        assert_eq!(std::fs::read_to_string(root.join(RELEASE_TOML)).unwrap(), crate::friend_toml::fixture::POLICY_TOML, "the merge base");
        root
    }

    fn step_msg(ev: &[Progress], want: StepId) -> String {
        ev.iter().find_map(|e| if let Progress::StepDone { id, msg, .. } = e { (*id == want).then(|| msg.clone()) } else { None }).unwrap_or_default()
    }

    /// The acceptance of P3 on fixtures: a DLL-only release updates without any rebuild (only the changed files are
    /// copied), the player's hot setting survives, the co-op id change is announced, and Roll back restores the old
    /// DLL, settings and record.
    #[test]
    fn p3_dll_only_update_keeps_settings_and_rolls_back() {
        let t = TempDir::new("sess-p3-dll");
        let fake = Fake::default();
        let root = installed_020(&t, &fake);
        let old_dll = std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap();
        let old_id = InstallState::load(&root).unwrap().coop_id;
        // the player moves the shoulder camera (a hot setting)
        let toml_p = root.join("mw2er.toml");
        let player = std::fs::read_to_string(&toml_p).unwrap().replace("[0.55, 0.25, 0.0]", "[0.6, 0.3, 0.05]");
        std::fs::write(&toml_p, &player).unwrap();
        let script = root.join(r"tools\scripts\convert_weapons.py");
        let script_time = std::fs::metadata(&script).unwrap().modified().unwrap();
        // release 0.2.1: only the DLL bytes differ
        new_dll(&t, b"fake mw2er.dll 0.2.1");
        release_json(&t, "0.2.1");
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        assert_eq!(s.update_from.as_deref(), Some("0.2.0"));
        let plan = s.plan().unwrap();
        let sum = s.update_summary(&plan).unwrap();
        assert_eq!(sum.headline(), "Update 0.2.0 -> 0.2.1");
        assert_eq!(sum.build_steps(), vec![StepId::Payload], "no dumps, converters or regulation");
        assert_eq!(sum.changes.iter().map(|c| c.what.as_str()).collect::<Vec<_>>(), ["The mod (mw2er.dll)"]);
        assert!(sum.coop_changes() && sum.coop_note().unwrap().contains("both players must install 0.2.1"), "{:?}", sum);
        assert!(sum.settings.as_deref().unwrap().contains("1 of your setting(s) kept"), "{:?}", sum.settings);
        let mut ev = Vec::new();
        let t0 = std::time::Instant::now();
        s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
        assert!(t0.elapsed().as_secs() < 30);
        assert_eq!(fake.calls(), ["verify_converted", "verify_all"], "nothing rebuilt");
        let msg = step_msg(&ev, StepId::Payload);
        assert!(msg.contains("already up to date") && msg.contains("1 of your setting(s) kept"), "{msg}");
        assert!(ev.iter().any(|e| matches!(e, Progress::Line { text, .. } if text.starts_with("NOTE The co-op id changes"))));
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"fake mw2er.dll 0.2.1");
        assert_eq!(std::fs::metadata(&script).unwrap().modified().unwrap(), script_time, "an unchanged file is not rewritten");
        let merged = std::fs::read_to_string(&toml_p).unwrap();
        assert!(merged.contains("[0.6, 0.3, 0.05]"), "the hot setting survived:\n{merged}");
        assert!(merged.contains("always_available     = false"), "the policy still applies");
        assert!(std::fs::read_dir(root.join(SETTINGS_BACKUPS)).unwrap().count() == 1, "the old toml is backed up");
        let st = InstallState::load(&root).unwrap();
        assert_eq!((st.release_version.as_str(), st.update_from.as_str()), ("0.2.1", ""));
        assert_ne!(st.coop_id, old_id);
        assert!(!root.join(r"build\natives\mw2er.dll.p3new").exists());
        let store = rollback::available(&root).expect("a rollback store");
        assert!(store.rec.complete && store.rec.from_version == "0.2.0" && store.rec.to_coop_id == st.coop_id);
        assert_eq!(std::fs::read(store.dir.join(r"files\build\natives\mw2er.dll")).unwrap(), old_dll);
        // Roll back
        let p = rollback::plan(&root).unwrap();
        assert!(p.restore.contains(&"build/natives/mw2er.dll".to_string()) && p.restore.contains(&"mw2er.toml".to_string()), "{:?}", p.restore);
        rollback::execute(&p, &s.guard, None).unwrap();
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), old_dll, "the old DLL is back");
        assert_eq!(std::fs::read_to_string(&toml_p).unwrap(), player, "the player's file as it was");
        assert_eq!(std::fs::read_to_string(root.join(RELEASE_TOML)).unwrap(), crate::friend_toml::fixture::POLICY_TOML, "0.2.0's merge base is back");
        let back = InstallState::load(&root).unwrap();
        assert_eq!((back.release_version.as_str(), back.coop_id.as_str()), ("0.2.0", old_id.as_str()));
        assert!(rollback::available(&root).is_none());
        // the update again: the rolled-back install is updated like the first time
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake, None);
        assert_eq!(s2.update_from.as_deref(), Some("0.2.0"));
        s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "verify_all"]);
    }

    /// An update that changes a regulation build input rebuilds only the regulation (the old one is kept for a
    /// rollback); cancelled half-way it resumes as the same update, and it can be rolled back.
    #[test]
    fn p3_rows_update_rebuilds_only_the_regulation_and_resumes() {
        let t = TempDir::new("sess-p3-rows");
        let mut fake = Fake::default();
        let root = installed_020(&t, &fake);
        let old_reg = std::fs::read(root.join(regulation::PACKAGE)).unwrap();
        let rel_toml = t.0.join(r"pkg\mw2er.toml");
        std::fs::write(&rel_toml, std::fs::read_to_string(&rel_toml).unwrap().replace("scale = 1.0", "scale = 1.5")).unwrap();
        release_json(&t, "0.2.1");
        fake.regbuild_out = Some(Arc::new((b"package regulation".to_vec(), "{}".into())));
        fake.cancel_in = Some("regbuild".into());
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        let sum = s.update_summary(&s.plan().unwrap()).unwrap();
        assert_eq!(sum.build_steps(), vec![StepId::Payload, StepId::Regulation], "{:?}", sum.steps);
        assert!(sum.changes.iter().any(|c| c.what.starts_with("Regulation build settings") && c.detail == "[damage]"), "{:?}", sum.changes);
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.cancelled() && e.step == Some(StepId::Regulation));
        assert_eq!(fake.calls(), ["verify_converted", "regbuild"], "the converters did not rerun");
        let st = InstallState::load(&root).unwrap();
        assert_eq!((st.release_version.as_str(), st.update_from.as_str()), ("0.2.1", "0.2.0"), "an update in progress");
        let store = rollback::available(&root).expect("the interrupted update can be rolled back");
        assert!(!store.rec.complete);
        assert_eq!(std::fs::read(store.dir.join(r"moved\build\package\regulation.bin")).unwrap(), old_reg, "moved, not deleted");
        // resumed: the same update; the payload is done, the regulation is built
        fake.cancel_in = None;
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake, None);
        assert_eq!(s2.update_from.as_deref(), Some("0.2.0"));
        assert_eq!(s2.update_summary(&s2.plan().unwrap()).unwrap().headline(), "Update 0.2.0 -> 0.2.1");
        s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "regbuild", "verify_all"]);
        let st = InstallState::load(&root).unwrap();
        assert!(st.update_from.is_empty() && st.last_result.starts_with("PASS"));
        assert!(std::fs::read_to_string(root.join("mw2er.toml")).unwrap().contains("scale = 1.5"), "the locked build input comes from the release");
        let store = rollback::available(&root).unwrap();
        assert!(store.rec.complete);
        // Roll back: the old regulation and settings
        let p = rollback::plan(&root).unwrap();
        assert_eq!(p.unmove, vec!["build/package".to_string(), "build/rows.json".into()]);
        rollback::execute(&p, &s2.guard, None).unwrap();
        assert_eq!(std::fs::read(root.join(regulation::PACKAGE)).unwrap(), old_reg);
        assert!(std::fs::read_to_string(root.join("mw2er.toml")).unwrap().contains("scale = 1.0"));
        assert_eq!(InstallState::load(&root).unwrap().release_version, "0.2.0");
    }

    /// The release changes a hot setting's default: a player who never touched it gets the new default, a player who
    /// changed it keeps theirs; a key the release dropped is removed.
    #[test]
    fn p3_hot_default_changes_follow_the_release_unless_changed() {
        let t = TempDir::new("sess-p3-hot");
        let fake = Fake::default();
        let root = installed_020(&t, &fake);
        let rel_toml = t.0.join(r"pkg\mw2er.toml");
        let text = std::fs::read_to_string(&rel_toml).unwrap();
        std::fs::write(&rel_toml, text.replace("shoulder_ads_offset_m = [0.45, 0.15, -0.6]", "shoulder_ads_offset_m = [0.5, 0.2, -0.5]").replace("shoulder_offset_m = [0.55, 0.25, 0.0]", "shoulder_offset_m = [0.5, 0.2, 0.0]")).unwrap();
        release_json(&t, "0.2.1");
        // the player changed shoulder_offset_m, not shoulder_ads_offset_m
        let toml_p = root.join("mw2er.toml");
        std::fs::write(&toml_p, std::fs::read_to_string(&toml_p).unwrap().replace("[0.55, 0.25, 0.0]", "[0.7, 0.3, 0.1]")).unwrap();
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        assert!(s.update_summary(&s.plan().unwrap()).unwrap().build_steps() == vec![StepId::Payload]);
        let mut ev = Vec::new();
        s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
        assert_eq!(fake.calls(), ["verify_converted", "verify_all"]);
        let merged = std::fs::read_to_string(&toml_p).unwrap();
        assert!(merged.contains("shoulder_offset_m = [0.7, 0.3, 0.1]"), "{merged}");
        assert!(merged.contains("shoulder_ads_offset_m = [0.5, 0.2, -0.5]"), "the new default: {merged}");
        assert!(ev.iter().any(|e| matches!(e, Progress::Line { text, .. } if text == "mw2er.toml new default: [third_person] shoulder_ads_offset_m")));
        assert_eq!(std::fs::read_to_string(root.join(RELEASE_TOML)).unwrap(), std::fs::read_to_string(&rel_toml).unwrap(), "the next base");
    }

    /// Writes a 0.2.1 release in t\pkg whose only change is the default of the hot `shoulder_ads_offset_m`.
    fn release_with_new_ads_default(t: &TempDir) -> PathBuf {
        let rel_toml = t.0.join(r"pkg\mw2er.toml");
        let text = std::fs::read_to_string(&rel_toml).unwrap();
        assert!(text.contains(ADS_OLD));
        std::fs::write(&rel_toml, text.replace(ADS_OLD, ADS_NEW)).unwrap();
        release_json(t, "0.2.1");
        rel_toml
    }
    const ADS_OLD: &str = "shoulder_ads_offset_m = [0.45, 0.15, -0.6]";
    const ADS_NEW: &str = "shoulder_ads_offset_m = [0.5, 0.2, -0.5]";

    /// Roll back restores the merge base too (state\release_mw2er.toml), keeps a hot setting the player changed
    /// after the update (the current file is backed up first), and the next update still gives an untouched setting
    /// the new release's default.
    #[test]
    fn p3_rollback_restores_the_base_and_keeps_later_edits() {
        let t = TempDir::new("sess-p3-base");
        let fake = Fake::default();
        let root = installed_020(&t, &fake);
        let rel_toml = release_with_new_ads_default(&t);
        let toml_p = root.join("mw2er.toml");
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(std::fs::read_to_string(&toml_p).unwrap().contains(ADS_NEW));
        assert_eq!(std::fs::read_to_string(root.join(RELEASE_TOML)).unwrap(), std::fs::read_to_string(&rel_toml).unwrap());
        assert!(rollback::plan(&root).unwrap().restore.contains(&"state/release_mw2er.toml".to_string()));
        // after the update the player moves the shoulder camera, then rolls back
        std::fs::write(&toml_p, std::fs::read_to_string(&toml_p).unwrap().replace("[0.55, 0.25, 0.0]", "[0.9, 0.3, 0.1]")).unwrap();
        let lines = rollback::execute(&rollback::plan(&root).unwrap(), &s.guard, None).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("your current mw2er.toml is backed up as")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("kept 1 setting(s) you changed after the update")), "{lines:?}");
        assert_eq!(std::fs::read_dir(root.join(SETTINGS_BACKUPS)).unwrap().count(), 2, "the update's and the rollback's backup");
        assert_eq!(std::fs::read_to_string(root.join(RELEASE_TOML)).unwrap(), crate::friend_toml::fixture::POLICY_TOML, "0.2.0's base is back");
        let back = std::fs::read_to_string(&toml_p).unwrap();
        assert!(back.contains(ADS_OLD) && back.contains("[0.9, 0.3, 0.1]"), "{back}");
        // the update again: the untouched default follows 0.2.1 (it did not with the stale base), the edit stays
        let mut s2 = reprepare_p2(&t, Mode::Solo, &fake, None);
        s2.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let again = std::fs::read_to_string(&toml_p).unwrap();
        assert!(again.contains(ADS_NEW) && again.contains("[0.9, 0.3, 0.1]"), "{again}");
    }

    /// An install made before P3 has no recorded base: when the player's file is still exactly the installed
    /// release's (its release.json hash) it is the base, so new defaults apply; otherwise the summary says they
    /// cannot be told apart. A rollback deletes the base the update added.
    #[test]
    fn p3_update_of_a_pre_p3_install_finds_its_base() {
        for edited in [false, true] {
            let t = TempDir::new(if edited { "sess-p3-nob-e" } else { "sess-p3-nob" });
            let fake = Fake::default();
            let root = installed_020(&t, &fake);
            std::fs::remove_file(root.join(RELEASE_TOML)).unwrap();
            let toml_p = root.join("mw2er.toml");
            // a real release stages its mw2er.toml under the policy already (release_tool.py): the installed file is
            // then byte-for-byte the release's (the fixture's is not; make it so, as 0.2.0 recorded it)
            std::fs::copy(&toml_p, t.0.join(r"pkg\mw2er.toml")).unwrap();
            release_json(&t, "0.2.0");
            std::fs::copy(t.0.join(r"pkg\manifest\release.json"), root.join(release::RELEASE_JSON)).unwrap();
            if edited {
                std::fs::write(&toml_p, std::fs::read_to_string(&toml_p).unwrap().replace("[0.55, 0.25, 0.0]", "[0.9, 0.3, 0.1]")).unwrap();
            }
            release_with_new_ads_default(&t);
            let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
            let sum = s.update_summary(&s.plan().unwrap()).unwrap().settings.unwrap();
            assert_eq!(sum.contains(NO_BASE), edited, "{sum}");
            let mut ev = Vec::new();
            s.install(None, &AtomicBool::new(false), &mut |e| ev.push(e)).unwrap();
            assert_eq!(step_msg(&ev, StepId::Payload).contains(NO_BASE), edited);
            let merged = std::fs::read_to_string(&toml_p).unwrap();
            assert_eq!(merged.contains(ADS_NEW), !edited, "{merged}");
            assert!(root.join(RELEASE_TOML).is_file());
            rollback::execute(&rollback::plan(&root).unwrap(), &s.guard, None).unwrap();
            assert!(!root.join(RELEASE_TOML).exists(), "added by the update: removed by the rollback");
        }
    }

    /// Never while the game runs: the update stops at the checks; nothing is changed, no store is made.
    #[test]
    fn p3_update_refused_while_the_game_runs() {
        let t = TempDir::new("sess-p3-er");
        let fake = Fake::default();
        let root = installed_020(&t, &fake);
        new_dll(&t, b"fake mw2er.dll 0.2.1");
        release_json(&t, "0.2.1");
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        assert!(running_game_check(|_| false).is_none());
        // the game starts after the detection (the window was open): install checks again, live
        s.is_running = |_| false;
        s.refresh_running();
        assert!(s.blocking().is_empty(), "not running at the detection");
        s.is_running = |e| e == "eldenring.exe";
        let e = s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert_eq!(e.step, Some(StepId::Preflight));
        assert!(e.msg.contains("Elden Ring is running"), "{}", e.msg);
        assert!(fake.calls().is_empty());
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"fake mw2er.dll");
        assert!(!root.join(rollback::DIR).exists());
        assert_eq!(InstallState::load(&root).unwrap().release_version, "0.2.0");
    }

    /// A same-version repair keeps the player's settings too (it used to reset the file to the release's).
    #[test]
    fn p3_repair_keeps_settings_and_makes_no_store() {
        let t = TempDir::new("sess-p3-repair");
        let fake = Fake::default();
        let root = installed_020(&t, &fake);
        let toml_p = root.join("mw2er.toml");
        std::fs::write(&toml_p, std::fs::read_to_string(&toml_p).unwrap().replace("[0.55, 0.25, 0.0]", "[0.6, 0.3, 0.05]")).unwrap();
        let mut s = reprepare_p2(&t, Mode::Solo, &fake, None);
        s.req.force = true;
        assert_eq!(s.update_from, None);
        assert_eq!(s.update_summary(&s.plan().unwrap()).unwrap().headline(), "Repair 0.2.0");
        s.install(None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(std::fs::read_to_string(&toml_p).unwrap().contains("[0.6, 0.3, 0.05]"));
        assert!(!root.join(rollback::DIR).exists(), "no rollback store for a repair");
    }

    #[test]
    fn events_are_redacted_and_logged() {
        let pw = Secret::new("s3cret-word".into()).unwrap();
        let e = Progress::Line { id: StepId::Seamless, text: "x s3cret-word y".into() }.redacted(Some(&pw));
        assert_eq!(e, Progress::Line { id: StepId::Seamless, text: "x <hidden> y".into() });
        assert_eq!(Progress::Fraction { id: StepId::Dumps, value: 0.5 }.log_line(), None);
        assert_eq!(
            Progress::StepDone { id: StepId::Dumps, ok: false, secs: 1.0, msg: "m".into() }.log_line().unwrap(),
            "FAIL dumps (1.0 s): m"
        );
        assert!(Progress::Plan(vec![(StepId::Preflight, true), (StepId::Dumps, false)]).log_line().unwrap().ends_with("preflight -dumps"));
    }
}
