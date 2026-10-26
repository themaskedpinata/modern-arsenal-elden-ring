//! C6 depth fallback (work/c6/depth3.md 3, docs/C6_HOLD_DESIGN.md 13.2): depth-only body capsules drawn into the world
//! guns' own depth buffer right after its clear, so a world gun behind a character's body fails its depth test.
//!
//! - Our objects only: a root signature (one root CBV), a depth-only PSO (no pixel shader, no render target, D32_FLOAT,
//!   GREATER = the renderer's reversed-Z) and a persistently mapped upload ring (one 256-aligned block per frame slot).
//!   Everything is created by [`Occluders::new`] (the loader thread, `Renderer::prepare_occluders`); recording allocates
//!   nothing, creates nothing and records no barrier.
//! - The capsule mesh is generated in the vertex shader (shaders/occluder.hlsl): `VERTS_PER_CAPSULE` vertices per
//!   instance, one instance per capsule. ER's depth is never bound, read or tested.

use mw2er_core::body_occl::{Capsule, MAX_CAPSULES};
use mw2er_core::xform::Mat4;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::{Ctx, Mapped, VmError, create_buffer, serialize_root_signature, weak};

/// Vertex shader (DXBC vs_5_1) of shaders/occluder.hlsl.
pub const OCCLUDER_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/occluder_vs.cso"));
pub const OCCLUDER_VS_SHA256: &str = env!("MW2ER_OCCLUDER_VS_SHA256");
/// shaders/occluder.hlsl: SEGS 16, HALF 4 -> (2 * 4 + 1) rows of quads x 16 segments x 6 vertices.
pub const VERTS_PER_CAPSULE: u32 = (2 * 4 + 1) * 16 * 6;
/// `cbuffer Capsules`: view_proj (64 B) + 2 float4 per capsule.
pub const CAPS_CB_BYTES: usize = 64 + MAX_CAPSULES * 32;
const SLOT: usize = crate::align_up(CAPS_CB_BYTES, 256);

pub struct Occluders {
    rs: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    ring: ID3D12Resource,
    ptr: Mapped,
    num_frames: usize,
}

/// The constant block of one frame (no allocation): view_proj (row-major) then (a, r), (b, 0) per capsule; at most
/// [`MAX_CAPSULES`]. Capsules with a non-finite value or a radius outside (0, 1] m are dropped. Returns the count.
pub fn fill_consts(block: &mut [f32; CAPS_CB_BYTES / 4], view_proj: &Mat4, caps: &[Capsule]) -> u32 {
    for (r, row) in view_proj.iter().enumerate() {
        block[r * 4..r * 4 + 4].copy_from_slice(row);
    }
    let mut n = 0u32;
    for c in caps {
        if n as usize >= MAX_CAPSULES {
            break;
        }
        if !(c.r > 0.0 && c.r <= 1.0) || !c.a.iter().chain(&c.b).all(|x| x.is_finite()) {
            continue;
        }
        let o = 16 + n as usize * 8;
        block[o..o + 8].copy_from_slice(&[c.a[0], c.a[1], c.a[2], c.r, c.b[0], c.b[1], c.b[2], 0.0]);
        n += 1;
    }
    n
}

impl Occluders {
    /// Root signature, PSO and the ring (`num_frames` blocks). Loader thread / vmtest only.
    pub fn new(dev: &ID3D12Device, num_frames: usize) -> Result<Occluders, VmError> {
        let params = [D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
        }];
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
        let rs = serialize_root_signature(dev, &desc)?;
        let stencil_op = D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: D3D12_STENCIL_OP_KEEP,
            StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
            StencilPassOp: D3D12_STENCIL_OP_KEEP,
            StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
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
            RenderTargetWriteMask: 0,
        };
        let pd = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: weak(&rs),
            VS: D3D12_SHADER_BYTECODE { pShaderBytecode: OCCLUDER_VS.as_ptr() as *const _, BytecodeLength: OCCLUDER_VS.len() },
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
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_GREATER,
                StencilEnable: false.into(),
                StencilReadMask: 0xFF,
                StencilWriteMask: 0xFF,
                FrontFace: stencil_op,
                BackFace: stencil_op,
            },
            InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: std::ptr::null(), NumElements: 0 },
            IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 0,
            RTVFormats: [DXGI_FORMAT_UNKNOWN; 8],
            DSVFormat: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            NodeMask: 0,
            CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
            Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
            ..Default::default()
        };
        // SAFETY: `pd` points at stack data and the embedded shader bytes, all alive during the call.
        let pso = unsafe { dev.CreateGraphicsPipelineState(&pd) }.ctx("CreateGraphicsPipelineState(occluder)")?;
        let ring = create_buffer(dev, D3D12_HEAP_TYPE_UPLOAD, (SLOT * num_frames.max(1)) as u64, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: an UPLOAD buffer may stay mapped for its lifetime; the CPU never reads it (empty read range).
        unsafe { ring.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(occluder ring)")?;
        Ok(Occluders { rs, pso, ring, ptr: Mapped(p as *mut u8), num_frames: num_frames.max(1) })
    }

    /// Records the capsules into `list`: the caller has our depth (`dsv`) cleared; the render target is unbound for the
    /// depth-only draws (the caller re-binds it). `slot` = the frame slot (the caller waited on its fence before reuse).
    /// Returns the capsules drawn (0 = nothing recorded).
    pub(crate) fn record(&self, list: &ID3D12GraphicsCommandList, slot: usize, dsv: D3D12_CPU_DESCRIPTOR_HANDLE, view_proj: &Mat4, caps: &[Capsule]) -> u32 {
        if slot >= self.num_frames || caps.is_empty() {
            return 0;
        }
        let mut block = [0f32; CAPS_CB_BYTES / 4];
        let n = fill_consts(&mut block, view_proj, caps);
        if n == 0 {
            return 0;
        }
        // SAFETY: the ring is mapped for its lifetime with SLOT * num_frames bytes; slot < num_frames; the GPU finished
        // reading this slot (the caller waited on its fence before reusing the frame index).
        unsafe { std::ptr::copy_nonoverlapping(block.as_ptr() as *const u8, self.ptr.0.add(slot * SLOT), CAPS_CB_BYTES) };
        // SAFETY: recording into the caller's open list; every object is ours and lives as long as the renderer.
        unsafe {
            let va = self.ring.GetGPUVirtualAddress() + (slot * SLOT) as u64;
            list.OMSetRenderTargets(0, None, false, Some(&dsv));
            list.SetGraphicsRootSignature(&self.rs);
            list.SetPipelineState(&self.pso);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRootConstantBufferView(0, va);
            list.DrawInstanced(VERTS_PER_CAPSULE, n, 0, 0);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_occluder_consts_layout() {
        let vp = [[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0], [9.0, 10.0, 11.0, 12.0], [13.0, 14.0, 15.0, 16.0]];
        let good = Capsule { a: [1.0, 2.0, 3.0], b: [4.0, 5.0, 6.0], r: 0.1 };
        let bad = [Capsule { r: 0.0, ..good }, Capsule { r: 1.5, ..good }, Capsule { a: [f32::NAN, 0.0, 0.0], ..good }];
        let mut caps = vec![good];
        caps.extend_from_slice(&bad);
        let mut v = [0f32; CAPS_CB_BYTES / 4];
        assert_eq!(fill_consts(&mut v, &vp, &caps), 1);
        assert_eq!(&v[..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&v[16..24], &[1.0, 2.0, 3.0, 0.1, 4.0, 5.0, 6.0, 0.0]);
        let many = vec![good; MAX_CAPSULES + 5];
        assert_eq!(fill_consts(&mut v, &vp, &many) as usize, MAX_CAPSULES);
        assert_eq!(VERTS_PER_CAPSULE, 864);
        assert_eq!(SLOT % 256, 0);
        assert!(CAPS_CB_BYTES <= 4096 * 16, "cbuffer size limit");
        assert_eq!(MAX_CAPSULES * 2, 128, "caps[128] in occluder.hlsl");
    }
}
