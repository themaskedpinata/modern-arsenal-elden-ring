//! C6 depth (work/c6/depth3.md 3, the no-GPU-access fallback; docs/C6_HOLD_DESIGN.md part 13): depth-only capsules on a
//! character's body bones, drawn into OUR world-gun depth before the world guns, so a gun behind the body (or behind the
//! back-carried shield) is hidden there. `[coop] world_gun_body_occlusion` (default false).
//!
//! Why not ER's scene depth: depth3.md option 3 (copy ER's DepthTexture at UiBegin) needs the depth buffer's resource
//! state at that point, which only a probe-only ResourceBarrier hook can measure (an open decision); the read-only part of
//! that probe is built (`er_depth`). Capsules need nothing of ER's GPU: the bone array is already read world-space for the
//! hand (`hold`), and a capsule can only hide what lies BEHIND it from the camera, so the radii are kept inside the body
//! (a slightly thin body lets a sliver of a gun show through; it never cuts a gun held in front).
//!
//! Pure: the DLL resolves [`BODY_SLOTS`] to FLVER indices once (the havok names, as `hand_bone_auto`), reads the entries
//! each frame and calls [`capsules`].

use crate::vec3::{UP, V3};

/// The skeleton names looked up (ER c0000 havok skeleton [I: the FromSoftware humanoid names; R_Weapon / R_Hand were
/// found in the 17:31 run]). Slot index = position here.
pub const BODY_SLOTS: [&str; 17] = [
    "Pelvis",     // 0
    "Spine",      // 1
    "Spine1",     // 2
    "Spine2",     // 3
    "Neck",       // 4
    "Head",       // 5
    "L_UpperArm", // 6
    "L_Forearm",  // 7
    "L_Hand",     // 8
    "R_UpperArm", // 9
    "R_Forearm",  // 10
    "L_Thigh",    // 11
    "L_Calf",     // 12
    "L_Foot",     // 13
    "R_Thigh",    // 14
    "R_Calf",     // 15
    "R_Foot",     // 16
];

/// One body capsule: the first found slot of `from` to the first found of `to` (+ `extend_m` beyond `to` along the
/// segment), radius `r`; `back_m` > 0 shifts both ends that far behind the character (against its horizontal facing)
/// and `side_m` sideways (the back-carried shield). The right forearm / hand are never occluders: they hold the gun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapsuleSpec {
    pub name: &'static str,
    pub from: &'static [usize],
    pub to: &'static [usize],
    pub r: f32,
    pub extend_m: f32,
    pub back_m: f32,
    pub side_m: f32,
}

pub const CAPSULES: [CapsuleSpec; 14] = [
    CapsuleSpec { name: "hips", from: &[0], to: &[2, 1, 3], r: 0.11, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "chest", from: &[2, 1, 0], to: &[4, 3], r: 0.13, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "head", from: &[4], to: &[5], r: 0.09, extend_m: 0.12, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "l_upperarm", from: &[6], to: &[7], r: 0.045, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "l_forearm", from: &[7], to: &[8], r: 0.04, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "r_upperarm", from: &[9], to: &[10], r: 0.045, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "l_thigh", from: &[11], to: &[12], r: 0.065, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "l_calf", from: &[12], to: &[13], r: 0.05, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "r_thigh", from: &[14], to: &[15], r: 0.065, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    CapsuleSpec { name: "r_calf", from: &[15], to: &[16], r: 0.05, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
    // the back (a shield / weapon carried there): behind the spine only, its front surface stays inside the torso
    CapsuleSpec { name: "back", from: &[2, 1, 0], to: &[4, 3], r: 0.12, extend_m: 0.0, back_m: 0.14, side_m: 0.0 },
    CapsuleSpec { name: "back_l", from: &[2, 1, 0], to: &[4, 3], r: 0.11, extend_m: 0.0, back_m: 0.13, side_m: 0.12 },
    CapsuleSpec { name: "back_r", from: &[2, 1, 0], to: &[4, 3], r: 0.11, extend_m: 0.0, back_m: 0.13, side_m: -0.12 },
    // shoulders (the chest capsule ends at the neck: the clavicle span)
    CapsuleSpec { name: "shoulders", from: &[6], to: &[9], r: 0.07, extend_m: 0.0, back_m: 0.0, side_m: 0.0 },
];

/// One capsule for the renderer (ER world metres).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub r: f32,
}

/// At most this many capsules per frame (every player's body); the renderer's constant block holds them.
pub const MAX_CAPSULES: usize = 64;
/// A body point farther than this from the feet is not a body point (a torn / wrong read).
pub const BODY_MAX_M: f32 = 2.5;
/// A body segment longer than this is not a limb.
pub const SEG_MAX_M: f32 = 1.0;
/// The hips -> chest -> head chain must rise (world up) at least this much from the pelvis to the head: else the slot
/// lookup matched the wrong bones (or the character lies down: no capsules then, the guns draw as before).
pub const MIN_TORSO_RISE_M: f32 = 0.15;

fn first(points: &[Option<V3>], slots: &[usize]) -> Option<V3> {
    slots.iter().find_map(|&k| points.get(k).copied().flatten())
}

fn arr(v: V3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// Why a player has no capsules this frame (the log token).
pub fn reject_label(r: Reject) -> &'static str {
    match r {
        Reject::NoPelvis => "no_pelvis",
        Reject::Far => "far",
        Reject::Torso => "torso",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    NoPelvis,
    Far,
    Torso,
}

/// The body capsules of one character from its slot points (world, [`BODY_SLOTS`] order, `None` = not resolved /
/// unreadable), its feet and its horizontal facing (`None`: no back capsules). Validation: the pelvis and a torso top
/// exist, every point used is finite and within [`BODY_MAX_M`] of the feet, the torso rises [`MIN_TORSO_RISE_M`];
/// a single capsule with a missing end / too long / degenerate is dropped alone.
pub fn capsules(points: &[Option<V3>], feet: V3, facing: Option<V3>) -> Result<Vec<Capsule>, Reject> {
    let pelvis = first(points, &[0]).ok_or(Reject::NoPelvis)?;
    let ok = |p: V3| p.is_finite() && (p - feet).len() <= BODY_MAX_M;
    if !ok(pelvis) {
        return Err(Reject::Far);
    }
    let top = first(points, &[5, 4, 3]).ok_or(Reject::Torso)?;
    if !ok(top) || (top - pelvis).dot(UP) < MIN_TORSO_RISE_M {
        return Err(Reject::Torso);
    }
    let back = facing.and_then(|f| V3::new(f.x, 0.0, f.z).normalize()).map(|f| f * -1.0);
    let side = back.and_then(|b| b.cross(UP).normalize());
    let mut out = Vec::with_capacity(CAPSULES.len());
    for c in CAPSULES {
        let (Some(a), Some(b)) = (first(points, c.from), first(points, c.to)) else { continue };
        if !ok(a) || !ok(b) {
            continue;
        }
        let d = b - a;
        let len = d.len();
        if !(len > 0.01 && len <= SEG_MAX_M) {
            continue;
        }
        let b = b + d * (c.extend_m / len);
        let shift = if c.back_m > 0.0 || c.side_m != 0.0 {
            let (Some(bk), Some(sd)) = (back, side) else { continue };
            bk * c.back_m + sd * c.side_m
        } else {
            V3::ZERO
        };
        out.push(Capsule { a: arr(a + shift), b: arr(b + shift), r: c.r });
    }
    Ok(out)
}

/// The capsules' names that resolved (for the `HOLD body ok` line).
pub fn resolved_text(points: &[Option<V3>]) -> String {
    BODY_SLOTS.iter().enumerate().filter(|(k, _)| points.get(*k).copied().flatten().is_some()).map(|(_, n)| *n).collect::<Vec<_>>().join(",")
}

/// Distance from `p` to the capsule's surface (negative inside).
pub fn capsule_sdf(c: &Capsule, p: V3) -> f32 {
    let a = V3::new(c.a[0], c.a[1], c.a[2]);
    let b = V3::new(c.b[0], c.b[1], c.b[2]);
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).len() - c.r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standing c0000-like pose (feet at `o`, facing +z): heights from a 1.75 m character.
    fn standing(o: V3) -> Vec<Option<V3>> {
        let p = |x: f32, y: f32, z: f32| Some(o + V3::new(x, y, z));
        vec![
            p(0.0, 1.00, 0.0),   // Pelvis
            p(0.0, 1.10, 0.0),   // Spine
            p(0.0, 1.25, 0.0),   // Spine1
            p(0.0, 1.38, 0.0),   // Spine2
            p(0.0, 1.52, 0.0),   // Neck
            p(0.0, 1.62, 0.0),   // Head
            p(0.18, 1.45, 0.0),  // L_UpperArm
            p(0.22, 1.18, 0.0),  // L_Forearm
            p(0.24, 0.95, 0.05), // L_Hand
            p(-0.18, 1.45, 0.0), // R_UpperArm
            p(-0.22, 1.18, 0.0), // R_Forearm
            p(0.10, 0.95, 0.0),  // L_Thigh
            p(0.11, 0.52, 0.02), // L_Calf
            p(0.12, 0.08, 0.0),  // L_Foot
            p(-0.10, 0.95, 0.0), // R_Thigh
            p(-0.11, 0.52, 0.02),
            p(-0.12, 0.08, 0.0),
        ]
    }

    #[test]
    fn c6_body_capsules_cover_the_body_and_spare_the_gun() {
        let o = V3::new(-7.35, 6.42, -3.96);
        let pts = standing(o);
        assert_eq!(pts.len(), BODY_SLOTS.len());
        let caps = capsules(&pts, o, Some(V3::new(0.0, 0.0, 1.0))).unwrap();
        assert_eq!(caps.len(), CAPSULES.len());
        let inside = |p: V3| caps.iter().any(|c| capsule_sdf(c, p) < 0.0);
        // the chest, the head, the back (where a shield hangs) and the thighs are covered
        for (what, p) in [
            ("chest", V3::new(0.0, 1.35, 0.0)),
            ("head", V3::new(0.0, 1.70, 0.0)),
            ("back shield", V3::new(0.0, 1.35, -0.20)),
            ("back shield left", V3::new(0.15, 1.30, -0.18)),
            ("thigh", V3::new(0.10, 0.75, 0.0)),
        ] {
            assert!(inside(o + p), "{what} not covered");
        }
        // a gun in the ER crossbow carry (fist at the right hip, ~0.3 m out to the side and ahead, barrel forward):
        // none of its points is inside a capsule (the right forearm / hand are never occluders)
        for k in 0..12 {
            let p = V3::new(-0.30, 0.95, 0.10 + 0.05 * k as f32);
            assert!(!inside(o + p), "gun point {k} cut");
        }
        // the front of the chest: a gun held 15 cm in front of the sternum is never cut by the back capsules
        assert!(!inside(o + V3::new(0.0, 1.30, 0.15)));
        // no facing: the back capsules are dropped, the rest kept
        assert_eq!(capsules(&pts, o, None).unwrap().len(), CAPSULES.len() - 3);
        assert!(resolved_text(&pts).starts_with("Pelvis,Spine,Spine1"));
    }

    #[test]
    fn c6_body_capsules_validation() {
        let o = V3::new(10.0, 2.0, 3.0);
        let pts = standing(o);
        // no pelvis / far / a torso that does not rise (lying down or the wrong bones): no capsules at all
        let mut p = pts.clone();
        p[0] = None;
        assert_eq!(capsules(&p, o, None), Err(Reject::NoPelvis));
        assert_eq!(capsules(&pts, o + V3::new(5.0, 0.0, 0.0), None), Err(Reject::Far));
        let mut p = pts.clone();
        for k in [3, 4, 5] {
            p[k] = Some(o + V3::new(0.5, 1.0, 0.0));
        }
        assert_eq!(capsules(&p, o, None), Err(Reject::Torso));
        // missing names: the capsule falls to an alternate end, or is dropped alone
        let mut p = pts.clone();
        p[2] = None; // Spine1 missing: hips end at Spine, chest starts at Spine
        p[16] = None; // R_Foot missing: no r_calf
        let caps = capsules(&p, o, None).unwrap();
        assert_eq!(caps.len(), CAPSULES.len() - 3 - 1);
        assert!(caps.iter().all(|c| c.a.iter().chain(&c.b).all(|v| v.is_finite()) && c.r > 0.0));
        // one torn point (NaN / 3 m away) drops only its capsules
        let mut p = pts.clone();
        p[12] = Some(V3::new(f32::NAN, 0.0, 0.0));
        assert_eq!(capsules(&p, o, None).unwrap().len(), CAPSULES.len() - 3 - 2);
        let mut p = pts;
        p[8] = Some(o + V3::new(3.0, 0.0, 0.0));
        assert_eq!(capsules(&p, o, None).unwrap().len(), CAPSULES.len() - 3 - 1);
        assert_eq!(reject_label(Reject::Torso), "torso");
    }
}
