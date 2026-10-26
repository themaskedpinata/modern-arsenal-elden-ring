//! Hit confirmation from HP samples (docs/M3_DESIGN.md section 11, replacing M2 section 12.1).
//!
//! M2 run 2 showed that `ChrIns.last_hit_by` stays empty for our bullets, so attribution by `last_hit_by`
//! alone only caught kills. An HP drop of a tracked character is now **attributed** to us when either
//!   1. `last_hit_by` is the player (`via=last_hit_by`, the old rule, still inside the shot window), or
//!   2. one of our impact markers landed within `[t_drop - impact_window_s, t_drop + impact_lag_s]` and
//!      within `impact_radius_m` of the character's vertical axis segment `pos .. pos + capsule_height_m`,
//!      `pos` being the character position recorded at the drop (`via=impact`).
//!
//! **Order (measured, M2 run 2):** the IMPACT of the bullet that caused a drop is recorded one task tick
//! *after* the drop (+16..18 ms on every close-range hit), because the marker bullet is seen gone only on the
//! next poll. A drop is therefore kept **pending** until `impact_lag_s` has passed, and only then resolved.
//! When an impact is eligible for several pending drops of characters within the radius, it goes to the drop
//! nearest in time (so back-to-back drops 83 ms apart do not steal each other's impact). `n` is the number of
//! impacts a drop consumed. A drop that ends at hp <= 0 is a kill (`kill=1`, excluded from damage
//! calibration). Unattributed drops inside the shot window are reported as `HpDrop` (diagnostic).

use std::collections::HashMap;

/// One character as read from the game this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChrSample {
    pub handle: u64,
    pub character_id: u32,
    pub hp: i32,
    pub max_hp: i32,
    pub last_hit_by: u64,
    /// Physics position (the feet), metres, Y up.
    pub pos: [f32; 3],
}

/// One of our marker-bullet impacts (`IMPACT kind=hit`), kept for about a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactSample {
    pub t: f64,
    pub pos: [f32; 3],
    pub used: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    LastHitBy,
    Impact,
}

impl Via {
    pub fn label(self) -> &'static str {
        match self {
            Via::LastHitBy => "last_hit_by",
            Via::Impact => "impact",
        }
    }
}

/// Events carry `t_drop`, the time the drop was seen (they are emitted up to `impact_lag_s` later), so the
/// caller can look up the shot that was current at the drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitEvent {
    /// C3: `impact_pos` is the latest of our impacts this drop consumed (the damage number's anchor), if any.
    Hit { character_id: u32, handle: u64, hp_before: i32, hp_after: i32, via: Via, n: u32, kill: bool, t_drop: f64, impact_pos: Option<[f32; 3]> },
    Kill { character_id: u32, handle: u64, t_drop: f64 },
    HpDrop { character_id: u32, handle: u64, hp_before: i32, hp_after: i32, last_hit_by: u64, t_drop: f64 },
}

impl HitEvent {
    pub fn damage(&self) -> i32 {
        match *self {
            HitEvent::Hit { hp_before, hp_after, .. } | HitEvent::HpDrop { hp_before, hp_after, .. } => {
                hp_before - hp_after
            }
            HitEvent::Kill { .. } => 0,
        }
    }
}

/// Inputs that change per tick.
#[derive(Clone, Copy, Debug)]
pub struct HitContext<'a> {
    pub player_handle: u64,
    pub now: f64,
    pub last_shot_t: Option<f64>,
    pub window_s: f64,
    pub exclude_character_ids: &'a [u32],
    /// How long before the drop an impact may have been recorded.
    pub impact_window_s: f64,
    /// How long after the drop an impact may still be recorded (the drop waits this long before resolving).
    pub impact_lag_s: f64,
    pub impact_radius_m: f32,
    pub capsule_height_m: f32,
}

/// Distance from `p` to the vertical segment `base .. base + (0, h, 0)`.
pub fn dist_to_axis(p: &[f32; 3], base: &[f32; 3], h: f32) -> f32 {
    let y = p[1].clamp(base[1], base[1] + h.max(0.0));
    let d = [p[0] - base[0], p[1] - y, p[2] - base[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Keeps impacts of the last `keep_s` seconds (task side; cleared on world enter/leave and re-base).
pub fn prune_impacts(impacts: &mut Vec<ImpactSample>, now: f64, keep_s: f64) {
    impacts.retain(|i| now - i.t <= keep_s && i.pos.iter().all(|v| v.is_finite()));
}

/// A drop seen but not yet resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    t: f64,
    handle: u64,
    character_id: u32,
    hp_before: i32,
    hp_after: i32,
    pos: [f32; 3],
    last_hit_by: u64,
    by_player_in_window: bool,
    in_window: bool,
}

#[derive(Clone, Debug, Default)]
pub struct HitTracker {
    prev: HashMap<u64, i32>,
    pending: Vec<Pending>,
}

impl HitTracker {
    /// Forgets HP history and pending drops (world left).
    pub fn clear(&mut self) {
        self.prev.clear();
        self.pending.clear();
    }

    pub fn tracked(&self) -> usize {
        self.prev.len()
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Diffs this tick's samples against the previous tick's, queues new drops, resolves drops older than
    /// `impact_lag_s` (consuming matching impacts) and appends their events in drop order.
    pub fn update(
        &mut self,
        samples: &[ChrSample],
        ctx: &HitContext<'_>,
        impacts: &mut [ImpactSample],
        out: &mut Vec<HitEvent>,
    ) {
        let in_window = ctx.last_shot_t.is_some_and(|t| ctx.now >= t && ctx.now - t <= ctx.window_s);
        let mut next = HashMap::with_capacity(samples.len());
        for s in samples {
            if s.handle == ctx.player_handle || ctx.exclude_character_ids.contains(&s.character_id) {
                continue;
            }
            if let Some(&before) = self.prev.get(&s.handle)
                && s.hp < before
            {
                self.pending.push(Pending {
                    t: ctx.now,
                    handle: s.handle,
                    character_id: s.character_id,
                    hp_before: before,
                    hp_after: s.hp,
                    pos: s.pos,
                    last_hit_by: s.last_hit_by,
                    by_player_in_window: s.last_hit_by == ctx.player_handle && in_window,
                    in_window,
                });
            }
            next.insert(s.handle, s.hp);
        }
        self.prev = next;
        self.resolve(ctx, impacts, out, false);
    }

    /// Resolves every pending drop now, whatever its age (used by tests and on shutdown paths).
    pub fn flush(&mut self, ctx: &HitContext<'_>, impacts: &mut [ImpactSample], out: &mut Vec<HitEvent>) {
        self.resolve(ctx, impacts, out, true);
    }

    fn resolve(&mut self, ctx: &HitContext<'_>, impacts: &mut [ImpactSample], out: &mut Vec<HitEvent>, all: bool) {
        // Bad timing input must not keep drops forever.
        let lag = if ctx.impact_lag_s.is_finite() { ctx.impact_lag_s.max(0.0) } else { 0.0 };
        let before = if ctx.impact_window_s.is_finite() { ctx.impact_window_s.max(0.0) } else { 0.0 };
        let due = |p: &Pending| all || ctx.now - p.t >= lag - 1e-9 || !(ctx.now >= p.t);
        if !self.pending.iter().any(due) {
            return;
        }
        self.pending.sort_by(|a, b| a.t.total_cmp(&b.t));
        // For every unused impact: the eligible pending drop (any age) nearest in time, ties to the nearer axis.
        let mut owner: Vec<Option<usize>> = vec![None; impacts.len()];
        for (i, imp) in impacts.iter().enumerate() {
            if imp.used {
                continue;
            }
            let mut best: Option<(usize, f64, f32)> = None;
            for (d, p) in self.pending.iter().enumerate() {
                let dt = imp.t - p.t;
                if !(-before - 1e-9..=lag + 1e-9).contains(&dt) {
                    continue;
                }
                let dist = dist_to_axis(&imp.pos, &p.pos, ctx.capsule_height_m);
                if !(dist.is_finite() && dist <= ctx.impact_radius_m) {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((_, bdt, bdist)) => dt.abs() < bdt - 1e-9 || (dt.abs() <= bdt + 1e-9 && dist < bdist),
                };
                if better {
                    best = Some((d, dt.abs(), dist));
                }
            }
            owner[i] = best.map(|b| b.0);
        }
        let mut keep = Vec::with_capacity(self.pending.len());
        for (d, p) in self.pending.iter().enumerate() {
            if !due(p) {
                keep.push(*p);
                continue;
            }
            let mut n = 0u32;
            let mut last: Option<(f64, [f32; 3])> = None;
            for (i, imp) in impacts.iter_mut().enumerate() {
                if owner[i] == Some(d) {
                    imp.used = true;
                    n += 1;
                    if last.is_none_or(|(t, _)| imp.t >= t) {
                        last = Some((imp.t, imp.pos));
                    }
                }
            }
            let impact_pos = last.map(|(_, p)| p);
            let via = if p.by_player_in_window {
                Some(Via::LastHitBy)
            } else if n > 0 {
                Some(Via::Impact)
            } else {
                None
            };
            match via {
                Some(via) => {
                    let kill = p.hp_after <= 0 && p.hp_before > 0;
                    out.push(HitEvent::Hit {
                        character_id: p.character_id,
                        handle: p.handle,
                        hp_before: p.hp_before,
                        hp_after: p.hp_after,
                        via,
                        n,
                        kill,
                        t_drop: p.t,
                        impact_pos,
                    });
                    if kill {
                        out.push(HitEvent::Kill { character_id: p.character_id, handle: p.handle, t_drop: p.t });
                    }
                }
                None if p.in_window => out.push(HitEvent::HpDrop {
                    character_id: p.character_id,
                    handle: p.handle,
                    hp_before: p.hp_before,
                    hp_after: p.hp_after,
                    last_hit_by: p.last_hit_by,
                    t_drop: p.t,
                }),
                None => {}
            }
        }
        self.pending = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: u64 = 0xFFFF_FFFF_1000_0001;
    const NOBODY: u64 = u64::MAX;

    fn chr(handle: u64, id: u32, hp: i32, by: u64) -> ChrSample {
        ChrSample { handle, character_id: id, hp, max_hp: 219, last_hit_by: by, pos: [10.0, 0.0, 10.0] }
    }

    fn at(mut c: ChrSample, pos: [f32; 3]) -> ChrSample {
        c.pos = pos;
        c
    }

    fn ctx(now: f64, shot: Option<f64>) -> HitContext<'static> {
        HitContext {
            player_handle: PLAYER,
            now,
            last_shot_t: shot,
            window_s: 0.6,
            exclude_character_ids: &[8000],
            impact_window_s: 0.25,
            impact_lag_s: 0.1,
            impact_radius_m: 1.0,
            capsule_height_m: 2.2,
        }
    }

    fn imp(t: f64, pos: [f32; 3]) -> ImpactSample {
        ImpactSample { t, pos, used: false }
    }

    /// Feeds `samples` at `now`, then flushes, returning all events.
    fn step_flush(t: &mut HitTracker, samples: &[ChrSample], c: &HitContext<'_>, imps: &mut [ImpactSample]) -> Vec<HitEvent> {
        let mut ev = Vec::new();
        t.update(samples, c, imps, &mut ev);
        t.flush(c, imps, &mut ev);
        ev
    }

    #[test]
    fn hit_logic_impact_at_chest_attributes() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(0.95, [10.2, 1.3, 10.1])];
        let ev = step_flush(&mut t, &[chr(1, 4311, 170, NOBODY)], &ctx(1.0, Some(0.94)), &mut imps);
        assert_eq!(
            ev,
            vec![HitEvent::Hit {
                character_id: 4311,
                handle: 1,
                hp_before: 219,
                hp_after: 170,
                via: Via::Impact,
                n: 1,
                kill: false,
                t_drop: 1.0,
                impact_pos: Some([10.2, 1.3, 10.1])
            }]
        );
        assert!(imps[0].used);
        assert_eq!(ev[0].damage(), 49);
    }

    #[test]
    fn hit_logic_drop_waits_for_late_impact() {
        // Real order (M2 run 2): drop at T, its IMPACT recorded at T + 17 ms.
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        let mut imps: Vec<ImpactSample> = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut imps, &mut ev);
        t.update(&[chr(1, 4311, 150, NOBODY)], &ctx(1.000, Some(0.95)), &mut imps, &mut ev);
        assert!(ev.is_empty(), "pending, not resolved yet: {ev:?}");
        assert_eq!(t.pending(), 1);
        imps.push(imp(1.017, [10.2, 1.3, 10.1]));
        t.update(&[chr(1, 4311, 150, NOBODY)], &ctx(1.017, Some(0.95)), &mut imps, &mut ev);
        assert!(ev.is_empty());
        t.update(&[chr(1, 4311, 150, NOBODY)], &ctx(1.101, Some(0.95)), &mut imps, &mut ev);
        assert!(matches!(ev[..], [HitEvent::Hit { n: 1, via: Via::Impact, t_drop: 1.0, kill: false, .. }]), "{ev:?}");
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn hit_logic_old_late_or_far_impact_is_hpdrop() {
        for (label, im) in [
            ("0.3 s before", imp(0.7, [10.2, 1.3, 10.1])),
            ("0.15 s after", imp(1.15, [10.2, 1.3, 10.1])),
            ("2 m away", imp(0.95, [12.0, 1.3, 10.0])),
        ] {
            let mut t = HitTracker::default();
            let mut ev = Vec::new();
            t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
            let mut imps = [im];
            t.update(&[chr(1, 4311, 170, NOBODY)], &ctx(1.0, Some(0.94)), &mut imps, &mut ev);
            t.update(&[chr(1, 4311, 170, NOBODY)], &ctx(1.2, Some(0.94)), &mut imps, &mut ev);
            assert!(matches!(ev[..], [HitEvent::HpDrop { .. }]), "{label}: {ev:?}");
            assert!(!imps[0].used, "{label}");
        }
    }

    #[test]
    fn hit_logic_one_impact_two_chars_goes_to_nearer() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        let a = at(chr(1, 4311, 219, NOBODY), [10.0, 0.0, 10.0]);
        let b = at(chr(2, 4311, 219, NOBODY), [10.8, 0.0, 10.0]);
        t.update(&[a, b], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(1.017, [10.6, 1.0, 10.0])];
        let (mut a2, mut b2) = (a, b);
        a2.hp = 170;
        b2.hp = 170;
        let ev = step_flush(&mut t, &[a2, b2], &ctx(1.0, Some(0.94)), &mut imps);
        let hits: Vec<_> = ev.iter().filter(|e| matches!(e, HitEvent::Hit { .. })).collect();
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0], HitEvent::Hit { handle: 2, n: 1, .. }));
        assert!(ev.iter().any(|e| matches!(e, HitEvent::HpDrop { handle: 1, .. })));
    }

    #[test]
    fn hit_logic_two_impacts_one_drop_counts_two() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(0.9, [10.1, 1.0, 10.0]), imp(1.017, [9.9, 1.5, 10.0])];
        let ev = step_flush(&mut t, &[chr(1, 4311, 121, NOBODY)], &ctx(1.0, Some(0.97)), &mut imps);
        assert!(matches!(ev[0], HitEvent::Hit { n: 2, via: Via::Impact, .. }), "{ev:?}");
    }

    /// C3 (docs/C3_DESIGN.md 2.6): the latest owned impact is the damage number's anchor; a last_hit_by drop without
    /// an impact has none; the counters are unchanged.
    #[test]
    fn c3_hitlogic_impact_pos_is_the_latest_owned_impact() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(1.017, [9.9, 1.5, 10.0]), imp(0.9, [10.1, 1.0, 10.0])];
        let ev = step_flush(&mut t, &[chr(1, 4311, 121, NOBODY)], &ctx(1.0, Some(0.97)), &mut imps);
        assert!(matches!(ev[0], HitEvent::Hit { n: 2, impact_pos: Some([9.9, 1.5, 10.0]), .. }), "{ev:?}");
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, 0)], &ctx(0.0, None), &mut [], &mut ev);
        let ev = step_flush(&mut t, &[chr(1, 4311, 116, PLAYER)], &ctx(0.1, Some(0.05)), &mut []);
        assert!(matches!(ev[0], HitEvent::Hit { via: Via::LastHitBy, n: 0, impact_pos: None, .. }), "{ev:?}");
        assert_eq!(ev[0].damage(), 103);
    }

    #[test]
    fn hit_logic_back_to_back_drops_keep_their_own_impacts() {
        // Drops 83 ms apart; each impact lands 17 ms after its drop and must not go to the neighbour.
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        let mut imps: Vec<ImpactSample> = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut imps, &mut ev);
        let seq = [(1.000, 150), (1.083, 101), (1.166, 52)];
        let mut now;
        for &(td, hp) in &seq {
            now = td;
            t.update(&[chr(1, 4311, hp, NOBODY)], &ctx(now, Some(td - 0.05)), &mut imps, &mut ev);
            imps.push(imp(td + 0.017, [10.1, 1.2, 10.0]));
            now = td + 0.017;
            t.update(&[chr(1, 4311, hp, NOBODY)], &ctx(now, Some(td - 0.05)), &mut imps, &mut ev);
        }
        t.update(&[chr(1, 4311, 52, NOBODY)], &ctx(1.4, Some(1.1)), &mut imps, &mut ev);
        let ns: Vec<_> = ev.iter().map(|e| if let HitEvent::Hit { n, .. } = e { *n } else { 99 }).collect();
        assert_eq!(ns, vec![1, 1, 1], "{ev:?}");
    }

    #[test]
    fn hit_logic_kill_marked() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 23, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(1.017, [10.0, 1.0, 10.0])];
        let ev1 = step_flush(&mut t, &[chr(1, 4311, 0, NOBODY)], &ctx(1.0, Some(0.94)), &mut imps);
        assert!(matches!(ev1[0], HitEvent::Hit { kill: true, hp_after: 0, .. }));
        assert_eq!(ev1[1], HitEvent::Kill { character_id: 4311, handle: 1, t_drop: 1.0 });
        let ev2 = step_flush(&mut t, &[chr(1, 4311, 0, NOBODY)], &ctx(1.2, Some(1.05)), &mut []);
        assert!(ev2.is_empty(), "no second kill");
    }

    #[test]
    fn hit_logic_small_ticks_without_impact_are_hpdrop() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4351, 657, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        let ev1 = step_flush(&mut t, &[chr(1, 4351, 655, NOBODY)], &ctx(0.1, Some(0.05)), &mut []);
        assert!(matches!(ev1[0], HitEvent::HpDrop { hp_before: 657, hp_after: 655, .. }));
        // outside the shot window: nothing at all
        let ev2 = step_flush(&mut t, &[chr(1, 4351, 653, NOBODY)], &ctx(5.0, Some(0.05)), &mut []);
        assert!(ev2.is_empty());
    }

    #[test]
    fn hit_logic_last_hit_by_still_attributes() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, 0)], &ctx(0.0, None), &mut [], &mut ev);
        let ev = step_flush(&mut t, &[chr(1, 4311, 116, PLAYER)], &ctx(0.1, Some(0.05)), &mut []);
        assert!(matches!(ev[0], HitEvent::Hit { via: Via::LastHitBy, n: 0, kill: false, .. }));
    }

    #[test]
    fn hit_logic_torrent_and_player_excluded() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(2, 8000, 500, 0), chr(PLAYER, 0, 300, 0)], &ctx(0.0, None), &mut [], &mut ev);
        let mut imps = [imp(0.05, [10.0, 1.0, 10.0])];
        let ev = step_flush(&mut t, &[chr(2, 8000, 400, PLAYER), chr(PLAYER, 0, 200, PLAYER)], &ctx(0.1, Some(0.05)), &mut imps);
        assert!(ev.is_empty());
        assert!(!imps[0].used, "Torrent never consumes an impact");
        assert_eq!(t.tracked(), 0);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn hit_logic_vanished_handle_dropped() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, 0), chr(3, 4311, 219, 0)], &ctx(0.0, None), &mut [], &mut ev);
        assert_eq!(t.tracked(), 2);
        t.update(&[chr(3, 4311, 219, 0)], &ctx(0.1, None), &mut [], &mut ev);
        assert_eq!(t.tracked(), 1);
        let ev = step_flush(&mut t, &[chr(1, 4311, 10, PLAYER)], &ctx(0.2, Some(0.15)), &mut []);
        assert!(ev.is_empty());
    }

    #[test]
    fn hit_logic_clear_drops_pending() {
        let mut t = HitTracker::default();
        let mut ev = Vec::new();
        t.update(&[chr(1, 4311, 219, NOBODY)], &ctx(0.0, None), &mut [], &mut ev);
        t.update(&[chr(1, 4311, 150, NOBODY)], &ctx(1.0, Some(0.95)), &mut [], &mut ev);
        assert_eq!(t.pending(), 1);
        t.clear();
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn hit_logic_axis_distance_and_prune() {
        assert!((dist_to_axis(&[1.0, 1.0, 0.0], &[0.0, 0.0, 0.0], 2.2) - 1.0).abs() < 1e-6);
        assert!((dist_to_axis(&[0.0, 3.2, 0.0], &[0.0, 0.0, 0.0], 2.2) - 1.0).abs() < 1e-6);
        assert!((dist_to_axis(&[0.0, -0.5, 0.0], &[0.0, 0.0, 0.0], 2.2) - 0.5).abs() < 1e-6);
        let mut v = vec![imp(0.0, [0.0; 3]), imp(0.9, [0.0; 3]), imp(1.0, [f32::NAN, 0.0, 0.0])];
        prune_impacts(&mut v, 1.5, 1.0);
        assert_eq!(v.len(), 1);
    }

    /// Replay of M2 run 2 (`logs\mw2er.log`, second session, 2026-10-14 09:35:33 and 09:35:58): every HPDROP and
    /// every IMPACT kind=hit with its real timestamp and position, at a 60 Hz tick. Character positions are not
    /// in that log, so each character stands still near the centre of its own close impacts (which spread over
    /// about 1.7 m as it moved) and the radius is widened to 1.5 m: this test checks the timing rule, not the
    /// radius (the far impacts at 10+ m stay excluded either way).
    #[test]
    fn hit_logic_replay_m2_run2() {
        // (t, character id, hp after); hp before = previous value (219 / 582 at the start)
        let drops_4311: &[(f64, i32)] =
            &[(33.169, 150), (33.435, 101), (33.536, 52), (33.703, 3), (33.886, 0)];
        let drops_4351: &[(f64, i32)] = &[
            (58.470, 524),
            (58.570, 466),
            (58.653, 408),
            (58.837, 321),
            (58.937, 277),
            (59.020, 233),
            (59.103, 189),
            (59.203, 145),
            (59.369, 101),
            (59.737, 57),
            (59.920, 13),
            (60.004, 0),
        ];
        let impacts: &[(f64, [f32; 3])] = &[
            (33.103, [50.80, -7.42, -59.40]),
            (33.186, [34.94, -4.48, -48.76]),
            (33.302, [68.80, -6.62, -68.68]),
            (33.385, [50.87, -7.49, -58.96]),
            (33.453, [35.32, -5.44, -48.73]),
            (33.553, [35.85, -5.75, -48.13]),
            (33.620, [25.94, -4.62, -43.24]),
            (33.720, [35.91, -5.16, -48.73]),
            (33.820, [34.64, -5.76, -49.47]),
            (33.903, [35.28, -4.71, -48.48]),
            (34.003, [35.57, -5.58, -48.59]),
            (58.487, [3.02, 0.66, -4.00]),
            (58.587, [2.86, 0.64, -4.05]),
            (58.670, [2.73, 0.42, -4.39]),
            (58.787, [15.47, -1.57, 1.54]),
            (58.853, [1.71, 0.81, -4.96]),
            (58.953, [1.89, 0.54, -4.59]),
            (59.037, [2.31, 0.65, -4.38]),
            (59.120, [2.93, 0.70, -4.37]),
            (59.220, [2.73, 0.60, -3.70]),
            (59.303, [8.22, -1.00, 0.13]),
            (59.387, [2.87, 0.61, -3.74]),
            (59.503, [20.81, -1.46, 3.52]),
            (59.569, [11.32, -1.39, -0.17]),
            (59.687, [13.64, -1.53, 0.58]),
            (59.753, [3.38, -0.01, -3.51]),
            (59.852, [6.11, -1.05, -2.19]),
            (59.936, [2.95, 0.35, -3.53]),
            (60.020, [2.99, 0.53, -3.56]),
        ];
        let pos_4311 = [35.4f32, -6.4, -48.6];
        let pos_4351 = [2.6f32, -1.0, -4.2];
        let mut t = HitTracker::default();
        let mut imps: Vec<ImpactSample> = Vec::new();
        let mut ev = Vec::new();
        let (mut hp_a, mut hp_b) = (219, 582);
        let (mut ia, mut ib, mut ii) = (0usize, 0usize, 0usize);
        // tick at 60 Hz from 33.0 to 34.2 and from 58.3 to 60.2; events snap to their own logged times
        let mut times: Vec<f64> = Vec::new();
        for (a, b) in [(33.0, 34.2), (58.3, 60.2)] {
            let mut x: f64 = a;
            while x < b {
                times.push(x);
                x += 1.0 / 60.0;
            }
        }
        times.extend(drops_4311.iter().map(|d| d.0));
        times.extend(drops_4351.iter().map(|d| d.0));
        times.extend(impacts.iter().map(|d| d.0));
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        for &now in &times {
            while ii < impacts.len() && impacts[ii].0 <= now + 1e-9 {
                imps.push(imp(impacts[ii].0, impacts[ii].1));
                ii += 1;
            }
            while ia < drops_4311.len() && drops_4311[ia].0 <= now + 1e-9 {
                hp_a = drops_4311[ia].1;
                ia += 1;
            }
            while ib < drops_4351.len() && drops_4351[ib].0 <= now + 1e-9 {
                hp_b = drops_4351[ib].1;
                ib += 1;
            }
            prune_impacts(&mut imps, now, 1.0);
            let s = [at(chr(11, 4311, hp_a, NOBODY), pos_4311), at(chr(12, 4351, hp_b, NOBODY), pos_4351)];
            let c = HitContext { impact_radius_m: 1.5, ..ctx(now, Some(now - 0.03)) };
            t.update(&s, &c, &mut imps, &mut ev);
        }
        let hits = |id: u32, kill: bool| -> Vec<(i32, u32)> {
            ev.iter()
                .filter_map(|e| match *e {
                    HitEvent::Hit { character_id, hp_before, hp_after, n, kill: k, .. } if character_id == id && k == kill => {
                        Some((hp_before - hp_after, n))
                    }
                    _ => None,
                })
                .collect()
        };
        assert!(!ev.iter().any(|e| matches!(e, HitEvent::HpDrop { .. })), "unattributed drops: {ev:?}");
        assert_eq!(hits(4311, false), vec![(69, 1), (49, 1), (49, 1), (49, 1)], "{ev:?}");
        assert_eq!(hits(4311, true).len(), 1);
        let b = hits(4351, false);
        assert_eq!(b.len(), 11, "{ev:?}");
        assert!(b.iter().all(|&(_, n)| n == 1), "{b:?}");
        assert_eq!(b.iter().map(|x| x.0).collect::<Vec<_>>(), vec![58, 58, 58, 87, 44, 44, 44, 44, 44, 44, 44]);
        assert_eq!(hits(4351, true), vec![(13, 1)]);
        assert_eq!(t.pending(), 0);
    }
}
