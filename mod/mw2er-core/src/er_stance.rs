//! C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance of MW2 mode's hidden ER weapon.
//!
//! MW2 mode equips nothing: marker SpEffect 79001090 (vfx 8060, `isInvisibleWeapon`) hides whatever the active right
//! slot holds (applied with a peer present, and solo too while `hold_mode = "hand"` or the stance is on). The stance is a runtime patch of five MOTION fields of that EquipParamWeapon row (the values all 12
//! vanilla crossbows share), toggled by `[coop] er_stance`, restored exactly. `weaponCategory` and `wepType` stay the
//! original weapon's (never 11 / 55), so ER has no reason to run its crossbow shoot / reload logic [I2]; R3 melee is
//! suppressed while the stance is active and a stray-bullet guard turns the stance off for the session.
//!
//! C6 aim 4 (work/c6/aim4.md, docs part 14): with `[coop] tp_ads_pose = "er_aim"` (default off) a third-person MW2 ADS
//! episode writes the two kept fields ([`AIM_FIELDS`]: category 11 / type 55) on OUR stanced row and holds ER's aim
//! input, so ER plays its shouldered crossbow aim; restored at the episode's end ([`AimCtl`]). The refresh also learned
//! the `dead` refusal and that a failed exit chord never disables the stance ([`chord_verdict`]).
//!
//! Pure: the DLL (`mw2er/src/er_stance.rs`) reads the row bytes / ChrAsm, writes through `game::write`, and asks this
//! module what to patch, restore, chord and log.

/// EquipParamWeapon row size (Paramdex, reg 11730034) [V].
pub const ROW_SIZE: usize = 664;
/// Kept fields (never written by the stance) [V offsets].
pub const OFF_BEHAVIOR_VARIATION: usize = 0x004;
pub const OFF_WEAPON_CATEGORY: usize = 0x0E6;
pub const OFF_ARROW_BOLT: usize = 0x1A0;
pub const OFF_WEP_TYPE: usize = 0x1A6;

/// One stance field: name, offset, width in bytes, the crossbow value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub off: usize,
    pub width: u8,
    pub value: u16,
}

/// The five motion fields and the values every vanilla crossbow (wepType 55) has [V, work/c6/stance.md 2].
pub const STANCE_FIELDS: [Field; 5] = [
    Field { name: "wepmotionCategory", off: 0x0E7, width: 1, value: 46 },
    Field { name: "guardmotionCategory", off: 0x0E8, width: 1, value: 2 },
    Field { name: "spAtkcategory", off: 0x0EE, width: 2, value: 0 },
    Field { name: "wepmotionOneHandId", off: 0x0F0, width: 1, value: 0 },
    Field { name: "wepmotionBothHandId", off: 0x0F1, width: 1, value: 16 },
];

/// The five values in [`STANCE_FIELDS`] order.
pub type Vals = [u16; 5];

pub fn stance_vals() -> Vals {
    STANCE_FIELDS.map(|f| f.value)
}

/// `[cat,guard,spatk,one,both]` for the log.
pub fn vals_text(v: &Vals) -> String {
    format!("[{},{},{},{},{}]", v[0], v[1], v[2], v[3], v[4])
}

/// What a row holds now against what we stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Original,
    Stance,
    /// Neither: something else changed the row; we never write it again.
    Foreign,
}

pub fn classify(cur: &Vals, orig: &Vals) -> RowState {
    classify_set(cur, orig, &stance_vals())
}

/// C6 part 17: [`classify`] against the values a row was written with (the crossbow or the polearm set).
pub fn classify_set(cur: &Vals, orig: &Vals, written: &Vals) -> RowState {
    if cur == orig {
        RowState::Original
    } else if cur == written {
        RowState::Stance
    } else {
        RowState::Foreign
    }
}

/// C6 part 17 (docs 17, work/c6/guard9.md 1.3): the vanilla Spear 16010000's five motion values in [`STANCE_FIELDS`]
/// order [V EquipParamWeapon, reg 11730034, `param_rows.py`; the same on 16020000 Crystal Spear / 16030000 Clayman's
/// Harpoon]: wepmotionCategory 36 (Spear), guardmotionCategory 2, spAtkcategory 0, wepmotionOneHandId 3,
/// wepmotionBothHandId 13 (the polearm two-handed base set: spears, great spears, halberds; the hands spread along a
/// shaft). weaponCategory / wepType stay the weapon's own (the Longsword is already a melee type: its guard is the
/// normal melee path, nothing else to write).
pub const POLEARM_VALS: Vals = [36, 2, 0, 3, 13];

/// C6 part 17: which motion set the stance writes (`[coop] er_stance`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StanceSet {
    /// The crossbow set (docs 3.2): the carry, the er_aim pose and the carrier.
    #[default]
    Crossbow,
    /// The polearm set ([`POLEARM_VALS`]): the carry and the third-person guard ADS.
    Polearm,
}

impl StanceSet {
    pub fn vals(self) -> Vals {
        match self {
            StanceSet::Crossbow => stance_vals(),
            StanceSet::Polearm => POLEARM_VALS,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StanceSet::Crossbow => "crossbow",
            StanceSet::Polearm => "polearm",
        }
    }
}

/// EquipParamWeapon rows are shared by every upgrade level / affinity: `(id / 100) * 100` [V vendored get_equip_param].
/// `None` for an empty slot (-1 / 0).
pub fn row_of(param_id: i32) -> Option<u32> {
    (param_id > 0).then_some(((param_id / 100) * 100) as u32)
}

/// Every row we have equipped (the six `ChrAsm.equipment_param_ids`, left 0/2/4, right 1/3/5), sorted, unique.
pub fn equipped_rows(ids: &[i32; 6]) -> Vec<u32> {
    let mut v: Vec<u32> = ids.iter().filter_map(|&id| row_of(id)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Our own stance is refused when one of our LEFT weapons (any of the three slots) uses the same row as the active right
/// weapon (dual-wielding the same weapon row, any upgrade level): the patched row would pose the left weapon too.
pub fn own_shared_left(right_row: u32, ids: &[i32; 6]) -> bool {
    [ids[0], ids[2], ids[4]].iter().any(|&id| row_of(id) == Some(right_row))
}

/// Rows the stance never patches: a real ranged weapon keeps its own logic, a shield is not held like a gun.
/// wepType 50 light bow, 51 bow, 53 greatbow, 55 crossbow, 56 ballista; 65 / 67 / 69 / 90 shields [V vanilla unpack].
pub fn skip_reason(wep_type: u16, weapon_category: u8) -> Option<&'static str> {
    if (50..=56).contains(&wep_type) || matches!(weapon_category, 10 | 11) {
        return Some("ranged");
    }
    if matches!(wep_type, 65 | 67 | 69 | 90) {
        return Some("shield");
    }
    None
}

/// Everything that decides whether OUR stance is wanted this tick (docs 3.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WantIn {
    /// `[coop] er_stance = "crossbow"`.
    pub flag: bool,
    pub mode_on: bool,
    pub in_world: bool,
    pub fp_on: bool,
    pub ac130: bool,
    /// The nuke sequence holds the player.
    pub nuke: bool,
    /// A loading screen / cutscene latch.
    pub latched: bool,
    /// Disabled for the session (stray bullet, foreign row, refresh failures, write refused).
    pub disabled: bool,
}

pub fn stance_wanted(i: &WantIn) -> bool {
    i.flag && i.mode_on && i.in_world && !i.fp_on && !i.ac130 && !i.nuke && !i.latched && !i.disabled
}

/// Our own active right row this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnStance {
    pub row: u32,
    pub wanted: bool,
}

/// A peer's active right row and whether it carries the MW2-mode marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerStance {
    pub handle: u64,
    pub row: Option<u32>,
    pub marker: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatchPlan {
    /// The full wanted set (sorted, unique).
    pub want: Vec<u32>,
    /// Rows to patch now (in `want`, not in the previous set).
    pub patch: Vec<u32>,
    /// Rows to restore now (in the previous set, not in `want`).
    pub restore: Vec<u32>,
    /// Peers not patched: (handle, row, reason).
    pub skip: Vec<(u64, u32, &'static str)>,
}

/// The patch set (docs 3.3): our row when our stance is wanted; each peer's row when it carries 79001090 (and
/// `peers_on`), except a peer row equal to ANY row we have equipped (`ours`: the six weapon slots, left included; see
/// [`equipped_rows`]) other than our own stanced active row (`shared_row`: none of our weapons may inherit a peer's
/// stance: not the left hand, not an inactive slot we may switch to). Everything previously patched and no longer wanted
/// is restored.
pub fn stance_patch_set(own: Option<OwnStance>, ours: &[u32], peers: &[PeerStance], peers_on: bool, prev: &[u32]) -> PatchPlan {
    let mut plan = PatchPlan::default();
    if let Some(o) = own
        && o.wanted
    {
        plan.want.push(o.row);
    }
    if peers_on {
        for p in peers {
            let Some(row) = p.row else { continue };
            if !p.marker {
                continue;
            }
            let own_stanced = own.is_some_and(|o| o.wanted && o.row == row);
            let shared = ours.contains(&row) || own.is_some_and(|o| o.row == row);
            if shared && !own_stanced {
                plan.skip.push((p.handle, row, "shared_row"));
                continue;
            }
            plan.want.push(row);
        }
    }
    plan.want.sort_unstable();
    plan.want.dedup();
    plan.patch = plan.want.iter().copied().filter(|r| !prev.contains(r)).collect();
    plan.restore = prev.iter().copied().filter(|r| !plan.want.contains(r)).collect();
    plan.restore.sort_unstable();
    plan.restore.dedup();
    plan
}

/// At most one enter / exit transition per `min_s` (F6 / FP toggles do not spam chords).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hysteresis {
    pub on: bool,
    last: Option<f64>,
}

impl Hysteresis {
    /// `Some(new state)` on a transition. The first transition is never delayed.
    pub fn step(&mut self, now: f64, want: bool, min_s: f64) -> Option<bool> {
        if want == self.on {
            return None;
        }
        if self.last.is_some_and(|t| now - t < min_s) {
            return None;
        }
        self.on = want;
        self.last = Some(now);
        Some(want)
    }

    /// An immediate exit (restore points that must not wait: mode off, FP on, AC-130, world leave, flag off).
    pub fn force_off(&mut self, now: f64) -> bool {
        let was = self.on;
        if was {
            self.on = false;
            self.last = Some(now);
        }
        was
    }
}

// ---- the grip refresh (docs 3.4) -----------------------------------------------------------------------------------

/// `ChrAsmEquipment.arm_style`.
pub const ARM_EMPTY: u32 = 0;
pub const ARM_ONE: u32 = 1;
pub const ARM_LEFT_BOTH: u32 = 2;
pub const ARM_RIGHT_BOTH: u32 = 3;
/// Two chords (3 -> 1 -> 3) are this far apart.
pub const CHORD_GAP_S: f64 = 0.4;
/// `arm_style` is read this long after a chord.
pub const CHORD_CHECK_S: f64 = 0.5;
/// A refused chord is retried this often.
pub const CHORD_RETRY_S: f64 = 0.5;

/// Chords to send at enter: one from one-handed (or empty / left both), two (3 -> 1 -> 3) when already right-two-handed
/// so the bank re-resolves.
pub fn enter_chords(arm: u32) -> u8 {
    if arm == ARM_RIGHT_BOTH { 2 } else { 1 }
}

/// The grip to return to at exit: the saved one (left-both cannot be reached with the right chord: one-handed).
pub fn exit_target(saved: u32) -> u32 {
    if saved == ARM_LEFT_BOTH { ARM_ONE } else { saved }
}

/// Chords to send at exit (after the fields are restored): to the target grip; back at 3 with a target of 3 needs the
/// bank re-resolved (3 -> 1 -> 3).
pub fn exit_chords(saved: u32, current: u32) -> u8 {
    let target = exit_target(saved);
    let cur_two = current == ARM_RIGHT_BOTH;
    let tgt_two = target == ARM_RIGHT_BOTH;
    match (cur_two, tgt_two) {
        (true, true) => 2,
        (true, false) | (false, true) => 1,
        (false, false) => 0,
    }
}

/// The refresh a chord should have produced: did `arm_style` toggle between one-handed and right-both?
pub fn chord_took(before: u32, after: u32) -> bool {
    (before == ARM_RIGHT_BOTH) != (after == ARM_RIGHT_BOTH)
}

/// Why a chord cannot be sent now (task side; the pad detour adds `prompt` / `pulse` / `hold`). C6 aim 4 (aim4.md 5):
/// `dead` (hp <= 0, death_flag, the death camera, or the nuke's player sequence): ER ignores a grip chord on a dead
/// character, so the 18:54 run's exit chords to the dead player read `took=0` twice and disabled the stance.
/// C6 round 6 (the 21:25:30 `refresh fail`): `spawn` while ER is still spawning the player after a world enter
/// ([`spawn_settling`]): the respawn's get-up ignored both enter chords (`3->3 took=0`).
pub fn chord_refusal(menu: bool, air: bool, latched: bool, prompt: bool, dead: bool, settling: bool) -> Option<&'static str> {
    if menu || latched {
        Some("menu")
    } else if dead {
        Some("dead")
    } else if settling {
        Some("spawn")
    } else if prompt {
        Some("prompt")
    } else if air {
        Some("air")
    } else {
        None
    }
}

/// Vertical speed above this (m/s) counts as mid-air for the chord.
pub const AIR_VY_MPS: f32 = 2.0;

/// C6 round 6: after a world enter (a load, a respawn) ER is still spawning the player for at least this long (the
/// 21:25 respawn: the screen fade 1.0 -> 0.9 at +2.1 s, the camera 194 m away, the get-up animations).
pub const SPAWN_SETTLE_S: f64 = 2.0;
/// ... and while the screen fade or the far camera last, at most this long after the world enter (a fade plate that
/// never clears cannot hold the chords / the stance for good).
pub const SPAWN_SETTLE_MAX_S: f64 = 12.0;
/// The largest screen-fade alpha (`fp_logic::fade_max`) at or above which the spawn is not finished.
pub const SPAWN_FADE: f32 = 0.05;

/// C6 round 6 (the 21:25:30 `stance refresh fail row=2000000 arm=3`): ER ignores a grip chord while it spawns the player
/// (fade up, MOVE blocked, the camera gate `dist`, the get-up animations), so both enter chords read `3->3 took=0`.
/// Settling = less than [`SPAWN_SETTLE_S`] since the world enter (NaN = no enter seen yet), or (within
/// [`SPAWN_SETTLE_MAX_S`]) the fade at or above [`SPAWN_FADE`] or ER's camera still far from the player (`cam_far`: the
/// camera gate's `dist`).
pub fn spawn_settling(since_enter_s: f64, fade: Option<f32>, cam_far: bool) -> bool {
    if !(since_enter_s >= SPAWN_SETTLE_S) {
        return true;
    }
    since_enter_s < SPAWN_SETTLE_MAX_S && (fade.is_some_and(|a| a >= SPAWN_FADE) || cam_far)
}

/// C6 aim 4 (aim4.md 5): what a checked chord means for the refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordVerdict {
    /// The grip changed.
    Took,
    /// The player was dead while the chord ran / was checked: ER ignores it; neither a success nor a failure.
    Void,
    /// The first failure: retried.
    Retry,
    /// The second failed EXIT chord: the exit gives up (the grip is the player's again); the stance stays enabled.
    ExitGaveUp,
    /// The second failed ENTER chord on a live player: `HOLD stance refresh fail`, the stance is disabled.
    RefreshFail,
}

/// The verdict of one checked chord and the failure count after it. Only enter chords on a live, unrefused player can
/// disable the stance (aim4.md 5 fix 3); a dead player's chord is void (fix 1). C6 round 6: an ENTER chord whose verdict
/// arrives after the stance it was sent for went off (`stance_left`: first person / an exit meanwhile; the 21:25:30
/// chord started 29.417, the stance exited 30.203, the verdict 30.236) is void too: it can no longer disable anything.
pub fn chord_verdict(took: bool, dead_seen: bool, exit: bool, stance_left: bool, fails: u32) -> (ChordVerdict, u32) {
    if took {
        return (ChordVerdict::Took, 0);
    }
    if dead_seen || (!exit && stance_left) {
        return (ChordVerdict::Void, fails);
    }
    if fails + 1 >= 2 {
        return (if exit { ChordVerdict::ExitGaveUp } else { ChordVerdict::RefreshFail }, 0);
    }
    (ChordVerdict::Retry, fails + 1)
}

// ---- C6 part 16.9 (work/c6/xbow8.md 4): the ignored chord ----------------------------------------------------------

/// ER's head animation is idle / locomotion (`id % 1_000_000` = 0 or the 20xxx / 22xxx families, as logged: 16000000,
/// 16020120, 16022100): ER accepts a grip chord. A jump (10202040), the respawn get-up (63000, 16004050), a roll /
/// backstep (..5100 / ..5200) or a grip switch in progress (..2904x-2907x) is busy.
pub fn er_idle_head(anim: i32) -> bool {
    let m = anim.rem_euclid(1_000_000);
    m == 0 || (20_000..21_000).contains(&m) || (22_000..23_000).contains(&m)
}

/// The head must have been idle / locomotion and unchanged for at least this long (a landing / get-up just ended).
pub const ER_IDLE_S: f64 = 0.3;

/// 16.9 fix 1: `HOLD stance refused reason=er_busy` (never counted): ER is not idle, or the head changed less than
/// [`ER_IDLE_S`] ago. An unreadable head (None) never blocks (the chord as before).
pub fn er_busy(head: Option<i32>, head_age_s: f64) -> bool {
    head.is_some_and(|h| !er_idle_head(h) || head_age_s < ER_IDLE_S)
}

/// How long the head animation has been the same (task side, one per tick).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeadAge {
    head: Option<i32>,
    since: f64,
}

impl HeadAge {
    /// The age of `head` at `now` (0 when it just changed).
    pub fn step(&mut self, now: f64, head: Option<i32>) -> f64 {
        if head != self.head {
            self.head = head;
            self.since = now;
        }
        now - self.since
    }
}

/// ER played its grip-switch animation (2904x-2907x: 29040 / 29050 to one hand, 29060 / 29070 to two) somewhere in
/// the queue: the chord reached ER.
pub fn chord_played(queue: &[i32]) -> bool {
    queue.iter().any(|a| (29_040..29_080).contains(&a.rem_euclid(1_000_000)))
}

/// 16.9 fix 2: the chord did not change the grip and ER played no grip-switch animation in its window: ER ignored the
/// input (`verdict=ignored`: retried with backoff, never counted). `played` None = the queue was unreadable (the old
/// verdict decides).
pub fn chord_ignored(took: bool, played: Option<bool>) -> bool {
    !took && played == Some(false)
}

/// `HOLD chord verdict=played|ignored|fail`.
pub fn chord_verdict_label(took: bool, played: Option<bool>) -> &'static str {
    if took {
        "played"
    } else if chord_ignored(took, played) {
        "ignored"
    } else {
        "fail"
    }
}

/// Ignored chords are retried after 0.5 / 1 / 2 / 4 s, at most [`IGNORED_CAP`] in [`IGNORED_WINDOW_S`] (then the next
/// waits until the oldest leaves the window): never a refresh failure.
pub const IGNORED_DELAYS_S: [f64; 4] = [0.5, 1.0, 2.0, 4.0];
pub const IGNORED_CAP: usize = 6;
pub const IGNORED_WINDOW_S: f64 = 20.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IgnoredBackoff {
    t: Vec<f64>,
}

impl IgnoredBackoff {
    /// An ignored chord at `now`: the delay before the next one.
    pub fn delay(&mut self, now: f64) -> f64 {
        self.t.retain(|t| now - t < IGNORED_WINDOW_S);
        let n = self.t.len();
        self.t.push(now);
        let base = IGNORED_DELAYS_S[n.min(IGNORED_DELAYS_S.len() - 1)];
        if n + 1 >= IGNORED_CAP {
            base.max(self.t[self.t.len() - IGNORED_CAP] + IGNORED_WINDOW_S - now)
        } else {
            base
        }
    }

    /// A chord that took (or a stance enter) starts the backoff over.
    pub fn clear(&mut self) {
        self.t.clear();
    }

    pub fn count(&self) -> usize {
        self.t.len()
    }
}

/// C6 part 16 addendum (2026-10-22 swap test): an aborted chord (a menu / prompt opened inside it, the pad detour
/// aborted it) whose grip-switch animation ER had already played is finished, not refused: the keys are released as
/// for any end and the verdict follows CHORD_CHECK_S later (10:00:35.6: 29060 played, arm 1 -> 3 at 35.71, yet
/// `chord refused reason=aborted` was the last word for 15 s).
pub fn abort_finishes(played: bool) -> bool {
    played
}

// ---- C6 part 16 addendum: one line per aim episode (`HOLD aim episode`) -------------------------------------------

/// ER's crossbow aim animation family: `id % 1_000_000` in 37000..38000 (probe 0, 09:55:37-40: 46037130 / 46037110 /
/// 46037120 / 46037100 with the weapon frame moving; never seen in a carrier episode of the 09:58 run).
pub fn aim_anim(id: i32) -> bool {
    id >= 0 && (37_000..38_000).contains(&id.rem_euclid(1_000_000))
}

/// An episode ends once neither our aim episode nor ER's aim bit was on for this long (ER's bit trails the input).
pub const EPISODE_GAP_S: f64 = 0.3;
/// At most this many `HOLD aim episode` lines per session.
pub const EPISODE_MAX: u32 = 200;

/// One tick's readings for the episode line (task side; every field read non-faulting, None = unreadable).
#[derive(Clone, Copy, Debug, Default)]
pub struct EpisodeIn {
    pub now: f64,
    /// Our third-person aim episode holds ER's aim input.
    pub ours: bool,
    /// ER's aim bit (`precision_shooting`).
    pub prec: bool,
    /// MW2 ADS held.
    pub ads: bool,
    pub head: Option<i32>,
    /// The first ER crossbow aim animation anywhere in ER's 10-entry queue this tick.
    pub queue_aim: Option<i32>,
    pub arm: u32,
    pub camtype: Option<u32>,
    pub snipe: Option<f32>,
    /// `bolt_loaded_states` of the active right slot.
    pub loaded: Option<bool>,
    /// The equipped bolt quantity (Bolt1 + Bolt2).
    pub bolts: Option<u32>,
    /// `tp_aim_cam` in effect this tick: true = "keep" (ER's camera / reticle / fade masked).
    pub cam_keep: bool,
    pub carrier: bool,
    pub mode_on: bool,
    pub row: Option<u32>,
}

/// One finished episode (`HOLD aim episode ...`).
#[derive(Clone, Debug, PartialEq)]
pub struct Episode {
    pub t0: f64,
    pub t1: f64,
    pub carrier: bool,
    pub mode_on: bool,
    pub row: Option<u32>,
    /// `tp_aim_cam` at the start and the end ("keep" / "er").
    pub cam0: bool,
    pub cam1: bool,
    pub arm0: u32,
    pub arm1: u32,
    pub head0: Option<i32>,
    pub head1: Option<i32>,
    /// The first ER aim animation and its delay after the episode start (ms), how many ticks showed one.
    pub aim_first: Option<(i32, u64)>,
    pub aim_last: Option<i32>,
    pub aim_ticks: u32,
    /// ER's aim bit: total ms on, first rise after the start (ms).
    pub prec_ms: u64,
    pub prec_at: Option<u64>,
    pub ours_ms: u64,
    pub ads_ms: u64,
    /// Camera types seen (bit k = type k, k < 16), the minimum snipe fade.
    pub camtypes: u16,
    pub snipe_min: Option<f32>,
    pub loaded0: Option<bool>,
    pub loaded1: Option<bool>,
    pub bolts0: Option<u32>,
    pub bolts1: Option<u32>,
}

impl Episode {
    fn start(i: &EpisodeIn) -> Self {
        Episode {
            t0: i.now,
            t1: i.now,
            carrier: i.carrier,
            mode_on: i.mode_on,
            row: i.row,
            cam0: i.cam_keep,
            cam1: i.cam_keep,
            arm0: i.arm,
            arm1: i.arm,
            head0: i.head,
            head1: i.head,
            aim_first: None,
            aim_last: None,
            aim_ticks: 0,
            prec_ms: 0,
            prec_at: None,
            ours_ms: 0,
            ads_ms: 0,
            camtypes: 0,
            snipe_min: None,
            loaded0: i.loaded,
            loaded1: i.loaded,
            bolts0: i.bolts,
            bolts1: i.bolts,
        }
    }

    /// The line's text after `aim episode n=<n> ` (the analyzer's `aim anim` row reads aim=, cam=, arm=, carrier=).
    pub fn text(&self) -> String {
        let ms = |s: f64| (s * 1000.0).round().max(0.0) as u64;
        let opt_i = |v: Option<i32>| v.map_or("?".to_string(), |x| x.to_string());
        let opt_b = |v: Option<bool>| v.map_or("?".to_string(), |x| (x as u8).to_string());
        let opt_u = |v: Option<u32>| v.map_or("?".to_string(), |x| x.to_string());
        let cam = |k: bool| if k { "keep" } else { "er" };
        let cams: Vec<String> = (0..16).filter(|k| self.camtypes & (1 << k) != 0).map(|k| k.to_string()).collect();
        format!(
            "ms={} carrier={} mode={} row={} cam={}{} arm={}->{} aim={} aim_at_ms={} aim_last={} aim_ticks={} head={}->{} prec_ms={} prec_at_ms={} ours_ms={} ads_ms={} camtypes=[{}] snipe_min={} loaded={}->{} bolts={}->{}",
            ms(self.t1 - self.t0),
            self.carrier as u8,
            self.mode_on as u8,
            self.row.map_or(-1, |r| r as i64),
            cam(self.cam0),
            if self.cam0 != self.cam1 { format!(">{}", cam(self.cam1)) } else { String::new() },
            self.arm0,
            self.arm1,
            self.aim_first.map_or("none".to_string(), |(a, _)| a.to_string()),
            self.aim_first.map_or("-".to_string(), |(_, t)| t.to_string()),
            opt_i(self.aim_last),
            self.aim_ticks,
            opt_i(self.head0),
            opt_i(self.head1),
            self.prec_ms,
            self.prec_at.map_or("-".to_string(), |t| t.to_string()),
            self.ours_ms,
            self.ads_ms,
            cams.join(","),
            self.snipe_min.map_or("?".to_string(), |v| format!("{v:.2}")),
            opt_b(self.loaded0),
            opt_b(self.loaded1),
            opt_u(self.bolts0),
            opt_u(self.bolts1),
        )
    }
}

/// The per-episode tracker: an episode runs while our aim episode or ER's aim bit is on (plus [`EPISODE_GAP_S`]); the
/// finished episode is returned once (read-only bookkeeping, no game access).
#[derive(Clone, Debug, Default)]
pub struct EpisodeLog {
    cur: Option<Episode>,
    last_on: f64,
    last_t: Option<f64>,
    n: u32,
}

impl EpisodeLog {
    pub fn step(&mut self, i: &EpisodeIn) -> Option<Episode> {
        let on = i.ours || i.prec;
        let dt = self.last_t.map_or(0.0, |t| (i.now - t).clamp(0.0, 0.25));
        self.last_t = Some(i.now);
        let mut ms = (dt * 1000.0).round() as u64;
        if self.cur.is_none() {
            if !on {
                return None;
            }
            // the first tick of an episode counts from its start
            ms = 0;
            self.cur = Some(Episode::start(i));
            self.last_on = i.now;
        }
        let e = self.cur.as_mut()?;
        if on {
            self.last_on = i.now;
        }
        e.t1 = i.now;
        e.cam1 = i.cam_keep;
        e.carrier |= i.carrier;
        e.mode_on |= i.mode_on;
        if e.row.is_none() {
            e.row = i.row;
        }
        e.arm1 = i.arm;
        if i.head.is_some() {
            e.head1 = i.head;
        }
        let since = ((i.now - e.t0) * 1000.0).round().max(0.0) as u64;
        let aim = i.queue_aim.or(i.head.filter(|h| aim_anim(*h)));
        if let Some(a) = aim {
            if e.aim_first.is_none() {
                e.aim_first = Some((a, since));
            }
            e.aim_last = Some(a);
            e.aim_ticks += 1;
        }
        if i.prec {
            e.prec_ms += ms;
            if e.prec_at.is_none() {
                e.prec_at = Some(since);
            }
        }
        if i.ours {
            e.ours_ms += ms;
        }
        if i.ads {
            e.ads_ms += ms;
        }
        if let Some(c) = i.camtype.filter(|c| *c < 16) {
            e.camtypes |= 1 << c;
        }
        if let Some(s) = i.snipe {
            e.snipe_min = Some(e.snipe_min.map_or(s, |m| m.min(s)));
        }
        if i.loaded.is_some() {
            e.loaded1 = i.loaded;
            if e.loaded0.is_none() {
                e.loaded0 = i.loaded;
            }
        }
        if i.bolts.is_some() {
            e.bolts1 = i.bolts;
            if e.bolts0.is_none() {
                e.bolts0 = i.bolts;
            }
        }
        if !on && i.now - self.last_on >= EPISODE_GAP_S {
            let mut done = self.cur.take()?;
            // the trailing gap is not part of the episode
            done.t1 = self.last_on;
            self.n += 1;
            return (self.n <= EPISODE_MAX).then_some(done);
        }
        None
    }

    /// An episode is open (the task reads the queue / bolts only then).
    pub fn open(&self) -> bool {
        self.cur.is_some()
    }

    /// Episodes finished so far (the line's `n=`).
    pub fn count(&self) -> u32 {
        self.n
    }

    /// A hard stop (world leave): the open episode is dropped unlogged.
    pub fn clear(&mut self) {
        self.cur = None;
        self.last_t = None;
    }
}

/// C6 aim 4 (aim4.md 5 fix 2): an exit caused by the player's death or a world leave restores the row bytes and drops
/// its chords (the respawn's enter starts from the grip ER gives it). The reason for the log. Review C6 aim 4: a nuke exit
/// keeps them (the nuke blocks first person at its player PREP, the player still alive and able to survive it:
/// `WarnProtected` / `WarnNotDead`): the chords wait (refusal `nuke`) and die with the player (the refresh's death drop).
pub fn exit_drops_chords(dead: bool, in_world: bool) -> Option<&'static str> {
    if !in_world {
        Some("world")
    } else if dead {
        Some("death")
    } else {
        None
    }
}

// ---- C6 aim 4 (work/c6/aim4.md 3, docs part 14): third-person ADS -> ER's crossbow aim pose -------------------------

/// The two KEPT fields ER's behaviour script asks before it takes the crossbow aim branch [I, aim4.md 1]: written on OUR
/// stanced row only while an ADS episode runs (`[coop] tp_ads_pose = "er_aim"`), restored at its end; the values every
/// vanilla crossbow has (weaponCategory 11, wepType 55). Never on a peer's row.
pub const AIM_FIELDS: [Field; 2] = [
    Field { name: "weaponCategory", off: OFF_WEAPON_CATEGORY, width: 1, value: 11 },
    Field { name: "wepType", off: OFF_WEP_TYPE, width: 2, value: 55 },
];

/// The two values in [`AIM_FIELDS`] order (`[category, type]`).
pub type AimVals = [u16; 2];

pub fn aim_vals() -> AimVals {
    AIM_FIELDS.map(|f| f.value)
}

pub fn aim_text(v: &AimVals) -> String {
    format!("[{},{}]", v[0], v[1])
}

/// The aim fields now against the stored originals (Stance = the crossbow values, ours).
pub fn classify_aim(cur: &AimVals, orig: &AimVals) -> RowState {
    if cur == orig {
        RowState::Original
    } else if *cur == aim_vals() {
        RowState::Stance
    } else {
        RowState::Foreign
    }
}

/// `ChrIns.chr_flags1c5` bit 1 `precision_shooting`: ER sets it every frame while its bow / crossbow aim runs [V vendored].
pub const PRECISION_SHOOTING: u8 = 0x02;
/// The aim state must show within this long after the aim input is held, else the episode fails (`reason=no_state`).
pub const AIM_CONFIRM_S: f64 = 0.5;
/// After the aim input is released the fields are restored once `precision_shooting` stayed clear for
/// [`AIM_CLEAR_S`], at the latest after this long.
pub const AIM_RELEASE_S: f64 = 0.4;
pub const AIM_CLEAR_S: f64 = 0.1;
/// Failed episodes (no aim state) that turn the aim pose off for the session, counted only until ER's aim state was
/// seen once (review C6 aim 4: after that a failure is ER being busy, a roll / flask / stagger / recovery, not a cached type).
pub const AIM_FAIL_LIMIT: u32 = 2;
/// Review C6 aim 4: after the aim state was seen once, a failed episode re-arms within the same ADS press this long after
/// the failure (the press that started in a roll gets the pose once the roll ends).
pub const AIM_RETRY_S: f64 = 1.0;

/// Everything that decides whether the aim pose is wanted this tick (aim4.md 3.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AimIn {
    /// `[coop] tp_ads_pose = "er_aim"` (or the old probe key `tp_ads_er_aim = true`).
    pub flag: bool,
    /// The stance holds the weapon: active, grip 3 (two-handed), no chord in flight, the enter's re-resolve cycle done.
    pub posed: bool,
    /// MW2 ADS held and allowed (not sprinting, the gun not lowered).
    pub ads: bool,
    /// Third person (first person off; a scope peek counts as `scoped`).
    pub tp: bool,
    /// The gun's scope takes third-person ADS (`tp_scope_peek` with an overlay gun): no aim pose.
    pub scoped: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub dead: bool,
    /// A menu, a loading / cutscene latch, out of the world or the window unfocused.
    pub menu: bool,
    pub prompt: bool,
    pub mounted: bool,
    pub air: bool,
    /// A fresh input route for the aim button (pad LB synth, else the keyboard's RMB).
    pub input: bool,
}

pub fn aim_wanted(i: &AimIn) -> bool {
    i.flag && i.posed && i.ads && i.tp && !i.scoped && !i.ac130 && !i.nuke && !i.dead && !i.menu && !i.prompt && !i.mounted && !i.air && i.input
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum AimPhase {
    #[default]
    Idle,
    /// The fields were written at `t`; the aim input is held from the next tick (`held`) until the state shows.
    Armed { t: f64, held: bool },
    /// ER's aim state was seen; the input stays held while wanted.
    On { t: f64 },
    /// The input was released at `t`; the fields go back once the state stayed clear (`clear` = since when).
    Release { t: f64, clear: Option<f64>, fail: bool },
}

/// One aim step's actions and its event.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimOut {
    /// Write the aim fields now (the episode starts).
    pub write: bool,
    /// Hold ER's aim input this tick.
    pub hold: bool,
    /// Restore the aim fields now (the episode ended).
    pub restore: bool,
    pub ev: Option<AimEv>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AimEv {
    /// The fields were written.
    Patch,
    /// ER's aim state (`precision_shooting`) appeared `ms` after the fields were written.
    On { ms: u32 },
    /// The state did not appear within [`AIM_CONFIRM_S`]: the input is released (`n` failures this session).
    Fail { n: u32 },
    /// The fields were restored `ms` after the release (`timeout` = the state never cleared); `disabled` = the
    /// failure limit was reached with this episode.
    Restore { ms: u32, timeout: bool, disabled: bool },
}

/// The per-episode aim machine (aim4.md 3.2): ADS edge -> write the fields; next tick -> hold the aim input; the state
/// within 0.5 s -> on, else release + `fail`; ADS up / any reason -> release the input first, restore the fields once
/// the state is clear (at most 0.4 s). A re-press inside a release (after a success) keeps the fields and holds again.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimCtl {
    phase: AimPhase,
    fails: u32,
    disabled: bool,
    /// A failed episode waits for the ADS release before the next try (one try per press) until `ever_on`.
    need_release: bool,
    /// ER's aim state was seen once this session: failures no longer count toward the limit.
    ever_on: bool,
    /// After a failure with `ever_on`: the next try within the same press not before this time.
    retry_at: f64,
}

impl AimCtl {
    pub fn step(&mut self, now: f64, wanted: bool, prec: bool) -> AimOut {
        let mut out = AimOut::default();
        if !wanted {
            self.need_release = false;
        }
        match self.phase {
            AimPhase::Idle => {
                if wanted && !self.disabled && !self.need_release && now >= self.retry_at {
                    self.phase = AimPhase::Armed { t: now, held: false };
                    out.write = true;
                    out.ev = Some(AimEv::Patch);
                }
            }
            AimPhase::Armed { t, held } => {
                if !wanted {
                    self.phase = AimPhase::Release { t: now, clear: None, fail: false };
                } else if held && prec {
                    self.phase = AimPhase::On { t: now };
                    self.ever_on = true;
                    out.hold = true;
                    out.ev = Some(AimEv::On { ms: ((now - t) * 1000.0).round().max(0.0) as u32 });
                } else if now - t > AIM_CONFIRM_S {
                    self.fails += 1;
                    if self.ever_on {
                        self.retry_at = now + AIM_RETRY_S;
                    } else {
                        self.need_release = true;
                    }
                    self.phase = AimPhase::Release { t: now, clear: None, fail: true };
                    out.ev = Some(AimEv::Fail { n: self.fails });
                } else {
                    self.phase = AimPhase::Armed { t, held: true };
                    out.hold = true;
                }
            }
            AimPhase::On { .. } => {
                if wanted {
                    out.hold = true;
                } else {
                    self.phase = AimPhase::Release { t: now, clear: None, fail: false };
                }
            }
            AimPhase::Release { t, clear, fail } => {
                if wanted && !fail && !self.disabled {
                    // a re-press while the fields are still written: hold again, the state must show again
                    self.phase = AimPhase::Armed { t: now, held: true };
                    out.hold = true;
                    return out;
                }
                let clear = if prec { None } else { Some(clear.unwrap_or(now)) };
                let cleared = clear.is_some_and(|c| now - c >= AIM_CLEAR_S);
                let timeout = now - t >= AIM_RELEASE_S;
                if cleared || timeout {
                    self.phase = AimPhase::Idle;
                    out.restore = true;
                    let disabled = fail && !self.ever_on && self.fails >= AIM_FAIL_LIMIT;
                    if disabled {
                        self.disabled = true;
                    }
                    out.ev = Some(AimEv::Restore { ms: ((now - t) * 1000.0).round().max(0.0) as u32, timeout: !cleared, disabled });
                } else {
                    self.phase = AimPhase::Release { t, clear, fail };
                }
            }
        }
        out
    }

    /// A hard stop (stance exit, weapon switch, panic): the input is released and the caller restores the fields at
    /// once. True when an episode was running (its fields were written).
    pub fn stop(&mut self) -> bool {
        let was = self.phase != AimPhase::Idle;
        self.phase = AimPhase::Idle;
        was
    }

    /// The aim pose is off for the session (a failed write, a foreign row, the failure limit).
    pub fn disable(&mut self) {
        self.phase = AimPhase::Idle;
        self.disabled = true;
    }

    /// An episode runs: the fields are written (until the restore); no grip chord may be sent meanwhile.
    pub fn active(&self) -> bool {
        self.phase != AimPhase::Idle
    }

    /// ER's aim state was confirmed and the input is held.
    pub fn on(&self) -> bool {
        matches!(self.phase, AimPhase::On { .. })
    }

    pub fn disabled(&self) -> bool {
        self.disabled
    }

    pub fn fails(&self) -> u32 {
        self.fails
    }

    /// ER's aim state was seen once this session (later failures are busy, never disable).
    pub fn ever_on(&self) -> bool {
        self.ever_on
    }
}

// ---- C6 part 17 (docs 17, work/c6/guard9.md): third-person ADS -> ER's two-handed polearm guard ---------------------

/// The guard pose must show (ER's weapon frame moved off the carry, or a guard animation) within this long after the
/// block input is held, else the episode raises the gun along the camera (`tp_ads_hold = "aim"`, the round-7 fallback)
/// until it shows.
pub const GUARD_CONFIRM_S: f64 = 0.6;
/// Consecutive weapon-frame samples off the carry (`aim_cam::pose_moved`: > 0.10 m or > 15 deg) that detect the guard.
pub const GUARD_MOVED_N: u32 = 2;
/// At most this many `HOLD guard episode` lines per session; at most this many distinct head animations per line.
pub const GUARD_EPISODE_MAX: u32 = 200;
pub const GUARD_HEADS_MAX: usize = 8;

/// ER's guard animation family [I: the id convention of guard9.md 1.2 (`a0{set}_0xxxxx`) with the guard moves at
/// `xx017xxx` (ER's TAE numbering of the guard start / loop / end)]. C6 part 17 addendum [L 2026-10-22 11:46-11:51]: the
/// polearm guard plays `xx019xxx` (13019000 was the head at every one of the 11 weapon-frame detections; 13019200 /
/// 13019500 inside guard episodes), so that block counts too: a guard started walking (no weapon-frame detection) is
/// detected by its anim id instead of the fallback (9 such episodes in that run). Except `xx0191xx`: 13019100 is the
/// head right after every release (ER's guard LOWER, `ads=0` in the aim probe), never a raised guard.
pub fn guard_anim(id: i32) -> bool {
    id >= 0 && matches!(id.rem_euclid(1_000_000), 17_000..18_000 | 19_000..19_100 | 19_200..20_000)
}

/// C6 part 17 review: no carry sample this long after a guard episode ended (ER's guard-lower animation still plays).
pub const GUARD_CARRY_QUIET_S: f64 = 0.5;

/// C6 part 17 review: ER's weapon frame may be stored as the guard's standing carry: the player stands still, the
/// stance is posed, ER is idle (`er_busy` false: idle head for [`ER_IDLE_S`]), the head is the base-set idle (`id %
/// 1_000_000 == 0`, never a guard animation; an unreadable head does not block, as `er_busy`) and the last guard episode
/// ended at least [`GUARD_CARRY_QUIET_S`] ago (`since_end_s`).
pub fn guard_carry_ok(standing: bool, posed: bool, head: Option<i32>, busy: bool, since_end_s: f64) -> bool {
    standing
        && posed
        && !busy
        && head.is_none_or(|h| h.rem_euclid(1_000_000) == 0 && !guard_anim(h))
        && since_end_s >= GUARD_CARRY_QUIET_S
}

/// One tick's readings for the guard (task side; non-faulting reads, None = unreadable / not sampled this tick).
#[derive(Clone, Copy, Debug, Default)]
pub struct GuardSample {
    /// (moved m, turned deg, off the carry) of ER's weapon frame against the last standing carry (`aim_cam::pose_moved`).
    pub moved: Option<(f32, f32, bool)>,
    pub head: Option<i32>,
    pub arm: u32,
    /// (deg, clamped) of the gun's aim align this frame (coop_vis, last tick).
    pub align: Option<(f32, bool)>,
    /// The body-yaw write went out this tick (`tp_guard_yaw`).
    pub yaw: bool,
    /// C6 part 17 addendum: the guard frame the gun used this frame ("builtin" | "auto" | "fixed"; None = none / off).
    pub corr: Option<&'static str>,
}

/// One guard episode (`HOLD guard episode n=.. <text>`).
#[derive(Clone, Debug, PartialEq)]
pub struct GuardEpisode {
    pub t0: f64,
    pub t1: f64,
    pub row: Option<u32>,
    pub route: &'static str,
    pub arm0: u32,
    pub arm1: u32,
    pub head0: Option<i32>,
    /// Distinct head animations in order of appearance (at most [`GUARD_HEADS_MAX`]).
    pub heads: Vec<i32>,
    pub guard_anim: Option<i32>,
    /// (ms after the start, via "frame" | "anim").
    pub detected: Option<(u64, &'static str)>,
    pub moved_max: f32,
    pub turned_max: f32,
    /// The first align angle seen while detected, the smallest, any clamp.
    pub align_first: Option<f32>,
    pub align_min: Option<f32>,
    pub clamped: bool,
    /// The gun-raise fallback started (ms after the start).
    pub fallback: Option<u64>,
    pub yaw_writes: u32,
    /// C6 part 17 addendum: the guard frame the gun used once detected (the first one seen; None = off / never).
    pub corr: Option<&'static str>,
}

impl GuardEpisode {
    /// The line's text after `guard episode n=<n> ` (the analyzer's `guard` row reads detected=, fallback=, align=).
    pub fn text(&self) -> String {
        let ms = |s: f64| (s * 1000.0).round().max(0.0) as u64;
        let heads: Vec<String> = self.heads.iter().map(|h| h.to_string()).collect();
        format!(
            "ms={} row={} route={} arm={}->{} head0={} heads=[{}] guard_anim={} detected={} via={} det_ms={} moved_max={:.3} turned_max={:.1} align={} align_min={} clamped={} fallback={} fb_ms={} yaw={} corr={}",
            ms(self.t1 - self.t0),
            self.row.map_or(-1, |r| r as i64),
            self.route,
            self.arm0,
            self.arm1,
            self.head0.map_or("?".to_string(), |h| h.to_string()),
            heads.join(","),
            self.guard_anim.map_or("none".to_string(), |a| a.to_string()),
            self.detected.is_some() as u8,
            self.detected.map_or("-", |d| d.1),
            self.detected.map_or("-".to_string(), |d| d.0.to_string()),
            self.moved_max,
            self.turned_max,
            self.align_first.map_or("none".to_string(), |d| format!("{d:.1}")),
            self.align_min.map_or("none".to_string(), |d| format!("{d:.1}")),
            self.clamped as u8,
            self.fallback.is_some() as u8,
            self.fallback.map_or("-".to_string(), |t| t.to_string()),
            self.yaw_writes,
            self.corr.unwrap_or("off")
        )
    }
}

/// One guard step's actions and events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuardOut {
    /// Hold ER's block input this tick (pad LB / keyboard RMB).
    pub hold: bool,
    /// The episode started this tick.
    pub started: bool,
    /// The guard pose was detected this tick: (ms, via).
    pub detected_now: Option<(u64, &'static str)>,
    /// The fallback started this tick (ms).
    pub fallback_now: Option<u64>,
    /// The episode ended this tick (None past [`GUARD_EPISODE_MAX`]: still ended, not logged).
    pub ended: Option<GuardEpisode>,
}

/// The guard ADS machine (docs 17.3): wanted (the aim pose's gates, [`aim_wanted`] with the guard flag) -> the block
/// input is held from this tick; the guard is detected by ER's weapon frame leaving the carry ([`GUARD_MOVED_N`]
/// samples) or a guard animation; not detected within [`GUARD_CONFIRM_S`] = the fallback (the gun raise) until it is;
/// not wanted = released at once (nothing was written: nothing to restore). No failure count, never disables: an
/// undetected guard only costs the fallback.
#[derive(Clone, Debug, Default)]
pub struct GuardCtl {
    ep: Option<GuardEpisode>,
    consec: u32,
    n: u32,
}

impl GuardCtl {
    pub fn step(&mut self, now: f64, wanted: bool, row: Option<u32>, route: &'static str, s: &GuardSample) -> GuardOut {
        let mut out = GuardOut::default();
        if !wanted {
            out.ended = self.stop(now);
            return out;
        }
        if self.ep.is_none() {
            self.consec = 0;
            out.started = true;
            self.ep = Some(GuardEpisode {
                t0: now,
                t1: now,
                row,
                route,
                arm0: s.arm,
                arm1: s.arm,
                head0: s.head,
                heads: Vec::new(),
                guard_anim: None,
                detected: None,
                moved_max: 0.0,
                turned_max: 0.0,
                align_first: None,
                align_min: None,
                clamped: false,
                fallback: None,
                yaw_writes: 0,
                corr: None,
            });
        }
        let Some(e) = self.ep.as_mut() else { return out };
        out.hold = true;
        e.t1 = now;
        e.arm1 = s.arm;
        let since = ((now - e.t0) * 1000.0).round().max(0.0) as u64;
        if let Some(h) = s.head {
            if !e.heads.contains(&h) && e.heads.len() < GUARD_HEADS_MAX {
                e.heads.push(h);
            }
            if guard_anim(h) && e.guard_anim.is_none() {
                e.guard_anim = Some(h);
            }
        }
        let mut via = None;
        if let Some((m, t, off)) = s.moved {
            e.moved_max = e.moved_max.max(m);
            e.turned_max = e.turned_max.max(t);
            self.consec = if off { self.consec + 1 } else { 0 };
            if self.consec >= GUARD_MOVED_N {
                via = Some("frame");
            }
        }
        if via.is_none() && s.head.is_some_and(guard_anim) {
            via = Some("anim");
        }
        if e.detected.is_none()
            && let Some(v) = via
        {
            e.detected = Some((since, v));
            out.detected_now = Some((since, v));
        }
        if e.detected.is_none() && now - e.t0 >= GUARD_CONFIRM_S && e.fallback.is_none() {
            e.fallback = Some(since);
            out.fallback_now = Some(since);
        }
        if e.detected.is_some()
            && let Some((d, clamped)) = s.align
        {
            e.align_first.get_or_insert(d);
            e.align_min = Some(e.align_min.map_or(d, |m| m.min(d)));
            e.clamped |= clamped;
        }
        if e.detected.is_some() && e.corr.is_none() {
            e.corr = s.corr;
        }
        if s.yaw {
            e.yaw_writes += 1;
        }
        out
    }

    /// A hard stop (stance exit, flag off, panic) or the release: the episode ends (the input is released by the caller).
    pub fn stop(&mut self, now: f64) -> Option<GuardEpisode> {
        let mut e = self.ep.take()?;
        // the release tick ends the hold (a stop without a tick in between keeps the last held tick)
        if now >= e.t1 && now - e.t1 <= 0.25 {
            e.t1 = now;
        }
        self.consec = 0;
        self.n += 1;
        (self.n <= GUARD_EPISODE_MAX).then_some(e)
    }

    /// An episode holds ER's block input (no grip chord meanwhile).
    pub fn active(&self) -> bool {
        self.ep.is_some()
    }

    /// The guard pose was detected in this episode (the gun aligns on ER's weapon frame).
    pub fn detected(&self) -> bool {
        self.ep.as_ref().is_some_and(|e| e.detected.is_some())
    }

    /// Not detected within [`GUARD_CONFIRM_S`]: the gun raise (`tp_ads_hold = "aim"`) until it is.
    pub fn fallback(&self) -> bool {
        self.ep.as_ref().is_some_and(|e| e.detected.is_none() && e.fallback.is_some())
    }

    /// Episodes ended so far (the line's `n=`).
    pub fn count(&self) -> u32 {
        self.n
    }
}

/// C6 part 17 (docs 17.4): the guard's body-yaw write (`tp_guard_yaw`) only while the guard is held, standing still (no
/// move input, no horizontal movement within [`BUSY_MOVE_S`]), not locked on, turning allowed (`action_flags` readable,
/// `AF_DISABLE_TURNING` clear), no throw, alive; and only when the body is off the camera yaw by more than
/// [`GUARD_YAW_MIN_DEG`] (no write per frame for nothing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuardYawIn {
    pub flag: bool,
    pub held: bool,
    pub move_input: bool,
    pub moved_recently: bool,
    pub locked_on: bool,
    /// `action_modifiers_flags` (None = unreadable: no write).
    pub action_flags: Option<u64>,
    pub throw: bool,
    pub dead: bool,
}

pub const GUARD_YAW_MIN_DEG: f32 = 1.0;

pub fn guard_yaw_wanted(i: &GuardYawIn) -> bool {
    i.flag
        && i.held
        && !i.move_input
        && !i.moved_recently
        && !i.locked_on
        && i.action_flags.is_some_and(|f| f & (1u64 << crate::fp_math::AF_DISABLE_TURNING) == 0)
        && !i.throw
        && !i.dead
}

// ---- C6 pose 3 (work/c6/pose3.md 1 / 4.2): the deferred exit chords ------------------------------------------------

/// A stance exit that may wait for its chords: first person on while everything else still allows the stance (the
/// 17:31 run: 8 enters / 8 exits in 3 min, ~4 chords = ~3.3 s of grip-change animation per F6 toggle). Review C6 pose:
/// MW2 mode off (F5) is NOT deferred any more: in third person the player would hold ER's own weapon in the crossbow
/// carry for the window. Every other exit (F5, AC-130, nuke, a latch, the flag, a world leave, a disable) chords at once.
pub fn exit_deferrable(i: &WantIn) -> bool {
    i.flag && i.in_world && i.mode_on && i.fp_on && !i.ac130 && !i.nuke && !i.latched && !i.disabled
}

/// Review C6 pose: a deferred exit's chords that ran out mid-first-person wait while the player is busy: MW2 fire or
/// ADS held, or moved within [`BUSY_MOVE_S`] (horizontal speed above [`BUSY_MOVE_MPS`]); the reason is `busy`. Only
/// for a deferred exit while first person is on (every other exit chords as before).
pub fn exit_chord_busy(fire: bool, ads: bool, since_move_s: f64) -> bool {
    fire || ads || since_move_s < BUSY_MOVE_S
}

pub const BUSY_MOVE_S: f64 = 0.5;
pub const BUSY_MOVE_MPS: f32 = 0.5;

/// The deferred exit: the row is restored at once (first person and mode off see the original row), the exit's grip
/// chords wait `defer_s`; a re-enter inside that window with the grip unchanged needs no chord at all, because no grip
/// change happened since the stance's own enter chords: ER still holds the crossbow bank it resolved then [I1].
/// Review C6 pose: the weapon row counts too: a weapon switched in first person had its motion bank resolved while its
/// row was unpatched, so a re-enter on another row needs its 3 -> 1 -> 3 re-resolve.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExitDefer {
    /// (until, grip at the exit, our row at the exit, the stance had settled: enter chords done) while an exit waits.
    pending: Option<(f64, u32, Option<u32>, bool)>,
    /// The window ran out and the exit's chords are now allowed (they wait while busy in first person); cleared by
    /// the next exit / resume / clear.
    ran_out: bool,
}

impl ExitDefer {
    /// An exit at `now`. `settled` = the stance was posed (active, no chord in flight, the enter's re-resolve cycle
    /// done); only then can a re-enter skip its chords. `defer_s <= 0` = no deferral.
    pub fn exit(&mut self, now: f64, defer_s: f64, arm: u32, row: Option<u32>, settled: bool) {
        self.pending = (defer_s > 0.0).then_some((now + defer_s, arm, row, settled));
        self.ran_out = false;
    }

    /// The exit's chords may go now (no deferral, or it ran out; then it is cleared).
    pub fn chords_allowed(&mut self, now: f64) -> bool {
        match self.pending {
            Some((until, _, _, _)) if now < until => false,
            Some(_) => {
                self.pending = None;
                self.ran_out = true;
                true
            }
            None => true,
        }
    }

    /// A re-enter at `now` with grip `arm` on row `row`: true = resume without chords (inside the window, the grip
    /// and the weapon row unchanged, the stance had settled). Always clears the deferral.
    pub fn resume(&mut self, now: f64, arm: u32, row: Option<u32>) -> bool {
        let r = self.pending.is_some_and(|(until, a, r0, settled)| now < until && a == arm && row.is_some() && r0 == row && settled);
        self.pending = None;
        self.ran_out = false;
        r
    }

    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// The exit's chords come from a deferral that ran out (the `busy` refusal applies to them).
    pub fn ran_out(&self) -> bool {
        self.ran_out
    }

    pub fn clear(&mut self) {
        self.pending = None;
        self.ran_out = false;
    }
}

// ---- the stray-bullet guard (docs 3.5) -----------------------------------------------------------------------------

/// The mod's whole row range (twins, M2, M6, AC-130, nuke): ours, never a stray.
pub fn mod_row(id: i32) -> bool {
    (79_000_000..=79_999_999).contains(&id)
}

/// Two strays within this window disable the stance for the session.
pub const STRAY_WINDOW_S: f64 = 10.0;
pub const STRAY_LIMIT: usize = 2;
/// A bullet within this long of an ER use-item pulse (a throwing pot, a knife from the quick slot) is not counted.
pub const STRAY_USE_ITEM_S: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stray {
    /// Already seen (same bullet handle) or a mod row.
    Known,
    /// A use-item bullet (not counted).
    UseItem,
    /// A counted stray: n in the window.
    Counted(usize),
    /// The limit was reached: restore and disable.
    Disable(usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrayGuard {
    seen: Vec<(u64, f64)>,
    hits: Vec<f64>,
}

impl StrayGuard {
    /// One of OUR bullets seen this tick while the stance is active.
    pub fn note(&mut self, now: f64, handle: u64, row: i32, last_use_item: Option<f64>) -> Stray {
        self.seen.retain(|(_, t)| now - *t <= 30.0);
        if mod_row(row) || self.seen.iter().any(|(h, _)| *h == handle) {
            return Stray::Known;
        }
        if self.seen.len() < 256 {
            self.seen.push((handle, now));
        }
        if last_use_item.is_some_and(|t| now - t <= STRAY_USE_ITEM_S) {
            return Stray::UseItem;
        }
        self.hits.retain(|t| now - *t <= STRAY_WINDOW_S);
        self.hits.push(now);
        if self.hits.len() >= STRAY_LIMIT { Stray::Disable(self.hits.len()) } else { Stray::Counted(self.hits.len()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c6_stance_fields_are_the_vanilla_crossbow_values() {
        // work/c6/stance.md 2: all 12 wepType-55 rows: motion 46, guard 2, spAtk 0, one 0, both 16
        assert_eq!(stance_vals(), [46, 2, 0, 0, 16]);
        let names: Vec<_> = STANCE_FIELDS.iter().map(|f| f.name).collect();
        assert_eq!(names, ["wepmotionCategory", "guardmotionCategory", "spAtkcategory", "wepmotionOneHandId", "wepmotionBothHandId"]);
        // never the "is a crossbow" fields, the bolt, the attack behaviour; inside the row; no overlap
        let kept = [(OFF_WEAPON_CATEGORY, 1), (OFF_WEP_TYPE, 2), (OFF_ARROW_BOLT, 4), (OFF_BEHAVIOR_VARIATION, 4)];
        let mut bytes = Vec::new();
        for f in STANCE_FIELDS {
            assert!(f.off + f.width as usize <= ROW_SIZE);
            assert!(f.width == 1 || f.width == 2);
            assert!(f.width == 2 || f.value <= 255);
            for b in f.off..f.off + f.width as usize {
                assert!(!bytes.contains(&b), "overlap at 0x{b:x}");
                bytes.push(b);
                for (k, w) in kept {
                    assert!(!(k..k + w).contains(&b), "stance touches kept field at 0x{k:x}");
                }
            }
        }
        assert_eq!(STANCE_FIELDS.map(|f| f.off), [0xE7, 0xE8, 0xEE, 0xF0, 0xF1]);
    }

    #[test]
    fn c6_stance_classify_and_rows() {
        let orig = [23, 1, 0, 3, 10];
        assert_eq!(classify(&orig, &orig), RowState::Original);
        assert_eq!(classify(&stance_vals(), &orig), RowState::Stance);
        assert_eq!(classify(&[23, 1, 0, 3, 11], &orig), RowState::Foreign);
        assert_eq!(row_of(2_000_325), Some(2_000_300));
        assert_eq!(row_of(110_000), Some(110_000));
        assert_eq!(row_of(-1), None);
        assert_eq!(row_of(0), None);
        assert_eq!(vals_text(&orig), "[23,1,0,3,10]");
    }

    #[test]
    fn c6_stance_ranged_and_shield_skip() {
        for wt in [50, 51, 53, 55, 56] {
            assert_eq!(skip_reason(wt, 10), Some("ranged"));
        }
        for wt in [65, 67, 69, 90] {
            assert_eq!(skip_reason(wt, 12), Some("shield"));
        }
        // unarmed, a longsword, a staff, a torch are patched
        for (wt, cat) in [(33, 9), (9, 3), (57, 8), (87, 12)] {
            assert_eq!(skip_reason(wt, cat), None, "{wt}/{cat}");
        }
    }

    #[test]
    fn c6_stance_wanted_conditions() {
        let on = WantIn { flag: true, mode_on: true, in_world: true, ..WantIn::default() };
        assert!(stance_wanted(&on));
        for off in [
            WantIn { flag: false, ..on },
            WantIn { mode_on: false, ..on },
            WantIn { in_world: false, ..on },
            WantIn { fp_on: true, ..on },
            WantIn { ac130: true, ..on },
            WantIn { nuke: true, ..on },
            WantIn { latched: true, ..on },
            WantIn { disabled: true, ..on },
        ] {
            assert!(!stance_wanted(&off), "{off:?}");
        }
    }

    #[test]
    fn c6_stance_patch_set() {
        let me = |row, wanted| Some(OwnStance { row, wanted });
        let peer = |handle, row, marker| PeerStance { handle, row: Some(row), marker };
        let set = |own: Option<OwnStance>, peers: &[PeerStance], on, prev: &[u32]| {
            let ours: Vec<u32> = own.map(|o| o.row).into_iter().collect();
            stance_patch_set(own, &ours, peers, on, prev)
        };
        // own only
        let p = set(me(2_000_000, true), &[], true, &[]);
        assert_eq!((p.want.clone(), p.patch.clone(), p.restore.clone()), (vec![2_000_000], vec![2_000_000], vec![]));
        // own + peers (a peer without the marker is ignored; the same peer row once)
        let peers = [peer(1, 3_000_000, true), peer(2, 4_000_000, false), peer(3, 3_000_000, true)];
        let p = set(me(2_000_000, true), &peers, true, &[2_000_000]);
        assert_eq!((p.want.clone(), p.patch.clone()), (vec![2_000_000, 3_000_000], vec![3_000_000]));
        // peers off: only ours
        assert_eq!(set(me(2_000_000, true), &peers, false, &[]).want, vec![2_000_000]);
        // a peer with our row while we are NOT in the stance: skipped (shared_row)
        let p = set(me(3_000_000, false), &peers, true, &[]);
        assert!(p.want.is_empty());
        assert_eq!(p.skip, vec![(1, 3_000_000, "shared_row"), (3, 3_000_000, "shared_row")]);
        // ... while we ARE in the stance the shared row is simply ours
        assert_eq!(set(me(3_000_000, true), &peers, true, &[]).want, vec![3_000_000]);
        // removal -> restore
        let p = set(me(2_000_000, false), &[], true, &[2_000_000, 3_000_000]);
        assert_eq!((p.patch.clone(), p.restore.clone()), (vec![], vec![2_000_000, 3_000_000]));
        // weapon switch: restore the old, patch the new
        let p = set(me(5_000_000, true), &[], true, &[2_000_000]);
        assert_eq!((p.patch, p.restore), (vec![5_000_000], vec![2_000_000]));
        // no own sample: peers only
        assert_eq!(set(None, &[peer(1, 7_000_000, true)], true, &[]).want, vec![7_000_000]);
    }

    #[test]
    fn c6_stance_patch_set_skips_every_equipped_row() {
        // review: a peer row equal to our LEFT weapon or an inactive right slot is never patched on our PC, stance or not
        let ids = [3_000_105, 2_000_000, -1, 6_000_200, 4_000_000, -1]; // left 3.0M / 4.0M, right 2.0M (active) / 6.0M
        let ours = equipped_rows(&ids);
        assert_eq!(ours, vec![2_000_000, 3_000_100, 4_000_000, 6_000_200]);
        let peers = [
            PeerStance { handle: 1, row: Some(3_000_100), marker: true },
            PeerStance { handle: 2, row: Some(6_000_200), marker: true },
            PeerStance { handle: 3, row: Some(8_000_000), marker: true },
            PeerStance { handle: 4, row: Some(2_000_000), marker: true },
        ];
        for wanted in [false, true] {
            let p = stance_patch_set(Some(OwnStance { row: 2_000_000, wanted }), &ours, &peers, true, &[]);
            let mut skipped: Vec<u32> = p.skip.iter().map(|s| s.1).collect();
            skipped.sort_unstable();
            if wanted {
                // our stanced active row is simply ours
                assert_eq!(p.want, vec![2_000_000, 8_000_000]);
                assert_eq!(skipped, vec![3_000_100, 6_000_200]);
            } else {
                assert_eq!(p.want, vec![8_000_000]);
                assert_eq!(skipped, vec![2_000_000, 3_000_100, 6_000_200]);
            }
            assert!(p.skip.iter().all(|s| s.2 == "shared_row"));
        }
        // a previously patched peer row we then equip (left) is restored
        let p = stance_patch_set(None, &[3_000_100], &peers[..1], true, &[3_000_100]);
        assert_eq!((p.want.clone(), p.restore), (vec![], vec![3_000_100]));
        // our own stance is refused when a left slot holds the same row (any upgrade level: +25 shares the row)
        assert!(own_shared_left(2_000_000, &[2_000_025, 2_000_000, -1, -1, -1, -1]));
        assert!(own_shared_left(2_000_000, &[-1, 2_000_000, -1, -1, 2_000_000, -1]));
        assert!(!own_shared_left(2_000_000, &ids));
        assert!(!own_shared_left(2_000_000, &[-1, 2_000_000, -1, 2_000_100, -1, -1]), "a second right slot is fine");
        assert_eq!(equipped_rows(&[-1, 0, -1, 0, -1, 0]), Vec::<u32>::new());
    }

    #[test]
    fn c6_stance_hysteresis() {
        let mut h = Hysteresis::default();
        assert_eq!(h.step(0.0, true, 1.5), Some(true));
        assert_eq!(h.step(0.5, false, 1.5), None, "too soon");
        assert_eq!(h.step(1.0, true, 1.5), None, "no change");
        assert_eq!(h.step(1.6, false, 1.5), Some(false));
        assert_eq!(h.step(2.0, true, 1.5), None);
        assert!(!h.on);
        assert_eq!(h.step(3.2, true, 1.5), Some(true));
        // a forced exit never waits
        assert!(h.force_off(3.3));
        assert!(!h.on && !h.force_off(3.4));
    }

    #[test]
    fn c6_chord_plans() {
        // enter from one-handed = one chord; from right-both = two (3 -> 1 -> 3)
        assert_eq!(enter_chords(ARM_ONE), 1);
        assert_eq!(enter_chords(ARM_EMPTY), 1);
        assert_eq!(enter_chords(ARM_RIGHT_BOTH), 2);
        // exit returns to the saved grip
        assert_eq!(exit_chords(ARM_ONE, ARM_RIGHT_BOTH), 1);
        assert_eq!(exit_chords(ARM_RIGHT_BOTH, ARM_RIGHT_BOTH), 2);
        assert_eq!(exit_chords(ARM_ONE, ARM_ONE), 0);
        assert_eq!(exit_chords(ARM_RIGHT_BOTH, ARM_ONE), 1);
        assert_eq!(exit_target(ARM_LEFT_BOTH), ARM_ONE);
        assert!(chord_took(ARM_ONE, ARM_RIGHT_BOTH) && chord_took(ARM_RIGHT_BOTH, ARM_ONE) && !chord_took(ARM_ONE, ARM_ONE));
        assert_eq!(chord_refusal(true, false, false, false, false, false), Some("menu"));
        assert_eq!(chord_refusal(false, true, false, true, false, false), Some("prompt"));
        assert_eq!(chord_refusal(false, true, false, false, false, false), Some("air"));
        assert_eq!(chord_refusal(false, false, false, false, false, false), None);
    }

    /// C6 aim 4 (aim4.md 5, the 18:54 run): a dead player refuses chords; a chord checked while dead is void; failed exit
    /// chords give up without disabling; two failed enter chords still disable; death / nuke / world-leave exits drop
    /// their chords.
    #[test]
    fn c6_chord_dead_void_and_exit_gave_up() {
        // the YOU DIED banner is a prompt too, but dead comes first (never counted)
        assert_eq!(chord_refusal(false, false, false, true, true, false), Some("dead"));
        assert_eq!(chord_refusal(true, false, false, false, true, false), Some("menu"));
        assert_eq!(chord_refusal(false, true, false, false, true, false), Some("dead"));
        // seq 5 took, seq 6 / 7 went to the dead player: void, the count never moves
        assert_eq!(chord_verdict(true, false, true, false, 1), (ChordVerdict::Took, 0));
        assert_eq!(chord_verdict(false, true, true, false, 0), (ChordVerdict::Void, 0));
        assert_eq!(chord_verdict(false, true, false, false, 1), (ChordVerdict::Void, 1));
        // two failed EXIT chords on a live player: give up, never disable
        assert_eq!(chord_verdict(false, false, true, false, 0), (ChordVerdict::Retry, 1));
        assert_eq!(chord_verdict(false, false, true, false, 1), (ChordVerdict::ExitGaveUp, 0));
        // two failed ENTER chords on a live player: the refresh fail (disable) as before
        assert_eq!(chord_verdict(false, false, false, false, 0), (ChordVerdict::Retry, 1));
        assert_eq!(chord_verdict(false, false, false, false, 1), (ChordVerdict::RefreshFail, 0));
        // the 18:54 replay: exit at the nuke's player prep (nuke=1) drops its chords; a death / world leave too
        assert_eq!(exit_drops_chords(true, true), Some("death"));
        assert_eq!(exit_drops_chords(true, false), Some("world"));
        assert_eq!(exit_drops_chords(false, false), Some("world"));
        assert_eq!(exit_drops_chords(false, true), None, "F6 / F5 / AC-130 / nuke (alive) exits keep their chords");
        let mut fails = 0;
        for (took, dead) in [(true, false), (false, true), (false, true)] {
            let (v, f) = chord_verdict(took, dead, true, false, fails);
            fails = f;
            assert!(!matches!(v, ChordVerdict::RefreshFail | ChordVerdict::ExitGaveUp), "{v:?}");
        }
        assert_eq!(fails, 0);
    }

    /// C6 round 6 (the 21:25:30 run, log lines 91913-92006): the respawn's world enter at 27.587, the stance entered at
    /// 27.591 with `arm=3` (two enter chords), the chords went out at 28.1 / 29.4 while ER still spawned the player (fade
    /// 1.0 / 0.9, camera 194 m away, MOVE blocked cutscene), both `3->3 took=0`; the second verdict came after the stance
    /// exited (first person at 30.147). Now: `spawn` refuses the chords (never counted), and an enter verdict after the
    /// stance left is void.
    #[test]
    fn c6_chord_spawn_refusal_and_stale_enter_void() {
        // the spawn window: always for 2 s, then while the fade / the far camera last, never past 12 s
        assert!(spawn_settling(0.0, Some(0.0), false));
        assert!(spawn_settling(1.99, None, false));
        assert!(spawn_settling(2.07, Some(0.9), false), "fade 0.9 at 29.66 (+2.07 s)");
        assert!(spawn_settling(3.0, Some(0.0), true), "the camera gate's dist");
        assert!(!spawn_settling(2.56, Some(0.0), false), "fade 0.00 at 30.147: settled");
        assert!(!spawn_settling(12.0, Some(1.0), true), "a fade plate that never clears holds at most 12 s");
        assert!(spawn_settling(f64::NAN, None, false), "no world-enter time yet = settling");
        assert!(!spawn_settling(2.5, Some(SPAWN_FADE - 0.001), false) && spawn_settling(2.5, Some(SPAWN_FADE), false));
        // the refusal order: menu, dead, spawn, prompt, air
        assert_eq!(chord_refusal(false, true, false, true, false, true), Some("spawn"));
        assert_eq!(chord_refusal(false, false, false, false, true, true), Some("dead"));
        assert_eq!(chord_refusal(true, false, false, false, false, true), Some("menu"));
        assert_eq!(chord_refusal(false, false, false, false, false, true), Some("spawn"));
        // the replay with the old rules: two took=0 enter chords = refresh fail
        let (v, f) = chord_verdict(false, false, false, false, 0);
        assert_eq!((v, f), (ChordVerdict::Retry, 1));
        assert_eq!(chord_verdict(false, false, false, false, f), (ChordVerdict::RefreshFail, 0));
        // now: the second enter chord's verdict arrives after the stance went off: void, the count kept
        assert_eq!(chord_verdict(false, false, false, true, f), (ChordVerdict::Void, 1));
        // an exit chord after the stance left is a normal exit chord (gave_up, never a disable)
        assert_eq!(chord_verdict(false, false, true, true, 1), (ChordVerdict::ExitGaveUp, 0));
        // a chord that took is a success whatever happened since
        assert_eq!(chord_verdict(true, false, false, true, 1), (ChordVerdict::Took, 0));
    }

    #[test]
    fn c6_aim_fields_are_the_crossbow_type() {
        assert_eq!(aim_vals(), [11, 55]);
        assert_eq!(AIM_FIELDS.map(|f| f.off), [OFF_WEAPON_CATEGORY, OFF_WEP_TYPE]);
        assert_eq!(AIM_FIELDS.map(|f| f.width), [1, 2]);
        // never a motion field, the bolt or the behaviour variation
        for a in AIM_FIELDS {
            for b in a.off..a.off + a.width as usize {
                for s in STANCE_FIELDS {
                    assert!(!(s.off..s.off + s.width as usize).contains(&b), "{} overlaps {}", a.name, s.name);
                }
                for (k, w) in [(OFF_ARROW_BOLT, 4), (OFF_BEHAVIOR_VARIATION, 4)] {
                    assert!(!(k..k + w).contains(&b));
                }
            }
        }
        // the aim values are what skip_reason calls ranged (a real crossbow is never stanced, so never "ours")
        assert_eq!(skip_reason(55, 11), Some("ranged"));
        let orig = [3, 9];
        assert_eq!(classify_aim(&orig, &orig), RowState::Original);
        assert_eq!(classify_aim(&aim_vals(), &orig), RowState::Stance);
        assert_eq!(classify_aim(&[3, 55], &orig), RowState::Foreign);
        assert_eq!(aim_text(&orig), "[3,9]");
        assert_eq!(PRECISION_SHOOTING, 1 << 1);
    }

    #[test]
    fn c6_aim_wanted_conditions() {
        let on = AimIn { flag: true, posed: true, ads: true, tp: true, input: true, ..AimIn::default() };
        assert!(aim_wanted(&on));
        for off in [
            AimIn { flag: false, ..on },
            AimIn { posed: false, ..on },
            AimIn { ads: false, ..on },
            AimIn { tp: false, ..on },
            AimIn { scoped: true, ..on },
            AimIn { ac130: true, ..on },
            AimIn { nuke: true, ..on },
            AimIn { dead: true, ..on },
            AimIn { menu: true, ..on },
            AimIn { prompt: true, ..on },
            AimIn { mounted: true, ..on },
            AimIn { air: true, ..on },
            AimIn { input: false, ..on },
        ] {
            assert!(!aim_wanted(&off), "{off:?}");
        }
    }

    /// The episode at 60 Hz: write on the ADS edge, the input from the next tick, on when the state shows, release the
    /// input first and restore once the state stayed clear; the input is never held while the fields are not written.
    #[test]
    fn c6_aim_episode_write_hold_confirm_release_restore() {
        let dt = 1.0 / 60.0;
        let mut a = AimCtl::default();
        assert_eq!(a.step(0.0, false, false), AimOut::default());
        let o = a.step(1.0, true, false);
        assert!(o.write && !o.hold && !o.restore && o.ev == Some(AimEv::Patch) && a.active());
        let o = a.step(1.0 + dt, true, false);
        assert!(o.hold && !o.write && o.ev.is_none());
        // ER needs ~0.2 s to raise the crossbow
        let mut t = 1.0 + dt;
        let mut on_ms = None;
        while t < 1.25 {
            t += dt;
            let o = a.step(t, true, t > 1.2);
            assert!(o.hold && !o.write && !o.restore);
            if let Some(AimEv::On { ms }) = o.ev {
                on_ms = Some(ms);
            }
        }
        assert!(a.on() && (190..=230).contains(&on_ms.unwrap()), "{on_ms:?}");
        // ADS up: the input goes first, the fields stay while the state is still set
        let o = a.step(2.0, false, true);
        assert!(!o.hold && !o.restore && a.active() && !a.on());
        let o = a.step(2.05, false, false);
        assert!(!o.hold && !o.restore);
        let o = a.step(2.16, false, false);
        assert!(o.restore && !o.hold && !a.active());
        assert!(matches!(o.ev, Some(AimEv::Restore { timeout: false, disabled: false, .. })));
        // a state that never clears: restored at the 0.4 s limit
        a.step(3.0, true, false);
        a.step(3.0 + dt, true, true);
        a.step(3.0 + 2.0 * dt, true, true);
        assert!(a.on());
        assert!(!a.step(4.0, false, true).restore);
        let o = a.step(4.0 + AIM_RELEASE_S, false, true);
        assert!(o.restore && matches!(o.ev, Some(AimEv::Restore { timeout: true, .. })));
        assert_eq!(a.fails(), 0);
    }

    /// Review C6 aim 4: after one confirmed episode, busy failures (a roll, a flask) never disable, and the same press
    /// re-arms AIM_RETRY_S after the failure.
    #[test]
    fn c6_aim_busy_fails_after_on_never_disable_and_rearm() {
        let mut a = AimCtl::default();
        a.step(0.0, true, false);
        a.step(0.02, true, true);
        assert!(a.step(0.04, true, true).ev.is_some() && a.on() && a.ever_on());
        a.step(0.5, false, false);
        let mut t = 0.6;
        while t < 1.2 {
            a.step(t, false, false);
            t += 1.0 / 60.0;
        }
        assert!(!a.active());
        // three presses in rolls: each fails, none disables
        let mut base = 2.0;
        for n in 1..=3u32 {
            let mut t = base;
            let mut evs = Vec::new();
            while t < base + 1.2 {
                evs.extend(a.step(t, true, false).ev);
                t += 1.0 / 60.0;
            }
            assert!(evs.contains(&AimEv::Fail { n }), "{evs:?}");
            assert!(!evs.iter().any(|e| matches!(e, AimEv::Restore { disabled: true, .. })) && !a.disabled());
            a.step(t, false, false);
            let mut t2 = t;
            while t2 < t + 0.6 {
                a.step(t2, false, false);
                t2 += 1.0 / 60.0;
            }
            base = t2 + 0.5;
        }
        // a press that starts in a roll: fail, restore, then the same press re-arms after AIM_RETRY_S and gets the pose
        let mut b = AimCtl::default();
        b.step(0.0, true, false);
        b.step(0.02, true, true);
        b.step(0.04, true, true);
        let mut t = 0.1;
        while t < 1.0 {
            b.step(t, false, false);
            t += 1.0 / 60.0;
        }
        let mut evs = Vec::new();
        let mut fail_t = None;
        let mut t = 2.0;
        while t < 5.0 {
            // the roll ends at 3.5 s: the state shows from then on
            let o = b.step(t, true, t >= 3.5);
            if matches!(o.ev, Some(AimEv::Fail { .. })) {
                fail_t.get_or_insert(t);
            }
            evs.extend(o.ev);
            t += 1.0 / 60.0;
        }
        let patches = evs.iter().filter(|e| matches!(e, AimEv::Patch)).count();
        assert!(patches >= 2 && b.on() && !b.disabled(), "re-armed in the same press: {evs:?}");
        assert!(fail_t.is_some());
    }

    #[test]
    fn c6_aim_repress_inside_the_release_keeps_the_fields() {
        let mut a = AimCtl::default();
        a.step(0.0, true, false);
        a.step(0.02, true, true);
        assert!(a.step(0.04, true, true).ev.is_some() && a.on());
        a.step(0.5, false, true);
        // pressed again 0.05 s later: no second write, the input held at once, on again with the state
        let o = a.step(0.55, true, true);
        assert!(o.hold && !o.write && !o.restore);
        let o = a.step(0.57, true, true);
        assert!(o.hold && matches!(o.ev, Some(AimEv::On { .. })) && a.on());
    }

    /// No state within 0.5 s: release, restore, one try per ADS press, two failures turn it off for the session.
    #[test]
    fn c6_aim_no_state_fails_twice_then_disabled() {
        let mut a = AimCtl::default();
        let mut t = 0.0;
        let mut evs = Vec::new();
        // first press, held 2 s, ER never aims (wepType cached at equip: aim4.md 6)
        while t < 2.0 {
            let o = a.step(t, true, false);
            assert!(!(o.hold && o.restore));
            evs.extend(o.ev);
            t += 1.0 / 60.0;
        }
        assert_eq!(evs.iter().filter(|e| matches!(e, AimEv::Patch)).count(), 1, "one try per press: {evs:?}");
        assert!(evs.contains(&AimEv::Fail { n: 1 }));
        assert!(evs.iter().any(|e| matches!(e, AimEv::Restore { disabled: false, .. })));
        assert!(!a.active() && !a.disabled());
        // released, pressed again: the second failure disables after its restore
        a.step(2.1, false, false);
        let mut t = 2.2;
        let mut last = None;
        while t < 4.0 {
            if let Some(e) = a.step(t, true, false).ev {
                last = Some(e);
            }
            t += 1.0 / 60.0;
        }
        assert!(matches!(last, Some(AimEv::Restore { disabled: true, .. })) && a.disabled() && a.fails() == 2);
        // never again this session
        a.step(4.1, false, false);
        assert_eq!(a.step(4.2, true, true), AimOut::default());
        // a hard stop / disable never leaves an episode running
        let mut b = AimCtl::default();
        b.step(0.0, true, false);
        assert!(b.stop() && !b.active() && !b.stop());
        b.disable();
        assert!(b.disabled() && b.step(1.0, true, true) == AimOut::default());
    }

    #[test]
    fn c6_stance_exit_defer() {
        let on = WantIn { flag: true, mode_on: true, in_world: true, ..WantIn::default() };
        // F6 into first person: deferrable; every other exit chords at once (review C6 pose: F5 mode off too)
        assert!(exit_deferrable(&WantIn { fp_on: true, ..on }));
        for hard in [
            WantIn { mode_on: false, ..on },
            WantIn { fp_on: true, mode_on: false, ..on },
            WantIn { fp_on: true, ac130: true, ..on },
            WantIn { fp_on: true, nuke: true, ..on },
            WantIn { fp_on: true, latched: true, ..on },
            WantIn { fp_on: true, disabled: true, ..on },
            WantIn { fp_on: true, flag: false, ..on },
            WantIn { fp_on: true, in_world: false, ..on },
            on,
        ] {
            assert!(!exit_deferrable(&hard), "{hard:?}");
        }
        // the 17:33:11 toggle: exit, F6 back 1.5 s later: no chords out, none back
        let mut d = ExitDefer::default();
        d.exit(10.0, 3.0, ARM_RIGHT_BOTH, Some(7), true);
        assert!(d.waiting() && !d.chords_allowed(10.5) && !d.chords_allowed(12.9));
        assert!(d.resume(11.5, ARM_RIGHT_BOTH, Some(7)));
        assert!(!d.waiting() && d.chords_allowed(11.6));
        // a longer first person: the exit's chords go once the window ran out; a later enter chords as before
        d.exit(20.0, 3.0, ARM_RIGHT_BOTH, Some(7), true);
        assert!(!d.ran_out() && !d.chords_allowed(22.9) && d.chords_allowed(23.0) && !d.waiting() && d.ran_out());
        assert!(!d.resume(23.5, ARM_RIGHT_BOTH, Some(7)) && !d.ran_out());
        // the grip changed meanwhile (the user one-handed in first person) / the stance had not settled: chords
        d.exit(30.0, 3.0, ARM_RIGHT_BOTH, Some(7), true);
        assert!(!d.resume(31.0, ARM_ONE, Some(7)));
        d.exit(40.0, 3.0, ARM_RIGHT_BOTH, Some(7), false);
        assert!(!d.resume(41.0, ARM_RIGHT_BOTH, Some(7)));
        // review C6 pose: another weapon equipped in first person (row A out, row B back): the re-resolve chords
        d.exit(45.0, 3.0, ARM_RIGHT_BOTH, Some(7), true);
        assert!(!d.resume(46.0, ARM_RIGHT_BOTH, Some(8)));
        d.exit(47.0, 3.0, ARM_RIGHT_BOTH, None, true);
        assert!(!d.resume(48.0, ARM_RIGHT_BOTH, None));
        // defer 0 = the first build (chords at once)
        d.exit(50.0, 0.0, ARM_RIGHT_BOTH, Some(7), true);
        assert!(!d.waiting() && d.chords_allowed(50.0) && !d.ran_out() && !d.resume(50.1, ARM_RIGHT_BOTH, Some(7)));
        d.exit(60.0, 3.0, ARM_ONE, Some(7), true);
        d.clear();
        assert!(!d.waiting() && !d.ran_out());
        // the busy refusal of a ran-out deferral in first person
        assert!(exit_chord_busy(true, false, 9.0) && exit_chord_busy(false, true, 9.0) && exit_chord_busy(false, false, 0.2));
        assert!(!exit_chord_busy(false, false, BUSY_MOVE_S));
    }

    #[test]
    fn c6_stance_stray_guard() {
        let mut g = StrayGuard::default();
        // our own rows never count
        assert_eq!(g.note(0.0, 1, 79_100_105, None), Stray::Known);
        assert_eq!(g.note(0.0, 2, 79_800_001, None), Stray::Known);
        // a use-item bullet within 3 s is not counted
        assert_eq!(g.note(1.0, 3, 10_170_000, Some(0.5)), Stray::UseItem);
        // one stray counted, the same handle again is not a second
        assert_eq!(g.note(2.0, 4, 52_000_000, None), Stray::Counted(1));
        assert_eq!(g.note(2.1, 4, 52_000_000, None), Stray::Known);
        // a second within 10 s disables
        assert_eq!(g.note(5.0, 5, 52_000_000, None), Stray::Disable(2));
        // spread out: never two in the window
        let mut g = StrayGuard::default();
        assert_eq!(g.note(0.0, 1, 52_000_000, None), Stray::Counted(1));
        assert_eq!(g.note(11.0, 2, 52_000_000, None), Stray::Counted(1));
        assert!(mod_row(79_000_963) && !mod_row(52_000_000));
    }

    /// C6 part 16.9 (work/c6/xbow8.md 4): the er_busy gate, the played / ignored / fail verdict and the ignored backoff.
    #[test]
    fn c6_chord_verdict_ignored_er_busy_backoff() {
        // the logged idle / locomotion heads accept a chord; jump / get-up / roll / grip switch are busy
        for a in [16_000_000, 16_020_120, 16_022_100, 16_020_150, 0] {
            assert!(er_idle_head(a), "{a}");
        }
        for a in [10_202_040, 63_000, 16_004_050, 16_005_100, 16_005_200, 16_029_060] {
            assert!(!er_idle_head(a), "{a}");
        }
        assert!(!er_busy(Some(16_000_000), 0.5) && er_busy(Some(16_000_000), 0.1), "idle for 0.3 s");
        assert!(er_busy(Some(10_202_040), 5.0), "the 23:42:22 jump head");
        assert!(!er_busy(None, 0.0), "an unreadable head never blocks");
        let mut h = HeadAge::default();
        assert_eq!(h.step(1.0, Some(16_000_000)), 0.0);
        assert!((h.step(1.4, Some(16_000_000)) - 0.4).abs() < 1e-9);
        assert_eq!(h.step(1.5, Some(10_202_040)), 0.0);
        // a grip-switch id anywhere in the queue = played
        assert!(chord_played(&[16_000_000, 16_029_060, -1]) && chord_played(&[29_040]) && !chord_played(&[16_000_000, 10_202_040]));
        // the 23:42:22 / 21:25:28 failures: no grip anim = ignored (never counted); played but wrong = fail
        assert!(chord_ignored(false, Some(false)) && !chord_ignored(false, Some(true)) && !chord_ignored(false, None) && !chord_ignored(true, Some(false)));
        assert_eq!(
            [chord_verdict_label(true, Some(true)), chord_verdict_label(false, Some(false)), chord_verdict_label(false, Some(true)), chord_verdict_label(false, None)],
            ["played", "ignored", "fail", "fail"]
        );
        // backoff 0.5 / 1 / 2 / 4 / 4, the 6th in 20 s waits for the window
        let mut b = IgnoredBackoff::default();
        let d: Vec<f64> = (0..5).map(|k| b.delay(k as f64)).collect();
        assert_eq!(d, vec![0.5, 1.0, 2.0, 4.0, 4.0]);
        assert!((b.delay(5.0) - 15.0).abs() < 1e-9, "the 6th waits until the first leaves the 20 s window");
        assert_eq!(b.count(), 6);
        b.clear();
        assert_eq!(b.delay(100.0), 0.5);
        // 25 s later the window is empty again
        assert_eq!(b.delay(125.0), 0.5);
    }

    /// C6 part 16 addendum (the 10:00:35 replay): the aborted keyboard chord ER had already played (29060 at 35.612,
    /// arm 3 at 35.713) finishes with a verdict; one ER never saw is refused and retried.
    #[test]
    fn c6_chord_abort_after_played_finishes() {
        let q = [16_029_060, 16_000_000, -1];
        assert!(abort_finishes(chord_played(&q)), "ER played the grip switch: the chord is done, not refused");
        assert!(!abort_finishes(chord_played(&[16_000_000, -1])), "no grip switch yet: aborted, retried");
        // the verdict once checked: arm 1 -> 3 took, played
        assert_eq!(chord_verdict_label(chord_took(1, 3), Some(true)), "played");
    }

    fn ep(now: f64, ours: bool, prec: bool) -> EpisodeIn {
        EpisodeIn { now, ours, prec, ads: ours, head: Some(16_000_000), arm: 3, camtype: Some(0), snipe: Some(1.0), loaded: Some(false), bolts: Some(20), cam_keep: true, carrier: true, mode_on: true, row: Some(43_020_000), ..Default::default() }
    }

    /// C6 part 16 addendum: the aim animation family and the per-episode line (start, ER's aim anim, the gap, the cap).
    #[test]
    fn c6_aim_episode_line() {
        for a in [46_037_130, 46_037_100, 46_037_110, 46_037_120, 37_000] {
            assert!(aim_anim(a), "{a}");
        }
        for a in [16_000_000, 16_029_060, 46_036_999, 46_038_000, -1, 0] {
            assert!(!aim_anim(a), "{a}");
        }
        let mut log = EpisodeLog::default();
        assert!(log.step(&ep(0.0, false, false)).is_none(), "nothing before an episode");
        let mut t = 1.0;
        let mut out = None;
        // our episode 1.0..2.0 s, ER's bit from 1.1 s to 2.1 s, its aim anim from 1.5 s; the camera switched to er at 1.8 s
        while t < 3.0 && out.is_none() {
            let mut i = ep(t, t < 2.0, (1.1..2.1).contains(&t));
            if t >= 1.5 && t < 2.0 {
                i.queue_aim = Some(46_037_130);
                i.camtype = Some(2);
            }
            if t >= 1.8 {
                i.cam_keep = false;
            }
            out = log.step(&i);
            t = ((t + 0.05) * 1000.0).round() / 1000.0;
        }
        let e = out.expect("the episode ends 0.3 s after the bit dropped");
        assert!((e.t0 - 1.0).abs() < 1e-9 && (e.t1 - 2.05).abs() < 1e-9, "{e:?}");
        assert_eq!(e.aim_first, Some((46_037_130, 500)));
        assert!(e.prec_at == Some(100) && e.prec_ms == 1000 && e.ours_ms == 950, "{e:?}");
        assert!(e.cam0 && !e.cam1 && e.camtypes == 0b101, "{e:?}");
        let txt = e.text();
        for s in ["carrier=1", "cam=keep>er", "arm=3->3", "aim=46037130", "aim_at_ms=500", "camtypes=[0,2]", "loaded=0->0", "bolts=20->20", "row=43020000"] {
            assert!(txt.contains(s), "{s} in {txt}");
        }
        assert_eq!(log.count(), 1);
        // the 09:58 carrier episode: ER's bit on, never an aim anim
        let mut log = EpisodeLog::default();
        let mut out = None;
        for k in 0..40 {
            let t = 10.0 + k as f64 * 0.05;
            out = out.or(log.step(&ep(t, t < 10.8, (10.1..10.9).contains(&t))));
        }
        let e = out.expect("ended");
        assert!(e.aim_first.is_none() && e.text().contains("aim=none aim_at_ms=-"), "{}", e.text());
        // the cap: at most EPISODE_MAX lines
        let mut log = EpisodeLog::default();
        let mut lines = 0;
        for k in 0..(EPISODE_MAX + 5) {
            let t0 = k as f64 * 2.0;
            for (dt, on) in [(0.0, true), (0.1, false), (0.5, false)] {
                lines += log.step(&ep(t0 + dt, on, false)).is_some() as u32;
            }
        }
        assert_eq!(lines, EPISODE_MAX);
    }

    /// C6 part 17: the polearm set is the vanilla Spear's (36 / 2 / 0 / 3 / 13), differs from the crossbow set and from
    /// the Longsword's own row (23 / 2 / 0 / 0 / 10), and a row written with one set reads Stance only against that set.
    #[test]
    fn c6_polearm_set_is_the_vanilla_spear() {
        assert_eq!(POLEARM_VALS, [36, 2, 0, 3, 13]);
        assert_eq!(StanceSet::Polearm.vals(), POLEARM_VALS);
        assert_eq!(StanceSet::Crossbow.vals(), stance_vals());
        assert_eq!(StanceSet::default(), StanceSet::Crossbow);
        assert_eq!((StanceSet::Crossbow.label(), StanceSet::Polearm.label()), ("crossbow", "polearm"));
        let longsword: Vals = [23, 2, 0, 0, 10];
        assert_ne!(POLEARM_VALS, longsword);
        assert_ne!(POLEARM_VALS, stance_vals());
        assert_eq!(classify_set(&POLEARM_VALS, &longsword, &POLEARM_VALS), RowState::Stance);
        assert_eq!(classify_set(&longsword, &longsword, &POLEARM_VALS), RowState::Original);
        // a row written with the polearm set is foreign to the crossbow classification (a hot set change restores first)
        assert_eq!(classify_set(&POLEARM_VALS, &longsword, &stance_vals()), RowState::Foreign);
        assert_eq!(classify(&stance_vals(), &longsword), RowState::Stance);
        // never the kept fields: the five offsets are the motion fields only
        for f in STANCE_FIELDS {
            assert!(![OFF_WEAPON_CATEGORY, OFF_WEP_TYPE, OFF_ARROW_BOLT, OFF_BEHAVIOR_VARIATION].contains(&f.off));
        }
    }

    fn gs(moved: Option<(f32, f32, bool)>, head: Option<i32>) -> GuardSample {
        GuardSample { moved, head, arm: 3, align: None, yaw: false, corr: None }
    }

    /// C6 part 17 review: the guard carry is sampled only standing, posed, ER idle on the base-set idle, and not right
    /// after a guard episode (ER's guard-lower animation).
    #[test]
    fn c6_guard_carry_ok_only_on_the_settled_idle() {
        assert!(guard_carry_ok(true, true, Some(13_000_000), false, 10.0));
        assert!(guard_carry_ok(true, true, None, false, f64::INFINITY), "an unreadable head does not block");
        assert!(!guard_carry_ok(false, true, Some(13_000_000), false, 10.0), "moving: no standing carry");
        assert!(!guard_carry_ok(true, false, Some(13_000_000), false, 10.0), "not posed");
        assert!(!guard_carry_ok(true, true, Some(13_000_000), true, 10.0), "ER busy (head age)");
        assert!(!guard_carry_ok(true, true, Some(13_017_020), false, 10.0), "a guard animation");
        assert!(!guard_carry_ok(true, true, Some(13_020_000), false, 10.0), "locomotion, not the idle");
        assert!(!guard_carry_ok(true, true, Some(13_000_000), false, 0.3), "the guard lower after a release");
        assert!(guard_carry_ok(true, true, Some(13_000_000), false, GUARD_CARRY_QUIET_S));
    }

    /// C6 part 17: hold from the first wanted tick, detect on two weapon-frame samples off the carry, release at once,
    /// one line per episode with the anim ids, the arm, the detection and the align.
    #[test]
    fn c6_guard_episode_hold_detect_release() {
        let mut g = GuardCtl::default();
        assert!(!g.active() && g.step(0.0, false, Some(2_000_000), "pad", &gs(None, None)).ended.is_none());
        let o = g.step(1.0, true, Some(2_000_000), "pad", &gs(Some((0.02, 2.0, false)), Some(13_000_000)));
        assert!(o.hold && o.started && g.active() && !g.detected() && !g.fallback());
        // one moved sample is not enough
        let o = g.step(1.1, true, Some(2_000_000), "pad", &gs(Some((0.2, 30.0, true)), Some(13_017_000)));
        assert!(o.hold && o.detected_now == Some((100, "anim")), "the guard anim family detects at once: {o:?}");
        let mut g = GuardCtl::default();
        g.step(1.0, true, Some(2_000_000), "pad", &gs(Some((0.02, 2.0, false)), Some(13_000_000)));
        let o = g.step(1.1, true, Some(2_000_000), "pad", &gs(Some((0.2, 30.0, true)), Some(13_000_100)));
        assert!(o.hold && o.detected_now.is_none());
        let o = g.step(1.2, true, Some(2_000_000), "pad", &gs(Some((0.25, 35.0, true)), Some(13_000_110)));
        assert_eq!(o.detected_now, Some((200, "frame")));
        assert!(g.detected() && !g.fallback());
        let mut s = gs(None, Some(13_000_110));
        s.align = Some((12.5, false));
        s.yaw = true;
        s.corr = Some("builtin");
        g.step(1.3, true, Some(2_000_000), "pad", &s);
        s.align = Some((8.0, false));
        g.step(1.4, true, Some(2_000_000), "pad", &s);
        let o = g.step(1.45, false, Some(2_000_000), "pad", &gs(None, None));
        assert!(!o.hold && !g.active());
        let e = o.ended.expect("the release ends the episode");
        assert_eq!((e.detected, e.align_first, e.align_min, e.yaw_writes, e.fallback), (Some((200, "frame")), Some(12.5), Some(8.0), 2, None));
        assert_eq!(e.heads, vec![13_000_000, 13_000_100, 13_000_110]);
        assert_eq!(e.guard_anim, None);
        let txt = e.text();
        for k in ["ms=450", "row=2000000", "route=pad", "arm=3->3", "head0=13000000", "heads=[13000000,13000100,13000110]", "guard_anim=none", "detected=1 via=frame det_ms=200", "moved_max=0.250", "turned_max=35.0", "align=12.5 align_min=8.0 clamped=0 fallback=0 fb_ms=- yaw=2 corr=builtin"] {
            assert!(txt.contains(k), "{k} in {txt}");
        }
        assert_eq!(g.count(), 1);
    }

    /// C6 part 17: not detected within GUARD_CONFIRM_S = the fallback (the gun raise) until the guard shows; an anim id
    /// of the guard family detects it too; the align is counted only once detected; a hard stop ends the episode.
    #[test]
    fn c6_guard_fallback_then_late_detect() {
        let mut g = GuardCtl::default();
        let mut fb = None;
        for k in 0..4 {
            let t = 5.0 + k as f64 * 0.25;
            let mut s = gs(Some((0.01, 1.0, false)), Some(13_000_000));
            s.align = Some((40.0, true));
            fb = fb.or(g.step(t, true, None, "kb", &s).fallback_now);
        }
        assert_eq!(fb, Some(750));
        assert!(g.fallback() && !g.detected());
        let o = g.step(6.0, true, None, "kb", &gs(None, Some(37_017_000)));
        assert_eq!(o.detected_now, Some((1000, "anim")));
        assert!(!g.fallback(), "detected late: the align takes over");
        let e = g.stop(6.05).expect("ended");
        assert_eq!((e.fallback, e.align_first, e.clamped), (Some(750), None, false), "the clamps before the detection are not the guard's");
        assert!(e.text().contains("route=kb") && e.text().contains("row=-1") && e.text().contains("fallback=1 fb_ms=750") && e.text().ends_with("corr=off"), "{}", e.text());
        assert!(g.stop(6.1).is_none(), "no episode, no line");
        // the cap
        let mut g = GuardCtl::default();
        let mut lines = 0;
        for k in 0..(GUARD_EPISODE_MAX + 3) {
            let t = k as f64;
            g.step(t, true, None, "pad", &gs(None, None));
            lines += g.step(t + 0.5, false, None, "pad", &gs(None, None)).ended.is_some() as u32;
        }
        assert_eq!(lines, GUARD_EPISODE_MAX);
        assert!(guard_anim(13_017_000) && guard_anim(17_000) && !guard_anim(16_000_000) && !guard_anim(46_037_130) && !guard_anim(-1));
        // C6 part 17 addendum [L 11:46-11:51]: the polearm guard 13019000 / 13019200 / 13019500 detect; its lower 13019100
        // (the head after every release), the idle 13000000 / 13000100 and the walk 13020120 do not
        assert!(guard_anim(13_019_000) && guard_anim(13_019_200) && guard_anim(13_019_500));
        assert!(!guard_anim(13_019_100) && !guard_anim(13_000_000) && !guard_anim(13_000_100) && !guard_anim(13_020_120) && !guard_anim(5111));
        let mut g = GuardCtl::default();
        g.step(0.0, true, None, "pad", &gs(None, Some(13_020_120)));
        assert_eq!(g.step(0.1, true, None, "pad", &gs(None, Some(13_019_000))).detected_now, Some((100, "anim")), "a guard started walking");
        let mut g = GuardCtl::default();
        assert!(g.step(0.0, true, None, "pad", &gs(None, Some(13_019_100))).detected_now.is_none(), "a re-press during the lower");
    }

    /// C6 part 17: the guard's body-yaw write only while held, standing, not locked on, turning allowed, no throw, alive.
    #[test]
    fn c6_guard_yaw_gate() {
        let ok = GuardYawIn { flag: true, held: true, action_flags: Some(0), ..Default::default() };
        assert!(guard_yaw_wanted(&ok));
        let turn_off = 1u64 << crate::fp_math::AF_DISABLE_TURNING;
        for (bad, why) in [
            (GuardYawIn { flag: false, ..ok }, "flag"),
            (GuardYawIn { held: false, ..ok }, "held"),
            (GuardYawIn { move_input: true, ..ok }, "move input"),
            (GuardYawIn { moved_recently: true, ..ok }, "moving"),
            (GuardYawIn { locked_on: true, ..ok }, "lock-on"),
            (GuardYawIn { action_flags: None, ..ok }, "unreadable flags"),
            (GuardYawIn { action_flags: Some(turn_off), ..ok }, "turning disabled"),
            (GuardYawIn { throw: true, ..ok }, "throw"),
            (GuardYawIn { dead: true, ..ok }, "dead"),
        ] {
            assert!(!guard_yaw_wanted(&bad), "{why}");
        }
    }

    /// C6 part 17: the guard uses the aim pose's gates (aim_wanted with the guard flag): never scoped (the scope peek),
    /// in first person, the gunship, the nuke, dead, a menu, a prompt, mounted, mid-air or without a fresh input route.
    #[test]
    fn c6_guard_wanted_gates() {
        let ok = AimIn { flag: true, posed: true, ads: true, tp: true, input: true, ..Default::default() };
        assert!(aim_wanted(&ok));
        for bad in [
            AimIn { flag: false, ..ok },
            AimIn { posed: false, ..ok },
            AimIn { ads: false, ..ok },
            AimIn { tp: false, ..ok },
            AimIn { scoped: true, ..ok },
            AimIn { ac130: true, ..ok },
            AimIn { nuke: true, ..ok },
            AimIn { dead: true, ..ok },
            AimIn { menu: true, ..ok },
            AimIn { prompt: true, ..ok },
            AimIn { mounted: true, ..ok },
            AimIn { air: true, ..ok },
            AimIn { input: false, ..ok },
        ] {
            assert!(!aim_wanted(&bad), "{bad:?}");
        }
    }
}
