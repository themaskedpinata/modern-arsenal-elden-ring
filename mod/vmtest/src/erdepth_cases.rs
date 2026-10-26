//! C6 depth 5 vmtest cases (docs/C6_HOLD_DESIGN.md 15.5): the DLL's own `mw2er_render::er_depth` (the barrier detour,
//! the reader's copy / sample, the prime) and `Renderer::record_world_occluded` on an ER-like D32S8 depth texture.
//!
//!   vmtest --erdepth [--gbv] [--warp] [--out <dir>]
//!       ED0 an ER-like R32G8X24_TYPELESS depth written by an ER-like list and left in 0xC0; our reader (copy + the 22-texel
//!           sample) spliced ahead of an ER-like list B whose first barrier is 0xC0 -> DEPTH_WRITE (B then clears the
//!           depth to 0.5): the half-resolution copy == the CPU's farthest-of-2x2, the texels == the CPU values, B's
//!           clear never seen (queue order) and B's 0xC0 before-state held; C6 round 7: the chest cross median == the
//!           CPU's (ED0e: a cross over a wall edge is an edge sample)
//!       ED1 a wall nearer than the gun hides it; a wall behind it leaves the image bit-identical to the unprimed one; a
//!           wall over the right part hides only gun pixels right of its edge
//!       ED2 no prime (None) == record_world, on a renderer with and without the prepared prime pass; a prime given to a
//!           renderer without the pass is ignored
//!       ED3 the bias edges against the gun's nearest point (CPU from its mesh): a wall 2 cm in front stays visible
//!           with the bias (3 cm + 0.4 %), hides it with no bias, a wall 10 cm in front hides it with the bias
//!       ED4 the prime and the capsules make a union (each alone leaves gun pixels, both together none)
//!       ED5 the real detour (`resource_barrier_detour`, its trampoline = slot 26 of this device's list vtable) on a
//!           scripted sequence over 3 lists and 2 frames (ALL / per plane, a split pair, re-recorded lists): the fold
//!           equals the script (Q, the state at the armed call, transitions, split, no mismatch, vt_eq)
//!       ED8 (C6 round 6, work/c6/depth6.md 4) the corrected probe: calls are counted with nothing watched; two watched
//!           candidates (A: an ER-like 3D depth, B: a UI-only `plane 1 0xC0 -> 0x10 -> 0xC0` pair = the positive control)
//!           plus a third unwatched depth: each fold equals the script, B's control counts its frames, the census
//!           holds all three
//!       ED7 a resized reader (fence started at the last value handed out): the old values are passed on its fence,
//!           its next copy splices and equals the CPU (a fence from 0 would leave every old slot busy)
//!       ED6 (--gbv only) the negative control: the reader's copy of the depth left in DEPTH_WRITE is reported by GBV
//!       zero debug-layer messages of any severity from our records and their execution (+ GBV with --gbv); the
//!       harness's own depth clears are drained separately. Images: `<out>\erdepth_<case>.png`. Exit 0 = PASS.

use std::path::Path;

use mw2er_core::body_occl::Capsule;
use mw2er_core::er_depth_state::{self as eds, EclIn, Fold, SUB_ALL};
use mw2er_core::vec3::V3;
use mw2er_core::vm_assets::{TextureCache, VmAssets};
use mw2er_core::xform::m_point;
use mw2er_render::er_depth::{DepthReader, PrimeIn};
use mw2er_render::{LightParams, OutputMode, Renderer, Target, Variant, WorldDraw, WorldFrame};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use super::{Gpu, H1, KEY, KEY_F, NUM_FRAMES, Report, Tgt, W1, e, new_target, project_root, read_rows, readback_for, record_copy, write_png};

pub struct ErDepthArgs<'a> {
    pub out: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

/// ER's near plane in these cases (an infinite far: z = n / d).
const NEAR_ER: f32 = 0.1;
const REST: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(0xC0);
const CAM_Z: f32 = -3.0;

/// The ER-like scene depth: R32G8X24_TYPELESS, back-buffer sized, resting in 0xC0, its own DSV.
struct ErDepth {
    res: ID3D12Resource,
    _heap: ID3D12DescriptorHeap,
    dsv: D3D12_CPU_DESCRIPTOR_HANDLE,
}

fn er_depth_tex(g: &Gpu) -> Result<ErDepth, String> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: W1 as u64,
        Height: H1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R32G8X24_TYPELESS,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE {
        Format: DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
        Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 } },
    };
    let mut res: Option<ID3D12Resource> = None;
    let hp = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(unsafe { g.dev.CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, REST, Some(&clear), &mut res) }, "CreateCommittedResource(er depth)")?;
    let res = res.ok_or("er depth null")?;
    // SAFETY: plain COM creation + a DSV into its own heap.
    let heap: ID3D12DescriptorHeap = e(
        unsafe { g.dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 1, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE, NodeMask: 0 }) },
        "CreateDescriptorHeap(er dsv)",
    )?;
    // SAFETY: as above.
    let dsv = unsafe {
        let h = heap.GetCPUDescriptorHandleForHeapStart();
        g.dev.CreateDepthStencilView(
            &res,
            Some(&D3D12_DEPTH_STENCIL_VIEW_DESC {
                Format: DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
                ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
                Flags: D3D12_DSV_FLAG_NONE,
                Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 } },
            }),
            h,
        );
        h
    };
    Ok(ErDepth { res, _heap: heap, dsv })
}

/// One transition barrier on `sub` (ALL / plane) with `flags`.
fn tb(r: &ID3D12Resource, sub: u32, a: u32, b: u32, flags: D3D12_RESOURCE_BARRIER_FLAGS) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: flags,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: super::weak(r),
                Subresource: sub,
                StateBefore: D3D12_RESOURCE_STATES(a as i32),
                StateAfter: D3D12_RESOURCE_STATES(b as i32),
            }),
        },
    }
}

/// ER's frame: the depth from 0xC0 to DEPTH_WRITE, cleared to the sky (0), the walls, back to 0xC0. The CPU's values.
fn er_frame(g: &mut Gpu, d: &ErDepth, walls: &[(RECT, f32)]) -> Result<Vec<f32>, String> {
    g.begin(1)?;
    // SAFETY: recording into the harness's open list; the resources outlive the wait.
    unsafe {
        g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0xC0, 0x10, D3D12_RESOURCE_BARRIER_FLAG_NONE)]);
        g.list.ClearDepthStencilView(d.dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
        for (r, v) in walls {
            g.list.ClearDepthStencilView(d.dsv, D3D12_CLEAR_FLAG_DEPTH, *v, 0, Some(&[*r]));
        }
        g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0x10, 0xC0, D3D12_RESOURCE_BARRIER_FLAG_NONE)]);
    }
    g.submit_wait()?;
    let mut cpu = vec![0f32; (W1 * H1) as usize];
    for (r, v) in walls {
        for y in r.top.max(0) as u32..(r.bottom as u32).min(H1) {
            for x in r.left.max(0) as u32..(r.right as u32).min(W1) {
                cpu[(y * W1 + x) as usize] = *v;
            }
        }
    }
    Ok(cpu)
}

/// ER's list B (not executed here): its first barrier 0xC0 -> DEPTH_WRITE, a clear to 0.5, back to 0xC0.
fn record_er_b(g: &Gpu, d: &ErDepth) -> Result<(), String> {
    g.begin(3)?;
    // SAFETY: recording into the harness's open list.
    unsafe {
        g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0xC0, 0x10, D3D12_RESOURCE_BARRIER_FLAG_NONE)]);
        g.list.ClearDepthStencilView(d.dsv, D3D12_CLEAR_FLAG_DEPTH, 0.5, 0, None);
        g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0x10, 0xC0, D3D12_RESOURCE_BARRIER_FLAG_NONE)]);
        e(g.list.Close(), "Close(B)")
    }
}

/// hudhook patch 0002's splice at `at = 0`: our list, Signal(our fence, value), then ER's call; then the harness waits.
fn splice_run(g: &mut Gpu, rd: &DepthReader, slot: usize, value: u64, with_b: bool) -> Result<(), String> {
    let ours: ID3D12CommandList = e(rd.list(slot).ok_or("slot")?.cast(), "cast ours")?;
    // SAFETY: closed lists of ours / the harness; the objects live until the wait below.
    unsafe {
        g.queue.ExecuteCommandLists(&[Some(ours)]);
        e(g.queue.Signal(rd.fence(), value), "Signal(ours)")?;
        if with_b {
            g.queue.ExecuteCommandLists(&[Some(e(g.list.cast(), "cast B")?)]);
        }
    }
    g.fence_val += 1;
    // SAFETY: signalling the harness fence after both calls.
    e(unsafe { g.queue.Signal(&g.fence, g.fence_val) }, "Signal(harness)")?;
    g.wait(g.fence_val)
}

/// A readback of our copy texture (R32_FLOAT, half resolution).
fn read_copy(g: &mut Gpu, tex: &ID3D12Resource) -> Result<Vec<f32>, String> {
    // SAFETY: plain query.
    let desc = unsafe { tex.GetDesc() };
    let (rb, layout) = readback_for(g, &desc)?;
    g.begin(2)?;
    record_copy(g, tex, &rb, layout, REST);
    g.submit_wait()?;
    let b = read_rows(&rb, &layout, desc.Width as u32, desc.Height, 4)?;
    Ok(b.chunks(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// The test camera: at (0, 1.5, -3) looking +z (ER rows right, up, forward), 50 deg vertical (as occl_cases).
fn frame_of() -> Result<WorldFrame, String> {
    let rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (view, view_proj) = mw2er_core::coop_vis::world_view_proj([0.0, 1.5, CAM_Z], rows, 50f32.to_radians(), W1, H1, eds::OUR_NEAR, eds::OUR_FAR).ok_or("view_proj")?;
    Ok(WorldFrame { view, view_proj, light: LightParams::default(), out: OutputMode::Srgb, paper_white_nits: 200.0, variant: Variant::default() })
}

fn gun_matrix() -> Result<mw2er_core::xform::Mat34, String> {
    mw2er_core::coop_vis::world_gun_matrix(V3::new(0.0, 1.5, 0.0), V3::new(1.0, 0.0, 0.0), [0.0, 0.0, 0.0], 0.0).ok_or_else(|| "gun matrix".to_string())
}

fn draw(g: &mut Gpu, r: &mut Renderer, t: &Tgt, caps: &[Capsule], prime: Option<&PrimeIn<'_>>, plain: bool) -> Result<(Vec<u8>, bool), String> {
    let f = frame_of()?;
    let draws = [WorldDraw { gun: 0, lod: 0, model: gun_matrix()? }];
    g.begin(0)?;
    // SAFETY: recording into our open list; the target is in RENDER_TARGET state.
    unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
    let target = Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h };
    if plain {
        r.record_world(&g.list, &target, 0, &draws, &f).map_err(|x| format!("record_world: {x}"))?;
    } else {
        r.record_world_occluded(&g.list, &target, 0, &draws, &f, caps, prime).map_err(|x| format!("record_world_occluded: {x}"))?;
    }
    let primed = r.last_primed();
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    Ok((read_rows(&t.rb, &t.layout, t.w, t.h, 4)?, primed))
}

fn gun_px(p: &[u8]) -> usize {
    p.chunks(4).filter(|c| c[..3] != KEY[..3]).count()
}

fn renderer(g: &Gpu, prime: bool) -> Result<(Renderer, f32), String> {
    let dir = project_root().join("converted").join("guns").join("striker_mp");
    let mut cache = TextureCache::new();
    let list = VmAssets::load_gun_world(&dir, &mut cache)?;
    let w = list.iter().find(|w| w.lod == 0).ok_or("striker_mp has no world_lod0")?;
    // the gun's nearest point in view z (CPU, from its mesh)
    let m = gun_matrix()?;
    let mut zmin = f32::INFINITY;
    for i in 0..w.mesh.vertex_count {
        if let Some(v) = w.mesh.vertex(i) {
            zmin = zmin.min(m_point(&m, &v.pos)[2] - CAM_Z);
        }
    }
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, 0).map_err(|x| x.to_string())?;
    r.upload_world(0, 0, &w.mesh, &w.textures).map_err(|x| format!("upload_world: {x}"))?;
    r.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, Variant::default(), W1, H1).map_err(|x| format!("prepare: {x}"))?;
    r.prepare_occluders().map_err(|x| format!("prepare_occluders: {x}"))?;
    if prime {
        r.prepare_er_depth().map_err(|x| format!("prepare_er_depth: {x}"))?;
    }
    Ok((r, zmin))
}

/// ER's reverse-Z value of a wall at view z (infinite far).
fn d_of(z: f32) -> f32 {
    NEAR_ER / z
}

fn full() -> RECT {
    RECT { left: 0, top: 0, right: W1 as i32, bottom: H1 as i32 }
}

pub fn run_erdepth(a: &ErDepthArgs<'_>) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let (mut r, zmin) = renderer(&g, true)?;
    let t = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W1, H1, 0)?;
    let d = er_depth_tex(&g)?;
    let mut rd = DepthReader::new(&g.dev, W1, H1, 0).map_err(|x| format!("DepthReader::new: {x}"))?;
    let _ = g.drain_all();
    let png = |name: &str, p: &[u8]| {
        let rgba: Vec<u8> = p.chunks(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        let _ = write_png(&a.out.join(format!("erdepth_{name}.png")), &rgba, W1, H1);
    };
    // our messages (records + spliced execution + prime) and the harness's (its clears), kept apart
    let mut msgs: Vec<String> = Vec::new();
    let mut harness: Vec<String> = Vec::new();
    let mut value = 0u64;

    // ED0: the copy and the sample, spliced ahead of ER's list B
    let wall = RECT { left: 600, top: 200, right: 1301, bottom: 801 };
    let cpu = er_frame(&mut g, &d, &[(wall, d_of(2.0)), (RECT { left: 1301, top: 801, right: 1500, bottom: 1000 }, 0.75)])?;
    harness.extend(g.drain_all());
    let pts = eds::sample_points(W1, H1, Some((1000, 500)));
    rd.record(0, &d.res, true, Some(&pts)).map_err(|x| format!("ED0 record: {x}"))?;
    msgs.extend(g.drain_all());
    record_er_b(&g, &d)?;
    harness.extend(g.drain_all());
    value += 1;
    splice_run(&mut g, &rd, 0, value, true)?;
    msgs.extend(g.drain_all());
    let got = read_copy(&mut g, rd.texture(0).ok_or("tex")?)?;
    let want = eds::copy_ref(&cpu, W1, H1);
    let copy_bad = got.iter().zip(&want).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
    let b_seen = got.iter().filter(|x| **x == 0.5).count();
    let tex = rd.read_sample();
    let sample_bad = pts.iter().zip(tex.iter()).filter(|(p, v)| cpu[(p[1] * W1 + p[0]) as usize].to_bits() != v.to_bits()).count();
    // C6 round 7: the chest's 5-texel cross median (GPU texels) == the CPU's, inside the wall: no spread
    let cross = |src: &dyn Fn(usize) -> f32| {
        let mut d5 = [0f32; eds::CHEST_N];
        for (k, v) in d5.iter_mut().enumerate() {
            *v = src(eds::CHEST_AT + k);
        }
        eds::chest_median(&d5, NEAR_ER, None)
    };
    let gpu_med = cross(&|k| tex[k]);
    let cpu_med = cross(&|k| cpu[(pts[k][1] * W1 + pts[k][0]) as usize]);
    let med_ok = gpu_med.is_some() && gpu_med == cpu_med && gpu_med.is_some_and(|(d, sp)| d == d_of(2.0) && sp == 0.0);
    // after B: the depth holds B's clear (B's 0xC0 before-state held: no message), then ER's frame again
    harness.extend(g.drain_all());
    rep.add(
        copy_bad == 0 && b_seen == 0 && sample_bad == 0 && got.len() == want.len() && tex[eds::CHEST_AT] == d_of(2.0) && med_ok && eds::SAMPLE_N == 22,
        "ED0",
        format!(
            "copy {}x{} == CPU farthest-of-2x2: mismatches {copy_bad}/{}; B's 0.5 seen {b_seen}; {} texels == CPU: mismatches {sample_bad} (chest {:.5}; cross median {:?} == CPU {:?})",
            eds::half(W1, H1).0,
            eds::half(W1, H1).1,
            want.len(),
            eds::SAMPLE_N,
            tex[eds::CHEST_AT],
            gpu_med,
            cpu_med
        ),
    );
    // C6 round 7: a chest at the wall's right edge: the cross straddles it (one texel off the wall = an edge sample)
    {
        let pe = eds::sample_points(W1, H1, Some((1300, 500)));
        let edge = cross(&|k| cpu[(pe[k][1] * W1 + pe[k][0]) as usize]);
        rep.add(
            edge.is_some_and(|(d, sp)| d == d_of(2.0) && !(sp <= eds::SELF_EDGE_M)),
            "ED0e",
            format!("a chest cross over the wall's edge: median {edge:?} (the wall), spread above {} m = an edge sample, never judged", eds::SELF_EDGE_M),
        );
    }

    // ED2 first (the baselines): no prime == record_world
    let (base, _) = draw(&mut g, &mut r, &t, &[], None, true)?;
    let (none, p0) = draw(&mut g, &mut r, &t, &[], None, false)?;
    let gun = gun_px(&base);
    png("base", &base);
    let (mut r2, _) = renderer(&g, false)?;
    let (base2, _) = draw(&mut g, &mut r2, &t, &[], None, true)?;
    msgs.extend(g.drain_all());

    let bias = (0.03, 0.004);
    let (p_none2, pr2) = {
        let p = PrimeIn { tex: rd.texture(0).ok_or("tex")?, near: NEAR_ER, far: 0.0, bias_m: bias.0, bias_rel: bias.1 };
        draw(&mut g, &mut r2, &t, &[], Some(&p), false)?
    };
    msgs.extend(g.drain_all());
    // a full-screen wall at view z, copied, then primed at Present
    let mut primed_frame = |g: &mut Gpu, r: &mut Renderer, walls: &[(RECT, f32)], caps: &[Capsule], bias: (f32, f32), value: &mut u64, msgs: &mut Vec<String>, harness: &mut Vec<String>| -> Result<(Vec<u8>, bool), String> {
        er_frame(g, &d, walls)?;
        harness.extend(g.drain_all());
        rd.record(0, &d.res, true, None).map_err(|x| format!("record: {x}"))?;
        *value += 1;
        let v = *value;
        splice_run(g, &rd, 0, v, false)?;
        msgs.extend(g.drain_all());
        let p = PrimeIn { tex: rd.texture(0).ok_or("tex")?, near: NEAR_ER, far: 0.0, bias_m: bias.0, bias_rel: bias.1 };
        let out = draw(g, r, &t, caps, Some(&p), false)?;
        msgs.extend(g.drain_all());
        Ok(out)
    };
    rep.add(
        none == base && !p0 && gun > 1000 && p_none2 == base2 && !pr2 && !r2.has_er_depth() && r.has_er_depth(),
        "ED2",
        format!("no prime == record_world {} ({gun} gun px); a prime on a renderer without the pass ignored {} (primed {pr2})", none == base, p_none2 == base2),
    );

    // ED1: in front / behind / the right part
    let (front, pf) = primed_frame(&mut g, &mut r, &[(full(), d_of(2.0))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    png("ED1_front", &front);
    let (behind, pb) = primed_frame(&mut g, &mut r, &[(full(), d_of(4.0))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    let edge = (W1 / 2) as i32;
    let (right, _) = primed_frame(&mut g, &mut r, &[(RECT { left: edge, top: 0, right: W1 as i32, bottom: H1 as i32 }, d_of(2.0))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    png("ED1_right", &right);
    let (mut right_gun, mut left_diff, mut left_gun) = (0usize, 0usize, 0usize);
    for y in 0..H1 {
        for x in 0..W1 {
            let i = ((y * W1 + x) * 4) as usize;
            if x as i32 >= edge + 2 {
                right_gun += (right[i..i + 3] != KEY[..3]) as usize;
            } else if (x as i32) < edge - 2 {
                left_gun += (base[i..i + 3] != KEY[..3]) as usize;
                left_diff += (base[i..i + 4] != right[i..i + 4]) as usize;
            }
        }
    }
    rep.add(
        gun_px(&front) == 0 && pf && behind == base && pb && right_gun == 0 && left_diff == 0 && left_gun > 0,
        "ED1",
        format!("wall at 2 m (gun at 3 m): {} gun px; wall at 4 m: identical to unprimed {}; right wall: right gun px {right_gun}, left {left_gun} gun px with {left_diff} differing", gun_px(&front), behind == base),
    );

    // ED3: the bias edges against the gun's nearest point
    let z2 = zmin - 0.02;
    let z10 = zmin - 0.10;
    let (b2, _) = primed_frame(&mut g, &mut r, &[(full(), d_of(z2))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    let (b2_nobias, _) = primed_frame(&mut g, &mut r, &[(full(), d_of(z2))], &[], (0.0, 0.0), &mut value, &mut msgs, &mut harness)?;
    let (b10, _) = primed_frame(&mut g, &mut r, &[(full(), d_of(z10))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    rep.add(
        gun_px(&b2) > 0 && gun_px(&b2_nobias) == 0 && gun_px(&b10) == 0,
        "ED3",
        format!("gun nearest z {zmin:.3} m: wall 2 cm in front with the bias {} gun px (visible), no bias {}, wall 10 cm in front {}", gun_px(&b2), gun_px(&b2_nobias), gun_px(&b10)),
    );

    // ED4: the union (the left capsule of OC3 + an ER wall over the right part)
    let left_cap = Capsule { a: [-3.0, 1.5, -1.0], b: [-0.6, 1.5, -1.0], r: 0.5 };
    let wall_r = RECT { left: edge - 100, top: 0, right: W1 as i32, bottom: H1 as i32 };
    let (cap_only, _) = draw(&mut g, &mut r, &t, &[left_cap], None, false)?;
    msgs.extend(g.drain_all());
    let (wall_only, _) = primed_frame(&mut g, &mut r, &[(wall_r, d_of(2.0))], &[], bias, &mut value, &mut msgs, &mut harness)?;
    let (both, _) = primed_frame(&mut g, &mut r, &[(wall_r, d_of(2.0))], &[left_cap], bias, &mut value, &mut msgs, &mut harness)?;
    png("ED4_both", &both);
    rep.add(
        gun_px(&cap_only) > 0 && gun_px(&wall_only) > 0 && gun_px(&both) == 0,
        "ED4",
        format!("capsule only {} gun px, ER wall only {}, both {}", gun_px(&cap_only), gun_px(&wall_only), gun_px(&both)),
    );

    // ED5: the real detour on a scripted barrier sequence
    run_ed5(&mut g, &mut rep, &mut msgs)?;
    // ED8 (C6 round 6): two candidates, the control, the census, calls before any gate
    run_ed8(&mut g, &mut rep, &mut msgs)?;

    // ED7 (review 2026-10-21): a resized reader's fence starts at the last value handed out, so the old slot / sample
    // values are already passed on it and the next copy splices (a fence started at 0 would leave every slot busy)
    {
        let mut rd2 = DepthReader::new(&g.dev, W1, H1, value).map_err(|x| format!("ED7 DepthReader::new: {x}"))?;
        let rd0 = DepthReader::new(&g.dev, W1, H1, 0).map_err(|x| format!("ED7 DepthReader::new(0): {x}"))?;
        let _ = g.drain_all();
        let start_ok = rd2.completed() >= value;
        let control_busy = rd0.completed() < value;
        let cpu = er_frame(&mut g, &d, &[(wall, d_of(2.5))])?;
        harness.extend(g.drain_all());
        rd2.record(1, &d.res, true, None).map_err(|x| format!("ED7 record: {x}"))?;
        msgs.extend(g.drain_all());
        value += 1;
        splice_run(&mut g, &rd2, 1, value, false)?;
        msgs.extend(g.drain_all());
        let passed = rd2.completed() >= value;
        let got = read_copy(&mut g, rd2.texture(1).ok_or("tex")?)?;
        let want = eds::copy_ref(&cpu, W1, H1);
        let bad = got.iter().zip(&want).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
        rep.add(
            start_ok && control_busy && passed && bad == 0 && got.len() == want.len(),
            "ED7",
            format!("resized reader: fence starts at {} (old values passed {start_ok}; a fence from 0 leaves them busy {control_busy}); its next splice (value {value}) passed {passed}, copy mismatches {bad}/{}", value - 1, want.len()),
        );
    }

    let other = harness.iter().filter(|m| !m.contains("ClearDepthStencilView")).count();
    rep.add(
        msgs.is_empty() && other == 0,
        "infoqueue",
        format!(
            "debug-layer messages of our records / spliced execution / prime (any severity{}): {}{}; harness clear messages {} (other harness messages {other})",
            if a.gbv { ", GBV on" } else { "" },
            msgs.len(),
            msgs.first().map(|m| format!(" first: {}", m.chars().take(200).collect::<String>())).unwrap_or_default(),
            harness.len() - other
        ),
    );

    // ED6 (--gbv only): the negative control
    if a.gbv {
        g.begin(1)?;
        // SAFETY: recording into the harness's open list.
        unsafe { g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0xC0, 0x10, D3D12_RESOURCE_BARRIER_FLAG_NONE)]) };
        g.submit_wait()?;
        let _ = g.drain_all();
        rd.record(0, &d.res, true, None).map_err(|x| format!("ED6 record: {x}"))?;
        value += 1;
        splice_run(&mut g, &rd, 0, value, false)?;
        let bad = g.drain_errors();
        g.begin(1)?;
        // SAFETY: as above.
        unsafe { g.list.ResourceBarrier(&[tb(&d.res, SUB_ALL, 0x10, 0xC0, D3D12_RESOURCE_BARRIER_FLAG_NONE)]) };
        g.submit_wait()?;
        let _ = g.drain_all();
        rep.add(!bad.is_empty(), "ED6", format!("negative control (GBV): the copy of a depth left in DEPTH_WRITE reports {} error(s){}", bad.len(), bad.first().map(|m| format!(" first: {}", m.chars().take(160).collect::<String>())).unwrap_or_default()));
    }

    std::fs::write(a.out.join("erdepth_report.txt"), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    let ok = rep.fails == 0;
    println!("{} VMTEST ERDEPTH {}/{} cases", if ok { "PASS" } else { "FAIL" }, rep.lines.len() - rep.fails, rep.lines.len());
    Ok(ok)
}

/// A closed-able list of our own (ED5: three lists on their own allocators).
fn new_list(g: &Gpu, alloc: usize) -> Result<ID3D12GraphicsCommandList, String> {
    // SAFETY: plain COM creation; the list is closed at once.
    let l: ID3D12GraphicsCommandList = e(unsafe { g.dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &g.allocs[alloc], None::<&ID3D12PipelineState>) }, "CreateCommandList(ed5)")?;
    e(unsafe { l.Close() }, "Close(ed5 new)")?;
    Ok(l)
}

fn reset(g: &Gpu, l: &ID3D12GraphicsCommandList, alloc: usize) -> Result<(), String> {
    // SAFETY: the allocator's previous work finished (every ECL below is waited on).
    unsafe {
        e(g.allocs[alloc].Reset(), "Reset(ed5 alloc)")?;
        e(l.Reset(&g.allocs[alloc], None::<&ID3D12PipelineState>), "Reset(ed5 list)")
    }
}

/// The barriers through the REAL detour (the trampoline is the list's own slot-26 function: the original runs first).
fn via_detour(l: &ID3D12GraphicsCommandList, b: &[D3D12_RESOURCE_BARRIER]) {
    // SAFETY: the detour with TRAMP = this device's ResourceBarrier: exactly what the hooked vtable call does.
    unsafe { mw2er_render::er_depth::resource_barrier_detour(l.as_raw(), b.len() as u32, b.as_ptr()) };
}

fn ecl(g: &mut Gpu, lists: &[&ID3D12GraphicsCommandList]) -> Result<(), String> {
    let v: Vec<Option<ID3D12CommandList>> = lists.iter().map(|l| l.cast().ok()).collect();
    // SAFETY: closed lists; waited on below.
    unsafe { g.queue.ExecuteCommandLists(&v) };
    g.fence_val += 1;
    // SAFETY: as above.
    e(unsafe { g.queue.Signal(&g.fence, g.fence_val) }, "Signal(ed5)")?;
    g.wait(g.fence_val)
}

/// One ECL of the script (queue / thread 1 = the HAL queue and the render thread).
fn ein(q: usize, watch: usize, target: usize, lists: &[usize], armed: bool, seq: u64) -> EclIn<'_> {
    EclIn { queue: q, tid: 1, lists, watch, epoch: 1, armed, seq, judged: true, hal_queue: q, rt_tid: 1, target }
}

fn run_ed5(g: &mut Gpu, rep: &mut Report, msgs: &mut Vec<String>) -> Result<(), String> {
    let d = er_depth_tex(g)?;
    let (l1, l2, l3) = (new_list(g, 5)?, new_list(g, 6)?, new_list(g, 7)?);
    let _ = g.drain_all();
    // the trampoline: slot 26 of this device's list vtable (the debug layer's on this harness: the original either way)
    // SAFETY: a live COM object's first qword is its vtable; slot 26 is ResourceBarrier.
    let target = unsafe { *(*(l1.as_raw() as *const *const usize)).add(eds::SLOT_RESOURCE_BARRIER) };
    mw2er_render::er_depth::TRAMP.store(target, std::sync::atomic::Ordering::SeqCst);
    let watch = d.res.as_raw() as usize;
    mw2er_render::er_depth::WATCH.store(watch, std::sync::atomic::Ordering::SeqCst);
    mw2er_render::er_depth::ON.store(true, std::sync::atomic::Ordering::SeqCst);
    let fold = Fold::new();
    let vt_of = |l: usize| -> Option<usize> {
        // SAFETY: the harness's live lists only.
        Some(unsafe { *(*(l as *const *const usize)).add(eds::SLOT_RESOURCE_BARRIER) })
    };
    let q = g.queue.as_raw() as usize;
    let raw = |l: &ID3D12GraphicsCommandList| l.as_raw() as usize;
    let none = D3D12_RESOURCE_BARRIER_FLAG_NONE;
    // frame 1: L1 writes (ALL 0xC0 -> DEPTH_WRITE), clears, back per plane; L2 a split pair then DEPTH_WRITE -> 0xC0;
    // L3 the armed UI list (no barrier on the depth)
    reset(g, &l1, 5)?;
    via_detour(&l1, &[tb(&d.res, SUB_ALL, 0xC0, 0x10, none)]);
    // SAFETY: recording into our open list.
    unsafe { l1.ClearDepthStencilView(d.dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None) };
    via_detour(&l1, &[tb(&d.res, 0, 0x10, 0xC0, none), tb(&d.res, 1, 0x10, 0xC0, none)]);
    e(unsafe { l1.Close() }, "Close(L1)")?;
    reset(g, &l2, 6)?;
    via_detour(&l2, &[tb(&d.res, SUB_ALL, 0xC0, 0x10, D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY)]);
    via_detour(&l2, &[tb(&d.res, SUB_ALL, 0xC0, 0x10, D3D12_RESOURCE_BARRIER_FLAG_END_ONLY)]);
    via_detour(&l2, &[tb(&d.res, SUB_ALL, 0x10, 0xC0, none)]);
    e(unsafe { l2.Close() }, "Close(L2)")?;
    reset(g, &l3, 7)?;
    e(unsafe { l3.Close() }, "Close(L3)")?;
    let pend1 = mw2er_render::er_depth::TABLE.pending(raw(&l1));
    let pend2 = mw2er_render::er_depth::TABLE.pending(raw(&l2));
    msgs.extend(g.drain_all());
    let a1 = fold.ecl(&mw2er_render::er_depth::TABLE, &ein(q, watch, target, &[raw(&l1), raw(&l2)], false, 1), &vt_of);
    ecl(g, &[&l1, &l2])?;
    let at1 = fold.ecl(&mw2er_render::er_depth::TABLE, &ein(q, watch, target, &[raw(&l3)], true, 1), &vt_of);
    ecl(g, &[&l3])?;
    msgs.extend(g.drain_all());
    // frame 2: L1 re-recorded (0xC0 -> DEPTH_WRITE), executed alone; L2 re-recorded (DEPTH_WRITE -> 0xC0); L3 again
    reset(g, &l1, 5)?;
    via_detour(&l1, &[tb(&d.res, SUB_ALL, 0xC0, 0x10, none)]);
    e(unsafe { l1.Close() }, "Close(L1b)")?;
    fold.ecl(&mw2er_render::er_depth::TABLE, &ein(q, watch, target, &[raw(&l1)], false, 2), &vt_of);
    ecl(g, &[&l1])?;
    let q_mid = fold.q();
    reset(g, &l2, 6)?;
    via_detour(&l2, &[tb(&d.res, SUB_ALL, 0x10, 0xC0, none)]);
    e(unsafe { l2.Close() }, "Close(L2b)")?;
    fold.ecl(&mw2er_render::er_depth::TABLE, &ein(q, watch, target, &[raw(&l2)], false, 2), &vt_of);
    ecl(g, &[&l2])?;
    let at2 = fold.ecl(&mw2er_render::er_depth::TABLE, &ein(q, watch, target, &[raw(&l3)], true, 2), &vt_of);
    ecl(g, &[&l3])?;
    msgs.extend(g.drain_all());
    mw2er_render::er_depth::ON.store(false, std::sync::atomic::Ordering::SeqCst);
    mw2er_render::er_depth::WATCH.store(0, std::sync::atomic::Ordering::SeqCst);
    let s = fold.stats(&mw2er_render::er_depth::TABLE);
    let ok = pend1 == Some(([0xC0, 0xC0], [0xC0, 0xC0], 3))
        && pend2 == Some(([0xC0, 0xC0], [0xC0, 0xC0], 3))
        && a1.is_none()
        && at1 == Some(0xC0)
        && q_mid == [0x10, 0x10]
        && at2 == Some(0xC0)
        && fold.q() == [0xC0, 0xC0]
        && (s.lists, s.transitions, s.split, s.mismatch, s.queues, s.threads, s.frames, s.readable) == (4, 8, 2, 0, 1, 1, 2, 2)
        && s.vt_eq == s.vt_n
        && s.vt_n == 4
        && s.barriers == 8
        && mw2er_render::er_depth::CALLS.load(std::sync::atomic::Ordering::Relaxed) >= 7;
    rep.add(
        ok,
        "ED5",
        format!(
            "real detour (trampoline 0x{target:x}): L1 {pend1:?} L2 {pend2:?}; at_ui frame 1 {} frame 2 {}; Q after L1' {:?}; {}",
            at1.map_or("none".into(), eds::state_text),
            at2.map_or("none".into(), eds::state_text),
            q_mid,
            eds::state_text_line(watch, fold.q()[0], &s)
        ),
    );
    Ok(())
}

/// C6 round 6 (work/c6/depth6.md 4 item 7): the corrected probe on the real detour. Calls with nothing watched are
/// counted (the 20:57 run's dead-hook / idle ambiguity); then A (3D: ALL 0xC0 -> 0x10 -> 0xC0) and B (the UI list: plane
/// 1 0xC0 -> 0x10 -> 0xC0, the positive control) are watched, a third depth C (unwatched) gets 3D traffic too.
fn run_ed8(g: &mut Gpu, rep: &mut Report, msgs: &mut Vec<String>) -> Result<(), String> {
    use mw2er_render::er_depth::{CALLS, CENSUS, ON, TABLE, TABLE_B, TRAMP, WATCH, WATCH_B};
    use std::sync::atomic::Ordering::{Relaxed, SeqCst};
    let (da, db, dc) = (er_depth_tex(g)?, er_depth_tex(g)?, er_depth_tex(g)?);
    let (l1, l2) = (new_list(g, 5)?, new_list(g, 6)?);
    let _ = g.drain_all();
    // SAFETY: a live COM object's first qword is its vtable; slot 26 is ResourceBarrier.
    let target = unsafe { *(*(l1.as_raw() as *const *const usize)).add(eds::SLOT_RESOURCE_BARRIER) };
    TRAMP.store(target, SeqCst);
    let raw = |l: &ID3D12GraphicsCommandList| l.as_raw() as usize;
    let none = D3D12_RESOURCE_BARRIER_FLAG_NONE;
    let (ra, rb, rc) = (da.res.as_raw() as usize, db.res.as_raw() as usize, dc.res.as_raw() as usize);
    // the census is per process (keyed by the pointer): an earlier case's freed texture may share an address, so the
    // counts are compared against their values now
    let census_n = |r: usize| CENSUS.rows().iter().find(|x| x.res == r).map_or((0, 0), |x| (x.n, x.sub_all));
    let base = [ra, rb, rc].map(census_n);
    // 1) nothing watched (recording on, as the install sets it): every call counted, no table entry
    WATCH.store(0, SeqCst);
    WATCH_B.store(0, SeqCst);
    ON.store(true, SeqCst);
    let calls0 = CALLS.load(Relaxed);
    reset(g, &l1, 5)?;
    via_detour(&l1, &[tb(&dc.res, SUB_ALL, 0xC0, 0x10, none)]);
    via_detour(&l1, &[tb(&dc.res, SUB_ALL, 0x10, 0xC0, none)]);
    e(unsafe { l1.Close() }, "Close(ED8 L1u)")?;
    let unwatched_calls = CALLS.load(Relaxed) - calls0;
    let unwatched_tables = (TABLE.pending(raw(&l1)), TABLE_B.pending(raw(&l1)));
    msgs.extend(g.drain_all());
    ecl(g, &[&l1])?;
    msgs.extend(g.drain_all());
    // 2) A and B watched, 3 frames: L1 = the 3D list (A and C), L2 = the armed UI list (B's plane-1 pair)
    WATCH.store(ra, SeqCst);
    WATCH_B.store(rb, SeqCst);
    let (fa, fb) = (Fold::new(), Fold::new());
    let vt_of = |l: usize| -> Option<usize> {
        // SAFETY: the harness's live lists only.
        Some(unsafe { *(*(l as *const *const usize)).add(eds::SLOT_RESOURCE_BARRIER) })
    };
    let q = g.queue.as_raw() as usize;
    let mut at = Vec::new();
    for seq in 1..=3u64 {
        reset(g, &l1, 5)?;
        via_detour(&l1, &[tb(&da.res, SUB_ALL, 0xC0, 0x10, none), tb(&dc.res, SUB_ALL, 0xC0, 0x10, none)]);
        via_detour(&l1, &[tb(&da.res, SUB_ALL, 0x10, 0xC0, none), tb(&dc.res, SUB_ALL, 0x10, 0xC0, none)]);
        e(unsafe { l1.Close() }, "Close(ED8 L1)")?;
        reset(g, &l2, 6)?;
        via_detour(&l2, &[tb(&db.res, 1, 0xC0, 0x10, none)]);
        via_detour(&l2, &[tb(&db.res, 1, 0x10, 0xC0, none)]);
        e(unsafe { l2.Close() }, "Close(ED8 L2)")?;
        msgs.extend(g.drain_all());
        fa.ecl(&TABLE, &ein(q, ra, target, &[raw(&l1)], false, seq), &vt_of);
        fb.ecl(&TABLE_B, &ein(q, rb, target, &[raw(&l1)], false, seq), &vt_of);
        ecl(g, &[&l1])?;
        let a = fa.ecl(&TABLE, &ein(q, ra, target, &[raw(&l2)], true, seq), &vt_of);
        let b = fb.ecl(&TABLE_B, &ein(q, rb, target, &[raw(&l2)], true, seq), &vt_of);
        at.push((a, b));
        ecl(g, &[&l2])?;
        msgs.extend(g.drain_all());
    }
    ON.store(false, SeqCst);
    WATCH.store(0, SeqCst);
    WATCH_B.store(0, SeqCst);
    let (sa, sb) = (fa.stats(&TABLE), fb.stats(&TABLE_B));
    let rows = CENSUS.rows();
    let row = |r: usize, k: usize| {
        rows.iter().find(|x| x.res == r).copied().map(|mut x| {
            x.n -= base[k].0;
            x.sub_all -= base[k].1;
            x
        })
    };
    let (ca, cb, cc) = (row(ra, 0), row(rb, 1), row(rc, 2));
    let ok = unwatched_calls == 2
        && unwatched_tables == (None, None)
        && at.iter().all(|(a, b)| *a == Some(0xC0) && *b == Some(eds::UNKNOWN))
        && (sa.lists, sa.transitions, sa.frames, sa.readable, sa.mismatch, sa.armed) == (3, 6, 3, 3, 0, 3)
        && (sb.lists, sb.transitions, sb.frames, sb.unknown, sb.mismatch, sb.armed, sb.ctl_hits) == (3, 6, 3, 3, 0, 3, 2)
        && fb.q() == [eds::UNKNOWN, 0xC0]
        && ca.is_some_and(|r| r.n == 6 && r.sub_all == 6)
        && cb.is_some_and(|r| r.n == 6 && r.sub_all == 0 && r.last[1] == (0x10, 0xC0))
        && cc.is_some_and(|r| r.n == 8 && r.states & 0xD0 == 0xD0);
    rep.add(
        ok,
        "ED8",
        format!(
            "calls with nothing watched {unwatched_calls} (tables {unwatched_tables:?}); at_ui A/B {at:?}; A lists={} trans={} readable={}/{}; B lists={} trans={} control={}/{} q={:?}; census A={:?} B={:?} C={:?}",
            sa.lists,
            sa.transitions,
            sa.readable,
            sa.frames,
            sb.lists,
            sb.transitions,
            sb.ctl_hits,
            sb.armed,
            fb.q(),
            ca.map(|r| r.n),
            cb.map(|r| r.n),
            cc.map(|r| r.n)
        ),
    );
    Ok(())
}
