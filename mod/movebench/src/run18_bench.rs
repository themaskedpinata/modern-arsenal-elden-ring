//! movebench run-18 scenarios (B25, B26): the first in-game test of M8 got the player stuck inside a wagon.
//!
//! - B25: an ER asset (a flatbed cart, crates) lives on another Havok collision layer than the map: with the M8 ray
//!   filter (map only) the controller walks into it; with `[movement.rays] mode = "union"` it stops at the cart,
//!   mantles onto its bed, a crate, refuses a 2.3 m box; the body probes (waist ray + front column) stop a slab
//!   between the knee and chest rays even on the map layer; prone stops at the bed too.
//! - B26: stuck inside an object (the axis in it, ER pushing the proxy out on ~15 % of the ticks): the unstick
//!   detector hands back within its window + 1 tick, the holds double (1 / 2 / 4 / 8 s), at most `max_per_window`
//!   within the backoff window, a `Capped` rest; pressing W into a wall or the cart never fires.

use mw2er_core::mantle::{MantleKind, Refuse};
use mw2er_core::move_collide::synth::{Convex, Layered, World};
use mw2er_core::move_logic::{Unstick, UnstickEvent, UnstickIn, UnstickParams};
use mw2er_core::move_sim::{Input, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::MoveData;
use mw2er_core::stance::StanceKind;
use mw2er_core::vec3::V3;

use crate::mantle_bench::{RATES, Report};

const M: f32 = 1.0 / 0.0254;
const FACE_Z: f32 = 40.0;

/// A flatbed cart: the bed 0.62-0.95 m high, 2.3 m wide, 3 m long (between the knee ray at 0.51 m and the chest ray).
fn cart() -> Convex {
    Convex::aabb(V3::new(-45.0, 0.62 * M, FACE_Z), V3::new(45.0, 0.95 * M, FACE_Z + 3.0 * M))
}

fn bx(h_m: f32) -> Convex {
    Convex::aabb(V3::new(-30.0, 0.0, FACE_Z), V3::new(30.0, h_m * M, FACE_Z + 40.0))
}

/// The map: a floor only; the objects: `objs` (ER assets).
fn layered(objs: Vec<Convex>, mode: u8) -> Layered {
    Layered { map: World::flat(), objects: World { solids: objs, rays: 0 }, mode, hits: [0; 2] }
}

fn keys(w: bool, held: bool, press: bool) -> Input {
    Input { w, jump_held: held, jump_presses: press as u32, fwd_h: V3::new(0.0, 0.0, 1.0), right_h: V3::new(1.0, 0.0, 0.0), ..Input::default() }
}

struct Run<'a> {
    d: &'a MoveData,
    cfg: SimCfg,
    p: Player,
    w: Layered,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
    /// The oracle world (both layers) and the first inside points.
    all: World,
    inside: Option<(f64, V3)>,
    capsule: Option<(f64, V3)>,
    rays_max: u32,
}

impl<'a> Run<'a> {
    fn new(d: &'a MoveData, w: Layered, pos: V3, hz: f64) -> Run<'a> {
        let all = w.all();
        Run { d, cfg: SimCfg::defaults(d), p: Player::new(d, pos), w, t: 0.0, hz, ev: Vec::new(), all, inside: None, capsule: None, rays_max: 0 }
    }

    fn tick(&mut self, i: Input) -> mw2er_core::move_sim::TickStats {
        self.t += 1.0 / self.hz;
        let st = self.p.tick(self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev);
        self.rays_max = self.rays_max.max(st.rays);
        let hull = self.p.stance.hull_in(&self.d.mw2) * self.cfg.v_scale;
        if self.inside.is_none()
            && let Some(q) = self.all.axis_inside(self.p.pos, hull, 64, 0.05)
        {
            self.inside = Some((self.t, q));
        }
        if self.capsule.is_none()
            && let Some(q) = self.all.capsule_inside(self.p.pos, hull, self.cfg.radius, 48, 0.75)
        {
            self.capsule = Some((self.t, q));
        }
        st
    }

    fn take(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.ev)
    }

    fn walk(&mut self, secs: f64, i: Input) {
        let n = (secs * self.hz) as usize;
        for _ in 0..n {
            self.tick(i);
        }
    }

    /// W + jump (pressed once, then held) for `secs`: (start kind, end y, refusals).
    fn mantle(&mut self, secs: f64) -> (Option<MantleKind>, Option<f32>, Vec<Refuse>) {
        let mut kind = None;
        let mut end = None;
        let mut refusals = Vec::new();
        let n = (secs * self.hz) as usize;
        for k in 0..n {
            self.tick(keys(true, true, k == 0));
            for e in self.take() {
                match e {
                    SimEvent::MantleStart(m) => kind = kind.or(Some(m.kind)),
                    SimEvent::MantleEnd { pos, .. } => end = Some(pos.y),
                    SimEvent::MantleRefuse { why, .. } => refusals.push(why),
                    _ => {}
                }
            }
            if end.is_some() && self.p.grounded {
                break;
            }
        }
        (kind, end, refusals)
    }
}

fn b25(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let id = "B25";
        // (a) the M8 filter (map only): the cart is invisible and the capsule walks into it (run 18)
        let mut x = Run::new(d, layered(vec![cart()], 1), V3::ZERO, hz);
        x.walk(3.0, keys(true, false, false));
        r.check(id, hz, "map filter only (M8): walks into the cart (run 18 reproduced)", x.inside.is_some() || x.capsule.is_some(), format!("inside {:?} capsule {:?} z {:.1}", x.inside, x.capsule, x.p.pos.z));
        // (b) union: stops at the cart, never inside
        let mut x = Run::new(d, layered(vec![cart()], 0), V3::ZERO, hz);
        x.walk(3.0, keys(true, false, false));
        let stop = FACE_Z - x.cfg.radius - x.cfg.skin;
        r.check(id, hz, "union filters: stops at the cart's bed", x.inside.is_none() && x.capsule.is_none() && x.p.pos.z <= stop + 0.1 && x.p.pos.z > stop - 3.0, format!("z {:.2} (stop {:.2}) inside {:?} capsule {:?} obj_hits {}", x.p.pos.z, stop, x.inside, x.capsule, x.w.hits[1]));
        r.check(id, hz, &format!("main rays per tick <= {}", x.cfg.max_rays), x.rays_max <= x.cfg.max_rays, format!("{}", x.rays_max));
        // (c) union + jump: mantles onto the bed (0.95 m)
        let mut x = Run::new(d, layered(vec![cart()], 0), V3::ZERO, hz);
        let (kind, end, refusals) = x.mantle(3.0);
        r.check(id, hz, "union: jump at the cart mantles onto its bed (0.95 m)", kind.is_some() && end.is_some_and(|y| (y - 0.95 * M).abs() <= 0.1) && x.p.grounded, format!("{kind:?} end {end:?} refusals {refusals:?}"));
        r.check(id, hz, "union: never inside the cart (axis / capsule radius)", x.inside.is_none() && x.capsule.is_none(), format!("{:?} {:?}", x.inside, x.capsule));
        // (d) chest-high (1.3 m) and head-high (1.75 m) crates mantle; a 2.3 m box is refused, no climb
        for (h, ok_want) in [(1.3f32, true), (1.75, true), (2.3, false)] {
            let mut x = Run::new(d, layered(vec![bx(h)], 0), V3::ZERO, hz);
            let (kind, end, refusals) = x.mantle(3.0);
            if ok_want {
                r.check(id, hz, &format!("{h} m crate (object layer): mantled on top"), kind.is_some() && end.is_some_and(|y| (y - h * M).abs() <= 0.1), format!("{kind:?} {end:?} {refusals:?}"));
            } else {
                r.check(id, hz, &format!("{h} m box: refused too_high, no climb"), kind.is_none() && refusals.contains(&Refuse::TooHigh) && x.p.pos.y < 1.0 * M, format!("{kind:?} {refusals:?} y {:.1}", x.p.pos.y));
            }
            r.check(id, hz, &format!("{h} m: never inside"), x.inside.is_none() && x.capsule.is_none(), format!("{:?} {:?}", x.inside, x.capsule));
        }
        // (e) the slab between the probe heights on the MAP layer: the M5 heights walk into it, the body probes stop
        let mut w = layered(vec![], 1);
        w.map.solids.push(cart());
        let mut x = Run::new(d, w.clone(), V3::ZERO, hz);
        x.cfg.body_probes = false;
        x.walk(3.0, keys(true, false, false));
        r.check(id, hz, "body probes off (M8): walks into a bed between the knee and chest rays", x.inside.is_some() || x.capsule.is_some(), format!("z {:.1}", x.p.pos.z));
        let mut x = Run::new(d, w, V3::ZERO, hz);
        x.walk(3.0, keys(true, false, false));
        r.check(id, hz, "body probes on: stops at the bed", x.inside.is_none() && x.capsule.is_none() && x.p.pos.z <= FACE_Z - x.cfg.radius - x.cfg.skin + 0.1, format!("z {:.2} {:?} {:?}", x.p.pos.z, x.inside, x.capsule));
        // (f) prone into the cart (union): the bed (0.62 m) is under the prone hull (0.8 m): stopped, never inside
        let mut x = Run::new(d, layered(vec![cart()], 0), V3::ZERO, hz);
        x.tick(Input { prone_presses: 1, ..keys(false, false, false) });
        x.walk(1.5, keys(false, false, false));
        let prone = x.p.stance.effective() == StanceKind::Prone;
        x.walk(8.0, keys(true, false, false));
        r.check(id, hz, "prone crawl into the cart: stopped at the bed, never inside", prone && x.inside.is_none() && x.capsule.is_none() && x.p.pos.z < FACE_Z, format!("prone {prone} z {:.2} {:?} {:?}", x.p.pos.z, x.inside, x.capsule));
    }
}

/// One stuck episode like run 18: the player's axis inside a 1 m object box (rays from inside it miss, so the
/// controller cannot see it), W held against a map wall in front, ER pushing the proxy out (0.04 m toward -z) on every
/// 7th tick, the controller walking back each time (run 18: speed_world ~0.5 m/s, the feet within ~1 m for a minute).
/// Returns the time to the detector's first event, the event and whether the axis was inside the object.
fn stuck_until_event(d: &MoveData, hz: f64, u: &mut Unstick, p: &UnstickParams, t0: f64, max_s: f64) -> (f64, Option<UnstickEvent>, bool) {
    let boxx = Convex::aabb(V3::new(-30.0, 0.0, FACE_Z), V3::new(30.0, 1.0 * M, FACE_Z + 60.0));
    let mut w = layered(vec![boxx], 0);
    w.map.solids.push(Convex::aabb(V3::new(-200.0, 0.0, FACE_Z + 40.0), V3::new(200.0, 3.0 * M, FACE_Z + 80.0)));
    // already pressed against the map wall (the contact distance r + skin)
    let c = SimCfg::defaults(d);
    let mut x = Run::new(d, w, V3::new(0.0, 0.0, FACE_Z + 40.0 - c.radius - c.skin - 0.01), hz);
    x.t = t0;
    let n = (max_s * hz) as usize;
    let mut was_inside = false;
    for k in 0..n {
        // ER's proxy push-out (applied by the DLL as Input.push through the collide-and-slide): 0.04 m toward -z
        let pushed = k % 7 == 0;
        let push = if pushed { V3::new(0.0, 0.0, -0.04 * M) } else { V3::ZERO };
        let st = x.tick(Input { push, ..keys(true, false, false) });
        was_inside |= x.all.axis_inside(x.p.pos, 60.0, 16, 0.05).is_some();
        let ui = UnstickIn { now: x.t, pos: x.p.pos * 0.0254, wish: true, pushed, inside: st.inside > 0, mantling: x.p.mantling() };
        if let Some(e) = u.tick(&ui, p) {
            return (x.t - t0, Some(e), was_inside);
        }
    }
    (max_s, None, was_inside)
}

fn b26(d: &MoveData, r: &mut Report) {
    let p = UnstickParams::default();
    for hz in RATES {
        let id = "B26";
        // the first episode fires within the window (+ 1 tick) with a 1 s hold
        let mut u = Unstick::default();
        let (dt, e, inside) = stuck_until_event(d, hz, &mut u, &p, 0.0, 10.0);
        let first = matches!(e, Some(UnstickEvent::Fire(f)) if f.n == 1 && (f.hold_s - p.hold_s).abs() < 1e-9);
        r.check(id, hz, "stuck in an object with ER pushing: unstick fires within the window", first && inside && dt <= p.window_s + 2.0 / hz, format!("dt {dt:.3} {e:?} inside {inside}"));
        // repeated episodes (the handback resolved nothing): holds 1 / 2 / 4 / 8 s, then a Capped rest
        let mut t = dt;
        let mut holds = Vec::new();
        let mut capped = None;
        if let Some(UnstickEvent::Fire(f)) = e {
            holds.push(f.hold_s);
            t += f.hold_s;
        }
        for _ in 0..6 {
            u.reset_window();
            let (dt, e, _) = stuck_until_event(d, hz, &mut u, &p, t, 10.0);
            t += dt;
            match e {
                Some(UnstickEvent::Fire(f)) => {
                    holds.push(f.hold_s);
                    t += f.hold_s;
                }
                Some(UnstickEvent::Capped { n, until }) => {
                    capped = Some((n, until, t));
                    break;
                }
                None => break,
            }
        }
        r.check(id, hz, "repeats: holds double 1/2/4/8 s", holds == vec![1.0, 2.0, 4.0, 8.0], format!("{holds:?}"));
        r.check(id, hz, "then capped (max_per_window within the backoff window): no endless loop", capped.is_some_and(|(n, until, at)| n == p.max_per_window && until > at), format!("{capped:?}"));
        // while capped: no fire until the window frees
        if let Some((_, until, at)) = capped {
            u.reset_window();
            let (_, e, _) = stuck_until_event(d, hz, &mut u, &p, at, (until - at - 0.5).clamp(1.0, 20.0));
            r.check(id, hz, "resting: no fire while capped", !matches!(e, Some(UnstickEvent::Fire(_))), format!("{e:?}"));
        }
        // never on an ordinary wall (map) or the cart (objects) with W held for 30 s: no evidence
        for (name, w) in [("wall", { let mut l = layered(vec![], 0); l.map.solids.push(bx(3.0)); l }), ("cart", layered(vec![cart()], 0))] {
            let mut x = Run::new(d, w, V3::ZERO, hz);
            let mut u = Unstick::default();
            let mut fired = None;
            for _ in 0..(30.0 * hz) as usize {
                let st = x.tick(keys(true, false, false));
                let ui = UnstickIn { now: x.t, pos: x.p.pos * 0.0254, wish: true, pushed: false, inside: st.inside > 0, mantling: false };
                if let Some(e) = u.tick(&ui, &p) {
                    fired = Some(e);
                    break;
                }
            }
            r.check(id, hz, &format!("W into the {name} for 30 s: never fires"), fired.is_none() && x.inside.is_none(), format!("{fired:?} inside {:?}", x.inside));
        }
    }
}

pub fn run_all(d: &MoveData, r: &mut Report) {
    b25(d, r);
    b26(d, r);
}
