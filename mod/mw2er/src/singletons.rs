//! Boot gate (docs/M2_DESIGN.md section 7; CS2 note gotchas 1-2).
//!
//! fromsoftware-rs caches its first reflection scan forever, and an early scan lacks singletons
//! that are registered later (`InvalidRva`). So nothing calls `FromStatic::instance*()` until:
//! 1. the game's own top-level window exists (seen on two consecutive polls), and
//! 2. a FRESH scan (`from_singleton::find::*`, which never touches the crate's cache) contains
//!    every required reflection name, taken from `<T as FromStatic>::name()` at runtime.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eldenring::cs::{CSBulletManager, CSCamera, CSHavokMan, CSMenuManImp, CSMouseMan, CSTaskImp, SoloParamRepository, WorldChrMan};
use eldenring::fd4::FD4PadManager;
use fromsoftware_shared::FromStatic;
use pelite::pe64::PeView;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};
use windows::core::BOOL;

use crate::guard::guard;
use crate::log::{log_i, log_w};
use crate::shared::GAME_HWND;

const POLL: Duration = Duration::from_millis(250);
const REPORT_EVERY: Duration = Duration::from_secs(10);

/// Reflection names the gameplay needs (all must be present).
pub fn required_names() -> Vec<String> {
    vec![
        <CSTaskImp as FromStatic>::name().into_owned(),
        <WorldChrMan as FromStatic>::name().into_owned(),
        <CSBulletManager as FromStatic>::name().into_owned(),
        <CSCamera as FromStatic>::name().into_owned(),
        <CSMenuManImp as FromStatic>::name().into_owned(),
        <CSHavokMan as FromStatic>::name().into_owned(),
        <SoloParamRepository as FromStatic>::name().into_owned(),
    ]
}

/// Logged when present, never waited for.
pub fn optional_names() -> Vec<String> {
    vec![<CSMouseMan as FromStatic>::name().into_owned(), <FD4PadManager as FromStatic>::name().into_owned()]
}

struct EnumState {
    pid: u32,
    found: Vec<isize>,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // An unwind must never cross this callback frame.
    let r = std::panic::catch_unwind(|| {
        // SAFETY: lparam is the &mut EnumState passed to EnumWindows below, alive for the call.
        let st = unsafe { &mut *(lparam.0 as *mut EnumState) };
        let mut pid = 0u32;
        // SAFETY: valid hwnd from EnumWindows and a valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // SAFETY: valid hwnd.
        if pid != st.pid || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return;
        }
        let mut cls = [0u16; 128];
        // SAFETY: valid buffer.
        let n = unsafe { GetClassNameW(hwnd, &mut cls) }.max(0) as usize;
        let class = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
        // hudhook creates and destroys a dummy "HUDHOOK" window while applying its hooks.
        if class == "HUDHOOK" || class == "ConsoleWindowClass" {
            return;
        }
        st.found.push(hwnd.0 as isize);
    });
    let _ = r;
    BOOL(1) // continue enumeration
}

fn visible_windows(pid: u32) -> Vec<isize> {
    let mut st = EnumState { pid, found: Vec::new() };
    // SAFETY: the callback only uses `st` during this call.
    let _ = unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut st as *mut EnumState as isize)) };
    st.found
}

/// Blocks until the game's main window exists on two consecutive polls. Returns its HWND.
pub fn wait_for_window() -> isize {
    // SAFETY: no preconditions.
    let pid = unsafe { GetCurrentProcessId() };
    let start = Instant::now();
    let mut last_report = Instant::now();
    let mut prev: Vec<isize> = Vec::new();
    loop {
        let now = visible_windows(pid);
        if let Some(&h) = now.iter().find(|h| prev.contains(h)) {
            GAME_HWND.store(h as usize, Ordering::SeqCst);
            log_i!("BOOTGATE", "window hwnd=0x{:x} after {} ms", h, start.elapsed().as_millis());
            return h;
        }
        prev = now;
        if last_report.elapsed() >= REPORT_EVERY {
            log_i!("BOOTGATE", "wait: no game window yet ({} s)", start.elapsed().as_secs());
            last_report = Instant::now();
        }
        std::thread::sleep(POLL);
    }
}

/// One fresh scan: the set of reflection names currently resolvable.
fn scan_names() -> BTreeSet<String> {
    guard("bootgate.scan", || {
        // SAFETY: `None` = the exe module.
        let Ok(base) = (unsafe { GetModuleHandleW(None) }) else { return BTreeSet::new() };
        // SAFETY: the exe image is mapped for the process lifetime.
        let pe = unsafe { PeView::module(base.0 as *const u8) };
        let partial = from_singleton::find::fd4_singletons(pe);
        let mut names = BTreeSet::new();
        if !partial.all_null() {
            // SAFETY: some FD4Singleton is non-null, so Dantelion2 reflection is initialised
            // (the condition from-singleton itself uses before finishing a scan).
            let map = unsafe { partial.finish() };
            names.extend(map.keys().cloned());
        }
        let derived = from_singleton::find::derived_singletons(pe);
        names.extend(derived.keys().cloned());
        names
    })
    .unwrap_or_default()
}

/// Blocks until a fresh scan contains every required name. No timeout (the user may idle at the
/// title screen); logs the missing names every 10 s.
pub fn wait_for_reflection() {
    let required = required_names();
    let start = Instant::now();
    let mut last_report = Instant::now();
    loop {
        let names = scan_names();
        let missing: Vec<&String> = required.iter().filter(|n| !names.contains(*n)).collect();
        if missing.is_empty() {
            let optional: Vec<String> =
                optional_names().into_iter().map(|n| format!("{n}:{}", if names.contains(&n) { "yes" } else { "no" })).collect();
            log_i!(
                "BOOTGATE",
                "PASS after {} ms names=[{}] optional=[{}] scanned={}",
                start.elapsed().as_millis(),
                required.join(","),
                optional.join(","),
                names.len()
            );
            return;
        }
        if last_report.elapsed() >= REPORT_EVERY {
            log_w!(
                "BOOTGATE",
                "waiting missing=[{}] ({} names found so far)",
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","),
                names.len()
            );
            last_report = Instant::now();
        }
        let wait = if start.elapsed() < Duration::from_secs(60) { POLL } else { Duration::from_secs(1) };
        std::thread::sleep(wait);
    }
}
