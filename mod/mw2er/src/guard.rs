//! `catch_unwind` at every boundary where the game (or Windows, or hudhook) calls our code.
//!
//! A panic unwinding into an `extern "system"`/`extern "C"` frame aborts the whole game, so every
//! entry point runs its body through [`guard`]. The panic hook (log.rs) has already written the
//! message and a backtrace by the time `guard` sees the unwind.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::log::log_e;

/// Runs `f`, catching a panic. Returns `None` (after logging `ERROR <name> panicked`) on panic.
pub fn guard<R>(name: &str, f: impl FnOnce() -> R) -> Option<R> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(payload) => {
            log_e!("ERROR", "{name} panicked: {}", panic_message(payload.as_ref()));
            None
        }
    }
}

/// Best-effort text of a panic payload.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
