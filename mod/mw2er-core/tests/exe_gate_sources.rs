//! Review 2026-10-20 (port 2.7.2.0): source-level guards for the version gate (offline, no game files).
//! 1. mw2er's mirrored fromsoftware-rs detection (`mw2er/src/version_gate.rs`, `ErWw`) has exactly the WW arms of the
//!    vendored crate's `ERGameVersion` (`crates/eldenring/src/rva.rs`), each naming the same table, and every
//!    `ExeBuild` is one of them. A crate change that adds, removes or renames an arm fails here.
//! 2. `exe_build::UNUSED_CRATE_CODE` is honest: no mw2er source names a fromsoftware-rs API that reaches one of those
//!    RVAs (a code difference there is only a WARN at boot).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mw2er_core::exe_build::{ExeBuild, UNUSED_CRATE_CODE};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `(version, table)` of every `(LANG_ID_EN, "x.y.z.w") => Some(<prefix>Name...)` arm.
fn en_arms(src: &str, prefix: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("(LANG_ID_EN, \"") else { continue };
        let Some((ver, tail)) = rest.split_once('"') else { continue };
        let Some(after) = tail.split_once(prefix).map(|x| x.1) else { panic!("unexpected arm: {l}") };
        let name: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        out.insert((ver.to_string(), name));
    }
    out
}

#[test]
fn version_gate_mirrors_crate_detection() {
    let crate_rva = read(&root().join(r"vendor/fromsoftware-rs/crates/eldenring/src/rva.rs"));
    let gate = read(&root().join(r"mod/mw2er/src/version_gate.rs"));
    let crate_arms = en_arms(&crate_rva, "Some(Self::");
    let gate_arms = en_arms(&gate, "Some(ErWw(ExeBuild::");
    assert!(!crate_arms.is_empty(), "no LANG_ID_EN arm parsed from rva.rs");
    assert_eq!(gate_arms, crate_arms, "version_gate's ErWw arms != the crate's ERGameVersion WW arms");
    // every supported build is an arm, with the version and table name the gate logs
    let builds: BTreeSet<(String, String)> = ExeBuild::ALL
        .iter()
        .map(|b| {
            let (a, c, d, e) = b.version();
            (format!("{a}.{c}.{d}.{e}"), b.table_name().to_string())
        })
        .collect();
    assert_eq!(builds, crate_arms, "ExeBuild::ALL != the crate's WW tables");
    // each WW table maps to its own RVA module (Ww2710 -> rva_ww, Ww2720 -> rva_ww2720)
    for (want, module) in [("Ww2710", "rva_ww::RVAS"), ("Ww2720", "rva_ww2720::RVAS")] {
        assert!(crate_rva.contains(&format!("Self::{want} => {module}")), "rva.rs: {want} does not select {module}");
    }
    println!("PASS version gate mirrors the crate's WW detection: {crate_arms:?}");
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn unused_crate_code_is_unused() {
    // fromsoftware-rs API (vendored @59fbd3b + patches) -> the RVA it calls
    let apis: [(&str, &[&str]); 11] = [
        ("cs_action_button_man_execute_action_button", &["present_action_button", "CSActionButtonManImp"]),
        ("cs_ez_draw_draw_capsule", &["draw_capsule", "CSEzDraw", "RendMan"]),
        ("cs_ez_draw_draw_dodecadron", &["draw_dodecadron"]),
        ("cs_ez_draw_draw_line", &["draw_line"]),
        ("cs_ez_draw_draw_sphere", &["draw_sphere"]),
        ("cs_ez_draw_draw_triangle", &["draw_triangle"]),
        ("cs_ez_draw_draw_wedge", &["draw_wedge"]),
        ("cs_menu_man_imp_display_status_message", &["display_status_message"]),
        ("initialize_spawn_geometry_request", &["spawn_geometry", "CSWorldGeomMan", "GeometrySpawn"]),
        ("obtain_dl_user_input_manager", &["DLUserInputManager", "user_input_manager"]),
        ("spawn_geometry", &["spawn_geometry"]),
    ];
    let listed: BTreeSet<&str> = apis.iter().map(|a| a.0).collect();
    assert_eq!(listed, UNUSED_CRATE_CODE.iter().copied().collect(), "every unused RVA has its crate API names here");
    // the crate really reaches each RVA through those names (so the guard below looks for the right words)
    let vendor = root().join(r"vendor/fromsoftware-rs/crates/eldenring/src");
    let mut vfiles = Vec::new();
    rs_files(&vendor, &mut vfiles);
    let vendor_src: String = vfiles
        .iter()
        .filter(|p| !p.to_string_lossy().contains("rva_"))
        .map(|p| read(p))
        .collect::<Vec<_>>()
        .join("\n");
    for (rva, _) in &apis {
        assert!(vendor_src.contains(&format!("get().{rva}")), "crate no longer uses rva {rva}");
    }
    // mw2er (the only crate depending on eldenring) names none of them
    let mut files = Vec::new();
    rs_files(&root().join(r"mod/mw2er/src"), &mut files);
    assert!(files.len() > 20, "mw2er sources not found");
    let mut hits = Vec::new();
    for f in &files {
        let src = read(f);
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for (rva, names) in &apis {
                for n in *names {
                    if code.contains(n) {
                        hits.push(format!("{}:{}: {n} ({rva})", f.display(), i + 1));
                    }
                }
            }
        }
    }
    assert!(hits.is_empty(), "mw2er uses a crate API whose RVA is treated as unused: {hits:#?}");
    println!("PASS {} unused crate code RVAs: no mw2er source reaches them ({} files)", apis.len(), files.len());
}
