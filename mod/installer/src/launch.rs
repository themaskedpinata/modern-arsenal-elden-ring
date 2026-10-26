//! The launcher (the shortcuts' target: `MW2inER.exe play solo|coop`; docs/INSTALLER_DESIGN.md 2.5).
//!
//! [`preflight`] is pure checking (< 1 s): the install's files against `state\install.json`'s hashes, the profile
//! rules of launch_coop.ps1 / launch_solo.ps1 (no `start_online` / `disable_arxan`; solo: its own save, no
//! Seamless), the Seamless copy's ini (two values + a password set; the value is never read), the game version,
//! Steam running and Elden Ring not already running. [`save_backup_if_needed`] is launch.ps1's rule (every live
//! `ER0000.sl2` must equal a copy under `backups\`, else it is copied first; the source is only read).
//! [`command`] builds `me3 launch -g eldenring -p <profile> [--no-mem-patch]`; only [`spawn`] runs it, and only the
//! `play` command calls that (never a test).

use std::path::{Path, PathBuf};

use crate::detect::{self, Check, Level};
use crate::guard::Guard;
use crate::release::Pins;
use crate::seamless;
use crate::state::{InstallState, Mode};
use crate::util;

pub const PROFILE_COOP: &str = r"profiles\mw2er-coop.me3";
pub const PROFILE_SOLO: &str = r"profiles\mw2er-solo.me3";
pub const SOLO_SAVE: &str = "ER0000.mw2er.sl2";

pub fn profile_path(root: &Path, mode: Mode) -> PathBuf {
    root.join(if mode.coop() { PROFILE_COOP } else { PROFILE_SOLO })
}

/// The files hashed into `state.preflight` at install time and compared before every launch.
pub fn preflight_files(mode: Mode) -> Vec<&'static str> {
    let mut v = vec!["build/natives/mw2er.dll", "build/package/regulation.bin", "converted/weapons.json"];
    if mode.coop() {
        v.push("seamless/SeamlessCoop/ersc.dll");
    }
    v
}

/// A TOML basic string (`"..."`, backslashes and quotes escaped): unlike the templates' literal `'...'` strings it
/// holds any path, e.g. one under `C:\Users\O'Brien`.
pub fn toml_basic_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            c if c.is_control() => o.push_str(&format!("\\u{:04X}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Fills a profile template: each `'@@X@@'` / `"@@X@@"` (a quoted placeholder) becomes a TOML basic string of the
/// value, so a path with an apostrophe or a quote still gives a valid profile; a bare placeholder is replaced as is.
pub fn fill_profile_template(t: &str, values: &[(&str, &str)]) -> String {
    let mut text = t.to_string();
    for (ph, v) in values {
        let q = toml_basic_string(v);
        text = text.replace(&format!("'{ph}'"), &q).replace(&format!("\"{ph}\""), &q).replace(ph, v);
    }
    text
}

/// The profile text rules (launch_coop.ps1 / launch_solo.ps1); the text must also parse as TOML (a broken profile
/// fails at install time / in the preflight, not in me3 at play time).
pub fn check_profile_text(text: &str, mode: Mode) -> Result<(), String> {
    if let Err(e) = text.parse::<toml::Table>() {
        return Err(format!("the profile is not valid TOML: {}", e.message()));
    }
    for line in text.lines() {
        let l = line.trim_start().trim_start_matches(['"', '\'']);
        let key = l.split(['=', ' ', '\t', '"', '\'']).next().unwrap_or("");
        if key == "start_online" || key == "disable_arxan" {
            return Err("the profile must not set start_online / disable_arxan".into());
        }
    }
    if mode == Mode::Solo {
        let save_ok = text.lines().any(|l| {
            let t: String = l.chars().filter(|c| !c.is_whitespace()).collect();
            t == format!("savefile=\"{SOLO_SAVE}\"")
        });
        if !save_ok {
            return Err(format!("the solo profile must set savefile = \"{SOLO_SAVE}\" (separate save)"));
        }
        if text.to_lowercase().contains("ersc.dll") {
            return Err("the solo profile must not load Seamless (ersc.dll)".into());
        }
    } else if !text.to_lowercase().contains("ersc.dll") {
        return Err("the co-op profile does not load Seamless (ersc.dll)".into());
    }
    Ok(())
}

/// What the preflight looks at besides the root (injectable for tests).
pub struct World<'a> {
    pub steam_running: bool,
    pub er_running: bool,
    /// eldenring.exe FileVersion (None = unreadable / missing).
    pub er_version: Option<String>,
    pub pins: &'a Pins,
}

/// Preflight rows; launching is allowed when none is `Fail`.
pub fn preflight(root: &Path, mode: Mode, state: &InstallState, w: &World) -> Vec<Check> {
    let mut out = Vec::new();
    let mut add = |id: &'static str, level: Level, msg: String| out.push(Check { id, level, msg, fix: None });
    let prof = profile_path(root, mode);
    match std::fs::read_to_string(&prof) {
        Err(_) => add("profile", Level::Fail, format!("{} missing: run the setup (install) first", prof.display())),
        Ok(t) => match check_profile_text(&t, mode) {
            Ok(()) => add("profile", Level::Ok, format!("profile {}", prof.display())),
            Err(e) => add("profile", Level::Fail, e),
        },
    }
    for rel in preflight_files(mode) {
        let p = root.join(rel.replace('/', "\\"));
        match (util::sha256_file(&p), state.preflight.get(rel)) {
            (Err(_), _) => add("files", Level::Fail, format!("{rel} missing: run the setup (repair)")),
            (Ok(h), Some(want)) if h.eq_ignore_ascii_case(want) => {}
            (Ok(_), Some(_)) => add("files", Level::Fail, format!("{rel} changed since the install: run the setup (repair / verify)")),
            (Ok(_), None) => add("files", Level::Warn, format!("{rel}: no recorded hash (old install record)")),
        }
    }
    if mode.coop() {
        let ini = root.join(seamless::COPY_DIR).join(seamless::INI);
        match seamless::read_ini_state(&ini) {
            Err(_) => add("seamless_ini", Level::Fail, format!("{} missing: run the setup", ini.display())),
            Ok(st) => {
                if !st.keys_ok() {
                    add("seamless_ini", Level::Fail, "ersc_settings.ini must have allow_invaders = 0 and save_file_extension = mw2co2: run the setup".into());
                }
                if !st.password_set {
                    add("seamless_ini", Level::Fail, "no co-op password set: run the setup (Change co-op password)".into());
                }
            }
        }
    }
    match &w.er_version {
        Some(v) if detect::cmp_version(v, &w.pins.er_exe_version).is_eq() => add("er.version", Level::Ok, format!("eldenring.exe {v}")),
        Some(v) => add(
            "er.version",
            Level::Fail,
            format!("Elden Ring was updated ({v}). This version of the mod needs {}; wait for the mod update", w.pins.er_exe_version),
        ),
        None => add("er.version", Level::Fail, "eldenring.exe not found or unreadable (the install record's game folder)".into()),
    }
    if !w.steam_running {
        add("steam", Level::Fail, "Steam is not running: start Steam (and log in) first, then start this again".into());
    }
    if w.er_running {
        add("er.running", Level::Fail, "Elden Ring is already running".into());
    }
    out
}

/// launch.ps1's save rule. Returns one line per live save; copies into `backups\EldenRing-<stamp>\<id>\` when no
/// identical copy exists. `save_root` = `%APPDATA%\EldenRing` (only read).
pub fn save_backup_if_needed(save_root: &Path, backups: &Path, stamp: &str, guard: &Guard) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let Ok(rd) = std::fs::read_dir(save_root) else { return Ok(vec![format!("no {} (nothing to back up)", save_root.display())]) };
    let live: Vec<PathBuf> = rd.flatten().map(|e| e.path().join("ER0000.sl2")).filter(|p| p.is_file()).collect();
    if live.is_empty() {
        return Ok(vec!["no ER0000.sl2 (nothing to back up)".into()]);
    }
    let have: std::collections::HashSet<String> = util::walk_files(backups)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("ER0000.sl2")))
        .filter_map(|p| util::sha256_file(&p).ok())
        .collect();
    for l in live {
        let h = util::sha256_file(&l).map_err(|e| format!("cannot read {}: {e}", l.display()))?;
        if have.contains(&h) {
            lines.push(format!("save backup is current for {} (sha256 {})", l.display(), util::short(&h)));
            continue;
        }
        let dir = l.parent().unwrap();
        let dest = backups.join(format!("EldenRing-{stamp}")).join(dir.file_name().unwrap_or_default());
        for name in ["ER0000.sl2", "ER0000.sl2.bak", "steam_autocloud.vdf"] {
            let src = dir.join(name);
            if !src.is_file() {
                continue;
            }
            let to = dest.join(name);
            guard.copy(&src, &to).map_err(|e| format!("cannot back up {name}: {e}"))?;
            if util::sha256_file(&src).ok() != util::sha256_file(&to).ok() {
                return Err(format!("hash mismatch after copying {name}"));
            }
        }
        if util::sha256_file(&dest.join("ER0000.sl2")).ok().as_deref() != Some(h.as_str()) {
            return Err("the save changed while it was copied".into());
        }
        lines.push(format!("backed up {} to {} (hash-verified)", dir.display(), dest.display()));
    }
    Ok(lines)
}

/// `me3 launch -g eldenring -p <profile>` (+ `--no-mem-patch` for co-op: me3's allocator patch crashes Seamless 2.0.1).
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchCommand {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl LaunchCommand {
    pub fn display(&self) -> String {
        let q = |s: &str| if s.contains(' ') { format!("\"{s}\"") } else { s.to_string() };
        let mut s = q(&self.exe.display().to_string());
        for a in &self.args {
            s.push(' ');
            s.push_str(&q(a));
        }
        s
    }
}

pub fn command(me3: &Path, root: &Path, mode: Mode) -> LaunchCommand {
    let mut args = vec!["launch".into(), "-g".into(), "eldenring".into(), "-p".into(), profile_path(root, mode).display().to_string()];
    if mode.coop() {
        args.push("--no-mem-patch".into());
    }
    LaunchCommand { exe: me3.to_path_buf(), args, cwd: root.to_path_buf() }
}

/// Starts me3 (play time only). The launcher does not wait for the game.
pub fn spawn(c: &LaunchCommand) -> std::io::Result<std::process::Child> {
    // 2026-10-22 (user: "Play gave an me3 error"): the windowed launcher has no console, so me3 got no usable stdout /
    // stderr; give it a log file under <root>\logs (and no stdin) so it runs the same as from a console and its errors
    // are kept: logs\me3_launch.log (overwritten per launch)
    let mut cmd = std::process::Command::new(&c.exe);
    cmd.args(&c.args).current_dir(&c.cwd).stdin(std::process::Stdio::null());
    let log_dir = c.cwd.join("logs");
    if std::fs::create_dir_all(&log_dir).is_ok()
        && let Ok(f) = std::fs::File::create(log_dir.join("me3_launch.log"))
        && let Ok(f2) = f.try_clone()
    {
        cmd.stdout(f).stderr(f2);
    }
    cmd.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    const COOP: &str = "profileVersion = \"v1\"\nsavefile = \"ER0000.mw2coop.sl2\"\n# start_online and disable_arxan are deliberately absent\n[[natives]]\npath = 'X:\\seamless\\SeamlessCoop\\ersc.dll'\n";
    const SOLO: &str = "profileVersion = \"v1\"\nsavefile = \"ER0000.mw2er.sl2\"\n[[natives]]\npath = 'X:\\build\\natives\\mw2er.dll'\n";

    #[test]
    fn profile_paths_with_an_apostrophe() {
        // the shipped templates use TOML literal strings: '@@DLL@@'
        let tpl = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\tools\coop_friend\mw2er-coop.template.me3")).unwrap();
        let root = r#"C:\Users\O'Brien "Ob"\AppData\Local\MW2inER"#;
        let (ersc, dll, pkg) = (format!(r"{root}\seamless\SeamlessCoop\ersc.dll"), format!(r"{root}\build\natives\mw2er.dll"), format!(r"{root}\build\package"));
        // plain replacement (P0) gives a broken profile, which the check now refuses
        let naive = tpl.replace("@@ERSC@@", &ersc).replace("@@DLL@@", &dll).replace("@@PACKAGE@@", &pkg);
        assert!(check_profile_text(&naive, Mode::Coop).unwrap_err().contains("not valid TOML"));
        let text = fill_profile_template(&tpl, &[("@@ERSC@@", &ersc), ("@@DLL@@", &dll), ("@@PACKAGE@@", &pkg)]);
        check_profile_text(&text, Mode::Coop).unwrap();
        let t: toml::Table = text.parse().unwrap();
        let natives = t["natives"].as_array().unwrap();
        assert_eq!(natives[0]["path"].as_str(), Some(ersc.as_str()));
        assert_eq!(natives[1]["path"].as_str(), Some(dll.as_str()));
        assert_eq!(t["packages"].as_array().unwrap()[0]["path"].as_str(), Some(pkg.as_str()));
        assert_eq!(toml_basic_string("a\\b\"c\td"), "\"a\\\\b\\\"c\\u0009d\"");
        // the solo template keeps its separate-save line
        let solo = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\tools\coop_friend\mw2er-solo.template.me3")).unwrap();
        check_profile_text(&fill_profile_template(&solo, &[("@@DLL@@", &dll), ("@@PACKAGE@@", &pkg)]), Mode::Solo).unwrap();
    }

    #[test]
    fn profile_rules() {
        assert!(check_profile_text(COOP, Mode::Coop).is_ok());
        assert!(check_profile_text(SOLO, Mode::Solo).is_ok());
        assert!(check_profile_text(&format!("{SOLO}start_online = true\n"), Mode::Solo).is_err());
        assert!(check_profile_text(&format!("{COOP}\"disable_arxan\" = true\n"), Mode::Coop).is_err());
        assert!(check_profile_text(COOP, Mode::Solo).is_err());
        assert!(check_profile_text(SOLO, Mode::Coop).is_err());
        assert!(check_profile_text(&SOLO.replace("mw2er.sl2", "sl2"), Mode::Solo).is_err());
    }

    #[test]
    fn command_lines() {
        let c = command(Path::new(r"C:\me3\me3.exe"), Path::new(r"C:\M"), Mode::Coop);
        assert_eq!(c.args, ["launch", "-g", "eldenring", "-p", r"C:\M\profiles\mw2er-coop.me3", "--no-mem-patch"]);
        let s = command(Path::new(r"C:\me3\me3.exe"), Path::new(r"C:\M"), Mode::Solo);
        assert_eq!(s.args.last().unwrap(), r"C:\M\profiles\mw2er-solo.me3");
        assert!(!s.display().contains("--no-mem-patch"));
    }

    #[test]
    fn preflight_rows() {
        let t = TempDir::new("pre");
        let root = t.0.clone();
        t.write(PROFILE_COOP, COOP);
        let mut st = InstallState::new();
        for f in preflight_files(Mode::Coop) {
            let p = t.write(f, f.as_bytes());
            st.preflight.insert(f.into(), util::sha256_file(&p).unwrap());
        }
        t.write(r"seamless\SeamlessCoop\ersc_settings.ini", "allow_invaders = 0\ncooppassword = x\nsave_file_extension = mw2co2\n");
        let pins = Pins { er_exe_version: "2.7.2.0".into(), ..Default::default() };
        let w = World { steam_running: true, er_running: false, er_version: Some("2.7.2.0".into()), pins: &pins };
        let rows = preflight(&root, Mode::Coop, &st, &w);
        assert_eq!(detect::worst(&rows), Level::Ok, "{rows:#?}");
        t.write("build/natives/mw2er.dll", b"changed");
        t.write(r"seamless\SeamlessCoop\ersc_settings.ini", "allow_invaders = 1\ncooppassword =\nsave_file_extension = mw2co2\n");
        let w2 = World { steam_running: false, er_running: true, er_version: Some("2.7.3.0".into()), pins: &pins };
        let rows = preflight(&root, Mode::Coop, &st, &w2);
        let fails = rows.iter().filter(|c| c.level == Level::Fail).count();
        assert_eq!(fails, 6, "{rows:#?}");
        assert!(preflight(&root, Mode::Solo, &st, &w).iter().any(|c| c.id == "profile" && c.level == Level::Fail));
    }

    #[test]
    fn save_backup_rule() {
        let t = TempDir::new("saves");
        let saves = t.0.join(r"Roaming\EldenRing");
        t.write(r"Roaming\EldenRing\7656\ER0000.sl2", b"save1");
        t.write(r"Roaming\EldenRing\7656\ER0000.sl2.bak", b"bak");
        t.write(r"Roaming\EldenRing\GraphicsConfig.xml", b"cfg");
        let backups = t.0.join(r"root\backups");
        let g = Guard::default();
        let l = save_backup_if_needed(&saves, &backups, "2026-10-22_140615", &g).unwrap();
        assert!(l[0].starts_with("backed up"), "{l:?}");
        assert_eq!(std::fs::read(backups.join(r"EldenRing-2026-10-22_140615\7656\ER0000.sl2")).unwrap(), b"save1");
        let l = save_backup_if_needed(&saves, &backups, "2026-10-22_150000", &g).unwrap();
        assert!(l[0].starts_with("save backup is current"));
        assert!(!backups.join("EldenRing-2026-10-22_150000").exists());
        assert_eq!(std::fs::read(saves.join(r"7656\ER0000.sl2")).unwrap(), b"save1", "source untouched");
        assert!(save_backup_if_needed(&t.0.join("none"), &backups, "x", &g).unwrap()[0].starts_with("no "));
    }
}
