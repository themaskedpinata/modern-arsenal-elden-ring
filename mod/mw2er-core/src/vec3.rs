//! Minimal 3D vector math for aiming (Elden Ring world space: metres, +Y up).

use std::ops::{Add, Mul, Neg, Sub};

/// A plain 3D vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// World up in Elden Ring (metres, Y-up).
pub const UP: V3 = V3 { x: 0.0, y: 1.0, z: 0.0 };

impl V3 {
    pub const ZERO: V3 = V3 { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Length in the horizontal (XZ) plane.
    pub fn planar_len(self) -> f32 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    /// Unit vector, or `None` for a (near) zero or non-finite vector.
    pub fn normalize(self) -> Option<V3> {
        let l = self.len();
        if l.is_finite() && l > 1e-6 { Some(self * (1.0 / l)) } else { None }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Angle between two vectors in degrees (0 for degenerate input).
    pub fn angle_deg(self, o: V3) -> f32 {
        match (self.normalize(), o.normalize()) {
            (Some(a), Some(b)) => a.dot(b).clamp(-1.0, 1.0).acos().to_degrees(),
            _ => 0.0,
        }
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        V3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ops() {
        let a = V3::new(1.0, 0.0, 0.0);
        let b = V3::new(0.0, 0.0, 1.0);
        assert_eq!(a.dot(b), 0.0);
        assert_eq!(a.cross(b), V3::new(0.0, -1.0, 0.0));
        assert!((V3::new(3.0, 4.0, 0.0).len() - 5.0).abs() < 1e-6);
        assert!(V3::ZERO.normalize().is_none());
        assert!((a.angle_deg(b) - 90.0).abs() < 1e-4);
        assert!((V3::new(3.0, 9.0, 4.0).planar_len() - 5.0).abs() < 1e-6);
    }
}
