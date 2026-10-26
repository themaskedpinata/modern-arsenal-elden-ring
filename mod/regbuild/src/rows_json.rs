//! `build\rows.json` (build_regulation.write_rows_json): the same keys, order and int / float types, dumped like
//! `json.dumps(data, indent=2) + "\n"`. `generator` says regbuild (decided once); `built_utc` and
//! `package_regulation_sha256` are per build (coop_manifest hashes the file without `built_utc`).

use crate::plan::Plan;
use crate::pyfmt::Json;

pub fn rows_json(p: &Plan, built_utc: &str, package_sha: &str, hit_radius: f64) -> Json {
    let ac = &p.ac130;
    let g = |k: &str| ac.get(k).cloned().unwrap_or(Json::Null);
    Json::obj()
        .with("schema", 1)
        .with("generator", crate::GENERATOR)
        .with("built_utc", built_utc)
        .with("vanilla_sha256", crate::VANILLA_SHA256)
        .with("package_regulation_sha256", package_sha)
        .with("k", p.k)
        .with("id_base", p.id_base)
        .with("marker_id", p.marker_id)
        .with(
            "bullet",
            Json::obj().with("speed_mps", p.speed).with("life_s", p.life).with("hit_radius_m", hit_radius).with("clone_from", p.clone_bullet),
        )
        .with("atk", Json::obj().with("clone_from", p.clone_atk))
        .with("weapons", p.weapons.clone())
        .with("arsenal", p.arsenal.clone())
        .with("nuke", p.nuke.clone())
        .with("coop", p.coop.clone())
        .with(
            "ac130",
            Json::obj()
                .with("k", g("k"))
                .with("gun_scale", g("gun_scale"))
                .with("atk_mode", g("atk_mode"))
                .with("protect_speffect_id", g("protect_speffect_id"))
                .with("protect_endurance_s", g("protect_endurance_s"))
                .with("clone", g("clone"))
                .with("guns", g("guns")),
        )
}

/// The keys that differ between two correct builds (or between the WitchyBND and the regbuild build).
pub const VOLATILE: &[&str] = &["built_utc", "generator", "package_regulation_sha256"];

/// Compares two rows.json texts without the volatile keys: the differing JSON paths ([] = equal values and types).
pub fn compare(a: &str, b: &str) -> Result<Vec<String>, String> {
    let pa: serde_json::Value = serde_json::from_str(a).map_err(|e| format!("rows.json A: {e}"))?;
    let pb: serde_json::Value = serde_json::from_str(b).map_err(|e| format!("rows.json B: {e}"))?;
    let strip = |v: &serde_json::Value| {
        let mut j = Json::from_serde(v);
        if let Json::Obj(m) = &mut j {
            m.retain(|(k, _)| !VOLATILE.contains(&k.as_str()));
        }
        j.sorted()
    };
    let mut out = Vec::new();
    walk(&strip(&pa), &strip(&pb), "$", &mut out);
    Ok(out)
}

fn walk(a: &Json, b: &Json, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Json::Obj(x), Json::Obj(y)) => {
            for (k, va) in x {
                match b.get(k) {
                    Some(vb) => walk(va, vb, &format!("{path}.{k}"), out),
                    None => out.push(format!("{path}.{k}: only in A")),
                }
            }
            for (k, _) in y {
                if a.get(k).is_none() {
                    out.push(format!("{path}.{k}: only in B"));
                }
            }
        }
        (Json::Arr(x), Json::Arr(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: {} vs {} items", x.len(), y.len()));
            }
            for (n, (va, vb)) in x.iter().zip(y).enumerate() {
                walk(va, vb, &format!("{path}[{n}]"), out);
            }
        }
        (x, y) if x == y => {}
        (x, y) => out.push(format!("{path}: {} vs {}", x.dumps(), y.dumps())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_ignores_volatile_keys_and_sees_int_vs_float() {
        let a = r#"{"schema": 1, "generator": "build_regulation.py", "built_utc": "1", "k": 4.0, "x": {"b": [1, 2]}}"#;
        let b = r#"{"schema": 1, "generator": "regbuild 0.2.0", "built_utc": "2", "x": {"b": [1, 2]}, "k": 4.0}"#;
        assert_eq!(compare(a, b).unwrap(), Vec::<String>::new());
        let c = r#"{"schema": 1, "k": 4, "x": {"b": [1, 3]}}"#;
        let d = compare(a, c).unwrap();
        assert_eq!(d, vec!["$.k: 4.0 vs 4".to_string(), "$.x.b[1]: 2 vs 3".to_string()]);
    }
}
