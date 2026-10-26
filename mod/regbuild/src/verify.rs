//! The round-trip verification of a built regulation (port of build_regulation.verify, minus the WitchyBND XML
//! read-back): run on the output re-read from its own encrypted bytes.
//!
//! (a) every untouched param byte-identical to vanilla; the 3 edited params differ ONLY by exactly the planned rows,
//! sorted (param_diff.check_added); (b) every added row == its clone's bytes outside the override fields, each override
//! field == the planned value, the planned name, plus the SAFETY / SAFETY_M6 / SAFETY_M7 / SAFETY_C3 / object-break /
//! SFX / endurance assertions; (c) the BND4 manifest (header, names, ids, flags, hash table) unchanged.

use std::collections::{HashMap, HashSet};

use crate::bnd4::Bnd4;
use crate::fields::{self, ParamLayout, V};
use crate::param::Param;
use crate::plan::{self, AC130_FLAT_ATK, AC130_RANGE, C3_SP_MODE, C3_SP_MODE_VFX, C3_SP_RANGE, C3_TWIN_RANGE, EDITED, M6_GUN_FLAT, M6_RANGE, Plan, twin_of};

pub const PARAM_COUNT: usize = 194;
pub const ASSET_PARAM: &str = "AssetEnvironmentGeometryParam";

const fn i(v: i64) -> V {
    V::I(v)
}
const fn f(v: f64) -> V {
    V::F(v)
}

const SAFETY_BULLET: &[(&str, V)] = &[
    ("isHitBothTeam", i(0)),
    ("isUseSharedHitList", i(0)),
    ("isPenetrateChr", i(1)),
    ("isPenetrateObj", i(1)),
    ("isPenetrateMap", i(1)),
    ("HitBulletID", i(-1)),
    ("initVellocity", f(0.0)),
    ("dist", f(0.0)),
];
const SAFETY_ATK: &[(&str, V)] =
    &[("selfTarget", i(0)), ("friendlyTarget", i(0)), ("opposeTarget", i(1)), ("atkFire", i(0)), ("mapHitType", i(2)), ("isDisableNoDamage", i(0))];
const SAFETY_SP: &[(&str, V)] = &[
    ("noDead", i(1)),
    ("slashDamageCutRate", f(0.0)),
    ("blowDamageCutRate", f(0.0)),
    ("thrustDamageCutRate", f(0.0)),
    ("neutralDamageCutRate", f(0.0)),
    ("magicDamageCutRate", f(0.0)),
    ("fireDamageCutRate", f(0.0)),
    ("thunderDamageCutRate", f(0.0)),
    ("darkDamageCutRate", f(0.0)),
];
const SAFETY_M6_GUN_BULLET: &[(&str, V)] = &[("isHitBothTeam", i(0)), ("isUseSharedHitList", i(0)), ("gravityInRange", f(0.0)), ("homingAngle", f(0.0))];
const SAFETY_M6_EXPL_BULLET: &[(&str, V)] =
    &[("isHitBothTeam", i(0)), ("isUseSharedHitList", i(0)), ("HitBulletID", i(-1)), ("initVellocity", f(0.0)), ("dist", f(0.0))];
const SAFETY_M6_EXPL_ATK: &[(&str, V)] = &[("selfTarget", i(0)), ("friendlyTarget", i(0)), ("opposeTarget", i(1))];
const SAFETY_M6_MARKER_BULLET: &[(&str, V)] = &[("atkId_Bullet", i(-1)), ("sfxId_Hit", i(-1)), ("sfxId_Flick", i(-1)), ("HitBulletID", i(-1))];
const SAFETY_M7_SPEFFECT: &[(&str, V)] = &[
    ("effectEndurance", f(0.0)),
    ("changeHpRate", f(100.0)),
    ("noDead", i(0)),
    ("dontDeleteOnDead", i(0)),
    ("isHpBurnEffect", i(0)),
    ("isDestinedDeathHpMult", i(0)),
    ("vfxId", i(-1)),
    ("stateInfo", i(0)),
    ("effectTargetSelfTarget", i(1)),
];
const SAFETY_M7_LETHAL_BULLET: &[(&str, V)] =
    &[("isHitBothTeam", i(0)), ("sfxId_Bullet", i(-1)), ("sfxId_Hit", i(-1)), ("sfxId_Flick", i(-1)), ("HitBulletID", i(-1))];
const SAFETY_M7_LETHAL_ATK: &[(&str, V)] =
    &[("selfTarget", i(0)), ("friendlyTarget", i(0)), ("opposeTarget", i(1)), ("disableGuard", i(1)), ("isAddBaseAtk", i(1)), ("atkPhys", i(9999))];
const SAFETY_M7_HORIZON_BULLET: &[(&str, V)] = &[("atkId_Bullet", i(-1)), ("isHitBothTeam", i(0)), ("HitBulletID", i(-1))];
const SAFETY_C3_TWIN: &[(&str, V)] = &[("atkId_Bullet", i(-1)), ("HitBulletID", i(-1)), ("isHitBothTeam", i(0))];
const SAFETY_C3_MARKER_BULLET: &[(&str, V)] =
    &[("atkId_Bullet", i(-1)), ("HitBulletID", i(-1)), ("sfxId_Bullet", i(-1)), ("sfxId_Hit", i(-1)), ("sfxId_Flick", i(-1))];
const SAFETY_C3_MARKER_SP: &[(&str, V)] = &[
    ("isDisableNetSync", i(0)),
    ("effectEndurance", f(plan::C3_MARKER_ENDURANCE_S)),
    ("changeHpRate", f(0.0)),
    ("changeHpPoint", i(0)),
    ("slashDamageCutRate", f(1.0)),
    ("magicDamageCutRate", f(1.0)),
    ("noDead", i(0)),
    ("iconId", i(-1)),
    ("stateInfo", i(0)),
];

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub untouched_identical: usize,
    /// (stem, rows vanilla, rows built, added)
    pub edited: Vec<(String, usize, usize, usize)>,
    pub rows_checked: usize,
}

/// param_diff.check_added: B == A plus exactly `ids`, sorted, vanilla rows unchanged and in order.
pub fn check_added(a: &Param, b: &Param, ids: &HashSet<i32>) -> Vec<String> {
    let mut problems = Vec::new();
    let (ha, hb) = (a.header_key(), b.header_key());
    if ha != hb {
        problems.push(format!("header differs: {ha:?} -> {hb:?}"));
    }
    let idb: Vec<i32> = b.rows.iter().map(|r| r.id).collect();
    if let Some(k) = (0..idb.len().saturating_sub(1)).find(|&k| idb[k] > idb[k + 1]) {
        problems.push(format!("rows in B are not sorted by id (at index {k}: {} > {})", idb[k], idb[k + 1]));
    }
    let present_a: HashSet<i32> = a.rows.iter().map(|r| r.id).collect();
    let mut clash: Vec<i32> = present_a.intersection(ids).copied().collect();
    clash.sort();
    if !clash.is_empty() {
        problems.push(format!("expected-added ids already exist in A: {:?}", &clash[..clash.len().min(10)]));
    }
    let mut count_b: HashMap<i32, usize> = HashMap::new();
    for id in &idb {
        *count_b.entry(*id).or_default() += 1;
    }
    let mut missing: Vec<i32> = ids.iter().filter(|id| count_b.get(id).copied().unwrap_or(0) == 0).copied().collect();
    let mut dup: Vec<i32> = ids.iter().filter(|id| count_b.get(id).copied().unwrap_or(0) > 1).copied().collect();
    missing.sort();
    dup.sort();
    if !missing.is_empty() {
        problems.push(format!("expected rows missing in B: {missing:?}"));
    }
    if !dup.is_empty() {
        problems.push(format!("expected rows duplicated in B: {dup:?}"));
    }
    let rest_b: Vec<_> = b.rows.iter().filter(|r| !ids.contains(&r.id)).collect();
    if rest_b.len() != a.rows.len() {
        let rest_ids: HashSet<i32> = rest_b.iter().map(|r| r.id).collect();
        let mut extra: Vec<i32> = rest_ids.difference(&present_a).copied().collect();
        let mut gone: Vec<i32> = present_a.difference(&rest_ids).copied().collect();
        extra.sort();
        gone.sort();
        problems.push(format!(
            "unexpected row count change: A {}, B minus added {}; unexpected added {:?}, removed {:?}",
            a.rows.len(),
            rest_b.len(),
            &extra[..extra.len().min(10)],
            &gone[..gone.len().min(10)]
        ));
    } else {
        let mut changed = Vec::new();
        for (ra, rb) in a.rows.iter().zip(&rest_b) {
            if ra.id != rb.id {
                problems.push(format!("vanilla row order changed at id {} (B has {})", ra.id, rb.id));
                break;
            }
            if ra.data != rb.data || ra.name.as_deref().unwrap_or("") != rb.name.as_deref().unwrap_or("") {
                changed.push(ra.id);
            }
        }
        if !changed.is_empty() {
            problems.push(format!("vanilla rows changed: {} {:?}", changed.len(), &changed[..changed.len().min(10)]));
        }
    }
    problems
}

/// The field names (or byte offsets) where `a` and `b` differ outside `mask`.
fn diff_fields(layout: &ParamLayout, a: &[u8], b: &[u8], mask: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in 0..a.len() {
        if (a[k] ^ b[k]) & !mask[k] != 0 {
            let names = layout.fields_at(k);
            let s = if names.is_empty() { format!("+0x{k:03x}") } else { names.join("/") };
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

fn check_table(problems: &mut Vec<String>, tag: &str, stem: &str, rid: i32, layout: &ParamLayout, row: &[u8], table: &[(&str, V)]) {
    for (k, w) in table {
        match layout.field(k) {
            None => problems.push(format!("{stem}[{rid}]: {tag} field {k} not in the regbuild field table")),
            Some(fd) => {
                let got = fields::read(row, fd);
                if !got.same(*w) {
                    problems.push(format!("{stem}[{rid}]: {tag} {k}={got} (must be {w})"));
                }
            }
        }
    }
}

fn val(layout: &ParamLayout, row: &[u8], k: &str) -> Option<V> {
    layout.field(k).map(|fd| fields::read(row, fd))
}

/// AC-130 (gun id, tier) of an explosion row id (79000000 + slot*100 + tier; the 25 mm's single row is tier 1).
pub fn ac130_gun_tier(rid: i64) -> Option<(&'static str, &'static str)> {
    let (slot, t) = ((rid - 79_000_000) / 100, (rid - 79_000_000) % 100);
    let gid = plan::AC130_GUN_SLOT.iter().find(|g| g.1 == slot)?.0;
    if gid == "25" {
        return Some((gid, "single"));
    }
    Some((gid, ["", "core", "ring", "single"].get(t as usize).copied().filter(|s| !s.is_empty())?))
}

/// The per-row assertions of build_regulation.verify (b) beyond clone + overrides.
fn safety(problems: &mut Vec<String>, stem: &str, rid: i32, layout: &ParamLayout, got: &[u8], plan: &Plan) {
    let r = rid as i64;
    let v = |k: &str| val(layout, got, k);
    // M6 (SAFETY_M6)
    if (stem == "Bullet" || stem == "AtkParam_Pc") && (M6_RANGE.0..=M6_RANGE.1).contains(&r) {
        let rest = (r - plan.arsenal_id_base).rem_euclid(1000);
        let (kind, idx) = (rest / 100, rest % 100);
        let kind = if kind == plan::M6_KIND_EXPLOSION {
            "expl"
        } else if kind == plan::M6_KIND_LAUNCHER && idx == 1 {
            "marker"
        } else {
            "gun"
        };
        let table: &[(&str, V)] = match (kind, stem) {
            ("gun", "Bullet") => SAFETY_M6_GUN_BULLET,
            ("gun", _) => M6_GUN_FLAT,
            ("expl", "Bullet") => SAFETY_M6_EXPL_BULLET,
            ("expl", _) => SAFETY_M6_EXPL_ATK,
            ("marker", "Bullet") => SAFETY_M6_MARKER_BULLET,
            _ => &[],
        };
        check_table(problems, "SAFETY_M6", stem, rid, layout, got, table);
        if kind == "expl" && stem == "AtkParam_Pc" {
            check_table(problems, "SAFETY_M6", stem, rid, layout, got, AC130_FLAT_ATK);
        }
        if stem == "Bullet" && kind == "gun" {
            if v("HitBulletID") != Some(V::I(plan.marker_id)) {
                problems.push(format!("{stem}[{rid}]: SAFETY_M6 HitBulletID={:?} (the marker)", v("HitBulletID")));
            }
            if plan.arsenal_bullet_sfx == "none" && (v("sfxId_Hit") != Some(V::I(-1)) || v("sfxId_Flick") != Some(V::I(-1))) {
                problems.push(format!("{stem}[{rid}]: SAFETY_M6 sfxId_Hit/Flick {:?}/{:?} (bullet_sfx none: -1, the bug-2 likely fix)", v("sfxId_Hit"), v("sfxId_Flick")));
            }
        }
    }
    // M7 (SAFETY_M7)
    let nk = if stem == "SpEffectParam" && r == plan.lethal_speffect_id {
        Some("speffect")
    } else if r == plan.lethal_bullet_id {
        Some(if stem == "Bullet" { "lethal_bullet" } else { "lethal_atk" })
    } else if stem == "Bullet" && r == plan.horizon_bullet_id {
        Some("horizon_bullet")
    } else {
        None
    };
    if let Some(nk) = nk {
        let table = match nk {
            "speffect" => SAFETY_M7_SPEFFECT,
            "lethal_bullet" => SAFETY_M7_LETHAL_BULLET,
            "lethal_atk" => SAFETY_M7_LETHAL_ATK,
            _ => SAFETY_M7_HORIZON_BULLET,
        };
        check_table(problems, "SAFETY_M7", stem, rid, layout, got, table);
        if nk == "lethal_atk" && v("spEffectId0") != Some(V::I(plan.lethal_speffect_id)) {
            problems.push(format!("{stem}[{rid}]: SAFETY_M7 spEffectId0={:?} (the lethal SpEffect)", v("spEffectId0")));
        }
        if nk == "speffect" && v("isDisableNetSync") != Some(V::I(1)) {
            problems.push(format!("{stem}[{rid}]: SAFETY_C3 isDisableNetSync={:?} (the lethal SpEffect must never sync)", v("isDisableNetSync")));
        }
    }
    // C3 PASS C (SAFETY_C3)
    let ck = if stem == "Bullet" && (C3_TWIN_RANGE.0..=C3_TWIN_RANGE.1).contains(&r) {
        Some("twin")
    } else if stem == "Bullet" && r == plan::C3_NUKE_MARKER_BULLET {
        Some("marker_bullet")
    } else if stem == "SpEffectParam" && (C3_SP_RANGE.0..=C3_SP_RANGE.1).contains(&r) {
        Some("marker_sp")
    } else {
        None
    };
    if let Some(ck) = ck {
        let table = match ck {
            "twin" => SAFETY_C3_TWIN,
            "marker_bullet" => SAFETY_C3_MARKER_BULLET,
            _ => SAFETY_C3_MARKER_SP,
        };
        check_table(problems, "SAFETY_C3", stem, rid, layout, got, table);
        if ck == "marker_sp" {
            let want = if r == C3_SP_MODE { C3_SP_MODE_VFX } else { -1 };
            if v("vfxId") != Some(V::I(want)) {
                problems.push(format!("{stem}[{rid}]: SAFETY_C3 vfxId={:?} (want {want})", v("vfxId")));
            }
        }
        if ck == "twin" {
            let src = plan.twins.iter().find(|t| t.0 == r).map(|t| t.1);
            if src.is_none_or(|s| twin_of(s) != Some(r)) {
                problems.push(format!("{stem}[{rid}]: SAFETY_C3 twin id rule broken (src {src:?})"));
            }
        }
    }
    // M4 (SAFETY, object destruction, SFX posture, finite protection)
    let is_ac130 = if stem == "SpEffectParam" { r == plan.protect_speffect_id } else { (AC130_RANGE.0..=AC130_RANGE.1).contains(&r) };
    if is_ac130 {
        let table: &[(&str, V)] = match stem {
            "Bullet" => SAFETY_BULLET,
            "AtkParam_Pc" => SAFETY_ATK,
            _ => SAFETY_SP,
        };
        check_table(problems, "SAFETY", stem, rid, layout, got, table);
        if stem == "AtkParam_Pc" {
            check_table(problems, "SAFETY", stem, rid, layout, got, AC130_FLAT_ATK);
            match ac130_gun_tier(r) {
                None => problems.push(format!("{stem}[{rid}]: not an AC-130 tier id")),
                Some((gid, tier)) => {
                    let (_obj, bushes, beats) = plan::ac130_object(gid, tier);
                    if !matches!(v("atkObj"), Some(V::I(o)) if o > beats) {
                        problems.push(format!("{stem}[{rid}]: OBJECT atkObj={:?} must exceed asset defence {beats} ({gid}mm {tier})", v("atkObj")));
                    }
                    if v("isEnableCalcDamageForBushesObj") != Some(V::I(bushes)) {
                        problems.push(format!("{stem}[{rid}]: OBJECT isEnableCalcDamageForBushesObj={:?} (want {bushes})", v("isEnableCalcDamageForBushesObj")));
                    }
                }
            }
        }
        if stem == "Bullet" && v("sfxPostureType") != Some(V::I(plan::AC130_SFX_POSTURE)) {
            problems.push(format!("{stem}[{rid}]: SFX sfxPostureType={:?} (want {})", v("sfxPostureType"), plan::AC130_SFX_POSTURE));
        }
        if stem == "SpEffectParam" {
            let e = v("effectEndurance");
            let want = V::F(plan.protect_endurance_s);
            if !e.is_some_and(|e| e.as_f64() > 0.0 && e.same(want)) {
                problems.push(format!("{stem}[{rid}]: SAFETY effectEndurance={e:?} (must be finite {want})"));
            }
        }
    }
}

/// The built files: `built`'s data with `replace` (entry index -> bytes) in place of some entries (self-test).
pub struct Files<'a> {
    pub bnd: &'a Bnd4<'a>,
    pub replace: Vec<(usize, &'a [u8])>,
}

impl<'a> Files<'a> {
    pub fn new(bnd: &'a Bnd4<'a>) -> Self {
        Files { bnd, replace: Vec::new() }
    }

    pub fn get(&self, k: usize) -> &'a [u8] {
        self.replace.iter().find(|r| r.0 == k).map(|r| r.1).unwrap_or_else(|| self.bnd.data(k))
    }
}

/// build_regulation.verify on the parsed output (`built`) against the vanilla BND4.
pub fn verify(vanilla: &Bnd4, built: &Files, plan: &Plan) -> (Vec<String>, Report) {
    let mut problems = Vec::new();
    let mut report = Report::default();
    let names_a: Vec<&str> = vanilla.entries.iter().map(|e| e.file_name()).collect();
    let names_b: Vec<&str> = built.bnd.entries.iter().map(|e| e.file_name()).collect();
    if names_a != names_b || names_a.len() != PARAM_COUNT {
        problems.push(format!("param sets differ: {} vs {} (want {PARAM_COUNT} each)", names_a.len(), names_b.len()));
        return (problems, report);
    }
    for (k, name) in names_a.iter().enumerate() {
        let stem = name.trim_end_matches(".param");
        if EDITED.contains(&stem) {
            continue;
        }
        if vanilla.data(k) != built.get(k) {
            problems.push(format!("{stem}: not byte-identical to vanilla"));
        } else {
            report.untouched_identical += 1;
        }
    }
    for stem in EDITED {
        let k = names_a.iter().position(|n| n.trim_end_matches(".param") == stem).unwrap_or(usize::MAX);
        if k == usize::MAX {
            problems.push(format!("{stem}: missing"));
            continue;
        }
        let (a, b) = match (Param::parse(vanilla.data(k)), Param::parse(built.get(k))) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                problems.push(format!("{stem}: cannot parse ({e})"));
                continue;
            }
        };
        let rows = plan.rows(stem);
        let ids: HashSet<i32> = rows.iter().map(|r| r.id as i32).collect();
        problems.extend(check_added(&a, &b, &ids).into_iter().map(|p| format!("{stem}: {p}")));
        report.edited.push((stem.to_string(), a.rows.len(), b.rows.len(), ids.len()));
        let layout = fields::layout_of(stem).unwrap();
        if a.row_size != layout.row_size || a.param_type != layout.param_type {
            problems.push(format!("{stem}: def {} row size {} != binary {} {}", layout.param_type, layout.row_size, a.param_type, a.row_size));
            continue;
        }
        let mut src: HashMap<i64, &[u8]> = HashMap::new();
        for r in &a.rows {
            src.entry(r.id as i64).or_insert(r.data);
        }
        let mut missing_clone: Vec<i64> = rows.iter().map(|r| r.clone).filter(|c| !src.contains_key(c)).collect();
        missing_clone.sort();
        missing_clone.dedup();
        if !missing_clone.is_empty() {
            problems.push(format!("{stem}: clone source(s) {missing_clone:?} missing in vanilla"));
            continue;
        }
        let mut got_rows: HashMap<i32, (&str, &[u8])> = HashMap::new();
        for r in &b.rows {
            if ids.contains(&r.id) {
                got_rows.entry(r.id).or_insert((r.name.as_deref().unwrap_or(""), r.data));
            }
        }
        for row in rows {
            let rid = row.id as i32;
            let Some((got_name, got)) = got_rows.get(&rid) else {
                problems.push(format!("{stem}[{rid}]: row missing in the built binary"));
                continue;
            };
            let clone = src[&row.clone];
            let mut mask = vec![0u8; layout.row_size];
            let mut bad = Vec::new();
            for (k, w) in &row.ov {
                match layout.field(k) {
                    None => problems.push(format!("{stem}[{rid}]: override field {k} not in the paramdef")),
                    Some(fd) => {
                        fields::mark(&mut mask, fd);
                        let g = fields::read(got, fd);
                        if !g.same(*w) {
                            bad.push(format!("{k}={g} (want {w})"));
                        }
                    }
                }
            }
            for x in diff_fields(layout, clone, got, &mask) {
                bad.push(format!("{x} differs from clone {}", row.clone));
            }
            if !bad.is_empty() {
                bad.truncate(6);
                problems.push(format!("{stem}[{rid}]: {}", bad.join(", ")));
            }
            if *got_name != row.name {
                problems.push(format!("{stem}[{rid}]: binary name {got_name:?} (want {:?})", row.name));
            }
            safety(&mut problems, stem, rid, layout, got, plan);
            report.rows_checked += 1;
        }
    }
    if vanilla.header_without_layout() != built.bnd.header_without_layout() || vanilla.version != built.bnd.version {
        problems.push("BND4 manifest (header / names / ids / flags / hash table) differs from the vanilla one".into());
    }
    (problems, report)
}

/// asset_break_report: (damage-breakable vanilla assets, per AC-130 tier how many its atkObj beats); an error when a
/// tier fails an asset of the defence it is meant to beat.
pub fn asset_break_report(vanilla: &Bnd4) -> Result<(usize, Vec<(String, usize)>), String> {
    let k = vanilla.find(&format!("{ASSET_PARAM}.param")).ok_or(format!("{ASSET_PARAM}.param missing"))?;
    let p = Param::parse(vanilla.data(k))?;
    let l = &fields::ASSET;
    if p.row_size != l.row_size || p.param_type != l.param_type {
        return Err(format!("{ASSET_PARAM}: def row size {} != binary {}", l.row_size, p.row_size));
    }
    let (hp, def, bush) = (l.field("hp").unwrap(), l.field("defense").unwrap(), l.field("isBushesForDamage").unwrap());
    let assets: Vec<(i64, i64)> = p
        .rows
        .iter()
        .filter(|r| fields::read(r.data, hp).as_i64() > 0)
        .map(|r| (fields::read(r.data, def).as_i64(), fields::read(r.data, bush).as_i64()))
        .collect();
    let mut out = Vec::new();
    for (gid, tier) in plan::AC130_OBJECT_TIERS {
        let (obj, bushes, beats) = plan::ac130_object(gid, tier);
        let n = assets.iter().filter(|(d, b)| obj > *d && (bushes != 0 || *b == 0)).count();
        let short = assets.iter().filter(|(d, _)| *d <= beats && obj <= *d).count();
        if short > 0 {
            return Err(format!("ac130 {gid}mm {tier}: atkObj {obj} fails {short} asset(s) with defence <= {beats}"));
        }
        out.push((format!("{gid}/{tier}"), n));
    }
    Ok((assets.len(), out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{ASSET, ATK, BULLET, SPEFFECT};
    use crate::{bnd4, param};

    fn row(layout: &ParamLayout, set: &[(&str, V)]) -> Vec<u8> {
        let mut r = vec![0u8; layout.row_size];
        for (k, v) in set {
            fields::write(&mut r, layout.field(k).unwrap(), *v).unwrap();
        }
        r
    }

    /// A 194-file regulation of tiny params holding only the clone rows the plan needs (with the clone values the
    /// SAFETY tables rely on), the asset param of the object check and 189 one-row fillers.
    fn vanilla() -> Vec<u8> {
        let bullet = param::build_fixture(BULLET.param_type, BULLET.row_size, &[
            (10_030_001, "", row(&BULLET, &[("isPenetrateChr", i(1)), ("isPenetrateObj", i(1)), ("isPenetrateMap", i(1)), ("hitRadius", f(1.5))])),
            (10_170_000, "", row(&BULLET, &[("hitRadius", f(0.02)), ("atkId_Bullet", i(10_170_000))])),
            (90_000_000, "", row(&BULLET, &[])),
        ]);
        let atk = param::build_fixture(ATK.param_type, ATK.row_size, &[
            (10_030_001, "", row(&ATK, &[("opposeTarget", i(1)), ("mapHitType", i(2)), ("atkFire", i(40))])),
            (10_170_000, "", row(&ATK, &[("opposeTarget", i(1)), ("atkPhys", i(30))])),
        ]);
        let sp = param::build_fixture(SPEFFECT.param_type, SPEFFECT.row_size, &[
            (0, "", row(&SPEFFECT, &[("slashDamageCutRate", f(1.0)), ("magicDamageCutRate", f(1.0))])),
            (1540, "", row(&SPEFFECT, &[("noDead", i(1))])),
            (17_039, "", row(&SPEFFECT, &[("effectTargetSelfTarget", i(1)), ("changeHpRate", f(100.0))])),
        ]);
        let asset = param::build_fixture(ASSET.param_type, ASSET.row_size, &[
            (1, "", row(&ASSET, &[("hp", i(100)), ("defense", i(90))])),
            (2, "", row(&ASSET, &[("hp", i(50)), ("defense", i(199)), ("isBushesForDamage", i(1))])),
            (3, "", row(&ASSET, &[("hp", i(0)), ("defense", i(999))])),
        ]);
        let filler = param::build_fixture("FILLER_ST", 4, &[(1, "", vec![1, 2, 3, 4])]);
        let mut names: Vec<(String, &[u8])> = vec![
            ("Bullet.param".into(), &bullet),
            ("AtkParam_Pc.param".into(), &atk),
            ("SpEffectParam.param".into(), &sp),
            (format!("{ASSET_PARAM}.param"), &asset),
        ];
        for k in 0..PARAM_COUNT - 4 {
            names.push((format!("Filler{k:03}.param"), &filler));
        }
        let refs: Vec<(&str, &[u8])> = names.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        bnd4::build_fixture("11730034", &refs)
    }

    #[test]
    fn fixture_build_verifies_and_mutations_are_caught() {
        let raw = vanilla();
        let vb = Bnd4::parse(&raw).unwrap();
        let p = plan::make_plan(&crate::plan::tests::inputs("")).unwrap();
        let (n, tiers) = asset_break_report(&vb).unwrap();
        assert_eq!(n, 2);
        assert_eq!(tiers[0], ("105/core".to_string(), 2));
        let files = crate::apply(&vb, &p).unwrap();
        let bnd = vb.write(&files.iter().map(|f| f.as_slice()).collect::<Vec<_>>()).unwrap();
        let bb = Bnd4::parse(&bnd).unwrap();
        let (problems, report) = verify(&vb, &Files::new(&bb), &p);
        assert!(problems.is_empty(), "{problems:#?}");
        assert_eq!(report.untouched_identical, PARAM_COUNT - 3);
        assert_eq!(report.rows_checked, p.bullets.len() + p.atks.len() + p.speffects.len());
        // a twin that attacks, an AC-130 row that hurts the player, a synced lethal effect, a vanilla byte flipped
        let cases: [(&str, i64, &str, V); 3] = [
            ("Bullet", 79_800_201, "atkId_Bullet", i(79_000_201)),
            ("AtkParam_Pc", 79_000_302, "selfTarget", i(1)),
            ("SpEffectParam", 79_000_960, "isDisableNetSync", i(0)),
        ];
        for (stem, rid, field, v) in cases {
            let k = bb.find(&format!("{stem}.param")).unwrap();
            let m = crate::selftest::mutate_field(bb.data(k), stem, rid, field, v).unwrap();
            let mut fs = Files::new(&bb);
            fs.replace.push((k, &m));
            assert!(!verify(&vb, &fs, &p).0.is_empty(), "{stem}[{rid}].{field}");
        }
        let k = bb.find("Filler007.param").unwrap();
        let mut fs = Files::new(&bb);
        let mut m = bb.data(k).to_vec();
        let off = m.len() - 12;
        m[off] ^= 0xFF;
        fs.replace.push((k, &m));
        assert!(verify(&vb, &fs, &p).0.iter().any(|x| x.contains("Filler007")));
        // the clone missing from vanilla: apply refuses
        let mut p2 = p.clone();
        p2.bullets[0].clone = 12345;
        assert!(crate::apply(&vb, &p2).unwrap_err().contains("clone source 12345 found 0 times"));
        // a vanilla row inside our id range: apply refuses
        let mut p3 = p.clone();
        p3.atks[0].id = 10_170_000;
        p3.atks.sort_by_key(|r| r.id);
        assert!(crate::apply(&vb, &p3).is_err());
    }
}
