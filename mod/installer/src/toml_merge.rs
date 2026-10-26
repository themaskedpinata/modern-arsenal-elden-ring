//! The P3 settings merge of `mw2er.toml` (docs/INSTALLER_DESIGN.md 2.8 "Settings", decision D10): an update (or a
//! repair) installs the new release's file but keeps the player's hot settings (keys, sensitivity, HUD, camera
//! offsets ...). `mw2er.toml` is also a BUILD input, so some keys always come from the release:
//!
//! - the build inputs: `[damage]`, every `[regulation*]` table, `[arsenal] guns`, `[launchers] explosion_tiers`
//!   ([`plan::is_build_section`] / [`plan::BUILD_KEYS`]) and whatever `manifest\release.json` lists in `locked_toml`;
//! - the release policy ([`friend_toml::FORCED`] probes off, [`friend_toml::REQUIRED`] tested features on);
//! - the file layout (`[paths]`, `dir` / `assets_dir` keys: where the converted data lives) and the top-level
//!   `schema`.
//!
//! Every other key is hot: the player's value is kept when it differs from the release's, unless it is still the
//! value the player got from the previous release (`base`, the pristine release file recorded at the last install:
//! then the new release's default applies - the player never changed it). Keys new in the release get its default;
//! keys the release dropped are dropped (the DLL refuses unknown keys). Values of a different type are not kept.
//!
//! The merge is line based on the release's text, so its comments and layout stay: only the value text of a kept
//! key is swapped (a multi-line value: its whole lines). The result is parsed again and its build inputs compared
//! with the release's; on any doubt the release's file is used as is (the player's old file is backed up by the
//! caller either way).

use std::collections::BTreeMap;

use crate::{friend_toml, plan};

/// Sections owned by the release as a whole (file layout).
pub const LAYOUT_SECTIONS: &[&str] = &["paths"];
/// Keys (in any section) naming the mod's own data folders.
pub const LAYOUT_KEYS: &[&str] = &["dir", "assets_dir"];
/// The longest multi-line value looked for (lines).
const MAX_SPAN: usize = 64;

/// One `key = value` of a toml text (top-level tables only; dotted keys and inline-table keys are not entries).
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// First line (0-based) and the number of lines the value spans.
    line: usize,
    span: usize,
    section: String,
    key: String,
    value: toml::Value,
    /// Byte range of the value text in its line (single-line values only).
    raw: Option<(usize, usize)>,
}

impl Entry {
    fn name(&self) -> String {
        if self.section.is_empty() { self.key.clone() } else { format!("[{}] {}", self.section, self.key) }
    }
}

/// The end of the value text in `s` (before an unquoted `#`), respecting basic ("...", with escapes) and literal
/// ('...') strings.
fn value_end(s: &str) -> usize {
    let b = s.as_bytes();
    let mut q: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match q {
            Some(b'"') if c == b'\\' => i += 1,
            Some(x) if c == x => q = None,
            None if c == b'"' || c == b'\'' => q = Some(c),
            None if c == b'#' => return i,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// The entries of a toml text (lines without their line ending).
fn entries(lines: &[String]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut sec = String::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(s) = friend_toml::section_header(t) {
            sec = s;
            i += 1;
            continue;
        }
        let Some((key, _)) = friend_toml::key_value(t) else {
            i += 1;
            continue;
        };
        let line = &lines[i];
        let Some(eq) = line.find('=') else {
            i += 1;
            continue;
        };
        let rest = &line[eq + 1..];
        let mut found = None;
        for n in 1..=MAX_SPAN.min(lines.len() - i) {
            let mut doc = format!("v ={rest}");
            for l in &lines[i + 1..i + n] {
                doc.push('\n');
                doc.push_str(l);
            }
            if let Ok(tab) = doc.parse::<toml::Table>()
                && tab.len() == 1
                && let Some(v) = tab.get("v")
            {
                found = Some((n, v.clone()));
                break;
            }
        }
        match found {
            Some((span, value)) => {
                let raw = (span == 1).then(|| {
                    let start = eq + 1 + (rest.len() - rest.trim_start().len());
                    let end = eq + 1 + rest[..value_end(rest)].trim_end().len();
                    (start, end.max(start))
                });
                out.push(Entry { line: i, span, section: sec.clone(), key, value, raw });
                i += span;
            }
            None => i += 1,
        }
    }
    out
}

/// Why the release owns a key (None = a hot setting the player keeps).
pub fn release_owned(section: &str, key: &str, locked: &[String]) -> Option<&'static str> {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if section.is_empty() {
        return Some("file format");
    }
    if plan::is_build_section(section) || plan::BUILD_KEYS.iter().any(|(s, k)| eq(s, section) && eq(k, key)) {
        return Some("build input");
    }
    let full = format!("{section}.{key}");
    for p in locked {
        let hit = match p.strip_suffix('*') {
            Some(prefix) => section.starts_with(prefix),
            None => section == p || section.starts_with(&format!("{p}.")) || full == *p,
        };
        if hit {
            return Some("build input");
        }
    }
    let policy = friend_toml::FORCED.iter().any(|(s, ks)| eq(s, section) && ks.iter().any(|(k, _, _)| eq(k, key)))
        || friend_toml::REQUIRED.iter().any(|(s, ks)| eq(s, section) && ks.iter().any(|(k, _)| eq(k, key)));
    if policy {
        return Some("release policy");
    }
    if LAYOUT_SECTIONS.iter().any(|s| eq(s, section)) || LAYOUT_KEYS.iter().any(|k| eq(k, key)) {
        return Some("file layout");
    }
    None
}

/// The accepted shoulder camera: three numbers, not all zero ([`friend_toml::SHOULDER_KEYS`]).
fn shoulder_ok(v: &toml::Value) -> bool {
    let Some(a) = v.as_array() else { return false };
    let n: Vec<f64> = a.iter().filter_map(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64))).collect();
    n.len() == 3 && a.len() == 3 && n.iter().any(|x| *x != 0.0)
}

/// What the merge did (key names as `[section] key`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    pub text: String,
    /// The player's own values kept (they differ from the release's).
    pub kept: Vec<String>,
    /// Keys the player never changed whose default the new release changed (the new default applies).
    pub new_defaults: Vec<String>,
    /// The player's value differed but the release owns the key: `[sec] key (why)`.
    pub release_owned: Vec<String>,
    /// Keys new in the release (its default).
    pub added: Vec<String>,
    /// The player's keys the release does not have any more.
    pub dropped: Vec<String>,
    /// Anything unusual (a value of another type, a duplicate key, an unparsable file).
    pub notes: Vec<String>,
}

impl Merged {
    /// One line for the log / the summary.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} of your setting(s) kept", self.kept.len())];
        for (n, what) in [
            (self.new_defaults.len(), "new default(s)"),
            (self.added.len(), "new setting(s)"),
            (self.dropped.len(), "removed"),
            (self.release_owned.len(), "set by the release"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        format!("mw2er.toml merged: {}", parts.join(", "))
    }

    /// Every detail line (log).
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![self.summary()];
        for (tag, list) in [
            ("kept", &self.kept),
            ("new default", &self.new_defaults),
            ("set by the release", &self.release_owned),
            ("added", &self.added),
            ("removed", &self.dropped),
            ("note", &self.notes),
        ] {
            v.extend(list.iter().map(|k| format!("mw2er.toml {tag}: {k}")));
        }
        v
    }
}

fn split_lines(s: &str) -> Vec<String> {
    s.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect()
}

/// Merges the player's `mw2er.toml` into the release's (see the module doc). `base` = the release file the player's
/// copy came from (None for an install made before P3: every differing hot value counts as the player's own).
pub fn merge(player: &str, base: Option<&str>, release: &str, locked: &[String]) -> Merged {
    let mut m = Merged::default();
    let fallback = |mut m: Merged, why: String| {
        m.kept.clear();
        m.new_defaults.clear();
        m.notes.push(why);
        m.text = release.to_string();
        m
    };
    if let Err(e) = player.parse::<toml::Table>() {
        return fallback(m, format!("your mw2er.toml did not parse ({}): the release's file was installed", e.message()));
    }
    let nl = if release.contains("\r\n") { "\r\n" } else { "\n" };
    let rl = split_lines(release);
    let pl = split_lines(player);
    let rel = entries(&rl);
    let ply = entries(&pl);
    let base_e = base.filter(|b| b.parse::<toml::Table>().is_ok()).map(|b| entries(&split_lines(b)));
    let id = |e: &Entry| (e.section.clone(), e.key.clone());
    let mut by_id: BTreeMap<(String, String), Vec<&Entry>> = BTreeMap::new();
    for e in &ply {
        by_id.entry(id(e)).or_default().push(e);
    }
    let base_of = |k: &(String, String)| base_e.as_ref().and_then(|b| b.iter().find(|e| id(e) == *k).map(|e| e.value.clone()));
    // release line index -> replacement lines
    let mut repl: BTreeMap<usize, (usize, Vec<String>)> = BTreeMap::new();
    let mut rel_ids = std::collections::BTreeSet::new();
    let mut rel_count: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in &rel {
        *rel_count.entry(id(r)).or_default() += 1;
    }
    for r in &rel {
        let k = id(r);
        rel_ids.insert(k.clone());
        let Some(ps) = by_id.get(&k) else {
            m.added.push(r.name());
            continue;
        };
        if ps.len() > 1 || rel_count[&k] > 1 {
            m.notes.push(format!("{} appears more than once: the release's value is used", r.name()));
            continue;
        }
        let p = ps[0];
        if p.value == r.value {
            continue;
        }
        if let Some(why) = release_owned(&r.section, &r.key, locked) {
            m.release_owned.push(format!("{} ({why})", r.name()));
            continue;
        }
        if base_of(&k).is_some_and(|b| b == p.value) {
            m.new_defaults.push(r.name());
            continue;
        }
        if std::mem::discriminant(&p.value) != std::mem::discriminant(&r.value) {
            m.notes.push(format!("{}: your value has another type than the release's ({} vs {}): the release's value is used", r.name(), p.value.type_str(), r.value.type_str()));
            continue;
        }
        if r.section.eq_ignore_ascii_case("third_person") && friend_toml::SHOULDER_KEYS.iter().any(|s| s.eq_ignore_ascii_case(&r.key)) && !shoulder_ok(&p.value) {
            m.notes.push(format!("{}: your value is not a usable shoulder camera (three numbers, not all zero): the release's value is used", r.name()));
            continue;
        }
        let lines = match (r.raw, p.raw) {
            (Some((rs, re)), Some((ps0, pe))) => {
                let line = &rl[r.line];
                let pv = &pl[p.line][ps0..pe];
                let rest = &line[re..];
                // keep the release's comment column where the new value fits
                let pad = rest.len() - rest.trim_start().len();
                let width = (re - rs) + pad;
                let tail = rest.trim_start();
                let text = if tail.is_empty() {
                    format!("{}{pv}", &line[..rs])
                } else {
                    format!("{}{pv:<w$}{tail}", &line[..rs], w = width.max(pv.len() + 1))
                };
                vec![text]
            }
            _ => pl[p.line..p.line + p.span].to_vec(),
        };
        repl.insert(r.line, (r.span, lines));
        m.kept.push(r.name());
    }
    for p in &ply {
        if !rel_ids.contains(&id(p)) {
            m.dropped.push(p.name());
        }
    }
    m.dropped.dedup();
    let mut out: Vec<String> = Vec::with_capacity(rl.len());
    let mut i = 0;
    while i < rl.len() {
        if let Some((span, lines)) = repl.get(&i) {
            out.extend(lines.iter().cloned());
            i += span;
        } else {
            out.push(rl[i].clone());
            i += 1;
        }
    }
    let text = out.join(nl);
    // the result must parse, keep the release's build inputs and hold the kept values
    let check = || -> Result<(), String> {
        let t: toml::Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        if plan::toml_build_sections(&text) != plan::toml_build_sections(release) {
            return Err("the build inputs changed".into());
        }
        let got = entries(&split_lines(&text));
        for k in &m.kept {
            let ok = got.iter().any(|g| g.name() == *k && ply.iter().any(|p| p.name() == *k && p.value == g.value));
            if !ok {
                return Err(format!("{k} did not keep its value"));
            }
        }
        drop(t);
        Ok(())
    };
    match check() {
        Ok(()) => {
            m.text = text;
            m
        }
        Err(e) => fallback(m, format!("the merged file was not usable ({e}): the release's file was installed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::friend_toml::fixture::POLICY_TOML;

    const LOCKED: &[&str] = &["damage", "regulation*", "arsenal.guns", "launchers.explosion_tiers"];

    fn locked() -> Vec<String> {
        LOCKED.iter().map(|s| s.to_string()).collect()
    }

    fn value(text: &str, sec: &str, key: &str) -> Option<toml::Value> {
        let t: toml::Table = text.parse().unwrap();
        let mut cur = &t;
        for part in sec.split('.').filter(|s| !s.is_empty()) {
            cur = cur.get(part)?.as_table()?;
        }
        cur.get(key).cloned()
    }

    const RELEASE: &str = "# release toml\nschema = 1\n\n[paths]                # layout\nlog_dir = 'logs'\n\n[keys]\ntoggle            = \"F5\"      # F1-F12\nreload            = \"R\"\n\n\
[aim]\nsensitivity = 1.0   # mouse\nnew_key = 3   # new in this release\n\n[damage]\nk = 4.0\n\n[regulation.arsenal]\nscale = 1.0\n\n\
[arsenal]\nenabled = true\ndir = 'converted\\guns'\nguns = [   # cycle order\n    \"m4_mp\",\n    \"ak47_mp\",\n]\npreload = \"all\"\n\n\
[launchers]\nexplosion_tiers = 2\nrocket_impact = true\n\n[third_person]\nshoulder_offset_m = [0.55, 0.25, 0.0]\n\n[nuke]\nalways_available = false # policy\n";

    #[test]
    fn keeps_hot_settings_and_takes_the_locked_ones() {
        let player = "schema = 1\n[paths]\nlog_dir = 'my_logs'\n[keys]\ntoggle = \"F6\"\nreload = \"R\"\nold_key = \"X\"\n[aim]\nsensitivity = 1.35\n[damage]\nk = 9.0\n[regulation.arsenal]\nscale = 2.0\n\
[arsenal]\nenabled = false\ndir = 'x'\nguns = [\"m4_mp\"]\npreload = \"none\"\n[launchers]\nexplosion_tiers = 1\nrocket_impact = false\n[third_person]\nshoulder_offset_m = [0.7, 0.3, 0.1]\n[nuke]\nalways_available = true\n";
        let m = merge(player, None, RELEASE, &locked());
        let t = &m.text;
        assert_eq!(value(t, "keys", "toggle").unwrap().as_str(), Some("F6"));
        assert_eq!(value(t, "aim", "sensitivity").unwrap().as_float(), Some(1.35));
        assert_eq!(value(t, "arsenal", "enabled").unwrap().as_bool(), Some(false));
        assert_eq!(value(t, "arsenal", "preload").unwrap().as_str(), Some("none"));
        assert_eq!(value(t, "launchers", "rocket_impact").unwrap().as_bool(), Some(false));
        assert_eq!(value(t, "third_person", "shoulder_offset_m").unwrap().as_array().unwrap().len(), 3);
        assert!(t.contains("shoulder_offset_m = [0.7, 0.3, 0.1]"));
        // locked / forced / layout from the release
        assert_eq!(value(t, "damage", "k").unwrap().as_float(), Some(4.0));
        assert_eq!(value(t, "regulation.arsenal", "scale").unwrap().as_float(), Some(1.0));
        assert_eq!(value(t, "arsenal", "guns").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(value(t, "launchers", "explosion_tiers").unwrap().as_integer(), Some(2));
        assert_eq!(value(t, "nuke", "always_available").unwrap().as_bool(), Some(false));
        assert_eq!(value(t, "paths", "log_dir").unwrap().as_str(), Some("logs"));
        assert_eq!(value(t, "arsenal", "dir").unwrap().as_str(), Some("converted\\guns"));
        // new key with its default, the dropped key gone; comments kept
        assert_eq!(value(t, "aim", "new_key").unwrap().as_integer(), Some(3));
        assert!(value(t, "keys", "old_key").is_none());
        assert!(t.contains("# release toml") && t.contains("# new in this release") && t.contains("# cycle order"));
        assert!(t.contains("toggle            = \"F6\"      # F1-F12"), "comment column kept:\n{t}");
        assert!(t.contains("sensitivity = 1.35  # mouse"), "{t}");
        assert_eq!(plan::toml_build_sections(t), plan::toml_build_sections(RELEASE));
        // the report
        for k in ["[keys] toggle", "[aim] sensitivity", "[arsenal] enabled", "[arsenal] preload", "[launchers] rocket_impact", "[third_person] shoulder_offset_m"] {
            assert!(m.kept.contains(&k.to_string()), "{k}: {:?}", m.kept);
        }
        assert_eq!(m.kept.len(), 6, "{:?}", m.kept);
        for k in ["[damage] k (build input)", "[regulation.arsenal] scale (build input)", "[arsenal] guns (build input)", "[launchers] explosion_tiers (build input)", "[nuke] always_available (release policy)", "[paths] log_dir (file layout)", "[arsenal] dir (file layout)"] {
            assert!(m.release_owned.contains(&k.to_string()), "{k}: {:?}", m.release_owned);
        }
        assert_eq!(m.added, vec!["[aim] new_key".to_string()]);
        assert_eq!(m.dropped, vec!["[keys] old_key".to_string()]);
        assert!(m.summary().starts_with("mw2er.toml merged: 6 of your setting(s) kept"), "{}", m.summary());
        assert!(m.lines().iter().any(|l| l == "mw2er.toml removed: [keys] old_key"));
    }

    #[test]
    fn a_new_default_reaches_a_player_who_never_changed_it() {
        let base = RELEASE.replace("sensitivity = 1.0", "sensitivity = 0.8").replace("reload            = \"R\"", "reload            = \"T\"");
        // the player changed reload (T -> E) but not the sensitivity (still the old 0.8)
        let player = base.replace("reload            = \"T\"", "reload = \"E\"");
        let m = merge(&player, Some(&base), RELEASE, &locked());
        assert_eq!(value(&m.text, "aim", "sensitivity").unwrap().as_float(), Some(1.0), "new default");
        assert_eq!(value(&m.text, "keys", "reload").unwrap().as_str(), Some("E"), "the player's own value");
        assert_eq!(m.new_defaults, vec!["[aim] sensitivity".to_string()]);
        assert_eq!(m.kept, vec!["[keys] reload".to_string()]);
        // without the base (an install before P3) the old value counts as the player's
        let m2 = merge(&player, None, RELEASE, &locked());
        assert_eq!(value(&m2.text, "aim", "sensitivity").unwrap().as_float(), Some(0.8));
        // the same file: nothing to do, byte-identical to the release
        let same = merge(RELEASE, Some(RELEASE), RELEASE, &locked());
        assert_eq!(same.text, RELEASE);
        assert!(same.kept.is_empty() && same.added.is_empty() && same.dropped.is_empty());
    }

    #[test]
    fn unusable_values_and_files_fall_back_to_the_release() {
        // another type, a duplicate, an all-zero shoulder camera
        let player = "[keys]\ntoggle = 5\n[aim]\nsensitivity = 2.0\nsensitivity = 3.0\n[third_person]\nshoulder_offset_m = [0.0, 0.0, 0.0]\n";
        let m = merge(player, None, RELEASE, &locked());
        assert!(m.notes.iter().any(|n| n.contains("did not parse")), "duplicate key = invalid toml: {:?}", m.notes);
        assert_eq!(m.text, RELEASE);
        let player = "[keys]\ntoggle = 5\n[third_person]\nshoulder_offset_m = [0.0, 0.0, 0.0]\n";
        let m = merge(player, None, RELEASE, &locked());
        assert_eq!(value(&m.text, "keys", "toggle").unwrap().as_str(), Some("F5"));
        assert_eq!(value(&m.text, "third_person", "shoulder_offset_m").unwrap().as_array().unwrap()[0].as_float(), Some(0.55));
        assert!(m.notes.iter().any(|n| n.contains("[keys] toggle") && n.contains("another type")), "{:?}", m.notes);
        assert!(m.notes.iter().any(|n| n.contains("shoulder camera")), "{:?}", m.notes);
        // a broken player file
        let m = merge("[keys\ntoggle = ", None, RELEASE, &locked());
        assert_eq!(m.text, RELEASE);
        assert!(m.notes[0].contains("did not parse"));
    }

    #[test]
    fn multi_line_values_and_crlf() {
        let rel = RELEASE.replace("preload = \"all\"", "order = [\n  1,\n  2,\n]\npreload = \"all\"").replace('\n', "\r\n");
        let player = "[arsenal]\norder = [3,\n 4]\npreload = \"all\"\n";
        let m = merge(player, None, &rel, &locked());
        assert_eq!(value(&m.text, "arsenal", "order").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(value(&m.text, "arsenal", "order").unwrap().as_array().unwrap()[0].as_integer(), Some(3));
        assert!(m.text.contains("\r\n") && !m.text.replace("\r\n", "").contains('\n'), "CRLF kept");
        assert!(m.text.contains("guns = [   # cycle order"), "the locked multi-line guns of the release");
        assert_eq!(m.kept, vec!["[arsenal] order".to_string()]);
    }

    #[test]
    fn the_policy_toml_round_trips_through_the_policy() {
        // a player toml under the policy keeps a changed hot value; the result still fits the policy
        let rel = friend_toml::apply(POLICY_TOML).unwrap().text;
        let player = rel.replace("shoulder_offset_m = [0.55, 0.25, 0.0]", "shoulder_offset_m = [0.65, 0.2, 0.05]").replace("always_available     = false", "always_available = true");
        let m = merge(&player, None, &rel, &locked());
        let a = friend_toml::apply(&m.text).unwrap();
        assert!(a.changed.is_empty(), "already fits: {:?}", a.changed);
        assert!(m.text.contains("[0.65, 0.2, 0.05]"));
        assert!(m.release_owned.iter().any(|k| k.starts_with("[nuke] always_available")));
        assert_eq!(release_owned("gamepad.bind", "heal", &[]), Some("release policy"));
        assert_eq!(release_owned("regulation.nuke", "x", &[]), Some("build input"));
        assert_eq!(release_owned("custom", "x", &["custom".into()]), Some("build input"));
        assert_eq!(release_owned("custom.sub", "x", &["custom".into()]), Some("build input"));
        assert_eq!(release_owned("hud", "scale", &[]), None);
        assert_eq!(release_owned("", "schema", &[]), Some("file format"));
        assert_eq!(value_end("a = \"x # y\" # c"), 12);
        assert_eq!(value_end("'C:\\#x' # c"), 8);
    }

    /// The real files: the host's mw2er.toml merged into the release's (read only; skipped when absent).
    #[test]
    fn real_host_toml_into_the_release_toml() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..");
        let (Ok(host), Ok(rel)) = (std::fs::read_to_string(root.join("mw2er.toml")), std::fs::read_to_string(root.join(r"dist\release\0.2.0\stage\MW2inER\mw2er.toml"))) else {
            return;
        };
        let m = merge(&host, None, &rel, &locked());
        assert!(m.notes.is_empty(), "{:?}", m.notes);
        assert!(m.text.parse::<toml::Table>().is_ok());
        assert!(friend_toml::apply(&m.text).is_ok(), "the merged file fits the policy");
        assert_eq!(plan::toml_build_sections(&m.text), plan::toml_build_sections(&rel));
        // the host's own probes are policy keys: they stay off
        assert!(m.release_owned.iter().any(|k| k.starts_with("[coop] probe_er_depth ")), "{:?}", m.release_owned);
        // the release merged into itself is itself
        assert_eq!(merge(&rel, Some(&rel), &rel, &locked()).text, rel);
    }
}
