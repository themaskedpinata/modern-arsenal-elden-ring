//! C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance of MW2 mode's hidden ER weapon.
//!
//! With `[coop] er_stance = "crossbow"`, in third person with MW2 mode on, the active right weapon's EquipParamWeapon
//! row gets the crossbow motion set: five motion fields (`mw2er_core::er_stance::STANCE_FIELDS`) written through
//! `game::write` at the address of the row (a read-only `SoloParamRepository::get` lookup), the originals stored per row
//! and written back at every exit (mode off, first person, AC-130, nuke, world leave, the panic path, flag off, a
//! weapon switch, a stray bullet). `weaponCategory` / `wepType` stay the weapon's own, so ER has no reason to run its
//! crossbow shoot / reload logic [I2]; R3 melee is suppressed while the stance is active. ER picks the bank up on a grip
//! change [I1]: the pad's two-hand chord (Y held + RB inside it) or, without a pad, E held + LMB [I4].
//!
//! Peers (`stance_peers`): each PC patches its own copy of a peer's active right row while the peer carries 79001090,
//! except a row equal to any weapon row we have equipped (left hand and inactive slots included) other than our own
//! stanced active row (`shared_row`). Our own stance is refused (`shared_left`) while a left slot uses the active right
//! row. A row that cannot be patched (missing, unreadable, already the crossbow values) is refused for the session
//! (logged once, never retried, our stance stays off for it).
//!
//! Every byte read is non-faulting; every write is `game::write` (u8 / u16 of a heap param row; `arm_style` only with the
//! `stance_refresh = "arm_style"` probe). Nothing here touches the regulation file.
//!
//! C6 aim 4 (work/c6/aim4.md, docs/C6_HOLD_DESIGN.md part 14): `[coop] tp_ads_pose = "er_aim"` (default off). While the
//! stance holds the weapon two-handed in third person and MW2 ADS is held (non-scoped guns), our own stanced row also
//! gets weaponCategory 11 / wepType 55 (`AIM_FIELDS`, originals stored, re-read before every write / restore, foreign =
//! left alone + stance disabled) and ER's aim input is held (pad: the LB synth; keyboard: RMB in ER's mouse buffer), so
//! ER raises the crossbow to the shoulder. Confirmed by `ChrIns.chr_flags1c5.precision_shooting` within 0.5 s, else
//! released + restored (`HOLD aim fail reason=no_state`, two = off for the session). Released first and restored once
//! the state is clear at ADS release; at once on every stance exit. No chord while the fields are written (`aim`).
//! The refresh: `dead` refuses chords, a dead player's chord is void, death / nuke / world-leave exits drop their chords,
//! two failed exit chords give up (`HOLD stance exit_chords gave_up`) and never disable the stance.

use mw2er_core::config::Config;
use mw2er_core::er_stance::{
    AIM_FIELDS, ARM_RIGHT_BOTH, AIR_VY_MPS, AimCtl, AimEv, AimIn, AimVals, BUSY_MOVE_MPS, CHORD_CHECK_S, CHORD_RETRY_S, ChordVerdict, ExitDefer, Hysteresis,
    OFF_WEAPON_CATEGORY, OFF_WEP_TYPE, OwnStance, PRECISION_SHOOTING, PeerStance, RowState, STANCE_FIELDS, Stray, StrayGuard, Vals, WantIn, aim_text,
    aim_vals, aim_wanted, chord_refusal, chord_took, chord_verdict, classify_aim, equipped_rows, exit_chord_busy, exit_deferrable,
    exit_drops_chords, exit_target, own_shared_left, row_of, skip_reason, stance_patch_set, stance_wanted, vals_text,
};
// C6 part 16.9: the ignored chord (ER busy, no grip-switch animation)
use mw2er_core::er_stance::{HeadAge, IgnoredBackoff, chord_ignored, chord_played, chord_verdict_label, er_busy};
// C6 part 16 addendum: an aborted chord ER already played finishes; one line per aim episode
use mw2er_core::er_stance::{EpisodeIn, EpisodeLog, abort_finishes, aim_anim};
// C6 part 17: the polearm set and the third-person guard ADS
use mw2er_core::er_stance::{GuardCtl, GuardEpisode, GuardSample, GuardYawIn, StanceSet, classify_set, guard_yaw_wanted};
// C6 part 17 review: the guard's carry only on ER's settled idle
use mw2er_core::er_stance::guard_carry_ok;

use crate::carrier::CarrierView;
use mw2er_core::logfmt;
use mw2er_core::vec3::V3;

use crate::game::chars::{ASM_ARM_STYLE, ArmRead, read_arm};
use crate::log::{log_i, log_w};

/// A chord whose pad event never arrives (no pad read) is given up after this long.
const CHORD_TIMEOUT_S: f64 = 1.5;
/// `HOLD melee suppressed` at most this often.
const MELEE_LOG_S: f64 = 30.0;
/// The keyboard route is used while ER polled the keyboard this recently.
const KBD_FRESH_MS: u64 = 250;

/// What the task hands in every tick.
pub struct StanceIn<'a> {
    pub now: f64,
    pub cfg: &'a Config,
    pub in_world: bool,
    pub mode_on: bool,
    pub fp_on: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub latched: bool,
    pub menu_open: bool,
    pub er_prompt: bool,
    /// Our PlayerIns (0 = none), handle and position.
    pub player: usize,
    pub handle: u64,
    pub pos: Option<V3>,
    /// (handle, PlayerIns, carries the MW2-mode marker) of every peer (`CoopVis::peer_states`).
    pub peers: Vec<(u64, usize, bool)>,
    /// MW2 ADS held (the busy rule) and the camera / body yaw for the `HOLD er_aim` lines.
    pub ads_held: bool,
    /// MW2 fire held (review C6 pose: a ran-out deferred exit's chords wait while busy in first person).
    pub fire_held: bool,
    pub cam_fwd: Option<V3>,
    pub body_fwd: Option<V3>,
    /// C6 aim 4: the player is dead (hp <= 0, death_flag, the death camera): chords are refused (`dead`), never counted.
    pub dead: bool,
    /// C6 aim 4: `chr_flags1c5` as read this tick (bit 1 = ER's aim state), None = unreadable.
    pub flags1c5: Option<u8>,
    /// C6 aim 4: MW2 ADS held AND allowed (not sprinting, the gun not lowered): the aim pose's ADS.
    pub ads_aim: bool,
    /// C6 aim 4: the gun's scope takes third-person ADS (`tp_scope_peek` with an overlay gun): no aim pose.
    pub scoped: bool,
    /// C6 aim 4: riding Torrent.
    pub mounted: bool,
    /// C6 aim 4 probe: `CSCamera` camera type (None = unreadable).
    pub camera_type: Option<u32>,
    /// C6 round 6: ER is still spawning the player after a world enter (`er_stance::spawn_settling`): chords are refused
    /// (`spawn`, never counted) and the stance does not enter (`HOLD stance skip reason=spawn`).
    pub settling: bool,
    /// C6 part 16: the real crossbow carrier as it stands this tick (ticked before the stance).
    pub carrier: CarrierView,
    /// C6 part 17: our first-person read of this tick (the physics module, its orientations, lock-on, action flags,
    /// throw) for the guard's body-yaw write; None = unreadable (no write).
    pub fp: Option<crate::game::fp_read::FpPlayer>,
    /// C6 part 17: the gun's aim align this frame (deg, clamped) as coop_vis drew it (it ticks before the stance).
    pub guard_align: Option<(f32, bool)>,
    /// C6 part 17 addendum: the guard frame the gun used this frame ("builtin" | "auto" | "fixed"; None = none / off).
    pub guard_corr: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
struct Patched {
    row: u32,
    addr: usize,
    orig: Vals,
    /// 0 = ours, else the peer's handle that brought it in.
    peer: u64,
    /// C6 aim 4: the original `[category, type]` while the aim fields are written on this (our own) row.
    aim_orig: Option<AimVals>,
    /// C6 part 17: the motion set written (the restore re-reads against it; a hot set change restores the row first).
    set: StanceSet,
}

#[derive(Clone, Copy, Debug)]
enum Route {
    Pad(u32),
    Kb,
}

/// C6 aim 4: how ER's aim input is held for the episode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AimRoute {
    Pad,
    Kb,
}

impl AimRoute {
    fn label(self) -> &'static str {
        match self {
            AimRoute::Pad => "pad",
            AimRoute::Kb => "kb",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Inflight {
    route: Route,
    t: f64,
    arm_before: u32,
    /// When the chord ended (pad Done / keyboard 0.3 s); `arm_style` is read CHORD_CHECK_S after it.
    done_t: Option<f64>,
    /// C6 aim 4: the chord's own row (the stanced row at an enter, the restored row at an exit), an exit chord, and
    /// whether the player was dead at any tick of it (then it is void, aim4.md 5).
    row: Option<u32>,
    exit: bool,
    dead_seen: bool,
    /// C6 part 16.9: ER's grip-switch animation (2904x-2907x) seen in its queue during the chord window; whether the
    /// queue was readable at all.
    played: bool,
    queue_ok: bool,
}

/// C6 aim 4 probe: at most this many `HOLD aim probe` lines per session, at most one per PROBE_EVERY_S.
const PROBE_MAX: u32 = 300;
const PROBE_EVERY_S: f64 = 0.1;
/// C6 round 7 probe P-A7: at most this many `HOLD aim pose` lines per session, one per POSE_EVERY_S while ER aims.
const POSE_MAX: u32 = 400;
const POSE_EVERY_S: f64 = 0.25;
/// The left palm bone (as `coop_vis`'s `HOLD axes lh_d=`).
const LEFT_HAND_BONE: u32 = 29;
/// C6 part 17: ER's weapon frame is sampled this often while a guard is held (the carry every POSE_EVERY_S).
const GUARD_SAMPLE_S: f64 = 0.1;

pub struct ErStance {
    patched: Vec<Patched>,
    hyst: Hysteresis,
    /// The own active row while the stance is active, and the grip to return to.
    own_row: Option<u32>,
    saved_arm: Option<u32>,
    /// Send chords until arm_style is the target; `cycle` = one extra 3 -> 1 -> 3 so the bank re-resolves.
    cycle: bool,
    inflight: Option<Inflight>,
    next_chord: f64,
    chord_fails: u32,
    refused_logged: Option<&'static str>,
    disabled: Option<&'static str>,
    skip_logged: Option<&'static str>,
    shared_noted: Vec<u32>,
    /// Rows whose raw bytes matched the vendored accessors (true) or did not (false: never patched, logged once).
    checked_rows: Vec<(u32, bool)>,
    /// Rows a patch soft-failed on (missing / unreadable / already the stance values): never retried this session.
    refused_rows: Vec<u32>,
    peer_logged: Vec<(u64, u32, &'static str)>,
    mode_on_t: Option<f64>,
    prev_pos: Option<(V3, f64)>,
    vy: f32,
    /// Review C6 pose: the last tick the player moved horizontally (above BUSY_MOVE_MPS).
    last_move_t: f64,
    stray: StrayGuard,
    use_item_counts: (u32, u32),
    last_use_item: Option<f64>,
    melee_seen: u32,
    melee_log_t: f64,
    er_aim: Option<(f32, f32)>,
    /// `stance_refresh = "arm_style"`: the value we overwrote.
    arm_written: Option<(usize, u32)>,
    /// C6 attach 2: the grip read this tick (`posed`).
    last_arm: u32,
    /// C6 pose 3: the exit chords wait `stance_exit_defer_s`; a re-enter before then sends none (`HOLD stance resume`).
    defer: ExitDefer,
    /// C6 aim 4: the row the next chords belong to (the stanced row while active, the exit's row after an exit).
    chord_row: Option<u32>,
    /// C6 aim 4: the third-person ADS -> ER crossbow aim episode, its input route.
    aim: AimCtl,
    aim_route: Option<AimRoute>,
    aim_disabled_logged: bool,
    /// C6 aim 4 probe: the last logged (anim, prec, arm, row, type, cat, camtype, mode, ads), lines so far, next time.
    probe_last: Option<(i32, bool, u32, i64, i32, i32, i64, bool, bool, i32)>,
    probe_n: u32,
    probe_next: f64,
    /// C6 round 7 probe P-A7: the last carry (ER's weapon frame in the body frame), inside an aim, lines, next time.
    pose_carry: Option<mw2er_core::aim_cam::PoseRel>,
    pose_aim: bool,
    pose_n: u32,
    pose_next: f64,
    /// C6 round 6: ER's aim camera / crossbow reticle masked during our own aim episodes (`tp_aim_cam = "keep"`).
    aimcam: crate::aim_cam::AimCam,
    /// C6 part 16.9: ER's head animation, how long it has been the same, ER busy now, the ignored-chord backoff.
    head_age: HeadAge,
    head_now: Option<i32>,
    er_busy_now: bool,
    ignored: IgnoredBackoff,
    /// C6 part 16 (X6 / P-X2): the carrier's grip write in flight (time, arm before, grip anim seen) and whether it failed
    /// (then the chord); the stance entered with the carrier on (its exit needs no 3 -> 1 -> 3 cycle).
    xgrip: Option<(f64, u32, bool)>,
    xgrip_failed: bool,
    xgrip_tries: u32,
    /// C6 part 16: two failed grips with the carrier = carried one-handed (`HOLD xbow grip fail`), no more chords.
    grip_gave_up: bool,
    carrier_stance: bool,
    /// C6 part 16: what the carrier reads from the stance (last tick): why a swap must wait, the active right weapon is
    /// a real ranged weapon.
    feed_busy: Option<&'static str>,
    feed_ranged: bool,
    /// C6 part 16 review: the carrier's last `grip_back` seq taken as an exit target.
    grip_back_seen: u32,
    /// C6 part 16 addendum: ER reverted the carrier's `arm_style` write once this session (P-X2 answered, 10:00:35.46):
    /// later carrier enters go straight to the chord.
    xgrip_reverted: bool,
    /// C6 part 16 addendum: `HOLD aim episode` (one line per aim episode: grip, ER's aim anim, camera, bolts).
    episodes: EpisodeLog,
    /// The aim input route of the open episode (`none` = ER's own bit: the user's RMB / LB with MW2 mode off).
    ep_route: &'static str,
    /// C6 part 17: the third-person guard ADS (ER's block input held with the polearm set), its input route, ER's weapon
    /// frame in the body frame at the last standing carry, the next weapon-frame sample, the yaw line once per episode.
    guard: GuardCtl,
    guard_route: Option<AimRoute>,
    guard_carry: Option<mw2er_core::aim_cam::PoseRel>,
    guard_next: f64,
    guard_yaw_logged: bool,
    /// C6 part 17 review: when the carry was sampled, when the last guard episode ended (no carry sample for
    /// GUARD_CARRY_QUIET_S: ER's guard lower), the episode started standing (else no weapon-frame detection: the anim id).
    guard_carry_t: f64,
    guard_end_t: f64,
    guard_frame_ok: bool,
    /// C6 part 17 addendum: the guard is held this tick standing still (the guard-frame learning's gate).
    guard_standing: bool,
    /// C6 part 17: the set of our last stance enter (a deferral never resumes across a set change).
    own_set: Option<StanceSet>,
}

impl Default for ErStance {
    fn default() -> Self {
        Self::new()
    }
}

fn yaw_deg(v: Option<V3>) -> Option<f32> {
    v.filter(|v| v.x * v.x + v.z * v.z > 1e-4).map(|v| v.x.atan2(v.z).to_degrees())
}

fn wrap_deg(d: f32) -> f32 {
    (d + 540.0).rem_euclid(360.0) - 180.0
}

impl ErStance {
    pub fn new() -> Self {
        ErStance {
            patched: Vec::new(),
            hyst: Hysteresis::default(),
            own_row: None,
            saved_arm: None,
            cycle: false,
            inflight: None,
            next_chord: 0.0,
            chord_fails: 0,
            refused_logged: None,
            disabled: None,
            skip_logged: None,
            shared_noted: Vec::new(),
            checked_rows: Vec::new(),
            refused_rows: Vec::new(),
            peer_logged: Vec::new(),
            mode_on_t: None,
            prev_pos: None,
            vy: 0.0,
            last_move_t: f64::NEG_INFINITY,
            stray: StrayGuard::default(),
            use_item_counts: (crate::pad::heal_pulses(), crate::shared::HEAL_KEY_PULSES.load(std::sync::atomic::Ordering::Relaxed)),
            last_use_item: None,
            melee_seen: crate::pad::melee_suppressed(),
            melee_log_t: f64::NEG_INFINITY,
            er_aim: None,
            arm_written: None,
            last_arm: 0,
            defer: ExitDefer::default(),
            chord_row: None,
            aim: AimCtl::default(),
            aim_route: None,
            aim_disabled_logged: false,
            probe_last: None,
            probe_n: 0,
            probe_next: 0.0,
            pose_carry: None,
            pose_aim: false,
            pose_n: 0,
            pose_next: 0.0,
            aimcam: crate::aim_cam::AimCam::default(),
            head_age: HeadAge::default(),
            head_now: None,
            er_busy_now: false,
            ignored: IgnoredBackoff::default(),
            xgrip: None,
            xgrip_failed: false,
            xgrip_tries: 0,
            grip_gave_up: false,
            carrier_stance: false,
            feed_busy: None,
            feed_ranged: false,
            grip_back_seen: 0,
            xgrip_reverted: false,
            episodes: EpisodeLog::default(),
            ep_route: "none",
            guard: GuardCtl::default(),
            guard_route: None,
            guard_carry: None,
            guard_next: 0.0,
            guard_yaw_logged: false,
            guard_carry_t: f64::NEG_INFINITY,
            guard_end_t: f64::NEG_INFINITY,
            guard_frame_ok: false,
            guard_standing: false,
            own_set: None,
        }
    }

    /// C6 part 16: what the carrier needs from the stance (its last tick): why a swap must wait now (ER busy, mid-air,
    /// mounted, a prompt, an aim episode, a chord in flight), the active right weapon is a real ranged weapon, the stance
    /// is disabled for the session.
    pub fn carrier_feed(&self) -> (Option<&'static str>, bool, bool) {
        (self.feed_busy, self.feed_ranged, self.disabled.is_some())
    }

    /// One tick on the game's main thread.
    pub fn tick(&mut self, i: &StanceIn) {
        let c = &i.cfg.coop;
        let flag = c.stance_on();
        if !flag && self.patched.is_empty() && !self.hyst.on && self.saved_arm.is_none() && self.inflight.is_none() {
            // inert with the default `er_stance = "off"`: no line, no read, no write
            crate::pad::set_stance_wants(false, false);
            crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);
            crate::shared::KB_AIM.store(false, std::sync::atomic::Ordering::Relaxed);
            // C6 round 6: the camera filter disarmed first (and its span line flushed)
            self.aimcam.stop();
            self.aimcam.tick(i.now, false, 0, mw2er_core::aim_cam::ArmIn::default());
            self.aim.stop();
            self.aim_route = None;
            // C6 part 17: no guard without the stance (the input is released above)
            self.guard_stop(i.now, "flag");
            return;
        }
        let now = i.now;
        // mid-air: vertical speed from the position delta
        if let Some(p) = i.pos {
            if let Some((q, t)) = self.prev_pos
                && now > t
                && (p - q).len() < 5.0
            {
                self.vy = (p.y - q.y) / (now - t) as f32;
                let (dx, dz) = (p.x - q.x, p.z - q.z);
                if (dx * dx + dz * dz).sqrt() / (now - t) as f32 > BUSY_MOVE_MPS {
                    self.last_move_t = now;
                }
            }
            self.prev_pos = Some((p, now));
        }
        self.mode_on_t = if i.mode_on { Some(self.mode_on_t.unwrap_or(now)) } else { None };
        self.note_use_item(now);
        // C6 part 16.9: ER's head animation (the chord's er_busy gate; the carrier's swap gate through the feed)
        // SAFETY: our PlayerIns from the game's own pointer this tick (or 0); non-faulting read.
        let head = if i.player != 0 { unsafe { crate::game::chars::read_anim_id(i.player) } } else { None };
        let head_age = self.head_age.step(now, head);
        self.head_now = head;
        self.er_busy_now = er_busy(head, head_age);
        let cv = i.carrier;
        // SAFETY: our PlayerIns from the game's own pointer this tick (or 0); non-faulting reads.
        let arm = unsafe { read_arm(i.player) };
        let own_row = arm.and_then(|a| row_of(a.right_id));
        // C6 part 16: the carrier's verified real crossbow is the active row: the stance holds it as it is (no patch)
        let carrier_row = cv.row.filter(|r| cv.on && own_row == Some(*r));
        // the rows we never patch (a real ranged weapon, a shield) / cannot (no param repository)
        let own_skip0 = match own_row {
            None => Some("empty"),
            Some(_) if carrier_row.is_some() => None,
            Some(r) if self.refused_rows.contains(&r) => Some("refused"),
            Some(r) if arm.is_some_and(|a| own_shared_left(r, &a.ids)) => Some("shared_left"),
            // C6 aim 4: while the aim fields are written our row reads as a crossbow: its stored type decides
            Some(r) => match row_addr(r) {
                None => Some("exe"),
                Some(addr) => self.kept(r, addr).and_then(|(wt, cat)| skip_reason(wt, cat)).or_else(|| self.check_layout(r, addr).err()),
            },
        };
        // C6 round 6: no stance enter while ER still spawns the player (a respawn into first person flashed the stance in
        // and out, 21:25:27.6-30.2); an active stance is not dropped by it
        let own_skip = own_skip0.or((i.settling && !self.hyst.on).then_some("spawn"));
        // C6 part 16: the carrier is about to swap / restore: the sword is not patched meanwhile
        let own_skip = own_skip.or((cv.holds && carrier_row.is_none()).then_some("carrier"));
        let wi = WantIn {
            flag,
            mode_on: i.mode_on,
            in_world: i.in_world,
            fp_on: i.fp_on,
            ac130: i.ac130,
            nuke: i.nuke,
            latched: i.latched,
            disabled: self.disabled.is_some(),
        };
        // C6 part 17: a hot er_stance change (crossbow <-> polearm): our row written with the other set exits first (the
        // plan restores it, the exit chords run), the re-enter writes the new set after the hysteresis; a peer's row is
        // restored here and re-patched with the new set by this tick's plan
        let set = c.stance_set();
        let own_stale = self.patched.iter().any(|p| p.peer == 0 && p.set != set);
        let peer_stale: Vec<u32> = self.patched.iter().filter(|p| p.peer != 0 && p.set != set).map(|p| p.row).collect();
        for r in peer_stale {
            self.restore_row(r, 0, "set");
        }
        if own_stale {
            log_i!("HOLD", "stance set change new={} (our row exits with its old set restored; the re-enter writes the new set)", set.label());
        }
        let want = stance_wanted(&wi) && own_skip.is_none() && arm.is_some() && !own_stale;
        // C6 pose 3: the stance had settled (its enter chords done) before this tick's transition
        let settled_before = self.posed() && !self.cycle;
        if flag && i.mode_on && i.in_world && !i.fp_on {
            self.skip(true, own_skip.unwrap_or(""));
        }
        // enter waits the hysteresis (and, with peers, stance_peer_delay_s after mode on); every exit is immediate
        let peers_present = !i.peers.is_empty();
        let delay_ok = !peers_present || self.mode_on_t.is_some_and(|t| now - t >= c.stance_peer_delay_s as f64);
        let was = self.hyst.on;
        if !want {
            self.hyst.force_off(now);
        } else if delay_ok {
            self.hyst.step(now, true, c.stance_min_switch_s as f64);
        }
        let on = self.hyst.on;
        // the patch set: ours while active, the peers' with their marker
        let peers: Vec<PeerStance> = if flag && c.stance_peers && self.disabled.is_none() && i.in_world {
            i.peers
                .iter()
                .map(|&(handle, addr, marker)| {
                    // SAFETY: the peer's PlayerIns from the player set this tick; non-faulting reads.
                    let row = unsafe { read_arm(addr) }.and_then(|a| row_of(a.right_id)).filter(|r| !self.refused_rows.contains(r));
                    PeerStance { handle, row: row.filter(|r| row_addr(*r).and_then(|a| self.kept(*r, a)).is_some_and(|(wt, cat)| skip_reason(wt, cat).is_none())), marker }
                })
                .collect()
        } else {
            Vec::new()
        };
        // C6 part 16: the carrier's crossbow row is never patched (a real crossbow already has the motion set)
        let own = own_row.filter(|_| carrier_row.is_none()).map(|row| OwnStance { row, wanted: on });
        let prev: Vec<u32> = self.patched.iter().map(|p| p.row).collect();
        let ours = arm.map_or_else(Vec::new, |a| equipped_rows(&a.ids));
        let plan = stance_patch_set(own, &ours, &peers, flag && c.stance_peers, &prev);
        for (h, row, why) in &plan.skip {
            if !self.peer_logged.contains(&(*h, *row, why)) {
                self.peer_logged.push((*h, *row, why));
                log_i!("HOLD", "peer_stance peer={} row={row} patched=0 reason={why}", logfmt::handle(*h));
            }
        }
        // exit: restore first, then the chords back; enter: patch first, then the chords
        let a_now = arm.map_or(0, |a| a.arm_style);
        self.last_arm = a_now;
        for row in &plan.restore {
            self.restore_row(*row, a_now, "exit");
        }
        for row in &plan.patch {
            let peer = if own.is_some_and(|o| o.wanted && o.row == *row) { 0 } else { peers.iter().find(|p| p.row == Some(*row)).map_or(0, |p| p.handle) };
            // C6 aim 4 fallback `tp_ads_patch = "stance"`: the aim fields follow in aim_tick once the enter chords settled
            self.patch_row(*row, peer, arm, set);
        }
        // our own row did not get patched (a soft failure: refused for the session): the stance is not active
        if self.disabled.is_some() || (self.hyst.on && carrier_row.is_none() && own_row.is_some_and(|r| !self.patched.iter().any(|p| p.row == r))) {
            self.hyst.force_off(now);
        }
        let on = self.hyst.on;
        // own transitions -> the grip target
        if on && !was {
            let a = a_now;
            // C6 part 16: with the carrier the exit target is the grip before the swap (EquipItem left it one-handed)
            self.carrier_stance = carrier_row.is_some();
            let back = if self.carrier_stance { cv.orig_arm.unwrap_or(a) } else { a };
            self.saved_arm = Some(self.saved_arm.unwrap_or(back));
            self.own_row = own_row;
            self.chord_fails = 0;
            self.ignored.clear();
            self.xgrip = None;
            // C6 part 16 addendum: ER reverted the arm_style write before (P-X2): the chord at once
            self.xgrip_failed = self.xgrip_reverted;
            self.xgrip_tries = 0;
            self.grip_gave_up = false;
            self.next_chord = self.next_chord.max(now);
            if self.carrier_stance {
                log_i!(
                    "HOLD",
                    "stance carrier enter row={} arm={a} orig_arm={back} grip_via={} (the real crossbow: nothing patched; grip to two-handed)",
                    own_row.map_or(-1, |r| r as i64),
                    if a == ARM_RIGHT_BOTH { "none" } else if self.xgrip_reverted { "chord" } else { "arm_style" }
                );
            }
            // C6 pose 3: back inside the exit's deferral with the grip and the weapon row unchanged: ER never left the
            // crossbow bank (review C6 pose: another row = switched in first person, its bank resolved unpatched: cycle)
            // C6 part 17: a set changed inside the deferral: ER holds the old set's bank, the re-enter must re-resolve
            if self.own_set.is_some_and(|s| s != set) {
                self.defer.clear();
            }
            self.own_set = Some(set);
            if !self.carrier_stance && self.inflight.is_none() && self.defer.resume(now, a, own_row) {
                self.cycle = false;
                log_i!(
                    "HOLD",
                    "stance resume row={} arm={a} (no chords: the exit's chords had not started, ER still holds the crossbow bank)",
                    own_row.map_or(-1, |r| r as i64)
                );
            } else {
                // C6 part 16.9 fix 3: a fresh equip resolves the real crossbow's bank: one 1 -> 3, no cycle
                self.cycle = a == ARM_RIGHT_BOTH && !self.carrier_stance;
            }
            self.chord_row = own_row;
        } else if !on && was {
            let exit_row = self.own_row.take();
            // C6 aim 4: the exit's chords (and their log lines) belong to the restored row
            self.chord_row = exit_row;
            // back at 3 with a target of 3: the bank must re-resolve (3 -> 1 -> 3); C6 part 16: after the carrier the
            // original is equipped anew (its own bank): no cycle, the chords wait for the restore (`carrier`)
            self.cycle = !self.carrier_stance && self.saved_arm.is_some_and(|s| exit_target(s) == ARM_RIGHT_BOTH) && a_now == ARM_RIGHT_BOTH;
            if self.carrier_stance {
                log_i!("HOLD", "stance carrier exit row={} arm={a_now} target={} (the carrier restores the original; its grip follows)", exit_row.map_or(-1, |r| r as i64), self.saved_arm.map_or(-1, |s| exit_target(s) as i64));
            }
            // C6 aim 4 (aim4.md 5 fix 2): the player's death or a world leave: the bytes are restored (above), the chords
            // dropped (ER ignores a grip chord on a dead character; the respawn enters anew). Review C6 aim 4: a nuke exit
            // (first person blocked at the PREP, the player alive) keeps them: refused `nuke`, dropped at the death
            if let Some(why) = exit_drops_chords(i.dead, i.in_world) {
                self.drop_exit_chords(exit_row, a_now, why);
            } else if exit_deferrable(&wi) && c.stance_exit_defer_s > 0.0 {
                self.defer.exit(now, c.stance_exit_defer_s as f64, a_now, exit_row, settled_before);
                log_i!(
                    "HOLD",
                    "stance exit_defer s={:.1} arm={a_now} row={} resumable={}",
                    c.stance_exit_defer_s,
                    exit_row.map_or(-1, |r| r as i64),
                    settled_before as u8
                );
            } else {
                self.defer.clear();
            }
        } else if on && own_row != self.own_row {
            // a weapon switch while active: the old row was restored, the new one patched; one refresh
            log_i!("HOLD", "stance switch row={}->{} arm={a_now}", self.own_row.map_or(0, |r| r as i64), own_row.map_or(0, |r| r as i64));
            self.own_row = own_row;
            self.chord_row = own_row;
            self.cycle = a_now == ARM_RIGHT_BOTH;
        }
        // C6 part 16 review: the carrier restored the original after the exit chords were dropped (death / world leave)
        // or with no stance at all (a journal restore after a crash): its grip before the swap is the exit target, once
        // per restore, taken only while the stance is off with no exit of its own pending (a live exit already has it)
        if let Some((seq, g)) = cv.grip_back
            && seq != self.grip_back_seen
            && !self.hyst.on
            && !i.dead
            && i.in_world
            && self.inflight.is_none()
        {
            self.grip_back_seen = seq;
            if self.saved_arm.is_none() && g != a_now {
                self.saved_arm = Some(g);
                self.cycle = false;
                self.carrier_stance = false;
                log_i!("HOLD", "stance grip_back arm={a_now} target={g} (the carrier restored the original; its grip before the swap, the exit chords follow)");
            }
        }
        self.refresh(i, arm);
        // C6 aim 4: third-person ADS -> ER's crossbow aim (the fields on our row, ER's aim input held)
        let aim_hold = self.aim_tick(i, own_row);
        // C6 part 17: third-person ADS -> ER's two-handed guard with the polearm set (only the block input; nothing written
        // on the row; the two poses are exclusive by config: er_aim needs the crossbow set, the guard the polearm set)
        let guard_hold = self.guard_tick(i, arm, own_row);
        let hold = aim_hold.or(guard_hold);
        // C6 round 6 (docs 14.8): only ER's aim BODY animation: ER's aim camera / crossbow reticle masked for our own
        // third-person aim episode (`tp_aim_cam = "keep"`, the aim pose's behaviour; "er" = the escape hatch)
        let prec_now = i.flags1c5.is_some_and(|f| f & PRECISION_SHOOTING != 0);
        self.aimcam.tick(
            now,
            c.aim_cam_keep() && self.disabled.is_none() && !self.aim.disabled(),
            i.player,
            mw2er_core::aim_cam::ArmIn {
                keep: true,
                episode: self.aim.active(),
                tp: !i.fp_on && !i.ac130 && !i.nuke,
                in_world: i.in_world,
                dead: i.dead,
                player: i.player != 0,
                prec: prec_now,
                since_end_s: 0.0,
            },
        );
        // C6 part 16 addendum: one line per aim episode (ours or ER's own bit), read-only
        if c.probe_er_aim || cv.on {
            self.episode_tick(i, arm, own_row, prec_now, c.aim_cam_keep());
        }
        // the wishes for the detours: no R3 melee while active; ER's aim input while the aim episode holds it
        crate::pad::set_stance_wants(on, hold == Some(AimRoute::Pad));
        crate::shared::KB_AIM.store(hold == Some(AimRoute::Kb), std::sync::atomic::Ordering::Relaxed);
        self.log_er_aim(hold.is_some(), i);
        self.log_melee(now);
        if on || cv.equipped {
            self.stray_guard(now, i.handle, a_now);
        }
        if c.probe_er_aim {
            self.probe(i, arm, own_row);
        }
        // C6 part 16: what the carrier reads next tick (a swap waits for an idle ER, the ground, no prompt / aim / chord,
        // and never replaces a patched sword under an active stance)
        self.feed_ranged = own_skip0 == Some("ranged");
        self.feed_busy = if self.er_busy_now {
            Some("er_busy")
        } else if self.vy.abs() > AIR_VY_MPS {
            Some("air")
        } else if i.mounted {
            Some("mount")
        } else if i.er_prompt {
            Some("prompt")
        } else if self.aim.active() || prec_now || self.guard.active() {
            Some("aim")
        } else if self.inflight.is_some() {
            Some("chord")
        } else if self.hyst.on && !self.carrier_stance {
            Some("stance")
        } else {
            None
        };
    }

    /// C6 aim 4: the stored (type, category) of our row while the aim fields are written on it, else the live bytes.
    fn kept(&self, row: u32, addr: usize) -> Option<(u16, u8)> {
        match self.patched.iter().find(|p| p.row == row).and_then(|p| p.aim_orig) {
            Some(o) => Some((o[1], o[0] as u8)),
            None => read_kept(addr),
        }
    }

    /// C6 aim 4 (aim4.md 5 fix 2): an exit's chords are dropped (death / world leave): the grip is the player's.
    fn drop_exit_chords(&mut self, row: Option<u32>, arm: u32, why: &'static str) {
        let pending = self.saved_arm.is_some_and(|s| exit_target(s) != arm) || self.cycle || self.inflight.is_some();
        self.saved_arm = None;
        self.cycle = false;
        self.carrier_stance = false;
        if self.inflight.take().is_some_and(|f| matches!(f.route, Route::Kb)) {
            crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        self.defer.clear();
        log_i!(
            "HOLD",
            "stance exit chords=0 row={} arm={arm} why={why} (dropped={}: ER ignores a grip chord on a dead / leaving player; the respawn enters anew)",
            row.map_or(-1, |r| r as i64),
            pending as u8
        );
    }

    /// C6 aim 4: one aim step. Returns the route whose aim input is held this tick (None = released).
    fn aim_tick(&mut self, i: &StanceIn, own_row: Option<u32>) -> Option<AimRoute> {
        let c = &i.cfg.coop;
        let now = i.now;
        let stance_mode = c.aim_patch_stance();
        let own_k = own_row.and_then(|r| self.patched.iter().position(|p| p.row == r));
        let route = if crate::pad::pad_fresh() {
            Some(AimRoute::Pad)
        } else if mw2er_core::input_logic::fresh(
            crate::shared::now_ms_if_started(),
            crate::shared::LAST_GDS_MOUSE_MS.load(std::sync::atomic::Ordering::Relaxed),
            KBD_FRESH_MS,
        ) {
            Some(AimRoute::Kb)
        } else {
            None
        };
        let ai = AimIn {
            // C6 round 7 (aim7.md 4 A): with tp_aim_cam = "keep" no episode before the filter (camera, reticle AND ER's
            // snipe body fade) is installed; refused / unavailable = the aim pose stays off (no invisible body)
            flag: c.ads_pose_on() && self.disabled.is_none() && (!c.aim_cam_keep() || crate::aim_cam::keep_ready()),
            // C6 part 16: the carrier's real crossbow is posed without a patched row (nothing is written for its aim)
            posed: self.posed() && !self.cycle && (own_k.is_some() || self.carrier_stance),
            ads: i.ads_aim,
            tp: !i.fp_on,
            scoped: i.scoped,
            ac130: i.ac130,
            nuke: i.nuke,
            dead: i.dead,
            menu: i.menu_open || i.latched || !i.in_world,
            prompt: i.er_prompt,
            mounted: i.mounted,
            air: self.vy.abs() > AIR_VY_MPS,
            input: route.is_some(),
        };
        let prec = i.flags1c5.is_some_and(|f| f & PRECISION_SHOOTING != 0);
        let row_txt = own_row.map_or(-1, |r| r as i64);
        // fallback `tp_ads_patch = "stance"` (review C6 aim 4): the fields go on only once the stance holds the weapon and
        // the enter's chords settled (never a chord with the crossbow type on the row: Y + RB would fire a bolt)
        let stance_fields = stance_mode && ai.flag && ai.posed && !self.aim.disabled();
        if stance_fields
            && let Some(k) = own_k
            && self.patched[k].aim_orig.is_none()
        {
            match self.aim_write(k) {
                Ok(o) => log_i!(
                    "HOLD",
                    "aim patch row={row_txt} orig={} new={} when=stance (after the enter chords; kept while posed)",
                    aim_text(&o),
                    aim_text(&aim_vals())
                ),
                Err(why) => {
                    log_w!("HOLD", "aim refused row={row_txt} reason={why} (tp_ads_pose off for this session; the stance keeps working)");
                    self.aim.disable();
                    self.aim_disabled_logged = true;
                }
            }
        }
        let st = self.aim.step(now, aim_wanted(&ai), prec);
        if st.write {
            self.aim_route = route;
            match own_k {
                Some(k) if self.patched[k].aim_orig.is_some() => {
                    log_i!("HOLD", "aim patch row={row_txt} already=1 route={} when=stance", route.map_or("none", |r| r.label()));
                }
                Some(k) => match self.aim_write(k) {
                    Ok(orig) => log_i!(
                        "HOLD",
                        "aim patch row={row_txt} orig={} new={} route={} when=ads (ER's crossbow aim: the type for this ADS)",
                        aim_text(&orig),
                        aim_text(&aim_vals()),
                        route.map_or("none", |r| r.label())
                    ),
                    Err(why) => {
                        log_w!("HOLD", "aim refused row={row_txt} reason={why} (tp_ads_pose off for this session; the stance keeps working)");
                        self.aim.disable();
                        self.aim_disabled_logged = true;
                    }
                },
                None if self.carrier_stance => {
                    log_i!(
                        "HOLD",
                        "aim hold row={row_txt} carrier=1 route={} when=ads (the real crossbow: ER's own aim, nothing written)",
                        route.map_or("none", |r| r.label())
                    );
                }
                None => {
                    self.aim.stop();
                }
            }
        }
        let mut restore_why = "release";
        match st.ev {
            Some(AimEv::On { ms }) => {
                log_i!("HOLD", "aim on t={ms}ms row={row_txt} route={} (precision_shooting set: ER holds the crossbow aim)", self.aim_route.map_or("none", |r| r.label()));
            }
            Some(AimEv::Fail { n }) => {
                log_w!(
                    "HOLD",
                    "aim fail reason=no_state n={n}/{} row={row_txt} (precision_shooting not seen within {:.1}s; input released)",
                    mw2er_core::er_stance::AIM_FAIL_LIMIT,
                    mw2er_core::er_stance::AIM_CONFIRM_S
                );
            }
            Some(AimEv::Restore { ms, timeout, disabled }) => {
                restore_why = if timeout { "timeout" } else { "release" };
                log_i!("HOLD", "aim off row={row_txt} ms={ms} why={restore_why}");
                if disabled {
                    log_w!("HOLD", "aim disabled reason=no_state (tp_ads_pose off for this session; the stance and the hand pose keep working)");
                    self.aim_disabled_logged = true;
                }
            }
            _ => {}
        }
        // the fields are written exactly while an episode runs (or while posed with `tp_ads_patch = "stance"`: a grip
        // change / a re-resolve restores them first so its chords run without the crossbow type)
        let want_fields = !self.aim.disabled() && (self.aim.active() || stance_fields);
        if !want_fields
            && let Some(k) = self.patched.iter().position(|p| p.aim_orig.is_some())
        {
            let why = if self.aim.disabled() {
                "disabled"
            } else if matches!(st.ev, Some(AimEv::Restore { .. })) {
                restore_why
            } else {
                "cfg"
            };
            if let Err(code) = self.aim_restore_at(k, why) {
                self.disable(code);
                return None;
            }
        }
        if self.aim.disabled() && !self.aim_disabled_logged {
            self.aim_disabled_logged = true;
            log_w!("HOLD", "aim disabled reason=write (tp_ads_pose off for this session)");
        }
        if !self.aim.active() {
            self.aim_route = None;
        }
        st.hold.then_some(self.aim_route).flatten()
    }

    /// C6 part 17 (docs 17.3): one guard step (`tp_ads_pose = "guard"` with `er_stance = "polearm"`). While the polearm
    /// stance holds the weapon two-handed in third person and MW2 ADS is held (non-scoped guns; the aim pose's gates), ER's
    /// block input is held (pad LB synth / keyboard RMB, the aim pose's routes), so ER raises the hidden weapon into its
    /// two-handed polearm guard. Nothing is written on the row (the Longsword is already a melee type), no aim camera /
    /// reticle / fade mask. The guard is detected by ER's weapon frame leaving the standing carry (read-only), else the
    /// fallback (the gun raise). Released at once when not wanted. Returns the route held this tick (None = released).
    fn guard_tick(&mut self, i: &StanceIn, arm: Option<ArmRead>, own_row: Option<u32>) -> Option<AimRoute> {
        let c = &i.cfg.coop;
        let now = i.now;
        let flag = c.guard_pose_on() && self.disabled.is_none();
        if !flag && !self.guard.active() {
            // inert (the default): no read, no line
            self.guard_carry = None;
            return None;
        }
        let own_k = own_row.and_then(|r| self.patched.iter().position(|p| p.row == r && p.peer == 0));
        let route = input_route();
        let ai = AimIn {
            flag,
            // the polearm set on our own row, two-handed, no chord in flight, the enter's re-resolve done
            posed: self.posed() && !self.cycle && own_k.is_some_and(|k| self.patched[k].set == StanceSet::Polearm),
            ads: i.ads_aim,
            tp: !i.fp_on,
            scoped: i.scoped,
            ac130: i.ac130,
            nuke: i.nuke,
            dead: i.dead,
            menu: i.menu_open || i.latched || !i.in_world,
            prompt: i.er_prompt,
            mounted: i.mounted,
            air: self.vy.abs() > AIR_VY_MPS,
            input: route.is_some(),
        };
        let wanted = aim_wanted(&ai);
        let move_input = (crate::shared::RAW_MOVE_KEYS.load(std::sync::atomic::Ordering::Relaxed) | crate::pad::move_bits()) != 0;
        let moved_recently = now - self.last_move_t < mw2er_core::er_stance::BUSY_MOVE_S;
        let standing = !move_input && !moved_recently;
        let was_active = self.guard.active();
        if wanted && !was_active {
            self.guard_route = route;
            self.guard_yaw_logged = false;
            // C6 part 17 review: the weapon-frame detection only for an episode started standing (the walk / run arm
            // swing passes the 0.10 m / 15 deg test against the standing carry): else the guard animation id only
            self.guard_frame_ok = standing;
        }
        let moved = self.guard_frame(i, wanted, standing);
        // C6 part 17 (docs 17.4): the body faces the camera while the guard is held standing (`tp_guard_yaw`)
        let yaw = wanted && self.guard_yaw(i, move_input, moved_recently);
        // C6 part 17 addendum: a guard held standing may teach the guard frame (coop_vis, `guard_axes = "auto"`)
        self.guard_standing = wanted && standing;
        let sample = GuardSample { moved, head: self.head_now, arm: arm.map_or(0, |a| a.arm_style), align: i.guard_align, yaw, corr: i.guard_corr };
        let route_txt = self.guard_route.map_or("none", |r| r.label());
        let out = self.guard.step(now, wanted, own_row, route_txt, &sample);
        let row_txt = own_row.map_or(-1, |r| r as i64);
        if out.started {
            log_i!(
                "HOLD",
                "guard on row={row_txt} route={route_txt} arm={} head={} carry={} carry_age={} frame={} (ER's block input held: the two-handed polearm guard; nothing written on the row)",
                sample.arm,
                self.head_now.map_or(-1, i64::from),
                self.guard_carry.is_some() as u8,
                if self.guard_carry.is_some() { format!("{:.2}", now - self.guard_carry_t) } else { "-".to_string() },
                self.guard_frame_ok as u8
            );
        }
        if let Some((ms, via)) = out.detected_now {
            log_i!(
                "HOLD",
                "guard detected ms={ms} via={via} head={} moved={} (ER raised the weapon: the gun aligns to the aim line on its frame, max {:.0} deg)",
                self.head_now.map_or(-1, i64::from),
                moved.map_or("?".to_string(), |(m, t, _)| format!("{m:.3}/{t:.1}")),
                c.tp_guard_align_max_deg
            );
        }
        if let Some(ms) = out.fallback_now {
            log_i!(
                "HOLD",
                "guard fallback ms={ms} carry={} ({})",
                self.guard_carry.is_some() as u8,
                if c.tp_ads_hold == "aim" {
                    "no guard pose detected: the gun raises along the camera (tp_ads_hold = aim) until it shows"
                } else {
                    "no guard pose detected; tp_ads_hold = hand: the gun stays on ER's weapon frame"
                }
            );
        }
        if let Some(e) = out.ended {
            self.log_guard_episode(&e, guard_end_why(&ai));
        }
        if !self.guard.active() {
            self.guard_route = None;
            if was_active {
                self.guard_end_t = now;
            }
        }
        out.hold.then_some(self.guard_route).flatten()
    }

    /// C6 part 17: ER's weapon frame (read-only, `aim_cam::weapon_rel` in the body-yaw frame) against the last standing
    /// carry: the carry is sampled every POSE_EVERY_S while no guard is held and `guard_carry_ok` (review: standing, posed,
    /// ER idle on the base-set idle, GUARD_CARRY_QUIET_S after the last episode); while held every GUARD_SAMPLE_S, only
    /// standing in an episode started standing -> (moved m, turned deg, off the carry). None = not sampled this tick / no
    /// carry / moving / unreadable.
    fn guard_frame(&mut self, i: &StanceIn, held: bool, standing: bool) -> Option<(f32, f32, bool)> {
        use mw2er_core::aim_cam::{pose_moved, weapon_rel};
        if i.player == 0 || i.now < self.guard_next {
            return None;
        }
        self.guard_next = i.now + if held { GUARD_SAMPLE_S } else { POSE_EVERY_S };
        let (Some(feet), Some(yaw)) = (i.pos, yaw_deg(i.body_fwd)) else { return None };
        // SAFETY: our PlayerIns from the game's own pointer this tick; non-faulting reads only.
        let root = unsafe { crate::game::chars::read_weapon_model(i.player, crate::game::chars::ASM_SLOT_RIGHT0) }.ok().and_then(|r| r.root)?;
        let rel = weapon_rel(&root, [feet.x, feet.y, feet.z], yaw);
        if !held {
            if guard_carry_ok(standing, self.posed(), self.head_now, self.er_busy_now, i.now - self.guard_end_t) {
                self.guard_carry = Some(rel);
                self.guard_carry_t = i.now;
            }
            return None;
        }
        if !(self.guard_frame_ok && standing) {
            return None;
        }
        let carry = self.guard_carry?;
        Some(pose_moved(&carry, &rel))
    }

    /// C6 part 17 (docs 17.4): `tp_guard_yaw`: our body faces the camera yaw while the guard is held standing (the M3B
    /// first-person yaw write: one 32-byte write of CSChrPhysicsModule's orientation + interpolated orientation, ER's own
    /// facing that it rewrites itself: nothing to restore). Only when the body is off the camera yaw by more than
    /// GUARD_YAW_MIN_DEG. True when the write went out.
    fn guard_yaw(&mut self, i: &StanceIn, move_input: bool, moved_recently: bool) -> bool {
        use mw2er_core::fp_math::{Axis, angle_diff_deg, horizontal_forward, quat_from_yaw, yaw_from_quat, yaw_of, yaw_pair};
        let c = &i.cfg.coop;
        if !c.guard_yaw_on() {
            return false;
        }
        let Some(p) = i.fp.filter(|p| p.player == i.player && i.player != 0) else { return false };
        let gi = GuardYawIn {
            flag: true,
            held: true,
            move_input,
            moved_recently,
            locked_on: p.locked_on,
            action_flags: p.action_flags,
            throw: mw2er_core::fp_logic::in_throw(p.throw_state),
            dead: i.dead,
        };
        if !guard_yaw_wanted(&gi) {
            return false;
        }
        let Some(h) = i.cam_fwd.and_then(horizontal_forward) else { return false };
        let axis = match i.cfg.first_person.yaw_forward_axis {
            mw2er_core::config::YawAxisCfg::PlusZ => Axis::PlusZ,
            _ => Axis::MinusZ,
        };
        let yaw = yaw_of(h, axis);
        let cur = yaw_from_quat(p.orientation, axis);
        let off = cur.map(|b| angle_diff_deg(yaw, b));
        if off.is_some_and(|d| d.abs() < mw2er_core::er_stance::GUARD_YAW_MIN_DEG) {
            return false;
        }
        // SAFETY: our PlayerIns's physics module (read this tick by read_player_fp, the same PlayerIns); two adjacent XYZW
        // quaternions the game rewrites itself (as first_person.rs's yaw follow); game::write checks the page.
        let ok = unsafe { crate::game::write(p.physics + crate::game::fp_read::ORIENTATION, yaw_pair(quat_from_yaw(yaw))) };
        if !self.guard_yaw_logged {
            self.guard_yaw_logged = true;
            log_i!(
                "HOLD",
                "guard yaw write ok={} off_deg={} axis={} (the body turns to the camera while the guard is held standing; ER's own facing, no restore)",
                ok as u8,
                off.map_or("?".to_string(), |d| format!("{d:.1}")),
                if axis == Axis::PlusZ { "+z" } else { "-z" }
            );
        }
        ok
    }

    /// C6 part 17: the finished guard episode (one line; read-only bookkeeping).
    fn log_guard_episode(&mut self, e: &GuardEpisode, why: &str) {
        log_i!("HOLD", "guard episode n={} {} why={why}", self.guard.count(), e.text());
    }

    /// C6 part 17: a hard stop of the guard (stance exit / restore_all / disable / flag off): the episode ends (the caller
    /// releases the input: KB_AIM / the pad wish are set from this tick's hold or cleared by the restore path).
    fn guard_stop(&mut self, now: f64, why: &str) {
        if self.guard.active() {
            // C6 part 17 review: no carry sample during ER's guard lower
            self.guard_end_t = now;
        }
        if let Some(e) = self.guard.stop(now) {
            self.log_guard_episode(&e, why);
        }
        self.guard_route = None;
    }

    /// C6 part 17: a guard episode holds ER's block input (the HUD keeps the MW2 crosshair, no grip chord).
    pub fn guard_active(&self) -> bool {
        self.guard.active()
    }

    /// C6 part 17: the guard pose was detected in the running episode (the gun aligns to the aim line on ER's frame).
    pub fn guard_aligned(&self) -> bool {
        self.guard.detected()
    }

    /// C6 part 17: the running guard episode is in its fallback (not detected within GUARD_CONFIRM_S: the gun raise).
    pub fn guard_fallback(&self) -> bool {
        self.guard.fallback()
    }

    /// C6 part 17 addendum: the guard episode is held standing still this tick (no move input, no movement in the last
    /// BUSY_MOVE_S): only such frames teach the guard frame (`guard_axes = "auto"`).
    pub fn guard_standing(&self) -> bool {
        self.guard.active() && self.guard_standing
    }

    /// C6 aim 4: writes the aim fields on the patched row `k` (originals stored first; a failed write is put back).
    fn aim_write(&mut self, k: usize) -> Result<AimVals, &'static str> {
        let addr = self.patched[k].addr;
        let orig = read_aim(addr).ok_or("read")?;
        if orig == aim_vals() {
            // already a crossbow's type and not by us (a real crossbow is never stanced): never written
            return Err("foreign");
        }
        if !write_aim(addr, &aim_vals()) || read_aim(addr) != Some(aim_vals()) {
            let _ = write_aim(addr, &orig);
            return Err("write");
        }
        self.patched[k].aim_orig = Some(orig);
        Ok(orig)
    }

    /// C6 aim 4: puts the aim fields of the patched row `k` back (re-read first: foreign bytes are left alone).
    fn aim_restore_at(&mut self, k: usize, why: &str) -> Result<(), &'static str> {
        let p = self.patched[k];
        let Some(orig) = p.aim_orig else { return Ok(()) };
        self.patched[k].aim_orig = None;
        let cur = read_aim(p.addr);
        let state = cur.map_or(RowState::Foreign, |c| classify_aim(&c, &orig));
        let restored = match state {
            RowState::Stance => write_aim(p.addr, &orig) && read_aim(p.addr) == Some(orig),
            RowState::Original => true,
            RowState::Foreign => false,
        };
        log_i!("HOLD", "aim restore row={} restored={} orig={} why={why}", p.row, restored as u8, aim_text(&orig));
        if state == RowState::Foreign {
            log_w!("HOLD", "aim foreign row={} cur={} orig={} (left alone)", p.row, cur.map_or("?".to_string(), |c| aim_text(&c)), aim_text(&orig));
            return Err("foreign");
        }
        if !restored {
            log_w!("HOLD", "aim restore failed row={} orig={}", p.row, aim_text(&orig));
            return Err("write");
        }
        Ok(())
    }

    /// C6 aim 4 probe (aim4.md 4 P-A, read-only): ER's animation, aim state, grip, the active row's type and the camera
    /// type, logged on every change (at most every 0.1 s, 300 lines per session); also with MW2 mode off.
    fn probe(&mut self, i: &StanceIn, arm: Option<ArmRead>, own_row: Option<u32>) {
        let prec = i.flags1c5.is_some_and(|f| f & PRECISION_SHOOTING != 0);
        // C6 round 7: ER's snipe body fade (1 = drawn; < 0.5 while our filter is kept = the body vanished)
        // SAFETY: our PlayerIns from the game's own pointer this tick (or 0); non-faulting read.
        let snipe = unsafe { crate::game::chars::read_snipe(i.player) };
        self.pose_probe(i, prec, snipe, own_row);
        if self.probe_n >= PROBE_MAX || i.now < self.probe_next || i.player == 0 {
            return;
        }
        // SAFETY: our PlayerIns from the game's own pointer this tick; non-faulting reads.
        let anim = unsafe { crate::game::chars::read_anim_id(i.player) }.unwrap_or(-1);
        let (wt, cat) = own_row.and_then(|r| row_addr(r).and_then(read_kept)).map_or((-1, -1), |(t, c)| (t as i32, c as i32));
        let cur = (
            anim,
            prec,
            arm.map_or(0, |a| a.arm_style),
            own_row.map_or(-1, |r| r as i64),
            wt,
            cat,
            i.camera_type.map_or(-1, |t| t as i64),
            i.mode_on,
            i.ads_held,
            // the fade in quarters (a fade shows as its own lines)
            snipe.map_or(-1, |v| (v * 4.0).round() as i32),
        );
        if self.probe_last == Some(cur) {
            return;
        }
        self.probe_last = Some(cur);
        self.probe_n += 1;
        self.probe_next = i.now + PROBE_EVERY_S;
        log_i!(
            "HOLD",
            "aim probe anim={} prec={} arm={} row={} type={} cat={} camtype={} mode={} ads={} aim={} snipe={} n={}/{PROBE_MAX}",
            cur.0,
            cur.1 as u8,
            cur.2,
            cur.3,
            cur.4,
            cur.5,
            cur.6,
            cur.7 as u8,
            cur.8 as u8,
            self.aim.on() as u8,
            snipe.map_or("?".to_string(), |v| format!("{v:.2}")),
            self.probe_n
        );
    }

    /// C6 round 7 probe P-A7 (work/c6/aim7.md 4 C, read-only, with `probe_er_aim`; also with MW2 mode off and a real
    /// crossbow): does ER's aim move the arms? The carry (ER's weapon frame in the body-yaw frame) is sampled every
    /// 0.25 s while ER's bit is clear; at the bit's rise it is logged once (`phase=carry`), then every 0.25 s while the
    /// bit lasts (`phase=aim`) with the move / turn against that carry (`arm=1` = an arm pose: > 0.10 m or > 15 deg),
    /// the left hand's distance to the weapon, all 10 queued animation ids, the bolt slots and the snipe fade.
    fn pose_probe(&mut self, i: &StanceIn, prec: bool, snipe: Option<f32>, own_row: Option<u32>) {
        use mw2er_core::aim_cam::{pose_moved, weapon_rel};
        if i.player == 0 || i.now < self.pose_next || self.pose_n >= POSE_MAX {
            if !prec {
                self.pose_aim = false;
            }
            return;
        }
        self.pose_next = i.now + POSE_EVERY_S;
        let (Some(feet), Some(yaw)) = (i.pos, yaw_deg(i.body_fwd)) else { return };
        // SAFETY: our PlayerIns from the game's own pointer this tick; non-faulting reads only.
        let w = unsafe { crate::game::chars::read_weapon_model(i.player, crate::game::chars::ASM_SLOT_RIGHT0) }.ok().and_then(|r| r.root);
        let Some(root) = w else { return };
        let rel = weapon_rel(&root, [feet.x, feet.y, feet.z], yaw);
        if !prec {
            self.pose_carry = Some(rel);
            self.pose_aim = false;
            return;
        }
        let v3 = |v: [f32; 3]| format!("[{:.3},{:.3},{:.3}]", v[0], v[1], v[2]);
        // SAFETY: as above.
        let lh = unsafe { crate::game::chars::read_hand(i.player, LEFT_HAND_BONE) }
            .and_then(|r| mw2er_core::hold::hand_world(&r.e, &r.m, feet).ok())
            .map(|(m, _)| mw2er_core::hold::translation(&m));
        let lh_d = lh.map_or("?".to_string(), |l| format!("{:.3}", (l - V3::new(root[12], root[13], root[14])).len()));
        // SAFETY: as above.
        let q = unsafe { crate::game::chars::read_anim_queue(i.player) }.map_or("?".to_string(), |q| q.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
        // SAFETY: as above.
        let bolt = unsafe { crate::game::chars::read_bolts(i.player) }.map_or("?".to_string(), |b| format!("{},{},{}", b[0], b[1], b[2]));
        let snipe = snipe.map_or("?".to_string(), |v| format!("{v:.2}"));
        if !self.pose_aim {
            self.pose_aim = true;
            if let Some(c) = self.pose_carry {
                self.pose_n += 1;
                log_i!("HOLD", "aim pose phase=carry rw_p={} rw_x={} rw_y={} n={}/{POSE_MAX}", v3(c.0), v3(c.1), v3(c.2), self.pose_n);
            }
        }
        let (m, t, armp) = self.pose_carry.map_or((f32::NAN, f32::NAN, false), |c| pose_moved(&c, &rel));
        self.pose_n += 1;
        log_i!(
            "HOLD",
            "aim pose phase=aim rw_p={} rw_x={} rw_y={} moved={m:.3} turned={t:.1} arm={} lh_d={lh_d} q=[{q}] bolt=[{bolt}] snipe={snipe} mode={} row={} carrier={} n={}/{POSE_MAX}",
            v3(rel.0),
            v3(rel.1),
            v3(rel.2),
            armp as u8,
            i.mode_on as u8,
            // C6 part 16.1: the row (probe 0 needs a real crossbow row 430200xx with mode=0) and the carrier
            own_row.map_or(-1, |r| r as i64),
            i.carrier.on as u8,
            self.pose_n
        );
    }

    /// C6 part 16 addendum (the 09:58 swap test: 12 carrier aim samples, ER's aim bit on, never its aim animation): one
    /// `HOLD aim episode` line per aim episode (ours or ER's own bit, also MW2 mode off) with the grip at its start / end,
    /// ER's first / last crossbow aim animation (46037xxx) and when it came, the aim bit's time, the camera types and
    /// `tp_aim_cam` in effect (the hot A/B "keep" vs "er"), the snipe fade, the active right slot's loaded state and the
    /// equipped bolt count. Read-only (non-faulting reads; the queue / bolts only while an episode runs).
    fn episode_tick(&mut self, i: &StanceIn, arm: Option<ArmRead>, own_row: Option<u32>, prec: bool, cam_keep: bool) {
        let ours = self.aim.active();
        if i.player == 0 || !i.in_world {
            self.episodes.clear();
            return;
        }
        if !(ours || prec || self.episodes.open()) {
            // the tracker still needs the tick time (an episode's first tick counts from its start)
            let _ = self.episodes.step(&EpisodeIn { now: i.now, ..Default::default() });
            return;
        }
        // SAFETY: our PlayerIns from the game's own pointer this tick; non-faulting reads only.
        let queue = unsafe { crate::game::chars::read_anim_queue(i.player) };
        let queue_aim = queue.and_then(|q| q.iter().copied().find(|a| aim_anim(*a)));
        // SAFETY: as above.
        let snipe = unsafe { crate::game::chars::read_snipe(i.player) };
        // the active right slot's bolt_loaded_states (ChrAsm slot 1 / 3 / 5)
        // SAFETY: as above.
        let loaded = arm.and_then(|a| unsafe { crate::game::equip::player_slot(i.player, 1 + 2 * a.right_slot as usize) }).map(|x| x.2);
        let ein = EpisodeIn {
            now: i.now,
            ours,
            prec,
            ads: i.ads_held,
            head: self.head_now,
            queue_aim,
            arm: arm.map_or(0, |a| a.arm_style),
            camtype: i.camera_type,
            snipe,
            loaded,
            bolts: crate::game::equip::main_bolt_qty(),
            cam_keep,
            carrier: i.carrier.on,
            mode_on: i.mode_on,
            row: own_row,
        };
        if ours && let Some(r) = self.aim_route {
            self.ep_route = r.label();
        }
        if let Some(e) = self.episodes.step(&ein) {
            log_i!(
                "HOLD",
                "aim episode n={} {} route={} (ER's crossbow aim anim 46037xxx: aim=; tp_aim_cam: cam=)",
                self.episodes.count(),
                e.text(),
                self.ep_route
            );
            self.ep_route = "none";
        }
    }

    /// C6 aim 4: ER's crossbow aim is confirmed (the gun aligns to the aim line, the HUD keeps a centre dot).
    pub fn aim_on(&self) -> bool {
        self.aim.on()
    }

    /// C6 aim 4: an aim episode runs (the fields are written; `tp_ads_hold = "aim"` stays out of it).
    pub fn aim_active(&self) -> bool {
        self.aim.active()
    }

    /// C6 attach 2: the crossbow stance holds the weapon now (active, two-handed, no chord in flight): `weapon_axes =
    /// "auto"` learns its map only then.
    pub fn posed(&self) -> bool {
        self.hyst.on && self.inflight.is_none() && self.last_arm == ARM_RIGHT_BOTH
    }

    /// C6 pose 3: our grip (`arm_style`) as read this tick (0 = unknown / the stance inert).
    pub fn arm(&self) -> u32 {
        self.last_arm
    }

    /// Restores every patched row at once (no chords): world leave, the panic path, the C3 strip path.
    pub fn restore_all(&mut self, why: &str) {
        // C6 round 6: the camera filter disarmed before anything is restored
        self.aimcam.stop();
        let rows: Vec<u32> = self.patched.iter().map(|p| p.row).collect();
        for r in rows {
            self.restore_row(r, 0, why);
        }
        self.hyst.force_off(0.0);
        self.own_row = None;
        self.chord_row = None;
        self.inflight = None;
        self.defer.clear();
        self.aim.stop();
        self.aim_route = None;
        // C6 part 17: the guard ends with the stance (its input is released just below)
        self.guard_stop(0.0, why);
        crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);
        crate::shared::KB_AIM.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::pad::set_stance_wants(false, false);
        if let Some((asm, v)) = self.arm_written.take() {
            // SAFETY: the ChrAsm address we wrote before (a heap field); game::write refuses anything else.
            let ok = unsafe { crate::game::write::<u32>(asm + ASM_ARM_STYLE, v) };
            log_i!("HOLD", "stance arm_style restored={v} ok={} why={why}", ok as u8);
        }
        // the grip is the player's again after a world leave / panic (no chords then)
        self.saved_arm = None;
        self.cycle = false;
        self.carrier_stance = false;
        self.xgrip = None;
    }

    fn skip(&mut self, relevant: bool, why: &'static str) {
        let cur = if relevant && !why.is_empty() { Some(why) } else { None };
        if cur != self.skip_logged {
            self.skip_logged = cur;
            if let Some(w) = cur {
                log_i!("HOLD", "stance skip reason={w}");
            }
        }
    }

    fn disable(&mut self, why: &'static str) {
        if self.disabled.is_some() {
            return;
        }
        self.disabled = Some(why);
        self.aimcam.stop();
        log_w!("HOLD", "stance disabled reason={why} (every patched row restored; hand attachment keeps running; for this session)");
        let rows: Vec<u32> = self.patched.iter().map(|p| p.row).collect();
        for r in rows {
            self.restore_row(r, 0, why);
        }
        self.hyst.force_off(0.0);
        self.defer.clear();
        self.aim.stop();
        self.aim_route = None;
        // C6 part 17: the guard ends with the stance
        self.guard_stop(0.0, why);
        crate::shared::KB_AIM.store(false, std::sync::atomic::Ordering::Relaxed);
        if why == "refresh_fail" {
            // the chord does not work here: never try to chord back
            self.saved_arm = None;
            self.cycle = false;
        }
    }

    /// The first time a row is seen: our raw offsets read the same values as the vendored accessors (pins the layout).
    fn check_layout(&mut self, row: u32, addr: usize) -> Result<(), &'static str> {
        if let Some(&(_, good)) = self.checked_rows.iter().find(|(r, _)| *r == row) {
            return if good { Ok(()) } else { Err("layout") };
        }
        let Some(r) = param_row(row) else { return Err("exe") };
        let acc: Vals = [r.wepmotion_category() as u16, r.guardmotion_category() as u16, r.sp_atkcategory(), r.wepmotion_one_hand_id() as u16, r.wepmotion_both_hand_id() as u16];
        let raw = read_vals(addr);
        let kept = read_kept(addr);
        let good = raw == Some(acc) && kept == Some((r.wep_type(), r.weapon_category()));
        if !good {
            log_w!("HOLD", "stance layout mismatch row={row} raw={:?} accessors={} kept={kept:?} (no patch)", raw, vals_text(&acc));
        }
        if self.checked_rows.len() < 256 {
            self.checked_rows.push((row, good));
        }
        if good { Ok(()) } else { Err("layout") }
    }

    fn patch_row(&mut self, row: u32, peer: u64, arm: Option<ArmRead>, set: StanceSet) {
        if self.patched.iter().any(|p| p.row == row) || self.disabled.is_some() {
            return;
        }
        // C6 part 17: the crossbow set (docs 3.2) or the polearm set (the vanilla Spear's, docs 17.2)
        let vals = set.vals();
        let Some(addr) = row_addr(row) else {
            self.refuse_row(row, peer, "row_missing", String::new());
            return;
        };
        if self.check_layout(row, addr).is_err() {
            // `layout` is already remembered per row (checked_rows) and logged once
            return;
        }
        let Some(orig) = read_vals(addr) else {
            self.refuse_row(row, peer, "read", String::new());
            return;
        };
        if orig == vals {
            // already the set's values and not by us (a real crossbow / spear row): the original is unknown, never written
            self.refuse_row(row, peer, "foreign", format!(" cur={} set={} (already the stance values; not ours)", vals_text(&orig), set.label()));
            return;
        }
        if !write_vals(addr, &vals) || read_vals(addr) != Some(vals) {
            let code = crate::game::refusal_label(crate::game::LAST_WRITE_REFUSAL.load(std::sync::atomic::Ordering::Relaxed));
            log_w!("HOLD", "stance write refused row={row} why={code}");
            // undo whatever went through
            let _ = write_vals(addr, &orig);
            self.disable("write");
            return;
        }
        self.patched.push(Patched { row, addr, orig, peer, aim_orig: None, set });
        if peer == 0 {
            let a = arm.map_or(-1, |a| a.arm_style as i64);
            let left = arm.and_then(|a| row_of(a.left_id)).map_or(-1, |r| r as i64);
            log_i!("HOLD", "stance enter row={row} orig={} arm={a} left={left} set={} new={}", vals_text(&orig), set.label(), vals_text(&vals));
        } else {
            log_i!("HOLD", "peer_stance peer={} row={row} patched=1 orig={} set={}", logfmt::handle(peer), vals_text(&orig), set.label());
        }
        if !self.shared_noted.contains(&row) && self.shared_noted.len() < 64 {
            self.shared_noted.push(row);
            log_i!("HOLD", "stance row={row} shared_note (every character using this row shares the motion set until the restore)");
        }
    }

    /// A soft patch failure: the row is refused for the session (logged once, skipped by every later plan; for our own
    /// row the stance stays off, see `tick`).
    fn refuse_row(&mut self, row: u32, peer: u64, why: &'static str, extra: String) {
        if self.refused_rows.contains(&row) {
            return;
        }
        if self.refused_rows.len() < 256 {
            self.refused_rows.push(row);
        }
        let who = if peer == 0 { "self".to_string() } else { logfmt::handle(peer) };
        log_w!("HOLD", "stance refused row={row} who={who} reason={why}{extra} (not patched; this row is skipped for the session)");
    }

    fn restore_row(&mut self, row: u32, arm: u32, why: &str) {
        let Some(k) = self.patched.iter().position(|p| p.row == row) else { return };
        // C6 aim 4: the aim fields first (an exit / switch / disable during an ADS episode: the input released at once)
        let aim_res = if self.patched[k].aim_orig.is_some() {
            // C6 round 6: the camera filter first (disarmed before the fields go back)
            self.aimcam.stop();
            self.aim.stop();
            self.aim_route = None;
            crate::shared::KB_AIM.store(false, std::sync::atomic::Ordering::Relaxed);
            self.aim_restore_at(k, why)
        } else {
            Ok(())
        };
        // C6 part 17: our own row's restore ends a guard episode (the hold wish follows from this tick's guard step)
        if self.patched[k].peer == 0 {
            self.guard_stop(0.0, why);
        }
        let p = self.patched.remove(k);
        let cur = read_vals(p.addr);
        let state = cur.map_or(RowState::Foreign, |c| classify_set(&c, &p.orig, &p.set.vals()));
        let restored = match state {
            RowState::Stance => write_vals(p.addr, &p.orig) && read_vals(p.addr) == Some(p.orig),
            RowState::Original => true,
            RowState::Foreign => false,
        };
        if p.peer == 0 {
            log_i!("HOLD", "stance exit row={row} restored={} arm={arm} why={why}", restored as u8);
        } else {
            log_i!("HOLD", "peer_stance peer={} row={row} patched=0 restored={} reason={why}", logfmt::handle(p.peer), restored as u8);
        }
        if state == RowState::Foreign {
            log_w!("HOLD", "stance foreign row={row} cur={} orig={} (left alone)", cur.map_or("?".to_string(), |c| vals_text(&c)), vals_text(&p.orig));
            self.disable("foreign");
        } else if !restored {
            log_w!("HOLD", "stance restore failed row={row} orig={}", vals_text(&p.orig));
            self.disable("write");
        }
        if let Err(code) = aim_res {
            self.disable(code);
        }
    }

    /// The grip refresh (docs 3.4): chords until `arm_style` is the target (3 while active, the saved grip after),
    /// one extra 3 -> 1 -> 3 cycle when the bank must re-resolve; refused at a menu / prompt / mid-air, retried.
    fn refresh(&mut self, i: &StanceIn, arm: Option<ArmRead>) {
        let c = &i.cfg.coop;
        let now = i.now;
        let Some(arm) = arm else { return };
        let a = arm.arm_style;
        let cv = i.carrier;
        // C6 part 16.9: ER's grip-switch animation in the queue while a chord / the carrier's grip write is checked
        let played_now = if self.inflight.is_some() || self.xgrip.is_some() {
            // SAFETY: our PlayerIns from the game's own pointer this tick; non-faulting reads.
            unsafe { crate::game::chars::read_anim_queue(i.player) }.map(|q| chord_played(&q))
        } else {
            None
        };
        if let Some(f) = self.inflight.as_mut()
            && let Some(pl) = played_now
        {
            f.queue_ok = true;
            f.played |= pl;
        }
        // C6 aim 4 (aim4.md 5): a chord that ran while the player was dead is void; an exit's chords die with the player
        if i.dead {
            if let Some(f) = self.inflight.as_mut() {
                f.dead_seen = true;
            }
            if !self.hyst.on && self.saved_arm.is_some() && self.inflight.is_none() {
                let row = self.chord_row;
                self.drop_exit_chords(row, a, "death");
                return;
            }
        }
        let target = if self.hyst.on { Some(ARM_RIGHT_BOTH) } else { self.saved_arm.map(exit_target) };
        let Some(target) = target else { return };
        // C6 part 16 (X6, probe P-X2): with the carrier, first the arm_style write (no button at all), the chord after it
        // failed; two failed grips = the crossbow carried one-handed (no more chords this stance)
        if self.carrier_stance && self.hyst.on {
            if self.grip_gave_up && self.inflight.is_none() {
                return;
            }
            if cv.on && c.stance_refresh != "none" && !self.xgrip_failed && self.inflight.is_none() && self.carrier_grip(i, arm, played_now) {
                return;
            }
        }
        match c.stance_refresh.as_str() {
            "none" => {
                if !self.hyst.on {
                    self.saved_arm = None;
                }
                return;
            }
            "arm_style" => {
                self.refresh_arm_style(i, arm, target);
                return;
            }
            _ => {}
        }
        // a chord in flight: wait for its end, then read arm_style CHORD_CHECK_S later
        if let Some(mut f) = self.inflight {
            if f.done_t.is_none() {
                match f.route {
                    Route::Pad(seq) => {
                        let (s, code, why) = crate::pad::chord_event();
                        if s == (seq & 0xffff) {
                            match code {
                                2 => f.done_t = Some(now),
                                // C6 part 16 addendum: ER already played the grip switch: the chord is done (verdict follows)
                                3 | 4 if abort_finishes(f.played) => {
                                    log_i!("HOLD", "chord abort_after_played route=pad why={} (ER had played the grip switch: judged as done)", if code == 3 { "aborted" } else { why });
                                    f.done_t = Some(now);
                                }
                                3 | 4 => {
                                    self.refused(if code == 3 { "aborted" } else { why });
                                    self.inflight = None;
                                    self.next_chord = now + CHORD_RETRY_S;
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if f.done_t.is_none() && now - f.t > CHORD_TIMEOUT_S {
                            self.refused("no_pad_read");
                            self.inflight = None;
                            self.next_chord = now + CHORD_RETRY_S;
                            return;
                        }
                    }
                    Route::Kb => {
                        if let Some(why) = refuse_now(i, self.vy) {
                            // a prompt / menu appeared: E and LMB released together
                            crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);
                            if abort_finishes(f.played) {
                                // C6 part 16 addendum (10:00:35.696: the user's menu opened 0.2 s into the chord, after ER had
                                // played 29060): the chord is done, its verdict follows CHORD_CHECK_S later
                                log_i!("HOLD", "chord abort_after_played route=kb why={why} (ER had played the grip switch: judged as done)");
                                f.done_t = Some(now);
                                self.inflight = Some(f);
                                return;
                            }
                            self.refused("aborted");
                            self.inflight = None;
                            self.next_chord = now + CHORD_RETRY_S;
                            return;
                        }
                        if now - f.t >= mw2er_core::pad_logic::CHORD_HOLD_MS as f64 / 1000.0 {
                            crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);
                            f.done_t = Some(now);
                        }
                    }
                }
                self.inflight = Some(f);
                return;
            }
            let done_t = f.done_t.unwrap_or(now);
            if now - done_t < CHORD_CHECK_S {
                return;
            }
            self.inflight = None;
            let took = chord_took(f.arm_before, a);
            let route = match f.route {
                Route::Pad(s) => format!("pad seq={s}"),
                Route::Kb => "kb".to_string(),
            };
            // C6 part 16.9 fix 2: no grip-switch animation in the window = ER ignored the input (busy): retried with
            // backoff, never counted (the 23:42:22 / 21:25:28 refresh fails)
            let played = f.queue_ok.then_some(f.played);
            let head = self.head_now.map_or(-1, i64::from);
            if chord_ignored(took, played) && !(f.dead_seen || i.dead) {
                let d = self.ignored.delay(now);
                log_i!(
                    "HOLD",
                    "chord sent route={route} arm={}->{a} dt={:.0}ms took=0 row={} kind={}",
                    f.arm_before,
                    (now - f.t) * 1000.0,
                    f.row.map_or(-1, |r| r as i64),
                    if f.exit { "exit" } else { "enter" }
                );
                log_i!(
                    "HOLD",
                    "chord verdict=ignored head={head} arm={}->{a} retry_s={d:.1} n={} (no grip-switch animation: ER ignored the input; not counted)",
                    f.arm_before,
                    self.ignored.count()
                );
                self.next_chord = now + d;
                return;
            }
            // C6 aim 4: a dead player's chord is void; failed exit chords give up; only enter chords can disable. C6 round 6:
            // an enter chord checked after its stance went off (first person / an exit meanwhile) is void too
            let (verdict, fails) = chord_verdict(took, f.dead_seen || i.dead, f.exit, !self.hyst.on, self.chord_fails);
            self.chord_fails = fails;
            let took_txt = if verdict == ChordVerdict::Void { "void" } else if took { "1" } else { "0" };
            let row_txt = f.row.map_or(-1, |r| r as i64);
            log_i!(
                "HOLD",
                "chord sent route={route} arm={}->{a} dt={:.0}ms took={took_txt} row={row_txt} kind={}",
                f.arm_before,
                (now - f.t) * 1000.0,
                if f.exit { "exit" } else { "enter" }
            );
            let label = if verdict == ChordVerdict::Void { "void" } else { chord_verdict_label(took, played) };
            log_i!("HOLD", "chord verdict={label} head={head} arm={}->{a} row={row_txt} played={}", f.arm_before, played.map_or("?", |p| if p { "1" } else { "0" }));
            // C6 part 16 addendum: the carrier's grip outcome by the chord (the 09:58 run only had the arm_style line)
            if self.carrier_stance && self.hyst.on && !f.exit && verdict != ChordVerdict::Void {
                log_i!(
                    "HOLD",
                    "xbow grip via=chord arm={}->{a} ok={} played={} (ER's own two-hand toggle)",
                    f.arm_before,
                    (a == ARM_RIGHT_BOTH) as u8,
                    played.map_or("?", |p| if p { "1" } else { "0" })
                );
            }
            if took {
                self.ignored.clear();
            }
            match verdict {
                ChordVerdict::Took => {
                    if self.cycle && a != ARM_RIGHT_BOTH {
                        self.cycle = false;
                    }
                    self.next_chord = now + mw2er_core::er_stance::CHORD_GAP_S;
                }
                ChordVerdict::Void | ChordVerdict::Retry => self.next_chord = now + CHORD_RETRY_S,
                ChordVerdict::ExitGaveUp => {
                    log_w!(
                        "HOLD",
                        "stance exit_chords gave_up row={row_txt} arm={a} (two exit chords did not change the grip; the grip is the player's, the stance stays enabled)"
                    );
                    self.saved_arm = None;
                    self.cycle = false;
                    self.defer.clear();
                }
                ChordVerdict::RefreshFail if self.carrier_stance && self.hyst.on => {
                    // C6 part 16 (16.3 step 6): the real crossbow stays, carried one-handed (the gun still on its model)
                    log_w!("HOLD", "xbow grip fail row={row_txt} arm={a} (two chords ER played did not change the grip: carried one-handed; no more chords)");
                    self.grip_gave_up = true;
                }
                ChordVerdict::RefreshFail => {
                    log_w!("HOLD", "stance refresh fail row={row_txt} arm={a} (two chords did not change the grip)");
                    self.disable("refresh_fail");
                }
            }
            return;
        }
        let need = a != target || (self.cycle && a == ARM_RIGHT_BOTH);
        if !need {
            if !self.hyst.on && !(self.carrier_stance && (cv.busy || cv.equipped)) {
                // exit complete: the grip is the player's again
                self.saved_arm = None;
                self.carrier_stance = false;
                if self.defer.ran_out() {
                    self.defer.clear();
                }
            }
            if let Some(why) = self.refused_logged.take() {
                // C6 part 16 addendum: a refusal ends when the grip is the target (the analyzer's chord row closes on it)
                log_i!("HOLD", "chord done arm={a} target={target} after={why} (the grip is the target; nothing more to send)");
            }
            return;
        }
        if now < self.next_chord {
            return;
        }
        // C6 pose 3: an exit's chords wait out the deferral (a quick F6 / F5 toggle back needs none)
        if !self.hyst.on && !self.defer.chords_allowed(now) {
            return;
        }
        // review C6 pose: a deferral that ran out mid-first-person never chords while the player fires, aims or moves
        let busy = !self.hyst.on && self.defer.ran_out() && i.fp_on && exit_chord_busy(i.fire_held, i.ads_held, now - self.last_move_t);
        // C6 aim 4: never a chord (Y + RB) while the aim fields are written / ER's aim input is held (review C6 aim 4:
        // any row with the fields on, the `tp_ads_patch = "stance"` fallback included)
        // C6 part 17: nor while a guard episode holds ER's block input (Y + RB would be a guard attack)
        let aiming = self.aim.active() || self.patched.iter().any(|p| p.aim_orig.is_some()) || self.guard.active();
        // C6 part 16.9: never while ER is busy (a jump, the get-up: ER ignores the chord); C6 part 16: never during a
        // carrier swap / restore (an exit waits for the original back), the RB only on an unloaded crossbow
        let carrier_wait = cv.busy || (!self.hyst.on && self.carrier_stance && cv.equipped);
        let loaded = cv.equipped && !mw2er_core::carrier::chord_rb_allowed(cv.loaded);
        if let Some(why) = refuse_now(i, self.vy)
            .or(aiming.then_some("aim"))
            .or(busy.then_some("busy"))
            .or(self.er_busy_now.then_some("er_busy"))
            .or(carrier_wait.then_some("carrier"))
            .or(loaded.then_some("loaded"))
        {
            self.refused(why);
            self.next_chord = now + CHORD_RETRY_S;
            return;
        }
        let route = if crate::pad::pad_fresh() {
            Route::Pad(crate::pad::chord_request())
        } else if mw2er_core::input_logic::fresh(
            crate::shared::now_ms_if_started(),
            crate::shared::LAST_GDS_KBD_MS.load(std::sync::atomic::Ordering::Relaxed),
            KBD_FRESH_MS,
        ) {
            crate::shared::KB_CHORD_T0.store(crate::shared::now_ms_if_started().max(1), std::sync::atomic::Ordering::Relaxed);
            Route::Kb
        } else {
            self.refused("no_input");
            self.next_chord = now + CHORD_RETRY_S;
            return;
        };
        self.refused_logged = None;
        self.inflight = Some(Inflight {
            route,
            t: now,
            arm_before: a,
            done_t: None,
            row: self.chord_row,
            exit: !self.hyst.on,
            dead_seen: false,
            played: false,
            queue_ok: false,
        });
    }

    /// C6 part 16 (X6, probe P-X2): the carrier's grip without a button: `arm_style` 3 on our PlayerIns ChrAsm through
    /// `game::write` (ER's change tracker watches the grip, so it should raise the same weapon-changed notify as ER's own
    /// toggle [I]); judged CHORD_CHECK_S later (`HOLD xbow grip via=arm_style ... ok=0|1 played=0|1`). A revert or a
    /// second attempt = the chord (the RB only while the crossbow is unloaded). Returns true when it handled this tick.
    fn carrier_grip(&mut self, i: &StanceIn, arm: ArmRead, played_now: Option<bool>) -> bool {
        let now = i.now;
        let a = arm.arm_style;
        if let Some((t0, before, played)) = self.xgrip {
            let played = played || played_now == Some(true);
            if now - t0 < CHORD_CHECK_S {
                self.xgrip = Some((t0, before, played));
                return true;
            }
            self.xgrip = None;
            let ok = mw2er_core::carrier::grip_write_ok(ARM_RIGHT_BOTH, a);
            log_i!(
                "HOLD",
                "xbow grip via=arm_style arm={before}->{a} ok={} played={} try={} (P-X2: {})",
                ok as u8,
                played as u8,
                self.xgrip_tries,
                if ok { "ER kept the two-handed grip, no button pressed" } else { "reverted: the chord follows" }
            );
            if !ok {
                self.xgrip_failed = true;
                // C6 part 16 addendum: answered for the session (ER resets its own grip): later enters chord at once
                self.xgrip_reverted = true;
                self.next_chord = now;
            }
            return true;
        }
        if a == ARM_RIGHT_BOTH {
            return false;
        }
        if self.xgrip_tries >= 2 {
            // the write held once and was lost again: the chord from here on
            self.xgrip_failed = true;
            return false;
        }
        if now < self.next_chord || refuse_now(i, self.vy).is_some() || self.er_busy_now || i.carrier.busy {
            return true;
        }
        // SAFETY: the live ChrAsm of our PlayerIns (heap); game::write checks the page and never faults.
        let ok = unsafe { crate::game::write::<u32>(arm.asm + ASM_ARM_STYLE, ARM_RIGHT_BOTH) };
        self.xgrip_tries += 1;
        if !ok {
            self.xgrip_failed = true;
            log_w!("HOLD", "xbow grip via=arm_style write refused why={} (the chord follows)", crate::game::refusal_label(crate::game::LAST_WRITE_REFUSAL.load(std::sync::atomic::Ordering::Relaxed)));
            return false;
        }
        self.xgrip = Some((now, a, false));
        self.next_chord = now + CHORD_CHECK_S;
        true
    }

    fn refused(&mut self, why: &'static str) {
        if self.refused_logged != Some(why) {
            self.refused_logged = Some(why);
            log_i!("HOLD", "chord refused reason={why} (retried every {CHORD_RETRY_S:.1}s)");
        }
    }

    /// `stance_refresh = "arm_style"` (probe): write arm_style to the target through `game::write` and log what ER does.
    fn refresh_arm_style(&mut self, i: &StanceIn, arm: ArmRead, target: u32) {
        if arm.arm_style == target {
            if !self.hyst.on {
                self.saved_arm = None;
                self.arm_written = None;
            }
            return;
        }
        if i.now < self.next_chord {
            return;
        }
        self.next_chord = i.now + CHORD_CHECK_S;
        // SAFETY: the live ChrAsm of our PlayerIns (heap); game::write checks the page and never faults.
        let ok = unsafe { crate::game::write::<u32>(arm.asm + ASM_ARM_STYLE, target) };
        if self.hyst.on && self.arm_written.is_none() {
            self.arm_written = Some((arm.asm, arm.arm_style));
        }
        log_i!("HOLD", "stance refresh arm_style {}->{target} ok={} (probe: does the motion set change?)", arm.arm_style, ok as u8);
        if !ok {
            self.disable("write");
        }
    }

    fn note_use_item(&mut self, now: f64) {
        let cur = (crate::pad::heal_pulses(), crate::shared::HEAL_KEY_PULSES.load(std::sync::atomic::Ordering::Relaxed));
        if cur != self.use_item_counts {
            self.use_item_counts = cur;
            self.last_use_item = Some(now);
        }
    }

    /// Docs 3.5: a non-mod bullet of ours while the stance is active is logged; two in 10 s disable the stance.
    fn stray_guard(&mut self, now: f64, owner: u64, arm: u32) {
        for (h, row) in crate::game::bullets::read_owned_foreign(owner) {
            match self.stray.note(now, h, row, self.last_use_item) {
                Stray::Known => {}
                Stray::UseItem => log_i!("HOLD", "stray_bullet row={row} ignored=use_item"),
                Stray::Counted(n) => log_w!("HOLD", "stray_bullet row={row} n={n} arm={arm}"),
                Stray::Disable(n) => {
                    log_w!("HOLD", "stray_bullet row={row} n={n} arm={arm}");
                    self.disable("stray_bullet");
                }
            }
        }
    }

    fn log_melee(&mut self, now: f64) {
        let n = crate::pad::melee_suppressed();
        let k = n.wrapping_sub(self.melee_seen);
        if k > 0 && now - self.melee_log_t >= MELEE_LOG_S {
            self.melee_seen = n;
            self.melee_log_t = now;
            log_i!("HOLD", "melee suppressed n={k}/30s (R3 does nothing in third person while er_stance is on)");
        }
    }

    fn log_er_aim(&mut self, on: bool, i: &StanceIn) {
        match (on, self.er_aim) {
            (true, None) => {
                let (cy, by) = (yaw_deg(i.cam_fwd).unwrap_or(0.0), yaw_deg(i.body_fwd).unwrap_or(0.0));
                self.er_aim = Some((cy, by));
                log_i!("HOLD", "er_aim on cam_yaw={cy:.1} body_yaw={by:.1}");
            }
            (false, Some((cy0, by0))) => {
                self.er_aim = None;
                let (cy, by) = (yaw_deg(i.cam_fwd).unwrap_or(cy0), yaw_deg(i.body_fwd).unwrap_or(by0));
                log_i!("HOLD", "er_aim off cam_yaw_delta={:.1} body_yaw_delta={:.1}", wrap_deg(cy - cy0), wrap_deg(by - by0));
            }
            _ => {}
        }
    }
}

/// The task-side chord refusals (the pad detour adds prompt / pulse / hold on its own).
/// Review C6 aim 4: `nuke` while the nuke's player sequence runs (an exit's chords wait; dropped at the death, sent if the
/// player survives and the block ends). C6 round 6: `spawn` while ER still spawns the player after a world enter.
fn refuse_now(i: &StanceIn, vy: f32) -> Option<&'static str> {
    chord_refusal(i.menu_open, vy.abs() > AIR_VY_MPS, i.latched, i.er_prompt, i.dead, i.settling)
        .or((!i.in_world).then_some("menu"))
        .or(i.nuke.then_some("nuke"))
}

/// C6 part 17: the route for ER's aim / block button this tick (as the aim pose: the pad while it is fresh, else the
/// keyboard / mouse while ER polled the mouse recently).
fn input_route() -> Option<AimRoute> {
    if crate::pad::pad_fresh() {
        Some(AimRoute::Pad)
    } else if mw2er_core::input_logic::fresh(
        crate::shared::now_ms_if_started(),
        crate::shared::LAST_GDS_MOUSE_MS.load(std::sync::atomic::Ordering::Relaxed),
        KBD_FRESH_MS,
    ) {
        Some(AimRoute::Kb)
    } else {
        None
    }
}

/// C6 part 17: why a guard episode ended (the first gate that failed; `release` = MW2 ADS let go).
fn guard_end_why(a: &AimIn) -> &'static str {
    if !a.flag {
        "flag"
    } else if !a.tp {
        "fp"
    } else if a.scoped {
        "scoped"
    } else if !a.posed {
        "stance"
    } else if !a.ads {
        "release"
    } else if a.ac130 || a.nuke {
        "killstreak"
    } else if a.dead {
        "dead"
    } else if a.menu {
        "menu"
    } else if a.prompt {
        "prompt"
    } else if a.mounted {
        "mounted"
    } else if a.air {
        "air"
    } else {
        "input"
    }
}

// ---- the param row (read-only lookup; bytes through game::read / game::write) --------------------------------------

const _: () = assert!(size_of::<eldenring::param::EQUIP_PARAM_WEAPON_ST>() == mw2er_core::er_stance::ROW_SIZE);

fn param_row(row: u32) -> Option<&'static eldenring::param::EQUIP_PARAM_WEAPON_ST> {
    use eldenring::cs::{EquipParamWeapon, SoloParamRepository};
    use fromsoftware_shared::FromStatic;
    // SAFETY: main thread after the boot gate (as `game::params::check`); the repository lives for the process.
    let repo = unsafe { SoloParamRepository::instance() }.ok()?;
    repo.get::<EquipParamWeapon>(row)
}

/// The row's address (read-only lookup: a pointer turned into an integer, never a `&mut`).
fn row_addr(row: u32) -> Option<usize> {
    param_row(row).map(|r| r as *const _ as usize)
}

fn read_vals(addr: usize) -> Option<Vals> {
    let mut v = [0u16; 5];
    for (k, f) in STANCE_FIELDS.iter().enumerate() {
        // SAFETY: non-faulting reads of plain integers inside a live param row.
        v[k] = unsafe {
            match f.width {
                1 => crate::game::read::<u8>(addr + f.off)? as u16,
                _ => crate::game::read::<u16>(addr + f.off)?,
            }
        };
    }
    Some(v)
}

/// C6 aim 4: the aim fields `[weaponCategory, wepType]` (`AIM_FIELDS` order).
fn read_aim(addr: usize) -> Option<AimVals> {
    let mut v = [0u16; 2];
    for (k, f) in AIM_FIELDS.iter().enumerate() {
        // SAFETY: non-faulting reads of plain integers inside a live param row.
        v[k] = unsafe {
            match f.width {
                1 => crate::game::read::<u8>(addr + f.off)? as u16,
                _ => crate::game::read::<u16>(addr + f.off)?,
            }
        };
    }
    Some(v)
}

fn write_aim(addr: usize, v: &AimVals) -> bool {
    let mut ok = true;
    for (k, f) in AIM_FIELDS.iter().enumerate() {
        // SAFETY: a field of a live EquipParamWeapon row (heap); game::write checks the page, never faults, and is
        // version-gated. Written only on our own stanced row for an ADS episode (aim4.md 3.2) and restored after it.
        ok &= unsafe {
            match f.width {
                1 => crate::game::write::<u8>(addr + f.off, v[k] as u8),
                _ => crate::game::write::<u16>(addr + f.off, v[k]),
            }
        };
    }
    ok
}

/// (wepType, weaponCategory): the fields the stance never writes (C6 aim 4: only the ADS episode does, see `kept`).
fn read_kept(addr: usize) -> Option<(u16, u8)> {
    // SAFETY: non-faulting reads of plain integers inside a live param row.
    unsafe { Some((crate::game::read::<u16>(addr + OFF_WEP_TYPE)?, crate::game::read::<u8>(addr + OFF_WEAPON_CATEGORY)?)) }
}

fn write_vals(addr: usize, v: &Vals) -> bool {
    let mut ok = true;
    for (k, f) in STANCE_FIELDS.iter().enumerate() {
        // SAFETY: a field of a live EquipParamWeapon row (heap); game::write checks the page, never faults, and is
        // version-gated. The game tolerates motion-field changes (they are re-read on a grip change [I1]).
        ok &= unsafe {
            match f.width {
                1 => crate::game::write::<u8>(addr + f.off, v[k] as u8),
                _ => crate::game::write::<u16>(addr + f.off, v[k]),
            }
        };
    }
    ok
}

#[cfg(test)]
mod tests {
    use mw2er_core::er_stance::{OFF_WEAPON_CATEGORY, OFF_WEP_TYPE, STANCE_FIELDS};

    /// C6: the raw offsets the stance writes are the vendored struct fields (set through the accessors, read as bytes).
    #[test]
    fn c6_stance_offsets_match_vendored_layout() {
        // SAFETY: EQUIP_PARAM_WEAPON_ST is plain data (integers, floats, bitfield bytes): all-zero is a valid value.
        let mut r: eldenring::param::EQUIP_PARAM_WEAPON_ST = unsafe { std::mem::zeroed() };
        r.set_wepmotion_category(0x11);
        r.set_guardmotion_category(0x22);
        r.set_sp_atkcategory(0x4433);
        r.set_wepmotion_one_hand_id(0x55);
        r.set_wepmotion_both_hand_id(0x66);
        r.set_weapon_category(0x77);
        r.set_wep_type(0x9988);
        // SAFETY: reading the bytes of our own local value.
        let b = unsafe { std::slice::from_raw_parts(&r as *const _ as *const u8, std::mem::size_of_val(&r)) };
        let got: Vec<u16> = STANCE_FIELDS.iter().map(|f| if f.width == 1 { b[f.off] as u16 } else { u16::from_le_bytes([b[f.off], b[f.off + 1]]) }).collect();
        assert_eq!(got, [0x11, 0x22, 0x4433, 0x55, 0x66]);
        assert_eq!(b[OFF_WEAPON_CATEGORY], 0x77);
        assert_eq!(u16::from_le_bytes([b[OFF_WEP_TYPE], b[OFF_WEP_TYPE + 1]]), 0x9988);
    }
}
