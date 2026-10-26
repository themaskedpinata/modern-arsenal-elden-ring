//! Model of `converted\sounds\sounds.json` (written by `tools/scripts/copy_sounds.py`).
//!
//! Paths are relative to the `converted\` folder (the parent of `sounds\`), with forward slashes.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct SoundsFile {
    pub schema: u32,
    #[serde(default)]
    pub generator: String,
    pub weapons: BTreeMap<String, WeaponSounds>,
}

/// One gun's sound roles. A missing role is simply silent.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct WeaponSounds {
    pub fire: Option<String>,
    pub reload_lift: Option<String>,
    pub reload_clipout: Option<String>,
    pub reload_clipin: Option<String>,
    pub reload_chamber: Option<String>,
    #[serde(default)]
    pub durations_s: BTreeMap<String, f64>,
}

impl SoundsFile {
    pub fn parse(text: &str) -> Result<SoundsFile, String> {
        let f: SoundsFile = serde_json::from_str(text).map_err(|e| format!("sounds.json: {e}"))?;
        if f.schema != 1 {
            return Err(format!("sounds.json: unsupported schema {}", f.schema));
        }
        for (gun, s) in &f.weapons {
            for p in [&s.fire, &s.reload_lift, &s.reload_clipout, &s.reload_clipin, &s.reload_chamber]
                .into_iter()
                .flatten()
            {
                if p.contains("..") || p.starts_with('/') || p.contains(':') {
                    return Err(format!("sounds.json: {gun}: path {p:?} must stay inside converted/"));
                }
            }
        }
        Ok(f)
    }
}

// ---- M6: per-gun sound tables (converted\guns\<name>\sounds.json, docs/M6_DESIGN.md 3.2) -----------------

/// One WAV variant of an MW2 sound alias (MW2 picks one at random with a random volume and pitch in the ranges).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AliasVariant {
    /// Relative to `converted\guns\` (`_snd/<sha8>_<name>.wav`).
    pub file: String,
    pub vol: [f32; 2],
    pub pitch: [f32; 2],
    /// A layer alias played with it (mech / raise / fire layers), one level deep.
    #[serde(default)]
    pub secondary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AliasEntry {
    #[serde(default)]
    pub zone: String,
    pub variants: Vec<AliasVariant>,
}

/// `sounds.json` of one gun: role -> alias, notetrack -> alias, alias -> variants.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct GunSoundsFile {
    pub schema: u32,
    pub roles: BTreeMap<String, String>,
    pub notes: BTreeMap<String, String>,
    pub aliases: BTreeMap<String, AliasEntry>,
}

impl GunSoundsFile {
    pub fn parse(text: &str) -> Result<GunSoundsFile, String> {
        let f: GunSoundsFile = serde_json::from_str(text).map_err(|e| format!("sounds.json: {e}"))?;
        if f.schema != 1 {
            return Err(format!("sounds.json: unsupported schema {}", f.schema));
        }
        for (a, e) in &f.aliases {
            for v in &e.variants {
                let ok = v.file.strip_prefix("_snd/").is_some_and(|n| !n.is_empty() && !n.contains(['/', '\\', ':']) && !n.contains(".."));
                if !ok {
                    return Err(format!("sounds.json: alias {a}: file {:?} must be _snd/<name>", v.file));
                }
                if !(v.vol.iter().chain(v.pitch.iter()).all(|x| x.is_finite() && *x >= 0.0 && *x <= 8.0)) {
                    return Err(format!("sounds.json: alias {a}: vol/pitch out of range"));
                }
            }
        }
        for (k, a) in f.roles.iter().chain(f.notes.iter()) {
            if !f.aliases.contains_key(a) {
                return Err(format!("sounds.json: {k} -> alias {a} not listed"));
            }
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_gun_table_parses_and_rejects_escapes() {
        let ok = r#"{"schema":1,"roles":{"fire":"f_plr"},"notes":{"n1":"c_plr"},"aliases":{
            "f_plr":{"zone":"z","variants":[{"file":"_snd/ab_f.wav","vol":[0.4,0.5],"pitch":[0.9,1.0],"secondary":"m_plr"}]},
            "c_plr":{"variants":[{"file":"_snd/cd_c.wav","vol":[1,1],"pitch":[1,1]}]}}}"#;
        let f = GunSoundsFile::parse(ok).unwrap();
        assert_eq!(f.roles["fire"], "f_plr");
        assert_eq!(f.aliases["f_plr"].variants[0].secondary.as_deref(), Some("m_plr"));
        assert!(GunSoundsFile::parse(&ok.replace("_snd/ab_f.wav", "../x.wav")).is_err());
        assert!(GunSoundsFile::parse(&ok.replace(r#""n1":"c_plr""#, r#""n1":"zz""#)).is_err(), "dangling alias");
    }

    #[test]
    fn sounds_json_parses_and_rejects_escapes() {
        let ok = r#"{"schema":1,"generator":"t","weapons":{"g":{"fire":"sounds/g/f.wav",
                    "reload_lift":"sounds/g/l.wav","durations_s":{"fire":2.5}}}}"#;
        let f = SoundsFile::parse(ok).unwrap();
        let g = &f.weapons["g"];
        assert_eq!(g.fire.as_deref(), Some("sounds/g/f.wav"));
        assert!(g.reload_chamber.is_none());
        assert_eq!(g.durations_s["fire"], 2.5);
        let bad = ok.replace("sounds/g/f.wav", "../../x.wav");
        assert!(SoundsFile::parse(&bad).is_err());
    }
}
