//! C6 part 18 (docs/C6_HOLD_DESIGN.md part 18): the task side of the third-person over-the-shoulder camera of MW2 mode
//! (`[third_person]`, hot; off while both offsets are zero). Per tick: the gates (`mw2er_core::shoulder::gate`), the
//! blend (`ShoulderCtl`), the spring arm (a map ray + an object ray from ER's own camera position to the offset
//! position, pulled in by `shoulder_margin_m` before a hit) and the publish to the camera-step detour
//! (`cam_hook::shoulder_publish` / `shoulder_stop`), which moves `CSCamera.pers_cam_1` row 3 only. Nothing else is
//! written. The task's camera read already sees the offset camera (the detour runs before the task, M5 1.1), so the
//! shot ray, the HUD projection, the damage numbers, the world-gun occlusion and ER-depth all use the camera on screen.
//!
//! Lines: `CAM shoulder cfg ..`, `CAM shoulder on ..`, `CAM shoulder off why= ..`, `CAM shoulder pull ..`,
//! `CAM shoulder pull end ..`, `CAM shoulder unavailable reason=no_cam_hook`.

use std::sync::atomic::Ordering;

use mw2er_core::config::Config;
use mw2er_core::shoulder::{GateIn, PULL_FRAC, ShoulderCtl, Why, arm_frac, gate, world_offset};
use mw2er_core::vec3::V3;

use crate::cam_hook;
use crate::game::camera::{CameraSample, read_pers_cam1_17};
use crate::game::havok::{OBJECT_FILTER, map_ray, ray_t};
use crate::game::player::PlayerSample;
use crate::log::log_i;

/// At most one `CAM shoulder pull` start line per this many seconds (the rest are counted).
const PULL_LOG_EVERY_S: f64 = 1.0;

pub struct ShoulderIn<'a> {
    pub now: f64,
    pub dt: f32,
    pub cfg: &'a Config,
    /// The gates without `cfg_on` / `hook` (filled here).
    pub gate: GateIn,
    /// 0 hip .. 1 ADS (the MW2 ADS fraction; 1 in the C6 guard / crossbow aim).
    pub ads_w: f32,
    pub cam: Option<CameraSample>,
    pub player: Option<&'a PlayerSample>,
}

#[derive(Default)]
struct Episode {
    t0: f64,
    writes0: u64,
    skips0: u64,
    persisted0: u64,
    conflicts0: u64,
    pulls: u32,
    min_frac: f32,
}

#[derive(Default)]
pub struct ShoulderRt {
    ctl: ShoulderCtl,
    cfg_line: String,
    unavailable_logged: bool,
    ep: Option<Episode>,
    /// The current pull: (start time, min fraction).
    pull: Option<(f64, f32)>,
    last_pull_log: f64,
    /// A panic turned it off for the session.
    disabled: bool,
    /// Consecutive ticks with the camera gate invalid (a re-base blip within `[first_person] cam_gate_grace_ticks`
    /// does not stop the offset).
    cam_bad_ticks: u32,
}

fn counters() -> (u64, u64, u64, u64) {
    (
        cam_hook::SH_WRITES.load(Ordering::Relaxed),
        cam_hook::SH_SKIPS.load(Ordering::Relaxed),
        cam_hook::SH_PERSISTED.load(Ordering::Relaxed),
        cam_hook::SH_CONFLICTS.load(Ordering::Relaxed),
    )
}

fn v3s(v: V3) -> String {
    format!("[{:.2},{:.2},{:.2}]", v.x, v.y, v.z)
}

impl ShoulderRt {
    pub fn tick(&mut self, i: &ShoulderIn) {
        if self.disabled {
            return;
        }
        let tp = &i.cfg.third_person;
        let sp = tp.shoulder();
        let hook = cam_hook::INSTALLED.load(Ordering::Relaxed);
        let line = format!(
            "cfg on={} off={} ads={} side={} blend={:.2} margin={:.2} mounted={} hook={}",
            sp.on() as u8,
            v3s(V3::new(sp.offset[0], sp.offset[1], sp.offset[2])),
            v3s(V3::new(sp.ads_offset[0], sp.ads_offset[1], sp.ads_offset[2])),
            sp.side.label(),
            sp.blend_s,
            sp.margin_m,
            tp.shoulder_mounted as u8,
            hook as u8
        );
        if line != self.cfg_line {
            log_i!("CAM", "shoulder {line}");
            self.cfg_line = line;
        }
        if sp.on() && !hook && !self.unavailable_logged {
            self.unavailable_logged = true;
            log_i!("CAM", "shoulder unavailable reason=no_cam_hook (needs [first_person] enabled + camera_mode = \"hook\", or [ac130] enabled, at boot)");
        }
        self.cam_bad_ticks = if i.gate.cam_valid { 0 } else { self.cam_bad_ticks.saturating_add(1) };
        // the grace only keeps a running offset (never starts one on an unsettled camera)
        let cam_valid = i.gate.cam_valid || (self.ctl.applying && self.cam_bad_ticks <= i.cfg.first_person.cam_gate_grace_ticks);
        let g = gate(&GateIn { cfg_on: sp.on(), hook, mounted_ok: tp.shoulder_mounted, cam_valid, ..i.gate });
        let st = self.ctl.step(g, sp.target(i.ads_w), i.dt, sp.blend_s);
        if !st.apply {
            self.stop(i.now, g.err().unwrap_or(Why::Cfg));
            return;
        }
        // the spring arm: from ER's own camera (this frame's position before our write) toward the offset position
        let (Some(c), Some(p)) = (i.cam, i.player) else {
            self.stop(i.now, Why::Camera);
            return;
        };
        let (er_pos, er_fwd) = cam_hook::shoulder_er_view(c.pos).unwrap_or((c.pos, c.row2));
        let Some(want) = world_offset(er_fwd, st.local) else {
            self.stop(i.now, Why::Camera);
            return;
        };
        let len = want.len();
        let hit = match want.normalize() {
            Some(dir) if len > 1e-3 => {
                let range = len + sp.margin_m;
                let a = map_ray(er_pos, p.chr.pos_w, dir, range, p.player_ins);
                let b = ray_t(er_pos, p.chr.pos_w, dir, range, p.player_ins, OBJECT_FILTER);
                match (a, b) {
                    (Some(x), Some(y)) => Some((x.min(y), if x <= y { "map" } else { "obj" })),
                    (Some(x), None) => Some((x, "map")),
                    (None, Some(y)) => Some((y, "obj")),
                    (None, None) => None,
                }
            }
            _ => None,
        };
        let allowed = arm_frac(len, hit.map(|h| h.0), sp.margin_m);
        let local = self.ctl.arm(allowed, i.dt, sp.blend_s);
        let Some((addr, _)) = read_pers_cam1_17() else {
            self.stop(i.now, Why::Camera);
            return;
        };
        let was_on = self.ep.is_some();
        cam_hook::shoulder_publish(local, addr, crate::game::fp_read::cscamera_addr());
        if !was_on {
            let (w, s, pe, cf) = counters();
            self.ep = Some(Episode { t0: i.now, writes0: w, skips0: s, persisted0: pe, conflicts0: cf, pulls: 0, min_frac: 1.0 });
            log_i!(
                "CAM",
                "shoulder on side={} ads={:.2} local={} er_cam={} fwd={}",
                sp.side.label(),
                i.ads_w,
                v3s(st.local),
                v3s(er_pos),
                v3s(er_fwd)
            );
        }
        // pulls (the spring arm under PULL_FRAC)
        let frac = self.ctl.frac;
        if let Some(ep) = self.ep.as_mut() {
            ep.min_frac = ep.min_frac.min(frac);
        }
        match (self.pull, frac < PULL_FRAC) {
            (None, true) => {
                self.pull = Some((i.now, frac));
                if let Some(ep) = self.ep.as_mut() {
                    ep.pulls += 1;
                }
                if i.now - self.last_pull_log >= PULL_LOG_EVERY_S {
                    self.last_pull_log = i.now;
                    let (t, by) = hit.map_or((f32::NAN, "none"), |h| h);
                    log_i!("CAM", "shoulder pull frac={frac:.2} want={len:.2}m hit={t:.2}m by={by} margin={:.2}", sp.margin_m);
                }
            }
            (Some((t0, m)), true) => self.pull = Some((t0, m.min(frac))),
            (Some((t0, m)), false) => {
                self.pull = None;
                if i.now - self.last_pull_log >= PULL_LOG_EVERY_S {
                    self.last_pull_log = i.now;
                    log_i!("CAM", "shoulder pull end ms={:.0} min_frac={m:.2}", (i.now - t0) * 1000.0);
                }
            }
            (None, false) => {}
        }
    }

    /// Disarms the detour (if armed) and logs the end of an episode.
    fn stop(&mut self, now: f64, why: Why) {
        cam_hook::shoulder_stop();
        self.pull = None;
        if let Some(ep) = self.ep.take() {
            let (w, s, pe, cf) = counters();
            log_i!(
                "CAM",
                "shoulder off why={} ms={:.0} writes={} skips={} persisted={} conflicts={} pulls={} min_frac={:.2}",
                why.label(),
                (now - ep.t0) * 1000.0,
                w.saturating_sub(ep.writes0),
                s.saturating_sub(ep.skips0),
                pe.saturating_sub(ep.persisted0),
                cf.saturating_sub(ep.conflicts0),
                ep.pulls,
                ep.min_frac
            );
        }
    }

    /// A panic (here or in the task): disarm at once, off for the session.
    pub fn emergency_off(&mut self) {
        cam_hook::shoulder_stop();
        if !self.disabled {
            crate::log::log_e!("CAM", "shoulder off why=panic; the shoulder camera is off for this session");
        }
        self.disabled = true;
        self.ep = None;
        self.pull = None;
        self.ctl = ShoulderCtl::default();
    }
}
