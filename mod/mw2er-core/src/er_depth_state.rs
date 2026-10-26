//! C6 depth 5 (docs/C6_HOLD_DESIGN.md part 15): the state ER leaves its scene depth in, learnt from ER's own
//! `ResourceBarrier` calls, and the rules that let world guns be depth-tested against that depth.
//!
//! - [`Table`]: the lock-free record of the read-only barrier hook (P-S). The DLL's detour (mw2er-render
//!   `er_depth::resource_barrier_detour`) calls the original FIRST, parses the caller's barrier array into [`Bar`]s and
//!   hands each to [`Table::record`]: fixed storage of [`TABLE_N`] entries keyed by the command list (a CAS on the key,
//!   then atomics; no lock, no allocation, no wait). Per list and plane (0 = depth, 1 = stencil) it keeps the first
//!   StateBefore and the last StateAfter of the transitions on the watched resource, plus split / aliasing / UAV
//!   barriers. A barrier into a folded (`done`) entry starts a new recording of that list (ER re-recorded it), so
//!   `Reset` needs no hook.
//! - [`Fold`]: hudhook's ExecuteCommandLists callback applies the recorded lists in queue order ([`Fold::ecl`]) and
//!   keeps the queue's state `Q[plane]`; on the armed UiBegin ECL it stores `Q[0]` BEFORE any splice: the state our
//!   spliced list would see. D3D12 never promotes or decays a depth-stencil texture implicitly, so the explicit
//!   transitions fully determine the state in queue order. A list whose first StateBefore differs from `Q` counts as
//!   a `mismatch` (a barrier we did not see: an enhanced `Barrier()`, another queue, a re-executed list).
//! - [`verdict`], [`copy_gate`], [`live_step`], [`trip_of`]: the pure rules of 15.1 / 15.2 / 15.3.
//! - [`lin`], [`prime_ndc`], [`turn_deg`]: ER reverse-Z depth -> view z -> our reversed-Z projection (15.2 step 2).
//! - [`SelfTally`]: probe P-S2's chest-pixel self check (15.1).
//! - C6 round 6 (work/c6/depth6.md, docs 15.8): the 20:57 run's `verdict ... unknown=3476 queues=0` could not tell a dead
//!   hook from an idle resource. Now the detour counts every call / barrier / transition before any gate, a [`Census`]
//!   keeps every resource with depth-state traffic, BOTH candidates (A "DepthTexture", B the scene context's
//!   DefDepthStencil) get their own [`Table`] / [`Fold`], and B's own UiBegin barrier (`plane 1 0xC0 -> 0x10`, every UI
//!   frame) is the positive control ([`Ctl`], [`verdict_cand`]: `why=hook` / `idle`).
//!
//! Nothing here reads memory; the DLL feeds plain values. ER's resource is never written, barriered or bound for
//! writing by anything this module decides.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::c4::TexDesc;

/// `D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES`.
pub const SUB_ALL: u32 = 0xFFFF_FFFF;
/// A plane whose state is not known (never transitioned since the watch began).
pub const UNKNOWN: u32 = u32::MAX;
/// A plane in the middle of a split barrier (BEGIN_ONLY seen, END_ONLY not yet): never readable.
pub const SPLIT: u32 = u32::MAX - 1;
pub const ST_RENDER_TARGET: u32 = 0x4;
pub const ST_UAV: u32 = 0x8;
pub const ST_DEPTH_WRITE: u32 = 0x10;
pub const ST_DEPTH_READ: u32 = 0x20;
pub const ST_NON_PIXEL: u32 = 0x40;
pub const ST_PIXEL: u32 = 0x80;
/// RT | UAV | DEPTH_WRITE | STREAM_OUT | COPY_DEST | RESOLVE_DEST.
pub const WRITE_BITS: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x400 | 0x1000;
pub const FLAG_BEGIN_ONLY: u32 = 1;
pub const FLAG_END_ONLY: u32 = 2;
/// Entries of the barrier table (distinct command lists that ever transitioned the watched resource).
pub const TABLE_N: usize = 512;
/// Transitions of one frame kept for the `DEPTH state path` line.
pub const PATH_N: usize = 24;
/// The verdict needs this many judged UiBegin frames.
pub const VERDICT_FRAMES: u64 = 600;
/// Frames after a watch change (a new resource: Q unknown) that are not judged.
pub const WARMUP_S: f64 = 2.0;
/// `ID3D12GraphicsCommandList::ResourceBarrier`'s vtable slot.
pub const SLOT_RESOURCE_BARRIER: usize = 26;
/// C6 round 6: entries of the depth census (distinct resources with DEPTH_WRITE / DEPTH_READ traffic).
pub const CENSUS_N: usize = 32;
/// C6 round 6: the census rows logged (by transitions).
pub const CENSUS_TOP: usize = 8;
/// C6 round 6: the positive control (candidate B's own UiBegin / UiEnd barriers) is judged after this many armed frames
/// and must have shown B transitions in at least [`CONTROL_PCT`] % of them.
pub const CONTROL_MIN_FRAMES: u64 = 60;
pub const CONTROL_PCT: u64 = 90;
/// Our world-gun projection (`coop_vis::world_view_proj` at Present).
pub const OUR_NEAR: f32 = 0.05;
pub const OUR_FAR: f32 = 3000.0;
/// Copy / sample slots (each its own list, allocator and copy texture; reused only after its fence passed).
pub const SLOTS: usize = 3;
/// Probe P-S2's texels: 16 grid points, the centre, the chest pixel and (C6 round 7) a +-[`CHEST_CROSS_PX_4K`] cross
/// around it (17 = the chest, 18 / 19 = +x / -x, 20 / 21 = +y / -y).
pub const SAMPLE_N: usize = 22;
/// The first chest texel and their count.
pub const CHEST_AT: usize = 17;
pub const CHEST_N: usize = 5;
/// The chest cross's arm at a 3840-wide back buffer (scaled by the width, at least 1 px).
pub const CHEST_CROSS_PX_4K: u32 = 8;
/// The convention test's guess of the visible surface in front of the Spine2 bone (view z; only `conv_of`'s 0.5..2 x
/// window uses it: C6 round 7 fits the real offset in the self check, depth7.md 1).
pub const CHEST_FRONT_M: f32 = 0.15;
/// C6 round 7 (work/c6/depth7.md 1) / part 16.8: the self check fits the line `z_bone = s * lin(d) + o`; the offset
/// gate is [`FIT_O_MIN_M`]..[`FIT_O_MAX_M`]; >= 90 % of the judged samples within this of the line on the NEAR side
/// (a surface measured behind the line can only let the gun show through: 16.8, the 23:42:39 sample k16 o=-0.49) ...
pub const SELF_RESID_M: f32 = 0.10;
/// ... the 95th-percentile residual below this ...
pub const SELF_P95_M: f32 = 0.25;
/// ... with at least this many judged samples.
pub const SELF_MIN_JUDGED: usize = 6;
/// A sample is an edge (not judged) when its 5 chest texels spread by more than this (linear metres, or one is the
/// sky); more than [`SELF_EDGE_MAX`] of the still samples as edges fails (`why=edge`: failures cannot hide).
pub const SELF_EDGE_M: f32 = 0.15;
pub const SELF_EDGE_MAX: f32 = 0.25;
/// "Still" also needs the camera to have moved less than this per task tick (the snapshot vs ER's frame).
pub const SELF_STILL_MV_M: f32 = 0.05;
/// C6 part 16.8 (work/c6/depth8.md 1): the self check fits slope AND offset together, `z_bone = s * lin(d, near_cam) + o`
/// (Theil-Sen over every judged still sample; no offset pre-filter). Pairs need at least this lin(d) difference (round 7's
/// 0.10 m let 2-4 cm noise decide the slope: the 23:42 `slope=0.925`).
pub const SCALE_PAIR_M: f32 = 0.30;
/// The scale is proven only with leverage: at least [`SCALE_LEV_N`] judged samples at or below the median lin(d) minus
/// [`SCALE_LEV_M`] AND as many at or above the median plus it (one far sample never proves a scale: the 23:41:39
/// `scale=proven slope=1.026`) ...
pub const SCALE_LEV_M: f32 = 0.30;
pub const SCALE_LEV_N: usize = 3;
/// ... and the 25-75 % pair slopes within this of the slope (a tight fit).
pub const SCALE_IQR_REL: f32 = 0.03;
/// The slope gate: outside it the linearisation is wrong (`why=scale`); 23:39 log: s = 1.098.
pub const SCALE_S_MIN: f32 = 0.95;
pub const SCALE_S_MAX: f32 = 1.25;
/// The fitted offset gate (metres): outside it `why=offset`.
pub const FIT_O_MIN_M: f32 = -0.10;
pub const FIT_O_MAX_M: f32 = 0.50;
/// The fit (not the residuals) skips judged samples farther than this from the pure-scale line z = median(z / zl) * zl.
pub const FIT_GROSS_M: f32 = 0.30;
/// A live candidate trips when the proven slope's low end (25 % pair slope) beats the live near ratio by more than this.
pub const SCALE_TRIP_REL: f32 = 0.05;
/// The finite far model needs samples where it and the infinite model differ by at least this (metres)...
pub const FINITE_EVIDENCE_M: f32 = 0.25;
/// ... at least this many of them ...
pub const FINITE_MIN_N: usize = 6;
/// ... and a median error better than the infinite model's by this (well above the chest-offset uncertainty).
pub const FINITE_MARGIN_M: f32 = 0.10;

/// Is `s` a state a compute / pixel shader may read the depth plane in: NON_PIXEL_SHADER_RESOURCE (a compute SRV
/// read) and no write bit. Expected [I]: 0xC0 or 0xE0 (DEPTH_READ | PIXEL | NON_PIXEL).
pub fn readable(s: u32) -> bool {
    s != UNKNOWN && s != SPLIT && s & ST_NON_PIXEL != 0 && s & WRITE_BITS == 0
}

pub fn state_text(s: u32) -> String {
    match s {
        UNKNOWN => "unknown".into(),
        SPLIT => "split".into(),
        s => format!("0x{s:x}"),
    }
}

/// One parsed `D3D12_RESOURCE_BARRIER` (raw pointers as integers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    Transition { res: usize, sub: u32, before: u32, after: u32, flags: u32 },
    Alias { before: usize, after: usize },
    Uav { res: usize },
    Other,
}

const E_EMPTY: u32 = 0;
const E_REC: u32 = 1;
const E_DONE: u32 = 2;

/// One command list's record (atomics only: the recording thread writes, the ECL fold reads after Close).
pub struct Entry {
    key: AtomicUsize,
    state: AtomicU32,
    res: AtomicUsize,
    fb: [AtomicU32; 2],
    la: [AtomicU32; 2],
    n: AtomicU32,
    split: AtomicU32,
    alias: AtomicU32,
    uav: AtomicU32,
    bad_sub: AtomicU32,
    tid: AtomicU32,
}

impl Entry {
    const fn new() -> Entry {
        Entry {
            key: AtomicUsize::new(0),
            state: AtomicU32::new(E_EMPTY),
            res: AtomicUsize::new(0),
            fb: [AtomicU32::new(UNKNOWN), AtomicU32::new(UNKNOWN)],
            la: [AtomicU32::new(UNKNOWN), AtomicU32::new(UNKNOWN)],
            n: AtomicU32::new(0),
            split: AtomicU32::new(0),
            alias: AtomicU32::new(0),
            uav: AtomicU32::new(0),
            bad_sub: AtomicU32::new(0),
            tid: AtomicU32::new(0),
        }
    }

    fn restart(&self, watch: usize, tid: u32) {
        for p in 0..2 {
            self.fb[p].store(UNKNOWN, Ordering::Relaxed);
            self.la[p].store(UNKNOWN, Ordering::Relaxed);
        }
        for a in [&self.n, &self.split, &self.alias, &self.uav, &self.bad_sub] {
            a.store(0, Ordering::Relaxed);
        }
        self.res.store(watch, Ordering::Relaxed);
        self.tid.store(tid, Ordering::Relaxed);
        self.state.store(E_REC, Ordering::Release);
    }
}

/// The barrier table of P-S (one static instance in the DLL; vmtest ED5 builds its own).
pub struct Table {
    e: [Entry; TABLE_N],
    /// A barrier on the watched resource found no free entry (the verdict is then not ready).
    pub full: AtomicU64,
    /// Barriers on the watched resource recorded.
    pub recorded: AtomicU64,
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

fn hash(list: usize) -> usize {
    (list >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 7
}

impl Table {
    pub const fn new() -> Table {
        Table { e: [const { Entry::new() }; TABLE_N], full: AtomicU64::new(0), recorded: AtomicU64::new(0) }
    }

    /// The entry of `list` (claimed when `claim`; None = table full / not found).
    fn slot(&self, list: usize, claim: bool) -> Option<&Entry> {
        let h = hash(list);
        for i in 0..TABLE_N {
            let e = &self.e[(h + i) % TABLE_N];
            let k = e.key.load(Ordering::Acquire);
            if k == list {
                return Some(e);
            }
            if k == 0 {
                if !claim {
                    return None;
                }
                match e.key.compare_exchange(0, list, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return Some(e),
                    Err(cur) if cur == list => return Some(e),
                    Err(_) => continue,
                }
            }
        }
        None
    }

    /// The detour's body for one barrier of `list` (any thread; atomics only). Only barriers naming `watch` are kept.
    pub fn record(&self, list: usize, tid: u32, watch: usize, b: Bar) {
        if watch == 0 || list == 0 {
            return;
        }
        let touches = match b {
            Bar::Transition { res, .. } | Bar::Uav { res } => res == watch,
            Bar::Alias { before, after } => before == watch || after == watch,
            Bar::Other => false,
        };
        if !touches {
            return;
        }
        let Some(e) = self.slot(list, true) else {
            self.full.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if e.state.load(Ordering::Acquire) != E_REC || e.res.load(Ordering::Relaxed) != watch {
            e.restart(watch, tid);
        }
        self.recorded.fetch_add(1, Ordering::Relaxed);
        match b {
            Bar::Transition { sub, before, after, flags, .. } => {
                let planes: &[usize] = match sub {
                    SUB_ALL => &[0, 1],
                    0 => &[0],
                    1 => &[1],
                    _ => {
                        e.bad_sub.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                };
                if flags & (FLAG_BEGIN_ONLY | FLAG_END_ONLY) != 0 {
                    e.split.fetch_add(1, Ordering::Relaxed);
                }
                for &p in planes {
                    if e.fb[p].load(Ordering::Relaxed) == UNKNOWN {
                        e.fb[p].store(before, Ordering::Relaxed);
                    }
                    e.la[p].store(if flags & FLAG_BEGIN_ONLY != 0 { SPLIT } else { after }, Ordering::Relaxed);
                }
                e.n.fetch_add(1, Ordering::Relaxed);
            }
            Bar::Alias { .. } => {
                e.alias.fetch_add(1, Ordering::Relaxed);
            }
            Bar::Uav { .. } => {
                e.uav.fetch_add(1, Ordering::Relaxed);
            }
            Bar::Other => {}
        }
    }

    /// The not yet folded record of `list` (vmtest / tests): (first before, last after) per plane, transitions.
    pub fn pending(&self, list: usize) -> Option<([u32; 2], [u32; 2], u32)> {
        let e = self.slot(list, false)?;
        (e.state.load(Ordering::Acquire) == E_REC).then(|| {
            ([0, 1].map(|p| e.fb[p].load(Ordering::Relaxed)), [0, 1].map(|p| e.la[p].load(Ordering::Relaxed)), e.n.load(Ordering::Relaxed))
        })
    }
}

/// The inputs of one ECL call for [`Fold::ecl`].
pub struct EclIn<'a> {
    pub queue: usize,
    pub tid: u32,
    pub lists: &'a [usize],
    /// The watched resource and its epoch (bumped on every change of the watched resource).
    pub watch: usize,
    pub epoch: u64,
    /// This call carries the frame's armed Scaleform list at index 0 (the C4.1 rule) in this seq.
    pub armed: bool,
    pub seq: u64,
    /// The frame is judged (past the warm-up of the current watch).
    pub judged: bool,
    /// ER's HAL queue and render thread of this frame (the path line's q=hal / t=rt).
    pub hal_queue: usize,
    pub rt_tid: u32,
    /// The hooked ResourceBarrier (the `vt_eq` check: `[[list] + 26 * 8]` must equal it).
    pub target: usize,
}

/// The queue-order fold of P-S (one static instance in the DLL). All counters are cumulative.
pub struct Fold {
    q: [AtomicU32; 2],
    epoch: AtomicU64,
    pub lists: AtomicU64,
    pub transitions: AtomicU64,
    pub mismatch: AtomicU64,
    pub unmatched: AtomicU64,
    pub split: AtomicU64,
    pub alias: AtomicU64,
    pub uav: AtomicU64,
    pub bad_sub: AtomicU64,
    queues: [AtomicUsize; 4],
    threads: [AtomicU32; 4],
    /// More than 4 distinct queues / threads.
    over: AtomicU32,
    pub vt_eq: AtomicU64,
    pub vt_n: AtomicU64,
    /// The state at the armed UiBegin ECL of `at_seq`.
    at_seq: AtomicU64,
    at_state: AtomicU32,
    /// Judged frames: all, readable, a write bit, unknown, and the at_ui histogram 0xC0 / 0xE0 / 0x10 / other.
    pub frames: AtomicU64,
    pub readable: AtomicU64,
    pub write_at_ui: AtomicU64,
    pub unknown: AtomicU64,
    pub h_c0: AtomicU64,
    pub h_e0: AtomicU64,
    pub h_10: AtomicU64,
    pub h_other: AtomicU64,
    /// Armed frames seen (judged or not).
    pub armed_frames: AtomicU64,
    /// C6 round 6: armed frames whose previous frame folded at least one transition of this candidate (the positive
    /// control on B: its own UiBegin barrier each UI frame), and the transition count at the last armed frame.
    pub ctl_hits: AtomicU64,
    ctl_last: AtomicU64,
    path: [AtomicU64; PATH_N],
    path_n: AtomicU32,
    /// The last complete frame's path (published at the next armed ECL) and its generation.
    last_path: [AtomicU64; PATH_N],
    last_n: AtomicU32,
    last_seq: AtomicU64,
    last_at: AtomicU32,
    pub last_gen: AtomicU64,
}

impl Default for Fold {
    fn default() -> Self {
        Self::new()
    }
}

/// A path entry: plane 0 first-before (24 bits), last-after (24 bits), planes touched (bits 48-49), HAL queue (50),
/// render thread (51). UNKNOWN / SPLIT map to 0xFFFFFF / 0xFFFFFE.
fn pack_path(fb: u32, la: u32, planes: u32, halq: bool, rt: bool) -> u64 {
    let s = |x: u32| match x {
        UNKNOWN => 0xFF_FFFF,
        SPLIT => 0xFF_FFFE,
        x => (x & 0xFF_FFFF).min(0xFF_FFFD),
    } as u64;
    s(fb) | s(la) << 24 | ((planes & 3) as u64) << 48 | (halq as u64) << 50 | (rt as u64) << 51
}

fn unpack_state(v: u64) -> u32 {
    match v & 0xFF_FFFF {
        0xFF_FFFF => UNKNOWN,
        0xFF_FFFE => SPLIT,
        x => x as u32,
    }
}

/// One path entry as text: `0x10>0xc0 sub=all q=hal t=rt`.
pub fn path_entry_text(v: u64) -> String {
    let sub = match (v >> 48) & 3 {
        3 => "all",
        1 => "0",
        2 => "1",
        _ => "none",
    };
    format!(
        "{}>{} sub={sub} q={} t={}",
        state_text(unpack_state(v)),
        state_text(unpack_state(v >> 24)),
        if v >> 50 & 1 != 0 { "hal" } else { "other" },
        if v >> 51 & 1 != 0 { "rt" } else { "other" }
    )
}

fn note_distinct_usize(arr: &[AtomicUsize; 4], over: &AtomicU32, v: usize) {
    if v == 0 {
        return;
    }
    for a in arr {
        let cur = a.load(Ordering::Relaxed);
        if cur == v {
            return;
        }
        if cur == 0 {
            match a.compare_exchange(0, v, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(x) if x == v => return,
                Err(_) => continue,
            }
        }
    }
    over.fetch_add(1, Ordering::Relaxed);
}

fn note_distinct_u32(arr: &[AtomicU32; 4], over: &AtomicU32, v: u32) {
    if v == 0 {
        return;
    }
    for a in arr {
        let cur = a.load(Ordering::Relaxed);
        if cur == v {
            return;
        }
        if cur == 0 {
            match a.compare_exchange(0, v, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return,
                Err(x) if x == v => return,
                Err(_) => continue,
            }
        }
    }
    over.fetch_add(1, Ordering::Relaxed);
}

/// A plain copy of the fold's counters (the task logs and judges it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateStats {
    pub frames: u64,
    pub readable: u64,
    pub write_at_ui: u64,
    pub unknown: u64,
    pub h_c0: u64,
    pub h_e0: u64,
    pub h_10: u64,
    pub h_other: u64,
    pub lists: u64,
    pub transitions: u64,
    pub barriers: u64,
    pub queues: u32,
    pub threads: u32,
    pub mismatch: u64,
    pub unmatched: u64,
    pub split: u64,
    pub alias: u64,
    pub uav: u64,
    pub bad_sub: u64,
    pub table_full: u64,
    pub vt_eq: u64,
    pub vt_n: u64,
    /// C6 round 6: armed frames seen (judged or not) and those with a transition of this candidate since the last one.
    pub armed: u64,
    pub ctl_hits: u64,
}

impl Fold {
    pub const fn new() -> Fold {
        Fold {
            q: [AtomicU32::new(UNKNOWN), AtomicU32::new(UNKNOWN)],
            epoch: AtomicU64::new(0),
            lists: AtomicU64::new(0),
            transitions: AtomicU64::new(0),
            mismatch: AtomicU64::new(0),
            unmatched: AtomicU64::new(0),
            split: AtomicU64::new(0),
            alias: AtomicU64::new(0),
            uav: AtomicU64::new(0),
            bad_sub: AtomicU64::new(0),
            queues: [const { AtomicUsize::new(0) }; 4],
            threads: [const { AtomicU32::new(0) }; 4],
            over: AtomicU32::new(0),
            vt_eq: AtomicU64::new(0),
            vt_n: AtomicU64::new(0),
            at_seq: AtomicU64::new(u64::MAX),
            at_state: AtomicU32::new(UNKNOWN),
            frames: AtomicU64::new(0),
            readable: AtomicU64::new(0),
            write_at_ui: AtomicU64::new(0),
            unknown: AtomicU64::new(0),
            h_c0: AtomicU64::new(0),
            h_e0: AtomicU64::new(0),
            h_10: AtomicU64::new(0),
            h_other: AtomicU64::new(0),
            armed_frames: AtomicU64::new(0),
            ctl_hits: AtomicU64::new(0),
            ctl_last: AtomicU64::new(0),
            path: [const { AtomicU64::new(0) }; PATH_N],
            path_n: AtomicU32::new(0),
            last_path: [const { AtomicU64::new(0) }; PATH_N],
            last_n: AtomicU32::new(0),
            last_seq: AtomicU64::new(0),
            last_at: AtomicU32::new(UNKNOWN),
            last_gen: AtomicU64::new(0),
        }
    }

    /// Q per plane now.
    pub fn q(&self) -> [u32; 2] {
        [0, 1].map(|p| self.q[p].load(Ordering::Relaxed))
    }

    /// `Q[0]` at the armed UiBegin ECL of `seq` (None: that frame's armed call was not folded).
    pub fn at_ui(&self, seq: u64) -> Option<u32> {
        (self.at_seq.load(Ordering::Acquire) == seq).then(|| self.at_state.load(Ordering::Relaxed))
    }

    pub fn queues(&self) -> u32 {
        self.queues.iter().filter(|a| a.load(Ordering::Relaxed) != 0).count() as u32 + self.over.load(Ordering::Relaxed).min(1)
    }

    pub fn threads(&self) -> u32 {
        self.threads.iter().filter(|a| a.load(Ordering::Relaxed) != 0).count() as u32 + self.over.load(Ordering::Relaxed).min(1)
    }

    pub fn stats(&self, t: &Table) -> StateStats {
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StateStats {
            frames: l(&self.frames),
            readable: l(&self.readable),
            write_at_ui: l(&self.write_at_ui),
            unknown: l(&self.unknown),
            h_c0: l(&self.h_c0),
            h_e0: l(&self.h_e0),
            h_10: l(&self.h_10),
            h_other: l(&self.h_other),
            lists: l(&self.lists),
            transitions: l(&self.transitions),
            barriers: l(&t.recorded),
            queues: self.queues(),
            threads: self.threads(),
            mismatch: l(&self.mismatch),
            unmatched: l(&self.unmatched),
            split: l(&self.split),
            alias: l(&self.alias),
            uav: l(&self.uav),
            bad_sub: l(&self.bad_sub),
            table_full: l(&t.full),
            vt_eq: l(&self.vt_eq),
            vt_n: l(&self.vt_n),
            armed: l(&self.armed_frames),
            ctl_hits: l(&self.ctl_hits),
        }
    }

    /// C6 round 6: `Q[0]` at the last armed UiBegin call (the sample's candidate choice: readable there last frame).
    pub fn last_at_ui(&self) -> u32 {
        self.at_state.load(Ordering::Relaxed)
    }

    /// The last complete frame's path: (generation, seq, entries, state at UI).
    pub fn last_path(&self) -> (u64, u64, Vec<u64>, u32) {
        let g = self.last_gen.load(Ordering::Acquire);
        let n = (self.last_n.load(Ordering::Relaxed) as usize).min(PATH_N);
        let v = self.last_path[..n].iter().map(|a| a.load(Ordering::Relaxed)).collect();
        (g, self.last_seq.load(Ordering::Relaxed), v, self.last_at.load(Ordering::Relaxed))
    }

    /// hudhook's ECL callback (any thread; atomics only, no allocation): the fold of one call, BEFORE any splice.
    /// Returns `Q[0]` at the armed UiBegin call (the state our list spliced at index 0 would see), else None.
    /// `vt_of(list)` = `[[list] + 26 * 8]` (a non-faulting read; None = unreadable).
    pub fn ecl(&self, t: &Table, i: &EclIn<'_>, vt_of: &dyn Fn(usize) -> Option<usize>) -> Option<u32> {
        if i.watch == 0 {
            return None;
        }
        if self.epoch.swap(i.epoch, Ordering::AcqRel) != i.epoch {
            for p in 0..2 {
                self.q[p].store(UNKNOWN, Ordering::Relaxed);
            }
        }
        let mut at = None;
        if i.armed && self.at_seq.load(Ordering::Acquire) != i.seq {
            let s = self.q[0].load(Ordering::Relaxed);
            self.at_state.store(s, Ordering::Relaxed);
            self.at_seq.store(i.seq, Ordering::Release);
            self.armed_frames.fetch_add(1, Ordering::Relaxed);
            // C6 round 6: the control: did this candidate see a transition since the last armed frame?
            let tr = self.transitions.load(Ordering::Relaxed);
            if tr > self.ctl_last.swap(tr, Ordering::Relaxed) {
                self.ctl_hits.fetch_add(1, Ordering::Relaxed);
            }
            at = Some(s);
            if i.judged {
                self.frames.fetch_add(1, Ordering::Relaxed);
                if readable(s) {
                    self.readable.fetch_add(1, Ordering::Relaxed);
                }
                if s == UNKNOWN {
                    self.unknown.fetch_add(1, Ordering::Relaxed);
                } else if s != SPLIT && s & WRITE_BITS != 0 {
                    self.write_at_ui.fetch_add(1, Ordering::Relaxed);
                }
                match s {
                    0xC0 => self.h_c0.fetch_add(1, Ordering::Relaxed),
                    0xE0 => self.h_e0.fetch_add(1, Ordering::Relaxed),
                    0x10 => self.h_10.fetch_add(1, Ordering::Relaxed),
                    UNKNOWN => 0,
                    _ => self.h_other.fetch_add(1, Ordering::Relaxed),
                };
            }
            // the frame's path ends here: publish it (generation odd while writing)
            let n = (self.path_n.swap(0, Ordering::AcqRel) as usize).min(PATH_N);
            self.last_gen.fetch_add(1, Ordering::AcqRel);
            for k in 0..n {
                self.last_path[k].store(self.path[k].load(Ordering::Relaxed), Ordering::Relaxed);
            }
            self.last_n.store(n as u32, Ordering::Relaxed);
            self.last_seq.store(i.seq, Ordering::Relaxed);
            self.last_at.store(s, Ordering::Relaxed);
            self.last_gen.fetch_add(1, Ordering::AcqRel);
        }
        for &list in i.lists {
            let Some(e) = t.slot(list, false) else { continue };
            if e.state.load(Ordering::Acquire) != E_REC {
                // folded already: the list was re-recorded without a barrier on the resource (no state change)
                continue;
            }
            if e.res.load(Ordering::Relaxed) != i.watch {
                e.state.store(E_DONE, Ordering::Release);
                self.unmatched.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            self.vt_n.fetch_add(1, Ordering::Relaxed);
            if i.target != 0 && vt_of(list) == Some(i.target) {
                self.vt_eq.fetch_add(1, Ordering::Relaxed);
            }
            let mut planes = 0u32;
            for p in 0..2 {
                let fb = e.fb[p].load(Ordering::Relaxed);
                if fb == UNKNOWN {
                    continue;
                }
                planes |= 1 << p;
                let q = self.q[p].load(Ordering::Relaxed);
                if q != UNKNOWN && q != fb {
                    self.mismatch.fetch_add(1, Ordering::Relaxed);
                }
                self.q[p].store(e.la[p].load(Ordering::Relaxed), Ordering::Relaxed);
            }
            self.lists.fetch_add(1, Ordering::Relaxed);
            self.transitions.fetch_add(e.n.load(Ordering::Relaxed) as u64, Ordering::Relaxed);
            for (src, dst) in [(&e.split, &self.split), (&e.alias, &self.alias), (&e.uav, &self.uav), (&e.bad_sub, &self.bad_sub)] {
                let v = src.load(Ordering::Relaxed);
                if v > 0 {
                    dst.fetch_add(v as u64, Ordering::Relaxed);
                }
            }
            note_distinct_usize(&self.queues, &self.over, i.queue);
            note_distinct_u32(&self.threads, &self.over, i.tid);
            let k = self.path_n.fetch_add(1, Ordering::AcqRel) as usize;
            if k < PATH_N {
                let fb0 = e.fb[0].load(Ordering::Relaxed);
                let la0 = e.la[0].load(Ordering::Relaxed);
                self.path[k].store(pack_path(fb0, la0, planes, i.queue == i.hal_queue && i.queue != 0, i.tid == i.rt_tid && i.tid != 0), Ordering::Relaxed);
            } else {
                self.path_n.store(PATH_N as u32, Ordering::Relaxed);
            }
            e.state.store(E_DONE, Ordering::Release);
        }
        at
    }
}

/// `DEPTH state res=.. q=.. at_ui={..} frames=.. ...` (5 s).
pub fn state_text_line(res: usize, q: u32, s: &StateStats) -> String {
    format!(
        "state res=0x{res:x} q={} at_ui={{0xC0:{},0xE0:{},0x10:{},unknown:{},other:{}}} frames={} barriers={} lists={} queues={} threads={} split={} alias={} uav={} mismatch={} unmatched={} table_full={} bad_sub={} vt_eq={}/{}",
        state_text(q),
        s.h_c0,
        s.h_e0,
        s.h_10,
        s.unknown,
        s.h_other,
        s.frames,
        s.barriers,
        s.lists,
        s.queues,
        s.threads,
        s.split,
        s.alias,
        s.uav,
        s.mismatch,
        s.unmatched,
        s.table_full,
        s.bad_sub,
        s.vt_eq,
        s.vt_n
    )
}

/// `DEPTH state path seq=<s> [..] at_ui=0x..`.
pub fn path_line(seq: u64, path: &[u64], at: u32) -> String {
    format!("state path seq={seq} [{}] at_ui={}", path.iter().map(|v| path_entry_text(*v)).collect::<Vec<_>>().join(", "), state_text(at))
}

/// Design 15.1: ready iff >= 600 judged UiBegin frames, every one readable (no write bit, none unknown), one queue,
/// one thread, no mismatch / split / alias / full table / odd subresource, and every checked list's slot 26 the hooked
/// function. Returns (ready, the first failing row).
pub fn verdict(s: &StateStats) -> (bool, &'static str) {
    let why = if s.frames < VERDICT_FRAMES {
        "frames"
    } else if s.write_at_ui > 0 {
        "write_at_ui"
    } else if s.unknown > 0 {
        "unknown"
    } else if s.readable != s.frames {
        "readable"
    } else if s.queues != 1 {
        "queues"
    } else if s.threads != 1 {
        "threads"
    } else if s.mismatch > 0 {
        "mismatch"
    } else if s.split > 0 {
        "split"
    } else if s.alias > 0 {
        "alias"
    } else if s.table_full > 0 {
        "table_full"
    } else if s.bad_sub > 0 {
        "subresource"
    } else if s.vt_n == 0 || s.vt_eq != s.vt_n {
        "vt_eq"
    } else {
        return (true, "none");
    };
    (false, why)
}

/// `DEPTH state verdict ready=.. ...` (60 s, then every 60 s).
pub fn verdict_line(s: &StateStats) -> String {
    let (ready, why) = verdict(s);
    format!(
        "state verdict ready={} frames={} readable={}/{} write_at_ui={} unknown={} queues={} threads={} mismatch={} split={} alias={} table_full={} vt_eq={}/{} why={why}",
        ready as u8, s.frames, s.readable, s.frames, s.write_at_ui, s.unknown, s.queues, s.threads, s.mismatch, s.split, s.alias, s.table_full, s.vt_eq, s.vt_n
    )
}

// ---- C6 round 6: the census, the positive control, the per-candidate verdict ------------------------------------------

/// The candidates' tags (index 0 = A `[RS+0x50]+0xA70` "DepthTexture", 1 = B the scene context's DefDepthStencil).
pub const CAND_TAGS: [&str; 2] = ["A", "B"];

/// One census entry (atomics only: any recording thread writes, the task reads).
pub struct CensusEntry {
    key: AtomicUsize,
    n: AtomicU64,
    states: AtomicU32,
    lb: [AtomicU32; 2],
    la: [AtomicU32; 2],
    sub_all: AtomicU32,
    split: AtomicU32,
}

impl CensusEntry {
    const fn new() -> CensusEntry {
        CensusEntry {
            key: AtomicUsize::new(0),
            n: AtomicU64::new(0),
            states: AtomicU32::new(0),
            lb: [AtomicU32::new(UNKNOWN), AtomicU32::new(UNKNOWN)],
            la: [AtomicU32::new(UNKNOWN), AtomicU32::new(UNKNOWN)],
            sub_all: AtomicU32::new(0),
            split: AtomicU32::new(0),
        }
    }
}

/// C6 round 6 (depth6.md 4 item 1): every resource transitioned into / out of a depth state (DEPTH_WRITE 0x10 /
/// DEPTH_READ 0x20) by any list of the process: a fixed open-addressing table keyed by the resource pointer (a CAS
/// insert; no lock, no allocation, no wait). It names ER's real scene depth even if it is neither candidate.
pub struct Census {
    e: [CensusEntry; CENSUS_N],
    /// A depth transition found no free entry.
    pub full: AtomicU64,
}

impl Default for Census {
    fn default() -> Self {
        Self::new()
    }
}

/// One census row (task thread copy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CensusRow {
    pub res: usize,
    pub n: u64,
    /// OR of every before / after state seen.
    pub states: u32,
    /// The last (before, after) per plane.
    pub last: [(u32, u32); 2],
    pub sub_all: u32,
    pub split: u32,
}

impl Census {
    pub const fn new() -> Census {
        Census { e: [const { CensusEntry::new() }; CENSUS_N], full: AtomicU64::new(0) }
    }

    /// The detour's body for one transition (any thread; atomics only). Only depth-state traffic is kept.
    pub fn record(&self, res: usize, sub: u32, before: u32, after: u32, flags: u32) {
        if res == 0 || (before | after) & (ST_DEPTH_WRITE | ST_DEPTH_READ) == 0 {
            return;
        }
        let h = hash(res);
        let mut slot = None;
        for i in 0..CENSUS_N {
            let e = &self.e[(h + i) % CENSUS_N];
            let k = e.key.load(Ordering::Acquire);
            if k == res {
                slot = Some(e);
                break;
            }
            if k == 0 {
                match e.key.compare_exchange(0, res, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {
                        slot = Some(e);
                        break;
                    }
                    Err(cur) if cur == res => {
                        slot = Some(e);
                        break;
                    }
                    Err(_) => continue,
                }
            }
        }
        let Some(e) = slot else {
            self.full.fetch_add(1, Ordering::Relaxed);
            return;
        };
        e.n.fetch_add(1, Ordering::Relaxed);
        e.states.fetch_or(before | after, Ordering::Relaxed);
        if flags & (FLAG_BEGIN_ONLY | FLAG_END_ONLY) != 0 {
            e.split.fetch_add(1, Ordering::Relaxed);
        }
        let planes: &[usize] = match sub {
            SUB_ALL => {
                e.sub_all.fetch_add(1, Ordering::Relaxed);
                &[0, 1]
            }
            0 => &[0],
            1 => &[1],
            _ => &[],
        };
        for &p in planes {
            e.lb[p].store(before, Ordering::Relaxed);
            e.la[p].store(after, Ordering::Relaxed);
        }
    }

    /// Every used entry, most transitions first (task thread).
    pub fn rows(&self) -> Vec<CensusRow> {
        let mut v: Vec<CensusRow> = self
            .e
            .iter()
            .filter_map(|e| {
                let res = e.key.load(Ordering::Acquire);
                (res != 0).then(|| CensusRow {
                    res,
                    n: e.n.load(Ordering::Relaxed),
                    states: e.states.load(Ordering::Relaxed),
                    last: [0, 1].map(|p| (e.lb[p].load(Ordering::Relaxed), e.la[p].load(Ordering::Relaxed))),
                    sub_all: e.sub_all.load(Ordering::Relaxed),
                    split: e.split.load(Ordering::Relaxed),
                })
            })
            .collect();
        v.sort_by(|a, b| b.n.cmp(&a.n).then(a.res.cmp(&b.res)));
        v
    }
}

/// `DEPTH census res=.. n=.. states=0x.. p0=a>b p1=a>b sub_all=.. split=.. is=A|B|- vt_ok=0|1`.
pub fn census_line(r: &CensusRow, is: &str, vt_ok: bool) -> String {
    let pl = |p: usize| format!("{}>{}", state_text(r.last[p].0), state_text(r.last[p].1));
    format!(
        "census res=0x{:x} n={} states=0x{:x} p0={} p1={} sub_all={} split={} is={is} vt_ok={}",
        r.res,
        r.n,
        r.states,
        pl(0),
        pl(1),
        r.sub_all,
        r.split,
        vt_ok as u8
    )
}

/// The positive control: the detour's call count and B's armed frames / hits ([`Fold::ctl_hits`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ctl {
    pub calls: u64,
    pub frames: u64,
    pub hits: u64,
}

/// The control's state: None = not judged yet (fewer than [`CONTROL_MIN_FRAMES`] of B), Some(pass).
pub fn control_ok(c: &Ctl) -> Option<bool> {
    if c.calls == 0 && c.frames >= CONTROL_MIN_FRAMES {
        return Some(false);
    }
    (c.frames >= CONTROL_MIN_FRAMES).then(|| c.calls > 0 && c.hits * 100 >= c.frames * CONTROL_PCT)
}

/// `DEPTH state control B ui=<hits>/<frames> calls=.. bars=.. trans=.. census_full=.. ok=0|1|-` (5 s).
pub fn control_line(c: &Ctl, bars: u64, trans: u64, census_full: u64) -> String {
    format!(
        "state control B ui={}/{} calls={} bars={bars} trans={trans} census_full={census_full} ok={}",
        c.hits,
        c.frames,
        c.calls,
        control_ok(c).map_or("-".to_string(), |b| (b as u8).to_string())
    )
}

/// C6 round 6 (depth6.md 4 item 5): one candidate's verdict with the control first: no detour call at all, or B's own
/// UI barriers missing in more than 10 % of the frames = `hook` (nothing about the candidate can be said); the control
/// passing and the candidate never transitioned = `idle`; then the 15.1 rows ([`verdict`]). `why=control` while the
/// control is not judged yet (B never watched / too few frames).
pub fn verdict_cand(s: &StateStats, c: &Ctl) -> (bool, &'static str) {
    match control_ok(c) {
        Some(false) => return (false, "hook"),
        None if c.calls == 0 => return (false, "hook"),
        None => {
            if s.frames >= VERDICT_FRAMES {
                return (false, "control");
            }
        }
        Some(true) => {
            if s.lists == 0 && s.transitions == 0 && s.frames >= VERDICT_FRAMES {
                return (false, "idle");
            }
        }
    }
    verdict(s)
}

/// `DEPTH state verdict ready=.. ... why=.. cand=A|B` (60 s; the old fields first, the candidate last).
pub fn cand_verdict_line(cand: usize, s: &StateStats, c: &Ctl) -> String {
    let (ready, why) = verdict_cand(s, c);
    format!(
        "state verdict ready={} frames={} readable={}/{} write_at_ui={} unknown={} queues={} threads={} mismatch={} split={} alias={} table_full={} vt_eq={}/{} lists={} transitions={} why={why} cand={}",
        ready as u8,
        s.frames,
        s.readable,
        s.frames,
        s.write_at_ui,
        s.unknown,
        s.queues,
        s.threads,
        s.mismatch,
        s.split,
        s.alias,
        s.table_full,
        s.vt_eq,
        s.vt_n,
        s.lists,
        s.transitions,
        CAND_TAGS[cand.min(1)]
    )
}

/// C6 round 6: which candidate a sample / the live copy reads: the live one, else the one whose `Q[0]` was readable at
/// the last armed UiBegin (A first: the design's candidate), else None (no sample this frame).
pub fn pick_cand(live: Option<usize>, last_at: [u32; 2], watched: [bool; 2]) -> Option<usize> {
    if let Some(c) = live {
        return watched[c.min(1)].then_some(c.min(1));
    }
    (0..2).find(|&k| watched[k] && readable(last_at[k]))
}

// ---- the copy gate (15.2 step 1) -------------------------------------------------------------------------------------

/// What Present last wanted (published every Present for the copy gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Nothing published recently (no Present, a loading screen).
    None = 0,
    /// First person: no ER-depth prime (peers' guns draw as today).
    Fp = 1,
    /// No world gun this frame (MW2 mode off, no peer, a menu, the AC-130, a nuke flash).
    NoGun = 2,
    /// A third-person world gun is drawn.
    Gun = 3,
}

impl Want {
    pub fn from_code(c: u8) -> Want {
        match c {
            1 => Want::Fp,
            2 => Want::NoGun,
            3 => Want::Gun,
            _ => Want::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyMiss {
    Fp,
    NoGun,
    Thread,
    Queue,
    NoFrame,
    Ahead,
    Chain,
    Recreated,
    Desc,
    Format,
    Size,
    Slot,
    Busy,
    Pso,
    State,
    Turn,
    Pair,
}

impl CopyMiss {
    pub const ALL: [CopyMiss; 17] = [
        CopyMiss::Fp,
        CopyMiss::NoGun,
        CopyMiss::Thread,
        CopyMiss::Queue,
        CopyMiss::NoFrame,
        CopyMiss::Ahead,
        CopyMiss::Chain,
        CopyMiss::Recreated,
        CopyMiss::Desc,
        CopyMiss::Format,
        CopyMiss::Size,
        CopyMiss::Slot,
        CopyMiss::Busy,
        CopyMiss::Pso,
        CopyMiss::State,
        CopyMiss::Turn,
        CopyMiss::Pair,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CopyMiss::Fp => "fp",
            CopyMiss::NoGun => "nogun",
            CopyMiss::Thread => "thread",
            CopyMiss::Queue => "queue",
            CopyMiss::NoFrame => "noframe",
            CopyMiss::Ahead => "ahead",
            CopyMiss::Chain => "chain",
            CopyMiss::Recreated => "recreated",
            CopyMiss::Desc => "desc",
            CopyMiss::Format => "format",
            CopyMiss::Size => "size",
            CopyMiss::Slot => "slot",
            CopyMiss::Busy => "busy",
            CopyMiss::Pso => "pso",
            CopyMiss::State => "state",
            CopyMiss::Turn => "turn",
            CopyMiss::Pair => "pair",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

/// UiBegin's inputs of the copy gate (ER's render thread; every value plain).
#[derive(Clone, Copy, Debug, Default)]
pub struct CopyIn {
    /// `world_gun_er_depth` is live (15.3) / probe P-S2 asks for a sample this frame.
    pub live: bool,
    pub sample_due: bool,
    pub want: Option<Want>,
    /// `byte [G+0x11c] != 0`, `tid == hal_tid == rt_tid` (C4.1), the HAL queue == the present queue.
    pub mt: bool,
    pub tid: u32,
    pub hal_tid: u32,
    pub rt_tid: u32,
    pub queue_ok: bool,
    /// The UI job runs (movies > 0) with an armed list.
    pub armed: bool,
    /// The previous UiBegin's seq differed (C4.1 `ahead`).
    pub not_ahead: bool,
    /// The 6 pins of this frame's chain, its resource == the watched one, stable for 2 frames.
    pub chain_ok: bool,
    pub res: usize,
    pub watch: usize,
    pub stable: bool,
    /// A fresh GetDesc of `res` (None = refused) and the back buffer's size.
    pub desc: Option<TexDesc>,
    pub bb: (u32, u32),
    /// Our reader exists (worker-created) for this size; the slot's fence passed; nothing armed yet this frame.
    pub ready: bool,
    pub reader_size: (u32, u32),
    pub slot_free: bool,
    pub idle: bool,
}

/// What a passing frame records: the copy (live) and / or the sample (P-S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyWhat {
    pub copy: bool,
    pub sample: bool,
}

/// Design 15.2 step 1 (pure, in this order). The fold's state at the splice point is checked later, in the ECL
/// callback (miss `state`), because the lists ahead of the armed one are submitted after UiBegin returns.
pub fn copy_gate(i: &CopyIn) -> Result<CopyWhat, CopyMiss> {
    let copy = i.live
        && match i.want {
            Some(Want::Gun) => true,
            Some(Want::Fp) => return if i.sample_due { gate_rest(i, false) } else { Err(CopyMiss::Fp) },
            _ => return if i.sample_due { gate_rest(i, false) } else { Err(CopyMiss::NoGun) },
        };
    if !copy && !i.sample_due {
        return Err(CopyMiss::NoGun);
    }
    gate_rest(i, copy)
}

fn gate_rest(i: &CopyIn, copy: bool) -> Result<CopyWhat, CopyMiss> {
    if !i.mt || i.tid == 0 || i.tid != i.hal_tid || i.tid != i.rt_tid {
        return Err(CopyMiss::Thread);
    }
    if !i.queue_ok {
        return Err(CopyMiss::Queue);
    }
    if !i.armed {
        return Err(CopyMiss::NoFrame);
    }
    if !i.not_ahead {
        return Err(CopyMiss::Ahead);
    }
    if !i.chain_ok || i.res == 0 {
        return Err(CopyMiss::Chain);
    }
    if i.res != i.watch || !i.stable {
        return Err(CopyMiss::Recreated);
    }
    let Some(d) = i.desc else { return Err(CopyMiss::Desc) };
    if crate::er_depth::desc_rule(&d, i.bb).is_err() {
        return Err(if d.width != i.bb.0 as u64 || d.height != i.bb.1 { CopyMiss::Size } else { CopyMiss::Desc });
    }
    // the plane-0 SRV (R32_FLOAT_X8X24_TYPELESS) needs the typeless resource
    if d.format != crate::er_depth::FMT_R32G8X24_TYPELESS {
        return Err(CopyMiss::Format);
    }
    if !i.ready {
        return Err(CopyMiss::Pso);
    }
    if i.reader_size != (d.width as u32, d.height) {
        return Err(CopyMiss::Size);
    }
    if !i.idle {
        return Err(CopyMiss::Busy);
    }
    if !i.slot_free {
        return Err(CopyMiss::Slot);
    }
    Ok(CopyWhat { copy, sample: i.sample_due })
}

/// The splice decision in the ECL callback: the state the fold found at the armed call must be readable.
pub fn splice_ok(at_ui: Option<u32>) -> Result<(), CopyMiss> {
    match at_ui {
        Some(s) if readable(s) => Ok(()),
        _ => Err(CopyMiss::State),
    }
}

/// Present's pairing (15.2 step 2): the copy of this Present's seq was spliced and no splice Signal failed since.
pub fn prime_pair(copy_seq: Option<u64>, present_seq: u64, signal_fails_now: u64, signal_fails_at_splice: u64) -> bool {
    copy_seq == Some(present_seq) && signal_fails_now == signal_fails_at_splice
}

// ---- depth math (15.2 step 2) -----------------------------------------------------------------------------------------

/// ER's reverse-Z depth `d` -> view z (metres). `far` None = an infinite far plane (`z = n / d`); else
/// `z = n * f / (d * (f - n) + n)`. None for the sky (d <= 0) or a non-finite value.
pub fn lin(d: f32, near: f32, far: Option<f32>) -> Option<f32> {
    if !(d > 0.0 && d.is_finite() && near > 0.0 && near.is_finite()) {
        return None;
    }
    let z = match far {
        Some(f) if f.is_finite() && f > near => near * f / (d * (f - near) + near),
        _ => near / d,
    };
    (z.is_finite() && z > 0.0).then_some(z)
}

/// The occluder's view z pushed back by the bias (never in front of what ER drew).
pub fn pushed(z: f32, bias_m: f32, bias_rel: f32) -> f32 {
    z + bias_m + bias_rel * z
}

/// Our reversed-Z NDC depth of a view z (`vm_proj::proj_reversed`: ndc = a + b / z).
pub fn our_ndc(z: f32, near: f32, far: f32) -> f32 {
    let a = -near / (far - near);
    let b = near * far / (far - near);
    a + b / z
}

/// What the prime pass writes for ER's `d` (None = discarded: the sky). The shader does exactly this.
pub fn prime_ndc(d: f32, near: f32, far: Option<f32>, bias_m: f32, bias_rel: f32) -> Option<f32> {
    let z = lin(d, near, far)?;
    Some(our_ndc(pushed(z, bias_m, bias_rel), OUR_NEAR, OUR_FAR).clamp(0.0, 1.0))
}

/// The angle between two camera forward vectors (degrees; 0 for a degenerate input).
pub fn turn_deg(f0: [f32; 3], f1: [f32; 3]) -> f32 {
    let n0 = (f0[0] * f0[0] + f0[1] * f0[1] + f0[2] * f0[2]).sqrt();
    let n1 = (f1[0] * f1[0] + f1[1] * f1[1] + f1[2] * f1[2]).sqrt();
    if !(n0 > 1e-6 && n1 > 1e-6) {
        return 0.0;
    }
    let c = ((f0[0] * f1[0] + f0[1] * f1[1] + f0[2] * f1[2]) / (n0 * n1)).clamp(-1.0, 1.0);
    c.acos().to_degrees()
}

/// The half-resolution copy's size (the copy keeps the FARTHEST of each 2x2 block: occluder edges shrink by at most
/// one pixel, never grow, and ER's TAA jitter of <= 0.5 px is absorbed).
pub fn half(w: u32, h: u32) -> (u32, u32) {
    (w.div_ceil(2), h.div_ceil(2))
}

/// The CPU reference of the copy: the farthest (smallest reverse-Z value, the sky's 0 included) of each 2x2 block.
pub fn copy_ref(src: &[f32], w: u32, h: u32) -> Vec<f32> {
    let (hw, hh) = half(w, h);
    let mut out = Vec::with_capacity((hw * hh) as usize);
    for y in 0..hh {
        for x in 0..hw {
            let (x0, y0) = (2 * x, 2 * y);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let at = |x: u32, y: u32| src[(y * w + x) as usize];
            out.push(at(x0, y0).min(at(x1, y0)).min(at(x0, y1)).min(at(x1, y1)));
        }
    }
    out
}

// ---- probe P-S2: the sample and the self check (15.1) -------------------------------------------------------------------

/// The chest cross's arm in pixels at a `w`-wide back buffer.
pub fn cross_px(w: u32) -> u32 {
    ((CHEST_CROSS_PX_4K * w + 1920) / 3840).max(1)
}

/// 16 grid points (4 x 4 at the centres of a 4 x 4 split), the centre, and the chest pixel (or the centre again) with
/// its +-[`cross_px`] cross (clamped to the buffer). Allocation-free (UiBegin's detour builds it).
pub fn sample_points(w: u32, h: u32, chest: Option<(u32, u32)>) -> [[u32; 2]; SAMPLE_N] {
    let mut p = [[0u32; 2]; SAMPLE_N];
    let (w1, h1) = (w.max(1), h.max(1));
    for gy in 0..4u32 {
        for gx in 0..4u32 {
            p[(gy * 4 + gx) as usize] = [((2 * gx + 1) * w1 / 8).min(w1 - 1), ((2 * gy + 1) * h1 / 8).min(h1 - 1)];
        }
    }
    p[16] = [w1 / 2, h1 / 2];
    let [x, y] = match chest {
        Some((x, y)) if x < w1 && y < h1 => [x, y],
        _ => [w1 / 2, h1 / 2],
    };
    let o = cross_px(w1);
    p[CHEST_AT] = [x, y];
    p[CHEST_AT + 1] = [(x + o).min(w1 - 1), y];
    p[CHEST_AT + 2] = [x.saturating_sub(o), y];
    p[CHEST_AT + 3] = [x, (y + o).min(h1 - 1)];
    p[CHEST_AT + 4] = [x, y.saturating_sub(o)];
    p
}

/// The chest texels of a sample: the median ER depth of the 5 and their spread in linear metres (infinite = one of
/// them is the sky / not linearisable). None when the median itself is not linearisable.
pub fn chest_median(d: &[f32; CHEST_N], near: f32, far: Option<f32>) -> Option<(f32, f32)> {
    let mut s = *d;
    s.sort_by(|a, b| a.total_cmp(b));
    let med = s[CHEST_N / 2];
    lin(med, near, far)?;
    let z: Vec<Option<f32>> = d.iter().map(|v| lin(*v, near, far)).collect();
    let spread = if z.iter().all(|v| v.is_some()) {
        let z: Vec<f32> = z.into_iter().flatten().collect();
        z.iter().copied().fold(f32::NEG_INFINITY, f32::max) - z.iter().copied().fold(f32::INFINITY, f32::min)
    } else {
        f32::INFINITY
    };
    Some((med, spread))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conv {
    Reverse,
    Forward,
    Unknown,
}

impl Conv {
    pub fn label(self) -> &'static str {
        match self {
            Conv::Reverse => "reverse",
            Conv::Forward => "forward",
            Conv::Unknown => "unknown",
        }
    }
}

/// The convention of one sample: reverse when the chest texel's `z * d` is about the camera's near plane (an infinite
/// or far reverse-Z: z * d = n near the camera) and no texel is above 1; forward when most texels sit near 1.
pub fn conv_of(grid: &[f32; 16], chest_d: f32, chest_z: Option<f32>, near: f32) -> Conv {
    let ones = grid.iter().filter(|d| **d >= 0.999).count();
    if grid.iter().any(|d| !d.is_finite() || *d > 1.0 || *d < 0.0) {
        return Conv::Unknown;
    }
    if let Some(z) = chest_z
        && chest_d > 0.0
        && near > 0.0
    {
        let r = z * chest_d / near;
        if (0.5..=2.0).contains(&r) && chest_d < 0.999 {
            return Conv::Reverse;
        }
    }
    if ones >= 8 || chest_d >= 0.999 { Conv::Forward } else { Conv::Unknown }
}

/// `DEPTH probe sample n=<k> conv=.. sky=<d> center=<d> zeros=<n>/16 ones=<n>/16 state=0x..`.
pub fn sample_line(n: u32, conv: Conv, grid: &[f32; 16], center: f32, state: u32) -> String {
    let sky = grid.iter().copied().fold(f32::INFINITY, f32::min);
    format!(
        "probe sample n={n} conv={} sky={sky:.6} center={center:.6} zeros={}/16 ones={}/16 state={}",
        conv.label(),
        grid.iter().filter(|d| **d == 0.0).count(),
        grid.iter().filter(|d| **d >= 0.999).count(),
        state_text(state)
    )
}

/// One self-check sample: the 5 chest texels (ER depth), our chest view z (the Spine2 bone), the camera turn (deg) and
/// the camera move (m) of that task tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SelfSample {
    pub d: [f32; CHEST_N],
    pub z: f32,
    pub turn: f32,
    pub mv: f32,
}

impl SelfSample {
    /// The snapshot camera is ER's frame camera: no turn above [`SELF_STILL_DEG`], no move above [`SELF_STILL_MV_M`].
    pub fn still(&self) -> bool {
        self.turn < SELF_STILL_DEG && self.mv < SELF_STILL_MV_M
    }
}

/// The self check's samples (at most 4096).
#[derive(Clone, Debug, Default)]
pub struct SelfTally {
    pub s: Vec<SelfSample>,
}

/// C6 round 7 / part 16.8: how the near / linearisation scale stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// Leverage ([`SCALE_LEV_N`] samples on both sides), a tight fit ([`SCALE_IQR_REL`]) and the slope in the gate.
    Proven,
    /// The slope is outside [`SCALE_S_MIN`]..[`SCALE_S_MAX`]: the linearisation is wrong (fails).
    Bad,
    /// No leverage / not tight yet: live with the bone-on-surface near bound ([`SelfOut::near_rel`]).
    Unproven,
}

impl Scale {
    pub fn label(self) -> &'static str {
        match self {
            Scale::Proven => "proven",
            Scale::Bad => "bad",
            Scale::Unproven => "unproven",
        }
    }
}

/// The self check's result (`DEPTH probe self ...`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelfOut {
    pub n: u32,
    /// Still samples, the edges among them, the judged (still, not an edge) and the ok (near-side residual about the
    /// fitted line < [`SELF_RESID_M`]); `far` = judged samples more than [`SELF_RESID_M`] BEHIND the line (harmless).
    pub still: u32,
    pub edges: u32,
    pub judged: u32,
    pub ok: u32,
    pub far_n: u32,
    /// The fitted offset `o` of `z_bone = s * lin(d) + o` (metres; NaN = none).
    pub o_m: f32,
    /// The near-side residuals' p50 / p95 about the line (cm).
    pub p50_cm: f32,
    pub p95_cm: f32,
    /// The judged samples' bone-z span (m, reported only).
    pub span_m: f32,
    pub scale: Scale,
    /// The fitted slope (Theil-Sen over pairs >= [`SCALE_PAIR_M`] apart; without a pair the median `z / lin(d)`) and the
    /// 25 / 75 % pair slopes (NaN without pairs).
    pub s: f32,
    pub s_lo: f32,
    pub s_hi: f32,
    /// Leverage: judged samples at / below the median lin(d) - [`SCALE_LEV_M`] and at / above + it.
    pub lev_lo: u32,
    pub lev_hi: u32,
    /// `median(z_bone * d) / near_cam` over the judged = median(z_bone / lin(d)) (the bone-on-surface bound).
    pub ratio: f32,
    /// The near the prime should linearise with, relative to CSCam's (>= 1: only ever pushes ER's surfaces back):
    /// unproven `max(1, ratio)`, proven `max(1, s_hi)`; 1 when the check fails.
    pub near_rel: f32,
    /// Metres added to the prime's bias: `-o` when the proven line puts the surface behind the bone (o < 0), else 0.
    pub bias_add_m: f32,
    /// The far model: None = infinite (finite only when proven).
    pub far: Option<f32>,
    pub lag: bool,
}

/// A turn above this per tick does not count for `ok` (the snapshot camera and ER's frame camera may differ).
pub const SELF_STILL_DEG: f32 = 1.0;

fn pct(v: &mut [f32], q: f32) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f32 * q).round() as usize]
}

/// One judged sample of a model: (bone z, lin(d) of the chest median); None = an edge (spread / sky).
fn judged_of(s: &SelfSample, near: f32, far: Option<f32>) -> Option<(f32, f32)> {
    let (med, spread) = chest_median(&s.d, near, far)?;
    if !(spread <= SELF_EDGE_M) {
        return None;
    }
    Some((s.z, lin(med, near, far)?))
}

/// The pair slopes of `z` against `zl` (pairs at least [`SCALE_PAIR_M`] apart in zl), sorted.
fn pair_slopes(p: &[(f32, f32)]) -> Vec<f32> {
    let mut sl = Vec::new();
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            let dx = p[j].1 - p[i].1;
            if dx.abs() >= SCALE_PAIR_M {
                sl.push((p[j].0 - p[i].0) / dx);
            }
        }
    }
    sl.sort_by(|a, b| a.total_cmp(b));
    sl
}

fn median_of(v: impl Iterator<Item = f32>) -> f32 {
    pct(&mut v.collect::<Vec<_>>(), 0.5)
}

/// Part 16.8: the line `z = s * zl + o` through the judged samples: (s, s_lo, s_hi, o). Theil-Sen over the pairs >=
/// [`SCALE_PAIR_M`] apart; without a pair (one range only) a pure scale: s = median(z / zl), o = median(z - s * zl).
pub fn fit_line(j: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    let mut sl = pair_slopes(j);
    let (s, lo, hi) = if sl.is_empty() {
        (median_of(j.iter().map(|(z, zl)| z / zl)), f32::NAN, f32::NAN)
    } else {
        (pct(&mut sl, 0.5), pct(&mut sl, 0.25), pct(&mut sl, 0.75))
    };
    let o = median_of(j.iter().map(|(z, zl)| z - s * zl));
    (s, lo, hi, o)
}

impl SelfTally {
    /// The single-texel form (the 5 chest texels equal, no camera move): tests.
    pub fn add(&mut self, d: f32, chest_z: f32, turn: f32) {
        self.add5([d; CHEST_N], chest_z, turn, 0.0);
    }

    pub fn add5(&mut self, d: [f32; CHEST_N], chest_z: f32, turn: f32, mv: f32) {
        if self.s.len() < 4096 && d.iter().all(|v| v.is_finite()) && chest_z.is_finite() && turn.is_finite() && mv.is_finite() {
            self.s.push(SelfSample { d, z: chest_z, turn, mv });
        }
    }

    /// The judged (bone z, lin) pairs of a model, still or moving.
    fn pairs(&self, near: f32, far: Option<f32>, still: bool) -> Vec<(f32, f32)> {
        self.s.iter().filter(|s| s.still() == still).filter_map(|s| judged_of(s, near, far)).collect()
    }

    /// Is the finite far model proven (review 2026-10-21): only on still samples where the two models differ by at least
    /// [`FINITE_EVIDENCE_M`] (at chest range they differ by ~z^2/F, millimetres), at least [`FINITE_MIN_N`] of them, and
    /// the finite median residual (each model against its own fitted offset) better by [`FINITE_MARGIN_M`].
    fn finite_proven(&self, near: f32, far: f32) -> bool {
        let oi = median_of(self.pairs(near, None, true).iter().map(|(z, zl)| z - zl));
        let of = median_of(self.pairs(near, Some(far), true).iter().map(|(z, zl)| z - zl));
        let (mut ei, mut ef) = (Vec::new(), Vec::new());
        for s in self.s.iter().filter(|s| s.still()) {
            let (Some((z, zi)), Some((_, zf))) = (judged_of(s, near, None), judged_of(s, near, Some(far))) else { continue };
            if (zi - zf).abs() < FINITE_EVIDENCE_M {
                continue;
            }
            ei.push((z - zi - oi).abs());
            ef.push((z - zf - of).abs());
        }
        if ei.len() < FINITE_MIN_N {
            return false;
        }
        let (pi, pf) = (pct(&mut ei, 0.5), pct(&mut ef, 0.5));
        pi.is_finite() && pf.is_finite() && pf + FINITE_MARGIN_M < pi
    }

    /// The result with CSCam's near / far (part 16.8, depth8.md 1): the infinite far model unless the finite one is
    /// proven; the line `z_bone = s * lin(d) + o` fitted on every judged still sample; the scale proven only with
    /// leverage and a tight fit; the near the prime uses always on the safe side (`near_rel >= 1`: ER's surfaces are only
    /// ever pushed back, never pulled in front of a gun).
    pub fn result(&self, near: f32, far: f32) -> SelfOut {
        let far_m = (far.is_finite() && far > near && self.finite_proven(near, far)).then_some(far);
        let still = self.s.iter().filter(|s| s.still()).count() as u32;
        let edges = self.s.iter().filter(|s| s.still() && judged_of(s, near, far_m).is_none()).count() as u32;
        let j = self.pairs(near, far_m, true);
        // the fit skips only gross outliers about the pure-scale line (a chest pixel on another surface: 23:42:39 k16
        // 0.9 m behind); they still count below (ok / far / p95), so they can fail the check but never pass it
        let r0 = median_of(j.iter().map(|(z, zl)| z / zl));
        let jf: Vec<(f32, f32)> = j.iter().copied().filter(|(z, zl)| (z - r0 * zl).abs() <= FIT_GROSS_M).collect();
        let (s, s_lo, s_hi, o_m) = if jf.is_empty() { (f32::NAN, f32::NAN, f32::NAN, f32::NAN) } else { fit_line(&jf) };
        // signed residual: > 0 = the measured surface is NEARER than the line (the phantom-occlusion side)
        let signed: Vec<f32> = j.iter().map(|(z, zl)| z - s * zl - o_m).collect();
        let mut res: Vec<f32> = signed.iter().map(|r| r.max(0.0)).collect();
        let ok = res.iter().filter(|r| **r < SELF_RESID_M).count() as u32;
        let far_n = signed.iter().filter(|r| **r <= -SELF_RESID_M).count() as u32;
        let p50 = pct(&mut res, 0.5);
        let p95 = pct(&mut res, 0.95);
        let span_m = if j.is_empty() {
            0.0
        } else {
            j.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max) - j.iter().map(|p| p.0).fold(f32::INFINITY, f32::min)
        };
        let med_zl = median_of(j.iter().map(|p| p.1));
        let lev_lo = j.iter().filter(|p| p.1 <= med_zl - SCALE_LEV_M).count() as u32;
        let lev_hi = j.iter().filter(|p| p.1 >= med_zl + SCALE_LEV_M).count() as u32;
        let ratio = median_of(j.iter().map(|(z, zl)| z / zl));
        let in_gate = s.is_finite() && (SCALE_S_MIN..=SCALE_S_MAX).contains(&s);
        let tight = s_lo.is_finite() && s_hi.is_finite() && s_lo >= s * (1.0 - SCALE_IQR_REL) && s_hi <= s * (1.0 + SCALE_IQR_REL);
        let lev = lev_lo as usize >= SCALE_LEV_N && lev_hi as usize >= SCALE_LEV_N;
        let scale = if !in_gate && j.len() >= SELF_MIN_JUDGED {
            Scale::Bad
        } else if lev && tight && in_gate {
            Scale::Proven
        } else {
            Scale::Unproven
        };
        let (near_rel, bias_add_m) = match scale {
            Scale::Proven => (s_hi.max(1.0).min(SCALE_S_MAX), (-o_m).max(0.0)),
            Scale::Unproven => (if ratio.is_finite() { ratio.clamp(1.0, SCALE_S_MAX) } else { 1.0 }, 0.0),
            Scale::Bad => (1.0, 0.0),
        };
        let te: Vec<f32> = self.pairs(near, far_m, false).iter().map(|(z, zl)| (z - s * zl - o_m).abs()).collect();
        let n_te = te.len();
        let tp50 = median_of(te.into_iter());
        let lag = n_te >= 3 && tp50.is_finite() && tp50 > (2.0 * p50).max(0.10);
        SelfOut {
            n: self.s.len() as u32,
            still,
            edges,
            judged: j.len() as u32,
            ok,
            far_n,
            o_m,
            p50_cm: p50 * 100.0,
            p95_cm: p95 * 100.0,
            span_m,
            scale,
            s,
            s_lo,
            s_hi,
            lev_lo,
            lev_hi,
            ratio,
            near_rel,
            bias_add_m,
            far: far_m,
            lag,
        }
    }
}

/// Why the self check fails (None = it passes): too few judged samples, too many edges, the fitted offset outside
/// [`FIT_O_MIN_M`]..[`FIT_O_MAX_M`], the near-side residual spread, the p95 tail, a slope outside the gate.
pub fn self_why(o: &SelfOut) -> Option<&'static str> {
    if (o.judged as usize) < SELF_MIN_JUDGED {
        Some("few")
    } else if o.edges as f32 > SELF_EDGE_MAX * o.still as f32 {
        Some("edge")
    } else if !(o.o_m >= FIT_O_MIN_M && o.o_m <= FIT_O_MAX_M) {
        Some("offset")
    } else if (o.ok as f32) < 0.9 * o.judged as f32 {
        Some("spread")
    } else if !(o.p95_cm < SELF_P95_M * 100.0) {
        Some("p95")
    } else if o.scale == Scale::Bad {
        Some("scale")
    } else {
        None
    }
}

/// The self check passes (the fitted line, the residuals, the edges and the scale; see [`self_why`]).
pub fn self_pass(o: &SelfOut) -> bool {
    self_why(o).is_none()
}

/// Judged samples a live candidate's offset / spread failure needs before it trips (twice the pass minimum: one noisy
/// sample cannot trip, a model error that grows with the data does).
pub const SELF_TRIP_MIN_JUDGED: usize = 2 * SELF_MIN_JUDGED;

/// Part 16.8 step 4: the live candidate's self check (it keeps running while live) trips `self` (capsules for the
/// session) when, with leverage, the slope's low end beats the live near ratio (`live_rel` = the near the prime uses /
/// near_cam) by more than [`SCALE_TRIP_REL`] (ER's surfaces would sit nearer than linearised: phantom occlusion), or the
/// slope is out of the gate with leverage; and (round 7 review) on `offset` / `spread` over at least
/// [`SELF_TRIP_MIN_JUDGED`] judged samples. A slope of ~1.10 never trips once the live near already carries it.
pub fn self_trips(o: &SelfOut, live_rel: f32) -> bool {
    let lev = o.lev_lo as usize >= SCALE_LEV_N && o.lev_hi as usize >= SCALE_LEV_N;
    if lev && o.s_lo.is_finite() && o.s_lo > live_rel * (1.0 + SCALE_TRIP_REL) {
        return true;
    }
    match self_why(o) {
        Some("scale") => lev,
        Some("offset" | "spread") => o.judged as usize >= SELF_TRIP_MIN_JUDGED,
        _ => false,
    }
}

/// `DEPTH probe self n=.. ok=a/J far=.. edges=e/S o_cm=.. err_cm_p50=.. p95=.. span_m=.. scale=.. s=.. s_iqr=lo..hi
/// n_eff=.. near_rel=.. bias_add_cm=.. lev=lo/hi ratio=.. near_cam=.. far_cam=.. model=.. lag=0|1 why=.. pass=0|1`.
/// `n_eff` = the near the prime linearises with (near_cam * near_rel).
pub fn self_line(o: &SelfOut, near_cam: f32, far_cam: f32) -> String {
    format!(
        "probe self n={} ok={}/{} far={} edges={}/{} o_cm={:.1} err_cm_p50={:.1} p95={:.1} span_m={:.2} scale={} s={:.3} s_iqr={:.3}..{:.3} n_eff={:.4} near_rel={:.3} bias_add_cm={:.1} lev={}/{} ratio={:.3} near_cam={near_cam:.4} far_cam={far_cam:.1} model={} lag={} why={} pass={}",
        o.n,
        o.ok,
        o.judged,
        o.far_n,
        o.edges,
        o.still,
        o.o_m * 100.0,
        o.p50_cm,
        o.p95_cm,
        o.span_m,
        o.scale.label(),
        o.s,
        o.s_lo,
        o.s_hi,
        near_cam * o.near_rel,
        o.near_rel,
        o.bias_add_m * 100.0,
        o.lev_lo,
        o.lev_hi,
        o.ratio,
        if o.far.is_some() { "finite" } else { "inf" },
        o.lag as u8,
        self_why(o).unwrap_or("none"),
        self_pass(o) as u8
    )
}

/// `DEPTH probe self_s k=.. z=.. d=.. zl=.. o=.. spread=.. edge=0|1 turn=.. mv=.. zd=..` (the first still samples,
/// once: offset vs scale readable from the log; `zd` = z_bone * d, flat across ranges = a pure near scale, 16.8).
pub fn self_s_line(k: u32, s: &SelfSample, near: f32, far: Option<f32>) -> String {
    let cm = chest_median(&s.d, near, far);
    let zl = cm.and_then(|(m, _)| lin(m, near, far));
    let edge = !cm.is_some_and(|c| c.1 <= SELF_EDGE_M);
    format!(
        "probe self_s k={k} z={:.3} d={:.6} zl={} o={} spread={} edge={} turn={:.2} mv={:.3} zd={:.5}",
        s.z,
        cm.map_or(f32::NAN, |c| c.0),
        zl.map_or("?".to_string(), |v| format!("{v:.3}")),
        zl.map_or("?".to_string(), |v| format!("{:.3}", s.z - v)),
        cm.map_or("?".to_string(), |c| if c.1.is_finite() { format!("{:.3}", c.1) } else { "sky".to_string() }),
        edge as u8,
        s.turn,
        s.mv,
        cm.map_or(f32::NAN, |c| c.0 * s.z)
    )
}

// ---- the live gate (15.3) -------------------------------------------------------------------------------------------

/// A per-build pin of the verified behaviour (None until a passing run is recorded in MODLOG).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatePin {
    pub readable_state: u32,
    pub infinite_far: bool,
}

/// The pin per build: none yet (2026-10-21: no in-game run of P-S has been made).
pub const STATE_PIN_2720: Option<StatePin> = None;

#[derive(Clone, Copy, Debug, Default)]
pub struct LiveIn {
    pub cfg_on: bool,
    pub pinned: bool,
    pub probe_state: bool,
    pub probe_sample: bool,
    pub hook_on: bool,
    pub ui_hooks: bool,
    /// The last state verdict (None: not judged yet).
    pub verdict: Option<bool>,
    /// The sample's convention, the self check's pass, the near fit, a D3D12 failure of the sample.
    pub conv: Option<Conv>,
    pub self_ok: Option<bool>,
    pub sample_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveStep {
    Wait,
    On,
    Off(&'static str),
}

/// Design 15.3: `world_gun_er_depth` goes live only on this session's ready verdict + a reverse-Z sample + a passing
/// self check (no pin exists yet, so the probe flags are required).
pub fn live_step(i: &LiveIn) -> LiveStep {
    if !i.cfg_on {
        return LiveStep::Off("config");
    }
    if !i.ui_hooks {
        return LiveStep::Off("ui_hooks");
    }
    if !(i.probe_state && i.probe_sample) {
        return LiveStep::Off(if i.pinned { "pin_needs_probe" } else { "unpinned" });
    }
    if !i.hook_on {
        return LiveStep::Off("hook");
    }
    if i.sample_failed {
        return LiveStep::Off("sample");
    }
    match i.verdict {
        None => return LiveStep::Wait,
        Some(false) => return LiveStep::Off("probe_verdict"),
        Some(true) => {}
    }
    match (i.conv, i.self_ok) {
        (Some(Conv::Reverse), Some(true)) => LiveStep::On,
        (Some(Conv::Forward), _) => LiveStep::Off("conv"),
        (Some(Conv::Reverse), Some(false)) => LiveStep::Wait,
        _ => LiveStep::Wait,
    }
}

/// Design 15.3 trips while live (the whole session falls back to the capsules): a fold mismatch, a second queue /
/// thread on the resource, a split / aliasing barrier, a full table (counters compared with the values at going live).
pub fn trip_of(at_live: &StateStats, now: &StateStats) -> Option<&'static str> {
    if now.mismatch > at_live.mismatch {
        Some("mismatch")
    } else if now.queues > 1 || now.threads > 1 {
        Some("queue")
    } else if now.split > at_live.split {
        Some("split")
    } else if now.alias > at_live.alias {
        Some("alias")
    } else if now.table_full > at_live.table_full {
        Some("table_full")
    } else if now.write_at_ui > at_live.write_at_ui || now.unknown > at_live.unknown {
        Some("state")
    } else {
        None
    }
}

/// `DEPTH live n=.. copies=.. primed=.. miss=<why:n,..>`.
pub fn live_line(n: u64, copies: u64, primed: u64, miss: &[(CopyMiss, u64)]) -> String {
    let m: Vec<String> = miss.iter().filter(|(_, v)| *v > 0).map(|(k, v)| format!("{}:{v}", k.label())).collect();
    format!("live n={n} copies={copies} primed={primed} miss={}", if m.is_empty() { "none".to_string() } else { m.join(",") })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: usize = 0x7000_1000;
    const L1: usize = 0x5000_0010;
    const L2: usize = 0x5000_0020;
    const L3: usize = 0x5000_0030;
    const Q: usize = 0x9000_0000;
    const TID: u32 = 77;
    const TARGET: usize = 0x7ff0_0000;

    fn tr(sub: u32, before: u32, after: u32) -> Bar {
        Bar::Transition { res: RES, sub, before, after, flags: 0 }
    }

    fn ecl<'a>(lists: &'a [usize], armed: bool, seq: u64) -> EclIn<'a> {
        EclIn { queue: Q, tid: TID, lists, watch: RES, epoch: 1, armed, seq, judged: true, hal_queue: Q, rt_tid: TID, target: TARGET }
    }

    fn vt_ok(_: usize) -> Option<usize> {
        Some(TARGET)
    }

    #[test]
    fn c6_depth_state_readable_rule() {
        for s in [0xC0, 0xE0, 0x40, 0x60] {
            assert!(readable(s), "0x{s:x}");
        }
        for s in [0x10, 0x30, 0x80, 0x20, 0x4C0, 0xC8, 0x0, UNKNOWN, SPLIT, 0x1040] {
            assert!(!readable(s), "0x{s:x}");
        }
        assert_eq!(state_text(UNKNOWN), "unknown");
        assert_eq!(state_text(0xC0), "0xc0");
    }

    /// The scripted frame of ED5: ER writes the depth (0xC0 -> DEPTH_WRITE), reads it back (-> 0xC0), the UI call;
    /// per plane and ALL, a split, a re-record of a folded list.
    #[test]
    fn c6_depth_state_fold_follows_the_script() {
        let t = Table::new();
        let f = Fold::new();
        // frame 1: L1 0xC0 -> 0x10 (all), L2 plane 0 0x10 -> 0xC0 and plane 1 0x10 -> 0x20
        t.record(L1, TID, RES, tr(SUB_ALL, 0xC0, 0x10));
        t.record(L2, TID, RES, tr(0, 0x10, 0xC0));
        t.record(L2, TID, RES, tr(1, 0x10, 0x20));
        // barriers on other resources are ignored
        t.record(L2, TID, RES, Bar::Transition { res: RES + 8, sub: 0, before: 0x4, after: 0xC0, flags: 0 });
        assert_eq!(t.pending(L1), Some(([0xC0, 0xC0], [0x10, 0x10], 1)));
        assert_eq!(t.pending(L2), Some(([0x10, 0x10], [0xC0, 0x20], 2)));
        assert_eq!(f.ecl(&t, &ecl(&[L1, L2], false, 1), &vt_ok), None);
        assert_eq!(f.q(), [0xC0, 0x20]);
        // the armed UI call: the state at UI is Q[0] before its lists
        assert_eq!(f.ecl(&t, &ecl(&[L3], true, 1), &vt_ok), Some(0xC0));
        assert_eq!(f.at_ui(1), Some(0xC0));
        // a second armed call of the same seq does not count again
        assert_eq!(f.ecl(&t, &ecl(&[L3], true, 1), &vt_ok), None);
        // frame 2: L1 re-recorded (a barrier into a folded entry starts a new recording)
        t.record(L1, TID, RES, tr(SUB_ALL, 0xC0, 0x10));
        assert_eq!(t.pending(L1), Some(([0xC0, 0xC0], [0x10, 0x10], 1)));
        // stencil was 0x20: plane 1's first before 0xC0 is a mismatch
        f.ecl(&t, &ecl(&[L1], false, 2), &vt_ok);
        assert_eq!(f.mismatch.load(Ordering::Relaxed), 1);
        // L2 executed again WITHOUT a re-record of a barrier: folded already, no state change
        f.ecl(&t, &ecl(&[L2], false, 2), &vt_ok);
        assert_eq!(f.q(), [0x10, 0x10]);
        assert_eq!(f.ecl(&t, &ecl(&[L3], true, 2), &vt_ok), Some(0x10));
        let s = f.stats(&t);
        assert_eq!((s.frames, s.readable, s.write_at_ui, s.h_c0, s.h_10, s.lists, s.transitions), (2, 1, 1, 1, 1, 3, 4));
        assert_eq!((s.queues, s.threads, s.vt_eq, s.vt_n), (1, 1, 3, 3));
        // the path of frame 2 (published at its armed call)
        let (_, seq, path, at) = f.last_path();
        assert_eq!((seq, at), (2, 0x10));
        assert_eq!(path.len(), 1);
        assert_eq!(path_entry_text(path[0]), "0xc0>0x10 sub=all q=hal t=rt");
        assert!(path_line(seq, &path, at).starts_with("state path seq=2 [0xc0>0x10 sub=all q=hal t=rt] at_ui=0x10"));
    }

    #[test]
    fn c6_depth_state_split_alias_uav_subresource_full() {
        let t = Table::new();
        let f = Fold::new();
        t.record(L1, TID, RES, Bar::Transition { res: RES, sub: 0, before: 0xC0, after: 0x10, flags: FLAG_BEGIN_ONLY });
        assert_eq!(t.pending(L1).map(|p| p.1[0]), Some(SPLIT));
        t.record(L1, TID, RES, Bar::Transition { res: RES, sub: 0, before: 0xC0, after: 0x10, flags: FLAG_END_ONLY });
        assert_eq!(t.pending(L1).map(|p| p.1[0]), Some(0x10));
        t.record(L1, TID, RES, Bar::Alias { before: 0, after: RES });
        t.record(L1, TID, RES, Bar::Uav { res: RES });
        t.record(L1, TID, RES, tr(5, 0x10, 0xC0));
        t.record(L1, TID, RES, Bar::Alias { before: 1, after: 2 });
        f.ecl(&t, &ecl(&[L1], false, 1), &|_| Some(1));
        let s = f.stats(&t);
        assert_eq!((s.split, s.alias, s.uav, s.bad_sub, s.vt_eq, s.vt_n), (2, 1, 1, 1, 0, 1));
        // a full table counts, never blocks
        let t2 = Table::new();
        for k in 0..TABLE_N {
            t2.record(0x1000 + 16 * k, TID, RES, tr(0, 0x10, 0xC0));
        }
        assert_eq!(t2.full.load(Ordering::Relaxed), 0);
        t2.record(0x9999_0000, TID, RES, tr(0, 0x10, 0xC0));
        assert_eq!(t2.full.load(Ordering::Relaxed), 1);
        // no watch: nothing recorded
        let t3 = Table::new();
        t3.record(L1, TID, 0, tr(0, 0x10, 0xC0));
        assert_eq!(t3.pending(L1), None);
    }

    #[test]
    fn c6_depth_state_new_resource_resets_and_queues_count() {
        let t = Table::new();
        let f = Fold::new();
        t.record(L1, TID, RES, tr(SUB_ALL, 0x10, 0xC0));
        f.ecl(&t, &ecl(&[L1], false, 1), &vt_ok);
        assert_eq!(f.q()[0], 0xC0);
        // a new epoch (the watched resource changed): Q unknown, an entry of the old resource is unmatched
        t.record(L2, TID, RES, tr(SUB_ALL, 0x10, 0xC0));
        let mut i = ecl(&[L3], true, 2);
        i.epoch = 2;
        assert_eq!(f.ecl(&t, &i, &vt_ok), Some(UNKNOWN));
        let mut i = ecl(&[L2], false, 2);
        i.epoch = 2;
        i.watch = RES + 0x100;
        f.ecl(&t, &i, &vt_ok);
        assert_eq!(f.unmatched.load(Ordering::Relaxed), 1);
        // another queue and another thread
        t.record(L1, TID, RES, tr(SUB_ALL, 0xC0, 0xC0));
        let mut i = ecl(&[L1], false, 3);
        i.epoch = 2;
        i.queue = Q + 8;
        i.tid = TID + 1;
        f.ecl(&t, &i, &vt_ok);
        assert_eq!((f.queues(), f.threads()), (2, 2));
    }

    fn good_stats() -> StateStats {
        StateStats { frames: 700, readable: 700, h_c0: 700, queues: 1, threads: 1, vt_eq: 1400, vt_n: 1400, lists: 1400, ..StateStats::default() }
    }

    #[test]
    fn c6_depth_state_verdict_every_row() {
        let g = good_stats();
        assert_eq!(verdict(&g), (true, "none"));
        assert!(verdict_line(&g).starts_with("state verdict ready=1 frames=700 readable=700/700"));
        for (m, why) in [
            (StateStats { frames: 599, readable: 599, ..g }, "frames"),
            (StateStats { write_at_ui: 1, ..g }, "write_at_ui"),
            (StateStats { unknown: 1, ..g }, "unknown"),
            (StateStats { readable: 699, ..g }, "readable"),
            (StateStats { queues: 2, ..g }, "queues"),
            (StateStats { threads: 2, ..g }, "threads"),
            (StateStats { mismatch: 1, ..g }, "mismatch"),
            (StateStats { split: 1, ..g }, "split"),
            (StateStats { alias: 1, ..g }, "alias"),
            (StateStats { table_full: 1, ..g }, "table_full"),
            (StateStats { bad_sub: 1, ..g }, "subresource"),
            (StateStats { vt_eq: 1399, ..g }, "vt_eq"),
            (StateStats { vt_n: 0, vt_eq: 0, ..g }, "vt_eq"),
        ] {
            assert_eq!(verdict(&m), (false, why), "{why}");
        }
        assert!(state_text_line(RES, 0xC0, &g).contains("at_ui={0xC0:700,0xE0:0,0x10:0,unknown:0,other:0} frames=700"));
    }

    fn good_copy() -> CopyIn {
        CopyIn {
            live: true,
            sample_due: false,
            want: Some(Want::Gun),
            mt: true,
            tid: 5,
            hal_tid: 5,
            rt_tid: 5,
            queue_ok: true,
            armed: true,
            not_ahead: true,
            chain_ok: true,
            res: RES,
            watch: RES,
            stable: true,
            desc: Some(TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: 19, samples: 1, flags: 0x2 }),
            bb: (3840, 2160),
            ready: true,
            reader_size: (3840, 2160),
            slot_free: true,
            idle: true,
        }
    }

    #[test]
    fn c6_depth_state_copy_gate_every_miss() {
        let g = good_copy();
        assert_eq!(copy_gate(&g), Ok(CopyWhat { copy: true, sample: false }));
        assert_eq!(copy_gate(&CopyIn { sample_due: true, ..g }), Ok(CopyWhat { copy: true, sample: true }));
        // the sample alone in first person / with no gun / not live
        assert_eq!(copy_gate(&CopyIn { want: Some(Want::Fp), sample_due: true, ..g }), Ok(CopyWhat { copy: false, sample: true }));
        assert_eq!(copy_gate(&CopyIn { live: false, sample_due: true, ..g }), Ok(CopyWhat { copy: false, sample: true }));
        let d = g.desc.unwrap();
        for (m, why) in [
            (CopyIn { want: Some(Want::Fp), ..g }, CopyMiss::Fp),
            (CopyIn { want: Some(Want::NoGun), ..g }, CopyMiss::NoGun),
            (CopyIn { want: None, ..g }, CopyMiss::NoGun),
            (CopyIn { live: false, ..g }, CopyMiss::NoGun),
            (CopyIn { mt: false, ..g }, CopyMiss::Thread),
            (CopyIn { rt_tid: 6, ..g }, CopyMiss::Thread),
            (CopyIn { hal_tid: 6, ..g }, CopyMiss::Thread),
            (CopyIn { queue_ok: false, ..g }, CopyMiss::Queue),
            (CopyIn { armed: false, ..g }, CopyMiss::NoFrame),
            (CopyIn { not_ahead: false, ..g }, CopyMiss::Ahead),
            (CopyIn { chain_ok: false, ..g }, CopyMiss::Chain),
            (CopyIn { watch: RES + 8, ..g }, CopyMiss::Recreated),
            (CopyIn { stable: false, ..g }, CopyMiss::Recreated),
            (CopyIn { desc: None, ..g }, CopyMiss::Desc),
            (CopyIn { desc: Some(TexDesc { flags: 0xa, ..d }), ..g }, CopyMiss::Desc),
            (CopyIn { desc: Some(TexDesc { width: 1920, ..d }), ..g }, CopyMiss::Size),
            (CopyIn { desc: Some(TexDesc { format: 20, ..d }), ..g }, CopyMiss::Format),
            (CopyIn { ready: false, ..g }, CopyMiss::Pso),
            (CopyIn { reader_size: (1920, 1080), ..g }, CopyMiss::Size),
            (CopyIn { idle: false, ..g }, CopyMiss::Busy),
            (CopyIn { slot_free: false, ..g }, CopyMiss::Slot),
        ] {
            assert_eq!(copy_gate(&m), Err(why), "{why:?}");
        }
        assert_eq!(splice_ok(Some(0xC0)), Ok(()));
        for s in [None, Some(0x10), Some(UNKNOWN), Some(SPLIT)] {
            assert_eq!(splice_ok(s), Err(CopyMiss::State));
        }
        assert!(prime_pair(Some(9), 9, 0, 0));
        assert!(!prime_pair(Some(8), 9, 0, 0) && !prime_pair(None, 9, 0, 0) && !prime_pair(Some(9), 9, 1, 0));
        let labels: Vec<&str> = CopyMiss::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(labels.len(), 17);
        assert!(CopyMiss::ALL.iter().enumerate().all(|(k, m)| m.index() == k));
        assert_eq!(live_line(3, 2, 1, &[(CopyMiss::Turn, 4), (CopyMiss::Fp, 0)]), "live n=3 copies=2 primed=1 miss=turn:4");
    }

    #[test]
    fn c6_depth_state_lin_prime_and_turn() {
        // infinite far: z = n / d
        assert!((lin(0.01, 0.1, None).unwrap() - 10.0).abs() < 1e-4);
        // finite far: d = n (f - z) / (z (f - n)) at z = 10 -> back to 10
        let (n, f, z) = (0.1f32, 1000.0f32, 10.0f32);
        let d = n * (f - z) / (z * (f - n));
        assert!((lin(d, n, Some(f)).unwrap() - z).abs() < 1e-3);
        assert_eq!(lin(0.0, 0.1, None), None);
        assert_eq!(lin(f32::NAN, 0.1, None), None);
        assert_eq!(lin(0.5, 0.0, None), None);
        // our projection matches vm_proj::proj_reversed
        let p = crate::vm_proj::proj_reversed(1.0, 1.0, OUR_NEAR, OUR_FAR);
        let zz = 7.5f32;
        assert!(((p[2][2] * zz + p[2][3]) / zz - our_ndc(zz, OUR_NEAR, OUR_FAR)).abs() < 1e-6);
        // the prime pushes back: a gun 20 cm behind ER's wall at 10 m is hidden, 10 cm in front stays
        let dw = 0.1 / 10.0;
        let prime = prime_ndc(dw, 0.1, None, 0.03, 0.004).unwrap();
        let gun_behind = our_ndc(10.0 + 0.2, OUR_NEAR, OUR_FAR);
        let gun_front = our_ndc(10.0 - 0.1, OUR_NEAR, OUR_FAR);
        assert!(gun_behind < prime && gun_front > prime);
        // inside the bias (3 cm + 4 cm at 10 m): visible (never a false cut from the jitter / near mismatch)
        assert!(our_ndc(10.05, OUR_NEAR, OUR_FAR) > prime);
        assert_eq!(prime_ndc(0.0, 0.1, None, 0.03, 0.004), None);
        assert!((pushed(10.0, 0.03, 0.004) - 10.07).abs() < 1e-5);
        assert!((turn_deg([0.0, 0.0, 1.0], [0.0, 0.0, 1.0])).abs() < 1e-3);
        assert!((turn_deg([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]) - 90.0).abs() < 1e-3);
        assert_eq!(turn_deg([0.0; 3], [1.0, 0.0, 0.0]), 0.0);
        // the half copy: the farthest of each block, odd sizes clamp
        assert_eq!(half(3840, 2160), (1920, 1080));
        assert_eq!(half(5, 3), (3, 2));
        let src = [0.5, 0.4, 0.9, 0.3, 0.6, 0.0, 0.8, 0.7, 0.2, 0.1, 1.0, 0.5, 0.5, 0.5, 0.5];
        assert_eq!(copy_ref(&src, 5, 3), vec![0.0, 0.2, 0.1, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn c6_depth_state_sample_conv_and_self_check() {
        let p = sample_points(3840, 2160, Some((100, 200)));
        assert_eq!(p[0], [480, 270]);
        assert_eq!(p[15], [3360, 1890]);
        assert_eq!((p[16], p[17]), ([1920, 1080], [100, 200]));
        assert_eq!(sample_points(3840, 2160, Some((5000, 1)))[17], [1920, 1080]);
        // C6 round 7: the chest cross (+-8 px at 4K, scaled, clamped at the edges)
        assert_eq!(&p[18..22], &[[108, 200], [92, 200], [100, 208], [100, 192]]);
        assert_eq!((cross_px(3840), cross_px(1920), cross_px(100)), (8, 4, 1));
        let e = sample_points(1920, 1080, Some((1, 1079)));
        assert_eq!(&e[17..22], &[[1, 1079], [5, 1079], [0, 1079], [1, 1079], [1, 1075]]);
        // reverse-Z: the chest at 3 m with near 0.1 (inf far): d = 0.1 / 2.85
        let near = 0.1;
        let zc = 3.0;
        let d = near / (zc - CHEST_FRONT_M);
        let mut grid = [0.0f32; 16];
        grid[8] = 0.02;
        assert_eq!(conv_of(&grid, d, Some(zc - CHEST_FRONT_M), near), Conv::Reverse);
        assert_eq!(conv_of(&[1.0; 16], 0.9995, Some(2.85), near), Conv::Forward);
        assert_eq!(conv_of(&[0.5; 16], 0.5, Some(2.85), near), Conv::Unknown);
        assert_eq!(conv_of(&[1.5; 16], d, Some(2.85), near), Conv::Unknown);
        assert!(sample_line(1, Conv::Reverse, &grid, 0.02, 0xC0).starts_with("probe sample n=1 conv=reverse sky=0.000000 center=0.020000 zeros=15/16 ones=0/16 state=0xc0"));
        // the chest median and its spread (a sky texel = infinite spread)
        let (m, sp) = chest_median(&[near / 3.0, near / 3.1, near / 2.9, near / 3.05, near / 1.0], near, None).unwrap();
        assert!((m - near / 3.0).abs() < 1e-7 && (sp - 2.1).abs() < 1e-3, "{m} {sp}");
        assert_eq!(chest_median(&[near / 3.0, 0.0, near / 3.0, near / 3.0, near / 3.0], near, None).map(|c| c.1), Some(f32::INFINITY));
        assert_eq!(chest_median(&[0.0, 0.0, 0.0, near / 3.0, near / 3.0], near, None), None, "the median is the sky");
        // the self check: still samples over a span, offset 0.12 m: pass, scale proven, slope 1
        let mut t = SelfTally::default();
        for k in 0..10 {
            let z = 2.0 + k as f32 * 0.3;
            t.add(near / (z - 0.12), z, 0.2);
        }
        let o = t.result(near, 5000.0);
        assert!(self_pass(&o), "{o:?}");
        assert_eq!(o.scale, Scale::Proven, "{o:?}");
        assert!((o.o_m - 0.12).abs() < 0.005 && (o.s - 1.0).abs() < 0.01 && o.near_rel == 1.0 && o.bias_add_m == 0.0, "{o:?}");
        assert!(!o.lag);
        assert!(self_line(&o, near, 5000.0).contains(" ok=10/10 far=0 edges=0/10 o_cm=12.0 "), "{}", self_line(&o, near, 5000.0));
        assert!(self_line(&o, near, 5000.0).contains(" scale=proven s=1.000 ") && self_line(&o, near, 5000.0).ends_with(" why=none pass=1"));
        assert!(self_line(&o, near, 5000.0).contains(" n_eff=0.1000 near_rel=1.000 ") && self_line(&o, near, 5000.0).contains(" lev=5/4 "), "{}", self_line(&o, near, 5000.0));
        // C6 16.8: a wrong near (ER's 0.2): the line's slope is half -> outside the gate: scale (fails)
        let o2 = t.result(0.2, 5000.0);
        assert!(!self_pass(&o2), "{o2:?}");
        assert_eq!((self_why(&o2), o2.scale, o2.near_rel), (Some("scale"), Scale::Bad, 1.0));
        // too few samples
        let mut t3 = SelfTally::default();
        t3.add(d, zc, 0.0);
        assert_eq!(self_why(&t3.result(near, 5000.0)), Some("few"));
        // lag: turning samples with a large residual
        for _ in 0..4 {
            t.add(near / 1.0, 4.0, 5.0);
        }
        assert!(t.result(near, 5000.0).lag);
        // a moving camera is not still (never judged)
        let mut tm = SelfTally::default();
        tm.add5([d; CHEST_N], zc, 0.2, 0.08);
        assert_eq!((tm.result(near, 5000.0).still, tm.s[0].still()), (0, false));
        let s0 = SelfSample { d: [near / 2.66; CHEST_N], z: 3.0, turn: 0.1, mv: 0.01 };
        assert!(self_s_line(1, &s0, near, None).starts_with("probe self_s k=1 z=3.000 d=0.037594 zl=2.660 o=0.340 spread=0.000 edge=0 "));
    }

    /// C6 round 7 (work/c6/depth7.md 2) / part 16.8 (work/c6/depth8.md 2): the two-parameter self fit, its leverage rule,
    /// the safe-side near and the trips.
    #[test]
    fn c6_depth_state_self_fit_offset_scale_edges() {
        let near = 0.0501;
        // (a) the 22:42 shape: one camera distance (+-4 %), ER's surface at 0.34 m in front of Spine2: one range cannot
        // tell offset from scale, so the fit is the pure scale (no pair 0.3 m apart): unproven, near_rel = median(z/zl)
        let mut a = SelfTally::default();
        for k in 0..25 {
            let z = 3.45 + (k % 5) as f32 * 0.03;
            a.add(near / (z - 0.34), z, 0.3);
        }
        let o = a.result(near, 5000.0);
        assert!(self_pass(&o), "{o:?}");
        assert_eq!((o.scale, o.lev_lo, o.lev_hi), (Scale::Unproven, 0, 0));
        assert!((o.near_rel - 3.51 / 3.17).abs() < 0.005 && o.o_m.abs() < 0.01, "{o:?}");
        // the safe-side near pushes ER's surface back to (at least) the bone: a gun in front of the wall is never hidden
        let d_surf = near / 3.17;
        assert!(lin(d_surf, near * o.near_rel, None).unwrap() >= 3.5, "{o:?}");
        // (b) a 6 % scale error over a 1.5 m span with an offset of 0.15 m: the line fits both, proven, the near carries
        // the 6 % (round 7 called this `bad`)
        let mut b = SelfTally::default();
        for k in 0..20 {
            let z = 2.5 + k as f32 * 0.08;
            b.add(near * 1.06 / (z - 0.15), z, 0.3);
        }
        let ob = b.result(near, 5000.0);
        assert_eq!((ob.scale, self_why(&ob)), (Scale::Proven, None), "{ob:?}");
        assert!((ob.s - 1.06).abs() < 0.002 && (ob.o_m - 0.15).abs() < 0.005 && (ob.near_rel - 1.06).abs() < 0.002, "{ob:?}");
        assert!(ob.lev_lo >= 3 && ob.lev_hi >= 3 && ob.bias_add_m == 0.0);
        // (b2) the trip rule: the proven slope's low end against the LIVE near ratio (> 5 % = phantom occlusion)
        let mut t2 = SelfTally::default();
        for k in 0..20 {
            let z = 2.0 + k as f32 * 0.12;
            t2.add(near * 1.12 / z, z, 0.3);
        }
        let o2 = t2.result(near, 5000.0);
        assert_eq!(o2.scale, Scale::Proven, "{o2:?}");
        assert!(self_trips(&o2, 1.0), "live on near_cam, the proof says 1.12: trip");
        assert!(!self_trips(&o2, 1.104), "live already on the bone bound 1.104: within 5 %, no trip");
        assert!(!self_trips(&o, 1.0) && !self_trips(&ob, 1.06), "{o:?}");
        // an unproven fit never trips on the scale (no leverage)
        let mut c1 = SelfTally::default();
        for k in 0..12 {
            let z = 3.4 + (k % 4) as f32 * 0.05;
            c1.add(near * 1.3 / z, z, 0.3);
        }
        let oc1 = c1.result(near, 5000.0);
        assert_eq!((oc1.scale, self_why(&oc1)), (Scale::Bad, Some("scale")), "{oc1:?}");
        assert!(!self_trips(&oc1, 1.0), "a scale failure without leverage only fails the check");
        // (c) the surface BEHIND the bone at one range (s = 0.94): out of the gate, fails, near stays near_cam
        let mut c = SelfTally::default();
        for k in 0..12 {
            let z = 3.0 + (k % 3) as f32 * 0.02;
            c.add(near / (z + 0.2), z, 0.3);
        }
        let oc = c.result(near, 5000.0);
        assert_eq!((self_why(&oc), oc.near_rel), (Some("scale"), 1.0), "{oc:?}");
        // (c2) an offset outside the gate with leverage (0.7 m): fails; trips with >= 12 judged, not with 6
        let mut co = SelfTally::default();
        for k in 0..12 {
            let z = 2.2 + k as f32 * 0.2;
            co.add(near / (z - 0.7), z, 0.3);
        }
        let oco = co.result(near, 5000.0);
        assert_eq!(self_why(&oco), Some("offset"), "{oco:?}");
        assert!(self_trips(&oco, 1.0), "12 judged offset failures trip");
        let mut c6 = SelfTally::default();
        c6.s = co.s[..SELF_MIN_JUDGED].to_vec();
        assert!(!self_trips(&c6.result(near, 5000.0), 1.0), "a few offset samples only fail the check, no trip");
        // (d) 3 outliers in 25 are edges (one cross texel off the torso): judged correctly, still passing
        let mut dd = a.clone();
        dd.s.truncate(22);
        for z in [3.5f32, 3.52, 3.47] {
            let on = near / (z - 0.34);
            dd.add5([on, on, near / 30.0, on, on], z, 0.3, 0.0);
        }
        let od = dd.result(near, 5000.0);
        assert_eq!((od.edges, od.judged, od.ok), (3, 22, 22), "{od:?}");
        assert!(self_pass(&od), "{od:?}");
        // ... the same 3 hitting something NEAR with all five texels (no edge): judged, the check fails
        let mut dn = a.clone();
        dn.s.truncate(22);
        for z in [3.5f32, 3.52, 3.47] {
            dn.add(near / 1.9, z, 0.3);
        }
        assert!(!self_pass(&dn.result(near, 5000.0)), "{:?}", dn.result(near, 5000.0));
        // ... while 3 surfaces BEHIND the line (a gap: the 23:42:39 k16) are harmless: counted far, still passing
        let mut df = a.clone();
        df.s.truncate(22);
        for z in [3.5f32, 3.52, 3.47] {
            df.add(near / (z + 0.5), z, 0.3);
        }
        let odf = df.result(near, 5000.0);
        assert!(self_pass(&odf) && odf.far_n == 3, "{odf:?}");
        // (e) more than 25 % edges: fails (edges cannot hide a failure)
        let mut e = a.clone();
        e.s.truncate(15);
        for _ in 0..6 {
            e.add5([near / 3.2, near / 3.2, 0.0, near / 3.2, near / 3.2], 3.5, 0.3, 0.0);
        }
        let oe = e.result(near, 5000.0);
        assert_eq!((oe.edges, self_why(&oe)), (6, Some("edge")), "{oe:?}");
        // (f) s = 0.90 (CSCam's near too large) over a span: out of the gate, fails, the near never goes below near_cam
        let mut f = SelfTally::default();
        for k in 0..20 {
            let z = 2.0 + k as f32 * 0.12;
            f.add(near * 0.9 / z, z, 0.3);
        }
        let of = f.result(near, 5000.0);
        assert_eq!((of.scale, self_why(&of), of.near_rel), (Scale::Bad, Some("scale"), 1.0), "{of:?}");
        // (g) one far sample + a cluster: never proven (the 23:41:39 `scale=proven slope=1.026`)
        let mut g = SelfTally::default();
        for k in 0..10 {
            let z = 3.4 + (k % 4) as f32 * 0.1;
            g.add(near * 1.1 / z, z, 0.3);
        }
        g.add(near * 1.1 / 4.55, 4.55, 0.3);
        let og = g.result(near, 5000.0);
        assert_eq!((og.scale, og.lev_hi), (Scale::Unproven, 1), "{og:?}");
    }

    /// C6 part 16.8 (work/c6/depth8.md 2 b): the 23:39-23:43 log's 17 still `DEPTH probe self_s` samples verbatim (z, d,
    /// spread): the fit gives s ~ 1.10-1.13, unproven (leverage 2 / 2), n_eff = median(z * d) ~ 0.0553, and the check passes
    /// (the near-side residuals): live is allowed with near 0.0553 instead of tripping. The prime then linearises a wall
    /// 3.5 m away correctly instead of 0.35 m too near (a gun at 3.3 m stays visible).
    #[test]
    fn c6_depth_state_self_fit_round7_log() {
        const S: [(f32, f32, f32); 17] = [
            (3.575, 0.015465, 0.004),
            (3.582, 0.015468, 0.004),
            (3.559, 0.016081, 0.240),
            (3.458, 0.016503, 0.020),
            (3.470, 0.016011, 0.007),
            (3.678, 0.015191, 0.013),
            (4.547, 0.012168, 0.015),
            (3.700, 0.014802, 0.008),
            (2.818, 0.019222, 0.019),
            (3.625, 0.015287, 0.005),
            (2.004, 0.027269, 0.056),
            (3.704, 0.014812, 0.002),
            (3.616, 0.014806, 0.061),
            (3.570, 0.015054, 0.205),
            (3.569, 0.015061, 0.192),
            (3.618, 0.012195, 0.014),
            (3.499, 0.015858, 0.005),
        ];
        let near = 0.0501;
        let mut t = SelfTally::default();
        for (z, d, spread) in S {
            // the median texel `d`, one texel moved so the cross spreads by the logged amount (edges stay edges)
            let zl = near / d;
            let d_far = near / (zl + spread);
            t.add5([d, d, d, d, d_far], z, 0.0, 0.0);
        }
        let o = t.result(near, 10000.0);
        assert_eq!((o.still, o.edges, o.judged), (17, 3, 14), "{o:?}");
        // depth8.md: 1.098 over all 14 judged; the fit skips the gross outlier k16 (0.9 m behind the line): 1.126
        assert!((1.08..=1.13).contains(&o.s), "{o:?}");
        assert_eq!((o.scale, o.lev_lo, o.lev_hi), (Scale::Unproven, 2, 2), "{o:?}");
        let n_eff = near * o.near_rel;
        assert!((n_eff - 0.0553).abs() < 0.0003, "{n_eff} {o:?}");
        // the offset ~0 (depth8.md: +0.02 over all 14, -0.06 without k16; least squares -0.02..-0.06): a pure scale
        assert!(o.o_m.abs() < 0.08, "{o:?}");
        assert_eq!(self_why(&o), None, "{}", self_line(&o, near, 10000.0));
        let line = self_line(&o, near, 10000.0);
        assert!(line.contains(" scale=unproven s=1.1") && line.contains(" lev=2/2 ") && line.contains(" n_eff=0.055"), "{line}");
        assert!(!self_trips(&o, o.near_rel), "live on n_eff never trips on its own data");
        // the prime: a wall 3.5 m away (ER's depth with its effective near), our gun at 3.3 m in front of it
        let d_wall = 0.0553 / 3.5;
        let gun = our_ndc(3.3, OUR_NEAR, OUR_FAR);
        let with_cam = prime_ndc(d_wall, near, None, 0.03, 0.004).unwrap();
        let with_eff = prime_ndc(d_wall, n_eff, None, 0.03, 0.004).unwrap();
        assert!(with_cam > gun, "near_cam puts the wall in front of the gun (the 23:41 phantom occlusion)");
        assert!(with_eff < gun, "n_eff keeps the wall behind the gun");
    }

    /// Review 2026-10-21: the self check must not pick the finite model on chest-range noise (the true front offset
    /// 0.10 m, not CHEST_FRONT_M, with infinite reverse-Z data favours "finite" by millimetres); finite only on proof.
    #[test]
    fn c6_depth_state_self_far_model_default_infinite() {
        let near = 0.1;
        let far = 5000.0;
        let mut t = SelfTally::default();
        for k in 0..12 {
            let z = 2.0 + k as f32 * 0.25;
            t.add(near / (z - 0.10), z, 0.2);
        }
        let o = t.result(near, far);
        assert_eq!(o.far, None, "{o:?}");
        assert!(self_line(&o, near, far).contains(" model=inf "));
        assert!(self_pass(&o), "{o:?}");
        // a no-far CSCam value: infinite
        assert_eq!(t.result(near, f32::NAN).far, None);
        // finite reverse-Z data at long range (models differ by metres): finite is proven
        let lin_f = |z: f32| near * (far - z) / (z * (far - near));
        let mut tf = SelfTally::default();
        for k in 0..10 {
            let z = 60.0 + k as f32 * 10.0;
            tf.add(lin_f(z - CHEST_FRONT_M), z, 0.2);
        }
        assert_eq!(tf.result(near, far).far, Some(far));
        // the same long-range data but infinite: stays infinite
        let mut ti = SelfTally::default();
        for k in 0..10 {
            let z = 60.0 + k as f32 * 10.0;
            ti.add(near / (z - CHEST_FRONT_M), z, 0.2);
        }
        assert_eq!(ti.result(near, far).far, None);
        // too few long-range samples: infinite even on finite data
        let mut t5 = SelfTally::default();
        for k in 0..5 {
            let z = 60.0 + k as f32 * 10.0;
            t5.add(lin_f(z - CHEST_FRONT_M), z, 0.2);
        }
        assert_eq!(t5.result(near, far).far, None);
    }

    /// C6 round 6 (the 20:57 run: `verdict ... unknown=3476 queues=0`): the census keeps every depth-state resource
    /// (A idle, B with its UI pair, a third with 0x10 traffic), ignores non-depth transitions, counts a full table.
    #[test]
    fn c6_depth_state_census_rows_and_full() {
        let c = Census::new();
        const A: usize = 0x1792_ed6b_9a0;
        const B: usize = 0x1792_ed6a_680;
        const C: usize = 0x1792_0000_100;
        for _ in 0..3 {
            c.record(B, 1, 0xC0, 0x10, 0);
            c.record(B, 1, 0x10, 0xC0, 0);
        }
        c.record(C, SUB_ALL, 0xC0, 0x10, 0);
        c.record(C, SUB_ALL, 0x10, 0xC0, FLAG_BEGIN_ONLY);
        c.record(A, 0, 0x4, 0xC0, 0); // no depth bit: not depth traffic
        c.record(0x5555, 0, 0x4, 0x8, 0);
        let rows = c.rows();
        assert_eq!(rows.len(), 2, "A and the colour target never appear: {rows:?}");
        assert_eq!((rows[0].res, rows[0].n, rows[0].states, rows[0].last[1], rows[0].last[0]), (B, 6, 0xD0, (0x10, 0xC0), (UNKNOWN, UNKNOWN)));
        assert_eq!((rows[1].res, rows[1].n, rows[1].sub_all, rows[1].split, rows[1].last), (C, 2, 2, 1, [(0x10, 0xC0), (0x10, 0xC0)]));
        let line = census_line(&rows[0], "B", true);
        assert!(line.starts_with("census res=0x1792ed6a680 n=6 states=0xd0 p0=unknown>unknown p1=0x10>0xc0 sub_all=0 split=0 is=B vt_ok=1"), "{line}");
        // a full census counts and never blocks
        let f = Census::new();
        for k in 0..(CENSUS_N + 3) {
            f.record(0x1000 + k * 0x40, 0, 0xC0, 0x10, 0);
        }
        assert_eq!((f.rows().len(), f.full.load(Ordering::Relaxed)), (CENSUS_N, 3));
    }

    /// C6 round 6: B's own UI barrier pair is the positive control; the per-candidate verdict names a dead hook
    /// (`hook`), an idle candidate (`idle`) and otherwise the 15.1 rows; the fold counts the control per armed frame.
    #[test]
    fn c6_depth_state_control_and_cand_verdict() {
        // the fold of B: the UI list carries `1 0xC0 -> 0x10` + `1 0x10 -> 0xC0` every frame, folded after the armed check
        let t = Table::new();
        let f = Fold::new();
        let ui = |seq: u64| {
            t.record(L1, TID, RES, tr(1, 0xC0, 0x10));
            t.record(L1, TID, RES, tr(1, 0x10, 0xC0));
            f.ecl(&t, &ecl(&[L1], true, seq), &vt_ok);
        };
        for seq in 1..=10 {
            ui(seq);
        }
        let s = f.stats(&t);
        assert_eq!((s.armed, s.ctl_hits, s.transitions), (10, 9, 20), "the first armed frame has nothing before it");
        // the control rule
        assert_eq!(control_ok(&Ctl { calls: 100, frames: 59, hits: 59 }), None);
        assert_eq!(control_ok(&Ctl { calls: 100, frames: 100, hits: 90 }), Some(true));
        assert_eq!(control_ok(&Ctl { calls: 100, frames: 100, hits: 89 }), Some(false));
        assert_eq!(control_ok(&Ctl { calls: 0, frames: 100, hits: 100 }), Some(false));
        assert!(control_line(&Ctl { calls: 5, frames: 100, hits: 100 }, 9, 7, 0).starts_with("state control B ui=100/100 calls=5 bars=9 trans=7 census_full=0 ok=1"));
        // the 20:57 replay: A never transitioned, 3476 frames all unknown; with the control passing that is `idle`
        let a2057 = StateStats { frames: 3476, unknown: 3476, ..StateStats::default() };
        let pass = Ctl { calls: 40_000, frames: 3476, hits: 3476 };
        assert_eq!(verdict(&a2057), (false, "unknown"), "the old verdict");
        assert_eq!(verdict_cand(&a2057, &pass), (false, "idle"));
        // a dead hook: no call at all, or B's UI pair missing
        assert_eq!(verdict_cand(&a2057, &Ctl { calls: 0, frames: 0, hits: 0 }), (false, "hook"));
        assert_eq!(verdict_cand(&a2057, &Ctl { calls: 9, frames: 3476, hits: 10 }), (false, "hook"));
        // B never watched (no control frames): not judged
        assert_eq!(verdict_cand(&a2057, &Ctl { calls: 9, frames: 0, hits: 0 }), (false, "control"));
        // a good candidate: the 15.1 rows decide
        let good = StateStats { frames: 700, readable: 700, h_c0: 700, lists: 1400, transitions: 2800, queues: 1, threads: 1, vt_eq: 1400, vt_n: 1400, ..StateStats::default() };
        assert_eq!(verdict_cand(&good, &pass), (true, "none"));
        assert_eq!(verdict_cand(&StateStats { write_at_ui: 3, ..good }, &pass), (false, "write_at_ui"));
        assert_eq!(verdict_cand(&StateStats { frames: 100, ..a2057 }, &pass), (false, "frames"), "too few frames: frames first");
        let line = cand_verdict_line(1, &good, &pass);
        assert!(line.starts_with("state verdict ready=1 frames=700 readable=700/700") && line.ends_with("why=none cand=B"), "{line}");
        // the sample / live candidate
        assert_eq!(pick_cand(None, [UNKNOWN, 0xC0], [true, true]), Some(1));
        assert_eq!(pick_cand(None, [0xC0, 0xC0], [true, true]), Some(0), "A first");
        assert_eq!(pick_cand(None, [0xC0, 0x10], [false, true]), None, "B in DEPTH_WRITE, A unwatched");
        assert_eq!(pick_cand(Some(1), [0xC0, 0x10], [true, true]), Some(1), "live keeps its candidate (the splice still checks the state)");
        assert_eq!(pick_cand(Some(1), [0xC0, 0xC0], [true, false]), None);
    }

    #[test]
    fn c6_depth_state_live_gate_and_trips() {
        let g = LiveIn {
            cfg_on: true,
            pinned: false,
            probe_state: true,
            probe_sample: true,
            hook_on: true,
            ui_hooks: true,
            verdict: Some(true),
            conv: Some(Conv::Reverse),
            self_ok: Some(true),
            sample_failed: false,
        };
        assert_eq!(live_step(&g), LiveStep::On);
        assert_eq!(STATE_PIN_2720, None, "no pin before a passing in-game run");
        for (m, want) in [
            (LiveIn { cfg_on: false, ..g }, LiveStep::Off("config")),
            (LiveIn { ui_hooks: false, ..g }, LiveStep::Off("ui_hooks")),
            (LiveIn { probe_state: false, ..g }, LiveStep::Off("unpinned")),
            (LiveIn { probe_sample: false, ..g }, LiveStep::Off("unpinned")),
            (LiveIn { hook_on: false, ..g }, LiveStep::Off("hook")),
            (LiveIn { sample_failed: true, ..g }, LiveStep::Off("sample")),
            (LiveIn { verdict: None, ..g }, LiveStep::Wait),
            (LiveIn { verdict: Some(false), ..g }, LiveStep::Off("probe_verdict")),
            (LiveIn { conv: Some(Conv::Forward), ..g }, LiveStep::Off("conv")),
            (LiveIn { conv: None, ..g }, LiveStep::Wait),
            (LiveIn { self_ok: Some(false), ..g }, LiveStep::Wait),
        ] {
            assert_eq!(live_step(&m), want, "{m:?}");
        }
        let s = good_stats();
        assert_eq!(trip_of(&s, &s), None);
        for (m, why) in [
            (StateStats { mismatch: 1, ..s }, "mismatch"),
            (StateStats { queues: 2, ..s }, "queue"),
            (StateStats { threads: 2, ..s }, "queue"),
            (StateStats { split: 1, ..s }, "split"),
            (StateStats { alias: 1, ..s }, "alias"),
            (StateStats { table_full: 1, ..s }, "table_full"),
            (StateStats { write_at_ui: 1, ..s }, "state"),
        ] {
            assert_eq!(trip_of(&s, &m), Some(why));
        }
    }
}
