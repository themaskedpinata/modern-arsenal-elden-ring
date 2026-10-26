//! Viewmodel projection and view (docs/M3_DESIGN.md section 3.3).
//!
//! The viewmodel needs no Elden Ring camera: the eye is the posed `tag_view`, the field of view is MW2's
//! (`cg_fov` 65 at hip, `adsZoomFov` 55 in ADS, both 4:3 Hor+), and the depth range is the viewmodel's own
//! (reversed Z, near 0.1 in, far 1000 in). Units stay inches up to the projection.
//!
//! Spaces: MW2 is X forward, Y left, Z up (right-handed). D3D view space is x right, y up, z forward, so the
//! axis map is `(X, Y, Z) -> (-Y, Z, X)` (a reflection, det -1).

use crate::xform::{Mat4, Mat34, m_from_translation, m_inv_rigid, m_mul, m4_from34, m4_mul};

/// MW2 -> D3D view axes.
pub const M_AXIS: Mat4 = [[0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// Vertical FOV (radians) of an MW2 `cg_fov` value: `cg_fov` is the horizontal FOV of a 4:3 screen, so
/// `tan(vfov / 2) = tan(cg_fov / 2) * 0.75`, independent of the real aspect (Hor+).
pub fn vfov_from_cg_fov(cg_fov_deg: f32) -> f32 {
    2.0 * ((cg_fov_deg.to_radians() * 0.5).tan() * 0.75).atan()
}

/// Reversed-Z perspective: depth 1 at `near`, 0 at `far`; clip w = view z.
pub fn proj_reversed(tan_half_x: f32, tan_half_y: f32, near: f32, far: f32) -> Mat4 {
    let a = -near / (far - near);
    let b = near * far / (far - near);
    [
        [1.0 / tan_half_x, 0.0, 0.0, 0.0],
        [0.0, 1.0 / tan_half_y, 0.0, 0.0],
        [0.0, 0.0, a, b],
        [0.0, 0.0, 1.0, 0.0],
    ]
}

/// `M_AXIS * inverse(world[tag_view]) * T(offset)`: MW2 model space -> D3D view space.
pub fn view_matrix(tag_view_world: &Mat34, offset_in: [f32; 3]) -> Mat4 {
    let inv = m_mul(&m_inv_rigid(tag_view_world), &m_from_translation(&offset_in));
    m4_mul(&M_AXIS, &m4_from34(&inv))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjParams {
    pub hip_fov: f32,
    pub ads_fov: f32,
    pub near_in: f32,
    pub far_in: f32,
    pub offset_in: [f32; 3],
}

impl Default for ProjParams {
    fn default() -> Self {
        ProjParams { hip_fov: 65.0, ads_fov: 55.0, near_in: 0.1, far_in: 1000.0, offset_in: [0.0; 3] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewProj {
    pub view: Mat4,
    pub proj: Mat4,
    pub view_proj: Mat4,
    pub vfov_rad: f32,
}

/// The viewmodel's matrices for a `width` x `height` target. The FOV blends linearly in cg_fov degrees by the
/// ADS fraction (M3 simplification; adsZoomInFrac/OutFrac are M4).
pub fn build(tag_view_world: &Mat34, width: u32, height: u32, ads_frac: f32, p: &ProjParams) -> ViewProj {
    let aspect = if height > 0 { width.max(1) as f32 / height as f32 } else { 16.0 / 9.0 };
    let f = ads_frac.clamp(0.0, 1.0);
    let fov = p.hip_fov + (p.ads_fov - p.hip_fov) * f;
    let vfov = vfov_from_cg_fov(fov);
    let ty = (vfov * 0.5).tan();
    let proj = proj_reversed(ty * aspect, ty, p.near_in, p.far_in);
    let view = view_matrix(tag_view_world, p.offset_in);
    ViewProj { view, proj, view_proj: m4_mul(&proj, &view), vfov_rad: vfov }
}

/// NDC (x, y, depth) of a model-space point, or None behind the eye.
pub fn project(vp: &Mat4, p: &[f32; 3]) -> Option<[f32; 3]> {
    let c = crate::xform::m4_apply(vp, p);
    (c[3] > 1e-6).then(|| [c[0] / c[3], c[1] / c[3], c[2] / c[3]])
}

/// Pixel coordinates (top-left origin) of an NDC point on a `w` x `h` target.
pub fn ndc_to_pixel(ndc: [f32; 3], w: u32, h: u32) -> [f32; 2] {
    [(ndc[0] * 0.5 + 0.5) * w as f32, (0.5 - ndc[1] * 0.5) * h as f32]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm_assets::testdata;
    use crate::vm_pose::{PoseScratch, evaluate, static_state};
    use crate::vm_assets::ClipId;
    use crate::xform::{MAT34_IDENTITY, m_trs};

    #[test]
    fn vm_proj_vfov_numbers() {
        assert!((vfov_from_cg_fov(65.0).to_degrees() - 51.077).abs() < 0.01, "{}", vfov_from_cg_fov(65.0).to_degrees());
        assert!((vfov_from_cg_fov(55.0).to_degrees() - 42.654).abs() < 0.01);
        assert!((vfov_from_cg_fov(65.0) - 0.8915).abs() < 1e-3);
    }

    #[test]
    fn vm_proj_axis_center_and_depth_range() {
        let eye = m_trs(&[0.0, 0.0, 60.0], &[0.0, 0.0, 0.0, 1.0]);
        let vp = build(&eye, 1920, 1080, 0.0, &ProjParams::default());
        let c = project(&vp.view_proj, &[10.0, 0.0, 60.0]).unwrap();
        assert!(c[0].abs() < 1e-6 && c[1].abs() < 1e-6, "{c:?}");
        let near = project(&vp.view_proj, &[0.1, 0.0, 60.0]).unwrap();
        let far = project(&vp.view_proj, &[1000.0, 0.0, 60.0]).unwrap();
        assert!((near[2] - 1.0).abs() < 1e-5 && far[2].abs() < 1e-5, "{near:?} {far:?}");
        // MW2 left (+Y) is screen left (-x); MW2 up (+Z) is screen up (+y)
        let l = project(&vp.view_proj, &[10.0, 1.0, 61.0]).unwrap();
        assert!(l[0] < 0.0 && l[1] > 0.0);
        // the top edge of the frustum is at vfov / 2
        let t = (vp.vfov_rad * 0.5).tan() * 10.0;
        let top = project(&vp.view_proj, &[10.0, 0.0, 60.0 + t]).unwrap();
        assert!((top[1] - 1.0).abs() < 1e-4);
        // axis map is a reflection
        let m = M_AXIS;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        assert_eq!(det, -1.0);
        let _ = MAT34_IDENTITY;
    }

    #[test]
    fn vm_proj_flash_lower_right_at_idle() {
        let a = testdata::assets();
        let mut s = PoseScratch::new(a.skel.bones.len());
        let (st, now) = static_state(ClipId::Idle, 0.0, 0.0);
        evaluate(&a, &st, now, &mut s);
        let vp = build(&s.world[a.skel.tags.view as usize], 1920, 1080, 0.0, &ProjParams::default());
        let f = crate::xform::m_translation(&s.world[a.skel.tags.flash as usize]);
        let n = project(&vp.view_proj, &f).unwrap();
        assert!(n[0] > 0.0 && n[1] < 0.0, "tag_flash at idle projects lower right: {n:?}");
        let px = ndc_to_pixel(n, 1920, 1080);
        assert!(px[0] > 960.0 && px[1] > 540.0);
    }
}
