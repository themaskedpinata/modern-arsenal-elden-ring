//! Camera axes of the movement wish and the aim's chest reference while the controller drives
//! (docs/M5_DESIGN.md sections 1.4 and 8.5).

use crate::vec3::{UP, V3};

/// Horizontal forward and right of ER's camera (row 2 = forward, the M3 sign +1). `right = UP x forward`, the
/// convention the AC-130 frames use (run 9: tracers projected onto the screen where they were seen), so +right
/// projects to +x on screen. `strafe_sign` (-1) flips it as a hot override if the user's run shows otherwise.
pub fn axes(row2: V3, strafe_sign: i8) -> Option<(V3, V3)> {
    let f = V3::new(row2.x, 0.0, row2.z).normalize()?;
    let r = UP.cross(f).normalize()?;
    Some((f, if strafe_sign < 0 { -r } else { r }))
}

/// The chest reference of the M2/M3 shot checks while the controller drives: a constant `stand_eye - chest_height`
/// below the current eye, so crouched and prone shots keep the standing eye-to-chest geometry (0.40 m with the
/// defaults) instead of failing the 0.3 m camera-to-chest minimum.
pub fn chest_ref(eye: V3, stand_eye_m: f32, chest_height_m: f32) -> V3 {
    eye - V3::new(0.0, stand_eye_m - chest_height_m, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac130_cam::rows_for;
    use crate::ac130_hud::project;
    use crate::aim::{CAM_DIST_MIN_M, cam_dist_plausible};

    /// A real CAMFRAME of ER's third-person camera (tools\scripts\fixtures\m3_run3_session.log, tick 463, det +1).
    fn camframe() -> (V3, V3) {
        (V3::new(0.65, 2.68, 5.86), V3::new(0.960, -0.185, -0.212))
    }

    #[test]
    fn move_axes_right_projects_to_screen_right() {
        let (pos, fwd) = camframe();
        let rows = rows_for(fwd).unwrap();
        let (fh, rh) = axes(fwd, 1).unwrap();
        let base = pos + fwd.normalize().unwrap() * 5.0;
        let rr = [[rows[0].x, rows[0].y, rows[0].z], [rows[1].x, rows[1].y, rows[1].z], [rows[2].x, rows[2].y, rows[2].z]];
        let c = project([pos.x, pos.y, pos.z], rr, 0.8378, 1920.0, 1080.0, [base.x, base.y, base.z]).unwrap();
        let p = base + rh;
        let s = project([pos.x, pos.y, pos.z], rr, 0.8378, 1920.0, 1080.0, [p.x, p.y, p.z]).unwrap();
        assert!(s[0] > c[0] + 50.0, "right_h lands right of the centre: {c:?} -> {s:?}");
        // W moves along the camera's horizontal forward
        assert!(fh.y == 0.0 && fh.dot(V3::new(fwd.x, 0.0, fwd.z)) > 0.99 * V3::new(fwd.x, 0.0, fwd.z).len());
        assert!(rh.dot(fh).abs() < 1e-6 && rh.y == 0.0);
    }

    #[test]
    fn move_axes_strafe_sign_flips() {
        let (_, fwd) = camframe();
        let (_, r1) = axes(fwd, 1).unwrap();
        let (_, r2) = axes(fwd, -1).unwrap();
        assert_eq!(r1, -r2);
        assert!(axes(V3::new(0.0, 1.0, 0.0), 1).is_none(), "looking straight up: no heading");
    }

    #[test]
    fn move_aim_chest_crouch_and_prone_keep_distance() {
        for eye_h in [1.60f32, 1.067, 0.293] {
            let eye = V3::new(10.0, 5.0 + eye_h, -3.0);
            let chest = chest_ref(eye, 1.60, 1.2);
            let d = (eye - chest).len();
            assert!((d - 0.40).abs() < 1e-5, "{eye_h}: {d}");
            assert!(d >= CAM_DIST_MIN_M && cam_dist_plausible(d));
        }
        // the old fixed chest (feet + 1.2 m) fails for a crouched eye: the bug section 8.5 fixes
        let feet = V3::new(0.0, 0.0, 0.0);
        let crouch_eye = V3::new(0.0, 1.067, 0.0);
        assert!((crouch_eye - (feet + V3::new(0.0, 1.2, 0.0))).len() < CAM_DIST_MIN_M);
    }
}
