//! The release payload and the bundled Python runtime.
//!
//! - [`embedded`]: the release zip appended to the setup exe by `make_release.ps1` (one downloaded file); see the
//!   overlay section below. It is what the setup uses when no `--payload` is given.
//! - [`resolve`]: the release files come as a folder (today's friend package, or the unzipped release) or as a zip
//!   (`--payload MW2inER-<ver>.zip`). A zip is unpacked once into
//!   `%TEMP%\MW2inER-payload\<sha256 prefix>` (zip-slip safe) and read from there; the install root is written only by
//!   the payload step, which copies the files with their `release.json` hashes checked.
//! - The embeddable CPython (user decision D4: bundled, never downloaded by the installer): the release ships either
//!   `runtime\python\` (unpacked; copied by the payload step with per-file hashes) or the official zip
//!   `python-3.14.7-embed-amd64.zip`, which [`install_python_zip`] unpacks into `<root>\runtime\python` only when its
//!   sha256 equals [`PYTHON_EMBED_SHA256`]. The converters run on it with the stdlib only, under `-I` (checked with
//!   work\installer\embed_import_check.py: every stdlib module and every script module imports; MODLOG installer P1
//!   pipeline).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::guard::Guard;
use crate::release::Release;
use crate::util;

/// The official CPython embeddable package the release bundles (python.org; PSF licence, `LICENSE.txt` inside).
pub const PYTHON_EMBED_NAME: &str = "python-3.14.7-embed-amd64.zip";
/// Where it came from (informational: the installer never downloads Python; the release step bundles this file).
pub const PYTHON_EMBED_URL: &str = "https://www.python.org/ftp/python/3.14.7/python-3.14.7-embed-amd64.zip";
/// sha256 of the zip as published on https://www.python.org/downloads/release/python-3147/ (checked 2026-10-22).
pub const PYTHON_EMBED_SHA256: &str = "d297e5ff019966817ad8502465176139f2d3d840fa4ed84b13bed399a6ab1f15";
/// sha256 of `python.exe` inside that zip.
pub const PYTHON_EXE_SHA256: &str = "4942b86a6597e5aee0128daa00050ed79bc21f6e709a78eb19cbfeb0c2f39ac9";
/// The installed runtime folder (root-relative).
pub const RUNTIME_DIR: &str = r"runtime\python";

/// The payload folder for `p`: the folder itself, or a zip unpacked into a temporary staging folder.
pub fn resolve(p: &Path) -> Result<PathBuf, String> {
    if p.is_dir() {
        return Ok(p.to_path_buf());
    }
    if !p.is_file() {
        return Err(format!("{} not found", p.display()));
    }
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        return Err(format!("{} is neither a folder nor a .zip of the release", p.display()));
    }
    let sha = util::sha256_file(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    let stage = std::env::temp_dir().join("MW2inER-payload").join(&sha[..16]);
    stage_zip(p, &stage)
}

/// Unpacks `zip` into `stage` (reused when a previous unpack completed) and returns the folder holding the release
/// (the stage, or its single top folder).
pub fn stage_zip(zip: &Path, stage: &Path) -> Result<PathBuf, String> {
    let label = zip.display().to_string();
    stage_with(stage, &label, |g| {
        crate::witchy::extract_zip(zip, stage, g)?;
        Ok(util::sha256_file(zip).unwrap_or_default())
    })
}

/// Runs `unpack` into a cleared `stage` unless a previous unpack completed (`.staged`), then finds the release in it.
fn stage_with(stage: &Path, label: &str, unpack: impl FnOnce(&Guard) -> Result<String, String>) -> Result<PathBuf, String> {
    let done = stage.join(".staged");
    if !done.is_file() {
        let g = Guard::default();
        g.remove_dir_all(stage).map_err(|e| format!("cannot clear {}: {e}", stage.display()))?;
        let sha = unpack(&g)?;
        g.write(&done, sha).map_err(|e| e.to_string())?;
    }
    if Release::present(stage) {
        return Ok(stage.to_path_buf());
    }
    let tops: Vec<PathBuf> = std::fs::read_dir(stage).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match tops.as_slice() {
        [one] if Release::present(one) => Ok(one.clone()),
        _ => Err(format!("{label} holds no release (manifest\\host_pins.json + host_manifest.json, or manifest\\release.json)")),
    }
}

// ---- the release payload embedded in the setup exe ------------------------------------------------------------
//
// `make_release.ps1` (tools\scripts\release_tool.py `pack`) appends the release zip to the built
// `MW2inER-setup.exe` and ends the file with a 64-byte trailer:
//   [16] OVERLAY_MAGIC  [8] u64 LE offset of the zip  [8] u64 LE length of the zip  [32] sha256 of the zip (raw)
// with offset + length == file size - 64. The PE image before it is the unchanged cargo build (Windows ignores data
// after the last section; the exe is unsigned, decision D8). One downloaded file is the whole release; the installed
// `<root>\MW2inER.exe` is written without the overlay (the payload files are in the root by then).

/// Marks the trailer of an embedded release payload.
pub const OVERLAY_MAGIC: &[u8; 16] = b"MW2inER-payload1";
/// Trailer size in bytes.
pub const OVERLAY_TRAILER: u64 = 64;

/// Where the release zip sits inside the setup exe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub offset: u64,
    pub len: u64,
    /// sha256 (lowercase hex) of the zip bytes.
    pub sha256: String,
}

/// Reads the trailer of `exe`. Ok(None) = no embedded payload (the plain cargo build, the installed copy);
/// Err = a trailer whose offsets do not fit the file (a damaged / truncated download).
pub fn read_overlay(exe: &Path) -> Result<Option<Overlay>, String> {
    let mut f = std::fs::File::open(exe).map_err(|e| format!("cannot open {}: {e}", exe.display()))?;
    let size = f.metadata().map_err(|e| e.to_string())?.len();
    if size < OVERLAY_TRAILER {
        return Ok(None);
    }
    let mut t = [0u8; OVERLAY_TRAILER as usize];
    f.seek(SeekFrom::Start(size - OVERLAY_TRAILER)).and_then(|_| f.read_exact(&mut t)).map_err(|e| e.to_string())?;
    if &t[..16] != OVERLAY_MAGIC {
        return Ok(None);
    }
    let offset = u64::from_le_bytes(t[16..24].try_into().expect("8 bytes"));
    let len = u64::from_le_bytes(t[24..32].try_into().expect("8 bytes"));
    if offset == 0 || offset.checked_add(len) != Some(size - OVERLAY_TRAILER) || len < 22 {
        return Err(format!("{} is damaged (the embedded release does not fit the file): download the setup again", exe.display()));
    }
    Ok(Some(Overlay { offset, len, sha256: util::hex(&t[32..64]) }))
}

/// The length of the program image in front of an overlay (the whole file when there is none).
pub fn image_len(exe: &Path) -> Result<u64, String> {
    match read_overlay(exe)? {
        Some(o) => Ok(o.offset),
        None => std::fs::metadata(exe).map(|m| m.len()).map_err(|e| format!("{}: {e}", exe.display())),
    }
}

/// Copies the program without its embedded payload (the installed `<root>\MW2inER.exe`).
pub fn copy_without_overlay(from: &Path, to: &Path, guard: &Guard) -> Result<(), String> {
    match read_overlay(from)? {
        None => guard.copy(from, to).map(|_| ()).map_err(|e| e.to_string()),
        Some(o) => {
            let mut buf = Vec::with_capacity(o.offset as usize);
            std::fs::File::open(from).and_then(|f| f.take(o.offset).read_to_end(&mut buf)).map_err(|e| e.to_string())?;
            guard.write(to, &buf).map_err(|e| e.to_string())
        }
    }
}

/// A window `[start, start + len)` of a seekable reader (the zip inside the exe), itself seekable from 0.
pub struct Region<R> {
    inner: R,
    start: u64,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> Region<R> {
    pub fn new(mut inner: R, start: u64, len: u64) -> std::io::Result<Region<R>> {
        inner.seek(SeekFrom::Start(start))?;
        Ok(Region { inner, start, len, pos: 0 })
    }
}

impl<R: Read + Seek> Read for Region<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        if left == 0 {
            return Ok(0);
        }
        let n = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let got = self.inner.read(&mut buf[..n])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl<R: Read + Seek> Seek for Region<R> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        }
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start of the region"))?;
        self.inner.seek(SeekFrom::Start(self.start + target))?;
        self.pos = target;
        Ok(target)
    }
}

/// The release embedded in `exe`, unpacked once into `%TEMP%\MW2inER-payload\<sha256 prefix>` (zip-slip safe, the
/// zip's sha256 checked against the trailer before anything is unpacked). Ok(None) when `exe` carries none.
pub fn embedded(exe: &Path) -> Result<Option<PathBuf>, String> {
    let Some(o) = read_overlay(exe)? else { return Ok(None) };
    let stage = std::env::temp_dir().join("MW2inER-payload").join(&o.sha256[..16]);
    stage_embedded(exe, &o, &stage).map(Some)
}

/// [`embedded`] into a given stage folder.
pub fn stage_embedded(exe: &Path, o: &Overlay, stage: &Path) -> Result<PathBuf, String> {
    let label = format!("the release inside {}", exe.display());
    let open = || -> Result<Region<std::fs::File>, String> {
        let f = std::fs::File::open(exe).map_err(|e| format!("cannot open {}: {e}", exe.display()))?;
        Region::new(f, o.offset, o.len).map_err(|e| e.to_string())
    };
    stage_with(stage, &label, |g| {
        let h = util::sha256_reader(open()?).map_err(|e| e.to_string())?;
        if !h.eq_ignore_ascii_case(&o.sha256) {
            return Err(format!(
                "{} is damaged (embedded release sha256 {} != {}): download the setup again",
                exe.display(),
                util::short(&h),
                util::short(&o.sha256)
            ));
        }
        crate::witchy::extract_zip_from(open()?, &label, stage, g)?;
        Ok(h)
    })
}

/// Appends `zip` with the trailer to a copy of `exe` (what `release_tool.py pack` does; used by the tests).
pub fn write_overlay(exe: &Path, zip: &Path, out: &Path) -> std::io::Result<Overlay> {
    let mut data = std::fs::read(exe)?;
    let z = std::fs::read(zip)?;
    let o = Overlay { offset: data.len() as u64, len: z.len() as u64, sha256: util::sha256_bytes(&z) };
    data.extend_from_slice(&z);
    data.extend_from_slice(OVERLAY_MAGIC);
    data.extend_from_slice(&o.offset.to_le_bytes());
    data.extend_from_slice(&o.len.to_le_bytes());
    for i in 0..32 {
        data.push(u8::from_str_radix(&o.sha256[2 * i..2 * i + 2], 16).expect("hex"));
    }
    std::fs::write(out, data)?;
    Ok(o)
}

/// The installed runtime's python.exe.
pub fn runtime_python(root: &Path) -> PathBuf {
    root.join(RUNTIME_DIR).join("python.exe")
}

/// A pinned embeddable zip: the given one, else `<payload>\runtime\<name>`, `<payload>\<name>`,
/// `<root>\downloads\<name>`. Returns (path, sha256 matches the pin); a matching file wins over a wrong one.
pub fn find_python_zip(over: Option<&Path>, root: &Path, payload: &Path) -> Option<(PathBuf, bool)> {
    let mut cands: Vec<PathBuf> = over.map(Path::to_path_buf).into_iter().collect();
    cands.push(payload.join("runtime").join(PYTHON_EMBED_NAME));
    cands.push(payload.join(PYTHON_EMBED_NAME));
    cands.push(root.join("downloads").join(PYTHON_EMBED_NAME));
    let mut found: Option<(PathBuf, bool)> = None;
    for c in cands.into_iter().filter(|c| c.is_file()) {
        let ok = util::sha256_file(&c).is_ok_and(|h| h.eq_ignore_ascii_case(PYTHON_EMBED_SHA256));
        if ok {
            return Some((c, true));
        }
        found.get_or_insert((c, false));
    }
    found
}

/// Unpacks the pinned embeddable zip into `<root>\runtime\python` (through `python.part`, swapped in on success).
/// Refuses a zip whose sha256 is not the pin (or `pin`, for tests) and a result without python.exe / `._pth`.
pub fn install_python_zip(zip: &Path, root: &Path, pin: &str, guard: &Guard) -> Result<PathBuf, String> {
    let h = util::sha256_file(zip).map_err(|e| format!("cannot read {}: {e}", zip.display()))?;
    if !h.eq_ignore_ascii_case(pin) {
        return Err(format!(
            "{} is not the official CPython embeddable package (sha256 {}, want {}): refused",
            zip.display(),
            util::short(&h),
            util::short(pin)
        ));
    }
    let dir = root.join(RUNTIME_DIR);
    let part = root.join(format!("{RUNTIME_DIR}.part"));
    guard.remove_dir_all(&part).map_err(|e| e.to_string())?;
    crate::witchy::extract_zip(zip, &part, guard)?;
    let has_pth = std::fs::read_dir(&part)
        .map_err(|e| e.to_string())?
        .flatten()
        .any(|e| e.file_name().to_string_lossy().to_lowercase().ends_with("._pth"));
    if !part.join("python.exe").is_file() || !has_pth {
        let _ = guard.remove_dir_all(&part);
        return Err(format!("{} has no python.exe / ._pth (not an embeddable package)", zip.display()));
    }
    guard.remove_dir_all(&dir).map_err(|e| e.to_string())?;
    guard.rename(&part, &dir).map_err(|e| e.to_string())?;
    Ok(dir.join("python.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::io::Write;

    pub fn make_zip(p: &Path, entries: &[(&str, &[u8])]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut w = zip::ZipWriter::new(std::fs::File::create(p).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        for (n, d) in entries {
            w.start_file(*n, o).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn payload_folder_or_zip() {
        let t = TempDir::new("payload");
        let pkg = crate::release::fixture::package(&t);
        assert_eq!(resolve(&pkg).unwrap(), pkg);
        assert!(resolve(&t.0.join("nope")).is_err());
        t.write("x.txt", "x");
        assert!(resolve(&t.0.join("x.txt")).unwrap_err().contains("neither"));
        // a zip with one top folder
        let z = t.0.join("rel.zip");
        let pins = std::fs::read(pkg.join(crate::release::HOST_PINS)).unwrap();
        let man = std::fs::read(pkg.join(crate::release::HOST_MANIFEST)).unwrap();
        make_zip(&z, &[("MW2inER/manifest/host_pins.json", &pins), ("MW2inER/manifest/host_manifest.json", &man), ("MW2inER/mw2er.toml", b"x"), ("../evil.txt", b"no")]);
        let stage = t.0.join("stage");
        let d = stage_zip(&z, &stage).unwrap();
        assert_eq!(d, stage.join("MW2inER"));
        assert!(Release::load(&d).is_ok());
        assert!(!t.0.join("evil.txt").exists());
        assert_eq!(stage_zip(&z, &stage).unwrap(), d, "reused");
        // a zip without a release
        let z2 = t.0.join("empty.zip");
        make_zip(&z2, &[("readme.txt", b"x")]);
        assert!(stage_zip(&z2, &t.0.join("stage2")).is_err());
    }

    #[test]
    fn embedded_payload_overlay() {
        let t = TempDir::new("overlay");
        let pkg = crate::release::fixture::package(&t);
        let pins = std::fs::read(pkg.join(crate::release::HOST_PINS)).unwrap();
        let man = std::fs::read(pkg.join(crate::release::HOST_MANIFEST)).unwrap();
        let z = t.0.join("payload.zip");
        make_zip(&z, &[("manifest/host_pins.json", &pins), ("manifest/host_manifest.json", &man), ("mw2er.toml", b"x"), ("../evil.txt", b"no")]);
        let image = t.write("plain.exe", b"MZ fake program image");
        // the plain build: no overlay, copied as it is
        assert_eq!(read_overlay(&image).unwrap(), None);
        assert_eq!(embedded(&image).unwrap(), None);
        assert_eq!(image_len(&image).unwrap(), 21);
        let setup = t.0.join("setup.exe");
        let o = write_overlay(&image, &z, &setup).unwrap();
        assert_eq!(read_overlay(&setup).unwrap(), Some(o.clone()));
        assert_eq!((o.offset, image_len(&setup).unwrap()), (21, 21));
        assert_eq!(o.sha256, util::sha256_file(&z).unwrap());
        // staged from the exe: the release is found, the zip-slip entry is not written, a second call reuses it
        let stage = t.0.join("stage");
        let d = stage_embedded(&setup, &o, &stage).unwrap();
        assert_eq!(d, stage);
        assert!(Release::load(&d).is_ok());
        assert_eq!(std::fs::read(d.join("mw2er.toml")).unwrap(), b"x");
        assert!(!t.0.join("evil.txt").exists());
        assert_eq!(stage_embedded(&setup, &o, &stage).unwrap(), d, "reused");
        // the installed copy has no overlay and is byte-identical to the program image
        let g = Guard::default();
        let inst = t.0.join(r"root\MW2inER.exe");
        copy_without_overlay(&setup, &inst, &g).unwrap();
        assert_eq!(std::fs::read(&inst).unwrap(), std::fs::read(&image).unwrap());
        assert_eq!(read_overlay(&inst).unwrap(), None);
        // one flipped byte inside the zip: refused before anything is unpacked
        let mut bad = std::fs::read(&setup).unwrap();
        bad[o.offset as usize + 40] ^= 0x55;
        let badp = t.0.join("bad.exe");
        std::fs::write(&badp, &bad).unwrap();
        let stage2 = t.0.join("stage2");
        let e = stage_embedded(&badp, &read_overlay(&badp).unwrap().unwrap(), &stage2).unwrap_err();
        assert!(e.contains("damaged"), "{e}");
        assert!(!stage2.join("mw2er.toml").exists() && !stage2.join(".staged").exists());
        // a truncated download: the trailer no longer fits
        let mut cut = std::fs::read(&setup).unwrap();
        cut.drain(5..10);
        let cutp = t.0.join("cut.exe");
        std::fs::write(&cutp, &cut).unwrap();
        assert!(read_overlay(&cutp).unwrap_err().contains("damaged"));
        // the region reader seeks inside the window only
        let mut r = Region::new(std::fs::File::open(&setup).unwrap(), o.offset, o.len).unwrap();
        assert_eq!(r.seek(SeekFrom::End(0)).unwrap(), o.len);
        assert!(r.seek(SeekFrom::Current(-(o.len as i64) - 1)).is_err());
        r.seek(SeekFrom::Start(0)).unwrap();
        let mut head = [0u8; 4];
        r.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"PK\x03\x04");
    }

    #[test]
    fn python_zip_is_pinned() {
        let t = TempDir::new("pyzip");
        let good = t.0.join(r"pkg\runtime").join(PYTHON_EMBED_NAME);
        make_zip(&good, &[("python.exe", b"py"), ("python314._pth", b"python314.zip\n.\n"), ("LICENSE.txt", b"PSF")]);
        let pin = util::sha256_file(&good).unwrap();
        let root = t.0.join("root");
        let g = Guard::default();
        let exe = install_python_zip(&good, &root, &pin, &g).unwrap();
        assert_eq!(exe, runtime_python(&root));
        assert!(root.join(r"runtime\python\LICENSE.txt").is_file());
        assert!(!root.join(r"runtime\python.part").exists());
        // the real pin refuses this fake, and nothing is changed
        let e = install_python_zip(&good, &root, PYTHON_EMBED_SHA256, &g).unwrap_err();
        assert!(e.contains("refused"), "{e}");
        assert!(runtime_python(&root).is_file());
        // a pinned file that is no embeddable package
        let bad = t.0.join("bad.zip");
        make_zip(&bad, &[("readme.txt", b"x")]);
        let pin2 = util::sha256_file(&bad).unwrap();
        assert!(install_python_zip(&bad, &t.0.join("root2"), &pin2, &g).unwrap_err().contains("no python.exe"));
        assert!(!t.0.join(r"root2\runtime\python").exists());
        // search: the fake does not match the real pin
        let f = find_python_zip(None, &root, &t.0.join("pkg")).unwrap();
        assert_eq!(f, (good.clone(), false));
        assert_eq!(find_python_zip(None, &root, &t.0.join("none")), None);
        // a guarded root is refused before anything is written
        let g2 = Guard::new(Some(&t.0.join("root3")), None, &[], None, None);
        assert!(install_python_zip(&good, &t.0.join(r"root3\x"), &pin, &g2).is_err());
        assert!(!t.0.join(r"root3\x").exists());
    }
}
