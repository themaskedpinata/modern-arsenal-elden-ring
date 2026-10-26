//! vmtest: offscreen oracle of the M3 viewmodel renderer (docs/M3_DESIGN.md sections 12.2 and 12.3).
//!
//! Uses exactly the DLL's `mw2er_render::Renderer`, shaders and `mw2er_core::vm_pose` / `vm_proj`. The hudhook
//! callback is simulated: a fresh command list, a target in RENDER_TARGET state cleared to the key colour
//! (255,0,255), an RTV in a separate heap, `record(list, target, slot, params)`, execute, fence wait, copy to a
//! readback buffer, `work\m3\<case>.png` + `<case>.mask` (`VMMK`, u32 w, u32 h, u8 per pixel, 1 = not key).
//!
//! Usage:
//!   vmtest --version                       shader hashes
//!   vmtest --write-poses <dir>             poses.json for blender_ref.py
//!   vmtest --all --out <dir> [--gbv]       poses + cases D1-D5, W, O, F with the debug layer; exit 0 = PASS
//!   vmtest --compare <dir>                 IoU vs Blender masks (ref_<pose>.mask), diff PNGs; exit 0 = PASS
//!   vmtest --perf [--out <dir>]            GPU time at 3840x2160 (no debug layer, hardware only)
//!   vmtest --assets <dir> --load-only      load + validate only (exit 3 + "VM FAIL reason=assets: ..." on error)
//!   vmtest --thermal [--gbv] [--out <dir>] M4 thermal cases T1-T10 (debug layer; + GPU-based validation with --gbv);
//!                                          T10 = a real ER frame (<root>\work\m4\er_frame_ref.png): clipped-white limit
//!                                          at the configured [ac130] exposure (or --exposure x)
//!   vmtest --thermal-perf [--out <dir>]    M4 thermal GPU time at 3840x2160 R10G10B10A2 (hardware only)
//!   vmtest --thermal-image <png>           M4: the thermal pass on a user screenshot -> <out>\thermal_user.png, plus
//!                                          the clipped black/white fractions (exposure sweep + the configured value)
//!   vmtest --tracers [--gbv] [--out <dir>] run 8: AC-130 round tracer cases TR1-TR4 (debug layer; + GBV): a tracer
//!                                          at a known world position lands at the expected pixel, GPU == CPU, the
//!                                          order with the thermal film, the instance ring, the failure path
//!   vmtest --tracers-perf [--out <dir>]    run 8: tracer GPU time at 3840x2160 R10G10B10A2, worst case (< 0.2 ms)
//!   vmtest --nuke [--gbv] [--out <dir>]    M7 nuke post pass cases N1-N6 (debug layer; + GBV) vs nuke_fx (CPU)
//!   vmtest --nuke-perf [--out <dir>]       M7 nuke pass GPU time at 3840x2160 R10G10B10A2 (p95 < 1 ms)
//!   vmtest --arsenal [--gbv] [--out <dir>] [--guns <dir>]
//!                                          M6: every arsenal gun (idle / ADS / reload mid) through the multi-gun
//!                                          renderer path, cases AR1-AR6 (see arsenal_cases.rs); default out
//!                                          <root>\work\m6\vm, guns <root>\converted\guns
//!   vmtest --compare-arsenal <dir>         M6: IoU vs blender_ref.py --gun masks in <dir>\<gun>
//!   vmtest --c4 [--gbv] [--out <dir>]      C4: the early pass spliced ahead of an ER-like UI list, cases C4.1-C4.7 (see
//!                                          c4_cases.rs; debug layer); default out <root>\work\c4\vm
//!   vmtest --cutout [--gbv] [--out <dir>]  C5: `[ui_yield] mode = "cutout"`, cases CU0-CU5 (see cutout_cases.rs;
//!                                          debug layer); default out <root>\work\c5\vm
//!   vmtest --occluders [--gbv] [--out <dir>] C6: depth-only body capsules before the world guns, cases OC0-OC4 (see
//!                                          occl_cases.rs; debug layer); default out <root>\work\c6\vm
//!   vmtest --erdepth [--gbv] [--out <dir>]   C6 depth 5: ER's scene depth copied / sampled through our SRV and primed
//!                                          into our world-gun depth, the real barrier detour, cases ED0-ED6 (see
//!                                          erdepth_cases.rs; debug layer, + GBV); default out <root>\work\c6\vm
//!   --exposure x / --gamma x               override [ac130] exposure / gamma (thermal cases, --thermal-image, TR2)
//! Common: --assets <dir> (default <root>\converted\viewmodel), --ac130 <dir> (default <root>\converted\ac130),
//! --warp (debugging only; perf refuses it). Thermal outputs default to <root>\work\m4.
//! Exit codes: 0 PASS, 1 FAIL, 2 usage, 3 assets failure.

#![deny(unsafe_op_in_unsafe_fn)]

mod arsenal_cases;
mod c4_cases;
mod cutout_cases;
mod erdepth_cases;
mod nuke_cases;
mod occl_cases;
mod thermal_cases;
mod tracer_cases;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mw2er_core::vm_assets::VmAssets;
use mw2er_core::vm_pose::{PoseScratch, TEST_POSES, TestPose, evaluate, static_state};
use mw2er_core::vm_proj::{ProjParams, build, ndc_to_pixel, project};
use mw2er_core::xform::{Mat4, m_translation};
use mw2er_render::{
    CullMode, DebugMode, FrameParams, LightParams, OutputMode, Renderer, Settled, Target, Variant, wait_fence, wait_submitted,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

const KEY: [u8; 4] = [255, 0, 255, 255];
const KEY_F: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
const SCENE_QUAD_F: [f32; 4] = [0.0, 1.0, 1.0, 1.0];
const W1: u32 = 1920;
const H1: u32 = 1080;
const W4: u32 = 3840;
const H4: u32 = 2160;
const NUM_FRAMES: usize = 3;

// ------------------------------------------------------------------------------------------------ args
struct Args {
    cmd: String,
    out: PathBuf,
    out_set: bool,
    assets: PathBuf,
    ac130: PathBuf,
    image: PathBuf,
    /// --exposure (M4 thermal pre-exposure); None = [ac130] exposure from mw2er.toml (the DLL's value).
    exposure: Option<f32>,
    /// --gamma (run 8 mid-tone lift); None = [ac130] gamma from mw2er.toml.
    gamma: Option<f32>,
    gbv: bool,
    warp: bool,
    /// M6: converted\guns.
    guns: PathBuf,
}

fn project_root() -> PathBuf {
    // <root>\mod\target\release\vmtest.exe -> <root>
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors().nth(4).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn parse_args() -> Result<Args, String> {
    let root = project_root();
    let mut a = Args {
        cmd: String::new(),
        out: root.join("work").join("m3"),
        out_set: false,
        assets: root.join("converted").join("viewmodel"),
        ac130: root.join("converted").join("ac130"),
        image: PathBuf::new(),
        exposure: None,
        gamma: None,
        gbv: false,
        warp: false,
        guns: root.join("converted").join("guns"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--version" | "--all" | "--perf" | "--load-only" | "--thermal" | "--thermal-perf" | "--tracers" | "--tracers-perf" | "--arsenal" | "--nuke" | "--nuke-perf" | "--c4" | "--cutout" | "--occluders" | "--erdepth" => {
                if a.cmd.is_empty() || x == "--load-only" {
                    a.cmd = x
                }
            }
            "--thermal-image" => {
                a.cmd = x;
                a.image = PathBuf::from(it.next().ok_or("missing png")?);
            }
            "--compare" | "--write-poses" | "--compare-arsenal" => {
                a.cmd = x;
                a.out = PathBuf::from(it.next().ok_or("missing dir")?);
                a.out_set = true;
            }
            "--out" => {
                a.out = PathBuf::from(it.next().ok_or("missing --out dir")?);
                a.out_set = true;
            }
            "--assets" => a.assets = PathBuf::from(it.next().ok_or("missing --assets dir")?),
            "--guns" => a.guns = PathBuf::from(it.next().ok_or("missing --guns dir")?),
            "--ac130" => a.ac130 = PathBuf::from(it.next().ok_or("missing --ac130 dir")?),
            "--exposure" => {
                let v = it.next().ok_or("missing --exposure value")?;
                a.exposure = Some(v.parse::<f32>().map_err(|x| format!("--exposure {v}: {x}"))?);
            }
            "--gamma" => {
                let v = it.next().ok_or("missing --gamma value")?;
                a.gamma = Some(v.parse::<f32>().map_err(|x| format!("--gamma {v}: {x}"))?);
            }
            "--gbv" => a.gbv = true,
            "--warp" => a.warp = true,
            _ => return Err(format!("unknown argument {x}")),
        }
    }
    if a.cmd.is_empty() {
        return Err("no command".into());
    }
    if (a.cmd.starts_with("--thermal") || a.cmd.starts_with("--tracers")) && !a.out_set {
        a.out = root.join("work").join("m4");
    }
    if a.cmd.starts_with("--nuke") && !a.out_set {
        a.out = root.join("work").join("m7").join("vm");
    }
    if a.cmd == "--arsenal" && !a.out_set {
        a.out = root.join("work").join("m6").join("vm");
    }
    if a.cmd == "--c4" && !a.out_set {
        a.out = root.join("work").join("c4").join("vm");
    }
    if (a.cmd == "--occluders" || a.cmd == "--erdepth") && !a.out_set {
        a.out = root.join("work").join("c6").join("vm");
    }
    if a.cmd == "--cutout" && !a.out_set {
        a.out = root.join("work").join("c5").join("vm");
    }
    Ok(a)
}

// ------------------------------------------------------------------------------------------------ device
struct Gpu {
    dev: ID3D12Device,
    queue: ID3D12CommandQueue,
    allocs: Vec<ID3D12CommandAllocator>,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    fence_val: u64,
    info: Option<ID3D12InfoQueue>,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_inc: usize,
    dsv_heap: ID3D12DescriptorHeap,
    ts_freq: u64,
    adapter: String,
    software: bool,
}

fn e<T>(r: windows::core::Result<T>, call: &str) -> Result<T, String> {
    r.map_err(|x| format!("{call}: {x}"))
}

impl Gpu {
    fn new(debug: bool, gbv: bool, warp: bool) -> Result<Gpu, String> {
        if debug {
            let mut d: Option<ID3D12Debug> = None;
            // SAFETY: out pointer to a local Option.
            e(unsafe { D3D12GetDebugInterface(&mut d) }, "D3D12GetDebugInterface")?;
            let d = d.ok_or("no debug interface (Graphics Tools not installed?)")?;
            // SAFETY: enabling the layer before the device is created.
            unsafe { d.EnableDebugLayer() };
            if gbv {
                let d1: ID3D12Debug1 = e(d.cast(), "ID3D12Debug1")?;
                // SAFETY: as above.
                unsafe { d1.SetEnableGPUBasedValidation(true) };
            }
        }
        // SAFETY: plain factory creation.
        let factory: IDXGIFactory6 = e(unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }, "CreateDXGIFactory2")?;
        let adapter: IDXGIAdapter1 = if warp {
            // SAFETY: plain call.
            e(unsafe { factory.EnumWarpAdapter() }, "EnumWarpAdapter")?
        } else {
            // SAFETY: plain call.
            e(unsafe { factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }, "EnumAdapterByGpuPreference")?
        };
        // SAFETY: plain call.
        let desc = e(unsafe { adapter.GetDesc1() }, "GetDesc1")?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
        let software = (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0;
        let mut dev: Option<ID3D12Device> = None;
        // SAFETY: out pointer to a local Option.
        e(unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut dev) }, "D3D12CreateDevice")?;
        let dev = dev.ok_or("no device")?;
        let info: Option<ID3D12InfoQueue> = if debug { dev.cast().ok() } else { None };
        // SAFETY: plain COM object creation below.
        let queue: ID3D12CommandQueue = e(
            unsafe {
                dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    Priority: 0,
                    Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                    NodeMask: 0,
                })
            },
            "CreateCommandQueue",
        )?;
        let mut allocs = Vec::new();
        for _ in 0..8 {
            // SAFETY: as above.
            allocs.push(e(unsafe { dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }, "CreateCommandAllocator")?);
        }
        // SAFETY: as above.
        let list: ID3D12GraphicsCommandList = e(
            unsafe { dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocs[0], None::<&ID3D12PipelineState>) },
            "CreateCommandList",
        )?;
        // SAFETY: a fresh list is open; close it so begin() can Reset it.
        e(unsafe { list.Close() }, "Close")?;
        // SAFETY: as above.
        let fence: ID3D12Fence = e(unsafe { dev.CreateFence(0, D3D12_FENCE_FLAG_NONE) }, "CreateFence")?;
        // SAFETY: as above.
        let rtv_heap: ID3D12DescriptorHeap = e(
            unsafe {
                dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: 16,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                })
            },
            "CreateDescriptorHeap(RTV)",
        )?;
        // SAFETY: as above.
        let dsv_heap: ID3D12DescriptorHeap = e(
            unsafe {
                dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                })
            },
            "CreateDescriptorHeap(DSV)",
        )?;
        // SAFETY: plain queries.
        let rtv_inc = unsafe { dev.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) } as usize;
        // SAFETY: plain query.
        let ts_freq = e(unsafe { queue.GetTimestampFrequency() }, "GetTimestampFrequency")?;
        Ok(Gpu { dev, queue, allocs, list, fence, fence_val: 0, info, rtv_heap, rtv_inc, dsv_heap, ts_freq, adapter: name, software })
    }

    fn begin(&self, alloc: usize) -> Result<(), String> {
        // SAFETY: the allocator's previous work was waited on by the caller (submit_wait / per-frame fences).
        unsafe {
            e(self.allocs[alloc].Reset(), "Allocator.Reset")?;
            e(self.list.Reset(&self.allocs[alloc], None::<&ID3D12PipelineState>), "List.Reset")
        }
    }

    /// Closes + executes the list and signals; returns the fence value (no wait).
    fn submit(&mut self) -> Result<u64, String> {
        // SAFETY: the list was recorded by us; resources it references live in the caller until the wait.
        unsafe {
            e(self.list.Close(), "List.Close")?;
            self.queue.ExecuteCommandLists(&[Some(e(self.list.cast(), "cast")?)]);
            self.fence_val += 1;
            e(self.queue.Signal(&self.fence, self.fence_val), "Signal")?;
        }
        Ok(self.fence_val)
    }

    fn wait(&self, v: u64) -> Result<(), String> {
        wait_fence(&self.fence, v).map_err(|x| x.to_string())
    }

    fn submit_wait(&mut self) -> Result<(), String> {
        let v = self.submit()?;
        self.wait(v)
    }

    /// Drains the InfoQueue; returns the ERROR/CORRUPTION messages.
    fn drain_errors(&self) -> Vec<String> {
        self.drain(false)
    }

    /// C4.1 P2: drains the InfoQueue; returns EVERY message (any severity, prefixed with it).
    fn drain_all(&self) -> Vec<String> {
        self.drain(true)
    }

    fn drain(&self, all: bool) -> Vec<String> {
        let Some(q) = &self.info else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: two-call GetMessage pattern with a buffer of the reported size, 8-byte aligned.
        unsafe {
            let n = q.GetNumStoredMessages();
            for i in 0..n {
                let mut len = 0usize;
                if q.GetMessage(i, None, &mut len).is_err() || len == 0 {
                    continue;
                }
                let mut buf = vec![0u64; len.div_ceil(8)];
                let m = buf.as_mut_ptr() as *mut D3D12_MESSAGE;
                if q.GetMessage(i, Some(m), &mut len).is_err() {
                    continue;
                }
                let msg = &*m;
                if all {
                    let d = std::slice::from_raw_parts(msg.pDescription, msg.DescriptionByteLength.saturating_sub(1));
                    out.push(format!("[severity {}] {}", msg.Severity.0, String::from_utf8_lossy(d)));
                } else if msg.Severity == D3D12_MESSAGE_SEVERITY_ERROR || msg.Severity == D3D12_MESSAGE_SEVERITY_CORRUPTION {
                    let d = std::slice::from_raw_parts(msg.pDescription, msg.DescriptionByteLength.saturating_sub(1));
                    out.push(String::from_utf8_lossy(d).into_owned());
                }
            }
            q.ClearStoredMessages();
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ targets
fn bpp(fmt: DXGI_FORMAT) -> usize {
    if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT { 8 } else { 4 }
}

struct Tgt {
    res: ID3D12Resource,
    rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    fmt: DXGI_FORMAT,
    w: u32,
    h: u32,
    rb: ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

fn heap(t: D3D12_HEAP_TYPE) -> D3D12_HEAP_PROPERTIES {
    D3D12_HEAP_PROPERTIES { Type: t, ..Default::default() }
}

fn readback_for(g: &Gpu, desc: &D3D12_RESOURCE_DESC) -> Result<(ID3D12Resource, D3D12_PLACED_SUBRESOURCE_FOOTPRINT), String> {
    let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0u64;
    // SAFETY: one subresource, outputs point at locals.
    unsafe { g.dev.GetCopyableFootprints(desc, 0, 1, 0, Some(&mut layout), None, None, Some(&mut total)) };
    let bd = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut rb: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(
        unsafe { g.dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_READBACK), D3D12_HEAP_FLAG_NONE, &bd, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb) },
        "CreateCommittedResource(readback)",
    )?;
    Ok((rb.ok_or("readback null")?, layout))
}

fn new_target(g: &Gpu, fmt: DXGI_FORMAT, w: u32, h: u32, rtv_index: usize) -> Result<Tgt, String> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: w as u64,
        Height: h,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE { Format: fmt, Anonymous: D3D12_CLEAR_VALUE_0 { Color: KEY_F } };
    let mut res: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(
        unsafe { g.dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET, Some(&clear), &mut res) },
        "CreateCommittedResource(target)",
    )?;
    let res = res.ok_or("target null")?;
    // SAFETY: plain query + RTV creation into our own heap slot (rtv_index < 16).
    let rtv = unsafe {
        let h0 = g.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + rtv_index * g.rtv_inc };
        g.dev.CreateRenderTargetView(&res, None, rtv);
        rtv
    };
    let (rb, layout) = readback_for(g, &desc)?;
    Ok(Tgt { res, rtv, fmt, w, h, rb, layout })
}

fn weak<T: Interface + Clone>(r: &T) -> std::mem::ManuallyDrop<Option<T>> {
    // SAFETY: non-owning copy of a COM pointer that is never released; `r` outlives every use.
    unsafe { std::mem::transmute_copy(r) }
}

fn barrier(r: &ID3D12Resource, a: D3D12_RESOURCE_STATES, b: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: weak(r),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: a,
                StateAfter: b,
            }),
        },
    }
}

/// Records texture -> readback copy (state `state` before and after).
fn record_copy(g: &Gpu, res: &ID3D12Resource, rb: &ID3D12Resource, layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT, state: D3D12_RESOURCE_STATES) {
    // SAFETY: recording into our open list; the resources outlive the submission.
    unsafe {
        g.list.ResourceBarrier(&[barrier(res, state, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: weak(rb),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: layout },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: weak(res),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        g.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        g.list.ResourceBarrier(&[barrier(res, D3D12_RESOURCE_STATE_COPY_SOURCE, state)]);
    }
}

/// Tightly packed pixels of a readback buffer (after the GPU finished).
fn read_rows(rb: &ID3D12Resource, layout: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT, w: u32, h: u32, px: usize) -> Result<Vec<u8>, String> {
    let mut p = std::ptr::null_mut();
    // SAFETY: mapping a READBACK buffer whose copy completed.
    e(unsafe { rb.Map(0, None, Some(&mut p)) }, "Map(readback)")?;
    let mut out = vec![0u8; w as usize * h as usize * px];
    let pitch = layout.Footprint.RowPitch as usize;
    let row = w as usize * px;
    // SAFETY: the buffer holds `h` rows of `pitch` bytes starting at layout.Offset.
    unsafe {
        let base = (p as *const u8).add(layout.Offset as usize);
        for y in 0..h as usize {
            std::ptr::copy_nonoverlapping(base.add(y * pitch), out.as_mut_ptr().add(y * row), row);
        }
        rb.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ frames
struct Scene {
    assets: VmAssets,
    scratch: PoseScratch,
}

impl Scene {
    /// Palette + matrices of a test pose for a w x h target (+ the right and left wrist positions).
    fn pose(&mut self, p: &TestPose, w: u32, h: u32) -> (Vec<[f32; 12]>, Mat4, Mat4, [f32; 3], [f32; 3]) {
        let (st, now) = static_state(p.clip, p.t.max(0.0), p.ads);
        evaluate(&self.assets, &st, now, &mut self.scratch);
        let tv = self.scratch.world[self.assets.skel.tags.view as usize];
        let vp = build(&tv, w, h, p.ads, &ProjParams::default());
        let wr = m_translation(&self.scratch.world[self.assets.skel.tags.wrist_ri as usize]);
        let wl = m_translation(&self.scratch.world[self.assets.skel.tags.wrist_le as usize]);
        (self.scratch.palette.clone(), vp.view_proj, vp.view, wr, wl)
    }
}

struct Opts {
    debug: DebugMode,
    variant: Variant,
    out: OutputMode,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { debug: DebugMode::None, variant: Variant::default(), out: OutputMode::Srgb }
    }
}

/// One simulated hook frame: clear key colour, record, read back. Returns the packed pixels.
fn render_one(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t: &Tgt, pose: &TestPose, o: &Opts, slot: usize) -> Result<Vec<u8>, String> {
    let (pal, vp, view, _, _) = sc.pose(pose, t.w, t.h);
    g.begin(0)?;
    // SAFETY: recording into our open list; the target is in RENDER_TARGET state.
    unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
    let fp = FrameParams {
        palette: &pal,
        view_proj: vp,
        view,
        light: LightParams::default(),
        out: o.out,
        paper_white_nits: 200.0,
        debug: o.debug,
        variant: o.variant,
        timing: false,
    };
    r.record(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, slot, &fp)
        .map_err(|x| format!("record: {x}"))?;
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    read_rows(&t.rb, &t.layout, t.w, t.h, bpp(t.fmt))
}

fn mask_of(px: &[u8], w: u32, h: u32) -> Vec<u8> {
    (0..(w * h) as usize).map(|i| (px[i * 4..i * 4 + 3] != KEY[..3]) as u8).collect()
}

fn write_mask(path: &Path, m: &[u8], w: u32, h: u32) -> Result<(), String> {
    let mut v = b"VMMK".to_vec();
    v.extend_from_slice(&w.to_le_bytes());
    v.extend_from_slice(&h.to_le_bytes());
    v.extend_from_slice(m);
    std::fs::write(path, v).map_err(|x| format!("{}: {x}", path.display()))
}

fn read_mask(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let d = std::fs::read(path).map_err(|x| format!("{}: {x}", path.display()))?;
    if d.len() < 12 || &d[..4] != b"VMMK" {
        return Err(format!("{}: not a VMMK mask", path.display()));
    }
    let w = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
    let h = u32::from_le_bytes([d[8], d[9], d[10], d[11]]);
    if d.len() != 12 + (w * h) as usize {
        return Err(format!("{}: size mismatch", path.display()));
    }
    Ok((w, h, d[12..].to_vec()))
}

fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|x| format!("{}: {x}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|x| x.to_string())?;
    wr.write_image_data(rgba).map_err(|x| x.to_string())
}

// ------------------------------------------------------------------------------------------------ decode
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn half_to_f32(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1F) as i32;
    let m = (h & 0x3FF) as f32;
    s * match e {
        0 => m / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

fn pq_to_nits(e: f32) -> f32 {
    let (m1, m2, c1, c2, c3) = (0.159_301_76f32, 78.843_75f32, 0.835_937_5f32, 18.851_563f32, 18.687_5f32);
    let ep = e.max(0.0).powf(1.0 / m2);
    10000.0 * ((ep - c1).max(0.0) / (c2 - c3 * ep)).powf(1.0 / m1)
}

/// Linear BT.709 value of pixel i of a decoded target.
fn linear_at(px: &[u8], fmt: DXGI_FORMAT, i: usize, paper: f32) -> [f32; 3] {
    match fmt {
        DXGI_FORMAT_R16G16B16A16_FLOAT => {
            let c = |k: usize| half_to_f32(u16::from_le_bytes([px[i * 8 + k * 2], px[i * 8 + k * 2 + 1]]));
            [c(0) * 80.0 / paper, c(1) * 80.0 / paper, c(2) * 80.0 / paper]
        }
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let v = u32::from_le_bytes([px[i * 4], px[i * 4 + 1], px[i * 4 + 2], px[i * 4 + 3]]);
            let e = |sh: u32| ((v >> sh) & 0x3FF) as f32 / 1023.0;
            let c2020 = [pq_to_nits(e(0)) / paper, pq_to_nits(e(10)) / paper, pq_to_nits(e(20)) / paper];
            // BT.2020 -> BT.709
            let m = [[1.660_491, -0.587_641_1, -0.072_849_9], [-0.124_550_5, 1.132_899_9, -0.008_349_4], [-0.018_150_8, -0.100_578_9, 1.118_729_7]];
            [0, 1, 2].map(|r| m[r][0] * c2020[0] + m[r][1] * c2020[1] + m[r][2] * c2020[2])
        }
        _ => [0, 1, 2].map(|k| srgb_to_linear(px[i * 4 + k] as f32 / 255.0)),
    }
}

// ------------------------------------------------------------------------------------------------ cases
struct Report {
    lines: Vec<String>,
    fails: usize,
}

impl Report {
    fn add(&mut self, ok: bool, case: &str, detail: String) {
        if !ok {
            self.fails += 1;
        }
        let l = format!("{} {case} {detail}", if ok { "PASS" } else { "FAIL" });
        println!("{l}");
        self.lines.push(l);
    }
}

fn load(dir: &Path) -> Result<VmAssets, String> {
    VmAssets::load_dir(dir).map_err(|x| format!("assets: {x}"))
}

fn poses_json(out: &Path) -> Result<(), String> {
    let p = ProjParams::default();
    let poses: Vec<String> = TEST_POSES
        .iter()
        .map(|t| format!("  {{\"name\": \"{}\", \"clip\": \"{}\", \"t\": {}, \"ads\": {}}}", t.name, t.clip.name(), t.t, t.ads))
        .collect();
    let j = format!(
        "{{\n \"width\": {W1}, \"height\": {H1}, \"hip_fov\": {}, \"ads_fov\": {}, \"near_in\": {}, \"far_in\": {},\n \"poses\": [\n{}\n ]\n}}\n",
        p.hip_fov,
        p.ads_fov,
        p.near_in,
        p.far_in,
        poses.join(",\n")
    );
    std::fs::create_dir_all(out).map_err(|x| x.to_string())?;
    std::fs::write(out.join("poses.json"), j).map_err(|x| x.to_string())
}

fn run_all(a: &Args) -> Result<bool, String> {
    std::fs::create_dir_all(&a.out).map_err(|x| x.to_string())?;
    poses_json(&a.out)?;
    let assets = load(&a.assets)?;
    let bones = assets.skel.bones.len();
    let mut sc = Scene { assets, scratch: PoseScratch::new(bones) };
    let mut g = Gpu::new(true, a.gbv, a.warp)?;
    println!("adapter: {} (software={}) debug layer on{}", g.adapter, g.software, if a.gbv { " + GBV" } else { "" });
    let mut rep = Report { lines: Vec::new(), fails: 0 };
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    r.upload(&sc.assets).map_err(|x| format!("upload: {x}"))?;
    // As the DLL's loader thread does: PSOs + depth for the swap chain's format and size before the first frame.
    r.prepare(DXGI_FORMAT_R8G8B8A8_UNORM, Variant::default(), W1, H1).map_err(|x| format!("prepare: {x}"))?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "init", format!("Renderer::new + upload + prepare; InfoQueue errors {} {:?}", errs.len(), errs.first()));
    case_u(&g, &mut rep)?;

    let t1 = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W1, H1, 0)?;
    let t4 = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W4, H4, 1)?;

    // Poses at 1920x1080 (+ idle at 3840x2160): PNG + mask; D3 coverage == silhouette coverage.
    let mut singles = std::collections::BTreeMap::new();
    let mut d3_bad = Vec::new();
    for p in TEST_POSES.iter() {
        let px = render_one(&mut g, &mut r, &mut sc, &t1, p, &Opts::default(), 0)?;
        let m = mask_of(&px, W1, H1);
        write_png(&a.out.join(format!("{}.png", p.name)), &px, W1, H1)?;
        write_mask(&a.out.join(format!("{}.mask", p.name)), &m, W1, H1)?;
        let sil = render_one(&mut g, &mut r, &mut sc, &t1, p, &Opts { debug: DebugMode::Silhouette, ..Opts::default() }, 0)?;
        let ms = mask_of(&sil, W1, H1);
        let diff = m.iter().zip(&ms).filter(|(x, y)| x != y).count();
        let cover = m.iter().filter(|&&v| v == 1).count();
        if diff != 0 || cover == 0 {
            d3_bad.push(format!("{} differs on {diff} px (coverage {cover})", p.name));
        }
        println!("pose {:<18} coverage {:>6} px ({:.2}%)", p.name, cover, 100.0 * cover as f64 / (W1 * H1) as f64);
        singles.insert(p.name, px);
    }
    let a1 = r.take_present_allocs();
    rep.add(a1.is_empty(), "A1", format!("after prepare({W1}x{H1}) the first frames create nothing on the present thread {a1:?}"));
    let px4 = render_one(&mut g, &mut r, &mut sc, &t4, &TEST_POSES[0], &Opts::default(), 0)?;
    let a2 = r.take_present_allocs();
    rep.add(a2 == vec![format!("depth {W4}x{H4}")], "A2", format!("a size change creates exactly one depth buffer, reported for the log {a2:?}"));
    write_png(&a.out.join("idle_4k.png"), &px4, W4, H4)?;
    write_mask(&a.out.join("idle_4k.mask"), &mask_of(&px4, W4, H4), W4, H4)?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "poses", format!("9 poses @1920x1080 + idle @3840x2160 written; InfoQueue errors {} {:?}", errs.len(), errs.first()));
    rep.add(d3_bad.is_empty(), "D3", format!("changed pixels == geometry coverage for every pose (no clear, no blend leakage) {d3_bad:?}"));

    case_d1(&mut g, &mut r, &mut sc, &t1, &singles, &mut rep)?;
    case_d2(&mut g, &mut r, &mut sc, &t1, &mut rep)?;
    case_d4(&mut g, &mut r, &mut sc, &singles, &mut rep)?;
    case_d5(&mut g, &mut r, &mut sc, &t1, &t4, &singles, &px4, &mut rep)?;
    case_w(&mut g, &mut r, &mut sc, &t1, &mut rep)?;
    case_o(&mut g, &mut r, &mut sc, &singles, &mut rep)?;
    case_f(a, &mut rep)?;
    let errs = g.drain_errors();
    rep.add(errs.is_empty(), "infoqueue", format!("{} ERROR/CORRUPTION messages at the end", errs.len()));
    std::fs::write(a.out.join("vmtest_report.txt"), rep.lines.join("\n") + "\n").map_err(|x| x.to_string())?;
    println!("VMTEST {} ({} FAIL)", if rep.fails == 0 { "PASS" } else { "FAIL" }, rep.fails);
    Ok(rep.fails == 0)
}

/// U: the in-flight wait used after an upload is submitted reports a fence that never completes as
/// StillRunning (so `upload` leaks instead of freeing objects the GPU may still use) and a completed one as Done.
fn case_u(g: &Gpu, rep: &mut Report) -> Result<(), String> {
    // SAFETY: plain COM object creation.
    let f: ID3D12Fence = e(unsafe { g.dev.CreateFence(0, D3D12_FENCE_FLAG_NONE) }, "CreateFence(U)")?;
    let t0 = std::time::Instant::now();
    let a = wait_submitted(&g.dev, &f, 1, 300);
    let waited = t0.elapsed().as_millis();
    // SAFETY: CPU-side signal of our own fence.
    e(unsafe { f.Signal(1) }, "Signal(U)")?;
    let b = wait_submitted(&g.dev, &f, 1, 300);
    let ok = a == Settled::StillRunning && (250..2000).contains(&waited) && b == Settled::Done;
    rep.add(ok, "U", format!("unfinished fence -> {a:?} after {waited} ms (max 300), completed -> {b:?}"));
    Ok(())
}

/// D1: an ER-style scene (its own reversed-Z depth cleared to a point 2 in in front of the eye, bound with the
/// target, and a scene quad drawn) must not occlude the viewmodel, and the scene depth must be untouched.
fn case_d1(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t: &Tgt, singles: &std::collections::BTreeMap<&str, Vec<u8>>, rep: &mut Report) -> Result<(), String> {
    let p = ProjParams::default();
    let z = 2.0f32;
    let scene_depth = p.near_in * (p.far_in - z) / ((p.far_in - p.near_in) * z);
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: t.w as u64,
        Height: t.h,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_D32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE {
        Format: DXGI_FORMAT_D32_FLOAT,
        Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: scene_depth, Stencil: 0 } },
    };
    let mut ds: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptions; out pointer to a local Option.
    e(
        unsafe { g.dev.CreateCommittedResource(&heap(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(&clear), &mut ds) },
        "CreateCommittedResource(scene depth)",
    )?;
    let ds = ds.ok_or("scene depth null")?;
    // SAFETY: plain query + DSV creation into the harness's own heap.
    let dsv = unsafe {
        let h = g.dsv_heap.GetCPUDescriptorHandleForHeapStart();
        g.dev.CreateDepthStencilView(&ds, None, h);
        h
    };
    let (rb_before, lay) = readback_for(g, &desc)?;
    let (rb_after, _) = readback_for(g, &desc)?;
    let pose = &TEST_POSES[0];
    let (pal, vp, view, _, _) = sc.pose(pose, t.w, t.h);
    let quad = RECT { left: (t.w / 4) as i32, top: (t.h / 4) as i32, right: t.w as i32, bottom: t.h as i32 };
    g.begin(0)?;
    // SAFETY: recording into our open list; states as created.
    unsafe {
        g.list.ClearRenderTargetView(t.rtv, &KEY_F, None);
        g.list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, scene_depth, 0, None);
        g.list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv));
        g.list.ClearRenderTargetView(t.rtv, &SCENE_QUAD_F, Some(&[quad]));
    }
    record_copy(g, &ds, &rb_before, lay, D3D12_RESOURCE_STATE_DEPTH_WRITE);
    let pre = new_target(g, t.fmt, t.w, t.h, 5)?;
    // keep a copy of the pre-record image for the "outside unchanged" check
    // SAFETY: copying the target into the readback of `pre` (states restored by record_copy).
    record_copy(g, &t.res, &pre.rb, pre.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    // SAFETY: re-bind the scene depth as a host would have it bound before our callback.
    unsafe { g.list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv)) };
    let fp = FrameParams {
        palette: &pal,
        view_proj: vp,
        view,
        light: LightParams::default(),
        out: OutputMode::Srgb,
        paper_white_nits: 200.0,
        debug: DebugMode::None,
        variant: Variant::default(),
        timing: false,
    };
    r.record(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, 1, &fp)
        .map_err(|x| x.to_string())?;
    record_copy(g, &ds, &rb_after, lay, D3D12_RESOURCE_STATE_DEPTH_WRITE);
    record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
    g.submit_wait()?;
    let before_px = read_rows(&pre.rb, &pre.layout, t.w, t.h, 4)?;
    let after_px = read_rows(&t.rb, &t.layout, t.w, t.h, 4)?;
    let d0 = read_rows(&rb_before, &lay, t.w, t.h, 4)?;
    let d1 = read_rows(&rb_after, &lay, t.w, t.h, 4)?;
    let ref_mask = mask_of(&singles["idle"], t.w, t.h);
    let (mut sil, mut covered_by_quad_color, mut outside_changed) = (0usize, 0usize, 0usize);
    for i in 0..(t.w * t.h) as usize {
        let a = &after_px[i * 4..i * 4 + 3];
        if ref_mask[i] == 1 {
            sil += 1;
            if a == [0, 255, 255] || a == [255, 0, 255] {
                covered_by_quad_color += 1;
            }
        } else if a != &before_px[i * 4..i * 4 + 3] {
            outside_changed += 1;
        }
    }
    let shown = 1.0 - covered_by_quad_color as f64 / sil.max(1) as f64;
    let depth_same = d0 == d1;
    let ok = shown >= 0.999 && outside_changed == 0 && depth_same && sil > 0;
    let errs = g.drain_errors();
    rep.add(
        ok && errs.is_empty(),
        "D1",
        format!(
            "scene depth {scene_depth:.5} (2 in) bound + scene quad: {:.4}% of {sil} silhouette px drawn over it; {outside_changed} px outside changed; scene DSV byte-identical={depth_same}; InfoQueue {}",
            shown * 100.0,
            errs.len()
        ),
    );
    Ok(())
}

/// D2: part colours; where the hand is in front of the gun the depth test decides (green with depth, red when
/// depth is off and the gun is drawn last).
fn case_d2(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t: &Tgt, rep: &mut Report) -> Result<(), String> {
    let pose = &TEST_POSES[0];
    let on = render_one(g, r, sc, t, pose, &Opts { debug: DebugMode::PartColor, ..Opts::default() }, 2)?;
    let off_v = Variant { depth: false, ..Variant::default() };
    let off = render_one(g, r, sc, t, pose, &Opts { debug: DebugMode::PartColor, variant: off_v, ..Opts::default() }, 2)?;
    // The design's anchor is j_wrist_ri (hand in front of the grip). In this asset the right wrist projects
    // below the screen at idle, so the first on-screen wrist is used (j_wrist_le: the GPS-watch hand on the
    // foregrip); the anchor used is printed.
    let (_, vp, _, wr, wl) = sc.pose(pose, t.w, t.h);
    let mut anchor = None;
    for (name, p3) in [("j_wrist_ri", wr), ("j_wrist_le", wl)] {
        if let Some(n) = project(&vp, &p3) {
            let px = ndc_to_pixel(n, t.w, t.h);
            println!("D2 {name} projects to ({:.0},{:.0})", px[0], px[1]);
            if anchor.is_none() && px[0] >= 0.0 && px[1] >= 0.0 && px[0] < t.w as f32 && px[1] < t.h as f32 {
                anchor = Some((name, px));
            }
        }
    }
    let (aname, px) = anchor.ok_or("no wrist on screen")?;
    let (x, y) = (px[0].round() as i64, px[1].round() as i64);
    let at = |v: &[u8], x: i64, y: i64| -> [u8; 3] {
        if x < 0 || y < 0 || x >= t.w as i64 || y >= t.h as i64 {
            return [0, 0, 0];
        }
        let i = (y as usize * t.w as usize + x as usize) * 4;
        [v[i], v[i + 1], v[i + 2]]
    };
    let wrist_on = at(&on, x, y);
    let decided = (0..(t.w * t.h) as usize)
        .filter(|&i| on[i * 4..i * 4 + 3] == [0, 255, 0] && off[i * 4..i * 4 + 3] == [255, 0, 0])
        .count();
    // the design's pixel: j_wrist_ri; if it lands where the gun does not overlap the hand, the nearest decided
    // pixel within 40 px is reported instead
    let mut near = None;
    'o: for r_ in 0..=40i64 {
        for dy in -r_..=r_ {
            for dx in -r_..=r_ {
                if dx.abs() != r_ && dy.abs() != r_ {
                    continue;
                }
                if at(&on, x + dx, y + dy) == [0, 255, 0] && at(&off, x + dx, y + dy) == [255, 0, 0] {
                    near = Some((x + dx, y + dy, r_));
                    break 'o;
                }
            }
        }
    }
    write_png(&rep_path(rep, "d2_parts_depth_on.png"), &on, t.w, t.h).ok();
    let ok = wrist_on == [0, 255, 0] && decided >= 500 && near.is_some();
    let errs = g.drain_errors();
    rep.add(
        ok && errs.is_empty(),
        "D2",
        format!(
            "{aname} at px ({x},{y}) is {wrist_on:?} (green = hand) with depth; {decided} px are green with depth and red without (depth decides); nearest such px to the wrist {near:?}; InfoQueue {}",
            errs.len()
        ),
    );
    Ok(())
}

fn rep_path(_rep: &Report, name: &str) -> PathBuf {
    project_root().join("work").join("m3").join(name)
}

/// D4: 6 frames over slots 0,1,2,0,1,2 with different poses and hudhook's waiting rule (wait only for the fence
/// of the frame that last used the slot); every image equals its single-frame render.
fn case_d4(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, singles: &std::collections::BTreeMap<&str, Vec<u8>>, rep: &mut Report) -> Result<(), String> {
    let order = [0usize, 1, 2, 3, 4, 7];
    let mut tg = Vec::new();
    for (k, _) in order.iter().enumerate() {
        tg.push(new_target(g, DXGI_FORMAT_R8G8B8A8_UNORM, W1, H1, 6 + k)?);
    }
    let mut fences = [0u64; 6];
    for (k, &pi) in order.iter().enumerate() {
        let slot = k % NUM_FRAMES;
        if k >= NUM_FRAMES {
            g.wait(fences[k - NUM_FRAMES])?;
        }
        let pose = &TEST_POSES[pi];
        let (pal, vp, view, _, _) = sc.pose(pose, W1, H1);
        g.begin(1 + k)?;
        let t = &tg[k];
        // SAFETY: recording into our open list.
        unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
        let fp = FrameParams {
            palette: &pal,
            view_proj: vp,
            view,
            light: LightParams::default(),
            out: OutputMode::Srgb,
            paper_white_nits: 200.0,
            debug: DebugMode::None,
            variant: Variant::default(),
            timing: true,
        };
        r.record(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, slot, &fp)
            .map_err(|x| x.to_string())?;
        record_copy(g, &t.res, &t.rb, t.layout, D3D12_RESOURCE_STATE_RENDER_TARGET);
        fences[k] = g.submit()?;
    }
    g.wait(fences[5])?;
    let mut bad = Vec::new();
    for (k, &pi) in order.iter().enumerate() {
        let px = read_rows(&tg[k].rb, &tg[k].layout, W1, H1, 4)?;
        if px != singles[TEST_POSES[pi].name] {
            bad.push(format!("frame {k} ({})", TEST_POSES[pi].name));
        }
    }
    let errs = g.drain_errors();
    rep.add(bad.is_empty() && errs.is_empty(), "D4", format!("6 frames over slots 0,1,2,0,1,2 (wait only on the slot's previous fence): mismatches {bad:?}; InfoQueue {}", errs.len()));
    Ok(())
}

/// D5: the same Renderer renders 1920x1080, 3840x2160, 1920x1080 (its depth buffer is recreated each time).
#[allow(clippy::too_many_arguments)]
fn case_d5(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t1: &Tgt, t4: &Tgt, singles: &std::collections::BTreeMap<&str, Vec<u8>>, px4: &[u8], rep: &mut Report) -> Result<(), String> {
    let p = &TEST_POSES[0];
    let a = render_one(g, r, sc, t1, p, &Opts::default(), 1)?;
    let b = render_one(g, r, sc, t4, p, &Opts::default(), 2)?;
    let c = render_one(g, r, sc, t1, p, &Opts::default(), 0)?;
    let ok = a == singles["idle"] && b == px4 && c == singles["idle"];
    let errs = g.drain_errors();
    rep.add(ok && errs.is_empty(), "D5", format!("1080p -> 4K -> 1080p on one Renderer match the single-size renders: {ok}; InfoQueue {}", errs.len()));
    Ok(())
}

/// W: winding. The shipped variant (CULL_BACK + its FrontCCW) must equal CULL_NONE (front faces kept) and the
/// opposite FrontCCW must differ.
fn case_w(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, t: &Tgt, rep: &mut Report) -> Result<(), String> {
    let p = &TEST_POSES[0];
    let back = render_one(g, r, sc, t, p, &Opts::default(), 0)?;
    let none = render_one(g, r, sc, t, p, &Opts { variant: Variant { cull: CullMode::None, ..Variant::default() }, ..Opts::default() }, 0)?;
    let flip = Variant::default().front_ccw;
    let ccw = render_one(g, r, sc, t, p, &Opts { variant: Variant { front_ccw: !flip, ..Variant::default() }, ..Opts::default() }, 0)?;
    let m = mask_of(&none, t.w, t.h);
    let sil: Vec<usize> = (0..m.len()).filter(|&i| m[i] == 1).collect();
    let same = |v: &[u8]| sil.iter().filter(|&&i| v[i * 4..i * 4 + 3] == none[i * 4..i * 4 + 3]).count() as f64 / sil.len().max(1) as f64;
    let (sb, sc_) = (same(&back), same(&ccw));
    let ok = sb >= 0.99 && (1.0 - sc_) >= 0.20;
    let errs = g.drain_errors();
    rep.add(
        ok && errs.is_empty(),
        "W",
        format!(
            "CULL_BACK+FrontCCW={} equals CULL_NONE on {:.2}% of {} silhouette px (need >= 99%); FrontCCW={} differs on {:.2}% (need >= 20%); InfoQueue {}",
            flip,
            sb * 100.0,
            sil.len(),
            !flip,
            (1.0 - sc_) * 100.0,
            errs.len()
        ),
    );
    Ok(())
}

/// O: PQ (R10G10B10A2) and scRGB (R16G16B16A16_FLOAT) renders decode to the same linear values as the sRGB one.
fn case_o(g: &mut Gpu, r: &mut Renderer, sc: &mut Scene, singles: &std::collections::BTreeMap<&str, Vec<u8>>, rep: &mut Report) -> Result<(), String> {
    let p = &TEST_POSES[0];
    let srgb = &singles["idle"];
    let tp = new_target(g, DXGI_FORMAT_R10G10B10A2_UNORM, W1, H1, 12)?;
    let ts = new_target(g, DXGI_FORMAT_R16G16B16A16_FLOAT, W1, H1, 13)?;
    let pq = render_one(g, r, sc, &tp, p, &Opts { out: OutputMode::Pq, ..Opts::default() }, 0)?;
    let scrgb = render_one(g, r, sc, &ts, p, &Opts { out: OutputMode::Scrgb, ..Opts::default() }, 0)?;
    // 16 silhouette pixels bright enough that one 8-bit sRGB step stays under 1.7% of the linear value (the
    // 2% criterion is about paper-white consistency, not 8-bit quantisation of dark pixels)
    let cand: Vec<usize> = (0..(W1 * H1) as usize)
        .filter(|&i| srgb[i * 4..i * 4 + 3] != KEY[..3] && srgb[i * 4..i * 4 + 3].iter().all(|&c| (128..250).contains(&c)))
        .collect();
    let picks: Vec<usize> = (0..16).map(|k| cand[k * cand.len().max(1) / 16]).filter(|_| !cand.is_empty()).collect();
    // Criterion: the median relative difference <= 2% (paper white consistent: a wrong scale would move every
    // sample), and every sample within 2% plus one 8-bit sRGB step of its reference value (the sRGB reference is
    // quantised; blended strap pixels are also blended in different spaces per output mode).
    let (mut worst, mut over, mut all) = (0.0f32, 0usize, Vec::new());
    for &i in &picks {
        let a = linear_at(srgb, DXGI_FORMAT_R8G8B8A8_UNORM, i, 200.0);
        for (v, f) in [(&pq, DXGI_FORMAT_R10G10B10A2_UNORM), (&scrgb, DXGI_FORMAT_R16G16B16A16_FLOAT)] {
            let b = linear_at(v, f, i, 200.0);
            for k in 0..3 {
                let d = (a[k] - b[k]).abs() / a[k].max(1e-3);
                let c = srgb[i * 4 + k] as f32 / 255.0;
                let step = (srgb_to_linear((c + 1.0 / 255.0).min(1.0)) - srgb_to_linear(c)) / srgb_to_linear(c).max(1e-3);
                if d > 0.02 + step {
                    over += 1;
                }
                worst = worst.max(d);
                all.push(d);
            }
        }
    }
    all.sort_by(|a, b| a.total_cmp(b));
    let median = all.get(all.len() / 2).copied().unwrap_or(1.0);
    let ok = picks.len() == 16 && median <= 0.02 && over == 0;
    let errs = g.drain_errors();
    rep.add(
        ok && errs.is_empty(),
        "O",
        format!(
            "PQ R10G10B10A2 and scRGB FP16 vs sRGB at {} px (of {} with all channels 128..249): median relative difference {:.3}% (need <= 2%), worst {:.3}%, {over} samples beyond 2% + one sRGB step; InfoQueue {}",
            picks.len(),
            cand.len(),
            median * 100.0,
            worst * 100.0,
            errs.len()
        ),
    );
    Ok(())
}

/// F: missing / truncated / wrong-version assets exit 3 with `VM FAIL reason=assets: ...` (run as child
/// processes exactly like check_m3 does).
fn case_f(a: &Args, rep: &mut Report) -> Result<(), String> {
    let base = a.out.join("f_cases");
    let _ = std::fs::remove_dir_all(&base);
    let empty = base.join("empty");
    let trunc = base.join("truncated_mesh");
    let v2 = base.join("version2");
    for d in [&empty, &trunc, &v2] {
        std::fs::create_dir_all(d).map_err(|x| x.to_string())?;
    }
    for d in [&trunc, &v2] {
        for ent in std::fs::read_dir(&a.assets).map_err(|x| x.to_string())? {
            let p = ent.map_err(|x| x.to_string())?.path();
            std::fs::copy(&p, d.join(p.file_name().unwrap())).map_err(|x| x.to_string())?;
        }
    }
    let m = trunc.join("ump45.vmmesh");
    let d = std::fs::read(&m).map_err(|x| x.to_string())?;
    std::fs::write(&m, &d[..d.len() / 2]).map_err(|x| x.to_string())?;
    let s = v2.join("ump45.vmskel");
    let mut d = std::fs::read(&s).map_err(|x| x.to_string())?;
    d[4] = 2;
    std::fs::write(&s, &d).map_err(|x| x.to_string())?;
    let exe = std::env::current_exe().map_err(|x| x.to_string())?;
    let mut res = Vec::new();
    let mut ok = true;
    for dir in [&empty, &trunc, &v2] {
        let o = std::process::Command::new(&exe).args(["--load-only", "--assets"]).arg(dir).output().map_err(|x| x.to_string())?;
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let good = o.status.code() == Some(3) && text.contains("VM FAIL reason=assets: ") && !text.contains("panicked");
        ok &= good;
        res.push(format!("{}: exit {:?} {}", dir.file_name().unwrap().to_string_lossy(), o.status.code(), text.lines().next().unwrap_or("")));
    }
    // also the parser cannot be tricked by a manifest whose hashes were updated for the truncated mesh: the
    // structural checks still reject it (covered by mw2er-core unit tests); here: clean up
    let _ = std::fs::remove_dir_all(&base);
    rep.add(ok, "F", res.join(" | "));
    Ok(())
}

// ------------------------------------------------------------------------------------------------ compare
fn run_compare(dir: &Path) -> Result<bool, String> {
    let mut ok = true;
    println!("{:<18} {:>8} {:>10} {:>10} {:>9}  verdict", "pose", "IoU", "vmtest px", "blender px", "centroid");
    let mut rows = Vec::new();
    for p in TEST_POSES.iter() {
        let (w, h, a) = read_mask(&dir.join(format!("{}.mask", p.name)))?;
        let (w2, h2, b) = read_mask(&dir.join(format!("ref_{}.mask", p.name)))?;
        if (w, h) != (w2, h2) {
            return Err(format!("{}: sizes differ", p.name));
        }
        let inter = a.iter().zip(&b).filter(|(x, y)| **x == 1 && **y == 1).count();
        let uni = a.iter().zip(&b).filter(|(x, y)| **x == 1 || **y == 1).count();
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
        write_png(&dir.join(format!("diff_{}.png", p.name)), &diff, w, h)?;
        let line = format!(
            "{:<18} {:>8.4} {:>10} {:>10} {:>8.3}%  {}",
            p.name,
            iou,
            a.iter().filter(|&&v| v == 1).count(),
            b.iter().filter(|&&v| v == 1).count(),
            coff * 100.0,
            verdict
        );
        println!("{line}");
        rows.push(format!("{{\"pose\": \"{}\", \"iou\": {iou:.5}, \"centroid_off\": {coff:.5}}}", p.name));
    }
    std::fs::write(dir.join("iou.json"), format!("[{}]\n", rows.join(", "))).map_err(|x| x.to_string())?;
    println!("IOU {}", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

// ------------------------------------------------------------------------------------------------ perf
fn run_perf(a: &Args) -> Result<bool, String> {
    let assets = load(&a.assets)?;
    let bones = assets.skel.bones.len();
    let mut sc = Scene { assets, scratch: PoseScratch::new(bones) };
    let mut g = Gpu::new(false, false, a.warp)?;
    if g.software {
        println!("PERF FAIL perf needs hardware (adapter {} is software/WARP)", g.adapter);
        return Ok(false);
    }
    let mut r = Renderer::new(&g.dev, NUM_FRAMES, g.ts_freq).map_err(|x| x.to_string())?;
    r.upload(&sc.assets).map_err(|x| x.to_string())?;
    let t = new_target(&g, DXGI_FORMAT_R8G8B8A8_UNORM, W4, H4, 0)?;
    let pose = &TEST_POSES[0];
    let (pal, vp, view, _, _) = sc.pose(pose, W4, H4);
    let fp = FrameParams {
        palette: &pal,
        view_proj: vp,
        view,
        light: LightParams::default(),
        out: OutputMode::Srgb,
        paper_white_nits: 200.0,
        debug: DebugMode::None,
        variant: Variant::default(),
        timing: true,
    };
    let mut samples = Vec::new();
    for k in 0..320 {
        g.begin(0)?;
        // SAFETY: recording into our open list.
        unsafe { g.list.ClearRenderTargetView(t.rtv, &KEY_F, None) };
        r.record(&g.list, &Target { resource: &t.res, rtv: t.rtv, format: t.fmt, width: t.w, height: t.h }, 0, &fp)
            .map_err(|x| x.to_string())?;
        g.submit_wait()?;
        r.collect_timing(0);
        let s = r.take_timings();
        if k >= 20 {
            samples.extend(s);
        }
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    if samples.len() < 290 {
        println!("PERF FAIL only {} timestamp samples", samples.len());
        return Ok(false);
    }
    let q = |f: f64| samples[((samples.len() - 1) as f64 * f).round() as usize];
    let (p50, p95, max) = (q(0.5), q(0.95), samples[samples.len() - 1]);
    let ok = p95 < 1.0 && p50 < 0.5;
    std::fs::create_dir_all(&a.out).ok();
    let j = format!(
        "{{\"adapter\": \"{}\", \"width\": {W4}, \"height\": {H4}, \"frames\": {}, \"median_ms\": {p50:.4}, \"p95_ms\": {p95:.4}, \"max_ms\": {max:.4}, \"pass\": {ok}}}\n",
        g.adapter.replace('"', "'"),
        samples.len()
    );
    std::fs::write(a.out.join("perf.json"), j).map_err(|x| x.to_string())?;
    println!(
        "PERF {} {} @{W4}x{H4}: {} frames, median {p50:.4} ms, p95 {p95:.4} ms, max {max:.4} ms (need p95 < 1.0, median < 0.5)",
        if ok { "PASS" } else { "FAIL" },
        g.adapter,
        samples.len()
    );
    Ok(ok)
}

fn main() -> ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("vmtest: {e}");
            return ExitCode::from(2);
        }
    };
    let r: Result<bool, String> = match a.cmd.as_str() {
        "--version" => {
            println!(
                "vmtest {} VS_SHA256={} PS_SHA256={} VS_BYTES={} PS_BYTES={} THERMAL_VS_SHA256={} THERMAL_PS_SHA256={}",
                env!("CARGO_PKG_VERSION"),
                mw2er_render::VS_SHA256,
                mw2er_render::PS_SHA256,
                mw2er_render::VS.len(),
                mw2er_render::PS.len(),
                mw2er_render::thermal::THERMAL_VS_SHA256,
                mw2er_render::thermal::THERMAL_PS_SHA256
            );
            Ok(true)
        }
        "--write-poses" => poses_json(&a.out).map(|_| true),
        "--load-only" => match load(&a.assets) {
            Ok(x) => {
                println!("VM assets ok bones={} verts={} clips={} textures={} bytes={}", x.skel.bones.len(), x.mesh.vertex_count, x.clips.len(), x.textures.len(), x.total_bytes);
                Ok(true)
            }
            Err(e) => {
                println!("VM FAIL reason={e}");
                return ExitCode::from(3);
            }
        },
        "--all" => {
            if let Err(e) = load(&a.assets) {
                println!("VM FAIL reason={e}");
                return ExitCode::from(3);
            }
            run_all(&a)
        }
        "--compare" => run_compare(&a.out),
        "--arsenal" => arsenal_cases::run_arsenal(&arsenal_cases::ArsenalArgs { out: &a.out, guns: &a.guns, gbv: a.gbv, warp: a.warp }),
        "--compare-arsenal" => arsenal_cases::run_compare_arsenal(&a.out),
        "--c4" => c4_cases::run_c4(&c4_cases::C4Args { out: &a.out, assets: &a.assets, gbv: a.gbv, warp: a.warp }),
        "--occluders" => occl_cases::run_occl(&occl_cases::OcclArgs { out: &a.out, gbv: a.gbv, warp: a.warp }),
        "--erdepth" => erdepth_cases::run_erdepth(&erdepth_cases::ErDepthArgs { out: &a.out, gbv: a.gbv, warp: a.warp }),
        "--cutout" => cutout_cases::run_cutout(&cutout_cases::CutoutArgs { out: &a.out, assets: &a.assets, gbv: a.gbv, warp: a.warp }),
        "--nuke" | "--nuke-perf" => {
            let nuke = project_root().join("converted").join("nuke");
            let na = nuke_cases::NukeArgs { out: &a.out, nuke: &nuke, gbv: a.gbv, warp: a.warp };
            if a.cmd == "--nuke" { nuke_cases::run_nuke(&na) } else { nuke_cases::run_nuke_perf(&na) }
        }
        "--thermal" | "--thermal-perf" | "--thermal-image" | "--tracers" | "--tracers-perf" => {
            let (cfg_e, cfg_g) = thermal_cases::config_film_in(&project_root());
            let exposure = a.exposure.unwrap_or(cfg_e);
            let gamma = a.gamma.unwrap_or(cfg_g);
            let reference = project_root().join("work").join("m4").join(thermal_cases::ER_REFERENCE_PNG);
            let ta = thermal_cases::Args { out: &a.out, ac130: &a.ac130, gbv: a.gbv, warp: a.warp, exposure, gamma, reference: &reference };
            match a.cmd.as_str() {
                "--thermal" => thermal_cases::run_thermal(&ta),
                "--thermal-perf" => thermal_cases::run_thermal_perf(&ta),
                "--tracers" => tracer_cases::run_tracers(&ta),
                "--tracers-perf" => tracer_cases::run_tracers_perf(&ta),
                _ => thermal_cases::run_thermal_image(&ta, &a.image),
            }
        }
        "--perf" => {
            if let Err(e) = load(&a.assets) {
                println!("VM FAIL reason={e}");
                return ExitCode::from(3);
            }
            run_perf(&a)
        }
        _ => Err("unknown command".into()),
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            println!("FAIL vmtest: {e}");
            ExitCode::from(1)
        }
    }
}

