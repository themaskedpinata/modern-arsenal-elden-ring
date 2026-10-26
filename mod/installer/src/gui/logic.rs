//! The wizard's rules without any egui type (unit-tested): page order, which check row belongs to which page and
//! blocks it, the install progress model fed by [`Progress`] events, the overall progress weights, and the
//! folder choices of the "Choose..." buttons.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use installer::detect::{Check, Level};
use installer::plan::StepId;
use installer::run::Progress;
use installer::seamless;
use installer::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Games,
    Me3,
    Seamless,
    Install,
    Done,
    Uninstall,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Games => "Games",
            Page::Me3 => "me3",
            Page::Seamless => "Seamless Co-op",
            Page::Install => "Install",
            Page::Done => "Done",
            Page::Uninstall => "Uninstall",
        }
    }
}

/// The wizard's pages in order (the Seamless page only for co-op).
pub fn wizard_pages(coop: bool) -> Vec<Page> {
    let mut v = vec![Page::Welcome, Page::Games, Page::Me3];
    if coop {
        v.push(Page::Seamless);
    }
    v.extend([Page::Install, Page::Done]);
    v
}

pub fn next_page(p: Page, coop: bool) -> Page {
    let v = wizard_pages(coop);
    v.iter().position(|x| *x == p).and_then(|i| v.get(i + 1).copied()).unwrap_or(p)
}

pub fn prev_page(p: Page, coop: bool) -> Page {
    let v = wizard_pages(coop);
    match v.iter().position(|x| *x == p) {
        Some(i) if i > 0 => v[i - 1],
        _ => Page::Welcome,
    }
}

/// The page that shows (and is blocked by) a check row.
pub fn page_of(id: &str) -> Page {
    match id {
        "me3" => Page::Me3,
        "seamless" => Page::Seamless,
        "witchy" | "regulation.tool" => Page::Install,
        _ => Page::Games,
    }
}

pub fn rows_for(checks: &[Check], page: Page) -> Vec<&Check> {
    checks.iter().filter(|c| page_of(c.id) == page).collect()
}

/// A red row on this page: Next stays disabled.
pub fn page_blocked(checks: &[Check], page: Page) -> bool {
    rows_for(checks, page).iter().any(|c| c.level == Level::Fail)
}

/// Rough seconds of each step on the host self-test (docs 1.2, MODLOG "installer P1 pipeline"); the overall bar
/// moves by these weights so the long steps (dumps, regulation) are not one fourteenth of it.
pub fn step_weight(id: StepId) -> f32 {
    match id {
        StepId::Dumps => 40.0,
        StepId::Regulation => 50.0,
        StepId::Converters => 12.0,
        StepId::Witchy => 8.0,
        StepId::Payload | StepId::Runtime | StepId::VerifyConverted | StepId::VerifyAll => 4.0,
        StepId::SaveBackup | StepId::Cleanup => 2.0,
        _ => 1.0,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done(String),
    Failed(String),
    Skipped(String),
}

/// The install page's model, fed with the executor's events.
#[derive(Clone, Debug, Default)]
pub struct RunView {
    /// (step, runs) in plan order.
    pub plan: Vec<(StepId, bool)>,
    pub status: BTreeMap<StepId, StepStatus>,
    pub current: Option<StepId>,
    /// 0..1 inside the current step (when the step reports it).
    pub fraction: Option<f32>,
    pub log: VecDeque<String>,
    pub finished: Option<(bool, String)>,
}

pub const LOG_LINES: usize = 500;

impl RunView {
    pub fn push_log(&mut self, l: String) {
        if self.log.len() >= LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(l);
    }

    pub fn apply(&mut self, e: &Progress) {
        if let Some(l) = e.log_line() {
            self.push_log(l.trim_start().to_string());
        }
        match e {
            Progress::Plan(v) => {
                self.plan = v.clone();
                self.status = v.iter().map(|(id, _)| (*id, StepStatus::Pending)).collect();
            }
            Progress::StepStart { id, .. } => {
                self.current = Some(*id);
                self.fraction = None;
                self.status.insert(*id, StepStatus::Running);
            }
            Progress::Line { .. } => {}
            Progress::Fraction { id, value } => {
                if self.current == Some(*id) {
                    self.fraction = Some(value.clamp(0.0, 1.0));
                }
            }
            Progress::StepDone { id, ok, msg, .. } => {
                self.status.insert(*id, if *ok { StepStatus::Done(msg.clone()) } else { StepStatus::Failed(msg.clone()) });
                if self.current == Some(*id) {
                    self.current = None;
                    self.fraction = None;
                }
            }
            Progress::Skipped { id, reason } => {
                self.status.insert(*id, StepStatus::Skipped(reason.clone()));
            }
            Progress::Finished { ok, msg } => {
                self.current = None;
                self.finished = Some((*ok, msg.clone()));
            }
        }
    }

    /// 0..1 over the steps that run, weighted by [`step_weight`].
    pub fn overall(&self) -> f32 {
        let runs: Vec<StepId> = self.plan.iter().filter(|(_, r)| *r).map(|(id, _)| *id).collect();
        let total: f32 = runs.iter().map(|id| step_weight(*id)).sum();
        if total <= 0.0 {
            return if self.finished.is_some() { 1.0 } else { 0.0 };
        }
        let mut done = 0.0;
        for id in &runs {
            match self.status.get(id) {
                Some(StepStatus::Done(_)) => done += step_weight(*id),
                Some(StepStatus::Running) => done += step_weight(*id) * self.fraction.unwrap_or(0.0),
                _ => {}
            }
        }
        if matches!(self.finished, Some((true, _))) {
            return 1.0;
        }
        (done / total).clamp(0.0, 1.0)
    }

    /// "Step 3 of 9 · MW2 dumps" ([`step_short`]); "Done" / "Stopped" once finished.
    pub fn short_headline(&self) -> String {
        let runs: Vec<StepId> = self.plan.iter().filter(|(_, r)| *r).map(|(id, _)| *id).collect();
        match (self.current, &self.finished) {
            (Some(c), _) => {
                let k = runs.iter().position(|x| *x == c).map(|i| i + 1).unwrap_or(0);
                format!("Step {k} of {} \u{b7} {}", runs.len(), step_short(c))
            }
            (None, Some((true, _))) => "Done".into(),
            (None, Some((false, _))) => "Stopped".into(),
            (None, None) => "Starting ...".into(),
        }
    }
}

/// The step indicator's name of a wizard page (short: it sits under a dot).
pub fn indicator_title(p: Page) -> &'static str {
    match p {
        Page::Seamless => "Co-op",
        _ => p.title(),
    }
}

/// A step's name in the compact install list (the full [`StepId::title`] is its tooltip).
pub fn step_short(id: StepId) -> &'static str {
    match id {
        StepId::Preflight => "Checks",
        StepId::SaveBackup => "Save backup",
        StepId::Payload => "Mod files",
        StepId::Runtime => "Python runtime",
        StepId::Seamless => "Seamless Co-op copy",
        StepId::Witchy => "WitchyBND",
        StepId::Dumps => "MW2 dumps",
        StepId::Converters => "Converters",
        StepId::VerifyConverted => "Verify converted",
        StepId::Regulation => "Regulation",
        StepId::VerifyAll => "Verify everything",
        StepId::Cleanup => "Clean up",
        StepId::Profiles => "me3 profiles",
        StepId::Finish => "Shortcuts + record",
    }
}

/// A long path shortened to its last two parts (`...\ELDEN RING\Game`); the full path goes into a tooltip.
pub fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.trim_end_matches(['\\', '/']).split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 { p.to_string() } else { format!("...\\{}\\{}", parts[parts.len() - 2], parts[parts.len() - 1]) }
}

/// One check row as the window shows it: a short label, a short value, and for a yellow / red row a short
/// actionable hint (the full message is the row's tooltip).
#[derive(Clone, Debug, PartialEq)]
pub struct RowText {
    pub label: &'static str,
    pub value: String,
    pub hint: Option<String>,
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let a = s.find(start)? + start.len();
    let rest = &s[a..];
    Some(rest.find(end).map(|b| &rest[..b]).unwrap_or(rest)).filter(|v| !v.is_empty())
}

pub fn row_text(c: &Check) -> RowText {
    let m = c.msg.as_str();
    let label = match c.id {
        "steam" => "Steam",
        "er.found" | "er.running" => "Elden Ring",
        "er.version" => "Game version",
        "er.build" => "Steam build",
        "er.state" => "Steam state",
        "er.regulation" => "regulation.bin",
        "er.seamless_in_game" => "Seamless in game",
        "er.loaders" => "Other loaders",
        "mw2.found" => "Modern Warfare 2",
        "mw2.build" => "MW2 build",
        "me3" => "me3",
        "seamless" => "Seamless Co-op",
        "python" => "Python",
        "python.bundled" => "Bundled Python",
        "witchy" => "WitchyBND",
        "regulation.tool" => "Regulation builder",
        "vcredist" => "VC++ runtime",
        "disk" => "Free space",
        "root" => "Install folder",
        _ => "Check",
    };
    let s = |x: &str| x.to_string();
    let ok = c.level == Level::Ok;
    let (value, hint): (String, Option<&str>) = match c.id {
        "steam" => (s("not found"), Some("Choose the game folders by hand")),
        "er.found" if ok => (short_path(m.strip_prefix("Elden Ring: ").unwrap_or(m)), None),
        "er.found" => (s("not found"), Some("Choose the ELDEN RING folder")),
        "er.running" => (s("running"), Some("Close Elden Ring first")),
        "er.version" if ok => (s(m.strip_prefix("eldenring.exe ").unwrap_or(m)), None),
        "er.version" => (
            match (between(m, "eldenring.exe is ", ","), between(m, "needs ", " (")) {
                (Some(have), Some(need)) => format!("{have} (needs {need})"),
                _ => s("wrong version"),
            },
            Some("Update Elden Ring in Steam, then Check again"),
        ),
        "er.build" if ok => (s(m.strip_prefix("Steam build ").unwrap_or(m)), None),
        "er.build" => match between(m, "Steam build ", ",") {
            Some(b) => (s(b), Some("Not the release's build; the game version decides")),
            None => (s("unknown"), Some("No Steam manifest next to the folder (copied install?)")),
        },
        "er.state" => (s("update pending"), Some("Let Steam finish updating Elden Ring")),
        "er.regulation" if ok => (s("vanilla"), None),
        "er.regulation" => (s("modified"), Some("Verify the game files in Steam, then Check again")),
        "er.seamless_in_game" => (s("present"), Some("Left alone: the mod uses its own copy")),
        "er.loaders" => (s(between(m, ": ", " (").unwrap_or("found")), Some("Left alone; they can conflict")),
        "mw2.found" if ok => (short_path(m.strip_prefix("MW2: ").unwrap_or(m)), None),
        "mw2.found" if m.contains("common_mp.ff") => (s("wrong folder"), Some("Choose the Call of Duty: MW2 (2009) folder")),
        "mw2.found" => (s("not found"), Some("Choose the Call of Duty: MW2 (2009) folder")),
        "mw2.build" if ok => (s(between(m, "Steam build ", ",").unwrap_or("matches")), None),
        "mw2.build" => (s("differs"), Some("Install continues; the verify step decides")),
        "me3" if ok => (s(between(m, "me3 ", ":").unwrap_or("found")), None),
        "me3" if m.contains("newer") => (format!("{} (newer)", between(m, "me3 ", " ").unwrap_or("?")), Some("Newer than tested; it should work")),
        "me3" if m.contains("older") => (format!("{} (too old)", between(m, "me3 ", " ").unwrap_or("?")), Some("The mod needs me3 0.13.0 to load")),
        "me3" if m.contains("unreadable") => (s("version unknown"), Some("Could not read me3.exe's version")),
        "me3" => (s("not installed"), Some("The mod needs me3 0.13.0 to load")),
        "seamless" if ok => (s(between(m, "Seamless Co-op ", " (").unwrap_or("found")), None),
        "seamless" => (s("not found"), Some("Download it from Nexus Mods, then choose the zip")),
        "python" if ok && m.contains("bundled CPython ") => (format!("bundled {}", between(m, "bundled CPython ", " ").unwrap_or("")), None),
        "python" if ok => (s(between(m, "Python ", ":").or(between(m, "Python", ":")).unwrap_or("found").trim()), None),
        "python" if m.contains("3.11+ is needed") => (s("too old"), Some("Choose python.exe 3.11 or newer")),
        "python" => (s("not found"), Some("Choose python.exe 3.11 or newer")),
        "python.bundled" => (s("wrong file"), Some("Not the official CPython package: refused")),
        "witchy" if ok && m.contains("unpacked") => (s("ready"), None),
        "witchy" if ok => (s("zip ok"), None),
        "witchy" if c.level == Level::Warn => (s("download (48 MB)"), Some("Official GitHub release, sha256-pinned; or choose the zip")),
        "witchy" if m.contains("sha256 differs") => (s("wrong zip"), Some("Not the official 3.0.1.0 zip: allow the download or choose another")),
        "witchy" => (s("missing"), Some("Allow the download below or choose the zip")),
        "regulation.tool" if c.level == Level::Fail => (s("not for this release"), Some("Use the builder this release was made with (Advanced)")),
        "regulation.tool" => (
            s(if m.starts_with("Regulation builder: WitchyBND") { "WitchyBND (fallback)" } else { "built in" }),
            Some("Checked by content; the co-op id differs from the host's unless the host uses it too"),
        ),
        "vcredist" => (s("missing"), Some("Install the Microsoft VC++ 2015-2022 x64 runtime")),
        "disk" if ok => (s(m.strip_suffix(" free").unwrap_or(m)), None),
        "disk" if c.level == Level::Warn => (s("unknown"), None),
        "disk" => (s(between(m, "", " free").unwrap_or("too little")), Some("Free some space or choose another install folder")),
        "root" => (s("not usable"), Some(m)),
        _ if ok => (s(m), None),
        _ => (s(if c.level == Level::Warn { "warning" } else { "problem" }), Some(m)),
    };
    RowText { label, value, hint: hint.map(str::to_string) }
}

/// A report (Verify / Play output) as rows: (level of an `OK` / `WARN` / `FAIL` / `PASS` line, text).
pub fn report_lines(text: &str) -> Vec<(Option<Level>, String)> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let t = l.trim_start();
            for (tag, lv) in [("OK", Level::Ok), ("PASS", Level::Ok), ("WARN", Level::Warn), ("FAIL", Level::Fail)] {
                if let Some(rest) = t.strip_prefix(tag)
                    && (rest.is_empty() || rest.starts_with([' ', ':']))
                {
                    let rest = rest.trim_start_matches([' ', ':']);
                    return (Some(lv), if rest.is_empty() { tag.to_string() } else { rest.to_string() });
                }
            }
            (None, t.to_string())
        })
        .collect()
}

/// The worst level in a report (None = no tagged line).
pub fn report_level(lines: &[(Option<Level>, String)]) -> Option<Level> {
    lines.iter().filter_map(|(l, _)| *l).max()
}

/// A shortcut's chip text (its file name without `.lnk`).
pub fn shortcut_chip(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// The install folder for a folder the player picked: the folder itself when it is an install, is empty (or
/// missing) or holds only an earlier install's backups / logs, else a new `MW2inER` folder inside it (picking `D:\Games` installs into `D:\Games\MW2inER`; a non-empty
/// folder that only has the name `MW2inER` is no exception). The session's root check (`guard::check_root_contents`)
/// still refuses a result that holds other files.
pub fn root_choice(picked: &Path) -> PathBuf {
    if installer::guard::check_root_contents(picked, Path::new("")).is_ok() {
        util::normalize(picked)
    } else {
        util::normalize(&picked.join("MW2inER"))
    }
}

/// The Elden Ring `Game` folder for a folder the player picked (`ELDEN RING` or `ELDEN RING\Game`).
pub fn er_game_choice(picked: &Path) -> PathBuf {
    let g = picked.join("Game");
    if !picked.join("eldenring.exe").is_file() && g.join("eldenring.exe").is_file() { g } else { picked.to_path_buf() }
}

/// A picked `ersc.dll` / Seamless zip against the pin: Ok(note) when it is the pinned version, Err(why) otherwise.
pub fn seamless_pick_note(picked: &Path, pin: &str, version: &str) -> Result<String, String> {
    let is_zip = picked.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let hash = if is_zip {
        seamless::zip_ersc(picked).map(|(_, h)| h).ok_or_else(|| format!("{} has no SeamlessCoop\\ersc.dll inside", picked.display()))?
    } else {
        if !picked.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("ersc.dll")) {
            return Err("choose the ersc.dll inside the SeamlessCoop folder, or the zip you downloaded".into());
        }
        util::sha256_file(picked).map_err(|e| format!("cannot read {}: {e}", picked.display()))?
    };
    if hash.eq_ignore_ascii_case(pin) {
        Ok(format!("Seamless Co-op {version}: the right version"))
    } else {
        Err(format!(
            "this is not Seamless Co-op {version} (ersc.dll sha256 {}, needed {}): both players need exactly {version}; download it from Nexus Mods",
            util::short(&hash),
            util::short(pin)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join("mw2er-installer-tests").join(format!("gui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ck(id: &'static str, level: Level) -> Check {
        Check { id, level, msg: id.into(), fix: None }
    }

    #[test]
    fn page_order() {
        assert_eq!(wizard_pages(false), vec![Page::Welcome, Page::Games, Page::Me3, Page::Install, Page::Done]);
        assert_eq!(next_page(Page::Me3, true), Page::Seamless);
        assert_eq!(next_page(Page::Me3, false), Page::Install);
        assert_eq!(prev_page(Page::Install, true), Page::Seamless);
        assert_eq!(prev_page(Page::Install, false), Page::Me3);
        assert_eq!(prev_page(Page::Welcome, false), Page::Welcome);
        assert_eq!(next_page(Page::Done, false), Page::Done);
        assert_eq!(prev_page(Page::Uninstall, false), Page::Welcome);
    }

    #[test]
    fn rows_land_on_their_pages() {
        let c = vec![
            ck("er.version", Level::Fail),
            ck("mw2.build", Level::Warn),
            ck("me3", Level::Ok),
            ck("seamless", Level::Fail),
            ck("witchy", Level::Warn),
            ck("regulation.tool", Level::Warn),
            ck("disk", Level::Ok),
            ck("root", Level::Ok),
        ];
        assert_eq!(rows_for(&c, Page::Install).iter().map(|c| c.id).collect::<Vec<_>>(), ["witchy", "regulation.tool"]);
        assert_eq!(rows_for(&c, Page::Games).iter().map(|c| c.id).collect::<Vec<_>>(), ["er.version", "mw2.build", "disk", "root"]);
        assert!(page_blocked(&c, Page::Games));
        assert!(!page_blocked(&c, Page::Me3));
        assert!(page_blocked(&c, Page::Seamless));
        assert!(!page_blocked(&c, Page::Install));
    }

    #[test]
    fn run_view_follows_the_events() {
        let mut v = RunView::default();
        assert_eq!(v.overall(), 0.0);
        v.apply(&Progress::Plan(vec![(StepId::Preflight, true), (StepId::Payload, false), (StepId::Dumps, true), (StepId::Finish, true)]));
        v.apply(&Progress::StepStart { id: StepId::Preflight, index: 0, total: 4 });
        assert_eq!(v.short_headline(), "Step 1 of 3 \u{b7} Checks");
        v.apply(&Progress::StepDone { id: StepId::Preflight, ok: true, secs: 0.1, msg: "ok".into() });
        v.apply(&Progress::Skipped { id: StepId::Payload, reason: "unchanged".into() });
        v.apply(&Progress::StepStart { id: StepId::Dumps, index: 2, total: 4 });
        v.apply(&Progress::Fraction { id: StepId::Dumps, value: 0.5 });
        v.apply(&Progress::Fraction { id: StepId::Finish, value: 0.9 }); // not the running step: ignored
        let w = |s: StepId| step_weight(s);
        let want = (w(StepId::Preflight) + 0.5 * w(StepId::Dumps)) / (w(StepId::Preflight) + w(StepId::Dumps) + w(StepId::Finish));
        assert!((v.overall() - want).abs() < 1e-5, "{} vs {want}", v.overall());
        v.apply(&Progress::Line { id: StepId::Dumps, text: "OK   run 1".into() });
        v.apply(&Progress::StepDone { id: StepId::Dumps, ok: false, secs: 3.0, msg: "exit 1".into() });
        v.apply(&Progress::Finished { ok: false, msg: "INSTALL FAILED: dumps: exit 1".into() });
        assert_eq!(v.status[&StepId::Dumps], StepStatus::Failed("exit 1".into()));
        assert_eq!(v.status[&StepId::Payload], StepStatus::Skipped("unchanged".into()));
        assert_eq!(v.status[&StepId::Finish], StepStatus::Pending);
        assert!(v.overall() < 1.0);
        assert_eq!(v.short_headline(), "Stopped");
        assert!(v.log.iter().any(|l| l.contains("[dumps] OK   run 1")));
        v.finished = Some((true, "INSTALL PASSED".into()));
        assert_eq!(v.overall(), 1.0);
        for k in 0..(LOG_LINES + 10) {
            v.push_log(format!("l{k}"));
        }
        assert_eq!(v.log.len(), LOG_LINES);
    }

    #[test]
    fn folder_choices() {
        let t = tmp("choices");
        // an empty folder or an install is used as it is; any other folder gets MW2inER inside
        std::fs::create_dir_all(t.join("empty")).unwrap();
        assert_eq!(root_choice(&t.join("empty")), t.join("empty"));
        std::fs::create_dir_all(t.join("Games/other")).unwrap();
        assert_eq!(root_choice(&t.join("Games")), t.join(r"Games\MW2inER"));
        std::fs::create_dir_all(t.join("x/mw2iner/a")).unwrap();
        assert_eq!(root_choice(&t.join("x/mw2iner")), t.join(r"x\mw2iner\MW2inER"), "a non-empty folder named MW2inER is not taken as is");
        std::fs::create_dir_all(t.join(r"y\MW2inER\state")).unwrap();
        std::fs::write(t.join(r"y\MW2inER\state\install.json"), "{}").unwrap();
        assert_eq!(root_choice(&t.join(r"y\MW2inER")), t.join(r"y\MW2inER"));
        // ELDEN RING or ELDEN RING\Game
        std::fs::create_dir_all(t.join(r"ER\Game")).unwrap();
        std::fs::write(t.join(r"ER\Game\eldenring.exe"), b"x").unwrap();
        assert_eq!(er_game_choice(&t.join("ER")), t.join(r"ER\Game"));
        assert_eq!(er_game_choice(&t.join(r"ER\Game")), t.join(r"ER\Game"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn seamless_pick_notes() {
        let t = tmp("seam");
        let dll = t.join(r"SeamlessCoop\ersc.dll");
        std::fs::create_dir_all(dll.parent().unwrap()).unwrap();
        std::fs::write(&dll, b"ersc").unwrap();
        let pin = util::sha256_bytes(b"ersc");
        assert!(seamless_pick_note(&dll, &pin, "2.0.1").is_ok());
        assert!(seamless_pick_note(&dll, &util::sha256_bytes(b"other"), "2.0.1").unwrap_err().contains("not Seamless Co-op 2.0.1"));
        let other = t.join("readme.txt");
        std::fs::write(&other, b"x").unwrap();
        assert!(seamless_pick_note(&other, &pin, "2.0.1").is_err());
        let zip = t.join("nope.zip");
        std::fs::write(&zip, b"not a zip").unwrap();
        assert!(seamless_pick_note(&zip, &pin, "2.0.1").unwrap_err().contains("no SeamlessCoop"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn rows_are_short() {
        let c = |id: &'static str, level: Level, msg: &str| Check { id, level, msg: msg.into(), fix: None };
        let t = row_text(&c("er.version", Level::Ok, "eldenring.exe 2.7.2.0"));
        assert_eq!((t.label, t.value.as_str(), t.hint), ("Game version", "2.7.2.0", None));
        let t = row_text(&c(
            "er.version",
            Level::Fail,
            "eldenring.exe is 2.6.1.0, this mod needs 2.7.2.0 (Steam build 25827938). Update Elden Ring in Steam, or wait for the mod update if the game is newer",
        ));
        assert_eq!(t.value, "2.6.1.0 (needs 2.7.2.0)");
        assert!(t.hint.unwrap().contains("Update Elden Ring"));
        assert_eq!(row_text(&c("er.found", Level::Ok, r"Elden Ring: C:\Program Files (x86)\Steam\steamapps\common\ELDEN RING\Game")).value, r"...\ELDEN RING\Game");
        assert_eq!(row_text(&c("me3", Level::Ok, r"me3 0.13.0: C:\x\me3.exe")).value, "0.13.0");
        assert_eq!(row_text(&c("me3", Level::Fail, "me3 0.13.0 (the mod loader) is not installed")).value, "not installed");
        assert_eq!(row_text(&c("me3", Level::Fail, "me3 0.12.0 is older than the needed 0.13.0")).value, "0.12.0 (too old)");
        assert_eq!(row_text(&c("seamless", Level::Ok, r"Seamless Co-op 2.0.1 (zip C:\d\s.zip)")).value, "2.0.1");
        assert_eq!(row_text(&c("python", Level::Ok, r"Python: the bundled CPython 3.14.7 (x.zip, sha256-pinned) goes into runtime\python")).value, "bundled 3.14.7");
        assert_eq!(row_text(&c("python", Level::Ok, r"Python 3.12: C:\p\python.exe")).value, "3.12");
        assert_eq!(row_text(&c("python", Level::Ok, r"Python 3.12 (bundled): C:\p\python.exe")).value, "3.12 (bundled)");
        assert_eq!(row_text(&c("disk", Level::Ok, "212 GB free")).value, "212 GB");
        assert_eq!(row_text(&c("disk", Level::Fail, "3.1 GB free; the build needs about 6.0 GB while it runs")).value, "3.1 GB");
        assert_eq!(row_text(&c("mw2.build", Level::Ok, "Steam build 25374375, iw4sp.exe == release")).value, "25374375");
        assert_eq!(row_text(&c("er.build", Level::Warn, "no appmanifest_1245620.acf next to the folder (copied install?)")).value, "unknown");
        // P2: the regulation builder row (only when the builder is not the release's)
        let fb = row_text(&c("regulation.tool", Level::Warn, "Regulation builder: WitchyBND 3.0.1.0 (fallback) instead of this release's built in (regbuild). The params ..."));
        assert_eq!((fb.label, fb.value.as_str()), ("Regulation builder", "WitchyBND (fallback)"));
        assert_eq!(row_text(&c("regulation.tool", Level::Warn, "Regulation builder: built in (regbuild) instead of ...")).value, "built in");
        assert_eq!(row_text(&c("regulation.tool", Level::Fail, "This release was built with ...")).value, "not for this release");
        // an unknown row keeps its whole message as the hint; every red / yellow row has one
        let t = row_text(&c("new.thing", Level::Fail, "something new happened"));
        assert_eq!(t.hint.as_deref(), Some("something new happened"));
        for id in ["steam", "er.found", "er.running", "er.version", "er.regulation", "mw2.found", "me3", "seamless", "python", "python.bundled", "witchy", "regulation.tool", "vcredist", "disk", "root"] {
            assert!(row_text(&c(id, Level::Fail, "x")).hint.is_some(), "{id}");
            assert_ne!(row_text(&c(id, Level::Fail, "x")).label, "Check", "{id}");
        }
    }

    #[test]
    fn reports_and_names() {
        let r = report_lines("verify C:\\x\nOK   681/681 files\nWARN no shortcut\n\nFAIL Steam is not running\nPASS");
        assert_eq!(r[0], (None, "verify C:\\x".to_string()));
        assert_eq!(r[1], (Some(Level::Ok), "681/681 files".to_string()));
        assert_eq!(r[2].0, Some(Level::Warn));
        assert_eq!(r[3], (Some(Level::Fail), "Steam is not running".to_string()));
        assert_eq!(r[4], (Some(Level::Ok), "PASS".to_string()));
        assert_eq!(r.len(), 5);
        assert_eq!(report_level(&r), Some(Level::Fail));
        assert_eq!(report_level(&report_lines("OKAY this is text")), None, "OKAY is not a tag");
        assert_eq!(short_path(r"C:\a"), r"C:\a");
        assert_eq!(short_path(r"D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2\"), r"...\common\Call of Duty Modern Warfare 2");
        assert_eq!(shortcut_chip(Path::new(r"C:\u\Desktop\Modern Arsenal for Elden Ring - Co-op.lnk")), "Modern Arsenal for Elden Ring - Co-op");
        assert_eq!(wizard_pages(true).into_iter().map(indicator_title).collect::<Vec<_>>(), ["Welcome", "Games", "me3", "Co-op", "Install", "Done"]);
        for id in StepId::ALL {
            assert!(step_short(id).len() <= 20, "{id:?}");
        }
    }
}