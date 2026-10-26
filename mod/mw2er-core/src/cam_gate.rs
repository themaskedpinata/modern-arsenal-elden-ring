//! Camera validity gate (docs/M3_DESIGN.md section 9): removes the `cam_player_dist ~170 m` episodes.
//!
//! Root cause seen in the M2 logs: right after `TASK world entered`, `CSCamera.pers_cam_1` is the identity
//! placeholder at the origin (or the camera from before the load), and the M2 sign tracker trusted that first
//! sample. The gate rejects implausible samples and only reports the camera valid after 3 consecutive good
//! ticks; it resets on world enter/leave and on a re-base jump (> 5 m in one tick).

/// One camera sample (rows 0..2 of `CSCam.matrix`, position row 3, raw FOV) plus the player's chest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamSample {
    pub rows: [[f32; 3]; 3],
    pub pos: [f32; 3],
    pub fov: f32,
    pub chest: [f32; 3],
    pub player: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Ok,
    NonFinite,
    Identity,
    Ortho,
    Det,
    Fov,
    Dist,
    Settling,
}

impl Why {
    pub fn label(self) -> &'static str {
        match self {
            Why::Ok => "ok",
            Why::NonFinite => "nonfinite",
            Why::Identity => "identity",
            Why::Ortho => "ortho",
            Why::Det => "det",
            Why::Fov => "fov",
            Why::Dist => "dist",
            Why::Settling => "settling",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateParams {
    /// Expected sign of det(rows 0..2) (`aim.camera_det_sign`): +1 or -1 once a CAMFRAME log has shown it;
    /// 0 (default) accepts either sign (|det| must still be 1). A wrong guess here would block every shot, so
    /// the default does not guess.
    pub det_sign: f32,
    pub ticks_needed: u32,
    pub min_dist_m: f32,
    pub max_dist_m: f32,
    pub jump_m: f32,
    /// Run 18: first person drives the render camera (On, or about to arm), so ER's own third-person camera being
    /// pulled next to the chest by its collision (a wagon, a wall: cam_chest 0.08-0.29 m) is not a space mismatch:
    /// a distance below `min_dist_m` is accepted. Absurd distances (> `max_dist_m`, a re-base, the placeholder at the
    /// origin) are still rejected and a > `jump_m` jump still restarts the settle count.
    pub close_ok: bool,
}

impl Default for GateParams {
    fn default() -> Self {
        GateParams { det_sign: 0.0, ticks_needed: 3, min_dist_m: 0.3, max_dist_m: 15.0, jump_m: 5.0, close_ok: false }
    }
}

fn dot(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    dot(&d, &d).sqrt()
}

pub fn det3(r: &[[f32; 3]; 3]) -> f32 {
    r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0])
}

/// Orthonormality error: max of | |row| - 1 | and |row_i . row_j|.
pub fn ortho_err(r: &[[f32; 3]; 3]) -> f32 {
    let mut e = 0.0f32;
    for i in 0..3 {
        e = e.max((dot(&r[i], &r[i]).sqrt() - 1.0).abs());
        for j in i + 1..3 {
            e = e.max(dot(&r[i], &r[j]).abs());
        }
    }
    e
}

/// Per-sample plausibility (no history).
pub fn classify(s: &CamSample, p: &GateParams) -> Why {
    let all = s.rows.iter().flatten().chain(&s.pos).chain(&s.chest).chain(std::iter::once(&s.fov));
    if all.into_iter().any(|v| !v.is_finite()) {
        return Why::NonFinite;
    }
    let ident = s.rows == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    if ident && s.pos == [0.0, 0.0, 0.0] {
        return Why::Identity;
    }
    if ortho_err(&s.rows) > 1e-3 {
        return Why::Ortho;
    }
    let det = det3(&s.rows);
    let d = if p.det_sign == 0.0 { det.abs() } else { det * p.det_sign };
    if !(0.99..=1.01).contains(&d) {
        return Why::Det;
    }
    if !(0.2..=3.0).contains(&s.fov) {
        return Why::Fov;
    }
    let cd = dist(&s.pos, &s.chest);
    let lo = if p.close_ok { 0.0 } else { p.min_dist_m };
    if !(lo..=p.max_dist_m).contains(&cd) {
        return Why::Dist;
    }
    Why::Ok
}

#[derive(Clone, Debug, Default)]
pub struct CamGate {
    good: u32,
    last_cam: Option<[f32; 3]>,
    last_player: Option<[f32; 3]>,
    valid: bool,
}

impl CamGate {
    pub fn reset(&mut self) {
        *self = CamGate::default();
    }

    pub fn valid(&self) -> bool {
        self.valid
    }

    /// Feeds one tick's sample (None = no camera this tick). Returns (valid now, reason).
    pub fn update(&mut self, s: Option<&CamSample>, p: &GateParams) -> (bool, Why) {
        let Some(s) = s else {
            self.good = 0;
            self.valid = false;
            return (false, Why::NonFinite);
        };
        // A re-base (Havok block change / teleport) moves the player or the camera by meters in one tick.
        let jumped = self.last_cam.is_some_and(|c| dist(&c, &s.pos) > p.jump_m)
            || self.last_player.is_some_and(|c| dist(&c, &s.player) > p.jump_m);
        if s.pos.iter().chain(&s.player).all(|v| v.is_finite()) {
            self.last_cam = Some(s.pos);
            self.last_player = Some(s.player);
        }
        if jumped {
            self.good = 0;
        }
        let why = classify(s, p);
        if why == Why::Ok {
            self.good = self.good.saturating_add(1);
        } else {
            self.good = 0;
        }
        self.valid = self.good >= p.ticks_needed.max(1);
        (self.valid, if why == Why::Ok && !self.valid { Why::Settling } else { why })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(pos: [f32; 3], chest: [f32; 3]) -> CamSample {
        // a yaw of 30 degrees: rows right / up / forward, det +1
        let (s, c) = (0.5f32, 0.866_025_4f32);
        CamSample { rows: [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]], pos, fov: 0.838, chest, player: chest }
    }

    #[test]
    fn cam_gate_rejects_identity_placeholder() {
        let p = GateParams::default();
        let mut s = cam([0.0; 3], [100.0, 1.2, 100.0]);
        s.rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(classify(&s, &p), Why::Identity);
        let mut g = CamGate::default();
        for _ in 0..10 {
            assert!(!g.update(Some(&s), &p).0);
        }
        let mut bad = cam([1.0, 2.0, 3.0], [1.0, 1.2, 0.0]);
        bad.rows[0] = [2.0, 0.0, 0.0];
        assert_eq!(classify(&bad, &p), Why::Ortho);
        bad = cam([1.0, 2.0, 3.0], [1.0, 1.2, 0.0]);
        bad.rows[2] = [-bad.rows[2][0], -bad.rows[2][1], -bad.rows[2][2]];
        assert_eq!(classify(&bad, &p), Why::Ok, "default: either det sign");
        assert_eq!(classify(&bad, &GateParams { det_sign: 1.0, ..p }), Why::Det);
        assert_eq!(classify(&bad, &GateParams { det_sign: -1.0, ..p }), Why::Ok);
        bad.rows[2] = [bad.rows[2][0] * 0.5, bad.rows[2][1] * 0.5, bad.rows[2][2] * 0.5];
        assert_eq!(classify(&bad, &p), Why::Ortho);
        bad = cam([1.0, 2.0, 3.0], [1.0, 1.2, 0.0]);
        bad.fov = 48.0;
        assert_eq!(classify(&bad, &p), Why::Fov);
        bad = cam([1.0, 2.0, 3.0], [1.0, 1.2, 0.0]);
        bad.pos[0] = f32::NAN;
        assert_eq!(classify(&bad, &p), Why::NonFinite);
    }

    #[test]
    fn cam_gate_three_tick_hysteresis() {
        let p = GateParams::default();
        let mut g = CamGate::default();
        let s = cam([10.0, 2.5, 10.0], [10.0, 1.2, 13.0]);
        assert_eq!(g.update(Some(&s), &p), (false, Why::Settling));
        assert_eq!(g.update(Some(&s), &p), (false, Why::Settling));
        assert_eq!(g.update(Some(&s), &p), (true, Why::Ok));
        // one bad tick drops it and needs 3 good ones again
        assert!(!g.update(None, &p).0);
        assert!(!g.update(Some(&s), &p).0);
        assert!(!g.update(Some(&s), &p).0);
        assert!(g.update(Some(&s), &p).0);
    }

    #[test]
    fn cam_gate_stale_camera_settles_only_after_rebase() {
        // Session-1 pattern: the camera from before the load stays ~170 m from the player for a few ticks,
        // then jumps to the player (re-base). Shots must stay blocked until 3 good ticks after the jump.
        let p = GateParams::default();
        let mut g = CamGate::default();
        let player = [500.0, 10.0, 500.0];
        let stale = cam([380.0, 12.0, 380.0], [player[0], player[1] + 1.2, player[2]]);
        for _ in 0..5 {
            assert_eq!(g.update(Some(&stale), &p), (false, Why::Dist));
        }
        let good = cam([player[0], player[1] + 2.5, player[2] - 3.4], [player[0], player[1] + 1.2, player[2]]);
        assert!(!g.update(Some(&good), &p).0);
        assert!(!g.update(Some(&good), &p).0);
        assert!(g.update(Some(&good), &p).0);
    }

    #[test]
    fn cam_gate_close_camera_ok_only_while_first_person_drives() {
        // run 18: ER's camera pulled to 0.29 m / 0.08 m from the chest by its collision against a wagon
        let p = GateParams::default();
        let fp = GateParams { close_ok: true, ..p };
        for d in [0.29f32, 0.08, 0.0] {
            let s = cam([10.0, 9.7, 10.0 + d], [10.0, 9.7, 10.0]);
            assert_eq!(classify(&s, &p), Why::Dist, "{d} m rejected in third person");
            assert_eq!(classify(&s, &fp), Why::Ok, "{d} m accepted while first person drives");
        }
        // absurd distances / a stale camera / the placeholder are still rejected
        assert_eq!(classify(&cam([10.0, 2.5, 40.0], [10.0, 1.2, 13.0]), &fp), Why::Dist, "27 m");
        assert_eq!(classify(&cam([10.0, 2.5, 180.0], [10.0, 1.2, 13.0]), &fp), Why::Dist, "167 m");
        let mut ph = cam([0.0; 3], [0.1, 0.0, 0.1]);
        ph.rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(classify(&ph, &fp), Why::Identity);
        // the re-base grace: a 32 m chunk jump still restarts the 3-tick settle with close_ok
        let mut g = CamGate::default();
        let a = cam([10.0, 9.7, 10.2], [10.0, 9.7, 10.0]);
        for _ in 0..3 {
            g.update(Some(&a), &fp);
        }
        assert!(g.valid());
        let b = cam([42.0, 9.7, 10.2], [42.0, 9.7, 10.0]);
        assert_eq!(g.update(Some(&b), &fp), (false, Why::Settling));
        g.update(Some(&b), &fp);
        assert!(g.update(Some(&b), &fp).0);
        // a close camera for 16 ticks (run 18) keeps the gate valid while first person drives
        for _ in 0..16 {
            assert!(g.update(Some(&b), &fp).0);
        }
    }

    #[test]
    fn cam_gate_resets_on_jump() {
        let p = GateParams::default();
        let mut g = CamGate::default();
        let a = cam([10.0, 2.5, 10.0], [10.0, 1.2, 13.0]);
        for _ in 0..3 {
            g.update(Some(&a), &p);
        }
        assert!(g.valid());
        let b = cam([30.0, 2.5, 10.0], [30.0, 1.2, 13.0]);
        assert_eq!(g.update(Some(&b), &p), (false, Why::Settling), "a 20 m jump restarts the count");
        g.update(Some(&b), &p);
        assert!(g.update(Some(&b), &p).0);
        g.reset();
        assert!(!g.valid());
    }
}
