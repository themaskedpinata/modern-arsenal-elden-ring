//! MW2 damage falloff in metres, and the fixed falloff bands baked into regulation rows.
//!
//! IW4 gives full `damage` up to `maxDamageRange`, `minDamage` from `minDamageRange` on. Between the
//! two we interpolate linearly and round to the nearest integer; both choices are inference
//! (IW's exact rounding is unknown), documented in docs/M2_DESIGN.md section 10.5.
//!
//! The same band rule is implemented in `tools/scripts/build_regulation.py`; the `real_data` test
//! checks the two agree on the built `rows.json`.

/// Maximum number of bands (rows) per weapon.
pub const MAX_BANDS: usize = 16;

/// Falloff parameters of one gun in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Falloff {
    pub damage: i32,
    pub min_damage: i32,
    pub max_damage_range_m: f64,
    pub min_damage_range_m: f64,
}

impl Falloff {
    /// MW2 damage at distance `d_m` metres.
    pub fn damage_at(&self, d_m: f64) -> i32 {
        if !d_m.is_finite() || d_m >= self.min_damage_range_m {
            return self.min_damage;
        }
        if d_m <= self.max_damage_range_m {
            return self.damage;
        }
        let span = self.min_damage_range_m - self.max_damage_range_m;
        if span <= 0.0 {
            return self.min_damage;
        }
        let t = (d_m - self.max_damage_range_m) / span;
        let v = self.damage as f64 + (self.min_damage - self.damage) as f64 * t;
        (v.round() as i32).clamp(self.min_damage.min(self.damage), self.damage.max(self.min_damage))
    }
}

/// MW2 damage of each band, highest first. Every integer from `damage` down to `min_damage` when
/// that is at most 16 values; otherwise 16 evenly spaced, rounded values (first = damage,
/// last = min_damage).
pub fn band_damages(damage: i32, min_damage: i32) -> Vec<i32> {
    let (hi, lo) = (damage.max(min_damage), damage.min(min_damage));
    let n = (hi - lo + 1) as usize;
    if n <= MAX_BANDS {
        return (lo..=hi).rev().collect();
    }
    let mut v: Vec<i32> = (0..MAX_BANDS)
        .map(|i| {
            let t = i as f64 / (MAX_BANDS - 1) as f64;
            (hi as f64 - (hi - lo) as f64 * t).round() as i32
        })
        .collect();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ump() -> Falloff {
        // UMP45 values as converted (600/850 inches); tests may hold real values, the DLL may not.
        Falloff { damage: 40, min_damage: 35, max_damage_range_m: 15.24, min_damage_range_m: 21.59 }
    }

    #[test]
    fn falloff_known_answers() {
        let f = ump();
        assert_eq!(f.damage_at(0.0), 40);
        assert_eq!(f.damage_at(15.24), 40);
        assert_eq!(f.damage_at(17.0), 39);
        assert_eq!(f.damage_at(20.0), 36);
        assert_eq!(f.damage_at(21.59), 35);
        assert_eq!(f.damage_at(1000.0), 35);
        assert_eq!(f.damage_at(f64::NAN), 35);
    }

    #[test]
    fn falloff_is_monotonic() {
        let f = ump();
        let mut prev = i32::MAX;
        for i in 0..4000 {
            let d = f.damage_at(i as f64 * 0.01);
            assert!(d <= prev);
            prev = d;
        }
    }

    #[test]
    fn falloff_every_damage_has_a_band() {
        let f = ump();
        let bands = band_damages(40, 35);
        assert_eq!(bands, vec![40, 39, 38, 37, 36, 35]);
        for i in 0..3000 {
            assert!(bands.contains(&f.damage_at(i as f64 * 0.01)));
        }
        assert_eq!(band_damages(30, 20).len(), 11);
        assert_eq!(band_damages(70, 70), vec![70]);
        let wide = band_damages(100, 20);
        assert!(wide.len() <= MAX_BANDS);
        assert_eq!((wide[0], *wide.last().unwrap()), (100, 20));
    }
}
