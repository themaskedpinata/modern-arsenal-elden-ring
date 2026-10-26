//! The install plan (docs/INSTALLER_DESIGN.md 2.3 page 6, 2.8 "Incremental"): an ordered list of steps, each with
//! its inputs (label -> value or hash), its root-relative outputs, the folders it clears first, a skip key (sha256
//! over the inputs, chained through the steps it depends on) and a skip policy. [`build`] compares the keys with
//! `state\install.json` and decides run / skip per step; the same plan drives `--cli install`, `--cli dry-run` and
//! the wizard's build page.
//!
//! The steps are setup.ps1's 8 steps, split where the skip rules differ:
//! preflight, save backup, payload, Python runtime, Seamless (co-op), WitchyBND (only with the WitchyBND regulation
//! builder, the P2 fallback), MW2 dumps, converters, verify converted, regulation (the in-process Rust builder by
//! default), verify all, cleanup, profiles, finish (exe copy, shortcuts, registry, preflight hashes).
//! Resume: a step's record is dropped before it runs and written only when it passed, so an interrupted or
//! cancelled install reruns exactly the unfinished steps (the dumps of an interrupted build are reused).

use std::path::{Path, PathBuf};

use crate::regulation::{self, Tool};
use crate::release::Release;
use crate::state::{InstallState, Mode};
use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StepId {
    Preflight,
    SaveBackup,
    Payload,
    Runtime,
    Seamless,
    Witchy,
    Dumps,
    Converters,
    VerifyConverted,
    Regulation,
    VerifyAll,
    Cleanup,
    Profiles,
    Finish,
}

impl StepId {
    pub const ALL: [StepId; 14] = [
        StepId::Preflight,
        StepId::SaveBackup,
        StepId::Payload,
        StepId::Runtime,
        StepId::Seamless,
        StepId::Witchy,
        StepId::Dumps,
        StepId::Converters,
        StepId::VerifyConverted,
        StepId::Regulation,
        StepId::VerifyAll,
        StepId::Cleanup,
        StepId::Profiles,
        StepId::Finish,
    ];

    /// Stable id (state file keys, log file names).
    pub fn as_str(self) -> &'static str {
        match self {
            StepId::Preflight => "preflight",
            StepId::SaveBackup => "save_backup",
            StepId::Payload => "payload",
            StepId::Runtime => "runtime",
            StepId::Seamless => "seamless",
            StepId::Witchy => "witchybnd",
            StepId::Dumps => "dumps",
            StepId::Converters => "converters",
            StepId::VerifyConverted => "verify_converted",
            StepId::Regulation => "regulation",
            StepId::VerifyAll => "verify_all",
            StepId::Cleanup => "cleanup",
            StepId::Profiles => "profiles",
            StepId::Finish => "finish",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            StepId::Preflight => "Checks (games, versions, tools, disk space)",
            StepId::SaveBackup => "Save backup (read-only copy of %APPDATA%\\EldenRing)",
            StepId::Payload => "Mod files (DLL, scripts, tools)",
            StepId::Runtime => "Python runtime for the build (bundled CPython 3.14.7)",
            StepId::Seamless => "Seamless Co-op copy + settings",
            StepId::Witchy => "WitchyBND 3.0.1.0 (regulation tool)",
            StepId::Dumps => "MW2 dumps from your MW2 (Unlinker; 5-15 min, ~2 GB temporary)",
            StepId::Converters => "Converters (MW2 data -> converted\\)",
            StepId::VerifyConverted => "Verify converted files against the release",
            StepId::Regulation => "Regulation (rebuilt from your vanilla regulation.bin; under a minute, 2-5 min with WitchyBND)",
            StepId::VerifyAll => "Verify everything against the release (sha256)",
            StepId::Cleanup => "Remove temporary dumps\\ and work\\",
            StepId::Profiles => "me3 profiles",
            StepId::Finish => "Launcher, shortcuts, install record",
        }
    }

    pub fn parse(s: &str) -> Option<StepId> {
        StepId::ALL.into_iter().find(|i| i.as_str() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipPolicy {
    /// Always runs (checks, verification, cheap idempotent steps).
    Never,
    /// Skipped when its key equals the recorded key and every output exists.
    IfUnchanged,
    /// Runs exactly when that step runs (temporary inputs of it, e.g. WitchyBND for the regulation).
    With(StepId),
    /// Skipped when that step is skipped; when it runs, still skipped if this step's own key equals the recorded
    /// key and its outputs exist (resume: e.g. the dumps of an interrupted build are reused by the converters).
    NeededBy(StepId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Run(String),
    Skip(String),
}

impl Decision {
    pub fn runs(&self) -> bool {
        matches!(self, Decision::Run(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: StepId,
    /// Label -> value (a path, a pin, or a sha256 of a file).
    pub inputs: Vec<(String, String)>,
    /// Steps whose key is chained into this key.
    pub deps: Vec<StepId>,
    /// Root-relative outputs (`/` separators) that must exist to skip.
    pub outputs: Vec<String>,
    /// Root-relative paths removed before the step runs.
    pub clean: Vec<String>,
    pub policy: SkipPolicy,
    pub key: String,
    /// The key this step had in the record format of an earlier setup (P3: the converters' toml input was narrowed
    /// to the keys the converters read). A record equal to it counts as unchanged and is rewritten to `key`.
    pub legacy_key: Option<String>,
    pub decision: Decision,
}

impl Step {
    /// The recorded key is the legacy one (the executor records the current key for a skipped step then).
    pub fn migrates(&self, state: &InstallState) -> bool {
        !self.key.is_empty() && self.legacy_key.as_deref().is_some_and(|l| state.step_key(self.id.as_str()) == Some(l))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub root: PathBuf,
    pub mode: Mode,
    pub steps: Vec<Step>,
}

impl Plan {
    pub fn get(&self, id: StepId) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn runs(&self, id: StepId) -> bool {
        self.get(id).is_some_and(|s| s.decision.runs())
    }

    /// Human-readable listing (dry run).
    pub fn describe(&self) -> String {
        let mut s = String::new();
        let n = self.steps.len();
        for (i, st) in self.steps.iter().enumerate() {
            let (w, why) = match &st.decision {
                Decision::Run(r) => ("RUN ", r.as_str()),
                Decision::Skip(r) => ("SKIP", r.as_str()),
            };
            s.push_str(&format!("{w} {:>2}/{n} {:<17} {} - {why}\n", i + 1, st.id.as_str(), st.id.title()));
            if !st.outputs.is_empty() {
                s.push_str(&format!("            outputs: {}\n", st.outputs.join(", ")));
            }
            if !st.clean.is_empty() && st.decision.runs() {
                s.push_str(&format!("            clears first: {}\n", st.clean.join(", ")));
            }
            if !st.key.is_empty() {
                s.push_str(&format!("            key {}\n", util::short(&st.key)));
            }
        }
        s
    }
}

/// What the plan is built from (all resolved by detection / the player before planning).
#[derive(Clone, Debug)]
pub struct PlanInputs<'a> {
    pub root: &'a Path,
    pub payload: &'a Path,
    pub release: &'a Release,
    pub mode: Mode,
    pub er_game: &'a Path,
    pub mw2_dir: &'a Path,
    pub me3: &'a Path,
    pub python: &'a Path,
    /// The player's Seamless (co-op only).
    pub seamless_source: Option<&'a Path>,
    /// The WitchyBND zip (None = the pinned download; only used with [`Tool::Witchy`]).
    pub witchy_zip: Option<&'a Path>,
    /// The regulation builder (P2: [`Tool::Regbuild`] in-process, or the WitchyBND fallback).
    pub regulation_tool: Tool,
    pub mw2_buildid: &'a str,
    pub iw4sp_sha256: &'a str,
    pub vanilla_regulation_sha256: &'a str,
    pub keep_dumps: bool,
    /// Ignore the recorded keys (repair).
    pub force: bool,
}

/// Payload entries never copied into the root (results, logs, private data, the P0 user files).
pub const PAYLOAD_EXCLUDE: &[&str] = &[
    "converted",
    "dumps",
    "work",
    "build/package",
    "build/rows.json",
    "setup_logs",
    "logs",
    "backups",
    "seamless",
    "downloads",
    "state",
    "setup_inputs.json",
    "launch_command.txt",
    "tools/WitchyBND/app",
    "MW2inER.exe",
    // the marker payload::stage_zip / stage_embedded leave in a staged release
    ".staged",
];

pub fn payload_excluded(rel: &str) -> bool {
    let r = rel.replace('\\', "/");
    if r.starts_with("profiles/") && r.ends_with(".me3") && !r.ends_with(".template.me3") {
        return true;
    }
    PAYLOAD_EXCLUDE.iter().any(|x| r.eq_ignore_ascii_case(x) || r.to_lowercase().starts_with(&format!("{}/", x.to_lowercase())))
}

/// The payload files (relative `/` path, absolute path), sorted.
pub fn payload_files(payload: &Path) -> Vec<(String, PathBuf)> {
    util::walk_files(payload)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            let rel = util::rel_key(p.strip_prefix(payload).ok()?);
            (!payload_excluded(&rel)).then_some((rel, p))
        })
        .collect()
}

/// Written by the dumps step after friend_dump.py passed (its skip key); removed with dumps\ by the cleanup.
pub const DUMPS_COMPLETE: &str = "dumps/dumps.complete";

/// The scripts whose content feeds each Python step (tools\scripts\...).
pub const DUMP_SCRIPTS: &[&str] = &["friend_dump.py"];
pub const CONVERTERS: &[(&str, &str)] = &[
    ("convert_weapons", "convert_weapons.py"),
    ("copy_sounds", "copy_sounds.py"),
    ("convert_viewmodel", "convert_viewmodel.py"),
    ("convert_ac130", "convert_ac130.py"),
    ("convert_movement", "convert_movement.py"),
    ("convert_arsenal", "convert_arsenal.py"),
    ("convert_nuke", "convert_nuke.py"),
];
pub const CONVERTER_HELPERS: &[&str] = &["vm_format.py", "xanim_read.py", "inventory_m6.py", "check_assets.py"];
/// The scripts of the WitchyBND regulation path (the regbuild path runs none of them; its plan is a port of
/// build_regulation.py, so the toml keys of [`toml_build_sections`] cover both).
pub const REGULATION_SCRIPTS: &[&str] = &["build_regulation.py", "param_diff.py", "param_rows.py", "conpty_run.py"];

fn file_input(base: &Path, rel: &str) -> (String, String) {
    let p = base.join(rel.replace('/', "\\"));
    (rel.to_string(), util::sha256_file(&p).unwrap_or_else(|_| "missing".into()))
}

/// Single keys of otherwise hot sections that the regulation / converter scripts read (`[section] key`).
pub const BUILD_KEYS: &[(&str, &str)] = &[("arsenal", "guns"), ("launchers", "explosion_tiers")];

/// The `[section] key`s of mw2er.toml the dumps / converter scripts read (convert_arsenal.py: the gun list). A
/// change of any other build input reruns only the regulation (test `build_keys_cover_every_script_read`).
pub const CONVERTER_TOML_KEYS: &[(&str, &str)] = &[("arsenal", "guns")];

/// The converters' part of mw2er.toml ([`CONVERTER_TOML_KEYS`]) as canonical JSON.
pub fn toml_converter_inputs(text: &str) -> String {
    let Ok(t) = text.parse::<toml::Table>() else { return "unparsable".into() };
    let mut m = serde_json::Map::new();
    for (sec, key) in CONVERTER_TOML_KEYS {
        if let Some(g) = t.get(*sec).and_then(|a| a.get(*key)) {
            m.insert(format!("{sec}.{key}"), serde_json::to_value(g).unwrap_or_default());
        }
    }
    serde_json::Value::Object(m).to_string()
}

/// Whole sections the scripts read (`[damage]`, every `[regulation*]` table).
pub fn is_build_section(name: &str) -> bool {
    name == "damage" || name.starts_with("regulation")
}

/// The build-input sections of mw2er.toml (`[damage]`, `[regulation*]`, `[arsenal] guns`, `[launchers]
/// explosion_tiers`) as canonical JSON: a change there changes the regulation / converted data (docs 1.3), a hot
/// setting does not. The test `build_keys_cover_every_script_read` keeps this list in step with the scripts.
pub fn toml_build_sections(text: &str) -> String {
    let Ok(t) = text.parse::<toml::Table>() else { return "unparsable".into() };
    let mut m = serde_json::Map::new();
    for (k, v) in &t {
        if is_build_section(k) {
            m.insert(k.clone(), serde_json::to_value(v).unwrap_or_default());
        }
    }
    for (sec, key) in BUILD_KEYS {
        if let Some(g) = t.get(*sec).and_then(|a| a.get(*key)) {
            m.insert(format!("{sec}.{key}"), serde_json::to_value(g).unwrap_or_default());
        }
    }
    serde_json::Value::Object(m).to_string()
}

fn key_of(id: StepId, inputs: &[(String, String)], dep_keys: &[&str]) -> String {
    let mut s = format!("step {}\n", id.as_str());
    for (k, v) in inputs {
        s.push_str(&format!("{k}={v}\n"));
    }
    for d in dep_keys {
        s.push_str(&format!("dep={d}\n"));
    }
    util::sha256_bytes(s.as_bytes())
}

/// Builds the plan and decides each step against the recorded state.
pub fn build(i: &PlanInputs, state: &InstallState) -> Plan {
    let p = i.payload;
    let pins = &i.release.pins;
    let in_place = util::same_path(i.root, i.payload);
    let toml_text = std::fs::read_to_string(p.join("mw2er.toml")).unwrap_or_default();
    let build_sections = toml_build_sections(&toml_text);
    let path = |x: &Path| x.display().to_string();
    let mut steps: Vec<Step> = Vec::new();
    let add = |id: StepId, inputs: Vec<(String, String)>, deps: Vec<StepId>, outputs: &[&str], clean: &[&str], policy: SkipPolicy, steps: &mut Vec<Step>| {
        let dep_keys: Vec<String> = deps.iter().filter_map(|d| steps.iter().find(|s| s.id == *d)).map(|s| s.key.clone()).collect();
        let dk: Vec<&str> = dep_keys.iter().map(String::as_str).collect();
        let key = if matches!(policy, SkipPolicy::IfUnchanged | SkipPolicy::NeededBy(_)) { key_of(id, &inputs, &dk) } else { String::new() };
        steps.push(Step {
            id,
            inputs,
            deps,
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            clean: clean.iter().map(|s| s.to_string()).collect(),
            policy,
            key,
            legacy_key: None,
            decision: Decision::Run(String::new()),
        });
    };

    add(StepId::Preflight, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    add(StepId::SaveBackup, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    let payload_inputs: Vec<(String, String)> = if in_place {
        vec![("in_place".into(), path(p))]
    } else {
        payload_files(p).into_iter().map(|(rel, f)| (rel, util::sha256_file(&f).unwrap_or_default())).collect()
    };
    add(StepId::Payload, payload_inputs, vec![], &["build/natives/mw2er.dll", "mw2er.toml", "tools/scripts/coop_manifest.py"], &[], SkipPolicy::IfUnchanged, &mut steps);
    add(StepId::Runtime, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    if i.mode.coop() {
        let src = i.seamless_source.map(path).unwrap_or_default();
        add(StepId::Seamless, vec![("source".into(), src), ("ersc_sha256".into(), pins.ersc_sha256.clone())], vec![], &["seamless/SeamlessCoop/ersc.dll"], &[], SkipPolicy::Never, &mut steps);
    }
    let witchy = i.regulation_tool == Tool::Witchy;
    if witchy {
        add(
            StepId::Witchy,
            vec![
                ("zip".into(), i.witchy_zip.map(path).unwrap_or_else(|| "pinned download".into())),
                ("witchybnd_zip_sha256".into(), pins.witchybnd_zip_sha256.clone()),
                ("witchybnd_exe_sha256".into(), pins.witchybnd_exe_sha256.clone()),
                ("paramdex_version".into(), pins.paramdex_version.clone()),
            ],
            vec![],
            &["tools/WitchyBND/app/WitchyBND.exe"],
            &[],
            SkipPolicy::With(StepId::Regulation),
            &mut steps,
        );
    }
    let mut conv_inputs = vec![
        ("mw2_dir".into(), path(i.mw2_dir)),
        ("mw2_buildid".into(), i.mw2_buildid.to_string()),
        ("iw4sp_sha256".into(), i.iw4sp_sha256.to_string()),
        file_input(p, "tools/oat/bin/Unlinker.exe"),
        file_input(p, "tools/bin/sndconv.exe"),
        ("toml.converter_keys".into(), util::sha256_bytes(toml_converter_inputs(&toml_text).as_bytes())),
    ];
    for s in DUMP_SCRIPTS.iter().chain(CONVERTERS.iter().map(|c| &c.1)).chain(CONVERTER_HELPERS) {
        conv_inputs.push(file_input(p, &format!("tools/scripts/{s}")));
    }
    // the record format of the setups before P3 (every build input fed the converters' key): computed with the
    // installed mw2er.toml's build inputs (= the old release's; they are locked), valid only while the keys the
    // converters read are unchanged
    let installed_toml = std::fs::read_to_string(i.root.join("mw2er.toml")).ok();
    let conv_legacy = installed_toml.as_deref().filter(|t| toml_converter_inputs(t) == toml_converter_inputs(&toml_text)).map(|t| {
        let mut v = conv_inputs.clone();
        v[5] = ("toml.arsenal.guns+build".into(), util::sha256_bytes(toml_build_sections(t).as_bytes()));
        v
    });
    let dump_inputs = vec![
        ("mw2_dir".into(), path(i.mw2_dir)),
        ("mw2_buildid".into(), i.mw2_buildid.to_string()),
        ("iw4sp_sha256".into(), i.iw4sp_sha256.to_string()),
        file_input(p, "tools/oat/bin/Unlinker.exe"),
        file_input(p, "tools/scripts/friend_dump.py"),
    ];
    add(StepId::Dumps, dump_inputs, vec![], &[DUMPS_COMPLETE], &["dumps"], SkipPolicy::NeededBy(StepId::Converters), &mut steps);
    add(StepId::Converters, conv_inputs, vec![], &["converted/weapons.json"], &["converted", "work"], SkipPolicy::IfUnchanged, &mut steps);
    if let Some(c) = steps.last_mut() {
        c.legacy_key = conv_legacy.as_ref().map(|v| key_of(StepId::Converters, v, &[]));
    }
    add(StepId::VerifyConverted, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    let mut reg_inputs = vec![
        ("vanilla_regulation_sha256".into(), i.vanilla_regulation_sha256.to_string()),
        ("regulation_builder".into(), regulation::builder_key(i.regulation_tool)),
        ("toml.build_sections".into(), util::sha256_bytes(build_sections.as_bytes())),
    ];
    if witchy {
        reg_inputs.push(("witchybnd_exe_sha256".into(), pins.witchybnd_exe_sha256.clone()));
        reg_inputs.push(("paramdex_version".into(), pins.paramdex_version.clone()));
        for s in REGULATION_SCRIPTS {
            reg_inputs.push(file_input(p, &format!("tools/scripts/{s}")));
        }
    }
    let reg_legacy = steps.iter().find(|s| s.id == StepId::Converters).and_then(|s| s.legacy_key.clone()).map(|ck| key_of(StepId::Regulation, &reg_inputs, &[&ck]));
    add(StepId::Regulation, reg_inputs, vec![StepId::Converters], &["build/package/regulation.bin", "build/rows.json"], &["build/package", "build/rows.json", "work/regulation"], SkipPolicy::IfUnchanged, &mut steps);
    if let Some(r) = steps.last_mut() {
        r.legacy_key = reg_legacy;
    }
    add(StepId::VerifyAll, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    if !i.keep_dumps {
        add(StepId::Cleanup, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    }
    add(StepId::Profiles, vec![("me3".into(), path(i.me3)), ("er_game".into(), path(i.er_game))], vec![], &[], &[], SkipPolicy::Never, &mut steps);
    add(StepId::Finish, vec![], vec![], &[], &[], SkipPolicy::Never, &mut steps);

    // decisions: IfUnchanged first, then With
    for s in steps.iter_mut() {
        s.decision = match s.policy {
            SkipPolicy::Never => Decision::Run("always".into()),
            SkipPolicy::With(_) | SkipPolicy::NeededBy(_) => Decision::Run(String::new()),
            SkipPolicy::IfUnchanged => {
                if s.id == StepId::Payload && in_place {
                    Decision::Skip("installing in place (the payload folder is the install root)".into())
                } else if i.force {
                    Decision::Run("forced (repair)".into())
                } else {
                    match state.step_key(s.id.as_str()) {
                        None => Decision::Run("not done yet".into()),
                        Some(k) if k != s.key && s.legacy_key.as_deref() != Some(k) => Decision::Run("inputs changed".into()),
                        Some(_) => match s.outputs.iter().find(|o| !i.root.join(o.replace('/', "\\")).exists()) {
                            Some(o) => Decision::Run(format!("{o} missing")),
                            None => Decision::Skip("inputs unchanged, outputs present".into()),
                        },
                    }
                }
            }
        };
    }
    let snapshot: Vec<(StepId, bool)> = steps.iter().map(|s| (s.id, s.decision.runs())).collect();
    for s in steps.iter_mut() {
        let (SkipPolicy::With(other) | SkipPolicy::NeededBy(other)) = s.policy else { continue };
        let other_runs = snapshot.iter().find(|(id, _)| *id == other).is_some_and(|(_, r)| *r);
        let done_before = matches!(s.policy, SkipPolicy::NeededBy(_))
            && !i.force
            && state.step_key(s.id.as_str()) == Some(s.key.as_str())
            && s.outputs.iter().all(|o| i.root.join(o.replace('/', "\\")).exists());
        s.decision = if !other_runs {
            Decision::Skip(format!("{} is skipped", other.as_str()))
        } else if done_before {
            Decision::Skip(format!("already complete from an earlier run (reused by {})", other.as_str()))
        } else {
            Decision::Run(format!("needed by {}", other.as_str()))
        };
    }
    Plan { root: i.root.to_path_buf(), mode: i.mode, steps }
}

/// The steps whose records a failed verification invalidates (they rerun next time).
pub fn invalidated_by(failed: StepId) -> &'static [StepId] {
    match failed {
        StepId::VerifyConverted => &[StepId::Converters, StepId::Regulation],
        StepId::VerifyAll => &[StepId::Converters, StepId::Regulation, StepId::Payload],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    struct Fx {
        t: TempDir,
        pkg: PathBuf,
        rel: Release,
    }

    fn fx() -> Fx {
        let t = TempDir::new("plan");
        let pkg = crate::release::fixture::package(&t);
        let rel = Release::load(&pkg).unwrap();
        Fx { t, pkg, rel }
    }

    fn inputs<'a>(f: &'a Fx, root: &'a Path, mode: Mode) -> PlanInputs<'a> {
        PlanInputs {
            root,
            payload: &f.pkg,
            release: &f.rel,
            mode,
            er_game: Path::new(r"D:\G"),
            mw2_dir: Path::new(r"D:\M"),
            me3: Path::new(r"C:\me3.exe"),
            python: Path::new(r"C:\py.exe"),
            seamless_source: None,
            witchy_zip: None,
            regulation_tool: Tool::Witchy,
            mw2_buildid: "25374375",
            iw4sp_sha256: "bb",
            vanilla_regulation_sha256: "aa",
            keep_dumps: false,
            force: false,
        }
    }

    fn record_all(plan: &Plan, st: &mut InstallState, root: &Path) {
        for s in &plan.steps {
            if s.policy == SkipPolicy::IfUnchanged {
                st.record_step(s.id.as_str(), &s.key);
                for o in &s.outputs {
                    let p = root.join(o.replace('/', "\\"));
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, b"x").unwrap();
                }
            }
        }
    }

    #[test]
    fn fresh_install_runs_everything() {
        let f = fx();
        let root = f.t.0.join("root");
        let p = build(&inputs(&f, &root, Mode::Coop), &InstallState::new());
        assert!(p.steps.iter().all(|s| s.decision.runs()), "{}", p.describe());
        assert!(p.get(StepId::Seamless).is_some());
        assert!(build(&inputs(&f, &root, Mode::Solo), &InstallState::new()).get(StepId::Seamless).is_none());
        assert!(p.describe().contains("RUN   7/14 dumps"), "{}", p.describe());
    }

    #[test]
    fn rerun_skips_unchanged_and_chains_changes() {
        let f = fx();
        let root = f.t.0.join("root");
        let p1 = build(&inputs(&f, &root, Mode::Solo), &InstallState::new());
        let mut st = InstallState::new();
        record_all(&p1, &mut st, &root);
        let p2 = build(&inputs(&f, &root, Mode::Solo), &st);
        for id in [StepId::Payload, StepId::Witchy, StepId::Dumps, StepId::Converters, StepId::Regulation] {
            assert!(!p2.runs(id), "{id:?}\n{}", p2.describe());
        }
        for id in [StepId::Preflight, StepId::VerifyConverted, StepId::VerifyAll, StepId::Profiles, StepId::Finish] {
            assert!(p2.runs(id));
        }
        // a new gun in [arsenal] guns: converters (+ dumps) and regulation rerun, payload too (toml changed)
        let toml = f.pkg.join("mw2er.toml");
        std::fs::write(&toml, "[damage]\nscale = 1.0\n\n[ui]\nhud = true\n\n[arsenal]\nguns = [\"m4a1\", \"ak47\"]\n").unwrap();
        let p3 = build(&inputs(&f, &root, Mode::Solo), &st);
        assert!(p3.runs(StepId::Converters) && p3.runs(StepId::Dumps) && p3.runs(StepId::Regulation) && p3.runs(StepId::Witchy));
        // a hot setting only: payload reruns (file changed), the build does not
        std::fs::write(&toml, "[damage]\nscale = 1.0\n\n[ui]\nhud = false\n\n[arsenal]\nguns = [\"m4a1\"]\n").unwrap();
        let p4 = build(&inputs(&f, &root, Mode::Solo), &st);
        assert!(p4.runs(StepId::Payload));
        assert!(!p4.runs(StepId::Converters) && !p4.runs(StepId::Regulation) && !p4.runs(StepId::Dumps));
        // a changed vanilla regulation: only the regulation (+ WitchyBND)
        let mut i5 = inputs(&f, &root, Mode::Solo);
        i5.vanilla_regulation_sha256 = "cc";
        let p5 = build(&i5, &st);
        assert!(p5.runs(StepId::Regulation) && p5.runs(StepId::Witchy) && !p5.runs(StepId::Converters));
        // a missing output reruns; force reruns everything
        std::fs::remove_file(root.join(r"converted\weapons.json")).unwrap();
        let p6 = build(&inputs(&f, &root, Mode::Solo), &st);
        assert_eq!(p6.get(StepId::Converters).unwrap().decision, Decision::Run("converted/weapons.json missing".into()));
        let mut i7 = inputs(&f, &root, Mode::Solo);
        i7.force = true;
        assert!(build(&i7, &st).steps.iter().all(|s| s.decision.runs()));
    }

    /// P3: an update that changes only a regulation build input ([damage], [regulation*], explosion_tiers) reruns
    /// only the regulation; the gun list reruns the converters too; a DLL-only release reruns only the payload.
    #[test]
    fn a_regulation_input_reruns_only_the_regulation() {
        let f = fx();
        let root = f.t.0.join("root");
        let mut ir = inputs(&f, &root, Mode::Solo);
        ir.regulation_tool = Tool::Regbuild;
        let p1 = build(&ir, &InstallState::new());
        let mut st = InstallState::new();
        record_all(&p1, &mut st, &root);
        let toml = f.pkg.join("mw2er.toml");
        let base = std::fs::read_to_string(&toml).unwrap();
        for (what, from, to) in [
            ("[damage]", "scale = 1.0", "scale = 1.5"),
            ("[launchers] explosion_tiers", "[arsenal]", "[launchers]
explosion_tiers = 1

[arsenal]"),
            ("[regulation.nuke]", "[arsenal]", "[regulation.nuke]
radius = 9

[arsenal]"),
        ] {
            std::fs::write(&toml, base.replacen(from, to, 1)).unwrap();
            let p = build(&ir, &st);
            assert!(p.runs(StepId::Regulation) && p.runs(StepId::Payload), "{what}
{}", p.describe());
            assert!(!p.runs(StepId::Converters) && !p.runs(StepId::Dumps), "{what}
{}", p.describe());
        }
        std::fs::write(&toml, base.replace("guns = [\"m4a1\"]", "guns = [\"m4a1\", \"ak47\"]")).unwrap();
        let p = build(&ir, &st);
        assert!(p.runs(StepId::Converters) && p.runs(StepId::Dumps) && p.runs(StepId::Regulation));
        // DLL only
        std::fs::write(&toml, &base).unwrap();
        f.t.write("pkg/build/natives/mw2er.dll", b"fake mw2er.dll 0.2.1");
        let p = build(&ir, &st);
        let runs: Vec<StepId> = p.steps.iter().filter(|s| s.decision.runs() && s.policy != SkipPolicy::Never).map(|s| s.id).collect();
        assert_eq!(runs, vec![StepId::Payload], "{}", p.describe());
    }

    /// P3: a record written by a setup before P3 (the converters' key over every build input) counts as unchanged
    /// while the converters' inputs are; the executor then records the current key (`Step::migrates`).
    #[test]
    fn records_of_the_older_key_format_are_accepted() {
        let f = fx();
        let root = f.t.0.join("root");
        let mut ir = inputs(&f, &root, Mode::Solo);
        ir.regulation_tool = Tool::Regbuild;
        let p1 = build(&ir, &InstallState::new());
        assert!(p1.get(StepId::Converters).unwrap().legacy_key.is_none(), "no installed toml: no legacy key");
        // the old-format record: the converters' key over every build input, the regulation chained to it
        let mut st = InstallState::new();
        record_all(&p1, &mut st, &root);
        std::fs::copy(f.pkg.join("mw2er.toml"), root.join("mw2er.toml")).unwrap();
        let conv = p1.get(StepId::Converters).unwrap();
        let mut old_inputs = conv.inputs.clone();
        let text = std::fs::read_to_string(f.pkg.join("mw2er.toml")).unwrap();
        old_inputs[5] = ("toml.arsenal.guns+build".into(), util::sha256_bytes(toml_build_sections(&text).as_bytes()));
        let old_conv = key_of(StepId::Converters, &old_inputs, &[]);
        let reg = p1.get(StepId::Regulation).unwrap();
        let old_reg = key_of(StepId::Regulation, &reg.inputs, &[&old_conv]);
        st.record_step("converters", &old_conv);
        st.record_step("regulation", &old_reg);
        let p2 = build(&ir, &st);
        assert!(!p2.runs(StepId::Converters) && !p2.runs(StepId::Regulation), "{}", p2.describe());
        assert!(p2.get(StepId::Converters).unwrap().migrates(&st) && p2.get(StepId::Regulation).unwrap().migrates(&st));
        // a regulation input changed: only the regulation reruns, the converters still migrate
        std::fs::write(f.pkg.join("mw2er.toml"), text.replace("scale = 1.0", "scale = 2.0")).unwrap();
        let p3 = build(&ir, &st);
        assert!(p3.runs(StepId::Regulation) && !p3.runs(StepId::Converters), "{}", p3.describe());
        // the gun list changed: the legacy key is not offered, the converters rerun
        std::fs::write(f.pkg.join("mw2er.toml"), text.replace("guns = [\"m4a1\"]", "guns = [\"ak47\"]")).unwrap();
        let p4 = build(&ir, &st);
        assert!(p4.get(StepId::Converters).unwrap().legacy_key.is_none() && p4.runs(StepId::Converters));
        assert!(toml_converter_inputs("[arsenal]
guns = [1]
x = 2
[damage]
k = 1
") == toml_converter_inputs("[arsenal]
guns = [1]
"));
    }

    #[test]
    fn resume_reuses_finished_dumps() {
        let f = fx();
        let root = f.t.0.join("root");
        let p1 = build(&inputs(&f, &root, Mode::Solo), &InstallState::new());
        assert_eq!(p1.get(StepId::Dumps).unwrap().decision, Decision::Run("needed by converters".into()));
        // the dumps passed, the converters were interrupted
        let mut st = InstallState::new();
        let dk = p1.get(StepId::Dumps).unwrap().key.clone();
        st.record_step("dumps", &dk);
        f.t.write(r"root\dumps\dumps.complete", &dk);
        let p2 = build(&inputs(&f, &root, Mode::Solo), &st);
        assert!(p2.runs(StepId::Converters));
        assert!(!p2.runs(StepId::Dumps), "{}", p2.describe());
        // a different MW2 build makes the dumps stale
        let mut i3 = inputs(&f, &root, Mode::Solo);
        i3.mw2_buildid = "1";
        assert!(build(&i3, &st).runs(StepId::Dumps));
        // the cleanup removed them: rerun
        std::fs::remove_file(root.join(r"dumps\dumps.complete")).unwrap();
        assert!(build(&inputs(&f, &root, Mode::Solo), &st).runs(StepId::Dumps));
        // force always reruns
        f.t.write(r"root\dumps\dumps.complete", &dk);
        let mut i4 = inputs(&f, &root, Mode::Solo);
        i4.force = true;
        assert!(build(&i4, &st).runs(StepId::Dumps));
        // the runtime step is always there, right after the payload
        let ids: Vec<StepId> = p1.steps.iter().map(|s| s.id).collect();
        assert_eq!(&ids[..4], &[StepId::Preflight, StepId::SaveBackup, StepId::Payload, StepId::Runtime]);
        assert_eq!(p1.get(StepId::Converters).unwrap().clean, vec!["converted".to_string(), "work".to_string()]);
    }

    #[test]
    fn in_place_and_payload_filter() {
        let f = fx();
        let p = build(&inputs(&f, &f.pkg, Mode::Solo), &InstallState::new());
        assert!(!p.runs(StepId::Payload));
        assert!(payload_excluded("converted/weapons.json"));
        assert!(payload_excluded(".staged") && !payload_excluded("tools/.staged_notes.txt"));
        assert!(payload_excluded("profiles/mw2er-coop.me3"));
        assert!(!payload_excluded("profiles/mw2er-coop.template.me3"));
        assert!(payload_excluded("tools/WitchyBND/app/WitchyBND.exe"));
        assert!(!payload_excluded("tools/WitchyBND/PUT_NOTHING_HERE.txt"));
        assert!(payload_excluded("Seamless/SeamlessCoop/ersc.dll"));
        assert!(!payload_excluded("seamless_notes.txt"));
        f.t.write("pkg/converted/x.json", "{}");
        f.t.write("pkg/setup_logs/setup.log", "x");
        let files: Vec<String> = payload_files(&f.pkg).into_iter().map(|x| x.0).collect();
        assert!(files.contains(&"build/natives/mw2er.dll".to_string()) && files.contains(&"setup.ps1".to_string()));
        assert!(!files.iter().any(|x| x.starts_with("converted") || x.starts_with("setup_logs")));
        let mut no_keep = inputs(&f, &f.pkg, Mode::Solo);
        no_keep.keep_dumps = true;
        assert!(build(&no_keep, &InstallState::new()).get(StepId::Cleanup).is_none());
    }

    /// P2: the Rust builder needs no WitchyBND step; switching the builder reruns only the regulation.
    #[test]
    fn regbuild_plan_has_no_witchy_step_and_a_builder_switch_reruns_the_regulation() {
        let f = fx();
        let root = f.t.0.join("root");
        let mut ir = inputs(&f, &root, Mode::Solo);
        ir.regulation_tool = Tool::Regbuild;
        let p1 = build(&ir, &InstallState::new());
        assert!(p1.get(StepId::Witchy).is_none(), "{}", p1.describe());
        assert_eq!(p1.steps.len(), 12, "solo: 13 with WitchyBND");
        assert!(p1.describe().contains("RUN   5/12 dumps"), "{}", p1.describe());
        let reg = p1.get(StepId::Regulation).unwrap();
        assert!(reg.inputs.iter().any(|(k, v)| k == "regulation_builder" && v.contains("regbuild")));
        assert!(!reg.inputs.iter().any(|(k, _)| k.ends_with("build_regulation.py") || k == "witchybnd_exe_sha256"), "{:?}", reg.inputs);
        assert_eq!(reg.outputs, vec!["build/package/regulation.bin".to_string(), "build/rows.json".to_string()]);
        let mut st = InstallState::new();
        record_all(&p1, &mut st, &root);
        assert!(!build(&ir, &st).runs(StepId::Regulation));
        // a changed build_regulation.py does not matter to regbuild
        f.t.write("pkg/tools/scripts/build_regulation.py", "# changed\n");
        assert!(!build(&ir, &st).runs(StepId::Regulation));
        // the fallback: WitchyBND + regulation rerun, not the build before it (the payload only for the changed script)
        let iw = inputs(&f, &root, Mode::Solo);
        let p2 = build(&iw, &st);
        assert!(p2.runs(StepId::Regulation) && p2.runs(StepId::Witchy), "{}", p2.describe());
        for id in [StepId::Dumps, StepId::Converters] {
            assert!(!p2.runs(id), "{id:?}");
        }
        assert_ne!(p2.get(StepId::Regulation).unwrap().key, reg.key);
    }

    #[test]
    fn toml_sections_ignore_hot_settings() {
        let a = toml_build_sections("[damage]\nx = 1\n[regulation_rows]\ny = [1]\n[arsenal]\nguns = [\"a\"]\nother = 1\n[ui]\nz = 1\n");
        let b = toml_build_sections("[ui]\nz = 2\n[arsenal]\nother = 5\nguns = [\"a\"]\n[regulation_rows]\ny = [1]\n[damage]\nx = 1\n");
        assert_eq!(a, b);
        assert_ne!(a, toml_build_sections("[damage]\nx = 2\n"));
        assert_eq!(StepId::parse("verify_all"), Some(StepId::VerifyAll));
        assert_eq!(invalidated_by(StepId::VerifyConverted), &[StepId::Converters, StepId::Regulation]);
        // [launchers] explosion_tiers is a build input (build_regulation.py), the rest of [launchers] is hot
        let l1 = toml_build_sections("[launchers]\nexplosion_tiers = 2\nrocket_impact = false\n");
        assert_eq!(l1, toml_build_sections("[launchers]\nrocket_impact = true\nexplosion_tiers = 2\n"));
        assert_ne!(l1, toml_build_sections("[launchers]\nexplosion_tiers = 1\nrocket_impact = false\n"));
    }

    /// Every `cfg.get("<section>")[.get("<key>")]` read of mw2er.toml in the scripts the build steps run is covered
    /// by [`toml_build_sections`] (a whole build section, or a [`BUILD_KEYS`] entry), so the skip keys cannot drift
    /// from the scripts. Reads only the repository's scripts.
    #[test]
    fn build_keys_cover_every_script_read() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\tools\scripts");
        let scripts: Vec<&str> = DUMP_SCRIPTS
            .iter()
            .copied()
            .chain(CONVERTERS.iter().map(|c| c.1))
            .chain(CONVERTER_HELPERS.iter().copied())
            .chain(REGULATION_SCRIPTS.iter().copied())
            .collect();
        let quoted = |s: &str| -> Option<String> { s.strip_prefix('"').and_then(|r| r.split('"').next()).map(str::to_string) };
        let mut reads = Vec::new();
        for name in scripts {
            let text = std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            if text.contains("tomllib") {
                assert!(text.contains("cfg = tomllib.loads("), "{name} reads the toml under another name: extend this test");
            }
            assert!(!text.contains("cfg["), "{name} indexes cfg[...]: use cfg.get so this test sees it");
            for (at, _) in text.match_indices("cfg.get(") {
                let rest = &text[at + "cfg.get(".len()..];
                let sec = quoted(rest).unwrap_or_else(|| panic!("{name}: cfg.get without a literal section"));
                let close = rest.find(')').unwrap_or(rest.len());
                let key = rest[close + 1..].strip_prefix(".get(").and_then(quoted);
                reads.push((name, sec, key));
            }
        }
        assert!(reads.iter().any(|(_, s, k)| s == "launchers" && k.as_deref() == Some("explosion_tiers")), "the scan finds the known read");
        for (name, sec, key) in &reads {
            let ok = is_build_section(sec) || key.as_deref().is_some_and(|k| BUILD_KEYS.contains(&(sec.as_str(), k)));
            assert!(ok, "{name} reads [{sec}] {key:?} from mw2er.toml: add it to BUILD_KEYS / is_build_section (and release_tool.py LOCKED_TOML)");
            // P3: the dumps / converter scripts read only CONVERTER_TOML_KEYS (their skip key holds only those)
            if !REGULATION_SCRIPTS.contains(name) {
                let conv = key.as_deref().is_some_and(|k| CONVERTER_TOML_KEYS.contains(&(sec.as_str(), k)));
                assert!(conv, "{name} (a converter) reads [{sec}] {key:?}: add it to CONVERTER_TOML_KEYS");
            }
        }
        assert!(reads.iter().any(|(n, s, k)| *n == "convert_arsenal.py" && s == "arsenal" && k.as_deref() == Some("guns")));
    }
}
