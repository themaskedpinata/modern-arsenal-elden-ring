//! The release's `mw2er.toml` policy (tools\coop_friend\friend_toml.ps1 ported line for line; MODLOG 2026-10-22
//! "installer P0"): every test probe / unfinished experiment is forced OFF, and the in-game-tested features must be
//! ON. The P0 packager applies it to the host toml; the installer applies it again to the toml it installs (forced
//! keys rewritten, a missing required feature refuses the install), so no player ever runs a probe by accident.
//!
//! Semantics kept from the PowerShell original (PowerShell compares strings case-insensitively): a line-based,
//! section-aware scan; a forced key that appears twice is a problem, an absent forced key is left to its code default
//! (the same safe value); a forced line is rewritten as `key = value # friend package: why` (key padded to 20,
//! value to 8); a required key must appear exactly once with the tested value; both `[third_person]` shoulder offsets
//! must be three numbers, not all zero. The output keeps the line count and the line ending; it is idempotent.
//! The build inputs (`[damage]`, `[regulation*]`, `[arsenal] guns`, `[launchers] explosion_tiers`) are never touched
//! by the policy.

/// (key, forced value as a TOML literal, why).
pub type Forced = (&'static str, &'static str, &'static str);

/// section -> keys forced.
pub const FORCED: &[(&str, &[Forced])] = &[
    (
        "nuke",
        &[
            ("always_available", "false", "the nuke needs the 25-kill streak (the host may test with true)"),
            ("probe_forward", "false", "not built; must stay false"),
        ],
    ),
    ("ui", &[("probe_scaleform", "false", "test probe off"), ("probe_layer_sample", "false", "test probe off")]),
    ("movement", &[("write_last_update", "false", "probe off"), ("fp_anim_speed_scale", "false", "not built; must stay false")]),
    (
        "coop",
        &[
            ("probe_er_depth", "false", "test probe off"),
            ("probe_er_depth_state", "false", "test probe off"),
            ("probe_er_depth_sample", "false", "test probe off"),
            ("probe_er_aim", "false", "test probe off"),
            ("world_gun_er_depth", "false", "ER-depth occlusion is a test; body capsules + map rays stay on"),
            ("er_carrier", "\"off\"", "crossbow carrier experiment stopped (2026-10-22)"),
            ("carrier_give", "false", "never give an item into the save"),
            ("carrier_load", "false", "carrier experiment off"),
            ("tp_ads_er_aim", "false", "old probe alias of tp_ads_pose = \"er_aim\""),
            ("tp_aim_cam", "\"keep\"", "acts only with tp_ads_pose = \"er_aim\"; keep = our normal camera"),
        ],
    ),
];

/// section -> key -> required value (TOML literal as written) = the in-game-tested features shipped ON.
pub const REQUIRED: &[(&str, &[(&str, &str)])] = &[
    ("keys", &[("heal", "\"Q\"")]),
    ("ui", &[("inject_viewmodel", "true"), ("inject_ui_thread", "true")]),
    ("ui_yield", &[("mode", "\"cutout\"")]),
    ("gamepad", &[("enabled", "true"), ("er_use_item", "\"x\"")]),
    ("gamepad.bind", &[("heal", "\"dpad_right\"")]),
    ("first_person", &[("tp_scope_peek", "true")]),
    ("nuke", &[("allow_in_coop", "true")]),
    (
        "coop",
        &[
            ("hold_mode", "\"hand\""),
            ("er_stance", "\"polearm\""),
            ("tp_ads_pose", "\"guard\""),
            ("tp_guard_yaw", "true"),
            ("tp_ads_hold", "\"aim\""),
            ("hide_er_weapon", "true"),
            ("hide_state_patch", "true"),
            ("world_gun_body_occlusion", "true"),
            ("world_gun_occlusion", "true"),
            ("world_guns", "true"),
        ],
    ),
];

/// `[third_person]` keys that must hold three numbers, not all zero (the accepted shoulder camera).
pub const SHOULDER_KEYS: &[&str] = &["shoulder_offset_m", "shoulder_ads_offset_m"];

/// One key line: 0-based line index, section, key, value (text before a `#` outside a "..." string, trimmed).
#[derive(Clone, Debug, PartialEq)]
pub struct KeyLine {
    pub index: usize,
    pub section: String,
    pub key: String,
    pub value: String,
}

/// `^\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$` on the trimmed line.
pub(crate) fn section_header(t: &str) -> Option<String> {
    let rest = t.strip_prefix('[')?;
    let rest = rest.strip_prefix('[').unwrap_or(rest);
    let close = rest.find(']')?;
    let name = rest[..close].trim();
    if rest[..close].trim_start().is_empty() || name.is_empty() {
        return None;
    }
    let after = &rest[close + 1..];
    let after = after.strip_prefix(']').unwrap_or(after).trim_start();
    (after.is_empty() || after.starts_with('#')).then(|| name.to_string())
}

/// `^([A-Za-z0-9_]+)\s*=\s*(.*)$` on the trimmed line -> (key, value before an unquoted `#`, trimmed).
pub(crate) fn key_value(t: &str) -> Option<(String, String)> {
    let n = t.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    if n == 0 {
        return None;
    }
    let rest = t[n..].trim_start().strip_prefix('=')?;
    let mut v = String::new();
    let mut inq = false;
    for ch in rest.chars() {
        if ch == '"' {
            inq = !inq;
        }
        if ch == '#' && !inq {
            break;
        }
        v.push(ch);
    }
    Some((t[..n].to_string(), v.trim().to_string()))
}

/// Every key line with its section (`Get-TomlKeyLines`).
pub fn key_lines(lines: &[&str]) -> Vec<KeyLine> {
    let mut out = Vec::new();
    let mut sec = String::new();
    for (index, l) in lines.iter().enumerate() {
        let t = l.trim();
        if let Some(s) = section_header(t) {
            sec = s;
            continue;
        }
        if let Some((key, value)) = key_value(t) {
            out.push(KeyLine { index, section: sec.clone(), key, value });
        }
    }
    out
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn hits<'a>(kl: &'a [KeyLine], sec: &str, key: &str) -> Vec<&'a KeyLine> {
    kl.iter().filter(|k| eq(&k.section, sec) && eq(&k.key, key)).collect()
}

/// The numbers of a value (`-?\d+(\.\d+)?`).
pub(crate) fn numbers(s: &str) -> Vec<f64> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let neg = b[i] == b'-' && i + 1 < b.len() && b[i + 1].is_ascii_digit();
        let start = i;
        let mut j = if neg { i + 1 } else { i };
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            out.push(s[start..j].parse().unwrap_or(0.0));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The result of [`apply`]: the policy text and the forced values that changed (`[sec] key old -> new`).
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub text: String,
    pub changed: Vec<String>,
}

/// `ConvertTo-FriendToml`: the policy text, or every problem found (joined like the PowerShell error).
pub fn apply(toml: &str) -> Result<Applied, String> {
    let nl = if toml.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = toml.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let kl = key_lines(&refs);
    let mut problems = Vec::new();
    let mut changed = Vec::new();
    for (sec, keys) in FORCED {
        for (key, val, why) in *keys {
            let h = hits(&kl, sec, key);
            if h.len() > 1 {
                problems.push(format!("[{sec}] {key} appears {} times", h.len()));
                continue;
            }
            let Some(h) = h.first() else { continue };
            if !eq(&h.value, val) {
                changed.push(format!("[{sec}] {key} {} -> {val}", h.value));
            }
            lines[h.index] = format!("{key:<20} = {val:<8} # friend package: {why}");
        }
    }
    for (sec, keys) in REQUIRED {
        for (key, want) in *keys {
            let h = hits(&kl, sec, key);
            if h.len() != 1 {
                problems.push(format!("[{sec}] {key} must appear once (found {})", h.len()));
                continue;
            }
            if !eq(&h[0].value, want) {
                problems.push(format!("[{sec}] {key} = {} (the friend package ships the tested {want})", h[0].value));
            }
        }
    }
    for key in SHOULDER_KEYS {
        let h = hits(&kl, "third_person", key);
        if h.len() != 1 {
            problems.push(format!("[third_person] {key} must appear once"));
            continue;
        }
        let n = numbers(&h[0].value);
        if n.len() != 3 || n.iter().all(|x| *x == 0.0) {
            problems.push(format!("[third_person] {key} = {} (the friend package ships the accepted non-zero shoulder camera)", h[0].value));
        }
    }
    if !problems.is_empty() {
        return Err(format!("mw2er.toml does not fit the friend policy: {}", problems.join("; ")));
    }
    Ok(Applied { text: lines.join(nl), changed })
}

#[cfg(test)]
pub(crate) mod fixture {
    /// A toml that fits the policy, with two forced keys in their unsafe state and every build-input section.
    /// Its build-input sections equal the older fixture's (`[damage] scale = 1.0`, `[arsenal] guns = ["m4a1"]`).
    pub const POLICY_TOML: &str = "# test toml\n[damage]\nscale = 1.0\n\n[keys]\nheal = \"Q\"\n\n\
[ui]\ninject_viewmodel = true\ninject_ui_thread = true\nprobe_scaleform = true   # testing\n\n[ui_yield]\nmode = \"cutout\"\n\n\
[gamepad]\nenabled = true\ner_use_item = \"x\"\n\n[gamepad.bind]\nheal = \"dpad_right\"\n\n[first_person]\ntp_scope_peek = true\n\n\
[third_person]\nshoulder_offset_m = [0.55, 0.25, 0.0]\nshoulder_ads_offset_m = [0.45, 0.15, -0.6]\n\n\
[nuke]\nallow_in_coop = true\nalways_available = true\n\n\
[coop]\nhold_mode = \"hand\"\ner_stance = \"polearm\"\ntp_ads_pose = \"guard\"\ntp_guard_yaw = true\ntp_ads_hold = \"aim\"\n\
hide_er_weapon = true\nhide_state_patch = true\nworld_gun_body_occlusion = true\nworld_gun_occlusion = true\nworld_guns = true\n\
er_carrier = \"off\"\n\n[arsenal]\nguns = [\"m4a1\"]\n";
}

#[cfg(test)]
mod tests {
    use super::fixture::POLICY_TOML;
    use super::*;

    fn value(text: &str, sec: &str, key: &str) -> Option<String> {
        let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
        key_lines(&lines).into_iter().find(|k| k.section == sec && k.key == key).map(|k| k.value)
    }

    #[test]
    fn forces_probes_off_and_keeps_everything_else() {
        let a = apply(POLICY_TOML).unwrap();
        assert_eq!(a.changed, vec!["[nuke] always_available true -> false".to_string(), "[ui] probe_scaleform true -> false".to_string()]);
        assert_eq!(value(&a.text, "nuke", "always_available").as_deref(), Some("false"));
        assert_eq!(value(&a.text, "ui", "probe_scaleform").as_deref(), Some("false"));
        assert!(a.text.contains("probe_scaleform      = false    # friend package: test probe off"));
        assert_eq!(value(&a.text, "coop", "er_carrier").as_deref(), Some("\"off\""));
        assert_eq!(value(&a.text, "gamepad.bind", "heal").as_deref(), Some("\"dpad_right\""));
        assert_eq!(value(&a.text, "keys", "heal").as_deref(), Some("\"Q\""));
        assert_eq!(a.text.split('\n').count(), POLICY_TOML.split('\n').count(), "line count kept");
        assert_eq!(apply(&a.text).unwrap().text, a.text, "idempotent");
        assert!(apply(&a.text).unwrap().changed.is_empty());
        assert_eq!(crate::plan::toml_build_sections(&a.text), crate::plan::toml_build_sections(POLICY_TOML), "build inputs untouched");
        // CRLF stays CRLF
        let crlf = POLICY_TOML.replace('\n', "\r\n");
        let c = apply(&crlf).unwrap();
        assert!(c.text.contains("\r\n") && !c.text.replace("\r\n", "").contains('\n'));
        assert_eq!(c.text.replace("\r\n", "\n"), a.text);
    }

    #[test]
    fn mutations_are_refused() {
        let muts: &[(&str, &str, &str)] = &[
            ("tp_ads_pose off", "tp_ads_pose = \"guard\"", "tp_ads_pose = \"off\""),
            ("er_stance off", "er_stance = \"polearm\"", "er_stance = \"off\""),
            ("cutout off", "mode = \"cutout\"", "mode = \"lower\""),
            ("inject off", "inject_ui_thread = true", "inject_ui_thread = false"),
            ("shoulder zero", "shoulder_offset_m = [0.55, 0.25, 0.0]", "shoulder_offset_m = [0.0, 0.0, 0.0]"),
            ("shoulder two numbers", "shoulder_ads_offset_m = [0.45, 0.15, -0.6]", "shoulder_ads_offset_m = [0.45, 0.15]"),
            ("duplicate forced", "er_carrier = \"off\"", "er_carrier = \"off\"\ner_carrier = \"on\""),
            ("pad heal gone", "heal = \"dpad_right\"", "heal = \"\""),
            ("required missing", "world_guns = true\n", ""),
        ];
        for (name, from, to) in muts {
            let bad = POLICY_TOML.replacen(from, to, 1);
            assert_ne!(bad, POLICY_TOML, "{name} applies");
            assert!(apply(&bad).is_err(), "{name} refused");
        }
        let e = apply("[ui]\nx = 1\n").unwrap_err();
        assert!(e.starts_with("mw2er.toml does not fit the friend policy: [keys] heal must appear once (found 0)"), "{e}");
    }

    #[test]
    fn line_scanner_matches_the_powershell_rules() {
        assert_eq!(section_header("[gamepad.bind]"), Some("gamepad.bind".into()));
        assert_eq!(section_header("[[ natives ]] # x"), Some("natives".into()));
        assert_eq!(section_header("[ui] x"), None);
        assert_eq!(section_header("[]"), None);
        assert_eq!(key_value("a = \"x # y\" # c"), Some(("a".into(), "\"x # y\"".into())));
        assert_eq!(key_value("a.b = 1"), None);
        assert_eq!(key_value("=1"), None);
        assert_eq!(numbers("[0.55, -0.25, 3]"), vec![0.55, -0.25, 3.0]);
        assert_eq!(numbers("[-, 1.]"), vec![1.0]);
        // case-insensitive like PowerShell's -eq
        let up = POLICY_TOML.replace("tp_guard_yaw = true", "tp_guard_yaw = TRUE");
        assert!(apply(&up).is_ok());
    }

    /// Parity with friend_toml.ps1 on real files (manual; never run by default):
    /// MW2ER_TOML_PARITY_IN=<host mw2er.toml> MW2ER_TOML_PARITY_PS=<ConvertTo-FriendToml output> cargo test -p installer
    /// friend_toml::tests::parity_with_powershell -- --ignored
    #[test]
    #[ignore]
    fn parity_with_powershell() {
        let i = std::env::var("MW2ER_TOML_PARITY_IN").expect("MW2ER_TOML_PARITY_IN");
        let p = std::env::var("MW2ER_TOML_PARITY_PS").expect("MW2ER_TOML_PARITY_PS");
        let host = std::fs::read_to_string(i).unwrap();
        let ps = std::fs::read_to_string(p).unwrap();
        let a = apply(&host).unwrap();
        assert_eq!(a.text, ps);
        println!("parity OK: {} forced change(s): {:?}", a.changed.len(), a.changed);
    }
}
