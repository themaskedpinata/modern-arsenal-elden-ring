//! C3 review 2026-10-24 ("no bullet occlusion, bullet trails can be seen through walls"; two-player run, logs\coop\
//! host_c6_2026-10-24.log / friend_c6_2026-10-24.log): the peers' tracers (`coop_vis::RemoteTracker` segments) and our
//! own launcher streaks are ImGui lines on the HUD's one background draw list, drawn at Present over ER's finished frame
//! with no depth test. ER's scene depth (docs/C6_HOLD_DESIGN.md part 15) is not live in either log (no `DEPTH` line; it
//! only ever primes the world-gun pass, never ImGui), so the visibility comes from ER's map rays on the CPU instead.
//!
//! Pure; the DLL passes a ray closure (`havok::map_ray` from the task thread) and draws the pieces:
//! - **clip**: one ray from the shot's start along its direction; the tracer never reaches past the first map hit
//!   (minus [`CLIP_BACK_M`]), whatever the twin's last read position was;
//! - **visibility**: up to [`SAMPLES`] points along the (clipped) path, each a ray camera -> point; a point is hidden
//!   when the map is hit more than [`HIT_TOL_M`] before it. The drawn pieces are the runs of visible points (a pair with
//!   one visible end is cut at its middle). Cached per tracer, re-cast when the camera moved [`MOVE_REFRESH_M`] since
//!   that point was cast; at most `budget` rays per tick, new tracers' clips first;
//! - **fallback**: a point or clip without a result yet counts as visible / unclipped, and with no rays at all (no
//!   physics world, `tracer_occlusion = false`) a tracer draws exactly as before.

use crate::vec3::V3;

/// Points per path (at most; a short path uses one per [`MIN_STEP_M`]).
pub const SAMPLES: usize = 9;
/// Never sample closer than this along a path.
pub const MIN_STEP_M: f32 = 1.0;
/// A map hit this close before the point does not hide it (the impact point sits on the wall it hit).
pub const HIT_TOL_M: f32 = 0.25;
/// The clipped end stays this far in front of the wall.
pub const CLIP_BACK_M: f32 = 0.05;
/// The clip ray's range (a twin flies at most a band's 150 m; launchers / far shots stay unclipped beyond).
pub const CLIP_RANGE_M: f32 = 400.0;
/// A peer's explosion flash is hidden only when the map is hit this far before it (it is big and sits on a surface).
pub const FLASH_TOL_M: f32 = 1.0;
/// A point is re-cast when the camera moved this far since it was cast.
pub const MOVE_REFRESH_M: f32 = 0.75;
/// Paths tracked (more draw unchanged).
pub const MAX_PATHS: usize = 64;
/// The first sample span of a path whose clip missed (or is not cast yet); it doubles while the bullet flies on.
pub const MIN_SPAN_M: f32 = 8.0;
/// Points nearer the camera than this need no ray (always visible).
pub const NEAR_M: f32 = 0.3;

/// A map ray: origin, unit direction, range (m) -> the first hit's distance, if any.
pub type RayFn<'a> = &'a mut dyn FnMut(V3, V3, f32) -> Option<f32>;

/// One tracer this tick: `key` is stable for its life (the twin's handle), `a` its start (the peer's muzzle), `b` its
/// current / last position, `dir` the shot direction (the twin's velocity) when known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathIn {
    pub key: u64,
    pub a: V3,
    pub b: V3,
    pub dir: Option<V3>,
}

impl PathIn {
    /// The direction the path is sampled / clipped along and its current length along it.
    fn line(&self) -> Option<(V3, f32)> {
        let d = self.dir.and_then(|d| d.normalize()).or_else(|| (self.b - self.a).normalize())?;
        Some((d, (self.b - self.a).dot(d).max(0.0)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clip {
    Unknown,
    /// The first map hit along the shot, metres from `a`.
    Hit(f32),
    Miss,
}

#[derive(Clone, Copy, Debug)]
struct Path {
    key: u64,
    a: V3,
    dir: V3,
    clip: Clip,
    span: f32,
    /// Intervals (points = n + 1).
    n: usize,
    /// (visible, the camera it was cast from)
    vis: [Option<(bool, V3)>; SAMPLES],
}

impl Path {
    fn s(&self, k: usize) -> f32 {
        self.span * k as f32 / self.n as f32
    }

    fn end(&self, cur: f32) -> f32 {
        match self.clip {
            Clip::Hit(h) => cur.min((h - CLIP_BACK_M).max(0.0)),
            _ => cur,
        }
    }

    /// Sets the span (and the point count); resets the points when it changed.
    fn set_span(&mut self, span: f32) {
        if (span - self.span).abs() > 1e-3 {
            self.span = span.max(1e-3);
            self.n = ((self.span / MIN_STEP_M).ceil() as usize).clamp(1, SAMPLES - 1);
            self.vis = [None; SAMPLES];
        }
    }

    /// The last point index needed for a path drawn to `e` (the point after the end bounds the tip's interval).
    fn need(&self, e: f32) -> usize {
        let step = self.span / self.n as f32;
        (((e / step).floor() as usize) + 1).min(self.n)
    }
}

/// Counters of one log window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OcclStats {
    pub rays: u32,
    pub clip_hits: u32,
    /// Ticks that ended with a needed point or clip still uncast (budget).
    pub budget_out: u32,
    /// Ticks with tracers but no rays (no physics world / occlusion off): drawn unchanged.
    pub no_rays: u32,
    /// Tracers drawn this window (per tick) fully hidden / partly hidden / fully shown.
    pub hidden: u32,
    pub partial: u32,
    pub shown: u32,
}

/// The visible parts of one tracer (world metres) and whether its head (the `b` end) is among them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pieces {
    pub parts: Vec<(V3, V3)>,
    pub tip: bool,
    pub clipped: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TracerOccl {
    paths: Vec<Path>,
    pub stats: OcclStats,
}

/// The sample span of a path: powers of two from [`MIN_SPAN_M`] covering the bullet's travel `cur` (kept while it
/// still covers it), capped at the clip hit. Review 2026-10-24: the span was the clip distance itself (up to 400 m,
/// 50 m between points), so a tracer that stopped on an enemy 20 m out was judged by its muzzle point and one point
/// past its end.
fn span_for(clip: Clip, cur: f32, prev: f32) -> f32 {
    let grown = if cur <= prev && prev > 0.0 {
        prev
    } else {
        let mut sp = MIN_SPAN_M;
        while sp < cur && sp < 1.0e5 {
            sp *= 2.0;
        }
        sp
    };
    match clip {
        Clip::Hit(h) => grown.min(h.max(CLIP_BACK_M)),
        _ => grown,
    }
}

/// Whether `p` is visible from `cam` (None = no ray cast: too near needs none and is visible).
pub fn point_visible(cam: V3, p: V3, tol: f32, ray: RayFn) -> bool {
    let d = p - cam;
    let len = d.len();
    match d.normalize() {
        Some(dir) if len > NEAR_M => ray(cam, dir, len).is_none_or(|t| t >= len - tol),
        _ => true,
    }
}

impl TracerOccl {
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn tracked(&self) -> usize {
        self.paths.len()
    }

    pub fn take_stats(&mut self) -> OcclStats {
        std::mem::take(&mut self.stats)
    }

    /// One tick: forget ended tracers, add new ones, then cast up to `budget` rays (clips of new tracers first, then
    /// uncast points from the muzzle on, then points cast from a camera more than [`MOVE_REFRESH_M`] away). `rays` None
    /// = no rays available: nothing is cast, every tracer draws unchanged.
    pub fn update(&mut self, cam: V3, segs: &[PathIn], budget: u32, range_m: f32, rays: Option<RayFn>) {
        self.paths.retain(|p| segs.iter().any(|s| s.key == p.key));
        for s in segs {
            let Some((dir, cur)) = s.line() else { continue };
            if !self.paths.iter().any(|p| p.key == s.key) && self.paths.len() < MAX_PATHS {
                self.paths.push(Path { key: s.key, a: s.a, dir, clip: Clip::Unknown, span: 0.0, n: 1, vis: [None; SAMPLES] });
            }
            if let Some(p) = self.paths.iter_mut().find(|p| p.key == s.key) {
                let span = span_for(p.clip, cur, p.span);
                p.set_span(span);
            }
        }
        if segs.is_empty() {
            return;
        }
        let Some(ray) = rays else {
            self.stats.no_rays += 1;
            return;
        };
        let mut left = budget;
        let cur_of = |key: u64| segs.iter().find(|s| s.key == key).and_then(|s| s.line()).map_or(0.0, |(_, c)| c);
        // 1. clips
        for p in self.paths.iter_mut().filter(|p| p.clip == Clip::Unknown) {
            if left == 0 {
                break;
            }
            left -= 1;
            self.stats.rays += 1;
            p.clip = match ray(p.a, p.dir, range_m) {
                Some(t) => {
                    self.stats.clip_hits += 1;
                    Clip::Hit(t)
                }
                None => Clip::Miss,
            };
            if let Clip::Hit(_) = p.clip {
                let span = span_for(p.clip, cur_of(p.key), p.span);
                p.set_span(span);
            }
        }
        // 2. uncast points, 3. stale points
        for stale in [false, true] {
            for p in self.paths.iter_mut() {
                let e = p.end(cur_of(p.key));
                for k in 0..=p.need(e) {
                    let redo = match p.vis[k] {
                        None => !stale,
                        Some((_, c)) => stale && (c - cam).len() > MOVE_REFRESH_M,
                    };
                    if !redo {
                        continue;
                    }
                    if left == 0 {
                        break;
                    }
                    let pt = p.a + p.dir * p.s(k);
                    let d = (pt - cam).len();
                    if d > NEAR_M {
                        left -= 1;
                        self.stats.rays += 1;
                    }
                    p.vis[k] = Some((point_visible(cam, pt, HIT_TOL_M, ray), cam));
                }
            }
        }
        let uncast = self.paths.iter().any(|p| p.clip == Clip::Unknown || (0..=p.need(p.end(cur_of(p.key)))).any(|k| p.vis[k].is_none()));
        if uncast {
            self.stats.budget_out += 1;
        }
    }

    /// The parts of `s` to draw (an untracked tracer: the whole segment, as before).
    pub fn pieces(&self, s: &PathIn) -> Pieces {
        let whole = Pieces { parts: vec![(s.a, s.b)], tip: true, clipped: false };
        let Some(p) = self.paths.iter().find(|p| p.key == s.key) else { return whole };
        let Some((_, cur)) = s.line() else { return whole };
        let e = p.end(cur);
        let clipped = e < cur - 1e-3;
        let v = |k: usize| p.vis[k].is_none_or(|(x, _)| x);
        if e <= 1e-4 {
            return Pieces { parts: if v(0) { vec![(s.a, s.a)] } else { Vec::new() }, tip: v(0), clipped };
        }
        let mut runs: Vec<(f32, f32)> = Vec::new();
        for k in 0..p.n {
            let lo = p.s(k);
            if lo >= e {
                break;
            }
            let hi = p.s(k + 1).min(e);
            let mid = 0.5 * (p.s(k) + p.s(k + 1));
            let part = match (v(k), v(k + 1)) {
                (true, true) => Some((lo, hi)),
                (true, false) => Some((lo, hi.min(mid))),
                (false, true) if mid < hi => Some((mid.max(lo), hi)),
                _ => None,
            };
            if let Some((x, y)) = part {
                match runs.last_mut() {
                    Some(r) if (r.1 - x).abs() < 1e-4 => r.1 = y,
                    _ => runs.push((x, y)),
                }
            }
        }
        let tip = runs.last().is_some_and(|r| (r.1 - e).abs() < 1e-3);
        if !clipped && tip && runs.len() == 1 && runs[0].0 < 1e-4 {
            // all of it: exactly the segment as before (no float noise from a + dir * s)
            return whole;
        }
        Pieces { parts: runs.into_iter().map(|(x, y)| (p.a + p.dir * x, p.a + p.dir * y)).collect(), tip, clipped }
    }

    /// Counts one drawn tracer into the window (hidden / partial / shown against its unclipped length).
    pub fn note_drawn(&mut self, s: &PathIn, pc: &Pieces) {
        let full = s.line().map_or(0.0, |(_, c)| c);
        let drawn: f32 = pc.parts.iter().map(|(x, y)| (*y - *x).len()).sum();
        if pc.parts.is_empty() {
            self.stats.hidden += 1;
        } else if drawn < full - 0.1 {
            self.stats.partial += 1;
        } else {
            self.stats.shown += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thin vertical wall: the plane through `p` with normal `n`, `half` metres either side of `p` along `u`
    /// (horizontal, in the plane), unbounded up / down.
    #[derive(Clone, Copy)]
    struct Wall {
        p: V3,
        n: V3,
        u: V3,
        half: f32,
    }

    fn wall_ray(walls: &[Wall]) -> impl FnMut(V3, V3, f32) -> Option<f32> + '_ {
        move |o: V3, d: V3, range: f32| {
            walls
                .iter()
                .filter_map(|w| {
                    let dn = d.dot(w.n);
                    if dn.abs() < 1e-6 {
                        return None;
                    }
                    let t = (w.p - o).dot(w.n) / dn;
                    let q = o + d * t;
                    (t > 0.0 && t <= range && (q - w.p).dot(w.u).abs() <= w.half).then_some(t)
                })
                .reduce(f32::min)
        }
    }

    fn near(a: V3, b: V3, tol: f32) -> bool {
        (a - b).len() <= tol
    }

    fn side_of(d: V3) -> V3 {
        V3::new(d.z, 0.0, -d.x).normalize().unwrap()
    }

    /// friend_c6_2026-10-24.log 14:16:29.870 `SHOT n=832 ... d=22.8m src=map origin=(13.29,-1.34,-7.56)
    /// dir=(-0.165,0.020,-0.986)` / 14:16:29.937 `IMPACT pos=(9.51,-0.88,-30.05) dist_from_origin=22.8m kind=hit`: the
    /// friend's twin of that shot as the host tracks it. The wall it hit stands 22.8 m down the shot: a twin read past
    /// it (25 m) is clipped at the wall, a twin that stopped short (20 m) is drawn to its own end.
    #[test]
    fn c3_tracer_occl_clip_at_first_map_hit_friend_n832() {
        let o = V3::new(13.29, -1.34, -7.56);
        let d = V3::new(-0.165, 0.020, -0.986).normalize().unwrap();
        let hit = o + d * 22.8;
        assert!(near(hit, V3::new(9.51, -0.88, -30.05), 0.6), "the logged impact");
        let walls = [Wall { p: hit, n: d, u: side_of(d), half: 10.0 }];
        let mut ray = wall_ray(&walls);
        let cam = o + side_of(d) * 1.0 + V3::new(0.0, 1.2, 0.0) - d * 3.0; // over the shooter's shoulder
        let mut t = TracerOccl::default();
        let over = PathIn { key: 7, a: o, b: o + d * 25.0, dir: Some(d) };
        t.update(cam, &[over], 32, 300.0, Some(&mut ray));
        let pc = t.pieces(&over);
        assert!(pc.clipped && pc.tip && pc.parts.len() == 1, "{pc:?}");
        assert!(near(pc.parts[0].0, o, 1e-3));
        assert!(near(pc.parts[0].1, o + d * (22.8 - CLIP_BACK_M), 0.01), "clipped at the wall: {:?} vs {hit:?}", pc.parts[0].1);
        assert_eq!(t.stats.clip_hits, 1);
        let short = PathIn { key: 8, a: o, b: o + d * 20.0, dir: Some(d) };
        t.update(cam, &[over, short], 32, 300.0, Some(&mut ray));
        let ps = t.pieces(&short);
        assert!(!ps.clipped && ps.tip && ps.parts.len() == 1, "{ps:?}");
        assert!(near(ps.parts[0].0, o, 1e-3) && near(ps.parts[0].1, short.b, 0.01));
        // the twin's velocity unknown: the a -> b line is used
        let nodir = PathIn { key: 9, a: o, b: o + d * 25.0, dir: None };
        t.update(cam, &[nodir], 32, 300.0, Some(&mut ray));
        assert!(t.pieces(&nodir).clipped);
    }

    /// friend_c6_2026-10-24.log 14:10:42.893 `SHOT n=558 ... origin=(-1.29,-2.75,27.43) dir=(-0.537,-0.023,0.843)` and
    /// 14:10:42.977 `IMPACT ... dist_from_origin=37.0m`: that twin's path seen from a camera 15 m off its side (the
    /// camera is synthetic: the host stood in another tile then, `COOP peer fx skipped other_frame`). A short wall 3 m
    /// in front of the camera cuts the path's middle out (two pieces, the tip visible); a 60 m wall hides it all; open =
    /// exactly as before; no rays = unchanged.
    #[test]
    fn c3_tracer_occl_hides_what_a_wall_covers_friend_n558() {
        let o = V3::new(-1.29, -2.75, 27.43);
        let d = V3::new(-0.537, -0.023, 0.843).normalize().unwrap();
        let path = PathIn { key: 1, a: o, b: o + d * 37.0, dir: Some(d) };
        let side = side_of(d);
        let mid = o + d * 18.5;
        let cam = mid + side * 15.0;
        // open
        let mut ray = wall_ray(&[]);
        let mut t = TracerOccl::default();
        t.update(cam, &[path], 64, 300.0, Some(&mut ray));
        let pc = t.pieces(&path);
        assert!(pc.tip && !pc.clipped && pc.parts.len() == 1 && near(pc.parts[0].0, o, 1e-3) && near(pc.parts[0].1, path.b, 1e-3), "{pc:?}");
        t.note_drawn(&path, &pc);
        assert_eq!((t.stats.shown, t.stats.clip_hits), (1, 0));
        // a wall 3 m in front of the camera, parallel to the path, 2 m either side: its shadow on the path (15 m away)
        // is 18.5 -+ 2 * 15 / 3 = 8.5 .. 28.5 m
        let walls = [Wall { p: mid + side * 12.0, n: side, u: d, half: 2.0 }];
        let mut ray = wall_ray(&walls);
        let mut t = TracerOccl::default();
        t.update(cam, &[path], 64, 300.0, Some(&mut ray));
        let pc = t.pieces(&path);
        assert!(!pc.clipped && pc.tip, "the clip ray runs along the path: no wall there; {pc:?}");
        assert_eq!(pc.parts.len(), 2, "the middle is cut out: {pc:?}");
        assert!(near(pc.parts[0].0, o, 1e-3) && near(pc.parts[1].1, path.b, 1e-3));
        let (gap_lo, gap_hi) = ((pc.parts[0].1 - o).dot(d), (pc.parts[1].0 - o).dot(d));
        // span 64 m, 8 m between points: the cut is within one step of the shadow
        assert!((gap_lo - 8.5).abs() <= 8.0 && (gap_hi - 28.5).abs() <= 8.0 && gap_lo < gap_hi, "gap {gap_lo:.2}..{gap_hi:.2}");
        t.note_drawn(&path, &pc);
        assert_eq!((t.stats.partial, t.stats.hidden), (1, 0));
        // a 60 m wall hides all of it
        let walls = [Wall { p: mid + side * 12.0, n: side, u: d, half: 30.0 }];
        let mut ray = wall_ray(&walls);
        let mut t = TracerOccl::default();
        t.update(cam, &[path], 64, 300.0, Some(&mut ray));
        let pc = t.pieces(&path);
        assert!(pc.parts.is_empty() && !pc.tip, "{pc:?}");
        t.note_drawn(&path, &pc);
        assert_eq!(t.stats.hidden, 1);
        // no rays at all: unchanged, counted
        let mut t = TracerOccl::default();
        t.update(cam, &[path], 64, 300.0, None);
        assert_eq!(t.pieces(&path), Pieces { parts: vec![(path.a, path.b)], tip: true, clipped: false });
        assert_eq!(t.stats.no_rays, 1);
    }

    /// The budget (uncast = visible, counted), no re-cast while the camera stands, a re-cast after it moved, the
    /// growing span of a live twin, a finished tracer forgotten.
    #[test]
    fn c3_tracer_occl_budget_staleness_and_lifetime() {
        let o = V3::new(-1.29, -2.75, 27.43);
        let d = V3::new(-0.537, -0.023, 0.843).normalize().unwrap();
        let cam = o + side_of(d) * 10.0;
        let mut ray = |_: V3, _: V3, _: f32| None;
        let mut t = TracerOccl::default();
        // the C3 tracker fixture's twin moves 16 m per tick: tick 1 at 16 m
        let p1 = PathIn { key: 1, a: o, b: o + d * 16.0, dir: Some(d) };
        t.update(cam, &[p1], 3, 300.0, Some(&mut ray));
        assert_eq!((t.stats.rays, t.stats.budget_out), (3, 1), "the clip + 2 points");
        let pc = t.pieces(&p1);
        assert!(pc.tip && pc.parts.len() == 1 && near(pc.parts[0].1, p1.b, 1e-3), "uncast points draw: {pc:?}");
        t.update(cam, &[p1], 64, 300.0, Some(&mut ray));
        let r1 = t.stats.rays;
        t.update(cam, &[p1], 64, 300.0, Some(&mut ray));
        assert_eq!(t.stats.rays, r1, "nothing re-cast while the camera stands");
        // tick 3: 37 m (> the 16 m span): the span doubles, the points are cast again
        let p2 = PathIn { b: o + d * 37.0, ..p1 };
        t.update(cam, &[p2], 64, 300.0, Some(&mut ray));
        assert!(t.stats.rays > r1);
        let r2 = t.stats.rays;
        // the camera moves 1 m: the needed points are re-cast once
        t.update(cam + V3::new(1.0, 0.0, 0.0), &[p2], 64, 300.0, Some(&mut ray));
        assert!(t.stats.rays > r2);
        let r3 = t.stats.rays;
        t.update(cam + V3::new(1.0, 0.0, 0.0), &[p2], 64, 300.0, Some(&mut ray));
        assert_eq!(t.stats.rays, r3);
        // gone from the input = forgotten; an untracked tracer draws whole
        t.update(cam, &[], 64, 300.0, Some(&mut ray));
        assert_eq!(t.tracked(), 0);
        assert_eq!(t.pieces(&p2).parts, vec![(p2.a, p2.b)]);
    }

    /// Review 2026-10-24: a peer's shot that stopped on an enemy 20 m out while the map along it is hit at 400 m (the
    /// clip range), fired out of a doorway (the muzzle point hidden from the camera, everything past it in view). With
    /// the span = the clip distance (50 m steps) the tracer was judged by the muzzle and the 50 m point only and drew
    /// nothing; the span now follows the bullet (32 m, 4 m steps) and the visible piece ends at the tip.
    #[test]
    fn c3_tracer_occl_short_tracer_long_clip_keeps_visible_tip() {
        let o = V3::new(-1.29, -2.75, 27.43);
        let d = V3::new(-0.537, -0.023, 0.843).normalize().unwrap();
        let cam = o + side_of(d) * 6.0 + d * 10.0;
        let mut ray = |org: V3, dir: V3, range: f32| {
            if (org - o).len() < 1e-4 {
                return Some(400.0); // the clip ray: the map 400 m down the shot
            }
            // the camera ray to the muzzle point is blocked halfway (the doorway's jamb); every other point is open
            ((org + dir * range - o).len() < 0.5).then_some(range * 0.5)
        };
        let path = PathIn { key: 3, a: o, b: o + d * 20.0, dir: Some(d) };
        let mut t = TracerOccl::default();
        t.update(cam, &[path], 64, CLIP_RANGE_M, Some(&mut ray));
        t.update(cam, &[path], 64, CLIP_RANGE_M, Some(&mut ray));
        assert_eq!(t.stats.clip_hits, 1);
        assert_eq!(t.stats.budget_out, 0, "every needed point cast");
        let pc = t.pieces(&path);
        assert!(!pc.clipped && pc.tip && pc.parts.len() == 1, "{pc:?}");
        let (lo, hi) = ((pc.parts[0].0 - o).dot(d), (pc.parts[0].1 - o).dot(d));
        assert!((hi - 20.0).abs() < 1e-3, "ends at the tip: {hi}");
        assert!(lo > 0.0 && lo <= 4.0 + 1e-3, "only the hidden muzzle's half step is cut: {lo}");
        t.note_drawn(&path, &pc);
        assert_eq!((t.stats.partial, t.stats.hidden), (1, 0));
        // the span stays capped at a nearer clip hit (n832's wall at 22.8 m)
        assert_eq!(span_for(Clip::Hit(22.8), 25.0, 0.0), 22.8);
        assert_eq!(span_for(Clip::Hit(400.0), 20.0, 0.0), 32.0);
        assert_eq!(span_for(Clip::Hit(400.0), 20.0, 32.0), 32.0);
        assert_eq!(span_for(Clip::Miss, 37.0, 32.0), 64.0);
    }

    #[test]
    fn c3_tracer_occl_point_visible_tolerance() {
        let cam = V3::ZERO;
        let p = V3::new(0.0, 0.0, 10.0);
        // the impact point on the wall it hit: the ray hits at 9.9 m = within the tolerance
        assert!(point_visible(cam, p, HIT_TOL_M, &mut |_, _, _| Some(9.9)));
        assert!(!point_visible(cam, p, HIT_TOL_M, &mut |_, _, _| Some(5.0)));
        assert!(point_visible(cam, p, HIT_TOL_M, &mut |_, _, _| None));
        // too near for a ray
        let mut n = 0;
        assert!(point_visible(cam, V3::new(0.0, 0.0, 0.1), HIT_TOL_M, &mut |_, _, _| {
            n += 1;
            Some(0.0)
        }));
        assert_eq!(n, 0);
    }
}
