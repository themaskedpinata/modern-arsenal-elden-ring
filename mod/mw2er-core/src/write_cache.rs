//! Page-verdict cache for the DLL's game-memory writes (2026-10-14, after run 5).
//!
//! Why it exists: run 5 measured a steady ~16.8 ticks/s with first person on, and ~70 ms per tick with yaw follow on.
//! The cause is the cost of one `game::write`. It used to call `VirtualQuery` and then `WriteProcessMemory`, and
//! `WriteProcessMemory` runs `NtQueryVirtualMemory` again inside kernelbase. Both queries take time proportional to
//! the size of the region that holds the address, because the kernel walks every page of the run to compute
//! `RegionSize`. ER's heap is one committed region of several GB. Measured on this PC with a touched, committed
//! test region: 1 GB = 3.1 ms per query, 4 GB = 26 ms per query, against 0.3 us for `NtWriteVirtualMemory`. So:
//! - one write per frame (the camera hook) cost ~35 ms per frame: 60 -> ~17 FPS;
//! - the yaw path did two writes per tick (orientation + interpolated orientation): ~70 ms per tick.
//!
//! The fix keeps the page check (committed, not image, exactly PAGE_READWRITE) but runs it once per 4 KiB page and
//! caches the verdict here. The DLL flushes the cache whenever the player leaves the world (loading screens free and
//! reuse heap pages). The copy itself uses `NtWriteVirtualMemory`, which never changes a page's protection (a
//! read-only or code page makes it fail instead of being written) and does no region query.
//!
//! Lock-free (atomics only): the camera-step and follow-cam detours use it and must not lock or allocate.

use std::sync::atomic::{AtomicUsize, Ordering};

pub const PAGE: usize = 0x1000;
/// Number of cached page verdicts. First person writes to about six pages (CSCam, ChrCam, ChrExFollowCam, ChrIns,
/// CSChrPhysicsModule); 32 leaves room for the restore paths and the next character after a load.
pub const SLOTS: usize = 32;

/// The first and last page touched by `len` bytes at `addr` (`None` for len 0 or an overflowing range).
pub fn pages(addr: usize, len: usize) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let last = addr.checked_add(len - 1)?;
    Some((addr & !(PAGE - 1), last & !(PAGE - 1)))
}

/// Verified-page cache. A slot holds a page base (never 0: page 0 is never a valid write target).
pub struct PageCache {
    slots: [AtomicUsize; SLOTS],
    next: AtomicUsize,
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PageCache {
    pub const fn new() -> Self {
        PageCache { slots: [const { AtomicUsize::new(0) }; SLOTS], next: AtomicUsize::new(0) }
    }

    /// True when `page` (a page base) was verified since the last flush.
    pub fn contains(&self, page: usize) -> bool {
        page != 0 && self.slots.iter().any(|s| s.load(Ordering::Acquire) == page)
    }

    /// Records a verified page (round robin; a racing insert at worst evicts a verdict, which only costs one more
    /// check later).
    pub fn insert(&self, page: usize) {
        if page == 0 || page & (PAGE - 1) != 0 || self.contains(page) {
            return;
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % SLOTS;
        self.slots[i].store(page, Ordering::Release);
    }

    /// Forgets every verdict (the player left the world).
    pub fn flush(&self) {
        for s in &self.slots {
            s.store(0, Ordering::Release);
        }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.load(Ordering::Relaxed) != 0).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The pages of `addr..addr+len` that still need a check (0, 1 or 2 pages; our writes are at most 64 bytes).
    pub fn missing(&self, addr: usize, len: usize) -> [Option<usize>; 2] {
        match pages(addr, len) {
            None => [None, None],
            Some((a, b)) => {
                let first = (!self.contains(a)).then_some(a);
                let second = (b != a && !self.contains(b)).then_some(b);
                [first, second]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_cache_pages_span() {
        assert_eq!(pages(0x1234_5ff8, 16), Some((0x1234_5000, 0x1234_6000)));
        assert_eq!(pages(0x1234_5000, 16), Some((0x1234_5000, 0x1234_5000)));
        assert_eq!(pages(0x1234_5fff, 1), Some((0x1234_5000, 0x1234_5000)));
        assert_eq!(pages(0x1234_5000, 0), None);
        assert_eq!(pages(usize::MAX, 2), None);
    }

    #[test]
    fn write_cache_hit_miss_flush() {
        let c = PageCache::new();
        assert_eq!(c.missing(0x7ff1_0000_0ff8, 16), [Some(0x7ff1_0000_0000), Some(0x7ff1_0000_1000)]);
        c.insert(0x7ff1_0000_0000);
        assert_eq!(c.missing(0x7ff1_0000_0ff8, 16), [None, Some(0x7ff1_0000_1000)]);
        c.insert(0x7ff1_0000_1000);
        assert_eq!(c.missing(0x7ff1_0000_0ff8, 16), [None, None]);
        assert_eq!(c.missing(0x7ff1_0000_0040, 16), [None, None]);
        c.insert(0x7ff1_0000_1000); // no duplicate slot
        assert_eq!(c.len(), 2);
        c.flush();
        assert!(c.is_empty());
        assert_eq!(c.missing(0x7ff1_0000_0040, 16), [Some(0x7ff1_0000_0000), None]);
        // never caches page 0 or an unaligned value
        c.insert(0);
        c.insert(0x7ff1_0000_0040);
        assert!(c.is_empty() && !c.contains(0));
    }

    #[test]
    fn write_cache_round_robin_bounded() {
        let c = PageCache::new();
        for i in 1..=(SLOTS + 5) {
            c.insert(i * PAGE);
        }
        assert_eq!(c.len(), SLOTS);
        // the newest pages are present, the oldest were evicted
        assert!(c.contains((SLOTS + 5) * PAGE));
        assert!(!c.contains(PAGE));
    }

    /// The regression guard of the run-5 bug in pure form: with the cache, a steady state of N writes per tick to
    /// the same fields needs zero page checks after the first tick (each check is a multi-ms VirtualQuery on ER's heap).
    #[test]
    fn write_cache_steady_state_needs_no_checks() {
        let c = PageCache::new();
        let fields = [(0x7ff1_3cf5_0fa0usize, 16usize), (0x7ff2_3c21_8750, 32), (0x7ff2_3b81_99c5, 1), (0x7ff1_0000_0040, 16)];
        let mut checks = 0;
        for tick in 0..600 {
            for (a, l) in fields {
                for p in c.missing(a, l).into_iter().flatten() {
                    checks += 1;
                    c.insert(p);
                }
            }
            if tick == 0 {
                assert_eq!(checks, 4);
            }
        }
        assert_eq!(checks, 4, "page checks must not repeat per tick");
    }
}
