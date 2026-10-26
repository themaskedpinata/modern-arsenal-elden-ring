//! WitchyBND 3.0.1.0 (P1 only; replaced by the Rust regulation builder in P2): unpacks the pinned zip into
//! `<root>\tools\WitchyBND\app` exactly as setup.ps1 step 3 does: one top folder flattened, `WitchyBND.exe`
//! checked against its pin, the bundled `Assets\Paramdex.zip` unpacked up front (WitchyBND would do it on its first
//! start and undo the upgrader bump mid-build), `appsettings.override.json` (Offline, Flexible=false) and the
//! Paramdex upgrader `version.txt` = the release's `paramdex_version`.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use crate::detect::WITCHY_APP;
use crate::guard::Guard;
use crate::release::Pins;
use crate::util;

pub const OVERRIDE_JSON: &str = "{\n  \"Offline\": true,\n  \"PauseOnError\": false,\n  \"EndDelay\": 0,\n  \"Flexible\": false\n}\n";

/// Extracts every entry of a zip under `dest` (entries escaping `dest` are skipped). Returns the file count.
pub fn extract_zip(zip_path: &Path, dest: &Path, guard: &Guard) -> Result<usize, String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("cannot open {}: {e}", zip_path.display()))?;
    extract_zip_from(f, &zip_path.display().to_string(), dest, guard)
}

/// [`extract_zip`] over any seekable reader (the release zip appended to the setup exe, `payload::Region`);
/// `label` names it in messages.
pub fn extract_zip_from<R: Read + Seek>(r: R, label: &str, dest: &Path, guard: &Guard) -> Result<usize, String> {
    let mut a = zip::ZipArchive::new(r).map_err(|e| format!("cannot read {label}: {e}"))?;
    let mut n = 0;
    for i in 0..a.len() {
        let mut e = a.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else { continue };
        let to = dest.join(rel);
        if e.is_dir() {
            guard.create_dir_all(&to).map_err(|e| e.to_string())?;
            continue;
        }
        let mut buf = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        guard.write(&to, &buf).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

/// Unpacks the zip into `<root>\tools\WitchyBND\app` (cleared first) and applies the settings. Returns the exe.
pub fn prepare(zip_path: &Path, root: &Path, pins: &Pins, guard: &Guard) -> Result<PathBuf, String> {
    let zsha = util::sha256_file(zip_path).map_err(|e| format!("cannot read {}: {e}", zip_path.display()))?;
    if !zsha.eq_ignore_ascii_case(&pins.witchybnd_zip_sha256) {
        return Err(format!(
            "{} is not the official WitchyBND 3.0.1.0 zip (sha256 {}, want {})",
            zip_path.display(),
            util::short(&zsha),
            util::short(&pins.witchybnd_zip_sha256)
        ));
    }
    let app = root.join(WITCHY_APP);
    guard.remove_dir_all(&app).map_err(|e| e.to_string())?;
    guard.create_dir_all(&app).map_err(|e| e.to_string())?;
    extract_zip(zip_path, &app, guard)?;
    let mut exe = app.join("WitchyBND.exe");
    if !exe.is_file() {
        // one top folder: move its content up
        let inner = std::fs::read_dir(&app)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("WitchyBND.exe").is_file())
            .ok_or("the zip has no WitchyBND.exe")?;
        for e in std::fs::read_dir(&inner).map_err(|e| e.to_string())?.flatten() {
            guard.rename(&e.path(), &app.join(e.file_name())).map_err(|e| e.to_string())?;
        }
        guard.remove_dir_all(&inner).map_err(|e| e.to_string())?;
        exe = app.join("WitchyBND.exe");
    }
    let esha = util::sha256_file(&exe).map_err(|e| e.to_string())?;
    if !esha.eq_ignore_ascii_case(&pins.witchybnd_exe_sha256) {
        return Err("WitchyBND.exe after unzipping is not the expected 3.0.1.0 build".into());
    }
    let pdz = app.join(r"Assets\Paramdex.zip");
    if pdz.is_file() {
        extract_zip(&pdz, &app.join(r"Assets\Paramdex"), guard)?;
        guard.remove_file(&pdz).map_err(|e| e.to_string())?;
    }
    guard.write(&app.join("appsettings.override.json"), OVERRIDE_JSON).map_err(|e| e.to_string())?;
    guard
        .write(&app.join(r"Assets\Paramdex\ER\Upgrader\version.txt"), pins.paramdex_version.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::io::Write;

    fn make_zip(p: &Path, entries: &[(&str, &[u8])]) {
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
    fn prepares_like_setup_ps1() {
        let t = TempDir::new("witchy");
        let pd = t.0.join("pd.zip");
        make_zip(&pd, &[("ER/Defs/BulletParam.xml", b"<x/>"), ("ER/Upgrader/version.txt", b"old")]);
        let pd_bytes = std::fs::read(&pd).unwrap();
        let z = t.0.join("WitchyBND-3.0.1.0-win-x64.zip");
        make_zip(&z, &[("WitchyBND/WitchyBND.exe", b"witchy exe"), ("WitchyBND/Assets/Paramdex.zip", &pd_bytes), ("../escape.txt", b"no")]);
        let pins = Pins {
            witchybnd_zip_sha256: util::sha256_file(&z).unwrap(),
            witchybnd_exe_sha256: util::sha256_bytes(b"witchy exe"),
            paramdex_version: "1_17_3_0034L".into(),
            ..Default::default()
        };
        let root = t.0.join("root");
        let exe = prepare(&z, &root, &pins, &Guard::default()).unwrap();
        let app = root.join(WITCHY_APP);
        assert_eq!(exe, app.join("WitchyBND.exe"));
        assert!(!app.join("WitchyBND").exists(), "top folder flattened");
        assert!(!app.join(r"Assets\Paramdex.zip").exists());
        assert!(app.join(r"Assets\Paramdex\ER\Defs\BulletParam.xml").is_file());
        assert_eq!(std::fs::read_to_string(app.join(r"Assets\Paramdex\ER\Upgrader\version.txt")).unwrap(), "1_17_3_0034L");
        assert_eq!(std::fs::read_to_string(app.join("appsettings.override.json")).unwrap(), OVERRIDE_JSON);
        assert!(!t.0.join("escape.txt").exists() && !root.join(r"tools\WitchyBND\escape.txt").exists());
        // wrong zip / exe pins are refused
        let mut p2 = pins.clone();
        p2.witchybnd_zip_sha256 = util::sha256_bytes(b"other");
        assert!(prepare(&z, &root, &p2, &Guard::default()).is_err());
        let mut p3 = pins.clone();
        p3.witchybnd_exe_sha256 = util::sha256_bytes(b"other");
        assert!(prepare(&z, &root, &p3, &Guard::default()).unwrap_err().contains("not the expected"));
    }
}
