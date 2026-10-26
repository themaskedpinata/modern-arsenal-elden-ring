//! Runtime data: `converted\weapons.json`, `build\rows.json`, `converted\sounds\sounds.json`.
//!
//! No MW2 values are compiled into the DLL; everything comes from these files (written by the
//! converter and the regulation build). A missing or inconsistent weapons/rows file makes MW2 mode
//! refuse to turn on (DATA FAIL); missing sounds only mean silence.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use mw2er_core::ac130_audio::Cue;
use mw2er_core::ac130_data;
use mw2er_core::ac130_dmg::{Explosion, gun_tiers};
use mw2er_core::config::Config;
use mw2er_core::falloff::band_damages;
use mw2er_core::rows::{RowsAc130, RowsFile, RowsWeapon};
use mw2er_core::sounds::{SoundsFile, WeaponSounds};
use mw2er_core::weapon_sim::SfxCue;
use mw2er_core::weapons::{WeaponSpec, WeaponsFile};

use crate::log::{log_e, log_i, log_w};
use crate::paths::Paths;

#[derive(Clone, Debug)]
pub struct GameData {
    pub weapon: WeaponSpec,
    pub rows: RowsFile,
    pub rows_weapon: RowsWeapon,
    /// Resolved sound files (absolute paths), possibly empty.
    pub sound_files: Vec<(SfxCue, PathBuf)>,
}

fn read(p: &PathBuf) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn sound_files(sounds_json: &PathBuf, s: &WeaponSounds) -> Vec<(SfxCue, PathBuf)> {
    // Paths in sounds.json are relative to converted\ (the parent of the sounds\ folder).
    let base = sounds_json.parent().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_default();
    [
        (SfxCue::Fire, &s.fire),
        (SfxCue::ReloadLift, &s.reload_lift),
        (SfxCue::ReloadClipOut, &s.reload_clipout),
        (SfxCue::ReloadClipIn, &s.reload_clipin),
        (SfxCue::ReloadChamber, &s.reload_chamber),
    ]
    .into_iter()
    .filter_map(|(cue, rel)| rel.as_ref().map(|r| (cue, base.join(r.replace('/', "\\")))))
    .collect()
}

/// Loads and cross-checks the data for `cfg.gameplay.weapon`. Logs `DATA ...` lines.
pub fn load(paths: &Paths, cfg: &Config) -> Result<GameData, String> {
    let weapon_name = cfg.gameplay.weapon.as_str();
    let wpath = paths.rel(&cfg.paths.weapons_json);
    let weapons = WeaponsFile::parse(&read(&wpath)?)?;
    let weapon = weapons.spec(weapon_name)?;
    log_i!(
        "DATA",
        "weapons.json ok ({} guns) weapon={} mode={} damage={}/{} range={:.2}/{:.2}m fireTime={} clip={} maxAmmo={} reload={}/{} spread={}/{}",
        weapons.weapons.len(),
        weapon.name,
        weapon.fire_mode.label(),
        weapon.damage,
        weapon.min_damage,
        weapon.max_damage_range_m,
        weapon.min_damage_range_m,
        weapon.fire_time_s,
        weapon.clip_size,
        weapon.max_ammo,
        weapon.reload_s,
        weapon.reload_empty_s,
        weapon.spread_min_deg,
        weapon.spread_max_deg
    );

    let rpath = paths.rel(&cfg.paths.rows_json);
    let rows = RowsFile::parse(&read(&rpath)?)?;
    let rows_weapon = rows
        .weapons
        .get(weapon_name)
        .cloned()
        .ok_or_else(|| format!("rows.json has no rows for {weapon_name}; rerun build_regulation.py"))?;
    let want = band_damages(weapon.damage, weapon.min_damage);
    if rows_weapon.band_damages() != want {
        return Err(format!(
            "rows.json bands {:?} do not match weapons.json {}..{} (stale build; rerun build_regulation.py)",
            rows_weapon.band_damages(),
            weapon.damage,
            weapon.min_damage
        ));
    }
    let first = rows_weapon.bands.first().map_or(0, |b| b.id);
    let last = rows_weapon.bands.last().map_or(0, |b| b.id);
    log_i!(
        "DATA",
        "rows.json ok k={} bands={} ids={}..{} marker={} speed={}m/s life={}s built={}",
        rows.k,
        rows_weapon.bands.len(),
        first,
        last,
        rows.marker_id,
        rows.bullet.speed_mps,
        rows.bullet.life_s,
        rows.built_utc
    );
    if (rows.k - cfg.damage.k).abs() > 1e-9 {
        log_w!(
            "CONFIG",
            "WARN damage.k={} but the rows were built with k={}; rerun build_regulation.py (the rows' k applies)",
            cfg.damage.k,
            rows.k
        );
    }

    let spath = paths.rel(&cfg.paths.sounds_json);
    let sound_files = match read(&spath).and_then(|t| SoundsFile::parse(&t)) {
        Ok(s) => match s.weapons.get(weapon_name) {
            Some(ws) => {
                let files = sound_files(&spath, ws);
                for (cue, p) in &files {
                    if !p.is_file() {
                        log_w!("DATA", "WARN sound {} missing: {}", cue.name(), p.display());
                    }
                }
                log_i!("DATA", "sounds.json ok {} sounds for {weapon_name}", files.len());
                files
            }
            None => {
                log_w!("DATA", "WARN sounds.json has no sounds for {weapon_name}; silent");
                Vec::new()
            }
        },
        Err(e) => {
            log_w!("DATA", "WARN {e}; silent");
            Vec::new()
        }
    };

    Ok(GameData { weapon, rows, rows_weapon, sound_files })
}

// ---- M4: converted\ac130\ ---------------------------------------------------------------------------------

/// The AC-130 data (docs/M4_DESIGN.md section 13), loaded once at boot; shared read-only with the present thread.
#[derive(Debug)]
pub struct Ac130Data {
    pub dir: PathBuf,
    pub weapons: ac130_data::WeaponsFile,
    pub timing: ac130_data::TimingFile,
    pub vision: ac130_data::VisionFile,
    pub hud: ac130_data::HudFile,
    pub sounds: ac130_data::SoundsFile,
    /// Resolved files per cue (absolute paths).
    pub sound_files: Vec<(Cue, Vec<PathBuf>)>,
    /// The rows the regulation build added for the guns (from rows.json).
    pub rows: RowsAc130,
}

pub static AC_DATA: OnceLock<Arc<Ac130Data>> = OnceLock::new();

/// Loads and cross-checks `converted\ac130\*.json` against `build\rows.json`'s ac130 block.
pub fn load_ac130(paths: &Paths, cfg: &Config, rows: Option<&RowsFile>) -> Result<Ac130Data, String> {
    let dir = paths.rel(&cfg.ac130.assets_dir);
    let rd = |f: &str| read(&dir.join(f));
    let weapons = ac130_data::WeaponsFile::parse(&rd(ac130_data::WEAPONS_FILE)?)?;
    let timing = ac130_data::TimingFile::parse(&rd(ac130_data::TIMING_FILE)?)?;
    let vision = ac130_data::VisionFile::parse(&rd(ac130_data::VISION_FILE)?)?;
    let hud = ac130_data::HudFile::parse(&rd(ac130_data::HUD_FILE)?)?;
    let sounds = ac130_data::SoundsFile::parse(&rd(ac130_data::SOUNDS_FILE)?)?;
    let rows = rows
        .and_then(|r| r.ac130.clone())
        .ok_or("rows.json has no ac130 block (rerun build_regulation.py with converted\\ac130 present)")?;
    // nothing hard-coded twice: the rows must follow from the converted MW2 numbers and the rows' k
    for (i, gid) in ac130_data::GUN_IDS.iter().enumerate() {
        let g = &weapons.guns[i];
        let e = Explosion { radius_in: g.explosion_radius_in, inner: g.explosion_inner, outer: g.explosion_outer };
        let want = gun_tiers(&e, rows.k_gun(gid), *gid != "25").map_err(|e| format!("ac130 {gid}: {e}"))?;
        let got = rows.gun(gid).ok_or_else(|| format!("rows.json ac130 has no gun {gid}"))?;
        let pairs = [(want.core, got.core), (want.ring, got.ring), (Some(want.single), Some(got.single))];
        for (w, r) in pairs {
            match (w, r) {
                (Some(w), Some(r)) if w.atk_phys == r.atk_phys as i64 && (w.radius_m - r.radius_m).abs() < 1e-4 => {}
                (None, None) => {}
                _ => return Err(format!("rows.json ac130 {gid} differs from weapons_ac130.json (stale build; rerun build_regulation.py)")),
            }
        }
    }
    let mut sound_files = Vec::new();
    for cue in Cue::ALL {
        match sounds.cues.get(cue.key()) {
            Some(c) => sound_files.push((cue, c.files.iter().map(|f| dir.join(f.replace('/', "\\"))).collect())),
            None => log_w!("AC130", "WARN sounds_ac130.json has no cue {}; silent", cue.key()),
        }
    }
    Ok(Ac130Data { dir, weapons, timing, vision, hud, sounds, sound_files, rows })
}

/// `load_ac130` with the result logged; the AC-130 stays unavailable (`rows_missing`) on failure, M2/M3 unaffected.
pub fn load_ac130_logged(paths: &Paths, cfg: &Config, rows: Option<&RowsFile>) -> Option<Arc<Ac130Data>> {
    match load_ac130(paths, cfg, rows) {
        Ok(d) => {
            let g = &d.weapons.guns;
            log_i!(
                "DATA",
                "ac130 ok dir={} guns clip={}/{}/{} fireTime={}/{}/{} duration_s={} rig={:.1}/{:.1}in visions={} hud_items={} cues={} rows k={} k_gun={:.3}/{:.3}/{:.3} atk_mode={}",
                d.dir.display(),
                g[0].clip_size,
                g[1].clip_size,
                g[2].clip_size,
                g[0].fire_time_s,
                g[1].fire_time_s,
                g[2].fire_time_s,
                d.timing.duration_s,
                d.timing.rig_radius_in,
                d.timing.rig_height_in,
                d.vision.visions.len(),
                d.hud.items.len(),
                d.sound_files.len(),
                d.rows.k,
                d.rows.k_gun("105"),
                d.rows.k_gun("40"),
                d.rows.k_gun("25"),
                if d.rows.atk_mode.is_empty() { "weapon_ar(old build)" } else { d.rows.atk_mode.as_str() }
            );
            let a = Arc::new(d);
            let _ = AC_DATA.set(a.clone());
            Some(a)
        }
        Err(e) => {
            log_e!("DATA", "FAIL ac130 {e}; the AC-130 is unavailable (MW2 mode unaffected)");
            None
        }
    }
}

/// `load` with the failure logged as `DATA FAIL`.
pub fn load_logged(paths: &Paths, cfg: &Config) -> Option<GameData> {
    match load(paths, cfg) {
        Ok(d) => Some(d),
        Err(e) => {
            log_e!("DATA", "FAIL {e}; MW2 mode will refuse to enable");
            None
        }
    }
}

// ---- M7: converted\nuke\ ----------------------------------------------------------------------------------

/// The nuke data (docs/M7_DESIGN.md 3.1), loaded once at boot; shared read-only with the present / audio / HUD threads.
#[derive(Debug)]
pub struct NukeData {
    pub dir: PathBuf,
    pub file: mw2er_core::nuke_data::NukeFile,
    /// The rows the regulation build added (rows.json "nuke").
    pub rows: mw2er_core::rows::RowsNuke,
    /// Resolved files per sound key (absolute paths).
    pub sound_files: Vec<(String, Vec<PathBuf>)>,
    /// The dpad_nuke icon as RGBA8 (width, height, pixels), if the file loaded.
    pub icon: Option<(u32, u32, Vec<u8>)>,
}

pub static NUKE_DATA: OnceLock<Arc<NukeData>> = OnceLock::new();

pub fn load_nuke(paths: &Paths, cfg: &Config, rows: Option<&RowsFile>) -> Result<NukeData, String> {
    let dir = paths.rel(&cfg.nuke.assets_dir);
    let file = mw2er_core::nuke_data::NukeFile::parse(&read(&dir.join(mw2er_core::nuke_data::NUKE_FILE))?)?;
    let rows = rows.and_then(|r| r.nuke).ok_or("rows.json has no nuke block (rerun build_regulation.py)")?;
    let mut sound_files = Vec::new();
    for (k, s) in &file.sounds {
        sound_files.push((k.clone(), s.files.iter().map(|f| dir.join(f.replace('/', "\\"))).collect()));
    }
    let icon = if file.hud.icon_file.is_empty() {
        None
    } else {
        std::fs::read(dir.join(&file.hud.icon_file)).ok().and_then(|b| {
            if b.len() < 16 || &b[..8] != b"MW2RGBA1" {
                return None;
            }
            let w = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
            let h = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
            (w > 0 && h > 0 && w <= 2048 && h <= 2048 && b.len() == 16 + (w * h * 4) as usize).then(|| (w, h, b[16..].to_vec()))
        })
    };
    Ok(NukeData { dir, file, rows, sound_files, icon })
}

/// `load_nuke` with the result logged; the nuke stays unavailable (`NUKE blocked reason=data`) on failure.
pub fn load_nuke_logged(paths: &Paths, cfg: &Config, rows: Option<&RowsFile>) -> Option<Arc<NukeData>> {
    match load_nuke(paths, cfg, rows) {
        Ok(d) => {
            let t = &d.file.timeline;
            log_i!(
                "DATA",
                "nuke ok dir={} cost={} timer={}s incoming=T-{}s vision=T+{}s/{}s death=T+{}s aftermath={}s beep={}s quake={}x{}s(commented={}) aliases={} icon={} rows speffect={} bullet={} horizon={}",
                d.dir.display(),
                d.file.cost,
                t.timer_s,
                t.incoming_lead_s,
                t.vision_delay_s,
                t.vision_blend_s,
                t.death_delay_s,
                t.aftermath_blend_s,
                t.beep_every_s,
                t.quake_scale,
                t.quake_duration_s,
                t.quake_commented as u8,
                d.file.sounds.len(),
                d.icon.as_ref().map_or("none".to_string(), |i| format!("{}x{}", i.0, i.1)),
                d.rows.lethal_speffect_id,
                d.rows.lethal_bullet_id,
                d.rows.horizon_bullet_id
            );
            if d.file.cost != cfg.nuke.streak_cost {
                log_w!("NUKE", "WARN nuke.streak_cost={} differs from MW2's {} (killstreaktable.csv); the config applies", cfg.nuke.streak_cost, d.file.cost);
            }
            let a = Arc::new(d);
            let _ = NUKE_DATA.set(a.clone());
            Some(a)
        }
        Err(e) => {
            log_e!("DATA", "FAIL nuke {e}; the Tactical Nuke is unavailable (MW2 mode unaffected)");
            None
        }
    }
}