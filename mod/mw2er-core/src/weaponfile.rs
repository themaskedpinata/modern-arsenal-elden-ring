//! Parser for IW4 `WEAPONFILE` text dumps (`WEAPONFILE\key\value\key\value...`).
//!
//! This is the Rust twin of `tools/scripts/convert_weapons.py`. The DLL never reads dumps; this
//! parser exists so the `real_data` test can check, field by field, that the Python converter and
//! an independent implementation agree on what the dumps contain.
//!
//! Format facts (verified on the three M2 dumps): tokens are separated by `\` only; values may
//! contain newlines (`hideTags`, `notetrackSoundMap`, `notetrackRumbleMap`); there is no trailing
//! newline; each file has 671 key/value pairs.

use std::collections::HashMap;
use std::fmt;

/// Number of key/value pairs in every IW4 weapon dump of this game build.
pub const EXPECTED_FIELD_COUNT: usize = 671;

/// A parsed weapon file: ordered fields plus a key index.
#[derive(Clone, Debug)]
pub struct WeaponFile {
    fields: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WeaponFileError {
    MissingHeader,
    OddTokenCount(usize),
    EmptyKey(usize),
    DuplicateKey(String),
    MissingKey(String),
    NotANumber { key: String, value: String },
}

impl fmt::Display for WeaponFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "file does not start with WEAPONFILE"),
            Self::OddTokenCount(n) => write!(f, "odd number of key/value tokens ({n})"),
            Self::EmptyKey(i) => write!(f, "empty key at pair {i}"),
            Self::DuplicateKey(k) => write!(f, "duplicate key {k}"),
            Self::MissingKey(k) => write!(f, "missing key {k}"),
            Self::NotANumber { key, value } => write!(f, "{key}={value:?} is not a number"),
        }
    }
}

impl std::error::Error for WeaponFileError {}

/// Parses a WEAPONFILE string.
pub fn parse(text: &str) -> Result<WeaponFile, WeaponFileError> {
    let mut tokens = text.split('\\');
    if tokens.next() != Some("WEAPONFILE") {
        return Err(WeaponFileError::MissingHeader);
    }
    let rest: Vec<&str> = tokens.collect();
    if rest.len() % 2 != 0 {
        return Err(WeaponFileError::OddTokenCount(rest.len()));
    }
    let mut fields = Vec::with_capacity(rest.len() / 2);
    let mut index = HashMap::with_capacity(rest.len() / 2);
    for (i, pair) in rest.chunks_exact(2).enumerate() {
        let (key, value) = (pair[0], pair[1]);
        if key.is_empty() {
            return Err(WeaponFileError::EmptyKey(i));
        }
        if index.insert(key.to_string(), i).is_some() {
            return Err(WeaponFileError::DuplicateKey(key.to_string()));
        }
        fields.push((key.to_string(), value.to_string()));
    }
    Ok(WeaponFile { fields, index })
}

impl WeaponFile {
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|&i| self.fields[i].1.as_str())
    }

    pub fn require(&self, key: &str) -> Result<&str, WeaponFileError> {
        self.get(key).ok_or_else(|| WeaponFileError::MissingKey(key.to_string()))
    }

    pub fn number(&self, key: &str) -> Result<f64, WeaponFileError> {
        let v = self.require(key)?;
        v.trim().parse::<f64>().map_err(|_| WeaponFileError::NotANumber {
            key: key.to_string(),
            value: v.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic strings with made-up values (no MW2 data in tests that ship with the code).
    const GOOD: &str = "WEAPONFILE\\displayName\\WEAPON_TEST\\hideTags\\tag_a\ntag_b\n\\damage\\12\\fireTime\\0.25";

    #[test]
    fn weaponfile_parses_good_and_keeps_newlines() {
        let w = parse(GOOD).unwrap();
        assert_eq!(w.len(), 4);
        assert_eq!(w.get("displayName"), Some("WEAPON_TEST"));
        assert_eq!(w.get("hideTags"), Some("tag_a\ntag_b\n"));
        assert_eq!(w.number("damage").unwrap(), 12.0);
        assert_eq!(w.number("fireTime").unwrap(), 0.25);
        assert_eq!(w.fields()[0].0, "displayName");
    }

    #[test]
    fn weaponfile_rejects_bad_input() {
        assert_eq!(parse("WEAPONFIL\\a\\b").unwrap_err(), WeaponFileError::MissingHeader);
        assert_eq!(parse("WEAPONFILE\\a\\b\\c").unwrap_err(), WeaponFileError::OddTokenCount(3));
        assert_eq!(
            parse("WEAPONFILE\\a\\1\\a\\2").unwrap_err(),
            WeaponFileError::DuplicateKey("a".into())
        );
        assert_eq!(parse("WEAPONFILE\\\\1").unwrap_err(), WeaponFileError::EmptyKey(0));
        let w = parse("WEAPONFILE\\a\\x").unwrap();
        assert!(matches!(w.number("a"), Err(WeaponFileError::NotANumber { .. })));
        assert_eq!(w.require("b").unwrap_err(), WeaponFileError::MissingKey("b".into()));
    }
}
