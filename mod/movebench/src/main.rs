//! movebench: the headless bench of docs/M5_DESIGN.md section 13.2.
//!
//! Runs the real `mw2er_core` controller (pmove + collide + stance + sprint + fall) with the converted MW2 data
//! (`converted\movement.json`) on scripted inputs in synthetic test maps, at 30, 60 and 144 Hz, and compares with
//! hand-computed MW2 numbers. Offline only: no game, no Win32, never linked into mw2er.dll.
//!
//! Usage: `movebench --check [--root <dir>]` (writes `work\m5\bench.json`, exit 1 on any miss),
//!        `movebench --perf [--root <dir>]` (10,000 ticks of B9 at 30 Hz: CPU p95 < 50 us, rays_max <= max_rays, 48).

use std::path::PathBuf;
use std::time::Instant;

mod c3_bench;
mod mantle_bench;
mod run18_bench;

use mw2er_core::move_collide::synth::{Convex, World};
use mw2er_core::move_sim::{Input, Player, SimCfg, SimEvent};
use mw2er_core::movement_data::MoveData;
use mw2er_core::sprint::{EndWhy, SprintEvent};
use mw2er_core::stance::StanceKind;
use mw2er_core::vec3::V3;
use serde_json::{Value, json};

const M: f32 = 1.0 / 0.0254;
const RATES: [f64; 3] = [30.0, 60.0, 144.0];

struct Run<'a> {
    d: &'a MoveData,
    cfg: SimCfg,
    p: Player,
    w: World,
    t: f64,
    hz: f64,
    ev: Vec<SimEvent>,
    max_rays: u32,
}

fn axes() -> (V3, V3) {
    (V3::new(0.0, 0.0, 1.0), V3::new(1.0, 0.0, 0.0))
}

fn keys(w: bool, s: bool, a: bool, d: bool) -> Input {
    let (f, r) = axes();
    Input { w, s, a, d, fwd_h: f, right_h: r, ..Input::default() }
}

impl<'a> Run<'a> {
    fn new(d: &'a MoveData, w: World, pos: V3, hz: f64) -> Run<'a> {
        let cfg = SimCfg::defaults(d);
        Run { d, cfg, p: Player::new(d, pos), w, t: 0.0, hz, ev: Vec::new(), max_rays: 0 }
    }
    fn tick(&mut self, i: Input) {
        self.t += 1.0 / self.hz;
        let st = self.p.tick(self.d, &self.cfg, &i, self.t, 1.0 / self.hz, &mut self.w, &mut self.ev);
        self.max_rays = self.max_rays.max(st.rays);
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

struct Report {
    rows: Vec<Value>,
    fails: u32,
}

impl Report {
    fn check(&mut self, id: &str, hz: f64, what: &str, got: f64, want: f64, tol: f64) {
        let ok = got.is_finite() && (got - want).abs() <= tol;
        self.push(id, hz, what, ok, format!("{got:.4}"), format!("{want} +- {tol}"));
    }
    fn check_true(&mut self, id: &str, hz: f64, what: &str, ok: bool, got: String) {
        self.push(id, hz, what, ok, got, "true".into());
    }
    fn push(&mut self, id: &str, hz: f64, what: &str, ok: bool, got: String, want: String) {
        println!("{} {id} {hz:>3} Hz {what}: {got} (want {want})", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.fails += 1;
        }
        self.rows.push(json!({"scenario": id, "hz": hz, "what": what, "pass": ok, "got": got, "want": want}));
    }
}

fn b1(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        let mut reached = None;
        while x.t < 5.0 - 1e-9 {
            x.tick(keys(true, false, false, false));
            if reached.is_none() && x.speed() >= 189.99 {
                reached = Some(x.t);
            }
        }
        r.check("B1", hz, "walk W steady m/s", (x.speed() / M) as f64, 4.826, 4.826 * 0.005);
        r.check_true("B1", hz, "190 in/s by 0.20 s", reached.is_some_and(|t| t <= 0.2 + 1e-9), format!("{reached:?}"));
    }
}

fn b2(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        x.run(2.0, keys(false, false, true, false));
        r.check("B2", hz, "strafe A m/s", (x.speed() / M) as f64, 3.861, 3.861 * 0.005);
        let mut y = Run::new(d, World::flat(), V3::ZERO, hz);
        y.run(2.0, keys(false, true, false, false));
        r.check("B2", hz, "back S m/s", (y.speed() / M) as f64, 3.378, 3.378 * 0.005);
    }
}

/// B3 + B4: sprint speed, duration, distance; then the recharge rules.
fn b3_b4(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        x.run(1.0, keys(true, false, false, false));
        x.take();
        let z0 = x.p.pos.z;
        let t0 = x.t;
        x.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, false, false) });
        let start_t = t0;
        let mut top_speed = 0.0f32;
        let mut end = None;
        while x.t < t0 + 6.0 && end.is_none() {
            x.tick(keys(true, false, false, false));
            top_speed = top_speed.max(x.speed());
            for e in x.take() {
                if let SimEvent::Sprint(SprintEvent::End { why, dur, .. }) = e {
                    end = Some((why, dur, x.p.pos.z - z0, x.t));
                }
            }
        }
        r.check("B3", hz, "sprint m/s", (top_speed / M) as f64, 7.239, 7.239 * 0.005);
        let (why, dur, dist, t_end) = end.unwrap_or((EndWhy::Handback, -1.0, -1.0, 0.0));
        r.check_true("B3", hz, "ends why=time", why == EndWhy::Time, why.label().into());
        r.check("B3", hz, "sprint duration s", dur, 4.0, 0.034);
        r.check("B3", hz, "sprint distance m", (dist / M) as f64, 28.86, 0.3);
        // B4: tap at 4.5 s after the start: refused; at 5.05 s: accepted (1:1 recharge, > 1 s minimum)
        let at = |x: &mut Run, t: f64| {
            while x.t < t - 1e-9 {
                x.tick(keys(true, false, false, false));
            }
        };
        x.take();
        at(&mut x, start_t + 4.5);
        x.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, false, false) });
        let refused = x.take().iter().any(|e| matches!(e, SimEvent::Sprint(SprintEvent::Refused { .. })));
        r.check_true("B4", hz, "tap at 4.5 s refused", refused, format!("refused={refused} end_t={t_end:.3}"));
        at(&mut x, start_t + 5.05);
        x.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, false, false) });
        let started = x.take().iter().any(|e| matches!(e, SimEvent::Sprint(SprintEvent::Start { .. })));
        r.check_true("B4", hz, "tap at 5.05 s accepted", started, format!("started={started}"));
    }
}

fn b5(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        for (st, presses, speed, eye, settle) in [(StanceKind::Crouch, (1, 0), 3.137f64, 1.067f64, 0.2f64), (StanceKind::Prone, (0, 1), 0.724, 0.293, 0.6)] {
            let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
            x.tick(Input { crouch_presses: presses.0, prone_presses: presses.1, ..keys(false, false, false, false) });
            let t0 = x.t - 1.0 / hz;
            while x.t < t0 + settle - 1e-9 {
                x.tick(keys(false, false, false, false));
            }
            let e = (x.p.eye_height(d, &x.cfg, x.t, true) / M) as f64;
            r.check("B5", hz, &format!("{} eye m after {settle} s", st.label()), e, eye, 0.002);
            x.run(3.0, keys(true, false, false, false));
            r.check("B5", hz, &format!("{} walk m/s", st.label()), (x.speed() / M) as f64, speed, speed * 0.005);
        }
    }
}

fn b6(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        x.run(0.6, keys(false, false, false, false));
        x.take();
        x.tick(Input { jump_presses: 1, ..keys(false, false, false, false) });
        let t_jump = x.t - 1.0 / hz;
        let (mut apex, mut air) = (None, None);
        while x.t < t_jump + 2.0 {
            x.tick(keys(false, false, false, false));
            for e in x.take() {
                match e {
                    SimEvent::Apex { h, .. } => apex = Some(h),
                    SimEvent::Land(_) if air.is_none() => air = Some(x.t - t_jump - x.p.land_late_s as f64),
                    _ => {}
                }
            }
        }
        r.check("B6", hz, "apex m", (apex.unwrap_or(-1.0) / M) as f64, 0.991, 0.003);
        r.check("B6", hz, "air time s", air.unwrap_or(-1.0), 0.6245, 1.0 / hz);
    }
}

fn b7(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        x.run(1.0, keys(true, false, false, false));
        x.take();
        let z0 = x.p.pos.z;
        x.tick(Input { jump_presses: 1, ..keys(true, false, false, false) });
        // the take-off is at the start of that tick's first substep: correct z0 for the motion before it
        let mut res = None;
        while x.t < 3.0 && res.is_none() {
            x.tick(keys(true, false, false, false));
            for e in x.take() {
                if let SimEvent::Land(l) = e {
                    let touch = x.p.pos.z - x.p.land_vel_pre.z * x.p.land_late_s;
                    res = Some((x.speed(), touch - z0, l));
                }
            }
        }
        let (land_speed, dist, _) = res.expect("landed");
        r.check("B7", hz, "landing speed m/s", (land_speed / M) as f64, 2.10, 0.01);
        r.check("B7", hz, "jump distance m", (dist / M) as f64, 3.01, 0.05);
    }
}

fn stairs(n: usize, rise_m: f32, tread_m: f32, z0: f32) -> World {
    let mut w = World::flat();
    for k in 0..n {
        let z = z0 + k as f32 * tread_m * M;
        w = w.with(Convex::aabb(V3::new(-200.0, -10.0, z), V3::new(200.0, (k + 1) as f32 * rise_m * M, z0 + 100.0 * M)));
    }
    w
}

fn b8(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let z0 = 30.0;
        let mut x = Run::new(d, stairs(10, 0.2, 0.3, z0), V3::ZERO, hz);
        let top = 10.0 * 0.2 * M;
        let mut t_top = None;
        let mut below = 0;
        while x.t < 4.0 {
            x.tick(keys(true, false, false, false));
            let k = ((x.p.pos.z - z0) / (0.3 * M)).floor();
            if (0.0..10.0).contains(&k) && x.p.grounded && x.p.pos.y < (k + 1.0) * 0.2 * M - 0.5 {
                below += 1;
            }
            if t_top.is_none() && (x.p.pos.y - top).abs() < 0.5 {
                t_top = Some(x.t);
            }
        }
        r.check_true("B8", hz, "10 x 0.2 m stairs climbed at walk < 2.5 s", t_top.is_some_and(|t| t < 2.5), format!("{t_top:?}"));
        r.check_true("B8", hz, "feet never below a tread", below == 0, format!("{below} ticks"));
    }
}

fn b9_world() -> (World, f32) {
    let wall_x = 3.0 * M;
    (World::flat().with(Convex::aabb(V3::new(wall_x, 0.0, -1000.0 * M), V3::new(wall_x + 20.0, 3.0 * M, 1000.0 * M))), wall_x)
}

fn b9_input() -> Input {
    // heading 30 deg into the wall (the wall is at +x, its tangent is +z)
    let a = 30f32.to_radians();
    let f = V3::new(a.sin(), 0.0, a.cos());
    let rr = V3::new(a.cos(), 0.0, -a.sin());
    Input { w: true, fwd_h: f, right_h: rr, ..Input::default() }
}

fn b9(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let (w, wall_x) = b9_world();
        let mut x = Run::new(d, w, V3::ZERO, hz);
        x.tick(Input { sprint_presses: 1, sprint_held: true, ..b9_input() });
        let mut min_gap = f32::MAX;
        let mut tang = Vec::new();
        while x.t < 3.5 {
            x.tick(b9_input());
            min_gap = min_gap.min(wall_x - x.p.pos.x - x.cfg.radius);
            if x.t > 1.5 {
                tang.push(x.p.vel.z);
            }
        }
        let t_avg = tang.iter().sum::<f32>() / tang.len().max(1) as f32;
        r.check_true("B9", hz, "no penetration (gap >= 0)", min_gap >= 0.0, format!("{:.3} m", min_gap / M));
        let need = 0.85 * 285.0 * 30f32.to_radians().cos();
        r.check_true("B9", hz, "tangential speed >= 0.85 sprint cos 30", t_avg >= need, format!("{t_avg:.1} in/s (need {need:.1})"));
    }
}

fn b10(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        for (hm, want, lethal) in [(6.0f32, 24.1f64, false), (9.0, 100.0, true)] {
            let w = World::flat().with(Convex::aabb(V3::new(-100.0, -100.0, -100.0), V3::new(100.0, hm * M, 20.0)));
            let mut x = Run::new(d, w, V3::new(0.0, hm * M, 0.0), hz);
            x.run(0.05, keys(false, false, false, false));
            let mut land = None;
            while x.t < 4.0 && land.is_none() {
                x.tick(keys(true, false, false, false));
                for e in x.take() {
                    if let SimEvent::Land(l) = e {
                        land = Some(l);
                    }
                }
            }
            let l = land.expect("landed");
            r.check("B10", hz, &format!("{hm} m drop damage %"), l.dmg_pct as f64, want, 0.2);
            r.check("B10", hz, &format!("{hm} m drop fall_h in"), l.fall_h as f64, (hm * M) as f64, 0.5);
            r.check_true("B10", hz, &format!("{hm} m lethal={lethal}"), l.lethal == lethal, format!("{}", l.lethal));
        }
    }
}

fn b11(d: &MoveData, r: &mut Report) {
    for hz in RATES {
        let mut x = Run::new(d, World::flat(), V3::ZERO, hz);
        x.run(1.0, keys(true, false, false, false));
        x.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, false, false) });
        x.run(1.0, keys(true, false, false, false));
        let t0 = x.t;
        let mut speeds = Vec::new();
        let mut stop = None;
        while x.t < t0 + 1.0 {
            x.tick(keys(false, false, false, false));
            speeds.push(x.speed());
            if stop.is_none() && x.speed() == 0.0 {
                stop = Some(x.t - t0);
            }
        }
        if hz == 60.0 {
            for (k, want) in [(0usize, 258.875f64), (1, 235.145), (2, 213.59), (10, 98.98)] {
                r.check("B11", hz, &format!("frame {} in/s", k + 1), speeds[k] as f64, want, 0.02);
            }
            r.check_true("B11", hz, "below 100 at frame 11", speeds[9] >= 100.0 && speeds[10] < 100.0, format!("{:.2}/{:.2}", speeds[9], speeds[10]));
            r.check("B11", hz, "stop s", stop.unwrap_or(-1.0), 0.367, 1e-3);
        } else {
            r.check("B11", hz, "stop s", stop.unwrap_or(-1.0), 0.367, 0.035);
        }
    }
}

fn b12(d: &MoveData, r: &mut Report) {
    let wall_z = 2.0 * M;
    let w = World::flat().with(Convex::aabb(V3::new(-500.0, 0.0, wall_z), V3::new(500.0, 3.0 * M, wall_z + 0.05 * M)));
    let mut x = Run::new(d, w, V3::ZERO, 15.0);
    x.cfg.speed_scale = 3.0;
    x.tick(Input { sprint_presses: 1, sprint_held: true, ..keys(true, false, false, false) });
    x.run(3.0, keys(true, false, false, false));
    r.check_true("B12", 15.0, "5 cm wall at 3x sprint not crossed", x.p.pos.z < wall_z, format!("z {:.3} m wall {:.3} m", x.p.pos.z / M, wall_z / M));
}

fn perf(d: &MoveData) -> bool {
    let (w, _) = b9_world();
    let mut x = Run::new(d, w, V3::ZERO, 30.0);
    x.tick(Input { sprint_presses: 1, sprint_held: true, ..b9_input() });
    let mut us = Vec::with_capacity(10_000);
    for k in 0..10_000 {
        // keep the run going along the wall: re-sprint whenever the meter allows it
        let i = Input { sprint_presses: (k % 300 == 0) as u32, sprint_held: true, ..b9_input() };
        let t = Instant::now();
        x.tick(i);
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    us.sort_by(|a, b| a.total_cmp(b));
    let p50 = us[us.len() / 2];
    let p95 = us[us.len() * 95 / 100];
    let max = us[us.len() - 1];
    let ok = p95 < 50.0 && x.max_rays <= x.cfg.max_rays;
    println!(
        "{} PERF movebench B9 at 30 Hz, 10000 ticks: p50 {p50:.2} us, p95 {p95:.2} us, max {max:.1} us (need p95 < 50); rays_max {} (<= {}); synthetic rays {}",
        if ok { "PASS" } else { "FAIL" },
        x.max_rays,
        x.cfg.max_rays,
        x.w.rays
    );
    ok
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args
        .iter()
        .position(|a| a == "--root")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));
    let path = root.join("converted").join("movement.json");
    let d = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| MoveData::from_json(&t)) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL movebench: {} ({e}); run tools\\scripts\\convert_movement.py", path.display());
            std::process::exit(1);
        }
    };
    if args.iter().any(|a| a == "--perf") {
        std::process::exit(if perf(&d) { 0 } else { 1 });
    }
    if args.iter().any(|a| a == "--mantle") {
        // M8: B13-B24 (docs/M8_DESIGN.md 12.2 + the review), written to work\m8\bench_m8.json
        let r = mantle_bench::run_all(&d);
        let mut scen: Vec<String> = Vec::new();
        for v in &r.rows {
            if let Some(s) = v["scenario"].as_str()
                && !scen.iter().any(|x| x == s)
            {
                scen.push(s.to_string());
            }
        }
        let passed = scen.iter().filter(|s| !r.rows.iter().any(|v| v["scenario"] == s.as_str() && v["pass"] == false)).count();
        let out = root.join("work").join("m8").join("bench_m8.json");
        let _ = std::fs::create_dir_all(out.parent().unwrap_or(&root));
        let doc = json!({"data": path.display().to_string(), "rates": mantle_bench::RATES, "rows": r.rows, "fails": r.fails});
        if let Err(e) = std::fs::write(&out, serde_json::to_string_pretty(&doc).unwrap_or_default()) {
            println!("FAIL writing {}: {e}", out.display());
            std::process::exit(1);
        }
        println!(
            "{} MOVEBENCH M8 {passed}/{} scenarios ({}) at 30/60/144 Hz, {} checks, {} failed; wrote {}",
            if r.fails == 0 && scen.len() == mantle_bench::SCENARIOS { "PASS" } else { "FAIL" },
            scen.len(),
            scen.join(","),
            r.rows.len(),
            r.fails,
            out.display()
        );
        std::process::exit(if r.fails == 0 && scen.len() == mantle_bench::SCENARIOS { 0 } else { 1 });
    }
    if args.iter().any(|a| a == "--c3") {
        // C3: C21-C24 (docs/C3_DESIGN.md 1.5), written to work\c3\bench_c3.json
        let r = c3_bench::run_all(&d);
        let mut scen: Vec<String> = Vec::new();
        for v in &r.rows {
            if let Some(s) = v["scenario"].as_str()
                && !scen.iter().any(|x| x == s)
            {
                scen.push(s.to_string());
            }
        }
        let passed = scen.iter().filter(|s| !r.rows.iter().any(|v| v["scenario"] == s.as_str() && v["pass"] == false)).count();
        let out = root.join("work").join("c3").join("bench_c3.json");
        let _ = std::fs::create_dir_all(out.parent().unwrap_or(&root));
        let doc = json!({"data": path.display().to_string(), "rates": mantle_bench::RATES, "rows": r.rows, "fails": r.fails});
        if let Err(e) = std::fs::write(&out, serde_json::to_string_pretty(&doc).unwrap_or_default()) {
            println!("FAIL writing {}: {e}", out.display());
            std::process::exit(1);
        }
        let ok = r.fails == 0 && scen.len() == c3_bench::SCENARIOS;
        println!(
            "{} MOVEBENCH C3 {passed}/{} scenarios ({}) at 30/60/144 Hz, {} checks, {} failed; wrote {}",
            if ok { "PASS" } else { "FAIL" },
            scen.len(),
            scen.join(","),
            r.rows.len(),
            r.fails,
            out.display()
        );
        std::process::exit(if ok { 0 } else { 1 });
    }
    if !args.iter().any(|a| a == "--check") {
        println!("usage: movebench --check | --mantle | --c3 | --perf [--root <dir>]");
        std::process::exit(2);
    }
    let mut r = Report { rows: Vec::new(), fails: 0 };
    b1(&d, &mut r);
    b2(&d, &mut r);
    b3_b4(&d, &mut r);
    b5(&d, &mut r);
    b6(&d, &mut r);
    b7(&d, &mut r);
    b8(&d, &mut r);
    b9(&d, &mut r);
    b10(&d, &mut r);
    b11(&d, &mut r);
    b12(&d, &mut r);
    let mut scen: Vec<String> = Vec::new();
    for v in &r.rows {
        if let Some(s) = v["scenario"].as_str()
            && !scen.iter().any(|x| x == s)
        {
            scen.push(s.to_string());
        }
    }
    let passed: Vec<&String> = scen.iter().filter(|s| !r.rows.iter().any(|v| v["scenario"] == s.as_str() && v["pass"] == false)).collect();
    let out = root.join("work").join("m5").join("bench.json");
    let _ = std::fs::create_dir_all(out.parent().unwrap_or(&root));
    let doc = json!({"data": path.display().to_string(), "exe_sha256": d.exe_sha256, "rates": RATES, "rows": r.rows, "fails": r.fails});
    if let Err(e) = std::fs::write(&out, serde_json::to_string_pretty(&doc).unwrap_or_default()) {
        println!("FAIL writing {}: {e}", out.display());
        std::process::exit(1);
    }
    println!(
        "{} MOVEBENCH {}/{} scenarios ({}) at 30/60/144 Hz, {} checks, {} failed; wrote {}",
        if r.fails == 0 { "PASS" } else { "FAIL" },
        passed.len(),
        scen.len(),
        scen.join(","),
        r.rows.len(),
        r.fails,
        out.display()
    );
    std::process::exit(if r.fails == 0 { 0 } else { 1 });
}
