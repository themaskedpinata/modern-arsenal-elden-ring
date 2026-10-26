//! First-person enable/handback state machine (docs/M3B_DESIGN.md section 5). Pure.
//!
//! `Off -> Arming(n) -> On` after `arm_ticks` consecutive ticks without any trigger; `On -> Off` in the same tick as
//! any trigger, with ONE reason chosen by a fixed priority ([`Reason`] order). Transient reasons re-arm by
//! themselves; `f5` / `f6` / `config` wait for the user (their trigger stays active until the user acts).
//!
//! Edge-based signals ([`EdgeWatch`], [`FadeWatch`]) compare against a baseline taken when the machine turns On, so
//! an unknown idle value (a talk id that keeps its last value, a fade plate stuck at alpha 1) can never block or
//! flap first person: only a CHANGE while On hands back.

/// Handback reasons, in priority order (the first active one is reported).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    Panic,
    ReadFail,
    CamPersist,
    Loading,
    Death,
    /// M4: the AC-130 ride (Starting/Active) hands first person back; re-arms after the ride.
    Ac130,
    /// M7: the nuke's player-death prep hands first person back (re-arms after the respawn).
    Nuke,
    Cutscene,
    Torrent,
    Ladder,
    Throw,
    Talk,
    Menu,
    CamGate,
    F5,
    F6,
    Config,
}

impl Reason {
    pub const ALL: [Reason; 17] = [
        Reason::Panic,
        Reason::ReadFail,
        Reason::CamPersist,
        Reason::Loading,
        Reason::Death,
        Reason::Ac130,
        Reason::Nuke,
        Reason::Cutscene,
        Reason::Torrent,
        Reason::Ladder,
        Reason::Throw,
        Reason::Talk,
        Reason::Menu,
        Reason::CamGate,
        Reason::F5,
        Reason::F6,
        Reason::Config,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Reason::Panic => "panic",
            Reason::ReadFail => "read_fail",
            Reason::CamPersist => "cam_persist",
            Reason::Loading => "loading",
            Reason::Death => "death",
            Reason::Ac130 => "ac130",
            Reason::Nuke => "nuke",
            Reason::Cutscene => "cutscene",
            Reason::Torrent => "torrent",
            Reason::Ladder => "ladder",
            Reason::Throw => "throw",
            Reason::Talk => "talk",
            Reason::Menu => "menu",
            Reason::CamGate => "cam_gate",
            Reason::F5 => "f5",
            Reason::F6 => "f6",
            Reason::Config => "config",
        }
    }

    /// Reasons that need the user to act before first person comes back (everything else re-arms by itself).
    pub fn waits_for_user(self) -> bool {
        matches!(self, Reason::F5 | Reason::F6 | Reason::Config)
    }
}

/// This tick's trigger inputs (true = trigger active). `Panic` is not an input: the panic path calls
/// [`FpMachine::force_off`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Triggers {
    pub read_fail: bool,
    pub cam_persist: bool,
    pub loading: bool,
    pub dead: bool,
    /// M4: an AC-130 ride is starting or running.
    pub ac130: bool,
    /// M7: the nuke's player-death prep.
    pub nuke: bool,
    pub cutscene: bool,
    pub riding: bool,
    pub ladder: bool,
    pub throw: bool,
    pub talk: bool,
    pub menu: bool,
    pub cam_invalid: bool,
    pub mode_off: bool,
    pub unwanted: bool,
    pub disabled: bool,
}

impl Triggers {
    /// The highest-priority active trigger.
    pub fn first(&self) -> Option<Reason> {
        let pairs = [
            (self.read_fail, Reason::ReadFail),
            (self.cam_persist, Reason::CamPersist),
            (self.loading, Reason::Loading),
            (self.dead, Reason::Death),
            (self.ac130, Reason::Ac130),
            (self.nuke, Reason::Nuke),
            (self.cutscene, Reason::Cutscene),
            (self.riding, Reason::Torrent),
            (self.ladder, Reason::Ladder),
            (self.throw, Reason::Throw),
            (self.talk, Reason::Talk),
            (self.menu, Reason::Menu),
            (self.cam_invalid, Reason::CamGate),
            (self.mode_off, Reason::F5),
            (self.unwanted, Reason::F6),
            (self.disabled, Reason::Config),
        ];
        pairs.iter().find(|(on, _)| *on).map(|(_, r)| *r)
    }

    /// A Triggers value with exactly the trigger behind `r` active (`Panic` -> none).
    pub fn only(r: Reason) -> Triggers {
        let mut t = Triggers::default();
        match r {
            Reason::Panic => {}
            Reason::ReadFail => t.read_fail = true,
            Reason::CamPersist => t.cam_persist = true,
            Reason::Loading => t.loading = true,
            Reason::Death => t.dead = true,
            Reason::Ac130 => t.ac130 = true,
            Reason::Nuke => t.nuke = true,
            Reason::Cutscene => t.cutscene = true,
            Reason::Torrent => t.riding = true,
            Reason::Ladder => t.ladder = true,
            Reason::Throw => t.throw = true,
            Reason::Talk => t.talk = true,
            Reason::Menu => t.menu = true,
            Reason::CamGate => t.cam_invalid = true,
            Reason::F5 => t.mode_off = true,
            Reason::F6 => t.unwanted = true,
            Reason::Config => t.disabled = true,
        }
        t
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpState {
    Off,
    Arming(u32),
    On,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpEvent {
    /// Turned on this tick: save and write.
    On,
    /// Turned off this tick: restore everything, log the reason.
    Off(Reason),
    /// While off, the blocking reason changed (log `FP blocked reason=...`).
    Blocked(Reason),
}

#[derive(Clone, Debug)]
pub struct FpMachine {
    state: FpState,
    last_block: Option<Reason>,
}

impl Default for FpMachine {
    fn default() -> Self {
        FpMachine { state: FpState::Off, last_block: None }
    }
}

impl FpMachine {
    pub fn state(&self) -> FpState {
        self.state
    }

    pub fn is_on(&self) -> bool {
        self.state == FpState::On
    }

    /// C3: why first person is off (the last handback / block reason; None while On or arming clear).
    pub fn last_block(&self) -> Option<Reason> {
        if self.is_on() { None } else { self.last_block }
    }

    pub fn step(&mut self, t: &Triggers, arm_ticks: u32) -> Option<FpEvent> {
        let r = t.first();
        match (self.state, r) {
            (FpState::On, Some(r)) => {
                self.state = FpState::Off;
                self.last_block = Some(r);
                Some(FpEvent::Off(r))
            }
            (FpState::On, None) => None,
            (_, Some(r)) => {
                self.state = FpState::Off;
                if self.last_block != Some(r) {
                    self.last_block = Some(r);
                    Some(FpEvent::Blocked(r))
                } else {
                    None
                }
            }
            (s, None) => {
                let n = match s {
                    FpState::Arming(n) => n + 1,
                    _ => 1,
                };
                if n >= arm_ticks.max(1) {
                    self.state = FpState::On;
                    self.last_block = None;
                    Some(FpEvent::On)
                } else {
                    self.state = FpState::Arming(n);
                    None
                }
            }
        }
    }

    /// Emergency off (panic path or a write failure). Returns true when it was On (restore needed).
    pub fn force_off(&mut self, r: Reason) -> bool {
        let was = self.is_on();
        self.state = FpState::Off;
        self.last_block = Some(r);
        was
    }
}

/// C6 scope peek (work/c6/scope3p.md): third-person MW2 ADS with a scoped gun asks for first person for as long as ADS
/// is held (the eye camera, the ADS-in, the scope overlay all come from the existing first-person path).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeekIn {
    /// `[first_person] tp_scope_peek` and `enabled`, `[viewmodel] enabled` (the overlay is drawn by the viewmodel).
    pub flag: bool,
    pub fp_enabled: bool,
    pub viewmodel: bool,
    /// The gun in hand has a scope overlay (`spec.ads.overlay`), no weapon switch is running.
    pub overlay_gun: bool,
    pub switch_ready: bool,
    /// MW2 mode drives in the world (`blank`), no AC-130 ride / nuke prep.
    pub live: bool,
    pub ac130: bool,
    pub nuke: bool,
    /// MW2 ADS held and allowed (not sprinting, weapon not lowered).
    pub ads_held: bool,
}

pub fn peek_wanted(i: &PeekIn) -> bool {
    i.flag && i.fp_enabled && i.viewmodel && i.overlay_gun && i.switch_ready && i.live && !i.ac130 && !i.nuke && i.ads_held
}

/// Whether a scoped ADS clears the F6 (`unwanted`) trigger this step: only while first person is off (a peek may
/// start) or the running episode began as a peek. F6 out of the user's own first person while scoped turns it off as
/// before instead of silently becoming a peek (C6 review).
pub fn peek_holds(scope_peek: bool, peek_episode: bool, on: bool) -> bool {
    scope_peek && (peek_episode || !on)
}

/// The arm ticks of this step: the short `peek_arm` only for a peek after a third person the USER chose (the last
/// block F6, or none); any other block (camera gate, talk, menu, ...) keeps the full `arm` (its anti-flap stays).
pub fn arm_ticks_for(peek: bool, wanted: bool, last_block: Option<Reason>, arm: u32, peek_arm: u32) -> u32 {
    if peek && !wanted && matches!(last_block, None | Some(Reason::F6)) { peek_arm.clamp(1, arm.max(1)) } else { arm }
}

/// A raw value that hands back when it CHANGES while first person is On (load counter, camera_mask, talk id).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeWatch<T> {
    baseline: Option<T>,
}

impl<T> Default for EdgeWatch<T> {
    fn default() -> Self {
        EdgeWatch { baseline: None }
    }
}

impl<T: Copy + PartialEq> EdgeWatch<T> {
    /// Takes the baseline (when the machine turns On). `None` (signal unavailable) disarms.
    pub fn arm(&mut self, current: Option<T>) {
        self.baseline = current;
    }

    pub fn clear(&mut self) {
        self.baseline = None;
    }

    pub fn baseline(&self) -> Option<T> {
        self.baseline
    }

    /// True when armed and the current value differs from the baseline. An unavailable value (None) is not a change.
    pub fn changed(&self, current: Option<T>) -> bool {
        matches!((self.baseline, current), (Some(b), Some(c)) if b != c)
    }
}

/// 2026-10-21 (boss fight): `CSPopupMenu.current_talk_id` also changes when a boss or NPC speaks on its own (c2130
/// mid-fight 1000 -> 204101000, its arena line 1000 -> 302001000, ui_states idx 3 shows the subtitle), and the talk
/// handback turned first person and MW2 movement off in the middle of the fight. A dialog the player opens (grace,
/// NPC) always starts with ER's interact: E, or the pad X hold that synthesizes it (both count in
/// `PRESSES_MOVE[PRESS_INTERACT]`). A talk id change is a dialog only within `window_s` of such a press (C2 host
/// grace: 3.6 s; the voice lines came 11-60 s after the last press). `window_s <= 0` = every change (before).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TalkGate {
    last_interact: f64,
}

impl Default for TalkGate {
    fn default() -> Self {
        TalkGate { last_interact: f64::NEG_INFINITY }
    }
}

impl TalkGate {
    /// `n` interact presses (E edges, pad interact holds) were taken this tick.
    pub fn interacted(&mut self, n: u32, now: f64) {
        if n > 0 {
            self.last_interact = now;
        }
    }

    /// Seconds since the last interact press (infinite before the first).
    pub fn since(&self, now: f64) -> f64 {
        now - self.last_interact
    }

    /// A talk id change at `now` is a dialog the player opened (a handback), not a voice line.
    pub fn is_dialog(&self, now: f64, window_s: f64) -> bool {
        !(window_s > 0.0) || self.since(now) <= window_s
    }
}

/// How a handback latch lets go (review M3b major 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatchParams {
    /// The value must stay back at its pre-handback baseline this long before the latch releases.
    pub holdoff_s: f64,
    /// Fallback when the value never comes back: release once it has been stable (no change) AND the menu cursor
    /// has been closed for this long. 0 = never (only a return to the baseline releases).
    pub stale_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchRelease {
    /// The value returned to its baseline and stayed there for `holdoff_s`.
    Returned,
    /// The value stayed changed but stable, with the cursor closed, for `stale_s` (a new idle value).
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LatchStep {
    /// Not latched: no effect.
    Idle,
    /// Latched: the handback cause is still active (keep the trigger on, first person must not arm).
    Hold,
    /// Released this tick.
    Released(LatchRelease),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Latched<T> {
    baseline: T,
    last: Option<T>,
    since: f64,
    stable_since: f64,
    menu_clear_since: Option<f64>,
    back_since: Option<f64>,
}

/// Keeps an edge handback (load counter, camera_mask, talk id, a fade plate) active after the edge fired, until the
/// value is back at the baseline it had while first person was On (plus a hold-off), or, if it never returns, until
/// it has been stable with the cursor closed for `stale_s`.
///
/// Without it an edge fires exactly once: the machine turns Off, the edge baseline is cleared, the trigger reads
/// false on the next tick and first person re-arms after `arm_ticks` in the middle of the cutscene/talk, taking the
/// cutscene value as its new baseline (review M3b major 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeLatch<T> {
    st: Option<Latched<T>>,
}

impl<T> Default for EdgeLatch<T> {
    fn default() -> Self {
        EdgeLatch { st: None }
    }
}

impl<T: Copy + PartialEq> EdgeLatch<T> {
    /// Latches after the edge fired: `baseline` is the value first person was On with, `current` the new value.
    pub fn latch(&mut self, baseline: T, current: Option<T>, now: f64) {
        self.st = Some(Latched { baseline, last: current, since: now, stable_since: now, menu_clear_since: None, back_since: None });
    }

    pub fn is_latched(&self) -> bool {
        self.st.is_some()
    }

    pub fn baseline(&self) -> Option<T> {
        self.st.map(|s| s.baseline)
    }

    /// Seconds since the latch was set (0 when not latched).
    pub fn age(&self, now: f64) -> f64 {
        self.st.map_or(0.0, |s| now - s.since)
    }

    /// Drops the latch without a release (signal switched off or muted).
    pub fn reset(&mut self) {
        self.st = None;
    }

    /// One tick. `current` None = the signal is unreadable this tick: it counts as "still changed" (never a return).
    pub fn step(&mut self, current: Option<T>, menu_open: bool, now: f64, p: LatchParams) -> LatchStep {
        let Some(s) = self.st.as_mut() else { return LatchStep::Idle };
        if current != s.last {
            s.last = current;
            s.stable_since = now;
        }
        if menu_open {
            s.menu_clear_since = None;
        } else if s.menu_clear_since.is_none() {
            s.menu_clear_since = Some(now);
        }
        if current == Some(s.baseline) {
            let t = *s.back_since.get_or_insert(now);
            if now - t >= p.holdoff_s {
                self.st = None;
                return LatchStep::Released(LatchRelease::Returned);
            }
            return LatchStep::Hold;
        }
        s.back_since = None;
        if p.stale_s > 0.0
            && let Some(mc) = s.menu_clear_since
            && now - s.stable_since.max(mc) >= p.stale_s
        {
            self.st = None;
            return LatchStep::Released(LatchRelease::Stale);
        }
        LatchStep::Hold
    }
}

pub const FADE_PLATES: usize = 9;

/// Cutscene/fade signal: a `CSFade` plate whose alpha ROSE above the threshold since first person turned on. Plates
/// already above it at that moment are ignored for the episode (a plate stuck at 1 cannot block first person).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FadeWatch {
    above_at_arm: Option<[bool; FADE_PLATES]>,
}

impl FadeWatch {
    pub fn arm(&mut self, alphas: &[Option<f32>; FADE_PLATES], thr: f32) {
        let mut b = [false; FADE_PLATES];
        for (i, a) in alphas.iter().enumerate() {
            b[i] = a.is_some_and(|a| a > thr);
        }
        self.above_at_arm = Some(b);
    }

    pub fn clear(&mut self) {
        self.above_at_arm = None;
    }

    /// The first plate that is above the threshold now but was not at arm time: (index, alpha).
    pub fn fired(&self, alphas: &[Option<f32>; FADE_PLATES], thr: f32) -> Option<(usize, f32)> {
        let base = self.above_at_arm?;
        alphas.iter().enumerate().find_map(|(i, a)| match a {
            Some(a) if *a > thr && a.is_finite() && !base[i] => Some((i, *a)),
            _ => None,
        })
    }
}

/// Highest alpha of the readable plates (for the probe line).
pub fn fade_max(alphas: &[Option<f32>; FADE_PLATES]) -> Option<f32> {
    alphas.iter().flatten().copied().filter(|a| a.is_finite()).reduce(f32::max)
}

/// Torrent: `ride_node->ride_state` (0 none, 3 getting on, 5 riding, 7 dismounting: inferred values) or
/// `is_mounting`. A missing ride node (None) means "not riding". `is_mounted` is a probe only (docs/M3B_DESIGN.md
/// section 5.1). `use_ride_state = false` ([first_person] use_ride_state_signal) ignores `ride_state`, in case a ride
/// node keeps a non-zero value after dismounting and would block first person for good (review M3b minor 8).
pub fn riding(ride_state: Option<u32>, is_mounting: bool, use_ride_state: bool) -> bool {
    (use_ride_state && ride_state.is_some_and(|s| s != 0)) || is_mounting
}

/// Ladder: `CSChrLadderModule.state` read as i8; -1 = none.
pub fn on_ladder(state: i8) -> bool {
    state != -1
}

/// Throw/grab: `throw_node->throw_state` read as u32 in 3..=6 (InThrowAttacker/Target, DeathAttacker/Target).
pub fn in_throw(throw_state: Option<u32>) -> bool {
    throw_state.is_some_and(|s| (3..=6).contains(&s))
}

/// `ChrCam.camera_type` DeathCam.
pub const CAMERA_TYPE_DEATH: u32 = 7;
/// `ChrIns.chr_flags1c5` bit 7.
pub const DEATH_FLAG: u8 = 0x80;

pub fn dead(hp: i32, flags1c5: u8, camera_type: Option<u32>) -> bool {
    hp <= 0 || flags1c5 & DEATH_FLAG != 0 || camera_type == Some(CAMERA_TYPE_DEATH)
}

/// 2026-10-24 (user: "can still shoot after death"): a live player sample must read alive this long, without a gap,
/// before MW2 firing re-arms after a death (the respawn's first alive reads; a single alive read never re-arms).
pub const DEATH_REARM_S: f64 = 0.5;

/// What [`DeathGate::update`] reports (one `SHOT dead gate ...` line each).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DeathEv {
    /// The player died this tick: MW2 firing is refused from here.
    On,
    /// Alive again for [`DEATH_REARM_S`] after the respawn: `dead_s` since [`DeathEv::On`], `refused` fire presses.
    Off { dead_s: f64, refused: u32 },
}

/// 2026-10-24 (user: "can still shoot after death"; host log 2026-10-24 13:27:52-13:30:44: third person, killed by
/// an enemy, 40 SHOT lines while dead: 25 `skipped:` (ammo used), 15 real bullets at 13:30:40-41). The death refusal of every MW2 fire
/// path (hitscan, launchers, the AC-130 call, the nuke key, ADS, reload, the melee / heal synth), first and third
/// person alike: on from the first tick [`dead`] holds, off only after a real player read was alive for
/// [`DEATH_REARM_S`] without a gap. A tick without a read (loading, the respawn, a read failure) decides nothing:
/// a dead player stays refused through the respawn load, an alive one is never refused by a missing read. Pure.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeathGate {
    since: Option<f64>,
    alive_since: Option<f64>,
    refused: u32,
    prev_down: bool,
}

impl DeathGate {
    /// `sample`: `Some(dead(..))` from this tick's player read, `None` = no read this tick.
    pub fn update(&mut self, now: f64, sample: Option<bool>) -> Option<DeathEv> {
        let Some(since) = self.since else {
            if sample == Some(true) {
                self.since = Some(now);
                self.alive_since = None;
                self.refused = 0;
                return Some(DeathEv::On);
            }
            return None;
        };
        match sample {
            Some(true) | None => self.alive_since = None,
            Some(false) => {
                let t0 = *self.alive_since.get_or_insert(now);
                if now - t0 >= DEATH_REARM_S {
                    let ev = DeathEv::Off { dead_s: now - since, refused: self.refused };
                    *self = Self { prev_down: self.prev_down, ..Self::default() };
                    return Some(ev);
                }
            }
        }
        None
    }

    /// MW2 firing is refused (the player is dead or not yet alive again after the respawn).
    pub fn blocked(&self) -> bool {
        self.since.is_some()
    }

    /// The raw fire button this tick. Returns the refused press count on a new press while [`Self::blocked`].
    pub fn press(&mut self, down: bool) -> Option<u32> {
        let edge = down && !self.prev_down;
        self.prev_down = down;
        if edge && self.blocked() {
            self.refused += 1;
            return Some(self.refused);
        }
        None
    }
}

/// Anti-flap for the inferred edge signals (docs/M3B_DESIGN.md section 5.1): a signal that causes `FLAP_MAX`
/// handbacks within `FLAP_WINDOW_S` is muted for the session, so a value that changes during normal play cannot
/// make first person flicker. Verified signals never go through this.
#[derive(Clone, Debug, Default)]
pub struct FlapGuard {
    hits: Vec<f64>,
    muted: bool,
}

pub const FLAP_MAX: usize = 5;
pub const FLAP_WINDOW_S: f64 = 60.0;

impl FlapGuard {
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Records one handback caused by this signal. Returns true exactly when it becomes muted.
    pub fn record(&mut self, now: f64) -> bool {
        if self.muted {
            return false;
        }
        self.hits.retain(|t| now - t < FLAP_WINDOW_S);
        self.hits.push(now);
        if self.hits.len() >= FLAP_MAX {
            self.muted = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(m: &mut FpMachine, n: u32) -> Vec<FpEvent> {
        (0..n).filter_map(|_| m.step(&Triggers::default(), 30)).collect()
    }

    #[test]
    fn fp_logic_arming_needs_clear_ticks() {
        let mut m = FpMachine::default();
        assert_eq!(arm(&mut m, 29), vec![]);
        assert_eq!(m.state(), FpState::Arming(29));
        assert_eq!(m.step(&Triggers::default(), 30), Some(FpEvent::On));
        assert!(m.is_on());
        assert_eq!(m.step(&Triggers::default(), 30), None, "stays on");
        // a block during arming restarts the count and never yields On (no flapping)
        let mut m = FpMachine::default();
        arm(&mut m, 20);
        assert_eq!(m.step(&Triggers::only(Reason::Menu), 30), Some(FpEvent::Blocked(Reason::Menu)));
        assert_eq!(m.step(&Triggers::only(Reason::Menu), 30), None, "logged once per change");
        assert_eq!(m.state(), FpState::Off);
        let mut alternating = FpMachine::default();
        for i in 0..200 {
            let t = if i % 10 == 9 { Triggers::only(Reason::CamGate) } else { Triggers::default() };
            assert_ne!(alternating.step(&t, 30), Some(FpEvent::On), "tick {i}");
        }
    }

    #[test]
    fn fp_logic_each_trigger_hands_back() {
        // Every trigger in the design table switches On -> Off in the same tick and reports itself.
        for r in Reason::ALL.into_iter().filter(|r| *r != Reason::Panic) {
            let mut m = FpMachine::default();
            arm(&mut m, 30);
            assert!(m.is_on(), "{r:?}");
            assert_eq!(Triggers::only(r).first(), Some(r));
            assert_eq!(m.step(&Triggers::only(r), 30), Some(FpEvent::Off(r)), "{r:?}");
            assert!(!m.is_on());
            // while the trigger lasts nothing re-arms
            for _ in 0..100 {
                assert_eq!(m.step(&Triggers::only(r), 30), None);
            }
            // once clear, everything re-arms after arm_ticks (f5/f6/config clear only when the user acts)
            assert_eq!(arm(&mut m, 30), vec![FpEvent::On], "{r:?}");
        }
        // panic: forced off from the panic path
        let mut m = FpMachine::default();
        arm(&mut m, 30);
        assert!(m.force_off(Reason::Panic));
        assert!(!m.force_off(Reason::Panic), "already off");
        assert!(Reason::F6.waits_for_user() && Reason::F5.waits_for_user() && !Reason::Torrent.waits_for_user());
        let labels: Vec<&str> = Reason::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(
            labels,
            ["panic", "read_fail", "cam_persist", "loading", "death", "ac130", "nuke", "cutscene", "torrent", "ladder", "throw", "talk", "menu", "cam_gate", "f5", "f6", "config"]
        );
    }

    #[test]
    fn c6_scope3p_peek_wanted_truth_table() {
        let all = PeekIn { flag: true, fp_enabled: true, viewmodel: true, overlay_gun: true, switch_ready: true, live: true, ac130: false, nuke: false, ads_held: true };
        assert!(peek_wanted(&all));
        let offs: [fn(&mut PeekIn); 9] = [
            |p| p.flag = false,
            |p| p.fp_enabled = false,
            |p| p.viewmodel = false,
            |p| p.overlay_gun = false,
            |p| p.switch_ready = false,
            |p| p.live = false,
            |p| p.ac130 = true,
            |p| p.nuke = true,
            |p| p.ads_held = false,
        ];
        for (k, f) in offs.iter().enumerate() {
            let mut p = all;
            f(&mut p);
            assert!(!peek_wanted(&p), "gate {k}");
        }
        // the short arm only after a user-chosen third person
        assert_eq!(arm_ticks_for(true, false, Some(Reason::F6), 30, 1), 1);
        assert_eq!(arm_ticks_for(true, false, None, 30, 1), 1);
        for r in [Reason::CamGate, Reason::Talk, Reason::Menu, Reason::Torrent, Reason::Cutscene, Reason::Loading, Reason::F5] {
            assert_eq!(arm_ticks_for(true, false, Some(r), 30, 1), 30, "{r:?}");
        }
        assert_eq!(arm_ticks_for(false, false, Some(Reason::F6), 30, 1), 30, "no peek");
        assert_eq!(arm_ticks_for(true, true, Some(Reason::F6), 30, 1), 30, "the user's own first person: unchanged");
        assert_eq!(arm_ticks_for(true, false, Some(Reason::F6), 30, 0), 1, "at least one tick");
    }

    #[test]
    fn c6_scope3p_peek_holds_only_for_a_peek_episode() {
        // off: a scoped ADS may start a peek
        assert!(peek_holds(true, false, false));
        // a running peek keeps holding while scoped
        assert!(peek_holds(true, true, true));
        // the user's own first person (not a peek) + F6 while scoped: no hold, F6 turns it off as before
        assert!(!peek_holds(true, false, true));
        // no scoped ADS: never
        for (e, on) in [(false, false), (true, true), (false, true)] {
            assert!(!peek_holds(false, e, on));
        }
    }

    #[test]
    fn c6_scope3p_machine_peek_on_and_off() {
        // third person by F6: unwanted
        let mut m = FpMachine::default();
        arm(&mut m, 30);
        assert_eq!(m.step(&Triggers::only(Reason::F6), 30), Some(FpEvent::Off(Reason::F6)));
        assert_eq!(m.step(&Triggers::only(Reason::F6), 30), None);
        // ADS pressed: the peek clears `unwanted`; one tick of arming turns first person on
        let a = arm_ticks_for(true, false, m.last_block(), 30, 1);
        assert_eq!(m.step(&Triggers::default(), a), Some(FpEvent::On));
        for _ in 0..100 {
            assert_eq!(m.step(&Triggers::default(), 30), None, "held: stays on");
        }
        // released: off in the same tick, reason f6 (the existing on/off pairing)
        assert_eq!(m.step(&Triggers::only(Reason::F6), 30), Some(FpEvent::Off(Reason::F6)));
        // a camera-gate block during the hold keeps the full arm (anti-flap)
        let mut g = FpMachine::default();
        assert_eq!(g.step(&Triggers::only(Reason::CamGate), 30), Some(FpEvent::Blocked(Reason::CamGate)));
        let a = arm_ticks_for(true, false, g.last_block(), 30, 1);
        assert_eq!(a, 30);
        for k in 0..29 {
            assert_eq!(g.step(&Triggers::default(), a), None, "tick {k}");
        }
        assert_eq!(g.step(&Triggers::default(), a), Some(FpEvent::On));
    }

    #[test]
    fn fp_logic_ac130_blocks_and_rearms() {
        // M4: the AC-130 hands first person back in the same tick, blocks arming while it lasts, and first person
        // re-arms through the normal 30 clear ticks afterwards
        let mut m = FpMachine::default();
        arm(&mut m, 30);
        assert_eq!(m.step(&Triggers::only(Reason::Ac130), 30), Some(FpEvent::Off(Reason::Ac130)));
        for _ in 0..2400 {
            assert_eq!(m.step(&Triggers::only(Reason::Ac130), 30), None, "blocked during the ride");
        }
        for k in 0..29 {
            assert_eq!(m.step(&Triggers::default(), 30), None, "tick {k}");
        }
        assert_eq!(m.step(&Triggers::default(), 30), Some(FpEvent::On));
        assert!(!Reason::Ac130.waits_for_user());
        // death outranks it, it outranks a cutscene / menu
        assert_eq!(Triggers { dead: true, ac130: true, ..Triggers::default() }.first(), Some(Reason::Death));
        assert_eq!(Triggers { ac130: true, menu: true, cutscene: true, ..Triggers::default() }.first(), Some(Reason::Ac130));
    }

    #[test]
    fn fp_logic_priority_order() {
        // Simultaneous triggers report the first in the fixed order, for every pair.
        for (i, a) in Reason::ALL.iter().enumerate().skip(1) {
            for b in Reason::ALL.iter().skip(i + 1) {
                let mut t = Triggers::only(*a);
                let tb = Triggers::only(*b);
                t = Triggers {
                    read_fail: t.read_fail || tb.read_fail,
                    cam_persist: t.cam_persist || tb.cam_persist,
                    loading: t.loading || tb.loading,
                    dead: t.dead || tb.dead,
                    ac130: t.ac130 || tb.ac130,
                nuke: t.nuke || tb.nuke,
                    cutscene: t.cutscene || tb.cutscene,
                    riding: t.riding || tb.riding,
                    ladder: t.ladder || tb.ladder,
                    throw: t.throw || tb.throw,
                    talk: t.talk || tb.talk,
                    menu: t.menu || tb.menu,
                    cam_invalid: t.cam_invalid || tb.cam_invalid,
                    mode_off: t.mode_off || tb.mode_off,
                    unwanted: t.unwanted || tb.unwanted,
                    disabled: t.disabled || tb.disabled,
                };
                assert_eq!(t.first(), Some(*a), "{a:?} vs {b:?}");
            }
        }
        // e.g. death while riding at a grace with the menu open and the gate invalid -> death
        let t = Triggers { dead: true, riding: true, talk: true, menu: true, cam_invalid: true, ..Triggers::default() };
        let mut m = FpMachine::default();
        arm(&mut m, 30);
        assert_eq!(m.step(&t, 30), Some(FpEvent::Off(Reason::Death)));
    }

    #[test]
    fn fp_logic_edge_and_fade_watch() {
        // talk id: unknown idle value; only a change while On counts
        let mut e = EdgeWatch::<i32>::default();
        assert!(!e.changed(Some(7)), "not armed");
        e.arm(Some(-1));
        assert!(!e.changed(Some(-1)));
        assert!(e.changed(Some(1000)));
        assert!(!e.changed(None), "unavailable is not a change");
        e.arm(None);
        assert!(!e.changed(Some(5)), "an unavailable baseline disarms");
        e.arm(Some(3));
        e.clear();
        assert!(!e.changed(Some(4)));
        // fade: plate 2 stuck at 1.0 since arm time is ignored; plate 5 rising above 0.5 fires
        let thr = 0.5;
        let mut f = FadeWatch::default();
        let mut a = [Some(0.0f32); FADE_PLATES];
        a[2] = Some(1.0);
        a[7] = None;
        assert_eq!(f.fired(&a, thr), None, "not armed");
        f.arm(&a, thr);
        assert_eq!(f.fired(&a, thr), None, "stuck plate ignored");
        a[5] = Some(0.4);
        assert_eq!(f.fired(&a, thr), None);
        a[5] = Some(0.83);
        assert_eq!(f.fired(&a, thr), Some((5, 0.83)));
        assert_eq!(fade_max(&a), Some(1.0));
        f.clear();
        assert_eq!(f.fired(&a, thr), None);
        assert_eq!(fade_max(&[None; FADE_PLATES]), None);
    }

    #[test]
    fn fp_logic_signal_rules() {
        assert!(!riding(None, false, true), "no ride node = not riding");
        assert!(!riding(Some(0), false, true));
        for s in [3, 5, 7] {
            assert!(riding(Some(s), false, true), "{s}");
            assert!(!riding(Some(s), false, false), "{s}: ride_state switched off");
        }
        assert!(riding(Some(0), true, true), "mounting");
        assert!(riding(Some(0), true, false), "is_mounting stays even with the ride_state switch off");
        assert!(!on_ladder(-1));
        for s in [0i8, 1, 5, 22] {
            assert!(on_ladder(s));
        }
        assert!(!in_throw(None) && !in_throw(Some(1)) && !in_throw(Some(7)));
        for s in 3..=6 {
            assert!(in_throw(Some(s)));
        }
        assert!(dead(0, 0x08, Some(0)) && dead(-5, 0, None));
        assert!(dead(100, 0x88, Some(0)), "death_flag bit 7");
        assert!(dead(100, 0x08, Some(7)), "DeathCam");
        assert!(!dead(100, 0x08 | 0x10 | 0x02, Some(5)), "render/invincible/precision bits are not death");
    }

    const LP: LatchParams = LatchParams { holdoff_s: 2.0, stale_s: 20.0 };
    const DT: f64 = 1.0 / 60.0;

    /// The DLL's per-tick wiring of one edge signal (first_person.rs), reduced to the pure parts: the edge fires only
    /// while On; when it fires it latches with the On baseline; while latched the trigger stays active.
    struct Sim {
        m: FpMachine,
        edge: EdgeWatch<i32>,
        latch: EdgeLatch<i32>,
        ons: u32,
        offs: Vec<(f64, Reason)>,
        releases: Vec<(f64, LatchRelease)>,
    }

    impl Sim {
        fn new() -> Sim {
            Sim { m: FpMachine::default(), edge: EdgeWatch::default(), latch: EdgeLatch::default(), ons: 0, offs: vec![], releases: vec![] }
        }

        fn tick(&mut self, now: f64, value: Option<i32>, menu: bool) {
            self.tick_gated(now, value, menu, menu, &TalkGate::default(), 0.0);
        }

        /// 2026-10-21: `talk_menu` = the talk latch's menu input (cursor or the dialog index 3), `gate` / `window_s`
        /// the interact gate (window 0 = every change, as `tick`); an ignored change re-arms the edge on it.
        fn tick_gated(&mut self, now: f64, value: Option<i32>, menu: bool, talk_menu: bool, gate: &TalkGate, window_s: f64) {
            let hold = match self.latch.step(value, talk_menu, now, LP) {
                LatchStep::Hold => true,
                LatchStep::Released(r) => {
                    self.releases.push((now, r));
                    false
                }
                LatchStep::Idle => false,
            };
            let moved = self.edge.changed(value);
            let fired = moved && gate.is_dialog(now, window_s);
            if moved && !fired {
                self.edge.arm(value);
            }
            if fired {
                self.latch.latch(self.edge.baseline().unwrap(), value, now);
            }
            let t = Triggers { talk: fired || hold, menu, ..Triggers::default() };
            match self.m.step(&t, 30) {
                Some(FpEvent::On) => {
                    self.ons += 1;
                    self.edge.arm(value);
                }
                Some(FpEvent::Off(r)) => {
                    self.offs.push((now, r));
                    self.edge.clear();
                }
                _ => {}
            }
        }
    }

    #[test]
    fn fp_logic_edge_latch_keeps_handback_while_value_changed() {
        // Review M3b major 1: the talk id changes (grace), first person hands back, the value STAYS changed for
        // 60 s with no cursor shown: the machine must never reach On during that time.
        let mut s = Sim::new();
        let mut t = 0.0;
        for _ in 0..40 {
            s.tick(t, Some(-1), false);
            t += DT;
        }
        assert!(s.m.is_on() && s.ons == 1);
        let fire_t = t;
        while t < fire_t + 15.0 {
            // the talk lasts 15 s; the value moves between talk nodes (resets the stale clock)
            let v = if t < fire_t + 7.0 { 1000 } else { 1001 };
            s.tick(t, Some(v), false);
            assert!(!s.m.is_on(), "re-armed during the talk at t={t:.2}");
            t += DT;
        }
        assert_eq!(s.offs.len(), 1);
        assert_eq!(s.offs[0].1, Reason::Talk);
        // talk ends: the value returns to -1; still blocked for the 2 s hold-off + 30 arm ticks
        let back_t = t;
        while t < back_t + 1.9 {
            s.tick(t, Some(-1), false);
            assert!(!s.m.is_on(), "hold-off at t={t:.2}");
            t += DT;
        }
        while t < back_t + 3.0 {
            s.tick(t, Some(-1), false);
            t += DT;
        }
        assert_eq!(s.releases.len(), 1);
        assert_eq!(s.releases[0].1, LatchRelease::Returned);
        assert!(s.releases[0].0 >= back_t + 2.0 - 1e-9);
        assert!(s.m.is_on() && s.ons == 2, "re-armed after the hold-off + arm_ticks");

        // Without the latch (old behaviour) the same scenario re-armed 0.5 s after the edge: prove the test bites.
        let mut m = FpMachine::default();
        let mut e = EdgeWatch::<i32>::default();
        let mut on_again = false;
        for k in 0..400 {
            let v = if k < 40 { -1 } else { 1000 };
            let fired = e.changed(Some(v));
            match m.step(&Triggers { talk: fired, ..Triggers::default() }, 30) {
                Some(FpEvent::On) => {
                    e.arm(Some(v));
                    on_again |= k > 40;
                }
                Some(FpEvent::Off(_)) => e.clear(),
                _ => {}
            }
        }
        assert!(on_again, "the unlatched edge re-arms inside the talk (the reviewed bug)");
    }

    /// Seconds of the day of a log timestamp.
    fn hms(h: u32, m: u32, s: f64) -> f64 {
        (h * 3600 + m * 60) as f64 + s
    }

    /// 2026-10-21: replays one talk id change from the logs at 60 Hz: first person On, the interact presses at
    /// `presses`, the id goes `from -> to` at `at` and stays, ui_states idx 3 (subtitle / dialog) is open for 4 s
    /// from the change. Returns (handed back for `talk`, first person On at the end).
    fn replay_talk(presses: &[f64], from: i32, to: i32, at: f64, window_s: f64) -> (bool, bool) {
        let mut s = Sim::new();
        let mut g = TalkGate::default();
        let mut t = presses.iter().copied().fold(at, f64::min) - 2.0;
        let mut next = 0;
        while t < at + 10.0 {
            let n = presses[next..].iter().take_while(|&&p| p <= t).count();
            next += n;
            g.interacted(n as u32, t);
            let v = if t < at { from } else { to };
            let idx3 = t >= at && t < at + 4.0;
            s.tick_gated(t, Some(v), false, idx3, &g, window_s);
            t += DT;
        }
        (s.offs.iter().any(|o| o.1 == Reason::Talk), s.m.is_on())
    }

    #[test]
    fn fp_talk_gate_boss_voice_lines_keep_first_person_on() {
        const W: f64 = 8.0;
        // 2026-10-21 13:05:10.948: c2130 at hp ~1800, last interact (pad X hold) 48 s before
        let mid = [hms(13, 4, 15.016), hms(13, 4, 19.299), hms(13, 4, 22.699)];
        assert_eq!(replay_talk(&mid, 1000, 204_101_000, hms(13, 5, 10.948), W), (false, true));
        // 13:08:59.444: the arena line, 11.2 s after the interact that teleported the player (fade at 13:08:48.634)
        let arena = [hms(13, 8, 41.498), hms(13, 8, 48.201)];
        assert_eq!(replay_talk(&arena, 1000, 302_001_000, hms(13, 8, 59.444), W), (false, true));
        // 2026-10-19 (C2 host): the same two lines (15:16:19.126 mid-fight; 15:18:50.721, 13.3 s after an E) and
        // 302001000 -> 302201000 (15:33:53.224, a minute after an E)
        assert_eq!(replay_talk(&[hms(15, 3, 13.711)], 1000, 204_101_000, hms(15, 16, 19.126), W), (false, true));
        let host_arena = [hms(15, 18, 13.254), hms(15, 18, 37.424)];
        assert_eq!(replay_talk(&host_arena, 1000, 302_001_000, hms(15, 18, 50.721), W), (false, true));
        assert_eq!(replay_talk(&[hms(15, 32, 53.249)], 302_001_000, 302_201_000, hms(15, 33, 53.224), W), (false, true));
        // C2 friend 15:52:37.045: -1 -> 301006000, 46.8 s after the friend's last E
        assert_eq!(replay_talk(&[hms(15, 51, 50.235)], -1, 301_006_000, hms(15, 52, 37.045), W), (false, true));
        // mutation: without the gate (window 0, the C3 behaviour) every one of them hands back
        assert_eq!(replay_talk(&mid, 1000, 204_101_000, hms(13, 5, 10.948), 0.0).0, true);
        assert_eq!(replay_talk(&arena, 1000, 302_001_000, hms(13, 8, 59.444), 0.0).0, true);
    }

    #[test]
    fn fp_talk_gate_keeps_grace_and_npc_dialog_handbacks() {
        const W: f64 = 8.0;
        // C2 host 15:03:17.351: E at 15:03:13.711 (sit at grace, kneel animation), -1 -> 1000: hands back, and the
        // C3 latch holds it off while idx 3 is open + the stale time (FP still off at the end of the replay window)
        let grace = [hms(15, 3, 0.045), hms(15, 3, 13.711)];
        assert_eq!(replay_talk(&grace, -1, 1000, hms(15, 3, 17.351), W), (true, false));
        let (back, _) = replay_talk(&grace, -1, 1000, hms(15, 3, 17.351), 3.0);
        assert!(!back, "a 3 s window misses the grace (3.6 s): the default must stay above it");
        // an NPC: E, the id changes 0.8 s later (1000 -> 302001000 after a grace)
        assert_eq!(replay_talk(&[100.0], 1000, 302_001_000, 100.8, W).0, true);
        // exactly at the window edge counts; just past it does not
        let g = {
            let mut g = TalkGate::default();
            g.interacted(0, 1.0);
            assert!(!g.is_dialog(1.0, W) && g.is_dialog(1.0, 0.0) && g.since(1.0).is_infinite());
            g.interacted(2, 10.0);
            g
        };
        assert!(g.is_dialog(18.0, W) && !g.is_dialog(18.01, W) && g.is_dialog(1e9, -1.0));
    }

    #[test]
    fn fp_talk_gate_ignored_line_rearms_then_a_grace_still_hands_back() {
        // the boss line is ignored (the edge takes 204101000 as its new baseline, FP stays On); 30 s later the player
        // sits at a grace (E, 3.6 s, 204101000 -> 1000): that change hands back
        let mut s = Sim::new();
        let mut g = TalkGate::default();
        let mut t = 0.0_f64;
        while t < 60.0 {
            if (t - 40.0).abs() < DT / 2.0 {
                g.interacted(1, t);
            }
            let v = if t < 5.0 { 1000 } else if t < 43.6 { 204_101_000 } else { 1000 };
            s.tick_gated(t, Some(v), false, (5.0..9.0).contains(&t) || (43.6..50.0).contains(&t), &g, 8.0);
            if t > 1.0 && t < 43.6 {
                assert!(s.m.is_on(), "off at t={t:.2}");
            }
            t += DT;
        }
        assert_eq!(s.offs.len(), 1);
        assert!(s.offs[0].0 >= 43.6 - DT && s.offs[0].1 == Reason::Talk);
    }

    #[test]
    fn fp_logic_edge_latch_stale_release_needs_closed_cursor() {
        // A value that never returns (a talk id that keeps its last value, a load counter) releases only after it
        // has been stable AND the cursor has been closed for stale_s.
        let mut l = EdgeLatch::<i32>::default();
        assert_eq!(l.step(Some(5), false, 0.0, LP), LatchStep::Idle);
        l.latch(-1, Some(1000), 0.0);
        assert!(l.is_latched() && l.baseline() == Some(-1));
        let mut t = 0.0;
        // cursor open (grace menu) for 30 s: holds the whole time
        while t < 30.0 {
            assert_eq!(l.step(Some(1000), true, t, LP), LatchStep::Hold, "t={t}");
            t += 0.1;
        }
        // cursor closes at 30 s: holds for stale_s more, then releases as Stale
        let closed = t;
        let mut released = None;
        while t < closed + 25.0 {
            if let LatchStep::Released(r) = l.step(Some(1000), false, t, LP) {
                released = Some((t, r));
                break;
            }
            t += 0.1;
        }
        let (rt, r) = released.expect("stale release");
        assert_eq!(r, LatchRelease::Stale);
        assert!(rt >= closed + 20.0 - 1e-9 && rt < closed + 20.2, "{rt}");
        assert!(!l.is_latched());
        // a value that keeps moving never goes stale
        l.latch(0, Some(1), 0.0);
        for k in 1..600 {
            assert_eq!(l.step(Some(1 + (k % 2)), false, k as f64 * 0.1, LP), LatchStep::Hold);
        }
        // unreadable counts as still changed; a brief return shorter than the hold-off does not release
        l.latch(0, Some(1), 0.0);
        assert_eq!(l.step(None, false, 0.1, LP), LatchStep::Hold);
        assert_eq!(l.step(Some(0), false, 0.2, LP), LatchStep::Hold);
        assert_eq!(l.step(Some(0), false, 1.5, LP), LatchStep::Hold);
        assert_eq!(l.step(Some(3), false, 1.6, LP), LatchStep::Hold, "left the baseline again: hold-off restarts");
        assert_eq!(l.step(Some(0), false, 2.0, LP), LatchStep::Hold);
        assert_eq!(l.step(Some(0), false, 4.1, LP), LatchStep::Released(LatchRelease::Returned));
        // stale_s = 0: only a return releases
        let mut l = EdgeLatch::<bool>::default();
        l.latch(false, Some(true), 0.0);
        assert_eq!(l.step(Some(true), false, 1000.0, LatchParams { holdoff_s: 2.0, stale_s: 0.0 }), LatchStep::Hold);
        l.reset();
        assert_eq!(l.step(Some(true), false, 1001.0, LP), LatchStep::Idle);
    }

    /// Seconds since 13:00:00 of a `13:mm:ss.mmm` log time.
    fn death_t(m: u32, s: f64) -> f64 {
        m as f64 * 60.0 + s
    }

    #[test]
    fn fp_logic_death_gate_host_c6_third_person_death_refuses_every_shot_until_respawn() {
        // logs/coop/host_c6_2026-10-24.log: 13:27:52.425 `MOVE suspend reason=death hp=0` / `FP off reason=death`
        // (third person after it), hp_le0=1 in every FP probe up to 13:30:42.688, `TASK world left` 13:30:42.789,
        // `TASK world entered` 13:30:44.769. The fire presses are the SHOT lines of that window (n=946..985).
        let mut g = DeathGate::default();
        // alive and shooting before (SHOT n=945 13:27:51.140 mag=0/100): never refused
        assert_eq!(g.update(death_t(27, 51.140), Some(false)), None);
        assert_eq!(g.press(true), None);
        assert!(!g.blocked());
        g.press(false);
        assert_eq!(g.update(death_t(27, 52.425), Some(true)), Some(DeathEv::On));
        assert!(g.blocked());
        let presses = [
            (28, 3.208),  // n=946 skipped: camera settling (dist)
            (29, 54.518), // n=948 skipped: origin 10.76m from the chest
            (30, 8.968),  // n=949..956
            (30, 11.069), // n=957..965, then RELOAD start 13:30:11.802 (while dead)
            (30, 21.352), // n=966..968
            (30, 25.468), // n=969..970
            (30, 40.023), // n=971..985: 15 real bullets spawned at the corpse (-24.7,-1.77,12.8)
        ];
        for (i, (m, s)) in presses.iter().enumerate() {
            let t = death_t(*m, *s);
            assert_eq!(g.update(t, Some(true)), None);
            assert_eq!(g.press(true), Some(i as u32 + 1), "press at 13:{m}:{s} refused");
            assert_eq!(g.update(t + 0.1, Some(true)), None);
            assert!(g.blocked());
            g.press(false);
        }
        // the respawn load: no player read decides nothing
        assert_eq!(g.update(death_t(30, 42.789), None), None);
        assert_eq!(g.update(death_t(30, 43.8), None), None);
        assert!(g.blocked());
        // world entered alive: refused for DEATH_REARM_S more, then off with the 7 refused presses
        assert_eq!(g.update(death_t(30, 44.769), Some(false)), None);
        assert_eq!(g.press(true), Some(8), "held through the first alive read");
        assert_eq!(g.update(death_t(30, 45.0), Some(false)), None);
        assert!(g.blocked());
        match g.update(death_t(30, 45.269), Some(false)) {
            Some(DeathEv::Off { dead_s, refused }) => {
                assert!((dead_s - 172.844).abs() < 1e-6, "{dead_s}");
                assert_eq!(refused, 8);
            }
            e => panic!("{e:?}"),
        }
        assert!(!g.blocked());
        // the button still held from the dead window is not a new press
        assert_eq!(g.press(true), None);
    }

    #[test]
    fn fp_logic_death_gate_friend_c6_pre_death_shots_stay_live() {
        // logs/coop/friend_c6_2026-10-24.log: SHOT n=1215 14:20:39.339 (alive), `MOVE suspend reason=death hp=0`
        // 14:20:39.507 (168 ms later), respawn `TASK world entered` 14:20:48.782
        let mut g = DeathGate::default();
        assert_eq!(g.update(20.0 * 60.0 + 39.339, Some(false)), None);
        assert_eq!(g.press(true), None);
        assert!(!g.blocked(), "the last shot before the death fires");
        assert_eq!(g.update(20.0 * 60.0 + 39.507, Some(true)), Some(DeathEv::On));
        assert!(g.blocked(), "the trigger held into the death is refused from the death tick");
        assert_eq!(g.press(true), None, "held, not a new press");
    }

    #[test]
    fn fp_logic_death_gate_signals_and_gaps() {
        // a missing read while alive never blocks
        let mut g = DeathGate::default();
        assert_eq!(g.update(0.0, None), None);
        assert!(!g.blocked());
        // death flag / DeathCam with hp > 0 block like hp <= 0 (the `dead` signal)
        assert_eq!(g.update(1.0, Some(dead(500, DEATH_FLAG, Some(0)))), Some(DeathEv::On));
        let mut g2 = DeathGate::default();
        assert_eq!(g2.update(1.0, Some(dead(500, 0x08, Some(CAMERA_TYPE_DEATH)))), Some(DeathEv::On));
        // one alive read, then a gap: the re-arm time restarts
        assert_eq!(g.update(2.0, Some(false)), None);
        assert_eq!(g.update(2.4, None), None);
        assert_eq!(g.update(2.6, Some(false)), None);
        assert_eq!(g.update(3.0, Some(false)), None);
        assert!(g.blocked(), "0.4 s alive since the gap");
        // a dead read again restarts it too
        assert_eq!(g.update(3.05, Some(true)), None);
        assert_eq!(g.update(3.1, Some(false)), None);
        assert!(matches!(g.update(3.6, Some(false)), Some(DeathEv::Off { refused: 0, .. })));
        // the next death is a new episode with its own count
        g.press(false);
        assert_eq!(g.update(10.0, Some(true)), Some(DeathEv::On));
        assert_eq!(g.press(true), Some(1));
    }

    #[test]
    fn fp_logic_flap_guard_mutes_a_flapping_signal() {
        let mut g = FlapGuard::default();
        // spread out: never muted
        for i in 0..20 {
            assert!(!g.record(i as f64 * 20.0));
        }
        assert!(!g.muted());
        // 5 within 60 s: muted exactly once
        let mut g = FlapGuard::default();
        let got: Vec<bool> = (0..7).map(|i| g.record(1000.0 + i as f64)).collect();
        assert_eq!(got, [false, false, false, false, true, false, false]);
        assert!(g.muted());
    }
}
