//! M7 Tactical Nuke sequence (docs/M7_DESIGN.md 3.5-3.8, 3.12). Pure: the DLL ticks it and does the calls.
//!
//! Timeline from the activation (MW2 `_nuke.gsc`, read from `converted\nuke\nuke.json` [V dumps]): a timer beep every
//! `beep_every_s` from 0 (the beep at the detonation itself is dropped: it races the explosion), `nuke_incoming` at
//! `timer - incoming_lead`, the detonation at `timer` (explosion sounds, the flash, horizon explosions, an AC-130 ride
//! is ended), the `mpnuke` vision from `timer + vision_delay` blended over `vision_blend`, the kills at `timer +
//! death_delay` with `mpnuke_aftermath` blended over `aftermath_blend`, the shake at `timer + quake_delay` (MW2 threads
//! `nukeEarthquake` at the same time as `nukeDeath`; it stops at the player's death), survivor retries, the player's death
//! `player_delay_s` after the kills (prep 0.5 s earlier), the aftermath held `aftermath_hold_s` and faded over 2 s.
//! Slow motion is skipped (user decision).
//!
//! Events fire exactly once and in order, whatever the tick rate (a 500 ms hitch fires everything due at once).
//! Cancel paths: a loading screen before the detonation cancels and refunds; after it the sequence ends early. F5 and
//! F7 are refused while a sequence runs. The caller's own death does not cancel (MW2 `scr_nukeCancelMode 0`).

use crate::ac130_shake::{QuakeSample, ShakeOut, ShakeParams, sample};
use crate::nuke_data::Timeline;

/// The aftermath fades out over this long after its hold (our addition).
pub const AFTERMATH_FADE_S: f64 = 2.0;
/// The player's prep (first person / movement / inputs handed back) comes this long before the lethal effect.
pub const PLAYER_PREP_LEAD_S: f64 = 0.5;
/// Flash shape, rise / hold / decay (seconds from the detonation). The flash itself is MW2 behaviour: `nukeEffects`
/// plays the `nuke_flash` fx (player_death_nuke_flash) `nukeDistance` in front of each player at T; ours stands in for
/// that fx as a full-screen post-pass flash (the full-screen form and this shape are ours).
pub const FLASH_RISE_S: f64 = 0.08;
pub const FLASH_HOLD_S: f64 = 0.25;
pub const FLASH_DECAY_S: f64 = 2.5;

/// The plan of one sequence (MW2 timeline + config), absolute seconds from the activation.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub timer_s: f64,
    pub incoming_at: f64,
    pub beep_every_s: f64,
    pub vision_at: f64,
    pub vision_blend_s: f64,
    pub kill_at: f64,
    pub aftermath_blend_s: f64,
    /// Absolute times of the survivor retries.
    pub retries: Vec<f64>,
    pub player_dies: bool,
    pub player_prep_at: f64,
    pub player_lethal_at: f64,
    pub aftermath_hold_s: f64,
    pub flash: bool,
    pub shake: bool,
    pub shake_scale: f32,
    pub shake_s: f64,
    /// The shake starts here (absolute; MW2 `nukeEarthquake` at timer + 1.5).
    pub quake_at: f64,
    /// The horizon explosions after the detonation (seconds after it).
    pub horizon: Vec<f64>,
    pub end_at: f64,
}

impl Plan {
    pub fn new(t: &Timeline, cfg: &crate::config::Nuke) -> Plan {
        let timer = t.timer_s;
        let kill_at = timer + t.death_delay_s;
        let lethal = kill_at + cfg.player_delay_s;
        let end_at = kill_at + t.aftermath_blend_s + cfg.aftermath_hold_s + AFTERMATH_FADE_S;
        Plan {
            timer_s: timer,
            incoming_at: (timer - t.incoming_lead_s).max(0.0),
            beep_every_s: t.beep_every_s.max(0.1),
            vision_at: timer + t.vision_delay_s,
            vision_blend_s: t.vision_blend_s,
            kill_at,
            aftermath_blend_s: t.aftermath_blend_s,
            retries: cfg.retry_s.iter().map(|r| kill_at + r).collect(),
            player_dies: cfg.player_dies,
            player_prep_at: lethal - PLAYER_PREP_LEAD_S,
            player_lethal_at: lethal,
            aftermath_hold_s: cfg.aftermath_hold_s,
            flash: cfg.flash,
            shake: cfg.shake,
            shake_scale: cfg.shake_scale,
            shake_s: cfg.shake_s,
            quake_at: timer + t.quake_delay_s,
            horizon: (0..cfg.horizon_count).map(|k| 0.15 * k as f64).collect(),
            end_at: end_at.max(if cfg.player_dies { lethal + 1.0 } else { 0.0 }),
        }
    }

    /// The beeps: 0, every, 2 x every, ... strictly before the detonation.
    pub fn beeps(&self) -> Vec<f64> {
        let mut v = Vec::new();
        let mut k = 0u32;
        loop {
            let t = k as f64 * self.beep_every_s;
            if t >= self.timer_s - 1e-9 || k > 1000 {
                break;
            }
            v.push(t);
            k += 1;
        }
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    /// Activation sounds (detonator press + announcer), the HUD countdown.
    Activate,
    Beep(u32),
    /// `nuke_incoming`
    Incoming,
    /// The explosion chain + `nuke_wave`, the shake, an AC-130 ride ends, the HUD counter hides.
    Detonate,
    /// One horizon explosion (index).
    Horizon(u32),
    /// The first kill pass (0) and the retries (1..).
    Kill(u32),
    /// First person, movement, inputs, reload handed back; the protection removed.
    PlayerPrep,
    /// The player's lethal effect may be applied (after the protection is confirmed absent).
    PlayerLethal,
    /// The aftermath fade is over: the sequence ends.
    End,
}

impl Ev {
    pub fn label(self) -> String {
        match self {
            Ev::Activate => "activate".into(),
            Ev::Beep(k) => format!("beep{k}"),
            Ev::Incoming => "incoming".into(),
            Ev::Detonate => "detonate".into(),
            Ev::Horizon(k) => format!("horizon{k}"),
            Ev::Kill(k) => format!("kill{k}"),
            Ev::PlayerPrep => "player_prep".into(),
            Ev::PlayerLethal => "player_lethal".into(),
            Ev::End => "end".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Countdown,
    Flash,
    Killing,
    Aftermath,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Countdown => "countdown",
            Phase::Flash => "flash",
            Phase::Killing => "killing",
            Phase::Aftermath => "aftermath",
        }
    }
}

/// One running sequence.
#[derive(Clone, Debug)]
pub struct NukeSeq {
    pub t0: f64,
    pub plan: Plan,
    events: Vec<(f64, Ev)>,
    next: usize,
    pub test: bool,
    pub detonated: bool,
    pub ended: bool,
}

impl NukeSeq {
    pub fn new(now: f64, plan: Plan, test: bool) -> NukeSeq {
        let p = &plan;
        let mut ev: Vec<(f64, Ev)> = vec![(0.0, Ev::Activate)];
        ev.extend(p.beeps().into_iter().enumerate().map(|(k, t)| (t, Ev::Beep(k as u32))));
        ev.push((p.incoming_at, Ev::Incoming));
        ev.push((p.timer_s, Ev::Detonate));
        ev.extend(p.horizon.iter().enumerate().map(|(k, d)| (p.timer_s + d, Ev::Horizon(k as u32))));
        ev.push((p.kill_at, Ev::Kill(0)));
        ev.extend(p.retries.iter().enumerate().map(|(k, t)| (*t, Ev::Kill(k as u32 + 1))));
        if p.player_dies {
            ev.push((p.player_prep_at, Ev::PlayerPrep));
            ev.push((p.player_lethal_at, Ev::PlayerLethal));
        }
        ev.push((p.end_at, Ev::End));
        // stable: equal times keep the insertion order (activate before beep 0, detonate before horizon 0 ...)
        ev.sort_by(|a, b| a.0.total_cmp(&b.0));
        NukeSeq { t0: now, plan, events: ev, next: 0, test, detonated: false, ended: false }
    }

    /// Every event due by `now`, in order, each once.
    pub fn tick(&mut self, now: f64) -> Vec<Ev> {
        let t = now - self.t0;
        let mut out = Vec::new();
        while self.next < self.events.len() && self.events[self.next].0 <= t + 1e-9 {
            let e = self.events[self.next].1;
            if e == Ev::Detonate {
                self.detonated = true;
            }
            if e == Ev::End {
                self.ended = true;
            }
            out.push(e);
            self.next += 1;
        }
        out
    }

    /// Seconds since the activation.
    pub fn t(&self, now: f64) -> f64 {
        now - self.t0
    }

    /// The HUD countdown (`S.t`) before the detonation.
    pub fn countdown_left(&self, now: f64) -> Option<f64> {
        let left = self.plan.timer_s - self.t(now);
        (left > 0.0 && !self.detonated).then_some(left)
    }

    pub fn phase(&self, now: f64) -> Phase {
        let t = self.t(now);
        if t < self.plan.timer_s {
            Phase::Countdown
        } else if t < self.plan.kill_at {
            Phase::Flash
        } else if t < self.plan.kill_at + self.plan.aftermath_blend_s {
            Phase::Killing
        } else {
            Phase::Aftermath
        }
    }

    /// The post pass of this frame.
    pub fn post(&self, now: f64) -> PostState {
        post_at(self.t(now) - self.plan.timer_s, &self.plan)
    }

    /// The camera shake of this frame (MW2 has none: [`Plan::shake`] adds the commented-out MW2 earthquake, from
    /// [`Plan::quake_at`] like MW2's `nukeEarthquake` thread).
    pub fn shake(&self, now: f64, p: &ShakeParams) -> ShakeOut {
        let age = self.t(now) - self.plan.quake_at;
        if !self.plan.shake || age < 0.0 {
            return ShakeOut::ZERO;
        }
        let q = QuakeSample { amp: self.plan.shake_scale * p.scale, duration_s: self.plan.shake_s as f32, age_s: age as f32, seed: 0x6e75_6b65 };
        sample(&[q], p)
    }

    /// The sequence ran this much longer than planned (`NUKE WARN stuck`).
    pub fn stuck(&self, now: f64, stuck_s: f64) -> bool {
        !self.ended && self.t(now) > self.plan.end_at + stuck_s
    }
}

/// The per-frame post-pass state (docs/M7_DESIGN.md 3.7).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PostState {
    /// White flash 0..1 (stands in for MW2's `nuke_flash` fx at T; full-screen form ours).
    pub flash: f32,
    /// How much of the vision applies (0 = ER's image as is).
    pub amount: f32,
    /// 0 = `mpnuke`, 1 = `mpnuke_aftermath`.
    pub blend: f32,
}

impl PostState {
    pub fn active(&self) -> bool {
        self.flash > 0.0 || self.amount > 0.0
    }
}

fn ramp(t: f64, t0: f64, dur: f64) -> f32 {
    if t <= t0 {
        0.0
    } else if dur <= 0.0 || t >= t0 + dur {
        1.0
    } else {
        ((t - t0) / dur) as f32
    }
}

/// The post state `dt` seconds after the detonation (negative = before it).
pub fn post_at(dt: f64, p: &Plan) -> PostState {
    if dt < 0.0 {
        return PostState::default();
    }
    let flash = if !p.flash {
        0.0
    } else if dt < FLASH_RISE_S {
        (dt / FLASH_RISE_S) as f32
    } else if dt < FLASH_RISE_S + FLASH_HOLD_S {
        1.0
    } else {
        (1.0 - ramp(dt, FLASH_RISE_S + FLASH_HOLD_S, FLASH_DECAY_S)).max(0.0)
    };
    let vis0 = p.vision_at - p.timer_s;
    let kill = p.kill_at - p.timer_s;
    let hold_end = kill + p.aftermath_blend_s + p.aftermath_hold_s;
    let amount = ramp(dt, vis0, p.vision_blend_s) * (1.0 - ramp(dt, hold_end, AFTERMATH_FADE_S));
    let blend = ramp(dt, kill, p.aftermath_blend_s);
    PostState { flash, amount, blend }
}

/// Why key 4 is refused (`NUKE blocked reason=`), in check order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Disabled,
    Inbound,
    Ac130,
    /// M8 B1: on a ladder.
    Ladder,
    Loading,
    Dead,
    ModeOff,
    NotEarned,
    Data,
    /// Co-op C2: a peer is in the session and `[nuke] allow_in_coop = false`.
    Coop,
}

impl Block {
    pub fn label(self) -> &'static str {
        match self {
            Block::Disabled => "disabled",
            Block::Inbound => "inbound",
            Block::Ac130 => "ac130",
            Block::Ladder => "ladder",
            Block::Loading => "loading",
            Block::Dead => "dead",
            Block::ModeOff => "mode_off",
            Block::NotEarned => "not_earned",
            Block::Data => "data",
            Block::Coop => "coop_session",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GateIn {
    pub enabled: bool,
    pub data_ok: bool,
    pub inbound: bool,
    pub ac130: bool,
    /// M8 B1: `CSChrLadderModule.state != -1`.
    pub ladder: bool,
    pub loading: bool,
    pub dead: bool,
    pub mode_on: bool,
    pub available: bool,
    /// Co-op C2: the peers the gate sees (`coop::gate_peers`: 0 with `allow_in_coop`).
    pub coop_peers: u32,
}

pub fn gate(g: &GateIn) -> Result<(), Block> {
    if !g.enabled {
        return Err(Block::Disabled);
    }
    if !g.data_ok {
        return Err(Block::Data);
    }
    if g.coop_peers > 0 {
        return Err(Block::Coop);
    }
    if g.inbound {
        return Err(Block::Inbound);
    }
    if g.ac130 {
        return Err(Block::Ac130);
    }
    if g.ladder {
        return Err(Block::Ladder);
    }
    if g.loading {
        return Err(Block::Loading);
    }
    if g.dead {
        return Err(Block::Dead);
    }
    if !g.mode_on {
        return Err(Block::ModeOff);
    }
    if !g.available {
        return Err(Block::NotEarned);
    }
    Ok(())
}

/// What a loading screen does to a running sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnLoading {
    /// Before the detonation: cancelled, the nuke is given back (`NUKE cancel reason=loading refund=1`).
    CancelRefund,
    /// After it: the kills / retries / post pass stop (`NUKE end reason=loading`).
    End,
}

pub fn on_loading(seq: &NukeSeq) -> OnLoading {
    if seq.detonated { OnLoading::End } else { OnLoading::CancelRefund }
}

// ---- a peer's nuke marker (C3 PASS C, review 2026-10-24) -------------------------------------------------------

/// The state a peer's nuke marker meets in this game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteIn {
    /// `coop_mode = "owned"`.
    pub owned: bool,
    pub allow_in_coop: bool,
    /// enabled + data + rows ok.
    pub ready: bool,
    /// A sequence runs already (ours or a peer's).
    pub busy: bool,
    /// A loading screen / not in the world.
    pub loading: bool,
    /// The local player's hp > 0.
    pub alive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteAct {
    /// Start the sequence. `spectate` = the local player is dead: the kill passes run (this game's enemies die only
    /// here, an application by the peer's game does nothing), the player's death and the screen effects do not.
    Start { spectate: bool },
    /// Loading: kept and started on the first tick in the world while the marker's window lasts ([`PendingMarker`]).
    Pend,
    Ignore(&'static str),
}

/// Review 2026-10-24: `NukeMarkerRx::seen` reports a marker once, so a game that dropped it (`action=ignored why=dead`
/// / `why=loading`) never ran a kill pass and the enemies it simulates outlived the nuke (Seamless kept the host dead
/// in the world 13:27:52-13:30:44 in host_c6_2026-10-24.log, census readable: `STATE ... chars=12`). Dead now starts a
/// spectating sequence, loading pends.
pub fn remote_marker(r: &RemoteIn) -> RemoteAct {
    if !r.owned {
        RemoteAct::Ignore("coop_mode_local")
    } else if !r.allow_in_coop {
        RemoteAct::Ignore("allow_in_coop_0")
    } else if !r.ready {
        RemoteAct::Ignore("disabled_or_rows")
    } else if r.busy {
        RemoteAct::Ignore("busy")
    } else if r.loading {
        RemoteAct::Pend
    } else {
        RemoteAct::Start { spectate: !r.alive }
    }
}

/// A peer's marker kept through a loading screen: started on the first tick out of it before `until` (the receipt +
/// the plan's `kill_at` + `coop_window_s`, the caller's marker window).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingMarker {
    pub from: u64,
    pub via: &'static str,
    pub until: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendPoll {
    Wait,
    Due,
    Expired,
}

impl PendingMarker {
    pub fn poll(&self, now: f64, loading: bool) -> PendPoll {
        if now >= self.until {
            PendPoll::Expired
        } else if loading {
            PendPoll::Wait
        } else {
            PendPoll::Due
        }
    }
}

impl Plan {
    /// The plan of a sequence started for a dead local player: no player death (and no prep / input hand-back); the
    /// kills, retries and the window are unchanged.
    pub fn spectating(mut self) -> Plan {
        self.player_dies = false;
        self
    }
}

// ---- the player's death (docs/M7_DESIGN.md 3.12) ---------------------------------------------------------------

/// The `dead` input of [`PlayerKill::step`] from this tick's player sample: `None` (no sample: a transient read
/// failure) = no decision this tick (the caller skips the step; `stuck_s` ends a sequence that never gets one).
pub fn player_dead_input(hp: Option<i32>) -> Option<bool> {
    hp.map(|h| h <= 0)
}

/// How many times the protection removal is retried before the player kill is abandoned.
pub const PROTECT_TRIES: u32 = 3;
/// The lethal effect is re-applied this often while the player still lives, at most [`LETHAL_TRIES`] times.
pub const LETHAL_RETRY_S: f64 = 1.0;
pub const LETHAL_TRIES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PkStage {
    Waiting,
    /// Prep done; the protection must be confirmed absent before the lethal time.
    Prepared,
    /// The lethal effect went in; watching for the death.
    Watching,
    Done,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PkAction {
    None,
    /// Hand back first person, movement, inputs; cancel the reload; end an AC-130 ride.
    Prep,
    /// Remove the protection SpEffect (it has noDead 1: no death with it on).
    RemoveProtect { try_n: u32 },
    /// The lethal SpEffect on the player (only with the protection confirmed absent).
    ApplyLethal { try_n: u32 },
    /// `NUKE player dead t_ms=`
    Dead,
    /// `NUKE WARN player_protected` (the kill is abandoned: never an HP write).
    WarnProtected,
    /// `NUKE WARN player_not_dead`
    WarnNotDead,
}

/// The player-death order: prep first, then the protection removed and confirmed absent, then the lethal effect,
/// then the watch. Never an HP write.
#[derive(Clone, Copy, Debug)]
pub struct PlayerKill {
    pub stage: PkStage,
    protect_tries: u32,
    lethal_tries: u32,
    t_last: f64,
    pub t_lethal: f64,
}

impl Default for PlayerKill {
    fn default() -> Self {
        PlayerKill { stage: PkStage::Waiting, protect_tries: 0, lethal_tries: 0, t_last: f64::NEG_INFINITY, t_lethal: 0.0 }
    }
}

impl PlayerKill {
    /// `prep_due` / `lethal_due`: the plan's times reached. `protect_present`: the protection SpEffect on the player
    /// now (None = unreadable: treated as present). `dead`: hp 0 or the death flag.
    pub fn step(&mut self, now: f64, prep_due: bool, lethal_due: bool, protect_present: Option<bool>, dead: bool) -> PkAction {
        match self.stage {
            PkStage::Waiting => {
                if prep_due {
                    self.stage = PkStage::Prepared;
                    return PkAction::Prep;
                }
                PkAction::None
            }
            PkStage::Prepared => {
                if dead {
                    // an enemy got there first: nothing to apply
                    self.stage = PkStage::Done;
                    return PkAction::Dead;
                }
                let present = protect_present != Some(false);
                if present {
                    if now - self.t_last < 0.25 {
                        return PkAction::None;
                    }
                    if self.protect_tries >= PROTECT_TRIES {
                        self.stage = PkStage::Aborted;
                        return PkAction::WarnProtected;
                    }
                    self.protect_tries += 1;
                    self.t_last = now;
                    return PkAction::RemoveProtect { try_n: self.protect_tries };
                }
                if !lethal_due {
                    return PkAction::None;
                }
                self.stage = PkStage::Watching;
                self.lethal_tries = 1;
                self.t_last = now;
                self.t_lethal = now;
                PkAction::ApplyLethal { try_n: 1 }
            }
            PkStage::Watching => {
                if dead {
                    self.stage = PkStage::Done;
                    return PkAction::Dead;
                }
                if now - self.t_last < LETHAL_RETRY_S {
                    return PkAction::None;
                }
                if self.lethal_tries >= LETHAL_TRIES {
                    self.stage = PkStage::Done;
                    return PkAction::WarnNotDead;
                }
                if protect_present != Some(false) {
                    // the protection came back (an AC-130 refresh?): never apply over it
                    self.stage = PkStage::Prepared;
                    return PkAction::None;
                }
                self.lethal_tries += 1;
                self.t_last = now;
                PkAction::ApplyLethal { try_n: self.lethal_tries }
            }
            PkStage::Done | PkStage::Aborted => PkAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Nuke;
    use crate::nuke_data::tests::fixture;

    fn plan() -> Plan {
        Plan::new(&fixture().timeline, &Nuke::default())
    }

    #[test]
    fn nuke_logic_timeline_mw2() {
        let p = plan();
        assert_eq!(p.timer_s, 10.0);
        assert!((p.incoming_at - 6.7).abs() < 1e-9);
        assert_eq!((p.vision_at, p.kill_at), (10.25, 11.5));
        assert_eq!(p.retries, vec![12.0, 13.0, 14.5, 16.5]);
        assert_eq!((p.player_prep_at, p.player_lethal_at), (14.0, 14.5));
        assert_eq!(p.end_at, 11.5 + 5.0 + 8.0 + 2.0);
        assert_eq!(p.beeps(), (0..10).map(|k| k as f64).collect::<Vec<_>>(), "the t = 10 beep is dropped");
        assert_eq!(p.horizon, vec![0.0, 0.15, 0.3]);
    }

    fn run(seq: &mut NukeSeq, hz: f64, until: f64, hitch: Option<(f64, f64)>) -> Vec<(f64, Ev)> {
        let mut out = Vec::new();
        let mut t = seq.t0;
        while t <= seq.t0 + until {
            for e in seq.tick(t) {
                out.push((t - seq.t0, e));
            }
            t += 1.0 / hz;
            if let Some((at, len)) = hitch
                && (t - seq.t0 - at).abs() < 0.5 / hz
            {
                t += len;
            }
        }
        out
    }

    #[test]
    fn nuke_logic_events_once_in_order_every_rate_and_hitch() {
        let want: Vec<Ev> = NukeSeq::new(0.0, plan(), false).events.iter().map(|e| e.1).collect();
        for hz in [20.0, 60.0, 144.0] {
            for hitch in [None, Some((6.5, 0.5)), Some((9.8, 0.5)), Some((11.3, 0.5)), Some((14.2, 0.5))] {
                let mut s = NukeSeq::new(100.0, plan(), false);
                let got = run(&mut s, hz, 40.0, hitch);
                let evs: Vec<Ev> = got.iter().map(|x| x.1).collect();
                assert_eq!(evs, want, "{hz} Hz hitch {hitch:?}");
                // never early, at most one tick (or the hitch) late
                let late = 1.0 / hz + hitch.map_or(0.0, |h| h.1) + 1e-6;
                for (t, e) in &got {
                    let due = s.events.iter().find(|x| x.1 == *e).unwrap().0;
                    assert!(*t >= due - 1e-6 && *t - due <= late, "{e:?} at {t} due {due}");
                }
                assert!(s.ended && s.detonated);
            }
        }
    }

    #[test]
    fn nuke_logic_countdown_phase_and_cancel() {
        let mut s = NukeSeq::new(50.0, plan(), true);
        assert!(s.test);
        assert!((s.countdown_left(50.6).unwrap() - 9.4).abs() < 1e-9, "S.t shows 9.4");
        assert_eq!(s.phase(55.0), Phase::Countdown);
        assert_eq!(on_loading(&s), OnLoading::CancelRefund);
        s.tick(60.0);
        assert!(s.detonated && s.countdown_left(60.01).is_none());
        assert_eq!(on_loading(&s), OnLoading::End);
        assert_eq!((s.phase(60.5), s.phase(62.0), s.phase(70.0)), (Phase::Flash, Phase::Killing, Phase::Aftermath));
        assert!(!s.stuck(76.0, 30.0) && s.stuck(50.0 + 26.5 + 30.1, 30.0));
    }

    #[test]
    fn nuke_logic_post_ramps_known_values() {
        let p = plan();
        let at = |dt: f64| post_at(dt, &p);
        assert_eq!(at(-1.0), PostState::default());
        assert_eq!(at(0.0), PostState { flash: 0.0, amount: 0.0, blend: 0.0 }, "T");
        let a = at(0.25);
        assert_eq!((a.flash, a.amount, a.blend), (1.0, 0.0, 0.0), "T+0.25: full flash, mpnuke starts");
        let b = at(1.5);
        assert!((b.flash - (1.0 - (1.5 - 0.33) / 2.5) as f32).abs() < 1e-5 && (b.amount - 1.25 / 3.0).abs() < 1e-5 && b.blend == 0.0, "T+1.5 {b:?}");
        let c = at(3.25);
        assert!(c.flash == 0.0 && c.amount == 1.0 && (c.blend - 0.35).abs() < 1e-5, "T+3.25 {c:?}");
        let d = at(6.5);
        assert_eq!((d.flash, d.amount, d.blend), (0.0, 1.0, 1.0), "T+6.5: aftermath fully in");
        assert!((at(14.5 + 1.0).amount - 0.5).abs() < 1e-5, "half faded 1 s into the fade");
        assert_eq!(at(16.5).amount, 0.0);
        assert!(!at(17.0).active());
        let mut p2 = p.clone();
        p2.flash = false;
        assert_eq!(post_at(0.2, &p2).flash, 0.0);
    }

    #[test]
    fn nuke_logic_shake_mw2_commented_values() {
        let p = plan();
        let s = NukeSeq::new(0.0, p, false);
        let sp = ShakeParams { scale: 1.0, rot_deg: 2.0, roll_deg: 0.6, pos_m: 1.5, freq_hz: 11.0, mw2_falloff: false };
        assert_eq!(s.plan.quake_at, 11.5, "MW2 nukeEarthquake at timer + 1.5");
        assert_eq!(s.shake(5.0, &sp), ShakeOut::ZERO, "before the detonation");
        assert_eq!(s.shake(10.5, &sp), ShakeOut::ZERO, "after the detonation, before nukeEarthquake");
        assert_eq!(s.shake(11.49, &sp), ShakeOut::ZERO);
        let mut peak = 0.0f32;
        for k in 0..200 {
            let o = s.shake(11.5 + k as f64 * 0.005, &sp);
            peak = peak.max(o.yaw_deg.abs()).max(o.pitch_deg.abs());
        }
        assert!(peak > 0.5 && peak <= 0.6 * 2.0 + 1e-4, "peak {peak} deg (0.6 x 2 deg)");
        assert_ne!(s.shake(20.0, &sp), ShakeOut::ZERO, "still shaking 8.5 s in");
        assert_eq!(s.shake(21.6, &sp), ShakeOut::ZERO, "10 s duration from T+1.5");
    }

    #[test]
    fn nuke_logic_gate_order() {
        let ok = GateIn { enabled: true, data_ok: true, inbound: false, ac130: false, ladder: false, loading: false, dead: false, mode_on: true, available: true, coop_peers: 0 };
        assert_eq!(gate(&ok), Ok(()));
        assert_eq!(gate(&GateIn { inbound: true, ac130: true, ..ok }), Err(Block::Inbound));
        assert_eq!(gate(&GateIn { ac130: true, ..ok }), Err(Block::Ac130));
        assert_eq!(gate(&GateIn { ladder: true, loading: true, ..ok }), Err(Block::Ladder), "M8 B1: after the AC-130, before loading");
        assert_eq!(Block::Ladder.label(), "ladder");
        assert_eq!(gate(&GateIn { loading: true, dead: true, ..ok }), Err(Block::Loading));
        assert_eq!(gate(&GateIn { dead: true, ..ok }), Err(Block::Dead));
        assert_eq!(gate(&GateIn { mode_on: false, ..ok }), Err(Block::ModeOff));
        assert_eq!(gate(&GateIn { available: false, ..ok }), Err(Block::NotEarned));
        assert_eq!(gate(&GateIn { enabled: false, ..ok }), Err(Block::Disabled));
        assert_eq!(gate(&GateIn { data_ok: false, ..ok }), Err(Block::Data));
        // co-op C2: refused with a peer (allow_in_coop = false), before the inbound / AC-130 / earned checks
        assert_eq!(gate(&GateIn { coop_peers: 1, inbound: true, available: false, ..ok }), Err(Block::Coop));
        assert_eq!(gate(&GateIn { coop_peers: 1, data_ok: false, ..ok }), Err(Block::Data));
        assert_eq!(Block::Coop.label(), "coop_session");
        // allow_in_coop = true: gate_peers gives 0, the solo gate applies unchanged
        assert_eq!(gate(&GateIn { coop_peers: crate::coop::gate_peers(2, true), ..ok }), Ok(()));
        assert_eq!(gate(&GateIn { coop_peers: crate::coop::gate_peers(2, false), ..ok }), Err(Block::Coop));
    }

    #[test]
    fn nuke_logic_player_death_order() {
        // no lethal before the protection is confirmed absent, prep before lethal
        let mut pk = PlayerKill::default();
        assert_eq!(pk.step(0.0, false, false, Some(true), false), PkAction::None);
        assert_eq!(pk.step(1.0, true, false, Some(true), false), PkAction::Prep);
        assert_eq!(pk.step(1.1, true, false, Some(true), false), PkAction::RemoveProtect { try_n: 1 });
        assert_eq!(pk.step(1.2, true, false, Some(true), false), PkAction::None, "0.25 s between removals");
        assert_eq!(pk.step(1.4, true, false, Some(false), false), PkAction::None, "absent but not yet due");
        assert_eq!(pk.step(1.5, true, true, None, false), PkAction::RemoveProtect { try_n: 2 }, "unreadable = present");
        assert_eq!(pk.step(1.8, true, true, Some(false), false), PkAction::ApplyLethal { try_n: 1 });
        assert_eq!(pk.step(2.0, true, true, Some(false), false), PkAction::None);
        assert_eq!(pk.step(2.9, true, true, Some(false), true), PkAction::Dead);
        assert_eq!(pk.stage, PkStage::Done);
        // the protection never goes: abandoned after 3 tries, never a lethal
        let mut pk = PlayerKill::default();
        pk.step(0.0, true, false, Some(true), false);
        let mut acts = Vec::new();
        for k in 1..40 {
            acts.push(pk.step(k as f64 * 0.3, true, true, Some(true), false));
        }
        assert!(!acts.iter().any(|a| matches!(a, PkAction::ApplyLethal { .. })), "{acts:?}");
        assert_eq!(acts.iter().filter(|a| matches!(a, PkAction::RemoveProtect { .. })).count(), 3);
        assert!(acts.contains(&PkAction::WarnProtected));
        assert_eq!(pk.stage, PkStage::Aborted);
        // still alive: re-applied each second, 3 times, then the warning
        let mut pk = PlayerKill::default();
        pk.step(0.0, true, false, Some(false), false);
        assert_eq!(pk.step(0.1, true, true, Some(false), false), PkAction::ApplyLethal { try_n: 1 });
        assert_eq!(pk.step(1.2, true, true, Some(false), false), PkAction::ApplyLethal { try_n: 2 });
        assert_eq!(pk.step(2.3, true, true, Some(false), false), PkAction::ApplyLethal { try_n: 3 });
        assert_eq!(pk.step(3.4, true, true, Some(false), false), PkAction::WarnNotDead);
        // killed by an enemy before the lethal time: nothing applied
        let mut pk = PlayerKill::default();
        pk.step(0.0, true, false, Some(true), false);
        assert_eq!(pk.step(0.5, true, false, Some(true), true), PkAction::Dead);
    }

    #[test]
    fn nuke_logic_player_death_missing_sample_is_no_decision() {
        assert_eq!(player_dead_input(None), None, "a missing sample never reads as a death");
        assert_eq!(player_dead_input(Some(0)), Some(true));
        assert_eq!(player_dead_input(Some(-5)), Some(true));
        assert_eq!(player_dead_input(Some(1)), Some(false));
        // Prepared with a missing sample on one tick: the caller skips the step, nothing changes; a real sample decides
        let mut pk = PlayerKill::default();
        assert_eq!(pk.step(0.0, true, false, Some(false), false), PkAction::Prep);
        for now in [0.1, 0.2] {
            if let Some(dead) = player_dead_input(None) {
                pk.step(now, true, true, Some(false), dead);
            }
        }
        assert_eq!(pk.stage, PkStage::Prepared, "no false 'player dead' on a missing sample");
        assert_eq!(pk.step(0.3, true, true, Some(false), player_dead_input(Some(500)).unwrap()), PkAction::ApplyLethal { try_n: 1 });
        assert_eq!(pk.step(0.4, true, true, Some(false), player_dead_input(Some(0)).unwrap()), PkAction::Dead);
    }

    #[test]
    fn nuke_logic_no_player_events_when_spared() {
        let cfg = Nuke { player_dies: false, ..Nuke::default() };
        let s = NukeSeq::new(0.0, Plan::new(&fixture().timeline, &cfg), false);
        assert!(!s.events.iter().any(|e| matches!(e.1, Ev::PlayerPrep | Ev::PlayerLethal)));
    }
}
