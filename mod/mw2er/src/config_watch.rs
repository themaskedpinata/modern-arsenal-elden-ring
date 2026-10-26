//! Config hot reload and the periodic STATE line (boot thread, once a second).
//!
//! Hot: log, keys, gameplay (not the weapon in M2), aim, hit, menu, hud, audio.
//! Not hot: paths, damage.k (baked into the regulation rows), regulation (build-time).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use mw2er_core::config::Config;

use crate::log::{log_e, log_i, log_w};
use crate::shared::{self, LAST_GDS_KBD_MS, LAST_GDS_MOUSE_MS, SNAPSHOT, now_ms_if_started, now_s};

pub struct Watcher {
    path: PathBuf,
    mtime: Option<SystemTime>,
    last_state: f64,
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Watcher {
    pub fn new(path: PathBuf) -> Self {
        let mtime = mtime(&path);
        Watcher { path, mtime, last_state: 0.0 }
    }

    /// Re-reads the file when its modification time changed.
    pub fn poll(&mut self) {
        let m = mtime(&self.path);
        if m.is_none() || m == self.mtime {
            return;
        }
        self.mtime = m;
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => {
                log_e!("CONFIG", "ERROR {}: {e}; keeping previous", self.path.display());
                return;
            }
        };
        let new = match Config::parse(&text) {
            Ok(c) => c,
            Err(e) => {
                log_e!("CONFIG", "ERROR {e}; keeping previous");
                return;
            }
        };
        let old = shared::config();
        let changed = old.changed_sections(&new);
        // The weapon is fixed for the session in M2 (one gun's rows/sounds are loaded at boot).
        let (merged, ignored) = old.merge_hot(&new, false);
        shared::set_config(Arc::new(merged));
        log_i!("CONFIG", "reloaded (changed: {})", if changed.is_empty() { "nothing".into() } else { changed.join(",") });
        for i in ignored {
            log_w!("CONFIG", "WARN {i} changed but is not hot-reloadable; restart (damage.k: rerun build_regulation.py)");
            // C4 (docs/C4_UI_DESIGN.md 3.1): turning the injection on needs a restart (hooks + objects are boot work)
            if i.starts_with("ui.inject_") {
                log_w!("UI", "inject on ignored (cold): {i}; restart the game to turn it on");
            }
        }
    }

    /// `STATE ...` every `log.state_interval_s` while in world.
    pub fn state_line(&mut self) {
        let cfg = shared::config();
        let now = now_s();
        if now - self.last_state < cfg.log.state_interval_s {
            return;
        }
        let s = shared::lock(&SNAPSHOT).clone();
        if !s.in_world || now - s.t > 2.0 {
            return;
        }
        self.last_state = now;
        let age = |a: &std::sync::atomic::AtomicU64| {
            let t = a.load(Ordering::Relaxed);
            if t == 0 { "never".to_string() } else { format!("{}ms", now_ms_if_started().saturating_sub(t)) }
        };
        log_i!(
            "STATE",
            "mode={} mag={}/{} reloading={} spread={:.2} menu={} fg={} chars={} shots={} hits={} gds_mouse={} gds_kbd={}",
            if s.mode_on { "on" } else { "off" },
            s.mag,
            s.reserve,
            s.reloading as u8,
            s.spread_deg,
            s.menu_open as u8,
            s.foreground as u8,
            s.chars,
            s.shots,
            s.hits,
            age(&LAST_GDS_MOUSE_MS),
            age(&LAST_GDS_KBD_MS)
        );
    }
}
