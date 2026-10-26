//! C6 depth fallback vmtest cases (docs/C6_HOLD_DESIGN.md 13.2): the DLL's own `Renderer::record_world_occluded` with
//! the Striker's world model and depth-only body capsules in OUR world-gun depth.
//!
//!   vmtest --occluders [--gbv] [--warp] [--out <dir>]
//!       OC0 no capsules: bit-identical to `record_world` (the flag-off path), gun pixels drawn
//!       OC1 a capsule between the camera and the gun covering it: no gun pixel drawn
//!       OC2 the same capsule behind the gun: bit-identical to OC0 (a capsule only hides what is behind it)
//!       OC3 a capsule covering the gun's left part: fewer gun pixels; every pixel right of the capsule's screen edge
//!           identical to OC0
//!       OC4 a renderer whose occluder pass was never prepared: the capsules are ignored (bit-identical to its OC0)
//!       zero debug-layer messages. Images: `<out>\occl_<case>.png`. Exit 0 = PASS.

use std::path::Path;

use mw2er_core::body_occl::Capsule;
use mw2er_core::vec3::V3;
use mw2er_core::vm_assets::{TextureCache, VmAssets};
use mw2er_render::{LightParams, OutputMode, Renderer, Target, Variant, WorldDraw, WorldFrame};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{Gpu, H1, KEY, KEY_F, NUM_FRAMES, Report, Tgt, W1, new_target, project_root, read_rows, record_copy, write_png};

pub struct OcclArgs<'a> {
    pub out: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

/// The test camera: at (0, 1.5, -3) looking +z (ER rows right, up, forward), 50 deg vertical.
fn frame_of() -> Result<WorldFrame, String> {
    let rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (view, view_proj) = mw2er_core::coop_vis::world_view_proj([0.0, 1.5, -3.0], rows, 50f32.to_radians(), W1, H1, 0.05, 3000.0).ok_or("view_proj")?;
    Ok(WorldFrame { view, view_proj, light: LightParams::default(), out: OutputMode::Srgb, paper_white_nits: 200.0, variant: Variant::default() })
}

fn draw(g: &mut Gpu, r: &mut Renderer, t: &Tgt, caps: Option<&[Capsule]>) -> Result<(Vec<u8>, u32, u32), String> {
    let f = frame_of()?;
    // the gun 3 m ahead, crosswise (its length across the screen)
    let m = mw2er_core::coop_vis::world_gun_matrix(V3::new(0.0, 1.5, 0.0), V3::new(1.0, 0.0, 0.0), [0.0, 0.0, 0.0], 0.0).ok_or("gun matrix")?;
    let draws = [WorldDraw { gun: 0, lod: 0, model: m }];
    g.begin(0)?;
    // SAFETY: recording into our open list; the target is in RENDER_TARGET state.
    unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
    let target = Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h };
    let (n, nc) = match caps {
        None => (r.record_world(&g.list, &target, 0, &draws, &f).map_err(|x| format!("record_world: {x}"))?, 0),
        Some(c) => r.record_world_occluded(&g.list, &target, 0, &draws, &f, c, None).map_err(|x| format!("record_world_occluded: {x}"))?,
    };
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    Ok((read_rows(&t.rb, &t.layout, t.w, t.h, 4)?, n, nc))
}

fn gun_px(p: &[u8]) -> usize {
    p.chunks(4).filter(|c| c[..3] != KEY[..3]).count()
}

fn renderer(g: &Gpu, occl: bool) -> Result<Renderer, String> {
    let dir = project_root().join("converted").join("guns").join("striker_mp");
    let mut cache = TextureCache::new();
    let list = VmAssets::load_gun_world(&dir, &mut cache)?;
    let w = list.iter().find(|w| w.lod == 0).ok_or("striker_mp has no world_lod0")?;
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, 0).map_err(|x| x.to_string())?;
    r.upload_world(0, 0, &w.mesh, &w.textures).map_err(|x| format!("upload_world: {x}"))?;
    r.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, Variant::default(), W1, H1).map_err(|x| format!("prepare: {x}"))?;
    if occl {
        r.prepare_occluders().map_err(|x| format!("prepare_occluders: {x}"))?;
    }
    Ok(r)
}

pub fn run_occl(a: &OcclArgs<'_>) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut r = renderer(&g, true)?;
    let t = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W1, H1, 0)?;
    let _ = g.drain_errors();
    let png = |name: &str, p: &[u8]| {
        let rgba: Vec<u8> = p.chunks(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        let _ = write_png(&a.out.join(format!("occl_{name}.png")), &rgba, W1, H1);
    };

    let (base, n0, _) = draw(&mut g, &mut r, &t, None)?;
    let (none, n1, c1) = draw(&mut g, &mut r, &t, Some(&[]))?;
    let gun = gun_px(&base);
    png("OC0", &base);
    rep.add(none == base && gun > 1000 && n0 == 1 && n1 == 1 && c1 == 0, "OC0", format!("no capsules: identical to record_world {} ({gun} gun px, guns {n0}/{n1}, capsules {c1})", none == base));

    // OC1: between the camera (z = -3) and the gun (z = 0), wide and tall enough to cover it
    let front = Capsule { a: [-1.2, 1.5, -1.0], b: [1.2, 1.5, -1.0], r: 0.5 };
    let (p1, _, nc) = draw(&mut g, &mut r, &t, Some(&[front]))?;
    png("OC1", &p1);
    rep.add(gun_px(&p1) == 0 && nc == 1, "OC1", format!("a capsule in front: {} of {gun} gun px drawn (capsules {nc})", gun_px(&p1)));

    // OC2: the same capsule behind the gun
    let behind = Capsule { a: [-1.2, 1.5, 1.0], b: [1.2, 1.5, 1.0], r: 0.5 };
    let (p2, _, nc2) = draw(&mut g, &mut r, &t, Some(&[behind]))?;
    rep.add(p2 == base && nc2 == 1, "OC2", format!("a capsule behind the gun: identical to OC0 {}", p2 == base));

    // OC3: in front, covering x < -0.1 m at z = -1 (capsule surface): screen x < centre - (0.1 / 2 m) * focal
    let left = Capsule { a: [-3.0, 1.5, -1.0], b: [-0.6, 1.5, -1.0], r: 0.5 };
    let (p3, _, _) = draw(&mut g, &mut r, &t, Some(&[left]))?;
    png("OC3", &p3);
    let focal = (H1 as f32 * 0.5) / (25f32.to_radians().tan());
    let edge = (W1 as f32 * 0.5 + (-0.1 / 2.0) * focal).ceil() as u32 + 2;
    let mut diff_right = 0usize;
    let mut gun_right = 0usize;
    for y in 0..H1 {
        for x in edge..W1 {
            let i = ((y * W1 + x) * 4) as usize;
            gun_right += (base[i..i + 3] != KEY[..3]) as usize;
            diff_right += (base[i..i + 4] != p3[i..i + 4]) as usize;
        }
    }
    let g3 = gun_px(&p3);
    rep.add(
        g3 > 0 && g3 < gun && diff_right == 0 && gun_right > 0,
        "OC3",
        format!("a capsule over the left part: {g3} of {gun} gun px drawn; right of x={edge}: {gun_right} gun px, differing {diff_right}"),
    );

    // OC4: never prepared: capsules ignored
    let mut r2 = renderer(&g, false)?;
    let (b4, _, _) = draw(&mut g, &mut r2, &t, None)?;
    let (p4, _, nc4) = draw(&mut g, &mut r2, &t, Some(&[front]))?;
    rep.add(p4 == b4 && nc4 == 0 && !r2.has_occluders() && r.has_occluders(), "OC4", format!("no occluder pass: capsules ignored, identical {} (capsules {nc4})", p4 == b4));

    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "infoqueue", format!("{} ERROR/CORRUPTION messages{}", errs.len(), errs.first().map(|m| format!(" first: {m}")).unwrap_or_default()));
    std::fs::write(a.out.join("occl_report.txt"), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    let ok = rep.fails == 0;
    println!("{} VMTEST OCCLUDERS {}/{} cases", if ok { "PASS" } else { "FAIL" }, rep.lines.len() - rep.fails, rep.lines.len());
    Ok(ok)
}
