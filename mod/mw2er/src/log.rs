//! File logger (`logs\mw2er.log`), panic hook and the `tracing` bridge for hudhook.
//!
//! Line format (mw2er-core::logfmt): `YYYY-MM-DD HH:MM:SS.mmm L [thread] TAG message`, local time
//! from `GetLocalTime`. Every line is flushed so the last line survives a crash. The input detour
//! never logs; the panic hook only uses `try_lock` (a panic while holding the writer must not
//! deadlock).

use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use mw2er_core::logfmt::{self, Stamp};
pub use mw2er_core::logfmt::Level;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::shared::{lock, now_s, try_lock};

/// Rotate the log at boot when it is larger than this (one backup, `mw2er.1.log`).
const MAX_LOG_BYTES: u64 = 20 * 1024 * 1024;

static WRITER: Mutex<Option<BufWriter<File>>> = Mutex::new(None);

thread_local! {
    static ROLE: Cell<&'static str> = const { Cell::new("") };
}

/// Names the current thread in log lines (game threads have no Rust name).
pub fn set_thread_role(role: &'static str) {
    ROLE.with(|r| r.set(role));
}

fn thread_label() -> String {
    let role = ROLE.with(|r| r.get());
    if !role.is_empty() {
        return role.to_string();
    }
    let t = std::thread::current();
    match t.name() {
        Some(n) => n.trim_start_matches("mw2er-").to_string(),
        None => format!("{:?}", t.id()).replace("ThreadId", "t"),
    }
}

fn stamp() -> Stamp {
    // SAFETY: GetLocalTime has no preconditions.
    let st = unsafe { GetLocalTime() };
    Stamp {
        year: st.wYear,
        month: st.wMonth as u8,
        day: st.wDay as u8,
        hour: st.wHour as u8,
        minute: st.wMinute as u8,
        second: st.wSecond as u8,
        millis: st.wMilliseconds,
    }
}

/// One line to the debugger output (DebugView), for the case where no log file can be opened.
pub fn debug_out(msg: &str) {
    let wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated UTF-16 buffer that outlives the call.
    unsafe { OutputDebugStringW(windows::core::PCWSTR(wide.as_ptr())) };
}

/// Opens (appends to) the log file, rotating it first if it is over 20 MB.
pub fn init(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if std::fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let backup = path.with_file_name("mw2er.1.log");
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(path, &backup);
    }
    let f = OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    *lock(&WRITER) = Some(BufWriter::new(f));
    Ok(())
}

fn format_line(level: Level, tag: &str, msg: &str) -> String {
    logfmt::line(&stamp(), level, &thread_label(), tag, msg)
}

/// Writes and flushes one line. Never panics.
pub fn write(level: Level, tag: &str, msg: &str) {
    let line = format_line(level, tag, msg);
    if let Some(w) = lock(&WRITER).as_mut() {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

/// Like [`write`] but gives up instead of waiting for the lock (panic hook).
fn write_nonblocking(level: Level, tag: &str, msg: &str) {
    let line = format_line(level, tag, msg);
    if let Some(mut g) = try_lock(&WRITER)
        && let Some(w) = g.as_mut()
    {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

macro_rules! log_i { ($tag:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Info, $tag, &format!($($arg)*)) } }
macro_rules! log_w { ($tag:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Warn, $tag, &format!($($arg)*)) } }
macro_rules! log_e { ($tag:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Error, $tag, &format!($($arg)*)) } }
pub(crate) use {log_e, log_i, log_w};

/// Logs every panic with location and backtrace before it unwinds into a `guard`.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string payload>".into());
        let bt = std::backtrace::Backtrace::force_capture();
        write_nonblocking(Level::Error, "PANIC", &format!("thread={} at {loc}: {msg}\n{bt}", thread_label()));
    }));
}

// ---- tracing bridge (hudhook logs through `tracing`) ---------------------------------------------

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

#[derive(Default)]
struct MsgVisitor {
    msg: String,
    extra: String,
}

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.msg = format!("{value:?}");
        } else {
            self.extra.push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.msg = value.to_string();
        } else {
            self.extra.push_str(&format!(" {}={value}", field.name()));
        }
    }
}

/// Per-callsite rate limit: the first event is written, repeats are counted and summarised at most
/// every 10 s (hudhook warns once per ExecuteCommandLists while it searches for the queue).
#[derive(Default)]
struct BridgeLayer {
    seen: Mutex<HashMap<tracing::callsite::Identifier, (f64, u64)>>,
}

const REPEAT_WINDOW_S: f64 = 10.0;

impl<S: Subscriber> Layer<S> for BridgeLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let _ = crate::guard::guard("log.tracing", || {
            let meta = event.metadata();
            let now = now_s();
            let repeats = {
                let mut seen = lock(&self.seen);
                let e = seen.entry(meta.callsite()).or_insert((f64::NEG_INFINITY, 0));
                if now - e.0 < REPEAT_WINDOW_S {
                    e.1 += 1;
                    return;
                }
                let r = e.1;
                *e = (now, 0);
                r
            };
            let mut v = MsgVisitor::default();
            event.record(&mut v);
            let level = match *meta.level() {
                tracing::Level::ERROR => Level::Error,
                tracing::Level::WARN => Level::Warn,
                tracing::Level::INFO => Level::Info,
                _ => Level::Debug,
            };
            let rep = if repeats > 0 { format!(" (+{repeats} repeats in the last {REPEAT_WINDOW_S}s)") } else { String::new() };
            write(level, "HUDHOOK", &format!("{}: {}{}{rep}", meta.target(), v.msg, v.extra));
        });
    }
}

/// Routes hudhook's `tracing` events at `level` (never trace) into our log.
pub fn init_tracing(level: &str) {
    use tracing_subscriber::prelude::*;
    let level = match level {
        "error" | "warn" | "info" | "debug" => level,
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::new(format!("hudhook={level}"));
    if tracing_subscriber::registry().with(filter).with(BridgeLayer::default()).try_init().is_err() {
        write(Level::Warn, "WARN", "tracing subscriber already set; hudhook messages not bridged");
    }
}
