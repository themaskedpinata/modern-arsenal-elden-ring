//! AC-130 round tracers (run 8 feature request; MW2's AC-130 shows every round as a glowing tracer from the gunship to
//! the impact). Pure: the shot ring buffer, the flight interpolation, the projection through the gunship camera and the
//! GPU instance list. The DLL draws the instances in its own D3D12 pass (`mw2er_render::tracer`) BEFORE the thermal
//! pass, so a tracer is part of the "scene" the thermal film sees: white hot in WHOT, black in BHOT, like a hot round
//! in MW2's thermal view; it gets the same grain and the gun overlay on top of it.
//!
//! - Every `AC130 SHOT` pushes one [`TracerShot`] (muzzle, impact, fire time, flight time; the numbers of the SHOT
//!   line) into a fixed ring of [`MAX_TRACERS`] (the oldest is overwritten; `Copy`, no allocation).
//! - At draw time the round's head is `lerp(muzzle, impact, t / flight_s)`, the streak runs `length_m` back along the
//!   path; after landing a short impact flash (optional) replaces it.
//! - Projection: exactly [`crate::ac130_hud::project`]'s convention: camera rows (right, up, forward) and position as
//!   written to `CSCamera.pers_cam_1`, vertical FOV, pixels with a top-left origin. The segment is clipped to
//!   [`NEAR_M`] in front of the eye first.
//! - After run 9 the impact flash is per gun (`flash_s`, `flash_gain`) and can carry a wide soft "thermal bloom"
//!   (`bloom_m`, a second flash instance at a fraction of the flash's brightness) when the instance list has room:
//!   the 105 gets the biggest, longest flash.
//! - The muzzle sits `muzzle_offset_m` (right, up, forward, camera space at fire time) from the camera, so a round
//!   streaks in from the side towards the aim point instead of being a dot on the screen centre.

use crate::ac130_hud::project;

/// Shots kept in flight (the 25 mm fires 20 in 1.4 s at 0.6 s flight: ~9 in the air; 64 leaves room for the flashes).
pub const MAX_TRACERS: usize = 64;
/// Instances per frame: one per shot (a streak while in flight, then its impact flash).
pub const MAX_INSTANCES: usize = MAX_TRACERS;
/// Near clip of the tracer segment (metres in front of the eye).
pub const NEAR_M: f32 = 0.5;
/// Bytes per GPU instance (8 floats; `tracer.hlsl` `struct Inst`).
pub const INSTANCE_BYTES: usize = 32;

/// One fired round (the SHOT line's o=, impact=, flight_s=).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracerShot {
    pub gun: u8,
    pub muzzle: [f32; 3],
    pub impact: [f32; 3],
    /// `shared::now_s()` of the shot.
    pub t_fire: f64,
    pub flight_s: f32,
}

impl TracerShot {
    pub const NONE: TracerShot = TracerShot { gun: 0, muzzle: [0.0; 3], impact: [0.0; 3], t_fire: f64::NEG_INFINITY, flight_s: 0.0 };
}

/// Fixed ring of the last [`MAX_TRACERS`] shots (`Copy`: the HUD snapshot carries it without allocating).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracerRing {
    shots: [TracerShot; MAX_TRACERS],
    next: usize,
    len: usize,
}

impl Default for TracerRing {
    fn default() -> Self {
        Self::new()
    }
}

impl TracerRing {
    pub const fn new() -> Self {
        TracerRing { shots: [TracerShot::NONE; MAX_TRACERS], next: 0, len: 0 }
    }

    /// Adds a shot; the oldest one is overwritten when full.
    pub fn push(&mut self, s: TracerShot) {
        self.shots[self.next] = s;
        self.next = (self.next + 1) % MAX_TRACERS;
        self.len = (self.len + 1).min(MAX_TRACERS);
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &TracerShot> {
        let start = (self.next + MAX_TRACERS - self.len) % MAX_TRACERS;
        (0..self.len).map(move |k| &self.shots[(start + k) % MAX_TRACERS])
    }
}

/// Look of one gun's tracer (`[ac130.tracers]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracerStyle {
    /// Full width of the bright core in metres (perspective: thinner far away).
    pub width_m: f32,
    /// Length of the streak behind the head in metres.
    pub length_m: f32,
    /// Peak added brightness (1 = the core reaches white on a black background).
    pub brightness: f32,
    /// Impact flash radius in metres (0 = none).
    pub flash_m: f32,
    /// Flash length in seconds (0 = the global `TracerParams::flash_s`).
    pub flash_s: f32,
    /// Peak flash brightness relative to `brightness` (1 = as bright as the round).
    pub flash_gain: f32,
    /// Radius (m) of a wide soft bloom drawn around the flash (0 = none).
    pub bloom_m: f32,
}

/// Brightness of the bloom relative to the flash.
pub const BLOOM_GAIN: f32 = 0.35;

impl TracerStyle {
    /// The run-8 look (no per-gun flash length, gain 1, no bloom).
    pub const fn basic(width_m: f32, length_m: f32, brightness: f32, flash_m: f32) -> TracerStyle {
        TracerStyle { width_m, length_m, brightness, flash_m, flash_s: 0.0, flash_gain: 1.0, bloom_m: 0.0 }
    }

    /// The flash length of this gun (its own, else the global one).
    pub fn flash_len(&self, global_s: f32) -> f32 {
        if self.flash_s > 0.0 { self.flash_s } else { global_s }
    }
}

/// The view the tracers are projected through (the gunship camera write).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracerView {
    pub pos: [f32; 3],
    /// Rows right, up, forward (as written to pers_cam_1).
    pub rows: [[f32; 3]; 3],
    /// Vertical FOV, radians.
    pub vfov: f32,
    pub width: f32,
    pub height: f32,
}

/// Drawing parameters (all from `[ac130.tracers]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracerParams {
    pub styles: [TracerStyle; 3],
    pub flash: bool,
    pub flash_s: f32,
    /// Smallest half width in pixels at 1080p (scaled with the height), so a far 25 mm round never vanishes.
    pub min_half_px: f32,
    /// Brightness at the tail end of the streak (head = 1).
    pub tail: f32,
}

/// One GPU instance: a screen-space capsule (`kind` 0) from `p0` (tail) to `p1` (head), or a round flash (`kind` 1,
/// `p0 == p1`). Layout = `tracer.hlsl` `struct Inst` (8 floats, [`INSTANCE_BYTES`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct TracerInstance {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub half_w: f32,
    pub intensity: f32,
    pub tail: f32,
    pub kind: f32,
}

impl TracerInstance {
    pub fn floats(&self) -> [f32; 8] {
        [self.p0[0], self.p0[1], self.p1[0], self.p1[1], self.half_w, self.intensity, self.tail, self.kind]
    }
}

/// The muzzle of a shot: `cam_pos + right * o.x + up * o.y + forward * o.z`.
pub fn muzzle(cam_pos: [f32; 3], rows: [[f32; 3]; 3], offset_m: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| cam_pos[i] + rows[0][i] * offset_m[0] + rows[1][i] * offset_m[1] + rows[2][i] * offset_m[2])
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn depth(v: &TracerView, p: [f32; 3]) -> f32 {
    let d = [p[0] - v.pos[0], p[1] - v.pos[1], p[2] - v.pos[2]];
    v.rows[2][0] * d[0] + v.rows[2][1] * d[1] + v.rows[2][2] * d[2]
}

/// Pixels per metre at depth `z` (vertical focal length / z).
fn px_per_m(v: &TracerView, z: f32) -> f32 {
    v.height * 0.5 / (v.vfov * 0.5).tan() / z
}

/// The visible part of the world segment `a` -> `b` (clipped to [`NEAR_M`]) as pixels plus the depths of its ends.
pub fn project_segment(v: &TracerView, a: [f32; 3], b: [f32; 3]) -> Option<([f32; 2], [f32; 2], f32, f32)> {
    let (za, zb) = (depth(v, a), depth(v, b));
    if za < NEAR_M && zb < NEAR_M {
        return None;
    }
    let (mut a, mut b) = (a, b);
    if za < NEAR_M {
        a = lerp3(a, b, (NEAR_M - za) / (zb - za));
    } else if zb < NEAR_M {
        b = lerp3(b, a, (NEAR_M - zb) / (za - zb));
    }
    let pa = project(v.pos, v.rows, v.vfov, v.width, v.height, a)?;
    let pb = project(v.pos, v.rows, v.vfov, v.width, v.height, b)?;
    Some((pa, pb, depth(v, a), depth(v, b)))
}

/// Fills `out` with this frame's instances and returns how many (never more than `out.len()`; no allocation).
pub fn build_instances(ring: &TracerRing, now: f64, v: &TracerView, p: &TracerParams, out: &mut [TracerInstance]) -> usize {
    let mut n = 0;
    if !(v.vfov > 0.0 && v.width > 0.0 && v.height > 0.0) {
        return 0;
    }
    let min_half = p.min_half_px * v.height / 1080.0;
    // a quad far outside the screen is skipped (it would only cost fill rate on the guard band)
    let margin = v.width.max(v.height);
    let on_screen = |q: [f32; 2]| q[0] > -margin && q[0] < v.width + margin && q[1] > -margin && q[1] < v.height + margin;
    for s in ring.iter() {
        if n >= out.len() {
            break;
        }
        let st = p.styles[(s.gun as usize).min(2)];
        let age = (now - s.t_fire) as f32;
        let flight = s.flight_s.max(1e-3);
        if !(age >= 0.0) {
            continue;
        }
        if age < flight {
            if st.brightness <= 0.0 || st.width_m <= 0.0 {
                continue;
            }
            let dist = {
                let d = [s.impact[0] - s.muzzle[0], s.impact[1] - s.muzzle[1], s.impact[2] - s.muzzle[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3)
            };
            let f_head = age / flight;
            let f_tail = (f_head - st.length_m / dist).max(0.0);
            let head = lerp3(s.muzzle, s.impact, f_head);
            let tail = lerp3(s.muzzle, s.impact, f_tail);
            let Some((pt, ph, _zt, zh)) = project_segment(v, tail, head) else { continue };
            if !(on_screen(pt) || on_screen(ph)) {
                continue;
            }
            let half = (0.5 * st.width_m * px_per_m(v, zh)).max(min_half);
            out[n] = TracerInstance { p0: pt, p1: ph, half_w: half, intensity: st.brightness, tail: p.tail, kind: 0.0 };
            n += 1;
        } else if p.flash && st.flash_m > 0.0 && st.flash_len(p.flash_s) > 0.0 {
            let t = (age - flight) / st.flash_len(p.flash_s);
            if t >= 1.0 {
                continue;
            }
            let z = depth(v, s.impact);
            if z < NEAR_M {
                continue;
            }
            let Some(c) = project(v.pos, v.rows, v.vfov, v.width, v.height, s.impact) else { continue };
            if !on_screen(c) {
                continue;
            }
            let k = 1.0 - t;
            let ppm = px_per_m(v, z);
            let r = (st.flash_m * ppm * (0.6 + 0.4 * t)).max(min_half);
            let peak = st.brightness * st.flash_gain.max(0.0);
            out[n] = TracerInstance { p0: c, p1: c, half_w: r, intensity: peak * k * k, tail: 1.0, kind: 1.0 };
            n += 1;
            // the bloom: a wide soft glow that grows while it fades (only when the list still has room)
            if st.bloom_m > 0.0 && n < out.len() {
                let rb = (st.bloom_m * ppm * (0.7 + 0.3 * t)).max(r);
                out[n] = TracerInstance { p0: c, p1: c, half_w: rb, intensity: peak * BLOOM_GAIN * k, tail: 1.0, kind: 1.0 };
                n += 1;
            }
        }
    }
    n
}

/// CPU reference of `tracer.hlsl`'s pixel shader: the added brightness of instance `t` at pixel centre `q`.
pub fn shade(t: &TracerInstance, q: [f32; 2]) -> f32 {
    let d = [t.p1[0] - t.p0[0], t.p1[1] - t.p0[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let ax = if len > 1e-3 { [d[0] / len, d[1] / len] } else { [1.0, 0.0] };
    let r = [q[0] - t.p0[0], q[1] - t.p0[1]];
    let u = r[0] * ax[0] + r[1] * ax[1];
    let w = -r[0] * ax[1] + r[1] * ax[0];
    let s = u.clamp(0.0, len);
    let dist = ((u - s) * (u - s) + w * w).sqrt();
    let hw = t.half_w.max(1e-3);
    let core = (1.0 - dist / hw).clamp(0.0, 1.0);
    let glow = (-(dist * dist) / (2.0 * hw * hw)).exp();
    let along = if len > 1e-3 { t.tail + (1.0 - t.tail) * (s / len) } else { 1.0 };
    (core * core + 0.45 * glow) * along * t.intensity
}

/// Half extent of an instance's quad around the segment (pixels; the glow is < 1/256 beyond it).
pub fn quad_margin(half_w: f32) -> f32 {
    3.5 * half_w
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: TracerView = TracerView {
        pos: [0.0, 100.0, 0.0],
        // looking straight down: right = +x, up = +z, forward = -y
        rows: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
        vfov: 0.6,
        width: 1920.0,
        height: 1080.0,
    };

    fn params() -> TracerParams {
        TracerParams {
            styles: [
                TracerStyle::basic(1.2, 14.0, 1.0, 6.0),
                TracerStyle::basic(0.6, 10.0, 0.9, 2.5),
                TracerStyle::basic(0.25, 22.0, 0.8, 0.8),
            ],
            flash: true,
            flash_s: 0.25,
            min_half_px: 1.0,
            tail: 0.15,
        }
    }

    fn shot(gun: u8, t: f64) -> TracerShot {
        TracerShot { gun, muzzle: muzzle(VIEW.pos, VIEW.rows, [-1.5, -2.5, 0.0]), impact: [10.0, 0.0, 5.0], t_fire: t, flight_s: 2.0 }
    }

    #[test]
    fn ac130_tracer_ring_overwrites_oldest() {
        let mut r = TracerRing::new();
        assert!(r.is_empty());
        for k in 0..(MAX_TRACERS + 5) {
            r.push(TracerShot { t_fire: k as f64, ..TracerShot::NONE });
        }
        assert_eq!(r.len(), MAX_TRACERS);
        let ts: Vec<f64> = r.iter().map(|s| s.t_fire).collect();
        assert_eq!(ts.first(), Some(&5.0));
        assert_eq!(ts.last(), Some(&((MAX_TRACERS + 4) as f64)));
        assert!(ts.windows(2).all(|w| w[1] == w[0] + 1.0), "oldest first");
        r.clear();
        assert_eq!(r.iter().count(), 0);
        // Copy: the snapshot can carry it without allocating
        let a = r;
        let _b = a;
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn ac130_tracer_projection_matches_hud_project() {
        // a point 3 m right and 2 m "up" (screen) at 100 m depth
        let p = [3.0, 0.0, 2.0];
        let c = project(VIEW.pos, VIEW.rows, VIEW.vfov, VIEW.width, VIEW.height, p).unwrap();
        let fy = 540.0 / (0.3f32).tan();
        assert!((c[0] - (960.0 + 3.0 / 100.0 * fy)).abs() < 1e-3, "{c:?}");
        assert!((c[1] - (540.0 - 2.0 / 100.0 * fy)).abs() < 1e-3, "{c:?}");
        let (a, b, za, zb) = project_segment(&VIEW, p, p).unwrap();
        assert_eq!((a, b), (c, c));
        assert!((za - 100.0).abs() < 1e-4 && (zb - 100.0).abs() < 1e-4);
    }

    #[test]
    fn ac130_tracer_head_moves_over_flight_time() {
        let mut r = TracerRing::new();
        r.push(shot(0, 10.0));
        let mut out = [TracerInstance::default(); MAX_INSTANCES];
        let p = params();
        assert_eq!(build_instances(&r, 9.9, &VIEW, &p, &mut out), 0, "not fired yet");
        let n = build_instances(&r, 11.0, &VIEW, &p, &mut out);
        assert_eq!(n, 1);
        let s = shot(0, 10.0);
        let head = lerp3(s.muzzle, s.impact, 0.5);
        let want = project(VIEW.pos, VIEW.rows, VIEW.vfov, VIEW.width, VIEW.height, head).unwrap();
        assert!((out[0].p1[0] - want[0]).abs() < 1e-2 && (out[0].p1[1] - want[1]).abs() < 1e-2, "{:?} {want:?}", out[0]);
        assert_eq!(out[0].kind, 0.0);
        // just before landing the head is at the impact; the streak trails behind (towards the muzzle side)
        let n = build_instances(&r, 11.999, &VIEW, &p, &mut out);
        let imp = project(VIEW.pos, VIEW.rows, VIEW.vfov, VIEW.width, VIEW.height, s.impact).unwrap();
        assert_eq!(n, 1);
        assert!((out[0].p1[0] - imp[0]).abs() < 0.5 && (out[0].p1[1] - imp[1]).abs() < 0.5);
        assert!(out[0].p0 != out[0].p1, "a streak, not a dot");
        // landed: the flash, fading, then nothing
        let n = build_instances(&r, 12.05, &VIEW, &p, &mut out);
        assert_eq!((n, out[0].kind), (1, 1.0));
        assert!((out[0].p0[0] - imp[0]).abs() < 1e-3);
        let i1 = out[0].intensity;
        build_instances(&r, 12.2, &VIEW, &p, &mut out);
        assert!(out[0].intensity < i1);
        assert_eq!(build_instances(&r, 12.3, &VIEW, &p, &mut out), 0);
        let no_flash = TracerParams { flash: false, ..p };
        assert_eq!(build_instances(&r, 12.05, &VIEW, &no_flash, &mut out), 0);
    }

    #[test]
    fn ac130_tracer_widths_per_gun_and_min_px() {
        let mut r = TracerRing::new();
        for g in 0..3 {
            r.push(shot(g, 0.0));
        }
        let mut out = [TracerInstance::default(); MAX_INSTANCES];
        let n = build_instances(&r, 1.9, &VIEW, &params(), &mut out);
        assert_eq!(n, 3);
        assert!(out[0].half_w > out[1].half_w && out[1].half_w > out[2].half_w, "105 > 40 > 25");
        let thin = TracerParams { min_half_px: 4.0, ..params() };
        build_instances(&r, 1.9, &VIEW, &thin, &mut out);
        assert!(out[2].half_w >= 4.0);
        // the streak length follows length_m: the 25 mm streak is the longest on screen
        let l = |t: &TracerInstance| ((t.p1[0] - t.p0[0]).powi(2) + (t.p1[1] - t.p0[1]).powi(2)).sqrt();
        build_instances(&r, 1.9, &VIEW, &params(), &mut out);
        assert!(l(&out[2]) > l(&out[0]));
    }

    #[test]
    fn ac130_tracer_near_clip_and_capacity() {
        // a shot from behind the camera to the ground: clipped to NEAR_M, still drawn
        let mut r = TracerRing::new();
        r.push(TracerShot { gun: 0, muzzle: [0.0, 105.0, 0.0], impact: [0.0, 0.0, 0.0], t_fire: 0.0, flight_s: 1.0 });
        let mut out = [TracerInstance::default(); MAX_INSTANCES];
        assert_eq!(build_instances(&r, 0.01, &VIEW, &params(), &mut out), 0, "head still behind the eye");
        assert_eq!(build_instances(&r, 0.5, &VIEW, &params(), &mut out), 1);
        // a full ring never writes past `out`
        let mut r = TracerRing::new();
        for _ in 0..MAX_TRACERS {
            r.push(shot(2, 0.0));
        }
        let mut small = [TracerInstance::default(); 10];
        assert_eq!(build_instances(&r, 1.0, &VIEW, &params(), &mut small), 10);
        assert_eq!(build_instances(&r, 1.0, &VIEW, &params(), &mut out), MAX_TRACERS);
        // a broken view draws nothing
        let bad = TracerView { vfov: 0.0, ..VIEW };
        assert_eq!(build_instances(&r, 1.0, &bad, &params(), &mut out), 0);
    }

    #[test]
    fn ac130_tracer_105_flash_bigger_longer_with_bloom() {
        // the run-9 config defaults: 105 flash 14 m / 0.5 s / gain 1.6 + 32 m bloom; 40 2.5 m / 0.3 s + 7 m bloom
        let cfg = crate::config::Ac130Tracers::default().params();
        let mut r = TracerRing::new();
        r.push(shot(0, 0.0));
        r.push(shot(1, 0.0));
        let mut out = [TracerInstance::default(); MAX_INSTANCES];
        // 0.05 s after landing (flight 2 s): 105 flash + bloom, 40 flash + bloom
        let n = build_instances(&r, 2.05, &VIEW, &cfg, &mut out);
        assert_eq!(n, 4);
        let (f105, b105, f40) = (out[0], out[1], out[2]);
        assert!(f105.kind == 1.0 && b105.kind == 1.0);
        assert!(f105.half_w > 4.0 * f40.half_w, "105 flash much bigger: {} vs {}", f105.half_w, f40.half_w);
        assert!(f105.intensity > 1.0 && f105.intensity > f40.intensity);
        assert!(b105.half_w > 2.0 * f105.half_w && b105.intensity < f105.intensity);
        // 0.4 s after landing: the 40's flash is over (0.3 s), the 105's still burns (0.5 s)
        let n = build_instances(&r, 2.4, &VIEW, &cfg, &mut out);
        assert_eq!(n, 2);
        assert_eq!(build_instances(&r, 2.51, &VIEW, &cfg, &mut out), 0);
        // flash_s 0 = the global length; the bloom is skipped when the list is full
        let st = TracerStyle::basic(1.0, 1.0, 1.0, 1.0);
        assert_eq!(st.flash_len(0.25), 0.25);
        let mut one = [TracerInstance::default(); 1];
        assert_eq!(build_instances(&r, 2.05, &VIEW, &cfg, &mut one), 1);
        assert_eq!(one[0], f105);
        // flash off: nothing after landing
        assert_eq!(build_instances(&r, 2.05, &VIEW, &TracerParams { flash: false, ..cfg }, &mut out), 0);
    }

    #[test]
    fn ac130_tracer_shade_profile() {
        let t = TracerInstance { p0: [100.0, 100.0], p1: [200.0, 100.0], half_w: 4.0, intensity: 1.0, tail: 0.2, kind: 0.0 };
        let head = shade(&t, [200.0, 100.0]);
        let tail = shade(&t, [100.0, 100.0]);
        assert!((head - 1.45).abs() < 1e-4, "core 1 + glow 0.45 at the head: {head}");
        assert!((tail - 0.2 * 1.45).abs() < 1e-4, "{tail}");
        assert!(shade(&t, [150.0, 102.0]) < shade(&t, [150.0, 100.0]), "falls off across");
        assert!(shade(&t, [150.0, 100.0 + quad_margin(4.0)]) < 1.0 / 256.0, "nothing visible outside the quad");
        let f = TracerInstance { p0: [50.0, 50.0], p1: [50.0, 50.0], half_w: 10.0, intensity: 0.5, tail: 1.0, kind: 1.0 };
        assert!((shade(&f, [50.0, 50.0]) - 0.5 * 1.45).abs() < 1e-4);
        assert_eq!(std::mem::size_of::<TracerInstance>(), INSTANCE_BYTES);
    }
}
