//! M7 nuke target selection and staggering (docs/M7_DESIGN.md 3.10-3.11). Pure.
//!
//! Who dies: every live character within `radius_m` except
//! - the main player and anything in the player / ghost / summon-buddy (spirit ashes, Torrent) / debug ChrSets;
//! - system dummies (`npc_param_id < 20 000 000`: bonfires, talk dummies, BuddyStone, caravan dummies);
//! - `hit.exclude_character_ids` (8000 Torrent);
//! - phantoms / players by chr_type (only `Npc` and the summon NPC types the sets did not catch are characters here);
//! - with `spare_friendly` (default): runtime `team_type` in `spare_teams` (a deny-list: bosses use 15 different
//!   team types, so an allow-list of "enemy" teams would spare bosses; a merchant turned hostile 26 -> 27 dies).
//! A character in another havok frame (`pos_w` differs from the player's) has no comparable distance: it is counted
//! and logged as `other_frame` (never silently dropped), not killed.
//! An invincible character (event flag) is deferred to the retry passes, not skipped; a retry kills it only once it
//! reads as not invincible (a scripted phase change is never cut short).
//!
//! Safety of the friendly exclusion (run 15): the runtime team byte is [I]. [`team_check`] refuses every NPC kill when
//! the census teams look implausible, and `nuke.dry_run` logs the selection without applying anything. MW2 itself has no radius and no
//! team check (`spare_friendly = false` gives "everyone dies"; Torrent and spirit summons are still skipped: they are
//! part of the player).

use std::collections::{BTreeMap, VecDeque};

/// The NpcParam id below which a character is a system dummy, never killed.
pub const DUMMY_NPC_PARAM_BELOW: i32 = 20_000_000;
/// `ChrType::Npc`.
pub const CHR_TYPE_NPC: i32 = 5;

/// One character as the DLL read it (plain data).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cand {
    pub handle: u64,
    pub npc_param_id: i32,
    pub character_id: u32,
    pub team: u8,
    pub chr_type: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub dead_flag: bool,
    pub invincible: bool,
    pub active: bool,
    pub dist_m: f32,
    /// In the player / ghost / summon-buddy / debug ChrSets.
    pub in_player_sets: bool,
    pub is_main_player: bool,
    /// Same havok frame as the player (`pos_w` equal): `dist_m` is comparable.
    pub same_frame: bool,
    /// C3: `ChrSetEntry.chr_update_type` of the character (`upd=` in the census). Logged only: it is not the network
    /// owner (2026-10-24 review: solo sessions read `upd={remote:30}` and killed all of them).
    pub owner: Owner,
}

/// C3: `ChrSetEntry.chr_update_type` as read (0 `Local`, 4 `Remote`, else / unread `Unknown`). [I] when built as the
/// network owner; refuted 2026-10-24: offline solo censuses read `upd={remote:30}` / `{remote:31}` and the solo nuke
/// killed 24/24, 22/22, 20/20 of them, and in the two-player Seamless run both games read every enemy as `remote` (so
/// nobody killed). Kept for the census histogram only; the co-op kill rule is [`coop_hold`] (host first, client fallback).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Owner {
    Local,
    Remote,
    #[default]
    Unknown,
}

impl Owner {
    pub fn from_update_type(upd: Option<u8>) -> Owner {
        match upd {
            Some(0) => Owner::Local,
            Some(4) => Owner::Remote,
            _ => Owner::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Owner::Local => "local",
            Owner::Remote => "remote",
            Owner::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    MainPlayer,
    Set,
    Dummy,
    ExcludedId,
    ChrType,
    Team,
    Dead,
    Inactive,
    Range,
    OtherFrame,
    /// C3 co-op nuke (review 2026-10-24): a client holds its kills while the host's game kills first ([`coop_hold`]).
    AwaitHost,
}

impl Skip {
    pub fn label(self) -> &'static str {
        match self {
            Skip::MainPlayer => "player",
            Skip::Set => "set",
            Skip::Dummy => "dummy",
            Skip::ExcludedId => "torrent",
            Skip::ChrType => "chr_type",
            Skip::Team => "team",
            Skip::Dead => "dead",
            Skip::Inactive => "inactive",
            Skip::Range => "range",
            Skip::OtherFrame => "other_frame",
            Skip::AwaitHost => "await_host",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetCfg {
    pub radius_m: f32,
    pub spare_friendly: bool,
    pub spare_teams: Vec<u8>,
    pub exclude_character_ids: Vec<u32>,
    /// C3 co-op nuke with `coop_everyone`: no radius (and another havok frame is no reason to spare a character).
    pub no_radius: bool,
    /// C3 co-op nuke `owned` (review 2026-10-24): this pass holds every NPC kill (a client before its fallback, see
    /// [`coop_hold`]); held characters are re-checked on every pass.
    pub hold: bool,
}

impl TargetCfg {
    pub fn from_config(c: &crate::config::Config) -> Self {
        TargetCfg {
            radius_m: c.nuke.radius_m,
            spare_friendly: c.nuke.spare_friendly,
            spare_teams: c.nuke.spare_teams.clone(),
            exclude_character_ids: c.hit.exclude_character_ids.clone(),
            no_radius: false,
            hold: false,
        }
    }

    /// C3 (docs/C3_DESIGN.md 3.8, review 2026-10-24): the co-op rules of a pass with `peers` other players
    /// (`coop_mode = "owned"`): `hold` = this game waits for the host's kills this pass ([`coop_hold`]);
    /// `coop_everyone` drops the radius.
    pub fn with_coop(mut self, peers: u32, owned: bool, everyone: bool, hold: bool) -> Self {
        if peers > 0 && owned {
            self.hold = hold;
            self.no_radius = everyone;
        }
        self
    }
}

/// Ok(true) = kill now, Ok(false) = eligible but invincible right now (deferred to the retries), Err = skipped.
pub fn classify(c: &Cand, cfg: &TargetCfg) -> Result<bool, Skip> {
    if c.is_main_player {
        return Err(Skip::MainPlayer);
    }
    if c.in_player_sets {
        return Err(Skip::Set);
    }
    if c.npc_param_id < DUMMY_NPC_PARAM_BELOW {
        return Err(Skip::Dummy);
    }
    if cfg.exclude_character_ids.contains(&c.character_id) {
        return Err(Skip::ExcludedId);
    }
    if c.chr_type != CHR_TYPE_NPC {
        return Err(Skip::ChrType);
    }
    if c.hp <= 0 || c.max_hp <= 0 || c.dead_flag {
        return Err(Skip::Dead);
    }
    if !c.active {
        return Err(Skip::Inactive);
    }
    if !cfg.no_radius {
        if !c.same_frame {
            return Err(Skip::OtherFrame);
        }
        if !(c.dist_m.is_finite() && c.dist_m <= cfg.radius_m) {
            return Err(Skip::Range);
        }
    }
    if cfg.spare_friendly && cfg.spare_teams.contains(&c.team) {
        return Err(Skip::Team);
    }
    // last: a held character is one this game would kill now (alive, eligible, in range)
    if cfg.hold {
        return Err(Skip::AwaitHost);
    }
    Ok(!c.invincible)
}

/// The kill list (nearest first), the deferred (invincible) ones and the skip counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub kill: Vec<u64>,
    pub deferred: Vec<u64>,
    pub skips: BTreeMap<Skip, u32>,
    /// Characters per team in the radius (the census line `by_team={..}`).
    pub by_team: BTreeMap<u8, u32>,
    pub in_radius: u32,
    /// Live, eligible-looking characters in another havok frame (distance unknown): counted, not killed.
    pub other_frame: u32,
    /// C3 (review 2026-10-24): characters this pass would kill but holds for the host's game (a client before its
    /// fallback).
    pub held: Vec<u64>,
    /// C3: the `chr_update_type` histogram of the counted characters (`upd={local:..,remote:..,unknown:..}`; logged only).
    pub by_owner: BTreeMap<&'static str, u32>,
}

impl Selection {
    pub fn skipped(&self) -> u32 {
        self.skips.values().sum()
    }

    /// `team:3,set:1` (merged counts per reason).
    pub fn skip_text(&self) -> String {
        if self.skips.is_empty() {
            return "-".into();
        }
        self.skips.iter().map(|(k, v)| format!("{}:{v}", k.label())).collect::<Vec<_>>().join(",")
    }
}

pub fn select(cands: &[Cand], cfg: &TargetCfg) -> Selection {
    let mut order: Vec<&Cand> = cands.iter().collect();
    order.sort_by(|a, b| a.dist_m.total_cmp(&b.dist_m));
    let mut s = Selection::default();
    let mut seen = std::collections::HashSet::new();
    for c in order {
        if !seen.insert(c.handle) {
            continue; // the union of the lists may list one character twice
        }
        let counted = (cfg.no_radius || (c.same_frame && c.dist_m.is_finite() && c.dist_m <= cfg.radius_m)) && !c.is_main_player;
        if counted {
            s.in_radius += 1;
            *s.by_team.entry(c.team).or_insert(0) += 1;
            *s.by_owner.entry(c.owner.label()).or_insert(0) += 1;
        }
        match classify(c, cfg) {
            Ok(true) => s.kill.push(c.handle),
            Ok(false) => s.deferred.push(c.handle),
            // out of range is not a "skip" of the census (the census counts the radius only)
            Err(Skip::Range | Skip::MainPlayer) => {}
            Err(Skip::OtherFrame) => s.other_frame += 1,
            Err(k) => {
                if k == Skip::AwaitHost {
                    s.held.push(c.handle);
                }
                if counted {
                    *s.skips.entry(k).or_insert(0) += 1;
                }
            }
        }
    }
    s
}

/// The `teamType` values of the vanilla NpcParam (7045 rows, `work\regulation\unpacked` scan, 2026-10-18) [V].
/// Runtime values outside these and `spare_teams` mean the team byte is not what we think it is.
pub const VANILLA_TEAMS: [u8; 35] = [
    0, 1, 2, 5, 6, 7, 8, 9, 10, 11, 24, 26, 27, 28, 29, 30, 33, 47, 48, 49, 50, 51, 52, 54, 55, 56, 57, 58, 59, 60, 61, 63, 64, 65, 66,
];

/// Below this many characters in the radius, a single shared team value is normal (a camp of soldiers).
pub const TEAM_UNIFORM_MIN: u32 = 4;

/// Why the NPC kills of a pass are refused (`NUKE blocked reason=team_unverified ...`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TeamDoubt {
    /// A runtime team value no vanilla NpcParam row and no `spare_teams` entry has.
    Unknown(Vec<u8>),
    /// Every character in the radius reads one team that is itself spared (a zeroed / wrong byte reads 0 everywhere).
    UniformSpared(u8, u32),
}

impl TeamDoubt {
    pub fn text(&self) -> String {
        match self {
            TeamDoubt::Unknown(v) => format!("unknown_values={v:?}"),
            TeamDoubt::UniformSpared(t, n) => format!("uniform team={t} n={n} (a spared team for every character)"),
        }
    }
}

/// Is the census team histogram plausible? `Ok` = the NPC kills may go ahead (with `spare_friendly`).
pub fn team_check(by_team: &BTreeMap<u8, u32>, spare_teams: &[u8]) -> Result<(), TeamDoubt> {
    let unknown: Vec<u8> = by_team.keys().copied().filter(|t| !VANILLA_TEAMS.contains(t) && !spare_teams.contains(t)).collect();
    if !unknown.is_empty() {
        return Err(TeamDoubt::Unknown(unknown));
    }
    if by_team.len() == 1 {
        let (t, n) = by_team.iter().next().map(|(t, n)| (*t, *n)).unwrap_or((0, 0));
        if n >= TEAM_UNIFORM_MIN && spare_teams.contains(&t) {
            return Err(TeamDoubt::UniformSpared(t, n));
        }
    }
    Ok(())
}

/// Whether the NPC kills are armed: not a dry run and the teams look right. `Err(reason)` = log, do not apply.
pub fn npc_kills_armed(dry_run: bool, spare_friendly: bool, by_team: &BTreeMap<u8, u32>, spare_teams: &[u8]) -> Result<(), String> {
    if dry_run {
        return Err("dry_run".into());
    }
    if spare_friendly && let Err(d) = team_check(by_team, spare_teams) {
        return Err(format!("team_unverified {}", d.text()));
    }
    Ok(())
}

/// The death watch: at most `hz` passes per second, at most `cap` targets per pass, round-robin (bounded cost per
/// frame whatever the number of targets).
#[derive(Clone, Debug, Default)]
pub struct WatchSched {
    last: Option<f64>,
    cursor: usize,
}

impl WatchSched {
    /// The indices (into a list of `n` watched targets, in a stable order) to re-read now; empty when not due.
    pub fn pick(&mut self, now: f64, hz: f64, n: usize, cap: usize) -> Vec<usize> {
        if n == 0 || self.last.is_some_and(|l| now - l < 1.0 / hz - 1e-9) {
            return Vec::new();
        }
        self.last = Some(now);
        let k = cap.min(n);
        let start = self.cursor % n;
        self.cursor = (start + k) % n;
        (0..k).map(|i| (start + i) % n).collect()
    }
}

/// A target is watched until confirmed dead or given up: every pass applied (`applied > retries`) and `grace_s` since
/// the last application.
pub fn watch_given_up(applied: u32, retries: usize, since_last_apply_s: f64, grace_s: f64) -> bool {
    applied as usize > retries && since_last_apply_s >= grace_s
}

/// Applies at most `per_tick` kills per tick (spreads ER's death / item-lot work over ticks).
#[derive(Clone, Debug, Default)]
pub struct Stagger {
    queue: VecDeque<u64>,
}

impl Stagger {
    pub fn push_all(&mut self, h: &[u64]) {
        for x in h {
            if !self.queue.contains(x) {
                self.queue.push_back(*x);
            }
        }
    }

    pub fn take(&mut self, per_tick: u32) -> Vec<u64> {
        let n = (per_tick as usize).min(self.queue.len());
        self.queue.drain(..n).collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

/// Lethal bullets allowed this tick (`kill_mode` bullet / both): at most `per_tick` and never above the emitter
/// throttle (the bullets are priority spawns, but they leave the guns' room alone).
pub fn bullets_allowed(live_emitters: Option<u32>, per_tick: u32, throttle: u32) -> u32 {
    match live_emitters {
        None => per_tick.min(1),
        Some(l) => per_tick.min(throttle.saturating_sub(l)),
    }
}

/// C3 co-op nuke (docs/C3_DESIGN.md 3.8): from the detonation's first kill pass, a census pass every `every_s` for
/// `window_s` (ownership may move while the peers' games kill theirs; a peer-owned character is re-checked each pass).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoopKillSched {
    next: f64,
    end: f64,
    pub passes: u32,
}

impl CoopKillSched {
    pub fn start(at: f64, window_s: f64) -> Self {
        CoopKillSched { next: at, end: at + window_s.max(0.0), passes: 0 }
    }

    pub fn active(&self, now: f64) -> bool {
        now <= self.end && self.end > 0.0
    }

    /// True when a pass is due now (at most one per call).
    pub fn due(&mut self, now: f64, every_s: f64) -> bool {
        if !self.active(now) || now + 1e-9 < self.next {
            return false;
        }
        self.next = now + every_s.max(0.05);
        self.passes += 1;
        true
    }
}

/// C3 co-op nuke (review 2026-10-24): this game's session role for the kill rule (`COOP session ... role=`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoopRole {
    Host,
    Client,
    #[default]
    Unknown,
}

impl CoopRole {
    pub fn from_label(s: &str) -> CoopRole {
        match s {
            "host" => CoopRole::Host,
            "client" => CoopRole::Client,
            _ => CoopRole::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CoopRole::Host => "host",
            CoopRole::Client => "client",
            CoopRole::Unknown => "unknown",
        }
    }
}

/// C3 co-op nuke kill rule (review 2026-10-24). No per-enemy ownership signal is readable (`chr_update_type` reads 4 for
/// every enemy in solo and in both co-op games; `network_authority` reads 0 for all but one), so:
/// - the host's game applies the lethal effect to every eligible enemy from its first kill pass;
/// - a client (or a game whose role is unknown) holds its kills for `fallback_s` after its own first kill pass, then
///   applies to every eligible enemy still alive (the host's kills it has observed are dead and drop out as `dead`).
///
/// An application by a game that does not simulate the enemy has no effect (C2: 42 of 49 host applications left the
/// enemy at full HP, the 7 that worked died on the peer too), so every enemy dies once, in the game that owns it.
/// `fallback_s` is clamped to the window so the fallback always runs inside it. True = hold this pass.
pub fn coop_hold(role: CoopRole, since_first_pass_s: f64, fallback_s: f64, window_s: f64) -> bool {
    match role {
        CoopRole::Host => false,
        CoopRole::Client | CoopRole::Unknown => since_first_pass_s + 1e-9 < fallback_s.clamp(0.0, window_s.max(0.0)),
    }
}

/// The horizon explosions only while the emitter pool is quiet.
pub fn horizon_allowed(live_emitters: Option<u32>, max_live: u32) -> bool {
    live_emitters.is_some_and(|l| l < max_live)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(handle: u64, npc: i32, chr_id: u32, team: u8, dist: f32) -> Cand {
        Cand {
            handle,
            npc_param_id: npc,
            character_id: chr_id,
            team,
            chr_type: CHR_TYPE_NPC,
            hp: 500,
            max_hp: 500,
            dead_flag: false,
            invincible: false,
            active: true,
            dist_m: dist,
            in_player_sets: false,
            is_main_player: false,
            same_frame: true,
            owner: Owner::Unknown,
        }
    }

    fn cfg(spare: bool) -> TargetCfg {
        TargetCfg {
            radius_m: 200.0,
            spare_friendly: spare,
            spare_teams: vec![0, 2, 8, 10, 12, 20, 26, 28, 30, 47],
            exclude_character_ids: vec![8000],
            no_radius: false,
            hold: false,
        }
    }

    /// The census of the vanilla NpcParam scan (docs/M7_DESIGN.md 3.11) as fixtures.
    fn census() -> Vec<Cand> {
        let torrent = Cand { in_player_sets: true, ..cand(1, 80_000_000, 8000, 10, 2.0) };
        let malenia = cand(2, 21_000_000, 2120, 33, 30.0);
        let talk_dummy = cand(3, 10_006_050, 1000, 33, 5.0);
        let kale = cand(4, 30_000_000, 3010, 26, 12.0);
        let kale_hostile = cand(5, 30_000_000, 3010, 27, 13.0);
        let wolf = cand(6, 41_000_000, 4300, 48, 50.0);
        let spirit = cand(7, 25_000_000, 4500, 47, 3.0);
        let spirit_in_set = Cand { in_player_sets: true, ..cand(8, 25_000_001, 4501, 47, 3.5) };
        let far = cand(9, 43_000_000, 4311, 6, 250.0);
        let phantom = Cand { chr_type: 1, ..cand(10, 50_000_000, 0, 1, 10.0) };
        let dead = Cand { hp: 0, ..cand(11, 43_000_000, 4311, 6, 20.0) };
        let invincible = Cand { invincible: true, ..cand(12, 21_000_000, 2120, 33, 40.0) };
        let player = Cand { is_main_player: true, ..cand(13, 0, 0, 0, 0.0) };
        let ghostflame = cand(14, 46_000_000, 4600, 64, 120.0);
        vec![torrent, malenia, talk_dummy, kale, kale_hostile, wolf, spirit, spirit_in_set, far, phantom, dead, invincible, player, ghostflame]
    }

    #[test]
    fn nuke_targets_census_spare_friendly() {
        let s = select(&census(), &cfg(true));
        // nearest first: Kale hostile 13 m, Malenia 30, wolf 50, Ghostflame 120
        assert_eq!(s.kill, vec![5, 2, 6, 14]);
        assert_eq!(s.deferred, vec![12], "invincible boss: retried, not skipped");
        assert_eq!(s.skips.get(&Skip::Set), Some(&2), "Torrent + the summoned spirit");
        assert_eq!(s.skips.get(&Skip::Dummy), Some(&1), "talk dummy 10006050");
        assert_eq!(s.skips.get(&Skip::Team), Some(&2), "Kale (26) and a spirit outside the sets (47)");
        assert_eq!(s.skips.get(&Skip::ChrType), Some(&1));
        assert_eq!(s.skips.get(&Skip::Dead), Some(&1));
        assert_eq!(s.in_radius, 12, "everyone but the player and the far one");
        // killed + deferred + skipped = in radius; no skip without a reason
        assert_eq!(s.kill.len() as u32 + s.deferred.len() as u32 + s.skipped(), s.in_radius);
        assert!(s.skip_text().contains("team:2"));
    }

    #[test]
    fn nuke_targets_census_everyone_dies() {
        let s = select(&census(), &cfg(false));
        assert_eq!(s.kill, vec![7, 4, 5, 2, 6, 14], "spirit outside the sets (47) and Kale (26) die too; nearest first");
        assert!(!s.skips.contains_key(&Skip::Team));
        assert_eq!(s.skips.get(&Skip::Set), Some(&2), "Torrent and summoned spirits are still spared");
        assert_eq!(s.kill.len() as u32 + s.deferred.len() as u32 + s.skipped(), s.in_radius);
    }

    #[test]
    fn nuke_targets_torrent_by_id_and_duplicates() {
        // Torrent outside the summon set (a read miss) is still caught by the excluded id 8000
        let t = Cand { in_player_sets: false, ..cand(1, 80_000_000, 8000, 10, 2.0) };
        assert_eq!(classify(&t, &cfg(false)), Err(Skip::ExcludedId));
        let s = select(&[cand(5, 30_000_000, 1, 27, 1.0), cand(5, 30_000_000, 1, 27, 1.0)], &cfg(true));
        assert_eq!(s.kill, vec![5], "a character listed twice dies once");
        assert_eq!(classify(&Cand { active: false, ..cand(6, 41_000_000, 4300, 48, 5.0) }, &cfg(true)), Err(Skip::Inactive));
        assert_eq!(classify(&cand(6, 41_000_000, 4300, 48, f32::NAN), &cfg(true)), Err(Skip::Range));
    }

    #[test]
    fn nuke_targets_census_other_frame_counted_not_dropped() {
        // a wolf 30 m away by its raw position but in another havok frame: not killed, not in the radius, counted
        let mut c = census();
        c.push(Cand { same_frame: false, ..cand(20, 41_000_000, 4300, 48, 30.0) });
        c.push(Cand { same_frame: false, ..cand(21, 41_000_000, 4300, 48, 5000.0) });
        assert_eq!(classify(&c[c.len() - 2], &cfg(true)), Err(Skip::OtherFrame));
        let s = select(&c, &cfg(true));
        assert_eq!(s.other_frame, 2, "both counted, whatever their raw distance");
        assert!(!s.kill.contains(&20) && !s.deferred.contains(&20));
        assert_eq!(s.in_radius, 12, "the radius count is unchanged");
        assert_eq!(s.kill.len() as u32 + s.deferred.len() as u32 + s.skipped(), s.in_radius);
        assert_eq!(Skip::OtherFrame.label(), "other_frame");
    }

    #[test]
    fn nuke_targets_team_check_refuses_implausible() {
        let spare = cfg(true).spare_teams;
        let h = |v: &[(u8, u32)]| v.iter().copied().collect::<BTreeMap<u8, u32>>();
        // the fixture census: plausible
        assert_eq!(team_check(&select(&census(), &cfg(true)).by_team, &spare), Ok(()));
        // a camp of 12 soldiers on team 6: normal
        assert_eq!(team_check(&h(&[(6, 12)]), &spare), Ok(()));
        // a value no NpcParam row has (a wrong offset reading another byte)
        assert_eq!(team_check(&h(&[(6, 3), (200, 2)]), &spare), Err(TeamDoubt::Unknown(vec![200])));
        // everyone reads 0 (zeroed byte) with 4+ characters: refused; 3 characters on 0: allowed (too few to tell)
        assert_eq!(team_check(&h(&[(0, 9)]), &spare), Err(TeamDoubt::UniformSpared(0, 9)));
        assert_eq!(team_check(&h(&[(0, 3)]), &spare), Ok(()));
        // the user's spare_teams may add values (12, 20 are not in the vanilla scan)
        assert_eq!(team_check(&h(&[(6, 3), (12, 1), (20, 1)]), &spare), Ok(()));
        assert!(VANILLA_TEAMS.windows(2).all(|w| w[0] < w[1]));
        // arming: dry run first, then the team check (only with spare_friendly)
        assert_eq!(npc_kills_armed(true, true, &h(&[(6, 3)]), &spare), Err("dry_run".into()));
        assert_eq!(npc_kills_armed(false, true, &h(&[(6, 3)]), &spare), Ok(()));
        assert!(npc_kills_armed(false, true, &h(&[(0, 9)]), &spare).unwrap_err().starts_with("team_unverified uniform team=0 n=9"));
        assert_eq!(npc_kills_armed(false, false, &h(&[(0, 9)]), &spare), Ok(()), "MW2 everyone-dies mode ignores teams");
    }

    #[test]
    fn nuke_targets_watch_bounded_round_robin() {
        let mut w = WatchSched::default();
        assert_eq!(w.pick(0.0, 10.0, 5, 32), vec![0, 1, 2, 3, 4], "few targets: all");
        assert!(w.pick(0.05, 10.0, 5, 32).is_empty(), "10 Hz: not due 50 ms later");
        // 500 targets, cap 32: every pass reads 32, round-robin covers all in 16 passes (1.6 s at 10 Hz)
        let mut w = WatchSched::default();
        let mut seen = std::collections::HashSet::new();
        let mut reads = 0;
        for k in 0..16 {
            let p = w.pick(k as f64 * 0.1, 10.0, 500, 32);
            assert!(p.len() <= 32);
            reads += p.len();
            seen.extend(p);
        }
        assert_eq!(seen.len(), 500);
        assert_eq!(reads, 512);
        // at 144 Hz for 1 s: at most 11 passes x 32 reads (no per-frame cost growth)
        let mut w = WatchSched::default();
        let total: usize = (0..144).map(|f| w.pick(f as f64 / 144.0, 10.0, 500, 32).len()).sum();
        assert!(total <= 11 * 32, "{total}");
        assert!(w.pick(100.0, 10.0, 0, 32).is_empty());
        // given up after every pass was applied and 2 s passed
        assert!(!watch_given_up(4, 4, 10.0, 2.0) && !watch_given_up(5, 4, 1.0, 2.0) && watch_given_up(5, 4, 2.0, 2.0));
    }

    #[test]
    fn c3_nukecoop_upd_is_not_ownership() {
        // the census owner (chr_update_type) never decides a kill any more: Remote / Local / Unknown are all killed by
        // the host's pass, and all held by a client's pass before its fallback
        let c = vec![
            Cand { owner: Owner::Local, ..cand(1, 43_000_000, 4311, 6, 20.0) },
            Cand { owner: Owner::Remote, ..cand(2, 43_000_000, 4311, 6, 25.0) },
            Cand { owner: Owner::Unknown, ..cand(3, 43_000_000, 4311, 6, 30.0) },
        ];
        let host = select(&c, &cfg(true).with_coop(1, true, true, false));
        assert_eq!(host.kill, vec![1, 2, 3], "upd=4 (remote) is killed by the host");
        assert!(host.held.is_empty());
        assert_eq!(host.by_owner.get("remote"), Some(&1), "the upd histogram is still logged");
        let client = select(&c, &cfg(true).with_coop(1, true, true, true));
        assert!(client.kill.is_empty());
        assert_eq!(client.held, vec![1, 2, 3]);
        assert_eq!(client.skips.get(&Skip::AwaitHost), Some(&3));
        // solo / coop_mode local: never held (C2 behaviour)
        assert_eq!(select(&c, &cfg(true).with_coop(0, true, true, true)).kill, vec![1, 2, 3]);
        assert_eq!(select(&c, &cfg(true).with_coop(1, false, true, true)).kill, vec![1, 2, 3], "coop_mode local");
        assert_eq!(Owner::from_update_type(Some(0)), Owner::Local);
        assert_eq!(Owner::from_update_type(Some(4)), Owner::Remote);
        assert_eq!(Owner::from_update_type(Some(2)), Owner::Unknown);
        assert_eq!(Owner::from_update_type(None), Owner::Unknown);
    }

    /// The census of the 2026-10-24 two-player run (release 0.3.0, co-op id 11c2066c) as the DLL read it: host pass=0
    /// at 13:15:27.473 `by_distance=20 by_team={0:7,2:1,6:10,9:1,26:1} upd={remote:20}` with skips dummy 6, chr_type 1,
    /// dead 5 and 8 live enemies (the 8 handles of `skip reason=remote_owner`); the friend's pass=0 at 14:15:27.747
    /// `by_distance=21 by_team={0:7,2:1,6:11,9:1,26:1} upd={remote:21}`, the same 8 handles + 1. Every entry upd=4.
    fn census_c6_2026_10_24(friend: bool) -> Vec<Cand> {
        let live: [u64; 8] = [
            0x0a00_0000_1000_00bf,
            0x0a00_0000_1000_0070,
            0x0a00_0000_1000_005d,
            0x0a00_0000_1000_0072,
            0x0a00_0000_1000_0004,
            0x0a00_0000_1000_00ca,
            0x0a00_0000_1000_007b,
            0x0a00_0000_1000_0006,
        ];
        let r = |c: Cand| Cand { owner: Owner::Remote, ..c };
        let mut v = Vec::new();
        let mut h = 0x0a00_0000_1000_0100u64;
        let mut next = || {
            h += 1;
            h
        };
        for k in 0..6 {
            v.push(r(cand(next(), 10_000_000 + k, 1000, 0, 5.0 + k as f32))); // dummies (team 0)
        }
        v.push(r(Cand { chr_type: 1, ..cand(next(), 50_000_000, 0, 0, 8.0) })); // the peer player (team 0)
        for t in [2u8, 26, 6, 6, 6] {
            v.push(r(Cand { hp: 0, dead_flag: true, ..cand(next(), 43_000_000, 4311, t, 30.0) }));
        }
        for (i, hh) in live.iter().enumerate() {
            v.push(r(cand(*hh, 43_000_000, 4311, if i == 7 { 9 } else { 6 }, 40.0 + i as f32)));
        }
        if friend {
            v.push(r(cand(0x0a00_0000_1000_0099, 43_000_000, 4311, 6, 60.0)));
        }
        v
    }

    #[test]
    fn c3_nukecoop_c6_census_host_kills_client_holds_then_falls_back() {
        let tc = |hold: bool| cfg(true).with_coop(1, true, true, hold);
        let (fallback, window) = (1.0, 6.0);
        // the host: role=host never holds -> the 8 live enemies are killed (the run: kill=0 skipped=20)
        let host = census_c6_2026_10_24(false);
        let hold = coop_hold(CoopRole::from_label("host"), 0.0, fallback, window);
        assert!(!hold);
        let s = select(&host, &tc(hold));
        assert_eq!(s.in_radius, 20);
        assert_eq!(s.by_owner.get("remote"), Some(&20), "upd={{remote:20}} as logged");
        assert_eq!(s.kill.len(), 8, "{:?}", s.skips);
        assert_eq!((s.skips.get(&Skip::Dummy), s.skips.get(&Skip::ChrType), s.skips.get(&Skip::Dead)), (Some(&6), Some(&1), Some(&5)));
        assert!(s.held.is_empty() && !s.skips.contains_key(&Skip::AwaitHost));
        let teams: Vec<(u8, u32)> = s.by_team.iter().map(|(t, n)| (*t, *n)).collect();
        assert_eq!(teams, vec![(0, 7), (2, 1), (6, 10), (9, 1), (26, 1)], "by_team as logged");
        assert_eq!(team_check(&s.by_team, &cfg(true).spare_teams), Ok(()), "armed=1 as logged");
        assert_eq!(s.kill.len() as u32 + s.deferred.len() as u32 + s.skipped(), s.in_radius);
        // the client (role=client): holds the 9 on its first passes
        let fr = census_c6_2026_10_24(true);
        let role = CoopRole::from_label("client");
        let s0 = select(&fr, &tc(coop_hold(role, 0.0, fallback, window)));
        assert!(s0.kill.is_empty());
        assert_eq!(s0.held.len(), 9);
        assert_eq!(s0.skipped(), 21, "skipped=21 as logged, now await_host instead of remote_owner");
        // the host's kills observed (its 8 read dead here): the fallback pass finds only the one the host did not kill
        let mut after = fr.clone();
        for c in after.iter_mut().filter(|c| s.kill.contains(&c.handle)) {
            c.hp = 0;
        }
        let s1 = select(&after, &tc(coop_hold(role, 1.0, fallback, window)));
        assert_eq!(s1.kill, vec![0x0a00_0000_1000_0099], "only the enemy the host's game did not kill");
        // the host's kills not observed at all (the 2026-10-24 outcome): the client applies to all 9 itself
        let s2 = select(&fr, &tc(coop_hold(role, 1.25, fallback, window)));
        assert_eq!(s2.kill.len(), 9);
        assert!(s2.held.is_empty());
        // every live enemy is applied by at least one game
        let union: std::collections::HashSet<u64> = s.kill.iter().chain(s2.kill.iter()).copied().collect();
        assert_eq!(union.len(), 9);
    }

    /// Review 2026-10-24: a peer's marker that meets a dead receiver (host_c6_2026-10-24.log: dead in the world
    /// 13:27:52-13:30:44, `STATE ... chars=12`) or a loading screen (friend_c6_2026-10-24.log 14:15:40.565 `NUKE end
    /// reason=loading`, its respawn) is no longer dropped: dead starts a spectating sequence whose kill passes still
    /// select this game's enemies (`census pass=0 ... kill=8`, then `coop kills local=8 left=0`), loading pends.
    #[test]
    fn c3_nukecoop_dead_or_loading_receiver_still_kills() {
        use crate::nuke_logic::{Ev, NukeSeq, PendPoll, PendingMarker, Plan, RemoteAct, RemoteIn, remote_marker};
        let base = RemoteIn { owned: true, allow_in_coop: true, ready: true, busy: false, loading: false, alive: true };
        assert_eq!(remote_marker(&base), RemoteAct::Start { spectate: false });
        assert_eq!(remote_marker(&RemoteIn { alive: false, ..base }), RemoteAct::Start { spectate: true });
        assert_eq!(remote_marker(&RemoteIn { loading: true, alive: false, ..base }), RemoteAct::Pend);
        assert_eq!(remote_marker(&RemoteIn { busy: true, alive: false, ..base }), RemoteAct::Ignore("busy"));
        assert_eq!(remote_marker(&RemoteIn { owned: false, ..base }), RemoteAct::Ignore("coop_mode_local"));
        assert_eq!(remote_marker(&RemoteIn { allow_in_coop: false, ..base }), RemoteAct::Ignore("allow_in_coop_0"));
        assert_eq!(remote_marker(&RemoteIn { ready: false, ..base }), RemoteAct::Ignore("disabled_or_rows"));
        // pending: waits through the loading screen, starts on the first tick in the world, expires with the window
        let p = PendingMarker { from: 0xffff_ffff_16f0_0001, via: "bullet", until: 100.0 + 13.0 + 6.0 };
        assert_eq!(p.poll(105.0, true), PendPoll::Wait);
        assert_eq!(p.poll(106.0, false), PendPoll::Due);
        assert_eq!(p.poll(119.0, false), PendPoll::Expired);
        // the dead host's spectating sequence: no player death, the kills unchanged
        let cfgn = crate::config::Nuke::default();
        let plan = Plan::new(&crate::nuke_data::tests::fixture().timeline, &cfgn);
        let sp = plan.clone().spectating();
        assert!(!sp.player_dies && sp.kill_at == plan.kill_at && sp.retries == plan.retries);
        let evs = NukeSeq::new(0.0, sp, true).tick(1.0e3);
        assert!(evs.contains(&Ev::Kill(0)) && !evs.iter().any(|e| matches!(e, Ev::PlayerPrep | Ev::PlayerLethal)));
        // its pass 0 as the host (never holds): the 8 live enemies of the C6 census
        let host = census_c6_2026_10_24(false);
        let s0 = select(&host, &cfg(true).with_coop(1, true, true, coop_hold(CoopRole::Host, 0.0, 1.0, 6.0)));
        assert_eq!((s0.kill.len(), s0.held.len()), (8, 0));
        // as the client: held for the fallback, then all 9 (no host kills observed)
        let fr = census_c6_2026_10_24(true);
        let s1 = select(&fr, &cfg(true).with_coop(1, true, true, coop_hold(CoopRole::Client, 1.0, 1.0, 6.0)));
        assert_eq!((s1.kill.len(), s1.held.len()), (9, 0));
    }

    #[test]
    fn c3_nukecoop_hold_rule() {
        // host: never; client / unknown: until the fallback, clamped to the window
        assert!(!coop_hold(CoopRole::Host, 0.0, 1.0, 6.0));
        assert!(coop_hold(CoopRole::Client, 0.0, 1.0, 6.0) && coop_hold(CoopRole::Client, 0.75, 1.0, 6.0));
        assert!(!coop_hold(CoopRole::Client, 1.0, 1.0, 6.0) && !coop_hold(CoopRole::Client, 5.0, 1.0, 6.0));
        assert!(coop_hold(CoopRole::Unknown, 0.5, 1.0, 6.0) && !coop_hold(CoopRole::Unknown, 1.0, 1.0, 6.0));
        assert!(!coop_hold(CoopRole::Client, 6.0, 60.0, 6.0), "a fallback past the window still runs at its end");
        assert!(!coop_hold(CoopRole::Client, 0.0, 0.0, 6.0), "fallback 0 = every game at once");
        assert_eq!(CoopRole::from_label("solo"), CoopRole::Unknown);
        assert_eq!((CoopRole::Host.label(), CoopRole::Client.label(), CoopRole::Unknown.label()), ("host", "client", "unknown"));
        assert_eq!(Skip::AwaitHost.label(), "await_host");
        // the window passes at 4 Hz from the first kill pass: with fallback 1.0 the client holds pass 0 and 101..103
        // (t = 0, 0.25, 0.5, 0.75) and applies from pass 104 (t = 1.0)
        let held: Vec<bool> = (0..6).map(|k| coop_hold(CoopRole::Client, k as f64 * 0.25, 1.0, 6.0)).collect();
        assert_eq!(held, vec![true, true, true, true, false, false]);
    }

    #[test]
    fn c3_nukecoop_no_radius_with_everyone() {
        let mut c = census();
        c.push(Cand { same_frame: false, ..cand(20, 41_000_000, 4300, 48, 30.0) });
        // coop_everyone: the far (250 m) soldier and the other-frame wolf die too; the player sets stay spared
        let s = select(&c, &cfg(true).with_coop(1, true, true, false));
        assert!(s.kill.contains(&9) && s.kill.contains(&20), "{:?}", s.kill);
        assert_eq!(s.other_frame, 0);
        assert_eq!(s.skips.get(&Skip::Set), Some(&2));
        assert!(!s.kill.contains(&13), "never the main player");
        // without everyone the radius stays
        let s2 = select(&c, &cfg(true).with_coop(1, true, false, false));
        assert!(!s2.kill.contains(&9) && !s2.kill.contains(&20));
    }

    #[test]
    fn c3_nukecoop_window_passes() {
        let mut k = CoopKillSched::start(10.0, 6.0);
        let mut n = 0;
        let mut t = 10.0;
        while t < 20.0 {
            if k.due(t, 0.25) {
                n += 1;
            }
            t += 1.0 / 60.0;
        }
        assert!((24..=26).contains(&n), "{n} passes in a 6 s window at 4 Hz");
        assert!(!k.active(16.5) && k.active(15.9));
        assert!(!CoopKillSched::default().active(1.0));
        assert_eq!(k.passes, n);
    }

    #[test]
    fn nuke_targets_stagger_and_pool_caps() {
        let mut st = Stagger::default();
        let hs: Vec<u64> = (1..=40).collect();
        st.push_all(&hs);
        st.push_all(&[3, 41]);
        assert_eq!(st.len(), 41, "no duplicate in the queue");
        assert_eq!(st.take(16), (1..=16).collect::<Vec<_>>());
        assert_eq!(st.take(16).len(), 16);
        assert_eq!(st.take(16).len(), 9);
        assert!(st.is_empty());
        // bullets never above the emitter throttle; horizon only below 160
        assert_eq!(bullets_allowed(Some(10), 8, 224), 8);
        assert_eq!(bullets_allowed(Some(220), 8, 224), 4);
        assert_eq!(bullets_allowed(Some(230), 8, 224), 0);
        assert_eq!(bullets_allowed(None, 8, 224), 1, "unreadable: one at a time");
        assert!(horizon_allowed(Some(159), 160) && !horizon_allowed(Some(160), 160) && !horizon_allowed(None, 160));
    }
}
