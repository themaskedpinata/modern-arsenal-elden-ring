//! The regulation step's builder (docs/INSTALLER_DESIGN.md P2): the Rust regulation builder (`regbuild`, in-process,
//! no WitchyBND, no download) by default, WitchyBND under `build_regulation.py` as the fallback
//! (`--regulation-tool witchy`, the wizard's Advanced box) until the in-game smoke test of the regbuild package passed.
//!
//! Which builder a release's manifest was made with is the release's pin `regulation_builder` ("regbuild" /
//! "witchybnd"; empty = WitchyBND: every release before P2 and the P0 friend package). The two builders give the same
//! params byte for byte (MODLOG "installer P2 builder": 194/194 params, the whole BND4) but different zstd frames, so
//! `regulation.bin` (and the `generator` / `package_regulation_sha256` lines of rows.json) differ:
//! - the builder == the release's: the host-manifest check as before (sha256 of the file, canonical rows.json);
//! - a different builder: the two regulation entries are checked by content instead ([`check_content`]: sha256 of
//!   the unpacked BND4 == `package_bnd4_sha256`, rows.json without its builder lines == `rows_json_core_sha256`), and
//!   the co-op id becomes [`coop_id`] (it differs from the host's: equal ids still mean byte-identical installs).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::release::{HostManifest, Pins, Release};
use crate::util;

/// Root-relative outputs of the regulation step.
pub const PACKAGE: &str = r"build\package\regulation.bin";
pub const ROWS_JSON: &str = r"build\rows.json";
/// The host-manifest keys of those outputs.
pub const MANIFEST_KEYS: [&str; 2] = ["build/package/regulation.bin", "build/rows.json"];
/// rows.json keys that depend on the builder or the build time (left out of [`rows_core_sha256`]).
pub const ROWS_VOLATILE: [&str; 3] = ["built_utc", "generator", "package_regulation_sha256"];
/// The regulation step's own log inside `<root>\logs\install` (the regbuild path; WitchyBND logs build_regulation.log).
pub const LOG_NAME: &str = "regulation.log";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// The Rust regulation builder (mod\regbuild), in-process.
    #[default]
    Regbuild,
    /// WitchyBND 3.0.1.0 under tools\scripts\build_regulation.py (the P1 path; fallback).
    Witchy,
}

impl Tool {
    /// The value of the release pin / `state\install.json` / `--regulation-tool`.
    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Regbuild => "regbuild",
            Tool::Witchy => "witchybnd",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Regbuild => "built in (regbuild)",
            Tool::Witchy => "WitchyBND 3.0.1.0 (fallback)",
        }
    }

    pub fn parse(s: &str) -> Option<Tool> {
        match s.trim().to_ascii_lowercase().as_str() {
            "regbuild" | "rust" | "builtin" | "built-in" => Some(Tool::Regbuild),
            "witchy" | "witchybnd" => Some(Tool::Witchy),
            _ => None,
        }
    }

    /// The builder a release's manifest was made with (pin `regulation_builder`; empty or unknown = WitchyBND).
    pub fn of_release(pins: &Pins) -> Tool {
        Tool::parse(&pins.regulation_builder).unwrap_or(Tool::Witchy)
    }
}

/// The builder's identity in the regulation step's skip key (a different builder, or a different libzstd, rebuilds).
pub fn builder_key(tool: Tool) -> String {
    match tool {
        Tool::Regbuild => regbuild::BUILDER_ID.to_string(),
        Tool::Witchy => "witchybnd 3.0.1.0 + tools/scripts/build_regulation.py".to_string(),
    }
}

/// What the in-process build gets.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildJob {
    /// The install root (mw2er.toml, converted\; outputs [`PACKAGE`] and [`ROWS_JSON`]).
    pub root: PathBuf,
    /// The game's vanilla regulation.bin (only read).
    pub vanilla: PathBuf,
}

/// Runs the build: (job, cancel flag, progress line) -> the step's message. Tests put a fake one into the session.
pub type Builder = Arc<dyn Fn(&BuildJob, &AtomicBool, &mut dyn FnMut(&str)) -> Result<String, String> + Send + Sync>;

/// The real builder: [`regbuild::build`] (vanilla sha256 pin, plan, apply, BND4 -> DCX_ZSTD -> AES, re-read + verify,
/// publish through a temp file + rename). The cancel flag is checked between its phases.
pub fn system_builder() -> Builder {
    Arc::new(|job: &BuildJob, cancel: &AtomicBool, line: &mut dyn FnMut(&str)| {
        let o = regbuild::BuildOptions {
            root: job.root.clone(),
            vanilla: job.vanilla.clone(),
            package: job.root.join(PACKAGE),
            rows_json: job.root.join(ROWS_JSON),
            built_utc: None,
            cancel: Some(cancel),
        };
        let r = regbuild::build(&o, line)?;
        for l in &r.lines {
            line(l);
        }
        let secs: f64 = r.timings.iter().map(|t| t.1).sum();
        line(&format!("timings: {}", r.timings.iter().map(|(n, s)| format!("{n} {s:.1} s")).collect::<Vec<_>>().join(", ")));
        Ok(format!(
            "REGULATION PASS (regbuild, {secs:.0} s): package {} ({} B), added {}",
            util::short(&r.package_sha256),
            r.package_len,
            r.added.iter().map(|(p, n)| format!("{p} {n}")).collect::<Vec<_>>().join(", ")
        ))
    })
}

/// Canonical text of a JSON value: object keys sorted, no spaces (the same code on the host and the player's PC, so
/// float formatting is not an issue).
fn canonical(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// sha256 of rows.json without [`ROWS_VOLATILE`] (builder- and time-independent: equal for the WitchyBND and the
/// regbuild build of the same inputs).
pub fn rows_core_sha256(text: &str) -> Result<String, String> {
    let mut v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("rows.json: {e}"))?;
    let m = v.as_object_mut().ok_or("rows.json is not an object")?;
    for k in ROWS_VOLATILE {
        m.remove(k);
    }
    let mut s = String::new();
    canonical(&v, &mut s);
    Ok(util::sha256_bytes(s.as_bytes()))
}

/// The builder-independent facts of an install's (or the host's) regulation outputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Content {
    pub package_sha256: String,
    pub package_len: u64,
    /// sha256 of the decrypted, decompressed BND4 (the 194 params).
    pub bnd4_sha256: String,
    pub rows_core_sha256: String,
    /// rows.json `generator` ("build_regulation.py" = WitchyBND, "regbuild <ver>").
    pub rows_generator: String,
    pub rows_package_sha256: String,
    pub rows_vanilla_sha256: String,
}

impl Content {
    /// The builder rows.json names.
    pub fn tool(&self) -> Tool {
        if self.rows_generator.starts_with("regbuild") { Tool::Regbuild } else { Tool::Witchy }
    }
}

/// Reads `<root>\build\package\regulation.bin` (AES / DCX_ZSTD / BND4 through regbuild's reader) and rows.json.
pub fn content(root: &Path) -> Result<Content, String> {
    let pkg = root.join(PACKAGE);
    let bytes = std::fs::read(&pkg).map_err(|e| format!("{}: {e}", pkg.display()))?;
    let u = regbuild::unpack(&bytes).map_err(|e| format!("{}: {e}", pkg.display()))?;
    let rows_p = root.join(ROWS_JSON);
    let text = std::fs::read_to_string(&rows_p).map_err(|e| format!("{}: {e}", rows_p.display()))?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("rows.json: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(Content {
        package_sha256: util::sha256_bytes(&bytes),
        package_len: bytes.len() as u64,
        bnd4_sha256: util::sha256_bytes(&u.bnd),
        rows_core_sha256: rows_core_sha256(&text)?,
        rows_generator: s("generator"),
        rows_package_sha256: s("package_regulation_sha256"),
        rows_vanilla_sha256: s("vanilla_sha256"),
    })
}

/// The release check of the regulation outputs by content (a builder other than the release's): the params, the
/// rows.json data and its self-consistency. Ok = a one-line summary; Err = every problem.
pub fn check_content(pins: &Pins, root: &Path) -> Result<String, String> {
    if !util::is_sha256(&pins.package_bnd4_sha256) || !util::is_sha256(&pins.rows_json_core_sha256) {
        return Err(format!(
            "this release has no builder-independent regulation pins (package_bnd4_sha256, rows_json_core_sha256): install with the builder it was made with (--regulation-tool {})",
            Tool::of_release(pins).as_str()
        ));
    }
    let c = content(root)?;
    let mut problems = Vec::new();
    if !c.bnd4_sha256.eq_ignore_ascii_case(&pins.package_bnd4_sha256) {
        problems.push(format!(
            "regulation.bin: the unpacked params differ from the release's (BND4 sha256 {}, want {})",
            util::short(&c.bnd4_sha256),
            util::short(&pins.package_bnd4_sha256)
        ));
    }
    if !c.rows_core_sha256.eq_ignore_ascii_case(&pins.rows_json_core_sha256) {
        problems.push(format!("rows.json: its rows differ from the release's ({} != {})", util::short(&c.rows_core_sha256), util::short(&pins.rows_json_core_sha256)));
    }
    if !c.rows_package_sha256.eq_ignore_ascii_case(&c.package_sha256) {
        problems.push("rows.json: package_regulation_sha256 is not the sha256 of build\\package\\regulation.bin".into());
    }
    if util::is_sha256(&pins.vanilla_regulation_sha256) && !c.rows_vanilla_sha256.eq_ignore_ascii_case(&pins.vanilla_regulation_sha256) {
        problems.push("rows.json: built from another vanilla regulation.bin than the release's".into());
    }
    if problems.is_empty() {
        Ok(format!(
            "regulation by content: params (BND4 {}) and rows.json == the release's; the file ({}, {}) differs from the release's {} build only in its compression",
            util::short(&c.bnd4_sha256),
            c.rows_generator,
            util::short(&c.package_sha256),
            Tool::of_release(pins).as_str()
        ))
    } else {
        Err(problems.join("; "))
    }
}

/// True when an install built with `tool` is checked by content (the release was made with the other builder).
pub fn by_content(pins: &Pins, tool: Tool) -> bool {
    tool != Tool::of_release(pins)
}

/// The host manifest without the two regulation entries (checked by content instead).
pub fn manifest_without_regulation(m: &HostManifest) -> HostManifest {
    let mut out = m.clone();
    out.files.retain(|k, _| !MANIFEST_KEYS.contains(&k.as_str()));
    out
}

/// The co-op id of an install: the release's when built with the release's builder; otherwise derived from the
/// release's id, the builder and the regulation file actually built (two installs on the same fallback still match).
pub fn coop_id(release: &Release, tool: Tool, package_sha256: Option<&str>) -> String {
    let id = release.coop_id();
    if !by_content(&release.pins, tool) {
        return id;
    }
    let s = format!("{id}\nregulation {} {}\n", tool.as_str(), package_sha256.unwrap_or("?").to_lowercase());
    util::sha256_bytes(s.as_bytes())[..8].to_string()
}

#[cfg(test)]
pub(crate) mod fixture {
    /// A small, real regulation.bin (AES + DCX_ZSTD + BND4 of two PARAM files) through regbuild's writer; `tag`
    /// changes one data byte (other params), `level` the zstd level (another "builder": same BND4, other bytes).
    pub fn regulation(tag: u8, level: i32) -> Vec<u8> {
        let p = regbuild::param::build_fixture("X_ST", 4, &[(1, "", vec![1, 2, 3, tag])]);
        let b = regbuild::bnd4::build_fixture(regbuild::REGULATION_VERSION, &[("X.param", &p), ("Y.param", b"yy")]);
        let mut tmpl = [0u8; regbuild::dcx::HEADER_LEN];
        tmpl[..4].copy_from_slice(b"DCX\0");
        for (o, v) in [(4usize, 0x11000u32), (8, 0x18), (0xC, 0x24), (0x10, 0x44), (0x14, 0x4C), (0x2C, 0x20), (0x48, 8)] {
            tmpl[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        tmpl[0x18..0x1C].copy_from_slice(b"DCS\0");
        tmpl[0x24..0x2C].copy_from_slice(b"DCP\0ZSTD");
        tmpl[0x30] = 21;
        tmpl[0x44..0x48].copy_from_slice(b"DCA\0");
        let frame = regbuild::dcx::compress_level(&b, level).unwrap();
        regbuild::crypto::encrypt(&regbuild::dcx::write(&tmpl, b.len(), &frame).unwrap())
    }

    /// rows.json as a builder writes it.
    pub fn rows(generator: &str, package_sha: &str, vanilla_sha: &str, k: f64) -> String {
        format!(
            "{{\n  \"schema\": 1,\n  \"generator\": \"{generator}\",\n  \"built_utc\": \"20261022T000000Z\",\n  \"vanilla_sha256\": \"{vanilla_sha}\",\n  \"package_regulation_sha256\": \"{package_sha}\",\n  \"k\": {k:?},\n  \"bullet\": {{\"life_s\": 0.15, \"speed_mps\": 999.0}}\n}}\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::path::Path;

    #[test]
    fn tool_names_and_the_release_default() {
        assert_eq!(Tool::parse("witchy"), Some(Tool::Witchy));
        assert_eq!(Tool::parse("WitchyBND"), Some(Tool::Witchy));
        assert_eq!(Tool::parse("regbuild"), Some(Tool::Regbuild));
        assert_eq!(Tool::parse("oodle"), None);
        for t in [Tool::Regbuild, Tool::Witchy] {
            assert_eq!(Tool::parse(t.as_str()), Some(t));
        }
        let mut p = Pins::default();
        assert_eq!(Tool::of_release(&p), Tool::Witchy, "releases before P2 were built with WitchyBND");
        p.regulation_builder = "regbuild".into();
        assert_eq!(Tool::of_release(&p), Tool::Regbuild);
        assert!(by_content(&p, Tool::Witchy) && !by_content(&p, Tool::Regbuild));
        assert_ne!(builder_key(Tool::Regbuild), builder_key(Tool::Witchy));
        assert!(builder_key(Tool::Regbuild).contains("libzstd 1.5.7"));
    }

    #[test]
    fn rows_core_ignores_builder_lines_only() {
        let a = fixture::rows("build_regulation.py", &"a".repeat(64), &"v".repeat(64), 4.0);
        let b = fixture::rows("regbuild 0.2.0", &"b".repeat(64), &"v".repeat(64), 4.0);
        assert_eq!(rows_core_sha256(&a).unwrap(), rows_core_sha256(&b).unwrap());
        let c = fixture::rows("regbuild 0.2.0", &"b".repeat(64), &"v".repeat(64), 4.5);
        assert_ne!(rows_core_sha256(&a).unwrap(), rows_core_sha256(&c).unwrap());
        // key order and whitespace do not matter, the vanilla line does
        let d = "{\"bullet\":{\"speed_mps\":999.0,\"life_s\":0.15},\"k\":4.0,\"schema\":1,\"vanilla_sha256\":\"".to_string() + &"v".repeat(64) + "\"}";
        assert_eq!(rows_core_sha256(&a).unwrap(), rows_core_sha256(&d).unwrap());
        assert_ne!(rows_core_sha256(&a).unwrap(), rows_core_sha256(&fixture::rows("x", "y", &"w".repeat(64), 4.0)).unwrap());
        assert!(rows_core_sha256("[1]").is_err() && rows_core_sha256("{").is_err());
    }

    fn put(t: &TempDir, reg: &[u8], generator: &str, k: f64) -> String {
        let sha = util::sha256_bytes(reg);
        t.write(r"root\build\package\regulation.bin", reg);
        t.write(r"root\build\rows.json", fixture::rows(generator, &sha, &"v".repeat(64), k));
        sha
    }

    #[test]
    fn content_check_accepts_the_other_builder_and_catches_changes() {
        let t = TempDir::new("regcontent");
        let root = t.0.join("root");
        // the "release" build (level 21) and the "fallback" build (level 3): same BND4, other bytes
        let host = fixture::regulation(4, 21);
        let other = fixture::regulation(4, 3);
        assert_ne!(host, other);
        put(&t, &host, "regbuild 0.2.0", 4.0);
        let h = content(&root).unwrap();
        assert_eq!(h.tool(), Tool::Regbuild);
        let mut pins = Pins {
            regulation_builder: "regbuild".into(),
            vanilla_regulation_sha256: "v".repeat(64),
            package_bnd4_sha256: h.bnd4_sha256.clone(),
            rows_json_core_sha256: h.rows_core_sha256.clone(),
            ..Default::default()
        };
        put(&t, &other, "build_regulation.py", 4.0);
        let c = content(&root).unwrap();
        assert_eq!(c.tool(), Tool::Witchy);
        assert_ne!(c.package_sha256, h.package_sha256);
        let ok = check_content(&pins, &root).unwrap();
        assert!(ok.contains("only in its compression"), "{ok}");
        // other params
        put(&t, &fixture::regulation(5, 3), "build_regulation.py", 4.0);
        assert!(check_content(&pins, &root).unwrap_err().contains("params differ"));
        // other rows
        put(&t, &other, "build_regulation.py", 4.5);
        assert!(check_content(&pins, &root).unwrap_err().contains("rows differ"));
        // rows.json naming another package
        put(&t, &other, "build_regulation.py", 4.0);
        t.write(r"root\build\rows.json", fixture::rows("build_regulation.py", &"0".repeat(64), &"v".repeat(64), 4.0));
        assert!(check_content(&pins, &root).unwrap_err().contains("package_regulation_sha256"));
        // not a regulation at all
        t.write(r"root\build\package\regulation.bin", b"package regulation");
        assert!(check_content(&pins, &root).is_err());
        // a release without the content pins: refused with the way out
        pins.package_bnd4_sha256.clear();
        assert!(check_content(&pins, &root).unwrap_err().contains("--regulation-tool regbuild"));
    }

    /// Real data (run on request: `cargo test -p installer --lib real_regulation -- --ignored`): the installer's
    /// builder ([`system_builder`], i.e. regbuild in-process) on a temporary root holding a copy of the repository's
    /// mw2er.toml + the converted files it reads, from the staged vanilla copy work\regulation\vanilla\regulation.bin
    /// (read only), gives the params and rows of the host's current build\package (WitchyBND), so the fallback's
    /// content check accepts the host build against regbuild pins and vice versa. Writes only into %TEMP%.
    #[test]
    #[ignore = "real data: needs the staged vanilla regulation.bin and the host build; ~20 s"]
    fn real_regulation_matches_the_host_build() {
        let repo = util::normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\.."));
        let vanilla = repo.join(r"work\regulation\vanilla\regulation.bin");
        assert!(vanilla.is_file() && repo.join(PACKAGE).is_file(), "no staged vanilla / host build");
        let t = TempDir::new("regreal");
        let copy = |rel: &str| {
            let to = t.0.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(repo.join(rel), to).unwrap();
        };
        for rel in ["mw2er.toml", r"converted\weapons.json", r"converted\ac130\weapons_ac130.json", r"converted\guns\index.json"] {
            copy(rel);
        }
        for e in std::fs::read_dir(repo.join(r"converted\guns")).unwrap().flatten() {
            if e.path().join("stats.json").is_file() {
                copy(&format!(r"converted\guns\{}\stats.json", e.file_name().to_string_lossy()));
            }
        }
        std::fs::create_dir_all(t.0.join(r"build\package")).unwrap();
        let mut lines = Vec::new();
        let msg = (system_builder())(&BuildJob { root: t.0.clone(), vanilla }, &AtomicBool::new(false), &mut |l: &str| lines.push(l.to_string())).unwrap();
        assert!(msg.starts_with("REGULATION PASS (regbuild"), "{msg}");
        let ours = content(&t.0).unwrap();
        let host = content(&repo).unwrap();
        eprintln!("{msg}\n{}\nours {ours:?}\nhost {host:?}", lines.join("\n"));
        assert_eq!(ours.tool(), Tool::Regbuild);
        assert_eq!(ours.bnd4_sha256, host.bnd4_sha256, "the same 194 params, byte for byte");
        assert_eq!(ours.rows_core_sha256, host.rows_core_sha256, "the same rows.json data");
        assert_eq!(ours.rows_package_sha256, ours.package_sha256);
        // a regbuild release's pins accept the host's WitchyBND build by content (and the other way round)
        let pins = |c: &Content, b: Tool| Pins {
            regulation_builder: b.as_str().into(),
            vanilla_regulation_sha256: c.rows_vanilla_sha256.clone(),
            package_bnd4_sha256: c.bnd4_sha256.clone(),
            rows_json_core_sha256: c.rows_core_sha256.clone(),
            ..Default::default()
        };
        check_content(&pins(&ours, Tool::Regbuild), &repo).unwrap();
        check_content(&pins(&host, host.tool()), &t.0).unwrap();
    }

    #[test]
    fn manifest_and_coop_id_of_the_fallback() {
        let t = TempDir::new("regcoop");
        let pkg = crate::release::fixture::package(&t);
        let mut rel = Release::load(&pkg).unwrap();
        assert_eq!(Tool::of_release(&rel.pins), Tool::Witchy);
        let m = manifest_without_regulation(&rel.manifest);
        assert_eq!(m.files.len(), rel.manifest.files.len() - 2);
        assert!(MANIFEST_KEYS.iter().all(|k| !m.files.contains_key(*k)));
        // the release's builder: the release's id, whatever the file
        assert_eq!(coop_id(&rel, Tool::Witchy, Some("ab")), rel.coop_id());
        rel.pins.regulation_builder = "regbuild".into();
        assert_eq!(coop_id(&rel, Tool::Regbuild, None), rel.coop_id());
        // the fallback: another id, equal for equal files
        let a = coop_id(&rel, Tool::Witchy, Some("AB"));
        assert_ne!(a, rel.coop_id());
        assert_eq!(a, coop_id(&rel, Tool::Witchy, Some("ab")));
        assert_ne!(a, coop_id(&rel, Tool::Witchy, Some("cd")));
        assert_eq!(a.len(), 8);
    }
}
