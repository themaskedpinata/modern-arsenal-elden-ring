//! C3 (docs/C3_DESIGN.md 1.4): `MOVE assist`. When the player holds a move direction and our controller makes no
//! progress (a steep slope ER walks, a root, a small rock, a body our rays block but ER's proxy passes), ER's own
//! movement takes over for a short window (`Reason::Assist`, the existing suspend / resync path), then hands back.
//! This is what the user did by hand with F5 in C2.
//!
//! - Trigger: driving, grounded, move input held, ER-world speed below `stall_speed` for `stall_s`; or (with
//!   `after_refusal`) a mantle refusal while stalled, which skips the wait [deviation from the design's "immediately":
//!   a held jump in the open is refused `no_face` on every press, so the refusal alone must not hand back].
//! - Hold `hold_s`; a repeat within 3 s of the previous window's end doubles it, up to 2.0 s.
//! - At most `max_per_min` per 60 s (then `Capped`, logged once per rest).
//! - Never in the air, during a mantle, on a ladder, in the AC-130, the nuke or a menu.

/// `[movement.assist]` in the detector's units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssistParams {
    pub enabled: bool,
    pub stall_speed: f32,
    pub stall_s: f64,
    pub hold_s: f64,
    pub max_per_min: u32,
    pub after_refusal: bool,
}

impl Default for AssistParams {
    fn default() -> Self {
        AssistParams { enabled: true, stall_speed: 0.15, stall_s: 0.4, hold_s: 0.8, max_per_min: 8, after_refusal: true }
    }
}

/// The longest assist window (s).
pub const ASSIST_MAX_HOLD_S: f64 = 2.0;
/// A fire within this long after the previous window ended doubles the hold.
pub const ASSIST_REPEAT_S: f64 = 3.0;
const WINDOW_S: f64 = 60.0;

/// One driving tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AssistIn {
    pub now: f64,
    /// Move input held (W/A/S/D or the left stick past its deadzone).
    pub wish: bool,
    /// Our controller is on the ground.
    pub grounded: bool,
    /// ER-world planar speed this tick (m/s).
    pub speed: f32,
    /// A mantle refusal (`no_face`, `path_blocked`, `edge`) happened this tick.
    pub refusal: bool,
    /// Mantle, ladder, AC-130, nuke or menu: never.
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistWhy {
    Stall,
    Refusal,
}

impl AssistWhy {
    pub fn label(self) -> &'static str {
        match self {
            AssistWhy::Stall => "stall",
            AssistWhy::Refusal => "refusal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AssistEvent {
    Fire { n: u32, hold_s: f64, why: AssistWhy, stalled_s: f64 },
    Capped { n: u32, until: f64 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assist {
    stall_since: Option<f64>,
    fires: Vec<f64>,
    last_end: f64,
    last_hold: f64,
    capped_until: f64,
}

impl Assist {
    /// A handback / resync / warp: the stall timer restarts.
    pub fn reset(&mut self) {
        self.stall_since = None;
    }

    pub fn recent(&self, now: f64) -> u32 {
        self.fires.iter().filter(|t| now - **t < WINDOW_S).count() as u32
    }

    pub fn tick(&mut self, i: &AssistIn, p: &AssistParams) -> Option<AssistEvent> {
        if !p.enabled || i.blocked || !i.grounded || !i.wish || !(i.speed.is_finite()) {
            self.stall_since = None;
            return None;
        }
        if i.speed >= p.stall_speed {
            self.stall_since = None;
            return None;
        }
        let since = *self.stall_since.get_or_insert(i.now);
        let stalled = i.now - since;
        let why = if stalled >= p.stall_s {
            AssistWhy::Stall
        } else if p.after_refusal && i.refusal && stalled > 0.0 {
            AssistWhy::Refusal
        } else {
            return None;
        };
        self.stall_since = None;
        self.fires.retain(|t| i.now - *t < WINDOW_S);
        let k = self.fires.len() as u32;
        if k >= p.max_per_min.max(1) {
            let until = self.fires.iter().copied().fold(f64::INFINITY, f64::min) + WINDOW_S;
            if i.now >= self.capped_until {
                self.capped_until = until;
                return Some(AssistEvent::Capped { n: k, until });
            }
            return None;
        }
        let base = p.hold_s.clamp(0.3, ASSIST_MAX_HOLD_S);
        let hold = if !self.fires.is_empty() && i.now - self.last_end < ASSIST_REPEAT_S { (self.last_hold * 2.0).min(ASSIST_MAX_HOLD_S) } else { base };
        self.fires.push(i.now);
        self.last_hold = hold;
        self.last_end = i.now + hold;
        Some(AssistEvent::Fire { n: k + 1, hold_s: hold, why, stalled_s: stalled })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(a: &mut Assist, p: &AssistParams, now: f64, speed: f32) -> Option<AssistEvent> {
        a.tick(&AssistIn { now, wish: true, grounded: true, speed, refusal: false, blocked: false }, p)
    }

    #[test]
    fn c3_assist_stall_timer_fires_after_stall_s() {
        let p = AssistParams::default();
        let mut a = Assist::default();
        let mut t = 0.0;
        let mut got = None;
        while t < 1.0 && got.is_none() {
            got = tick(&mut a, &p, t, 0.05);
            t += 1.0 / 60.0;
        }
        match got {
            Some(AssistEvent::Fire { n: 1, hold_s, why: AssistWhy::Stall, stalled_s }) => {
                assert!((hold_s - 0.8).abs() < 1e-9 && stalled_s >= 0.4 && t < 0.45, "{t} {stalled_s}");
            }
            g => panic!("{g:?}"),
        }
        // moving resets the timer
        let mut a = Assist::default();
        for k in 0..60 {
            let sp = if k % 20 == 19 { 1.0 } else { 0.0 };
            assert_eq!(tick(&mut a, &p, k as f64 / 60.0, sp), None);
        }
    }

    #[test]
    fn c3_assist_refusal_skips_the_wait_only_while_stalled() {
        let p = AssistParams::default();
        let mut a = Assist::default();
        let r = |a: &mut Assist, now: f64, speed: f32| a.tick(&AssistIn { now, wish: true, grounded: true, speed, refusal: true, blocked: false }, &p);
        assert_eq!(r(&mut a, 0.0, 3.0), None, "a refusal while running (jump in the open) is never an assist");
        assert_eq!(r(&mut a, 0.1, 0.0), None, "first stalled tick starts the timer");
        assert!(matches!(r(&mut a, 0.12, 0.0), Some(AssistEvent::Fire { why: AssistWhy::Refusal, .. })));
        let off = AssistParams { after_refusal: false, ..p };
        let mut a = Assist::default();
        a.tick(&AssistIn { now: 0.0, wish: true, grounded: true, speed: 0.0, refusal: true, blocked: false }, &off);
        assert_eq!(a.tick(&AssistIn { now: 0.1, wish: true, grounded: true, speed: 0.0, refusal: true, blocked: false }, &off), None);
    }

    #[test]
    fn c3_assist_cap_per_minute_and_doubling() {
        let p = AssistParams::default();
        let mut a = Assist::default();
        let mut fires = Vec::new();
        let mut capped = 0;
        let mut t = 0.0;
        while t < 59.0 {
            match tick(&mut a, &p, t, 0.0) {
                Some(AssistEvent::Fire { hold_s, n, .. }) => {
                    fires.push((t, hold_s, n));
                    t += hold_s + 0.5; // the hand-back window, then stalled again
                }
                Some(AssistEvent::Capped { .. }) => capped += 1,
                None => {}
            }
            t += 1.0 / 60.0;
        }
        assert_eq!(fires.len(), 8, "{fires:?}");
        assert_eq!(capped, 1, "one Capped line per rest");
        assert!((fires[0].1 - 0.8).abs() < 1e-9);
        assert!((fires[1].1 - 1.6).abs() < 1e-9, "repeat within 3 s doubles");
        assert!((fires[2].1 - 2.0).abs() < 1e-9, "capped at 2.0 s");
        // a repeat after a long quiet spell starts at hold_s again
        let mut a = Assist::default();
        tick(&mut a, &p, 0.0, 0.0);
        assert!(matches!(tick(&mut a, &p, 0.5, 0.0), Some(AssistEvent::Fire { .. })));
        tick(&mut a, &p, 10.0, 0.0);
        assert!(matches!(tick(&mut a, &p, 10.5, 0.0), Some(AssistEvent::Fire { hold_s, .. }) if (hold_s - 0.8).abs() < 1e-9));
    }

    #[test]
    fn c3_assist_never_in_air_mantle_or_menu() {
        let p = AssistParams::default();
        for (grounded, blocked, wish) in [(false, false, true), (true, true, true), (true, false, false)] {
            let mut a = Assist::default();
            for k in 0..120 {
                let e = a.tick(&AssistIn { now: k as f64 / 60.0, wish, grounded, speed: 0.0, refusal: true, blocked }, &p);
                assert_eq!(e, None, "{grounded} {blocked} {wish}");
            }
        }
        let off = AssistParams { enabled: false, ..p };
        let mut a = Assist::default();
        for k in 0..120 {
            assert_eq!(tick(&mut a, &off, k as f64 / 60.0, 0.0), None);
        }
    }
}
