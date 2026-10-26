//! C4 (docs/C4_UI_DESIGN.md): the pure half of drawing the MW2 viewmodel UNDER Elden Ring's UI.
//!
//! ER's UI is Scaleform GFx on its own D3D12 HAL. Its UI frame begins in ER's `UiBegin` (0x140d80900 on 2.7.2.0),
//! which records `0xC0 -> RENDER_TARGET` on the UI target into Scaleform's current list. The DLL (`ui_probe.rs`,
//! `ui_inject.rs`) hooks UiBegin / UiEnd and hudhook's ExecuteCommandLists (patch 0002) and records the gun into its
//! OWN list, which hudhook splices into the same ECL call directly ahead of that Scaleform list. Everything that can
//! be decided on plain numbers lives here, unit-tested:
//!
//! - [`parse_getter`] / [`chain_res`]: the "plain getter" test of `(sel+0x70)->vtbl[1]` and the pointer chain to the
//!   target resource (I3; we never call an ER virtual).
//! - [`format_rule`] / [`desc_ok`]: the accepted targets of design 4.1 (refuse anything else, never guess).
//! - [`Miss`] / [`gate`]: the per-frame preconditions of design 3.3 in their order, expected misses vs trips (5.1/5.2).
//! - the frame state machine ([`Kind`], [`pack`], [`begin_ok`], [`ecl_submit`], [`ui_end`], [`present`]): one `u64`
//!   the DLL keeps in an atomic and moves with compare-exchange, so a frame draws the gun exactly once: early when
//!   our list was spliced, else at Present (never both, never neither).
//! - [`find_armed`] / [`splice_calls`]: the splice index math; [`pick_slot`] / [`release_ready`]: the 3-slot fence ring.
//! - [`ProbeStats`] / [`verdict`] and the `UI probe` line formats (design 2.2 / 2.3, the in-DLL verdict).
//! - [`teardown_plan`]: the order of the unload / trip teardown (design S1).
//! - C4.1 (docs/C4_1_DESIGN.md 2, probe P41, read only): [`chain41`] / [`rtdata41`] (the pinned read-only chain to
//!   ER's UI layer and its cross-check; the getter is never called), [`Output41`] / [`blend41`], the thread / order
//!   helpers, [`conv_vote`] (the alpha convention), [`ProbeStats41`] / [`verdict41`] and the `UI probe41` formats.

// ---- the getter (I3) -------------------------------------------------------------------------------------------------

/// How the getter derives the object from `this`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetterKind {
    /// `mov rax, [rcx + off]; ret`
    Load,
    /// `lea rax, [rcx + off]; ret` (or `mov rax, rcx; ret` with off 0)
    Lea,
}

/// A plain getter: no call, no branch, no store; the object is `[this+off]` or `this+off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Getter {
    pub kind: GetterKind,
    pub off: u32,
}

/// Recognises the plain-getter byte forms (design 2.1): `48 8b 41 xx c3`, `48 8b 81 xx xx xx xx c3`, the two `lea`
/// forms `48 8d 41 xx c3` / `48 8d 81 xx xx xx xx c3`, and `48 8b c1 c3`. Anything else (a call, a branch, another
/// register, a store) is not plain: the probe reports `plain=0` and the injection never runs.
pub fn parse_getter(b: &[u8]) -> Option<Getter> {
    let kind = |op: u8| match op {
        0x8b => Some(GetterKind::Load),
        0x8d => Some(GetterKind::Lea),
        _ => None,
    };
    match b {
        [0x48, 0x8b, 0xc1, 0xc3, ..] => Some(Getter { kind: GetterKind::Lea, off: 0 }),
        [0x48, op, 0x41, d, 0xc3, ..] => {
            // disp8 is sign-extended; a negative offset is no getter of an embedded member
            if *d >= 0x80 {
                return None;
            }
            Some(Getter { kind: kind(*op)?, off: *d as u32 })
        }
        [0x48, op, 0x81, d0, d1, d2, d3, 0xc3, ..] => {
            let off = u32::from_le_bytes([*d0, *d1, *d2, *d3]);
            if off >= 0x1000_0000 {
                return None;
            }
            Some(Getter { kind: kind(*op)?, off })
        }
        _ => None,
    }
}

/// The colour resource of UiBegin's chain without calling the virtual (design 1.1 step 3): `obj = getter(this)`,
/// `res = [[obj + wrap] + res_off]`. `read` is a non-faulting qword read; a zero pointer anywhere is `None`.
pub fn chain_res(this: usize, g: Getter, wrap: usize, res_off: usize, read: &dyn Fn(usize) -> Option<usize>) -> Option<usize> {
    let nz = |p: usize| (p != 0).then_some(p);
    let obj = match g.kind {
        GetterKind::Load => nz(read(this.checked_add(g.off as usize)?)?)?,
        GetterKind::Lea => this.checked_add(g.off as usize)?,
    };
    let w = nz(read(obj.checked_add(wrap)?)?)?;
    nz(read(w.checked_add(res_off)?)?)
}

// ---- targets and formats (design 4.1) ---------------------------------------------------------------------------------

/// DXGI_FORMAT values of the table (and a few refused ones the tests name).
pub const DXGI_R16G16B16A16_FLOAT: u32 = 10;
pub const DXGI_R10G10B10A2_TYPELESS: u32 = 23;
pub const DXGI_R10G10B10A2_UNORM: u32 = 24;
pub const DXGI_R11G11B10_FLOAT: u32 = 26;
pub const DXGI_R8G8B8A8_TYPELESS: u32 = 27;
pub const DXGI_R8G8B8A8_UNORM: u32 = 28;
pub const DXGI_R8G8B8A8_UNORM_SRGB: u32 = 29;
pub const DXGI_B8G8R8A8_UNORM: u32 = 87;
pub const DXGI_B8G8R8A8_TYPELESS: u32 = 90;
pub const DXGI_B8G8R8A8_UNORM_SRGB: u32 = 91;
/// D3D12_RESOURCE_DIMENSION_TEXTURE2D and D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.
pub const DIM_TEXTURE2D: u32 = 3;
pub const FLAG_ALLOW_RENDER_TARGET: u32 = 1;

/// The `fmt=<n>:<NAME>` name of a DXGI format.
pub fn format_name(f: u32) -> &'static str {
    match f {
        DXGI_R16G16B16A16_FLOAT => "R16G16B16A16_FLOAT",
        DXGI_R10G10B10A2_TYPELESS => "R10G10B10A2_TYPELESS",
        DXGI_R10G10B10A2_UNORM => "R10G10B10A2_UNORM",
        DXGI_R11G11B10_FLOAT => "R11G11B10_FLOAT",
        DXGI_R8G8B8A8_TYPELESS => "R8G8B8A8_TYPELESS",
        DXGI_R8G8B8A8_UNORM => "R8G8B8A8_UNORM",
        DXGI_R8G8B8A8_UNORM_SRGB => "R8G8B8A8_UNORM_SRGB",
        DXGI_B8G8R8A8_UNORM => "B8G8R8A8_UNORM",
        DXGI_B8G8R8A8_TYPELESS => "B8G8R8A8_TYPELESS",
        DXGI_B8G8R8A8_UNORM_SRGB => "B8G8R8A8_UNORM_SRGB",
        _ => "OTHER",
    }
}

/// Which texture UiBegin selected (design 1.1 step 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiPath {
    /// `[S+0xF0]`, the finished scene image (SDR: Scaleform draws straight onto it).
    #[default]
    Scene,
    /// `[S+0x100]`, the UI layer ER composites over the HDR scene afterwards.
    Layer,
}

impl UiPath {
    pub fn label(self) -> &'static str {
        match self {
            UiPath::Scene => "scene",
            UiPath::Layer => "layer",
        }
    }
}

/// UiBegin's own choice: the layer when (`S+0x13c` or `S+0x13e`) and `[S+0x100] != 0`, else the scene.
pub fn select_path(b13c: u8, b13e: u8, layer: usize) -> UiPath {
    if (b13c != 0 || b13e != 0) && layer != 0 { UiPath::Layer } else { UiPath::Scene }
}

/// The shader output for the early target (from the target format and path, never from the swap chain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarlyOut {
    /// sRGB OETF into a UNORM target (as at Present for an SDR back buffer).
    Srgb,
    /// Linear (the RTV is `_SRGB`: the hardware encodes).
    Linear,
}

/// What the early PSO does for a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FmtRule {
    pub out: EarlyOut,
    /// RGBA with A = 1 (the `ui_layer` PSO variant); `false` = RGB only (the scene's alpha is left alone).
    pub write_alpha: bool,
}

/// Design 4.1: the accepted target formats. `ui_layer` = `[ui] inject_ui_layer` (the HDR path is refused without it).
pub fn format_rule(fmt: u32, path: UiPath, ui_layer: bool) -> Result<FmtRule, Miss> {
    let unorm = matches!(fmt, DXGI_R10G10B10A2_UNORM | DXGI_R8G8B8A8_UNORM | DXGI_B8G8R8A8_UNORM);
    let srgb = matches!(fmt, DXGI_R8G8B8A8_UNORM_SRGB | DXGI_B8G8R8A8_UNORM_SRGB);
    match path {
        UiPath::Scene if unorm => Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: false }),
        UiPath::Scene if srgb => Ok(FmtRule { out: EarlyOut::Linear, write_alpha: false }),
        UiPath::Layer if !ui_layer => Err(Miss::HdrOff),
        UiPath::Layer if unorm => Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: true }),
        _ => Err(Miss::Format),
    }
}

/// The parts of a `D3D12_RESOURCE_DESC` the probe logs and the gate checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TexDesc {
    pub dimension: u32,
    pub width: u64,
    pub height: u32,
    pub array: u16,
    pub mips: u16,
    pub format: u32,
    pub samples: u32,
    pub flags: u32,
}

/// `rt_ok`: a single-sample 2D texture, one array slice, allowing render targets, of a drawable size (I4).
pub fn desc_ok(d: &TexDesc) -> bool {
    d.dimension == DIM_TEXTURE2D
        && d.array == 1
        && d.samples == 1
        && d.flags & FLAG_ALLOW_RENDER_TARGET != 0
        && (1..=16384).contains(&d.width)
        && (1..=16384).contains(&d.height)
}

/// The target check of the gate: the desc first, then the format table.
pub fn target_rule(d: &TexDesc, path: UiPath, ui_layer: bool) -> Result<FmtRule, Miss> {
    if !desc_ok(d) {
        return Err(Miss::Format);
    }
    format_rule(d.format, path, ui_layer)
}

// ---- misses and trips (design 5.1 / 5.2) --------------------------------------------------------------------------------

/// Why a frame did not draw early. The expected ones only count (that frame draws at Present); a trip turns the
/// injection off for the session (`UI inject trip why=..`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    // expected (5.1)
    Config,
    Build,
    NoHal,
    NoMovies,
    Flags,
    NotReady,
    Ac130,
    Hidden,
    NotFp,
    ScopedNoOverlay,
    SecondBegin,
    NoTarget,
    HdrOff,
    Format,
    Busy,
    PsoPending,
    Locked,
    Cancelled,
    Split,
    // unexpected (5.2): trips
    Vtable,
    Thread,
    Getter,
    Module,
    Device,
    FormatFlap,
    Hresult,
    Panic,
    NeverSubmitted,
    DeviceRemoved,
    // C4.1 (docs/C4_1_DESIGN.md 3.3 / 4), expected
    /// ER runs single-threaded (`byte [G+0x11c] == 0`): no dedicated render thread.
    StMode,
    /// This UiBegin read the previous UiBegin's PRESENT seq (it ran ahead of Present's increment).
    Ahead,
    /// The layer resource changed (or the rtData cross-check failed): a new one must pass two frames in a row.
    Recreated,
    /// ER renders below 1.0 render scale (the `_UPSAMPLE_` composite), or its composite shaders are not ready.
    Scale,
    /// The layer's size is not the back buffer's, or our depth for it is not prepared yet (the worker does that).
    Size,
    /// The layer's alpha convention is not pinned (per build) nor decided by the probe.
    AlphaUnknown,
    /// The session canary has not passed yet (a canary frame, or its readback in flight).
    Canary,
    /// The first UiBegin after going live records nothing.
    Warmup,
    // C4.1 trips
    /// A pin of the read-only chain broke after a good frame.
    Pins,
    /// A Present callback inside an open UI frame whose early list was already spliced (J2 broke).
    Order,
    /// The convention re-check disagreed with the pinned convention twice in a row.
    Alpha,
    /// The canary written at UiBegin was gone after UiEnd: something clears the layer after UiBegin (J6).
    Cleared,
}

impl Miss {
    pub const ALL: [Miss; 41] = [
        Miss::Config,
        Miss::Build,
        Miss::NoHal,
        Miss::NoMovies,
        Miss::Flags,
        Miss::NotReady,
        Miss::Ac130,
        Miss::Hidden,
        Miss::NotFp,
        Miss::ScopedNoOverlay,
        Miss::SecondBegin,
        Miss::NoTarget,
        Miss::HdrOff,
        Miss::Format,
        Miss::Busy,
        Miss::PsoPending,
        Miss::Locked,
        Miss::Cancelled,
        Miss::Split,
        Miss::Vtable,
        Miss::Thread,
        Miss::Getter,
        Miss::Module,
        Miss::Device,
        Miss::FormatFlap,
        Miss::Hresult,
        Miss::Panic,
        Miss::NeverSubmitted,
        Miss::DeviceRemoved,
        Miss::StMode,
        Miss::Ahead,
        Miss::Recreated,
        Miss::Scale,
        Miss::Size,
        Miss::AlphaUnknown,
        Miss::Canary,
        Miss::Warmup,
        Miss::Pins,
        Miss::Order,
        Miss::Alpha,
        Miss::Cleared,
    ];

    /// The `miss=<why:n,...>` / `trip why=` name.
    pub fn label(self) -> &'static str {
        match self {
            Miss::Config => "config",
            Miss::Build => "build",
            Miss::NoHal => "nohal",
            Miss::NoMovies => "nomovies",
            Miss::Flags => "flags",
            Miss::NotReady => "notready",
            Miss::Ac130 => "ac130",
            Miss::Hidden => "hidden",
            Miss::NotFp => "notfp",
            Miss::ScopedNoOverlay => "scoped_no_overlay",
            Miss::SecondBegin => "second_begin",
            Miss::NoTarget => "notarget",
            Miss::HdrOff => "hdr_off",
            Miss::Format => "format",
            Miss::Busy => "busy",
            Miss::PsoPending => "pso_pending",
            Miss::Locked => "locked",
            Miss::Cancelled => "cancelled",
            Miss::Split => "split",
            Miss::Vtable => "vtable",
            Miss::Thread => "thread",
            Miss::Getter => "getter",
            Miss::Module => "module",
            Miss::Device => "device",
            Miss::FormatFlap => "format_flap",
            Miss::Hresult => "hresult",
            Miss::Panic => "panic",
            Miss::NeverSubmitted => "never_submitted",
            Miss::DeviceRemoved => "device_removed",
            Miss::StMode => "st_mode",
            Miss::Ahead => "ahead",
            Miss::Recreated => "recreated",
            Miss::Scale => "scale",
            Miss::Size => "size",
            Miss::AlphaUnknown => "alpha_unknown",
            Miss::Canary => "canary",
            Miss::Warmup => "warmup",
            Miss::Pins => "pins",
            Miss::Order => "order",
            Miss::Alpha => "alpha",
            Miss::Cleared => "cleared",
        }
    }

    pub fn is_trip(self) -> bool {
        matches!(
            self,
            Miss::Vtable
                | Miss::Thread
                | Miss::Getter
                | Miss::Module
                | Miss::Device
                | Miss::FormatFlap
                | Miss::Hresult
                | Miss::Panic
                | Miss::NeverSubmitted
                | Miss::DeviceRemoved
                | Miss::Pins
                | Miss::Order
                | Miss::Alpha
                | Miss::Cleared
        )
    }

    /// Index into [`Miss::ALL`] (the DLL's per-reason counters).
    pub fn index(self) -> usize {
        Miss::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

/// What the Present path would do with the gun this frame (`vm_wants_draw` of the snapshot, factored out of
/// viewmodel.rs so both paths decide the same way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wants {
    /// The gun (and the world guns before it).
    Gun,
    /// Scoped: the overlay instead of the gun.
    Overlay,
    /// Nothing to draw this frame (menu, MW2 mode off, hidden, stale snapshot).
    Nothing,
    /// The AC-130 owns the frame (thermal at Present).
    Ac130,
    /// Third person without `draw_in_third_person`.
    NotFp,
    /// Scoped but the overlay is not loaded (Present then also draws nothing).
    ScopedNoOverlay,
    /// The renderer / assets are not loaded (or the viewmodel is disabled).
    NotReady,
}

/// Every input of the UiBegin gate (design 3.3 step 2), read by the detour after calling the original.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateIn {
    /// `[ui] inject_viewmodel` is live (not hot-disabled) and the feature has not tripped.
    pub enabled: bool,
    /// The build has `ScaleformUi` addresses and the hooks are installed.
    pub build: bool,
    pub hal: usize,
    pub movies: u64,
    /// `[hal] == base + fs_hal_vtable`.
    pub vt_ok: bool,
    /// `[hal+0x58]`.
    pub hal_flags: u32,
    pub tid: u32,
    pub hal_tid: u32,
    /// hudhook's present thread (patch 0002; 0 = no Present seen yet).
    pub present_tid: u32,
    /// A thread mismatch trips once the probe verdict was ready (else it is an expected miss).
    pub thread_trips: bool,
    /// This is the first UiBegin of this Present ([`begin_ok`]).
    pub first: bool,
    pub wants: Wants,
    /// The getter bytes equal the ones pinned at the first good frame (and are plain).
    pub getter_ok: bool,
    /// The chain gave a non-zero resource.
    pub chain_ok: bool,
    /// The resource's vtable lies in the module of our own D3D12 objects.
    pub module_ok: bool,
    /// The target check ([`target_rule`]).
    pub target: Result<FmtRule, Miss>,
    /// `[hal+0x1410]` is our device (pointer equality).
    pub device_ok: bool,
    /// An EarlyPass slot is free (its fence passed).
    pub slot_free: bool,
    /// The PSOs for this format / variant exist.
    pub pso_ready: bool,
}

/// HAL flags: 1 ModeSet, 2 InFrame (BeginFrame ran).
pub const HAL_FLAGS_FRAME: u32 = 3;

/// The gate in the design's order; `Ok` = record the early list this frame.
pub fn gate(g: &GateIn) -> Result<FmtRule, Miss> {
    if !g.enabled {
        return Err(Miss::Config);
    }
    if !g.build {
        return Err(Miss::Build);
    }
    if g.hal == 0 {
        return Err(Miss::NoHal);
    }
    if g.movies == 0 {
        return Err(Miss::NoMovies);
    }
    if !g.vt_ok {
        return Err(Miss::Vtable);
    }
    if g.hal_flags & HAL_FLAGS_FRAME != HAL_FLAGS_FRAME {
        return Err(Miss::Flags);
    }
    if g.tid != g.hal_tid || g.present_tid == 0 || g.tid != g.present_tid {
        return Err(if g.thread_trips && g.present_tid != 0 { Miss::Thread } else { Miss::NotReady });
    }
    if !g.first {
        return Err(Miss::SecondBegin);
    }
    match g.wants {
        Wants::Gun | Wants::Overlay => {}
        Wants::Nothing => return Err(Miss::Hidden),
        Wants::Ac130 => return Err(Miss::Ac130),
        Wants::NotFp => return Err(Miss::NotFp),
        Wants::ScopedNoOverlay => return Err(Miss::ScopedNoOverlay),
        Wants::NotReady => return Err(Miss::NotReady),
    }
    if !g.getter_ok {
        return Err(Miss::Getter);
    }
    if !g.chain_ok {
        return Err(Miss::NoTarget);
    }
    if !g.module_ok {
        return Err(Miss::Module);
    }
    let rule = g.target?;
    if !g.device_ok {
        return Err(Miss::Device);
    }
    if !g.slot_free {
        return Err(Miss::Busy);
    }
    if !g.pso_ready {
        return Err(Miss::PsoPending);
    }
    Ok(rule)
}

/// A refused format that appears 3 times in 10 s after a good one trips (`format_flap`); this counts the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlapWindow {
    times: Vec<f64>,
}

impl FlapWindow {
    pub const fn new() -> Self {
        FlapWindow { times: Vec::new() }
    }

    /// A desc change to a refused target at `now`; true = trip.
    pub fn refused_change(&mut self, now: f64) -> bool {
        self.times.retain(|t| now - *t < 10.0);
        self.times.push(now);
        self.times.len() >= 3
    }
}

// ---- the frame state machine (design 3.3) ----------------------------------------------------------------------------

/// Per-Present state of the early draw. Encoded in one `u64` (kind bits 0-2, slot bits 3-4, seq bits 5..) so the
/// UiBegin detour, the ECL callback (any thread) and the Present callback move it with compare-exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Nothing happened this Present yet.
    Idle = 0,
    /// Our list is recorded and waits for the Scaleform list it goes ahead of.
    Armed = 1,
    /// Our list was spliced (the gun is drawn under the UI this frame).
    Submitted = 2,
    /// Armed, but the UI ended (or Present came) before the Scaleform list was submitted: never executed.
    Cancelled = 3,
    /// Present's callback consumed the frame (nothing may arm or submit for this seq any more).
    Closed = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub slot: u8,
    pub seq: u64,
}

const SEQ_MASK: u64 = (1 << 59) - 1;

pub fn pack(kind: Kind, slot: u8, seq: u64) -> u64 {
    (kind as u64) | ((slot as u64 & 3) << 3) | ((seq & SEQ_MASK) << 5)
}

pub fn unpack(v: u64) -> Frame {
    let kind = match v & 7 {
        1 => Kind::Armed,
        2 => Kind::Submitted,
        3 => Kind::Cancelled,
        4 => Kind::Closed,
        _ => Kind::Idle,
    };
    Frame { kind, slot: ((v >> 3) & 3) as u8, seq: v >> 5 }
}

/// UiBegin may arm for `seq` (hudhook's PRESENT_SEQ): the state belongs to an older Present, or this Present has not
/// been touched. A second UiBegin of the same Present never arms (`second_begin`).
pub fn begin_ok(cur: u64, seq: u64) -> bool {
    let f = unpack(cur);
    f.seq != (seq & SEQ_MASK) || f.kind == Kind::Idle
}

/// The state after recording into `slot`.
pub fn armed(seq: u64, slot: u8) -> u64 {
    pack(Kind::Armed, slot, seq)
}

/// The ECL callback found the armed Scaleform list: Armed of this Present -> Submitted (`None` = no splice).
pub fn ecl_submit(cur: u64, seq: u64) -> Option<u64> {
    let f = unpack(cur);
    (f.kind == Kind::Armed && f.seq == seq & SEQ_MASK).then(|| pack(Kind::Submitted, f.slot, f.seq))
}

/// UiEnd: Armed of this Present -> Cancelled (its list never runs; the slot stays free).
pub fn ui_end(cur: u64, seq: u64) -> Option<u64> {
    let f = unpack(cur);
    (f.kind == Kind::Armed && f.seq == seq & SEQ_MASK).then(|| pack(Kind::Cancelled, f.slot, f.seq))
}

/// What Present's callback does this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentDecision {
    /// The early list ran this frame: skip the world guns, the gun and the overlay at Present.
    pub early: bool,
    /// The frame was still Armed (cancelled here).
    pub cancelled: bool,
}

/// Present's callback: decides and closes the frame (the next state is `Closed` for `seq`).
pub fn present(cur: u64, seq: u64) -> (PresentDecision, u64) {
    let f = unpack(cur);
    let mine = f.seq == seq & SEQ_MASK;
    let d = PresentDecision { early: mine && f.kind == Kind::Submitted, cancelled: mine && f.kind == Kind::Armed };
    (d, pack(Kind::Closed, 0, seq))
}

/// "Armed but not submitted 30 frames in a row" (I5 broke) trips; this counts the streak.
pub const NEVER_SUBMITTED_TRIP: u32 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Streak {
    pub cancelled_in_row: u32,
}

impl Streak {
    /// One armed frame's outcome; true = trip.
    pub fn armed_outcome(&mut self, submitted: bool) -> bool {
        if submitted {
            self.cancelled_in_row = 0;
            false
        } else {
            self.cancelled_in_row += 1;
            self.cancelled_in_row >= NEVER_SUBMITTED_TRIP
        }
    }
}

// ---- the splice (design 3.2) ------------------------------------------------------------------------------------------

/// The index of the armed Scaleform list among an ECL call's lists (`armed == 0` never matches).
pub fn find_armed(lists: &[usize], armed: usize) -> Option<usize> {
    if armed == 0 {
        return None;
    }
    lists.iter().position(|&l| l == armed)
}

/// The three trampoline calls of a splice at `at` in `n` lists: `(0, at)` (skipped when `at == 0`), ours, then
/// `(at, n - at)`. `None` when `at` is outside the call.
pub fn splice_calls(n: usize, at: usize) -> Option<((usize, usize), (usize, usize))> {
    (at < n).then_some(((0, at), (at, n - at)))
}

/// C4 v1 splices only when the armed list is the FIRST list of its ECL call (`at == 0`): then no ER list is moved
/// into a separate ExecuteCommandLists call (resource-state decay happens between ECL calls). Scaleform's
/// SubmitPending submits one list per call [V]; any other shape is a `split` miss (the probe's `armed_idx_max`).
pub fn splice_allowed(at: usize) -> bool {
    at == 0
}

// ---- the fence ring (design 4.3) --------------------------------------------------------------------------------------

/// EarlyPass slots (allocator + list + constants + RTV each).
pub const SLOTS: usize = 3;

/// The next reusable slot after `last` (round robin): its last submitted fence value has completed. Never waits:
/// `None` = all three in flight (`busy`).
pub fn pick_slot(values: &[u64; SLOTS], completed: u64, last: usize) -> Option<usize> {
    (1..=SLOTS).map(|d| (last + d) % SLOTS).find(|&k| values[k] <= completed)
}

/// Removes and returns the retired items whose fence value has completed (the rest stay).
pub fn release_ready<T>(retired: &mut Vec<(u64, T)>, completed: u64) -> Vec<T> {
    let mut out = Vec::new();
    let mut keep = Vec::with_capacity(retired.len());
    for (v, t) in retired.drain(..) {
        if v <= completed { out.push(t) } else { keep.push((v, t)) }
    }
    *retired = keep;
    out
}

// ---- the probe (design 2.2 / 2.3) -------------------------------------------------------------------------------------

/// Where an ECL call falls relative to this Present's UiBegin / UiEnd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Pre,
    Ui,
    Post,
}

/// The per-frame facts of one ECL call the probe counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EclCount {
    pub armed_hit: bool,
    pub armed_first: bool,
    pub between_other: bool,
    pub late: bool,
}

/// One ECL call: `phase`, whether it is on the HAL's queue, whether it holds this frame's armed list, whether the
/// armed list was already seen this frame, and whether another hal-queue ECL came after UiBegin before it.
pub fn count_ecl(phase: Phase, halq: bool, has_armed: bool, armed_seen: bool, other_since_begin: bool) -> EclCount {
    let mut c = EclCount::default();
    if !halq {
        return c;
    }
    match (phase, has_armed, armed_seen) {
        (Phase::Ui, true, false) => {
            c.armed_hit = true;
            c.armed_first = !other_since_begin;
        }
        (Phase::Ui, false, false) => c.between_other = true,
        (Phase::Pre | Phase::Post, true, false) => c.late = true,
        _ => {}
    }
    c
}

/// The probe's totals (atomics in the DLL) as the verdict reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeStats {
    pub hook_begin: bool,
    pub hook_end: bool,
    pub ecl_hook: bool,
    pub skips: u32,
    /// hal lines: seen at all, and every one with vt_ok=1 / dev_eq=1.
    pub hal_seen: bool,
    pub hal_vt_bad: u32,
    pub hal_dev_bad: u32,
    /// `same_thread` false at least once.
    pub thread_bad: u32,
    pub begins: u64,
    pub begin_tid_ok: u64,
    /// 5 s windows with UiBegin per Present outside 0..=1, and windows in total.
    pub per_present_bad_windows: u32,
    pub windows: u32,
    pub getter_seen: bool,
    pub getter_plain: bool,
    pub getter_changes: u32,
    pub target_seen: bool,
    pub target_bad: u32,
    pub armed: u64,
    pub armed_hit: u64,
    pub armed_first: u64,
    pub between_other: u64,
    pub late: u64,
}

/// The in-DLL verdict (design 2.3): every row PASS -> ready. `why` names the failing rows.
pub fn verdict(s: &ProbeStats) -> (bool, Vec<&'static str>) {
    let mut why = Vec::new();
    if !(s.hook_begin && s.hook_end && s.ecl_hook && s.skips == 0) {
        why.push("hooks");
    }
    if !s.hal_seen || s.hal_vt_bad > 0 || s.hal_dev_bad > 0 {
        why.push("hal");
    }
    let per_ok = s.windows > 0 && (s.per_present_bad_windows as u64) * 100 <= s.windows as u64;
    if s.begins == 0 || s.thread_bad > 0 || s.begin_tid_ok != s.begins || !per_ok {
        why.push("thread");
    }
    if !s.getter_seen || !s.getter_plain || s.getter_changes > 0 {
        why.push("getter");
    }
    if !s.target_seen || s.target_bad > 0 {
        why.push("target");
    }
    if s.armed == 0 || s.armed_hit != s.armed || s.armed_first != s.armed || s.between_other != 0 || s.late != 0 {
        why.push("adjacency");
    }
    (why.is_empty(), why)
}

/// `UI probe verdict ready=0|1 why=<comma list or none>`.
pub fn fmt_verdict(s: &ProbeStats) -> String {
    let (ready, why) = verdict(s);
    format!("probe verdict ready={} why={}", ready as u8, if why.is_empty() { "none".to_string() } else { why.join(",") })
}

/// `UI probe hal vt=0x.. vt_ok=0|1 dev_eq=0|1 queue_eq_present=0|1 rtid=<n> present_tid=<n> same_thread=0|1`.
pub fn fmt_hal(vt: usize, vt_ok: bool, dev_eq: bool, queue_eq_present: bool, rtid: u32, present_tid: u32) -> String {
    format!(
        "probe hal vt=0x{vt:x} vt_ok={} dev_eq={} queue_eq_present={} rtid={rtid} present_tid={present_tid} same_thread={}",
        vt_ok as u8,
        dev_eq as u8,
        queue_eq_present as u8,
        (rtid != 0 && rtid == present_tid) as u8
    )
}

/// `UI probe mode b13c=0|1 b13e=0|1 layer=0|1 sel=scene|layer`.
pub fn fmt_mode(b13c: u8, b13e: u8, layer: usize) -> String {
    format!(
        "probe mode b13c={} b13e={} layer={} sel={}",
        (b13c != 0) as u8,
        (b13e != 0) as u8,
        (layer != 0) as u8,
        select_path(b13c, b13e, layer).label()
    )
}

/// `UI probe getter addr=0x.. bytes=<hex16> plain=0|1 off=0x..`.
pub fn fmt_getter(addr: usize, bytes: &[u8]) -> String {
    let g = parse_getter(bytes);
    format!(
        "probe getter addr=0x{addr:x} bytes={} plain={} off=0x{:x}",
        hex(bytes),
        g.is_some() as u8,
        g.map_or(0, |g| g.off)
    )
}

/// `UI probe rt which=scene|layer res=0x.. fmt=<n>:<NAME> w=<n> h=<n> dim=<n> mips=<n> arr=<n> samples=<n> flags=0x.. rt_ok=0|1 bb=<w>x<h>:<NAME> distinct=<k>`.
pub fn fmt_rt(which: UiPath, res: usize, d: &TexDesc, bb: (u32, u32, u32), distinct: u32) -> String {
    format!(
        "probe rt which={} res=0x{res:x} fmt={}:{} w={} h={} dim={} mips={} arr={} samples={} flags=0x{:x} rt_ok={} bb={}x{}:{} distinct={distinct}",
        which.label(),
        d.format,
        format_name(d.format),
        d.width,
        d.height,
        d.dimension,
        d.mips,
        d.array,
        d.samples,
        d.flags,
        desc_ok(d) as u8,
        bb.0,
        bb.1,
        format_name(bb.2)
    )
}

/// `UI probe frame present=<n> ui_begin=<n> ui_end=<n> nomovies=<n> per_present=<min>-<max> begin_tid_ok=<n>/<n>`.
pub fn fmt_frame(present: u64, ui_begin: u64, ui_end: u64, nomovies: u64, per_min: u32, per_max: u32, tid_ok: u64) -> String {
    format!("probe frame present={present} ui_begin={ui_begin} ui_end={ui_end} nomovies={nomovies} per_present={per_min}-{per_max} begin_tid_ok={tid_ok}/{ui_begin}")
}

/// The `UI probe ecl total=..` counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EclTotals {
    pub total: u64,
    pub halq: u64,
    pub otherq: u64,
    pub pre: u64,
    pub ui: u64,
    pub post: u64,
    pub armed: u64,
    pub armed_hit: u64,
    pub armed_first: u64,
    pub armed_idx_max: u64,
    pub armed_n_max: u64,
    pub armed_tid_ok: u64,
    pub between_other: u64,
    pub late: u64,
}

pub fn fmt_ecl(t: &EclTotals) -> String {
    format!(
        "probe ecl total={} halq={} otherq={} pre={} ui={} post={} armed={} armed_hit={} armed_first={} armed_idx_max={} armed_n_max={} armed_tid_ok={} between_other={} late={}",
        t.total,
        t.halq,
        t.otherq,
        t.pre,
        t.ui,
        t.post,
        t.armed,
        t.armed_hit,
        t.armed_first,
        t.armed_idx_max,
        t.armed_n_max,
        t.armed_tid_ok,
        t.between_other,
        t.late
    )
}

/// `UI inject early=<n> present=<n> splice=<n> cancelled=<n> miss=<why:n,...>` (only the non-zero reasons).
pub fn fmt_inject(early: u64, present: u64, splice: u64, cancelled: u64, misses: &[u64]) -> String {
    let m: Vec<String> = Miss::ALL.iter().zip(misses).filter(|(_, n)| **n > 0).map(|(k, n)| format!("{}:{n}", k.label())).collect();
    format!(
        "inject early={early} present={present} splice={splice} cancelled={cancelled} miss={}",
        if m.is_empty() { "none".to_string() } else { m.join(",") }
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join("")
}

// ---- teardown (design S1) ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeardownStep {
    /// `ACTIVE = false`: the detours and the ECL callback become passthrough.
    Deactivate,
    /// `MH_DisableHook` on every stored target (no new entries into our detours).
    Disable,
    /// Wait (1 s) until no thread is counted inside a detour. Review C4: there is no `MH_RemoveHook` step; the
    /// trampolines are never freed (a thread between a detour's entry and its in-flight increment is not counted).
    WaitInflight,
    /// Wait (2 s) on our fence.
    WaitFence,
    /// Release our D3D12 objects: only if the fence passed, else they are leaked (never freed while in use).
    Release,
}

/// The steps in order. `full` = the DLL is unloading (FreeLibrary); the hot-disable / trip path stops after the hooks
/// (the objects stay, so a slot can never be in use by a re-armed frame). The design's MH_RemoveHook is dropped: it
/// frees the trampoline a thread inside a detour may still call, and the in-flight count cannot see every such thread.
pub fn teardown_plan(full: bool, inflight_zero: bool, fence_done: bool) -> Vec<TeardownStep> {
    let mut v = vec![TeardownStep::Deactivate, TeardownStep::Disable, TeardownStep::WaitInflight];
    if full {
        v.push(TeardownStep::WaitFence);
        if fence_done && inflight_zero {
            v.push(TeardownStep::Release);
        }
    }
    v
}

// ---- C4.1 (docs/C4_1_DESIGN.md 2): the probe extension P41 -------------------------------------------------------------
//
// The same UiBegin / UiEnd detours and ECL callback as C4 (no new hook), read only: the render-thread rule, the pinned
// read-only chain to ER's UI layer (the getter is never called), the rtData cross-check, the composite's inputs, and
// the `UI probe41 ...` lines with their in-DLL verdict ([`verdict41`]).

/// The UI layer's alpha convention (design 0 row 4), pinned per build from a probe sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerAlpha {
    /// `out = scene*a + ui`: an empty layer pixel is (0,0,0,1).
    Transmit,
    /// `out = scene*(1-a) + ui`: an empty layer pixel is (0,0,0,0).
    Premul,
}

impl LayerAlpha {
    pub fn label(self) -> &'static str {
        match self {
            LayerAlpha::Transmit => "transmit",
            LayerAlpha::Premul => "premul",
        }
    }
}

/// `transmit|premul|unknown`.
pub fn alpha_label(a: Option<LayerAlpha>) -> &'static str {
    a.map_or("unknown", LayerAlpha::label)
}

/// A non-faulting little-endian read of `len` (1, 4 or 8) bytes at an address (`None` = unreadable).
pub type Rd<'a> = &'a dyn Fn(usize, usize) -> Option<u64>;

/// `[p + off]` as a pointer (0 for a zero base, an overflow or an unreadable qword).
fn q_at(rd: Rd, p: usize, off: usize) -> usize {
    if p == 0 {
        return 0;
    }
    p.checked_add(off).and_then(|a| rd(a, 8)).unwrap_or(0) as usize
}

fn n_at(rd: Rd, p: usize, off: usize, len: usize) -> u64 {
    if p == 0 {
        return 0;
    }
    p.checked_add(off).and_then(|a| rd(a, len)).unwrap_or(0)
}

/// One candidate's chain (design 1.2): the 7 pins, the image count and the resource UiBegin barriers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chain41 {
    /// `[sel] == base + sel_vt`
    pub vt: bool,
    /// `[[sel] + 0x38] == base + sel_vt_count`
    pub vt7: bool,
    /// `[sel + 0x70] == base + sub_vt`
    pub sub: bool,
    /// `[[sel + 0x70]] == base + sub_slot0`, `[[sel + 0x70] + 8] == base + sub_slot1`
    pub slot0: bool,
    pub slot1: bool,
    /// `T = [sel + 0x40] != 0`, `arr = [sel + 0x78] != 0`
    pub tex: bool,
    pub arr: bool,
    pub count: u32,
    pub obj: usize,
    /// `[[obj + 8] + 0x20]`: ID3D12Resource* (0 unless every pin holds and count >= 1).
    pub res: usize,
    /// `[obj + 0x10]`: ER's RTV handle.
    pub rtv: usize,
}

impl Chain41 {
    pub fn pins_ok(&self) -> bool {
        self.vt && self.vt7 && self.sub && self.slot0 && self.slot1 && self.tex && self.arr
    }

    pub fn ok(&self) -> bool {
        self.pins_ok() && self.count >= 1 && self.res != 0
    }

    /// The 7 pins as bits 0..7 (vt, vt7, sub, slot0, slot1, tex, arr), for one atomic.
    pub fn bits(&self) -> u8 {
        [self.vt, self.vt7, self.sub, self.slot0, self.slot1, self.tex, self.arr].iter().enumerate().fold(0, |b, (i, f)| b | (*f as u8) << i)
    }

    pub fn from_bits(bits: u8, count: u32, obj: usize, res: usize, rtv: usize) -> Chain41 {
        let f = |i: u8| bits >> i & 1 != 0;
        Chain41 { vt: f(0), vt7: f(1), sub: f(2), slot0: f(3), slot1: f(4), tex: f(5), arr: f(6), count, obj, res, rtv }
    }
}

/// Design 1.2: the read-only chain from a selected texture object (`sel` = `[S+0x100]` or `[S+0xF0]`). Mirrors what
/// ER's getter (`0x1419f4970` -> slot 0 `0x1419f49a0` -> primary slot 7 `0x1419f3dc0`) computes, with memory reads
/// only; nothing past a failed pin is followed to the resource.
pub fn chain41(sel: usize, base: usize, u: &crate::exe_build::Ui41, l: &crate::exe_build::UiLayout, rd: Rd) -> Chain41 {
    let mut c = Chain41::default();
    if sel == 0 {
        return c;
    }
    let pin = |v: usize, rva: usize| v != 0 && base.checked_add(rva) == Some(v);
    let vtp = q_at(rd, sel, 0);
    c.vt = pin(vtp, u.sel_vt);
    c.vt7 = c.vt && pin(q_at(rd, vtp, u.sel_count_slot), u.sel_vt_count);
    let subp = q_at(rd, sel, l.obj);
    c.sub = pin(subp, u.sub_vt);
    c.slot0 = c.sub && pin(q_at(rd, subp, 0), u.sub_slot0);
    c.slot1 = c.sub && pin(q_at(rd, subp, 8), u.sub_slot1);
    let t = q_at(rd, sel, u.sel_tex);
    let arr = q_at(rd, sel, u.sel_arr);
    c.tex = t != 0;
    c.arr = arr != 0;
    if c.tex {
        let d = q_at(rd, t, u.tex_desc);
        c.count = if d != 0 { n_at(rd, q_at(rd, d, u.desc_data), u.desc_count, 4) as u32 } else { n_at(rd, t, u.tex_count, 1) as u32 };
    }
    if c.pins_ok() && c.count >= 1 {
        c.obj = q_at(rd, arr, 0);
        c.rtv = q_at(rd, c.obj, u.obj_rtv);
        c.res = q_at(rd, q_at(rd, c.obj, l.wrap), l.res);
    }
    c
}

/// The HAL-init render target `[owner+0xDD0]` and its rtData (format, ER's RTV handle): a check, never a source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RtData41 {
    pub rt: usize,
    pub data: usize,
    pub fmt: u32,
    pub rtv: usize,
}

pub fn rtdata41(owner: usize, u: &crate::exe_build::Ui41, rd: Rd) -> RtData41 {
    let rt = q_at(rd, owner, u.owner_rt);
    let data = q_at(rd, rt, u.rt_data);
    RtData41 { rt, data, fmt: n_at(rd, data, u.rtdata_fmt, 4) as u32, rtv: q_at(rd, data, u.rtdata_rtv) }
}

/// rtData describes this chain's image: the same RTV handle, and (once a desc was sampled: `desc_fmt != 0`) the
/// same format. A mismatch means the target was recreated.
pub fn rtdata_match(c: &Chain41, r: &RtData41, desc_fmt: u32) -> bool {
    c.rtv != 0 && r.rtv == c.rtv && (desc_fmt == 0 || r.fmt == desc_fmt)
}

/// The composite's inputs (design 1.2, optB 2).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Output41 {
    pub b13c: u8,
    pub b13e: u8,
    pub eb9: u8,
    pub scale_x: f32,
    pub scale_y: f32,
    pub shaders: bool,
}

impl Output41 {
    /// ER's HDR output (`b13c`).
    pub fn hdr(&self) -> bool {
        self.b13c != 0
    }

    /// Render scale exactly 1.0 (no `_UPSAMPLE_` composite).
    pub fn scale_one(&self) -> bool {
        self.scale_x == 1.0 && self.scale_y == 1.0
    }
}

/// The composite variant `141afce70` selects: `sdr2sdr` (b13c = 0), `hdr` (`_BlendUI_F`), `sdr2hdr` (eb9 != 0), with
/// `_upsample` when either render scale is below 1.0.
pub fn blend41(o: &Output41) -> &'static str {
    let up = o.scale_x < 1.0 || o.scale_y < 1.0;
    match (o.b13c != 0, o.eb9 != 0, up) {
        (false, _, false) => "sdr2sdr",
        (false, _, true) => "sdr2sdr_upsample",
        (true, false, false) => "hdr",
        (true, false, true) => "hdr_upsample",
        (true, true, false) => "sdr2hdr",
        (true, true, true) => "sdr2hdr_upsample",
    }
}

/// `rt_ok`: UiBegin ran on ER's render thread, the one the HAL names (J1).
pub fn rt_ok41(begin_tid: u32, hal_tid: u32, rt_tid: u32) -> bool {
    begin_tid != 0 && begin_tid == hal_tid && hal_tid == rt_tid
}

/// A Present callback inside an open UI frame (UiBegin of this seq ran, its UiEnd not yet): J2 / J10.
pub fn present_in_ui41(begin_seq: u64, end_seq: u64, seq: u64) -> bool {
    begin_seq == seq && end_seq != seq
}

/// `ahead`: a UiBegin whose seq equals the previous UiBegin's (`u64::MAX` = none before).
pub fn ahead41(prev_seq: u64, seq: u64) -> bool {
    prev_seq != u64::MAX && prev_seq == seq
}

/// Desc w/h equal the back buffer's.
pub fn size_eq41(d: &TexDesc, bb: (u32, u32, u32)) -> bool {
    d.width == bb.0 as u64 && d.height == bb.1
}

/// The `layer` row of one line (J4): R8G8B8A8_UNORM, a drawable single-sample 1-mip RT, back-buffer sized, and every
/// rtData cross-check passed (`rtdata` = (ok, n), n > 0).
pub fn layer_ok41(d: &TexDesc, bb: (u32, u32, u32), rtdata: (u64, u64)) -> bool {
    d.format == DXGI_R8G8B8A8_UNORM && desc_ok(d) && d.mips == 1 && d.samples == 1 && size_eq41(d, bb) && rtdata.1 > 0 && rtdata.0 == rtdata.1
}

/// The `pins` row of one `pins which=layer` line (J3; the format is compared with the layer lines separately).
pub fn pins_line_ok41(c: &Chain41, res_mod: bool, r: &RtData41) -> bool {
    c.ok() && res_mod && c.rtv != 0 && r.rtv == c.rtv
}

/// The 5 s thread facts (`UI probe41 thread ..`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Thread41 {
    /// Every UiBegin so far read `byte [G+0x11c] != 0`.
    pub mt: bool,
    pub rt_tid: u32,
    pub hal_tid: u32,
    pub begin_tid: u32,
    pub present_tid: u32,
    /// UiBegins (with a HAL) on the render thread, of `begins`.
    pub rt_ok: u64,
    pub begins: u64,
    pub rtid_changes: u32,
    pub ahead: u64,
    pub present_in_ui: u64,
}

/// `UI probe41 on sample=0|1`.
pub fn fmt41_on(sample: bool) -> String {
    format!("probe41 on sample={}", sample as u8)
}

/// `UI probe41 pins which=.. vt= vt7= sub= slot0= slot1= tex= arr= count= res=0x.. res_mod= rtv=0x.. rtdata_fmt= rtdata_rtv_eq=`.
pub fn fmt41_pins(which: UiPath, c: &Chain41, res_mod: bool, r: &RtData41) -> String {
    format!(
        "probe41 pins which={} vt={} vt7={} sub={} slot0={} slot1={} tex={} arr={} count={} res=0x{:x} res_mod={} rtv=0x{:x} rtdata_fmt={} rtdata_rtv_eq={}",
        which.label(),
        c.vt as u8,
        c.vt7 as u8,
        c.sub as u8,
        c.slot0 as u8,
        c.slot1 as u8,
        c.tex as u8,
        c.arr as u8,
        c.count,
        c.res,
        res_mod as u8,
        c.rtv,
        r.fmt,
        (c.rtv != 0 && r.rtv == c.rtv) as u8
    )
}

/// `UI probe41 thread mt= rt_tid= hal_tid= begin_tid= present_tid= rt_ok=n/n rtid_changes= ahead= present_in_ui=`.
pub fn fmt41_thread(t: &Thread41) -> String {
    format!(
        "probe41 thread mt={} rt_tid={} hal_tid={} begin_tid={} present_tid={} rt_ok={}/{} rtid_changes={} ahead={} present_in_ui={}",
        t.mt as u8, t.rt_tid, t.hal_tid, t.begin_tid, t.present_tid, t.rt_ok, t.begins, t.rtid_changes, t.ahead, t.present_in_ui
    )
}

/// `UI probe41 layer|scene res=0x.. fmt=<n>:<NAME> w= h= mips= samples= flags=0x.. rt_ok= bb=WxH:NAME size_eq= res_changes= rtdata_ok=n/n|n/a`.
pub fn fmt41_target(which: UiPath, res: usize, d: &TexDesc, bb: (u32, u32, u32), res_changes: u32, rtdata: Option<(u64, u64)>) -> String {
    format!(
        "probe41 {} res=0x{res:x} fmt={}:{} w={} h={} mips={} samples={} flags=0x{:x} rt_ok={} bb={}x{}:{} size_eq={} res_changes={res_changes} rtdata_ok={}",
        which.label(),
        d.format,
        format_name(d.format),
        d.width,
        d.height,
        d.mips,
        d.samples,
        d.flags,
        desc_ok(d) as u8,
        bb.0,
        bb.1,
        format_name(bb.2),
        size_eq41(d, bb) as u8,
        rtdata.map_or("n/a".to_string(), |(a, b)| format!("{a}/{b}"))
    )
}

/// `UI probe41 output b13c= b13e= eb9= scale=<x>x<y> shaders= blend=..`.
pub fn fmt41_output(o: &Output41) -> String {
    format!(
        "probe41 output b13c={} b13e={} eb9={} scale={}x{} shaders={} blend={}",
        (o.b13c != 0) as u8,
        (o.b13e != 0) as u8,
        (o.eb9 != 0) as u8,
        o.scale_x,
        o.scale_y,
        o.shaders as u8,
        blend41(o)
    )
}

/// `UI probe41 between n=<k> frame=<i> list=0x..` (the k-th between_other ECL, `frame` = UiBegins with a HAL so far).
pub fn fmt41_between(n: u32, frame: u64, list: usize) -> String {
    format!("probe41 between n={n} frame={frame} list=0x{list:x}")
}

/// `UI probe41 recreate what=sel|res|rtdata which=layer|scene old=0x.. new=0x.. t=<s>`.
pub fn fmt41_recreate(what: &str, which: UiPath, old: usize, new: usize, t: f64) -> String {
    format!("probe41 recreate what={what} which={} old=0x{old:x} new=0x{new:x} t={t:.1}", which.label())
}

/// One sampled layer texel's vote: `Some(alpha)` when it is empty (rgb == 0).
pub fn empty_alpha(px: [u8; 4]) -> Option<u8> {
    (px[0] == 0 && px[1] == 0 && px[2] == 0).then_some(px[3])
}

/// The empty points a convention needs before it is decided (design 2.2).
pub const CONV_MIN_EMPTY: u32 = 48;

/// The convention from all samples so far: >= 90 % of at least 48 empty points agree, else unknown.
pub fn conv_vote(empty: u32, a255: u32, a0: u32) -> Option<LayerAlpha> {
    if empty < CONV_MIN_EMPTY {
        return None;
    }
    let agree = |n: u32| n as u64 * 10 >= empty as u64 * 9;
    if agree(a255) {
        Some(LayerAlpha::Transmit)
    } else if agree(a0) {
        Some(LayerAlpha::Premul)
    } else {
        None
    }
}

/// The running tally of the P2 samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlphaTally {
    pub empty: u32,
    pub a255: u32,
    pub a0: u32,
    pub other: u32,
}

impl AlphaTally {
    pub fn add(&mut self, px: [u8; 4]) {
        match empty_alpha(px) {
            Some(255) => self.a255 += 1,
            Some(0) => self.a0 += 1,
            Some(_) => self.other += 1,
            None => return,
        }
        self.empty += 1;
    }

    pub fn conv(&self) -> Option<LayerAlpha> {
        conv_vote(self.empty, self.a255, self.a0)
    }
}

/// `UI probe41 sample n=<k> empty=<e>/16 a255= a0= other= conv=.. px=<r,g,b,a;...>` (`empty` = this sample's empty
/// points of 16; the counts and `conv` cover every sample so far; up to 4 points shown).
pub fn fmt41_sample(n: u32, empty_now: u32, t: &AlphaTally, px: &[[u8; 4]]) -> String {
    let p: Vec<String> = px.iter().take(4).map(|c| format!("{},{},{},{}", c[0], c[1], c[2], c[3])).collect();
    format!(
        "probe41 sample n={n} empty={empty_now}/16 a255={} a0={} other={} conv={} px={}",
        t.a255,
        t.a0,
        t.other,
        alpha_label(t.conv()),
        p.join(";")
    )
}

/// `UI probe41 sample off why=config|gate|hresult|pso`.
pub fn fmt41_sample_off(why: &str) -> String {
    format!("probe41 sample off why={why}")
}

// ---- C4.1 P2: the layer sample (docs/C4_1_DESIGN.md 2.1 P2 / 2.4) ----------------------------------------------------
//
// One small compute list of ours reads 16 texels of ER's UI layer through our own SRV and copies them to our readback
// buffer. It is recorded in UiBegin's detour and spliced by hudhook patch 0002 directly ahead of the armed Scaleform
// list (`at == 0`), whose first command is ER's own `0xC0 -> RENDER_TARGET` barrier: the layer is therefore in `0xC0`
// (PIXEL | NON_PIXEL_SHADER_RESOURCE) when our list runs, a compute read needs NON_PIXEL, and our list never barriers
// ER's resource (design 2.4). Everything decidable on plain numbers is here.

/// Texels per sample.
pub const SAMPLE_N: usize = 16;

/// The 4 x 4 grid of design 2.1 (x 0.40..0.60, y 0.30..0.42 of the layer: above the gun, below the boss bar), in
/// texels, row-major, clamped inside the texture.
pub fn sample_points41(w: u32, h: u32) -> [[u32; 2]; SAMPLE_N] {
    let mut p = [[0u32; 2]; SAMPLE_N];
    let at = |f: f64, n: u32| ((f * n as f64).round() as u32).min(n.saturating_sub(1));
    for (i, q) in p.iter_mut().enumerate() {
        let (cx, cy) = ((i % 4) as f64, (i / 4) as f64);
        *q = [at(0.40 + 0.20 * cx / 3.0, w), at(0.30 + 0.12 * cy / 3.0, h)];
    }
    p
}

/// The shader's packed texel (`r | g << 8 | b << 16 | a << 24`, each `round(saturate(c) * 255)`) as RGBA bytes.
pub fn unpack_px(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// The layer formats the sample reads (UNORM 8-bit RGBA; an SRV of the resource's own format, so B8G8R8A8 reads back
/// in RGBA order too). Anything else: `sample off why=gate` (never guessed).
pub fn sample_format_ok41(fmt: u32) -> bool {
    matches!(fmt, DXGI_R8G8B8A8_UNORM | DXGI_B8G8R8A8_UNORM)
}

/// Everything UiBegin's detour knows when a sample is due (design 2.1 P2: "the C4.1 gate's thread / pins / desc rows").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SampleIn {
    /// The sampler exists, P2 is not off, the task asked for a sample (5 s), the sample state is Idle.
    pub ready: bool,
    pub due: bool,
    pub idle: bool,
    pub hal: usize,
    pub movies: u64,
    /// `[hal+0x1420]` after the original: the armed Scaleform list (the splice point).
    pub list: usize,
    /// UiBegin selected the layer `[S+0x100]`.
    pub layer_path: bool,
    /// `byte [G+0x11c] != 0` and `tid == hal_tid == rt_tid`.
    pub mt: bool,
    pub rt_ok: bool,
    /// The HAL's queue is the queue hudhook presents on (a DIRECT queue: our list type).
    pub queue_ok: bool,
    /// The layer chain: all 7 pins, count >= 1, res != 0; the resource's vtable in the D3D12 module (task-checked).
    pub chain_ok: bool,
    pub res_mod: bool,
    /// A fresh GetDesc of the layer this UiBegin (None = not read).
    pub desc: Option<TexDesc>,
    /// The back buffer (w, h, fmt) the present callback published.
    pub bb: (u32, u32, u32),
}

/// Why a due sample was not recorded this UiBegin (`Err`): `gate` (permanent: the layer is not a format the sample
/// reads; P2 goes off) or a per-frame reason (it is retried on the next UiBegin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleMiss {
    NotDue,
    Busy,
    NoFrame,
    Thread,
    Queue,
    Chain,
    Desc,
    /// The layer exists and is valid but not R8G8B8A8 / B8G8R8A8 UNORM.
    Format,
}

impl SampleMiss {
    pub fn label(self) -> &'static str {
        match self {
            SampleMiss::NotDue => "not_due",
            SampleMiss::Busy => "busy",
            SampleMiss::NoFrame => "no_frame",
            SampleMiss::Thread => "thread",
            SampleMiss::Queue => "queue",
            SampleMiss::Chain => "chain",
            SampleMiss::Desc => "desc",
            SampleMiss::Format => "format",
        }
    }
}

/// The P2 gate, in order. `Ok` = record now (the desc's format, width, height).
pub fn sample_gate41(s: &SampleIn) -> Result<(u32, u32, u32), SampleMiss> {
    if !s.ready || !s.due {
        return Err(SampleMiss::NotDue);
    }
    if !s.idle {
        return Err(SampleMiss::Busy);
    }
    if s.hal == 0 || s.movies == 0 || s.list == 0 || !s.layer_path {
        return Err(SampleMiss::NoFrame);
    }
    if !(s.mt && s.rt_ok) {
        return Err(SampleMiss::Thread);
    }
    if !s.queue_ok {
        return Err(SampleMiss::Queue);
    }
    if !(s.chain_ok && s.res_mod) {
        return Err(SampleMiss::Chain);
    }
    let Some(d) = s.desc else { return Err(SampleMiss::Desc) };
    if !(desc_ok(&d) && d.mips == 1 && size_eq41(&d, s.bb)) {
        return Err(SampleMiss::Desc);
    }
    if !sample_format_ok41(d.format) {
        return Err(SampleMiss::Format);
    }
    Ok((d.format, d.width as u32, d.height))
}

/// The sample's state (one `u64` the DLL moves with compare-exchange): Idle, Armed (recorded in the UiBegin of PRESENT
/// seq `s`, waiting for the ECL of its armed list), Submitted (spliced; our fence value `v`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleState {
    Idle,
    Armed(u64),
    Submitted(u64),
}

const SAMPLE_KIND_SHIFT: u32 = 62;
const SAMPLE_PAYLOAD: u64 = (1 << SAMPLE_KIND_SHIFT) - 1;

pub fn sample_pack(s: SampleState) -> u64 {
    match s {
        SampleState::Idle => 0,
        SampleState::Armed(x) => 1 << SAMPLE_KIND_SHIFT | x & SAMPLE_PAYLOAD,
        SampleState::Submitted(x) => 2 << SAMPLE_KIND_SHIFT | x & SAMPLE_PAYLOAD,
    }
}

pub fn sample_unpack(v: u64) -> SampleState {
    match v >> SAMPLE_KIND_SHIFT {
        1 => SampleState::Armed(v & SAMPLE_PAYLOAD),
        2 => SampleState::Submitted(v & SAMPLE_PAYLOAD),
        _ => SampleState::Idle,
    }
}

/// What the ECL callback does with an armed sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleEcl {
    /// Not ours (no armed sample, another frame's seq, or the armed list is not in this call).
    Pass,
    /// Splice our list at index 0, then the state is `Submitted(value)`.
    Splice,
    /// The armed list is in this call but not first: never spliced (patch 0002 rule `at == 0`); back to Idle.
    Cancel,
}

/// The ECL decision for state `cur` in PRESENT seq `seq` with the armed list at `at` in this call.
pub fn sample_ecl41(cur: SampleState, seq: u64, at: Option<usize>) -> SampleEcl {
    match (cur, at) {
        (SampleState::Armed(s), Some(i)) if s == seq & SAMPLE_PAYLOAD => {
            if splice_allowed(i) {
                SampleEcl::Splice
            } else {
                SampleEcl::Cancel
            }
        }
        _ => SampleEcl::Pass,
    }
}

/// UiEnd (after the original): a sample still Armed was never submitted. Its list never ran: back to Idle.
pub fn sample_ui_end41(cur: SampleState) -> Option<SampleState> {
    matches!(cur, SampleState::Armed(_)).then_some(SampleState::Idle)
}

/// The task thread: a Submitted sample whose fence value completed may be read, then Idle.
pub fn sample_ready41(cur: SampleState, completed: u64) -> bool {
    matches!(cur, SampleState::Submitted(v) if completed >= v)
}

/// A Submitted sample whose fence has not passed after this long (task seconds) ends P2 (`why=hresult`): the Signal or
/// the list never completed.
pub const SAMPLE_STUCK_S: f64 = 10.0;

/// The convention the verdict uses: the tally's, unless P2 ended on a D3D12 failure (then unknown: a sample list that
/// misbehaved on ER's queue never pins a convention).
pub fn sample_alpha41(t: &AlphaTally, failed: bool) -> Option<LayerAlpha> {
    if failed { None } else { t.conv() }
}

/// The C4.1 probe's totals as [`verdict41`] reads them (the task thread accumulates them from the logged lines).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeStats41 {
    /// The C4 `hooks` row passed (UiBegin / UiEnd / ECL in, no skip) and `probe41 on` was logged.
    pub hooks_c4: bool,
    pub on: bool,
    /// thread lines, and lines with `mt=0`; the last line's totals.
    pub thread_lines: u32,
    pub mt_bad_lines: u32,
    pub begins: u64,
    pub rt_ok: u64,
    pub rtid_changes: u32,
    pub present_tid: u32,
    pub ahead: u64,
    pub present_in_ui: u64,
    /// The C4 ECL totals.
    pub late: u64,
    pub armed: u64,
    pub armed_hit: u64,
    pub armed_idx_max: u64,
    pub armed_n_max: u64,
    pub between_other: u64,
    /// The largest `frame` of the logged between events.
    pub between_frame_max: u64,
    /// `pins which=layer` lines, the failing ones, distinct layer `res` values.
    pub pins_lines: u32,
    pub pins_bad: u32,
    pub res_distinct: u32,
    /// `layer` lines and the failing ones.
    pub layer_lines: u32,
    pub layer_bad: u32,
    /// The last `output` facts.
    pub output_seen: bool,
    pub output: Output41,
    /// The convention from the P2 samples (`None` = P2 off or undecided).
    pub alpha: Option<LayerAlpha>,
}

/// The C4.1 rows in the analyzer's order (design 2.3; `recreate` / `coverage` are report-only).
pub const ROWS41: [&str; 8] = ["hooks41", "thread41", "order", "adjacency41", "pins", "layer", "output", "alpha"];

/// The in-DLL C4.1 verdict: (ready, failing rows, output sdr|hdr, alpha).
pub fn verdict41(s: &ProbeStats41) -> (bool, Vec<&'static str>, &'static str, Option<LayerAlpha>) {
    let mut why = Vec::new();
    if !(s.hooks_c4 && s.on) {
        why.push("hooks41");
    }
    if !(s.thread_lines > 0 && s.mt_bad_lines == 0 && s.begins > 0 && s.rt_ok == s.begins && s.rtid_changes == 0 && s.present_tid != 0) {
        why.push("thread41");
    }
    if !(s.thread_lines > 0
        && s.ahead == 0
        && s.present_in_ui == 0
        && s.late == 0
        && s.armed > 0
        && s.armed_hit == s.armed
        && s.armed_idx_max == 0
        && s.armed_n_max == 1)
    {
        why.push("order");
    }
    if !(s.between_other == 0 || (s.between_other == 1 && s.between_frame_max <= 2)) {
        why.push("adjacency41");
    }
    if !(s.pins_lines > 0 && s.pins_bad == 0 && (1..=2).contains(&s.res_distinct)) {
        why.push("pins");
    }
    if !(s.layer_lines > 0 && s.layer_bad == 0) {
        why.push("layer");
    }
    if !(s.output_seen && s.output.shaders && s.output.scale_one()) {
        why.push("output");
    }
    if s.alpha.is_none() {
        why.push("alpha");
    }
    (why.is_empty(), why, if s.output.hdr() { "hdr" } else { "sdr" }, s.alpha)
}

/// `UI probe41 verdict ready=0|1 why=<rows or none> output=sdr|hdr alpha=transmit|premul|unknown`.
pub fn fmt41_verdict(s: &ProbeStats41) -> String {
    let (ready, why, out, alpha) = verdict41(s);
    format!(
        "probe41 verdict ready={} why={} output={out} alpha={}",
        ready as u8,
        if why.is_empty() { "none".to_string() } else { why.join(",") },
        alpha_label(alpha)
    )
}

// ---- C4.1 PASS I41 (docs/C4_1_DESIGN.md 3 / 4 / 5): the UI-thread early pass --------------------------------------------
//
// The early list is recorded in UiBegin's detour on ER's render thread (`tid == [hal+0x50] == [[G+0x6d0]+0x14]`, not the
// Present thread), into the resource the pinned read-only chain gives (`[S+0x100]`, ER's UI layer, R8G8B8A8_UNORM), as
// opaque UI-layer content in the layer's alpha convention, so ER's own composite puts its UI over the gun. Everything
// decidable on plain numbers is here: the PSO variant ([`EarlyMode`] + the pixel math the blend states implement), the
// gate ([`gate41`]), the recreate rule ([`res_step41`]), the frame-state rule for a Present inside a UI frame
// ([`present41`]), the canary and the convention re-check, the going-live decision ([`live41`]) and the `UI inject` lines.

/// What the early PSOs write into the target (C4 4.1, C4.1 3.5 table). The PSO set is keyed by (format, mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarlyMode {
    /// RGB only, the target's alpha left alone (C4's scene path).
    Scene = 0,
    /// ER's UI layer, premul convention (empty a = 0): opaque draws write A = 1; alpha draws a = α + a_dst (1 - α).
    Premul = 1,
    /// ER's UI layer, transmit convention (empty a = 1): opaque draws write A = 0; alpha draws a = a_dst (1 - α).
    Transmit = 2,
}

impl EarlyMode {
    pub fn of(conv: LayerAlpha) -> EarlyMode {
        match conv {
            LayerAlpha::Premul => EarlyMode::Premul,
            LayerAlpha::Transmit => EarlyMode::Transmit,
        }
    }

    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn from_code(c: u32) -> Option<EarlyMode> {
        match c {
            0 => Some(EarlyMode::Scene),
            1 => Some(EarlyMode::Premul),
            2 => Some(EarlyMode::Transmit),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EarlyMode::Scene => "scene",
            EarlyMode::Premul => "premul",
            EarlyMode::Transmit => "transmit",
        }
    }
}

/// One RGBA pixel, 0..1 (the layer's stored values).
pub type Px41 = [f32; 4];

/// An empty layer pixel (ER's clear) in a convention: transmit (0,0,0,1), premul (0,0,0,0).
pub fn empty_px41(conv: LayerAlpha) -> Px41 {
    match conv {
        LayerAlpha::Transmit => [0.0, 0.0, 0.0, 1.0],
        LayerAlpha::Premul => [0.0, 0.0, 0.0, 0.0],
    }
}

/// What one early draw leaves in a target pixel `dst`: a straight-alpha source `src` with coverage `a`; `opaque` = the
/// opaque PSO (no colour blend). This is the arithmetic of mw2er-render's blend states (`layer_blend`): colour
/// `src*a + dst*(1-a)` (opaque: `src`); alpha per [`EarlyMode`].
pub fn layer_write41(mode: EarlyMode, dst: Px41, src: [f32; 3], a: f32, opaque: bool) -> Px41 {
    let a = a.clamp(0.0, 1.0);
    let c = |k: usize| if opaque { src[k] } else { src[k] * a + dst[k] * (1.0 - a) };
    let alpha = match (mode, opaque) {
        (EarlyMode::Scene, _) => dst[3],
        (EarlyMode::Premul, true) => 1.0,
        (EarlyMode::Premul, false) => a + dst[3] * (1.0 - a),
        (EarlyMode::Transmit, true) => 0.0,
        (EarlyMode::Transmit, false) => dst[3] * (1.0 - a),
    };
    [c(0), c(1), c(2), alpha]
}

/// A UI draw of ER's into its own layer ("over" in the layer's convention), as the vmtest's ER-like list draws it.
pub fn ui_over41(conv: LayerAlpha, dst: Px41, src: [f32; 3], a: f32) -> Px41 {
    layer_write41(EarlyMode::of(conv), dst, src, a, false)
}

/// ER's composite of one pixel (`_BlendUI_SDR2SDR_F`, the convention's math [I]): transmit `scene*a + ui`, premul
/// `scene*(1-a) + ui` (the layer's colour is premultiplied by construction: "over" onto an empty pixel).
pub fn composite41(conv: LayerAlpha, scene: [f32; 3], l: Px41) -> [f32; 3] {
    let t = match conv {
        LayerAlpha::Transmit => l[3],
        LayerAlpha::Premul => 1.0 - l[3],
    };
    [0, 1, 2].map(|k| scene[k] * t + l[k])
}

/// The layer rule of C4.1 (design 3.3 row 8): a drawable single-sample 1-mip 2D target, R8G8B8A8 / B8G8R8A8 UNORM
/// (else `format`), back-buffer sized (else `size`). `Ok` = (format, width, height).
pub fn layer_rule41(d: &TexDesc, bb: (u32, u32, u32)) -> Result<(u32, u32, u32), Miss> {
    if !(desc_ok(d) && d.mips == 1) || !sample_format_ok41(d.format) {
        return Err(Miss::Format);
    }
    if bb.0 == 0 || !size_eq41(d, bb) {
        return Err(Miss::Size);
    }
    Ok((d.format, d.width as u32, d.height))
}

/// The layer resource the injection uses (design 3.3 row 6): `good` is in use; a different resource is a candidate
/// that must be seen in two UiBegins in a row (with the rtData cross-check passing) before it is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResPin41 {
    pub good: usize,
    pub cand: usize,
    pub cand_n: u32,
}

/// UiBegins in a row a new layer resource must pass before it is used.
pub const RES_STABLE41: u32 = 2;

/// One UiBegin's layer resource: (next pin, `Ok` = use it / `Err(recreated)`, `changed` = a new resource appeared
/// after a good one: one step toward `format_flap`).
pub fn res_step41(p: ResPin41, res: usize, rtdata_ok: bool) -> (ResPin41, Result<(), Miss>, bool) {
    if res == 0 {
        return (p, Err(Miss::NoTarget), false);
    }
    if !rtdata_ok {
        return (ResPin41 { cand: 0, cand_n: 0, ..p }, Err(Miss::Recreated), false);
    }
    if res == p.good {
        return (ResPin41 { cand: 0, cand_n: 0, ..p }, Ok(()), false);
    }
    let changed = p.cand != res && p.good != 0;
    let n = if p.cand == res { p.cand_n + 1 } else { 1 };
    if n >= RES_STABLE41 {
        (ResPin41 { good: res, cand: 0, cand_n: 0 }, Ok(()), changed)
    } else {
        (ResPin41 { cand: res, cand_n: n, ..p }, Err(Miss::Recreated), changed)
    }
}

/// `format_flap` without allocation (the render thread counts it): the last 3 events; 3 within 10 s trip.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlapRing {
    t: [f64; 3],
    n: u32,
}

impl FlapRing {
    pub const fn new() -> Self {
        FlapRing { t: [0.0; 3], n: 0 }
    }

    /// An event at `now`; true = the third within 10 s (trip).
    pub fn note(&mut self, now: f64) -> bool {
        self.t[(self.n % 3) as usize] = now;
        self.n = self.n.saturating_add(1);
        self.n >= 3 && now - self.t[(self.n % 3) as usize] < 10.0
    }
}

/// The session canary as the gate sees it (design 3.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanaryGate {
    /// The next armed frame records the canary instead of the gun.
    Due,
    /// A canary (or the persistence check) was recorded; its readback has not been judged (the frame is a `canary`
    /// miss).
    InFlight,
    /// The canary was read back intact at UiEnd; the next armed frame records an EMPTY list (the gun at Present) whose
    /// UiEnd readback must find the canary texel empty again: ER clears its layer between UI frames (review C4.1).
    Check,
    /// The canary was read back intact and was gone one UI frame later: the gun is drawn early.
    Passed,
}

/// Every input of the C4.1 gate (design 3.3), read by UiBegin's detour after the original.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateIn41 {
    pub enabled: bool,
    pub build: bool,
    pub hal: usize,
    pub movies: u64,
    pub vt_ok: bool,
    pub hal_flags: u32,
    /// `byte [G+0x11c] != 0`.
    pub mt: bool,
    pub tid: u32,
    pub hal_tid: u32,
    /// `[[G+0x6d0]+0x14]`.
    pub rt_tid: u32,
    pub present_tid: u32,
    /// A good armed frame was seen this session: a thread mismatch or a broken pin then trips.
    pub had_good: bool,
    /// This UiBegin read the previous UiBegin's seq ([`ahead41`]).
    pub ahead: bool,
    pub first: bool,
    pub wants: Wants,
    /// UiBegin selected the layer `[S+0x100]` (the scene path is not drawn by C4.1).
    pub layer_path: bool,
    pub sel: usize,
    pub chain: Chain41,
    /// The resource's vtable is in the D3D12 runtime's module (checked on the task thread; `None` = not yet).
    pub module: Option<bool>,
    /// [`res_step41`]'s answer for this UiBegin.
    pub res: Result<(), Miss>,
    pub output: Output41,
    /// `[ui] inject_ui_layer` (the C4.1 meaning: ER's HDR output, `b13c = 1`, is allowed).
    pub hdr_ok: bool,
    /// [`layer_rule41`] of a fresh GetDesc.
    pub target: Result<(u32, u32, u32), Miss>,
    /// The session's convention (pinned per build, or the probe's verdict).
    pub alpha: Option<LayerAlpha>,
    pub canary: CanaryGate,
    /// The warm-up UiBegin already happened.
    pub warm: bool,
    pub device_ok: bool,
    pub slot_free: bool,
    /// Our depth buffer exists for the layer's size (prepared by the worker, never on the render thread).
    pub depth_ready: bool,
    pub pso_ready: bool,
}

/// What the passed gate records: the canary, the persistence check (an empty list) or the gun, into the layer of
/// `format` / size, in `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Early41 {
    pub canary: bool,
    /// [`CanaryGate::Check`]: our list only barriers (no draw); its UiEnd readback looks for the canary texel.
    pub check: bool,
    pub mode: EarlyMode,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// The C4.1 gate in the design's order (3.3); `Ok` = record this frame.
pub fn gate41(g: &GateIn41) -> Result<Early41, Miss> {
    if !g.enabled {
        return Err(Miss::Config);
    }
    if !g.build {
        return Err(Miss::Build);
    }
    if g.hal == 0 {
        return Err(Miss::NoHal);
    }
    if g.movies == 0 {
        return Err(Miss::NoMovies);
    }
    if !g.vt_ok {
        return Err(Miss::Vtable);
    }
    if g.hal_flags & HAL_FLAGS_FRAME != HAL_FLAGS_FRAME {
        return Err(Miss::Flags);
    }
    if !g.mt {
        return Err(Miss::StMode);
    }
    if !(rt_ok41(g.tid, g.hal_tid, g.rt_tid) && g.present_tid != 0) {
        let known = g.present_tid != 0 && g.rt_tid != 0 && g.hal_tid != 0;
        return Err(if g.had_good && known { Miss::Thread } else { Miss::NotReady });
    }
    if g.ahead {
        return Err(Miss::Ahead);
    }
    if !g.first {
        return Err(Miss::SecondBegin);
    }
    match g.wants {
        Wants::Gun | Wants::Overlay => {}
        Wants::Nothing => return Err(Miss::Hidden),
        Wants::Ac130 => return Err(Miss::Ac130),
        Wants::NotFp => return Err(Miss::NotFp),
        Wants::ScopedNoOverlay => return Err(Miss::ScopedNoOverlay),
        Wants::NotReady => return Err(Miss::NotReady),
    }
    if !g.layer_path || g.sel == 0 {
        return Err(Miss::NoTarget);
    }
    if !g.chain.pins_ok() {
        return Err(if g.had_good { Miss::Pins } else { Miss::NoTarget });
    }
    if !g.chain.ok() {
        return Err(Miss::NoTarget);
    }
    match g.module {
        None => return Err(Miss::NotReady),
        Some(false) => return Err(Miss::Module),
        Some(true) => {}
    }
    g.res?;
    if !g.output.shaders || !g.output.scale_one() {
        return Err(Miss::Scale);
    }
    if g.output.hdr() && !g.hdr_ok {
        return Err(Miss::HdrOff);
    }
    let (format, width, height) = g.target?;
    let Some(conv) = g.alpha else { return Err(Miss::AlphaUnknown) };
    if !g.warm {
        return Err(Miss::Warmup);
    }
    if g.canary == CanaryGate::InFlight {
        return Err(Miss::Canary);
    }
    if !g.device_ok {
        return Err(Miss::Device);
    }
    if !g.slot_free {
        return Err(Miss::Busy);
    }
    if !g.depth_ready {
        return Err(Miss::Size);
    }
    if !g.pso_ready {
        return Err(Miss::PsoPending);
    }
    Ok(Early41 { canary: g.canary == CanaryGate::Due, check: g.canary == CanaryGate::Check, mode: EarlyMode::of(conv), format, width, height })
}

/// Present's callback with the C4.1 rule (design 3.4): as [`present`], plus a Present inside an open UI frame
/// (`ui_open`: this seq's UiBegin ran, its UiEnd not yet) whose early list was already spliced draws at Present anyway
/// (a one-frame double gun at worst, never neither) and trips `order` (the third value). An Armed frame there is
/// cancelled exactly as [`present`] does.
pub fn present41(cur: u64, seq: u64, ui_open: bool) -> (PresentDecision, u64, bool) {
    let (mut d, next) = present(cur, seq);
    let order = ui_open && d.early;
    if order {
        d.early = false;
    }
    (d, next, order)
}

/// The canary texel (top-left corner, outside every ER layout region) and its colour (design 3.6).
pub const CANARY_XY: [u32; 2] = [2, 2];
pub const CANARY_RGB: [u8; 3] = [1, 2, 3];
/// Canary frames at most before an unreadable result counts as `cleared`.
pub const CANARY_TRIES: u32 = 3;

/// The canary as stored in the layer: (1, 2, 3)/255, opaque in the convention (premul a = 255, transmit a = 0).
pub fn canary_px41(conv: LayerAlpha) -> [u8; 4] {
    let a = match conv {
        LayerAlpha::Premul => 255,
        LayerAlpha::Transmit => 0,
    };
    [CANARY_RGB[0], CANARY_RGB[1], CANARY_RGB[2], a]
}

/// The canary's shader colour (UNORM, written without blending or encoding).
pub fn canary_color41(conv: LayerAlpha) -> [f32; 4] {
    canary_px41(conv).map(|v| v as f32 / 255.0)
}

/// One canary readback (after UiEnd).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanaryRead {
    /// Exactly the canary: nothing cleared the layer between UiBegin and UiEnd.
    Ok,
    /// An empty texel: the layer was cleared after our list (J6 broke).
    Cleared,
    /// Something else (ER drew there): no answer.
    Unknown,
}

impl CanaryRead {
    pub fn label(self) -> &'static str {
        match self {
            CanaryRead::Ok => "ok",
            CanaryRead::Cleared => "cleared",
            CanaryRead::Unknown => "unknown",
        }
    }
}

pub fn canary_read41(px: [u8; 4], conv: LayerAlpha) -> CanaryRead {
    if px == canary_px41(conv) {
        CanaryRead::Ok
    } else if px[..3] == [0, 0, 0] {
        CanaryRead::Cleared
    } else {
        CanaryRead::Unknown
    }
}

/// After the `tries`-th canary readback: `Some(true)` = the gun may start, `Some(false)` = trip `cleared`, `None` = try
/// another canary frame.
pub fn canary_next41(r: CanaryRead, tries: u32) -> Option<bool> {
    match r {
        CanaryRead::Ok => Some(true),
        CanaryRead::Cleared => Some(false),
        CanaryRead::Unknown => (tries >= CANARY_TRIES).then_some(false),
    }
}

/// The persistence check's readback (review C4.1): the canary texel at UiEnd of a LATER UI frame whose list drew
/// nothing. The canary passing only shows that nothing clears the layer after our splice inside UiBegin..UiEnd; a layer
/// ER never clears between frames (or clears only where it redraws) would keep every early gun (trails, a double gun on
/// a frame without a UI frame) and the canary dot for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanaryGone {
    /// Empty in the pinned convention (rgb 0; a 255 transmit / 0 premul): ER cleared its layer since the canary frame.
    Gone,
    /// Still exactly the canary: ER does not clear that texel between UI frames (trip).
    Persisted,
    /// Something else (ER drew there): no answer.
    Unknown,
}

impl CanaryGone {
    pub fn label(self) -> &'static str {
        match self {
            CanaryGone::Gone => "gone",
            CanaryGone::Persisted => "persisted",
            CanaryGone::Unknown => "unknown",
        }
    }
}

pub fn canary_gone41(px: [u8; 4], conv: LayerAlpha) -> CanaryGone {
    let e = empty_px41(conv);
    if px == [0, 0, 0, (e[3] * 255.0).round() as u8] {
        CanaryGone::Gone
    } else if px == canary_px41(conv) {
        CanaryGone::Persisted
    } else {
        CanaryGone::Unknown
    }
}

/// After the `tries`-th persistence readback: `Some(true)` = the gun may start, `Some(false)` = trip (`cleared`,
/// logged `canary persisted`), `None` = another check frame.
pub fn gone_next41(r: CanaryGone, tries: u32) -> Option<bool> {
    match r {
        CanaryGone::Gone => Some(true),
        CanaryGone::Persisted => Some(false),
        CanaryGone::Unknown => (tries >= CANARY_TRIES).then_some(false),
    }
}

/// The 16 texels the UiEnd readback reads: on a canary frame the canary texel first, then 15 grid points; else the
/// 16 grid points of [`sample_points41`] (they carry the convention for the re-check).
pub fn readback_points41(w: u32, h: u32, canary: bool) -> [[u32; 2]; SAMPLE_N] {
    let grid = sample_points41(w, h);
    if !canary {
        return grid;
    }
    let mut p = [[0u32; 2]; SAMPLE_N];
    p[0] = [CANARY_XY[0].min(w.saturating_sub(1)), CANARY_XY[1].min(h.saturating_sub(1))];
    p[1..].copy_from_slice(&grid[..SAMPLE_N - 1]);
    p
}

/// The convention re-check (design 3.6): every this many seconds, on a frame whose early list drew the gun.
pub const RECHECK_S: f64 = 60.0;
/// Empty texels one re-check sample needs before it votes.
pub const RECHECK_MIN_EMPTY: u32 = 8;

/// One re-check sample's vote: >= 90 % of at least 8 empty texels agree, else none.
pub fn recheck_vote41(px: &[[u8; 4]]) -> Option<LayerAlpha> {
    let mut t = AlphaTally::default();
    for p in px {
        t.add(*p);
    }
    if t.empty < RECHECK_MIN_EMPTY {
        return None;
    }
    let agree = |n: u32| n as u64 * 10 >= t.empty as u64 * 9;
    if agree(t.a255) {
        Some(LayerAlpha::Transmit)
    } else if agree(t.a0) {
        Some(LayerAlpha::Premul)
    } else {
        None
    }
}

/// Re-check votes against the session's convention: two disagreements in a row trip `alpha`; an agreeing vote resets
/// the streak, no vote leaves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recheck41 {
    pub disagree: u32,
}

impl Recheck41 {
    pub fn step(&mut self, vote: Option<LayerAlpha>, conv: LayerAlpha) -> bool {
        match vote {
            None => false,
            Some(v) if v == conv => {
                self.disagree = 0;
                false
            }
            Some(_) => {
                self.disagree += 1;
                self.disagree >= 2
            }
        }
    }
}

/// How long the injection waits for a probe verdict whose only failing row is `alpha` while P2 still samples.
pub const LIVE_WAIT_ALPHA_S: f64 = 600.0;

/// The going-live decision (design 3.2 / 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live41 {
    /// Go live with this convention.
    Go(LayerAlpha),
    /// Keep waiting for a (later) probe verdict.
    Wait,
    /// Off for the session (`UI inject off why=<label>`).
    Off(&'static str),
}

/// What the task thread knows when deciding: the per-build pin, and (probe on) the in-DLL C4.1 verdict.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveIn41 {
    pub pinned: Option<LayerAlpha>,
    /// `[ui] probe_scaleform` with the C4.1 reads (then its verdict gates).
    pub probe: bool,
    pub judged: bool,
    pub ready: bool,
    /// The verdict's alpha (P2's convention).
    pub probe_alpha: Option<LayerAlpha>,
    /// Every row but `alpha` passed.
    pub only_alpha: bool,
    /// P2 still samples (not off).
    pub sampling: bool,
    /// Seconds since the injection's hooks were ready.
    pub waited_s: f64,
}

/// The per-build pin wins when there is no probe. With the probe on, its verdict gates: ready -> its convention (a pin
/// that contradicts it -> off); not ready only on `alpha` -> the pin if there is one, else wait while P2 samples (at
/// most [`LIVE_WAIT_ALPHA_S`]); any other failing row -> off.
pub fn live41(l: &LiveIn41) -> Live41 {
    if !l.probe {
        return match l.pinned {
            Some(c) => Live41::Go(c),
            None => Live41::Off("alpha_unpinned"),
        };
    }
    if !l.judged {
        return Live41::Wait;
    }
    if l.ready {
        return match (l.pinned, l.probe_alpha) {
            (Some(p), Some(a)) if p != a => Live41::Off("probe_verdict"),
            (Some(p), _) => Live41::Go(p),
            (None, Some(a)) => Live41::Go(a),
            (None, None) => Live41::Off("probe_verdict"),
        };
    }
    if l.only_alpha {
        if let Some(p) = l.pinned {
            return Live41::Go(p);
        }
        if l.sampling && l.waited_s < LIVE_WAIT_ALPHA_S {
            return Live41::Wait;
        }
        return Live41::Off("alpha_unpinned");
    }
    Live41::Off("probe_verdict")
}

/// `UI inject on build=.. path=layer mode=ui_thread output=sdr|hdr alpha=.. fmt=<NAME> size=<w>x<h> slots=3`.
pub fn fmt_inject41_on(build: &str, hdr: bool, conv: LayerAlpha, format: u32, w: u32, h: u32) -> String {
    format!(
        "inject on build={build} path=layer mode=ui_thread output={} alpha={} fmt={} size={w}x{h} slots={SLOTS}",
        if hdr { "hdr" } else { "sdr" },
        conv.label(),
        format_name(format)
    )
}

/// `UI inject early=<n> present=<n> splice=<n> cancelled=<n> fallback_cut=<n> miss=<why:n,...>` (C4.1; the C4 line
/// [`fmt_inject`] has no `fallback_cut`).
pub fn fmt_inject41(early: u64, present: u64, splice: u64, cancelled: u64, fallback_cut: u64, misses: &[u64]) -> String {
    let m: Vec<String> = Miss::ALL.iter().zip(misses).filter(|(_, n)| **n > 0).map(|(k, n)| format!("{}:{n}", k.label())).collect();
    format!(
        "inject early={early} present={present} splice={splice} cancelled={cancelled} fallback_cut={fallback_cut} miss={}",
        if m.is_empty() { "none".to_string() } else { m.join(",") }
    )
}

/// `UI inject canary ok|cleared|unknown value=<r,g,b,a>`.
pub fn fmt_canary41(r: CanaryRead, px: [u8; 4]) -> String {
    format!("inject canary {} value={},{},{},{}", r.label(), px[0], px[1], px[2], px[3])
}

/// `UI inject canary gone|persisted|unknown value=<r,g,b,a>` (the persistence check one UI frame later).
pub fn fmt_canary_gone41(r: CanaryGone, px: [u8; 4]) -> String {
    format!("inject canary {} value={},{},{},{}", r.label(), px[0], px[1], px[2], px[3])
}

/// `UI inject recheck vote=.. alpha=.. empty=<e>/16 disagree=<n>` (the 60 s convention re-check).
pub fn fmt_recheck41(vote: Option<LayerAlpha>, conv: LayerAlpha, empty: u32, disagree: u32) -> String {
    format!("inject recheck vote={} alpha={} empty={empty}/16 disagree={disagree}", alpha_label(vote), conv.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_getter_plain_patterns() {
        assert_eq!(parse_getter(&[0x48, 0x8b, 0x41, 0x18, 0xc3, 0xcc]), Some(Getter { kind: GetterKind::Load, off: 0x18 }));
        assert_eq!(parse_getter(&[0x48, 0x8d, 0x41, 0x08, 0xc3]), Some(Getter { kind: GetterKind::Lea, off: 8 }));
        assert_eq!(parse_getter(&[0x48, 0x8b, 0x81, 0x20, 0x01, 0, 0, 0xc3]), Some(Getter { kind: GetterKind::Load, off: 0x120 }));
        assert_eq!(parse_getter(&[0x48, 0x8d, 0x81, 0x00, 0x10, 0, 0, 0xc3]), Some(Getter { kind: GetterKind::Lea, off: 0x1000 }));
        assert_eq!(parse_getter(&[0x48, 0x8b, 0xc1, 0xc3]), Some(Getter { kind: GetterKind::Lea, off: 0 }));
    }

    #[test]
    fn c4_getter_rejects_non_plain() {
        // a call, a branch, another register, no ret, a negative disp8, a store, too short, a huge disp32
        for b in [
            &[0xe8, 0, 0, 0, 0, 0xc3][..],
            &[0x48, 0x85, 0xc9, 0x74, 0x05, 0xc3],
            &[0x48, 0x8b, 0x42, 0x18, 0xc3],
            &[0x48, 0x8b, 0x41, 0x18, 0x90],
            &[0x48, 0x8b, 0x41, 0xf8, 0xc3],
            &[0x48, 0x89, 0x41, 0x18, 0xc3],
            &[0x48, 0x8b, 0x41],
            &[0x48, 0x8b, 0x81, 0, 0, 0, 0x40, 0xc3],
        ] {
            assert_eq!(parse_getter(b), None, "{b:02x?}");
        }
    }

    #[test]
    fn c4_getter_chain_reads() {
        let mem = |a: usize| -> Option<usize> {
            Some(match a {
                0x1018 => 0x2000,
                0x2008 => 0x3000,
                0x3020 => 0xdead_0000,
                0x1108 => 0x3000,
                0x7000 => return None,
                _ => 0,
            })
        };
        let load = Getter { kind: GetterKind::Load, off: 0x18 };
        assert_eq!(chain_res(0x1000, load, 8, 0x20, &mem), Some(0xdead_0000));
        assert_eq!(chain_res(0x1000, Getter { kind: GetterKind::Lea, off: 0x100 }, 8, 0x20, &mem), Some(0xdead_0000));
        assert_eq!(chain_res(0x5000, load, 8, 0x20, &mem), None, "a zero object");
        assert_eq!(chain_res(0x6fe8, load, 8, 0x20, &mem), None, "an unreadable qword");
        assert_eq!(chain_res(usize::MAX - 4, load, 8, 0x20, &mem), None, "no wrap-around");
    }

    #[test]
    fn c4_fmt_accepted_table() {
        for f in [DXGI_R10G10B10A2_UNORM, DXGI_R8G8B8A8_UNORM, DXGI_B8G8R8A8_UNORM] {
            assert_eq!(format_rule(f, UiPath::Scene, false), Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: false }));
            assert_eq!(format_rule(f, UiPath::Scene, true), Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: false }));
            assert_eq!(format_rule(f, UiPath::Layer, true), Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: true }));
            assert_eq!(format_rule(f, UiPath::Layer, false), Err(Miss::HdrOff), "HDR path without inject_ui_layer");
        }
        for f in [DXGI_R8G8B8A8_UNORM_SRGB, DXGI_B8G8R8A8_UNORM_SRGB] {
            assert_eq!(format_rule(f, UiPath::Scene, false), Ok(FmtRule { out: EarlyOut::Linear, write_alpha: false }));
            assert_eq!(format_rule(f, UiPath::Layer, true), Err(Miss::Format), "sRGB on the layer is not in the table");
        }
    }

    #[test]
    fn c4_fmt_refuses_unknown() {
        for f in [DXGI_R16G16B16A16_FLOAT, DXGI_R11G11B10_FLOAT, DXGI_R10G10B10A2_TYPELESS, DXGI_R8G8B8A8_TYPELESS, DXGI_B8G8R8A8_TYPELESS, 0, 2, 999] {
            assert_eq!(format_rule(f, UiPath::Scene, true), Err(Miss::Format), "{f}");
            assert!(format_rule(f, UiPath::Layer, true).is_err(), "{f}");
        }
        let good = TexDesc { dimension: DIM_TEXTURE2D, width: 3840, height: 2160, array: 1, mips: 1, format: DXGI_R10G10B10A2_UNORM, samples: 1, flags: 1 };
        assert!(desc_ok(&good));
        assert!(target_rule(&good, UiPath::Scene, false).is_ok());
        for bad in [
            TexDesc { samples: 4, ..good },
            TexDesc { array: 2, ..good },
            TexDesc { dimension: 4, ..good },
            TexDesc { flags: 0x8, ..good },
            TexDesc { width: 0, ..good },
            TexDesc { height: 20000, ..good },
        ] {
            assert!(!desc_ok(&bad), "{bad:?}");
            assert_eq!(target_rule(&bad, UiPath::Scene, true), Err(Miss::Format));
        }
        assert_eq!(format_name(DXGI_R10G10B10A2_UNORM), "R10G10B10A2_UNORM");
        assert_eq!(format_name(1234), "OTHER");
        assert_eq!(select_path(1, 0, 0x1000), UiPath::Layer);
        assert_eq!(select_path(0, 1, 0x1000), UiPath::Layer);
        assert_eq!(select_path(1, 1, 0), UiPath::Scene, "no layer texture: the scene");
        assert_eq!(select_path(0, 0, 0x1000), UiPath::Scene);
    }

    fn good_gate() -> GateIn {
        GateIn {
            enabled: true,
            build: true,
            hal: 0x1000,
            movies: 3,
            vt_ok: true,
            hal_flags: 3,
            tid: 7,
            hal_tid: 7,
            present_tid: 7,
            thread_trips: true,
            first: true,
            wants: Wants::Gun,
            getter_ok: true,
            chain_ok: true,
            module_ok: true,
            target: Ok(FmtRule { out: EarlyOut::Srgb, write_alpha: false }),
            device_ok: true,
            slot_free: true,
            pso_ready: true,
        }
    }

    #[test]
    fn c4_gate_every_reason() {
        let g = good_gate();
        assert!(gate(&g).is_ok());
        assert!(gate(&GateIn { wants: Wants::Overlay, ..g }).is_ok());
        let cases = [
            (GateIn { enabled: false, ..g }, Miss::Config),
            (GateIn { build: false, ..g }, Miss::Build),
            (GateIn { hal: 0, ..g }, Miss::NoHal),
            (GateIn { movies: 0, ..g }, Miss::NoMovies),
            (GateIn { vt_ok: false, ..g }, Miss::Vtable),
            (GateIn { hal_flags: 1, ..g }, Miss::Flags),
            (GateIn { tid: 8, ..g }, Miss::Thread),
            (GateIn { tid: 8, thread_trips: false, ..g }, Miss::NotReady),
            (GateIn { present_tid: 0, ..g }, Miss::NotReady),
            (GateIn { first: false, ..g }, Miss::SecondBegin),
            (GateIn { wants: Wants::Nothing, ..g }, Miss::Hidden),
            (GateIn { wants: Wants::Ac130, ..g }, Miss::Ac130),
            (GateIn { wants: Wants::NotFp, ..g }, Miss::NotFp),
            (GateIn { wants: Wants::ScopedNoOverlay, ..g }, Miss::ScopedNoOverlay),
            (GateIn { wants: Wants::NotReady, ..g }, Miss::NotReady),
            (GateIn { getter_ok: false, ..g }, Miss::Getter),
            (GateIn { chain_ok: false, ..g }, Miss::NoTarget),
            (GateIn { module_ok: false, ..g }, Miss::Module),
            (GateIn { target: Err(Miss::Format), ..g }, Miss::Format),
            (GateIn { target: Err(Miss::HdrOff), ..g }, Miss::HdrOff),
            (GateIn { device_ok: false, ..g }, Miss::Device),
            (GateIn { slot_free: false, ..g }, Miss::Busy),
            (GateIn { pso_ready: false, ..g }, Miss::PsoPending),
        ];
        for (i, want) in cases {
            assert_eq!(gate(&i), Err(want), "{i:?}");
        }
        // the cheap / expected checks come first: no movies never trips on a bad vtable
        assert_eq!(gate(&GateIn { movies: 0, vt_ok: false, ..g }), Err(Miss::NoMovies));
        assert_eq!(gate(&GateIn { enabled: false, vt_ok: false, getter_ok: false, ..g }), Err(Miss::Config));
    }

    #[test]
    fn c4_gate_trip_set_and_labels() {
        let trips: Vec<&str> = Miss::ALL.iter().filter(|m| m.is_trip()).map(|m| m.label()).collect();
        assert_eq!(
            trips,
            ["vtable", "thread", "getter", "module", "device", "format_flap", "hresult", "panic", "never_submitted", "device_removed", "pins", "order", "alpha", "cleared"]
        );
        for m in [
            Miss::NoMovies,
            Miss::Ac130,
            Miss::Hidden,
            Miss::NotFp,
            Miss::ScopedNoOverlay,
            Miss::Busy,
            Miss::PsoPending,
            Miss::Locked,
            Miss::HdrOff,
            Miss::SecondBegin,
            Miss::Cancelled,
            Miss::StMode,
            Miss::Ahead,
            Miss::Recreated,
            Miss::Scale,
            Miss::Size,
            Miss::AlphaUnknown,
            Miss::Canary,
            Miss::Warmup,
        ] {
            assert!(!m.is_trip(), "{m:?} is an expected miss (design 5.1)");
        }
        let mut labels: Vec<&str> = Miss::ALL.iter().map(|m| m.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Miss::ALL.len());
        for (i, m) in Miss::ALL.iter().enumerate() {
            assert_eq!(m.index(), i);
        }
        let mut w = FlapWindow::default();
        assert!(!w.refused_change(1.0) && !w.refused_change(5.0));
        assert!(w.refused_change(9.0), "3 refused changes in 10 s");
        let mut w = FlapWindow::default();
        assert!(!w.refused_change(1.0) && !w.refused_change(6.0) && !w.refused_change(12.0), "spread over more than 10 s");
    }

    #[test]
    fn c4_state_pack_roundtrip() {
        for kind in [Kind::Idle, Kind::Armed, Kind::Submitted, Kind::Cancelled, Kind::Closed] {
            for slot in 0..3u8 {
                for seq in [0u64, 1, 77, SEQ_MASK] {
                    assert_eq!(unpack(pack(kind, slot, seq)), Frame { kind, slot, seq });
                }
            }
        }
        assert_eq!(pack(Kind::Idle, 0, 0), 0);
    }

    #[test]
    fn c4_state_single_frame_rules() {
        let s = 5u64;
        let a = armed(s, 2);
        assert!(begin_ok(0, s) && begin_ok(pack(Kind::Closed, 0, 4), s), "an older Present's state never blocks");
        assert!(!begin_ok(a, s) && !begin_ok(pack(Kind::Closed, 0, s), s), "a second UiBegin of the same Present");
        assert_eq!(ecl_submit(a, s), Some(pack(Kind::Submitted, 2, s)));
        assert_eq!(ecl_submit(a, s + 1), None, "never into the next Present");
        assert_eq!(ui_end(a, s), Some(pack(Kind::Cancelled, 2, s)));
        assert_eq!(ui_end(pack(Kind::Submitted, 2, s), s), None);
        let (d, n) = present(pack(Kind::Submitted, 2, s), s);
        assert_eq!((d, n), (PresentDecision { early: true, cancelled: false }, pack(Kind::Closed, 0, s)));
        let (d, _) = present(a, s);
        assert_eq!(d, PresentDecision { early: false, cancelled: true });
        let (d, _) = present(pack(Kind::Submitted, 0, s - 1), s);
        assert!(!d.early, "a Submitted of an older Present is not this frame's gun");
        assert_eq!(ecl_submit(n, s), None, "after Present nothing submits");
    }

    /// One simulated frame: the events in order on the state; returns (early drawn, present drew, present ran).
    fn run_frame(st: &mut u64, seq: u64, events: &[u8]) -> (bool, bool, bool) {
        let (mut early, mut drew, mut ran) = (false, false, false);
        for &e in events {
            match e {
                // UiBegin whose gate passes / fails
                0 => {
                    if begin_ok(*st, seq) {
                        *st = armed(seq, (seq % 3) as u8);
                    }
                }
                1 => {}
                // ECL with the armed list / without it
                2 => {
                    if let Some(n) = ecl_submit(*st, seq) {
                        *st = n;
                        assert!(!ran, "a splice after this frame's Present");
                        early = true;
                    }
                }
                3 => {}
                // UiEnd
                4 => {
                    if let Some(n) = ui_end(*st, seq) {
                        *st = n;
                    }
                }
                // Present's callback (once per frame)
                _ => {
                    if ran {
                        continue;
                    }
                    ran = true;
                    let (d, n) = present(*st, seq);
                    *st = n;
                    drew = !d.early;
                    assert_eq!(d.early, early, "Present sees exactly the frame's splice");
                }
            }
        }
        (early, drew, ran)
    }

    #[test]
    fn c4_state_exhaustive_never_both_never_neither() {
        // every event order of length <= 4 in two consecutive Presents (the state carries over)
        let mut seqs: Vec<Vec<u8>> = vec![vec![]];
        for len in 1..=4 {
            let mut v = vec![0u8; len];
            loop {
                seqs.push(v.clone());
                let mut i = 0;
                while i < len {
                    v[i] += 1;
                    if v[i] < 6 {
                        break;
                    }
                    v[i] = 0;
                    i += 1;
                }
                if i == len {
                    break;
                }
            }
        }
        assert_eq!(seqs.len(), 1 + 6 + 36 + 216 + 1296);
        let mut n = 0u64;
        for a in &seqs {
            for b in &seqs {
                let mut st = 0u64;
                for (seq, ev) in [(10u64, a), (11, b)] {
                    let (early, drew, ran) = run_frame(&mut st, seq, ev);
                    assert!(!(early && drew), "{a:?} {b:?}: gun drawn twice");
                    if ran {
                        assert!(early || drew, "{a:?} {b:?}: no gun with Present run");
                    }
                    n += 1;
                }
            }
        }
        assert!(n > 2_000_000);
    }

    #[test]
    fn c4_state_never_submitted_streak() {
        let mut s = Streak::default();
        for _ in 0..NEVER_SUBMITTED_TRIP - 1 {
            assert!(!s.armed_outcome(false));
        }
        assert!(!s.armed_outcome(true), "a submit resets the streak");
        for i in 0..NEVER_SUBMITTED_TRIP {
            assert_eq!(s.armed_outcome(false), i == NEVER_SUBMITTED_TRIP - 1);
        }
    }

    #[test]
    fn c4_splice_index_math() {
        assert_eq!(find_armed(&[0x10], 0x10), Some(0));
        assert_eq!(find_armed(&[0x10, 0x20, 0x30], 0x30), Some(2));
        assert_eq!(find_armed(&[0x10, 0x20], 0x40), None, "list absent");
        assert_eq!(find_armed(&[0, 0x20], 0), None, "nothing armed never matches a null entry");
        assert_eq!(find_armed(&[], 0x10), None);
        assert_eq!(splice_calls(1, 0), Some(((0, 0), (0, 1))), "n = 1: ours, then ER's whole call");
        assert_eq!(splice_calls(3, 0), Some(((0, 0), (0, 3))));
        assert_eq!(splice_calls(3, 2), Some(((0, 2), (2, 1))), "at = n - 1");
        assert_eq!(splice_calls(3, 3), None);
        assert_eq!(splice_calls(0, 0), None);
        assert!(splice_allowed(0) && !splice_allowed(1), "v1 never splits an ER call");
    }

    #[test]
    fn c4_ring_slots_and_retire() {
        assert_eq!(pick_slot(&[0, 0, 0], 0, 2), Some(0));
        assert_eq!(pick_slot(&[5, 6, 7], 5, 0), Some(0), "only slot 0 completed");
        assert_eq!(pick_slot(&[5, 6, 7], 6, 0), Some(1), "round robin after the last slot");
        assert_eq!(pick_slot(&[5, 6, 7], 4, 0), None, "all in flight: busy, never a wait");
        assert_eq!(pick_slot(&[1, 2, 3], 3, 1), Some(2));
        let mut r = vec![(4u64, "a"), (9, "b"), (6, "c")];
        assert_eq!(release_ready(&mut r, 5), vec!["a"]);
        assert_eq!(r, vec![(9, "b"), (6, "c")]);
        assert_eq!(release_ready(&mut r, 6), vec!["c"]);
        assert!(release_ready(&mut r, 8).is_empty(), "the depth of a list still in flight stays");
        assert_eq!(release_ready(&mut r, 9), vec!["b"]);
        assert!(r.is_empty());
    }

    fn ready_stats() -> ProbeStats {
        ProbeStats {
            hook_begin: true,
            hook_end: true,
            ecl_hook: true,
            hal_seen: true,
            begins: 3600,
            begin_tid_ok: 3600,
            windows: 24,
            getter_seen: true,
            getter_plain: true,
            target_seen: true,
            armed: 3500,
            armed_hit: 3500,
            armed_first: 3500,
            ..ProbeStats::default()
        }
    }

    #[test]
    fn c4_probe_verdict_rows() {
        let s = ready_stats();
        assert_eq!(verdict(&s), (true, vec![]));
        assert_eq!(fmt_verdict(&s), "probe verdict ready=1 why=none");
        let cases: [(ProbeStats, &str); 13] = [
            (ProbeStats { hook_end: false, ..s }, "hooks"),
            (ProbeStats { skips: 1, ..s }, "hooks"),
            (ProbeStats { ecl_hook: false, ..s }, "hooks"),
            (ProbeStats { hal_vt_bad: 1, ..s }, "hal"),
            (ProbeStats { hal_dev_bad: 2, ..s }, "hal"),
            (ProbeStats { thread_bad: 1, ..s }, "thread"),
            (ProbeStats { begin_tid_ok: 3599, ..s }, "thread"),
            (ProbeStats { per_present_bad_windows: 1, windows: 24, ..s }, "thread"),
            (ProbeStats { getter_changes: 1, ..s }, "getter"),
            (ProbeStats { target_bad: 1, ..s }, "target"),
            (ProbeStats { armed_first: 3499, ..s }, "adjacency"),
            (ProbeStats { between_other: 1, ..s }, "adjacency"),
            (ProbeStats { late: 1, ..s }, "adjacency"),
        ];
        for (c, row) in cases {
            let (ready, why) = verdict(&c);
            assert!(!ready && why == vec![row], "{row}: {why:?}");
        }
        // 1 bad window of 100 is within the 99 % rule
        assert!(verdict(&ProbeStats { per_present_bad_windows: 1, windows: 100, ..s }).0);
        let (r, why) = verdict(&ProbeStats::default());
        assert!(!r && why == vec!["hooks", "hal", "thread", "getter", "target", "adjacency"]);
        assert_eq!(fmt_verdict(&ProbeStats { late: 2, target_bad: 1, ..s }), "probe verdict ready=0 why=target,adjacency");
    }

    #[test]
    fn c4_probe_line_formats() {
        assert_eq!(
            fmt_hal(0x7ff6_02c0_bb40, true, true, false, 1234, 1234),
            "probe hal vt=0x7ff602c0bb40 vt_ok=1 dev_eq=1 queue_eq_present=0 rtid=1234 present_tid=1234 same_thread=1"
        );
        assert!(fmt_hal(1, true, true, true, 5, 6).ends_with("same_thread=0"));
        assert!(fmt_hal(1, true, true, true, 0, 0).ends_with("same_thread=0"), "no thread seen is not the same thread");
        assert_eq!(fmt_mode(1, 0, 0x55), "probe mode b13c=1 b13e=0 layer=1 sel=layer");
        assert_eq!(fmt_mode(0, 0, 0x55), "probe mode b13c=0 b13e=0 layer=1 sel=scene");
        assert_eq!(fmt_getter(0x140001000, &[0x48, 0x8b, 0x41, 0x18, 0xc3, 0xcc, 0xcc, 0xcc]), "probe getter addr=0x140001000 bytes=488b4118c3cccccc plain=1 off=0x18");
        let d = TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: 24, samples: 1, flags: 1 };
        assert_eq!(
            fmt_rt(UiPath::Scene, 0xabc0, &d, (3840, 2160, 24), 1),
            "probe rt which=scene res=0xabc0 fmt=24:R10G10B10A2_UNORM w=3840 h=2160 dim=3 mips=1 arr=1 samples=1 flags=0x1 rt_ok=1 bb=3840x2160:R10G10B10A2_UNORM distinct=1"
        );
        assert_eq!(fmt_frame(300, 299, 299, 1, 0, 1, 299), "probe frame present=300 ui_begin=299 ui_end=299 nomovies=1 per_present=0-1 begin_tid_ok=299/299");
        let t = EclTotals { total: 9, halq: 7, otherq: 2, ui: 3, armed: 2, armed_hit: 2, armed_first: 2, armed_n_max: 1, armed_tid_ok: 2, ..EclTotals::default() };
        assert_eq!(
            fmt_ecl(&t),
            "probe ecl total=9 halq=7 otherq=2 pre=0 ui=3 post=0 armed=2 armed_hit=2 armed_first=2 armed_idx_max=0 armed_n_max=1 armed_tid_ok=2 between_other=0 late=0"
        );
        let mut misses = [0u64; 29];
        misses[Miss::NoMovies.index()] = 3;
        misses[Miss::Busy.index()] = 1;
        assert_eq!(fmt_inject(100, 4, 100, 0, &misses), "inject early=100 present=4 splice=100 cancelled=0 miss=nomovies:3,busy:1");
        assert_eq!(fmt_inject(0, 0, 0, 0, &[0; 29]), "inject early=0 present=0 splice=0 cancelled=0 miss=none");
    }

    #[test]
    fn c4_probe_ecl_counting() {
        // the armed list first on the hal queue in the UI phase
        assert_eq!(count_ecl(Phase::Ui, true, true, false, false), EclCount { armed_hit: true, armed_first: true, ..EclCount::default() });
        // another hal ECL came first
        assert_eq!(count_ecl(Phase::Ui, true, true, false, true), EclCount { armed_hit: true, ..EclCount::default() });
        assert_eq!(count_ecl(Phase::Ui, true, false, false, false), EclCount { between_other: true, ..EclCount::default() });
        assert_eq!(count_ecl(Phase::Post, true, true, false, false), EclCount { late: true, ..EclCount::default() });
        assert_eq!(count_ecl(Phase::Ui, false, true, false, false), EclCount::default(), "another queue never counts");
        assert_eq!(count_ecl(Phase::Ui, true, false, true, false), EclCount::default(), "after the armed submit");
        assert_eq!(count_ecl(Phase::Pre, true, false, false, false), EclCount::default());
    }

    #[test]
    fn c4_teardown_order() {
        use TeardownStep::*;
        assert_eq!(teardown_plan(false, true, false), vec![Deactivate, Disable, WaitInflight], "hot disable / trip: disable only, trampolines kept");
        assert_eq!(teardown_plan(false, false, false), vec![Deactivate, Disable, WaitInflight]);
        assert_eq!(teardown_plan(true, true, true), vec![Deactivate, Disable, WaitInflight, WaitFence, Release]);
        assert_eq!(teardown_plan(true, true, false), vec![Deactivate, Disable, WaitInflight, WaitFence], "fence timeout: leak, never free");
        assert_eq!(teardown_plan(true, false, true), vec![Deactivate, Disable, WaitInflight, WaitFence], "a thread still inside: leak");
        let p = teardown_plan(true, true, true);
        let at = |s| p.iter().position(|x| *x == s).unwrap();
        assert!(at(Deactivate) < at(Disable) && at(Disable) < at(WaitInflight) && at(WaitInflight) < at(WaitFence) && at(WaitFence) < at(Release));
    }

    // ---- C4.1 ----

    use crate::exe_build::{SCALEFORM_UI_2720, UI41_2720, UI_LAYOUT_2720};
    use std::collections::HashMap;

    const BASE: usize = 0x1_4000_0000;

    /// A fake address space for the chain: qwords (u32 / u8 reads take the low bytes).
    fn chain_mem() -> HashMap<usize, u64> {
        let u = UI41_2720;
        let (vtp, subp) = (BASE + u.sel_vt, BASE + u.sub_vt);
        let mut m = HashMap::new();
        for (a, v) in [
            (0x1000, vtp),
            (vtp + 0x38, BASE + u.sel_vt_count),
            (0x1070, subp),
            (subp, BASE + u.sub_slot0),
            (subp + 8, BASE + u.sub_slot1),
            (0x1040, 0x2000),
            (0x1078, 0x3000),
            (0x2038, 0x2100),
            (0x2108, 0x2200),
            (0x2208, 1),
            (0x2046, 0),
            (0x3000, 0x4000),
            (0x4010, 0x7777),
            (0x4008, 0x5000),
            (0x5020, 0xdead_0000),
            // owner+0xdd0 -> rt, rt+0x20 -> rtData (format 28, rtv 0x7777)
            (0x9000 + 0xdd0, 0xa000),
            (0xa020, 0xb000),
            (0xb020, 28),
            (0xb028, 0x7777),
        ] {
            m.insert(a, v as u64);
        }
        m
    }

    fn rd_of(m: &HashMap<usize, u64>) -> impl Fn(usize, usize) -> Option<u64> + '_ {
        move |a, len| {
            m.get(&a).map(|v| match len {
                1 => v & 0xff,
                4 => v & 0xffff_ffff,
                _ => *v,
            })
        }
    }

    fn chain_of(m: &HashMap<usize, u64>, sel: usize) -> Chain41 {
        chain41(sel, BASE, &UI41_2720, &UI_LAYOUT_2720, &rd_of(m))
    }

    #[test]
    fn c4_chain41_pins_and_res() {
        let m = chain_mem();
        let c = chain_of(&m, 0x1000);
        assert!(c.pins_ok() && c.ok(), "{c:?}");
        assert_eq!((c.count, c.obj, c.res, c.rtv), (1, 0x4000, 0xdead_0000, 0x7777));
        assert_eq!(c.bits(), 0x7f);
        assert_eq!(Chain41::from_bits(c.bits(), c.count, c.obj, c.res, c.rtv), c);
        assert_eq!(chain_of(&m, 0), Chain41::default(), "no selected texture");
        // each pin broken: its flag drops and the resource is never followed
        let u = UI41_2720;
        let (vtp, subp) = (BASE + u.sel_vt, BASE + u.sub_vt);
        for (addr, flag) in [(0x1000usize, 0u8), (vtp + 0x38, 1), (0x1070, 2), (subp, 3), (subp + 8, 4), (0x1040, 5), (0x1078, 6)] {
            let mut b = m.clone();
            // a vtable / slot moved by 8, or T / arr zero
            *b.get_mut(&addr).unwrap() = if flag >= 5 { 0 } else { m[&addr] + 8 };
            let c = chain_of(&b, 0x1000);
            assert_eq!(c.bits() >> flag & 1, 0, "pin {flag} at 0x{addr:x}: {c:?}");
            assert!(!c.ok() && c.res == 0, "pin {flag}: {c:?}");
            // unreadable is a failed pin too
            let mut b = m.clone();
            b.remove(&addr);
            assert!(chain_of(&b, 0x1000).res == 0, "pin {flag} unreadable");
        }
        // a wrong primary vtable never reads its slot 7; a wrong sub vtable never reads its slots
        let mut b = m.clone();
        *b.get_mut(&0x1000).unwrap() = 0x8000;
        b.insert(0x8038, (BASE + u.sel_vt_count) as u64);
        assert!(!chain_of(&b, 0x1000).vt7);
    }

    #[test]
    fn c4_chain41_count_and_zero_steps() {
        let m = chain_mem();
        // the fallback count `u8 [T+0x46]` when `[T+0x38] == 0`
        let mut b = m.clone();
        *b.get_mut(&0x2038).unwrap() = 0;
        assert_eq!(chain_of(&b, 0x1000).count, 0, "u8 [T+0x46] = 0");
        *b.get_mut(&0x2046).unwrap() = 0x0102;
        let c = chain_of(&b, 0x1000);
        assert_eq!(c.count, 2, "only the low byte");
        assert!(c.ok());
        // count 0: index 0 is out of range (ER's slot 0 fails), nothing followed
        let mut b = m.clone();
        *b.get_mut(&0x2208).unwrap() = 0;
        let c = chain_of(&b, 0x1000);
        assert!(c.pins_ok() && !c.ok() && c.obj == 0 && c.res == 0);
        // a count read as u32
        let mut b = m.clone();
        *b.get_mut(&0x2208).unwrap() = 0x1_0000_0003;
        assert_eq!(chain_of(&b, 0x1000).count, 3);
        // every zero pointer after the pins
        for addr in [0x3000usize, 0x4008, 0x5020] {
            let mut b = m.clone();
            *b.get_mut(&addr).unwrap() = 0;
            let c = chain_of(&b, 0x1000);
            assert!(c.pins_ok() && c.res == 0 && !c.ok(), "zero at 0x{addr:x}: {c:?}");
        }
        // a zero desc data pointer is a count of 0
        let mut b = m.clone();
        *b.get_mut(&0x2108).unwrap() = 0;
        assert_eq!(chain_of(&b, 0x1000).count, 0);
        // no wrap-around near the top of the address space
        assert_eq!(chain41(usize::MAX - 8, BASE, &UI41_2720, &UI_LAYOUT_2720, &rd_of(&m)).res, 0);
    }

    #[test]
    fn c4_chain41_rtdata_check() {
        let m = chain_mem();
        let rd = rd_of(&m);
        let r = rtdata41(0x9000, &UI41_2720, &rd);
        assert_eq!(r, RtData41 { rt: 0xa000, data: 0xb000, fmt: 28, rtv: 0x7777 });
        let c = chain_of(&m, 0x1000);
        assert!(rtdata_match(&c, &r, 0), "no desc sampled yet: the RTV alone");
        assert!(rtdata_match(&c, &r, DXGI_R8G8B8A8_UNORM));
        assert!(!rtdata_match(&c, &r, DXGI_R10G10B10A2_UNORM), "format mismatch = recreated");
        assert!(!rtdata_match(&c, &RtData41 { rtv: 0x7778, ..r }, 0), "another RTV");
        assert!(!rtdata_match(&Chain41 { rtv: 0, ..c }, &RtData41 { rtv: 0, ..r }, 0), "two zeros never match");
        assert_eq!(rtdata41(0, &UI41_2720, &rd), RtData41::default());
        assert!(pins_line_ok41(&c, true, &r));
        assert!(!pins_line_ok41(&c, false, &r), "res outside the D3D12 module");
        assert!(!pins_line_ok41(&c, true, &RtData41 { rtv: 1, ..r }));
        assert_eq!(SCALEFORM_UI_2720.ui41, UI41_2720);
    }

    #[test]
    fn c4_fmt41_rules() {
        let d = TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: 28, samples: 1, flags: 1 };
        let bb = (3840, 2160, DXGI_R10G10B10A2_UNORM);
        assert!(size_eq41(&d, bb) && layer_ok41(&d, bb, (5, 5)));
        assert!(!layer_ok41(&d, bb, (4, 5)), "an rtData mismatch");
        assert!(!layer_ok41(&d, bb, (0, 0)), "no rtData check at all");
        for bad in [
            TexDesc { format: DXGI_R16G16B16A16_FLOAT, ..d },
            TexDesc { format: DXGI_B8G8R8A8_UNORM, ..d },
            TexDesc { mips: 2, ..d },
            TexDesc { samples: 4, ..d },
            TexDesc { width: 2880, ..d },
            TexDesc { flags: 0, ..d },
        ] {
            assert!(!layer_ok41(&bad, bb, (5, 5)), "{bad:?}");
        }
        let o = Output41 { b13c: 0, b13e: 1, eb9: 0, scale_x: 1.0, scale_y: 1.0, shaders: true };
        assert_eq!(blend41(&o), "sdr2sdr");
        assert!(!o.hdr() && o.scale_one());
        assert_eq!(blend41(&Output41 { scale_x: 0.75, ..o }), "sdr2sdr_upsample");
        assert_eq!(blend41(&Output41 { b13c: 1, ..o }), "hdr");
        assert_eq!(blend41(&Output41 { b13c: 1, eb9: 1, ..o }), "sdr2hdr");
        assert_eq!(blend41(&Output41 { b13c: 1, eb9: 1, scale_y: 0.5, ..o }), "sdr2hdr_upsample");
        assert_eq!(blend41(&Output41 { b13c: 1, scale_y: 0.5, ..o }), "hdr_upsample");
        assert_eq!(blend41(&Output41 { eb9: 1, ..o }), "sdr2sdr", "eb9 only matters on HDR output");
        assert!(!Output41 { scale_x: 1.5, ..o }.scale_one(), "the row wants exactly 1x1");
        assert!(rt_ok41(7, 7, 7) && !rt_ok41(7, 7, 8) && !rt_ok41(7, 8, 8) && !rt_ok41(0, 0, 0));
        assert!(present_in_ui41(5, 4, 5) && !present_in_ui41(5, 5, 5) && !present_in_ui41(4, 4, 5));
        assert!(ahead41(5, 5) && !ahead41(4, 5) && !ahead41(u64::MAX, u64::MAX));
    }

    #[test]
    fn c4_probe41_line_formats() {
        assert_eq!(fmt41_on(false), "probe41 on sample=0");
        let c = Chain41 { vt: true, vt7: true, sub: true, slot0: true, slot1: true, tex: true, arr: true, count: 1, obj: 0x40, res: 0xabc0, rtv: 0x77 };
        let r = RtData41 { rt: 1, data: 2, fmt: 28, rtv: 0x77 };
        assert_eq!(
            fmt41_pins(UiPath::Layer, &c, true, &r),
            "probe41 pins which=layer vt=1 vt7=1 sub=1 slot0=1 slot1=1 tex=1 arr=1 count=1 res=0xabc0 res_mod=1 rtv=0x77 rtdata_fmt=28 rtdata_rtv_eq=1"
        );
        assert!(fmt41_pins(UiPath::Scene, &Chain41 { slot1: false, rtv: 0x78, ..c }, false, &r).contains("which=scene vt=1 vt7=1 sub=1 slot0=1 slot1=0 "));
        assert!(fmt41_pins(UiPath::Scene, &Chain41 { rtv: 0x78, ..c }, false, &r).ends_with("rtdata_rtv_eq=0"));
        let t = Thread41 { mt: true, rt_tid: 21960, hal_tid: 21960, begin_tid: 21960, present_tid: 34032, rt_ok: 300, begins: 300, rtid_changes: 0, ahead: 0, present_in_ui: 0 };
        assert_eq!(
            fmt41_thread(&t),
            "probe41 thread mt=1 rt_tid=21960 hal_tid=21960 begin_tid=21960 present_tid=34032 rt_ok=300/300 rtid_changes=0 ahead=0 present_in_ui=0"
        );
        let d = TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: 28, samples: 1, flags: 0x1 };
        let bb = (3840, 2160, DXGI_R10G10B10A2_UNORM);
        assert_eq!(
            fmt41_target(UiPath::Layer, 0xabc0, &d, bb, 0, Some((299, 299))),
            "probe41 layer res=0xabc0 fmt=28:R8G8B8A8_UNORM w=3840 h=2160 mips=1 samples=1 flags=0x1 rt_ok=1 bb=3840x2160:R10G10B10A2_UNORM size_eq=1 res_changes=0 rtdata_ok=299/299"
        );
        assert!(fmt41_target(UiPath::Scene, 0x1, &TexDesc { format: 24, ..d }, bb, 1, None).starts_with("probe41 scene res=0x1 fmt=24:R10G10B10A2_UNORM "));
        assert!(fmt41_target(UiPath::Scene, 0x1, &d, bb, 1, None).ends_with("res_changes=1 rtdata_ok=n/a"));
        let o = Output41 { b13c: 0, b13e: 1, eb9: 0, scale_x: 1.0, scale_y: 1.0, shaders: true };
        assert_eq!(fmt41_output(&o), "probe41 output b13c=0 b13e=1 eb9=0 scale=1x1 shaders=1 blend=sdr2sdr");
        assert_eq!(fmt41_output(&Output41 { b13c: 1, eb9: 3, scale_x: 0.75, scale_y: 0.75, shaders: false, ..o }), "probe41 output b13c=1 b13e=1 eb9=1 scale=0.75x0.75 shaders=0 blend=sdr2hdr_upsample");
        assert_eq!(fmt41_between(1, 1, 0x7ff2_0000_1000), "probe41 between n=1 frame=1 list=0x7ff200001000");
        assert_eq!(fmt41_recreate("res", UiPath::Layer, 0x10, 0x20, 61.34), "probe41 recreate what=res which=layer old=0x10 new=0x20 t=61.3");
        let mut tl = AlphaTally::default();
        for _ in 0..12 {
            tl.add([0, 0, 0, 255]);
        }
        tl.add([10, 0, 0, 255]);
        assert_eq!(
            fmt41_sample(1, 12, &tl, &[[0, 0, 0, 255], [10, 0, 0, 255], [0, 0, 0, 255], [0, 0, 0, 255], [9, 9, 9, 9]]),
            "probe41 sample n=1 empty=12/16 a255=12 a0=0 other=0 conv=unknown px=0,0,0,255;10,0,0,255;0,0,0,255;0,0,0,255"
        );
        assert_eq!(fmt41_sample_off("config"), "probe41 sample off why=config");
        // the C4 analyzer keys on "UI probe " (with a space): no C4.1 line may start that way
        for l in [fmt41_on(true), fmt41_thread(&t), fmt41_output(&o), fmt41_verdict(&ProbeStats41::default())] {
            assert!(l.starts_with("probe41 "), "{l}");
        }
    }

    fn ready41() -> ProbeStats41 {
        ProbeStats41 {
            hooks_c4: true,
            on: true,
            thread_lines: 12,
            begins: 3600,
            rt_ok: 3600,
            present_tid: 34032,
            armed: 3500,
            armed_hit: 3500,
            armed_n_max: 1,
            between_other: 1,
            between_frame_max: 1,
            pins_lines: 1,
            res_distinct: 1,
            layer_lines: 12,
            output_seen: true,
            output: Output41 { b13c: 0, b13e: 1, eb9: 0, scale_x: 1.0, scale_y: 1.0, shaders: true },
            alpha: Some(LayerAlpha::Transmit),
            ..ProbeStats41::default()
        }
    }

    #[test]
    fn c4_probe41_verdict_rows() {
        let s = ready41();
        assert_eq!(verdict41(&s), (true, vec![], "sdr", Some(LayerAlpha::Transmit)));
        assert_eq!(fmt41_verdict(&s), "probe41 verdict ready=1 why=none output=sdr alpha=transmit");
        let o = s.output;
        let cases: [(ProbeStats41, &str); 21] = [
            (ProbeStats41 { on: false, ..s }, "hooks41"),
            (ProbeStats41 { hooks_c4: false, ..s }, "hooks41"),
            (ProbeStats41 { mt_bad_lines: 1, ..s }, "thread41"),
            (ProbeStats41 { rt_ok: 3599, ..s }, "thread41"),
            (ProbeStats41 { rtid_changes: 1, ..s }, "thread41"),
            (ProbeStats41 { present_tid: 0, ..s }, "thread41"),
            (ProbeStats41 { ahead: 3, ..s }, "order"),
            (ProbeStats41 { present_in_ui: 1, ..s }, "order"),
            (ProbeStats41 { late: 1, ..s }, "order"),
            (ProbeStats41 { armed_hit: 3499, ..s }, "order"),
            (ProbeStats41 { armed_idx_max: 1, ..s }, "order"),
            (ProbeStats41 { armed_n_max: 2, ..s }, "order"),
            (ProbeStats41 { between_frame_max: 900, ..s }, "adjacency41"),
            (ProbeStats41 { between_other: 2, ..s }, "adjacency41"),
            (ProbeStats41 { pins_bad: 1, ..s }, "pins"),
            (ProbeStats41 { res_distinct: 3, ..s }, "pins"),
            (ProbeStats41 { layer_bad: 1, ..s }, "layer"),
            (ProbeStats41 { output: Output41 { scale_x: 0.75, scale_y: 0.75, ..o }, ..s }, "output"),
            (ProbeStats41 { output: Output41 { shaders: false, ..o }, ..s }, "output"),
            (ProbeStats41 { output_seen: false, ..s }, "output"),
            (ProbeStats41 { alpha: None, ..s }, "alpha"),
        ];
        for (c, row) in cases {
            let (ready, why, _, _) = verdict41(&c);
            assert!(!ready && why == vec![row], "{row}: {why:?}");
        }
        // no between event at all is fine; HDR output passes (reported as needing inject_ui_layer)
        assert!(verdict41(&ProbeStats41 { between_other: 0, between_frame_max: 0, ..s }).0);
        let (r, _, out, _) = verdict41(&ProbeStats41 { output: Output41 { b13c: 1, ..o }, ..s });
        assert!(r && out == "hdr");
        assert!(verdict41(&ProbeStats41 { res_distinct: 2, alpha: Some(LayerAlpha::Premul), ..s }).0, "one recreate");
        // without P2 the run can never be ready (design S2 task 4)
        assert_eq!(fmt41_verdict(&ProbeStats41 { alpha: None, ..s }), "probe41 verdict ready=0 why=alpha output=sdr alpha=unknown");
        let (r, why, _, _) = verdict41(&ProbeStats41::default());
        assert!(!r && why == ROWS41.iter().filter(|r| **r != "adjacency41").copied().collect::<Vec<_>>(), "{why:?}");
    }

    #[test]
    fn c4_alpha_vote() {
        assert_eq!(conv_vote(47, 47, 0), None, "too few empty points");
        assert_eq!(conv_vote(48, 48, 0), Some(LayerAlpha::Transmit));
        assert_eq!(conv_vote(48, 0, 48), Some(LayerAlpha::Premul));
        assert_eq!(conv_vote(100, 90, 10), Some(LayerAlpha::Transmit), "exactly 90 %");
        assert_eq!(conv_vote(100, 89, 11), None);
        assert_eq!(conv_vote(100, 50, 50), None);
        assert_eq!(conv_vote(100, 5, 91), Some(LayerAlpha::Premul));
        let mut t = AlphaTally::default();
        t.add([1, 0, 0, 0]);
        t.add([0, 0, 1, 255]);
        assert_eq!(t, AlphaTally::default(), "non-empty points never vote");
        for i in 0..64 {
            t.add(if i % 16 == 0 { [0, 0, 0, 128] } else { [0, 0, 0, 255] });
        }
        assert_eq!((t.empty, t.a255, t.a0, t.other), (64, 60, 0, 4));
        assert_eq!(t.conv(), Some(LayerAlpha::Transmit));
        assert_eq!(empty_alpha([0, 0, 0, 7]), Some(7));
        assert_eq!(empty_alpha([0, 1, 0, 7]), None);
        assert_eq!((alpha_label(None), alpha_label(Some(LayerAlpha::Premul))), ("unknown", "premul"));
    }

    #[test]
    fn c4_sample_points_grid() {
        let p = sample_points41(3840, 2160);
        assert_eq!(p[0], [1536, 648]);
        assert_eq!(p[3], [2304, 648]);
        assert_eq!(p[12], [1536, 907]);
        assert_eq!(p[15], [2304, 907]);
        // row-major, strictly increasing x within a row and y down the columns
        for r in 0..4 {
            for c in 1..4 {
                assert!(p[r * 4 + c][0] > p[r * 4 + c - 1][0] && p[r * 4 + c][1] == p[r * 4][1]);
            }
        }
        // inside the 0.40..0.60 x 0.30..0.42 window at every size, and inside a tiny texture
        for (w, h) in [(1920u32, 1080u32), (2560, 1440), (1280, 720)] {
            for q in sample_points41(w, h) {
                assert!(q[0] as f64 >= 0.40 * w as f64 - 0.5 && q[0] as f64 <= 0.60 * w as f64 + 0.5, "{w} {q:?}");
                assert!(q[1] as f64 >= 0.30 * h as f64 - 0.5 && q[1] as f64 <= 0.42 * h as f64 + 0.5, "{h} {q:?}");
            }
        }
        assert!(sample_points41(1, 1).iter().all(|q| *q == [0, 0]));
        assert_eq!(unpack_px(0xff00_0000), [0, 0, 0, 255]);
        assert_eq!(unpack_px(0x0403_0201), [1, 2, 3, 4]);
    }

    fn sample_in() -> SampleIn {
        SampleIn {
            ready: true,
            due: true,
            idle: true,
            hal: 0x10,
            movies: 2,
            list: 0x20,
            layer_path: true,
            mt: true,
            rt_ok: true,
            queue_ok: true,
            chain_ok: true,
            res_mod: true,
            desc: Some(TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: DXGI_R8G8B8A8_UNORM, samples: 1, flags: 0x1 }),
            bb: (3840, 2160, DXGI_R10G10B10A2_UNORM),
        }
    }

    #[test]
    fn c4_sample_gate_every_reason() {
        let s = sample_in();
        assert_eq!(sample_gate41(&s), Ok((DXGI_R8G8B8A8_UNORM, 3840, 2160)));
        let d = s.desc.unwrap();
        assert_eq!(sample_gate41(&SampleIn { desc: Some(TexDesc { format: DXGI_B8G8R8A8_UNORM, ..d }), ..s }), Ok((DXGI_B8G8R8A8_UNORM, 3840, 2160)));
        let cases: [(SampleIn, SampleMiss); 19] = [
            (SampleIn { ready: false, ..s }, SampleMiss::NotDue),
            (SampleIn { due: false, ..s }, SampleMiss::NotDue),
            (SampleIn { idle: false, ..s }, SampleMiss::Busy),
            (SampleIn { hal: 0, ..s }, SampleMiss::NoFrame),
            (SampleIn { movies: 0, ..s }, SampleMiss::NoFrame),
            (SampleIn { list: 0, ..s }, SampleMiss::NoFrame),
            (SampleIn { layer_path: false, ..s }, SampleMiss::NoFrame),
            (SampleIn { mt: false, ..s }, SampleMiss::Thread),
            (SampleIn { rt_ok: false, ..s }, SampleMiss::Thread),
            (SampleIn { queue_ok: false, ..s }, SampleMiss::Queue),
            (SampleIn { chain_ok: false, ..s }, SampleMiss::Chain),
            (SampleIn { res_mod: false, ..s }, SampleMiss::Chain),
            (SampleIn { desc: None, ..s }, SampleMiss::Desc),
            (SampleIn { desc: Some(TexDesc { mips: 2, ..d }), ..s }, SampleMiss::Desc),
            (SampleIn { desc: Some(TexDesc { samples: 4, ..d }), ..s }, SampleMiss::Desc),
            (SampleIn { desc: Some(TexDesc { width: 1920, ..d }), ..s }, SampleMiss::Desc),
            (SampleIn { desc: Some(TexDesc { format: DXGI_R16G16B16A16_FLOAT, ..d }), ..s }, SampleMiss::Format),
            (SampleIn { desc: Some(TexDesc { format: DXGI_R8G8B8A8_UNORM_SRGB, ..d }), ..s }, SampleMiss::Format),
            (SampleIn { desc: Some(TexDesc { format: DXGI_R8G8B8A8_TYPELESS, ..d }), ..s }, SampleMiss::Format),
        ];
        for (c, want) in cases {
            assert_eq!(sample_gate41(&c), Err(want), "{c:?}");
        }
        assert_eq!(SampleMiss::Format.label(), "format");
    }

    #[test]
    fn c4_sample_state_machine() {
        for st in [SampleState::Idle, SampleState::Armed(7), SampleState::Submitted(u64::MAX >> 2), SampleState::Armed(0)] {
            assert_eq!(sample_unpack(sample_pack(st)), st);
        }
        assert_eq!(sample_pack(SampleState::Idle), 0, "a zeroed atomic is Idle");
        // splice only the armed frame's own seq, only at index 0
        assert_eq!(sample_ecl41(SampleState::Armed(5), 5, Some(0)), SampleEcl::Splice);
        assert_eq!(sample_ecl41(SampleState::Armed(5), 5, Some(1)), SampleEcl::Cancel);
        assert_eq!(sample_ecl41(SampleState::Armed(5), 5, None), SampleEcl::Pass);
        assert_eq!(sample_ecl41(SampleState::Armed(5), 6, Some(0)), SampleEcl::Pass, "an older Present's armed list");
        assert_eq!(sample_ecl41(SampleState::Idle, 5, Some(0)), SampleEcl::Pass);
        assert_eq!(sample_ecl41(SampleState::Submitted(9), 5, Some(0)), SampleEcl::Pass, "never twice");
        // UiEnd cancels only an unsubmitted sample
        assert_eq!(sample_ui_end41(SampleState::Armed(5)), Some(SampleState::Idle));
        assert_eq!(sample_ui_end41(SampleState::Submitted(9)), None);
        assert_eq!(sample_ui_end41(SampleState::Idle), None);
        // the readback waits for the fence
        assert!(!sample_ready41(SampleState::Submitted(9), 8));
        assert!(sample_ready41(SampleState::Submitted(9), 9));
        assert!(!sample_ready41(SampleState::Armed(9), 100) && !sample_ready41(SampleState::Idle, 100));
        // exhaustive: from every state, every event sequence keeps "recorded only from Idle, read only once done"
        let events = [0u8, 1, 2, 3];
        for a in events {
            for b in events {
                for c in events {
                    let (mut st, mut submitted, mut read) = (SampleState::Idle, 0u32, 0u32);
                    for ev in [a, b, c] {
                        st = match ev {
                            0 if st == SampleState::Idle => SampleState::Armed(1),
                            1 => match sample_ecl41(st, 1, Some(0)) {
                                SampleEcl::Splice => {
                                    submitted += 1;
                                    SampleState::Submitted(submitted as u64)
                                }
                                _ => st,
                            },
                            2 => sample_ui_end41(st).unwrap_or(st),
                            3 if sample_ready41(st, u64::MAX) => {
                                read += 1;
                                SampleState::Idle
                            }
                            _ => st,
                        };
                    }
                    assert!(read <= submitted, "{a}{b}{c}");
                }
            }
        }
    }

    #[test]
    fn c4_alpha_sample_verdict() {
        let mut t = AlphaTally::default();
        for _ in 0..48 {
            t.add([0, 0, 0, 0]);
        }
        assert_eq!(sample_alpha41(&t, false), Some(LayerAlpha::Premul));
        assert_eq!(sample_alpha41(&t, true), None, "a P2 that failed on ER's queue never pins a convention");
        // the verdict takes the P2 convention as its alpha row
        let s = ProbeStats41 { alpha: sample_alpha41(&t, false), ..ready41() };
        assert_eq!(fmt41_verdict(&s), "probe41 verdict ready=1 why=none output=sdr alpha=premul");
        assert!(sample_format_ok41(DXGI_R8G8B8A8_UNORM) && sample_format_ok41(DXGI_B8G8R8A8_UNORM));
        assert!(!sample_format_ok41(DXGI_R8G8B8A8_UNORM_SRGB) && !sample_format_ok41(DXGI_R10G10B10A2_UNORM));
        assert_eq!(fmt41_sample_off("hresult"), "probe41 sample off why=hresult");
    }

    // ---- C4.1 PASS I41 ----

    fn good_chain() -> Chain41 {
        Chain41 { vt: true, vt7: true, sub: true, slot0: true, slot1: true, tex: true, arr: true, count: 1, obj: 0x40, res: 0xabc0, rtv: 0x77 }
    }

    fn good_gate41() -> GateIn41 {
        GateIn41 {
            enabled: true,
            build: true,
            hal: 0x1000,
            movies: 3,
            vt_ok: true,
            hal_flags: 3,
            mt: true,
            tid: 6012,
            hal_tid: 6012,
            rt_tid: 6012,
            present_tid: 26856,
            had_good: true,
            ahead: false,
            first: true,
            wants: Wants::Gun,
            layer_path: true,
            sel: 0x5000,
            chain: good_chain(),
            module: Some(true),
            res: Ok(()),
            output: Output41 { b13c: 0, b13e: 1, eb9: 0, scale_x: 1.0, scale_y: 1.0, shaders: true },
            hdr_ok: false,
            target: Ok((DXGI_R8G8B8A8_UNORM, 3840, 2160)),
            alpha: Some(LayerAlpha::Transmit),
            canary: CanaryGate::Passed,
            warm: true,
            device_ok: true,
            slot_free: true,
            depth_ready: true,
            pso_ready: true,
        }
    }

    #[test]
    fn c4_gate41_every_reason() {
        let g = good_gate41();
        let gun = Early41 { canary: false, check: false, mode: EarlyMode::Transmit, format: DXGI_R8G8B8A8_UNORM, width: 3840, height: 2160 };
        assert_eq!(gate41(&g), Ok(gun));
        assert_eq!(gate41(&GateIn41 { wants: Wants::Overlay, ..g }), Ok(gun));
        assert_eq!(gate41(&GateIn41 { canary: CanaryGate::Due, ..g }), Ok(Early41 { canary: true, ..gun }), "the canary frame records");
        assert_eq!(gate41(&GateIn41 { canary: CanaryGate::Check, ..g }), Ok(Early41 { check: true, ..gun }), "the persistence check records (no gun)");
        assert_eq!(gate41(&GateIn41 { canary: CanaryGate::Check, wants: Wants::Overlay, ..g }), Ok(Early41 { check: true, ..gun }), "a scope frame checks too");
        assert_eq!(gate41(&GateIn41 { alpha: Some(LayerAlpha::Premul), ..g }).map(|e| e.mode), Ok(EarlyMode::Premul));
        // the present thread is NOT the render thread on this PC (the C4 rule's refutation): C4.1 does not care
        assert!(gate41(&GateIn41 { present_tid: 6012, ..g }).is_ok());
        let o = g.output;
        let chain = good_chain();
        let cases = [
            (GateIn41 { enabled: false, ..g }, Miss::Config),
            (GateIn41 { build: false, ..g }, Miss::Build),
            (GateIn41 { hal: 0, ..g }, Miss::NoHal),
            (GateIn41 { movies: 0, ..g }, Miss::NoMovies),
            (GateIn41 { vt_ok: false, ..g }, Miss::Vtable),
            (GateIn41 { hal_flags: 1, ..g }, Miss::Flags),
            (GateIn41 { mt: false, ..g }, Miss::StMode),
            (GateIn41 { tid: 1, ..g }, Miss::Thread),
            (GateIn41 { rt_tid: 1, ..g }, Miss::Thread),
            (GateIn41 { tid: 1, had_good: false, ..g }, Miss::NotReady),
            (GateIn41 { rt_tid: 0, ..g }, Miss::NotReady),
            (GateIn41 { present_tid: 0, ..g }, Miss::NotReady),
            (GateIn41 { ahead: true, ..g }, Miss::Ahead),
            (GateIn41 { first: false, ..g }, Miss::SecondBegin),
            (GateIn41 { wants: Wants::Nothing, ..g }, Miss::Hidden),
            (GateIn41 { wants: Wants::Ac130, ..g }, Miss::Ac130),
            (GateIn41 { wants: Wants::NotFp, ..g }, Miss::NotFp),
            (GateIn41 { wants: Wants::ScopedNoOverlay, ..g }, Miss::ScopedNoOverlay),
            (GateIn41 { wants: Wants::NotReady, ..g }, Miss::NotReady),
            (GateIn41 { layer_path: false, ..g }, Miss::NoTarget),
            (GateIn41 { sel: 0, ..g }, Miss::NoTarget),
            (GateIn41 { chain: Chain41 { slot1: false, ..chain }, ..g }, Miss::Pins),
            (GateIn41 { chain: Chain41 { slot1: false, ..chain }, had_good: false, ..g }, Miss::NoTarget),
            (GateIn41 { chain: Chain41 { count: 0, ..chain }, ..g }, Miss::NoTarget),
            (GateIn41 { chain: Chain41 { res: 0, ..chain }, ..g }, Miss::NoTarget),
            (GateIn41 { module: None, ..g }, Miss::NotReady),
            (GateIn41 { module: Some(false), ..g }, Miss::Module),
            (GateIn41 { res: Err(Miss::Recreated), ..g }, Miss::Recreated),
            (GateIn41 { output: Output41 { scale_x: 0.75, ..o }, ..g }, Miss::Scale),
            (GateIn41 { output: Output41 { shaders: false, ..o }, ..g }, Miss::Scale),
            (GateIn41 { output: Output41 { b13c: 1, ..o }, ..g }, Miss::HdrOff),
            (GateIn41 { target: Err(Miss::Size), ..g }, Miss::Size),
            (GateIn41 { target: Err(Miss::Format), ..g }, Miss::Format),
            (GateIn41 { alpha: None, ..g }, Miss::AlphaUnknown),
            (GateIn41 { warm: false, ..g }, Miss::Warmup),
            (GateIn41 { canary: CanaryGate::InFlight, ..g }, Miss::Canary),
            (GateIn41 { device_ok: false, ..g }, Miss::Device),
            (GateIn41 { slot_free: false, ..g }, Miss::Busy),
            (GateIn41 { depth_ready: false, ..g }, Miss::Size),
            (GateIn41 { pso_ready: false, ..g }, Miss::PsoPending),
        ];
        for (i, want) in cases {
            assert_eq!(gate41(&i), Err(want), "{i:?}");
        }
        // HDR output with inject_ui_layer (its C4.1 meaning) passes
        assert!(gate41(&GateIn41 { output: Output41 { b13c: 1, ..o }, hdr_ok: true, ..g }).is_ok());
        // the cheap / expected rows come first: no movies never trips on a bad vtable, a missed thread never on a pin
        assert_eq!(gate41(&GateIn41 { movies: 0, vt_ok: false, ..g }), Err(Miss::NoMovies));
        assert_eq!(gate41(&GateIn41 { mt: false, tid: 1, ..g }), Err(Miss::StMode));
        assert_eq!(gate41(&GateIn41 { wants: Wants::Ac130, chain: Chain41 { vt: false, ..chain }, ..g }), Err(Miss::Ac130), "AC-130 frames never look at the chain");
        assert_eq!(gate41(&GateIn41 { alpha: None, canary: CanaryGate::Due, ..g }), Err(Miss::AlphaUnknown), "no canary without a convention");
        // every new reason has a label and its trip class (design 4)
        for (m, trip) in [(Miss::Pins, true), (Miss::Order, true), (Miss::Alpha, true), (Miss::Cleared, true), (Miss::StMode, false), (Miss::Warmup, false)] {
            assert_eq!(m.is_trip(), trip, "{m:?}");
        }
    }

    #[test]
    fn c4_gate41_layer_rule_and_recreate() {
        let d = TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: DXGI_R8G8B8A8_UNORM, samples: 1, flags: 1 };
        let bb = (3840, 2160, DXGI_R10G10B10A2_UNORM);
        assert_eq!(layer_rule41(&d, bb), Ok((DXGI_R8G8B8A8_UNORM, 3840, 2160)));
        assert_eq!(layer_rule41(&TexDesc { format: DXGI_B8G8R8A8_UNORM, ..d }, bb), Ok((DXGI_B8G8R8A8_UNORM, 3840, 2160)));
        for (bad, want) in [
            (TexDesc { format: DXGI_R16G16B16A16_FLOAT, ..d }, Miss::Format),
            (TexDesc { format: DXGI_R8G8B8A8_UNORM_SRGB, ..d }, Miss::Format),
            (TexDesc { format: DXGI_R10G10B10A2_UNORM, ..d }, Miss::Format),
            (TexDesc { mips: 2, ..d }, Miss::Format),
            (TexDesc { samples: 4, ..d }, Miss::Format),
            (TexDesc { flags: 0, ..d }, Miss::Format),
            (TexDesc { width: 1920, ..d }, Miss::Size),
        ] {
            assert_eq!(layer_rule41(&bad, bb), Err(want), "{bad:?}");
        }
        assert_eq!(layer_rule41(&d, (0, 0, 0)), Err(Miss::Size), "no Present seen yet");
        // a new resource is used only after two UiBegins in a row with the rtData check passing
        let p = ResPin41::default();
        let (p, r, ch) = res_step41(p, 0x10, true);
        assert_eq!((r, ch), (Err(Miss::Recreated), false), "the first sight");
        let (p, r, _) = res_step41(p, 0x10, true);
        assert_eq!((r, p.good), (Ok(()), 0x10));
        let (p, r, _) = res_step41(p, 0x10, true);
        assert_eq!(r, Ok(()));
        // recreated at a new address: two misses? no, one miss, then used
        let (p, r, ch) = res_step41(p, 0x20, true);
        assert_eq!((r, ch), (Err(Miss::Recreated), true));
        let (p2, r, ch) = res_step41(p, 0x20, true);
        assert_eq!((r, ch, p2.good), (Ok(()), false, 0x20));
        // an rtData mismatch resets the candidate (even for the good resource)
        let (p3, r, _) = res_step41(p, 0x20, false);
        assert_eq!((r, p3.cand_n), (Err(Miss::Recreated), 0));
        assert_eq!(res_step41(p2, 0x20, false).1, Err(Miss::Recreated), "same address, other rtData: recreated");
        // flapping between two addresses never settles
        let (mut q, mut oks) = (p2, 0);
        for i in 0..10 {
            let (n, r, _) = res_step41(q, if i % 2 == 0 { 0x30 } else { 0x40 }, true);
            q = n;
            oks += r.is_ok() as u32;
        }
        assert_eq!(oks, 0);
        assert_eq!(res_step41(p2, 0, true).1, Err(Miss::NoTarget));
        // format_flap without allocation: 3 changes in 10 s trip
        let mut f = FlapRing::new();
        assert!(!f.note(1.0) && !f.note(4.0) && f.note(9.0));
        let mut f = FlapRing::new();
        assert!(!f.note(1.0) && !f.note(6.0) && !f.note(12.0) && !f.note(30.0));
        assert!(!f.note(31.0) && f.note(32.0), "the last three within 10 s");
    }

    #[test]
    fn c4_state_present_in_ui_trips() {
        let s = 9u64;
        let sub = pack(Kind::Submitted, 1, s);
        // outside a UI frame: exactly C4's rule
        assert_eq!(present41(sub, s, false), (PresentDecision { early: true, cancelled: false }, pack(Kind::Closed, 0, s), false));
        // inside a UI frame with our list spliced: Present draws anyway and trips `order` (never neither)
        assert_eq!(present41(sub, s, true), (PresentDecision { early: false, cancelled: false }, pack(Kind::Closed, 0, s), true));
        // an Armed frame there is cancelled as today, no trip
        assert_eq!(present41(armed(s, 2), s, true), (PresentDecision { early: false, cancelled: true }, pack(Kind::Closed, 0, s), false));
        // an older Present's Submitted is not this frame's gun, open or not
        assert_eq!(present41(pack(Kind::Submitted, 0, s - 1), s, true).0, PresentDecision { early: false, cancelled: false });
        assert!(!present41(pack(Kind::Submitted, 0, s - 1), s, true).2);
    }

    #[test]
    fn c4_state_ahead_refused() {
        // design 3.4: frame N's UiBegin missed (state Closed(s-2)), seq s-1; UiBegin(N+1) arrives before Present(N)'s
        // increment and reads s-1 again
        let s = 20u64;
        let st = pack(Kind::Closed, 0, s - 2);
        assert!(begin_ok(st, s - 1), "without `ahead` the state machine alone would arm for s-1");
        assert!(ahead41(s - 1, s - 1), "UiBegin(N+1) read the seq UiBegin(N) read");
        let g = GateIn41 { ahead: ahead41(s - 1, s - 1), ..good_gate41() };
        assert_eq!(gate41(&g), Err(Miss::Ahead));
        // Present(N) then draws at Present and closes s-1; UiBegin(N+2) reads s: not ahead, arms
        let (d, st, _) = present41(st, s - 1, false);
        assert!(!d.early);
        assert!(!ahead41(s - 1, s) && begin_ok(st, s));
        // the very first UiBegin of a session is never ahead
        assert!(!ahead41(u64::MAX, 0));
    }

    /// One run of the two-thread model: `order` = the merged event order (true = a Present-thread event), `pass` /
    /// `ecl` per UI frame (the gate passed / the armed list reached an ECL). Returns per presented seq (spliced, drew,
    /// order trip).
    fn run_mt(order: &[bool], pass: u8, ecl: u8) -> Vec<(u64, u32, bool, bool)> {
        let mut seq = 10u64;
        let mut st = 0u64;
        let mut last_begin = u64::MAX;
        let (mut begin_seq, mut end_seq) = (u64::MAX, u64::MAX);
        let mut spliced: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
        let mut out = Vec::new();
        let (mut pi, mut ri) = (0usize, 0usize);
        for &present_thread in order {
            if present_thread {
                if pi % 2 == 0 {
                    // the Present callback (before hudhook's PRESENT_SEQ++)
                    let open = present_in_ui41(begin_seq, end_seq, seq);
                    let (d, next, trip) = present41(st, seq, open);
                    st = next;
                    let n = spliced.get(&seq).copied().unwrap_or(0);
                    out.push((seq, n, !d.early, trip));
                } else {
                    seq += 1;
                }
                pi += 1;
            } else {
                let u = ri / 3;
                match ri % 3 {
                    0 => {
                        // UiBegin: reads the seq, the gate (with `ahead`), arms
                        let ahead = ahead41(last_begin, seq);
                        last_begin = seq;
                        begin_seq = seq;
                        if pass >> u & 1 != 0 && !ahead && begin_ok(st, seq) {
                            st = armed(seq, (u % 3) as u8);
                        }
                    }
                    1 => {
                        // the ECL of the armed list (if the UI frame submits it)
                        if ecl >> u & 1 != 0
                            && let Some(n) = ecl_submit(st, seq)
                        {
                            st = n;
                            *spliced.entry(seq).or_insert(0) += 1;
                        }
                    }
                    _ => {
                        // UiEnd
                        if let Some(n) = ui_end(st, seq) {
                            st = n;
                        }
                        end_seq = seq;
                    }
                }
                ri += 1;
            }
        }
        out
    }

    #[test]
    fn c4_state_mt_two_threads() {
        // 3 Presents (callback, seq++) on the present thread, 3 UI frames (UiBegin, ECL, UiEnd) on ER's render thread,
        // every interleaving of the two programs, every gate pass / ECL pattern
        const F: usize = 3;
        let (np, nr) = (2 * F, 3 * F);
        let mut orders: Vec<Vec<bool>> = Vec::new();
        fn interleave(np: usize, nr: usize, cur: &mut Vec<bool>, out: &mut Vec<Vec<bool>>) {
            if np == 0 && nr == 0 {
                out.push(cur.clone());
                return;
            }
            if np > 0 {
                cur.push(true);
                interleave(np - 1, nr, cur, out);
                cur.pop();
            }
            if nr > 0 {
                cur.push(false);
                interleave(np, nr - 1, cur, out);
                cur.pop();
            }
        }
        interleave(np, nr, &mut Vec::new(), &mut orders);
        assert_eq!(orders.len(), 5005);
        let (mut runs, mut early_seen, mut trips) = (0u64, 0u64, 0u64);
        for o in &orders {
            for pass in 0..(1u8 << F) {
                for ecl in 0..(1u8 << F) {
                    for (seq, spliced, drew, trip) in run_mt(o, pass, ecl) {
                        // each presented image gets the gun exactly once: our list for its seq, or Present; both only
                        // when Present came inside the UI frame after the splice, which trips `order`
                        assert!(spliced <= 1, "{o:?} {pass} {ecl}: seq {seq} spliced {spliced} times");
                        if trip {
                            assert!(drew && spliced == 1, "{o:?} {pass} {ecl}: an order trip without a splice");
                            trips += 1;
                        } else {
                            assert_eq!(spliced + drew as u32, 1, "{o:?} {pass} {ecl}: seq {seq} spliced {spliced} drew {drew}");
                        }
                        early_seen += (!drew) as u64;
                    }
                    runs += 1;
                }
            }
        }
        assert_eq!(runs, 5005 * 64);
        assert!(early_seen > 0 && trips > 0, "both outcomes are reached ({early_seen} early, {trips} order trips)");
        // the natural order (UI frame, then its Present) draws every frame early
        let natural: Vec<bool> = (0..F).flat_map(|_| [false, false, false, true, true]).collect();
        assert!(run_mt(&natural, 0b111, 0b111).iter().all(|(_, n, drew, trip)| *n == 1 && !drew && !trip));
        // a missed gate or an armed list that never reaches an ECL: that frame at Present
        let r = run_mt(&natural, 0b101, 0b111);
        assert_eq!(r.iter().map(|x| x.2).collect::<Vec<_>>(), vec![false, true, false]);
        let r = run_mt(&natural, 0b111, 0b011);
        assert_eq!(r.iter().map(|x| x.2).collect::<Vec<_>>(), vec![false, false, true]);
    }

    #[test]
    fn c4_alpha_variants_composite() {
        let scene = [0.2, 0.4, 0.6];
        let gun = [0.9, 0.5, 0.1];
        let ui = [0.0, 0.3, 1.0];
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|k| a[k] * t + b[k] * (1.0 - t));
        for conv in [LayerAlpha::Transmit, LayerAlpha::Premul] {
            let mode = EarlyMode::of(conv);
            let e = empty_px41(conv);
            assert!(close(composite41(conv, scene, e), scene), "{conv:?}: an empty layer shows the scene");
            // the opaque gun shows opaque (its colour exactly)
            let g = layer_write41(mode, e, gun, 1.0, true);
            assert!(close(composite41(conv, scene, g), gun), "{conv:?} opaque");
            // a translucent gun texel (alpha PSO): gun over scene
            let h = layer_write41(mode, e, gun, 0.25, false);
            assert!(close(composite41(conv, scene, h), mix(gun, scene, 0.25)), "{conv:?} alpha");
            // ER's UI drawn over the gun in the same layer: UI over gun (the point of C4.1)
            for b in [1.0f32, 0.5, 0.1] {
                let u = ui_over41(conv, g, ui, b);
                assert!(close(composite41(conv, scene, u), mix(ui, gun, b)), "{conv:?} ui {b} over the opaque gun");
                let u = ui_over41(conv, h, ui, b);
                assert!(close(composite41(conv, scene, u), mix(ui, mix(gun, scene, 0.25), b)), "{conv:?} ui {b} over the alpha gun");
            }
            // the scope overlay (black, alpha) is the same math: scene darkened by its coverage
            let ov = layer_write41(mode, e, [0.0; 3], 0.8, false);
            assert!(close(composite41(conv, scene, ov), scene.map(|c| c * 0.2)), "{conv:?} overlay");
            // two alpha draws compose like "over" twice
            let two = layer_write41(mode, h, gun, 0.5, false);
            assert!(close(composite41(conv, scene, two), mix(gun, mix(gun, scene, 0.25), 0.5)), "{conv:?} alpha twice");
            // the wrong convention gives a ghost / black gun: why it is pinned per build and re-checked
            let other = if conv == LayerAlpha::Transmit { LayerAlpha::Premul } else { LayerAlpha::Transmit };
            let wrong = layer_write41(EarlyMode::of(other), e, gun, 1.0, true);
            assert!(!close(composite41(conv, scene, wrong), gun), "{conv:?}: the other convention's gun is wrong");
        }
        // the scene mode never touches alpha
        assert_eq!(layer_write41(EarlyMode::Scene, [0.1, 0.1, 0.1, 0.3], gun, 1.0, true)[3], 0.3);
        assert_eq!(layer_write41(EarlyMode::Transmit, [0.0, 0.0, 0.0, 1.0], gun, 1.0, true)[3], 0.0, "transmit opaque A = 0");
        assert_eq!(layer_write41(EarlyMode::Premul, [0.0; 4], gun, 1.0, true)[3], 1.0, "premul opaque A = 1");
        for m in [EarlyMode::Scene, EarlyMode::Premul, EarlyMode::Transmit] {
            assert_eq!(EarlyMode::from_code(m.code()), Some(m));
        }
        assert_eq!(EarlyMode::from_code(3), None);
        assert_eq!((EarlyMode::Transmit.label(), EarlyMode::of(LayerAlpha::Premul)), ("transmit", EarlyMode::Premul));
    }

    #[test]
    fn c4_canary_gone41_persistence() {
        // transmit: empty = (0,0,0,255), canary a = 0; premul: empty = (0,0,0,0), canary a = 255
        assert_eq!(canary_gone41([0, 0, 0, 255], LayerAlpha::Transmit), CanaryGone::Gone);
        assert_eq!(canary_gone41([0, 0, 0, 0], LayerAlpha::Premul), CanaryGone::Gone);
        for conv in [LayerAlpha::Transmit, LayerAlpha::Premul] {
            assert_eq!(canary_gone41(to_px(empty_px41(conv)), conv), CanaryGone::Gone);
            assert_eq!(canary_gone41(canary_px41(conv), conv), CanaryGone::Persisted, "ER never cleared the texel");
            assert_eq!(canary_gone41([200, 10, 10, 255], conv), CanaryGone::Unknown);
        }
        // the other convention's empty pixel is no pass (the layer is not what was pinned)
        assert_eq!(canary_gone41([0, 0, 0, 0], LayerAlpha::Transmit), CanaryGone::Unknown);
        assert_eq!(canary_gone41([0, 0, 0, 255], LayerAlpha::Premul), CanaryGone::Unknown);
        assert_eq!(gone_next41(CanaryGone::Gone, 1), Some(true));
        assert_eq!(gone_next41(CanaryGone::Persisted, 1), Some(false));
        assert_eq!(gone_next41(CanaryGone::Unknown, 1), None);
        assert_eq!(gone_next41(CanaryGone::Unknown, CANARY_TRIES), Some(false), "never decided: off");
        assert_eq!(fmt_canary_gone41(CanaryGone::Gone, [0, 0, 0, 255]), "inject canary gone value=0,0,0,255");
        assert_eq!(fmt_canary_gone41(CanaryGone::Persisted, [1, 2, 3, 0]), "inject canary persisted value=1,2,3,0");
    }

    #[test]
    fn c4_alpha_canary_and_recheck() {
        for conv in [LayerAlpha::Transmit, LayerAlpha::Premul] {
            let c = canary_px41(conv);
            assert_eq!(&c[..3], &CANARY_RGB);
            assert_eq!(c[3], if conv == LayerAlpha::Premul { 255 } else { 0 }, "opaque in the convention");
            assert_eq!(canary_color41(conv).map(|v| (v * 255.0).round() as u8), c);
            assert_eq!(canary_read41(c, conv), CanaryRead::Ok);
            assert_eq!(canary_read41(to_px(empty_px41(conv)), conv), CanaryRead::Cleared);
            assert_eq!(canary_read41([200, 10, 10, 255], conv), CanaryRead::Unknown);
        }
        // the wrong alpha is no pass (the layer kept another alpha: something wrote there)
        assert_eq!(canary_read41([1, 2, 3, 255], LayerAlpha::Transmit), CanaryRead::Unknown);
        assert_eq!(canary_next41(CanaryRead::Ok, 1), Some(true));
        assert_eq!(canary_next41(CanaryRead::Cleared, 1), Some(false));
        assert_eq!(canary_next41(CanaryRead::Unknown, 1), None);
        assert_eq!(canary_next41(CanaryRead::Unknown, CANARY_TRIES), Some(false), "never decided: off");
        let p = readback_points41(3840, 2160, true);
        assert_eq!(p[0], CANARY_XY, "the persistence check reads the canary texel as point 0 too");
        assert_eq!(p[0], CANARY_XY);
        assert_eq!(p[1..], sample_points41(3840, 2160)[..15]);
        assert_eq!(readback_points41(3840, 2160, false), sample_points41(3840, 2160));
        assert_eq!(readback_points41(1, 1, true)[0], [0, 0], "clamped inside a tiny texture");
        // the re-check: 8 empty texels at least, 90 % agreement
        let mut px = [[0u8, 0, 0, 255]; 16];
        assert_eq!(recheck_vote41(&px), Some(LayerAlpha::Transmit));
        for p in px.iter_mut().take(9) {
            *p = [50, 60, 70, 0];
        }
        assert_eq!(recheck_vote41(&px), None, "7 empty texels: no vote");
        px[0] = [0, 0, 0, 255];
        assert_eq!(recheck_vote41(&px), Some(LayerAlpha::Transmit));
        let mixed: Vec<[u8; 4]> = (0..16).map(|i| if i < 12 { [0, 0, 0, 0] } else { [0, 0, 0, 255] }).collect();
        assert_eq!(recheck_vote41(&mixed), None, "12 / 16: below 90 %");
        let mut r = Recheck41::default();
        assert!(!r.step(Some(LayerAlpha::Premul), LayerAlpha::Transmit));
        assert!(!r.step(None, LayerAlpha::Transmit), "no vote leaves the streak");
        assert!(r.step(Some(LayerAlpha::Premul), LayerAlpha::Transmit), "two disagreements in a row trip");
        let mut r = Recheck41::default();
        assert!(!r.step(Some(LayerAlpha::Premul), LayerAlpha::Transmit));
        assert!(!r.step(Some(LayerAlpha::Transmit), LayerAlpha::Transmit) && r.disagree == 0, "an agreeing vote resets");
        assert!(!r.step(Some(LayerAlpha::Premul), LayerAlpha::Transmit));
    }

    fn to_px(p: Px41) -> [u8; 4] {
        p.map(|v| (v * 255.0).round() as u8)
    }

    #[test]
    fn c4_live41_decision() {
        let t = Some(LayerAlpha::Transmit);
        let p = Some(LayerAlpha::Premul);
        // no probe: the per-build pin, or off
        assert_eq!(live41(&LiveIn41 { pinned: t, ..LiveIn41::default() }), Live41::Go(LayerAlpha::Transmit));
        assert_eq!(live41(&LiveIn41::default()), Live41::Off("alpha_unpinned"));
        // probe on: wait for the verdict
        let probe = LiveIn41 { probe: true, ..LiveIn41::default() };
        assert_eq!(live41(&probe), Live41::Wait);
        let ready = LiveIn41 { judged: true, ready: true, probe_alpha: p, ..probe };
        assert_eq!(live41(&ready), Live41::Go(LayerAlpha::Premul), "P2's convention");
        assert_eq!(live41(&LiveIn41 { pinned: p, ..ready }), Live41::Go(LayerAlpha::Premul));
        assert_eq!(live41(&LiveIn41 { pinned: t, ..ready }), Live41::Off("probe_verdict"), "a pin the probe contradicts");
        // not ready only on alpha: the pin answers it; else wait while P2 samples, at most 10 minutes
        let only_alpha = LiveIn41 { judged: true, only_alpha: true, sampling: true, waited_s: 60.0, ..probe };
        assert_eq!(live41(&only_alpha), Live41::Wait);
        assert_eq!(live41(&LiveIn41 { pinned: t, ..only_alpha }), Live41::Go(LayerAlpha::Transmit));
        assert_eq!(live41(&LiveIn41 { waited_s: LIVE_WAIT_ALPHA_S, ..only_alpha }), Live41::Off("alpha_unpinned"));
        assert_eq!(live41(&LiveIn41 { sampling: false, ..only_alpha }), Live41::Off("alpha_unpinned"));
        // any other failing row: off, pin or not
        assert_eq!(live41(&LiveIn41 { judged: true, pinned: t, ..probe }), Live41::Off("probe_verdict"));
    }

    #[test]
    fn c4_inject41_line_formats() {
        assert_eq!(
            fmt_inject41_on("2.7.2.0", false, LayerAlpha::Transmit, DXGI_R8G8B8A8_UNORM, 3840, 2160),
            "inject on build=2.7.2.0 path=layer mode=ui_thread output=sdr alpha=transmit fmt=R8G8B8A8_UNORM size=3840x2160 slots=3"
        );
        let mut m = vec![0u64; Miss::ALL.len()];
        m[Miss::Canary.index()] = 3;
        m[Miss::Ahead.index()] = 1;
        assert_eq!(fmt_inject41(100, 4, 100, 0, 2, &m), "inject early=100 present=4 splice=100 cancelled=0 fallback_cut=2 miss=ahead:1,canary:3");
        assert_eq!(fmt_inject41(0, 0, 0, 0, 0, &m[..0]), "inject early=0 present=0 splice=0 cancelled=0 fallback_cut=0 miss=none");
        assert_eq!(fmt_canary41(CanaryRead::Ok, [1, 2, 3, 0]), "inject canary ok value=1,2,3,0");
        assert_eq!(fmt_canary41(CanaryRead::Cleared, [0, 0, 0, 255]), "inject canary cleared value=0,0,0,255");
        assert_eq!(fmt_recheck41(None, LayerAlpha::Premul, 3, 0), "inject recheck vote=unknown alpha=premul empty=3/16 disagree=0");
        // the C4 line keeps its form (no fallback_cut)
        assert_eq!(fmt_inject(1, 2, 1, 0, &m[..0]), "inject early=1 present=2 splice=1 cancelled=0 miss=none");
    }
}
