//! vmtest M4: the thermal pass offscreen (docs/M4_DESIGN.md section 14.2), with the DLL's own
//! `mw2er_render::thermal::ThermalPass` and shaders, compared with the CPU reference `mw2er_core::thermal`.
//!
//! Every case reproduces hudhook's sequence: the target holds a synthetic "ER frame" (grey ramp, RGB bars, checker,
//! a fake HUD bar) in COMMON (= PRESENT), barrier COMMON -> RENDER_TARGET, `OMSetRenderTargets`, `record`, barrier
//! RENDER_TARGET -> COMMON, read back. Cases: T1 white hot, T2 black hot, T3 overlays 105/40/25, T4 grain, T5 reload
//! blink, T6 resize (3 frames in flight), T7 formats (R8G8B8A8 + R10G10B10A2), T8 state proof (an unbalanced variant
//! must be flagged by the debug layer), T9 failure path (record on a CLOSED list must not record anything).

use std::path::{Path, PathBuf};

use mw2er_core::ac130_data::{OVERLAY_FILES, VisionFile};
use mw2er_core::ac130_hud::{blink_alpha, box43};
use mw2er_core::thermal::{Address, Film, LUMA_709, bilinear, bc3_decode, box_uv, clip_fractions_g, compose_g, exposed_luma, grain_offset, pack};
use mw2er_core::vm_assets::parse_tex;
use mw2er_render::thermal::{ThermalParams, ThermalPass, ThermalTarget};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{Gpu, Report, Tgt, barrier, e, heap, new_target, read_rows, record_copy, weak, write_png};

pub(crate) const NUM_FRAMES: usize = 3;

/// Decoded overlays (RGBA8, 512x512) in shader order 105, 40, 25, grain.
pub(crate) struct Overlays {
    pub(crate) tex: Vec<mw2er_core::vm_assets::Texture>,
    pub(crate) rgba: Vec<Vec<u8>>,
}

pub(crate) fn load_overlays(dir: &Path) -> Result<Overlays, String> {
    let mut tex = Vec::new();
    let mut rgba = Vec::new();
    for f in OVERLAY_FILES {
        let b = std::fs::read(dir.join(f)).map_err(|x| format!("{}: {x}", dir.join(f).display()))?;
        let t = parse_tex(&b).map_err(|x| format!("{f}: {x}"))?;
        rgba.push(bc3_decode(&t.mips[0].data, t.width, t.height)?);
        tex.push(t);
    }
    Ok(Overlays { tex, rgba })
}

pub(crate) fn films(dir: &Path) -> Result<(Film, Film), String> {
    let t = std::fs::read_to_string(dir.join("vision.json")).map_err(|x| format!("vision.json: {x}"))?;
    let v = VisionFile::parse(&t)?;
    Ok((pack(&v.visions["ac130"]), pack(&v.visions["ac130_inverted"])))
}

/// The synthetic "ER frame" as 8-bit RGB.
fn source_rgb(w: u32, h: u32) -> Vec<[u8; 3]> {
    let mut v = vec![[0u8; 3]; (w * h) as usize];
    let bars: [[u8; 3]; 8] = [[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 0, 255], [255, 255, 255]];
    for y in 0..h {
        for x in 0..w {
            let q = y * 4 / h;
            let c = match q {
                0 => {
                    let g = (x * 255 / (w - 1)) as u8;
                    [g, g, g]
                }
                1 => bars[(x * 8 / w) as usize],
                2 => {
                    if ((x / 32) + (y / 32)) % 2 == 0 { [40, 40, 40] } else { [200, 190, 180] }
                }
                _ => {
                    // fake ER HUD: dark band with an HP (red) and FP (blue) bar on the left
                    let (yy, xx) = (y - 3 * h / 4, x);
                    if xx > w / 20 && xx < w / 3 && yy > h / 40 && yy < h / 40 + h / 60 {
                        [200, 30, 30]
                    } else if xx > w / 20 && xx < w / 4 && yy > h / 40 + h / 50 && yy < h / 40 + h / 50 + h / 60 {
                        [40, 60, 200]
                    } else {
                        [20, 22, 24]
                    }
                }
            };
            v[(y * w + x) as usize] = c;
        }
    }
    v
}

fn is10(fmt: DXGI_FORMAT) -> bool {
    fmt == DXGI_FORMAT_R10G10B10A2_UNORM
}

pub(crate) fn levels(fmt: DXGI_FORMAT) -> f32 {
    if is10(fmt) { 1023.0 } else { 255.0 }
}

/// The source as stored floats (exactly what the GPU loads).
pub(crate) fn source_float(src: &[[u8; 3]], fmt: DXGI_FORMAT) -> Vec<[f32; 3]> {
    let l = levels(fmt);
    src.iter().map(|c| c.map(|v| if is10(fmt) { (v as f32 * 1023.0 / 255.0).round() / l } else { v as f32 / l })).collect()
}

fn encode(src: &[[u8; 3]], fmt: DXGI_FORMAT) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() * 4);
    for c in src {
        if is10(fmt) {
            let q = |v: u8| ((v as f32 * 1023.0 / 255.0).round() as u32) & 0x3ff;
            let p = q(c[0]) | (q(c[1]) << 10) | (q(c[2]) << 20) | (3u32 << 30);
            out.extend_from_slice(&p.to_le_bytes());
        } else {
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    out
}

/// M7 vmtest: the stored bytes of a source in mt (for the bit-exact identity check of the nuke pass).
pub(crate) fn encode_pub(src: &[[u8; 3]], fmt: DXGI_FORMAT) -> Vec<u8> {
    encode(src, fmt)
}

/// Readback pixels -> stored integer levels per channel.
pub(crate) fn decode(px: &[u8], fmt: DXGI_FORMAT) -> Vec<[u32; 3]> {
    px.chunks_exact(4)
        .map(|b| {
            if is10(fmt) {
                let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                [v & 0x3ff, (v >> 10) & 0x3ff, (v >> 20) & 0x3ff]
            } else {
                [b[0] as u32, b[1] as u32, b[2] as u32]
            }
        })
        .collect()
}

pub(crate) fn to_png(lv: &[[u32; 3]], fmt: DXGI_FORMAT) -> Vec<u8> {
    let s = if is10(fmt) { 255.0 / 1023.0 } else { 1.0 };
    lv.iter().flat_map(|c| [(c[0] as f32 * s).round() as u8, (c[1] as f32 * s).round() as u8, (c[2] as f32 * s).round() as u8, 255]).collect()
}

/// A target with a persistent upload buffer holding the source frame.
pub(crate) struct Frame {
    pub(crate) t: Tgt,
    up: ID3D12Resource,
    pub(crate) src: Vec<[u8; 3]>,
}

pub(crate) fn make_frame(g: &Gpu, fmt: DXGI_FORMAT, w: u32, h: u32, rtv: usize) -> Result<Frame, String> {
    let t = new_target(g, fmt, w, h, rtv)?;
    let src = source_rgb(w, h);
    let bytes = encode(&src, fmt);
    let size = t.layout.Offset + t.layout.Footprint.RowPitch as u64 * h as u64;
    let bd = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut up: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(unsafe { g.dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_UPLOAD), D3D12_HEAP_FLAG_NONE, &bd, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut up) }, "CreateCommittedResource(upload)")?;
    let up = up.ok_or("upload null")?;
    let mut p = std::ptr::null_mut();
    // SAFETY: mapping our own UPLOAD buffer for writing; rows stay inside the footprint.
    unsafe {
        e(up.Map(0, None, Some(&mut p)), "Map(upload)")?;
        let base = (p as *mut u8).add(t.layout.Offset as usize);
        let row = w as usize * 4;
        for y in 0..h as usize {
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(y * row), base.add(y * t.layout.Footprint.RowPitch as usize), row);
        }
        up.Unmap(0, None);
    }
    Ok(Frame { t, up, src })
}

/// Records: source -> target (ends in COMMON), then hudhook's COMMON -> RENDER_TARGET + OMSetRenderTargets.
pub(crate) fn record_frame_start(g: &Gpu, f: &Frame, first: bool) {
    let before = if first { D3D12_RESOURCE_STATE_RENDER_TARGET } else { D3D12_RESOURCE_STATE_COMMON };
    // SAFETY: recording into our open list; the resources outlive the submission.
    unsafe {
        g.list.ResourceBarrier(&[barrier(&f.t.res, before, D3D12_RESOURCE_STATE_COPY_DEST)]);
        let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: weak(&f.t.res), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
        let src = D3D12_TEXTURE_COPY_LOCATION { pResource: weak(&f.up), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: f.t.layout } };
        g.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        g.list.ResourceBarrier(&[barrier(&f.t.res, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON)]);
        // hudhook: PRESENT (COMMON) -> RENDER_TARGET, bind its RTV, then the pre-ImGui callback
        g.list.ResourceBarrier(&[barrier(&f.t.res, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        g.list.OMSetRenderTargets(1, Some(&f.t.rtv), false, None);
    }
}

/// hudhook after the callback (and ImGui): RENDER_TARGET -> COMMON; then our readback copy.
pub(crate) fn record_frame_end(g: &Gpu, f: &Frame) {
    // SAFETY: recording into our open list.
    unsafe { g.list.ResourceBarrier(&[barrier(&f.t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON)]) };
    record_copy(g, &f.t.res, &f.t.rb, f.t.layout, D3D12_RESOURCE_STATE_COMMON);
}

pub(crate) fn target<'a>(f: &'a Frame) -> ThermalTarget<'a> {
    ThermalTarget { resource: &f.t.res, rtv: f.t.rtv, rtv_format: f.t.fmt, width: f.t.w, height: f.t.h }
}

pub(crate) fn base_params(film: Film, w: u32, h: u32) -> ThermalParams {
    ThermalParams {
        film,
        luma: LUMA_709,
        gamma: 1.0,
        fade: 1.0,
        box_px: box43(w as f32, h as f32),
        overlay: Some(0),
        ov_alpha: 1.0,
        grain: false,
        grain_offset: [0.0, 0.0],
        grain_scale: [1.0, 1.0],
        grain_tint: [0.9, 1.0, 1.0],
        grain_alpha: 0.3,
        timing: false,
        test_unbalanced: false,
    }
}

/// One frame through the pass; returns the read-back levels.
pub(crate) fn run(g: &mut Gpu, pass: &mut ThermalPass, f: &Frame, p: &ThermalParams, first: bool) -> Result<Vec<[u32; 3]>, String> {
    g.begin(0)?;
    record_frame_start(g, f, first);
    pass.record(&g.list, &target(f), 0, p).map_err(|x| format!("record: {x}"))?;
    record_frame_end(g, f);
    g.submit_wait()?;
    Ok(decode(&read_rows(&f.t.rb, &f.t.layout, f.t.w, f.t.h, 4)?, f.t.fmt))
}

/// CPU expectation of one pixel in stored levels (float, before quantisation).
#[allow(clippy::too_many_arguments)]
fn expect_px(src: &[f32; 3], p: &ThermalParams, ov: Option<&[u8]>, grain: Option<&[u8]>, x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let ovs = match (p.overlay, ov) {
        (Some(_), Some(img)) => box_uv(x, y, p.box_px).map(|uv| bilinear(img, 512, 512, uv[0], uv[1], Address::Clamp)),
        _ => None,
    };
    let gs = match (p.grain, grain) {
        (true, Some(img)) => {
            let u = (x as f32 + 0.5) / w as f32 * p.grain_scale[0] + p.grain_offset[0];
            let v = (y as f32 + 0.5) / h as f32 * p.grain_scale[1] + p.grain_offset[1];
            Some(bilinear(img, 512, 512, u, v, Address::Wrap))
        }
        _ => None,
    };
    compose_g(*src, &p.film, p.luma, p.gamma, p.fade, ovs, p.ov_alpha, gs, p.grain_tint, p.grain_alpha)
}

struct Cmp {
    n: usize,
    over: usize,
    worst: f32,
}

/// Compares GPU levels with the CPU expectation over the pixels selected by `sel`; `tol` in LSB.
#[allow(clippy::too_many_arguments)]
fn compare(gpu: &[[u32; 3]], f: &Frame, p: &ThermalParams, ov: Option<&[u8]>, grain: Option<&[u8]>, tol: f32, sel: impl Fn(u32, u32) -> bool) -> Cmp {
    let (w, h) = (f.t.w, f.t.h);
    let l = levels(f.t.fmt);
    let src = source_float(&f.src, f.t.fmt);
    let mut c = Cmp { n: 0, over: 0, worst: 0.0 };
    for y in 0..h {
        for x in 0..w {
            if !sel(x, y) {
                continue;
            }
            let i = (y * w + x) as usize;
            let e = expect_px(&src[i], p, ov, grain, x, y, w, h);
            let mut bad = false;
            for k in 0..3 {
                let d = (gpu[i][k] as f32 - e[k] * l).abs();
                c.worst = c.worst.max(d);
                bad |= d > tol;
            }
            c.n += 1;
            c.over += bad as usize;
        }
    }
    c
}

fn inside(p: &ThermalParams) -> impl Fn(u32, u32) -> bool + '_ {
    move |x, y| box_uv(x, y, p.box_px).is_some()
}

fn outside(p: &ThermalParams) -> impl Fn(u32, u32) -> bool + '_ {
    move |x, y| box_uv(x, y, p.box_px).is_none()
}

pub struct Args<'a> {
    pub out: &'a Path,
    pub ac130: &'a Path,
    pub gbv: bool,
    pub warp: bool,
    /// The thermal pre-exposure (the DLL's `[ac130] exposure`, or --exposure).
    pub exposure: f32,
    /// The mid-tone gamma (the DLL's `[ac130] gamma`, or --gamma; run 8).
    pub gamma: f32,
    /// A real Elden Ring frame for T10 (gitignored work\m4; the user's own screenshot).
    pub reference: &'a Path,
}

/// File name of the T10 reference frame under `<root>\work\m4` (a copy of one of the user's own ER screenshots).
pub const ER_REFERENCE_PNG: &str = "er_frame_ref.png";
/// T10 limit: at most this fraction of a real ER frame may clip to pure white at the configured exposure
/// (review M4 major 8; MW2's look keeps heat sources white, so a small share is expected).
pub const T10_WHITE_CLIP_MAX: f64 = 0.10;
/// T10 limit for pure black (crushed shadows).
pub const T10_BLACK_CLIP_MAX: f64 = 0.10;

/// `[ac130] exposure` and `gamma` from `<root>\mw2er.toml` (what the DLL uses); 1.0 / 1.0 if the file does not parse.
pub fn config_film_in(root: &Path) -> (f32, f32) {
    match std::fs::read_to_string(root.join("mw2er.toml")).map_err(|x| x.to_string()).and_then(|t| mw2er_core::config::Config::parse(&t).map_err(|x| format!("{x:?}"))) {
        Ok(c) => (c.ac130.exposure, c.ac130.gamma),
        Err(x) => {
            println!("WARN mw2er.toml not read ({x}); exposure 1.0, gamma 1.0");
            (1.0, 1.0)
        }
    }
}

/// An 8-bit RGB copy of a PNG (sizes cropped to multiples of 4).
fn load_png_rgb(image: &Path) -> Result<(u32, u32, Vec<[u8; 3]>), String> {
    let file = std::fs::File::open(image).map_err(|x| format!("{}: {x}", image.display()))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|x| x.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|x| x.to_string())?;
    let (w, h) = (info.width - info.width % 4, info.height - info.height % 4);
    let ch = info.color_type.samples();
    let mut src = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = (y * info.width + x) as usize * ch;
            src.push(if ch >= 3 { [buf[i], buf[i + 1], buf[i + 2]] } else { [buf[i]; 3] });
        }
    }
    Ok((w, h, src))
}

/// A frame whose COMMON-state target holds `src` (a real screenshot) instead of the synthetic card.
fn image_frame(g: &Gpu, src: Vec<[u8; 3]>, w: u32, h: u32) -> Result<Frame, String> {
    image_frame_fmt(g, src, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, 0)
}

/// `image_frame` for any SDR format and RTV slot.
pub(crate) fn image_frame_fmt(g: &Gpu, src: Vec<[u8; 3]>, w: u32, h: u32, fmt: DXGI_FORMAT, rtv: usize) -> Result<Frame, String> {
    let mut f = make_frame(g, fmt, w, h, rtv)?;
    let bytes = encode(&src, f.t.fmt);
    let mut p = std::ptr::null_mut();
    // SAFETY: mapping our own UPLOAD buffer; rows stay inside the footprint.
    unsafe {
        e(f.up.Map(0, None, Some(&mut p)), "Map(user upload)")?;
        let base = (p as *mut u8).add(f.t.layout.Offset as usize);
        for y in 0..h as usize {
            std::ptr::copy_nonoverlapping(bytes.as_ptr().add(y * w as usize * 4), base.add(y * f.t.layout.Footprint.RowPitch as usize), w as usize * 4);
        }
        f.up.Unmap(0, None);
    }
    f.src = src;
    Ok(f)
}

/// GPU clip fractions of a film-only output (pure black / pure white, every channel).
fn gpu_clip(out: &[[u32; 3]], fmt: DXGI_FORMAT) -> (f64, f64) {
    let top = levels(fmt) as u32;
    let lo = out.iter().filter(|c| c.iter().all(|&v| v == 0)).count();
    let hi = out.iter().filter(|c| c.iter().all(|&v| v == top)).count();
    (lo as f64 / out.len().max(1) as f64, hi as f64 / out.len().max(1) as f64)
}

/// A 1/`k` box-downscaled RGB preview (for looking at 4K frames).
fn preview(px: &[[u8; 3]], w: u32, h: u32, k: u32) -> (Vec<u8>, u32, u32) {
    let (pw, ph) = (w / k, h / k);
    let mut out = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            let mut s = [0u32; 3];
            for dy in 0..k {
                for dx in 0..k {
                    let c = px[((y * k + dy) * w + x * k + dx) as usize];
                    for i in 0..3 {
                        s[i] += c[i] as u32;
                    }
                }
            }
            let n = k * k;
            out.extend_from_slice(&[(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255]);
        }
    }
    (out, pw, ph)
}

pub fn run_thermal(a: &Args) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let ovl = load_overlays(a.ac130)?;
    let (white, black) = films(a.ac130)?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{} [thermal]", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut pass = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.upload_overlays(&ovl.tex).map_err(|x| format!("upload: {x}"))?;
    pass.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, 1920, 1080).map_err(|x| format!("prepare: {x}"))?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "Tinit", format!("ThermalPass::new + upload_overlays + prepare; InfoQueue errors {} {:?}", errs.len(), errs.first()));

    let (w, h) = (1920u32, 1080u32);
    let f8 = make_frame(&g, DXGI_FORMAT_R8G8B8A8_UNORM, w, h, 0)?;
    write_png(&a.out.join("thermal_source.png"), &to_png(&decode(&encode(&f8.src, f8.t.fmt), f8.t.fmt), f8.t.fmt), w, h)?;
    let ov105 = Some(&ovl.rgba[0][..]);

    // T1 white hot (outside the box: film only; the 105 overlay is on but must not touch it)
    let p1 = base_params(white, w, h);
    let out1 = run(&mut g, &mut pass, &f8, &p1, true)?;
    write_png(&a.out.join("thermal_t1_white.png"), &to_png(&out1, f8.t.fmt), w, h)?;
    let c = compare(&out1, &f8, &p1, ov105, None, 1.0, outside(&p1));
    let allocs = pass.take_present_allocs();
    let errs = g.drain_errors();
    rep.add(c.over == 0 && c.n > 0 && errs.is_empty() && allocs.is_empty(), "T1", format!("white hot R8G8B8A8: {} px outside the 4:3 box, {} beyond 1 LSB (worst {:.2}); present-thread allocs {allocs:?}; InfoQueue {}", c.n, c.over, c.worst, errs.len()));

    // T2 black hot
    let p2 = base_params(black, w, h);
    let out2 = run(&mut g, &mut pass, &f8, &p2, false)?;
    write_png(&a.out.join("thermal_t2_black.png"), &to_png(&out2, f8.t.fmt), w, h)?;
    let c = compare(&out2, &f8, &p2, ov105, None, 1.0, outside(&p2));
    let errs = g.drain_errors();
    rep.add(c.over == 0 && c.n > 0 && errs.is_empty(), "T2", format!("black hot: {} px outside the box, {} beyond 1 LSB (worst {:.2}); InfoQueue {}", c.n, c.over, c.worst, errs.len()));

    // T3 overlays 105/40/25 inside the box (3 LSB, <= 0.5% outliers; coverage within 0.1%)
    let mut t3 = Vec::new();
    let mut t3_ok = true;
    for (k, name) in ["105", "40", "25"].iter().enumerate() {
        let p = ThermalParams { overlay: Some(k), ..base_params(white, w, h) };
        let out = run(&mut g, &mut pass, &f8, &p, false)?;
        write_png(&a.out.join(format!("thermal_t3_{name}.png")), &to_png(&out, f8.t.fmt), w, h)?;
        let c = compare(&out, &f8, &p, Some(&ovl.rgba[k]), None, 3.0, inside(&p));
        // coverage: pixels the overlay visibly changed (GPU vs film-only) against the reference
        let film_only = ThermalParams { overlay: None, ..p };
        let src = source_float(&f8.src, f8.t.fmt);
        let (mut cov_gpu, mut cov_ref, mut box_n, mut outside_changed) = (0usize, 0usize, 0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let base = expect_px(&src[i], &film_only, None, None, x, y, w, h);
                let refv = expect_px(&src[i], &p, Some(&ovl.rgba[k]), None, x, y, w, h);
                let gd = (0..3).any(|j| (out[i][j] as f32 - base[j] * 255.0).abs() > 2.0);
                if box_uv(x, y, p.box_px).is_some() {
                    box_n += 1;
                    cov_gpu += gd as usize;
                    cov_ref += (0..3).any(|j| ((refv[j] - base[j]) * 255.0).abs() > 2.0) as usize;
                } else if gd {
                    outside_changed += 1;
                }
            }
        }
        let cov_d = (cov_gpu as f64 - cov_ref as f64).abs() / box_n as f64;
        let ok = (c.over as f64) <= 0.005 * c.n as f64 && cov_d <= 0.001 && outside_changed == 0 && cov_ref > 0;
        t3_ok &= ok;
        t3.push(format!("{name}: {} beyond 3 LSB of {} ({:.3}%), worst {:.1}, coverage gpu {cov_gpu} ref {cov_ref} (diff {:.4}% of the box), outside changed {outside_changed}", c.over, c.n, 100.0 * c.over as f64 / c.n as f64, c.worst, cov_d * 100.0));
    }
    let errs = g.drain_errors();
    rep.add(t3_ok && errs.is_empty(), "T3", format!("{}; InfoQueue {}", t3.join(" | "), errs.len()));

    // T4 grain on/off: difference statistics vs the CPU expectation (1%)
    let off = ThermalParams { overlay: None, ..base_params(white, w, h) };
    let on = ThermalParams { grain: true, grain_offset: grain_offset(7, 512), ..off };
    let o_off = run(&mut g, &mut pass, &f8, &off, false)?;
    let o_on = run(&mut g, &mut pass, &f8, &on, false)?;
    write_png(&a.out.join("thermal_t4_grain.png"), &to_png(&o_on, f8.t.fmt), w, h)?;
    let src = source_float(&f8.src, f8.t.fmt);
    let (mut gs, mut gs2, mut cs, mut cs2) = (0f64, 0f64, 0f64, 0f64);
    let n = (w * h * 3) as f64;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let a0 = expect_px(&src[i], &off, None, None, x, y, w, h);
            let a1 = expect_px(&src[i], &on, None, Some(&ovl.rgba[3]), x, y, w, h);
            for k in 0..3 {
                // both sides quantised to 8 bits, as the GPU writes them (the difference of two rounded images)
                let dg = (o_on[i][k] as f64 - o_off[i][k] as f64) / 255.0;
                let dc = ((a1[k] as f64 * 255.0).round() - (a0[k] as f64 * 255.0).round()) / 255.0;
                gs += dg;
                gs2 += dg * dg;
                cs += dc;
                cs2 += dc * dc;
            }
        }
    }
    let (gm, cm) = (gs / n, cs / n);
    let (gsd, csd) = ((gs2 / n - gm * gm).max(0.0).sqrt(), (cs2 / n - cm * cm).max(0.0).sqrt());
    let ok4 = (gm - cm).abs() <= 0.01 * cm.abs().max(1.0 / 255.0) && (gsd - csd).abs() <= 0.01 * csd.max(1.0 / 255.0) && csd > 0.0;
    let errs = g.drain_errors();
    rep.add(ok4 && errs.is_empty(), "T4", format!("grain on - off: mean gpu {gm:.5} cpu {cm:.5}, std gpu {gsd:.5} cpu {csd:.5} (1%); InfoQueue {}", errs.len()));

    // T5 reload blink: the overlay alpha factor at 3 sample times
    let mut t5 = Vec::new();
    let mut t5_ok = true;
    for ms in [0.0f64, 141.4, 400.0] {
        let alpha = blink_alpha(ms, 90.0, 0.65);
        let p = ThermalParams { ov_alpha: alpha, ..base_params(white, w, h) };
        let out = run(&mut g, &mut pass, &f8, &p, false)?;
        let c = compare(&out, &f8, &p, ov105, None, 3.0, inside(&p));
        let ok = (c.over as f64) <= 0.005 * c.n as f64;
        t5_ok &= ok;
        t5.push(format!("t={ms} ms alpha {alpha:.3}: {} beyond 3 LSB", c.over));
    }
    let errs = g.drain_errors();
    rep.add(t5_ok && errs.is_empty(), "T5", format!("{}; InfoQueue {}", t5.join(", "), errs.len()));

    // T6 resize: 1080p -> 4K -> 1080p, three frames submitted without waiting
    let f4 = make_frame(&g, DXGI_FORMAT_R8G8B8A8_UNORM, 3840, 2160, 1)?;
    let f1b = make_frame(&g, DXGI_FORMAT_R8G8B8A8_UNORM, w, h, 2)?;
    let mut fences = Vec::new();
    for (k, (fr, first)) in [(&f8, false), (&f4, true), (&f1b, true)].into_iter().enumerate() {
        let p = base_params(white, fr.t.w, fr.t.h);
        g.begin(1 + k)?;
        record_frame_start(&g, fr, first);
        pass.record(&g.list, &target(fr), k % NUM_FRAMES, &p).map_err(|x| format!("T6 record {k}: {x}"))?;
        record_frame_end(&g, fr);
        fences.push(g.submit()?);
    }
    for v in fences {
        g.wait(v)?;
    }
    let mut t6 = Vec::new();
    let mut t6_ok = true;
    for fr in [&f8, &f4, &f1b] {
        let out = decode(&read_rows(&fr.t.rb, &fr.t.layout, fr.t.w, fr.t.h, 4)?, fr.t.fmt);
        let p = base_params(white, fr.t.w, fr.t.h);
        let c = compare(&out, fr, &p, ov105, None, 1.0, outside(&p));
        t6_ok &= c.over == 0 && c.n > 0;
        t6.push(format!("{}x{}: {} of {} beyond 1 LSB", fr.t.w, fr.t.h, c.over, c.n));
    }
    let allocs = pass.take_present_allocs();
    let errs = g.drain_errors();
    rep.add(t6_ok && errs.is_empty() && allocs.len() == 2, "T6", format!("{}; scene copies created on the present thread {allocs:?} (want 2); InfoQueue {}", t6.join(", "), errs.len()));

    // T7 formats: R10G10B10A2 passes T1 (1 LSB of 10 bits)
    let f10 = make_frame(&g, DXGI_FORMAT_R10G10B10A2_UNORM, w, h, 3)?;
    let p7 = base_params(white, w, h);
    let out7 = run(&mut g, &mut pass, &f10, &p7, true)?;
    let c = compare(&out7, &f10, &p7, ov105, None, 1.0, outside(&p7));
    let errs = g.drain_errors();
    rep.add(c.over == 0 && c.n > 0 && errs.is_empty(), "T7", format!("R10G10B10A2 white hot: {} px, {} beyond 1 LSB (worst {:.2}); R8G8B8A8 = T1; InfoQueue {}", c.n, c.over, c.worst, errs.len()));

    // T11 (run 8) the mid-tone gamma path: the configured exposure + gamma on the card, white hot, film only
    let p11 = ThermalParams { overlay: None, luma: exposed_luma(LUMA_709, a.exposure), gamma: a.gamma, ..base_params(white, w, h) };
    // its own pass: T6/T7 just used every scene descriptor slot of the shared one within num_frames + 1 records
    let mut pass11 = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass11.upload_overlays(&ovl.tex).map_err(|x| format!("upload T11: {x}"))?;
    let out11 = run(&mut g, &mut pass11, &f8, &p11, false)?;
    drop(pass11);
    write_png(&a.out.join("thermal_t11_gamma.png"), &to_png(&out11, f8.t.fmt), w, h)?;
    let c = compare(&out11, &f8, &p11, None, None, 2.0, |_, _| true);
    let (lo11, hi11) = clip_fractions_g(&f8.src, &white, p11.luma, p11.gamma);
    let (_, hi_old) = clip_fractions_g(&f8.src, &white, exposed_luma(LUMA_709, 0.6), 1.0);
    let errs = g.drain_errors();
    rep.add(
        c.over <= c.n / 200 && c.n > 0 && errs.is_empty(),
        "T11",
        format!(
            "exposure {:.2} gamma {:.2} white hot on the card: {} px, {} beyond 2 LSB (worst {:.2}); card clip white {:.1}% black {:.1}% (old default 0.60/1.00: white {:.1}%); InfoQueue {}",
            a.exposure, a.gamma, c.n, c.over, c.worst, hi11 * 100.0, lo11 * 100.0, hi_old * 100.0, errs.len()
        ),
    );

    // T10 a real ER frame at the configured exposure (its own pass: the main pass stays prepared for 1920x1080)
    let mut pass10 = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass10.upload_overlays(&ovl.tex).map_err(|x| format!("upload T10: {x}"))?;
    match t10(&mut g, &mut pass10, a, white)? {
        Some((ok, detail)) => rep.add(ok, "T10", detail),
        None => {
            // not a FAIL: no real ER frame exists offline yet (the user takes one in the next run); reported as such
            let l = format!(
                "NEEDS-DATA T10 reference frame {} missing: copy one of your own ER screenshots (SDR PNG, game window only) there; exposure {:.2} is [I] until then",
                a.reference.display(),
                a.exposure
            );
            println!("{l}");
            rep.lines.push(l);
        }
    }
    drop(pass10);

    // T9 failure path: invalid input -> Err before any command (recorded into a CLOSED list: any command would be a
    // debug-layer error)
    g.begin(5)?;
    // SAFETY: closing our own list.
    e(unsafe { g.list.Close() }, "Close(T9)")?;
    let wrong_fmt = ThermalTarget { resource: &f8.t.res, rtv: f8.t.rtv, rtv_format: DXGI_FORMAT_R10G10B10A2_UNORM, width: w, height: h };
    let r1 = pass.record(&g.list, &wrong_fmt, 0, &base_params(white, w, h));
    let mut fresh = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    let r2 = fresh.record(&g.list, &target(&f8), 0, &base_params(white, w, h));
    let wrong_size = ThermalTarget { resource: &f8.t.res, rtv: f8.t.rtv, rtv_format: f8.t.fmt, width: 1280, height: 720 };
    let r3 = pass.record(&g.list, &wrong_size, 0, &base_params(white, w, h));
    let errs = g.drain_errors();
    let ok9 = r1.is_err() && r2.is_err() && r3.is_err() && errs.is_empty();
    rep.add(ok9, "T9", format!("wrong format -> {:?}; overlays missing -> {:?}; wrong size -> {:?}; commands recorded into the closed list: {} debug error(s)", r1.err().map(|x| x.to_string()), r2.err().map(|x| x.to_string()), r3.err().map(|x| x.to_string()), errs.len()));

    // T8 state proof: the balanced pass + hudhook's RT -> COMMON gives no error (T1..T7); an unbalanced variant does.
    // Its own pass: T6/T7 just used every scene descriptor slot of the shared one within num_frames + 1 records.
    let mut pass8 = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass8.upload_overlays(&ovl.tex).map_err(|x| format!("upload: {x}"))?;
    g.begin(4)?;
    record_frame_start(&g, &f8, false);
    let bad = ThermalParams { test_unbalanced: true, ..base_params(white, w, h) };
    pass8.record(&g.list, &target(&f8), 1, &bad).map_err(|x| format!("T8 record: {x}"))?;
    // SAFETY: recording into our open list (hudhook's barrier, whose before-state is now wrong).
    unsafe { g.list.ResourceBarrier(&[barrier(&f8.t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON)]) };
    // SAFETY: closing our own list; it is never executed.
    let _ = unsafe { g.list.Close() };
    let errs = g.drain_errors();
    rep.add(!errs.is_empty(), "T8", format!("unbalanced variant (BB left in COPY_SOURCE) flagged by the debug layer: {} error(s) {:?}; the balanced pass gave 0 in T1-T7", errs.len(), errs.first().map(|s| &s[..s.len().min(120)])));

    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "Tinfoqueue", format!("{} ERROR/CORRUPTION messages at the end", errs.len()));
    std::fs::write(a.out.join(if a.gbv { "thermal_report_gbv.txt" } else { "thermal_report.txt" }), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    println!("THERMAL {} ({} FAIL)", if rep.fails == 0 { "PASS" } else { "FAIL" }, rep.fails);
    Ok(rep.fails == 0)
}

/// Tperf: 3840x2160 R10G10B10A2, 320 frames (first 20 dropped), hardware, no debug layer.
pub fn run_thermal_perf(a: &Args) -> Result<bool, String> {
    let ovl = load_overlays(a.ac130)?;
    let (white, _) = films(a.ac130)?;
    let mut g = Gpu::new(false, false, a.warp)?;
    if g.software {
        println!("THERMAL PERF FAIL perf needs hardware (adapter {} is software/WARP)", g.adapter);
        return Ok(false);
    }
    let mut pass = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.upload_overlays(&ovl.tex).map_err(|x| x.to_string())?;
    let (w, h) = (3840u32, 2160u32);
    pass.prepare(DXGI_FORMAT_R10G10B10A2_UNORM, w, h).map_err(|x| x.to_string())?;
    let f = make_frame(&g, DXGI_FORMAT_R10G10B10A2_UNORM, w, h, 0)?;
    let mut samples = Vec::new();
    for k in 0..320u64 {
        let p = ThermalParams { grain: true, grain_offset: grain_offset(k, 512), timing: true, ..base_params(white, w, h) };
        g.begin(0)?;
        record_frame_start(&g, &f, k == 0);
        pass.record(&g.list, &target(&f), 0, &p).map_err(|x| x.to_string())?;
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
        println!("THERMAL PERF FAIL only {} timestamp samples", samples.len());
        return Ok(false);
    }
    let q = |f: f64| samples[((samples.len() - 1) as f64 * f).round() as usize];
    let (p50, p95, max) = (q(0.5), q(0.95), samples[samples.len() - 1]);
    let ok = p95 < 1.0 && p50 < 0.3;
    std::fs::create_dir_all(a.out).ok();
    let j = format!(
        "{{\"adapter\": \"{}\", \"pass\": \"thermal\", \"format\": \"R10G10B10A2_UNORM\", \"width\": {w}, \"height\": {h}, \"frames\": {}, \"median_ms\": {p50:.4}, \"p95_ms\": {p95:.4}, \"max_ms\": {max:.4}, \"pass_ok\": {ok}}}\n",
        g.adapter.replace('"', "'"),
        samples.len()
    );
    std::fs::write(a.out.join("perf_thermal.json"), j).map_err(|x| x.to_string())?;
    println!(
        "THERMAL PERF {} {} @{w}x{h} R10G10B10A2: {} frames, median {p50:.4} ms, p95 {p95:.4} ms, max {max:.4} ms (need p95 < 1.0, median < 0.3)",
        if ok { "PASS" } else { "FAIL" },
        g.adapter,
        samples.len()
    );
    Ok(ok)
}

/// Runs the pass on a user screenshot: the clip fractions of the film alone (CPU, an exposure sweep), the GPU film
/// at the configured exposure (must agree with the CPU), then white hot + 105 overlay + grain -> `thermal_user.png`
/// and 1/4-size previews of the source and the output.
pub fn run_thermal_image(a: &Args, image: &PathBuf) -> Result<bool, String> {
    let ovl = load_overlays(a.ac130)?;
    let (white, black) = films(a.ac130)?;
    let (w, h, src) = load_png_rgb(image)?;
    // mean film output of the mid-tones (source luma 0.15..0.45): how bright the image reads
    let mid: Vec<[u8; 3]> = src.iter().copied().filter(|c| {
        let l = (c[0] as f32 * LUMA_709[0] + c[1] as f32 * LUMA_709[1] + c[2] as f32 * LUMA_709[2]) / 255.0;
        (0.15..=0.45).contains(&l)
    }).collect();
    for (ex, ga) in [(1.0f32, 1.0f32), (0.8, 1.0), (0.65, 1.0), (0.6, 1.0), (0.5, 1.0), (0.6, 0.8), (0.55, 0.8), (0.52, 0.75), (0.5, 0.7), (0.55, 0.75), (0.58, 0.85)] {
        let l = exposed_luma(LUMA_709, ex);
        let (lo, hi) = clip_fractions_g(&src, &white, l, ga);
        let (blo, bhi) = clip_fractions_g(&src, &black, l, ga);
        let m = if mid.is_empty() {
            f32::NAN
        } else {
            mid.iter().map(|c| mw2er_core::thermal::shade_pixel_g(c.map(|v| v as f32 / 255.0), &white, l, ga, 1.0)[0]).sum::<f32>() / mid.len() as f32
        };
        println!(
            "THERMAL IMAGE sweep exposure={ex:.2} gamma={ga:.2} white_hot: black={:.1}% white={:.1}% midtone_out={m:.3} | black_hot: black={:.1}% white={:.1}%",
            lo * 100.0,
            hi * 100.0,
            blo * 100.0,
            bhi * 100.0
        );
    }
    let mut g = Gpu::new(true, false, a.warp)?;
    let mut pass = ThermalPass::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    pass.upload_overlays(&ovl.tex).map_err(|x| x.to_string())?;
    let luma = exposed_luma(LUMA_709, a.exposure);
    let f = image_frame(&g, src.clone(), w, h)?;
    // film only at the configured exposure + gamma: GPU vs CPU clip fractions
    let pf = ThermalParams { overlay: None, luma, gamma: a.gamma, ..base_params(white, w, h) };
    let film = run(&mut g, &mut pass, &f, &pf, true)?;
    let (glo, ghi) = gpu_clip(&film, f.t.fmt);
    let (clo, chi) = clip_fractions_g(&src, &white, luma, a.gamma);
    println!(
        "THERMAL IMAGE exposure={:.2} gamma={:.2} (configured) white_hot GPU black={:.2}% white={:.2}% CPU black={:.2}% white={:.2}%",
        a.exposure,
        a.gamma,
        glo * 100.0,
        ghi * 100.0,
        clo * 100.0,
        chi * 100.0
    );
    let params = ThermalParams { grain: true, grain_offset: grain_offset(3, 512), luma, gamma: a.gamma, ..base_params(white, w, h) };
    let out = run(&mut g, &mut pass, &f, &params, false)?;
    std::fs::create_dir_all(a.out).ok();
    write_png(&a.out.join("thermal_user.png"), &to_png(&out, f.t.fmt), w, h)?;
    let (pv, pw, ph) = preview(&src, w, h, 4);
    write_png(&a.out.join("thermal_user_source_preview.png"), &pv, pw, ph)?;
    let out8: Vec<[u8; 3]> = out.iter().map(|c| c.map(|v| v.min(255) as u8)).collect();
    let (pv, pw, ph) = preview(&out8, w, h, 4);
    write_png(&a.out.join("thermal_user_preview.png"), &pv, pw, ph)?;
    let errs = g.drain_errors();
    let agree = (glo - clo).abs() < 0.005 && (ghi - chi).abs() < 0.005;
    println!(
        "THERMAL IMAGE {}x{} written to {} (+ previews); GPU/CPU clip agree={}; InfoQueue {}",
        w,
        h,
        a.out.join("thermal_user.png").display(),
        agree as u8,
        errs.len()
    );
    Ok(errs.is_empty() && agree)
}

/// T10 (review M4 major 8): the film at the configured exposure on a real ER frame keeps the clipped-white and
/// clipped-black shares under the limits, GPU == CPU. `None` when the reference frame is missing.
fn t10(g: &mut Gpu, pass: &mut ThermalPass, a: &Args, white: Film) -> Result<Option<(bool, String)>, String> {
    if !a.reference.is_file() {
        return Ok(None);
    }
    let (w, h, src) = load_png_rgb(a.reference)?;
    pass.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, w, h).map_err(|x| format!("prepare T10: {x}"))?;
    let luma = exposed_luma(LUMA_709, a.exposure);
    let f = image_frame(g, src.clone(), w, h)?;
    let p = ThermalParams { overlay: None, luma, gamma: a.gamma, ..base_params(white, w, h) };
    let out = run(g, pass, &f, &p, true)?;
    let (glo, ghi) = gpu_clip(&out, f.t.fmt);
    let (clo, chi) = clip_fractions_g(&src, &white, luma, a.gamma);
    let (_, raw_hi) = clip_fractions_g(&src, &white, LUMA_709, 1.0);
    let errs = g.drain_errors();
    let ok = ghi <= T10_WHITE_CLIP_MAX && glo <= T10_BLACK_CLIP_MAX && (ghi - chi).abs() < 0.005 && (glo - clo).abs() < 0.005 && errs.is_empty();
    Ok(Some((
        ok,
        format!(
            "real ER frame {}x{} white hot at exposure {:.2} gamma {:.2}: white {:.2}% (max {:.0}%), black {:.2}% (max {:.0}%); CPU {:.2}%/{:.2}%; at exposure 1.00 (MW2 as is) white would be {:.1}%; InfoQueue {}",
            w,
            h,
            a.exposure,
            a.gamma,
            ghi * 100.0,
            T10_WHITE_CLIP_MAX * 100.0,
            glo * 100.0,
            T10_BLACK_CLIP_MAX * 100.0,
            chi * 100.0,
            clo * 100.0,
            raw_hi * 100.0,
            errs.len()
        ),
    )))
}
