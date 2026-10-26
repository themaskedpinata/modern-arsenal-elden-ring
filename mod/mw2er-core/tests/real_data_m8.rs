//! M8 real-data cross-check (check_m8.ps1 check 4/6): the converter's mantle block in `converted\movement.json` loads
//! through the model the DLL uses, equals the design's SP numbers (the test data set), and a mantle onto a 1.0 m box
//! with it takes the SP time. Ignored by default (it needs the local converted data); run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data_m8 -- --include-ignored`. A missing input
//! prints `SKIP:` and returns (check_m8 counts that as FAIL).

use std::path::{Path, PathBuf};

use mw2er_core::move_collide::synth::{Convex, World};
use mw2er_core::move_sim::{Input, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::{MoveData, test_data};
use mw2er_core::vec3::V3;

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

#[test]
#[ignore = "needs converted/movement.json (tools/scripts/convert_movement.py)"]
fn real_mantle_block_loads_and_matches_design() {
    let p = root().join("converted").join("movement.json");
    let Ok(text) = std::fs::read_to_string(&p) else {
        println!("SKIP: {} missing", p.display());
        return;
    };
    let d = MoveData::from_json(&text).expect("movement.json validates");
    let m = d.mantle.as_ref().expect("the mantle block");
    let t = test_data();
    let tm = t.mantle.as_ref().unwrap();
    assert!(m.enable);
    assert_eq!(
        (m.check_range, m.check_angle_deg, m.max_rise, m.forward, m.rise_min, m.player_radius, m.clear_height, m.over_forward, m.over_drop),
        (tm.check_range, tm.check_angle_deg, tm.max_rise, tm.forward, tm.rise_min, tm.player_radius, tm.clear_height, tm.over_forward, tm.over_drop)
    );
    assert!((m.check_radius - 0.1).abs() < 1e-6 && m.max_vert_end_vel == 0.0 && m.yawcap_deg == 60.0);
    assert!((m.rise_s - 1.0 / 3.0).abs() < 1e-5 && (m.cross_s - 1.0 / 6.0).abs() < 1e-5 && (m.up_s - 0.5).abs() < 1e-5);
    assert!((m.over_s - 2.0 / 3.0).abs() < 1e-5);
    let heights: Vec<f32> = m.rows.iter().map(|r| r.height).collect();
    assert_eq!(heights, vec![57.0, 51.0, 45.0, 39.0, 33.0, 27.0, 21.0]);
    let drops: Vec<bool> = m.rows.iter().map(|r| r.drop_weapon).collect();
    assert_eq!(drops, vec![true, true, true, true, true, false, false]);
    // the dumped curves equal the design's samples (the test set carries up_57 / up_39 / up_21 / over_high)
    for (a, b) in m.rows[0].rise.iter().zip(tm.rows[0].rise.iter()) {
        assert!((a - b).abs() < 0.002, "up_57 rise {a} vs {b}");
    }
    for (a, b) in m.over_x[0].iter().zip(tm.over_x[0].iter()) {
        assert!((a - b).abs() < 0.002, "over_high x {a} vs {b}");
    }
    assert_eq!(m.quick_times("ump45_mp"), (0.25, 0.55));
    assert_eq!(m.quick.len(), 34);
    assert_eq!(m.labels.get("max_height_above_ground_m").map(String::as_str), Some("I"));
    assert_eq!(m.labels.get("mantle_check_range").map(String::as_str), Some("V"));
    // a mantle onto a 1.0 m box with the real data: grounded on top after the SP time (+- 1 tick)
    let cfg = SimCfg::defaults(&d);
    let mut p = Player::new(&d, V3::ZERO);
    let mut w = World::flat().with(Convex::aabb(V3::new(-200.0, -10.0, 30.0), V3::new(200.0, 1.0 / 0.0254, 2000.0)));
    let mut ev = Vec::new();
    let (mut t0, mut dur, mut dt) = (0.0, 0.0, 0.0);
    for k in 0..120 {
        let now = (k + 1) as f64 / 60.0;
        let i = Input { w: true, jump_held: true, jump_presses: (k == 0) as u32, fwd_h: V3::new(0.0, 0.0, 1.0), right_h: V3::new(1.0, 0.0, 0.0), ..Input::default() };
        p.tick(&d, &cfg, &i, now, 1.0 / 60.0, &mut w, &mut ev);
        for e in ev.drain(..) {
            match e {
                SimEvent::MantleStart(s) => {
                    t0 = now;
                    dur = s.dur;
                }
                SimEvent::MantleEnd { dt: x, .. } => dt = x,
                _ => {}
            }
        }
    }
    assert!(t0 > 0.0 && (dt - dur).abs() <= 1.0 / 60.0 + 1e-6, "dur {dur} dt {dt}");
    // M8 review: the plan is MW2's own up time (the SP anims' 500 ms), not the code's own number
    assert!((dur - m.up_s as f64).abs() < 1e-5, "planned {dur} vs MW2 up {}", m.up_s);
    // the quick clips by name (V): the UMP45 reuses its putaway / pullout, the M4 has dedicated quick xanims
    let u = m.quick_clips("ump45_mp").expect("ump45 quick clips");
    assert!(u.drop_anim == "viewmodel_ump45_putaway" && u.raise_anim == "viewmodel_ump45_pullout" && u.drop_exact && u.raise_exact);
    let m4 = m.quick_clips("m4_mp").expect("m4 quick clips");
    assert!(m4.drop_anim == "viewmodel_M4_quick_putaway" && !m4.drop_exact);
    let exact = m.quick_clips.values().filter(|q| q.drop_exact && q.raise_exact).count();
    assert_eq!(m.quick_clips.len(), 34);
    assert_eq!(m.labels.get("mantle_cross_distance_eased").map(String::as_str), Some("I"));
    assert!(p.grounded && (p.pos.y - 1.0 / 0.0254).abs() < 0.1);
    println!(
        "PASS: movement.json mantle block loads ({} labels, curves sha {}) and equals the design's SP numbers; a 1.0 m mantle takes {dt:.3} s (planned {dur:.3} = MW2 up {:.3}); quick clips {exact}/34 guns exact putaway/pullout: {}",
        m.labels.len(),
        &m.curves_sha[..12],
        m.up_s,
        m.summary()
    );
}
