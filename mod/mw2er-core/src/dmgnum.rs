//! C3 (docs/C3_DESIGN.md 2.6): floating damage numbers at the hit position. The task pushes one per attributed HIT of
//! ours (never an HPDROP, never a peer's drop); the HUD projects each anchor every frame, lets it rise and fade.
//! Bursts and pellets on one target within `merge_s` merge into one number (the damage adds up, the timer restarts).
//!
//! Pure. ER is Y-up [V]; positions are ER world metres (the task's frame; shifted on a chunk re-base).

use std::collections::VecDeque;

/// What dealt the hit (the colour does not depend on it; the log line names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmgSrc {
    Gun,
    Launcher,
    Ac130,
}

impl DmgSrc {
    pub fn label(self) -> &'static str {
        match self {
            DmgSrc::Gun => "gun",
            DmgSrc::Launcher => "launcher",
            DmgSrc::Ac130 => "ac130",
        }
    }
}

/// Where the number is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorVia {
    /// Our impact marker of the hit.
    Impact,
    /// No impact: the character position + 0.9 x the capsule height.
    Chr,
}

impl AnchorVia {
    pub fn label(self) -> &'static str {
        match self {
            AnchorVia::Impact => "impact",
            AnchorVia::Chr => "chr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DmgNum {
    /// Start of its life (restarted by a merge).
    pub t: f64,
    pub handle: u64,
    pub anchor: [f32; 3],
    pub dmg: i32,
    pub kill: bool,
    pub head: bool,
    pub src: DmgSrc,
}

/// The number's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmgColor {
    White,
    /// Head-shot guess.
    Yellow,
    /// A kill (text gets a trailing `x`).
    Red,
}

impl DmgNum {
    pub fn color(&self) -> DmgColor {
        if self.kill {
            DmgColor::Red
        } else if self.head {
            DmgColor::Yellow
        } else {
            DmgColor::White
        }
    }

    pub fn text(&self) -> String {
        if self.kill { format!("{}x", self.dmg.max(0)) } else { self.dmg.max(0).to_string() }
    }
}

/// The anchor of a hit: the impact, else the character position + 0.9 x the capsule height (ER is Y-up).
pub fn anchor(impact: Option<[f32; 3]>, chr_pos: [f32; 3], capsule_h: f32) -> ([f32; 3], AnchorVia) {
    match impact {
        Some(i) if i.iter().all(|v| v.is_finite()) => (i, AnchorVia::Impact),
        _ => ([chr_pos[0], chr_pos[1] + capsule_h * 0.9, chr_pos[2]], AnchorVia::Chr),
    }
}

/// Head-shot guess [I]: the impact is in the top 18 % of the capsule.
pub fn head_guess(impact: Option<[f32; 3]>, chr_pos: [f32; 3], capsule_h: f32) -> bool {
    impact.is_some_and(|i| i[1] - chr_pos[1] >= 0.82 * capsule_h)
}

/// The look of a number of age `a` with life `life`: (world rise m, screen rise fraction 0..1 (x 18 px x s1080),
/// alpha). None once its life is over (or a bad age).
pub fn style(a: f64, life: f64) -> Option<(f32, f32, f32)> {
    if !(a >= 0.0) || !(life > 0.0) || a >= life {
        return None;
    }
    let u = (a / life) as f32;
    let alpha = if u < 0.45 { 1.0 } else { (1.0 - (u - 0.45) / 0.55).clamp(0.0, 1.0) };
    Some((0.35 * u, u, alpha))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DmgNums {
    list: VecDeque<DmgNum>,
}

impl DmgNums {
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DmgNum> {
        self.list.iter()
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Adds a hit. Returns true when it merged into a live number of the same target (within `merge_s` of that
    /// number's last hit; a kill number never takes more). At most `max` numbers live (the oldest goes first).
    #[allow(clippy::too_many_arguments)]
    pub fn push(&mut self, now: f64, handle: u64, at: [f32; 3], dmg: i32, kill: bool, head: bool, src: DmgSrc, merge_s: f64, max: usize) -> bool {
        if let Some(n) = self.list.iter_mut().rev().find(|n| n.handle == handle && !n.kill && now - n.t <= merge_s && now >= n.t) {
            n.dmg += dmg.max(0);
            n.kill |= kill;
            n.head |= head;
            n.t = now;
            n.anchor = at;
            n.src = src;
            return true;
        }
        self.list.push_back(DmgNum { t: now, handle, anchor: at, dmg: dmg.max(0), kill, head, src });
        while self.list.len() > max.max(1) {
            self.list.pop_front();
        }
        false
    }

    /// Drops numbers whose life is over.
    pub fn prune(&mut self, now: f64, life: f64) {
        self.list.retain(|n| now - n.t < life && now >= n.t - 1.0);
    }

    /// A chunk re-base moved the world by `d` (metres): the anchors move with it.
    pub fn rebase(&mut self, d: [f32; 3]) {
        for n in self.list.iter_mut() {
            for k in 0..3 {
                n.anchor[k] += d[k];
            }
        }
    }

    pub fn to_vec(&self) -> Vec<DmgNum> {
        self.list.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c3_dmgnum_merge_within_window_restarts_timer() {
        let mut d = DmgNums::default();
        assert!(!d.push(1.0, 7, [0.0, 1.0, 0.0], 40, false, false, DmgSrc::Gun, 0.25, 24));
        assert!(d.push(1.2, 7, [0.1, 1.1, 0.0], 35, false, true, DmgSrc::Gun, 0.25, 24));
        assert_eq!(d.len(), 1);
        let n = d.iter().next().copied().unwrap();
        assert_eq!((n.dmg, n.head, n.t), (75, true, 1.2));
        // after the window: a new number
        assert!(!d.push(1.5, 7, [0.0; 3], 10, false, false, DmgSrc::Gun, 0.25, 24));
        // another target never merges
        assert!(!d.push(1.55, 8, [0.0; 3], 10, false, false, DmgSrc::Gun, 0.25, 24));
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn c3_dmgnum_kill_colour_and_no_merge_into_kill() {
        let mut d = DmgNums::default();
        d.push(0.0, 1, [0.0; 3], 300, true, false, DmgSrc::Launcher, 0.25, 24);
        assert!(!d.push(0.1, 1, [0.0; 3], 5, false, false, DmgSrc::Gun, 0.25, 24), "a kill number never takes more");
        let v = d.to_vec();
        assert_eq!((v[0].color(), v[0].text().as_str()), (DmgColor::Red, "300x"));
        assert_eq!((v[1].color(), v[1].text().as_str()), (DmgColor::White, "5"));
        let y = DmgNum { head: true, ..v[1] };
        assert_eq!(y.color(), DmgColor::Yellow);
    }

    #[test]
    fn c3_dmgnum_life_fade_curve() {
        assert_eq!(style(0.0, 0.9), Some((0.0, 0.0, 1.0)));
        let (r, u, a) = style(0.4, 0.9).unwrap();
        assert!((r - 0.35 * 0.4 / 0.9).abs() < 1e-6 && (u - 0.4 / 0.9).abs() < 1e-6 && a == 1.0);
        let (_, _, a) = style(0.9 * 0.725, 0.9).unwrap();
        assert!((a - 0.5).abs() < 1e-3, "{a}");
        assert_eq!(style(0.9, 0.9), None);
        assert_eq!(style(-0.1, 0.9), None);
        assert_eq!(style(f64::NAN, 0.9), None);
    }

    #[test]
    fn c3_dmgnum_cap_prune_and_rebase() {
        let mut d = DmgNums::default();
        for i in 0..30u64 {
            d.push(i as f64 * 0.01, i, [0.0; 3], 1, false, false, DmgSrc::Gun, 0.25, 24);
        }
        assert_eq!(d.len(), 24);
        assert_eq!(d.iter().next().unwrap().handle, 6, "oldest dropped first");
        d.rebase([32.0, 0.0, -8.0]);
        assert_eq!(d.iter().next().unwrap().anchor, [32.0, 0.0, -8.0]);
        d.prune(0.29 + 0.9, 0.9);
        assert!(d.len() < 24);
        d.prune(10.0, 0.9);
        assert!(d.is_empty());
    }

    #[test]
    fn c3_dmgnum_head_guess_threshold_and_anchor() {
        let chr = [10.0, 5.0, 3.0];
        assert!(head_guess(Some([10.0, 5.0 + 0.82 * 2.2 + 0.01, 3.0]), chr, 2.2));
        assert!(!head_guess(Some([10.0, 5.0 + 1.2, 3.0]), chr, 2.2));
        assert!(!head_guess(None, chr, 2.2));
        assert_eq!(anchor(Some([1.0, 2.0, 3.0]), chr, 2.2), ([1.0, 2.0, 3.0], AnchorVia::Impact));
        let (a, v) = anchor(None, chr, 2.0);
        assert_eq!((a, v), ([10.0, 6.8, 3.0], AnchorVia::Chr));
        assert_eq!(anchor(Some([f32::NAN, 0.0, 0.0]), chr, 2.0).1, AnchorVia::Chr);
    }

    /// docs/C3_DESIGN.md 2.6: projection of an anchor 5 m in front of a camera lands in the screen centre; behind
    /// the camera is None (FP-eye and TP-camera fixtures).
    #[test]
    fn c3_project_round_trip_fp_and_tp() {
        use crate::ac130_hud::project;
        // identity rows (x right, y up, z forward), eye at 1.6 m
        let rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let eye = [0.0, 1.6, 0.0];
        let p = project(eye, rows, 0.84, 1920.0, 1080.0, [0.0, 1.6, 5.0]).unwrap();
        assert!((p[0] - 960.0).abs() < 0.01 && (p[1] - 540.0).abs() < 0.01, "{p:?}");
        assert!(project(eye, rows, 0.84, 1920.0, 1080.0, [0.0, 1.6, -5.0]).is_none());
        // a third-person camera 3.5 m behind and above, looking along +z: the anchor above the screen centre line
        let cam = [0.0, 2.5, -3.5];
        let p = project(cam, rows, 0.84, 1920.0, 1080.0, [0.0, 2.6, 5.0]).unwrap();
        assert!((p[0] - 960.0).abs() < 0.01 && p[1] < 540.0, "{p:?}");
        // the rise moves it up on screen
        let q = project(cam, rows, 0.84, 1920.0, 1080.0, [0.0, 2.6 + 0.35, 5.0]).unwrap();
        assert!(q[1] < p[1]);
    }
}
