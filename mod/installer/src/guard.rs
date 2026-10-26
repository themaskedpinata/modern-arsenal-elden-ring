//! The write guard (the `Guard` of setup.ps1, ported): every file or folder this program creates, changes or
//! deletes goes through [`Guard::check`]. It refuses anything under the Elden Ring folder or its parent, the MW2
//! folder, any Steam library's `steamapps`, the Steam folder, and `%APPDATA%\EldenRing` (the saves are only ever
//! read, by the backup copy).
//!
//! [`check_root`] decides whether a folder may be the install root (docs/INSTALLER_DESIGN.md 2.2): not inside
//! any forbidden folder, not `Program Files` (no admin rights), not a drive root, not the Windows folder.

use std::path::{Path, PathBuf};

use crate::util::{is_under, normalize};

#[derive(Clone, Debug, Default)]
pub struct Guard {
    forbidden: Vec<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub struct Refused {
    pub path: PathBuf,
    pub inside: PathBuf,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refusing to write {} (inside {})", self.path.display(), self.inside.display())
    }
}

impl std::error::Error for Refused {}

impl From<Refused> for std::io::Error {
    fn from(r: Refused) -> Self {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, r.to_string())
    }
}

impl Guard {
    /// `er_game` = the ER `Game` folder (its parent is forbidden too), `mw2` = the MW2 folder, `steamapps` = every
    /// library's steamapps folder, `steam` = the Steam client folder, `appdata` = `%APPDATA%`.
    pub fn new(
        er_game: Option<&Path>,
        mw2: Option<&Path>,
        steamapps: &[PathBuf],
        steam: Option<&Path>,
        appdata: Option<&Path>,
    ) -> Guard {
        let mut f = Vec::new();
        if let Some(e) = er_game {
            f.push(normalize(e));
            if let Some(p) = normalize(e).parent() {
                f.push(p.to_path_buf());
            }
        }
        f.extend(mw2.map(normalize));
        f.extend(steamapps.iter().map(|p| normalize(p)));
        f.extend(steam.map(normalize));
        if let Some(a) = appdata {
            f.push(normalize(&a.join("EldenRing")));
        }
        f.retain(|p| p.components().count() > 1);
        Guard { forbidden: f }
    }

    /// Ok when `path` may be written.
    pub fn check(&self, path: &Path) -> Result<(), Refused> {
        for f in &self.forbidden {
            if is_under(path, f) {
                return Err(Refused { path: path.to_path_buf(), inside: f.clone() });
            }
        }
        Ok(())
    }

    pub fn forbidden(&self) -> &[PathBuf] {
        &self.forbidden
    }

    /// Adds one more folder (or file) this program must never write, e.g. the player's own Seamless download.
    pub fn forbid(&mut self, p: &Path) {
        let n = normalize(p);
        if n.components().count() > 2 && !self.forbidden.contains(&n) {
            self.forbidden.push(n);
        }
    }

    // ---- guarded file operations ------------------------------------------------------------------------------

    pub fn create_dir_all(&self, p: &Path) -> std::io::Result<()> {
        self.check(p)?;
        std::fs::create_dir_all(p)
    }

    pub fn write(&self, p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
        self.check(p)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, data)
    }

    /// Writes through `<p>.part` and a rename, so a crash never leaves a half file under the final name.
    pub fn write_atomic(&self, p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
        self.check(p)?;
        let part = p.with_extension(format!("{}.part", p.extension().and_then(|e| e.to_str()).unwrap_or("")));
        self.write(&part, data)?;
        std::fs::rename(&part, p)
    }

    pub fn copy(&self, from: &Path, to: &Path) -> std::io::Result<u64> {
        self.check(to)?;
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::copy(from, to)
    }

    pub fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        self.check(p)?;
        match std::fs::remove_file(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    /// Removes a folder tree (Ok when missing).
    pub fn remove_dir_all(&self, p: &Path) -> std::io::Result<()> {
        self.check(p)?;
        match std::fs::remove_dir_all(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    pub fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.check(from)?;
        self.check(to)?;
        std::fs::rename(from, to)
    }
}

/// Why a folder cannot be the install root.
pub fn check_root(root: &Path, guard: &Guard, program_files: &[PathBuf], windows_dir: Option<&Path>) -> Result<(), String> {
    let r = normalize(root);
    if r.components().count() <= 2 {
        return Err(format!("{} is a drive root: choose a folder, e.g. %LOCALAPPDATA%\\MW2inER", r.display()));
    }
    if let Err(e) = guard.check(&r) {
        return Err(format!("{} is inside a game, Steam, save or Seamless Co-op folder ({}): choose another folder", r.display(), e.inside.display()));
    }
    for pf in program_files {
        if is_under(&r, pf) {
            return Err(format!("{} is under {} (needs admin rights): choose another folder", r.display(), pf.display()));
        }
    }
    if let Some(w) = windows_dir
        && is_under(&r, w)
    {
        return Err(format!("{} is inside the Windows folder", r.display()));
    }
    Ok(())
}

/// Top-level entries an earlier run (or an uninstall that kept the save backups) may leave in a root that has no
/// `state\install.json`; a root holding only these is still taken as the install's own.
pub const LEFTOVER_ENTRIES: &[&str] = &["backups", "logs", "state"];

/// Whether an existing folder may become (or stay) the install root: it does not exist yet, is empty, already holds
/// an install (`state\install.json`), holds only [`LEFTOVER_ENTRIES`], or is the release folder itself (`payload`,
/// installing in place) without a Seamless copy of unknown origin. Any other folder is refused: uninstall deletes
/// the install's entries from the root, and `<root>\seamless\SeamlessCoop` must never be the player's own Seamless.
pub fn check_root_contents(root: &Path, payload: &Path) -> Result<(), String> {
    if root.is_file() {
        return Err(format!("{} is a file: choose a folder", root.display()));
    }
    let Ok(rd) = std::fs::read_dir(root) else { return Ok(()) };
    let names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect();
    if names.is_empty() || crate::state::InstallState::path(root).is_file() {
        return Ok(());
    }
    if names.iter().all(|n| LEFTOVER_ENTRIES.contains(&n.as_str())) {
        return Ok(());
    }
    let seamless_copy = root.join(crate::seamless::COPY_DIR).exists();
    if crate::util::same_path(root, payload) && !seamless_copy {
        return Ok(());
    }
    Err(format!(
        "{} already holds other files{} and is not an MW2inER install: choose an empty or new folder, e.g. {}",
        root.display(),
        if seamless_copy { " (a Seamless Co-op folder)" } else { "" },
        root.join("MW2inER").display()
    ))
}

/// `Program Files` folders from the environment.
pub fn program_files_dirs() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn g() -> Guard {
        Guard::new(
            Some(Path::new(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game")),
            Some(Path::new(r"D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2")),
            &[PathBuf::from(r"D:\SteamLibrary\steamapps"), PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps")],
            Some(Path::new(r"C:\Program Files (x86)\Steam")),
            Some(Path::new(r"C:\Users\p\AppData\Roaming")),
        )
    }

    #[test]
    fn refuses_every_forbidden_root() {
        let g = g();
        for p in [
            r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\regulation.bin",
            r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game",
            r"D:\SteamLibrary\steamapps\common\ELDEN RING\SeamlessCoop\ersc_settings.ini",
            r"d:\steamlibrary\STEAMAPPS\common\call of duty modern warfare 2\main\x.iwd",
            r"D:\SteamLibrary\steamapps\appmanifest_1245620.acf",
            r"D:\SteamLibrary\steamapps\common\Other Game\x",
            r"C:\Program Files (x86)\Steam\config\x.vdf",
            r"C:\Users\p\AppData\Roaming\EldenRing\765\ER0000.sl2",
            r"C:\Users\p\AppData\Roaming\EldenRing",
            r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\..\Game\x",
        ] {
            assert!(g.check(Path::new(p)).is_err(), "{p}");
        }
        for p in [
            r"C:\Users\p\AppData\Local\MW2inER\build\natives\mw2er.dll",
            r"C:\Users\p\AppData\Roaming\EldenRingX\x",
            r"D:\SteamLibrary\MW2inER",
            r"D:\Mods\MW2inER-coop\seamless\SeamlessCoop\ersc_settings.ini",
        ] {
            assert!(g.check(Path::new(p)).is_ok(), "{p}");
        }
    }

    #[test]
    fn guarded_ops_refuse_before_touching_anything() {
        let t = TempDir::new("guard");
        let game = t.0.join(r"lib\steamapps\common\ELDEN RING\Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("regulation.bin"), b"v").unwrap();
        let g = Guard::new(Some(&game), None, &[t.0.join(r"lib\steamapps")], None, None);
        assert!(g.write(&game.join("x.txt"), b"x").is_err());
        assert!(g.remove_file(&game.join("regulation.bin")).is_err());
        assert!(g.remove_dir_all(&game).is_err());
        assert!(g.copy(&game.join("regulation.bin"), &game.join("y")).is_err());
        assert_eq!(std::fs::read(game.join("regulation.bin")).unwrap(), b"v");
        assert!(!game.join("x.txt").exists());
        let ok = t.0.join(r"root\state\install.json");
        g.write_atomic(&ok, b"{}").unwrap();
        assert_eq!(std::fs::read(&ok).unwrap(), b"{}");
        assert!(!t.0.join(r"root\state\install.json.part").exists());
        g.remove_dir_all(&t.0.join("root")).unwrap();
        g.remove_dir_all(&t.0.join("root")).unwrap();
    }

    #[test]
    fn install_root_rules() {
        let g = g();
        let pf = vec![PathBuf::from(r"C:\Program Files"), PathBuf::from(r"C:\Program Files (x86)")];
        let w = Path::new(r"C:\Windows");
        assert!(check_root(Path::new(r"C:\Users\p\AppData\Local\MW2inER"), &g, &pf, Some(w)).is_ok());
        assert!(check_root(Path::new(r"D:\Games\MW2inER"), &g, &pf, Some(w)).is_ok());
        assert!(check_root(Path::new(r"D:\"), &g, &pf, Some(w)).is_err());
        assert!(check_root(Path::new(r"C:\Program Files\MW2inER"), &g, &pf, Some(w)).is_err());
        assert!(check_root(Path::new(r"D:\SteamLibrary\steamapps\common\MW2inER"), &g, &pf, Some(w)).is_err());
        assert!(check_root(Path::new(r"C:\Users\p\AppData\Roaming\EldenRing\mod"), &g, &pf, Some(w)).is_err());
        assert!(check_root(Path::new(r"C:\Windows\Temp\x"), &g, &pf, Some(w)).is_err());
    }

    #[test]
    fn root_contents_rules() {
        let t = TempDir::new("rootc");
        let pay = t.0.join("payload");
        // missing or empty: ok
        assert!(check_root_contents(&t.0.join("new"), &pay).is_ok());
        std::fs::create_dir_all(t.0.join("empty")).unwrap();
        assert!(check_root_contents(&t.0.join("empty"), &pay).is_ok());
        // a folder of the player's own files: refused (uninstall would delete them)
        t.write(r"Games\Other Game\game.exe", "x");
        t.write(r"Games\notes.txt", "x");
        let e = check_root_contents(&t.0.join("Games"), &pay).unwrap_err();
        assert!(e.contains(r"Games\MW2inER"), "{e}");
        // a folder named MW2inER is no exception
        t.write(r"x\MW2inER\mine.txt", "x");
        assert!(check_root_contents(&t.0.join(r"x\MW2inER"), &pay).is_err());
        // the player's Seamless download (Documents\seamless\SeamlessCoop) is never taken as the copy
        t.write(r"Documents\seamless\SeamlessCoop\ersc.dll", "dll");
        t.write(r"Documents\letter.txt", "x");
        assert!(check_root_contents(&t.0.join("Documents"), &pay).unwrap_err().contains("Seamless"));
        // ... not even when it is the only thing there
        t.write(r"OnlySeamless\seamless\SeamlessCoop\ersc.dll", "dll");
        assert!(check_root_contents(&t.0.join("OnlySeamless"), &pay).is_err());
        // a recorded install, or only the backups an uninstall kept: ok
        t.write(r"Inst\state\install.json", "{}");
        t.write(r"Inst\build\natives\mw2er.dll", "x");
        assert!(check_root_contents(&t.0.join("Inst"), &pay).is_ok());
        t.write(r"Kept\backups\EldenRing-1\ER0000.sl2", "save");
        assert!(check_root_contents(&t.0.join("Kept"), &pay).is_ok());
        // the release folder itself (in place), unless it holds a Seamless folder
        t.write(r"payload\mw2er.toml", "x");
        assert!(check_root_contents(&pay, &pay).is_ok());
        let f = t.write("afile", "x");
        assert!(check_root_contents(&f, &pay).is_err());
    }

    #[test]
    fn forbid_adds_a_folder() {
        let mut g = Guard::default();
        g.forbid(Path::new(r"C:\Users\p\Documents\seamless\SeamlessCoop"));
        g.forbid(Path::new(r"C:\"));
        assert_eq!(g.forbidden().len(), 1);
        assert!(g.check(Path::new(r"C:\Users\p\Documents\seamless\SeamlessCoop\ersc_settings.ini")).is_err());
        assert!(g.check(Path::new(r"C:\Users\p\Documents\MW2inER\x")).is_ok());
    }
}
