//! Landing and fall damage (docs/M5_DESIGN.md section 2.7), from MW2's `PM_CrashLand` (iw4sp.exe 0x140078B90) and
//! the jump slowdown of `Jump_ApplySlowdown`: fall height from the impact speed, the 0.67 landing slowdown, the
//! damage-landing TIME_LAND, the jump slowdown (x0.65 / 1800 ms below origin+18, else x0.5 / 1200 ms), MW2's
//! 200 / 350 in damage band, the view dip, and the ER HP effect.

use crate::movement_data::Mw2;

/// Fall height (inches) from the vertical impact speed: v^2 / 2g.
pub fn fall_height(m: &Mw2, v_impact: f32) -> f32 {
    v_impact * v_impact / (2.0 * m.gravity)
}

/// The impact speed of a fall of `h` inches.
pub fn impact_speed(m: &Mw2, h: f32) -> f32 {
    (2.0 * m.gravity * h.max(0.0)).sqrt()
}

/// MW2 fall damage percent: <= 200 in: 0; 200..350: (h - 200) / 150 x 100; >= 350: 100 (lethal).
pub fn damage_pct(m: &Mw2, h: f32) -> f32 {
    if h <= m.fall_min {
        0.0
    } else if h >= m.fall_max {
        100.0
    } else {
        (h - m.fall_min) / (m.fall_max - m.fall_min) * 100.0
    }
}

/// TIME_LAND length (ms) of a damage landing: min(500 + 35 x dmg, 2000).
pub fn time_land_ms(m: &Mw2, dmg_pct: f32) -> f32 {
    (m.time_land_base_ms + m.time_land_per_pct_ms * dmg_pct).min(m.time_land_max_ms)
}

/// Horizontal velocity scale of a damage landing: 0.5 at 500 ms .. 0.2 at >= 1500 ms.
pub fn time_land_vel_scale(m: &Mw2, land_ms: f32) -> f32 {
    let t = ((land_ms - 500.0) / 1000.0).clamp(0.0, 1.0);
    m.land_vel_scale_short + (m.land_vel_scale_long - m.land_vel_scale_short) * t
}

/// View dip (inches): min(24, 4 + 4 (h - 12) / 26), 0 below 12 in.
pub fn view_dip(m: &Mw2, h: f32) -> f32 {
    if h < m.land_slow_min { 0.0 } else { (4.0 + 4.0 * (h - m.land_slow_min) / 26.0).min(m.view_dip_max) }
}

/// The landing's effect on the controller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub fall_h: f32,
    pub dmg_pct: f32,
    /// Horizontal velocity factor applied.
    pub vel_scale: f32,
    /// The first factor (0.67 or the TIME_LAND scale) and the jump-slowdown factor, for the log.
    pub slow_a: f32,
    pub slow_b: f32,
    /// TIME_LAND ms (0 = none) and jump slowdown ms (0 = none).
    pub land_ms: f32,
    pub slow_ms: f32,
    pub dip_in: f32,
    pub lethal: bool,
}

/// Resolves a touchdown. `jumping`: the JUMPING flag is set; `below_band`: landed below jump origin + 18 in.
pub fn land(m: &Mw2, fall_h: f32, crouched: bool, jumping: bool, below_band: bool, jump_slowdown: bool) -> Landing {
    let dmg = damage_pct(m, fall_h);
    let lethal = dmg >= 100.0;
    let mut l = Landing {
        fall_h,
        dmg_pct: dmg,
        vel_scale: 1.0,
        slow_a: 1.0,
        slow_b: 1.0,
        land_ms: 0.0,
        slow_ms: 0.0,
        dip_in: view_dip(m, fall_h),
        lethal,
    };
    if dmg > 0.0 && dmg < 100.0 && !crouched {
        l.land_ms = time_land_ms(m, dmg);
        l.slow_a = time_land_vel_scale(m, l.land_ms);
    } else if fall_h >= m.land_slow_min {
        l.slow_a = m.land_slow_scale;
    }
    if jumping && jump_slowdown {
        if below_band {
            l.slow_b = m.slowdown_low_scale;
            l.slow_ms = m.slowdown_low_ms;
        } else {
            l.slow_b = m.slowdown_high_scale;
            l.slow_ms = m.slowdown_high_ms;
        }
    }
    l.vel_scale = l.slow_a * l.slow_b;
    l
}

/// `[movement] fall_damage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallDamage {
    Mw2,
    Off,
}

/// ER HP after a landing: round(dmg% / 100 x max_hp x scale), never above the current HP, non-lethal results keep at
/// least 1 HP, a lethal one writes 0. `None` = no HP write.
pub fn hp_after(mode: FallDamage, hp: i32, max_hp: i32, dmg_pct: f32, scale: f32) -> Option<i32> {
    if mode == FallDamage::Off || dmg_pct <= 0.0 || hp <= 0 || max_hp <= 0 {
        return None;
    }
    if dmg_pct >= 100.0 {
        return Some(0);
    }
    let dmg = (dmg_pct as f64 / 100.0 * max_hp as f64 * scale as f64).round() as i64;
    if dmg <= 0 {
        return None;
    }
    let new = (hp as i64 - dmg).max(1).min(hp as i64) as i32;
    (new != hp).then_some(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement_data::test_data;

    #[test]
    fn fall_damage_band() {
        let d = test_data();
        let m = &d.mw2;
        assert_eq!(damage_pct(m, 200.0), 0.0);
        assert!((damage_pct(m, 275.0) - 50.0).abs() < 1e-4);
        assert!((damage_pct(m, 349.9) - 99.933).abs() < 1e-2);
        assert_eq!(damage_pct(m, 350.0), 100.0);
        // 6 m and 9 m drops (B10)
        assert!((damage_pct(m, 6.0 / 0.0254) - 24.1).abs() < 0.05);
        assert_eq!(damage_pct(m, 9.0 / 0.0254), 100.0);
    }

    #[test]
    fn fall_height_from_impact_speed() {
        let d = test_data();
        let m = &d.mw2;
        assert!((fall_height(m, 565.7) - 200.0).abs() < 0.05);
        assert!((fall_height(m, 748.3) - 350.0).abs() < 0.05);
        assert!((impact_speed(m, 39.0) - 249.8).abs() < 0.05);
    }

    #[test]
    fn fall_time_land_and_velocity() {
        let d = test_data();
        let m = &d.mw2;
        assert_eq!(time_land_ms(m, 10.0), 850.0);
        assert_eq!(time_land_ms(m, 60.0), 2000.0);
        assert_eq!(time_land_vel_scale(m, 500.0), 0.5);
        assert!((time_land_vel_scale(m, 1500.0) - 0.2).abs() < 1e-6);
        // flat running jump: 190 x 0.67 x 0.65 = 82.8 with slowdown, 127.3 without
        let l = land(m, 39.0, false, true, true, true);
        assert!((190.0 * l.vel_scale - 82.745).abs() < 0.01);
        assert_eq!(l.slow_ms, 1800.0);
        let l = land(m, 39.0, false, true, true, false);
        assert!((190.0 * l.vel_scale - 127.3).abs() < 0.01);
        let l = land(m, 39.0, false, true, false, true);
        assert_eq!((l.slow_b, l.slow_ms), (0.5, 1200.0), "above origin+18");
        // damage landing: TIME_LAND, crouched: 0.67
        let l = land(m, 236.22, false, false, false, true);
        assert!(l.land_ms > 500.0 && l.slow_a < 0.5 && l.slow_a >= 0.2);
        let l = land(m, 236.22, true, false, false, true);
        assert_eq!((l.land_ms, l.slow_a), (0.0, 0.67));
        let l = land(m, 400.0, false, false, false, true);
        assert!(l.lethal && l.slow_a == 0.67);
        assert_eq!(land(m, 6.0, false, false, false, true).vel_scale, 1.0, "below 12 in: nothing");
        assert!((view_dip(m, 38.0) - 8.0).abs() < 1e-5);
        assert_eq!(view_dip(m, 1000.0), 24.0);
    }

    #[test]
    fn fall_hp_effect() {
        // non-lethal never below 1 HP, off = no change, lethal = 0, never raises
        assert_eq!(hp_after(FallDamage::Mw2, 1000, 1000, 50.0, 1.0), Some(500));
        assert_eq!(hp_after(FallDamage::Mw2, 100, 1000, 50.0, 1.0), Some(1));
        assert_eq!(hp_after(FallDamage::Mw2, 1, 1000, 50.0, 1.0), None);
        assert_eq!(hp_after(FallDamage::Mw2, 1000, 1000, 100.0, 1.0), Some(0));
        assert_eq!(hp_after(FallDamage::Off, 1000, 1000, 100.0, 1.0), None);
        assert_eq!(hp_after(FallDamage::Mw2, 1000, 1000, 0.0, 1.0), None);
        assert_eq!(hp_after(FallDamage::Mw2, 0, 1000, 50.0, 1.0), None, "already dead");
        assert_eq!(hp_after(FallDamage::Mw2, 1000, 1000, 24.1, 2.0), Some(518));
        for hp in [1, 5, 500, 1000] {
            for pct in [0.5f32, 10.0, 99.9] {
                if let Some(n) = hp_after(FallDamage::Mw2, hp, 1000, pct, 1.0) {
                    assert!(n >= 1 && n <= hp);
                }
            }
        }
    }
}
