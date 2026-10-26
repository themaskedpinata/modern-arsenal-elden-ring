//! AC-130 explosion damage model (docs/M4_DESIGN.md section 8.2). Pure; `tools/scripts/build_regulation.py`
//! implements the same function and check_m4 compares the two through `build\rows.json`.
//!
//! MW2 radius damage is assumed linear [I]: `dmg(d) = outer + (inner - outer) * (1 - d / r)`.
//! Two-tier additive model: the DLL spawns a core sphere (radius r/2) and a ring sphere (radius r) in the same tick.
//! The ring carries `dmg(3r/4)`, the core carries `dmg(r/4) - dmg(3r/4)`, so an enemy inside the core receives
//! `dmg(r/4)` in total. The single-sphere fallback (and the 25 mm, always) carries `dmg(r/2)`.
//! ER `atkPhys = round_half_away(mw2 * k)`, which must fit a u16.

/// One gun's MW2 explosion numbers (from `converted\ac130\weapons_ac130.json`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Explosion {
    pub radius_in: f64,
    pub inner: f64,
    pub outer: f64,
}

/// MW2 inches per metre conversion (1 in = 0.0254 m).
pub const M_PER_IN: f64 = 0.0254;

/// `dmg(d)` for `0 <= d <= r` (clamped outside).
pub fn dmg_at(e: &Explosion, d: f64) -> f64 {
    if e.radius_in <= 0.0 {
        return e.inner;
    }
    let f = (1.0 - d / e.radius_in).clamp(0.0, 1.0);
    e.outer + (e.inner - e.outer) * f
}

/// Round half away from zero (the same rule as Rust `f64::round` and `build_regulation.rhaz`).
pub fn rhaz(x: f64) -> i64 {
    x.round() as i64
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tier {
    /// Sphere radius in metres.
    pub radius_m: f64,
    /// MW2 damage carried by this sphere.
    pub mw2: f64,
    /// ER atkPhys.
    pub atk_phys: i64,
}

/// The rows of one gun: core + ring (two-tier) and the single fallback; `core`/`ring` are `None` for a gun that
/// always uses one sphere (the 25 mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunTiers {
    pub core: Option<Tier>,
    pub ring: Option<Tier>,
    pub single: Tier,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DmgError {
    OutOfRange { what: &'static str, atk: i64 },
}

impl std::fmt::Display for DmgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DmgError::OutOfRange { what, atk } => write!(f, "{what}: atkPhys {atk} outside 1..=65535"),
        }
    }
}

fn tier(what: &'static str, radius_m: f64, mw2: f64, k: f64) -> Result<Tier, DmgError> {
    let atk = rhaz(mw2 * k);
    if !(1..=65535).contains(&atk) {
        return Err(DmgError::OutOfRange { what, atk });
    }
    Ok(Tier { radius_m, mw2, atk_phys: atk })
}

/// The tier plan of one gun. `two_tier = false` (the 25 mm) gives only the single sphere.
pub fn gun_tiers(e: &Explosion, k: f64, two_tier: bool) -> Result<GunTiers, DmgError> {
    let r = e.radius_in;
    let r_m = r * M_PER_IN;
    let single = tier("single", r_m, dmg_at(e, r / 2.0), k)?;
    if !two_tier {
        return Ok(GunTiers { core: None, ring: None, single });
    }
    let q1 = dmg_at(e, r / 4.0);
    let q3 = dmg_at(e, 3.0 * r / 4.0);
    let core = tier("core", r_m / 2.0, q1 - q3, k)?;
    let ring = tier("ring", r_m, q3, k)?;
    Ok(GunTiers { core: Some(core), ring: Some(ring), single })
}

/// Community Elden Ring attack-vs-defence curve (the same as `analyze_log.er_damage` / `build_regulation.er_damage`;
/// [R], info only: used to choose k for the flat-damage rows of the run-8 fix, never checked against the game).
pub fn er_damage(atk: f64, defence: f64) -> f64 {
    if atk <= 0.0 || defence <= 0.0 {
        return atk.max(0.0);
    }
    let r = atk / defence;
    let m = if r > 8.0 {
        0.9
    } else if r > 2.5 {
        0.9 - 0.8 / 121.0 * (r - 8.0).powi(2)
    } else if r > 1.0 {
        0.7 - 0.4 / 3.0 * (r - 2.5).powi(2)
    } else if r > 0.125 {
        0.1 + 19.2 / 49.0 * (r - 0.125).powi(2)
    } else {
        0.1
    };
    atk * m
}

/// Hits needed to kill `hp` with `dmg` per hit.
pub fn hits_to_kill(hp: f64, dmg: f64) -> u32 {
    if dmg <= 0.0 { u32::MAX } else { (hp / dmg).ceil() as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E105: Explosion = Explosion { radius_in: 600.0, inner: 1200.0, outer: 300.0 };
    const E40: Explosion = Explosion { radius_in: 150.0, inner: 500.0, outer: 50.0 };
    const E25: Explosion = Explosion { radius_in: 32.0, inner: 500.0, outer: 10.0 };

    #[test]
    fn ac130_dmg_endpoints() {
        assert_eq!(dmg_at(&E105, 0.0), 1200.0);
        assert_eq!(dmg_at(&E105, 600.0), 300.0);
        assert_eq!(dmg_at(&E105, 900.0), 300.0, "clamped beyond r");
        assert_eq!(dmg_at(&E40, 75.0), 275.0);
    }

    #[test]
    fn ac130_dmg_table_8_2_exact() {
        // docs/M4_DESIGN.md table 8.2 at k = 4
        assert_eq!(dmg_at(&E105, 150.0), 975.0);
        assert_eq!(dmg_at(&E105, 450.0), 525.0);
        assert_eq!(dmg_at(&E105, 300.0), 750.0);
        assert_eq!(dmg_at(&E40, 37.5), 387.5);
        assert_eq!(dmg_at(&E40, 112.5), 162.5);
        let g = gun_tiers(&E105, 4.0, true).unwrap();
        assert_eq!((g.core.unwrap().atk_phys, g.ring.unwrap().atk_phys, g.single.atk_phys), (1800, 2100, 3000));
        let g = gun_tiers(&E40, 4.0, true).unwrap();
        assert_eq!((g.core.unwrap().atk_phys, g.ring.unwrap().atk_phys, g.single.atk_phys), (900, 650, 1100));
        let g = gun_tiers(&E25, 4.0, false).unwrap();
        assert!(g.core.is_none() && g.ring.is_none());
        assert_eq!(g.single.atk_phys, 1020);
        // radii in metres
        assert!((gun_tiers(&E105, 4.0, true).unwrap().core.unwrap().radius_m - 7.62).abs() < 1e-9);
        assert!((g.single.radius_m - 0.8128).abs() < 1e-9);
    }

    #[test]
    fn ac130_dmg_additive_core_plus_ring() {
        for (e, k) in [(E105, 4.0), (E40, 4.0), (E105, 3.0), (E40, 2.5)] {
            let g = gun_tiers(&e, k, true).unwrap();
            let total = g.core.unwrap().mw2 + g.ring.unwrap().mw2;
            assert!((total - dmg_at(&e, e.radius_in / 4.0)).abs() < 1e-9, "core + ring = dmg(r/4)");
            assert!((total * k - (g.core.unwrap().atk_phys + g.ring.unwrap().atk_phys) as f64).abs() <= 1.0);
        }
    }

    #[test]
    fn ac130_dmg_er_curve_continuous() {
        // the four pieces meet at r = 0.125, 1, 2.5 and 8
        for r in [0.125, 1.0, 2.5, 8.0] {
            let lo = er_damage(100.0 * (r - 1e-9), 100.0) / (r - 1e-9);
            let hi = er_damage(100.0 * (r + 1e-9), 100.0) / (r + 1e-9);
            assert!((lo - hi).abs() < 1e-5, "r={r}: {lo} vs {hi}");
        }
        assert!((er_damage(1000.0, 100.0) - 900.0).abs() < 1e-9);
        assert_eq!(er_damage(0.0, 100.0), 0.0);
    }

    /// Run-8 fix targets (mw2er.toml [regulation.ac130] k 0.65, scale_25 0.7, flat damage) on a Limgrave Godrick
    /// Soldier (c4311, 219 HP, def ~110, neutral cut 1.0) [I]: 105 one-shot inside the core, 40 mm 2-3 direct hits,
    /// 25 mm several (4-6) hits.
    #[test]
    fn ac130_dmg_run8_targets() {
        let (hp, def) = (219.0, 110.0);
        let g = gun_tiers(&E105, 0.65, true).unwrap();
        let core = er_damage(g.core.unwrap().atk_phys as f64, def) + er_damage(g.ring.unwrap().atk_phys as f64, def);
        assert_eq!((g.core.unwrap().atk_phys, g.ring.unwrap().atk_phys, g.single.atk_phys), (293, 341, 488));
        assert!(core >= 1.5 * hp, "105 inside the core: {core}");
        assert_eq!(hits_to_kill(hp, core), 1);
        let g = gun_tiers(&E40, 0.65, true).unwrap();
        assert_eq!((g.core.unwrap().atk_phys, g.ring.unwrap().atk_phys, g.single.atk_phys), (146, 106, 179));
        let d40 = er_damage(g.core.unwrap().atk_phys as f64, def) + er_damage(g.ring.unwrap().atk_phys as f64, def);
        assert!((2..=3).contains(&hits_to_kill(hp, d40)), "40 mm direct {d40}");
        let g = gun_tiers(&E25, 0.65 * 0.7, false).unwrap();
        assert_eq!(g.single.atk_phys, 116);
        let d25 = er_damage(g.single.atk_phys as f64, def);
        assert!((4..=6).contains(&hits_to_kill(hp, d25)), "25 mm {d25}");
        // the old k = 4 rows (if ER had used atkPhys) would have one-shot with every gun and every tier
        assert_eq!(hits_to_kill(hp, er_damage(650.0, def)), 1);
    }

    #[test]
    fn ac130_dmg_rounding_and_u16() {
        assert_eq!(rhaz(2.5), 3);
        assert_eq!(rhaz(-2.5), -3);
        assert_eq!(rhaz(1100.4999), 1100);
        // k = 0.0001 makes an atkPhys of 0: refused (no zero-damage rows)
        assert!(gun_tiers(&E25, 0.0001, false).is_err());
        // a huge k overflows u16: refused
        let e = gun_tiers(&E105, 100.0, true).unwrap_err();
        assert!(e.to_string().contains("65535"), "{e}");
        assert!(gun_tiers(&E105, 54.0, true).is_ok());
    }
}
