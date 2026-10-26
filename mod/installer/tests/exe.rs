//! The built `MW2inER-setup.exe`: its Windows resources (icon group, version block, asInvoker manifest from build.rs)
//! and the console front end without a window (`--cli`: output on stdout, exit codes, the shell waits).
//! Runs only this program (never a game, Steam or me3) and reads no game folder.

use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_MW2inER-setup");

#[test]
fn resources_icon_version_manifest() {
    let v = installer::win::pe_version(std::path::Path::new(EXE)).expect("a version block");
    let want = env!("CARGO_PKG_VERSION");
    assert_eq!(v.product_version.as_deref(), Some(want));
    assert_eq!(v.file_version.as_deref(), Some(format!("{want}.0").as_str()));
    let map = pelite::FileMap::open(EXE).unwrap();
    use pelite::pe64::Pe;
    let pe = pelite::pe64::PeFile::from_bytes(map.as_ref()).unwrap();
    let res = pe.resources().unwrap();
    let m = res.manifest().expect("an application manifest");
    assert!(m.contains(r#"level="asInvoker""#), "no admin rights / no UAC prompt");
    assert!(!m.contains("requireAdministrator"));
    let icons: Vec<_> = res.icons().filter_map(Result::ok).collect();
    assert_eq!(icons.len(), 1, "one icon group");
    let g = &icons[0].1;
    let sizes: Vec<u8> = g.entries().iter().map(|e| e.bWidth).collect();
    assert!(sizes.contains(&16) && sizes.contains(&32) && sizes.contains(&48), "{sizes:?}");
    for e in g.entries() {
        let img = g.image(e.nId).expect("each icon image is present");
        assert_eq!(&img[..4], &40u32.to_le_bytes(), "32-bit DIB with a BITMAPINFOHEADER");
    }
}

#[test]
fn cli_runs_without_a_window() {
    let out = Command::new(EXE).args(["--cli", "help"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("usage:") && s.contains("--gui"), "{s}");
    // a usage error: exit 2 on the console, no window
    let out = Command::new(EXE).args(["--cli", "frobnicate"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("unknown command"));
    // play from a terminal (shared console, no --cli): still the console front end; no install = exit 1 before
    // anything is started
    let t = std::env::temp_dir().join(format!("mw2er-installer-tests/exe-{}", std::process::id()));
    let out = Command::new(EXE).args(["play", "coop", "--dry-run", "--root"]).arg(&t).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no install"), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(!t.exists());
}
