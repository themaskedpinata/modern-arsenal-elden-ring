//! Collision from ray casts (docs/M5_DESIGN.md section 5), in IW4 inches.
//!
//! Elden Ring only offers `CSPhysWorld::cast_ray` (a hit point, no normal, no shape cast), so the player's capsule
//! is approximated by rays: up to 3 heights x 3 lateral offsets along the move, wall normals from two hits at one
//! height (or two +-25 deg feelers), the exact stop distance against that plane, Q3 clip-and-slide, step-up onto
//! stairs, a ground probe with a 3-point normal, a headroom ray and a ceiling ray. Every ray goes through
//! [`RayBudget::cast`], which enforces `max_rays_per_tick` and reserves the vertical rays of the remaining substeps.
//!
//! [`synth`] is an exact ray-vs-convex world (boxes, wedges, planes) for the tests and the headless bench.

use crate::vec3::{UP, V3};

/// The game's (or the test world's) ray query: the first hit point on the segment `origin .. origin + delta`.
pub trait RayWorld {
    fn cast(&mut self, origin: V3, delta: V3) -> Option<V3>;
}

/// Ray kinds for the budget: horizontal rays may not eat the vertical rays reserved for later substeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayKind {
    Horizontal,
    Vertical,
}

/// Per-tick ray budget (`max_rays_per_tick`) and cost counter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RayBudget {
    pub used: u32,
    pub max: u32,
    /// Rays kept back for the vertical probes of the substeps still to run.
    pub reserve: u32,
    /// A horizontal request was refused this tick (logged as `budget_hits`).
    pub exhausted: bool,
}

impl RayBudget {
    pub fn new(max: u32) -> RayBudget {
        RayBudget { used: 0, max, reserve: 0, exhausted: false }
    }

    /// Whether a ray of `kind` may be cast now.
    pub fn allows(&self, kind: RayKind) -> bool {
        match kind {
            RayKind::Horizontal => self.used + self.reserve < self.max,
            RayKind::Vertical => self.used < self.max,
        }
    }

    /// Casts through the budget. `Err(())` = refused (budget spent); `Ok(hit)` otherwise.
    pub fn cast(&mut self, w: &mut dyn RayWorld, kind: RayKind, origin: V3, delta: V3) -> Result<Option<V3>, ()> {
        if !self.allows(kind) {
            if kind == RayKind::Horizontal {
                self.exhausted = true;
            }
            return Err(());
        }
        self.used += 1;
        Ok(w.cast(origin, delta))
    }
}

/// Geometry of the ray capsule for one substep (inches).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub radius: f32,
    pub skin: f32,
    /// Hull height of the current stance (scaled).
    pub hull: f32,
    pub step: f32,
    pub walkable_y: f32,
}

/// Fixed probe heights (inches): the low ray at 0.10 m.
pub const LOW_H: f32 = 0.10 / 0.0254;
/// Margin above a step for the knee ray and the step/ground probes (0.05 m).
pub const MARGIN: f32 = 0.05 / 0.0254;
/// Two probe heights closer than this are merged (0.1 m).
pub const MERGE_H: f32 = 0.10 / 0.0254;
/// Lateral offsets of the side rays (x radius).
pub const SIDE_OFFSET: f32 = 0.7;
/// Feeler yaw (degrees) when only one ray hit.
pub const FEELER_DEG: f32 = 25.0;
/// Ground-normal baseline (inches).
pub const NORMAL_BASE: f32 = 4.0;
/// Hits within the move plus this fraction of the radius (and a skin) are wall candidates; the exact stop then comes
/// from the wall plane.
pub const DETECT_EXTRA: f32 = 0.3;

/// Run 18: a waist probe is added when the knee and chest rays are further apart than this (0.45 m: standing 1.21 m,
/// crouched 0.68 m; prone 0.35 m keeps its two heights).
pub const WAIST_GAP: f32 = 0.45 / 0.0254;

impl Capsule {
    /// Probe heights above the feet: low, knee (step + margin), chest (hull - 0.15 m); merged when too close.
    pub fn heights(&self) -> ([f32; 4], usize) {
        self.heights_m5()
    }

    /// The M5 heights (low, knee, chest).
    pub fn heights_m5(&self) -> ([f32; 4], usize) {
        let low = LOW_H;
        let knee = self.step + MARGIN;
        let chest = self.hull - 0.15 / 0.0254;
        let mut out = [low, 0.0, 0.0, 0.0];
        let mut n = 1;
        for h in [knee, chest] {
            if h > out[n - 1] + MERGE_H && h < self.hull {
                out[n] = h;
                n += 1;
            }
        }
        (out, n)
    }

    /// Run 18: the M5 heights plus a waist ray half way between the knee and the chest when they are more than
    /// [`WAIST_GAP`] apart (a cart bed or a table top between 0.5 and 1.7 m was invisible to the standing sweep).
    pub fn heights_body(&self) -> ([f32; 4], usize) {
        let (mut out, n) = self.heights_m5();
        if n == 3 && out[2] - out[1] > WAIST_GAP {
            out = [out[0], out[1], 0.5 * (out[1] + out[2]), out[2]];
            return (out, 4);
        }
        (out, n)
    }
}

fn planar(v: V3) -> V3 {
    V3::new(v.x, 0.0, v.z)
}

/// Horizontal unit vector to the right of `d` (y-up): right = d x up... chosen so `side(d)` rotates d by -90 deg.
fn side_of(d: V3) -> V3 {
    V3::new(d.z, 0.0, -d.x)
}

fn rotate_y(v: V3, deg: f32) -> V3 {
    let (s, c) = deg.to_radians().sin_cos();
    V3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c)
}

/// One ray hit of a sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepHit {
    pub height: f32,
    pub offset: f32,
    pub point: V3,
    /// Allowed travel along the move before this ray's capsule slice touches (inches, may be negative).
    pub allow: f32,
}

/// Result of a horizontal sweep.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sweep {
    pub hits: Vec<SweepHit>,
    /// The budget refused a ray: the move must not happen.
    pub refused: bool,
}

impl Sweep {
    pub fn min_hit(&self) -> Option<SweepHit> {
        self.hits.iter().copied().min_by(|a, b| a.allow.total_cmp(&b.allow))
    }
}

/// Casts the 3 x 3 rays of a planar move `d` from feet `p`.
pub fn sweep(b: &mut RayBudget, w: &mut dyn RayWorld, p: V3, d: V3, c: &Capsule, heights: &[f32]) -> Sweep {
    let mut out = Sweep::default();
    let len = d.planar_len();
    let Some(dir) = planar(d).normalize() else { return out };
    let side = side_of(dir);
    for &h in heights {
        for o in [0.0, SIDE_OFFSET * c.radius, -SIDE_OFFSET * c.radius] {
            let r_eff = (c.radius * c.radius - o * o).max(0.0).sqrt();
            let origin = p + UP * h + side * o;
            // longer than the move by the skin and 0.3 r: a slanted wall is found before the skin is entered
            let ray = dir * (len + r_eff + 2.0 * c.skin + DETECT_EXTRA * c.radius);
            match b.cast(w, RayKind::Horizontal, origin, ray) {
                Err(()) => {
                    out.refused = true;
                    return out;
                }
                Ok(Some(hit)) => {
                    let t = (hit - origin).dot(dir);
                    out.hits.push(SweepHit { height: h, offset: o, point: hit, allow: t - r_eff - c.skin });
                }
                Ok(None) => {}
            }
        }
    }
    out
}

/// Run 18: the front body column of a planar move `d` from feet `p`: one down ray at the capsule's leading edge after
/// the move (axis + dir x (|d| + r + skin)) from just under the hull top down to `MERGE_H` above the knee ray, so a
/// slab between the probe heights (a cart bed, a table, a beam) is seen; stairs and walkable slopes stay below its
/// lower end. A hit is confirmed by one horizontal ray from the axis at the hit height minus 1 in (inside the
/// capsule): the face it finds is returned as a sweep hit (offset 0). A column hit whose confirm ray misses (the top
/// of something whose face is not straight ahead) stops the move at the column (`allow` up to the column, minus r).
/// `Err` = the budget refused a ray (the move must not happen).
pub fn body_column(b: &mut RayBudget, w: &mut dyn RayWorld, p: V3, d: V3, c: &Capsule, heights: &[f32]) -> Result<Option<SweepHit>, ()> {
    let len = d.planar_len();
    let Some(dir) = planar(d).normalize() else { return Ok(None) };
    let knee = heights.get(1).copied().unwrap_or(c.step + MARGIN);
    let lo = knee + MERGE_H;
    let hi = c.hull - 2.0;
    if hi <= lo + 1.0 {
        return Ok(None);
    }
    let ahead = len + c.radius + c.skin;
    let top = p + dir * ahead + UP * hi;
    let Some(hit) = b.cast(w, RayKind::Horizontal, top, UP * -(hi - lo))? else { return Ok(None) };
    let h = (hit.y - p.y - 1.0).clamp(lo - 1.0, hi);
    let origin = p + UP * h;
    let reach = ahead + 2.0 * c.skin;
    match b.cast(w, RayKind::Horizontal, origin, dir * reach)? {
        Some(face) => {
            let t = (face - origin).dot(dir);
            Ok(Some(SweepHit { height: h, offset: 0.0, point: face, allow: t - c.radius - c.skin }))
        }
        None => {
            let at = V3::new(p.x, p.y + h, p.z) + dir * ahead;
            Ok(Some(SweepHit { height: h, offset: 0.0, point: at, allow: (ahead - c.radius - c.skin).min(len) - c.skin }))
        }
    }
}

/// Horizontal wall normal facing against `dir` from two points on the wall.
fn normal_from_points(a: V3, b: V3, dir: V3) -> Option<V3> {
    let t = planar(b - a).normalize()?;
    let n = V3::new(-t.z, 0.0, t.x);
    Some(if n.dot(dir) > 0.0 { -n } else { n })
}

/// The wall normal of the closest hit: a second hit at the same height, else two feelers, else `-dir`.
pub fn wall_normal(b: &mut RayBudget, w: &mut dyn RayWorld, p: V3, dir: V3, first: &SweepHit, s: &Sweep, c: &Capsule) -> V3 {
    let other = s
        .hits
        .iter()
        .filter(|h| h.height == first.height && h.offset != first.offset && planar(h.point - first.point).len() > 0.2)
        .min_by(|a, b| a.allow.total_cmp(&b.allow));
    if let Some(o) = other
        && let Some(n) = normal_from_points(first.point, o.point, dir)
    {
        return n;
    }
    let origin = p + UP * first.height;
    let reach = (first.point - origin).planar_len() * 2.0 + c.radius + 4.0;
    let mut pts = Vec::with_capacity(2);
    for deg in [FEELER_DEG, -FEELER_DEG] {
        let fd = rotate_y(dir, deg);
        if let Ok(Some(hit)) = b.cast(w, RayKind::Horizontal, origin, fd * reach) {
            pts.push(hit);
        }
    }
    for q in &pts {
        if planar(*q - first.point).len() > 0.2
            && let Some(n) = normal_from_points(first.point, *q, dir)
        {
            return n;
        }
    }
    if pts.len() == 2
        && let Some(n) = normal_from_points(pts[0], pts[1], dir)
    {
        return n;
    }
    -dir
}

/// A ground probe result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    pub y: f32,
    pub point: V3,
}

/// Straight down from `top` to `bottom` (heights) at (x, z): the first hit.
pub fn down_ray(b: &mut RayBudget, w: &mut dyn RayWorld, at: V3, top: f32, bottom: f32) -> Result<Option<Ground>, ()> {
    if bottom >= top {
        return Ok(None);
    }
    let o = V3::new(at.x, top, at.z);
    b.cast(w, RayKind::Vertical, o, V3::new(0.0, bottom - top, 0.0)).map(|h| h.map(|p| Ground { y: p.y, point: p }))
}

/// Plane normal from three ground points (y > 0), or `None` when degenerate.
pub fn normal3(a: V3, bb: V3, c: V3) -> Option<V3> {
    let n = (bb - a).cross(c - a).normalize()?;
    Some(if n.y < 0.0 { -n } else { n })
}

/// Ground normal at `at` from four extra down rays: at `NORMAL_BASE / 2` and `NORMAL_BASE` along `fwd` and along its
/// side. A plane gives a linear height profile on both axes; a stair edge gives a jump (the half-way sample is not
/// half of the full one), which is `None` (an edge, not a slope), as are a missed ray and a refused budget (the
/// caller keeps its last normal).
pub fn ground_normal(b: &mut RayBudget, w: &mut dyn RayWorld, at: V3, ground_y: f32, fwd: V3, step: f32) -> Option<V3> {
    let f = planar(fwd).normalize().unwrap_or(V3::new(0.0, 0.0, 1.0));
    let s = side_of(f);
    if b.used + 4 + b.reserve > b.max {
        return None;
    }
    let top = ground_y + step + MARGIN;
    let bottom = ground_y - step - MARGIN;
    let a = V3::new(at.x, ground_y, at.z);
    let mut axis = |dir: V3| -> Option<V3> {
        let half = down_ray(b, w, a + dir * (NORMAL_BASE * 0.5), top, bottom).ok()??;
        let full = down_ray(b, w, a + dir * NORMAL_BASE, top, bottom).ok()??;
        let (dh, df) = (half.y - ground_y, full.y - ground_y);
        if (df - 2.0 * dh).abs() > 0.25 + 0.05 * df.abs() {
            return None;
        }
        Some(full.point)
    };
    let p1 = axis(f)?;
    let p2 = axis(s)?;
    normal3(a, p1, p2)
}

/// Up ray: is there room from `from_y + 2 in` to `to_y` at (x, z)? `Err` = budget refused (counts as blocked).
pub fn headroom(b: &mut RayBudget, w: &mut dyn RayWorld, at: V3, from_y: f32, to_y: f32) -> bool {
    let o = V3::new(at.x, from_y + 2.0, at.z);
    let len = to_y - o.y;
    if len <= 0.0 {
        return true;
    }
    matches!(b.cast(w, RayKind::Vertical, o, V3::new(0.0, len, 0.0)), Ok(None))
}

/// Ceiling ray of a rising player: the highest feet y allowed for a rise to `y_new` (`None` = no ceiling).
pub fn ceiling(b: &mut RayBudget, w: &mut dyn RayWorld, p: V3, hull: f32, y_new: f32) -> Option<f32> {
    let o = p + UP * (hull - 2.0);
    let len = (y_new - p.y) + 2.0 + MARGIN;
    match b.cast(w, RayKind::Vertical, o, V3::new(0.0, len, 0.0)) {
        Ok(Some(h)) => Some(h.y - hull - 0.5),
        _ => None,
    }
}

/// The exact allowed travel of a capsule (center `c` at the hit height) along `dir` against the wall plane through
/// `point` with normal `n`, keeping `radius + skin` away. `None` when moving parallel / away.
pub fn plane_allow(c: V3, dir: V3, point: V3, n: V3, radius: f32, skin: f32) -> Option<f32> {
    let into = -dir.dot(n);
    if into <= 1e-4 {
        return None;
    }
    let dist = (c - point).dot(n);
    Some((dist - radius - skin) / into)
}

/// Exact ray-vs-convex test world (inches) for tests and the bench. A ray starting inside a solid misses it (the
/// game's assumed behaviour).
pub mod synth {
    use super::RayWorld;
    use crate::vec3::V3;

    /// Convex solid: inside = every `n . p <= d`.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Convex {
        pub planes: Vec<(V3, f32)>,
    }

    impl Convex {
        pub fn aabb(min: V3, max: V3) -> Convex {
            Convex {
                planes: vec![
                    (V3::new(1.0, 0.0, 0.0), max.x),
                    (V3::new(-1.0, 0.0, 0.0), -min.x),
                    (V3::new(0.0, 1.0, 0.0), max.y),
                    (V3::new(0.0, -1.0, 0.0), -min.y),
                    (V3::new(0.0, 0.0, 1.0), max.z),
                    (V3::new(0.0, 0.0, -1.0), -min.z),
                ],
            }
        }

        /// A ramp rising along +z from (z0, y0) at `deg`, spanning x in [x0, x1], up to `y_top`, solid below.
        pub fn ramp(x0: f32, x1: f32, z0: f32, y0: f32, deg: f32, y_top: f32) -> Convex {
            let (s, c) = deg.to_radians().sin_cos();
            // top surface: points with (y - y0) cos - (z - z0) sin <= 0  ->  n = (0, cos, -sin)
            let n = V3::new(0.0, c, -s);
            let d = n.dot(V3::new(0.0, y0, z0));
            let z_top = z0 + (y_top - y0) / deg.to_radians().tan();
            Convex {
                planes: vec![
                    (n, d),
                    (V3::new(1.0, 0.0, 0.0), x1),
                    (V3::new(-1.0, 0.0, 0.0), -x0),
                    (V3::new(0.0, -1.0, 0.0), -(y0 - 1000.0)),
                    (V3::new(0.0, 1.0, 0.0), y_top),
                    (V3::new(0.0, 0.0, 1.0), z_top + 1000.0),
                ],
            }
        }

        /// M8: a rounded rock: an `n`-sided vertical prism (base at `base.y`, circumradius `radius`, flat crown at
        /// `top`) whose upper edge is cut by rings of planes tangent to a dome of radius `dome` at tilts 15/35/55 deg
        /// (n.y 0.97 / 0.82 / 0.57: only the crown and the first ring are walkable).
        pub fn rock(base: V3, radius: f32, top: f32, n: usize, dome: f32) -> Convex {
            let n = n.max(3);
            let mut planes = vec![(V3::new(0.0, 1.0, 0.0), top), (V3::new(0.0, -1.0, 0.0), -base.y)];
            for k in 0..n {
                let a = (k as f32 + 0.5) * std::f32::consts::TAU / n as f32;
                let nn = V3::new(a.cos(), 0.0, a.sin());
                planes.push((nn, nn.dot(V3::new(base.x, 0.0, base.z)) + radius));
            }
            let centre = V3::new(base.x, top - dome, base.z);
            for tilt in [15.0f32, 35.0, 55.0] {
                let (s, c) = tilt.to_radians().sin_cos();
                for k in 0..n {
                    let a = k as f32 * std::f32::consts::TAU / n as f32;
                    let nn = V3::new(a.cos() * s, c, a.sin() * s);
                    planes.push((nn, nn.dot(centre) + dome));
                }
            }
            Convex { planes }
        }

        /// M8: a block spanning x0..x1, z0..z1 from `y_lo`, whose top passes through `at` and rises at `deg` along the
        /// planar unit direction `rise_dir` (a sloped top).
        pub fn wedge(x0: f32, x1: f32, z0: f32, z1: f32, y_lo: f32, at: V3, deg: f32, rise_dir: V3) -> Convex {
            let (s, c) = deg.to_radians().sin_cos();
            let n = V3::new(-s * rise_dir.x, c, -s * rise_dir.z);
            Convex {
                planes: vec![
                    (n, n.dot(at)),
                    (V3::new(1.0, 0.0, 0.0), x1),
                    (V3::new(-1.0, 0.0, 0.0), -x0),
                    (V3::new(0.0, 0.0, 1.0), z1),
                    (V3::new(0.0, 0.0, -1.0), -z0),
                    (V3::new(0.0, -1.0, 0.0), -y_lo),
                ],
            }
        }

        /// The point is inside the solid (M8: the never-inside-geometry oracle).
        pub fn inside(&self, p: V3) -> bool {
            self.planes.iter().all(|(n, d)| n.dot(p) <= *d + 1e-4)
        }

        /// Strictly inside by more than `margin` (inches) from every face.
        pub fn inside_by(&self, p: V3, margin: f32) -> bool {
            self.planes.iter().all(|(n, d)| n.dot(p) < *d - margin)
        }

        /// Entry parameter t in [0, 1] of the segment, or None.
        pub fn ray(&self, o: V3, delta: V3) -> Option<f32> {
            if self.inside(o) {
                return None;
            }
            let (mut t0, mut t1) = (0.0f32, 1.0f32);
            for (n, d) in &self.planes {
                let denom = n.dot(delta);
                let dist = d - n.dot(o);
                if denom.abs() < 1e-9 {
                    if dist < 0.0 {
                        return None;
                    }
                } else {
                    let t = dist / denom;
                    if denom < 0.0 {
                        t0 = t0.max(t);
                    } else {
                        t1 = t1.min(t);
                    }
                }
                if t0 > t1 {
                    return None;
                }
            }
            Some(t0)
        }
    }

    /// The test world: a list of convex solids. Counts rays.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct World {
        pub solids: Vec<Convex>,
        pub rays: u64,
    }

    impl World {
        /// A flat floor (top at y = 0) spanning +-10 km.
        pub fn flat() -> World {
            World { solids: vec![Convex::aabb(V3::new(-4e5, -100.0, -4e5), V3::new(4e5, 0.0, 4e5))], rays: 0 }
        }

        pub fn with(mut self, c: Convex) -> World {
            self.solids.push(c);
            self
        }

        /// M8: the first of `n` points on the vertical segment feet .. feet + height that is more than `margin`
        /// inside a solid (`None` = the capsule axis is clear).
        pub fn axis_inside(&self, feet: V3, height: f32, n: usize, margin: f32) -> Option<V3> {
            (0..n.max(2)).map(|i| feet + V3::new(0.0, height * i as f32 / (n.max(2) - 1) as f32, 0.0)).find(|p| self.solids.iter().any(|s| s.inside_by(*p, margin)))
        }

        /// M8 review: the capsule's radius, not only its axis. The first of `n` axis points from feet + radius to feet
        /// + height - radius (the cylinder between the rounded ends) whose disc of `radius` reaches more than `margin`
        /// into a solid: each plane is pushed out by `radius x |n_planar|` (exact on faces, a little conservative at
        /// vertical edges: about 0.55 in on the 12-sided rocks).
        pub fn capsule_inside(&self, feet: V3, height: f32, radius: f32, n: usize, margin: f32) -> Option<V3> {
            let lo = radius.min(height * 0.5);
            let hi = (height - radius).max(lo);
            let n = n.max(2);
            (0..n).map(|i| feet + V3::new(0.0, lo + (hi - lo) * i as f32 / (n - 1) as f32, 0.0)).find(|p| {
                self.solids.iter().any(|s| {
                    s.planes.iter().all(|(nn, d)| {
                        let grow = radius * (nn.x * nn.x + nn.z * nn.z).sqrt();
                        nn.dot(*p) < *d + grow - margin
                    })
                })
            })
        }
    }

    impl RayWorld for World {
        fn cast(&mut self, origin: V3, delta: V3) -> Option<V3> {
            self.rays += 1;
            let t = self.solids.iter().filter_map(|s| s.ray(origin, delta)).fold(f32::INFINITY, f32::min);
            t.is_finite().then(|| origin + delta * t)
        }
    }

    /// Run 18: two collision layers like ER's Havok filters: `map` (what the map filter 0x2000058 sees) and
    /// `objects` (assets: wagons, crates, rocks placed as assets), cast by `mode` like the DLL's `HavokRays`.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Layered {
        pub map: World,
        pub objects: World,
        /// 0 = union (the nearer hit), 1 = map only (the M8 behaviour), 2 = objects only.
        pub mode: u8,
        /// Hits per source (map, object) of the nearest hit.
        pub hits: [u64; 2],
    }

    impl Layered {
        /// Every solid of both layers (the inside oracles judge against all of them).
        pub fn all(&self) -> World {
            let mut w = self.map.clone();
            w.solids.extend(self.objects.solids.iter().cloned());
            w
        }
    }

    impl RayWorld for Layered {
        fn cast(&mut self, origin: V3, delta: V3) -> Option<V3> {
            let a = if self.mode != 2 { self.map.cast(origin, delta) } else { None };
            let b = if self.mode != 1 { self.objects.cast(origin, delta) } else { None };
            match (a, b) {
                (Some(x), Some(y)) => {
                    let near_obj = (y - origin).len() < (x - origin).len();
                    self.hits[near_obj as usize] += 1;
                    Some(if near_obj { y } else { x })
                }
                (Some(x), None) => {
                    self.hits[0] += 1;
                    Some(x)
                }
                (None, Some(y)) => {
                    self.hits[1] += 1;
                    Some(y)
                }
                (None, None) => None,
            }
        }
    }

    /// A world where every ray misses and costs nothing (the bench's CPU measurement).
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Empty;

    impl RayWorld for Empty {
        fn cast(&mut self, _: V3, _: V3) -> Option<V3> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::synth::{Convex, World};
    use super::*;

    #[test]
    fn move_collide_synth_ray_exact() {
        let mut w = World::flat().with(Convex::aabb(V3::new(-10.0, 0.0, 50.0), V3::new(10.0, 100.0, 52.0)));
        let h = w.cast(V3::new(0.0, 10.0, 0.0), V3::new(0.0, 0.0, 100.0)).unwrap();
        assert!((h.z - 50.0).abs() < 1e-4);
        assert!(w.cast(V3::new(0.0, 10.0, 0.0), V3::new(0.0, 0.0, 40.0)).is_none());
        let g = w.cast(V3::new(0.0, 10.0, 0.0), V3::new(0.0, -20.0, 0.0)).unwrap();
        assert!(g.y.abs() < 1e-4);
        // starting inside: miss
        assert!(w.cast(V3::new(0.0, 10.0, 51.0), V3::new(0.0, 0.0, 10.0)).is_none());
        // ramp surface at 30 deg
        let r = Convex::ramp(-50.0, 50.0, 0.0, 0.0, 30.0, 100.0);
        let t = r.ray(V3::new(0.0, 50.0, 20.0), V3::new(0.0, -100.0, 0.0)).unwrap();
        let y = 50.0 - 100.0 * t;
        assert!((y - 20.0 * 30f32.to_radians().tan()).abs() < 1e-3, "{y}");
    }

    #[test]
    fn move_collide_budget_reserves_vertical() {
        let mut w = World::flat();
        let mut b = RayBudget { used: 0, max: 4, reserve: 2, exhausted: false };
        assert!(b.cast(&mut w, RayKind::Horizontal, V3::ZERO, V3::new(1.0, 0.0, 0.0)).is_ok());
        assert!(b.cast(&mut w, RayKind::Horizontal, V3::ZERO, V3::new(1.0, 0.0, 0.0)).is_ok());
        assert!(b.cast(&mut w, RayKind::Horizontal, V3::ZERO, V3::new(1.0, 0.0, 0.0)).is_err(), "reserve kept");
        assert!(b.exhausted);
        assert!(b.cast(&mut w, RayKind::Vertical, V3::ZERO, V3::new(0.0, -1.0, 0.0)).is_ok());
        assert!(b.cast(&mut w, RayKind::Vertical, V3::ZERO, V3::new(0.0, -1.0, 0.0)).is_ok());
        assert!(b.cast(&mut w, RayKind::Vertical, V3::ZERO, V3::new(0.0, -1.0, 0.0)).is_err());
        assert_eq!(b.used, 4);
        assert_eq!(w.rays, 4);
    }

    #[test]
    fn move_collide_body_column_sees_a_slab_between_the_probe_heights() {
        // run 18: a cart bed 0.62-0.95 m high (between the knee ray at 0.51 m and the waist / chest rays) right ahead
        let bed = Convex::aabb(V3::new(-40.0, 0.62 / 0.0254, 30.0), V3::new(40.0, 0.95 / 0.0254, 100.0));
        let mut w = World::flat().with(bed);
        let c = Capsule { radius: 15.75, skin: 1.18, hull: 73.5, step: 18.0, walkable_y: 0.7 };
        let p = V3::new(0.0, 0.0, 10.0);
        let d = V3::new(0.0, 0.0, 6.0);
        let mut b = RayBudget::new(64);
        let (hs, n) = c.heights_m5();
        let s = sweep(&mut b, &mut w, p, d, &c, &hs[..n]);
        assert!(s.hits.iter().all(|h| h.allow > 6.0), "the M5 heights miss the bed: {s:?}");
        let hit = body_column(&mut b, &mut w, p, d, &c, &hs[..n]).unwrap().expect("the column sees the bed");
        assert!((hit.point.z - 30.0).abs() < 1e-3, "{hit:?}");
        assert!((hit.allow - (20.0 - c.radius - c.skin)).abs() < 1e-3, "{hit:?}");
        // stairs below the column's lower end (a 0.45 m riser) and an open floor are not hits
        let mut w2 = World::flat().with(Convex::aabb(V3::new(-40.0, -1.0, 30.0), V3::new(40.0, 0.45 / 0.0254, 100.0)));
        assert_eq!(body_column(&mut b, &mut w2, p, d, &c, &hs[..n]), Ok(None));
        assert_eq!(body_column(&mut b, &mut World::flat(), p, d, &c, &hs[..n]), Ok(None));
        // prone (hull 0.8 m): no waist ray; crawling under the bed, the chest ray (0.65 m) meets its face
        let pr = Capsule { hull: 0.8 / 0.0254, step: 10.0, ..c };
        let (hp, np) = pr.heights_m5();
        assert_eq!(pr.heights_body().1, np, "prone keeps its heights");
        let sp = sweep(&mut b, &mut w, p, d, &pr, &hp[..np]);
        assert!(sp.hits.iter().any(|h| (h.point.z - 30.0).abs() < 1e-3), "{sp:?}");
        // the budget refusal is reported
        let mut b0 = RayBudget::new(0);
        assert_eq!(body_column(&mut b0, &mut w, p, d, &c, &hs[..n]), Err(()));
    }

    #[test]
    fn move_collide_layered_world_union_map_object() {
        use super::synth::Layered;
        let mut l = Layered {
            map: World::flat(),
            objects: World { solids: vec![Convex::aabb(V3::new(-10.0, 0.0, 50.0), V3::new(10.0, 60.0, 80.0))], rays: 0 },
            mode: 0,
            hits: [0; 2],
        };
        let o = V3::new(0.0, 20.0, 0.0);
        assert!((l.cast(o, V3::new(0.0, 0.0, 100.0)).unwrap().z - 50.0).abs() < 1e-4, "union sees the object");
        l.mode = 1;
        assert!(l.cast(o, V3::new(0.0, 0.0, 100.0)).is_none(), "map only: the object is invisible (run 18)");
        assert!(l.cast(o, V3::new(0.0, -30.0, 0.0)).is_some(), "map only still sees the floor");
        l.mode = 2;
        assert!(l.cast(o, V3::new(0.0, -30.0, 0.0)).is_none(), "objects only: no floor");
        assert_eq!(l.hits, [1, 1]);
        assert_eq!(l.all().solids.len(), 2);
    }

    #[test]
    fn move_collide_wall_normal_and_plane_allow() {
        // a wall along x at z = 100; moving at 45 deg into it
        let mut w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, 100.0), V3::new(500.0, 200.0, 120.0)));
        let c = Capsule { radius: 15.0, skin: 1.2, hull: 70.0, step: 18.0, walkable_y: 0.7 };
        let dir = V3::new(1.0, 0.0, 1.0).normalize().unwrap();
        let p = V3::new(0.0, 0.0, 70.0);
        let mut b = RayBudget::new(64);
        let (hs, n) = c.heights();
        let s = sweep(&mut b, &mut w, p, dir * 30.0, &c, &hs[..n]);
        assert_eq!(n, 3, "the M5 heights");
        let (hb, nb) = c.heights_body();
        assert_eq!(nb, 4, "a waist ray between the knee and the chest");
        assert!(hb[2] > hb[1] && hb[2] < hb[3]);
        let first = s.min_hit().unwrap();
        let nrm = wall_normal(&mut b, &mut w, p, dir, &first, &s, &c);
        assert!((nrm.z + 1.0).abs() < 1e-3, "{nrm:?}");
        let a = plane_allow(p + UP * first.height, dir, first.point, nrm, c.radius, c.skin).unwrap();
        let end = p + dir * a;
        assert!((100.0 - end.z - (c.radius + c.skin)).abs() < 1e-3);
    }
}
