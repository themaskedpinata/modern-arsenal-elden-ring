//! Small helpers shared by every module: sha256 of files and streams, path normalisation and the "is under"
//! test the guard is built on, timestamps, and a hash-checked recursive copy.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// Lowercase hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// sha256 (lowercase hex) of everything `r` yields.
pub fn sha256_reader(mut r: impl Read) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// sha256 (lowercase hex) of a file, streamed.
pub fn sha256_file(p: &Path) -> io::Result<String> {
    sha256_reader(fs::File::open(p)?)
}

/// sha256 (lowercase hex) of bytes.
pub fn sha256_bytes(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// The first `n` characters of a hash, for messages.
pub fn short(h: &str) -> &str {
    &h[..h.len().min(12)]
}

/// True when `s` is 64 lowercase/uppercase hex digits.
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Absolute, lexically normalised path (`.`/`..` resolved, `/` -> `\`, no trailing separator). Does not touch the
/// file system beyond `current_dir` for a relative input, so it works for paths that do not exist yet.
pub fn normalize(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                // never pop the prefix / root
                if out.components().count() > 2 || (out.components().count() == 2 && !out.has_root()) {
                    out.pop();
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Comparable form: normalised, lowercase (Windows paths are case-insensitive), `\` separators, trailing `\`.
fn key(p: &Path) -> String {
    let mut s = normalize(p).to_string_lossy().replace('/', "\\").to_lowercase();
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        s = rest.to_string();
    }
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

/// True when `child` is `parent` or inside it (case-insensitive, lexical; the PowerShell `Under` of setup.ps1).
pub fn is_under(child: &Path, parent: &Path) -> bool {
    if child.as_os_str().is_empty() || parent.as_os_str().is_empty() {
        return false;
    }
    key(child).starts_with(&key(parent))
}

/// True when both paths name the same location (lexically).
pub fn same_path(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Civil date (UTC) of a Unix time: (year, month, day, hour, minute, second).
pub fn civil(t: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // Howard Hinnant's days_from_civil inverse
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32, (secs % 60) as u32)
}

/// ISO-8601 UTC timestamp, e.g. `2026-10-22T14:06:15Z`.
pub fn utc_stamp() -> String {
    let (y, mo, d, h, mi, s) = civil(unix_now());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Local-time folder stamp `yyyy-MM-dd_HHmmss` (the backup folder names of setup.ps1 / launch.ps1).
pub fn local_folder_stamp() -> String {
    // SAFETY: GetLocalTime has no preconditions; it only returns a SYSTEMTIME by value.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Copies every file under `src` into `dst` (created), checking each copy's sha256 against its source.
/// Returns the number of files. The source is only read.
pub fn copy_tree_checked(src: &Path, dst: &Path) -> io::Result<usize> {
    let mut n = 0;
    for f in walk_files(src)? {
        let rel = f.strip_prefix(src).map_err(|_| io::Error::other("walk outside source"))?;
        let to = dst.join(rel);
        if let Some(p) = to.parent() {
            fs::create_dir_all(p)?;
        }
        fs::copy(&f, &to)?;
        if sha256_file(&f)? != sha256_file(&to)? {
            return Err(io::Error::other(format!("{} differs after copying", rel.display())));
        }
        n += 1;
    }
    Ok(n)
}

/// Every file under `dir` (recursive, sorted; symlinked directories are not followed).
pub fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let t = e.file_type()?;
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// `rel` with `/` separators (manifest keys).
pub fn rel_key(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};

    /// A fresh empty folder under the target dir's tmp (removed on drop). Never under a Steam library or %APPDATA%.
    pub struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new(tag: &str) -> TempDir {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let base = std::env::temp_dir().join("mw2er-installer-tests");
            let p = base.join(format!("{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
        pub fn write(&self, rel: &str, data: impl AsRef<[u8]>) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempDir;
    use super::*;

    #[test]
    fn sha256_known_answers() {
        assert_eq!(sha256_bytes(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_bytes(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let t = TempDir::new("sha");
        let p = t.write("a.bin", b"abc");
        assert_eq!(sha256_file(&p).unwrap(), sha256_bytes(b"abc"));
    }

    #[test]
    fn under_is_case_insensitive_and_component_wise() {
        assert!(is_under(Path::new(r"D:\Steam\steamapps\common\ELDEN RING\Game\x"), Path::new(r"d:\steam\STEAMAPPS")));
        assert!(is_under(Path::new(r"D:\a\b"), Path::new(r"D:\a\b\")));
        assert!(is_under(Path::new(r"D:/a/b/c"), Path::new(r"D:\a")));
        assert!(!is_under(Path::new(r"D:\ab"), Path::new(r"D:\a")));
        assert!(!is_under(Path::new(r"D:\a"), Path::new(r"D:\a\b")));
        assert!(is_under(Path::new(r"D:\a\b\..\c"), Path::new(r"D:\a")));
        assert!(!is_under(Path::new(r"D:\a\..\x"), Path::new(r"D:\a")));
        assert!(!is_under(Path::new(""), Path::new(r"D:\a")));
        assert!(same_path(Path::new(r"C:\X\y\"), Path::new(r"c:\x\Y")));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(civil(1_792_659_392), (2026, 10, 22, 8, 56, 32));
        assert_eq!(utc_stamp().len(), 20);
        assert_eq!(local_folder_stamp().len(), 17);
    }

    #[test]
    fn copy_tree_checks_every_file() {
        let t = TempDir::new("copy");
        t.write("src/a.txt", "a");
        t.write("src/sub/b.bin", [1u8, 2, 3]);
        let n = copy_tree_checked(&t.0.join("src"), &t.0.join("dst")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(t.0.join("dst/sub/b.bin")).unwrap(), vec![1, 2, 3]);
        assert_eq!(walk_files(&t.0.join("dst")).unwrap().len(), 2);
    }
}
