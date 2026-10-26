//! The release manifest: the version pins (today's `manifest\host_pins.json`), the host's hash manifest (today's
//! `manifest\host_manifest.json`, written by `coop_manifest.py write`), the pinned download sources, the release
//! version, the per-payload-file sha256 and the co-op id (docs/INSTALLER_DESIGN.md 2.4, 2.8).
//!
//! Two layouts are read: `manifest\release.json` (P1+: everything in one file) or the P0 friend package's two files.
//! The host-manifest check is kept exactly: `verify` here re-hashes the plain entries in Rust and leaves the two
//! canonicalised JSON entries (`canon_sha256`) to `coop_manifest.py verify` (Python's float formatting).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::util;

pub const RELEASE_JSON: &str = r"manifest\release.json";
pub const HOST_PINS: &str = r"manifest\host_pins.json";
pub const HOST_MANIFEST: &str = r"manifest\host_manifest.json";

/// The official WitchyBND 3.0.1.0 release asset (docs/INSTALLER_DESIGN.md 3.2 (c); the URL setup.ps1 P0 uses).
pub const WITCHY_URL: &str = "https://github.com/ividyon/WitchyBND/releases/download/3.0.1.0/WitchyBND-3.0.1.0-win-x64.zip";
pub const WITCHY_ZIP_NAME: &str = "WitchyBND-3.0.1.0-win-x64.zip";
pub const NEXUS_SEAMLESS: &str = "https://www.nexusmods.com/eldenring/mods/510?tab=files";
pub const ME3_RELEASES: &str = "https://github.com/garyttierney/me3/releases/tag/v0.13.0";
pub const VC_REDIST: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";
/// The official, unmodified me3 0.13.0 installer (decision D2: bundled into the release as `tools\me3\me3_installer.exe`;
/// this program never downloads it). sha256 = the asset digest GitHub publishes for the v0.13.0 release (verified on the
/// development copy in work\installer\cache\me3-0.13.0, Authenticode-signed by SignPath Foundation). Used when the
/// release's pins carry no `me3_installer_sha256`.
pub const ME3_INSTALLER_URL: &str = "https://github.com/garyttierney/me3/releases/download/v0.13.0/me3_installer.exe";
pub const ME3_INSTALLER_SHA256: &str = "a9326aef000f97a020062f376ff5863c3e0e212717603f40bf13be3b54484c1f";

/// The pins (field names = host_pins.json; unknown fields are ignored, missing ones are empty).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pins {
    pub er_exe_version: String,
    /// Free text in host_pins.json ("Steam build 25827938, the 2026-10-20 update"); the digits are the build id.
    pub er_app_version: String,
    pub vanilla_regulation_sha256: String,
    pub package_regulation_sha256: String,
    pub mw2_buildid: String,
    pub iw4sp_sha256: String,
    pub me3_version: String,
    pub seamless_version: String,
    pub ersc_sha256: String,
    pub witchybnd_zip_sha256: String,
    pub witchybnd_exe_sha256: String,
    pub paramdex_version: String,
    pub mw2er_dll_sha256: String,
    pub unlinker_sha256: String,
    pub oat_commit: String,
    /// P1: the bundled official me3 installer (empty = not bundled).
    pub me3_installer_sha256: String,
    /// P2: the builder the host's regulation package (and so the manifest) was made with: "regbuild" or "witchybnd"
    /// (empty = WitchyBND, every release before P2; see [`crate::regulation::Tool::of_release`]).
    pub regulation_builder: String,
    /// P2: sha256 of the package's unpacked BND4 (the 194 params; the same for both builders).
    pub package_bnd4_sha256: String,
    /// P2: sha256 of rows.json without built_utc / generator / package_regulation_sha256
    /// ([`crate::regulation::rows_core_sha256`]; the same for both builders).
    pub rows_json_core_sha256: String,
}

impl Pins {
    /// The me3 installer pin: the release's `me3_installer_sha256`, else [`ME3_INSTALLER_SHA256`].
    pub fn me3_installer_pin(&self) -> &str {
        if util::is_sha256(&self.me3_installer_sha256) { &self.me3_installer_sha256 } else { ME3_INSTALLER_SHA256 }
    }

    /// The Elden Ring Steam build id: `er_buildid` if a release file has it, else the first run of 6+ digits of
    /// `er_app_version`.
    pub fn er_buildid(&self) -> Option<String> {
        let s = &self.er_app_version;
        let mut run = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                run.push(c);
            } else {
                if run.len() >= 6 {
                    return Some(run);
                }
                run.clear();
            }
        }
        (run.len() >= 6).then_some(run)
    }
}

/// One host-manifest entry: a plain file hash, or a canonical-JSON hash (`coop_manifest.py` VOLATILE).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Entry {
    Plain { sha256: String, size: u64 },
    Canon { canon_sha256: String, drops: Vec<String> },
}

impl Entry {
    pub fn hash(&self) -> &str {
        match self {
            Entry::Plain { sha256, .. } => sha256,
            Entry::Canon { canon_sha256, .. } => canon_sha256,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostManifest {
    #[serde(default)]
    pub what: String,
    pub files: BTreeMap<String, Entry>,
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

/// A pinned download: the only URLs this program fetches from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinnedSource {
    pub url: String,
    pub sha256: String,
    pub file_name: String,
}

/// `manifest\release.json` (P1). Built by the release step from host_manifest + host_pins + the payload list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseFile {
    pub what: String,
    pub release_version: String,
    pub pins: Pins,
    pub manifest: HostManifest,
    /// Shipped payload files (relative, `/`) -> sha256.
    pub payload: BTreeMap<String, String>,
    /// Build-input key paths the release locks in mw2er.toml (the P3 settings merge takes them from the release).
    pub locked_toml: Vec<String>,
    /// P3: `owner/name` of the GitHub repository the opt-in "newer release" check asks (empty = no check offered).
    pub update_repo: String,
}

/// The loaded release, from either layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub pins: Pins,
    pub manifest: HostManifest,
    pub payload: BTreeMap<String, String>,
    /// Where it was read from (the package / payload root).
    pub dir: PathBuf,
    /// The manifest file the Python verifier is given (release.json embeds it; the P0 layout has its own file).
    pub manifest_path: PathBuf,
    /// `locked_toml` of release.json (empty for the P0 layout: the settings merge locks the build inputs anyway).
    pub locked_toml: Vec<String>,
    /// `update_repo` of release.json (empty = the GitHub check is hidden).
    pub update_repo: String,
}

#[derive(Debug)]
pub struct ReleaseError(pub String);
impl std::fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T, ReleaseError> {
    let t = fs::read_to_string(p).map_err(|e| ReleaseError(format!("cannot read {}: {e}", p.display())))?;
    serde_json::from_str(t.trim_start_matches('\u{feff}')).map_err(|e| ReleaseError(format!("{}: {e}", p.display())))
}

impl Release {
    /// True when `dir` holds a release (either layout).
    pub fn present(dir: &Path) -> bool {
        dir.join(RELEASE_JSON).is_file() || (dir.join(HOST_PINS).is_file() && dir.join(HOST_MANIFEST).is_file())
    }

    pub fn load(dir: &Path) -> Result<Release, ReleaseError> {
        let rj = dir.join(RELEASE_JSON);
        if rj.is_file() {
            let r: ReleaseFile = read_json(&rj)?;
            // coop_manifest.py verify reads "files" at the top level: release.json carries the manifest under
            // "manifest", so the release step also writes host_manifest.json; use it when present.
            let mp = dir.join(HOST_MANIFEST);
            return Ok(Release {
                version: r.release_version,
                pins: r.pins,
                manifest: r.manifest,
                payload: r.payload,
                dir: dir.to_path_buf(),
                manifest_path: if mp.is_file() { mp } else { rj },
                locked_toml: r.locked_toml,
                update_repo: r.update_repo.trim().to_string(),
            });
        }
        let pins: Pins = read_json(&dir.join(HOST_PINS))?;
        let manifest: HostManifest = read_json(&dir.join(HOST_MANIFEST))?;
        Ok(Release {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pins,
            manifest,
            payload: BTreeMap::new(),
            dir: dir.to_path_buf(),
            manifest_path: dir.join(HOST_MANIFEST),
            locked_toml: Vec::new(),
            update_repo: String::new(),
        })
    }

    /// The pinned WitchyBND source (None when the release has no valid zip pin: no download then).
    pub fn witchy_source(&self) -> Option<PinnedSource> {
        util::is_sha256(&self.pins.witchybnd_zip_sha256).then(|| PinnedSource {
            url: WITCHY_URL.into(),
            sha256: self.pins.witchybnd_zip_sha256.to_lowercase(),
            file_name: WITCHY_ZIP_NAME.into(),
        })
    }

    /// The co-op id: the first 8 hex of sha256 over every manifest entry (`<path> <hash>\n`, sorted) and
    /// `seamless <ersc pin>\n`. Equal ids = byte-identical builds and the same Seamless (docs 2.8).
    pub fn coop_id(&self) -> String {
        let mut s = String::new();
        for (k, e) in &self.manifest.files {
            s.push_str(&format!("{k} {}\n", e.hash().to_lowercase()));
        }
        s.push_str(&format!("seamless {}\n", self.pins.ersc_sha256.to_lowercase()));
        util::sha256_bytes(s.as_bytes())[..8].to_string()
    }
}

/// Which part of the manifest to check (`coop_manifest.py --only`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Only {
    Converted,
    Regulation,
    All,
}

impl Only {
    pub fn wants(self, k: &str) -> bool {
        match self {
            Only::Converted => k.starts_with("converted/"),
            Only::Regulation => k == "build/package/regulation.bin" || k == "build/rows.json",
            Only::All => true,
        }
    }

    pub fn arg(self) -> &'static str {
        match self {
            Only::Converted => "converted",
            Only::Regulation => "regulation",
            Only::All => "all",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerifyReport {
    pub checked: usize,
    pub ok: usize,
    pub mismatched: Vec<String>,
    pub missing: Vec<String>,
    /// Canonical-JSON entries not checked here (left to coop_manifest.py).
    pub deferred: Vec<String>,
    /// Files under converted\ the manifest does not list (harmless; reported like coop_manifest.py).
    pub extra: Vec<String>,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        self.mismatched.is_empty() && self.missing.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} {}/{} plain files match ({} mismatched, {} missing, {} canonical-JSON left to coop_manifest.py, {} extra)",
            if self.passed() { "PASS" } else { "FAIL" },
            self.ok,
            self.checked,
            self.mismatched.len(),
            self.missing.len(),
            self.deferred.len(),
            self.extra.len()
        )
    }
}

/// Re-hashes the plain entries of the manifest under `root` (read-only).
pub fn verify_plain(m: &HostManifest, root: &Path, only: Only) -> VerifyReport {
    let mut r = VerifyReport::default();
    for (k, e) in m.files.iter().filter(|(k, _)| only.wants(k)) {
        let p = root.join(k.replace('/', "\\"));
        match e {
            Entry::Canon { .. } => {
                if p.is_file() {
                    r.deferred.push(k.clone());
                } else {
                    r.checked += 1;
                    r.missing.push(k.clone());
                }
            }
            Entry::Plain { sha256, size } => {
                r.checked += 1;
                let Ok(md) = fs::metadata(&p) else {
                    r.missing.push(k.clone());
                    continue;
                };
                if md.len() == *size && util::sha256_file(&p).is_ok_and(|h| h.eq_ignore_ascii_case(sha256)) {
                    r.ok += 1;
                } else {
                    r.mismatched.push(k.clone());
                }
            }
        }
    }
    if matches!(only, Only::All | Only::Converted)
        && let Ok(files) = util::walk_files(&root.join("converted"))
    {
        for f in files {
            if let Ok(rel) = f.strip_prefix(root) {
                let k = util::rel_key(rel);
                if !m.files.contains_key(&k) {
                    r.extra.push(k);
                }
            }
        }
    }
    r
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::util::testutil::TempDir;

    /// A P0-layout package in `t/pkg` with pins matching the steam fixture, a DLL, toml, templates, scripts.
    pub fn package(t: &TempDir) -> PathBuf {
        let dll = b"fake mw2er.dll";
        t.write("pkg/build/natives/mw2er.dll", dll);
        // fits the release toml policy (friend_toml), two probes on (the payload step forces them off)
        t.write("pkg/mw2er.toml", crate::friend_toml::fixture::POLICY_TOML);
        t.write("pkg/profiles/mw2er-coop.template.me3", "profileVersion = \"v1\"\nsavefile = \"ER0000.mw2coop.sl2\"\n[[natives]]\npath = '@@ERSC@@'\n[[natives]]\npath = '@@DLL@@'\n[[packages]]\npath = '@@PACKAGE@@'\n");
        t.write("pkg/profiles/mw2er-solo.template.me3", "profileVersion = \"v1\"\nsavefile = \"ER0000.mw2er.sl2\"\n[[natives]]\npath = '@@DLL@@'\n[[packages]]\npath = '@@PACKAGE@@'\n");
        for s in ["friend_dump.py", "coop_manifest.py", "build_regulation.py", "convert_weapons.py"] {
            t.write(&format!("pkg/tools/scripts/{s}"), format!("# {s}\n"));
        }
        t.write("pkg/tools/oat/bin/Unlinker.exe", b"unlinker");
        t.write("pkg/tools/bin/sndconv.exe", b"sndconv");
        t.write("pkg/setup.ps1", "# p0 fallback\n");
        let pins = serde_json::json!({
            "what": "test",
            "er_exe_version": "2.7.2.0",
            "er_app_version": "Steam build 25827938, the 2026-10-20 update",
            "vanilla_regulation_sha256": util::sha256_bytes(b"vanilla regulation"),
            "package_regulation_sha256": util::sha256_bytes(b"package regulation"),
            "mw2_buildid": "25374375",
            "iw4sp_sha256": util::sha256_bytes(b"iw4sp"),
            "me3_version": "0.13.0",
            "seamless_version": "2.0.1",
            "ersc_sha256": util::sha256_bytes(b"ersc"),
            "witchybnd_zip_sha256": "a3e6b2a0f7eac13f5e83b6602a1149322439c0662baa140ecdd84be28af50364",
            "witchybnd_exe_sha256": util::sha256_bytes(b"witchy exe"),
            "paramdex_version": "1_17_3_0034L",
            "mw2er_dll_sha256": util::sha256_bytes(dll),
            "unlinker_sha256": util::sha256_bytes(b"unlinker"),
            "oat_commit": "df203f9fe5820ea8f63e50d524fb437861f8e427"
        });
        t.write("pkg/manifest/host_pins.json", serde_json::to_string_pretty(&pins).unwrap());
        let man = serde_json::json!({
            "counts": {"converted": 1, "total": 4},
            "files": {
                "build/natives/mw2er.dll": {"sha256": util::sha256_bytes(dll), "size": dll.len()},
                "build/package/regulation.bin": {"sha256": util::sha256_bytes(b"package regulation"), "size": 18},
                "build/rows.json": {"canon_sha256": "73b7", "drops": ["built_utc"]},
                "converted/weapons.json": {"sha256": util::sha256_bytes(b"{}"), "size": 2}
            }
        });
        t.write("pkg/manifest/host_manifest.json", serde_json::to_string_pretty(&man).unwrap());
        t.0.join("pkg")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn loads_the_p0_layout_and_the_real_pin_format() {
        let t = TempDir::new("rel");
        let pkg = fixture::package(&t);
        assert!(Release::present(&pkg));
        let r = Release::load(&pkg).unwrap();
        assert_eq!(r.pins.er_exe_version, "2.7.2.0");
        assert_eq!(r.pins.er_buildid().as_deref(), Some("25827938"));
        assert_eq!(r.manifest.files.len(), 4);
        assert!(matches!(r.manifest.files["build/rows.json"], Entry::Canon { .. }));
        assert_eq!(r.manifest_path, pkg.join(HOST_MANIFEST));
        assert_eq!(r.witchy_source().unwrap().url, WITCHY_URL);
        assert_eq!(r.coop_id().len(), 8);
        let mut r2 = r.clone();
        r2.pins.ersc_sha256 = "00".into();
        assert_ne!(r.coop_id(), r2.coop_id(), "the Seamless pin is part of the co-op id");
        assert!(!Release::present(&t.0));
        assert!(Release::load(&t.0).is_err());
    }

    #[test]
    fn release_json_layout() {
        let t = TempDir::new("rel-json");
        let rf = ReleaseFile {
            release_version: "0.3.0".into(),
            pins: Pins { er_exe_version: "2.7.2.0".into(), ..Default::default() },
            ..Default::default()
        };
        t.write("manifest/release.json", serde_json::to_string(&rf).unwrap());
        let r = Release::load(t.path()).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.witchy_source(), None, "no pin, no download");
        assert_eq!(r.manifest_path, t.0.join(RELEASE_JSON));
    }

    /// The layout `tools\scripts\release_tool.py manifest` writes (extra informational keys are ignored).
    #[test]
    fn release_json_as_make_release_writes_it() {
        let t = TempDir::new("rel-tool");
        let pkg = fixture::package(&t);
        let pins: serde_json::Value = serde_json::from_slice(&std::fs::read(pkg.join(HOST_PINS)).unwrap()).unwrap();
        let man: serde_json::Value = serde_json::from_slice(&std::fs::read(pkg.join(HOST_MANIFEST)).unwrap()).unwrap();
        let p0_id = Release::load(&pkg).unwrap().coop_id();
        let mut p = pins.clone();
        p["me3_installer_sha256"] = ME3_INSTALLER_SHA256.into();
        p["python_embed_sha256"] = crate::payload::PYTHON_EMBED_SHA256.into();
        let rj = serde_json::json!({
            "what": "MW2 in Elden Ring release manifest",
            "format": 1,
            "release_version": "0.2.0",
            "pins": p,
            "manifest": man,
            "payload": {"mw2er.toml": util::sha256_file(&pkg.join("mw2er.toml")).unwrap(), "manifest/host_pins.json": "ab"},
            "locked_toml": ["damage", "regulation*", "arsenal.guns"],
            "sources": [{"name": "witchybnd", "url": WITCHY_URL, "sha256": pins["witchybnd_zip_sha256"], "file_name": WITCHY_ZIP_NAME, "how": "downloaded at install time on a click"}],
            "coop_id": "0badc0de",
            "update_repo": "owner/mw2iner"
        });
        t.write("pkg/manifest/release.json", serde_json::to_string_pretty(&rj).unwrap());
        let r = Release::load(&pkg).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.pins.me3_installer_pin(), ME3_INSTALLER_SHA256);
        assert_eq!(r.manifest.files.len(), 4);
        assert_eq!(r.payload.len(), 2);
        assert_eq!(r.manifest_path, pkg.join(HOST_MANIFEST), "coop_manifest.py gets the plain host manifest");
        assert_eq!(r.coop_id(), p0_id, "the same host manifest + Seamless pin = the same co-op id in both layouts");
        assert_eq!(r.locked_toml, vec!["damage".to_string(), "regulation*".into(), "arsenal.guns".into()]);
        assert_eq!(r.update_repo, "owner/mw2iner");
        assert!(Release::load(&t.0.join("pkg")).is_ok());
        assert!(fixture_p0_has_no_update_repo(&t));
    }

    fn fixture_p0_has_no_update_repo(t: &TempDir) -> bool {
        std::fs::remove_file(t.0.join("pkg").join(RELEASE_JSON)).unwrap();
        let r = Release::load(&t.0.join("pkg")).unwrap();
        r.update_repo.is_empty() && r.locked_toml.is_empty()
    }

    #[test]
    fn plain_verify_finds_mismatch_missing_and_extra() {
        let t = TempDir::new("verify");
        let pkg = fixture::package(&t);
        let r = Release::load(&pkg).unwrap();
        // the package has the DLL only
        let v = verify_plain(&r.manifest, &pkg, Only::All);
        assert_eq!(v.ok, 1);
        assert_eq!(v.missing.len(), 3, "regulation, weapons.json, rows.json");
        assert!(!v.passed());
        t.write("pkg/build/package/regulation.bin", b"package regulation");
        t.write("pkg/build/rows.json", "{}");
        t.write("pkg/converted/weapons.json", b"{]");
        t.write("pkg/converted/extra.bin", b"x");
        let v = verify_plain(&r.manifest, &pkg, Only::All);
        assert_eq!(v.mismatched, vec!["converted/weapons.json".to_string()]);
        assert_eq!(v.deferred, vec!["build/rows.json".to_string()]);
        assert_eq!(v.extra, vec!["converted/extra.bin".to_string()]);
        t.write("pkg/converted/weapons.json", b"{}");
        let v = verify_plain(&r.manifest, &pkg, Only::Converted);
        assert!(v.passed());
        assert_eq!(v.checked, 1);
        assert!(verify_plain(&r.manifest, &pkg, Only::Regulation).passed());
        assert!(v.summary().starts_with("PASS"));
    }
}
