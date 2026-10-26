//! C4 vmtest cases (docs/C4_UI_DESIGN.md 8.1 item 5): the early pass through the DLL's own `EarlyPass` /
//! `Renderer::record_*_early`, spliced like hudhook patch 0002 does it, against an "ER" UI target.
//!
//!   vmtest --c4 [--gbv] [--warp] [--out <dir>] [--assets <dir>]
//!       An "ER" texture in `0xC0` (PIXEL | NON_PIXEL_SHADER_RESOURCE) on a direct queue; an ER-like list A that
//!       barriers it `0xC0 -> RENDER_TARGET`, draws a "UI" box (a rect clear: Scaleform never clears the target, the
//!       box only stands for UI pixels) and barriers it back; our early list spliced AHEAD of A (ExecuteCommandLists of
//!       ours, Signal of our fence, then A's call, as patch 0002). With the D3D12 debug layer (+ GBV with --gbv):
//!         C4.1 R10G10B10A2_UNORM, C4.2 R8G8B8A8_UNORM, C4.3 R8G8B8A8_UNORM_SRGB: zero debug-layer messages, our gun
//!              pixels present outside the UI box and overdrawn inside it (the gun under the "UI"), the state sequence
//!              intact (the debug layer validates every barrier's before-state)
//!         C4.4 the ui_layer PSO variant: gun pixels have alpha 255 on an alpha-0 layer, the rest keeps alpha 0
//!         C4.5 a refused target (R16G16B16A16_FLOAT): the format table refuses it, nothing is recorded
//!         C4.6 a busy slot: three slots in flight -> no free slot at once (no wait)
//!         C4.7 a resize: the old depth is released only after the fence of its last list passed
//!       Images: `<out>\c4_<case>.png`. Exit 0 = PASS.

use std::path::Path;

use mw2er_core::c4::{self, UiPath};
use mw2er_core::vm_pose::{PoseScratch, TEST_POSES};
use mw2er_render::{DebugMode, EarlyMode, EarlyPass, EarlyTarget, FrameParams, LightParams, OutputMode, Renderer, UI_TARGET_STATE, Variant};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use super::{Gpu, H1, KEY_F, NUM_FRAMES, Report, Scene, W1, barrier, bpp, e, heap, load, read_rows, readback_for, record_copy, write_png};

// C4.1 P2 cases (docs/C4_1_DESIGN.md 2.4), in the same `--c4` run:
//   P2.1 R8G8B8A8_UNORM "transmit" layer (empty (0,0,0,1)), P2.2 "premul" (empty (0,0,0,0)), P2.3 B8G8R8A8_UNORM:
//        `LayerSampler` (the DLL's own) records 16 texels of the layer in 0xC0 with NO barrier on it, spliced ahead of
//        list A (whose first command is 0xC0 -> RENDER_TARGET): the texels equal a CPU readback taken before, list A's
//        box is not seen (queue order) but is in the layer after (A's before-state held), the vote decides the
//        convention, and the debug layer stores ZERO messages of any severity (+ GBV with --gbv)
//   P2.4 a format the shader does not read is refused before any GPU work
//
// C4.1 PASS I41 cases (docs/C4_1_DESIGN.md 3.5 / 3.6 / 6), in the same `--c4` run, on an R8G8B8A8_UNORM "ER layer"
// cleared to its convention's empty value (its optimized clear value), left in 0xC0:
//   I41.1 transmit / I41.2 premul: our early gun list (the DLL's EarlyPass + record_gun_early in that convention, with
//        `begin_prepared`: nothing created) is RECORDED ON A SECOND THREAD (ER's render thread is not the present
//        thread), spliced ahead of an ER-like list A on the direct queue (0xC0 -> RT, a half-transparent "UI" quad
//        drawn "over" in the convention across the left half of the gun, RT -> 0xC0). A CPU composite of the layer
//        over a constant "scene" (`c4::composite41`: transmit scene*a + ui, premul scene*(1-a) + ui) must show the
//        scene where the layer is empty, the gun opaque, and the UI over the gun (ui*b + gun*(1-b)); the debug layer
//        stores ZERO messages of any severity (list A draws, it never clears)
//   I41.3 the canary (`record_canary`, one texel at (2, 2), opaque in the convention) ahead of list A, read back after
//        A by the DLL's LayerSampler (as UiEnd's readback does, the layer in 0xC0, no barrier on it): `canary ok`
//   I41.4 list A clears the whole layer after the canary: `canary cleared` (the injection would trip)
//   I41.5 the convention re-check on the layer after I41.1 / I41.2's frame (16 grid texels): votes its own convention
//   I41.6 the persistence check (review C4.1) after I41.3's canary passed: the next frame's list draws NOTHING (the
//        DLL's check list: two barriers), list A runs again and the UiEnd readback reads the canary texel. A list A
//        that does not clear the layer: `canary persisted` (the injection would trip); a list A that clears the layer
//        at the start of its UI frame (an ER that clears between frames): `canary gone` (the gun may start)

pub struct C4Args<'a> {
    pub out: &'a Path,
    pub assets: &'a Path,
    pub gbv: bool,
    pub warp: bool,
}

/// The "UI" colour of list A's box.
const UI_F: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

struct ErTex {
    res: ID3D12Resource,
    rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    fmt: DXGI_FORMAT,
    rb: ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

/// An "ER" UI target: created in RENDER_TARGET, cleared to `clear`, then left in `0xC0` (as ER's between UI frames).
fn er_texture(g: &mut Gpu, fmt: DXGI_FORMAT, rtv_index: usize, clear: [f32; 4]) -> Result<ErTex, String> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: W1 as u64,
        Height: H1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };
    let cv = D3D12_CLEAR_VALUE { Format: fmt, Anonymous: D3D12_CLEAR_VALUE_0 { Color: clear } };
    let mut res: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(
        unsafe { g.dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET, Some(&cv), &mut res) },
        "CreateCommittedResource(er texture)",
    )?;
    let res = res.ok_or("er texture null")?;
    // SAFETY: an RTV into the harness's own heap slot (rtv_index < 16).
    let rtv = unsafe {
        let h0 = g.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + rtv_index * g.rtv_inc };
        g.dev.CreateRenderTargetView(&res, None, rtv);
        rtv
    };
    g.begin(0)?;
    // SAFETY: recording into the harness's open list.
    unsafe {
        g.list.ClearRenderTargetView(rtv, &clear, None);
        g.list.ResourceBarrier(&[barrier(&res, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]);
    }
    g.submit_wait()?;
    let (rb, layout) = readback_for(g, &desc)?;
    Ok(ErTex { res, rtv, fmt, rb, layout })
}

/// List A (ER's UI frame): `0xC0 -> 4`, the "UI" box, `4 -> 0xC0`. Recorded into the harness list, closed.
fn record_er_list(g: &Gpu, t: &ErTex, boxr: RECT) -> Result<(), String> {
    g.begin(1)?;
    // SAFETY: recording into the harness's open list; the texture lives until the wait.
    unsafe {
        g.list.ResourceBarrier(&[barrier(&t.res, UI_TARGET_STATE, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        g.list.ClearRenderTargetView(t.rtv, &UI_F, Some(&[boxr]));
        g.list.ResourceBarrier(&[barrier(&t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]);
        e(g.list.Close(), "Close(A)")
    }
}

/// hudhook patch 0002's splice at `at = 0`: our list, Signal(fence, value), then ER's call unchanged.
fn splice_submit(g: &Gpu, ours: &ID3D12GraphicsCommandList, fence: &ID3D12Fence, value: u64, er: Option<&ID3D12GraphicsCommandList>) -> Result<(), String> {
    let o: ID3D12CommandList = e(ours.cast(), "cast ours")?;
    // SAFETY: closed lists of ours / the harness; the objects live until the waits below.
    unsafe {
        g.queue.ExecuteCommandLists(&[Some(o)]);
        e(g.queue.Signal(fence, value), "Signal(early)")?;
        if let Some(a) = er {
            g.queue.ExecuteCommandLists(&[Some(e(a.cast(), "cast A")?)]);
        }
    }
    Ok(())
}

/// The target's pixels after the frame (copied with `0xC0 -> COPY_SOURCE -> 0xC0`).
fn read_back(g: &mut Gpu, t: &ErTex) -> Result<Vec<u8>, String> {
    g.begin(2)?;
    record_copy(g, &t.res, &t.rb, t.layout, UI_TARGET_STATE);
    g.submit_wait()?;
    read_rows(&t.rb, &t.layout, W1, H1, bpp(t.fmt))
}

fn px_is(px: &[u8], i: usize, fmt: DXGI_FORMAT, c: [f32; 4]) -> bool {
    match fmt {
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let v = u32::from_le_bytes([px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]]);
            [0, 10, 20].iter().zip(c).all(|(sh, f)| ((v >> sh) & 0x3ff) == (f * 1023.0).round() as u32)
        }
        _ => (0..3).all(|k| px[i * 4 + k] == (c[k] * 255.0).round() as u8),
    }
}

/// One gun frame through the early pass into `t` (slot / fence value), optionally followed by list A's UI box.
#[allow(clippy::too_many_arguments)]
fn early_frame(g: &mut Gpu, r: &Renderer, ep: &mut EarlyPass, sc: &mut Scene, t: &ErTex, ui_layer: bool, slot: usize, value: u64, boxr: Option<RECT>) -> Result<(), String> {
    let rule = c4::format_rule(t.fmt.0 as u32, if ui_layer { UiPath::Layer } else { UiPath::Scene }, true).map_err(|m| format!("format refused: {}", m.label()))?;
    let (out, paper) = match rule.out {
        c4::EarlyOut::Srgb => (OutputMode::Srgb, 200.0),
        c4::EarlyOut::Linear => (OutputMode::Scrgb, 80.0),
    };
    let (pal, vp, view, _, _) = sc.pose(&TEST_POSES[0], W1, H1);
    let et = EarlyTarget { resource: &t.res, format: t.fmt, width: W1, height: H1, mode: if ui_layer { EarlyMode::Premul } else { EarlyMode::Scene } };
    ep.begin(slot, value, &et).map_err(|x| format!("begin: {x}"))?;
    let fp = FrameParams { palette: &pal, view_proj: vp, view, light: LightParams::default(), out, paper_white_nits: paper, debug: DebugMode::None, variant: Variant::default(), timing: false };
    r.record_gun_early(ep, slot, &et, 0, 0, &fp).map_err(|x| format!("record_gun_early: {x}"))?;
    ep.finish(slot, &et).map_err(|x| format!("finish: {x}"))?;
    let ours = ep_list(ep, slot)?;
    let er = match boxr {
        Some(b) => {
            record_er_list(g, t, b)?;
            Some(g.list.clone())
        }
        None => None,
    };
    splice_submit(g, &ours, &ep_fence(ep)?, value, er.as_ref())?;
    // wait on our fence (and the harness's own queue order behind it)
    g.fence_val += 1;
    // SAFETY: signalling the harness fence on its queue after both calls.
    e(unsafe { g.queue.Signal(&g.fence, g.fence_val) }, "Signal(harness)")?;
    g.wait(g.fence_val)
}

fn ep_list(ep: &EarlyPass, slot: usize) -> Result<ID3D12GraphicsCommandList, String> {
    let raw = ep.list_raw(slot) as *mut std::ffi::c_void;
    // SAFETY: the pass's own list, alive while `ep` is; borrowed then cloned (AddRef).
    unsafe { ID3D12GraphicsCommandList::from_raw_borrowed(&raw) }.cloned().ok_or_else(|| "no early list".to_string())
}

fn ep_fence(ep: &EarlyPass) -> Result<ID3D12Fence, String> {
    let raw = ep.fence_raw() as *mut std::ffi::c_void;
    // SAFETY: as above.
    unsafe { ID3D12Fence::from_raw_borrowed(&raw) }.cloned().ok_or_else(|| "no early fence".to_string())
}

/// The bounding box of the pixels that differ from `key` (the gun).
fn bbox(px: &[u8], fmt: DXGI_FORMAT, key: [f32; 4]) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..H1 {
        for x in 0..W1 {
            if !px_is(px, (y * W1 + x) as usize, fmt, key) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((a, c, d, f)) => (a.min(x), c.min(y), d.max(x), f.max(y)),
                });
            }
        }
    }
    b
}

fn to_png(px: &[u8], fmt: DXGI_FORMAT) -> Vec<u8> {
    match fmt {
        DXGI_FORMAT_R10G10B10A2_UNORM => px
            .chunks(4)
            .flat_map(|c| {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                [(v & 0x3ff) >> 2, ((v >> 10) & 0x3ff) >> 2, ((v >> 20) & 0x3ff) >> 2, 255].map(|x| x as u8)
            })
            .collect(),
        _ => px.chunks(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
    }
}

pub fn run_c4(a: &C4Args<'_>) -> Result<bool, String> {
    std::fs::create_dir_all(a.out).map_err(|x| x.to_string())?;
    let assets = load(a.assets)?;
    let bones = assets.skel.bones.len();
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, 0).map_err(|x| x.to_string())?;
    r.upload(&assets).map_err(|x| format!("upload: {x}"))?;
    let mut sc = Scene { assets, scratch: PoseScratch::new(bones) };
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let _ = g.drain_errors();

    // C4.1-C4.3: the gun under the "UI" for the three SDR formats of the table
    let mut value = 0u64;
    for (case, fmt) in [("C4.1", DXGI_FORMAT_R10G10B10A2_UNORM), ("C4.2", DXGI_FORMAT_R8G8B8A8_UNORM), ("C4.3", DXGI_FORMAT_R8G8B8A8_UNORM_SRGB)] {
        let mut ep = r.create_early(fmt, EarlyMode::Scene, W1, H1).map_err(|x| format!("{case} create_early: {x}"))?;
        // a frame without UI finds the gun's box; the next frame puts the "UI" box over its left half
        let t = er_texture(&mut g, fmt, 0, KEY_F)?;
        value += 1;
        early_frame(&mut g, &r, &mut ep, &mut sc, &t, false, 0, value, None)?;
        let px0 = read_back(&mut g, &t)?;
        let Some((x0, y0, x1, y1)) = bbox(&px0, fmt, KEY_F) else {
            rep.add(false, case, format!("{}: no gun pixels drawn by the early list", mw2er_render::format_name(fmt)));
            continue;
        };
        let boxr = RECT { left: x0 as i32, top: y0 as i32, right: ((x0 + x1) / 2) as i32, bottom: (y1 + 1) as i32 };
        let t2 = er_texture(&mut g, fmt, 1, KEY_F)?;
        value += 1;
        early_frame(&mut g, &r, &mut ep, &mut sc, &t2, false, 1, value, Some(boxr))?;
        let px = read_back(&mut g, &t2)?;
        let errs = g.drain_errors();
        let (mut gun_right, mut ui_inside, mut inside) = (0usize, 0usize, 0usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = (y * W1 + x) as usize;
                let gun0 = !px_is(&px0, i, fmt, KEY_F);
                if (x as i32) < boxr.right {
                    inside += 1;
                    ui_inside += px_is(&px, i, fmt, UI_F) as usize;
                } else if gun0 && !px_is(&px, i, fmt, KEY_F) && !px_is(&px, i, fmt, UI_F) {
                    gun_right += 1;
                }
            }
        }
        let _ = write_png(&a.out.join(format!("c4_{}.png", case.replace('.', "_"))), &to_png(&px, fmt), W1, H1);
        let ok = errs.is_empty() && gun_right > 0 && ui_inside == inside && completed(&ep) >= value;
        rep.add(ok, case, format!(
            "{}: gun box ({x0},{y0})-({x1},{y1}); UI over the gun inside the box {ui_inside}/{inside} px, gun kept right of it {gun_right} px; fence {}>={value}; debug-layer messages {}{}",
            mw2er_render::format_name(fmt),
            completed(&ep),
            errs.len(),
            errs.first().map(|m| format!(" first: {m}")).unwrap_or_default()
        ));
    }

    // C4.4: the ui_layer variant on an alpha-0 layer: alpha 255 exactly on the gun's pixels
    {
        let fmt = DXGI_FORMAT_R8G8B8A8_UNORM;
        let mut ep = r.create_early(fmt, EarlyMode::Premul, W1, H1).map_err(|x| format!("C4.4 create_early: {x}"))?;
        // the key colour with alpha 0 (an empty UI layer): gun pixels = not the key
        let t = er_texture(&mut g, fmt, 2, [1.0, 0.0, 1.0, 0.0])?;
        value += 1;
        early_frame(&mut g, &r, &mut ep, &mut sc, &t, true, 0, value, None)?;
        let px = read_back(&mut g, &t)?;
        let errs = g.drain_errors();
        let (mut gun, mut gun_a, mut rest_a) = (0usize, 0usize, 0usize);
        for i in 0..(W1 * H1) as usize {
            let key = px[i * 4..i * 4 + 3] == [255, 0, 255];
            if !key {
                gun += 1;
                gun_a += (px[i * 4 + 3] == 255) as usize;
            } else if px[i * 4 + 3] != 0 {
                rest_a += 1;
            }
        }
        let ok = errs.is_empty() && gun > 0 && gun_a * 100 >= gun * 99 && rest_a == 0;
        rep.add(ok, "C4.4", format!("ui_layer R8G8B8A8_UNORM: gun px {gun}, alpha 255 on {gun_a}; alpha set off the gun {rest_a}; debug-layer messages {}", errs.len()));
    }

    // C4.5: a refused FLOAT target never reaches the record
    {
        let refused = c4::format_rule(DXGI_FORMAT_R16G16B16A16_FLOAT.0 as u32, UiPath::Scene, true);
        let typeless = c4::format_rule(DXGI_FORMAT_R10G10B10A2_TYPELESS.0 as u32, UiPath::Scene, true);
        rep.add(refused == Err(c4::Miss::Format) && typeless == Err(c4::Miss::Format), "C4.5", format!("R16G16B16A16_FLOAT -> {refused:?}, R10G10B10A2_TYPELESS -> {typeless:?} (refused: drawn at Present)"));
    }

    // C4.6: three slots in flight (values reserved, fence behind): no free slot, decided without a wait
    {
        let mut ep = r.create_early(DXGI_FORMAT_R8G8B8A8_UNORM, EarlyMode::Scene, W1, H1).map_err(|x| format!("C4.6 create_early: {x}"))?;
        let base = completed(&ep);
        let t0 = std::time::Instant::now();
        let busy = c4::pick_slot(&[base + 1, base + 2, base + 3], base, 0);
        let free = c4::pick_slot(&[base + 1, base, base + 3], base, 0);
        let us = t0.elapsed().as_micros();
        // and the real pass: a slot whose list has not run yet keeps its value above the fence
        let fence = ep_fence(&ep)?;
        let t = er_texture(&mut g, DXGI_FORMAT_R8G8B8A8_UNORM, 3, KEY_F)?;
        let et = EarlyTarget { resource: &t.res, format: t.fmt, width: W1, height: H1, mode: EarlyMode::Scene };
        ep.begin(0, base + 1, &et).map_err(|x| x.to_string())?;
        ep.finish(0, &et).map_err(|x| x.to_string())?;
        let not_yet = completed(&ep) < base + 1;
        splice_submit(&g, &ep_list(&ep, 0)?, &fence, base + 1, None)?;
        mw2er_render::wait_fence(&fence, base + 1).map_err(|x| x.to_string())?;
        let errs = g.drain_errors();
        rep.add(busy.is_none() && free == Some(1) && us < 1000 && not_yet && errs.is_empty(), "C4.6", format!("all in flight -> {busy:?}, slot 1 completed -> {free:?} ({us} us, no wait); unsubmitted slot above the fence: {not_yet}; debug-layer messages {}", errs.len()));
    }

    // C4.7: a resize retires the old depth by fence value: kept while its list is pending, released after
    {
        let fmt = DXGI_FORMAT_R8G8B8A8_UNORM;
        let mut ep = r.create_early(fmt, EarlyMode::Scene, W1, H1).map_err(|x| format!("C4.7 create_early: {x}"))?;
        let fence = ep_fence(&ep)?;
        let base = completed(&ep);
        let t = er_texture(&mut g, fmt, 4, KEY_F)?;
        let et = EarlyTarget { resource: &t.res, format: fmt, width: W1, height: H1, mode: EarlyMode::Scene };
        // frame 1 at the full size (recorded, not yet submitted), frame 2 at half the size: the first depth retires
        ep.begin(0, base + 1, &et).map_err(|x| x.to_string())?;
        ep.finish(0, &et).map_err(|x| x.to_string())?;
        let small = EarlyTarget { resource: &t.res, format: fmt, width: W1 / 2, height: H1 / 2, mode: EarlyMode::Scene };
        ep.begin(1, base + 2, &small).map_err(|x| x.to_string())?;
        ep.finish(1, &small).map_err(|x| x.to_string())?;
        let early_release = ep.release_retired();
        splice_submit(&g, &ep_list(&ep, 0)?, &fence, base + 1, None)?;
        splice_submit(&g, &ep_list(&ep, 1)?, &fence, base + 2, None)?;
        mw2er_render::wait_fence(&fence, base + 2).map_err(|x| x.to_string())?;
        let released = ep.release_retired();
        let errs = g.drain_errors();
        rep.add(early_release == 0 && released == 1 && errs.is_empty(), "C4.7", format!("old depth released while its list pending: {early_release}, after the fence: {released}; debug-layer messages {}", errs.len()));
    }

    // P2.1-P2.3 (C4.1 probe P2, docs/C4_1_DESIGN.md 2.4): the layer sample spliced ahead of list A, no barrier on the
    // "ER" layer; every debug-layer message of any severity counts
    run_p2(&mut g, &mut rep, a.gbv)?;

    // I41.1-I41.5 (C4.1 PASS I41, docs/C4_1_DESIGN.md 3.5 / 3.6 / 6): the UI-thread early pass into an "ER layer"
    run_i41(&mut g, &r, &mut sc, a.out, &mut rep)?;

    let ok = rep.fails == 0;
    println!("{} VMTEST C4 {}/{} cases", if ok { "PASS" } else { "FAIL" }, rep.lines.len() - rep.fails, rep.lines.len());
    Ok(ok)
}

/// A rect clear of `color` into the layer, as an earlier UI frame of ER's would leave it (`0xC0 -> 4 -> 0xC0`).
fn er_ui_box(g: &mut Gpu, t: &ErTex, boxr: RECT, color: [f32; 4]) -> Result<(), String> {
    g.begin(1)?;
    // SAFETY: recording into the harness's open list; the texture lives until the wait.
    unsafe {
        g.list.ResourceBarrier(&[barrier(&t.res, UI_TARGET_STATE, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        g.list.ClearRenderTargetView(t.rtv, &color, Some(&[boxr]));
        g.list.ResourceBarrier(&[barrier(&t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]);
    }
    g.submit_wait()
}

/// One readback texel as RGBA bytes (B8G8R8A8 memory is swizzled back).
fn rgba_at(px: &[u8], fmt: DXGI_FORMAT, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W1 + x) * 4) as usize;
    let c = [px[i], px[i + 1], px[i + 2], px[i + 3]];
    if fmt == DXGI_FORMAT_B8G8R8A8_UNORM { [c[2], c[1], c[0], c[3]] } else { c }
}

fn to_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// P2: four samples per case through the DLL's own `LayerSampler`, each spliced exactly like patch 0002 (ours, our
/// Signal, then list A, whose first command is `0xC0 -> RENDER_TARGET` on the layer). Checks: the sample equals a
/// CPU readback of the layer taken before (A's box over every point is NOT seen: queue order), A's box is in the layer
/// afterwards (A's barrier before-state `0xC0` held: our list left the state alone), the convention vote, and zero
/// debug-layer messages of any severity.
fn run_p2(g: &mut Gpu, rep: &mut Report, gbv: bool) -> Result<(), String> {
    let mut s = mw2er_render::LayerSampler::new(&g.dev).map_err(|x| format!("LayerSampler::new: {x}"))?;
    let pts = c4::sample_points41(W1, H1);
    // the earlier UI frame's box over points 0 and 1 (x 768 / 896, y 324): not empty, never votes
    let prior = RECT { left: 760, top: 318, right: 900, bottom: 330 };
    let mut value = 0u64;
    let cases: [(&str, DXGI_FORMAT, [f32; 4], [f32; 4], Option<c4::LayerAlpha>); 3] = [
        ("P2.1", DXGI_FORMAT_R8G8B8A8_UNORM, [0.0, 0.0, 0.0, 1.0], [0.8, 0.6, 0.4, 0.5], Some(c4::LayerAlpha::Transmit)),
        ("P2.2", DXGI_FORMAT_R8G8B8A8_UNORM, [0.0, 0.0, 0.0, 0.0], [0.4, 0.2, 0.8, 0.5], Some(c4::LayerAlpha::Premul)),
        ("P2.3", DXGI_FORMAT_B8G8R8A8_UNORM, [0.0, 0.0, 0.0, 1.0], [1.0, 0.5, 0.25, 0.75], Some(c4::LayerAlpha::Transmit)),
    ];
    // A's box covers the whole sample window (x 0.40..0.60, y 0.30..0.42) and more
    let a_box = RECT { left: (W1 as f32 * 0.38) as i32, top: (H1 as f32 * 0.28) as i32, right: (W1 as f32 * 0.62) as i32, bottom: (H1 as f32 * 0.44) as i32 };
    for (case, fmt, empty, ui, want) in cases {
        let mut tally = c4::AlphaTally::default();
        let (mut mism, mut a_seen, mut a_after, mut msgs, mut harness) = (0usize, 0usize, 0usize, Vec::new(), Vec::<String>::new());
        let mut first_px = [[0u8; 4]; 4];
        let mut empty0 = 0u32;
        for k in 0..4 {
            let t = er_texture(g, fmt, 5, empty)?;
            if k == 0 {
                er_ui_box(g, &t, prior, ui)?;
            }
            let before = read_back(g, &t)?;
            // the harness's own set-up (its rect clears differ from the texture's optimized clear value: a WARNING
            // about the harness, emitted when the clear is recorded) is drained here and below and never counted
            harness.extend(g.drain_all());
            s.record(&t.res, fmt, &pts).map_err(|x| format!("{case} record: {x}"))?;
            msgs.extend(g.drain_all());
            record_er_list(g, &t, a_box)?;
            harness.extend(g.drain_all());
            value += 1;
            splice_submit(g, s.list(), s.fence(), value, Some(&g.list.clone()))?;
            g.fence_val += 1;
            // SAFETY: signalling the harness fence on its queue after both calls.
            e(unsafe { g.queue.Signal(&g.fence, g.fence_val) }, "Signal(harness)")?;
            g.wait(g.fence_val)?;
            if s.completed() < value {
                return Err(format!("{case}: sample fence {} < {value}", s.completed()));
            }
            // execution: our list, our Signal, list A (its 0xC0 -> RENDER_TARGET before-state validated here)
            msgs.extend(g.drain_all());
            let got = s.read().map(c4::unpack_px);
            let after = read_back(g, &t)?;
            harness.extend(g.drain_all());
            for (i, p) in pts.iter().enumerate() {
                let want_px = rgba_at(&before, fmt, p[0], p[1]);
                mism += (got[i] != want_px) as usize;
                a_seen += (got[i] == to_u8(UI_F)) as usize;
                a_after += (rgba_at(&after, fmt, p[0], p[1]) == to_u8(UI_F)) as usize;
                tally.add(got[i]);
            }
            if k == 0 {
                first_px.copy_from_slice(&got[..4]);
                empty0 = got.iter().filter(|p| c4::empty_alpha(**p).is_some()).count() as u32;
            }
        }
        let ui_px = to_u8(ui);
        // the harness messages may only be that clear-value warning (anything else fails the case too)
        let other = harness.iter().filter(|m| !m.contains("ClearRenderTargetView: The clear values do not match")).count();
        let ok = msgs.is_empty() && other == 0 && mism == 0 && a_seen == 0 && a_after == 64 && tally.conv() == want && first_px[0] == ui_px && tally.empty == 62;
        rep.add(ok, case, format!(
            "{} empty={:?} prior UI {:?}: 4 samples x 16 texels == the CPU readback before list A: mismatches {mism}; A's box seen by the sample {a_seen} (queue order), in the layer after A {a_after}/64 (A's 0xC0 before-state held); {}; px {:?}; harness clear-value warnings {} (other harness messages {other}); debug-layer messages of our record + the spliced execution (any severity{}) {}{}",
            mw2er_render::format_name(fmt),
            to_u8(empty),
            ui_px,
            c4::fmt41_sample(4, empty0, &tally, &first_px),
            first_px,
            harness.len() - other,
            if gbv { ", GBV on" } else { "" },
            msgs.len(),
            msgs.first().map(|m| format!(" first: {m}")).unwrap_or_default()
        ));
    }
    // the record refuses a format the shader does not read (no GPU work, no message)
    let t = er_texture(g, DXGI_FORMAT_R8G8B8A8_UNORM, 5, [0.0; 4])?;
    let _ = g.drain_all();
    let refused = s.record(&t.res, DXGI_FORMAT_R10G10B10A2_UNORM, &pts);
    let msgs = g.drain_all();
    rep.add(refused.is_err() && msgs.is_empty() && !c4::sample_format_ok41(DXGI_FORMAT_R10G10B10A2_UNORM.0 as u32), "P2.4", format!("R10G10B10A2_UNORM refused before recording: {refused:?}; debug-layer messages {}", msgs.len()));
    // P2.5 (--gbv only): the negative control. The same record with the layer left in RENDER_TARGET must be reported by
    // GPU-based validation, so P2.1-P2.3's zero count is a real check of the 0xC0 read, not a blind spot.
    if gbv {
        g.begin(1)?;
        // SAFETY: recording into the harness's open list.
        unsafe { g.list.ResourceBarrier(&[barrier(&t.res, UI_TARGET_STATE, D3D12_RESOURCE_STATE_RENDER_TARGET)]) };
        g.submit_wait()?;
        let _ = g.drain_all();
        s.record(&t.res, DXGI_FORMAT_R8G8B8A8_UNORM, &pts).map_err(|x| format!("P2.5 record: {x}"))?;
        value += 1;
        splice_submit(g, s.list(), s.fence(), value, None)?;
        mw2er_render::wait_fence(s.fence(), value).map_err(|x| x.to_string())?;
        let bad = g.drain_errors();
        g.begin(1)?;
        // SAFETY: as above.
        unsafe { g.list.ResourceBarrier(&[barrier(&t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]) };
        g.submit_wait()?;
        let _ = g.drain_all();
        rep.add(!bad.is_empty(), "P2.5", format!("negative control (GBV): the sample of a layer in RENDER_TARGET reports {} error(s){}", bad.len(), bad.first().map(|m| format!(" first: {}", m.chars().take(160).collect::<String>())).unwrap_or_default()));
    }
    Ok(())
}

fn completed(ep: &EarlyPass) -> u64 {
    ep.completed()
}

// ---- C4.1 PASS I41 ------------------------------------------------------------------------------------------------------

/// The constant "scene" under the layer (ER's composite input) and list A's "UI" colour + coverage.
const SCENE41: [f32; 3] = [0.2, 0.4, 0.6];
const UI41: [f32; 4] = [0.1, 0.8, 0.3, 0.6];

/// List A's "UI" pipeline: shaders/solid.hlsl with the layer's "over" blend in the convention.
struct UiQuad {
    rs: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

fn ui_quad(g: &Gpu, conv: c4::LayerAlpha) -> Result<UiQuad, String> {
    let rs = mw2er_render::create_solid_root_signature(&g.dev).map_err(|x| x.to_string())?;
    let pso = mw2er_render::create_solid_pso(&g.dev, &rs, DXGI_FORMAT_R8G8B8A8_UNORM, mw2er_render::layer_blend(EarlyMode::of(conv), true)).map_err(|x| x.to_string())?;
    Ok(UiQuad { rs, pso })
}

/// List A of C4.1 (ER's UI frame on its layer): `0xC0 -> RT`, the "UI" quad inside `boxr` (or a full clear to the
/// layer's own clear value), `RT -> 0xC0`. Recorded into the harness list, closed. Draws only: no clear-value warning.
fn record_er_ui41(g: &Gpu, t: &ErTex, q: &UiQuad, boxr: RECT, clear: Option<[f32; 4]>) -> Result<(), String> {
    g.begin(1)?;
    let consts = UI41.map(f32::to_bits);
    // SAFETY: recording into the harness's open list; the texture and the pipeline live until the wait.
    unsafe {
        g.list.ResourceBarrier(&[barrier(&t.res, UI_TARGET_STATE, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        match clear {
            Some(c) => g.list.ClearRenderTargetView(t.rtv, &c, None),
            None => {
                g.list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: W1 as f32, Height: H1 as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
                g.list.RSSetScissorRects(&[boxr]);
                g.list.OMSetRenderTargets(1, Some(&t.rtv), false, None);
                g.list.SetGraphicsRootSignature(&q.rs);
                g.list.SetPipelineState(&q.pso);
                g.list.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                g.list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const _, 0);
                g.list.DrawInstanced(3, 1, 0, 0);
            }
        }
        g.list.ResourceBarrier(&[barrier(&t.res, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]);
        e(g.list.Close(), "Close(A41)")
    }
}

/// What our early list of one I41 frame draws.
enum Draw41<'a> {
    Gun(&'a FrameParams<'a>),
    Canary([f32; 4]),
    /// The persistence check: barriers only (as `viewmodel::record_canary41(q, None)`).
    Check,
}

/// One I41 frame: our list RECORDED ON A SECOND THREAD (`begin_prepared`: nothing is created there), then hudhook's
/// splice ahead of list A (if any), then a wait. Returns the debug-layer messages of the record + the execution.
#[allow(clippy::too_many_arguments)]
fn frame41(g: &mut Gpu, r: &Renderer, ep: &mut EarlyPass, t: &ErTex, mode: EarlyMode, slot: usize, value: u64, draw: Draw41<'_>, a: Option<(&UiQuad, RECT, Option<[f32; 4]>)>) -> Result<Vec<String>, String> {
    let mut msgs = g.drain_all();
    let et = EarlyTarget { resource: &t.res, format: t.fmt, width: W1, height: H1, mode };
    /// The renderer handed to the recording thread while this one only waits for it (the DLL does the same through
    /// its `RENDERER` mutex: one thread at a time).
    struct Lent<'a>(&'a Renderer);
    // SAFETY: the spawning thread blocks in `join` while the recording thread uses the renderer: never two at a time.
    unsafe impl Sync for Lent<'_> {}
    impl Lent<'_> {
        fn get(&self) -> &Renderer {
            self.0
        }
    }
    let lent = Lent(r);
    let rec = std::thread::scope(|s| {
        s.spawn(|| -> Result<(), String> {
            let r = lent.get();
            ep.begin_prepared(slot, value, &et).map_err(|x| format!("begin_prepared: {x}"))?;
            match draw {
                Draw41::Gun(fp) => r.record_gun_early(ep, slot, &et, 0, 0, fp).map_err(|x| format!("record_gun_early: {x}"))?,
                Draw41::Canary(c) => ep.record_canary(slot, &et, c4::CANARY_XY, c).map_err(|x| format!("record_canary: {x}"))?,
                Draw41::Check => {}
            }
            ep.finish(slot, &et).map_err(|x| format!("finish: {x}"))
        })
        .join()
        .map_err(|_| "the recording thread panicked".to_string())
    })?;
    rec?;
    let er = match a {
        Some((q, b, c)) => {
            record_er_ui41(g, t, q, b, c)?;
            Some(g.list.clone())
        }
        None => None,
    };
    splice_submit(g, &ep_list(ep, slot)?, &ep_fence(ep)?, value, er.as_ref())?;
    g.fence_val += 1;
    // SAFETY: signalling the harness fence on its queue after both calls.
    e(unsafe { g.queue.Signal(&g.fence, g.fence_val) }, "Signal(harness)")?;
    g.wait(g.fence_val)?;
    msgs.extend(g.drain_all());
    Ok(msgs)
}

/// UiEnd's readback: the DLL's LayerSampler on the layer in 0xC0 (after list A), executed with its own Signal.
fn readback41(g: &mut Gpu, s: &mut mw2er_render::LayerSampler, t: &ErTex, canary: bool, value: u64) -> Result<([[u8; 4]; 16], Vec<String>), String> {
    let mut msgs = g.drain_all();
    s.record(&t.res, t.fmt, &c4::readback_points41(W1, H1, canary)).map_err(|x| format!("sampler record: {x}"))?;
    splice_submit(g, s.list(), s.fence(), value, None)?;
    mw2er_render::wait_fence(s.fence(), value).map_err(|x| x.to_string())?;
    msgs.extend(g.drain_all());
    Ok((s.read().map(c4::unpack_px), msgs))
}

fn lpx(px: &[u8], i: usize) -> [f32; 4] {
    [0, 1, 2, 3].map(|k| px[i * 4 + k] as f32 / 255.0)
}

fn run_i41(g: &mut Gpu, r: &Renderer, sc: &mut Scene, out: &Path, rep: &mut Report) -> Result<(), String> {
    let fmt = DXGI_FORMAT_R8G8B8A8_UNORM;
    let (pal, vp, view, _, _) = sc.pose(&TEST_POSES[0], W1, H1);
    let fp = FrameParams { palette: &pal, view_proj: vp, view, light: LightParams::default(), out: OutputMode::Srgb, paper_white_nits: 200.0, debug: DebugMode::None, variant: Variant::default(), timing: false };
    let mut sampler = mw2er_render::LayerSampler::new(&g.dev).map_err(|x| format!("LayerSampler::new: {x}"))?;
    let mut sv = 0u64;
    for (case, conv) in [("I41.1", c4::LayerAlpha::Transmit), ("I41.2", c4::LayerAlpha::Premul)] {
        let mode = EarlyMode::of(conv);
        let empty = c4::empty_px41(conv);
        let q = ui_quad(g, conv)?;
        let mut ep = r.create_early(fmt, mode, W1, H1).map_err(|x| format!("{case} create_early: {x}"))?;
        let mut value = completed(&ep);
        // frame 0: the gun alone finds its box; frame 1: list A's "UI" quad over the gun's left half
        let t0 = er_texture(g, fmt, 6, empty)?;
        value += 1;
        let mut msgs = frame41(g, r, &mut ep, &t0, mode, 0, value, Draw41::Gun(&fp), None)?;
        let px0 = read_back(g, &t0)?;
        let _ = g.drain_all();
        let empty_u8 = to_u8(empty);
        let gun_px = |i: usize| px0[i * 4..i * 4 + 4] != empty_u8;
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..H1 {
            for x in 0..W1 {
                if gun_px((y * W1 + x) as usize) {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        if x0 == u32::MAX {
            rep.add(false, case, format!("{}: no gun pixels drawn by the early list", conv.label()));
            continue;
        }
        let boxr = RECT { left: x0 as i32, top: y0 as i32, right: ((x0 + x1) / 2) as i32, bottom: (y1 + 1) as i32 };
        let t1 = er_texture(g, fmt, 7, empty)?;
        let _ = g.drain_all();
        value += 1;
        msgs.extend(frame41(g, r, &mut ep, &t1, mode, 1, value, Draw41::Gun(&fp), Some((&q, boxr, None)))?);
        let px1 = read_back(g, &t1)?;
        let _ = g.drain_all();
        // the composite of both frames over the scene
        let n = (W1 * H1) as usize;
        let (mut empty_ok, mut empty_n, mut opaque_ok, mut gun_n, mut ui_ok, mut ui_n, mut ui_gun, mut outside_same) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut img = vec![0u8; n * 4];
        for i in 0..n {
            let (x, y) = ((i as u32) % W1, (i as u32) / W1);
            let c0 = c4::composite41(conv, SCENE41, lpx(&px0, i));
            let c1 = c4::composite41(conv, SCENE41, lpx(&px1, i));
            for k in 0..3 {
                img[i * 4 + k] = (c1[k].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            img[i * 4 + 3] = 255;
            let inside = (x as i32) >= boxr.left && (x as i32) < boxr.right && (y as i32) >= boxr.top && (y as i32) < boxr.bottom;
            if inside {
                // UI over (gun over scene): ui*b + c0*(1-b), within 8-bit rounding
                ui_n += 1;
                let want = [0, 1, 2].map(|k| UI41[k] * UI41[3] + c0[k] * (1.0 - UI41[3]));
                ui_ok += (0..3).all(|k| (c1[k] - want[k]).abs() <= 2.5 / 255.0) as usize;
                ui_gun += gun_px(i) as usize;
            } else {
                outside_same += (px1[i * 4..i * 4 + 4] == px0[i * 4..i * 4 + 4]) as usize;
                if gun_px(i) {
                    // the gun's opaque pixels: alpha 0 (transmit) / 255 (premul): shown exactly as their colour
                    gun_n += 1;
                    let a = px0[i * 4 + 3];
                    opaque_ok += (a == empty_u8[3] ^ 255 && (0..3).all(|k| (c0[k] - px0[i * 4 + k] as f32 / 255.0).abs() < 1e-6)) as usize;
                } else {
                    empty_n += 1;
                    empty_ok += (0..3).all(|k| (c0[k] - SCENE41[k]).abs() <= 0.5 / 255.0 + 1e-6) as usize;
                }
            }
        }
        let _ = write_png(&out.join(format!("c4_{}.png", case.replace('.', "_"))), &img, W1, H1);
        let outside = n - ui_n;
        let ok = msgs.is_empty()
            && gun_n > 0
            && opaque_ok * 100 >= gun_n * 99
            && empty_n > 0
            && empty_ok == empty_n
            && ui_gun > 0
            && ui_ok * 1000 >= ui_n * 995
            && outside_same == outside
            && completed(&ep) >= value;
        rep.add(ok, case, format!(
            "{} layer (empty {:?}): recorded on a second thread; gun box ({x0},{y0})-({x1},{y1}); composite over the scene: empty {empty_ok}/{empty_n} show the scene, opaque gun {opaque_ok}/{gun_n} (alpha {}), UI over the gun inside A's box {ui_ok}/{ui_n} px (gun px under it {ui_gun}), outside the box unchanged {outside_same}/{outside}; fence {}>={value}; debug-layer messages (any severity) {}{}",
            conv.label(),
            empty_u8,
            empty_u8[3] ^ 255,
            completed(&ep),
            msgs.len(),
            msgs.first().map(|m| format!(" first: {m}")).unwrap_or_default()
        ));
        // I41.5 (per convention): the re-check reads the layer after the frame (gun + UI): its own convention
        sv += 1;
        let (px, m) = readback41(g, &mut sampler, &t1, false, sv)?;
        let vote = c4::recheck_vote41(&px);
        let empty_pts = px.iter().filter(|p| c4::empty_alpha(**p).is_some()).count();
        rep.add(vote == Some(conv) && m.is_empty(), &format!("I41.5{}", if conv == c4::LayerAlpha::Transmit { "t" } else { "p" }), format!(
            "{} layer after the frame: re-check vote {} ({empty_pts}/16 empty texels); debug-layer messages {}",
            conv.label(),
            c4::alpha_label(vote),
            m.len()
        ));
        // I41.3 / I41.4 (per convention): the canary, read back after list A
        let tc = er_texture(g, fmt, 8, empty)?;
        let _ = g.drain_all();
        value += 1;
        let elsewhere = RECT { left: 400, top: 400, right: 600, bottom: 500 };
        let mut cm = frame41(g, r, &mut ep, &tc, mode, 2, value, Draw41::Canary(c4::canary_color41(conv)), Some((&q, elsewhere, None)))?;
        sv += 1;
        let (px, m) = readback41(g, &mut sampler, &tc, true, sv)?;
        cm.extend(m);
        let read = c4::canary_read41(px[0], conv);
        rep.add(
            read == c4::CanaryRead::Ok && c4::canary_next41(read, 1) == Some(true) && cm.is_empty(),
            &format!("I41.3{}", if conv == c4::LayerAlpha::Transmit { "t" } else { "p" }),
            format!("{} canary at (2,2) ahead of list A, read back after it: {}; debug-layer messages {}{}", conv.label(), c4::fmt_canary41(read, px[0]), cm.len(), cm.first().map(|m| format!(" first: {m}")).unwrap_or_default()),
        );
        // I41.6 (per convention): the persistence check on the same layer one UI frame later (our list draws nothing)
        value += 1;
        let mut pm = frame41(g, r, &mut ep, &tc, mode, 1, value, Draw41::Check, Some((&q, elsewhere, None)))?;
        sv += 1;
        let (px, m) = readback41(g, &mut sampler, &tc, true, sv)?;
        pm.extend(m);
        let kept = c4::canary_gone41(px[0], conv);
        let kept_px = px[0];
        value += 1;
        pm.extend(frame41(g, r, &mut ep, &tc, mode, 2, value, Draw41::Check, Some((&q, elsewhere, Some(empty))))?);
        sv += 1;
        let (px, m) = readback41(g, &mut sampler, &tc, true, sv)?;
        pm.extend(m);
        let gone = c4::canary_gone41(px[0], conv);
        rep.add(
            kept == c4::CanaryGone::Persisted && c4::gone_next41(kept, 1) == Some(false) && gone == c4::CanaryGone::Gone && c4::gone_next41(gone, 1) == Some(true) && pm.is_empty(),
            &format!("I41.6{}", if conv == c4::LayerAlpha::Transmit { "t" } else { "p" }),
            format!(
                "{} persistence check (our list draws nothing): list A without a clear -> {} (trip `cleared`); list A clearing at its frame start -> {} (the gun may start); debug-layer messages {}{}",
                conv.label(),
                c4::fmt_canary_gone41(kept, kept_px),
                c4::fmt_canary_gone41(gone, px[0]),
                pm.len(),
                pm.first().map(|m| format!(" first: {m}")).unwrap_or_default()
            ),
        );
        let tk = er_texture(g, fmt, 9, empty)?;
        let _ = g.drain_all();
        value += 1;
        let mut km = frame41(g, r, &mut ep, &tk, mode, 0, value, Draw41::Canary(c4::canary_color41(conv)), Some((&q, elsewhere, Some(empty))))?;
        sv += 1;
        let (px, m) = readback41(g, &mut sampler, &tk, true, sv)?;
        km.extend(m);
        let read = c4::canary_read41(px[0], conv);
        rep.add(
            read == c4::CanaryRead::Cleared && c4::canary_next41(read, 1) == Some(false) && km.is_empty(),
            &format!("I41.4{}", if conv == c4::LayerAlpha::Transmit { "t" } else { "p" }),
            format!("{} list A clears the layer after the canary: {} (trip `cleared`); debug-layer messages {}", conv.label(), c4::fmt_canary41(read, px[0]), km.len()),
        );
    }
    Ok(())
}
