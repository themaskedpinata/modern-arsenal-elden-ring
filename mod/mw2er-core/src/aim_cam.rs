//! C6 round 6 (work/c6/aim6.md, docs/C6_HOLD_DESIGN.md 14.8): keep ER's crossbow aim BODY animation, drop ER's aim
//! camera and its crossbow reticle.
//!
//! Everything in ER's precision aim keys off one bit, `ChrIns+0x1C5 bit 1` (`precision_shooting`), read through a
//! 12-byte leaf getter called from 24 places [V eldenring.exe 2.7.2.0, read as a file]. Two of those callers turn the
//! user-visible "crossbow view" on: `ChrCam::Update` (its only getter call drives the camera-type switch at
//! `ChrCam+0x7C`: 0 follow -> 1 aim blend -> 2 aim) and the FeMan reticle update (`CSFeManImp+0x3710 = 2`, the
//! crossbow reticle, only for the main player with the bit set). The others (HKS `env()`, the input handler's body turn,
//! FE / draw) must keep seeing the bit, or the aim animation drops [I strong].
//!
//! So the DLL (`mw2er/src/aim_cam.rs`) hooks the getter with a naked detour that returns 0 ONLY for those return
//! addresses, only for our own main PlayerIns, only while armed; every other call goes to the original. ER's bit is
//! never written. This module holds the 2.7.2.0 addresses, the install-time validation of the bytes and call sites
//! (pure: the DLL hands in a non-faulting reader) and the arming rule.
//!
//! C6 round 7 (work/c6/aim7.md 1, docs/C6_HOLD_DESIGN.md 14.11): the round-6 test's invisible body is ER's own
//! "snipe mode" draw-alpha fade (`PlayerIns+0x5D8`, `snipe_mode_draw_alpha_fade_timer`), run down by the PlayerIns
//! update while the bit is set (the DS3-style "aim = first-person view" hide) and multiplied into the body's draw alpha
//! and the attached parts' visibility. Its writer reads the bit through a third getter call (0x14065B3DB): the same
//! detour masks that return address too (`RET_FADE`), so the timer runs back to 1 and the body stays drawn. `+0x5D8` is
//! never written (ER rewrites it every frame).

/// The addresses of one exe build (RVAs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AimCamAddrs {
    /// `IsPrecisionShooting(ChrIns*)`: `movzx eax, byte [rcx+1C5h]; shr al,1; and al,1; ret`.
    pub getter: usize,
    pub getter_bytes: &'static [u8],
    /// `ChrCam::Update` and its getter call (`E8 rel32`; the return address is `+5`).
    pub chrcam_update: usize,
    pub chrcam_prologue: &'static [u8],
    pub chrcam_call: usize,
    /// The FeMan reticle update and its getter call.
    pub reticle_update: usize,
    pub reticle_prologue: &'static [u8],
    pub reticle_call: usize,
    /// C6 round 7: the snipe-fade writer in the PlayerIns update: `fade_site` = the 33 bytes from its enable test to the
    /// branch on the second condition (`fade_bytes`), `fade_call` = its getter call inside them, `fade_stores` = the two
    /// `movss [rdi+5D8h], xmm0` timer stores ([`FADE_STORE_2720`]: the purpose pin).
    pub fade_site: usize,
    pub fade_bytes: &'static [u8],
    pub fade_call: usize,
    pub fade_stores: [usize; 2],
}

/// The getter's 12 bytes on 2.7.2.0 (0x1403F4A90).
pub const GETTER_2720: [u8; 12] = [0x0f, 0xb6, 0x81, 0xc5, 0x01, 0x00, 0x00, 0xd0, 0xe8, 0x24, 0x01, 0xc3];
/// `ChrCam::Update` (0x1403B11D0): `mov r11,rsp; push rbp; push rsi; push rdi; push r12; push r13; push r14`.
pub const CHRCAM_PROLOGUE_2720: [u8; 12] = [0x4c, 0x8b, 0xdc, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56];
/// The reticle update (0x1407725C0): `push rbx; sub rsp,20h; movss xmm0,[rip+..]`.
pub const RETICLE_PROLOGUE_2720: [u8; 10] = [0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0xf3, 0x0f, 0x10, 0x05];

/// The snipe-fade writer on 2.7.2.0 (0x14065B3CF): `cmp byte [0x143B1EFD8],0; je; mov rcx,rdi; call getter; test al,al;
/// jne; mov rcx,rdi; call 0x1403F4E10; test al,al; je` [V exe file, work/c6/aim7.md 1].
pub const FADE_SITE_2720: [u8; 33] = [
    0x80, 0x3d, 0x02, 0x3c, 0x4c, 0x03, 0x00, 0x74, 0x55, 0x48, 0x8b, 0xcf, 0xe8, 0xb0, 0x96, 0xd9, 0xff, 0x84, 0xc0, 0x75, 0x0c, 0x48, 0x8b, 0xcf, 0xe8, 0x24,
    0x9a, 0xd9, 0xff, 0x84, 0xc0, 0x74, 0x3d,
];
/// `movss dword ptr [rdi+5D8h], xmm0` (the timer store, at 0x14065B417 and 0x14065B451) [V exe file].
pub const FADE_STORE_2720: [u8; 8] = [0xf3, 0x0f, 0x11, 0x87, 0xd8, 0x05, 0x00, 0x00];
/// `PlayerIns+0x5D8`: the snipe-mode draw-alpha fade timer (1 = drawn, 0 = faded out; read-only for us).
pub const SNIPE_FADE_OFF: usize = 0x5d8;

/// 2.7.2.0 [V exe file f9118fea2cfd, work/c6/aim6/chrcam_update.txt + tools: the call bytes `e8 a6 35 04 00` at
/// 0x1403B14E5 and `e8 5d 24 c8 ff` at 0x14077262E both target 0x1403F4A90; round 7: `e8 b0 96 d9 ff` at 0x14065B3DB too].
pub const AIM_CAM_2720: AimCamAddrs = AimCamAddrs {
    getter: 0x3f_4a90,
    getter_bytes: &GETTER_2720,
    chrcam_update: 0x3b_11d0,
    chrcam_prologue: &CHRCAM_PROLOGUE_2720,
    chrcam_call: 0x3b_14e5,
    reticle_update: 0x77_25c0,
    reticle_prologue: &RETICLE_PROLOGUE_2720,
    reticle_call: 0x77_262e,
    fade_site: 0x65_b3cf,
    fade_bytes: &FADE_SITE_2720,
    fade_call: 0x65_b3db,
    fade_stores: [0x65_b417, 0x65_b451],
};

/// MinHook relocates the getter's first instruction (`movzx eax, byte ptr [rcx+1C5h]`, 7 bytes, no RIP-relative
/// operand) into the trampoline and patches only those bytes.
pub const GETTER_FIRST_INSN: usize = 7;

/// The target of an `E8 rel32` call at `site` (None = not a direct call).
pub fn call_target(site: usize, b: &[u8; 5]) -> Option<usize> {
    if b[0] != 0xE8 {
        return None;
    }
    let rel = i32::from_le_bytes([b[1], b[2], b[3], b[4]]) as i64;
    usize::try_from(site as i64 + 5 + rel).ok()
}

/// The caller's return address of the camera site / the reticle site / the snipe-fade site (absolute, for the
/// detour's compare).
pub fn ret_addrs(a: &AimCamAddrs, base: usize) -> (usize, usize, usize) {
    (base + a.chrcam_call + 5, base + a.reticle_call + 5, base + a.fade_call + 5)
}

/// Install-time validation (`rd(abs_addr, out)` = a non-faulting read of the loaded image): the getter's bytes, both
/// functions' prologues, both call sites `E8 rel32` onto the getter and (round 7) the snipe-fade writer's 33 bytes,
/// its call onto the getter and its two timer stores. Err = the reason for `HOLD aim cam refused` (`fade_site` = the
/// fade cannot be masked: the aim pose then stays off with "keep"; an invisible body is worse than no pose).
pub fn validate(a: &AimCamAddrs, base: usize, rd: &dyn Fn(usize, &mut [u8]) -> bool) -> Result<(), &'static str> {
    let same = |rva: usize, want: &[u8]| {
        let mut b = [0u8; 64];
        let n = want.len();
        n <= b.len() && rd(base + rva, &mut b[..n]) && b[..n] == *want
    };
    if !same(a.getter, a.getter_bytes) {
        return Err("getter");
    }
    if !same(a.chrcam_update, a.chrcam_prologue) || !same(a.reticle_update, a.reticle_prologue) {
        return Err("prologue");
    }
    for site in [a.chrcam_call, a.reticle_call] {
        let mut b = [0u8; 5];
        if !rd(base + site, &mut b) || call_target(base + site, &b) != Some(base + a.getter) {
            return Err("site");
        }
    }
    let mut b = [0u8; 5];
    let inside = a.fade_call >= a.fade_site && a.fade_call + 5 <= a.fade_site + a.fade_bytes.len();
    if !inside
        || !same(a.fade_site, a.fade_bytes)
        || !rd(base + a.fade_call, &mut b)
        || call_target(base + a.fade_call, &b) != Some(base + a.getter)
        || !a.fade_stores.iter().all(|s| same(*s, &FADE_STORE_2720))
    {
        return Err("fade_site");
    }
    Ok(())
}

/// After an episode ended, the arm is kept this long while ER's bit is still set (the release window: ER clears it
/// within ~0.1 s; without this the camera would blend into the aim for those frames).
pub const AIM_CAM_GRACE_S: f64 = 1.0;

/// What decides the arm (task thread, every stance tick).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmIn {
    /// `aim_cam_keep()` (the aim pose with `tp_aim_cam = "keep"`) and the hook installed.
    pub keep: bool,
    /// An aim episode runs (`AimCtl::active`: from the ADS edge to the fields' restore).
    pub episode: bool,
    /// Third person (first person / a scope peek = never armed: first person identical).
    pub tp: bool,
    pub in_world: bool,
    pub dead: bool,
    /// Our main PlayerIns is known (the detour compares it).
    pub player: bool,
    /// ER's bit as the task read it this tick.
    pub prec: bool,
    /// Seconds since the last episode ended (infinite = none yet).
    pub since_end_s: f64,
}

/// The arm: only our own third-person aim episode (plus the release grace while ER's bit is still set). Every restore
/// point clears it before anything else (the DLL disarms first).
pub fn armed(i: &ArmIn) -> bool {
    i.keep && i.tp && i.in_world && !i.dead && i.player && (i.episode || (i.prec && i.since_end_s < AIM_CAM_GRACE_S))
}

/// `HOLD aim cam kept ms=.. hits_cam=.. hits_ret=.. hits_fade=..` (one line per armed span).
pub fn kept_line(ms: u64, hits_cam: u64, hits_ret: u64, hits_fade: u64) -> String {
    format!(
        "aim cam kept ms={ms} hits_cam={hits_cam} hits_ret={hits_ret} hits_fade={hits_fade} (ER's aim camera / crossbow reticle / snipe fade masked; the body turn saw the real bit)"
    )
}

// ---- C6 round 7 probe P-A7 (work/c6/aim7.md 4 C): does ER's aim move the arms at all? -------------------------------

/// A world vector in the body-yaw frame: `[right, up, forward]` with forward = (sin yaw, 0, cos yaw) (the yaw of
/// `atan2(x, z)`, as the `HOLD er_aim` lines) and right = up x forward.
pub fn body_frame(v: [f32; 3], yaw_deg: f32) -> [f32; 3] {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    [v[0] * c - v[2] * s, v[1], v[0] * s + v[2] * c]
}

/// ER's weapon frame relative to the body: (position from the feet, x axis, y axis), all in the body-yaw frame.
pub type PoseRel = ([f32; 3], [f32; 3], [f32; 3]);

/// The [`PoseRel`] of `root` = the weapon model's row-major 4x4 (rows 0..2 = axes, row 3 = translation, as `HOLD weapon
/// probe root=`).
pub fn weapon_rel(root: &[f32; 16], feet: [f32; 3], yaw_deg: f32) -> PoseRel {
    let p = [root[12] - feet[0], root[13] - feet[1], root[14] - feet[2]];
    (body_frame(p, yaw_deg), body_frame([root[0], root[1], root[2]], yaw_deg), body_frame([root[4], root[5], root[6]], yaw_deg))
}

/// An arm pose (not the carry) moves ER's weapon frame by more than this in the body frame, or turns it by more.
pub const POSE_MOVE_M: f32 = 0.10;
pub const POSE_TURN_DEG: f32 = 15.0;

fn ang_deg(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (na, nb) = ((a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt(), (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt());
    if !(na > 1e-6 && nb > 1e-6) {
        return 0.0;
    }
    ((a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (na * nb)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// (moved m, turned deg, an arm pose) of the in-aim weapon frame against the carry's (both from [`weapon_rel`]).
pub fn pose_moved(carry: &PoseRel, aim: &PoseRel) -> (f32, f32, bool) {
    let d = [aim.0[0] - carry.0[0], aim.0[1] - carry.0[1], aim.0[2] - carry.0[2]];
    let m = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let t = ang_deg(carry.1, aim.1).max(ang_deg(carry.2, aim.2));
    (m, t, m > POSE_MOVE_M || t > POSE_TURN_DEG)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C6 round 6: the 2.7.2.0 sites decode onto the getter; a moved / patched byte refuses the install.
    #[test]
    fn c6_aim_cam_sites_validate() {
        let base = 0x1_4000_0000usize;
        let a = AIM_CAM_2720;
        // the call bytes read from the exe file
        assert_eq!(call_target(base + a.chrcam_call, &[0xe8, 0xa6, 0x35, 0x04, 0x00]), Some(base + a.getter));
        assert_eq!(call_target(base + a.reticle_call, &[0xe8, 0x5d, 0x24, 0xc8, 0xff]), Some(base + a.getter));
        assert_eq!(call_target(0x1000, &[0xe9, 0, 0, 0, 0]), None, "a jmp is not the call");
        assert_eq!(ret_addrs(&a, base), (0x1_403b_14ea, 0x1_4077_2633, 0x1_4065_b3e0));
        // round 7: the fade site's call (5 bytes inside the pinned 33) and its second condition decode, its enable test
        // reads byte [0x143B1EFD8], both timer stores sit right behind it
        let k = a.fade_call - a.fade_site;
        let fc: [u8; 5] = FADE_SITE_2720[k..k + 5].try_into().unwrap();
        assert_eq!(call_target(base + a.fade_call, &fc), Some(base + a.getter));
        let c2: [u8; 5] = FADE_SITE_2720[24..29].try_into().unwrap();
        assert_eq!(call_target(base + a.fade_site + 24, &c2), Some(0x1_403f_4e10), "the second condition");
        let rel = i32::from_le_bytes([FADE_SITE_2720[2], FADE_SITE_2720[3], FADE_SITE_2720[4], FADE_SITE_2720[5]]) as usize;
        assert_eq!(base + a.fade_site + 7 + rel, 0x1_43b1_efd8);
        assert!(a.fade_stores.iter().all(|s| *s > a.fade_site && *s - a.fade_site < 0x100));
        assert_eq!(SNIPE_FADE_OFF, 0x5d8);
        // both call sites inside their functions
        assert!(a.chrcam_update < a.chrcam_call && a.chrcam_call - a.chrcam_update < 0x1000);
        assert!(a.reticle_update < a.reticle_call && a.reticle_call - a.reticle_update < 0x100);
        assert!(GETTER_FIRST_INSN >= 5 && a.getter_bytes.len() == 12 && a.getter_bytes[11] == 0xc3);
        // a fake image with the real bytes
        let image = |patch: Option<(usize, u8)>| {
            let mut m: Vec<(usize, Vec<u8>)> = vec![
                (base + a.getter, GETTER_2720.to_vec()),
                (base + a.chrcam_update, CHRCAM_PROLOGUE_2720.to_vec()),
                (base + a.reticle_update, RETICLE_PROLOGUE_2720.to_vec()),
                (base + a.chrcam_call, vec![0xe8, 0xa6, 0x35, 0x04, 0x00]),
                (base + a.reticle_call, vec![0xe8, 0x5d, 0x24, 0xc8, 0xff]),
                (base + a.fade_site, FADE_SITE_2720.to_vec()),
                (base + a.fade_stores[0], FADE_STORE_2720.to_vec()),
                (base + a.fade_stores[1], FADE_STORE_2720.to_vec()),
            ];
            if let Some((addr, v)) = patch {
                for (s, b) in m.iter_mut() {
                    if addr >= *s && addr < *s + b.len() {
                        b[addr - *s] = v;
                    }
                }
            }
            m
        };
        let check = |m: &Vec<(usize, Vec<u8>)>| {
            let rd = |addr: usize, out: &mut [u8]| {
                m.iter().any(|(s, b)| {
                    if addr >= *s && addr + out.len() <= *s + b.len() {
                        out.copy_from_slice(&b[addr - s..addr - s + out.len()]);
                        true
                    } else {
                        false
                    }
                })
            };
            validate(&a, base, &rd)
        };
        assert_eq!(check(&image(None)), Ok(()));
        assert_eq!(check(&image(Some((base + a.getter, 0xe9)))), Err("getter"), "someone else's hook on the getter");
        assert_eq!(check(&image(Some((base + a.chrcam_update + 3, 0x90)))), Err("prologue"));
        assert_eq!(check(&image(Some((base + a.reticle_update, 0xcc)))), Err("prologue"));
        assert_eq!(check(&image(Some((base + a.chrcam_call + 1, 0xa7)))), Err("site"), "the call no longer targets the getter");
        assert_eq!(check(&image(Some((base + a.reticle_call, 0xe9)))), Err("site"));
        // round 7: the snipe-fade writer
        assert_eq!(check(&image(Some((base + a.fade_site, 0x90)))), Err("fade_site"), "the enable test moved");
        assert_eq!(check(&image(Some((base + a.fade_call + 2, 0x97)))), Err("fade_site"), "the fade call no longer targets the getter");
        assert_eq!(check(&image(Some((base + a.fade_stores[1] + 4, 0xdc)))), Err("fade_site"), "a store of another field");
        let mut m = image(None);
        m.retain(|(s, _)| *s != base + a.fade_stores[0]);
        assert_eq!(check(&m), Err("fade_site"), "an unreadable store");
        assert_eq!(check(&Vec::new()), Err("getter"), "unreadable");
    }

    /// C6 round 6: armed only for our own third-person aim episode (+ the release grace while ER's bit lasts); first
    /// person, death, a world leave, no player, "er" never arm.
    #[test]
    fn c6_aim_cam_arm_conditions() {
        let on = ArmIn { keep: true, episode: true, tp: true, in_world: true, dead: false, player: true, prec: true, since_end_s: f64::INFINITY };
        assert!(armed(&on));
        for (what, i) in [
            ("er / not installed", ArmIn { keep: false, ..on }),
            ("first person", ArmIn { tp: false, ..on }),
            ("world leave", ArmIn { in_world: false, ..on }),
            ("death", ArmIn { dead: true, ..on }),
            ("no player", ArmIn { player: false, ..on }),
        ] {
            assert!(!armed(&i), "{what}");
        }
        // the episode runs before ER's bit shows (Armed) and through the release
        assert!(armed(&ArmIn { prec: false, ..on }));
        // after the episode: kept while the bit lasts, at most the grace
        let after = ArmIn { episode: false, since_end_s: 0.05, ..on };
        assert!(armed(&after));
        assert!(!armed(&ArmIn { prec: false, ..after }), "bit clear: disarmed");
        assert!(!armed(&ArmIn { since_end_s: AIM_CAM_GRACE_S, ..after }), "grace over");
        assert!(!armed(&ArmIn { episode: false, since_end_s: f64::INFINITY, ..on }), "a real crossbow's aim (no episode of ours) keeps ER's camera");
        assert!(kept_line(1200, 70, 70, 70).starts_with("aim cam kept ms=1200 hits_cam=70 hits_ret=70 hits_fade=70 "));
    }

    /// C6 round 7 P-A7: the weapon frame in the body-yaw frame cancels the body turn (the round-6 logs: a pure yaw of
    /// 125 deg between the carry and the aim = no arm pose); a raised, pitched weapon is an arm pose.
    #[test]
    fn c6_aim_pose_probe_body_frame() {
        let f = body_frame([1.0, 0.0, 0.0], 90.0);
        assert!((f[2] - 1.0).abs() < 1e-5 && f[0].abs() < 1e-5, "yaw 90 faces +x: {f:?}");
        let r = body_frame([0.0, 0.0, -1.0], 90.0);
        assert!((r[0] - 1.0).abs() < 1e-5, "right = up x forward: {r:?}");
        // the world rotation of a body yaw (a vector at yaw 0 seen after the body turned by `deg`)
        let yaw = |deg: f32| {
            let (s, c) = deg.to_radians().sin_cos();
            move |v: [f32; 3]| [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
        };
        let (x, y, p) = ([-0.154f32, 0.980, -0.128], [-0.664f32, -0.198, -0.721], [0.3f32, 1.1, 0.2]);
        let root = |rot: &dyn Fn([f32; 3]) -> [f32; 3], p: [f32; 3], feet: [f32; 3]| {
            let (x, y, p) = (rot(x), rot(y), p);
            [x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, 0.0, 0.0, 0.0, 0.0, p[0] + feet[0], p[1] + feet[1], p[2] + feet[2], 1.0]
        };
        let feet0 = [5.0, 2.0, -1.0];
        let carry = weapon_rel(&root(&yaw(0.0), p, feet0), feet0, 0.0);
        let feet1 = [5.2, 2.0, -1.1];
        let turned = weapon_rel(&root(&yaw(125.0), yaw(125.0)(p), feet1), feet1, 125.0);
        let (m, t, arm) = pose_moved(&carry, &turned);
        assert!(m < 1e-4 && t < 0.05 && !arm, "a body turn is no arm pose: {m} {t}");
        // the same yaw, the weapon raised 0.25 m and pitched 30 deg: an arm pose
        let pitch = |v: [f32; 3]| {
            let (s, c) = 30f32.to_radians().sin_cos();
            [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
        };
        let up = yaw(125.0)([p[0], p[1] + 0.25, p[2]]);
        let raised = weapon_rel(&root(&|v| yaw(125.0)(pitch(v)), up, feet1), feet1, 125.0);
        let (m, t, arm) = pose_moved(&carry, &raised);
        assert!(arm && (m - 0.25).abs() < 0.01 && t > POSE_TURN_DEG, "{m} {t}");
    }
}
