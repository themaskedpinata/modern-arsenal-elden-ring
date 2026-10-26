//! Player protection while in the gunship (docs/M4_DESIGN.md section 9). Pure model of what the DLL does with the
//! engine's own SpEffect list.
//!
//! The DLL applies SpEffectParam 79000950 (100% damage cut, no stagger, noDead; added by build_regulation.py) with
//! the engine call `ChrInsExt::apply_speffect` and removes it with `remove_speffect`. Presence is read from
//! `ChrIns.special_effect.entries()`: 0 before apply (else the stray cleanup runs first), 1 after apply within
//! `verify_ticks` (else the ride ends with `protect_fail`), re-checked every 30 ticks while Active, 0 after remove
//! (else one retry next tick, then `protect_stuck` and a retry every 60 ticks).
//!
//! Review M4 major 2: the row has a FINITE effectEndurance (default 90 s), so a copy the DLL fails to remove (task
//! panic out of world, quit) expires on its own. A ride longer than `endurance - REFRESH_MARGIN_S` re-applies it
//! ([`refresh_due`]); whether ER resets the timer or stacks a second entry on re-apply is [I] (both are safe: the
//! removal retries until the count is 0; if neither, the effect expires, the 30-tick check sees it missing and the
//! ride ends with `protect_fail`).

/// Ticks between presence checks while the ride is Active.
pub const ACTIVE_CHECK_EVERY_TICKS: u64 = 30;
/// After the first retry, a stuck effect is retried this often.
pub const STUCK_RETRY_EVERY_TICKS: u64 = 60;

/// A re-apply happens this long before the effect would expire.
pub const REFRESH_MARGIN_S: f64 = 20.0;

/// The protection applied at `applied_t` must be re-applied now (finite `endurance_s`; `<= 0` = never).
/// Default numbers: endurance 90 s -> refresh at 70 s, so a default 40 s ride never refreshes.
pub fn refresh_due(now: f64, applied_t: f64, endurance_s: f64) -> bool {
    endurance_s > 0.0 && now - applied_t >= (endurance_s - REFRESH_MARGIN_S).max(endurance_s * 0.5)
}

/// How many entries of `ids` carry the protection id.
pub fn count(ids: &[i32], protect_id: i32) -> usize {
    ids.iter().filter(|&&i| i == protect_id).count()
}

/// Calls are allowed only after the version gate (the crate's RVA table) and once the PARAMS check found the row.
pub fn calls_allowed(gate_passed: bool, row_present: bool) -> Result<(), &'static str> {
    if !gate_passed {
        return Err("version gate not passed");
    }
    if !row_present {
        return Err("SpEffectParam row missing (vanilla regulation?)");
    }
    Ok(())
}

/// Stray cleanup at the first in-world tick of a session and after each load: remove a copy that is present while
/// no ride is running (a quit during a ride, or an effect that survived in the save [I]).
pub fn needs_cleanup(ids: &[i32], protect_id: i32, ride_running: bool) -> bool {
    !ride_running && count(ids, protect_id) > 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveStep {
    /// Verified absent: done.
    Done,
    /// Still present: call remove again now.
    Retry,
    /// Still present after the retry: log `protect_stuck` once, keep retrying every 60 ticks.
    Stuck { first: bool, retry_now: bool },
}

/// The removal's verify/retry bookkeeping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoveTracker {
    attempts: u32,
    stuck_logged: bool,
    ticks_since_try: u64,
}

impl RemoveTracker {
    /// One verification tick after a remove call (`present` = this tick's scan).
    pub fn step(&mut self, present: bool) -> RemoveStep {
        if !present {
            *self = RemoveTracker::default();
            return RemoveStep::Done;
        }
        self.ticks_since_try += 1;
        if self.attempts == 0 {
            self.attempts = 1;
            self.ticks_since_try = 0;
            return RemoveStep::Retry;
        }
        let first = !self.stuck_logged;
        self.stuck_logged = true;
        let retry_now = self.ticks_since_try >= STUCK_RETRY_EVERY_TICKS;
        if retry_now {
            self.ticks_since_try = 0;
            self.attempts += 1;
        }
        RemoveStep::Stuck { first, retry_now }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// A test double of the engine's SpEffect list: apply appends, remove deletes the first matching entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryList(pub Vec<i32>);

impl EntryList {
    pub fn apply(&mut self, id: i32) {
        self.0.push(id);
    }

    pub fn remove(&mut self, id: i32) {
        if let Some(i) = self.0.iter().position(|&x| x == id) {
            self.0.remove(i);
        }
    }

    pub fn sorted(&self) -> Vec<i32> {
        let mut v = self.0.clone();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: i32 = 79000950;

    #[test]
    fn ac130_protect_round_trip_leaves_entries_identical() {
        let before = EntryList(vec![1540, 3110, 10, 4300]);
        let mut l = before.clone();
        assert_eq!(count(&l.0, ID), 0);
        l.apply(ID);
        assert_eq!(count(&l.0, ID), 1);
        l.remove(ID);
        assert_eq!(count(&l.0, ID), 0);
        assert_eq!(l.sorted(), before.sorted(), "multiset of param ids unchanged");
    }

    #[test]
    fn ac130_protect_stray_cleanup_removes_only_ours() {
        let mut l = EntryList(vec![10, ID, 20, 30]);
        assert!(needs_cleanup(&l.0, ID, false));
        assert!(!needs_cleanup(&l.0, ID, true), "never while a ride runs");
        l.remove(ID);
        assert_eq!(l.0, vec![10, 20, 30]);
        assert!(!needs_cleanup(&l.0, ID, false));
    }

    #[test]
    fn ac130_protect_remove_retry_and_stuck() {
        let mut t = RemoveTracker::default();
        assert_eq!(t.step(false), RemoveStep::Done);
        let mut t = RemoveTracker::default();
        assert_eq!(t.step(true), RemoveStep::Retry);
        assert_eq!(t.step(true), RemoveStep::Stuck { first: true, retry_now: false });
        let mut retries = 0;
        for _ in 0..200 {
            if let RemoveStep::Stuck { first, retry_now } = t.step(true) {
                assert!(!first, "protect_stuck logged once");
                retries += retry_now as u32;
            }
        }
        assert_eq!(retries, 3, "every 60 ticks");
        assert_eq!(t.step(false), RemoveStep::Done);
        assert_eq!(t.attempts(), 0);
    }

    #[test]
    fn ac130_protect_refresh_before_expiry() {
        assert!(!refresh_due(69.9, 0.0, 90.0));
        assert!(refresh_due(70.0, 0.0, 90.0), "20 s before the 90 s endurance");
        assert!(!refresh_due(40.0 + 5.0, 0.0, 90.0), "a default 40 s ride (+ shells) never refreshes");
        assert!(refresh_due(15.0, 0.0, 30.0) && !refresh_due(14.9, 0.0, 30.0), "short endurance: at half");
        assert!(!refresh_due(1e9, 0.0, 0.0), "0 = no refresh (permanent rows are refused by the PARAMS check)");
        // the refreshed effect is always re-applied before it can expire: margin > 0 for every allowed endurance
        for e in [30.0, 60.0, 90.0, 600.0] {
            let mut t = 0.0;
            let mut applied = 0.0;
            while t < 3.0 * e {
                assert!(t - applied < e, "expired at t={t} endurance={e}");
                if refresh_due(t, applied, e) {
                    applied = t;
                }
                t += 1.0 / 60.0;
            }
        }
    }

    #[test]
    fn ac130_protect_apply_refused_before_gate_or_row() {
        assert!(calls_allowed(false, true).is_err());
        assert!(calls_allowed(true, false).is_err());
        assert!(calls_allowed(true, true).is_ok());
    }
}
