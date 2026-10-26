//! Detection and version checks (read-only): Elden Ring, MW2, me3, Seamless, Python, WitchyBND, the VC++ runtime,
//! disk space. Facts are gathered from the file system into plain structs, then [`evaluate`] turns facts + pins
//! into green / yellow / red rows ([`Check`]) for both front ends (the wizard's "Games" page and `--cli status`).
//!
//! [`Env`] holds every location of the outside world, so tests run on fixture folders and never see the real
//! Steam libraries, `%APPDATA%\EldenRing` or the registry.

use std::path::{Path, PathBuf};

use crate::release::{self, Pins, Release};
use crate::seamless;
use crate::steam::{self, AppInstall, Steam};
use crate::util;
use crate::win;

/// The outside world's locations.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The Steam folder (None = Steam not found).
    pub steam_path: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    /// Start menu `Programs` folder.
    pub programs: Option<PathBuf>,
    /// `PATH` entries.
    pub path: Vec<PathBuf>,
    pub system32: Option<PathBuf>,
    /// Read the registry (me3 uninstall entry). Off in tests.
    pub use_registry: bool,
}

impl Env {
    pub fn from_system() -> Env {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        Env {
            steam_path: steam::steam_path_from_registry(),
            local_appdata: var("LOCALAPPDATA"),
            appdata: var("APPDATA"),
            downloads: win::known_folder(win::Known::Downloads).or_else(|| var("USERPROFILE").map(|u| u.join("Downloads"))),
            documents: win::known_folder(win::Known::Documents),
            desktop: win::known_folder(win::Known::Desktop),
            programs: win::known_folder(win::Known::Programs),
            path: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            system32: var("SystemRoot").map(|w| w.join("System32")),
            use_registry: true,
        }
    }

    pub fn steam(&self) -> Steam {
        self.steam_path.as_deref().map(Steam::from_steam_path).unwrap_or_default()
    }

    /// `%APPDATA%\EldenRing` (the saves; only ever read).
    pub fn save_root(&self) -> Option<PathBuf> {
        self.appdata.as_ref().map(|a| a.join("EldenRing"))
    }

    /// The default install root `%LOCALAPPDATA%\MW2inER`.
    pub fn default_root(&self) -> Option<PathBuf> {
        self.local_appdata.as_ref().map(|l| l.join("MW2inER"))
    }

    fn which(&self, exe: &str) -> Option<PathBuf> {
        self.path.iter().map(|d| d.join(exe)).find(|p| p.is_file())
    }
}

// ---- checks ---------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// What the front end offers next to a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    OpenUrl(String),
    /// `steam://nav/games/details/<appid>` + "Properties > Installed Files > Verify integrity".
    VerifyInSteam(u32),
    UpdateInSteam(u32),
    /// A "Browse..." for this input.
    Browse(Input),
    /// Run the bundled official me3 installer (path).
    RunMe3Installer(PathBuf),
    /// Download the pinned WitchyBND zip (or browse for it).
    DownloadWitchy,
}

/// The inputs the player can choose by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    ErGame,
    Mw2Dir,
    Me3,
    Seamless,
    WitchyZip,
    Python,
    Root,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    /// Stable id (`er.version`, `mw2.build`, ...).
    pub id: &'static str,
    pub level: Level,
    pub msg: String,
    pub fix: Option<Fix>,
}

fn ck(id: &'static str, level: Level, msg: impl Into<String>, fix: Option<Fix>) -> Check {
    Check { id, level, msg: msg.into(), fix }
}

// ---- facts ----------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErFacts {
    /// The `Game` folder.
    pub game: Option<PathBuf>,
    pub buildid: Option<String>,
    pub fully_installed: Option<bool>,
    pub exe_version: Option<String>,
    pub regulation_sha256: Option<String>,
    /// `SeamlessCoop\ersc.dll` inside the ER folder (left alone; warned).
    pub seamless_in_game: bool,
    /// `dinput8.dll` / `modengine*` leftovers in the Game folder.
    pub other_loaders: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mw2Facts {
    pub dir: Option<PathBuf>,
    pub buildid: Option<String>,
    pub iw4sp_sha256: Option<String>,
    pub has_common_mp: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Me3Facts {
    pub exe: Option<PathBuf>,
    pub version: Option<String>,
    /// The bundled official installer (payload `tools\me3\me3_installer.exe`) and whether it matches its pin.
    pub bundled_installer: Option<(PathBuf, bool)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeamlessFacts {
    pub found: Option<seamless::Found>,
    /// The copy inside the install root (if any) and its ini state.
    pub copy_ini: Option<seamless::IniState>,
    pub needed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PythonFacts {
    pub exe: Option<PathBuf>,
    pub bundled: bool,
    /// `major*100+minor` once probed (running python; never in unit tests).
    pub version: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WitchyFacts {
    pub zip: Option<PathBuf>,
    pub zip_ok: bool,
    /// Already unpacked in the root with the pinned exe.
    pub unpacked_ok: bool,
    pub download_possible: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub steam_found: bool,
    pub er: ErFacts,
    pub mw2: Mw2Facts,
    pub me3: Me3Facts,
    pub seamless: SeamlessFacts,
    pub python: PythonFacts,
    pub witchy: WitchyFacts,
    pub vc_missing: Vec<String>,
    pub free_bytes: Option<u64>,
    pub need_bytes: u64,
}

/// Inputs chosen by hand / remembered (each overrides discovery).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    pub er_game: Option<PathBuf>,
    pub mw2_dir: Option<PathBuf>,
    pub me3: Option<PathBuf>,
    pub seamless: Option<PathBuf>,
    pub witchy_zip: Option<PathBuf>,
    pub python: Option<PathBuf>,
}

pub const ER_DIR: &str = "ELDEN RING";
pub const MW2_DIR: &str = "Call of Duty Modern Warfare 2";
pub const ME3_REL: &str = r"Programs\garyttierney\me3\bin\me3.exe";
pub const PYTHON_BUNDLED: &str = r"runtime\python\python.exe";
pub const WITCHY_APP: &str = r"tools\WitchyBND\app";
pub const ME3_INSTALLER: &str = r"tools\me3\me3_installer.exe";

pub fn er_game_dir(app: &AppInstall) -> PathBuf {
    app.dir.join("Game")
}

/// Elden Ring facts (hashes regulation.bin: ~2 MB).
pub fn er_facts(steam: &Steam, over: Option<&Path>) -> ErFacts {
    let mut f = ErFacts::default();
    let (game, acf) = match over {
        Some(g) => (Some(g.to_path_buf()), steam::acf_for_dir(g, steam::APP_ELDEN_RING)),
        None => match steam.find_app(steam::APP_ELDEN_RING, ER_DIR) {
            Some(a) => (Some(er_game_dir(&a)), Some(a.acf)),
            None => (None, None),
        },
    };
    let Some(game) = game.filter(|g| g.join("eldenring.exe").is_file()) else { return f };
    if let Some(m) = acf.as_deref().and_then(steam::read_app_manifest) {
        f.buildid = m.buildid.clone();
        f.fully_installed = Some(m.fully_installed());
    }
    f.exe_version = win::pe_version(&game.join("eldenring.exe")).and_then(|v| v.file_version);
    f.regulation_sha256 = util::sha256_file(&game.join("regulation.bin")).ok();
    f.seamless_in_game = game.join(r"SeamlessCoop\ersc.dll").is_file()
        || game.parent().is_some_and(|p| p.join(r"SeamlessCoop\ersc.dll").is_file());
    for n in ["dinput8.dll", "modengine2_launcher.exe", "modengine2", "mods"] {
        if game.join(n).exists() {
            f.other_loaders.push(n.to_string());
        }
    }
    f.game = Some(util::normalize(&game));
    f
}

/// MW2 facts (hashes iw4sp.exe).
pub fn mw2_facts(steam: &Steam, over: Option<&Path>) -> Mw2Facts {
    let mut f = Mw2Facts::default();
    let (dir, acf) = match over {
        Some(d) => (Some(d.to_path_buf()), steam::acf_for_dir(d, steam::APP_MW2)),
        None => match steam.find_app(steam::APP_MW2, MW2_DIR) {
            Some(a) => (Some(a.dir), Some(a.acf)),
            None => (None, None),
        },
    };
    let Some(dir) = dir.filter(|d| d.is_dir()) else { return f };
    f.has_common_mp = dir.join(r"zone\english\common_mp.ff").is_file();
    f.buildid = acf.as_deref().and_then(steam::read_app_manifest).and_then(|m| m.buildid);
    f.iw4sp_sha256 = util::sha256_file(&dir.join("iw4sp.exe")).ok();
    f.dir = Some(util::normalize(&dir));
    f
}

/// me3.exe: override, `%LOCALAPPDATA%\Programs\garyttierney\me3\bin`, PATH, the uninstall registry entry.
pub fn find_me3(env: &Env, over: Option<&Path>) -> Option<PathBuf> {
    if let Some(o) = over {
        return o.is_file().then(|| o.to_path_buf());
    }
    if let Some(p) = env.local_appdata.as_ref().map(|l| l.join(ME3_REL)).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(p) = env.which("me3.exe") {
        return Some(p);
    }
    if env.use_registry {
        let base = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
        for k in win::reg_subkeys(win::Hive::CurrentUser, base) {
            let sk = format!(r"{base}\{k}");
            let name = win::reg_get_string(win::Hive::CurrentUser, &sk, "DisplayName").unwrap_or_default();
            if !name.to_lowercase().starts_with("me3") {
                continue;
            }
            if let Some(loc) = win::reg_get_string(win::Hive::CurrentUser, &sk, "InstallLocation") {
                let p = PathBuf::from(loc).join(r"bin\me3.exe");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

pub fn me3_facts(env: &Env, over: Option<&Path>, payload: Option<&Path>, pins: &Pins) -> Me3Facts {
    let exe = find_me3(env, over);
    let version = exe.as_deref().and_then(win::pe_version).and_then(|v| v.product_version);
    let bundled_installer = payload.map(|p| p.join(ME3_INSTALLER)).filter(|p| p.is_file()).map(|p| {
        let ok = util::sha256_file(&p).is_ok_and(|h| h.eq_ignore_ascii_case(pins.me3_installer_pin()));
        (p, ok)
    });
    Me3Facts { exe, version, bundled_installer }
}

/// Python: the bundled runtime (root, then payload), `%LOCALAPPDATA%\Programs\Python\Python31x`, PATH (never the
/// WindowsApps store alias). Not probed (no process is run here).
pub fn find_python(env: &Env, over: Option<&Path>, root: &Path, payload: Option<&Path>) -> PythonFacts {
    if let Some(o) = over.filter(|o| o.is_file()) {
        return PythonFacts { exe: Some(o.to_path_buf()), bundled: false, version: None };
    }
    for base in std::iter::once(root).chain(payload) {
        let p = base.join(PYTHON_BUNDLED);
        if p.is_file() {
            return PythonFacts { exe: Some(p), bundled: true, version: None };
        }
    }
    if let Some(l) = &env.local_appdata {
        for minor in (11..=20).rev() {
            let p = l.join(format!(r"Programs\Python\Python3{minor}\python.exe"));
            if p.is_file() {
                return PythonFacts { exe: Some(p), bundled: false, version: None };
            }
        }
    }
    let exe = env.which("python.exe").filter(|p| !p.to_string_lossy().to_lowercase().contains(r"\windowsapps\"));
    PythonFacts { exe, bundled: false, version: None }
}

/// Runs `python -I -c` to read its version (`major*100+minor`). Runs Python only (never a game).
pub fn probe_python(exe: &Path) -> Option<u32> {
    let out = std::process::Command::new(exe)
        .args(["-I", "-c", "import sys; print(sys.version_info[0] * 100 + sys.version_info[1])"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// WitchyBND zip candidates: override, `<root>\downloads`, `<payload>`, Downloads.
pub fn witchy_facts(env: &Env, over: Option<&Path>, root: &Path, payload: Option<&Path>, rel: &Release) -> WitchyFacts {
    let pin = rel.pins.witchybnd_zip_sha256.to_lowercase();
    let name = release::WITCHY_ZIP_NAME;
    let mut cands: Vec<PathBuf> = over.map(Path::to_path_buf).into_iter().collect();
    cands.push(root.join("downloads").join(name));
    if let Some(p) = payload {
        cands.push(p.join(name));
    }
    if let Some(d) = &env.downloads {
        cands.push(d.join(name));
    }
    let mut f = WitchyFacts { download_possible: rel.witchy_source().is_some(), ..Default::default() };
    for c in cands.into_iter().filter(|c| c.is_file()) {
        let ok = util::sha256_file(&c).is_ok_and(|h| h == pin);
        if ok || f.zip.is_none() {
            f.zip = Some(c);
            f.zip_ok = ok;
        }
        if ok {
            break;
        }
    }
    let exe = root.join(WITCHY_APP).join("WitchyBND.exe");
    f.unpacked_ok = exe.is_file()
        && util::sha256_file(&exe).is_ok_and(|h| h.eq_ignore_ascii_case(&rel.pins.witchybnd_exe_sha256));
    f
}

/// The VC++ runtime DLLs the Unlinker imports, missing from System32.
pub fn vc_missing(env: &Env) -> Vec<String> {
    let Some(s) = &env.system32 else { return Vec::new() };
    ["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"]
        .iter()
        .filter(|d| !s.join(d).is_file())
        .map(|d| d.to_string())
        .collect()
}

/// Everything at once (the wizard's first pages, `--cli status`, the install preflight).
pub struct Gather<'a> {
    pub env: &'a Env,
    pub release: &'a Release,
    pub root: &'a Path,
    pub payload: Option<&'a Path>,
    pub over: &'a Overrides,
    pub coop: bool,
    /// Bytes the coming build needs (0 = no build).
    pub need_bytes: u64,
}

impl Gather<'_> {
    pub fn run(&self) -> Facts {
        let steam = self.env.steam();
        let er = er_facts(&steam, self.over.er_game.as_deref());
        let er_parent = er.game.as_ref().and_then(|g| g.parent().map(Path::to_path_buf));
        let pins = &self.release.pins;
        let mut seam = SeamlessFacts { needed: self.coop, ..Default::default() };
        if self.coop {
            let copy = self.root.join(seamless::COPY_DIR).join("ersc.dll");
            let mut direct: Vec<PathBuf> = self.over.seamless.iter().cloned().collect();
            // the copy inside the root counts only in a recorded install (never a folder of the player's own)
            if crate::state::InstallState::path(self.root).is_file() {
                direct.push(copy.clone());
            }
            let bases: Vec<PathBuf> = [&self.env.downloads, &self.env.documents, &self.env.desktop]
                .into_iter()
                .flatten()
                .cloned()
                .chain(self.root.parent().map(Path::to_path_buf))
                .collect();
            seam.found = match self.over.seamless.as_deref() {
                Some(z) if z.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) => seamless::zip_ersc(z)
                    .filter(|(_, h)| h.eq_ignore_ascii_case(&pins.ersc_sha256))
                    .map(|_| seamless::Found::Zip(z.to_path_buf())),
                _ => seamless::find_auto(&pins.ersc_sha256, &direct, &bases, self.env.downloads.as_deref(), er_parent.as_deref()),
            };
            seam.copy_ini = seamless::read_ini_state(&copy.with_file_name(seamless::INI)).ok();
        }
        Facts {
            steam_found: steam.steam_path.is_some(),
            mw2: mw2_facts(&steam, self.over.mw2_dir.as_deref()),
            me3: me3_facts(self.env, self.over.me3.as_deref(), self.payload, pins),
            python: find_python(self.env, self.over.python.as_deref(), self.root, self.payload),
            witchy: witchy_facts(self.env, self.over.witchy_zip.as_deref(), self.root, self.payload, self.release),
            vc_missing: vc_missing(self.env),
            free_bytes: win::free_space(self.root),
            need_bytes: self.need_bytes,
            er,
            seamless: seam,
        }
    }
}

/// Compares dotted versions numerically (`0.13.0` vs `0.13.0.0` are equal).
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let p = |s: &str| -> Vec<u64> {
        let mut v: Vec<u64> = s.split(['.', ',', ' ']).filter(|x| !x.is_empty()).map(|x| x.trim().parse().unwrap_or(0)).collect();
        while v.last() == Some(&0) {
            v.pop();
        }
        v
    };
    p(a).cmp(&p(b))
}

/// Facts + pins -> rows. `need_witchy` = the regulation step will run.
pub fn evaluate(f: &Facts, pins: &Pins, need_witchy: bool) -> Vec<Check> {
    use Level::*;
    let mut out = Vec::new();
    if !f.steam_found {
        out.push(ck("steam", Warn, "Steam was not found in the registry: choose the game folders by hand", None));
    }
    // Elden Ring
    match &f.er.game {
        None => out.push(ck("er.found", Fail, "Elden Ring was not found in any Steam library", Some(Fix::Browse(Input::ErGame)))),
        Some(g) => {
            out.push(ck("er.found", Ok, format!("Elden Ring: {}", g.display()), None));
            match &f.er.exe_version {
                Some(v) if cmp_version(v, &pins.er_exe_version).is_eq() => out.push(ck("er.version", Ok, format!("eldenring.exe {v}"), None)),
                v => out.push(ck(
                    "er.version",
                    Fail,
                    format!(
                        "eldenring.exe is {}, this mod needs {} ({}). Update Elden Ring in Steam, or wait for the mod update if the game is newer",
                        v.as_deref().unwrap_or("unreadable"),
                        pins.er_exe_version,
                        pins.er_app_version
                    ),
                    Some(Fix::UpdateInSteam(steam::APP_ELDEN_RING)),
                )),
            }
            if let Some(want) = pins.er_buildid() {
                match &f.er.buildid {
                    Some(b) if *b == want => out.push(ck("er.build", Ok, format!("Steam build {b}"), None)),
                    Some(b) => out.push(ck("er.build", Warn, format!("Steam build {b}, the release was made on {want} (the exe version decides)"), None)),
                    None => out.push(ck("er.build", Warn, "no appmanifest_1245620.acf next to the folder (copied install?)", None)),
                }
            }
            if f.er.fully_installed == Some(false) {
                out.push(ck("er.state", Warn, "Steam says Elden Ring has an update pending", Some(Fix::UpdateInSteam(steam::APP_ELDEN_RING))));
            }
            match &f.er.regulation_sha256 {
                Some(h) if h.eq_ignore_ascii_case(&pins.vanilla_regulation_sha256) => {
                    out.push(ck("er.regulation", Ok, format!("vanilla regulation.bin {}", util::short(h)), None))
                }
                h => out.push(ck(
                    "er.regulation",
                    Fail,
                    format!(
                        "regulation.bin ({}) is not the vanilla file of this version ({}): Steam > Elden Ring > Properties > Installed Files > Verify integrity",
                        h.as_deref().map(util::short).unwrap_or("missing"),
                        util::short(&pins.vanilla_regulation_sha256)
                    ),
                    Some(Fix::VerifyInSteam(steam::APP_ELDEN_RING)),
                )),
            }
            if f.er.seamless_in_game {
                out.push(ck("er.seamless_in_game", Warn, "a Seamless ersc.dll sits inside the Elden Ring folder; this mod never uses or changes it (it uses its own copy)", None));
            }
            if !f.er.other_loaders.is_empty() {
                out.push(ck("er.loaders", Warn, format!("other mod loader files in the Game folder: {} (left alone; they can conflict)", f.er.other_loaders.join(", ")), None));
            }
        }
    }
    // MW2
    match &f.mw2.dir {
        Some(d) if f.mw2.has_common_mp => {
            out.push(ck("mw2.found", Ok, format!("MW2: {}", d.display()), None));
            let b_ok = f.mw2.buildid.as_deref() == Some(pins.mw2_buildid.as_str());
            let e_ok = f.mw2.iw4sp_sha256.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(&pins.iw4sp_sha256));
            if b_ok && e_ok {
                out.push(ck("mw2.build", Ok, format!("Steam build {}, iw4sp.exe == release", pins.mw2_buildid), None));
            } else {
                out.push(ck(
                    "mw2.build",
                    Warn,
                    format!(
                        "MW2 differs from the release (build {}, iw4sp.exe {}; release: build {}, {}). Install continues; the verify step decides",
                        f.mw2.buildid.as_deref().unwrap_or("?"),
                        f.mw2.iw4sp_sha256.as_deref().map(util::short).unwrap_or("missing"),
                        pins.mw2_buildid,
                        util::short(&pins.iw4sp_sha256)
                    ),
                    None,
                ));
            }
        }
        Some(d) => out.push(ck("mw2.found", Fail, format!("{} has no zone\\english\\common_mp.ff (not the MW2 2009 Steam install?)", d.display()), Some(Fix::Browse(Input::Mw2Dir)))),
        None => out.push(ck("mw2.found", Fail, "Call of Duty: Modern Warfare 2 (2009) was not found in any Steam library", Some(Fix::Browse(Input::Mw2Dir)))),
    }
    // me3 (D11: newer allowed with a warning)
    let me3_fix = f.me3.bundled_installer.as_ref().filter(|(_, ok)| *ok).map(|(p, _)| Fix::RunMe3Installer(p.clone())).unwrap_or(Fix::OpenUrl(release::ME3_RELEASES.into()));
    match (&f.me3.exe, &f.me3.version) {
        (None, _) => out.push(ck("me3", Fail, format!("me3 {} (the mod loader) is not installed", pins.me3_version), Some(me3_fix))),
        (Some(p), Some(v)) => match cmp_version(v, &pins.me3_version) {
            std::cmp::Ordering::Equal => out.push(ck("me3", Ok, format!("me3 {v}: {}", p.display()), None)),
            std::cmp::Ordering::Greater => out.push(ck("me3", Warn, format!("me3 {v} is newer than the tested {}; it should work (report problems)", pins.me3_version), None)),
            std::cmp::Ordering::Less => out.push(ck("me3", Fail, format!("me3 {v} is older than the needed {}", pins.me3_version), Some(me3_fix))),
        },
        (Some(p), None) => out.push(ck("me3", Warn, format!("{}: version unreadable", p.display()), None)),
    }
    // Seamless
    if f.seamless.needed {
        match &f.seamless.found {
            Some(found) => {
                let what = match found {
                    seamless::Found::Dll(p) => format!("folder {}", p.parent().unwrap_or(p).display()),
                    seamless::Found::Zip(z) => format!("zip {}", z.display()),
                };
                out.push(ck("seamless", Ok, format!("Seamless Co-op {} ({what})", pins.seamless_version), None));
            }
            None => out.push(ck(
                "seamless",
                Fail,
                format!("Seamless Co-op {} was not found (download it from Nexus Mods; both players need exactly this version)", pins.seamless_version),
                Some(Fix::OpenUrl(release::NEXUS_SEAMLESS.into())),
            )),
        }
    }
    // Python
    match &f.python.exe {
        None => out.push(ck("python", Fail, "Python 3.11+ was not found (the release bundles one in runtime\\python)", Some(Fix::Browse(Input::Python)))),
        Some(p) => match f.python.version {
            Some(v) if v < 311 => out.push(ck("python", Fail, format!("{} is Python {}.{}; 3.11+ is needed", p.display(), v / 100, v % 100), Some(Fix::Browse(Input::Python)))),
            Some(v) => out.push(ck("python", Ok, format!("Python {}.{}{}: {}", v / 100, v % 100, if f.python.bundled { " (bundled)" } else { "" }, p.display()), None)),
            None => out.push(ck("python", Ok, format!("Python{}: {}", if f.python.bundled { " (bundled)" } else { "" }, p.display()), None)),
        },
    }
    // WitchyBND
    if need_witchy {
        if f.witchy.unpacked_ok {
            out.push(ck("witchy", Ok, "WitchyBND 3.0.1.0 already unpacked", None));
        } else if f.witchy.zip_ok {
            out.push(ck("witchy", Ok, format!("WitchyBND zip == the pinned 3.0.1.0 release: {}", f.witchy.zip.as_ref().unwrap().display()), None));
        } else if let Some(z) = &f.witchy.zip {
            out.push(ck("witchy", Fail, format!("{} is not the official WitchyBND 3.0.1.0 zip (sha256 differs)", z.display()), Some(Fix::DownloadWitchy)));
        } else if f.witchy.download_possible {
            out.push(ck("witchy", Warn, "WitchyBND 3.0.1.0 will be downloaded from its official GitHub release (48 MB, sha256-pinned), or choose the zip", Some(Fix::DownloadWitchy)));
        } else {
            out.push(ck("witchy", Fail, "WitchyBND-3.0.1.0-win-x64.zip not found", Some(Fix::Browse(Input::WitchyZip))));
        }
    }
    if !f.vc_missing.is_empty() {
        out.push(ck("vcredist", Fail, format!("Microsoft Visual C++ 2015-2022 x64 runtime missing ({})", f.vc_missing.join(", ")), Some(Fix::OpenUrl(release::VC_REDIST.into()))));
    }
    if f.need_bytes > 0 {
        match f.free_bytes {
            Some(b) if b < f.need_bytes => out.push(ck("disk", Fail, format!("{:.1} GB free; the build needs about {:.1} GB while it runs", gb(b), gb(f.need_bytes)), Some(Fix::Browse(Input::Root)))),
            Some(b) => out.push(ck("disk", Ok, format!("{:.0} GB free", gb(b)), None)),
            None => out.push(ck("disk", Warn, "free space unknown", None)),
        }
    }
    out
}

fn gb(b: u64) -> f64 {
    b as f64 / 1_073_741_824.0
}

/// The worst level of a set of rows.
pub fn worst(c: &[Check]) -> Level {
    c.iter().map(|c| c.level).max().unwrap_or(Level::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn env(t: &TempDir, steam: &Path) -> Env {
        Env {
            steam_path: Some(steam.to_path_buf()),
            local_appdata: Some(t.0.join("Local")),
            appdata: Some(t.0.join("Roaming")),
            downloads: Some(t.0.join("Downloads")),
            documents: Some(t.0.join("Documents")),
            desktop: None,
            programs: None,
            path: vec![],
            system32: None,
            use_registry: false,
        }
    }

    #[test]
    fn fixture_install_is_detected_and_evaluated() {
        let t = TempDir::new("detect");
        let (steam, _) = crate::steam::fixture::steam(&t, "25827938", "25374375");
        let pkg = crate::release::fixture::package(&t);
        let rel = Release::load(&pkg).unwrap();
        t.write(r"Local\Programs\garyttierney\me3\bin\me3.exe", b"not a PE");
        t.write(r"Local\Programs\Python\Python314\python.exe", b"py");
        crate::seamless::fixture::zip(&t, "Downloads/Seamless Co-op v2.0.1.zip", "", b"ersc");
        let e = env(&t, &steam);
        let over = Overrides::default();
        let root = t.0.join("root");
        let f = Gather { env: &e, release: &rel, root: &root, payload: Some(&pkg), over: &over, coop: true, need_bytes: 1 }.run();
        assert!(f.steam_found);
        assert!(f.er.game.as_ref().unwrap().ends_with(r"ELDEN RING\Game"));
        assert_eq!(f.er.buildid.as_deref(), Some("25827938"));
        assert_eq!(f.er.exe_version, None, "the fixture exe is no PE");
        assert_eq!(f.mw2.buildid.as_deref(), Some("25374375"));
        assert!(f.me3.exe.is_some() && f.me3.version.is_none());
        assert!(matches!(f.seamless.found, Some(seamless::Found::Zip(_))));
        assert!(f.python.exe.as_ref().unwrap().ends_with(r"Python314\python.exe"));
        assert!(!f.witchy.zip_ok && f.witchy.download_possible);
        let rows = evaluate(&f, &rel.pins, true);
        let lvl = |id: &str| rows.iter().find(|c| c.id == id).map(|c| c.level);
        assert_eq!(lvl("er.version"), Some(Level::Fail), "unreadable exe version is red");
        assert_eq!(lvl("er.build"), Some(Level::Ok));
        assert_eq!(lvl("er.regulation"), Some(Level::Ok));
        assert_eq!(lvl("mw2.build"), Some(Level::Ok));
        assert_eq!(lvl("me3"), Some(Level::Warn));
        assert_eq!(lvl("seamless"), Some(Level::Ok));
        assert_eq!(lvl("witchy"), Some(Level::Warn));
        assert_eq!(lvl("disk"), Some(Level::Ok));
        assert_eq!(worst(&rows), Level::Fail);
    }

    #[test]
    fn evaluate_rules() {
        let pins = Pins {
            er_exe_version: "2.7.2.0".into(),
            er_app_version: "Steam build 25827938".into(),
            vanilla_regulation_sha256: "aa".into(),
            mw2_buildid: "1".into(),
            iw4sp_sha256: "bb".into(),
            me3_version: "0.13.0".into(),
            ..Default::default()
        };
        let mut f = Facts::default();
        f.er = ErFacts {
            game: Some("G".into()),
            buildid: Some("25827938".into()),
            fully_installed: Some(true),
            exe_version: Some("2.7.2.0".into()),
            regulation_sha256: Some("AA".into()),
            ..Default::default()
        };
        f.mw2 = Mw2Facts { dir: Some("M".into()), buildid: Some("2".into()), iw4sp_sha256: Some("bb".into()), has_common_mp: true };
        f.me3 = Me3Facts { exe: Some("me3.exe".into()), version: Some("0.13.0.0".into()), bundled_installer: None };
        f.python = PythonFacts { exe: Some("py".into()), bundled: true, version: Some(314) };
        f.steam_found = true;
        let rows = evaluate(&f, &pins, false);
        assert_eq!(worst(&rows), Level::Warn, "{rows:#?}");
        assert_eq!(rows.iter().find(|c| c.id == "mw2.build").unwrap().level, Level::Warn);
        f.er.exe_version = Some("2.7.3.0".into());
        f.me3.version = Some("0.12.1".into());
        f.python.version = Some(310);
        f.vc_missing = vec!["msvcp140.dll".into()];
        f.need_bytes = 6 << 30;
        f.free_bytes = Some(1 << 30);
        let rows = evaluate(&f, &pins, false);
        let fails: Vec<&str> = rows.iter().filter(|c| c.level == Level::Fail).map(|c| c.id).collect();
        assert_eq!(fails, vec!["er.version", "me3", "python", "vcredist", "disk"]);
        f.me3.version = Some("0.14.0".into());
        assert_eq!(evaluate(&f, &pins, false).iter().find(|c| c.id == "me3").unwrap().level, Level::Warn);
    }

    #[test]
    fn bundled_me3_installer_is_pinned() {
        let t = TempDir::new("me3inst");
        let pkg = t.0.join("pkg");
        let inst = t.write(r"pkg\tools\me3\me3_installer.exe", b"not the official installer");
        let e = Env { local_appdata: Some(t.0.join("Local")), ..Default::default() };
        // no pin in the release: the compiled-in pin of the official 0.13.0 installer decides
        let f = me3_facts(&e, None, Some(&pkg), &Pins::default());
        assert_eq!(f.bundled_installer, Some((inst.clone(), false)));
        let rows = evaluate(&Facts { me3: f, ..Default::default() }, &Pins { me3_version: "0.13.0".into(), ..Default::default() }, false);
        assert!(matches!(rows.iter().find(|c| c.id == "me3").unwrap().fix, Some(Fix::OpenUrl(_))), "a wrong file is never offered");
        // the release's own pin
        let pins = Pins { me3_installer_sha256: util::sha256_bytes(b"not the official installer"), me3_version: "0.13.0".into(), ..Default::default() };
        let f = me3_facts(&e, None, Some(&pkg), &pins);
        assert_eq!(f.bundled_installer, Some((inst.clone(), true)));
        let rows = evaluate(&Facts { me3: f, ..Default::default() }, &pins, false);
        assert_eq!(rows.iter().find(|c| c.id == "me3").unwrap().fix, Some(Fix::RunMe3Installer(inst)));
        // the development copy of the official installer (work\installer\cache) has the compiled-in pin
        let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\work\installer\cache\me3-0.13.0\me3_installer.exe");
        if cache.is_file() {
            assert_eq!(util::sha256_file(&cache).unwrap(), release::ME3_INSTALLER_SHA256);
        }
    }

    #[test]
    fn versions_compare_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(cmp_version("0.13.0", "0.13.0.0"), Equal);
        assert_eq!(cmp_version("0.9.0", "0.13.0"), Less);
        assert_eq!(cmp_version("2.7.10.0", "2.7.2.0"), Greater);
    }

    #[test]
    fn python_search_order_skips_windowsapps() {
        let t = TempDir::new("py");
        let mut e = env(&t, &t.0);
        let w = t.0.join(r"Microsoft\WindowsApps");
        t.write(r"Microsoft\WindowsApps\python.exe", b"stub");
        e.path = vec![w];
        assert_eq!(find_python(&e, None, &t.0.join("root"), None).exe, None);
        t.write(r"pkg\runtime\python\python.exe", b"py");
        let f = find_python(&e, None, &t.0.join("root"), Some(&t.0.join("pkg")));
        assert!(f.bundled);
    }
}
