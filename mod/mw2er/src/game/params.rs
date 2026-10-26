//! Runtime PARAMS check (docs/M2_DESIGN.md section 16.5): the rows `build_regulation.py` added are
//! present in the live regulation with the intended values. Uses `SoloParamRepository::get::<P>`,
//! which selects the table by index; never `FD4ParamRepository::get::<ATK_PARAM_ST>`, which can
//! return AtkParam_Npc (first table with that struct name). Read-only: M2 never writes params.

use eldenring::cs::{AtkParam_Pc, Bullet, SoloParamRepository, SpEffectParam};
use fromsoftware_shared::FromStatic;
use mw2er_core::rows::{RowsAc130, RowsFile};

/// M4: every AC-130 explosion row (Bullet + AtkParam_Pc) and the protection SpEffect are live with the intended
/// values. `Ok((bullets, atks, speffects, effects))`; `Err` names the first missing / stale row. After run 9 the
/// object-damage fields (atkObj, isEnableCalcDamageForBushesObj) and the impact SFX (sfxId_Bullet, sfxPostureType) are
/// checked too when rows.json lists them; `effects` is the 'obj=... sfx=...' summary for the log.
pub fn check_ac130(a: &RowsAc130) -> Result<(usize, usize, usize, String), String> {
    // SAFETY: main thread after the boot gate (as `check`).
    let repo = unsafe { SoloParamRepository::instance() }.map_err(|e| format!("SoloParamRepository: {e}"))?;
    let (mut nb, mut na) = (0, 0);
    let (mut objs, mut sfxs) = (Vec::new(), Vec::new());
    for (gid, g) in &a.guns {
        for r in [g.core, g.ring, Some(g.single)].into_iter().flatten() {
            let id = r.id as u32;
            let b = repo.get::<Bullet>(id).ok_or_else(|| format!("ac130 {gid} id={} param=Bullet missing", r.id))?;
            if b.atk_id_bullet() != r.id || (b.hit_radius() as f64 - r.radius_m).abs() > 1e-3 {
                return Err(format!("ac130 {gid} id={} Bullet atkId={} hitRadius={} (want {} / {})", r.id, b.atk_id_bullet(), b.hit_radius(), r.id, r.radius_m));
            }
            if b.sfx_id_bullet() != r.sfx || r.sfx_posture.is_some_and(|p| b.sfx_posture_type() != p) {
                return Err(format!(
                    "ac130 {gid} id={} Bullet sfxId_Bullet={} sfxPostureType={} (want {} / {:?}; rebuild/redeploy the regulation)",
                    r.id,
                    b.sfx_id_bullet(),
                    b.sfx_posture_type(),
                    r.sfx,
                    r.sfx_posture
                ));
            }
            sfxs.push(format!("{}:{}", r.id % 1000, b.sfx_id_bullet()));
            nb += 1;
            let k = repo.get::<AtkParam_Pc>(id).ok_or_else(|| format!("ac130 {gid} id={} param=AtkParam_Pc missing", r.id))?;
            if k.atk_phys() as i32 != r.atk_phys || k.self_target() {
                return Err(format!("ac130 {gid} id={} AtkParam_Pc atkPhys={} selfTarget={} (want {} / false)", r.id, k.atk_phys(), k.self_target(), r.atk_phys));
            }
            if r.atk_obj.is_some_and(|o| k.atk_obj() != o) || r.obj_bushes.is_some_and(|x| k.is_enable_calc_damage_for_bushes_obj() != x) {
                return Err(format!(
                    "ac130 {gid} id={} AtkParam_Pc atkObj={} bushes={} (want {:?} / {:?}; rebuild/redeploy the regulation)",
                    r.id,
                    k.atk_obj(),
                    k.is_enable_calc_damage_for_bushes_obj(),
                    r.atk_obj,
                    r.obj_bushes
                ));
            }
            objs.push(format!("{}:{}{}", r.id % 1000, k.atk_obj(), if k.is_enable_calc_damage_for_bushes_obj() != 0 { "b" } else { "" }));
            na += 1;
        }
    }
    let s = repo
        .get::<SpEffectParam>(a.protect_speffect_id as u32)
        .ok_or_else(|| format!("id={} param=SpEffectParam missing", a.protect_speffect_id))?;
    // review M4 major 2: the protection must be FINITE (it expires on its own if the DLL cannot remove it)
    let e = s.effect_endurance() as f64;
    if !s.no_dead() || !(e > 0.0) || a.protect_endurance_s <= 0.0 || (e - a.protect_endurance_s).abs() > 0.01 {
        return Err(format!(
            "id={} SpEffectParam noDead={} effectEndurance={} (want true / finite {} from rows.json; rebuild the regulation if 0)",
            a.protect_speffect_id,
            s.no_dead(),
            e,
            a.protect_endurance_s
        ));
    }
    Ok((nb, na, 1, format!("obj={} sfx={}", objs.join(","), sfxs.join(","))))
}

/// M7 (docs/M7_DESIGN.md 3.13): the nuke rows are live: the lethal SpEffect one-shot (effectEndurance 0), 100 % max HP,
/// noDead off, not an HP burn, not kept on the dead; the lethal bullet's attack carries it; the horizon row has no attack.
pub fn check_nuke(n: &mw2er_core::rows::RowsNuke) -> Result<String, String> {
    // SAFETY: main thread after the boot gate (as `check`).
    let repo = unsafe { SoloParamRepository::instance() }.map_err(|e| format!("SoloParamRepository: {e}"))?;
    let s = repo
        .get::<SpEffectParam>(n.lethal_speffect_id as u32)
        .ok_or_else(|| format!("id={} param=SpEffectParam missing", n.lethal_speffect_id))?;
    if s.effect_endurance() != 0.0 || (s.change_hp_rate() - 100.0).abs() > 1e-3 || s.no_dead() || s.is_hp_burn_effect() || s.dont_delete_on_dead() || s.is_destined_death_hp_mult() {
        return Err(format!(
            "id={} SpEffectParam effectEndurance={} changeHpRate={} noDead={} hpBurn={} dontDeleteOnDead={} destinedDeath={} (want 0 / 100 / false x4)",
            n.lethal_speffect_id,
            s.effect_endurance(),
            s.change_hp_rate(),
            s.no_dead(),
            s.is_hp_burn_effect(),
            s.dont_delete_on_dead(),
            s.is_destined_death_hp_mult()
        ));
    }
    let k = repo.get::<AtkParam_Pc>(n.lethal_bullet_id as u32).ok_or_else(|| format!("id={} param=AtkParam_Pc missing", n.lethal_bullet_id))?;
    if k.sp_effect_id0() != n.lethal_speffect_id || k.self_target() {
        return Err(format!("id={} AtkParam_Pc spEffectId0={} selfTarget={} (want {} / false)", n.lethal_bullet_id, k.sp_effect_id0(), k.self_target(), n.lethal_speffect_id));
    }
    let b = repo.get::<Bullet>(n.lethal_bullet_id as u32).ok_or_else(|| format!("id={} param=Bullet missing", n.lethal_bullet_id))?;
    if b.atk_id_bullet() != n.lethal_bullet_id {
        return Err(format!("id={} Bullet atkId_Bullet={} (want itself)", n.lethal_bullet_id, b.atk_id_bullet()));
    }
    let h = repo.get::<Bullet>(n.horizon_bullet_id as u32).ok_or_else(|| format!("id={} param=Bullet missing", n.horizon_bullet_id))?;
    if h.atk_id_bullet() != -1 || h.sfx_id_bullet() != n.horizon_sfx {
        return Err(format!("id={} Bullet atkId_Bullet={} sfx={} (want -1 / {})", n.horizon_bullet_id, h.atk_id_bullet(), h.sfx_id_bullet(), n.horizon_sfx));
    }
    Ok(format!("speffect={} endurance=0 hp_rate=100 no_dead=0 bullet={} horizon={} sfx={}", n.lethal_speffect_id, n.lethal_bullet_id, n.horizon_bullet_id, n.horizon_sfx))
}

/// M6 (docs/M6_DESIGN.md 9.7): the arsenal rows of one gun are live with the planned values (flat damage: isAddBaseAtk,
/// corrections 0, both-hands bonus off, byPoint 0, Standard attribute, never selfTarget; bullets: the marker as hit
/// bullet, the planned speed, no hit effects with bullet_sfx none; launcher marker inert). `Ok(rows)`; `Err` names
/// the first mismatch as `id=<id> field=<f> live=<v> want=<w>`.
pub fn check_arsenal_gun(a: &mw2er_core::rows::RowsArsenal, g: &mw2er_core::rows::ArsenalGunRows, marker: i32, speed: f64) -> Result<usize, String> {
    // SAFETY: main thread after the boot gate (as `check`).
    let repo = unsafe { SoloParamRepository::instance() }.map_err(|e| format!("SoloParamRepository: {e}"))?;
    let no_sfx = a.bullet_sfx == "none";
    let mut n = 0;
    let gun_row = |id: i32, atk: i32| -> Result<(), String> {
        let b = repo.get::<Bullet>(id as u32).ok_or_else(|| format!("id={id} field=Bullet live=missing want=present"))?;
        let k = repo.get::<AtkParam_Pc>(id as u32).ok_or_else(|| format!("id={id} field=AtkParam_Pc live=missing want=present"))?;
        let checks: [(&str, String, String); 11] = [
            ("atkId_Bullet", b.atk_id_bullet().to_string(), id.to_string()),
            ("HitBulletID", b.hit_bullet_id().to_string(), marker.to_string()),
            ("initVellocity", format!("{:.2}", b.init_vellocity()), format!("{speed:.2}")),
            ("sfxId_Hit", (if no_sfx { b.sfx_id_hit() } else { -1 }).to_string(), "-1".into()),
            ("atkPhys", k.atk_phys().to_string(), atk.to_string()),
            ("isAddBaseAtk", k.is_add_base_atk().to_string(), "true".into()),
            ("atkPhysCorrection", k.atk_phys_correction().to_string(), "0".into()),
            ("isDisableBothHandsAtkBonus", k.is_disable_both_hands_atk_bonus().to_string(), "true".into()),
            ("spEffectAtkPowerCorrectRate_byPoint", k.sp_effect_atk_power_correct_rate_by_point().to_string(), "0".into()),
            ("atkAttribute", k.atk_attribute().to_string(), "3".into()),
            ("selfTarget", k.self_target().to_string(), "false".into()),
        ];
        for (f, live, want) in checks {
            if live != want {
                return Err(format!("id={id} field={f} live={live} want={want}"));
            }
        }
        if k.disable_guard() {
            return Err(format!("id={id} field=disableGuard live=true want=false"));
        }
        Ok(())
    };
    for b in &g.bands {
        gun_row(b.id, b.atk)?;
        n += 2;
    }
    if let Some(l) = &g.launcher {
        gun_row(l.impact.id, l.impact.atk)?;
        n += 2;
        let m = repo.get::<Bullet>(l.marker_id as u32).ok_or_else(|| format!("id={} field=Bullet live=missing want=present", l.marker_id))?;
        if m.atk_id_bullet() != -1 {
            return Err(format!("id={} field=atkId_Bullet live={} want=-1", l.marker_id, m.atk_id_bullet()));
        }
        n += 1;
        for r in [l.core, l.ring, Some(l.single)].into_iter().flatten() {
            let b = repo.get::<Bullet>(r.id as u32).ok_or_else(|| format!("id={} field=Bullet live=missing want=present", r.id))?;
            let k = repo.get::<AtkParam_Pc>(r.id as u32).ok_or_else(|| format!("id={} field=AtkParam_Pc live=missing want=present", r.id))?;
            if b.atk_id_bullet() != r.id || (b.hit_radius() as f64 - r.radius_m).abs() > 1e-3 {
                return Err(format!("id={} field=hitRadius live={} want={}", r.id, b.hit_radius(), r.radius_m));
            }
            if k.atk_phys() as i32 != r.atk || !k.is_add_base_atk() || k.self_target() {
                return Err(format!(
                    "id={} field=atkPhys/isAddBaseAtk/selfTarget live={}/{}/{} want={}/true/false",
                    r.id,
                    k.atk_phys(),
                    k.is_add_base_atk(),
                    k.self_target(),
                    r.atk
                ));
            }
            n += 2;
        }
    }
    Ok(n)
}

/// C3 PASS C (docs/C3_DESIGN.md 3.3): the co-op rows are live: every twin row without an attack and without a hit
/// bullet, the nuke marker bullet inert, every marker SpEffect synced (isDisableNetSync off) with a finite endurance
/// (the MW2-mode one with the invisible-weapon vfx), and the nuke's lethal SpEffect still never synced.
pub fn check_coop(c: &mw2er_core::rows::RowsCoop) -> Result<String, String> {
    c.validate()?;
    // SAFETY: main thread after the boot gate (as `check`).
    let repo = unsafe { SoloParamRepository::instance() }.map_err(|e| format!("SoloParamRepository: {e}"))?;
    for t in &c.twins {
        let b = repo.get::<Bullet>(t.id as u32).ok_or_else(|| format!("id={} field=Bullet live=missing want=present (twin of {})", t.id, t.src))?;
        if b.atk_id_bullet() != -1 || b.hit_bullet_id() != -1 {
            return Err(format!("id={} field=atkId_Bullet/HitBulletID live={}/{} want=-1/-1 (a twin never attacks)", t.id, b.atk_id_bullet(), b.hit_bullet_id()));
        }
    }
    let m = repo.get::<Bullet>(c.nuke_marker_bullet as u32).ok_or_else(|| format!("id={} field=Bullet live=missing want=present", c.nuke_marker_bullet))?;
    if m.atk_id_bullet() != -1 {
        return Err(format!("id={} field=atkId_Bullet live={} want=-1", c.nuke_marker_bullet, m.atk_id_bullet()));
    }
    let mut ids = vec![c.sp_mode, c.sp_ac130, c.sp_nuke];
    ids.extend((0..c.sp_gun_count).map(|k| c.sp_gun_base + k));
    for id in &ids {
        let s = repo.get::<SpEffectParam>(*id as u32).ok_or_else(|| format!("id={id} field=SpEffectParam live=missing want=present"))?;
        let e = s.effect_endurance() as f64;
        if s.is_disable_net_sync() || !(e > 0.0) || (e - c.marker_endurance_s).abs() > 0.01 {
            return Err(format!("id={id} field=isDisableNetSync/effectEndurance live={}/{e} want=false/{}", s.is_disable_net_sync(), c.marker_endurance_s));
        }
        let want_vfx = if *id == c.sp_mode { c.sp_mode_vfx } else { -1 };
        if s.vfx_id() != want_vfx {
            return Err(format!("id={id} field=vfxId live={} want={want_vfx}", s.vfx_id()));
        }
        // C6 hide: only the MW2-mode row may carry stateInfo 184 (our in-memory hide patch); every other marker stays inert
        let state = s.state_info() as u32;
        let state_ok = state == 0 || (*id == c.sp_mode && state == mw2er_core::coop_vis::STATE_INFO_HIDE_WEAPON);
        if !state_ok {
            return Err(format!("id={id} field=stateInfo live={state} want=0{}", if *id == c.sp_mode { " or 184 (C6 hide patch)" } else { "" }));
        }
    }
    let lethal = repo
        .get::<SpEffectParam>(mw2er_core::coop_vis::NUKE_LETHAL_SP as u32)
        .ok_or_else(|| format!("id={} field=SpEffectParam live=missing", mw2er_core::coop_vis::NUKE_LETHAL_SP))?;
    if !lethal.is_disable_net_sync() {
        return Err(format!("id={} field=isDisableNetSync live=false want=true (the lethal effect must never sync)", mw2er_core::coop_vis::NUKE_LETHAL_SP));
    }
    Ok(format!(
        "twins={} nuke_marker={} markers={} mode={} (vfx {}) guns={}..{} ac130={} nuke={} endurance={}s lethal_unsynced=1",
        c.twins.len(),
        c.nuke_marker_bullet,
        ids.len(),
        c.sp_mode,
        c.sp_mode_vfx,
        c.sp_gun_base,
        c.sp_gun_base + c.sp_gun_count - 1,
        c.sp_ac130,
        c.sp_nuke,
        c.marker_endurance_s
    ))
}

/// Checks every band row of `weapon` plus the marker. `Ok(n)` = rows verified.
pub fn check(rows: &RowsFile, weapon: &str) -> Result<usize, String> {
    // SAFETY: called on the main thread after the boot gate; the repository is a fully loaded
    // singleton by the time a world is loaded (checked again here via the Result).
    let repo = unsafe { SoloParamRepository::instance() }.map_err(|e| format!("SoloParamRepository: {e}"))?;
    let w = rows.weapons.get(weapon).ok_or_else(|| format!("rows.json has no {weapon}"))?;
    let mut n = 0;
    for b in &w.bands {
        let id = b.id as u32;
        let bullet = repo.get::<Bullet>(id).ok_or_else(|| format!("id={} param=Bullet missing", b.id))?;
        if bullet.atk_id_bullet() != b.id {
            return Err(format!("id={} field=atkId_Bullet live={} want={}", b.id, bullet.atk_id_bullet(), b.id));
        }
        if bullet.hit_bullet_id() != rows.marker_id {
            return Err(format!("id={} field=HitBulletID live={} want={}", b.id, bullet.hit_bullet_id(), rows.marker_id));
        }
        if (bullet.init_vellocity() as f64 - rows.bullet.speed_mps).abs() > 0.01 {
            return Err(format!(
                "id={} field=initVellocity live={} want={}",
                b.id,
                bullet.init_vellocity(),
                rows.bullet.speed_mps
            ));
        }
        n += 1;
        let atk = repo.get::<AtkParam_Pc>(id).ok_or_else(|| format!("id={} param=AtkParam_Pc missing", b.id))?;
        if atk.atk_phys() as i32 != b.atk_phys {
            return Err(format!("id={} field=atkPhys live={} want={}", b.id, atk.atk_phys(), b.atk_phys));
        }
        n += 1;
    }
    let marker = repo
        .get::<Bullet>(rows.marker_id as u32)
        .ok_or_else(|| format!("id={} param=Bullet (marker) missing", rows.marker_id))?;
    if marker.atk_id_bullet() != -1 {
        return Err(format!("id={} field=atkId_Bullet live={} want=-1", rows.marker_id, marker.atk_id_bullet()));
    }
    n += 1;
    Ok(n)
}
