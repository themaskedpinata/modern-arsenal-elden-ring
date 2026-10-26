//! M6: the MW2 arsenal at runtime (docs/M6_DESIGN.md sections 4, 5, 11): the data of every gun loaded at boot
//! (`converted\guns\<name>\{stats,sounds}.json` + `build\rows.json`'s arsenal block), the shared sound-alias table
//! the audio thread loads, and the per-gun enable flags (a gun whose stats, rows, sounds or viewmodel fail is
//! disabled on its own; the others keep working). No MW2 value is compiled in: everything is read from these files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use mw2er_core::arsenal::{GunSpec, IndexFile, StatsFile};
use mw2er_core::config::Config;
use mw2er_core::rows::{ArsenalGunRows, RowsArsenal, RowsFile};
use mw2er_core::sounds::GunSoundsFile;

use crate::log::{log_e, log_i, log_w};
use crate::paths::Paths;

/// One gun's data (immutable after boot).
#[derive(Clone, Debug)]
pub struct GunData {
    pub name: String,
    pub spec: GunSpec,
    pub rows: ArsenalGunRows,
    /// role -> alias id, notetrack name -> alias id (into the shared [`SoundTable`]).
    pub roles: HashMap<String, u16>,
    pub notes: HashMap<String, u16>,
}

/// Every gun of the arsenal in cycle order (`[arsenal] guns`, or index.json's order).
#[derive(Debug)]
pub struct ArsenalData {
    pub dir: PathBuf,
    pub guns: Vec<GunData>,
    /// Guns of the list that failed to load (name, why): they are not in `guns`.
    pub disabled: Vec<(String, String)>,
    pub rows: RowsArsenal,
    pub marker_id: i32,
    pub bullet_speed_mps: f64,
    /// C3 PASS C: the co-op twin / marker rows (rows.json `coop`; None before C3 PASS C).
    pub coop: Option<mw2er_core::rows::RowsCoop>,
}

impl ArsenalData {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.guns.iter().position(|g| g.name == name)
    }
}

/// One WAV variant of an alias.
#[derive(Clone, Debug)]
pub struct VariantSound {
    pub path: PathBuf,
    pub vol: [f32; 2],
    pub pitch: [f32; 2],
    pub secondary: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct AliasSound {
    pub name: String,
    pub variants: Vec<VariantSound>,
    /// A fire role of some gun (truncated like the M2 fire sound).
    pub fire: bool,
}

/// Every alias any gun uses, deduplicated by name (the audio thread decodes each file once).
#[derive(Clone, Debug, Default)]
pub struct SoundTable {
    pub aliases: Vec<AliasSound>,
    pub index: HashMap<String, u16>,
}

pub static ARSENAL: OnceLock<Arc<ArsenalData>> = OnceLock::new();
pub static SOUNDS: OnceLock<Arc<SoundTable>> = OnceLock::new();
/// The viewmodel loader marks a gun whose bundle failed (bit = slot); the task disables it for switching.
pub static VM_GUN_FAILED: AtomicU64 = AtomicU64::new(0);
/// The viewmodel loader marks a gun whose bundle is on the GPU (bit = slot).
pub static VM_GUN_READY: AtomicU64 = AtomicU64::new(0);

pub fn vm_failed(slot: usize) -> bool {
    // review M6: a slot beyond the mask has no assets (load() never keeps more than 64 guns)
    slot >= 64 || VM_GUN_FAILED.load(Ordering::Relaxed) & (1u64 << slot) != 0
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The gun list in cycle order: `[arsenal] guns`, else every ok gun of index.json.
pub fn gun_names(dir: &Path, cfg: &Config) -> Result<Vec<String>, String> {
    if !cfg.arsenal.guns.is_empty() {
        return Ok(cfg.arsenal.guns.clone());
    }
    let idx = IndexFile::parse(&read(&dir.join("index.json"))?)?;
    Ok(idx.guns.into_iter().map(|g| g.name).collect())
}

fn intern(t: &mut SoundTable, dir: &Path, s: &GunSoundsFile, alias: &str, depth: u32) -> Option<u16> {
    if let Some(&i) = t.index.get(alias) {
        return Some(i);
    }
    let e = s.aliases.get(alias)?;
    let id = t.aliases.len() as u16;
    t.index.insert(alias.to_string(), id);
    t.aliases.push(AliasSound { name: alias.to_string(), variants: Vec::new(), fire: false });
    let mut vs = Vec::new();
    for v in &e.variants {
        let secondary = if depth == 0 { v.secondary.as_deref().and_then(|a| intern(t, dir, s, a, 1)) } else { None };
        vs.push(VariantSound { path: dir.join(v.file.replace('/', "\\")), vol: v.vol, pitch: v.pitch, secondary });
    }
    t.aliases[id as usize].variants = vs;
    Some(id)
}

/// Loads every gun of the list (section 4.2). A gun that fails is listed in `disabled` (logged) and skipped.
pub fn load(paths: &Paths, cfg: &Config) -> Result<(ArsenalData, SoundTable), String> {
    let dir = paths.rel(&cfg.arsenal.dir);
    let rows = RowsFile::parse(&read(&paths.rel(&cfg.paths.rows_json))?)?;
    let ar = rows.arsenal.clone().ok_or("rows.json has no arsenal block (rerun build_regulation.py)")?;
    let names = gun_names(&dir, cfg)?;
    let mut guns = Vec::new();
    let mut disabled = Vec::new();
    let mut table = SoundTable::default();
    for name in &names {
        // review M6: the enable / failure flags are u64 masks and the renderer has MAX_ARSENAL_GUNS slots
        if guns.len() >= mw2er_core::config::MAX_ARSENAL_GUNS {
            log_e!("ARSENAL", "disable gun={name} why=slots (at most {} guns)", mw2er_core::config::MAX_ARSENAL_GUNS);
            disabled.push((name.clone(), "slots".to_string()));
            continue;
        }
        let gd = dir.join(name);
        let one = || -> Result<GunData, (String, String)> {
            let st = read(&gd.join("stats.json")).and_then(|t| StatsFile::parse(&t)).map_err(|e| ("stats".to_string(), e))?;
            let spec = GunSpec::from_stats(&st).map_err(|e| ("stats".to_string(), e))?;
            let r = ar.guns.get(name).cloned().ok_or_else(|| ("rows".to_string(), format!("rows.json arsenal has no {name}")))?;
            if r.g != spec.g {
                return Err(("rows".into(), format!("rows.json g {} != stats g {}", r.g, spec.g)));
            }
            if spec.is_launcher() != r.launcher.is_some() {
                return Err(("rows".into(), "launcher rows do not match the class".into()));
            }
            if !spec.is_launcher() {
                let want = spec.band_damages();
                let got: Vec<i32> = r.bands.iter().map(|b| b.mw2).collect();
                if got != want {
                    return Err(("rows".into(), format!("rows.json bands {got:?} != stats {want:?} (stale build)")));
                }
            }
            Ok(GunData { name: name.clone(), spec, rows: r, roles: HashMap::new(), notes: HashMap::new() })
        };
        match one() {
            Ok(mut g) => {
                // sounds: a failure only means silence for this gun
                match read(&gd.join("sounds.json")).and_then(|t| GunSoundsFile::parse(&t)) {
                    Ok(s) => {
                        for (role, a) in &s.roles {
                            if let Some(id) = intern(&mut table, &dir, &s, a, 0) {
                                g.roles.insert(role.clone(), id);
                                if role == "fire" || role == "last_shot" {
                                    table.aliases[id as usize].fire = true;
                                }
                            }
                        }
                        for (note, a) in &s.notes {
                            if let Some(id) = intern(&mut table, &dir, &s, a, 0) {
                                g.notes.insert(note.clone(), id);
                            }
                        }
                    }
                    Err(e) => log_w!("ARSENAL", "WARN gun={name} sounds: {e}; silent"),
                }
                guns.push(g);
            }
            Err((why, e)) => {
                log_e!("ARSENAL", "disable gun={name} why={why} ({e})");
                disabled.push((name.clone(), why));
            }
        }
    }
    if guns.is_empty() {
        return Err("no gun of the arsenal loaded".into());
    }
    Ok((
        ArsenalData {
            dir,
            guns,
            disabled,
            rows: ar,
            marker_id: rows.marker_id,
            bullet_speed_mps: rows.bullet.speed_mps,
            coop: rows.coop.clone(),
        },
        table,
    ))
}

/// `load` with the result logged; publishes the data for the viewmodel loader and the audio thread.
pub fn load_logged(paths: &Paths, cfg: &Config) -> Option<Arc<ArsenalData>> {
    if !cfg.arsenal.enabled {
        log_i!("ARSENAL", "disabled by config ([arsenal] enabled = false): the M2-M5 single gun");
        return None;
    }
    let t0 = std::time::Instant::now();
    match load(paths, cfg) {
        Ok((d, t)) => {
            let files: usize = t.aliases.iter().map(|a| a.variants.len()).sum();
            log_i!(
                "ARSENAL",
                "data n={} ok={} disabled={} sound_aliases={} sound_files={} rows mode={} s={} d_ref={} ms={}",
                d.guns.len() + d.disabled.len(),
                d.guns.len(),
                d.disabled.len(),
                t.aliases.len(),
                files,
                d.rows.mode,
                d.rows.s,
                d.rows.d_ref,
                t0.elapsed().as_millis()
            );
            let d = Arc::new(d);
            let _ = ARSENAL.set(d.clone());
            let _ = SOUNDS.set(Arc::new(t));
            Some(d)
        }
        Err(e) => {
            log_e!("ARSENAL", "FAIL {e}; the M2-M5 single gun is used");
            None
        }
    }
}
