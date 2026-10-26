//! Real-data oracle 1 (docs/M3_DESIGN.md 12.1: 1a Rust, 1e cross-decoder) plus the 1g/vm_proj known answers on
//! the converted assets. Ignored unless `MW2ER_ROOT` points at the project (the dumps are the user's own files):
//!   set MW2ER_ROOT=D:\Mods\MW2inER && cargo test -p mw2er-core --test xanim_cross -- --include-ignored --nocapture
//!
//! The XAnim reader below is an independent Rust implementation of the compiled XAnim layout described in
//! tools/scripts/xanim_read.py's docstring and docs/M3_DESIGN.md section 2 (not OAT or IW4L code). It is only used
//! by this test and is not part of the shipped DLL.
//!
//! Limit (review finding, stated plainly): this reader and the Python converter follow the same layout notes,
//! so 1e cannot catch a shared misreading of the byte layout; it catches converter/runtime mismatches.
//! iw4L's own decoder (`fastfile_iw4` + `xmodel_runtime::xanim_clip`) reads the in-memory XAnimParts of a zone,
//! not OAT's compiled files, so it cannot run on these bytes. The third test ports iw4L's *sampling and
//! composition* rules (Apache-2.0, `iw4l` module below) and checks that our runtime pose equals what iw4L would
//! compose from the same decoded keys. That port found the gun translation tracks are deltas (fixed in
//! convert_viewmodel 1.1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mw2er_core::vm_assets::{self, ClipId, VmAssets};
use mw2er_core::vm_pose::{PoseScratch, evaluate, static_state};
use mw2er_core::vm_proj::{ProjParams, build, project};
use mw2er_core::xform::{m_inv_rigid, m_mul, m_point, m_translation};

const Q: f32 = 32767.0;
const HALF_SCALE: f32 = 0.003_921_568_859_368_563;
const FULL_SCALE: f32 = 0.000_015_259_021_893_143_65;

#[derive(Debug, Clone)]
struct XTrack {
    name: String,
    rot_frames: Vec<u16>,
    rot: Vec<[i32; 4]>,
    trans_frames: Vec<u16>,
    trans: Vec<[f32; 3]>,
}

#[derive(Debug, Clone)]
struct XAnim {
    numframes: u16,
    looped: bool,
    fps: u16,
    tracks: Vec<XTrack>,
    notes: Vec<(String, u16)>,
}

struct R<'a> {
    d: &'a [u8],
    o: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.d.get(self.o..self.o + n).ok_or(format!("truncated at {}", self.o))?;
        self.o += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }
    fn f32(&mut self) -> Result<f32, String> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn cstr(&mut self) -> Result<String, String> {
        let rest = &self.d[self.o..];
        let z = rest.iter().position(|&c| c == 0).ok_or("unterminated string")?;
        let s = String::from_utf8(rest[..z].to_vec()).map_err(|_| "bad string")?;
        self.o += z + 1;
        Ok(s)
    }
}

fn w_of(sum: i32) -> i32 {
    let t = 0x3FFF_0001 - sum;
    if t <= 0 { 0 } else { ((t as f64).sqrt() + 0.5).floor() as i32 }
}

fn indices(r: &mut R<'_>, n: usize, byte_idx: bool, loop_frames: usize) -> Result<Vec<u16>, String> {
    if n >= loop_frames {
        return Ok((0..n as u16).collect());
    }
    (0..n).map(|_| if byte_idx { r.u8().map(u16::from) } else { r.u16() }).collect()
}

fn quat_track(r: &mut R<'_>, lf: usize, bi: bool, flip: bool, half: bool) -> Result<(Vec<u16>, Vec<[i32; 4]>), String> {
    let n = r.u16()? as usize;
    let one = |r: &mut R<'_>| -> Result<[i32; 4], String> {
        if half {
            let z = r.i16()? as i32;
            Ok([0, 0, z, w_of(z * z)])
        } else {
            let (x, y, z) = (r.i16()? as i32, r.i16()? as i32, r.i16()? as i32);
            Ok([x, y, z, w_of(x * x + y * y + z * z)])
        }
    };
    match n {
        0 => Ok((vec![], vec![])),
        1 => {
            let mut q = one(r)?;
            if flip {
                q = q.map(|c| -c);
            }
            Ok((vec![0], vec![q]))
        }
        _ => {
            let idx = indices(r, n, bi, lf)?;
            let mut out: Vec<[i32; 4]> = Vec::with_capacity(n);
            for k in 0..n {
                let mut q = one(r)?;
                let neg = if k == 0 {
                    flip
                } else {
                    let p = out[k - 1];
                    (p[0] as i64 * q[0] as i64 + p[1] as i64 * q[1] as i64 + p[2] as i64 * q[2] as i64
                        + p[3] as i64 * q[3] as i64)
                        < 0
                };
                if neg {
                    q = q.map(|c| -c);
                }
                out.push(q);
            }
            Ok((idx, out))
        }
    }
}

fn trans_track(r: &mut R<'_>, lf: usize, bi: bool) -> Result<(Vec<u16>, Vec<[f32; 3]>), String> {
    let n = r.u16()? as usize;
    match n {
        0 => Ok((vec![], vec![])),
        1 => Ok((vec![0], vec![[r.f32()?, r.f32()?, r.f32()?]])),
        _ => {
            let idx = indices(r, n, bi, lf)?;
            let small = r.u8()? != 0;
            let mins = [r.f32()?, r.f32()?, r.f32()?];
            let sc = if small { HALF_SCALE } else { FULL_SCALE };
            let size = [r.f32()? * sc, r.f32()? * sc, r.f32()? * sc];
            let mut t = Vec::with_capacity(n);
            for _ in 0..n {
                let v = if small {
                    [r.u8()? as f32, r.u8()? as f32, r.u8()? as f32]
                } else {
                    [r.u16()? as f32, r.u16()? as f32, r.u16()? as f32]
                };
                t.push([mins[0] + size[0] * v[0], mins[1] + size[1] * v[1], mins[2] + size[2] * v[2]]);
            }
            Ok((idx, t))
        }
    }
}

fn read_xanim(d: &[u8]) -> Result<XAnim, String> {
    let mut r = R { d, o: 0 };
    let version = r.u16()?;
    if version != 17 && version != 18 {
        return Err(format!("version {version}"));
    }
    let nf_raw = r.u16()? as usize;
    let bones = r.u16()? as usize;
    let flags = r.u8()?;
    let _asset_type = r.u8()?;
    let fps = r.u16()?;
    let looped = flags & 1 != 0;
    let delta = flags & 2 != 0;
    let delta3d = version == 18 && flags & 4 != 0;
    let lf = if looped { nf_raw + 1 } else { nf_raw };
    let numframes = lf.saturating_sub(1);
    let bi = numframes < 256;
    if delta || delta3d {
        quat_track(&mut r, lf, bi, false, !delta3d)?;
        trans_track(&mut r, lf, bi)?;
    }
    let mut tracks = Vec::new();
    if bones > 0 {
        let nb = bones.div_ceil(8);
        let flip = r.take(nb)?.to_vec();
        let half = r.take(nb)?.to_vec();
        let names: Vec<String> = (0..bones).map(|_| r.cstr()).collect::<Result<_, _>>()?;
        for (i, name) in names.into_iter().enumerate() {
            let fl = flip[i >> 3] & (1 << (i & 7)) != 0;
            let hf = half[i >> 3] & (1 << (i & 7)) != 0;
            let (rot_frames, rot) = quat_track(&mut r, lf, bi, fl, hf)?;
            let (trans_frames, trans) = trans_track(&mut r, lf, bi)?;
            tracks.push(XTrack { name, rot_frames, rot, trans_frames, trans });
        }
    }
    let nn = r.u8()?;
    let mut notes = Vec::new();
    for _ in 0..nn {
        let n = r.cstr()?;
        notes.push((n, r.u16()?));
    }
    if r.o != d.len() {
        return Err(format!("{} trailing bytes", d.len() - r.o));
    }
    Ok(XAnim { numframes: numframes as u16, looped, fps, tracks, notes })
}

fn root() -> Option<PathBuf> {
    std::env::var_os("MW2ER_ROOT").map(PathBuf::from).filter(|p| p.join("dumps").is_dir())
}

fn xanim_dir(root: &Path) -> PathBuf {
    root.join("dumps").join("common_mp_assets").join("xanim")
}

fn load_assets(root: &Path) -> VmAssets {
    VmAssets::load_dir(&root.join("converted").join("viewmodel")).expect("converted\\viewmodel loads (run convert_viewmodel.py)")
}

#[test]
#[ignore]
fn xanim_cross_all_parse_and_match_vmanim() {
    let Some(root) = root() else {
        println!("SKIP: MW2ER_ROOT not set or no dumps");
        return;
    };
    // 1a (Rust): all 38 UMP45 xanims parse to exact EOF
    let mut files: Vec<PathBuf> = std::fs::read_dir(xanim_dir(&root))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("viewmodel_ump45_"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 38, "38 UMP45 xanims");
    let mut parsed = BTreeMap::new();
    for f in &files {
        let d = std::fs::read(f).unwrap();
        let x = read_xanim(&d).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        // a truncated copy must be rejected
        assert!(read_xanim(&d[..d.len() - 1]).is_err(), "{} truncated accepted", f.display());
        parsed.insert(f.file_name().unwrap().to_string_lossy().to_string(), x);
    }
    // 1e: the Rust reader vs the converted .vmanim on all 12 clips
    let a = load_assets(&root);
    let names: Vec<&str> = a.skel.bones.iter().map(|b| b.name.as_str()).collect();
    let (mut dq_max, mut dt_max, mut keys) = (0.0f32, 0.0f32, 0usize);
    for id in ClipId::ALL {
        let x = &parsed[&format!("viewmodel_ump45_{}", id.name())];
        let c = a.clip(id);
        assert_eq!((c.fps, c.numframes, c.looped), (x.fps, x.numframes, x.looped), "{}", id.name());
        assert_eq!(c.notes, x.notes, "{} notetracks", id.name());
        let mine: Vec<&XTrack> = x.tracks.iter().filter(|t| names.contains(&t.name.as_str())).collect();
        assert_eq!(mine.len(), c.tracks.len(), "{} track count", id.name());
        for xt in mine {
            let bone = names.iter().position(|n| *n == xt.name).unwrap();
            let ct = c.track_of(bone).unwrap_or_else(|| panic!("{} {} missing", id.name(), xt.name));
            assert_eq!(ct.rot_frames, xt.rot_frames, "{} {} rot frames", id.name(), xt.name);
            assert_eq!(ct.trans_frames, xt.trans_frames, "{} {} trans frames", id.name(), xt.name);
            for (cq, xq) in ct.rot.iter().zip(&xt.rot) {
                let xf = xq.map(|v| v as f32 / Q);
                let s = if cq.iter().zip(&xf).map(|(a, b)| a * b).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
                let d = cq.iter().zip(&xf).map(|(a, b)| (a - s * b).abs()).fold(0.0f32, f32::max);
                dq_max = dq_max.max(d);
                keys += 1;
            }
            // gun bones (j_gun and after): the converter stores bind + delta (convert_viewmodel.convert_clip)
            let base = if bone >= a.skel.tags.j_gun as usize { a.skel.bones[bone].t } else { [0.0; 3] };
            for (ct3, xt3) in ct.trans.iter().zip(&xt.trans) {
                let d = (0..3).map(|k| (ct3[k] - (xt3[k] + base[k])).abs()).fold(0.0f32, f32::max);
                dt_max = dt_max.max(d);
                keys += 1;
            }
        }
    }
    assert!(dq_max <= 2e-5, "max |dq| {dq_max}");
    assert!(dt_max <= 1e-4, "max |dt| {dt_max} in");
    println!(
        "PASS: 1a(Rust) 38/38 xanims parse to EOF (truncation rejected); 1e Rust reader == .vmanim on 12 clips, \
         {keys} keys, max |dq| {dq_max:.2e}, max |dt| {dt_max:.2e} in, notetracks equal"
    );
}

#[test]
#[ignore]
fn xanim_cross_known_answers_on_converted_assets() {
    let Some(root) = root() else {
        println!("SKIP: MW2ER_ROOT not set or no dumps");
        return;
    };
    let a = load_assets(&root);
    let t = a.skel.tags;
    let mut s = PoseScratch::new(a.skel.bones.len());
    // 1g: j_gun relative to tag_view at idle
    let (st, now) = static_state(ClipId::Idle, 0.0, 0.0);
    evaluate(&a, &st, now, &mut s);
    let v = m_inv_rigid(&s.world[t.view as usize]);
    let jg = m_translation(&m_mul(&v, &s.world[t.j_gun as usize]));
    for (x, w) in jg.iter().zip([10.90f32, -2.62, -4.13]) {
        assert!((x - w).abs() <= 0.05, "j_gun view {jg:?}");
    }
    // vm_proj: tag_flash projects lower right at idle (1920x1080, hip fov)
    let vp = build(&s.world[t.view as usize], 1920, 1080, 0.0, &ProjParams::default());
    let fl = project(&vp.view_proj, &m_translation(&s.world[t.flash as usize])).unwrap();
    assert!(fl[0] > 0.0 && fl[1] < 0.0, "tag_flash NDC {fl:?}");
    // ADS end: tag_ads local and the rear sight at the screen centre (top row of gun vertices in view x 5..7)
    let (st, now) = static_state(ClipId::Idle, 0.0, 1.0);
    evaluate(&a, &st, now, &mut s);
    let ads_t = s.t[t.ads as usize];
    for (x, w) in ads_t.iter().zip([-4.238f32, 2.618, 1.025]) {
        assert!((x - w).abs() <= 0.01, "tag_ads {ads_t:?}");
    }
    let vinv = m_inv_rigid(&s.world[t.view as usize]);
    let mut pts = Vec::new();
    for surf in a.mesh.surfaces.iter().filter(|s| s.part == 0) {
        for k in surf.first_index..surf.first_index + surf.index_count {
            let vx = a.mesh.vertex(a.mesh.indices[k as usize] as usize).unwrap();
            let mut p = [0.0f32; 3];
            for (j, w) in vx.joints.iter().zip(vx.weights) {
                if w > 0 {
                    let q = m_point(&s.palette[*j as usize], &vx.pos);
                    for c in 0..3 {
                        p[c] += q[c] * w as f32 / 255.0;
                    }
                }
            }
            let pv = m_point(&vinv, &p);
            if (5.0..=7.0).contains(&pv[0]) {
                pts.push(p);
            }
        }
    }
    let view_z = |p: &[f32; 3]| m_point(&vinv, p)[2];
    let top = pts.iter().map(view_z).fold(f32::MIN, f32::max);
    let row: Vec<&[f32; 3]> = pts.iter().filter(|p| view_z(p) >= top - 0.005).collect();
    let c = row.iter().fold([0.0f32; 3], |acc, p| [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]]);
    let c = c.map(|x| x / row.len() as f32);
    let vp = build(&s.world[t.view as usize], 1920, 1080, 1.0, &ProjParams::default());
    let n = project(&vp.view_proj, &c).unwrap();
    assert!(n[0].abs() <= 0.01 && n[1].abs() <= 0.01, "rear sight NDC at ADS end {n:?}");
    // truncating each real converted file at every 97th byte returns Err (never panics)
    let dir = root.join("converted").join("viewmodel");
    for f in [vm_assets::SKEL_FILE, vm_assets::MESH_FILE, vm_assets::ANIM_FILE] {
        let d = std::fs::read(dir.join(f)).unwrap();
        for cut in (0..d.len()).step_by(97 * 31) {
            let r = match f {
                vm_assets::SKEL_FILE => vm_assets::parse_skel(&d[..cut]).map(|_| ()),
                vm_assets::MESH_FILE => vm_assets::parse_mesh(&d[..cut], 91).map(|_| ()),
                _ => vm_assets::parse_anim(&d[..cut], 91).map(|_| ()),
            };
            assert!(r.is_err());
        }
    }
    println!(
        "PASS: known answers: j_gun@idle view {jg:.3?}; tag_flash NDC ({:.3},{:.3}); tag_ads@ADS {ads_t:.3?}; \
         rear sight NDC at ADS end ({:.4},{:.4}); real-file truncations rejected",
        fl[0], fl[1], n[0], n[1]
    );
}

/// Port of the sampling and pose-composition rules of iw4L (https://github.com/vladtrc/iw4L, commit
/// fc3d8eaf6045cb131eb74cd41954f0c4922b9913, Apache License 2.0; its NOTICE and LICENSE are kept in
/// vendor\iw4L). Functions follow `crates/anim_iw4/src/sample.rs` (span, sparse keys, time_to_frame),
/// `quat.rs` (lerp_quat, normalize), `channel.rs` (full_quat), `compose.rs` (compose_translation) and
/// `crates/xmodel_runtime/src/dobj.rs` (DObj::pose locals: controlled bones take the anim rotation and
/// XModel.trans + anim translation; uncontrolled bones take the bind rotation and XModel.trans). Test-only code,
/// not shipped. Changes: f32 arrays instead of glam, a single anim at weight 1, our Mat34 helpers for FK.
mod iw4l {
    pub type Quat = [f32; 4];
    pub type Vec3 = [f32; 3];

    pub fn normalize(q: Quat) -> Quat {
        let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if len > 1e-6 { [q[0] / len, q[1] / len, q[2] / len, q[3] / len] } else { [0.0, 0.0, 0.0, 1.0] }
    }

    pub fn full_quat(v: [i32; 4]) -> Quat {
        normalize([v[0] as f32 / 32767.0, v[1] as f32 / 32767.0, v[2] as f32 / 32767.0, v[3] as f32 / 32767.0])
    }

    /// sample.rs `span` (Sparse kind).
    pub fn span(frames: &[u16], count: usize, frame: f32) -> (usize, usize, f32) {
        if count <= 1 || frames.len() < count {
            return (0, 0, 0.0);
        }
        if frame <= frames[0] as f32 {
            return (0, 0, 0.0);
        }
        let last = count - 1;
        if frame >= frames[last] as f32 {
            return (last, last, 0.0);
        }
        let mut i = 0;
        while i < last {
            let (start, end) = (frames[i] as f32, frames[i + 1] as f32);
            if frame <= end {
                return (i, i + 1, if end > start { (frame - start) / (end - start) } else { 0.0 });
            }
            i += 1;
        }
        (last, last, 0.0)
    }

    pub fn time_to_frame(time: f32, framerate: f32, numframes: u16, looping: bool) -> f32 {
        if framerate <= 0.0 {
            return 0.0;
        }
        let raw = time * framerate;
        if looping && numframes > 0 {
            let n = numframes as f32;
            raw - n * (raw / n).floor()
        } else {
            raw.clamp(0.0, numframes as f32)
        }
    }

    pub fn lerp4(a: Quat, b: Quat, t: f32) -> Quat {
        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2] + t * (b[2] - a[2]), a[3] + t * (b[3] - a[3])]
    }

    pub fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2] + t * (b[2] - a[2])]
    }

    /// compose.rs `compose_translation` (scale 1).
    pub fn compose_translation(bind: Vec3, anim: Vec3) -> Vec3 {
        [bind[0] + anim[0], bind[1] + anim[1], bind[2] + anim[2]]
    }
}

#[test]
#[ignore]
fn xanim_cross_iw4l_composition_matches_runtime() {
    let Some(root) = root() else {
        println!("SKIP: MW2ER_ROOT not set or no dumps");
        return;
    };
    let a = load_assets(&root);
    let names: Vec<&str> = a.skel.bones.iter().map(|b| b.name.as_str()).collect();
    let n = names.len();
    let gun0 = a.skel.tags.j_gun as usize;
    // XModel.trans per bone as iw4L's DObj sees it: the viewhands model has a nulled trans array (OAT xmodel JSON
    // "type": "viewhands", asserted by convert_viewmodel 1m); the gun ("animated") has its bind offsets, and its
    // root j_gun is attached with zero translation (dobj.rs root bones).
    let model_trans: Vec<[f32; 3]> = (0..n).map(|i| if i > gun0 { a.skel.bones[i].t } else { [0.0; 3] }).collect();
    let mut skinned = vec![false; n];
    for v in 0..a.mesh.vertex_count {
        let vx = a.mesh.vertex(v).unwrap();
        for (j, w) in vx.joints.iter().zip(vx.weights) {
            if w > 0 {
                skinned[*j as usize] = true;
            }
        }
    }
    let view = a.skel.tags.view as usize;
    let mut s = PoseScratch::new(n);
    let (mut worst, mut worst_at) = (0.0f32, String::new());
    let mut samples = 0usize;
    let mut tag_view_local = None;
    // the first M3 reading (gun tracks absolute) for comparison: how far off it was
    let mut old_worst = 0.0f32;
    for id in [ClipId::Reload, ClipId::ReloadEmpty, ClipId::Fire, ClipId::SprintLoop, ClipId::Pullout, ClipId::Idle] {
        let d = std::fs::read(xanim_dir(&root).join(format!("viewmodel_ump45_{}", id.name()))).unwrap();
        let x = read_xanim(&d).unwrap();
        let c = a.clip(id);
        let dur = c.numframes as f32 / c.fps as f32;
        for k in 0..=8 {
            let t = dur * k as f32 / 8.0;
            let frame = iw4l::time_to_frame(t, x.fps as f32, x.numframes, x.looped);
            let mut lq: Vec<iw4l::Quat> = a.skel.bones.iter().map(|b| b.q).collect();
            let mut lt: Vec<iw4l::Vec3> = model_trans.clone();
            let mut lt_old: Vec<iw4l::Vec3> = a.skel.bones.iter().map(|b| b.t).collect();
            for xt in &x.tracks {
                let Some(i) = names.iter().position(|nm| *nm == xt.name) else { continue };
                let rot = if xt.rot.is_empty() {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    let (p, q, u) = iw4l::span(&xt.rot_frames, xt.rot.len(), frame);
                    iw4l::lerp4(iw4l::full_quat(xt.rot[p]), iw4l::full_quat(xt.rot[q]), u)
                };
                let tr = if xt.trans.is_empty() {
                    [0.0; 3]
                } else {
                    let (p, q, u) = iw4l::span(&xt.trans_frames, xt.trans.len(), frame);
                    iw4l::lerp3(xt.trans[p], xt.trans[q], u)
                };
                lq[i] = iw4l::normalize(rot);
                lt[i] = iw4l::compose_translation(model_trans[i], tr);
                if !xt.trans.is_empty() {
                    lt_old[i] = tr;
                }
            }
            if tag_view_local.is_none() {
                tag_view_local = Some(lt[view]);
            }
            let fk = |lt: &[iw4l::Vec3]| {
                let mut w = vec![[0.0f32; 12]; n];
                for (i, b) in a.skel.bones.iter().enumerate() {
                    let local = mw2er_core::xform::m_trs(&lt[i], &lq[i]);
                    w[i] = if b.parent >= 0 { m_mul(&w[b.parent as usize], &local) } else { local };
                }
                w
            };
            let (w, w_old) = (fk(&lt), fk(&lt_old));
            let (st, now) = static_state(id, t, 0.0);
            evaluate(&a, &st, now, &mut s);
            let (vi, vo, vold) = (m_inv_rigid(&w[view]), m_inv_rigid(&s.world[view]), m_inv_rigid(&w_old[view]));
            for i in (0..n).filter(|&i| skinned[i]) {
                let pi = m_translation(&m_mul(&vi, &w[i]));
                let po = m_translation(&m_mul(&vo, &s.world[i]));
                let pold = m_translation(&m_mul(&vold, &w_old[i]));
                let d = (0..3).map(|c| (pi[c] - po[c]).abs()).fold(0.0f32, f32::max);
                old_worst = old_worst.max((0..3).map(|c| (pi[c] - pold[c]).abs()).fold(0.0f32, f32::max));
                samples += 1;
                if d > worst {
                    worst = d;
                    worst_at = format!("{} t={t:.3} {} iw4l={pi:.3?} ours={po:.3?}", id.name(), names[i]);
                }
            }
        }
    }
    assert!(worst <= 0.01, "eye-relative skinned-bone positions differ from the iw4L composition by {worst} in at {worst_at}");
    assert!(old_worst > 1.0, "the first M3 reading should have differed (gun tracks absolute): {old_worst}");
    let tv = tag_view_local.unwrap();
    println!(
        "PASS: iw4L composition (sampling + XModel.trans + anim, ported test-only) == runtime pose for every skinned \
         bone relative to tag_view: 6 clips x 9 times, {samples} bone samples, max |d| {worst:.2e} in (the first M3 \
         reading, gun tracks absolute, was off by up to {old_worst:.2} in); known difference: tag_view is uncontrolled, \
         local t iw4L {tv:?} vs ours {:?}, which moves the eye and everything under it together (no eye-relative effect)",
        a.skel.bones[view].t
    );
}
