//! The P3 rollback store (docs/INSTALLER_DESIGN.md 2.8 "Rollback"): an update from version X keeps everything it
//! replaces in `<root>\state\rollback\<X>\`, and "Roll back" (the window's Installed card, `--cli rollback`) puts
//! version X back - its files, its `mw2er.toml` and its `state\install.json`.
//!
//! Layout of a store:
//! - `rollback.json` ([`Record`]): from / to version and co-op id, complete flag, the files the update added;
//! - `install.json`: X's install record as it was before the update (written once);
//! - `files\<rel>`: X's copy of every file the update replaced (the DLL, scripts, `mw2er.toml`, `MW2inER.exe`,
//!   profiles ...), copied before the new one goes in (first copy wins: a resumed update never overwrites it);
//! - `moved\<rel>`: build outputs the update rebuilt (`converted\`, `build\package\`, `build\rows.json`, the
//!   Python runtime), moved aside instead of deleted (a rename on the same drive: instant, no extra space).
//!
//! Only one store is kept: starting an update from another version removes older stores; a rollback consumes it.
//! An interrupted update can be rolled back too (what was saved so far is restored; everything else is X's still).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::guard::Guard;
use crate::state::InstallState;
use crate::util;

/// Root-relative folder of the stores.
pub const DIR: &str = r"state\rollback";
pub const RECORD: &str = "rollback.json";
pub const OLD_STATE: &str = "install.json";
/// Suffix of a copy into `files\` in progress (renamed to the final name once complete; never restored).
pub const PART: &str = ".rbpart";
/// Build outputs an update moves into the store instead of deleting them (the step's `clean` entries).
pub const MOVE_ON_UPDATE: &[&str] = &["converted", "build/package", "build/rows.json", "runtime/python"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub what: String,
    pub from_version: String,
    pub from_coop_id: String,
    pub to_version: String,
    pub to_coop_id: String,
    pub started_utc: String,
    /// The update finished (PASS); false = it was interrupted or failed (a rollback is still possible).
    pub complete: bool,
    pub completed_utc: String,
    /// Root-relative files the update created (deleted by a rollback).
    pub added: Vec<String>,
}

/// An open store.
#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    pub dir: PathBuf,
    pub rec: Record,
}

fn safe_name(v: &str) -> String {
    let s: String = v.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+') { c } else { '_' }).collect();
    if s.is_empty() || s.chars().all(|c| c == '.') { "unknown".into() } else { s }
}

fn rel_path(base: &Path, rel: &str) -> PathBuf {
    base.join(rel.replace('/', "\\"))
}

impl Store {
    pub fn dir_for(root: &Path, from_version: &str) -> PathBuf {
        root.join(DIR).join(safe_name(from_version))
    }

    /// Opens the store of an update from `old` (resumed: the existing one is kept as it is) or starts it: X's record
    /// is written once, and stores of other versions are removed.
    pub fn begin(root: &Path, old: &InstallState, to_version: &str, guard: &Guard) -> std::io::Result<Store> {
        let dir = Self::dir_for(root, &old.release_version);
        if let Ok(rd) = std::fs::read_dir(root.join(DIR)) {
            for e in rd.flatten() {
                if !util::same_path(&e.path(), &dir) {
                    guard.remove_dir_all(&e.path())?;
                }
            }
        }
        if let Some(s) = Self::open(&dir) {
            let mut s = s;
            if s.rec.to_version != to_version {
                // an earlier, unfinished update from X to another version: same originals, new target
                s.rec.to_version = to_version.to_string();
                s.rec.complete = false;
                s.write(guard)?;
            }
            return Ok(s);
        }
        guard.create_dir_all(&dir)?;
        let mut t = serde_json::to_string_pretty(old).map_err(std::io::Error::other)?;
        t.push('\n');
        guard.write_atomic(&dir.join(OLD_STATE), t)?;
        let s = Store {
            dir,
            rec: Record {
                what: "MW2 in Elden Ring rollback store: the files an update replaced (Roll back restores them)".into(),
                from_version: old.release_version.clone(),
                from_coop_id: old.coop_id.clone(),
                to_version: to_version.to_string(),
                started_utc: util::utc_stamp(),
                ..Default::default()
            },
        };
        s.write(guard)?;
        Ok(s)
    }

    /// The store in `dir` (None when it has no readable record).
    pub fn open(dir: &Path) -> Option<Store> {
        let t = std::fs::read_to_string(dir.join(RECORD)).ok()?;
        let rec: Record = serde_json::from_str(&t).ok()?;
        dir.join(OLD_STATE).is_file().then(|| Store { dir: dir.to_path_buf(), rec })
    }

    pub fn write(&self, guard: &Guard) -> std::io::Result<()> {
        let mut t = serde_json::to_string_pretty(&self.rec).map_err(std::io::Error::other)?;
        t.push('\n');
        guard.write_atomic(&self.dir.join(RECORD), t)
    }

    /// Before `root\rel` is replaced: keeps X's copy (once). A file that does not exist yet is recorded as added.
    /// Returns true when a copy was made now.
    pub fn save_file(&mut self, root: &Path, rel: &str, guard: &Guard) -> std::io::Result<bool> {
        let rel = rel.replace('\\', "/");
        let src = rel_path(root, &rel);
        let dst = rel_path(&self.dir.join("files"), &rel);
        if !src.is_file() {
            if !dst.exists() && !self.rec.added.contains(&rel) {
                self.rec.added.push(rel);
                self.write(guard)?;
            }
            return Ok(false);
        }
        if dst.exists() || self.rec.added.contains(&rel) {
            return Ok(false);
        }
        // through a temporary name: a file under its final name is always a complete copy (a power cut during the
        // copy leaves only the part file, and the resumed update copies again; first COMPLETE copy wins)
        let part = dst.with_file_name(format!("{}{PART}", dst.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
        guard.copy(&src, &part)?;
        guard.rename(&part, &dst)?;
        Ok(true)
    }

    /// Instead of deleting the build output `root\rel` before a rebuild: moves it into the store (once; when X's copy
    /// is already there, the current one is deleted). Returns true when something was moved now.
    pub fn move_away(&self, root: &Path, rel: &str, guard: &Guard) -> std::io::Result<bool> {
        let src = rel_path(root, rel);
        if !src.exists() {
            return Ok(false);
        }
        let dst = rel_path(&self.dir.join("moved"), rel);
        if dst.exists() {
            if src.is_dir() { guard.remove_dir_all(&src) } else { guard.remove_file(&src) }?;
            return Ok(false);
        }
        if let Some(p) = dst.parent() {
            guard.create_dir_all(p)?;
        }
        guard.rename(&src, &dst)?;
        Ok(true)
    }

    /// The update passed.
    pub fn finish(&mut self, to_coop_id: &str, guard: &Guard) -> std::io::Result<()> {
        self.rec.complete = true;
        self.rec.completed_utc = util::utc_stamp();
        self.rec.to_coop_id = to_coop_id.to_string();
        self.write(guard)
    }

    /// X's install record.
    pub fn old_state(&self) -> Option<InstallState> {
        let t = std::fs::read_to_string(self.dir.join(OLD_STATE)).ok()?;
        serde_json::from_str(&t).ok()
    }

    /// Root-relative files kept under `files\`, sorted.
    pub fn saved_files(&self) -> Vec<String> {
        let base = self.dir.join("files");
        util::walk_files(&base)
            .unwrap_or_default()
            .iter()
            .filter(|p| !p.to_string_lossy().ends_with(PART))
            .filter_map(|p| p.strip_prefix(&base).ok().map(util::rel_key))
            .collect()
    }

    /// Root-relative entries kept under `moved\` ([`MOVE_ON_UPDATE`] that were moved).
    pub fn moved(&self) -> Vec<String> {
        MOVE_ON_UPDATE.iter().filter(|r| rel_path(&self.dir.join("moved"), r).exists()).map(|r| r.to_string()).collect()
    }
}

/// The store a rollback would use: the one whose target is the installed version (or an update in progress).
pub fn available(root: &Path) -> Option<Store> {
    let st = InstallState::load(root)?;
    let rd = std::fs::read_dir(root.join(DIR)).ok()?;
    let mut v: Vec<Store> = rd.flatten().filter_map(|e| Store::open(&e.path())).collect();
    v.retain(|s| s.rec.to_version == st.release_version || (!st.update_from.is_empty() && s.rec.from_version == st.update_from));
    v.pop()
}

/// What a rollback does (reads only).
#[derive(Clone, Debug, PartialEq)]
pub struct RollbackPlan {
    pub root: PathBuf,
    pub store: Store,
    /// The installed version now (the update's target) and the version restored.
    pub from: String,
    pub to: String,
    pub restore: Vec<String>,
    pub unmove: Vec<String>,
    pub delete: Vec<String>,
}

impl RollbackPlan {
    pub fn describe(&self) -> String {
        let mut s = format!(
            "roll back {} from {} to {}{}\n",
            self.root.display(),
            self.from,
            self.to,
            if self.store.rec.complete { "" } else { " (the update did not finish)" }
        );
        for r in &self.restore {
            s.push_str(&format!("  restore {r}\n"));
        }
        for r in &self.unmove {
            s.push_str(&format!("  restore {r} (the build output of {})\n", self.to));
        }
        for r in &self.delete {
            s.push_str(&format!("  delete  {r} (new in {})\n", self.from));
        }
        s.push_str(&format!("  restore state\\install.json ({} record)\n", self.to));
        s
    }
}

pub fn plan(root: &Path) -> Result<RollbackPlan, String> {
    let st = InstallState::load(root).ok_or_else(|| format!("no install in {}", root.display()))?;
    let store = available(root).ok_or_else(|| format!("nothing to roll back in {} (no earlier version was kept)", root.display()))?;
    Ok(RollbackPlan {
        root: root.to_path_buf(),
        from: st.release_version.clone(),
        to: store.rec.from_version.clone(),
        restore: store.saved_files(),
        unmove: store.moved(),
        delete: store.rec.added.clone(),
        store,
    })
}

/// Puts the old version back. `self_exe`: the running program (when it is the root's MW2inER.exe it is renamed
/// aside first; Windows allows renaming a running exe). Returns the lines done.
pub fn execute(p: &RollbackPlan, guard: &Guard, self_exe: Option<&Path>) -> Result<Vec<String>, String> {
    let root = &p.root;
    let e = |x: std::io::Error| x.to_string();
    let mut done = Vec::new();
    let old = p.store.old_state().ok_or("the store has no install record")?;
    // the newer release's own file (the merge base of the newer install): read before the restore puts X's back
    let newer_base = std::fs::read_to_string(root.join(crate::run::RELEASE_TOML)).ok();
    for rel in &p.delete {
        let f = rel_path(root, rel);
        if f.is_file() {
            guard.remove_file(&f).map_err(e)?;
            done.push(format!("deleted {rel}"));
        }
    }
    for rel in &p.unmove {
        let cur = rel_path(root, rel);
        if cur.is_dir() {
            guard.remove_dir_all(&cur).map_err(e)?;
        } else if cur.is_file() {
            guard.remove_file(&cur).map_err(e)?;
        }
        if let Some(d) = cur.parent() {
            guard.create_dir_all(d).map_err(e)?;
        }
        guard.rename(&rel_path(&p.store.dir.join("moved"), rel), &cur).map_err(e)?;
        done.push(format!("restored {rel}"));
    }
    let trash = root.join(r"state\trash");
    for rel in p.restore.iter().filter(|r| *r != SETTINGS) {
        let to = rel_path(root, rel);
        let from = rel_path(&p.store.dir.join("files"), rel);
        if self_exe.is_some_and(|s| util::same_path(s, &to)) {
            let aside = trash.join(format!("{}.{}.old", to.file_name().and_then(|n| n.to_str()).unwrap_or("exe"), util::local_folder_stamp()));
            guard.create_dir_all(&trash).map_err(e)?;
            guard.rename(&to, &aside).map_err(e)?;
        }
        let tmp = to.with_file_name(format!("{}.rollback-tmp", to.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
        guard.copy(&from, &tmp).map_err(e)?;
        guard.rename(&tmp, &to).map_err(e)?;
        done.push(format!("restored {rel}"));
    }
    if p.restore.iter().any(|r| r == SETTINGS) {
        done.extend(restore_settings(p, guard, newer_base.as_deref())?);
    }
    let mut st = old;
    // P4 review: the shortcuts on disk now are the newer release's (an update to 0.3.0 renamed them); X's record
    // must own them, or X's uninstall leaves them behind (read the current record before X's replaces it)
    let current = InstallState::load(root).map(|c| c.shortcuts).unwrap_or_default();
    done.extend(restore_shortcuts(&current, &mut st, guard));
    st.update_from.clear();
    st.last_result = format!("ROLLED BACK from {} to {} {}", p.from, p.to, util::utc_stamp());
    st.save(root, guard).map_err(e)?;
    done.push(format!("restored the {} install record", p.to));
    guard.remove_dir_all(&p.store.dir).map_err(e)?;
    done.push(format!("rolled back to {} (co-op id {})", p.to, st.coop_id));
    Ok(done)
}

/// The shortcuts of the rolled-back record `st` (X's): every shortcut the current record lists and that exists is
/// handed over to X. When X used a pre-0.3.0 name ([`crate::LEGACY_DISPLAY_NAMES`]; X's record lists such a
/// shortcut) a current-name shortcut is moved back to X's name and place (`<name>.lnk` on the Desktop, the Start-menu
/// folder `<old name>\`), or deleted when X's own file is there already, and the empty current Start-menu folder is
/// removed: X's uninstall and repair then find exactly their own shortcuts. Otherwise it is recorded as it is (X's
/// uninstall removes recorded shortcuts). X's entries whose file is gone are dropped. Shortcut problems never stop
/// the rollback (a WARN line; the shortcut stays recorded).
fn restore_shortcuts(current: &[PathBuf], st: &mut InstallState, guard: &Guard) -> Vec<String> {
    use crate::run::{SHORTCUT_FOLDER, is_legacy_shortcut, shortcut_names};
    let mut done = Vec::new();
    let has = |v: &[PathBuf], p: &Path| v.iter().any(|x| util::same_path(x, p));
    // the old name X used (the first legacy name of a shortcut in X's record)
    let legacy = crate::LEGACY_DISPLAY_NAMES.iter().copied().find(|n| {
        let names = shortcut_names(n).map(|s| s.to_lowercase());
        st.shortcuts.iter().any(|s| s.file_name().is_some_and(|f| names.contains(&f.to_string_lossy().to_lowercase())))
    });
    let new_names = shortcut_names(crate::DISPLAY_NAME).map(|s| s.to_lowercase());
    let mut emptied: Vec<PathBuf> = Vec::new();
    for s in current {
        if !s.is_file() || has(&st.shortcuts, s) {
            continue;
        }
        let idx = s.file_name().and_then(|f| new_names.iter().position(|n| *n == f.to_string_lossy().to_lowercase()));
        let (Some(old_name), Some(i), false) = (legacy, idx, is_legacy_shortcut(s)) else {
            st.shortcuts.push(s.clone());
            done.push(format!("kept the shortcut {} (recorded)", s.display()));
            continue;
        };
        let Some(parent) = s.parent() else { continue };
        let in_folder = parent.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(SHORTCUT_FOLDER));
        let dir = match (in_folder, parent.parent()) {
            (true, Some(pp)) => pp.join(old_name),
            _ => parent.to_path_buf(),
        };
        let to = dir.join(&shortcut_names(old_name)[i]);
        let r = if to.is_file() {
            guard.remove_file(s).map(|()| format!("deleted the shortcut {} ({} is there)", s.display(), to.display()))
        } else {
            guard.create_dir_all(&dir).and_then(|()| guard.rename(s, &to)).map(|()| format!("renamed the shortcut {} back to {}", s.display(), to.display()))
        };
        match r {
            Ok(line) => {
                done.push(line);
                if !has(&st.shortcuts, &to) {
                    st.shortcuts.push(to);
                }
                if in_folder && !has(&emptied, parent) {
                    emptied.push(parent.to_path_buf());
                }
            }
            Err(e) => {
                done.push(format!("WARN could not move the shortcut {} back: {e}; it stays recorded", s.display()));
                st.shortcuts.push(s.clone());
            }
        }
    }
    for d in emptied {
        let _ = std::fs::remove_dir(&d); // only when empty
    }
    st.shortcuts.retain(|s| s.is_file());
    done
}

/// The player's settings file (root-relative).
const SETTINGS: &str = "mw2er.toml";

/// X's `mw2er.toml` back, without losing what the player changed after the update: the current file is backed up
/// (`backups\settings`, like the update's merge does) and merged into X's ([`crate::toml_merge::merge`] with X's file as
/// the "release" and the newer release's file as the base: a hot value the player changed since the update is
/// kept, everything else is X's). Without a base the current file cannot be told apart from the newer release's
/// defaults: X's file is restored as it was (the current one stays in the backup).
fn restore_settings(p: &RollbackPlan, guard: &Guard, newer_base: Option<&str>) -> Result<Vec<String>, String> {
    let e = |x: std::io::Error| x.to_string();
    let root = &p.root;
    let to = rel_path(root, SETTINGS);
    let saved = std::fs::read_to_string(rel_path(&p.store.dir.join("files"), SETTINGS)).map_err(e)?;
    let mut done = Vec::new();
    let current = std::fs::read_to_string(&to).ok();
    let mut text = saved.clone();
    if let Some(cur) = current.as_deref().filter(|c| *c != saved) {
        let backup = root.join(crate::run::SETTINGS_BACKUPS).join(format!("mw2er-{}-{}.toml", safe_name(&p.from), util::local_folder_stamp()));
        guard.copy(&to, &backup).map_err(e)?;
        done.push(format!("your current mw2er.toml is backed up as {}", backup.display()));
        if let Some(base) = newer_base {
            // X's release.json is back already (restored above): its locked keys
            let locked = crate::release::Release::load(root).map(|r| r.locked_toml).unwrap_or_default();
            let m = crate::toml_merge::merge(cur, Some(base), &saved, &locked);
            if !m.kept.is_empty() {
                done.push(format!("kept {} setting(s) you changed after the update: {}", m.kept.len(), m.kept.join(", ")));
            }
            text = m.text;
        } else {
            done.push(format!("restored the {} settings as they were (changes made after the update are in the backup)", p.to));
        }
    }
    let tmp = to.with_file_name(format!("{SETTINGS}.rollback-tmp"));
    guard.write(&tmp, text.as_bytes()).map_err(e)?;
    guard.rename(&tmp, &to).map_err(e)?;
    done.push(format!("restored {SETTINGS}"));
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn installed(t: &TempDir, ver: &str) -> InstallState {
        let mut st = InstallState::new();
        st.release_version = ver.into();
        st.coop_id = format!("id{ver}");
        st.record_step("payload", "k1");
        st.save(&t.0.join("root"), &Guard::default()).unwrap();
        st
    }

    #[test]
    fn store_saves_once_moves_and_rolls_back() {
        let t = TempDir::new("rollback");
        let g = Guard::default();
        let root = t.0.join("root");
        t.write(r"root\build\natives\mw2er.dll", b"dll 0.2.0");
        t.write(r"root\mw2er.toml", "[keys]\ntoggle = \"F6\"\n");
        t.write(r"root\converted\weapons.json", b"old converted");
        t.write(r"root\build\package\regulation.bin", b"old regulation");
        t.write(r"root\MW2inER.exe", b"old exe");
        let old = installed(&t, "0.2.0");
        // an older store is removed when a new update starts
        t.write(r"root\state\rollback\0.1.0\rollback.json", "{}");
        let mut s = Store::begin(&root, &old, "0.2.1", &g).unwrap();
        assert!(!root.join(r"state\rollback\0.1.0").exists());
        assert!(s.save_file(&root, "build/natives/mw2er.dll", &g).unwrap());
        assert!(s.save_file(&root, "mw2er.toml", &g).unwrap());
        assert!(s.save_file(&root, "MW2inER.exe", &g).unwrap());
        assert!(!s.save_file(&root, "tools/scripts/new.py", &g).unwrap(), "new file: recorded as added");
        // the update writes its files ...
        t.write(r"root\build\natives\mw2er.dll", b"dll 0.2.1");
        t.write(r"root\tools\scripts\new.py", b"new");
        assert!(!s.save_file(&root, "build/natives/mw2er.dll", &g).unwrap(), "first copy wins");
        assert!(s.move_away(&root, "converted", &g).unwrap());
        assert!(s.move_away(&root, "build/package", &g).unwrap());
        assert!(!s.move_away(&root, "build/rows.json", &g).unwrap(), "absent");
        t.write(r"root\converted\weapons.json", b"new converted");
        t.write(r"root\build\package\regulation.bin", b"new regulation");
        t.write(r"root\MW2inER.exe", b"new exe");
        t.write(r"root\mw2er.toml", "[keys]\ntoggle = \"F6\"\nnew = 1\n");
        let mut st = old.clone();
        st.release_version = "0.2.1".into();
        st.coop_id = "id0.2.1".into();
        st.save(&root, &g).unwrap();
        s.finish("id0.2.1", &g).unwrap();
        // resume: an existing store is reopened, not rewritten
        let again = Store::begin(&root, &old, "0.2.1", &g).unwrap();
        assert_eq!(again.rec.from_version, "0.2.0");
        assert!(again.rec.complete);
        // plan + execute (the running exe is the root's: renamed aside)
        let p = plan(&root).unwrap();
        assert_eq!((p.from.as_str(), p.to.as_str()), ("0.2.1", "0.2.0"));
        assert_eq!(p.restore, vec!["MW2inER.exe".to_string(), "build/natives/mw2er.dll".into(), "mw2er.toml".into()]);
        assert_eq!(p.unmove, vec!["converted".to_string(), "build/package".into()]);
        assert_eq!(p.delete, vec!["tools/scripts/new.py".to_string()]);
        assert!(p.describe().contains("from 0.2.1 to 0.2.0"));
        let lines = execute(&p, &g, Some(&root.join("MW2inER.exe"))).unwrap();
        assert!(lines.last().unwrap().contains("rolled back to 0.2.0 (co-op id id0.2.0)"), "{lines:?}");
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"dll 0.2.0");
        assert_eq!(std::fs::read(root.join(r"converted\weapons.json")).unwrap(), b"old converted");
        assert_eq!(std::fs::read(root.join(r"build\package\regulation.bin")).unwrap(), b"old regulation");
        assert_eq!(std::fs::read(root.join("MW2inER.exe")).unwrap(), b"old exe");
        assert_eq!(std::fs::read_to_string(root.join("mw2er.toml")).unwrap(), "[keys]\ntoggle = \"F6\"\n");
        assert!(!root.join(r"tools\scripts\new.py").exists());
        assert_eq!(std::fs::read_dir(root.join(r"state\trash")).unwrap().count(), 1, "the running exe was renamed aside");
        let back = InstallState::load(&root).unwrap();
        assert_eq!((back.release_version.as_str(), back.coop_id.as_str()), ("0.2.0", "id0.2.0"));
        assert_eq!(back.step_key("payload"), Some("k1"));
        assert!(back.last_result.starts_with("ROLLED BACK from 0.2.1 to 0.2.0"));
        assert!(!root.join(DIR).join("0.2.0").exists(), "the store is consumed");
        assert!(plan(&root).unwrap_err().contains("nothing to roll back"));
    }

    /// A copy cut off half-way (power cut) leaves only a part file: it is never restored, and the resumed update
    /// copies the original again.
    #[test]
    fn a_cut_off_copy_is_not_kept() {
        let t = TempDir::new("rollback-part");
        let g = Guard::default();
        let root = t.0.join("root");
        t.write(r"root\build\natives\mw2er.dll", b"dll 0.2.0");
        let old = installed(&t, "0.2.0");
        let mut s = Store::begin(&root, &old, "0.2.1", &g).unwrap();
        let part = s.dir.join(format!(r"files\build\natives\mw2er.dll{PART}"));
        t.write(part.strip_prefix(&t.0).unwrap().to_str().unwrap(), b"dll 0");
        assert!(s.saved_files().is_empty(), "a part file is not a saved file");
        assert!(s.save_file(&root, "build/natives/mw2er.dll", &g).unwrap(), "copied again");
        assert!(!part.exists());
        assert_eq!(std::fs::read(s.dir.join(r"files\build\natives\mw2er.dll")).unwrap(), b"dll 0.2.0");
        assert_eq!(s.saved_files(), vec!["build/natives/mw2er.dll".to_string()]);
    }

    /// P4 review: 0.2.0 (old shortcut names) -> 0.3.0 (the update renamed them) -> rollback: X's record owns the
    /// shortcuts on disk again, under X's names and Start-menu folder; a shortcut X never had is recorded as it is.
    #[test]
    fn rollback_hands_the_current_shortcuts_back_to_the_old_record() {
        let t = TempDir::new("rollback-lnk");
        let g = Guard::default();
        let root = t.0.join("root");
        t.write(r"root\mw2er.toml", "");
        let desk = t.0.join("Desktop");
        let progs = t.0.join("Programs");
        let (old_n, new_n) = (crate::LEGACY_DISPLAY_NAMES[0], crate::DISPLAY_NAME);
        let old_lnk = |d: &Path, i: usize| d.join(&crate::run::shortcut_names(old_n)[i]);
        let new_lnk = |d: &Path, i: usize| d.join(&crate::run::shortcut_names(new_n)[i]);
        let mut old = installed(&t, "0.2.0");
        old.shortcuts = vec![old_lnk(&desk, 0), old_lnk(&desk, 2), old_lnk(&progs.join(old_n), 0), old_lnk(&progs.join(old_n), 2)];
        old.save(&root, &g).unwrap();
        let s = Store::begin(&root, &old, "0.3.0", &g).unwrap();
        // the 0.3.0 update replaced the old shortcuts (deleted, the old folder removed) with new-name ones
        let made = vec![new_lnk(&desk, 0), new_lnk(&desk, 2), new_lnk(&progs.join(new_n), 0), new_lnk(&progs.join(new_n), 2), new_lnk(&desk, 1)];
        for m in &made {
            t.write(m.strip_prefix(&t.0).unwrap().to_str().unwrap(), b"lnk");
        }
        let mut st = old.clone();
        st.release_version = "0.3.0".into();
        st.shortcuts = made.clone();
        st.save(&root, &g).unwrap();
        drop(s);
        let p = plan(&root).unwrap();
        let lines = execute(&p, &g, None).unwrap();
        let back = InstallState::load(&root).unwrap();
        assert_eq!(back.release_version, "0.2.0");
        for want in &old.shortcuts {
            assert!(want.is_file(), "{} renamed back ({lines:?})", want.display());
            assert!(back.shortcuts.contains(want), "{} recorded", want.display());
        }
        for m in &made[..4] {
            assert!(!m.exists(), "{} moved away", m.display());
        }
        // the co-op one: renamed to X's name too (X's uninstall removes it); the emptied new folder is gone
        assert!(old_lnk(&desk, 1).is_file() && back.shortcuts.contains(&old_lnk(&desk, 1)));
        assert_eq!(back.shortcuts.len(), 5);
        assert!(!progs.join(new_n).exists(), "{lines:?}");
        // second round: an install record without shortcuts of the old name records new-name ones as they are
        let mut x = installed(&t, "0.3.0");
        x.shortcuts = vec![progs.join("gone.lnk")];
        x.save(&root, &g).unwrap();
        let s2 = Store::begin(&root, &x, "0.3.1", &g).unwrap();
        t.write(r"Desktop\Modern Arsenal for Elden Ring.lnk", b"lnk");
        let mut st2 = x.clone();
        st2.release_version = "0.3.1".into();
        st2.shortcuts = vec![new_lnk(&desk, 0)];
        st2.save(&root, &g).unwrap();
        drop(s2);
        execute(&plan(&root).unwrap(), &g, None).unwrap();
        let back2 = InstallState::load(&root).unwrap();
        assert_eq!(back2.shortcuts, vec![new_lnk(&desk, 0)], "recorded as it is; the missing entry dropped");
        assert!(new_lnk(&desk, 0).is_file());
        // the uninstall of the rolled-back install removes it
        let up = crate::uninstall::plan(&root, false, None).unwrap();
        crate::uninstall::execute(&up, &g, false).unwrap();
        assert!(!new_lnk(&desk, 0).exists());
    }

    #[test]
    fn interrupted_update_can_be_rolled_back_and_the_guard_applies() {
        let t = TempDir::new("rollback-int");
        let g = Guard::default();
        let root = t.0.join("root");
        t.write(r"root\build\natives\mw2er.dll", b"dll 0.2.0");
        t.write(r"root\mw2er.toml", "");
        let old = installed(&t, "0.2.0");
        let mut s = Store::begin(&root, &old, "0.3.0", &g).unwrap();
        s.save_file(&root, "build/natives/mw2er.dll", &g).unwrap();
        t.write(r"root\build\natives\mw2er.dll", b"half");
        // the interrupted session: the state says 0.3.0 + update_from 0.2.0
        let mut st = old.clone();
        st.release_version = "0.3.0".into();
        st.update_from = "0.2.0".into();
        st.save(&root, &g).unwrap();
        let p = plan(&root).unwrap();
        assert!(p.describe().contains("did not finish"));
        // a guard forbidding the root refuses before anything changes
        let bad = Guard::new(Some(&root), None, &[], None, None);
        assert!(execute(&p, &bad, None).is_err());
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"half");
        execute(&p, &g, None).unwrap();
        assert_eq!(std::fs::read(root.join(r"build\natives\mw2er.dll")).unwrap(), b"dll 0.2.0");
        let back = InstallState::load(&root).unwrap();
        assert!(back.update_from.is_empty() && back.release_version == "0.2.0");
        assert_eq!(safe_name("../x"), ".._x");
        assert_eq!(safe_name(""), "unknown");
    }
}
