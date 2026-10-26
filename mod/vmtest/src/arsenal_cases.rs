//! M6 vmtest cases (docs/M6_DESIGN.md section 12): every arsenal gun through the DLL's multi-gun renderer path.
//!
//!   vmtest --arsenal [--gbv] [--out <dir>] [--guns <dir>]
//!       Loads every gun of `<guns>\index.json` with the shared texture cache, uploads each into its own slot (as the
//!       DLL's loader does), then renders idle / ADS end / reload mid / reload early for every gun at 1920x1080 with the D3D12 debug
//!       layer: `<out>\<gun>\<pose>.png` + `.mask` + `poses.json` (for blender_ref.py --gun). Cases:
//!         AR1 every gun loads + uploads; VRAM of all guns <= 128 MB; InfoQueue clean
//!         AR2 every pose of every gun draws (coverage > 0, < 60 % of the frame) and the silhouette debug mode covers
//!             exactly the same pixels (no clear / blend leakage)
//!         AR3 switching = drawing another slot: rendering every gun in cycle order (twice) uploads nothing
//!         AR4 ADS alignment: on the guns with tag_sight_on the sight projects onto the centre column at ADS end
//!         AR5 the RPG with its rocket part hidden draws fewer pixels than with it
//!         AR6 the scope overlay: black corners, see-through lens centre
//!   vmtest --compare-arsenal <dir>   IoU of `<dir>\<gun>\<pose>.mask` vs Blender's `ref_<pose>.mask` (guns that have
//!                                    reference masks only); same thresholds as the M3 compare (IoU >= 0.90, centroid
//!                                    <= 1 % of the width)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mw2er_core::arsenal::{GunSpec, IndexFile, StatsFile};
use mw2er_core::vm_assets::{ClipId, NO_TAG, PART_ROCKET, TextureCache, VmAssets, parse_tex};
use mw2er_core::vm_pose::{PoseScratch, evaluate, static_state};
use mw2er_core::vm_proj::{ProjParams, build, ndc_to_pixel, project};
use mw2er_core::xform::m_translation;
use mw2er_render::{DebugMode, FrameParams, LightParams, OutputMode, Renderer, Target, Variant};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{Gpu, H1, KEY_F, NUM_FRAMES, Report, Tgt, W1, bpp, mask_of, new_target, read_mask, read_rows, record_copy, write_mask, write_png};

/// The DLL's budget for every gun on the GPU at once (docs/M6_DESIGN.md section 4.3).
pub const VRAM_BUDGET: u64 = 128 * 1024 * 1024;
/// ADS end (informational): the top of the gun in the centre column vs the centre row, as a fraction of the height.
pub const ADS_TOL_FRAC: f32 = 0.03;
/// ADS end: tag_sight_on within this fraction of the width of the centre column.
pub const SIGHT_X_TOL_FRAC: f32 = 0.01;

#[derive(Clone, Debug)]
pub struct GunPose {
    pub name: &'static str,
    pub clip: ClipId,
    pub t: f32,
    pub ads: f32,
}

/// idle, ADS end, reload at half and at a tenth of its native length (the reload clip of a segmented gun is its
/// loop; the AT4 / Stinger / Javelin reloads lower the weapon off screen in the middle).
pub fn gun_poses(a: &VmAssets) -> Vec<GunPose> {
    let rd = a.clip(ClipId::Reload).duration();
    vec![
        GunPose { name: "idle", clip: ClipId::Idle, t: 0.0, ads: 0.0 },
        GunPose { name: "ads", clip: ClipId::Idle, t: 0.0, ads: 1.0 },
        GunPose { name: "reload_mid", clip: ClipId::Reload, t: rd * 0.5, ads: 0.0 },
        GunPose { name: "reload_early", clip: ClipId::Reload, t: rd * 0.1, ads: 0.0 },
    ]
}

pub struct ArsenalArgs<'a> {
    pub out: &'a Path,
    pub guns: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

struct Loaded {
    name: String,
    assets: VmAssets,
    slot: usize,
    spec: Option<GunSpec>,
}

fn scoped(l: &Loaded) -> bool {
    l.spec.as_ref().is_some_and(|s| s.ads.overlay.is_some())
}

fn launcher(l: &Loaded) -> bool {
    l.spec.as_ref().is_some_and(|s| s.is_launcher())
}

fn poses_json(dir: &Path, poses: &[GunPose]) -> Result<(), String> {
    let p = ProjParams::default();
    let rows: Vec<String> = poses
        .iter()
        .map(|t| format!("  {{\"name\": \"{}\", \"clip\": \"{}\", \"t\": {}, \"ads\": {}}}", t.name, t.clip.name(), t.t, t.ads))
        .collect();
    let j = format!(
        "{{\n \"width\": {W1}, \"height\": {H1}, \"hip_fov\": {}, \"ads_fov\": {}, \"near_in\": {}, \"far_in\": {},\n \"poses\": [\n{}\n ]\n}}\n",
        p.hip_fov,
        p.ads_fov,
        p.near_in,
        p.far_in,
        rows.join(",\n")
    );
    std::fs::write(dir.join("poses.json"), j).map_err(|x| x.to_string())
}

/// One simulated hook frame of gun `slot` (as viewmodel.rs records it). Returns the packed pixels + the sight pixel.
fn render_gun(g: &mut Gpu, r: &mut Renderer, l: &Loaded, sc: &mut PoseScratch, t: &Tgt, p: &GunPose, debug: DebugMode, hide: u32) -> Result<(Vec<u8>, Option<[f32; 2]>), String> {
    let (st, now) = static_state(p.clip, p.t.max(0.0), p.ads);
    evaluate(&l.assets, &st, now, sc);
    let tv = sc.world[l.assets.skel.tags.view as usize];
    let vp = build(&tv, t.w, t.h, p.ads, &ProjParams::default());
    let sight = (l.assets.skel.tags.sight_on != NO_TAG)
        .then(|| project(&vp.view_proj, &m_translation(&sc.world[l.assets.skel.tags.sight_on as usize])))
        .flatten()
        .map(|ndc| ndc_to_pixel(ndc, t.w, t.h));
    g.begin(0)?;
    // SAFETY: recording into our open list; the target is in RENDER_TARGET state.
    unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
    let fp = FrameParams {
        palette: &sc.palette,
        view_proj: vp.view_proj,
        view: vp.view,
        light: LightParams::default(),
        out: OutputMode::Srgb,
        paper_white_nits: 200.0,
        debug,
        variant: Variant::default(),
        timing: false,
    };
    r.record_gun(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, 0, l.slot, hide, &fp)
        .map_err(|x| format!("record_gun {}: {x}", l.name))?;
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    Ok((read_rows(&t.rb, &t.layout, t.w, t.h, bpp(t.fmt))?, sight))
}

fn overlay_frame(g: &mut Gpu, r: &mut Renderer, t: &Tgt) -> Result<Vec<u8>, String> {
    g.begin(0)?;
    // SAFETY: as in render_gun.
    unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
    r.record_overlay(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, 1.0)
        .map_err(|x| format!("record_overlay: {x}"))?;
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    read_rows(&t.rb, &t.layout, t.w, t.h, bpp(t.fmt))
}

pub fn run_arsenal(a: &ArsenalArgs<'_>) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let idx_text = std::fs::read_to_string(a.guns.join("index.json")).map_err(|x| format!("index.json: {x}"))?;
    let idx = IndexFile::parse(&idx_text)?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    r.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, Variant::default(), W1, H1).map_err(|x| format!("prepare: {x}"))?;

    // AR1: load + upload every gun into its own slot (the DLL loader's order and cache)
    let t0 = std::time::Instant::now();
    let mut cache = TextureCache::new();
    let mut guns: Vec<Loaded> = Vec::new();
    let mut load_fail = Vec::new();
    let mut overlay_file = None;
    for e in &idx.guns {
        let dir = a.guns.join(&e.name);
        match VmAssets::load_gun_dir(&dir, &mut cache) {
            Ok(assets) => {
                let slot = guns.len();
                if let Err(x) = r.upload_gun(slot, &assets) {
                    load_fail.push(format!("{}: upload {x}", e.name));
                    continue;
                }
                let st = std::fs::read_to_string(dir.join("stats.json")).map_err(|x| x.to_string());
                let spec = st.and_then(|t| StatsFile::parse(&t)).and_then(|f| GunSpec::from_stats(&f)).ok();
                if overlay_file.is_none()
                    && let Some(o) = spec.as_ref().and_then(|s| s.ads.overlay.as_ref())
                {
                    overlay_file = Some(o.file.clone());
                }
                guns.push(Loaded { name: e.name.clone(), assets, slot, spec });
            }
            Err(x) => load_fail.push(format!("{}: {x}", e.name)),
        }
    }
    let load_ms = t0.elapsed().as_millis();
    let overlay_ok = match &overlay_file {
        Some(f) => std::fs::read(a.guns.join(f.replace('/', "\\")))
            .map_err(|x| x.to_string())
            .and_then(|d| parse_tex(&d))
            .and_then(|t| r.upload_overlay(&t).map_err(|x| x.to_string())),
        None => Err("no gun names an overlay".into()),
    };
    let vram = r.gpu_bytes();
    let uploads_after_load = r.upload_count();
    let errs = g.drain_errors();
    rep.add(
        load_fail.is_empty() && !guns.is_empty() && guns.len() == idx.guns.len() && vram <= VRAM_BUDGET && overlay_ok.is_ok() && errs.is_empty(),
        "AR1",
        format!(
            "{} of {} guns loaded + uploaded in {load_ms} ms, shared textures {}, uploads {uploads_after_load}, VRAM {:.1} MB (budget {} MB), overlay {:?}; failures {:?}; InfoQueue errors {} {:?}",
            guns.len(),
            idx.guns.len(),
            cache.len(),
            vram as f64 / (1024.0 * 1024.0),
            VRAM_BUDGET / (1024 * 1024),
            overlay_ok.as_ref().map(|_| "ok").map_err(|x| x.clone()),
            load_fail,
            errs.len(),
            errs.first()
        ),
    );

    let t1 = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W1, H1, 0)?;
    let mut sc = PoseScratch::new(1);
    let total = (W1 * H1) as usize;
    let mut bad_cover = Vec::new();
    let mut bad_d3 = Vec::new();
    let mut ads_rows = Vec::new();
    let mut ads_bad = Vec::new();
    let mut sight_rows = Vec::new();
    let mut offscreen_reloads: Vec<String> = Vec::new();
    let mut summary = Vec::new();
    for l in &guns {
        let gdir = a.out.join(&l.name);
        std::fs::create_dir_all(&gdir).map_err(|x| x.to_string())?;
        let poses = gun_poses(&l.assets);
        poses_json(&gdir, &poses)?;
        let mut cov = BTreeMap::new();
        for p in &poses {
            let (px, sight) = render_gun(&mut g, &mut r, l, &mut sc, &t1, p, DebugMode::None, 0)?;
            let m = mask_of(&px, W1, H1);
            write_png(&gdir.join(format!("{}.png", p.name)), &px, W1, H1)?;
            write_mask(&gdir.join(format!("{}.mask", p.name)), &m, W1, H1)?;
            let (sil, _) = render_gun(&mut g, &mut r, l, &mut sc, &t1, p, DebugMode::Silhouette, 0)?;
            let ms = mask_of(&sil, W1, H1);
            let diff = m.iter().zip(&ms).filter(|(x, y)| x != y).count();
            let c = m.iter().filter(|&&v| v == 1).count();
            // ADS of a scoped gun / a launcher sight may fill the frame (the overlay replaces the sniper view in game);
            // a reload that lowers the weapon off screen at its middle is checked over the whole clip
            let full_ok = p.name == "ads" && (scoped(l) || launcher(l));
            if (c == 0 && !p.name.starts_with("reload")) || (c * 10 > total * 6 && !full_ok) {
                bad_cover.push(format!("{}/{} {c} px", l.name, p.name));
            }
            if c == 0 && p.name == "reload_mid" {
                let d = l.assets.clip(ClipId::Reload).duration();
                let mut seen = Vec::new();
                for k in 1..10 {
                    let q = GunPose { name: "reload_scan", clip: ClipId::Reload, t: d * k as f32 / 10.0, ads: 0.0 };
                    let (px, _) = render_gun(&mut g, &mut r, l, &mut sc, &t1, &q, DebugMode::None, 0)?;
                    seen.push(mask_of(&px, W1, H1).iter().filter(|&&v| v == 1).count());
                }
                println!("gun {:<14} reload_mid is off screen; coverage at 10 %..90 % of the clip: {seen:?}", l.name);
                if seen.iter().all(|&v| v == 0) {
                    bad_cover.push(format!("{}/reload never on screen", l.name));
                }
                offscreen_reloads.push(l.name.clone());
            }
            if p.name == "ads" && !scoped(l) && !launcher(l) {
                // the sight picture: the top of the gun in the centre column (front post / blade) at the crosshair
                let mut top = None;
                for y in 0..H1 {
                    if (W1 / 2 - 5..=W1 / 2 + 5).any(|x| m[(y * W1 + x) as usize] == 1) {
                        top = Some(y);
                        break;
                    }
                }
                let dy = top.map_or(f32::INFINITY, |y| (y as f32 - H1 as f32 / 2.0).abs());
                // informational: an aperture ring or a front-sight guard sits above the post, and the Ranger has no
                // sight (its ADS clip is one frame of tag_ads); the post tips were checked on the PNGs by eye
                ads_rows.push(format!("{} {dy:.0}px{}", l.name, if dy > ADS_TOL_FRAC * H1 as f32 { "*" } else { "" }));
            }
            if diff != 0 {
                bad_d3.push(format!("{}/{} differs on {diff} px", l.name, p.name));
            }
            cov.insert(p.name, c);
            if p.name == "ads"
                && let Some(s) = sight
            {
                // tag_sight_on sits on the rear sight, close to the eye: only its column is meaningful
                let dx = (s[0] - W1 as f32 / 2.0).abs();
                sight_rows.push(format!("{} {dx:.1}px", l.name));
                if dx > SIGHT_X_TOL_FRAC * W1 as f32 {
                    ads_bad.push(format!("{} tag_sight_on at x={:.0} ({dx:.1} px from the centre column)", l.name, s[0]));
                }
            }
        }
        let line = format!(
            "gun {:<14} slot {:>2} bones {:>3} tris {:>5} idle {:>6} ads {:>7} reload_mid {:>6} reload_early {:>6} px",
            l.name,
            l.slot,
            l.assets.skel.bones.len(),
            l.assets.mesh.indices.len() / 3,
            cov["idle"],
            cov["ads"],
            cov["reload_mid"],
            cov["reload_early"]
        );
        println!("{line}");
        summary.push(line);
    }
    let errs = g.drain_errors();
    rep.add(
        bad_cover.is_empty() && bad_d3.is_empty() && errs.is_empty(),
        "AR2",
        format!(
            "{} guns x 4 poses drawn (coverage in (0, 60 %]; scoped / launcher ADS may fill the frame; reload middle off screen, drawn elsewhere in the clip: {offscreen_reloads:?}) {bad_cover:?}; silhouette == colour coverage {bad_d3:?}; InfoQueue errors {} {:?}",
            guns.len(),
            errs.len(),
            errs.first()
        ),
    );

    // AR3: a second pass in cycle order (the switch path draws another slot) uploads nothing
    let before = r.upload_count();
    let pose = GunPose { name: "idle", clip: ClipId::Idle, t: 0.0, ads: 0.0 };
    for l in guns.iter().chain(guns.iter().rev()) {
        render_gun(&mut g, &mut r, l, &mut sc, &t1, &pose, DebugMode::None, 0)?;
    }
    let after = r.upload_count();
    let allocs = r.take_present_allocs();
    rep.add(
        before == after && before == uploads_after_load && allocs.is_empty(),
        "AR3",
        format!("switching through all {} slots forward and back: uploads {before} -> {after} (after load {uploads_after_load}); present-thread allocations {allocs:?}", guns.len()),
    );

    // AR4: ADS alignment
    rep.add(
        ads_bad.is_empty() && !ads_rows.is_empty(),
        "AR4",
        format!(
            "ADS end: tag_sight_on within {3:.0} px of the centre column on {4} guns {5:?}; off: {ads_bad:?} | info: top of the gun in the centre column vs the centre row (* = more than {0:.0} px: a ring / guard above the post) on {1} iron-sight guns {2:?}",
            ADS_TOL_FRAC * H1 as f32,
            ads_rows.len(),
            ads_rows,
            SIGHT_X_TOL_FRAC * W1 as f32,
            sight_rows.len(),
            sight_rows
        ),
    );

    // AR5: the RPG's rocket part can be hidden
    match guns.iter().find(|l| l.name == "rpg_mp") {
        Some(l) if l.assets.mesh.surfaces.iter().any(|s| s.part == PART_ROCKET) => {
            let (with, _) = render_gun(&mut g, &mut r, l, &mut sc, &t1, &pose, DebugMode::None, 0)?;
            let (without, _) = render_gun(&mut g, &mut r, l, &mut sc, &t1, &pose, DebugMode::None, 1 << PART_ROCKET)?;
            let (cw, co) = (mask_of(&with, W1, H1).iter().filter(|&&v| v == 1).count(), mask_of(&without, W1, H1).iter().filter(|&&v| v == 1).count());
            write_png(&a.out.join("rpg_mp").join("idle_rocket_hidden.png"), &without, W1, H1)?;
            rep.add(co < cw && co > 0, "AR5", format!("rpg_mp idle: {cw} px with the rocket, {co} px with part {PART_ROCKET} hidden"));
        }
        _ => rep.add(false, "AR5", "rpg_mp missing or without a rocket part".into()),
    }

    // AR6: the scope overlay (black alpha mask): corners black, lens centre keeps the key colour
    if overlay_ok.is_ok() {
        let px = overlay_frame(&mut g, &mut r, &t1)?;
        write_png(&a.out.join("scope_overlay.png"), &px, W1, H1)?;
        let at = |x: u32, y: u32| {
            let i = ((y * W1 + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2]]
        };
        // a lens point off the reticle lines (the lens is vignetted: close to the key colour) and a screen corner
        let lens = at(W1 / 2 + H1 / 8, H1 / 2 - H1 / 8);
        let corner = at(8, 8);
        let ok = lens[0] >= 200 && lens[1] < 40 && lens[2] >= 200 && corner.iter().all(|&c| c < 16);
        rep.add(ok, "AR6", format!("overlay: lens {lens:?} ~ key colour (see-through), corner {corner:?} black"));
    } else {
        rep.add(false, "AR6", "overlay not uploaded".into());
    }
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "infoqueue", format!("{} ERROR/CORRUPTION messages at the end", errs.len()));
    let mut text = summary;
    text.extend(rep.lines.iter().cloned());
    std::fs::write(a.out.join("arsenal_report.txt"), text.join("\n") + "\n").map_err(|x| x.to_string())?;
    println!("VMTEST ARSENAL {} ({} FAIL)", if rep.fails == 0 { "PASS" } else { "FAIL" }, rep.fails);
    Ok(rep.fails == 0)
}

/// IoU vs Blender (`ref_<pose>.mask` written by blender_ref.py --gun into each gun's folder).
pub fn run_compare_arsenal(dir: &Path) -> Result<bool, String> {
    let mut ok = true;
    let mut n = 0;
    let mut rows = Vec::new();
    let mut gdirs: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|x| x.to_string())?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
    gdirs.sort();
    println!("{:<14} {:<12} {:>8} {:>10} {:>10} {:>9}  verdict", "gun", "pose", "IoU", "vmtest px", "blender px", "centroid");
    for gd in gdirs {
        let gun = gd.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for pose in ["idle", "ads", "reload_mid", "reload_early"] {
            let rp = gd.join(format!("ref_{pose}.mask"));
            if !rp.is_file() {
                continue;
            }
            let (w, h, a) = read_mask(&gd.join(format!("{pose}.mask")))?;
            let (w2, h2, b) = read_mask(&rp)?;
            if (w, h) != (w2, h2) {
                return Err(format!("{gun}/{pose}: sizes differ"));
            }
            let inter = a.iter().zip(&b).filter(|(x, y)| **x == 1 && **y == 1).count();
            let uni = a.iter().zip(&b).filter(|(x, y)| **x == 1 || **y == 1).count();
            if uni == 0 {
                // both renderers agree the weapon is off screen (a launcher reload middle): no IoU, not counted
                println!("{gun:<14} {pose:<12} both empty (weapon off screen in both renderers)");
                continue;
            }
            n += 1;
            let iou = inter as f64 / uni.max(1) as f64;
            let bbox = |m: &[u8]| {
                let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
                for (i, &v) in m.iter().enumerate() {
                    if v == 1 {
                        let (x, y) = (i as u32 % w, i as u32 / w);
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
                ((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0)
            };
            let (ca, cb) = (bbox(&a), bbox(&b));
            let coff = ((ca.0 - cb.0).powi(2) + (ca.1 - cb.1).powi(2)).sqrt() / w as f64;
            let verdict = if iou < 0.90 || coff > 0.01 {
                ok = false;
                "FAIL"
            } else if iou < 0.95 {
                "PASS (WARN < 0.95)"
            } else {
                "PASS"
            };
            let mut diff = vec![0u8; (w * h * 4) as usize];
            for i in 0..(w * h) as usize {
                let c = match (a[i], b[i]) {
                    (1, 1) => [255, 255, 255, 255],
                    (1, 0) => [255, 40, 40, 255],
                    (0, 1) => [40, 90, 255, 255],
                    _ => [0, 0, 0, 255],
                };
                diff[i * 4..i * 4 + 4].copy_from_slice(&c);
            }
            write_png(&gd.join(format!("diff_{pose}.png")), &diff, w, h)?;
            println!(
                "{:<14} {:<12} {:>8.4} {:>10} {:>10} {:>8.3}%  {verdict}",
                gun,
                pose,
                iou,
                a.iter().filter(|&&v| v == 1).count(),
                b.iter().filter(|&&v| v == 1).count(),
                coff * 100.0
            );
            rows.push(format!("{{\"gun\": \"{gun}\", \"pose\": \"{pose}\", \"iou\": {iou:.5}, \"centroid_off\": {coff:.5}}}"));
        }
    }
    std::fs::write(dir.join("iou_arsenal.json"), format!("[{}]\n", rows.join(",\n "))).map_err(|x| x.to_string())?;
    if n == 0 {
        println!("IOU ARSENAL FAIL (no reference masks under {})", dir.display());
        return Ok(false);
    }
    println!("IOU ARSENAL {} ({n} masks)", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}
