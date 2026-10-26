//! M7 A1 (docs/M7_DESIGN.md 2.1): a model of ER's bullet-emitter pool and a long synthetic bench.
//!
//! What the model encodes [V disassembly of eldenring.exe 2.7.1.0, read from disk]:
//! - every top-level `CSBulletManager::spawn_bullet` call allocates one emitter (`0x1403a7cc0`) from a list of 64
//!   preallocated + 192 heap entries; a null allocation writes error 4 (`0x1403a5bd9`) = our `SPAWN FAIL code=4`;
//! - the emitter keeps "emitting" (+0x329) unless its auto-finish byte (+0x4C4, copied from BulletSpawnData+0x44 bit 0
//!   at `0x14038e66a`) is set; it stays alive while it emits or while it still has live bullets (+0x320 [I]); a dead
//!   emitter is destroyed by the per-frame loop (`0x1403a983f`);
//! - engine-made children (the HitBulletID impact marker) do not use an emitter;
//! - a world reload empties the pool.
//!
//! Calibration [V logs]: without the bit the first code 4 came after 254 (run 6), 256 (run 13) and 253 (run 14) of our
//! calls since world entry: 256 slots minus 0..3 emitters of ER's own that were alive at the time.

/// Preallocated emitters.
pub const PREALLOC: u32 = 64;
/// Heap emitters on top (`BufferAndAllocLinkedList<[u8; 0x9d0], 64, 192>`).
pub const HEAP_CAP: u32 = 192;
/// Simultaneously live emitters.
pub const CAP: u32 = PREALLOC + HEAP_CAP;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Emitter {
    auto_finish: bool,
    /// The last of its bullets ends at this time (row life; explosion spheres included).
    bullets_until: f64,
    ours: bool,
}

/// The pool: emitters alive, counters and the first failing call.
#[derive(Clone, Debug, Default)]
pub struct EmitterPool {
    live: Vec<Emitter>,
    pub calls: u32,
    pub failures: u32,
    pub peak: u32,
    pub first_fail_call: Option<u32>,
}

impl EmitterPool {
    /// A pool that already holds `n` emitters of ER's own (persistent until the reload, e.g. a looping effect).
    pub fn with_foreign(n: u32) -> Self {
        let mut p = EmitterPool::default();
        for _ in 0..n.min(CAP) {
            p.live.push(Emitter { auto_finish: false, bullets_until: f64::INFINITY, ours: false });
        }
        p
    }

    pub fn live(&self) -> u32 {
        self.live.len() as u32
    }

    /// Our emitters that will never finish (auto-finish clear).
    pub fn persistent_ours(&self) -> u32 {
        self.live.iter().filter(|e| e.ours && !e.auto_finish).count() as u32
    }

    /// The per-frame destroy pass: an auto-finishing emitter without live bullets is freed.
    pub fn update(&mut self, now: f64) {
        self.live.retain(|e| !(e.auto_finish && now >= e.bullets_until));
    }

    /// One `spawn_bullet` call at `now` whose bullet lives `life_s`. `Err(4)` when every slot is taken.
    pub fn spawn(&mut self, now: f64, auto_finish: bool, life_s: f64) -> Result<(), i32> {
        self.update(now);
        self.calls += 1;
        if self.live() >= CAP {
            self.failures += 1;
            self.first_fail_call.get_or_insert(self.calls);
            return Err(4);
        }
        self.live.push(Emitter { auto_finish, bullets_until: now + life_s.max(0.0), ours: true });
        self.peak = self.peak.max(self.live());
        Ok(())
    }

    /// A loading screen: every emitter goes.
    pub fn reload(&mut self) {
        self.live.clear();
    }
}

/// The long synthetic session of docs/M7_DESIGN.md 2.1.4 oracle 3. Returns the pool after the session.
///
/// 20 000 gun shots at 1000 rpm (bullet row life 0.15 s), every 40th shot followed by an 8-pellet shotgun magazine
/// (8 shots, 8 pellets each, 0.8 s apart), 50 launcher shots (core + ring: 2 calls, life 0.1 / 0.07 s), 3 AC-130 rides
/// (40 s: a 105 every 5 s = core + ring, a 40 mm every 0.6 s = core + ring, 25 mm bursts of 10 at 0.1 s), 1 nuke
/// (3 horizon explosions + `nuke_bullets` lethal bullets, 8 per tick at 60 Hz), all in one world without a reload.
pub fn bench(auto_finish: bool, foreign: u32, nuke_bullets: u32) -> EmitterPool {
    let mut p = EmitterPool::with_foreign(foreign);
    let mut t = 0.0f64;
    let gun_dt = 0.06;
    let mut launchers = 0;
    let mut rides = 0;
    for shot in 0..20_000u32 {
        t += gun_dt;
        let _ = p.spawn(t, auto_finish, 0.15);
        if shot % 40 == 39 {
            for _ in 0..8 {
                t += 0.8;
                for _ in 0..8 {
                    let _ = p.spawn(t, auto_finish, 0.15);
                }
            }
        }
        if shot % 400 == 399 && launchers < 50 {
            launchers += 1;
            t += 1.5;
            let _ = p.spawn(t, auto_finish, 0.1);
            let _ = p.spawn(t, auto_finish, 0.07);
        }
        if shot % 6000 == 5999 && rides < 3 {
            rides += 1;
            let t0 = t;
            let mut k = 0u32;
            while t < t0 + 40.0 {
                t += 0.1;
                k += 1;
                if k % 50 == 0 {
                    let _ = p.spawn(t, auto_finish, 0.1);
                    let _ = p.spawn(t, auto_finish, 0.07);
                }
                if k % 6 == 0 {
                    let _ = p.spawn(t, auto_finish, 0.1);
                    let _ = p.spawn(t, auto_finish, 0.07);
                }
                if k % 20 < 10 {
                    let _ = p.spawn(t, auto_finish, 0.07);
                }
            }
        }
    }
    // the nuke at the end of the session
    t += 10.0;
    for _ in 0..3 {
        t += 0.15;
        let _ = p.spawn(t, auto_finish, 0.1);
    }
    let mut left = nuke_bullets;
    while left > 0 {
        t += 1.0 / 60.0;
        for _ in 0..left.min(8) {
            let _ = p.spawn(t, auto_finish, 0.1);
        }
        left -= left.min(8);
    }
    p.update(t + 1.0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitter_bench_with_autofinish_stays_bounded() {
        let p = bench(true, 3, 64);
        assert!(p.calls > 21_000, "the bench made {} calls", p.calls);
        assert_eq!(p.failures, 0, "no code 4 with the auto-finish bit");
        assert!(p.peak < 64, "peak {} live emitters (want < 64)", p.peak);
        assert_eq!(p.persistent_ours(), 0);
        assert_eq!(p.live(), 3, "only ER's own 3 emitters are left");
    }

    #[test]
    fn emitter_bench_without_flag_fails_at_call_257() {
        let p = bench(false, 0, 0);
        assert_eq!(p.first_fail_call, Some(CAP + 1), "the 257th call fails");
        assert!(p.failures > 20_000, "and every call after it");
        // the run-6/13/14 calibration: 254 / 256 / 253 counted calls, i.e. 0..4 of ER's own emitters alive
        for (foreign, want) in [(0, 257), (1, 256), (3, 254), (4, 253)] {
            assert_eq!(bench(false, foreign, 0).first_fail_call, Some(want), "{foreign} foreign emitters");
        }
        for run in [254u32, 256, 253] {
            assert!((253..=257).contains(&(run + 1)), "run value {run} inside the +-4 tolerance");
        }
    }

    #[test]
    fn emitter_pool_reload_frees_everything() {
        let mut p = EmitterPool::default();
        for k in 0..CAP {
            assert!(p.spawn(k as f64, false, 0.1).is_ok());
        }
        assert_eq!(p.spawn(300.0, true, 0.1), Err(4));
        assert_eq!(p.persistent_ours(), CAP);
        p.reload();
        assert!(p.spawn(301.0, true, 0.1).is_ok());
        // an auto-finishing emitter lives as long as its bullet
        p.update(301.05);
        assert_eq!(p.live(), 1);
        p.update(301.1);
        assert_eq!(p.live(), 0);
    }
}
