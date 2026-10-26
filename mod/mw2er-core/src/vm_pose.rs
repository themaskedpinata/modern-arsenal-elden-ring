//! Viewmodel pose evaluation (docs/M3_DESIGN.md section 3.1): clip sampling, crossfade, the ADS layer, forward
//! kinematics and the skinning palette. Runs on the present thread at its own `now`, so it allocates nothing
//! per frame ([`PoseScratch`] is reused).
//!
//! Semantics (verified on the data by oracle 1g): a translation track replaces the bind translation; no
//! translation track keeps the bind translation; a bone listed in a clip without a quat track gets the
//! identity rotation; bones absent from a clip keep the full bind. ADS overrides only `tag_ads`.

use crate::vm_anim::{AdsDir, ClipPlay, VmState};
use crate::vm_assets::{Clip, ClipId, Skeleton, Track, VmAssets};
use crate::xform::{MAT34_IDENTITY, Mat34, QUAT_IDENTITY, Quat, Vec3, m_mul, m_trs, q_lerp_keys, q_nlerp, v_lerp};

/// Reused buffers (one per thread that evaluates poses).
#[derive(Clone, Debug, Default)]
pub struct PoseScratch {
    pub t: Vec<Vec3>,
    pub q: Vec<Quat>,
    pt: Vec<Vec3>,
    pq: Vec<Quat>,
    pub world: Vec<Mat34>,
    pub palette: Vec<Mat34>,
}

impl PoseScratch {
    /// No buffers yet (a `static`'s initial value; [`PoseScratch::reserve`] sizes it).
    pub const fn empty() -> Self {
        PoseScratch { t: Vec::new(), q: Vec::new(), pt: Vec::new(), pq: Vec::new(), world: Vec::new(), palette: Vec::new() }
    }

    pub fn new(bones: usize) -> Self {
        PoseScratch {
            t: vec![[0.0; 3]; bones],
            q: vec![QUAT_IDENTITY; bones],
            pt: vec![[0.0; 3]; bones],
            pq: vec![QUAT_IDENTITY; bones],
            world: vec![MAT34_IDENTITY; bones],
            palette: vec![MAT34_IDENTITY; bones],
        }
    }

    /// Exactly `PoseScratch::new(n)`'s contents when the bone count changes, refilled in place: no allocation while
    /// `n` stays within the capacity ([`PoseScratch::reserve`]; C4.1 poses on ER's render thread).
    fn fit(&mut self, n: usize) {
        if self.t.len() != n {
            for v in [&mut self.t, &mut self.pt] {
                v.clear();
                v.resize(n, [0.0; 3]);
            }
            for v in [&mut self.q, &mut self.pq] {
                v.clear();
                v.resize(n, QUAT_IDENTITY);
            }
            for v in [&mut self.world, &mut self.palette] {
                v.clear();
                v.resize(n, MAT34_IDENTITY);
            }
        }
    }

    /// Capacity for `bones` bones in every buffer (allocated here, once, off the render thread).
    pub fn reserve(&mut self, bones: usize) {
        for v in [&mut self.t, &mut self.pt] {
            v.reserve(bones.saturating_sub(v.len()));
        }
        for v in [&mut self.q, &mut self.pq] {
            v.reserve(bones.saturating_sub(v.len()));
        }
        for v in [&mut self.world, &mut self.palette] {
            v.reserve(bones.saturating_sub(v.len()));
        }
    }

    /// The smallest capacity of its buffers (the bone count it can pose without allocating).
    pub fn capacity(&self) -> usize {
        [self.t.capacity(), self.pt.capacity(), self.q.capacity(), self.pq.capacity(), self.world.capacity(), self.palette.capacity()].into_iter().min().unwrap_or(0)
    }
}

/// Frame index (fractional) of `clip` after `t` seconds at `rate`: clamped for one-shot clips, wrapped for
/// looped ones (keys run 0..=numframes, the last equal to the first).
pub fn clip_frame(clip: &Clip, t: f32, rate: f32) -> f32 {
    let nf = clip.numframes as f32;
    let f = t * clip.fps as f32 * rate;
    if !f.is_finite() {
        return 0.0;
    }
    if clip.looped && nf > 0.0 { f.rem_euclid(nf) } else { f.clamp(0.0, nf) }
}

/// (key a, key b, blend) for frame `f`, clamping outside the keyed range.
pub fn span(frames: &[u16], f: f32) -> (usize, usize, f32) {
    let n = frames.len();
    if n == 0 || f <= frames[0] as f32 {
        return (0, 0, 0.0);
    }
    if f >= frames[n - 1] as f32 {
        return (n - 1, n - 1, 0.0);
    }
    // first key strictly after f
    let hi = frames.partition_point(|&k| (k as f32) <= f);
    let lo = hi - 1;
    let (a, b) = (frames[lo] as f32, frames[hi] as f32);
    (lo, hi, if b > a { (f - a) / (b - a) } else { 0.0 })
}

/// Sampled (rotation, translation) of one track; `None` where the track has no keys of that kind.
pub fn sample_track(tr: &Track, f: f32) -> (Option<Quat>, Option<Vec3>) {
    let rot = (!tr.rot.is_empty()).then(|| {
        let (a, b, u) = span(&tr.rot_frames, f);
        q_lerp_keys(&tr.rot[a], &tr.rot[b], u)
    });
    let trans = (!tr.trans.is_empty()).then(|| {
        let (a, b, u) = span(&tr.trans_frames, f);
        v_lerp(&tr.trans[a], &tr.trans[b], u)
    });
    (rot, trans)
}

pub fn bind_into(skel: &Skeleton, t: &mut [Vec3], q: &mut [Quat]) {
    for (i, b) in skel.bones.iter().enumerate() {
        t[i] = b.t;
        q[i] = b.q;
    }
}

/// Overrides the bones `clip` drives at frame `f`.
pub fn apply_clip(clip: &Clip, f: f32, t: &mut [Vec3], q: &mut [Quat]) {
    for tr in &clip.tracks {
        let i = tr.bone as usize;
        if i >= t.len() {
            continue;
        }
        let (rot, trans) = sample_track(tr, f);
        if let Some(v) = trans {
            t[i] = v;
        }
        q[i] = rot.unwrap_or(QUAT_IDENTITY);
    }
}

pub fn fk_into(skel: &Skeleton, t: &[Vec3], q: &[Quat], world: &mut [Mat34]) {
    for (i, b) in skel.bones.iter().enumerate() {
        let local = m_trs(&t[i], &q[i]);
        world[i] = if b.parent >= 0 { m_mul(&world[b.parent as usize], &local) } else { local };
    }
}

pub fn palette_into(skel: &Skeleton, world: &[Mat34], pal: &mut [Mat34]) {
    for (i, b) in skel.bones.iter().enumerate() {
        pal[i] = m_mul(&world[i], &b.ibm);
    }
}

/// Local pose of one playing clip at `now` (bind first, then the clip's tracks).
fn play_into(a: &VmAssets, p: &ClipPlay, now: f64, t: &mut [Vec3], q: &mut [Quat]) {
    bind_into(&a.skel, t, q);
    let clip = a.clip(p.clip);
    let secs = ((now - p.t0) as f32).max(0.0);
    apply_clip(clip, clip_frame(clip, secs, p.rate), t, q);
}

/// The ADS layer: the bones the ADS clip drives change (the UMP45's drive only `tag_ads`; M6: the RPG's drive
/// `tag_torso`). Entering samples ads_up at frac * duration, leaving samples ads_down at (1 - frac) * duration;
/// frac 0 = no layer.
pub fn ads_layer(a: &VmAssets, frac: f32, dir: AdsDir, t: &mut [Vec3], q: &mut [Quat]) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let (clip, f) = match dir {
        AdsDir::Out => {
            let c = a.clip(ClipId::AdsDown);
            (c, (1.0 - frac) * c.numframes as f32)
        }
        AdsDir::In | AdsDir::Hold => {
            let c = a.clip(ClipId::AdsUp);
            (c, frac * c.numframes as f32)
        }
    };
    for tr in &clip.tracks {
        let i = tr.bone as usize;
        if i >= t.len() {
            continue;
        }
        let (rot, trans) = sample_track(tr, f);
        if let Some(v) = trans {
            t[i] = v;
        }
        q[i] = rot.unwrap_or(QUAT_IDENTITY);
    }
}

/// Full evaluation for the renderer: base clip, crossfade from the previous clip, ADS layer, FK, palette.
/// Results are in `s.world` and `s.palette`.
pub fn evaluate(a: &VmAssets, st: &VmState, now: f64, s: &mut PoseScratch) {
    let n = a.skel.bones.len();
    s.fit(n);
    play_into(a, &st.base, now, &mut s.t, &mut s.q);
    if let Some(prev) = st.prev {
        let w = if st.fade_s > 0.0 { ((now - st.fade_t0) as f32 / st.fade_s).clamp(0.0, 1.0) } else { 1.0 };
        if w < 1.0 {
            let (mut pt, mut pq) = (std::mem::take(&mut s.pt), std::mem::take(&mut s.pq));
            play_into(a, &prev, now, &mut pt, &mut pq);
            for i in 0..n {
                s.t[i] = v_lerp(&pt[i], &s.t[i], w);
                s.q[i] = q_nlerp(&pq[i], &s.q[i], w);
            }
            s.pt = pt;
            s.pq = pq;
        }
    }
    ads_layer(a, st.ads_at(now), st.ads_dir, &mut s.t, &mut s.q);
    fk_into(&a.skel, &s.t, &s.q, &mut s.world);
    palette_into(&a.skel, &s.world, &mut s.palette);
}

/// One fixed pose of the offline oracles (shared by vmtest and blender_ref.py via work\m3\poses.json).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TestPose {
    pub name: &'static str,
    pub clip: ClipId,
    /// Native seconds into the clip (rate 1); negative = the clip end.
    pub t: f32,
    pub ads: f32,
}

pub const TEST_POSES: [TestPose; 9] = [
    TestPose { name: "idle", clip: ClipId::Idle, t: 0.0, ads: 0.0 },
    TestPose { name: "fire", clip: ClipId::Fire, t: 0.05, ads: 0.0 },
    TestPose { name: "reload_a", clip: ClipId::Reload, t: 0.5, ads: 0.0 },
    TestPose { name: "reload_b", clip: ClipId::Reload, t: 1.2, ads: 0.0 },
    TestPose { name: "reload_c", clip: ClipId::Reload, t: 2.0, ads: 0.0 },
    TestPose { name: "reload_empty_bolt", clip: ClipId::ReloadEmpty, t: 2.3, ads: 0.0 },
    TestPose { name: "ads", clip: ClipId::Idle, t: 0.0, ads: 1.0 },
    // the design said t = 0.1 s, where the weapon is still entirely below the screen (0 px in both renderers, IoU
    // undefined); 0.5 s of the 0.833 s pullout shows it coming up
    TestPose { name: "pullout", clip: ClipId::Pullout, t: 0.5, ads: 0.0 },
    TestPose { name: "sprint", clip: ClipId::SprintLoop, t: 0.5, ads: 0.0 },
];

/// A static pose (clip at `t` native seconds, ADS entering at `ads`), as a VmState evaluated at now = t.
pub fn static_state(clip: ClipId, t: f32, ads: f32) -> (VmState, f64) {
    let mut st = VmState::hidden();
    st.visible = true;
    st.base = ClipPlay { clip, t0: 0.0, rate: 1.0 };
    st.ads_frac = ads;
    st.ads_dir = AdsDir::Hold;
    (st, t as f64)
}

// ---- M5: stance pose and weapon bob (docs/M5_DESIGN.md section 8.2) ------------------------------------------

/// What the controller tells the viewmodel each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StancePoseIn {
    /// 0 stand, 1 crouch, 2 prone (the target stance).
    pub stance: u8,
    /// Planar speed (in/s).
    pub speed: f32,
    /// Strafe fraction -1..1 (rightmove / 127).
    pub strafe: f32,
    pub sprinting: bool,
    pub airborne: bool,
    pub ads: f32,
    /// Distance walked (inches): the bob cycle.
    pub walked: f32,
}

/// The weapon-file pose numbers (inches; forward, right, up) and the MW2 bob amplitudes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StancePoseParams {
    pub ducked_ofs: [f32; 3],
    pub prone_ofs: [f32; 3],
    pub stand_move: [f32; 3],
    pub ducked_move: [f32; 3],
    pub prone_move: [f32; 3],
    pub strafe_move: [f32; 3],
    pub move_rate: f32,
    pub prone_move_rate: f32,
    /// Weapon bob (x, y) per stance: stand, ducked, prone, sprint.
    pub bob: [[f32; 2]; 4],
    pub bob_max: f32,
    pub ads_bob_factor: f32,
    /// player_moveThreshhold (in/s).
    pub move_threshold: f32,
}

/// Bob scale from MW2's amplitude x speed to inches [I: the engine formula is not decoded].
/// Run 11: 0.15 gave ~1.2 in of sideways gun motion while walking (user: "bouncing very quickly", ADS sway
/// "almost impossible to aim"); 0.06 is ~0.5 in at hip.
pub const BOB_TO_IN: f32 = 0.06;
/// Extra weapon-bob scale at full ADS (MW2's adsBobFactor is 1.0 for the UMP45, but its bob is small enough
/// that the sights barely move; with our inferred scale the ADS bob must be damped to stay aimable) [I].
pub const ADS_WEAPON_BOB: f32 = 0.15;

/// The smoothed view-space offset of the gun (inches, forward / right / up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StancePose {
    pub pos: [f32; 3],
}

impl StancePose {
    /// The target offset of the stance and motion (no bob).
    pub fn target(p: &StancePoseParams, i: &StancePoseIn) -> [f32; 3] {
        let base = match i.stance {
            1 => p.ducked_ofs,
            2 => p.prone_ofs,
            _ => [0.0; 3],
        };
        let moving = i.speed > p.move_threshold && !i.airborne;
        let mv = if moving {
            match i.stance {
                1 => p.ducked_move,
                2 => p.prone_move,
                _ => p.stand_move,
            }
        } else {
            [0.0; 3]
        };
        let st = if moving { i.strafe.abs().min(1.0) } else { 0.0 };
        // Hip-only offsets: they fade out with ADS so the sights stay centred (run 12: ADS "broke" crouched/prone
        // because duckedOfs/proneOfs still moved the gun at full ADS).
        let hip = 1.0 - i.ads.clamp(0.0, 1.0);
        [0, 1, 2].map(|k| (base[k] + mv[k] + p.strafe_move[k] * st) * hip)
    }

    /// One step toward the target at posMoveRate (posProneMoveRate prone): 63 % after 1/rate s.
    pub fn step(&mut self, p: &StancePoseParams, i: &StancePoseIn, dt: f32) -> [f32; 3] {
        let tgt = Self::target(p, i);
        let rate = if i.stance == 2 { p.prone_move_rate } else { p.move_rate };
        let k = 1.0 - (-rate * dt.max(0.0)).exp();
        for (x, t) in self.pos.iter_mut().zip(tgt) {
            *x += (t - *x) * k;
        }
        let b = Self::bob(p, i);
        [self.pos[0], self.pos[1] + b[0], self.pos[2] + b[1]]
    }

    /// Weapon bob (right, up) in inches: amplitude per stance x speed, clamped at bobMax, no bob in the air.
    pub fn bob(p: &StancePoseParams, i: &StancePoseIn) -> [f32; 2] {
        if i.airborne || i.speed <= p.move_threshold {
            return [0.0, 0.0];
        }
        let idx = if i.sprinting { 3 } else { (i.stance as usize).min(2) };
        let a = i.ads.clamp(0.0, 1.0);
        let ads = (1.0 + (p.ads_bob_factor - 1.0) * a) * (1.0 + (ADS_WEAPON_BOB - 1.0) * a);
        let phase = i.walked / 36.0 * std::f32::consts::PI;
        let ax = (p.bob[idx][0] * i.speed).min(p.bob_max) * BOB_TO_IN * ads;
        let ay = (p.bob[idx][1] * i.speed).min(p.bob_max) * BOB_TO_IN * ads;
        // Vertical: a smooth dip per step, (1 - cos 2x) / 2, instead of |sin 2x| whose sharp V read as a fast bounce.
        [ax * phase.sin(), -ay * 0.5 * (1.0 - (2.0 * phase).cos())]
    }
}

/// The renderer's `offset_in` axes from (forward, right, up): IW model space is x forward, y left, z up [I].
pub fn offset_from_fru(v: [f32; 3]) -> [f32; 3] {
    [v[0], -v[1], v[2]]
}

#[cfg(test)]
mod stance_pose_tests {
    use super::*;

    fn params() -> StancePoseParams {
        StancePoseParams {
            ducked_ofs: [0.0, 0.8, 0.0],
            prone_ofs: [0.0, 1.5, 0.0],
            stand_move: [0.0, 0.0, -0.5],
            ducked_move: [-0.5, 0.0, 0.0],
            prone_move: [-50.0, 3.0, -105.0],
            strafe_move: [0.0, 0.5, 0.0],
            move_rate: 6.0,
            prone_move_rate: 25.0,
            bob: [[0.055, 0.025], [0.045, 0.025], [0.02, 0.005], [0.02, 0.014]],
            bob_max: 8.0,
            ads_bob_factor: 1.0,
            move_threshold: 10.0,
        }
    }

    #[test]
    fn vm_pose_stance_offsets_converge_at_pos_move_rate() {
        let p = params();
        let mut s = StancePose::default();
        let i = StancePoseIn { stance: 1, ..StancePoseIn::default() };
        for _ in 0..10 {
            s.step(&p, &i, 1.0 / 60.0);
        }
        // 1/6 s at rate 6: 63 %
        assert!((s.pos[1] - 0.8 * (1.0 - (-1.0f32).exp())).abs() < 1e-3, "{:?}", s.pos);
        for _ in 0..600 {
            s.step(&p, &i, 1.0 / 60.0);
        }
        assert!((s.pos[1] - 0.8).abs() < 1e-4);
    }

    #[test]
    fn vm_pose_prone_uses_prone_rate_and_crawl_hides() {
        let p = params();
        let mut s = StancePose::default();
        let i = StancePoseIn { stance: 2, ..StancePoseIn::default() };
        s.step(&p, &i, 1.0 / 25.0);
        assert!((s.pos[1] - 1.5 * (1.0 - (-1.0f32).exp())).abs() < 1e-3);
        // crawling: proneMove pushes the gun down out of view
        let crawl = StancePoseIn { stance: 2, speed: 28.5, ..StancePoseIn::default() };
        assert!(StancePose::target(&p, &crawl)[2] < -100.0);
    }

    #[test]
    fn vm_pose_bob_per_stance_and_none_in_air() {
        let p = params();
        let run = StancePoseIn { speed: 190.0, walked: 18.0, ..StancePoseIn::default() };
        let b_stand = StancePose::bob(&p, &run);
        let b_prone = StancePose::bob(&p, &StancePoseIn { stance: 2, ..run });
        assert!(b_stand[0].abs() > b_prone[0].abs() && b_stand[0] != 0.0);
        assert_eq!(StancePose::bob(&p, &StancePoseIn { airborne: true, ..run }), [0.0, 0.0]);
        assert_eq!(StancePose::bob(&p, &StancePoseIn { speed: 5.0, ..run }), [0.0, 0.0]);
        // clamped at bobMax
        let fast = StancePoseIn { speed: 10_000.0, ..run };
        assert!(StancePose::bob(&p, &fast)[0].abs() <= p.bob_max * BOB_TO_IN + 1e-6);
        assert_eq!(offset_from_fru([1.0, 2.0, 3.0]), [1.0, -2.0, 3.0]);
    }
    #[test]
    fn vm_pose_stance_offsets_fade_out_in_ads() {
        let mut p = params();
        p.ducked_ofs = [-2.0, 1.0, -1.5];
        p.prone_ofs = [-4.0, 2.0, -3.0];
        for stance in [1u8, 2] {
            let hip = StancePose::target(&p, &StancePoseIn { stance, ads: 0.0, ..Default::default() });
            let ads = StancePose::target(&p, &StancePoseIn { stance, ads: 1.0, ..Default::default() });
            assert!(hip.iter().any(|v| v.abs() > 0.5), "hip offset present");
            assert_eq!(ads, [0.0; 3], "no stance offset at full ADS");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm_assets::testdata;
    use crate::xform::m_translation;

    fn close(a: &[f32], b: &[f32], eps: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
    }

    #[test]
    fn c4_pose_scratch_reserved_never_reallocates() {
        // C4.1 (docs/C4_1_DESIGN.md 3.5): the render thread's scratch is reserved off it; switching guns of other bone
        // counts refills in place, with exactly `new(n)`'s contents
        let mut s = PoseScratch::default();
        s.reserve(128);
        assert!(s.capacity() >= 128);
        let ptrs = |s: &PoseScratch| (s.t.as_ptr() as usize, s.q.as_ptr() as usize, s.world.as_ptr() as usize, s.palette.as_ptr() as usize);
        let p0 = ptrs(&s);
        for n in [91usize, 40, 128, 7, 91] {
            s.fit(n);
            assert_eq!(ptrs(&s), p0, "no reallocation for {n} bones");
            let fresh = PoseScratch::new(n);
            assert_eq!((s.t.len(), s.pt.len(), s.q.len(), s.pq.len(), s.world.len(), s.palette.len()), (n, n, n, n, n, n));
            assert!(s.t == fresh.t && s.q == fresh.q && s.world == fresh.world && s.palette == fresh.palette && s.pt == fresh.pt && s.pq == fresh.pq);
            s.t.iter_mut().for_each(|v| v[0] = 9.0);
        }
        // the posing itself never grows the buffers
        let a = testdata::assets();
        let mut s = PoseScratch::default();
        s.reserve(a.skel.bones.len());
        let p0 = ptrs(&s);
        evaluate(&a, &VmState::hidden(), 1.0, &mut s);
        assert_eq!(ptrs(&s), p0);
    }

    #[test]
    fn vm_pose_sampling_lerps_and_clamps() {
        let a = testdata::assets();
        let c = a.clip(ClipId::Fire);
        let tr = c.track_of(3).unwrap();
        assert!(close(&sample_track(tr, 5.0).1.unwrap(), &[11.0, -2.0, -4.0], 1e-6));
        assert!(close(&sample_track(tr, -3.0).1.unwrap(), &[10.0, -2.0, -4.0], 1e-6));
        assert!(close(&sample_track(tr, 99.0).1.unwrap(), &[12.0, -2.0, -4.0], 1e-6));
        assert_eq!(span(&[0, 4, 10], 4.0), (1, 2, 0.0));
        assert_eq!(span(&[0, 4, 10], 7.0), (1, 2, 0.5));
        // a one-shot clip clamps at its end; frame = t * fps * rate
        assert_eq!(clip_frame(c, 1.0, 1.0), 10.0);
        assert_eq!(clip_frame(c, 0.1, 2.0), 6.0);
        assert_eq!(clip_frame(c, -1.0, 1.0), 0.0);
        assert_eq!(clip_frame(c, f32::NAN, 1.0), 0.0);
    }

    #[test]
    fn vm_pose_loop_wraps() {
        let a = testdata::assets();
        let c = a.clip(ClipId::SprintLoop);
        assert!((clip_frame(c, 0.5, 1.0) - 5.0).abs() < 1e-5); // 0.5 s * 30 = 15 -> 15 mod 10
        assert!(clip_frame(c, 10.0, 1.0) < 10.0);
    }

    #[test]
    fn vm_pose_semantics_no_quat_track_is_identity() {
        let a = testdata::assets();
        let mut s = PoseScratch::new(a.skel.bones.len());
        bind_into(&a.skel, &mut s.t, &mut s.q);
        s.q[3] = [0.0, 0.0, 1.0, 0.0]; // pretend tag_weapon had a non-identity bind rotation
        apply_clip(a.clip(ClipId::Fire), 10.0, &mut s.t, &mut s.q);
        assert_eq!(s.q[3], QUAT_IDENTITY, "translation-only track -> identity rotation");
        assert!(close(&s.t[3], &[12.0, -2.0, -4.0], 1e-6), "translation replaces bind (not bind + delta)");
        assert_eq!(s.t[5], a.skel.bones[5].t, "bones absent from the clip keep bind");
    }

    #[test]
    fn vm_pose_fk_known_answers() {
        let a = testdata::assets();
        let mut s = PoseScratch::new(a.skel.bones.len());
        let (st, now) = static_state(ClipId::Idle, 0.0, 0.0);
        evaluate(&a, &st, now, &mut s);
        // tag_flash = view (0,0,60) + weapon (10,-2,-4) + flash (16,0,0)
        assert!(close(&m_translation(&s.world[5]), &[26.0, -2.0, 56.0], 1e-4));
        // bind palette is the identity everywhere
        for p in &s.palette {
            assert!(close(p, &MAT34_IDENTITY, 1e-4));
        }
        // fire at its end: tag_weapon x = 12 and j_gun turned 90 degrees about Z -> tag_flash at y + 16
        let (st, now) = static_state(ClipId::Fire, 10.0, 0.0);
        evaluate(&a, &st, now, &mut s);
        assert!(close(&m_translation(&s.world[5]), &[12.0, 14.0, 56.0], 1e-3), "{:?}", m_translation(&s.world[5]));
    }

    #[test]
    fn vm_pose_ads_layer_moves_only_tag_ads() {
        let a = testdata::assets();
        let mut s = PoseScratch::new(a.skel.bones.len());
        let (st, now) = static_state(ClipId::Idle, 0.0, 1.0);
        evaluate(&a, &st, now, &mut s);
        assert!(close(&s.t[2], &[-4.0, 2.0, 1.0], 1e-6));
        assert_eq!(s.t[3], [10.0, -2.0, -4.0]);
        let (mut st, now) = static_state(ClipId::Idle, 0.0, 0.5);
        evaluate(&a, &st, now, &mut s);
        assert!(close(&s.t[2], &[-2.0, 1.0, 0.5], 1e-6));
        // leaving at frac 0.25: ads_down at 75% -> a quarter of the way from bind
        st.ads_frac = 0.25;
        st.ads_dir = AdsDir::Out;
        evaluate(&a, &st, now, &mut s);
        assert!(close(&s.t[2], &[-1.0, 0.5, 0.25], 1e-6));
    }

    #[test]
    fn vm_pose_crossfade_blends_then_settles() {
        let a = testdata::assets();
        let mut s = PoseScratch::new(a.skel.bones.len());
        let mut st = VmState::hidden();
        st.visible = true;
        // previous clip: fire held at its end (x = 12); new clip: idle (x = 10), fading over 0.1 s from t = 1
        st.prev = Some(ClipPlay { clip: ClipId::Fire, t0: 0.0, rate: 1.0 });
        st.base = ClipPlay { clip: ClipId::Idle, t0: 1.0, rate: 1.0 };
        st.fade_t0 = 1.0;
        st.fade_s = 0.1;
        evaluate(&a, &st, 1.05, &mut s);
        assert!((s.t[3][0] - 11.0).abs() < 1e-4, "half way: {}", s.t[3][0]);
        evaluate(&a, &st, 1.2, &mut s);
        assert!((s.t[3][0] - 10.0).abs() < 1e-6);
        // deterministic for the same (state, now)
        evaluate(&a, &st, 1.05, &mut s);
        let w1 = s.world.clone();
        evaluate(&a, &st, 1.05, &mut s);
        assert_eq!(w1, s.world);
    }

    #[test]
    fn vm_pose_test_poses_are_the_designed_nine() {
        let names: Vec<&str> = TEST_POSES.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["idle", "fire", "reload_a", "reload_b", "reload_c", "reload_empty_bolt", "ads", "pullout", "sprint"]
        );
    }

}
