//! The P3 update path, the parts without side effects (docs/INSTALLER_DESIGN.md 2.8): which kind of run this is
//! ("Update 0.2.0 -> 0.2.1", repair, downgrade), what an update changes (payload files by sha256, the build inputs
//! of `mw2er.toml`, the settings merge), the co-op id before / after with the "both players must update" note, and
//! the opt-in "newer release" check (decision D6: manual for the friend - the host sends the new setup exe; for the
//! public an opt-in check of GitHub Releases).
//!
//! The GitHub check runs only when the player ticks it (off by default) and only for a release whose
//! `manifest\release.json` names `update_repo` (empty = the feature is hidden). It asks exactly one URL on the
//! pinned host [`GITHUB_API`] (`/repos/<owner>/<repo>/releases/latest`), never downloads anything, and offers the
//! release page (https://github.com/<repo>/releases/...) to open in the browser on a click.

use std::cmp::Ordering;
use std::path::Path;

use crate::plan::{self, StepId};
use crate::util;

/// The only host the release check talks to.
pub const GITHUB_API: &str = "https://api.github.com";

/// `0.2.1`, `v0.2.1`, `0.3.0-rc1` -> ([0, 2, 1], pre-release?). None = not a version.
pub fn parse_version(s: &str) -> Option<(Vec<u64>, bool)> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let (core, pre) = match s.split_once(['-', '+']) {
        Some((c, rest)) => (c, s.as_bytes()[c.len()] == b'-' && !rest.is_empty()),
        None => (s, false),
    };
    let nums: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    nums.filter(|n| !n.is_empty() && n.len() <= 4).map(|n| (n, pre))
}

/// SemVer-like order (missing parts = 0; a pre-release sorts before its release). Unparsable versions compare as
/// strings after every parsable one.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (parse_version(a), parse_version(b)) {
        (Some((x, xp)), Some((y, yp))) => {
            let n = x.len().max(y.len());
            let get = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
            (0..n).map(|i| get(&x, i).cmp(&get(&y, i))).find(|o| o.is_ne()).unwrap_or_else(|| yp.cmp(&xp))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No install in the root.
    Fresh,
    /// The installed version again.
    Repair,
    /// A newer release over the installed one.
    Update,
    /// An older release over the installed one.
    Downgrade,
}

pub fn kind(installed: Option<&str>, release: &str) -> Kind {
    match installed.filter(|v| !v.is_empty()) {
        None => Kind::Fresh,
        Some(v) => match compare(release, v) {
            Ordering::Equal => Kind::Repair,
            Ordering::Greater => Kind::Update,
            Ordering::Less => Kind::Downgrade,
        },
    }
}

/// One line of "What changes".
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub what: String,
    pub detail: String,
}

/// What an update / repair will do, for the window's Update card and `--cli status`.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub kind: Kind,
    pub from_version: String,
    pub to_version: String,
    pub from_coop_id: String,
    pub to_coop_id: String,
    pub changes: Vec<Change>,
    /// The steps that will run (plan order).
    pub steps: Vec<StepId>,
    /// The settings merge preview ([`crate::toml_merge::Merged::summary`]).
    pub settings: Option<String>,
}

impl Summary {
    pub fn headline(&self) -> String {
        match self.kind {
            Kind::Fresh => format!("Install {}", self.to_version),
            Kind::Repair => format!("Repair {}", self.to_version),
            Kind::Update => format!("Update {} -> {}", self.from_version, self.to_version),
            Kind::Downgrade => format!("Downgrade {} -> {}", self.from_version, self.to_version),
        }
    }

    /// The co-op id changes (an installed id that differs from the new one).
    pub fn coop_changes(&self) -> bool {
        !self.from_coop_id.is_empty() && !self.to_coop_id.is_empty() && self.from_coop_id != self.to_coop_id
    }

    pub fn coop_note(&self) -> Option<String> {
        self.coop_changes().then(|| {
            format!(
                "The co-op id changes ({} -> {}): both players must install {} before playing together.",
                self.from_coop_id, self.to_coop_id, self.to_version
            )
        })
    }

    /// The build steps that run (the always-run checks and verifies left out).
    pub fn build_steps(&self) -> Vec<StepId> {
        self.steps.iter().copied().filter(|s| matches!(s, StepId::Payload | StepId::Witchy | StepId::Dumps | StepId::Converters | StepId::Regulation)).collect()
    }

    /// Text lines (`--cli status`, the install log).
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![self.headline()];
        if matches!(self.kind, Kind::Update | Kind::Downgrade | Kind::Repair) {
            v.push(format!(
                "co-op id: {} -> {}{}",
                if self.from_coop_id.is_empty() { "?" } else { &self.from_coop_id },
                self.to_coop_id,
                if self.coop_changes() { " (changes)" } else { " (unchanged)" }
            ));
        }
        if let Some(n) = self.coop_note() {
            v.push(format!("NOTE {n}"));
        }
        if self.kind != Kind::Fresh {
            if self.changes.is_empty() {
                v.push("what changes: nothing in the mod's files".into());
            }
            for c in &self.changes {
                v.push(format!("change: {}{}", c.what, if c.detail.is_empty() { String::new() } else { format!(" ({})", c.detail) }));
            }
            if let Some(s) = &self.settings {
                v.push(s.clone());
            }
            let b = self.build_steps();
            v.push(format!(
                "rebuilds: {}",
                if b.iter().any(|s| *s != StepId::Payload) {
                    b.iter().filter(|s| **s != StepId::Payload).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                } else {
                    "nothing (no dumps, converters or regulation)".into()
                }
            ));
        }
        v
    }
}

/// The group of a payload file in "What changes" (None = not listed: the manifests, the settings file).
fn group(rel: &str) -> Option<&'static str> {
    let r = rel.to_lowercase();
    Some(match r.as_str() {
        "mw2er.toml" => return None,
        "build/natives/mw2er.dll" => "The mod (mw2er.dll)",
        _ if r.starts_with("manifest/") => return None,
        _ if r.starts_with("tools/scripts/") => "Build scripts",
        _ if r.starts_with("tools/oat/") || r.starts_with("tools/bin/") => "Build tools",
        _ if r.starts_with("runtime/") => "Python runtime",
        _ if r.starts_with("tools/me3/") => "Bundled me3 installer",
        _ if r.starts_with("profiles/") => "me3 profile templates",
        _ => "Other files (guides, licences, fallback scripts)",
    })
}

/// The payload files that differ from the root's (sha256), grouped. `files` = (rel, sha256 of the new file).
pub fn file_changes(root: &Path, files: &[(String, String)]) -> Vec<Change> {
    let mut groups: Vec<(&'static str, Vec<String>, Vec<String>)> = Vec::new();
    for (rel, sha) in files {
        let Some(g) = group(rel) else { continue };
        let dest = root.join(rel.replace('/', "\\"));
        let new = !dest.is_file();
        if !new && util::sha256_file(&dest).is_ok_and(|h| h.eq_ignore_ascii_case(sha)) {
            continue;
        }
        let pos = match groups.iter().position(|x| x.0 == g) {
            Some(p) => p,
            None => {
                groups.push((g, Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
        if new { groups[pos].2.push(name) } else { groups[pos].1.push(name) }
    }
    let names = |v: &[String]| {
        let mut s = v.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if v.len() > 3 {
            s.push_str(&format!(" +{}", v.len() - 3));
        }
        s
    };
    groups
        .into_iter()
        .map(|(g, changed, added)| {
            let mut d = Vec::new();
            if !changed.is_empty() {
                d.push(format!("{} changed: {}", changed.len(), names(&changed)));
            }
            if !added.is_empty() {
                d.push(format!("{} new: {}", added.len(), names(&added)));
            }
            Change { what: g.to_string(), detail: d.join("; ") }
        })
        .collect()
}

/// The build inputs of `mw2er.toml` that differ (`[damage]`, `[regulation*]`, `arsenal.guns` ...): a change here
/// rebuilds the regulation (and the converted data for `arsenal.guns`).
pub fn build_input_changes(installed_toml: &str, release_toml: &str) -> Option<Change> {
    let a: serde_json::Value = serde_json::from_str(&plan::toml_build_sections(installed_toml)).unwrap_or_default();
    let b: serde_json::Value = serde_json::from_str(&plan::toml_build_sections(release_toml)).unwrap_or_default();
    if a == b {
        return None;
    }
    let empty = serde_json::Map::new();
    let (ma, mb) = (a.as_object().unwrap_or(&empty), b.as_object().unwrap_or(&empty));
    let mut keys: Vec<String> = Vec::new();
    for k in ma.keys().chain(mb.keys()) {
        if ma.get(k) != mb.get(k) && !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let label = |k: &str| match k.split_once('.') {
        Some((s, key)) if plan::BUILD_KEYS.iter().any(|(bs, bk)| *bs == s && *bk == key) => format!("[{s}] {key}"),
        _ => format!("[{k}]"),
    };
    Some(Change { what: "Regulation build settings (mw2er.toml)".into(), detail: keys.iter().map(|k| label(k)).collect::<Vec<_>>().join(", ") })
}

// ---- the opt-in "newer release" check ----------------------------------------------------------------------------

/// `owner/name` of a GitHub repository (letters, digits, `-`, `_`, `.`; no `..`).
pub fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    matches!(repo.split_once('/'), Some((o, n)) if ok(o) && ok(n) && !n.contains('/'))
}

/// The newest release on GitHub.
#[derive(Clone, Debug, PartialEq)]
pub struct Latest {
    pub tag: String,
    /// Newer than the version compared with.
    pub newer: bool,
    /// The release page to open (always under https://github.com/<repo>/releases).
    pub page: String,
    /// sha256 values listed in the release notes (to compare with the downloaded setup's `.sha256`).
    pub sha256: Vec<String>,
}

/// Reads the `releases/latest` answer.
pub fn parse_latest(json: &str, repo: &str, current: &str) -> Result<Latest, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("GitHub sent no release data ({e})"))?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).ok_or("GitHub sent no tag_name")?.to_string();
    let releases = format!("https://github.com/{repo}/releases");
    let page = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .filter(|u| u.to_lowercase().starts_with(&format!("{}/", releases.to_lowercase())) && !u.contains(['"', '\'', ' ', '\\']) && !u.contains(".."))
        .map(str::to_string)
        .unwrap_or(releases);
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let mut sha256 = Vec::new();
    for w in body.split(|c: char| !c.is_ascii_hexdigit()) {
        if util::is_sha256(w) && !sha256.contains(&w.to_lowercase()) {
            sha256.push(w.to_lowercase());
        }
    }
    let newer = parse_version(&tag).is_some() && compare(&tag, current) == Ordering::Greater;
    Ok(Latest { tag, newer, page, sha256 })
}

/// Asks GitHub for the newest release of `repo` (the player ticked the check). Nothing is downloaded.
pub fn check_latest(repo: &str, current: &str) -> Result<Latest, String> {
    check_latest_at(GITHUB_API, repo, current)
}

fn check_latest_at(base: &str, repo: &str, current: &str) -> Result<Latest, String> {
    if base != GITHUB_API && !(cfg!(test) && base.starts_with("http://127.0.0.1:")) {
        return Err(format!("{base} is not the pinned update host"));
    }
    if !valid_repo(repo) {
        return Err(format!("\"{repo}\" is not a GitHub repository name (owner/name)"));
    }
    let url = format!("{base}/repos/{repo}/releases/latest");
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(!cfg!(test))
        .max_redirects(0)
        .user_agent(concat!("MW2inER-setup/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .into();
    let resp = agent.get(&url).header("Accept", "application/vnd.github+json").call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => format!("{repo} has no published release yet"),
        ureq::Error::StatusCode(c) => format!("GitHub answered {c}; try again later"),
        e => format!("cannot reach GitHub ({e})"),
    })?;
    let text = resp.into_body().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("cannot read GitHub's answer ({e})"))?;
    parse_latest(&text, repo, current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::io::{Read, Write};

    #[test]
    fn versions_order_like_semver() {
        assert_eq!(compare("0.2.1", "0.2.0"), Ordering::Greater);
        assert_eq!(compare("v0.10.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare("0.2", "0.2.0"), Ordering::Equal);
        assert_eq!(compare("0.3.0-rc1", "0.3.0"), Ordering::Less);
        assert_eq!(compare("0.3.0-rc1", "0.2.9"), Ordering::Greater);
        assert_eq!(compare("0.3.0+build5", "0.3.0"), Ordering::Equal);
        assert_eq!(compare("abc", "0.1.0"), Ordering::Greater);
        assert_eq!(parse_version("x.1"), None);
        assert_eq!(kind(None, "0.2.0"), Kind::Fresh);
        assert_eq!(kind(Some(""), "0.2.0"), Kind::Fresh);
        assert_eq!(kind(Some("0.2.0"), "0.2.0"), Kind::Repair);
        assert_eq!(kind(Some("0.2.0"), "0.2.1"), Kind::Update);
        assert_eq!(kind(Some("0.2.1"), "0.2.0"), Kind::Downgrade);
    }

    #[test]
    fn summary_text_and_the_coop_note() {
        let mut s = Summary {
            kind: Kind::Update,
            from_version: "0.2.0".into(),
            to_version: "0.2.1".into(),
            from_coop_id: "2799328d".into(),
            to_coop_id: "2799328d".into(),
            changes: vec![Change { what: "The mod (mw2er.dll)".into(), detail: "1 changed: mw2er.dll".into() }],
            steps: vec![StepId::Preflight, StepId::Payload, StepId::VerifyAll, StepId::Finish],
            settings: Some("mw2er.toml merged: 3 of your setting(s) kept".into()),
        };
        assert_eq!(s.headline(), "Update 0.2.0 -> 0.2.1");
        assert!(!s.coop_changes() && s.coop_note().is_none());
        let l = s.lines();
        assert!(l.contains(&"co-op id: 2799328d -> 2799328d (unchanged)".to_string()), "{l:?}");
        assert!(l.contains(&"rebuilds: nothing (no dumps, converters or regulation)".to_string()), "{l:?}");
        assert!(l.contains(&"change: The mod (mw2er.dll) (1 changed: mw2er.dll)".to_string()));
        s.to_coop_id = "0badc0de".into();
        s.steps.insert(2, StepId::Regulation);
        assert!(s.coop_note().unwrap().contains("both players must install 0.2.1"));
        assert!(s.lines().iter().any(|x| x == "rebuilds: regulation"));
        assert!(s.lines().iter().any(|x| x.starts_with("NOTE The co-op id changes (2799328d -> 0badc0de)")));
        s.kind = Kind::Fresh;
        s.from_coop_id.clear();
        assert_eq!(s.lines(), vec!["Install 0.2.1".to_string()]);
        s.kind = Kind::Downgrade;
        assert_eq!(s.headline(), "Downgrade 0.2.0 -> 0.2.1");
    }

    #[test]
    fn file_changes_by_hash() {
        let t = TempDir::new("upd-files");
        t.write(r"root\build\natives\mw2er.dll", b"old");
        t.write(r"root\tools\scripts\a.py", b"same");
        t.write(r"root\mw2er.toml", b"player");
        let files = vec![
            ("build/natives/mw2er.dll".to_string(), util::sha256_bytes(b"new")),
            ("tools/scripts/a.py".to_string(), util::sha256_bytes(b"same")),
            ("tools/scripts/b.py".to_string(), util::sha256_bytes(b"b")),
            ("mw2er.toml".to_string(), util::sha256_bytes(b"release")),
            ("manifest/release.json".to_string(), util::sha256_bytes(b"x")),
            ("README.md".to_string(), util::sha256_bytes(b"r")),
        ];
        let c = file_changes(&t.0.join("root"), &files);
        assert_eq!(
            c,
            vec![
                Change { what: "The mod (mw2er.dll)".into(), detail: "1 changed: mw2er.dll".into() },
                Change { what: "Build scripts".into(), detail: "1 new: b.py".into() },
                Change { what: "Other files (guides, licences, fallback scripts)".into(), detail: "1 new: README.md".into() },
            ]
        );
        assert!(file_changes(&t.0.join("root"), &files[1..2]).is_empty());
        // the build inputs of the toml
        let a = "[damage]\nk = 4.0\n[hud]\nx = 1\n[arsenal]\nguns = [\"a\"]\n";
        assert_eq!(build_input_changes(a, &a.replace("x = 1", "x = 2")), None, "hot settings are no build input");
        let ch = build_input_changes(a, &a.replace("k = 4.0", "k = 5.0").replace("[\"a\"]", "[\"b\"]")).unwrap();
        assert_eq!(ch.detail, "[arsenal] guns, [damage]");
    }

    #[test]
    fn release_answer_parsing() {
        let body = r#"{"tag_name":"v0.2.1","html_url":"https://github.com/owner/mw2iner/releases/tag/v0.2.1","body":"MW2inER-0.2.1-setup.exe sha256 AF8B87EA2AC4000000000000000000000000000000000000000000000000ABCD\n"}"#;
        let l = parse_latest(body, "owner/mw2iner", "0.2.0").unwrap();
        assert!(l.newer);
        assert_eq!(l.page, "https://github.com/owner/mw2iner/releases/tag/v0.2.1");
        assert_eq!(l.sha256, vec!["af8b87ea2ac4000000000000000000000000000000000000000000000000abcd".to_string()]);
        assert!(!parse_latest(body, "owner/mw2iner", "0.2.1").unwrap().newer);
        // a page outside the repository's releases is not offered
        let evil = r#"{"tag_name":"0.9.0","html_url":"https://evil.example/x"}"#;
        assert_eq!(parse_latest(evil, "owner/mw2iner", "0.2.0").unwrap().page, "https://github.com/owner/mw2iner/releases");
        let evil2 = r#"{"tag_name":"0.9.0","html_url":"https://github.com/owner/mw2iner/releases/../../other"}"#;
        assert_eq!(parse_latest(evil2, "owner/mw2iner", "0.2.0").unwrap().page, "https://github.com/owner/mw2iner/releases");
        assert!(parse_latest("{}", "o/r", "0.2.0").is_err());
        assert!(parse_latest("not json", "o/r", "0.2.0").is_err());
        assert!(!parse_latest(r#"{"tag_name":"nightly"}"#, "o/r", "0.2.0").unwrap().newer);
        assert!(valid_repo("owner/mw2iner") && valid_repo("a-b/c.d_e"));
        for bad in ["", "owner", "owner/", "/x", "a/b/c", "../x", "a/..", "a b/c", "a/c?x=1", "https://github.com/a/b"] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }

    /// A one-shot local stub of the GitHub API (127.0.0.1 only; the tests never reach the internet). Returns the
    /// base URL and the request line it saw.
    fn stub(status: &'static str, body: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut req = [0u8; 4096];
                let n = s.read(&mut req).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&req[..n]).into_owned());
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    #[test]
    fn the_check_asks_one_url_and_downloads_nothing() {
        let (base, seen) = stub("200 OK", r#"{"tag_name":"0.3.0","html_url":"https://github.com/owner/mw2iner/releases/tag/0.3.0","assets":[{"browser_download_url":"https://github.com/x.exe"}]}"#);
        let l = check_latest_at(&base, "owner/mw2iner", "0.2.0").unwrap();
        assert!(l.newer && l.tag == "0.3.0");
        let req = seen.recv().unwrap();
        assert!(req.starts_with("GET /repos/owner/mw2iner/releases/latest HTTP/1.1"), "{req}");
        assert!(req.to_lowercase().contains("accept: application/vnd.github+json") && req.contains("MW2inER-setup/"), "{req}");
        let (base, _) = stub("404 Not Found", r#"{"message":"Not Found"}"#);
        assert!(check_latest_at(&base, "owner/mw2iner", "0.2.0").unwrap_err().contains("no published release"));
        // only the pinned host (and the local stub in tests); never another URL, never a bad repo name
        assert!(check_latest_at("https://example.com", "owner/mw2iner", "0.2.0").unwrap_err().contains("not the pinned update host"));
        assert!(check_latest_at("http://api.github.com", "owner/mw2iner", "0.2.0").is_err());
        let (base, _) = stub("200 OK", "{}");
        assert!(check_latest_at(&base, "../x", "0.2.0").unwrap_err().contains("not a GitHub repository"));
        // nothing reachable: an error, not a hang
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        drop(l);
        assert!(check_latest_at(&gone, "owner/mw2iner", "0.2.0").unwrap_err().contains("cannot reach GitHub"));
    }
}
