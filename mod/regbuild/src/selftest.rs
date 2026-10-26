//! The checks must be able to fail: port of build_regulation.py `--self-test` (every mutation case that does not need
//! WitchyBND's XML; the "manifest missing" case becomes "BND4 manifest changed", the XML read-back case has no
//! binary-route counterpart). Each case mutates the built BND4 / one param / the plan / the inputs in memory and
//! must make `verify` (or the plan) report a problem; the unmutated control must pass.

use crate::bnd4::Bnd4;
use crate::fields::{self, V};
use crate::plan::{self, Inputs, Plan};
use crate::verify::{self, Files};

/// (case label, caught, first problem)
pub type CaseResult = (String, bool, String);

/// Edits a param's bytes given its row table ((entry offset, id) per row).
type RowTableEdit<'f> = dyn Fn(&mut Vec<u8>, &[(usize, i32)]) -> Result<(), String> + 'f;

type Case<'s> = (String, Box<dyn Fn() -> Vec<String> + 's>);

fn row_table(buf: &[u8]) -> Vec<(usize, i32)> {
    let n = u16::from_le_bytes([buf[0xA], buf[0xB]]) as usize;
    (0..n).map(|k| (0x40 + k * 0x18, i32::from_le_bytes(buf[0x40 + k * 0x18..0x44 + k * 0x18].try_into().unwrap()))).collect()
}

fn set_id(buf: &mut [u8], entry: usize, id: i32) {
    buf[entry..entry + 4].copy_from_slice(&id.to_le_bytes());
}

/// mutate_field: one field of row `rid` changed in the param bytes.
pub fn mutate_field(src: &[u8], stem: &str, rid: i64, field: &str, value: V) -> Result<Vec<u8>, String> {
    let mut buf = src.to_vec();
    let layout = fields::layout_of(stem).ok_or("self-test setup: no layout")?;
    let fd = layout.field(field).ok_or_else(|| format!("self-test setup: field {field} not in the table"))?;
    let (entry, _) = row_table(&buf).into_iter().find(|r| r.1 as i64 == rid).ok_or_else(|| format!("self-test setup: row {rid} not in {stem}"))?;
    let doff = i64::from_le_bytes(buf[entry + 8..entry + 16].try_into().unwrap()) as usize;
    fields::write(&mut buf[doff..doff + layout.row_size], fd, value)?;
    Ok(buf)
}

pub struct SelfTest<'a> {
    pub vanilla: &'a Bnd4<'a>,
    pub built: &'a Bnd4<'a>,
    pub plan: &'a Plan,
    pub inputs: &'a Inputs,
}

impl<'a> SelfTest<'a> {
    fn idx(&self, stem: &str) -> usize {
        self.built.find(&format!("{stem}.param")).unwrap()
    }

    fn run_with(&self, stem: &str, bytes: &[u8]) -> Vec<String> {
        let mut f = Files::new(self.built);
        f.replace.push((self.idx(stem), bytes));
        verify::verify(self.vanilla, &f, self.plan).0
    }

    fn table_case(&self, stem: &str, fun: &RowTableEdit<'_>) -> Vec<String> {
        let mut buf = self.built.data(self.idx(stem)).to_vec();
        let rows = row_table(&buf);
        if let Err(e) = fun(&mut buf, &rows) {
            return vec![format!("self-test setup: {e}")];
        }
        self.run_with(stem, &buf)
    }

    fn field_case(&self, stem: &str, rid: i64, field: &str, value: V) -> Vec<String> {
        match mutate_field(self.built.data(self.idx(stem)), stem, rid, field, value) {
            Ok(buf) => self.run_with(stem, &buf),
            Err(e) => vec![e],
        }
    }

    /// Runs the control and every case. Err = the control failed.
    pub fn run<'s>(&'s self) -> Result<Vec<CaseResult>, String> {
        let control = verify::verify(self.vanilla, &Files::new(self.built), self.plan).0;
        if !control.is_empty() {
            return Err(format!("control (unmutated) check does not pass: {}", control[..control.len().min(3)].join("; ")));
        }
        let p = self.plan;
        let marker = p.marker_id as i32;
        let (first, second) = (p.bullets[0].id as i32, p.bullets[1].id as i32);
        let mut cases: Vec<Case<'s>> = Vec::new();
        let mut add = |label: String, f: Box<dyn Fn() -> Vec<String> + 's>| cases.push((label, f));
        add(
            "flipped byte in a vanilla Bullet row".into(),
            Box::new(|| {
                self.table_case("Bullet", &|buf, rows| {
                    let off = i64::from_le_bytes(buf[rows[0].0 + 8..rows[0].0 + 16].try_into().unwrap()) as usize;
                    buf[off] ^= 0xFF;
                    Ok(())
                })
            }),
        );
        add(
            "expected atkPhys off by 1".into(),
            Box::new(|| {
                let mut p2 = p.clone();
                let r = &mut p2.atks[0];
                let v = r.get("atkPhys").unwrap();
                if let Some(slot) = r.ov.iter_mut().find(|x| x.0 == "atkPhys") {
                    slot.1 = V::I(v.as_i64() + 1);
                }
                verify::verify(self.vanilla, &Files::new(self.built), &p2).0
            }),
        );
        add(
            "missing marker row".into(),
            Box::new(move || {
                self.table_case("Bullet", &|buf, rows| {
                    let e = rows.iter().find(|r| r.1 == marker).ok_or("no marker")?.0;
                    set_id(buf, e, marker + 1);
                    Ok(())
                })
            }),
        );
        add(
            "row out of order".into(),
            Box::new(move || {
                self.table_case("Bullet", &|buf, rows| {
                    let ea = rows.iter().find(|r| r.1 == first).ok_or("no first")?.0;
                    let eb = rows.iter().find(|r| r.1 == second).ok_or("no second")?.0;
                    set_id(buf, ea, second);
                    set_id(buf, eb, first);
                    Ok(())
                })
            }),
        );
        add(
            "flipped byte in an untouched param (EquipParamWeapon)".into(),
            Box::new(|| {
                let Some(k) = self.built.find("EquipParamWeapon.param") else { return vec!["self-test setup: EquipParamWeapon.param unavailable".into()] };
                let mut buf = self.built.data(k).to_vec();
                let mid = buf.len() / 2;
                buf[mid] ^= 0xFF;
                let mut f = Files::new(self.built);
                f.replace.push((k, &buf));
                verify::verify(self.vanilla, &f, p).0
            }),
        );
        add(
            "BND4 manifest changed (an entry id; the XML-era 'manifest missing' case)".into(),
            Box::new(|| {
                let mut raw = self.built.raw.to_vec();
                raw[0x40 + 0x1C] ^= 0x01;
                match Bnd4::parse(&raw) {
                    Ok(b) => verify::verify(self.vanilla, &Files::new(&b), p).0,
                    Err(e) => vec![e],
                }
            }),
        );
        let protect = p.protect_speffect_id;
        let fc = |label: &str, stem: &'static str, rid: i64, field: &'static str, v: V| -> Case<'s> {
            (label.to_string(), Box::new(move || self.field_case(stem, rid, field, v)))
        };
        let mut more = vec![
            fc("M4: selfTarget = 1 on AtkParam_Pc 79000201", "AtkParam_Pc", 79_000_201, "selfTarget", V::I(1)),
            fc("M4: isHitBothTeam = 1 on Bullet 79000302", "Bullet", 79_000_302, "isHitBothTeam", V::I(1)),
            fc(&format!("M4: an extra override (iconId) on SpEffectParam {protect}"), "SpEffectParam", protect, "iconId", V::I(123)),
        ];
        more.push((
            "M4: unsorted SpEffect insert".into(),
            Box::new(move || {
                self.table_case("SpEffectParam", &|buf, rows| {
                    let e = rows.iter().find(|r| r.1 as i64 == protect).ok_or("no protection row")?.0;
                    set_id(buf, e, 1);
                    Ok(())
                })
            }),
        ));
        more.extend([
            fc(&format!("M4: permanent protection (effectEndurance -1) on SpEffectParam {protect}"), "SpEffectParam", protect, "effectEndurance", V::F(-1.0)),
            fc("run 8: weapon-AR damage again (isAddBaseAtk 0) on AtkParam_Pc 79000201", "AtkParam_Pc", 79_000_201, "isAddBaseAtk", V::I(0)),
            fc("run 8: weapon AR x 100% added (atkPhysCorrection 100) on AtkParam_Pc 79000302", "AtkParam_Pc", 79_000_302, "atkPhysCorrection", V::I(100)),
            fc("objects: the 105 core back to atkObj 100 (must beat asset defence 199) on AtkParam_Pc 79000201", "AtkParam_Pc", 79_000_201, "atkObj", V::I(100)),
            fc("objects: no bush-asset damage (isEnableCalcDamageForBushesObj 0) on AtkParam_Pc 79000203", "AtkParam_Pc", 79_000_203, "isEnableCalcDamageForBushesObj", V::I(0)),
            fc("sfx: the 105 ring's dust layer dropped (sfxId_Bullet -1) on Bullet 79000202", "Bullet", 79_000_202, "sfxId_Bullet", V::I(-1)),
            fc("sfx: parent-oriented effect (sfxPostureType 0) on Bullet 79000401", "Bullet", 79_000_401, "sfxPostureType", V::I(0)),
        ]);
        // M6
        let guns: Vec<(String, &crate::pyfmt::Json)> = match p.arsenal.get("guns") {
            Some(crate::pyfmt::Json::Obj(m)) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
            _ => vec![],
        };
        let jint = |j: Option<&crate::pyfmt::Json>| match j {
            Some(crate::pyfmt::Json::Int(i)) => *i,
            _ => 0,
        };
        let gun = guns.iter().find(|g| matches!(g.1.get("bands"), Some(crate::pyfmt::Json::Arr(a)) if !a.is_empty()));
        if let Some((_, g)) = gun {
            let b = match g.get("bands") {
                Some(crate::pyfmt::Json::Arr(a)) => &a[0],
                _ => unreachable!(),
            };
            let b0 = jint(b.get("id"));
            let atk = jint(b.get("atk"));
            more.extend([
                fc(&format!("M6: weapon-AR damage (isAddBaseAtk 0) on AtkParam_Pc {b0}"), "AtkParam_Pc", b0, "isAddBaseAtk", V::I(0)),
                fc(&format!("M6: atkPhysCorrection 100 on AtkParam_Pc {b0}"), "AtkParam_Pc", b0, "atkPhysCorrection", V::I(100)),
                fc(&format!("M6: pierce attribute (atkAttribute 2) on AtkParam_Pc {b0}"), "AtkParam_Pc", b0, "atkAttribute", V::I(2)),
                fc(&format!("M6: band atkPhys off by 1 vs the inversion on AtkParam_Pc {b0}"), "AtkParam_Pc", b0, "atkPhys", V::I(atk + 1)),
                fc(&format!("M6: shield-ignoring gun (disableGuard 1) on AtkParam_Pc {b0}"), "AtkParam_Pc", b0, "disableGuard", V::I(1)),
                fc(&format!("M6: dagger hit effect (sfxId_Hit 300002) with bullet_sfx none on Bullet {b0}"), "Bullet", b0, "sfxId_Hit", V::I(300_002)),
            ]);
        }
        if let Some((_, g)) = guns.iter().find(|g| g.1.get("launcher").is_some()) {
            let lt = g.get("launcher").unwrap();
            let mk = jint(lt.get("marker_id"));
            let single = jint(lt.get("single").and_then(|s| s.get("id")));
            more.extend([
                fc(&format!("M6: launcher marker with an attack (atkId_Bullet 0) on Bullet {mk}"), "Bullet", mk, "atkId_Bullet", V::I(0)),
                fc(&format!("M6: explosion hurts the player (selfTarget 1) on AtkParam_Pc {single}"), "AtkParam_Pc", single, "selfTarget", V::I(1)),
            ]);
        }
        // M7
        let (ls, lb, hb) = (p.lethal_speffect_id, p.lethal_bullet_id, p.horizon_bullet_id);
        more.extend([
            fc(&format!("M7: 99 % instead of 100 % (changeHpRate 99) on SpEffectParam {ls}"), "SpEffectParam", ls, "changeHpRate", V::F(99.0)),
            fc(&format!("M7: a permanent lethal effect (effectEndurance -1) on SpEffectParam {ls}"), "SpEffectParam", ls, "effectEndurance", V::F(-1.0)),
            fc(&format!("M7: noDead 1 on SpEffectParam {ls}"), "SpEffectParam", ls, "noDead", V::I(1)),
            fc(&format!("M7: an HP-burn effect (isHpBurnEffect 1) on SpEffectParam {ls}"), "SpEffectParam", ls, "isHpBurnEffect", V::I(1)),
            fc(&format!("M7: kept on the dead (dontDeleteOnDead 1) on SpEffectParam {ls}"), "SpEffectParam", ls, "dontDeleteOnDead", V::I(1)),
            fc(&format!("M7: a visual effect (vfxId 1) on SpEffectParam {ls}"), "SpEffectParam", ls, "vfxId", V::I(1)),
            fc(&format!("M7: the lethal bullet hurts the player (selfTarget 1) on AtkParam_Pc {lb}"), "AtkParam_Pc", lb, "selfTarget", V::I(1)),
            fc(&format!("M7: the lethal bullet without its SpEffect (spEffectId0 -1) on AtkParam_Pc {lb}"), "AtkParam_Pc", lb, "spEffectId0", V::I(-1)),
            fc(&format!("M7: the horizon explosion attacks (atkId_Bullet {lb}) on Bullet {hb}"), "Bullet", hb, "atkId_Bullet", V::I(lb)),
        ]);
        let inputs = self.inputs;
        more.push((
            "M6: duplicate gun index g".into(),
            Box::new(move || {
                let mut i2 = inputs.clone();
                if i2.arsenal.len() < 2 {
                    return vec!["self-test setup: fewer than 2 arsenal guns".into()];
                }
                let g0 = i2.arsenal[0].1["g"].clone();
                i2.arsenal[1].1["g"] = g0;
                match plan::make_plan(&i2) {
                    Err(e) => vec![e],
                    Ok(_) => vec![],
                }
            }),
        ));
        more.push((
            "M6: sniper body multiplier ignored (Intervention 70 instead of 105)".into(),
            Box::new(move || {
                let mut i2 = inputs.clone();
                let Some(g) = i2.arsenal.iter_mut().find(|g| g.0 == "cheytac_mp") else { return vec!["self-test setup: cheytac_mp not in the arsenal".into()] };
                g.1["num"]["locTorsoUpper"] = serde_json::json!(1.0);
                match plan::make_plan(&i2) {
                    Ok(p2) => plan::arsenal_known_answers(&p2),
                    Err(e) => vec![e],
                }
            }),
        ));
        // C3 PASS C
        let tw_t = p.twins.iter().find(|t| t.0 >= plan::C3_TWIN_BASE + 1000).copied().unwrap_or(p.twins[0]);
        let tw = tw_t.0;
        let tw_ac = p.twins.iter().find(|t| t.0 < plan::C3_TWIN_BASE + 1000).map(|t| t.0).unwrap_or(tw);
        let co_sp_gun = plan::C3_SP_GUN_BASE;
        more.extend([
            fc(&format!("C3: a gun twin that attacks (atkId_Bullet {}) on Bullet {tw}", tw_t.1), "Bullet", tw, "atkId_Bullet", V::I(tw_t.1)),
            fc(&format!("C3: a twin that spawns the impact marker (HitBulletID {marker}) on Bullet {tw}"), "Bullet", tw, "HitBulletID", V::I(marker as i64)),
            fc(&format!("C3: an AC-130 twin that attacks on Bullet {tw_ac}"), "Bullet", tw_ac, "atkId_Bullet", V::I(tw_ac - plan::C3_TWIN_BASE + 79_000_000)),
            fc(&format!("C3: the nuke marker bullet attacks on Bullet {}", plan::C3_NUKE_MARKER_BULLET), "Bullet", plan::C3_NUKE_MARKER_BULLET, "atkId_Bullet", V::I(lb)),
            fc(&format!("C3: an unsynced marker (isDisableNetSync 1) on SpEffectParam {}", plan::C3_SP_MODE), "SpEffectParam", plan::C3_SP_MODE, "isDisableNetSync", V::I(1)),
            fc(&format!("C3: a permanent marker (effectEndurance -1) on SpEffectParam {co_sp_gun}"), "SpEffectParam", co_sp_gun, "effectEndurance", V::F(-1.0)),
            fc(&format!("C3: the MW2-mode marker without the invisible-weapon vfx (vfxId -1) on SpEffectParam {}", plan::C3_SP_MODE), "SpEffectParam", plan::C3_SP_MODE, "vfxId", V::I(-1)),
            fc(&format!("C3: a marker that heals (changeHpRate -10) on SpEffectParam {}", plan::C3_SP_NUKE), "SpEffectParam", plan::C3_SP_NUKE, "changeHpRate", V::F(-10.0)),
            fc(&format!("C3: the lethal nuke SpEffect synced (isDisableNetSync 0) on SpEffectParam {ls}"), "SpEffectParam", ls, "isDisableNetSync", V::I(0)),
        ]);
        more.push((
            "C3: a twin listed for the wrong source row".into(),
            Box::new(|| {
                let mut p2 = p.clone();
                p2.twins[0].1 += 1000;
                verify::verify(self.vanilla, &Files::new(self.built), &p2).0
            }),
        ));
        cases.extend(more);
        let mut out = Vec::new();
        for (label, run) in &cases {
            let probs = run();
            let caught = !probs.is_empty() && !probs[0].starts_with("self-test setup");
            out.push((label.clone(), caught, probs.first().cloned().unwrap_or_default()));
        }
        Ok(out)
    }
}
