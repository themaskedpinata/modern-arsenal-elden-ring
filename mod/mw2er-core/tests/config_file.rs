//! The shipped `mw2er.toml` must be accepted by the same parser the DLL uses (check_m2 check 17).

use std::path::Path;

use mw2er_core::config::Config;

#[test]
fn shipped_config_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("mw2er.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let cfg = Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(cfg.regulation.weapons.contains_key(&cfg.gameplay.weapon), "gameplay.weapon has a regulation slot");
    println!("PASS: {} parses (k={}, weapon={})", path.display(), cfg.damage.k, cfg.gameplay.weapon);
}

/// C6 (docs/C6_HOLD_DESIGN.md parts 12-18): the shipped config is the released C6 feature set (0.3.0+): the hand hold,
/// the polearm stance with the guard ADS pose, the scope peek and the shoulder camera on; every test probe, the
/// ER-depth occlusion, the crossbow carrier and its item grant off. The release policy (tools/coop_friend/friend_toml.ps1)
/// produces this from the host's toml, so the host's own test toml fails here until it is put through that policy.
#[test]
fn c6_shipped_config_release_set() {
    // MW2ER_RELEASE_TOML: the host toml after the release policy (check_c6 check 6); default the repository's mw2er.toml
    let path = std::env::var_os("MW2ER_RELEASE_TOML").map(std::path::PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("mw2er.toml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let c = Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())).coop;
    // released: hand hold on the weapon frame, polearm carry, guard ADS (C6 parts 12-17)
    assert_eq!((c.hold_mode.as_str(), c.er_stance.as_str(), c.tp_ads_hold.as_str(), c.tp_ads_er_aim), ("hand", "polearm", "aim", false));
    assert_eq!((c.stance_refresh.as_str(), c.hold_mounted.as_str(), c.hold_source.as_str()), ("chord", "hand", "weapon"));
    assert_eq!((c.tp_ads_pose.as_str(), c.tp_ads_patch.as_str(), c.hide_state_patch, c.world_gun_body_occlusion), ("guard", "ads", true, true));
    assert!(c.tp_guard_yaw && c.guard_pose_on() && c.polearm_grip_on());
    // never shipped: the probes, the ER-depth occlusion, the crossbow carrier (no inventory change, no give, no load)
    assert_eq!((c.probe_er_depth, c.probe_er_depth_state, c.probe_er_depth_sample, c.probe_er_aim), (false, false, false, false));
    assert!(!c.world_gun_er_depth, "coop.world_gun_er_depth is a test and must stay false");
    assert_eq!((c.er_carrier.as_str(), c.carrier_give, c.carrier_load), ("off", false, false));
    let cfg = Config::parse(&text).unwrap();
    assert!(cfg.first_person.tp_scope_peek, "first_person.tp_scope_peek is released (C6 scope peek)");
    assert!(cfg.third_person.shoulder().on(), "third_person shoulder camera is released (C6 part 18)");
    println!("PASS: C6 release set (hold_mode=hand er_stance=polearm tp_ads_pose=guard tp_scope_peek=true shoulder=on; probes / er_depth / carrier off)");
}