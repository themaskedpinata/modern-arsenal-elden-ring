//! M6: the MW2 arsenal data model (docs/M6_DESIGN.md section 4.1).
//!
//! `converted\guns\<name>\stats.json` (written by `tools/scripts/convert_arsenal.py`) holds the weapon-file numbers
//! under their original names; [`GunSpec`] is the validated plain view the simulation uses. No MW2 value is
//! compiled into the DLL: everything here is read at runtime (the tests use made-up guns).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::falloff::Falloff;
use crate::units::INCH_TO_M;
use crate::weapons::FireMode;

pub const STATS_SCHEMA: u32 = 1;

/// The whole `stats.json` of one gun.
#[derive(Clone, Debug, Deserialize)]
pub struct StatsFile {
    pub schema: u32,
    pub name: String,
    pub g: u32,
    pub class: String,
    pub display: String,
    #[serde(rename = "fireType")]
    pub fire_type: String,
    #[serde(rename = "weaponClass")]
    pub weapon_class: String,
    #[serde(default, rename = "weaponType")]
    pub weapon_type: String,
    #[serde(default, rename = "guidedMissileType")]
    pub guided_missile_type: String,
    #[serde(default, rename = "projExplosionType")]
    pub proj_explosion_type: String,
    #[serde(default, rename = "rocketModel")]
    pub rocket_model: String,
    pub move_class: String,
    pub num: BTreeMap<String, f64>,
    #[serde(default)]
    pub overlay: Option<OverlayInfo>,
    #[serde(default)]
    pub tags: BTreeMap<String, bool>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OverlayInfo {
    pub material: String,
    pub file: String,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
}

/// Weapon class of the user's list (`class` in stats.json; the regulation id scheme groups by it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GunClass {
    Rifle,
    Smg,
    Lmg,
    Sniper,
    Shotgun,
    Launcher,
}

impl GunClass {
    pub fn parse(s: &str) -> Result<GunClass, String> {
        Ok(match s {
            "ar" => GunClass::Rifle,
            "smg" => GunClass::Smg,
            "lmg" => GunClass::Lmg,
            "sniper" => GunClass::Sniper,
            "shotgun" => GunClass::Shotgun,
            "launcher" => GunClass::Launcher,
            o => return Err(format!("unknown class {o:?}")),
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            GunClass::Rifle => "ar",
            GunClass::Smg => "smg",
            GunClass::Lmg => "lmg",
            GunClass::Sniper => "sniper",
            GunClass::Shotgun => "shotgun",
            GunClass::Launcher => "launcher",
        }
    }
}

/// MW2 MP `updateMoveSpeedScale` classes (docs/M6_DESIGN.md section 7) and their scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveClass {
    Rifle,
    Mg,
    Smg,
    Spread,
    Launcher,
    Sniper,
    Other,
}

impl MoveClass {
    pub fn parse(s: &str) -> MoveClass {
        match s {
            "rifle" => MoveClass::Rifle,
            "mg" => MoveClass::Mg,
            "smg" => MoveClass::Smg,
            "spread" => MoveClass::Spread,
            "launcher" => MoveClass::Launcher,
            "sniper" => MoveClass::Sniper,
            _ => MoveClass::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MoveClass::Rifle => "rifle",
            MoveClass::Mg => "mg",
            MoveClass::Smg => "smg",
            MoveClass::Spread => "spread",
            MoveClass::Launcher => "launcher",
            MoveClass::Sniper => "sniper",
            MoveClass::Other => "other",
        }
    }

    /// `_weapons.gsc updateMoveSpeedScale` (MP): rifle 0.95, mg 0.875, smg 1.0, spread 0.95, rocketlauncher 0.80,
    /// sniper 1.0, anything else 1.0.
    pub fn scale(self) -> f32 {
        match self {
            MoveClass::Rifle | MoveClass::Spread => 0.95,
            MoveClass::Mg => 0.875,
            MoveClass::Launcher => 0.80,
            MoveClass::Smg | MoveClass::Sniper | MoveClass::Other => 1.0,
        }
    }
}

/// How a gun reloads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReloadKind {
    /// Whole magazine, added at the end (M2 behaviour). `empty_s` = `reload_s` when the file has 0 (launchers).
    Mag { reload_s: f64, empty_s: f64 },
    /// Shell by shell (`segmentedReload 1`): start, n loops, end (docs/M6_DESIGN.md section 6.3).
    Segmented { start_s: f64, start_add: u32, start_add_s: f64, loop_s: f64, add: u32, add_s: f64, end_s: f64 },
}

/// Bolt / pump / lever rechamber (only `boltAction == 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rechamber {
    pub time_s: f64,
    pub while_ads: bool,
}

/// Hip spread of the weapon file (degrees) + ADS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HipSpread {
    pub stand: [f32; 2],
    pub ducked: [f32; 2],
    pub prone: [f32; 2],
    pub fire_add: f32,
    pub decay_per_s: f32,
    pub move_add: f32,
    pub ducked_decay: f32,
    pub prone_decay: f32,
    pub ads_spread: f32,
}

/// View kick (deg/s impulses, deg/s^2 recentre) and gun kick numbers, hip and ADS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KickParams {
    /// [pitch min, pitch max, yaw min, yaw max]
    pub hip: [f32; 4],
    pub ads: [f32; 4],
    pub hip_center: f32,
    pub ads_center: f32,
    pub hip_gun: [f32; 4],
    pub ads_gun: [f32; 4],
}

/// ADS of one gun.
#[derive(Clone, Debug, PartialEq)]
pub struct AdsParams {
    pub zoom_fov: f32,
    pub trans_in_s: f32,
    pub trans_out_s: f32,
    pub idle_amount: f32,
    pub idle_speed: f32,
    pub overlay: Option<OverlayInfo>,
}

/// Lock-on rule of a launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    None,
    /// `lockonSupported` without `requireLockonToFire` (AT4): dumbfire.
    Supported,
    /// Stinger: lock a target before firing.
    Required,
    /// Javelin: point or target lock, top attack.
    RequiredTopAttack,
}

/// A launcher's projectile (inches / seconds as in the weapon file; SI helpers below).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileSpec {
    pub speed_in: f32,
    pub speed_up_in: f32,
    pub lifetime_s: f32,
    pub ignition_s: f32,
    pub activate_dist_in: f32,
    /// Grenade launcher (M79): ballistic flight with arming.
    pub ballistic: bool,
    pub radius_in: f32,
    pub inner: f32,
    pub outer: f32,
    pub impact_damage: f32,
    pub max_steering_accel_in: f32,
    pub lock: LockKind,
}

impl ProjectileSpec {
    pub fn speed_mps(&self) -> f32 {
        self.speed_in * INCH_TO_M as f32
    }
    pub fn radius_m(&self) -> f32 {
        self.radius_in * INCH_TO_M as f32
    }
    pub fn arm_m(&self) -> f32 {
        self.activate_dist_in * INCH_TO_M as f32
    }
    pub fn max_accel_mps2(&self) -> f32 {
        self.max_steering_accel_in * INCH_TO_M as f32
    }
}

/// Validated plain numbers of one gun (section 4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct GunSpec {
    pub name: String,
    pub g: u32,
    pub class: GunClass,
    pub display: String,
    pub fire: FireMode,
    pub fire_time_s: f64,
    pub clip: u32,
    pub start_ammo: u32,
    pub max_ammo: u32,
    pub reload: ReloadKind,
    pub rechamber: Option<Rechamber>,
    pub pellets: u32,
    /// The falloff of a BODY hit: the weapon file's damage / minDamage x `body_mult` (review M6, see body_mult).
    pub falloff: Falloff,
    /// ER has no hit locations: every hit is a body hit at the weapon file's `locTorsoUpper` multiplier when it is > 1
    /// (Intervention / Barrett / WA2000 1.5 -> 105, M21 1.1 -> 77; every other gun 1.0). M6_DESIGN deviation 15.
    pub body_mult: f64,
    pub loc_head: f32,
    pub spread: HipSpread,
    pub kick: KickParams,
    pub ads: AdsParams,
    pub move_class: MoveClass,
    pub projectile: Option<ProjectileSpec>,
    pub raise_s: f64,
    pub drop_s: f64,
    pub first_raise_s: f64,
    pub sprint_in_s: f64,
    pub sprint_out_s: f64,
    pub low_ammo_threshold: f32,
    pub reticle_side: f32,
    pub show_rocket_s: f64,
    pub has_rocket_part: bool,
}

/// A missing number reads as 0 (every key the converter writes is a weapon-file field; tests use small maps).
fn n(m: &BTreeMap<String, f64>, k: &str) -> f64 {
    m.get(k).copied().filter(|v| v.is_finite()).unwrap_or(0.0)
}

fn nf(m: &BTreeMap<String, f64>, k: &str) -> f32 {
    n(m, k) as f32
}

fn count(m: &BTreeMap<String, f64>, k: &str, max: f64) -> Result<u32, String> {
    let v = n(m, k);
    if v.fract() != 0.0 || !(0.0..=max).contains(&v) {
        return Err(format!("{k}={v} is not an integer in 0..={max}"));
    }
    Ok(v as u32)
}

/// The body-hit multiplier (review M6 fidelity): `locTorsoUpper` when > 1, else 1 (launchers: always 1, their impact /
/// explosion damage has no hit location). Mirror of build_regulation.py `body_mult`.
pub fn body_mult(m: &BTreeMap<String, f64>, launcher: bool) -> f64 {
    let v = n(m, "locTorsoUpper");
    if launcher || !(v > 1.0 && v < 10.0) { 1.0 } else { v }
}

/// MW2 damage x the body multiplier, rounded half away from zero (Python `rhaz`).
pub fn body_damage(d: i32, mult: f64) -> i32 {
    if mult == 1.0 { d } else { (d as f64 * mult).round() as i32 }
}

impl FireMode {
    /// IW4 `fireType` incl. the M6 additions (`Double Barrel`).
    pub fn parse_m6(s: &str) -> Result<FireMode, String> {
        match s.trim() {
            "Double Barrel" => Ok(FireMode::DoubleBarrel),
            o => FireMode::parse(o),
        }
    }
}

impl GunSpec {
    pub fn from_stats(s: &StatsFile) -> Result<GunSpec, String> {
        let err = |e: String| format!("{}: {e}", s.name);
        if s.schema != STATS_SCHEMA {
            return Err(err(format!("stats.json schema {} (want {STATS_SCHEMA})", s.schema)));
        }
        let m = &s.num;
        let class = GunClass::parse(&s.class).map_err(err)?;
        let fire = FireMode::parse_m6(&s.fire_type).map_err(err)?;
        let fire_time_s = n(m, "fireTime");
        if !(fire_time_s > 0.0 && fire_time_s < 10.0) {
            return Err(err(format!("fireTime {fire_time_s} invalid")));
        }
        let clip = count(m, "clipSize", 1000.0).map_err(err)?;
        let start_ammo = count(m, "startAmmo", 100_000.0).map_err(err)?;
        let max_ammo = count(m, "maxAmmo", 100_000.0).map_err(err)?;
        if clip == 0 {
            return Err(err("clipSize 0".into()));
        }
        let launcher = class == GunClass::Launcher;
        let damage = count(m, "damage", 10_000.0).map_err(err)? as i32;
        let min_damage = count(m, "minDamage", 10_000.0).map_err(err)? as i32;
        // section 4.1: minDamage >= 1 only for non-launchers; launchers need their explosion fields instead
        if !launcher && !(damage >= 1 && (1..=damage).contains(&min_damage)) {
            return Err(err(format!("damage {damage}/{min_damage} out of range")));
        }
        let (max_r, min_r) = (n(m, "maxDamageRange"), n(m, "minDamageRange"));
        if !launcher && !(max_r > 0.0 && min_r >= max_r) {
            return Err(err(format!("ranges {max_r}/{min_r} invalid")));
        }
        let reload_s = n(m, "reloadTime");
        let empty_raw = n(m, "reloadEmptyTime");
        if !(reload_s > 0.0) {
            return Err(err("reloadTime must be > 0".into()));
        }
        // section 4.1: reloadEmptyTime > 0 only for non-launchers; launchers use reloadTime for both
        if !launcher && !(empty_raw > 0.0) && n(m, "segmentedReload") != 1.0 {
            return Err(err("reloadEmptyTime must be > 0".into()));
        }
        let empty_s = if empty_raw > 0.0 { empty_raw } else { reload_s };
        let reload = if n(m, "segmentedReload") == 1.0 {
            let start_s = n(m, "reloadStartTime");
            let end_s = n(m, "reloadEndTime");
            if !(start_s >= 0.0 && end_s >= 0.0 && start_s + end_s < 30.0) {
                return Err(err("segmented reload start/end times invalid".into()));
            }
            ReloadKind::Segmented {
                start_s,
                start_add: count(m, "reloadStartAdd", 100.0).map_err(err)?,
                start_add_s: n(m, "reloadStartAddTime").clamp(0.0, start_s),
                loop_s: reload_s,
                add: count(m, "reloadAmmoAdd", 100.0).map_err(err)?.max(1),
                // section 6.3: the add time is clamped to the loop end (SPAS 1.0 > loop 0.667)
                add_s: n(m, "reloadAddTime").clamp(0.0, reload_s),
                end_s,
            }
        } else {
            ReloadKind::Mag { reload_s, empty_s }
        };
        let rechamber = (n(m, "boltAction") == 1.0 && n(m, "rechamberTime") > 0.0)
            .then(|| Rechamber { time_s: n(m, "rechamberTime"), while_ads: n(m, "rechamberWhileAds") != 0.0 });
        let pellets = count(m, "shotCount", 64.0).map_err(err)?.max(1);
        let spread = HipSpread {
            stand: [nf(m, "hipSpreadStandMin"), nf(m, "hipSpreadMax")],
            ducked: [nf(m, "hipSpreadDuckedMin"), nf(m, "hipSpreadDuckedMax")],
            prone: [nf(m, "hipSpreadProneMin"), nf(m, "hipSpreadProneMax")],
            fire_add: nf(m, "hipSpreadFireAdd"),
            decay_per_s: nf(m, "hipSpreadDecayRate"),
            move_add: nf(m, "hipSpreadMoveAdd"),
            ducked_decay: nf(m, "hipSpreadDuckedDecay").max(0.0),
            prone_decay: nf(m, "hipSpreadProneDecay").max(0.0),
            ads_spread: nf(m, "adsSpread").max(0.0),
        };
        if !(spread.stand[0] >= 0.0 && spread.stand[1] >= spread.stand[0] && spread.stand[1] < 90.0) {
            return Err(err("hip spread min/max invalid".into()));
        }
        let k4 = |p: &str| [nf(m, &format!("{p}PitchMin")), nf(m, &format!("{p}PitchMax")), nf(m, &format!("{p}YawMin")), nf(m, &format!("{p}YawMax"))];
        let kick = KickParams {
            hip: k4("hipViewKick"),
            ads: k4("adsViewKick"),
            hip_center: nf(m, "hipViewKickCenterSpeed").max(1.0),
            ads_center: nf(m, "adsViewKickCenterSpeed").max(1.0),
            hip_gun: k4("hipGunKick"),
            ads_gun: k4("adsGunKick"),
        };
        let ads = AdsParams {
            zoom_fov: nf(m, "adsZoomFov"),
            trans_in_s: nf(m, "adsTransInTime").max(0.01),
            trans_out_s: nf(m, "adsTransOutTime").max(0.01),
            idle_amount: nf(m, "adsIdleAmount"),
            idle_speed: nf(m, "adsIdleSpeed"),
            overlay: s.overlay.clone(),
        };
        let projectile = if launcher {
            let lock = if n(m, "requireLockonToFire") != 0.0 {
                if s.name.starts_with("javelin") || s.guided_missile_type.eq_ignore_ascii_case("javelin") {
                    LockKind::RequiredTopAttack
                } else {
                    LockKind::Required
                }
            } else if n(m, "lockonSupported") != 0.0 {
                LockKind::Supported
            } else {
                LockKind::None
            };
            let p = ProjectileSpec {
                speed_in: nf(m, "projectileSpeed"),
                speed_up_in: nf(m, "projectileSpeedUp"),
                lifetime_s: nf(m, "projectileLifetime"),
                ignition_s: nf(m, "projIgnitionDelay") / 1000.0,
                activate_dist_in: nf(m, "projectileActivateDist"),
                ballistic: s.weapon_class == "grenade" || s.proj_explosion_type == "grenade",
                radius_in: nf(m, "explosionRadius"),
                inner: nf(m, "explosionInnerDamage"),
                outer: nf(m, "explosionOuterDamage"),
                impact_damage: damage as f32,
                max_steering_accel_in: nf(m, "maxSteeringAccel"),
                lock,
            };
            if !(p.speed_in > 0.0 && p.radius_in > 0.0 && p.inner > 0.0 && p.lifetime_s > 0.0) {
                return Err(err(format!("launcher needs speed/radius/inner/lifetime > 0 ({p:?})")));
            }
            Some(p)
        } else {
            None
        };
        let body_mult = body_mult(m, launcher);
        let (damage, min_damage) = if launcher {
            (damage, damage)
        } else {
            (body_damage(damage, body_mult), body_damage(min_damage, body_mult))
        };
        let falloff = Falloff {
            damage,
            min_damage,
            max_damage_range_m: max_r * INCH_TO_M,
            min_damage_range_m: min_r * INCH_TO_M,
        };
        let raise_s = n(m, "raiseTime");
        let drop_s = n(m, "dropTime");
        if !(raise_s > 0.0 && drop_s > 0.0 && raise_s < 10.0 && drop_s < 10.0) {
            return Err(err(format!("raiseTime/dropTime {raise_s}/{drop_s} invalid")));
        }
        let first = n(m, "firstRaiseTime");
        Ok(GunSpec {
            name: s.name.clone(),
            g: s.g,
            class,
            display: if s.display.trim().is_empty() { s.name.clone() } else { s.display.clone() },
            fire,
            fire_time_s,
            clip,
            start_ammo,
            max_ammo,
            reload,
            rechamber,
            pellets,
            falloff,
            body_mult,
            loc_head: nf(m, "locHead"),
            spread,
            kick,
            ads,
            move_class: MoveClass::parse(&s.move_class),
            projectile,
            raise_s,
            drop_s,
            first_raise_s: if first > 0.0 { first } else { raise_s },
            sprint_in_s: n(m, "sprintInTime"),
            sprint_out_s: n(m, "sprintOutTime"),
            low_ammo_threshold: nf(m, "lowAmmoWarningThreshold"),
            reticle_side: nf(m, "reticleSideSize"),
            show_rocket_s: n(m, "reloadShowRocketTime"),
            has_rocket_part: !s.rocket_model.trim().is_empty(),
        })
    }

    pub fn is_launcher(&self) -> bool {
        self.projectile.is_some()
    }

    pub fn is_shotgun(&self) -> bool {
        self.class == GunClass::Shotgun
    }

    /// Ammo below which the HUD counter turns red (MW2 style, docs/M6_DESIGN.md section 11 [I]).
    pub fn low_ammo_count(&self) -> u32 {
        (self.clip as f32 * self.low_ammo_threshold).floor().max(0.0) as u32
    }

    /// Reserve after a refill (`[arsenal] reserve_source`): "max" = maxAmmo (the verified UMP45 behaviour) or
    /// "start" = startAmmo - clip (MW2 MP spawn) [I].
    pub fn refill_reserve(&self, source: ReserveSource) -> u32 {
        match source {
            ReserveSource::Max => self.max_ammo,
            ReserveSource::Start => self.start_ammo.saturating_sub(self.clip),
        }
    }

    /// MW2 damage of the hitscan bands (one per falloff band, as M2's band rule).
    pub fn band_damages(&self) -> Vec<i32> {
        crate::falloff::band_damages(self.falloff.damage, self.falloff.min_damage)
    }
}

/// `[arsenal] reserve_source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReserveSource {
    Max,
    Start,
}

impl StatsFile {
    pub fn parse(text: &str) -> Result<StatsFile, String> {
        serde_json::from_str(text).map_err(|e| format!("stats.json: {e}"))
    }
}

/// One clip role of `clips.json`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ClipRole {
    pub clip: String,
    pub frames: u32,
    pub fps: u32,
    pub duration: f64,
    pub looped: bool,
    #[serde(default)]
    pub time_field: Option<String>,
    pub time: f64,
    pub rate: f64,
    #[serde(default)]
    pub notes: Vec<ClipNote>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ClipNote {
    pub name: String,
    pub frame: u32,
    pub t_frac: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClipsFile {
    pub schema: u32,
    pub gun: String,
    pub roles: BTreeMap<String, ClipRole>,
}

impl ClipsFile {
    pub fn parse(text: &str) -> Result<ClipsFile, String> {
        let f: ClipsFile = serde_json::from_str(text).map_err(|e| format!("clips.json: {e}"))?;
        for (r, c) in &f.roles {
            if !(c.rate.is_finite() && c.rate > 0.0) {
                return Err(format!("clips.json: role {r} rate {}", c.rate));
            }
        }
        Ok(f)
    }
}

/// `converted\guns\index.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct IndexFile {
    pub schema: u32,
    pub guns: Vec<IndexEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    #[serde(default)]
    pub g: Option<u32>,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub problems: Vec<String>,
}

impl IndexFile {
    pub fn parse(text: &str) -> Result<IndexFile, String> {
        serde_json::from_str(text).map_err(|e| format!("index.json: {e}"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A made-up gun (not MW2 data): full auto, 30 rounds, 40..30 damage.
    pub(crate) fn stats_json(name: &str, class: &str, fire: &str, extra: &[(&str, f64)]) -> String {
        let mut num: BTreeMap<&str, f64> = [
            ("fireTime", 0.1),
            ("clipSize", 30.0),
            ("startAmmo", 90.0),
            ("maxAmmo", 180.0),
            ("damage", 40.0),
            ("minDamage", 30.0),
            ("maxDamageRange", 1000.0),
            ("minDamageRange", 2000.0),
            ("reloadTime", 2.0),
            ("reloadEmptyTime", 2.5),
            ("hipSpreadStandMin", 3.0),
            ("hipSpreadMax", 6.0),
            ("hipSpreadDuckedMin", 2.5),
            ("hipSpreadDuckedMax", 5.0),
            ("hipSpreadProneMin", 2.0),
            ("hipSpreadProneMax", 4.0),
            ("hipSpreadFireAdd", 0.5),
            ("hipSpreadDecayRate", 4.0),
            ("hipSpreadMoveAdd", 3.0),
            ("hipViewKickPitchMin", 10.0),
            ("hipViewKickPitchMax", 60.0),
            ("hipViewKickYawMin", -20.0),
            ("hipViewKickYawMax", 20.0),
            ("hipViewKickCenterSpeed", 1500.0),
            ("adsViewKickPitchMin", 10.0),
            ("adsViewKickPitchMax", 50.0),
            ("adsViewKickCenterSpeed", 1500.0),
            ("adsZoomFov", 50.0),
            ("adsTransInTime", 0.25),
            ("adsTransOutTime", 0.25),
            ("raiseTime", 0.75),
            ("dropTime", 0.45),
            ("firstRaiseTime", 0.85),
            ("shotCount", 1.0),
            ("lowAmmoWarningThreshold", 0.33),
        ]
        .into_iter()
        .collect();
        for (k, v) in extra {
            num.insert(k, *v);
        }
        let body: Vec<String> = num.iter().map(|(k, v)| format!("\"{k}\": {v}")).collect();
        let wclass = match class {
            "launcher" => "rocketlauncher",
            "shotgun" => "spread",
            "lmg" => "mg",
            _ => "rifle",
        };
        format!(
            r#"{{"schema":1,"name":"{name}","g":7,"class":"{class}","display":"Test {name}","fireType":"{fire}",
               "weaponClass":"{wclass}","move_class":"{}","num":{{{}}}}}"#,
            if wclass == "rocketlauncher" { "launcher" } else { wclass },
            body.join(",")
        )
    }

    pub(crate) fn spec(name: &str, class: &str, fire: &str, extra: &[(&str, f64)]) -> GunSpec {
        GunSpec::from_stats(&StatsFile::parse(&stats_json(name, class, fire, extra)).unwrap()).unwrap()
    }

    #[test]
    fn arsenal_spec_parses_and_validates() {
        let s = spec("testar_mp", "ar", "Full Auto", &[]);
        assert_eq!((s.clip, s.max_ammo, s.pellets, s.fire), (30, 180, 1, FireMode::FullAuto));
        assert!(matches!(s.reload, ReloadKind::Mag { reload_s, empty_s } if reload_s == 2.0 && empty_s == 2.5));
        assert!(s.rechamber.is_none());
        assert_eq!(s.move_class, MoveClass::Rifle);
        assert!((s.move_class.scale() - 0.95).abs() < 1e-6);
        assert_eq!(s.band_damages().first(), Some(&40));
        assert_eq!(s.refill_reserve(ReserveSource::Max), 180);
        assert_eq!(s.refill_reserve(ReserveSource::Start), 60);
        // bad data rejected
        let bad = stats_json("x_mp", "ar", "Full Auto", &[("minDamage", 50.0)]);
        assert!(GunSpec::from_stats(&StatsFile::parse(&bad).unwrap()).is_err());
        let bad = stats_json("x_mp", "ar", "Pump", &[]);
        assert!(GunSpec::from_stats(&StatsFile::parse(&bad).unwrap()).is_err());
        let bad = stats_json("x_mp", "ar", "Full Auto", &[("clipSize", 0.0)]);
        assert!(GunSpec::from_stats(&StatsFile::parse(&bad).unwrap()).is_err());
    }

    #[test]
    fn arsenal_body_mult_sniper_torso() {
        // review M6 fidelity: Intervention-like locTorsoUpper 1.5 -> 105 (the one-shot body kill), M21-like 1.1 -> 77,
        // a multiplier <= 1 or a launcher keeps the file damage
        let s = spec("bolt_mp", "sniper", "Single Shot", &[("damage", 70.0), ("minDamage", 70.0), ("locTorsoUpper", 1.5), ("locTorsoLower", 1.1)]);
        assert_eq!((s.body_mult, s.falloff.damage, s.falloff.min_damage, s.band_damages()), (1.5, 105, 105, vec![105]));
        let m = spec("m21x_mp", "sniper", "Single Shot", &[("damage", 70.0), ("minDamage", 70.0), ("locTorsoUpper", 1.1)]);
        assert_eq!(m.band_damages(), vec![77]);
        let a = spec("ar_mp", "ar", "Full Auto", &[("locTorsoUpper", 1.0)]);
        assert_eq!((a.body_mult, a.band_damages().first().copied()), (1.0, Some(40)));
        assert_eq!(body_damage(30, 1.5), 45);
        assert_eq!(body_damage(35, 1.1), 39, "38.5 rounds half away from zero (Python rhaz)");
    }

    #[test]
    fn arsenal_double_barrel_and_rechamber_rules() {
        // Double Barrel parses (weapons.rs had no arm); rechamber only with boltAction 1
        let s = spec("db_mp", "shotgun", "Double Barrel", &[("shotCount", 6.0), ("rechamberTime", 0.467)]);
        assert_eq!(s.fire, FireMode::DoubleBarrel);
        assert!(s.rechamber.is_none(), "rechamberTime without boltAction: no rechamber");
        assert_eq!(s.pellets, 6);
        let b = spec("bolt_mp", "sniper", "Single Shot", &[("boltAction", 1.0), ("rechamberTime", 0.866), ("rechamberWhileAds", 1.0)]);
        assert_eq!(b.rechamber, Some(Rechamber { time_s: 0.866, while_ads: true }));
    }

    #[test]
    fn arsenal_segmented_reload_fields() {
        let s = spec(
            "pump_mp",
            "shotgun",
            "Single Shot",
            &[
                ("segmentedReload", 1.0),
                ("reloadStartTime", 1.0),
                ("reloadStartAdd", 1.0),
                ("reloadStartAddTime", 0.6),
                ("reloadTime", 0.667),
                ("reloadAmmoAdd", 1.0),
                ("reloadAddTime", 1.0),
                ("reloadEndTime", 1.0),
                ("reloadEmptyTime", 0.0),
            ],
        );
        match s.reload {
            ReloadKind::Segmented { start_s, start_add, loop_s, add, add_s, end_s, .. } => {
                assert_eq!((start_s, start_add, loop_s, add, end_s), (1.0, 1, 0.667, 1, 1.0));
                assert_eq!(add_s, 0.667, "the add time is clamped to the loop end");
            }
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn arsenal_launcher_rules() {
        let l = spec(
            "tube_mp",
            "launcher",
            "Single Shot",
            &[
                ("damage", 1000.0),
                ("minDamage", 0.0),
                ("reloadEmptyTime", 0.0),
                ("reloadTime", 3.0),
                ("clipSize", 1.0),
                ("projectileSpeed", 2500.0),
                ("projectileLifetime", 7.0),
                ("explosionRadius", 400.0),
                ("explosionInnerDamage", 155.0),
                ("explosionOuterDamage", 55.0),
                ("requireLockonToFire", 1.0),
                ("projIgnitionDelay", 500.0),
            ],
        );
        let p = l.projectile.unwrap();
        assert_eq!(p.lock, LockKind::Required);
        assert!((p.speed_mps() - 63.5).abs() < 1e-3);
        assert!((p.ignition_s - 0.5).abs() < 1e-6);
        assert!(matches!(l.reload, ReloadKind::Mag { empty_s, .. } if empty_s == 3.0), "launchers reuse reloadTime");
        assert_eq!(l.move_class, MoveClass::Launcher);
        assert!((l.move_class.scale() - 0.8).abs() < 1e-6);
        // a launcher without explosion fields is rejected
        let bad = stats_json("t2_mp", "launcher", "Single Shot", &[("minDamage", 0.0), ("reloadEmptyTime", 0.0)]);
        assert!(GunSpec::from_stats(&StatsFile::parse(&bad).unwrap()).is_err());
    }

    #[test]
    fn arsenal_move_classes() {
        for (s, v) in [("rifle", 0.95), ("mg", 0.875), ("smg", 1.0), ("spread", 0.95), ("launcher", 0.8), ("sniper", 1.0), ("other", 1.0)] {
            assert!((MoveClass::parse(s).scale() - v).abs() < 1e-6, "{s}");
        }
    }
}
