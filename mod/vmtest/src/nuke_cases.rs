//! vmtest M7: the nuke post pass offscreen (docs/M7_DESIGN.md 6.3), with the DLL's own `mw2er_render::nuke::NukePass`
//! and shaders, compared with the CPU reference `mw2er_core::nuke_fx` on the real MW2 visions (`converted\nuke`).
//!
//! Every case reproduces hudhook's sequence (target in COMMON -> RENDER_TARGET, record, -> COMMON, read back), with the
//! D3D12 debug layer (and GPU-based validation with --gbv). Cases:
//! N1 black + flash 1 -> white; N2 grey with `mpnuke` full -> orange-white (R >= G >= B, R >= 0.95) == CPU within
//! 2 LSB; N3 the aftermath on grey -> R > G > B == CPU; N4 flash 0 + amount 0 -> output == input bit for bit (8 and
//! 10 bit); N5 the timeline on the test card (T, T+0.25, T+1.5, T+3.25, T+6.5 -> PNGs), R10G10B10A2, a resize and 3
//! frames in flight, no present-thread allocation after prepare; N6 a non-finite constant is refused before anything is
//! recorded. `--nuke-perf`: 3840x2160 R10G10B10A2, p95 < 1.0 ms.

use std::path::Path;

use mw2er_core::nuke_data::NukeFile;
use mw2er_core::nuke_fx::{Vis, constants, uniform, vis_at};
use mw2er_core::nuke_logic::{Plan, PostState, post_at};
use mw2er_render::nuke::{NukePass, NukeTarget};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::thermal_cases::{Frame, decode, image_frame_fmt, levels, make_frame, record_frame_end, record_frame_start, to_png};
use super::{Gpu, Report, barrier, read_rows, write_png};

const NUM_FRAMES: usize = 3;

pub struct NukeArgs<'a> {
    pub out: &'a Path,
    pub nuke: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

fn load(dir: &Path) -> Result<NukeFile, String> {
    let t = std::fs::read_to_string(dir.join("nuke.json")).map_err(|x| format!("{}: {x}", dir.join("nuke.json").display()))?;
    NukeFile::parse(&t)
}

fn target(f: &Frame) -> NukeTarget<'_> {
    NukeTarget { resource: &f.t.res, rtv: f.t.rtv, rtv_format: f.t.fmt, width: f.t.w, height: f.t.h }
}

fn state(f: &NukeFile, flash: f32, amount: f32, blend: f32) -> (Vis, PostState) {
    let p = PostState { flash, amount, blend };
    (vis_at(f, &p), p)
}

fn run(g: &mut Gpu, pass: &mut NukePass, fr: &Frame, v: &Vis, p: &PostState, first: bool) -> Result<Vec<[u32; 3]>, String> {
    g.begin(0)?;
    record_frame_start(g, fr, first);
    let c = constants(v, p, fr.t.w, fr.t.h);
    pass.record(&g.list, &target(fr), 0, &c, false).map_err(|x| format!("record: {x}"))?;
    record_frame_end(g, fr);
    g.submit_wait()?;
    Ok(decode(&read_rows(&fr.t.rb, &fr.t.layout, fr.t.w, fr.t.h, 4)?, fr.t.fmt))
}

fn uniform_frame(g: &Gpu, c: [u8; 3], w: u32, h: u32, fmt: DXGI_FORMAT, rtv: usize) -> Result<Frame, String> {
    image_frame_fmt(g, vec![c; (w * h) as usize], w, h, fmt, rtv)
}

/// Worst difference in LSB of a uniform output against the CPU expectation (and the expectation in levels).
fn uniform_check(out: &[[u32; 3]], fmt: DXGI_FORMAT, src: [u8; 3], v: &Vis, p: &PostState) -> (f32, [f32; 3]) {
    let l = levels(fmt);
    let s = src.map(|x| if fmt == DXGI_FORMAT_R10G10B10A2_UNORM { (x as f32 * 1023.0 / 255.0).round() / l } else { x as f32 / l });
    let e = uniform(s, v, p).map(|x| x * l);
    let worst = out.iter().flat_map(|c| (0..3).map(move |k| (c[k] as f32 - e[k]).abs())).fold(0.0f32, f32::max);
    (worst, e)
}

pub fn run_nuke(a: &NukeArgs) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let nf = load(a.nuke)?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{} [nuke]", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut pass = NukePass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, 1920, 1080).map_err(|x| format!("prepare: {x}"))?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "Ninit", format!("NukePass::new + prepare 1920x1080 R8G8B8A8; InfoQueue errors {} {:?}", errs.len(), errs.first()));
    let (w, h) = (1920u32, 1080u32);
    let fmt8 = DXGI_FORMAT_R8G8B8A8_UNORM;

    // N1 black + flash 1 -> white
    let black = uniform_frame(&g, [0, 0, 0], w, h, fmt8, 0)?;
    let (v, p) = state(&nf, 1.0, 0.0, 0.0);
    let out = run(&mut g, &mut pass, &black, &v, &p, true)?;
    let white = out.iter().all(|c| *c == [255, 255, 255]);
    let allocs = pass.take_present_allocs();
    let errs = g.drain_errors();
    rep.add(white && allocs.is_empty() && errs.is_empty(), "N1", format!("black + flash 1: every pixel 255,255,255={white}; present-thread allocs {allocs:?}; InfoQueue {}", errs.len()));

    // N2 grey with mpnuke full -> orange-white, GPU == CPU
    let grey = uniform_frame(&g, [128, 128, 128], w, h, fmt8, 1)?;
    let (v, p) = state(&nf, 0.0, 1.0, 0.0);
    let out = run(&mut g, &mut pass, &grey, &v, &p, true)?;
    let (worst, e) = uniform_check(&out, fmt8, [128, 128, 128], &v, &p);
    let c = out[(h / 2 * w + w / 2) as usize];
    let orange = c[0] >= c[1] && c[1] >= c[2] && c[0] as f32 >= 0.95 * 255.0 && c[2] + 25 <= c[0];
    write_png(&a.out.join("nuke_n2_mpnuke_grey.png"), &to_png(&out, fmt8), w, h)?;
    let errs = g.drain_errors();
    rep.add(orange && worst <= 2.0 && errs.is_empty(), "N2", format!("grey 128 + mpnuke: centre {c:?} (cpu {:.1},{:.1},{:.1}), worst {worst:.2} LSB (<= 2), R>=G>=B and R>=0.95: {orange}; InfoQueue {}", e[0], e[1], e[2], errs.len()));

    // N3 the aftermath on grey: R > G > B
    let (v, p) = state(&nf, 0.0, 1.0, 1.0);
    let out = run(&mut g, &mut pass, &grey, &v, &p, false)?;
    let (worst, e) = uniform_check(&out, fmt8, [128, 128, 128], &v, &p);
    let c = out[(h / 2 * w + w / 2) as usize];
    let tint = c[0] > c[1] && c[1] > c[2];
    write_png(&a.out.join("nuke_n3_aftermath_grey.png"), &to_png(&out, fmt8), w, h)?;
    let errs = g.drain_errors();
    rep.add(tint && worst <= 2.0 && errs.is_empty(), "N3", format!("grey 128 + mpnuke_aftermath: centre {c:?} (cpu {:.1},{:.1},{:.1}), worst {worst:.2} LSB, R>G>B: {tint}; InfoQueue {}", e[0], e[1], e[2], errs.len()));

    // N4 flash 0 + amount 0 -> output == input (8 and 10 bit), on the test card
    let card8 = make_frame(&g, fmt8, w, h, 2)?;
    let (v, p) = state(&nf, 0.0, 0.0, 0.3);
    let out = run(&mut g, &mut pass, &card8, &v, &p, true)?;
    let src8 = decode(&super::thermal_cases::encode_pub(&card8.src, fmt8), fmt8);
    let diff8 = out.iter().zip(&src8).filter(|(a, b)| a != b).count();
    let fmt10 = DXGI_FORMAT_R10G10B10A2_UNORM;
    pass.prepare(fmt10, w, h).map_err(|x| format!("prepare R10: {x}"))?;
    let card10 = make_frame(&g, fmt10, w, h, 3)?;
    let out10 = run(&mut g, &mut pass, &card10, &v, &p, true)?;
    let src10 = decode(&super::thermal_cases::encode_pub(&card10.src, fmt10), fmt10);
    let diff10 = out10.iter().zip(&src10).filter(|(a, b)| a != b).count();
    let errs = g.drain_errors();
    rep.add(diff8 == 0 && diff10 == 0 && errs.is_empty(), "N4", format!("flash 0 + amount 0: pixels changed R8G8B8A8 {diff8}, R10G10B10A2 {diff10} (want 0); InfoQueue {}", errs.len()));

    // N5 the timeline on the card (PNGs to look at), R10, a resize to 4K and back with 3 frames in flight
    let cfg = mw2er_core::config::Config::default();
    let plan = Plan::new(&nf.timeline, &cfg.nuke);
    let mut t5 = Vec::new();
    for (name, dt) in [("t0", 0.0f64), ("t0_25", 0.25), ("t1_5", 1.5), ("t3_25", 3.25), ("t6_5", 6.5)] {
        let p = post_at(dt, &plan);
        let v = vis_at(&nf, &p);
        let out = run(&mut g, &mut pass, &card8, &v, &p, false)?;
        write_png(&a.out.join(format!("nuke_n5_{name}.png")), &to_png(&out, fmt8), w, h)?;
        let mean: f64 = out.iter().map(|c| (c[0] + c[1] + c[2]) as f64 / 765.0).sum::<f64>() / out.len() as f64;
        t5.push(format!("{name}: flash {:.2} amount {:.2} blend {:.2} mean {mean:.3}", p.flash, p.amount, p.blend));
    }
    let _ = pass.take_present_allocs(); // the timeline above re-created the pair for R8G8B8A8 (after N4's R10)
    let f4 = make_frame(&g, fmt8, 3840, 2160, 4)?;
    let (v, p) = state(&nf, 0.4, 0.8, 0.5);
    let mut fences = Vec::new();
    for (k, (fr, first)) in [(&card8, false), (&f4, true), (&card8, false)].into_iter().enumerate() {
        g.begin(1 + k)?;
        record_frame_start(&g, fr, first);
        let c = constants(&v, &p, fr.t.w, fr.t.h);
        pass.record(&g.list, &target(fr), k % NUM_FRAMES, &c, true).map_err(|x| format!("N5 record {k}: {x}"))?;
        record_frame_end(&g, fr);
        fences.push(g.submit()?);
    }
    for f in fences {
        g.wait(f)?;
    }
    let allocs = pass.take_present_allocs();
    let errs = g.drain_errors();
    rep.add(errs.is_empty() && allocs.len() == 2, "N5", format!("{}; resize 1080p -> 4K -> 1080p with 3 frames in flight: present allocs {} (the 2 resizes) {:?}; InfoQueue {}", t5.join(" | "), allocs.len(), allocs, errs.len()));

    // N6 failure path: a non-finite constant is refused before anything is recorded
    g.begin(0)?;
    let mut c = constants(&v, &p, w, h);
    c[11] = f32::NAN;
    let refused = pass.record(&g.list, &target(&card8), 0, &c, false).is_err();
    // the list is empty: close + submit must be clean
    g.submit_wait()?;
    let errs = g.drain_errors();
    rep.add(refused && errs.is_empty(), "N6", format!("NaN constant refused before recording: {refused}; InfoQueue {}", errs.len()));

    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "infoqueue", format!("final InfoQueue errors/warnings {}", errs.len()));
    let ok = rep.fails == 0;
    println!("VMTEST NUKE {} ({} cases, {} failed)", if ok { "PASS" } else { "FAIL" }, rep.lines.len(), rep.fails);
    Ok(ok)
}

pub fn run_nuke_perf(a: &NukeArgs) -> Result<bool, String> {
    let nf = load(a.nuke)?;
    let mut g = Gpu::new(false, false, a.warp)?;
    if g.software {
        println!("NUKE PERF FAIL perf needs hardware (adapter {} is software/WARP)", g.adapter);
        return Ok(false);
    }
    let (w, h) = (3840u32, 2160u32);
    let fmt = DXGI_FORMAT_R10G10B10A2_UNORM;
    let mut pass = NukePass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.prepare(fmt, w, h).map_err(|x| x.to_string())?;
    let f = make_frame(&g, fmt, w, h, 0)?;
    let (v, p) = state(&nf, 0.5, 1.0, 0.5);
    let c = constants(&v, &p, w, h);
    let mut samples = Vec::new();
    for k in 0..320u64 {
        g.begin(0)?;
        record_frame_start(&g, &f, k == 0);
        pass.record(&g.list, &target(&f), 0, &c, true).map_err(|x| x.to_string())?;
        // SAFETY: recording into our open list.
        unsafe { g.list.ResourceBarrier(&[barrier(&f.t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON)]) };
        g.submit_wait()?;
        pass.collect(0);
        let s = pass.take_timings();
        if k >= 20 {
            samples.extend(s);
        }
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    if samples.len() < 290 {
        println!("NUKE PERF FAIL only {} timestamp samples", samples.len());
        return Ok(false);
    }
    let q = |f: f64| samples[((samples.len() - 1) as f64 * f).round() as usize];
    let (p50, p95, max) = (q(0.5), q(0.95), samples[samples.len() - 1]);
    let ok = p95 < 1.0 && p50 < 1.0;
    std::fs::create_dir_all(a.out).ok();
    let j = format!(
        "{{\"adapter\": \"{}\", \"pass\": \"nuke\", \"format\": \"R10G10B10A2_UNORM\", \"width\": {w}, \"height\": {h}, \"frames\": {}, \"median_ms\": {p50:.4}, \"p95_ms\": {p95:.4}, \"max_ms\": {max:.4}, \"pass_ok\": {ok}}}\n",
        g.adapter.replace('"', "'"),
        samples.len()
    );
    std::fs::write(a.out.join("perf_nuke.json"), j).map_err(|x| x.to_string())?;
    println!(
        "NUKE PERF {} {} @{w}x{h} R10G10B10A2: {} frames, median {p50:.4} ms, p95 {p95:.4} ms, max {max:.4} ms (need p95 < 1.0)",
        if ok { "PASS" } else { "FAIL" },
        g.adapter,
        samples.len()
    );
    Ok(ok)
}
