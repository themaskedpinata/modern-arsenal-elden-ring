//! Models of the M4 data files in `converted\ac130\` (written by `tools\scripts\convert_ac130.py` from the user's
//! own MW2 dumps; docs/M4_DESIGN.md section 13). No MW2 number or string is compiled into the DLL: everything the
//! AC-130 uses at runtime comes from these files, and config values only override them.

use std::collections::BTreeMap;

use serde::Deserialize;

pub const WEAPONS_FILE: &str = "weapons_ac130.json";
pub const TIMING_FILE: &str = "timing.json";
pub const VISION_FILE: &str = "vision.json";
pub const HUD_FILE: &str = "hud_layout.json";
pub const SOUNDS_FILE: &str = "sounds_ac130.json";
/// The overlay textures, in shader slot order (105, 40, 25, grain).
pub const OVERLAY_FILES: [&str; 4] =
    ["ac130_overlay_105mm.vmtex", "ac130_overlay_40mm.vmtex", "ac130_overlay_25mm.vmtex", "ac130_overlay_grain.vmtex"];
/// Gun ids in MW2's give order (105 -> 40 -> 25); index = gun slot everywhere in the mod.
pub const GUN_IDS: [&str; 3] = ["105", "40", "25"];

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Gun {
    pub id: String,
    pub weapon: String,
    pub clip_size: u32,
    pub fire_time_s: f64,
    pub reload_s: f64,
    pub projectile_speed_ips: f64,
    pub projectile_speed_mps: f64,
    pub explosion_radius_in: f64,
    pub explosion_radius_m: f64,
    pub explosion_inner: f64,
    pub explosion_outer: f64,
    pub damage: f64,
    pub ads_zoom_fov: f64,
    pub fire_sound: String,
    pub explosion_sound: String,
    pub big_explosion: u32,
    pub impact_type: String,
    pub fire_type: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct WeaponsFile {
    pub schema: u32,
    #[serde(default)]
    pub generator: String,
    pub guns: Vec<Gun>,
}

fn finite_in(name: &str, v: f64, lo: f64, hi: f64) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} = {v} outside {lo}..={hi}")) }
}

impl WeaponsFile {
    pub fn parse(text: &str) -> Result<WeaponsFile, String> {
        let f: WeaponsFile = serde_json::from_str(text).map_err(|e| format!("{WEAPONS_FILE}: {e}"))?;
        if f.schema != 1 {
            return Err(format!("{WEAPONS_FILE}: schema {}", f.schema));
        }
        let ids: Vec<&str> = f.guns.iter().map(|g| g.id.as_str()).collect();
        if ids != GUN_IDS {
            return Err(format!("{WEAPONS_FILE}: guns {ids:?} (want {GUN_IDS:?} in this order)"));
        }
        for g in &f.guns {
            let p = |n: &str| format!("{WEAPONS_FILE} {}.{n}", g.id);
            if !(1..=999).contains(&g.clip_size) {
                return Err(format!("{} = {}", p("clip_size"), g.clip_size));
            }
            finite_in(&p("fire_time_s"), g.fire_time_s, 0.01, 10.0)?;
            finite_in(&p("reload_s"), g.reload_s, 0.0, 60.0)?;
            finite_in(&p("projectile_speed_mps"), g.projectile_speed_mps, 1.0, 10_000.0)?;
            finite_in(&p("explosion_radius_m"), g.explosion_radius_m, 0.01, 100.0)?;
            finite_in(&p("explosion_inner"), g.explosion_inner, 0.0, 100_000.0)?;
            finite_in(&p("explosion_outer"), g.explosion_outer, 0.0, 100_000.0)?;
            finite_in(&p("ads_zoom_fov"), g.ads_zoom_fov, 1.0, 120.0)?;
            if (g.projectile_speed_ips * 0.0254 - g.projectile_speed_mps).abs() > 1e-6
                || (g.explosion_radius_in * 0.0254 - g.explosion_radius_m).abs() > 1e-6
            {
                return Err(format!("{WEAPONS_FILE} {}: inch/metre values disagree", g.id));
            }
        }
        Ok(f)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Quake {
    pub scale: f64,
    pub duration_s: f64,
    /// MW2 earthquake radius (inches; convert_ac130.py 1.1, after run 9; 0 in an older timing.json).
    #[serde(default)]
    pub radius_in: f64,
    /// "plane" (weaponFiredThread) or "impact" (shotFired); empty in an older timing.json.
    #[serde(default)]
    pub origin: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DarkPulse {
    pub alpha: f64,
    pub fade_in_s: f64,
    pub hold_s: f64,
    pub fade_out_s: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TimingFile {
    pub schema: u32,
    pub duration_s: f64,
    pub orbit_period_s: f64,
    pub ramp_deg: f64,
    /// MW2 `PlayerLinkWeaponviewToDelta` arcs: right, left, top, bottom (degrees).
    pub arc_deg: [f64; 4],
    pub rig_radius_in: f64,
    pub rig_height_in: f64,
    pub tag_pitch_deg: f64,
    pub center_pitch_deg: f64,
    pub reload_s: BTreeMap<String, f64>,
    pub fade_in_s: f64,
    pub to_black_hot_s: f64,
    pub to_white_hot_s: f64,
    pub fire_quake: BTreeMap<String, Quake>,
    pub impact_quake: BTreeMap<String, Quake>,
    pub dark_pulse: DarkPulse,
}

impl TimingFile {
    pub fn parse(text: &str) -> Result<TimingFile, String> {
        let f: TimingFile = serde_json::from_str(text).map_err(|e| format!("{TIMING_FILE}: {e}"))?;
        if f.schema != 1 {
            return Err(format!("{TIMING_FILE}: schema {}", f.schema));
        }
        finite_in("timing.duration_s", f.duration_s, 1.0, 600.0)?;
        finite_in("timing.orbit_period_s", f.orbit_period_s, 1.0, 3600.0)?;
        finite_in("timing.rig_radius_in", f.rig_radius_in, 1.0, 100_000.0)?;
        finite_in("timing.rig_height_in", f.rig_height_in, 1.0, 100_000.0)?;
        finite_in("timing.tag_pitch_deg", f.tag_pitch_deg, 1.0, 89.0)?;
        for (i, a) in f.arc_deg.iter().enumerate() {
            finite_in(&format!("timing.arc_deg[{i}]"), *a, 0.0, 90.0)?;
        }
        finite_in("timing.fade_in_s", f.fade_in_s, 0.0, 10.0)?;
        finite_in("timing.to_black_hot_s", f.to_black_hot_s, 0.0, 10.0)?;
        finite_in("timing.to_white_hot_s", f.to_white_hot_s, 0.0, 10.0)?;
        Ok(f)
    }

    /// Rig geometry in metres (MW2 inches x 0.0254).
    pub fn rig_m(&self) -> (f64, f64) {
        (self.rig_radius_in * 0.0254, self.rig_height_in * 0.0254)
    }
}

/// One MW2 `.vision` file's film values (`r_film*`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct VisionDef {
    pub contrast: f32,
    pub brightness: f32,
    pub desaturation: f32,
    pub invert: u32,
    pub light_tint: [f32; 3],
    pub dark_tint: [f32; 3],
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct VisionFile {
    pub schema: u32,
    pub visions: BTreeMap<String, VisionDef>,
}

impl VisionFile {
    pub fn parse(text: &str) -> Result<VisionFile, String> {
        let f: VisionFile = serde_json::from_str(text).map_err(|e| format!("{VISION_FILE}: {e}"))?;
        if f.schema != 1 {
            return Err(format!("{VISION_FILE}: schema {}", f.schema));
        }
        for need in ["ac130", "ac130_inverted", "thermal_mp", "missilecam", "black_bw"] {
            let v = f.visions.get(need).ok_or_else(|| format!("{VISION_FILE}: vision {need} missing"))?;
            let all = [v.contrast, v.brightness, v.desaturation]
                .into_iter()
                .chain(v.light_tint)
                .chain(v.dark_tint);
            if all.into_iter().any(|x| !(x.is_finite() && x.abs() <= 16.0)) || v.invert > 1 {
                return Err(format!("{VISION_FILE}: vision {need} has out-of-range values"));
            }
        }
        Ok(f)
    }
}

/// One menu item of `ac130_hud_hd.menu` (virtual 640x480 space).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HudItem {
    pub key: String,
    #[serde(default)]
    pub text: String,
    /// x, y, w, h, horzAlign, vertAlign.
    pub rect: [f32; 6],
    #[serde(default)]
    pub textalign: u32,
    #[serde(default)]
    pub textscale: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HudFile {
    pub schema: u32,
    pub items: Vec<HudItem>,
    pub overlay_rect: [f32; 6],
    pub grain_forecolor: [f32; 4],
    pub blink_ms: f32,
    pub blink_amp: f32,
    /// gun id -> label ("105 mm" ...), localized strings from the user's dump.
    pub labels: BTreeMap<String, String>,
    /// reloading / whot / bhot / agl.
    pub strings: BTreeMap<String, String>,
}

impl HudFile {
    pub fn parse(text: &str) -> Result<HudFile, String> {
        let f: HudFile = serde_json::from_str(text).map_err(|e| format!("{HUD_FILE}: {e}"))?;
        if f.schema != 1 {
            return Err(format!("{HUD_FILE}: schema {}", f.schema));
        }
        for id in GUN_IDS {
            if !f.labels.contains_key(id) {
                return Err(format!("{HUD_FILE}: label for gun {id} missing"));
            }
        }
        if !(f.blink_ms > 0.0 && f.blink_ms.is_finite() && (0.0..=1.0).contains(&f.blink_amp)) {
            return Err(format!("{HUD_FILE}: blink {} ms / {}", f.blink_ms, f.blink_amp));
        }
        Ok(f)
    }

    pub fn item(&self, key: &str) -> Option<&HudItem> {
        self.items.iter().find(|i| i.key == key)
    }

    pub fn string(&self, key: &str) -> &str {
        self.strings.get(key).map(String::as_str).unwrap_or("")
    }
}

/// One sound cue (an MW2 alias): its files (variants picked at random), volume/pitch ranges, distance range in
/// inches and the alias `startdelay`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SoundCue {
    pub files: Vec<String>,
    pub vol_min: f32,
    pub vol_max: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    #[serde(default)]
    pub dist_min_in: f32,
    #[serde(default)]
    pub dist_max_in: f32,
    #[serde(default)]
    pub startdelay_ms: u32,
    #[serde(default)]
    pub looping: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SoundsFile {
    pub schema: u32,
    pub cues: BTreeMap<String, SoundCue>,
}

impl SoundsFile {
    pub fn parse(text: &str) -> Result<SoundsFile, String> {
        let f: SoundsFile = serde_json::from_str(text).map_err(|e| format!("{SOUNDS_FILE}: {e}"))?;
        if f.schema != 1 {
            return Err(format!("{SOUNDS_FILE}: schema {}", f.schema));
        }
        for (name, c) in &f.cues {
            if c.files.is_empty() || c.files.len() > 16 {
                return Err(format!("{SOUNDS_FILE}: cue {name} has {} files", c.files.len()));
            }
            for p in &c.files {
                if p.contains("..") || p.starts_with('/') || p.starts_with('\\') || p.contains(':') {
                    return Err(format!("{SOUNDS_FILE}: cue {name}: path {p:?} must stay inside converted/ac130/"));
                }
            }
            let ok = [c.vol_min, c.vol_max, c.pitch_min, c.pitch_max].iter().all(|v| v.is_finite() && (0.0..=4.0).contains(v))
                && c.vol_min <= c.vol_max
                && c.pitch_min <= c.pitch_max
                && c.pitch_min > 0.0;
            if !ok {
                return Err(format!("{SOUNDS_FILE}: cue {name} volume/pitch range"));
            }
        }
        Ok(f)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn weapons_json() -> String {
        let gun = |id: &str, clip: u32, ft: f64, rl: f64, sp: f64, r: f64, i: f64, o: f64, fov: f64| {
            format!(
                r#"{{"id":"{id}","weapon":"ac130_{id}mm_mp","clip_size":{clip},"fire_time_s":{ft},"reload_s":{rl},
                "projectile_speed_ips":{sp},"projectile_speed_mps":{},"explosion_radius_in":{r},"explosion_radius_m":{},
                "explosion_inner":{i},"explosion_outer":{o},"damage":1000,"ads_zoom_fov":{fov},"fire_sound":"f",
                "explosion_sound":"e","big_explosion":1,"impact_type":"none","fire_type":"Full Auto"}}"#,
                sp * 0.0254,
                r * 0.0254
            )
        };
        format!(
            r#"{{"schema":1,"generator":"t","guns":[{},{},{}]}}"#,
            gun("105", 1, 0.1, 5.0, 3000.0, 600.0, 1200.0, 300.0, 45.0),
            gun("40", 4, 0.3, 3.0, 5000.0, 150.0, 500.0, 50.0, 20.0),
            gun("25", 20, 0.07, 1.5, 10000.0, 32.0, 500.0, 10.0, 5.0)
        )
    }

    pub(crate) fn timing_json() -> String {
        r#"{"schema":1,"duration_s":40.0,"orbit_period_s":70.0,"ramp_deg":10.0,"arc_deg":[35,35,35,35],
            "rig_radius_in":4566.0,"rig_height_in":4212.4,"tag_pitch_deg":64.5,"center_pitch_deg":42.69,
            "reload_s":{"105":5.0,"40":3.0,"25":1.5},"fade_in_s":1.0,"to_black_hot_s":0.62,"to_white_hot_s":0.51,
            "fire_quake":{"105":{"scale":0.2,"duration_s":1.0},"40":{"scale":0.1,"duration_s":0.5}},
            "impact_quake":{"105":{"scale":0.4,"duration_s":1.0},"40":{"scale":0.2,"duration_s":0.5}},
            "dark_pulse":{"alpha":0.6,"fade_in_s":0.2,"hold_s":0.4,"fade_out_s":0.8}}"#
            .to_string()
    }

    #[test]
    fn ac130_data_models_parse_and_validate() {
        let w = WeaponsFile::parse(&weapons_json()).unwrap();
        assert_eq!(w.guns[1].clip_size, 4);
        assert!(WeaponsFile::parse(&weapons_json().replace(r#""id":"40""#, r#""id":"41""#)).is_err());
        assert!(WeaponsFile::parse(&weapons_json().replace(r#""fire_time_s":0.3"#, r#""fire_time_s":-0.3"#)).is_err());
        let t = TimingFile::parse(&timing_json()).unwrap();
        let (r, h) = t.rig_m();
        assert!((r - 115.9764).abs() < 1e-3 && (h - 106.997).abs() < 1e-2);
        assert!(TimingFile::parse(&timing_json().replace("\"duration_s\":40.0", "\"duration_s\":0.0")).is_err());
        let s = SoundsFile::parse(r#"{"schema":1,"cues":{"fire_105":{"files":["sounds/a.wav"],"vol_min":0.3,"vol_max":0.3,"pitch_min":0.7,"pitch_max":0.8}}}"#).unwrap();
        assert_eq!(s.cues["fire_105"].startdelay_ms, 0);
        assert!(SoundsFile::parse(r#"{"schema":1,"cues":{"x":{"files":["../a.wav"],"vol_min":0.3,"vol_max":0.3,"pitch_min":0.7,"pitch_max":0.8}}}"#).is_err());
    }
}
