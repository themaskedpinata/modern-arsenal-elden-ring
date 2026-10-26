//! Model of `converted\movement.json` (docs/M5_DESIGN.md section 1.5), written by
//! `tools\scripts\convert_movement.py` from the user's MW2 dumps and a static read of `iw4sp.exe`.
//!
//! Every constant is `{ "value": .., "unit": .., "src": .., "label": "V" | "D" | "I" }` (V byte-checked by the converter,
//! D decoded by the research disassembly only, I inferred). The DLL loads the file at boot; a
//! missing file or a missing/invalid constant means the controller stays `Off(no_data)` (no typed fallbacks, design
//! section 16 rule 5). The numbers are IW4 units (inches, seconds, ms) exactly as MW2 uses them.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One constant with its provenance.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Const {
    pub value: f64,
    pub unit: String,
    pub src: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct InputFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct WeaponBlock {
    pub name: String,
    pub src: String,
    pub fields: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Curves {
    pub src: String,
    /// `[percent, inches]` points, 0..100 percent.
    pub stand_crouch: Vec<[f64; 2]>,
    pub crouch_stand: Vec<[f64; 2]>,
    pub crouch_prone: Vec<[f64; 2]>,
    pub prone_crouch: Vec<[f64; 2]>,
}

/// The raw file.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MovementFile {
    pub schema: u32,
    pub generated_by: String,
    pub exe_sha256: String,
    pub inputs: BTreeMap<String, InputFile>,
    pub constants: BTreeMap<String, Const>,
    pub eye_curves: Curves,
    pub weapon: WeaponBlock,
    pub binds: BTreeMap<String, String>,
    /// M8 (docs/M8_DESIGN.md section 1): the MW2 SP mantle block. Missing (an M5-M7 file) = no mantling.
    #[serde(default)]
    pub mantle: Option<MantleFile>,
}

/// One row of the transition table (iw4sp.exe 0x1403BCBD0).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MantleTableRow {
    pub row: u32,
    pub height: f64,
    pub up: String,
    pub over: String,
    /// "drop" (quick drop to no weapon) | "keep".
    pub weapon: String,
    pub src: String,
    pub label: String,
}

/// One SP `player_mantle_*` xanim: per-frame delta translation normalized by its end (frames 0..=frames).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MantleAnim {
    pub frames: u32,
    pub fps: f64,
    pub dur_ms: f64,
    /// End delta (x forward, y side, z up), inches.
    pub end: [f64; 3],
    pub x: Vec<f64>,
    pub z: Vec<f64>,
    pub sha256: String,
    pub src: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QuickTimes {
    pub drop: f64,
    pub raise: f64,
    pub src: String,
    pub label: String,
    /// M8 review: the gun's quickDropAnim / quickRaiseAnim (V, its weapon file) and whether each is the same xanim
    /// as its dropAnim / raiseAnim (the putaway / pullout clip the viewmodel converter loaded).
    #[serde(default)]
    pub drop_anim: Option<String>,
    #[serde(default)]
    pub raise_anim: Option<String>,
    #[serde(default)]
    pub drop_is_putaway: Option<bool>,
    #[serde(default)]
    pub raise_is_pullout: Option<bool>,
}

/// M8 review: MW2's quick clips of one gun (V names) and whether our putaway / pullout clip is that xanim.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuickClips {
    pub drop_anim: String,
    pub raise_anim: String,
    pub drop_exact: bool,
    pub raise_exact: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MantleFile {
    pub src: String,
    pub constants: BTreeMap<String, Const>,
    pub table: Vec<MantleTableRow>,
    pub anims: BTreeMap<String, MantleAnim>,
    pub weapon_quick: BTreeMap<String, QuickTimes>,
    /// The MP set (333-833 ms), recorded for information only; never read by the controller.
    pub info_mp: serde_json::Value,
    pub curves_sha256: String,
    /// The xanim files read (path, sha256).
    #[serde(default)]
    pub inputs: BTreeMap<String, InputFile>,
}

/// One table row, typed: the rise curve (z, frames 0..=k) and the cross curve (x, frames k..=n) of its up anim.
#[derive(Clone, Debug, PartialEq)]
pub struct MantleRow {
    pub height: f32,
    /// Normalized rise samples (0 -> 1), uniform in time over `rise_s`.
    pub rise: Vec<f32>,
    /// Normalized forward samples (0 -> 1), uniform in time over `cross_s`.
    pub cross: Vec<f32>,
    /// 0 high, 1 mid, 2 low (the over anim of the row).
    pub over: usize,
    /// MW2 quick-drops the weapon on this row (BG_IsWeaponInactive: over anim != over_low).
    pub drop_weapon: bool,
}

/// The typed mantle data (IW4 inches / seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct MantleData {
    pub enable: bool,
    pub check_range: f32,
    pub check_radius: f32,
    pub check_angle_deg: f32,
    pub yawcap_deg: f32,
    pub max_vert_end_vel: f32,
    /// Highest test height above the current feet (ledge_h_60).
    pub max_rise: f32,
    /// The ledge point is `forward` in ahead of the origin (16).
    pub forward: f32,
    /// The ledge floor: the top must rise more than this (18).
    pub rise_min: f32,
    pub player_radius: f32,
    /// Clearance box height at the ledge (crouch hull, 50).
    pub clear_height: f32,
    pub over_forward: f32,
    /// The over anim's drop (18, positive).
    pub over_drop: f32,
    pub rows: Vec<MantleRow>,
    /// The over anims' forward curve (x, frames 0..=k), high / mid / low.
    pub over_x: [Vec<f32>; 3],
    /// Time of the rise (frames 0..k of the up anims), the forward part after it, the whole up and over anims (s).
    pub rise_s: f32,
    pub cross_s: f32,
    pub up_s: f32,
    pub over_s: f32,
    /// Over anim forward time (frames 0..k).
    pub over_cross_s: f32,
    /// gun name -> (quickDropTime, quickRaiseTime) s.
    pub quick: BTreeMap<String, (f32, f32)>,
    /// gun name -> its quick clips (empty for a file before the M8 review).
    pub quick_clips: BTreeMap<String, QuickClips>,
    /// The user's cap (I): top above the ground left, metres.
    pub max_height_m: f32,
    pub curves_sha: String,
    pub labels: BTreeMap<String, String>,
}

impl MantleData {
    /// The quick drop / raise of `gun` (`*_mp` name), else the UMP45's, else MW2's common 0.25 / 0.55 s.
    pub fn quick_times(&self, gun: &str) -> (f32, f32) {
        self.quick.get(gun).or_else(|| self.quick.get("ump45_mp")).copied().unwrap_or((0.25, 0.55))
    }

    /// MW2's quick clips of `gun`, if the data has them.
    pub fn quick_clips(&self, gun: &str) -> Option<&QuickClips> {
        self.quick_clips.get(gun)
    }

    /// The table row nearest to `rise` (inches): MW2 picks argmin |H - rise|.
    pub fn row_for(&self, rise: f32) -> usize {
        let mut best = 0;
        for (i, r) in self.rows.iter().enumerate() {
            if (r.height - rise).abs() < (self.rows[best].height - rise).abs() {
                best = i;
            }
        }
        best
    }

    fn from_file(m: &MantleFile) -> Result<MantleData, String> {
        for (k, c) in &m.constants {
            if !matches!(c.label.as_str(), "V" | "D" | "I") {
                return Err(format!("mantle constant {k}: label {:?} must be V, D or I", c.label));
            }
            if !c.value.is_finite() || c.src.trim().is_empty() || c.unit.trim().is_empty() {
                return Err(format!("mantle constant {k}: value/src/unit invalid"));
            }
        }
        let g = |n: &str| -> Result<f32, String> { m.constants.get(n).map(|c| c.value as f32).ok_or_else(|| format!("missing mantle constant {n}")) };
        let anim = |n: &str| -> Result<&MantleAnim, String> {
            let a = m.anims.get(n).ok_or_else(|| format!("missing mantle anim {n}"))?;
            if a.label != "V" || a.frames < 4 || a.x.len() != a.frames as usize + 1 || a.z.len() != a.x.len() || !(a.fps > 0.0) {
                return Err(format!("mantle anim {n}: bad shape or label"));
            }
            if a.x.iter().chain(a.z.iter()).any(|v| !v.is_finite() || !(-0.01..=1.01).contains(v)) {
                return Err(format!("mantle anim {n}: curve value outside 0..1"));
            }
            Ok(a)
        };
        // the frame where the forward motion starts (the rise ends): the last frame with x ~ 0
        let split = |a: &MantleAnim| -> usize { a.x.iter().rposition(|v| v.abs() < 1e-3).unwrap_or(0) };
        let mut rows = Vec::new();
        if m.table.len() != 7 {
            return Err(format!("mantle table has {} rows (want 7)", m.table.len()));
        }
        let mut rise_frames = None;
        for (i, t) in m.table.iter().enumerate() {
            if t.row as usize != i || !matches!(t.weapon.as_str(), "drop" | "keep") || !matches!(t.label.as_str(), "V" | "D" | "I") {
                return Err(format!("mantle table row {i}: bad row/weapon/label"));
            }
            let a = anim(&t.up)?;
            let k = split(a);
            if k < 2 || k + 2 > a.frames as usize {
                return Err(format!("mantle anim {}: no rise/forward split", t.up));
            }
            if *rise_frames.get_or_insert((k, a.frames, a.fps)) != (k, a.frames, a.fps) {
                return Err("mantle up anims differ in their rise/forward split or length".into());
            }
            let rise: Vec<f32> = a.z[..=k].iter().map(|v| *v as f32).collect();
            let cross: Vec<f32> = a.x[k..].iter().map(|v| *v as f32).collect();
            let over = match t.over.rsplit('_').next() {
                Some("high") => 0,
                Some("mid") => 1,
                Some("low") => 2,
                _ => return Err(format!("mantle table row {i}: over anim {}", t.over)),
            };
            rows.push(MantleRow { height: t.height as f32, rise, cross, over, drop_weapon: t.weapon == "drop" });
        }
        let (k, n, fps) = rise_frames.unwrap_or((10, 15, 30.0));
        let mut over_x: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut over_k = 10usize;
        let mut over_n = 20u32;
        for (j, name) in ["player_mantle_over_high", "player_mantle_over_mid", "player_mantle_over_low"].iter().enumerate() {
            let a = anim(name)?;
            // forward until x reaches 1, then the drop
            let kx = a.x.iter().position(|v| (*v - 1.0).abs() < 1e-3).unwrap_or(a.frames as usize);
            if kx < 2 {
                return Err(format!("mantle anim {name}: no forward part"));
            }
            over_k = kx;
            over_n = a.frames;
            over_x[j] = a.x[..=kx].iter().map(|v| *v as f32).collect();
        }
        let quick = m.weapon_quick.iter().map(|(k, q)| (k.clone(), (q.drop as f32, q.raise as f32))).collect();
        let quick_clips = m
            .weapon_quick
            .iter()
            .filter_map(|(k, q)| {
                Some((
                    k.clone(),
                    QuickClips {
                        drop_anim: q.drop_anim.clone()?,
                        raise_anim: q.raise_anim.clone()?,
                        drop_exact: q.drop_is_putaway.unwrap_or(false),
                        raise_exact: q.raise_is_pullout.unwrap_or(false),
                    },
                ))
            })
            .collect();
        let d = MantleData {
            enable: g("mantle_enable")? != 0.0,
            check_range: g("mantle_check_range")?,
            check_radius: g("mantle_check_radius")?,
            check_angle_deg: g("mantle_check_angle")?,
            yawcap_deg: g("mantle_view_yawcap")?,
            max_vert_end_vel: g("mantle_max_vertical_end_velocity")?,
            max_rise: g("ledge_h_60")?,
            forward: g("ledge_forward")?,
            rise_min: g("ledge_floor")?,
            player_radius: g("player_radius")?,
            clear_height: g("ledge_clear_height")?,
            over_forward: g("over_forward")?,
            over_drop: g("over_drop")?,
            rows,
            over_x,
            rise_s: k as f32 / fps as f32,
            cross_s: (n as usize - k) as f32 / fps as f32,
            up_s: n as f32 / fps as f32,
            over_s: over_n as f32 / fps as f32,
            over_cross_s: over_k as f32 / fps as f32,
            quick,
            quick_clips,
            max_height_m: g("max_height_above_ground_m")?,
            curves_sha: m.curves_sha256.clone(),
            labels: m.constants.iter().map(|(k, c)| (k.clone(), c.label.clone())).collect(),
        };
        if !(d.rise_min > 0.0 && d.max_rise > d.rise_min && d.forward > 0.0 && d.check_range >= 0.0 && d.over_forward > 0.0) {
            return Err("mantle geometry implausible".into());
        }
        if (d.up_s - 0.5).abs() > 0.05 || (d.over_s - 0.6667).abs() > 0.05 {
            return Err(format!("mantle durations {:.3}/{:.3} s are not the SP 500/667 ms", d.up_s, d.over_s));
        }
        if d.rows.windows(2).any(|w| w[0].height <= w[1].height) {
            return Err("mantle table heights must descend".into());
        }
        Ok(d)
    }

    /// The `MOVE data ... mantle` part of the data line.
    pub fn summary(&self) -> String {
        let v = |k: &str| self.labels.get(k).map_or("?", |s| s.as_str());
        format!(
            "mantle dvars={} rows={} up={:.0}ms rise={:.0}ms over={:.0}ms range={}in angle={} rise={}..{}in curves_sha={}",
            v("mantle_check_range"),
            self.rows.len(),
            self.up_s * 1000.0,
            self.rise_s * 1000.0,
            self.over_s * 1000.0,
            self.check_range,
            self.check_angle_deg,
            self.rise_min,
            self.max_rise,
            &self.curves_sha[..self.curves_sha.len().min(12)]
        )
    }
}

/// Weapon fields the controller, the viewmodel pose and the spread need (UMP45 from its weapon file).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponMove {
    pub move_speed_scale: f32,
    pub ads_move_speed_scale: f32,
    pub sprint_duration_scale: f32,
    pub sprint_in_s: f32,
    pub sprint_loop_s: f32,
    pub sprint_out_s: f32,
    pub spread_stand: [f32; 2],
    pub spread_ducked: [f32; 2],
    pub spread_prone: [f32; 2],
    pub spread_decay_rate: f32,
    pub spread_fire_add: f32,
    pub spread_move_add: f32,
    pub spread_ducked_decay: f32,
    pub spread_prone_decay: f32,
    /// View-space offsets in inches, (forward, right, up).
    pub ducked_ofs: [f32; 3],
    pub prone_ofs: [f32; 3],
    pub stand_move: [f32; 3],
    pub ducked_move: [f32; 3],
    pub prone_move: [f32; 3],
    pub strafe_move: [f32; 3],
    /// (pitch, yaw, roll) degrees.
    pub strafe_rot: [f32; 3],
    pub prone_rot: [f32; 3],
    pub pos_move_rate: f32,
    pub pos_prone_move_rate: f32,
    pub pos_rot_rate: f32,
    pub pos_prone_rot_rate: f32,
    pub idle_crouch_factor: f32,
    pub idle_prone_factor: f32,
    pub ads_bob_factor: f32,
    pub ads_view_bob_mult: f32,
}

/// Every MW2 number of table 2.1 the controller uses, typed (IW4 units).
#[derive(Clone, Debug, PartialEq)]
pub struct Mw2 {
    pub g_speed: f32,
    pub sprint_speed_scale: f32,
    pub back_speed_scale: f32,
    pub strafe_speed_scale: f32,
    pub sprint_strafe_speed_scale: f32,
    pub crouch_speed_scale: f32,
    pub prone_speed_scale: f32,
    pub ads_walk_scale: f32,
    pub gravity: f32,
    pub jump_height: f32,
    pub jump_min_interval_ms: f32,
    pub slowdown_low_band: f32,
    pub slowdown_low_scale: f32,
    pub slowdown_low_ms: f32,
    pub slowdown_high_scale: f32,
    pub slowdown_high_ms: f32,
    pub land_factor_full_ms: f32,
    pub land_factor_max: f32,
    pub friction: f32,
    pub stopspeed: f32,
    pub land_friction_scale: f32,
    pub accel_stand: f32,
    pub accel_crouch: f32,
    pub accel_prone: f32,
    pub time_land_accel_scale: f32,
    pub air_accel: f32,
    pub step_height: f32,
    pub step_height_prone: f32,
    pub walkable_normal_y: f32,
    pub eye_stand: f32,
    pub eye_crouch: f32,
    pub eye_prone: f32,
    pub hull_stand: f32,
    pub hull_crouch: f32,
    pub hull_prone: f32,
    pub stand_crouch_ms: f32,
    pub crouch_prone_ms: f32,
    pub sprint_time_s: f32,
    pub sprint_time_max_s: f32,
    pub sprint_min_time_s: f32,
    pub sprint_recharge_pause_s: f32,
    pub sprint_forward_min: f32,
    pub fall_min: f32,
    pub fall_max: f32,
    pub land_slow_min: f32,
    pub land_slow_scale: f32,
    pub time_land_base_ms: f32,
    pub time_land_per_pct_ms: f32,
    pub time_land_max_ms: f32,
    pub land_vel_scale_short: f32,
    pub land_vel_scale_long: f32,
    pub view_dip_max: f32,
    pub jump_spread_add: f32,
    pub aim_spread_move_threshold: f32,
    pub run_threshold: f32,
    pub move_threshold: f32,
    pub sprint_camera_bob: f32,
    pub lightweight_scale: f32,
    pub class_smg: f32,
    pub class_rifle: f32,
    pub class_mg: f32,
    pub class_launcher: f32,
    /// Weapon bob amplitudes (x, y) per stance: stand, ducked, prone, sprint.
    pub weap_bob: [[f32; 2]; 4],
    pub weap_bob_max: f32,
    pub weap_bob_lag: f32,
    /// View bob amplitudes (x, y) per stance: stand, ducked, prone, sprint.
    pub view_bob: [[f32; 2]; 4],
    pub cl_stance_hold_ms: f32,
}

/// The loaded data: typed numbers, the eye curves (percent, inches), the weapon and the provenance for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveData {
    pub mw2: Mw2,
    pub weapon: WeaponMove,
    pub weapon_name: String,
    pub curves: [Vec<(f32, f32)>; 4],
    pub exe_sha256: String,
    pub labels: BTreeMap<String, String>,
    pub binds: BTreeMap<String, String>,
    /// M8: the SP mantle data (`None` = an M5-M7 file: no mantling).
    pub mantle: Option<MantleData>,
}

/// Indices into [`MoveData::curves`].
pub const CURVE_STAND_CROUCH: usize = 0;
pub const CURVE_CROUCH_STAND: usize = 1;
pub const CURVE_CROUCH_PRONE: usize = 2;
pub const CURVE_PRONE_CROUCH: usize = 3;

/// The constant names `MoveData::from_json` requires (the converter writes exactly these).
pub const REQUIRED: &[&str] = &[
    "g_speed",
    "sprint_speed_scale",
    "back_speed_scale",
    "strafe_speed_scale",
    "sprint_strafe_speed_scale",
    "crouch_speed_scale",
    "prone_speed_scale",
    "ads_walk_scale",
    "gravity",
    "jump_height",
    "jump_min_interval_ms",
    "slowdown_low_band",
    "slowdown_low_scale",
    "slowdown_low_ms",
    "slowdown_high_scale",
    "slowdown_high_ms",
    "land_factor_full_ms",
    "land_factor_max",
    "friction",
    "stopspeed",
    "land_friction_scale",
    "accel_stand",
    "accel_crouch",
    "accel_prone",
    "time_land_accel_scale",
    "air_accel",
    "step_height",
    "step_height_prone",
    "walkable_normal_y",
    "eye_stand",
    "eye_crouch",
    "eye_prone",
    "hull_stand",
    "hull_crouch",
    "hull_prone",
    "stand_crouch_ms",
    "crouch_prone_ms",
    "sprint_time_s",
    "sprint_time_max_s",
    "sprint_min_time_s",
    "sprint_recharge_pause_s",
    "sprint_forward_min",
    "fall_min",
    "fall_max",
    "land_slow_min",
    "land_slow_scale",
    "time_land_base_ms",
    "time_land_per_pct_ms",
    "time_land_max_ms",
    "land_vel_scale_short",
    "land_vel_scale_long",
    "view_dip_max",
    "jump_spread_add",
    "aim_spread_move_threshold",
    "run_threshold",
    "move_threshold",
    "sprint_camera_bob",
    "lightweight_scale",
    "class_smg",
    "class_rifle",
    "class_mg",
    "class_launcher",
    "bob_stand_x",
    "bob_stand_y",
    "bob_ducked_x",
    "bob_ducked_y",
    "bob_prone_x",
    "bob_prone_y",
    "bob_sprint_x",
    "bob_sprint_y",
    "bob_max",
    "bob_lag",
    "view_bob_stand_x",
    "view_bob_stand_y",
    "view_bob_ducked_x",
    "view_bob_ducked_y",
    "view_bob_prone_x",
    "view_bob_prone_y",
    "view_bob_sprint_x",
    "view_bob_sprint_y",
    "cl_stance_hold_ms",
];

/// Weapon-file fields the data must carry.
pub const WEAPON_FIELDS: &[&str] = &[
    "moveSpeedScale",
    "adsMoveSpeedScale",
    "sprintDurationScale",
    "sprintInTime",
    "sprintLoopTime",
    "sprintOutTime",
    "hipSpreadStandMin",
    "hipSpreadMax",
    "hipSpreadDuckedMin",
    "hipSpreadDuckedMax",
    "hipSpreadProneMin",
    "hipSpreadProneMax",
    "hipSpreadDecayRate",
    "hipSpreadFireAdd",
    "hipSpreadMoveAdd",
    "hipSpreadDuckedDecay",
    "hipSpreadProneDecay",
    "duckedOfsF",
    "duckedOfsR",
    "duckedOfsU",
    "proneOfsF",
    "proneOfsR",
    "proneOfsU",
    "standMoveF",
    "standMoveR",
    "standMoveU",
    "duckedMoveF",
    "duckedMoveR",
    "duckedMoveU",
    "proneMoveF",
    "proneMoveR",
    "proneMoveU",
    "strafeMoveF",
    "strafeMoveR",
    "strafeMoveU",
    "strafeRotP",
    "strafeRotY",
    "strafeRotR",
    "proneRotP",
    "proneRotY",
    "proneRotR",
    "posMoveRate",
    "posProneMoveRate",
    "posRotRate",
    "posProneRotRate",
    "idleCrouchFactor",
    "idleProneFactor",
    "adsBobFactor",
    "adsViewBobMult",
];

fn curve(name: &str, pts: &[[f64; 2]]) -> Result<Vec<(f32, f32)>, String> {
    if pts.len() < 2 {
        return Err(format!("eye curve {name}: fewer than 2 points"));
    }
    let mut out = Vec::with_capacity(pts.len());
    let mut last = -1.0;
    for p in pts {
        if !(p[0].is_finite() && p[1].is_finite()) || p[0] <= last || !(0.0..=100.0).contains(&p[0]) || !(0.0..=100.0).contains(&p[1]) {
            return Err(format!("eye curve {name}: bad point {p:?}"));
        }
        last = p[0];
        out.push((p[0] as f32, p[1] as f32));
    }
    if out[0].0 != 0.0 || out[out.len() - 1].0 != 100.0 {
        return Err(format!("eye curve {name}: must span 0..100 percent"));
    }
    Ok(out)
}

impl MoveData {
    /// Parses and validates the json. `Err` = the controller stays `Off(no_data)`.
    pub fn from_json(text: &str) -> Result<MoveData, String> {
        let f: MovementFile = serde_json::from_str(text).map_err(|e| format!("movement.json: {e}"))?;
        Self::from_file(&f)
    }

    pub fn from_file(f: &MovementFile) -> Result<MoveData, String> {
        if f.schema != 1 {
            return Err(format!("movement.json schema {} unsupported (want 1)", f.schema));
        }
        for (k, c) in &f.constants {
            if !matches!(c.label.as_str(), "V" | "D" | "I") {
                return Err(format!("constant {k}: label {:?} must be V, D or I", c.label));
            }
            if !c.value.is_finite() {
                return Err(format!("constant {k}: value not finite"));
            }
            if c.src.trim().is_empty() || c.unit.trim().is_empty() {
                return Err(format!("constant {k}: src/unit missing"));
            }
        }
        let g = |name: &str| -> Result<f32, String> {
            f.constants.get(name).map(|c| c.value as f32).ok_or_else(|| format!("missing constant {name}"))
        };
        for n in REQUIRED {
            g(n)?;
        }
        let w = |name: &str| -> Result<f32, String> {
            f.weapon.fields.get(name).map(|v| *v as f32).filter(|v| v.is_finite()).ok_or_else(|| format!("missing weapon field {name}"))
        };
        for n in WEAPON_FIELDS {
            w(n)?;
        }
        let mw2 = Mw2 {
            g_speed: g("g_speed")?,
            sprint_speed_scale: g("sprint_speed_scale")?,
            back_speed_scale: g("back_speed_scale")?,
            strafe_speed_scale: g("strafe_speed_scale")?,
            sprint_strafe_speed_scale: g("sprint_strafe_speed_scale")?,
            crouch_speed_scale: g("crouch_speed_scale")?,
            prone_speed_scale: g("prone_speed_scale")?,
            ads_walk_scale: g("ads_walk_scale")?,
            gravity: g("gravity")?,
            jump_height: g("jump_height")?,
            jump_min_interval_ms: g("jump_min_interval_ms")?,
            slowdown_low_band: g("slowdown_low_band")?,
            slowdown_low_scale: g("slowdown_low_scale")?,
            slowdown_low_ms: g("slowdown_low_ms")?,
            slowdown_high_scale: g("slowdown_high_scale")?,
            slowdown_high_ms: g("slowdown_high_ms")?,
            land_factor_full_ms: g("land_factor_full_ms")?,
            land_factor_max: g("land_factor_max")?,
            friction: g("friction")?,
            stopspeed: g("stopspeed")?,
            land_friction_scale: g("land_friction_scale")?,
            accel_stand: g("accel_stand")?,
            accel_crouch: g("accel_crouch")?,
            accel_prone: g("accel_prone")?,
            time_land_accel_scale: g("time_land_accel_scale")?,
            air_accel: g("air_accel")?,
            step_height: g("step_height")?,
            step_height_prone: g("step_height_prone")?,
            walkable_normal_y: g("walkable_normal_y")?,
            eye_stand: g("eye_stand")?,
            eye_crouch: g("eye_crouch")?,
            eye_prone: g("eye_prone")?,
            hull_stand: g("hull_stand")?,
            hull_crouch: g("hull_crouch")?,
            hull_prone: g("hull_prone")?,
            stand_crouch_ms: g("stand_crouch_ms")?,
            crouch_prone_ms: g("crouch_prone_ms")?,
            sprint_time_s: g("sprint_time_s")?,
            sprint_time_max_s: g("sprint_time_max_s")?,
            sprint_min_time_s: g("sprint_min_time_s")?,
            sprint_recharge_pause_s: g("sprint_recharge_pause_s")?,
            sprint_forward_min: g("sprint_forward_min")?,
            fall_min: g("fall_min")?,
            fall_max: g("fall_max")?,
            land_slow_min: g("land_slow_min")?,
            land_slow_scale: g("land_slow_scale")?,
            time_land_base_ms: g("time_land_base_ms")?,
            time_land_per_pct_ms: g("time_land_per_pct_ms")?,
            time_land_max_ms: g("time_land_max_ms")?,
            land_vel_scale_short: g("land_vel_scale_short")?,
            land_vel_scale_long: g("land_vel_scale_long")?,
            view_dip_max: g("view_dip_max")?,
            jump_spread_add: g("jump_spread_add")?,
            aim_spread_move_threshold: g("aim_spread_move_threshold")?,
            run_threshold: g("run_threshold")?,
            move_threshold: g("move_threshold")?,
            sprint_camera_bob: g("sprint_camera_bob")?,
            lightweight_scale: g("lightweight_scale")?,
            class_smg: g("class_smg")?,
            class_rifle: g("class_rifle")?,
            class_mg: g("class_mg")?,
            class_launcher: g("class_launcher")?,
            weap_bob: [
                [g("bob_stand_x")?, g("bob_stand_y")?],
                [g("bob_ducked_x")?, g("bob_ducked_y")?],
                [g("bob_prone_x")?, g("bob_prone_y")?],
                [g("bob_sprint_x")?, g("bob_sprint_y")?],
            ],
            weap_bob_max: g("bob_max")?,
            weap_bob_lag: g("bob_lag")?,
            view_bob: [
                [g("view_bob_stand_x")?, g("view_bob_stand_y")?],
                [g("view_bob_ducked_x")?, g("view_bob_ducked_y")?],
                [g("view_bob_prone_x")?, g("view_bob_prone_y")?],
                [g("view_bob_sprint_x")?, g("view_bob_sprint_y")?],
            ],
            cl_stance_hold_ms: g("cl_stance_hold_ms")?,
        };
        // plausibility (a corrupt or mutated file must not drive the player)
        let pos = [
            ("g_speed", mw2.g_speed),
            ("gravity", mw2.gravity),
            ("jump_height", mw2.jump_height),
            ("friction", mw2.friction),
            ("stopspeed", mw2.stopspeed),
            ("accel_stand", mw2.accel_stand),
            ("step_height", mw2.step_height),
            ("eye_stand", mw2.eye_stand),
            ("hull_stand", mw2.hull_stand),
            ("sprint_time_s", mw2.sprint_time_s),
        ];
        for (n, v) in pos {
            if v <= 0.0 {
                return Err(format!("constant {n} = {v} must be > 0"));
            }
        }
        if !(mw2.eye_prone < mw2.eye_crouch && mw2.eye_crouch < mw2.eye_stand) {
            return Err("eye heights must be prone < crouch < stand".into());
        }
        if mw2.fall_max <= mw2.fall_min {
            return Err("fall_max must exceed fall_min".into());
        }
        let wf = WeaponMove {
            move_speed_scale: w("moveSpeedScale")?,
            ads_move_speed_scale: w("adsMoveSpeedScale")?,
            sprint_duration_scale: w("sprintDurationScale")?,
            sprint_in_s: w("sprintInTime")?,
            sprint_loop_s: w("sprintLoopTime")?,
            sprint_out_s: w("sprintOutTime")?,
            spread_stand: [w("hipSpreadStandMin")?, w("hipSpreadMax")?],
            spread_ducked: [w("hipSpreadDuckedMin")?, w("hipSpreadDuckedMax")?],
            spread_prone: [w("hipSpreadProneMin")?, w("hipSpreadProneMax")?],
            spread_decay_rate: w("hipSpreadDecayRate")?,
            spread_fire_add: w("hipSpreadFireAdd")?,
            spread_move_add: w("hipSpreadMoveAdd")?,
            spread_ducked_decay: w("hipSpreadDuckedDecay")?,
            spread_prone_decay: w("hipSpreadProneDecay")?,
            ducked_ofs: [w("duckedOfsF")?, w("duckedOfsR")?, w("duckedOfsU")?],
            prone_ofs: [w("proneOfsF")?, w("proneOfsR")?, w("proneOfsU")?],
            stand_move: [w("standMoveF")?, w("standMoveR")?, w("standMoveU")?],
            ducked_move: [w("duckedMoveF")?, w("duckedMoveR")?, w("duckedMoveU")?],
            prone_move: [w("proneMoveF")?, w("proneMoveR")?, w("proneMoveU")?],
            strafe_move: [w("strafeMoveF")?, w("strafeMoveR")?, w("strafeMoveU")?],
            strafe_rot: [w("strafeRotP")?, w("strafeRotY")?, w("strafeRotR")?],
            prone_rot: [w("proneRotP")?, w("proneRotY")?, w("proneRotR")?],
            pos_move_rate: w("posMoveRate")?,
            pos_prone_move_rate: w("posProneMoveRate")?,
            pos_rot_rate: w("posRotRate")?,
            pos_prone_rot_rate: w("posProneRotRate")?,
            idle_crouch_factor: w("idleCrouchFactor")?,
            idle_prone_factor: w("idleProneFactor")?,
            ads_bob_factor: w("adsBobFactor")?,
            ads_view_bob_mult: w("adsViewBobMult")?,
        };
        if wf.move_speed_scale <= 0.0 || wf.ads_move_speed_scale <= 0.0 || wf.sprint_duration_scale <= 0.0 {
            return Err("weapon move/ads/sprint scales must be > 0".into());
        }
        let c = &f.eye_curves;
        let curves = [
            curve("stand_crouch", &c.stand_crouch)?,
            curve("crouch_stand", &c.crouch_stand)?,
            curve("crouch_prone", &c.crouch_prone)?,
            curve("prone_crouch", &c.prone_crouch)?,
        ];
        let labels = f.constants.iter().map(|(k, c)| (k.clone(), c.label.clone())).collect();
        Ok(MoveData {
            mw2,
            weapon: wf,
            weapon_name: f.weapon.name.clone(),
            curves,
            exe_sha256: f.exe_sha256.clone(),
            labels,
            binds: f.binds.clone(),
            mantle: f.mantle.as_ref().map(MantleData::from_file).transpose()?,
        })
    }

    /// The `MOVE data ...` log line body.
    pub fn summary(&self) -> String {
        let m = &self.mw2;
        let w = &self.weapon;
        format!(
            "g_speed={} sprint={}x{:.1}s min={:.1} jump={}in grav={} accel={}/{}/{} air={} fric={}/{} step={}/{} fall={}/{} {} move={} ads={} dur={} src_exe_sha={}",
            m.g_speed,
            m.sprint_speed_scale,
            m.sprint_time_s,
            m.sprint_min_time_s,
            m.jump_height,
            m.gravity,
            m.accel_stand,
            m.accel_crouch,
            m.accel_prone,
            m.air_accel,
            m.friction,
            m.stopspeed,
            m.step_height,
            m.step_height_prone,
            m.fall_min,
            m.fall_max,
            self.weapon_name.trim_end_matches("_mp"),
            w.move_speed_scale,
            w.ads_move_speed_scale,
            w.sprint_duration_scale,
            &self.exe_sha256[..self.exe_sha256.len().min(12)]
        ) + &self.mantle.as_ref().map_or(String::from(" mantle=none"), |m| format!(" {}", m.summary()))
    }
}

/// A complete, valid data set with the design's MW2 numbers (table 2.1), for tests and the headless bench only. The
/// DLL never uses it: it loads `converted\movement.json` or stays `Off(no_data)`.
pub fn test_json() -> String {
    let consts: &[(&str, f64, &str)] = &[
        ("g_speed", 190.0, "in/s"),
        ("sprint_speed_scale", 1.5, "x"),
        ("back_speed_scale", 0.7, "x"),
        ("strafe_speed_scale", 0.8, "x"),
        ("sprint_strafe_speed_scale", 0.667, "x"),
        ("crouch_speed_scale", 0.65, "x"),
        ("prone_speed_scale", 0.15, "x"),
        ("ads_walk_scale", 0.4, "x"),
        ("gravity", 800.0, "in/s2"),
        ("jump_height", 39.0, "in"),
        ("jump_min_interval_ms", 500.0, "ms"),
        ("slowdown_low_band", 18.0, "in"),
        ("slowdown_low_scale", 0.65, "x"),
        ("slowdown_low_ms", 1800.0, "ms"),
        ("slowdown_high_scale", 0.5, "x"),
        ("slowdown_high_ms", 1200.0, "ms"),
        ("land_factor_full_ms", 1700.0, "ms"),
        ("land_factor_max", 2.5, "x"),
        ("friction", 5.5, "1/s"),
        ("stopspeed", 100.0, "in/s"),
        ("land_friction_scale", 0.3, "x"),
        ("accel_stand", 9.0, "1/s"),
        ("accel_crouch", 12.0, "1/s"),
        ("accel_prone", 19.0, "1/s"),
        ("time_land_accel_scale", 0.25, "x"),
        ("air_accel", 1.0, "1/s"),
        ("step_height", 18.0, "in"),
        ("step_height_prone", 10.0, "in"),
        ("walkable_normal_y", 0.7, "cos"),
        ("eye_stand", 60.0, "in"),
        ("eye_crouch", 40.0, "in"),
        ("eye_prone", 11.0, "in"),
        ("hull_stand", 70.0, "in"),
        ("hull_crouch", 50.0, "in"),
        ("hull_prone", 30.0, "in"),
        ("stand_crouch_ms", 200.0, "ms"),
        ("crouch_prone_ms", 400.0, "ms"),
        ("sprint_time_s", 4.0, "s"),
        ("sprint_time_max_s", 16.383, "s"),
        ("sprint_min_time_s", 1.0, "s"),
        ("sprint_recharge_pause_s", 0.0, "s"),
        ("sprint_forward_min", 105.0, "of 127"),
        ("fall_min", 200.0, "in"),
        ("fall_max", 350.0, "in"),
        ("land_slow_min", 12.0, "in"),
        ("land_slow_scale", 0.67, "x"),
        ("time_land_base_ms", 500.0, "ms"),
        ("time_land_per_pct_ms", 35.0, "ms"),
        ("time_land_max_ms", 2000.0, "ms"),
        ("land_vel_scale_short", 0.5, "x"),
        ("land_vel_scale_long", 0.2, "x"),
        ("view_dip_max", 24.0, "in"),
        ("jump_spread_add", 64.0, "/255"),
        ("aim_spread_move_threshold", 11.0, "in/s"),
        ("run_threshold", 110.0, "in/s"),
        ("move_threshold", 10.0, "in/s"),
        ("sprint_camera_bob", 0.5, "x"),
        ("lightweight_scale", 1.07, "x"),
        ("class_smg", 1.0, "x"),
        ("class_rifle", 0.95, "x"),
        ("class_mg", 0.875, "x"),
        ("class_launcher", 0.8, "x"),
        ("bob_stand_x", 0.055, "x"),
        ("bob_stand_y", 0.025, "x"),
        ("bob_ducked_x", 0.045, "x"),
        ("bob_ducked_y", 0.025, "x"),
        ("bob_prone_x", 0.02, "x"),
        ("bob_prone_y", 0.005, "x"),
        ("bob_sprint_x", 0.02, "x"),
        ("bob_sprint_y", 0.014, "x"),
        ("bob_max", 8.0, "in"),
        ("bob_lag", 0.25, "x"),
        ("view_bob_stand_x", 0.007, "x"),
        ("view_bob_stand_y", 0.007, "x"),
        ("view_bob_ducked_x", 0.0075, "x"),
        ("view_bob_ducked_y", 0.0075, "x"),
        ("view_bob_prone_x", 0.02, "x"),
        ("view_bob_prone_y", 0.005, "x"),
        ("view_bob_sprint_x", 0.02, "x"),
        ("view_bob_sprint_y", 0.014, "x"),
        ("cl_stance_hold_ms", 300.0, "ms"),
    ];
    let weapon: &[(&str, f64)] = &[
        ("moveSpeedScale", 1.0),
        ("adsMoveSpeedScale", 2.0),
        ("sprintDurationScale", 1.0),
        ("sprintInTime", 0.3),
        ("sprintLoopTime", 0.7),
        ("sprintOutTime", 0.3),
        ("hipSpreadStandMin", 2.0),
        ("hipSpreadMax", 5.0),
        ("hipSpreadDuckedMin", 1.75),
        ("hipSpreadDuckedMax", 4.5),
        ("hipSpreadProneMin", 1.5),
        ("hipSpreadProneMax", 4.0),
        ("hipSpreadDecayRate", 4.0),
        ("hipSpreadFireAdd", 0.52),
        ("hipSpreadMoveAdd", 4.0),
        ("hipSpreadDuckedDecay", 1.0),
        ("hipSpreadProneDecay", 1.0),
        ("duckedOfsF", 0.0),
        ("duckedOfsR", 0.8),
        ("duckedOfsU", 0.0),
        ("proneOfsF", 0.0),
        ("proneOfsR", 1.5),
        ("proneOfsU", 0.0),
        ("standMoveF", 0.0),
        ("standMoveR", 0.0),
        ("standMoveU", -0.5),
        ("duckedMoveF", -0.5),
        ("duckedMoveR", 0.0),
        ("duckedMoveU", 0.0),
        ("proneMoveF", -50.0),
        ("proneMoveR", 3.0),
        ("proneMoveU", -105.0),
        ("strafeMoveF", 0.0),
        ("strafeMoveR", 0.5),
        ("strafeMoveU", 0.0),
        ("strafeRotP", 0.0),
        ("strafeRotY", 0.0),
        ("strafeRotR", 5.0),
        ("proneRotP", 0.0),
        ("proneRotY", 280.0),
        ("proneRotR", -300.0),
        ("posMoveRate", 6.0),
        ("posProneMoveRate", 25.0),
        ("posRotRate", 6.0),
        ("posProneRotRate", 30.0),
        ("idleCrouchFactor", 0.75),
        ("idleProneFactor", 0.4),
        ("adsBobFactor", 1.0),
        ("adsViewBobMult", 0.25),
    ];
    let mut s = String::from("{\"schema\":1,\"generated_by\":\"test\",\"exe_sha256\":\"0000000000000000\",\"inputs\":{},\"constants\":{");
    for (i, (n, v, u)) in consts.iter().enumerate() {
        let label = match *n {
            "walkable_normal_y" | "ads_walk_scale" => "I",
            "bob_lag" | "time_land_base_ms" => "D",
            _ => "V",
        };
        s += &format!("{}\"{n}\":{{\"value\":{v},\"unit\":\"{u}\",\"src\":\"design table 2.1\",\"label\":\"{label}\"}}", if i > 0 { "," } else { "" });
    }
    s += "},\"eye_curves\":{\"src\":\"design section 3\",";
    s += "\"stand_crouch\":[[0,60],[1,59.5],[4,58.5],[30,56],[80,44],[90,41.5],[95,40.5],[100,40]],";
    s += "\"crouch_stand\":[[0,40],[5,40.5],[10,41.5],[20,44],[70,56],[96,58.5],[99,59.5],[100,60]],";
    s += "\"crouch_prone\":[[0,40],[11,38],[22,33],[34,25],[45,16],[50,15],[55,16],[70,18],[90,17],[100,11]],";
    s += "\"prone_crouch\":[[0,11],[5,10],[30,21],[50,25],[67,31],[83,34],[100,40]]},";
    s += "\"weapon\":{\"name\":\"ump45_mp\",\"src\":\"test\",\"fields\":{";
    for (i, (n, v)) in weapon.iter().enumerate() {
        s += &format!("{}\"{n}\":{v}", if i > 0 { "," } else { "" });
    }
    s += "}},\"binds\":{\"+breath_sprint\":\"SHIFT\",\"+gostand\":\"SPACE\",\"togglecrouch\":\"c\",\"toggleprone\":\"CTRL\"},";
    s += &test_mantle_json();
    s += "}";
    s
}

/// The `"mantle": {...}` block of [`test_json`]: the design's SP numbers (section 1) and the dumped per-frame curves of
/// up_57 / up_39 / up_21 and over_high (tests and the bench only).
pub fn test_mantle_json() -> String {
    let consts: &[(&str, f64, &str, &str)] = &[
        ("mantle_enable", 1.0, "bool", "V"),
        ("mantle_debug", 0.0, "bool", "V"),
        ("mantle_check_range", 20.0, "in", "V"),
        ("mantle_check_radius", 0.1, "in", "V"),
        ("mantle_check_angle", 60.0, "deg", "V"),
        ("mantle_view_yawcap", 60.0, "deg", "V"),
        ("mantle_max_vertical_end_velocity", 0.0, "in/s", "V"),
        ("ledge_h_60", 60.0, "in", "V"),
        ("ledge_h_40", 40.0, "in", "V"),
        ("ledge_h_20", 20.0, "in", "V"),
        ("ledge_forward", 16.0, "in", "V"),
        ("ledge_floor", 18.0, "in", "V"),
        ("player_radius", 15.0, "in", "V"),
        ("ledge_clear_height", 50.0, "in", "V"),
        ("over_forward", 31.0, "in", "V"),
        ("over_drop", 18.0, "in", "V"),
        ("max_height_above_ground_m", 2.0, "m", "I"),
        ("airborne_reach_in", 99.0, "in", "I"),
    ];
    let z57 = "[0,0.045,0.131,0.247,0.382,0.526,0.668,0.797,0.903,0.974,1,1,1,1,1,1]";
    let z39 = "[0,0.053,0.143,0.261,0.396,0.538,0.678,0.803,0.906,0.975,1,1,1,1,1,1]";
    let z21 = "[0,0.109,0.232,0.364,0.496,0.625,0.744,0.847,0.928,0.981,1,1,1,1,1,1]";
    let xup = "[0,0,0,0,0,0,0,0,0,0,0,0.073,0.258,0.507,0.771,1]";
    let xov = "[0,0.110,0.233,0.365,0.498,0.626,0.745,0.848,0.928,0.981,1,1,1,1,1,1,1,1,1,1,1]";
    let zov = "[0,0,0,0,0,0,0,0,0,0,0,0.019,0.072,0.153,0.256,0.375,0.504,0.636,0.768,0.891,1]";
    let mut s = String::from("\"mantle\":{\"src\":\"design section 1\",\"constants\":{");
    for (i, (n, v, u, l)) in consts.iter().enumerate() {
        s += &format!("{}\"{n}\":{{\"value\":{v},\"unit\":\"{u}\",\"src\":\"design section 1\",\"label\":\"{l}\"}}", if i > 0 { "," } else { "" });
    }
    s += "},\"table\":[";
    let rows = [(57, "high", "drop"), (51, "high", "drop"), (45, "mid", "drop"), (39, "mid", "drop"), (33, "mid", "drop"), (27, "low", "keep"), (21, "low", "keep")];
    for (i, (h, ov, wpn)) in rows.iter().enumerate() {
        s += &format!(
            "{}{{\"row\":{i},\"height\":{h},\"up\":\"player_mantle_up_{h}\",\"over\":\"player_mantle_over_{ov}\",\"weapon\":\"{wpn}\",\"src\":\"design 1.3\",\"label\":\"V\"}}",
            if i > 0 { "," } else { "" }
        );
    }
    s += "],\"anims\":{";
    for (i, (h, _, _)) in rows.iter().enumerate() {
        let z = if *h >= 45 { z57 } else if *h >= 33 { z39 } else { z21 };
        s += &format!(
            "{}\"player_mantle_up_{h}\":{{\"frames\":15,\"fps\":30,\"dur_ms\":500,\"end\":[16,0,{h}],\"x\":{xup},\"z\":{z},\"sha256\":\"0\",\"src\":\"test\",\"label\":\"V\"}}",
            if i > 0 { "," } else { "" }
        );
    }
    for ov in ["high", "mid", "low"] {
        s += &format!(",\"player_mantle_over_{ov}\":{{\"frames\":20,\"fps\":30,\"dur_ms\":667,\"end\":[31,0,-18],\"x\":{xov},\"z\":{zov},\"sha256\":\"0\",\"src\":\"test\",\"label\":\"V\"}}");
    }
    s += "},\"weapon_quick\":{\"ump45_mp\":{\"drop\":0.25,\"raise\":0.55,\"src\":\"test\",\"label\":\"V\",\"drop_anim\":\"viewmodel_ump45_putaway\",\"raise_anim\":\"viewmodel_ump45_pullout\",\"drop_is_putaway\":true,\"raise_is_pullout\":true},\"m4_mp\":{\"drop\":0.25,\"raise\":0.5,\"src\":\"test\",\"label\":\"V\",\"drop_anim\":\"viewmodel_M4_quick_putaway\",\"raise_anim\":\"viewmodel_M4_quick_pullout\",\"drop_is_putaway\":false,\"raise_is_pullout\":false}},\"info_mp\":{},\"curves_sha256\":\"0000\"}";
    s
}

/// [`test_json`] parsed (tests and the bench).
pub fn test_data() -> MoveData {
    MoveData::from_json(&test_json()).expect("test data is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_data_round_trip_and_known_values() {
        let d = test_data();
        assert_eq!(d.mw2.g_speed, 190.0);
        assert_eq!(d.mw2.gravity, 800.0);
        assert_eq!(d.weapon.ads_move_speed_scale, 2.0);
        assert_eq!(d.curves[CURVE_CROUCH_PRONE].len(), 10);
        assert_eq!(d.labels["walkable_normal_y"], "I");
        assert!(d.summary().contains("g_speed=190 sprint=1.5x4.0s"));
        let f: MovementFile = serde_json::from_str(&test_json()).unwrap();
        assert_eq!(MoveData::from_file(&f).unwrap(), d);
    }

    #[test]
    fn movement_data_missing_field_is_no_data() {
        let j = test_json().replace("\"gravity\":", "\"gravityX\":");
        let e = MoveData::from_json(&j).unwrap_err();
        assert!(e.contains("missing constant gravity"), "{e}");
        let j = test_json().replace("\"sprintOutTime\":", "\"sprintOutTimeX\":");
        assert!(MoveData::from_json(&j).unwrap_err().contains("sprintOutTime"));
        assert!(MoveData::from_json("{}").is_err());
        assert!(MoveData::from_json("").is_err());
    }

    #[test]
    fn movement_data_labels_only_v_d_or_i() {
        let j = test_json().replacen("\"label\":\"V\"", "\"label\":\"R\"", 1);
        assert!(MoveData::from_json(&j).unwrap_err().contains("must be V, D or I"));
        // D (decoded, not byte-checked) loads like V and I and keeps its label
        let d = MoveData::from_json(&test_json()).unwrap();
        assert_eq!(d.labels.get("time_land_base_ms").map(String::as_str), Some("D"));
        let j = test_json().replacen("\"value\":190", "\"value\":-190", 1);
        assert!(MoveData::from_json(&j).is_err());
        let j = test_json().replace("[100,11]", "[100,50]");
        assert!(MoveData::from_json(&j).is_ok(), "curve values only bounded 0..100");
        let j = test_json().replace("[90,17],[100,11]", "[90,17],[95,11]");
        assert!(MoveData::from_json(&j).unwrap_err().contains("0..100"));
    }
}
