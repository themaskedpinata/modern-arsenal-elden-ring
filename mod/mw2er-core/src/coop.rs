//! Co-op C2 (docs/COOP_PLAN.md): who counts as a co-op peer, the debounced peer set behind the
//! `COOP session peers=<n>` lines, the session role (host / client) and what peers mean for the nuke.
//!
//! Nuke in a session (user decision 2026-10-19, revised): `[nuke] allow_in_coop = true` (default) keeps the local
//! nuke (the caller's own enemies + the caller dies; peers are never targeted: the kill pass skips the player set),
//! logs `coop_peers=<n> role=<host|client|unknown>` on the activation and shows a one-time HUD note per co-op session.
//! `allow_in_coop = false` refuses key 4 while a peer is present (`NUKE blocked reason=coop_session peers=<n>`) and
//! cancels a countdown with refund when a peer joins (`NUKE cancel reason=coop_session refund=1`).
//!
//! The DLL reads `WorldChrMan.player_chr_set` read-only (game/chars.rs `player_set_slots`) into plain [`PeerSlot`]s;
//! everything here is pure.
//!
//! Detection rule (vendored fromsoftware-rs 2.7.1.0 layout; [I] until the C2 run shows a peer's line):
//! a slot of the player ChrSet is a peer when it holds a character that is
//! - not the local player (`WorldChrMan.main_player`, also by handle),
//! - not the Spears of the Church `sinner_hunter` PlayerIns,
//! - in a loaded / loading state (`ChrLoadStatus` 1..=4; 0 Unloaded and 5 Unloading are ignored),
//! - not an NPC phantom type (`ChrType` 5 Npc, 19 WhiteSummonNpc, 20 BloodyFingerNpc, 21 RecusantNpc: NPC summons
//!   and NPC invaders keep the solo behaviour; they are counted as `npc=` for the log only).
//!
//! A false positive only refuses the nuke (safe side); a false negative is the risk the C2 run checks.

use crate::logfmt;

/// ChrType values that are NPCs dressed as players (never a human peer).
pub const NPC_CHR_TYPES: [i32; 4] = [5, 19, 20, 21];
/// A peer that disappears is dropped only after this long (loading screens / warps make entries flicker); a new
/// peer counts at once (the nuke is refused from the first tick it is seen).
pub const LEAVE_DEBOUNCE_S: f64 = 3.0;
/// The player set is read this often (a handful of slots: cheap).
pub const READ_EVERY_S: f64 = 0.25;
/// Never more slots than this are considered (the vendored set is small; a garbage capacity is clamped).
pub const MAX_SLOTS: usize = 64;

/// One slot of `WorldChrMan.player_chr_set` as plain data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerSlot {
    pub addr: usize,
    pub handle: u64,
    pub chr_type: i32,
    /// `ChrSetEntry.chr_load_status` (2 = Active).
    pub load_status: u8,
    /// `ChrSetEntry.chr_update_type` (0 Local, 4 Remote) - logged only.
    pub update_type: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Empty,
    Local,
    SinnerHunter,
    NotLoaded,
    NpcPlayer,
    Peer,
}

pub fn classify(s: &PeerSlot, main_addr: usize, main_handle: u64, sinner: usize) -> SlotKind {
    if s.addr == 0 {
        return SlotKind::Empty;
    }
    if s.addr == main_addr || (main_handle != 0 && s.handle == main_handle) {
        return SlotKind::Local;
    }
    if sinner != 0 && s.addr == sinner {
        return SlotKind::SinnerHunter;
    }
    if !(1..=4).contains(&s.load_status) {
        return SlotKind::NotLoaded;
    }
    if NPC_CHR_TYPES.contains(&s.chr_type) {
        return SlotKind::NpcPlayer;
    }
    SlotKind::Peer
}

/// This tick's view of the player set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerScan {
    pub peers: Vec<PeerSlot>,
    pub npc_players: u32,
    pub slots: u32,
}

/// Classifies every slot (at most [`MAX_SLOTS`]); `None` without a local player (not in the world: no decision).
pub fn scan(slots: &[PeerSlot], main_addr: usize, main_handle: u64, sinner: usize) -> Option<PeerScan> {
    if main_addr == 0 {
        return None;
    }
    let mut out = PeerScan { slots: slots.len().min(MAX_SLOTS) as u32, ..PeerScan::default() };
    for s in slots.iter().take(MAX_SLOTS) {
        match classify(s, main_addr, main_handle, sinner) {
            SlotKind::Peer => {
                if !out.peers.iter().any(|p| p.handle == s.handle && p.addr == s.addr) {
                    out.peers.push(*s);
                }
            }
            SlotKind::NpcPlayer => out.npc_players += 1,
            _ => {}
        }
    }
    out.peers.sort_by_key(|p| p.handle);
    Some(out)
}

/// One applied change of the peer set (`COOP session peers=<n> was=<m> ...`).
#[derive(Clone, Debug, PartialEq)]
pub struct PeerChange {
    pub peers: u32,
    pub was: u32,
    pub joined: Vec<u64>,
    pub left: Vec<u64>,
    /// The applied set after the change (handle, chr_type, load status, update type).
    pub set: Vec<PeerSlot>,
    pub npc_players: u32,
}

impl PeerChange {
    /// `peers=1 was=0 joined=[0x..] left=[] handles=[0x..:type=1:st=2:upd=4] npc=0`
    pub fn line(&self) -> String {
        let hs = |v: &[u64]| v.iter().map(|h| logfmt::handle(*h)).collect::<Vec<_>>().join(",");
        let set = self
            .set
            .iter()
            .map(|p| format!("{}:type={}:st={}:upd={}", logfmt::handle(p.handle), p.chr_type, p.load_status, p.update_type))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "peers={} was={} joined=[{}] left=[{}] handles=[{}] npc={}",
            self.peers,
            self.was,
            hs(&self.joined),
            hs(&self.left),
            set,
            self.npc_players
        )
    }
}

/// The debounced peer set: joins apply at once, a peer missing from the scans for [`LEAVE_DEBOUNCE_S`] is dropped.
#[derive(Clone, Debug, Default)]
pub struct PeerTracker {
    applied: Vec<PeerSlot>,
    /// handle -> first time it was missing
    missing_since: Vec<(u64, f64)>,
}

impl PeerTracker {
    pub fn peers(&self) -> u32 {
        self.applied.len() as u32
    }

    pub fn handles(&self) -> Vec<u64> {
        self.applied.iter().map(|p| p.handle).collect()
    }

    /// C3 PASS C: the applied peer slots (addresses for the peers' SpEffect lists and poses).
    pub fn slots(&self) -> &[PeerSlot] {
        &self.applied
    }

    /// One scan (`None` = not in the world / unreadable: the applied set is kept, nothing changes).
    pub fn update(&mut self, now: f64, scan: Option<&PeerScan>) -> Option<PeerChange> {
        let scan = scan?;
        let was = self.applied.len() as u32;
        let mut joined = Vec::new();
        let mut left = Vec::new();
        for p in &scan.peers {
            self.missing_since.retain(|(h, _)| *h != p.handle);
            if let Some(a) = self.applied.iter_mut().find(|a| a.handle == p.handle) {
                *a = *p; // the state fields refresh silently
            } else {
                self.applied.push(*p);
                joined.push(p.handle);
            }
        }
        let present: Vec<u64> = scan.peers.iter().map(|p| p.handle).collect();
        for a in self.applied.clone() {
            if present.contains(&a.handle) {
                continue;
            }
            let since = match self.missing_since.iter().find(|(h, _)| *h == a.handle) {
                Some((_, t)) => *t,
                None => {
                    self.missing_since.push((a.handle, now));
                    now
                }
            };
            if now - since >= LEAVE_DEBOUNCE_S {
                self.applied.retain(|x| x.handle != a.handle);
                self.missing_since.retain(|(h, _)| *h != a.handle);
                left.push(a.handle);
            }
        }
        if joined.is_empty() && left.is_empty() {
            return None;
        }
        self.applied.sort_by_key(|p| p.handle);
        joined.sort_unstable();
        left.sort_unstable();
        Some(PeerChange { peers: self.applied.len() as u32, was, joined, left, set: self.applied.clone(), npc_players: scan.npc_players })
    }
}

/// The peers the nuke gate sees: 0 when `[nuke] allow_in_coop` is set (the solo rules then apply in a session too).
pub fn gate_peers(peers: u32, allow_in_coop: bool) -> u32 {
    if allow_in_coop { 0 } else { peers }
}

/// What a peer in the session does to a running nuke sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnCoop {
    /// No peer (or allowed): nothing.
    Continue,
    /// Before the detonation: cancelled and given back (`NUKE cancel reason=coop_session refund=1`).
    CancelRefund,
    /// After the detonation: the sequence finishes (its kills skip every player-set member, so no peer is touched).
    Finish,
}

pub fn on_coop(gate_peers: u32, detonated: bool) -> OnCoop {
    match (gate_peers, detonated) {
        (0, _) => OnCoop::Continue,
        (_, false) => OnCoop::CancelRefund,
        (_, true) => OnCoop::Finish,
    }
}

/// The role of this game in the session from `CSSessionManager.lobby_state` (vanilla enum: 3 Host, 6 Client). Seamless
/// is [I] to drive the vanilla session manager; anything else (or no read) is `unknown`.
pub fn role_label(lobby_state: Option<u32>) -> &'static str {
    match lobby_state {
        Some(3) => "host",
        Some(6) => "client",
        _ => "unknown",
    }
}

/// The one-time HUD note ("Co-op nuke: only your game's enemies are affected; friends won't see it yet"): shown on the
/// first activation with peers present in a co-op session, again only after the session ended (peers back to 0).
#[derive(Clone, Copy, Debug, Default)]
pub struct CoopNote {
    shown: bool,
}

impl CoopNote {
    /// Called every tick with the current (debounced) peer count.
    pub fn on_peers(&mut self, peers: u32) {
        if peers == 0 {
            self.shown = false;
        }
    }

    /// Called on an activation; true = show the note now.
    pub fn on_activate(&mut self, peers: u32) -> bool {
        if peers == 0 || self.shown {
            return false;
        }
        self.shown = true;
        true
    }
}

pub const COOP_NOTE: &str = "Co-op nuke: only your game's enemies are affected; friends won't see it yet";
/// C3 PASS C (`[nuke] coop_mode = "owned"`): the caller's marker starts the nuke in every peer's game.
pub const COOP_NOTE_OWNED: &str = "Co-op nuke: sent to your friends' games (each game kills its own enemies)";
pub const COOP_BLOCKED: &str = "Nuke disabled in co-op";

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: usize = 0x1000;
    const MAIN_H: u64 = 0xffff_ffff_0000_0001;

    fn slot(addr: usize, handle: u64, chr_type: i32, st: u8) -> PeerSlot {
        PeerSlot { addr, handle, chr_type, load_status: st, update_type: 4 }
    }

    fn local() -> PeerSlot {
        PeerSlot { addr: MAIN, handle: MAIN_H, chr_type: 0, load_status: 2, update_type: 0 }
    }

    #[test]
    fn solo_has_no_peers() {
        let s = scan(&[local(), PeerSlot::default(), PeerSlot::default()], MAIN, MAIN_H, 0).unwrap();
        assert_eq!((s.peers.len(), s.npc_players, s.slots), (0, 0, 3));
        assert!(scan(&[local()], 0, MAIN_H, 0).is_none(), "no local player: no decision");
        assert!(scan(&[], MAIN, MAIN_H, 0).unwrap().peers.is_empty());
    }

    #[test]
    fn classify_rules() {
        assert_eq!(classify(&PeerSlot::default(), MAIN, MAIN_H, 0), SlotKind::Empty);
        assert_eq!(classify(&local(), MAIN, MAIN_H, 0), SlotKind::Local);
        // the local player by handle at another address (a re-created PlayerIns) is still local
        assert_eq!(classify(&slot(0x9000, MAIN_H, 0, 2), MAIN, MAIN_H, 0), SlotKind::Local);
        assert_eq!(classify(&slot(0x2000, 7, 1, 2), MAIN, MAIN_H, 0x2000), SlotKind::SinnerHunter);
        for st in [0u8, 5, 9] {
            assert_eq!(classify(&slot(0x2000, 7, 1, st), MAIN, MAIN_H, 0), SlotKind::NotLoaded, "status {st}");
        }
        for st in 1u8..=4 {
            assert_eq!(classify(&slot(0x2000, 7, 1, st), MAIN, MAIN_H, 0), SlotKind::Peer, "status {st}");
        }
        for t in NPC_CHR_TYPES {
            assert_eq!(classify(&slot(0x2000, 7, t, 2), MAIN, MAIN_H, 0), SlotKind::NpcPlayer, "type {t}");
        }
        // human phantom types and anything unknown (Seamless may use its own) count as peers
        for t in [0, 1, 2, 8, 15, 16, 17, 18, 22, -1, 99] {
            assert_eq!(classify(&slot(0x2000, 7, t, 2), MAIN, MAIN_H, 0), SlotKind::Peer, "type {t}");
        }
    }

    #[test]
    fn scan_counts_and_dedups() {
        let slots = [local(), slot(0x3000, 30, 1, 2), slot(0x2000, 20, 1, 3), slot(0x2000, 20, 1, 3), slot(0x4000, 40, 19, 2), slot(0x5000, 50, 1, 5)];
        let s = scan(&slots, MAIN, MAIN_H, 0).unwrap();
        assert_eq!(s.peers.iter().map(|p| p.handle).collect::<Vec<_>>(), vec![20, 30], "sorted, deduplicated");
        assert_eq!(s.npc_players, 1);
        // a garbage capacity is clamped
        let many = vec![slot(0x2000, 20, 1, 2); 1000];
        assert_eq!(scan(&many, MAIN, MAIN_H, 0).unwrap().slots, MAX_SLOTS as u32);
    }

    fn sc(peers: &[u64]) -> PeerScan {
        PeerScan { peers: peers.iter().map(|h| slot(0x2000 + *h as usize, *h, 1, 2)).collect(), npc_players: 0, slots: 4 }
    }

    #[test]
    fn tracker_join_is_immediate_leave_is_debounced() {
        let mut t = PeerTracker::default();
        assert_eq!(t.update(0.0, Some(&sc(&[]))), None, "solo: no line");
        let c = t.update(1.0, Some(&sc(&[20]))).expect("join");
        assert_eq!((c.peers, c.was, c.joined.clone(), c.left.clone()), (1, 0, vec![20], vec![]));
        assert_eq!(c.line(), "peers=1 was=0 joined=[0x0000000000000014] left=[] handles=[0x0000000000000014:type=1:st=2:upd=4] npc=0");
        assert_eq!(t.update(1.25, Some(&sc(&[20]))), None, "no change: no line");
        // not in world (loading): nothing changes
        assert_eq!(t.update(2.0, None), None);
        assert_eq!(t.peers(), 1);
        // a flicker shorter than the debounce does not drop the peer
        assert_eq!(t.update(3.0, Some(&sc(&[]))), None);
        assert_eq!(t.update(4.0, Some(&sc(&[]))), None);
        assert_eq!(t.update(4.5, Some(&sc(&[20]))), None);
        assert_eq!(t.update(7.0, Some(&sc(&[]))), None, "the missing clock restarted at the reappearance");
        assert_eq!(t.peers(), 1);
        let c = t.update(10.0, Some(&sc(&[]))).expect("leave after 3 s");
        assert_eq!((c.peers, c.was, c.left.clone()), (0, 1, vec![20]));
        assert_eq!(t.peers(), 0);
    }

    #[test]
    fn tracker_second_peer_and_swap() {
        let mut t = PeerTracker::default();
        t.update(0.0, Some(&sc(&[20])));
        let c = t.update(1.0, Some(&sc(&[20, 30]))).unwrap();
        assert_eq!((c.peers, c.was, c.joined.clone()), (2, 1, vec![30]));
        // 20 leaves, 40 joins in the same scan: the join is logged at once, the leave 3 s later
        let c = t.update(2.0, Some(&sc(&[30, 40]))).unwrap();
        assert_eq!((c.peers, c.joined.clone(), c.left.clone()), (3, vec![40], vec![]));
        let c = t.update(5.0, Some(&sc(&[30, 40]))).unwrap();
        assert_eq!((c.peers, c.left.clone()), (2, vec![20]));
        assert_eq!(t.handles(), vec![30, 40]);
    }

    #[test]
    fn gate_and_running_sequence() {
        assert_eq!(gate_peers(2, false), 2);
        assert_eq!(gate_peers(2, true), 0, "allow_in_coop: solo rules");
        assert_eq!(gate_peers(0, false), 0);
        assert_eq!(on_coop(0, false), OnCoop::Continue, "solo unchanged");
        assert_eq!(on_coop(0, true), OnCoop::Continue);
        assert_eq!(on_coop(1, false), OnCoop::CancelRefund, "a countdown is cancelled with refund");
        assert_eq!(on_coop(1, true), OnCoop::Finish, "after the detonation the local sequence finishes");
        assert_eq!(on_coop(gate_peers(1, true), false), OnCoop::Continue, "allow_in_coop: a countdown runs on");
        assert_eq!(on_coop(gate_peers(3, true), true), OnCoop::Continue);
    }

    #[test]
    fn role_and_note() {
        assert_eq!(role_label(Some(3)), "host");
        assert_eq!(role_label(Some(6)), "client");
        for s in [None, Some(0), Some(1), Some(4), Some(7), Some(99)] {
            assert_eq!(role_label(s), "unknown");
        }
        let mut n = CoopNote::default();
        assert!(!n.on_activate(0), "solo: no note");
        n.on_peers(1);
        assert!(n.on_activate(1), "first co-op activation");
        assert!(!n.on_activate(1), "one time per session");
        n.on_peers(2);
        assert!(!n.on_activate(2));
        n.on_peers(0);
        n.on_peers(1);
        assert!(n.on_activate(1), "a new session shows it again");
    }
}
