//! The three AC-130 guns (docs/M4_DESIGN.md section 7.2). Pure.
//!
//! Each gun keeps its clip, next-fire time and reload clock. Full auto on the fire button for every gun (MW2
//! fireType "Full Auto"). A reload starts when the clip reaches 0 and lasts `reloadTime` from THAT SHOT, as MW2's
//! `_ac130.gsc` `weaponReload()` (it waits `reloadTime` after the `weapon_fired` notify, then refills; review M4
//! minor 10), so the interval across a reload is `reloadTime` (105: one shot per 5.0 s, 40: 3.0 s, 25: 1.5 s); it
//! runs even while another gun is selected (the gsc thread waits on time). Switching is instant (MW2 has no
//! raise time in the gunship [I]); the order is 105 -> 40 -> 25 -> 105.

pub const GUNS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunSpec {
    pub fire_time_s: f64,
    pub clip: u32,
    pub reload_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunState {
    pub clip: u32,
    pub next_fire_t: f64,
    pub reload_until: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GunEvent {
    Shot { gun: usize, clip_after: u32, t: f64 },
    ReloadStart { gun: usize, until: f64 },
    ReloadDone { gun: usize, clip: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchVia {
    Wheel,
    Key,
}

impl SwitchVia {
    pub fn label(self) -> &'static str {
        match self {
            SwitchVia::Wheel => "wheel",
            SwitchVia::Key => "key",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Guns {
    pub specs: [GunSpec; GUNS],
    st: [GunState; GUNS],
    sel: usize,
}

impl Guns {
    /// All clips full, the 105 selected (MW2 `SwitchToWeapon("ac130_105mm_mp")`).
    pub fn new(specs: [GunSpec; GUNS], now: f64) -> Guns {
        Guns { specs, st: specs.map(|s| GunState { clip: s.clip, next_fire_t: now, reload_until: None }), sel: 0 }
    }

    pub fn selected(&self) -> usize {
        self.sel
    }

    pub fn state(&self, g: usize) -> GunState {
        self.st[g]
    }

    pub fn reloading(&self, g: usize) -> bool {
        self.st[g].reload_until.is_some()
    }

    /// Reload progress 0..1 of gun `g` (None when not reloading).
    pub fn reload_progress(&self, g: usize, now: f64) -> Option<f32> {
        let s = self.specs[g];
        self.st[g].reload_until.map(|u| (1.0 - ((u - now) / s.reload_s.max(1e-6))).clamp(0.0, 1.0) as f32)
    }

    /// Direct select; true when the selection changed.
    pub fn select(&mut self, g: usize) -> bool {
        if g < GUNS && g != self.sel {
            self.sel = g;
            true
        } else {
            false
        }
    }

    /// Cycles by `steps` (wheel notches: +1 next, -1 previous). True when the selection changed.
    pub fn cycle(&mut self, steps: i32) -> bool {
        if steps == 0 {
            return false;
        }
        let n = GUNS as i32;
        self.select(((self.sel as i32 + steps).rem_euclid(n)) as usize)
    }

    /// One task tick: reload clocks of every gun, then the selected gun fires while `trigger` is held, at most
    /// `max_per_tick` shots (catch-up after a hitch; anything beyond is dropped, not queued).
    pub fn step(&mut self, now: f64, trigger: bool, max_per_tick: u32, out: &mut Vec<GunEvent>) {
        for g in 0..GUNS {
            if let Some(u) = self.st[g].reload_until
                && now >= u
            {
                self.st[g].reload_until = None;
                self.st[g].clip = self.specs[g].clip;
                // the next shot is due at the end of the reload (or the end of the last fire cycle, if later)
                self.st[g].next_fire_t = self.st[g].next_fire_t.max(u);
                out.push(GunEvent::ReloadDone { gun: g, clip: self.st[g].clip });
            }
        }
        let g = self.sel;
        let spec = self.specs[g];
        let s = &mut self.st[g];
        if !trigger {
            // released: the earliest next shot is now (no burst of stored shots on the next press)
            if s.next_fire_t < now && s.reload_until.is_none() {
                s.next_fire_t = now;
            }
            return;
        }
        let mut shots = 0;
        while s.reload_until.is_none() && s.clip > 0 && now >= s.next_fire_t - 1e-9 && shots < max_per_tick.max(1) {
            let t = s.next_fire_t.max(now - spec.fire_time_s * (max_per_tick.max(1) as f64));
            s.clip -= 1;
            s.next_fire_t = t + spec.fire_time_s;
            shots += 1;
            out.push(GunEvent::Shot { gun: g, clip_after: s.clip, t });
            if s.clip == 0 {
                let until = t + spec.reload_s;
                s.reload_until = Some(until);
                out.push(GunEvent::ReloadStart { gun: g, until });
            }
        }
        // a hitch longer than the catch-up cap: drop the missed shots instead of firing them later
        if s.reload_until.is_none() && s.next_fire_t < now - spec.fire_time_s {
            s.next_fire_t = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: [GunSpec; 3] = [
        GunSpec { fire_time_s: 0.1, clip: 1, reload_s: 5.0 },
        GunSpec { fire_time_s: 0.3, clip: 4, reload_s: 3.0 },
        GunSpec { fire_time_s: 0.07, clip: 20, reload_s: 1.5 },
    ];

    /// Shot times of gun `g` over `secs` of held fire at `dt` ticks.
    fn held(g: usize, secs: f64, dt: f64) -> Vec<f64> {
        let mut guns = Guns::new(SPECS, 0.0);
        guns.select(g);
        let mut out = Vec::new();
        let mut t = 0.0;
        let mut shots = Vec::new();
        while t <= secs + 1e-9 {
            out.clear();
            guns.step(t, true, 2, &mut out);
            for e in &out {
                if let GunEvent::Shot { t: st, .. } = e {
                    shots.push(*st);
                }
            }
            t += dt;
        }
        shots
    }

    #[test]
    fn ac130_guns_cadence_per_gun() {
        let s = held(0, 10.0, 0.001);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].abs() < 1e-6 && (s[1] - 5.0).abs() < 2e-3, "105: 0 and 5.0 s (reloadTime from the shot) {s:?}");
        let s = held(1, 4.0, 0.001);
        assert_eq!(s.len(), 5);
        for (i, want) in [0.0, 0.3, 0.6, 0.9, 3.9].iter().enumerate() {
            assert!((s[i] - want).abs() < 2e-3, "40mm shot {i}: {} vs {want}", s[i]);
        }
        let s = held(2, 2.85, 0.001);
        assert_eq!(s.len(), 21, "20 rounds then the reload, then one more");
        assert!((s[19] - 19.0 * 0.07).abs() < 2e-3);
        assert!((s[20] - (19.0 * 0.07 + 1.5)).abs() < 2e-3, "25mm: +1.5 s after the 20th shot");
    }

    #[test]
    fn ac130_guns_reload_continues_while_switched_away() {
        let mut g = Guns::new(SPECS, 0.0);
        let mut out = Vec::new();
        g.step(0.0, true, 2, &mut out); // 105 fires its only round, reload until 5.0
        assert!(g.reloading(0));
        g.select(1);
        for k in 1..600 {
            out.clear();
            g.step(k as f64 * 0.01, false, 2, &mut out);
        }
        assert!(!g.reloading(0), "the 105 reloaded in the background");
        assert_eq!(g.state(0).clip, 1);
        assert!(g.reload_progress(0, 6.0).is_none());
        let mut g = Guns::new(SPECS, 0.0);
        g.step(0.0, true, 2, &mut out);
        assert!((g.reload_progress(0, 2.5).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ac130_guns_switch_order_wheel_q_and_keys() {
        let mut g = Guns::new(SPECS, 0.0);
        assert_eq!(g.selected(), 0);
        assert!(g.cycle(1));
        assert_eq!(g.selected(), 1);
        assert!(g.cycle(1));
        assert_eq!(g.selected(), 2);
        assert!(g.cycle(1));
        assert_eq!(g.selected(), 0, "25 -> 105");
        assert!(g.cycle(-1));
        assert_eq!(g.selected(), 2, "wheel back");
        assert!(!g.cycle(0));
        assert!(!g.select(2), "already selected");
        assert!(g.select(0) && g.select(1) && !g.select(7));
        assert_eq!(SwitchVia::Wheel.label(), "wheel");
    }

    #[test]
    fn ac130_guns_catch_up_capped() {
        let mut g = Guns::new(SPECS, 0.0);
        g.select(2);
        let mut out = Vec::new();
        g.step(0.0, true, 2, &mut out);
        out.clear();
        // a 0.5 s hitch: at most 2 shots in the next tick, the rest dropped
        g.step(0.5, true, 2, &mut out);
        let n = out.iter().filter(|e| matches!(e, GunEvent::Shot { .. })).count();
        assert_eq!(n, 2);
        out.clear();
        g.step(0.51, true, 2, &mut out);
        assert!(out.iter().filter(|e| matches!(e, GunEvent::Shot { .. })).count() <= 1, "no burst of stored shots");
    }

    #[test]
    fn ac130_guns_fractional_tick_long_run_rate() {
        // 16.7 ms ticks: the 25 mm's 0.07 s cadence stays exact over a long burst (shot times are on the schedule,
        // not on the tick grid)
        let s = held(2, 1.0, 0.0167);
        let n = s.len();
        assert_eq!(n, 15, "{s:?}");
        let span = s[n - 1] - s[0];
        assert!((span / (n - 1) as f64 - 0.07).abs() < 1e-9, "mean interval {}", span / (n - 1) as f64);
    }

    #[test]
    fn ac130_guns_release_resets_next_shot() {
        let mut g = Guns::new(SPECS, 0.0);
        g.select(1);
        let mut out = Vec::new();
        g.step(0.0, true, 2, &mut out);
        let mut t = 0.0;
        while t < 2.0 {
            t += 0.0167;
            g.step(t, false, 2, &mut out);
        }
        out.clear();
        g.step(t + 0.0167, true, 2, &mut out);
        let shots: Vec<_> = out.iter().filter(|e| matches!(e, GunEvent::Shot { .. })).collect();
        assert_eq!(shots.len(), 1, "a fresh press fires once, not a catch-up burst");
    }
}
