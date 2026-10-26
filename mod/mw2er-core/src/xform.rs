//! Small rigid-transform math for the viewmodel (docs/M3_DESIGN.md section 3): quaternions (x, y, z, w),
//! 3x4 row-major affine matrices acting on column vectors, and 4x4 row-major matrices for the projection.
//! Everything is f32 and plain data so the same numbers go into the GPU constant buffers.

pub type Quat = [f32; 4];
pub type Vec3 = [f32; 3];
/// Rows 0..2 of a 4x4 affine matrix (row-major, column-vector convention: `p' = M * [p, 1]`).
pub type Mat34 = [f32; 12];
/// Row-major 4x4 matrix, column-vector convention.
pub type Mat4 = [[f32; 4]; 4];

pub const QUAT_IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];
pub const MAT34_IDENTITY: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

pub fn q_dot(a: &Quat, b: &Quat) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

pub fn q_normalize(q: Quat) -> Quat {
    let n = q_dot(&q, &q).sqrt();
    if n > 1e-12 && n.is_finite() { [q[0] / n, q[1] / n, q[2] / n, q[3] / n] } else { QUAT_IDENTITY }
}

/// Normalised linear interpolation along the shorter arc.
pub fn q_nlerp(a: &Quat, b: &Quat, u: f32) -> Quat {
    let s = if q_dot(a, b) < 0.0 { -1.0 } else { 1.0 };
    q_normalize([
        a[0] + u * (s * b[0] - a[0]),
        a[1] + u * (s * b[1] - a[1]),
        a[2] + u * (s * b[2] - a[2]),
        a[3] + u * (s * b[3] - a[3]),
    ])
}

/// Linear interpolation of the raw keys (keys are already sign-continuous), then normalisation.
pub fn q_lerp_keys(a: &Quat, b: &Quat, u: f32) -> Quat {
    q_normalize([
        a[0] + u * (b[0] - a[0]),
        a[1] + u * (b[1] - a[1]),
        a[2] + u * (b[2] - a[2]),
        a[3] + u * (b[3] - a[3]),
    ])
}

pub fn v_lerp(a: &Vec3, b: &Vec3, u: f32) -> Vec3 {
    [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1]), a[2] + u * (b[2] - a[2])]
}

pub fn v_sub(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn v_len(a: &Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

pub fn v_normalize(a: Vec3) -> Vec3 {
    let n = v_len(&a);
    if n > 1e-12 && n.is_finite() { [a[0] / n, a[1] / n, a[2] / n] } else { [0.0, 0.0, 0.0] }
}

/// Affine matrix from a translation and a unit quaternion.
pub fn m_trs(t: &Vec3, q: &Quat) -> Mat34 {
    let [x, y, z, w] = *q;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - z * w),
        2.0 * (x * z + y * w),
        t[0],
        2.0 * (x * y + z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - x * w),
        t[1],
        2.0 * (x * z - y * w),
        2.0 * (y * z + x * w),
        1.0 - 2.0 * (x * x + y * y),
        t[2],
    ]
}

/// `a * b` (apply b first).
pub fn m_mul(a: &Mat34, b: &Mat34) -> Mat34 {
    let mut o = [0.0f32; 12];
    for r in 0..3 {
        for c in 0..4 {
            let mut s = a[r * 4] * b[c] + a[r * 4 + 1] * b[4 + c] + a[r * 4 + 2] * b[8 + c];
            if c == 3 {
                s += a[r * 4 + 3];
            }
            o[r * 4 + c] = s;
        }
    }
    o
}

/// Inverse of a rigid (rotation + translation) matrix.
pub fn m_inv_rigid(m: &Mat34) -> Mat34 {
    let r = [[m[0], m[4], m[8]], [m[1], m[5], m[9]], [m[2], m[6], m[10]]];
    let t = [m[3], m[7], m[11]];
    let mut o = [0.0f32; 12];
    for i in 0..3 {
        o[i * 4] = r[i][0];
        o[i * 4 + 1] = r[i][1];
        o[i * 4 + 2] = r[i][2];
        o[i * 4 + 3] = -(r[i][0] * t[0] + r[i][1] * t[1] + r[i][2] * t[2]);
    }
    o
}

pub fn m_point(m: &Mat34, p: &Vec3) -> Vec3 {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

pub fn m_translation(m: &Mat34) -> Vec3 {
    [m[3], m[7], m[11]]
}

pub fn m_from_translation(t: &Vec3) -> Mat34 {
    [1.0, 0.0, 0.0, t[0], 0.0, 1.0, 0.0, t[1], 0.0, 0.0, 1.0, t[2]]
}

pub fn m4_from34(m: &Mat34) -> Mat4 {
    [[m[0], m[1], m[2], m[3]], [m[4], m[5], m[6], m[7]], [m[8], m[9], m[10], m[11]], [0.0, 0.0, 0.0, 1.0]]
}

pub fn m4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [[0.0f32; 4]; 4];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

/// `M * [p, 1]` as a homogeneous 4-vector.
pub fn m4_apply(m: &Mat4, p: &Vec3) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (r, v) in o.iter_mut().enumerate() {
        *v = m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32], eps: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
    }

    #[test]
    fn xform_trs_inverse_roundtrip() {
        let q = q_normalize([0.2, -0.4, 0.1, 0.9]);
        let m = m_trs(&[1.0, 2.0, 3.0], &q);
        let i = m_inv_rigid(&m);
        assert!(close(&m_mul(&m, &i), &MAT34_IDENTITY, 1e-5));
        let p = m_point(&m, &[0.5, -1.0, 2.0]);
        assert!(close(&m_point(&i, &p), &[0.5, -1.0, 2.0], 1e-5));
    }

    #[test]
    fn xform_quarter_turn_about_z() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let m = m_trs(&[0.0; 3], &[0.0, 0.0, h, h]);
        assert!(close(&m_point(&m, &[1.0, 0.0, 0.0]), &[0.0, 1.0, 0.0], 1e-6));
    }

    #[test]
    fn xform_nlerp_shorter_arc() {
        let a = QUAT_IDENTITY;
        let b = [0.0, 0.0, 0.0, -1.0]; // the same rotation
        assert!(close(&q_nlerp(&a, &b, 0.5), &QUAT_IDENTITY, 1e-6));
    }
}
