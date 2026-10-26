//! M6: the arsenal controller run by the game task (docs/M6_DESIGN.md sections 4-8, 11): which gun is in hand,
//! weapon switching (keys 1/2 + the wheel, MW2 drop / raise), the per-gun parked state (ammo, sim, spread are kept
//! across switches and MODE off/on), view kick / ADS sway / hold breath / ADS zoom published to the camera-step detour,
//! the launchers' simplified lock-on and projectiles. Main thread only; the shot pipeline stays in task.rs.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mw2er_core::arsenal::{GunSpec, LockKind, MoveClass};
use mw2er_core::config::Config;
use mw2er_core::input_logic::{CounterReader, wheel_steps};
use mw2er_core::kick::{Breath, BreathEvent, BreathParams, GunKick, ViewKick, sway_offset, zoom_ratio};
use mw2er_core::projectile::{Lock, LockEvent, PointLock, Projectile, STINGER_GRACE_S, STINGER_LOCK_S};
use mw2er_core::spread::{SpreadParams, SpreadState, StanceSpread};
use mw2er_core::switch_logic::{GunTimes, SwitchEvent, SwitchIn, SwitchMachine, Via};
use mw2er_core::vec3::V3;
use mw2er_core::vm_anim::{VmParams, VmState, notes_between};
use mw2er_core::vm_pose::clip_frame;
use mw2er_core::weapon_sim::{SimParams, WeaponSim};
use mw2er_core::weapons::FireMode;

use crate::arsenal_rt::{ArsenalData, GunData, vm_failed};
use crate::log::{log_i, log_w};
use crate::shared::{self, ARSENAL_MOVE_CLASS, ARSENAL_WHEEL, PRESSES_WEAPON};

/// MW2's hip fov (cg_fov 65) for the ADS zoom ratio [I].
pub const MW2_HIP_FOV_DEG: f32 = 65.0;

/// One gun's state while it is not in hand (the gun in hand keeps its sim in the task).
pub struct Parked {
    pub sim: Option<WeaponSim>,
    pub spread: SpreadState,
}

/// A launcher explosion kept for HIT attribution.
#[derive(Clone, Copy, Debug)]
pub struct LBlast {
    pub t: f64,
    pub pos: V3,
    pub radius_m: f32,
    pub gun: usize,
    pub tier: u8,
}

pub struct ArsenalCtl {
    pub data: Arc<ArsenalData>,
    pub cur: usize,
    pub sw: SwitchMachine,
    pub parked: Vec<Parked>,
    pub params_ok: Vec<Option<bool>>,
    readers: [CounterReader; 2],
    wheel_acc: i32,
    pub kick: ViewKick,
    pub gunkick: GunKick,
    pub breath: Breath,
    pub lock: Lock,
    pub plock: PointLock,
    pub projs: Vec<Projectile>,
    pub next_proj: u32,
    pub blasts: Vec<LBlast>,
    /// Review M6: the havok frame (pos_w) the live projectiles were launched in (a change expires them).
    pub proj_w: Option<f32>,
    /// Review M6: player HP before each launcher explosion (t, hp, gun, projectile id), logged 0.6 s later.
    pub blast_hp: Vec<(f64, i32, usize, u32)>,
    pub switch_until: f64,
    blocked_log_t: f64,
    /// Notetracks: the clip play last scanned (seq, gun) and its frame.
    note_seq: u32,
    note_gun: u16,
    note_frame: Option<f32>,
    pub lock_refused_t: f64,
}

fn times(g: &GunSpec) -> GunTimes {
    GunTimes { drop_s: g.drop_s, raise_s: g.raise_s, first_raise_s: g.first_raise_s }
}

impl ArsenalCtl {
    pub fn new(data: Arc<ArsenalData>, cfg: &Config) -> ArsenalCtl {
        let start = data.index_of(cfg.start_gun()).unwrap_or(0);
        let n = data.guns.len();
        let parked = (0..n).map(|_| Parked { sim: None, spread: SpreadState::default() }).collect();
        log_i!(
            "ARSENAL",
            "cfg guns={} start={} reserve_source={:?} flight={} lock={} pellet_bands={} wheel={} keys={}/{} rechamber_mode={} both_barrels={}",
            n,
            data.guns[start].name,
            cfg.arsenal.reserve_source,
            cfg.launchers.flight,
            cfg.launchers.lock as u8,
            cfg.arsenal.pellet_bands,
            cfg.arsenal.wheel as u8,
            cfg.keys.weapon_next,
            cfg.keys.weapon_prev,
            cfg.arsenal.rechamber_mode,
            cfg.arsenal.both_barrels as u8
        );
        for (name, why) in &data.disabled {
            log_w!("ARSENAL", "disable gun={name} why={why} (not in the cycle)");
        }
        ArsenalCtl {
            cur: start,
            sw: SwitchMachine::new(start, n),
            parked,
            params_ok: vec![None; n],
            readers: [0, 1].map(|i| CounterReader::starting_at(PRESSES_WEAPON[i].load(Ordering::SeqCst))),
            wheel_acc: 0,
            kick: ViewKick::default(),
            gunkick: GunKick::default(),
            breath: Breath::default(),
            lock: Lock::default(),
            plock: PointLock::default(),
            projs: Vec::new(),
            next_proj: 1,
            blasts: Vec::new(),
            proj_w: None,
            blast_hp: Vec::new(),
            switch_until: f64::NEG_INFINITY,
            blocked_log_t: f64::NEG_INFINITY,
            note_seq: u32::MAX,
            note_gun: u16::MAX,
            note_frame: None,
            lock_refused_t: f64::NEG_INFINITY,
            data,
        }
    }

    pub fn gun(&self, i: usize) -> &GunData {
        &self.data.guns[i]
    }

    pub fn cur_gun(&self) -> &GunData {
        &self.data.guns[self.cur]
    }

    pub fn spec(&self) -> &GunSpec {
        &self.data.guns[self.cur].spec
    }

    /// Enabled for switching: rows ok (PARAMS), viewmodel not failed.
    pub fn enabled(&self) -> Vec<bool> {
        (0..self.data.guns.len()).map(|i| self.params_ok[i] != Some(false) && !vm_failed(i)).collect()
    }

    pub fn sim_params(&self, i: usize, cfg: &Config) -> SimParams {
        SimParams::from_gun(
            &self.data.guns[i].spec,
            cfg.gameplay.auto_reload_on_empty,
            cfg.gameplay.max_shots_per_tick,
            cfg.arsenal.rechamber_mode == "overlap",
            cfg.arsenal.both_barrels,
        )
    }

    pub fn spread_params(&self, i: usize) -> SpreadParams {
        let s = &self.data.guns[i].spec.spread;
        SpreadParams { min_deg: s.stand[0], max_deg: s.stand[1], fire_add: s.fire_add, decay_per_s: s.decay_per_s, move_add: s.move_add }
    }

    /// The gun's stance spread with the movement data's MW2 constants (M5).
    pub fn stance_spread(&self, i: usize) -> Option<StanceSpread> {
        let d = crate::movement::move_data()?;
        let s = &self.data.guns[i].spec.spread;
        Some(StanceSpread {
            ducked: s.ducked,
            prone: s.prone,
            ducked_decay: if s.ducked_decay > 0.0 { s.ducked_decay } else { 1.0 },
            prone_decay: if s.prone_decay > 0.0 { s.prone_decay } else { 1.0 },
            move_threshold_mps: d.mw2.aim_spread_move_threshold * 0.0254,
            jump_add: d.mw2.jump_spread_add / 255.0,
        })
    }

    /// A fresh sim for gun `i` with the configured reserve (full magazine).
    pub fn new_sim(&self, i: usize, cfg: &Config) -> WeaponSim {
        let mut s = WeaponSim::new(self.sim_params(i, cfg));
        s.refill_with(self.data.guns[i].spec.refill_reserve(cfg.arsenal.reserve_source));
        s
    }

    /// The VmParams of gun `i` once its bundle is loaded.
    pub fn vm_params(&self, i: usize, cfg: &Config) -> Option<VmParams> {
        crate::viewmodel::gun_assets(i).map(|a| VmParams::from_gun_assets(&a, cfg.viewmodel.crossfade_s, cfg.viewmodel.sprint_speed_mps))
    }

    /// The fire-mode tag for the HUD.
    pub fn fire_tag(&self) -> &'static str {
        match self.spec().fire {
            FireMode::Burst(_) => "BURST",
            FireMode::DoubleBarrel => "2x",
            _ => "",
        }
    }

    /// Publishes the movement class of gun `i` (`weapon_move_class = "auto"`) and logs it.
    pub fn publish_move_class(&self, i: usize) {
        let mc = self.data.guns[i].spec.move_class;
        let code = match mc {
            MoveClass::Rifle | MoveClass::Spread => 1,
            MoveClass::Mg => 2,
            MoveClass::Launcher => 3,
            MoveClass::Smg | MoveClass::Sniper | MoveClass::Other => 0,
        };
        ARSENAL_MOVE_CLASS.store(code, Ordering::Relaxed);
        log_i!("MOVE", "class gun={} class={} scale={}", self.data.guns[i].name, mc.label(), mc.scale());
    }

    /// Requests of this tick: keys 1 / 2 and the wheel (only when `live`; otherwise the counters are drained).
    pub fn take_request(&mut self, live: bool, wheel_on: bool, invert: bool) -> Option<(i32, Via)> {
        let next = self.readers[0].take(PRESSES_WEAPON[0].load(Ordering::SeqCst));
        let prev = self.readers[1].take(PRESSES_WEAPON[1].load(Ordering::SeqCst));
        let dz = ARSENAL_WHEEL.swap(0, Ordering::Relaxed);
        if !live {
            self.wheel_acc = 0;
            return None;
        }
        // DirectInput lZ: positive = wheel forward (up) = previous gun (down = next, section 5.1)
        let notches = if wheel_on { wheel_steps(&mut self.wheel_acc, dz) } else { 0 };
        let wheel = if invert { notches } else { -notches };
        if next > 0 {
            Some((next.min(4) as i32, Via::Key1))
        } else if prev > 0 {
            Some((-(prev.min(4) as i32), Via::Key2))
        } else if wheel != 0 {
            Some((wheel.clamp(-8, 8), Via::Wheel))
        } else {
            None
        }
    }

    /// One switch-machine tick; logs the switch lines (section 5.2) and returns the events for the task.
    pub fn switch_tick(&mut self, now: f64, request: Option<(i32, Via)>, ac130_active: bool, mode_on: bool, busy: bool) -> Vec<SwitchEvent> {
        let en = self.enabled();
        let times: Vec<GunTimes> = self.data.guns.iter().map(|g| times(&g.spec)).collect();
        let mut out = Vec::new();
        self.sw.tick(&SwitchIn { now, enabled: &en, times: &times, ac130_active, mode_on, busy, request }, &mut out);
        for e in &out {
            match *e {
                SwitchEvent::Start { from, to, via, drop_s } => {
                    log_i!("SWITCH", "start from={} to={} via={} drop_s={drop_s:.2}", self.gun(from).name, self.gun(to).name, via.label());
                    self.switch_until = now + 1.5;
                }
                SwitchEvent::Retarget { to } => {
                    log_i!("SWITCH", "retarget to={}", self.gun(to).name);
                    self.switch_until = now + 1.5;
                }
                SwitchEvent::Reversed { from, to, drop_s } => {
                    log_i!("SWITCH", "start from={} to={} via=retarget drop_s={drop_s:.2} (reversed raise)", self.gun(from).name, self.gun(to).name);
                    self.switch_until = now + 1.5;
                }
                SwitchEvent::RaiseStart { gun, first, raise_s } => {
                    log_i!("SWITCH", "raise gun={} first={} raise_s={raise_s:.2}", self.gun(gun).name, first as u8);
                }
                SwitchEvent::Raised { gun, t, first } => {
                    log_i!("SWITCH", "raised gun={} t={t:.2} first={}", self.gun(gun).name, first as u8);
                }
                SwitchEvent::Blocked(w) => {
                    if now - self.blocked_log_t > 1.0 {
                        self.blocked_log_t = now;
                        log_i!("SWITCH", "blocked why={}", w.label());
                    }
                }
                SwitchEvent::Waiting => log_i!("SWITCH", "wait gun={} why=burst", self.cur_gun().name),
            }
        }
        out
    }

    /// The names for the HUD switch list (previous, target, next of the target).
    pub fn switch_names(&self) -> [String; 3] {
        let en = self.enabled();
        let t = self.sw.target();
        let prev = mw2er_core::switch_logic::step_enabled(t, -1, &en).unwrap_or(t);
        let next = mw2er_core::switch_logic::step_enabled(t, 1, &en).unwrap_or(t);
        [prev, t, next].map(|i| self.gun(i).spec.display.clone())
    }

    /// Sound of a role of the gun in hand (`fire` uses the fire gain).
    pub fn play_role(&self, gun: usize, role: &str) {
        if let Some(&id) = self.gun(gun).roles.get(role) {
            shared::play_gun(id, role == "fire" || role == "last_shot");
        }
    }

    /// The notetrack sounds of the clip the viewmodel plays since the last tick (section 3.2: notes carry the reload,
    /// rechamber and raise foley).
    pub fn play_notes(&mut self, vm: &VmState, now: f64) {
        let gun = vm.gun as usize;
        let Some(a) = crate::viewmodel::gun_assets(gun) else { return };
        let clip = a.clip(vm.base.clip);
        let secs = ((now - vm.base.t0) as f32).max(0.0);
        let f = clip_frame(clip, secs, vm.base.rate);
        let from = if vm.seq == self.note_seq && vm.gun == self.note_gun { self.note_frame } else { None };
        if !clip.looped && vm.visible {
            for n in notes_between(clip, from, f) {
                if let Some(&id) = self.gun(gun).notes.get(n) {
                    shared::play_gun(id, false);
                }
            }
        }
        self.note_seq = vm.seq;
        self.note_gun = vm.gun;
        self.note_frame = Some(f);
    }

    /// View effects of this tick (kick + sway angles, the ADS zoom ratio) for the camera-step detour, and the sens scale.
    pub fn view_fx(&mut self, cfg: &Config, now: f64, dt: f32, ads: f32, scoped: bool, sprint_key: bool) -> (f32, f32, f32) {
        self.kick.tick(dt);
        self.gunkick.tick(dt);
        let spec = &self.data.guns[self.cur].spec;
        let overlay = spec.ads.overlay.is_some();
        if cfg.arsenal.hold_breath && overlay {
            match self.breath.tick(now, dt, sprint_key, scoped, &BreathParams::default()) {
                Some(BreathEvent::Hold) => log_i!("BREATH", "hold gun={}", self.data.guns[self.cur].name),
                Some(BreathEvent::Gasp { timeout }) => log_i!("BREATH", "gasp gun={} why={}", self.data.guns[self.cur].name, if timeout { "timeout" } else { "release" }),
                None => {}
            }
        } else {
            self.breath = Breath::default();
        }
        let (kp, ky) = if cfg.kick.enabled { self.kick.angles() } else { (0.0, 0.0) };
        let (sp, sy) = if cfg.kick.sway {
            sway_offset(now, spec.ads.idle_amount, spec.ads.idle_speed, cfg.arsenal.sway_deg_per_unit, ads, self.breath.scale)
        } else {
            (0.0, 0.0)
        };
        let ratio = if cfg.arsenal.ads_fov { zoom_ratio(spec.ads.zoom_fov, ads, MW2_HIP_FOV_DEG) } else { 1.0 };
        (kp + sp, ky + sy, ratio)
    }

    /// Stinger / Javelin lock this tick. `candidate` = the target in the reticle with LOS (Stinger), `aim` = the aim
    /// point (Javelin). Returns (state 0..2, target point for the HUD).
    pub fn lock_tick(&mut self, now: f64, ads_ok: bool, candidate: Option<(u64, V3)>, aim: Option<(V3, f32, Option<u64>)>) -> (u8, Option<V3>) {
        let lk = self.spec().projectile.map(|p| p.lock).unwrap_or(LockKind::None);
        let name = self.data.guns[self.cur].name.clone();
        let ev = match lk {
            LockKind::Required => self.lock.tick(now, ads_ok, candidate.map(|c| c.0), STINGER_LOCK_S, STINGER_GRACE_S),
            LockKind::RequiredTopAttack => self.plock.tick(now, ads_ok, aim),
            _ => return (0, None),
        };
        match ev {
            Some(LockEvent::Start(t)) => log_i!("LOCK", "start gun={name} target={}", if t == 0 { "point".to_string() } else { format!("chr#{t:x}") }),
            Some(LockEvent::Acquired(t)) => log_i!("LOCK", "acquired gun={name} target={} t={:.2}", if t == 0 { "point".to_string() } else { format!("chr#{t:x}") }, if lk == LockKind::Required { STINGER_LOCK_S } else { mw2er_core::projectile::JAVELIN_LOCK_S }),
            Some(LockEvent::Lost { sight }) => log_i!("LOCK", "lost gun={name} why={}", if sight { "sight" } else { "ads" }),
            None => {}
        }
        let st = match lk {
            LockKind::Required => self.lock.state,
            _ => self.plock.lock.state,
        };
        let point = match lk {
            LockKind::Required => candidate.map(|c| c.1),
            _ => self.plock.point,
        };
        match st {
            mw2er_core::projectile::LockState::Idle => (0, None),
            mw2er_core::projectile::LockState::Locking { .. } => (1, point),
            mw2er_core::projectile::LockState::Locked { .. } => (2, point),
        }
    }

    /// The launcher in hand needs a lock to fire and has none (`[launchers] lock = true`).
    pub fn lock_refuses(&self, cfg: &Config) -> bool {
        if !cfg.launchers.lock {
            return false;
        }
        match self.spec().projectile.map(|p| p.lock) {
            Some(LockKind::Required) => self.lock.locked().is_none(),
            Some(LockKind::RequiredTopAttack) => self.plock.lock.locked().is_none(),
            _ => false,
        }
    }

    pub fn reset_lock(&mut self) {
        self.lock.reset();
        self.plock.reset();
    }

    /// Review M6: drops every live projectile (Havok re-base / teleport / a new havok frame): their positions belong
    /// to the old frame and must never be ray-tested or exploded in the new one.
    pub fn expire_projectiles(&mut self, why: &str) {
        for pr in self.projs.drain(..) {
            log_i!("PROJ", "hit kind=expire gun={} id={} why={why}", self.data.guns[pr.gun].name, pr.id);
        }
        self.proj_w = None;
        self.reset_lock();
    }

    /// The launcher blast covering `pos` at `t_drop` (HIT attribution, like the AC-130's).
    pub fn attribute(&self, pos: V3, t_drop: f64, capsule_h: f32) -> Option<LBlast> {
        self.blasts
            .iter()
            .rev()
            .find(|b| (t_drop - b.t).abs() <= 0.6 && {
                let c = pos + V3::new(0.0, capsule_h * 0.5, 0.0);
                (c - b.pos).len() <= b.radius_m + capsule_h * 0.5
            })
            .copied()
    }
}
