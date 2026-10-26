//! M7 killstreak counter for the Tactical Nuke (docs/M7_DESIGN.md 3.2). Pure.
//!
//! MW2 rules [V dumps: patch_mp `_damage.gsc`, killstreaktable.csv]: the streak counts kills while the attacker is
//! alive; death resets it (`cur_kill_streak = 0`); the nuke costs 25 and is earned once per life. Ours:
//! - +1 per kill the DLL attributes to an MW2 weapon (gun incl. pellets, launcher explosion, AC-130), one per victim
//!   handle (a second kill event for the same handle within [`DEDUPE_S`] is a duplicate);
//! - not counted: nuke kills, kills while the player is dead, kills while MW2 mode is off;
//! - the player's death (hp edge > 0 -> 0): streak 0, life + 1; an earned nuke is kept until used
//!   (`keep_earned_on_death`, MW2 behaviour [I]) and is not persisted across game restarts;
//! - activation: earned -> false, in_sequence; the streak keeps counting during the sequence but cannot earn; at the
//!   end of the sequence the streak is 0. A cancelled sequence (loading before the detonation) refunds the nuke.

/// A second kill event for the same victim within this window is a duplicate.
pub const DEDUPE_S: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillSrc {
    Gun,
    Launcher,
    Ac130,
    Nuke,
}

impl KillSrc {
    pub fn label(self) -> &'static str {
        match self {
            KillSrc::Gun => "gun",
            KillSrc::Launcher => "launcher",
            KillSrc::Ac130 => "ac130",
            KillSrc::Nuke => "nuke",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ignored {
    NukeKill,
    PlayerDead,
    ModeOff,
    Duplicate,
}

impl Ignored {
    pub fn label(self) -> &'static str {
        match self {
            Ignored::NukeKill => "nuke_kill",
            Ignored::PlayerDead => "player_dead",
            Ignored::ModeOff => "mode_off",
            Ignored::Duplicate => "duplicate",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KsEvent {
    /// `NUKE streak n=<n>/<cost> src=..`
    Counted { n: u32, src: KillSrc },
    /// `NUKE earned streak=<n> life=<id>` (with the earn sound and the hint).
    Earned { streak: u32, life: u32 },
    Ignored(Ignored),
    /// `NUKE streak reset reason=death n=<n>`
    Reset { n: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateBlock {
    NotEarned,
    Inbound,
}

#[derive(Clone, Debug, Default)]
pub struct Killstreak {
    pub streak: u32,
    pub earned: bool,
    pub earned_this_life: bool,
    pub life_id: u32,
    pub in_sequence: bool,
    /// (victim handle, time) of recent kills for the dedupe.
    recent: Vec<(u64, f64)>,
}

impl Killstreak {
    /// One attributed kill.
    pub fn on_kill(&mut self, now: f64, handle: u64, src: KillSrc, player_alive: bool, mode_on: bool, cost: u32) -> Vec<KsEvent> {
        self.recent.retain(|(_, t)| now - *t <= DEDUPE_S);
        if src == KillSrc::Nuke {
            return vec![KsEvent::Ignored(Ignored::NukeKill)];
        }
        if !player_alive {
            return vec![KsEvent::Ignored(Ignored::PlayerDead)];
        }
        if !mode_on {
            return vec![KsEvent::Ignored(Ignored::ModeOff)];
        }
        if handle != 0 && self.recent.iter().any(|(h, _)| *h == handle) {
            return vec![KsEvent::Ignored(Ignored::Duplicate)];
        }
        if handle != 0 {
            self.recent.push((handle, now));
        }
        self.streak += 1;
        let mut out = vec![KsEvent::Counted { n: self.streak, src }];
        if self.streak >= cost.max(1) && !self.earned_this_life && !self.in_sequence && !self.earned {
            self.earned = true;
            self.earned_this_life = true;
            out.push(KsEvent::Earned { streak: self.streak, life: self.life_id });
        }
        out
    }

    /// The player died (the hp edge).
    pub fn on_player_death(&mut self, keep_earned: bool) -> KsEvent {
        let n = self.streak;
        self.streak = 0;
        self.life_id += 1;
        self.earned_this_life = false;
        self.recent.clear();
        if !keep_earned && !self.in_sequence {
            self.earned = false;
        }
        KsEvent::Reset { n }
    }

    /// Key 4: starts the sequence (test mode: without a streak).
    pub fn activate(&mut self, always_available: bool) -> Result<bool, ActivateBlock> {
        if self.in_sequence {
            return Err(ActivateBlock::Inbound);
        }
        if !self.earned && !always_available {
            return Err(ActivateBlock::NotEarned);
        }
        let test = !self.earned;
        self.earned = false;
        self.in_sequence = true;
        Ok(test)
    }

    /// The sequence is over (detonated): the streak starts again (the player's death resets it anyway).
    pub fn sequence_end(&mut self) {
        self.in_sequence = false;
        self.streak = 0;
    }

    /// Cancelled before the detonation (loading): the nuke is given back (`test` = it was a test activation).
    pub fn refund(&mut self, test: bool) {
        self.in_sequence = false;
        if !test {
            self.earned = true;
        }
    }

    /// What the HUD shows: `Nuke: n/cost`, or ready.
    pub fn hud_count(&self, cost: u32) -> (u32, u32) {
        (self.streak.min(cost), cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kills(k: &mut Killstreak, n: u32, t0: f64) -> Vec<KsEvent> {
        let mut out = Vec::new();
        for i in 0..n {
            out.extend(k.on_kill(t0 + i as f64 * 3.0, 1000 + i as u64, KillSrc::Gun, true, true, 25));
        }
        out
    }

    fn earned(ev: &[KsEvent]) -> usize {
        ev.iter().filter(|e| matches!(e, KsEvent::Earned { .. })).count()
    }

    #[test]
    fn killstreak_earned_once_at_25() {
        let mut k = Killstreak::default();
        let ev = kills(&mut k, 24, 0.0);
        assert_eq!(earned(&ev), 0);
        assert!(!k.earned);
        let ev = k.on_kill(100.0, 77, KillSrc::Ac130, true, true, 25);
        assert_eq!(ev, vec![KsEvent::Counted { n: 25, src: KillSrc::Ac130 }, KsEvent::Earned { streak: 25, life: 0 }]);
        assert!(k.earned);
        // 50 kills in one life: earned once
        let ev = kills(&mut k, 25, 200.0);
        assert_eq!(earned(&ev), 0, "once per life");
        assert_eq!(k.streak, 50);
    }

    #[test]
    fn killstreak_death_resets_and_keeps_earned() {
        let mut k = Killstreak::default();
        kills(&mut k, 24, 0.0);
        assert_eq!(k.on_player_death(true), KsEvent::Reset { n: 24 });
        assert_eq!((k.streak, k.life_id), (0, 1));
        // earned survives a death (keep_earned_on_death), a new life can count up again but not earn a second one
        kills(&mut k, 25, 100.0);
        assert!(k.earned);
        k.on_player_death(true);
        assert!(k.earned, "earned nuke kept across death");
        let mut k2 = Killstreak::default();
        kills(&mut k2, 25, 0.0);
        k2.on_player_death(false);
        assert!(!k2.earned, "keep_earned_on_death = false");
    }

    #[test]
    fn killstreak_ignored_kills() {
        let mut k = Killstreak::default();
        assert_eq!(k.on_kill(0.0, 5, KillSrc::Gun, false, true, 25), vec![KsEvent::Ignored(Ignored::PlayerDead)]);
        assert_eq!(k.on_kill(0.0, 5, KillSrc::Nuke, true, true, 25), vec![KsEvent::Ignored(Ignored::NukeKill)]);
        assert_eq!(k.on_kill(0.0, 5, KillSrc::Gun, true, false, 25), vec![KsEvent::Ignored(Ignored::ModeOff)]);
        assert_eq!(k.on_kill(0.0, 5, KillSrc::Gun, true, true, 25), vec![KsEvent::Counted { n: 1, src: KillSrc::Gun }]);
        assert_eq!(k.on_kill(1.0, 5, KillSrc::Launcher, true, true, 25), vec![KsEvent::Ignored(Ignored::Duplicate)], "same victim within 2 s");
        assert_eq!(k.on_kill(3.5, 5, KillSrc::Gun, true, true, 25), vec![KsEvent::Counted { n: 2, src: KillSrc::Gun }], "a respawned handle later counts");
        assert_eq!(k.streak, 2);
    }

    #[test]
    fn killstreak_activation_sequence_refund_and_test_mode() {
        let mut k = Killstreak::default();
        assert_eq!(k.activate(false), Err(ActivateBlock::NotEarned));
        assert_eq!(k.activate(true), Ok(true), "always_available: a test activation");
        assert_eq!(k.activate(true), Err(ActivateBlock::Inbound), "never two at once");
        // kills during the sequence count but cannot earn
        kills(&mut k, 30, 0.0);
        assert!(!k.earned);
        k.sequence_end();
        assert_eq!(k.streak, 0);
        // earned -> activate -> loading cancel: refunded
        let mut k = Killstreak::default();
        kills(&mut k, 25, 0.0);
        assert_eq!(k.activate(false), Ok(false));
        assert!(!k.earned && k.in_sequence);
        k.refund(false);
        assert!(k.earned && !k.in_sequence, "refund");
        // a test activation is not refunded as a real nuke
        let mut k = Killstreak::default();
        k.activate(true).unwrap();
        k.refund(true);
        assert!(!k.earned);
        assert_eq!(k.hud_count(25), (0, 25));
    }
}
