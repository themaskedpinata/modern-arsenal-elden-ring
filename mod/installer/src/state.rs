//! `<root>\state\install.json`: what was installed (release version, co-op id, mode), the paths found or chosen
//! (so a rerun or an update asks nothing), the skip key of every finished build step, the hashes the launcher's
//! quick preflight compares, and the shortcuts / registry entries to remove on uninstall.
//!
//! It never holds the co-op password: there is no field for it, and [`InstallState::save`] refuses to write a
//! document containing a `password` key (belt and braces against a future field).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::guard::Guard;

pub const STATE_FILE: &str = r"state\install.json";
pub const SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Solo,
    Coop,
}

impl Mode {
    pub fn coop(self) -> bool {
        self == Mode::Coop
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "solo" => Some(Mode::Solo),
            "coop" | "co-op" => Some(Mode::Coop),
            _ => None,
        }
    }
}

/// Paths found or chosen (all optional; never a password).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub er_game: Option<PathBuf>,
    pub mw2_dir: Option<PathBuf>,
    pub me3: Option<PathBuf>,
    /// The player's own Seamless (folder `ersc.dll` or zip) the copy was made from.
    pub seamless_source: Option<PathBuf>,
    pub witchy_zip: Option<PathBuf>,
    pub python: Option<PathBuf>,
    pub payload: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    /// The step's skip key (sha256 over its inputs) when it last succeeded.
    pub key: String,
    pub done_utc: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallState {
    pub schema: u32,
    pub what: String,
    pub release_version: String,
    pub coop_id: String,
    pub mode: Mode,
    pub paths: Paths,
    /// Step id -> record.
    pub steps: BTreeMap<String, StepRecord>,
    /// Root-relative file -> sha256, compared by the launcher's quick preflight (DLL, regulation, weapons.json,
    /// the Seamless copy's ersc.dll).
    pub preflight: BTreeMap<String, String>,
    pub shortcuts: Vec<PathBuf>,
    /// HKCU subkeys this install created (removed on uninstall).
    pub registry: Vec<String>,
    /// Top-level entries of the root the payload step put there (lowercase names; uninstall deletes only these and
    /// `uninstall::OWNED`).
    pub created: Vec<String>,
    pub installed_utc: String,
    pub updated_utc: String,
    /// The last install / verify result ("PASS ..." / "FAIL ...").
    pub last_result: String,
    /// The builder of the installed regulation ("regbuild" / "witchybnd"; empty = an install before P2, i.e.
    /// WitchyBND). `verify` checks the regulation by content when it differs from the release's builder.
    pub regulation_tool: String,
    /// P3: set while an update from this version is under way (an interrupted update resumes as an update and can
    /// be rolled back); cleared when it passed.
    pub update_from: String,
}

impl InstallState {
    pub fn new() -> InstallState {
        InstallState {
            schema: SCHEMA,
            what: "MW2 in Elden Ring install state (paths, step keys, hashes; the co-op secret is never stored here)".into(),
            ..Default::default()
        }
    }

    pub fn path(root: &Path) -> PathBuf {
        root.join(STATE_FILE)
    }

    /// The state of an install root (None when there is none or it does not parse).
    pub fn load(root: &Path) -> Option<InstallState> {
        let t = std::fs::read_to_string(Self::path(root)).ok()?;
        serde_json::from_str(&t).ok()
    }

    pub fn load_or_new(root: &Path) -> InstallState {
        Self::load(root).unwrap_or_default()
    }

    pub fn save(&self, root: &Path, guard: &Guard) -> std::io::Result<()> {
        let v = serde_json::to_value(self).map_err(std::io::Error::other)?;
        if has_password_key(&v) {
            return Err(std::io::Error::other("refusing to write a state file with a password field"));
        }
        let mut t = serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?;
        t.push('\n');
        guard.write_atomic(&Self::path(root), t)
    }

    pub fn step_key(&self, id: &str) -> Option<&str> {
        self.steps.get(id).map(|r| r.key.as_str())
    }

    pub fn record_step(&mut self, id: &str, key: &str) {
        self.steps.insert(id.to_string(), StepRecord { key: key.to_string(), done_utc: crate::util::utc_stamp() });
    }

    pub fn forget_step(&mut self, id: &str) {
        self.steps.remove(id);
    }
}

fn has_password_key(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => m.iter().any(|(k, v)| k.to_lowercase().contains("password") || has_password_key(v)),
        serde_json::Value::Array(a) => a.iter().any(has_password_key),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn round_trip_and_no_password() {
        let t = TempDir::new("state");
        let mut s = InstallState::new();
        s.mode = Mode::Coop;
        s.paths.er_game = Some(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game".into());
        s.paths.seamless_source = Some(r"C:\Users\p\Downloads\Seamless.zip".into());
        s.record_step("dumps", "abc");
        s.preflight.insert("build/natives/mw2er.dll".into(), "00".into());
        s.save(t.path(), &Guard::default()).unwrap();
        let text = std::fs::read_to_string(InstallState::path(t.path())).unwrap();
        assert!(!text.to_lowercase().contains("password"));
        assert!(text.contains("\"mode\": \"coop\""));
        let back = InstallState::load(t.path()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.step_key("dumps"), Some("abc"));
        assert!(has_password_key(&serde_json::json!({"a": [{"CoopPassword": 1}]})));
        assert_eq!(InstallState::load(&t.0.join("none")), None);
        assert_eq!(Mode::parse("Co-op"), Some(Mode::Coop));
        assert_eq!(Mode::parse("x"), None);
    }

    #[test]
    fn tolerant_load() {
        let t = TempDir::new("state2");
        t.write(STATE_FILE, "{\"schema\": 1, \"mode\": \"solo\", \"future_field\": 3}");
        let s = InstallState::load(t.path()).unwrap();
        assert_eq!(s.mode, Mode::Solo);
        assert!(s.steps.is_empty());
    }
}
