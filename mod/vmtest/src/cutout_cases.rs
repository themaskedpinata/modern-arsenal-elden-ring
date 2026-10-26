//! C5 vmtest cases (`[ui_yield] mode = "cutout"`): the viewmodel through the DLL's own `Renderer::record` with
//! `Renderer::set_cutout`, as the Present path draws it while ER shows a message.
//!
//!   vmtest --cutout [--gbv] [--warp] [--out <dir>] [--assets <dir>]
//!       CU0 Cutout::NONE and a region at strength 0: bit-identical to a frame without a cutout (the zero-cost path)
//!       CU1 a full-screen region: nothing drawn (every pixel keeps the clear colour)
//!       CU2 the left half of the gun's box, no feather, R8G8B8A8_UNORM: every pixel inside untouched, every pixel
//!           outside identical to the uncut frame
//!       CU3 the same with a feather: inside untouched, beyond the feather identical, the band partly drawn and
//!           thinning towards the region (dithered)
//!       CU4 strength 0.5 (a fade): about half of the gun's pixels inside drawn, outside identical
//!       CU5 CU2 on R10G10B10A2_UNORM with the PQ output (the user's HDR swap chain)
//!       zero debug-layer messages. Images: `<out>\cutout_<case>.png`. Exit 0 = PASS.

use std::path::Path;

use mw2er_core::vm_pose::{PoseScratch, TEST_POSES};
use mw2er_render::{Cutout, OutputMode, Renderer, Variant};
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{Gpu, H1, NUM_FRAMES, Opts, Report, Scene, Tgt, W1, load, new_target, render_one, write_png};

pub struct CutoutArgs<'a> {
    pub out: &'a Path,
    pub assets: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

fn px(p: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W1 + x) * 4) as usize;
    [p[i], p[i + 1], p[i + 2], p[i + 3]]
}

fn frame(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t: &Tgt, out: OutputMode, cut: Cutout) -> Result<Vec<u8>, String> {
    r.set_cutout(cut);
    let px = render_one(g, r, sc, t, &TEST_POSES[0], &Opts { out, ..Opts::default() }, 0);
    r.set_cutout(Cutout::NONE);
    px
}

fn one(rect: [f32; 4], strength: f32, feather: f32) -> Cutout {
    let mut c = Cutout { n: 1, feather, ..Cutout::NONE };
    c.rects[0] = rect;
    c.strength[0] = strength;
    c
}

/// Pixel counts of a cut frame against the uncut one, for the region `[x0, x1) x [y0, y1)` (pixels) grown by `band` px.
#[derive(Debug, Default)]
struct Tally {
    /// gun pixels of the uncut frame inside the region
    gun_in: usize,
    /// pixels inside the region that differ from the clear colour (must be 0 at strength 1)
    touched_in: usize,
    /// gun pixels inside the region still drawn (strength < 1)
    kept_in: usize,
    /// pixels beyond the band that differ from the uncut frame (must be 0)
    diff_out: usize,
    /// gun pixels compared beyond the band
    gun_out: usize,
    /// gun pixels in the inner / outer half of the band, and how many of them are still drawn
    band: [(usize, usize); 2],
}

fn tally(base: &[u8], cut: &[u8], key: [u8; 4], (x0, y0, x1, y1): (u32, u32, u32, u32), band: u32) -> Tally {
    let mut t = Tally::default();
    for y in 0..H1 {
        for x in 0..W1 {
            let (b, c) = (px(base, x, y), px(cut, x, y));
            let gun = b != key;
            let inside = x >= x0 && x < x1 && y >= y0 && y < y1;
            // distance outside the region (pixels, Chebyshev like the shader's min(d.x, d.y))
            let dx = if x < x0 { x0 - x } else if x >= x1 { x + 1 - x1 } else { 0 };
            let dy = if y < y0 { y0 - y } else if y >= y1 { y + 1 - y1 } else { 0 };
            let d = dx.max(dy);
            if inside {
                t.gun_in += gun as usize;
                t.touched_in += (c != key) as usize;
                t.kept_in += (gun && c == b) as usize;
            } else if d > band + 1 {
                t.diff_out += (c != b) as usize;
                t.gun_out += gun as usize;
            } else if band > 0 && gun && d <= band {
                let h = if d <= band / 2 { 0 } else { 1 };
                t.band[h].0 += 1;
                t.band[h].1 += (c != key) as usize;
            }
        }
    }
    t
}

fn to_png(p: &[u8], fmt: DXGI_FORMAT) -> Vec<u8> {
    match fmt {
        DXGI_FORMAT_R10G10B10A2_UNORM => p
            .chunks(4)
            .flat_map(|c| {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                [(v & 0x3ff) >> 2, ((v >> 10) & 0x3ff) >> 2, ((v >> 20) & 0x3ff) >> 2, 255].map(|x| x as u8)
            })
            .collect(),
        _ => p.chunks(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
    }
}

pub fn run_cutout(a: &CutoutArgs<'_>) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let assets = load(a.assets)?;
    let bones = assets.skel.bones.len();
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, 0).map_err(|x| x.to_string())?;
    r.upload(&assets).map_err(|x| format!("upload: {x}"))?;
    r.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, Variant::default(), W1, H1).map_err(|x| format!("prepare: {x}"))?;
    let mut sc = Scene { assets, scratch: PoseScratch::new(bones) };
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let _ = g.drain_errors();

    for (fmt, out, rtv, cases) in [
        (DXGI_FORMAT_R8G8B8A8_UNORM, OutputMode::Srgb, 0usize, ["CU0", "CU1", "CU2", "CU3", "CU4"].as_slice()),
        (DXGI_FORMAT_R10G10B10A2_UNORM, OutputMode::Pq, 1usize, ["CU5"].as_slice()),
    ] {
        let t = new_target(&g, fmt, W1, H1, rtv)?;
        let fname = mw2er_render::format_name(fmt);
        let base = frame(&mut g, &mut r, &mut sc, &t, out, Cutout::NONE)?;
        // CU1 first: a full-screen region draws nothing, so its pixels are the clear colour (the key for the rest)
        let empty = frame(&mut g, &mut r, &mut sc, &t, out, one([0.0, 0.0, 1.0, 1.0], 1.0, 0.0))?;
        let key = px(&empty, 0, 0);
        let all_key = empty.chunks(4).all(|c| c == key);
        if cases.contains(&"CU1") {
            rep.add(all_key, "CU1", format!("{fname}: a full-screen region at strength 1 leaves every pixel at the clear colour: {all_key}"));
        }
        if cases.contains(&"CU0") {
            let none = frame(&mut g, &mut r, &mut sc, &t, out, Cutout::NONE)?;
            let zero = frame(&mut g, &mut r, &mut sc, &t, out, one([0.0, 0.0, 1.0, 1.0], 0.0, 0.05))?;
            let gun = base.chunks(4).filter(|c| *c != key).count();
            rep.add(none == base && zero == base && gun > 0, "CU0", format!("{fname}: NONE and strength 0 bit-identical to the uncut frame: {} / {} ({gun} gun px)", none == base, zero == base));
        }
        // the gun's box in the uncut frame; the region = its left half
        let (mut bx0, mut by0, mut bx1, mut by1) = (W1, H1, 0, 0);
        for y in 0..H1 {
            for x in 0..W1 {
                if px(&base, x, y) != key {
                    (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x + 1), by1.max(y + 1));
                }
            }
        }
        if bx1 <= bx0 {
            rep.add(false, cases[0], format!("{fname}: no gun pixels in the uncut frame"));
            continue;
        }
        let (rx0, ry0, rx1, ry1) = (bx0, by0, (bx0 + bx1) / 2, by1);
        let rect = [rx0 as f32 / W1 as f32, ry0 as f32 / H1 as f32, rx1 as f32 / W1 as f32, ry1 as f32 / H1 as f32];
        let reg = (rx0, ry0, rx1, ry1);
        for case in cases.iter().filter(|c| matches!(**c, "CU2" | "CU5")) {
            let cut = frame(&mut g, &mut r, &mut sc, &t, out, one(rect, 1.0, 0.0))?;
            let tl = tally(&base, &cut, key, reg, 0);
            let _ = write_png(&a.out.join(format!("cutout_{case}.png")), &to_png(&cut, fmt), W1, H1);
            let ok = tl.gun_in > 0 && tl.touched_in == 0 && tl.diff_out == 0 && tl.gun_out > 0;
            rep.add(ok, case, format!("{fname}: region ({rx0},{ry0})-({rx1},{ry1}) over {} gun px: touched inside {}; outside {} gun px, differing {}", tl.gun_in, tl.touched_in, tl.gun_out, tl.diff_out));
        }
        if cases.contains(&"CU3") {
            let feather = 0.02f32;
            let band = (feather * H1 as f32).ceil() as u32;
            let cut = frame(&mut g, &mut r, &mut sc, &t, out, one(rect, 1.0, feather))?;
            let tl = tally(&base, &cut, key, reg, band);
            let _ = write_png(&a.out.join("cutout_CU3.png"), &to_png(&cut, fmt), W1, H1);
            let frac = |(n, k): (usize, usize)| if n == 0 { f64::NAN } else { k as f64 / n as f64 };
            let (inner, outer) = (frac(tl.band[0]), frac(tl.band[1]));
            let ok = tl.touched_in == 0 && tl.diff_out == 0 && tl.band[0].0 > 0 && tl.band[1].0 > 0 && inner < outer && inner < 0.9 && outer > 0.1;
            rep.add(ok, "CU3", format!("{fname}: feather {band} px: touched inside {}; beyond the band differing {}/{}; band drawn inner {:.2} ({} px) < outer {:.2} ({} px)", tl.touched_in, tl.diff_out, tl.gun_out, inner, tl.band[0].0, outer, tl.band[1].0));
        }
        if cases.contains(&"CU4") {
            let cut = frame(&mut g, &mut r, &mut sc, &t, out, one(rect, 0.5, 0.0))?;
            let tl = tally(&base, &cut, key, reg, 0);
            let kept = tl.kept_in as f64 / tl.gun_in.max(1) as f64;
            let _ = write_png(&a.out.join("cutout_CU4.png"), &to_png(&cut, fmt), W1, H1);
            let ok = tl.gun_in > 0 && (0.3..=0.7).contains(&kept) && tl.diff_out == 0;
            rep.add(ok, "CU4", format!("{fname}: strength 0.5: {:.2} of {} gun px inside drawn (opaque: dithered); outside differing {}", kept, tl.gun_in, tl.diff_out));
        }
    }
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "infoqueue", format!("{} ERROR/CORRUPTION messages{}", errs.len(), errs.first().map(|m| format!(" first: {m}")).unwrap_or_default()));
    std::fs::write(a.out.join("cutout_report.txt"), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    let ok = rep.fails == 0;
    println!("{} VMTEST CUTOUT {}/{} cases", if ok { "PASS" } else { "FAIL" }, rep.lines.len() - rep.fails, rep.lines.len());
    Ok(ok)
}
