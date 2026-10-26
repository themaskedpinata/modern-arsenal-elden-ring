//! Real-data checks (ignored by default: they read the game-derived regulation files of this PC).
//! `cargo test -p regbuild --test real_data -- --ignored`
//! Inputs: <root>\work\regulation\vanilla\regulation.bin (the staged read-only copy of the game file),
//! <root>\build\package\regulation.bin (the WitchyBND build), mw2er.toml + converted\ (the plan inputs).

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(rel: &str) -> Option<Vec<u8>> {
    let p = root().join(rel);
    match std::fs::read(&p) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("skipped: {} not present", p.display());
            None
        }
    }
}

#[test]
#[ignore]
fn vanilla_no_edit_round_trip_is_byte_identical() {
    let Some(raw) = read(r"work\regulation\vanilla\regulation.bin") else { return };
    assert_eq!(regbuild::sha256_hex(&raw), regbuild::VANILLA_SHA256);
    let u = regbuild::unpack(&raw).unwrap();
    let b = u.bnd4().unwrap();
    assert_eq!((b.version.as_str(), b.entries.len()), (regbuild::REGULATION_VERSION, 194));
    let files: Vec<&[u8]> = (0..b.entries.len()).map(|k| b.data(k)).collect();
    assert_eq!(b.write(&files).unwrap(), u.bnd);
    let empty = regbuild::plan::Plan { bullets: vec![], atks: vec![], speffects: vec![], ..plan() };
    let edited = regbuild::apply(&b, &empty).unwrap();
    assert_eq!(b.write(&edited.iter().map(|f| f.as_slice()).collect::<Vec<_>>()).unwrap(), u.bnd, "empty plan == vanilla");
}

fn plan() -> regbuild::plan::Plan {
    regbuild::plan::make_plan(&regbuild::plan::load_inputs(&root()).unwrap()).unwrap()
}

#[test]
#[ignore]
fn build_matches_the_witchybnd_bnd4_and_verifies() {
    let (Some(raw), Some(wit)) = (read(r"work\regulation\vanilla\regulation.bin"), read(r"build\package\regulation.bin")) else { return };
    let u = regbuild::unpack(&raw).unwrap();
    let vb = u.bnd4().unwrap();
    let p = plan();
    let files = regbuild::apply(&vb, &p).unwrap();
    let bnd = vb.write(&files.iter().map(|f| f.as_slice()).collect::<Vec<_>>()).unwrap();
    let w = regbuild::unpack(&wit).unwrap();
    assert_eq!(bnd.len(), w.bnd.len());
    assert!(bnd == w.bnd, "the regbuild BND4 is byte-identical to WitchyBND's (when build\\package is the WitchyBND build of the same inputs)");
    let bb = regbuild::bnd4::Bnd4::parse(&bnd).unwrap();
    let (problems, report) = regbuild::verify::verify(&vb, &regbuild::verify::Files::new(&bb), &p);
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!((report.untouched_identical, report.rows_checked), (191, 659));
}

#[test]
#[ignore]
fn self_test_catches_every_mutation() {
    let Some(raw) = read(r"work\regulation\vanilla\regulation.bin") else { return };
    let u = regbuild::unpack(&raw).unwrap();
    let vb = u.bnd4().unwrap();
    let inputs = regbuild::plan::load_inputs(&root()).unwrap();
    let p = regbuild::plan::make_plan(&inputs).unwrap();
    let files = regbuild::apply(&vb, &p).unwrap();
    let bnd = vb.write(&files.iter().map(|f| f.as_slice()).collect::<Vec<_>>()).unwrap();
    let bb = regbuild::bnd4::Bnd4::parse(&bnd).unwrap();
    let res = regbuild::selftest::SelfTest { vanilla: &vb, built: &bb, plan: &p, inputs: &inputs }.run().unwrap();
    let missed: Vec<_> = res.iter().filter(|r| !r.1).map(|r| r.0.clone()).collect();
    assert!(missed.is_empty(), "{missed:#?}");
    assert_eq!(res.len(), 46);
}
