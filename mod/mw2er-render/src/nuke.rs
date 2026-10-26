//! The M7 nuke post pass (docs/M7_DESIGN.md 3.7), shared by `mw2er.dll` (hudhook's pre-ImGui callback) and
//! `vmtest.exe` (cases N1-N6 and the 4K perf): same code, same shader bytes. Built on the thermal pass's plumbing
//! (scene copy ring, shader-visible heap, balanced barriers, timestamps).
//!
//! Record order, with NOTHING fallible between the first and the last barrier:
//! 1. validate everything (size, live format == RTV family, PSOs, the slot's scene copy + bloom texture); any failure
//!    returns `Err` before a single command is recorded;
//! 2. `[BB RENDER_TARGET -> COPY_SOURCE, scene PIXEL_SHADER_RESOURCE -> COPY_DEST]`, `CopyResource(scene, BB)`,
//!    `[BB -> RENDER_TARGET, scene -> PIXEL_SHADER_RESOURCE, bloom PIXEL_SHADER_RESOURCE -> RENDER_TARGET]`;
//! 3. the bloom draw into the 1/8 texture, `[bloom RENDER_TARGET -> PIXEL_SHADER_RESOURCE]`;
//! 4. the composite draw into the back buffer (RGB only). Optional timestamps around 2-4.
//!
//! The back buffer leaves in RENDER_TARGET. A size change creates a new scene copy + bloom texture in a slot that no
//! list of the last `num_frames + 1` records used; the old pair is retired for `num_frames + 1` records.

use mw2er_core::nuke_fx::{NUKE_CONSTANTS, bloom_size};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::thermal::{BUSY_PREFIX, scene_formats};
use crate::{Ctx, VmError, base_format, create_buffer, format_name, heap_props, serialize_root_signature, transition, weak};

pub const NUKE_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nuke_vs.cso"));
pub const NUKE_BLOOM_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nuke_bloom_ps.cso"));
pub const NUKE_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nuke_ps.cso"));
pub const NUKE_VS_SHA256: &str = env!("MW2ER_NUKE_VS_SHA256");
pub const NUKE_BLOOM_PS_SHA256: &str = env!("MW2ER_NUKE_BLOOM_PS_SHA256");
pub const NUKE_PS_SHA256: &str = env!("MW2ER_NUKE_PS_SHA256");
/// The bloom texture format (HDR headroom for the bright pass).
pub const BLOOM_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R16G16B16A16_FLOAT;

/// The caller's back buffer (RENDER_TARGET state).
pub struct NukeTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub rtv_format: DXGI_FORMAT,
    pub width: u32,
    pub height: u32,
}

struct Pair {
    scene: ID3D12Resource,
    bloom: ID3D12Resource,
    w: u32,
    h: u32,
    family: DXGI_FORMAT,
    slot: usize,
}

pub struct NukePass {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    /// (back-buffer format, composite PSO)
    psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    bloom_pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    rtv_heap: ID3D12DescriptorHeap,
    inc: u32,
    rtv_inc: u32,
    pair: Option<Pair>,
    slot_used: Vec<u64>,
    retired: Vec<(u64, ID3D12Resource, ID3D12Resource)>,
    frames: u64,
    num_frames: usize,
    queries: ID3D12QueryHeap,
    readback: ID3D12Resource,
    slot_written: Vec<bool>,
    ts_freq: u64,
    samples: Vec<f32>,
    present_allocs: Vec<String>,
}

fn create_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 2,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: NUKE_CONSTANTS as u32 } },
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
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
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

fn create_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT, ps: &[u8], rgb_only: bool) -> Result<ID3D12PipelineState, VmError> {
    let mask = if rgb_only {
        (D3D12_COLOR_WRITE_ENABLE_RED.0 | D3D12_COLOR_WRITE_ENABLE_GREEN.0 | D3D12_COLOR_WRITE_ENABLE_BLUE.0) as u8
    } else {
        D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8
    };
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
        RenderTargetWriteMask: mask,
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
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: NUKE_VS.as_ptr() as *const _, BytecodeLength: NUKE_VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr() as *const _, BytecodeLength: ps.len() },
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
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState(nuke)")
}

impl NukePass {
    pub fn new(device: &ID3D12Device, num_frames: usize, ts_freq: u64) -> Result<Self, VmError> {
        if !(1..=8).contains(&num_frames) {
            return Err(VmError::Invalid(format!("num_frames {num_frames}")));
        }
        let root_sig = create_root_signature(device)?;
        let bloom_pso = create_pso(device, &root_sig, BLOOM_FORMAT, NUKE_BLOOM_PS, false)?;
        let slots = num_frames + 1;
        // SAFETY: valid descriptor-heap descriptions.
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: (2 * slots) as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(nuke)")?;
        // SAFETY: as above.
        let rtv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: slots as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(nuke rtv)")?;
        // SAFETY: plain queries.
        let (inc, rtv_inc) = unsafe {
            (
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
            )
        };
        let mut queries: Option<ID3D12QueryHeap> = None;
        // SAFETY: valid description; out pointer to a local Option.
        unsafe {
            device.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: (2 * num_frames) as u32, NodeMask: 0 }, &mut queries)
        }
        .ctx("CreateQueryHeap(nuke)")?;
        let queries = queries.ok_or(VmError::Invalid("CreateQueryHeap returned null".into()))?;
        let readback = create_buffer(device, D3D12_HEAP_TYPE_READBACK, (16 * num_frames) as u64, D3D12_RESOURCE_STATE_COPY_DEST)?;
        Ok(NukePass {
            device: device.clone(),
            root_sig,
            psos: Vec::new(),
            bloom_pso,
            heap,
            rtv_heap,
            inc,
            rtv_inc,
            pair: None,
            slot_used: vec![0; slots],
            retired: Vec::new(),
            frames: 0,
            num_frames,
            queries,
            readback,
            slot_written: vec![false; num_frames],
            ts_freq,
            samples: Vec::with_capacity(1024),
            present_allocs: Vec::new(),
        })
    }

    fn slots(&self) -> usize {
        self.num_frames + 1
    }

    fn cpu(&self, i: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query.
        let h0 = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + i * self.inc as usize }
    }

    fn gpu(&self, i: usize) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query.
        let h0 = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + i as u64 * self.inc as u64 }
    }

    fn rtv(&self, slot: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query.
        let h0 = unsafe { self.rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + slot * self.rtv_inc as usize }
    }

    fn pso_index(&mut self, format: DXGI_FORMAT) -> Result<usize, VmError> {
        if let Some(i) = self.psos.iter().position(|(f, _)| *f == format) {
            return Ok(i);
        }
        let p = create_pso(&self.device, &self.root_sig, format, NUKE_PS, true)?;
        self.psos.push((format, p));
        Ok(self.psos.len() - 1)
    }

    /// PSO + scene copy + bloom texture for the swap chain's format and size, ahead of the first frame (loader).
    pub fn prepare(&mut self, rtv_format: DXGI_FORMAT, width: u32, height: u32) -> Result<(), VmError> {
        self.pso_index(rtv_format)?;
        if let Some((family, srv)) = scene_formats(rtv_format)
            && width > 0
            && height > 0
            && width <= 16384
            && height <= 16384
        {
            self.ensure_pair(width, height, family, srv)?;
        }
        Ok(())
    }

    pub fn take_present_allocs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.present_allocs)
    }

    fn free_slot(&self, avoid: Option<usize>) -> Option<usize> {
        let keep = self.num_frames as u64 + 1;
        (0..self.slots()).find(|&s| Some(s) != avoid && (self.slot_used[s] == 0 || self.slot_used[s] + keep <= self.frames))
    }

    fn texture(&self, w: u32, h: u32, format: DXGI_FORMAT, rt: bool) -> Result<ID3D12Resource, VmError> {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: if rt { D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET } else { D3D12_RESOURCE_FLAG_NONE },
        };
        let mut r: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; out pointer to a local Option.
        unsafe {
            self.device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, None, &mut r)
        }
        .ctx("CreateCommittedResource(nuke)")?;
        r.ok_or(VmError::Invalid("nuke texture null".into()))
    }

    fn ensure_pair(&mut self, w: u32, h: u32, family: DXGI_FORMAT, srv: DXGI_FORMAT) -> Result<bool, VmError> {
        if self.pair.as_ref().is_some_and(|p| p.w == w && p.h == h && p.family == family) {
            return Ok(false);
        }
        let slot = self
            .free_slot(self.pair.as_ref().map(|p| p.slot))
            .ok_or(VmError::Invalid(format!("{BUSY_PREFIX} no free nuke slot (resized within {} records); this frame is skipped", self.num_frames + 1)))?;
        let scene = self.texture(w, h, family, false)?;
        let (bw, bh) = bloom_size(w, h);
        let bloom = self.texture(bw, bh, BLOOM_FORMAT, true)?;
        let sd = |format: DXGI_FORMAT| D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        // SAFETY: `slot` is a slot no list of the last num_frames + 1 records used; the views describe the textures
        // just created.
        unsafe {
            self.device.CreateShaderResourceView(&scene, Some(&sd(srv)), self.cpu(2 * slot));
            self.device.CreateShaderResourceView(&bloom, Some(&sd(BLOOM_FORMAT)), self.cpu(2 * slot + 1));
            self.device.CreateRenderTargetView(
                &bloom,
                Some(&D3D12_RENDER_TARGET_VIEW_DESC {
                    Format: BLOOM_FORMAT,
                    ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_RTV { MipSlice: 0, PlaneSlice: 0 } },
                }),
                self.rtv(slot),
            );
        }
        if let Some(old) = self.pair.replace(Pair { scene, bloom, w, h, family, slot }) {
            self.retired.push((self.frames, old.scene, old.bloom));
        }
        Ok(true)
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

    /// Collects the timestamps of a slot whose fence the caller waited on (vmtest).
    pub fn collect(&mut self, slot: usize) {
        self.collect_timing(slot);
    }

    pub fn take_timings(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    /// Records the pass into `list` (target in RENDER_TARGET state). Every check runs before the first command; on
    /// `Err` nothing was recorded.
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, t: &NukeTarget<'_>, slot: usize, consts: &[f32; NUKE_CONSTANTS], timing: bool) -> Result<(), VmError> {
        // ---- 1. validate (no command recorded yet) ----
        if slot >= self.num_frames {
            return Err(VmError::Invalid(format!("slot {slot} >= {}", self.num_frames)));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
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
        if consts.iter().any(|c| !c.is_finite()) {
            return Err(VmError::Invalid("non-finite nuke constants".into()));
        }
        let n_psos = self.psos.len();
        let pi = self.pso_index(t.rtv_format)?;
        if self.psos.len() != n_psos {
            self.present_allocs.push(format!("nuke PSO for {}", format_name(t.rtv_format)));
        }
        if self.ensure_pair(t.width, t.height, family, srv)? {
            self.present_allocs.push(format!("nuke scene + bloom {}x{}", t.width, t.height));
        }
        let (pslot, scene, bloom) = match self.pair.as_ref() {
            Some(p) => (p.slot, p.scene.clone(), p.bloom.clone()),
            None => return Err(VmError::Invalid("no scene copy".into())),
        };
        let (bw, bh) = bloom_size(t.width, t.height);
        self.frames += 1;
        let frames = self.frames;
        let keep = self.num_frames as u64 + 1;
        self.retired.retain(|(f, _, _)| frames < f + keep);
        self.slot_used[pslot] = frames;
        self.collect_timing(slot);
        let table = self.gpu(2 * pslot);
        let bloom_rtv = self.rtv(pslot);
        let timing = timing && self.ts_freq > 0;
        let pso = self.psos[pi].1.clone();
        let pre = [
            transition(t.resource, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE),
            transition(&scene, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST),
        ];
        let mid = [
            transition(&scene, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
            transition(t.resource, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
            transition(&bloom, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
        ];
        let post = [transition(&bloom, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)];
        // ---- 2.-4. record: nothing fallible from here on ----
        // SAFETY: recording into the caller's open list; every object referenced is owned by `self` (retired pairs are
        // kept num_frames + 1 records) or is the caller's back buffer, which outlives the GPU work.
        unsafe {
            if timing {
                list.EndQuery(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot) as u32);
            }
            list.ResourceBarrier(&pre);
            list.CopyResource(&scene, t.resource);
            list.ResourceBarrier(&mid);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, NUKE_CONSTANTS as u32, consts.as_ptr() as *const _, 0);
            list.SetGraphicsRootDescriptorTable(1, table);
            // the bloom (1/8)
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: bw as f32, Height: bh as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: bw as i32, bottom: bh as i32 }]);
            list.OMSetRenderTargets(1, Some(&bloom_rtv), false, None);
            list.SetPipelineState(&self.bloom_pso);
            list.DrawInstanced(3, 1, 0, 0);
            list.ResourceBarrier(&post);
            // the composite into the back buffer
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, None);
            list.SetPipelineState(&pso);
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
    fn render_nuke_shaders_embedded() {
        for (b, s) in [(NUKE_VS, NUKE_VS_SHA256), (NUKE_BLOOM_PS, NUKE_BLOOM_PS_SHA256), (NUKE_PS, NUKE_PS_SHA256)] {
            assert!(b.starts_with(b"DXBC"));
            assert_eq!(mw2er_core::sha256::sha256_hex(b), s);
        }
    }
}
