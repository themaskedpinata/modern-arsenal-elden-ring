//! Model of `converted\weapons.json` (written by `tools/scripts/convert_weapons.py`).
//!
//! The `mw2` object keeps the original WEAPONFILE key names (camelCase) with MW2 units (inches,
//! seconds, degrees); the `si` object holds the derived metric values. [`WeaponSpec`] is the
//! validated, plain-number view the simulation uses. No MW2 values are compiled into the DLL:
//! everything here is read from the JSON at runtime.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::units::INCH_TO_M;

/// The whole weapons.json file.
#[derive(Clone, Debug, Deserialize)]
pub struct WeaponsFile {
    pub schema: u32,
    #[serde(default)]
    pub generator: String,
    pub inch_to_m: f64,
    pub weapons: BTreeMap<String, WeaponEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WeaponEntry {
    pub mw2: Mw2Weapon,
    pub si: SiWeapon,
}

/// MW2 values under their WEAPONFILE names (unknown extra keys are ignored).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mw2Weapon {
    #[serde(default)]
    pub display_name: String,
    pub fire_type: String,
    pub damage: f64,
    pub min_damage: f64,
    pub max_damage_range: f64,
    pub min_damage_range: f64,
    pub fire_time: f64,
    pub clip_size: f64,
    pub max_ammo: f64,
    #[serde(default)]
    pub start_ammo: f64,
    pub reload_time: f64,
    pub reload_empty_time: f64,
    #[serde(default)]
    pub reload_add_time: f64,
    pub hip_spread_stand_min: f64,
    pub hip_spread_max: f64,
    pub hip_spread_decay_rate: f64,
    pub hip_spread_fire_add: f64,
    #[serde(default)]
    pub hip_spread_move_add: f64,
    #[serde(default)]
    pub hip_spread_turn_add: f64,
    #[serde(default)]
    pub ads_spread: f64,
    pub reticle_side_size: f64,
    #[serde(default)]
    pub reticle_center_size: f64,
    pub low_ammo_warning_threshold: f64,
    #[serde(default = "one")]
    pub shot_count: f64,
    #[serde(default)]
    pub penetrate_type: String,
    #[serde(default = "one")]
    pub loc_head: f64,
    #[serde(default = "one")]
    pub loc_helmet: f64,
    #[serde(default)]
    pub fire_sound_player: String,
}

fn one() -> f64 {
    1.0
}

/// Derived metric values.
#[derive(Clone, Debug, Deserialize)]
pub struct SiWeapon {
    #[serde(rename = "maxDamageRange_m")]
    pub max_damage_range_m: f64,
    #[serde(rename = "minDamageRange_m")]
    pub min_damage_range_m: f64,
    #[serde(rename = "fireRate_rpm")]
    pub fire_rate_rpm: f64,
}

/// IW4 `fireType` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireMode {
    FullAuto,
    SingleShot,
    Burst(u32),
    /// M6: the Ranger (`Double Barrel`): one shot per press, two barrels per clip (docs/M6_DESIGN.md section 6.1).
    DoubleBarrel,
}

impl FireMode {
    pub fn parse(s: &str) -> Result<FireMode, String> {
        match s.trim() {
            "Full Auto" => Ok(FireMode::FullAuto),
            "Single Shot" => Ok(FireMode::SingleShot),
            "2-Round Burst" => Ok(FireMode::Burst(2)),
            "3-Round Burst" => Ok(FireMode::Burst(3)),
            "4-Round Burst" => Ok(FireMode::Burst(4)),
            other => Err(format!("unknown fireType {other:?}")),
        }
    }

    pub fn label(self) -> String {
        match self {
            FireMode::FullAuto => "Full Auto".into(),
            FireMode::SingleShot => "Single Shot".into(),
            FireMode::Burst(n) => format!("{n}-Round Burst"),
            FireMode::DoubleBarrel => "Double Barrel".into(),
        }
    }
}

/// Validated plain numbers the simulation needs for one gun.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponSpec {
    pub name: String,
    pub fire_mode: FireMode,
    pub damage: i32,
    pub min_damage: i32,
    pub max_damage_range_m: f64,
    pub min_damage_range_m: f64,
    pub fire_time_s: f64,
    pub clip_size: u32,
    pub max_ammo: u32,
    pub reload_s: f64,
    pub reload_empty_s: f64,
    pub spread_min_deg: f32,
    pub spread_max_deg: f32,
    pub spread_decay_per_s: f32,
    pub spread_fire_add: f32,
    pub spread_move_add: f32,
    pub reticle_side_size: f32,
    pub low_ammo_threshold: f32,
}

impl WeaponSpec {
    /// Shots left in the magazine at or below which the ammo counter turns red
    /// (`floor(clipSize * lowAmmoWarningThreshold)`, 10 for the UMP45).
    pub fn low_ammo_count(&self) -> u32 {
        (self.clip_size as f32 * self.low_ammo_threshold).floor().max(0.0) as u32
    }
}

fn int_field(name: &str, v: f64) -> Result<i64, String> {
    if v.is_finite() && v.fract() == 0.0 {
        Ok(v as i64)
    } else {
        Err(format!("{name}={v} is not an integer"))
    }
}

impl WeaponsFile {
    pub fn parse(text: &str) -> Result<WeaponsFile, String> {
        let f: WeaponsFile = serde_json::from_str(text).map_err(|e| format!("weapons.json: {e}"))?;
        if f.schema != 1 {
            return Err(format!("weapons.json: unsupported schema {}", f.schema));
        }
        if (f.inch_to_m - INCH_TO_M).abs() > 1e-12 {
            return Err(format!("weapons.json: inch_to_m {} != {INCH_TO_M}", f.inch_to_m));
        }
        Ok(f)
    }

    /// Validated spec for one weapon.
    pub fn spec(&self, name: &str) -> Result<WeaponSpec, String> {
        let e = self.weapons.get(name).ok_or_else(|| format!("weapon {name} not in weapons.json"))?;
        WeaponSpec::from_entry(name, e)
    }
}

impl WeaponSpec {
    pub fn from_entry(name: &str, e: &WeaponEntry) -> Result<WeaponSpec, String> {
        let m = &e.mw2;
        let err = |s: String| format!("{name}: {s}");
        let fire_mode = FireMode::parse(&m.fire_type).map_err(err)?;
        let damage = int_field("damage", m.damage).map_err(err)?;
        let min_damage = int_field("minDamage", m.min_damage).map_err(err)?;
        let clip = int_field("clipSize", m.clip_size).map_err(err)?;
        let max_ammo = int_field("maxAmmo", m.max_ammo).map_err(err)?;
        if !(1..=10_000).contains(&damage) || !(1..=damage).contains(&min_damage) {
            return Err(err(format!("damage {damage}/{min_damage} out of range")));
        }
        if !(m.max_damage_range > 0.0 && m.min_damage_range >= m.max_damage_range) {
            return Err(err(format!(
                "ranges {}/{} invalid",
                m.max_damage_range, m.min_damage_range
            )));
        }
        if !(m.fire_time > 0.0 && m.fire_time < 10.0) {
            return Err(err(format!("fireTime {} invalid", m.fire_time)));
        }
        if !(1..=1000).contains(&clip) || !(0..=100_000).contains(&max_ammo) {
            return Err(err(format!("clip {clip} / maxAmmo {max_ammo} invalid")));
        }
        if !(m.reload_time > 0.0 && m.reload_empty_time > 0.0) {
            return Err(err("reload times must be > 0".into()));
        }
        if !(m.hip_spread_stand_min >= 0.0 && m.hip_spread_max >= m.hip_spread_stand_min && m.hip_spread_max < 90.0) {
            return Err(err("hip spread min/max invalid".into()));
        }
        if m.hip_spread_decay_rate < 0.0 || m.hip_spread_fire_add < 0.0 || m.hip_spread_move_add < 0.0 {
            return Err(err("hip spread rates must be >= 0".into()));
        }
        // The si block must be exactly the mw2 ranges converted with 0.0254 m/inch.
        let (smax, smin) = (m.max_damage_range * INCH_TO_M, m.min_damage_range * INCH_TO_M);
        if (e.si.max_damage_range_m - smax).abs() > 1e-6 || (e.si.min_damage_range_m - smin).abs() > 1e-6 {
            return Err(err("si ranges do not match mw2 ranges x 0.0254".into()));
        }
        Ok(WeaponSpec {
            name: name.to_string(),
            fire_mode,
            damage: damage as i32,
            min_damage: min_damage as i32,
            max_damage_range_m: smax,
            min_damage_range_m: smin,
            fire_time_s: m.fire_time,
            clip_size: clip as u32,
            max_ammo: max_ammo as u32,
            reload_s: m.reload_time,
            reload_empty_s: m.reload_empty_time,
            spread_min_deg: m.hip_spread_stand_min as f32,
            spread_max_deg: m.hip_spread_max as f32,
            spread_decay_per_s: m.hip_spread_decay_rate as f32,
            spread_fire_add: m.hip_spread_fire_add as f32,
            spread_move_add: m.hip_spread_move_add as f32,
            reticle_side_size: m.reticle_side_size as f32,
            low_ammo_threshold: m.low_ammo_warning_threshold as f32,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic weapons.json (made-up gun "testgun_mp"; real values are only read from disk).
    pub(crate) const SAMPLE: &str = r#"{
      "schema": 1, "generator": "test", "inch_to_m": 0.0254,
      "source": {"testgun_mp": {"file": "x", "sha256": "0", "fields": 671}},
      "weapons": {"testgun_mp": {
        "mw2": {"displayName": "WEAPON_TEST", "fireType": "Full Auto", "damage": 50, "minDamage": 30,
                "maxDamageRange": 1000, "minDamageRange": 2000, "fireTime": 0.1, "clipSize": 20,
                "maxAmmo": 100, "startAmmo": 60, "reloadTime": 2.0, "reloadEmptyTime": 3.0,
                "reloadAddTime": 1.0, "hipSpreadStandMin": 3, "hipSpreadMax": 6,
                "hipSpreadDecayRate": 5, "hipSpreadFireAdd": 0.5, "hipSpreadMoveAdd": 3,
                "hipSpreadTurnAdd": 0, "adsSpread": 0, "reticleSideSize": 6, "reticleCenterSize": 4,
                "lowAmmoWarningThreshold": 0.25, "shotCount": 1, "penetrateType": "small",
                "locHead": 1.5, "locHelmet": 1.5, "fireSoundPlayer": "test_fire", "someOtherKey": 7},
        "si": {"maxDamageRange_m": 25.4, "minDamageRange_m": 50.8, "fireRate_rpm": 600.0}}}
    }"#;

    #[test]
    fn weapons_json_parses_and_validates() {
        let f = WeaponsFile::parse(SAMPLE).unwrap();
        let s = f.spec("testgun_mp").unwrap();
        assert_eq!(s.fire_mode, FireMode::FullAuto);
        assert_eq!((s.damage, s.min_damage, s.clip_size, s.max_ammo), (50, 30, 20, 100));
        assert!((s.max_damage_range_m - 25.4).abs() < 1e-9);
        assert!((s.min_damage_range_m - 50.8).abs() < 1e-9);
        assert_eq!(s.low_ammo_count(), 5);
        assert!(f.spec("nope").is_err());
    }

    #[test]
    fn weapons_json_rejects_inconsistent_data() {
        let bad_si = SAMPLE.replace("\"maxDamageRange_m\": 25.4", "\"maxDamageRange_m\": 25.0");
        assert!(WeaponsFile::parse(&bad_si).unwrap().spec("testgun_mp").is_err());
        let bad_dmg = SAMPLE.replace("\"minDamage\": 30", "\"minDamage\": 60");
        assert!(WeaponsFile::parse(&bad_dmg).unwrap().spec("testgun_mp").is_err());
        let bad_mode = SAMPLE.replace("Full Auto", "Pump");
        assert!(WeaponsFile::parse(&bad_mode).unwrap().spec("testgun_mp").is_err());
        let bad_schema = SAMPLE.replace("\"schema\": 1", "\"schema\": 2");
        assert!(WeaponsFile::parse(&bad_schema).is_err());
        assert!(WeaponsFile::parse("{").is_err());
    }

    #[test]
    fn weapons_fire_modes() {
        assert_eq!(FireMode::parse("Single Shot").unwrap(), FireMode::SingleShot);
        assert_eq!(FireMode::parse("3-Round Burst").unwrap(), FireMode::Burst(3));
        assert_eq!(FireMode::Burst(3).label(), "3-Round Burst");
    }
}
