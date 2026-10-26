//! C4 (docs/C4_UI_DESIGN.md 3.3, 4.3): the early pass. The viewmodel, the world guns and the scope overlay recorded
//! into OUR OWN command lists on ER's render thread (inside ER's UiBegin), executed on ER's direct queue directly
//! ahead of the Scaleform list that holds ER's `0xC0 -> RENDER_TARGET` barrier on the UI target, so ER's UI draws
//! over the gun.
//!
//! - Three slots, each with its own allocator, list, RTV and constant sub-ring; a slot is reused only when the fence
//!   value of its last submitted list completed ([`EarlyPass::slot_values`] + `mw2er_core::c4::pick_slot`). The
//!   record never waits (`busy`).
//! - Its own root signatures (identical layouts), PSOs per (format, [`EarlyMode`]), D32 depth (cleared per record) and
//!   constant ring, separate from the Present path's: nothing here races hudhook's `frame_index` ring. The Present
//!   path's `Renderer::record_*` functions are not touched.
//! - The list barriers the target `0xC0 -> RENDER_TARGET` first and `RENDER_TARGET -> 0xC0` last, so the queue-order
//!   state sequence ER expects is unchanged; ER's depth/stencil is never bound.
//! - PSOs are never created on the render thread: [`EarlyPass::pso_builder`] + [`PsoBuilder::build`] run on a worker
//!   and [`EarlyPass::insert_psos`] hands them over (`pso_pending` until then).
//! - Retired depth buffers are released by fence value ([`EarlyPass::release_retired`]).

use mw2er_core::c4::SLOTS;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

use crate::{
    Ctx, DRAWS_PER_FRAME, DebugMode, FRAME_CB_BYTES, FrameParams, GpuAssets, LightParams, MAX_PALETTE, MAX_WORLD_DRAWS, Mapped,
    EarlyMode, OutputMode, Psos, Renderer, SLOT_BYTES, VmError, Variant, WORLD_LODS, WorldDraw, WorldFrame, create_buffer,
    create_overlay_pso_mode, create_overlay_root_signature, create_pso_mode, create_root_signature, create_solid_pso, create_solid_root_signature,
    heap_props, solid_write, transition,
};

/// `E_INVALIDARG`: a C4.1 record refused its input (no allocation on the render thread).
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

/// `D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE`: the UI target's state between UI
/// frames [V: UiBegin's own barrier source].
pub const UI_TARGET_STATE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(0xC0);

/// The early target: ER's UI target as UiBegin selected it (in [`UI_TARGET_STATE`]).
pub struct EarlyTarget<'a> {
    pub resource: &'a ID3D12Resource,
    pub format: DXGI_FORMAT,
    pub width: u32,
    pub height: u32,
    /// What the PSOs write: `Scene` (RGB only), or ER's UI layer in its alpha convention (C4's `ui_layer` is `Premul`;
    /// C4.1: the convention pinned per build or by the probe).
    pub mode: EarlyMode,
}

struct EarlyPsos {
    format: DXGI_FORMAT,
    mode: EarlyMode,
    gun: Psos,
    overlay: ID3D12PipelineState,
    /// C4.1: the canary texel (one constant colour, every channel written as is).
    canary: ID3D12PipelineState,
}

struct EarlyDepth {
    res: ID3D12Resource,
    w: u32,
    h: u32,
}

/// PSOs for one (format, mode), built off the render thread.
pub struct PsoSet {
    format: DXGI_FORMAT,
    mode: EarlyMode,
    opaque: ID3D12PipelineState,
    alpha: ID3D12PipelineState,
    overlay: ID3D12PipelineState,
    canary: ID3D12PipelineState,
}

/// What a worker needs to build PSOs without holding the pass (clones of immutable COM objects).
pub struct PsoBuilder {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    overlay_rs: ID3D12RootSignature,
    solid_rs: ID3D12RootSignature,
}

impl PsoBuilder {
    pub fn build(&self, format: DXGI_FORMAT, mode: EarlyMode) -> Result<PsoSet, VmError> {
        let v = Variant::default();
        Ok(PsoSet {
            format,
            mode,
            opaque: create_pso_mode(&self.device, &self.root_sig, format, v, false, mode)?,
            alpha: create_pso_mode(&self.device, &self.root_sig, format, v, true, mode)?,
            overlay: create_overlay_pso_mode(&self.device, &self.overlay_rs, format, mode)?,
            canary: create_solid_pso(&self.device, &self.solid_rs, format, solid_write())?,
        })
    }
}

pub struct EarlyPass {
    device: ID3D12Device,
    root_sig: ID3D12RootSignature,
    overlay_rs: ID3D12RootSignature,
    solid_rs: ID3D12RootSignature,
    allocs: Vec<ID3D12CommandAllocator>,
    lists: Vec<ID3D12GraphicsCommandList>,
    /// Signalled by hudhook right after our list (patch 0002's splice).
    fence: ID3D12Fence,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_inc: usize,
    dsv_heap: ID3D12DescriptorHeap,
    depth: Option<EarlyDepth>,
    retired: Vec<(u64, ID3D12Resource)>,
    ring: ID3D12Resource,
    ring_ptr: Mapped,
    psos: Vec<EarlyPsos>,
    /// Highest fence value reserved for a record (submitted or not): a retired depth buffer is released once the
    /// fence passes it (values only grow, so a cancelled value is passed by the next submitted one).
    reserved: u64,
}

impl EarlyPass {
    /// Lists (closed), allocators, the fence, the heaps and the ring on `device` (ER's device = the swap chain's).
    /// Loader thread.
    pub fn new(device: &ID3D12Device) -> Result<EarlyPass, VmError> {
        let root_sig = create_root_signature(device)?;
        let overlay_rs = create_overlay_root_signature(device)?;
        let solid_rs = create_solid_root_signature(device)?;
        let mut allocs = Vec::with_capacity(SLOTS);
        let mut lists = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            // SAFETY: plain COM object creation.
            let a: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ctx("CreateCommandAllocator(early)")?;
            // SAFETY: as above; a fresh list is open, closed at once so the first record can Reset it.
            let l: ID3D12GraphicsCommandList =
                unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &a, None::<&ID3D12PipelineState>) }.ctx("CreateCommandList(early)")?;
            // SAFETY: closing our own open list.
            unsafe { l.Close() }.ctx("Close(early, new)")?;
            allocs.push(a);
            lists.push(l);
        }
        // SAFETY: plain COM object creation.
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ctx("CreateFence(early)")?;
        // SAFETY: valid descriptor-heap descriptions.
        let rtv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: SLOTS as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(early RTV)")?;
        // SAFETY: as above.
        let dsv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }
        .ctx("CreateDescriptorHeap(early DSV)")?;
        // SAFETY: plain query.
        let rtv_inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) } as usize;
        let ring = create_buffer(device, D3D12_HEAP_TYPE_UPLOAD, (SLOT_BYTES * SLOTS * DRAWS_PER_FRAME) as u64, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        let mut p = std::ptr::null_mut();
        // SAFETY: an UPLOAD buffer may stay mapped for its lifetime; the CPU never reads it.
        unsafe { ring.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)) }.ctx("Map(early ring)")?;
        Ok(EarlyPass {
            device: device.clone(),
            root_sig,
            overlay_rs,
            solid_rs,
            allocs,
            lists,
            fence,
            rtv_heap,
            rtv_inc,
            dsv_heap,
            depth: None,
            retired: Vec::new(),
            ring,
            ring_ptr: Mapped(p as *mut u8),
            psos: Vec::new(),
            reserved: 0,
        })
    }

    /// Raw COM pointers for hudhook's ECL callback (it clones them into the splice; the pass keeps them alive).
    pub fn list_raw(&self, slot: usize) -> usize {
        self.lists.get(slot).map_or(0, |l| l.as_raw() as usize)
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

    /// The vtable pointer of one of our own D3D12 resources (the module check of ER's target compares against it).
    pub fn own_resource_vtable(&self) -> usize {
        // SAFETY: the first qword of a COM object is its vtable; the ring is alive.
        unsafe { *(self.ring.as_raw() as *const usize) }
    }

    /// `GetDeviceRemovedReason() != S_OK` (checked at Present: a trip).
    pub fn device_removed(&self) -> Option<i32> {
        // SAFETY: plain COM call.
        unsafe { self.device.GetDeviceRemovedReason() }.err().map(|e| e.code().0)
    }

    pub fn has_psos(&self, format: DXGI_FORMAT, mode: EarlyMode) -> bool {
        self.psos.iter().any(|p| p.format == format && p.mode == mode)
    }

    pub fn pso_builder(&self) -> PsoBuilder {
        PsoBuilder { device: self.device.clone(), root_sig: self.root_sig.clone(), overlay_rs: self.overlay_rs.clone(), solid_rs: self.solid_rs.clone() }
    }

    /// Hands over a set a worker built. The vector grows only here (worker / task thread, under the pass's lock); the
    /// render thread only searches it.
    pub fn insert_psos(&mut self, s: PsoSet) {
        if !self.has_psos(s.format, s.mode) {
            self.psos.push(EarlyPsos {
                format: s.format,
                mode: s.mode,
                gun: Psos { format: s.format, variant: Variant::default(), opaque: s.opaque, alpha: s.alpha },
                overlay: s.overlay,
                canary: s.canary,
            });
        }
    }

    /// C4.1: our depth buffer exists for `w` x `h` (the render thread then never creates one; design 3.5).
    pub fn has_depth(&self, w: u32, h: u32) -> bool {
        self.depth.as_ref().is_some_and(|d| d.w == w && d.h == h)
    }

    /// Releases the retired depth buffers whose last list completed; returns how many.
    pub fn release_retired(&mut self) -> usize {
        let done = self.completed();
        mw2er_core::c4::release_ready(&mut self.retired, done).len()
    }

    fn ensure_depth(&mut self, w: u32, h: u32) -> Result<bool, VmError> {
        if self.depth.as_ref().is_some_and(|d| d.w == w && d.h == h) {
            return Ok(false);
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
            self.device.CreateCommittedResource(&heap_props(D3D12_HEAP_TYPE_DEFAULT), D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(&clear), &mut r)
        }
        .ctx("CreateCommittedResource(early depth)")?;
        let r = r.ok_or(VmError::Invalid("early depth null".into()))?;
        // SAFETY: the DSV heap has one slot; DSV descriptors are consumed when OMSetRenderTargets is recorded, so
        // rewriting it does not affect lists recorded earlier (the old resource itself is retired below).
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
        if let Some(old) = self.depth.replace(EarlyDepth { res: r, w, h }) {
            // every list that used it has a value <= the last reserved one
            self.retired.push((self.reserved, old.res));
        }
        Ok(true)
    }

    /// Creates the depth buffer for `w` x `h` ahead of the first record (loader / worker thread; C4.1: the worker
    /// prepares every size the gate met, the render thread never creates one).
    pub fn prepare_depth(&mut self, w: u32, h: u32) -> Result<(), VmError> {
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err(VmError::Invalid(format!("early target {w}x{h}")));
        }
        self.ensure_depth(w, h).map(|_| ())
    }

    fn rtv(&self, slot: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: plain query; slot < SLOTS (checked by the callers).
        let h0 = unsafe { self.rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: h0.ptr + slot * self.rtv_inc }
    }

    /// Opens slot `slot` for a record into `t` that will signal fence `value` (the caller reserves monotonic values):
    /// Reset (the caller checked the slot's fence), our RTV of the target, the depth for its size, then the
    /// `0xC0 -> RENDER_TARGET` barrier. Returns true when the depth was (re)created on this thread (a one-off hitch the
    /// DLL logs). Never waits.
    pub fn begin(&mut self, slot: usize, value: u64, t: &EarlyTarget<'_>) -> Result<bool, VmError> {
        self.begin_inner(slot, value, t, true)
    }

    /// C4.1 (design 3.5): [`EarlyPass::begin`] that never creates a resource or allocates. The caller checked
    /// [`EarlyPass::has_depth`] and the PSOs; a miss here is an error (`VmError::D3d`, no allocation).
    pub fn begin_prepared(&mut self, slot: usize, value: u64, t: &EarlyTarget<'_>) -> Result<(), VmError> {
        self.begin_inner(slot, value, t, false).map(|_| ())
    }

    fn begin_inner(&mut self, slot: usize, value: u64, t: &EarlyTarget<'_>, create: bool) -> Result<bool, VmError> {
        if !create && (slot >= SLOTS || !self.has_depth(t.width, t.height) || !self.has_psos(t.format, t.mode)) {
            return Err(VmError::D3d { call: "EarlyPass::begin_prepared (slot / depth / PSOs not prepared)", hr: E_INVALIDARG });
        }
        if slot >= SLOTS {
            return Err(VmError::Invalid(format!("early slot {slot}")));
        }
        if t.width == 0 || t.height == 0 || t.width > 16384 || t.height > 16384 {
            return Err(VmError::Invalid(format!("early target {}x{}", t.width, t.height)));
        }
        if !self.has_psos(t.format, t.mode) {
            return Err(VmError::Invalid("early PSOs not prepared".into()));
        }
        // a depth buffer retired now is still used by lists up to the last reserved value
        let created = self.ensure_depth(t.width, t.height)?;
        self.reserved = self.reserved.max(value);
        let (alloc, list) = (&self.allocs[slot], &self.lists[slot]);
        // SAFETY: the slot's last list completed on the GPU (or was never executed): the allocator may be reset.
        unsafe {
            alloc.Reset().ctx("Reset(early allocator)")?;
            list.Reset(alloc, None::<&ID3D12PipelineState>).ctx("Reset(early list)")?;
        }
        let rtv = self.rtv(slot);
        let desc = D3D12_RENDER_TARGET_VIEW_DESC {
            Format: t.format,
            ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
            Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_RTV { MipSlice: 0, PlaneSlice: 0 } },
        };
        // SAFETY: an RTV of the caller's live single-sample 2D target into our own heap slot; descriptors are consumed
        // at record time, so rewriting the slot later does not affect this list.
        unsafe { self.device.CreateRenderTargetView(t.resource, Some(&desc), rtv) };
        // SAFETY: recording into our own open list.
        unsafe { list.ResourceBarrier(&[transition(t.resource, UI_TARGET_STATE, D3D12_RESOURCE_STATE_RENDER_TARGET)]) };
        Ok(created)
    }

    /// Closes slot `slot`: the `RENDER_TARGET -> 0xC0` barrier, then Close. After this the list may be spliced.
    pub fn finish(&mut self, slot: usize, t: &EarlyTarget<'_>) -> Result<(), VmError> {
        // C4.1: ER's render thread calls this every armed frame: an error that never allocates (`ok_or` evaluates its
        // argument even on success)
        let list = self.lists.get(slot).ok_or(VmError::D3d { call: "EarlyPass::finish (slot)", hr: E_INVALIDARG })?;
        // SAFETY: recording into our own open list, then closing it.
        unsafe {
            list.ResourceBarrier(&[transition(t.resource, D3D12_RESOURCE_STATE_RENDER_TARGET, UI_TARGET_STATE)]);
            list.Close().ctx("Close(early)")
        }
    }

    /// Closes a slot whose record failed after [`EarlyPass::begin`] (the list is never submitted; Close may fail too).
    pub fn abandon(&mut self, slot: usize) {
        if let Some(l) = self.lists.get(slot) {
            // SAFETY: closing our own list; an error only means it is in an error state (never submitted).
            let _ = unsafe { l.Close() };
        }
    }

    fn psos_for(&self, t: &EarlyTarget<'_>) -> Result<&EarlyPsos, VmError> {
        self.psos.iter().find(|p| p.format == t.format && p.mode == t.mode).ok_or(VmError::D3d { call: "early PSOs not prepared", hr: E_INVALIDARG })
    }

    /// C4.1 (design 3.6): the session canary into the open list of `slot` (between [`EarlyPass::begin_prepared`] and
    /// [`EarlyPass::finish`]): one texel at `xy` of the target, `color` written as is (alpha included; no blending,
    /// no encoding). No allocation.
    pub fn record_canary(&self, slot: usize, t: &EarlyTarget<'_>, xy: [u32; 2], color: [f32; 4]) -> Result<(), VmError> {
        if slot >= SLOTS || xy[0] >= t.width || xy[1] >= t.height {
            return Err(VmError::D3d { call: "EarlyPass::record_canary (slot / texel)", hr: E_INVALIDARG });
        }
        let psos = self.psos_for(t)?;
        let list = &self.lists[slot];
        let rtv = self.rtv(slot);
        let consts = color.map(f32::to_bits);
        // SAFETY: recording into our own open list; the target is ER's live layer (in RENDER_TARGET after begin's
        // barrier), the PSO and root signature are ours.
        unsafe {
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: xy[0] as i32, top: xy[1] as i32, right: xy[0] as i32 + 1, bottom: xy[1] as i32 + 1 }]);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.SetGraphicsRootSignature(&self.solid_rs);
            list.SetPipelineState(&psos.canary);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const _, 0);
            list.DrawInstanced(3, 1, 0, 0);
        }
        Ok(())
    }

    /// Viewport, scissor, our RTV + DSV (cleared), heaps, root signature, topology: everything the draws use.
    fn bind_common(&self, list: &ID3D12GraphicsCommandList, slot: usize, t: &EarlyTarget<'_>, srv_heap: &ID3D12DescriptorHeap) {
        let rtv = self.rtv(slot);
        // SAFETY: recording into our own open list; every object outlives the GPU work (fence-retired).
        unsafe {
            let dsv = self.dsv_heap.GetCPUDescriptorHandleForHeapStart();
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));
            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
            list.SetDescriptorHeaps(&[Some(srv_heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_sig);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
    }

    /// Writes one draw's constants into sub-slot `sub` of slot `slot`; returns the ring offset.
    fn put_consts(&self, slot: usize, sub: usize, fc: &[u32; FRAME_CB_BYTES / 4], palette: &[u8]) -> usize {
        let off = (slot * DRAWS_PER_FRAME + sub) * SLOT_BYTES;
        // SAFETY: the ring holds SLOT_BYTES * SLOTS * DRAWS_PER_FRAME bytes; slot < SLOTS, sub < DRAWS_PER_FRAME and the
        // palette is at most MAX_PALETTE * 48 bytes (checked by the callers); the GPU finished reading this slot (its
        // fence passed before begin()).
        unsafe {
            let dst = self.ring_ptr.0.add(off);
            std::ptr::copy_nonoverlapping(fc.as_ptr() as *const u8, dst, FRAME_CB_BYTES);
            std::ptr::copy_nonoverlapping(palette.as_ptr(), dst.add(FRAME_CB_BYTES), palette.len());
        }
        off
    }

    fn ring_va(&self) -> u64 {
        // SAFETY: plain query.
        unsafe { self.ring.GetGPUVirtualAddress() }
    }
}

/// The 256-byte frame constants (`FrameConsts` of viewmodel.hlsl), as `Renderer::record_gun` / `record_world` fill them.
fn frame_consts(view_proj: &mw2er_core::xform::Mat4, view: &mw2er_core::xform::Mat4, l: &LightParams, out: OutputMode, paper_white_nits: f32, debug: DebugMode) -> [u32; FRAME_CB_BYTES / 4] {
    let mut fc = [0u32; FRAME_CB_BYTES / 4];
    for r in 0..4 {
        for c in 0..4 {
            fc[r * 4 + c] = view_proj[r][c].to_bits();
            fc[16 + r * 4 + c] = view[r][c].to_bits();
        }
    }
    let n = mw2er_core::xform::v_normalize(l.dir_view);
    for (i, v) in [n[0], n[1], n[2], l.diffuse, l.ambient_sky[0], l.ambient_sky[1], l.ambient_sky[2], l.exposure, l.ambient_ground[0], l.ambient_ground[1], l.ambient_ground[2]]
        .into_iter()
        .enumerate()
    {
        fc[32 + i] = v.to_bits();
    }
    fc[43] = out as u32;
    fc[44] = paper_white_nits.to_bits();
    fc[45] = debug as u32;
    fc
}

/// The draw loop of one mesh (opaque then alpha items, the part mask, per-item SRV + root constants).
fn draw_items(list: &ID3D12GraphicsCommandList, gpu: &GpuAssets, psos: &Psos, srv_gpu: D3D12_GPU_DESCRIPTOR_HANDLE, srv_inc: u32, hide_parts: u32) {
    // SAFETY: recording into our own open list; the mesh and the SRV heap belong to the renderer (alive).
    unsafe {
        list.IASetVertexBuffers(0, Some(&[gpu.vbv]));
        list.IASetIndexBuffer(Some(&gpu.ibv));
        let mut cur_alpha = None;
        for d in &gpu.draws {
            if d.part < 32 && hide_parts & (1 << d.part) != 0 {
                continue;
            }
            if cur_alpha != Some(d.alpha) {
                list.SetPipelineState(if d.alpha { &psos.alpha } else { &psos.opaque });
                cur_alpha = Some(d.alpha);
            }
            list.SetGraphicsRootDescriptorTable(2, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu.ptr + d.srv as u64 * srv_inc as u64 });
            let rc = [d.part, d.alpha as u32, 0, 0];
            list.SetGraphicsRoot32BitConstants(3, 4, rc.as_ptr() as *const _, 0);
            list.DrawIndexedInstanced(d.count, 1, d.first, 0, 0);
        }
    }
}

impl Renderer {
    /// C4: creates the early pass on the renderer's device and its PSOs for (`format`, `mode`) (loader thread).
    pub fn create_early(&self, format: DXGI_FORMAT, mode: EarlyMode, width: u32, height: u32) -> Result<EarlyPass, VmError> {
        let mut e = EarlyPass::new(&self.device)?;
        let set = e.pso_builder().build(format, mode)?;
        e.insert_psos(set);
        if width > 0 && height > 0 {
            e.prepare_depth(width, height)?;
        }
        Ok(e)
    }

    /// C4: the world guns into the early list of `slot` (between [`EarlyPass::begin`] and [`EarlyPass::finish`]),
    /// the same draws as [`Renderer::record_world`] with the early pass's depth, constants and PSOs.
    pub fn record_world_early(&self, e: &EarlyPass, slot: usize, t: &EarlyTarget<'_>, draws: &[WorldDraw], f: &WorldFrame) -> Result<u32, VmError> {
        // C4.1: errors that never allocate (ER's render thread records this)
        if slot >= SLOTS {
            return Err(VmError::D3d { call: "record_world_early (slot)", hr: E_INVALIDARG });
        }
        if draws.is_empty() {
            return Ok(0);
        }
        let psos = e.psos_for(t)?;
        let list = &e.lists[slot];
        e.bind_common(list, slot, t, &self.srv_heap);
        // SAFETY: plain query.
        let srv_gpu = unsafe { self.srv_heap.GetGPUDescriptorHandleForHeapStart() };
        let ring_va = e.ring_va();
        let fc = frame_consts(&f.view_proj, &f.view, &f.light, f.out, f.paper_white_nits, DebugMode::None);
        let mut drawn = 0u32;
        for (k, d) in draws.iter().take(MAX_WORLD_DRAWS).enumerate() {
            let Some(gpu_i) = (0..=d.lod as usize).rev().map(|l| d.gun * WORLD_LODS + l).find(|i| self.world.get(*i).is_some_and(|g| g.is_some())) else {
                continue;
            };
            if !d.model.iter().all(|x| x.is_finite()) {
                continue;
            }
            let Some(gpu) = self.world.get(gpu_i).and_then(|g| g.as_ref()) else { continue };
            // the rigid mesh reads bone 0 only (every vertex: joint 0, weight 255)
            let palette = [d.model; 4];
            // SAFETY: plain bytes of 4 x Mat34 (48 bytes each).
            let pb = unsafe { std::slice::from_raw_parts(palette.as_ptr() as *const u8, palette.len() * 48) };
            let off = e.put_consts(slot, 1 + k, &fc, pb);
            // SAFETY: recording into our own open list.
            unsafe {
                list.SetGraphicsRootConstantBufferView(0, ring_va + off as u64);
                list.SetGraphicsRootConstantBufferView(1, ring_va + (off + FRAME_CB_BYTES) as u64);
            }
            draw_items(list, gpu, &psos.gun, srv_gpu, self.srv_inc, 0);
            drawn += 1;
        }
        Ok(drawn)
    }

    /// C4: the viewmodel into the early list of `slot` (as [`Renderer::record_gun`], no timestamps).
    pub fn record_gun_early(&self, e: &EarlyPass, slot: usize, t: &EarlyTarget<'_>, gun: usize, hide_parts: u32, p: &FrameParams<'_>) -> Result<(), VmError> {
        // C4.1: errors that never allocate (ER's render thread records this every gun frame; `ok_or` evaluates its
        // argument even on success)
        if slot >= SLOTS {
            return Err(VmError::D3d { call: "record_gun_early (slot)", hr: E_INVALIDARG });
        }
        let gpu = self.guns.get(gun).and_then(|g| g.as_ref()).ok_or(VmError::D3d { call: "record_gun_early (gun assets not uploaded)", hr: E_INVALIDARG })?;
        if p.palette.len() < gpu.bones || p.palette.len() > MAX_PALETTE {
            return Err(VmError::D3d { call: "record_gun_early (palette size)", hr: E_INVALIDARG });
        }
        let psos = e.psos_for(t)?;
        let list = &e.lists[slot];
        e.bind_common(list, slot, t, &self.srv_heap);
        let fc = frame_consts(&p.view_proj, &p.view, &p.light, p.out, p.paper_white_nits, p.debug);
        // SAFETY: plain bytes of the palette (Mat34 = 12 f32).
        let pb = unsafe { std::slice::from_raw_parts(p.palette.as_ptr() as *const u8, p.palette.len() * 48) };
        let off = e.put_consts(slot, 0, &fc, pb);
        let ring_va = e.ring_va();
        // SAFETY: plain query; recording into our own open list.
        let srv_gpu = unsafe { self.srv_heap.GetGPUDescriptorHandleForHeapStart() };
        unsafe {
            list.SetGraphicsRootConstantBufferView(0, ring_va + off as u64);
            list.SetGraphicsRootConstantBufferView(1, ring_va + (off + FRAME_CB_BYTES) as u64);
        }
        draw_items(list, gpu, &psos.gun, srv_gpu, self.srv_inc, hide_parts);
        Ok(())
    }

    /// C4: the scope overlay into the early list of `slot` (as [`Renderer::record_overlay`]).
    pub fn record_overlay_early(&self, e: &EarlyPass, slot: usize, t: &EarlyTarget<'_>, opacity: f32) -> Result<(), VmError> {
        // C4.1: errors that never allocate (ER's render thread records this every scope frame)
        if slot >= SLOTS {
            return Err(VmError::D3d { call: "record_overlay_early (slot)", hr: E_INVALIDARG });
        }
        let srv = self.overlay_srv.ok_or(VmError::D3d { call: "record_overlay_early (overlay not uploaded)", hr: E_INVALIDARG })?;
        let psos = e.psos_for(t)?;
        let list = &e.lists[slot];
        let rtv = e.rtv(slot);
        let consts = [(t.width as f32).to_bits(), (t.height as f32).to_bits(), opacity.clamp(0.0, 1.0).to_bits(), 0u32];
        // SAFETY: recording into our own open list; the overlay texture and the SRV heap belong to the renderer.
        unsafe {
            let srv_gpu = self.srv_heap.GetGPUDescriptorHandleForHeapStart();
            list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: t.width as f32, Height: t.height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
            list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: t.width as i32, bottom: t.height as i32 }]);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
            list.SetGraphicsRootSignature(&e.overlay_rs);
            list.SetPipelineState(&psos.overlay);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const _, 0);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu.ptr + srv as u64 * self.srv_inc as u64 });
            list.DrawInstanced(3, 1, 0, 0);
        }
        Ok(())
    }
}

/// The DXGI format of a `mw2er_core::c4` format number.
pub fn dxgi(f: u32) -> DXGI_FORMAT {
    DXGI_FORMAT(f as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_ring_early_layout() {
        // 3 slots x 17 sub-slots x 6400 B, every root CBV 256-aligned
        assert_eq!(SLOT_BYTES * SLOTS * DRAWS_PER_FRAME, 326_400);
        assert_eq!(UI_TARGET_STATE.0, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0);
        assert_eq!(dxgi(mw2er_core::c4::DXGI_R10G10B10A2_UNORM), DXGI_FORMAT_R10G10B10A2_UNORM);
        assert_eq!(dxgi(mw2er_core::c4::DXGI_B8G8R8A8_UNORM_SRGB), DXGI_FORMAT_B8G8R8A8_UNORM_SRGB);
        let fc = frame_consts(&[[1.0; 4]; 4], &[[2.0; 4]; 4], &LightParams::default(), OutputMode::Scrgb, 80.0, DebugMode::None);
        assert_eq!((fc[0], fc[16], fc[43], f32::from_bits(fc[44])), (1.0f32.to_bits(), 2.0f32.to_bits(), 2, 80.0));
    }
}
