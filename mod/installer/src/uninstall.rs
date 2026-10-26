//! Uninstall (docs/INSTALLER_DESIGN.md 2.9): removes the shortcuts and HKCU entries the install recorded, and the
//! root's entries the install created ([`OWNED`] + the recorded payload entries) except `backups\` (unless the player
//! asks to delete them too); any other entry of the root is listed as KEEP and left alone. Refuses a folder that is
//! not an install (no `state\install.json` + `mw2er.toml`). Never touches the game folders, the player's own Seamless
//! download, me3 or `%APPDATA%\EldenRing` (the guard refuses them anyway). The running exe inside the root is
//! removed by a detached `cmd` after this process exits.

use std::path::{Path, PathBuf};

use crate::guard::Guard;
use crate::state::InstallState;
use crate::util;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UninstallPlan {
    pub root: PathBuf,
    pub shortcuts: Vec<PathBuf>,
    pub registry: Vec<String>,
    /// Top-level entries of the root to delete.
    pub delete: Vec<PathBuf>,
    pub keep: Vec<PathBuf>,
    /// This exe lives in the root (deleted after exit).
    pub self_in_root: Option<PathBuf>,
}

impl UninstallPlan {
    pub fn describe(&self) -> String {
        let mut s = format!("uninstall {}\n", self.root.display());
        for x in &self.shortcuts {
            s.push_str(&format!("  delete shortcut {}\n", x.display()));
        }
        for x in &self.registry {
            s.push_str(&format!("  delete HKCU\\{x}\n"));
        }
        for x in &self.delete {
            s.push_str(&format!("  delete {}\n", x.display()));
        }
        for x in &self.keep {
            s.push_str(&format!("  KEEP   {}\n", x.display()));
        }
        if let Some(e) = &self.self_in_root {
            s.push_str(&format!("  delete {} after exit\n", e.display()));
        }
        s
    }
}

/// Top-level entries every install creates in its root (besides the payload's, recorded in `state.created` and
/// listed by `manifest\release.json`). Lowercase.
pub const OWNED: &[&str] = &[
    "build",
    "converted",
    "tools",
    "manifest",
    "profiles",
    "runtime",
    "seamless",
    "logs",
    "state",
    "downloads",
    "dumps",
    "work",
    "mw2er.toml",
    "mw2iner.exe",
    "launch_command.txt",
];

/// The lowercase top-level names this install owns: [`OWNED`], the recorded `created` entries, and (for a record
/// written before `created` existed) the top folders of the release files in the root's `manifest\release.json`.
fn owned_names(root: &Path, state: &InstallState) -> Vec<String> {
    let mut v: Vec<String> = OWNED.iter().map(|s| s.to_string()).collect();
    v.extend(state.created.iter().map(|s| s.to_lowercase()));
    if let Ok(r) = crate::release::Release::load(root) {
        v.extend(r.payload.keys().chain(r.manifest.files.keys()).filter_map(|k| k.replace('\\', "/").split('/').next().map(str::to_lowercase)));
    }
    v.sort();
    v.dedup();
    v
}

/// Builds the plan (reads only). Deletes only the entries the install owns; anything else in the root is kept.
pub fn plan(root: &Path, delete_backups: bool, self_exe: Option<&Path>) -> Result<UninstallPlan, String> {
    let state = InstallState::load(root).ok_or_else(|| format!("{} is not an MW2inER install (no state\\install.json)", root.display()))?;
    if !root.join("mw2er.toml").is_file() {
        return Err(format!("{} is not an MW2inER install (no mw2er.toml)", root.display()));
    }
    let mut p = UninstallPlan { root: root.to_path_buf(), shortcuts: state.shortcuts.clone(), registry: state.registry.clone(), ..Default::default() };
    p.shortcuts.retain(|s| s.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")));
    p.registry.retain(|k| k == crate::run::APP_KEY || k == crate::run::UNINSTALL_KEY);
    let owned = owned_names(root, &state);
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        let name = e.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name == "backups" && !delete_backups {
            p.keep.push(e);
        } else if self_exe.is_some_and(|s| util::same_path(s, &e)) {
            p.self_in_root = Some(e);
        } else if owned.contains(&name) {
            p.delete.push(e);
        } else {
            p.keep.push(e);
        }
    }
    Ok(p)
}

/// Executes the plan. Returns the lines done. `registry` = really delete HKCU keys (off in tests).
pub fn execute(p: &UninstallPlan, guard: &Guard, registry: bool) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for s in &p.shortcuts {
        guard.remove_file(s).map_err(|e| e.to_string())?;
        done.push(format!("deleted {}", s.display()));
        // the Start-menu folder (the current name or a pre-0.3.0 one), only when empty
        if let Some(d) = s.parent()
            && crate::run::is_legacy_folder(d)
        {
            let _ = std::fs::remove_dir(d);
        }
    }
    if registry {
        for k in &p.registry {
            crate::win::reg_delete_tree_hkcu(k).map_err(|e| e.to_string())?;
            done.push(format!("deleted HKCU\\{k}"));
        }
    }
    for e in &p.delete {
        if e.is_dir() { guard.remove_dir_all(e) } else { guard.remove_file(e) }.map_err(|x| format!("{}: {x}", e.display()))?;
        done.push(format!("deleted {}", e.display()));
    }
    if let Some(exe) = &p.self_in_root {
        schedule_self_delete(exe, &p.root, guard)?;
        done.push(format!("{} is deleted after this program exits", exe.display()));
    } else if p.keep.is_empty() {
        let _ = std::fs::remove_dir(&p.root);
    }
    Ok(done)
}

/// The running exe inside the root cannot delete itself: a detached `cmd` retries deleting it once a second until
/// this process has exited and the file is free (at most 10 minutes), then removes the root if it is empty (`rmdir`
/// without `/s`). The wizard calls it when its window closes (it may stay open a while after the uninstall); the CLI
/// right away.
pub fn schedule_self_delete(exe: &Path, root: &Path, guard: &Guard) -> Result<(), String> {
    guard.check(exe).map_err(|e| e.to_string())?;
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd.exe")
        .args(["/d", "/c"])
        .raw_arg(self_delete_command(exe, root))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000 | 0x0000_0008) // CREATE_NO_WINDOW | DETACHED_PROCESS
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The `cmd /c` line of [`schedule_self_delete`].
pub fn self_delete_command(exe: &Path, root: &Path) -> String {
    let (x, r) = (exe.display(), root.display());
    format!("for /l %i in (1,1,600) do @(if exist \"{x}\" (del /f /q \"{x}\" >nul 2>nul & ping -n 2 127.0.0.1 >nul) else (rmdir \"{r}\" 2>nul & exit))")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn removes_the_install_but_keeps_backups_and_outside_files() {
        let t = TempDir::new("uninst");
        let root = t.0.join("MW2inER");
        t.write(r"MW2inER\mw2er.toml", "x");
        t.write(r"MW2inER\build\natives\mw2er.dll", "x");
        t.write(r"MW2inER\backups\EldenRing-1\ER0000.sl2", "save");
        let lnk = t.write(r"Desktop\Modern Arsenal for Elden Ring.lnk", "lnk");
        // P4: a pre-0.3.0 Start-menu shortcut (old name) that an update kept recorded: removed with its folder
        let old = t.write(r"Programs\MW2 in Elden Ring\MW2 in Elden Ring - Setup.lnk", "lnk");
        let new = t.write(r"Programs\Modern Arsenal for Elden Ring\Modern Arsenal for Elden Ring - Setup.lnk", "lnk");
        let other = t.write(r"Desktop\Other.lnk", "other");
        let seam = t.write(r"Downloads\Seamless.zip", "zip");
        let mut st = InstallState::new();
        st.shortcuts = vec![lnk.clone(), seam.clone(), old.clone(), new.clone()];
        st.registry = vec![crate::run::APP_KEY.into(), r"Software\Valve".into()];
        st.save(&root, &Guard::default()).unwrap();
        let p = plan(&root, false, None).unwrap();
        assert_eq!(p.shortcuts, vec![lnk.clone(), old.clone(), new.clone()], "only .lnk files");
        assert_eq!(p.registry, vec![crate::run::APP_KEY.to_string()], "only our keys");
        assert!(p.describe().contains("KEEP"));
        execute(&p, &Guard::default(), false).unwrap();
        assert!(!lnk.exists() && other.exists() && seam.exists());
        assert!(!old.parent().unwrap().exists() && !new.parent().unwrap().exists(), "the empty Start-menu folders are removed");
        assert!(t.0.join("Programs").is_dir());
        assert!(root.join(r"backups\EldenRing-1\ER0000.sl2").is_file());
        assert!(!root.join("build").exists() && !root.join("mw2er.toml").exists());
        assert!(plan(&root, false, None).is_err(), "no longer an install");
    }

    #[test]
    fn deletes_only_what_the_install_created() {
        let t = TempDir::new("uninst-own");
        let root = t.0.join("Games");
        t.write(r"Games\mw2er.toml", "x");
        t.write(r"Games\build\natives\mw2er.dll", "x");
        t.write(r"Games\README.md", "payload");
        t.write(r"Games\licenses\python\LICENSE.txt", "payload");
        t.write(r"Games\MW2inER.exe", "exe");
        let other = t.write(r"Games\Other Game\game.exe", "mine");
        let notes = t.write(r"Games\notes.txt", "mine");
        let mut st = InstallState::new();
        st.created = vec!["readme.md".into(), "licenses".into()];
        st.save(&root, &Guard::default()).unwrap();
        let p = plan(&root, false, None).unwrap();
        assert_eq!(p.keep, vec![root.join("Other Game"), root.join("notes.txt")]);
        execute(&p, &Guard::default(), false).unwrap();
        assert!(other.is_file() && notes.is_file(), "the player's own entries stay");
        for gone in ["mw2er.toml", "build", "README.md", "licenses", "MW2inER.exe", "state"] {
            assert!(!root.join(gone).exists(), "{gone}");
        }
        // a record without `created` (0.2.0): the payload names come from manifest\release.json's lists
        let root2 = t.0.join("Old");
        t.write(r"Old\mw2er.toml", "x");
        t.write(r"Old\Install.cmd", "x");
        let keep = t.write(r"Old\mine.txt", "x");
        InstallState::new().save(&root2, &Guard::default()).unwrap();
        let p = plan(&root2, false, None).unwrap();
        assert!(p.keep.contains(&keep) && p.keep.contains(&root2.join("Install.cmd")), "no release.json: Install.cmd unknown");
    }

    #[test]
    fn self_delete_line_removes_the_exe_then_the_empty_root() {
        use std::os::windows::process::CommandExt;
        let t = TempDir::new("selfdel");
        let root = t.0.join("My MW2inER (test)");
        let exe = t.write(r"My MW2inER (test)\MW2inER.exe", b"not a program");
        let line = self_delete_command(&exe, &root);
        // run it in the foreground on the temp file (the real one is detached and waits for this process)
        let out = std::process::Command::new("cmd.exe").args(["/d", "/c"]).raw_arg(&line).output().unwrap();
        assert!(out.status.success() || !exe.exists(), "{line}");
        assert!(!exe.exists() && !root.exists(), "{line}");
        // a root that still holds backups\ stays
        let exe = t.write(r"Keep\MW2inER.exe", b"x");
        t.write(r"Keep\backups\EldenRing-1\ER0000.sl2", b"save");
        std::process::Command::new("cmd.exe").args(["/d", "/c"]).raw_arg(self_delete_command(&exe, &t.0.join("Keep"))).output().unwrap();
        assert!(!exe.exists() && t.0.join(r"Keep\backups\EldenRing-1\ER0000.sl2").is_file());
    }

    #[test]
    fn refuses_a_folder_that_is_not_an_install() {
        let t = TempDir::new("uninst-no");
        t.write("mw2er.toml", "x");
        assert!(plan(t.path(), true, None).is_err());
        t.write(r"state\install.json", "{}");
        let p = plan(t.path(), true, None).unwrap();
        // the guard still applies: a root inside a forbidden folder deletes nothing
        let g = Guard::new(Some(t.path()), None, &[], None, None);
        assert!(execute(&p, &g, false).is_err());
        assert!(t.0.join("mw2er.toml").is_file());
    }
}
