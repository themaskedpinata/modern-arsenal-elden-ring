//! Viewmodel animation state machine (docs/M3_DESIGN.md section 3.2; M6 additions docs/M6_DESIGN.md 5.3).
//!
//! The game task calls [`VmAnim::step`] once per tick with the tick's inputs and the weapon simulation's events;
//! the resulting [`VmState`] (small, `Copy`) is published in the HUD snapshot and the present thread evaluates
//! the pose at its own clock ([`crate::vm_pose::evaluate`]), so animation is smooth at the present rate.
//!
//! M6: the state names the gun whose assets it poses (`gun`); weapon switching drops gun A (its putaway clip over
//! dropTime) and raises gun B (pullout, or first_raise the first time) on the switch machine's events; the rechamber
//! (bolt / pump / lever) and the three segments of a shell-by-shell reload are phases of their own; the last shot
//! and an empty magazine use their clips when the gun has them. With [`VmParams::from_gun_assets`] every timed clip
//! plays at `duration / weapon time` (M3 scaled only raise / drop / sprint).

use crate::vm_assets::{Clip, ClipId, N_CLIPS, VmAssets, WeaponTimes};

/// Which way the ADS fraction is moving (it also picks ads_up or ads_down for the layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdsDir {
    In,
    Out,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmPhase {
    Hidden,
    Raise,
    Idle,
    Fire,
    Reload,
    ReloadEmpty,
    SprintIn,
    SprintLoop,
    SprintOut,
    Drop,
    /// M6: bolt / pump / lever.
    Rechamber,
    /// M6: shell-by-shell reload segments.
    ReloadStart,
    ReloadLoop,
    ReloadEnd,
}

impl VmPhase {
    pub fn label(self) -> &'static str {
        match self {
            VmPhase::Hidden => "hidden",
            VmPhase::Raise => "raise",
            VmPhase::Idle => "idle",
            VmPhase::Fire => "fire",
            VmPhase::Reload => "reload",
            VmPhase::ReloadEmpty => "reload_empty",
            VmPhase::SprintIn => "sprint_in",
            VmPhase::SprintLoop => "sprint_loop",
            VmPhase::SprintOut => "sprint_out",
            VmPhase::Drop => "drop",
            VmPhase::Rechamber => "rechamber",
            VmPhase::ReloadStart => "reload_start",
            VmPhase::ReloadLoop => "reload_loop",
            VmPhase::ReloadEnd => "reload_end",
        }
    }

    fn reloading(self) -> bool {
        matches!(self, VmPhase::Reload | VmPhase::ReloadEmpty | VmPhase::ReloadStart | VmPhase::ReloadLoop | VmPhase::ReloadEnd)
    }

    fn sprinting(self) -> bool {
        matches!(self, VmPhase::SprintIn | VmPhase::SprintLoop | VmPhase::SprintOut)
    }
}

/// One clip playing since `t0` at `rate` (native seconds per real second).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipPlay {
    pub clip: ClipId,
    pub t0: f64,
    pub rate: f32,
}

/// Everything the present thread needs to pose the viewmodel at any `now` (published once per tick).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VmState {
    pub visible: bool,
    pub phase: VmPhase,
    pub base: ClipPlay,
    pub prev: Option<ClipPlay>,
    pub fade_t0: f64,
    pub fade_s: f32,
    /// ADS fraction at `ads_frac_t`, moving in `ads_dir` at `ads_rate_in` / `ads_rate_out` per second.
    pub ads_frac: f32,
    pub ads_frac_t: f64,
    pub ads_dir: AdsDir,
    pub ads_rate_in: f32,
    pub ads_rate_out: f32,
    /// Bumped on every phase change.
    pub seq: u32,
    /// M6: the arsenal slot whose assets `base` / `prev` refer to (0 for the M3 single gun).
    pub gun: u16,
    /// M6: the RPG's rocket part is hidden (magazine empty, before reloadShowRocketTime).
    pub hide_rocket: bool,
}

impl VmState {
    pub const fn hidden() -> Self {
        VmState {
            visible: false,
            phase: VmPhase::Hidden,
            base: ClipPlay { clip: ClipId::Idle, t0: 0.0, rate: 1.0 },
            prev: None,
            fade_t0: 0.0,
            fade_s: 0.0,
            ads_frac: 0.0,
            ads_frac_t: 0.0,
            ads_dir: AdsDir::Hold,
            ads_rate_in: 5.0,
            ads_rate_out: 5.0,
            seq: 0,
            gun: 0,
            hide_rocket: false,
        }
    }

    /// The ADS fraction extrapolated to `now` (the present thread runs ahead of the last tick).
    pub fn ads_at(&self, now: f64) -> f32 {
        let dt = ((now - self.ads_frac_t) as f32).max(0.0);
        let f = match self.ads_dir {
            AdsDir::In => self.ads_frac + dt * self.ads_rate_in,
            AdsDir::Out => self.ads_frac - dt * self.ads_rate_out,
            AdsDir::Hold => self.ads_frac,
        };
        if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.0 }
    }
}

/// Weapon-simulation events the state machine reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmEvent {
    Shot,
    ReloadStart { empty: bool },
    ReloadDone,
    ReloadCancelled,
    /// M6: the shot that emptied the magazine (the last-shot clip when the gun has one).
    LastShot,
    /// M6: rechamber began (`ads`: the ADS rechamber clip).
    Rechamber { ads: bool },
    /// M6: a shell-by-shell segment began.
    SegStart,
    SegLoop,
    SegEnd,
    /// M6: weapon switch: drop the gun in hand.
    SwitchDrop,
    /// M6: weapon switch: raise `gun` (its params must already be set).
    SwitchRaise { gun: u16, first: bool },
}

/// Plain per-tick inputs.
#[derive(Clone, Copy, Debug)]
pub struct VmInputs<'a> {
    pub now: f64,
    pub mode_on: bool,
    pub in_world: bool,
    pub menu_open: bool,
    pub ads_held: bool,
    pub planar_speed_mps: f32,
    pub events: &'a [VmEvent],
    /// M5: the MW2 controller's sprint flag while it drives (`None` = the M3 speed heuristic).
    pub sprinting: Option<bool>,
    /// M8: the gun is lowered (a mantle row >= 33 in, a ladder): the putaway clip over `quick_drop_s`, held down,
    /// then the pullout over `quick_raise_s`. Its own input: never the weapon-switch path.
    pub lowered: bool,
    pub quick_drop_s: f32,
    pub quick_raise_s: f32,
}

/// Clip durations and rates (rates = native duration / weapon-file time, docs/M3_DESIGN.md section 3.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VmParams {
    pub dur: [f32; N_CLIPS],
    pub rate: [f32; N_CLIPS],
    /// M6: the gun has its own clip for the role (else the fallback plays).
    pub native: [bool; N_CLIPS],
    pub crossfade_s: f32,
    /// 0 = no sprint animations.
    pub sprint_speed_mps: f32,
    pub ads_in_s: f32,
    pub ads_out_s: f32,
}

/// The speed must stay above the threshold this long before SprintIn starts.
pub const SPRINT_HOLD_S: f64 = 0.2;
/// Review M6 [I]: the fastest a fire / last-shot clip plays (UMP45 1.5x, ARs 1.8-2.5x unchanged; FAMAS 3.9x,
/// snipers, the Ranger capped).
pub const FIRE_MAX_RATE: f32 = 2.5;
/// No sprint within this long after a shot.
pub const SPRINT_AFTER_SHOT_S: f64 = 0.3;

impl VmParams {
    /// M3: the 12 base clips; raise / drop / sprint scaled to their weapon times, the others at their native rate.
    pub fn new(native_dur: [f32; 12], wt: &WeaponTimes, crossfade_s: f32, sprint_speed_mps: f32) -> Self {
        let mut dur = [0.0f32; N_CLIPS];
        dur[..12].copy_from_slice(&native_dur);
        let mut native = [false; N_CLIPS];
        native[..12].fill(true);
        Self::build(dur, native, wt, crossfade_s, sprint_speed_mps, false)
    }

    fn build(dur: [f32; N_CLIPS], native: [bool; N_CLIPS], wt: &WeaponTimes, crossfade_s: f32, sprint_speed_mps: f32, scale_all: bool) -> Self {
        let mut rate = [1.0f32; N_CLIPS];
        let mut scale = |id: ClipId, time: f32| {
            let d = dur[id.index()];
            if d > 0.0 && time > 0.0 && (d / time).is_finite() {
                rate[id.index()] = d / time;
            }
        };
        scale(ClipId::Pullout, wt.raise);
        scale(ClipId::Putaway, wt.drop);
        scale(ClipId::SprintIn, wt.sprint_in);
        scale(ClipId::SprintLoop, wt.sprint_loop);
        scale(ClipId::SprintOut, wt.sprint_out);
        if scale_all {
            // M6 (section 5.3): every timed clip plays over its weapon time (M3 reloads played at their native rate:
            // 2.500 / 3.033 s vs 2.5 / 3.03, a coincidence of the UMP45)
            scale(ClipId::Reload, wt.reload);
            scale(ClipId::ReloadEmpty, wt.reload_empty);
            scale(ClipId::FirstRaise, if wt.first_raise > 0.0 { wt.first_raise } else { wt.raise });
            scale(ClipId::Rechamber, wt.rechamber);
            scale(ClipId::AdsRechamber, wt.rechamber);
            scale(ClipId::ReloadStart, wt.reload_start);
            scale(ClipId::ReloadEnd, wt.reload_end);
            scale(ClipId::Fire, wt.fire);
            scale(ClipId::LastShot, wt.fire);
            scale(ClipId::FireAds, wt.fire);
            // review M6 [I]: a fire clip is never played faster than FIRE_MAX_RATE (the Ranger's 0.583 s kick would play
            // at 58x to fit fireTime 0.01, the snipers' at 4.7-9.2x: an invisible recoil). Whether IW4 stretches fire
            // anims to fireTime is not verified; the next shot restarts the clip either way (no crossfade).
            for id in [ClipId::Fire, ClipId::LastShot, ClipId::FireAds] {
                rate[id.index()] = rate[id.index()].min(FIRE_MAX_RATE);
            }
        }
        // a fallback role plays its fallback's clip at the fallback's rate
        for id in ClipId::EXTRA {
            if !native[id.index()] {
                rate[id.index()] = rate[id.fallback().index()];
            }
        }
        VmParams {
            dur,
            rate,
            native,
            crossfade_s,
            sprint_speed_mps,
            ads_in_s: wt.ads_in.max(0.01),
            ads_out_s: wt.ads_out.max(0.01),
        }
    }

    fn durations(a: &VmAssets) -> [f32; N_CLIPS] {
        let mut dur = [0.0f32; N_CLIPS];
        for id in ClipId::every() {
            dur[id.index()] = a.clip(id).duration();
        }
        dur
    }

    /// M3 rates (the single-gun path, vmtest and the M3 oracles).
    pub fn from_assets(a: &VmAssets, crossfade_s: f32, sprint_speed_mps: f32) -> Self {
        Self::build(Self::durations(a), a.native, &a.weapon_times, crossfade_s, sprint_speed_mps, false)
    }

    /// M6: one arsenal gun: every timed clip scaled to its weapon time.
    pub fn from_gun_assets(a: &VmAssets, crossfade_s: f32, sprint_speed_mps: f32) -> Self {
        let mut p = Self::build(Self::durations(a), a.native, &a.weapon_times, crossfade_s, sprint_speed_mps, true);
        // a looped ADS fire clip (the snipers' adsFireAnim is the idle loop) is not a timed clip
        if a.clip(ClipId::FireAds).looped {
            p.rate[ClipId::FireAds.index()] = 1.0;
        }
        p
    }

    /// Real seconds the clip takes at its rate.
    pub fn real_duration(&self, id: ClipId) -> f64 {
        (self.dur[id.index()] / self.rate[id.index()].max(1e-3)) as f64
    }
}

/// A phase change (for the `VM state` log line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: VmPhase,
    pub to: VmPhase,
}

#[derive(Clone, Debug)]
pub struct VmAnim {
    pub p: VmParams,
    st: VmState,
    sprint_since: Option<f64>,
    last_shot: f64,
    /// M6: a switch is in progress (the drop does not end in Hidden; it waits for the raise event).
    switching: bool,
    mag_empty: bool,
    /// M8: held down by `VmInputs::lowered`.
    lowered: bool,
}

impl VmAnim {
    pub fn new(p: VmParams) -> Self {
        let mut st = VmState::hidden();
        st.ads_rate_in = 1.0 / p.ads_in_s;
        st.ads_rate_out = 1.0 / p.ads_out_s;
        VmAnim { p, st, sprint_since: None, last_shot: f64::NEG_INFINITY, switching: false, mag_empty: false, lowered: false }
    }

    pub fn state(&self) -> VmState {
        self.st
    }

    pub fn set_params(&mut self, p: VmParams) {
        self.p = p;
        self.st.ads_rate_in = 1.0 / p.ads_in_s;
        self.st.ads_rate_out = 1.0 / p.ads_out_s;
    }

    /// M6: the gun whose assets the state refers to (set before its raise).
    pub fn set_gun(&mut self, gun: u16) {
        self.st.gun = gun;
    }

    /// M6: the magazine is empty (the empty idle clip, the RPG's rocket hidden).
    pub fn set_mag_empty(&mut self, empty: bool) {
        self.mag_empty = empty;
    }

    pub fn set_hide_rocket(&mut self, hide: bool) {
        self.st.hide_rocket = hide;
    }

    /// M6: a switch drop / raise is in progress.
    pub fn switching(&self) -> bool {
        self.switching
    }

    /// M6: the switch machine was reset mid-switch (world left, MODE toggled): no raise event will come, so the
    /// drop either finishes to Hidden or (mode on, in world) raises the gun in hand again on the next step.
    pub fn cancel_switch(&mut self) {
        self.switching = false;
    }

    fn native(&self, id: ClipId) -> bool {
        self.p.native[id.index()]
    }

    fn clip_for(&self, ph: VmPhase) -> ClipId {
        match ph {
            VmPhase::Hidden | VmPhase::Idle => {
                if self.mag_empty && self.native(ClipId::EmptyIdle) {
                    ClipId::EmptyIdle
                } else {
                    ClipId::Idle
                }
            }
            VmPhase::Raise => ClipId::Pullout,
            VmPhase::Fire => {
                if self.st.ads_frac >= 0.5 {
                    ClipId::FireAds
                } else {
                    ClipId::Fire
                }
            }
            VmPhase::Reload => ClipId::Reload,
            VmPhase::ReloadEmpty => ClipId::ReloadEmpty,
            VmPhase::SprintIn => ClipId::SprintIn,
            VmPhase::SprintLoop => ClipId::SprintLoop,
            VmPhase::SprintOut => ClipId::SprintOut,
            VmPhase::Drop => ClipId::Putaway,
            VmPhase::Rechamber => ClipId::Rechamber,
            VmPhase::ReloadStart => ClipId::ReloadStart,
            VmPhase::ReloadLoop => ClipId::Reload,
            VmPhase::ReloadEnd => ClipId::ReloadEnd,
        }
    }

    fn enter(&mut self, ph: VmPhase, now: f64, out: &mut Option<Transition>) {
        let clip = self.clip_for(ph);
        self.enter_clip(ph, clip, now, out);
    }

    fn enter_clip(&mut self, ph: VmPhase, clip: ClipId, now: f64, out: &mut Option<Transition>) {
        let from = self.st.phase;
        let play = ClipPlay { clip, t0: now, rate: self.p.rate[clip.index()] };
        // Fire (including a restart) starts without a crossfade, as in MW2; everything else fades.
        if ph == VmPhase::Fire || from == VmPhase::Hidden || self.p.crossfade_s <= 0.0 {
            self.st.prev = None;
        } else {
            self.st.prev = Some(self.st.base);
            self.st.fade_t0 = now;
            self.st.fade_s = self.p.crossfade_s;
        }
        self.st.base = play;
        self.st.phase = ph;
        self.st.seq = self.st.seq.wrapping_add(1);
        if out.is_none() || out.is_some_and(|t| t.to != ph) {
            *out = Some(Transition { from: out.map_or(from, |t| t.from), to: ph });
        }
    }

    fn clip_done(&self, now: f64) -> bool {
        let id = self.st.base.clip;
        if self.st.base.rate != self.p.rate[id.index()] {
            // M8: a quick drop / raise plays its clip at its own rate
            return now - self.st.base.t0 >= (self.p.dur[id.index()] / self.st.base.rate.max(1e-3)) as f64;
        }
        now - self.st.base.t0 >= self.p.real_duration(id)
    }

    /// M8: the rate that plays `clip` over `secs` (the clip's own rate if `secs` is not usable).
    fn quick_rate(&self, clip: ClipId, secs: f32) -> f32 {
        let d = self.p.dur[clip.index()];
        if d > 0.0 && secs > 0.01 && (d / secs).is_finite() { d / secs } else { self.p.rate[clip.index()] }
    }

    /// M8: the gun is held down (mantle / ladder).
    pub fn lowered(&self) -> bool {
        self.lowered
    }

    /// M6: starts the raise of the gun set with `set_gun` (no crossfade from the other gun's pose).
    fn raise_new_gun(&mut self, first: bool, now: f64, tr: &mut Option<Transition>) {
        self.switching = false;
        let clip = if first && self.native(ClipId::FirstRaise) { ClipId::FirstRaise } else { ClipId::Pullout };
        self.st.prev = None;
        let from = self.st.phase;
        self.st.phase = VmPhase::Hidden; // no crossfade across guns
        self.enter_clip(VmPhase::Raise, clip, now, tr);
        if let Some(t) = tr.as_mut() {
            t.from = from;
        }
    }

    /// Advances one task tick. Returns the phase change of this tick, if any.
    pub fn step(&mut self, i: &VmInputs<'_>) -> Option<Transition> {
        let now = i.now;
        let mut tr = None;

        // ADS fraction (moves toward the wish at 1/adsTransInTime and 1/adsTransOutTime per second).
        let cur = self.st.ads_at(now);
        let want = i.ads_held
            && i.mode_on
            && i.in_world
            && !i.menu_open
            && !self.switching
            && !self.st.phase.reloading()
            && !self.st.phase.sprinting()
            && !matches!(self.st.phase, VmPhase::Hidden | VmPhase::Drop | VmPhase::Raise);
        self.st.ads_frac = cur;
        self.st.ads_frac_t = now;
        self.st.ads_dir = if want && cur < 1.0 {
            AdsDir::In
        } else if !want && cur > 0.0 {
            AdsDir::Out
        } else {
            AdsDir::Hold
        };
        // M6: a switch drops ADS at once (section 5.2)
        if self.switching && cur > 0.0 {
            self.st.ads_frac = 0.0;
            self.st.ads_dir = AdsDir::Hold;
        }

        // M8: the lowered gun (mantle / ladder): a quick drop, held down, then a quick raise
        if i.lowered && !self.lowered && !self.switching && self.st.phase != VmPhase::Hidden {
            self.lowered = true;
            self.st.ads_frac = 0.0;
            self.st.ads_dir = AdsDir::Hold;
            self.enter_clip(VmPhase::Drop, ClipId::Putaway, now, &mut tr);
            self.st.base.rate = self.quick_rate(ClipId::Putaway, i.quick_drop_s);
        } else if !i.lowered && self.lowered {
            self.lowered = false;
            if i.mode_on && i.in_world && !self.switching {
                self.enter_clip(VmPhase::Raise, ClipId::Pullout, now, &mut tr);
                self.st.base.rate = self.quick_rate(ClipId::Pullout, i.quick_raise_s);
            }
        }
        if self.lowered {
            // held down: no auto-raise, hidden once the drop clip has played
            self.st.visible = self.st.phase == VmPhase::Drop && !self.clip_done(now) && i.in_world && !i.menu_open;
            return tr;
        }

        // Mode and world.
        match self.st.phase {
            VmPhase::Hidden => {
                if i.mode_on && i.in_world {
                    self.switching = false;
                    self.enter(VmPhase::Raise, now, &mut tr);
                }
            }
            VmPhase::Drop => {
                if i.mode_on && i.in_world && !self.switching {
                    self.enter(VmPhase::Raise, now, &mut tr);
                } else if (!i.mode_on || !i.in_world) && self.switching {
                    // MODE off / world left during a switch: the drop just continues to Hidden
                    self.switching = false;
                } else if !self.switching && self.clip_done(now) {
                    self.enter(VmPhase::Hidden, now, &mut tr);
                }
            }
            _ => {
                if !i.mode_on {
                    self.switching = false;
                    self.enter(VmPhase::Drop, now, &mut tr);
                }
            }
        }

        let active = !matches!(self.st.phase, VmPhase::Hidden) && (self.st.phase != VmPhase::Drop || self.switching);
        if active {
            // Events, in order (reload has priority over fire; weapon_sim blocks shots while reloading).
            for ev in i.events {
                match *ev {
                    VmEvent::SwitchDrop => {
                        self.switching = true;
                        self.st.ads_frac = 0.0;
                        self.st.ads_dir = AdsDir::Hold;
                        self.enter(VmPhase::Drop, now, &mut tr);
                    }
                    VmEvent::SwitchRaise { gun, first } => {
                        self.st.gun = gun;
                        self.raise_new_gun(first, now, &mut tr);
                    }
                    _ if self.switching => {}
                    VmEvent::ReloadStart { empty } => {
                        let ph = if empty { VmPhase::ReloadEmpty } else { VmPhase::Reload };
                        self.enter(ph, now, &mut tr);
                    }
                    VmEvent::SegStart => self.enter(VmPhase::ReloadStart, now, &mut tr),
                    VmEvent::SegLoop => self.enter(VmPhase::ReloadLoop, now, &mut tr),
                    VmEvent::SegEnd => self.enter(VmPhase::ReloadEnd, now, &mut tr),
                    VmEvent::ReloadDone | VmEvent::ReloadCancelled => {
                        if self.st.phase.reloading() || self.st.phase == VmPhase::Rechamber {
                            self.enter(VmPhase::Idle, now, &mut tr);
                        }
                    }
                    VmEvent::Shot | VmEvent::LastShot => {
                        self.last_shot = now;
                        if !self.st.phase.reloading() {
                            let last = *ev == VmEvent::LastShot && self.native(ClipId::LastShot) && self.st.ads_frac < 0.5;
                            if last {
                                self.enter_clip(VmPhase::Fire, ClipId::LastShot, now, &mut tr);
                            } else {
                                self.enter(VmPhase::Fire, now, &mut tr);
                            }
                        }
                    }
                    VmEvent::Rechamber { ads } => {
                        let clip = if ads && self.native(ClipId::AdsRechamber) { ClipId::AdsRechamber } else { ClipId::Rechamber };
                        self.enter_clip(VmPhase::Rechamber, clip, now, &mut tr);
                    }
                }
            }
            if self.switching {
                self.st.visible = i.in_world && !i.menu_open;
                return tr;
            }
            // Clip ends.
            if self.clip_done(now) {
                match self.st.phase {
                    VmPhase::Raise
                    | VmPhase::Fire
                    | VmPhase::Reload
                    | VmPhase::ReloadEmpty
                    | VmPhase::SprintOut
                    | VmPhase::Rechamber
                    | VmPhase::ReloadEnd => self.enter(VmPhase::Idle, now, &mut tr),
                    VmPhase::SprintIn => self.enter(VmPhase::SprintLoop, now, &mut tr),
                    // a shell segment holds its last frame until the sim starts the next one
                    _ => {}
                }
            }
            // the empty idle follows the magazine
            if self.st.phase == VmPhase::Idle {
                let want_clip = self.clip_for(VmPhase::Idle);
                if want_clip != self.st.base.clip {
                    self.enter(VmPhase::Idle, now, &mut tr);
                }
            }
            // M5: while the controller drives, the sprint clips follow its sprint flag (docs/M5_DESIGN.md section 8.1)
            if let Some(flag) = i.sprinting {
                self.sprint_since = None;
                match self.st.phase {
                    VmPhase::Idle | VmPhase::SprintOut if flag => self.enter(VmPhase::SprintIn, now, &mut tr),
                    VmPhase::SprintIn | VmPhase::SprintLoop if !flag => self.enter(VmPhase::SprintOut, now, &mut tr),
                    _ => {}
                }
                self.st.visible = self.st.phase != VmPhase::Hidden && i.in_world && !i.menu_open;
                return tr;
            }
            // Sprint.
            let fast = self.p.sprint_speed_mps > 0.0 && i.planar_speed_mps > self.p.sprint_speed_mps;
            let may_sprint = fast && !i.ads_held && cur <= 0.0 && now - self.last_shot >= SPRINT_AFTER_SHOT_S;
            match self.st.phase {
                VmPhase::Idle => {
                    if may_sprint {
                        let since = *self.sprint_since.get_or_insert(now);
                        if now - since >= SPRINT_HOLD_S {
                            self.enter(VmPhase::SprintIn, now, &mut tr);
                        }
                    } else {
                        self.sprint_since = None;
                    }
                }
                VmPhase::SprintIn | VmPhase::SprintLoop => {
                    if !fast || i.ads_held {
                        self.sprint_since = None;
                        self.enter(VmPhase::SprintOut, now, &mut tr);
                    }
                }
                _ => self.sprint_since = None,
            }
        }

        // A menu or leaving the world hides the viewmodel; the state continues underneath.
        self.st.visible = self.st.phase != VmPhase::Hidden && i.in_world && !i.menu_open;
        tr
    }
}

/// M6: the notetracks of `clip` between frames `from` (exclusive; `None` = from the clip start, inclusive) and `to`
/// (inclusive), in order. The task calls it with the frames of the playing clip at the previous and this tick.
pub fn notes_between(clip: &Clip, from: Option<f32>, to: f32) -> impl Iterator<Item = &str> {
    clip.notes.iter().filter(move |(_, f)| {
        let f = *f as f32;
        f <= to && from.is_none_or(|a| f > a)
    }).map(|(n, _)| n.as_str())
}

/// The present thread draws only when all of these hold (docs/M3_DESIGN.md section 4.5).
#[derive(Clone, Copy, Debug)]
pub struct DrawGate {
    pub renderer_ready: bool,
    pub vm_disabled: bool,
    pub cfg_enabled: bool,
    pub snapshot_age_s: f64,
    pub in_world: bool,
    pub menu_open: bool,
    pub mode_on: bool,
    pub visible: bool,
    /// M4: never drawn while an AC-130 ride runs (the thermal pass owns the frame).
    pub ac130_active: bool,
    /// Run 15: first person is On (a first-person gun over ER's third-person view looks broken).
    pub fp_on: bool,
    /// `[viewmodel] draw_in_third_person`: draw it anyway when first person is off.
    pub draw_in_tp: bool,
}

pub const SNAPSHOT_MAX_AGE_S: f64 = 0.5;

pub fn vm_should_draw(g: &DrawGate) -> bool {
    g.renderer_ready
        && !g.vm_disabled
        && g.cfg_enabled
        && g.snapshot_age_s >= 0.0
        && g.snapshot_age_s < SNAPSHOT_MAX_AGE_S
        && g.in_world
        && !g.menu_open
        && g.visible
        && !g.ac130_active
        && (g.fp_on || g.draw_in_tp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> VmParams {
        // native durations of the UMP45 clips
        let dur = [
            0.0333, 0.1333, 0.1667, 2.5, 3.0333, 0.8333, 0.7667, 0.3333, 1.0, 0.3333, 0.3667, 0.4,
        ];
        VmParams::new(dur, &WeaponTimes::default(), 0.05, 5.0)
    }

    struct Sim {
        a: VmAnim,
        t: f64,
    }

    impl Sim {
        fn new() -> Self {
            Sim { a: VmAnim::new(params()), t: 0.0 }
        }
        fn tick(&mut self, mode: bool, ads: bool, speed: f32, ev: &[VmEvent]) -> Option<Transition> {
            self.t += 1.0 / 60.0;
            self.a.step(&VmInputs {
                now: self.t,
                mode_on: mode,
                in_world: true,
                menu_open: false,
                ads_held: ads,
                planar_speed_mps: speed,
                events: ev,
                sprinting: None, lowered: false, quick_drop_s: 0.25, quick_raise_s: 0.55,
            })
        }
        fn run(&mut self, secs: f64, mode: bool, ads: bool, speed: f32) {
            let end = self.t + secs;
            while self.t < end {
                self.tick(mode, ads, speed, &[]);
            }
        }
        fn ph(&self) -> VmPhase {
            self.a.state().phase
        }
    }

    #[test]
    fn vm_anim_transition_table() {
        let mut s = Sim::new();
        assert_eq!(s.ph(), VmPhase::Hidden);
        assert!(!s.a.state().visible);
        let t = s.tick(true, false, 0.0, &[]).unwrap();
        assert_eq!((t.from, t.to), (VmPhase::Hidden, VmPhase::Raise));
        assert_eq!(s.a.state().base.clip, ClipId::Pullout);
        assert!((s.a.state().base.rate - 0.8333 / 0.76).abs() < 1e-4, "raise rate = native / raiseTime");
        s.run(0.8, true, false, 0.0);
        assert_eq!(s.ph(), VmPhase::Idle);
        s.tick(true, false, 0.0, &[VmEvent::Shot]);
        assert_eq!((s.ph(), s.a.state().base.clip), (VmPhase::Fire, ClipId::Fire));
        s.run(0.2, true, false, 0.0);
        assert_eq!(s.ph(), VmPhase::Idle, "fire returns to idle at the clip end");
        s.tick(true, false, 0.0, &[VmEvent::ReloadStart { empty: false }]);
        assert_eq!(s.ph(), VmPhase::Reload);
        s.tick(true, false, 0.0, &[VmEvent::ReloadDone]);
        assert_eq!(s.ph(), VmPhase::Idle);
        s.tick(true, false, 0.0, &[VmEvent::ReloadStart { empty: true }]);
        assert_eq!((s.ph(), s.a.state().base.clip), (VmPhase::ReloadEmpty, ClipId::ReloadEmpty));
        s.tick(true, false, 0.0, &[VmEvent::ReloadCancelled]);
        assert_eq!(s.ph(), VmPhase::Idle);
        s.tick(true, false, 0.0, &[VmEvent::ReloadStart { empty: false }]);
        s.run(2.6, true, false, 0.0);
        assert_eq!(s.ph(), VmPhase::Idle, "reload ends at its clip end without ReloadDone");
        // sprint in -> loop -> out
        s.run(0.1, true, false, 8.0);
        assert_eq!(s.ph(), VmPhase::Idle, "speed must hold for 0.2 s");
        s.run(0.15, true, false, 8.0);
        assert_eq!(s.ph(), VmPhase::SprintIn);
        s.run(0.35, true, false, 8.0);
        assert_eq!(s.ph(), VmPhase::SprintLoop);
        s.run(2.0, true, false, 8.0);
        assert_eq!(s.ph(), VmPhase::SprintLoop, "the loop keeps going");
        s.tick(true, false, 1.0, &[]);
        assert_eq!(s.ph(), VmPhase::SprintOut);
        s.run(0.4, true, false, 1.0);
        assert_eq!(s.ph(), VmPhase::Idle);
        // drop -> hidden
        s.tick(false, false, 0.0, &[]);
        assert_eq!(s.ph(), VmPhase::Drop);
        assert_eq!(s.a.state().base.clip, ClipId::Putaway);
        s.run(0.5, false, false, 0.0);
        assert!(s.a.state().visible, "putaway keeps visible until its end");
        s.run(0.4, false, false, 0.0);
        assert_eq!(s.ph(), VmPhase::Hidden);
        assert!(!s.a.state().visible);
    }

    #[test]
    fn vm_anim_fire_restart_keeps_fire() {
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        let mut seqs = Vec::new();
        for _ in 0..20 {
            s.tick(true, false, 0.0, &[VmEvent::Shot]);
            assert_eq!(s.ph(), VmPhase::Fire);
            assert!(s.a.state().prev.is_none(), "fire restarts without a crossfade");
            seqs.push(s.a.state().seq);
            s.run(0.09 - 1.0 / 60.0, true, false, 0.0);
            assert_eq!(s.ph(), VmPhase::Fire, "a shot every 0.09 s keeps state Fire");
        }
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "every restart bumps seq");
    }

    #[test]
    fn vm_anim_reload_during_sprint_and_shot_cuts_sprint() {
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        s.run(0.6, true, false, 8.0);
        assert!(matches!(s.ph(), VmPhase::SprintIn | VmPhase::SprintLoop));
        s.tick(true, false, 8.0, &[VmEvent::ReloadStart { empty: false }]);
        assert_eq!(s.ph(), VmPhase::Reload);
        s.run(3.0, true, false, 8.0);
        s.run(0.6, true, false, 8.0);
        assert!(matches!(s.ph(), VmPhase::SprintIn | VmPhase::SprintLoop));
        s.tick(true, false, 8.0, &[VmEvent::Shot]);
        assert_eq!(s.ph(), VmPhase::Fire, "a shot cuts straight to Fire");
        s.run(0.2, true, false, 8.0);
        assert_eq!(s.ph(), VmPhase::Idle, "no sprint within 0.3 s of a shot");
    }

    #[test]
    fn vm_anim_ads_fraction_rate_and_clamp() {
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        let t0 = s.t;
        s.tick(true, true, 0.0, &[]);
        let st = s.a.state();
        assert_eq!(st.ads_dir, AdsDir::In);
        assert!((st.ads_at(t0 + 0.1) - 0.5).abs() < 0.1);
        assert_eq!(st.ads_at(t0 + 5.0), 1.0, "clamped at 1");
        s.run(0.25, true, true, 0.0);
        assert_eq!(s.a.state().ads_at(s.t), 1.0, "reaches 1 within adsTransInTime (0.2 s)");
        s.tick(true, true, 0.0, &[VmEvent::Shot]);
        assert_eq!(s.a.state().base.clip, ClipId::FireAds, "fire_ads when ADS >= 0.5");
        s.run(0.25, true, false, 0.0);
        assert_eq!(s.a.state().ads_at(s.t), 0.0);
        // no ADS while reloading
        s.tick(true, true, 0.0, &[VmEvent::ReloadStart { empty: false }]);
        s.run(0.3, true, true, 0.0);
        assert_eq!(s.a.state().ads_at(s.t), 0.0);
    }

    #[test]
    fn vm_anim_menu_hides_and_state_continues() {
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        s.t += 1.0 / 60.0;
        s.a.step(&VmInputs {
            now: s.t,
            mode_on: true,
            in_world: true,
            menu_open: true,
            ads_held: false,
            planar_speed_mps: 0.0,
            events: &[VmEvent::ReloadStart { empty: false }],
            sprinting: None, lowered: false, quick_drop_s: 0.25, quick_raise_s: 0.55,
        });
        assert!(!s.a.state().visible);
        assert_eq!(s.ph(), VmPhase::Reload, "the state continues under the menu");
        s.tick(true, false, 0.0, &[]);
        assert!(s.a.state().visible);
    }

    #[test]
    fn vm_anim_state_is_small_and_deterministic() {
        assert!(std::mem::size_of::<VmState>() < 128, "{}", std::mem::size_of::<VmState>());
        let mut a = Sim::new();
        let mut b = Sim::new();
        for k in 0..300 {
            let ev: &[VmEvent] = if k % 7 == 0 { &[VmEvent::Shot] } else { &[] };
            a.tick(true, k % 50 > 25, (k % 90) as f32 / 10.0, ev);
            b.tick(true, k % 50 > 25, (k % 90) as f32 / 10.0, ev);
            assert_eq!(a.a.state(), b.a.state());
        }
    }

    #[test]
    fn vm_anim_draw_gate() {
        let ok = DrawGate {
            renderer_ready: true,
            vm_disabled: false,
            cfg_enabled: true,
            snapshot_age_s: 0.1,
            in_world: true,
            menu_open: false,
            mode_on: true,
            visible: true,
            ac130_active: false,
            fp_on: true,
            draw_in_tp: false,
        };
        assert!(vm_should_draw(&ok));
        // run 15: not drawn in third person unless configured
        assert!(!vm_should_draw(&DrawGate { fp_on: false, ..ok }));
        assert!(vm_should_draw(&DrawGate { fp_on: false, draw_in_tp: true, ..ok }));
        assert!(!vm_should_draw(&DrawGate { vm_disabled: true, ..ok }));
        assert!(!vm_should_draw(&DrawGate { renderer_ready: false, ..ok }));
        assert!(!vm_should_draw(&DrawGate { cfg_enabled: false, ..ok }));
        assert!(!vm_should_draw(&DrawGate { snapshot_age_s: 0.6, ..ok }));
        assert!(!vm_should_draw(&DrawGate { menu_open: true, ..ok }));
        assert!(!vm_should_draw(&DrawGate { in_world: false, ..ok }));
        assert!(!vm_should_draw(&DrawGate { visible: false, ..ok }));
        assert!(!vm_should_draw(&DrawGate { ac130_active: true, ..ok }), "M4: not drawn while the AC-130 is active");
    }

    fn flag_tick(s: &mut Sim, sprinting: bool, speed: f32, ev: &[VmEvent]) -> Option<Transition> {
        s.t += 1.0 / 60.0;
        s.a.step(&VmInputs {
            now: s.t,
            mode_on: true,
            in_world: true,
            menu_open: false,
            ads_held: false,
            planar_speed_mps: speed,
            events: ev,
            sprinting: Some(sprinting), lowered: false, quick_drop_s: 0.25, quick_raise_s: 0.55,
        })
    }

    #[test]
    fn vm_anim_sprint_flag_drives_in_loop_out() {
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        assert_eq!(s.ph(), VmPhase::Idle);
        flag_tick(&mut s, true, 7.2, &[]);
        assert_eq!(s.ph(), VmPhase::SprintIn, "the flag starts sprint_in at once (no 0.2 s speed hold)");
        for _ in 0..25 {
            flag_tick(&mut s, true, 7.2, &[]);
        }
        assert_eq!(s.ph(), VmPhase::SprintLoop, "sprint_in (0.3 s) -> loop");
        let t_end = s.t;
        flag_tick(&mut s, false, 7.2, &[]);
        assert_eq!(s.ph(), VmPhase::SprintOut, "the end of the sprint plays sprint_out");
        while s.ph() == VmPhase::SprintOut {
            flag_tick(&mut s, false, 7.2, &[]);
        }
        let out = s.t - t_end;
        assert!((out - 0.3).abs() < 0.05, "sprint_out lasts sprintOutTime: {out}");
        assert_eq!(s.ph(), VmPhase::Idle);
    }

    #[test]
    fn vm_anim_sprint_flag_overrides_speed_heuristic() {
        // fast but not sprinting (the controller's run): no sprint clip
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        for _ in 0..60 {
            flag_tick(&mut s, false, 9.0, &[]);
        }
        assert_eq!(s.ph(), VmPhase::Idle);
        // the heuristic still applies with None
        s.run(0.4, true, false, 9.0);
        assert!(matches!(s.ph(), VmPhase::SprintIn | VmPhase::SprintLoop));
    }

    #[test]
    fn vm_anim_sprint_flag_fire_after_out() {
        // fire pressed during the sprint: the controller ends the sprint (flag false), sprint_out plays, then the shot
        let mut s = Sim::new();
        s.tick(true, false, 0.0, &[]);
        s.run(1.0, true, false, 0.0);
        for _ in 0..40 {
            flag_tick(&mut s, true, 7.2, &[]);
        }
        flag_tick(&mut s, false, 7.2, &[]);
        assert_eq!(s.ph(), VmPhase::SprintOut);
        for _ in 0..19 {
            flag_tick(&mut s, false, 7.2, &[]);
        }
        flag_tick(&mut s, false, 7.2, &[VmEvent::Shot]);
        assert_eq!(s.ph(), VmPhase::Fire, "the shot after the out time plays fire");
    }

    // ---- M6 (docs/M6_DESIGN.md 5.3) ---------------------------------------------------------------------------

    /// UMP45-like params plus native M6 roles (made-up durations).
    fn m6_params() -> VmParams {
        let mut p = params();
        for (id, d) in [(ClipId::FirstRaise, 1.2f32), (ClipId::Rechamber, 0.9333), (ClipId::AdsRechamber, 0.9333), (ClipId::ReloadStart, 1.3333), (ClipId::ReloadEnd, 1.0), (ClipId::LastShot, 0.4)] {
            p.dur[id.index()] = d;
            p.native[id.index()] = true;
            p.rate[id.index()] = 1.0;
        }
        p
    }

    fn m6_tick(a: &mut VmAnim, t: f64, ads: bool, ev: &[VmEvent]) -> Option<Transition> {
        a.step(&VmInputs { now: t, mode_on: true, in_world: true, menu_open: false, ads_held: ads, planar_speed_mps: 0.0, events: ev, sprinting: Some(false), lowered: false, quick_drop_s: 0.25, quick_raise_s: 0.55 })
    }

    #[test]
    fn switching_vm_drop_waits_then_raises_other_gun() {
        let mut a = VmAnim::new(m6_params());
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut a, t, false, &[]);
            t += 1.0 / 60.0;
        }
        assert_eq!(a.state().phase, VmPhase::Idle);
        m6_tick(&mut a, t, false, &[VmEvent::SwitchDrop]);
        assert_eq!((a.state().phase, a.state().base.clip), (VmPhase::Drop, ClipId::Putaway));
        assert!(a.switching());
        // the putaway ends but the state waits for the raise (no Hidden)
        for _ in 0..120 {
            t += 1.0 / 60.0;
            m6_tick(&mut a, t, true, &[]);
        }
        assert_eq!(a.state().phase, VmPhase::Drop);
        assert!(a.state().visible);
        assert_eq!(a.state().ads_at(t), 0.0, "no ADS while switching");
        t += 1.0 / 60.0;
        m6_tick(&mut a, t, false, &[VmEvent::SwitchRaise { gun: 7, first: true }]);
        let s = a.state();
        assert_eq!((s.phase, s.base.clip, s.gun), (VmPhase::Raise, ClipId::FirstRaise, 7));
        assert!(s.prev.is_none(), "no crossfade from the other gun's pose");
        // not first: the pullout
        let mut b = VmAnim::new(params());
        m6_tick(&mut b, 0.0, false, &[VmEvent::SwitchDrop]);
        m6_tick(&mut b, 0.5, false, &[VmEvent::SwitchRaise { gun: 2, first: true }]);
        assert_eq!(b.state().base.clip, ClipId::Pullout, "no first_raise clip: pullout");
    }

    #[test]
    fn switching_vm_cancel_mid_switch_never_sticks() {
        // a reset of the switch machine mid-drop (MODE toggled / world left) must not leave the drop waiting forever
        let mut a = VmAnim::new(m6_params());
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut a, t, false, &[]);
            t += 1.0 / 60.0;
        }
        m6_tick(&mut a, t, false, &[VmEvent::SwitchDrop]);
        a.cancel_switch();
        t += 1.0 / 60.0;
        m6_tick(&mut a, t, false, &[]);
        assert!(!a.switching());
        assert_eq!(a.state().phase, VmPhase::Raise, "mode on in world: the gun in hand is raised again");
        // world left mid-switch without a cancel: the drop finishes to Hidden
        let mut b = VmAnim::new(m6_params());
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut b, t, false, &[]);
            t += 1.0 / 60.0;
        }
        m6_tick(&mut b, t, false, &[VmEvent::SwitchDrop]);
        for _ in 0..240 {
            t += 1.0 / 60.0;
            b.step(&VmInputs { now: t, mode_on: true, in_world: false, menu_open: false, ads_held: false, planar_speed_mps: 0.0, events: &[], sprinting: Some(false), lowered: false, quick_drop_s: 0.25, quick_raise_s: 0.55 });
        }
        assert_eq!(b.state().phase, VmPhase::Hidden);
        assert!(!b.switching());
    }

    #[test]
    fn switching_vm_retarget_back_to_from_never_sticks() {
        // review M6: drop of gun 3 retargeted back to 3 -> switch_logic sends RaiseStart{3}; ADS works afterwards
        let mut a = VmAnim::new(m6_params());
        a.set_gun(3);
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut a, t, false, &[]);
            t += 1.0 / 60.0;
        }
        m6_tick(&mut a, t, false, &[VmEvent::SwitchDrop]);
        t += 0.5;
        m6_tick(&mut a, t, false, &[VmEvent::SwitchRaise { gun: 3, first: false }]);
        assert!(!a.switching());
        assert_eq!((a.state().phase, a.state().base.clip, a.state().gun), (VmPhase::Raise, ClipId::Pullout, 3));
        for _ in 0..240 {
            t += 1.0 / 60.0;
            m6_tick(&mut a, t, true, &[]);
        }
        assert_eq!(a.state().phase, VmPhase::Idle);
        assert!(a.state().ads_at(t) > 0.99, "ADS works again after the retarget-back");
        m6_tick(&mut a, t + 0.01, true, &[VmEvent::Shot]);
        assert_eq!(a.state().phase, VmPhase::Fire, "shots are no longer swallowed");
    }

    #[test]
    fn rechamber_vm_clip_choice_and_end() {
        let mut a = VmAnim::new(m6_params());
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut a, t, false, &[]);
            t += 1.0 / 60.0;
        }
        m6_tick(&mut a, t, false, &[VmEvent::Rechamber { ads: true }]);
        assert_eq!((a.state().phase, a.state().base.clip), (VmPhase::Rechamber, ClipId::AdsRechamber));
        let mut pb = m6_params();
        pb.native[ClipId::AdsRechamber.index()] = false;
        let mut b = VmAnim::new(pb);
        m6_tick(&mut b, 0.0, false, &[]);
        m6_tick(&mut b, 1.0, false, &[VmEvent::Rechamber { ads: true }]);
        assert_eq!(b.state().base.clip, ClipId::Rechamber, "no ADS rechamber clip: the plain one");
        while t < 3.0 {
            t += 1.0 / 60.0;
            m6_tick(&mut a, t, false, &[]);
        }
        assert_eq!(a.state().phase, VmPhase::Idle);
    }

    #[test]
    fn shell_reload_vm_segments_and_last_shot() {
        let mut a = VmAnim::new(m6_params());
        let mut t = 0.0;
        while t < 1.5 {
            m6_tick(&mut a, t, false, &[]);
            t += 1.0 / 60.0;
        }
        m6_tick(&mut a, t, true, &[VmEvent::ReloadStart { empty: true }, VmEvent::SegStart]);
        assert_eq!((a.state().phase, a.state().base.clip), (VmPhase::ReloadStart, ClipId::ReloadStart));
        m6_tick(&mut a, t + 1.4, true, &[VmEvent::SegLoop]);
        assert_eq!((a.state().phase, a.state().base.clip), (VmPhase::ReloadLoop, ClipId::Reload));
        m6_tick(&mut a, t + 4.0, true, &[]);
        assert_eq!(a.state().phase, VmPhase::ReloadLoop, "a segment holds until the sim moves on");
        assert_eq!(a.state().ads_at(t + 4.0), 0.0, "no ADS while reloading");
        m6_tick(&mut a, t + 4.1, false, &[VmEvent::SegEnd]);
        assert_eq!(a.state().base.clip, ClipId::ReloadEnd);
        m6_tick(&mut a, t + 5.2, false, &[VmEvent::ReloadDone]);
        assert_eq!(a.state().phase, VmPhase::Idle);
        m6_tick(&mut a, t + 6.0, false, &[VmEvent::LastShot]);
        assert_eq!(a.state().base.clip, ClipId::LastShot);
    }

    #[test]
    fn vm_params_rates_scale_every_timed_clip() {
        let mut wt = WeaponTimes::default();
        wt.rechamber = 0.467;
        let mut dur = [0.0f32; N_CLIPS];
        dur[ClipId::Reload.index()] = 0.8667;
        dur[ClipId::Rechamber.index()] = 0.9333;
        dur[ClipId::Pullout.index()] = 0.8333;
        let mut native = [true; N_CLIPS];
        native[ClipId::LastShot.index()] = false;
        let p = VmParams::build(dur, native, &WeaponTimes { reload: 0.667, ..wt }, 0.05, 5.0, true);
        assert!((p.rate[ClipId::Reload.index()] - 0.8667 / 0.667).abs() < 1e-4);
        assert!((p.rate[ClipId::Rechamber.index()] - 0.9333 / 0.467).abs() < 1e-3, "rechamber 2.0x");
        assert!((p.real_duration(ClipId::Rechamber) - 0.467).abs() < 1e-4);
        assert_eq!(p.rate[ClipId::Idle.index()], 1.0, "a 0-duration clip keeps rate 1 (never divided by 0)");
        assert_eq!(p.rate[ClipId::LastShot.index()], p.rate[ClipId::Fire.index()], "fallback plays at its fallback's rate");
        let m3 = VmParams::build(dur, native, &WeaponTimes { reload: 0.667, ..wt }, 0.05, 5.0, false);
        assert_eq!(m3.rate[ClipId::Reload.index()], 1.0, "M3: reloads at their native rate");
    }

    #[test]
    fn vm_params_rates_fire_clip_capped() {
        // review M6 [I]: the Ranger's 0.583 s fire clip at fireTime 0.01 plays at 2.5x (0.233 s), not 58x; the UMP45's
        // 0.133 s at 0.09 keeps its 1.48x
        let mut dur = [0.0f32; N_CLIPS];
        dur[ClipId::Fire.index()] = 0.583;
        dur[ClipId::FireAds.index()] = 0.583;
        let native = [true; N_CLIPS];
        let p = VmParams::build(dur, native, &WeaponTimes { fire: 0.01, ..WeaponTimes::default() }, 0.05, 5.0, true);
        assert_eq!((p.rate[ClipId::Fire.index()], p.rate[ClipId::FireAds.index()]), (FIRE_MAX_RATE, FIRE_MAX_RATE));
        assert!((p.real_duration(ClipId::Fire) - 0.2332).abs() < 1e-3);
        dur[ClipId::Fire.index()] = 0.133;
        let u = VmParams::build(dur, native, &WeaponTimes { fire: 0.09, ..WeaponTimes::default() }, 0.05, 5.0, true);
        assert!((u.rate[ClipId::Fire.index()] - 0.133 / 0.09).abs() < 1e-4);
    }

    #[test]
    fn vm_notes_between_frames() {
        let c = Clip { name: "r".into(), fps: 30, numframes: 30, looped: false, notes: vec![("a".into(), 0), ("b".into(), 10), ("c".into(), 20)], tracks: vec![] };
        assert_eq!(notes_between(&c, None, 0.0).collect::<Vec<_>>(), ["a"]);
        assert_eq!(notes_between(&c, Some(0.0), 10.0).collect::<Vec<_>>(), ["b"]);
        assert_eq!(notes_between(&c, Some(10.0), 19.9).count(), 0);
        assert_eq!(notes_between(&c, Some(10.0), 30.0).collect::<Vec<_>>(), ["c"]);
    }

    #[test]
    fn mantle_weapon_vm_lowered_quick_drop_and_raise() {
        let mut a = VmAnim::new(params());
        let step = |a: &mut VmAnim, t: f64, lowered: bool, ev: &[VmEvent]| {
            a.step(&VmInputs { now: t, mode_on: true, in_world: true, menu_open: false, ads_held: true, planar_speed_mps: 0.0, events: ev, sprinting: Some(false), lowered, quick_drop_s: 0.25, quick_raise_s: 0.55 })
        };
        let mut t = 0.0;
        while t < 1.5 {
            t += 1.0 / 60.0;
            step(&mut a, t, false, &[]);
        }
        assert_eq!(a.state().phase, VmPhase::Idle);
        assert!(a.state().ads_at(t) > 0.5, "aimed before the mantle");
        // lowered: the putaway clip at the quick-drop rate, ADS dropped at once, held down (no auto raise)
        t += 1.0 / 60.0;
        step(&mut a, t, true, &[]);
        let s = a.state();
        assert_eq!((s.phase, s.base.clip), (VmPhase::Drop, ClipId::Putaway));
        assert!((s.base.rate - 0.7667 / 0.25).abs() < 1e-3, "quick drop rate {}", s.base.rate);
        assert_eq!(s.ads_at(t), 0.0);
        assert!(!a.switching(), "never the weapon-switch path");
        for _ in 0..60 {
            t += 1.0 / 60.0;
            step(&mut a, t, true, &[VmEvent::Shot, VmEvent::ReloadStart { empty: false }]);
        }
        assert_eq!(a.state().phase, VmPhase::Drop, "held down, events ignored");
        assert!(!a.state().visible, "gone once the drop played");
        assert!(a.lowered());
        // released: the pullout at the quick-raise rate, then idle
        t += 1.0 / 60.0;
        step(&mut a, t, false, &[]);
        let s = a.state();
        assert_eq!((s.phase, s.base.clip), (VmPhase::Raise, ClipId::Pullout));
        assert!((s.base.rate - 0.8333 / 0.55).abs() < 1e-3, "quick raise rate {}", s.base.rate);
        for _ in 0..40 {
            t += 1.0 / 60.0;
            step(&mut a, t, false, &[]);
        }
        assert_eq!(a.state().phase, VmPhase::Idle);
    }
}
