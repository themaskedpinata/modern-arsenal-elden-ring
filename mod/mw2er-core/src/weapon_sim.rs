//! Fire cadence and the ammo/reload state machine (docs/M2_DESIGN.md sections 10.1 and 11; M6 additions in
//! docs/M6_DESIGN.md sections 6.1-6.3).
//!
//! Time is real time in seconds (the DLL passes a QPC-backed clock), never frames. The machine is
//! stepped once per game tick with the trigger state and the number of reload presses since the
//! last tick, and appends [`SimEvent`]s for the caller to act on (spawn bullets, play sounds, log).
//!
//! M6: bolt / pump / lever rechamber (`boltAction 1`: after a shot that leaves rounds in the magazine the gun is not
//! chambered until `rechamber_s` has passed, starting `fireTime` after the shot), shell-by-shell reloads
//! (`segmentedReload 1`: start, loops that each commit their shells, end; a fire press finishes the segment, plays the
//! end and fires), Double Barrel (one shot per press, or both barrels with `both_barrels`), a reload press during a
//! burst waits for the burst to end, and a `hold` input (sprint / switch) that blocks fire and rechamber starts.

use crate::weapons::{FireMode, WeaponSpec};

/// M7 A2: the largest lag of the fire schedule behind the tick time, as a fraction of fireTime (shots are never
/// closer than `1 - CARRY_MAX` fireTimes).
pub const CARRY_MAX: f64 = 0.15;

/// Reload sound cue positions as fractions of the reload duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueFractions {
    pub lift: f64,
    pub clipout: f64,
    pub clipin: f64,
    /// Empty reloads only.
    pub chamber: f64,
}

impl Default for CueFractions {
    fn default() -> Self {
        CueFractions { lift: 0.0, clipout: 0.15, clipin: 0.55, chamber: 0.80 }
    }
}

/// Shell-by-shell reload timing (docs/M6_DESIGN.md section 6.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegParams {
    pub start_s: f64,
    pub start_add: u32,
    pub start_add_s: f64,
    pub loop_s: f64,
    pub add: u32,
    pub add_s: f64,
    pub end_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimParams {
    pub fire_mode: FireMode,
    pub fire_time_s: f64,
    pub clip_size: u32,
    pub max_ammo: u32,
    pub reload_s: f64,
    pub reload_empty_s: f64,
    pub auto_reload_on_empty: bool,
    pub max_shots_per_tick: u32,
    pub cues: CueFractions,
    /// M6: rechamber time (boltAction 1 only); `None` = no rechamber.
    pub rechamber_s: Option<f64>,
    /// M6: true = the rechamber starts at the shot ("overlap"), false = after fireTime ("after_fire", default) [I].
    pub rechamber_overlap: bool,
    /// M6: shell-by-shell reload; `None` = whole magazine.
    pub segmented: Option<SegParams>,
    /// M6 Double Barrel: one press fires both barrels [I].
    pub both_barrels: bool,
    /// The M2 fraction-timed reload cues (the UMP45 path); in M6 the clips' notetracks carry the sounds instead.
    pub fraction_cues: bool,
}

impl SimParams {
    pub fn from_spec(w: &WeaponSpec, auto_reload_on_empty: bool, max_shots_per_tick: u32, cues: CueFractions) -> Self {
        SimParams {
            fire_mode: w.fire_mode,
            fire_time_s: w.fire_time_s,
            clip_size: w.clip_size,
            max_ammo: w.max_ammo,
            reload_s: w.reload_s,
            reload_empty_s: w.reload_empty_s,
            auto_reload_on_empty,
            max_shots_per_tick: max_shots_per_tick.max(1),
            cues,
            rechamber_s: None,
            rechamber_overlap: false,
            segmented: None,
            both_barrels: false,
            fraction_cues: true,
        }
    }

    /// M6: the parameters of one arsenal gun (no fraction cues: the clips' notetracks carry the sounds).
    pub fn from_gun(g: &crate::arsenal::GunSpec, auto_reload_on_empty: bool, max_shots_per_tick: u32, rechamber_overlap: bool, both_barrels: bool) -> Self {
        use crate::arsenal::ReloadKind;
        let (reload_s, reload_empty_s, segmented) = match g.reload {
            ReloadKind::Mag { reload_s, empty_s } => (reload_s, empty_s, None),
            ReloadKind::Segmented { start_s, start_add, start_add_s, loop_s, add, add_s, end_s } => {
                (loop_s, loop_s, Some(SegParams { start_s, start_add, start_add_s, loop_s, add, add_s, end_s }))
            }
        };
        SimParams {
            fire_mode: g.fire,
            fire_time_s: g.fire_time_s,
            clip_size: g.clip,
            max_ammo: g.max_ammo,
            reload_s,
            reload_empty_s,
            auto_reload_on_empty,
            max_shots_per_tick: max_shots_per_tick.max(1),
            cues: CueFractions::default(),
            rechamber_s: g.rechamber.map(|r| r.time_s),
            rechamber_overlap,
            segmented,
            both_barrels,
            fraction_cues: false,
        }
    }
}

/// Sounds the simulation asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SfxCue {
    Fire,
    ReloadLift,
    ReloadClipOut,
    ReloadClipIn,
    ReloadChamber,
}

impl SfxCue {
    pub fn name(self) -> &'static str {
        match self {
            SfxCue::Fire => "fire",
            SfxCue::ReloadLift => "reload_lift",
            SfxCue::ReloadClipOut => "reload_clipout",
            SfxCue::ReloadClipIn => "reload_clipin",
            SfxCue::ReloadChamber => "reload_chamber",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadIgnored {
    Full,
    NoReserve,
}

/// The phases of a shell-by-shell reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegPhase {
    Start,
    Loop,
    End,
}

impl SegPhase {
    pub fn label(self) -> &'static str {
        match self {
            SegPhase::Start => "start",
            SegPhase::Loop => "loop",
            SegPhase::End => "end",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimEvent {
    /// One round fired; the caller spawns the bullet and plays the fire sound.
    Shot { mag_after: u32 },
    ReloadStart { empty: bool, duration_s: f64 },
    ReloadDone { added: u32, mag: u32, reserve: u32 },
    ReloadCancelled,
    ReloadIgnored(ReloadIgnored),
    /// Trigger pressed with nothing to fire (logged once per press).
    DryFire,
    Cue(SfxCue),
    /// M6: the rechamber (bolt / pump / lever) began; it lasts `duration_s`.
    RechamberStart { duration_s: f64 },
    RechamberDone,
    /// M7 A3: a running rechamber was cancelled (switch, sprint, mode off, AC-130, nuke); it restarts later.
    RechamberCancelled,
    /// M6: a segment of a shell-by-shell reload began (`duration_s` = its length).
    Segment { phase: SegPhase, duration_s: f64 },
    /// M6: shells committed by a segment.
    ShellAdded { added: u32, mag: u32, reserve: u32 },
    /// M6: a burst ended (`fired` rounds, first to last shot `span_s`).
    BurstDone { fired: u32, span_s: f64 },
    /// M6: a reload press waits for the burst in flight.
    ReloadDeferred,
}

#[derive(Clone, Copy, Debug)]
struct Reload {
    start: f64,
    until: f64,
    empty: bool,
    next_cue: usize,
}

#[derive(Clone, Copy, Debug)]
struct Seg {
    phase: SegPhase,
    until: f64,
    add_at: f64,
    add_n: u32,
    added: bool,
    want_fire: bool,
    start: f64,
    est_end: f64,
    total_added: u32,
}

/// The per-gun state machine.
#[derive(Clone, Debug)]
pub struct WeaponSim {
    p: SimParams,
    mag: u32,
    reserve: u32,
    next_shot: f64,
    reload: Option<Reload>,
    prev_trigger: bool,
    burst_left: u32,
    cues: Vec<(f64, SfxCue, bool)>,
    seg: Option<Seg>,
    chambered: bool,
    /// The rechamber starts at this time (after fireTime) unless cancelled.
    rechamber_due: Option<f64>,
    /// (start, until) of a rechamber in progress.
    rechamber: Option<(f64, f64)>,
    pending_reload: bool,
    pending_shot: bool,
    burst_fired: u32,
    burst_t0: f64,
    burst_last: f64,
    hold: bool,
}

impl WeaponSim {
    /// Starts with a full magazine and full reserve (`clipSize` / `maxAmmo`).
    pub fn new(p: SimParams) -> Self {
        let mut cues = vec![
            (p.cues.lift, SfxCue::ReloadLift, false),
            (p.cues.clipout, SfxCue::ReloadClipOut, false),
            (p.cues.clipin, SfxCue::ReloadClipIn, false),
            (p.cues.chamber, SfxCue::ReloadChamber, true),
        ];
        cues.sort_by(|a, b| a.0.total_cmp(&b.0));
        WeaponSim {
            p,
            mag: p.clip_size,
            reserve: p.max_ammo,
            next_shot: f64::NEG_INFINITY,
            reload: None,
            prev_trigger: false,
            burst_left: 0,
            cues,
            seg: None,
            chambered: true,
            rechamber_due: None,
            rechamber: None,
            pending_reload: false,
            pending_shot: false,
            burst_fired: 0,
            burst_t0: 0.0,
            burst_last: 0.0,
            hold: false,
        }
    }

    pub fn params(&self) -> &SimParams {
        &self.p
    }

    /// Applies new parameters (config hot reload) keeping ammo and any reload in progress.
    pub fn set_params(&mut self, p: SimParams) {
        let old = self.clone();
        *self = WeaponSim::new(p);
        self.mag = old.mag.min(p.clip_size);
        self.reserve = old.reserve;
        self.reload = old.reload;
        self.next_shot = old.next_shot;
        self.seg = old.seg;
        self.chambered = old.chambered;
        self.rechamber_due = old.rechamber_due;
        self.rechamber = old.rechamber;
        self.hold = old.hold;
    }

    pub fn mag(&self) -> u32 {
        self.mag
    }

    pub fn reserve(&self) -> u32 {
        self.reserve
    }

    pub fn is_reloading(&self) -> bool {
        self.reload.is_some() || self.seg.is_some()
    }

    /// M6: the current segment of a shell-by-shell reload.
    pub fn segment(&self) -> Option<SegPhase> {
        self.seg.map(|s| s.phase)
    }

    /// M6: a round is in the chamber (false between a bolt-action shot and the end of its rechamber).
    pub fn chambered(&self) -> bool {
        self.chambered
    }

    /// M6: a rechamber is running (or due).
    pub fn is_rechambering(&self) -> bool {
        self.rechamber.is_some() || self.rechamber_due.is_some()
    }

    /// M6: rounds of a burst still to fire.
    pub fn burst_in_flight(&self) -> bool {
        self.burst_left > 0
    }

    /// M6: sprint / weapon switch: no shot and no rechamber start while held.
    pub fn set_hold(&mut self, hold: bool) {
        self.hold = hold;
    }

    /// Reload progress in [0, 1], or `None` when not reloading.
    pub fn reload_progress(&self, now: f64) -> Option<f32> {
        if let Some(s) = self.seg {
            return Some((((now - s.start) / (s.est_end - s.start).max(1e-6)).clamp(0.0, 1.0)) as f32);
        }
        self.reload.map(|r| (((now - r.start) / (r.until - r.start).max(1e-6)).clamp(0.0, 1.0)) as f32)
    }

    /// Full magazine and reserve (MW2 mode enabled with `refill_on_mode_enable`).
    pub fn refill(&mut self) {
        self.refill_with(self.p.max_ammo);
    }

    /// M6: full magazine and the given reserve (`[arsenal] reserve_source`).
    pub fn refill_with(&mut self, reserve: u32) {
        self.mag = self.p.clip_size;
        self.reserve = reserve;
        self.reload = None;
        self.seg = None;
        self.burst_left = 0;
        self.burst_fired = 0;
        self.chambered = true;
        self.rechamber_due = None;
        self.rechamber = None;
        self.pending_reload = false;
        self.pending_shot = false;
    }

    /// Sets ammo directly (tests and future save/restore).
    pub fn set_ammo(&mut self, mag: u32, reserve: u32) {
        self.mag = mag.min(self.p.clip_size);
        self.reserve = reserve;
    }

    /// Cancels a reload in progress without adding ammo (MW2 mode off, world left, sprint, switch). M6: a
    /// shell-by-shell reload keeps the shells it committed; a rechamber in progress leaves the gun unchambered (it
    /// rechambers on a later step).
    pub fn cancel_reload(&mut self, out: &mut Vec<SimEvent>) {
        let had_reload = self.reload.take().is_some();
        let had_seg = self.seg.take().is_some();
        if had_reload || had_seg {
            out.push(SimEvent::ReloadCancelled);
        }
        // M7 A3: a running (or due) rechamber is cancelled with an event (run 14: silent, so the analyzer saw a
        // rechamber start without done); the gun stays unchambered and rechambers after the raise (M6 rule)
        let had_running = self.rechamber.take().is_some();
        let had_due = self.rechamber_due.take().is_some();
        if had_running || had_due {
            self.chambered = false;
            if had_running {
                out.push(SimEvent::RechamberCancelled);
            }
        }
        self.end_burst(out);
        self.burst_left = 0;
        self.prev_trigger = false;
        self.pending_reload = false;
        self.pending_shot = false;
    }

    /// Review M6: the gun may no longer fire (menu, AC-130 start, breaker, focus lost): the rest of a burst in flight
    /// is dropped instead of firing without the trigger. Returns true when a burst was cut.
    pub fn cancel_burst(&mut self, out: &mut Vec<SimEvent>) -> bool {
        if self.burst_left == 0 {
            return false;
        }
        self.end_burst(out);
        self.burst_left = 0;
        self.pending_shot = false;
        true
    }

    fn end_burst(&mut self, out: &mut Vec<SimEvent>) {
        if self.burst_fired > 0 {
            out.push(SimEvent::BurstDone { fired: self.burst_fired, span_s: self.burst_last - self.burst_t0 });
        }
        self.burst_fired = 0;
    }

    /// One tick. `trigger` is the fire button already gated by the caller (mode on, no menu...);
    /// `reload_presses` counts reload-key presses since the previous tick.
    pub fn step(&mut self, now: f64, trigger: bool, reload_presses: u32, out: &mut Vec<SimEvent>) {
        let edge = trigger && !self.prev_trigger;
        // M6 shell-by-shell reload: segments commit shells; a fire press finishes the current segment, then the end.
        if self.seg.is_some() {
            // boundaries up to now first: a press at `now` acts on the segment running now, not on one that ended
            // earlier in a long tick
            self.seg_advance(now, out);
            if self.seg.is_some() {
                if edge
                    && self.mag > 0
                    && let Some(s) = self.seg.as_mut()
                {
                    s.want_fire = true;
                }
                self.prev_trigger = trigger;
                return;
            }
        }
        // Reloading: play due cues, finish when the time is up, ignore trigger and R meanwhile.
        if self.reload.is_some() {
            self.fire_due_cues(now, out);
            if let Some(r) = self.reload
                && now >= r.until
            {
                self.reload = None;
                let add = (self.p.clip_size - self.mag).min(self.reserve);
                self.mag += add;
                self.reserve -= add;
                self.chambered = true;
                out.push(SimEvent::ReloadDone { added: add, mag: self.mag, reserve: self.reserve });
            }
        }
        if self.reload.is_some() {
            self.prev_trigger = trigger;
            return;
        }

        // M6 rechamber: due after fireTime, then rechamber_s; no start while held (sprint / switch).
        self.rechamber_tick(now, out);

        let pending_reload = std::mem::take(&mut self.pending_reload);
        if reload_presses > 0 || pending_reload {
            if self.burst_left > 0 {
                // section 6.1: a reload press during a burst waits for the burst to end
                if !pending_reload {
                    out.push(SimEvent::ReloadDeferred);
                }
                self.pending_reload = true;
            } else if self.start_reload(now, out) {
                self.prev_trigger = trigger;
                return;
            }
        }

        let can_fire = !self.hold && self.chambered && self.rechamber.is_none() && self.rechamber_due.is_none();
        let pending = std::mem::take(&mut self.pending_shot);
        self.prev_trigger = trigger;
        if can_fire {
            match self.p.fire_mode {
                FireMode::FullAuto => {
                    if trigger {
                        self.fire_due(now, u32::MAX, out);
                    }
                }
                FireMode::SingleShot => {
                    if edge || pending {
                        self.fire_due(now, 1, out);
                    }
                }
                FireMode::DoubleBarrel => {
                    if edge || pending {
                        // one barrel per press; with both_barrels both go off together (fireTime is only the
                        // minimum spacing between presses)
                        let fired = self.fire_due(now, 1, out);
                        if fired > 0 && self.p.both_barrels && self.mag > 0 {
                            self.next_shot = now;
                            self.fire_due(now, 1, out);
                        }
                    }
                }
                FireMode::Burst(n) => {
                    if (edge || pending) && self.burst_left == 0 {
                        self.burst_left = n;
                        self.burst_fired = 0;
                    }
                    if self.burst_left > 0 {
                        let fired = self.fire_due(now, self.burst_left, out);
                        if fired > 0 {
                            if self.burst_fired == 0 {
                                self.burst_t0 = now;
                            }
                            self.burst_fired += fired;
                            self.burst_last = now;
                        }
                        self.burst_left = if self.mag == 0 { 0 } else { self.burst_left - fired };
                        if self.burst_left == 0 {
                            self.end_burst(out);
                            if self.pending_reload {
                                self.pending_reload = false;
                                self.start_reload(now, out);
                                return;
                            }
                        }
                    }
                }
            }
        }

        if edge && self.mag == 0 && (self.reserve == 0 || !self.p.auto_reload_on_empty) {
            out.push(SimEvent::DryFire);
        }
        // M7 A3: never while held (switch drop / sprint): run 14 restarted the outgoing gun's reload in the same tick
        // the switch cancelled it and parked the gun mid-reload; an empty gun reloads after its raise instead.
        if self.mag == 0 && self.reserve > 0 && self.p.auto_reload_on_empty && self.burst_left == 0 && !self.hold {
            self.start_reload(now, out);
        }
    }

    fn rechamber_tick(&mut self, now: f64, out: &mut Vec<SimEvent>) {
        let Some(rs) = self.p.rechamber_s else { return };
        if let Some((_, until)) = self.rechamber
            && now >= until
        {
            self.rechamber = None;
            self.chambered = true;
            out.push(SimEvent::RechamberDone);
        }
        if let Some(due) = self.rechamber_due
            && now >= due
            && !self.hold
        {
            self.rechamber_due = None;
            self.rechamber = Some((due, due + rs));
            out.push(SimEvent::RechamberStart { duration_s: rs });
            if now >= due + rs {
                self.rechamber = None;
                self.chambered = true;
                out.push(SimEvent::RechamberDone);
            }
        }
        // a cancelled rechamber (sprint / switch) restarts once the gun is free again
        if !self.chambered && self.rechamber.is_none() && self.rechamber_due.is_none() && !self.hold && self.mag > 0 {
            self.rechamber = Some((now, now + rs));
            out.push(SimEvent::RechamberStart { duration_s: rs });
        }
    }

    /// Fires every shot that is due (capped per tick and by `limit`). Returns shots fired.
    fn fire_due(&mut self, now: f64, limit: u32, out: &mut Vec<SimEvent>) -> u32 {
        // No catch-up after a pause or a hitch (M7 A2, docs/M7_DESIGN.md 2.2): the schedule may lag `now` by at most
        // 0.15 x fireTime, so the shot after this one is never closer than 0.85 x fireTime in real time. The M2 rule
        // (reset only when more than one fireTime overdue) let run 14's P90 fire at 40.295 after a 117 ms hitch with
        // next_shot 40.243, keep next_shot 40.308 and fire again on the next tick (17 ms < fireTime 65 ms).
        self.next_shot = self.next_shot.max(now - CARRY_MAX * self.p.fire_time_s);
        let cap = self.p.max_shots_per_tick.min(limit);
        let mut n = 0;
        while now >= self.next_shot && self.mag > 0 && n < cap {
            self.mag -= 1;
            self.next_shot += self.p.fire_time_s;
            n += 1;
            out.push(SimEvent::Shot { mag_after: self.mag });
            if self.p.rechamber_s.is_some() && self.mag > 0 {
                // section 6.2: no rechamber after the last round (the reload chambers)
                self.chambered = false;
                self.rechamber_due = Some(if self.p.rechamber_overlap { now } else { now + self.p.fire_time_s });
                break;
            }
        }
        n
    }

    fn start_reload(&mut self, now: f64, out: &mut Vec<SimEvent>) -> bool {
        if self.mag >= self.p.clip_size {
            out.push(SimEvent::ReloadIgnored(ReloadIgnored::Full));
            return false;
        }
        if self.reserve == 0 {
            out.push(SimEvent::ReloadIgnored(ReloadIgnored::NoReserve));
            return false;
        }
        let empty = self.mag == 0;
        // the reload chambers the gun: a running rechamber ends here (M7 A3: with an event, never silently)
        if self.rechamber.take().is_some() {
            out.push(SimEvent::RechamberCancelled);
        }
        self.rechamber_due = None;
        self.burst_left = 0;
        if let Some(sp) = self.p.segmented {
            let need = (self.p.clip_size - self.mag).min(self.reserve);
            let after_start = need.saturating_sub(if sp.start_s > 0.0 { sp.start_add } else { 0 });
            let loops = after_start.div_ceil(sp.add.max(1)) as f64;
            let est = sp.start_s + loops * sp.loop_s + sp.end_s;
            out.push(SimEvent::ReloadStart { empty, duration_s: est });
            self.seg = Some(Seg {
                phase: SegPhase::Start,
                until: now,
                add_at: now,
                add_n: 0,
                added: true,
                want_fire: false,
                start: now,
                est_end: now + est,
                total_added: 0,
            });
            let first = if sp.start_s > 0.0 { SegPhase::Start } else { SegPhase::Loop };
            self.enter_seg(first, now, out);
            self.seg_advance(now, out);
            return true;
        }
        let dur = if empty { self.p.reload_empty_s } else { self.p.reload_s };
        self.reload = Some(Reload { start: now, until: now + dur, empty, next_cue: 0 });
        out.push(SimEvent::ReloadStart { empty, duration_s: dur });
        self.fire_due_cues(now, out);
        true
    }

    fn enter_seg(&mut self, phase: SegPhase, t0: f64, out: &mut Vec<SimEvent>) {
        let Some(sp) = self.p.segmented else { return };
        let Some(s) = self.seg.as_mut() else { return };
        let (dur, add_s, add_n) = match phase {
            SegPhase::Start => (sp.start_s, sp.start_add_s.min(sp.start_s), sp.start_add),
            SegPhase::Loop => (sp.loop_s, sp.add_s.min(sp.loop_s), sp.add.max(1)),
            SegPhase::End => (sp.end_s, 0.0, 0),
        };
        s.phase = phase;
        s.until = t0 + dur;
        s.add_at = t0 + add_s;
        s.add_n = add_n;
        s.added = add_n == 0;
        out.push(SimEvent::Segment { phase, duration_s: dur });
    }

    /// Runs every segment boundary up to `now` (several in one long tick).
    fn seg_advance(&mut self, now: f64, out: &mut Vec<SimEvent>) {
        for _ in 0..512 {
            let Some(s) = self.seg else { return };
            if !s.added && now >= s.add_at {
                let add = s.add_n.min(self.p.clip_size - self.mag).min(self.reserve);
                self.mag += add;
                self.reserve -= add;
                if let Some(sm) = self.seg.as_mut() {
                    sm.added = true;
                    sm.total_added += add;
                }
                if add > 0 {
                    out.push(SimEvent::ShellAdded { added: add, mag: self.mag, reserve: self.reserve });
                }
                continue;
            }
            if now < s.until {
                return;
            }
            let more = self.mag < self.p.clip_size && self.reserve > 0;
            // a fire press with rounds in the gun finishes the reload; with an empty gun the reload continues
            let stop = s.want_fire && self.mag > 0;
            match s.phase {
                SegPhase::Start | SegPhase::Loop if more && !stop => self.enter_seg(SegPhase::Loop, s.until, out),
                SegPhase::Start | SegPhase::Loop => self.enter_seg(SegPhase::End, s.until, out),
                SegPhase::End => {
                    self.seg = None;
                    self.chambered = true;
                    if s.want_fire && self.mag > 0 {
                        self.pending_shot = true;
                    }
                    out.push(SimEvent::ReloadDone { added: s.total_added, mag: self.mag, reserve: self.reserve });
                }
            }
        }
    }

    fn fire_due_cues(&mut self, now: f64, out: &mut Vec<SimEvent>) {
        if !self.p.fraction_cues {
            return;
        }
        let Some(r) = self.reload.as_mut() else { return };
        let dur = r.until - r.start;
        while r.next_cue < self.cues.len() {
            let (frac, cue, empty_only) = self.cues[r.next_cue];
            if now < r.start + frac * dur {
                break;
            }
            r.next_cue += 1;
            if !empty_only || r.empty {
                out.push(SimEvent::Cue(cue));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ump_params() -> SimParams {
        SimParams {
            fire_mode: FireMode::FullAuto,
            fire_time_s: 0.09,
            clip_size: 32,
            max_ammo: 192,
            reload_s: 2.5,
            reload_empty_s: 3.03,
            auto_reload_on_empty: true,
            max_shots_per_tick: 2,
            cues: CueFractions::default(),
            rechamber_s: None,
            rechamber_overlap: false,
            segmented: None,
            both_barrels: false,
            fraction_cues: true,
        }
    }

    fn shots(ev: &[SimEvent]) -> usize {
        ev.iter().filter(|e| matches!(e, SimEvent::Shot { .. })).count()
    }

    /// Runs the trigger held over the given tick times; returns the time of each shot.
    fn run_held(sim: &mut WeaponSim, times: &[f64]) -> Vec<f64> {
        let mut at = Vec::new();
        for &t in times {
            let mut ev = Vec::new();
            sim.step(t, true, 0, &mut ev);
            let n = shots(&ev);
            assert!(n <= 2, "at most max_shots_per_tick per tick ({n} at {t})");
            at.extend(std::iter::repeat_n(t, n));
        }
        at
    }

    #[test]
    fn cadence_60hz_one_second_is_12_shots() {
        let mut sim = WeaponSim::new(ump_params());
        let times: Vec<f64> = (0..=60).map(|k| k as f64 / 60.0).collect();
        assert_eq!(run_held(&mut sim, &times).len(), 12); // due at 0, 0.09, ..., 0.99
    }

    #[test]
    fn cadence_30hz_never_beats_085_fire_time() {
        // M7 A2: at 30 Hz the 90 ms UMP45 can no longer alternate 67 / 100 ms intervals (67 < 0.85 x 90): it fires
        // every 3rd tick (100 ms), 11 shots in the second instead of the M2 12 (60 Hz stays exact, test above)
        let mut sim = WeaponSim::new(ump_params());
        let times: Vec<f64> = (0..=30).map(|k| k as f64 / 30.0).collect();
        let at = run_held(&mut sim, &times);
        assert_eq!(at.len(), 11);
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 0.85 * 0.09 - 1e-9, "{w:?}");
        }
    }

    // ---- M7 (docs/M7_DESIGN.md 2.2 / 2.3) ------------------------------------------------------------------------

    #[test]
    fn cadence_clamp_after_hitch_every_fire_time() {
        // ticks at t, t + 1.8 ft (a hitch), t + 1.8 ft + 1/60 s: the interval never drops below 0.85 x fireTime
        for k in 0..60 {
            let ft = 0.040 + 0.005 * k as f64; // 40 .. 335 ms (every MW2 MP fireTime of the arsenal is inside)
            let mut sim = WeaponSim::new(SimParams { fire_time_s: ft, max_shots_per_tick: 2, ..ump_params() });
            let mut at = Vec::new();
            let mut tick = |sim: &mut WeaponSim, t: f64| {
                let mut ev = Vec::new();
                sim.step(t, true, 0, &mut ev);
                at.extend(std::iter::repeat_n(t, shots(&ev)));
            };
            let t0 = 10.0;
            // steady 60 Hz first so the schedule carries a lag, then the hitch
            for i in 0..30 {
                tick(&mut sim, t0 + i as f64 / 60.0);
            }
            let t1 = t0 + 29.0 / 60.0;
            tick(&mut sim, t1 + 1.8 * ft);
            tick(&mut sim, t1 + 1.8 * ft + 1.0 / 60.0);
            for i in 2..20 {
                tick(&mut sim, t1 + 1.8 * ft + i as f64 / 60.0);
            }
            for w in at.windows(2) {
                assert!(w[1] - w[0] >= 0.85 * ft - 1e-9, "ft {ft}: shots at {} and {} ({:.1} ms)", w[0], w[1], (w[1] - w[0]) * 1e3);
            }
        }
    }

    #[test]
    fn cadence_clamp_p90_run14_replay() {
        // run 14: P90 (fireTime 65 ms) shot 234 at 40.178, a 117 ms hitch to 40.295 (shot 235), then 40.312
        let mut sim = WeaponSim::new(SimParams { fire_time_s: 0.065, clip_size: 50, ..ump_params() });
        let mut times: Vec<f64> = (0..=10).map(|k| 40.178 - 0.065 * 3.0 + k as f64 / 60.0).filter(|&t| t <= 40.1781).collect();
        times.extend([40.178, 40.295, 40.312, 40.329, 40.345, 40.362]);
        let at = run_held(&mut sim, &times);
        assert!(at.contains(&40.295), "{at:?}");
        assert!(!at.contains(&40.312), "the 17 ms double shot of run 14: {at:?}");
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 0.85 * 0.065 - 1e-9, "{w:?}");
        }
    }

    #[test]
    fn cadence_median_within_one_frame_every_rate() {
        // the M6 cadence promise still holds at 20 / 60 / 144 Hz for a range of fireTimes (median within one tick)
        for hz in [20.0, 60.0, 144.0] {
            for ft in [0.05, 0.065, 0.075, 0.09, 0.1, 0.13] {
                let mut sim = WeaponSim::new(SimParams { fire_time_s: ft, clip_size: 1000, max_ammo: 0, ..ump_params() });
                let mut at = Vec::new();
                for k in 0..(hz as usize * 4) {
                    let t = k as f64 / hz;
                    let mut ev = Vec::new();
                    sim.step(t, true, 0, &mut ev);
                    at.extend(std::iter::repeat_n(t, shots(&ev)));
                }
                let mut gaps: Vec<f64> = at.windows(2).map(|w| w[1] - w[0]).collect();
                gaps.sort_by(|a, b| a.total_cmp(b));
                let med = gaps[gaps.len() / 2];
                assert!((med - ft).abs() <= 1.0 / hz + 1e-9, "{hz} Hz ft {ft}: median {med}");
                assert!(gaps[0] >= 0.85 * ft - 1e-9, "{hz} Hz ft {ft}: min {}", gaps[0]);
            }
        }
    }

    #[test]
    fn reload_switch_empty_mag_no_start_during_drop() {
        // run 14: `RELOAD cancelled (switch)` then `RELOAD start empty=1` in the same tick on the outgoing gun
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(0, 100);
        let mut ev = Vec::new();
        sim.step(0.0, false, 0, &mut ev);
        assert!(sim.is_reloading(), "auto reload on empty");
        ev.clear();
        // the switch: cancel, then the drop is stepped with hold = true
        sim.cancel_reload(&mut ev);
        sim.set_hold(true);
        for k in 1..60 {
            sim.step(k as f64 / 60.0, false, 0, &mut ev);
        }
        assert_eq!(ev, vec![SimEvent::ReloadCancelled], "no restart while held: {ev:?}");
        assert!(!sim.is_reloading());
        // the raise: an empty gun reloads normally
        sim.set_hold(false);
        ev.clear();
        sim.step(2.0, false, 0, &mut ev);
        assert!(matches!(ev.first(), Some(SimEvent::ReloadStart { empty: true, .. })), "{ev:?}");
        assert_eq!(ev.iter().filter(|e| matches!(e, SimEvent::ReloadStart { .. })).count(), 1);
    }

    #[test]
    fn rechamber_cancel_emits_event() {
        let mut sim = WeaponSim::new(bolt_params(0.05, 0.8));
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        sim.step(0.06, false, 0, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::RechamberStart { .. })));
        ev.clear();
        sim.cancel_reload(&mut ev);
        assert_eq!(ev, vec![SimEvent::RechamberCancelled]);
        // a rechamber only due (not started yet) cancels without an event (no start was logged)
        let mut sim = WeaponSim::new(bolt_params(0.05, 0.8));
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        ev.clear();
        sim.cancel_reload(&mut ev);
        assert!(ev.is_empty(), "{ev:?}");
        assert!(!sim.chambered());
    }

    #[test]
    fn reload_rechamber_pairing_property() {
        // 1000 random switch / sprint / fire / reload sequences over the gun kinds: every start ends with done or
        // cancelled, never a second start while one is open
        let kinds = [ump_params(), bolt_params(0.05, 0.8), seg_params(), burst_params(0.06), SimParams { both_barrels: true, fire_mode: FireMode::DoubleBarrel, clip_size: 2, ..ump_params() }];
        let mut rng = crate::rng::Rng::new(0x4d37);
        for case in 0..1000 {
            let p = kinds[case % kinds.len()];
            let mut sim = WeaponSim::new(p);
            sim.set_ammo((rng.next_u64() % (p.clip_size as u64 + 1)) as u32, 40);
            let (mut reload_open, mut rech_open) = (false, false);
            let mut t = 0.0;
            let mut hold_until = -1.0;
            let check = |ev: &[SimEvent], reload_open: &mut bool, rech_open: &mut bool| {
                for e in ev {
                    match e {
                        SimEvent::ReloadStart { .. } => {
                            assert!(!*reload_open, "case {case}: reload start while one is open");
                            *reload_open = true;
                        }
                        SimEvent::ReloadDone { .. } | SimEvent::ReloadCancelled => {
                            assert!(*reload_open, "case {case}: {e:?} without a start");
                            *reload_open = false;
                        }
                        SimEvent::RechamberStart { .. } => {
                            assert!(!*rech_open, "case {case}: rechamber start while one is open");
                            *rech_open = true;
                        }
                        SimEvent::RechamberDone | SimEvent::RechamberCancelled => {
                            assert!(*rech_open, "case {case}: {e:?} without a start");
                            *rech_open = false;
                        }
                        _ => {}
                    }
                }
            };
            for _ in 0..400 {
                t += 1.0 / 60.0;
                let mut ev = Vec::new();
                let r = rng.next_u64() % 100;
                if r < 2 {
                    // a switch or sprint start: cancel, then held for a while
                    sim.cancel_reload(&mut ev);
                    hold_until = t + 0.3 + 0.7 * rng.next_f32() as f64;
                }
                sim.set_hold(t < hold_until);
                let trig = rng.next_u64() % 3 != 0;
                let presses = u32::from(rng.next_u64() % 50 == 0);
                sim.step(t, trig, presses, &mut ev);
                check(&ev, &mut reload_open, &mut rech_open);
            }
            // idle long enough for anything running to finish
            sim.set_hold(false);
            for k in 0..900 {
                let mut ev = Vec::new();
                sim.step(t + k as f64 / 60.0, false, 0, &mut ev);
                check(&ev, &mut reload_open, &mut rech_open);
            }
            assert!(!reload_open && !rech_open, "case {case}: still open after idling");
        }
    }

    #[test]
    fn cadence_hitch_does_not_burst() {
        let mut sim = WeaponSim::new(ump_params());
        let mut times: Vec<f64> = (0..=6).map(|k| k as f64 / 60.0).collect();
        times.push(0.1 + 0.5); // 0.5 s hitch
        times.extend((1..=6).map(|k| 0.6 + k as f64 / 60.0));
        let at = run_held(&mut sim, &times);
        let after_hitch = at.iter().filter(|&&t| (t - 0.6).abs() < 1e-9).count();
        assert_eq!(after_hitch, 1, "the hitch tick fires once, no catch-up");
    }

    #[test]
    fn cadence_irregular_ticks_never_beat_fire_time() {
        let mut sim = WeaponSim::new(ump_params());
        let mut rng = crate::rng::Rng::new(3);
        let mut t = 0.0;
        let mut times = Vec::new();
        while t < 2.5 {
            times.push(t);
            t += 0.004 + 0.06 * rng.next_f32() as f64;
        }
        let at = run_held(&mut sim, &times);
        assert!(at.len() <= (2.5f64 / 0.09) as usize + 1);
        assert!(at.len() >= 20);
    }

    #[test]
    fn cadence_tapping_cannot_beat_fire_time() {
        let mut sim = WeaponSim::new(ump_params());
        let mut at = Vec::new();
        let mut t = 0.0;
        // press 20 ms, release 10 ms, repeatedly (taps every 30 ms)
        for i in 0..200 {
            let down = i % 3 != 2;
            let mut ev = Vec::new();
            sim.step(t, down, 0, &mut ev);
            at.extend(std::iter::repeat_n(t, shots(&ev)));
            t += 0.01;
        }
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 0.09 - 1e-9, "shots {} and {} too close", w[0], w[1]);
        }
        assert!(at.len() > 5);
    }

    #[test]
    fn cadence_single_shot_needs_new_press() {
        let mut p = ump_params();
        p.fire_mode = FireMode::SingleShot;
        let mut sim = WeaponSim::new(p);
        let times: Vec<f64> = (0..60).map(|k| k as f64 / 60.0).collect();
        assert_eq!(run_held(&mut sim, &times).len(), 1, "holding fires once");
        let mut ev = Vec::new();
        sim.step(1.0, false, 0, &mut ev);
        sim.step(1.02, true, 0, &mut ev);
        assert_eq!(shots(&ev), 1);
    }

    #[test]
    fn ammo_32_shots_empty_then_auto_reload_empty() {
        let mut sim = WeaponSim::new(ump_params());
        let mut t = 0.0;
        let mut total = 0;
        let mut start = None;
        while total < 32 {
            let mut ev = Vec::new();
            sim.step(t, true, 0, &mut ev);
            total += shots(&ev);
            if let Some(SimEvent::ReloadStart { empty, duration_s }) =
                ev.iter().find(|e| matches!(e, SimEvent::ReloadStart { .. })).copied()
            {
                assert!(empty);
                assert!((duration_s - 3.03).abs() < 1e-9);
                start = Some(t);
            }
            t += 1.0 / 60.0;
        }
        assert_eq!(sim.mag(), 0);
        let start = start.expect("auto reload started on the 32nd shot");
        assert!(sim.is_reloading());
        // no firing while reloading
        let mut ev = Vec::new();
        sim.step(start + 1.0, true, 0, &mut ev);
        assert_eq!(shots(&ev), 0);
        sim.step(start + 3.0, false, 0, &mut ev);
        assert!(sim.is_reloading());
        sim.step(start + 3.031, false, 0, &mut ev);
        assert!(!sim.is_reloading());
        assert_eq!((sim.mag(), sim.reserve()), (32, 160));
        assert!(ev.contains(&SimEvent::ReloadDone { added: 32, mag: 32, reserve: 160 }));
    }

    #[test]
    fn ammo_partial_reload_takes_reload_time() {
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(10, 192);
        let mut ev = Vec::new();
        sim.step(5.0, false, 1, &mut ev);
        assert!(ev.contains(&SimEvent::ReloadStart { empty: false, duration_s: 2.5 }));
        sim.step(7.49, false, 0, &mut ev);
        assert_eq!(sim.mag(), 10);
        sim.step(7.5, false, 0, &mut ev);
        assert_eq!((sim.mag(), sim.reserve()), (32, 170));
    }

    #[test]
    fn ammo_short_reserve_partial_fill() {
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(10, 5);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        sim.step(2.6, false, 0, &mut ev);
        assert_eq!((sim.mag(), sim.reserve()), (15, 0));
    }

    #[test]
    fn ammo_reload_ignored_when_full_or_no_reserve() {
        let mut sim = WeaponSim::new(ump_params());
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        assert_eq!(ev, vec![SimEvent::ReloadIgnored(ReloadIgnored::Full)]);
        sim.set_ammo(3, 0);
        ev.clear();
        sim.step(0.1, false, 1, &mut ev);
        assert_eq!(ev, vec![SimEvent::ReloadIgnored(ReloadIgnored::NoReserve)]);
    }

    #[test]
    fn ammo_empty_without_reserve_dry_fires_once_per_press() {
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(0, 0);
        let mut ev = Vec::new();
        for k in 0..10 {
            sim.step(k as f64 * 0.016, true, 0, &mut ev);
        }
        assert_eq!(ev.iter().filter(|e| **e == SimEvent::DryFire).count(), 1);
        assert_eq!(shots(&ev), 0);
        assert!(!sim.is_reloading());
    }

    #[test]
    fn ammo_mode_off_cancels_reload_without_ammo() {
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(5, 100);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        assert!(sim.is_reloading());
        sim.cancel_reload(&mut ev);
        assert!(!sim.is_reloading());
        assert!(ev.contains(&SimEvent::ReloadCancelled));
        assert_eq!((sim.mag(), sim.reserve()), (5, 100));
        sim.refill();
        assert_eq!((sim.mag(), sim.reserve()), (32, 192));
    }

    #[test]
    fn ammo_set_params_keeps_state() {
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(7, 50);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        let mut p = ump_params();
        p.max_shots_per_tick = 1;
        sim.set_params(p);
        assert!(sim.is_reloading());
        assert_eq!((sim.mag(), sim.reserve()), (7, 50));
        assert_eq!(sim.params().max_shots_per_tick, 1);
    }

    #[test]
    fn ammo_reload_cues_at_fractions() {
        // partial reload (2.5 s): lift 0, clipout 0.375, clipin 1.375, no chamber
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(10, 100);
        let mut cue_times = Vec::new();
        let mut t = 0.0;
        while t <= 2.6 {
            let mut ev = Vec::new();
            sim.step(t, false, if t == 0.0 { 1 } else { 0 }, &mut ev);
            for e in ev {
                if let SimEvent::Cue(c) = e {
                    cue_times.push((c, t));
                }
            }
            t += 0.001;
        }
        let names: Vec<SfxCue> = cue_times.iter().map(|c| c.0).collect();
        assert_eq!(names, vec![SfxCue::ReloadLift, SfxCue::ReloadClipOut, SfxCue::ReloadClipIn]);
        assert!(cue_times[0].1 < 1e-9);
        assert!((cue_times[1].1 - 0.375).abs() < 0.0015);
        assert!((cue_times[2].1 - 1.375).abs() < 0.0015);

        // empty reload (3.03 s) adds the chamber cue at 0.80 * 3.03
        let mut sim = WeaponSim::new(ump_params());
        sim.set_ammo(0, 100);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        sim.step(2.42, false, 0, &mut ev);
        assert!(!ev.contains(&SimEvent::Cue(SfxCue::ReloadChamber)));
        sim.step(2.425, false, 0, &mut ev);
        assert!(ev.contains(&SimEvent::Cue(SfxCue::ReloadChamber)));
    }

    // ---- M6 (docs/M6_DESIGN.md sections 6.1-6.3; made-up timings unless the name says otherwise) ----------------

    fn burst_params(ft: f64) -> SimParams {
        SimParams { fire_mode: FireMode::Burst(3), fire_time_s: ft, clip_size: 30, max_ammo: 90, ..ump_params() }
    }

    /// Steps at `hz` from t0 for `secs` with the trigger function; returns shot times.
    fn run(sim: &mut WeaponSim, t0: f64, secs: f64, hz: f64, trig: impl Fn(f64) -> bool, ev_all: &mut Vec<SimEvent>) -> Vec<f64> {
        let mut at = Vec::new();
        let n = (secs * hz).round() as usize;
        for k in 0..=n {
            let t = t0 + k as f64 / hz;
            let mut ev = Vec::new();
            sim.step(t, trig(t - t0), 0, &mut ev);
            at.extend(std::iter::repeat_n(t, shots(&ev)));
            ev_all.extend(ev);
        }
        at
    }

    #[test]
    fn fire_mode_burst_three_rounds_at_every_rate() {
        for hz in [20.0, 60.0, 144.0] {
            // held trigger: exactly one burst of 3, spaced >= fireTime
            let mut sim = WeaponSim::new(burst_params(0.06));
            let mut ev = Vec::new();
            let at = run(&mut sim, 0.0, 1.0, hz, |_| true, &mut ev);
            assert_eq!(at.len(), 3, "{hz} Hz: one burst while held");
            // M7 A2: each round comes 0.85..1 fireTime + one tick after the previous one (the schedule may lag the
            // tick by 0.15 fireTime at most; at 60 Hz and above the burst still spans (n - 1) fireTime + a tick)
            for w in at.windows(2) {
                let gap = w[1] - w[0];
                assert!(gap >= 0.85 * 0.06 - 1e-9 && gap <= 0.06 + 1.0 / hz + 1e-9, "{hz} Hz: gap {gap}");
            }
            if hz >= 60.0 {
                assert!(at[2] - at[0] <= 2.0 * 0.06 + 1.0 / hz + 1e-9, "{hz} Hz: span {}", at[2] - at[0]);
            }
            assert!(ev.iter().any(|e| matches!(e, SimEvent::BurstDone { fired: 3, .. })));
            // a 10 ms tap still fires 3 rounds
            let mut sim = WeaponSim::new(burst_params(0.06));
            let at = run(&mut sim, 0.0, 1.0, hz, |t| t < 0.010, &mut Vec::new());
            assert_eq!(at.len(), 3, "{hz} Hz: a tap completes the burst");
        }
    }

    #[test]
    fn fire_mode_burst_cancelled_when_gun_blocked() {
        // review M6: a pause menu / AC-130 start after the first round: the remaining 2 rounds never fire
        let mut sim = WeaponSim::new(burst_params(0.06));
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        assert_eq!(shots(&ev), 1);
        assert!(sim.burst_in_flight());
        ev.clear();
        assert!(sim.cancel_burst(&mut ev));
        assert!(ev.iter().any(|e| matches!(e, SimEvent::BurstDone { fired: 1, .. })));
        assert!(!sim.burst_in_flight());
        ev.clear();
        for k in 1..20 {
            sim.step(k as f64 * 0.02, false, 0, &mut ev);
        }
        assert_eq!(shots(&ev), 0, "no round fires without the trigger after the cancel");
        assert!(!sim.cancel_burst(&mut ev), "nothing to cancel");
        // a new press fires a full burst again
        ev.clear();
        let at = run(&mut sim, 1.0, 0.5, 60.0, |_| true, &mut ev);
        assert_eq!(at.len(), 3);
    }

    #[test]
    fn fire_mode_burst_reload_deferred() {
        let mut sim = WeaponSim::new(burst_params(0.06));
        sim.set_ammo(20, 90);
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        assert_eq!(shots(&ev), 1);
        ev.clear();
        sim.step(0.03, true, 1, &mut ev);
        assert!(ev.contains(&SimEvent::ReloadDeferred), "R during a burst waits");
        assert!(!sim.is_reloading());
        ev.clear();
        sim.step(0.07, true, 0, &mut ev);
        sim.step(0.13, true, 0, &mut ev);
        assert_eq!(shots(&ev), 2, "the burst completes");
        assert!(sim.is_reloading(), "then the reload starts");
        assert!(ev.iter().any(|e| matches!(e, SimEvent::ReloadStart { .. })));
    }

    #[test]
    fn fire_mode_double_barrel() {
        let p = SimParams { fire_mode: FireMode::DoubleBarrel, fire_time_s: 0.01, clip_size: 2, max_ammo: 40, ..ump_params() };
        let mut sim = WeaponSim::new(p);
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        sim.step(0.005, true, 0, &mut ev);
        assert_eq!(shots(&ev), 1, "one barrel per press");
        sim.step(0.02, false, 0, &mut ev);
        sim.step(0.03, true, 0, &mut ev);
        assert_eq!(shots(&ev), 2, "second press, second barrel");
        // both_barrels: one press fires both
        let mut sim = WeaponSim::new(SimParams { both_barrels: true, ..p });
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        assert_eq!(shots(&ev), 2);
    }

    fn bolt_params(ft: f64, rech: f64) -> SimParams {
        SimParams { fire_mode: FireMode::SingleShot, fire_time_s: ft, clip_size: 5, max_ammo: 40, rechamber_s: Some(rech), ..ump_params() }
    }

    /// Cycle time (shot to next possible shot) with the trigger pressed every tick it can fire.
    fn cycle(p: SimParams, hz: f64) -> f64 {
        let mut sim = WeaponSim::new(p);
        let mut at = Vec::new();
        let mut down = false;
        for k in 0..(hz as usize * 5) {
            let t = k as f64 / hz;
            down = !down;
            let mut ev = Vec::new();
            sim.step(t, down, 0, &mut ev);
            at.extend(std::iter::repeat_n(t, shots(&ev)));
            if at.len() >= 2 {
                break;
            }
        }
        at[1] - at[0]
    }

    #[test]
    fn rechamber_cycle_times() {
        // the design's cycles: fireTime + rechamberTime (0.05 + 0.866, 0.47 + 0.467, 0.43 + 1.0), +-2 ticks at 240 Hz
        for (ft, rt, want) in [(0.05, 0.866, 0.916), (0.47, 0.467, 0.937), (0.43, 1.0, 1.43)] {
            let c = cycle(bolt_params(ft, rt), 240.0);
            assert!((c - want).abs() <= 2.0 / 240.0 + 1e-9, "cycle {c} vs {want}");
        }
    }

    #[test]
    fn rechamber_events_last_round_and_cancel() {
        let mut sim = WeaponSim::new(bolt_params(0.05, 0.8));
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        assert!(!sim.chambered());
        sim.step(0.06, false, 0, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::RechamberStart { .. })));
        sim.step(0.5, true, 0, &mut ev);
        assert_eq!(shots(&ev), 1, "no shot while rechambering");
        // sprint / switch cancels: unchambered, held -> no restart; released -> rechamber again
        sim.cancel_reload(&mut ev);
        assert!(!sim.chambered() && !sim.is_rechambering());
        sim.set_hold(true);
        ev.clear();
        sim.step(1.0, false, 0, &mut ev);
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::RechamberStart { .. })), "held: no rechamber");
        sim.set_hold(false);
        sim.step(1.1, false, 0, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::RechamberStart { .. })), "rechamber after the raise");
        sim.step(2.0, false, 0, &mut ev);
        assert!(sim.chambered());
        // the last round: no rechamber
        let mut sim = WeaponSim::new(bolt_params(0.05, 0.8));
        sim.set_ammo(1, 10);
        let mut ev = Vec::new();
        sim.step(0.0, true, 0, &mut ev);
        assert!(sim.chambered() && !sim.is_rechambering());
        assert!(sim.is_reloading(), "auto reload on empty");
    }

    fn seg_params() -> SimParams {
        // the design's SPAS total: 1.0 start (1 shell), 0.667 loops, 1.0 end
        SimParams {
            fire_mode: FireMode::SingleShot,
            fire_time_s: 0.47,
            clip_size: 8,
            max_ammo: 64,
            reload_s: 0.667,
            reload_empty_s: 0.667,
            segmented: Some(SegParams { start_s: 1.0, start_add: 1, start_add_s: 0.5, loop_s: 0.667, add: 1, add_s: 0.667, end_s: 1.0 }),
            ..ump_params()
        }
    }

    #[test]
    fn shell_reload_spas_from_empty_total_time() {
        for hz in [30.0, 60.0, 144.0] {
            let mut sim = WeaponSim::new(seg_params());
            sim.set_ammo(0, 64);
            let mut ev = Vec::new();
            sim.step(0.0, false, 1, &mut ev);
            let mut t = 0.0;
            while sim.is_reloading() && t < 20.0 {
                t += 1.0 / hz;
                sim.step(t, false, 0, &mut ev);
            }
            let want = 1.0 + 7.0 * 0.667 + 1.0;
            assert!((t - want).abs() <= 1.0 / hz + 1e-9, "{hz} Hz: {t} vs {want}");
            assert_eq!((sim.mag(), sim.reserve()), (8, 56));
            let shells = ev.iter().filter(|e| matches!(e, SimEvent::ShellAdded { .. })).count();
            assert_eq!(shells, 8, "one shell per segment, committed as it goes");
        }
    }

    #[test]
    fn shell_reload_fire_interrupt_and_cancel() {
        let mut sim = WeaponSim::new(seg_params());
        sim.set_ammo(2, 64);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        sim.step(1.2, true, 0, &mut ev); // press during the first loop (mag 3 after the start shell)
        let mut t = 1.2;
        let mut shot_t = None;
        while t < 5.0 && shot_t.is_none() {
            t += 1.0 / 60.0;
            let mut e = Vec::new();
            sim.step(t, true, 0, &mut e);
            if shots(&e) > 0 {
                shot_t = Some(t);
            }
            ev.extend(e);
        }
        // the loop finishes (its shell counts), then the end (1.0 s), then the shot
        let st = shot_t.expect("the press fires after the end segment");
        assert!((st - (1.0 + 0.667 + 1.0)).abs() <= 1.0 / 60.0 + 1e-9, "{st}");
        assert_eq!(sim.mag(), 3, "4 rounds (2 + start + loop) - the shot");
        // cancel keeps committed shells
        let mut sim = WeaponSim::new(seg_params());
        sim.set_ammo(0, 64);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        sim.step(1.5, false, 0, &mut ev);
        let m = sim.mag();
        sim.cancel_reload(&mut ev);
        assert_eq!(sim.mag(), m);
        assert!(m >= 1 && !sim.is_reloading());
    }

    #[test]
    fn shell_reload_add_time_clamped_and_no_start_segment() {
        // add time after the loop end is clamped to the loop end; start 0 goes straight into the loops
        let mut p = seg_params();
        p.segmented = Some(SegParams { start_s: 0.0, start_add: 0, start_add_s: 0.0, loop_s: 0.5, add: 1, add_s: 0.5, end_s: 0.3 });
        let mut sim = WeaponSim::new(p);
        sim.set_ammo(6, 10);
        let mut ev = Vec::new();
        sim.step(0.0, false, 1, &mut ev);
        sim.step(0.49, false, 0, &mut ev);
        assert_eq!(sim.mag(), 6);
        sim.step(0.5, false, 0, &mut ev);
        assert_eq!(sim.mag(), 7);
        sim.step(1.0, false, 0, &mut ev);
        sim.step(1.3, false, 0, &mut ev);
        assert_eq!(sim.mag(), 8);
        assert!(!sim.is_reloading());
    }
}
