//! M7 real-data cross-checks (check_m7.ps1 check 7): `converted\nuke\nuke.json` loads through the model the DLL uses
//! and gives MW2's timeline, and `build\rows.json`'s nuke block names the rows the regulation added; the post pass on
//! the real vision numbers meets the known answers of docs/M7_DESIGN.md 6.3.
//! Ignored by default (it needs the local converted data); run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data_m7 -- --include-ignored`. A missing input
//! prints `SKIP:` and returns (check_m7 counts that as FAIL).

use std::path::{Path, PathBuf};

use mw2er_core::config::Config;
use mw2er_core::nuke_data::NukeFile;
use mw2er_core::nuke_fx::{uniform, vis_at};
use mw2er_core::nuke_logic::{NukeSeq, Plan, PostState};
use mw2er_core::rows::RowsFile;

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

#[test]
#[ignore = "needs converted/nuke (tools/scripts/convert_nuke.py)"]
fn real_nuke_data_timeline_and_visions() {
    let p = root().join("converted").join("nuke").join("nuke.json");
    let Ok(text) = std::fs::read_to_string(&p) else {
        println!("SKIP: {} missing", p.display());
        return;
    };
    let f = NukeFile::parse(&text).unwrap();
    assert_eq!(f.cost, 25);
    let cfg = Config::parse(&std::fs::read_to_string(root().join("mw2er.toml")).unwrap()).unwrap();
    let plan = Plan::new(&f.timeline, &cfg.nuke);
    assert_eq!(plan.timer_s, 10.0);
    assert!((plan.incoming_at - 6.7).abs() < 1e-9);
    assert_eq!((plan.vision_at, plan.kill_at), (10.25, 11.5));
    assert_eq!((f.timeline.quake_delay_s, plan.quake_at), (1.5, 11.5), "MW2 nukeEarthquake at timer + 1.5");
    let s = NukeSeq::new(0.0, plan, false);
    assert!(s.countdown_left(0.6).is_some_and(|l| (l - 9.4).abs() < 1e-9));
    // the post-pass known answers on the real visions
    let at = |amount: f32, blend: f32, flash: f32| {
        let post = PostState { flash, amount, blend };
        (vis_at(&f, &post), post)
    };
    let (v, p0) = at(0.0, 0.0, 1.0);
    assert_eq!(uniform([0.0; 3], &v, &p0), [1.0; 3], "black + flash 1 -> white");
    let (v, p1) = at(1.0, 0.0, 0.0);
    let o = uniform([0.5; 3], &v, &p1);
    assert!(o[0] >= o[1] && o[1] >= o[2] && o[0] >= 0.95 && o[2] <= o[0] - 0.1, "mpnuke on grey {o:?}");
    let (v, p2) = at(1.0, 1.0, 0.0);
    let o = uniform([0.5; 3], &v, &p2);
    assert!(o[0] > o[1] && o[1] > o[2], "aftermath on grey {o:?}");
    // every sound key the DLL plays resolves to files on disk
    for (k, snd) in &f.sounds {
        for file in &snd.files {
            assert!(root().join("converted").join("nuke").join(file.replace('/', "\\")).is_file(), "{k}: {file}");
        }
    }
    println!(
        "PASS: nuke.json cost {} timer {} s incoming T-{} vision T+{}/{} s death T+{} aftermath {} s, {} aliases, mpnuke white-out + aftermath tint known answers",
        f.cost, f.timeline.timer_s, f.timeline.incoming_lead_s, f.timeline.vision_delay_s, f.timeline.vision_blend_s, f.timeline.death_delay_s,
        f.timeline.aftermath_blend_s, f.sounds.len()
    );
}

#[test]
#[ignore = "needs build/rows.json (tools/scripts/build_regulation.py)"]
fn real_rows_nuke_block() {
    let p = root().join("build").join("rows.json");
    let Ok(text) = std::fs::read_to_string(&p) else {
        println!("SKIP: {} missing", p.display());
        return;
    };
    let r = RowsFile::parse(&text).unwrap();
    let n = r.nuke.expect("rows.json has a nuke block");
    let cfg = Config::default();
    assert_eq!(n.lethal_speffect_id as i64, cfg.regulation.nuke.lethal_speffect_id);
    assert_eq!(n.lethal_bullet_id as i64, cfg.regulation.nuke.lethal_bullet_id);
    assert_eq!(n.horizon_bullet_id as i64, cfg.regulation.nuke.horizon_bullet_id);
    assert_eq!(n.horizon_sfx as i64, cfg.regulation.nuke.horizon_sfx);
    let ac = r.ac130.expect("ac130 block");
    assert_ne!(n.lethal_speffect_id, ac.protect_speffect_id);
    println!("PASS: rows.json nuke block speffect {} bullet {} horizon {} (sfx {})", n.lethal_speffect_id, n.lethal_bullet_id, n.horizon_bullet_id, n.horizon_sfx);
}
