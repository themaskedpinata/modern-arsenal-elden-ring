//! M8 mantle oracles (docs/M8_DESIGN.md section 12.1): detection against synthetic worlds, the over path, the
//! ray-origin axiom, the path, the state machine with its aborts, never-inside-geometry, the trigger, the end and the
//! weapon rule.

use super::*;
use crate::move_collide::synth::{Convex, World};
use crate::move_sim::{Input, Player, SimCfg, SimEvent};
use crate::movement_data::{MoveData, test_data};
use crate::sprint::{EndWhy, SprintEvent};
use crate::stance::StanceKind;

const M: f32 = 1.0 / 0.0254;
const FWD: V3 = V3 { x: 0.0, y: 0.0, z: 1.0 };
const RIGHT: V3 = V3 { x: 1.0, y: 0.0, z: 0.0 };
/// The face of every test obstacle (the player stands at z = 0, 30 in from it: radius + skin + ~13 in).
const FACE_Z: f32 = 30.0;

fn wall(h_m: f32, depth_in: f32) -> Convex {
    Convex::aabb(V3::new(-200.0, -10.0, FACE_Z), V3::new(200.0, h_m * M, FACE_Z + depth_in))
}

fn world_with(c: Convex) -> World {
    World::flat().with(c)
}

/// M8 review: a wall `h_m` high whose face carries a rim `up_in` higher and `deep_in` deep (a curb, a coping).
fn rim_world(h_m: f32, up_in: f32, deep_in: f32) -> World {
    let top = h_m * M;
    world_with(wall(h_m, 80.0)).with(Convex::aabb(V3::new(-200.0, top - 1.0, FACE_Z), V3::new(200.0, top + up_in, FACE_Z + deep_in)))
}

/// M8 review: a slab from `under_in` up to `top_m` with nothing below it (an undercut ledge, a cart bed).
fn shelf(under_in: f32, top_m: f32) -> Convex {
    Convex::aabb(V3::new(-200.0, under_in, FACE_Z), V3::new(200.0, top_m * M, FACE_Z + 60.0))
}

/// M8 review: rock proud of the face by `out_in`, from `y0` to `y1` above the floor.
fn bulge(y0: f32, y1: f32, out_in: f32) -> Convex {
    Convex::aabb(V3::new(-200.0, y0, FACE_Z - out_in), V3::new(200.0, y1, FACE_Z + 1.0))
}

fn query<'a>(d: &MoveData, c: &SimCfg, feet: V3, g0: f32, fwd: V3, others: &'a [V3]) -> Query<'a> {
    Query {
        feet,
        fwd,
        radius: c.radius,
        skin: c.skin,
        hull_stand: d.mw2.hull_stand * c.v_scale,
        hull_crouch: d.mw2.hull_crouch * c.v_scale,
        ground_y0: g0,
        walkable_y: c.walkable_y,
        others,
    }
}

fn detect_cfg(w: &mut World, feet: V3, g0: f32, mc: &MantleCfg) -> Result<Ledge, (Refuse, f32)> {
    let d = test_data();
    let c = SimCfg::defaults(&d);
    let mut b = RayBudget::new(mc.max_rays);
    let r = find_ledge(&mut b, w, &query(&d, &c, feet, g0, FWD, &[]), mc, d.mantle.as_ref().unwrap());
    assert!(b.used <= mc.max_rays, "rays {} > {}", b.used, mc.max_rays);
    r
}

fn detect(w: &mut World, feet: V3, g0: f32) -> Result<Ledge, (Refuse, f32)> {
    let d = test_data();
    let c = SimCfg::defaults(&d);
    detect_cfg(w, feet, g0, &c.mantle)
}

fn why(r: Result<Ledge, (Refuse, f32)>) -> Refuse {
    match r {
        Err((w, _)) => w,
        Ok(l) => panic!("accepted: {l:?}"),
    }
}

// ---- a small simulator --------------------------------------------------------------------------------------------

struct Sim {
    d: MoveData,
    cfg: SimCfg,
    p: Player,
    w: World,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
    max_mantle_rays: u32,
    max_rays: u32,
    inside: Option<(f64, V3)>,
    /// M8 review: the capsule radius check on mantle ticks (not only the axis).
    capsule: Option<(f64, V3)>,
    /// A tick spent more mantle rays than its budget (max_rays + 4 per extra substep), or a detection more than
    /// DETECT_MAX_RAYS.
    over_budget: Option<(u32, u32)>,
    max_detect: u32,
}

impl Sim {
    fn new(w: World, pos: V3, hz: f64) -> Sim {
        let d = test_data();
        let cfg = SimCfg::defaults(&d);
        let p = Player::new(&d, pos);
        Sim { d, cfg, p, w, t: 0.0, hz, ev: Vec::new(), max_mantle_rays: 0, max_rays: 0, inside: None, capsule: None, over_budget: None, max_detect: 0 }
    }

    fn tick(&mut self, i: Input) {
        self.t += 1.0 / self.hz;
        let was = self.p.mantling();
        let n0 = self.ev.len();
        let st = self.p.tick(&self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev);
        self.max_mantle_rays = self.max_mantle_rays.max(st.mantle_rays);
        self.max_rays = self.max_rays.max(st.rays);
        if st.mantle_rays > st.mantle_budget && self.over_budget.is_none() {
            self.over_budget = Some((st.mantle_rays, st.mantle_budget));
        }
        for e in &self.ev[n0..] {
            match e {
                SimEvent::MantleStart(m) => self.max_detect = self.max_detect.max(m.rays),
                SimEvent::MantleRefuse { rays, .. } => self.max_detect = self.max_detect.max(*rays),
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
    }

    /// The bench-wide budget rules (M8 review): per tick <= its budget, a detection <= DETECT_MAX_RAYS.
    fn budget_ok(&self) -> bool {
        self.over_budget.is_none() && self.max_detect <= DETECT_MAX_RAYS && self.max_rays <= self.cfg.max_rays
    }

    fn take(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.ev)
    }
}

fn keys(w: bool, jump_held: bool, jump_presses: u32) -> Input {
    Input { w, fwd_h: FWD, right_h: RIGHT, jump_held, jump_presses, ..Input::default() }
}

/// Holds W + jump (one press on the first tick) until the mantle ends or `secs` pass. Returns the start / end events.
fn mantle_run(s: &mut Sim, secs: f64) -> (Option<crate::move_sim::MantleStart>, Option<(f64, MantleKind, V3)>, Vec<SimEvent>) {
    let mut all = Vec::new();
    let t_end = s.t + secs;
    let mut first = true;
    let (mut start, mut end) = (None, None);
    while s.t < t_end {
        s.tick(keys(true, true, first as u32));
        first = false;
        for e in s.take() {
            match e {
                SimEvent::MantleStart(m) => start = Some(m),
                SimEvent::MantleEnd { dt, kind, pos, .. } => end = Some((dt, kind, pos)),
                _ => {}
            }
            all.push(e);
        }
        if end.is_some() && s.p.grounded {
            break;
        }
    }
    (start, end, all)
}

// ---- detection --------------------------------------------------------------------------------------------------

#[test]
fn mantle_detect_ground_heights_accepted() {
    let d = test_data();
    let md = d.mantle.as_ref().unwrap();
    for (h, kind) in [(0.6f32, MantleKind::Up), (1.0, MantleKind::Up), (1.5, MantleKind::Up)] {
        let mut w = world_with(wall(h, 60.0));
        let l = detect(&mut w, V3::ZERO, 0.0).unwrap_or_else(|e| panic!("{h} m: {e:?}"));
        assert_eq!(l.kind, kind, "{h} m");
        assert!((l.ledge_y - h * M).abs() < 0.1, "{h} m: ledge_y {}", l.ledge_y);
        assert_eq!(l.row, md.row_for(h * M));
        assert!((l.dir - FWD).len() < 1e-3);
        assert!(l.end.z > FACE_Z + 15.0, "the whole footprint on the top: end z {}", l.end.z);
        assert!(l.rays <= 16, "{} rays", l.rays);
    }
    // 2.0 m from the jump apex (feet 39 in up, the ground left at 0): the 2.0 m cap holds
    let mut w = world_with(wall(1.98, 60.0));
    let l = detect(&mut w, V3::new(0.0, 39.0, 0.0), 0.0).expect("2 m from the apex");
    assert!((l.ledge_y - 1.98 * M).abs() < 0.1);
    assert!(l.rise > 18.0 && l.rise <= 60.0);
}

#[test]
fn mantle_detect_too_high_refused() {
    let mut w = world_with(wall(2.5, 60.0));
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::TooHigh, "2.5 m from the ground");
    assert_eq!(why(detect(&mut w, V3::new(0.0, 39.0, 0.0), 0.0)), Refuse::TooHigh, "2.5 m from the apex");
    // 2.1 m from the apex: within 60 in of the feet but above the 2.0 m cap
    let mut w = world_with(wall(2.1, 60.0));
    assert_eq!(why(detect(&mut w, V3::new(0.0, 39.0, 0.0), 0.0)), Refuse::TooHigh, "above the user's cap");
    // from the ground a 1.6 m top is more than 60 in above the feet (MW2's window)
    let mut w = world_with(wall(1.6, 60.0));
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::TooHigh);
}

#[test]
fn mantle_detect_low_step_no_mantle() {
    // 0.40 m: below the knee probe (the step-up handles it)
    let mut w = world_with(wall(0.40, 60.0));
    assert!(matches!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::NoFace | Refuse::TooLow));
}

#[test]
fn mantle_detect_headroom_and_crouch() {
    let top = 1.5 * M;
    for (gap_m, want) in [(1.0f32, Err(Refuse::NoHeadroom)), (1.45, Ok(MantleKind::Crouch))] {
        // a roof over the top only (not over the player)
        let roof = Convex::aabb(V3::new(-200.0, top + gap_m * M, FACE_Z), V3::new(200.0, top + gap_m * M + 20.0, FACE_Z + 80.0));
        let mut w = world_with(wall(1.5, 80.0)).with(roof);
        let r = detect(&mut w, V3::ZERO, 0.0).map(|l| l.kind).map_err(|e| e.0);
        assert_eq!(r, want, "gap {gap_m} m");
    }
}

#[test]
fn mantle_detect_slope() {
    // a top tilted sideways at 50 deg (n.y 0.64 < 0.7): refused; at 35 deg (n.y 0.82, walkable by 0.7): accepted
    for (deg, ok) in [(50.0f32, false), (35.0, true)] {
        let wd = Convex::wedge(-200.0, 200.0, FACE_Z, FACE_Z + 80.0, -10.0, V3::new(0.0, 1.0 * M, 0.0), deg, RIGHT);
        let mut w = World::flat().with(wd);
        let r = detect(&mut w, V3::ZERO, 0.0);
        if ok {
            assert!(r.is_ok(), "{deg} deg: {r:?}");
        } else {
            assert!(matches!(why(r), Refuse::Slope | Refuse::Edge), "{deg} deg");
        }
    }
}

#[test]
fn mantle_detect_rounded_rock_crown_only() {
    // a 12-sided rock, 1.2 m high, crown flat; approached at its centre line: the crown is walkable
    let rock = |r_in: f32, dome: f32| Convex::rock(V3::new(0.0, 0.0, FACE_Z + r_in), r_in, 1.2 * M, 12, dome);
    let mut w = World::flat().with(rock(60.0, 200.0));
    let l = detect(&mut w, V3::ZERO, 0.0).expect("broad crown");
    // a walkable 15 deg shoulder facet or the crown, never below the rim (37.7 in)
    assert!(l.ledge_y > 1.0 * M && l.ledge_y <= 1.2 * M + 0.01, "{l:?}");
    // a small dome (steep shoulders): the first probe lands on a 35-55 deg facet, the deeper retry on the crown
    let mut found_retry = false;
    for dome in [40.0f32, 50.0, 60.0, 80.0] {
        let mut w = World::flat().with(rock(45.0, dome));
        match detect(&mut w, V3::ZERO, 0.0) {
            Ok(l) => found_retry |= l.retried,
            Err((r, _)) => assert!(matches!(r, Refuse::Slope | Refuse::Edge | Refuse::Budget | Refuse::NoLedge | Refuse::TooHigh | Refuse::PathBlocked), "dome {dome}: {r:?}"),
        }
    }
    // a steep cone-like rock (only 55 deg facets reach the probes): never accepted
    let cone = Convex::rock(V3::new(0.0, 0.0, FACE_Z + 40.0), 40.0, 1.2 * M, 12, 12.0);
    let mut w = World::flat().with(cone);
    let r = detect(&mut w, V3::ZERO, 0.0);
    assert!(r.is_err(), "a cone top: {r:?}");
    let _ = found_retry;
}

#[test]
fn mantle_detect_rounded_rock_retry_exercised() {
    // a ramp shoulder (40 deg rising away) then a flat crown 8 in further: the first centre probe is on the slope
    let shoulder = Convex::wedge(-200.0, 200.0, FACE_Z, FACE_Z + 300.0, -10.0, V3::new(0.0, 0.9 * M, FACE_Z), 40.0, FWD);
    let crown = Convex::aabb(V3::new(-200.0, -10.0, FACE_Z), V3::new(200.0, 0.9 * M + 22.0, FACE_Z + 300.0));
    let mut w = World::flat().with(shoulder).with(crown);
    let mut mc = SimCfg::defaults(&test_data()).mantle;
    mc.max_rays = 24;
    let r = detect_cfg(&mut w, V3::ZERO, 0.0, &mc);
    match r {
        Ok(l) => assert!(l.ledge_y > 0.9 * M),
        Err((w_, _)) => assert!(matches!(w_, Refuse::Slope | Refuse::PathBlocked | Refuse::TooHigh), "{w_:?}"),
    }
}

#[test]
fn mantle_detect_overhang_lip_path_blocked() {
    let top = 1.0 * M;
    let lip = Convex::aabb(V3::new(-200.0, top + 30.0, FACE_Z), V3::new(200.0, top + 60.0, FACE_Z + 10.0));
    let mut w = world_with(wall(1.0, 80.0)).with(lip);
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::PathBlocked);
}

#[test]
fn mantle_detect_protrusion_toward_the_capsule_path_blocked() {
    // M8 review probe: the feet 17 in from the face (W released), a 1.5 m top, rock 12 in proud of the face at
    // 26-32 in or 45-55 in (between the knee and the lip), or 8 in proud: the capsule (r 15.75) would climb through it
    let feet = V3::new(0.0, 0.0, 13.0);
    for (y0, y1, out) in [(26.0f32, 32.0f32, 12.0f32), (45.0, 55.0, 12.0), (26.0, 32.0, 8.0), (45.0, 55.0, 8.0)] {
        let mut w = world_with(wall(1.5, 80.0)).with(bulge(y0, y1, out));
        assert_eq!(why(detect(&mut w, feet, 0.0)), Refuse::PathBlocked, "bulge {y0}-{y1} in, {out} in proud");
    }
    // rock leaning over the climb above the head's start (80-90 in, 12 in proud): the front column sees it too
    let mut w = world_with(wall(1.5, 80.0)).with(bulge(80.0, 90.0, 12.0));
    assert_eq!(why(detect(&mut w, feet, 0.0)), Refuse::PathBlocked, "overhang over the climb");
    // the same face without the bulge is accepted from there (no false refusal), and a bulge the capsule clears too
    let mut w = world_with(wall(1.5, 80.0));
    assert!(detect(&mut w, feet, 0.0).is_ok());
    let mut w = world_with(wall(1.5, 80.0)).with(bulge(26.0, 40.0, 6.0));
    assert!(detect(&mut w, V3::ZERO, 0.0).is_ok(), "6 in proud, 30 in away: clear of the capsule");
    // a face nearer than r + s/2 (the capsule already in it at the knee): run 18 backs the start off to r + s when
    // two rays prove the space behind free; with `depenetrate_in = 0` (the M8 rule) it is refused
    let mut w = world_with(wall(1.0, 80.0));
    let inside = V3::new(0.0, 0.0, FACE_Z - 15.0);
    let l = detect(&mut w, inside, 0.0).expect("backed off and accepted");
    let c0 = SimCfg::defaults(&test_data());
    assert!(l.depen > 0.0 && (l.start.z - (FACE_Z - c0.radius - c0.skin)).abs() < 1e-3 && l.start.x == 0.0 && l.start.y == 0.0, "{l:?}");
    let mut mc = c0.mantle;
    mc.depenetrate_in = 0.0;
    assert_eq!(why(detect_cfg(&mut w, inside, 0.0, &mc)), Refuse::PathBlocked);
    // deeper than depenetrate_in (12 in), or a wall right behind the player: refused
    assert_eq!(why(detect(&mut w, V3::new(0.0, 0.0, FACE_Z - 2.0), 0.0)), Refuse::PathBlocked);
    let mut wb = world_with(wall(1.0, 80.0)).with(Convex::aabb(V3::new(-200.0, -10.0, FACE_Z - 40.0), V3::new(200.0, 200.0, FACE_Z - 30.0)));
    assert_eq!(why(detect(&mut wb, inside, 0.0)), Refuse::PathBlocked, "a wall behind");
    // and the player standing against the face at the walk's contact distance (r + skin) still mantles
    let c = SimCfg::defaults(&test_data());
    let touch = V3::new(0.0, 0.0, FACE_Z - c.radius - c.skin + 0.01);
    assert!(detect(&mut w, touch, 0.0).is_ok(), "contact distance");
}

#[test]
fn mantle_detect_raised_rim_crosses_over_it() {
    // M8 review probe: a 1.0 m wall with a 3-8 in deep rim 2.0-2.9 in higher at the face: the forward move runs over
    // the rim (lip + lift), R8 checks the feet's own path, the set-down waits for the end column
    for up in [2.0f32, 2.5, 2.9] {
        for deep in [2.0f32, 3.0, 5.0, 8.0] {
            let mut w = rim_world(1.0, up, deep);
            let l = detect(&mut w, V3::ZERO, 0.0).unwrap_or_else(|e| panic!("rim {up} x {deep}: {e:?}"));
            assert!(l.rim, "rim {up} x {deep}: {l:?}");
            assert!((l.ledge_y - 1.0 * M).abs() < 0.1, "the end on the top behind the rim");
            assert!((l.y_cross - (1.0 * M + up + LIFT_IN)).abs() < 0.05, "crossing over the rim: {}", l.y_cross);
            for hz in [30.0, 60.0, 144.0] {
                let mut s = Sim::new(rim_world(1.0, up, deep), V3::ZERO, hz);
                let (st, end, _) = mantle_run(&mut s, 2.0);
                let (st, (dt, _, pos)) = (st.expect("start"), end.expect("end"));
                assert!(s.inside.is_none(), "rim {up} x {deep} at {hz} Hz: axis inside {:?}", s.inside);
                assert!(s.capsule.is_none(), "rim {up} x {deep} at {hz} Hz: capsule inside {:?}", s.capsule);
                assert!((pos.y - 1.0 * M).abs() < 0.05 && s.p.grounded);
                assert!((st.dur - (0.5 + RIM_SETDOWN_S as f64)).abs() < 1e-5 && (dt - st.dur).abs() <= 1.0 / hz + 1e-6, "{} {dt}", st.dur);
            }
        }
    }
    // a rim narrower than the lip probe's offset (1 in deep): never crossed through; R8 on the feet's path refuses it
    let mut w = rim_world(1.0, 2.5, 1.0);
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::PathBlocked, "a 1 in deep rim");
    // a flat top (no rim) keeps the 2 in lift path and MW2's 0.5 s
    let mut w = world_with(wall(1.0, 80.0));
    let l = detect(&mut w, V3::ZERO, 0.0).unwrap();
    assert!(!l.rim && (l.y_cross - (l.ledge_y + LIFT_IN)).abs() < 1e-4);
    // a curb rising above the crossing line further in (a step on the top): refused, never crossed through
    let step = Convex::aabb(V3::new(-200.0, -10.0, FACE_Z + 12.0), V3::new(200.0, 1.0 * M + 3.0, FACE_Z + 16.0));
    let mut w = world_with(wall(1.0, 80.0)).with(step);
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::PathBlocked);
}

#[test]
fn mantle_detect_undercut_shelf_and_19in_block() {
    // M8 review: a shelf with a gap below (37 in up to 1.2 m): the knee ray misses, the chest ray finds the face
    let mut w = World::flat().with(shelf(37.0, 1.2));
    let l = detect(&mut w, V3::ZERO, 0.0).expect("undercut shelf");
    assert!(l.kind == MantleKind::Up && (l.ledge_y - 1.2 * M).abs() < 0.1, "{l:?}");
    // a 19 in rock: above the 18 in step, below the old 20 in knee ray: now a low mantle (row 21, the gun stays up)
    let d = test_data();
    let md = d.mantle.as_ref().unwrap();
    let mut w = world_with(Convex::aabb(V3::new(-200.0, -10.0, FACE_Z), V3::new(200.0, 19.0, FACE_Z + 80.0)));
    let l = detect(&mut w, V3::ZERO, 0.0).expect("19 in block");
    assert!((l.rise - 19.0).abs() < 0.1 && md.rows[l.row].height == 21.0 && !md.rows[l.row].drop_weapon, "{l:?}");
    // an 18 in block is the step's (no face above the knee), and open ground is no_face after both face rays
    let mut w = world_with(Convex::aabb(V3::new(-200.0, -10.0, FACE_Z), V3::new(200.0, 18.0, FACE_Z + 80.0)));
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::NoFace);
    let mut w = World::flat();
    let c = SimCfg::defaults(&d);
    let mut b = RayBudget::new(24);
    let r = find_ledge(&mut b, &mut w, &query(&d, &c, V3::ZERO, 0.0, FWD, &[]), &c.mantle, md);
    assert_eq!((why(r), b.used), (Refuse::NoFace, 2), "R0 + R0b");
}

#[test]
fn mantle_detect_low_roof_over_player_ceiling() {
    let roof = Convex::aabb(V3::new(-200.0, 80.0, -60.0), V3::new(200.0, 100.0, 20.0));
    for h in [1.0f32, 1.5] {
        let mut w = world_with(wall(h, 80.0)).with(roof.clone());
        assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::Ceiling, "{h} m");
    }
}

#[test]
fn mantle_detect_angle() {
    // a face at 70 deg to the view (the reach widened so the probe meets it)
    let d = test_data();
    let c = SimCfg::defaults(&d);
    let mut mc = c.mantle;
    mc.check_range_in = 40.0;
    let a = 70f32.to_radians();
    let fwd = V3::new(a.sin(), 0.0, a.cos());
    let mut w = World::flat().with(Convex::aabb(V3::new(-400.0, -10.0, 18.0), V3::new(400.0, 1.0 * M, 100.0)));
    let mut b = RayBudget::new(16);
    let r = find_ledge(&mut b, &mut w, &query(&d, &c, V3::ZERO, 0.0, fwd, &[]), &mc, d.mantle.as_ref().unwrap());
    assert_eq!(why(r), Refuse::Angle);
    // 40 deg: accepted, the path along the face normal (not the view)
    let a = 40f32.to_radians();
    let fwd = V3::new(a.sin(), 0.0, a.cos());
    let mut b = RayBudget::new(16);
    let l = find_ledge(&mut b, &mut w, &query(&d, &c, V3::ZERO, 0.0, fwd, &[]), &mc, d.mantle.as_ref().unwrap()).expect("40 deg");
    assert!((l.dir - FWD).len() < 1e-3, "{:?}", l.dir);
}

#[test]
fn mantle_detect_occupied_and_budget() {
    let d = test_data();
    let c = SimCfg::defaults(&d);
    let mut w = world_with(wall(1.0, 80.0));
    let l = detect(&mut w, V3::ZERO, 0.0).unwrap();
    let others = [l.top + V3::new(5.0, 0.0, 3.0)];
    let mut b = RayBudget::new(16);
    let r = find_ledge(&mut b, &mut w, &query(&d, &c, V3::ZERO, 0.0, FWD, &others), &c.mantle, d.mantle.as_ref().unwrap());
    assert_eq!(why(r), Refuse::Occupied);
    // a budget of 10: refused `budget`, never a partial accept
    let mut mc = c.mantle;
    mc.max_rays = 10;
    assert_eq!(why(detect_cfg(&mut w, V3::ZERO, 0.0, &mc)), Refuse::Budget);
    // and the controller starts nothing with it
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.cfg.mantle.max_rays = 10;
    let (start, _, ev) = mantle_run(&mut s, 1.0);
    assert!(start.is_none());
    assert!(ev.iter().any(|e| matches!(e, SimEvent::MantleRefuse { why: Refuse::Budget, .. })));
    assert!(s.max_mantle_rays <= 10);
}

#[test]
fn mantle_detect_refuse_labels_fixed_set() {
    let labels: Vec<&str> = Refuse::ALL.iter().map(|r| r.label()).collect();
    assert_eq!(labels.len(), 15);
    for want in ["no_face", "angle", "ceiling", "too_high", "too_low", "no_ledge", "slope", "edge", "narrow", "no_headroom", "path_blocked", "over_blocked", "over_drop", "occupied", "budget"] {
        assert!(labels.contains(&want), "{want}");
    }
}

// ---- over ------------------------------------------------------------------------------------------------------

#[test]
fn mantle_over_thin_wall_floor_behind() {
    let mut w = world_with(wall(1.0, 0.3 * M));
    let l = detect(&mut w, V3::ZERO, 0.0).expect("thin wall");
    assert_eq!(l.kind, MantleKind::Over);
    assert!(l.drop_y.is_some_and(|y| y.abs() < 0.1), "{:?}", l.drop_y);
    assert!(l.end.z > FACE_Z + 0.3 * M + 15.0, "released beyond the wall: {:?}", l.end);
}

#[test]
fn mantle_over_deep_drop_refused() {
    // floor only in front of the wall; a 6 m pit behind it
    let mut w = World {
        solids: vec![
            Convex::aabb(V3::new(-4e3, -100.0, -4e3), V3::new(4e3, 0.0, FACE_Z + 0.3 * M)),
            Convex::aabb(V3::new(-4e3, -6.0 * M - 50.0, FACE_Z), V3::new(4e3, -6.0 * M, 4e3)),
            wall(1.0, 0.3 * M),
        ],
        rays: 0,
    };
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::OverDrop);
}

#[test]
fn mantle_over_low_far_floor_no_drop() {
    // a platform 0.3 m below the wall top behind it
    let plat = Convex::aabb(V3::new(-200.0, -10.0, FACE_Z + 0.3 * M), V3::new(200.0, 0.7 * M, FACE_Z + 200.0));
    let mut w = world_with(wall(1.0, 0.3 * M)).with(plat);
    let l = detect(&mut w, V3::ZERO, 0.0).expect("over onto a platform");
    assert_eq!(l.kind, MantleKind::Over);
    let dy = l.ledge_y - l.drop_y.unwrap();
    assert!(dy > 0.0 && dy <= 18.0 + 0.1, "a step down, not a drop: {dy}");
}

#[test]
fn mantle_over_far_obstacle_blocked_and_off() {
    let crate_ = Convex::aabb(V3::new(-200.0, -10.0, FACE_Z + 0.3 * M + 8.0), V3::new(200.0, 1.5 * M, FACE_Z + 0.3 * M + 40.0));
    let mut w = world_with(wall(1.0, 0.3 * M)).with(crate_);
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::OverBlocked);
    let mut mc = SimCfg::defaults(&test_data()).mantle;
    mc.over = false;
    let mut w = world_with(wall(1.0, 0.3 * M));
    assert_eq!(why(detect_cfg(&mut w, V3::ZERO, 0.0, &mc)), Refuse::Narrow);
}

// ---- the origin axiom -------------------------------------------------------------------------------------------

/// A random rock field in front of the player (never within its capsule): boxes, wedges, rocks, roofs.
fn random_world(seed: u64) -> World {
    let mut r = crate::rng::Rng::new(seed);
    let mut f = |lo: f32, hi: f32| lo + (hi - lo) * r.next_f32();
    let mut w = World::flat();
    let n = 1 + (f(0.0, 3.0) as usize);
    for _ in 0..n {
        let z0 = f(18.0, 40.0);
        let kind = f(0.0, 4.0) as u32;
        let h = f(0.3, 2.4) * M;
        let c = match kind {
            0 => Convex::aabb(V3::new(f(-80.0, -10.0), -10.0, z0), V3::new(f(10.0, 80.0), h, z0 + f(6.0, 90.0))),
            1 => Convex::rock(V3::new(f(-20.0, 20.0), 0.0, z0 + 40.0), 40.0, h, 10, f(15.0, 200.0)),
            2 => Convex::wedge(-80.0, 80.0, z0, z0 + 80.0, -10.0, V3::new(0.0, h, z0), f(0.0, 60.0), if f(0.0, 1.0) > 0.5 { RIGHT } else { FWD }),
            // a roof: never inside the player's capsule (feet up to 30 in, head 103.5 in)
            _ => Convex::aabb(V3::new(-80.0, h.max(115.0), f(-40.0, 30.0)), V3::new(80.0, h.max(115.0) + 20.0, z0 + 60.0)),
        };
        w = w.with(c);
    }
    w
}

#[test]
fn mantle_origin_axiom_holds_on_random_rocks() {
    use std::cell::RefCell;
    let d = test_data();
    let c = SimCfg::defaults(&d);
    let md = d.mantle.as_ref().unwrap();
    // rays whose miss refuses (may start anywhere) and rays whose miss means "free" (must start in proven-free space)
    // (M8 review: R0's miss now leads on to R0b, so R0 is a free-on-miss ray from inside the capsule; R0b refuses)
    let refusing = [RayId::R0b, RayId::R3, RayId::R4a, RayId::R4b, RayId::O1];
    let mut accepted = 0;
    let mut checked = 0;
    for seed in 0..500u64 {
        let w0 = random_world(seed + 1);
        let feet = if seed % 3 == 0 { V3::new(0.0, 30.0, 0.0) } else { V3::ZERO };
        let q = query(&d, &c, feet, 0.0, FWD, &[]);
        // 1. baseline: every free-on-miss origin is outside every solid
        let bad: RefCell<Vec<(RayId, V3)>> = RefCell::new(Vec::new());
        let solids = w0.solids.clone();
        let rec = |id: RayId, o: V3| -> V3 {
            if !refusing.contains(&id) && solids.iter().any(|s| s.inside_by(o, 0.01)) {
                bad.borrow_mut().push((id, o));
            }
            o
        };
        let mut w = w0.clone();
        let mut b = RayBudget::new(16);
        let base = find_ledge_hooked(&mut b, &mut w, &q, &c.mantle, md, Some(&rec));
        assert!(bad.borrow().is_empty(), "seed {seed}: free-on-miss ray started inside a solid: {:?}", bad.borrow());
        accepted += base.is_ok() as u32;
        // 2. moving one refusing ray's origin into a solid never turns a refusal into an accept
        for id in refusing {
            let into = |rid: RayId, o: V3| -> V3 { if rid == id { V3::new(o.x, -50.0, o.z) } else { o } };
            let mut w = w0.clone();
            let mut b = RayBudget::new(16);
            let m = find_ledge_hooked(&mut b, &mut w, &q, &c.mantle, md, Some(&into));
            if base.is_err() {
                assert!(m.is_err(), "seed {seed}: {id:?} inside a solid turned {:?} into {m:?}", base);
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2500);
    assert!(accepted > 20, "the random field should contain mantleable rocks ({accepted})");
}

// ---- the path -------------------------------------------------------------------------------------------------

fn ledge_for(h: f32) -> (MoveData, SimCfg, Ledge) {
    let d = test_data();
    let c = SimCfg::defaults(&d);
    let mut w = world_with(wall(h, 80.0));
    let mut b = RayBudget::new(16);
    let l = find_ledge(&mut b, &mut w, &query(&d, &c, V3::ZERO, 0.0, FWD, &[]), &c.mantle, d.mantle.as_ref().unwrap()).unwrap();
    (d, c, l)
}

#[test]
fn mantle_path_c1_entry_monotone_rise_then_forward() {
    let (d, c, l) = ledge_for(1.0);
    let md = d.mantle.as_ref().unwrap();
    for v0 in [0.0f32, 120.0, 249.8] {
        let m = Mantle::plan(&l, V3::ZERO, v0, md, &c.mantle, 249.8, Trigger::Held, v0 > 0.0);
        let eps = 1e-4;
        let (p1, _) = m.at(eps);
        let dy = (p1.y - m.start.y) / eps as f32;
        assert!((dy - v0).abs() <= 0.01 * v0.max(1.0) + 0.5, "v0 {v0}: dy/dt(0) = {dy}");
        let mut last = m.start;
        let mut t = 0.0;
        while t <= m.duration() + 1e-9 {
            let (p, ph) = m.at(t);
            assert!(p.y >= last.y - 1e-3 || ph == Phase::Cross, "y monotone in the rise at {t}");
            if p.y < m.y_top - 1e-3 && ph == Phase::Rise {
                assert!((p.x - m.start.x).abs() < 1e-4 && (p.z - m.start.z).abs() < 1e-4, "planar motion below the ledge at {t}");
            }
            assert!(p.y <= m.y_top + 1e-3);
            last = p;
            t += 0.002;
        }
        let (pe, _) = m.at(m.duration());
        assert!((pe - l.end).len() < 0.001, "end exact: {pe:?} vs {:?}", l.end);
    }
}

#[test]
fn mantle_path_duration_table_and_time_scale() {
    let (d, c, l) = ledge_for(1.0);
    let md = d.mantle.as_ref().unwrap();
    let m = Mantle::plan(&l, V3::ZERO, 0.0, md, &c.mantle, 249.8, Trigger::Held, false);
    // M8 review: MW2's own time against MW2's own number: a mantle up takes md.up_s (the SP up anims' 500 ms, V)
    // within 1 us, whatever the forward distance X (the full footprint, ~2x MW2's 16 in: eased faster, not longer)
    assert!(l.x > 30.0, "the full-footprint X {}", l.x);
    let want = md.up_s as f64;
    assert!((m.duration() - want).abs() < 1e-6, "{} vs MW2 up {want}", m.duration());
    assert!((md.up_s - 0.5).abs() < 1e-6 && (md.rise_s - 1.0 / 3.0).abs() < 1e-6);
    // the over: rise + MW2's up-forward + the over anim's forward part (the over anim's drop is gravity)
    let mut wt = world_with(wall(1.0, 0.3 * M));
    let lo = detect(&mut wt, V3::ZERO, 0.0).expect("thin wall");
    let mo = Mantle::plan(&lo, V3::ZERO, 0.0, md, &c.mantle, 249.8, Trigger::Held, false);
    assert!((mo.duration() - (md.rise_s + md.cross_s + md.over_cross_s) as f64).abs() < 1e-6, "over {}", mo.duration());
    let mut c2 = c.mantle;
    c2.time_scale = 2.0;
    let m2 = Mantle::plan(&l, V3::ZERO, 0.0, md, &c2, 249.8, Trigger::Held, false);
    assert!((m2.duration() - 2.0 * want).abs() < 1e-5);
    // the curve samples are the data's (rows from movement.json)
    let row = &md.rows[l.row];
    assert_eq!(row.rise.first().copied(), Some(0.0));
    assert_eq!(row.rise.last().copied(), Some(1.0));
    let cu = Curve::uniform(&row.rise);
    for (i, v) in row.rise.iter().enumerate() {
        assert!((cu.at(i as f32 / (row.rise.len() - 1) as f32) - v).abs() < 1e-5);
    }
    // the controller takes the planned time +- 1 tick at 30 / 60 / 144 Hz
    for hz in [30.0, 60.0, 144.0] {
        let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, hz);
        let (start, end, _) = mantle_run(&mut s, 2.0);
        let (st, (dt, _, _)) = (start.expect("start"), end.expect("end"));
        assert!((dt - st.dur).abs() <= 1.0 / hz + 1e-6, "{hz} Hz: {dt} vs {}", st.dur);
    }
}

// ---- the state machine and aborts ------------------------------------------------------------------------------

fn start_mantle(hz: f64, w: World) -> Sim {
    let mut s = Sim::new(w, V3::ZERO, hz);
    s.tick(keys(true, true, 1));
    assert!(s.p.mantling(), "mantle started: {:?}", s.take());
    s
}

#[test]
fn mantle_state_start_end_and_handback_aborts() {
    for frac in [0.1f32, 0.5, 0.9] {
        let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
        while s.p.mantle.as_ref().is_some_and(|m| m.frac() < frac) {
            s.tick(keys(true, true, 0));
        }
        let lv = s.p.mantle.as_ref().unwrap().last_valid;
        let e = s.p.mantle_abort(AbortWhy::Handback("menu")).expect("abort");
        assert!(matches!(e, SimEvent::MantleAbort { why: AbortWhy::Handback("menu"), .. }));
        assert_eq!(s.p.pos, lv);
        assert!(s.w.axis_inside(s.p.pos, 70.0, 64, 0.05).is_none(), "last_valid outside the solids");
        assert!(!s.p.mantling() && !s.p.grounded && s.p.vel == V3::ZERO);
        // gravity settles it without damage
        let mut land = None;
        for _ in 0..120 {
            s.tick(keys(false, false, 0));
            for e in s.take() {
                if let SimEvent::Land(l) = e {
                    land = Some(l);
                }
            }
        }
        if let Some(l) = land {
            assert_eq!(l.dmg_pct, 0.0);
            assert!(l.fall_h <= 2.0 * M);
        }
        assert!(s.p.mantle_abort(AbortWhy::Warp).is_none(), "nothing left to abort");
    }
    assert_eq!(AbortWhy::Handback("death").label(), "handback:death");
}

#[test]
fn mantle_state_box_inserted_aborts_blocked() {
    let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    while s.p.mantle.as_ref().is_some_and(|m| m.frac() < 0.75) {
        s.tick(keys(true, true, 0));
    }
    // a box appears on the path over the top
    let p = s.p.pos;
    s.w.solids.push(Convex::aabb(V3::new(-50.0, p.y + 5.0, p.z + 20.0), V3::new(50.0, p.y + 60.0, p.z + 40.0)));
    let mut aborted = None;
    for _ in 0..40 {
        s.tick(keys(true, true, 0));
        for e in s.take() {
            if let SimEvent::MantleAbort { why, pos, .. } = e {
                aborted = Some((why, pos));
            }
        }
    }
    let (why, pos) = aborted.expect("aborted");
    assert_eq!(why, AbortWhy::Blocked);
    assert!(s.w.axis_inside(pos, 70.0, 64, 0.05).is_none());
}

#[test]
fn mantle_state_ledge_gone_and_er_push() {
    let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    while s.p.mantle.as_ref().is_some_and(|m| m.frac() < 0.4) {
        s.tick(keys(true, true, 0));
    }
    // the ledge removed before 60 %: the half-way check finds no top
    s.w.solids.truncate(1);
    let mut why = None;
    for _ in 0..30 {
        s.tick(keys(true, true, 0));
        for e in s.take() {
            if let SimEvent::MantleAbort { why: w, .. } = e {
                why = Some(w);
            }
        }
    }
    assert_eq!(why, Some(AbortWhy::LedgeGone));
    // a 0.4 m ER push mid-mantle: er_push
    let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    s.tick(keys(true, true, 0));
    s.take();
    s.tick(Input { er_delta_in: 0.4 * M, ..keys(true, true, 0) });
    assert!(s.take().iter().any(|e| matches!(e, SimEvent::MantleAbort { why: AbortWhy::ErPush, .. })));
    // 0.2 m: measured, not applied
    let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    s.tick(Input { er_delta_in: 0.2 * M, push: V3::new(0.0, 0.0, -10.0), ..keys(true, true, 0) });
    assert!(s.p.mantling());
    assert!((s.p.pos.z).abs() < 1e-3, "the push is not applied during the rise");
}

#[test]
fn mantle_state_rebase_translates_and_warp_resyncs() {
    let mut a = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    let mut b = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    for _ in 0..10 {
        a.tick(keys(true, true, 0));
        b.tick(keys(true, true, 0));
    }
    // b's world and player move by a re-base (32 m, 8 m up)
    let by = V3::new(32.0 * M, 8.0 * M, 0.0);
    for s in b.w.solids.iter_mut() {
        for (n, d) in s.planes.iter_mut() {
            *d += n.dot(by);
        }
    }
    b.p.translate(by);
    let (_, ea, _) = mantle_run(&mut a, 2.0);
    let (_, eb, _) = mantle_run(&mut b, 2.0);
    let (pa, pb) = (ea.unwrap().2, eb.unwrap().2);
    assert!((pb - by - pa).len() < 0.01, "same end relative to the geometry: {pa:?} {pb:?}");
    // a real warp: resync clears the path (the DLL logs abort why=warp first)
    let mut s = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    assert!(matches!(s.p.mantle_abort(AbortWhy::Warp), Some(SimEvent::MantleAbort { why: AbortWhy::Warp, .. })));
    let mut s2 = start_mantle(60.0, world_with(wall(1.0, 80.0)));
    s2.p.resync(V3::new(500.0, 0.0, 0.0), V3::ZERO, true);
    assert!(!s2.p.mantling(), "a resync never keeps a stale path");
    s.tick(keys(false, false, 0));
}

#[test]
fn mantle_state_new_player_never_resumes() {
    // enter() builds a new Player: no mantle, and a re-arm block of 0.3 s
    let d = test_data();
    let p = Player::new(&d, V3::ZERO);
    assert!(p.mantle.is_none());
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.p.mantle_block_until = s.t + REARM_BLOCK_S;
    s.tick(keys(true, true, 1));
    assert!(!s.p.mantling(), "no check within 0.3 s of a re-arm");
}

// ---- never inside geometry -----------------------------------------------------------------------------------------

#[test]
fn mantle_never_inside_geometry_all_worlds() {
    let worlds: Vec<(&str, World, V3)> = vec![
        ("0.6 m", world_with(wall(0.6, 80.0)), V3::ZERO),
        ("1.0 m", world_with(wall(1.0, 80.0)), V3::ZERO),
        ("1.5 m", world_with(wall(1.5, 80.0)), V3::ZERO),
        ("thin wall", world_with(wall(1.0, 0.3 * M)), V3::ZERO),
        ("crouch roof", world_with(wall(1.5, 80.0)).with(Convex::aabb(V3::new(-200.0, 1.5 * M + 1.45 * M, FACE_Z), V3::new(200.0, 1.5 * M + 1.45 * M + 20.0, FACE_Z + 80.0))), V3::ZERO),
        ("rock", World::flat().with(Convex::rock(V3::new(0.0, 0.0, FACE_Z + 60.0), 60.0, 1.2 * M, 12, 200.0)), V3::ZERO),
        // M8 review: a raised rim at the face (a curb / coping), an undercut shelf, a 19 in block, a bulge 6 in proud
        // of the face that the capsule clears (the player 30 in from the face)
        ("raised rim", rim_world(1.0, 2.9, 8.0), V3::ZERO),
        ("undercut shelf", World::flat().with(shelf(37.0, 1.2)), V3::ZERO),
        ("19 in block", world_with(Convex::aabb(V3::new(-200.0, -10.0, FACE_Z), V3::new(200.0, 19.0, FACE_Z + 80.0))), V3::ZERO),
        ("clear bulge", world_with(wall(1.5, 80.0)).with(bulge(26.0, 40.0, 6.0)), V3::ZERO),
    ];
    for hz in [30.0, 60.0, 144.0] {
        for (name, w, pos) in &worlds {
            let mut s = Sim::new(w.clone(), *pos, hz);
            let (start, _, _) = mantle_run(&mut s, 2.5);
            for _ in 0..(hz as usize) {
                s.tick(keys(true, false, 0));
            }
            assert!(start.is_some(), "{name} at {hz} Hz: no mantle");
            assert!(s.inside.is_none(), "{name} at {hz} Hz: axis inside geometry {:?}", s.inside);
            assert!(s.capsule.is_none(), "{name} at {hz} Hz: the capsule radius inside geometry during the mantle {:?}", s.capsule);
            assert!(s.budget_ok(), "{name}: mantle rays {:?} detect {} main {}", s.over_budget, s.max_detect, s.max_rays);
        }
    }
    // and through every abort
    for seed in 0..60u64 {
        let w = random_world(seed + 1000);
        let mut s = Sim::new(w, V3::ZERO, 60.0);
        s.tick(keys(true, true, 1));
        let stop = seed % 20;
        for k in 0..60 {
            if k as u64 == stop {
                s.p.mantle_abort(AbortWhy::Handback("menu"));
            }
            s.tick(keys(true, k % 7 == 0, (k % 7 == 0) as u32));
        }
        assert!(s.inside.is_none(), "seed {seed}: {:?}", s.inside);
    }
}

// ---- the trigger ----------------------------------------------------------------------------------------------

#[test]
fn mantle_trigger_crouched_refuses_without_rays() {
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.tick(Input { crouch_presses: 1, ..keys(false, false, 0) });
    for _ in 0..30 {
        s.tick(keys(false, false, 0));
    }
    assert_eq!(s.p.stance.effective(), StanceKind::Crouch);
    let st = s.p.tick(&s.d, &s.cfg, &Input { jump_held: true, ..keys(true, true, 0) }, s.t + 1.0 / 60.0, 1.0 / 60.0, &mut s.w, &mut s.ev);
    assert_eq!((st.mantle_checks, st.mantle_rays), (0, 0));
    assert!(!s.p.mantling());
}

#[test]
fn mantle_trigger_grounded_press_replaces_the_jump() {
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.tick(keys(true, false, 1));
    let ev = s.take();
    assert!(ev.iter().any(|e| matches!(e, SimEvent::MantleStart(_))), "{ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, SimEvent::Jump { .. })), "no Jump event");
    assert_eq!(s.p.slow_ms, 0.0);
    // a failed grounded check (2.5 m) jumps normally
    let mut s = Sim::new(world_with(wall(2.5, 80.0)), V3::ZERO, 60.0);
    s.tick(keys(true, false, 1));
    let ev = s.take();
    assert!(ev.iter().any(|e| matches!(e, SimEvent::MantleRefuse { why: Refuse::TooHigh, .. })));
    assert!(ev.iter().any(|e| matches!(e, SimEvent::Jump { .. })));
}

#[test]
fn mantle_trigger_held_jump_catches_in_the_air() {
    // 1.8 m: too high from the ground, reached by holding jump through the arc
    let mut s = Sim::new(world_with(wall(1.8, 80.0)), V3::ZERO, 60.0);
    let (start, end, ev) = mantle_run(&mut s, 2.0);
    let st = start.expect("caught in the air");
    assert!(st.air && st.trigger == Trigger::Held, "{st:?}");
    assert!(ev.iter().any(|e| matches!(e, SimEvent::Jump { .. })));
    assert!(end.is_some());
    assert!((s.p.pos.y - 1.8 * M).abs() < 0.1);
}

#[test]
fn mantle_trigger_key_modes_and_cache() {
    // press mode: holding jump without a new press does nothing in the air
    let mut s = Sim::new(world_with(wall(1.8, 80.0)), V3::ZERO, 60.0);
    s.cfg.mantle.key_mode = KeyMode::Press;
    let (start, _, _) = mantle_run(&mut s, 1.5);
    assert!(start.is_none(), "press mode needs a press in the window");
    // auto: walking into a 1.0 m ledge without jump mantles
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.cfg.mantle.key_mode = KeyMode::Auto;
    let mut started = false;
    for _ in 0..60 {
        s.tick(keys(true, false, 0));
        started |= s.take().iter().any(|e| matches!(e, SimEvent::MantleStart(_)));
    }
    assert!(started, "auto mantle while walking into the face");
    // mw2 mode without the jump key: never
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    for _ in 0..60 {
        s.tick(keys(true, false, 0));
    }
    assert!(s.take().iter().all(|e| !matches!(e, SimEvent::MantleStart(_) | SimEvent::MantleRefuse { .. })));
    // the negative cache: standing still with jump held at a 2.5 m wall checks once per 150 ms
    let mut s = Sim::new(world_with(wall(2.5, 80.0)), V3::ZERO, 60.0);
    s.p.grounded = true;
    let mut checks = 0;
    for _ in 0..60 {
        let t = s.t + 1.0 / 60.0;
        s.t = t;
        let st = s.p.tick(&s.d, &s.cfg, &Input { jump_held: true, ..keys(false, false, 0) }, t, 1.0 / 60.0, &mut s.w, &mut s.ev);
        checks += st.mantle_checks;
    }
    assert!((6..=8).contains(&checks), "about one check per 150 ms over 1 s: {checks}");
    // ...and re-opens on 0.1 m of motion
    let mut c = RefuseCache::default();
    c.refused(0.0, V3::ZERO, FWD, 0.15);
    assert!(!c.open(0.05, V3::new(0.0, 0.0, 1.0), FWD));
    assert!(c.open(0.06, V3::new(0.0, 0.0, 4.5), FWD));
    c.refused(0.0, V3::ZERO, FWD, 0.15);
    assert!(c.open(0.05, V3::ZERO, V3::new(0.3, 0.0, 1.0)), "turned > 10 deg");
}

#[test]
fn mantle_trigger_one_detection_per_tick() {
    // 30 Hz = 2 substeps: at most one detection per tick
    let mut s = Sim::new(world_with(wall(2.5, 80.0)), V3::ZERO, 30.0);
    for _ in 0..30 {
        let t = s.t + 1.0 / 30.0;
        s.t = t;
        let st = s.p.tick(&s.d, &s.cfg, &Input { jump_held: true, ..keys(true, false, 0) }, t, 1.0 / 30.0, &mut s.w, &mut s.ev);
        assert!(st.substeps == 2 && st.mantle_checks <= 1);
        assert!(st.mantle_rays <= st.mantle_budget && st.mantle_budget == s.cfg.mantle.max_rays + SUBSTEP_RAYS);
    }
}

#[test]
fn mantle_trigger_hitch_dt_0_1_no_budget_abort() {
    // M8 review probe: a 0.1 s frame (6 substeps): the budget grows by 4 per extra substep, so the climb is never a
    // false `budget` abort (it was `ABORT Budget t=0.73` at 16 rays)
    for h in [1.0f32, 1.5] {
        let mut s = Sim::new(world_with(wall(h, 80.0)), V3::ZERO, 10.0);
        let (st, end, ev) = mantle_run(&mut s, 2.0);
        assert!(st.is_some() && end.is_some(), "{h} m at 10 Hz: {ev:?}");
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::MantleAbort { .. })), "{h} m: aborted");
        assert!(s.budget_ok(), "{h} m: {:?}", s.over_budget);
        assert!((s.p.pos.y - h * M).abs() < 0.05 && s.p.grounded);
        assert!(s.inside.is_none() && s.capsule.is_none());
    }
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 10.0);
    let st = s.p.tick(&s.d, &s.cfg, &keys(true, true, 1), 0.1, 0.1, &mut s.w, &mut s.ev);
    assert_eq!((st.substeps, st.mantle_budget), (6, s.cfg.mantle.max_rays + 5 * SUBSTEP_RAYS));
}

#[test]
fn mantle_trigger_budget_guard_at_hitch() {
    // the per-tick budget max_rays + 4 x (substeps - 1) always leaves a detection (<= max_rays) the rays the rest of
    // the tick re-validates with: at 0.1 s the default starts at once, nothing is deferred, the rays stay in budget
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 10.0);
    let st = s.p.tick(&s.d, &s.cfg, &keys(true, true, 1), 0.1, 0.1, &mut s.w, &mut s.ev);
    assert!(s.p.mantling() && st.mantle_deferred == 0 && st.mantle_rays <= st.mantle_budget, "{st:?}");
    // max_rays below the detection's needs (10; the config allows 20..40): at 60 Hz refused `budget` (no partial
    // accept); at 0.1 s the detection fits the tick's budget (30) but leaves 16 < 4 x 5 for the rest of the tick:
    // deferred (no start, no refusal, no negative cache), never a climb it could not re-validate
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 60.0);
    s.cfg.mantle.max_rays = 10;
    s.tick(keys(true, true, 1));
    let ev = s.take();
    assert!(!s.p.mantling() && ev.iter().any(|e| matches!(e, SimEvent::MantleRefuse { why: Refuse::Budget, .. })), "{ev:?}");
    let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, 10.0);
    s.cfg.mantle.max_rays = 10;
    let st = s.p.tick(&s.d, &s.cfg, &keys(true, true, 1), 0.1, 0.1, &mut s.w, &mut s.ev);
    let ev = s.take();
    assert!(!s.p.mantling() && st.mantle_deferred == 1, "{st:?} {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, SimEvent::MantleStart(_) | SimEvent::MantleRefuse { .. })), "{ev:?}");
    assert_eq!(DETECT_MAX_RAYS, 24);
}

#[test]
fn mantle_state_rock_over_the_head_mid_rise_aborts() {
    // rock that appears over the head during the rise, proud of the face toward the player but clear of the axis
    // (the up ray misses it): the head's forward ray aborts the climb at the last validated point
    let mut s = start_mantle(60.0, world_with(wall(1.5, 80.0)));
    while s.p.mantle.as_ref().is_some_and(|m| m.frac() < 0.15) {
        s.tick(keys(true, true, 0));
    }
    let hull = s.d.mw2.hull_stand * s.cfg.v_scale;
    let y = s.p.pos.y + hull + 3.0;
    s.w.solids.push(Convex::aabb(V3::new(-200.0, y, 8.0), V3::new(200.0, y + 30.0, FACE_Z + 1.0)));
    let mut aborted = None;
    for _ in 0..30 {
        s.tick(keys(true, true, 0));
        for e in s.take() {
            if let SimEvent::MantleAbort { why, pos, .. } = e {
                aborted = Some((why, pos));
            }
        }
    }
    let (w, pos) = aborted.expect("aborted in the rise");
    assert_eq!(w, AbortWhy::Blocked);
    assert!(s.w.capsule_inside(pos, hull - 1.0, s.cfg.radius, 48, 0.75).is_none(), "last_valid clear of the rock");
    assert_eq!(AbortWhy::Insane.label(), "position_insane");
}

// ---- the end ---------------------------------------------------------------------------------------------------

#[test]
fn mantle_end_grounded_no_land_deferred_stance() {
    for hz in [30.0, 60.0, 144.0] {
        let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::ZERO, hz);
        s.tick(keys(true, false, 1));
        // a crouch press during the climb
        s.tick(Input { crouch_presses: 1, ..keys(true, true, 0) });
        let mut lands = 0;
        let mut ended = false;
        for _ in 0..(hz as usize) {
            s.tick(keys(true, false, 0));
            for e in s.take() {
                match e {
                    SimEvent::Land(_) => lands += 1,
                    SimEvent::MantleEnd { .. } => {
                        ended = true;
                        assert!(s.p.grounded || s.p.vel.y <= 0.0);
                    }
                    _ => {}
                }
            }
            if ended {
                break;
            }
        }
        assert!(ended, "{hz} Hz");
        assert!(s.p.grounded && s.p.vel.y == 0.0, "{hz} Hz");
        assert!(s.p.vel.planar_len() <= 190.0 + 1e-3);
        assert_eq!(lands, 0, "no Land event at the end");
        assert_eq!(s.p.stance.goal(), StanceKind::Crouch, "the deferred crouch press");
    }
}

// ---- the weapon -----------------------------------------------------------------------------------------------

#[test]
fn mantle_weapon_rows_gate_and_sprint() {
    let d = test_data();
    let md = d.mantle.as_ref().unwrap();
    let drops: Vec<bool> = md.rows.iter().map(|r| r.drop_weapon).collect();
    assert_eq!(drops, vec![true, true, true, true, true, false, false], "rows >= 33 in drop, 27 / 21 keep");
    assert_eq!(md.quick_times("ump45_mp"), (0.25, 0.55));
    let mut g = WeaponGate::default();
    assert_eq!(g.update(0.0, true, false, 0.55), Some(GateEvent::Lowered("mantle")));
    assert!(g.lowered() && g.blocked(0.1));
    assert_eq!(g.update(0.6, false, false, 0.55), Some(GateEvent::RaiseStart("mantle")));
    assert!(!g.lowered() && g.blocked(1.0), "blocked until the raise ends");
    assert_eq!(g.update(1.2, false, false, 0.55), Some(GateEvent::Raised("mantle")));
    assert!(!g.blocked(1.2));
    assert_eq!(g.update(2.0, false, true, 0.55), Some(GateEvent::Lowered("ladder")));
    // a sprint is force-ended by the mantle
    let mut s = Sim::new(world_with(wall(1.0, 200.0)), V3::new(0.0, 0.0, -150.0), 60.0);
    s.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, 0) });
    for _ in 0..40 {
        s.tick(keys(true, false, 0));
    }
    s.take();
    assert!(s.p.sprint.active());
    let mut ended = None;
    for _ in 0..40 {
        s.tick(keys(true, true, 0));
        for e in s.take() {
            if let SimEvent::Sprint(SprintEvent::End { why, .. }) = e {
                ended = Some(why);
            }
        }
        if s.p.mantling() {
            break;
        }
    }
    assert!(s.p.mantling());
    assert_eq!(ended, Some(EndWhy::Mantle));
    // a low row (27 / 21) with low_keeps_weapon: the gun stays up
    let (_, c, l) = ledge_for(0.6);
    let m = Mantle::plan(&l, V3::ZERO, 0.0, md, &c.mantle, 249.8, Trigger::Held, false);
    assert!(!m.drop_weapon, "row {} keeps the gun", l.row);
    let mut c2 = c.mantle;
    c2.low_keeps_weapon = false;
    assert!(Mantle::plan(&l, V3::ZERO, 0.0, md, &c2, 249.8, Trigger::Held, false).drop_weapon);
}


// ---- run 18: narrow tops and depenetration ----------------------------------------------------------------------

#[test]
fn mantle_detect_narrow_top_retry_run18() {
    // run 18: `edge` on rocks 0.5-1.4 m: a crest 16 in wide (narrower than the side probes at +-0.7 r = +-11 in) is
    // probed again at +-0.35 r and accepted; a 6 in post is still refused `edge`
    let crest = Convex::aabb(V3::new(-8.0, -10.0, FACE_Z), V3::new(8.0, 1.2 * M, FACE_Z + 80.0));
    let mut w = world_with(crest);
    let l = detect(&mut w, V3::ZERO, 0.0).expect("a 16 in crest");
    assert!(l.narrow_top && (l.ledge_y - 1.2 * M).abs() < 0.05, "{l:?}");
    assert!(l.rays <= DETECT_MAX_RAYS);
    let post = Convex::aabb(V3::new(-3.0, -10.0, FACE_Z), V3::new(3.0, 1.2 * M, FACE_Z + 80.0));
    let mut w = world_with(post);
    assert_eq!(why(detect(&mut w, V3::ZERO, 0.0)), Refuse::Edge);
    // a full-width wall needs no retry
    let mut w = world_with(wall(1.2, 80.0));
    assert!(!detect(&mut w, V3::ZERO, 0.0).unwrap().narrow_top);
}

#[test]
fn mantle_path_depenetrated_start_climbs_without_entering_geometry() {
    // run 18 (`path_blocked rays=3` x34 against a wagon): the capsule 1.9 in into a 1.0 m face; the climb starts from
    // the backed-off feet (r + skin from the face), ends on the top and the capsule radius never enters the wall
    for hz in [30.0, 60.0, 144.0] {
        let mut s = Sim::new(world_with(wall(1.0, 80.0)), V3::new(0.0, 0.0, FACE_Z - 15.0), hz);
        let (start, end, _) = mantle_run(&mut s, 2.5);
        let st = start.expect("a mantle from inside the face");
        assert!(st.depen > 1.5 && st.depen < 2.5 && (st.from.z - (FACE_Z - 15.0)).abs() < 1e-3, "{st:?}");
        assert!(end.is_some_and(|(_, _, p)| (p.y - 1.0 * M).abs() < 0.05), "{end:?}");
        assert!(s.capsule.is_none(), "{hz} Hz: {:?}", s.capsule);
        assert!(s.budget_ok(), "{hz} Hz: {:?} detect {}", s.over_budget, s.max_detect);
    }
}

// ---- 2026-10-21: a ledge whose top rises away (a hillside) -------------------------------------------------------

/// 2026-10-21 (12:54-13:12 session): a 1.15 m step whose top keeps rising away from the face at `deg` (the hillside
/// around it: slope_rays 26.7 deg), as one wedge (the face at FACE_Z, the top plane through its upper edge).
fn rising_top(h_m: f32, deg: f32) -> World {
    World::flat().with(Convex::wedge(-200.0, 200.0, FACE_Z, FACE_Z + 400.0, -10.0, V3::new(0.0, h_m * M, FACE_Z), deg, FWD))
}

#[test]
fn mantle_detect_rising_top_hillside_accepted() {
    // 2026-10-21 (`MOVE mantle check ok=0 why=path_blocked rise=0.00m rays=4` x25, then F5 and ER got up): R2's line
    // at feet + 60 in met a top rising away inside x + r. Before the fix every case here was refused path_blocked
    for (h, deg) in [(1.15f32, 27.0f32), (1.15, 30.0), (1.3, 20.0), (1.3, 25.0), (1.0, 35.0), (1.0, 40.0), (0.8, 44.0)] {
        let mut w = rising_top(h, deg);
        let l = detect(&mut w, V3::ZERO, 0.0).unwrap_or_else(|e| panic!("{h} m at {deg} deg: {e:?}"));
        assert_eq!(l.kind, MantleKind::Up, "{h} m at {deg} deg");
        // the end centre on the slope itself, x past the face
        let want = h * M + (l.end.z - FACE_Z) * deg.to_radians().tan();
        assert!((l.ledge_y - want).abs() < 0.2 && l.end.z > FACE_Z + 15.0, "{h} m at {deg} deg: {l:?}");
        assert!(l.rays <= DETECT_MAX_RAYS && l.line_up > 0.0, "{} rays, line_up {}", l.rays, l.line_up);
    }
    // MW2's window still holds: an end centre more than 60 in above the feet is too_high
    assert_eq!(why(detect(&mut rising_top(1.3, 30.0), V3::ZERO, 0.0)), Refuse::TooHigh);
    assert_eq!(why(detect(&mut rising_top(1.15, 40.0), V3::ZERO, 0.0)), Refuse::TooHigh);
    // a flat or gently rising top needs no second line (the 14 rays of before)
    for (h, deg) in [(1.15f32, 0.0f32), (1.15, 20.0), (1.0, 27.0)] {
        assert_eq!(detect(&mut rising_top(h, deg), V3::ZERO, 0.0).map(|l| (l.rays, l.line_up)).map_err(|e| e.0), Ok((14, 0.0)), "{h} m at {deg} deg");
    }
}

#[test]
fn mantle_detect_rising_top_fix_keeps_obstructions_blocked() {
    // what R2 meets above the top that is not the top itself is still path_blocked: a beam across the reach line
    // (feet + 59-65 in, 10-14 in past the face) over a flat top and over a 30 deg rising one (7 in above the slope)
    let beam = || Convex::aabb(V3::new(-200.0, 59.0, FACE_Z + 10.0), V3::new(200.0, 65.0, FACE_Z + 14.0));
    assert_eq!(why(detect(&mut world_with(wall(1.15, 80.0)).with(beam()), V3::ZERO, 0.0)), Refuse::PathBlocked, "beam, flat top");
    assert_eq!(why(detect(&mut rising_top(1.15, 30.0).with(beam()), V3::ZERO, 0.0)), Refuse::PathBlocked, "beam, rising top");
    // a second tier 14-26 in past the face, just above the line (1.6 m: the end centre on it is too_high, before it
    // R2's hit is no top) or a tall one (2.5 m: R2b meets it too)
    for z2 in [14.0f32, 20.0, 26.0] {
        for h2 in [1.6f32, 2.5] {
            let tier = Convex::aabb(V3::new(-200.0, -10.0, FACE_Z + z2), V3::new(200.0, h2 * M, FACE_Z + 120.0));
            let r = why(detect(&mut world_with(wall(1.15, 80.0)).with(tier), V3::ZERO, 0.0));
            assert!(matches!(r, Refuse::PathBlocked | Refuse::TooHigh), "tier {h2} m at {z2} in: {r:?}");
        }
    }
    // the overhanging lip and the face going on above the reach, as before
    let lip = Convex::aabb(V3::new(-200.0, 1.0 * M + 30.0, FACE_Z), V3::new(200.0, 1.0 * M + 60.0, FACE_Z + 10.0));
    assert_eq!(why(detect(&mut world_with(wall(1.0, 80.0)).with(lip), V3::ZERO, 0.0)), Refuse::PathBlocked);
    assert_eq!(why(detect(&mut world_with(wall(2.5, 80.0)), V3::ZERO, 0.0)), Refuse::TooHigh);
    // review: a crate (a wagon bed's cargo, a stepped stone ledge) on a flat top 26-34 in past the face, its top
    // between the reach line and R2b: R2 met its vertical face where a slope would be, the raised line passed over
    // it and the climb ran the capsule into it (accepted with line_up 12-18 in before the face check). Refused now
    for (h, dz, crate_top) in [(1.3f32, 28.0f32, 72.0f32), (1.3, 29.0, 77.0), (1.2, 32.0, 68.0), (1.4, 26.0, 75.0), (1.3, 34.0, 63.0)] {
        let crate_box = Convex::aabb(V3::new(-200.0, h * M - 1.0, FACE_Z + dz), V3::new(200.0, crate_top, FACE_Z + dz + 30.0));
        let r = detect(&mut world_with(wall(h, 80.0)).with(crate_box), V3::ZERO, 0.0);
        assert!(matches!(r, Err((Refuse::PathBlocked, _))), "crate {h} m / {dz} in / top {crate_top}: {r:?}");
    }
    // the run: the grounded check (line_up 12-18 in before) is refused path_blocked and no climb over a raised line
    // starts. Not covered (pre-existing, not R2b): from the jump the reach line passes over this crate (line_up 0)
    // and the end capsule's front still meets it (R8 stops at the end centre, R9 / R10 run above it)
    for hz in [60.0, 144.0] {
        let crate_box = Convex::aabb(V3::new(-200.0, 1.3 * M - 1.0, FACE_Z + 28.0), V3::new(200.0, 72.0, FACE_Z + 58.0));
        let mut s = Sim::new(world_with(wall(1.3, 80.0)).with(crate_box), V3::ZERO, hz);
        let (_, _, ev) = mantle_run(&mut s, 2.5);
        let first = ev.iter().find_map(|e| match e {
            SimEvent::MantleStart(m) => Some(Ok(m.air)),
            SimEvent::MantleRefuse { why, .. } => Some(Err(*why)),
            _ => None,
        });
        assert_eq!(first, Some(Err(Refuse::PathBlocked)), "{hz} Hz: the grounded check");
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::MantleStart(m) if m.line_up > 0.0)), "{hz} Hz: {ev:?}");
    }
}

#[test]
fn mantle_run_rising_top_climbs_onto_the_slope() {
    // the climb onto a 1.15 m step with a 30 deg top: ends grounded on the slope, the capsule never enters it
    for hz in [30.0, 60.0, 144.0] {
        let mut s = Sim::new(rising_top(1.15, 30.0), V3::ZERO, hz);
        let (start, end, _) = mantle_run(&mut s, 2.5);
        assert!(start.is_some_and(|m| m.kind == MantleKind::Up), "{hz} Hz: {start:?}");
        assert!(end.is_some() && s.p.grounded && s.p.pos.y > 1.15 * M, "{hz} Hz: {end:?} {:?}", s.p.pos);
        assert!(s.inside.is_none() && s.capsule.is_none(), "{hz} Hz: {:?} {:?}", s.inside, s.capsule);
        assert!(s.budget_ok(), "{hz} Hz: {:?} detect {}", s.over_budget, s.max_detect);
    }
}
