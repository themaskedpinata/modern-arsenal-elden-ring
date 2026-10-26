//! AC-130 shot path, "Route A" (docs/M4_DESIGN.md section 7.3), and the attribution of HP drops to AC-130
//! explosions. Pure.
//!
//! A shot is a ray from the gunship camera along its forward: the map ray (the DLL's `havok::map_ray`) and a
//! ray-vs-capsule test against nearby characters; the nearer hit wins. The capsule test matters for the 25 mm: at
//! these angles a ground-only ray passes through the enemy and lands about a metre behind its feet. The shell then
//! flies for `dist / projectileSpeed` (MW2) before the explosion rows are spawned at the impact.

use crate::hit_logic::dist_to_axis;
use crate::vec3::V3;

/// Distance along the ray `o + t d` (`d` unit) to the first point within `radius` of the vertical segment
/// `base .. base + (0, height, 0)` (a capsule), or `None`. A ray starting inside returns 0.
pub fn ray_capsule(o: V3, d: V3, base: V3, radius: f32, height: f32) -> Option<f32> {
    if !(radius > 0.0 && o.is_finite() && d.is_finite() && base.is_finite()) {
        return None;
    }
    let top = base + V3::new(0.0, height.max(0.0), 0.0);
    if dist_to_axis(&[o.x, o.y, o.z], &[base.x, base.y, base.z], height) <= radius {
        return Some(0.0);
    }
    let mut best: Option<f32> = None;
    let mut take = |t: f32| {
        if t >= 0.0 && t.is_finite() && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    };
    // infinite vertical cylinder, restricted to the segment's height
    let (ox, oz) = (o.x - base.x, o.z - base.z);
    let a = d.x * d.x + d.z * d.z;
    if a > 1e-12 {
        let b = 2.0 * (ox * d.x + oz * d.z);
        let c = ox * ox + oz * oz - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let s = disc.sqrt();
            for t in [(-b - s) / (2.0 * a), (-b + s) / (2.0 * a)] {
                let y = o.y + t * d.y;
                if y >= base.y && y <= top.y {
                    take(t);
                }
            }
        }
    }
    // the two end spheres
    for centre in [base, top] {
        let oc = o - centre;
        let b = oc.dot(d);
        let c = oc.dot(oc) - radius * radius;
        let disc = b * b - c;
        if disc >= 0.0 {
            take(-b - disc.sqrt());
        }
    }
    best
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Via {
    Map,
    Chr(usize),
    None,
}

impl Via {
    pub fn label(self) -> &'static str {
        match self {
            Via::Map => "map",
            Via::Chr(_) => "chr",
            Via::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimHit {
    pub t: f32,
    pub via: Via,
}

/// The nearer of the map hit `map_t` and the first character capsule (feet positions `chars`) within `max_t`.
pub fn first_hit(o: V3, d: V3, map_t: Option<f32>, chars: &[V3], radius: f32, height: f32, max_t: f32) -> AimHit {
    let mut best = AimHit { t: f32::INFINITY, via: Via::None };
    if let Some(t) = map_t.filter(|t| t.is_finite() && *t >= 0.0 && *t <= max_t) {
        best = AimHit { t, via: Via::Map };
    }
    for (i, c) in chars.iter().enumerate() {
        if let Some(t) = ray_capsule(o, d, *c, radius, height).filter(|t| *t <= max_t)
            && t < best.t
        {
            best = AimHit { t, via: Via::Chr(i) };
        }
    }
    if best.via == Via::None { AimHit { t: 0.0, via: Via::None } } else { best }
}

/// MW2 shell flight time: `dist / projectileSpeed` (both metres / metres per second).
pub fn flight_time(dist_m: f32, speed_mps: f32) -> f32 {
    if speed_mps > 0.0 && dist_m.is_finite() { (dist_m / speed_mps).max(0.0) } else { 0.0 }
}

/// One spawned AC-130 explosion, kept for HIT attribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blast {
    pub t: f64,
    pub pos: V3,
    pub radius_m: f32,
    pub gun: usize,
    /// 0 = core, 1 = ring, 2 = single.
    pub tier: u8,
}

/// An HP drop belongs to the AC-130 when an explosion landed in `[t_drop - before_s, t_drop + after_s]` and its
/// sphere (+ `margin_m`) reaches the character's capsule axis. The explosion nearest in time wins (ties: the nearer
/// one). Returns the blast index and the distance from the blast to the axis.
pub fn attribute(chr_feet: V3, t_drop: f64, blasts: &[Blast], capsule_h: f32, before_s: f64, after_s: f64, margin_m: f32) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f64, f32)> = None;
    for (i, b) in blasts.iter().enumerate() {
        let dt = b.t - t_drop;
        if !(-before_s - 1e-9..=after_s + 1e-9).contains(&dt) {
            continue;
        }
        let d = dist_to_axis(&[b.pos.x, b.pos.y, b.pos.z], &[chr_feet.x, chr_feet.y, chr_feet.z], capsule_h);
        if !(d.is_finite() && d <= b.radius_m + margin_m) {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, bdt, bd)) => dt.abs() < bdt - 1e-9 || (dt.abs() <= bdt + 1e-9 && d < bd),
        };
        if better {
            best = Some((i, dt.abs(), d));
        }
    }
    best.map(|(i, _, d)| (i, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac130_aim_ray_capsule_hit_miss_graze() {
        let down = V3::new(0.0, -1.0, 0.0);
        // straight down onto a 2.2 m capsule at the origin: hits the top sphere at y = 2.2 + 0.75
        let t = ray_capsule(V3::new(0.0, 100.0, 0.0), down, V3::ZERO, 0.75, 2.2).unwrap();
        assert!((t - (100.0 - 2.95)).abs() < 1e-3, "{t}");
        // a diagonal ray hitting the side
        let d = V3::new(1.0, -1.0, 0.0).normalize().unwrap();
        let t = ray_capsule(V3::new(-10.0, 11.0, 0.0), d, V3::ZERO, 0.75, 2.2).unwrap();
        let p = V3::new(-10.0, 11.0, 0.0) + d * t;
        assert!(dist_to_axis(&[p.x, p.y, p.z], &[0.0, 0.0, 0.0], 2.2) <= 0.75 + 1e-3);
        // miss by 0.8 m sideways
        assert!(ray_capsule(V3::new(0.8, 100.0, 0.0), down, V3::ZERO, 0.75, 2.2).is_none());
        // graze: 0.749 m sideways still hits
        assert!(ray_capsule(V3::new(0.749, 100.0, 0.0), down, V3::ZERO, 0.75, 2.2).is_some());
        // behind the ray origin: no hit
        assert!(ray_capsule(V3::new(0.0, -10.0, 0.0), down, V3::ZERO, 0.75, 2.2).is_none());
        // starting inside
        assert_eq!(ray_capsule(V3::new(0.0, 1.0, 0.0), down, V3::ZERO, 0.75, 2.2), Some(0.0));
    }

    #[test]
    fn ac130_aim_nearest_of_map_vs_chr() {
        let o = V3::new(0.0, 100.0, -100.0);
        let target = V3::new(0.0, 0.0, 0.0);
        let d = (target + V3::new(0.0, 1.0, 0.0) - o).normalize().unwrap();
        let ground_t = 140.0;
        // the enemy is hit before the ground behind it
        let h = first_hit(o, d, Some(ground_t), &[V3::new(50.0, 0.0, 0.0), target], 0.75, 2.2, 600.0);
        assert_eq!(h.via, Via::Chr(1));
        assert!(h.t < ground_t);
        // a wall in front wins
        let h = first_hit(o, d, Some(20.0), &[target], 0.75, 2.2, 600.0);
        assert_eq!(h.via, Via::Map);
        assert_eq!(h.t, 20.0);
        assert_eq!(Via::Chr(3).label(), "chr");
    }

    #[test]
    fn ac130_aim_via_none() {
        let h = first_hit(V3::new(0.0, 100.0, 0.0), V3::new(0.0, 1.0, 0.0), None, &[V3::ZERO], 0.75, 2.2, 600.0);
        assert_eq!(h.via, Via::None);
        let h = first_hit(V3::new(0.0, 100.0, 0.0), V3::new(0.0, -1.0, 0.0), Some(900.0), &[], 0.75, 2.2, 600.0);
        assert_eq!(h.via, Via::None, "beyond max_ray_m");
        assert_eq!(Via::None.label(), "none");
    }

    #[test]
    fn ac130_aim_flight_time() {
        assert!((flight_time(157.8, 76.2) - 2.0709).abs() < 1e-3, "105");
        assert!((flight_time(157.8, 127.0) - 1.2425).abs() < 1e-3, "40");
        assert!((flight_time(157.8, 254.0) - 0.6213).abs() < 1e-3, "25");
        assert_eq!(flight_time(100.0, 0.0), 0.0);
        assert_eq!(flight_time(f32::NAN, 10.0), 0.0);
    }

    #[test]
    fn ac130_aim_attribution() {
        let blasts = [
            Blast { t: 10.0, pos: V3::new(0.0, 0.0, 0.0), radius_m: 15.24, gun: 0, tier: 1 },
            Blast { t: 10.0, pos: V3::new(0.0, 0.0, 0.0), radius_m: 7.62, gun: 0, tier: 0 },
            Blast { t: 20.0, pos: V3::new(100.0, 0.0, 0.0), radius_m: 0.81, gun: 2, tier: 2 },
        ];
        // a drop 0.05 s after, 5 m away: the nearer-in-time tie goes to the closer sphere (both 5 m: first one)
        let a = attribute(V3::new(5.0, 0.0, 0.0), 10.05, &blasts, 2.2, 0.6, 0.15, 0.5).unwrap();
        assert!(a.0 <= 1 && (a.1 - 5.0).abs() < 1e-4);
        // 30 m away: not ours
        assert!(attribute(V3::new(30.0, 0.0, 0.0), 10.05, &blasts, 2.2, 0.6, 0.15, 0.5).is_none());
        // too late
        assert!(attribute(V3::new(5.0, 0.0, 0.0), 11.0, &blasts, 2.2, 0.6, 0.15, 0.5).is_none());
        // the 25 mm one: 1 m away is within 0.81 + 0.5
        assert_eq!(attribute(V3::new(101.0, 0.0, 0.0), 20.02, &blasts, 2.2, 0.6, 0.15, 0.5).map(|x| x.0), Some(2));
    }
}
