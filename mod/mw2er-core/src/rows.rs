//! Model of `build\rows.json`, the manifest `build_regulation.py` writes next to the package.
//!
//! It lists the regulation rows the mod added (Bullet + AtkParam_Pc share ids), the damage factor
//! `k` that was baked into `atkPhys`, and the marker bullet id. The DLL reads it at runtime to pick
//! a falloff band per shot and to verify the live params (PARAMS check).

use std::collections::BTreeMap;

use serde::Deserialize;

/// Row id of one band: `id_base + slot * 100 + band` (docs/M2_DESIGN.md section 16.1).
pub fn band_row_id(id_base: i64, slot: u32, band: u32) -> i64 {
    id_base + slot as i64 * 100 + band as i64
}

#[derive(Clone, Debug, Deserialize)]
pub struct RowsFile {
    pub schema: u32,
    #[serde(default)]
    pub generator: String,
    #[serde(default)]
    pub built_utc: String,
    #[serde(default)]
    pub vanilla_sha256: String,
    #[serde(default)]
    pub package_regulation_sha256: String,
    pub k: f64,
    pub id_base: i64,
    pub marker_id: i32,
    pub bullet: BulletInfo,
    pub atk: AtkInfo,
    pub weapons: BTreeMap<String, RowsWeapon>,
    /// M4: the AC-130 explosion rows and the protection SpEffect (absent in an M2/M3 build).
    #[serde(default)]
    pub ac130: Option<RowsAc130>,
    /// M6: the arsenal rows (absent before M6).
    #[serde(default)]
    pub arsenal: Option<RowsArsenal>,
    /// M7: the nuke rows (absent before M7).
    #[serde(default)]
    pub nuke: Option<RowsNuke>,
    /// C3 PASS C: the co-op twins and marker rows (absent before C3 PASS C).
    #[serde(default)]
    pub coop: Option<RowsCoop>,
}

/// C3 PASS C (docs/C3_DESIGN.md 3.3): one visual twin (no attack, network-synced at spawn) per gun / launcher kind /
/// explosion tier / AC-130 tier, the nuke marker bullet and the synced marker SpEffects.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RowsCoop {
    pub twins: Vec<RowsTwin>,
    pub nuke_marker_bullet: i32,
    pub sp_mode: i32,
    pub sp_gun_base: i32,
    pub sp_gun_count: i32,
    pub sp_ac130: i32,
    pub sp_nuke: i32,
    /// The vanilla SpEffectVfx row the MW2-mode marker uses (8060: isInvisibleWeapon, no sfx).
    pub sp_mode_vfx: i32,
    /// effectEndurance of the marker SpEffects (finite: an orphan expires on its own).
    pub marker_endurance_s: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct RowsTwin {
    pub id: i32,
    pub src: i32,
}

impl RowsCoop {
    /// The ids agree with the DLL's id rules (`coop_vis`): every twin is `twin_of(src)`, the markers are the reserved
    /// ones, the endurance finite.
    pub fn validate(&self) -> Result<(), String> {
        use crate::coop_vis as cv;
        if self.twins.is_empty() {
            return Err("coop: no twins".into());
        }
        for t in &self.twins {
            if cv::twin_of(t.src) != Some(t.id) || cv::decode_twin(t.id).is_none() {
                return Err(format!("coop: twin {} of {} breaks the id rule", t.id, t.src));
            }
        }
        let want = (cv::NUKE_MARKER_BULLET, cv::SP_MODE, cv::SP_GUN_BASE, cv::SP_AC130, cv::SP_NUKE);
        let got = (self.nuke_marker_bullet, self.sp_mode, self.sp_gun_base, self.sp_ac130, self.sp_nuke);
        if got != want || self.sp_gun_count != cv::GUN_G.len() as i32 {
            return Err(format!("coop: marker ids {got:?} / {} (want {want:?} / {})", self.sp_gun_count, cv::GUN_G.len()));
        }
        if !(self.marker_endurance_s > 0.0 && self.marker_endurance_s <= 600.0) {
            return Err(format!("coop: marker endurance {} (finite 0..600 s)", self.marker_endurance_s));
        }
        Ok(())
    }

    pub fn twin_ids(&self) -> Vec<i32> {
        self.twins.iter().map(|t| t.id).collect()
    }
}

/// M7 (docs/M7_DESIGN.md 3.13): the lethal SpEffect, the optional lethal bullet and the visual horizon explosion.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct RowsNuke {
    pub lethal_speffect_id: i32,
    pub lethal_bullet_id: i32,
    pub horizon_bullet_id: i32,
    pub horizon_sfx: i32,
}

// ---- M6: the arsenal block (docs/M6_DESIGN.md section 9) ------------------------------------------------------

/// `id = 79_100_000 + g*1000 + kind*100 + idx` (Bullet and AtkParam_Pc share it).
pub fn arsenal_row_id(id_base: i64, g: u32, kind: u32, idx: u32) -> i64 {
    id_base + g as i64 * 1000 + kind as i64 * 100 + idx as i64
}

pub const ARSENAL_KIND_BANDS: u32 = 0;
pub const ARSENAL_KIND_LAUNCHER: u32 = 5;
pub const ARSENAL_KIND_EXPLOSION: u32 = 6;

/// The atkPhys whose ER damage on defence `d` is `target` (monotone bisection of the community curve, the mirror of
/// build_regulation.inv_er) [I curve].
pub fn inv_er(target: f64, d: f64) -> f64 {
    if !(target > 0.0 && target.is_finite()) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0f64, 65_535.0f64);
    if crate::ac130_dmg::er_damage(hi, d) < target {
        return hi;
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if crate::ac130_dmg::er_damage(mid, d) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The planned atkPhys of an MW2 damage: `round(inv_er(s * mw2, d_ref))` (half away from zero, as rhaz in Python).
pub fn planned_atk(mw2: f64, s: f64, d_ref: f64) -> i32 {
    inv_er(s * mw2, d_ref).round() as i32
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ArsenalBand {
    pub band: u32,
    pub id: i32,
    pub mw2: i32,
    pub atk: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ArsenalExplRow {
    pub id: i32,
    pub radius_m: f64,
    pub mw2: f64,
    pub atk: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ArsenalLauncherRows {
    /// The impact row (kind 5 idx 0): a hitscan spawned at a direct character hit.
    pub impact: ArsenalBand,
    /// kind 5 idx 1: the inert marker of the engine-projectile fallback.
    pub marker_id: i32,
    #[serde(default)]
    pub core: Option<ArsenalExplRow>,
    #[serde(default)]
    pub ring: Option<ArsenalExplRow>,
    pub single: ArsenalExplRow,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ArsenalGunRows {
    pub g: u32,
    pub class: String,
    #[serde(default)]
    pub bands: Vec<ArsenalBand>,
    #[serde(default)]
    pub launcher: Option<ArsenalLauncherRows>,
}

impl ArsenalGunRows {
    /// The band whose MW2 damage is closest to `mw2` (ties go to the higher damage).
    pub fn band_for_damage(&self, mw2: i32) -> Option<&ArsenalBand> {
        self.bands.iter().min_by_key(|b| ((b.mw2 - mw2).abs(), -b.mw2))
    }

    pub fn ids(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.bands.iter().map(|b| b.id).collect();
        if let Some(l) = &self.launcher {
            v.push(l.impact.id);
            v.push(l.marker_id);
            v.extend([l.core, l.ring, Some(l.single)].into_iter().flatten().map(|r| r.id));
        }
        v
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RowsArsenal {
    /// "flat_inv": atkPhys = round(inv_er(s * mw2, d_ref)), flat damage (isAddBaseAtk 1, corrections 0).
    pub mode: String,
    pub s: f64,
    pub d_ref: f64,
    pub id_base: i64,
    /// "none" (sfxId_Hit/Flick -1, the bug-2 likely fix) or "dagger" (the clone hit effects).
    #[serde(default)]
    pub bullet_sfx: String,
    pub guns: BTreeMap<String, ArsenalGunRows>,
}

impl RowsArsenal {
    pub fn ids(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.guns.values().flat_map(|g| g.ids()).collect();
        v.sort_unstable();
        v
    }

    pub fn validate(&self, marker_id: i32) -> Result<(), String> {
        if self.mode != "flat_inv" {
            return Err(format!("rows.json: arsenal mode {:?} (want flat_inv)", self.mode));
        }
        if !(self.s > 0.0 && self.s.is_finite() && self.d_ref > 0.0 && self.d_ref.is_finite()) {
            return Err("rows.json: arsenal s / d_ref must be > 0".into());
        }
        let mut gs = std::collections::BTreeSet::new();
        for (name, g) in &self.guns {
            if !gs.insert(g.g) {
                return Err(format!("rows.json: arsenal duplicate g {}", g.g));
            }
            for (i, b) in g.bands.iter().enumerate() {
                if b.band as usize != i || b.id as i64 != arsenal_row_id(self.id_base, g.g, ARSENAL_KIND_BANDS, b.band) {
                    return Err(format!("rows.json: arsenal {name} band {} id {} breaks the id scheme", b.band, b.id));
                }
                if i > 0 && b.mw2 >= g.bands[i - 1].mw2 {
                    return Err(format!("rows.json: arsenal {name} band damages must strictly decrease"));
                }
                let want = planned_atk(b.mw2 as f64, self.s, self.d_ref);
                if b.atk != want || !(1..=65535).contains(&b.atk) {
                    return Err(format!("rows.json: arsenal {name} band {} atk {} != inv_er({} x {}, {}) = {want}", b.band, b.atk, b.mw2, self.s, self.d_ref));
                }
                if b.id == marker_id {
                    return Err(format!("rows.json: arsenal {name} band id equals the marker"));
                }
            }
            if let Some(l) = &g.launcher {
                let base = arsenal_row_id(self.id_base, g.g, ARSENAL_KIND_LAUNCHER, 0);
                if l.impact.id as i64 != base || l.marker_id as i64 != base + 1 {
                    return Err(format!("rows.json: arsenal {name} launcher ids break the scheme"));
                }
                for (r, idx) in [(l.core, 1u32), (l.ring, 2), (Some(l.single), 3)] {
                    if let Some(r) = r
                        && (r.id as i64 != arsenal_row_id(self.id_base, g.g, ARSENAL_KIND_EXPLOSION, idx) || !(r.radius_m > 0.0) || !(1..=65535).contains(&r.atk))
                    {
                        return Err(format!("rows.json: arsenal {name} explosion row {} invalid", r.id));
                    }
                }
                if l.core.is_some() != l.ring.is_some() {
                    return Err(format!("rows.json: arsenal {name} has a core without a ring"));
                }
            } else if g.bands.is_empty() {
                return Err(format!("rows.json: arsenal {name} has no rows"));
            }
        }
        let ids = self.ids();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err("rows.json: duplicate arsenal ids".into());
        }
        Ok(())
    }
}

/// One AC-130 explosion row (Bullet + AtkParam_Pc share the id).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Ac130Row {
    pub id: i32,
    pub radius_m: f64,
    pub mw2: f64,
    pub atk_phys: i32,
    pub sfx: i32,
    /// After run 9 (object destruction): AtkParam_Pc atkObj and isEnableCalcDamageForBushesObj; Bullet
    /// sfxPostureType. `None` in an older rows.json (the PARAMS check then skips them).
    #[serde(default)]
    pub atk_obj: Option<u16>,
    #[serde(default)]
    pub obj_bushes: Option<u8>,
    #[serde(default)]
    pub sfx_posture: Option<u8>,
}

/// One gun's rows: core + ring (two-tier) and the single-sphere fallback.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Ac130GunRows {
    #[serde(default)]
    pub core: Option<Ac130Row>,
    #[serde(default)]
    pub ring: Option<Ac130Row>,
    pub single: Ac130Row,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RowsAc130 {
    pub k: f64,
    /// Per-gun multiplier of k (run 8 fix; absent in older rows.json = 1.0): atkPhys = round(mw2 * k * scale).
    #[serde(default)]
    pub gun_scale: BTreeMap<String, f64>,
    /// "flat" (run 8 fix: isAddBaseAtk 1, corrections 0) or "" (an older build: ER used the weapon AR).
    #[serde(default)]
    pub atk_mode: String,
    pub protect_speffect_id: i32,
    /// effectEndurance of the protection row (seconds; 0 = an old rows.json from before review M4 major 2).
    #[serde(default)]
    pub protect_endurance_s: f64,
    /// gun id ("105", "40", "25") -> rows.
    pub guns: BTreeMap<String, Ac130GunRows>,
}

/// The id range of the AC-130 explosion rows (slots 2..4).
pub const AC130_ID_RANGE: (i32, i32) = (79_000_200, 79_000_499);

impl RowsAc130 {
    /// Every explosion row id, ascending.
    pub fn ids(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.guns.values().flat_map(|g| [g.core, g.ring, Some(g.single)]).flatten().map(|r| r.id).collect();
        v.sort_unstable();
        v
    }

    pub fn gun(&self, id: &str) -> Option<&Ac130GunRows> {
        self.guns.get(id)
    }

    /// The effective k of one gun: `k * gun_scale[gid]` (1.0 when the gun has no scale).
    pub fn k_gun(&self, gid: &str) -> f64 {
        self.k * self.gun_scale.get(gid).copied().unwrap_or(1.0)
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.k > 0.0 && self.k.is_finite()) {
            return Err(format!("rows.json: ac130.k={} must be > 0", self.k));
        }
        if self.gun_scale.values().any(|s| !(*s > 0.0 && s.is_finite())) {
            return Err(format!("rows.json: ac130.gun_scale {:?} must be > 0", self.gun_scale));
        }
        for (gid, g) in &self.guns {
            let k = self.k_gun(gid);
            for r in [g.core, g.ring, Some(g.single)].into_iter().flatten() {
                if !(AC130_ID_RANGE.0..=AC130_ID_RANGE.1).contains(&r.id) {
                    return Err(format!("rows.json: ac130 {gid} id {} outside {:?}", r.id, AC130_ID_RANGE));
                }
                if r.atk_phys as i64 != (r.mw2 * k).round() as i64 || !(1..=65535).contains(&r.atk_phys) {
                    return Err(format!("rows.json: ac130 {gid} id {} atk_phys {} != round({} * k {k})", r.id, r.atk_phys, r.mw2));
                }
                if !(r.radius_m > 0.0 && r.radius_m.is_finite()) {
                    return Err(format!("rows.json: ac130 {gid} id {} radius {}", r.id, r.radius_m));
                }
            }
            if g.core.is_some() != g.ring.is_some() {
                return Err(format!("rows.json: ac130 {gid} has a core without a ring (or the reverse)"));
            }
        }
        let ids = self.ids();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err("rows.json: duplicate ac130 ids".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BulletInfo {
    pub speed_mps: f64,
    pub life_s: f64,
    pub hit_radius_m: f64,
    pub clone_from: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AtkInfo {
    pub clone_from: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RowsWeapon {
    pub slot: u32,
    pub bands: Vec<Band>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Band {
    pub band: u32,
    pub id: i32,
    pub mw2_damage: i32,
    pub atk_phys: i32,
}

impl RowsFile {
    pub fn parse(text: &str) -> Result<RowsFile, String> {
        let f: RowsFile = serde_json::from_str(text).map_err(|e| format!("rows.json: {e}"))?;
        f.validate()?;
        Ok(f)
    }

    /// Structural checks: ids follow the scheme, bands are ordered, the marker is outside the bands.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("rows.json: unsupported schema {}", self.schema));
        }
        if !(self.k > 0.0 && self.k.is_finite()) {
            return Err(format!("rows.json: k={} must be > 0", self.k));
        }
        for (name, w) in &self.weapons {
            if w.bands.is_empty() {
                return Err(format!("rows.json: {name} has no bands"));
            }
            for (i, b) in w.bands.iter().enumerate() {
                if b.band as usize != i {
                    return Err(format!("rows.json: {name} band {} at position {i}", b.band));
                }
                if b.id as i64 != band_row_id(self.id_base, w.slot, b.band) {
                    return Err(format!("rows.json: {name} band {} id {} breaks the id scheme", b.band, b.id));
                }
                if b.id == self.marker_id {
                    return Err(format!("rows.json: {name} band id {} equals the marker id", b.id));
                }
                if i > 0 && b.mw2_damage >= w.bands[i - 1].mw2_damage {
                    return Err(format!("rows.json: {name} band damages must strictly decrease"));
                }
                let expect = (b.mw2_damage as f64 * self.k).round() as i32;
                if b.atk_phys != expect {
                    return Err(format!(
                        "rows.json: {name} band {} atk_phys {} != round({} * k {})",
                        b.band, b.atk_phys, b.mw2_damage, self.k
                    ));
                }
            }
        }
        if let Some(a) = &self.ac130 {
            a.validate()?;
            let ours = self.all_ids();
            if a.ids().iter().any(|id| ours.contains(id)) {
                return Err("rows.json: an ac130 id collides with a weapon band or the marker".into());
            }
        }
        if let Some(ar) = &self.arsenal {
            ar.validate(self.marker_id)?;
            let ours = self.all_ids();
            let ac = self.ac130.as_ref().map(|a| a.ids()).unwrap_or_default();
            if ar.ids().iter().any(|id| ours.contains(id) || ac.contains(id)) {
                return Err("rows.json: an arsenal id collides with the legacy or ac130 rows".into());
            }
        }
        Ok(())
    }

    /// Every row id (bands of all weapons + the marker).
    pub fn all_ids(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self.weapons.values().flat_map(|w| w.bands.iter().map(|b| b.id)).collect();
        v.push(self.marker_id);
        v.sort_unstable();
        v
    }
}

impl RowsWeapon {
    /// The band whose MW2 damage is closest to `mw2_damage` (ties go to the higher damage).
    pub fn band_for_damage(&self, mw2_damage: i32) -> Option<&Band> {
        self.bands.iter().min_by_key(|b| ((b.mw2_damage - mw2_damage).abs(), -b.mw2_damage))
    }

    /// The MW2 damage of each band in band order.
    pub fn band_damages(&self) -> Vec<i32> {
        self.bands.iter().map(|b| b.mw2_damage).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample(k: f64) -> String {
        let bands: Vec<String> = (0..6)
            .map(|b| {
                let d = 40 - b;
                format!(
                    r#"{{"band":{b},"id":{},"mw2_damage":{d},"atk_phys":{}}}"#,
                    79000100 + b,
                    (d as f64 * k).round() as i32
                )
            })
            .collect();
        format!(
            r#"{{"schema":1,"generator":"t","k":{k},"id_base":79000000,"marker_id":79000900,
                "bullet":{{"speed_mps":999.0,"life_s":0.15,"hit_radius_m":0.02,"clone_from":10170000}},
                "atk":{{"clone_from":10170000}},
                "weapons":{{"g":{{"slot":1,"bands":[{}]}}}}}}"#,
            bands.join(",")
        )
    }

    #[test]
    fn rows_parse_and_band_choice() {
        let f = RowsFile::parse(&sample(4.0)).unwrap();
        let w = &f.weapons["g"];
        assert_eq!(w.bands[0].atk_phys, 160);
        assert_eq!(w.bands[5].atk_phys, 140);
        assert_eq!(w.band_for_damage(40).unwrap().id, 79000100);
        assert_eq!(w.band_for_damage(37).unwrap().id, 79000103);
        assert_eq!(w.band_for_damage(35).unwrap().id, 79000105);
        assert_eq!(w.band_for_damage(10).unwrap().id, 79000105);
        assert_eq!(w.band_for_damage(99).unwrap().id, 79000100);
        assert_eq!(f.all_ids().len(), 7);
        assert_eq!(band_row_id(79000000, 1, 5), 79000105);
    }

    #[test]
    fn c3_twin_rows_block_validates() {
        let ok = r#"{"twins":[{"id":79812000,"src":79112000},{"id":79800201,"src":79000201}],"nuke_marker_bullet":79000963,
            "sp_mode":79001090,"sp_gun_base":79001100,"sp_gun_count":34,"sp_ac130":79001150,"sp_nuke":79001190,
            "sp_mode_vfx":8060,"marker_endurance_s":30.0}"#;
        let c: RowsCoop = serde_json::from_str(ok).unwrap();
        c.validate().unwrap();
        assert_eq!(c.twin_ids(), vec![79812000, 79800201]);
        // a twin of a damaging row id that breaks the rule, a moved marker, a permanent marker: refused
        for (from, to) in [("\"id\":79812000", "\"id\":79812001"), ("79001190", "79001191"), ("30.0", "-1.0"), ("\"src\":79000201", "\"src\":79000960")] {
            let bad: RowsCoop = serde_json::from_str(&ok.replacen(from, to, 1)).unwrap();
            assert!(bad.validate().is_err(), "{from} -> {to} accepted");
        }
    }

    #[test]
    fn rows_reject_broken_scheme() {
        let s = sample(4.0).replace("79000103", "79000113");
        assert!(RowsFile::parse(&s).is_err());
        let s = sample(4.0).replace(r#""atk_phys":160"#, r#""atk_phys":161"#);
        assert!(RowsFile::parse(&s).is_err());
    }

    pub(crate) fn with_ac130(k: f64) -> String {
        let row = |id: i32, r: f64, mw2: f64, sfx: i32| {
            format!(r#"{{"id":{id},"radius_m":{r},"mw2":{mw2},"atk_phys":{},"sfx":{sfx}}}"#, (mw2 * k).round() as i32)
        };
        let block = format!(
            r#""ac130":{{"k":{k},"protect_speffect_id":79000950,"protect_endurance_s":90.0,"guns":{{
                "105":{{"core":{},"ring":{},"single":{}}},
                "40":{{"core":{},"ring":{},"single":{}}},
                "25":{{"single":{}}}}}}}"#,
            row(79000201, 7.62, 450.0, 524023),
            row(79000202, 15.24, 525.0, -1),
            row(79000203, 15.24, 750.0, 524023),
            row(79000301, 1.905, 225.0, 300602),
            row(79000302, 3.81, 162.5, -1),
            row(79000303, 3.81, 275.0, 300602),
            row(79000401, 0.8128, 255.0, -1)
        );
        let s = sample(k);
        let cut = s.rfind('}').unwrap();
        format!("{},{}}}", &s[..cut], block)
    }

    #[test]
    fn rows_ac130_gun_scale() {
        // run 8 fix: per-gun scale of k (25 mm x0.7); atk_phys must follow k * scale, an old block has none
        let s = with_ac130(4.0).replace(r#""ac130":{"k":4,"#, r#""ac130":{"k":4,"gun_scale":{"25":0.5},"atk_mode":"flat","#);
        assert!(RowsFile::parse(&s).is_err(), "25 mm atk_phys 1020 != round(255 * 4 * 0.5)");
        let s = s.replace(r#""atk_phys":1020"#, r#""atk_phys":510"#);
        let f = RowsFile::parse(&s).unwrap();
        let a = f.ac130.as_ref().unwrap();
        assert_eq!((a.k_gun("25"), a.k_gun("105"), a.atk_mode.as_str()), (2.0, 4.0, "flat"));
        assert!(RowsFile::parse(&s.replace(r#""25":0.5"#, r#""25":0.0"#)).is_err(), "scale 0");
        let old = RowsFile::parse(&with_ac130(4.0)).unwrap();
        assert!(old.ac130.as_ref().unwrap().gun_scale.is_empty() && old.ac130.as_ref().unwrap().atk_mode.is_empty());
    }

    #[test]
    fn rows_ac130_block() {
        let f = RowsFile::parse(&sample(4.0)).unwrap();
        assert!(f.ac130.is_none(), "an M2 rows.json has no ac130 block");
        let f = RowsFile::parse(&with_ac130(4.0)).unwrap();
        let a = f.ac130.as_ref().unwrap();
        assert_eq!(a.ids(), vec![79000201, 79000202, 79000203, 79000301, 79000302, 79000303, 79000401]);
        assert_eq!(a.gun("105").unwrap().core.unwrap().atk_phys, 1800);
        assert_eq!(a.gun("25").unwrap().single.atk_phys, 1020);
        assert_eq!(a.protect_endurance_s, 90.0);
        assert!(a.gun("25").unwrap().core.is_none());
        assert_eq!(f.all_ids().len(), 7, "the UMP45 id set is unchanged");
        assert!(RowsFile::parse(&with_ac130(4.0).replace(r#""atk_phys":1800"#, r#""atk_phys":1801"#)).is_err());
        assert!(RowsFile::parse(&with_ac130(4.0).replace("79000401", "79000105")).is_err(), "outside the range");
        assert!(RowsFile::parse(&with_ac130(4.0).replace("79000303", "79000302")).is_err(), "duplicate");
    }

    #[test]
    fn regulation_inv_er_reproduces_targets() {
        // the design table (s 1.225, d_ref 85): MW2 40 -> atkPhys 103 -> ER ~49.3 on c4311
        assert_eq!(planned_atk(40.0, 1.225, 85.0), 103);
        for mw2 in [15.0, 20.0, 30.0, 35.0, 40.0, 55.0, 70.0, 75.0, 135.0, 1000.0] {
            let atk = planned_atk(mw2, 1.225, 85.0) as f64;
            let er = crate::ac130_dmg::er_damage(atk, 85.0);
            // rounding atkPhys to an integer moves the ER damage by < 0.5 x slope (<= 0.9)
            assert!((er - 1.225 * mw2).abs() <= 0.5, "mw2 {mw2}: atk {atk} -> {er}");
        }
        // ratios between guns are kept on d_ref (40 vs 30: 4/3)
        let a = crate::ac130_dmg::er_damage(planned_atk(40.0, 1.225, 85.0) as f64, 85.0);
        let b = crate::ac130_dmg::er_damage(planned_atk(30.0, 1.225, 85.0) as f64, 85.0);
        // (integer atkPhys: within the rounding of one ER point per row)
        assert!(((a / b) / (4.0 / 3.0) - 1.0).abs() < 0.03, "{}", a / b);
        assert_eq!(arsenal_row_id(79_100_000, 12, 0, 5), 79_112_005);
        assert_eq!(arsenal_row_id(79_100_000, 51, 6, 2), 79_151_602);
    }

    pub(crate) fn with_arsenal() -> String {
        let bands: Vec<String> = [40, 39, 38]
            .iter()
            .enumerate()
            .map(|(i, d)| format!(r#"{{"band":{i},"id":{},"mw2":{d},"atk":{}}}"#, 79_112_000 + i, planned_atk(*d as f64, 1.225, 85.0)))
            .collect();
        let block = format!(
            r#""arsenal":{{"mode":"flat_inv","s":1.225,"d_ref":85.0,"id_base":79100000,"bullet_sfx":"none","guns":{{
                "ump45_mp":{{"g":12,"class":"smg","bands":[{}]}},
                "at4_mp":{{"g":51,"class":"launcher","launcher":{{"impact":{{"band":0,"id":79151500,"mw2":1000,"atk":1361}},"marker_id":79151501,
                   "core":{{"id":79151601,"radius_m":5.08,"mw2":50,"atk":116}},"ring":{{"id":79151602,"radius_m":10.16,"mw2":80,"atk":154}},
                   "single":{{"id":79151603,"radius_m":10.16,"mw2":105,"atk":187}}}}}}}}}}"#,
            bands.join(",")
        );
        let s = sample(4.0);
        let cut = s.rfind('}').unwrap();
        format!("{},{}}}", &s[..cut], block)
    }

    #[test]
    fn regulation_arsenal_block() {
        let f = RowsFile::parse(&with_arsenal()).unwrap();
        let a = f.arsenal.as_ref().unwrap();
        let u = &a.guns["ump45_mp"];
        assert_eq!(u.band_for_damage(40).unwrap().id, 79_112_000);
        assert_eq!(u.band_for_damage(10).unwrap().id, 79_112_002);
        assert_eq!(a.ids().len(), 3 + 5);
        assert_eq!(f.all_ids().len(), 7, "the legacy id set is unchanged");
        // broken atk / scheme / duplicate g rejected
        let bad = with_arsenal().replace(&format!("\"atk\":{}}}", planned_atk(40.0, 1.225, 85.0)), "\"atk\":999}");
        assert!(RowsFile::parse(&bad).is_err());
        assert!(RowsFile::parse(&with_arsenal().replace("79112001", "79112011")).is_err());
        assert!(RowsFile::parse(&with_arsenal().replace("\"g\":51", "\"g\":12")).is_err());
        assert!(RowsFile::parse(&with_arsenal().replace("flat_inv", "linear")).is_err());
    }

    #[test]
    fn rows_ac130_object_fields() {
        // after run 9: atk_obj / obj_bushes / sfx_posture per row; an older rows.json has none (None)
        let old = RowsFile::parse(&with_ac130(4.0)).unwrap();
        let r = old.ac130.as_ref().unwrap().gun("105").unwrap().core.unwrap();
        assert_eq!((r.atk_obj, r.obj_bushes, r.sfx_posture), (None, None, None));
        let s = with_ac130(4.0).replace(r#""sfx":524023}"#, r#""sfx":524028,"atk_obj":200,"obj_bushes":1,"sfx_posture":1}"#);
        let f = RowsFile::parse(&s).unwrap();
        let g = f.ac130.as_ref().unwrap().gun("105").unwrap();
        assert_eq!((g.core.unwrap().atk_obj, g.core.unwrap().obj_bushes, g.core.unwrap().sfx_posture), (Some(200), Some(1), Some(1)));
        assert_eq!(g.ring.unwrap().atk_obj, None);
    }
}
