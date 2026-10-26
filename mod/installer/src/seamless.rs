//! Seamless Co-op (the player's own download; never bundled, downloaded or patched by this program):
//! detection (a folder, a zip in Downloads, remembered paths; never the copy inside the Elden Ring folder), the
//! copy into `<root>\seamless\SeamlessCoop\` (from a folder or straight out of the zip), and `ersc_settings.ini`.
//!
//! The co-op password: [`Secret`] has no `Debug` content, no `Display`, no `Serialize`; it is written only into the
//! mod's own `ersc_settings.ini` copy (plain text, as Seamless requires). [`IniState`] reports only whether the
//! password line is non-empty; nothing in this crate returns, logs or stores the value.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::guard::Guard;
use crate::util;

/// The folder inside the install root that holds the copy.
pub const COPY_DIR: &str = r"seamless\SeamlessCoop";
pub const INI: &str = "ersc_settings.ini";
pub const SAVE_EXT: &str = "mw2co2";

/// The co-op password. Its only way out is [`set_ini_keys`].
pub struct Secret(String);

impl Secret {
    /// Rejects an empty password and line breaks / control characters (they would break the ini line).
    pub fn new(s: String) -> Result<Secret, &'static str> {
        let t = s.trim();
        if t.is_empty() {
            return Err("the co-op password is empty");
        }
        if t.chars().any(|c| c.is_control()) {
            return Err("the co-op password contains a line break or control character");
        }
        let t = t.to_string();
        let mut s = s;
        wipe(&mut s);
        Ok(Secret(t))
    }

    fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// `text` with every occurrence of the password replaced by `<hidden>` (belt and braces for log lines; the
    /// password never leaves through here). Passwords shorter than 3 characters are not searched for.
    pub fn redact(&self, text: &str) -> String {
        if self.0.chars().count() >= 3 && text.contains(self.0.as_str()) {
            text.replace(self.0.as_str(), "<hidden>")
        } else {
            text.to_string()
        }
    }
}

fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8; the string is cleared right after.
    unsafe { s.as_bytes_mut().fill(0) };
    s.clear();
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<hidden>)")
    }
}

/// What the program may know about an ini: two values and whether a password is set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IniState {
    pub allow_invaders: Option<String>,
    pub save_file_extension: Option<String>,
    pub password_line: bool,
    pub password_set: bool,
}

impl IniState {
    /// The two values the mod needs (the password is checked separately).
    pub fn keys_ok(&self) -> bool {
        self.allow_invaders.as_deref() == Some("0") && self.save_file_extension.as_deref() == Some(SAVE_EXT)
    }
}

/// `(start, value_start, value_end)` byte offsets of `key = value` lines (ini keys at the line start).
fn find_key(b: &[u8], key: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let end = b[i..].iter().position(|&c| c == b'\n' || c == b'\r').map_or(b.len(), |p| i + p);
        let line = &b[i..end];
        let mut j = 0;
        while j < line.len() && (line[j] == b' ' || line[j] == b'\t') {
            j += 1;
        }
        if line[j..].len() >= key.len() && line[j..j + key.len()].eq_ignore_ascii_case(key.as_bytes()) {
            let mut k = j + key.len();
            while k < line.len() && (line[k] == b' ' || line[k] == b'\t') {
                k += 1;
            }
            if k < line.len() && line[k] == b'=' {
                out.push((i, i + k + 1, end));
            }
        }
        i = end;
        while i < b.len() && (b[i] == b'\n' || b[i] == b'\r') {
            i += 1;
        }
    }
    out
}

fn value(b: &[u8], key: &str) -> Option<String> {
    let (_, s, e) = *find_key(b, key).last()?;
    Some(String::from_utf8_lossy(&b[s..e]).trim().to_string())
}

/// Reads the state (never the password value).
pub fn read_ini_state(ini: &Path) -> std::io::Result<IniState> {
    let b = fs::read(ini)?;
    let pw = find_key(&b, "cooppassword").last().copied();
    Ok(IniState {
        allow_invaders: value(&b, "allow_invaders"),
        save_file_extension: value(&b, "save_file_extension"),
        password_line: pw.is_some(),
        password_set: pw.is_some_and(|(_, s, e)| b[s..e].iter().any(|c| !c.is_ascii_whitespace())),
    })
}

/// Sets `allow_invaders = 0`, `save_file_extension = mw2co2` and (when given) `cooppassword = <password>` in
/// place: every other byte of the file stays as it was (BOM, line endings, comments, other keys). Refuses when a
/// key line is missing (a damaged ini: re-import Seamless). Writes through the guard.
pub fn set_ini_keys(ini: &Path, password: Option<&Secret>, guard: &Guard) -> Result<(), String> {
    let b = fs::read(ini).map_err(|e| format!("cannot read {}: {e}", ini.display()))?;
    let mut edits: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let mut want: Vec<(&str, Vec<u8>)> = vec![("allow_invaders", b" 0".to_vec()), ("save_file_extension", format!(" {SAVE_EXT}").into_bytes())];
    if let Some(p) = password {
        let mut v = b" ".to_vec();
        v.extend_from_slice(p.bytes());
        want.push(("cooppassword", v));
    }
    for (k, v) in want {
        let found = find_key(&b, k);
        if found.is_empty() {
            return Err(format!("{} has no '{k} =' line: re-import Seamless Co-op (its ini is incomplete)", ini.display()));
        }
        for (_, s, e) in found {
            edits.push((s, e, v.clone()));
        }
    }
    edits.sort_by_key(|e| e.0);
    let mut out = Vec::with_capacity(b.len() + 64);
    let mut pos = 0;
    for (s, e, v) in &edits {
        out.extend_from_slice(&b[pos..*s]);
        out.extend_from_slice(v);
        pos = *e;
    }
    out.extend_from_slice(&b[pos..]);
    let r = guard.write_atomic(ini, &out).map_err(|e| format!("cannot write {}: {e}", ini.display()));
    out.fill(0);
    r
}

// ---- detection -------------------------------------------------------------------------------------------------

/// Where a pinned Seamless was found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// `...\SeamlessCoop\ersc.dll` in a folder.
    Dll(PathBuf),
    /// A zip containing `SeamlessCoop/ersc.dll` with the pinned sha256.
    Zip(PathBuf),
}

/// sha256 of `SeamlessCoop/ersc.dll` inside a zip (with the entry name); None when absent / unreadable.
pub fn zip_ersc(zip_path: &Path) -> Option<(String, String)> {
    let mut z = zip::ZipArchive::new(fs::File::open(zip_path).ok()?).ok()?;
    let name = ersc_entry_name(&mut z)?;
    let e = z.by_name(&name).ok()?;
    Some((name, util::sha256_reader(e).ok()?))
}

fn ersc_entry_name<R: std::io::Read + std::io::Seek>(z: &mut zip::ZipArchive<R>) -> Option<String> {
    z.file_names()
        .filter_map(Result::ok)
        .find(|n| {
            let n = n.replace('\\', "/").to_lowercase();
            n == "seamlesscoop/ersc.dll" || n.ends_with("/seamlesscoop/ersc.dll")
        })
        .map(|n| n.to_string())
}

/// Looks for the pinned Seamless without asking (setup_ui.ps1 Find-SeamlessAuto): `direct` candidates (remembered
/// path, the copy inside the root), then `<base>\SeamlessCoop\ersc.dll` and `<base>\<dir>\SeamlessCoop\ersc.dll`
/// for each base, then `*seamless*.zip` in `zip_dir` (newest first). A copy under `er_parent` (the ELDEN RING
/// folder) is never used.
pub fn find_auto(pin: &str, direct: &[PathBuf], bases: &[PathBuf], zip_dir: Option<&Path>, er_parent: Option<&Path>) -> Option<Found> {
    let mut cands: Vec<PathBuf> = direct.to_vec();
    for b in bases.iter().filter(|b| b.is_dir()) {
        cands.push(b.join(r"SeamlessCoop\ersc.dll"));
        if let Ok(rd) = fs::read_dir(b) {
            let mut subs: Vec<PathBuf> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()).collect();
            subs.sort();
            cands.extend(subs.into_iter().map(|s| s.join(r"SeamlessCoop\ersc.dll")));
        }
    }
    for c in cands {
        if !c.is_file() || er_parent.is_some_and(|e| util::is_under(&c, e)) {
            continue;
        }
        if util::sha256_file(&c).is_ok_and(|h| h.eq_ignore_ascii_case(pin)) {
            return Some(Found::Dll(util::normalize(&c)));
        }
    }
    let dir = zip_dir.filter(|d| d.is_dir())?;
    let mut zips: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.ends_with(".zip") && n.contains("seamless")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    zips.sort_by(|a, b| b.0.cmp(&a.0));
    zips.into_iter()
        .map(|(_, p)| p)
        .find(|p| zip_ersc(p).is_some_and(|(_, h)| h.eq_ignore_ascii_case(pin)))
        .map(Found::Zip)
}

/// Copies the player's Seamless into `<root>\seamless\SeamlessCoop` (the source is only read). An existing copy
/// with the pinned `ersc.dll` in a recorded install (`state\install.json`) is kept as it is (its ini may already hold the
/// password); a source folder that overlaps the copy is refused. Returns the copied `ersc.dll`.
pub fn import(src: &Found, root: &Path, pin: &str, er_parent: Option<&Path>, guard: &Guard) -> Result<PathBuf, String> {
    let target = root.join(COPY_DIR);
    let dll = target.join("ersc.dll");
    // an existing copy is ours only in a recorded install (else it may be the player's own Seamless folder)
    let recorded = crate::state::InstallState::path(root).is_file();
    if recorded && dll.is_file() && util::sha256_file(&dll).is_ok_and(|h| h.eq_ignore_ascii_case(pin)) {
        return Ok(dll);
    }
    match src {
        Found::Dll(d) => {
            if er_parent.is_some_and(|e| util::is_under(d, e)) {
                return Err("that ersc.dll is inside the Elden Ring folder: choose the zip you downloaded or a copy of your own".into());
            }
            let h = util::sha256_file(d).map_err(|e| format!("cannot read {}: {e}", d.display()))?;
            if !h.eq_ignore_ascii_case(pin) {
                return Err(format!("that ersc.dll (sha256 {}) is not the pinned Seamless Co-op ({})", util::short(&h), util::short(pin)));
            }
            let from = d.parent().ok_or("ersc.dll has no folder")?;
            if util::is_under(&target, from) || util::is_under(from, &target) {
                if recorded && util::same_path(from, &target) {
                    return Ok(dll);
                }
                return Err(format!(
                    "the Seamless Co-op folder {} overlaps the install's copy {}: choose another install folder (the player's own Seamless is only read)",
                    from.display(),
                    target.display()
                ));
            }
            guard.remove_dir_all(&target).map_err(|e| e.to_string())?;
            guard.create_dir_all(&target).map_err(|e| e.to_string())?;
            for f in util::walk_files(from).map_err(|e| e.to_string())? {
                let rel = f.strip_prefix(from).map_err(|e| e.to_string())?;
                guard.copy(&f, &target.join(rel)).map_err(|e| format!("copy {}: {e}", rel.display()))?;
            }
        }
        Found::Zip(z) => {
            let mut a = zip::ZipArchive::new(fs::File::open(z).map_err(|e| format!("cannot open {}: {e}", z.display()))?)
                .map_err(|e| format!("cannot read the zip: {e}"))?;
            let name = ersc_entry_name(&mut a).ok_or("this zip has no SeamlessCoop\\ersc.dll")?;
            let h = util::sha256_reader(a.by_name(&name).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if !h.eq_ignore_ascii_case(pin) {
                return Err(format!("the ersc.dll in this zip has sha256 {}, not the pinned Seamless Co-op ({})", util::short(&h), util::short(pin)));
            }
            let prefix = name[..name.len() - "ersc.dll".len()].replace('\\', "/").to_lowercase();
            guard.remove_dir_all(&target).map_err(|e| e.to_string())?;
            guard.create_dir_all(&target).map_err(|e| e.to_string())?;
            for i in 0..a.len() {
                let mut e = a.by_index(i).map_err(|e| e.to_string())?;
                let Ok(name) = e.name() else { continue };
                let n = name.replace('\\', "/");
                if e.is_dir() || !n.to_lowercase().starts_with(&prefix) {
                    continue;
                }
                let rel = &n[prefix.len()..];
                // no path escapes: every component must be a plain name
                let rel_path: PathBuf = rel.split('/').collect();
                if rel.split('/').any(|c| c.is_empty() || c == "." || c == ".." || c.contains(':')) {
                    continue;
                }
                let to = target.join(&rel_path);
                guard.check(&to).map_err(|e| e.to_string())?;
                fs::create_dir_all(to.parent().unwrap_or(&target)).map_err(|e| e.to_string())?;
                let mut buf = Vec::new();
                e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                guard.write(&to, &buf).map_err(|e| e.to_string())?;
            }
        }
    }
    match util::sha256_file(&dll) {
        Ok(h) if h.eq_ignore_ascii_case(pin) => Ok(dll),
        _ => Err("ersc.dll after copying does not match".into()),
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use crate::util::testutil::TempDir;
    use std::io::Write;

    pub const INI_TEXT: &str = "\u{feff}; Seamless Co-op settings\r\n[GAMEPLAY]\r\nallow_invaders = 1\r\ndeath_debuffs = 1\r\n\r\n[PASSWORD]\r\ncooppassword = \r\n\r\n[SAVE]\r\nsave_file_extension = co2\r\n";

    /// A zip with `<top>SeamlessCoop/ersc.dll` (+ ini, locale file, launcher, and a `../` escape attempt).
    pub fn zip(t: &TempDir, rel: &str, top: &str, ersc: &[u8]) -> std::path::PathBuf {
        let p = t.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (n, d) in [
            (format!("{top}SeamlessCoop/ersc.dll"), ersc.to_vec()),
            (format!("{top}SeamlessCoop/{}", super::INI), INI_TEXT.as_bytes().to_vec()),
            (format!("{top}SeamlessCoop/locale/english.json"), b"{}".to_vec()),
            (format!("{top}ersc_launcher.exe"), b"launcher".to_vec()),
            (format!("{top}SeamlessCoop/../../evil.txt"), b"evil".to_vec()),
        ] {
            w.start_file(n, o).unwrap();
            w.write_all(&d).unwrap();
        }
        w.finish().unwrap();
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn pin() -> String {
        util::sha256_bytes(b"ersc")
    }

    #[test]
    fn secret_rules_and_no_leak_in_debug() {
        assert!(Secret::new("   ".into()).is_err());
        assert!(Secret::new("a\nb".into()).is_err());
        let s = Secret::new("  hunter2 ".into()).unwrap();
        assert_eq!(s.bytes(), b"hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn ini_writer_keeps_every_other_byte() {
        let t = TempDir::new("ini");
        let ini = t.write("s/ersc_settings.ini", fixture::INI_TEXT);
        let g = Guard::default();
        let st = read_ini_state(&ini).unwrap();
        assert_eq!(st.allow_invaders.as_deref(), Some("1"));
        assert!(st.password_line && !st.password_set && !st.keys_ok());
        set_ini_keys(&ini, None, &g).unwrap();
        let after = fs::read_to_string(&ini).unwrap();
        assert_eq!(after, fixture::INI_TEXT.replace("allow_invaders = 1", "allow_invaders = 0").replace("save_file_extension = co2", "save_file_extension = mw2co2"));
        let pw = Secret::new("pässwörd 42".into()).unwrap();
        set_ini_keys(&ini, Some(&pw), &g).unwrap();
        let st = read_ini_state(&ini).unwrap();
        assert!(st.keys_ok() && st.password_set);
        let after2 = fs::read(&ini).unwrap();
        let expect = after.replace("cooppassword = ", "cooppassword = pässwörd 42");
        assert_eq!(after2, expect.as_bytes(), "UTF-8 password, BOM and CRLF kept byte for byte");
        // idempotent
        set_ini_keys(&ini, Some(&pw), &g).unwrap();
        assert_eq!(fs::read(&ini).unwrap(), expect.as_bytes());
        // a damaged ini is refused, unchanged
        let bad = t.write("s2/ersc_settings.ini", "[GAMEPLAY]\nallow_invaders = 1\n");
        assert!(set_ini_keys(&bad, None, &g).unwrap_err().contains("save_file_extension"));
        assert_eq!(fs::read_to_string(&bad).unwrap(), "[GAMEPLAY]\nallow_invaders = 1\n");
        // the guard applies
        let g2 = Guard::new(Some(&t.0.join("s")), None, &[], None, None);
        assert!(set_ini_keys(&ini, None, &g2).is_err());
    }

    #[test]
    fn password_never_reaches_messages() {
        let t = TempDir::new("ini-msg");
        let ini = t.write("s/ersc_settings.ini", "[PASSWORD]\ncooppassword = topsecret\n");
        let st = read_ini_state(&ini).unwrap();
        assert!(st.password_set);
        assert!(!format!("{st:?}").contains("topsecret"));
        let pw = Secret::new("other-secret".into()).unwrap();
        let e = set_ini_keys(&ini, Some(&pw), &Guard::default()).unwrap_err();
        assert!(!e.contains("topsecret") && !e.contains("other-secret"), "{e}");
    }

    #[test]
    fn find_and_import_from_zip() {
        let t = TempDir::new("seam-zip");
        let dl = t.0.join("Downloads");
        fixture::zip(&t, "Downloads/Seamless Co-op v2.0.1 510 2.0.1.zip", "", b"ersc");
        fixture::zip(&t, "Downloads/seamless-wrong.zip", "", b"other");
        fixture::zip(&t, "Downloads/unrelated.zip", "", b"ersc");
        let f = find_auto(&pin(), &[], &[], Some(&dl), None).unwrap();
        assert_eq!(f, Found::Zip(dl.join("Seamless Co-op v2.0.1 510 2.0.1.zip")));
        let root = t.0.join("root");
        let g = Guard::default();
        let dll = import(&f, &root, &pin(), None, &g).unwrap();
        assert_eq!(dll, root.join(COPY_DIR).join("ersc.dll"));
        assert!(root.join(COPY_DIR).join("locale/english.json").is_file());
        assert!(!root.join("ersc_launcher.exe").exists() && !root.join(COPY_DIR).join("ersc_launcher.exe").exists());
        assert!(!t.0.join("evil.txt").exists() && !root.join("evil.txt").exists() && !root.join("seamless/evil.txt").exists());
        // an existing pinned copy is reused (its ini keeps the password), but only in a recorded install
        fs::write(root.join(COPY_DIR).join(INI), "[PASSWORD]\ncooppassword = x\n").unwrap();
        crate::state::InstallState::new().save(&root, &g).unwrap();
        import(&f, &root, &pin(), None, &g).unwrap();
        assert!(read_ini_state(&root.join(COPY_DIR).join(INI)).unwrap().password_set);
        // nested top folder
        let z2 = fixture::zip(&t, "z/Seamless.zip", "Seamless Co-op/", b"ersc");
        assert_eq!(zip_ersc(&z2).unwrap().0, "Seamless Co-op/SeamlessCoop/ersc.dll");
        let r2 = t.0.join("root2");
        import(&Found::Zip(z2), &r2, &pin(), None, &g).unwrap();
        assert!(r2.join(COPY_DIR).join(INI).is_file());
        // wrong pin refused
        assert!(import(&Found::Zip(dl.join("seamless-wrong.zip")), &t.0.join("root3"), &pin(), None, &g).is_err());
    }

    #[test]
    fn find_and_import_from_folder_never_the_game_copy() {
        let t = TempDir::new("seam-dir");
        let er_parent = t.0.join(r"lib\steamapps\common\ELDEN RING");
        t.write(r"lib\steamapps\common\ELDEN RING\SeamlessCoop\ersc.dll", b"ersc");
        let in_game = er_parent.join(r"SeamlessCoop\ersc.dll");
        assert_eq!(find_auto(&pin(), std::slice::from_ref(&in_game), std::slice::from_ref(&er_parent), None, Some(&er_parent)), None);
        t.write(r"Documents\seamless\SeamlessCoop\ersc.dll", b"ersc");
        t.write(r"Documents\seamless\SeamlessCoop\ersc_settings.ini", fixture::INI_TEXT);
        let f = find_auto(&pin(), &[], &[t.0.join("Documents")], None, Some(&er_parent)).unwrap();
        let Found::Dll(src) = &f else { panic!() };
        let before = util::sha256_file(&src.with_file_name(INI)).unwrap();
        let root = t.0.join("root");
        import(&f, &root, &pin(), Some(&er_parent), &Guard::default()).unwrap();
        set_ini_keys(&root.join(COPY_DIR).join(INI), None, &Guard::default()).unwrap();
        assert_eq!(util::sha256_file(&src.with_file_name(INI)).unwrap(), before, "the player's own copy is untouched");
        assert!(import(&Found::Dll(in_game), &t.0.join("r2"), &pin(), Some(&er_parent), &Guard::default()).is_err());
        // root = Documents: <root>\seamless\SeamlessCoop IS the player's download; never adopted or rewritten
        let docs = t.0.join("Documents");
        let e = import(&f, &docs, &pin(), Some(&er_parent), &Guard::default()).unwrap_err();
        assert!(e.contains("overlaps"), "{e}");
        assert_eq!(util::sha256_file(&src.with_file_name(INI)).unwrap(), before);
        assert!(docs.join(r"seamless\SeamlessCoop\ersc.dll").is_file());
        // a source folder that holds the copy's folder overlaps too
        let up = Found::Dll(t.write(r"Up\SeamlessCoop\ersc.dll", b"ersc"));
        assert!(import(&up, &t.0.join(r"Up\SeamlessCoop\x"), &pin(), None, &Guard::default()).is_err());
    }
}
