//! C4.1 probe P2 (docs/C4_1_DESIGN.md 2.1 / 2.4): the layer sample. One small compute list of OURS reads 16 texels of
//! ER's UI layer through OUR SRV and copies them to OUR readback buffer, to pin the layer's alpha convention.
//!
//! - Everything is created by [`LayerSampler::new`] on a worker thread: the root signature, the compute PSO, a
//!   one-descriptor shader-visible heap, the UAV buffer, the persistently mapped readback buffer, one allocator + list
//!   and the fence hudhook signals after the list (patch 0002's splice).
//! - [`LayerSampler::record`] runs on ER's render thread (UiBegin's detour): no allocation, no resource creation. It
//!   writes one SRV descriptor of ER's resource into our heap (a descriptor write; the previous one is no longer read:
//!   the caller records only after the previous sample's fence passed or its list never ran), resets the list and
//!   records `Dispatch(1)` + the copy. Errors are `VmError::D3d` (no allocation).
//! - **ER's resource is never barriered.** The list is executed directly ahead of the Scaleform list whose first
//!   command is ER's own `0xC0 -> RENDER_TARGET` barrier, so the layer is in `0xC0` = PIXEL | NON_PIXEL_SHADER_RESOURCE
//!   at our dispatch; a compute SRV read needs NON_PIXEL_SHADER_RESOURCE, which `0xC0` contains (design 2.4).
//! - Our own UAV buffer goes `COMMON -> UNORDERED_ACCESS -> COPY_SOURCE -> COMMON` with explicit barriers inside the
//!   list (no reliance on implicit promotion / decay); the readback buffer stays in COPY_DEST.

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::{Ctx, Mapped, VmError, buffer_desc, heap_props, serialize_root_signature, transition};

/// The compute shader (DXBC, cs_5_1) compiled by build.rs from shaders/layer_sample.hlsl.
pub const LAYER_SAMPLE_CS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/layer_sample_cs.cso"));
pub const LAYER_SAMPLE_CS_SHA256: &str = env!("MW2ER_LAYER_SAMPLE_CS_SHA256");

/// Texels per sample (= `mw2er_core::c4::SAMPLE_N`).
pub const SAMPLE_TEXELS: usize = 16;
const OUT_BYTES: u64 = (SAMPLE_TEXELS * 4) as u64;

/// `E_INVALIDARG`: the record refused its input (no allocation on the render thread).
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

pub struct LayerSampler {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    /// `[Some(heap)]` built once, so the record binds it without an AddRef / clone on ER's render thread.
    heaps: [Option<ID3D12DescriptorHeap>; 1],
    uav: ID3D12Resource,
    rb: ID3D12Resource,
    rb_ptr: Mapped,
    alloc: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Signalled by hudhook right after our list (patch 0002's splice).
    fence: ID3D12Fence,
}

/// The formats the sample's SRV accepts: 8-bit UNORM RGBA (`mw2er_core::c4::sample_format_ok41`).
pub fn sample_format_ok(f: DXGI_FORMAT) -> bool {
    f == DXGI_FORMAT_R8G8B8A8_UNORM || f == DXGI_FORMAT_B8G8R8A8_UNORM
}

fn create_sample_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
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
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: (SAMPLE_TEXELS * 2) as u32 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 0, RegisterSpace: 0 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 0,
        pStaticSamplers: std::ptr::null(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    serialize_root_signature(dev, &desc)
}

impl LayerSampler {
    /// Creates every object of the sample on `device` (ER's device = the renderer's). Worker thread only.
    pub fn new(device: &ID3D12Device) -> Result<LayerSampler, VmError> {
        let root_sig = create_sample_root_signature(device)?;
        let pd = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: crate::weak(&root_sig),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: LAYER_SAMPLE_CS.as_ptr() as *const _, BytecodeLength: LAYER_SAMPLE_CS.len() },
            NodeMask: 0,
            CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
            Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        };
        // SAFETY: the description and the shader bytes live on this frame during the call.
        let pso: ID3D12PipelineState = unsafe { device.CreateComputePipelineState(&pd) }.ctx("CreateComputePipelineState(layer sample)")?;
        // SAFETY: a valid descriptor-heap description.
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(layer sample)")?;
        let mut ud = buffer_desc(OUT_BYTES);
        ud.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let mut uav: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; out pointer to a local Option. Buffers are created in COMMON.
        unsafe { device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &ud, D3D12_RESOURCE_STATE_COMMON, None, &mut uav) }
            .ctx("CreateCommittedResource(layer sample uav)")?;
        let uav = uav.ok_or(VmError::Invalid("layer sample uav null".into()))?;
        let mut rb: Option<ID3D12Resource> = None;
        // SAFETY: as above; a READBACK buffer lives in COPY_DEST.
        unsafe {
            device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_READBACK), D3D12_HEAP_FLAG_NONE, &buffer_desc(OUT_BYTES), D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb)
        }
        .ctx("CreateCommittedResource(layer sample readback)")?;
        let rb = rb.ok_or(VmError::Invalid("layer sample readback null".into()))?;
        let mut p = std::ptr::null_mut();
        // SAFETY: a READBACK buffer may stay mapped; the CPU reads it only after the fence of the list that wrote it.
        unsafe { rb.Map(0, Some(&D3D12_RANGE { Begin: 0, End: OUT_BYTES as usize }), Some(&mut p)) }.ctx("Map(layer sample readback)")?;
        // SAFETY: plain COM object creation.
        let alloc: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ctx("CreateCommandAllocator(layer sample)")?;
        // SAFETY: as above; a fresh list is open, closed at once so the first record can Reset it.
        let list: ID3D12GraphicsCommandList =
            unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None::<&ID3D12PipelineState>) }.ctx("CreateCommandList(layer sample)")?;
        // SAFETY: closing our own open list.
        unsafe { list.Close() }.ctx("Close(layer sample, new)")?;
        // SAFETY: plain COM object creation.
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ctx("CreateFence(layer sample)")?;
        let heaps = [Some(heap.clone())];
        Ok(LayerSampler { device: device.clone(), root_sig, pso, heap, heaps, uav, rb, rb_ptr: Mapped(p as *mut u8), alloc, list, fence })
    }

    /// Raw COM pointers for hudhook's ECL callback (it clones them into the splice; the sampler keeps them alive).
    pub fn list_raw(&self) -> usize {
        self.list.as_raw() as usize
    }

    pub fn fence_raw(&self) -> usize {
        self.fence.as_raw() as usize
    }

    pub fn device_raw(&self) -> usize {
        self.device.as_raw() as usize
    }

    /// The fence's completed value (`u64::MAX` after a device removal: the caller checks the device).
    pub fn completed(&self) -> u64 {
        // SAFETY: plain COM call.
        unsafe { self.fence.GetCompletedValue() }
    }

    /// `GetDeviceRemovedReason() != S_OK`.
    pub fn device_removed(&self) -> Option<i32> {
        // SAFETY: plain COM call.
        unsafe { self.device.GetDeviceRemovedReason() }.err().map(|e| e.code().0)
    }

    /// Records the sample of `layer` (ER's resource, in `0xC0`, single-sample 2D, 1 mip, of `format`) at `points`
    /// (texel coordinates inside the texture) into our list and closes it. ER's render thread: no allocation, no
    /// resource creation, never waits. The caller guarantees the list is not in flight (the previous sample's fence
    /// passed, or its list was never executed). After `Ok` the list may be spliced; it never barriers `layer`. On `Err`
    /// the list is closed (a failed Reset leaves it closed; only Reset and Close can fail) and must not be spliced.
    pub fn record(&mut self, layer: &ID3D12Resource, format: DXGI_FORMAT, points: &[[u32; 2]; SAMPLE_TEXELS]) -> Result<(), VmError> {
        if !sample_format_ok(format) {
            return Err(VmError::D3d { call: "LayerSampler::record(format)", hr: E_INVALIDARG });
        }
        let mut consts = [0u32; SAMPLE_TEXELS * 2];
        for (i, p) in points.iter().enumerate() {
            consts[2 * i] = p[0];
            consts[2 * i + 1] = p[1];
        }
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        // SAFETY: our list's previous execution finished (caller), so the allocator may be reset and the descriptor
        // rewritten; the SRV is of the caller's live resource (alive for this UI frame: ER's own list of the same
        // ECL call references it); everything else is ours and outlives the GPU work (never released in-session).
        unsafe {
            self.alloc.Reset().ctx("Reset(layer sample allocator)")?;
            self.list.Reset(&self.alloc, &self.pso).ctx("Reset(layer sample list)")?;
            self.device.CreateShaderResourceView(layer, Some(&srv), self.heap.GetCPUDescriptorHandleForHeapStart());
            self.list.SetDescriptorHeaps(&self.heaps);
            self.list.SetComputeRootSignature(&self.root_sig);
            self.list.SetComputeRoot32BitConstants(0, consts.len() as u32, consts.as_ptr() as *const _, 0);
            self.list.SetComputeRootDescriptorTable(1, self.heap.GetGPUDescriptorHandleForHeapStart());
            self.list.ResourceBarrier(&[transition(&self.uav, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
            self.list.SetComputeRootUnorderedAccessView(2, self.uav.GetGPUVirtualAddress());
            self.list.Dispatch(1, 1, 1);
            self.list.ResourceBarrier(&[transition(&self.uav, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            self.list.CopyBufferRegion(&self.rb, 0, &self.uav, 0, OUT_BYTES);
            self.list.ResourceBarrier(&[transition(&self.uav, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON)]);
            self.list.Close().ctx("Close(layer sample)")
        }
    }

    /// The 16 packed texels (`r | g << 8 | b << 16 | a << 24`) of the last executed record. Only after its fence value
    /// completed (the caller checks).
    pub fn read(&self) -> [u32; SAMPLE_TEXELS] {
        let mut out = [0u32; SAMPLE_TEXELS];
        // SAFETY: the mapped readback buffer holds OUT_BYTES bytes; the GPU finished writing it (caller); unaligned-safe
        // byte copy.
        unsafe { std::ptr::copy_nonoverlapping(self.rb_ptr.0 as *const u8, out.as_mut_ptr() as *mut u8, OUT_BYTES as usize) };
        out
    }

    /// The list (vmtest: executed like hudhook's splice does).
    pub fn list(&self) -> &ID3D12GraphicsCommandList {
        &self.list
    }

    pub fn fence(&self) -> &ID3D12Fence {
        &self.fence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_sample_shader_and_formats() {
        assert!(LAYER_SAMPLE_CS.starts_with(b"DXBC"));
        assert_eq!(mw2er_core::sha256::sha256_hex(LAYER_SAMPLE_CS), LAYER_SAMPLE_CS_SHA256);
        assert_eq!(SAMPLE_TEXELS, mw2er_core::c4::SAMPLE_N);
        for f in [DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R10G10B10A2_UNORM] {
            assert_eq!(sample_format_ok(f), mw2er_core::c4::sample_format_ok41(f.0 as u32), "{f:?}");
        }
        assert_eq!(D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, 0x1688);
    }
}
