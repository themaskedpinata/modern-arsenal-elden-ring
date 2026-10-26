//! M4 real-data cross-checks (check_m4.ps1 checks 3 and 6; docs/M4_DESIGN.md 14.1): the rows.json ac130 block against
//! mw2er_core::ac130_dmg, and the Rust BC3 decoder against the Python converter's decode of the overlays. Ignored by
//! default (they need the local converted data and build); run with
//! `MW2ER_ROOT=D:\Mods\MW2inER cargo test -p mw2er-core --test real_data_m4 -- --include-ignored`. Missing inputs
//! print `SKIP:` and return (check_m4 counts that as FAIL).

use std::path::{Path, PathBuf};

use mw2er_core::rows::RowsFile;

fn root() -> PathBuf {
    std::env::var_os("MW2ER_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

// ---- M4 (docs/M4_DESIGN.md 14.1 ac130_dmg / thermal real-data tests; check_m4 checks 3 and 6) ----------------------

#[test]
#[ignore = "needs build/rows.json (with the ac130 block) and converted/ac130/weapons_ac130.json"]
fn real_ac130_rows_match_dmg() {
    use mw2er_core::ac130_data::WeaponsFile as AcWeapons;
    use mw2er_core::ac130_dmg::{Explosion, M_PER_IN, gun_tiers};
    let root = root();
    let (Some(rows), Some(weapons)) = (
        read(&root.join("build").join("rows.json")),
        read(&root.join("converted").join("ac130").join("weapons_ac130.json")),
    ) else {
        println!("SKIP: build/rows.json or converted/ac130/weapons_ac130.json missing");
        return;
    };
    let rows = RowsFile::parse(&rows).expect("rows.json validates");
    let ac = rows.ac130.as_ref().expect("rows.json has the ac130 block");
    let w = AcWeapons::parse(&weapons).expect("weapons_ac130.json validates");
    let mut n = 0;
    for g in &w.guns {
        let e = Explosion { radius_in: g.explosion_radius_in, inner: g.explosion_inner, outer: g.explosion_outer };
        assert!((g.explosion_radius_m - g.explosion_radius_in * M_PER_IN).abs() < 1e-4, "gun {} radius_m", g.id);
        let r = ac.gun(&g.id).unwrap_or_else(|| panic!("rows.json ac130 has no gun {}", g.id));
        let two = r.core.is_some();
        let t = gun_tiers(&e, ac.k_gun(&g.id), two).expect("tiers in range");
        let pairs = [(t.core, r.core), (t.ring, r.ring), (Some(t.single), Some(r.single))];
        for (want, got) in pairs {
            match (want, got) {
                (Some(a), Some(b)) => {
                    assert_eq!(a.atk_phys, b.atk_phys as i64, "gun {} row {} atkPhys", g.id, b.id);
                    assert!((a.mw2 - b.mw2).abs() < 1e-9, "gun {} row {} mw2 {} != {}", g.id, b.id, a.mw2, b.mw2);
                    assert!((a.radius_m - b.radius_m).abs() < 1e-4, "gun {} row {} radius", g.id, b.id);
                    n += 1;
                }
                (None, None) => {}
                other => panic!("gun {}: tier presence differs {other:?}", g.id),
            }
        }
    }
    assert_eq!(n, 7, "7 AC-130 explosion rows");
    assert_eq!(ac.atk_mode, "flat", "run 8 fix: the explosion rows deal flat damage (rebuild the regulation)");
    println!("PASS: rows.json ac130 block == ac130_dmg (k={}) for {n} rows; radii == MW2 radius x 0.0254", ac.k);
}

#[test]
#[ignore = "needs converted/ac130/*.vmtex and work/m4/overlay_stats.json (convert_ac130.py)"]
fn real_overlay_bc3_matches_python() {
    use mw2er_core::ac130_data::OVERLAY_FILES;
    use mw2er_core::sha256::sha256_hex;
    use mw2er_core::thermal::bc3_decode;
    use mw2er_core::vm_assets::parse_tex;
    let root = root();
    let Some(stats) = read(&root.join("work").join("m4").join("overlay_stats.json")) else {
        println!("SKIP: work/m4/overlay_stats.json missing");
        return;
    };
    let stats: serde_json::Value = serde_json::from_str(&stats).expect("overlay_stats.json is JSON");
    for f in OVERLAY_FILES {
        let Ok(b) = std::fs::read(root.join("converted").join("ac130").join(f)) else {
            println!("SKIP: converted/ac130/{f} missing");
            return;
        };
        let t = parse_tex(&b).expect("vmtex parses");
        let rgba = bc3_decode(&t.mips[0].data, t.width, t.height).expect("bc3 decodes");
        let key = f.trim_end_matches(".vmtex");
        let s = &stats["overlays"][key];
        assert_eq!(sha256_hex(&rgba), s["rgba_sha256"].as_str().expect("rgba_sha256"), "{f}: Rust BC3 != Python BC3");
        let cov = rgba.chunks_exact(4).filter(|p| p[3] > 0).count() as f64 / (t.width * t.height) as f64;
        assert!((cov - s["alpha_coverage"].as_f64().expect("coverage")).abs() < 1e-12, "{f}: alpha coverage");
    }
    println!("PASS: Rust thermal::bc3_decode == the Python decoder (rgba sha256 + alpha coverage) for {} overlays", OVERLAY_FILES.len());
}
