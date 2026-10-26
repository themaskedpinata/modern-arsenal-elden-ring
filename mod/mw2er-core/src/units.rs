//! Unit conversion between IW4 (MW2) units and Elden Ring metres.
//!
//! IW4 world units are inches (verified for weapon ranges and model sizes in MODLOG M1);
//! Elden Ring uses metres.

/// One IW4 unit (inch) in metres.
pub const INCH_TO_M: f64 = 0.0254;

/// Inches to metres.
pub fn inches_to_m(inches: f64) -> f64 {
    inches * INCH_TO_M
}

/// Metres to inches.
pub fn m_to_inches(metres: f64) -> f64 {
    metres / INCH_TO_M
}

/// Rounds of fire per minute for a fire interval in seconds.
pub fn rpm(fire_time_s: f64) -> f64 {
    if fire_time_s > 0.0 { 60.0 / fire_time_s } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ump45_ranges_in_metres() {
        assert!((inches_to_m(600.0) - 15.24).abs() < 1e-9);
        assert!((inches_to_m(850.0) - 21.59).abs() < 1e-9);
        assert!((m_to_inches(15.24) - 600.0).abs() < 1e-9);
    }

    #[test]
    fn ump45_rpm() {
        assert!((rpm(0.09) - 666.666_666).abs() < 1e-3);
        assert_eq!(rpm(0.0), 0.0);
    }
}
