//! M6 real-data cross-checks (check_m6.ps1 check 6): every converted gun of `converted\guns\` loads through the same
//! models the DLL uses (stats.json -> GunSpec, clips.json, the viewmodel bundle with shared textures), every timed
//! clip plays over its weapon time, and `build\rows.json`'s arsenal block equals the Rust inversion row by row.
//! Ignored by default (it needs the local converted data); run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data_m6 -- --include-ignored`. A missing input
//! prints `SKIP:` and returns (check_m6 counts that as FAIL).

use std::path::{Path, PathBuf};

use mw2er_core::arsenal::{ClipsFile, GunClass, GunSpec, IndexFile, StatsFile};
use mw2er_core::rows::{RowsFile, planned_atk};
use mw2er_core::vm_anim::VmParams;
use mw2er_core::vm_assets::{ClipId, TextureCache, VmAssets};

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

/// Role name -> ClipId (the converter's role names are the ClipId names).
fn clip_of(role: &str) -> Option<ClipId> {
    ClipId::every().find(|c| c.name() == role)
}

#[test]
#[ignore = "needs converted/guns (tools/scripts/convert_arsenal.py)"]
fn real_arsenal_guns_load_and_clip_rates() {
    let dir = root().join("converted").join("guns");
    let Ok(text) = std::fs::read_to_string(dir.join("index.json")) else {
        println!("SKIP: {} missing", dir.join("index.json").display());
        return;
    };
    let idx = IndexFile::parse(&text).unwrap();
    let mut cache = TextureCache::new();
    let (mut n, mut clips_checked, mut tex_refs) = (0, 0, 0);
    let mut classes = std::collections::BTreeMap::new();
    for e in &idx.guns {
        assert!(e.ok, "{} not converted: {:?}", e.name, e.problems);
        let gd = dir.join(&e.name);
        let st = StatsFile::parse(&std::fs::read_to_string(gd.join("stats.json")).unwrap()).unwrap();
        let spec = GunSpec::from_stats(&st).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(Some(spec.g), e.g, "{}: g", e.name);
        *classes.entry(spec.class.label()).or_insert(0) += 1;
        let clips = ClipsFile::parse(&std::fs::read_to_string(gd.join("clips.json")).unwrap()).unwrap();
        let a = VmAssets::load_gun_dir(&gd, &mut cache).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        tex_refs += a.textures.len();
        assert!(a.skel.bones.len() <= 128, "{}: {} bones over the shader palette", e.name, a.skel.bones.len());
        let p = VmParams::from_gun_assets(&a, 0.05, 5.0);
        for (role, c) in &clips.roles {
            let Some(id) = clip_of(role) else { continue };
            assert!(a.has(id), "{}: role {role} written but not loaded", e.name);
            assert!((a.clip(id).duration() as f64 - c.duration).abs() < 1e-3, "{} {role}: duration", e.name);
            // every timed clip plays over its weapon time (section 5.3), within one tick
            if c.time > 0.0 && c.duration > 0.0 && !c.looped && !matches!(id, ClipId::AdsUp | ClipId::AdsDown) {
                let real = p.real_duration(id);
                // review M6 [I]: fire clips never play faster than FIRE_MAX_RATE
                let want = if matches!(id, ClipId::Fire | ClipId::FireAds | ClipId::LastShot) {
                    c.time.max(c.duration / mw2er_core::vm_anim::FIRE_MAX_RATE as f64)
                } else {
                    c.time
                };
                assert!((real - want).abs() <= 1.0 / 60.0, "{} {role}: {real} s vs {want} s", e.name);
                clips_checked += 1;
            }
        }
        // mechanics flags agree with the clips
        if spec.rechamber.is_some() {
            assert!(a.has(ClipId::Rechamber), "{}: bolt gun without rechamber clip", e.name);
        }
        if matches!(spec.reload, mw2er_core::arsenal::ReloadKind::Segmented { .. }) {
            assert!(a.has(ClipId::ReloadStart) && a.has(ClipId::ReloadEnd), "{}: segmented without start/end", e.name);
        }
        if spec.has_rocket_part {
            assert!(a.has_rocket(), "{}: rocket part missing", e.name);
        }
        n += 1;
    }
    assert_eq!(n, idx.guns.len());
    assert!(cache.len() < tex_refs, "textures are shared ({} files for {tex_refs} references)", cache.len());
    println!(
        "PASS: {n} guns load (stats -> GunSpec, clips.json, viewmodel bundles with {} shared textures for {tex_refs} references), \
         {clips_checked} timed clips play over their weapon times; classes {classes:?}",
        cache.len()
    );
}

#[test]
#[ignore = "needs build/rows.json with the arsenal block and converted/guns"]
fn real_arsenal_rows_match_inversion() {
    let r = root();
    let Ok(text) = std::fs::read_to_string(r.join("build").join("rows.json")) else {
        println!("SKIP: build/rows.json missing");
        return;
    };
    let rows = RowsFile::parse(&text).expect("rows.json validates (incl. every arsenal atk == the Rust inversion)");
    let Some(a) = rows.arsenal.as_ref() else {
        println!("SKIP: rows.json has no arsenal block");
        return;
    };
    let mut n = 0;
    for (name, g) in &a.guns {
        let st = StatsFile::parse(&std::fs::read_to_string(r.join("converted").join("guns").join(name).join("stats.json")).unwrap()).unwrap();
        let spec = GunSpec::from_stats(&st).unwrap();
        assert_eq!(g.g, spec.g, "{name}: g");
        if spec.class == GunClass::Launcher {
            let l = g.launcher.expect("launcher rows");
            assert_eq!(l.impact.atk, planned_atk(spec.falloff.damage as f64, a.s, a.d_ref));
            n += 2 + [l.core, l.ring].iter().flatten().count() + 1;
        } else {
            let want = spec.band_damages();
            let got: Vec<i32> = g.bands.iter().map(|b| b.mw2).collect();
            assert_eq!(got, want, "{name}: bands");
            for b in &g.bands {
                assert_eq!(b.atk, planned_atk(b.mw2 as f64, a.s, a.d_ref), "{name} band {}", b.band);
            }
            n += g.bands.len();
        }
    }
    let u = &a.guns["ump45_mp"];
    let close = mw2er_core::ac130_dmg::er_damage(u.bands[0].atk as f64, a.d_ref);
    assert!((close - 49.0).abs() <= 1.0, "UMP45 close {close} (~49 on c4311)");
    println!(
        "PASS: rows.json arsenal == Rust inversion: {} guns, {n} rows (mode {} s={} d_ref={}); UMP45 close {close:.1} far {:.1}",
        a.guns.len(),
        a.mode,
        a.s,
        a.d_ref,
        mw2er_core::ac130_dmg::er_damage(u.bands.last().unwrap().atk as f64, a.d_ref)
    );
}
