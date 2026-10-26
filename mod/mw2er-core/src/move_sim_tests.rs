//! Tests of [`super`] (the controller against the exact synthetic world), docs/M5_DESIGN.md section 13.1.

use super::*;
use crate::move_collide::synth::{Convex, World};
use crate::movement_data::test_data;

const M: f32 = 1.0 / 0.0254;

struct Run {
    p: Player,
    d: MoveData,
    cfg: SimCfg,
    w: World,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
}

fn fwd() -> V3 {
    V3::new(0.0, 0.0, 1.0)
}
fn right() -> V3 {
    V3::new(1.0, 0.0, 0.0)
}

fn walk() -> Input {
    Input { w: true, fwd_h: fwd(), right_h: right(), ..Input::default() }
}

fn idle() -> Input {
    Input { fwd_h: fwd(), right_h: right(), ..Input::default() }
}

impl Run {
    fn new(w: World, pos: V3, hz: f64) -> Run {
        let d = test_data();
        let cfg = SimCfg::defaults(&d);
        Run { p: Player::new(&d, pos), d, cfg, w, t: 0.0, hz, ev: Vec::new() }
    }
    fn tick(&mut self, i: Input) -> TickStats {
        self.t += 1.0 / self.hz;
        self.p.tick(&self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev)
    }
    fn run(&mut self, secs: f64, i: Input) {
        let end = self.t + secs;
        while self.t < end - 1e-9 {
            self.tick(i);
        }
    }
    fn speed(&self) -> f32 {
        self.p.vel.planar_len()
    }
    fn take(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.ev)
    }
}

fn flat(hz: f64) -> Run {
    Run::new(World::flat(), V3::ZERO, hz)
}

#[test]
fn pmove_accel_reaches_190_within_020s_at_30_60_144() {
    for hz in [30.0, 60.0, 144.0] {
        let mut r = flat(hz);
        let mut reached = None;
        for _ in 0..(hz as usize) {
            r.tick(walk());
            if reached.is_none() && r.speed() >= 189.99 {
                reached = Some(r.t);
            }
        }
        let t = reached.unwrap_or(9.0);
        assert!(t <= 0.20 + 1e-6, "{hz} Hz reached 190 at {t}");
        assert!((r.speed() - 190.0).abs() < 1e-2);
    }
}

#[test]
fn pmove_friction_stop_time_30_60_144() {
    for hz in [30.0, 60.0, 144.0] {
        let mut r = flat(hz);
        r.run(1.0, walk());
        let t0 = r.t;
        let mut stop = None;
        for _ in 0..(hz as usize) {
            r.tick(idle());
            if stop.is_none() && r.speed() == 0.0 {
                stop = Some(r.t - t0);
            }
        }
        let s = stop.unwrap();
        assert!((s - 0.30).abs() <= 0.035, "{hz}: {s}");
    }
}

#[test]
fn pmove_jump_apex_and_air_time() {
    for hz in [30.0, 60.0, 144.0] {
        let mut r = flat(hz);
        r.run(0.6, idle());
        r.take();
        r.tick(Input { jump_presses: 1, ..idle() });
        let t_jump = r.t - 1.0 / hz;
        let mut apex = None;
        let mut land = None;
        for _ in 0..(2 * hz as usize) {
            r.tick(idle());
            for e in r.take() {
                match e {
                    SimEvent::Apex { h, t, .. } => apex = Some((h, t)),
                    SimEvent::Land(l) if land.is_none() => land = Some((r.t - t_jump, l)),
                    _ => {}
                }
            }
        }
        let (h, ta) = apex.unwrap();
        assert!((h - 39.0).abs() <= 0.1, "{hz}: apex {h}");
        assert!((ta - 0.3122).abs() <= 1.0 / hz + 1e-6, "{hz}: t_apex {ta}");
        let (air, l) = land.unwrap();
        assert!((air - 0.6245).abs() <= 1.0 / hz + 1e-6, "{hz}: air {air}");
        assert!((l.fall_h - 39.0).abs() < 0.5, "{hz}: fall_h {}", l.fall_h);
        assert!(r.p.pos.y.abs() < 1e-3);
    }
}

#[test]
fn pmove_jump_running_distance() {
    for hz in [30.0, 60.0, 144.0] {
        let mut r = flat(hz);
        r.run(1.0, walk());
        r.take();
        let z0 = r.p.pos.z;
        r.tick(Input { jump_presses: 1, ..walk() });
        let mut dist = None;
        for _ in 0..(2 * hz as usize) {
            r.tick(walk());
            if dist.is_none() && r.take().iter().any(|e| matches!(e, SimEvent::Land(_))) {
                dist = Some(r.p.pos.z - z0);
            }
        }
        let dz = dist.unwrap();
        assert!((dz - 118.6).abs() <= 2.0 + 190.0 / hz as f32, "{hz}: {dz}");
    }
}

#[test]
fn pmove_jump_rules_interval_hold_and_crouch() {
    let mut r = flat(60.0);
    r.run(0.6, idle());
    r.tick(Input { jump_presses: 1, ..idle() });
    // holding space (no new press) never jumps again
    r.run(2.0, idle());
    let jumps = r.take().iter().filter(|e| matches!(e, SimEvent::Jump { .. })).count();
    assert_eq!(jumps, 1);
    // < 500 ms after the last jump: refused
    r.p.last_jump_t = r.t;
    r.tick(Input { jump_presses: 1, ..idle() });
    assert!(!r.take().iter().any(|e| matches!(e, SimEvent::Jump { .. })), "within 500 ms");
    r.run(0.5, idle());
    r.tick(Input { jump_presses: 1, ..idle() });
    assert!(r.take().iter().any(|e| matches!(e, SimEvent::Jump { .. })));
    r.run(1.5, idle());
    // crouch + space: stands up, no jump
    r.tick(Input { crouch_presses: 1, ..idle() });
    r.run(0.5, idle());
    assert_eq!(r.p.stance.settled(), Some(StanceKind::Crouch));
    r.take();
    r.tick(Input { jump_presses: 1, ..idle() });
    let ev = r.take();
    assert!(!ev.iter().any(|e| matches!(e, SimEvent::Jump { .. })));
    assert_eq!(r.p.stance.goal(), StanceKind::Stand);
    assert!(r.p.grounded);
}

#[test]
fn pmove_jump_ceiling_stops_rise() {
    // a ceiling 20 in above the head (scaled stand hull)
    let d = test_data();
    let cfg = SimCfg::defaults(&d);
    let hull = d.mw2.hull_stand * cfg.v_scale;
    let ceil_y = hull + 20.0;
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, ceil_y, -500.0), V3::new(500.0, ceil_y + 10.0, 500.0)));
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.run(0.6, idle());
    r.tick(Input { jump_presses: 1, ..idle() });
    let mut top = 0.0f32;
    let mut apex = None;
    for _ in 0..90 {
        r.tick(idle());
        top = top.max(r.p.pos.y);
        for e in r.take() {
            if let SimEvent::Apex { h, ceiling, .. } = e {
                apex = Some((h, ceiling));
            }
        }
    }
    assert!(top + hull <= ceil_y + 1e-3, "head {} vs ceiling {ceil_y}", top + hull);
    assert!(top < 20.5);
    let (h, c) = apex.unwrap();
    assert!(c && h < 20.5);
}

fn land_speed(slowdown: bool) -> f32 {
    let mut r = flat(60.0);
    r.cfg.jump_slowdown = slowdown;
    r.run(1.0, walk());
    r.tick(Input { jump_presses: 1, ..walk() });
    for _ in 0..90 {
        r.tick(walk());
        for e in r.take() {
            if let SimEvent::Land(_) = e {
                return r.speed();
            }
        }
    }
    panic!("no landing");
}

#[test]
fn pmove_land_running_jump_slowdown() {
    let s = land_speed(true);
    assert!((s - 82.745).abs() < 0.5, "{s}");
    let s = land_speed(false);
    assert!((s - 127.3).abs() < 0.5, "{s}");
}

#[test]
fn pmove_land_rejump_land_factor() {
    for (wait_ms, want_v0) in [(0.0f32, 158.0f32), (950.0, 188.8)] {
        let mut r = flat(60.0);
        r.run(0.6, idle());
        r.tick(Input { jump_presses: 1, ..idle() });
        loop {
            r.tick(idle());
            if r.take().iter().any(|e| matches!(e, SimEvent::Land(_))) {
                break;
            }
        }
        assert!((r.p.slow_ms - 1800.0).abs() < 20.0, "{}", r.p.slow_ms);
        // jump when pm_time is 1800 (right away) or 850 (after 950 ms); the substep timer runs first
        r.p.slow_ms = 1800.0 - wait_ms + 1000.0 / 60.0;
        r.p.last_jump_t = f64::NEG_INFINITY;
        r.tick(Input { jump_presses: 1, ..idle() });
        let v0 = r.take().iter().find_map(|e| if let SimEvent::Jump { v0, .. } = e { Some(*v0) } else { None }).unwrap();
        assert!((v0 - want_v0).abs() < 0.2, "wait {wait_ms}: v0 {v0}");
    }
}

#[test]
fn pmove_land_band_above_origin_plus_18() {
    // jump onto a 20 in platform: landing above origin + 18 -> x0.5 / 1200 ms
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, 45.0), V3::new(500.0, 20.0, 2000.0)));
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.run(0.2, idle());
    r.p.vel = V3::new(0.0, 0.0, 190.0);
    r.tick(Input { jump_presses: 1, ..walk() });
    let mut land = None;
    for _ in 0..90 {
        r.tick(walk());
        for e in r.take() {
            if let SimEvent::Land(l) = e {
                land.get_or_insert(l);
            }
        }
    }
    let l = land.unwrap();
    assert!((r.p.pos.y - 20.0).abs() < 0.1, "on the platform: {}", r.p.pos.y);
    assert_eq!((l.slow_b, l.slow_ms), (0.5, 1200.0));
}

#[test]
fn fall_freefall_handback_after_30m() {
    let w = World::default(); // no floor at all
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.p.grounded = false;
    let mut fallen = None;
    for _ in 0..600 {
        r.tick(idle());
        for e in r.take() {
            if let SimEvent::Freefall { fallen: f } = e {
                fallen.get_or_insert(f);
            }
        }
    }
    let f = fallen.unwrap();
    assert!(f > 30.0 * M && f < 31.0 * M, "{}", f / M);
}

#[test]
fn fall_drop_damage_6m_and_9m() {
    for (hm, want) in [(6.0f32, 24.1f32), (9.0, 100.0)] {
        let w = World::flat().with(Convex::aabb(V3::new(-100.0, -100.0, -100.0), V3::new(100.0, hm * M, 20.0)));
        let mut r = Run::new(w, V3::new(0.0, hm * M, 0.0), 30.0);
        r.run(0.1, idle());
        let mut land = None;
        for _ in 0..200 {
            r.tick(walk());
            for e in r.take() {
                if let SimEvent::Land(l) = e {
                    land.get_or_insert(l);
                }
            }
        }
        let l = land.unwrap();
        assert!((l.dmg_pct - want).abs() < 0.3, "{hm} m: {} ({} in)", l.dmg_pct, l.fall_h);
    }
}

#[test]
fn move_collide_ground_miss_kept_while_er_grounded() {
    // review M5 major 1: no floor for our rays (a lift / platform the map filter misses) while ER stands on it
    for hz in [30.0, 60.0, 144.0] {
        let mut r = Run::new(World::default(), V3::ZERO, hz);
        let held = Input { er_ground: Some(true), ..walk() };
        let mut n_held = 0;
        for _ in 0..(hz as usize) {
            n_held += r.tick(held).ground_held;
        }
        assert!(r.p.grounded && r.p.pos.y == 0.0, "{hz}: feet kept {:?}", r.p.pos);
        assert!(n_held >= hz as u32, "{hz}: every substep counted ({n_held})");
        // ER also airborne: the fall starts at once
        r.tick(Input { er_ground: Some(false), ..walk() });
        assert!(!r.p.grounded, "{hz}: ER airborne too -> fall");
        // no engine (bench / tests): the ray alone decides, as before
        let mut r = Run::new(World::default(), V3::ZERO, hz);
        r.tick(walk());
        assert!(!r.p.grounded);
    }
}

#[test]
fn fall_damage_needs_er_airborne_for_the_fall() {
    // a 9 m drop (lethal in MW2 terms): damage is trusted only if ER saw the player airborne after the grace ticks
    for (er, confirmed) in [(None, true), (Some(false), true), (Some(true), false)] {
        let w = World::flat();
        let mut r = Run::new(w, V3::new(0.0, 9.0 * M, 0.0), 60.0);
        r.p.grounded = false;
        let mut land = None;
        for _ in 0..240 {
            r.tick(Input { er_ground: er, ..idle() });
            for e in r.take() {
                if let SimEvent::Land(l) = e {
                    land.get_or_insert(l);
                }
            }
        }
        let l = land.expect("landed");
        assert!(l.dmg_pct >= 99.9, "{er:?}: {}", l.dmg_pct);
        assert_eq!(r.p.fall_er_confirmed(), confirmed, "{er:?}: er_ground_air={}", r.p.er_ground_air);
    }
    // ER reporting ground only in the first grace ticks (it reads our previous write) does not veto the damage
    let mut r = Run::new(World::flat(), V3::new(0.0, 9.0 * M, 0.0), 60.0);
    r.p.grounded = false;
    for k in 0..240 {
        r.tick(Input { er_ground: Some(k < ER_AIR_GRACE_TICKS as usize), ..idle() });
    }
    assert!(r.p.grounded && r.p.fall_er_confirmed());
}

#[test]
fn move_collide_pushout_stopped_by_wall() {
    // review M5 major 3: a character pushing the player into a wall at 0.2 m per tick never moves him through it
    for hz in [30.0, 60.0, 144.0] {
        let wall_z = 1.0 * M;
        let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, wall_z), V3::new(500.0, 3.0 * M, wall_z + 0.3 * M)));
        let mut r = Run::new(w, V3::ZERO, hz);
        let push = Input { push: V3::new(0.0, 0.0, 0.2 * M), ..idle() };
        for _ in 0..(2 * hz as usize) {
            r.tick(push);
            assert!(r.p.pos.z + r.cfg.radius <= wall_z + 1e-3, "{hz}: z {} into the wall at {wall_z}", r.p.pos.z);
        }
        assert!(r.p.pos.z + r.cfg.radius > wall_z - r.cfg.skin - 1.0, "{hz}: pushed up to the wall ({})", r.p.pos.z);
        // a vertical push component is ignored, a non-finite one too
        let before = r.p.pos;
        r.tick(Input { push: V3::new(0.0, 50.0, 0.0), ..idle() });
        r.tick(Input { push: V3::new(f32::NAN, 0.0, 0.0), ..idle() });
        assert!((r.p.pos.y - before.y).abs() < 1e-3 && r.p.pos.is_finite());
    }
}

#[test]
fn move_collide_wall_at_45_keeps_tangent_and_skin() {
    for hz in [30.0, 60.0, 144.0] {
        let wall_z = 3.0 * M;
        let w = World::flat().with(Convex::aabb(V3::new(-50.0 * M, 0.0, wall_z), V3::new(50.0 * M, 3.0 * M, wall_z + 20.0)));
        let mut r = Run::new(w, V3::ZERO, hz);
        let diag = V3::new(1.0, 0.0, 1.0).normalize().unwrap();
        let side = V3::new(1.0, 0.0, -1.0).normalize().unwrap();
        let i = Input { w: true, fwd_h: diag, right_h: side, ..Input::default() };
        let mut min_gap = f32::MAX;
        for _ in 0..(3 * hz as usize) {
            r.tick(i);
            min_gap = min_gap.min(wall_z - r.p.pos.z - r.cfg.radius);
        }
        assert!(min_gap >= r.cfg.skin - 0.04, "{hz}: gap {min_gap} in (skin {})", r.cfg.skin);
        // along the wall the tangent component of the wish survives (Q3/IW slide: at least 190 cos 45)
        assert!(r.p.vel.x >= 190.0 * 0.7071 * 0.99, "{hz}: {}", r.p.vel.x);
        assert!(r.p.vel.z.abs() < 1.0);
    }
}

#[test]
fn move_collide_corner_stops() {
    let w = World::flat()
        .with(Convex::aabb(V3::new(-500.0, 0.0, 100.0), V3::new(500.0, 200.0, 120.0)))
        .with(Convex::aabb(V3::new(100.0, 0.0, -500.0), V3::new(120.0, 200.0, 500.0)));
    let mut r = Run::new(w, V3::ZERO, 60.0);
    let diag = V3::new(1.0, 0.0, 1.0).normalize().unwrap();
    let i = Input { w: true, fwd_h: diag, right_h: V3::new(1.0, 0.0, -1.0).normalize().unwrap(), ..Input::default() };
    r.run(4.0, i);
    let p = r.p.pos;
    assert!(p.x <= 100.0 - r.cfg.radius + 0.5 && p.z <= 100.0 - r.cfg.radius + 0.5, "{p:?}");
    assert!(r.speed() < 60.0, "pinned in the corner: {}", r.speed());
}

fn stairs(n: usize, rise_m: f32, tread_m: f32, z0: f32) -> World {
    let mut w = World::flat();
    for k in 0..n {
        let z = z0 + k as f32 * tread_m * M;
        w = w.with(Convex::aabb(V3::new(-100.0, -10.0, z), V3::new(100.0, (k + 1) as f32 * rise_m * M, z0 + 100.0 * M)));
    }
    w
}

#[test]
fn move_collide_stairs_climbed_walk_and_sprint() {
    for (sprint, hz) in [(false, 60.0), (true, 60.0), (false, 30.0), (true, 144.0)] {
        let z0 = 30.0;
        let mut r = Run::new(stairs(10, 0.2, 0.3, z0), V3::ZERO, hz);
        let top = 10.0 * 0.2 * M;
        r.tick(Input { sprint_presses: sprint as u32, sprint_held: sprint, ..walk() });
        let mut t_top = None;
        for _ in 0..(4 * hz as usize) {
            r.tick(walk());
            // feet never below the tread under the centre
            let k = ((r.p.pos.z - z0) / (0.3 * M)).floor();
            if (0.0..10.0).contains(&k) {
                let tread = (k + 1.0) * 0.2 * M;
                assert!(r.p.pos.y >= tread - 0.5 || !r.p.grounded, "below tread {k}: {} < {tread}", r.p.pos.y);
            }
            if t_top.is_none() && (r.p.pos.y - top).abs() < 0.5 {
                t_top = Some(r.t);
            }
        }
        let t = t_top.unwrap_or(99.0);
        assert!(t < 2.5, "sprint={sprint} {hz}: top at {t}");
    }
}

#[test]
fn move_collide_block_heights_wall_vs_step_prone() {
    // 0.5 m block: a wall standing (step 18 in = 0.457 m)
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, 40.0), V3::new(500.0, 0.5 * M, 400.0)));
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.run(2.0, walk());
    assert!(r.p.pos.y < 1.0 && r.p.pos.z < 40.0, "{:?}", r.p.pos);
    // 0.3 m block: climbed standing, a wall prone (step 10 in = 0.254 m)
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, 40.0), V3::new(500.0, 0.3 * M, 400.0)));
    let mut r = Run::new(w.clone(), V3::ZERO, 60.0);
    r.run(2.0, walk());
    assert!((r.p.pos.y - 0.3 * M).abs() < 0.5, "standing climbs 0.3 m: {:?}", r.p.pos);
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.tick(Input { prone_presses: 1, ..idle() });
    r.run(1.0, idle());
    assert_eq!(r.p.stance.settled(), Some(StanceKind::Prone));
    r.run(8.0, walk());
    assert!(r.p.pos.y < 1.0 && r.p.pos.z < 40.0, "prone blocked: {:?}", r.p.pos);
}

#[test]
fn move_collide_ramp_30_walkable_both_ways() {
    let ramp = Convex::ramp(-500.0, 500.0, 40.0, 0.0, 30.0, 2.0 * M);
    let w = World::flat().with(ramp);
    let mut r = Run::new(w, V3::ZERO, 60.0);
    r.run(4.0, walk());
    assert!((r.p.pos.y - 2.0 * M).abs() < 1.0, "up the ramp: {:?}", r.p.pos);
    // back down: never leaves the ground (snap)
    let back = Input { s: true, ..idle() };
    let mut airborne = 0;
    for _ in 0..360 {
        r.tick(back);
        if !r.p.grounded {
            airborne += 1;
        }
    }
    assert_eq!(airborne, 0);
    assert!(r.p.pos.y < 1.0, "down again: {:?}", r.p.pos);
}

#[test]
fn move_collide_ramp_50_slides_down() {
    let ramp = Convex::ramp(-500.0, 500.0, 0.0, 0.0, 50.0, 4.0 * M);
    let w = World::flat().with(ramp);
    // standing on the slope 1.5 m up
    let z = 1.5 * M / 50f32.to_radians().tan();
    let mut r = Run::new(w, V3::new(0.0, 1.5 * M, z), 60.0);
    r.run(3.0, idle());
    assert!(r.p.pos.y < 0.2 * M, "slid down: {:?}", r.p.pos);
    // walking into it: blocked like a wall
    let ramp = Convex::ramp(-500.0, 500.0, 40.0, 0.0, 50.0, 4.0 * M);
    let mut r = Run::new(World::flat().with(ramp), V3::ZERO, 60.0);
    r.run(3.0, walk());
    assert!(r.p.pos.y < 0.5 * M, "50 deg not climbed: {:?}", r.p.pos);
}

#[test]
fn move_collide_ledge_2m_falls_and_lands() {
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, -100.0, -500.0), V3::new(500.0, 2.0 * M, 20.0)));
    let mut r = Run::new(w, V3::new(0.0, 2.0 * M, 0.0), 60.0);
    let mut land = None;
    for _ in 0..240 {
        r.tick(walk());
        for e in r.take() {
            if let SimEvent::Land(l) = e {
                land.get_or_insert(l);
            }
        }
    }
    let l = land.unwrap();
    assert!((l.fall_h - 78.74).abs() < 0.5, "{}", l.fall_h);
    assert_eq!(l.dmg_pct, 0.0);
    assert!(r.p.pos.y.abs() < 1e-3);
}

#[test]
fn move_collide_thin_wall_not_crossed() {
    for hz in [15.0, 30.0, 60.0] {
        for mult in [1.0f32, 3.0] {
            let wall_z = 2.0 * M;
            let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, wall_z), V3::new(500.0, 3.0 * M, wall_z + 0.05 * M)));
            let mut r = Run::new(w, V3::ZERO, hz);
            r.cfg.speed_scale = mult;
            r.tick(Input { sprint_presses: 1, sprint_held: true, ..walk() });
            r.run(3.0, walk());
            assert!(r.p.pos.z < wall_z, "{hz} Hz x{mult}: z {} wall {wall_z}", r.p.pos.z);
        }
    }
}

#[test]
fn move_collide_fast_fall_stops_on_floor_30hz() {
    let mut r = Run::new(World::flat(), V3::new(0.0, 2.0 * M, 0.0), 30.0);
    r.p.grounded = false;
    r.p.vel = V3::new(0.0, -50.0 * M, 0.0);
    r.run(1.0, idle());
    assert!(r.p.grounded && r.p.pos.y.abs() < 1e-3, "{:?}", r.p.pos);
}

#[test]
fn move_collide_budget_never_exceeded_and_drops_horizontal_only() {
    let w = World::flat()
        .with(Convex::aabb(V3::new(-500.0, 0.0, 100.0), V3::new(500.0, 200.0, 120.0)))
        .with(Convex::aabb(V3::new(100.0, 0.0, -500.0), V3::new(120.0, 200.0, 500.0)));
    let mut r = Run::new(w, V3::ZERO, 15.0);
    let diag = V3::new(1.0, 0.0, 1.0).normalize().unwrap();
    let i = Input { w: true, fwd_h: diag, right_h: V3::new(1.0, 0.0, -1.0).normalize().unwrap(), ..Input::default() };
    for _ in 0..60 {
        let st = r.tick(i);
        assert!(st.rays <= r.cfg.max_rays, "{}", st.rays);
    }
    // a tiny budget: the horizontal move is dropped, the ground ray still runs (no fall through the floor)
    let mut r = flat(30.0);
    r.cfg.max_rays = 4;
    r.run(1.0, walk());
    assert!(r.p.grounded && r.p.pos.y.abs() < 1e-3);
    let st = r.tick(walk());
    assert!(st.rays <= 4 && st.budget_hit, "{st:?}");
}

#[test]
fn move_collide_strafe_and_back_speeds() {
    let mut r = flat(60.0);
    r.run(1.0, Input { a: true, ..idle() });
    assert!((r.speed() - 152.0).abs() < 0.01);
    assert!(r.p.vel.x < 0.0, "A moves along -right");
    let mut r = flat(60.0);
    r.run(1.0, Input { s: true, ..idle() });
    assert!((r.speed() - 133.0).abs() < 0.01);
}

#[test]
fn move_collide_sprint_speed_and_duration() {
    let mut r = flat(60.0);
    r.run(1.0, walk());
    r.tick(Input { sprint_presses: 1, sprint_held: true, ..walk() });
    r.run(1.0, walk());
    assert!((r.speed() - 285.0).abs() < 0.01, "{}", r.speed());
    let mut end = None;
    for _ in 0..300 {
        r.tick(walk());
        for e in r.take() {
            if let SimEvent::Sprint(SprintEvent::End { why, dur, .. }) = e {
                end.get_or_insert((why, dur));
            }
        }
    }
    let (why, dur) = end.unwrap();
    assert_eq!(why, EndWhy::Time);
    assert!((dur - 4.0).abs() < 0.034, "{dur}");
}


/// C3 PASS B (docs/C3_DESIGN.md 4.5): the left stick's analog usercmd: full forward is full speed and sprints, half
/// a stick walks slower and never sprints (pmove's forwardmove > 105 gate), keys win over the stick.
#[test]
fn c3_pad_analog_cmd_speed_and_sprint_gate() {
    let stick = |fm: f32, rm: f32| Input { analog: Some((fm, rm)), ..idle() };
    let mut r = flat(60.0);
    r.run(1.0, stick(127.0, 0.0));
    assert!((r.speed() - 190.0).abs() < 1e-2, "full stick = full walk speed: {}", r.speed());
    r.tick(Input { sprint_presses: 1, sprint_held: true, ..stick(127.0, 0.0) });
    r.run(0.5, stick(127.0, 0.0));
    assert!(r.p.sprint.active(), "full forward sprints");
    let mut r = flat(60.0);
    r.run(1.0, stick(64.0, 0.0));
    let half = r.speed();
    assert!(half > 60.0 && half < 120.0, "half stick walks at about half speed: {half}");
    r.tick(Input { sprint_presses: 1, sprint_held: true, ..stick(100.0, 0.0) });
    r.run(0.5, stick(100.0, 0.0));
    assert!(!r.p.sprint.active(), "forwardmove 100 never sprints");
    // keys win; a NaN stick is no movement
    assert_eq!(Input { w: true, ..stick(-127.0, 0.0) }.cmd(), crate::pmove::Cmd::from_keys(true, false, false, false));
    assert_eq!(stick(f32::NAN, 300.0).cmd(), crate::pmove::Cmd { forwardmove: 0.0, rightmove: 127.0 });
    assert!(stick(1.0, 0.0).wish() && !idle().wish());
}


// ---- 2026-10-21: the roof (a 21.2 deg slope the object filter 0x5b sees) ----

/// The controller on a layered world (the DLL's union of the map and object filters).
fn roof_run(objs: Vec<Convex>, pos: V3) -> (Player, MoveData, SimCfg, crate::move_collide::synth::Layered) {
    let d = test_data();
    let cfg = SimCfg::defaults(&d);
    let l = crate::move_collide::synth::Layered { map: World::flat(), objects: World { solids: objs, rays: 0 }, mode: 0, hits: [0; 2] };
    (Player::new(&d, pos), d, cfg, l)
}

/// A roof rising along +z from z = 40 in at `deg`, 6 m high, as an object (the map filter does not see it).
fn roof(deg: f32) -> Convex {
    Convex::ramp(-50.0 * M, 50.0 * M, 40.0, 0.0, deg, 6.0 * M)
}

/// Run of 2026-10-21 12:23:27-12:23:40 (`MOVE state ... slope_rays=21.2deg slope_er=21.2deg ... speed_sim=0.7in/s
/// wish=166.2 ... hits=map:0/obj:..`): a jump on an uphill roof left a remembered contact plane from the low ray (the
/// slope met 10 in ahead, airborne, so no floor exemption); after the landing its one-ray re-check hit the slope again
/// every substep and removed the whole uphill move: stuck until the player turned away. Now the walk continues.
#[test]
fn move_collide_roof_jump_then_walk_up_object_slope() {
    for hz in [30.0, 60.0, 144.0] {
        for deg in [15.0f32, 21.2, 27.0] {
            let (mut p, d, cfg, mut w) = roof_run(vec![roof(deg)], V3::ZERO);
            let mut ev = Vec::new();
            let mut t = 0.0;
            let mut step = |p: &mut Player, w: &mut crate::move_collide::synth::Layered, i: Input, t: &mut f64| -> TickStats {
                *t += 1.0 / hz;
                p.tick(&d, &cfg, &i, *t, 1.0 / hz, w, &mut ev)
            };
            for _ in 0..(hz as usize) {
                step(&mut p, &mut w, walk(), &mut t);
            }
            assert!(p.pos.y > 10.0 && p.grounded, "{hz} Hz {deg} deg: on the roof {:?}", p.pos);
            for jump in 0..3 {
                step(&mut p, &mut w, Input { jump_presses: 1, jump_held: true, ..walk() }, &mut t);
                for _ in 0..(hz as usize * 7 / 10) {
                    step(&mut p, &mut w, walk(), &mut t);
                }
                assert!(p.grounded, "{hz} Hz {deg} deg jump {jump}: landed {:?}", p.pos);
                assert_eq!(p.ncontacts, 0, "{hz} Hz {deg} deg jump {jump}: the slope is not a remembered wall");
                let y0 = p.pos.y;
                for _ in 0..(hz as usize / 2) {
                    step(&mut p, &mut w, walk(), &mut t);
                }
                let climbed = p.pos.y - y0;
                // 0.5 s after the landing slowdown: at least ~100 in/s along the ground (190 on flat; was 0)
                let want = 0.5 * 100.0 * deg.to_radians().sin();
                assert!(climbed > want || p.pos.y > 6.0 * M - 1.0, "{hz} Hz {deg} deg jump {jump}: climbed {climbed:.1} in (want {want:.1}), speed {:.1}", p.vel.planar_len());
            }
            assert!(w.hits[1] > 0, "the roof came through the object layer");
            assert!(w.all().capsule_inside(p.pos, 70.0, cfg.radius, 8, 1.0).is_none(), "never inside: {:?}", p.pos);
        }
    }
}

/// The same roof as shingle rows: each 0.5 m row of the 21.2 deg slope starts 0.6 in above the row below it (walking
/// up meets a small lip), ending in a drop at the top. No stall walking up, none after a jump, walking up the slope is
/// no penetration evidence for the unstick detector (the low ray meets the slope inside the radius: floor, not a wall),
/// and the top edge is walked over (the low ray clipping the crest with nothing past it was a wall 10 in short of it).
#[test]
fn move_collide_roof_shingles_no_stall_no_inside_evidence() {
    let deg = 21.2f32;
    let tan = deg.to_radians().tan();
    let row = 0.5 * M;
    let lip = 0.6;
    const ROWS: usize = 16;
    let top = ROWS as f32 * row * tan;
    let end_z = 40.0 + ROWS as f32 * row;
    let objs: Vec<Convex> = (0..ROWS)
        .map(|k| {
            let z0 = 40.0 + k as f32 * row;
            let at = V3::new(0.0, (z0 - 40.0) * tan + lip, z0);
            Convex::wedge(-50.0 * M, 50.0 * M, z0, z0 + row, -10.0, at, deg, V3::new(0.0, 0.0, 1.0))
        })
        .collect();
    for hz in [30.0, 60.0, 144.0] {
        let (mut p, d, cfg, mut w) = roof_run(objs.clone(), V3::ZERO);
        let mut ev = Vec::new();
        let mut t = 0.0;
        let mut inside = 0;
        let mut slow = 0;
        let mut run_slow = 0;
        let mut on_roof = 0;
        let mut max_y = 0.0f32;
        for k in 0..(4 * hz as usize) {
            t += 1.0 / hz;
            let jump = k == hz as usize;
            let i = Input { jump_presses: jump as u32, jump_held: jump, ..walk() };
            // the landing tick is not counted: an airborne low hit on the slope still stops the jump's horizontal
            // move (as before; the walk then accelerates again)
            let was_grounded = p.grounded;
            let st = p.tick(&d, &cfg, &i, t, 1.0 / hz, &mut w, &mut ev);
            max_y = max_y.max(p.pos.y);
            if was_grounded && p.grounded && p.pos.z > 40.0 + 0.5 * M && p.pos.y < top - 2.0 {
                on_roof += 1;
                inside += st.inside;
                // stuck: under 30 in/s for 0.1 s (the first ticks after the landing re-accelerate from 0)
                run_slow = if p.vel.planar_len() < 30.0 { run_slow + 1 } else { 0 };
                if run_slow as f64 >= 0.1 * hz {
                    slow += 1;
                }
            }
        }
        assert!(on_roof > hz as usize / 2, "{hz} Hz: walked on the roof ({on_roof} ticks) {:?}", p.pos);
        assert_eq!(slow, 0, "{hz} Hz: stalled ticks on the shingles {:?}", p.pos);
        assert_eq!(inside, 0, "{hz} Hz: inside evidence on a walkable slope");
        assert!(max_y > top - 2.0, "{hz} Hz: reached the top edge: {max_y} of {top}");
        assert!(p.pos.z > end_z && p.grounded && p.pos.y.abs() < 1e-3, "{hz} Hz: walked over the edge and down: {:?}", p.pos);
    }
}

/// The guard of the fix: an airborne low hit on something that is not floor within a step (a 0.6 m object crate:
/// above the 18 in step) is still remembered and still stops the walk after the landing (wagons and crates stay
/// solid), and the floor exemption never lets the capsule into it.
#[test]
fn move_collide_roof_fix_keeps_object_crate_solid_after_a_jump() {
    for hz in [30.0, 60.0, 144.0] {
        let crate_z = 1.2 * M;
        let objs = vec![Convex::aabb(V3::new(-1.0 * M, 0.0, crate_z), V3::new(1.0 * M, 0.6 * M, crate_z + 1.0 * M))];
        let (mut p, d, cfg, mut w) = roof_run(objs, V3::ZERO);
        let mut ev = Vec::new();
        let mut t = 0.0;
        // a short hop (released early by a 0.1 s walk-in) toward the crate, then keep walking into it
        t += 1.0 / hz;
        p.tick(&d, &cfg, &Input { jump_presses: 1, jump_held: true, mantle_block: true, ..walk() }, t, 1.0 / hz, &mut w, &mut ev);
        let mut remembered = false;
        for _ in 0..(3 * hz as usize) {
            t += 1.0 / hz;
            p.tick(&d, &cfg, &Input { mantle_block: true, ..walk() }, t, 1.0 / hz, &mut w, &mut ev);
            remembered |= p.ncontacts > 0;
            assert!(p.pos.z + cfg.radius <= crate_z + 1e-3, "{hz} Hz: into the crate {:?}", p.pos);
        }
        assert!(remembered, "{hz} Hz: the crate face is a remembered wall");
        assert!(p.grounded && p.pos.y.abs() < 1e-3, "{hz} Hz: stopped on the floor in front of it {:?}", p.pos);
    }
}
