//! MW2 timed sprint (docs/M5_DESIGN.md section 2.5), the rules of `PM_UpdateSprint` (iw4sp.exe 0x14007B400).
//!
//! Press mode (MW2): a press of the sprint key starts a sprint, releasing it keeps sprinting, a second press stops
//! it. The meter is 4 s x sprintDurationScale, it recharges 1:1 in real time (also while ER drives), a start needs
//! more than sprintMinTime (1 s) left. Fire, ADS, reload, jump, crouch, prone, forward <= 105 (S, strafe-only or W
//! released) and an empty meter end it; the gun then needs sprintOutTime before it can fire or aim.

/// Why a sprint ended (the `MOVE sprint end why=` label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndWhy {
    Time,
    Fire,
    Ads,
    Reload,
    Jump,
    Crouch,
    Prone,
    Forward,
    Toggle,
    Release,
    Handback,
    /// M8: a mantle started (MW2 PM_UpdateSprint ends the sprint on the mantle branch).
    Mantle,
}

impl EndWhy {
    pub fn label(self) -> &'static str {
        match self {
            EndWhy::Time => "time",
            EndWhy::Fire => "fire",
            EndWhy::Ads => "ads",
            EndWhy::Reload => "reload",
            EndWhy::Jump => "jump",
            EndWhy::Crouch => "crouch",
            EndWhy::Prone => "prone",
            EndWhy::Forward => "forward",
            EndWhy::Toggle => "toggle",
            EndWhy::Release => "release",
            EndWhy::Handback => "handback",
            EndWhy::Mantle => "mantle",
        }
    }

    pub const ALL: [EndWhy; 12] = [
        EndWhy::Time,
        EndWhy::Fire,
        EndWhy::Ads,
        EndWhy::Reload,
        EndWhy::Jump,
        EndWhy::Crouch,
        EndWhy::Prone,
        EndWhy::Forward,
        EndWhy::Toggle,
        EndWhy::Release,
        EndWhy::Handback,
        EndWhy::Mantle,
    ];
}

/// Why a start was refused (`MOVE sprint refused why=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefuseWhy {
    Meter,
    ButtonUp,
    Forward,
    Mask,
    Headroom,
}

impl RefuseWhy {
    pub fn label(self) -> &'static str {
        match self {
            RefuseWhy::Meter => "meter",
            RefuseWhy::ButtonUp => "button_up",
            RefuseWhy::Forward => "forward",
            RefuseWhy::Mask => "mask",
            RefuseWhy::Headroom => "headroom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SprintMode {
    Press,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SprintParams {
    pub time_s: f64,
    pub time_max_s: f64,
    pub min_time_s: f64,
    pub recharge_pause_s: f64,
    pub forward_min: f32,
    pub duration_scale: f64,
    pub out_s: f64,
    pub marathon: bool,
    pub mode: SprintMode,
}

impl SprintParams {
    /// The meter length: min(time x sprintDurationScale, 16.383 s).
    pub fn max_s(&self) -> f64 {
        (self.time_s * self.duration_scale).min(self.time_max_s)
    }
}

/// One update's inputs (the controller calls it once per substep).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SprintIn {
    /// New presses of the sprint key since the last update.
    pub presses: u32,
    /// The sprint key is held now.
    pub held: bool,
    pub forwardmove: f32,
    /// Buttons held (start mask) or pressed (cancel).
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    /// Jump / crouch / prone pressed this update.
    pub jump: bool,
    pub crouch: bool,
    pub prone: bool,
    /// Crouched or prone now (a start must stand up).
    pub low_stance: bool,
    /// A stand-up headroom ray passed (only read when `low_stance`).
    pub can_stand: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SprintEvent {
    Start { meter: f64, from_low_stance: bool },
    End { why: EndWhy, dur: f64, meter: f64 },
    Refused { why: RefuseWhy, meter: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprint {
    active: bool,
    start_t: f64,
    meter_at_start: f64,
    /// Meter left when the last sprint ended and when that was (None = never sprinted: full).
    last_end: Option<(f64, f64)>,
    exhausted: bool,
    need_button_up: bool,
    /// Fire and ADS stay blocked until this time (sprint_out).
    out_until: f64,
}

impl Default for Sprint {
    fn default() -> Self {
        Sprint {
            active: false,
            start_t: 0.0,
            meter_at_start: 0.0,
            last_end: None,
            exhausted: false,
            need_button_up: false,
            out_until: f64::NEG_INFINITY,
        }
    }
}

impl Sprint {
    pub fn active(&self) -> bool {
        self.active
    }

    /// Seconds of sprint left now (the meter).
    pub fn meter(&self, p: &SprintParams, now: f64) -> f64 {
        let max = p.max_s();
        if p.marathon {
            return max;
        }
        if self.active {
            return (self.meter_at_start - (now - self.start_t)).clamp(0.0, max);
        }
        match self.last_end {
            None => max,
            Some((left, t)) => {
                let pause = if self.exhausted { p.recharge_pause_s } else { 0.0 };
                (left + (now - t - pause).max(0.0)).clamp(0.0, max)
            }
        }
    }

    /// Fire and ADS are blocked while sprinting and for sprintOutTime after it ends.
    pub fn fire_blocked(&self, now: f64) -> bool {
        self.active || now < self.out_until
    }

    /// End of the sprint-out time (the viewmodel plays sprint_out until then).
    pub fn out_until(&self) -> f64 {
        self.out_until
    }

    fn end(&mut self, p: &SprintParams, now: f64, why: EndWhy, held: bool) -> SprintEvent {
        let meter = self.meter(p, now);
        let dur = now - self.start_t;
        self.active = false;
        self.exhausted = why == EndWhy::Time;
        self.last_end = Some((meter, now));
        self.out_until = now + p.out_s;
        if held {
            self.need_button_up = true;
        }
        SprintEvent::End { why, dur, meter }
    }

    /// Ends a running sprint from outside the update (handback). Returns the event if one was running.
    pub fn force_end(&mut self, p: &SprintParams, now: f64, why: EndWhy) -> Option<SprintEvent> {
        self.active.then(|| self.end(p, now, why, false))
    }

    /// One update. At most one event per call (an end, a start or a refusal).
    pub fn update(&mut self, p: &SprintParams, i: &SprintIn, now: f64) -> Option<SprintEvent> {
        if !i.held && i.presses == 0 {
            self.need_button_up = false;
        }
        let fwd_ok = i.forwardmove > p.forward_min;
        if self.active {
            let why = if !p.marathon && self.meter(p, now) <= 0.0 {
                Some(EndWhy::Time)
            } else if i.fire {
                Some(EndWhy::Fire)
            } else if i.ads {
                Some(EndWhy::Ads)
            } else if i.reload {
                Some(EndWhy::Reload)
            } else if i.jump {
                Some(EndWhy::Jump)
            } else if i.crouch {
                Some(EndWhy::Crouch)
            } else if i.prone {
                Some(EndWhy::Prone)
            } else if !fwd_ok {
                Some(EndWhy::Forward)
            } else if p.mode == SprintMode::Hold && !i.held {
                Some(EndWhy::Release)
            } else if p.mode == SprintMode::Press && i.presses > 0 {
                Some(EndWhy::Toggle)
            } else {
                None
            };
            return why.map(|w| {
                // a toggle press is still down: the next start needs a release first
                let held = i.held || w == EndWhy::Toggle;
                self.end(p, now, w, held)
            });
        }
        let wants = match p.mode {
            SprintMode::Press => i.presses > 0,
            SprintMode::Hold => i.held,
        };
        if !wants {
            return None;
        }
        if p.mode == SprintMode::Press && i.presses > 0 {
            // a new press is a release + press: the button-up requirement is met
            self.need_button_up = false;
        }
        let meter = self.meter(p, now);
        let refuse = if self.need_button_up {
            Some(RefuseWhy::ButtonUp)
        } else if !fwd_ok {
            Some(RefuseWhy::Forward)
        } else if i.fire || i.ads || i.reload || i.jump {
            Some(RefuseWhy::Mask)
        } else if meter <= p.min_time_s && !p.marathon {
            Some(RefuseWhy::Meter)
        } else if i.low_stance && !i.can_stand {
            Some(RefuseWhy::Headroom)
        } else {
            None
        };
        if let Some(why) = refuse {
            if p.mode == SprintMode::Hold {
                // hold mode: keep trying while held, but report only once per hold
                if self.need_button_up {
                    return None;
                }
                self.need_button_up = true;
            }
            return Some(SprintEvent::Refused { why, meter });
        }
        self.active = true;
        self.start_t = now;
        self.meter_at_start = meter;
        self.exhausted = false;
        Some(SprintEvent::Start { meter, from_low_stance: i.low_stance })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> SprintParams {
        SprintParams {
            time_s: 4.0,
            time_max_s: 16.383,
            min_time_s: 1.0,
            recharge_pause_s: 0.0,
            forward_min: 105.0,
            duration_scale: 1.0,
            out_s: 0.3,
            marathon: false,
            mode: SprintMode::Press,
        }
    }

    fn w() -> SprintIn {
        SprintIn { forwardmove: 127.0, can_stand: true, ..SprintIn::default() }
    }

    fn press() -> SprintIn {
        SprintIn { presses: 1, held: true, ..w() }
    }

    /// Runs W held at `hz` until `until`, returns the end event.
    fn run(s: &mut Sprint, p: &SprintParams, t: &mut f64, until: f64, hz: f64) -> Option<SprintEvent> {
        let mut ev = None;
        while *t < until - 1e-9 {
            *t += 1.0 / hz;
            if let Some(e) = s.update(p, &w(), *t) {
                ev = Some(e);
            }
        }
        ev
    }

    #[test]
    fn sprint_full_meter_lasts_4s_and_ends_time() {
        for hz in [30.0, 60.0, 144.0] {
            let p = p();
            let mut s = Sprint::default();
            let mut t = 0.0;
            assert!(matches!(s.update(&p, &press(), t), Some(SprintEvent::Start { .. })));
            let ev = run(&mut s, &p, &mut t, 5.0, hz);
            match ev {
                Some(SprintEvent::End { why: EndWhy::Time, dur, meter }) => {
                    assert!((dur - 4.0).abs() <= 1.0 / hz + 1e-9, "{hz}: {dur}");
                    assert_eq!(meter, 0.0);
                }
                e => panic!("{e:?}"),
            }
            assert!(s.exhausted);
        }
    }

    #[test]
    fn sprint_recharge_one_to_one_and_min_time() {
        let p = p();
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        // stop at 2 s (S pressed -> forward)
        let e = s.update(&p, &SprintIn { forwardmove: -127.0, ..w() }, 2.0);
        assert!(matches!(e, Some(SprintEvent::End { why: EndWhy::Forward, .. })));
        assert!((s.meter(&p, 2.0) - 2.0).abs() < 1e-9);
        assert!((s.meter(&p, 3.0) - 3.0).abs() < 1e-9, "1:1 recharge");
        assert!((s.meter(&p, 10.0) - 4.0).abs() < 1e-9, "clamped at max");
        // exhausted at 4 s: refused at 4.5 (0.5 s), accepted at 5.05 (1.05 s)
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        let mut t = 0.0;
        run(&mut s, &p, &mut t, 4.2, 60.0);
        assert!(!s.active());
        s.update(&p, &SprintIn::default(), 4.4);
        assert!(matches!(s.update(&p, &press(), 4.5), Some(SprintEvent::Refused { why: RefuseWhy::Meter, .. })));
        s.update(&p, &SprintIn::default(), 4.6);
        assert!(matches!(s.update(&p, &press(), 5.05), Some(SprintEvent::Start { .. })));
    }

    #[test]
    fn sprint_press_mode_release_keeps_second_press_stops() {
        let p = p();
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        // released: still sprinting
        assert_eq!(s.update(&p, &w(), 0.5), None);
        assert!(s.active());
        // second press: stops (toggle), and needs a release before a new start
        assert!(matches!(s.update(&p, &press(), 1.0), Some(SprintEvent::End { why: EndWhy::Toggle, .. })));
        assert!(!s.active());
        assert_eq!(s.update(&p, &SprintIn { held: true, ..w() }, 1.1), None, "held, no new press: nothing");
        assert!(s.need_button_up);
        s.update(&p, &w(), 1.2);
        assert!(!s.need_button_up, "released");
        assert!(matches!(s.update(&p, &press(), 2.5), Some(SprintEvent::Start { .. })));
    }

    #[test]
    fn sprint_hold_mode_release_stops() {
        let p = SprintParams { mode: SprintMode::Hold, ..p() };
        let mut s = Sprint::default();
        assert!(matches!(s.update(&p, &SprintIn { held: true, ..w() }, 0.0), Some(SprintEvent::Start { .. })));
        assert_eq!(s.update(&p, &SprintIn { held: true, ..w() }, 0.5), None);
        assert!(matches!(s.update(&p, &w(), 0.6), Some(SprintEvent::End { why: EndWhy::Release, .. })));
    }

    #[test]
    fn sprint_every_end_reason() {
        let p = p();
        let cases: [(EndWhy, SprintIn); 7] = [
            (EndWhy::Fire, SprintIn { fire: true, ..w() }),
            (EndWhy::Ads, SprintIn { ads: true, ..w() }),
            (EndWhy::Reload, SprintIn { reload: true, ..w() }),
            (EndWhy::Jump, SprintIn { jump: true, ..w() }),
            (EndWhy::Crouch, SprintIn { crouch: true, ..w() }),
            (EndWhy::Prone, SprintIn { prone: true, ..w() }),
            (EndWhy::Forward, SprintIn { forwardmove: 0.0, ..w() }),
        ];
        for (want, input) in cases {
            let mut s = Sprint::default();
            s.update(&p, &press(), 0.0);
            match s.update(&p, &input, 1.0) {
                Some(SprintEvent::End { why, .. }) => assert_eq!(why, want),
                e => panic!("{want:?}: {e:?}"),
            }
            assert!(s.fire_blocked(1.29), "sprint_out blocks fire");
            assert!(!s.fire_blocked(1.31));
        }
        // handback
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        assert!(matches!(s.force_end(&p, 1.0, EndWhy::Handback), Some(SprintEvent::End { why: EndWhy::Handback, .. })));
        assert_eq!(s.force_end(&p, 1.1, EndWhy::Handback), None);
        // strafe-only (W released) and S both end it: forwardmove <= 105
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        assert!(matches!(s.update(&p, &SprintIn { forwardmove: 105.0, ..w() }, 0.1), Some(SprintEvent::End { why: EndWhy::Forward, .. })));
        assert_eq!(EndWhy::ALL.len(), 12);
    }

    #[test]
    fn sprint_fire_held_blocks_start_and_forward_needed() {
        let p = p();
        let mut s = Sprint::default();
        assert!(matches!(s.update(&p, &SprintIn { fire: true, ..press() }, 0.0), Some(SprintEvent::Refused { why: RefuseWhy::Mask, .. })));
        assert!(matches!(s.update(&p, &SprintIn { forwardmove: 0.0, ..press() }, 0.1), Some(SprintEvent::Refused { why: RefuseWhy::Forward, .. })));
        assert!(!s.active());
    }

    #[test]
    fn sprint_from_crouch_needs_headroom() {
        let p = p();
        let mut s = Sprint::default();
        let e = s.update(&p, &SprintIn { low_stance: true, can_stand: false, ..press() }, 0.0);
        assert!(matches!(e, Some(SprintEvent::Refused { why: RefuseWhy::Headroom, .. })));
        let e = s.update(&p, &SprintIn { low_stance: true, can_stand: true, ..press() }, 0.1);
        assert!(matches!(e, Some(SprintEvent::Start { from_low_stance: true, .. })));
    }

    #[test]
    fn sprint_duration_scale_and_marathon() {
        let p2 = SprintParams { duration_scale: 2.0, ..p() };
        let mut s = Sprint::default();
        s.update(&p2, &press(), 0.0);
        let mut t = 0.0;
        match run(&mut s, &p2, &mut t, 9.0, 60.0) {
            Some(SprintEvent::End { why: EndWhy::Time, dur, .. }) => assert!((dur - 8.0).abs() < 0.02, "{dur}"),
            e => panic!("{e:?}"),
        }
        let pm = SprintParams { marathon: true, ..p() };
        let mut s = Sprint::default();
        s.update(&pm, &press(), 0.0);
        let mut t = 0.0;
        assert_eq!(run(&mut s, &pm, &mut t, 30.0, 60.0), None, "marathon never ends by time");
        assert!(s.active());
        let big = SprintParams { duration_scale: 10.0, ..p() };
        assert!((big.max_s() - 16.383).abs() < 1e-9, "capped at 16.383 s");
    }

    #[test]
    fn sprint_meter_recharges_while_suspended() {
        // the meter is time based: a handback in the middle changes nothing about the recharge
        let p = p();
        let mut s = Sprint::default();
        s.update(&p, &press(), 0.0);
        s.force_end(&p, 3.0, EndWhy::Handback);
        assert!((s.meter(&p, 3.0) - 1.0).abs() < 1e-9);
        assert!((s.meter(&p, 5.5) - 3.5).abs() < 1e-9);
    }
}
