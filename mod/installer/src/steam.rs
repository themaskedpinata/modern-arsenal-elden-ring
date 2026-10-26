//! Steam discovery (read-only): the Steam folder from the registry, every library from
//! `steamapps\libraryfolders.vdf` (which also lists the app ids installed in each library), and
//! `appmanifest_<id>.acf` for `installdir`, `buildid` and `StateFlags`.
//!
//! Elden Ring = app 1245620 (`<lib>\steamapps\common\<installdir>\Game`), MW2 (2009) = app 10180
//! (`<lib>\steamapps\common\<installdir>`). Nothing here writes anything.

use std::fs;
use std::path::{Path, PathBuf};

use crate::vdf;
use crate::win::{self, Hive};

pub const APP_ELDEN_RING: u32 = 1_245_620;
pub const APP_MW2: u32 = 10_180;
/// `StateFlags` bit 4 = fully installed (`StateFlags` 4 = installed and up to date; 6 / 1030 etc. = update pending).
pub const STATE_FULLY_INSTALLED: u32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub path: PathBuf,
    /// App ids listed for this library in libraryfolders.vdf (empty in the old file format).
    pub apps: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steam {
    /// The Steam client folder (None when not found).
    pub steam_path: Option<PathBuf>,
    pub libraries: Vec<Library>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppManifest {
    pub appid: u32,
    pub name: Option<String>,
    pub installdir: String,
    pub buildid: Option<String>,
    pub state_flags: Option<u32>,
}

impl AppManifest {
    /// `StateFlags == 4` (installed, no update pending).
    pub fn fully_installed(&self) -> bool {
        self.state_flags == Some(STATE_FULLY_INSTALLED)
    }
}

/// An installed app found in a library.
#[derive(Clone, Debug, PartialEq)]
pub struct AppInstall {
    pub library: PathBuf,
    /// `<lib>\steamapps\common\<installdir>`
    pub dir: PathBuf,
    pub acf: PathBuf,
    pub manifest: Option<AppManifest>,
}

/// The Steam folder from `HKCU\Software\Valve\Steam\SteamPath`, else
/// `HKLM\SOFTWARE\WOW6432Node\Valve\Steam\InstallPath`.
pub fn steam_path_from_registry() -> Option<PathBuf> {
    win::reg_get_string(Hive::CurrentUser, r"Software\Valve\Steam", "SteamPath")
        .or_else(|| win::reg_get_string(Hive::LocalMachine, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
        .filter(|s| !s.trim().is_empty())
        .map(|s| crate::util::normalize(Path::new(&s.replace('/', "\\"))))
}

/// Libraries from a libraryfolders.vdf text (modern `"0" { "path" ... "apps" {...} }` and the old
/// `"1" "D:\\SteamLibrary"` format).
pub fn parse_library_folders(text: &str) -> Result<Vec<Library>, vdf::ParseError> {
    let doc = vdf::parse(text)?;
    let mut out = Vec::new();
    let Some(root) = doc.get("libraryfolders") else { return Ok(out) };
    for (k, v) in root.entries() {
        if k.parse::<u32>().is_err() {
            continue;
        }
        match v {
            vdf::Value::Str(p) => out.push(Library { path: PathBuf::from(p), apps: Vec::new() }),
            vdf::Value::Block(_) => {
                let Some(p) = v.get_str("path") else { continue };
                let apps = v
                    .get("apps")
                    .map(|a| a.entries().iter().filter_map(|(id, _)| id.parse().ok()).collect())
                    .unwrap_or_default();
                out.push(Library { path: PathBuf::from(p), apps });
            }
        }
    }
    Ok(out)
}

/// Parses an `appmanifest_<id>.acf` text.
pub fn parse_app_manifest(text: &str) -> Option<AppManifest> {
    let doc = vdf::parse(text).ok()?;
    let a = doc.get("AppState")?;
    Some(AppManifest {
        appid: a.get_str("appid")?.trim().parse().ok()?,
        name: a.get_str("name").map(str::to_string),
        installdir: a.get_str("installdir")?.to_string(),
        buildid: a.get_str("buildid").map(str::to_string),
        state_flags: a.get_str("StateFlags").and_then(|s| s.trim().parse().ok()),
    })
}

pub fn read_app_manifest(acf: &Path) -> Option<AppManifest> {
    parse_app_manifest(&fs::read_to_string(acf).ok()?)
}

impl Steam {
    /// Discovery from the registry (None steam path = Steam not found; libraries empty).
    pub fn discover() -> Steam {
        match steam_path_from_registry() {
            Some(p) => Steam::from_steam_path(&p),
            None => Steam::default(),
        }
    }

    /// Discovery from a known Steam folder (tests use a fixture folder). The Steam folder itself is always a
    /// library; duplicates (case-insensitive) are dropped.
    pub fn from_steam_path(steam: &Path) -> Steam {
        let mut libs = vec![Library { path: steam.to_path_buf(), apps: Vec::new() }];
        if let Ok(t) = fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf"))
            && let Ok(found) = parse_library_folders(&t)
        {
            for l in found {
                if let Some(e) = libs.iter_mut().find(|e| crate::util::same_path(&e.path, &l.path)) {
                    if e.apps.is_empty() {
                        e.apps = l.apps;
                    }
                } else {
                    libs.push(l);
                }
            }
        }
        Steam { steam_path: Some(steam.to_path_buf()), libraries: libs }
    }

    /// Finds an app: first the libraries whose vdf lists it, then every library with its appmanifest.
    /// Falls back to `common\<fallback_dir>` when the acf is missing (a copied install).
    pub fn find_app(&self, appid: u32, fallback_dir: &str) -> Option<AppInstall> {
        let mut order: Vec<&Library> = self.libraries.iter().filter(|l| l.apps.contains(&appid)).collect();
        order.extend(self.libraries.iter().filter(|l| !l.apps.contains(&appid)));
        for l in &order {
            let acf = l.path.join(format!(r"steamapps\appmanifest_{appid}.acf"));
            if let Some(m) = read_app_manifest(&acf) {
                let dir = l.path.join(r"steamapps\common").join(&m.installdir);
                if dir.is_dir() {
                    return Some(AppInstall { library: l.path.clone(), dir, acf, manifest: Some(m) });
                }
            }
        }
        for l in &order {
            let dir = l.path.join(r"steamapps\common").join(fallback_dir);
            if dir.is_dir() {
                let acf = l.path.join(format!(r"steamapps\appmanifest_{appid}.acf"));
                let manifest = read_app_manifest(&acf);
                return Some(AppInstall { library: l.path.clone(), dir, acf, manifest });
            }
        }
        None
    }

    /// Every `<lib>\steamapps` folder (the guard refuses writes under all of them).
    pub fn steamapps_dirs(&self) -> Vec<PathBuf> {
        self.libraries.iter().map(|l| l.path.join("steamapps")).collect()
    }
}

/// The library + acf of a folder chosen by hand (`<lib>\steamapps\common\<dir>[\Game]`): walks up to `steamapps`.
pub fn acf_for_dir(dir: &Path, appid: u32) -> Option<PathBuf> {
    let mut p = crate::util::normalize(dir);
    loop {
        if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("steamapps")) {
            let acf = p.join(format!("appmanifest_{appid}.acf"));
            return acf.is_file().then_some(acf);
        }
        if !p.pop() {
            return None;
        }
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use crate::util::testutil::TempDir;

    /// A fake Steam install: Steam folder (library 0, empty) + library D with ER and MW2.
    pub fn steam(t: &TempDir, er_build: &str, mw2_build: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let steam = t.0.join("Steam");
        let lib = t.0.join("SteamLibrary");
        let esc = |p: &std::path::Path| p.to_string_lossy().replace('\\', "\\\\");
        t.write(
            "Steam/steamapps/libraryfolders.vdf",
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"228980\"\t\t\"1\"\n\t\t}}\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"10180\"\t\t\"1\"\n\t\t\t\"1245620\"\t\t\"1\"\n\t\t}}\n\t}}\n}}\n",
                esc(&steam),
                esc(&lib)
            ),
        );
        t.write(
            "SteamLibrary/steamapps/appmanifest_1245620.acf",
            format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"ELDEN RING\"\n\t\"buildid\"\t\t\"{er_build}\"\n}}\n"),
        );
        t.write(
            "SteamLibrary/steamapps/appmanifest_10180.acf",
            format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"10180\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"InstallDir\"\t\t\"Call of Duty Modern Warfare 2\"\n\t\"buildid\"\t\t\"{mw2_build}\"\n}}\n"),
        );
        t.write("SteamLibrary/steamapps/common/ELDEN RING/Game/eldenring.exe", b"fake");
        t.write("SteamLibrary/steamapps/common/ELDEN RING/Game/regulation.bin", b"vanilla regulation");
        t.write("SteamLibrary/steamapps/common/Call of Duty Modern Warfare 2/zone/english/common_mp.ff", b"ff");
        t.write("SteamLibrary/steamapps/common/Call of Duty Modern Warfare 2/iw4sp.exe", b"iw4sp");
        (steam, lib)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn discovery_finds_both_games_in_a_second_library() {
        let t = TempDir::new("steam");
        let (steam, lib) = fixture::steam(&t, "25827938", "25374375");
        let s = Steam::from_steam_path(&steam);
        assert_eq!(s.libraries.len(), 2, "the Steam folder is listed once even though the vdf repeats it");
        let er = s.find_app(APP_ELDEN_RING, "ELDEN RING").unwrap();
        assert_eq!(er.library, lib);
        assert!(er.dir.ends_with(r"common\ELDEN RING"));
        let m = er.manifest.unwrap();
        assert_eq!(m.buildid.as_deref(), Some("25827938"));
        assert!(m.fully_installed());
        let mw = s.find_app(APP_MW2, "Call of Duty Modern Warfare 2").unwrap();
        assert_eq!(mw.manifest.unwrap().installdir, "Call of Duty Modern Warfare 2");
        assert_eq!(s.find_app(42, "nothing"), None);
        assert_eq!(s.steamapps_dirs().len(), 2);
        assert_eq!(acf_for_dir(&er.dir.join("Game"), APP_ELDEN_RING).unwrap(), er.acf);
        assert_eq!(acf_for_dir(&t.0, APP_ELDEN_RING), None);
    }

    #[test]
    fn old_vdf_format_and_missing_acf_fallback() {
        let t = TempDir::new("steam-old");
        let lib = t.0.join("Lib2");
        t.write(
            "Steam/steamapps/libraryfolders.vdf",
            format!("\"LibraryFolders\"\n{{\n\t\"TimeNextStatsReport\"\t\t\"1\"\n\t\"1\"\t\t\"{}\"\n}}\n", lib.to_string_lossy().replace('\\', "\\\\")),
        );
        t.write("Lib2/steamapps/common/Call of Duty Modern Warfare 2/iw4sp.exe", b"x");
        let s = Steam::from_steam_path(&t.0.join("Steam"));
        assert_eq!(s.libraries.len(), 2);
        assert!(s.libraries[1].apps.is_empty());
        let mw = s.find_app(APP_MW2, "Call of Duty Modern Warfare 2").unwrap();
        assert_eq!(mw.manifest, None);
    }

    #[test]
    fn acf_parsing() {
        let m = parse_app_manifest("\"AppState\" { \"appid\" \"10180\" \"installdir\" \"X\" \"StateFlags\" \"1030\" }").unwrap();
        assert!(!m.fully_installed());
        assert_eq!(m.buildid, None);
        assert_eq!(parse_app_manifest("\"AppState\" { \"appid\" \"x\" \"installdir\" \"X\" }"), None);
        assert_eq!(parse_app_manifest("garbage {"), None);
    }
}
