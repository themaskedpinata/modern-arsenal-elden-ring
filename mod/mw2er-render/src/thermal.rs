//! The M4 thermal pass (docs/M4_DESIGN.md section 6), shared by `mw2er.dll` (hudhook's pre-ImGui callback) and
//! `vmtest.exe` (cases T1-T9 and the thermal perf): same code, same shader bytes.
//!
//! Record order (section 6.2), with NOTHING fallible between the two barrier calls:
//! 1. validate everything (size, live format == RTV format family, overlays uploaded, PSO, scene copy, a free scene
//!    descriptor slot); any failure returns `Err` before a single command is recorded;
//! 2. `[BB RENDER_TARGET -> COPY_SOURCE, scene PIXEL_SHADER_RESOURCE -> COPY_DEST]`, `CopyResource(scene, BB)`,
//!    `[BB COPY_SOURCE -> RENDER_TARGET, scene COPY_DEST -> PIXEL_SHADER_RESOURCE]` (balanced);
//! 3. full state (viewport, scissor, RTV without DSV, heap, root signature, PSO, constants, 3 tables) and one
//!    `DrawInstanced(3, 1, 0, 0)`; optional timestamps around 2-3.
//!
//! The back buffer leaves in RENDER_TARGET (what hudhook's next barrier expects) and no reference to it is kept.
//! A size change creates a new scene copy whose SRV goes into a scene descriptor slot that no list of the last
//! `num_frames + 1` records used (a ring of `num_frames + 1` slots: a shader-visible descriptor is read when the GPU
//! executes, so a slot an in-flight list uses is never rewritten); the old texture is retired for `num_frames + 1`
//! records.

use mw2er_core::thermal::Film;
use mw2er_core::vm_assets::{DXGI_BC3_UNORM, Texture};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::{Ctx, InFlight, Settled, UPLOAD_MAX_WAIT_MS, VmError, align_up, base_format, create_buffer, format_name, heap_props, transition, wait_submitted, weak};

/// Thermal vertex shader (DXBC, vs_5_1) compiled by build.rs from shaders/thermal.hlsl.
pub const THERMAL_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/thermal_vs.cso"));
/// Thermal pixel shader (DXBC, ps_5_1).
pub const THERMAL_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/thermal_ps.cso"));
pub const THERMAL_VS_SHA256: &str = env!("MW2ER_THERMAL_VS_SHA256");
pub const THERMAL_PS_SHA256: &str = env!("MW2ER_THERMAL_PS_SHA256");

/// Overlay textures: 105, 40, 25, grain.
pub const OVERLAY_COUNT: usize = 4;
/// c0..c7 (M4) + c8 (run 8: x = mid-tone gamma).
pub const ROOT_CONSTANTS: u32 = 36;
/// Errors starting with this are transient (the caller skips the frame instead of disabling the pass).
pub const BUSY_PREFIX: &str = "busy:";

/// The format family of a back buffer the pass can read: (typeless scene format, SRV format).
pub fn scene_formats(live: DXGI_FORMAT) -> Option<(DXGI_FORMAT, DXGI_FORMAT)> {
    match base_format(live) {
        DXGI_FORMAT_R8G8B8A8_UNORM => Some((DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM)),
        DXGI_FORMAT_B8G8R8A8_UNORM => Some((DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM)),
        DXGI_FORMAT_R10G10B10A2_UNORM => Some((DXGI_FORMAT_R10G10B10A2_TYPELESS, DXGI_FORMAT_R10G10B10A2_UNORM)),
        _ => None,
    }
}

/// The caller's back buffer (RENDER_TARGET state).
pub struct ThermalTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub rtv_format: DXGI_FORMAT,
    pub width: u32,
    pub height: u32,
}

/// Per-frame parameters (all go into root constants).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermalParams {
    pub film: Film,
    pub luma: [f32; 3],
    /// Mid-tone lift `g^gamma` of the (exposed) film input (run 8; 1 = MW2 as is).
    pub gamma: f32,
    pub fade: f32,
    /// The 4:3 box `[x0, y0, w, h]` in pixels.
    pub box_px: [f32; 4],
    /// 0 = 105, 1 = 40, 2 = 25; `None` = no overlay.
    pub overlay: Option<usize>,
    pub ov_alpha: f32,
    pub grain: bool,
    pub grain_offset: [f32; 2],
    pub grain_scale: [f32; 2],
    pub grain_tint: [f32; 3],
    pub grain_alpha: f32,
    pub timing: bool,
    /// vmtest T8 only: skip the back buffer's COPY_SOURCE -> RENDER_TARGET barrier (proves the state check bites).
    #[doc(hidden)]
    pub test_unbalanced: bool,
}

impl ThermalParams {
    /// The 36 root constants (shader layout c0..c8).
    pub fn constants(&self, w: u32, h: u32) -> [f32; 36] {
        let f = &self.film;
        let mut c = [0.0f32; 36];
        c[0..3].copy_from_slice(&f.base);
        c[3] = f.offset;
        c[4..7].copy_from_slice(&f.delta);
        c[7] = f.desat;
        c[8..11].copy_from_slice(&f.quad);
        c[11] = self.fade;
        c[12..15].copy_from_slice(&self.luma);
        c[15] = self.ov_alpha;
        c[16..20].copy_from_slice(&self.box_px);
        c[20..22].copy_from_slice(&self.grain_offset);
        c[22..24].copy_from_slice(&self.grain_scale);
        c[24..27].copy_from_slice(&self.grain_tint);
        c[27] = self.grain_alpha;
        c[28] = if self.overlay.is_some() { 1.0 } else { 0.0 };
        c[29] = if self.grain { 1.0 } else { 0.0 };
        c[30] = w as f32;
        c[31] = h as f32;
        c[32] = self.gamma;
        c
    }
}

struct Scene {
    res: ID3D12Resource,
    w: u32,
    h: u32,
    family: DXGI_FORMAT,
    slot: usize,
}

pub struct ThermalPass {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    heap: ID3D12DescriptorHeap,
    inc: u32,
    scene: Option<Scene>,
    /// Record count that last used each scene descriptor slot.
    slot_used: Vec<u64>,
    retired: Vec<(u64, ID3D12Resource)>,
    frames: u64,
    num_frames: usize,
    overlays: Vec<ID3D12Resource>,
    queries: ID3D12QueryHeap,
    readback: ID3D12Resource,
    slot_written: Vec<bool>,
    ts_freq: u64,
    samples: Vec<f32>,
    present_allocs: Vec<String>,
}

fn create_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let ranges = [0u32, 1, 2].map(|reg| D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: reg,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    });
    let table = |i: usize| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &ranges[i] } },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: ROOT_CONSTANTS } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        table(0),
        table(1),
        table(2),
    ];
    let sampler = |reg: u32, mode: D3D12_TEXTURE_ADDRESS_MODE| D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: mode,
        AddressV: mode,
        AddressW: mode,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: reg,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let samplers = [sampler(0, D3D12_TEXTURE_ADDRESS_MODE_CLAMP), sampler(1, D3D12_TEXTURE_ADDRESS_MODE_WRAP)];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
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
        return Err(VmError::Invalid(format!("thermal D3D12SerializeRootSignature 0x{:08x} {}", e.code().0 as u32, msg.unwrap_or_default())));
    }
    let blob = blob.ok_or(VmError::Invalid("thermal root signature blob null".into()))?;
    // SAFETY: the blob's buffer is valid for GetBufferSize bytes while `blob` lives.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) };
    // SAFETY: serialized root signature bytes.
    unsafe { dev.CreateRootSignature(0, bytes) }.ctx("CreateRootSignature(thermal)")
}

fn create_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, VmError> {
    let rt = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: false.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        // RGB only: the host's back-buffer alpha is left alone.
        RenderTargetWriteMask: (D3D12_COLOR_WRITE_ENABLE_RED.0 | D3D12_COLOR_WRITE_ENABLE_GREEN.0 | D3D12_COLOR_WRITE_ENABLE_BLUE.0) as u8,
    };
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
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: THERMAL_VS.as_ptr() as *const _, BytecodeLength: THERMAL_VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: THERMAL_PS.as_ptr() as *const _, BytecodeLength: THERMAL_PS.len() },
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
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState(thermal)")
}

impl ThermalPass {
    /// Root signature, the shader-visible heap (`num_frames + 1` scene slots + 4 textures) and the timestamp heap.
    pub fn new(device: &ID3D12Device, num_frames: usize, ts_freq: u64) -> Result<Self, VmError> {
        if !(1..=8).contains(&num_frames) {
            return Err(VmError::Invalid(format!("num_frames {num_frames}")));
        }
        let root_sig = create_root_signature(device)?;
        let scene_slots = num_frames + 1;
        // SAFETY: valid descriptor-heap description.
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: (scene_slots + OVERLAY_COUNT) as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(thermal)")?;
        // SAFETY: plain query.
        let inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) };
        let mut queries: Option<ID3D12QueryHeap> = None;
        // SAFETY: valid description; out pointer to a local Option.
        unsafe {
            device.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: (2 * num_frames) as u32, NodeMask: 0 }, &mut queries)
        }
        .ctx("CreateQueryHeap(thermal)")?;
        let queries = queries.ok_or(VmError::Invalid("CreateQueryHeap returned null".into()))?;
        let readback = create_buffer(device, D3D12_HEAP_TYPE_READBACK, (16 * num_frames) as u64, D3D12_RESOURCE_STATE_COPY_DEST)?;
        Ok(ThermalPass {
            device: device.clone(),
            root_sig,
            psos: Vec::new(),
            heap,
            inc,
            scene: None,
            slot_used: vec![0; scene_slots],
            retired: Vec::new(),
            frames: 0,
            num_frames,
            overlays: Vec::new(),
            queries,
            readback,
            slot_written: vec![false; num_frames],
            ts_freq,
            samples: Vec::with_capacity(1024),
            present_allocs: Vec::new(),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.overlays.len() == OVERLAY_COUNT
    }

    fn scene_slots(&self) -> usize {
        self.num_frames + 1
    }

    fn cpu_handle(&self, i: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query.
        let h0 = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + i * self.inc as usize }
    }

    fn gpu_handle(&self, i: usize) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query.
        let h0 = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + i as u64 * self.inc as u64 }
    }

    fn pso_index(&mut self, format: DXGI_FORMAT) -> Result<usize, VmError> {
        if let Some(i) = self.psos.iter().position(|(f, _)| *f == format) {
            return Ok(i);
        }
        let p = create_pso(&self.device, &self.root_sig, format)?;
        self.psos.push((format, p));
        Ok(self.psos.len() - 1)
    }

    /// PSO + scene copy for the swap chain's format and size, ahead of the first frame (loader thread).
    pub fn prepare(&mut self, rtv_format: DXGI_FORMAT, width: u32, height: u32) -> Result<(), VmError> {
        self.pso_index(rtv_format)?;
        if let Some((family, srv)) = scene_formats(rtv_format)
            && width > 0
            && height > 0
            && width <= 16384
            && height <= 16384
        {
            self.ensure_scene(width, height, family, srv)?;
        }
        Ok(())
    }

    /// What `record` had to create on the present thread since the last call (empty normally).
    pub fn take_present_allocs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.present_allocs)
    }

    /// A scene slot no list of the last `num_frames + 1` records used (`None` = all busy).
    fn free_slot(&self, avoid: Option<usize>) -> Option<usize> {
        let keep = self.num_frames as u64 + 1;
        (0..self.scene_slots()).find(|&s| Some(s) != avoid && (self.slot_used[s] == 0 || self.slot_used[s] + keep <= self.frames))
    }

    fn ensure_scene(&mut self, w: u32, h: u32, family: DXGI_FORMAT, srv: DXGI_FORMAT) -> Result<bool, VmError> {
        if self.scene.as_ref().is_some_and(|s| s.w == w && s.h == h && s.family == family) {
            return Ok(false);
        }
        let slot = self.free_slot(self.scene.as_ref().map(|s| s.slot)).ok_or(VmError::Invalid(format!("{BUSY_PREFIX} no free scene descriptor slot (resized {} times within {} records); this frame is skipped", self.scene_slots(), self.num_frames + 1)))?;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: family,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut r: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; out pointer to a local Option.
        unsafe {
            self.device.CreateCommittedResource(
                &heap_props(D3D12_HEAP_TYPE_DEFAULT),
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                None,
                &mut r,
            )
        }
        .ctx("CreateCommittedResource(thermal scene)")?;
        let res = r.ok_or(VmError::Invalid("thermal scene null".into()))?;
        let sd = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: srv,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        // SAFETY: `slot` is a scene slot no list of the last num_frames + 1 records used (so no in-flight list reads
        // it); the view describes the texture just created.
        unsafe { self.device.CreateShaderResourceView(&res, Some(&sd), self.cpu_handle(slot)) };
        if let Some(old) = self.scene.replace(Scene { res, w, h, family, slot }) {
            self.retired.push((self.frames, old.res));
        }
        Ok(true)
    }

    /// Uploads the 4 overlay textures (BC3, viewed as UNORM: stored values) on its own queue and waits.
    /// Loader thread / vmtest only. Objects the GPU may still use are leaked rather than freed on a timeout.
    pub fn upload_overlays(&mut self, texs: &[Texture]) -> Result<u64, VmError> {
        if texs.len() != OVERLAY_COUNT {
            return Err(VmError::Invalid(format!("{} overlay textures (want {OVERLAY_COUNT})", texs.len())));
        }
        let dev = self.device.clone();
        let mut res = Vec::new();
        let mut layouts = Vec::new();
        let mut offset = 0u64;
        for t in texs {
            if t.format != DXGI_BC3_UNORM {
                return Err(VmError::Invalid(format!("overlay format {} (want BC3)", t.format)));
            }
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: t.width as u64,
                Height: t.height,
                DepthOrArraySize: 1,
                MipLevels: t.mips.len() as u16,
                Format: DXGI_FORMAT_BC3_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let mut r: Option<ID3D12Resource> = None;
            // SAFETY: valid descriptions; out pointer to a local Option.
            unsafe { dev.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut r) }
                .ctx("CreateCommittedResource(overlay)")?;
            let n = t.mips.len();
            let mut lay = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
            let mut rows = vec![0u32; n];
            let mut row_bytes = vec![0u64; n];
            let mut total = 0u64;
            // SAFETY: the output arrays have `n` entries each.
            unsafe {
                dev.GetCopyableFootprints(&desc, 0, n as u32, offset, Some(lay.as_mut_ptr()), Some(rows.as_mut_ptr()), Some(row_bytes.as_mut_ptr()), Some(&mut total))
            };
            for (m, mip) in t.mips.iter().enumerate() {
                if row_bytes[m] != mip.row_pitch as u64 || rows[m] != mip.rows {
                    return Err(VmError::Invalid(format!("overlay mip {m}: footprint {}x{} != {}x{}", row_bytes[m], rows[m], mip.row_pitch, mip.rows)));
                }
            }
            offset = align_up((offset + total) as usize, 512) as u64;
            res.push(r.ok_or(VmError::Invalid("overlay null".into()))?);
            layouts.push(lay);
        }
        let staging = create_buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, offset, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: mapping an UPLOAD buffer for writing.
        unsafe { staging.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(overlay staging)")?;
        let base = p as *mut u8;
        // SAFETY: every row write stays inside the footprints GetCopyableFootprints computed for this buffer.
        unsafe {
            for (t, lay) in texs.iter().zip(&layouts) {
                for (mip, l) in t.mips.iter().zip(lay) {
                    for r in 0..mip.rows as usize {
                        let src = &mip.data[r * mip.row_pitch as usize..(r + 1) * mip.row_pitch as usize];
                        std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(l.Offset as usize + r * l.Footprint.RowPitch as usize), src.len());
                    }
                }
            }
            staging.Unmap(0, None);
        }
        // SAFETY: plain COM object creation.
        let queue: ID3D12CommandQueue = unsafe {
            dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, Priority: 0, Flags: D3D12_COMMAND_QUEUE_FLAG_NONE, NodeMask: 0 })
        }
        .ctx("CreateCommandQueue(overlay)")?;
        // SAFETY: as above.
        let alloc: ID3D12CommandAllocator = unsafe { dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ctx("CreateCommandAllocator(overlay)")?;
        // SAFETY: as above.
        let list: ID3D12GraphicsCommandList =
            unsafe { dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None::<&ID3D12PipelineState>) }.ctx("CreateCommandList(overlay)")?;
        // SAFETY: plain COM object creation (before anything is submitted).
        let fence: ID3D12Fence = unsafe { dev.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ctx("CreateFence(overlay)")?;
        // SAFETY: recording into our own fresh list; every resource lives until the wait below.
        unsafe {
            let mut barriers = Vec::new();
            for (t, lay) in res.iter().zip(&layouts) {
                for (m, l) in lay.iter().enumerate() {
                    let dst = D3D12_TEXTURE_COPY_LOCATION { pResource: weak(t), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 } };
                    let src = D3D12_TEXTURE_COPY_LOCATION { pResource: weak(&staging), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *l } };
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                }
                barriers.push(transition(t, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE));
            }
            list.ResourceBarrier(&barriers);
            list.Close().ctx("Close(overlay)")?;
        }
        let cmd: ID3D12CommandList = list.cast().ctx("cast(overlay list)")?;
        // SAFETY: submitting our own closed list; from here on the objects below are in flight.
        unsafe { queue.ExecuteCommandLists(&[Some(cmd)]) };
        // SAFETY: signalling our own queue.
        let signalled = unsafe { queue.Signal(&fence, 1) }.ctx("Signal(overlay)");
        let inflight = InFlight { _queue: queue, _alloc: alloc, _list: list, _fence: fence.clone(), _staging: staging, _dst: res.clone() };
        if let Err(e) = signalled {
            std::mem::forget(res);
            inflight.leak();
            return Err(e);
        }
        match wait_submitted(&dev, &fence, 1, UPLOAD_MAX_WAIT_MS) {
            Settled::Done => drop(inflight),
            Settled::DeviceRemoved(hr) => {
                std::mem::forget(res);
                inflight.leak();
                return Err(VmError::D3d { call: "overlay upload wait: device removed", hr });
            }
            Settled::StillRunning => {
                std::mem::forget(res);
                inflight.leak();
                return Err(VmError::Invalid("overlay upload not finished (objects leaked, not freed)".into()));
            }
        }
        for (k, (t, r)) in texs.iter().zip(&res).enumerate() {
            let sd = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_BC3_UNORM,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: t.mips.len() as u32, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
            };
            // SAFETY: the texture slots come after the scene ring; the views are written once, before any record.
            unsafe { dev.CreateShaderResourceView(r, Some(&sd), self.cpu_handle(self.scene_slots() + k)) };
        }
        self.overlays = res;
        Ok(offset)
    }

    fn collect_timing(&mut self, slot: usize) {
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
            if ms.is_finite() && ms < 1000.0 && self.samples.len() < 100_000 {
                self.samples.push(ms as f32);
            }
        }
    }

    /// Collects the timestamps of a slot whose fence the caller waited on (vmtest; the DLL relies on `record`).
    pub fn collect(&mut self, slot: usize) {
        self.collect_timing(slot);
    }

    /// Drains the collected GPU-time samples (ms).
    pub fn take_timings(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    /// Records the pass into `list` (target in RENDER_TARGET state). Every check runs before the first command; on
    /// `Err` nothing was recorded.
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, t: &ThermalTarget<'_>, slot: usize, p: &ThermalParams) -> Result<(), VmError> {
        // ---- 1. validate (no command recorded yet) ----
        if slot >= self.num_frames {
            return Err(VmError::Invalid(format!("slot {slot} >= {}", self.num_frames)));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
        }
        if !self.is_ready() {
            return Err(VmError::Invalid("overlays not uploaded".into()));
        }
        // SAFETY: plain query on the caller's live back buffer.
        let live = unsafe { t.resource.GetDesc() };
        if live.Width != t.width as u64 || live.Height != t.height || live.SampleDesc.Count != 1 {
            return Err(VmError::Invalid(format!("back buffer {}x{} ms{} != target {}x{}", live.Width, live.Height, live.SampleDesc.Count, t.width, t.height)));
        }
        if base_format(live.Format) != base_format(t.rtv_format) {
            return Err(VmError::Invalid(format!("back-buffer format {} != RTV {}", format_name(live.Format), format_name(t.rtv_format))));
        }
        let (family, srv) = scene_formats(live.Format).ok_or_else(|| VmError::Invalid(format!("unsupported back-buffer format {} (SDR 8/10-bit only)", format_name(live.Format))))?;
        if p.overlay.is_some_and(|o| o >= 3) {
            return Err(VmError::Invalid("overlay index".into()));
        }
        let n_psos = self.psos.len();
        let pi = self.pso_index(t.rtv_format)?;
        if self.psos.len() != n_psos {
            self.present_allocs.push(format!("thermal PSO for {}", format_name(t.rtv_format)));
        }
        if self.ensure_scene(t.width, t.height, family, srv)? {
            self.present_allocs.push(format!("thermal scene {}x{}", t.width, t.height));
        }
        let scene_slot = self.scene.as_ref().map(|s| s.slot).ok_or(VmError::Invalid("no scene copy".into()))?;
        let scene_res = self.scene.as_ref().map(|s| s.res.clone()).ok_or(VmError::Invalid("no scene copy".into()))?;
        self.frames += 1;
        let frames = self.frames;
        let keep = self.num_frames as u64 + 1;
        self.retired.retain(|(f, _)| frames < f + keep);
        self.slot_used[scene_slot] = frames;
        self.collect_timing(slot);
        let consts = p.constants(t.width, t.height);
        let ov = self.scene_slots() + p.overlay.unwrap_or(0);
        let grain = self.scene_slots() + 3;
        let (h_scene, h_ov, h_grain) = (self.gpu_handle(scene_slot), self.gpu_handle(ov), self.gpu_handle(grain));
        let timing = p.timing && self.ts_freq > 0;
        let pso = &self.psos[pi].1;
        // vmtest T8 only: leave the BB restore out so the debug layer must flag hudhook's next barrier
        let post_n = if p.test_unbalanced { 1 } else { 2 };
        let pre = [
            transition(t.resource, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE),
            transition(&scene_res, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST),
        ];
        let post = [
            transition(&scene_res, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
            transition(t.resource, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
        ];
        // ---- 2. + 3. record: nothing fallible from here on ----
        // SAFETY: recording into the caller's open list; every object referenced is owned by `self` (retired scene
        // copies are kept num_frames + 1 records) or is the caller's back buffer, which outlives the GPU work.
        unsafe {
            if timing {
                list.EndQuery(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot) as u32);
            }
            list.ResourceBarrier(&pre);
            list.CopyResource(&scene_res, t.resource);
            list.ResourceBarrier(&post[..post_n]);
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, None);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.SetPipelineState(pso);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, ROOT_CONSTANTS, consts.as_ptr() as *const _, 0);
            list.SetGraphicsRootDescriptorTable(1, h_scene);
            list.SetGraphicsRootDescriptorTable(2, h_ov);
            list.SetGraphicsRootDescriptorTable(3, h_grain);
            list.DrawInstanced(3, 1, 0, 0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_thermal_shaders_embedded() {
        assert!(THERMAL_VS.starts_with(b"DXBC") && THERMAL_PS.starts_with(b"DXBC"));
        assert_eq!(mw2er_core::sha256::sha256_hex(THERMAL_VS), THERMAL_VS_SHA256);
        assert_eq!(mw2er_core::sha256::sha256_hex(THERMAL_PS), THERMAL_PS_SHA256);
    }

    #[test]
    fn render_thermal_constants_layout() {
        let p = ThermalParams {
            film: Film { base: [1.0, 2.0, 3.0], offset: 4.0, delta: [5.0, 6.0, 7.0], desat: 8.0, quad: [9.0, 10.0, 11.0] },
            luma: [0.1, 0.2, 0.3],
            gamma: 0.8,
            fade: 12.0,
            box_px: [480.0, 0.0, 2880.0, 2160.0],
            overlay: Some(1),
            ov_alpha: 0.5,
            grain: true,
            grain_offset: [0.25, 0.5],
            grain_scale: [1.0, 1.0],
            grain_tint: [0.9, 1.0, 1.0],
            grain_alpha: 0.3,
            timing: false,
            test_unbalanced: false,
        };
        let c = p.constants(3840, 2160);
        assert_eq!(&c[0..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(c[11], 12.0);
        assert_eq!(c[15], 0.5);
        assert_eq!(&c[16..20], &[480.0, 0.0, 2880.0, 2160.0]);
        assert_eq!((c[28], c[29], c[30], c[31], c[32]), (1.0, 1.0, 3840.0, 2160.0, 0.8));
        assert_eq!(scene_formats(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), Some((DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM)));
        assert_eq!(scene_formats(DXGI_FORMAT_R16G16B16A16_FLOAT), None, "scRGB: thermal off");
    }
}
