//! M7: the converted MW2 Tactical Nuke data (`converted\nuke\nuke.json`, written by `tools\scripts\convert_nuke.py`
//! from the user's MW2 dumps). Nothing MW2 is typed in the DLL: the timeline, the cost, the two visions, the HUD
//! layout, the strings and the sound aliases all come from this file (docs/M7_DESIGN.md 3.1).

use std::collections::BTreeMap;

use serde::Deserialize;

pub const NUKE_FILE: &str = "nuke.json";

/// One MW2 `.vision` (glow + film tweak).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Vision {
    pub glow_radius: f32,
    pub glow_cutoff: f32,
    pub glow_desaturation: f32,
    pub glow_intensity: f32,
    pub film_contrast: f32,
    pub film_brightness: f32,
    pub film_desaturation: f32,
    pub film_invert: u8,
    pub film_light_tint: [f32; 3],
    pub film_dark_tint: [f32; 3],
}

/// The `_nuke.gsc` timeline and numbers (seconds; distances in inches).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Timeline {
    /// `scr_nukeTimer`
    pub timer_s: f64,
    /// `nukeSoundIncoming` at timer - this
    pub incoming_lead_s: f64,
    /// `nukeVision` at timer + this, blending `mpnuke` over `vision_blend_s`
    pub vision_delay_s: f64,
    pub vision_blend_s: f64,
    /// `nukeDeath` at timer + this
    pub death_delay_s: f64,
    /// `mpnuke_aftermath` blend after the death
    pub aftermath_blend_s: f64,
    /// `ui_mp_nukebomb_timer` every this many seconds from the activation
    pub beep_every_s: f64,
    /// `nukeDistance` (the flash fx ahead of each player)
    pub nuke_distance_in: f64,
    /// `scr_nukeCancelMode`: 0 = the caller's death does not cancel
    pub cancel_mode: i32,
    /// `nukeEarthquake` at timer + this (MW2 starts it together with `nukeDeath`).
    pub quake_delay_s: f64,
    /// The commented-out `earthquake(scale, duration, pos, radius)` of `nukeEarthquake`.
    pub quake_scale: f64,
    pub quake_duration_s: f64,
    pub quake_radius_in: f64,
    /// true: the earthquake line is commented out in MW2 (no shake in MW2).
    pub quake_commented: bool,
}

/// The HUD bits (scorebar_hd.menu countdown item, colours, the strings).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hud {
    /// `rect x y w h` of the countdown item on the 640x480 virtual screen.
    pub timer_rect: [f32; 4],
    pub timer_textscale: f32,
    pub timer_forecolor: [f32; 4],
    pub timer_glowcolor: [f32; 4],
    pub strings: BTreeMap<String, String>,
    /// The dpad_nuke icon as raw RGBA8 (`converted\nuke\<file>`), its size.
    #[serde(default)]
    pub icon_file: String,
    #[serde(default)]
    pub icon_size: [u32; 2],
}

/// One sound alias: its variants (files relative to the nuke folder), volume / pitch ranges and start delay.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Sound {
    pub alias: String,
    pub files: Vec<String>,
    pub vol_min: f32,
    pub vol_max: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    pub startdelay_ms: u32,
    /// Another key of `sounds` started together with this one (IW4 secondaryaliasname), or "".
    #[serde(default)]
    pub secondary: String,
    pub duration_s: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NukeFile {
    pub schema: u32,
    pub generator: String,
    /// killstreaktable.csv cost.
    pub cost: u32,
    pub timeline: Timeline,
    pub visions: BTreeMap<String, Vision>,
    pub hud: Hud,
    pub sounds: BTreeMap<String, Sound>,
}

/// The sound keys the DLL plays (docs/M7_DESIGN.md 3.6).
pub const SOUND_KEYS: [&str; 10] = [
    "activate_trigger",
    "use_us",
    "use_uk",
    "timer",
    "incoming",
    "explosion",
    "wave",
    "earned",
    "achieve_us",
    "achieve_uk",
];

impl NukeFile {
    pub fn parse(text: &str) -> Result<NukeFile, String> {
        let f: NukeFile = serde_json::from_str(text).map_err(|e| format!("nuke.json: {e}"))?;
        f.validate()?;
        Ok(f)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("nuke.json schema {}", self.schema));
        }
        let t = &self.timeline;
        for (n, v) in [
            ("timer_s", t.timer_s),
            ("incoming_lead_s", t.incoming_lead_s),
            ("vision_delay_s", t.vision_delay_s),
            ("vision_blend_s", t.vision_blend_s),
            ("death_delay_s", t.death_delay_s),
            ("aftermath_blend_s", t.aftermath_blend_s),
            ("beep_every_s", t.beep_every_s),
            ("quake_delay_s", t.quake_delay_s),
        ] {
            if !(v.is_finite() && (0.0..=120.0).contains(&v)) {
                return Err(format!("nuke.json timeline.{n} = {v}"));
            }
        }
        if t.timer_s < 1.0 || t.beep_every_s < 0.1 || t.incoming_lead_s > t.timer_s {
            return Err("nuke.json timeline: timer / beep / incoming out of order".into());
        }
        if !(1..=100).contains(&self.cost) {
            return Err(format!("nuke.json cost {}", self.cost));
        }
        for v in ["mpnuke", "mpnuke_aftermath"] {
            if !self.visions.contains_key(v) {
                return Err(format!("nuke.json: vision {v} missing"));
            }
        }
        for k in SOUND_KEYS {
            let Some(s) = self.sounds.get(k) else { return Err(format!("nuke.json: sound {k} missing")) };
            if s.files.is_empty() || s.files.iter().any(|f| f.contains("..") || f.contains(':')) {
                return Err(format!("nuke.json: sound {k} files {:?}", s.files));
            }
            if !s.secondary.is_empty() && !self.sounds.contains_key(&s.secondary) {
                return Err(format!("nuke.json: sound {k} secondary {} missing", s.secondary));
            }
        }
        Ok(())
    }

    pub fn vision(&self, name: &str) -> Vision {
        self.visions[name]
    }

    /// The localized string or the key itself.
    pub fn string<'a>(&'a self, key: &'a str) -> &'a str {
        self.hud.strings.get(key).map_or(key, |s| s.as_str())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A hand-written fixture with the shape the converter writes: MW2's timeline numbers (the sequence tests need
    /// them), synthetic vision values (warm white-out, orange aftermath; the real MW2 visions are checked only by
    /// `tests/real_data_m7.rs`, which reads `converted\nuke` at run time).
    pub fn fixture() -> NukeFile {
        let v = |gr: f32, gc: f32, gd: f32, gi: f32, c: f32, b: f32, d: f32, l: [f32; 3], k: [f32; 3]| Vision {
            glow_radius: gr,
            glow_cutoff: gc,
            glow_desaturation: gd,
            glow_intensity: gi,
            film_contrast: c,
            film_brightness: b,
            film_desaturation: d,
            film_invert: 0,
            film_light_tint: l,
            film_dark_tint: k,
        };
        let mut visions = BTreeMap::new();
        visions.insert("mpnuke".to_string(), v(2.5, 0.25, 0.3, 3.5, 1.5, 0.03, 0.4, [1.9, 1.4, 0.6], [0.8, 0.85, 1.0]));
        visions.insert("mpnuke_aftermath".to_string(), v(5.0, 0.6, 0.6, 0.5, 1.7, 0.04, 0.5, [1.0, 0.95, 0.9], [0.8, 0.4, 0.25]));
        let mut sounds = BTreeMap::new();
        for (i, k) in SOUND_KEYS.iter().enumerate() {
            sounds.insert(
                k.to_string(),
                Sound {
                    alias: format!("alias_{k}"),
                    files: vec![format!("sounds/{i}.wav")],
                    vol_min: 0.5,
                    vol_max: 0.5,
                    pitch_min: 1.0,
                    pitch_max: 1.0,
                    startdelay_ms: 0,
                    secondary: String::new(),
                    duration_s: 1.0,
                },
            );
        }
        let mut strings = BTreeMap::new();
        strings.insert("MP_TACTICAL_NUKE".to_string(), "Tactical Nuke".to_string());
        NukeFile {
            schema: 1,
            generator: "fixture".into(),
            cost: 25,
            timeline: Timeline {
                timer_s: 10.0,
                incoming_lead_s: 3.3,
                vision_delay_s: 0.25,
                vision_blend_s: 3.0,
                death_delay_s: 1.5,
                aftermath_blend_s: 5.0,
                beep_every_s: 1.0,
                nuke_distance_in: 5000.0,
                cancel_mode: 0,
                quake_delay_s: 1.5,
                quake_scale: 0.6,
                quake_duration_s: 10.0,
                quake_radius_in: 100000.0,
                quake_commented: true,
            },
            visions,
            hud: Hud {
                timer_rect: [112.0, 42.0, 100.0, 30.0],
                timer_textscale: 0.3333,
                timer_forecolor: [1.0, 1.0, 1.0, 1.0],
                timer_glowcolor: [1.0, 0.25, 0.0, 1.0],
                strings,
                icon_file: String::new(),
                icon_size: [0, 0],
            },
            sounds,
        }
    }

    #[test]
    fn nuke_data_fixture_validates_and_rejects() {
        let f = fixture();
        f.validate().unwrap();
        assert_eq!(f.string("MP_TACTICAL_NUKE"), "Tactical Nuke");
        assert_eq!(f.string("NOPE"), "NOPE");
        let mut g = f.clone();
        g.visions.remove("mpnuke");
        assert!(g.validate().is_err());
        let mut g = f.clone();
        g.timeline.timer_s = f64::NAN;
        assert!(g.validate().is_err());
        let mut g = f.clone();
        g.sounds.get_mut("timer").unwrap().files = vec!["..\\x.wav".into()];
        assert!(g.validate().is_err());
        let mut g = f.clone();
        g.sounds.get_mut("explosion").unwrap().secondary = "nope".into();
        assert!(g.validate().is_err());
    }
}
