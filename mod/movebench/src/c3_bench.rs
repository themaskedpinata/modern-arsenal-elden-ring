//! movebench C3 scenarios C21-C24 (docs/C3_DESIGN.md 1.5; named C2x because the M8 bench already uses B21-B24):
//! the real controller on synthetic geometry whose solids carry ER collision LAYERS, cast like the DLL's rays per
//! `[movement.rays] mode` with the default layer table (`collision_layers`, emu2.py):
//!
//! - C21: ramps of 20 / 30 / 40 deg (terrain layer 0x20): speed along the surface >= 90 % of flat at 20 / 30 deg and
//!   >= 70 % at 40 deg; never inside.
//! - C22: a 0.30 m root and a 0.45 m rock across the path (terrain): crossed with W alone, no stall > 0.4 s.
//! - C23: the filter: a ramp of layer 0x21 (player-solid, the map filter cannot see it) must be walked up under
//!   `player` and is walked INTO under `map` (the in-game invisible-wall / ER-blocks case); a query-only wall of layer
//!   0x1d (the camera / union filters hit it, no character collides with it) blocks under `union` (the C2 invisible
//!   wall) and is passed under `player`; a wagon of layer 0x43 stays solid under `player`.
//! - C24: the assist detector against the controller: W into a wall fires after `stall_s` (+1 tick), repeats double
//!   (0.8 / 1.6 / 2.0 s), at most `max_per_min` per minute then one `Capped`; walking on flat ground or jumping in the
//!   open (no_face refusals) never fires.

use mw2er_core::collision_layers;
use mw2er_core::move_collide::RayWorld;
use mw2er_core::move_collide::synth::{Convex, World};
use mw2er_core::move_logic::{Assist, AssistEvent, AssistIn, AssistParams};
use mw2er_core::move_sim::{Input, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::MoveData;
use mw2er_core::vec3::V3;

use crate::mantle_bench::{RATES, Report};

const M: f32 = 1.0 / 0.0254;
const FACE_Z: f32 = 40.0;

/// Solids tagged with an ER layer, cast with one or two filter layers like `HavokRays` (the nearer hit).
#[derive(Clone)]
struct LayerWorld {
    solids: Vec<(u8, Convex)>,
    /// The filter layers of the casts (one for `player` / `map`, two for `union`).
    filters: Vec<u8>,
}

impl LayerWorld {
    fn new(mode: &str) -> LayerWorld {
        let filters = match mode {
            "player" => vec![3u8],
            "map" => vec![collision_layers::layer_of(collision_layers::MAP_FILTER_WORD)],
            _ => vec![collision_layers::layer_of(collision_layers::MAP_FILTER_WORD), collision_layers::layer_of(collision_layers::CAMERA_FILTER_WORD)],
        };
        // the floor: terrain layer 0x20 (every filter above hits it)
        let floor = Convex::aabb(V3::new(-4e5, -100.0, -4e5), V3::new(4e5, 0.0, 4e5));
        LayerWorld { solids: vec![(0x20, floor)], filters }
    }

    fn with(mut self, layer: u8, c: Convex) -> LayerWorld {
        self.solids.push((layer, c));
        self
    }

    /// Every solid the PLAYER collides with (layer 3's row): the never-inside oracle.
    fn player_solid(&self) -> World {
        World { solids: self.solids.iter().filter(|(l, _)| collision_layers::collides(3, *l)).map(|(_, c)| c.clone()).collect(), rays: 0 }
    }
}

impl RayWorld for LayerWorld {
    fn cast(&mut self, origin: V3, delta: V3) -> Option<V3> {
        let mut best = f32::INFINITY;
        for f in &self.filters {
            for (l, c) in &self.solids {
                if collision_layers::collides(*f, *l)
                    && let Some(t) = c.ray(origin, delta)
                {
                    best = best.min(t);
                }
            }
        }
        best.is_finite().then(|| origin + delta * best)
    }
}

fn w_input(w: bool) -> Input {
    Input { w, fwd_h: V3::new(0.0, 0.0, 1.0), right_h: V3::new(1.0, 0.0, 0.0), ..Input::default() }
}

struct Run<'a> {
    d: &'a MoveData,
    cfg: SimCfg,
    p: Player,
    w: LayerWorld,
    oracle: World,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
    inside: Option<(f64, V3)>,
    /// The capsule AXIS inside geometry (the M8 oracle; a step-up rides the capsule's lower radius over a ledge edge).
    axis: Option<(f64, V3)>,
    rays_max: u32,
}

impl<'a> Run<'a> {
    fn new(d: &'a MoveData, w: LayerWorld, pos: V3, hz: f64) -> Run<'a> {
        let oracle = w.player_solid();
        Run { d, cfg: SimCfg::defaults(d), p: Player::new(d, pos), w, oracle, t: 0.0, hz, ev: Vec::new(), inside: None, axis: None, rays_max: 0 }
    }

    fn tick(&mut self, i: Input) -> mw2er_core::move_sim::TickStats {
        self.t += 1.0 / self.hz;
        let st = self.p.tick(self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev);
        self.rays_max = self.rays_max.max(st.rays);
        let hull = self.p.stance.hull_in(&self.d.mw2) * self.cfg.v_scale;
        if self.inside.is_none()
            && let Some(q) = self.oracle.capsule_inside(self.p.pos, hull, self.cfg.radius, 48, 0.75)
        {
            self.inside = Some((self.t, q));
        }
        if self.axis.is_none()
            && let Some(q) = self.oracle.axis_inside(self.p.pos, hull, 64, 0.05)
        {
            self.axis = Some((self.t, q));
        }
        st
    }

    /// W for `secs`; returns the longest stall (planar speed < 0.15 m/s while W is held and grounded, after the first
    /// 0.5 s of acceleration).
    fn walk(&mut self, secs: f64) -> f64 {
        let n = (secs * self.hz) as usize;
        let (mut stall, mut longest) = (0.0f64, 0.0f64);
        let mut prev = self.p.pos;
        for k in 0..n {
            self.tick(w_input(true));
            self.ev.clear();
            let v = V3::new(self.p.pos.x - prev.x, 0.0, self.p.pos.z - prev.z).len() * 0.0254 * self.hz as f32;
            prev = self.p.pos;
            if (k as f64) / self.hz > 0.5 && v < 0.15 {
                stall += 1.0 / self.hz;
                longest = longest.max(stall);
            } else {
                stall = 0.0;
            }
        }
        longest
    }
}

/// Distance along the surface walked from `a` to `b` (inches).
fn dist3(a: V3, b: V3) -> f32 {
    (b - a).len()
}

fn c21(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let id = "C21";
        // flat reference: 3 s of W after 1 s of acceleration
        let mut x = Run::new(d, LayerWorld::new("player"), V3::ZERO, hz);
        x.walk(1.0);
        let a = x.p.pos;
        x.walk(2.0);
        let flat = dist3(a, x.p.pos) / 2.0;
        for (deg, need) in [(20.0f32, 0.90f32), (30.0, 0.90), (40.0, 0.70)] {
            let ramp = Convex::ramp(-400.0, 400.0, 60.0, 0.0, deg, 40.0 * M);
            let mut x = Run::new(d, LayerWorld::new("player").with(0x20, ramp), V3::ZERO, hz);
            x.walk(1.5);
            let a = x.p.pos;
            x.walk(2.0);
            let on = dist3(a, x.p.pos) / 2.0;
            let climbed = x.p.pos.y > a.y + 10.0;
            r.check(
                id,
                hz,
                &format!("{deg} deg ramp: speed along the surface >= {:.0} % of flat, climbing, never inside", need * 100.0),
                on >= need * flat && climbed && x.inside.is_none(),
                format!("ramp {on:.1} in/s flat {flat:.1} in/s ({:.0} %) y {:.1} -> {:.1} inside {:?}", 100.0 * on / flat.max(1e-3), a.y, x.p.pos.y, x.inside),
            );
            r.check(id, hz, &format!("{deg} deg: main rays per tick <= {}", x.cfg.max_rays), x.rays_max <= x.cfg.max_rays, format!("{}", x.rays_max));
        }
    }
}

fn c22(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let id = "C22";
        for (name, h) in [("0.30 m root", 0.30f32), ("0.45 m rock", 0.45)] {
            let obstacle = Convex::aabb(V3::new(-400.0, 0.0, FACE_Z), V3::new(400.0, h * M, FACE_Z + 0.25 * M));
            let mut x = Run::new(d, LayerWorld::new("player").with(0x20, obstacle), V3::ZERO, hz);
            let stall = x.walk(4.0);
            r.check(
                id,
                hz,
                &format!("{name}: crossed with W alone (a step), no stall > 0.4 s, the axis never inside"),
                x.p.pos.z > FACE_Z + 0.25 * M + 10.0 && stall <= 0.4 && x.axis.is_none(),
                format!("z {:.1} (past {:.1}) stall {stall:.2}s axis {:?} (info: capsule-radius overlap at the edge {:?})", x.p.pos.z, FACE_Z + 0.25 * M, x.axis, x.inside),
            );
        }
    }
}

fn c23(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let id = "C23";
        // (a) a ramp on layer 0x21: walkable under player, invisible to the map filter
        let ramp21 = Convex::ramp(-400.0, 400.0, 60.0, 0.0, 25.0, 40.0 * M);
        let mut x = Run::new(d, LayerWorld::new("player").with(0x21, ramp21.clone()), V3::ZERO, hz);
        x.walk(3.0);
        r.check(id, hz, "layer 0x21 ramp under player: walked up, never inside", x.p.pos.y > 20.0 && x.inside.is_none(), format!("y {:.1} inside {:?}", x.p.pos.y, x.inside));
        let mut x = Run::new(d, LayerWorld::new("map").with(0x21, ramp21), V3::ZERO, hz);
        x.walk(3.0);
        r.check(id, hz, "layer 0x21 ramp under map (run 18): walked into (ER's proxy would block: the C2 stall)", x.inside.is_some(), format!("y {:.1} inside {:?}", x.p.pos.y, x.inside));
        // (b) a query-only wall (layer 0x1d): union blocks (the invisible wall), player passes
        let wall = Convex::aabb(V3::new(-400.0, 0.0, FACE_Z), V3::new(400.0, 3.0 * M, FACE_Z + 10.0));
        let mut x = Run::new(d, LayerWorld::new("union").with(0x1d, wall.clone()), V3::ZERO, hz);
        x.walk(3.0);
        r.check(id, hz, "layer 0x1d (query-only) wall under union: stops the player (the C2 invisible wall)", x.p.pos.z < FACE_Z, format!("z {:.1}", x.p.pos.z));
        let mut x = Run::new(d, LayerWorld::new("player").with(0x1d, wall), V3::ZERO, hz);
        x.walk(3.0);
        r.check(id, hz, "layer 0x1d wall under player: passed (no character collides with it)", x.p.pos.z > FACE_Z + 20.0, format!("z {:.1}", x.p.pos.z));
        // (c) a wagon (layer 0x43: the camera filter hits it, the map filter does not): solid under player
        let wagon = Convex::aabb(V3::new(-45.0, 0.0, FACE_Z), V3::new(45.0, 2.0 * M, FACE_Z + 3.0 * M));
        let mut x = Run::new(d, LayerWorld::new("player").with(0x43, wagon), V3::ZERO, hz);
        x.walk(3.0);
        let stop = FACE_Z - x.cfg.radius - x.cfg.skin;
        r.check(id, hz, "wagon (layer 0x43) under player: stops at it, never inside", x.p.pos.z <= stop + 0.1 && x.inside.is_none(), format!("z {:.2} stop {stop:.2} inside {:?}", x.p.pos.z, x.inside));
    }
}

fn c24(d: &MoveData, r: &mut Report) {
    let p = AssistParams::default();
    for hz in RATES {
        let id = "C24";
        // W into a wall the controller sees: stalled -> assist after stall_s (+1 tick)
        let wall = Convex::aabb(V3::new(-400.0, 0.0, FACE_Z), V3::new(400.0, 3.0 * M, FACE_Z + 10.0));
        let mut x = Run::new(d, LayerWorld::new("player").with(0x20, wall), V3::new(0.0, 0.0, FACE_Z - 30.0), hz);
        let mut a = Assist::default();
        let mut fires: Vec<(f64, f64)> = Vec::new();
        let mut capped = 0;
        let mut prev = x.p.pos;
        let mut first_t = None;
        let n = (60.0 * hz) as usize;
        let mut k = 0;
        while k < n {
            x.tick(w_input(true));
            x.ev.clear();
            let speed = V3::new(x.p.pos.x - prev.x, 0.0, x.p.pos.z - prev.z).len() * 0.0254 * hz as f32;
            prev = x.p.pos;
            let ai = AssistIn { now: x.t, wish: true, grounded: x.p.grounded, speed, refusal: false, blocked: false };
            match a.tick(&ai, &p) {
                Some(AssistEvent::Fire { hold_s, .. }) => {
                    first_t.get_or_insert(x.t);
                    fires.push((x.t, hold_s));
                    // ER drives for hold_s (the controller is suspended: time passes, the detector restarts)
                    let skip = (hold_s * hz) as usize;
                    x.t += hold_s;
                    k += skip;
                    a.reset();
                }
                Some(AssistEvent::Capped { .. }) => capped += 1,
                None => {}
            }
            k += 1;
        }
        let holds: Vec<f64> = fires.iter().map(|f| f.1).collect();
        r.check(
            id,
            hz,
            "W into a wall: assist after stall_s (+ the acceleration into it), holds 0.8 / 1.6 / 2.0 s",
            first_t.is_some_and(|t| t <= 1.5) && holds.len() >= 3 && (holds[0] - 0.8).abs() < 1e-9 && (holds[1] - 1.6).abs() < 1e-9 && (holds[2] - 2.0).abs() < 1e-9,
            format!("first {first_t:?} holds {holds:?}"),
        );
        r.check(id, hz, "at most max_per_min per minute, then one Capped", fires.len() as u32 == p.max_per_min && capped == 1, format!("{} fires, {capped} capped", fires.len()));
        // flat ground with W, and jumps in the open (no_face refusals): never
        let mut x = Run::new(d, LayerWorld::new("player"), V3::ZERO, hz);
        let mut a = Assist::default();
        let mut prev = x.p.pos;
        let mut fired = 0;
        for k in 0..(20.0 * hz) as usize {
            let jump = k % (hz as usize) == 0;
            x.tick(Input { jump_presses: jump as u32, jump_held: jump, ..w_input(true) });
            let refused = x.ev.iter().any(|e| matches!(e, SimEvent::MantleRefuse { .. }));
            x.ev.clear();
            let speed = V3::new(x.p.pos.x - prev.x, 0.0, x.p.pos.z - prev.z).len() * 0.0254 * hz as f32;
            prev = x.p.pos;
            if a.tick(&AssistIn { now: x.t, wish: true, grounded: x.p.grounded, speed, refusal: refused, blocked: false }, &p).is_some() {
                fired += 1;
            }
        }
        r.check(id, hz, "running and jumping in the open (refusals): never fires", fired == 0, format!("{fired} fires"));
    }
}

/// C21-C24; returns the report.
pub fn run_all(d: &MoveData) -> Report {
    let mut r = Report { rows: Vec::new(), fails: 0 };
    c21(d, &mut r);
    c22(d, &mut r);
    c23(d, &mut r);
    c24(d, &mut r);
    r
}

/// The number of scenarios `run_all` must report.
pub const SCENARIOS: usize = 4;
