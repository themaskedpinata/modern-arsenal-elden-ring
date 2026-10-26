//! M6 bug 2 (docs/M6_DESIGN.md section 10.2): why `CSBulletManager::spawn_bullet` returns 4 ("construction
//! failed"), classified from read-only snapshots of the manager's three pools, and the spawn breaker counted per
//! shot (a pellet shot fails only if every pellet failed).
//!
//! The manager (vendored layout [V]) keeps three `BufferAndAllocLinkedList`s: bullets (128 preallocated + 128 heap),
//! an sfx-related list (64 + 192) and an unknown list (4 + 28). A list is full when its preallocated buffer has no
//! empty spot and its heap allocations reached the limit. The "sfx-related" list is ER's bullet-emitter pool (M7,
//! docs/M7_DESIGN.md 2.1): every spawn_bullet call holds one emitter, and ours never finished before vendored patch
//! 0003 (auto-finish bit). The M6 hypothesis H1 (the dagger's hit effects) is refuted; the cause label is
//! `pool_emitters_full`.

/// One `BufferAndAllocLinkedList` snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListSnap {
    pub has_head: bool,
    /// The preallocated buffer has a free slot (`empty_spot != null`).
    pub empty_spot: bool,
    /// Heap allocations alive.
    pub allocated: u32,
    /// The heap limit (MAX_ALLOCS of the type).
    pub heap_cap: u32,
    /// Never decreases.
    pub counter: u32,
}

impl ListSnap {
    pub fn full(&self) -> bool {
        !self.empty_spot && self.allocated >= self.heap_cap
    }
}

/// The three pools plus our own counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolSnap {
    pub bullets: ListSnap,
    pub sfx: ListSnap,
    pub unk40: ListSnap,
    /// Live bullets found by walking the list (all owners), and ours.
    pub live_all: u32,
    pub live_ours: u32,
    /// Engine bullets we spawned since the world was entered.
    pub spawns_world: u32,
}

pub const BULLET_PREALLOC: u32 = 128;
pub const BULLET_HEAP_CAP: u32 = 128;
pub const SFX_PREALLOC: u32 = 64;
pub const SFX_HEAP_CAP: u32 = 192;
pub const UNK40_PREALLOC: u32 = 4;
pub const UNK40_HEAP_CAP: u32 = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    PoolEmittersFull,
    PoolBulletFull,
    Owner,
    Unknown,
}

impl Cause {
    pub fn label(self) -> &'static str {
        match self {
            Cause::PoolEmittersFull => "pool_emitters_full",
            Cause::PoolBulletFull => "pool_bullet_full",
            Cause::Owner => "owner",
            Cause::Unknown => "unknown",
        }
    }
}

/// The cause rule of section 10.2: emitter list full -> pool_emitters_full; bullet list full -> pool_bullet_full; both with
/// headroom and the inert marker probe failing with the same owner -> owner; else unknown. `None` pool = unreadable.
pub fn classify(pool: Option<&PoolSnap>, marker_probe: Option<bool>) -> Cause {
    let Some(p) = pool else { return Cause::Unknown };
    if p.sfx.full() {
        return Cause::PoolEmittersFull;
    }
    if p.bullets.full() || p.live_all >= BULLET_PREALLOC + BULLET_HEAP_CAP {
        return Cause::PoolBulletFull;
    }
    if marker_probe == Some(false) {
        return Cause::Owner;
    }
    Cause::Unknown
}

/// Free bullet slots (both lists hold at most prealloc + heap) for the pellet gate.
pub fn bullet_headroom(p: &PoolSnap) -> u32 {
    (BULLET_PREALLOC + BULLET_HEAP_CAP).saturating_sub(p.live_all)
}

/// How many of `n` pellets may spawn: each pellet also spawns its impact marker, so 2 slots each; none when the sfx
/// list is full (section 6.4 gate).
pub fn pellets_allowed(p: Option<&PoolSnap>, n: u32) -> u32 {
    match p {
        None => n,
        Some(p) if p.sfx.full() => 0,
        Some(p) => (bullet_headroom(p) / 2).min(n),
    }
}

/// The spawn breaker counted per shot: `fail_shot` when every spawn of the shot failed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Breaker {
    pub consecutive: u32,
    pub open_until: Option<f64>,
    pub opened: u32,
}

impl Breaker {
    pub fn is_open(&self, now: f64) -> bool {
        self.open_until.is_some_and(|t| now < t)
    }

    /// Closes an expired breaker; returns true when it just closed.
    pub fn poll(&mut self, now: f64) -> bool {
        if let Some(t) = self.open_until
            && now >= t
        {
            self.open_until = None;
            self.consecutive = 0;
            return true;
        }
        false
    }

    /// Records one shot (`ok` = at least one of its spawns succeeded). Returns true when the breaker opens now.
    pub fn record(&mut self, now: f64, ok: bool, threshold: u32, cooldown_s: f64) -> bool {
        if ok {
            self.consecutive = 0;
            return false;
        }
        self.consecutive += 1;
        if self.consecutive >= threshold && self.open_until.is_none() {
            self.open_until = Some(now + cooldown_s);
            self.opened += 1;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(sfx_alloc: u32, sfx_spot: bool, b_alloc: u32, b_spot: bool, live: u32) -> PoolSnap {
        PoolSnap {
            bullets: ListSnap { has_head: true, empty_spot: b_spot, allocated: b_alloc, heap_cap: BULLET_HEAP_CAP, counter: 500 },
            sfx: ListSnap { has_head: true, empty_spot: sfx_spot, allocated: sfx_alloc, heap_cap: SFX_HEAP_CAP, counter: 900 },
            unk40: ListSnap { has_head: false, empty_spot: true, allocated: 0, heap_cap: UNK40_HEAP_CAP, counter: 3 },
            live_all: live,
            live_ours: 2,
            spawns_world: 250,
        }
    }

    #[test]
    fn spawn_cause_classify() {
        assert_eq!(classify(Some(&snap(192, false, 0, true, 4)), None), Cause::PoolEmittersFull);
        assert_eq!(classify(Some(&snap(10, true, 128, false, 256)), None), Cause::PoolBulletFull);
        assert_eq!(classify(Some(&snap(10, true, 0, true, 4)), Some(false)), Cause::Owner);
        assert_eq!(classify(Some(&snap(10, true, 0, true, 4)), Some(true)), Cause::Unknown, "the marker spawned: not the owner");
        assert_eq!(classify(Some(&snap(10, true, 0, true, 4)), None), Cause::Unknown);
        assert_eq!(classify(None, Some(false)), Cause::Unknown, "no pool data");
        // the rare-success pattern of run 13: one sfx slot frees, the next spawn succeeds, then full again
        let s = snap(191, false, 0, true, 4);
        assert!(!s.sfx.full());
        assert_eq!(classify(Some(&s), None), Cause::Unknown);
        assert_eq!(Cause::PoolEmittersFull.label(), "pool_emitters_full");
    }

    #[test]
    fn spawn_pellet_gate() {
        assert_eq!(pellets_allowed(Some(&snap(10, true, 0, true, 10)), 8), 8);
        assert_eq!(pellets_allowed(Some(&snap(10, true, 0, true, 250)), 8), 3, "6 free slots -> 3 pellets + markers");
        assert_eq!(pellets_allowed(Some(&snap(192, false, 0, true, 10)), 8), 0, "sfx list full");
        assert_eq!(pellets_allowed(None, 8), 8, "unreadable pool: no gate");
    }

    #[test]
    fn breaker_per_shot() {
        // an 8-pellet shot with 8 failed pellets counts 1 failure; 8 such shots open the breaker
        let mut b = Breaker::default();
        for k in 0..7 {
            assert!(!b.record(k as f64, false, 8, 3.0));
        }
        assert!(b.record(7.0, false, 8, 3.0));
        assert!(b.is_open(8.0) && !b.is_open(10.1));
        assert!(b.poll(10.1));
        // a shot with one good pellet resets
        let mut b = Breaker::default();
        for k in 0..20 {
            assert!(!b.record(k as f64, k % 5 == 0, 8, 3.0));
        }
    }
}
