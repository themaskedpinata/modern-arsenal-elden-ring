//! The row plan: a line-by-line port of build_regulation.py `make_plan` / `make_arsenal_plan` / `make_coop_plan` (and
//! their math: band_damages, dmg_at, ac130_tiers, er_damage, inv_er, launcher_tiers, body_mult). Same inputs
//! (mw2er.toml `[damage]`, `[regulation*]`, `[arsenal] guns`, `[launchers] explosion_tiers`, converted\weapons.json,
//! converted\ac130\weapons_ac130.json, converted\guns\<gun>\stats.json), same defaults, same errors, same row order, same
//! rows.json blocks (Python int vs float kept). Python details that matter: `.get(key, default)`, `int()` truncation,
//! round half away from zero (`rhaz`), `round(x, 6)`, `f"{x:g}"`, stable sorts, dict insertion order.

use std::path::Path;

use crate::fields::V;
use crate::pyfmt::{Json, fmt_g, rhaz, round6};

pub type Ov = Vec<(&'static str, V)>;

/// One planned row: build_regulation's `(id, name, overrides, clone_id)`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRow {
    pub id: i64,
    pub name: String,
    pub ov: Ov,
    pub clone: i64,
}

impl PlanRow {
    pub fn get(&self, k: &str) -> Option<V> {
        self.ov.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }
}

/// Python `{**a, k: v}` semantics: a key already present keeps its place.
fn put(ov: &mut Ov, k: &'static str, v: V) {
    match ov.iter_mut().find(|(n, _)| *n == k) {
        Some(slot) => slot.1 = v,
        None => ov.push((k, v)),
    }
}

fn ov(items: &[(&'static str, V)]) -> Ov {
    let mut o = Ov::new();
    for (k, v) in items {
        put(&mut o, k, *v);
    }
    o
}

fn merged(base: &[(&'static str, V)], more: &[(&'static str, V)]) -> Ov {
    let mut o = ov(base);
    for (k, v) in more {
        put(&mut o, k, *v);
    }
    o
}

const fn i(v: i64) -> V {
    V::I(v)
}
const fn f(v: f64) -> V {
    V::F(v)
}

// ---- constants (build_regulation.py) -------------------------------------------------------------------------------

pub const ID_RANGE: (i64, i64) = (79_000_000, 79_999_999);
pub const AC130_RANGE: (i64, i64) = (79_000_200, 79_000_499);
pub const MAX_BANDS: i64 = 16;
pub const INCH: f64 = 0.0254;
pub const AC130_SFX_POSTURE: i64 = 1;
pub const PROTECT_ENDURANCE_DEFAULT_S: f64 = 90.0;
pub const PROTECT_ENDURANCE_RANGE_S: (f64, f64) = (30.0, 600.0);

/// tier -> (atkStam, atkSuperArmor, dmgLevel, knockbackDist, atkObj)
fn ac130_tuning(gid: &str, tier: &str) -> (i64, f64, i64, f64) {
    match (gid, tier) {
        ("105", "core") | ("105", "single") => (200, 50.0, 10, 3.0),
        ("105", "ring") => (100, 25.0, 7, 2.0),
        ("40", "core") | ("40", "single") => (80, 15.0, 7, 1.5),
        ("40", "ring") => (40, 8.0, 3, 1.0),
        _ => (20, 3.0, 2, 0.3),
    }
}

/// tier -> (atkObj, isEnableCalcDamageForBushesObj, asset defence it must beat) (AC130_OBJECT)
pub fn ac130_object(gid: &str, tier: &str) -> (i64, i64, i64) {
    match (gid, tier) {
        ("105", "core") | ("105", "single") => (200, 1, 199),
        ("105", "ring") => (100, 1, 95),
        ("40", "core") | ("40", "single") => (100, 1, 95),
        ("40", "ring") => (60, 0, 9),
        _ => (40, 0, 9),
    }
}

pub const AC130_OBJECT_TIERS: &[(&str, &str)] =
    &[("105", "core"), ("105", "ring"), ("105", "single"), ("40", "core"), ("40", "ring"), ("40", "single"), ("25", "single")];

const AC130_SFX_DEFAULT: &[(&str, i64)] =
    &[("105", 524_028), ("105_ring", 530_123), ("40", 300_602), ("40_ring", 530_133), ("25", 530_133)];
pub const AC130_GUN_SLOT: &[(&str, i64)] = &[("105", 2), ("40", 3), ("25", 4)];
fn ac130_scale_default(gid: &str) -> f64 {
    if gid == "25" { 0.7 } else { 1.0 }
}
fn ac130_two_tier(gid: &str) -> bool {
    gid != "25"
}
fn ac130_tier_slot(tier: &str) -> i64 {
    match tier {
        "core" => 1,
        "ring" => 2,
        _ => 3,
    }
}

pub const AC130_FLAT_ATK: &[(&str, V)] = &[
    ("isAddBaseAtk", i(1)),
    ("atkPhysCorrection", i(0)),
    ("atkMagCorrection", i(0)),
    ("atkFireCorrection", i(0)),
    ("atkThunCorrection", i(0)),
    ("atkDarkCorrection", i(0)),
    ("atkStamCorrection", i(0)),
    ("guardAtkRateCorrection", i(0)),
    ("guardBreakCorrection", i(0)),
    ("isDisableBothHandsAtkBonus", i(1)),
    ("spEffectAtkPowerCorrectRate_byPoint", i(0)),
    ("disableGuard", i(1)),
];

const DMG_LV: &[&str] = &[
    "dmgLv_None", "dmgLv_S", "dmgLv_M", "dmgLv_L", "dmgLv_BlowM", "dmgLv_Push", "dmgLv_Strike", "dmgLv_BlowS", "dmgLv_Min",
    "dmgLv_Uppercut", "dmgLv_BlowLL", "dmgLv_Breath",
];

fn speffect_overrides() -> Ov {
    let mut o = ov(&[("toughnessDamageCutRate", f(0.0)), ("saReceiveDamageRate", f(0.0))]);
    for k in DMG_LV {
        put(&mut o, k, i(1));
    }
    for (k, v) in [("noDead", i(1)), ("fallDamageRate", f(0.0)), ("spCategory", i(0)), ("isDisableNetSync", i(1))] {
        put(&mut o, k, v);
    }
    o
}

pub const M6_RANGE: (i64, i64) = (79_100_000, 79_199_999);
pub const M6_KIND_BANDS: i64 = 0;
pub const M6_KIND_LAUNCHER: i64 = 5;
pub const M6_KIND_EXPLOSION: i64 = 6;
fn m6_tier_idx(tier: &str) -> i64 {
    ac130_tier_slot(tier)
}
pub const M6_GUN_FLAT: &[(&str, V)] = &[
    ("isAddBaseAtk", i(1)),
    ("atkPhysCorrection", i(0)),
    ("atkMagCorrection", i(0)),
    ("atkFireCorrection", i(0)),
    ("atkThunCorrection", i(0)),
    ("atkDarkCorrection", i(0)),
    ("atkStamCorrection", i(0)),
    ("guardAtkRateCorrection", i(0)),
    ("guardBreakCorrection", i(0)),
    ("isDisableBothHandsAtkBonus", i(1)),
    ("spEffectAtkPowerCorrectRate_byPoint", i(0)),
    ("disableGuard", i(0)),
    ("atkAttribute", i(3)),
    ("selfTarget", i(0)),
    ("friendlyTarget", i(0)),
    ("opposeTarget", i(1)),
];
fn m6_expl_obj(wname: &str) -> (i64, i64) {
    if wname == "m79_mp" { (60, 0) } else { (100, 1) }
}
fn m6_expl_sfx(tier: &str) -> i64 {
    if tier == "ring" { 530_133 } else { 300_602 }
}
/// Review M6 fidelity: the body-hit damage of the guns whose weapon file has locTorsoUpper > 1 (and two controls).
pub const M6_BODY_KNOWN: &[(&str, i64)] =
    &[("cheytac_mp", 105), ("barrett_mp", 105), ("wa2000_mp", 105), ("m21_mp", 77), ("ump45_mp", 40), ("m4_mp", 30)];

const NUKE_TARGETS: &[&str] = &[
    "effectTargetSelf", "effectTargetFriend", "effectTargetEnemy", "effectTargetPlayer", "effectTargetAI", "effectTargetLive",
    "effectTargetGhost", "effectTargetOpposeTarget", "effectTargetFriendlyTarget",
];
fn nuke_speffect() -> Ov {
    let mut o = ov(&[("effectEndurance", f(0.0)), ("changeHpRate", f(100.0)), ("changeHpPoint", i(0)), ("maxHpRate", f(1.0))]);
    for k in NUKE_TARGETS {
        put(&mut o, k, i(1));
    }
    for (k, v) in [
        ("noDead", i(0)),
        ("dontDeleteOnDead", i(0)),
        ("isHpBurnEffect", i(0)),
        ("isDestinedDeathHpMult", i(0)),
        ("vfxId", i(-1)),
        ("stateInfo", i(0)),
        ("iconId", i(-1)),
        ("isDisableNetSync", i(1)),
    ] {
        put(&mut o, k, v);
    }
    o
}

pub const C3_TWIN_BASE: i64 = 79_800_000;
pub const C3_TWIN_RANGE: (i64, i64) = (79_800_000, 79_899_999);
pub const C3_NUKE_MARKER_BULLET: i64 = 79_000_963;
pub const C3_SP_MODE: i64 = 79_001_090;
pub const C3_SP_GUN_BASE: i64 = 79_001_100;
pub const C3_GUN_G: &[i64] =
    &[1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 21, 22, 23, 24, 25, 31, 32, 33, 34, 41, 42, 43, 44, 45, 46, 51, 52, 53, 54, 55];
pub const C3_SP_AC130: i64 = 79_001_150;
pub const C3_SP_NUKE: i64 = 79_001_190;
pub const C3_SP_RANGE: (i64, i64) = (79_001_090, 79_001_190);
pub const C3_SP_CLONE: i64 = 0;
pub const C3_SP_MODE_VFX: i64 = 8060;
pub const C3_MARKER_ENDURANCE_S: f64 = 30.0;
const C3_SP_TARGETS: &[&str] = &[
    "effectTargetSelf", "effectTargetFriend", "effectTargetEnemy", "effectTargetPlayer", "effectTargetAI", "effectTargetLive",
    "effectTargetGhost", "effectTargetSelfTarget",
];

// ---- inputs ----------------------------------------------------------------------------------------------------------

/// The build inputs, as build_regulation.load_inputs reads them.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub cfg: toml::Table,
    /// converted\weapons.json
    pub weapons: serde_json::Value,
    /// converted\ac130\weapons_ac130.json
    pub ac130: serde_json::Value,
    /// (gun name, stats.json) in `[arsenal] guns` order
    pub arsenal: Vec<(String, serde_json::Value)>,
}

fn read_json(p: &Path, missing: &str) -> Result<serde_json::Value, String> {
    if !p.is_file() {
        return Err(format!("{} missing{missing}", p.display()));
    }
    let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(s.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", p.display()))
}

/// load_inputs + load_arsenal (`root` = the MW2inER folder: mw2er.toml, converted\).
pub fn load_inputs(root: &Path) -> Result<Inputs, String> {
    let cfg_p = root.join("mw2er.toml");
    if !cfg_p.is_file() {
        return Err(format!("{} missing", cfg_p.display()));
    }
    let weapons = read_json(&root.join(r"converted\weapons.json"), " (run convert_weapons.py first)")?;
    let ac130 = read_json(&root.join(r"converted\ac130\weapons_ac130.json"), " (run convert_ac130.py first)")?;
    let text = std::fs::read_to_string(&cfg_p).map_err(|e| format!("{}: {e}", cfg_p.display()))?;
    let cfg: toml::Table = toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("mw2er.toml: {e}"))?;
    let guns_dir = root.join(r"converted\guns");
    let idx = read_json(&guns_dir.join("index.json"), " (run convert_arsenal.py first)")?;
    let idx_guns = idx.get("guns").and_then(|g| g.as_array()).cloned().unwrap_or_default();
    let mut names: Vec<String> = match cfg.get("arsenal").and_then(|a| a.get("guns")) {
        Some(toml::Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or("arsenal.guns: not a string")).collect::<Result<_, _>>()?,
        Some(_) => return Err("arsenal.guns is not a list".into()),
        None => Vec::new(),
    };
    if names.is_empty() {
        names = idx_guns.iter().filter_map(|g| g.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect();
    }
    let mut arsenal = Vec::new();
    for n in names {
        let g = idx_guns.iter().rev().find(|g| g.get("name").and_then(|x| x.as_str()) == Some(&n));
        if !g.is_some_and(|g| truthy(g.get("ok"))) {
            return Err(format!("arsenal gun {n} is not converted (index.json ok=false or missing)"));
        }
        let st = read_json(&guns_dir.join(&n).join("stats.json"), "")?;
        arsenal.retain(|(m, _): &(String, serde_json::Value)| *m != n);
        arsenal.push((n, st));
    }
    Ok(Inputs { cfg, weapons, ac130, arsenal })
}

fn truthy(v: Option<&serde_json::Value>) -> bool {
    match v {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(serde_json::Value::String(s)) => !s.is_empty(),
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        Some(serde_json::Value::Object(o)) => !o.is_empty(),
    }
}

// ---- Python conversions ----------------------------------------------------------------------------------------------

fn sub<'a>(t: &'a toml::Table, k: &str) -> Result<Option<&'a toml::Table>, String> {
    match t.get(k) {
        None => Ok(None),
        Some(toml::Value::Table(x)) => Ok(Some(x)),
        Some(_) => Err(format!("mw2er.toml: {k} is not a table")),
    }
}

/// Python `float(t.get(k, default))`.
fn tf(t: Option<&toml::Table>, k: &str, default: f64, ctx: &str) -> Result<f64, String> {
    match t.and_then(|t| t.get(k)) {
        None => Ok(default),
        Some(toml::Value::Float(x)) => Ok(*x),
        Some(toml::Value::Integer(x)) => Ok(*x as f64),
        Some(toml::Value::Boolean(b)) => Ok(*b as i64 as f64),
        Some(toml::Value::String(s)) => s.trim().parse().map_err(|_| format!("{ctx}.{k}={s:?}: not a number")),
        Some(v) => Err(format!("{ctx}.{k}={v}: not a number")),
    }
}

/// Python `int(t.get(k, default))`.
fn ti(t: Option<&toml::Table>, k: &str, default: i64, ctx: &str) -> Result<i64, String> {
    match t.and_then(|t| t.get(k)) {
        None => Ok(default),
        Some(v) => tv_int(v).ok_or_else(|| format!("{ctx}.{k}={v}: not an integer")),
    }
}

fn tv_int(v: &toml::Value) -> Option<i64> {
    match v {
        toml::Value::Integer(x) => Some(*x),
        toml::Value::Float(x) if x.is_finite() => Some(x.trunc() as i64),
        toml::Value::Boolean(b) => Some(*b as i64),
        toml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn ts(t: Option<&toml::Table>, k: &str, default: &str) -> String {
    match t.and_then(|t| t.get(k)) {
        None => default.to_string(),
        Some(toml::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

/// Python `float(v)` of a JSON value.
fn jf(v: Option<&serde_json::Value>, what: &str) -> Result<f64, String> {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64().ok_or_else(|| format!("{what}: bad number")),
        Some(serde_json::Value::Bool(b)) => Ok(*b as i64 as f64),
        Some(serde_json::Value::String(s)) => s.trim().parse().map_err(|_| format!("{what}={s:?}: not a number")),
        _ => Err(format!("{what} missing or not a number")),
    }
}

/// Python `int(v)` of a JSON value.
fn ji(v: Option<&serde_json::Value>, what: &str) -> Result<i64, String> {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|x| x.trunc() as i64)).ok_or_else(|| format!("{what}: bad number")),
        Some(serde_json::Value::Bool(b)) => Ok(*b as i64),
        Some(serde_json::Value::String(s)) => s.trim().parse().map_err(|_| format!("{what}={s:?}: not an integer")),
        _ => Err(format!("{what} missing or not an integer")),
    }
}

fn jstr(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(x) => x.to_string(),
        None => String::new(),
    }
}

// ---- math (mirrors of build_regulation.py; the Python is the spec) ---------------------------------------------------

/// band_damages: every integer from hi down to lo when at most 16 values, else 16 evenly spaced rounded values.
pub fn band_damages(damage: i64, min_damage: i64) -> Vec<i64> {
    let (hi, lo) = (damage.max(min_damage), damage.min(min_damage));
    if hi - lo < MAX_BANDS {
        // Python: hi - lo + 1 <= MAX_BANDS
        return (lo..=hi).rev().collect();
    }
    let mut out: Vec<i64> = Vec::new();
    for k in 0..MAX_BANDS {
        // Python: hi - (hi - lo) * i / (MAX_BANDS - 1): the int product, true division, then int - float
        let v = rhaz(hi as f64 - ((hi - lo) * k) as f64 / (MAX_BANDS - 1) as f64);
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    out
}

/// MW2 radius damage, assumed linear [I].
pub fn dmg_at(inner: f64, outer: f64, r: f64, d: f64) -> f64 {
    if r <= 0.0 {
        return inner;
    }
    let x = 1.0 - d / r;
    let fr = if x > 0.0 { x } else { 0.0 };
    let fr = if fr < 1.0 { fr } else { 1.0 };
    outer + (inner - outer) * fr
}

/// The community ER attack-vs-defence curve (`x ** 2` as `x * x`, the same correctly rounded square).
pub fn er_damage(atk: f64, defence: f64) -> f64 {
    if atk <= 0.0 || defence <= 0.0 {
        return atk.max(0.0);
    }
    let r = atk / defence;
    let m = if r > 8.0 {
        0.9
    } else if r > 2.5 {
        0.9 - 0.8 / 121.0 * ((r - 8.0) * (r - 8.0))
    } else if r > 1.0 {
        0.7 - 0.4 / 3.0 * ((r - 2.5) * (r - 2.5))
    } else if r > 0.125 {
        0.1 + 19.2 / 49.0 * ((r - 0.125) * (r - 0.125))
    } else {
        0.1
    };
    atk * m
}

/// atkPhys whose er_damage on defence d is `target` (200-step bisection).
pub fn inv_er(target: f64, d: f64) -> f64 {
    if !(target > 0.0 && target.is_finite()) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0f64, 65_535.0f64);
    if er_damage(hi, d) < target {
        return hi;
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if er_damage(mid, d) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

pub fn planned_atk(mw2: f64, s: f64, d_ref: f64) -> i64 {
    rhaz(inv_er(s * mw2, d_ref))
}

/// body_mult: locTorsoUpper when 1 < v < 10 (not launchers), else 1.
pub fn body_mult(st: &serde_json::Value) -> Result<f64, String> {
    let raw = st.get("num").and_then(|n| n.get("locTorsoUpper"));
    let v = if truthy(raw) { jf(raw, "locTorsoUpper")? } else { 0.0 };
    let class = jstr(st.get("class"));
    Ok(if class != "launcher" && 1.0 < v && v < 10.0 { v } else { 1.0 })
}

fn body_damage(d: i64, mult: f64) -> i64 {
    if mult == 1.0 { d } else { rhaz(d as f64 * mult) }
}

fn m6_rid(id_base: i64, g: i64, kind: i64, idx: i64) -> i64 {
    id_base + g * 1000 + kind * 100 + idx
}

/// (tier, radius_m, mw2, atk) in the Python dict order: single, then core, ring.
type Tiers = Vec<(&'static str, f64, f64, i64)>;

fn ac130_tiers(gun: &serde_json::Value, k: f64, two_tier: bool) -> Result<Tiers, String> {
    let gid = jstr(gun.get("id"));
    let r = jf(gun.get("explosion_radius_in"), "explosion_radius_in")?;
    let inner = jf(gun.get("explosion_inner"), "explosion_inner")?;
    let outer = jf(gun.get("explosion_outer"), "explosion_outer")?;
    let r_m = r * INCH;
    let single = dmg_at(inner, outer, r, r / 2.0);
    let mut out: Tiers = vec![("single", r_m, single, rhaz(dmg_at(inner, outer, r, r / 2.0) * k))];
    if two_tier {
        let (q1, q3) = (dmg_at(inner, outer, r, r / 4.0), dmg_at(inner, outer, r, 3.0 * r / 4.0));
        out.push(("core", r_m / 2.0, q1 - q3, rhaz((q1 - q3) * k)));
        out.push(("ring", r_m, q3, rhaz(q3 * k)));
    }
    for (tier, _, _, atk) in &out {
        if !(0 < *atk && *atk <= 65535) {
            return Err(format!("ac130 {gid} {tier}: atkPhys {atk} out of u16 range"));
        }
    }
    Ok(out)
}

fn launcher_tiers(st: &serde_json::Value, s: f64, d_ref: f64, two_tier: bool) -> Result<Tiers, String> {
    let n = st.get("num");
    let g = |k: &str| jf(n.and_then(|n| n.get(k)), k);
    let (r, inner, outer) = (g("explosionRadius")?, g("explosionInnerDamage")?, g("explosionOuterDamage")?);
    let r_m = r * INCH;
    let single = dmg_at(inner, outer, r, r / 2.0);
    let mut out: Tiers = vec![("single", r_m, single, planned_atk(single, s, d_ref))];
    if two_tier {
        let (q1, q3) = (dmg_at(inner, outer, r, r / 4.0), dmg_at(inner, outer, r, 3.0 * r / 4.0));
        let ring_atk = planned_atk(q3, s, d_ref);
        let core_target = s * q1 - er_damage(ring_atk as f64, d_ref);
        let core_atk = rhaz(inv_er(core_target, d_ref));
        out.push(("core", r_m / 2.0, q1 - q3, core_atk));
        out.push(("ring", r_m, q3, ring_atk));
    }
    for (tier, _, _, atk) in &out {
        if !(0 < *atk && *atk <= 65535) {
            return Err(format!("{} {tier}: atkPhys {atk} out of u16 range", jstr(st.get("name"))));
        }
    }
    Ok(out)
}

/// Mirror of mw2er_core::coop_vis::twin_of (None = no twin).
pub fn twin_of(rid: i64) -> Option<i64> {
    if M6_RANGE.0 <= rid && rid <= M6_RANGE.1 {
        let (g, r) = ((rid - M6_RANGE.0).div_euclid(1000), (rid - M6_RANGE.0).rem_euclid(1000));
        let (kind, idx) = (r / 100, r % 100);
        if !C3_GUN_G.contains(&g) {
            return None;
        }
        return match kind {
            M6_KIND_BANDS => Some(C3_TWIN_BASE + g * 1000),
            M6_KIND_LAUNCHER if idx == 0 => Some(C3_TWIN_BASE + g * 1000 + 500),
            M6_KIND_EXPLOSION if (1..=3).contains(&idx) => Some(C3_TWIN_BASE + g * 1000 + 600 + idx),
            _ => None,
        };
    }
    if AC130_RANGE.0 <= rid && rid <= AC130_RANGE.1 {
        let (slot, tier) = ((rid - 79_000_000) / 100, (rid - 79_000_000) % 100);
        if (2..=4).contains(&slot) && (1..=3).contains(&tier) {
            return Some(C3_TWIN_BASE + slot * 100 + tier);
        }
    }
    None
}

// ---- the plan --------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Plan {
    pub k: f64,
    pub id_base: i64,
    pub marker_id: i64,
    pub speed: f64,
    pub life: f64,
    pub dist: i64,
    /// plan["clone"]: Bullet / AtkParam_Pc / SpEffectParam
    pub clone_bullet: i64,
    pub clone_atk: i64,
    pub clone_speffect: i64,
    pub bullets: Vec<PlanRow>,
    pub atks: Vec<PlanRow>,
    pub speffects: Vec<PlanRow>,
    /// rows.json blocks, key order and int / float as the Python dicts
    pub weapons: Json,
    pub ac130: Json,
    pub arsenal: Json,
    pub nuke: Json,
    pub coop: Json,
    // typed facts the verify needs
    pub arsenal_id_base: i64,
    pub arsenal_bullet_sfx: String,
    pub arsenal_s: f64,
    pub arsenal_d_ref: f64,
    pub protect_speffect_id: i64,
    pub protect_endurance_s: f64,
    pub lethal_speffect_id: i64,
    pub lethal_bullet_id: i64,
    pub horizon_bullet_id: i64,
    pub twins: Vec<(i64, i64)>,
}

impl Plan {
    pub fn rows(&self, stem: &str) -> &[PlanRow] {
        match stem {
            "Bullet" => &self.bullets,
            "AtkParam_Pc" => &self.atks,
            "SpEffectParam" => &self.speffects,
            _ => &[],
        }
    }

    pub fn rows_mut(&mut self, stem: &str) -> &mut Vec<PlanRow> {
        match stem {
            "Bullet" => &mut self.bullets,
            "AtkParam_Pc" => &mut self.atks,
            _ => &mut self.speffects,
        }
    }
}

pub const EDITED: [&str; 3] = ["Bullet", "AtkParam_Pc", "SpEffectParam"];

fn bands_json(v: Vec<Json>) -> Json {
    Json::Arr(v)
}

struct ArsenalOut {
    bullets: Vec<PlanRow>,
    atks: Vec<PlanRow>,
    block: Json,
    id_base: i64,
    bullet_sfx: String,
    s: f64,
    d_ref: f64,
}

#[allow(clippy::too_many_arguments)]
fn make_arsenal_plan(inp: &Inputs, marker: i64, speed: f64, life: f64, dist: i64, clone_bullet: i64, clone_atk: i64) -> Result<ArsenalOut, String> {
    let reg = sub(&inp.cfg, "regulation")?.map(|r| sub(r, "arsenal")).transpose()?.flatten();
    let dm = sub(&inp.cfg, "damage")?;
    let mode = ts(dm, "mode", "flat_inv");
    if mode != "flat_inv" {
        return Err(format!("damage.mode='{mode}': only flat_inv builds the arsenal rows (linear is the legacy A/B)"));
    }
    let s = tf(dm, "s", 1.225, "damage")?;
    let d_ref = tf(dm, "d_ref", 85.0, "damage")?;
    if !((0.05..=20.0).contains(&s) && (1.0..=2000.0).contains(&d_ref)) {
        return Err(format!("damage.s={} / d_ref={} out of range", crate::pyfmt::float_repr(s), crate::pyfmt::float_repr(d_ref)));
    }
    let id_base = ti(reg, "id_base", 79_100_000, "regulation.arsenal")?;
    let sfx_mode = ts(reg, "bullet_sfx", "none");
    if sfx_mode != "none" && sfx_mode != "dagger" {
        return Err(format!("regulation.arsenal.bullet_sfx='{sfx_mode}'"));
    }
    let clone_expl = ti(reg, "clone_explosion_from", 10_030_001, "regulation.arsenal")?;
    let tiers2 = ti(sub(&inp.cfg, "launchers")?, "explosion_tiers", 2, "launchers")? == 2;
    let sfx_off: Ov = if sfx_mode == "none" { ov(&[("sfxId_Hit", i(-1)), ("sfxId_Flick", i(-1))]) } else { Ov::new() };
    let (mut bullets, mut atks) = (Vec::new(), Vec::new());
    let mut guns = Json::obj();
    let mut seen_g = Vec::new();
    for (wname, st) in &inp.arsenal {
        let g = ji(st.get("g"), &format!("{wname} g"))?;
        if seen_g.contains(&g) {
            return Err(format!("arsenal: duplicate g {g} ({wname})"));
        }
        seen_g.push(g);
        let n = st.get("num");
        let mut gun_bullet = ov(&[
            ("sfxId_Bullet", i(-1)),
            ("life", f(life)),
            ("dist", i(dist)),
            ("gravityInRange", i(0)),
            ("gravityOutRange", i(0)),
            ("initVellocity", f(speed)),
            ("maxVellocity", f(speed)),
            ("minVellocity", f(speed)),
            ("accelInRange", i(0)),
            ("accelOutRange", i(0)),
            ("homingAngle", i(0)),
            ("lockShootLimitAng", i(0)),
            ("HitBulletID", i(marker)),
        ]);
        for (k, v) in &sfx_off {
            put(&mut gun_bullet, k, *v);
        }
        let class = jstr(st.get("class"));
        let mut entry = Json::obj().with("g", g).with("class", class.as_str());
        let mut bands = Vec::new();
        if class != "launcher" {
            let bm = body_mult(st)?;
            let dmg = ji(n.and_then(|n| n.get("damage")), &format!("{wname} damage"))?;
            let min = ji(n.and_then(|n| n.get("minDamage")), &format!("{wname} minDamage"))?;
            for (b, d) in band_damages(body_damage(dmg, bm), body_damage(min, bm)).into_iter().enumerate() {
                let b = b as i64;
                let rid = m6_rid(id_base, g, M6_KIND_BANDS, b);
                let atk = planned_atk(d as f64, s, d_ref);
                if !(0 < atk && atk <= 65535) {
                    return Err(format!("{wname} band {b}: atkPhys {atk} out of u16 range"));
                }
                bullets.push(PlanRow { id: rid, name: format!("MW2 {wname} band {b} dmg {d}"), ov: merged(&[("atkId_Bullet", i(rid))], &gun_bullet), clone: clone_bullet });
                atks.push(PlanRow { id: rid, name: format!("MW2 {wname} dmg {d}"), ov: merged(&[("atkPhys", i(atk))], M6_GUN_FLAT), clone: clone_atk });
                bands.push(Json::obj().with("band", b).with("id", rid).with("mw2", d).with("atk", atk));
            }
            entry.set("bands", bands_json(bands));
            if bm != 1.0 {
                entry.set("body_mult", bm);
            }
        } else {
            entry.set("bands", bands_json(bands));
            let imp = m6_rid(id_base, g, M6_KIND_LAUNCHER, 0);
            let mk = m6_rid(id_base, g, M6_KIND_LAUNCHER, 1);
            let impact = ji(n.and_then(|n| n.get("damage")), &format!("{wname} damage"))?;
            let iatk = planned_atk(impact as f64, s, d_ref);
            bullets.push(PlanRow { id: imp, name: format!("MW2 {wname} impact dmg {impact}"), ov: merged(&[("atkId_Bullet", i(imp))], &gun_bullet), clone: clone_bullet });
            atks.push(PlanRow { id: imp, name: format!("MW2 {wname} impact dmg {impact}"), ov: merged(&[("atkPhys", i(iatk))], M6_GUN_FLAT), clone: clone_atk });
            bullets.push(PlanRow {
                id: mk,
                name: format!("MW2 {wname} marker"),
                ov: ov(&[
                    ("atkId_Bullet", i(-1)),
                    ("sfxId_Bullet", i(-1)),
                    ("sfxId_Hit", i(-1)),
                    ("sfxId_Flick", i(-1)),
                    ("life", f(0.1)),
                    ("dist", i(0)),
                    ("initVellocity", i(0)),
                    ("maxVellocity", i(0)),
                    ("minVellocity", i(0)),
                    ("gravityInRange", i(0)),
                    ("gravityOutRange", i(0)),
                    ("accelInRange", i(0)),
                    ("accelOutRange", i(0)),
                    ("homingAngle", i(0)),
                    ("lockShootLimitAng", i(0)),
                    ("HitBulletID", i(-1)),
                    ("isAttackSFX", i(0)),
                ]),
                clone: clone_bullet,
            });
            let (obj, bush) = m6_expl_obj(wname);
            let mut lt = Json::obj()
                .with("impact", Json::obj().with("band", 0).with("id", imp).with("mw2", impact).with("atk", iatk))
                .with("marker_id", mk);
            for (tier, radius_m, mw2, atk) in launcher_tiers(st, s, d_ref, tiers2)? {
                let rid = m6_rid(id_base, g, M6_KIND_EXPLOSION, m6_tier_idx(tier));
                bullets.push(PlanRow {
                    id: rid,
                    name: format!("MW2 {wname} explosion {tier}"),
                    ov: ov(&[
                        ("atkId_Bullet", i(rid)),
                        ("hitRadius", f(round6(radius_m))),
                        ("life", f(if tier == "ring" { 0.07 } else { 0.1 })),
                        ("sfxId_Bullet", i(m6_expl_sfx(tier))),
                        ("sfxPostureType", i(AC130_SFX_POSTURE)),
                        ("HitBulletID", i(-1)),
                    ]),
                    clone: clone_expl,
                });
                atks.push(PlanRow {
                    id: rid,
                    name: format!("MW2 {wname} explosion {tier} dmg {}", fmt_g(mw2)),
                    ov: merged(
                        &[
                            ("atkPhys", i(atk)),
                            ("atkFire", i(0)),
                            ("atkObj", i(obj)),
                            ("isEnableCalcDamageForBushesObj", i(bush)),
                            ("selfTarget", i(0)),
                            ("friendlyTarget", i(0)),
                            ("opposeTarget", i(1)),
                        ],
                        AC130_FLAT_ATK,
                    ),
                    clone: clone_expl,
                });
                lt.set(tier, Json::obj().with("id", rid).with("radius_m", round6(radius_m)).with("mw2", mw2).with("atk", atk));
            }
            entry.set("launcher", lt);
        }
        guns.set(wname, entry);
    }
    for r in &bullets {
        if !(M6_RANGE.0 <= r.id && r.id <= M6_RANGE.1) {
            return Err(format!("arsenal row {} outside ({}, {})", r.id, M6_RANGE.0, M6_RANGE.1));
        }
    }
    let block = Json::obj()
        .with("mode", mode.as_str())
        .with("s", s)
        .with("d_ref", d_ref)
        .with("id_base", id_base)
        .with("bullet_sfx", sfx_mode.as_str())
        .with("clone", Json::obj().with("bullet", clone_bullet).with("atk", clone_atk).with("explosion", clone_expl))
        .with("guns", guns);
    Ok(ArsenalOut { bullets, atks, block, id_base, bullet_sfx: sfx_mode, s, d_ref })
}

/// (twin + marker Bullet rows, marker SpEffect rows, the rows.json coop block, (twin, source) ids)
type CoopOut = (Vec<PlanRow>, Vec<PlanRow>, Json, Vec<(i64, i64)>);

fn make_coop_plan(bullets: &[PlanRow], ac_clone_b: i64) -> Result<CoopOut, String> {
    let mut sorted: Vec<&PlanRow> = bullets.iter().collect();
    sorted.sort_by_key(|r| r.id);
    let mut twins: Vec<PlanRow> = Vec::new();
    let mut seen: Vec<(i64, i64)> = Vec::new();
    for r in sorted {
        let Some(tid) = twin_of(r.id) else { continue };
        if seen.iter().any(|(t, _)| *t == tid) {
            continue;
        }
        if r.get("atkId_Bullet").unwrap_or(V::I(-1)).same(V::I(-1)) {
            continue; // an inert row (a marker) needs no twin
        }
        seen.push((tid, r.id));
        let mut tov = r.ov.clone();
        put(&mut tov, "atkId_Bullet", i(-1));
        put(&mut tov, "HitBulletID", i(-1));
        let short = r.name.strip_prefix("MW2 ").unwrap_or(&r.name);
        let name: String = format!("MW2 co-op twin of {} ({short})", r.id).chars().take(120).collect();
        twins.push(PlanRow { id: tid, name, ov: tov, clone: r.clone });
    }
    for t in &twins {
        if !(C3_TWIN_RANGE.0 <= t.id && t.id <= C3_TWIN_RANGE.1) {
            return Err(format!("twin {} outside ({}, {})", t.id, C3_TWIN_RANGE.0, C3_TWIN_RANGE.1));
        }
    }
    let marker_b = PlanRow {
        id: C3_NUKE_MARKER_BULLET,
        name: "MW2 co-op nuke marker (synced, visual-only)".into(),
        ov: ov(&[
            ("atkId_Bullet", i(-1)),
            ("hitRadius", f(0.1)),
            ("life", f(0.5)),
            ("sfxId_Bullet", i(-1)),
            ("sfxId_Hit", i(-1)),
            ("sfxId_Flick", i(-1)),
            ("HitBulletID", i(-1)),
        ]),
        clone: ac_clone_b,
    };
    let mut sp = Vec::new();
    let mut marker = |sid: i64, name: String, vfx: i64| {
        let mut o = ov(&[("effectEndurance", f(C3_MARKER_ENDURANCE_S)), ("isDisableNetSync", i(0)), ("vfxId", i(vfx)), ("iconId", i(-1))]);
        for k in C3_SP_TARGETS {
            put(&mut o, k, i(1));
        }
        sp.push(PlanRow { id: sid, name, ov: o, clone: C3_SP_CLONE });
    };
    marker(C3_SP_MODE, "MW2 co-op marker: MW2 mode (hides the ER weapon)".into(), C3_SP_MODE_VFX);
    for (k, g) in C3_GUN_G.iter().enumerate() {
        marker(C3_SP_GUN_BASE + k as i64, format!("MW2 co-op marker: gun g={g}"), -1);
    }
    marker(C3_SP_AC130, "MW2 co-op marker: AC-130 ride".into(), -1);
    marker(C3_SP_NUKE, "MW2 co-op marker: Tactical Nuke".into(), -1);
    for r in &sp {
        if !(C3_SP_RANGE.0 <= r.id && r.id <= C3_SP_RANGE.1) {
            return Err(format!("marker SpEffect {} outside ({}, {})", r.id, C3_SP_RANGE.0, C3_SP_RANGE.1));
        }
    }
    let block = Json::obj()
        .with("twins", Json::Arr(seen.iter().map(|(t, s)| Json::obj().with("id", *t).with("src", *s)).collect()))
        .with("nuke_marker_bullet", C3_NUKE_MARKER_BULLET)
        .with("sp_mode", C3_SP_MODE)
        .with("sp_gun_base", C3_SP_GUN_BASE)
        .with("sp_gun_count", C3_GUN_G.len())
        .with("sp_ac130", C3_SP_AC130)
        .with("sp_nuke", C3_SP_NUKE)
        .with("sp_mode_vfx", C3_SP_MODE_VFX)
        .with("marker_endurance_s", C3_MARKER_ENDURANCE_S)
        .with("sp_clone", C3_SP_CLONE);
    twins.push(marker_b);
    Ok((twins, sp, block, seen))
}

/// make_plan: every row the build adds, sorted by id per param, and the rows.json blocks.
pub fn make_plan(inp: &Inputs) -> Result<Plan, String> {
    let reg = sub(&inp.cfg, "regulation")?;
    let k = tf(sub(&inp.cfg, "damage")?, "k", 4.0, "damage")?;
    if !(0.0 < k && k <= 100.0) {
        return Err(format!("damage.k={} out of range", crate::pyfmt::float_repr(k)));
    }
    let ctx = "regulation";
    let id_base = ti(reg, "id_base", 79_000_000, ctx)?;
    let marker = ti(reg, "marker_id", 79_000_900, ctx)?;
    let speed = tf(reg, "bullet_speed_mps", 999.0, ctx)?;
    let life = tf(reg, "bullet_life_s", 0.15, ctx)?;
    let marker_life = tf(reg, "marker_life_s", 0.1, ctx)?;
    let clone_bullet = ti(reg, "clone_bullet_from", 10_170_000, ctx)?;
    let clone_atk = ti(reg, "clone_atk_from", 10_170_000, ctx)?;
    let mut slots: Vec<(String, toml::Value)> = match reg.and_then(|r| r.get("weapons")) {
        None => vec![("ump45_mp".into(), toml::Value::Integer(1))],
        Some(toml::Value::Table(t)) => t.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Some(_) => return Err("regulation.weapons is not a table".into()),
    };
    if slots.is_empty() {
        return Err("regulation.weapons is empty".into());
    }
    let dist = (speed * life - 1e-9).ceil() as i64; // in-range parameters apply over the whole flight (150 m)
    let slot_key = |v: &toml::Value| match v {
        toml::Value::Integer(x) => *x as f64,
        toml::Value::Float(x) => *x,
        _ => f64::NAN,
    };
    slots.sort_by(|a, b| slot_key(&a.1).total_cmp(&slot_key(&b.1)));
    let (mut bullets, mut atks) = (Vec::new(), Vec::new());
    let mut rows_weapons = Json::obj();
    for (wname, slot_v) in &slots {
        let slot = tv_int(slot_v).ok_or_else(|| format!("regulation.weapons.{wname}={slot_v}: not an integer"))?;
        if !(1..=8).contains(&slot) {
            return Err(format!("regulation.weapons.{wname}={slot_v}: slot must be 1..8 (9 is the marker)"));
        }
        let w = inp.weapons.get("weapons").and_then(|w| w.get(wname)).ok_or_else(|| format!("{wname} not in converted/weapons.json"))?;
        let mw2 = w.get("mw2");
        let bands = band_damages(ji(mw2.and_then(|m| m.get("damage")), "damage")?, ji(mw2.and_then(|m| m.get("minDamage")), "minDamage")?);
        let mut lst = Vec::new();
        for (b, d) in bands.into_iter().enumerate() {
            let b = b as i64;
            let rid = id_base + slot * 100 + b;
            let atk_phys = rhaz(d as f64 * k);
            if !(0 < atk_phys && atk_phys <= 65535) {
                return Err(format!("{wname} band {b}: atkPhys {atk_phys} out of u16 range"));
            }
            bullets.push(PlanRow {
                id: rid,
                name: format!("MW2 {wname} band {b} dmg {d}"),
                ov: ov(&[
                    ("atkId_Bullet", i(rid)),
                    ("sfxId_Bullet", i(-1)),
                    ("life", f(life)),
                    ("dist", i(dist)),
                    ("gravityInRange", i(0)),
                    ("gravityOutRange", i(0)),
                    ("initVellocity", f(speed)),
                    ("maxVellocity", f(speed)),
                    ("minVellocity", f(speed)),
                    ("accelInRange", i(0)),
                    ("accelOutRange", i(0)),
                    ("homingAngle", i(0)),
                    ("lockShootLimitAng", i(0)),
                    ("HitBulletID", i(marker)),
                ]),
                clone: clone_bullet,
            });
            atks.push(PlanRow { id: rid, name: format!("MW2 {wname} dmg {d}"), ov: ov(&[("atkPhys", i(atk_phys))]), clone: clone_atk });
            lst.push(Json::obj().with("band", b).with("id", rid).with("mw2_damage", d).with("atk_phys", atk_phys));
        }
        rows_weapons.set(wname, Json::obj().with("slot", slot).with("bands", Json::Arr(lst)));
    }
    bullets.push(PlanRow {
        id: marker,
        name: "MW2 impact marker".into(),
        ov: ov(&[
            ("atkId_Bullet", i(-1)),
            ("sfxId_Bullet", i(-1)),
            ("sfxId_Hit", i(-1)),
            ("sfxId_Flick", i(-1)),
            ("life", f(marker_life)),
            ("dist", i(0)),
            ("initVellocity", i(0)),
            ("maxVellocity", i(0)),
            ("minVellocity", i(0)),
            ("gravityInRange", i(0)),
            ("gravityOutRange", i(0)),
            ("accelInRange", i(0)),
            ("accelOutRange", i(0)),
            ("homingAngle", i(0)),
            ("lockShootLimitAng", i(0)),
            ("HitBulletID", i(-1)),
            ("isAttackSFX", i(0)),
        ]),
        clone: clone_bullet,
    });
    // ---- M4: the AC-130 explosion rows + the protection SpEffect ----
    let ra = reg.map(|r| sub(r, "ac130")).transpose()?.flatten();
    let actx = "regulation.ac130";
    let k_ac = tf(ra, "k", 0.65, actx)?;
    if !(0.0 < k_ac && k_ac <= 100.0) {
        return Err(format!("regulation.ac130.k={} out of range", crate::pyfmt::float_repr(k_ac)));
    }
    let ac_clone_b = ti(ra, "clone_bullet_from", 10_030_001, actx)?;
    let ac_clone_a = ti(ra, "clone_atk_from", 10_030_001, actx)?;
    let sp_clone = ti(ra, "clone_speffect_from", 1540, actx)?;
    let protect = ti(ra, "protect_speffect_id", 79_000_950, actx)?;
    let mut sfx: Vec<(&str, i64)> = Vec::new();
    for (key, v) in AC130_SFX_DEFAULT {
        sfx.push((key, ti(ra, &format!("sfx_{key}"), *v, actx)?));
    }
    for (key, v) in &sfx {
        if *v != -1 && !(0 < *v && *v < 100_000_000) {
            return Err(format!("regulation.ac130.sfx_{key}={v}: -1 or an SFX id"));
        }
    }
    let sfx_of = |key: &str| sfx.iter().find(|(k, _)| *k == key).unwrap().1;
    let endurance = tf(ra, "protect_endurance_s", PROTECT_ENDURANCE_DEFAULT_S, actx)?;
    if !(PROTECT_ENDURANCE_RANGE_S.0 <= endurance && endurance <= PROTECT_ENDURANCE_RANGE_S.1) {
        return Err(format!("regulation.ac130.protect_endurance_s={} outside (30.0, 600.0)", crate::pyfmt::float_repr(endurance)));
    }
    let ac_guns: Vec<&serde_json::Value> = inp.ac130.get("guns").and_then(|g| g.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    let gun_of = |gid: &str| ac_guns.iter().rev().find(|g| jstr(g.get("id")) == gid).copied();
    let mut ids: Vec<String> = ac_guns.iter().map(|g| jstr(g.get("id"))).collect();
    ids.sort();
    ids.dedup();
    let mut want: Vec<&str> = AC130_GUN_SLOT.iter().map(|g| g.0).collect();
    want.sort();
    if ids != want {
        return Err(format!("weapons_ac130.json guns {ids:?} (want 105/40/25)"));
    }
    let mut scale = Vec::new();
    for (gid, _) in AC130_GUN_SLOT {
        let v = tf(ra, &format!("scale_{gid}"), ac130_scale_default(gid), actx)?;
        if !(0.05..=20.0).contains(&v) {
            return Err(format!("regulation.ac130.scale_{gid}={} outside 0.05..20", crate::pyfmt::float_repr(v)));
        }
        scale.push((*gid, v));
    }
    let mut ac_rows = Json::obj();
    let mut ac_ids = Vec::new();
    for ((gid, slot), (_, sc)) in AC130_GUN_SLOT.iter().zip(&scale) {
        let tiers = ac130_tiers(gun_of(gid).unwrap(), k_ac * sc, ac130_two_tier(gid))?;
        let mut out = Json::obj();
        for tier in ["core", "ring", "single"] {
            let Some(&(_, radius_m, mw2, atk)) = tiers.iter().find(|t| t.0 == tier) else { continue };
            let rid = 79_000_000 + slot * 100 + if *gid == "25" { 1 } else { ac130_tier_slot(tier) };
            let row_life = if tier == "ring" || *gid == "25" { 0.07 } else { 0.1 };
            let row_sfx = if tier == "ring" { sfx_of(&format!("{gid}_ring")) } else { sfx_of(gid) };
            bullets.push(PlanRow {
                id: rid,
                name: format!("MW2 AC-130 {gid}mm {tier}"),
                ov: ov(&[
                    ("atkId_Bullet", i(rid)),
                    ("hitRadius", f(round6(radius_m))),
                    ("life", f(row_life)),
                    ("sfxId_Bullet", i(row_sfx)),
                    ("sfxPostureType", i(AC130_SFX_POSTURE)),
                    ("HitBulletID", i(-1)),
                ]),
                clone: ac_clone_b,
            });
            let (st, sa, lvl, kb) = ac130_tuning(gid, tier);
            let (obj, bushes, _beats) = ac130_object(gid, tier);
            atks.push(PlanRow {
                id: rid,
                name: format!("MW2 AC-130 {gid}mm {tier} dmg {}", fmt_g(mw2)),
                ov: merged(
                    &[
                        ("atkPhys", i(atk)),
                        ("atkFire", i(0)),
                        ("atkStam", i(st)),
                        ("atkSuperArmor", f(sa)),
                        ("dmgLevel", i(lvl)),
                        ("knockbackDist", f(kb)),
                        ("atkObj", i(obj)),
                        ("isEnableCalcDamageForBushesObj", i(bushes)),
                    ],
                    AC130_FLAT_ATK,
                ),
                clone: ac_clone_a,
            });
            ac_ids.push(rid);
            out.set(
                tier,
                Json::obj()
                    .with("id", rid)
                    .with("radius_m", round6(radius_m))
                    .with("mw2", mw2)
                    .with("atk_phys", atk)
                    .with("sfx", row_sfx)
                    .with("atk_obj", obj)
                    .with("obj_bushes", bushes)
                    .with("sfx_posture", AC130_SFX_POSTURE),
            );
        }
        ac_rows.set(gid, out);
    }
    let mut prot = speffect_overrides();
    put(&mut prot, "effectEndurance", f(endurance));
    let mut speffects = vec![PlanRow { id: protect, name: "MW2 AC-130 gunship protection".into(), ov: prot, clone: sp_clone }];
    // ---- M6: the arsenal rows ----
    let ars = make_arsenal_plan(inp, marker, speed, life, dist, clone_bullet, clone_atk)?;
    bullets.extend(ars.bullets.iter().cloned());
    atks.extend(ars.atks.iter().cloned());
    // ---- M7: the nuke rows ----
    let rn = reg.map(|r| sub(r, "nuke")).transpose()?.flatten();
    let nctx = "regulation.nuke";
    let lethal_sp = ti(rn, "lethal_speffect_id", 79_000_960, nctx)?;
    let nuke_sp_clone = ti(rn, "clone_speffect_from", 17_039, nctx)?;
    let lethal_b = ti(rn, "lethal_bullet_id", 79_000_961, nctx)?;
    let horizon_b = ti(rn, "horizon_bullet_id", 79_000_962, nctx)?;
    let horizon_sfx = ti(rn, "horizon_sfx", 524_028, nctx)?;
    if horizon_sfx != -1 && !(0 < horizon_sfx && horizon_sfx < 100_000_000) {
        return Err(format!("regulation.nuke.horizon_sfx={horizon_sfx}: -1 or an SFX id"));
    }
    for rid in [lethal_sp, lethal_b, horizon_b] {
        if !(79_000_000..=79_099_999).contains(&rid) || (AC130_RANGE.0 <= rid && rid <= AC130_RANGE.1) {
            return Err(format!("regulation.nuke row id {rid} outside 79000000..79099999 or inside the AC-130 range"));
        }
    }
    speffects.push(PlanRow { id: lethal_sp, name: "MW2 Tactical Nuke (one-shot 100% max HP)".into(), ov: nuke_speffect(), clone: nuke_sp_clone });
    bullets.push(PlanRow {
        id: lethal_b,
        name: "MW2 Tactical Nuke lethal bullet".into(),
        ov: ov(&[
            ("atkId_Bullet", i(lethal_b)),
            ("hitRadius", f(0.5)),
            ("life", f(0.1)),
            ("sfxId_Bullet", i(-1)),
            ("sfxId_Hit", i(-1)),
            ("sfxId_Flick", i(-1)),
            ("HitBulletID", i(-1)),
        ]),
        clone: ac_clone_b,
    });
    atks.push(PlanRow {
        id: lethal_b,
        name: "MW2 Tactical Nuke lethal".into(),
        ov: merged(
            AC130_FLAT_ATK,
            &[
                ("atkPhys", i(9999)),
                ("atkFire", i(0)),
                ("atkObj", i(0)),
                ("spEffectId0", i(lethal_sp)),
                ("selfTarget", i(0)),
                ("friendlyTarget", i(0)),
                ("opposeTarget", i(1)),
            ],
        ),
        clone: ac_clone_a,
    });
    bullets.push(PlanRow {
        id: horizon_b,
        name: "MW2 Tactical Nuke horizon explosion (visual)".into(),
        ov: ov(&[
            ("atkId_Bullet", i(-1)),
            ("hitRadius", f(0.1)),
            ("life", f(0.1)),
            ("sfxId_Bullet", i(horizon_sfx)),
            ("sfxId_Hit", i(-1)),
            ("sfxId_Flick", i(-1)),
            ("sfxPostureType", i(AC130_SFX_POSTURE)),
            ("HitBulletID", i(-1)),
        ]),
        clone: ac_clone_b,
    });
    // ---- C3 PASS C: the co-op twins, the nuke marker bullet and the synced marker SpEffects ----
    let (c3_b, c3_s, coop, twins) = make_coop_plan(&bullets, ac_clone_b)?;
    bullets.extend(c3_b);
    speffects.extend(c3_s);
    speffects.sort_by_key(|r| r.id);
    bullets.sort_by_key(|r| r.id);
    atks.sort_by_key(|r| r.id);
    let mut bid: Vec<i64> = bullets.iter().map(|r| r.id).collect();
    bid.dedup();
    let mut sid: Vec<i64> = speffects.iter().map(|r| r.id).collect();
    sid.dedup();
    if bid.len() != bullets.len() || sid.len() != speffects.len() {
        return Err("duplicate row ids in the plan".into());
    }
    for rid in bullets.iter().chain(&speffects).map(|r| r.id) {
        if !(ID_RANGE.0 <= rid && rid <= ID_RANGE.1) {
            return Err(format!("row id {rid} outside the reserved range ({}, {})", ID_RANGE.0, ID_RANGE.1));
        }
    }
    for rid in &ac_ids {
        if !(AC130_RANGE.0 <= *rid && *rid <= AC130_RANGE.1) {
            return Err(format!("ac130 row {rid} outside ({}, {})", AC130_RANGE.0, AC130_RANGE.1));
        }
    }
    let mut gun_scale = Json::obj();
    for (gid, v) in &scale {
        gun_scale.set(gid, *v);
    }
    let ac130 = Json::obj()
        .with("k", k_ac)
        .with("gun_scale", gun_scale)
        .with("atk_mode", "flat")
        .with("protect_speffect_id", protect)
        .with("protect_endurance_s", endurance)
        .with("clone", Json::obj().with("bullet", ac_clone_b).with("atk", ac_clone_a).with("speffect", sp_clone))
        .with("guns", ac_rows);
    let nuke = Json::obj()
        .with("lethal_speffect_id", lethal_sp)
        .with("lethal_bullet_id", lethal_b)
        .with("horizon_bullet_id", horizon_b)
        .with("horizon_sfx", horizon_sfx)
        .with("clone", Json::obj().with("speffect", nuke_sp_clone).with("bullet", ac_clone_b).with("atk", ac_clone_a));
    Ok(Plan {
        k,
        id_base,
        marker_id: marker,
        speed,
        life,
        dist,
        clone_bullet,
        clone_atk,
        clone_speffect: sp_clone,
        bullets,
        atks,
        speffects,
        weapons: rows_weapons,
        ac130,
        arsenal: ars.block,
        nuke,
        coop,
        arsenal_id_base: ars.id_base,
        arsenal_bullet_sfx: ars.bullet_sfx,
        arsenal_s: ars.s,
        arsenal_d_ref: ars.d_ref,
        protect_speffect_id: protect,
        protect_endurance_s: endurance,
        lethal_speffect_id: lethal_sp,
        lethal_bullet_id: lethal_b,
        horizon_bullet_id: horizon_b,
        twins,
    })
}

/// arsenal_known_answers: the first band's MW2 damage of the M6_BODY_KNOWN guns present (and its inverted atk).
pub fn arsenal_known_answers(plan: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    let Some(guns) = plan.arsenal.get("guns") else { return out };
    for (name, want) in M6_BODY_KNOWN {
        let Some(g) = guns.get(name) else { continue };
        let b0 = match g.get("bands") {
            Some(Json::Arr(a)) => a.first(),
            _ => None,
        };
        let ok = b0.is_some_and(|b| {
            b.get("mw2") == Some(&Json::Int(*want)) && b.get("atk") == Some(&Json::Int(planned_atk(*want as f64, plan.arsenal_s, plan.arsenal_d_ref)))
        });
        if !ok {
            out.push(format!("arsenal known answer: {name} first band {:?} (want mw2 {want}, body multiplier locTorsoUpper)", b0));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn math_matches_the_python_known_answers() {
        assert_eq!(band_damages(40, 35), vec![40, 39, 38, 37, 36, 35]);
        assert_eq!(band_damages(35, 40), vec![40, 39, 38, 37, 36, 35]);
        assert_eq!(band_damages(70, 70), vec![70]);
        let wide = band_damages(100, 20);
        assert_eq!((wide.len(), wide[0], *wide.last().unwrap()), (16, 100, 20));
        assert_eq!(dmg_at(1200.0, 300.0, 600.0, 300.0), 750.0);
        assert_eq!(dmg_at(1200.0, 300.0, 0.0, 5.0), 1200.0);
        assert_eq!(dmg_at(1200.0, 300.0, 600.0, 900.0), 300.0);
        assert_eq!(er_damage(-5.0, 100.0), 0.0);
        assert_eq!(er_damage(1000.0, 100.0), 900.0);
        let a = inv_er(49.0, 85.0);
        assert!((er_damage(a, 85.0) - 49.0).abs() < 1e-9);
        assert_eq!(inv_er(0.0, 85.0), 0.0);
        assert_eq!(inv_er(1e9, 85.0), 65535.0);
        // build\rows.json: ump45_mp close 40 -> atk 49 * ... (flat_inv s 1.225, d_ref 85)
        assert_eq!(planned_atk(40.0, 1.225, 85.0), planned_atk(40.0, 1.225, 85.0));
    }

    #[test]
    fn math_agrees_with_the_dll_mirrors() {
        use mw2er_core::{ac130_dmg, coop_vis, falloff, rows};
        for (d, m) in [(40, 35), (30, 20), (100, 20), (70, 70), (1000, 50), (105, 105), (160, 60)] {
            let a = band_damages(d, m);
            let b: Vec<i64> = falloff::band_damages(d as i32, m as i32).into_iter().map(i64::from).collect();
            assert_eq!(a, b, "{d}/{m}");
        }
        for atk in [0.0, 1.0, 10.0, 50.0, 85.0, 100.0, 300.0, 1000.0, 9999.0] {
            assert_eq!(er_damage(atk, 85.0), ac130_dmg::er_damage(atk, 85.0), "{atk}");
        }
        for t in [1.0, 10.0, 49.0, 123.4, 700.0] {
            assert_eq!(inv_er(t, 85.0), rows::inv_er(t, 85.0));
            assert_eq!(planned_atk(t, 1.225, 85.0), rows::planned_atk(t, 1.225, 85.0) as i64);
        }
        for rid in [79_101_000i64, 79_101_005, 79_153_500, 79_153_501, 79_153_601, 79_153_604, 79_110_000, 79_000_201, 79_000_401, 79_000_900, 79_160_000] {
            assert_eq!(twin_of(rid), coop_vis::twin_of(rid as i32).map(i64::from), "{rid}");
        }
        assert_eq!(C3_GUN_G.len(), coop_vis::GUN_G.len());
    }

    #[test]
    fn dict_merge_keeps_python_order() {
        let o = merged(&[("a", i(1)), ("b", i(2))], &[("a", i(5)), ("c", i(3))]);
        assert_eq!(o, vec![("a", i(5)), ("b", i(2)), ("c", i(3))]);
    }

    pub(crate) fn inputs(toml_text: &str) -> Inputs {
        let st = |g: i64, class: &str, num: serde_json::Value| serde_json::json!({"g": g, "class": class, "name": "x", "num": num});
        Inputs {
            cfg: toml::from_str(toml_text).unwrap(),
            weapons: serde_json::json!({"weapons": {"ump45_mp": {"mw2": {"damage": 40, "minDamage": 35}}}}),
            ac130: serde_json::json!({"guns": [
                {"id": "105", "explosion_radius_in": 600.0, "explosion_inner": 1200.0, "explosion_outer": 300.0},
                {"id": "40", "explosion_radius_in": 155.0, "explosion_inner": 220.0, "explosion_outer": 50.0},
                {"id": "25", "explosion_radius_in": 32.0, "explosion_inner": 45.0, "explosion_outer": 25.0}]}),
            arsenal: vec![
                ("ump45_mp".into(), st(11, "smg", serde_json::json!({"damage": 40, "minDamage": 35}))),
                ("cheytac_mp".into(), st(21, "sniper", serde_json::json!({"damage": 70, "minDamage": 70, "locTorsoUpper": 1.5}))),
                ("rpg_mp".into(), st(53, "launcher", serde_json::json!({"damage": 1000, "minDamage": 0, "explosionRadius": 400,
                    "explosionInnerDamage": 160, "explosionOuterDamage": 60}))),
            ],
        }
    }

    #[test]
    fn plan_from_small_inputs() {
        let p = make_plan(&inputs("[damage]\nk = 4.0\n")).unwrap();
        // M2 UMP45 bands + marker
        let b0 = p.bullets.iter().find(|r| r.id == 79_000_100).unwrap();
        assert_eq!(b0.name, "MW2 ump45_mp band 0 dmg 40");
        assert_eq!(b0.get("dist"), Some(V::I(150)));
        assert_eq!(p.atks.iter().find(|r| r.id == 79_000_105).unwrap().get("atkPhys"), Some(V::I(140)));
        // sorted, unique, all in range
        for rows in [&p.bullets, &p.atks, &p.speffects] {
            assert!(rows.windows(2).all(|w| w[0].id < w[1].id));
            assert!(rows.iter().all(|r| (ID_RANGE.0..=ID_RANGE.1).contains(&r.id)));
        }
        // AC-130 105 core / ring / single, 25 mm single only
        for rid in [79_000_201, 79_000_202, 79_000_203, 79_000_301, 79_000_302, 79_000_303, 79_000_401] {
            assert!(p.atks.iter().any(|r| r.id == rid), "{rid}");
        }
        assert!(!p.atks.iter().any(|r| r.id == 79_000_402));
        assert_eq!(p.bullets.iter().find(|r| r.id == 79_000_202).unwrap().get("hitRadius"), Some(V::F(15.24)));
        assert_eq!(p.bullets.iter().find(|r| r.id == 79_000_201).unwrap().get("hitRadius"), Some(V::F(7.62)));
        // M6: sniper body multiplier, launcher tiers in Python dict order
        assert!(arsenal_known_answers(&p).is_empty());
        let rpg = p.arsenal.get("guns").unwrap().get("rpg_mp").unwrap().get("launcher").unwrap();
        let keys: Vec<&str> = match rpg {
            Json::Obj(m) => m.iter().map(|(k, _)| k.as_str()).collect(),
            _ => vec![],
        };
        assert_eq!(keys, ["impact", "marker_id", "single", "core", "ring"]);
        // C3: twins never attack
        assert!(!p.twins.is_empty());
        for (tid, src) in &p.twins {
            assert_eq!(twin_of(*src), Some(*tid));
            let t = p.bullets.iter().find(|r| r.id == *tid).unwrap();
            assert_eq!((t.get("atkId_Bullet"), t.get("HitBulletID")), (Some(V::I(-1)), Some(V::I(-1))));
        }
        assert_eq!(p.speffects.len(), 2 + 3 + C3_GUN_G.len());
        assert_eq!(p.atks.iter().find(|r| r.id == 79_000_961).unwrap().get("spEffectId0"), Some(V::I(79_000_960)));
    }

    #[test]
    fn plan_errors_match_the_python() {
        let e = |t: &str| make_plan(&inputs(t)).unwrap_err();
        assert!(e("[damage]\nk = 0\n").contains("damage.k=0.0 out of range"));
        assert!(e("[damage]\nmode = \"linear\"\n").contains("only flat_inv"));
        assert!(e("[regulation.weapons]\n").contains("regulation.weapons is empty"));
        assert!(e("[regulation.weapons]\nump45_mp = 9\n").contains("slot must be 1..8"));
        assert!(e("[regulation.weapons]\nm4_mp = 1\n").contains("m4_mp not in converted/weapons.json"));
        assert!(e("[regulation.ac130]\nprotect_endurance_s = 5\n").contains("outside (30.0, 600.0)"));
        assert!(e("[regulation.ac130]\nsfx_40 = 0\n").contains("sfx_40=0"));
        assert!(e("[regulation.nuke]\nlethal_bullet_id = 79000201\n").contains("inside the AC-130 range"));
        assert!(e("[regulation.arsenal]\nbullet_sfx = \"loud\"\n").contains("bullet_sfx"));
        let mut i2 = inputs("");
        i2.arsenal[1].1["g"] = serde_json::json!(11);
        assert!(make_plan(&i2).unwrap_err().contains("duplicate g 11"));
        // the old sniper bug: no body multiplier -> known answer fails
        let mut i3 = inputs("");
        i3.arsenal[1].1["num"]["locTorsoUpper"] = serde_json::json!(1.0);
        assert!(!arsenal_known_answers(&make_plan(&i3).unwrap()).is_empty());
        // one tier: no core / ring rows
        let p = make_plan(&inputs("[launchers]\nexplosion_tiers = 1\n")).unwrap();
        assert!(!p.atks.iter().any(|r| r.id == 79_153_601) && p.atks.iter().any(|r| r.id == 79_153_603));
    }
}
