//! AC-130 killstreak state machine (docs/M4_DESIGN.md section 2). Pure; the DLL's controller (`ac130.rs`) feeds it
//! the signals it read and acts on its events.
//!
//! Phases: `Idle -> Starting -> Active -> Ending -> Idle`. The cooldown is a clock (`cooldown_until`) that keeps
//! running whatever else happens (MW2 mode off, menus, loading): `Ready` = Idle and the cooldown over (or disabled).
//! `Unavailable` is not a phase: the start gate reports why F7 is refused now.
//!
//! - F7 in Idle runs the start gate (11 ordered checks); a pass enters `Starting` (the controller applies the
//!   protection SpEffect). `Starting -> Active` only once the effect is seen present, at most `verify_ticks` ticks;
//!   otherwise the ride ends with `protect_fail`. **No path reaches Active without protection present.**
//! - Every exit goes through [`Ac130Machine::end`] with ONE reason chosen by a fixed priority ([`Reason`] order).
//! - First person is blocked (fp_logic reason `ac130`) while Starting/Active; the block is cleared in the end tick.

/// Why F7 is refused (start gate), in check order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Disabled,
    Mw2Off,
    Cooldown,
    NoCamHook,
    RowsMissing,
    NotSettled,
    Loading,
    Dead,
    Cutscene,
    Talk,
    Menu,
    Camtype,
    Torrent,
    /// M8 B1: on a ladder (ER third person; the gunship would start mid-climb).
    Ladder,
    NoSky,
    Already,
    /// M6 (run-13 bug 1): a start that aborted for an environment reason waits `start_backoff_s` before the next try.
    Backoff,
}

impl Block {
    pub const ALL: [Block; 17] = [
        Block::Disabled,
        Block::Mw2Off,
        Block::Cooldown,
        Block::NoCamHook,
        Block::RowsMissing,
        Block::NotSettled,
        Block::Loading,
        Block::Dead,
        Block::Cutscene,
        Block::Talk,
        Block::Menu,
        Block::Camtype,
        Block::Torrent,
        Block::Ladder,
        Block::NoSky,
        Block::Already,
        Block::Backoff,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Block::Disabled => "disabled",
            Block::Mw2Off => "mw2_off",
            Block::Cooldown => "cooldown",
            Block::NoCamHook => "no_cam_hook",
            Block::RowsMissing => "rows_missing",
            Block::NotSettled => "not_settled",
            Block::Loading => "loading",
            Block::Dead => "dead",
            Block::Cutscene => "cutscene",
            Block::Talk => "talk",
            Block::Menu => "menu",
            Block::Camtype => "camtype",
            Block::Torrent => "torrent",
            Block::Ladder => "ladder",
            Block::NoSky => "no_sky",
            Block::Already => "already",
            Block::Backoff => "retry_backoff",
        }
    }
}

/// Exit reasons in priority order (the first active one is reported).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    Panic,
    WorldLeft,
    Loading,
    Death,
    ReadFail,
    CamFail,
    ProtectFail,
    Cutscene,
    Menu,
    Talk,
    Camtype,
    Moved,
    F5,
    ExitKey,
    Timeout,
    Config,
}

impl Reason {
    pub const ALL: [Reason; 16] = [
        Reason::Panic,
        Reason::WorldLeft,
        Reason::Loading,
        Reason::Death,
        Reason::ReadFail,
        Reason::CamFail,
        Reason::ProtectFail,
        Reason::Cutscene,
        Reason::Menu,
        Reason::Talk,
        Reason::Camtype,
        Reason::Moved,
        Reason::F5,
        Reason::ExitKey,
        Reason::Timeout,
        Reason::Config,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Reason::Panic => "panic",
            Reason::WorldLeft => "world_left",
            Reason::Loading => "loading",
            Reason::Death => "death",
            Reason::ReadFail => "read_fail",
            Reason::CamFail => "cam_fail",
            Reason::ProtectFail => "protect_fail",
            Reason::Cutscene => "cutscene",
            Reason::Menu => "menu",
            Reason::Talk => "talk",
            Reason::Camtype => "camtype",
            Reason::Moved => "moved",
            Reason::F5 => "f5",
            Reason::ExitKey => "exit_key",
            Reason::Timeout => "timeout",
            Reason::Config => "config",
        }
    }

    /// The protection SpEffect is removed in the end tick for these (the player is dying, loading, gone, the mod
    /// failed or MW2 mode went off); otherwise after the last explosion + 0.5 s.
    pub fn removes_protection_now(self) -> bool {
        matches!(self, Reason::Death | Reason::Loading | Reason::WorldLeft | Reason::Panic | Reason::F5)
    }

    /// M6: reasons from the world (not the player's choice): an aborted start for one of these backs off.
    pub fn is_environment(self) -> bool {
        matches!(self, Reason::Loading | Reason::Cutscene | Reason::Talk | Reason::Camtype | Reason::ReadFail | Reason::ProtectFail)
    }

    /// Shells in flight still land (if `shells_after_exit`) only for a normal end of the ride.
    pub fn keeps_shells(self, shells_after_exit: bool) -> bool {
        shells_after_exit && matches!(self, Reason::Timeout | Reason::ExitKey)
    }
}

/// Signals of the start gate that the controller reads (cooldown and "already" come from the machine).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GateIn {
    pub enabled: bool,
    pub mw2_on: bool,
    pub hook_ok: bool,
    pub rows_ok: bool,
    pub settled: bool,
    pub loading: bool,
    pub dead: bool,
    pub cutscene: bool,
    pub talk: bool,
    pub menu: bool,
    /// ChrCam.camera_type == 0 (normal follow camera).
    pub camtype_ok: bool,
    pub torrent: bool,
    /// M8 B1: `CSChrLadderModule.state != -1`.
    pub ladder: bool,
    /// The open-sky ray found nothing above the player (or the check is off).
    pub sky_ok: bool,
}

impl GateIn {
    /// Every external check passes.
    pub fn all_ok() -> GateIn {
        GateIn { enabled: true, mw2_on: true, hook_ok: true, rows_ok: true, settled: true, camtype_ok: true, sky_ok: true, ..GateIn::default() }
    }

    /// A gate input whose only failing check is `b` (machine-side blocks give `all_ok`).
    pub fn failing(b: Block) -> GateIn {
        let mut g = GateIn::all_ok();
        match b {
            Block::Disabled => g.enabled = false,
            Block::Mw2Off => g.mw2_on = false,
            Block::NoCamHook => g.hook_ok = false,
            Block::RowsMissing => g.rows_ok = false,
            Block::NotSettled => g.settled = false,
            Block::Loading => g.loading = true,
            Block::Dead => g.dead = true,
            Block::Cutscene => g.cutscene = true,
            Block::Talk => g.talk = true,
            Block::Menu => g.menu = true,
            Block::Camtype => g.camtype_ok = false,
            Block::Torrent => g.torrent = true,
            Block::Ladder => g.ladder = true,
            Block::NoSky => g.sky_ok = false,
            Block::Cooldown | Block::Already | Block::Backoff => {}
        }
        g
    }
}

/// This tick's exit triggers (true = active). `Panic` is not an input: see [`Ac130Machine::force_end`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExitIn {
    pub world_left: bool,
    pub loading: bool,
    pub dead: bool,
    pub read_fail: bool,
    pub cam_fail: bool,
    pub protect_missing: bool,
    pub cutscene: bool,
    pub menu: bool,
    pub talk: bool,
    pub camtype_changed: bool,
    pub moved: bool,
    pub mw2_off: bool,
    pub exit_key: bool,
    pub timeout: bool,
    pub disabled: bool,
}

impl ExitIn {
    pub fn first(&self) -> Option<Reason> {
        [
            (self.world_left, Reason::WorldLeft),
            (self.loading, Reason::Loading),
            (self.dead, Reason::Death),
            (self.read_fail, Reason::ReadFail),
            (self.cam_fail, Reason::CamFail),
            (self.protect_missing, Reason::ProtectFail),
            (self.cutscene, Reason::Cutscene),
            (self.menu, Reason::Menu),
            (self.talk, Reason::Talk),
            (self.camtype_changed, Reason::Camtype),
            (self.moved, Reason::Moved),
            (self.mw2_off, Reason::F5),
            (self.exit_key, Reason::ExitKey),
            (self.timeout, Reason::Timeout),
            (self.disabled, Reason::Config),
        ]
        .iter()
        .find(|(on, _)| *on)
        .map(|(_, r)| *r)
    }

    pub fn only(r: Reason) -> ExitIn {
        let mut e = ExitIn::default();
        match r {
            Reason::Panic => {}
            Reason::WorldLeft => e.world_left = true,
            Reason::Loading => e.loading = true,
            Reason::Death => e.dead = true,
            Reason::ReadFail => e.read_fail = true,
            Reason::CamFail => e.cam_fail = true,
            Reason::ProtectFail => e.protect_missing = true,
            Reason::Cutscene => e.cutscene = true,
            Reason::Menu => e.menu = true,
            Reason::Talk => e.talk = true,
            Reason::Camtype => e.camtype_changed = true,
            Reason::Moved => e.moved = true,
            Reason::F5 => e.mw2_off = true,
            Reason::ExitKey => e.exit_key = true,
            Reason::Timeout => e.timeout = true,
            Reason::Config => e.disabled = true,
        }
        e
    }

    pub fn or(self, o: ExitIn) -> ExitIn {
        ExitIn {
            world_left: self.world_left || o.world_left,
            loading: self.loading || o.loading,
            dead: self.dead || o.dead,
            read_fail: self.read_fail || o.read_fail,
            cam_fail: self.cam_fail || o.cam_fail,
            protect_missing: self.protect_missing || o.protect_missing,
            cutscene: self.cutscene || o.cutscene,
            menu: self.menu || o.menu,
            talk: self.talk || o.talk,
            camtype_changed: self.camtype_changed || o.camtype_changed,
            moved: self.moved || o.moved,
            mw2_off: self.mw2_off || o.mw2_off,
            exit_key: self.exit_key || o.exit_key,
            timeout: self.timeout || o.timeout,
            disabled: self.disabled || o.disabled,
        }
    }
}

/// Timing parameters (from `[ac130]` and `timing.json`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub duration_s: f64,
    pub cooldown_s: f64,
    pub cooldown_enabled: bool,
    pub verify_ticks: u32,
    pub shells_after_exit: bool,
    /// M6: seconds an aborted start (environment reason, before Active) blocks the next F7 (`Block::Backoff`).
    pub start_backoff_s: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params { duration_s: 40.0, cooldown_s: 120.0, cooldown_enabled: true, verify_ticks: PROTECT_VERIFY_TICKS, shells_after_exit: true, start_backoff_s: 1.0 }
    }
}

/// Ticks the protection SpEffect may take to show up in `special_effect.entries()` after the apply call.
pub const PROTECT_VERIFY_TICKS: u32 = 5;
/// The protection is removed this long after the last explosion (owner-safe anyway: selfTarget 0).
pub const PROTECT_LINGER_S: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Starting { ticks: u32 },
    Active { t0: f64 },
    Ending { reason: Reason, t_end: f64, protect_at: f64 },
}

/// What `end()` decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndPlan {
    pub reason: Reason,
    pub drop_shells: bool,
    pub protect_at: f64,
    /// Seconds the ride lasted (0 when it never became Active).
    pub t_s: f64,
    /// The cooldown this end charged: `cooldown_s` when the ride reached Active, 0 for an aborted start
    /// (protect_fail / read_fail / ... during Starting; review M4 minor 12) or with the cooldown disabled.
    pub cooldown_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Activated,
    Ended(EndPlan),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ac130Machine {
    phase: Phase,
    cooldown_until: f64,
    t_active: Option<f64>,
    /// M6: no new start before this time (an aborted start's back-off).
    backoff_until: f64,
}

impl Default for Ac130Machine {
    /// Ready right after DLL load (no cooldown pending).
    fn default() -> Self {
        Ac130Machine { phase: Phase::Idle, cooldown_until: f64::NEG_INFINITY, t_active: None, backoff_until: f64::NEG_INFINITY }
    }
}

/// Facts about pending explosions that decide when the protection may go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShellTimes {
    /// Latest due time of a shell still in flight.
    pub last_due: Option<f64>,
    /// Time of the last explosion spawned.
    pub last_explosion: Option<f64>,
}

impl Ac130Machine {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Active { .. })
    }

    /// First person stays handed back while the ride starts and runs.
    pub fn fp_blocked(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Active { .. })
    }

    pub fn cooldown_remaining(&self, now: f64, p: &Params) -> f64 {
        if !p.cooldown_enabled { 0.0 } else { (self.cooldown_until - now).max(0.0) }
    }

    pub fn ready(&self, now: f64, p: &Params) -> bool {
        self.is_idle() && self.cooldown_remaining(now, p) <= 0.0
    }

    /// Seconds since Active began.
    pub fn active_t(&self, now: f64) -> Option<f64> {
        match self.phase {
            Phase::Active { t0 } => Some(now - t0),
            _ => None,
        }
    }

    /// Runs the start gate (docs/M4_DESIGN.md table 2.2) and enters Starting on a pass.
    pub fn start(&mut self, now: f64, g: &GateIn, p: &Params) -> Result<(), Block> {
        self.check(now, g, p)?;
        self.phase = Phase::Starting { ticks: 0 };
        self.t_active = None;
        Ok(())
    }

    /// The start gate without any state change (the HUD indicator; the controller evaluates the open-sky ray only
    /// once every cheaper check passed).
    pub fn check(&self, now: f64, g: &GateIn, p: &Params) -> Result<(), Block> {
        let checks = [
            (!g.enabled, Block::Disabled),
            (!g.mw2_on, Block::Mw2Off),
            (self.is_idle() && self.cooldown_remaining(now, p) > 0.0, Block::Cooldown),
            (self.is_idle() && now < self.backoff_until, Block::Backoff),
            (!g.hook_ok, Block::NoCamHook),
            (!g.rows_ok, Block::RowsMissing),
            (!g.settled, Block::NotSettled),
            (g.loading, Block::Loading),
            (g.dead, Block::Dead),
            (g.cutscene, Block::Cutscene),
            (g.talk, Block::Talk),
            (g.menu, Block::Menu),
            (!g.camtype_ok, Block::Camtype),
            (g.torrent, Block::Torrent),
            (g.ladder, Block::Ladder),
            (!g.sky_ok, Block::NoSky),
            (!self.is_idle(), Block::Already),
        ];
        match checks.iter().find(|(fail, _)| *fail) {
            Some((_, b)) => Err(*b),
            None => Ok(()),
        }
    }

    fn end_plan(&mut self, r: Reason, now: f64, s: &ShellTimes, p: &Params) -> EndPlan {
        let keep = r.keeps_shells(p.shells_after_exit);
        let protect_at = if r.removes_protection_now() {
            now
        } else {
            let mut t = now;
            if let Some(e) = s.last_explosion {
                t = t.max(e + PROTECT_LINGER_S);
            }
            if keep && let Some(d) = s.last_due {
                t = t.max(d + PROTECT_LINGER_S);
            }
            t
        };
        let t_s = self.t_active.map_or(0.0, |t0| (now - t0).max(0.0));
        self.phase = Phase::Ending { reason: r, t_end: now, protect_at };
        // only a ride that became Active uses up the cooldown; an aborted start leaves the clock as it was (it had
        // run out, or the start gate would have refused F7)
        let charged = if self.t_active.is_some() && p.cooldown_enabled { p.cooldown_s } else { 0.0 };
        // M6 run-13 bug 1: an aborted start (never Active) for an environment reason backs off before the next try
        if self.t_active.is_none() && r.is_environment() {
            self.backoff_until = now + p.start_backoff_s.max(0.0);
        }
        if self.t_active.is_some() {
            self.cooldown_until = now + charged;
        }
        EndPlan { reason: r, drop_shells: !keep, protect_at, t_s, cooldown_s: charged }
    }

    /// One tick while Starting or Active. `protect_present` is this tick's `entries()` scan.
    pub fn tick(&mut self, now: f64, ex: &ExitIn, protect_present: bool, s: &ShellTimes, p: &Params) -> Option<Event> {
        match self.phase {
            Phase::Idle | Phase::Ending { .. } => None,
            Phase::Starting { ticks } => {
                // A ride that never became Active ends on any trigger but the clock.
                let ex = ExitIn { timeout: false, ..*ex };
                if let Some(r) = ex.first() {
                    return Some(Event::Ended(self.end_plan(r, now, s, p)));
                }
                if protect_present {
                    self.phase = Phase::Active { t0: now };
                    self.t_active = Some(now);
                    return Some(Event::Activated);
                }
                if ticks + 1 >= p.verify_ticks.max(1) {
                    return Some(Event::Ended(self.end_plan(Reason::ProtectFail, now, s, p)));
                }
                self.phase = Phase::Starting { ticks: ticks + 1 };
                None
            }
            Phase::Active { t0 } => {
                let mut ex = *ex;
                ex.timeout |= now - t0 >= p.duration_s;
                // a ride never continues without the protection (checked here every tick the caller scans)
                ex.protect_missing |= !protect_present;
                ex.first().map(|r| Event::Ended(self.end_plan(r, now, s, p)))
            }
        }
    }

    /// Ends a ride from outside the tick (panic path, config off). `None` when nothing was running.
    pub fn force_end(&mut self, r: Reason, now: f64, s: &ShellTimes, p: &Params) -> Option<EndPlan> {
        matches!(self.phase, Phase::Starting { .. } | Phase::Active { .. }).then(|| self.end_plan(r, now, s, p))
    }

    /// True while Ending and the protection removal is due.
    pub fn protect_due(&self, now: f64) -> bool {
        matches!(self.phase, Phase::Ending { protect_at, .. } if now >= protect_at)
    }

    /// Ending -> Idle once the protection removal was verified (or deferred to the stray cleanup).
    pub fn finish(&mut self) -> bool {
        if matches!(self.phase, Phase::Ending { .. }) {
            self.phase = Phase::Idle;
            true
        } else {
            false
        }
    }
}

// ---- M6: run-13 bug 1 (docs/M6_DESIGN.md section 10.1) -----------------------------------------------------------

/// The ride's exit edges (load counter, camera mask, talk id). Armed when a start passes the gate (from this tick's
/// values, so a load during Starting is caught against a fresh value) and cleared when the ride ends: a stale baseline
/// from an earlier ride can never end a new one (run 13: a death reload raised the counter 1 -> 2 after a ride and every
/// later F7 ended at once with `reason=loading t_s=0.0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RideEdges {
    pub load: crate::fp_logic::EdgeWatch<i32>,
    pub mask: crate::fp_logic::EdgeWatch<u32>,
    pub talk: crate::fp_logic::EdgeWatch<i32>,
}

impl RideEdges {
    pub fn arm(&mut self, load: Option<i32>, mask: Option<u32>, talk: Option<i32>) {
        self.load.arm(load);
        self.mask.arm(mask);
        self.talk.arm(talk);
    }

    pub fn clear(&mut self) {
        self.load.clear();
        self.mask.clear();
        self.talk.clear();
    }

    pub fn armed(&self) -> bool {
        self.load.baseline().is_some() || self.mask.baseline().is_some() || self.talk.baseline().is_some()
    }

    /// 2026-10-21 (review of the boss-fight fix): a talk id change with no interact press within `window_s` is a boss /
    /// NPC voice line (fp_logic::TalkGate), not a dialog: the talk baseline takes the new id (a later real dialog still
    /// fires against it) and `(old, new)` comes back to log. Call before [`Self::exit_flags`]. During a ride the
    /// capture hides ER's interact, so in practice every change is one.
    pub fn ignore_voice_line(&mut self, talk: Option<i32>, gate: &crate::fp_logic::TalkGate, now: f64, window_s: f64) -> Option<(i32, i32)> {
        let (Some(b), Some(c)) = (self.talk.baseline(), talk) else { return None };
        if b == c || gate.is_dialog(now, window_s) {
            return None;
        }
        self.talk.arm(talk);
        Some((b, c))
    }

    /// (loading, camera mask changed, talk changed) against the armed baseline.
    pub fn exit_flags(&self, load: Option<i32>, mask: Option<u32>, talk: Option<i32>) -> (bool, bool, bool) {
        (self.load.changed(load), self.mask.changed(mask), self.talk.changed(talk))
    }
}

/// Merges repeated identical aborted starts into one log line (`AC130 start aborted reason=<r> x<N> in <T>s`), flushed
/// on a different reason or after `MERGE_FLUSH_S`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbortMerge {
    reason: Option<Reason>,
    count: u32,
    t_first: f64,
    t_last: f64,
}

pub const MERGE_FLUSH_S: f64 = 5.0;

/// A merged run of identical aborts, ready to log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergedAborts {
    pub reason: Reason,
    pub count: u32,
    pub span_s: f64,
}

impl AbortMerge {
    /// Records an abort. Returns `(log_now, flushed)`: `log_now` is true for the first abort of a run (log it in full);
    /// `flushed` is the previous run when this abort starts a new one.
    pub fn push(&mut self, r: Reason, now: f64) -> (bool, Option<MergedAborts>) {
        if self.reason == Some(r) && now - self.t_first <= MERGE_FLUSH_S {
            self.count += 1;
            self.t_last = now;
            return (false, None);
        }
        let prev = self.take();
        *self = AbortMerge { reason: Some(r), count: 1, t_first: now, t_last: now };
        (true, prev)
    }

    /// The pending run if it is older than `MERGE_FLUSH_S` (call every tick).
    pub fn poll(&mut self, now: f64) -> Option<MergedAborts> {
        if self.reason.is_some() && now - self.t_first > MERGE_FLUSH_S { self.take() } else { None }
    }

    fn take(&mut self) -> Option<MergedAborts> {
        let r = self.reason.take()?;
        let out = (self.count > 1).then_some(MergedAborts { reason: r, count: self.count, span_s: self.t_last - self.t_first });
        *self = AbortMerge::default();
        out
    }
}

/// What the HUD indicator shows in normal MW2 mode.
#[derive(Clone, Debug, PartialEq)]
pub enum Indicator {
    Ready,
    Cooldown(f64),
    Unavailable(Block),
    Busy,
}

/// The indicator for the current state; `block` is the first failing external check (if any), evaluated by the
/// controller without the expensive sky ray.
pub fn indicator(m: &Ac130Machine, now: f64, p: &Params, block: Option<Block>) -> Indicator {
    if !m.is_idle() {
        return Indicator::Busy;
    }
    let rem = m.cooldown_remaining(now, p);
    match block {
        Some(Block::Disabled | Block::Mw2Off | Block::NoCamHook | Block::RowsMissing) => Indicator::Unavailable(block.unwrap_or(Block::Disabled)),
        _ if rem > 0.0 => Indicator::Cooldown(rem),
        Some(b) => Indicator::Unavailable(b),
        None => Indicator::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Params = Params { duration_s: 40.0, cooldown_s: 120.0, cooldown_enabled: true, verify_ticks: 5, shells_after_exit: true, start_backoff_s: 1.0 };
    const NO: ShellTimes = ShellTimes { last_due: None, last_explosion: None };

    /// Idle -> Starting -> Active at t.
    fn active(t: f64) -> Ac130Machine {
        let mut m = Ac130Machine::default();
        m.start(t, &GateIn::all_ok(), &P).unwrap();
        assert_eq!(m.tick(t, &ExitIn::default(), true, &NO, &P), Some(Event::Activated));
        m
    }

    #[test]
    fn ac130_logic_ready_on_start_and_gate_order() {
        let m = Ac130Machine::default();
        assert!(m.ready(0.0, &P), "ready right after DLL load");
        // each external block alone is reported
        for b in Block::ALL.into_iter().filter(|b| !matches!(b, Block::Cooldown | Block::Already | Block::Backoff)) {
            let mut m = Ac130Machine::default();
            assert_eq!(m.start(1.0, &GateIn::failing(b), &P), Err(b), "{b:?}");
            assert!(m.is_idle());
        }
        // table order: for every pair of failing checks, the earlier one is reported
        let ext: Vec<Block> = Block::ALL.into_iter().filter(|b| !matches!(b, Block::Cooldown | Block::Already | Block::Backoff)).collect();
        for (i, a) in ext.iter().enumerate() {
            for b in &ext[i + 1..] {
                let ga = GateIn::failing(*a);
                let gb = GateIn::failing(*b);
                let g = GateIn {
                    enabled: ga.enabled && gb.enabled,
                    mw2_on: ga.mw2_on && gb.mw2_on,
                    hook_ok: ga.hook_ok && gb.hook_ok,
                    rows_ok: ga.rows_ok && gb.rows_ok,
                    settled: ga.settled && gb.settled,
                    loading: ga.loading || gb.loading,
                    dead: ga.dead || gb.dead,
                    cutscene: ga.cutscene || gb.cutscene,
                    talk: ga.talk || gb.talk,
                    menu: ga.menu || gb.menu,
                    camtype_ok: ga.camtype_ok && gb.camtype_ok,
                    torrent: ga.torrent || gb.torrent,
                    ladder: ga.ladder || gb.ladder,
                    sky_ok: ga.sky_ok && gb.sky_ok,
                };
                assert_eq!(Ac130Machine::default().start(1.0, &g, &P), Err(*a), "{a:?} vs {b:?}");
            }
        }
        // cooldown comes after mw2_off and before no_cam_hook
        let mut m = active(0.0);
        let plan = m.tick(5.0, &ExitIn::only(Reason::ExitKey), true, &NO, &P);
        assert!(matches!(plan, Some(Event::Ended(_))));
        assert!(m.finish());
        assert_eq!(m.start(6.0, &GateIn::failing(Block::Mw2Off), &P), Err(Block::Mw2Off));
        assert_eq!(m.start(6.0, &GateIn::failing(Block::NoCamHook), &P), Err(Block::Cooldown));
        // already: F7 while Starting / Ending
        let mut s = Ac130Machine::default();
        s.start(0.0, &GateIn::all_ok(), &P).unwrap();
        assert_eq!(s.start(0.1, &GateIn::all_ok(), &P), Err(Block::Already));
        let labels: Vec<&str> = Block::ALL.iter().map(|b| b.label()).collect();
        assert_eq!(labels[..6], ["disabled", "mw2_off", "cooldown", "no_cam_hook", "rows_missing", "not_settled"]);
    }

    #[test]
    fn ac130_logic_active_only_after_protection() {
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &P).unwrap();
        assert!(m.fp_blocked(), "first person handed back from the Starting tick on");
        for k in 0..4 {
            assert_eq!(m.tick(k as f64 * 0.016, &ExitIn::default(), false, &NO, &P), None);
            assert!(!m.is_active());
        }
        assert_eq!(m.tick(0.08, &ExitIn::default(), true, &NO, &P), Some(Event::Activated));
        assert!(m.is_active());
        // Starting timeout -> protect_fail
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &P).unwrap();
        let mut ev = None;
        for k in 0..5 {
            ev = m.tick(k as f64 * 0.016, &ExitIn::default(), false, &NO, &P);
            if ev.is_some() {
                break;
            }
        }
        match ev {
            Some(Event::Ended(p)) => assert_eq!(p.reason, Reason::ProtectFail),
            e => panic!("{e:?}"),
        }
        assert!(!m.is_active() && !m.fp_blocked());
    }

    #[test]
    fn ac130_logic_each_exit_reason() {
        for r in Reason::ALL.into_iter().filter(|r| *r != Reason::Panic) {
            let mut m = active(0.0);
            let ex = if r == Reason::Timeout { ExitIn::default() } else { ExitIn::only(r) };
            let now = if r == Reason::Timeout { 40.0 } else { 3.0 };
            match m.tick(now, &ex, true, &NO, &P) {
                Some(Event::Ended(p)) => assert_eq!(p.reason, r, "{r:?}"),
                e => panic!("{r:?}: {e:?}"),
            }
            assert!(!m.fp_blocked() && !m.is_active(), "{r:?}: FP blocker cleared in the end tick");
            assert!(m.finish());
            assert!(m.is_idle());
        }
        // panic from outside the tick
        let mut m = active(0.0);
        assert_eq!(m.force_end(Reason::Panic, 1.0, &NO, &P).map(|p| p.reason), Some(Reason::Panic));
        assert!(m.force_end(Reason::Panic, 1.0, &NO, &P).is_none(), "nothing running any more");
        // timeout exactly at duration_s, not before
        let mut m = active(10.0);
        assert_eq!(m.tick(49.99, &ExitIn::default(), true, &NO, &P), None);
        assert!(matches!(m.tick(50.0, &ExitIn::default(), true, &NO, &P), Some(Event::Ended(EndPlan { reason: Reason::Timeout, .. }))));
        let labels: Vec<&str> = Reason::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(labels, ["panic", "world_left", "loading", "death", "read_fail", "cam_fail", "protect_fail", "cutscene", "menu", "talk", "camtype", "moved", "f5", "exit_key", "timeout", "config"]);
    }

    #[test]
    fn ac130_logic_priority_order() {
        for (i, a) in Reason::ALL.iter().enumerate().skip(1) {
            for b in Reason::ALL.iter().skip(i + 1) {
                let ex = ExitIn::only(*a).or(ExitIn::only(*b));
                assert_eq!(ex.first(), Some(*a), "{a:?} vs {b:?}");
            }
        }
        // e.g. death while the menu opens and F7 is pressed -> death
        let mut m = active(0.0);
        let ex = ExitIn { dead: true, menu: true, exit_key: true, ..ExitIn::default() };
        assert!(matches!(m.tick(1.0, &ex, true, &NO, &P), Some(Event::Ended(EndPlan { reason: Reason::Death, .. }))));
    }

    #[test]
    fn ac130_logic_protection_removal_timing() {
        let shells = ShellTimes { last_due: Some(12.0), last_explosion: Some(10.0) };
        for r in Reason::ALL {
            let mut m = active(0.0);
            let p = if r == Reason::Panic { m.force_end(r, 11.0, &shells, &P).unwrap() } else {
                match m.tick(if r == Reason::Timeout { 40.0 } else { 11.0 }, &if r == Reason::Timeout { ExitIn::default() } else { ExitIn::only(r) }, true, &shells, &P) {
                    Some(Event::Ended(p)) => p,
                    e => panic!("{e:?}"),
                }
            };
            let now = if r == Reason::Timeout { 40.0 } else { 11.0 };
            if matches!(r, Reason::Death | Reason::Loading | Reason::WorldLeft | Reason::Panic | Reason::F5) {
                assert_eq!(p.protect_at, now, "{r:?}: immediately");
            } else if matches!(r, Reason::Timeout | Reason::ExitKey) {
                assert_eq!(p.protect_at, now.max(12.5), "{r:?}: after the last shell + 0.5 s");
                assert!(!p.drop_shells);
            } else {
                assert_eq!(p.protect_at, now.max(10.5), "{r:?}: after the last explosion + 0.5 s");
                assert!(p.drop_shells, "{r:?}: shells dropped");
            }
            assert_eq!(m.protect_due(p.protect_at - 0.01), p.protect_at - 0.01 >= p.protect_at);
            assert!(m.protect_due(p.protect_at));
        }
        // shells_after_exit = false drops them on exit_key too
        let mut m = active(0.0);
        let q = Params { shells_after_exit: false, ..P };
        match m.tick(5.0, &ExitIn::only(Reason::ExitKey), true, &shells, &q) {
            Some(Event::Ended(p)) => assert!(p.drop_shells),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn ac130_logic_cooldown_exact_and_disabled() {
        let mut m = active(0.0);
        m.tick(40.0, &ExitIn::default(), true, &NO, &P);
        m.finish();
        assert!(!m.ready(159.99, &P));
        assert!((m.cooldown_remaining(100.0, &P) - 60.0).abs() < 1e-9);
        assert!(m.ready(160.0, &P), "120 s from the end of the ride");
        // the clock runs through MW2-off periods: nothing but time is needed
        assert_eq!(m.start(130.0, &GateIn::failing(Block::Mw2Off), &P), Err(Block::Mw2Off));
        assert!(m.ready(160.0, &P));
        // cooldown disabled: ready at once
        let q = Params { cooldown_enabled: false, ..P };
        let mut m = active(0.0);
        m.tick(3.0, &ExitIn::only(Reason::ExitKey), true, &NO, &q);
        m.finish();
        assert!(m.ready(3.0, &q));
        assert!(m.start(3.0, &GateIn::all_ok(), &q).is_ok());
        // ending before finish(): not ready yet
        let mut m = active(0.0);
        m.tick(3.0, &ExitIn::only(Reason::Menu), true, &NO, &q);
        assert!(!m.ready(500.0, &q));
    }

    #[test]
    fn ac130_logic_aborted_start_charges_no_cooldown() {
        // protect_fail at Starting (the effect never showed up) -> no cooldown, F7 works again at once
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &P).unwrap();
        let mut plan = None;
        for k in 0..10 {
            if let Some(Event::Ended(p)) = m.tick(k as f64 * 0.016, &ExitIn::default(), false, &NO, &P) {
                plan = Some(p);
                break;
            }
        }
        let plan = plan.expect("protect_fail");
        assert_eq!(plan.reason, Reason::ProtectFail);
        assert_eq!(plan.cooldown_s, 0.0);
        assert!(m.finish());
        assert!(m.ready(0.2, &P), "an aborted start uses no cooldown");
        // M6 (run-13 bug 1): an aborted start backs off start_backoff_s (1 s) instead of retrying at once
        assert_eq!(m.start(0.2, &GateIn::all_ok(), &P), Err(Block::Backoff), "F7 within the back-off after protect_fail");
        assert!(m.start(1.2, &GateIn::all_ok(), &P).is_ok(), "F7 after the back-off");
        // forced end during Starting (read_fail / world_left) also charges nothing
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &P).unwrap();
        assert_eq!(m.force_end(Reason::WorldLeft, 0.05, &NO, &P).map(|p| p.cooldown_s), Some(0.0));
        m.finish();
        assert!(m.ready(0.05, &P));
        // a ride that reached Active is charged the full cooldown
        let mut m = active(0.0);
        match m.tick(3.0, &ExitIn::only(Reason::ExitKey), true, &NO, &P) {
            Some(Event::Ended(p)) => assert_eq!(p.cooldown_s, 120.0),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn ac130_logic_f7_during_active_is_exit_key_and_f5_off() {
        let mut m = active(0.0);
        assert!(matches!(m.tick(2.0, &ExitIn { exit_key: true, ..ExitIn::default() }, true, &NO, &P), Some(Event::Ended(EndPlan { reason: Reason::ExitKey, .. }))));
        let mut m = active(0.0);
        assert!(matches!(m.tick(2.0, &ExitIn { mw2_off: true, ..ExitIn::default() }, true, &NO, &P), Some(Event::Ended(EndPlan { reason: Reason::F5, .. }))));
    }

    #[test]
    fn ac130_logic_fp_blocker_every_path() {
        // table-driven: the blocker is set from start and cleared in the end tick for every reason
        for r in Reason::ALL {
            let mut m = Ac130Machine::default();
            assert!(!m.fp_blocked());
            m.start(0.0, &GateIn::all_ok(), &P).unwrap();
            assert!(m.fp_blocked());
            m.tick(0.016, &ExitIn::default(), true, &NO, &P);
            assert!(m.fp_blocked());
            if r == Reason::Panic {
                m.force_end(r, 1.0, &NO, &P);
            } else if r == Reason::Timeout {
                m.tick(41.0, &ExitIn::default(), true, &NO, &P);
            } else {
                m.tick(1.0, &ExitIn::only(r), true, &NO, &P);
            }
            assert!(!m.fp_blocked(), "{r:?}");
        }
    }

    #[test]
    fn ac130_logic_never_active_without_protection() {
        // exhaustive over short random-ish traces: Active only when this tick's scan saw the effect
        for seed in 0..200u32 {
            let mut m = Ac130Machine::default();
            let mut x = seed.wrapping_mul(2654435761);
            let mut t = 0.0;
            for _ in 0..80 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let present = x % 3 == 0;
                if m.is_idle() {
                    let _ = m.start(t, &GateIn::all_ok(), &Params { cooldown_enabled: false, ..P });
                }
                let ev = m.tick(t, &ExitIn { exit_key: x % 17 == 0, ..ExitIn::default() }, present, &NO, &Params { cooldown_enabled: false, ..P });
                if m.is_active() {
                    assert!(present, "seed {seed}: active without protection present ({ev:?})");
                }
                if matches!(m.phase(), Phase::Ending { .. }) {
                    m.finish();
                }
                t += 0.016;
            }
        }
    }

    #[test]
    fn ac130_logic_indicator_states() {
        let m = Ac130Machine::default();
        assert_eq!(indicator(&m, 0.0, &P, None), Indicator::Ready);
        assert_eq!(indicator(&m, 0.0, &P, Some(Block::NoSky)), Indicator::Unavailable(Block::NoSky));
        let mut m = active(0.0);
        assert_eq!(indicator(&m, 1.0, &P, None), Indicator::Busy);
        m.tick(5.0, &ExitIn::only(Reason::ExitKey), true, &NO, &P);
        m.finish();
        assert_eq!(indicator(&m, 65.0, &P, None), Indicator::Cooldown(60.0));
        assert_eq!(indicator(&m, 65.0, &P, Some(Block::Mw2Off)), Indicator::Unavailable(Block::Mw2Off));
        assert_eq!(indicator(&m, 65.0, &P, Some(Block::Menu)), Indicator::Cooldown(60.0));
    }

    // ---- M6 run-13 bug 1 (docs/M6_DESIGN.md section 10.1) --------------------------------------------------------

    #[test]
    fn ac130_edges_no_carry_over() {
        let mut e = RideEdges::default();
        e.arm(Some(1), Some(0), Some(-1));
        assert_eq!(e.exit_flags(Some(1), Some(0), Some(-1)), (false, false, false));
        e.clear();
        assert!(!e.armed());
        // the death reload raises the counter while no ride runs; the next start arms at the fresh value
        assert_eq!(e.exit_flags(Some(2), Some(0), Some(-1)), (false, false, false), "cleared: nothing stale");
        e.arm(Some(2), Some(0), Some(-1));
        assert_eq!(e.exit_flags(Some(2), Some(0), Some(-1)), (false, false, false));
        assert_eq!(e.exit_flags(Some(3), Some(0), Some(-1)), (true, false, false), "a real load during the ride");
        // the run-13 bug: a baseline kept from the earlier ride (1) against the live 2
        let mut stale = RideEdges::default();
        stale.arm(Some(1), Some(0), Some(-1));
        assert_eq!(stale.exit_flags(Some(2), Some(0), Some(-1)).0, true, "what the old code compared");
    }

    #[test]
    fn ac130_mask_talk_no_carry_over() {
        let mut e = RideEdges::default();
        e.arm(Some(1), Some(0x10), Some(5));
        e.clear();
        e.arm(Some(1), Some(0x20), Some(-1));
        assert_eq!(e.exit_flags(Some(1), Some(0x20), Some(-1)), (false, false, false));
        assert_eq!(e.exit_flags(Some(1), Some(0x30), Some(7)), (false, true, true));
        // an unavailable signal (None) is never a change
        assert_eq!(e.exit_flags(None, None, None), (false, false, false));
    }

    #[test]
    fn ac130_talk_edge_ignores_boss_voice_lines() {
        use crate::fp_logic::TalkGate;
        const W: f64 = 8.0;
        let hms = |h: f64, m: f64, s: f64| h * 3600.0 + m * 60.0 + s;
        // replays one logged talk id change during a ride (armed at the old id 2 s before it, interact presses as
        // logged, 60 Hz): (ride ended by talk, ignored (old, new))
        let replay = |presses: &[f64], from: i32, to: i32, t_change: f64, window_s: f64| {
            let mut e = RideEdges::default();
            let mut g = TalkGate::default();
            for &t in presses {
                g.interacted(1, t);
            }
            e.arm(Some(1), Some(0), Some(from));
            let (mut ended, mut ignored) = (false, None);
            let mut t = t_change - 2.0;
            while t < t_change + 2.0 {
                let id = if t >= t_change { to } else { from };
                if let Some(x) = e.ignore_voice_line(Some(id), &g, t, window_s) {
                    ignored = Some(x);
                }
                ended |= e.exit_flags(Some(1), Some(0), Some(id)).2;
                t += 1.0 / 60.0;
            }
            (ended, ignored)
        };
        // 2026-10-21 13:05:10.948: c2130 mid-fight, last interact 48 s before; 13:08:59.444 the arena line, 11.2 s after
        let mid = [hms(13.0, 4.0, 15.016), hms(13.0, 4.0, 19.299), hms(13.0, 4.0, 22.699)];
        assert_eq!(replay(&mid, 1000, 204_101_000, hms(13.0, 5.0, 10.948), W), (false, Some((1000, 204_101_000))));
        let arena = [hms(13.0, 8.0, 41.498), hms(13.0, 8.0, 48.201)];
        assert_eq!(replay(&arena, 1000, 302_001_000, hms(13.0, 8.0, 59.444), W), (false, Some((1000, 302_001_000))));
        // mutation: without the gate (window 0, the old exit) both end the ride
        assert_eq!(replay(&mid, 1000, 204_101_000, hms(13.0, 5.0, 10.948), 0.0), (true, None));
        assert_eq!(replay(&arena, 1000, 302_001_000, hms(13.0, 8.0, 59.444), 0.0), (true, None));
        // a change right after an interact (a dialog) still ends it; after an ignored line the next change is
        // compared against the new id
        assert_eq!(replay(&[100.0], 1000, 302_001_000, 100.8, W), (true, None));
        let mut e = RideEdges::default();
        let mut g = TalkGate::default();
        e.arm(Some(1), Some(0), Some(1000));
        assert_eq!(e.ignore_voice_line(Some(204_101_000), &g, 50.0, W), Some((1000, 204_101_000)));
        assert_eq!(e.exit_flags(Some(1), Some(0), Some(204_101_000)), (false, false, false));
        g.interacted(1, 60.0);
        assert_eq!(e.ignore_voice_line(Some(1000), &g, 61.0, W), None);
        assert_eq!(e.exit_flags(Some(1), Some(0), Some(1000)), (false, false, true));
        // unarmed / unavailable: nothing
        assert_eq!(RideEdges::default().ignore_voice_line(Some(5), &g, 1e6, W), None);
        assert_eq!(e.ignore_voice_line(None, &g, 1e6, W), None);
    }

    #[test]
    fn ac130_machine_start_after_reload_activates() {
        // run-13 order: ride at load 1 -> end -> counter 2 -> start: the edges armed at start see no change
        let mut m = active(0.0);
        let mut e = RideEdges::default();
        e.arm(Some(1), Some(0), Some(-1));
        let plan = m.tick(10.0, &ExitIn::only(Reason::ExitKey), true, &NO, &Params { cooldown_enabled: false, ..P });
        assert!(matches!(plan, Some(Event::Ended(_))));
        e.clear();
        assert!(m.finish());
        let q = Params { cooldown_enabled: false, ..P };
        m.start(30.0, &GateIn::all_ok(), &q).unwrap();
        e.arm(Some(2), Some(0), Some(-1));
        let (loading, cut, talk) = e.exit_flags(Some(2), Some(0), Some(-1));
        let ex = ExitIn { loading, cutscene: cut, talk, ..ExitIn::default() };
        assert_eq!(m.tick(30.016, &ex, true, &NO, &q), Some(Event::Activated), "reaches Active");
        // a change during Starting ends with Reason::Loading
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &q).unwrap();
        let mut e = RideEdges::default();
        e.arm(Some(2), Some(0), Some(-1));
        let (loading, _, _) = e.exit_flags(Some(3), Some(0), Some(-1));
        match m.tick(0.016, &ExitIn { loading, ..ExitIn::default() }, false, &NO, &q) {
            Some(Event::Ended(p)) => assert_eq!((p.reason, p.t_s), (Reason::Loading, 0.0)),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn ac130_abort_backoff() {
        let q = Params { cooldown_enabled: false, ..P };
        let mut m = Ac130Machine::default();
        m.start(0.0, &GateIn::all_ok(), &q).unwrap();
        let _ = m.tick(0.016, &ExitIn::only(Reason::Loading), false, &NO, &q);
        assert!(m.finish());
        // two F7 within 1 s: one attempt + Block::Backoff
        assert_eq!(m.start(0.5, &GateIn::all_ok(), &q), Err(Block::Backoff));
        assert_eq!(Block::Backoff.label(), "retry_backoff");
        // after 1 s a new attempt
        assert_eq!(m.start(1.1, &GateIn::all_ok(), &q), Ok(()));
        // the player's own exit (exit_key) and a protect wait that succeeded do not back off
        let mut m = active(0.0);
        let _ = m.tick(1.0, &ExitIn::only(Reason::ExitKey), true, &NO, &q);
        m.finish();
        assert_eq!(m.start(1.2, &GateIn::all_ok(), &q), Ok(()));
    }

    #[test]
    fn ac130_abort_log_merge() {
        // 19 identical aborts (run 13: 20:22:22.963 .. 20:22:43.843): the first logged in full, then merged lines
        let mut mg = AbortMerge::default();
        let mut full = 0;
        let mut merged = Vec::new();
        for k in 0..19 {
            let t = k as f64 * 1.1;
            let (now, prev) = mg.push(Reason::Loading, t);
            full += now as u32;
            merged.extend(prev);
            merged.extend(mg.poll(t));
        }
        merged.extend(mg.poll(100.0));
        let total: u32 = merged.iter().map(|m| m.count).sum::<u32>() + full - merged.len() as u32;
        assert_eq!(total, 19, "every abort is accounted for");
        assert!(full + merged.len() as u32 <= 9, "19 aborts -> a handful of lines ({full} full + {} merged)", merged.len());
        // a different reason flushes the run
        let mut mg = AbortMerge::default();
        mg.push(Reason::Loading, 0.0);
        mg.push(Reason::Loading, 0.5);
        let (now, prev) = mg.push(Reason::Talk, 0.6);
        assert!(now);
        assert_eq!(prev, Some(MergedAborts { reason: Reason::Loading, count: 2, span_s: 0.5 }));
    }
}
