//! vmtest run 8: the AC-130 round tracer pass offscreen, with the DLL's own `mw2er_render::tracer::TracerPass` and
//! shaders and the instances from `mw2er_core::ac130_tracer::build_instances`, compared with the CPU reference
//! `mw2er_core::ac130_tracer::shade`. Same hudhook sequence as the thermal cases (the target holds a frame in COMMON,
//! COMMON -> RENDER_TARGET, the callback, RENDER_TARGET -> COMMON, read back).
//!
//! - TR1 position + shading: a 105 impact flash at a known WORLD point must land at the screen position computed here
//!   by hand from the camera definition (centroid within 0.75 px), and the whole frame (a flash with its bloom and two streaks on
//!   black) must equal the CPU reference within 2 LSB.
//! - TR2 order with the thermal film: tracers recorded before the thermal pass read white in white hot and black in
//!   black hot on the synthetic card (the same pixel without tracers is neither).
//! - TR3 R10G10B10A2 + the empty list (nothing recorded, the frame unchanged) + the failure path (wrong size, too many
//!   instances: `Err` before any command, recorded into a CLOSED list: 0 debug-layer errors).
//! - TR4 the instance ring: 3 frames in one submission, each with its own instances in its own slot.
//! - TRinfoqueue: no ERROR/CORRUPTION message at the end.
//! `--tracers-perf`: 3840x2160 R10G10B10A2, the worst case (a full ring: MAX_INSTANCES = MAX_TRACERS instances, half
//! streaks, half large impact flashes), GPU time p95 < 0.2 ms.

use mw2er_core::ac130_hud::project;
use mw2er_core::ac130_tracer::{
    MAX_INSTANCES, MAX_TRACERS, TracerInstance, TracerParams, TracerRing, TracerShot, TracerStyle, TracerView, build_instances, muzzle, quad_margin,
    shade,
};
use mw2er_core::thermal::{LUMA_709, exposed_luma};
use mw2er_render::thermal::{ThermalParams, ThermalPass};
use mw2er_render::tracer::{TracerPass, TracerTarget};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::thermal_cases::{Args, Frame, NUM_FRAMES, base_params, decode, films, image_frame_fmt, levels, load_overlays, make_frame, record_frame_end, record_frame_start, source_float, target, to_png};
use super::{Gpu, Report, barrier, e, read_rows, write_png};

/// Tracer pass perf limit at 4K (the request: < 0.2 ms).
pub const TRACER_P95_MAX_MS: f32 = 0.2;

/// Camera 100 m above the origin looking straight down: right = +x, up (screen) = +z, forward = -y.
const VIEW_POS: [f32; 3] = [0.0, 100.0, 0.0];
const VIEW_ROWS: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]];
const VFOV: f32 = 0.6;

fn view(w: u32, h: u32) -> TracerView {
    TracerView { pos: VIEW_POS, rows: VIEW_ROWS, vfov: VFOV, width: w as f32, height: h as f32 }
}

fn params() -> TracerParams {
    // the shipped defaults (mw2er.toml [ac130.tracers])
    mw2er_core::config::Ac130Tracers::default().params()
}

fn tt(f: &Frame) -> TracerTarget<'_> {
    TracerTarget { resource: &f.t.res, rtv: f.t.rtv, rtv_format: f.t.fmt, width: f.t.w, height: f.t.h }
}

fn black(w: u32, h: u32) -> Vec<[u8; 3]> {
    vec![[0u8; 3]; (w * h) as usize]
}

/// The scene for TR1/TR3/TR4: a 105 round that landed `flash_age` s ago at `flash_at` (flash), a 40 mm and a 25 mm
/// round in flight, all from the gunship muzzle. Returns (ring, now).
fn scene(flash_at: [f32; 3], flash_age: f64, shift: f32) -> (TracerRing, f64) {
    let now = 100.0;
    let m = muzzle(VIEW_POS, VIEW_ROWS, [-1.5, -2.5, 0.0]);
    let mut r = TracerRing::new();
    r.push(TracerShot { gun: 0, muzzle: m, impact: flash_at, t_fire: now - 2.0 - flash_age, flight_s: 2.0 });
    r.push(TracerShot { gun: 1, muzzle: m, impact: [-20.0 + shift, 0.0, 12.0], t_fire: now - 0.6, flight_s: 1.2 });
    r.push(TracerShot { gun: 2, muzzle: m, impact: [15.0, 0.0, -14.0 + shift], t_fire: now - 0.45, flight_s: 0.6 });
    (r, now)
}

/// CPU expectation (stored levels, float) of `instances` added onto `src`.
fn expect(src: &[[u8; 3]], fmt: DXGI_FORMAT, w: u32, h: u32, instances: &[TracerInstance]) -> Vec<[f32; 3]> {
    let base = source_float(src, fmt);
    let mut acc = vec![0.0f32; (w * h) as usize];
    for t in instances {
        let m = quad_margin(t.half_w);
        let x0 = (t.p0[0].min(t.p1[0]) - m).floor().max(0.0) as u32;
        let x1 = (t.p0[0].max(t.p1[0]) + m).ceil().min(w as f32 - 1.0).max(0.0) as u32;
        let y0 = (t.p0[1].min(t.p1[1]) - m).floor().max(0.0) as u32;
        let y1 = (t.p0[1].max(t.p1[1]) + m).ceil().min(h as f32 - 1.0).max(0.0) as u32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                acc[(y * w + x) as usize] += shade(t, [x as f32 + 0.5, y as f32 + 0.5]);
            }
        }
    }
    let l = levels(fmt);
    base.iter()
        .zip(&acc)
        .map(|(c, a)| [(c[0] + a).clamp(0.0, 1.0) * l, (c[1] + a * 0.97).clamp(0.0, 1.0) * l, (c[2] + a * 0.9).clamp(0.0, 1.0) * l])
        .collect()
}

/// (pixels, beyond `tol` LSB, worst difference) between the GPU levels and the CPU expectation.
fn compare(gpu: &[[u32; 3]], want: &[[f32; 3]], tol: f32) -> (usize, usize, f32) {
    let mut over = 0;
    let mut worst = 0.0f32;
    for (g, w) in gpu.iter().zip(want) {
        let d = (0..3).map(|i| (g[i] as f32 - w[i]).abs()).fold(0.0, f32::max);
        worst = worst.max(d);
        if d > tol {
            over += 1;
        }
    }
    (gpu.len(), over, worst)
}

/// One frame: the source, the tracer pass (slot 0), read back.
fn run_tr(g: &mut Gpu, pass: &mut TracerPass, f: &Frame, inst: &[TracerInstance], first: bool) -> Result<Vec<[u32; 3]>, String> {
    g.begin(0)?;
    record_frame_start(g, f, first);
    pass.record(&g.list, &tt(f), 0, inst, false).map_err(|x| format!("tracer record: {x}"))?;
    record_frame_end(g, f);
    g.submit_wait()?;
    Ok(decode(&read_rows(&f.t.rb, &f.t.layout, f.t.w, f.t.h, 4)?, f.t.fmt))
}

/// Brightness-weighted centroid (pixel centres) of the red channel in a square window.
fn centroid(img: &[[u32; 3]], w: u32, h: u32, c: [f32; 2], r: f32) -> ([f32; 2], u32) {
    let (mut sx, mut sy, mut s, mut peak) = (0.0f64, 0.0f64, 0.0f64, 0u32);
    let x0 = (c[0] - r).floor().max(0.0) as u32;
    let x1 = (c[0] + r).ceil().min(w as f32 - 1.0) as u32;
    let y0 = (c[1] - r).floor().max(0.0) as u32;
    let y1 = (c[1] + r).ceil().min(h as f32 - 1.0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let v = img[(y * w + x) as usize][0];
            peak = peak.max(v);
            sx += (x as f64 + 0.5) * v as f64;
            sy += (y as f64 + 0.5) * v as f64;
            s += v as f64;
        }
    }
    if s <= 0.0 { ([f32::NAN; 2], peak) } else { ([(sx / s) as f32, (sy / s) as f32], peak) }
}

pub fn run_tracers(a: &Args) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{} [tracers]", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut pass = TracerPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.prepare(DXGI_FORMAT_R8G8B8A8_UNORM).map_err(|x| x.to_string())?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "TRinit", format!("TracerPass::new + prepare (shaders {}..); InfoQueue errors {} {:?}", &mw2er_render::tracer::TRACER_PS_SHA256[..12], errs.len(), errs.first()));

    let (w, h) = (1920u32, 1080u32);
    let p = params();
    let mut inst = [TracerInstance::default(); MAX_INSTANCES];

    // TR1: a flash at a known world point lands where the camera definition says; the frame equals the CPU reference
    let world = [3.0f32, 0.0, 2.0];
    let fy = (h as f32 * 0.5) / (VFOV * 0.5).tan();
    let want_px = [w as f32 * 0.5 + world[0] / VIEW_POS[1] * fy, h as f32 * 0.5 - world[2] / VIEW_POS[1] * fy];
    let (ring, now) = scene(world, 0.05, 0.0);
    let n = build_instances(&ring, now, &view(w, h), &p, &mut inst);
    let f1 = image_frame_fmt(&g, black(w, h), w, h, DXGI_FORMAT_R8G8B8A8_UNORM, 0)?;
    let out1 = run_tr(&mut g, &mut pass, &f1, &inst[..n], true)?;
    write_png(&a.out.join("tracer_tr1.png"), &to_png(&out1, f1.t.fmt), w, h)?;
    let flash = inst[..n].iter().find(|t| t.kind > 0.5).copied();
    let cpu_px = project(VIEW_POS, VIEW_ROWS, VFOV, w as f32, h as f32, world).unwrap_or([f32::NAN; 2]);
    let (cen, peak) = flash.map_or(([f32::NAN; 2], 0), |t| centroid(&out1, w, h, want_px, 2.0 * t.half_w));
    let d_cen = ((cen[0] - want_px[0]).powi(2) + (cen[1] - want_px[1]).powi(2)).sqrt();
    let d_cpu = ((cpu_px[0] - want_px[0]).powi(2) + (cpu_px[1] - want_px[1]).powi(2)).sqrt();
    let (np, over, worst) = compare(&out1, &expect(&f1.src, f1.t.fmt, w, h, &inst[..n]), 2.0);
    let errs = g.drain_errors();
    let streaks = inst[..n].iter().filter(|t| t.kind < 0.5).count();
    rep.add(
        // after run 9 the 105 flash carries a bloom: flash + bloom + 2 streaks
        n == 4 && streaks == 2 && d_cen < 0.75 && d_cpu < 0.01 && peak > 200 && over <= np / 1000 && errs.is_empty(),
        "TR1",
        format!(
            "world ({:.1},{:.1},{:.1}) -> expected px ({:.2},{:.2}) by hand; core projection ({:.2},{:.2}); GPU flash centroid ({:.2},{:.2}) off {d_cen:.3} px, peak {peak}; {n} instances ({streaks} streaks); frame vs CPU: {np} px, {over} beyond 2 LSB (worst {worst:.2}); InfoQueue {}",
            world[0], world[1], world[2], want_px[0], want_px[1], cpu_px[0], cpu_px[1], cen[0], cen[1], errs.len()
        ),
    );

    // TR2: tracers before the thermal film: white in white hot, black in black hot (the same pixel without them is
    // neither); the flash sits on the card's dark band (no overlay)
    let ovl = load_overlays(a.ac130)?;
    let (white, blackf) = films(a.ac130)?;
    let mut th = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    th.upload_overlays(&ovl.tex).map_err(|x| format!("upload: {x}"))?;
    let card = make_frame(&g, DXGI_FORMAT_R8G8B8A8_UNORM, w, h, 1)?;
    let spot = [-10.0f32, 0.0, -24.0];
    let (ring2, now2) = scene(spot, 0.02, 0.0);
    let n2 = build_instances(&ring2, now2, &view(w, h), &p, &mut inst);
    let sp = project(VIEW_POS, VIEW_ROWS, VFOV, w as f32, h as f32, spot).ok_or("TR2 spot behind the camera")?;
    let idx = (sp[1] as u32 * w + sp[0] as u32) as usize;
    let luma = exposed_luma(LUMA_709, a.exposure);
    let mut vals = Vec::new();
    for (film, with) in [(white, true), (blackf, true), (white, false), (blackf, false)] {
        let tp = ThermalParams { overlay: None, luma, gamma: a.gamma, ..base_params(film, w, h) };
        g.begin(0)?;
        record_frame_start(&g, &card, vals.is_empty());
        if with {
            pass.record(&g.list, &tt(&card), 1, &inst[..n2], false).map_err(|x| format!("TR2 tracer: {x}"))?;
        }
        th.record(&g.list, &target(&card), 1, &tp).map_err(|x| format!("TR2 thermal: {x}"))?;
        record_frame_end(&g, &card);
        g.submit_wait()?;
        let out = decode(&read_rows(&card.t.rb, &card.t.layout, w, h, 4)?, card.t.fmt);
        if vals.len() < 2 {
            write_png(&a.out.join(if vals.is_empty() { "tracer_tr2_white.png" } else { "tracer_tr2_black.png" }), &to_png(&out, card.t.fmt), w, h)?;
        }
        vals.push(out[idx]);
    }
    let errs = g.drain_errors();
    let top = levels(card.t.fmt) as u32;
    let ok2 = vals[0].iter().all(|&v| v >= top - 2) && vals[1].iter().all(|&v| v <= 2) && vals[2][0] < top - 40 && vals[3][0] > 40 && errs.is_empty();
    rep.add(ok2, "TR2", format!("flash at px ({:.0},{:.0}) through the thermal film: white hot {:?}, black hot {:?}; without tracers {:?} / {:?}; InfoQueue {}", sp[0], sp[1], vals[0], vals[1], vals[2], vals[3], errs.len()));

    // TR3: R10G10B10A2 vs CPU; an empty list leaves the frame unchanged; the failure path records nothing
    let f10 = image_frame_fmt(&g, black(w, h), w, h, DXGI_FORMAT_R10G10B10A2_UNORM, 2)?;
    let n3 = build_instances(&ring, now, &view(w, h), &p, &mut inst);
    let out3 = run_tr(&mut g, &mut pass, &f10, &inst[..n3], true)?;
    let (np3, over3, worst3) = compare(&out3, &expect(&f10.src, f10.t.fmt, w, h, &inst[..n3]), 3.0);
    let card_src = source_float(&card.src, card.t.fmt);
    let out_empty = run_tr(&mut g, &mut pass, &card, &[], false)?;
    let unchanged = out_empty.iter().zip(&card_src).all(|(o, c)| (0..3).all(|i| (o[i] as f32 - c[i] * 255.0).abs() < 0.51));
    g.begin(5)?;
    // SAFETY: closing our own list: any command recorded below would be a debug-layer error.
    e(unsafe { g.list.Close() }, "Close(TR3)")?;
    let wrong = TracerTarget { resource: &f1.t.res, rtv: f1.t.rtv, rtv_format: f1.t.fmt, width: 1280, height: 720 };
    let r1 = pass.record(&g.list, &wrong, 0, &inst[..n3], false);
    let many = [TracerInstance::default(); MAX_INSTANCES + 1];
    let r2 = pass.record(&g.list, &tt(&f1), 0, &many, false);
    let r3 = pass.record(&g.list, &tt(&f1), NUM_FRAMES, &inst[..n3], false);
    let errs = g.drain_errors();
    rep.add(
        over3 <= np3 / 1000 && unchanged && r1.is_err() && r2.is_err() && r3.is_err() && errs.is_empty(),
        "TR3",
        format!(
            "R10G10B10A2 vs CPU: {np3} px, {over3} beyond 3 LSB of 10 bits (worst {worst3:.2}); empty list leaves the frame unchanged: {unchanged}; wrong size -> {:?}; {} instances -> {:?}; slot {NUM_FRAMES} -> {:?}; debug errors from the closed list {}",
            r1.err().map(|x| x.to_string()),
            MAX_INSTANCES + 1,
            r2.err().map(|x| x.to_string()),
            r3.err().map(|x| x.to_string()),
            errs.len()
        ),
    );

    // TR4: three frames recorded into one submission (slots 0, 1, 2) with different instances: each matches its own
    let frames: Vec<Frame> = (0..3).map(|k| image_frame_fmt(&g, black(w, h), w, h, DXGI_FORMAT_R8G8B8A8_UNORM, 6 + k)).collect::<Result<_, _>>()?;
    let mut sets = Vec::new();
    for k in 0..3 {
        let (rk, nk) = scene([3.0 + 8.0 * k as f32, 0.0, 2.0], 0.05, 6.0 * k as f32);
        let mut buf = [TracerInstance::default(); MAX_INSTANCES];
        let c = build_instances(&rk, nk, &view(w, h), &p, &mut buf);
        sets.push(buf[..c].to_vec());
    }
    g.begin(1)?;
    for (k, f) in frames.iter().enumerate() {
        record_frame_start(&g, f, true);
        pass.record(&g.list, &tt(f), k, &sets[k], false).map_err(|x| format!("TR4 record {k}: {x}"))?;
        record_frame_end(&g, f);
    }
    g.submit_wait()?;
    let mut t4 = Vec::new();
    let mut ok4 = true;
    for (k, f) in frames.iter().enumerate() {
        let out = decode(&read_rows(&f.t.rb, &f.t.layout, w, h, 4)?, f.t.fmt);
        let (np, over, worst) = compare(&out, &expect(&f.src, f.t.fmt, w, h, &sets[k]), 2.0);
        ok4 &= over <= np / 1000;
        t4.push(format!("slot {k}: {over} beyond 2 LSB (worst {worst:.2})"));
    }
    let errs = g.drain_errors();
    rep.add(ok4 && errs.is_empty(), "TR4", format!("3 frames in one submission: {}; InfoQueue {}", t4.join(", "), errs.len()));

    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "TRinfoqueue", format!("{} ERROR/CORRUPTION messages at the end", errs.len()));
    std::fs::write(a.out.join(if a.gbv { "tracer_report_gbv.txt" } else { "tracer_report.txt" }), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    println!("TRACERS {} ({} FAIL)", if rep.fails == 0 { "PASS" } else { "FAIL" }, rep.fails);
    Ok(rep.fails == 0)
}

/// TRperf: 3840x2160 R10G10B10A2, the worst case every frame: a full ring (MAX_INSTANCES quads), half streaks across
/// the screen, half 105-size impact flashes, 320 frames (first 20 dropped), hardware, no debug layer.
pub fn run_tracers_perf(a: &Args) -> Result<bool, String> {
    let mut g = Gpu::new(false, false, a.warp)?;
    if g.software {
        println!("TRACERS PERF FAIL perf needs hardware (adapter {} is software/WARP)", g.adapter);
        return Ok(false);
    }
    let (w, h) = (3840u32, 2160u32);
    let mut pass = TracerPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.prepare(DXGI_FORMAT_R10G10B10A2_UNORM).map_err(|x| x.to_string())?;
    let f = make_frame(&g, DXGI_FORMAT_R10G10B10A2_UNORM, w, h, 0)?;
    // worst case: big streaks + big flashes (wider than the defaults) all on screen
    let heavy = TracerParams {
        styles: [TracerStyle::basic(1.2, 14.0, 1.0, 6.0); 3],
        ..params()
    };
    let m = muzzle(VIEW_POS, VIEW_ROWS, [-1.5, -2.5, 0.0]);
    let mut samples = Vec::new();
    let mut inst = [TracerInstance::default(); MAX_INSTANCES];
    let mut max_n = 0;
    for k in 0..320u64 {
        let now = 50.0 + k as f64 / 60.0;
        let mut r = TracerRing::new();
        for i in 0..MAX_TRACERS {
            let ang = i as f32 * 0.37 + k as f32 * 0.01;
            let imp = [25.0 * ang.cos(), 0.0, 14.0 * ang.sin()];
            // half in flight (streaks), half just landed (flashes)
            let landed = i % 2 == 0;
            r.push(TracerShot { gun: 2, muzzle: m, impact: imp, t_fire: if landed { now - 0.62 } else { now - 0.3 }, flight_s: 0.6 });
        }
        let n = build_instances(&r, now, &view(w, h), &heavy, &mut inst);
        max_n = max_n.max(n);
        g.begin(0)?;
        record_frame_start(&g, &f, k == 0);
        pass.record(&g.list, &tt(&f), 0, &inst[..n], true).map_err(|x| x.to_string())?;
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
        println!("TRACERS PERF FAIL only {} timestamp samples", samples.len());
        return Ok(false);
    }
    let q = |f: f64| samples[((samples.len() - 1) as f64 * f).round() as usize];
    let (p50, p95, max) = (q(0.5), q(0.95), samples[samples.len() - 1]);
    let ok = p95 < TRACER_P95_MAX_MS;
    std::fs::create_dir_all(a.out).ok();
    let j = format!(
        "{{\"adapter\": \"{}\", \"pass\": \"tracers\", \"format\": \"R10G10B10A2_UNORM\", \"width\": {w}, \"height\": {h}, \"instances\": {max_n}, \"frames\": {}, \"median_ms\": {p50:.4}, \"p95_ms\": {p95:.4}, \"max_ms\": {max:.4}, \"pass_ok\": {ok}}}\n",
        g.adapter.replace('"', "'"),
        samples.len()
    );
    std::fs::write(a.out.join("perf_tracers.json"), j).map_err(|x| x.to_string())?;
    println!(
        "TRACERS PERF {} {} @{w}x{h} R10G10B10A2: {max_n} instances, {} frames, median {p50:.4} ms, p95 {p95:.4} ms, max {max:.4} ms (need p95 < {TRACER_P95_MAX_MS})",
        if ok { "PASS" } else { "FAIL" },
        g.adapter,
        samples.len()
    );
    Ok(ok)
}
