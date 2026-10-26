//! The ONLY modules that read engine structs (docs/M2_DESIGN.md section 2.2).
//!
//! Rules (Sunless note gotchas 3-4):
//! - Every read of game memory goes through [`read`], which copies with `ReadProcessMemory` on our
//!   own process: a stale or garbage pointer that passes [`plausible`] but points at an unmapped or
//!   no-access page makes the read return `None` instead of raising an access violation (which
//!   `catch_unwind` cannot stop). Measured offline on this PC: about 0.19 us per read, so the
//!   ~600 reads of a 64-character tick cost about 0.1 ms.
//! - Singletons exist before they are populated: every pointer is read as an INTEGER and
//!   null-checked before anything is formed from it. No `&T` to a game struct is ever created
//!   from a pointer that was not checked first; `NonNull`/`OwnedPtr` fields are read as `usize`.
//!   The only references formed are the ones handed to engine functions (havok.rs, spawn.rs).
//! - Engine enums are never read as Rust enums (invalid discriminants are UB); bytes and integers
//!   are read and compared instead.
//! - Readers return plain-data samples (`*Sample`), never references, so nothing outlives the tick.
//! - Field offsets come from `core::mem::offset_of!` on the vendored crate's `#[repr(C)]` types, so
//!   they track the crate's verified 2.7.1.0 layouts.
//!
//! Everything here runs on the game's main thread (inside the CSTask closure).

//!
//! M3b adds the only game-memory WRITES of the mod ([`write`]): first person (docs/M3B_DESIGN.md section 5.2).
//! They check the target pages with `VirtualQuery` (committed, non-image, exactly PAGE_READWRITE: heap data only),
//! once per 4 KiB page (verdict cached in `mw2er_core::write_cache`, flushed whenever the player leaves the world),
//! and then copy with `NtWriteVirtualMemory` on our own process (a page freed in between, or a read-only/code page,
//! fails instead of faulting; the protection is never changed). They are refused until the version gate has passed
//! (`shared::WRITES_ALLOWED`, set by boot.rs only after `version_gate::check()`).
//!
//! Run 5 (2026-10-14) root cause of the first-person FPS collapse (~16.8 ticks/s) and of the ~70 ms yaw ticks: the
//! old helper ran `VirtualQuery` + `WriteProcessMemory` on every write, and both query the region, which costs time
//! proportional to the region size (ER's heap: several GB, ~17-35 ms per query; measured offline: 4 GB = 26 ms).
//! See `mw2er_core::write_cache` for the numbers. [`write_stats`] feeds the PERF line so the next run proves it.
//!
//! One exe-image write exists: the `CAM_WALL_RECOVERY` bool in eldenring.exe's `.data` (erfps2's camera handling).
//! It has its own narrow path, [`validate_exe_data_bool`] + [`write_exe_bool`]: exactly one address, validated
//! once at boot (inside the exe image, in its writable non-executable `.data` section, MEM_IMAGE of the exe,
//! PAGE_READWRITE or PAGE_WRITECOPY, current value 0 or 1), and only the values 0 or 1 are ever written.

pub mod ac130_read;
pub mod bullets;
/// C3: the player's filter words, the live layer table, ER's UI message signals (all read-only).
pub mod c3_read;
pub mod camera;
pub mod chars;
/// C6 part 16: the main player's EquipGameData (read-only; the carrier equips through ER's own EquipItem).
pub mod equip;
pub mod fp_read;
pub mod havok;
pub mod menu;
pub mod move_read;
pub mod params;
pub mod player;

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use fromsoftware_shared::FromStatic;
use mw2er_core::write_cache::{PAGE, PageCache};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};
use windows::Win32::System::Threading::GetCurrentProcess;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    /// ntdll's native write into an address space (here: our own). Unlike kernelbase's `WriteProcessMemory` it
    /// neither queries the region (O(region size)) nor changes the page protection: a non-writable or unmapped
    /// target makes it return an error status.
    fn NtWriteVirtualMemory(process: *mut c_void, base: *mut c_void, buffer: *const c_void, size: usize, written: *mut usize) -> i32;
}

/// The pseudo-handle of the current process (what `GetCurrentProcess()` returns), as the native API takes it.
const CURRENT_PROCESS: *mut c_void = -1isize as *mut c_void;

/// Copies a `T` from `addr` without ever faulting; `None` for address 0 or when any byte of the
/// range is not readable (unmapped, no-access or guard page).
///
/// # Safety
/// `T` must be plain-old-data that is valid for every bit pattern (integers, floats, arrays of
/// them, `FieldInsHandle`). The address itself may be anything.
pub(crate) unsafe fn read<T: Copy>(addr: usize) -> Option<T> {
    if addr == 0 {
        return None;
    }
    let mut out = MaybeUninit::<T>::uninit();
    let mut copied = 0usize;
    // SAFETY: ReadProcessMemory on the current process validates the source range in the kernel and
    // fails with ERROR_PARTIAL_COPY / ERROR_NOACCESS instead of raising an exception; the
    // destination is our own `size_of::<T>()`-byte buffer. (Unlike WriteProcessMemory it does not query the region.)
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            addr as *const c_void,
            out.as_mut_ptr() as *mut c_void,
            size_of::<T>(),
            Some(&mut copied),
        )
    };
    if ok.is_ok() && copied == size_of::<T>() {
        // SAFETY: all bytes were written and T is valid for any bit pattern (caller contract).
        Some(unsafe { out.assume_init() })
    } else {
        None
    }
}

/// C6 part 16: copies `out.len()` bytes from `addr` without ever faulting (one `ReadProcessMemory`: the inventory list in
/// one call); false for address 0 or when any byte is not readable.
pub(crate) fn read_bytes(addr: usize, out: &mut [u8]) -> bool {
    if addr == 0 || out.is_empty() {
        return false;
    }
    let mut copied = 0usize;
    // SAFETY: as [`read`]: the kernel validates the source range; the destination is our own slice.
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as *const c_void, out.as_mut_ptr() as *mut c_void, out.len(), Some(&mut copied)) };
    ok.is_ok() && copied == out.len()
}

/// Why the last [`write`] was refused (0 = none). For the `FP off reason=read_fail write_fail ...` detail.
pub(crate) static LAST_WRITE_REFUSAL: AtomicU8 = AtomicU8::new(0);
pub(crate) const REFUSED_GATE: u8 = 1;
pub(crate) const REFUSED_QUERY: u8 = 2;
pub(crate) const REFUSED_PROTECT: u8 = 3;
pub(crate) const REFUSED_IMAGE: u8 = 4;
pub(crate) const REFUSED_SPAN: u8 = 5;
pub(crate) const REFUSED_COPY: u8 = 6;
pub(crate) const REFUSED_EXE_ADDR: u8 = 7;

pub(crate) fn refusal_label(code: u8) -> &'static str {
    match code {
        0 => "none",
        REFUSED_GATE => "version_gate",
        REFUSED_QUERY => "virtualquery_failed",
        REFUSED_PROTECT => "page_not_plain_readwrite",
        REFUSED_IMAGE => "page_in_module_image",
        REFUSED_SPAN => "range_crosses_region",
        REFUSED_COPY => "ntwritevirtualmemory_failed",
        REFUSED_EXE_ADDR => "exe_address_not_validated",
        _ => "?",
    }
}

/// Verified heap pages (see `mw2er_core::write_cache`).
static PAGE_CACHE: PageCache = PageCache::new();
// Cost counters for the PERF line (atomics: the detours write too).
static WRITES: AtomicU64 = AtomicU64::new(0);
static WRITE_NS_MAX: AtomicU64 = AtomicU64::new(0);
static SLOW_WRITES: AtomicU64 = AtomicU64::new(0);
static PAGE_CHECKS: AtomicU64 = AtomicU64::new(0);
static PAGE_CHECK_US_MAX: AtomicU64 = AtomicU64::new(0);
static PAGE_CHECK_US_TOTAL: AtomicU64 = AtomicU64::new(0);

/// A single copy slower than this is a regression of the run-5 bug (counted as `slow_writes` in PERF).
pub(crate) const SLOW_WRITE_US: u64 = 1_000;

/// Write cost counters. Totals are cumulative; the maxima reset on every [`write_stats`] call (every PERF line).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WriteStats {
    pub writes: u64,
    pub write_us_max: f64,
    pub slow_writes: u64,
    pub page_checks: u64,
    pub page_check_ms_max: f64,
    pub page_check_ms_total: f64,
    pub cached_pages: usize,
}

pub(crate) fn write_stats() -> WriteStats {
    WriteStats {
        writes: WRITES.load(Ordering::Relaxed),
        write_us_max: WRITE_NS_MAX.swap(0, Ordering::Relaxed) as f64 / 1000.0,
        slow_writes: SLOW_WRITES.load(Ordering::Relaxed),
        page_checks: PAGE_CHECKS.load(Ordering::Relaxed),
        page_check_ms_max: PAGE_CHECK_US_MAX.swap(0, Ordering::Relaxed) as f64 / 1000.0,
        page_check_ms_total: PAGE_CHECK_US_TOTAL.load(Ordering::Relaxed) as f64 / 1000.0,
        cached_pages: PAGE_CACHE.len(),
    }
}

/// Forgets every page verdict. The task calls it when the player leaves the world (loading frees and reuses heap).
pub(crate) fn flush_write_cache() {
    PAGE_CACHE.flush();
}

/// True when every byte of `addr..addr+len` lies in one committed, non-image region whose protection is exactly
/// PAGE_READWRITE (no guard, no execute, not copy-on-write), i.e. heap data. Every heap field first person writes
/// is heap data (ChrIns, CSChrPhysicsModule, CSCam, ChrCam, ChrExFollowCam). SLOW on big regions (the kernel walks
/// the region to compute RegionSize): [`write`] runs it once per page and caches the verdict.
fn heap_rw(addr: usize, len: usize) -> Result<(), u8> {
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: VirtualQuery only inspects the address space; the output is our own struct.
    let n = unsafe { VirtualQuery(Some(addr as *const c_void), &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
    if n == 0 || mbi.State != MEM_COMMIT {
        return Err(REFUSED_QUERY);
    }
    if mbi.Type == MEM_IMAGE {
        return Err(REFUSED_IMAGE);
    }
    if mbi.Protect != PAGE_READWRITE {
        return Err(REFUSED_PROTECT);
    }
    let end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
    if addr.checked_add(len).is_none_or(|e| e > end) {
        return Err(REFUSED_SPAN);
    }
    Ok(())
}

/// `NtWriteVirtualMemory` on our own process: `len` bytes from `src` to `addr`. Never faults, never changes a
/// page's protection, no region query. Timed for the PERF line.
///
/// # Safety
/// `src` must point to `len` readable bytes of our own.
unsafe fn nt_copy(addr: usize, src: *const c_void, len: usize) -> bool {
    let t = Instant::now();
    let mut written = 0usize;
    // SAFETY: the kernel validates the destination range in our address space and fails (status < 0) instead of
    // raising an exception; the source is the caller's buffer.
    let status = unsafe { NtWriteVirtualMemory(CURRENT_PROCESS, addr as *mut c_void, src, len, &mut written) };
    let ns = t.elapsed().as_nanos() as u64;
    WRITES.fetch_add(1, Ordering::Relaxed);
    WRITE_NS_MAX.fetch_max(ns, Ordering::Relaxed);
    if ns / 1000 > SLOW_WRITE_US {
        SLOW_WRITES.fetch_add(1, Ordering::Relaxed);
    }
    status >= 0 && written == len
}

/// Writes a `T` to `addr` without ever faulting. `false` when writes are not allowed yet (version gate), for
/// address 0, when a page of the range is not committed plain read-write heap memory (checked with `VirtualQuery`
/// once per page, then cached), or when the copy fails. No allocation, no lock, no log: usable from a detour (the
/// first write to a new page pays one page check, shown in PERF as `page_checks`). Callers log the field, address
/// and old/new value (first_person.rs).
///
/// `WriteProcessMemory` is deliberately NOT used: kernelbase's version queries the region on every call
/// (O(region size), ~35 ms on ER's heap: the run-5 FPS bug) and would make read-only and code pages temporarily
/// writable. The page check keeps stale pointers out of image memory; `NtWriteVirtualMemory` refuses non-writable
/// pages on its own.
///
/// # Safety
/// `addr` must be the address of a live game field of type `T` that the game tolerates being changed (one of the
/// fields listed in docs/M3B_DESIGN.md section 5.2), written on the game's main thread or from a detour of it.
pub(crate) unsafe fn write<T: Copy>(addr: usize, value: T) -> bool {
    if addr == 0 || !crate::shared::WRITES_ALLOWED.load(Ordering::SeqCst) {
        LAST_WRITE_REFUSAL.store(REFUSED_GATE, Ordering::Relaxed);
        return false;
    }
    for page in PAGE_CACHE.missing(addr, size_of::<T>()).into_iter().flatten() {
        let t = Instant::now();
        let verdict = heap_rw(page, PAGE);
        let us = t.elapsed().as_micros() as u64;
        PAGE_CHECKS.fetch_add(1, Ordering::Relaxed);
        PAGE_CHECK_US_TOTAL.fetch_add(us, Ordering::Relaxed);
        PAGE_CHECK_US_MAX.fetch_max(us, Ordering::Relaxed);
        if let Err(code) = verdict {
            LAST_WRITE_REFUSAL.store(code, Ordering::Relaxed);
            return false;
        }
        PAGE_CACHE.insert(page);
    }
    // SAFETY: the source is our own `size_of::<T>()`-byte value.
    let done = unsafe { nt_copy(addr, &value as *const T as *const c_void, size_of::<T>()) };
    if !done {
        LAST_WRITE_REFUSAL.store(REFUSED_COPY, Ordering::Relaxed);
        // the page changed under us (freed / protection changed): check every page again next time
        PAGE_CACHE.flush();
    }
    done
}

/// The one exe-image address [`write_exe_bool`] accepts (0 = none validated).
static EXE_BOOL_ADDR: AtomicUsize = AtomicUsize::new(0);

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

/// Validates `eldenring.exe + rva` as a writable bool in the exe's `.data` section and makes it the one address
/// [`write_exe_bool`] accepts. Boot thread, after the version gate. Returns the address and its current value.
pub(crate) fn validate_exe_data_bool(rva: usize) -> Result<(usize, u8), String> {
    use pelite::pe64::{Pe, PeView};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: `None` = the exe module, loaded for the process lifetime.
    let base = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?.0 as usize;
    // SAFETY: base is the mapped exe image (the version gate parsed it the same way).
    let view = unsafe { PeView::module(base as *const u8) };
    let size = view.optional_header().SizeOfImage as usize;
    if rva == 0 || rva >= size {
        return Err(format!("rva 0x{rva:x} outside the image (SizeOfImage 0x{size:x})"));
    }
    let sec = view
        .section_headers()
        .as_slice()
        .iter()
        .find(|h| (h.VirtualAddress as usize..h.VirtualAddress as usize + h.VirtualSize as usize).contains(&rva))
        .ok_or_else(|| format!("rva 0x{rva:x} is in no section"))?;
    let name = String::from_utf8_lossy(&sec.Name).trim_end_matches('\0').to_string();
    let ch = sec.Characteristics;
    if name != ".data" || ch & IMAGE_SCN_MEM_WRITE == 0 || ch & IMAGE_SCN_MEM_EXECUTE != 0 {
        return Err(format!("rva 0x{rva:x} is in section {name} (characteristics 0x{ch:08x}), not the writable .data"));
    }
    let addr = base + rva;
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: VirtualQuery only inspects the address space (a small image region: fast).
    let n = unsafe { VirtualQuery(Some(addr as *const c_void), &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
    if n == 0
        || mbi.State != MEM_COMMIT
        || mbi.Type != MEM_IMAGE
        || mbi.AllocationBase as usize != base
        || !(mbi.Protect == PAGE_READWRITE || mbi.Protect == PAGE_WRITECOPY)
    {
        return Err(format!(
            "0x{addr:x}: state=0x{:x} type=0x{:x} allocation_base=0x{:x} protect=0x{:x} (want MEM_COMMIT, MEM_IMAGE of the exe 0x{base:x}, PAGE_READWRITE/WRITECOPY)",
            mbi.State.0, mbi.Type.0, mbi.AllocationBase as usize, mbi.Protect.0
        ));
    }
    // SAFETY: non-faulting read of one byte.
    let v = unsafe { read::<u8>(addr) }.ok_or_else(|| format!("0x{addr:x} unreadable"))?;
    if v > 1 {
        return Err(format!("0x{addr:x} holds 0x{v:02x}, not a bool"));
    }
    EXE_BOOL_ADDR.store(addr, Ordering::SeqCst);
    Ok((addr, v))
}

/// Writes 0 or 1 to the exe bool validated by [`validate_exe_data_bool`], and to nothing else. Version-gated, never
/// faults, no allocation/lock/log (usable from a detour).
///
/// # Safety
/// The validated global must be one the game tolerates being changed (`CAM_WALL_RECOVERY`, as erfps2 does).
pub(crate) unsafe fn write_exe_bool(addr: usize, v: u8) -> bool {
    if !crate::shared::WRITES_ALLOWED.load(Ordering::SeqCst) {
        LAST_WRITE_REFUSAL.store(REFUSED_GATE, Ordering::Relaxed);
        return false;
    }
    if addr == 0 || addr != EXE_BOOL_ADDR.load(Ordering::SeqCst) || v > 1 {
        LAST_WRITE_REFUSAL.store(REFUSED_EXE_ADDR, Ordering::Relaxed);
        return false;
    }
    // SAFETY: one byte of our own.
    let ok = unsafe { nt_copy(addr, &v as *const u8 as *const c_void, 1) };
    if !ok {
        LAST_WRITE_REFUSAL.store(REFUSED_COPY, Ordering::Relaxed);
    }
    ok
}

/// Reads a pointer-sized integer (0 when `addr` is 0 or not readable).
///
/// # Safety
/// As [`read`].
pub(crate) unsafe fn read_ptr(addr: usize) -> usize {
    // SAFETY: forwarded.
    unsafe { read::<usize>(addr) }.unwrap_or(0)
}

/// Address of a singleton instance, 0 when missing or not yet created. Uses the crate's cached
/// reflection map, so it must only be called after the boot gate passed.
pub(crate) fn singleton_addr<T: FromStatic>() -> usize {
    T::instance_ptr().map(|p| p as usize).unwrap_or(0)
}

/// A plausible user-mode heap/image address (rejects small integers and kernel/garbage values).
pub(crate) fn plausible(addr: usize) -> bool {
    (0x10000..0x0000_7FFF_FFFF_0000).contains(&addr) && addr % 8 == 0
}

/// Finite 3-vector from 4 floats.
pub(crate) fn v3(a: [f32; 4]) -> Option<mw2er_core::vec3::V3> {
    let v = mw2er_core::vec3::V3::new(a[0], a[1], a[2]);
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    //! Regression guard of the run-5 FPS bug (needs Win32; `cargo test -p mw2er --lib`): a steady-state write into a
    //! big committed region must take microseconds, not milliseconds, and the safety refusals must still hold.
    use super::*;
    use windows::Win32::System::Memory::{
        MEM_RELEASE, MEM_RESERVE, PAGE_PROTECTION_FLAGS, PAGE_READONLY, VirtualAlloc, VirtualFree, VirtualProtect,
    };

    // The tests share the process-wide gate and cache: run them as one test so they never interleave.
    #[test]
    fn game_write_fast_and_safe() {
        crate::shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);
        steady_state_is_fast_on_a_large_region();
        refusals_hold();
    }

    fn steady_state_is_fast_on_a_large_region() {
        // A 1 GiB committed, touched region: one VirtualQuery or WriteProcessMemory on it costs ~3 ms on this PC
        // (4 GiB: ~26 ms), the shape of ER's heap.
        let size = 1usize << 30;
        // SAFETY: plain reserve+commit of our own memory, released below.
        let base = unsafe { VirtualAlloc(None, size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        assert!(base != 0, "VirtualAlloc 1 GiB");
        for off in (0..size).step_by(PAGE) {
            // SAFETY: committed RW memory of our own.
            unsafe { *((base + off) as *mut u8) = 1 };
        }
        let target = base + 0x40;
        let t = Instant::now();
        // SAFETY: our own committed RW memory.
        assert!(unsafe { write(target, [1.0f32, 2.0, 3.0, 1.0]) });
        let first_ms = t.elapsed().as_secs_f64() * 1e3;
        let n = 2000;
        let t = Instant::now();
        for i in 0..n {
            // SAFETY: as above.
            assert!(unsafe { write(target, [i as f32, 2.0, 3.0, 1.0]) });
        }
        let avg_us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        // SAFETY: plain read of our own memory.
        let back = unsafe { *(target as *const [f32; 4]) };
        assert_eq!(back, [(n - 1) as f32, 2.0, 3.0, 1.0]);
        println!("GAMEWRITE first write (page check) {first_ms:.3} ms; steady state {avg_us:.3} us/write over {n}");
        assert!(avg_us < 50.0, "steady-state write took {avg_us:.1} us (run-5 bug: ~35000 us)");
        assert!(write_stats().page_checks >= 1);
        // SAFETY: our own allocation.
        unsafe { VirtualFree(base as *mut c_void, 0, MEM_RELEASE) }.unwrap();
        flush_write_cache();
    }

    fn refusals_hold() {
        // read-only page: refused by the page check; the native copy alone also fails and does not unprotect it
        // SAFETY: plain allocation of our own memory, released below.
        let p = unsafe { VirtualAlloc(None, PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        let mut old = PAGE_PROTECTION_FLAGS(0);
        // SAFETY: our own page.
        unsafe { VirtualProtect(p as *const c_void, PAGE, PAGE_READONLY, &mut old) }.unwrap();
        // SAFETY: our own page (the call must refuse).
        assert!(!unsafe { write(p + 8, 7u32) });
        assert_eq!(LAST_WRITE_REFUSAL.load(Ordering::Relaxed), REFUSED_PROTECT);
        let v = 7u32;
        // SAFETY: the raw native copy on a read-only page fails without faulting.
        assert!(!unsafe { nt_copy(p + 8, &v as *const u32 as *const c_void, 4) });
        // SAFETY: plain read of our own page.
        assert_eq!(unsafe { *((p + 8) as *const u32) }, 0);
        // SAFETY: our own allocation.
        unsafe { VirtualFree(p as *mut c_void, 0, MEM_RELEASE) }.unwrap();
        // freed page: refused, no fault
        flush_write_cache();
        // SAFETY: freed address (the call must refuse).
        assert!(!unsafe { write(p + 8, 7u32) });
        assert_eq!(LAST_WRITE_REFUSAL.load(Ordering::Relaxed), REFUSED_QUERY);
        // code of our own image: refused (MEM_IMAGE)
        let code = refusals_hold as fn() as usize;
        // SAFETY: the call must refuse.
        assert!(!unsafe { write(code, 0u8) });
        assert_eq!(LAST_WRITE_REFUSAL.load(Ordering::Relaxed), REFUSED_IMAGE);
        // the exe-bool path only accepts the validated address (none in the test process)
        // SAFETY: the call must refuse.
        assert!(!unsafe { write_exe_bool(code, 0) });
        assert_eq!(LAST_WRITE_REFUSAL.load(Ordering::Relaxed), REFUSED_EXE_ADDR);
        // the gate closed: refused
        crate::shared::WRITES_ALLOWED.store(false, Ordering::SeqCst);
        let mut x = 0u32;
        // SAFETY: the call must refuse.
        assert!(!unsafe { write(&mut x as *mut u32 as usize, 1u32) });
        assert_eq!(LAST_WRITE_REFUSAL.load(Ordering::Relaxed), REFUSED_GATE);
        assert_eq!(x, 0);
        crate::shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);
    }
}
