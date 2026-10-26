//! MW2 stances (docs/M5_DESIGN.md section 3): stand / crouch / prone with the exact eye-height curves of
//! `PM_ViewHeightAdjust` (iw4sp.exe tables 0x1403BD470/4E0/550/5E0), transition times 200 / 400 ms, stand<->prone
//! always through crouch, the MW2 PC key table, headroom checks for raising, and the speed / accel per stance.

use crate::movement_data::{CURVE_CROUCH_PRONE, CURVE_CROUCH_STAND, CURVE_PRONE_CROUCH, CURVE_STAND_CROUCH, MoveData, Mw2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StanceKind {
    Stand,
    Crouch,
    Prone,
}

impl StanceKind {
    pub fn label(self) -> &'static str {
        match self {
            StanceKind::Stand => "stand",
            StanceKind::Crouch => "crouch",
            StanceKind::Prone => "prone",
        }
    }

    /// The MW2 eye height (inches) of the settled stance.
    pub fn eye(self, m: &Mw2) -> f32 {
        match self {
            StanceKind::Stand => m.eye_stand,
            StanceKind::Crouch => m.eye_crouch,
            StanceKind::Prone => m.eye_prone,
        }
    }

    pub fn hull(self, m: &Mw2) -> f32 {
        match self {
            StanceKind::Stand => m.hull_stand,
            StanceKind::Crouch => m.hull_crouch,
            StanceKind::Prone => m.hull_prone,
        }
    }

    pub fn speed_scale(self, m: &Mw2) -> f32 {
        match self {
            StanceKind::Stand => 1.0,
            StanceKind::Crouch => m.crouch_speed_scale,
            StanceKind::Prone => m.prone_speed_scale,
        }
    }

    pub fn accel(self, m: &Mw2) -> f32 {
        match self {
            StanceKind::Stand => m.accel_stand,
            StanceKind::Crouch => m.accel_crouch,
            StanceKind::Prone => m.accel_prone,
        }
    }
}

/// Piecewise-linear MW2 curve (percent, inches) at a fraction 0..1.
pub fn curve_at(c: &[(f32, f32)], frac: f32) -> f32 {
    let pct = (frac * 100.0).clamp(0.0, 100.0);
    for w in c.windows(2) {
        let (a, b) = (w[0], w[1]);
        if pct <= b.0 {
            let t = if b.0 > a.0 { (pct - a.0) / (b.0 - a.0) } else { 1.0 };
            return a.1 + (b.1 - a.1) * t;
        }
    }
    c.last().map_or(0.0, |p| p.1)
}

/// One leg of a transition (always between neighbours: stand<->crouch or crouch<->prone).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    pub from: StanceKind,
    pub to: StanceKind,
    pub t0: f64,
    pub dur: f64,
}

/// What a key or a rule asked for (`MOVE stance` / `MOVE stance blocked` lines).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StanceEvent {
    /// A leg started.
    Leg { from: StanceKind, to: StanceKind, dur_ms: f64 },
    /// A raise was refused for lack of headroom.
    Blocked { to: StanceKind },
}

/// Key presses of one update (MW2 PC binds: C togglecrouch, LCtrl toggleprone, Space +gostand).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StanceKeys {
    pub crouch_press: bool,
    pub prone_press: bool,
    /// Space while crouched / prone (while standing it is a jump, handled by the controller).
    pub gostand_press: bool,
    /// `crouch_mode = "hold"`: C held now.
    pub crouch_held: Option<bool>,
    /// `stance_hold` key: tap released (< 300 ms) -> crouch toggle; held 300 ms -> prone.
    pub hold_tap: bool,
    pub hold_long: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stance {
    /// The settled stance, or the `from` of the running leg.
    cur: StanceKind,
    leg: Option<Leg>,
    /// Where the player wants to end up (stand->prone chains through crouch).
    goal: StanceKind,
    pub crouch_ms: f64,
    pub prone_ms: f64,
    /// `gostand_from_prone`: stand (default) or crouch.
    pub gostand_from_prone: StanceKind,
}

impl Stance {
    pub fn new(m: &Mw2) -> Stance {
        Stance {
            cur: StanceKind::Stand,
            leg: None,
            goal: StanceKind::Stand,
            crouch_ms: m.stand_crouch_ms as f64,
            prone_ms: m.crouch_prone_ms as f64,
            gostand_from_prone: StanceKind::Stand,
        }
    }

    /// The stance the player is in or heading to (key decisions use it).
    pub fn goal(&self) -> StanceKind {
        self.goal
    }

    /// The settled stance (None while a leg runs).
    pub fn settled(&self) -> Option<StanceKind> {
        self.leg.is_none().then_some(self.cur)
    }

    pub fn leg(&self) -> Option<Leg> {
        self.leg
    }

    pub fn in_transition(&self) -> bool {
        self.leg.is_some()
    }

    /// Resets to stand at once (handbacks other than `menu`).
    pub fn reset(&mut self) {
        self.cur = StanceKind::Stand;
        self.goal = StanceKind::Stand;
        self.leg = None;
    }

    fn leg_dur(&self, a: StanceKind, b: StanceKind) -> f64 {
        if a.max(b) == StanceKind::Prone { self.prone_ms } else { self.crouch_ms }
    }

    /// The next neighbour on the way from `a` to `b`.
    fn step_toward(a: StanceKind, b: StanceKind) -> StanceKind {
        match (a, b) {
            (StanceKind::Stand, _) => StanceKind::Crouch,
            (StanceKind::Prone, _) => StanceKind::Crouch,
            (StanceKind::Crouch, x) => x,
        }
    }

    /// Advances legs finished by `now` (and starts the next leg of a chain).
    pub fn advance(&mut self, now: f64, out: &mut Vec<StanceEvent>) {
        while let Some(l) = self.leg {
            if now - l.t0 + 1e-7 < l.dur / 1000.0 {
                break;
            }
            self.cur = l.to;
            self.leg = None;
            if self.goal != self.cur {
                let next = Self::step_toward(self.cur, self.goal);
                let dur = self.leg_dur(self.cur, next);
                self.leg = Some(Leg { from: self.cur, to: next, t0: l.t0 + l.dur / 1000.0, dur });
                out.push(StanceEvent::Leg { from: self.cur, to: next, dur_ms: dur });
            }
        }
    }

    /// Asks for `goal`. Raising needs `headroom(target)`; going prone needs `grounded` and no running leg.
    pub fn request(
        &mut self,
        goal: StanceKind,
        now: f64,
        grounded: bool,
        headroom: &mut dyn FnMut(StanceKind) -> bool,
        out: &mut Vec<StanceEvent>,
    ) -> bool {
        if goal == self.goal {
            return false;
        }
        let raising = goal < self.goal;
        if raising && !headroom(goal) {
            out.push(StanceEvent::Blocked { to: goal });
            return false;
        }
        if goal == StanceKind::Prone && (!grounded || self.leg.is_some()) {
            return false;
        }
        self.goal = goal;
        match self.leg {
            None => {
                let next = Self::step_toward(self.cur, goal);
                let dur = self.leg_dur(self.cur, next);
                self.leg = Some(Leg { from: self.cur, to: next, t0: now, dur });
                out.push(StanceEvent::Leg { from: self.cur, to: next, dur_ms: dur });
            }
            Some(l) => {
                // A goal back toward the leg's start reverses the running leg from the mirrored point; a goal
                // further along just extends the chain (advance() starts the next leg).
                let reverse = (l.to > l.from && goal <= l.from) || (l.to < l.from && goal >= l.from);
                if reverse {
                    let done = ((now - l.t0) * 1000.0 / l.dur).clamp(0.0, 1.0);
                    let dur = l.dur;
                    let t0 = now - (1.0 - done) * dur / 1000.0;
                    self.cur = l.to;
                    self.leg = Some(Leg { from: l.to, to: l.from, t0, dur });
                    out.push(StanceEvent::Leg { from: l.to, to: l.from, dur_ms: dur });
                }
            }
        }
        true
    }

    /// One update with the MW2 PC key table (section 3). Returns true when Space was used as +gostand (no jump).
    pub fn keys(
        &mut self,
        k: &StanceKeys,
        now: f64,
        grounded: bool,
        headroom: &mut dyn FnMut(StanceKind) -> bool,
        out: &mut Vec<StanceEvent>,
    ) -> bool {
        self.advance(now, out);
        let g = self.goal;
        if k.crouch_press && k.crouch_held.is_none() {
            let to = match g {
                StanceKind::Stand => StanceKind::Crouch,
                StanceKind::Crouch => StanceKind::Stand,
                StanceKind::Prone => StanceKind::Crouch,
            };
            self.request(to, now, grounded, headroom, out);
        }
        if let Some(held) = k.crouch_held {
            if held && g == StanceKind::Stand {
                self.request(StanceKind::Crouch, now, grounded, headroom, out);
            } else if !held && g == StanceKind::Crouch {
                self.request(StanceKind::Stand, now, grounded, headroom, out);
            }
        }
        if k.prone_press {
            let to = if self.goal == StanceKind::Prone { StanceKind::Stand } else { StanceKind::Prone };
            self.request(to, now, grounded, headroom, out);
        }
        if k.hold_tap {
            let to = if self.goal == StanceKind::Crouch { StanceKind::Stand } else { StanceKind::Crouch };
            self.request(to, now, grounded, headroom, out);
        }
        if k.hold_long {
            self.request(StanceKind::Prone, now, grounded, headroom, out);
        }
        let mut gostand = false;
        if k.gostand_press && self.goal != StanceKind::Stand {
            gostand = true;
            let to = if self.goal == StanceKind::Prone { self.gostand_from_prone } else { StanceKind::Stand };
            self.request(to, now, grounded, headroom, out);
        }
        gostand
    }

    /// Fraction 0..1 of the running leg.
    pub fn frac(&self, now: f64) -> f32 {
        self.leg.map_or(1.0, |l| (((now - l.t0) * 1000.0 / l.dur) as f32).clamp(0.0, 1.0))
    }

    /// Eye height in MW2 inches (unscaled) from the exact curves.
    pub fn eye_in(&self, d: &MoveData, now: f64) -> f32 {
        match self.leg {
            None => self.cur.eye(&d.mw2),
            Some(l) => {
                let c = match (l.from, l.to) {
                    (StanceKind::Stand, StanceKind::Crouch) => CURVE_STAND_CROUCH,
                    (StanceKind::Crouch, StanceKind::Stand) => CURVE_CROUCH_STAND,
                    (StanceKind::Crouch, StanceKind::Prone) => CURVE_CROUCH_PRONE,
                    _ => CURVE_PRONE_CROUCH,
                };
                curve_at(&d.curves[c], self.frac(now))
            }
        }
    }

    /// Speed scale: stand<->crouch switches at the start of the leg [I]; crouch<->prone blends linearly (V).
    pub fn speed_scale(&self, m: &Mw2, now: f64) -> f32 {
        match self.leg {
            None => self.cur.speed_scale(m),
            Some(l) if l.from.max(l.to) == StanceKind::Prone => {
                let f = self.frac(now);
                l.from.speed_scale(m) + (l.to.speed_scale(m) - l.from.speed_scale(m)) * f
            }
            Some(l) => l.to.speed_scale(m),
        }
    }

    /// The stance whose accel, step height and hull apply now (the leg's target).
    pub fn effective(&self) -> StanceKind {
        self.leg.map_or(self.cur, |l| l.to)
    }

    /// The hull height (inches) used for headroom/ceiling: the larger of the leg's ends while it runs.
    pub fn hull_in(&self, m: &Mw2) -> f32 {
        match self.leg {
            None => self.cur.hull(m),
            Some(l) => l.from.hull(m).max(l.to.hull(m)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement_data::test_data;

    fn yes(_: StanceKind) -> bool {
        true
    }

    #[test]
    fn stance_curve_known_answers() {
        let d = test_data();
        assert!((curve_at(&d.curves[CURVE_STAND_CROUCH], 0.5) - 51.2).abs() < 1e-4);
        assert!((curve_at(&d.curves[CURVE_CROUCH_PRONE], 0.5) - 15.0).abs() < 1e-4);
        for (c, a, b) in [(CURVE_STAND_CROUCH, 60.0, 40.0), (CURVE_CROUCH_STAND, 40.0, 60.0), (CURVE_CROUCH_PRONE, 40.0, 11.0), (CURVE_PRONE_CROUCH, 11.0, 40.0)] {
            assert_eq!(curve_at(&d.curves[c], 0.0), a);
            assert_eq!(curve_at(&d.curves[c], 1.0), b);
        }
        // the eye through a running leg
        let mut s = Stance::new(&d.mw2);
        let mut ev = Vec::new();
        s.request(StanceKind::Crouch, 0.0, true, &mut yes, &mut ev);
        assert!((s.eye_in(&d, 0.1) - 51.2).abs() < 1e-3);
    }

    #[test]
    fn stance_durations_and_chain() {
        let d = test_data();
        let mut s = Stance::new(&d.mw2);
        let mut ev = Vec::new();
        s.request(StanceKind::Crouch, 0.0, true, &mut yes, &mut ev);
        assert_eq!(ev, vec![StanceEvent::Leg { from: StanceKind::Stand, to: StanceKind::Crouch, dur_ms: 200.0 }]);
        s.advance(0.1999, &mut ev);
        assert_eq!(s.settled(), None);
        s.advance(0.2, &mut ev);
        assert_eq!(s.settled(), Some(StanceKind::Crouch));
        // crouch -> prone 400 ms, prone -> crouch 400 ms, crouch -> stand 200 ms
        ev.clear();
        s.request(StanceKind::Prone, 1.0, true, &mut yes, &mut ev);
        s.advance(1.399, &mut ev);
        assert!(s.in_transition());
        s.advance(1.4, &mut ev);
        assert_eq!(s.settled(), Some(StanceKind::Prone));
        // stand <-> prone through crouch: 600 ms
        let mut s = Stance::new(&d.mw2);
        let mut ev = Vec::new();
        s.request(StanceKind::Prone, 0.0, true, &mut yes, &mut ev);
        s.advance(0.3, &mut ev);
        assert_eq!(s.leg().map(|l| (l.from, l.to)), Some((StanceKind::Crouch, StanceKind::Prone)));
        s.advance(0.5999, &mut ev);
        assert!(s.in_transition());
        s.advance(0.6, &mut ev);
        assert_eq!(s.settled(), Some(StanceKind::Prone));
        assert_eq!(ev.len(), 2);
        ev.clear();
        s.request(StanceKind::Stand, 1.0, true, &mut yes, &mut ev);
        s.advance(1.6, &mut ev);
        assert_eq!(s.settled(), Some(StanceKind::Stand), "prone -> stand 600 ms");
    }

    #[test]
    fn stance_key_table_every_cell() {
        let d = test_data();
        let at = |st: StanceKind| {
            let mut s = Stance::new(&d.mw2);
            let mut ev = Vec::new();
            s.request(st, 0.0, true, &mut yes, &mut ev);
            s.advance(10.0, &mut ev);
            s
        };
        let press = |st: StanceKind, k: StanceKeys| -> (StanceKind, bool) {
            let mut s = at(st);
            let mut ev = Vec::new();
            let g = s.keys(&k, 20.0, true, &mut yes, &mut ev);
            (s.goal(), g)
        };
        let c = StanceKeys { crouch_press: true, ..Default::default() };
        let p = StanceKeys { prone_press: true, ..Default::default() };
        let sp = StanceKeys { gostand_press: true, ..Default::default() };
        use StanceKind::*;
        assert_eq!(press(Stand, c).0, Crouch);
        assert_eq!(press(Crouch, c).0, Stand);
        assert_eq!(press(Prone, c).0, Crouch);
        assert_eq!(press(Stand, p).0, Prone);
        assert_eq!(press(Crouch, p).0, Prone);
        assert_eq!(press(Prone, p).0, Stand);
        assert_eq!(press(Stand, sp), (Stand, false), "space while standing is a jump (controller)");
        assert_eq!(press(Crouch, sp), (Stand, true));
        assert_eq!(press(Prone, sp), (Stand, true));
        // gostand_from_prone = crouch
        let mut s = at(Prone);
        s.gostand_from_prone = Crouch;
        let mut ev = Vec::new();
        s.keys(&sp, 20.0, true, &mut yes, &mut ev);
        assert_eq!(s.goal(), Crouch);
        // crouch_mode = hold
        let mut s = at(Stand);
        s.keys(&StanceKeys { crouch_held: Some(true), ..Default::default() }, 20.0, true, &mut yes, &mut ev);
        assert_eq!(s.goal(), Crouch);
        s.keys(&StanceKeys { crouch_held: Some(false), ..Default::default() }, 21.0, true, &mut yes, &mut ev);
        assert_eq!(s.goal(), Stand);
        // stance_hold key: tap = crouch, long = prone
        let mut s = at(Stand);
        s.keys(&StanceKeys { hold_tap: true, ..Default::default() }, 20.0, true, &mut yes, &mut ev);
        assert_eq!(s.goal(), Crouch);
        let mut s = at(Stand);
        s.keys(&StanceKeys { hold_long: true, ..Default::default() }, 20.0, true, &mut yes, &mut ev);
        assert_eq!(s.goal(), Prone);
    }

    #[test]
    fn stance_headroom_blocks_raise_and_prone_needs_ground() {
        let d = test_data();
        let mut s = Stance::new(&d.mw2);
        let mut ev = Vec::new();
        s.request(StanceKind::Crouch, 0.0, true, &mut yes, &mut ev);
        s.advance(1.0, &mut ev);
        ev.clear();
        let mut no = |_: StanceKind| false;
        assert!(!s.request(StanceKind::Stand, 2.0, true, &mut no, &mut ev));
        assert_eq!(ev, vec![StanceEvent::Blocked { to: StanceKind::Stand }]);
        assert_eq!(s.goal(), StanceKind::Crouch);
        // lowering never needs headroom
        assert!(s.request(StanceKind::Prone, 2.0, true, &mut no, &mut ev));
        // prone in the air is refused
        let mut s = Stance::new(&d.mw2);
        assert!(!s.request(StanceKind::Prone, 0.0, false, &mut yes, &mut ev));
    }

    #[test]
    fn stance_eye_ratio_vs_mw2() {
        let d = test_data();
        // ratio mode: eye_m = eye_in / 60 * 1.60
        let ratio = |e: f32| e / 60.0 * 1.60;
        assert!((ratio(d.mw2.eye_crouch) - 1.0667).abs() < 1e-3);
        assert!((ratio(d.mw2.eye_prone) - 0.2933).abs() < 1e-3);
        assert!((d.mw2.eye_crouch * 0.0254 - 1.016).abs() < 1e-4);
        assert!((d.mw2.eye_prone * 0.0254 - 0.2794).abs() < 1e-4);
        assert!((d.mw2.hull_stand / 60.0 * 1.6 - 1.8667).abs() < 1e-3);
    }

    #[test]
    fn stance_crouch_prone_speed_blend_linear() {
        let d = test_data();
        let m = &d.mw2;
        let mut s = Stance::new(m);
        let mut ev = Vec::new();
        s.request(StanceKind::Crouch, 0.0, true, &mut yes, &mut ev);
        assert_eq!(s.speed_scale(m, 0.0), 0.65, "stand->crouch switches at the start");
        s.advance(1.0, &mut ev);
        s.request(StanceKind::Prone, 1.0, true, &mut yes, &mut ev);
        assert!((s.speed_scale(m, 1.2) - (0.65 + (0.15 - 0.65) * 0.5)).abs() < 1e-5);
        assert!((s.speed_scale(m, 1.1) - (0.65 + (0.15 - 0.65) * 0.25)).abs() < 1e-5);
        s.advance(2.0, &mut ev);
        assert_eq!(s.speed_scale(m, 2.0), 0.15);
        assert_eq!(s.effective().accel(m), 19.0);
    }

    #[test]
    fn stance_prone_step_height() {
        let d = test_data();
        assert_eq!(d.mw2.step_height_prone, 10.0);
        assert_eq!(d.mw2.step_height, 18.0);
    }
}
