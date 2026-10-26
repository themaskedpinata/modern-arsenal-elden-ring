//! Run 8: the AC-130 round tracer pass, shared by `mw2er.dll` (hudhook's pre-ImGui callback, recorded right BEFORE the
//! thermal pass) and `vmtest.exe` (case TR and its perf): same code, same shader bytes (`shaders/tracer.hlsl`).
//!
//! - Instances come from `mw2er_core::ac130_tracer::build_instances` (CPU projection through the gunship camera, the
//!   same convention as the HUD's `project`). They are written into a persistently mapped UPLOAD ring of `num_frames`
//!   slots x [`MAX_INSTANCES`] (the slot of the frame being recorded; hudhook waits for a back buffer's previous frame
//!   before it records into it again, the same rule the thermal timestamps rely on) and read through a root SRV.
//! - No allocation per frame, no descriptor heap, no resource barrier: the back buffer stays in RENDER_TARGET.
//! - Blend ONE + ONE on RGB (additive light), the back buffer's alpha untouched; no depth.
//!
//! Every check runs before the first command; on `Err` nothing was recorded.

use mw2er_core::ac130_tracer::{INSTANCE_BYTES, MAX_INSTANCES, TracerInstance};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::{Ctx, VmError, base_format, create_buffer, format_name, weak};

/// Tracer vertex shader (DXBC, vs_5_1) compiled by build.rs from shaders/tracer.hlsl.
pub const TRACER_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tracer_vs.cso"));
/// Tracer pixel shader (DXBC, ps_5_1).
pub const TRACER_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tracer_ps.cso"));
pub const TRACER_VS_SHA256: &str = env!("MW2ER_TRACER_VS_SHA256");
pub const TRACER_PS_SHA256: &str = env!("MW2ER_TRACER_PS_SHA256");

const SLOT_BYTES: usize = MAX_INSTANCES * INSTANCE_BYTES;

/// The caller's back buffer (RENDER_TARGET state).
pub struct TracerTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub rtv_format: DXGI_FORMAT,
    pub width: u32,
    pub height: u32,
}

pub struct TracerPass {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    ring: ID3D12Resource,
    mapped: *mut u8,
    num_frames: usize,
    queries: ID3D12QueryHeap,
    readback: ID3D12Resource,
    slot_written: Vec<bool>,
    ts_freq: u64,
    samples: Vec<f32>,
    present_allocs: Vec<String>,
}

// SAFETY: `mapped` points into `ring`, which the pass owns; the pass is used by one thread at a time (behind the DLL's
// Mutex / vmtest's single thread) and the GPU only reads the slot of a frame it is executing.
unsafe impl Send for TracerPass {}

fn create_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS
            | D3D12_ROOT_SIGNATURE_FLAG_DENY_PIXEL_SHADER_ROOT_ACCESS,
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
        return Err(VmError::Invalid(format!("tracer D3D12SerializeRootSignature 0x{:08x} {}", e.code().0 as u32, msg.unwrap_or_default())));
    }
    let blob = blob.ok_or(VmError::Invalid("tracer root signature blob null".into()))?;
    // SAFETY: the blob's buffer is valid for GetBufferSize bytes while `blob` lives.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) };
    // SAFETY: serialized root signature bytes.
    unsafe { dev.CreateRootSignature(0, bytes) }.ctx("CreateRootSignature(tracer)")
}

fn create_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, VmError> {
    let rt = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ONE,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ZERO,
        DestBlendAlpha: D3D12_BLEND_ONE,
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
        VS: D3D12_SHADER_BYTECODE { pShaderBytecode: TRACER_VS.as_ptr() as *const _, BytecodeLength: TRACER_VS.len() },
        PS: D3D12_SHADER_BYTECODE { pShaderBytecode: TRACER_PS.as_ptr() as *const _, BytecodeLength: TRACER_PS.len() },
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
    unsafe { dev.CreateGraphicsPipelineState(&desc) }.ctx("CreateGraphicsPipelineState(tracer)")
}

impl TracerPass {
    /// Root signature, the mapped instance ring (`num_frames` slots) and the timestamp heap.
    pub fn new(device: &ID3D12Device, num_frames: usize, ts_freq: u64) -> Result<Self, VmError> {
        if !(1..=8).contains(&num_frames) {
            return Err(VmError::Invalid(format!("num_frames {num_frames}")));
        }
        let root_sig = create_root_signature(device)?;
        let ring = create_buffer(device, D3D12_HEAP_TYPE_UPLOAD, (SLOT_BYTES * num_frames) as u64, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: mapping an UPLOAD buffer for CPU writes; it stays mapped for the pass's life (allowed for UPLOAD heaps).
        unsafe { ring.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(tracer ring)")?;
        if p.is_null() {
            return Err(VmError::Invalid("tracer ring Map returned null".into()));
        }
        let mut queries: Option<ID3D12QueryHeap> = None;
        // SAFETY: valid description; out pointer to a local Option.
        unsafe {
            device.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: (2 * num_frames) as u32, NodeMask: 0 }, &mut queries)
        }
        .ctx("CreateQueryHeap(tracer)")?;
        let queries = queries.ok_or(VmError::Invalid("CreateQueryHeap returned null".into()))?;
        let readback = create_buffer(device, D3D12_HEAP_TYPE_READBACK, (16 * num_frames) as u64, D3D12_RESOURCE_STATE_COPY_DEST)?;
        Ok(TracerPass {
            device: device.clone(),
            root_sig,
            psos: Vec::new(),
            ring,
            mapped: p as *mut u8,
            num_frames,
            queries,
            readback,
            slot_written: vec![false; num_frames],
            ts_freq,
            samples: Vec::with_capacity(1024),
            present_allocs: Vec::new(),
        })
    }

    fn pso_index(&mut self, format: DXGI_FORMAT) -> Result<usize, VmError> {
        if let Some(i) = self.psos.iter().position(|(f, _)| *f == format) {
            return Ok(i);
        }
        let p = create_pso(&self.device, &self.root_sig, format)?;
        self.psos.push((format, p));
        Ok(self.psos.len() - 1)
    }

    /// The PSO for the swap chain's format, ahead of the first frame (loader thread).
    pub fn prepare(&mut self, rtv_format: DXGI_FORMAT) -> Result<(), VmError> {
        self.pso_index(rtv_format).map(|_| ())
    }

    /// What `record` had to create on the present thread since the last call (empty normally).
    pub fn take_present_allocs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.present_allocs)
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

    /// Drains the collected GPU-time samples (ms).
    pub fn take_timings(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    /// Records `instances` additively into the target (RENDER_TARGET state; it stays there). An empty list records
    /// nothing. `timing` brackets the draw with timestamps (read back by `collect` / the next record of the slot).
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, t: &TracerTarget<'_>, slot: usize, instances: &[TracerInstance], timing: bool) -> Result<(), VmError> {
        // ---- validate (no command recorded yet) ----
        if slot >= self.num_frames {
            return Err(VmError::Invalid(format!("slot {slot} >= {}", self.num_frames)));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("target {}x{}", t.width, t.height)));
        }
        if instances.len() > MAX_INSTANCES {
            return Err(VmError::Invalid(format!("{} tracer instances > {MAX_INSTANCES}", instances.len())));
        }
        // SAFETY: plain query on the caller's live back buffer.
        let live = unsafe { t.resource.GetDesc() };
        if live.Width != t.width as u64 || live.Height != t.height || live.SampleDesc.Count != 1 {
            return Err(VmError::Invalid(format!("back buffer {}x{} ms{} != target {}x{}", live.Width, live.Height, live.SampleDesc.Count, t.width, t.height)));
        }
        if base_format(live.Format) != base_format(t.rtv_format) {
            return Err(VmError::Invalid(format!("back-buffer format {} != RTV {}", format_name(live.Format), format_name(t.rtv_format))));
        }
        self.collect_timing(slot);
        if instances.is_empty() {
            return Ok(());
        }
        let n_psos = self.psos.len();
        let pi = self.pso_index(t.rtv_format)?;
        if self.psos.len() != n_psos {
            self.present_allocs.push(format!("tracer PSO for {}", format_name(t.rtv_format)));
        }
        // ---- the slot's instances: plain floats into the mapped ring (no allocation) ----
        let base = slot * SLOT_BYTES;
        for (k, inst) in instances.iter().enumerate() {
            let f = inst.floats();
            // SAFETY: `mapped` covers SLOT_BYTES * num_frames bytes; slot < num_frames and k < MAX_INSTANCES, so the
            // 32 bytes written stay inside this slot. The GPU reads this slot only for the frame recorded now.
            unsafe { std::ptr::copy_nonoverlapping(f.as_ptr() as *const u8, self.mapped.add(base + k * INSTANCE_BYTES), INSTANCE_BYTES) };
        }
        // SAFETY: plain query.
        let gpu_va = unsafe { self.ring.GetGPUVirtualAddress() } + base as u64;
        let consts = [t.width as f32, t.height as f32, 0.0, 0.0];
        let timing = timing && self.ts_freq > 0;
        let pso = &self.psos[pi].1;
        // ---- record: nothing fallible from here on ----
        // SAFETY: recording into the caller's open list; every object referenced is owned by `self` or is the caller's
        // back buffer, which outlives the GPU work.
        unsafe {
            if timing {
                list.EndQuery(&self.queries, D3D12_QUERY_TYPE_TIMESTAMP, (2 * slot) as u32);
            }
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&t.rtv), false, None);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.SetPipelineState(pso);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const _, 0);
            list.SetGraphicsRootShaderResourceView(1, gpu_va);
            list.DrawInstanced(6, instances.len() as u32, 0, 0);
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

impl Drop for TracerPass {
    fn drop(&mut self) {
        // SAFETY: unmapping our own UPLOAD ring (mapped once in new).
        unsafe { self.ring.Unmap(0, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tracer_shaders_embedded() {
        assert!(TRACER_VS.starts_with(b"DXBC") && TRACER_PS.starts_with(b"DXBC"));
        assert_eq!(mw2er_core::sha256::sha256_hex(TRACER_VS), TRACER_VS_SHA256);
        assert_eq!(mw2er_core::sha256::sha256_hex(TRACER_PS), TRACER_PS_SHA256);
        assert_eq!(SLOT_BYTES, MAX_INSTANCES * 32);
    }
}
