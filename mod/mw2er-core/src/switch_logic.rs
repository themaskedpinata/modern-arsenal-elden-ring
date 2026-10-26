//! M6 weapon switching (docs/M6_DESIGN.md section 5.2). Pure: the DLL feeds requests (keys 1/2, the wheel) and the
//! per-gun drop / raise times and acts on the events (drop / raise anims, ammo slot, movement class).
//!
//! States: `Ready(g) -> Dropping{from, to} -> Raising{to} -> Ready(to)`.
//! - A request in Ready starts a drop unless the AC-130 is active, MW2 mode is off, only one gun is enabled, or a burst
//!   is in flight (then it waits until the burst ends).
//! - A request while Dropping retargets (no extra anim). A request while Raising to another gun drops the raising gun,
//!   starting at the mirrored progress (`t_drop = (1 - raise_frac) * drop_s`) [I].
//! - The first raise of a gun in the session uses `firstRaiseTime` [I MW2 semantics].
//! - Death / world leave / MODE off: `reset` to Ready(current) with no anim.

/// Where the request came from (logged as `via=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Wheel,
    Key1,
    Key2,
}

impl Via {
    pub fn label(self) -> &'static str {
        match self {
            Via::Wheel => "wheel",
            Via::Key1 => "key1",
            Via::Key2 => "key2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwitchState {
    Ready { gun: usize },
    Dropping { from: usize, to: usize, t0: f64, dur: f64, via: Via },
    Raising { to: usize, t0: f64, dur: f64, first: bool },
}

/// Why a request did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    Ac130,
    ModeOff,
    Disabled,
    OnlyOne,
}

impl Blocked {
    pub fn label(self) -> &'static str {
        match self {
            Blocked::Ac130 => "ac130",
            Blocked::ModeOff => "mode_off",
            Blocked::Disabled => "disabled",
            Blocked::OnlyOne => "only_one",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwitchEvent {
    /// The drop of `from` began (play its drop clip over `drop_s`).
    Start { from: usize, to: usize, via: Via, drop_s: f64 },
    /// A request while dropping changed the target.
    Retarget { to: usize },
    /// The raise of `to` began (bind its slot; movement class switches now).
    RaiseStart { gun: usize, first: bool, raise_s: f64 },
    /// `to` is ready to fire.
    Raised { gun: usize, t: f64, first: bool },
    /// A raise was turned back into a drop (request to another gun while raising).
    Reversed { from: usize, to: usize, drop_s: f64 },
    Blocked(Blocked),
    /// A request waits for the burst in flight.
    Waiting,
}

/// Per-gun times (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunTimes {
    pub drop_s: f64,
    pub raise_s: f64,
    pub first_raise_s: f64,
}

/// One tick's inputs.
#[derive(Clone, Copy, Debug)]
pub struct SwitchIn<'a> {
    pub now: f64,
    pub enabled: &'a [bool],
    pub times: &'a [GunTimes],
    pub ac130_active: bool,
    pub mode_on: bool,
    /// A burst is in flight on the current gun: a request waits for it.
    pub busy: bool,
    /// Wheel / key steps this tick (+1 next, -1 previous; several notches add up), with the source.
    pub request: Option<(i32, Via)>,
}

#[derive(Clone, Debug)]
pub struct SwitchMachine {
    st: SwitchState,
    raised_once: Vec<bool>,
    waiting: Option<(i32, Via)>,
}

/// The `steps`-th enabled gun after (or before, for negative steps) `from`, wrapping; `None` when no other gun is
/// enabled.
pub fn step_enabled(from: usize, steps: i32, enabled: &[bool]) -> Option<usize> {
    let n = enabled.len();
    if n == 0 || steps == 0 {
        return None;
    }
    let mut cur = from.min(n - 1);
    let mut left = steps.unsigned_abs();
    let fwd = steps > 0;
    let mut guard = 0;
    while left > 0 {
        guard += 1;
        if guard > n * (steps.unsigned_abs() as usize + 1) + n {
            return None;
        }
        cur = if fwd { (cur + 1) % n } else { (cur + n - 1) % n };
        if enabled[cur] {
            left -= 1;
        }
    }
    (cur != from).then_some(cur)
}

impl SwitchMachine {
    pub fn new(start: usize, guns: usize) -> SwitchMachine {
        let mut raised_once = vec![false; guns];
        if let Some(r) = raised_once.get_mut(start) {
            // the gun in hand at MODE on raises through vm_anim's own pullout
            *r = true;
        }
        SwitchMachine { st: SwitchState::Ready { gun: start }, raised_once, waiting: None }
    }

    pub fn state(&self) -> SwitchState {
        self.st
    }

    /// The gun whose ammo / sim / spread is live: the dropping gun until the raise of the next one starts.
    pub fn current(&self) -> usize {
        match self.st {
            SwitchState::Ready { gun } => gun,
            SwitchState::Dropping { from, .. } => from,
            SwitchState::Raising { to, .. } => to,
        }
    }

    /// The gun that will be in hand when the switch is over.
    pub fn target(&self) -> usize {
        match self.st {
            SwitchState::Ready { gun } => gun,
            SwitchState::Dropping { to, .. } | SwitchState::Raising { to, .. } => to,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.st, SwitchState::Ready { .. })
    }

    /// 0..1 progress of the drop / raise in progress (1 in Ready).
    pub fn progress(&self, now: f64) -> f64 {
        match self.st {
            SwitchState::Ready { .. } => 1.0,
            SwitchState::Dropping { t0, dur, .. } | SwitchState::Raising { t0, dur, .. } => {
                if dur <= 0.0 { 1.0 } else { ((now - t0) / dur).clamp(0.0, 1.0) }
            }
        }
    }

    /// Death / world leave / MODE off: back to Ready on the target gun... no: on the gun currently live, no anim.
    pub fn reset(&mut self) {
        self.st = SwitchState::Ready { gun: self.current() };
        self.waiting = None;
    }

    /// Selects `gun` directly (config start gun / a disabled current gun), no anim.
    pub fn force(&mut self, gun: usize) {
        self.st = SwitchState::Ready { gun };
        if let Some(r) = self.raised_once.get_mut(gun) {
            *r = true;
        }
        self.waiting = None;
    }

    fn times(i: &SwitchIn<'_>, g: usize) -> GunTimes {
        i.times.get(g).copied().unwrap_or(GunTimes { drop_s: 0.5, raise_s: 0.5, first_raise_s: 0.5 })
    }

    fn begin_raise(&mut self, to: usize, t0: f64, i: &SwitchIn<'_>, out: &mut Vec<SwitchEvent>) {
        let first = !self.raised_once.get(to).copied().unwrap_or(true);
        let t = Self::times(i, to);
        let dur = if first { t.first_raise_s } else { t.raise_s }.max(0.0);
        self.st = SwitchState::Raising { to, t0, dur, first };
        out.push(SwitchEvent::RaiseStart { gun: to, first, raise_s: dur });
    }

    /// One tick.
    pub fn tick(&mut self, i: &SwitchIn<'_>, out: &mut Vec<SwitchEvent>) {
        let now = i.now;
        let mut req = i.request.or(self.waiting.take());
        if req.is_some() {
            let why = if i.ac130_active {
                Some(Blocked::Ac130)
            } else if !i.mode_on {
                Some(Blocked::ModeOff)
            } else if i.enabled.iter().filter(|e| **e).count() < 2 {
                Some(Blocked::OnlyOne)
            } else {
                None
            };
            if let Some(w) = why {
                out.push(SwitchEvent::Blocked(w));
                req = None;
            }
        }
        if let Some((steps, via)) = req {
            match self.st {
                SwitchState::Ready { gun } => {
                    if i.busy {
                        self.waiting = Some((steps, via));
                        out.push(SwitchEvent::Waiting);
                    } else if let Some(to) = step_enabled(gun, steps, i.enabled) {
                        let d = Self::times(i, gun).drop_s.max(0.0);
                        self.st = SwitchState::Dropping { from: gun, to, t0: now, dur: d, via };
                        out.push(SwitchEvent::Start { from: gun, to, via, drop_s: d });
                    } else {
                        out.push(SwitchEvent::Blocked(Blocked::Disabled));
                    }
                }
                SwitchState::Dropping { from, to, t0, dur, via: v0 } => {
                    if let Some(nt) = step_enabled(to, steps, i.enabled) {
                        // dropping back to the gun being dropped is a retarget too (it raises again)
                        self.st = SwitchState::Dropping { from, to: nt, t0, dur, via: v0 };
                        out.push(SwitchEvent::Retarget { to: nt });
                    }
                }
                SwitchState::Raising { to, t0, dur, .. } => {
                    if let Some(nt) = step_enabled(to, steps, i.enabled) {
                        let frac = if dur > 0.0 { ((now - t0) / dur).clamp(0.0, 1.0) } else { 1.0 };
                        let d = Self::times(i, to).drop_s.max(0.0);
                        let t_drop = (1.0 - frac) * d;
                        self.st = SwitchState::Dropping { from: to, to: nt, t0: now - t_drop, dur: d, via };
                        out.push(SwitchEvent::Reversed { from: to, to: nt, drop_s: d - t_drop });
                    }
                }
            }
        }
        // time
        for _ in 0..4 {
            match self.st {
                SwitchState::Dropping { to, t0, dur, .. } if now >= t0 + dur => {
                    // to == from (retargeted back to the gun being dropped): it raises again. The RaiseStart event is
                    // what ends the viewmodel's switch (vm_anim stays in Drop while `switching`), so it is always sent.
                    self.begin_raise(to, t0 + dur, i, out);
                }
                SwitchState::Raising { to, t0, dur, first } if now >= t0 + dur => {
                    if let Some(r) = self.raised_once.get_mut(to) {
                        *r = true;
                    }
                    self.st = SwitchState::Ready { gun: to };
                    out.push(SwitchEvent::Raised { gun: to, t: now - t0, first });
                }
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: GunTimes = GunTimes { drop_s: 0.5, raise_s: 0.6, first_raise_s: 0.9 };

    fn inp<'a>(now: f64, en: &'a [bool], times: &'a [GunTimes], req: Option<(i32, Via)>) -> SwitchIn<'a> {
        SwitchIn { now, enabled: en, times, ac130_active: false, mode_on: true, busy: false, request: req }
    }

    #[test]
    fn switching_step_enabled_wraps_and_skips() {
        let en = [true, false, true, true];
        assert_eq!(step_enabled(0, 1, &en), Some(2));
        assert_eq!(step_enabled(3, 1, &en), Some(0));
        assert_eq!(step_enabled(0, -1, &en), Some(3));
        assert_eq!(step_enabled(0, 2, &en), Some(3));
        assert_eq!(step_enabled(0, 3, &en), None, "a full lap lands on the same gun");
        assert_eq!(step_enabled(0, 1, &[true, false]), None, "only one gun");
    }

    #[test]
    fn switching_full_cycle_drop_then_raise() {
        let en = [true; 3];
        let times = [T; 3];
        let mut m = SwitchMachine::new(0, 3);
        let mut ev = Vec::new();
        m.tick(&inp(0.0, &en, &times, Some((1, Via::Key1))), &mut ev);
        assert_eq!(ev, vec![SwitchEvent::Start { from: 0, to: 1, via: Via::Key1, drop_s: 0.5 }]);
        assert_eq!(m.current(), 0, "gun A stays live while it drops");
        ev.clear();
        m.tick(&inp(0.49, &en, &times, None), &mut ev);
        assert!(ev.is_empty());
        m.tick(&inp(0.5, &en, &times, None), &mut ev);
        assert_eq!(ev, vec![SwitchEvent::RaiseStart { gun: 1, first: true, raise_s: 0.9 }], "first raise of gun 1");
        assert_eq!(m.current(), 1, "B is live from the raise start");
        ev.clear();
        m.tick(&inp(1.41, &en, &times, None), &mut ev);
        assert!(matches!(ev[..], [SwitchEvent::Raised { gun: 1, first: true, .. }]));
        assert!(m.is_ready());
        // back to 0 (raised before: normal raise time)
        ev.clear();
        m.tick(&inp(2.0, &en, &times, Some((-1, Via::Key2))), &mut ev);
        m.tick(&inp(2.5, &en, &times, None), &mut ev);
        assert!(ev.contains(&SwitchEvent::RaiseStart { gun: 0, first: false, raise_s: 0.6 }));
    }

    #[test]
    fn switching_retarget_and_reverse() {
        let en = [true; 4];
        let times = [T; 4];
        let mut m = SwitchMachine::new(0, 4);
        let mut ev = Vec::new();
        m.tick(&inp(0.0, &en, &times, Some((1, Via::Wheel))), &mut ev);
        m.tick(&inp(0.2, &en, &times, Some((1, Via::Wheel))), &mut ev);
        assert!(ev.contains(&SwitchEvent::Retarget { to: 2 }));
        assert_eq!(m.target(), 2);
        m.tick(&inp(0.5, &en, &times, None), &mut ev);
        assert!(matches!(m.state(), SwitchState::Raising { to: 2, .. }));
        // request while raising (25 % in): reversed into a drop of gun 2 with 75 % of its drop left
        ev.clear();
        m.tick(&inp(0.5 + 0.225, &en, &times, Some((1, Via::Wheel))), &mut ev);
        match ev[..] {
            [SwitchEvent::Reversed { from: 2, to: 3, drop_s }] => assert!((drop_s - 0.125).abs() < 1e-9, "{drop_s}"),
            _ => panic!("{ev:?}"),
        }
        ev.clear();
        m.tick(&inp(0.85, &en, &times, None), &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SwitchEvent::RaiseStart { gun: 3, .. })));
    }

    #[test]
    fn switching_blocks_wheel_multi_notch_and_busy() {
        let en = [true; 5];
        let times = [T; 5];
        let mut m = SwitchMachine::new(0, 5);
        let mut ev = Vec::new();
        // AC-130 active / mode off / only one gun
        m.tick(&SwitchIn { ac130_active: true, ..inp(0.0, &en, &times, Some((1, Via::Key1))) }, &mut ev);
        m.tick(&SwitchIn { mode_on: false, ..inp(0.0, &en, &times, Some((1, Via::Key1))) }, &mut ev);
        m.tick(&inp(0.0, &[true, false, false, false, false], &times, Some((1, Via::Key1))), &mut ev);
        assert_eq!(
            ev,
            vec![SwitchEvent::Blocked(Blocked::Ac130), SwitchEvent::Blocked(Blocked::ModeOff), SwitchEvent::Blocked(Blocked::OnlyOne)]
        );
        assert!(m.is_ready());
        // three notches in one tick: one switch, target +3
        ev.clear();
        m.tick(&inp(1.0, &en, &times, Some((3, Via::Wheel))), &mut ev);
        assert_eq!(ev, vec![SwitchEvent::Start { from: 0, to: 3, via: Via::Wheel, drop_s: 0.5 }]);
        // a burst in flight: the request waits, then goes
        let mut m = SwitchMachine::new(0, 5);
        ev.clear();
        m.tick(&SwitchIn { busy: true, ..inp(0.0, &en, &times, Some((1, Via::Key1))) }, &mut ev);
        assert_eq!(ev, vec![SwitchEvent::Waiting]);
        ev.clear();
        m.tick(&inp(0.1, &en, &times, None), &mut ev);
        assert!(matches!(ev[..], [SwitchEvent::Start { to: 1, .. }]));
        // reset (death / MODE off): Ready on the live gun, no anim
        m.reset();
        assert_eq!(m.state(), SwitchState::Ready { gun: 0 });
    }

    #[test]
    fn switching_retarget_back_to_from_raises_again() {
        // review M6: 1 then 2 during a drop targets the gun being dropped; the drop end must still send a raise
        let en = [true; 3];
        let times = [T; 3];
        let mut m = SwitchMachine::new(0, 3);
        let mut ev = Vec::new();
        m.tick(&inp(0.0, &en, &times, Some((1, Via::Key1))), &mut ev);
        m.tick(&inp(0.1, &en, &times, Some((-1, Via::Key2))), &mut ev);
        assert!(ev.contains(&SwitchEvent::Retarget { to: 0 }));
        assert_eq!(m.target(), 0);
        ev.clear();
        m.tick(&inp(0.5, &en, &times, None), &mut ev);
        assert_eq!(ev, vec![SwitchEvent::RaiseStart { gun: 0, first: false, raise_s: 0.6 }]);
        ev.clear();
        m.tick(&inp(1.2, &en, &times, None), &mut ev);
        assert!(matches!(ev[..], [SwitchEvent::Raised { gun: 0, first: false, .. }]));
        assert_eq!(m.state(), SwitchState::Ready { gun: 0 });
    }

    #[test]
    fn switching_random_sequences_every_drop_ends_in_a_raise() {
        // every Start / Reversed is followed by a RaiseStart before the machine is Ready again
        let en = [true, true, false, true];
        let times = [T; 4];
        let mut m = SwitchMachine::new(0, 4);
        let mut rng = crate::rng::Rng::new(29);
        let mut t = 0.0;
        let mut dropping = false;
        for _ in 0..2000 {
            t += 0.02 + rng.next_f32() as f64 * 0.2;
            let req = if rng.next_f32() < 0.35 { Some((if rng.next_f32() < 0.5 { 1 } else { -1 }, Via::Key1)) } else { None };
            let mut ev = Vec::new();
            m.tick(&inp(t, &en, &times, req), &mut ev);
            for e in &ev {
                match e {
                    SwitchEvent::Start { .. } | SwitchEvent::Reversed { .. } => dropping = true,
                    SwitchEvent::RaiseStart { .. } => dropping = false,
                    _ => {}
                }
            }
            if m.is_ready() {
                assert!(!dropping, "Ready without a raise after a drop at t={t}");
            }
        }
    }

    #[test]
    fn switching_disabled_guns_skipped_in_random_sequences() {
        let en = [true, false, true, true, false, true];
        let times = [T; 6];
        let mut m = SwitchMachine::new(0, 6);
        let mut rng = crate::rng::Rng::new(11);
        let mut t = 0.0;
        for _ in 0..500 {
            t += 0.05 + rng.next_f32() as f64 * 0.3;
            let req = if rng.next_f32() < 0.4 { Some((if rng.next_f32() < 0.5 { 1 } else { -1 }, Via::Wheel)) } else { None };
            let mut ev = Vec::new();
            m.tick(&inp(t, &en, &times, req), &mut ev);
            assert!(en[m.current()] && en[m.target()], "a disabled gun is never selected");
        }
    }
}
