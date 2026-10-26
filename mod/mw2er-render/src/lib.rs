//! The M3 viewmodel renderer (docs/M3_DESIGN.md section 4), shared by `mw2er.dll` (inside hudhook's pre-ImGui
//! callback, on Elden Ring's present queue) and `vmtest.exe` (offscreen harness): same code, same shader bytes.
//!
//! - GPU skinning in the vertex shader (91 of 128 palette entries), one directional light + hemisphere ambient on
//!   the MW2 colour map, two PSOs (opaque, alpha) per render-target format and [`Variant`].
//! - Its own D32_FLOAT reversed-Z depth buffer, cleared every frame: the host's depth is never bound, read or
//!   tested (section 5).
//! - Per-frame constants live in a persistently mapped upload ring indexed by the caller's frame slot (hudhook's
//!   `frame_index`, whose fence hudhook waited on before calling us), so [`Renderer::record`] never waits.
//! - [`Renderer::record`] records no resource barriers on caller resources; the target must be in
//!   RENDER_TARGET state. It sets its full state itself (viewport, scissor, heaps, root signature, PSO, RT+DSV).
//! - [`Renderer::upload`] blocks on its own queue + fence: call it on a loader thread (or in vmtest) only.
//!
//! No hudhook dependency (the harness must not pull MinHook) and no Elden Ring types.

#![deny(unsafe_op_in_unsafe_fn)]

/// M4: the thermal post-process pass (docs/M4_DESIGN.md section 6).
pub mod thermal;
/// Run 8: the AC-130 round tracers (drawn before the thermal pass).
pub mod tracer;
/// M7: the nuke flash / white-out / aftermath post pass.
pub mod nuke;
/// C4: the early pass (our own lists spliced ahead of ER's UI; docs/C4_UI_DESIGN.md 3.3 / 4.3).
pub mod early;
/// C4.1 probe P2: the layer sample (our compute list reads 16 texels of ER's UI layer; docs/C4_1_DESIGN.md 2.4).
pub mod sample;
/// C6 depth fallback: depth-only body capsules in the world guns' own depth (docs/C6_HOLD_DESIGN.md 13.2).
pub mod occluder;
/// C6 depth 5: the read-only barrier hook's detour, the copy / sample of ER's scene depth, the prime (docs part 15).
pub mod er_depth;

pub use early::{EarlyPass, EarlyTarget, PsoBuilder, PsoSet, UI_TARGET_STATE};
pub use mw2er_core::c4::EarlyMode;
pub use sample::LayerSampler;

use std::mem::ManuallyDrop;

use mw2er_core::config::VmOutput;
use mw2er_core::vm_assets::{DXGI_BC1_UNORM, DXGI_BC2_UNORM, DXGI_BC3_UNORM, Mesh, Texture, VmAssets};
use mw2er_core::xform::{Mat4, Mat34};
use windows::Win32::Foundation::{CloseHandle, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, s};

/// Vertex shader (DXBC, vs_5_1) compiled by build.rs from shaders/viewmodel.hlsl.
pub const VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/viewmodel_vs.cso"));
/// Pixel shader (DXBC, ps_5_1).
pub const PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/viewmodel_ps.cso"));
pub const VS_SHA256: &str = env!("MW2ER_VS_SHA256");
pub const PS_SHA256: &str = env!("MW2ER_PS_SHA256");
/// M6: the scope overlay shaders (shaders/overlay.hlsl).
pub const OVERLAY_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay_vs.cso"));
pub const OVERLAY_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay_ps.cso"));
pub const OVERLAY_VS_SHA256: &str = env!("MW2ER_OVERLAY_VS_SHA256");
pub const OVERLAY_PS_SHA256: &str = env!("MW2ER_OVERLAY_PS_SHA256");
/// C4.1 PASS I41: one constant colour (shaders/solid.hlsl): the canary texel; vmtest's ER-like UI quads.
pub const SOLID_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/solid_vs.cso"));
pub const SOLID_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/solid_ps.cso"));
pub const SOLID_VS_SHA256: &str = env!("MW2ER_SOLID_VS_SHA256");
pub const SOLID_PS_SHA256: &str = env!("MW2ER_SOLID_PS_SHA256");

/// Constant buffer sizes and the per-slot layout of the upload ring.
pub const FRAME_CB_BYTES: usize = 256;
pub const MAX_PALETTE: usize = 128;
pub const PALETTE_CB_BYTES: usize = MAX_PALETTE * 48;
pub const SLOT_BYTES: usize = align_up(FRAME_CB_BYTES + PALETTE_CB_BYTES, 256);
/// M6: the shared SRV heap of every gun (34 guns share ~60 colour maps) + the scope overlay. C3 PASS C: + the world
/// models' colour maps (~45 more).
pub const MAX_TEXTURES: usize = 256;
/// C3 PASS C: world guns drawn per frame at most (each has its own constants in the frame's slot of the ring).
pub const MAX_WORLD_DRAWS: usize = 16;
/// Constant sub-slots per frame slot: 0 = the viewmodel, 1..=MAX_WORLD_DRAWS = world guns.
pub const DRAWS_PER_FRAME: usize = 1 + MAX_WORLD_DRAWS;
/// C3 PASS C: world-model LODs per gun slot.
pub const WORLD_LODS: usize = 4;
/// M6: gun slots (the arsenal list).
pub const MAX_GUNS: usize = 64;
pub const VERTEX_STRIDE: u32 = 40;

pub const fn align_up(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Bytes of one BC mip level.
pub fn bc_mip_bytes(format: u32, w: u32, h: u32) -> u64 {
    let block = if format == DXGI_BC1_UNORM { 8 } else { 16 };
    w.max(1).div_ceil(4) as u64 * h.max(1).div_ceil(4) as u64 * block
}

#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    D3d { call: &'static str, hr: i32 },
    Invalid(String),
    /// An upload that did not finish within UPLOAD_MAX_WAIT_MS on a live device (its objects were leaked).
    Stuck(String),
}

impl VmError {
    /// Review M6: the device is gone or hung, so every further upload would fail the same way (or wait 120 s again).
    pub fn is_device_fatal(&self) -> bool {
        match self {
            VmError::Stuck(_) => true,
            // DXGI_ERROR_DEVICE_REMOVED / _HUNG / _RESET / DRIVER_INTERNAL_ERROR
            VmError::D3d { call, hr } => {
                call.contains("device removed") || matches!(*hr as u32, 0x887A_0005 | 0x887A_0006 | 0x887A_0007 | 0x887A_0020)
            }
            VmError::Invalid(_) => false,
        }
    }
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::D3d { call, hr } => write!(f, "d3d: {call} 0x{:08x}", *hr as u32),
            VmError::Invalid(s) | VmError::Stuck(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for VmError {}

trait Ctx<T> {
    fn ctx(self, call: &'static str) -> Result<T, VmError>;
}

impl<T> Ctx<T> for windows::core::Result<T> {
    fn ctx(self, call: &'static str) -> Result<T, VmError> {
        self.map_err(|e| VmError::D3d { call, hr: e.code().0 })
    }
}

/// Shader output transform (`FrameConsts.out_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Srgb = 0,
    Pq = 1,
    Scrgb = 2,
}

/// `DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020` and `..._G22_NONE_P709`.
pub const COLORSPACE_PQ: i32 = 12;
pub const COLORSPACE_SRGB: i32 = 0;

/// Picks the output transform (docs/M3_DESIGN.md 4.4). `colorspace` is the last `SetColorSpace1` value, -1 if
/// it was never called. `Auto` refuses unknown combinations instead of guessing.
pub fn select_output(cfg: VmOutput, rtv_format: DXGI_FORMAT, colorspace: i32) -> Result<OutputMode, String> {
    match cfg {
        VmOutput::Srgb => return Ok(OutputMode::Srgb),
        VmOutput::Pq => return Ok(OutputMode::Pq),
        VmOutput::Scrgb => return Ok(OutputMode::Scrgb),
        VmOutput::Auto => {}
    }
    let sdr_space = colorspace == -1 || colorspace == COLORSPACE_SRGB;
    match rtv_format {
        DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(OutputMode::Scrgb),
        DXGI_FORMAT_R10G10B10A2_UNORM if colorspace == COLORSPACE_PQ => Ok(OutputMode::Pq),
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM if sdr_space => {
            Ok(OutputMode::Srgb)
        }
        f => Err(format!("unknown output fmt/colorspace {} / {colorspace}", format_name(f))),
    }
}

/// The format family of a back buffer for comparing hudhook's RTV format with the live resource: `_SRGB`
/// and `_TYPELESS` variants map to their `_UNORM` base (an sRGB RTV on a UNORM buffer is the same storage).
pub fn base_format(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_R8G8B8A8_TYPELESS => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        f => f,
    }
}

pub fn format_name(f: DXGI_FORMAT) -> String {
    match f {
        DXGI_FORMAT_R8G8B8A8_UNORM => "R8G8B8A8_UNORM".into(),
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => "R8G8B8A8_UNORM_SRGB".into(),
        DXGI_FORMAT_B8G8R8A8_UNORM => "B8G8R8A8_UNORM".into(),
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => "B8G8R8A8_UNORM_SRGB".into(),
        DXGI_FORMAT_R10G10B10A2_UNORM => "R10G10B10A2_UNORM".into(),
        DXGI_FORMAT_R16G16B16A16_FLOAT => "R16G16B16A16_FLOAT".into(),
        f => format!("DXGI_FORMAT({})", f.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullMode {
    Back,
    None,
    Front,
}

/// Rasterizer/depth variant of the PSOs. The DLL always uses the default; vmtest uses the others for the
/// winding (W) and depth-decides (D2) cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub cull: CullMode,
    pub front_ccw: bool,
    pub depth: bool,
}

impl Default for Variant {
    /// CULL_BACK with FrontCounterClockwise = TRUE. The design expected clockwise front faces (glTF CCW through
    /// the reflecting MW2 -> D3D axis map), but vmtest W measured the opposite: with FrontCCW = FALSE, CULL_BACK
    /// kept only 0.15% of the CULL_NONE image, with FrontCCW = TRUE it differs on 0.01%. The OAT GLB index order
    /// is therefore clockwise in its own right-handed space (IW4's D3D9 convention carried through). Decided from
    /// the test as section 3.4 requires; recorded in MODLOG (M3 deviations).
    fn default() -> Self {
        Variant { cull: CullMode::Back, front_ccw: true, depth: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugMode {
    None = 0,
    PartColor = 1,
    Silhouette = 2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightParams {
    pub dir_view: [f32; 3],
    pub diffuse: f32,
    pub ambient_sky: [f32; 3],
    pub ambient_ground: [f32; 3],
    pub exposure: f32,
}

impl Default for LightParams {
    fn default() -> Self {
        LightParams {
            dir_view: [0.35, -0.80, 0.50],
            diffuse: 0.85,
            ambient_sky: [0.42, 0.42, 0.45],
            ambient_ground: [0.20, 0.19, 0.18],
            exposure: 1.0,
        }
    }
}

/// The caller's render target (in RENDER_TARGET state).
pub struct Target<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub format: DXGI_FORMAT,
    pub width: u32,
    pub height: u32,
}

pub struct FrameParams<'a> {
    pub palette: &'a [Mat34],
    pub view_proj: Mat4,
    pub view: Mat4,
    pub light: LightParams,
    pub out: OutputMode,
    pub paper_white_nits: f32,
    pub debug: DebugMode,
    pub variant: Variant,
    /// Bracket the draw with timestamp queries (in game: `viewmodel.gpu_timing`).
    pub timing: bool,
}

/// C5 (`[ui_yield] mode = "cutout"`): the screen regions the viewmodel and the world guns leave alone, set with
/// [`Renderer::set_cutout`] before the frame's records. `rects` = [x0, y0, x1, y1] as fractions of the target,
/// `strength` 0..1 per region (1 = not drawn inside it), `feather` = the soft edge outside it as a fraction of the
/// target height. `n = 0` ([`Cutout::NONE`]) writes zeros: the shader takes exactly the path of before.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cutout {
    pub rects: [[f32; 4]; MAX_CUT_RECTS],
    pub strength: [f32; MAX_CUT_RECTS],
    pub n: usize,
    pub feather: f32,
}

/// The shader's `cut_rect` array length (viewmodel.hlsl FrameConsts).
pub const MAX_CUT_RECTS: usize = mw2er_core::ui_yield::MAX_CUT_RECTS;
const _: () = assert!(MAX_CUT_RECTS == 4 && 48 + 4 * MAX_CUT_RECTS == FRAME_CB_BYTES / 4, "cut_rect[4] ends FrameConsts");

impl Cutout {
    pub const NONE: Cutout = Cutout { rects: [[0.0; 4]; MAX_CUT_RECTS], strength: [0.0; MAX_CUT_RECTS], n: 0, feather: 0.0 };

    /// `fc[46]` (cut_count) and `fc[48..64]` (cut_rect) of the frame constants for a `w` x `h` target. Regions with
    /// no strength or no pixels are dropped; the rest are packed in pixels (x0 | y0 << 16, x1 | y1 << 16).
    pub fn write_consts(&self, fc: &mut [u32], w: u32, h: u32) {
        fc[46] = 0;
        fc[47] = 0;
        for v in fc[48..64].iter_mut() {
            *v = 0;
        }
        let feather_px = if self.feather.is_finite() { (self.feather.clamp(0.0, 1.0) * h as f32).max(0.0) } else { 0.0 };
        let mut k = 0usize;
        for (r, &s) in self.rects.iter().zip(&self.strength).take(self.n.min(MAX_CUT_RECTS)) {
            if !(s.is_finite() && s > 0.0) || !r.iter().all(|v| v.is_finite()) {
                continue;
            }
            let px = |f: f32, size: u32| (f.clamp(0.0, 1.0) * size as f32).round().clamp(0.0, size.min(0xffff) as f32) as u32;
            let (x0, y0, x1, y1) = (px(r[0], w), px(r[1], h), px(r[2], w), px(r[3], h));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            fc[48 + 4 * k] = x0 | (y0 << 16);
            fc[49 + 4 * k] = x1 | (y1 << 16);
            fc[50 + 4 * k] = s.min(1.0).to_bits();
            fc[51 + 4 * k] = feather_px.to_bits();
            k += 1;
        }
        fc[46] = k as u32;
    }
}

/// Raw pointer of a persistently mapped upload buffer owned by the renderer.
struct Mapped(*mut u8);
// SAFETY: the pointer is CPU-visible memory of a buffer the Renderer owns; the Renderer is used by one thread at a
// time (it is moved from the loader thread to the present thread behind a Mutex), never concurrently.
unsafe impl Send for Mapped {}

struct DrawItem {
    first: u32,
    count: u32,
    srv: u32,
    part: u32,
    alpha: bool,
}

struct GpuAssets {
    _vb: ID3D12Resource,
    _ib: ID3D12Resource,
    vbv: D3D12_VERTEX_BUFFER_VIEW,
    ibv: D3D12_INDEX_BUFFER_VIEW,
    draws: Vec<DrawItem>,
    bones: usize,
}

struct Psos {
    format: DXGI_FORMAT,
    variant: Variant,
    opaque: ID3D12PipelineState,
    alpha: ID3D12PipelineState,
}

struct Depth {
    res: ID3D12Resource,
    w: u32,
    h: u32,
}

pub struct Renderer {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    psos: Vec<Psos>,
    dsv_heap: ID3D12DescriptorHeap,
    depth: Option<Depth>,
    retired: Vec<(u64, ID3D12Resource)>,
    srv_heap: ID3D12DescriptorHeap,
    srv_inc: u32,
    /// M6: one slot per gun (M3: slot 0).
    guns: Vec<Option<GpuAssets>>,
    /// C3 PASS C: the world models (index = gun slot x WORLD_LODS + lod), rigid (one joint).
    world: Vec<Option<GpuAssets>>,
    /// M6: every uploaded texture (index = SRV slot) and the content keys of the shared ones.
    tex_res: Vec<ID3D12Resource>,
    tex_keys: std::collections::HashMap<String, u32>,
    uploads: u64,
    gpu_bytes: u64,
    overlay_srv: Option<u32>,
    overlay_rs: Option<ID3D12RootSignature>,
    overlay_psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    ring: ID3D12Resource,
    ring_ptr: Mapped,
    num_frames: usize,
    queries: ID3D12QueryHeap,
    readback: ID3D12Resource,
    slot_written: Vec<bool>,
    ts_freq: u64,
    samples: Vec<f32>,
    last_ms: Option<f32>,
    frames: u64,
    /// Depth buffers / PSOs that [`Renderer::record`] had to create on the caller's (present) thread since the
    /// last [`Renderer::take_present_allocs`]: each is a one-off hitch, reported so the DLL can log it.
    present_allocs: Vec<String>,
    /// C5: the regions `record_gun` / `record_world` leave alone ([`Cutout::NONE`] = none).
    cutout: Cutout,
    /// C6 depth fallback: the body-capsule pass (created by [`Renderer::prepare_occluders`] on the loader thread).
    occl: Option<occluder::Occluders>,
    /// C6 depth 5: the ER-depth prime pass (created by [`Renderer::prepare_er_depth`] on the loader thread) and whether
    /// the last `record_world_occluded` recorded it.
    prime: Option<er_depth::DepthPrime>,
    last_primed: bool,
}

fn heap_props(t: D3D12_HEAP_TYPE) -> D3D12_HEAP_PROPERTIES {
    D3D12_HEAP_PROPERTIES {
        Type: t,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 0,
        VisibleNodeMask: 0,
    }
}

fn buffer_desc(bytes: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: bytes.max(256),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

fn create_buffer(dev: &ID3D12Device, t: D3D12_HEAP_TYPE, bytes: u64, state: D3D12_RESOURCE_STATES) -> Result<ID3D12Resource, VmError> {
    let mut r: Option<ID3D12Resource> = None;
    // SAFETY: valid descriptors on the stack; out pointer to a local Option.
    unsafe { dev.CreateCommittedResource(&heap_props(t), D3D12_HEAP_FLAG_NONE, &buffer_desc(bytes), state, None, &mut r) }
        .ctx("CreateCommittedResource(buffer)")?;
    r.ok_or(VmError::Invalid("CreateCommittedResource returned null".into()))
}

/// A non-owning copy of an interface pointer for D3D12 structs that hold `ManuallyDrop<Option<T>>`.
fn weak<T: Interface + Clone>(r: &T) -> ManuallyDrop<Option<T>> {
    // SAFETY: `Option<T>` for a COM interface is a single non-null-or-null pointer; the copy is wrapped in
    // ManuallyDrop and never released, so the reference count is untouched and `r` outlives every use.
    unsafe { std::mem::transmute_copy(r) }
}

fn transition(r: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: weak(r),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// Blocks until `fence` reaches `value` (loader thread / vmtest only; 10 s timeout). A failing
/// `SetEventOnCompletion` is returned at once (the event would never be set).
pub fn wait_fence(fence: &ID3D12Fence, value: u64) -> Result<(), VmError> {
    // SAFETY: plain COM call.
    if unsafe { fence.GetCompletedValue() } >= value {
        return Ok(());
    }
    // SAFETY: an unnamed auto-reset event, closed below.
    let ev = unsafe { CreateEventW(None, false, false, None) }.ctx("CreateEventW")?;
    // SAFETY: the event handle is valid until CloseHandle.
    let r = unsafe { fence.SetEventOnCompletion(value, ev) }.ctx("SetEventOnCompletion");
    // SAFETY: valid handle; waited on only when SetEventOnCompletion succeeded.
    let w = if r.is_ok() { unsafe { WaitForSingleObject(ev, 10_000) } } else { WAIT_OBJECT_0 };
    // SAFETY: we own the handle.
    let _ = unsafe { CloseHandle(ev) };
    r?;
    if w != WAIT_OBJECT_0 {
        return Err(VmError::Invalid("fence wait timed out (10 s)".into()));
    }
    Ok(())
}

/// Result of [`wait_submitted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The fence reached the value: the GPU is done with the submitted work.
    Done,
    /// The device was removed (the work will never run; nothing can be read back).
    DeviceRemoved(i32),
    /// Still not done after `max_ms` on a live device: the work may still be running.
    StillRunning,
}

/// Polls `fence` (100 ms steps, no event needed) until it reaches `value`, the device is removed, or `max_ms`
/// passed. For work already submitted: unlike [`wait_fence`] a slow GPU is not an error, and the caller learns
/// whether the submitted objects may be released ([`Settled::Done`]) or must be leaked.
pub fn wait_submitted(device: &ID3D12Device, fence: &ID3D12Fence, value: u64, max_ms: u64) -> Settled {
    let t0 = std::time::Instant::now();
    loop {
        // SAFETY: plain COM calls.
        if unsafe { fence.GetCompletedValue() } >= value {
            return Settled::Done;
        }
        // SAFETY: as above.
        if let Err(e) = unsafe { device.GetDeviceRemovedReason() } {
            return Settled::DeviceRemoved(e.code().0);
        }
        if t0.elapsed().as_millis() as u64 >= max_ms {
            return Settled::StillRunning;
        }
        std::thread::sleep(std::time::Duration::from_millis(if t0.elapsed().as_millis() < 50 { 1 } else { 100 }));
    }
}

/// Objects referenced by submitted GPU work. They may only be released once that work finished; otherwise they
/// are leaked on purpose ([`InFlight::leak`]): a few MB lost beats a GPU page fault inside Elden Ring.
struct InFlight {
    _queue: ID3D12CommandQueue,
    _alloc: ID3D12CommandAllocator,
    _list: ID3D12GraphicsCommandList,
    _fence: ID3D12Fence,
    _staging: ID3D12Resource,
    _dst: Vec<ID3D12Resource>,
}

impl InFlight {
    fn leak(self) {
        std::mem::forget(self);
    }
}

/// How long the loader waits for its upload copy before giving up (and leaking the in-flight objects).
pub const UPLOAD_MAX_WAIT_MS: u64 = 120_000;

impl Renderer {
    /// Root signature, descriptor heaps, the upload ring and the timestamp heap. `num_frames` = the caller's frame
    /// slots (hudhook: 3); `ts_freq` = the queue's `GetTimestampFrequency` (0 disables timing).
    pub fn new(device: &ID3D12Device, num_frames: usize, ts_freq: u64) -> Result<Self, VmError> {
        if !(1..=8).contains(&num_frames) {
            return Err(VmError::Invalid(format!("num_frames {num_frames}")));
        }
        let root_sig = create_root_signature(device)?;
        // SAFETY: valid descriptor-heap descriptions.
        let dsv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(DSV)")?;
        // SAFETY: as above.
        let srv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: MAX_TEXTURES as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(SRV)")?;
        // SAFETY: plain query.
        let srv_inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) };
        let ring = create_buffer(device, D3D12_HEAP_TYPE_UPLOAD, (SLOT_BYTES * num_frames * DRAWS_PER_FRAME) as u64, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: an UPLOAD buffer may stay mapped for its lifetime; the CPU never reads it (empty read range).
        unsafe { ring.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(ring)")?;
        let mut queries: Option<ID3D12QueryHeap> = None;
        // SAFETY: valid description; out pointer to a local Option.
        unsafe {
            device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: (2 * num_frames) as u32, NodeMask: 0 },
                &mut queries,
            )
        }
        .ctx("CreateQueryHeap")?;
        let queries = queries.ok_or(VmError::Invalid("CreateQueryHeap returned null".into()))?;
        let readback = create_buffer(device, D3D12_HEAP_TYPE_READBACK, (16 * num_frames) as u64, D3D12_RESOURCE_STATE_COPY_DEST)?;
        Ok(Renderer {
            device: device.clone(),
            root_sig,
            psos: Vec::new(),
            dsv_heap,
            depth: None,
            retired: Vec::new(),
            srv_heap,
            srv_inc,
            guns: Vec::new(),
            world: Vec::new(),
            tex_res: Vec::new(),
            tex_keys: std::collections::HashMap::new(),
            uploads: 0,
            gpu_bytes: 0,
            overlay_srv: None,
            overlay_rs: None,
            overlay_psos: Vec::new(),
            ring,
            ring_ptr: Mapped(p as *mut u8),
            num_frames,
            queries,
            readback,
            slot_written: vec![false; num_frames],
            ts_freq,
            samples: Vec::with_capacity(1024),
            last_ms: None,
            frames: 0,
            present_allocs: Vec::new(),
            cutout: Cutout::NONE,
            occl: None,
            prime: None,
            last_primed: false,
        })
    }

    /// C5: the regions the next `record_gun` / `record_world` calls leave alone (the DLL sets it every frame,
    /// [`Cutout::NONE`] while ER shows no message). The early pass (C4) never applies it.
    pub fn set_cutout(&mut self, c: Cutout) {
        self.cutout = c;
    }

    pub fn is_ready(&self) -> bool {
        self.guns.iter().any(|g| g.is_some())
    }

    /// The device the renderer was created on (C4: the early pass is created on the same one).
    pub fn device(&self) -> &ID3D12Device {
        &self.device
    }

    /// Creates the PSOs for `format` (and `variant`) and the depth buffer for `width` x `height` ahead of the first
    /// frame. The DLL calls this on the loader thread with the swap chain's current format and size, so the
    /// present thread neither compiles pipelines nor allocates the ~33 MB 4K depth buffer on the first frame.
    /// A later format or size change still creates them in [`Renderer::record`] (one-off hitch, reported by
    /// [`Renderer::take_present_allocs`]).
    pub fn prepare(&mut self, format: DXGI_FORMAT, variant: Variant, width: u32, height: u32) -> Result<(), VmError> {
        self.pso_index(format, variant)?;
        if width > 0 && height > 0 && width <= 16384 && height <= 16384 {
            self.ensure_depth(width, height)?;
        }
        Ok(())
    }

    /// What `record` had to create on the present thread since the last call (empty normally).
    pub fn take_present_allocs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.present_allocs)
    }

    fn pso_index(&mut self, format: DXGI_FORMAT, variant: Variant) -> Result<usize, VmError> {
        if let Some(i) = self.psos.iter().position(|p| p.format == format && p.variant == variant) {
            return Ok(i);
        }
        let opaque = create_pso(&self.device, &self.root_sig, format, variant, false)?;
        let alpha = create_pso(&self.device, &self.root_sig, format, variant, true)?;
        self.psos.push(Psos { format, variant, opaque, alpha });
        Ok(self.psos.len() - 1)
    }

    /// Uploads mesh + textures through a staging buffer on its own direct queue and blocks until done (the M3 single
    /// gun = slot 0).
    pub fn upload(&mut self, a: &VmAssets) -> Result<u64, VmError> {
        self.upload_gun(0, a)
    }

    /// M6: uploads one gun into its slot (docs/M6_DESIGN.md 4.3). Textures with a content hash (`Texture::sha256`, the
    /// arsenal's deduplicated `_tex` files) are uploaded once and shared by every gun that uses them; switching
    /// guns later binds another slot and uploads nothing. Blocks until the copy is done (loader thread / vmtest).
    pub fn upload_gun(&mut self, gun: usize, a: &VmAssets) -> Result<u64, VmError> {
        if gun >= MAX_GUNS {
            return Err(VmError::Invalid(format!("gun slot {gun} >= {MAX_GUNS}")));
        }
        if a.skel.bones.len() > MAX_PALETTE {
            return Err(VmError::Invalid(format!("{} bones > {MAX_PALETTE}", a.skel.bones.len())));
        }
        let (assets, bytes) = self.upload_mesh(&format!("gun{gun}"), &a.mesh, &a.textures, a.skel.bones.len())?;
        if self.guns.len() <= gun {
            self.guns.resize_with(gun + 1, || None);
        }
        self.guns[gun] = Some(assets);
        Ok(bytes)
    }

    /// C3 PASS C: uploads one LOD of a gun's world model (rigid: one joint) into its world slot. Loader thread only.
    pub fn upload_world(&mut self, gun: usize, lod: u8, mesh: &Mesh, textures: &[std::sync::Arc<Texture>]) -> Result<u64, VmError> {
        if gun >= MAX_GUNS || lod as usize >= WORLD_LODS {
            return Err(VmError::Invalid(format!("world slot {gun}/{lod} out of range")));
        }
        let (assets, bytes) = self.upload_mesh(&format!("world{gun}.{lod}"), mesh, textures, 1)?;
        let i = gun * WORLD_LODS + lod as usize;
        if self.world.len() <= i {
            self.world.resize_with(i + 1, || None);
        }
        self.world[i] = Some(assets);
        Ok(bytes)
    }

    /// C3 PASS C: a world model of `gun` at `lod` (or a lower LOD) is on the GPU.
    pub fn world_ready(&self, gun: usize, lod: u8) -> bool {
        (0..=lod as usize).any(|l| self.world.get(gun * WORLD_LODS + l).is_some_and(|g| g.is_some()))
    }

    /// Mesh + textures -> GPU buffers and SRVs (textures with a content hash are shared; `key` names the others).
    fn upload_mesh(&mut self, key: &str, mesh: &Mesh, textures: &[std::sync::Arc<Texture>], bones: usize) -> Result<(GpuAssets, u64), VmError> {
        // texture -> SRV slot: reuse a shared texture, upload the others
        let mut srv_of = Vec::with_capacity(textures.len());
        let mut fresh: Vec<(usize, &Texture)> = Vec::new();
        let mut pending: Vec<(String, u32)> = Vec::new();
        for (i, t) in textures.iter().enumerate() {
            let key = if t.sha256.is_empty() { format!("{key}/{i}/{}", self.uploads) } else { t.sha256.clone() };
            if let Some(&srv) = self.tex_keys.get(&key) {
                srv_of.push(srv);
            } else {
                let srv = self.tex_res.len() + fresh.len();
                if srv >= MAX_TEXTURES {
                    return Err(VmError::Invalid(format!("more than {MAX_TEXTURES} textures")));
                }
                srv_of.push(srv as u32);
                fresh.push((i, t));
                pending.push((key, srv as u32));
            }
        }
        let ib: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let res = self.upload_core(Some((&mesh.vertex_bytes, &ib)), &fresh.iter().map(|(_, t)| *t).collect::<Vec<_>>());
        let (bufs, texs, bytes) = res?;
        for ((key, srv), r) in pending.into_iter().zip(texs) {
            self.tex_keys.insert(key, srv);
            self.tex_res.push(r);
        }
        let (vb, ibuf) = bufs.ok_or(VmError::Invalid("no vertex buffer".into()))?;
        let mut draws = Vec::new();
        for alpha in [false, true] {
            for s in &mesh.surfaces {
                let m = &mesh.materials[s.material as usize];
                if (m.blend == 1) == alpha {
                    draws.push(DrawItem { first: s.first_index, count: s.index_count, srv: srv_of[m.texture as usize], part: s.part as u32, alpha });
                }
            }
        }
        let (vb_bytes, ib_bytes) = (mesh.vertex_bytes.len() as u64, ib.len() as u64);
        // SAFETY: plain queries.
        let (vb_va, ib_va) = unsafe { (vb.GetGPUVirtualAddress(), ibuf.GetGPUVirtualAddress()) };
        let assets = GpuAssets {
            vbv: D3D12_VERTEX_BUFFER_VIEW { BufferLocation: vb_va, SizeInBytes: vb_bytes as u32, StrideInBytes: VERTEX_STRIDE },
            ibv: D3D12_INDEX_BUFFER_VIEW { BufferLocation: ib_va, SizeInBytes: ib_bytes as u32, Format: DXGI_FORMAT_R16_UINT },
            _vb: vb,
            _ib: ibuf,
            draws,
            bones,
        };
        self.uploads += 1;
        self.gpu_bytes += bytes;
        Ok((assets, bytes))
    }

    /// M6: uploads the sniper scope overlay (one BC texture, any mip count); returns its SRV slot for `record_overlay`.
    pub fn upload_overlay(&mut self, t: &Texture) -> Result<u32, VmError> {
        let srv = self.tex_res.len();
        if srv >= MAX_TEXTURES {
            return Err(VmError::Invalid(format!("more than {MAX_TEXTURES} textures")));
        }
        let (_, texs, bytes) = self.upload_core(None, &[t])?;
        self.tex_res.extend(texs);
        self.gpu_bytes += bytes;
        self.overlay_srv = Some(srv as u32);
        Ok(srv as u32)
    }

    /// Bytes of GPU memory the uploads allocated (VB/IB + textures, footprint sizes).
    pub fn gpu_bytes(&self) -> u64 {
        self.gpu_bytes
    }

    /// Number of uploads (vmtest: switching must not upload).
    pub fn upload_count(&self) -> u64 {
        self.uploads
    }

    /// Slots with uploaded assets.
    pub fn gun_ready(&self, gun: usize) -> bool {
        self.guns.get(gun).is_some_and(|g| g.is_some())
    }

    /// Creates the buffers (if any) and the textures, copies through one staging buffer on an own queue, waits, and
    /// creates the textures' SRVs at `self.tex_res.len()..`. Returns (vb/ib, textures, bytes).
    #[allow(clippy::type_complexity)]
    fn upload_core(&mut self, vb_ib: Option<(&[u8], &[u8])>, textures: &[&Texture]) -> Result<(Option<(ID3D12Resource, ID3D12Resource)>, Vec<ID3D12Resource>, u64), VmError> {
        let dev = self.device.clone();
        let (vb_bytes, ib_bytes) = vb_ib.map_or((0u64, 0u64), |(v, i)| (v.len() as u64, i.len() as u64));
        let bufs = match vb_ib {
            Some(_) => Some((
                create_buffer(&dev, D3D12_HEAP_TYPE_DEFAULT, vb_bytes, D3D12_RESOURCE_STATE_COMMON)?,
                create_buffer(&dev, D3D12_HEAP_TYPE_DEFAULT, ib_bytes, D3D12_RESOURCE_STATE_COMMON)?,
            )),
            None => None,
        };

        // Textures (typeless so the SRV can be _SRGB) and their copyable footprints in the staging buffer.
        let mut texs = Vec::new();
        let mut layouts: Vec<Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>> = Vec::new();
        let mut offset = align_up((vb_bytes + ib_bytes) as usize, 512) as u64;
        for t in textures {
            let typeless = match t.format {
                DXGI_BC1_UNORM => DXGI_FORMAT_BC1_TYPELESS,
                DXGI_BC2_UNORM => DXGI_FORMAT_BC2_TYPELESS,
                _ => DXGI_FORMAT_BC3_TYPELESS,
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: t.width as u64,
                Height: t.height,
                DepthOrArraySize: 1,
                MipLevels: t.mips.len() as u16,
                Format: typeless,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let mut r: Option<ID3D12Resource> = None;
            // SAFETY: valid descriptions; out pointer to a local Option.
            unsafe {
                dev.CreateCommittedResource(
                    &heap_props(D3D12_HEAP_TYPE_DEFAULT),
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut r,
                )
            }
            .ctx("CreateCommittedResource(texture)")?;
            let n = t.mips.len();
            let mut lay = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
            let mut rows = vec![0u32; n];
            let mut row_bytes = vec![0u64; n];
            let mut total = 0u64;
            // SAFETY: the output arrays have `n` entries each.
            unsafe {
                dev.GetCopyableFootprints(
                    &desc,
                    0,
                    n as u32,
                    offset,
                    Some(lay.as_mut_ptr()),
                    Some(rows.as_mut_ptr()),
                    Some(row_bytes.as_mut_ptr()),
                    Some(&mut total),
                )
            };
            for (m, mip) in t.mips.iter().enumerate() {
                if row_bytes[m] != mip.row_pitch as u64 || rows[m] != mip.rows {
                    return Err(VmError::Invalid(format!("texture mip {m}: footprint {}x{} != {}x{}", row_bytes[m], rows[m], mip.row_pitch, mip.rows)));
                }
            }
            offset = align_up((offset + total) as usize, 512) as u64;
            texs.push(r.ok_or(VmError::Invalid("texture null".into()))?);
            layouts.push(lay);
        }
        let staging = create_buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, offset, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: mapping an UPLOAD buffer for writing.
        unsafe { staging.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(staging)")?;
        let base = p as *mut u8;
        // SAFETY: every write below stays inside [0, offset) of the mapped staging buffer: VB at 0, IB after it,
        // texture rows at their footprint offsets (computed by GetCopyableFootprints for this buffer size).
        unsafe {
            if let Some((v, i)) = vb_ib {
                std::ptr::copy_nonoverlapping(v.as_ptr(), base, vb_bytes as usize);
                std::ptr::copy_nonoverlapping(i.as_ptr(), base.add(vb_bytes as usize), ib_bytes as usize);
            }
            for (t, lay) in textures.iter().zip(&layouts) {
                for (mip, l) in t.mips.iter().zip(lay) {
                    for r in 0..mip.rows as usize {
                        let src = &mip.data[r * mip.row_pitch as usize..(r + 1) * mip.row_pitch as usize];
                        let dst = base.add(l.Offset as usize + r * l.Footprint.RowPitch as usize);
                        std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
                    }
                }
            }
            staging.Unmap(0, None);
        }

        // Record the copies on our own queue and wait.
        // SAFETY: plain COM object creation.
        let queue: ID3D12CommandQueue = unsafe {
            dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: 0,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .ctx("CreateCommandQueue(upload)")?;
        // SAFETY: as above.
        let alloc: ID3D12CommandAllocator =
            unsafe { dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ctx("CreateCommandAllocator(upload)")?;
        // SAFETY: as above.
        let list: ID3D12GraphicsCommandList = unsafe {
            dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None::<&ID3D12PipelineState>)
        }
        .ctx("CreateCommandList(upload)")?;
        // The fence exists before anything is submitted, so no error path between submission and completion
        // can drop objects the GPU still uses.
        // SAFETY: plain COM object creation.
        let fence: ID3D12Fence = unsafe { dev.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ctx("CreateFence(upload)")?;
        // SAFETY: recording into our own fresh list; every resource is alive until the fence wait below.
        unsafe {
            if let Some((vb, ibuf)) = &bufs {
                list.CopyBufferRegion(vb, 0, &staging, 0, vb_bytes);
                list.CopyBufferRegion(ibuf, 0, &staging, vb_bytes, ib_bytes);
            }
            let mut barriers = Vec::new();
            for (t, lay) in texs.iter().zip(&layouts) {
                for (m, l) in lay.iter().enumerate() {
                    let dst = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: weak(t),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
                    };
                    let src = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: weak(&staging),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *l },
                    };
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                }
                barriers.push(transition(t, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE));
            }
            // Buffers decay to COMMON after the copy queue work and are promoted to VB/IB reads implicitly.
            if !barriers.is_empty() {
                list.ResourceBarrier(&barriers);
            }
            list.Close().ctx("Close(upload)")?;
        }
        let cmd: ID3D12CommandList = list.cast().ctx("cast(list)")?;
        // SAFETY: submitting our own closed list; from here on the objects below are in flight.
        unsafe { queue.ExecuteCommandLists(&[Some(cmd)]) };
        // SAFETY: signalling our own queue.
        let signalled = unsafe { queue.Signal(&fence, 1) }.ctx("Signal(upload)");
        let mut dst = texs.clone();
        if let Some((vb, ibuf)) = &bufs {
            dst.push(vb.clone());
            dst.push(ibuf.clone());
        }
        let inflight = InFlight { _queue: queue, _alloc: alloc, _list: list, _fence: fence.clone(), _staging: staging, _dst: dst };
        if let Err(e) = signalled {
            // Without a signal there is no way to know when the copy ends: never release what it touches.
            std::mem::forget((bufs, texs));
            inflight.leak();
            return Err(e);
        }
        match wait_submitted(&dev, &fence, 1, UPLOAD_MAX_WAIT_MS) {
            Settled::Done => drop(inflight),
            Settled::DeviceRemoved(hr) => {
                std::mem::forget((bufs, texs));
                inflight.leak();
                return Err(VmError::D3d { call: "upload wait: device removed", hr });
            }
            Settled::StillRunning => {
                std::mem::forget((bufs, texs));
                inflight.leak();
                return Err(VmError::Stuck(format!("upload copy not finished after {} s (objects leaked, not freed)", UPLOAD_MAX_WAIT_MS / 1000)));
            }
        }

        // SRVs (sRGB views of the BC data) at the next free slots of the shared heap.
        // SAFETY: plain query.
        let cpu0 = unsafe { self.srv_heap.GetCPUDescriptorHandleForHeapStart() };
        let first = self.tex_res.len();
        for (k, (t, r)) in textures.iter().zip(&texs).enumerate() {
            let fmt = match t.format {
                DXGI_BC1_UNORM => DXGI_FORMAT_BC1_UNORM_SRGB,
                DXGI_BC2_UNORM => DXGI_FORMAT_BC2_UNORM_SRGB,
                _ => DXGI_FORMAT_BC3_UNORM_SRGB,
            };
            let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: fmt,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: t.mips.len() as u32, PlaneSlice: 0, ResourceMinLODClamp: 0.0 },
                },
            };
            let h = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu0.ptr + (first + k) * self.srv_inc as usize };
            // SAFETY: first + k < MAX_TEXTURES (checked by the callers); the resource is a typeless BC texture.
            unsafe { dev.CreateShaderResourceView(r, Some(&desc), h) };
        }
        let _ = DXGI_BC3_UNORM;
        Ok((bufs, texs, offset))
    }

    fn ensure_depth(&mut self, w: u32, h: u32) -> Result<(), VmError> {
        if self.depth.as_ref().is_some_and(|d| d.w == w && d.h == h) {
            return Ok(());
        }
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL | D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE,
        };
        let clear = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_D32_FLOAT,
            Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 } },
        };
        let mut r: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; out pointer to a local Option.
        unsafe {
            self.device.CreateCommittedResource(
                &heap_props(D3D12_HEAP_TYPE_DEFAULT),
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_DEPTH_WRITE,
                Some(&clear),
                &mut r,
            )
        }
        .ctx("CreateCommittedResource(depth)")?;
        let r = r.ok_or(VmError::Invalid("depth null".into()))?;
        // SAFETY: the DSV heap has one slot; DSV descriptors are consumed when OMSetRenderTargets is recorded, so
        // rewriting the slot does not affect lists recorded earlier (the old resource itself is retired below).
        unsafe {
            self.device.CreateDepthStencilView(
                &r,
                Some(&D3D12_DEPTH_STENCIL_VIEW_DESC {
                    Format: DXGI_FORMAT_D32_FLOAT,
                    ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
                    Flags: D3D12_DSV_FLAG_NONE,
                    Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 } },
                }),
                self.dsv_heap.GetCPUDescriptorHandleForHeapStart(),
            )
        };
        if let Some(old) = self.depth.replace(Depth { res: r, w, h }) {
            self.retired.push((self.frames, old.res));
        }
        Ok(())
    }

    /// Reads the timestamps the frame recorded in `slot` earlier wrote (valid once the caller waited on that
    /// slot's fence, as hudhook does before reusing a frame index).
    pub fn collect_timing(&mut self, slot: usize) {
        if slot >= self.num_frames || !self.slot_written[slot] || self.ts_freq == 0 {
            return;
        }
        self.slot_written[slot] = false;
        let range = D3D12_RANGE { Begin: slot * 16, End: slot * 16 + 16 };
        let mut p = std::ptr::null_mut();
        // SAFETY: mapping a READBACK buffer for reading the 16 bytes of this slot.
        if unsafe { self.readback.Map(0, Some(&range), Some(&mut p)) }.is_err() {
            return;
        }
        // SAFETY: the mapped buffer holds 16 * num_frames bytes; slot < num_frames.
        let (a, b) = unsafe {
            let q = (p as *const u8).add(slot * 16) as *const u64;
            (q.read_unaligned(), q.add(1).read_unaligned())
        };
        // SAFETY: nothing was written by the CPU.
        unsafe { self.readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 })) };
        if b > a {
            let ms = (b - a) as f64 * 1000.0 / self.ts_freq as f64;
            if ms.is_finite() && ms < 1000.0 {
                self.last_ms = Some(ms as f32);
                if self.samples.len() < 100_000 {
                    self.samples.push(ms as f32);
                }
            }
        }
    }

    /// GPU milliseconds of the most recent completed timed frame.
    pub fn timing(&self) -> Option<f32> {
        self.last_ms
    }

    /// Drains the collected GPU-time samples (ms).
    pub fn take_timings(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    /// Records the viewmodel into `list` (target in RENDER_TARGET state). Never waits, records no barriers on the
    /// target, and sets every piece of pipeline state it uses. (The M3 single gun: slot 0, every part drawn.)
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, t: &Target<'_>, slot: usize, p: &FrameParams<'_>) -> Result<(), VmError> {
        self.record_gun(list, t, slot, 0, 0, p)
    }

    /// M6: records gun `gun` (its uploaded slot); `hide_parts` is a bit mask of mesh parts not drawn (bit 2 = the RPG's
    /// rocket while the magazine is empty).
    pub fn record_gun(&mut self, list: &ID3D12GraphicsCommandList, t: &Target<'_>, slot: usize, gun: usize, hide_parts: u32, p: &FrameParams<'_>) -> Result<(), VmError> {
        if slot >= self.num_frames {
            return Err(VmError::Invalid(format!("slot {slot} >= {}", self.num_frames)));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
        }
        let bones = self.guns.get(gun).and_then(|g| g.as_ref()).map(|g| g.bones).ok_or(VmError::Invalid(format!("gun slot {gun}: assets not uploaded")))?;
        if p.palette.len() < bones || p.palette.len() > MAX_PALETTE {
            return Err(VmError::Invalid(format!("palette {} for {bones} bones", p.palette.len())));
        }
        self.frames += 1;
        let frames = self.frames;
        let keep = self.num_frames as u64 + 1;
        self.retired.retain(|(f, _)| frames < f + keep);
        self.collect_timing(slot);
        let n_psos = self.psos.len();
        let pi = self.pso_index(t.format, p.variant)?;
        if self.psos.len() != n_psos {
            self.present_allocs.push(format!("PSOs for {}", format_name(t.format)));
        }
        if !self.depth.as_ref().is_some_and(|d| d.w == t.width && d.h == t.height) {
            self.ensure_depth(t.width, t.height)?;
            self.present_allocs.push(format!("depth {}x{}", t.width, t.height));
        }

        // Constants into this slot of the ring.
        let mut fc = [0u32; FRAME_CB_BYTES / 4];
        let put = |fc: &mut [u32], at: usize, v: f32| fc[at] = v.to_bits();
        for r in 0..4 {
            for c in 0..4 {
                put(&mut fc, r * 4 + c, p.view_proj[r][c]);
                put(&mut fc, 16 + r * 4 + c, p.view[r][c]);
            }
        }
        let l = &p.light;
        let n = mw2er_core::xform::v_normalize(l.dir_view);
        for (i, v) in [n[0], n[1], n[2], l.diffuse, l.ambient_sky[0], l.ambient_sky[1], l.ambient_sky[2], l.exposure, l.ambient_ground[0], l.ambient_ground[1], l.ambient_ground[2]]
            .into_iter()
            .enumerate()
        {
            put(&mut fc, 32 + i, v);
        }
        fc[43] = p.out as u32;
        put(&mut fc, 44, p.paper_white_nits);
        fc[45] = p.debug as u32;
        self.cutout.write_consts(&mut fc, t.width, t.height);
        // C3 PASS C: sub-slot 0 of the frame's constants (1.. are the world guns')
        let slot_off = slot * DRAWS_PER_FRAME * SLOT_BYTES;
        // SAFETY: the ring is mapped for its lifetime with SLOT_BYTES * num_frames bytes; slot < num_frames; the
        // GPU finished reading this slot (the caller waited on its fence before reusing the frame index).
        unsafe {
            let dst = self.ring_ptr.0.add(slot_off);
            std::ptr::copy_nonoverlapping(fc.as_ptr() as *const u8, dst, FRAME_CB_BYTES);
            std::ptr::copy_nonoverlapping(p.palette.as_ptr() as *const u8, dst.add(FRAME_CB_BYTES), p.palette.len() * 48);
        }
        // SAFETY: plain queries.
        let (ring_va, dsv, srv_gpu) = unsafe {
            (
                self.ring.GetGPUVirtualAddress(),
                self.dsv_heap.GetCPUDescriptorHandleForHeapStart(),
                self.srv_heap.GetGPUDescriptorHandleForHeapStart(),
            )
        };
        let gpu = self.guns.get(gun).and_then(|g| g.as_ref()).ok_or(VmError::Invalid("assets not uploaded".into()))?;
        let psos = &self.psos[pi];
        let timing = p.timing && self.ts_freq > 0;
        // SAFETY: recording into the caller's open command list; every object referenced is owned by `self` (or
        // the caller's target) and outlives the GPU work (retired depth buffers are kept num_frames + 1 frames).
        unsafe {
            if timing {
                list.EndQuery(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot) as u32);
            }
            list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: t.width as f32,
                Height: t.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv));
            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
            list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.IASetVertexBuffers(0, Some(&[gpu.vbv]));
            list.IASetIndexBuffer(Some(&gpu.ibv));
            list.SetGraphicsRootConstantBufferView(0, ring_va + slot_off as u64);
            list.SetGraphicsRootConstantBufferView(1, ring_va + (slot_off + FRAME_CB_BYTES) as u64);
            let mut cur_alpha = None;
            for d in &gpu.draws {
                if d.part < 32 && hide_parts & (1 << d.part) != 0 {
                    continue;
                }
                if cur_alpha != Some(d.alpha) {
                    list.SetPipelineState(if d.alpha { &psos.alpha } else { &psos.opaque });
                    cur_alpha = Some(d.alpha);
                }
                let h = D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu.ptr + d.srv as u64 * self.srv_inc as u64 };
                list.SetGraphicsRootDescriptorTable(2, h);
                let rc = [d.part, d.alpha as u32, 0, 0];
                list.SetGraphicsRoot32BitConstants(3, 4, rc.as_ptr() as *const _, 0);
                list.DrawIndexedInstanced(d.count, 1, d.first, 0, 0);
            }
            if timing {
                list.EndQuery(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot + 1) as u32);
                list.ResolveQueryData(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot) as u32, 2, &self.readback, (slot * 16) as u64);
            }
        }
        if timing {
            self.slot_written[slot] = true;
        }
        Ok(())
    }
}

/// C3 PASS C: one world gun to draw: gun slot, LOD, model (MW2 inches) -> ER world (metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldDraw {
    pub gun: usize,
    pub lod: u8,
    pub model: Mat34,
}

/// C3 PASS C: the frame of the world guns: ER world -> D3D view (`view`) and the reversed-Z `view_proj` of ER's camera.
pub struct WorldFrame {
    pub view: Mat4,
    pub view_proj: Mat4,
    pub light: LightParams,
    pub out: OutputMode,
    pub paper_white_nits: f32,
    pub variant: Variant,
}

impl Renderer {
    /// C3 PASS C (docs/C3_DESIGN.md 3.6): records up to [`MAX_WORLD_DRAWS`] world guns into `list` (target in
    /// RENDER_TARGET state) with their own depth (cleared once here: our depth only, ER's is never bound), each with its
    /// constants in sub-slot 1.. of the frame slot. Never waits, records no barriers. Returns the guns drawn (a gun whose
    /// world model is not uploaded falls back to a lower LOD or is skipped). Call before the viewmodel (which clears the
    /// depth again and draws on top).
    pub fn record_world(&mut self, list: &ID3D12GraphicsCommandList, t: &Target<'_>, slot: usize, draws: &[WorldDraw], f: &WorldFrame) -> Result<u32, VmError> {
        self.record_world_occluded(list, t, slot, draws, f, &[], None).map(|(n, _)| n)
    }

    /// C6 depth fallback: creates the body-capsule pass (root signature, PSO, ring). Loader thread / vmtest only; a
    /// failure leaves the renderer without it (the world guns then draw as before).
    pub fn prepare_occluders(&mut self) -> Result<(), VmError> {
        if self.occl.is_none() {
            self.occl = Some(occluder::Occluders::new(&self.device, self.num_frames)?);
        }
        Ok(())
    }

    pub fn has_occluders(&self) -> bool {
        self.occl.is_some()
    }

    /// C6 depth 5: creates the ER-depth prime pass (root signature, depth-only PSO, its SRV heap). Loader thread / vmtest
    /// only; a failure leaves the renderer without it (the world guns then draw with the capsules only).
    pub fn prepare_er_depth(&mut self) -> Result<(), VmError> {
        if self.prime.is_none() {
            self.prime = Some(er_depth::DepthPrime::new(&self.device, self.num_frames)?);
        }
        Ok(())
    }

    pub fn has_er_depth(&self) -> bool {
        self.prime.is_some()
    }

    /// The last `record_world_occluded` primed our depth from ER's (C6 depth 5).
    pub fn last_primed(&self) -> bool {
        self.last_primed
    }

    /// [`Renderer::record_world`] with `caps`: after the depth clear, the capsules are drawn depth-only into OUR depth
    /// (never ER's), then the world guns test against it. With no capsules (or no prepared pass) the recorded commands
    /// are exactly `record_world`'s. Returns (guns drawn, capsules drawn).
    /// C6 depth 5: `prime` (this frame's copy of ER's depth, third person only) is written depth-only into OUR depth
    /// right after the clear and before the capsules; with None (or no prepared prime pass) the commands are exactly
    /// those without it ([`Renderer::last_primed`] tells whether it was recorded).
    pub fn record_world_occluded(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        t: &Target<'_>,
        slot: usize,
        draws: &[WorldDraw],
        f: &WorldFrame,
        caps: &[mw2er_core::body_occl::Capsule],
        prime: Option<&er_depth::PrimeIn<'_>>,
    ) -> Result<(u32, u32), VmError> {
        self.last_primed = false;
        if slot >= self.num_frames {
            return Err(VmError::Invalid(format!("slot {slot} >= {}", self.num_frames)));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
        }
        if draws.is_empty() {
            return Ok((0, 0));
        }
        let n_psos = self.psos.len();
        let pi = self.pso_index(t.format, f.variant)?;
        if self.psos.len() != n_psos {
            self.present_allocs.push(format!("PSOs for {}", format_name(t.format)));
        }
        if !self.depth.as_ref().is_some_and(|d| d.w == t.width && d.h == t.height) {
            self.ensure_depth(t.width, t.height)?;
            self.present_allocs.push(format!("depth {}x{}", t.width, t.height));
        }
        // SAFETY: plain queries.
        let (ring_va, dsv, srv_gpu) = unsafe {
            (self.ring.GetGPUVirtualAddress(), self.dsv_heap.GetCPUDescriptorHandleForHeapStart(), self.srv_heap.GetGPUDescriptorHandleForHeapStart())
        };
        let mut drawn = 0u32;
        let mut caps_drawn = 0u32;
        // SAFETY: recording into the caller's open command list; every object referenced is owned by `self` (or the
        // caller's target) and outlives the GPU work; the constants of each draw go to their own sub-slot of this frame
        // slot (the caller waited on the slot's fence before reusing it).
        unsafe {
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv));
            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
            // C6 depth 5: ER's scene depth (our copy of this frame) primed into our depth first (depth-only, no target)
            let mut rebind = false;
            if let (Some(pr), Some(pi)) = (self.prime.as_ref(), prime) {
                self.last_primed = pr.record(list, slot, dsv, pi);
                rebind = self.last_primed;
            }
            // C6 depth fallback: the body capsules into our depth only (depth-only, no render target), then the target again
            if let Some(o) = self.occl.as_ref()
                && !caps.is_empty()
            {
                caps_drawn = o.record(list, slot, dsv, &f.view_proj, caps);
                rebind |= caps_drawn > 0;
            }
            if rebind {
                list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv));
            }
            list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
        for (k, d) in draws.iter().take(MAX_WORLD_DRAWS).enumerate() {
            // the LOD asked for, else the nearest lower one on the GPU
            let Some(gpu_i) = (0..=d.lod as usize).rev().map(|l| d.gun * WORLD_LODS + l).find(|i| self.world.get(*i).is_some_and(|g| g.is_some())) else {
                continue;
            };
            if !d.model.iter().all(|x| x.is_finite()) {
                continue;
            }
            let mut fc = [0u32; FRAME_CB_BYTES / 4];
            let put = |fc: &mut [u32], at: usize, v: f32| fc[at] = v.to_bits();
            for r in 0..4 {
                for c in 0..4 {
                    put(&mut fc, r * 4 + c, f.view_proj[r][c]);
                    put(&mut fc, 16 + r * 4 + c, f.view[r][c]);
                }
            }
            let l = &f.light;
            let n = mw2er_core::xform::v_normalize(l.dir_view);
            for (i, v) in [n[0], n[1], n[2], l.diffuse, l.ambient_sky[0], l.ambient_sky[1], l.ambient_sky[2], l.exposure, l.ambient_ground[0], l.ambient_ground[1], l.ambient_ground[2]]
                .into_iter()
                .enumerate()
            {
                put(&mut fc, 32 + i, v);
            }
            fc[43] = f.out as u32;
            put(&mut fc, 44, f.paper_white_nits);
            fc[45] = DebugMode::None as u32;
            self.cutout.write_consts(&mut fc, t.width, t.height);
            let off = (slot * DRAWS_PER_FRAME + 1 + k) * SLOT_BYTES;
            // the rigid mesh reads bone 0 only (every vertex: joint 0, weight 255)
            let palette = [d.model; 4];
            // SAFETY: the ring holds SLOT_BYTES * num_frames * DRAWS_PER_FRAME bytes; off + FRAME_CB_BYTES + 4 x 48 is
            // inside sub-slot 1 + k < DRAWS_PER_FRAME of frame slot `slot` < num_frames.
            unsafe {
                let dst = self.ring_ptr.0.add(off);
                std::ptr::copy_nonoverlapping(fc.as_ptr() as *const u8, dst, FRAME_CB_BYTES);
                std::ptr::copy_nonoverlapping(palette.as_ptr() as *const u8, dst.add(FRAME_CB_BYTES), palette.len() * 48);
            }
            let Some(gpu) = self.world.get(gpu_i).and_then(|g| g.as_ref()) else { continue };
            let psos = &self.psos[pi];
            // SAFETY: as above.
            unsafe {
                list.IASetVertexBuffers(0, Some(&[gpu.vbv]));
                list.IASetIndexBuffer(Some(&gpu.ibv));
                list.SetGraphicsRootConstantBufferView(0, ring_va + off as u64);
                list.SetGraphicsRootConstantBufferView(1, ring_va + (off + FRAME_CB_BYTES) as u64);
                let mut cur_alpha = None;
                for di in &gpu.draws {
                    if cur_alpha != Some(di.alpha) {
                        list.SetPipelineState(if di.alpha { &psos.alpha } else { &psos.opaque });
                        cur_alpha = Some(di.alpha);
                    }
                    let h = D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu.ptr + di.srv as u64 * self.srv_inc as u64 };
                    list.SetGraphicsRootDescriptorTable(2, h);
                    let rc = [di.part, di.alpha as u32, 0, 0];
                    list.SetGraphicsRoot32BitConstants(3, 4, rc.as_ptr() as *const _, 0);
                    list.DrawIndexedInstanced(di.count, 1, di.first, 0, 0);
                }
            }
            drawn += 1;
        }
        Ok((drawn, caps_drawn))
    }
}

impl Renderer {
    /// M6: the scope overlay over the whole target (after the viewmodel; the viewmodel is not drawn while scoped).
    /// `opacity` fades it in with the ADS fraction. Creates its root signature / PSO on first use for a format (a
    /// one-off allocation reported through `take_present_allocs`).
    pub fn record_overlay(&mut self, list: &ID3D12GraphicsCommandList, t: &Target<'_>, opacity: f32) -> Result<(), VmError> {
        let srv = self.overlay_srv.ok_or(VmError::Invalid("overlay not uploaded".into()))?;
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
        }
        if self.overlay_rs.is_none() {
            self.overlay_rs = Some(create_overlay_root_signature(&self.device)?);
            self.present_allocs.push("overlay root signature".into());
        }
        let rs = self.overlay_rs.clone().ok_or(VmError::Invalid("overlay root signature".into()))?;
        let pso = match self.overlay_psos.iter().find(|(f, _)| *f == t.format) {
            Some((_, p)) => p.clone(),
            None => {
                let p = create_overlay_pso(&self.device, &rs, t.format)?;
                self.overlay_psos.push((t.format, p.clone()));
                self.present_allocs.push(format!("overlay PSO for {}", format_name(t.format)));
                p
            }
        };
        let consts = [(t.width as f32).to_bits(), (t.height as f32).to_bits(), opacity.clamp(0.0, 1.0).to_bits(), 0u32];
        // SAFETY: recording into the caller's open command list; every object referenced is owned by `self` (or the
        // caller's target) and outlives the GPU work.
        unsafe {
            let srv_gpu = self.srv_heap.GetGPUDescriptorHandleForHeapStart();
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, None);
            list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
            list.SetGraphicsRootSignature(&rs);
            list.SetPipelineState(&pso);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const _, 0);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu.ptr + srv as u64 * self.srv_inc as u64 });
            list.DrawInstanced(3, 1, 0, 0);
        }
        Ok(())
    }

    pub fn has_overlay(&self) -> bool {
        self.overlay_srv.is_some()
    }
}

fn create_overlay_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
    };
    serialize_root_signature(dev, &desc)
}

/// C4.1: the root signature of shaders/solid.hlsl: 4 root constants (the colour), nothing else.
pub fn create_solid_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let params = [D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    }];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
    };
    serialize_root_signature(dev, &desc)
}

/// C4.1: a PSO of shaders/solid.hlsl for `format` with the blend `rt` (no depth, no culling). The DLL builds the
/// canary with [`solid_write`] (the exact value, alpha included); vmtest builds its ER-like UI quads with the layer's
/// "over" blend ([`layer_blend`]`(mode, true)`).
pub fn create_solid_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT, rt: D3D12_RENDER_TARGET_BLEND_DESC) -> Result<ID3D12PipelineState, VmError> {
    let stencil_op = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_KEEP,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    };
    let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
    rtv_formats[0] = format;
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: weak(rs),
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: SOLID_VS.as_ptr() as *const _, BytecodeLength: SOLID_VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: SOLID_PS.as_ptr() as *const _, BytecodeLength: SOLID_PS.len() },
        BlendState: D3D12_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] },
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            FrontCounterClockwise: false.into(),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: true.into(),
            MultisampleEnable: false.into(),
            AntialiasedLineEnable: false.into(),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: false.into(),
            DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
            DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
            StencilEnable: false.into(),
            StencilReadMask: 0xFF,
            StencilWriteMask: 0xFF,
            FrontFace: stencil_op,
            BackFace: stencil_op,
        },
        InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: std::ptr::null(), NumElements: 0 },
        IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: rtv_formats,
        DSVFormat: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        NodeMask: 0,
        CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        ..Default::default()
    };
    // SAFETY: `desc` points at stack data and the embedded shader bytes, all alive during the call.
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState(solid)")
}

/// The plain write of every channel (the canary: its exact UNORM value, alpha included).
pub fn solid_write() -> D3D12_RENDER_TARGET_BLEND_DESC {
    D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    }
}

fn serialize_root_signature(dev: &ID3D12Device, desc: &D3D12_ROOT_SIGNATURE_DESC) -> Result<ID3D12RootSignature, VmError> {
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    // SAFETY: `desc` and everything it points to live on the caller's stack frame during the call.
    let r = unsafe { D3D12SerializeRootSignature(desc, D3D_ROOT_SIGNATURE_VERSION_1_0, &mut blob, Some(&mut err)) };
    if let Err(e) = r {
        let msg = err.map(|b| {
            // SAFETY: the error blob holds a text message of GetBufferSize bytes.
            let s = unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) };
            String::from_utf8_lossy(s).into_owned()
        });
        return Err(VmError::Invalid(format!("D3D12SerializeRootSignature 0x{:08x} {}", e.code().0 as u32, msg.unwrap_or_default())));
    }
    let blob = blob.ok_or(VmError::Invalid("root signature blob null".into()))?;
    // SAFETY: the blob's buffer is valid for GetBufferSize bytes while `blob` lives.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) };
    // SAFETY: serialized root signature bytes.
    unsafe { dev.CreateRootSignature(0, bytes) }.ctx("CreateRootSignature")
}

fn create_overlay_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, VmError> {
    create_overlay_pso_mode(dev, rs, format, EarlyMode::Scene)
}

/// The blend state of one early draw into a target (C4 4.1, C4.1 3.5 table); exactly the arithmetic of
/// `mw2er_core::c4::layer_write41` (tested against it). `alpha` = the alpha-blended draw (straight alpha, colour
/// SRC_ALPHA / INV_SRC_ALPHA), else the opaque draw (colour ONE / ZERO).
/// - `Scene`: RGB only, the target's alpha left alone (the M3 / M6 PSOs).
/// - `Premul` (empty a = 0): RGBA; opaque A = the shader's 1; alpha draws a = α + a_dst (1 - α) (C4's `ui_layer`).
/// - `Transmit` (empty a = 1): RGBA; opaque A = 0 (alpha factors ZERO / ZERO); alpha draws a = a_dst (1 - α).
pub fn layer_blend(mode: EarlyMode, alpha: bool) -> D3D12_RENDER_TARGET_BLEND_DESC {
    let (src, dst) = if alpha { (D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_INV_SRC_ALPHA) } else { (D3D12_BLEND_ONE, D3D12_BLEND_ZERO) };
    let (src_a, dst_a, enable) = match (mode, alpha) {
        // RGB only: the alpha factors never reach the target (write mask)
        (EarlyMode::Scene, true) => (D3D12_BLEND_ZERO, D3D12_BLEND_ONE, true),
        (EarlyMode::Scene, false) => (D3D12_BLEND_ONE, D3D12_BLEND_ZERO, false),
        (EarlyMode::Premul, true) => (D3D12_BLEND_ONE, D3D12_BLEND_INV_SRC_ALPHA, true),
        (EarlyMode::Premul, false) => (D3D12_BLEND_ONE, D3D12_BLEND_ZERO, false),
        (EarlyMode::Transmit, true) => (D3D12_BLEND_ZERO, D3D12_BLEND_INV_SRC_ALPHA, true),
        // the shader's opaque alpha is 1: the blend writes 0 (colour ONE / ZERO is the plain write)
        (EarlyMode::Transmit, false) => (D3D12_BLEND_ZERO, D3D12_BLEND_ZERO, true),
    };
    D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: enable.into(),
        LogicOpEnable: false.into(),
        SrcBlend: src,
        DestBlend: dst,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: src_a,
        DestBlendAlpha: dst_a,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: if mode == EarlyMode::Scene {
            (D3D12_COLOR_WRITE_ENABLE_RED.0 | D3D12_COLOR_WRITE_ENABLE_GREEN.0 | D3D12_COLOR_WRITE_ENABLE_BLUE.0) as u8
        } else {
            D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8
        },
    }
}

/// The overlay PSO (always the alpha draw: black with the mask's coverage) in `mode`. `Scene` is exactly the M6 PSO;
/// `Premul` is C4's `ui_layer` overlay ("over": a = α + a_dst (1 - α)); `Transmit` a = a_dst (1 - α).
fn create_overlay_pso_mode(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT, mode: EarlyMode) -> Result<ID3D12PipelineState, VmError> {
    let rt = layer_blend(mode, true);
    let stencil_op = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_KEEP,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    };
    let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
    rtv_formats[0] = format;
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: weak(rs),
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: OVERLAY_VS.as_ptr() as *const _, BytecodeLength: OVERLAY_VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: OVERLAY_PS.as_ptr() as *const _, BytecodeLength: OVERLAY_PS.len() },
        BlendState: D3D12_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] },
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            FrontCounterClockwise: false.into(),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: true.into(),
            MultisampleEnable: false.into(),
            AntialiasedLineEnable: false.into(),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: false.into(),
            DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
            DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
            StencilEnable: false.into(),
            StencilReadMask: 0xFF,
            StencilWriteMask: 0xFF,
            FrontFace: stencil_op,
            BackFace: stencil_op,
        },
        InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: std::ptr::null(), NumElements: 0 },
        IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: rtv_formats,
        DSVFormat: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        NodeMask: 0,
        CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        ..Default::default()
    };
    // SAFETY: `desc` points at stack data and the embedded shader bytes, all alive during the call.
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState(overlay)")
}

fn create_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let cbv = |reg: u32, vis| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: reg, RegisterSpace: 0 } },
        ShaderVisibility: vis,
    };
    let params = [
        cbv(0, D3D12_SHADER_VISIBILITY_ALL),
        cbv(1, D3D12_SHADER_VISIBILITY_VERTEX),
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 2, RegisterSpace: 0, Num32BitValues: 4 },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_ANISOTROPIC,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        MipLODBias: 0.0,
        MaxAnisotropy: 8,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    // SAFETY: `desc` and everything it points to live on this stack frame during the call.
    let r = unsafe { D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1_0, &mut blob, Some(&mut err)) };
    if let Err(e) = r {
        let msg = err.map(|b| {
            // SAFETY: the error blob holds a text message of GetBufferSize bytes.
            let s = unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) };
            String::from_utf8_lossy(s).into_owned()
        });
        return Err(VmError::Invalid(format!("D3D12SerializeRootSignature 0x{:08x} {}", e.code().0 as u32, msg.unwrap_or_default())));
    }
    let blob = blob.ok_or(VmError::Invalid("root signature blob null".into()))?;
    // SAFETY: the blob's buffer is valid for GetBufferSize bytes while `blob` lives.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) };
    // SAFETY: serialized root signature bytes.
    unsafe { dev.CreateRootSignature(0, bytes) }.ctx("CreateRootSignature")
}

fn create_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT, v: Variant, alpha: bool) -> Result<ID3D12PipelineState, VmError> {
    create_pso_mode(dev, rs, format, v, alpha, EarlyMode::Scene)
}

/// The viewmodel PSO with the blend of [`layer_blend`]. `Scene` is exactly the M3 PSO (RGB only, the host's alpha left
/// alone); `Premul` is C4's `ui_layer` (docs/C4_UI_DESIGN.md 4.1); `Transmit` is C4.1's other convention
/// (docs/C4_1_DESIGN.md 3.5): either way ER's composite treats the gun as opaque UI-layer coverage.
fn create_pso_mode(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT, v: Variant, alpha: bool, mode: EarlyMode) -> Result<ID3D12PipelineState, VmError> {
    let el = |name: windows::core::PCSTR, fmt: DXGI_FORMAT, off: u32| D3D12_INPUT_ELEMENT_DESC {
        SemanticName: name,
        SemanticIndex: 0,
        Format: fmt,
        InputSlot: 0,
        AlignedByteOffset: off,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    };
    let elements = [
        el(s!("POSITION"), DXGI_FORMAT_R32G32B32_FLOAT, 0),
        el(s!("NORMAL"), DXGI_FORMAT_R16G16B16A16_SNORM, 12),
        el(s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 20),
        el(s!("BLENDINDICES"), DXGI_FORMAT_R8G8B8A8_UINT, 28),
        el(s!("BLENDWEIGHT"), DXGI_FORMAT_R8G8B8A8_UNORM, 32),
        el(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 36),
    ];
    let rt = layer_blend(mode, alpha);
    let stencil_op = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_KEEP,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    };
    let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
    rtv_formats[0] = format;
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: weak(rs),
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: VS.as_ptr() as *const _, BytecodeLength: VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: PS.as_ptr() as *const _, BytecodeLength: PS.len() },
        BlendState: D3D12_BLEND_DESC {
            AlphaToCoverageEnable: false.into(),
            IndependentBlendEnable: false.into(),
            RenderTarget: [rt; 8],
        },
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: match v.cull {
                CullMode::Back => D3D12_CULL_MODE_BACK,
                CullMode::None => D3D12_CULL_MODE_NONE,
                CullMode::Front => D3D12_CULL_MODE_FRONT,
            },
            FrontCounterClockwise: v.front_ccw.into(),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: true.into(),
            MultisampleEnable: false.into(),
            AntialiasedLineEnable: false.into(),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: v.depth.into(),
            DepthWriteMask: if alpha || !v.depth { D3D12_DEPTH_WRITE_MASK_ZERO } else { D3D12_DEPTH_WRITE_MASK_ALL },
            DepthFunc: D3D12_COMPARISON_FUNC_GREATER,
            StencilEnable: false.into(),
            StencilReadMask: 0xFF,
            StencilWriteMask: 0xFF,
            FrontFace: stencil_op,
            BackFace: stencil_op,
        },
        InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: elements.as_ptr(), NumElements: elements.len() as u32 },
        IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: rtv_formats,
        DSVFormat: DXGI_FORMAT_D32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        NodeMask: 0,
        CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        ..Default::default()
    };
    // SAFETY: `desc` points at stack data and the embedded shader bytes, all alive during the call.
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arsenal_upload_error_device_fatal() {
        // review M6: the per-gun upload loop stops on these instead of trying (and leaking) 34 times
        assert!(VmError::Stuck("x".into()).is_device_fatal());
        assert!(VmError::D3d { call: "upload wait: device removed", hr: 0x8007_000Eu32 as i32 }.is_device_fatal());
        assert!(VmError::D3d { call: "CreateCommittedResource", hr: 0x887A_0005u32 as i32 }.is_device_fatal());
        assert!(!VmError::D3d { call: "CreateCommittedResource", hr: 0x8007_000Eu32 as i32 }.is_device_fatal());
        assert!(!VmError::Invalid("bad bundle".into()).is_device_fatal());
    }

    #[test]
    fn render_layout_helpers() {
        assert_eq!(align_up(6400, 256), 6400);
        assert_eq!(align_up(1, 256), 256);
        assert_eq!(SLOT_BYTES, 6400);
        assert_eq!(PALETTE_CB_BYTES, 6144);
        assert_eq!(SLOT_BYTES % 256, 0, "root CBV addresses must be 256-aligned");
        assert_eq!(FRAME_CB_BYTES % 256, 0);
    }

    #[test]
    fn render_bc_footprint_math() {
        assert_eq!(bc_mip_bytes(DXGI_BC1_UNORM, 1024, 1024), 524_288);
        assert_eq!(bc_mip_bytes(DXGI_BC3_UNORM, 1024, 1024), 1_048_576);
        assert_eq!(bc_mip_bytes(DXGI_BC1_UNORM, 1, 1), 8);
        assert_eq!(bc_mip_bytes(DXGI_BC3_UNORM, 2, 8), 32);
        // the full 1024^2 BC1 chain is 699064 bytes (= the DDS file minus its 128-byte header)
        let chain: u64 = (0..11).map(|m| bc_mip_bytes(DXGI_BC1_UNORM, 1024 >> m, 1024 >> m)).sum();
        assert_eq!(chain, 699_192 - 128);
    }

    #[test]
    fn render_output_mode_selection_table() {
        use VmOutput::*;
        assert_eq!(select_output(Auto, DXGI_FORMAT_R8G8B8A8_UNORM, -1), Ok(OutputMode::Srgb));
        assert_eq!(select_output(Auto, DXGI_FORMAT_B8G8R8A8_UNORM, 0), Ok(OutputMode::Srgb));
        assert_eq!(select_output(Auto, DXGI_FORMAT_R10G10B10A2_UNORM, -1), Ok(OutputMode::Srgb));
        assert_eq!(select_output(Auto, DXGI_FORMAT_R10G10B10A2_UNORM, 12), Ok(OutputMode::Pq));
        assert_eq!(select_output(Auto, DXGI_FORMAT_R16G16B16A16_FLOAT, 1), Ok(OutputMode::Scrgb));
        assert!(select_output(Auto, DXGI_FORMAT_R8G8B8A8_UNORM, 12).is_err(), "8-bit with PQ: refuse, don't guess");
        assert!(select_output(Auto, DXGI_FORMAT_R11G11B10_FLOAT, -1).is_err());
        assert_eq!(select_output(Srgb, DXGI_FORMAT_R11G11B10_FLOAT, 77), Ok(OutputMode::Srgb));
        assert_eq!(select_output(Pq, DXGI_FORMAT_R8G8B8A8_UNORM, 0), Ok(OutputMode::Pq));
        assert_eq!(select_output(Scrgb, DXGI_FORMAT_R8G8B8A8_UNORM, 0), Ok(OutputMode::Scrgb));
    }

    #[test]
    fn render_base_format_families() {
        assert_eq!(base_format(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), DXGI_FORMAT_R8G8B8A8_UNORM);
        assert_eq!(base_format(DXGI_FORMAT_B8G8R8A8_TYPELESS), DXGI_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(base_format(DXGI_FORMAT_R10G10B10A2_UNORM), DXGI_FORMAT_R10G10B10A2_UNORM);
        assert_ne!(base_format(DXGI_FORMAT_R8G8B8A8_UNORM), base_format(DXGI_FORMAT_R10G10B10A2_UNORM), "HDR toggle must differ");
        assert_ne!(base_format(DXGI_FORMAT_R8G8B8A8_UNORM), base_format(DXGI_FORMAT_R16G16B16A16_FLOAT));
    }

    #[test]
    fn c5_cutout_consts_packing() {
        let mut fc = [0xdead_beefu32; FRAME_CB_BYTES / 4];
        Cutout::NONE.write_consts(&mut fc, 3840, 2160);
        assert!(fc[46..64].iter().all(|&v| v == 0), "no cutout: zeros (the path of before)");
        let c = Cutout {
            rects: [[0.25, 0.5, 0.75, 1.0], [0.0, 0.0, 0.0, 0.1], [0.1, 0.1, 0.2, 0.2], [0.0; 4]],
            strength: [1.0, 1.0, 0.0, 0.0],
            n: 3,
            feather: 0.025,
        };
        c.write_consts(&mut fc, 3840, 2160);
        // the empty and the zero-strength regions are dropped
        assert_eq!(fc[46], 1);
        assert_eq!((fc[48], fc[49]), (960 | (1080 << 16), 2880 | (2160 << 16)));
        assert_eq!((f32::from_bits(fc[50]), f32::from_bits(fc[51])), (1.0, 54.0));
        assert!(fc[52..64].iter().all(|&v| v == 0));
        assert_eq!(fc[0], 0xdead_beef, "only the cutout words are written");
    }

    #[test]
    fn render_shaders_embedded() {
        assert!(VS.starts_with(b"DXBC") && PS.starts_with(b"DXBC"));
        assert_eq!(mw2er_core::sha256::sha256_hex(VS), VS_SHA256);
        assert_eq!(mw2er_core::sha256::sha256_hex(PS), PS_SHA256);
        assert!(SOLID_VS.starts_with(b"DXBC") && SOLID_PS.starts_with(b"DXBC"));
        assert_eq!(mw2er_core::sha256::sha256_hex(SOLID_VS), SOLID_VS_SHA256);
        assert_eq!(mw2er_core::sha256::sha256_hex(SOLID_PS), SOLID_PS_SHA256);
    }

    /// One D3D12 blend factor applied to (src rgba, dst rgba) for channel `k` (the alpha factors use alpha).
    fn factor(f: D3D12_BLEND, src: [f32; 4], dst: [f32; 4]) -> f32 {
        match f {
            D3D12_BLEND_ZERO => 0.0,
            D3D12_BLEND_ONE => 1.0,
            D3D12_BLEND_SRC_ALPHA => src[3],
            D3D12_BLEND_INV_SRC_ALPHA => 1.0 - src[3],
            D3D12_BLEND_DEST_ALPHA => dst[3],
            D3D12_BLEND_INV_DEST_ALPHA => 1.0 - dst[3],
            _ => panic!("factor {f:?} not modelled"),
        }
    }

    /// The output-merger result of one blend desc (ADD ops only; the write mask keeps unwritten channels).
    fn om(rt: &D3D12_RENDER_TARGET_BLEND_DESC, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
        assert!(rt.BlendOp == D3D12_BLEND_OP_ADD && rt.BlendOpAlpha == D3D12_BLEND_OP_ADD);
        let mut o = if rt.BlendEnable.as_bool() {
            let c = |k: usize| src[k] * factor(rt.SrcBlend, src, dst) + dst[k] * factor(rt.DestBlend, src, dst);
            [c(0), c(1), c(2), src[3] * factor(rt.SrcBlendAlpha, src, dst) + dst[3] * factor(rt.DestBlendAlpha, src, dst)]
        } else {
            src
        };
        for (k, bit) in [D3D12_COLOR_WRITE_ENABLE_RED, D3D12_COLOR_WRITE_ENABLE_GREEN, D3D12_COLOR_WRITE_ENABLE_BLUE, D3D12_COLOR_WRITE_ENABLE_ALPHA].iter().enumerate() {
            if rt.RenderTargetWriteMask & bit.0 as u8 == 0 {
                o[k] = dst[k];
            }
        }
        o
    }

    #[test]
    fn c4_alpha_variants_blend_states() {
        use mw2er_core::c4::{LayerAlpha, empty_px41, layer_write41};
        // the PSO blend states compute exactly the core's pixel math (which the composite tests prove right)
        let gun = [0.9, 0.5, 0.1];
        let dsts = [[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 0.0], [0.3, 0.2, 0.1, 0.6], [0.7, 0.7, 0.7, 0.25]];
        for mode in [EarlyMode::Scene, EarlyMode::Premul, EarlyMode::Transmit] {
            for alpha in [false, true] {
                let rt = layer_blend(mode, alpha);
                for dst in dsts {
                    for a in [1.0f32, 0.75, 0.25, 0.0] {
                        // the shader outputs (colour, 1) for opaque draws and (colour, α) for alpha draws
                        let src = [gun[0], gun[1], gun[2], if alpha { a } else { 1.0 }];
                        let got = om(&rt, src, dst);
                        let want = layer_write41(mode, dst, gun, if alpha { a } else { 1.0 }, !alpha);
                        assert!((0..4).all(|k| (got[k] - want[k]).abs() < 1e-6), "{mode:?} alpha={alpha} dst={dst:?} a={a}: {got:?} != {want:?}");
                    }
                }
            }
        }
        // the C4 / M3 / M6 PSOs are unchanged: Scene = RGB only, Premul = C4's `ui_layer`
        let s = layer_blend(EarlyMode::Scene, false);
        assert!(!s.BlendEnable.as_bool() && s.RenderTargetWriteMask == 7);
        let p = layer_blend(EarlyMode::Premul, true);
        assert!(p.SrcBlendAlpha == D3D12_BLEND_ONE && p.DestBlendAlpha == D3D12_BLEND_INV_SRC_ALPHA && p.RenderTargetWriteMask == 15);
        // the canary's plain write keeps its exact value, alpha included, on any layer
        for conv in [LayerAlpha::Transmit, LayerAlpha::Premul] {
            let c = mw2er_core::c4::canary_color41(conv);
            assert_eq!(om(&solid_write(), c, empty_px41(conv)), c);
        }
    }
}
