//! M8 ladder audit (docs/M8_DESIGN.md section 10.3): the `LADDER state` / `LADDER end` lines from
//! `CSChrLadderModule.state`, read every tick by the existing first-person read. Pure.
//!
//! Lines are emitted on the start, the end, the first up / down / idle, each attack, the first coast, damage and the
//! get-off states; the climb steps (2-5) and the idles (8/9) after the first are collapsed into counters that the end
//! line reports. Ladders stay ER third person: this only logs what ER does.

use crate::logfmt;
use crate::vec3::V3;

/// The state names of the vendored `LadderState` enum (source: common_define.hks, [D]); 17 and 19 have no name.
pub fn state_name(s: i8) -> &'static str {
    match s {
        -1 => "None",
        0 => "StartBottom",
        1 => "StartTop",
        2 => "UpRight",
        3 => "UpLeft",
        4 => "DownRight",
        5 => "DownLeft",
        6 => "EndTop",
        7 => "EndBottom",
        8 => "IdleRight",
        9 => "IdleLeft",
        10 => "AttackUpRight",
        11 => "AttackUpLeft",
        12 => "AttackDownRight",
        13 => "AttackDownLeft",
        14 => "CoastStart",
        15 => "CoastRight",
        16 => "CoastStop",
        18 => "CoastLeft",
        20 => "CoastLanding",
        21 => "DamageSmall",
        22 => "DamageLarge",
        _ => "?",
    }
}

fn is_up(s: i8) -> bool {
    matches!(s, 2 | 3)
}
fn is_down(s: i8) -> bool {
    matches!(s, 4 | 5)
}
fn is_idle(s: i8) -> bool {
    matches!(s, 8 | 9)
}
fn is_attack(s: i8) -> bool {
    (10..=13).contains(&s)
}
fn is_coast(s: i8) -> bool {
    (14..=18).contains(&s)
}

/// One tick's ladder sample.
#[derive(Clone, Copy, Debug)]
pub struct LadderSample {
    pub now: f64,
    pub state: i8,
    /// Feet (metres, Havok frame).
    pub pos: V3,
    pub top: Option<V3>,
    pub bottom: Option<V3>,
    pub er_ground: bool,
    /// The movement controller's state label (`driving` / `arming` / `suspended:<reason>` / `off:<reason>`).
    pub move_state: &'static str,
    pub fp_on: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LadderLine {
    State { from: i8, to: i8, pos: V3, top: Option<V3>, bottom: Option<V3>, er_ground: bool, move_state: &'static str, fp_on: bool },
    End { last: i8, dur: f64, dy: f32, ups: u32, downs: u32, attacks: u32, coast: bool, pos: V3 },
}

impl LadderLine {
    /// The message after the `LADDER` tag.
    pub fn text(&self) -> String {
        let o = |v: Option<V3>| v.map_or("?".to_string(), logfmt::v3);
        match self {
            LadderLine::State { from, to, pos, top, bottom, er_ground, move_state, fp_on } => format!(
                "state from={from}({}) to={to}({}) pos={} top={} bottom={} er_ground={} move={move_state} fp={}",
                state_name(*from),
                state_name(*to),
                logfmt::v3(*pos),
                o(*top),
                o(*bottom),
                *er_ground as u8,
                *fp_on as u8
            ),
            LadderLine::End { last, dur, dy, ups, downs, attacks, coast, pos } => format!(
                "end state={last}({}) dur={dur:.2}s dy={dy:.2}m ups={ups} downs={downs} attacks={attacks} coast={} pos={}",
                state_name(*last),
                *coast as u8,
                logfmt::v3(*pos)
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LadderLog {
    prev: i8,
    t0: f64,
    y0: f32,
    last: i8,
    ups: u32,
    downs: u32,
    attacks: u32,
    coast: bool,
    seen_up: bool,
    seen_down: bool,
    seen_idle: bool,
    /// Episodes seen (ladder trips).
    pub episodes: u32,
}

impl Default for LadderLog {
    fn default() -> Self {
        LadderLog { prev: -1, t0: 0.0, y0: 0.0, last: -1, ups: 0, downs: 0, attacks: 0, coast: false, seen_up: false, seen_down: false, seen_idle: false, episodes: 0 }
    }
}

impl LadderLog {
    /// On a ladder now (state != -1 at the last sample).
    pub fn on_ladder(&self) -> bool {
        self.prev != -1
    }

    /// One tick: the lines to log (at most a state line and an end line).
    pub fn feed(&mut self, s: &LadderSample) -> Vec<LadderLine> {
        let mut out = Vec::new();
        let (from, to) = (self.prev, s.state);
        if from == to {
            return out;
        }
        self.prev = to;
        let line = |out: &mut Vec<LadderLine>| {
            out.push(LadderLine::State { from, to, pos: s.pos, top: s.top, bottom: s.bottom, er_ground: s.er_ground, move_state: s.move_state, fp_on: s.fp_on })
        };
        if from == -1 {
            // a new trip
            self.episodes += 1;
            self.t0 = s.now;
            self.y0 = s.pos.y;
            self.ups = 0;
            self.downs = 0;
            self.attacks = 0;
            self.coast = false;
            self.seen_up = false;
            self.seen_down = false;
            self.seen_idle = false;
        }
        if to == -1 {
            line(&mut out);
            out.push(LadderLine::End {
                last: self.last,
                dur: s.now - self.t0,
                dy: s.pos.y - self.y0,
                ups: self.ups,
                downs: self.downs,
                attacks: self.attacks,
                coast: self.coast,
                pos: s.pos,
            });
            self.last = -1;
            return out;
        }
        self.last = to;
        let mut emit = from == -1 || matches!(to, 0 | 1 | 6 | 7 | 20 | 21 | 22) || !(is_up(to) || is_down(to) || is_idle(to) || is_attack(to) || is_coast(to));
        if is_up(to) {
            self.ups += 1;
            emit |= !std::mem::replace(&mut self.seen_up, true);
        }
        if is_down(to) {
            self.downs += 1;
            emit |= !std::mem::replace(&mut self.seen_down, true);
        }
        if is_idle(to) {
            emit |= !std::mem::replace(&mut self.seen_idle, true);
        }
        if is_attack(to) {
            self.attacks += 1;
            emit = true;
        }
        if is_coast(to) {
            emit |= !std::mem::replace(&mut self.coast, true);
        }
        if emit {
            line(&mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(now: f64, state: i8, y: f32) -> LadderSample {
        LadderSample { now, state, pos: V3::new(1.0, y, 2.0), top: Some(V3::new(1.0, 10.0, 2.5)), bottom: Some(V3::new(1.0, 2.0, 2.5)), er_ground: false, move_state: "suspended:ladder", fp_on: false }
    }

    #[test]
    fn ladder_log_collapses_steps_and_summarises() {
        let mut l = LadderLog::default();
        let seq: [i8; 16] = [-1, 0, 2, 3, 2, 3, 8, 9, 8, 2, 3, 10, 2, 6, -1, -1];
        let mut lines = Vec::new();
        for (k, st) in seq.iter().enumerate() {
            lines.extend(l.feed(&s(k as f64 * 0.5, *st, 2.0 + k as f32 * 0.5)));
        }
        let texts: Vec<String> = lines.iter().map(|x| x.text()).collect();
        // start, first up, first idle, the attack, the get-off, the end state line + the end summary
        assert_eq!(lines.len(), 7, "{texts:#?}");
        assert!(texts[0].starts_with("state from=-1(None) to=0(StartBottom) pos=(1.00,2.50,2.00) top=(1.00,10.00,2.50)"), "{}", texts[0]);
        assert!(texts[1].contains("to=2(UpRight)"));
        assert!(texts[2].contains("to=8(IdleRight)"));
        assert!(texts[3].contains("to=10(AttackUpRight)"));
        assert!(texts[4].contains("to=6(EndTop)"));
        assert!(texts[5].contains("from=6(EndTop) to=-1(None)"));
        assert_eq!(lines[6], LadderLine::End { last: 6, dur: 6.5, dy: 6.5, ups: 7, downs: 0, attacks: 1, coast: false, pos: V3::new(1.0, 9.0, 2.0) });
        assert!(texts[6].starts_with("end state=6(EndTop) dur=6.50s dy=6.50m ups=7 downs=0 attacks=1 coast=0"), "{}", texts[6]);
        assert!(!l.on_ladder());
        assert_eq!(l.episodes, 1);
        // a second trip (down, a coast) resets the counters
        let mut lines = Vec::new();
        for (k, st) in [1i8, 4, 5, 14, 15, 20, 7, -1].iter().enumerate() {
            lines.extend(l.feed(&s(10.0 + k as f64, *st, 9.0 - k as f32)));
        }
        let end = lines.last().unwrap().text();
        assert!(end.contains("ups=0 downs=2 attacks=0 coast=1"), "{end}");
        assert_eq!(l.episodes, 2);
        assert_eq!(state_name(17), "?");
        assert_eq!(state_name(19), "?");
        assert_eq!(state_name(22), "DamageLarge");
    }
}
