//! M7 A1 (docs/M7_DESIGN.md 2.1.3): the emitter watchdog every spawn path asks first (UMP45 / arsenal / launchers in
//! task.rs, the AC-130 explosions in ac130.rs, the nuke in nuke.rs). Main thread only (the CSTask closure).
//!
//! Per call: the live emitter count from the list counters (2 non-faulting reads, `bullets::emitter_upper`); once that
//! bound reaches `warn` it is corrected by a count-only walk (`bullets::emitter_count_walk`, <= 300 reads, cached for
//! WALK_CACHE_S so a shotgun burst walks once), then the pure decision of `mw2er_core::emitter_guard` and its log lines. A refusal is never silent: throttled / reserved drops are
//! merged into one `SPAWN throttle ...` line per second, a full pool logs `SPAWN FAIL code=4 cause=pool_emitters_full`.

use std::sync::Mutex;

use mw2er_core::emitter_guard::{EmitterGuard, GuardCfg, GuardLog, Kind, Verdict, effective_live, needs_walk};

use crate::game::bullets;
use crate::log::{log_e, log_i, log_w};
use crate::shared;

static GUARD: Mutex<Option<EmitterGuard>> = Mutex::new(None);
/// The last count-only walk: (time, walked live or None when truncated).
static WALK: Mutex<Option<(f64, Option<u32>)>> = Mutex::new(None);
/// A walk is reused this long (one per tick at 60 Hz; a pellet burst or an AC-130 explosion row set walks once).
const WALK_CACHE_S: f64 = 0.015;

/// (counter bound, walked count when the bound needed it).
fn estimate(now: f64) -> (Option<u32>, Option<u32>) {
    let upper = bullets::emitter_upper();
    if !needs_walk(upper, &cfg()) {
        return (upper, None);
    }
    let Ok(mut w) = WALK.lock() else { return (upper, None) };
    if let Some((t, v)) = *w
        && (now - t).abs() < WALK_CACHE_S
    {
        return (upper, v);
    }
    let v = bullets::emitter_count_walk();
    *w = Some((now, v));
    (upper, v)
}

/// The live emitter count the guard decides on (the nuke's bullet / horizon budgets use it too).
pub fn live_estimate(now: f64) -> Option<u32> {
    let (u, w) = estimate(now);
    effective_live(u, w)
}

fn cfg() -> GuardCfg {
    let c = shared::config();
    GuardCfg { warn: c.spawn.emitter_warn, throttle: c.spawn.emitter_throttle, reserve: c.spawn.emitter_reserve }
}

fn emit(logs: &[GuardLog]) {
    for l in logs {
        match l {
            GuardLog::Warn { live, upper, walked } => log_w!(
                "SPAWN",
                "WARN emitters live={live} upper={upper} walked={} cap={} (the pool is filling: throttle at {})",
                walked.map_or("?".to_string(), |w| w.to_string()),
                mw2er_core::emitter_guard::CAP,
                cfg().throttle
            ),
            GuardLog::Throttle { live, dropped, kinds } => log_i!("SPAWN", "throttle why=emitters live={live} dropped={dropped} kind={kinds}"),
            GuardLog::Persistent { persistent } => {
                log_e!("SPAWN", "WARN persistent={persistent} (our emitters that never finish: the auto-finish bit is not applied; regression)")
            }
        }
    }
}

/// May one spawn of `kind` go ahead now? `false` = refused (logged).
pub fn admit(now: f64, kind: Kind, bullet_id: i32) -> bool {
    let (upper, walked) = estimate(now);
    let live = effective_live(upper, walked);
    let mut logs = Vec::new();
    let v = {
        let Ok(mut g) = GUARD.lock() else { return true };
        let g = g.get_or_insert_with(EmitterGuard::default);
        g.decide_est(now, upper, walked, kind, &cfg(), &mut logs)
    };
    emit(&logs);
    match v {
        Verdict::Allow => true,
        Verdict::Throttled | Verdict::Reserved => false,
        Verdict::Full => {
            log_e!(
                "SPAWN",
                "FAIL code=4 cause=pool_emitters_full live={} upper={} walked={} kind={} id={bullet_id} (refused before the engine call)",
                live.unwrap_or(0),
                upper.map_or("?".to_string(), |u| u.to_string()),
                walked.map_or("?".to_string(), |w| w.to_string()),
                kind.label()
            );
            false
        }
    }
}

/// The merged throttle line once its second is over (called every tick).
pub fn tick(now: f64) {
    let mut logs = Vec::new();
    if let Ok(mut g) = GUARD.lock()
        && let Some(g) = g.as_mut()
    {
        g.flush(now, false, &mut logs);
    }
    emit(&logs);
}

/// A new world: the persistent warning re-arms.
pub fn world_entered() {
    if let Ok(mut g) = GUARD.lock() {
        g.get_or_insert_with(EmitterGuard::default).world_entered();
    }
}

/// The full walk (with every `SPAWN pool` line): logs `SPAWN emitters ...` and the regression warning.
pub fn log_walk(owner: u64, ids: &std::collections::HashSet<i32>) {
    let Some(w) = bullets::emitter_walk(owner, ids) else {
        log_w!("SPAWN", "emitters unreadable (no CSBulletManager or a broken list)");
        return;
    };
    let mut logs = Vec::new();
    let max = {
        let Ok(mut g) = GUARD.lock() else { return };
        let g = g.get_or_insert_with(EmitterGuard::default);
        g.on_walk(w.persistent, &mut logs);
        MAX_SEEN.fetch_max(w.live, std::sync::atomic::Ordering::Relaxed);
        MAX_SEEN.load(std::sync::atomic::Ordering::Relaxed)
    };
    log_i!("SPAWN", "emitters live={} ours={} persistent={} max={max} cap={}{}", w.live, w.ours, w.persistent, mw2er_core::emitter_guard::CAP, if w.truncated { " truncated=1" } else { "" });
    emit(&logs);
}

/// The session maximum of walked live emitters.
static MAX_SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
