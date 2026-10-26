//! movebench M8 scenarios B13-B24 (docs/M8_DESIGN.md section 12.2 + the review's B21-B24; run 18: B25-B26 in run18_bench.rs): the real controller with
//! the converted mantle data on synthetic rocks, ledges and walls at 30 / 60 / 144 Hz (B23: a 0.1 s hitch). Every
//! scenario also checks the bench-wide rules: mantle rays per tick <= that tick's budget (max_rays + 4 per extra
//! substep), a detection <= DETECT_MAX_RAYS (24 since run 18), main rays <= max_rays (48), no capsule-axis point inside geometry, and (M8 review) no
//! capsule-radius point inside geometry on a mantle tick.

use mw2er_core::mantle::{AbortWhy, DETECT_MAX_RAYS, MantleKind, RIM_SETDOWN_S, Refuse};
use mw2er_core::move_collide::synth::{Convex, World};
use mw2er_core::move_sim::{Input, MantleStart, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::MoveData;
use mw2er_core::stance::StanceKind;
use mw2er_core::vec3::V3;
use serde_json::{Value, json};

const M: f32 = 1.0 / 0.0254;
pub const RATES: [f64; 3] = [30.0, 60.0, 144.0];
const FACE_Z: f32 = 30.0;

pub struct Report {
    pub rows: Vec<Value>,
    pub fails: u32,
}

impl Report {
    pub fn check(&mut self, id: &str, hz: f64, what: &str, ok: bool, got: String) {
        println!("{} {id} {hz:>3} Hz {what}: {got}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.fails += 1;
        }
        self.rows.push(json!({"scenario": id, "hz": hz, "what": what, "pass": ok, "got": got}));
    }
}

struct Run<'a> {
    d: &'a MoveData,
    cfg: SimCfg,
    p: Player,
    w: World,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
    mantle_rays_max: u32,
    rays_max: u32,
    total_max: u32,
    inside: Option<(f64, V3)>,
    lowest: f32,
    /// M8 review: a tick over its mantle budget (rays, budget), the largest detection, the capsule-radius check.
    over_budget: Option<(u32, u32)>,
    detect_max: u32,
    capsule: Option<(f64, V3)>,
    budget_max: u32,
}

fn fwd_input(w: bool, held: bool, press: bool) -> Input {
    Input { w, jump_held: held, jump_presses: press as u32, fwd_h: V3::new(0.0, 0.0, 1.0), right_h: V3::new(1.0, 0.0, 0.0), ..Input::default() }
}

impl<'a> Run<'a> {
    fn new(d: &'a MoveData, w: World, pos: V3, hz: f64) -> Run<'a> {
        Run {
            d,
            cfg: SimCfg::defaults(d),
            p: Player::new(d, pos),
            w,
            t: 0.0,
            hz,
            ev: Vec::new(),
            mantle_rays_max: 0,
            rays_max: 0,
            total_max: 0,
            inside: None,
            lowest: pos.y,
            over_budget: None,
            detect_max: 0,
            capsule: None,
            budget_max: 0,
        }
    }

    fn tick(&mut self, i: Input) {
        self.t += 1.0 / self.hz;
        let was = self.p.mantling();
        let n0 = self.ev.len();
        let st = self.p.tick(self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev);
        self.mantle_rays_max = self.mantle_rays_max.max(st.mantle_rays);
        self.rays_max = self.rays_max.max(st.rays);
        self.total_max = self.total_max.max(st.rays + st.mantle_rays);
        self.budget_max = self.budget_max.max(st.mantle_budget);
        if st.mantle_rays > st.mantle_budget && self.over_budget.is_none() {
            self.over_budget = Some((st.mantle_rays, st.mantle_budget));
        }
        for e in &self.ev[n0..] {
            match e {
                SimEvent::MantleStart(m) => self.detect_max = self.detect_max.max(m.rays),
                SimEvent::MantleRefuse { rays, .. } => self.detect_max = self.detect_max.max(*rays),
                _ => {}
            }
        }
        let hull = self.d.mw2.hull_crouch * self.cfg.v_scale;
        if self.inside.is_none()
            && let Some(q) = self.w.axis_inside(self.p.pos, hull, 64, 0.05)
        {
            self.inside = Some((self.t, q));
        }
        if (was || self.p.mantling())
            && self.capsule.is_none()
            && let Some(q) = self.w.capsule_inside(self.p.pos, hull, self.cfg.radius, 48, 0.75)
        {
            self.capsule = Some((self.t, q));
        }
        self.lowest = self.lowest.min(self.p.pos.y);
    }

    fn take(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.ev)
    }

    /// W + jump (pressed on the first tick, then held) until the mantle ended and the player is grounded, or `secs`.
    fn mantle(&mut self, secs: f64, turn_at: Option<f64>) -> Outcome {
        let mut o = Outcome::default();
        let t_end = self.t + secs;
        let mut first = true;
        while self.t < t_end {
            let mut i = fwd_input(true, true, first);
            if let Some(ta) = turn_at
                && o.start.is_some()
                && self.t >= o.t_start + ta
            {
                // the view turned 90 deg during the climb (the path is fixed in world space)
                i.fwd_h = V3::new(1.0, 0.0, 0.0);
                i.right_h = V3::new(0.0, 0.0, -1.0);
                i.w = false;
            }
            first = false;
            self.tick(i);
            for e in self.take() {
                match e {
                    SimEvent::MantleStart(m) => {
                        o.start = Some(m);
                        o.t_start = self.t - 1.0 / self.hz;
                    }
                    SimEvent::MantleEnd { dt, kind, pos, .. } => o.end = Some((dt, kind, pos)),
                    SimEvent::MantleRefuse { why, .. } => o.refusals.push(why),
                    SimEvent::MantleAbort { why, pos, .. } => o.abort = Some((why, pos)),
                    SimEvent::Jump { .. } => o.jumps += 1,
                    SimEvent::Land(l) => o.lands.push((l.fall_h, l.dmg_pct)),
                    _ => {}
                }
            }
            if o.end.is_some() && self.p.grounded {
                o.grounded_after = true;
                break;
            }
        }
        o
    }

    fn bench_rules(&self, r: &mut Report, id: &str) {
        r.check(
            id,
            self.hz,
            "mantle rays per tick <= its budget (max_rays + 4 per extra substep), detection <= DETECT_MAX_RAYS",
            self.over_budget.is_none() && self.detect_max <= DETECT_MAX_RAYS,
            format!("max {} of budget {} (over {:?}), detection max {}", self.mantle_rays_max, self.budget_max, self.over_budget, self.detect_max),
        );
        r.check(id, self.hz, &format!("main rays per tick <= max_rays_per_tick ({})", self.cfg.max_rays), self.rays_max <= self.cfg.max_rays, format!("{} (total {})", self.rays_max, self.total_max));
        r.check(id, self.hz, "capsule axis never inside geometry", self.inside.is_none(), format!("{:?}", self.inside));
        r.check(id, self.hz, "capsule radius never inside geometry on a mantle tick", self.capsule.is_none(), format!("{:?}", self.capsule));
    }
}

#[derive(Default)]
struct Outcome {
    start: Option<MantleStart>,
    t_start: f64,
    end: Option<(f64, MantleKind, V3)>,
    refusals: Vec<Refuse>,
    abort: Option<(AbortWhy, V3)>,
    jumps: u32,
    lands: Vec<(f32, f32)>,
    grounded_after: bool,
}

fn wall(h_m: f32, depth_in: f32) -> Convex {
    Convex::aabb(V3::new(-300.0, -10.0, FACE_Z), V3::new(300.0, h_m * M, FACE_Z + depth_in))
}

fn b13(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        for (h, held) in [(1.0f32, false), (1.5, false), (1.98, true)] {
            let mut x = Run::new(d, World::flat().with(wall(h, 200.0)), V3::ZERO, hz);
            let o = x.mantle(2.5, None);
            let id = "B13";
            let what = format!("{h} m {}", if held { "held jump (airborne)" } else { "grounded press" });
            let ok_kind = o.start.is_some_and(|s| s.kind == MantleKind::Up) && o.end.is_some();
            r.check(id, hz, &format!("{what}: kind up, ended"), ok_kind, format!("{:?}", o.start.map(|s| s.kind)));
            if let (Some(s), Some((dt, _, pos))) = (o.start, o.end) {
                r.check(id, hz, &format!("{what}: time = planned +- 1 tick"), (dt - s.dur).abs() <= 1.0 / hz + 1e-6, format!("{dt:.4} vs {:.4}", s.dur));
                // M8 review: planned = MW2's SP up time (500 ms, V), not the code's own number
                let up = d.mantle.as_ref().map_or(0.5, |m| m.up_s as f64);
                r.check(id, hz, &format!("{what}: planned = MW2 up {up:.3} s"), (s.dur - up).abs() < 1e-5, format!("{:.4}", s.dur));
                r.check(id, hz, &format!("{what}: end on the top +- 0.05 in"), (pos.y - h * M).abs() <= 0.05 && (pos - s.end).len() <= 0.05, format!("y {:.3} (top {:.3})", pos.y, h * M));
                r.check(id, hz, &format!("{what}: grounded right after"), o.grounded_after && x.p.grounded, format!("{}", x.p.grounded));
                if !held {
                    r.check(id, hz, &format!("{what}: no Jump event (the mantle replaces the jump)"), o.jumps == 0 && !s.air, format!("jumps {} air {}", o.jumps, s.air));
                } else {
                    r.check(id, hz, &format!("{what}: caught in the air after the jump"), s.air && o.jumps == 1, format!("air {} jumps {}", s.air, o.jumps));
                }
            }
            x.bench_rules(r, id);
        }
    }
}

fn b14(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat().with(wall(2.5, 200.0)), V3::ZERO, hz);
        // jump in place facing the 2.5 m wall (no W)
        let mut refused = false;
        let mut jumps = 0;
        let mut lands = 0;
        for k in 0..(hz as usize * 2) {
            x.tick(Input { w: false, ..fwd_input(false, true, k == 0) });
            for e in x.take() {
                match e {
                    SimEvent::MantleRefuse { why: Refuse::TooHigh, .. } => refused = true,
                    SimEvent::MantleStart(_) => refused = false,
                    SimEvent::Jump { .. } => jumps += 1,
                    SimEvent::Land(_) => lands += 1,
                    _ => {}
                }
            }
        }
        r.check("B14", hz, "2.5 m refused too_high", refused && !x.p.mantling(), format!("refused {refused}"));
        r.check("B14", hz, "an ordinary jump and landing", jumps == 1 && lands >= 1 && x.p.grounded, format!("jumps {jumps} lands {lands}"));
        r.check("B14", hz, "no position change beyond the jump", x.p.pos.len() < 0.5, format!("{:?}", x.p.pos));
        x.bench_rules(r, "B14");
    }
}

fn b15(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        for (gap, want_crouch) in [(1.0f32, false), (1.45, true)] {
            let top = 1.5 * M;
            let roof = Convex::aabb(V3::new(-300.0, top + gap * M, FACE_Z), V3::new(300.0, top + gap * M + 20.0, FACE_Z + 200.0));
            let mut x = Run::new(d, World::flat().with(wall(1.5, 200.0)).with(roof), V3::ZERO, hz);
            let o = x.mantle(2.5, None);
            if want_crouch {
                let ok = o.start.is_some_and(|s| s.kind == MantleKind::Crouch) && o.end.is_some();
                r.check("B15", hz, "roof 1.45 m over the top: kind crouch", ok, format!("{:?}", o.start.map(|s| s.kind)));
                r.check("B15", hz, "stance crouch at the end", x.p.stance.goal() == StanceKind::Crouch, format!("{:?}", x.p.stance.goal()));
            } else {
                let ok = o.start.is_none() && o.refusals.contains(&Refuse::NoHeadroom);
                r.check("B15", hz, "roof 1.0 m over the top: no_headroom", ok, format!("{:?}", o.refusals.first()));
            }
            x.bench_rules(r, "B15");
        }
    }
}

fn b16_b17(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        // B16: a 0.3 m x 1.0 m wall with the floor behind
        let mut x = Run::new(d, World::flat().with(wall(1.0, 0.3 * M)), V3::ZERO, hz);
        let o = x.mantle(1.5, None);
        let mut lands = o.lands.clone();
        for _ in 0..(hz as usize * 2) {
            x.tick(fwd_input(false, false, false));
            for e in x.take() {
                if let SimEvent::Land(l) = e {
                    lands.push((l.fall_h, l.dmg_pct));
                }
            }
        }
        let over = o.start.is_some_and(|s| s.kind == MantleKind::Over);
        r.check("B16", hz, "thin wall: kind over", over, format!("{:?}", o.start.map(|s| s.kind)));
        r.check("B16", hz, "ends on the far side, on the floor", x.p.pos.z > FACE_Z + 0.3 * M && x.p.grounded && x.p.pos.y.abs() < 0.1, format!("{:?}", x.p.pos));
        r.check("B16", hz, "lands without damage", !lands.is_empty() && lands.iter().all(|(_, d)| *d == 0.0), format!("{lands:?}"));
        x.bench_rules(r, "B16");
        // B17: the same wall with a 6 m drop behind it
        let w = World {
            solids: vec![
                Convex::aabb(V3::new(-4e3, -100.0, -4e3), V3::new(4e3, 0.0, FACE_Z + 0.3 * M)),
                Convex::aabb(V3::new(-4e3, -6.0 * M - 50.0, FACE_Z), V3::new(4e3, -6.0 * M, 4e3)),
                wall(1.0, 0.3 * M),
            ],
            rays: 0,
        };
        let mut x = Run::new(d, w, V3::ZERO, hz);
        let o = x.mantle(1.5, None);
        r.check("B17", hz, "a 6 m drop behind: over_drop", o.start.is_none() && o.refusals.contains(&Refuse::OverDrop), format!("{:?}", o.refusals.first()));
        r.check("B17", hz, "stays on the near side", x.p.pos.z < FACE_Z, format!("{:?}", x.p.pos));
        x.bench_rules(r, "B17");
    }
}

fn b18(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut a = Run::new(d, World::flat().with(wall(1.0, 200.0)), V3::ZERO, hz);
        let oa = a.mantle(2.0, None);
        let mut b = Run::new(d, World::flat().with(wall(1.0, 200.0)), V3::ZERO, hz);
        let ob = b.mantle(2.0, Some(0.1));
        let ok = match (oa.end, ob.end) {
            (Some((_, _, pa)), Some((_, _, pb))) => (pa - pb).len() <= 0.01,
            _ => false,
        };
        r.check("B18", hz, "turning 90 deg mid-climb: the same end (+- 0.01 in)", ok, format!("{:?} vs {:?}", oa.end.map(|e| e.2), ob.end.map(|e| e.2)));
        b.bench_rules(r, "B18");
    }
}

fn b19(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        // a rounded rock with a broad walkable crown
        let rock = Convex::rock(V3::new(0.0, 0.0, FACE_Z + 60.0), 60.0, 1.2 * M, 12, 200.0);
        let mut x = Run::new(d, World::flat().with(rock), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        let on_crown = x.p.grounded && x.p.normal.y >= 0.7 && x.p.pos.y > 1.0 * M;
        r.check("B19", hz, "rounded rock: mantled onto a walkable crown", o.end.is_some() && on_crown, format!("y {:.1} n.y {:.3} grounded {} end {:?} refusals {:?} abort {:?}", x.p.pos.y, x.p.normal.y, x.p.grounded, o.end, o.refusals, o.abort));
        x.bench_rules(r, "B19");
        // a top tilted 50 deg (n.y 0.64 < walkable 0.7): refused (a 35 deg top is walkable by MW2's 0.7 and accepted)
        let wd = Convex::wedge(-300.0, 300.0, FACE_Z, FACE_Z + 200.0, -10.0, V3::new(0.0, 1.0 * M, 0.0), 50.0, V3::new(1.0, 0.0, 0.0));
        let mut x = Run::new(d, World::flat().with(wd), V3::ZERO, hz);
        let o = x.mantle(1.0, None);
        let refused = o.start.is_none() && o.refusals.iter().any(|w| matches!(w, Refuse::Slope | Refuse::Edge));
        r.check("B19", hz, "50 deg top refused slope", refused, format!("{:?}", o.refusals.first()));
        let wd = Convex::wedge(-300.0, 300.0, FACE_Z, FACE_Z + 200.0, -10.0, V3::new(0.0, 1.0 * M, 0.0), 35.0, V3::new(1.0, 0.0, 0.0));
        let mut x = Run::new(d, World::flat().with(wd), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        r.check("B19", hz, "35 deg top (walkable) accepted", o.end.is_some(), format!("{:?}", o.start.map(|s| s.kind)));
        x.bench_rules(r, "B19");
        // 2026-10-21 (`path_blocked rays=4` on a hillside): a 1.15 m step whose top rises away at 30 deg; a 2.5 m tier
        // 20 in past the face of a 1.15 m step (R2 meets a wall, not a top) is still refused
        let hill = Convex::wedge(-300.0, 300.0, FACE_Z, FACE_Z + 400.0, -10.0, V3::new(0.0, 1.15 * M, FACE_Z), 30.0, V3::new(0.0, 0.0, 1.0));
        let mut x = Run::new(d, World::flat().with(hill), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        let on_hill = o.end.is_some() && x.p.grounded && x.p.pos.y > 1.15 * M && o.abort.is_none();
        r.check("B19", hz, "1.15 m step, top rising 30 deg: mantled onto the slope", on_hill, format!("y {:.1} end {:?} refusals {:?} abort {:?}", x.p.pos.y, o.end, o.refusals, o.abort));
        x.bench_rules(r, "B19");
        let tier = Convex::aabb(V3::new(-300.0, -10.0, FACE_Z + 20.0), V3::new(300.0, 2.5 * M, FACE_Z + 200.0));
        let mut x = Run::new(d, World::flat().with(wall(1.15, 200.0)).with(tier), V3::ZERO, hz);
        let o = x.mantle(1.0, None);
        let refused = o.start.is_none() && o.refusals.iter().any(|w| matches!(w, Refuse::PathBlocked | Refuse::TooHigh));
        r.check("B19", hz, "2.5 m tier 20 in behind a 1.15 m step: refused, no climb", refused, format!("{:?}", o.refusals));
        x.bench_rules(r, "B19");
    }
}

fn b20(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        // a box appears on the path at 50 %
        let mut x = Run::new(d, World::flat().with(wall(1.0, 200.0)), V3::ZERO, hz);
        x.tick(fwd_input(true, true, true));
        let start = x.take().iter().any(|e| matches!(e, SimEvent::MantleStart(_)));
        let mut inserted = false;
        let mut abort = None;
        let mut lands: Vec<(f32, f32)> = Vec::new();
        for _ in 0..(hz as usize * 3) {
            if !inserted && x.p.mantle.as_ref().is_some_and(|m| m.frac() >= 0.5) {
                let p = x.p.pos;
                // just ahead of the capsule (r 15.75 + skin): it blocks the forward move, it is not dropped into the
                // capsule (the M8 review's capsule-radius rule judges every mantle tick)
                x.w.solids.push(Convex::aabb(V3::new(-60.0, p.y + 3.0, p.z + 18.0), V3::new(60.0, p.y + 80.0, p.z + 40.0)));
                inserted = true;
            }
            x.tick(fwd_input(false, false, false));
            for e in x.take() {
                match e {
                    SimEvent::MantleAbort { why, pos, .. } => abort = Some((why, pos)),
                    SimEvent::Land(l) => lands.push((l.fall_h, l.dmg_pct)),
                    _ => {}
                }
            }
        }
        let ok = start && matches!(abort, Some((AbortWhy::Blocked, _)));
        r.check("B20", hz, "box at 50 %: abort blocked", ok, format!("{abort:?}"));
        r.check("B20", hz, "after the abort: no fall damage, fall <= 2 m", lands.iter().all(|(h, d)| *d == 0.0 && *h <= 2.0 * M), format!("{lands:?}"));
        x.bench_rules(r, "B20");
        // a handback at 30 %
        let mut x = Run::new(d, World::flat().with(wall(1.5, 200.0)), V3::ZERO, hz);
        x.tick(fwd_input(true, true, true));
        while x.p.mantle.as_ref().is_some_and(|m| m.frac() < 0.3) {
            x.tick(fwd_input(true, true, false));
        }
        let lv = x.p.mantle.as_ref().map(|m| m.last_valid);
        let e = x.p.mantle_abort(AbortWhy::Handback("menu"));
        let at = x.p.pos;
        let mut lands: Vec<(f32, f32)> = Vec::new();
        for _ in 0..(hz as usize * 2) {
            x.tick(fwd_input(false, false, false));
            for e in x.take() {
                if let SimEvent::Land(l) = e {
                    lands.push((l.fall_h, l.dmg_pct));
                }
            }
        }
        let ok = matches!(e, Some(SimEvent::MantleAbort { why: AbortWhy::Handback("menu"), .. })) && lv == Some(at);
        r.check("B20", hz, "handback at 30 %: abort, feet at last_valid", ok, format!("{e:?}"));
        r.check("B20", hz, "falls <= 2 m without damage", lands.iter().all(|(h, d)| *d == 0.0 && *h <= 2.0 * M) && at.y - x.lowest <= 2.0 * M, format!("{lands:?} drop {:.1}", at.y - x.lowest));
        x.bench_rules(r, "B20");
    }
}

/// Rock proud of the face by `out_in` from `y0` to `y1` (M8 review).
fn bulge(y0: f32, y1: f32, out_in: f32) -> Convex {
    Convex::aabb(V3::new(-300.0, y0, FACE_Z - out_in), V3::new(300.0, y1, FACE_Z + 1.0))
}

/// B21 (M8 review): rock bulging toward the capsule between the knee and the lip, or over the climb.
fn b21(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        // the review's probe: feet 17 in from the face, W released, one press; 1.5 m top; slabs 12 / 8 in proud
        for (y0, y1, out) in [(26.0f32, 32.0f32, 12.0f32), (45.0, 55.0, 12.0), (45.0, 55.0, 8.0), (80.0, 90.0, 12.0)] {
            let mut x = Run::new(d, World::flat().with(wall(1.5, 200.0)).with(bulge(y0, y1, out)), V3::new(0.0, 0.0, 13.0), hz);
            let mut refused = None;
            let mut started = false;
            for k in 0..(hz as usize) {
                x.tick(fwd_input(false, false, k == 0));
                for e in x.take() {
                    match e {
                        SimEvent::MantleRefuse { why, .. } => refused = refused.or(Some(why)),
                        SimEvent::MantleStart(_) => started = true,
                        _ => {}
                    }
                }
            }
            r.check("B21", hz, &format!("rock {y0}-{y1} in, {out} in proud: refused path_blocked"), !started && refused == Some(Refuse::PathBlocked), format!("{refused:?} started {started}"));
            x.bench_rules(r, "B21");
        }
        // a 6 in bulge 30 in away: the capsule clears it, the mantle runs (capsule radius checked every mantle tick)
        let mut x = Run::new(d, World::flat().with(wall(1.5, 200.0)).with(bulge(26.0, 40.0, 6.0)), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        r.check("B21", hz, "a bulge the capsule clears: mantled, ended", o.start.is_some() && o.end.is_some(), format!("{:?}", o.refusals.first()));
        x.bench_rules(r, "B21");
    }
}

/// B22 (M8 review): a raised rim at the face (a curb / coping 2-2.9 in higher, 2-8 in deep).
fn b22(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        for (up, deep) in [(2.0f32, 3.0f32), (2.9, 8.0), (2.5, 2.0)] {
            let top = 1.0 * M;
            let rim = Convex::aabb(V3::new(-300.0, top - 1.0, FACE_Z), V3::new(300.0, top + up, FACE_Z + deep));
            let mut x = Run::new(d, World::flat().with(wall(1.0, 200.0)).with(rim), V3::ZERO, hz);
            let o = x.mantle(2.5, None);
            let what = format!("rim +{up} in x {deep} in");
            let ok = o.end.is_some_and(|(_, k, p)| k == MantleKind::Up && (p.y - top).abs() <= 0.05) && x.p.grounded;
            r.check("B22", hz, &format!("{what}: crossed over, ends on the top behind it"), ok, format!("{:?} refusals {:?}", o.end, o.refusals));
            if let (Some(s), Some((dt, _, _))) = (o.start, o.end) {
                let want = d.mantle.as_ref().map_or(0.5, |m| m.up_s as f64) + RIM_SETDOWN_S as f64;
                r.check("B22", hz, &format!("{what}: time = MW2 up + 50 ms set-down, +- 1 tick"), (s.dur - want).abs() < 1e-5 && (dt - s.dur).abs() <= 1.0 / hz + 1e-6, format!("{:.4} / {dt:.4}", s.dur));
            }
            x.bench_rules(r, "B22");
        }
    }
}

/// B23 (M8 review): a 0.1 s frame hitch (6 substeps) mid-climb: no false `budget` abort.
fn b23(d: &MoveData, r: &mut Report) {
    for h in [1.0f32, 1.5] {
        let mut x = Run::new(d, World::flat().with(wall(h, 200.0)), V3::ZERO, 10.0);
        let o = x.mantle(2.5, None);
        let ok = o.start.is_some() && o.end.is_some() && o.abort.is_none() && (x.p.pos.y - h * M).abs() <= 0.05 && x.p.grounded;
        r.check("B23", 10.0, &format!("{h} m at dt 0.1 s: mantled without an abort"), ok, format!("abort {:?} end {:?}", o.abort, o.end));
        r.check("B23", 10.0, &format!("{h} m at dt 0.1 s: the budget grew with the substeps"), x.budget_max >= x.cfg.mantle.max_rays + 20, format!("budget {}", x.budget_max));
        x.bench_rules(r, "B23");
    }
}

/// B24 (M8 review): an undercut shelf (the knee ray misses: the chest ray finds the face) and a 19 in block (above
/// the step, under the old 20 in knee ray).
fn b24(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let shelf = Convex::aabb(V3::new(-300.0, 37.0, FACE_Z), V3::new(300.0, 1.2 * M, FACE_Z + 80.0));
        let mut x = Run::new(d, World::flat().with(shelf), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        r.check("B24", hz, "undercut shelf 37 in - 1.2 m: mantled onto it", o.end.is_some_and(|(_, _, p)| (p.y - 1.2 * M).abs() <= 0.05), format!("{:?} {:?}", o.end, o.refusals));
        x.bench_rules(r, "B24");
        let block = Convex::aabb(V3::new(-300.0, -10.0, FACE_Z), V3::new(300.0, 19.0, FACE_Z + 200.0));
        let mut x = Run::new(d, World::flat().with(block), V3::ZERO, hz);
        let o = x.mantle(2.5, None);
        let low = o.start.is_some_and(|s| s.row_height == 21.0 && !s.drop_weapon);
        r.check("B24", hz, "19 in block: a low mantle (row 21, gun up), on top", low && o.end.is_some_and(|(_, _, p)| (p.y - 19.0).abs() <= 0.05), format!("{:?}", o.start.map(|s| (s.row_height, s.drop_weapon))));
        x.bench_rules(r, "B24");
    }
}

/// Runs B13-B24; returns the report.
pub fn run_all(d: &MoveData) -> Report {
    let mut r = Report { rows: Vec::new(), fails: 0 };
    if d.mantle.is_none() {
        r.check("B13", 0.0, "movement.json has the mantle block", false, "none (run tools\\scripts\\convert_movement.py)".into());
        return r;
    }
    b13(d, &mut r);
    b14(d, &mut r);
    b15(d, &mut r);
    b16_b17(d, &mut r);
    b18(d, &mut r);
    b19(d, &mut r);
    b20(d, &mut r);
    b21(d, &mut r);
    b22(d, &mut r);
    b23(d, &mut r);
    b24(d, &mut r);
    // run 18: the object layer (a cart, crates) and the unstick detector
    crate::run18_bench::run_all(d, &mut r);
    r
}

/// The number of scenarios `run_all` must report.
pub const SCENARIOS: usize = 14;
