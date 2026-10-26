//! M7: the MW2 Tactical Nuke (docs/M7_DESIGN.md section 3). Runs once per task tick on the game's main thread.
//!
//! Per tick: the killstreak counter (kills come from the task's HIT attribution), key 4 -> the gate -> the pure
//! sequence (`mw2er_core::nuke_logic`) -> act on its events:
//! - sounds (MW2 aliases from `converted\nuke`), the HUD counter / countdown / hints, the post pass state;
//! - the detonation: the explosion chain + the wave, the flash (stands in for MW2's `nuke_flash` fx), up to 3
//!   visual-only horizon explosions (priority spawns, skipped while the emitter pool is busy);
//! - the shake from T+1.5 like MW2's `nukeEarthquake` thread (its commented-out values, applied by the view-fx path),
//!   stopped at the player's death;
//! - the kills at T+1.5: a census of every character (by distance + the ChrSets, timed: `census_us`), the deny-list
//!   selection (`nuke_targets`; other havok frames counted as `other_frame`), then the one-shot lethal SpEffect through
//!   the engine (the same gated call as the AC-130 protection), at most `kills_per_tick` per tick, retried for
//!   survivors (a scripted-invincible character only once it reads as not invincible); never an HP write;
//! - run-15 safety: with `nuke.dry_run` (default) or an implausible team histogram (`blocked reason=team_unverified`)
//!   no NPC gets the lethal effect: `NUKE kill would-apply ...` lines only;
//! - the death watch at 10 Hz, at most WATCH_CAP re-reads per pass (round-robin);
//! - the player's death `player_delay_s` later: first person / movement / inputs handed back, the AC-130 protection
//!   removed and confirmed absent, then the same lethal SpEffect (ER's own YOU DIED / grace respawn follows).
//!
//! A loading screen before the detonation cancels and refunds the nuke; after it the sequence ends. F5 / F7 are refused
//! while a sequence runs. A panic inside the nuke tick ends the sequence and disables the nuke for the session
//! (`NUKE FAIL`), the rest of the mod keeps running.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mw2er_core::ac130_shake::{ShakeOut, ShakeParams};
use mw2er_core::config::Config;
use mw2er_core::coop::{COOP_NOTE, COOP_NOTE_OWNED, CoopNote, OnCoop, gate_peers, on_coop};
use mw2er_core::emitter_guard::Kind as SpawnKind;
use mw2er_core::input_logic::CounterReader;
use mw2er_core::killstreak::{ActivateBlock, KillSrc, Killstreak, KsEvent};
use mw2er_core::logfmt;
use mw2er_core::nuke_fx::vis_at;
use mw2er_core::nuke_logic::{
    Block, Ev, GateIn, NukeSeq, OnLoading, PendPoll, PendingMarker, PkAction, PkStage, Plan, PlayerKill, PostState, RemoteAct, RemoteIn, gate, on_loading,
    player_dead_input, remote_marker,
};
use mw2er_core::nuke_targets::{
    Cand, CoopKillSched, CoopRole, Owner, Selection, Stagger, TargetCfg, WatchSched, bullets_allowed, coop_hold, horizon_allowed, npc_kills_armed, select,
    watch_given_up,
};
use mw2er_core::rng::Rng;
use mw2er_core::vec3::V3;

use crate::data::{AC_DATA, NUKE_DATA, NukeData};
use crate::game::ac130_read::{main_player, speffect_count};
use crate::game::chars::{NukeChr, census, reread};
use crate::game::player::PlayerSample;
use crate::game::params;
use crate::guard::guard;
use crate::log::{log_e, log_i, log_w};
use crate::shared::{self, NukeHud, NukePlay, PRESSES_NUKE};
use crate::{spawn, spawn_guard};

/// One tick's inputs (read by the task).
pub struct NukeIn<'a> {
    pub now: f64,
    pub cfg: &'a Config,
    pub mode_on: bool,
    pub in_world: bool,
    /// A loading edge latch (first person's) or not in world.
    pub loading: bool,
    pub world_ticks: u64,
    pub player: Option<&'a PlayerSample>,
    /// An AC-130 ride is starting or running.
    pub ac130_busy: bool,
    /// M8 B1: the player is on a ladder.
    pub ladder: bool,
    /// ER's camera forward (row 2) for the horizon explosions.
    pub cam_fwd: Option<V3>,
    /// The M2 blanking gates hold (the nuke key is hidden from ER).
    pub blank: bool,
    /// Co-op C2: the debounced number of other players in the world (`coop::CoopCtl`).
    pub coop_peers: u32,
    /// Co-op C2: this game's session role (`host` / `client` / `unknown`), read on demand.
    pub coop_role: &'a dyn Fn() -> &'static str,
    /// C3 PASS C (docs/C3_DESIGN.md 3.8): a peer's nuke marker arrived this tick (peer handle, "speffect" | "bullet").
    pub remote_nuke: Option<(u64, &'static str)>,
    /// 2026-10-24: the task's death gate holds (`fp_logic::DeathGate`: dead, or not yet alive again after the respawn);
    /// the nuke key is refused (`reason=dead`) like with hp <= 0.
    pub player_dead: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NukeOut {
    /// First person / movement hand back (the player-death prep).
    pub fp_block: bool,
    /// No shot, no ADS, no reload from the prep on.
    pub fire_block: bool,
    /// F5 / F7 refused: a sequence runs.
    pub busy: bool,
    /// The prep tick: the task cancels a reload / rechamber (`reason=nuke`).
    pub cancel_reload: bool,
    pub shake: ShakeOut,
}

/// The death watch: passes per second and re-reads per pass (bounded main-thread cost).
const WATCH_HZ: f64 = 10.0;
const WATCH_CAP: usize = 32;
/// A target with every pass applied is watched this long after its last application, then given up (a survivor).
const WATCH_GRACE_S: f64 = 2.0;

#[derive(Clone, Copy, Debug)]
struct Target {
    addr: usize,
    npc: i32,
    chr: u32,
    team: u8,
    hp0: i32,
    max_hp: i32,
    applied: u32,
    dead: bool,
    t_dead_ms: f64,
    /// Dry run / unverified teams: the would-apply line was logged; never applied.
    would: bool,
    t_last_apply: f64,
    given_up: bool,
}

impl Target {
    fn new(n: &NukeChr) -> Target {
        Target {
            addr: n.addr,
            npc: n.npc_param_id,
            chr: n.c.character_id,
            team: n.team,
            hp0: n.c.hp,
            max_hp: n.c.max_hp,
            applied: 0,
            dead: false,
            t_dead_ms: 0.0,
            would: false,
            t_last_apply: 0.0,
            given_up: false,
        }
    }
}

pub struct NukeCtl {
    data: Option<Arc<NukeData>>,
    ks: Killstreak,
    seq: Option<NukeSeq>,
    reader: CounterReader,
    rng: Rng,
    disabled: bool,
    rows_ok: Option<bool>,
    in_world: bool,
    prev_hp: Option<i32>,
    // HUD
    hint_until: f64,
    inbound_until: f64,
    refusal: u8,
    refusal_until: f64,
    audio_gen: u32,
    // kills
    stagger: Stagger,
    targets: HashMap<u64, Target>,
    first: Option<Selection>,
    extra_targets: u32,
    t_kill0: f64,
    last_retry_done: bool,
    summary_logged: bool,
    watch: WatchSched,
    /// The NPC kills of this sequence: None = no pass yet, Err(reason) = refused for the whole sequence (sticky).
    npc_armed: Option<Result<(), String>>,
    census_us_max: u64,
    other_frame: u32,
    player_dead: bool,
    // player
    pk: PlayerKill,
    prep_logged: bool,
    // horizon
    horizon_spawned: u32,
    horizon_skipped: u32,
    last_cfg_line: String,
    /// Co-op C2: the one-time HUD note per co-op session.
    coop_note: CoopNote,
    coop_note_until: f64,
    // C3 PASS C (docs/C3_DESIGN.md 3.8): the co-op nuke
    /// This sequence was started by a peer's marker (its handle): no streak was used, nothing is refunded.
    remote: Option<u64>,
    /// Peers in the session when the sequence started (the co-op rules apply when > 0).
    coop_peers_at_start: u32,
    /// The census passes every 0.25 s for `coop_window_s` after the first kill pass (`coop_mode = "owned"`).
    coop_sched: Option<CoopKillSched>,
    /// Review 2026-10-24: this game's session role at the start (the co-op kill rule: the host kills first, a client
    /// holds for `coop_client_fallback_s`, see `nuke_targets::coop_hold`).
    coop_role: CoopRole,
    /// Characters some pass held for the host's game (a client before its fallback).
    held: std::collections::HashSet<u64>,
    /// Held by the latest pass (alive, eligible, never applied by this game yet): `left=` at the window end.
    last_held: usize,
    /// The first pass after a hold logged `NUKE coop fallback ...`.
    fallback_logged: bool,
    /// Our synced nuke marker is wanted until this time (the caller of an owned co-op nuke).
    marker_until: f64,
    /// Review 2026-10-24: this (peer's) sequence started while the local player was dead: kill passes only, no player
    /// death, no shake / flash / vision.
    spectate: bool,
    /// Review 2026-10-24: a peer's marker that met a loading screen (or whose sequence a loading screen cancelled
    /// before the detonation), started on the first tick in the world before its window ends.
    pending_remote: Option<PendingMarker>,
    /// The window end of the running peer's sequence (re-pended with it).
    remote_until: f64,
}

fn ms(a: f64, b: f64) -> f64 {
    (b - a) * 1000.0
}

impl NukeCtl {
    pub fn new(seed: u64) -> Self {
        NukeCtl {
            data: NUKE_DATA.get().cloned(),
            ks: Killstreak::default(),
            seq: None,
            reader: CounterReader::starting_at(PRESSES_NUKE.load(Ordering::SeqCst)),
            rng: Rng::new(seed ^ 0x6e75_6b65_6e75_6b65),
            disabled: false,
            rows_ok: None,
            in_world: false,
            prev_hp: None,
            hint_until: f64::NEG_INFINITY,
            inbound_until: f64::NEG_INFINITY,
            refusal: 0,
            refusal_until: f64::NEG_INFINITY,
            audio_gen: 0,
            stagger: Stagger::default(),
            targets: HashMap::new(),
            first: None,
            extra_targets: 0,
            t_kill0: 0.0,
            last_retry_done: false,
            summary_logged: false,
            watch: WatchSched::default(),
            npc_armed: None,
            census_us_max: 0,
            other_frame: 0,
            player_dead: false,
            pk: PlayerKill::default(),
            prep_logged: false,
            horizon_spawned: 0,
            horizon_skipped: 0,
            last_cfg_line: String::new(),
            coop_note: CoopNote::default(),
            coop_note_until: f64::NEG_INFINITY,
            remote: None,
            coop_peers_at_start: 0,
            coop_sched: None,
            coop_role: CoopRole::Unknown,
            held: std::collections::HashSet::new(),
            last_held: 0,
            fallback_logged: false,
            marker_until: f64::NEG_INFINITY,
            spectate: false,
            pending_remote: None,
            remote_until: f64::NEG_INFINITY,
        }
    }

    /// C3 PASS C: our synced nuke marker SpEffect is wanted now (we called an owned co-op nuke; until the kill window
    /// ends). The marker bullet is spawned once at the activation.
    pub fn marker_wanted(&self, now: f64, cfg: &Config) -> bool {
        self.seq.is_some() && self.remote.is_none() && cfg.nuke.coop_mode == "owned" && now < self.marker_until
    }

    /// The co-op rules of the running sequence: a peer was there at the start (or it came from one) and `owned`.
    fn coop_owned(&self, cfg: &Config) -> bool {
        cfg.nuke.coop_mode == "owned" && (self.coop_peers_at_start > 0 || self.remote.is_some())
    }

    /// Review C3: a peer's nuke runs here (started by its marker): its kills on this PC are not ours (no HIT credit,
    /// damage number or streak point).
    pub fn remote_running(&self) -> bool {
        self.seq.is_some() && self.remote.is_some()
    }

    /// A sequence runs (F5 / F7 refused).
    pub fn busy(&self) -> bool {
        self.seq.is_some()
    }

    /// One attributed kill (the task's HIT attribution: gun, launcher, AC-130).
    pub fn on_kill(&mut self, now: f64, handle: u64, src: KillSrc, player_alive: bool, mode_on: bool, cfg: &Config) {
        if self.disabled || !cfg.nuke.enabled {
            return;
        }
        let src = if self.seq.as_ref().is_some_and(|s| s.detonated) { KillSrc::Nuke } else { src };
        for e in self.ks.on_kill(now, handle, src, player_alive, mode_on, cfg.nuke.streak_cost) {
            match e {
                KsEvent::Counted { n, src } => log_i!("NUKE", "streak n={n}/{} src={}", cfg.nuke.streak_cost, src.label()),
                KsEvent::Earned { streak, life } => {
                    log_i!("NUKE", "earned streak={streak} life={life}");
                    self.hint_until = now + 4.0;
                    self.play("earned", 1.0);
                    let who = match cfg.nuke.announcer.as_str() {
                        "US" => Some("achieve_us"),
                        "UK" => Some("achieve_uk"),
                        _ => None,
                    };
                    if let Some(k) = who {
                        self.play(k, 1.0);
                    }
                }
                KsEvent::Ignored(r) => log_i!("NUKE", "streak ignored reason={} handle={}", r.label(), logfmt::handle(handle)),
                KsEvent::Reset { .. } => {}
            }
        }
    }

    /// A panic in the task (or in this controller): the sequence ends, sounds stop. No game state to restore (the nuke
    /// writes nothing but the engine's SpEffects).
    pub fn emergency_off(&mut self) {
        // key 4 back to ER on every exit path (input_hook keeps blanking while the flag is set and the task ticks)
        shared::WANT_BLANK_NUKE.store(false, Ordering::SeqCst);
        if self.seq.take().is_some() {
            log_w!("NUKE", "end reason=panic (sequence stopped; nothing to restore)");
        }
        self.stop_audio();
        self.stagger.clear();
        self.ks.refund(true);
    }

    fn stop_audio(&mut self) {
        self.audio_gen = self.audio_gen.wrapping_add(1);
        shared::NUKE_AUDIO_GEN.store(self.audio_gen, Ordering::SeqCst);
    }

    /// Plays a nuke alias and its secondary chain (IW4 secondaryaliasname: started together, own start delays).
    fn play(&mut self, key: &str, gain: f32) {
        let Some(d) = self.data.clone() else { return };
        let mut k = key.to_string();
        for _ in 0..8 {
            let Some((idx, s)) = d.file.sounds.iter().enumerate().find(|(_, (n, _))| **n == k).map(|(i, (_, s))| (i, s.clone())) else {
                return;
            };
            let n = s.files.len().max(1);
            let variant = (self.rng.next_u64() % n as u64) as u8;
            let vol = s.vol_min + (s.vol_max - s.vol_min) * self.rng.next_f32();
            let speed = s.pitch_min + (s.pitch_max - s.pitch_min) * self.rng.next_f32();
            shared::play_nuke(NukePlay { key: idx as u8, variant, gain: vol * gain, speed, delay_ms: s.startdelay_ms, generation: self.audio_gen });
            if s.secondary.is_empty() {
                return;
            }
            k = s.secondary.clone();
        }
    }

    fn refuse(&mut self, now: f64, b: Block, test: bool) {
        if b == Block::Ladder {
            log_i!("NUKE", "blocked reason=ladder test={} (ER drives the climb)", test as u8);
        } else if b == Block::Coop {
            // logged by the caller with the peer count
        } else {
            log_i!("NUKE", "blocked reason={} test={}", b.label(), test as u8);
        }
        self.refusal = match b {
            Block::Inbound => 1,
            Block::Ac130 => 2,
            Block::NotEarned => 3,
            Block::Coop => 4,
            _ => 0,
        };
        self.refusal_until = now + 2.0;
    }

    fn log_cfg(&mut self, cfg: &Config) {
        let n = &cfg.nuke;
        let line = format!(
            "cfg enabled={} always_available={} allow_in_coop={} cost={} radius_m={} spare_friendly={} spare_teams={:?} dry_run={} kill_mode={} kills_per_tick={} retry_s={:?} player_dies={} player_delay_s={} flash={} shake={}x{}s horizon={} key={} data={}",
            n.enabled as u8,
            n.always_available as u8,
            n.allow_in_coop as u8,
            n.streak_cost,
            n.radius_m,
            n.spare_friendly as u8,
            n.spare_teams,
            n.dry_run as u8,
            n.kill_mode,
            n.kills_per_tick,
            n.retry_s,
            n.player_dies as u8,
            n.player_delay_s,
            n.flash as u8,
            n.shake_scale,
            n.shake_s,
            n.horizon_count,
            cfg.keys.nuke,
            self.data.is_some() as u8
        );
        if line != self.last_cfg_line {
            log_i!("NUKE", "{line}");
            self.last_cfg_line = line;
        }
    }

    /// One task tick, guarded: a panic ends the sequence and disables the nuke for the session.
    pub fn tick(&mut self, i: &NukeIn) -> NukeOut {
        if self.disabled {
            // disabled after a panic: key 4 stays ER's (never left blanked by the last MW2-mode tick)
            shared::WANT_BLANK_NUKE.store(false, Ordering::SeqCst);
            return NukeOut::default();
        }
        match guard("nuke", || self.tick_inner(i)) {
            Some(o) => o,
            None => {
                self.disabled = true;
                let _ = guard("nuke.emergency", || self.emergency_off());
                log_e!("NUKE", "FAIL panic in the nuke tick; the nuke is disabled for this session (everything else keeps running)");
                NukeOut::default()
            }
        }
    }

    fn tick_inner(&mut self, i: &NukeIn) -> NukeOut {
        if self.data.is_none() {
            self.data = NUKE_DATA.get().cloned();
        }
        let cfg = i.cfg;
        let now = i.now;
        self.log_cfg(cfg);
        let mut out = NukeOut::default();
        // world transitions: the PARAMS check of the nuke rows on each world entry
        if i.in_world != self.in_world {
            self.in_world = i.in_world;
            if i.in_world {
                self.rows_ok = None;
            }
        }
        if i.in_world && self.rows_ok.is_none() && i.world_ticks >= 2 {
            self.rows_ok = Some(match self.data.as_ref().map(|d| params::check_nuke(&d.rows)) {
                Some(Ok(s)) => {
                    log_i!("NUKE", "rows ok {s}");
                    true
                }
                Some(Err(e)) => {
                    log_e!("NUKE", "blocked reason=rows {e}");
                    false
                }
                None => false,
            });
        }
        // the player's death resets the streak (the hp edge, as FP's death signal)
        let hp = i.player.map(|p| p.chr.hp);
        if let (Some(prev), Some(h)) = (self.prev_hp, hp)
            && prev > 0
            && h <= 0
            && let KsEvent::Reset { n } = self.ks.on_player_death(cfg.nuke.keep_earned_on_death)
        {
            log_i!("NUKE", "streak reset reason=death n={n} life={}", self.ks.life_id);
        }
        if hp.is_some() {
            self.prev_hp = hp;
        }
        let alive = hp.is_some_and(|h| h > 0);
        // the key (hidden from ER while MW2 mode drives)
        shared::WANT_BLANK_NUKE.store(i.blank && cfg.nuke.enabled, Ordering::SeqCst);
        let presses = self.reader.take(PRESSES_NUKE.load(Ordering::SeqCst));
        self.coop_note.on_peers(i.coop_peers);
        let gp = gate_peers(i.coop_peers, cfg.nuke.allow_in_coop);
        if presses > 0 && i.blank {
            let available = self.ks.earned || cfg.nuke.always_available;
            let g = GateIn {
                enabled: cfg.nuke.enabled,
                data_ok: self.data.is_some() && self.rows_ok == Some(true),
                inbound: self.seq.is_some(),
                ac130: i.ac130_busy,
                ladder: i.ladder,
                loading: i.loading,
                dead: !alive || i.player_dead,
                mode_on: i.mode_on,
                available,
                coop_peers: gp,
            };
            match gate(&g) {
                Err(Block::Coop) => {
                    log_i!("NUKE", "blocked reason=coop_session peers={} test={} (allow_in_coop=0)", i.coop_peers, cfg.nuke.always_available as u8);
                    self.refuse(now, Block::Coop, cfg.nuke.always_available);
                }
                Err(b) => self.refuse(now, b, cfg.nuke.always_available),
                Ok(()) => match self.ks.activate(cfg.nuke.always_available) {
                    Ok(test) => {
                        self.start(now, cfg, test, i.coop_peers, if i.coop_peers > 0 { (i.coop_role)() } else { "solo" }, None, i.player, false);
                        if self.coop_note.on_activate(i.coop_peers) {
                            self.coop_note_until = now + 6.0;
                            let note = if cfg.nuke.coop_mode == "owned" { COOP_NOTE_OWNED } else { COOP_NOTE };
                            log_i!("NUKE", "coop note shown peers={}: {}", i.coop_peers, note);
                        }
                    }
                    Err(ActivateBlock::Inbound) => self.refuse(now, Block::Inbound, false),
                    Err(ActivateBlock::NotEarned) => self.refuse(now, Block::NotEarned, false),
                },
            }
        }
        // C3 PASS C (docs/C3_DESIGN.md 3.8): a peer's nuke marker starts the sequence here too (no streak used); this
        // game kills the enemies it owns, then its own player
        // review 2026-10-24: a dead player's game still kills the enemies it simulates (spectate); a loading screen keeps
        // the marker pending until the first tick in the world inside its window
        let loading_now = i.loading || !i.in_world;
        let mut marker = i.remote_nuke.map(|(from, via)| (from, via, None));
        if marker.is_none()
            && let Some(p) = self.pending_remote
        {
            match p.poll(now, loading_now) {
                PendPoll::Wait => {}
                PendPoll::Expired => {
                    self.pending_remote = None;
                    log_i!("NUKE", "marker from={} via={} action=ignored why=expired", logfmt::handle(p.from), p.via);
                }
                PendPoll::Due => {
                    self.pending_remote = None;
                    marker = Some((p.from, p.via, Some(p.until)));
                }
            }
        }
        if let Some((from, via, until)) = marker {
            let r = RemoteIn {
                owned: cfg.nuke.coop_mode == "owned",
                allow_in_coop: cfg.nuke.allow_in_coop,
                ready: cfg.nuke.enabled && self.data.is_some() && self.rows_ok == Some(true),
                busy: self.seq.is_some(),
                loading: loading_now,
                alive,
            };
            let until = until.unwrap_or_else(|| now + self.data.as_ref().map_or(0.0, |d| Plan::new(&d.file.timeline, &cfg.nuke).kill_at) + cfg.nuke.coop_window_s);
            match remote_marker(&r) {
                RemoteAct::Ignore(w) => log_i!("NUKE", "marker from={} via={via} action=ignored why={w}", logfmt::handle(from)),
                RemoteAct::Pend => {
                    self.pending_remote = Some(PendingMarker { from, via, until });
                    log_i!("NUKE", "marker from={} via={via} action=pending why=loading until_s={:.1}", logfmt::handle(from), until - now);
                }
                RemoteAct::Start { spectate } => {
                    log_i!("NUKE", "marker from={} via={via} action=start peers={} spectate={}", logfmt::handle(from), i.coop_peers, spectate as u8);
                    self.start(now, cfg, true, i.coop_peers, (i.coop_role)(), Some(from), None, spectate);
                    self.remote_until = until;
                }
            }
        }
        // co-op C2 (allow_in_coop = false): a peer in the world cancels a countdown with refund
        if let Some(seq) = self.seq.as_ref()
            && self.remote.is_none()
            && on_coop(gp, seq.detonated) == OnCoop::CancelRefund
        {
            let test = seq.test;
            self.seq = None;
            self.ks.refund(test);
            self.stop_audio();
            self.stagger.clear();
            self.refusal = 4;
            self.refusal_until = now + 3.0;
            log_i!("NUKE", "cancel reason=coop_session refund={} peers={}", (!test) as u8, i.coop_peers);
        }
        // the sequence
        if let Some(seq) = self.seq.as_ref() {
            if i.loading {
                match on_loading(seq) {
                    OnLoading::CancelRefund => {
                        let test = seq.test;
                        self.seq = None;
                        match self.remote.take() {
                            None => self.ks.refund(test),
                            // review 2026-10-24: a peer's sequence goes back to pending (its kills still due here)
                            Some(from) if now < self.remote_until => {
                                self.pending_remote = Some(PendingMarker { from, via: "repend", until: self.remote_until });
                                log_i!("NUKE", "marker from={} via=repend action=pending why=loading until_s={:.1}", logfmt::handle(from), self.remote_until - now);
                            }
                            Some(_) => {}
                        }
                        self.stop_audio();
                        self.stagger.clear();
                        log_i!("NUKE", "cancel reason=loading refund={}", (!test) as u8);
                    }
                    OnLoading::End => self.end(now, "loading"),
                }
            } else if seq.stuck(now, cfg.nuke.stuck_s) {
                log_w!("NUKE", "WARN stuck stage={} t={:.1}s; forcing the end", seq.phase(now).label(), seq.t(now));
                self.end(now, "stuck");
            }
        }
        if self.seq.is_some() {
            self.step(i, &mut out);
        }
        out.busy = self.seq.is_some();
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, now: f64, cfg: &Config, test: bool, coop_peers: u32, role: &str, remote: Option<u64>, player: Option<&PlayerSample>, spectate: bool) {
        let Some(d) = self.data.clone() else { return };
        let plan = Plan::new(&d.file.timeline, &cfg.nuke);
        let plan = if spectate { plan.spectating() } else { plan };
        self.spectate = spectate;
        self.remote = remote;
        self.coop_peers_at_start = coop_peers;
        self.coop_sched = None;
        self.coop_role = CoopRole::from_label(role);
        self.held.clear();
        self.last_held = 0;
        self.fallback_logged = false;
        // C3 PASS C: the caller of an owned co-op nuke tells the peers (a synced SpEffect while the window lasts, applied
        // by the co-op visibility tick, and one synced marker bullet now)
        self.marker_until = if remote.is_none() && coop_peers > 0 && cfg.nuke.coop_mode == "owned" {
            now + plan.kill_at + cfg.nuke.coop_window_s
        } else {
            f64::NEG_INFINITY
        };
        if self.marker_until > now
            && let Some(p) = player
        {
            let id = mw2er_core::coop_vis::NUKE_MARKER_BULLET;
            let ok = spawn_guard::admit(now, SpawnKind::Nuke, id)
                && spawn::spawn_synced(p.field_ins_handle, id, p.chr.pos + V3::new(0.0, 1.8, 0.0), p.chr.pos_w, V3::new(0.0, -1.0, 0.0)).is_ok();
            log_i!("NUKE", "marker sent sp={} bullet={} ok={} peers={coop_peers}", mw2er_core::coop_vis::SP_NUKE, id, ok as u8);
        }
        self.seq = Some(NukeSeq::new(now, plan, test));
        self.targets.clear();
        self.stagger.clear();
        self.first = None;
        self.extra_targets = 0;
        self.last_retry_done = false;
        self.summary_logged = false;
        self.watch = WatchSched::default();
        self.npc_armed = None;
        self.census_us_max = 0;
        self.other_frame = 0;
        self.player_dead = false;
        self.pk = PlayerKill::default();
        self.prep_logged = false;
        self.horizon_spawned = 0;
        self.horizon_skipped = 0;
        self.stop_audio(); // a fresh generation for this sequence
        log_i!(
            "NUKE",
            "activate test={} streak={} life={} coop_peers={coop_peers} role={role} remote={} coop_mode={} spectate={}",
            test as u8,
            self.ks.streak,
            self.ks.life_id,
            remote.map_or("0".to_string(), logfmt::handle),
            cfg.nuke.coop_mode,
            spectate as u8
        );
    }

    fn end(&mut self, now: f64, reason: &str) {
        if let Some(seq) = self.seq.take() {
            // C3: a peer's nuke never used our streak
            if self.remote.take().is_none() {
                self.ks.sequence_end();
            }
            self.coop_sched = None;
            self.marker_until = f64::NEG_INFINITY;
            self.stagger.clear();
            if reason != "done" {
                self.stop_audio();
            }
            if seq.detonated && !self.summary_logged {
                self.log_summary(now);
            }
            log_i!("NUKE", "end reason={reason} t={:.1}s", seq.t(now));
        }
    }

    fn step(&mut self, i: &NukeIn, out: &mut NukeOut) {
        let now = i.now;
        let cfg = i.cfg;
        let Some(mut seq) = self.seq.take() else { return };
        for e in seq.tick(now) {
            match e {
                Ev::Activate => {
                    self.play("activate_trigger", 1.0);
                    match cfg.nuke.announcer.as_str() {
                        "US" => self.play("use_us", 1.0),
                        "UK" => self.play("use_uk", 1.0),
                        _ => {}
                    }
                    self.inbound_until = now + 3.0;
                    log_i!("NUKE", "countdown t={:.1}", seq.plan.timer_s);
                }
                Ev::Beep(_) => self.play("timer", 1.0),
                Ev::Incoming => {
                    self.play("incoming", 1.0);
                    log_i!("NUKE", "incoming t={:.2}", seq.t(now));
                }
                Ev::Detonate => {
                    self.play("explosion", 1.0);
                    self.play("wave", 1.0);
                    log_i!("NUKE", "detonate t={:.2} test={}", seq.t(now), seq.test as u8);
                }
                Ev::Horizon(k) => self.horizon(i, k),
                Ev::Kill(0) => {
                    self.t_kill0 = now;
                    self.kill_pass(i, 0, &seq);
                    // C3 PASS C: in an owned co-op nuke a census pass every 0.25 s for the window (ownership may move)
                    if self.coop_owned(cfg) {
                        self.coop_sched = Some(CoopKillSched::start(now + 0.25, cfg.nuke.coop_window_s));
                    }
                }
                Ev::Kill(k) => {
                    self.kill_pass(i, k, &seq);
                    if k as usize == seq.plan.retries.len() {
                        self.last_retry_done = true;
                    }
                }
                Ev::PlayerPrep | Ev::PlayerLethal => {}
                Ev::End => {
                    self.seq = Some(seq);
                    self.end(now, "done");
                    return;
                }
            }
        }
        // C3 PASS C: the co-op window passes (numbered 100+)
        if let Some(mut s) = self.coop_sched.take() {
            if s.due(now, 0.25) {
                let k = 100 + s.passes;
                self.coop_sched = Some(s);
                self.kill_pass(i, k, &seq);
            } else if s.active(now) {
                self.coop_sched = Some(s);
            } else {
                log_i!(
                    "NUKE",
                    "coop window end passes={} held={} left={} role={} window={:.1}s",
                    s.passes,
                    self.held.len(),
                    self.last_held,
                    self.coop_role.label(),
                    cfg.nuke.coop_window_s
                );
            }
        }
        // the staggered kills of this tick and the death watch of the applied ones
        self.apply_batch(i, &seq);
        self.watch_targets(now, seq.plan.retries.len());
        if self.last_retry_done && !self.summary_logged && now - self.t_kill0 > seq.plan.retries.last().copied().unwrap_or(0.0) - seq.plan.kill_at + 1.0 {
            self.log_summary(now);
        }
        // the player's death
        if seq.plan.player_dies {
            let t = seq.t(now);
            let prep_due = t >= seq.plan.player_prep_at;
            let lethal_due = t >= seq.plan.player_lethal_at;
            let chr = main_player();
            let protect = AC_DATA.get().map_or(0, |d| d.rows.protect_speffect_id);
            let present = if protect == 0 { Some(false) } else { speffect_count(chr, protect).map(|n| n > 0) };
            // no sample this tick (a transient read failure) = no decision: never a 'player dead' without a real hp <= 0
            // (stuck_s ends a sequence that never gets a sample again)
            let act = match player_dead_input(i.player.map(|p| p.chr.hp)) {
                Some(dead) => self.pk.step(now, prep_due, lethal_due, present, dead && prep_due),
                None => PkAction::None,
            };
            match act {
                PkAction::Prep => {
                    out.cancel_reload = true;
                    log_i!("NUKE", "player prep fp=off move=off fire=off protect_present={}", present.map_or("?".into(), |p| (p as u8).to_string()));
                }
                PkAction::RemoveProtect { try_n } => {
                    // SAFETY: the live local player read this tick on the game thread; gated inside.
                    let ok = unsafe { crate::ac130::remove_speffect(chr, protect) };
                    log_i!("NUKE", "player protect remove id={protect} try={try_n} call_ok={}", ok as u8);
                }
                PkAction::ApplyLethal { try_n } => {
                    let id = self.data.as_ref().map_or(0, |d| d.rows.lethal_speffect_id);
                    // SAFETY: as above; the protection was confirmed absent this tick (PlayerKill's order).
                    let ok = unsafe { crate::ac130::apply_speffect(chr, id) };
                    log_i!("NUKE", "player lethal ok={} try={try_n} id={id} protect_present=0", ok as u8);
                }
                PkAction::Dead => {
                    self.player_dead = true;
                    log_i!("NUKE", "player dead t_ms={:.0}", if self.pk.t_lethal > 0.0 { ms(self.pk.t_lethal, now) } else { 0.0 })
                }
                PkAction::WarnProtected => log_w!("NUKE", "WARN player_protected (the protection SpEffect is still present after 3 removals; the player is spared)"),
                PkAction::WarnNotDead => log_w!("NUKE", "WARN player_not_dead (3 lethal applications; no HP write)"),
                PkAction::None => {}
            }
            let prepped = !matches!(self.pk.stage, PkStage::Waiting);
            out.fp_block = prepped;
            out.fire_block = prepped;
        }
        // the shake (the view-fx path applies it; from T+1.5 like MW2, stopped at the player's death) and the HUD
        if cfg.nuke.shake && !self.player_dead && !self.spectate {
            let sh = cfg.ac130.shake.params(1.0);
            let p = ShakeParams { scale: 1.0, rot_deg: sh.rot_deg, roll_deg: sh.roll_deg, pos_m: sh.pos_m, freq_hz: sh.freq_hz, mw2_falloff: false };
            out.shake = seq.shake(now, &p);
        }
        self.seq = Some(seq);
    }

    /// The census + selection of one kill pass (0 = the first at T+1.5, k = retry k).
    fn kill_pass(&mut self, i: &NukeIn, k: u32, seq: &NukeSeq) {
        let cfg = i.cfg;
        let Some(p) = i.player else {
            log_w!("NUKE", "kill pass {k} skipped: no player sample");
            return;
        };
        let t_census = std::time::Instant::now();
        let Some(c) = census(1024) else {
            log_w!("NUKE", "kill pass {k} skipped: WorldChrMan unreadable");
            return;
        };
        let census_us = t_census.elapsed().as_micros() as u64;
        self.census_us_max = self.census_us_max.max(census_us);
        // every character goes to select(): another havok frame is counted (other_frame), never dropped silently
        let cands: Vec<Cand> = c.chars.iter().map(|n| cand(n, p, &c.player_sets, c.main_player)).collect();
        // C3 PASS C, review 2026-10-24: an owned co-op nuke has no radius (coop_everyone); the host's game kills
        // first, a client holds its kills for coop_client_fallback_s after its first kill pass, then kills every enemy
        // still alive (chr_update_type is not the owner: it read 4 for every enemy in both games and in solo)
        let owned = self.coop_owned(cfg);
        let since = (i.now - self.t_kill0).max(0.0);
        let hold = owned && coop_hold(self.coop_role, since, cfg.nuke.coop_client_fallback_s, cfg.nuke.coop_window_s);
        let tc = TargetCfg::from_config(cfg).with_coop(if owned { self.coop_peers_at_start.max(1) } else { 0 }, owned, cfg.nuke.coop_everyone, hold);
        let sel = select(&cands, &tc);
        let mut net: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for n in &c.chars {
            *net.entry(n.net_auth).or_insert(0) += 1;
        }
        let dist_auth = c.chars.iter().filter(|n| n.dist_auth).count();
        let upd_txt = sel.by_owner.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(",");
        let net_txt = net.iter().take(6).map(|(k, v)| format!("{}:{v}", if *k == u32::MAX { "?".to_string() } else { k.to_string() })).collect::<Vec<_>>().join(",");
        let was_held = !self.held.is_empty();
        self.held.extend(sel.held.iter().copied());
        self.last_held = sel.held.len();
        if owned && !hold && was_held && !self.fallback_logged {
            self.fallback_logged = true;
            log_i!(
                "NUKE",
                "coop fallback pass={k} role={} since_s={since:.2} alive={} held_before={} (the host's kills not observed for these)",
                self.coop_role.label(),
                sel.kill.len() + sel.deferred.len(),
                self.held.len()
            );
        }
        self.other_frame = self.other_frame.max(sel.other_frame);
        let by_team: Vec<String> = sel.by_team.iter().map(|(t, n)| format!("{t}:{n}")).collect();
        log_i!(
            "NUKE",
            "census pass={k} by_distance={} chrsets={} union={} in_radius={} kill={} deferred={} skipped={} other_frame={} by_team={{{}}} radius_m={} spare_friendly={} census_us={census_us} upd={{{upd_txt}}} net_auth={{{net_txt}}} dist_auth={dist_auth} held={} role={} hold={} coop={} no_radius={}",
            c.by_distance,
            c.chrsets,
            c.chars.len(),
            sel.in_radius,
            sel.kill.len(),
            sel.deferred.len(),
            sel.skipped(),
            sel.other_frame,
            by_team.join(","),
            tc.radius_m,
            tc.spare_friendly as u8,
            sel.held.len(),
            self.coop_role.label(),
            hold as u8,
            owned as u8,
            tc.no_radius as u8
        );
        if !sel.held.is_empty() && (k == 0 || k == 1 || k == 101) {
            let hs: Vec<String> = sel.held.iter().take(8).map(|h| logfmt::handle(*h)).collect();
            log_i!("NUKE", "skip reason=await_host pass={k} n={} role={} chr=[{}]", sel.held.len(), self.coop_role.label(), hs.join(","));
        }
        if k == 0 {
            for (r, n) in &sel.skips {
                log_i!("NUKE", "skip reason={} n={n}", r.label());
            }
            if sel.other_frame > 0 {
                log_i!("NUKE", "skip reason=other_frame n={} (another havok frame: distance unknown, not killed)", sel.other_frame);
            }
        }
        // arming (sticky for the sequence): a dry run, or teams that look wrong on any pass, refuse every NPC kill
        let armed = npc_kills_armed(cfg.nuke.dry_run, tc.spare_friendly, &sel.by_team, &tc.spare_teams);
        match (&self.npc_armed, &armed) {
            (None | Some(Ok(())), Err(why)) => {
                if why == "dry_run" {
                    log_i!("NUKE", "kill dry_run=1 pass={k}: no NPC gets the lethal effect (nuke.dry_run = true; would-apply lines only)");
                } else {
                    log_e!("NUKE", "blocked reason={why} pass={k} by_team={{{}}}: no NPC gets the lethal effect this sequence", by_team.join(","));
                }
                self.npc_armed = Some(armed.clone());
            }
            (None, Ok(())) => {
                log_i!("NUKE", "kill armed pass={k} (dry_run=0, teams plausible)");
                self.npc_armed = Some(Ok(()));
            }
            _ => {}
        }
        let by_handle: HashMap<u64, &NukeChr> = c.chars.iter().map(|n| (n.c.handle, n)).collect();
        let mut queue = Vec::new();
        // every pass queues only the eligible-now (sel.kill): new characters, survivors of earlier applications, and
        // deferred ones once they read as not invincible; a character that is invincible right now is never queued
        for &h in &sel.kill {
            let Some(n) = by_handle.get(&h) else { continue };
            match self.targets.get_mut(&h) {
                Some(t) if t.dead || t.would || t.given_up => {}
                Some(t) => {
                    t.addr = n.addr;
                    queue.push(h);
                }
                None => {
                    if k > 0 {
                        self.extra_targets += 1;
                    }
                    self.targets.insert(h, Target::new(n));
                    queue.push(h);
                }
            }
        }
        // the deferred (invincible right now) are targets too: survivors unless a later pass finds them killable
        for h in &sel.deferred {
            if let Some(n) = by_handle.get(h) {
                if !self.targets.contains_key(h) {
                    if k > 0 {
                        self.extra_targets += 1;
                    }
                    log_i!("NUKE", "kill deferred handle={} npc={} chr=c{:04} team={} why=invincible pass={k}", logfmt::handle(*h), n.npc_param_id, n.c.character_id, n.team);
                }
                self.targets.entry(*h).or_insert_with(|| Target::new(n));
            }
        }
        if k == 0 {
            self.first = Some(sel);
        }
        self.stagger.push_all(&queue);
        let _ = seq;
    }

    /// Up to `kills_per_tick` lethal applications this tick (SpEffect; bullets with kill_mode bullet / both).
    fn apply_batch(&mut self, i: &NukeIn, seq: &NukeSeq) {
        if self.stagger.is_empty() {
            return;
        }
        let cfg = i.cfg;
        let Some(d) = self.data.clone() else { return };
        let Some(p) = i.player else { return };
        let mode = cfg.nuke.kill_mode.as_str();
        let armed = matches!(self.npc_armed, Some(Ok(())));
        let mut bullets_left = if mode == "speffect" || !armed {
            0
        } else {
            bullets_allowed(spawn_guard::live_estimate(i.now), cfg.nuke.bullets_per_tick, cfg.spawn.emitter_throttle)
        };
        for h in self.stagger.take(cfg.nuke.kills_per_tick) {
            let Some(t) = self.targets.get(&h).copied() else { continue };
            // the same live character (the address may have been reused since the census)
            let Some(n) = reread(t.addr).filter(|n| n.c.handle == h) else {
                log_i!("NUKE", "kill skipped handle={} why=gone", logfmt::handle(h));
                continue;
            };
            if n.c.hp <= 0 || n.dead_flag() {
                continue;
            }
            // scripted invincibility (phase change, area load) since the census: never applied over it; a later pass
            // queues it again once it reads as not invincible
            if n.invincible() {
                log_i!("NUKE", "kill deferred handle={} npc={} chr=c{:04} team={} why=invincible t_ms={:.0}", logfmt::handle(h), t.npc, t.chr, t.team, ms(self.t_kill0, i.now));
                continue;
            }
            if !armed {
                // run-15 safety: dry run / unverified teams: log what would happen, apply nothing
                if let Some(tm) = self.targets.get_mut(&h) {
                    tm.would = true;
                }
                log_i!(
                    "NUKE",
                    "kill would-apply handle={} npc={} chr=c{:04} team={} hp={} max_hp={} reason={} t_ms={:.0}",
                    logfmt::handle(h),
                    t.npc,
                    t.chr,
                    t.team,
                    n.c.hp,
                    t.max_hp,
                    match &self.npc_armed {
                        Some(Err(w)) => w.split_whitespace().next().unwrap_or("refused").to_string(),
                        _ => "unarmed".to_string(),
                    },
                    ms(self.t_kill0, i.now)
                );
                continue;
            }
            let mut via = Vec::new();
            if mode != "bullet" {
                // SAFETY: a live ChrIns of the game's own lists (re-read this tick, same handle, not invincible), main
                // thread; gated inside.
                if unsafe { crate::ac130::apply_speffect(t.addr, d.rows.lethal_speffect_id) } {
                    via.push("speffect");
                }
            }
            if mode != "speffect" && bullets_left > 0 && spawn_guard::admit(i.now, SpawnKind::Nuke, d.rows.lethal_bullet_id) {
                let chest = n.c.pos + V3::new(0.0, 1.2, 0.0);
                if spawn::spawn(p.field_ins_handle, d.rows.lethal_bullet_id, chest + V3::new(0.0, 0.4, 0.0), p.chr.pos_w, V3::new(0.0, -1.0, 0.0)).is_ok() {
                    via.push("bullet");
                    bullets_left -= 1;
                }
            }
            if let Some(tm) = self.targets.get_mut(&h) {
                tm.applied += 1;
                tm.t_last_apply = i.now;
            }
            log_i!(
                "NUKE",
                "kill apply handle={} npc={} chr=c{:04} team={} hp={} max_hp={} via={} try={} t_ms={:.0}",
                logfmt::handle(h),
                t.npc,
                t.chr,
                t.team,
                n.c.hp,
                t.max_hp,
                if via.is_empty() { "none".to_string() } else { via.join("+") },
                t.applied + 1,
                ms(self.t_kill0, i.now)
            );
        }
        let _ = seq;
    }

    /// Confirms deaths (hp 0 or the death flag) of the characters the nuke applied to: 10 Hz, at most WATCH_CAP
    /// re-reads per pass (round-robin over a stable order); a target with every pass applied is given up after
    /// WATCH_GRACE_S (it stays a survivor in the summary).
    fn watch_targets(&mut self, now: f64, retries: usize) {
        let t0 = self.t_kill0;
        let mut keys: Vec<u64> = self.targets.iter().filter(|(_, t)| !t.dead && !t.given_up && t.applied > 0).map(|(h, _)| *h).collect();
        keys.sort_unstable();
        for idx in self.watch.pick(now, WATCH_HZ, keys.len(), WATCH_CAP) {
            let h = keys[idx];
            let Some(t) = self.targets.get_mut(&h) else { continue };
            let gone = match reread(t.addr) {
                Some(n) if n.c.handle == h => n.c.hp <= 0 || n.dead_flag(),
                _ => false,
            };
            if gone {
                t.dead = true;
                t.t_dead_ms = ms(t0, now);
                log_i!(
                    "NUKE",
                    "kill handle={} npc={} chr=c{:04} team={} hp={}->0 max_hp={} t_ms={:.0}",
                    logfmt::handle(h),
                    t.npc,
                    t.chr,
                    t.team,
                    t.hp0,
                    t.max_hp,
                    t.t_dead_ms
                );
            } else if watch_given_up(t.applied, retries, now - t.t_last_apply, WATCH_GRACE_S) {
                t.given_up = true;
            }
        }
    }

    fn log_summary(&mut self, now: f64) {
        self.summary_logged = true;
        let killed = self.targets.values().filter(|t| t.dead).count();
        let would = self.targets.values().filter(|t| !t.dead && t.would).count();
        let survivors: Vec<(&u64, &Target)> = self.targets.iter().filter(|(_, t)| !t.dead && !t.would).collect();
        for (h, t) in &survivors {
            let n = reread(t.addr).filter(|n| n.c.handle == **h);
            let why = match n {
                Some(n) if n.invincible() => "invincible",
                Some(n) if n.c.hp > 0 && t.applied > 0 => "hp>0 (immortal?)",
                Some(_) if t.applied == 0 => "never_applied",
                Some(_) => "hp>0",
                None => "gone",
            };
            log_w!(
                "NUKE",
                "survivor handle={} npc={} chr=c{:04} team={} hp={} max_hp={} applied={} why={why}",
                logfmt::handle(**h),
                t.npc,
                t.chr,
                t.team,
                n.map_or(-1, |n| n.c.hp),
                t.max_hp,
                t.applied
            );
        }
        let (in_radius, skipped) = self.first.as_ref().map_or((0, 0), |s| (s.in_radius, s.skipped()));
        log_i!(
            "NUKE",
            "kills total={} killed={killed} survivors={} skipped={skipped} in_radius={} would_apply={would} armed={} other_frame={} census_us_max={} horizon={}/{} t_ms={:.0}",
            self.targets.len(),
            survivors.len(),
            in_radius + self.extra_targets,
            match &self.npc_armed {
                Some(Ok(())) => "1".to_string(),
                Some(Err(w)) => format!("0({})", w.split_whitespace().next().unwrap_or("refused")),
                None => "-".to_string(),
            },
            self.other_frame,
            self.census_us_max,
            self.horizon_spawned,
            self.horizon_spawned + self.horizon_skipped,
            ms(self.t_kill0, now)
        );
        if self.coop_peers_at_start > 0 || self.remote.is_some() {
            let c = shared::config();
            log_i!(
                "NUKE",
                "coop kills local={killed} left={} held={} role={} fallback_s={:.2} window={:.1}s peers={} remote={}",
                self.last_held,
                self.held.len(),
                self.coop_role.label(),
                c.nuke.coop_client_fallback_s.min(c.nuke.coop_window_s),
                c.nuke.coop_window_s,
                self.coop_peers_at_start,
                self.remote.map_or("0".to_string(), logfmt::handle)
            );
        }
    }

    fn horizon(&mut self, i: &NukeIn, k: u32) {
        let Some(d) = self.data.clone() else { return };
        let (Some(p), Some(fwd)) = (i.player, i.cam_fwd) else { return };
        let live = spawn_guard::live_estimate(i.now);
        if !horizon_allowed(live, i.cfg.nuke.horizon_max_live) {
            self.horizon_skipped += 1;
            log_i!("NUKE", "horizon skipped k={k} why=emitters live={}", live.map_or("?".into(), |l| l.to_string()));
            return;
        }
        // MW2: 5000 units ahead of the player, flattened forward; ours: +-20 deg yaw spread, 30 m up [I visible]
        let flat = V3::new(fwd.x, 0.0, fwd.z).normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
        let yaw = [0.0f32, -20.0, 20.0][(k as usize) % 3].to_radians();
        let (s, c) = yaw.sin_cos();
        let dir = V3::new(flat.x * c - flat.z * s, 0.0, flat.x * s + flat.z * c);
        let dist = (d.file.timeline.nuke_distance_in * 0.0254) as f32;
        let at = p.chr.pos + dir * dist + V3::new(0.0, 30.0, 0.0);
        if !spawn_guard::admit(i.now, SpawnKind::Nuke, d.rows.horizon_bullet_id) {
            self.horizon_skipped += 1;
            return;
        }
        match spawn::spawn(p.field_ins_handle, d.rows.horizon_bullet_id, at, p.chr.pos_w, V3::new(0.0, -1.0, 0.0)) {
            Ok(_) => {
                self.horizon_spawned += 1;
                log_i!("NUKE", "horizon spawned={} k={k} at={} dist_m={dist:.0}", self.horizon_spawned, logfmt::v3(at));
            }
            Err(e) => {
                self.horizon_skipped += 1;
                log_w!("NUKE", "horizon skipped k={k} why=spawn {e}");
            }
        }
    }

    /// The HUD / post-pass part of the snapshot.
    pub fn hud(&self, now: f64, cfg: &Config, mode_on: bool, ac130_active: bool) -> NukeHud {
        let mut h = NukeHud::empty();
        let detonated = self.seq.as_ref().is_some_and(|s| s.detonated);
        h.show_counter = cfg.nuke.enabled && mode_on && !ac130_active && !detonated && !self.disabled;
        h.streak = self.ks.streak;
        h.cost = cfg.nuke.streak_cost;
        h.earned = self.ks.earned;
        h.test = cfg.nuke.always_available;
        h.countdown = self.seq.as_ref().and_then(|s| s.countdown_left(now)).map(|x| x as f32);
        h.inbound_until = self.inbound_until;
        h.hint_until = self.hint_until;
        h.refusal = self.refusal;
        h.refusal_until = self.refusal_until;
        h.coop_note_until = self.coop_note_until;
        if let (Some(s), Some(d)) = (self.seq.as_ref(), self.data.as_ref())
            && s.detonated
            && !self.spectate
        {
            let post: PostState = s.post(now);
            h.post = post;
            h.vis = vis_at(&d.file, &post);
        }
        h
    }

}

fn cand(n: &NukeChr, p: &PlayerSample, sets: &std::collections::HashSet<usize>, main: usize) -> Cand {
    Cand {
        handle: n.c.handle,
        npc_param_id: n.npc_param_id,
        character_id: n.c.character_id,
        team: n.team,
        chr_type: n.chr_type,
        hp: n.c.hp,
        max_hp: n.c.max_hp,
        dead_flag: n.dead_flag(),
        invincible: n.invincible(),
        active: n.active(),
        dist_m: (n.c.pos - p.chr.pos).len(),
        in_player_sets: sets.contains(&n.addr),
        is_main_player: n.addr == main || n.c.handle == p.chr.handle,
        same_frame: n.c.pos_w == p.chr.pos_w,
        owner: Owner::from_update_type(n.upd),
    }
}
