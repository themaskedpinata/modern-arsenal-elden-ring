//! Who drives the player, and the save/restore of the gravity fields (docs/M5_DESIGN.md sections 1.3, 4.2-4.3,
//! 5.5-5.6 and 9).
//!
//! - [`MoveCtl`]: `Off` -> `Arming` (rearm_ticks clear ticks with ER reporting the player grounded) -> `Driving` ->
//!   `Suspended(reason)` / `Off(reason)`, with the handback reasons of section 9.1 in priority order.
//! - [`GravityOwner`]: saves `gravity_multiplier` / `gravity_disabled` BEFORE the controller writes them, restores
//!   them only on the same physics module and only while they still hold our values, defers out of the world and
//!   drops a stale save (another module address).
//! - [`classify_jump`]: a > 5 m position jump is a Havok rebase (chunk position continuous) or a warp.
//! - [`er_pushout`] / [`char_pushout`]: what ER's own proxy push-out and nearby characters may move.

use crate::vec3::V3;

pub use crate::move_assist::{Assist, AssistEvent, AssistIn, AssistParams, AssistWhy};

/// Handback reasons, highest priority first (section 9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    Panic,
    WorldLeft,
    Loading,
    ReadFail,
    WriteFail,
    NoData,
    Death,
    Cutscene,
    Torrent,
    Ladder,
    Throw,
    Talk,
    Interact,
    ErMove,
    /// Our ground ray missed for a while although ER kept reporting the player on the ground (lifts, moving
    /// platforms, collision our map filter or streaming misses): ER drives instead of a fall through what it stands on.
    GroundDisagree,
    /// Run 18: W/A/S/D held, no progress and ER kept pushing (or a probe found the player inside geometry): ER's own
    /// physics resolves the overlap for a moment (with backoff), then the controller resyncs.
    Unstick,
    /// C3: our controller stalled while a direction is held: ER's own movement drives for a moment (docs/C3_DESIGN.md
    /// 1.4), then the controller resyncs.
    Assist,
    Freefall,
    Menu,
    Ac130,
    /// M7: the nuke's player-death prep (ER drives until the respawn).
    Nuke,
    FpOnly,
    TpEr,
    F5,
    Config,
}

/// What a reason does to the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Off until the condition clears (user / session reasons).
    Off,
    /// ER drives until the cause is over; the controller re-arms by itself.
    Suspend,
}

impl Reason {
    /// Every reason in priority order (the index is the priority).
    pub const ALL: [Reason; 25] = [
        Reason::Panic,
        Reason::WorldLeft,
        Reason::Loading,
        Reason::ReadFail,
        Reason::WriteFail,
        Reason::NoData,
        Reason::Death,
        Reason::Cutscene,
        Reason::Torrent,
        Reason::Ladder,
        Reason::Throw,
        Reason::Talk,
        Reason::Interact,
        Reason::ErMove,
        Reason::GroundDisagree,
        Reason::Unstick,
        Reason::Assist,
        Reason::Freefall,
        Reason::Menu,
        Reason::Ac130,
        Reason::Nuke,
        Reason::FpOnly,
        Reason::TpEr,
        Reason::F5,
        Reason::Config,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Reason::Panic => "panic",
            Reason::WorldLeft => "world_left",
            Reason::Loading => "loading",
            Reason::ReadFail => "read_fail",
            Reason::WriteFail => "write_fail",
            Reason::NoData => "no_data",
            Reason::Death => "death",
            Reason::Cutscene => "cutscene",
            Reason::Torrent => "torrent",
            Reason::Ladder => "ladder",
            Reason::Throw => "throw",
            Reason::Talk => "talk",
            Reason::Interact => "interact",
            Reason::ErMove => "er_move",
            Reason::GroundDisagree => "ground_disagree",
            Reason::Unstick => "unstick",
            Reason::Assist => "assist",
            Reason::Freefall => "freefall",
            Reason::Menu => "menu",
            Reason::Ac130 => "ac130",
            Reason::Nuke => "nuke",
            Reason::FpOnly => "fp_only",
            Reason::TpEr => "tp_er",
            Reason::F5 => "f5",
            Reason::Config => "config",
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Reason::Panic
            | Reason::WorldLeft
            | Reason::ReadFail
            | Reason::WriteFail
            | Reason::NoData
            | Reason::FpOnly
            | Reason::TpEr
            | Reason::F5
            | Reason::Config => Kind::Off,
            Reason::Loading
            | Reason::Death
            | Reason::Cutscene
            | Reason::Torrent
            | Reason::Ladder
            | Reason::Throw
            | Reason::Talk
            | Reason::Interact
            | Reason::ErMove
            | Reason::GroundDisagree
            | Reason::Unstick
            | Reason::Assist
            | Reason::Freefall
            | Reason::Menu
            | Reason::Ac130
            | Reason::Nuke => Kind::Suspend,
        }
    }

    /// Only a menu keeps the stance; every other handback resets it to stand (section 1.3).
    pub fn keeps_stance(self) -> bool {
        // C3: an assist hand-back (a moment of ER movement) must not stand a crouched / prone player up
        matches!(self, Reason::Menu | Reason::Assist)
    }

    fn index(self) -> usize {
        Reason::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// The trigger set of one tick (one flag per reason).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Triggers {
    flags: [bool; 25],
}

impl Triggers {
    pub fn set(&mut self, r: Reason, on: bool) {
        if on {
            self.flags[r.index()] = true;
        }
    }

    pub fn with(mut self, r: Reason) -> Triggers {
        self.set(r, true);
        self
    }

    pub fn has(&self, r: Reason) -> bool {
        self.flags[r.index()]
    }

    /// The highest-priority reason set, if any.
    pub fn first(&self) -> Option<Reason> {
        Reason::ALL.iter().copied().find(|r| self.has(*r))
    }

    pub fn any(&self) -> bool {
        self.flags.iter().any(|f| *f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off(Option<Reason>),
    Arming(u32),
    Driving,
    Suspended(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtlEvent {
    /// The controller takes over this tick (resync, save + write gravity, `MOVE on`).
    On,
    /// The controller gives control back this tick (restores, `MOVE off|suspend reason=`).
    Release(Reason),
    /// Not arming because of this reason (logged once per change: `MOVE blocked reason=`).
    Blocked(Reason),
}

/// The write plan of a tick: on a trigger tick nothing but restores may be written (section 9.1 same-tick rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WritePlan {
    pub position: bool,
    pub proxy: bool,
    pub gravity: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveCtl {
    state: State,
    blocked: Option<Reason>,
}

impl Default for MoveCtl {
    fn default() -> Self {
        MoveCtl { state: State::Off(None), blocked: None }
    }
}

impl MoveCtl {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn driving(&self) -> bool {
        self.state == State::Driving
    }

    /// One tick. `er_grounded`: ER reports the player on the ground (arming needs it, section 9.1).
    pub fn step(&mut self, t: &Triggers, er_grounded: bool, rearm_ticks: u32) -> Option<CtlEvent> {
        match (self.state, t.first()) {
            (State::Driving, Some(r)) => {
                self.state = match r.kind() {
                    Kind::Off => State::Off(Some(r)),
                    Kind::Suspend => State::Suspended(r),
                };
                self.blocked = Some(r);
                Some(CtlEvent::Release(r))
            }
            (State::Driving, None) => None,
            (_, Some(r)) => {
                self.state = match r.kind() {
                    Kind::Off => State::Off(Some(r)),
                    Kind::Suspend => State::Suspended(r),
                };
                if self.blocked != Some(r) {
                    self.blocked = Some(r);
                    return Some(CtlEvent::Blocked(r));
                }
                None
            }
            (st, None) => {
                self.blocked = None;
                let n = match st {
                    State::Arming(n) => n,
                    _ => 0,
                };
                let n = if er_grounded { n + 1 } else { 0 };
                if n >= rearm_ticks.max(1) {
                    self.state = State::Driving;
                    Some(CtlEvent::On)
                } else {
                    self.state = State::Arming(n);
                    None
                }
            }
        }
    }

    /// The writes allowed this tick, given this tick's event.
    pub fn write_plan(&self, ev: Option<CtlEvent>) -> WritePlan {
        let drive = self.driving() && !matches!(ev, Some(CtlEvent::Release(_)));
        WritePlan { position: drive, proxy: drive, gravity: drive }
    }

    /// Task panic: off for the session (the caller restores gravity first).
    pub fn emergency_off(&mut self) -> bool {
        let was = self.driving();
        self.state = State::Off(Some(Reason::Panic));
        was
    }
}

// ---- gravity ownership ----------------------------------------------------------------------------------------

/// The values ER had before the controller wrote its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GravitySave {
    pub module: usize,
    pub mult: f32,
    pub disabled: u8,
}

/// Our values while driving.
pub const OUR_MULT: f32 = 0.0;
pub const OUR_DISABLED: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldPlan {
    /// Write `new` (the saved value) over `cur` (still ours).
    Write { cur: f32, new: f32 },
    /// ER changed the field since: left alone (`skipped=changed`).
    Skipped { cur: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReleasePlan {
    Nothing,
    /// Not in the world (or the fields unreadable): kept, retried on the next in-world tick.
    Deferred,
    /// Another module address now: dropped, never written through a stale pointer.
    Stale(GravitySave),
    Restore { save: GravitySave, mult: FieldPlan, disabled: FieldPlan },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GravityOwner {
    save: Option<GravitySave>,
    deferred: bool,
    pub reasserts: u32,
}

impl GravityOwner {
    pub fn saved(&self) -> Option<GravitySave> {
        self.save
    }

    pub fn deferred(&self) -> bool {
        self.deferred
    }

    /// Registers the save (BEFORE any write) and returns the values to write. A save that is still pending for the
    /// same module is kept (its values are ER's, the current ones may be ours).
    pub fn own(&mut self, module: usize, cur_mult: f32, cur_disabled: u8) -> (f32, u8) {
        match self.save {
            Some(s) if s.module == module => {}
            _ => self.save = Some(GravitySave { module, mult: cur_mult, disabled: cur_disabled }),
        }
        self.deferred = false;
        (OUR_MULT, OUR_DISABLED)
    }

    /// While driving: ER cleared `gravity_disabled` (TAE flag) -> write 1 again (counted).
    pub fn reassert(&mut self, module: usize, cur_disabled: u8) -> bool {
        if self.save.is_some_and(|s| s.module == module) && cur_disabled != OUR_DISABLED {
            self.reasserts += 1;
            return true;
        }
        false
    }

    fn plan(save: GravitySave, cur: (f32, u8)) -> ReleasePlan {
        let mult = if cur.0 == OUR_MULT { FieldPlan::Write { cur: cur.0, new: save.mult } } else { FieldPlan::Skipped { cur: cur.0 } };
        let disabled = if cur.1 == OUR_DISABLED {
            FieldPlan::Write { cur: cur.1 as f32, new: save.disabled as f32 }
        } else {
            FieldPlan::Skipped { cur: cur.1 as f32 }
        };
        ReleasePlan::Restore { save, mult, disabled }
    }

    /// Handback: restore on the same module if the fields still hold our values; defer when not in the world (or
    /// the fields are unreadable); drop a save for another module.
    pub fn release(&mut self, in_world: bool, module: usize, cur: Option<(f32, u8)>) -> ReleasePlan {
        let Some(save) = self.save else { return ReleasePlan::Nothing };
        if !in_world || module == 0 || cur.is_none() {
            self.deferred = true;
            return ReleasePlan::Deferred;
        }
        self.save = None;
        self.deferred = false;
        if module != save.module {
            return ReleasePlan::Stale(save);
        }
        Self::plan(save, cur.unwrap_or((save.mult, save.disabled)))
    }

    /// Every in-world tick while a restore is deferred.
    pub fn resolve_deferred(&mut self, module: usize, cur: Option<(f32, u8)>) -> ReleasePlan {
        if !self.deferred {
            return ReleasePlan::Nothing;
        }
        self.release(true, module, cur)
    }
}

// ---- warps ----------------------------------------------------------------------------------------------------

/// The existing re-base threshold (task.rs `REBASE_JUMP_M`).
pub const REBASE_JUMP_M: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    None,
    /// ER moved its Havok frame: translate, keep velocity/stance/sprint.
    Rebase,
    /// A teleport: resync, zero velocity, end sprint.
    Warp,
}

impl Jump {
    pub fn label(self) -> &'static str {
        match self {
            Jump::None => "none",
            Jump::Rebase => "rebase",
            Jump::Warp => "warp",
        }
    }
}

/// M8 P1 (docs/M8_DESIGN.md section 2, B5): the largest mismatch (m) between ER's position jump and the chunk jump
/// that still counts as one re-base (plus this tick's own motion).
pub const REBASE_TOL_M: f32 = 1.0;
/// A chunk jump longer than this is never a re-base (ER re-bases per ~32 m chunk; 64 m = two chunks).
pub const REBASE_MAX_M: f32 = 64.0;

/// The classification of a > 5 m position jump with the vectors behind it (for the `MOVE resync` line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpVerdict {
    pub jump: Jump,
    /// Which sign matched for a re-base: `+1` (e ~ c) or `-1` (e ~ -c); `None` otherwise. The convention is not
    /// settled offline [I]; run 18's `sign=` settles it.
    pub sign: Option<i8>,
    /// |e| planar (m), the old `d=` of the line.
    pub planar: f32,
    /// |c| (m), the old `chunk_d=` of the line (`NaN` = unreadable).
    pub chunk: f32,
}

/// M8 P1 (B5): run 17 logged 42 of 42 ER chunk re-bases as warps, because `ChrIns.chunk_position` is not
/// continuous across a re-base: it jumps by the same vector as the Havok position (`d ~ chunk_d`, 32 m + 8 m grid).
///
/// `e` = P_er - P_w_last (m, 3D), `c` = chunk_now - chunk_prev (m, 3D; `None` = unreadable), `expected` = our own
/// motion this tick (m), `loading_recent` = a load / world-change signal within the last second. A jump with a planar
/// part over [`REBASE_JUMP_M`] is a `Rebase` iff the chunk moved with it (either sign, |e -+ c| < 1 m + expected),
/// |c| <= [`REBASE_MAX_M`] and no load was seen; otherwise a `Warp` (the safe choice: resync).
pub fn classify_jump(e: V3, c: Option<V3>, expected: f32, loading_recent: bool) -> JumpVerdict {
    let planar = V3::new(e.x, 0.0, e.z).len();
    let chunk = c.map_or(f32::NAN, |c| c.len());
    let mut v = JumpVerdict { jump: Jump::None, sign: None, planar, chunk };
    if !(planar > REBASE_JUMP_M) || !e.is_finite() {
        return v;
    }
    v.jump = Jump::Warp;
    let Some(c) = c.filter(|c| c.is_finite()) else { return v };
    if loading_recent || c.len() > REBASE_MAX_M {
        return v;
    }
    let tol = REBASE_TOL_M + expected.max(0.0);
    let (plus, minus) = ((e - c).len(), (e + c).len());
    if plus < tol && plus <= minus {
        v.jump = Jump::Rebase;
        v.sign = Some(1);
    } else if minus < tol {
        v.jump = Jump::Rebase;
        v.sign = Some(-1);
    }
    v
}

// ---- push-out --------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ErDelta {
    Ignore,
    /// Add this (metres) to our position; remove the velocity component into it.
    Apply(V3),
    /// A knockback / grab: hand back (`er_move`).
    ErMove,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PushParams {
    pub max_m: f32,
    pub handback_m: f32,
    pub min_m: f32,
}

impl Default for PushParams {
    fn default() -> Self {
        PushParams { max_m: 0.30, handback_m: 0.30, min_m: 0.02 }
    }
}

/// ER's own proxy displacement `e` (P_er - P_w_last, metres, < 5 m): only the planar part opposite to our last move
/// direction counts (push-out by walls ER sees and our rays miss), clamped to `max_m`. A planar displacement larger
/// than `handback_m` that is not along our own motion is a knockback.
pub fn er_pushout(e: V3, last_dir: V3, p: &PushParams) -> ErDelta {
    let ep = V3::new(e.x, 0.0, e.z);
    let dir = V3::new(last_dir.x, 0.0, last_dir.z).normalize();
    let along = dir.map_or(0.0, |d| ep.dot(d));
    // ER's own run root motion (third person hybrid) points along our motion: not a knockback
    let foreign = match dir {
        Some(d) if along > 0.0 => ep - d * along,
        _ => ep,
    };
    if foreign.len() > p.handback_m {
        return ErDelta::ErMove;
    }
    match dir {
        Some(d) if -along > p.min_m => ErDelta::Apply(d * -((-along).min(p.max_m))),
        _ => ErDelta::Ignore,
    }
}

/// C3 PASS C (docs/C3_DESIGN.md 3.6, M5 5.6): with W/A/S/D reaching ER (`tp_anim` / `fp_anim = "hybrid"`) ER's own run
/// root motion moves the proxy along our motion every tick before our write puts it back. That expected part (along
/// `last_dir`, at most `max_along_m` = ER's run speed x dt) is removed from ER's displacement `e` before it is used as
/// evidence (the mantle's er_push abort, the er_move handback during a mantle); what remains is ER's own push.
pub fn rm_compensate(e: V3, last_dir: V3, max_along_m: f32) -> V3 {
    let Some(d) = V3::new(last_dir.x, 0.0, last_dir.z).normalize() else { return e };
    let along = V3::new(e.x, 0.0, e.z).dot(d);
    if along <= 0.0 || !(max_along_m > 0.0) {
        return e;
    }
    e - d * along.min(max_along_m)
}

/// Circle push-out (planar, metres) from nearby characters: each closer than `r_player + r_char` pushes away, the
/// total limited to `max_step` per tick.
pub fn char_pushout(p: V3, r_player: f32, chars: &[V3], r_char: f32, max_step: f32) -> V3 {
    let mut push = V3::ZERO;
    for c in chars {
        let d = V3::new(p.x - c.x, 0.0, p.z - c.z);
        let dist = d.len();
        let min = r_player + r_char;
        if dist < min
            && dist > 1e-3
            && (p.y - c.y).abs() < 2.0
        {
            push = push + d * ((min - dist) / dist);
        }
    }
    let l = push.len();
    if l > max_step { push * (max_step / l) } else { push }
}

/// Largest distance (m) between the position we write and ER's position read this tick (review M5 minor 4). A
/// rebase is translated and a warp resynced before the write, so a normal tick is within one tick of motion.
pub const POS_WRITE_MAX_M: f32 = 5.0;

/// The position write is allowed: finite and within `max_m` of ER's position (else resync, skip, hand back).
pub fn position_write_ok(new_m: V3, er_m: V3, max_m: f32) -> bool {
    new_m.is_finite() && er_m.is_finite() && (new_m - er_m).len() <= max_m
}

/// Removes the part of `v` that goes against the push direction `n` (unit, planar).
pub fn remove_into(v: V3, n: V3) -> V3 {
    let into = v.dot(n);
    if into < 0.0 { v - n * into } else { v }
}

/// M8 B3 (docs/M8_DESIGN.md 10.2): the interact handback is counted in the anti-flap guard only when nothing ER
/// confirms took over during its hold. Before, five E uses (ladders, talks, levers) in a minute muted E for the
/// session; now an E that became a ladder / talk / cutscene / throw / Torrent episode is never a flap.
#[derive(Clone, Debug, Default)]
pub struct InteractFlap {
    pub guard: crate::fp_logic::FlapGuard,
    pending: Option<f64>,
    /// Interact handbacks an ER-confirmed reason took over (not counted).
    pub confirmed: u32,
}

impl InteractFlap {
    pub fn muted(&self) -> bool {
        self.guard.muted()
    }

    /// The controller released for `interact` at `now` (counted only when the hold ends unconfirmed).
    pub fn released(&mut self, now: f64) {
        self.pending = Some(now);
    }

    /// One tick. `confirmed`: an ER-confirmed reason is active now; `hold_active`: the interact hold still runs.
    /// Returns true exactly when the guard becomes muted.
    pub fn tick(&mut self, now: f64, confirmed: bool, hold_active: bool) -> bool {
        let Some(t0) = self.pending else { return false };
        if confirmed {
            self.pending = None;
            self.confirmed += 1;
            return false;
        }
        if hold_active {
            return false;
        }
        self.pending = None;
        let _ = now;
        self.guard.record(t0)
    }

    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// The reasons ER itself confirms (a state of its own), which turn an E press into a real episode (B3).
pub fn er_confirmed(t: &Triggers) -> bool {
    [Reason::Ladder, Reason::Talk, Reason::Cutscene, Reason::Throw, Reason::Torrent].iter().any(|r| t.has(*r))
}

/// A hold window (interact / er_move): triggered, extended while ER keeps moving the player, then `hold_s`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hold {
    until: f64,
}

impl Hold {
    pub fn trigger(&mut self, now: f64, hold_s: f64) {
        self.until = self.until.max(now + hold_s);
    }

    /// Active now; `er_moving` (ER displaced the player > 0.05 m this tick) extends it by `hold_s`.
    pub fn active(&mut self, now: f64, er_moving: bool, hold_s: f64) -> bool {
        if now < self.until && er_moving {
            self.until = now + hold_s;
        }
        now < self.until
    }

    pub fn clear(&mut self) {
        self.until = 0.0;
    }
}

// ---- run 18: unstick ---------------------------------------------------------------------------------------------

/// `[movement.unstick]` in the detector's units (metres, seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnstickParams {
    pub enabled: bool,
    pub window_s: f64,
    pub progress_m: f32,
    pub wish_frac: f32,
    pub evidence_frac: f32,
    pub hold_s: f64,
    pub max_hold_s: f64,
    pub max_per_window: u32,
    pub backoff_window_s: f64,
}

impl Default for UnstickParams {
    fn default() -> Self {
        UnstickParams {
            enabled: true,
            window_s: 2.5,
            progress_m: 0.5,
            wish_frac: 0.8,
            evidence_frac: 0.1,
            hold_s: 1.0,
            max_hold_s: 8.0,
            max_per_window: 4,
            backoff_window_s: 60.0,
        }
    }
}

/// One driving tick for the detector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UnstickIn {
    pub now: f64,
    /// The feet (metres).
    pub pos: V3,
    /// W/A/S/D held (the player wants to move).
    pub wish: bool,
    /// ER's own push-out was applied this tick (ER's proxy is inside something our rays did not stop at).
    pub pushed: bool,
    /// A probe found the capsule inside geometry (deeper than the skin), or a mantle start was backed off a face.
    pub inside: bool,
    /// A mantle runs (the detector rests and its window restarts).
    pub mantling: bool,
}

/// The detector fired: hand back for `hold_s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnstickFire {
    pub hold_s: f64,
    /// This is the n-th unstick within the backoff window (1 = the first).
    pub n: u32,
    pub window_s: f64,
    pub moved_m: f32,
    pub ticks: u32,
    pub wish_ticks: u32,
    pub pushed_ticks: u32,
    pub inside_ticks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnstickEvent {
    Fire(UnstickFire),
    /// `max_per_window` unsticks already happened within the backoff window: the detector rests until the oldest
    /// leaves it (logged once per rest; the controller keeps driving).
    Capped { n: u32, until: f64 },
}

/// Run 18 (stuck ~1 min inside a wagon, crawling at 0.5 m/s while ER pushed the proxy out on ~15 % of the ticks):
/// W/A/S/D held for `wish_frac` of `window_s`, the feet within `progress_m` of where the window started, and
/// penetration evidence (ER's push-out applied or a probe inside geometry) on `evidence_frac` of its ticks -> fire.
/// Pressing into an ordinary wall gives no evidence (our capsule stops at r + skin; ER does not push), so it never
/// fires. Each fire within `backoff_window_s` doubles the hold (up to `max_hold_s`); after `max_per_window` the detector
/// rests until the window frees (never an endless handback loop).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unstick {
    anchor: Option<(f64, V3)>,
    ticks: u32,
    wish_ticks: u32,
    pushed_ticks: u32,
    inside_ticks: u32,
    evidence_ticks: u32,
    fires: Vec<f64>,
    capped_until: f64,
}

impl Unstick {
    /// Restart the observation window (a handback, a resync, a warp).
    pub fn reset_window(&mut self) {
        self.anchor = None;
        self.ticks = 0;
        self.wish_ticks = 0;
        self.pushed_ticks = 0;
        self.inside_ticks = 0;
        self.evidence_ticks = 0;
    }

    /// Unsticks within the backoff window ending at `now`.
    pub fn recent(&self, now: f64, p: &UnstickParams) -> u32 {
        self.fires.iter().filter(|t| now - **t < p.backoff_window_s).count() as u32
    }

    pub fn tick(&mut self, i: &UnstickIn, p: &UnstickParams) -> Option<UnstickEvent> {
        if !p.enabled || i.mantling || !i.pos.is_finite() {
            self.reset_window();
            return None;
        }
        self.fires.retain(|t| i.now - *t < p.backoff_window_s);
        let Some((t0, a)) = self.anchor else {
            self.reset_window();
            self.anchor = Some((i.now, i.pos));
            return None;
        };
        let moved = (i.pos - a).len();
        if moved > p.progress_m {
            // progress: a new window from here
            self.reset_window();
            self.anchor = Some((i.now, i.pos));
            return None;
        }
        self.ticks += 1;
        self.wish_ticks += i.wish as u32;
        self.pushed_ticks += i.pushed as u32;
        self.inside_ticks += i.inside as u32;
        self.evidence_ticks += (i.pushed || i.inside) as u32;
        if i.now - t0 < p.window_s || self.ticks == 0 {
            return None;
        }
        let n = self.ticks as f32;
        let wish_ok = self.wish_ticks as f32 >= p.wish_frac * n;
        let ev_ok = self.evidence_ticks as f32 >= p.evidence_frac * n;
        let fire = UnstickFire {
            hold_s: 0.0,
            n: 0,
            window_s: i.now - t0,
            moved_m: moved,
            ticks: self.ticks,
            wish_ticks: self.wish_ticks,
            pushed_ticks: self.pushed_ticks,
            inside_ticks: self.inside_ticks,
        };
        // the window is judged once: a new one starts either way
        self.reset_window();
        self.anchor = Some((i.now, i.pos));
        if !(wish_ok && ev_ok) {
            return None;
        }
        let k = self.recent(i.now, p);
        if k >= p.max_per_window.max(1) {
            let until = self.fires.iter().copied().fold(f64::INFINITY, f64::min) + p.backoff_window_s;
            if i.now >= self.capped_until {
                self.capped_until = until;
                return Some(UnstickEvent::Capped { n: k, until });
            }
            return None;
        }
        self.fires.push(i.now);
        let hold = (p.hold_s * 2f64.powi(k as i32)).min(p.max_hold_s.max(p.hold_s));
        Some(UnstickEvent::Fire(UnstickFire { hold_s: hold, n: k + 1, ..fire }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_logic_arms_after_rearm_ticks_grounded_only() {
        let mut c = MoveCtl::default();
        let clear = Triggers::default();
        for _ in 0..9 {
            assert_eq!(c.step(&clear, true, 10), None);
        }
        assert_eq!(c.step(&clear, true, 10), Some(CtlEvent::On));
        assert!(c.driving());
        // not while ER reports the player airborne
        let mut c = MoveCtl::default();
        for _ in 0..30 {
            assert_eq!(c.step(&clear, false, 10), None);
        }
        for _ in 0..5 {
            c.step(&clear, true, 10);
        }
        c.step(&clear, false, 10);
        assert_eq!(c.state(), State::Arming(0), "airborne resets the count");
    }

    #[test]
    fn move_logic_every_reason_returns_itself_and_kind() {
        for r in Reason::ALL {
            let mut c = MoveCtl::default();
            for _ in 0..10 {
                c.step(&Triggers::default(), true, 10);
            }
            assert!(c.driving());
            assert_eq!(c.step(&Triggers::default().with(r), true, 10), Some(CtlEvent::Release(r)), "{r:?}");
            match r.kind() {
                Kind::Off => assert_eq!(c.state(), State::Off(Some(r))),
                Kind::Suspend => assert_eq!(c.state(), State::Suspended(r)),
            }
            assert!(!r.label().is_empty());
            assert_eq!(r.keeps_stance(), r == Reason::Menu || r == Reason::Assist, "stance kept only across a menu (and, C3, an assist)");
        }
        // C3: + Assist (after Unstick)
        assert_eq!(Reason::ALL.len(), 25);
        let at = |r: Reason| Reason::ALL.iter().position(|x| *x == r).unwrap();
        assert_eq!(at(Reason::Assist), at(Reason::Unstick) + 1);
        assert_eq!((Reason::Assist.label(), Reason::Assist.kind()), ("assist", Kind::Suspend));
    }

    #[test]
    fn move_logic_priority_and_blocked_once() {
        let t = Triggers::default().with(Reason::Menu).with(Reason::Death).with(Reason::F5);
        assert_eq!(t.first(), Some(Reason::Death));
        let t = Triggers::default().with(Reason::Config).with(Reason::Panic);
        assert_eq!(t.first(), Some(Reason::Panic));
        let mut c = MoveCtl::default();
        let t = Triggers::default().with(Reason::Ladder);
        assert_eq!(c.step(&t, true, 10), Some(CtlEvent::Blocked(Reason::Ladder)));
        assert_eq!(c.step(&t, true, 10), None, "once per change");
        assert_eq!(c.step(&Triggers::default().with(Reason::Torrent), true, 10), Some(CtlEvent::Blocked(Reason::Torrent)));
        // the order follows section 9.1
        let labels: Vec<&str> = Reason::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(labels[..4], ["panic", "world_left", "loading", "read_fail"]);
        assert_eq!(labels[labels.len() - 2..], ["f5", "config"]);
    }

    #[test]
    fn move_logic_no_write_on_trigger_tick() {
        let mut c = MoveCtl::default();
        for _ in 0..10 {
            c.step(&Triggers::default(), true, 10);
        }
        let ev = c.step(&Triggers::default(), true, 10);
        assert_eq!(c.write_plan(ev), WritePlan { position: true, proxy: true, gravity: true });
        for r in Reason::ALL {
            let mut c2 = c;
            let ev = c2.step(&Triggers::default().with(r), true, 10);
            assert_eq!(c2.write_plan(ev), WritePlan { position: false, proxy: false, gravity: false }, "{r:?}");
        }
        // arming ticks write nothing either
        let mut c3 = MoveCtl::default();
        let ev = c3.step(&Triggers::default(), true, 10);
        assert!(!c3.write_plan(ev).position);
    }

    #[test]
    fn move_logic_stance_reset_per_reason() {
        let kept: Vec<Reason> = Reason::ALL.iter().copied().filter(|r| r.keeps_stance()).collect();
        assert_eq!(kept, vec![Reason::Assist, Reason::Menu]);
    }

    #[test]
    fn move_logic_emergency_off() {
        let mut c = MoveCtl::default();
        for _ in 0..10 {
            c.step(&Triggers::default(), true, 10);
        }
        assert!(c.emergency_off());
        assert_eq!(c.state(), State::Off(Some(Reason::Panic)));
    }

    #[test]
    fn move_logic_gravity_round_trip_every_reason() {
        for r in Reason::ALL {
            let mut g = GravityOwner::default();
            let (m, d) = g.own(0x1000, 1.0, 0);
            assert_eq!((m, d), (0.0, 1));
            assert_eq!(g.saved(), Some(GravitySave { module: 0x1000, mult: 1.0, disabled: 0 }), "save before the write");
            // the fields hold ours; the handback restores exactly the saved values
            let in_world = !matches!(r, Reason::WorldLeft | Reason::Loading);
            let plan = g.release(in_world, 0x1000, Some((0.0, 1)));
            if in_world {
                assert_eq!(
                    plan,
                    ReleasePlan::Restore {
                        save: GravitySave { module: 0x1000, mult: 1.0, disabled: 0 },
                        mult: FieldPlan::Write { cur: 0.0, new: 1.0 },
                        disabled: FieldPlan::Write { cur: 1.0, new: 0.0 },
                    },
                    "{r:?}"
                );
                assert_eq!(g.saved(), None);
            } else {
                assert_eq!(plan, ReleasePlan::Deferred);
                let back = g.resolve_deferred(0x1000, Some((0.0, 1)));
                assert!(matches!(back, ReleasePlan::Restore { mult: FieldPlan::Write { new, .. }, .. } if new == 1.0));
            }
        }
    }

    #[test]
    fn move_logic_gravity_skipped_when_changed_and_stale() {
        let mut g = GravityOwner::default();
        g.own(0x1000, 1.0, 0);
        let plan = g.release(true, 0x1000, Some((0.5, 1)));
        assert!(matches!(plan, ReleasePlan::Restore { mult: FieldPlan::Skipped { cur }, disabled: FieldPlan::Write { .. }, .. } if cur == 0.5));
        // deferred out of world, another module on return: dropped (never written)
        let mut g = GravityOwner::default();
        g.own(0x1000, 1.0, 0);
        assert_eq!(g.release(false, 0, None), ReleasePlan::Deferred);
        assert!(g.deferred());
        assert_eq!(g.resolve_deferred(0x2000, Some((0.0, 1))), ReleasePlan::Stale(GravitySave { module: 0x1000, mult: 1.0, disabled: 0 }));
        assert_eq!(g.saved(), None);
        assert_eq!(g.resolve_deferred(0x2000, Some((0.0, 1))), ReleasePlan::Nothing);
        // a second own() on the same module keeps ER's values
        let mut g = GravityOwner::default();
        g.own(0x1000, 1.0, 0);
        g.own(0x1000, 0.0, 1);
        assert_eq!(g.saved().unwrap().mult, 1.0);
    }

    #[test]
    fn move_logic_gravity_reassert_counting_and_panic_path() {
        let mut g = GravityOwner::default();
        assert!(!g.reassert(0x1000, 0), "nothing owned");
        g.own(0x1000, 1.0, 0);
        assert!(!g.reassert(0x1000, 1));
        assert!(g.reassert(0x1000, 0));
        assert!(g.reassert(0x1000, 0));
        assert_eq!(g.reasserts, 2);
        assert!(!g.reassert(0x2000, 0), "other module");
        // the panic path restores like any handback
        let mut c = MoveCtl::default();
        for _ in 0..10 {
            c.step(&Triggers::default(), true, 10);
        }
        c.emergency_off();
        assert!(matches!(g.release(true, 0x1000, Some((0.0, 1))), ReleasePlan::Restore { .. }));
    }

    #[test]
    fn move_logic_resync_rebase_warp() {
        let j = |e: V3, c: Option<V3>| classify_jump(e, c, 0.1, false).jump;
        assert_eq!(j(V3::new(4.9, 0.0, 0.0), Some(V3::new(4.9, 0.0, 0.0))), Jump::None);
        assert_eq!(j(V3::new(50.0, 0.0, 0.0), Some(V3::new(50.0, 0.0, 0.2))), Jump::Rebase, "chunk moved with it");
        assert_eq!(j(V3::new(50.0, 0.0, 0.0), Some(V3::new(0.0, 0.0, 48.0))), Jump::Warp, "chunk moved elsewhere");
        assert_eq!(j(V3::new(50.0, 0.0, 0.0), None), Jump::Warp, "unknown chunk: the safe choice");
        assert_eq!(j(V3::new(f32::NAN, 0.0, 0.0), Some(V3::ZERO)), Jump::None);
    }

    /// The 42 run-17 re-bases (`MOVE resync why=warp d=.. chunk_d=..`, logs\mw2er.log 2026-10-18), rebuilt as vectors
    /// on the logged grid: planar (32, 8 k) m, plus 8 m vertical when chunk_d > d.
    const RUN17_PAIRS: [(f32, f32); 42] = [
        (32.98, 32.98), (32.98, 32.98), (32.00, 32.00), (35.78, 35.78), (32.98, 32.98), (32.98, 32.98), (32.00, 32.98),
        (32.98, 32.98), (35.78, 36.66), (32.98, 33.94), (40.00, 40.79), (35.71, 35.78), (32.98, 32.98), (31.95, 32.00),
        (35.78, 35.78), (32.00, 32.00), (32.00, 32.98), (32.00, 32.00), (35.78, 36.66), (32.98, 32.98), (32.00, 32.00),
        (32.00, 32.00), (35.78, 35.78), (32.00, 32.00), (32.00, 32.00), (32.98, 32.98), (35.78, 35.78), (35.78, 35.78),
        (32.00, 32.00), (35.78, 35.78), (32.98, 32.98), (32.98, 32.98), (35.78, 35.78), (32.98, 32.98), (35.78, 35.78),
        (35.78, 35.78), (32.98, 32.98), (32.00, 32.00), (32.99, 33.94), (40.00, 40.79), (35.78, 36.66), (35.78, 35.78),
    ];

    fn grid_vector(planar: f32, total: f32) -> V3 {
        // planar = |(32, 8k)| on the 8 m grid; total adds the vertical part
        let side = (planar * planar - 32.0 * 32.0).max(0.0).sqrt();
        let side = (side / 8.0).round() * 8.0;
        let vy = (total * total - planar * planar).max(0.0).sqrt();
        V3::new(32.0, (vy / 8.0).round() * 8.0, side)
    }

    #[test]
    fn rebase_run17_pairs_are_rebases_either_sign() {
        let mut n = 0;
        for (d, cd) in RUN17_PAIRS {
            let c = grid_vector(d, cd);
            assert!((c.len() - cd).abs() < 0.6, "grid {d}/{cd}: {c:?} len {}", c.len());
            // e: the chunk vector plus up to a sprint tick of own motion (0.12 m)
            let own = V3::new(0.08, 0.0, -0.09);
            for sign in [1.0f32, -1.0] {
                let e = c * sign + own;
                let v = classify_jump(e, Some(c), 0.12, false);
                assert_eq!(v.jump, Jump::Rebase, "{d}/{cd} sign {sign}: {v:?}");
                assert_eq!(v.sign, Some(sign as i8));
                assert!(v.planar > REBASE_JUMP_M);
            }
            n += 1;
        }
        assert_eq!(n, 42);
    }

    // ---- M8 ladder audit (docs/M8_DESIGN.md 10 / 12.1 `ladder_`) ----

    fn driving() -> MoveCtl {
        let mut c = MoveCtl::default();
        for _ in 0..10 {
            c.step(&Triggers::default(), true, 10);
        }
        assert!(c.driving());
        c
    }

    #[test]
    fn ladder_movectl_e_then_ladder_then_rearm() {
        let mut c = driving();
        // E: the interact hold hands back at once (ER climbs in third person)
        assert_eq!(c.step(&Triggers::default().with(Reason::Interact), true, 10), Some(CtlEvent::Release(Reason::Interact)));
        // the ladder state shows: blocked (logged once), whatever the hold does
        let lad = Triggers::default().with(Reason::Ladder).with(Reason::Interact);
        assert_eq!(c.step(&lad, false, 10), Some(CtlEvent::Blocked(Reason::Ladder)));
        for _ in 0..100 {
            assert_eq!(c.step(&Triggers::default().with(Reason::Ladder), false, 10), None);
        }
        // off the ladder: 20 airborne clear ticks keep it arming, 10 grounded ones re-arm
        for _ in 0..20 {
            assert_eq!(c.step(&Triggers::default(), false, 10), None);
            assert!(matches!(c.state(), State::Arming(0)));
        }
        for k in 0..10 {
            let ev = c.step(&Triggers::default(), true, 10);
            assert_eq!(ev, if k == 9 { Some(CtlEvent::On) } else { None });
        }
        assert!(c.driving());
        // the ladder on the very E tick: the ladder reason wins (priority)
        let mut c = driving();
        assert_eq!(c.step(&Triggers::default().with(Reason::Interact).with(Reason::Ladder), true, 10), Some(CtlEvent::Release(Reason::Ladder)));
        // a failing first-person read (read_fail fp:<field>) is a handback too, and re-arms once it clears
        let mut c = driving();
        assert_eq!(c.step(&Triggers::default().with(Reason::ReadFail), true, 10), Some(CtlEvent::Release(Reason::ReadFail)));
        for _ in 0..9 {
            assert_eq!(c.step(&Triggers::default(), true, 10), None);
        }
        assert_eq!(c.step(&Triggers::default(), true, 10), Some(CtlEvent::On));
    }

    #[test]
    fn ladder_b3_flap_guard_ignores_confirmed_episodes() {
        let mut f = InteractFlap::default();
        // ten E presses in a minute, each one a ladder trip: never muted
        for k in 0..10 {
            let t = k as f64 * 5.0;
            f.released(t);
            assert!(!f.tick(t + 0.1, false, true), "hold still running");
            assert!(!f.tick(t + 0.4, true, true), "the ladder confirms it");
            assert!(!f.pending());
        }
        assert!(!f.muted());
        assert_eq!(f.confirmed, 10);
        // five unconfirmed E presses (nothing ER confirms) in a minute: muted on the fifth, exactly once
        let mut g = InteractFlap::default();
        let mut mutes = 0;
        for k in 0..6 {
            let t = 100.0 + k as f64 * 3.0;
            g.released(t);
            g.tick(t + 0.5, false, true);
            mutes += g.tick(t + 2.0, false, false) as u32;
        }
        assert!(g.muted() && mutes == 1);
        let t = Triggers::default().with(Reason::Talk);
        assert!(er_confirmed(&t) && !er_confirmed(&Triggers::default().with(Reason::Interact)));
    }

    #[test]
    fn ladder_b4_interact_hold_cleared() {
        // the DLL clears the hold once the ladder shows: the re-arm then waits for the ladder only
        let mut h = Hold::default();
        h.trigger(10.0, 1.5);
        assert!(h.active(10.2, false, 1.5));
        h.clear();
        assert!(!h.active(10.3, true, 1.5), "cleared: ER moving the player on the ladder no longer extends it");
    }

    #[test]
    fn ladder_fp_machine_one_off_no_flicker_on_after_30() {
        use crate::fp_logic::{FpEvent, FpMachine, Reason as FpReason, Triggers as FpT, on_ladder};
        let mut m = FpMachine::default();
        for _ in 0..30 {
            m.step(&FpT::default(), 30);
        }
        assert!(m.is_on());
        let seq: Vec<i8> = [-1i8, 0, 2, 3, -1, 8, 6].into_iter().chain(std::iter::repeat_n(-1, 30)).collect();
        let mut offs = 0;
        let mut on_at = None;
        let mut blocked = 0;
        for (k, s) in seq.iter().enumerate() {
            match m.step(&FpT { ladder: on_ladder(*s), ..FpT::default() }, 30) {
                Some(FpEvent::Off(FpReason::Ladder)) => offs += 1,
                Some(FpEvent::On) => on_at = Some(k),
                Some(FpEvent::Blocked(_)) => blocked += 1,
                Some(e) => panic!("{e:?}"),
                None => {}
            }
        }
        assert_eq!(offs, 1, "exactly one Off(ladder)");
        assert_eq!(blocked, 0, "the 1-tick flicker to -1 does not re-log");
        assert_eq!(on_at, Some(6 + 30), "On at the 30th clear tick after the dismount");
    }

    #[test]
    fn ladder_gates_block_ac130_and_nuke() {
        use crate::ac130_logic::{Ac130Machine, Block as AcBlock, GateIn as AcGate, Params};
        use crate::nuke_logic::{Block as NkBlock, GateIn as NkGate, gate};
        let p = Params::default();
        let mut m = Ac130Machine::default();
        assert_eq!(m.start(0.0, &AcGate::failing(AcBlock::Ladder), &p), Err(AcBlock::Ladder));
        assert_eq!(AcBlock::Ladder.label(), "ladder");
        let ok = NkGate { enabled: true, data_ok: true, mode_on: true, available: true, ..NkGate::default() };
        assert_eq!(gate(&ok), Ok(()));
        assert_eq!(gate(&NkGate { ladder: true, ..ok }), Err(NkBlock::Ladder));
    }

    #[test]
    fn ladder_weapon_lowered_blocks_fire_ads_reload() {
        use crate::mantle::{GateEvent, WeaponGate};
        let mut g = WeaponGate::default();
        assert_eq!(g.update(1.0, false, true, 0.55), Some(GateEvent::Lowered("ladder")));
        for k in 0..100 {
            let t = 1.0 + k as f64 * 0.05;
            assert_eq!(g.update(t, false, true, 0.55), None);
            assert!(g.blocked(t) && g.lowered(), "fire / ADS / reload blocked on the ladder");
        }
        assert_eq!(g.update(7.0, false, false, 0.55), Some(GateEvent::RaiseStart("ladder")));
        assert!(g.blocked(7.3));
        assert_eq!(g.update(7.6, false, false, 0.55), Some(GateEvent::Raised("ladder")));
        assert!(!g.blocked(7.6));
    }

    #[test]
    fn rebase_teleports_are_warps() {
        // fast travel 300 m with a loading flag
        let e = V3::new(300.0, 4.0, -20.0);
        assert_eq!(classify_jump(e, Some(e), 0.1, true).jump, Jump::Warp, "loading seen");
        assert_eq!(classify_jump(e, Some(e), 0.1, false).jump, Jump::Warp, "longer than two chunks");
        // 40 m with a chunk jump that does not match
        let e = V3::new(40.0, 0.0, 0.0);
        assert_eq!(classify_jump(e, Some(V3::new(32.0, 0.0, 24.0)), 0.1, false).jump, Jump::Warp);
        assert_eq!(classify_jump(e, Some(V3::new(38.5, 0.0, 0.0)), 0.1, false).jump, Jump::Warp, "1.5 m off");
        assert_eq!(classify_jump(e, Some(V3::new(f32::NAN, 0.0, 0.0)), 0.1, false).jump, Jump::Warp);
        // the line's numbers
        let v = classify_jump(V3::new(32.0, 8.0, 0.0), Some(V3::new(32.0, 8.0, 0.0)), 0.0, false);
        assert!((v.planar - 32.0).abs() < 1e-4 && (v.chunk - 32.98).abs() < 0.01 && v.sign == Some(1));
    }

    #[test]
    fn move_logic_pushout_rules() {
        let p = PushParams::default();
        let dir = V3::new(0.0, 0.0, 1.0);
        // opposite to our motion: applied, clamped to 0.3
        assert_eq!(er_pushout(V3::new(0.0, 0.0, -0.1), dir, &p), ErDelta::Apply(V3::new(0.0, 0.0, -0.1)));
        match er_pushout(V3::new(0.0, 0.0, -0.29), dir, &p) {
            ErDelta::Apply(v) => assert!((v.z + 0.29).abs() < 1e-6),
            e => panic!("{e:?}"),
        }
        // same direction (ER root motion in third person): ignored, even large
        assert_eq!(er_pushout(V3::new(0.0, 0.0, 0.25), dir, &p), ErDelta::Ignore);
        assert_eq!(er_pushout(V3::new(0.0, 0.0, 0.8), dir, &p), ErDelta::Ignore);
        // tiny: ignored
        assert_eq!(er_pushout(V3::new(0.0, 0.0, -0.01), dir, &p), ErDelta::Ignore);
        // > 0.3 m sideways or backwards: knockback
        assert_eq!(er_pushout(V3::new(0.5, 0.0, 0.0), dir, &p), ErDelta::ErMove);
        assert_eq!(er_pushout(V3::new(0.0, 0.0, -0.6), dir, &p), ErDelta::ErMove);
        // vertical displacement is not push-out
        assert_eq!(er_pushout(V3::new(0.0, 2.0, 0.0), dir, &p), ErDelta::Ignore);
        // hold: 0.5 s after ER stops moving the player
        let mut h = Hold::default();
        h.trigger(10.0, 0.5);
        assert!(h.active(10.2, true, 0.5));
        assert!(h.active(10.6, false, 0.5));
        assert!(!h.active(10.71, false, 0.5));
    }

    #[test]
    fn move_logic_position_write_sanity() {
        let er = V3::new(100.0, 20.0, -50.0);
        assert!(position_write_ok(er + V3::new(0.1, 0.0, 0.0), er, POS_WRITE_MAX_M));
        assert!(position_write_ok(er + V3::new(0.0, -4.9, 0.0), er, POS_WRITE_MAX_M));
        assert!(!position_write_ok(er + V3::new(5.1, 0.0, 0.0), er, POS_WRITE_MAX_M), "too far");
        assert!(!position_write_ok(V3::new(f32::NAN, 0.0, 0.0), er, POS_WRITE_MAX_M), "NaN");
        assert!(!position_write_ok(V3::new(f32::INFINITY, 0.0, 0.0), er, POS_WRITE_MAX_M), "inf");
        assert!(!position_write_ok(er, V3::new(f32::NAN, 0.0, 0.0), POS_WRITE_MAX_M), "ER NaN");
    }

    #[test]
    fn move_logic_ground_disagree_is_a_suspend_that_resets_stance() {
        let r = Reason::GroundDisagree;
        assert_eq!(r.kind(), Kind::Suspend);
        assert!(!r.keeps_stance());
        assert_eq!(r.label(), "ground_disagree");
        // below the session / safety reasons, above the free fall and the user's toggles
        let t = Triggers::default().with(Reason::Freefall).with(r).with(Reason::F5);
        assert_eq!(t.first(), Some(r));
        let t = Triggers::default().with(Reason::Death).with(r);
        assert_eq!(t.first(), Some(Reason::Death));
    }

    #[test]
    fn move_logic_pushout_characters_limited() {
        let p = V3::new(0.0, 0.0, 0.0);
        let push = char_pushout(p, 0.4, &[V3::new(0.0, 0.0, 0.5)], 0.75, 0.2);
        assert!((push.len() - 0.2).abs() < 1e-6 && push.z < 0.0, "{push:?}");
        let small = char_pushout(p, 0.4, &[V3::new(0.0, 0.0, 1.1)], 0.75, 0.2);
        assert!((small.len() - 0.05).abs() < 1e-5);
        assert_eq!(char_pushout(p, 0.4, &[V3::new(0.0, 0.0, 3.0)], 0.75, 0.2), V3::ZERO);
        assert_eq!(char_pushout(p, 0.4, &[V3::new(0.0, 5.0, 0.5)], 0.75, 0.2), V3::ZERO, "another floor");
        let v = remove_into(V3::new(0.0, 0.0, 2.0), V3::new(0.0, 0.0, -1.0));
        assert_eq!(v, V3::ZERO);
    }

    fn us(now: f64, x: f32, wish: bool, pushed: bool, inside: bool) -> UnstickIn {
        UnstickIn { now, pos: V3::new(x, 8.2, 27.0), wish, pushed, inside, mantling: false }
    }

    #[test]
    fn unstick_fires_on_the_run18_pattern_and_backs_off() {
        let p = UnstickParams::default();
        let mut u = Unstick::default();
        // run 18: crawling into the wagon at 60 Hz, oscillating within 0.3 m, ER's push-out on ~15 % of the ticks
        let mut fired = Vec::new();
        let mut t = 0.0;
        for k in 0..(60 * 200) {
            t = k as f64 / 60.0;
            let x = 6.0 + 0.15 * ((k as f32) * 0.2).sin();
            if let Some(e) = u.tick(&us(t, x, true, k % 7 == 0, false), &p) {
                fired.push((t, e));
                if let UnstickEvent::Fire(f) = e {
                    // the handback: ER drives for the hold, then the controller re-arms (a new window)
                    u.reset_window();
                    t += f.hold_s;
                }
            }
        }
        let fires: Vec<UnstickFire> = fired.iter().filter_map(|(_, e)| if let UnstickEvent::Fire(f) = e { Some(*f) } else { None }).collect();
        assert!(!fires.is_empty() && fires[0].window_s >= p.window_s && fires[0].window_s < p.window_s + 0.05, "{fires:?}");
        assert_eq!(fires[0].hold_s, 1.0);
        // the holds double within the backoff window, capped at max_hold_s
        assert_eq!(fires.iter().take(4).map(|f| f.hold_s).collect::<Vec<_>>(), vec![1.0, 2.0, 4.0, 8.0]);
        assert_eq!(fires.iter().take(4).map(|f| f.n).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        // never more than max_per_window within any backoff window, and a Capped event once per rest
        for (i, f) in fired.iter().enumerate() {
            let n = fired.iter().filter(|(t2, e)| matches!(e, UnstickEvent::Fire(_)) && *t2 <= f.0 && f.0 - *t2 < p.backoff_window_s).count();
            assert!(n as u32 <= p.max_per_window, "event {i}: {n} fires within the window");
        }
        assert!(fired.iter().any(|(_, e)| matches!(e, UnstickEvent::Capped { n: 4, .. })), "{fired:?}");
        let _ = t;
    }

    #[test]
    fn unstick_never_fires_on_a_wall_progress_or_no_wish() {
        let p = UnstickParams::default();
        // pressing W into an ordinary wall for 30 s: no progress, but no evidence (no push-out, nothing inside)
        let mut u = Unstick::default();
        for k in 0..(60 * 30) {
            assert_eq!(u.tick(&us(k as f64 / 60.0, 3.0, true, false, false), &p), None);
        }
        // pushed but walking on (0.6 m/s): progress resets the window
        let mut u = Unstick::default();
        for k in 0..(60 * 30) {
            assert_eq!(u.tick(&us(k as f64 / 60.0, k as f32 * 0.01, true, true, false), &p), None);
        }
        // pushed but the keys released (ER pushing an idle player: a knockback is ErMove's job)
        let mut u = Unstick::default();
        for k in 0..(60 * 30) {
            assert_eq!(u.tick(&us(k as f64 / 60.0, 3.0, false, true, false), &p), None);
        }
        // a mantle rests the detector
        let mut u = Unstick::default();
        for k in 0..(60 * 30) {
            let mut i = us(k as f64 / 60.0, 3.0, true, true, true);
            i.mantling = true;
            assert_eq!(u.tick(&i, &p), None);
        }
        // disabled
        let mut u = Unstick::default();
        let off = UnstickParams { enabled: false, ..p };
        for k in 0..(60 * 30) {
            assert_eq!(u.tick(&us(k as f64 / 60.0, 3.0, true, true, true), &off), None);
        }
    }

    #[test]
    fn unstick_inside_evidence_alone_fires() {
        let p = UnstickParams::default();
        let mut u = Unstick::default();
        let mut got = None;
        for k in 0..(60 * 4) {
            if let Some(e) = u.tick(&us(k as f64 / 60.0, 3.0, true, false, k % 5 == 0), &p) {
                got = Some(e);
                break;
            }
        }
        assert!(matches!(got, Some(UnstickEvent::Fire(UnstickFire { n: 1, inside_ticks: 30.., .. }))), "{got:?}");
        assert!(Reason::Unstick.kind() == Kind::Suspend && !Reason::Unstick.keeps_stance() && Reason::Unstick.label() == "unstick");
    }

    #[test]
    fn c3_fpanim_root_motion_keeps_unstick_quiet() {
        // fp_anim = "hybrid": ER sees W and its run root motion (4.1 m/s) moves the proxy along our motion every tick
        // before our write puts it back. Walking into an ordinary wall for 30 s at 30 / 60 / 144 Hz: the push-out never
        // counts it (not opposite to our motion), so the unstick detector never gets evidence and stays quiet.
        let p = UnstickParams::default();
        let pp = PushParams::default();
        let dir = V3::new(0.0, 0.0, 1.0);
        for hz in [30.0f32, 60.0, 144.0] {
            let mut u = Unstick::default();
            let rm = 4.1 / hz;
            for k in 0..((30.0 * hz) as usize) {
                // ER's displacement: its root motion along W plus a little sideways jitter of the animation
                let e = V3::new(0.004 * ((k as f32) * 0.7).sin(), 0.0, rm);
                let pushed = matches!(er_pushout(e, dir, &pp), ErDelta::Apply(_));
                assert!(!pushed, "{hz} Hz tick {k}: root motion read as a push-out");
                assert_eq!(er_pushout(e, dir, &pp), ErDelta::Ignore);
                assert_eq!(u.tick(&us(k as f64 / hz as f64, 3.0, true, pushed, false), &p), None);
            }
            // the mantle's er_push measure: the expected root motion is removed, a real shove is kept
            let comp = rm_compensate(V3::new(0.0, 0.0, rm), dir, 4.1 / hz);
            assert!(comp.len() < 1e-6, "{hz} Hz: {comp:?}");
            let shove = rm_compensate(V3::new(0.4, 0.0, rm), dir, 4.1 / hz);
            assert!((shove.len() - 0.4).abs() < 1e-5, "a sideways shove is kept");
        }
        // at 20 Hz ER's run moves 0.205 m per tick: uncompensated a mantle would read it as ER pushing (0.30 m is the
        // abort); compensated it reads 0
        assert!(rm_compensate(V3::new(0.0, 0.0, 0.205), dir, 0.205).len() < 1e-6);
        // ER pushing us back (opposite to our motion) is never removed
        assert_eq!(rm_compensate(V3::new(0.0, 0.0, -0.1), dir, 0.2), V3::new(0.0, 0.0, -0.1));
        assert_eq!(rm_compensate(V3::new(0.1, 0.0, 0.1), V3::ZERO, 0.2), V3::new(0.1, 0.0, 0.1), "no motion: no compensation");
    }
}
