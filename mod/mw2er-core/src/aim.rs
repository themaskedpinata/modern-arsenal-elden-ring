//! Aim geometry on plain samples (docs/M2_DESIGN.md section 10.2).
//!
//! The camera matrix row 2 is the forward candidate; its sign is resolved by requiring the
//! third-person camera to look *toward* the player's chest. The bullet starts on the crosshair ray
//! just past the player, so the crosshair and the bullet path agree (no parallax).

use crate::vec3::V3;

/// Below this |dot| (metres along the view ray) the sign is not re-decided.
pub const SIGN_DEADZONE_M: f32 = 0.2;
/// Never start the ray closer than this to the camera.
pub const MIN_T0_M: f32 = 0.5;

/// Outcome of a camera sign update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignUpdate {
    /// Still unresolved (player too close to the camera plane).
    Unknown,
    /// First resolution.
    Resolved(f32),
    /// The sign flipped.
    Flipped(f32),
    Unchanged(f32),
}

/// Tracks the sign that turns matrix row 2 into the view direction.
#[derive(Clone, Copy, Debug, Default)]
pub struct CamSignTracker {
    sign: Option<f32>,
}

impl CamSignTracker {
    pub fn sign(&self) -> Option<f32> {
        self.sign
    }

    pub fn reset(&mut self) {
        self.sign = None;
    }

    pub fn update(&mut self, row2: V3, cam_pos: V3, chest: V3) -> SignUpdate {
        let Some(f) = row2.normalize() else {
            return self.sign.map_or(SignUpdate::Unknown, SignUpdate::Unchanged);
        };
        let d = f.dot(chest - cam_pos);
        if d.abs() < SIGN_DEADZONE_M || !d.is_finite() {
            return self.sign.map_or(SignUpdate::Unknown, SignUpdate::Unchanged);
        }
        let new = d.signum();
        match self.sign.replace(new) {
            None => SignUpdate::Resolved(new),
            Some(old) if old != new => SignUpdate::Flipped(new),
            Some(_) => SignUpdate::Unchanged(new),
        }
    }
}

/// View direction from row 2 and a resolved sign.
pub fn view_forward(row2: V3, sign: f32) -> Option<V3> {
    row2.normalize().map(|f| f * sign)
}

/// Bullet origin on the crosshair ray, `origin_ahead_m` past the player's depth along the ray.
pub fn bullet_origin(cam_pos: V3, fwd: V3, chest: V3, origin_ahead_m: f32) -> V3 {
    let t0 = fwd.dot(chest - cam_pos).max(MIN_T0_M);
    cam_pos + fwd * (t0 + origin_ahead_m)
}

/// Plausible camera-to-chest distance for ER's third-person camera. Outside it the camera and the
/// player are probably read in different coordinate spaces (unverified layout), so no shot is fired.
pub const CAM_DIST_MIN_M: f32 = 0.3;
pub const CAM_DIST_MAX_M: f32 = 15.0;
/// The bullet origin may lie at most `origin_ahead_m + ORIGIN_SLACK_M` from the player's chest.
pub const ORIGIN_SLACK_M: f32 = 3.0;

pub fn cam_dist_plausible(d: f32) -> bool {
    d.is_finite() && (CAM_DIST_MIN_M..=CAM_DIST_MAX_M).contains(&d)
}

/// Why a shot's engine inputs were rejected (the shot is logged as `SHOT ... skipped: <reason>`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShotReject {
    NonFinite,
    CameraDistance(f32),
    OriginFar(f32),
    BadDirection,
    BadRange,
}

impl ShotReject {
    pub fn reason(self) -> String {
        match self {
            ShotReject::NonFinite => "non-finite camera/player/origin/direction".into(),
            ShotReject::CameraDistance(d) => {
                format!("cam_player_dist={d:.2}m outside {CAM_DIST_MIN_M}..{CAM_DIST_MAX_M} m (camera space unverified)")
            }
            ShotReject::OriginFar(d) => format!("origin {d:.2}m from the player's chest (too far)"),
            ShotReject::BadDirection => "direction is not a unit vector".into(),
            ShotReject::BadRange => "ray range is not a positive finite number".into(),
        }
    }
}

/// Sanity check of everything that is handed to the engine's ray cast and `spawn_bullet` for one
/// shot. Rejects non-finite values, a camera that is implausibly far from the player, an origin far
/// from the player, a non-unit direction and a bad ray range.
pub fn check_shot(
    cam_pos: V3,
    chest: V3,
    origin: V3,
    dir: V3,
    origin_ahead_m: f32,
    range_m: f32,
) -> Result<(), ShotReject> {
    check_shot_fp(cam_pos, chest, origin, dir, origin_ahead_m, range_m, false)
}

/// [`check_shot`] with run 18's first-person rule: while the first-person eye is the bullet origin (`fp_eye`), ER's
/// third-person camera may sit closer than [`CAM_DIST_MIN_M`] to the chest (its collision pulled it in against an
/// obstacle); only the upper bound (a camera in another space) still applies. The origin-to-chest rule is unchanged.
pub fn check_shot_fp(
    cam_pos: V3,
    chest: V3,
    origin: V3,
    dir: V3,
    origin_ahead_m: f32,
    range_m: f32,
    fp_eye: bool,
) -> Result<(), ShotReject> {
    if !(cam_pos.is_finite() && chest.is_finite() && origin.is_finite() && dir.is_finite()) {
        return Err(ShotReject::NonFinite);
    }
    if !(range_m.is_finite() && range_m > 0.0) {
        return Err(ShotReject::BadRange);
    }
    let cam_d = (chest - cam_pos).len();
    let cam_ok = if fp_eye { cam_d.is_finite() && cam_d <= CAM_DIST_MAX_M } else { cam_dist_plausible(cam_d) };
    if !cam_ok {
        return Err(ShotReject::CameraDistance(cam_d));
    }
    let od = (origin - chest).len();
    if !(od <= origin_ahead_m.max(0.0) + ORIGIN_SLACK_M) {
        return Err(ShotReject::OriginFar(od));
    }
    if !((dir.len() - 1.0).abs() <= 1e-3) {
        return Err(ShotReject::BadDirection);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistSource {
    Chr,
    Map,
    None,
}

impl DistSource {
    pub fn label(self) -> &'static str {
        match self {
            DistSource::Chr => "chr",
            DistSource::Map => "map",
            DistSource::None => "none",
        }
    }
}

/// Distance along the shot to the first thing it is expected to hit.
///
/// `chests` are character chest points (player and excluded characters already removed);
/// a character counts when its chest is in front of the origin, within `max_range_m`, and within
/// `char_radius_m` of the ray. `map_t` is the map ray-cast hit distance, if any. With no
/// candidate the result is `max_range_m` (the minimum-damage band).
pub fn estimate_target_distance(
    origin: V3,
    dir: V3,
    chests: &[V3],
    map_t: Option<f32>,
    max_range_m: f32,
    char_radius_m: f32,
) -> (f32, DistSource) {
    let Some(dir) = dir.normalize() else { return (max_range_m, DistSource::None) };
    let mut best: Option<(f32, DistSource)> = None;
    for &p in chests {
        let v = p - origin;
        let t = v.dot(dir);
        if !(t > 0.0 && t <= max_range_m) {
            continue;
        }
        let perp = (v - dir * t).len();
        if perp < char_radius_m && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, DistSource::Chr));
        }
    }
    if let Some(mt) = map_t
        && mt.is_finite()
        && mt > 0.0
        && mt <= max_range_m
        && best.is_none_or(|(bt, _)| mt < bt)
    {
        best = Some((mt, DistSource::Map));
    }
    best.unwrap_or((max_range_m, DistSource::None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aim_sign_resolves_toward_player() {
        let mut s = CamSignTracker::default();
        let cam = V3::new(0.0, 2.0, -4.0);
        let chest = V3::new(0.0, 1.2, 0.0);
        // row2 pointing away from the player -> sign -1
        assert_eq!(s.update(V3::new(0.0, 0.0, -1.0), cam, chest), SignUpdate::Resolved(-1.0));
        assert_eq!(s.update(V3::new(0.0, 0.0, -1.0), cam, chest), SignUpdate::Unchanged(-1.0));
        assert_eq!(s.update(V3::new(0.0, 0.0, 1.0), cam, chest), SignUpdate::Flipped(1.0));
        // inside the dead zone the last sign is kept
        assert_eq!(s.update(V3::new(1.0, 0.0, 0.0), cam, V3::new(0.1, 2.0, -4.0)), SignUpdate::Unchanged(1.0));
        let fwd = view_forward(V3::new(0.0, 0.0, 2.0), -1.0).unwrap();
        assert_eq!(fwd, V3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn aim_origin_is_past_the_player_on_the_ray() {
        let cam = V3::new(0.0, 2.0, -4.0);
        let fwd = V3::new(0.0, 0.0, 1.0);
        let o = bullet_origin(cam, fwd, V3::new(0.0, 1.2, 0.0), 0.6);
        assert!((o.z - 0.6).abs() < 1e-5 && (o.y - 2.0).abs() < 1e-6);
        // player behind the camera plane: never closer than MIN_T0
        let o2 = bullet_origin(cam, fwd, V3::new(0.0, 1.2, -10.0), 0.6);
        assert!((o2.z - (-4.0 + 1.1)).abs() < 1e-5);
    }

    #[test]
    fn aim_shot_sanity_check() {
        let cam = V3::new(0.0, 2.0, -4.0);
        let chest = V3::new(0.0, 1.2, 0.0);
        let fwd = V3::new(0.0, 0.0, 1.0);
        let origin = bullet_origin(cam, fwd, chest, 0.6);
        assert_eq!(check_shot(cam, chest, origin, fwd, 0.6, 150.0), Ok(()));
        // camera in another space (hundreds of metres away): the origin would be far from the player
        let far_cam = V3::new(400.0, 2.0, -4.0);
        assert!(matches!(check_shot(far_cam, chest, origin, fwd, 0.6, 150.0), Err(ShotReject::CameraDistance(_))));
        // camera inside the player
        assert!(matches!(check_shot(chest, chest, origin, fwd, 0.6, 150.0), Err(ShotReject::CameraDistance(_))));
        // origin far from the chest
        let o2 = V3::new(0.0, 1.2, 9.0);
        assert!(matches!(check_shot(cam, chest, o2, fwd, 0.6, 150.0), Err(ShotReject::OriginFar(_))));
        // non-finite inputs
        let inf = V3::new(f32::INFINITY, 0.0, 0.0);
        assert_eq!(check_shot(cam, chest, inf, fwd, 0.6, 150.0), Err(ShotReject::NonFinite));
        let o_inf = bullet_origin(cam, fwd, chest, f32::INFINITY);
        assert_eq!(check_shot(cam, chest, o_inf, fwd, 0.6, 150.0), Err(ShotReject::NonFinite));
        assert_eq!(check_shot(cam, chest, origin, V3::new(0.0, 0.0, f32::NAN), 0.6, 150.0), Err(ShotReject::NonFinite));
        // bad direction / range
        assert_eq!(check_shot(cam, chest, origin, V3::new(0.0, 0.0, 2.0), 0.6, 150.0), Err(ShotReject::BadDirection));
        assert_eq!(check_shot(cam, chest, origin, fwd, 0.6, f32::INFINITY), Err(ShotReject::BadRange));
        assert_eq!(check_shot(cam, chest, origin, fwd, 0.6, 0.0), Err(ShotReject::BadRange));
        assert!(cam_dist_plausible(3.8) && !cam_dist_plausible(0.1) && !cam_dist_plausible(f32::NAN));
        assert!(!ShotReject::CameraDistance(400.0).reason().is_empty());
    }

    #[test]
    fn aim_shot_fp_eye_accepts_a_close_er_camera() {
        // run 18: against a wagon ER's camera sat 0.29 m (down to 0.08 m) from the chest; in first person the bullet
        // leaves from the eye, so that is not a space mismatch
        let chest = V3::new(5.4, 9.6, 27.4);
        let eye = V3::new(5.4, 9.85, 27.4);
        let fwd = V3::new(1.0, 0.0, 0.0);
        let cam = V3::new(5.62, 9.7, 27.4);
        let origin = bullet_origin(eye, fwd, chest, 0.6);
        assert!(matches!(check_shot(cam, chest, origin, fwd, 0.6, 150.0), Err(ShotReject::CameraDistance(_))), "third person keeps 0.3 m");
        assert_eq!(check_shot_fp(cam, chest, origin, fwd, 0.6, 150.0, true), Ok(()));
        assert_eq!(check_shot_fp(chest, chest, origin, fwd, 0.6, 150.0, true), Ok(()), "0 m");
        // still rejected in first person: another space, a far origin, non-finite, a bad direction
        let far = V3::new(400.0, 9.7, 27.4);
        assert!(matches!(check_shot_fp(far, chest, origin, fwd, 0.6, 150.0, true), Err(ShotReject::CameraDistance(_))));
        assert!(matches!(check_shot_fp(cam, chest, V3::new(5.4, 9.6, 37.0), fwd, 0.6, 150.0, true), Err(ShotReject::OriginFar(_))));
        assert_eq!(check_shot_fp(cam, chest, origin, V3::new(0.0, f32::NAN, 0.0), 0.6, 150.0, true), Err(ShotReject::NonFinite));
        assert_eq!(check_shot_fp(cam, chest, origin, V3::new(0.0, 0.0, 2.0), 0.6, 150.0, true), Err(ShotReject::BadDirection));
        // the origin from the eye stays in front of the chest (MIN_T0) even with the camera at the chest
        assert!((origin - chest).len() <= 0.6 + ORIGIN_SLACK_M);
    }

    #[test]
    fn aim_distance_estimate() {
        let o = V3::ZERO;
        let dir = V3::new(0.0, 0.0, 1.0);
        let chests = [V3::new(0.3, 0.0, 12.0), V3::new(0.0, 0.0, 30.0), V3::new(5.0, 0.0, 8.0), V3::new(0.0, 0.0, -3.0)];
        assert_eq!(estimate_target_distance(o, dir, &chests, None, 150.0, 0.75), (12.0, DistSource::Chr));
        assert_eq!(estimate_target_distance(o, dir, &chests, Some(9.0), 150.0, 0.75), (9.0, DistSource::Map));
        assert_eq!(estimate_target_distance(o, dir, &[], None, 150.0, 0.75), (150.0, DistSource::None));
        assert_eq!(estimate_target_distance(o, dir, &[], Some(500.0), 150.0, 0.75), (150.0, DistSource::None));
    }
}
