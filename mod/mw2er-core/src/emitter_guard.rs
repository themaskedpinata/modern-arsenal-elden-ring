//! M7 A1 watchdog (docs/M7_DESIGN.md 2.1.3): never fail a spawn silently, keep slots for the AC-130 and the nuke.
//!
//! The DLL reads the live emitter count (a cheap upper bound from the list counters per shot; once that bound reaches
//! `warn`, a bounded count-only walk of the list corrects it: [`needs_walk`] / [`effective_live`]) and asks
//! [`EmitterGuard::decide_est`] before each spawn. The upper bound alone assumes `allocated_count` drops when a heap
//! emitter is freed, which was never observed (run 14 only grew); deciding on it alone could refuse every spawn
//! forever after one busy burst, so a refusal at the reserve / cap is always backed by a walked count:
//! - `live >= warn` (192): one `SPAWN WARN emitters ...` per crossing (re-armed below `warn - 16`);
//! - `live >= throttle` (224): low-priority spawns (shotgun pellets after the first, the launcher ring tier) are
//!   dropped; each drop is counted and merged into one `SPAWN throttle ...` line per second;
//! - normal spawns (gun bullets, launcher cores, the marker probe) leave `reserve` (32) slots free: refused at
//!   `cap - reserve`;
//! - priority spawns (AC-130 rows, nuke) are refused only at `cap` (256), and then log `SPAWN FAIL code=4
//!   cause=pool_emitters_full` like a real failure;
//! - `persistent > 0` (one of our emitters that never finishes) after the auto-finish fix = regression:
//!   `SPAWN WARN persistent=<k>` once per world.

pub use crate::emitter_pool::CAP;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuardCfg {
    pub warn: u32,
    pub throttle: u32,
    pub reserve: u32,
}

impl Default for GuardCfg {
    fn default() -> Self {
        GuardCfg { warn: 192, throttle: 224, reserve: 32 }
    }
}

impl GuardCfg {
    /// The config rules (validated by `Config::validate`): warn <= throttle <= cap - 1, reserve 0..=128,
    /// cap - reserve >= throttle is not required (a smaller reserve lets normal spawns go on longer).
    pub fn validate(&self) -> Result<(), String> {
        if !(16..CAP).contains(&self.warn) || !(16..CAP).contains(&self.throttle) || self.warn > self.throttle {
            return Err(format!("spawn.emitter_warn {} / emitter_throttle {} must be 16..255 with warn <= throttle", self.warn, self.throttle));
        }
        if self.reserve > 128 {
            return Err(format!("spawn.emitter_reserve {} must be 0..128", self.reserve));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// C3 PASS C: the co-op visual twins (no damage): dropped first, from `warn` on.
    Lowest,
    /// Extra shotgun pellets (after the first), the launcher ring tier.
    Low,
    /// Gun bullets, the first pellet, launcher cores and impacts, the marker probe.
    Normal,
    /// AC-130 explosions, the nuke (horizon, lethal bullets).
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pellet,
    Ring,
    Gun,
    Launcher,
    Ac130,
    Nuke,
    /// C3 PASS C: a co-op visual twin / marker bullet (atkId -1, synced).
    Twin,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Pellet => "pellet",
            Kind::Ring => "ring",
            Kind::Gun => "gun",
            Kind::Launcher => "launcher",
            Kind::Ac130 => "ac130",
            Kind::Nuke => "nuke",
            Kind::Twin => "twin",
        }
    }

    pub fn priority(self) -> Priority {
        match self {
            Kind::Twin => Priority::Lowest,
            Kind::Pellet | Kind::Ring => Priority::Low,
            Kind::Gun | Kind::Launcher => Priority::Normal,
            Kind::Ac130 | Kind::Nuke => Priority::High,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Dropped by the throttle (logged merged per second).
    Throttled,
    /// Refused: the slots left are reserved for priority spawns (logged merged, `why=reserve`).
    Reserved,
    /// The pool is full: logged like an engine failure (`code=4 cause=pool_emitters_full`).
    Full,
}

/// A log event the DLL writes as one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardLog {
    /// `SPAWN WARN emitters live= upper= walked= ...` (crossing `warn`; both estimates logged).
    Warn { live: u32, upper: u32, walked: Option<u32> },
    /// `SPAWN throttle why=emitters live= dropped= kind=` (one per second, merged).
    Throttle { live: u32, dropped: u32, kinds: String },
    /// `SPAWN WARN persistent=<k>` (once per world).
    Persistent { persistent: u32 },
}

#[derive(Clone, Debug, Default)]
pub struct EmitterGuard {
    warned: bool,
    persistent_warned: bool,
    /// Drops since the last merged line: (count, kinds seen in first-seen order).
    pending: u32,
    pending_kinds: Vec<&'static str>,
    pending_live: u32,
    last_line_t: f64,
    pub dropped_total: u32,
    pub refused_full: u32,
}

impl EmitterGuard {
    /// A new world: the persistent warning is re-armed.
    pub fn world_entered(&mut self) {
        self.persistent_warned = false;
        self.warned = false;
    }

    /// The verdict for one spawn of `kind` with `live` emitters (None = unreadable: allowed, the engine decides).
    pub fn decide(&mut self, now: f64, live: Option<u32>, kind: Kind, cfg: &GuardCfg, out: &mut Vec<GuardLog>) -> Verdict {
        self.decide_est(now, live, None, kind, cfg, out)
    }

    /// As [`decide`](Self::decide) with the counter bound `upper` and, when it was walked, the walked count: the
    /// decision uses [`effective_live`] (the smaller one); the WARN line carries both.
    pub fn decide_est(&mut self, now: f64, upper: Option<u32>, walked: Option<u32>, kind: Kind, cfg: &GuardCfg, out: &mut Vec<GuardLog>) -> Verdict {
        let Some(live) = effective_live(upper, walked) else { return Verdict::Allow };
        if live >= cfg.warn && !self.warned {
            self.warned = true;
            out.push(GuardLog::Warn { live, upper: upper.unwrap_or(live), walked });
        } else if live + 16 < cfg.warn {
            self.warned = false;
        }
        let v = if live >= CAP {
            Verdict::Full
        } else {
            match kind.priority() {
                Priority::High => Verdict::Allow,
                Priority::Normal if live >= CAP.saturating_sub(cfg.reserve) => Verdict::Reserved,
                Priority::Normal => Verdict::Allow,
                Priority::Lowest if live >= cfg.warn || live >= CAP.saturating_sub(cfg.reserve) => Verdict::Throttled,
                Priority::Lowest => Verdict::Allow,
                Priority::Low if live >= cfg.throttle || live >= CAP.saturating_sub(cfg.reserve) => Verdict::Throttled,
                Priority::Low => Verdict::Allow,
            }
        };
        match v {
            Verdict::Throttled | Verdict::Reserved => {
                self.dropped_total += 1;
                self.pending += 1;
                self.pending_live = live;
                if !self.pending_kinds.contains(&kind.label()) {
                    self.pending_kinds.push(kind.label());
                }
            }
            Verdict::Full => self.refused_full += 1,
            Verdict::Allow => {}
        }
        self.flush(now, false, out);
        v
    }

    /// Writes the merged throttle line once per second (or now with `force`).
    pub fn flush(&mut self, now: f64, force: bool, out: &mut Vec<GuardLog>) {
        if self.pending > 0 && (force || now - self.last_line_t >= 1.0) {
            out.push(GuardLog::Throttle { live: self.pending_live, dropped: self.pending, kinds: self.pending_kinds.join("+") });
            self.pending = 0;
            self.pending_kinds.clear();
            self.last_line_t = now;
        }
    }

    /// The walk's persistent count (our emitters with auto-finish clear and still emitting).
    pub fn on_walk(&mut self, persistent: u32, out: &mut Vec<GuardLog>) {
        if persistent > 0 && !self.persistent_warned {
            self.persistent_warned = true;
            out.push(GuardLog::Persistent { persistent });
        }
    }
}

/// The counter bound is high enough that a refusal could follow: walk the list once (bounded) to correct it.
pub fn needs_walk(upper: Option<u32>, cfg: &GuardCfg) -> bool {
    upper.is_some_and(|u| u >= cfg.warn.min(cfg.throttle).min(CAP.saturating_sub(cfg.reserve)))
}

/// The count the guard decides on: the smaller of the counter bound and the walked count (a complete walk); the
/// walk alone when the counters are unreadable.
pub fn effective_live(upper: Option<u32>, walked: Option<u32>) -> Option<u32> {
    match (upper, walked) {
        (Some(u), Some(w)) => Some(u.min(w)),
        (u, w) => u.or(w),
    }
}

/// The cheap upper bound of live emitters from the list counters (no walk): `64 + heap allocations` while the
/// preallocated buffer is full, `63 + heap allocations` when it has a free spot.
pub fn live_upper(prealloc_has_free_spot: bool, heap_allocated: u32) -> u32 {
    let pre = if prealloc_has_free_spot { crate::emitter_pool::PREALLOC - 1 } else { crate::emitter_pool::PREALLOC };
    (pre + heap_allocated).min(CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitter_guard_transitions() {
        let cfg = GuardCfg::default();
        cfg.validate().unwrap();
        let mut g = EmitterGuard::default();
        let mut out = Vec::new();
        // below warn: everything allowed, nothing logged
        for k in [Kind::Pellet, Kind::Ring, Kind::Gun, Kind::Launcher, Kind::Ac130, Kind::Nuke] {
            assert_eq!(g.decide(0.0, Some(100), k, &cfg, &mut out), Verdict::Allow);
        }
        assert!(out.is_empty());
        // warn once per crossing
        assert_eq!(g.decide(0.1, Some(192), Kind::Gun, &cfg, &mut out), Verdict::Allow);
        assert_eq!(g.decide(0.2, Some(200), Kind::Gun, &cfg, &mut out), Verdict::Allow);
        assert_eq!(out, vec![GuardLog::Warn { live: 192, upper: 192, walked: None }]);
        out.clear();
        g.decide(0.3, Some(170), Kind::Gun, &cfg, &mut out);
        g.decide(0.4, Some(195), Kind::Gun, &cfg, &mut out);
        assert_eq!(out, vec![GuardLog::Warn { live: 195, upper: 195, walked: None }], "re-armed below warn - 16");
        out.clear();
        // throttle: low priority dropped at 224.., normal refused at cap - reserve (224), priority allowed to 255
        assert_eq!(g.decide(1.5, Some(224), Kind::Pellet, &cfg, &mut out), Verdict::Throttled);
        assert_eq!(g.decide(1.6, Some(230), Kind::Ring, &cfg, &mut out), Verdict::Throttled);
        assert_eq!(g.decide(1.7, Some(230), Kind::Gun, &cfg, &mut out), Verdict::Reserved);
        for live in 224..256 {
            assert_eq!(g.decide(1.8, Some(live), Kind::Ac130, &cfg, &mut out), Verdict::Allow, "priority at {live}");
            assert_eq!(g.decide(1.8, Some(live), Kind::Nuke, &cfg, &mut out), Verdict::Allow);
        }
        assert_eq!(g.decide(1.9, Some(256), Kind::Nuke, &cfg, &mut out), Verdict::Full);
        assert_eq!(g.decide(1.9, Some(256), Kind::Gun, &cfg, &mut out), Verdict::Full);
        assert_eq!(g.refused_full, 2);
        // every refusal yields log output: the 3 drops merge into one line (first at 1.5 s: last line at 0.0)
        let throttles: Vec<_> = out.iter().filter(|l| matches!(l, GuardLog::Throttle { .. })).collect();
        assert_eq!(throttles.len(), 1);
        assert_eq!(throttles[0], &GuardLog::Throttle { live: 224, dropped: 1, kinds: "pellet".into() });
        out.clear();
        g.flush(2.0, true, &mut out);
        assert_eq!(out, vec![GuardLog::Throttle { live: 230, dropped: 2, kinds: "ring+gun".into() }]);
        assert_eq!(g.dropped_total, 3);
        // unreadable: allowed (the engine decides; a failure is then logged with its cause)
        assert_eq!(g.decide(3.0, None, Kind::Pellet, &cfg, &mut out), Verdict::Allow);
    }

    #[test]
    fn emitter_guard_walk_corrects_a_stale_counter_bound() {
        // ER keeps freed heap nodes counted (allocated_count never drops): the counter bound says 256 forever after a
        // burst. The walk shows 20 live: guns, launchers, AC-130 and nuke spawns go on; the WARN line has both numbers.
        let cfg = GuardCfg::default();
        let stale = live_upper(false, 192);
        assert_eq!(stale, 256);
        assert!(needs_walk(Some(stale), &cfg) && needs_walk(Some(192), &cfg) && !needs_walk(Some(191), &cfg) && !needs_walk(None, &cfg));
        let mut g = EmitterGuard::default();
        let mut out = Vec::new();
        for k in [Kind::Gun, Kind::Launcher, Kind::Pellet, Kind::Ac130, Kind::Nuke] {
            assert_eq!(g.decide_est(0.0, Some(stale), Some(20), k, &cfg, &mut out), Verdict::Allow, "{k:?}");
        }
        assert!(out.is_empty(), "20 live: no warning {out:?}");
        assert_eq!(g.refused_full, 0);
        // the bound alone would have refused everything
        assert_eq!(EmitterGuard::default().decide(0.0, Some(stale), Kind::Ac130, &cfg, &mut Vec::new()), Verdict::Full);
        // a real fill: both agree, refused with both numbers on the WARN line
        let mut out = Vec::new();
        assert_eq!(g.decide_est(1.0, Some(256), Some(256), Kind::Ac130, &cfg, &mut out), Verdict::Full);
        assert_eq!(out[0], GuardLog::Warn { live: 256, upper: 256, walked: Some(256) });
        // a truncated / failed walk: the bound decides (conservative)
        assert_eq!(effective_live(Some(230), None), Some(230));
        assert_eq!(effective_live(None, Some(12)), Some(12));
        assert_eq!(effective_live(Some(64), Some(70)), Some(64), "never above the bound");
        assert_eq!(effective_live(None, None), None);
    }

    #[test]
    fn c3_twin_watchdog_priority() {
        // twins are dropped first: from warn (192), while pellets go on to the throttle (224)
        let cfg = GuardCfg::default();
        let mut g = EmitterGuard::default();
        let mut out = Vec::new();
        assert_eq!(g.decide(0.0, Some(191), Kind::Twin, &cfg, &mut out), Verdict::Allow);
        assert_eq!(g.decide(0.0, Some(192), Kind::Twin, &cfg, &mut out), Verdict::Throttled);
        assert_eq!(g.decide(0.0, Some(192), Kind::Pellet, &cfg, &mut out), Verdict::Allow);
        assert_eq!(g.decide(0.0, Some(200), Kind::Gun, &cfg, &mut out), Verdict::Allow);
        assert!(Kind::Twin.priority() < Kind::Pellet.priority());
        assert_eq!(Kind::Twin.label(), "twin");
    }

    #[test]
    fn emitter_guard_persistent_warns_once_per_world() {
        let mut g = EmitterGuard::default();
        let mut out = Vec::new();
        g.on_walk(0, &mut out);
        assert!(out.is_empty());
        g.on_walk(3, &mut out);
        g.on_walk(5, &mut out);
        assert_eq!(out, vec![GuardLog::Persistent { persistent: 3 }]);
        g.world_entered();
        g.on_walk(1, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn emitter_guard_live_upper_and_config() {
        assert_eq!(live_upper(true, 0), 63);
        assert_eq!(live_upper(false, 0), 64);
        assert_eq!(live_upper(false, 192), 256);
        assert_eq!(live_upper(false, 500), 256);
        assert!(GuardCfg { warn: 230, throttle: 224, reserve: 32 }.validate().is_err());
        assert!(GuardCfg { warn: 192, throttle: 256, reserve: 32 }.validate().is_err());
        assert!(GuardCfg { warn: 192, throttle: 224, reserve: 200 }.validate().is_err());
    }
}
