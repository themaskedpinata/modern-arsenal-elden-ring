//! C6 depth 5 (docs/C6_HOLD_DESIGN.md part 15): the GPU side of depth-testing world guns against ER's scene depth.
//!
//! - **The barrier hook's detour** ([`resource_barrier_detour`]): installed by the DLL with MinHook on d3d12's
//!   `ID3D12GraphicsCommandList::ResourceBarrier` (vtable slot 26) only with `[coop] probe_er_depth_state`. It calls the
//!   ORIGINAL FIRST (ER's barriers pass unchanged), then, while [`ON`] is set and [`WATCH`] names ER's scene depth, reads
//!   the caller's barrier array (still the caller's, valid until the call returns) and records the transitions on that
//!   resource into [`TABLE`] (`mw2er_core::er_depth_state::Table`: atomics, no lock, no allocation, no log, no COM
//!   call). It lives here (not in the DLL) so vmtest ED5 runs the very same function on WARP's list.
//! - **[`DepthReader`]** (worker-created; recorded on ER's render thread in UiBegin's detour): OUR list reads ER's
//!   depth plane 0 through OUR SRV (`R32_FLOAT_X8X24_TYPELESS`) and writes the farthest of each 2x2 block into OUR
//!   half-resolution R32_FLOAT texture (the copy) and / or 22 texels into OUR readback (probe P-S2). It is spliced at
//!   index 0 of the armed UiBegin ECL only when the fold proved the plane readable there; **it never barriers, binds
//!   for writing or writes ER's resource**: the only barriers are on our own texture (`0xC0 -> UAV -> 0xC0`) and our
//!   own buffer (`COMMON -> UAV -> COPY_SOURCE -> COMMON`).
//! - **[`DepthPrime`]** (created on the loader thread, `Renderer::prepare_er_depth`): at Present, after our world-gun
//!   depth clear and before the capsules and the guns, a full-screen depth-only pass (no render target, depth func
//!   ALWAYS, SV_Depth) writes the copy linearised and pushed back into OUR depth. The gun PSOs are unchanged.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use mw2er_core::er_depth_state::{Bar, Census, SAMPLE_N, SLOTS, Table, half};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::Interface;

use crate::{Ctx, Mapped, VmError, buffer_desc, heap_props, serialize_root_signature, transition, weak};

pub const COPY_CS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/er_depth_copy_cs.cso"));
pub const SAMPLE_CS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/er_depth_sample_cs.cso"));
pub const PRIME_VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/er_depth_vs.cso"));
pub const PRIME_PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/er_depth_prime_ps.cso"));
pub const COPY_CS_SHA256: &str = env!("MW2ER_ER_DEPTH_COPY_CS_SHA256");
pub const SAMPLE_CS_SHA256: &str = env!("MW2ER_ER_DEPTH_SAMPLE_CS_SHA256");
pub const PRIME_VS_SHA256: &str = env!("MW2ER_ER_DEPTH_VS_SHA256");
pub const PRIME_PS_SHA256: &str = env!("MW2ER_ER_DEPTH_PRIME_PS_SHA256");

/// The plane-0 SRV format of a `R32G8X24_TYPELESS` depth (DXGI 21).
pub const SRV_PLANE0: DXGI_FORMAT = DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS;
/// Our texture's resting state: PIXEL | NON_PIXEL_SHADER_RESOURCE (the prime reads it in a pixel shader).
pub const COPY_REST: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(0xC0);
const OUT_BYTES: u64 = (SAMPLE_N * 4) as u64;
/// The reader's root constants: dims (4 words) + 2 per sample point (C6 round 7: 22 points = 48 DWORDs).
const READER_CONSTS: usize = 4 + 2 * SAMPLE_N;
const _: () = assert!(READER_CONSTS == 48 && SAMPLE_N == 22);
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
/// At most this many barriers of one call are read (a bound for the detour; ER's calls are far smaller).
pub const MAX_BARRIERS_READ: u32 = 4096;

// ---- the barrier hook (P-S) -------------------------------------------------------------------------------------------

/// The barrier table of candidate A (one per process).
pub static TABLE: Table = Table::new();
/// C6 round 6 (work/c6/depth6.md 4): candidate B's table (the scene context's DefDepthStencil) and the depth census.
pub static TABLE_B: Table = Table::new();
pub static CENSUS: Census = Census::new();
/// The trampoline of the hooked `ResourceBarrier` (MinHook; 0 = not installed: the detour calls nothing).
pub static TRAMP: AtomicUsize = AtomicUsize::new(0);
/// Recording on (cleared first at teardown: the detour is then a pure passthrough).
pub static ON: AtomicBool = AtomicBool::new(false);
/// ER's scene depth candidate A (`ID3D12Resource*`) being watched (0 = none); C6 round 6: candidate B too.
pub static WATCH: AtomicUsize = AtomicUsize::new(0);
pub static WATCH_B: AtomicUsize = AtomicUsize::new(0);
/// Calls of the detour (C6 round 6: EVERY call, counted before any gate: a dead hook reads 0), the barriers and
/// transitions parsed while recording, and the panics caught in its body.
pub static CALLS: AtomicU64 = AtomicU64::new(0);
pub static BARS: AtomicU64 = AtomicU64::new(0);
pub static TRANS: AtomicU64 = AtomicU64::new(0);
pub static PANICS: AtomicU64 = AtomicU64::new(0);

pub type ResourceBarrierFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_RESOURCE_BARRIER);

/// `D3D12_RESOURCE_BARRIER` as raw words (x64: 32 bytes): Type, Flags, then the union (Transition: pResource,
/// Subresource, StateBefore, StateAfter; Aliasing: pResourceBefore, pResourceAfter; UAV: pResource).
#[repr(C)]
#[derive(Clone, Copy)]
struct RawBarrier {
    ty: u32,
    flags: u32,
    p0: usize,
    w0: u32,
    w1: u32,
    w2: u32,
    _pad: u32,
}

const _: () = assert!(std::mem::size_of::<RawBarrier>() == std::mem::size_of::<D3D12_RESOURCE_BARRIER>());

/// Parses one barrier (pure: the raw words).
fn parse_one(r: &RawBarrier) -> Bar {
    match r.ty {
        0 => Bar::Transition { res: r.p0, sub: r.w0, before: r.w1, after: r.w2, flags: r.flags },
        1 => Bar::Alias { before: r.p0, after: r.w0 as usize | (r.w1 as usize) << 32 },
        2 => Bar::Uav { res: r.p0 },
        _ => Bar::Other,
    }
}

/// Every barrier of a `ResourceBarrier(n, p)` call, in order.
///
/// # Safety
/// `p` points at `n` barriers (the caller's array of the call being intercepted, still alive).
pub unsafe fn for_each_barrier(n: u32, p: *const D3D12_RESOURCE_BARRIER, mut f: impl FnMut(Bar)) {
    if p.is_null() {
        return;
    }
    let raw = p as *const RawBarrier;
    for i in 0..n.min(MAX_BARRIERS_READ) as usize {
        // SAFETY: the caller's array holds `n` barriers; `RawBarrier` has the struct's size and alignment (plain words).
        let r = unsafe { std::ptr::read_unaligned(raw.add(i)) };
        f(parse_one(&r));
    }
}

/// The hook's detour: the ORIGINAL first, then (recording on) every barrier counted, the depth-state transitions into
/// [`CENSUS`] and the transitions on the watched candidates into [`TABLE`] / [`TABLE_B`]. Atomics and plain reads only; a
/// panic in the body is caught and counted (never unwinds into ER).
///
/// # Safety
/// Installed as the detour of `ID3D12GraphicsCommandList::ResourceBarrier` with [`TRAMP`] set to its trampoline
/// before the hook is enabled (vmtest: the real slot-26 function); called with that method's arguments.
pub unsafe extern "system" fn resource_barrier_detour(this: *mut c_void, n: u32, p: *const D3D12_RESOURCE_BARRIER) {
    let f = TRAMP.load(Ordering::Acquire);
    if f != 0 {
        // SAFETY: the trampoline of the hooked method, called first with ER's own arguments.
        unsafe { std::mem::transmute::<usize, ResourceBarrierFn>(f)(this, n, p) };
    }
    // C6 round 6: before any gate (the 20:57 run could not tell a dead hook from an idle resource)
    CALLS.fetch_add(1, Ordering::Relaxed);
    if !ON.load(Ordering::Relaxed) || n == 0 || p.is_null() {
        return;
    }
    let (wa, wb) = (WATCH.load(Ordering::Relaxed), WATCH_B.load(Ordering::Relaxed));
    let list = this as usize;
    if catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: GetCurrentThreadId has no preconditions.
        let tid = unsafe { GetCurrentThreadId() };
        // SAFETY: the method's own (n, p): the caller's array, valid for this call.
        unsafe {
            for_each_barrier(n, p, |b| {
                BARS.fetch_add(1, Ordering::Relaxed);
                if let Bar::Transition { res, sub, before, after, flags } = b {
                    TRANS.fetch_add(1, Ordering::Relaxed);
                    CENSUS.record(res, sub, before, after, flags);
                }
                TABLE.record(list, tid, wa, b);
                TABLE_B.record(list, tid, wb, b);
            })
        };
    }))
    .is_err()
    {
        PANICS.fetch_add(1, Ordering::Relaxed);
    }
}

// ---- the reader: copy + sample (our list, ER's render thread) -----------------------------------------------------------

struct ReaderSlot {
    alloc: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    tex: ID3D12Resource,
}

pub struct DepthReader {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    copy_pso: ID3D12PipelineState,
    sample_pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    /// `[Some(heap)]` built once: the record binds it without an AddRef on ER's render thread.
    heaps: [Option<ID3D12DescriptorHeap>; 1],
    inc: usize,
    slots: Vec<ReaderSlot>,
    w: u32,
    h: u32,
    out: ID3D12Resource,
    rb: ID3D12Resource,
    rb_ptr: Mapped,
    /// Signalled by hudhook right after our list (patch 0002's splice).
    fence: ID3D12Fence,
}

fn compute_root_signature(dev: &ID3D12Device) -> Result<ID3D12RootSignature, VmError> {
    let srv = D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV, NumDescriptors: 1, BaseShaderRegister: 0, RegisterSpace: 0, OffsetInDescriptorsFromTableStart: 0 };
    let uav = D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV, NumDescriptors: 1, BaseShaderRegister: 0, RegisterSpace: 0, OffsetInDescriptorsFromTableStart: 0 };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: READER_CONSTS as u32 } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &srv } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &uav } },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
            Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 1, RegisterSpace: 0 } },
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

fn compute_pso(dev: &ID3D12Device, rs: &ID3D12RootSignature, cs: &[u8], call: &'static str) -> Result<ID3D12PipelineState, VmError> {
    let pd = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: weak(rs),
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: cs.as_ptr() as *const _, BytecodeLength: cs.len() },
        NodeMask: 0,
        CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
    };
    // SAFETY: the description and the shader bytes live during the call.
    unsafe { dev.CreateComputePipelineState(&pd) }.ctx(call)
}

impl DepthReader {
    /// Every object of the copy / sample for ER's depth of `w` x `h` (the back-buffer size): root signature, two
    /// compute PSOs, a shader-visible heap (2 descriptors per slot), per slot an allocator, a list and our
    /// half-resolution R32_FLOAT texture (created in `0xC0`), the sample's UAV + mapped readback buffer and the fence.
    /// Worker thread only (never ER's render thread or the present thread). `fence_start`: the fence's initial value
    /// (a resize: the DLL's last spliced value, so its slot / sample values stay monotonic across readers).
    pub fn new(device: &ID3D12Device, w: u32, h: u32, fence_start: u64) -> Result<DepthReader, VmError> {
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err(VmError::Invalid(format!("er depth reader size {w}x{h}")));
        }
        let root_sig = compute_root_signature(device)?;
        let copy_pso = compute_pso(device, &root_sig, COPY_CS, "CreateComputePipelineState(er depth copy)")?;
        let sample_pso = compute_pso(device, &root_sig, SAMPLE_CS, "CreateComputePipelineState(er depth sample)")?;
        // SAFETY: a valid descriptor-heap description.
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: (2 * SLOTS) as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(er depth reader)")?;
        // SAFETY: plain query.
        let inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) } as usize;
        let (hw, hh) = half(w, h);
        let td = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: hw as u64,
            Height: hh,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        };
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            // SAFETY: plain COM object creation.
            let alloc: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ctx("CreateCommandAllocator(er depth)")?;
            // SAFETY: as above; a fresh list is open, closed at once so the first record can Reset it.
            let list: ID3D12GraphicsCommandList =
                unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None::<&ID3D12PipelineState>) }.ctx("CreateCommandList(er depth)")?;
            // SAFETY: closing our own open list.
            unsafe { list.Close() }.ctx("Close(er depth, new)")?;
            let mut tex: Option<ID3D12Resource> = None;
            // SAFETY: a valid texture description; out pointer to a local Option.
            unsafe { device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &td, COPY_REST, None, &mut tex) }
                .ctx("CreateCommittedResource(er depth copy)")?;
            let tex = tex.ok_or(VmError::Invalid("er depth copy texture null".into()))?;
            slots.push(ReaderSlot { alloc, list, tex });
        }
        let mut ud = buffer_desc(OUT_BYTES);
        ud.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let mut out: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; out pointer to a local Option. Buffers are created in COMMON.
        unsafe { device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &ud, D3D12_RESOURCE_STATE_COMMON, None, &mut out) }
            .ctx("CreateCommittedResource(er depth sample uav)")?;
        let out = out.ok_or(VmError::Invalid("er depth sample uav null".into()))?;
        let mut rb: Option<ID3D12Resource> = None;
        // SAFETY: as above; a READBACK buffer lives in COPY_DEST.
        unsafe {
            device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_READBACK), D3D12_HEAP_FLAG_NONE, &buffer_desc(OUT_BYTES), D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb)
        }
        .ctx("CreateCommittedResource(er depth sample readback)")?;
        let rb = rb.ok_or(VmError::Invalid("er depth sample readback null".into()))?;
        let mut p = std::ptr::null_mut();
        // SAFETY: a READBACK buffer may stay mapped; the CPU reads it only after the fence of the list that wrote it.
        unsafe { rb.Map(0, Some(&D3D12_RANGE { Begin: 0, End: OUT_BYTES as usize }), Some(&mut p)) }.ctx("Map(er depth sample readback)")?;
        // SAFETY: plain COM object creation.
        let fence: ID3D12Fence = unsafe { device.CreateFence(fence_start, D3D12_FENCE_FLAG_NONE) }.ctx("CreateFence(er depth)")?;
        let heaps = [Some(heap.clone())];
        Ok(DepthReader { device: device.clone(), root_sig, copy_pso, sample_pso, heap, heaps, inc, slots, w, h, out, rb, rb_ptr: Mapped(p as *mut u8), fence })
    }

    /// ER's depth size this reader was made for.
    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Raw COM pointers for hudhook's ECL callback (it clones them into the splice; the reader keeps them alive).
    pub fn list_raw(&self, slot: usize) -> usize {
        self.slots.get(slot).map_or(0, |s| s.list.as_raw() as usize)
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

    /// Our copy texture of `slot` (half resolution, R32_FLOAT, resting in `0xC0`).
    pub fn texture(&self, slot: usize) -> Option<&ID3D12Resource> {
        self.slots.get(slot).map(|s| &s.tex)
    }

    /// Records into `slot`'s list: the copy (`copy`) and / or the 22-texel sample (`points`) of ER's depth `res` (plane
    /// 0, in a state that contains NON_PIXEL_SHADER_RESOURCE when the list runs: the caller splices it only where the
    /// fold proved that), then closes it. ER's render thread: no allocation, no resource creation, never waits; the
    /// caller guarantees the slot's previous list finished (its fence passed) or never ran. **No barrier names `res`.**
    /// On `Err` the list is closed and must not be spliced (errors are static: no allocation).
    pub fn record(&mut self, slot: usize, res: &ID3D12Resource, copy: bool, points: Option<&[[u32; 2]; SAMPLE_N]>) -> Result<(), VmError> {
        let Some(s) = self.slots.get(slot) else {
            return Err(VmError::D3d { call: "DepthReader::record(slot)", hr: E_INVALIDARG });
        };
        if !copy && points.is_none() {
            return Err(VmError::D3d { call: "DepthReader::record(nothing)", hr: E_INVALIDARG });
        }
        let mut consts = [0u32; READER_CONSTS];
        consts[0] = self.w;
        consts[1] = self.h;
        if let Some(pts) = points {
            for (i, p) in pts.iter().enumerate() {
                consts[4 + 2 * i] = p[0].min(self.w.saturating_sub(1));
                consts[5 + 2 * i] = p[1].min(self.h.saturating_sub(1));
            }
        }
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: SRV_PLANE0,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        let uav = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: 0, PlaneSlice: 0 } },
        };
        let (hw, hh) = half(self.w, self.h);
        // SAFETY: the slot's previous execution finished (caller), so its allocator may be reset and its descriptors
        // rewritten; the SRV is of the caller's live resource (pinned in this UiBegin and referenced by ER's own lists
        // queued behind ours); everything else is ours and outlives the GPU work (never released in-session).
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            let at_cpu = |k: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + k * self.inc };
            let at_gpu = |k: usize| D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu.ptr + (k * self.inc) as u64 };
            s.alloc.Reset().ctx("Reset(er depth allocator)")?;
            s.list.Reset(&s.alloc, if copy { &self.copy_pso } else { &self.sample_pso }).ctx("Reset(er depth list)")?;
            self.device.CreateShaderResourceView(res, Some(&srv), at_cpu(2 * slot));
            self.device.CreateUnorderedAccessView(&s.tex, None::<&ID3D12Resource>, Some(&uav), at_cpu(2 * slot + 1));
            s.list.SetDescriptorHeaps(&self.heaps);
            s.list.SetComputeRootSignature(&self.root_sig);
            s.list.SetComputeRoot32BitConstants(0, consts.len() as u32, consts.as_ptr() as *const _, 0);
            s.list.SetComputeRootDescriptorTable(1, at_gpu(2 * slot));
            s.list.SetComputeRootDescriptorTable(2, at_gpu(2 * slot + 1));
            s.list.SetComputeRootUnorderedAccessView(3, self.out.GetGPUVirtualAddress());
            if copy {
                s.list.ResourceBarrier(&[transition(&s.tex, COPY_REST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
                s.list.Dispatch(hw.div_ceil(8), hh.div_ceil(8), 1);
                s.list.ResourceBarrier(&[transition(&s.tex, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, COPY_REST)]);
            }
            if points.is_some() {
                if copy {
                    s.list.SetPipelineState(&self.sample_pso);
                }
                s.list.ResourceBarrier(&[transition(&self.out, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
                s.list.Dispatch(1, 1, 1);
                s.list.ResourceBarrier(&[transition(&self.out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                s.list.CopyBufferRegion(&self.rb, 0, &self.out, 0, OUT_BYTES);
                s.list.ResourceBarrier(&[transition(&self.out, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON)]);
            }
            s.list.Close().ctx("Close(er depth)")
        }
    }

    /// The 22 texels of the last executed sample (only after its fence value completed: the caller checks).
    pub fn read_sample(&self) -> [f32; SAMPLE_N] {
        let mut out = [0f32; SAMPLE_N];
        // SAFETY: the mapped readback buffer holds OUT_BYTES bytes; the GPU finished writing it (caller).
        unsafe { std::ptr::copy_nonoverlapping(self.rb_ptr.0 as *const u8, out.as_mut_ptr() as *mut u8, OUT_BYTES as usize) };
        out
    }

    /// The slot's list (vmtest: executed like hudhook's splice does).
    pub fn list(&self, slot: usize) -> Option<&ID3D12GraphicsCommandList> {
        self.slots.get(slot).map(|s| &s.list)
    }

    pub fn fence(&self) -> &ID3D12Fence {
        &self.fence
    }
}

// ---- the prime (Present, our depth) ---------------------------------------------------------------------------------------

/// What the prime of one frame reads: our copy texture (half resolution, in `0xC0`) of this frame and ER's camera.
pub struct PrimeIn<'a> {
    pub tex: &'a ID3D12Resource,
    /// ER's near / far (`far` <= `near`: an infinite far plane).
    pub near: f32,
    pub far: f32,
    pub bias_m: f32,
    pub bias_rel: f32,
}

pub struct DepthPrime {
    dev: ID3D12Device,
    rs: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    heap: ID3D12DescriptorHeap,
    heaps: [Option<ID3D12DescriptorHeap>; 1],
    inc: usize,
    num_frames: usize,
}

/// The prime's 8 root constants (ER near, far (0 = infinite), bias m, bias rel, our a, our b, half-res flag, 0).
pub fn prime_consts(near: f32, far: f32, bias_m: f32, bias_rel: f32) -> [u32; 8] {
    let (n, f) = (mw2er_core::er_depth_state::OUR_NEAR, mw2er_core::er_depth_state::OUR_FAR);
    let a = -n / (f - n);
    let b = n * f / (f - n);
    let far = if far.is_finite() && far > near { far } else { 0.0 };
    [near.to_bits(), far.to_bits(), bias_m.to_bits(), bias_rel.to_bits(), a.to_bits(), b.to_bits(), 1, 0]
}

impl DepthPrime {
    /// Root signature, the depth-only PSO (D32_FLOAT, ALWAYS, write all, no render target) and a shader-visible heap
    /// with one SRV per frame slot. Loader thread / vmtest only.
    pub fn new(dev: &ID3D12Device, num_frames: usize) -> Result<DepthPrime, VmError> {
        let range = D3D12_DESCRIPTOR_RANGE { RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV, NumDescriptors: 1, BaseShaderRegister: 0, RegisterSpace: 0, OffsetInDescriptorsFromTableStart: 0 };
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 8 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ];
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: params.len() as u32,
            pParameters: params.as_ptr(),
            NumStaticSamplers: 0,
            pStaticSamplers: std::ptr::null(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS
                | D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS
                | D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
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
            VS: D3D12_SHADER_BYTECODE { pShaderBytecode: PRIME_VS.as_ptr() as *const _, BytecodeLength: PRIME_VS.len() },
            PS: D3D12_SHADER_BYTECODE { pShaderBytecode: PRIME_PS.as_ptr() as *const _, BytecodeLength: PRIME_PS.len() },
            BlendState: D3D12_BLEND_DESC { AlphaToCoverageEnable: false.into(), IndependentBlendEnable: false.into(), RenderTarget: [rt; 8] },
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                FrontCounterClockwise: false.into(),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: false.into(),
                MultisampleEnable: false.into(),
                AntialiasedLineEnable: false.into(),
                ForcedSampleCount: 0,
                ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
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
        let pso = unsafe { dev.CreateGraphicsPipelineState(&pd) }.ctx("CreateGraphicsPipelineState(er depth prime)")?;
        // SAFETY: a valid descriptor-heap description.
        let heap: ID3D12DescriptorHeap = unsafe {
            dev.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: num_frames.max(1) as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(er depth prime)")?;
        // SAFETY: plain query.
        let inc = unsafe { dev.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) } as usize;
        let heaps = [Some(heap.clone())];
        Ok(DepthPrime { dev: dev.clone(), rs, pso, heap, heaps, inc, num_frames: num_frames.max(1) })
    }

    /// Records the prime into `list`: the caller has our depth (`dsv`) cleared and re-binds its target afterwards; the
    /// viewport / scissor are the target's (== ER's depth size). `slot` = the frame slot (its fence was waited on, so
    /// its SRV descriptor is free). Returns whether it recorded.
    pub(crate) fn record(&self, list: &ID3D12GraphicsCommandList, slot: usize, dsv: D3D12_CPU_DESCRIPTOR_HANDLE, p: &PrimeIn<'_>) -> bool {
        if slot >= self.num_frames || !(p.near > 0.0 && p.near.is_finite()) {
            return false;
        }
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        let c = prime_consts(p.near, p.far, p.bias_m, p.bias_rel);
        // SAFETY: recording into the caller's open list; the SRV slot of this frame slot is not read by an earlier list
        // still in flight (the caller waited on the slot's fence); `p.tex` is our copy texture in `0xC0`.
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            self.dev.CreateShaderResourceView(p.tex, Some(&srv), D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + slot * self.inc });
            list.OMSetRenderTargets(0, None, false, Some(&dsv));
            list.SetDescriptorHeaps(&self.heaps);
            list.SetGraphicsRootSignature(&self.rs);
            list.SetPipelineState(&self.pso);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, c.len() as u32, c.as_ptr() as *const _, 0);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu.ptr + (slot * self.inc) as u64 });
            list.DrawInstanced(3, 1, 0, 0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_er_depth_shaders_and_layouts() {
        for (b, h) in [(COPY_CS, COPY_CS_SHA256), (SAMPLE_CS, SAMPLE_CS_SHA256), (PRIME_VS, PRIME_VS_SHA256), (PRIME_PS, PRIME_PS_SHA256)] {
            assert!(b.starts_with(b"DXBC"));
            assert_eq!(mw2er_core::sha256::sha256_hex(b), h);
        }
        assert_eq!(std::mem::size_of::<D3D12_RESOURCE_BARRIER>(), 32);
        assert_eq!(SRV_PLANE0.0, 21);
        // the raw parse
        let t = RawBarrier { ty: 0, flags: 1, p0: 0x1234, w0: 0xFFFF_FFFF, w1: 0xC0, w2: 0x10, _pad: 0 };
        assert_eq!(parse_one(&t), Bar::Transition { res: 0x1234, sub: 0xFFFF_FFFF, before: 0xC0, after: 0x10, flags: 1 });
        let a = RawBarrier { ty: 1, flags: 0, p0: 0x10, w0: 0x5678, w1: 0x1, w2: 0, _pad: 0 };
        assert_eq!(parse_one(&a), Bar::Alias { before: 0x10, after: 0x1_0000_5678 });
        assert_eq!(parse_one(&RawBarrier { ty: 2, flags: 0, p0: 0x99, w0: 0, w1: 0, w2: 0, _pad: 0 }), Bar::Uav { res: 0x99 });
        assert_eq!(parse_one(&RawBarrier { ty: 7, flags: 0, p0: 0, w0: 0, w1: 0, w2: 0, _pad: 0 }), Bar::Other);
        // the prime's constants: our projection, an infinite far as 0, the half-res flag
        let c = prime_consts(0.1, 0.0, 0.03, 0.004);
        assert_eq!((f32::from_bits(c[0]), f32::from_bits(c[1]), c[6]), (0.1, 0.0, 1));
        let z = 7.0f32;
        assert!((f32::from_bits(c[4]) + f32::from_bits(c[5]) / z - mw2er_core::er_depth_state::our_ndc(z, 0.05, 3000.0)).abs() < 1e-6);
    }
}
