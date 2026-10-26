//! Cross-check of the real converted data (check_m2.ps1 check 9). Ignored by default because it
//! needs the user's local dumps; run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data -- --include-ignored`.
//!
//! The Rust WEAPONFILE parser re-reads every dump and must agree with `converted\weapons.json`
//! (written by the Python converter) on every value the JSON carries. Missing inputs print
//! `SKIP:` and return, so the caller can report SKIP instead of PASS.

use std::path::{Path, PathBuf};

use mw2er_core::falloff::band_damages;
use mw2er_core::rows::RowsFile;
use mw2er_core::weaponfile::{self, EXPECTED_FIELD_COUNT};
use mw2er_core::weapons::{FireMode, WeaponsFile};

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

#[test]
#[ignore = "needs the local MW2 dumps and converted/weapons.json"]
fn real_weapons_json_matches_dumps() {
    let root = root();
    let Some(json) = read(&root.join("converted").join("weapons.json")) else {
        println!("SKIP: converted/weapons.json missing under {}", root.display());
        return;
    };
    let value: serde_json::Value = serde_json::from_str(&json).expect("weapons.json is JSON");
    let file = WeaponsFile::parse(&json).expect("weapons.json validates");
    let sources = value["source"].as_object().expect("source map");
    let mut compared = 0usize;
    for (name, entry) in value["weapons"].as_object().expect("weapons map") {
        let rel = sources[name]["file"].as_str().expect("source file");
        let dump_path = root.join(rel);
        let Some(text) = read(&dump_path) else {
            println!("SKIP: dump {} missing", dump_path.display());
            return;
        };
        let wf = weaponfile::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(wf.len(), EXPECTED_FIELD_COUNT, "{name}: field count");
        assert_eq!(sources[name]["fields"].as_u64(), Some(wf.len() as u64), "{name}: recorded count");
        for (key, jv) in entry["mw2"].as_object().expect("mw2 object") {
            let raw = wf.get(key).unwrap_or_else(|| panic!("{name}: {key} not in dump"));
            match jv {
                serde_json::Value::Number(n) => {
                    let want = n.as_f64().unwrap();
                    let got: f64 = raw.trim().parse().unwrap_or_else(|_| panic!("{name}.{key}={raw:?}"));
                    assert_eq!(got, want, "{name}.{key}");
                }
                serde_json::Value::String(s) => assert_eq!(raw, s, "{name}.{key}"),
                other => panic!("{name}.{key}: unexpected JSON {other}"),
            }
            compared += 1;
        }
        // Every metric value is exactly the inch value x 0.0254 (validated by spec()).
        file.spec(name).unwrap_or_else(|e| panic!("{e}"));
    }
    assert!(compared > 50, "compared {compared} values");

    // Known answers (MODLOG M1 oracle).
    let u = file.spec("ump45_mp").expect("ump45_mp converted");
    assert_eq!((u.damage, u.min_damage), (40, 35));
    assert!((u.max_damage_range_m - 15.24).abs() < 1e-9 && (u.min_damage_range_m - 21.59).abs() < 1e-9);
    assert_eq!((u.fire_time_s, u.clip_size, u.max_ammo), (0.09, 32, 192));
    assert_eq!((u.reload_s, u.reload_empty_s), (2.5, 3.03));
    assert_eq!((u.spread_min_deg, u.spread_max_deg), (2.0, 5.0));
    assert_eq!(u.fire_mode, FireMode::FullAuto);
    if let Ok(a) = file.spec("masada_mp") {
        assert_eq!((a.damage, a.min_damage, a.fire_time_s, a.clip_size), (30, 20, 0.077, 30));
    }
    if let Ok(c) = file.spec("cheytac_mp") {
        assert_eq!((c.damage, c.clip_size, c.fire_mode), (70, 5, FireMode::SingleShot));
    }
    println!("PASS: {compared} weapons.json values equal the Rust parse of the dumps");
}

#[test]
#[ignore = "needs build/rows.json from build_regulation.py"]
fn real_rows_json_matches_band_rule() {
    let root = root();
    let (Some(rows), Some(weapons)) = (
        read(&root.join("build").join("rows.json")),
        read(&root.join("converted").join("weapons.json")),
    ) else {
        println!("SKIP: build/rows.json or converted/weapons.json missing");
        return;
    };
    let rows = RowsFile::parse(&rows).expect("rows.json validates");
    let weapons = WeaponsFile::parse(&weapons).expect("weapons.json validates");
    for (name, w) in &rows.weapons {
        let spec = weapons.spec(name).expect("weapon in weapons.json");
        assert_eq!(
            w.band_damages(),
            band_damages(spec.damage, spec.min_damage),
            "{name}: Python band rule == Rust band rule"
        );
    }
    println!("PASS: rows.json bands follow the Rust band rule");
}
