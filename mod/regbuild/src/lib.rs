//! regbuild: the Rust regulation builder (docs/INSTALLER_DESIGN.md P2), in place of WitchyBND.
//!
//! regulation.bin = AES-256-CBC ([`crypto`]) of a DCX_ZSTD container ([`dcx`]) of a BND4 ([`bnd4`]) of 194 PARAM files
//! ([`param`]). [`build`] reads the player's vanilla regulation.bin (read only, sha256-pinned), adds exactly the rows
//! tools/scripts/build_regulation.py adds ([`plan`]: same inputs, same rules), rebuilds BND4 -> DCX_ZSTD level 21 ->
//! AES, re-reads its own output and verifies it ([`verify`]), then publishes regulation.bin + rows.json (temp file +
//! rename). Untouched params are copied as raw bytes; the 3 edited ones are re-serialised by the SoulsFormats rules,
//! which reproduces WitchyBND's BND4 byte for byte. Only the zstd frame differs from WitchyBND's (libzstd 1.5.7 here,
//! 1.4.5 there), so the package sha256 changes once versus the WitchyBND build.
//!
//! No game is started, no game folder is written: the vanilla file is only read.

pub mod bnd4;
pub mod crypto;
pub mod dcx;
pub mod diff;
pub mod fields;
pub mod param;
pub mod plan;
pub mod pyfmt;
pub mod rows_json;
pub mod selftest;
pub mod verify;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use sha2::{Digest, Sha256};

/// SHA-256 of the vanilla regulation.bin (ER 2.7.2.0, Steam build 25827938; == build_regulation.VANILLA_SHA256).
pub const VANILLA_SHA256: &str = "b8cdab84c0772f66811fdc5176a381877ca6657b37d420cc2c1562a90d63870e";
/// The BND4 version string of that regulation.
pub const REGULATION_VERSION: &str = "11730034";
/// What `rows.json` `generator` says (decided once: the builder and its version).
pub const GENERATOR: &str = concat!("regbuild ", env!("CARGO_PKG_VERSION"));
/// The builder id recorded in the installer's skip key / the release (the zstd crate pins libzstd 1.5.7).
pub const BUILDER_ID: &str = concat!("regbuild ", env!("CARGO_PKG_VERSION"), ", zstd 0.13.3 (libzstd 1.5.7), level 21, windowLog 16");

pub fn sha256_hex(b: &[u8]) -> String {
    let d = Sha256::digest(b);
    d.iter().map(|x| format!("{x:02x}")).collect()
}

/// A decrypted + decompressed regulation.
#[derive(Clone, Debug)]
pub struct Unpacked {
    pub file_sha256: String,
    pub file_len: usize,
    pub iv: [u8; 16],
    pub dcx_header: [u8; dcx::HEADER_LEN],
    pub frame: Vec<u8>,
    /// AES plaintext bytes after the DCX (zero padding + PKCS7 in WitchyBND's files, none in vanilla)
    pub trailer: Vec<u8>,
    pub bnd: Vec<u8>,
}

impl Unpacked {
    pub fn bnd4(&self) -> Result<bnd4::Bnd4<'_>, String> {
        bnd4::Bnd4::parse(&self.bnd)
    }
}

/// regulation.bin bytes -> BND4 bytes (and the container facts).
pub fn unpack(file: &[u8]) -> Result<Unpacked, String> {
    let plain = crypto::decrypt(file)?;
    let d = dcx::Dcx::parse(&plain)?;
    let bnd = d.decompress()?;
    Ok(Unpacked {
        file_sha256: sha256_hex(file),
        file_len: file.len(),
        iv: crypto::iv_of(file).unwrap(),
        dcx_header: d.header,
        frame: d.frame.to_vec(),
        trailer: plain[dcx::HEADER_LEN + d.frame.len()..].to_vec(),
        bnd,
    })
}

/// BND4 bytes -> regulation.bin bytes (DCX_ZSTD with `template`'s header, level 21 / windowLog 16, AES IV 0).
pub fn pack(bnd: &[u8], template: &[u8; dcx::HEADER_LEN]) -> Result<Vec<u8>, String> {
    let frame = dcx::compress(bnd)?;
    Ok(crypto::encrypt(&dcx::write(template, bnd.len(), &frame)?))
}

/// Applies the plan to the vanilla BND4: the new file list (edited params rebuilt, every other file the vanilla bytes).
/// Ports build_regulation.edit_xml's guards: each clone source exactly once, the plan sorted, no vanilla id in our
/// range, every override field known.
pub fn apply(vanilla: &bnd4::Bnd4, p: &plan::Plan) -> Result<Vec<Vec<u8>>, String> {
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(vanilla.entries.len());
    for (k, e) in vanilla.entries.iter().enumerate() {
        let stem = e.file_name().trim_end_matches(".param");
        if !plan::EDITED.contains(&stem) {
            out.push(vanilla.data(k).to_vec());
            continue;
        }
        let par = param::Param::parse(vanilla.data(k)).map_err(|x| format!("{stem}: {x}"))?;
        let layout = fields::layout_of(stem).unwrap();
        if par.param_type != layout.param_type || (!par.rows.is_empty() && par.row_size != layout.row_size) {
            return Err(format!("{stem}: type {} row size {} (the field table has {} {}): a regulation patch changed the layout; regenerate fields_gen.rs after review", par.param_type, par.row_size, layout.param_type, layout.row_size));
        }
        let rows = p.rows(stem);
        let mut clones: Vec<i64> = rows.iter().map(|r| r.clone).collect();
        clones.sort();
        clones.dedup();
        for c in &clones {
            let n = par.rows.iter().filter(|r| r.id as i64 == *c).count();
            if n != 1 {
                return Err(format!("{stem}.param: clone source {c} found {n} times"));
            }
        }
        if rows.windows(2).any(|w| w[0].id > w[1].id) {
            return Err(format!("{stem}.param: the planned rows are not sorted"));
        }
        if let (Some(lo), Some(hi)) = (rows.first().map(|r| r.id), rows.last().map(|r| r.id)) {
            let clash: Vec<i32> = par
                .rows
                .iter()
                .map(|r| r.id)
                .filter(|&id| (lo..=hi).contains(&(id as i64)) || (plan::ID_RANGE.0..=plan::ID_RANGE.1).contains(&(id as i64)))
                .take(5)
                .collect();
            if !clash.is_empty() {
                return Err(format!("{stem}.param: vanilla rows already use ids in our range: {clash:?}"));
            }
        }
        let mut new = Vec::with_capacity(rows.len());
        for r in rows {
            let unknown: Vec<&str> = r.ov.iter().map(|(k, _)| *k).filter(|k| layout.field(k).is_none()).collect();
            if !unknown.is_empty() {
                return Err(format!("{stem}.param: override fields not in the paramdef: {unknown:?}"));
            }
            let mut data = par.row(r.clone as i32).unwrap().data.to_vec();
            for (k, v) in &r.ov {
                fields::write(&mut data, layout.field(k).unwrap(), *v).map_err(|x| format!("{stem}[{}]: {x}", r.id))?;
            }
            let id = i32::try_from(r.id).map_err(|_| format!("{stem}: row id {} out of i32", r.id))?;
            new.push(param::NewRow { id, name: r.name.clone(), data });
        }
        out.push(param::insert_rows(&par, &new)?);
    }
    Ok(out)
}

pub struct BuildOptions<'a> {
    /// The MW2inER folder (mw2er.toml, converted\).
    pub root: PathBuf,
    /// The vanilla regulation.bin (the game's file or the staged copy; only read).
    pub vanilla: PathBuf,
    /// Where regulation.bin is published (its folder must hold nothing else, as build_regulation.publish requires).
    pub package: PathBuf,
    pub rows_json: PathBuf,
    /// rows.json `built_utc` (None = now, `YYYYMMDDTHHMMSSZ`).
    pub built_utc: Option<String>,
    pub cancel: Option<&'a AtomicBool>,
}

#[derive(Clone, Debug)]
pub struct BuildResult {
    pub package_sha256: String,
    pub package_len: usize,
    pub vanilla_sha256: String,
    pub added: [(&'static str, usize); 3],
    pub report: verify::Report,
    /// (phase, seconds)
    pub timings: Vec<(&'static str, f64)>,
    /// The info lines build_regulation.py prints after a PASS.
    pub lines: Vec<String>,
}

fn cancelled(c: Option<&AtomicBool>) -> Result<(), String> {
    if c.is_some_and(|c| c.load(Ordering::Relaxed)) { Err("cancelled".into()) } else { Ok(()) }
}

/// The UTC time as `YYYYMMDDTHHMMSSZ` (build_regulation's stamp).
pub fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Writes `bytes` to `path` through a temp file in the same folder + rename.
pub fn publish_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("out"), std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// The whole build (build_regulation.build without WitchyBND). `progress` gets one line per phase.
pub fn build(o: &BuildOptions, progress: &mut dyn FnMut(&str)) -> Result<BuildResult, String> {
    let mut timings = Vec::new();
    let mut t = Instant::now();
    let lap = |name: &'static str, t: &mut Instant, timings: &mut Vec<(&'static str, f64)>| {
        timings.push((name, t.elapsed().as_secs_f64()));
        *t = Instant::now();
    };
    let built_utc = o.built_utc.clone().unwrap_or_else(utc_stamp);
    progress("reading the vanilla regulation.bin (read only)");
    let raw = std::fs::read(&o.vanilla).map_err(|e| format!("game regulation not readable: {}: {e}", o.vanilla.display()))?;
    let vsha = sha256_hex(&raw);
    if vsha != VANILLA_SHA256 {
        return Err(format!("game regulation changed (sha256 {vsha}); the pinned vanilla is {VANILLA_SHA256} (Steam > Verify integrity, or a new mod release for the new patch)"));
    }
    let van = unpack(&raw)?;
    let vb = van.bnd4()?;
    if vb.version != REGULATION_VERSION || vb.entries.len() != verify::PARAM_COUNT {
        return Err(format!("vanilla BND4 version {} with {} files (want {REGULATION_VERSION}, {})", vb.version, vb.entries.len(), verify::PARAM_COUNT));
    }
    if van.dcx_header[0x30] != dcx::LEVEL as u8 {
        return Err(format!("vanilla DCX level {} (want {})", van.dcx_header[0x30], dcx::LEVEL));
    }
    lap("unpack", &mut t, &mut timings);
    cancelled(o.cancel)?;
    progress("planning the rows (mw2er.toml + converted data)");
    let inputs = plan::load_inputs(&o.root)?;
    let p = plan::make_plan(&inputs)?;
    let (n_assets, broken) = verify::asset_break_report(&vb)?;
    let mut problems = plan::arsenal_known_answers(&p);
    let files = apply(&vb, &p)?;
    let refs: Vec<&[u8]> = files.iter().map(|f| f.as_slice()).collect();
    let bnd = vb.write(&refs)?;
    drop(files);
    lap("plan+apply", &mut t, &mut timings);
    cancelled(o.cancel)?;
    progress("compressing (zstd level 21, ~20 s)");
    let out = pack(&bnd, &van.dcx_header)?;
    drop(bnd);
    lap("compress+encrypt", &mut t, &mut timings);
    cancelled(o.cancel)?;
    progress("re-reading and verifying the output");
    let back = unpack(&out)?;
    let bb = back.bnd4()?;
    if !bb.canonical_layout() {
        problems.push("output BND4 data layout is not the 16-byte chain".into());
    }
    let (vp, report) = verify::verify(&vb, &verify::Files::new(&bb), &p);
    problems.extend(vp);
    lap("verify", &mut t, &mut timings);
    if !problems.is_empty() {
        let n = problems.len();
        let mut msg = problems;
        msg.truncate(20);
        return Err(format!("round trip found {n} problem(s); package NOT updated:\n  {}", msg.join("\n  ")));
    }
    cancelled(o.cancel)?;
    if let Some(dir) = o.package.parent()
        && let Ok(rd) = std::fs::read_dir(dir)
    {
        let others: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| Some(n.as_str()) != o.package.file_name().and_then(|f| f.to_str()))
            .collect();
        if !others.is_empty() {
            return Err(format!("{} must contain only {} (found {others:?})", dir.display(), o.package.display()));
        }
    }
    publish_file(&o.package, &out)?;
    let package_sha256 = sha256_hex(&out);
    let bullet = vb.find("Bullet.param").ok_or("Bullet.param missing")?;
    let bp = param::Param::parse(vb.data(bullet))?;
    let clone = bp.row(p.clone_bullet as i32).ok_or("the Bullet clone row is missing")?;
    let hit_radius = pyfmt::round6(fields::read(clone.data, fields::BULLET.field("hitRadius").unwrap()).as_f64());
    let rows = rows_json::rows_json(&p, &built_utc, &package_sha256, hit_radius);
    publish_file(&o.rows_json, (rows.dumps() + "\n").as_bytes())?;
    lap("publish", &mut t, &mut timings);
    let mut lines = vec![format!(
        "round trip: {} untouched params byte-identical; {}; {} added rows field-checked against the clone source",
        report.untouched_identical,
        report.edited.iter().map(|(s, a, b, _)| format!("{s} {a}->{b}")).collect::<Vec<_>>().join(", "),
        report.rows_checked
    )];
    lines.push(format!(
        "  objects [I]: of {n_assets} vanilla damage-breakable assets (hp > 0) the atkObj of each tier beats: {}",
        broken.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
    ));
    Ok(BuildResult {
        package_sha256,
        package_len: out.len(),
        vanilla_sha256: vsha,
        added: [("Bullet", p.bullets.len()), ("AtkParam_Pc", p.atks.len()), ("SpEffectParam", p.speffects.len())],
        report,
        timings,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_shape() {
        let s = utc_stamp();
        assert_eq!(s.len(), 16);
        assert!(s.starts_with("20") && s.ends_with('Z') && &s[8..9] == "T", "{s}");
    }

    #[test]
    fn pack_unpack_round_trip_of_a_fixture() {
        let p = param::build_fixture("X_ST", 4, &[(1, "", vec![1, 2, 3, 4])]);
        let b = bnd4::build_fixture(REGULATION_VERSION, &[("X.param", &p), ("Y.param", b"yy")]);
        let mut tmpl = [0u8; dcx::HEADER_LEN];
        tmpl[..4].copy_from_slice(b"DCX\0");
        for (o, v) in [(4usize, 0x11000u32), (8, 0x18), (0xC, 0x24), (0x10, 0x44), (0x14, 0x4C), (0x2C, 0x20), (0x48, 8)] {
            tmpl[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        tmpl[0x18..0x1C].copy_from_slice(b"DCS\0");
        tmpl[0x24..0x2C].copy_from_slice(b"DCP\0ZSTD");
        tmpl[0x30] = 21;
        tmpl[0x44..0x48].copy_from_slice(b"DCA\0");
        let file = pack(&b, &tmpl).unwrap();
        let u = unpack(&file).unwrap();
        assert_eq!(u.bnd, b);
        assert_eq!(u.iv, [0u8; 16]);
        assert_eq!((dcx::HEADER_LEN + u.frame.len() + u.trailer.len()) % 16, 0);
        assert!(u.trailer.ends_with(&[16u8; 16]));
        assert_eq!(pack(&b, &tmpl).unwrap(), file, "deterministic");
    }
}
