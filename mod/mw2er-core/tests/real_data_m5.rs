//! M5 real-data cross-check (check_m5.ps1 check 3): the converter's `converted\movement.json` loads through the same
//! model the DLL uses and drives the controller to the hand-computed MW2 numbers. Ignored by default (it needs the
//! local converted data); run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data_m5 -- --include-ignored`. A missing input
//! prints `SKIP:` and returns (check_m5 counts that as FAIL).

use std::path::{Path, PathBuf};

use mw2er_core::movement_data::{MoveData, test_data};
use mw2er_core::pmove::{Cmd, ScaleIn, wishspeed};

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

#[test]
#[ignore = "needs converted/movement.json (tools/scripts/convert_movement.py)"]
fn real_movement_json_loads_and_matches_design_numbers() {
    let p = root().join("converted").join("movement.json");
    let Ok(text) = std::fs::read_to_string(&p) else {
        println!("SKIP: {} missing", p.display());
        return;
    };
    let d = MoveData::from_json(&text).expect("movement.json validates");
    let t = test_data();
    // every typed constant equals the design's table 2.1 values (the test data set)
    assert_eq!(format!("{:?}", d.mw2), format!("{:?}", t.mw2), "constants differ from the design table");
    assert_eq!(d.weapon, t.weapon, "UMP45 fields differ");
    assert_eq!(d.curves, t.curves, "eye curves differ");
    let w = Cmd::from_keys(true, false, false, false);
    assert_eq!(wishspeed(&d.mw2, w, &ScaleIn::plain(&d.weapon)), 190.0);
    println!(
        "PASS: movement.json loads ({} labels, exe sha {}) and equals the design's MW2 numbers: {}",
        d.labels.len(),
        &d.exe_sha256[..12],
        d.summary()
    );
}
