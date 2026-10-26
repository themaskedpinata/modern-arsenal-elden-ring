//! The MW2 player controller (docs/M5_DESIGN.md sections 2-5), headless and pure: the DLL runs it once per task
//! tick against Havok rays, the bench (`movebench`) and the tests against [`crate::move_collide::synth`].
//!
//! IW4 units (inches, seconds, y up). One tick = `ceil(dt / max_substep_s)` substeps, each:
//! timers -> sprint -> stance keys -> jump -> friction + accelerate (ground) / air accelerate -> horizontal
//! collide-and-slide with step-up -> gravity, ceiling, ground probe and snap -> landing.

use crate::fall::{self, Landing};
use crate::mantle::{self, AbortWhy, KeyMode, Ledge, Mantle, MantleCfg, MantleKind, Phase, Query, Refuse, RefuseCache, Trigger};
use crate::move_collide::{
    Capsule, DETECT_EXTRA, MARGIN, RayBudget, RayKind, RayWorld, body_column, ceiling, down_ray, ground_normal, headroom,
    plane_allow, sweep, wall_normal,
};
use crate::movement_data::MoveData;
use crate::pmove::{self, Cmd, ScaleIn};
use crate::sprint::{EndWhy, Sprint, SprintEvent, SprintIn, SprintParams};
use crate::stance::{Stance, StanceEvent, StanceKeys, StanceKind};
use crate::vec3::{UP, V3};

/// Controller settings that do not come from MW2 (config + ER's capsule), inches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimCfg {
    pub radius: f32,
    pub skin: f32,
    pub max_substep_s: f32,
    pub max_bumps: u32,
    pub max_rays: u32,
    /// Vertical scale of the stance sizes (eye, hull): `ratio` = eye_height_m / (60 in), `mw2` = 1.
    pub v_scale: f32,
    pub walkable_y: f32,
    pub jump_slowdown: bool,
    pub sprint: SprintParams,
    pub speed_scale: f32,
    pub class_scale: f32,
    pub lightweight: bool,
    pub crouch_hold: bool,
    pub gostand_from_prone: StanceKind,
    /// Free fall with no ground hit for this far (inches) -> `Freefall` event (handback).
    pub freefall_in: f32,
    /// M8: mantling (docs/M8_DESIGN.md); `enabled` false without the data's mantle block.
    pub mantle: MantleCfg,
    /// Run 18: the waist probe height and the front body column of the sweep (`[movement.rays] body_probes`).
    pub body_probes: bool,
}

impl SimCfg {
    /// The design defaults with the given data (tests and bench): r 0.4 m, skin 0.03 m, 1/60 s, 3 bumps, 32 rays.
    pub fn defaults(d: &MoveData) -> SimCfg {
        let m = &d.mw2;
        SimCfg {
            radius: 0.4 / 0.0254,
            skin: 0.03 / 0.0254,
            max_substep_s: 0.0167,
            max_bumps: 3,
            max_rays: 48,
            v_scale: 1.60 / (m.eye_stand * 0.0254),
            walkable_y: m.walkable_normal_y,
            jump_slowdown: true,
            sprint: SprintParams {
                time_s: m.sprint_time_s as f64,
                time_max_s: m.sprint_time_max_s as f64,
                min_time_s: m.sprint_min_time_s as f64,
                recharge_pause_s: m.sprint_recharge_pause_s as f64,
                forward_min: m.sprint_forward_min,
                duration_scale: d.weapon.sprint_duration_scale as f64,
                out_s: d.weapon.sprint_out_s as f64,
                marathon: false,
                mode: crate::sprint::SprintMode::Press,
            },
            speed_scale: 1.0,
            class_scale: 1.0,
            lightweight: false,
            crouch_hold: false,
            gostand_from_prone: StanceKind::Stand,
            freefall_in: 30.0 / 0.0254,
            mantle: MantleCfg { end_speed_max: m.g_speed, ..MantleCfg::defaults(d.mantle.as_ref()) },
            body_probes: true,
        }
    }
}

/// Up to this many nearby characters (feet, inches) make a ledge top `occupied` (section 3 rule 6).
pub const MAX_OTHERS: usize = 8;

/// One tick's input: key levels, press counts since the last tick, the camera's horizontal axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub w: bool,
    pub s: bool,
    pub a: bool,
    pub d: bool,
    pub fwd_h: V3,
    pub right_h: V3,
    pub sprint_presses: u32,
    pub sprint_held: bool,
    pub jump_presses: u32,
    pub crouch_presses: u32,
    pub crouch_held: bool,
    pub prone_presses: u32,
    pub hold_tap: bool,
    pub hold_long: bool,
    /// Fire / ADS held (the M2/M3 latches); reload pressed this tick.
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    /// ER's own ground signal (`is_touching_ground`, read before this tick); `None` = no engine (bench / tests).
    /// While grounded, a ground-ray miss starts a fall only if ER does not report ground (review M5 major 1).
    pub er_ground: Option<bool>,
    /// Push-out to apply this tick (inches, planar): ER's proxy push-out + nearby characters. It is added to the
    /// first substep's move and goes through the same collide-and-slide (and ray budget) as the walk.
    pub push: V3,
    /// M8: the jump key is held now (RAW_KEYS level; a press edge counts as held).
    pub jump_held: bool,
    /// M8: no mantle check this tick (a hold, a re-arm, a handback in progress: the DLL's gate).
    pub mantle_block: bool,
    /// M8: ER's planar displacement of the player this tick (inches), measured; during a mantle it is never applied
    /// and above `mantle.er_push_abort_in` it aborts the mantle.
    pub er_delta_in: f32,
    /// M8: nearby characters' feet (inches) for the `occupied` rule; the first `n_others` are valid.
    pub others: [V3; MAX_OTHERS],
    pub n_others: u8,
    /// C3 PASS B (docs/C3_DESIGN.md 4.5): the left stick's analog usercmd (forwardmove, rightmove, -127..127) outside
    /// its deadzone; W/A/S/D win when both are used.
    pub analog: Option<(f32, f32)>,
}

impl Input {
    /// A movement direction is wished (keys or the stick).
    pub fn wish(&self) -> bool {
        self.w || self.a || self.s || self.d || self.analog.is_some()
    }

    /// The usercmd of this tick: the keys at full scale, else the stick's analog values.
    pub fn cmd(&self) -> Cmd {
        match self.analog {
            Some((fm, rm)) if !(self.w || self.a || self.s || self.d) => {
                let c = |v: f32| if v.is_finite() { v.clamp(-crate::pmove::CMD_MAX, crate::pmove::CMD_MAX) } else { 0.0 };
                Cmd { forwardmove: c(fm), rightmove: c(rm) }
            }
            _ => Cmd::from_keys(self.w, self.s, self.a, self.d),
        }
    }
}

/// The start of a mantle (for the `MOVE mantle start` line).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MantleStart {
    pub kind: MantleKind,
    pub rise: f32,
    pub row: usize,
    pub row_height: f32,
    pub ledge_y: f32,
    pub from: V3,
    pub end: V3,
    pub dir: V3,
    pub d: f32,
    pub x: f32,
    pub dur: f64,
    pub rays: u32,
    pub trigger: Trigger,
    pub air: bool,
    pub v0y: f32,
    pub retried: bool,
    pub drop_weapon: bool,
    pub drop_y: Option<f32>,
    /// Run 18: the start was backed off a face the capsule reached into (inches; 0 = none).
    pub depen: f32,
    /// Run 18: the top's sides were probed at the narrow offset.
    pub narrow_top: bool,
    /// 2026-10-21: the over-the-top line raised over a top rising away (inches; 0 = none).
    pub line_up: f32,
}

/// The first ticks after leaving the ground in which ER's ground flag is ignored (ER reads our previous write).
pub const ER_AIR_GRACE_TICKS: u32 = 3;
/// A fall during which ER reported ground on at least this many ticks (after the grace) deals no fall damage: our
/// rays and ER disagree about the fall, so it is not trusted for an HP write (review M5 major 1).
pub const ER_GROUND_SKIP_TICKS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimEvent {
    Sprint(SprintEvent),
    Stance(StanceEvent),
    Jump { y0: f32, v0: f32, land_factor: f32 },
    /// Highest point of a jump: height above the take-off (inches), time since take-off.
    Apex { h: f32, t: f64, ceiling: bool },
    Land(Landing),
    /// Fell this far (inches) without a ground hit.
    Freefall { fallen: f32 },
    /// M8: a mantle started (replaces the jump when it came from a grounded press).
    MantleStart(MantleStart),
    /// M8: the mantle reached its end (grounded on the top; Over: the release into the drop follows).
    MantleEnd { dt: f64, kind: MantleKind, pos: V3, dist_end: f32, vel: V3 },
    /// M8: Over: released into normal gravity at the far side (the far floor height, inches).
    MantleRelease { drop_to: Option<f32>, pos: V3 },
    /// M8: the mantle stopped early; the feet are back at the last validated point.
    MantleAbort { why: AbortWhy, t_frac: f32, pos: V3 },
    /// M8: a check refused (`rise` = the top it saw above the feet, inches; 0 = unknown).
    MantleRefuse { why: Refuse, rise: f32, rays: u32, fwd: V3 },
}

/// Per-tick cost and state for the log / PERF line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TickStats {
    pub substeps: u32,
    pub rays: u32,
    pub budget_hit: bool,
    pub bumps: u32,
    pub steps: u32,
    /// Substeps in which the ground ray missed but ER reported ground (feet kept).
    pub ground_held: u32,
    /// M8: rays of the separate mantle budget this tick (detection + re-validation), and detections run (0 or 1).
    pub mantle_rays: u32,
    pub mantle_checks: u32,
    pub mantling: bool,
    /// M8 review: this tick's mantle budget (`max_rays + SUBSTEP_RAYS * (substeps - 1)`) and accepted detections
    /// not started because the rays left could not re-validate the rest of the tick (tried again next tick).
    pub mantle_budget: u32,
    pub mantle_deferred: u32,
    /// Run 18: penetration evidence this tick: sweep hits deeper than the skin inside the capsule, and mantle starts
    /// backed off a face the capsule reached into (the unstick detector's `inside` signal).
    pub inside: u32,
    /// Run 18: the body column (front, between knee and chest) stopped a move this tick.
    pub column_hits: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    /// Feet (inches, the controller's frame = ER's Havok frame / 0.0254).
    pub pos: V3,
    pub vel: V3,
    pub grounded: bool,
    pub normal: V3,
    pub stance: Stance,
    pub sprint: Sprint,
    pub jumping: bool,
    pub jump_origin_y: f32,
    pub jump_t: f64,
    pub last_jump_t: f64,
    /// Jump slowdown (pm_time of JUMPING) and TIME_LAND left, ms.
    pub slow_ms: f32,
    pub land_ms: f32,
    /// Highest feet y since leaving the ground (free fall / fall height).
    pub air_top_y: f32,
    freefall_sent: bool,
    apex_sent: bool,
    ceiling_hit: bool,
    pub dip_in: f32,
    pub dip_t: f64,
    pub bob_phase: f32,
    /// Distance walked (inches) for the bob cycle.
    pub walked: f32,
    /// Last horizontal move direction (for ER push-out filtering).
    pub last_dir: V3,
    /// The ER cross-check streak etc. live in the DLL; here only the controller.
    pub last_wish: f32,
    /// Remembered wall planes (point, normal, probe height), re-checked with one ray while pushed into.
    contacts: [(V3, V3, f32); 4],
    ncontacts: usize,
    /// The ground normal was measured since the last resync / landing.
    normal_fresh: bool,
    /// The last landing: how long before the end of its substep the feet touched down, and the horizontal velocity
    /// before the landing slowdown (the headless bench measures exact jump distances with them).
    pub land_late_s: f32,
    pub land_vel_pre: V3,
    /// Ticks since leaving the ground, and how many of them (after [`ER_AIR_GRACE_TICKS`]) ER reported ground.
    pub air_ticks: u32,
    pub er_ground_air: u32,
    /// M8: the running mantle (`None` = normal movement).
    pub mantle: Option<Mantle>,
    /// M8: the feet height at the last grounded substep (the 2.0 m cap is measured from it).
    pub ground_y0: f32,
    /// M8: no mantle check before this time (a re-arm: `enter()` sets now + 0.3 s).
    pub mantle_block_until: f64,
    pub refuse_cache: RefuseCache,
    /// M8: crouch / prone presses during a mantle, applied at its end.
    mantle_pending: (u32, u32),
}

impl Player {
    pub fn new(d: &MoveData, pos: V3) -> Player {
        Player {
            pos,
            vel: V3::ZERO,
            grounded: true,
            normal: UP,
            stance: Stance::new(&d.mw2),
            sprint: Sprint::default(),
            jumping: false,
            jump_origin_y: pos.y,
            jump_t: f64::NEG_INFINITY,
            last_jump_t: f64::NEG_INFINITY,
            slow_ms: 0.0,
            land_ms: 0.0,
            air_top_y: pos.y,
            freefall_sent: false,
            apex_sent: true,
            ceiling_hit: false,
            dip_in: 0.0,
            dip_t: f64::NEG_INFINITY,
            bob_phase: 0.0,
            walked: 0.0,
            last_dir: V3::ZERO,
            last_wish: 0.0,
            contacts: [(V3::ZERO, UP, 0.0); 4],
            ncontacts: 0,
            normal_fresh: false,
            land_late_s: 0.0,
            land_vel_pre: V3::ZERO,
            air_ticks: 0,
            er_ground_air: 0,
            mantle: None,
            ground_y0: pos.y,
            mantle_block_until: f64::NEG_INFINITY,
            refuse_cache: RefuseCache::default(),
            mantle_pending: (0, 0),
        }
    }

    /// M8: a mantle is running.
    pub fn mantling(&self) -> bool {
        self.mantle.is_some()
    }

    /// M8: stops a running mantle (handback, warp, ER push, read failure): the feet stay at the last validated point,
    /// airborne with zero velocity, so gravity (ER's after a handback) settles them at most ~2 m lower without damage.
    pub fn mantle_abort(&mut self, why: AbortWhy) -> Option<SimEvent> {
        let m = self.mantle.take()?;
        self.pos = m.last_valid;
        self.grounded = false;
        self.vel = V3::ZERO;
        self.jumping = false;
        self.air_top_y = self.pos.y;
        self.freefall_sent = false;
        self.apex_sent = true;
        self.normal = UP;
        self.ncontacts = 0;
        self.normal_fresh = false;
        self.mantle_pending = (0, 0);
        self.leave_ground();
        Some(SimEvent::MantleAbort { why, t_frac: m.frac(), pos: self.pos })
    }

    /// The feet left the ground (jump, ledge, steep slope, resync): a new fall starts.
    fn leave_ground(&mut self) {
        self.air_ticks = 0;
        self.er_ground_air = 0;
    }

    /// Whether ER agreed that the player was airborne for the fall that just ended (fall damage may be written).
    pub fn fall_er_confirmed(&self) -> bool {
        self.er_ground_air < ER_GROUND_SKIP_TICKS
    }

    /// Resync to ER's position (handback / warp / world entry): velocity and jump state cleared, `vel0` seeded.
    pub fn resync(&mut self, pos: V3, vel0: V3, grounded: bool) {
        self.pos = pos;
        self.vel = vel0;
        self.grounded = grounded;
        self.jumping = false;
        self.air_top_y = pos.y;
        self.freefall_sent = false;
        self.apex_sent = true;
        self.ceiling_hit = false;
        self.normal = UP;
        self.ncontacts = 0;
        self.normal_fresh = false;
        // a stale path never survives a resync (the DLL logs the abort first)
        self.mantle = None;
        self.mantle_pending = (0, 0);
        self.ground_y0 = pos.y;
        self.refuse_cache.clear();
        self.leave_ground();
    }

    /// A rebase: ER moved its Havok frame; translate, keep everything else (M8: the mantle path moves with it).
    pub fn translate(&mut self, by: V3) {
        self.pos = self.pos + by;
        for c in self.contacts[..self.ncontacts].iter_mut() {
            c.0 = c.0 + by;
        }
        self.jump_origin_y += by.y;
        self.air_top_y += by.y;
        self.ground_y0 += by.y;
        self.refuse_cache.translate(by);
        if let Some(m) = self.mantle.as_mut() {
            m.translate(by);
        }
    }

    pub fn capsule(&self, d: &MoveData, cfg: &SimCfg) -> Capsule {
        let m = &d.mw2;
        let eff = self.stance.effective();
        Capsule {
            radius: cfg.radius,
            skin: cfg.skin,
            hull: self.stance.hull_in(m) * cfg.v_scale,
            step: if eff == StanceKind::Prone { m.step_height_prone } else { m.step_height },
            walkable_y: cfg.walkable_y,
        }
    }

    /// Eye height above the feet (inches, scaled): the stance curve, minus the landing dip (150 ms ease-out).
    pub fn eye_height(&self, d: &MoveData, cfg: &SimCfg, now: f64, land_dip: bool) -> f32 {
        let mut e = self.stance.eye_in(d, now) * cfg.v_scale;
        if land_dip {
            let t = ((now - self.dip_t) / 0.15) as f32;
            if (0.0..1.0).contains(&t) {
                let k = 1.0 - (1.0 - (1.0 - t).powi(2));
                e -= self.dip_in * cfg.v_scale * k;
            }
        }
        e
    }

    /// Vertical view bob (inches) from the MW2 per-stance amplitudes [I formula]: amp_y x speed x 0.25 x sin.
    pub fn view_bob(&self, d: &MoveData, ads: bool) -> f32 {
        if !self.grounded {
            return 0.0;
        }
        let m = &d.mw2;
        let speed = self.vel.planar_len();
        if speed < m.move_threshold {
            return 0.0;
        }
        let idx = if self.sprint.active() {
            3
        } else {
            match self.stance.effective() {
                StanceKind::Stand => 0,
                StanceKind::Crouch => 1,
                StanceKind::Prone => 2,
            }
        };
        let mut amp = m.view_bob[idx][1] * speed * 0.25;
        if idx == 3 {
            amp *= m.sprint_camera_bob;
        }
        if ads {
            amp *= d.weapon.ads_view_bob_mult;
        }
        amp * self.bob_phase.sin()
    }

    /// Ends a running sprint because ER takes over (handback).
    pub fn end_sprint_handback(&mut self, cfg: &SimCfg, now: f64) -> Option<SprintEvent> {
        self.sprint.force_end(&cfg.sprint, now, EndWhy::Handback)
    }

    /// One task tick of `dt` seconds ending at `now`.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        d: &MoveData,
        cfg: &SimCfg,
        inp: &Input,
        now: f64,
        dt: f64,
        world: &mut dyn RayWorld,
        out: &mut Vec<SimEvent>,
    ) -> TickStats {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.1) } else { 0.0 };
        let mut st = TickStats::default();
        if dt <= 0.0 {
            return st;
        }
        // a mantle tick is not an airborne tick (no free-fall / ER-ground statistics)
        if !self.grounded && self.mantle.is_none() {
            self.air_ticks = self.air_ticks.saturating_add(1);
            if self.air_ticks > ER_AIR_GRACE_TICKS && inp.er_ground == Some(true) {
                self.er_ground_air = self.er_ground_air.saturating_add(1);
            }
        }
        let n =((dt / cfg.max_substep_s.max(1e-3) as f64).ceil() as u32).clamp(1, 16);
        let h = dt / n as f64;
        let mut b = RayBudget::new(cfg.max_rays);
        // M8: the mantle's own budget (detection + re-validation), never the main budget or its vertical reserve.
        // M8 review: sized by the substeps (a 0.1 s hitch runs 6), so a frame hitch never aborts a mantle `budget`
        let mut mb = RayBudget::new(cfg.mantle.max_rays + mantle::SUBSTEP_RAYS * (n - 1));
        let mut detected = false;
        let mut deferred = 0u32;
        let mut depen = 0u32;
        for i in 0..n {
            // vertical rays (ground + ceiling) of this and the remaining substeps stay reserved
            b.reserve = 2 * (n - i);
            let t = now - dt + h * (i + 1) as f64;
            let mut ms = MantleStep { mb: &mut mb, detected: &mut detected, left: n - 1 - i, deferred: &mut deferred, depen: &mut depen };
            self.substep(d, cfg, inp, i == 0, t, h as f32, &mut b, &mut ms, world, out, &mut st);
        }
        st.substeps = n;
        st.rays = b.used;
        st.budget_hit = b.exhausted;
        st.mantle_rays = mb.used;
        st.mantle_budget = mb.max;
        st.mantle_deferred = deferred;
        st.inside += depen;
        st.mantling = self.mantle.is_some();
        st
    }

    #[allow(clippy::too_many_arguments)]
    fn substep(
        &mut self,
        d: &MoveData,
        cfg: &SimCfg,
        inp: &Input,
        first: bool,
        t: f64,
        h: f32,
        b: &mut RayBudget,
        ms: &mut MantleStep,
        w: &mut dyn RayWorld,
        out: &mut Vec<SimEvent>,
        st: &mut TickStats,
    ) {
        let m = &d.mw2;
        self.slow_ms = (self.slow_ms - h * 1000.0).max(0.0);
        self.land_ms = (self.land_ms - h * 1000.0).max(0.0);
        // M8: a running mantle owns the substep (no walk, no jump, no gravity)
        if self.mantle.is_some() {
            self.mantle_substep(d, cfg, inp, first, t, h, b, ms.mb, w, out);
            return;
        }
        if self.grounded {
            self.ground_y0 = self.pos.y;
        }
        let cmd = inp.cmd();
        let press = |n: u32| first && n > 0;
        let jump_press = press(inp.jump_presses);
        let crouch_press = press(inp.crouch_presses);
        let prone_press = press(inp.prone_presses);

        // ---- sprint ----
        let mut stance_ev = Vec::new();
        self.stance.advance(t, &mut stance_ev);
        let low = self.stance.goal() != StanceKind::Stand;
        let sprint_wanted = press(inp.sprint_presses) || (cfg.sprint.mode == crate::sprint::SprintMode::Hold && inp.sprint_held);
        let can_stand = if low && sprint_wanted && !self.sprint.active() { self.headroom_for(d, cfg, b, w, StanceKind::Stand) } else { true };
        let sin = SprintIn {
            presses: if first { inp.sprint_presses } else { 0 },
            held: inp.sprint_held,
            forwardmove: cmd.forwardmove,
            fire: inp.fire,
            ads: inp.ads,
            reload: first && inp.reload,
            jump: jump_press && !low,
            crouch: crouch_press,
            prone: prone_press,
            low_stance: low,
            can_stand,
        };
        // key events happen at the start of the tick (the press time), level rules at the end of each substep
        let t_ev = if first { t - h as f64 } else { t };
        let sev = self.sprint.update(&cfg.sprint, &sin, t_ev);
        if let Some(e) = sev {
            out.push(SimEvent::Sprint(e));
            if let SprintEvent::Start { from_low_stance: true, .. } = e {
                let mut yes = |_: StanceKind| true; // headroom was checked by the start
                self.stance.request(StanceKind::Stand, t_ev, self.grounded, &mut yes, &mut stance_ev);
            }
        }

        // ---- stance keys (C / LCtrl / Space as +gostand) ----
        let keys = StanceKeys {
            crouch_press: crouch_press && !cfg.crouch_hold,
            prone_press,
            gostand_press: jump_press,
            crouch_held: cfg.crouch_hold.then_some(inp.crouch_held),
            hold_tap: first && inp.hold_tap,
            hold_long: first && inp.hold_long,
        };
        self.stance.gostand_from_prone = cfg.gostand_from_prone;
        let grounded = self.grounded;
        let mut hr_cache: [Option<bool>; 3] = [None; 3];
        // the closure only needs the feet, which do not change before keys() returns
        let pos = self.pos;
        let mut hr = |k: StanceKind| -> bool { headroom_at(d, cfg, b, w, pos, k, &mut hr_cache) };
        let gostand = self.stance.keys(&keys, t_ev, grounded, &mut hr, &mut stance_ev);
        for e in stance_ev {
            out.push(SimEvent::Stance(e));
        }

        // ---- M8 mantle: MW2 checks before the jump, every tick, on the ground or in the air (PmoveSingle order) ----
        if !gostand && !*ms.detected && self.mantle_gate(d, cfg, inp, cmd, jump_press, t) {
            *ms.detected = true;
            st.mantle_checks += 1;
            if self.try_mantle(d, cfg, inp, jump_press, t, ms, w, out) {
                // a grounded press that mantles replaces the jump: no Jump event, no slowdown
                return;
            }
        }

        // ---- jump ----
        if jump_press
            && !gostand
            && self.grounded
            && self.stance.goal() == StanceKind::Stand
            && !self.stance.in_transition()
            && t - self.last_jump_t >= m.jump_min_interval_ms as f64 / 1000.0
        {
            let lf = pmove::land_factor(m, self.slow_ms);
            let v0 = if self.slow_ms > 0.0 { pmove::jump_v0_slowed(m, self.slow_ms) } else { pmove::jump_v0(m) };
            self.vel.y = v0;
            self.grounded = false;
            self.jumping = true;
            self.jump_origin_y = self.pos.y;
            self.jump_t = t - h as f64;
            self.last_jump_t = t;
            self.air_top_y = self.pos.y;
            self.apex_sent = false;
            self.ceiling_hit = false;
            self.freefall_sent = false;
            self.leave_ground();
            out.push(SimEvent::Jump { y0: self.pos.y, v0, land_factor: lf });
        }

        // ---- wish ----
        let sprinting = self.sprint.active();
        let scale = ScaleIn {
            sprinting,
            stance_scale: self.stance.speed_scale(m, t),
            ads: inp.ads,
            weapon_move: d.weapon.move_speed_scale,
            weapon_ads_move: d.weapon.ads_move_speed_scale,
            class_scale: cfg.class_scale,
            lightweight: cfg.lightweight,
            speed_scale: cfg.speed_scale,
        };
        let wish = pmove::wishspeed(m, cmd, &scale);
        let rm = pmove::effective_rightmove(m, cmd, sprinting);
        let dir = pmove::wishdir(inp.fwd_h, inp.right_h, cmd.forwardmove, rm);
        self.last_wish = wish;

        // ---- friction / accelerate ----
        if self.grounded {
            let mut fs = if self.land_ms > 0.0 { m.land_friction_scale } else { 1.0 };
            if self.slow_ms > 0.0 {
                fs *= pmove::land_factor(m, self.slow_ms);
            }
            self.vel = pmove::friction(m, self.vel, h, fs);
            if let Some(dir) = dir {
                let mut accel = self.stance.effective().accel(m);
                if self.land_ms > 0.0 {
                    accel *= m.time_land_accel_scale;
                }
                self.vel = pmove::accelerate(m, self.vel, dir, wish, accel, h);
            }
            self.vel.y = 0.0;
        } else if let Some(dir) = dir {
            let vy = self.vel.y;
            self.vel = pmove::accelerate(m, V3::new(self.vel.x, 0.0, self.vel.z), dir, wish, m.air_accel, h);
            self.vel.y = vy;
        }

        // ---- horizontal collide and slide ----
        let before = self.pos;
        // the push-out of this tick rides on the first substep's move (walls stop it like the walk)
        let push = if first && inp.push.is_finite() { V3::new(inp.push.x, 0.0, inp.push.z) } else { V3::ZERO };
        let dmove = V3::new(self.vel.x * h + push.x, 0.0, self.vel.z * h + push.z);
        self.slide(d, cfg, b, w, dmove, st);
        let moved = (self.pos - before).planar_len();
        if moved > 1e-4 {
            self.last_dir = V3::new(self.pos.x - before.x, 0.0, self.pos.z - before.z) * (1.0 / moved);
        }
        if self.grounded {
            self.walked += moved;
            self.bob_phase = (self.walked / 36.0 * std::f32::consts::PI) % std::f32::consts::TAU;
        }

        // ---- vertical ----
        let cap = self.capsule(d, cfg);
        if self.grounded {
            match down_ray(b, w, self.pos, self.pos.y + cap.step + MARGIN, self.pos.y - cap.step) {
                Ok(Some(g)) => {
                    self.pos.y = g.y;
                    let moving = self.vel.planar_len() > 1.0;
                    if (moving || !self.normal_fresh || self.normal.y < cfg.walkable_y)
                        && let Some(n) = ground_normal(b, w, self.pos, g.y, self.last_dir, cap.step)
                    {
                        self.normal = n;
                        self.normal_fresh = true;
                    }
                    if self.normal.y < cfg.walkable_y {
                        // too steep: slide off it (airborne, clipped against the slope)
                        self.grounded = false;
                        self.jumping = false;
                        self.air_top_y = self.pos.y;
                        self.freefall_sent = false;
                        self.leave_ground();
                        self.vel = pmove::clip_velocity(self.vel, self.normal, 1.001);
                    }
                }
                Ok(None) if inp.er_ground == Some(true) => {
                    // our map ray sees no floor but ER stands on something (a lift, a platform, collision the map
                    // filter or streaming misses): keep the feet; the DLL hands back if this lasts
                    st.ground_held += 1;
                }
                Ok(None) => {
                    // walked off a ledge
                    self.leave_ground();
                    self.grounded = false;
                    self.jumping = false;
                    self.vel.y = 0.0;
                    self.air_top_y = self.pos.y;
                    self.freefall_sent = false;
                    self.apex_sent = true;
                    self.normal = UP;
                }
                Err(()) => {}
            }
        } else {
            let y0 = self.pos.y;
            let vy0 = self.vel.y;
            let (dy, mut vy1) = pmove::gravity_step(m.gravity, vy0, h);
            let mut y1 = y0 + dy;
            if y1 > y0
                && let Some(maxy) = ceiling(b, w, self.pos, cap.hull, y1)
                && y1 > maxy
            {
                y1 = maxy.max(y0);
                vy1 = 0.0;
                self.ceiling_hit = true;
            }
            if self.jumping {
                vy1 = pmove::jump_clamp_vy(m, vy1, y1, self.jump_origin_y);
            }
            // apex
            if !self.apex_sent && vy0 > 0.0 && vy1 <= 0.0 {
                self.apex_sent = true;
                let (apex_y, t_apex) = if self.ceiling_hit {
                    (y1, t - h as f64)
                } else {
                    (y0 + vy0 * vy0 / (2.0 * m.gravity), t - h as f64 + (vy0 / m.gravity) as f64)
                };
                if self.jumping {
                    out.push(SimEvent::Apex { h: apex_y - self.jump_origin_y, t: t_apex - self.jump_t, ceiling: self.ceiling_hit });
                }
                self.air_top_y = self.air_top_y.max(apex_y);
            }
            self.air_top_y = self.air_top_y.max(y1);
            self.pos.y = y1;
            self.vel.y = vy1;
            if vy1 <= 0.0 {
                let top = y0.max(y1) + 2.0;
                if let Ok(Some(g)) = down_ray(b, w, self.pos, top, y1 - 0.01) {
                    let n = ground_normal(b, w, V3::new(self.pos.x, g.y, self.pos.z), g.y, self.last_dir, cap.step).unwrap_or(UP);
                    self.pos.y = g.y;
                    self.normal_fresh = true;
                    if n.y >= cfg.walkable_y {
                        self.normal = n;
                        let v2 = (vy0 * vy0 + 2.0 * m.gravity * (y0 - g.y)).max(0.0);
                        let fall_h = v2 / (2.0 * m.gravity);
                        // the touchdown inside this substep (exact for constant gravity): for the bench's distances
                        let t_hit = ((vy0 + v2.sqrt()) / m.gravity).clamp(0.0, h);
                        self.land_late_s = h - t_hit;
                        self.land_vel_pre = V3::new(self.vel.x, 0.0, self.vel.z);
                        self.land(d, cfg, g.y, fall_h, t, out);
                    } else {
                        self.normal = n;
                        self.vel = pmove::clip_velocity(self.vel, n, 1.001);
                    }
                }
            }
            if !self.grounded && !self.freefall_sent && self.air_top_y - self.pos.y > cfg.freefall_in {
                self.freefall_sent = true;
                out.push(SimEvent::Freefall { fallen: self.air_top_y - self.pos.y });
            }
        }
    }

    fn land(&mut self, d: &MoveData, cfg: &SimCfg, gy: f32, fall_h: f32, t: f64, out: &mut Vec<SimEvent>) {
        let m = &d.mw2;
        let crouched = self.stance.goal() != StanceKind::Stand;
        let below = gy < self.jump_origin_y + m.slowdown_low_band;
        let l = fall::land(m, fall_h, crouched, self.jumping, below, cfg.jump_slowdown);
        self.vel.x *= l.vel_scale;
        self.vel.z *= l.vel_scale;
        self.vel.y = 0.0;
        if l.land_ms > 0.0 {
            self.land_ms = l.land_ms;
        }
        if l.slow_ms > 0.0 {
            self.slow_ms = l.slow_ms;
        }
        if l.dip_in > 0.0 {
            self.dip_in = l.dip_in;
            self.dip_t = t;
        }
        self.grounded = true;
        self.jumping = false;
        self.air_top_y = gy;
        self.freefall_sent = false;
        out.push(SimEvent::Land(l));
    }

    /// M8: the zero-ray gate of section 3 (data + config, not in a re-arm / hold, standing, the jump key by
    /// `key_mode`, the optional forward rule, the negative cache).
    fn mantle_gate(&mut self, d: &MoveData, cfg: &SimCfg, inp: &Input, cmd: Cmd, jump_press: bool, t: f64) -> bool {
        let c = &cfg.mantle;
        if !c.enabled || !d.mantle.as_ref().is_some_and(|m| m.enable) || inp.mantle_block || t < self.mantle_block_until {
            return false;
        }
        // MW2: standing only (eFlags & 0xC), no stance change running
        if self.stance.effective() != StanceKind::Stand || self.stance.in_transition() {
            return false;
        }
        let key = match c.key_mode {
            KeyMode::Mw2 => inp.jump_held || jump_press,
            KeyMode::Press => jump_press,
            KeyMode::Auto => inp.jump_held || jump_press || (cmd.forwardmove > 0.0 && self.vel.planar_len() > d.mw2.move_threshold),
        };
        if !key || (c.require_forward && cmd.forwardmove <= 0.0) {
            return false;
        }
        self.refuse_cache.open(t, self.pos, inp.fwd_h)
    }

    /// M8: one detection; on success the mantle starts at the current feet (the trigger tick writes them unchanged).
    #[allow(clippy::too_many_arguments)]
    fn try_mantle(&mut self, d: &MoveData, cfg: &SimCfg, inp: &Input, jump_press: bool, t: f64, ms: &mut MantleStep, w: &mut dyn RayWorld, out: &mut Vec<SimEvent>) -> bool {
        let Some(md) = d.mantle.as_ref() else { return false };
        let left = ms.left;
        let mb: &mut RayBudget = &mut *ms.mb;
        let m = &d.mw2;
        let n = (inp.n_others as usize).min(MAX_OTHERS);
        let q = Query {
            feet: self.pos,
            fwd: inp.fwd_h,
            radius: cfg.radius,
            skin: cfg.skin,
            hull_stand: m.hull_stand * cfg.v_scale,
            hull_crouch: m.hull_crouch * cfg.v_scale,
            ground_y0: if self.grounded { self.pos.y } else { self.ground_y0 },
            walkable_y: cfg.walkable_y,
            others: &inp.others[..n],
        };
        let used0 = mb.used;
        let held = inp.jump_held || jump_press;
        let res: Result<Ledge, (Refuse, f32)> = match mantle::find_ledge(mb, w, &q, &cfg.mantle, md) {
            // the MP automantle (no jump key): only while moving into the face
            Ok(l) if !held && self.vel.dot(l.dir) < m.move_threshold => Err((Refuse::NoFace, l.rise)),
            r => r,
        };
        match res {
            // M8 review: never start a climb the rest of this tick could not re-validate; try again next tick (no
            // refusal, no negative cache)
            Ok(_) if mb.max.saturating_sub(mb.used) < mantle::SUBSTEP_RAYS * left => {
                *ms.deferred += 1;
                false
            }
            Ok(l) => {
                if let Some(e) = self.sprint.force_end(&cfg.sprint, t, crate::sprint::EndWhy::Mantle) {
                    out.push(SimEvent::Sprint(e));
                }
                if l.kind == MantleKind::Crouch {
                    let mut yes = |_: StanceKind| true; // the crouch headroom is what the detection proved
                    let mut sev = Vec::new();
                    self.stance.request(StanceKind::Crouch, t, true, &mut yes, &mut sev);
                    out.extend(sev.into_iter().map(SimEvent::Stance));
                }
                let trigger = if jump_press {
                    Trigger::Press
                } else if inp.jump_held {
                    Trigger::Held
                } else {
                    Trigger::Auto
                };
                let air = !self.grounded;
                // run 18: a start backed off a face the capsule reached into (proved free by the detection's back rays)
                let from = self.pos;
                if l.depen > 0.0 {
                    *ms.depen += 1;
                    self.pos = l.start;
                    self.ncontacts = 0;
                }
                let mut mm = Mantle::plan(&l, self.pos, self.vel.y, md, &cfg.mantle, pmove::jump_v0(m), trigger, air);
                mm.rays = mb.used - used0;
                out.push(SimEvent::MantleStart(MantleStart {
                    kind: l.kind,
                    rise: l.rise,
                    row: l.row,
                    row_height: md.rows.get(l.row).map_or(0.0, |r| r.height),
                    ledge_y: l.ledge_y,
                    from,
                    end: l.end,
                    dir: l.dir,
                    d: l.d,
                    x: l.x,
                    dur: mm.duration(),
                    rays: mm.rays,
                    trigger,
                    air,
                    v0y: mm.v0y,
                    retried: l.retried,
                    drop_weapon: mm.drop_weapon,
                    drop_y: l.drop_y,
                    depen: l.depen,
                    narrow_top: l.narrow_top,
                    line_up: l.line_up,
                }));
                self.mantle = Some(mm);
                self.grounded = false;
                self.jumping = false;
                self.vel = V3::ZERO;
                self.air_top_y = self.pos.y;
                self.freefall_sent = false;
                self.apex_sent = true;
                self.ncontacts = 0;
                self.mantle_pending = (0, 0);
                true
            }
            Err((why, rise)) => {
                self.refuse_cache.refused(t, self.pos, inp.fwd_h, cfg.mantle.refuse_cache_s);
                out.push(SimEvent::MantleRefuse { why, rise, rays: mb.used - used0, fwd: inp.fwd_h });
                false
            }
        }
    }

    /// M8: one substep of a running mantle (section 5): the path point, re-validated with at most 4 rays of the mantle
    /// budget; an abort returns to the last validated point.
    #[allow(clippy::too_many_arguments)]
    fn mantle_substep(&mut self, d: &MoveData, cfg: &SimCfg, inp: &Input, first: bool, t: f64, h: f32, b: &mut RayBudget, mb: &mut RayBudget, w: &mut dyn RayWorld, out: &mut Vec<SimEvent>) {
        let Some(mut mm) = self.mantle.take() else { return };
        let mut sev = Vec::new();
        self.stance.advance(t, &mut sev);
        out.extend(sev.into_iter().map(SimEvent::Stance));
        if first {
            self.mantle_pending.0 += inp.crouch_presses;
            self.mantle_pending.1 += inp.prone_presses;
        }
        // ER's own displacement is measured, never applied, during a mantle; a large one is a knockback / grab
        if first && inp.er_delta_in.is_finite() && inp.er_delta_in > cfg.mantle.er_push_abort_in {
            self.mantle = Some(mm);
            if let Some(e) = self.mantle_abort(AbortWhy::ErPush) {
                out.push(e);
            }
            return;
        }
        let prev = self.pos;
        let t1 = mm.t + h as f64;
        let (mut target, phase) = mm.at(t1);
        if phase == Phase::Rise {
            target.y = target.y.max(mm.y_max);
        }
        let hull = if mm.kind == MantleKind::Crouch { d.mw2.hull_crouch * cfg.v_scale } else { self.stance.hull_in(&d.mw2) * cfg.v_scale };
        let check = |mb: &mut RayBudget, w: &mut dyn RayWorld, mm: &mut Mantle| -> Result<(), AbortWhy> {
            let cast = |mb: &mut RayBudget, w: &mut dyn RayWorld, o: V3, dl: V3| mb.cast(w, RayKind::Vertical, o, dl).map_err(|_| AbortWhy::Budget);
            match phase {
                Phase::Rise => {
                    let dy = target.y - prev.y;
                    if dy > 1e-4 {
                        if cast(mb, w, prev + UP * (hull - 1.0), UP * (dy + 1.0))?.is_some() {
                            return Err(AbortWhy::Blocked);
                        }
                        // M8 review: the head's new height toward the face (on the axis the up ray just proved): rock
                        // leaning over the climb that the detection's front column missed aborts here
                        let head = V3::new(prev.x, target.y + hull - 4.0, prev.z);
                        if cast(mb, w, head, mm.dir * (cfg.radius + 0.5 * cfg.skin))?.is_some() {
                            return Err(AbortWhy::Blocked);
                        }
                    }
                }
                Phase::Cross | Phase::Over => {
                    let step = V3::new(target.x - prev.x, 0.0, target.z - prev.z);
                    let l = step.planar_len();
                    if l > 0.01 {
                        let dir = step * (1.0 / l);
                        // feet, waist and head: 3 rays (+ the ledge check: at most 4 per substep). The feet ray only
                        // covers the axis' own step: a top rising ahead (a rock crown) is ground, not a wall. It runs
                        // just under the crossing height (M8 review: the detection's R8 line), not above the feet
                        let feet = V3::new(prev.x, mm.y_top - 0.25, prev.z);
                        let rays = [(feet, cfg.skin), (prev + UP * (hull * 0.5), cfg.radius + cfg.skin), (prev + UP * (hull - 4.0), cfg.radius + cfg.skin)];
                        for (o, ahead) in rays {
                            if cast(mb, w, o, dir * (l + ahead))?.is_some() {
                                return Err(AbortWhy::Blocked);
                            }
                        }
                    }
                }
            }
            // the top is still there (half way and near the end of the climb)
            let f = (t1 / mm.duration().max(1e-6)) as f32;
            for (flag, at) in [(0usize, 0.5f32), (1, 0.9)] {
                let done = if flag == 0 { mm.checked_half } else { mm.checked_end };
                if !done && f >= at {
                    if flag == 0 {
                        mm.checked_half = true;
                    } else {
                        mm.checked_end = true;
                    }
                    let o = V3::new(mm.top.x, mm.ledge_y + 4.0, mm.top.z);
                    if cast(mb, w, o, UP * -8.0)?.is_none() {
                        return Err(AbortWhy::LedgeGone);
                    }
                }
            }
            Ok(())
        };
        if let Err(why) = check(mb, w, &mut mm) {
            self.mantle = Some(mm);
            if let Some(e) = self.mantle_abort(why) {
                out.push(e);
            }
            return;
        }
        mm.t = t1;
        mm.y_max = mm.y_max.max(target.y);
        self.vel = (target - prev) * (1.0 / h.max(1e-6));
        self.pos = target;
        mm.last_valid = target;
        self.air_top_y = self.pos.y;
        if !mm.done() {
            self.mantle = Some(mm);
            return;
        }
        // ---- the end ----
        let planar_v = V3::new(self.vel.x, 0.0, self.vel.z);
        let sp = planar_v.planar_len();
        let vmax = cfg.mantle.end_speed_max.max(0.0);
        let planar_v = if sp > vmax { planar_v * (vmax / sp) } else { planar_v };
        if mm.kind == MantleKind::Over {
            // MW2's over drop: released into normal gravity (no jump: no slowdown at the landing)
            self.grounded = false;
            self.vel = V3::ZERO;
            self.jumping = false;
            self.air_top_y = self.pos.y;
            self.freefall_sent = false;
            self.apex_sent = true;
            self.leave_ground();
            out.push(SimEvent::MantleEnd { dt: mm.t, kind: mm.kind, pos: self.pos, dist_end: (self.pos - mm.end).len(), vel: self.vel });
            out.push(SimEvent::MantleRelease { drop_to: mm.drop_y, pos: self.pos });
        } else {
            self.pos = mm.end;
            self.grounded = true;
            // MW2 Mantle_Move: vel.z = min(vel.z, mantle_max_vertical_end_velocity) (V, the dvar is 0); grounded on
            // the top, so never downward either
            let vy_max = d.mantle.as_ref().map_or(0.0, |m| m.max_vert_end_vel.max(0.0));
            self.vel = V3::new(planar_v.x, self.vel.y.min(vy_max).max(0.0), planar_v.z);
            self.jumping = false;
            self.air_top_y = self.pos.y;
            self.ground_y0 = self.pos.y;
            self.freefall_sent = false;
            self.apex_sent = true;
            self.normal = UP;
            self.normal_fresh = false;
            out.push(SimEvent::MantleEnd { dt: mm.t, kind: mm.kind, pos: self.pos, dist_end: (self.pos - mm.end).len(), vel: self.vel });
        }
        // the crouch / prone presses made during the climb apply now
        let (c, p) = std::mem::take(&mut self.mantle_pending);
        if c > 0 || p > 0 {
            let keys = StanceKeys { crouch_press: c % 2 == 1 && !cfg.crouch_hold, prone_press: p % 2 == 1, gostand_press: false, crouch_held: None, hold_tap: false, hold_long: false };
            let pos = self.pos;
            let mut cache: [Option<bool>; 3] = [None; 3];
            let mut hr = |k: StanceKind| -> bool { headroom_at(d, cfg, b, w, pos, k, &mut cache) };
            let mut sev = Vec::new();
            self.stance.keys(&keys, t, self.grounded, &mut hr, &mut sev);
            out.extend(sev.into_iter().map(SimEvent::Stance));
        }
    }

    fn headroom_for(&self, d: &MoveData, cfg: &SimCfg, b: &mut RayBudget, w: &mut dyn RayWorld, k: StanceKind) -> bool {
        headroom(b, w, self.pos, self.pos.y, self.pos.y + k.hull(&d.mw2) * cfg.v_scale + MARGIN)
    }

    /// Remembered contact planes (section 5.2): a wall touched earlier is re-checked with ONE ray straight at it while
    /// the move still pushes into it, so a grazing move (whose forward rays run almost parallel to the wall) cannot
    /// creep into it. Returns the move with the into-wall part removed.
    fn apply_contacts(&mut self, b: &mut RayBudget, w: &mut dyn RayWorld, mut dmove: V3, cap: &Capsule) -> V3 {
        let want = cap.radius + cap.skin;
        let mut k = 0;
        while k < self.ncontacts {
            let (pt, n, hgt) = self.contacts[k];
            let c = self.pos + UP * hgt;
            let dist = (c - pt).dot(n);
            if dist > want + 6.0 {
                self.drop_contact(k);
                continue;
            }
            if dmove.dot(n) >= 0.0 {
                k += 1;
                continue;
            }
            match b.cast(w, crate::move_collide::RayKind::Horizontal, c, -n * (want + 2.0 + (dist - want).max(0.0))) {
                Ok(Some(hit)) => {
                    let dist = (c - hit).dot(n);
                    self.contacts[k] = (hit, n, hgt);
                    let room = (dist - want).max(0.0);
                    let into = -dmove.dot(n);
                    if into > room {
                        dmove = dmove + n * (into - room);
                        let vin = -self.vel.dot(n);
                        if vin > 0.0 {
                            self.vel = self.vel + n * vin;
                        }
                    }
                    k += 1;
                }
                Ok(None) => self.drop_contact(k),
                Err(()) => {
                    // budget spent: keep the plane, block the into-wall part conservatively
                    let room = (dist - want).max(0.0);
                    let into = -dmove.dot(n);
                    if into > room {
                        dmove = dmove + n * (into - room);
                    }
                    k += 1;
                }
            }
        }
        dmove
    }

    fn drop_contact(&mut self, k: usize) {
        self.contacts[k] = self.contacts[self.ncontacts - 1];
        self.ncontacts -= 1;
    }

    fn push_contact(&mut self, point: V3, n: V3, height: f32) {
        for c in self.contacts[..self.ncontacts].iter_mut() {
            if c.1.dot(n) > 0.98 {
                *c = (point, n, height);
                return;
            }
        }
        if self.ncontacts < self.contacts.len() {
            self.contacts[self.ncontacts] = (point, n, height);
            self.ncontacts += 1;
        } else {
            self.contacts[0] = (point, n, height);
        }
    }

    /// Whether the surface just past a low-ray hit is walkable floor within a step (a stair tread or a walkable
    /// slope): then the low rays do not block, and the ground probe lifts the feet once the centre is over it.
    fn floor_ahead(&self, b: &mut RayBudget, w: &mut dyn RayWorld, dir: V3, hit: V3, cap: &Capsule) -> bool {
        let q = V3::new(hit.x, self.pos.y, hit.z) + dir * 2.0;
        let g = match down_ray(b, w, q, self.pos.y + cap.step + MARGIN, self.pos.y - 0.5) {
            Ok(Some(g)) => g,
            Ok(None) => return self.crest(b, w, dir, hit, cap),
            Err(()) => return false,
        };
        let rise = g.y - self.pos.y;
        if rise > cap.step {
            return false;
        }
        match ground_normal(b, w, q, g.y, dir, cap.step) {
            Some(n) => n.y >= cap.walkable_y,
            // an edge (stairs) or the budget: a tread within a step counts as floor
            None => true,
        }
    }

    /// 2026-10-21: the low ray met the top of a walkable slope with nothing within a step just past it (a roof's top
    /// edge, a ramp over a drop): floor when the surface just before the hit is at the hit's own height (a top, not a
    /// vertical face: in front of a face the down ray reaches the floor under it) and the slope leading up to it,
    /// measured backwards, is walkable. Once the centre is past the edge the ground probe walks off it.
    fn crest(&self, b: &mut RayBudget, w: &mut dyn RayWorld, dir: V3, hit: V3, cap: &Capsule) -> bool {
        let q = V3::new(hit.x, self.pos.y, hit.z) - dir * 0.5;
        let Ok(Some(g)) = down_ray(b, w, q, hit.y + 1.0, self.pos.y - 0.5) else { return false };
        if g.y < hit.y - 1.0 || g.y - self.pos.y > cap.step {
            return false;
        }
        matches!(ground_normal(b, w, q, g.y, -dir, cap.step), Some(n) if n.y >= cap.walkable_y)
    }

    /// Q3-style slide (section 5.2): low hits on walkable floor within a step are not walls (stairs, ramps); the
    /// closest remaining hit is a wall: stop at its plane, clip the rest of the move and the velocity, at most
    /// `max_bumps` times; two opposing walls (a corner) stop the horizontal motion.
    fn slide(&mut self, d: &MoveData, cfg: &SimCfg, b: &mut RayBudget, w: &mut dyn RayWorld, dmove: V3, st: &mut TickStats) {
        let cap = self.capsule(d, cfg);
        let mut dmove = self.apply_contacts(b, w, dmove, &cap);
        let mut planes: [V3; 4] = [V3::ZERO; 4];
        let mut np = 0usize;
        let (hs, nh) = if cfg.body_probes { cap.heights_body() } else { cap.heights_m5() };
        for _ in 0..cfg.max_bumps.max(1) {
            let len = dmove.planar_len();
            if len < 1e-3 {
                break;
            }
            let dir = dmove * (1.0 / len);
            let mut s = sweep(b, w, self.pos, dmove, &cap, &hs[..nh]);
            if s.refused {
                // budget spent: the horizontal part of this substep is dropped
                break;
            }
            // run 18: the front body column catches a slab between the probe heights (a cart bed, a table top)
            if cfg.body_probes {
                match body_column(b, w, self.pos, dmove, &cap, &hs[..nh]) {
                    Err(()) => break,
                    Ok(Some(hit)) => {
                        st.column_hits += 1;
                        s.hits.push(hit);
                    }
                    Ok(None) => {}
                }
            }
            let reach = len + DETECT_EXTRA * cap.radius + cap.skin;
            if self.grounded
                && let Some(low) = s.hits.iter().filter(|h| h.height == hs[0] && h.allow < reach).min_by(|a, b| a.allow.total_cmp(&b.allow)).copied()
                && self.floor_ahead(b, w, dir, low.point, &cap)
            {
                s.hits.retain(|h| h.height != hs[0]);
                st.steps += 1;
            }
            // run 18: a probe that found the wall deeper inside the capsule than the skin: penetration evidence.
            // 2026-10-21: after the floor exemption (the low ray meets a 21 deg slope 10 in ahead, inside the radius:
            // floor, not a wall the capsule is in)
            if s.hits.iter().any(|h| h.allow < -(cap.skin + 0.5)) {
                st.inside += 1;
            }
            let Some(first) = s.min_hit().filter(|f| f.allow < reach) else {
                self.pos = self.pos + dmove;
                break;
            };
            let n = wall_normal(b, w, self.pos, dir, &first, &s, &cap);
            let c = self.pos + UP * first.height;
            let mut allow = plane_allow(c, dir, first.point, n, cap.radius, cap.skin).unwrap_or(first.allow);
            for hh in &s.hits {
                if (hh.point - first.point).dot(n).abs() > 1.0 {
                    allow = allow.min(hh.allow);
                }
            }
            // 2026-10-21 (the roof): an airborne low hit on walkable floor within a step (a slope or stair ahead) stops
            // this move like a wall, but is never remembered as one: the contact re-check is one ray at the low height,
            // which on an uphill slope hits the slope again every substep and blocked the walk after the landing
            let remember = self.grounded || first.height != hs[0] || !self.floor_ahead(b, w, dir, first.point, &cap);
            if allow >= len {
                // near the wall but this move stays outside its skin: move, and remember the wall
                self.pos = self.pos + dmove;
                if remember {
                    self.push_contact(first.point, n, first.height);
                }
                break;
            }
            st.bumps += 1;

            let allow = allow.clamp(0.0, len);
            self.pos = self.pos + dir * allow;
            // gentle push-out if we ended up inside the skin of this wall
            let dist = (self.pos + UP * first.height - first.point).dot(n);
            let want = cap.radius + cap.skin;
            if dist < want - 0.05 {
                self.pos = self.pos + n * (want - dist).min(0.5);
            }
            let rem = dmove * (1.0 - allow / len);
            let mut rem = pmove::clip_velocity(V3::new(rem.x, 0.0, rem.z), n, 1.001);
            rem.y = 0.0;
            let mut vh = pmove::clip_velocity(V3::new(self.vel.x, 0.0, self.vel.z), n, 1.001);
            vh.y = 0.0;
            if remember {
                self.push_contact(first.point, n, first.height);
            }
            let mut crease = false;
            for p in &planes[..np] {
                if rem.dot(*p) < -1e-4 || p.dot(n) < -0.1 {
                    crease = true;
                }
            }
            if crease {
                // a corner (two walls): stop the horizontal motion
                self.vel.x = 0.0;
                self.vel.z = 0.0;
                break;
            }
            if np < planes.len() {
                planes[np] = n;
                np += 1;
            }
            self.vel.x = vh.x;
            self.vel.z = vh.z;
            dmove = rem;
        }
    }
}

/// M8: the per-tick mantle state threaded through the substeps: the mantle's own ray budget and the one-detection-
/// per-tick flag.
pub struct MantleStep<'a> {
    pub mb: &'a mut RayBudget,
    pub detected: &'a mut bool,
    /// Substeps of this tick after the current one (each re-validates with up to `SUBSTEP_RAYS`).
    pub left: u32,
    pub deferred: &'a mut u32,
    /// Run 18: mantle starts backed off a face (counted as penetration evidence).
    pub depen: &'a mut u32,
}

/// Headroom with a per-substep cache (the stance key closure).
fn headroom_at(d: &MoveData, cfg: &SimCfg, b: &mut RayBudget, w: &mut dyn RayWorld, pos: V3, k: StanceKind, cache: &mut [Option<bool>; 3]) -> bool {
    let i = k as usize;
    if let Some(v) = cache[i] {
        return v;
    }
    let v = headroom(b, w, pos, pos.y, pos.y + k.hull(&d.mw2) * cfg.v_scale + MARGIN);
    cache[i] = Some(v);
    v
}

#[cfg(test)]
#[path = "move_sim_tests.rs"]
mod tests;
