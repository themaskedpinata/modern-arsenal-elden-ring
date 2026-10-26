//! Model of `mw2er.toml` (docs/M2_DESIGN.md section 19).
//!
//! Every field has a default (`#[serde(default)]`), so a missing key never breaks loading; unknown
//! keys are rejected (`deny_unknown_fields`) so a typo shows up as `CONFIG ERROR line:col` instead
//! of being silently ignored. The `[regulation]` section is read by `build_regulation.py`; it is
//! modelled here only so the file parses as a whole.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::ac130_tracer::{TracerParams, TracerStyle};

use crate::input_logic::dik_from_name;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema: u32,
    pub paths: Paths,
    pub log: LogCfg,
    pub keys: Keys,
    pub gameplay: Gameplay,
    pub aim: Aim,
    pub damage: Damage,
    pub regulation: Regulation,
    pub hit: Hit,
    pub menu: Menu,
    pub hud: Hud,
    pub audio: Audio,
    pub viewmodel: Viewmodel,
    pub first_person: FirstPerson,
    /// C6 part 18: the third-person over-the-shoulder camera (docs/C6_HOLD_DESIGN.md part 18).
    pub third_person: ThirdPerson,
    pub ac130: Ac130,
    /// M5: MW2 movement (docs/M5_DESIGN.md section 10).
    pub movement: Movement,
    /// M6: the MW2 arsenal (docs/M6_DESIGN.md).
    pub arsenal: Arsenal,
    /// M6: launchers (section 6.6).
    pub launchers: Launchers,
    /// M6: view kick / sway / gun kick (section 8).
    pub kick: Kick,
    /// M7 A1: the bullet-emitter watchdog (docs/M7_DESIGN.md 2.1.3).
    pub spawn: SpawnCfg,
    /// M7: the MW2 Tactical Nuke (docs/M7_DESIGN.md section 3).
    pub nuke: Nuke,
    /// C3: ER's messages above the gun (docs/C3_DESIGN.md 2.3).
    pub ui_yield: UiYieldCfg,
    /// C3: the Scaleform UI-pass probe (docs/C3_DESIGN.md 2.4; cold, off by default).
    pub ui: UiCfg,
    /// C3 PASS B: the XInput gamepad (docs/C3_DESIGN.md part 4).
    pub gamepad: GamepadCfg,
    /// C3 PASS C: seeing each other in co-op (docs/C3_DESIGN.md part 3).
    pub coop: CoopCfg,
}

/// `[coop]` (C3 PASS C, docs/C3_DESIGN.md 3.4-3.7). Hot. Everything here is cosmetic for the peers (no damage path);
/// `peer_drop_guard` only withholds kill credit / streak points / damage numbers from drops our shot cannot have dealt.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CoopCfg {
    /// Spawn a no-attack synced twin beside every damaging spawn while a peer is in the session (peers see tracers,
    /// hear the gun, see launcher / AC-130 explosions).
    pub visual_twins: bool,
    /// At most one twin per gun per this interval (an SMG at 40 ms: 25 per second).
    pub twin_min_interval_ms: u32,
    /// Peers' twins become tracers / sounds / flashes here.
    pub peer_effects: bool,
    pub peer_sounds: bool,
    pub peer_tracers: bool,
    /// Review C3: our HUD flash at a peer's explosion. Off by default: the explosion twin keeps its row's SFX, so ER
    /// draws the explosion on this PC already [I]; on only if the peer's explosions are invisible here.
    pub peer_flashes: bool,
    /// Review 2026-10-24 ("bullet trails can be seen through walls"): the peers' tracers, their HUD flashes and our
    /// launcher streaks (ImGui lines at Present, no depth test) are clipped at the shot's first map hit and hidden where
    /// map rays from the camera are blocked (`mw2er_core::tracer_occl`). Off = drawn as before.
    pub tracer_occlusion: bool,
    /// At most this many map rays per task tick for that (an uncast point draws, as before).
    pub tracer_occlusion_rays: u32,
    /// A peer's gun sound fades from `sound_min_m` to silence at `sound_max_m` (explosions keep `explosion_floor`).
    pub sound_min_m: f32,
    pub sound_max_m: f32,
    pub explosion_floor: f32,
    pub peer_gain: f32,
    /// Draw the MW2 world model of the gun every player holds (ours in third person with `world_gun_self_tp`).
    pub world_guns: bool,
    pub world_gun_self_tp: bool,
    /// Stage A anchor: forward, right, up (metres) from the character's feet along its facing.
    pub world_gun_offset_m: [f32; 3],
    /// lod1 beyond this distance (0 = always lod0).
    pub world_gun_lod1_m: f32,
    /// Hide a world gun when all three map rays from the camera to it are blocked.
    pub world_gun_occlusion: bool,
    /// Never draw a world gun farther than this.
    pub world_gun_max_m: f32,
    /// Review C3 stage B / C6: the right-hand FLVER bone index of the bone array (`COOP bone probe` / `HOLD bone` find
    /// it); -1 = the name lookup in hand mode (`hand_bone_auto`) / stage A in offset mode.
    pub hand_bone: i32,
    /// Review C3: log the bone palette (`COOP bone probe ...`) for 10 s after MW2 mode turns on (once per session).
    pub bone_probe: bool,
    /// Synced marker SpEffects (MW2 mode / gun in hand / AC-130 ride / nuke) on our character.
    pub markers: bool,
    pub marker_refresh_s: f32,
    /// The MW2-mode marker hides ER's own weapon on every PC (vanilla SpEffectVfx 8060, isInvisibleWeapon).
    pub hide_er_weapon: bool,
    /// A peer's gun is known for this long after its last twin when no gun marker is seen.
    pub twin_gun_fallback_s: f32,
    /// In a session an HP drop above `peer_drop_factor` x the row's attack x pellets is a peer's (`HIT peer_drop`).
    pub peer_drop_guard: bool,
    pub peer_drop_factor: f32,
    /// The `COOP twin sent / seen / peer fx` lines every this many seconds.
    pub log_every_s: f32,
    /// C6 (docs/C6_HOLD_DESIGN.md 4.4): "offset" = stage A (today: the fixed `world_gun_offset_m`); "hand" = the gun on
    /// the right-hand bone every frame (any failed read / check draws stage A for that player for that frame).
    pub hold_mode: String,
    /// C6: find the hand bone by name (`R_Weapon`, else `R_Hand`) in hand mode when `hand_bone = -1`.
    pub hand_bone_auto: bool,
    /// C6: MW2 model X, Y, Z -> hand-local axes as a signed permutation ("+x,+z,+y"); det must be -1 (like stage A).
    pub hand_axes: String,
    /// C6: the gun's grip in hand-local metres / degrees (X, then Y, then Z); per gun in `[coop.grip.<gun>]`.
    pub grip_offset_m: [f32; 3],
    pub grip_rot_deg: [f32; 3],
    /// C6: 1-frame lerp of the hand anchor toward the last good one (0 = none, at most 0.9).
    pub hand_smooth: f32,
    /// C6: per-gun grip overrides (`[coop.grip.m4]` or `[coop.grip.m4_mp]`); missing keys = the global values.
    pub grip: BTreeMap<String, GripCfg>,
    /// C6: on Torrent in hand mode: "hand" (the bone as usual) | "hide" (no world gun while mounted).
    pub hold_mounted: String,
    /// C6 (docs 6): MW2 ADS in third person: "hand" (the gun stays on the hand) | "aim" (blends to the camera-aimed
    /// stage-A pose in 0.12 s and back). C6 round 7: "aim" also acts inside a `tp_ads_pose = "er_aim"` episode when ER's
    /// aim pose did not turn the weapon (review: on ER's confirmed aim, not the align clamp; `hold::aim_nopose_step`).
    pub tp_ads_hold: String,
    /// C6 probe (superseded by `tp_ads_pose`; kept as its alias): `true` = `tp_ads_pose = "er_aim"`.
    pub tp_ads_er_aim: bool,
    /// C6 aim 4 (work/c6/aim4.md 3, docs part 14): MW2 ADS in third person with the stance posed (non-scoped guns):
    /// "off" | "er_aim" = ER's own shouldered crossbow aim: the stanced row gets weaponCategory 11 / wepType 55 for the
    /// ADS episode and ER's aim input (pad LB / keyboard RMB) is held; confirmed by `precision_shooting`, restored at
    /// the release. ER's aim camera runs; MW2 aiming / firing stay ours. C6 part 17 (docs 17): "guard" = ER's two-handed
    /// GUARD with the polearm stance (`er_stance = "polearm"` only): only ER's block input is held (pad LB / keyboard
    /// RMB), nothing is written on the row, no ER aim camera / reticle / fade mask; the gun aligns to the aim line on ER's
    /// weapon frame, the round-7 gun raise (`tp_ads_hold = "aim"`) when the guard pose is not detected.
    pub tp_ads_pose: String,
    /// C6 part 17: while a third-person guard ADS is held standing still, our body faces the camera yaw (the M3B first-
    /// person yaw write on CSChrPhysicsModule's two orientations, ER's own facing: no restore). Hot; only with the guard.
    pub tp_guard_yaw: bool,
    /// C6 part 17: the largest barrel-to-aim-line angle the guard's `aim_align` turns the gun by (about its grip, on ER's
    /// weapon frame); above it the guard episode raises the gun along the camera (`tp_ads_hold = "aim"`). Hot.
    pub tp_guard_align_max_deg: f32,
    /// C6 aim 4 fallback: when the aim fields are written: "ads" (per ADS episode) | "stance" (from the stance enter,
    /// before its chord, to its exit: for a game that caches the type at the grip change; aim4.md 6).
    pub tp_ads_patch: String,
    /// C6 aim 4 probe (read-only, hot): `HOLD aim probe anim=.. prec=.. arm=.. type=.. camtype=..` on every change while
    /// the stance tick runs (also with MW2 mode off and a real crossbow: aim4.md 4 P-A).
    pub probe_er_aim: bool,
    /// C6 round 6 (work/c6/aim6.md, docs 14.8): the camera / reticle during a `tp_ads_pose = "er_aim"` episode: "keep"
    /// (the user's wish: only ER's aim BODY animation; our normal third-person camera, no ER crossbow reticle: a
    /// read-only filter on ER's precision-shooting getter for exactly ER's camera update and its reticle update) | "er"
    /// (the escape hatch: ER's over-the-shoulder aim camera and crossbow reticle, as round 5). Hot. Only with the aim
    /// pose (code default off), so nothing is hooked by default. C6 round 7: "keep" also masks ER's aim-time body fade
    /// (the snipe fade's getter call) and, refused / unavailable, keeps the aim pose off (no invisible body).
    pub tp_aim_cam: String,
    /// C6 (docs 3): "off" | "crossbow": the hidden ER weapon's row gets the crossbow motion set in third person (a
    /// runtime patch of five motion fields, restored exactly; no regulation change). R3 melee is off while it is active.
    /// C6 part 17: "polearm" = the same patch with the vanilla Spear's values (motion 36, two-handed base set 13: the
    /// hands spread along a shaft), carry and guard; the crossbow aim / carrier need "crossbow", the guard "polearm".
    pub er_stance: String,
    /// C6: how the patched bank is picked up: "chord" (ER's two-hand chord, Y held + RB / E held + LMB) | "arm_style"
    /// (probe: write arm_style = 3) | "none".
    pub stance_refresh: String,
    /// C6: at most one stance enter / exit per this many seconds.
    pub stance_min_switch_s: f32,
    /// C6: also patch a peer's active right row while it carries the MW2-mode marker (needs `er_stance`).
    pub stance_peers: bool,
    /// C6: with peers present, our enter chord waits this long after mode on (the peer patches our row first).
    pub stance_peer_delay_s: f32,
    /// C6 attach 2 (work/c6/attach2.md): what hand mode puts the gun on: "weapon" = ER's own right-hand weapon model
    /// (CSChrAsmModelIns slot 11: exactly where ER draws the sword in every pose), falling back to the hand bone with the
    /// learned hand -> weapon grip, then the fixed offset | "bone" = the hand bone only (the first C6 build).
    pub hold_source: String,
    /// C6 attach 2 / grip 3: MW2 model X (forward), Y (left), Z (up) -> the ER weapon model's axes (det -1). Default
    /// `hold::WEAPON_AXES_RIG` "-y,+z,+x" (work/c6/grip3.md 1: ER's weapon frame is a rig constant). "auto" = a
    /// diagnostic: the in-game vote still runs while the stance holds the weapon, but only the rig map is ever accepted
    /// (`HOLD weapon axes auto ... accepted=0|1`); the rig map is drawn meanwhile.
    pub weapon_axes: String,
    /// C6 grip 3 (grip3.md 4): the grip rake about the weapon's Z in weapon mode (cancels ER's +18.8 deg muzzle-up hold).
    /// The built-in per-gun grips (`hold::GRIP_IN` -> `hold::weapon_grip`) use it; `[coop.grip.<gun>]` keys override.
    pub weapon_rake_deg: f32,
    /// C6 pose 3 (work/c6/pose3.md 4.2): a stance exit into first person restores the row at once but sends its grip
    /// chords only after this long; a re-enter before then on the same row sends none (ER still holds the crossbow bank:
    /// `HOLD stance resume`). 0 = the chords at once (the first build; review C6 pose: the shipped default, first person
    /// stays identical; the in-game test toml opts in). F5 mode off is never deferred.
    pub stance_exit_defer_s: f32,
    /// C6 depth (work/c6/depth3.md 3, fallback): depth-only capsules on every drawn player's body bones (ours in third
    /// person, the peers') are drawn into OUR world-gun depth first, so a gun behind a body (and the back-carried shield)
    /// is hidden there. Never reads or binds ER's GPU resources; any miss draws the guns as before. Third person only.
    pub world_gun_body_occlusion: bool,
    /// C6 depth probe P-D (depth3.md 5, lines 1-2 + the wrapper dump): read-only pointer chain to ER's scene depth
    /// buffer, its desc and the GX wrapper words (`DEPTH probe ...`), task thread, every 5 s. Cold. Nothing is bound,
    /// barriered or sampled.
    pub probe_er_depth: bool,
    /// C6 depth 5 probe P-S (docs/C6_HOLD_DESIGN.md 15.1): a read-only MinHook on d3d12's `ResourceBarrier` (original
    /// first; transitions on ER's scene depth recorded into a lock-free table) and the queue-order fold in hudhook's ECL
    /// callback (`DEPTH state ...` lines and the verdict). Cold; needs `probe_er_depth` and `[ui] inject_viewmodel`.
    pub probe_er_depth_state: bool,
    /// C6 depth 5 probe P-S2: every 5 s, 16 texels + the centre + our chest pixel of ER's depth through our SRV (our
    /// list spliced ahead of ER's UI only on a frame the fold proved readable; `DEPTH probe sample / self`). Cold; needs
    /// `probe_er_depth_state`.
    pub probe_er_depth_sample: bool,
    /// C6 depth 5 (15.2 / 15.3): ER's scene depth hides world guns in third person (a copy of its depth at UiBegin,
    /// primed into our world-gun depth before the capsules and the guns). Cold on, hot off; goes live only on this
    /// session's `DEPTH state verdict ready=1` + a passing sample / self check; any miss or trip = capsules only.
    pub world_gun_er_depth: bool,
    /// C6 depth 5: the occluder pushed back by `er_depth_bias_m + er_depth_bias_rel * z` (hot).
    pub er_depth_bias_m: f32,
    pub er_depth_bias_rel: f32,
    /// C6 depth 5: frames turning faster than this (degrees per frame) skip the prime (capsules only), hot.
    pub er_depth_max_turn_deg: f32,
    /// C6 hide (work/c6/hide.md): with hand mode or the stance and `hide_er_weapon`, the MW2-mode marker row 79001090 is
    /// patched in memory into an Unseen Blade-style effect (stateInfo 184 "hide weapon", vow bits, neutral rates) so it
    /// really hides ER's weapon model; restored when off. false = the marker as packaged (vfx only: hides nothing).
    pub hide_state_patch: bool,
    /// C6 part 16 (docs/C6_HOLD_DESIGN.md 16.2-16.7): "off" | "dry" | "crossbow". "crossbow" = in third person with MW2
    /// mode on, the active right ER weapon is swapped for an owned vanilla crossbow (ER's own menu equip, on the game
    /// thread, journal first; restored at mode off / first person / AC-130 / nuke / world leave / flag off / the next world
    /// enter after a crash), so ER plays its genuine crossbow carry and aim. "dry" = probe P-X1: everything except the
    /// call (`HOLD xbow dry ...`), no game write. Hot; 2.7.2.0 only. Needs the stance (`er_stance = "crossbow"`).
    pub er_carrier: String,
    /// C6 part 16 (X4): with no crossbow owned, give a vanilla Light Crossbow into the MOD save (ER's own give recipe;
    /// the item stays in the inventory). false = `HOLD xbow skip reason=none_owned` (the stance as the fallback). Hot.
    pub carrier_give: bool,
    /// C6 part 16.16 (the 2026-10-22 aim A/B: ER's aim anim never played with the carrier unloaded): once per swap, after
    /// the grip is two-handed and settled with no chord, the carrier crossbow is put in ER's loaded state
    /// (`bolt_loaded_states[slot] = 1` in EquipGameData's and PlayerIns's ChrAsm: the end state of ER's own reload TAE
    /// event 35; no bolt used); undone first at every exit / restore / grip change / chord. Hot; needs the carrier.
    pub carrier_load: bool,
    /// C6 part 16.7: the carrier crossbow's weapon-frame map (as weapon_axes, for a carried crossbow row); code default
    /// the sword's rig map until probe P-X5 (`HOLD xbow frame`) derives it. `[coop.grip.<gun>.xbow]` keys override the
    /// grip on the crossbow (the sword table stays).
    pub weapon_axes_xbow: String,
    /// C6 part 17 addendum (docs 17.12): with `er_stance = "polearm"` the gun sits on ER's polearm carry with its own
    /// grips (derived from the 2026-10-22 carry frames): a long gun along the shaft (the pistol grip in the right fist,
    /// the barrel toward the left hand), a short one level ahead from the right fist (`hold::polearm_kind`).
    /// `[coop.grip.<gun>.polearm]` keys override. false = the sword / crossbow grip table as before. Hot.
    pub polearm_grip: bool,
    /// C6 part 17 addendum: a gun whose muzzle reaches this far ahead of its pistol grip is "long" (0..2 m).
    pub polearm_long_min_m: f32,
    /// C6 part 17 addendum: the gun's frame in a DETECTED guard (`tp_ads_pose = "guard"`): "off" = ER's weapon frame
    /// with the carry grip (part 17) | "auto" = the built-in guard frame (the barrel along the body's forward from the
    /// right fist, from the 2026-10-22 guards) until this session learned the row's guard (`HOLD guard frame auto`) |
    /// a det -1 signed permutation (`HOLD guard frame auto axes=`) turned by `guard_rot_deg`. Blended in over the
    /// detection; `aim_align` keeps only the residual (over `tp_guard_align_max_deg`: the raise, as before). Hot.
    pub guard_axes: String,
    /// C6 part 17 addendum: the turn (deg, X then Y then Z) applied after an explicit `guard_axes` map.
    pub guard_rot_deg: [f32; 3],
}

/// C6: one gun's grip override (`[coop.grip.<gun>]`); a missing key takes the global `grip_offset_m` / `grip_rot_deg`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GripCfg {
    pub offset_m: Option<[f32; 3]>,
    pub rot_deg: Option<[f32; 3]>,
    /// C6 part 16.7: the grip while ER carries the real crossbow (`[coop.grip.<gun>.xbow]`).
    pub xbow: Option<GripXbowCfg>,
    /// C6 part 17 addendum: the grip on ER's polearm carry (`[coop.grip.<gun>.polearm]`, on the `weapon_axes` map).
    pub polearm: Option<GripXbowCfg>,
}

/// C6 part 16.7: one gun's grip on the carrier crossbow's weapon frame; a missing key = the built-in grip on the
/// `weapon_axes_xbow` map (never the sword's keys).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GripXbowCfg {
    pub offset_m: Option<[f32; 3]>,
    pub rot_deg: Option<[f32; 3]>,
}

impl CoopCfg {
    /// C6: hand mode is on.
    pub fn hold_hand(&self) -> bool {
        self.hold_mode == "hand"
    }

    /// C6: the stance is configured on (either motion set).
    pub fn stance_on(&self) -> bool {
        self.er_stance == "crossbow" || self.er_stance == "polearm"
    }

    /// C6 part 17: the crossbow motion set (the er_aim pose and the carrier need it).
    pub fn stance_crossbow(&self) -> bool {
        self.er_stance == "crossbow"
    }

    /// C6 part 17: the motion set the stance writes (meaningful only with `stance_on`).
    pub fn stance_set(&self) -> crate::er_stance::StanceSet {
        if self.er_stance == "polearm" { crate::er_stance::StanceSet::Polearm } else { crate::er_stance::StanceSet::Crossbow }
    }

    /// C6 aim 4: third-person ADS raises ER's crossbow aim (`tp_ads_pose = "er_aim"`, or the old probe key); acts only
    /// with the stance on. C6 part 17: only with the crossbow set (a polearm bank has no crossbow aim).
    pub fn ads_pose_on(&self) -> bool {
        self.stance_crossbow() && (self.tp_ads_pose == "er_aim" || self.tp_ads_er_aim)
    }

    /// C6 part 17: third-person ADS holds ER's two-handed guard (`tp_ads_pose = "guard"`); only with the polearm stance.
    /// Never the aim fields, the aim camera filter or the carrier.
    pub fn guard_pose_on(&self) -> bool {
        self.er_stance == "polearm" && self.tp_ads_pose == "guard"
    }

    /// C6 part 17: the body faces the camera while the guard ADS is held standing (`tp_guard_yaw`, only with the guard).
    pub fn guard_yaw_on(&self) -> bool {
        self.guard_pose_on() && self.tp_guard_yaw
    }

    /// C6 round 6 (docs 14.8): an aim episode keeps our normal third-person camera and no ER crossbow reticle (the aim
    /// pose on and `tp_aim_cam = "keep"`, the default of that option; "er" is the escape hatch).
    pub fn aim_cam_keep(&self) -> bool {
        self.ads_pose_on() && self.tp_aim_cam == "keep"
    }

    /// C6 aim 4: the aim fields stay written for the whole stance (the fallback), not per ADS episode.
    pub fn aim_patch_stance(&self) -> bool {
        self.ads_pose_on() && self.tp_ads_patch == "stance"
    }

    /// C6 part 16: the real crossbow carrier's mode (acts only with the stance on: the stance holds its hide, chords and
    /// aim pose).
    pub fn carrier_mode(&self) -> crate::carrier::Mode {
        if self.stance_crossbow() { crate::carrier::Mode::of(&self.er_carrier) } else { crate::carrier::Mode::Off }
    }

    /// C6 part 16.7: the grip of `gun` on the CARRIER crossbow's weapon frame: its `[coop.grip.<gun>.xbow]` keys over the
    /// built-in grip on the `weapon_axes_xbow` map. The third value: "table" | "config" | "mixed".
    pub fn weapon_grip_xbow(&self, gun: &str) -> ([f32; 3], [f32; 3], &'static str) {
        let axes = crate::hold::parse_axes(&self.weapon_axes_xbow)
            .or_else(|_| crate::hold::parse_axes(crate::hold::WEAPON_AXES_RIG))
            .unwrap_or(crate::xform::MAT34_IDENTITY);
        let (to, tr) = crate::hold::weapon_grip(crate::hold::grip_point_in(gun), &axes, self.weapon_rake_deg);
        let x = self.grip_entry(gun).and_then(|g| g.xbow.as_ref());
        let (o, r) = (x.and_then(|g| g.offset_m), x.and_then(|g| g.rot_deg));
        let src = match (o.is_some(), r.is_some()) {
            (true, true) => "config",
            (false, false) => "table",
            _ => "mixed",
        };
        (o.unwrap_or(to), r.unwrap_or(tr), src)
    }

    /// C6 part 17 addendum: the polearm carry grips are drawn (the polearm stance and `polearm_grip`).
    pub fn polearm_grip_on(&self) -> bool {
        self.er_stance == "polearm" && self.polearm_grip
    }

    /// C6 part 17 addendum: the grip of `gun` on ER's POLEARM carry: its `[coop.grip.<gun>.polearm]` keys over the
    /// built-in hold (`hold::polearm_frame` of its long / short kind, on the `weapon_axes` map when explicit, else the rig
    /// map). The third value: "polearm_long" | "polearm_short" | "config" | "mixed".
    pub fn weapon_grip_polearm(&self, gun: &str) -> ([f32; 3], [f32; 3], &'static str) {
        let axes = crate::hold::parse_axes(&self.weapon_axes)
            .or_else(|_| crate::hold::parse_axes(crate::hold::WEAPON_AXES_RIG))
            .unwrap_or(crate::xform::MAT34_IDENTITY);
        let kind = crate::hold::polearm_kind(gun, self.polearm_long_min_m);
        let (to, tr) = crate::hold::frame_grip(crate::hold::grip_point_in(gun), &crate::hold::polearm_frame(kind), &axes);
        let x = self.grip_entry(gun).and_then(|g| g.polearm.as_ref());
        let (o, r) = (x.and_then(|g| g.offset_m), x.and_then(|g| g.rot_deg));
        let src = match (o.is_some(), r.is_some()) {
            (true, true) => "config",
            (false, false) => kind.label(),
            _ => "mixed",
        };
        // a rot key alone keeps the grip point in the fist (the offset follows the new rotation)
        let o = o.unwrap_or_else(|| r.map_or(to, |r| crate::hold::weapon_grip_offset(crate::hold::grip_point_in(gun), &axes, r)));
        (o, r.unwrap_or(tr), src)
    }

    /// C6 part 17 addendum: the guard frame correction is on (the guard pose and `guard_axes` not "off").
    pub fn guard_frame_on(&self) -> bool {
        self.guard_pose_on() && self.guard_axes != "off"
    }

    /// C6 part 17 addendum: an explicit `guard_axes` map turned by `guard_rot_deg` (weapon coordinates); None for
    /// "off" / "auto".
    pub fn guard_frame_fixed(&self) -> Option<crate::xform::Mat34> {
        if matches!(self.guard_axes.as_str(), "off" | "auto") {
            return None;
        }
        let a = crate::hold::parse_axes(&self.guard_axes).ok()?;
        Some(crate::xform::m_mul(&crate::hold::rot_xyz_deg(self.guard_rot_deg), &a))
    }

    /// C6 attach 2: hand mode puts the gun on ER's own weapon model first.
    pub fn weapon_source(&self) -> bool {
        self.hold_hand() && self.hold_source == "weapon"
    }

    /// C6 hide: the MW2-mode marker row is patched into a real weapon hide (only where C6 shows the gun in the hand:
    /// hand mode or the stance; the defaults never touch the row).
    pub fn hide_patch_wanted(&self) -> bool {
        self.hide_er_weapon && self.hide_state_patch && (self.hold_hand() || self.stance_on())
    }

    /// C6: the grip of `gun` (an internal MP name such as "m4_mp") on the HAND BONE (`hold_source = "bone"` / the
    /// `hand_axes` path): its `[coop.grip]` entry (by full or short name), key by key over the globals.
    pub fn grip_for(&self, gun: &str) -> ([f32; 3], [f32; 3]) {
        let g = self.grip_entry(gun);
        (g.and_then(|g| g.offset_m).unwrap_or(self.grip_offset_m), g.and_then(|g| g.rot_deg).unwrap_or(self.grip_rot_deg))
    }

    fn grip_entry(&self, gun: &str) -> Option<&GripCfg> {
        let short = gun.strip_suffix("_mp").unwrap_or(gun);
        self.grip.get(gun).or_else(|| self.grip.get(short)).or_else(|| self.grip.get(&format!("{short}_mp")))
    }

    /// C6 grip 3: the grip of `gun` on ER's WEAPON frame (the weapon model and the learned bone grip): its `[coop.grip]`
    /// keys over the built-in grip (`hold::GRIP_IN` point, the `weapon_axes` map when explicit (else the rig map),
    /// `weapon_rake_deg`). The third value says where it came from: "table" | "config" (both keys) | "mixed".
    pub fn weapon_grip_for(&self, gun: &str) -> ([f32; 3], [f32; 3], &'static str) {
        let axes = crate::hold::parse_axes(&self.weapon_axes)
            .or_else(|_| crate::hold::parse_axes(crate::hold::WEAPON_AXES_RIG))
            .unwrap_or(crate::xform::MAT34_IDENTITY);
        let (to, tr) = crate::hold::weapon_grip(crate::hold::grip_point_in(gun), &axes, self.weapon_rake_deg);
        let g = self.grip_entry(gun);
        let (o, r) = (g.and_then(|g| g.offset_m), g.and_then(|g| g.rot_deg));
        let src = match (o.is_some(), r.is_some()) {
            (true, true) => "config",
            (false, false) => "table",
            _ => "mixed",
        };
        (o.unwrap_or(to), r.unwrap_or(tr), src)
    }
}

impl Default for CoopCfg {
    fn default() -> Self {
        CoopCfg {
            visual_twins: true,
            twin_min_interval_ms: 40,
            peer_effects: true,
            peer_sounds: true,
            peer_tracers: true,
            peer_flashes: false,
            tracer_occlusion: true,
            tracer_occlusion_rays: 32,
            sound_min_m: 10.0,
            sound_max_m: 150.0,
            explosion_floor: 0.15,
            peer_gain: 0.8,
            world_guns: true,
            world_gun_self_tp: true,
            world_gun_offset_m: [0.45, 0.20, 1.15],
            world_gun_lod1_m: 10.0,
            world_gun_occlusion: true,
            world_gun_max_m: 80.0,
            hand_bone: -1,
            bone_probe: true,
            markers: true,
            marker_refresh_s: 5.0,
            hide_er_weapon: true,
            twin_gun_fallback_s: 3.0,
            peer_drop_guard: true,
            peer_drop_factor: 1.5,
            log_every_s: 5.0,
            hold_mode: "offset".into(),
            hand_bone_auto: true,
            hand_axes: "+x,+z,+y".into(),
            grip_offset_m: [0.0; 3],
            grip_rot_deg: [0.0; 3],
            hand_smooth: 0.0,
            grip: BTreeMap::new(),
            hold_mounted: "hand".into(),
            tp_ads_hold: "hand".into(),
            tp_ads_er_aim: false,
            tp_ads_pose: "off".into(),
            tp_guard_yaw: false,
            tp_guard_align_max_deg: crate::hold::AIM_ALIGN_MAX_DEG,
            tp_ads_patch: "ads".into(),
            probe_er_aim: false,
            tp_aim_cam: "keep".into(),
            er_stance: "off".into(),
            stance_refresh: "chord".into(),
            stance_min_switch_s: 1.5,
            stance_peers: true,
            stance_peer_delay_s: 1.5,
            hold_source: "weapon".into(),
            weapon_axes: crate::hold::WEAPON_AXES_RIG.into(),
            hide_state_patch: true,
            weapon_rake_deg: crate::hold::WEAPON_RAKE_DEG,
            stance_exit_defer_s: 0.0,
            world_gun_body_occlusion: false,
            probe_er_depth: false,
            probe_er_depth_state: false,
            probe_er_depth_sample: false,
            world_gun_er_depth: false,
            er_depth_bias_m: 0.03,
            er_depth_bias_rel: 0.004,
            er_depth_max_turn_deg: 3.0,
            er_carrier: "off".into(),
            carrier_give: false,
            carrier_load: false,
            weapon_axes_xbow: crate::hold::WEAPON_AXES_RIG.into(),
            polearm_grip: true,
            polearm_long_min_m: crate::hold::POLEARM_LONG_MIN_M,
            guard_axes: "auto".into(),
            guard_rot_deg: [0.0; 3],
        }
    }
}

impl Config {
    fn validate_coop(&self) -> Result<(), String> {
        let c = &self.coop;
        if !(0..=1000).contains(&c.twin_min_interval_ms) {
            return Err("coop.twin_min_interval_ms must be 0..1000".into());
        }
        if c.tracer_occlusion_rays > 512 {
            return Err("coop.tracer_occlusion_rays must be 0..512".into());
        }
        in_range_f32("coop.sound_min_m", c.sound_min_m, 0.0, 1000.0)?;
        in_range_f32("coop.sound_max_m", c.sound_max_m, 1.0, 5000.0)?;
        if c.sound_max_m <= c.sound_min_m {
            return Err("coop.sound_max_m must be above coop.sound_min_m".into());
        }
        in_range_f32("coop.explosion_floor", c.explosion_floor, 0.0, 1.0)?;
        in_range_f32("coop.peer_gain", c.peer_gain, 0.0, 4.0)?;
        for (i, &v) in c.world_gun_offset_m.iter().enumerate() {
            in_range_f32(&format!("coop.world_gun_offset_m[{i}]"), v, -3.0, 3.0)?;
        }
        in_range_f32("coop.world_gun_lod1_m", c.world_gun_lod1_m, 0.0, 1000.0)?;
        in_range_f32("coop.world_gun_max_m", c.world_gun_max_m, 1.0, 1000.0)?;
        if !(-1..=1023).contains(&c.hand_bone) {
            return Err("coop.hand_bone must be -1 (name lookup / stage A) or a FLVER bone index 0..1023 (COOP bone probe / HOLD bone)".into());
        }
        // C6 (docs/C6_HOLD_DESIGN.md 4.4)
        if !matches!(c.hold_mode.as_str(), "offset" | "hand") {
            return Err("coop.hold_mode must be offset | hand".into());
        }
        crate::hold::parse_axes(&c.hand_axes).map_err(|e| format!("coop.{e}"))?;
        for (i, &v) in c.grip_offset_m.iter().enumerate() {
            in_range_f32(&format!("coop.grip_offset_m[{i}]"), v, -0.5, 0.5)?;
        }
        for (i, &v) in c.grip_rot_deg.iter().enumerate() {
            in_range_f32(&format!("coop.grip_rot_deg[{i}]"), v, -180.0, 180.0)?;
        }
        in_range_f32("coop.hand_smooth", c.hand_smooth, 0.0, 0.9)?;
        let mut grip_guns = std::collections::BTreeSet::new();
        for (name, g) in &c.grip {
            let full = if name.ends_with("_mp") { name.clone() } else { format!("{name}_mp") };
            let known = if self.arsenal.guns.is_empty() {
                crate::hold::MW2_GUNS.contains(&full.as_str())
            } else {
                self.arsenal.guns.iter().any(|x| *x == full)
            };
            if !known {
                return Err(format!("coop.grip.{name}: not a gun of arsenal.guns (use the internal name, e.g. [coop.grip.m4] or [coop.grip.m4_mp])"));
            }
            if !grip_guns.insert(full.clone()) {
                return Err(format!("coop.grip lists {full} twice (short and full name)"));
            }
            // grip 3: +-1 m (a launcher's grip sits up to 0.7 m from its model origin on ER's weapon frame)
            for (i, &v) in g.offset_m.iter().flatten().enumerate() {
                in_range_f32(&format!("coop.grip.{name}.offset_m[{i}]"), v, -1.0, 1.0)?;
            }
            for (i, &v) in g.rot_deg.iter().flatten().enumerate() {
                in_range_f32(&format!("coop.grip.{name}.rot_deg[{i}]"), v, -180.0, 180.0)?;
            }
            // C6 part 16.7: the carrier crossbow's grip
            for (i, &v) in g.xbow.iter().flat_map(|x| x.offset_m.iter().flatten()).enumerate() {
                in_range_f32(&format!("coop.grip.{name}.xbow.offset_m[{i}]"), v, -1.0, 1.0)?;
            }
            for (i, &v) in g.xbow.iter().flat_map(|x| x.rot_deg.iter().flatten()).enumerate() {
                in_range_f32(&format!("coop.grip.{name}.xbow.rot_deg[{i}]"), v, -180.0, 180.0)?;
            }
            // C6 part 17 addendum: the polearm carry's grip
            for (i, &v) in g.polearm.iter().flat_map(|x| x.offset_m.iter().flatten()).enumerate() {
                in_range_f32(&format!("coop.grip.{name}.polearm.offset_m[{i}]"), v, -1.0, 1.0)?;
            }
            for (i, &v) in g.polearm.iter().flat_map(|x| x.rot_deg.iter().flatten()).enumerate() {
                in_range_f32(&format!("coop.grip.{name}.polearm.rot_deg[{i}]"), v, -180.0, 180.0)?;
            }
        }
        if !matches!(c.hold_mounted.as_str(), "hand" | "hide") {
            return Err("coop.hold_mounted must be hand | hide".into());
        }
        if !matches!(c.tp_ads_hold.as_str(), "hand" | "aim") {
            return Err("coop.tp_ads_hold must be hand | aim".into());
        }
        if !matches!(c.tp_ads_pose.as_str(), "off" | "er_aim" | "guard") {
            return Err("coop.tp_ads_pose must be off | er_aim | guard".into());
        }
        // C6 part 17
        in_range_f32("coop.tp_guard_align_max_deg", c.tp_guard_align_max_deg, 0.0, 90.0)?;
        // C6 part 17 addendum
        in_range_f32("coop.polearm_long_min_m", c.polearm_long_min_m, 0.0, 2.0)?;
        if !matches!(c.guard_axes.as_str(), "off" | "auto") {
            crate::hold::parse_axes(&c.guard_axes).map_err(|e| format!("coop.guard_axes: {e} (or \"off\" | \"auto\")"))?;
        }
        for (i, &v) in c.guard_rot_deg.iter().enumerate() {
            in_range_f32(&format!("coop.guard_rot_deg[{i}]"), v, -180.0, 180.0)?;
        }
        if !matches!(c.tp_ads_patch.as_str(), "ads" | "stance") {
            return Err("coop.tp_ads_patch must be ads | stance".into());
        }
        if !matches!(c.tp_aim_cam.as_str(), "keep" | "er") {
            return Err("coop.tp_aim_cam must be keep | er".into());
        }
        if !matches!(c.er_stance.as_str(), "off" | "crossbow" | "polearm") {
            return Err("coop.er_stance must be off | crossbow | polearm".into());
        }
        if !matches!(c.stance_refresh.as_str(), "chord" | "arm_style" | "none") {
            return Err("coop.stance_refresh must be chord | arm_style | none".into());
        }
        in_range_f32("coop.stance_min_switch_s", c.stance_min_switch_s, 0.5, 30.0)?;
        in_range_f32("coop.stance_peer_delay_s", c.stance_peer_delay_s, 0.0, 10.0)?;
        if !matches!(c.hold_source.as_str(), "weapon" | "bone") {
            return Err("coop.hold_source must be weapon | bone".into());
        }
        if c.weapon_axes != "auto" {
            crate::hold::parse_axes(&c.weapon_axes).map_err(|e| format!("coop.weapon_axes: {e} (or \"auto\")"))?;
        }
        // C6 part 16
        if !matches!(c.er_carrier.as_str(), "off" | "dry" | "crossbow") {
            return Err("coop.er_carrier must be off | dry | crossbow".into());
        }
        crate::hold::parse_axes(&c.weapon_axes_xbow).map_err(|e| format!("coop.weapon_axes_xbow: {e}"))?;
        in_range_f32("coop.weapon_rake_deg", c.weapon_rake_deg, -45.0, 45.0)?;
        in_range_f32("coop.stance_exit_defer_s", c.stance_exit_defer_s, 0.0, 10.0)?;
        in_range_f32("coop.er_depth_bias_m", c.er_depth_bias_m, 0.0, 0.5)?;
        in_range_f32("coop.er_depth_bias_rel", c.er_depth_bias_rel, 0.0, 0.05)?;
        in_range_f32("coop.er_depth_max_turn_deg", c.er_depth_max_turn_deg, 0.5, 30.0)?;
        in_range_f32("coop.marker_refresh_s", c.marker_refresh_s, 1.0, 60.0)?;
        in_range_f32("coop.twin_gun_fallback_s", c.twin_gun_fallback_s, 0.0, 60.0)?;
        in_range_f32("coop.peer_drop_factor", c.peer_drop_factor, 1.0, 100.0)?;
        in_range_f32("coop.log_every_s", c.log_every_s, 1.0, 600.0)?;
        let n = &self.nuke;
        if !matches!(n.coop_mode.as_str(), "owned" | "local") {
            return Err("nuke.coop_mode must be owned | local (allow_in_coop = false refuses the nuke in a session)".into());
        }
        in_range("nuke.coop_window_s", n.coop_window_s, 0.5, 60.0)?;
        in_range("nuke.coop_client_fallback_s", n.coop_client_fallback_s, 0.0, 60.0)?;
        if n.probe_forward {
            return Err("nuke.probe_forward is not built in C3 (must stay false)".into());
        }
        if self.movement.fp_anim_speed_scale {
            return Err("movement.fp_anim_speed_scale is not built in C3 (must stay false: no animation-speed write)".into());
        }
        in_range_f32("movement.fp_anim_rm_mps", self.movement.fp_anim_rm_mps, 0.0, 20.0)?;
        Ok(())
    }
}

/// `[gamepad]` (C3 PASS B, docs/C3_DESIGN.md 4.6). Hot, except that the XInput hook is installed at boot only when
/// `enabled` (a hot `enabled = false` makes the detour pass everything through).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GamepadCfg {
    pub enabled: bool,
    /// XInput slot 0..3; -1 = the first connected slot that shows input.
    pub pad_index: i32,
    /// "mouse": the right stick turns ER's camera through the DirectInput mouse path; "er": ER keeps the stick
    /// (reshaped by the MW2 graph).
    pub look_route: String,
    pub look_sensitivity: f32,
    pub invert_y: bool,
    /// MW2 `input_graph` (view_input_N.graph, 0..3).
    pub input_graph: u8,
    pub turnrate_yaw: f32,
    pub turnrate_pitch: f32,
    pub turnrate_yaw_ads: f32,
    pub turnrate_pitch_ads: f32,
    pub accel: bool,
    pub accel_lerp: f32,
    /// Our choice, not MW2's.
    pub accel_max_yaw: f32,
    pub rs_deadzone: f32,
    pub rs_outer: f32,
    pub ls_deadzone: f32,
    pub ls_outer: f32,
    /// Trigger level (0..255) above which RT / LT count as pressed.
    pub trigger_threshold: u8,
    /// X tap (reload) vs hold (ER interact), and B tap (crouch) vs hold (prone) through MW2's cl_stanceHoldTime.
    pub hold_ms: u32,
    /// ER's camera degrees per mouse count; 0 = auto-calibrated in game (`PAD calib`).
    pub mouse_deg_per_count: f32,
    /// AC-130 aim counts per degree of stick turn; 0 = from `[ac130] mouse_deg_per_count` x `mouse_sens`.
    pub gunship_counts_per_deg: f32,
    /// Not implemented (no XInputSetState hook); must stay false.
    pub rumble: bool,
    pub log_every_s: u32,
    /// What ER does not see while MW2 mode drives: buttons, "rs" (right stick, route mouse) and "ls_fp" (left stick,
    /// same rule as W/A/S/D).
    pub blank_er: Vec<String>,
    /// The ER pad button pressed for ER while the interact hold fires (ER's event action) [I: Y on ER's default layout].
    pub er_interact: String,
    /// Review C3: the ER pad button pulsed once when `bind.melee` (R3) is pressed in MW2 mode (ER's right-hand light
    /// attack, RB on ER's default layout [I]); "" = melee does nothing.
    pub er_melee: String,
    /// 2026-10-21: the ER pad button pulsed once when `bind.heal` (D-pad right) is pressed in MW2 mode (ER's use-item =
    /// the selected quick item, X on ER's default layout [I]); "" = heal does nothing.
    pub er_use_item: String,
    /// 2026-10-21: `ui_states` indices of ER's action prompt ("Rest at site of grace", "Open", "Pick up") [I: 19 from the
    /// MENU logs]: while one is visible an X press is ER's interact at once (no reload); [] = X tap always reloads.
    pub prompt_indices: Vec<u32>,
    pub bind: PadBindCfg,
    pub gunship: PadGunshipCfg,
}

impl Default for GamepadCfg {
    fn default() -> Self {
        GamepadCfg {
            enabled: true,
            pad_index: -1,
            look_route: "mouse".into(),
            look_sensitivity: 1.0,
            invert_y: false,
            input_graph: 3,
            turnrate_yaw: 260.0,
            turnrate_pitch: 90.0,
            turnrate_yaw_ads: 90.0,
            turnrate_pitch_ads: 55.0,
            accel: true,
            accel_lerp: 1200.0,
            accel_max_yaw: 400.0,
            rs_deadzone: 0.18,
            rs_outer: 0.98,
            ls_deadzone: 0.20,
            ls_outer: 0.98,
            trigger_threshold: 30,
            hold_ms: 300,
            mouse_deg_per_count: 0.0,
            gunship_counts_per_deg: 0.0,
            rumble: false,
            log_every_s: 10,
            blank_er: ["a", "b", "x", "y", "lb", "rb", "lt", "rt", "l3", "r3", "dpad_up", "dpad_down", "dpad_left", "dpad_right", "rs", "ls_fp"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            er_interact: "y".into(),
            er_melee: "rb".into(),
            er_use_item: "x".into(),
            prompt_indices: vec![19],
            bind: PadBindCfg::default(),
            gunship: PadGunshipCfg::default(),
        }
    }
}

/// `[gamepad.bind]`: MW2-mode actions ("" = unbound; chords of two buttons as "l3+r3"; `start` never).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PadBindCfg {
    pub toggle: String,
    pub fp_toggle: String,
    pub fire: String,
    pub ads: String,
    pub reload: String,
    pub interact_hold: String,
    pub weapon_next: String,
    pub weapon_prev: String,
    pub jump: String,
    pub stance: String,
    pub sprint: String,
    pub melee: String,
    pub ac130: String,
    pub nuke: String,
    /// 2026-10-21: one ER use-item press (`er_use_item`): the selected quick item, the flask.
    pub heal: String,
}

impl Default for PadBindCfg {
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        PadBindCfg {
            toggle: s("l3+r3"),
            fp_toggle: s("dpad_left"),
            fire: s("rt"),
            ads: s("lt"),
            reload: s("x"),
            interact_hold: s("x"),
            weapon_next: s("y"),
            weapon_prev: s("lb"),
            jump: s("a"),
            stance: s("b"),
            sprint: s("l3"),
            melee: s("r3"),
            ac130: s("dpad_up"),
            nuke: s("dpad_down"),
            heal: s("dpad_right"),
        }
    }
}

/// `[gamepad.gunship]`: AC-130 ride actions.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PadGunshipCfg {
    pub fire: String,
    pub next: String,
    pub thermal: String,
    pub exit: String,
}

impl Default for PadGunshipCfg {
    fn default() -> Self {
        PadGunshipCfg { fire: "rt".into(), next: "y".into(), thermal: "x".into(), exit: "back".into() }
    }
}

impl GamepadCfg {
    /// The resolved, validated bindings.
    pub fn binds(&self) -> Result<crate::pad_logic::Binds, String> {
        let b = &self.bind;
        let g = &self.gunship;
        crate::pad_logic::Binds::resolve(
            &[
                ("toggle", b.toggle.as_str()),
                ("fp_toggle", b.fp_toggle.as_str()),
                ("fire", b.fire.as_str()),
                ("ads", b.ads.as_str()),
                ("reload", b.reload.as_str()),
                ("interact_hold", b.interact_hold.as_str()),
                ("weapon_next", b.weapon_next.as_str()),
                ("weapon_prev", b.weapon_prev.as_str()),
                ("jump", b.jump.as_str()),
                ("stance", b.stance.as_str()),
                ("sprint", b.sprint.as_str()),
                ("melee", b.melee.as_str()),
                ("ac130", b.ac130.as_str()),
                ("nuke", b.nuke.as_str()),
                ("heal", b.heal.as_str()),
            ],
            &[("fire", g.fire.as_str()), ("next", g.next.as_str()), ("thermal", g.thermal.as_str()), ("exit", g.exit.as_str())],
            &self.blank_er,
            &self.er_interact,
        )
        .and_then(|b| b.with_er_melee(&self.er_melee))
        .and_then(|b| b.with_er_use_item(&self.er_use_item))
    }

    pub fn look(&self) -> crate::pad_logic::LookParams {
        crate::pad_logic::LookParams {
            yaw: self.turnrate_yaw,
            pitch: self.turnrate_pitch,
            yaw_ads: self.turnrate_yaw_ads,
            pitch_ads: self.turnrate_pitch_ads,
            sens: self.look_sensitivity,
            invert_y: self.invert_y,
            accel: self.accel,
            accel_lerp: self.accel_lerp,
            accel_max_yaw: self.accel_max_yaw,
            dz: self.rs_deadzone,
            outer: self.rs_outer,
            graph: self.input_graph,
        }
    }

    pub fn route_mouse(&self) -> bool {
        self.look_route == "mouse"
    }
}

/// `[ui_yield]` (C3, docs/C3_DESIGN.md 2.3). Hot. Render-only: firing is never blocked.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UiYieldCfg {
    pub enabled: bool,
    /// "lower" (the gun slides down by `drop_in`) | "hide" (not drawn above frac 0.95) | "cutout" (C5: the gun stays,
    /// it is not drawn inside the message's screen regions `cutout_*`) | "off".
    pub mode: String,
    /// The viewmodel offset at full yield, inches in the renderer's axes (x forward, y left, z up).
    pub drop_in: [f32; 3],
    /// Alpha of the dimmed HUD texts (switch list, lock text, nuke hints, toast) at full yield.
    pub hud_dim_alpha: f32,
    /// `ui_states` indices that count as a message [I: banner / timed notification / co-op message, `learn` confirms].
    pub indices: Vec<u32>,
    pub use_popup: bool,
    pub use_announce: bool,
    pub use_status: bool,
    pub use_summon: bool,
    pub use_blink: bool,
    /// 60 s per-index duration statistics (`UI learn idx=..` lines).
    pub learn: bool,
    /// The toast position as a fraction of the screen width / height (was the top-left corner, over ER's gauges).
    pub toast_anchor: [f32; 2],
    /// A signal on longer than this is ignored until it clears (a misread signal never keeps the gun down).
    pub stuck_s: f32,
    /// C5 `mode = "cutout"`: the soft edge outside each region, as a fraction of the screen height.
    pub cutout_feather: f32,
    /// C5: the screen regions of each signal's message, [x0, y0, x1, y1] as fractions of the screen (0,0 = top-left),
    /// at most 4 per signal (and 4 drawn at once, strongest first). Defaults are from ER's layout [I] (see Default).
    pub cutout_popup: Vec<[f32; 4]>,
    pub cutout_announce: Vec<[f32; 4]>,
    pub cutout_status: Vec<[f32; 4]>,
    pub cutout_summon: Vec<[f32; 4]>,
    pub cutout_blink: Vec<[f32; 4]>,
    /// The configured `indices` (interaction prompts in the learn logs: idx 15 / 17 / 19 / 65 open 1-4 s near objects).
    pub cutout_idx: Vec<[f32; 4]>,
}

impl Default for UiYieldCfg {
    fn default() -> Self {
        UiYieldCfg {
            enabled: true,
            mode: "lower".into(),
            drop_in: [-2.0, 0.0, -9.0],
            hud_dim_alpha: 0.35,
            indices: vec![15, 24, 66],
            use_popup: true,
            use_announce: true,
            use_status: true,
            use_summon: true,
            use_blink: true,
            learn: false,
            toast_anchor: [0.012, 0.30],
            stuck_s: 20.0,
            // C5 defaults, all [I] (ER's 16:9 layout from memory, not measured on a screenshot):
            cutout_feather: 0.025,
            // dialogs / yes-no boxes / tutorial popups: the screen centre
            cutout_popup: vec![[0.28, 0.30, 0.72, 0.70]],
            // system announcements (multiplayer / network notices): a top-centre band
            cutout_announce: vec![[0.20, 0.06, 0.80, 0.26]],
            // proc-status messages ("cannot use here", item acquired): lower centre, above the prompt box
            cutout_status: vec![[0.30, 0.60, 0.70, 0.74]],
            // summon / phantom messages: a top-centre band
            cutout_summon: vec![[0.20, 0.06, 0.80, 0.26]],
            // big centre text (LOST GRACE DISCOVERED, area names, GREAT ENEMY FELLED, YOU DIED): a centre band
            cutout_blink: vec![[0.08, 0.36, 0.92, 0.64]],
            // interaction prompts ("Sit at grace", "Open", "Pick up"): the bottom-centre box; + subtitles below it
            cutout_idx: vec![[0.32, 0.70, 0.68, 0.86], [0.12, 0.86, 0.88, 0.97]],
        }
    }
}

impl UiYieldCfg {
    pub fn params(&self) -> crate::ui_yield::YieldParams {
        crate::ui_yield::YieldParams { stuck_s: self.stuck_s as f64, ..crate::ui_yield::YieldParams::default() }
    }

    /// C5: (signal bit, its regions) in the order ties are kept (`ui_yield::cutout_rects`).
    pub fn cutout_regions(&self) -> [(u32, &[[f32; 4]]); 6] {
        use crate::ui_yield::*;
        [
            (SIG_POPUP, &self.cutout_popup),
            (SIG_ANNOUNCE, &self.cutout_announce),
            (SIG_STATUS, &self.cutout_status),
            (SIG_SUMMON, &self.cutout_summon),
            (SIG_BLINK, &self.cutout_blink),
            (SIG_INDEX, &self.cutout_idx),
        ]
    }

    /// The yield moves / hides / cuts our draws (mode is not "off").
    pub fn active(&self) -> bool {
        self.enabled && self.mode != "off"
    }

    pub fn cutout(&self) -> bool {
        self.enabled && self.mode == "cutout"
    }
}

/// `[ui]` (C3, docs/C3_DESIGN.md 2.4; C4, docs/C4_UI_DESIGN.md 3.1). `probe_scaleform` is cold; the two C4 keys are
/// cold to turn on (hooks and D3D12 objects are boot work) and hot to turn off (passthrough at once).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UiCfg {
    /// Read-only detours on ER's Scaleform D3D12 HAL BeginScene / EndScene and (C4) ER's UiBegin / UiEnd and
    /// hudhook's ExecuteCommandLists that only log (`UI probe ...`).
    pub probe_scaleform: bool,
    /// C4: draw the viewmodel (world guns, scope overlay) in our own command list spliced ahead of ER's UI, so ER's
    /// messages appear over the gun. Any precondition miss draws at Present as before. Default off.
    pub inject_viewmodel: bool,
    /// C4: also on ER's HDR path (the UI layer `[S+0x100]`, UNORM only); off = HDR frames stay at Present.
    pub inject_ui_layer: bool,
    /// C4.1 probe P2 (docs/C4_1_DESIGN.md 2.1 / 2.4, cold, requires `probe_scaleform`): every 5 s our own compute list
    /// reads 16 texels of ER's UI layer (no barrier on ER's resource) to pin its alpha convention
    /// (`UI probe41 sample ..`). The only probe part that issues GPU work.
    pub probe_layer_sample: bool,
    /// C4.1 PASS I41 (docs/C4_1_DESIGN.md 3, cold, requires `inject_viewmodel`): record the early list on ER's render
    /// thread (UiBegin's own thread rule, the pinned chain to the UI layer `[S+0x100]`, the layer's alpha convention).
    /// With it `inject_ui_layer` takes its C4.1 meaning: also allow ER's HDR output (`b13c = 1`). Default off.
    pub inject_ui_thread: bool,
}

impl Default for UiCfg {
    fn default() -> Self {
        UiCfg { probe_scaleform: false, inject_viewmodel: false, inject_ui_layer: false, probe_layer_sample: false, inject_ui_thread: false }
    }
}

impl UiCfg {
    /// The HDR UI-layer path is only taken together with the injection (`inject_ui_layer` alone is ignored).
    pub fn ui_layer_effective(&self) -> bool {
        self.inject_viewmodel && self.inject_ui_layer
    }

    /// C4.1: the UI-thread early pass only together with the injection (`inject_ui_thread` alone is ignored).
    pub fn ui_thread_effective(&self) -> bool {
        self.inject_viewmodel && self.inject_ui_thread
    }

    /// Boot-time warnings of `[ui]` (logged as `CONFIG WARN ...`; not errors: the keys are just ignored).
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.inject_ui_layer && !self.inject_viewmodel {
            w.push("ui.inject_ui_layer = true without ui.inject_viewmodel is ignored".to_string());
        }
        if self.probe_layer_sample && !self.probe_scaleform {
            w.push("ui.probe_layer_sample = true without ui.probe_scaleform is ignored".to_string());
        }
        if self.inject_ui_thread && !self.inject_viewmodel {
            w.push("ui.inject_ui_thread = true without ui.inject_viewmodel is ignored".to_string());
        }
        w
    }
}

/// `[spawn]` (docs/M7_DESIGN.md 2.1.3): emitter watchdog thresholds (hot).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SpawnCfg {
    /// `SPAWN WARN emitters` once the live emitters reach this.
    pub emitter_warn: u32,
    /// Low-priority spawns (extra pellets, launcher rings) are dropped from here on.
    pub emitter_throttle: u32,
    /// Slots kept for the AC-130 and the nuke (normal spawns stop at 256 - reserve).
    pub emitter_reserve: u32,
}

impl Default for SpawnCfg {
    fn default() -> Self {
        SpawnCfg { emitter_warn: 192, emitter_throttle: 224, emitter_reserve: 32 }
    }
}

/// `[nuke]` (docs/M7_DESIGN.md 3.14). Hot except `assets_dir`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Nuke {
    pub enabled: bool,
    /// Testing: the nuke can be called without a 25-kill streak (log lines carry `test=1`).
    pub always_available: bool,
    /// Co-op C2: true (default, user decision 2026-10-19) = the local nuke also works while a Seamless peer is in the
    /// world (`NUKE activate ... coop_peers=<n> role=`, a one-time HUD note); false = key 4 refused with a peer present
    /// (`NUKE blocked reason=coop_session`) and a countdown cancelled with refund when one joins.
    pub allow_in_coop: bool,
    /// Kills in one life to earn it (MW2 killstreaktable.csv cost 25).
    pub streak_cost: u32,
    /// An earned nuke survives the player's death until used [I MW2 behaviour].
    pub keep_earned_on_death: bool,
    /// Kill radius around the player (MW2: the whole map).
    pub radius_m: f32,
    /// true = merchants / friendly NPCs live (deny-list of `spare_teams`); false = MW2 "everyone dies".
    pub spare_friendly: bool,
    /// Runtime team types spared with `spare_friendly`.
    pub spare_teams: Vec<u8>,
    /// Run 15 safety: census + selection + `NUKE kill would-apply ...` lines, but no lethal effect on any NPC (the
    /// team offset is [I] and ER autosaves: a wrongly killed merchant stays dead). The player still dies.
    pub dry_run: bool,
    /// "speffect" (one-shot 100 % max-HP SpEffect through the engine) | "bullet" | "both".
    pub kill_mode: String,
    pub kills_per_tick: u32,
    pub bullets_per_tick: u32,
    /// Survivors are re-applied this long after the first kill pass (seconds).
    pub retry_s: Vec<f64>,
    /// The player dies too, like MW2.
    pub player_dies: bool,
    /// The player's lethal effect comes this long after the enemy kill pass.
    pub player_delay_s: f64,
    /// The white flash at T (stands in for MW2's nuke_flash fx; the full-screen post-pass form is ours).
    pub flash: bool,
    /// The aftermath tint is held this long after its 5 s blend, then faded out over 2 s.
    pub aftermath_hold_s: f64,
    /// Camera shake (MW2 has none: the earthquake line is commented out; its values are the default).
    pub shake: bool,
    pub shake_scale: f32,
    pub shake_s: f64,
    /// ER explosions on the horizon (visual-only bullet row) at MW2's nukeDistance.
    pub horizon_count: u32,
    /// The horizon explosions only while fewer emitters than this are live.
    pub horizon_max_live: u32,
    /// "US" | "UK" | "none" (MW2 announcer voice).
    pub announcer: String,
    /// The dpad_nuke icon next to the counter.
    pub hud_icon: bool,
    pub gain: f32,
    /// converted\nuke (not hot).
    pub assets_dir: String,
    /// Any stage running this much longer than planned ends the sequence (`NUKE WARN stuck`).
    pub stuck_s: f64,
    /// C3 PASS C (docs/C3_DESIGN.md 3.8): "owned" = the caller sends a synced marker, every peer's game runs the
    /// sequence and kills the enemies it owns, then its own player; "local" = the C2 behaviour (this game only).
    pub coop_mode: String,
    /// In a session with `coop_mode = "owned"`: no radius (MW2 "everyone dies").
    pub coop_everyone: bool,
    /// Census passes every 0.25 s from the first kill pass for this long (ownership may move).
    pub coop_window_s: f64,
    /// Review 2026-10-24: in an owned co-op nuke the host's game kills first; a client (or an unknown role) holds its
    /// kills this long after its own first kill pass, then kills every enemy still alive (clamped to the window).
    pub coop_client_fallback_s: f64,
    /// Not built in C3 (a synced lethal twin for peer-owned NPCs); must stay false.
    pub probe_forward: bool,
}

impl Default for Nuke {
    fn default() -> Self {
        Nuke {
            enabled: true,
            always_available: false,
            allow_in_coop: true,
            streak_cost: 25,
            keep_earned_on_death: true,
            radius_m: 200.0,
            spare_friendly: true,
            spare_teams: vec![0, 2, 8, 10, 12, 20, 26, 28, 30, 47],
            dry_run: true,
            kill_mode: "speffect".into(),
            kills_per_tick: 16,
            bullets_per_tick: 8,
            retry_s: vec![0.5, 1.5, 3.0, 5.0],
            player_dies: true,
            player_delay_s: 3.0,
            flash: true,
            aftermath_hold_s: 8.0,
            shake: true,
            shake_scale: 0.6,
            shake_s: 10.0,
            horizon_count: 3,
            horizon_max_live: 160,
            announcer: "US".into(),
            hud_icon: true,
            gain: 1.0,
            assets_dir: r"converted\nuke".into(),
            stuck_s: 30.0,
            coop_mode: "owned".into(),
            coop_everyone: true,
            coop_window_s: 6.0,
            coop_client_fallback_s: 1.0,
            probe_forward: false,
        }
    }
}

impl Config {
    fn validate_m7(&self) -> Result<(), String> {
        crate::emitter_guard::GuardCfg { warn: self.spawn.emitter_warn, throttle: self.spawn.emitter_throttle, reserve: self.spawn.emitter_reserve }
            .validate()?;
        let n = &self.nuke;
        if !(1..=100).contains(&n.streak_cost) {
            return Err("nuke.streak_cost must be 1..100".into());
        }
        in_range_f32("nuke.radius_m", n.radius_m, 5.0, 2000.0)?;
        if n.spare_teams.len() > 64 {
            return Err("nuke.spare_teams lists more than 64 teams".into());
        }
        if !matches!(n.kill_mode.as_str(), "speffect" | "bullet" | "both") {
            return Err("nuke.kill_mode must be speffect | bullet | both".into());
        }
        if !(1..=64).contains(&n.kills_per_tick) {
            return Err("nuke.kills_per_tick must be 1..64".into());
        }
        if !(1..=32).contains(&n.bullets_per_tick) {
            return Err("nuke.bullets_per_tick must be 1..32".into());
        }
        if n.retry_s.len() > 8 {
            return Err("nuke.retry_s lists more than 8 retries".into());
        }
        let mut prev = 0.0;
        for (i, &r) in n.retry_s.iter().enumerate() {
            in_range(&format!("nuke.retry_s[{i}]"), r, 0.05, 30.0)?;
            if r <= prev {
                return Err("nuke.retry_s must be increasing".into());
            }
            prev = r;
        }
        in_range("nuke.player_delay_s", n.player_delay_s, 1.0, 30.0)?;
        in_range("nuke.aftermath_hold_s", n.aftermath_hold_s, 0.0, 120.0)?;
        in_range_f32("nuke.shake_scale", n.shake_scale, 0.0, 5.0)?;
        in_range("nuke.shake_s", n.shake_s, 0.0, 60.0)?;
        if n.horizon_count > 8 {
            return Err("nuke.horizon_count must be 0..8".into());
        }
        if !(16..=crate::emitter_guard::CAP).contains(&n.horizon_max_live) {
            return Err("nuke.horizon_max_live must be 16..256".into());
        }
        if !matches!(n.announcer.as_str(), "US" | "UK" | "none") {
            return Err("nuke.announcer must be US | UK | none".into());
        }
        in_range_f32("nuke.gain", n.gain, 0.0, 4.0)?;
        if n.assets_dir.trim().is_empty() {
            return Err("nuke.assets_dir must not be empty".into());
        }
        in_range("nuke.stuck_s", n.stuck_s, 5.0, 600.0)?;
        // the nuke key (MW2 +actionslot 4): never shared with another binding, never E (ER interact), never one of
        // the keys the movement controller hides from ER
        let k = &self.keys;
        let Some(d) = dik_from_name(&k.nuke) else {
            return Err(format!("keys.nuke = {:?} is not a known key", k.nuke));
        };
        if d == crate::move_input::DIK_E {
            return Err("keys.nuke must not be E (ER interact)".into());
        }
        let others = [
            ("keys.toggle", &k.toggle),
            ("keys.reload", &k.reload),
            ("keys.fp_toggle", &k.fp_toggle),
            ("keys.ac130", &k.ac130),
            ("keys.ac130_exit", &k.ac130_exit),
            ("keys.ac130_next", &k.ac130_next),
            ("keys.ac130_gun1", &k.ac130_gun1),
            ("keys.ac130_gun2", &k.ac130_gun2),
            ("keys.ac130_gun3", &k.ac130_gun3),
            ("keys.ac130_thermal", &k.ac130_thermal),
            ("keys.move_forward", &k.move_forward),
            ("keys.move_back", &k.move_back),
            ("keys.move_left", &k.move_left),
            ("keys.move_right", &k.move_right),
            ("keys.move_sprint", &k.move_sprint),
            ("keys.move_jump", &k.move_jump),
            ("keys.move_crouch", &k.move_crouch),
            ("keys.move_prone", &k.move_prone),
            ("keys.move_stance_hold", &k.move_stance_hold),
            ("keys.weapon_next", &k.weapon_next),
            ("keys.weapon_prev", &k.weapon_prev),
            ("keys.heal", &k.heal),
        ];
        for (o, v) in others {
            if !v.is_empty() && dik_from_name(v) == Some(d) {
                return Err(format!("keys.nuke = {:?} conflicts with {o}", k.nuke));
            }
        }
        if self.movement.blank_er.iter().any(|b| dik_from_name(b) == Some(d)) {
            return Err(format!("keys.nuke = {:?} is in movement.blank_er", k.nuke));
        }
        let rn = &self.regulation.nuke;
        for (name, id) in [("lethal_speffect_id", rn.lethal_speffect_id), ("lethal_bullet_id", rn.lethal_bullet_id), ("horizon_bullet_id", rn.horizon_bullet_id)] {
            if !(79_000_000..=79_099_999).contains(&id) {
                return Err(format!("regulation.nuke.{name} must be in the reserved 79000000..79099999 range"));
            }
        }
        Ok(())
    }
}

/// `[regulation.nuke]` (docs/M7_DESIGN.md 3.13): build-time only.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RegNuke {
    /// The one-shot 100 % max-HP SpEffect (enemies and the player).
    pub lethal_speffect_id: i64,
    /// Its vanilla clone source (17039: changeHpRate 100, one-shot).
    pub clone_speffect_from: i64,
    /// Bullet + AtkParam_Pc of the optional bullet kill mode.
    pub lethal_bullet_id: i64,
    /// The visual-only horizon explosion (atkId_Bullet -1).
    pub horizon_bullet_id: i64,
    pub horizon_sfx: i64,
}

impl Default for RegNuke {
    fn default() -> Self {
        RegNuke {
            lethal_speffect_id: 79_000_960,
            clone_speffect_from: 17_039,
            lethal_bullet_id: 79_000_961,
            horizon_bullet_id: 79_000_962,
            horizon_sfx: 524_028,
        }
    }
}

/// `[arsenal]` (docs/M6_DESIGN.md sections 2.7, 4, 5, 6). Hot except `dir`, `guns`, `preload`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Arsenal {
    pub enabled: bool,
    /// converted\guns (relative to the project root); not hot.
    pub dir: String,
    /// The cycle order (internal names); not hot. Empty = every gun of converted\guns\index.json.
    pub guns: Vec<String>,
    /// The gun in hand at the first MODE on ("" = `gameplay.weapon`).
    pub start: String,
    /// Reserve at a refill: "max" (maxAmmo, the verified UMP45 behaviour) or "start" (startAmmo - clip) [I].
    pub reserve_source: crate::arsenal::ReserveSource,
    /// Mouse wheel switches guns (down = next).
    pub wheel: bool,
    pub wheel_invert: bool,
    /// "all" = every gun loaded at boot ("neighbours" is reserved; not built in v1).
    pub preload: String,
    /// "after_fire" (rechamber starts fireTime after the shot) | "overlap" [I].
    pub rechamber_mode: String,
    /// Double Barrel: one press fires both barrels [I].
    pub both_barrels: bool,
    /// "shot" = one band for all pellets | "pellet" [I].
    pub pellet_bands: String,
    /// Shotgun pellets beyond minDamageRange are not spawned [I MW2].
    pub shotgun_range_cap: bool,
    /// Pellets spawn only while the bullet pool has room (bug 2 mitigation).
    pub pellet_pool_gate: bool,
    /// The world FOV follows the gun's adsZoomFov while aiming.
    pub ads_fov: bool,
    /// Mouse sensitivity scaled by tan(fov/2) / tan(hip/2) while aiming [I].
    pub ads_sens_scale: bool,
    /// Sniper hold breath on the sprint key while scoped.
    pub hold_breath: bool,
    /// ADS sway: degrees per unit of adsIdleAmount [I] (1/60: the Intervention wanders ~1 deg).
    pub sway_deg_per_unit: f32,
    /// Bug 2: re-read the owner and retry once when a spawn failure is classified `owner`.
    pub owner_retry: bool,
    /// Bug 2: `SPAWN pool` line every this many spawns (and on world entry / every failure).
    pub pool_log_every: u32,
}

/// Review M6: the most guns the arsenal can hold (u64 flag masks, mw2er-render MAX_GUNS).
pub const MAX_ARSENAL_GUNS: usize = 64;

/// `[launchers]` (section 6.6).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Launchers {
    /// "dll" = simulated projectiles (default) | "engine" (reserved).
    pub flight: String,
    /// Stinger / Javelin lock onto ER characters (MW2: air targets only; a deviation). false = dumbfire.
    pub lock: bool,
    pub max_live: u32,
    /// "ignore" | "add" projectileSpeedUp to the rockets [I].
    pub rocket_speed_up: String,
    /// A direct character hit also spawns the impact row (MW2 1000 vehicle impact) [I].
    pub rocket_impact: bool,
    /// 2 = core + ring spheres, 1 = single.
    pub explosion_tiers: u8,
    pub max_ray_m: f32,
    /// Review M6: an explosion whose sphere (+ `self_guard_margin_m`) reaches the player's capsule is a dud (ER's
    /// handling of selfTarget 0 on the cloned explosion rows is not verified). false = explode anyway.
    pub self_guard: bool,
    pub self_guard_margin_m: f32,
    /// Review M6: the Javelin boosts after ignition up to this speed at this acceleration [I] (MW2 cruises far faster
    /// than its 15.2 m/s launch projectileSpeed; the engine dvars are not in the dumps).
    pub javelin_cruise_mps: f32,
    pub javelin_accel_mps2: f32,
}

/// `[third_person]` (C6 part 18, docs/C6_HOLD_DESIGN.md part 18). Hot. The over-the-shoulder camera of MW2 mode in
/// third person: the render camera (`CSCamera.pers_cam_1` row 3) moved by the camera-step detour; needs the camera-step
/// hook (installed with `[first_person] enabled` + `camera_mode = "hook"`, or `[ac130] enabled`). Off while both
/// offsets are zero (the code default).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThirdPerson {
    /// `[right, up, back]` metres from ER's camera (right = horizontal right of the view, up = world up, back =
    /// against the view forward; negative back = closer).
    pub shoulder_offset_m: [f32; 3],
    /// The same while the MW2 ADS is held (or the C6 guard / crossbow aim): lerped by the ADS fraction.
    pub shoulder_ads_offset_m: [f32; 3],
    /// "right" | "left" (mirrors the right component).
    pub shoulder_side: String,
    /// Blend time (seconds, ~95 %) in / out / between hip and ADS, and of the spring arm letting out.
    pub shoulder_blend_s: f32,
    /// The spring arm stops this far before a wall / object hit (metres).
    pub shoulder_margin_m: f32,
    /// Also on Torrent.
    pub shoulder_mounted: bool,
}

impl Default for ThirdPerson {
    fn default() -> Self {
        ThirdPerson {
            shoulder_offset_m: [0.0; 3],
            shoulder_ads_offset_m: [0.0; 3],
            shoulder_side: "right".into(),
            shoulder_blend_s: 0.15,
            shoulder_margin_m: 0.20,
            shoulder_mounted: false,
        }
    }
}

impl ThirdPerson {
    /// The resolved shoulder keys (side falls back to right; validated before use).
    pub fn shoulder(&self) -> crate::shoulder::ShoulderParams {
        crate::shoulder::ShoulderParams {
            offset: self.shoulder_offset_m,
            ads_offset: self.shoulder_ads_offset_m,
            side: crate::shoulder::Side::parse(&self.shoulder_side).unwrap_or(crate::shoulder::Side::Right),
            blend_s: self.shoulder_blend_s,
            margin_m: self.shoulder_margin_m,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if crate::shoulder::Side::parse(&self.shoulder_side).is_none() {
            return Err(format!("third_person.shoulder_side {:?} must be right | left", self.shoulder_side));
        }
        for (n, v) in [("shoulder_offset_m", self.shoulder_offset_m), ("shoulder_ads_offset_m", self.shoulder_ads_offset_m)] {
            for (k, (lim, axis)) in [(1.5f32, "right"), (1.0, "up"), (2.0, "back")].into_iter().enumerate() {
                in_range_f32(&format!("third_person.{n}[{k}] ({axis})"), v[k], -lim, lim)?;
            }
        }
        in_range_f32("third_person.shoulder_blend_s", self.shoulder_blend_s, 0.0, 2.0)?;
        in_range_f32("third_person.shoulder_margin_m", self.shoulder_margin_m, 0.0, 1.0)?;
        Ok(())
    }
}

/// `[kick]` (section 8).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Kick {
    pub enabled: bool,
    pub scale: f32,
    /// Third person: the camera is kicked too (false: only the bullet direction).
    pub camera_tp: bool,
    pub gun_kick: bool,
    pub gun_scale: f32,
    pub sway: bool,
}

impl Default for Arsenal {
    fn default() -> Self {
        Arsenal {
            enabled: true,
            dir: r"converted\guns".into(),
            guns: Vec::new(),
            start: String::new(),
            reserve_source: crate::arsenal::ReserveSource::Max,
            wheel: true,
            wheel_invert: false,
            preload: "all".into(),
            rechamber_mode: "after_fire".into(),
            both_barrels: false,
            pellet_bands: "shot".into(),
            shotgun_range_cap: true,
            pellet_pool_gate: true,
            ads_fov: true,
            ads_sens_scale: true,
            hold_breath: true,
            sway_deg_per_unit: 1.0 / 60.0,
            owner_retry: true,
            pool_log_every: 50,
        }
    }
}

impl Default for Launchers {
    fn default() -> Self {
        Launchers {
            flight: "dll".into(),
            lock: true,
            max_live: 2,
            rocket_speed_up: "ignore".into(),
            rocket_impact: false,
            explosion_tiers: 2,
            max_ray_m: 300.0,
            self_guard: true,
            self_guard_margin_m: 0.5,
            javelin_cruise_mps: 60.0,
            javelin_accel_mps2: 40.0,
        }
    }
}

impl Default for Kick {
    fn default() -> Self {
        Kick { enabled: true, scale: 1.0, camera_tp: true, gun_kick: true, gun_scale: 1.0, sway: true }
    }
}

impl Config {
    /// M6: the start gun (`[arsenal] start`, or `gameplay.weapon` for old configs).
    pub fn start_gun(&self) -> &str {
        if self.arsenal.start.trim().is_empty() { self.gameplay.weapon.as_str() } else { self.arsenal.start.as_str() }
    }

    fn validate_arsenal(&self) -> Result<(), String> {
        let a = &self.arsenal;
        let k = &self.keys;
        if a.dir.trim().is_empty() {
            return Err("arsenal.dir must not be empty".into());
        }
        if !a.start.trim().is_empty() && self.gameplay.weapon != Gameplay::default().weapon && self.gameplay.weapon != a.start {
            return Err(format!("arsenal.start = {:?} and gameplay.weapon = {:?} both set (gameplay.weapon is the old alias)", a.start, self.gameplay.weapon));
        }
        // review M6: the per-gun enable / failure flags are u64 masks (viewmodel.rs; the renderer's MAX_GUNS = 64)
        if a.guns.len() > MAX_ARSENAL_GUNS {
            return Err(format!("arsenal.guns lists {} guns (at most {MAX_ARSENAL_GUNS})", a.guns.len()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for g in &a.guns {
            if !seen.insert(g.as_str()) {
                return Err(format!("arsenal.guns lists {g:?} twice"));
            }
            if !g.ends_with("_mp") || g.contains(['/', '\\', '.']) {
                return Err(format!("arsenal.guns: {g:?} is not an internal MP weapon name"));
            }
        }
        if !a.guns.is_empty() && !a.guns.iter().any(|g| g == self.start_gun()) {
            return Err(format!("the start gun {:?} is not in arsenal.guns", self.start_gun()));
        }
        if !matches!(a.preload.as_str(), "all" | "neighbours") {
            return Err("arsenal.preload must be all | neighbours".into());
        }
        if !matches!(a.rechamber_mode.as_str(), "after_fire" | "overlap") {
            return Err("arsenal.rechamber_mode must be after_fire | overlap".into());
        }
        if !matches!(a.pellet_bands.as_str(), "shot" | "pellet") {
            return Err("arsenal.pellet_bands must be shot | pellet".into());
        }
        in_range_f32("arsenal.sway_deg_per_unit", a.sway_deg_per_unit, 0.0, 1.0)?;
        if !(1..=10_000).contains(&a.pool_log_every) {
            return Err("arsenal.pool_log_every must be 1..10000".into());
        }
        // section 5.1: the weapon keys may share keys with the AC-130 gun keys (the gunship suspends ground controls)
        // but not with any other binding, and never E (ER interact)
        for (what, name) in [("keys.weapon_next", &k.weapon_next), ("keys.weapon_prev", &k.weapon_prev)] {
            if name.is_empty() {
                continue;
            }
            let Some(d) = dik_from_name(name) else {
                return Err(format!("{what} = {name:?} is not a known key"));
            };
            if d == crate::move_input::DIK_E {
                return Err(format!("{what} must not be E (ER interact)"));
            }
            let others = [
                ("keys.toggle", &k.toggle),
                ("keys.reload", &k.reload),
                ("keys.fp_toggle", &k.fp_toggle),
                ("keys.ac130", &k.ac130),
                ("keys.ac130_exit", &k.ac130_exit),
                ("keys.move_forward", &k.move_forward),
                ("keys.move_back", &k.move_back),
                ("keys.move_left", &k.move_left),
                ("keys.move_right", &k.move_right),
                ("keys.move_sprint", &k.move_sprint),
                ("keys.move_jump", &k.move_jump),
                ("keys.move_crouch", &k.move_crouch),
                ("keys.move_prone", &k.move_prone),
                ("keys.move_stance_hold", &k.move_stance_hold),
            ];
            for (o, v) in others {
                if !v.is_empty() && dik_from_name(v) == Some(d) {
                    return Err(format!("{what} = {name:?} conflicts with {o}"));
                }
            }
        }
        if !k.weapon_next.is_empty() && k.weapon_next.eq_ignore_ascii_case(&k.weapon_prev) {
            return Err("keys.weapon_next and keys.weapon_prev must differ".into());
        }
        let l = &self.launchers;
        if !matches!(l.flight.as_str(), "dll" | "engine") {
            return Err("launchers.flight must be dll | engine".into());
        }
        if !matches!(l.rocket_speed_up.as_str(), "ignore" | "add") {
            return Err("launchers.rocket_speed_up must be ignore | add".into());
        }
        if !(1..=8).contains(&l.max_live) {
            return Err("launchers.max_live must be 1..8".into());
        }
        if !matches!(l.explosion_tiers, 1 | 2) {
            return Err("launchers.explosion_tiers must be 1 or 2".into());
        }
        in_range_f32("launchers.max_ray_m", l.max_ray_m, 10.0, 2000.0)?;
        in_range_f32("launchers.self_guard_margin_m", l.self_guard_margin_m, 0.0, 10.0)?;
        in_range_f32("launchers.javelin_cruise_mps", l.javelin_cruise_mps, 0.0, 500.0)?;
        in_range_f32("launchers.javelin_accel_mps2", l.javelin_accel_mps2, 0.0, 1000.0)?;
        let kk = &self.kick;
        in_range_f32("kick.scale", kk.scale, 0.0, 5.0)?;
        in_range_f32("kick.gun_scale", kk.gun_scale, 0.0, 5.0)?;
        let d = &self.damage;
        if !matches!(d.mode.as_str(), "flat_inv" | "linear") {
            return Err("damage.mode must be flat_inv | linear".into());
        }
        in_range("damage.s", d.s, 0.05, 20.0)?;
        in_range("damage.d_ref", d.d_ref, 1.0, 2000.0)?;
        let ra = &self.regulation.arsenal;
        if !matches!(ra.bullet_sfx.as_str(), "none" | "dagger") {
            return Err("regulation.arsenal.bullet_sfx must be none | dagger".into());
        }
        if !(79_100_000..=79_900_000).contains(&ra.id_base) {
            return Err("regulation.arsenal.id_base must be in 79100000..79900000".into());
        }
        Ok(())
    }
}

/// Paths relative to the project root (`<dll_dir>\..\..`). Not hot-reloadable.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Paths {
    pub log_dir: String,
    pub weapons_json: String,
    pub sounds_json: String,
    pub rows_json: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShotLog {
    All,
    None,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogCfg {
    /// tracing level for hudhook's own events: error | warn | info | debug (never trace).
    pub hudhook_level: String,
    pub shots: ShotLog,
    pub state_interval_s: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Keys {
    pub toggle: String,
    pub reload: String,
    /// `DIMOUSESTATE.rgbButtons` index of the fire button (0 = LMB).
    pub fire_mouse_button: u8,
    pub blank_toggle_key: bool,
    /// `rgbButtons` index of the ADS button (1 = RMB); 255 = no ADS (M3).
    pub ads_mouse_button: u8,
    /// M3b: first person on/off while MW2 mode is on (hidden from the game when `blank_toggle_key`).
    pub fp_toggle: String,
    /// M4: call in the AC-130 (and end a ride early).
    pub ac130: String,
    /// M4: extra exit key while in the gunship.
    pub ac130_exit: String,
    /// M4: next gun (the mouse wheel also cycles).
    pub ac130_next: String,
    pub ac130_gun1: String,
    pub ac130_gun2: String,
    pub ac130_gun3: String,
    /// M4: white hot / black hot (MW2 `+activate`).
    pub ac130_thermal: String,
    /// M5: the MW2 PC movement binds (`default_mp_controls.cfg`).
    pub move_forward: String,
    pub move_back: String,
    pub move_left: String,
    pub move_right: String,
    /// MW2 `+breath_sprint`.
    pub move_sprint: String,
    /// MW2 `+gostand`: jump, or stand up from crouch / prone.
    pub move_jump: String,
    /// MW2 `togglecrouch`.
    pub move_crouch: String,
    /// MW2 `toggleprone`.
    pub move_prone: String,
    /// M6: next / previous gun (MW2 binds 1 and 2 to weapnext); "" = none.
    pub weapon_next: String,
    pub weapon_prev: String,
    /// Optional console-style stance key: tap = crouch, hold 300 ms (`cl_stanceHoldTime`) = prone. Empty = none.
    pub move_stance_hold: String,
    /// M7: call the Tactical Nuke (MW2 `+actionslot 4`); hidden from the game like keys 1 / 2.
    pub nuke: String,
    /// 2026-10-21: drink the selected ER quick item (the flask) while MW2 mode drives: ER's own use-item key
    /// (`er_use_item`) is pressed for it once; hidden from the game ("" = none). May share a key with the gunship-only
    /// AC-130 keys (default Q = ac130_next).
    pub heal: String,
    /// ER's use-item key as bound in ER's own key config (default R, which MW2 mode otherwise takes for reload).
    pub er_use_item: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum CameraSource {
    #[serde(rename = "pers_cam_1")]
    PersCam1,
    #[serde(rename = "chr_cam")]
    ChrCam,
}

impl CameraSource {
    pub fn label(self) -> &'static str {
        match self {
            CameraSource::PersCam1 => "pers_cam_1",
            CameraSource::ChrCam => "chr_cam",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Gameplay {
    pub weapon: String,
    pub spawn_bullets: bool,
    pub refill_on_mode_enable: bool,
    pub auto_reload_on_empty: bool,
    pub max_shots_per_tick: u32,
    pub apply_spread_to_bullets: bool,
    pub use_move_spread: bool,
    pub camera_source: CameraSource,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Aim {
    pub max_range_m: f32,
    pub char_radius_m: f32,
    pub chest_height_m: f32,
    pub origin_ahead_m: f32,
    /// Expected sign of det(camera rows 0..2) for the camera gate: 1 or -1 once CAMFRAME shows it, 0 = either.
    pub camera_det_sign: i8,
    /// M3 camera validity gate on shots (default on). Off = the M2 behaviour (sign tracker + distance check),
    /// an escape hatch in case the gate rejects a good camera in game (CAMFRAME why=...).
    pub camera_gate: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Damage {
    /// ER `atkPhys = round(mw2_damage * k)`; baked into the rows by build_regulation.py (not hot). M6: the legacy
    /// UMP45 rows 79000100-105 only.
    pub k: f64,
    /// M6: "flat_inv" (atkPhys = inv_er(s * mw2, d_ref), flat damage) | "linear" (A/B: round(mw2 * k)).
    pub mode: String,
    /// M6: target ER damage per MW2 point on the reference enemy (49/40: UMP45 close = 49 on c4311).
    pub s: f64,
    /// M6: the reference effective defence (c4311 fitted from the flat AC-130 hits) [I].
    pub d_ref: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Regulation {
    pub id_base: i64,
    pub marker_id: i64,
    pub clone_bullet_from: i64,
    pub clone_atk_from: i64,
    pub bullet_speed_mps: f64,
    pub bullet_life_s: f64,
    pub marker_life_s: f64,
    /// weapon name -> slot (row ids `id_base + slot*100 + band`).
    pub weapons: BTreeMap<String, u32>,
    /// M4: the AC-130 rows (read by build_regulation.py).
    pub ac130: RegAc130,
    /// M6: the arsenal rows (read by build_regulation.py).
    pub arsenal: RegArsenal,
    /// M7: the nuke rows (read by build_regulation.py).
    pub nuke: RegNuke,
}

/// `[regulation.arsenal]` (docs/M6_DESIGN.md section 9): build-time only.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RegArsenal {
    pub id_base: i64,
    /// "none" = sfxId_Hit / sfxId_Flick -1 on every M6 row (bug 2 likely fix) | "dagger" (the clone effects).
    pub bullet_sfx: String,
    /// true = the DLL fires the legacy UMP45 rows 79000100-105 (A/B); they stay in the package either way.
    pub legacy_ump45: bool,
    pub clone_explosion_from: i64,
}

impl Default for RegArsenal {
    fn default() -> Self {
        RegArsenal { id_base: 79_100_000, bullet_sfx: "none".into(), legacy_ump45: false, clone_explosion_from: 10_030_001 }
    }
}

/// `[regulation.ac130]` (docs/M4_DESIGN.md section 3.2): modelled so the file parses; build-time only.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RegAc130 {
    /// atkPhys = round(mw2 * k * scale_<gun>), flat damage (run 8 fix).
    pub k: f64,
    pub scale_105: f64,
    pub scale_40: f64,
    pub scale_25: f64,
    pub clone_bullet_from: i64,
    pub clone_atk_from: i64,
    pub clone_speffect_from: i64,
    pub protect_speffect_id: i64,
    /// effectEndurance of the protection SpEffect (seconds, finite): a copy the DLL fails to remove expires on its
    /// own; the DLL re-applies it every `protect_refresh_s` while riding (review M4 major 2).
    pub protect_endurance_s: f64,
    /// Impact visuals (after run 9): the core/single row's burst and the ring row's dust/debris layer (-1 = none).
    pub sfx_105: i64,
    pub sfx_105_ring: i64,
    pub sfx_40: i64,
    pub sfx_40_ring: i64,
    pub sfx_25: i64,
}

impl Default for RegAc130 {
    fn default() -> Self {
        RegAc130 {
            k: 0.65,
            scale_105: 1.0,
            scale_40: 1.0,
            scale_25: 0.7,
            clone_bullet_from: 10_030_001,
            clone_atk_from: 10_030_001,
            clone_speffect_from: 1540,
            protect_speffect_id: 79_000_950,
            protect_endurance_s: 90.0,
            // == build_regulation.py AC130_SFX_DEFAULT (vanilla player Bullet rows 10610051, 10470003, 10183001,
            // 10471009; docs/M4_DESIGN.md section 20)
            sfx_105: 524_028,
            sfx_105_ring: 530_123,
            sfx_40: 300_602,
            sfx_40_ring: 530_133,
            sfx_25: 530_133,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BaseViewCfg {
    /// Look at the call-in point (the default).
    Center,
    /// MW2's fixed tag pitch (64.5 deg down) with the yaw towards the centre.
    Mw2Tag,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VisionMode {
    /// ac130.vision / ac130_inverted.vision (singleplayer look).
    Sp,
    /// thermal_mp.vision / missilecam.vision (what MW2 MP sets).
    Mp,
}

impl VisionMode {
    pub fn label(self) -> &'static str {
        match self {
            VisionMode::Sp => "sp",
            VisionMode::Mp => "mp",
        }
    }

    /// (white hot, black hot) vision names in `vision.json`.
    pub fn names(self) -> (&'static str, &'static str) {
        match self {
            VisionMode::Sp => ("ac130", "ac130_inverted"),
            VisionMode::Mp => ("thermal_mp", "missilecam"),
        }
    }
}

/// `[ac130.audio]`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Ac130Audio {
    pub gain: f32,
    pub ambient_gain: f32,
    pub explosion_gain: f32,
    pub explosion_floor_gain: f32,
    pub announce_ready: bool,
}

impl Default for Ac130Audio {
    fn default() -> Self {
        Ac130Audio { gain: 0.8, ambient_gain: 1.0, explosion_gain: 1.0, explosion_floor_gain: 0.35, announce_ready: true }
    }
}

/// `[ac130.tracers]` (run 8 feature: MW2-style round tracers drawn in our own D3D12 pass before the thermal film).
/// Hot. The old `[ac130] tracers = true|false` flag still parses (= `enabled`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Ac130Tracers {
    pub enabled: bool,
    /// Impact flash after a round lands.
    pub flash: bool,
    pub flash_s: f32,
    /// Smallest half width in pixels at 1080p (scaled with the screen height).
    pub min_half_px: f32,
    /// Streak brightness at its tail (head = 1).
    pub tail: f32,
    /// Muzzle position relative to the gunship camera at fire time: right, up, forward (metres).
    pub muzzle_offset_m: [f32; 3],
    /// Core width (m), streak length (m), peak brightness, impact flash radius (m) per gun.
    pub width_105: f32,
    pub length_105: f32,
    pub brightness_105: f32,
    pub flash_105: f32,
    pub width_40: f32,
    pub length_40: f32,
    pub brightness_40: f32,
    pub flash_40: f32,
    pub width_25: f32,
    pub length_25: f32,
    pub brightness_25: f32,
    pub flash_25: f32,
    /// After run 9: per-gun impact flash length (s; 0 = `flash_s`), peak gain and a wide soft thermal bloom radius
    /// around the flash (m; 0 = none).
    pub flash_s_105: f32,
    pub flash_s_40: f32,
    pub flash_s_25: f32,
    pub flash_gain_105: f32,
    pub flash_gain_40: f32,
    pub flash_gain_25: f32,
    pub bloom_105: f32,
    pub bloom_40: f32,
    pub bloom_25: f32,
}

impl Default for Ac130Tracers {
    fn default() -> Self {
        // MW2 look: the 105 one big slow glowing round, the 40 medium rounds, the 25 a rapid thin stream [I: by eye]
        Ac130Tracers {
            enabled: true,
            flash: true,
            flash_s: 0.25,
            min_half_px: 1.0,
            tail: 0.15,
            muzzle_offset_m: [-1.5, -2.5, 0.0],
            width_105: 1.2,
            length_105: 14.0,
            brightness_105: 1.0,
            // after run 9: the 105 impact is the biggest flash (was 6 m)
            flash_105: 14.0,
            width_40: 0.6,
            length_40: 10.0,
            brightness_40: 0.9,
            flash_40: 2.5,
            width_25: 0.25,
            length_25: 22.0,
            brightness_25: 0.8,
            flash_25: 0.8,
            flash_s_105: 0.5,
            flash_s_40: 0.3,
            flash_s_25: 0.0,
            flash_gain_105: 1.6,
            flash_gain_40: 1.0,
            flash_gain_25: 1.0,
            bloom_105: 32.0,
            bloom_40: 7.0,
            bloom_25: 0.0,
        }
    }
}

impl Ac130Tracers {
    pub fn params(&self) -> TracerParams {
        TracerParams {
            styles: [
                TracerStyle {
                    width_m: self.width_105,
                    length_m: self.length_105,
                    brightness: self.brightness_105,
                    flash_m: self.flash_105,
                    flash_s: self.flash_s_105,
                    flash_gain: self.flash_gain_105,
                    bloom_m: self.bloom_105,
                },
                TracerStyle {
                    width_m: self.width_40,
                    length_m: self.length_40,
                    brightness: self.brightness_40,
                    flash_m: self.flash_40,
                    flash_s: self.flash_s_40,
                    flash_gain: self.flash_gain_40,
                    bloom_m: self.bloom_40,
                },
                TracerStyle {
                    width_m: self.width_25,
                    length_m: self.length_25,
                    brightness: self.brightness_25,
                    flash_m: self.flash_25,
                    flash_s: self.flash_s_25,
                    flash_gain: self.flash_gain_25,
                    bloom_m: self.bloom_25,
                },
            ],
            flash: self.flash,
            flash_s: self.flash_s,
            min_half_px: self.min_half_px,
            tail: self.tail,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TracersIn {
    Flag(bool),
    Table(Ac130Tracers),
}

fn de_tracers<'de, D: Deserializer<'de>>(d: D) -> Result<Ac130Tracers, D::Error> {
    Ok(match TracersIn::deserialize(d)? {
        TracersIn::Flag(b) => Ac130Tracers { enabled: b, ..Ac130Tracers::default() },
        TracersIn::Table(t) => t,
    })
}

/// `[ac130.shake]` (after run 9): the gunship camera shake of MW2's `earthquake(scale, duration, origin, radius)`
/// calls (converted\ac130\timing.json), applied in the camera write (`mw2er_core::ac130_shake`). Hot.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Ac130Shake {
    pub enabled: bool,
    /// Multiplies MW2's earthquake scale.
    pub scale: f32,
    /// View jitter (yaw and pitch) in degrees per unit of MW2 scale [I].
    pub rot_deg: f32,
    /// Roll in degrees per unit [I].
    pub roll_deg: f32,
    /// Camera position offset in metres per unit [I].
    pub pos_m: f32,
    /// Jitter frequency [I].
    pub freq_hz: f32,
    /// MW2's linear radius falloff from the quake origin (the impact quakes then barely reach the gunship, which
    /// flies ~158 m from the impact; MW2's impact radius is 3500 in = 89 m). Off = every quake at full strength.
    pub mw2_falloff: bool,
}

impl Default for Ac130Shake {
    fn default() -> Self {
        Ac130Shake { enabled: true, scale: 1.0, rot_deg: 2.0, roll_deg: 0.6, pos_m: 1.5, freq_hz: 11.0, mw2_falloff: false }
    }
}

impl Ac130Shake {
    /// The pure-logic parameters; `legacy_scale` is the old `[ac130] shake_scale` (multiplies, 1 = no change).
    pub fn params(&self, legacy_scale: f32) -> crate::ac130_shake::ShakeParams {
        crate::ac130_shake::ShakeParams {
            scale: if self.enabled { self.scale * legacy_scale } else { 0.0 },
            rot_deg: self.rot_deg,
            roll_deg: self.roll_deg,
            pos_m: self.pos_m,
            freq_hz: self.freq_hz,
            mw2_falloff: self.mw2_falloff,
        }
    }
}

/// `[ac130]` (M4, docs/M4_DESIGN.md section 3.2). Hot except `assets_dir` (the loader runs once) and `enabled` for the
/// camera-step hook (installed at boot when `enabled` is set; turning it on later needs a restart).
/// A value of 0 means "the MW2 value from converted\ac130\*.json" where noted.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Ac130 {
    pub enabled: bool,
    pub assets_dir: String,
    pub cooldown_s: f64,
    pub cooldown_enabled: bool,
    /// M6 run-13 bug 1: an F7 whose start aborted (loading, cutscene, talk, camera type, read / protect failure) blocks
    /// the next F7 this long (`AC130 blocked reason=retry_backoff`).
    pub start_backoff_s: f64,
    /// 0 = MW2 (timing.json, 40 s).
    pub duration_s: f64,
    /// Multiplies radius and altitude.
    pub scale: f32,
    /// 0 = MW2 rig (116.0 m).
    pub radius_m: f32,
    /// 0 = MW2 rig (107.0 m), above the call-in feet.
    pub altitude_m: f32,
    /// 0 = MW2 (70 s per turn).
    pub orbit_period_s: f32,
    /// 1 = yaw increasing (MW2), -1 = reverse.
    pub orbit_dir: i8,
    pub base_view: BaseViewCfg,
    /// 0 = MW2 (35 deg each side).
    pub aim_arc_deg: f32,
    /// MW2 cg_fov degrees per gun (4:3 Hor+), converted to ER vertical radians. 0 = the converted MW2 data
    /// (weapons_ac130.json ads_zoom_fov: 45/20/5; review M4 minor 9). Using adsZoomFov as the gunship zoom is [I]
    /// (all three dumps have aimDownSight 0).
    pub fov_105: f32,
    pub fov_40: f32,
    pub fov_25: f32,
    pub fov_is_vertical: bool,
    pub mouse_deg_per_count: f32,
    pub mouse_sens: f32,
    pub zoom_sens_scale: bool,
    pub invert_y: bool,
    pub vision: VisionMode,
    pub thermal: bool,
    pub start_black_hot: bool,
    pub grain: bool,
    pub luma_weights: [f32; 3],
    /// Pre-exposure of the thermal film input (review M4 major 8, [I]): scales the luma before MW2's film step so
    /// ER's lit frame is not clipped to white (1 = MW2's vision as is). Hot.
    pub exposure: f32,
    /// Mid-tone lift of the film input after the exposure (`g^gamma`; 1 = MW2 as is; run 8: "a little bit too dark").
    /// Hot.
    pub gamma: f32,
    pub hide_er_hud: bool,
    pub require_open_sky: bool,
    pub max_player_move_m: f32,
    /// 2 = core + ring (additive), 1 = single sphere.
    pub explosion_tiers: u8,
    pub shells_after_exit: bool,
    pub max_ray_m: f32,
    #[serde(deserialize_with = "de_tracers")]
    pub tracers: Ac130Tracers,
    pub debug_markers: bool,
    /// Fallback 1 (section 4.7): also write ChrCam.pers_cam. Off by default.
    pub sync_chr_cam: bool,
    /// Legacy multiplier of `[ac130.shake] scale` (the first M4 build's only shake knob; 0 = off).
    pub shake_scale: f32,
    /// After run 9: the gunship camera shake (`[ac130.shake]`).
    pub shake: Ac130Shake,
    pub log_every_ticks: u32,
    pub audio: Ac130Audio,
}

impl Default for Ac130 {
    fn default() -> Self {
        Ac130 {
            enabled: true,
            assets_dir: r"converted\ac130".into(),
            cooldown_s: 120.0,
            cooldown_enabled: true,
            start_backoff_s: 1.0,
            duration_s: 0.0,
            scale: 1.0,
            radius_m: 0.0,
            altitude_m: 0.0,
            orbit_period_s: 0.0,
            orbit_dir: 1,
            base_view: BaseViewCfg::Center,
            aim_arc_deg: 0.0,
            fov_105: 0.0,
            fov_40: 0.0,
            fov_25: 0.0,
            fov_is_vertical: true,
            mouse_deg_per_count: 0.022,
            mouse_sens: 1.0,
            zoom_sens_scale: true,
            invert_y: false,
            vision: VisionMode::Sp,
            thermal: true,
            start_black_hot: false,
            grain: true,
            luma_weights: [0.2126, 0.7152, 0.0722],
            // [I] until T10 runs on a real ER frame. Run 8 feedback ("a little bit too dark" at 0.6, gamma 1). On the
            // vmtest card: mid-tone output 0.302 -> 0.422 (+40%) at a white clip of 15.3% (was 10.4%); exposure 0.8
            // alone would give 0.382 at 34% clipped, 0.65 alone 0.324 at 15.1% (vmtest --thermal-image sweep).
            exposure: 0.52,
            gamma: 0.75,
            hide_er_hud: true,
            require_open_sky: true,
            max_player_move_m: 25.0,
            explosion_tiers: 2,
            shells_after_exit: true,
            max_ray_m: 600.0,
            tracers: Ac130Tracers::default(),
            debug_markers: false,
            sync_chr_cam: false,
            shake_scale: 1.0,
            shake: Ac130Shake::default(),
            log_every_ticks: 300,
            audio: Ac130Audio::default(),
        }
    }
}

impl Ac130 {
    /// The configured gun FOVs (cg_fov degrees) in gun order; 0 = use the MW2 data.
    pub fn fovs(&self) -> [f32; 3] {
        [self.fov_105, self.fov_40, self.fov_25]
    }

    /// The effective gun FOVs: each configured value, or (0) the MW2 data's `ads_zoom_fov` of that gun.
    pub fn resolve_fovs(&self, data: [f32; 3]) -> [f32; 3] {
        let c = self.fovs();
        [0, 1, 2].map(|k| if c[k] > 0.0 { c[k] } else { data[k] })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Hit {
    pub window_s: f64,
    pub max_chars: usize,
    pub exclude_character_ids: Vec<u32>,
    /// M3 impact attribution: an HP drop within this long after one of our impacts near the character is ours.
    pub impact_window_s: f64,
    /// How long after a drop its impact may still be recorded; the drop waits this long before it resolves
    /// (M2 run 2: the impact is seen one tick, +16..18 ms, after the drop).
    pub impact_lag_s: f64,
    /// Distance from the impact to the character's vertical axis segment.
    pub impact_radius_m: f32,
    pub capsule_height_m: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Menu {
    /// `CSMenuManImp.ui_states` indices that are visible in normal gameplay (fill from MENU logs).
    /// Empty: the visible set captured when MW2 mode is enabled is used instead.
    pub hud_indices: Vec<u32>,
    /// Use the `ui_states` visible bits as a menu signal. Off by default: the first in-game run showed
    /// HUD widgets (prompts, pickups) flipping indices constantly, which read as "menu open" and let R
    /// through to the game (flask). Turn on only once `hud_indices` is calibrated.
    pub use_ui_states: bool,
    pub use_show_cursor: bool,
    /// While `hud_indices` is empty, refuse F5 when `CSMouseMan.show_cursor` is set (a menu is
    /// probably open and would otherwise be captured into the baseline). Unverified signal: turn it
    /// off if F5 is refused with "cursor visible" while no menu is open.
    pub cursor_gate_at_enable: bool,
    /// `ui_states` indices that always mean a menu is open, cursor or not (2026-10-20: ER's pause menu on a pad shows
    /// no cursor; idx 28 is on exactly while it is open, never during play in any logged session).
    pub menu_indices: Vec<u32>,
    /// `ui_states` indices of ER's HUD: one of them that was visible when MW2 mode was enabled and is hidden now means a
    /// full-screen menu is open, cursor or not (2026-10-21: sitting at a site of grace on a pad hides idx 7 and shows no
    /// cursor; never hidden while moving or shooting in any logged session; the AC-130's own HUD hide is excluded).
    /// Empty = off (set it empty if ER's HUD option is "Auto", which hides the HUD during play).
    pub hud_off_indices: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Hud {
    pub enabled: bool,
    pub font_px: f32,
    pub font_path: String,
    pub crosshair_color: [f32; 4],
    pub crosshair_line_scale: f32,
    pub min_gap_px: f32,
    pub fallback_vfov_deg: f32,
    pub hitmarker_s: f64,
    pub show_boot_toast: bool,
    /// M5: a small stand / crouch / prone figure next to the ammo counter.
    pub stance_icon: bool,
    /// M6: the gun's name and fire-mode tag next to the ammo, the switch list, the lock-on brackets / text.
    pub weapon_name: bool,
    pub switch_list: bool,
    pub lock_ui: bool,
    /// C3 (docs/C3_DESIGN.md 2.6): floating damage numbers at each of our attributed hits.
    pub damage_numbers: bool,
    /// Life of a number (s).
    pub damage_numbers_s: f32,
    /// Hits on one target within this long merge into one number (bursts, pellets).
    pub damage_numbers_merge_s: f32,
    /// "er" (ER HP lost) | "mw2" (the shot's MW2 damage when known, else ER's).
    pub damage_numbers_value: String,
    /// Yellow for an impact in the top 18 % of the capsule [I].
    pub damage_numbers_head_guess: bool,
    pub damage_numbers_max: u32,
    /// Size of the numbers relative to the HUD font.
    pub damage_numbers_scale: f32,
    /// Hide our number while ER's own enemy tag shows the same target (ER draws its own damage there).
    pub damage_numbers_suppress_with_er_tag: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ReloadCues {
    pub lift: f64,
    pub clipout: f64,
    pub clipin: f64,
    /// Played on empty reloads only.
    pub chamber: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Audio {
    pub enabled: bool,
    pub master_gain: f32,
    pub fire_gain: f32,
    pub foley_gain: f32,
    pub fire_max_ms: u32,
    pub fire_fade_ms: u32,
    pub reload_cues: ReloadCues,
}

/// Colour output of the viewmodel renderer (docs/M3_DESIGN.md section 4.4).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmOutput {
    Auto,
    Srgb,
    Pq,
    Scrgb,
}

/// `[viewmodel]` (M3). Everything is hot except `enabled` (the loader runs once) and `assets_dir`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Viewmodel {
    pub enabled: bool,
    pub assets_dir: String,
    pub output: VmOutput,
    pub paper_white_nits: f32,
    pub hip_fov: f32,
    pub ads_fov: f32,
    pub near_in: f32,
    pub far_in: f32,
    pub offset_in: [f32; 3],
    pub light_dir_view: [f32; 3],
    pub diffuse: f32,
    pub ambient_sky: [f32; 3],
    pub ambient_ground: [f32; 3],
    pub exposure: f32,
    pub crossfade_s: f32,
    pub sprint_speed_mps: f32,
    pub gpu_timing: bool,
    /// Draw the first-person gun while first person is off (third person). Off: run 15 showed it overlaid on
    /// ER's third-person view after a respawn.
    pub draw_in_third_person: bool,
    pub log_interval_s: f64,
    pub runtime_shader_compile: bool,
}

impl Default for Viewmodel {
    fn default() -> Self {
        Viewmodel {
            enabled: true,
            assets_dir: r"converted\viewmodel".into(),
            output: VmOutput::Auto,
            paper_white_nits: 200.0,
            hip_fov: 65.0,
            ads_fov: 55.0,
            near_in: 0.1,
            far_in: 1000.0,
            offset_in: [0.0, 0.0, 0.0],
            light_dir_view: [0.35, -0.80, 0.50],
            diffuse: 0.85,
            ambient_sky: [0.42, 0.42, 0.45],
            ambient_ground: [0.20, 0.19, 0.18],
            exposure: 1.0,
            crossfade_s: 0.05,
            sprint_speed_mps: 5.0,
            gpu_timing: true,
            draw_in_third_person: false,
            log_interval_s: 10.0,
            runtime_shader_compile: false,
        }
    }
}

/// How the eye position reaches the render camera (docs/M3B_DESIGN.md section 2).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CameraMode {
    /// Tier A: overwrite `pers_cam_1` row 3 from our `ChrIns_PostPhysics` task.
    Task,
    /// Tier B: the same write from a hook on ER's camera-step update (installed at boot, 2.7.1.0 only).
    Hook,
}

impl CameraMode {
    pub fn label(self) -> &'static str {
        match self {
            CameraMode::Task => "task",
            CameraMode::Hook => "hook",
        }
    }
}

/// How the local player's body is hidden in first person (docs/M3B_DESIGN.md section 3).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HideMethod {
    /// `ChrIns.chr_flags1c5` bit 3 `enable_render` cleared.
    RenderFlag,
    /// `ChrIns.base_transparency_modifier` = -1 (engine fade).
    Fade,
    None,
}

impl HideMethod {
    pub fn label(self) -> &'static str {
        match self {
            HideMethod::RenderFlag => "render_flag",
            HideMethod::Fade => "fade",
            HideMethod::None => "none",
        }
    }
}

/// `[first_person]` (M3b). Everything is hot except `camera_mode`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FirstPerson {
    /// Run 15: reset the F6 choice to first person when the world loads again (respawn, fast travel).
    pub reset_on_respawn: bool,
    /// Run 16: consecutive invalid camera-gate ticks before first person hands back (a chunk re-base lasts 1).
    pub cam_gate_grace_ticks: u32,
    /// Run 18: while first person drives the render camera, ER's own camera closer than 0.3 m to the chest (its
    /// collision against a wagon / wall pulled it in) does not fail the camera gate (the upper bound still does).
    pub cam_gate_close_ok: bool,
    /// Master switch: off = F6 is refused and first person never arms.
    pub enabled: bool,
    pub camera_mode: CameraMode,
    /// Eye above the physics position (the feet), metres.
    pub eye_height_m: f32,
    /// Eye ahead along the horizontal view direction, metres.
    pub eye_forward_m: f32,
    /// Exponential filter on the eye height (stairs); 0 = off.
    pub eye_smooth_s: f32,
    pub pitch_clamp_deg: f32,
    pub hide_method: HideMethod,
    pub yaw_follow: bool,
    pub yaw_probe_samples: u32,
    /// 0 = keep ER's world FOV; otherwise the vertical FOV in degrees written to `pers_cam_1.fov`.
    pub world_fov_deg: f32,
    pub arm_ticks: u32,
    /// Cutscene trigger: a `CSFade` plate whose alpha rises above this while first person is on.
    pub fade_alpha_max: f32,
    pub use_fade_signal: bool,
    pub use_cam_mask_signal: bool,
    pub use_talk_signal: bool,
    /// Torrent: also block on `ride_node->ride_state != 0` (inferred values); false = `is_mounting` only.
    pub use_ride_state_signal: bool,
    /// After an edge handback (cutscene, talk, load counter) the value must be back at its pre-handback baseline
    /// this long before first person may arm again.
    pub latch_holdoff_s: f32,
    /// If the value never returns: release once it has been stable and the cursor closed this long (0 = never).
    pub latch_stale_s: f32,
    pub log_every_ticks: u32,
    /// Hook mode: also write the eye into `ChrCam.pers_cam` row 3 (ER's own player camera), as erfps2 does, so ER's
    /// follow camera and everything that reads the chr cam see the same position as the render camera.
    pub sync_chr_cam: bool,
    /// Hook mode: keep the exe's `CAM_WALL_RECOVERY` bool at 0 while first person is on (restored on handback).
    pub disable_wall_recovery: bool,
    /// Hook mode: keep `ChrExFollowCam.camera_auto_rotation` at 0 while first person is on (restored on handback).
    pub disable_auto_rotation: bool,
    /// M5: the character's local forward axis for the yaw write: `-z` (run 4, drift p95 < 1 deg) or `+z` fixed, or
    /// `auto` (the M3b probe; it only learns while ER moves the player, so it cannot run while the controller drives).
    pub yaw_forward_axis: YawAxisCfg,
    /// C3 (docs/C3_DESIGN.md 1.7): the talk latch releases once the talk id has been stable this long with the cursor
    /// and the dialog index closed (the talk id never returns to its old value).
    pub talk_stale_s: f32,
    /// C3: the `ui_states` index of the NPC dialog (3 in the C2 logs [V]); while visible the talk latch holds. -1 = off.
    pub talk_menu_idx: i32,
    /// C3: a fade plate latch releases after this long of a stable value with the cursor closed (was the general
    /// `latch_stale_s`; C2: a plate stuck at a = 0.53 kept first person off 30 s).
    pub fade_stale_s: f32,
    /// C3: F6 while first person is off for a non-user reason (talk, cutscene, menu, loading, camera gate) releases
    /// the latches and keeps first person wanted, instead of turning it off for good.
    pub f6_overrides_latches: bool,
    /// 2026-10-21: a talk id change hands back (first person and MW2 movement) only within this many seconds of an
    /// interact press (E, the pad X hold); a boss / NPC voice line changes it with no press. 0 = every change.
    pub talk_interact_window_s: f32,
    /// C6 (work/c6/scope3p.md): third-person MW2 ADS with a scoped gun (an overlay) runs a short real first-person
    /// episode (the "scope peek": eye camera, ADS-in, the scope overlay) and hands back to third person at ADS release.
    /// Off = today (third-person zoom without an overlay). Hot.
    pub tp_scope_peek: bool,
    /// C6: clear ticks before a scope peek turns first person on (1..30; only after a user-chosen third person: any
    /// other block keeps `arm_ticks`).
    pub scope_peek_arm_ticks: u32,
}

/// `[first_person] yaw_forward_axis`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum YawAxisCfg {
    #[serde(rename = "-z")]
    MinusZ,
    #[serde(rename = "+z")]
    PlusZ,
    #[serde(rename = "auto")]
    Auto,
}

impl YawAxisCfg {
    pub fn label(self) -> &'static str {
        match self {
            YawAxisCfg::MinusZ => "-z",
            YawAxisCfg::PlusZ => "+z",
            YawAxisCfg::Auto => "auto",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TpAnim {
    /// ER sees W/A/S/D and animates; the controller moves the player.
    Hybrid,
    /// Third person hands back to ER movement.
    Er,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SprintModeCfg {
    /// MW2: tap to start, release keeps sprinting, tap again to stop.
    Press,
    Hold,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CrouchMode {
    Toggle,
    Hold,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GostandFromProne {
    Stand,
    Crouch,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeaponMoveClass {
    Smg,
    Rifle,
    Mg,
    Launcher,
    /// M6: from the gun in hand (weaponClass, docs/M6_DESIGN.md section 7).
    Auto,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StanceEye {
    /// `[first_person] eye_height_m` x (60 / 40 / 11 in) / 60 in: keeps the verified 1.60 m stand eye.
    Ratio,
    /// MW2 inches x 0.0254 (1.524 / 1.016 / 0.279 m).
    Mw2,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FallDamageCfg {
    Mw2,
    Off,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LethalFall {
    /// Write HP 0 (ER's own death is expected to follow, [I]).
    Zero,
    /// Restore gravity before the impact and let ER resolve the fall.
    Handback,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GroundNormalCfg {
    Rays,
    Er,
}

/// `[movement]` (M5, docs/M5_DESIGN.md section 10). Everything is hot except `data`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Movement {
    pub enabled: bool,
    pub data: String,
    pub fp_only: bool,
    pub tp_anim: TpAnim,
    pub sprint_mode: SprintModeCfg,
    pub crouch_mode: CrouchMode,
    pub gostand_from_prone: GostandFromProne,
    pub jump_slowdown: bool,
    pub marathon: bool,
    pub lightweight: bool,
    pub weapon_move_class: WeaponMoveClass,
    /// The only non-MW2 tuning knob of the physics (0.5..2).
    pub speed_scale: f32,
    pub stance_eye: StanceEye,
    pub fall_damage: FallDamageCfg,
    pub fall_damage_scale: f32,
    pub lethal_fall: LethalFall,
    pub freefall_handback_m: f32,
    /// 0 = ER `physics.hit_radius` (clamped 0.2..0.6 m).
    pub hull_radius_m: f32,
    pub skin_m: f32,
    pub max_substep_s: f32,
    pub max_bumps: u32,
    pub max_rays_per_tick: u32,
    pub ground_normal: GroundNormalCfg,
    pub walkable_normal_y: f32,
    pub er_pushout: bool,
    pub er_pushout_max_m: f32,
    pub er_move_handback_m: f32,
    pub er_move_hold_s: f32,
    pub interact_hold_s: f32,
    /// Radius (m) of the other characters for the controller's push-out (review M5 major 3; separate from the
    /// aim's generous `aim.char_radius_m`).
    pub char_push_radius_m: f32,
    pub use_movement_limit_signal: bool,
    pub rearm_ticks: u32,
    pub publish_max_m: f32,
    /// 1 | -1: hot override of the strafe direction (section 1.4).
    pub strafe_sign: i8,
    /// Fallback: also set `ChrCtrl.chr_proxy_flags` bit 0.
    pub proxy_sync_flag: bool,
    /// Probe: also write `last_update_position` (+0x80) with the position.
    pub write_last_update: bool,
    /// Keys hidden from ER while the controller drives (latched until release).
    pub blank_er: Vec<String>,
    pub view_bob: bool,
    pub land_dip: bool,
    pub vm_stance_pose: bool,
    /// First-run key probe (MW2 mode off): logs ER's action bits per key once.
    pub keyprobe: bool,
    pub log_every_ticks: u32,
    /// M8 B2: while on a ladder LMB is still hidden from ER (no shot from the MW2 gun, no ER ladder attack); true
    /// hands LMB back to ER on a ladder (its ladder punches / kicks).
    pub ladder_er_attacks: bool,
    /// M8: `[movement.mantle]` (docs/M8_DESIGN.md section 8).
    pub mantle: MantleCfgFile,
    /// Run 18: `[movement.rays]` (the Havok filters of the controller's rays, the body probes).
    pub rays: RaysCfgFile,
    /// Run 18: `[movement.unstick]` (hand back to ER for a moment when the player makes no progress while ER pushes).
    pub unstick: UnstickCfgFile,
    /// C3: `[movement.assist]` (ER's own movement for a moment when ours stalls on a slope / root / rock).
    pub assist: AssistCfgFile,
    /// C3 PASS C (docs/C3_DESIGN.md 3.6): "hybrid" = in first person ER sees W/A/S/D and the left stick (walk / run
    /// animations, peers see them) while our position write discards its root motion; "blank" = the C2 behaviour.
    pub fp_anim: FpAnim,
    /// Not built in C3 (an animation-speed write); must stay false.
    pub fp_anim_speed_scale: bool,
    /// ER's run root motion removed from its displacement before it counts as a push (m/s; M5 5.6 / C3 3.6).
    pub fp_anim_rm_mps: f32,
}

/// `[movement] fp_anim` (C3 PASS C).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FpAnim {
    Hybrid,
    Blank,
}

impl FpAnim {
    pub fn label(self) -> &'static str {
        match self {
            FpAnim::Hybrid => "hybrid",
            FpAnim::Blank => "blank",
        }
    }
}

/// `[movement.rays]` (run 18). Hot.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RaysCfgFile {
    /// The map filter every ray used through M8 (`mov edx,2000058h` before the only call of CSPhysWorld::cast_ray in
    /// eldenring.exe, 0x1403fbc34, V).
    pub map_filter: u32,
    /// The object / asset filter (0 = off): 0x5b is the filter ER's follow camera passes to its sphere casts
    /// (`mov edx,5Bh` at 0x1403b746f / 0x1403b8da8 / 0x1403b97be / 0x1403bab31, V); the camera collides with
    /// wagons (run 18), the map filter apparently does not [I].
    pub object_filter: u32,
    /// C3 (docs/C3_DESIGN.md 1.3): "player" (one cast with the player's own proxy filter layer, read live
    /// from `CSChrPhysicsModule+0x338`), or the run-18 rollbacks "union" (both filters, the nearer hit), "map" (the
    /// M8 behaviour) and "object" (the object filter only).
    pub mode: String,
    /// The extra probes of the capsule: a waist height between the knee and the chest rays and the front body
    /// column (cart beds, tables, slabs between the probe heights).
    pub body_probes: bool,
    /// C3: the cast filter of mode "player" while the live word is unreadable or its layer is not accepted.
    pub player_fallback: u32,
    /// C3: the accepted layers of the live `physics+0x338` word (players are 2 / 3 / 4 in the table).
    pub player_layers: Vec<u8>,
    /// C3: one-shot dump of the live filter table rows (0x58, 0x5b, the cast layer) at the first `MOVE on`.
    pub log_live_table: bool,
}

impl Default for RaysCfgFile {
    fn default() -> Self {
        RaysCfgFile {
            map_filter: 0x0200_0058,
            object_filter: 0x5b,
            mode: "union".into(),
            body_probes: true,
            player_fallback: 0x3,
            player_layers: vec![2, 3, 4],
            log_live_table: true,
        }
    }
}

impl RaysCfgFile {
    /// The filters to cast with, in order (1 or 2). Mode "player" casts once with `player_fallback`; the DLL replaces
    /// it with the live layer ([`RaysCfgFile::filters_with`]).
    pub fn filters(&self) -> ([u32; 2], usize) {
        self.filters_with(self.player_fallback & 0x7f)
    }

    /// The filters with the `player` mode's cast filter (the live layer or the fallback, from
    /// `collision_layers::player_cast`).
    pub fn filters_with(&self, player_cast: u32) -> ([u32; 2], usize) {
        match self.mode.as_str() {
            "player" => ([if player_cast & 0x7f != 0 { player_cast & 0x7f } else { self.player_fallback & 0x7f }, 0], 1),
            "map" => ([self.map_filter, 0], 1),
            "object" => ([self.object_filter, 0], 1),
            _ if self.object_filter == 0 || self.object_filter == self.map_filter => ([self.map_filter, 0], 1),
            _ => ([self.map_filter, self.object_filter], 2),
        }
    }

    pub fn is_player(&self) -> bool {
        self.mode == "player"
    }

    fn validate(&self) -> Result<(), String> {
        if !matches!(self.mode.as_str(), "player" | "union" | "map" | "object") {
            return Err("movement.rays.mode must be player | union | map | object".into());
        }
        if self.map_filter == 0 {
            return Err("movement.rays.map_filter must not be 0".into());
        }
        if self.mode == "object" && self.object_filter == 0 {
            return Err("movement.rays.mode = object needs a non-zero object_filter".into());
        }
        if self.player_fallback & !0x7f != 0 || self.player_fallback == 0 {
            return Err("movement.rays.player_fallback must be a layer 0x1..0x7f (bits 7-31 zero)".into());
        }
        if self.player_layers.is_empty() || self.player_layers.len() > 16 || self.player_layers.iter().any(|&l| l > 0x7f) {
            return Err("movement.rays.player_layers must list 1..16 layers, each 0..0x7f".into());
        }
        Ok(())
    }
}

/// `[movement.assist]` (C3, docs/C3_DESIGN.md 1.4). Hot.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AssistCfgFile {
    pub enabled: bool,
    /// ER-world speed below this (m/s) while a direction is held counts as stalled.
    pub stall_speed: f32,
    pub stall_s: f32,
    /// ER drives this long (0.3..2.0 s; repeats within 3 s double it up to 2.0 s).
    pub hold_s: f32,
    pub max_per_min: u32,
    /// A mantle refusal while stalled skips the `stall_s` wait.
    pub after_refusal: bool,
}

impl Default for AssistCfgFile {
    fn default() -> Self {
        AssistCfgFile { enabled: true, stall_speed: 0.15, stall_s: 0.4, hold_s: 0.8, max_per_min: 8, after_refusal: true }
    }
}

impl AssistCfgFile {
    pub fn params(&self) -> crate::move_assist::AssistParams {
        crate::move_assist::AssistParams {
            enabled: self.enabled,
            stall_speed: self.stall_speed,
            stall_s: self.stall_s as f64,
            hold_s: self.hold_s as f64,
            max_per_min: self.max_per_min,
            after_refusal: self.after_refusal,
        }
    }

    fn validate(&self) -> Result<(), String> {
        in_range_f32("movement.assist.stall_speed", self.stall_speed, 0.01, 2.0)?;
        in_range_f32("movement.assist.stall_s", self.stall_s, 0.1, 5.0)?;
        in_range_f32("movement.assist.hold_s", self.hold_s, 0.3, 2.0)?;
        if !(1..=30).contains(&self.max_per_min) {
            return Err("movement.assist.max_per_min must be 1..30".into());
        }
        Ok(())
    }
}

/// `[movement.unstick]` (run 18). Hot.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UnstickCfgFile {
    pub enabled: bool,
    /// The observation window (s): W/A/S/D held for `wish_frac` of it, the feet moved less than `progress_m`.
    pub window_s: f32,
    pub progress_m: f32,
    pub wish_frac: f32,
    /// ...and at least this fraction of its ticks had penetration evidence (ER's push-out applied, a probe inside).
    pub evidence_frac: f32,
    /// The first handback lasts `hold_s`; each further one within `backoff_window_s` doubles it, up to `max_hold_s`.
    pub hold_s: f32,
    pub max_hold_s: f32,
    /// At most this many unsticks per `backoff_window_s`; then the detector rests until the window frees.
    pub max_per_window: u32,
    pub backoff_window_s: f32,
}

impl Default for UnstickCfgFile {
    fn default() -> Self {
        UnstickCfgFile {
            enabled: true,
            window_s: 2.5,
            progress_m: 0.5,
            wish_frac: 0.8,
            evidence_frac: 0.1,
            hold_s: 1.0,
            max_hold_s: 8.0,
            max_per_window: 4,
            backoff_window_s: 60.0,
        }
    }
}

impl UnstickCfgFile {
    pub fn params(&self) -> crate::move_logic::UnstickParams {
        crate::move_logic::UnstickParams {
            enabled: self.enabled,
            window_s: self.window_s as f64,
            progress_m: self.progress_m,
            wish_frac: self.wish_frac,
            evidence_frac: self.evidence_frac,
            hold_s: self.hold_s as f64,
            max_hold_s: self.max_hold_s as f64,
            max_per_window: self.max_per_window,
            backoff_window_s: self.backoff_window_s as f64,
        }
    }

    fn validate(&self) -> Result<(), String> {
        in_range_f32("movement.unstick.window_s", self.window_s, 0.5, 10.0)?;
        in_range_f32("movement.unstick.progress_m", self.progress_m, 0.05, 3.0)?;
        in_range_f32("movement.unstick.wish_frac", self.wish_frac, 0.0, 1.0)?;
        in_range_f32("movement.unstick.evidence_frac", self.evidence_frac, 0.01, 1.0)?;
        in_range_f32("movement.unstick.hold_s", self.hold_s, 0.2, 10.0)?;
        in_range_f32("movement.unstick.max_hold_s", self.max_hold_s, self.hold_s, 30.0)?;
        if !(1..=20).contains(&self.max_per_window) {
            return Err("movement.unstick.max_per_window must be 1..20".into());
        }
        in_range_f32("movement.unstick.backoff_window_s", self.backoff_window_s, 5.0, 600.0)?;
        Ok(())
    }
}

/// `[movement.mantle]` (M8). Hot except `max_rays`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MantleCfgFile {
    pub enabled: bool,
    /// "mw2" (jump held or pressed, MW2 SP) | "press" | "auto" (MP automantle perk).
    pub key_mode: String,
    pub require_forward: bool,
    /// Top above the ground left (the user's cap, I), 1.0..2.5 m.
    pub max_height_m: f32,
    /// Top above the current feet (MW2 60, V), 40..80 in; above 60 the rise time scales (I).
    pub max_rise_in: f32,
    pub check_angle_deg: f32,
    pub check_range_in: f32,
    /// 1.0 = MW2 SP timing.
    pub time_scale: f32,
    /// "thin" (cross thin walls with a floor behind) | "off".
    pub over: String,
    pub over_max_drop_m: f32,
    pub crouch_fallback: bool,
    pub low_keeps_weapon: bool,
    /// "drop" (MW2 quick drop / raise) | "sprint" (the sprint pose).
    pub vm: String,
    pub sound: bool,
    /// Cold: the mantle's own ray budget per tick (detection + re-validation).
    pub max_rays: u32,
    pub refuse_cache_ms: u32,
    pub log_refusals: bool,
    /// Run 18: a face nearer than the capsule radius (the capsule already reaches into it) is backed off by up to
    /// this much (two rays prove the space behind) and the detection goes on from there; 0 = refuse path_blocked.
    pub depenetrate_m: f32,
}

impl Default for MantleCfgFile {
    fn default() -> Self {
        MantleCfgFile {
            enabled: true,
            key_mode: "mw2".into(),
            require_forward: false,
            max_height_m: 2.0,
            max_rise_in: 60.0,
            check_angle_deg: 60.0,
            check_range_in: 20.0,
            time_scale: 1.0,
            over: "thin".into(),
            over_max_drop_m: 2.0,
            crouch_fallback: true,
            low_keeps_weapon: true,
            vm: "drop".into(),
            sound: true,
            max_rays: 24,
            refuse_cache_ms: 150,
            log_refusals: true,
            depenetrate_m: 0.30,
        }
    }
}

impl MantleCfgFile {
    /// The controller's settings (`end_speed_max` / `er_push_abort_in` are set by the caller).
    pub fn sim(&self) -> crate::mantle::MantleCfg {
        use crate::mantle::{KeyMode, MantleCfg, VmMode};
        MantleCfg {
            enabled: self.enabled,
            key_mode: match self.key_mode.as_str() {
                "press" => KeyMode::Press,
                "auto" => KeyMode::Auto,
                _ => KeyMode::Mw2,
            },
            require_forward: self.require_forward,
            max_height_in: self.max_height_m / 0.0254,
            max_rise_in: self.max_rise_in,
            check_angle_deg: self.check_angle_deg,
            check_range_in: self.check_range_in,
            time_scale: self.time_scale,
            over: self.over == "thin",
            over_max_drop_in: self.over_max_drop_m / 0.0254,
            crouch_fallback: self.crouch_fallback,
            low_keeps_weapon: self.low_keeps_weapon,
            vm: if self.vm == "sprint" { VmMode::Sprint } else { VmMode::Drop },
            max_rays: self.max_rays,
            refuse_cache_s: self.refuse_cache_ms as f64 / 1000.0,
            er_push_abort_in: 0.30 / 0.0254,
            end_speed_max: 190.0,
            depenetrate_in: self.depenetrate_m / 0.0254,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !matches!(self.key_mode.as_str(), "mw2" | "press" | "auto") {
            return Err("movement.mantle.key_mode must be mw2 | press | auto".into());
        }
        if !matches!(self.over.as_str(), "thin" | "off") {
            return Err("movement.mantle.over must be thin | off".into());
        }
        if !matches!(self.vm.as_str(), "drop" | "sprint") {
            return Err("movement.mantle.vm must be drop | sprint".into());
        }
        in_range_f32("movement.mantle.max_height_m", self.max_height_m, 1.0, 2.5)?;
        in_range_f32("movement.mantle.max_rise_in", self.max_rise_in, 40.0, 80.0)?;
        in_range_f32("movement.mantle.check_angle_deg", self.check_angle_deg, 0.0, 90.0)?;
        in_range_f32("movement.mantle.check_range_in", self.check_range_in, 4.0, 40.0)?;
        in_range_f32("movement.mantle.time_scale", self.time_scale, 0.5, 2.0)?;
        in_range_f32("movement.mantle.over_max_drop_m", self.over_max_drop_m, 0.5, 3.0)?;
        // M8 review: the detection alone needs up to mantle::DETECT_MAX_RAYS (run 18: 24 with the depenetration and the
        // narrow-top retry)
        if !(crate::mantle::DETECT_MAX_RAYS..=40).contains(&self.max_rays) {
            return Err(format!("movement.mantle.max_rays must be {}..40", crate::mantle::DETECT_MAX_RAYS));
        }
        if self.refuse_cache_ms > 1000 {
            return Err("movement.mantle.refuse_cache_ms must be 0..1000".into());
        }
        in_range_f32("movement.mantle.depenetrate_m", self.depenetrate_m, 0.0, 0.4)?;
        Ok(())
    }
}

impl Default for Movement {
    fn default() -> Self {
        Movement {
            enabled: true,
            data: r"converted\movement.json".into(),
            fp_only: false,
            tp_anim: TpAnim::Hybrid,
            sprint_mode: SprintModeCfg::Press,
            crouch_mode: CrouchMode::Toggle,
            gostand_from_prone: GostandFromProne::Stand,
            jump_slowdown: true,
            marathon: false,
            lightweight: false,
            weapon_move_class: WeaponMoveClass::Smg,
            speed_scale: 1.0,
            stance_eye: StanceEye::Ratio,
            fall_damage: FallDamageCfg::Mw2,
            fall_damage_scale: 1.0,
            lethal_fall: LethalFall::Handback,
            freefall_handback_m: 30.0,
            hull_radius_m: 0.0,
            skin_m: 0.03,
            max_substep_s: 0.0167,
            max_bumps: 3,
            max_rays_per_tick: 48,
            ground_normal: GroundNormalCfg::Rays,
            walkable_normal_y: 0.7,
            er_pushout: true,
            er_pushout_max_m: 0.30,
            er_move_handback_m: 0.30,
            er_move_hold_s: 0.5,
            interact_hold_s: 1.5,
            char_push_radius_m: 0.45,
            use_movement_limit_signal: false,
            rearm_ticks: 10,
            publish_max_m: 1.0,
            strafe_sign: 1,
            proxy_sync_flag: false,
            write_last_update: false,
            blank_er: crate::move_input::BLANK_ER_DEFAULT.iter().map(|s| s.to_string()).collect(),
            view_bob: true,
            land_dip: true,
            vm_stance_pose: true,
            keyprobe: true,
            log_every_ticks: 60,
            ladder_er_attacks: false,
            mantle: MantleCfgFile::default(),
            rays: RaysCfgFile::default(),
            unstick: UnstickCfgFile::default(),
            assist: AssistCfgFile::default(),
            fp_anim: FpAnim::Hybrid,
            fp_anim_speed_scale: false,
            fp_anim_rm_mps: 4.1,
        }
    }
}

impl Movement {
    /// The DIK codes of `blank_er` (validated by `Config::validate`).
    pub fn blank_er_diks(&self) -> Vec<u8> {
        self.blank_er.iter().filter_map(|n| dik_from_name(n)).collect()
    }
}

impl Keys {
    /// The movement keys (validated by `Config::validate`).
    pub fn move_keys(&self) -> Result<crate::move_input::MoveKeys, String> {
        crate::move_input::MoveKeys::from_names(
            &self.move_forward,
            &self.move_back,
            &self.move_left,
            &self.move_right,
            &self.move_sprint,
            &self.move_jump,
            &self.move_crouch,
            &self.move_prone,
            &self.move_stance_hold,
        )
    }
}

impl Default for FirstPerson {
    fn default() -> Self {
        FirstPerson {
            enabled: true,
            reset_on_respawn: true,
            cam_gate_grace_ticks: 8,
            cam_gate_close_ok: true,
            camera_mode: CameraMode::Hook, // run 4: task-mode writes were overwritten by ER (er_rewrite=1)
            eye_height_m: 1.60,
            eye_forward_m: 0.10,
            eye_smooth_s: 0.0,
            pitch_clamp_deg: 85.0,
            hide_method: HideMethod::RenderFlag,
            yaw_follow: true,
            yaw_probe_samples: 60,
            world_fov_deg: 0.0,
            arm_ticks: 30,
            fade_alpha_max: 0.5,
            use_fade_signal: true,
            use_cam_mask_signal: true,
            use_talk_signal: true,
            use_ride_state_signal: true,
            latch_holdoff_s: 2.0,
            latch_stale_s: 20.0,
            log_every_ticks: 300,
            sync_chr_cam: true,
            disable_wall_recovery: true,
            disable_auto_rotation: true,
            yaw_forward_axis: YawAxisCfg::MinusZ,
            talk_stale_s: 2.0,
            talk_menu_idx: 3,
            fade_stale_s: 3.0,
            f6_overrides_latches: true,
            talk_interact_window_s: 8.0,
            tp_scope_peek: false,
            scope_peek_arm_ticks: 1,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema: 1,
            paths: Paths::default(),
            log: LogCfg::default(),
            keys: Keys::default(),
            gameplay: Gameplay::default(),
            aim: Aim::default(),
            damage: Damage::default(),
            regulation: Regulation::default(),
            hit: Hit::default(),
            menu: Menu::default(),
            hud: Hud::default(),
            audio: Audio::default(),
            viewmodel: Viewmodel::default(),
            first_person: FirstPerson::default(),
            third_person: ThirdPerson::default(),
            ac130: Ac130::default(),
            movement: Movement::default(),
            arsenal: Arsenal::default(),
            launchers: Launchers::default(),
            kick: Kick::default(),
            spawn: SpawnCfg::default(),
            nuke: Nuke::default(),
            ui_yield: UiYieldCfg::default(),
            ui: UiCfg::default(),
            gamepad: GamepadCfg::default(),
            coop: CoopCfg::default(),
        }
    }
}

impl Default for Paths {
    fn default() -> Self {
        Paths {
            log_dir: "logs".into(),
            weapons_json: r"converted\weapons.json".into(),
            sounds_json: r"converted\sounds\sounds.json".into(),
            rows_json: r"build\rows.json".into(),
        }
    }
}

impl Default for LogCfg {
    fn default() -> Self {
        LogCfg { hudhook_level: "debug".into(), shots: ShotLog::All, state_interval_s: 5.0 }
    }
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            toggle: "F5".into(),
            reload: "R".into(),
            fire_mouse_button: 0,
            blank_toggle_key: true,
            ads_mouse_button: 1,
            fp_toggle: "F6".into(),
            ac130: "F7".into(),
            ac130_exit: "Back".into(),
            ac130_next: "Q".into(),
            ac130_gun1: "1".into(),
            ac130_gun2: "2".into(),
            ac130_gun3: "3".into(),
            ac130_thermal: "F".into(),
            move_forward: "W".into(),
            move_back: "S".into(),
            move_left: "A".into(),
            move_right: "D".into(),
            move_sprint: "LSHIFT".into(),
            move_jump: "SPACE".into(),
            move_crouch: "C".into(),
            move_prone: "LCTRL".into(),
            move_stance_hold: String::new(),
            weapon_next: "1".into(),
            weapon_prev: "2".into(),
            nuke: "4".into(),
            heal: "Q".into(),
            er_use_item: "R".into(),
        }
    }
}

impl Default for Gameplay {
    fn default() -> Self {
        Gameplay {
            weapon: "ump45_mp".into(),
            spawn_bullets: true,
            refill_on_mode_enable: true,
            auto_reload_on_empty: true,
            max_shots_per_tick: 2,
            apply_spread_to_bullets: true,
            use_move_spread: true,
            camera_source: CameraSource::PersCam1,
        }
    }
}

impl Default for Aim {
    fn default() -> Self {
        Aim { max_range_m: 150.0, char_radius_m: 0.75, chest_height_m: 1.2, origin_ahead_m: 0.6, camera_det_sign: 0, camera_gate: true }
    }
}

impl Default for Damage {
    fn default() -> Self {
        Damage { k: 4.0, mode: "flat_inv".into(), s: 1.225, d_ref: 85.0 }
    }
}

impl Default for Regulation {
    fn default() -> Self {
        let mut weapons = BTreeMap::new();
        weapons.insert("ump45_mp".to_string(), 1);
        Regulation {
            id_base: 79_000_000,
            marker_id: 79_000_900,
            clone_bullet_from: 10_170_000,
            clone_atk_from: 10_170_000,
            bullet_speed_mps: 999.0,
            bullet_life_s: 0.15,
            marker_life_s: 0.1,
            weapons,
            ac130: RegAc130::default(),
            arsenal: RegArsenal::default(),
            nuke: RegNuke::default(),
        }
    }
}

impl Default for Hit {
    fn default() -> Self {
        Hit {
            window_s: 0.6,
            max_chars: 64,
            exclude_character_ids: vec![8000],
            impact_window_s: 0.25,
            impact_lag_s: 0.1,
            impact_radius_m: 1.0,
            capsule_height_m: 2.2,
        }
    }
}

impl Default for Menu {
    fn default() -> Self {
        Menu { hud_indices: Vec::new(), use_ui_states: false, use_show_cursor: true, cursor_gate_at_enable: true, menu_indices: vec![28], hud_off_indices: vec![7] }
    }
}

impl Default for Hud {
    fn default() -> Self {
        Hud {
            enabled: true,
            font_px: 26.0,
            font_path: String::new(),
            crosshair_color: [1.0, 1.0, 1.0, 0.85],
            crosshair_line_scale: 1.0,
            min_gap_px: 4.0,
            fallback_vfov_deg: 48.0,
            hitmarker_s: 0.2,
            show_boot_toast: true,
            stance_icon: true,
            weapon_name: true,
            switch_list: true,
            lock_ui: true,
            damage_numbers: true,
            damage_numbers_s: 0.9,
            damage_numbers_merge_s: 0.25,
            damage_numbers_value: "er".into(),
            damage_numbers_head_guess: true,
            damage_numbers_max: 24,
            damage_numbers_scale: 1.0,
            damage_numbers_suppress_with_er_tag: false,
        }
    }
}

impl Default for ReloadCues {
    fn default() -> Self {
        ReloadCues { lift: 0.0, clipout: 0.15, clipin: 0.55, chamber: 0.80 }
    }
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            enabled: true,
            master_gain: 0.5,
            fire_gain: 0.6,
            foley_gain: 0.8,
            fire_max_ms: 600,
            fire_fade_ms: 80,
            reload_cues: ReloadCues::default(),
        }
    }
}

/// A config error with a 1-based line/column when the TOML parser reports a position.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} {}", self.line, self.col, self.msg)
    }
}

/// Upper bounds of the `[aim]` values (they feed engine calls: ray length, bullet origin).
pub const AIM_MAX_RANGE_M: f32 = 1000.0;
pub const AIM_MAX_CHAR_RADIUS_M: f32 = 5.0;
pub const AIM_MAX_CHEST_HEIGHT_M: f32 = 3.0;
pub const AIM_MAX_ORIGIN_AHEAD_M: f32 = 5.0;

/// `lo <= v <= hi` and finite (rejects NaN and both infinities).
fn in_range(name: &str, v: f64, lo: f64, hi: f64) -> Result<(), String> {
    if v.is_finite() && (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} = {v} must be a number in {lo}..={hi}")) }
}

fn in_range_f32(name: &str, v: f32, lo: f32, hi: f32) -> Result<(), String> {
    in_range(name, v as f64, lo as f64, hi as f64)
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

impl Config {
    /// Parses and validates `mw2er.toml` text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            let (line, col) = e.span().map_or((0, 0), |s| line_col(text, s.start));
            ConfigError { line, col, msg: e.message().replace('\n', " ") }
        })?;
        cfg.validate().map_err(|msg| ConfigError { line: 0, col: 0, msg })?;
        Ok(cfg)
    }

    /// Value checks that the type system cannot express.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("schema {} unsupported (want 1)", self.schema));
        }
        for (what, name) in [("keys.toggle", &self.keys.toggle), ("keys.reload", &self.keys.reload)] {
            if dik_from_name(name).is_none() {
                return Err(format!("{what} = {name:?} is not a known key (F1-F12, A-Z, 0-9)"));
            }
        }
        if self.keys.toggle.eq_ignore_ascii_case(&self.keys.reload) {
            return Err("keys.toggle and keys.reload must differ".into());
        }
        if dik_from_name(&self.keys.fp_toggle).is_none() {
            return Err(format!("keys.fp_toggle = {:?} is not a known key (F1-F12, A-Z, 0-9)", self.keys.fp_toggle));
        }
        if self.keys.fp_toggle.eq_ignore_ascii_case(&self.keys.toggle) || self.keys.fp_toggle.eq_ignore_ascii_case(&self.keys.reload)
        {
            return Err("keys.fp_toggle must differ from keys.toggle and keys.reload".into());
        }
        if ["W", "A", "S", "D"].iter().any(|k| self.keys.fp_toggle.eq_ignore_ascii_case(k)) {
            return Err("keys.fp_toggle must not be a movement key (W/A/S/D)".into());
        }
        if self.keys.fire_mouse_button > 7 {
            return Err("keys.fire_mouse_button must be 0..7".into());
        }
        if self.keys.ads_mouse_button != 255 && self.keys.ads_mouse_button > 7 {
            return Err("keys.ads_mouse_button must be 0..7 or 255 (no ADS)".into());
        }
        if self.keys.ads_mouse_button == self.keys.fire_mouse_button {
            return Err("keys.ads_mouse_button and keys.fire_mouse_button must differ".into());
        }
        if !matches!(self.aim.camera_det_sign, -1..=1) {
            return Err("aim.camera_det_sign must be -1, 0 or 1".into());
        }
        if !matches!(self.log.hudhook_level.as_str(), "error" | "warn" | "info" | "debug") {
            return Err(format!("log.hudhook_level {:?} must be error|warn|info|debug", self.log.hudhook_level));
        }
        // Every float below is checked with an inclusive range on a finite value: TOML accepts
        // `inf` and `nan`, and a one-sided check such as `x > 1.0` lets +inf through into engine
        // calls (ray cast length, bullet origin) on a hot reload.
        in_range("log.state_interval_s", self.log.state_interval_s, 0.5, 3600.0)?;
        if !(self.damage.k > 0.0 && self.damage.k <= 100.0) {
            return Err(format!("damage.k {} must be in (0, 100]", self.damage.k));
        }
        if !(1..=8).contains(&self.gameplay.max_shots_per_tick) {
            return Err("gameplay.max_shots_per_tick must be 1..8".into());
        }
        let a = &self.aim;
        in_range_f32("aim.max_range_m", a.max_range_m, 1.0, AIM_MAX_RANGE_M)?;
        in_range_f32("aim.char_radius_m", a.char_radius_m, 0.01, AIM_MAX_CHAR_RADIUS_M)?;
        in_range_f32("aim.chest_height_m", a.chest_height_m, 0.0, AIM_MAX_CHEST_HEIGHT_M)?;
        in_range_f32("aim.origin_ahead_m", a.origin_ahead_m, 0.0, AIM_MAX_ORIGIN_AHEAD_M)?;
        in_range("hit.window_s", self.hit.window_s, 0.01, 10.0)?;
        in_range("hit.impact_window_s", self.hit.impact_window_s, 0.0, 2.0)?;
        in_range("hit.impact_lag_s", self.hit.impact_lag_s, 0.0, 0.5)?;
        in_range_f32("hit.impact_radius_m", self.hit.impact_radius_m, 0.0, 10.0)?;
        in_range_f32("hit.capsule_height_m", self.hit.capsule_height_m, 0.0, 20.0)?;
        if !(self.hit.max_chars >= 1 && self.hit.max_chars <= 1024) {
            return Err("hit.max_chars must be 1..1024".into());
        }
        if self.menu.hud_indices.iter().any(|&i| i >= 128) {
            return Err("menu.hud_indices must be < 128".into());
        }
        if self.menu.menu_indices.iter().any(|&i| i >= 128) {
            return Err("menu.menu_indices must be < 128".into());
        }
        if self.menu.hud_off_indices.iter().any(|&i| i >= 128) {
            return Err("menu.hud_off_indices must be < 128".into());
        }
        if self.menu.hud_off_indices.iter().any(|i| self.menu.menu_indices.contains(i)) {
            return Err("menu.hud_off_indices and menu.menu_indices must not share an index".into());
        }
        let h = &self.hud;
        in_range_f32("hud.font_px", h.font_px, 6.0, 128.0)?;
        in_range_f32("hud.min_gap_px", h.min_gap_px, 0.0, 500.0)?;
        in_range("hud.hitmarker_s", h.hitmarker_s, 0.01, 5.0)?;
        in_range_f32("hud.fallback_vfov_deg", h.fallback_vfov_deg, 1.0, 170.0)?;
        in_range_f32("hud.crosshair_line_scale", h.crosshair_line_scale, 0.05, 10.0)?;
        for (i, &c) in h.crosshair_color.iter().enumerate() {
            in_range_f32(&format!("hud.crosshair_color[{i}]"), c, 0.0, 1.0)?;
        }
        let au = &self.audio;
        in_range_f32("audio.master_gain", au.master_gain, 0.0, 4.0)?;
        in_range_f32("audio.fire_gain", au.fire_gain, 0.0, 4.0)?;
        in_range_f32("audio.foley_gain", au.foley_gain, 0.0, 4.0)?;
        let c = &self.audio.reload_cues;
        for (n, v) in [("lift", c.lift), ("clipout", c.clipout), ("clipin", c.clipin), ("chamber", c.chamber)] {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("audio.reload_cues.{n} must be within 0..1"));
            }
        }
        self.validate_viewmodel()?;
        self.validate_first_person()?;
        self.validate_ac130()?;
        self.validate_movement()?;
        self.validate_arsenal()?;
        self.validate_m7()?;
        self.validate_c3_ui()?;
        self.validate_gamepad()?;
        self.validate_heal()?;
        self.validate_coop()?;
        self.third_person.validate()?;
        let r = &self.regulation;
        if r.weapons.is_empty() || r.weapons.values().any(|&s| s == 0 || s >= 9) {
            return Err("regulation.weapons slots must be 1..8 (slot 9 is the marker)".into());
        }
        let ra = &r.ac130;
        if !(ra.k > 0.0 && ra.k <= 100.0) {
            return Err(format!("regulation.ac130.k {} must be in (0, 100]", ra.k));
        }
        for (n, v) in [("scale_105", ra.scale_105), ("scale_40", ra.scale_40), ("scale_25", ra.scale_25)] {
            if !(0.05..=20.0).contains(&v) {
                return Err(format!("regulation.ac130.{n} {v} must be in [0.05, 20]"));
            }
        }
        if !(79_000_000..=79_999_999).contains(&ra.protect_speffect_id) {
            return Err("regulation.ac130.protect_speffect_id must be in the reserved 79xxxxxx range".into());
        }
        if !(30.0..=600.0).contains(&ra.protect_endurance_s) {
            return Err(format!("regulation.ac130.protect_endurance_s {} must be in [30, 600] (finite; review M4 major 2)", ra.protect_endurance_s));
        }
        Ok(())
    }

    fn validate_ac130(&self) -> Result<(), String> {
        let a = &self.ac130;
        let k = &self.keys;
        let ac_keys = [
            ("keys.ac130", &k.ac130),
            ("keys.ac130_exit", &k.ac130_exit),
            ("keys.ac130_next", &k.ac130_next),
            ("keys.ac130_gun1", &k.ac130_gun1),
            ("keys.ac130_gun2", &k.ac130_gun2),
            ("keys.ac130_gun3", &k.ac130_gun3),
            ("keys.ac130_thermal", &k.ac130_thermal),
        ];
        for (what, name) in ac_keys {
            if dik_from_name(name).is_none() {
                return Err(format!("{what} = {name:?} is not a known key (F1-F12, A-Z, 0-9, Back)"));
            }
        }
        for other in [&k.toggle, &k.reload, &k.fp_toggle] {
            if k.ac130.eq_ignore_ascii_case(other) {
                return Err("keys.ac130 must differ from keys.toggle, keys.reload and keys.fp_toggle".into());
            }
        }
        if ["W", "A", "S", "D"].iter().any(|m| k.ac130.eq_ignore_ascii_case(m)) {
            return Err("keys.ac130 must not be a movement key (W/A/S/D)".into());
        }
        if a.assets_dir.trim().is_empty() {
            return Err("ac130.assets_dir must not be empty".into());
        }
        in_range("ac130.cooldown_s", a.cooldown_s, 0.0, 3600.0)?;
        in_range("ac130.start_backoff_s", a.start_backoff_s, 0.0, 30.0)?;
        if a.duration_s != 0.0 {
            in_range("ac130.duration_s (0 = MW2)", a.duration_s, 5.0, 300.0)?;
        }
        in_range_f32("ac130.scale", a.scale, 0.25, 2.0)?;
        for (n, v) in [("radius_m", a.radius_m), ("altitude_m", a.altitude_m)] {
            if v != 0.0 {
                in_range_f32(&format!("ac130.{n} (0 = MW2 rig)"), v, 10.0, 1000.0)?;
            }
        }
        if a.orbit_period_s != 0.0 {
            in_range_f32("ac130.orbit_period_s (0 = MW2)", a.orbit_period_s, 10.0, 600.0)?;
        }
        if !matches!(a.orbit_dir, -1 | 1) {
            return Err("ac130.orbit_dir must be 1 or -1".into());
        }
        if a.aim_arc_deg != 0.0 {
            in_range_f32("ac130.aim_arc_deg (0 = MW2)", a.aim_arc_deg, 1.0, 89.0)?;
        }
        for (n, v) in [("fov_105", a.fov_105), ("fov_40", a.fov_40), ("fov_25", a.fov_25)] {
            if v != 0.0 {
                in_range_f32(&format!("ac130.{n} (0 = MW2 data)"), v, 1.0, 120.0)?;
            }
        }
        in_range_f32("ac130.mouse_deg_per_count", a.mouse_deg_per_count, 0.001, 1.0)?;
        in_range_f32("ac130.mouse_sens", a.mouse_sens, 0.01, 20.0)?;
        for (i, &w) in a.luma_weights.iter().enumerate() {
            in_range_f32(&format!("ac130.luma_weights[{i}]"), w, 0.0, 1.0)?;
        }
        in_range_f32("ac130.exposure", a.exposure, 0.05, 4.0)?;
        in_range_f32("ac130.gamma", a.gamma, 0.3, 2.0)?;
        let t = &a.tracers;
        in_range_f32("ac130.tracers.flash_s", t.flash_s, 0.0, 3.0)?;
        in_range_f32("ac130.tracers.min_half_px", t.min_half_px, 0.25, 20.0)?;
        in_range_f32("ac130.tracers.tail", t.tail, 0.0, 1.0)?;
        for (i, &o) in t.muzzle_offset_m.iter().enumerate() {
            in_range_f32(&format!("ac130.tracers.muzzle_offset_m[{i}]"), o, -50.0, 50.0)?;
        }
        for g in ["105", "40", "25"] {
            let s = t.params().styles[["105", "40", "25"].iter().position(|x| *x == g).unwrap_or(0)];
            in_range_f32(&format!("ac130.tracers.width_{g}"), s.width_m, 0.0, 20.0)?;
            in_range_f32(&format!("ac130.tracers.length_{g}"), s.length_m, 0.0, 200.0)?;
            in_range_f32(&format!("ac130.tracers.brightness_{g}"), s.brightness, 0.0, 4.0)?;
            in_range_f32(&format!("ac130.tracers.flash_{g}"), s.flash_m, 0.0, 50.0)?;
            in_range_f32(&format!("ac130.tracers.flash_s_{g}"), s.flash_s, 0.0, 3.0)?;
            in_range_f32(&format!("ac130.tracers.flash_gain_{g}"), s.flash_gain, 0.0, 4.0)?;
            in_range_f32(&format!("ac130.tracers.bloom_{g}"), s.bloom_m, 0.0, 200.0)?;
        }
        if a.luma_weights.iter().sum::<f32>() < 0.01 {
            return Err("ac130.luma_weights must not all be 0".into());
        }
        in_range_f32("ac130.max_player_move_m", a.max_player_move_m, 1.0, 1000.0)?;
        if !matches!(a.explosion_tiers, 1 | 2) {
            return Err("ac130.explosion_tiers must be 1 or 2".into());
        }
        in_range_f32("ac130.max_ray_m", a.max_ray_m, 10.0, 2000.0)?;
        in_range_f32("ac130.shake_scale", a.shake_scale, 0.0, 5.0)?;
        let sh = &a.shake;
        in_range_f32("ac130.shake.scale", sh.scale, 0.0, 5.0)?;
        in_range_f32("ac130.shake.rot_deg", sh.rot_deg, 0.0, 10.0)?;
        in_range_f32("ac130.shake.roll_deg", sh.roll_deg, 0.0, 10.0)?;
        in_range_f32("ac130.shake.pos_m", sh.pos_m, 0.0, 10.0)?;
        in_range_f32("ac130.shake.freq_hz", sh.freq_hz, 0.5, 40.0)?;
        if !(30..=36_000).contains(&a.log_every_ticks) {
            return Err("ac130.log_every_ticks must be 30..36000".into());
        }
        let au = &a.audio;
        in_range_f32("ac130.audio.gain", au.gain, 0.0, 4.0)?;
        in_range_f32("ac130.audio.ambient_gain", au.ambient_gain, 0.0, 4.0)?;
        in_range_f32("ac130.audio.explosion_gain", au.explosion_gain, 0.0, 4.0)?;
        in_range_f32("ac130.audio.explosion_floor_gain", au.explosion_floor_gain, 0.0, 1.0)?;
        Ok(())
    }

    fn validate_movement(&self) -> Result<(), String> {
        let k = &self.keys;
        let mk = k.move_keys()?;
        let mut others: Vec<(&str, u8)> = Vec::new();
        for (n, v) in [("toggle", &k.toggle), ("reload", &k.reload), ("fp_toggle", &k.fp_toggle), ("ac130", &k.ac130)] {
            if let Some(d) = dik_from_name(v) {
                others.push((n, d));
            }
        }
        mk.validate(&others)?;
        let m = &self.movement;
        if m.data.trim().is_empty() {
            return Err("movement.data must not be empty".into());
        }
        in_range_f32("movement.speed_scale", m.speed_scale, 0.5, 2.0)?;
        in_range_f32("movement.fall_damage_scale", m.fall_damage_scale, 0.0, 2.0)?;
        in_range_f32("movement.freefall_handback_m", m.freefall_handback_m, 5.0, 200.0)?;
        if m.hull_radius_m != 0.0 {
            in_range_f32("movement.hull_radius_m (0 = ER hit_radius)", m.hull_radius_m, 0.2, 0.6)?;
        }
        in_range_f32("movement.skin_m", m.skin_m, 0.005, 0.1)?;
        in_range_f32("movement.max_substep_s", m.max_substep_s, 0.004, 0.05)?;
        if !(1..=8).contains(&m.max_bumps) {
            return Err("movement.max_bumps must be 1..8".into());
        }
        if !(8..=128).contains(&m.max_rays_per_tick) {
            return Err("movement.max_rays_per_tick must be 8..128".into());
        }
        in_range_f32("movement.walkable_normal_y", m.walkable_normal_y, 0.3, 0.99)?;
        in_range_f32("movement.er_pushout_max_m", m.er_pushout_max_m, 0.0, 1.0)?;
        in_range_f32("movement.er_move_handback_m", m.er_move_handback_m, 0.05, 5.0)?;
        in_range_f32("movement.er_move_hold_s", m.er_move_hold_s, 0.0, 5.0)?;
        in_range_f32("movement.interact_hold_s", m.interact_hold_s, 0.0, 10.0)?;
        in_range_f32("movement.char_push_radius_m", m.char_push_radius_m, 0.0, 1.5)?;
        if !(1..=600).contains(&m.rearm_ticks) {
            return Err("movement.rearm_ticks must be 1..600".into());
        }
        in_range_f32("movement.publish_max_m", m.publish_max_m, 0.1, 5.0)?;
        if !matches!(m.strafe_sign, -1 | 1) {
            return Err("movement.strafe_sign must be 1 or -1".into());
        }
        let toggles = [dik_from_name(&k.toggle).unwrap_or(0), dik_from_name(&k.fp_toggle).unwrap_or(0)];
        for n in &m.blank_er {
            match dik_from_name(n) {
                None => return Err(format!("movement.blank_er: {n:?} is not a known key")),
                Some(crate::move_input::DIK_E) => return Err("movement.blank_er must not contain E (ER's interact key)".into()),
                Some(d) if toggles.contains(&d) => return Err(format!("movement.blank_er: {n:?} is the F5/F6 toggle key")),
                Some(_) => {}
            }
        }
        if !(10..=36_000).contains(&m.log_every_ticks) {
            return Err("movement.log_every_ticks must be 10..36000".into());
        }
        m.mantle.validate()?;
        m.rays.validate()?;
        m.unstick.validate()?;
        m.assist.validate()?;
        // M8 (docs/M8_DESIGN.md 10.2): E is ER's interact (ladders, levers, talks): never one of our keys
        for (what, name) in [("keys.reload", &k.reload), ("keys.toggle", &k.toggle), ("keys.fp_toggle", &k.fp_toggle), ("keys.ac130", &k.ac130), ("keys.ac130_exit", &k.ac130_exit)] {
            if dik_from_name(name) == Some(crate::move_input::DIK_E) {
                return Err(format!("{what} must not be E (ER interact: ladders, levers, talks)"));
            }
        }
        Ok(())
    }

    fn validate_viewmodel(&self) -> Result<(), String> {
        let v = &self.viewmodel;
        if v.assets_dir.trim().is_empty() {
            return Err("viewmodel.assets_dir must not be empty".into());
        }
        in_range_f32("viewmodel.paper_white_nits", v.paper_white_nits, 40.0, 1000.0)?;
        in_range_f32("viewmodel.hip_fov", v.hip_fov, 20.0, 120.0)?;
        in_range_f32("viewmodel.ads_fov", v.ads_fov, 10.0, 120.0)?;
        in_range_f32("viewmodel.near_in", v.near_in, 0.01, 10.0)?;
        in_range_f32("viewmodel.far_in", v.far_in, 10.0, 100_000.0)?;
        if v.far_in <= v.near_in * 10.0 {
            return Err("viewmodel.far_in must be more than 10 x near_in".into());
        }
        for (i, &o) in v.offset_in.iter().enumerate() {
            in_range_f32(&format!("viewmodel.offset_in[{i}]"), o, -50.0, 50.0)?;
        }
        for (i, &o) in v.light_dir_view.iter().enumerate() {
            in_range_f32(&format!("viewmodel.light_dir_view[{i}]"), o, -1.0, 1.0)?;
        }
        if v.light_dir_view.iter().map(|x| x * x).sum::<f32>() < 1e-4 {
            return Err("viewmodel.light_dir_view must not be zero".into());
        }
        in_range_f32("viewmodel.diffuse", v.diffuse, 0.0, 4.0)?;
        for (n, c) in [("ambient_sky", v.ambient_sky), ("ambient_ground", v.ambient_ground)] {
            for (i, &x) in c.iter().enumerate() {
                in_range_f32(&format!("viewmodel.{n}[{i}]"), x, 0.0, 4.0)?;
            }
        }
        in_range_f32("viewmodel.exposure", v.exposure, 0.01, 16.0)?;
        in_range_f32("viewmodel.crossfade_s", v.crossfade_s, 0.0, 1.0)?;
        in_range_f32("viewmodel.sprint_speed_mps", v.sprint_speed_mps, 0.0, 100.0)?;
        in_range("viewmodel.log_interval_s", v.log_interval_s, 1.0, 3600.0)?;
        Ok(())
    }

    fn validate_first_person(&self) -> Result<(), String> {
        let f = &self.first_person;
        in_range_f32("first_person.eye_height_m", f.eye_height_m, 1.2, 2.0)?;
        in_range_f32("first_person.eye_forward_m", f.eye_forward_m, 0.0, 0.5)?;
        in_range_f32("first_person.eye_smooth_s", f.eye_smooth_s, 0.0, 2.0)?;
        in_range_f32("first_person.pitch_clamp_deg", f.pitch_clamp_deg, 30.0, 89.9)?;
        if f.world_fov_deg != 0.0 {
            in_range_f32("first_person.world_fov_deg (0 = keep ER's)", f.world_fov_deg, 30.0, 120.0)?;
        }
        in_range_f32("first_person.fade_alpha_max", f.fade_alpha_max, 0.05, 0.99)?;
        in_range_f32("first_person.latch_holdoff_s", f.latch_holdoff_s, 0.0, 30.0)?;
        if f.latch_stale_s != 0.0 {
            in_range_f32("first_person.latch_stale_s (0 = never)", f.latch_stale_s, 2.0, 600.0)?;
        }
        if !(10..=600).contains(&f.yaw_probe_samples) {
            return Err("first_person.yaw_probe_samples must be 10..600".into());
        }
        if !(1..=600).contains(&f.arm_ticks) {
            return Err("first_person.arm_ticks must be 1..600".into());
        }
        if !(30..=36_000).contains(&f.log_every_ticks) {
            return Err("first_person.log_every_ticks must be 30..36000".into());
        }
        // C3
        in_range_f32("first_person.talk_stale_s", f.talk_stale_s, 0.5, 60.0)?;
        in_range_f32("first_person.fade_stale_s", f.fade_stale_s, 0.5, 60.0)?;
        if !(-1..=127).contains(&f.talk_menu_idx) {
            return Err("first_person.talk_menu_idx must be -1 (off) or 0..127".into());
        }
        // 2026-10-21: 0 = off; a grace takes 3.6 s from the press to the talk id
        in_range_f32("first_person.talk_interact_window_s", f.talk_interact_window_s, 0.0, 60.0)?;
        // C6 scope peek
        if !(1..=30).contains(&f.scope_peek_arm_ticks) {
            return Err("first_person.scope_peek_arm_ticks must be 1..30".into());
        }
        Ok(())
    }

    /// C3 (docs/C3_DESIGN.md 2.3 / 2.4 / 2.6): `[ui_yield]`, `[ui]` and the damage numbers of `[hud]`.
    fn validate_c3_ui(&self) -> Result<(), String> {
        let y = &self.ui_yield;
        if !matches!(y.mode.as_str(), "lower" | "hide" | "cutout" | "off") {
            return Err("ui_yield.mode must be lower | hide | cutout | off".into());
        }
        in_range_f32("ui_yield.cutout_feather", y.cutout_feather, 0.0, 0.2)?;
        for (name, rects) in [
            ("cutout_popup", &y.cutout_popup),
            ("cutout_announce", &y.cutout_announce),
            ("cutout_status", &y.cutout_status),
            ("cutout_summon", &y.cutout_summon),
            ("cutout_blink", &y.cutout_blink),
            ("cutout_idx", &y.cutout_idx),
        ] {
            if rects.len() > crate::ui_yield::MAX_CUT_RECTS {
                return Err(format!("ui_yield.{name} must list at most {} regions", crate::ui_yield::MAX_CUT_RECTS));
            }
            for (i, r) in rects.iter().enumerate() {
                for (k, &v) in r.iter().enumerate() {
                    in_range_f32(&format!("ui_yield.{name}[{i}][{k}]"), v, 0.0, 1.0)?;
                }
                if r[2] < r[0] + 0.01 || r[3] < r[1] + 0.01 {
                    return Err(format!("ui_yield.{name}[{i}] must be [x0, y0, x1, y1] with x1 >= x0 + 0.01 and y1 >= y0 + 0.01"));
                }
            }
        }
        for (i, &d) in y.drop_in.iter().enumerate() {
            in_range_f32(&format!("ui_yield.drop_in[{i}]"), d, -30.0, 30.0)?;
        }
        in_range_f32("ui_yield.hud_dim_alpha", y.hud_dim_alpha, 0.0, 1.0)?;
        if y.indices.len() > 32 || y.indices.iter().any(|&i| i >= 0x46) {
            return Err("ui_yield.indices must list at most 32 ui_states indices, each < 70".into());
        }
        for (i, &a) in y.toast_anchor.iter().enumerate() {
            in_range_f32(&format!("ui_yield.toast_anchor[{i}]"), a, 0.0, 1.0)?;
        }
        in_range_f32("ui_yield.stuck_s", y.stuck_s, 2.0, 600.0)?;
        let h = &self.hud;
        in_range_f32("hud.damage_numbers_s", h.damage_numbers_s, 0.2, 5.0)?;
        in_range_f32("hud.damage_numbers_merge_s", h.damage_numbers_merge_s, 0.0, 2.0)?;
        if !matches!(h.damage_numbers_value.as_str(), "er" | "mw2") {
            return Err("hud.damage_numbers_value must be er | mw2".into());
        }
        if !(1..=64).contains(&h.damage_numbers_max) {
            return Err("hud.damage_numbers_max must be 1..64".into());
        }
        in_range_f32("hud.damage_numbers_scale", h.damage_numbers_scale, 0.5, 4.0)?;
        Ok(())
    }

    /// C3 PASS B (docs/C3_DESIGN.md 4.6): `[gamepad]` values and its bind tables.
    fn validate_gamepad(&self) -> Result<(), String> {
        let g = &self.gamepad;
        if !(-1..=3).contains(&g.pad_index) {
            return Err("gamepad.pad_index must be -1 (first with input) or 0..3".into());
        }
        if !matches!(g.look_route.as_str(), "mouse" | "er") {
            return Err("gamepad.look_route must be mouse | er".into());
        }
        in_range_f32("gamepad.look_sensitivity", g.look_sensitivity, 0.05, 10.0)?;
        if g.input_graph > 3 {
            return Err("gamepad.input_graph must be 0..3 (MW2 view_input_N.graph)".into());
        }
        for (n, v) in [
            ("turnrate_yaw", g.turnrate_yaw),
            ("turnrate_pitch", g.turnrate_pitch),
            ("turnrate_yaw_ads", g.turnrate_yaw_ads),
            ("turnrate_pitch_ads", g.turnrate_pitch_ads),
        ] {
            in_range_f32(&format!("gamepad.{n}"), v, 1.0, 2000.0)?;
        }
        in_range_f32("gamepad.accel_lerp", g.accel_lerp, 0.0, 20000.0)?;
        in_range_f32("gamepad.accel_max_yaw", g.accel_max_yaw, 0.0, 2000.0)?;
        for (n, dz, outer) in [("rs", g.rs_deadzone, g.rs_outer), ("ls", g.ls_deadzone, g.ls_outer)] {
            in_range_f32(&format!("gamepad.{n}_deadzone"), dz, 0.0, 0.9)?;
            in_range_f32(&format!("gamepad.{n}_outer"), outer, 0.1, 1.0)?;
            if outer <= dz + 0.05 {
                return Err(format!("gamepad.{n}_outer must be above {n}_deadzone + 0.05"));
            }
        }
        if g.trigger_threshold == 0 || g.trigger_threshold == 255 {
            return Err("gamepad.trigger_threshold must be 1..254".into());
        }
        if !(50..=2000).contains(&g.hold_ms) {
            return Err("gamepad.hold_ms must be 50..2000".into());
        }
        if g.prompt_indices.len() > 8 || g.prompt_indices.iter().any(|&i| i >= 128) {
            return Err("gamepad.prompt_indices: at most 8 indices, each < 128".into());
        }
        if g.mouse_deg_per_count != 0.0 {
            in_range_f32("gamepad.mouse_deg_per_count", g.mouse_deg_per_count, 0.0001, 10.0)?;
        }
        in_range_f32("gamepad.gunship_counts_per_deg", g.gunship_counts_per_deg, 0.0, 10000.0)?;
        if g.rumble {
            return Err("gamepad.rumble is not implemented (no XInputSetState hook); keep it false".into());
        }
        if !(1..=3600).contains(&g.log_every_s) {
            return Err("gamepad.log_every_s must be 1..3600".into());
        }
        g.binds().map(|_| ())
    }

    /// 2026-10-21: the heal key and ER's use-item key. The heal key is never E (ER interact) nor one of the keys MW2
    /// mode reads outside the gunship (it may share a key with the gunship-only AC-130 keys and with movement.blank_er);
    /// ER's use-item key is a known key, not E, not the heal key (the reload key R is expected: it is ER's default).
    fn validate_heal(&self) -> Result<(), String> {
        let k = &self.keys;
        let Some(u) = dik_from_name(&k.er_use_item) else {
            return Err(format!("keys.er_use_item = {:?} is not a known key", k.er_use_item));
        };
        if u == crate::move_input::DIK_E {
            return Err("keys.er_use_item must not be E (ER interact)".into());
        }
        if k.heal.is_empty() {
            return Ok(());
        }
        let Some(d) = dik_from_name(&k.heal) else {
            return Err(format!("keys.heal = {:?} is not a known key", k.heal));
        };
        if d == crate::move_input::DIK_E {
            return Err("keys.heal must not be E (ER interact)".into());
        }
        if d == u {
            return Err("keys.heal must differ from keys.er_use_item".into());
        }
        let others = [
            ("keys.toggle", &k.toggle),
            ("keys.reload", &k.reload),
            ("keys.fp_toggle", &k.fp_toggle),
            ("keys.ac130", &k.ac130),
            ("keys.move_forward", &k.move_forward),
            ("keys.move_back", &k.move_back),
            ("keys.move_left", &k.move_left),
            ("keys.move_right", &k.move_right),
            ("keys.move_sprint", &k.move_sprint),
            ("keys.move_jump", &k.move_jump),
            ("keys.move_crouch", &k.move_crouch),
            ("keys.move_prone", &k.move_prone),
            ("keys.move_stance_hold", &k.move_stance_hold),
            ("keys.weapon_next", &k.weapon_next),
            ("keys.weapon_prev", &k.weapon_prev),
            ("keys.nuke", &k.nuke),
        ];
        for (o, v) in others {
            if !v.is_empty() && dik_from_name(v) == Some(d) {
                return Err(format!("keys.heal = {:?} conflicts with {o}", k.heal));
            }
        }
        Ok(())
    }

    /// Names of the top-level sections that differ (for the `CONFIG reloaded (changed: ...)` line).
    pub fn changed_sections(&self, new: &Config) -> Vec<&'static str> {
        let mut v = Vec::new();
        macro_rules! cmp {
            ($($f:ident),*) => { $( if self.$f != new.$f { v.push(stringify!($f)); } )* };
        }
        cmp!(schema, paths, log, keys, gameplay, aim, damage, regulation, hit, menu, hud, audio, viewmodel, first_person, ac130, movement, arsenal, launchers, kick, spawn, nuke, ui_yield, ui, gamepad);
        v
    }

    /// Applies a hot reload: hot sections come from `new`; `paths`, `damage` and `regulation`
    /// stay as loaded at boot (they are baked or structural). Returns the names of cold sections
    /// whose new value was ignored.
    pub fn merge_hot(&self, new: &Config, weapon_change_allowed: bool) -> (Config, Vec<&'static str>) {
        let mut merged = new.clone();
        let mut ignored = Vec::new();
        if new.paths != self.paths {
            merged.paths = self.paths.clone();
            ignored.push("paths");
        }
        if new.damage != self.damage {
            merged.damage = self.damage.clone();
            ignored.push("damage.k");
        }
        if new.regulation != self.regulation {
            merged.regulation = self.regulation.clone();
            ignored.push("regulation");
        }
        if new.viewmodel.assets_dir != self.viewmodel.assets_dir || new.viewmodel.enabled != self.viewmodel.enabled {
            merged.viewmodel.assets_dir = self.viewmodel.assets_dir.clone();
            merged.viewmodel.enabled = self.viewmodel.enabled;
            ignored.push("viewmodel.enabled/assets_dir");
        }
        if new.first_person.camera_mode != self.first_person.camera_mode {
            merged.first_person.camera_mode = self.first_person.camera_mode;
            ignored.push("first_person.camera_mode");
        }
        if new.movement.data != self.movement.data {
            merged.movement.data = self.movement.data.clone();
            ignored.push("movement.data");
        }
        if new.nuke.assets_dir != self.nuke.assets_dir {
            merged.nuke.assets_dir = self.nuke.assets_dir.clone();
            ignored.push("nuke.assets_dir");
        }
        if new.ac130.assets_dir != self.ac130.assets_dir {
            merged.ac130.assets_dir = self.ac130.assets_dir.clone();
            ignored.push("ac130.assets_dir");
        }
        if !weapon_change_allowed && new.gameplay.weapon != self.gameplay.weapon {
            merged.gameplay.weapon = self.gameplay.weapon.clone();
            ignored.push("gameplay.weapon");
        }
        // M6: the gun list and the folder are loaded at boot
        if new.arsenal.dir != self.arsenal.dir || new.arsenal.guns != self.arsenal.guns || new.arsenal.preload != self.arsenal.preload {
            merged.arsenal.dir = self.arsenal.dir.clone();
            merged.arsenal.guns = self.arsenal.guns.clone();
            merged.arsenal.preload = self.arsenal.preload.clone();
            ignored.push("arsenal.dir/guns/preload");
        }
        // C3: the Scaleform probe is installed at boot only
        if new.ui.probe_scaleform != self.ui.probe_scaleform {
            merged.ui.probe_scaleform = self.ui.probe_scaleform;
            ignored.push("ui.probe_scaleform");
        }
        // C4.1 P2: its objects are created once at boot (cold both ways, like the probe)
        if new.ui.probe_layer_sample != self.ui.probe_layer_sample {
            merged.ui.probe_layer_sample = self.ui.probe_layer_sample;
            ignored.push("ui.probe_layer_sample");
        }
        // C4 (docs/C4_UI_DESIGN.md 3.1): cold to turn on (the hooks and objects are boot work), hot to turn off
        if new.ui.inject_viewmodel && !self.ui.inject_viewmodel {
            merged.ui.inject_viewmodel = false;
            ignored.push("ui.inject_viewmodel (turning it on)");
        }
        if new.ui.inject_ui_layer && !self.ui.inject_ui_layer {
            merged.ui.inject_ui_layer = false;
            ignored.push("ui.inject_ui_layer (turning it on)");
        }
        // C4.1 (docs/C4_1_DESIGN.md 3.1): cold to turn on; turning off follows the master `inject_viewmodel` (hot)
        if new.ui.inject_ui_thread && !self.ui.inject_ui_thread {
            merged.ui.inject_ui_thread = false;
            ignored.push("ui.inject_ui_thread (turning it on)");
        }
        // C6 depth probe: boot work (cold both ways, like the other probes)
        if new.coop.probe_er_depth != self.coop.probe_er_depth {
            merged.coop.probe_er_depth = self.coop.probe_er_depth;
            ignored.push("coop.probe_er_depth");
        }
        // C6 depth 5: the barrier hook and the sample's objects are boot work (cold both ways); the ER-depth prime is cold
        // to turn on (its objects and the probe verdict), hot to turn off
        if new.coop.probe_er_depth_state != self.coop.probe_er_depth_state {
            merged.coop.probe_er_depth_state = self.coop.probe_er_depth_state;
            ignored.push("coop.probe_er_depth_state");
        }
        if new.coop.probe_er_depth_sample != self.coop.probe_er_depth_sample {
            merged.coop.probe_er_depth_sample = self.coop.probe_er_depth_sample;
            ignored.push("coop.probe_er_depth_sample");
        }
        if new.coop.world_gun_er_depth && !self.coop.world_gun_er_depth {
            merged.coop.world_gun_er_depth = false;
            ignored.push("coop.world_gun_er_depth (turning it on)");
        }
        (merged, ignored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_when_keys_missing() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let c = Config::parse("[damage]\nk = 3.0\n[gameplay]\ncamera_source = \"chr_cam\"\n").unwrap();
        assert_eq!(c.damage.k, 3.0);
        assert_eq!(c.gameplay.camera_source, CameraSource::ChrCam);
        let c = Config::parse("[gameplay]\ncamera_source = \"pers_cam_1\"\n").unwrap();
        assert_eq!(c.gameplay.camera_source, CameraSource::PersCam1);
        assert_eq!(c.keys.toggle, "F5");
        assert_eq!(c.regulation.weapons["ump45_mp"], 1);
        Config::default().validate().unwrap();
    }

    #[test]
    fn config_keys_m6_conflicts() {
        let c = Config::default();
        assert_eq!((c.keys.weapon_next.as_str(), c.keys.weapon_prev.as_str()), ("1", "2"));
        assert_eq!(c.keys.ac130_gun1, "1", "shared with the AC-130 gun keys (gunship only)");
        assert!(Config::parse("[keys]\nweapon_next = \"R\"\n").is_err(), "the reload key");
        assert!(Config::parse("[keys]\nweapon_next = \"E\"\n").is_err(), "ER interact");
        assert!(Config::parse("[keys]\nweapon_prev = \"W\"\n").is_err(), "a movement key");
        assert!(Config::parse("[keys]\nweapon_prev = \"F7\"\n").is_err(), "the AC-130 call");
        assert!(Config::parse("[keys]\nweapon_prev = \"1\"\n").is_err(), "next == prev");
        // 2026-10-21: Q is the heal key by default now; with heal moved, Q / 3 are still allowed (ac130 keys only)
        assert!(Config::parse("[keys]\nweapon_next = \"Q\"\nweapon_prev = \"3\"\n").is_err(), "keys.heal = Q");
        assert!(Config::parse("[keys]\nweapon_next = \"Q\"\nweapon_prev = \"3\"\nheal = \"G\"\n").is_ok(), "Q / 3 allowed (ac130 keys only)");
        assert!(Config::parse("[keys]\nweapon_next = \"\"\n").is_ok(), "unbound");
    }

    #[test]
    fn config_arsenal_sections_and_alias() {
        let c = Config::default();
        assert_eq!(c.start_gun(), "ump45_mp", "gameplay.weapon is the old alias of arsenal.start");
        assert_eq!((c.damage.mode.as_str(), c.damage.s, c.damage.d_ref), ("flat_inv", 1.225, 85.0));
        assert_eq!(c.regulation.arsenal.id_base, 79_100_000);
        let c = Config::parse("[arsenal]\nstart = \"m4_mp\"\nguns = [\"m4_mp\", \"ump45_mp\"]\n").unwrap();
        assert_eq!(c.start_gun(), "m4_mp");
        assert!(Config::parse("[gameplay]\nweapon = \"fal_mp\"\n[arsenal]\nstart = \"m4_mp\"\n").is_err(), "both set");
        assert!(Config::parse("[arsenal]\nguns = [\"m4_mp\", \"m4_mp\"]\n").is_err(), "duplicate");
        assert!(Config::parse("[arsenal]\nguns = [\"m4_mp\"]\n").is_err(), "start gun not listed");
        assert!(Config::parse("[arsenal]\npellet_bands = \"x\"\n").is_err());
        assert!(Config::parse("[launchers]\nflight = \"x\"\n").is_err());
        assert!(Config::parse("[kick]\nscale = inf\n").is_err());
        assert!(Config::parse("[arsenal]\nunknown_key = 1\n").is_err(), "unknown key");
        assert!(Config::parse("[movement]\nweapon_move_class = \"auto\"\n").is_ok());
        let (m, ign) = Config::default().merge_hot(&Config::parse("[arsenal]\nguns = [\"ump45_mp\"]\n").unwrap(), true);
        assert!(m.arsenal.guns.is_empty() && ign.contains(&"arsenal.dir/guns/preload"), "the gun list is not hot");
    }

    #[test]
    fn config_arsenal_review_m6_limits() {
        // more than 64 guns would overflow the u64 enable / failure masks
        let names: Vec<String> = (0..65).map(|i| format!("\"g{i}_mp\"")).collect();
        let toml = format!("[arsenal]\nstart = \"g0_mp\"\nguns = [{}]\n", names.join(", "));
        assert!(Config::parse(&toml).unwrap_err().msg.contains("at most 64"));
        let names: Vec<String> = (0..64).map(|i| format!("\"g{i}_mp\"")).collect();
        assert!(Config::parse(&format!("[arsenal]\nstart = \"g0_mp\"\nguns = [{}]\n", names.join(", "))).is_ok());
        // the launcher self guard and the Javelin boost
        let c = Config::default();
        assert_eq!((c.launchers.self_guard, c.launchers.self_guard_margin_m), (true, 0.5));
        assert_eq!((c.launchers.javelin_cruise_mps, c.launchers.javelin_accel_mps2), (60.0, 40.0));
        assert!(Config::parse("[launchers]\nself_guard_margin_m = -1.0\n").is_err());
        assert!(Config::parse("[launchers]\nself_guard = false\njavelin_cruise_mps = 0.0\n").is_ok());
    }

    #[test]
    fn config_bad_key_name_is_error() {
        let e = Config::parse("[keys]\ntoggle = \"F99\"\n").unwrap_err();
        assert!(e.msg.contains("keys.toggle"), "{e}");
        let e = Config::parse("[keys]\ntoggle = \"R\"\n").unwrap_err();
        assert!(e.msg.contains("differ"), "{e}");
    }

    #[test]
    fn config_unknown_field_reports_position() {
        let e = Config::parse("schema = 1\n\n[hud]\nenabeld = true\n").unwrap_err();
        assert_eq!(e.line, 4, "{e}");
        let e = Config::parse("[audio]\nmaster_gain = \"loud\"\n").unwrap_err();
        assert_eq!(e.line, 2, "{e}");
    }

    #[test]
    fn config_rejects_non_finite_and_huge_floats() {
        for bad in [
            "[aim]\nmax_range_m = inf\n",
            "[aim]\nmax_range_m = nan\n",
            "[aim]\nmax_range_m = 5000.0\n",
            "[aim]\norigin_ahead_m = inf\n",
            "[aim]\norigin_ahead_m = 6.0\n",
            "[aim]\nchest_height_m = inf\n",
            "[aim]\nchest_height_m = -0.5\n",
            "[aim]\nchar_radius_m = inf\n",
            "[hit]\nwindow_s = inf\n",
            "[log]\nstate_interval_s = inf\n",
            "[hud]\nhitmarker_s = inf\n",
            "[hud]\nmin_gap_px = inf\n",
            "[hud]\ncrosshair_line_scale = nan\n",
            "[hud]\ncrosshair_color = [1.0, 1.0, inf, 1.0]\n",
            "[audio]\nmaster_gain = inf\n",
            "[audio]\nfire_gain = -1.0\n",
            "[damage]\nk = inf\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        for bad in [
            "[viewmodel]\nhip_fov = inf\n",
            "[viewmodel]\nnear_in = nan\n",
            "[viewmodel]\nexposure = 0.0\n",
            "[viewmodel]\nlight_dir_view = [0.0, 0.0, 0.0]\n",
            "[viewmodel]\noutput = \"hdr10\"\n",
            "[viewmodel]\nfar_in = 0.5\n",
            "[hit]\nimpact_radius_m = inf\n",
            "[keys]\nads_mouse_button = 0\n",
            "[keys]\nads_mouse_button = 9\n",
            "[aim]\ncamera_det_sign = 2\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        let ok = Config::parse("[aim]\nmax_range_m = 1000.0\norigin_ahead_m = 5.0\nchest_height_m = 3.0\n").unwrap();
        assert_eq!(ok.aim.max_range_m, AIM_MAX_RANGE_M);
        assert!(Config::default().menu.cursor_gate_at_enable);
    }

    #[test]
    fn config_hot_merge_keeps_cold_sections() {
        let old = Config::default();
        let mut new = Config::default();
        new.damage.k = 3.0;
        new.hud.min_gap_px = 9.0;
        new.gameplay.weapon = "masada_mp".into();
        assert_eq!(old.changed_sections(&new), vec!["gameplay", "damage", "hud"]);
        let (m, ignored) = old.merge_hot(&new, false);
        assert_eq!(m.damage.k, 4.0);
        assert_eq!(m.hud.min_gap_px, 9.0);
        assert_eq!(m.gameplay.weapon, "ump45_mp");
        assert_eq!(ignored.len(), 2);
        let (m, _) = old.merge_hot(&new, true);
        assert_eq!(m.gameplay.weapon, "masada_mp");
    }
}

#[cfg(test)]
mod viewmodel_tests {
    use super::*;

    #[test]
    fn config_viewmodel_section_defaults_and_bounds() {
        let c = Config::parse("[viewmodel]\noutput = \"srgb\"\nhip_fov = 70.0\n[keys]\nads_mouse_button = 255\n").unwrap();
        assert_eq!(c.viewmodel.output, VmOutput::Srgb);
        assert_eq!(c.viewmodel.hip_fov, 70.0);
        assert_eq!(c.viewmodel.ads_fov, 55.0);
        assert_eq!(c.keys.ads_mouse_button, 255);
        let d = Config::default();
        assert_eq!(d.viewmodel.output, VmOutput::Auto);
        assert_eq!(d.viewmodel.assets_dir, "converted\\viewmodel");
        assert_eq!(d.hit.impact_window_s, 0.25);
        assert_eq!(d.hit.impact_lag_s, 0.1);
        assert_eq!(d.keys.ads_mouse_button, 1);
        // enabled / assets_dir are cold, the rest is hot
        let mut n = d.clone();
        n.viewmodel.exposure = 1.5;
        n.viewmodel.assets_dir = "elsewhere".into();
        let (m, ignored) = d.merge_hot(&n, false);
        assert_eq!(m.viewmodel.exposure, 1.5);
        assert_eq!(m.viewmodel.assets_dir, d.viewmodel.assets_dir);
        assert!(ignored.contains(&"viewmodel.enabled/assets_dir"));
    }
}

#[cfg(test)]
mod first_person_tests {
    use super::*;

    #[test]
    fn config_first_person_defaults_and_bounds() {
        let d = Config::default();
        let f = &d.first_person;
        assert!(f.enabled && f.yaw_follow && f.use_fade_signal && f.use_cam_mask_signal && f.use_talk_signal);
        assert!(f.use_ride_state_signal);
        assert!(f.sync_chr_cam && f.disable_wall_recovery && f.disable_auto_rotation);
        let c = Config::parse("[first_person]
sync_chr_cam = false
disable_wall_recovery = false
disable_auto_rotation = false
").unwrap();
        assert!(!c.first_person.sync_chr_cam && !c.first_person.disable_wall_recovery && !c.first_person.disable_auto_rotation);
        assert!(Config::parse("[first_person]
sync_chr_cam = 1
").is_err());
        assert_eq!((f.latch_holdoff_s, f.latch_stale_s), (2.0, 20.0));
        let c = Config::parse("[first_person]\nlatch_stale_s = 0.0\nuse_ride_state_signal = false\n").unwrap();
        assert_eq!((c.first_person.latch_stale_s, c.first_person.use_ride_state_signal), (0.0, false));
        assert_eq!(f.camera_mode, CameraMode::Hook);
        assert_eq!(f.hide_method, HideMethod::RenderFlag);
        assert_eq!((f.eye_height_m, f.eye_forward_m, f.pitch_clamp_deg, f.world_fov_deg), (1.60, 0.10, 85.0, 0.0));
        assert_eq!((f.arm_ticks, f.yaw_probe_samples, f.log_every_ticks), (30, 60, 300));
        assert_eq!(d.keys.fp_toggle, "F6");
        let c = Config::parse(
            // M4: F7 is now keys.ac130 (which must differ from fp_toggle), so this test uses F8
            "[first_person]\ncamera_mode = \"hook\"\nhide_method = \"fade\"\nworld_fov_deg = 65.0\n[keys]\nfp_toggle = \"F8\"\n",
        )
        .unwrap();
        assert_eq!(c.first_person.camera_mode, CameraMode::Hook);
        assert_eq!(c.first_person.hide_method, HideMethod::Fade);
        assert_eq!(c.first_person.world_fov_deg, 65.0);
        assert_eq!(Config::parse("[first_person]\nhide_method = \"none\"\n").unwrap().first_person.hide_method, HideMethod::None);
        for bad in [
            "[first_person]\neye_height_m = inf\n",
            "[first_person]\neye_height_m = nan\n",
            "[first_person]\neye_height_m = 3.0\n",
            "[first_person]\neye_forward_m = -0.1\n",
            "[first_person]\npitch_clamp_deg = 90.0\n",
            "[first_person]\nworld_fov_deg = 10.0\n",
            "[first_person]\nworld_fov_deg = nan\n",
            "[first_person]\nworld_fov_deg = inf\n",
            "[first_person]\nfade_alpha_max = 1.0\n",
            "[first_person]\narm_ticks = 0\n",
            "[first_person]\nyaw_probe_samples = 5\n",
            "[first_person]\ncamera_mode = \"chr_cam\"\n",
            "[first_person]\nhide_method = \"speffect\"\n",
            "[first_person]\nunknown = 1\n",
            "[first_person]\nlatch_holdoff_s = -1.0\n",
            "[first_person]\nlatch_holdoff_s = nan\n",
            "[first_person]\nlatch_stale_s = 1.0\n",
            "[first_person]\nlatch_stale_s = inf\n",
            "[keys]\nfp_toggle = \"F5\"\n",
            "[keys]\nfp_toggle = \"W\"\n",
            "[keys]\nfp_toggle = \"F13\"\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        // camera_mode is cold (the hook installs at boot); everything else is hot
        let mut n = d.clone();
        n.first_person.camera_mode = CameraMode::Task;
        n.first_person.eye_height_m = 1.7;
        let (m, ignored) = d.merge_hot(&n, false);
        assert_eq!(m.first_person.camera_mode, CameraMode::Hook);
        assert_eq!(m.first_person.eye_height_m, 1.7);
        assert!(ignored.contains(&"first_person.camera_mode"));
        assert_eq!(d.changed_sections(&n), vec!["first_person"]);
    }
}

#[cfg(test)]
mod ac130_tests {
    use super::*;

    #[test]
    fn config_ac130_defaults_bounds_and_cold_keys() {
        let d = Config::default();
        let a = &d.ac130;
        assert!(a.enabled && a.cooldown_enabled && a.thermal && a.grain && a.hide_er_hud && a.require_open_sky);
        assert_eq!((a.cooldown_s, a.duration_s, a.scale), (120.0, 0.0, 1.0));
        assert_eq!((a.fov_105, a.fov_40, a.fov_25), (0.0, 0.0, 0.0), "0 = the MW2 data");
        // the converted data flows through unless a gun is overridden (review M4 minor 9)
        assert_eq!(a.resolve_fovs([45.0, 20.0, 5.0]), [45.0, 20.0, 5.0]);
        assert_eq!(a.resolve_fovs([44.0, 21.0, 6.0]), [44.0, 21.0, 6.0], "a change in the data reaches the game");
        let o = Config::parse("[ac130]\nfov_40 = 30.0\n").unwrap();
        assert_eq!(o.ac130.resolve_fovs([45.0, 20.0, 5.0]), [45.0, 30.0, 5.0]);
        assert_eq!(d.regulation.ac130.protect_endurance_s, 90.0);
        // after run 9: impact visuals from vanilla player Bullet rows (== build_regulation.py AC130_SFX_DEFAULT)
        let r = &d.regulation.ac130;
        assert_eq!((r.sfx_105, r.sfx_105_ring, r.sfx_40, r.sfx_40_ring, r.sfx_25), (524_028, 530_123, 300_602, 530_133, 530_133));
        // [ac130.shake]: on by default, MW2 numbers at scale 1, the old shake_scale still multiplies, off = 0
        let sh = a.shake.params(a.shake_scale);
        assert_eq!((sh.scale, sh.rot_deg, sh.mw2_falloff), (1.0, 2.0, false));
        let off = Config::parse("[ac130.shake]\nenabled = false\n").unwrap();
        assert_eq!(off.ac130.shake.params(off.ac130.shake_scale).scale, 0.0);
        let leg = Config::parse("[ac130]\nshake_scale = 0.5\n[ac130.shake]\nscale = 2.0\n").unwrap();
        assert_eq!(leg.ac130.shake.params(leg.ac130.shake_scale).scale, 1.0);
        // [ac130.tracers] after run 9: the 105 has the biggest, longest impact flash and a bloom
        let ts = a.tracers.params().styles;
        assert!(ts[0].flash_m > ts[1].flash_m && ts[0].flash_s > ts[1].flash_s && ts[0].bloom_m > ts[1].bloom_m && ts[0].flash_gain > 1.0);
        assert_eq!((a.exposure, a.gamma), (0.52, 0.75), "thermal pre-exposure + mid-tone lift [I] (run 8: too dark at 0.6)");
        assert_eq!(d.regulation.ac130.k, 0.65, "run 8: flat damage rows");
        assert_eq!((d.regulation.ac130.scale_105, d.regulation.ac130.scale_40, d.regulation.ac130.scale_25), (1.0, 1.0, 0.7));
        // [ac130.tracers]: defaults, the table form, and the old `tracers = bool` flag
        let t = &a.tracers;
        assert!(t.enabled && t.flash);
        let p = t.params();
        assert!(p.styles[0].width_m > p.styles[1].width_m && p.styles[1].width_m > p.styles[2].width_m);
        assert!(!Config::parse("[ac130]\ntracers = false\n").unwrap().ac130.tracers.enabled, "old bool form");
        let tt = Config::parse("[ac130.tracers]\nenabled = true\nwidth_25 = 0.5\nflash = false\n").unwrap();
        assert_eq!((tt.ac130.tracers.width_25, tt.ac130.tracers.flash, tt.ac130.tracers.length_105), (0.5, false, 14.0));
        assert_eq!((a.base_view, a.vision, a.explosion_tiers, a.orbit_dir), (BaseViewCfg::Center, VisionMode::Sp, 2, 1));
        assert!(!a.sync_chr_cam && !a.debug_markers && a.fov_is_vertical);
        assert_eq!(a.assets_dir, "converted\\ac130");
        assert_eq!((d.keys.ac130.as_str(), d.keys.ac130_exit.as_str(), d.keys.ac130_next.as_str()), ("F7", "Back", "Q"));
        assert_eq!(d.keys.ac130_thermal, "F");
        assert_eq!(d.regulation.ac130.protect_speffect_id, 79_000_950);
        assert_eq!(d.regulation.ac130.clone_bullet_from, 10_030_001);
        assert_eq!(VisionMode::Mp.names(), ("thermal_mp", "missilecam"));
        let c = Config::parse(
            "[ac130]\ncooldown_enabled = false\nvision = \"mp\"\nbase_view = \"mw2_tag\"\nscale = 0.5\n[ac130.audio]\ngain = 0.5\n[regulation.ac130]\nk = 3.0\n",
        )
        .unwrap();
        assert!(!c.ac130.cooldown_enabled);
        assert_eq!((c.ac130.vision, c.ac130.base_view, c.ac130.scale, c.ac130.audio.gain), (VisionMode::Mp, BaseViewCfg::Mw2Tag, 0.5, 0.5));
        assert_eq!(c.regulation.ac130.k, 3.0);
        for bad in [
            "[ac130]\nunknown_key = 1\n",
            "[ac130.audio]\nloud = true\n",
            "[regulation.ac130]\nsfx_99 = 1\n",
            "[ac130]\nscale = 3.0\n",
            "[ac130]\nscale = nan\n",
            "[ac130]\ncooldown_s = inf\n",
            "[ac130]\nduration_s = 2.0\n",
            "[ac130]\nradius_m = 5.0\n",
            "[ac130]\norbit_dir = 0\n",
            "[ac130]\nexplosion_tiers = 3\n",
            "[ac130]\nfov_25 = 0.5\n",
            "[ac130]\nexposure = 0.0\n",
            "[ac130]\ngamma = 0.1\n",
            "[ac130.tracers]\nwidth_105 = -1.0\n",
            "[ac130.tracers]\nbogus = 1\n",
            "[ac130.tracers]\ntail = 2.0\n",
            "[ac130.tracers]\nbloom_105 = -1.0\n",
            "[ac130.tracers]\nflash_gain_105 = 9.0\n",
            "[ac130.shake]\nscale = 6.0\n",
            "[ac130.shake]\nfreq_hz = 0.0\n",
            "[ac130.shake]\nwobble = 1\n",
            "[regulation.ac130]\nsfx_105_dust = 1\n",
            "[ac130]\ntracers = 3\n",
            "[regulation.ac130]\nprotect_endurance_s = -1.0\n",
            "[regulation.ac130]\nprotect_endurance_s = 5.0\n",
            "[regulation.ac130]\nscale_25 = 0.0\n",
            "[ac130]\nvision = \"night\"\n",
            "[ac130]\nluma_weights = [0.0, 0.0, 0.0]\n",
            "[ac130]\nmax_ray_m = inf\n",
            "[ac130.audio]\nexplosion_floor_gain = 2.0\n",
            "[keys]\nac130 = \"F5\"\n",
            "[keys]\nac130 = \"F6\"\n",
            "[keys]\nac130 = \"W\"\n",
            "[keys]\nac130_exit = \"Escape\"\n",
            "[regulation.ac130]\nprotect_speffect_id = 1540\n",
            "[regulation.ac130]\nk = 0.0\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        // assets_dir is cold, the rest of [ac130] is hot
        let mut n = d.clone();
        n.ac130.assets_dir = "elsewhere".into();
        n.ac130.cooldown_s = 30.0;
        let (m, ignored) = d.merge_hot(&n, false);
        assert_eq!(m.ac130.assets_dir, d.ac130.assets_dir);
        assert_eq!(m.ac130.cooldown_s, 30.0);
        assert!(ignored.contains(&"ac130.assets_dir"));
        assert_eq!(d.changed_sections(&n), vec!["ac130"]);
    }
}

#[cfg(test)]
mod movement_tests {
    use super::*;

    #[test]
    fn config_movement_defaults_match_design() {
        let d = Config::default();
        let m = &d.movement;
        assert!(m.enabled && !m.fp_only && m.jump_slowdown && !m.marathon && !m.lightweight && m.er_pushout);
        assert_eq!(
            (m.tp_anim, m.sprint_mode, m.crouch_mode, m.gostand_from_prone),
            (TpAnim::Hybrid, SprintModeCfg::Press, CrouchMode::Toggle, GostandFromProne::Stand)
        );
        assert_eq!(
            (m.weapon_move_class, m.stance_eye, m.fall_damage, m.lethal_fall, m.ground_normal),
            (WeaponMoveClass::Smg, StanceEye::Ratio, FallDamageCfg::Mw2, LethalFall::Handback, GroundNormalCfg::Rays)
        );
        assert_eq!((m.speed_scale, m.fall_damage_scale, m.freefall_handback_m, m.hull_radius_m, m.skin_m), (1.0, 1.0, 30.0, 0.0, 0.03));
        // run 18: 48 rays (the waist probe and the body column)
        assert_eq!((m.max_substep_s, m.max_bumps, m.max_rays_per_tick, m.walkable_normal_y), (0.0167, 3, 48, 0.7));
        assert_eq!((m.er_pushout_max_m, m.er_move_handback_m, m.er_move_hold_s, m.interact_hold_s, m.char_push_radius_m), (0.30, 0.30, 0.5, 1.5, 0.45));
        assert_eq!((m.rearm_ticks, m.publish_max_m, m.strafe_sign, m.log_every_ticks), (10, 1.0, 1, 60));
        assert!(!m.proxy_sync_flag && !m.write_last_update && !m.use_movement_limit_signal);
        assert!(m.view_bob && m.land_dip && m.vm_stance_pose && m.keyprobe);
        assert_eq!(m.blank_er, ["SPACE", "LSHIFT", "C", "LCTRL", "F", "X", "Q", "LALT"]);
        assert_eq!(m.data, r"converted\movement.json");
        let k = &d.keys;
        assert_eq!(
            [&k.move_forward, &k.move_back, &k.move_left, &k.move_right, &k.move_sprint, &k.move_jump, &k.move_crouch, &k.move_prone],
            ["W", "S", "A", "D", "LSHIFT", "SPACE", "C", "LCTRL"]
        );
        assert_eq!(k.move_stance_hold, "");
        assert_eq!(k.move_keys().unwrap(), crate::move_input::MoveKeys::mw2_defaults());
        assert_eq!(d.first_person.yaw_forward_axis, YawAxisCfg::MinusZ);
        assert!(d.hud.stance_icon);
        let c = Config::parse(
            "[movement]\ntp_anim = \"er\"\nsprint_mode = \"hold\"\nstance_eye = \"mw2\"\n[first_person]\nyaw_forward_axis = \"auto\"\n[hud]\nstance_icon = false\n",
        )
        .unwrap();
        assert_eq!((c.movement.tp_anim, c.movement.sprint_mode, c.movement.stance_eye), (TpAnim::Er, SprintModeCfg::Hold, StanceEye::Mw2));
        assert_eq!(c.first_person.yaw_forward_axis, YawAxisCfg::Auto);
        assert!(!c.hud.stance_icon);
    }

    #[test]
    fn config_movement_bounds_unknown_and_duplicates() {
        for bad in [
            "[movement]\nunknown_key = 1\n",
            "[movement]\nspeed_scale = 3.0\n",
            "[movement]\nspeed_scale = nan\n",
            "[movement]\nmax_rays_per_tick = 4\n",
            "[movement]\nmax_bumps = 0\n",
            "[movement]\nhull_radius_m = 0.1\n",
            "[movement]\nwalkable_normal_y = 1.5\n",
            "[movement]\nstrafe_sign = 0\n",
            "[movement]\ntp_anim = \"none\"\n",
            "[movement]\nblank_er = [\"E\"]\n",
            "[movement]\nblank_er = [\"F5\"]\n",
            "[movement]\nblank_er = [\"NOPE\"]\n",
            "[movement]\nfreefall_handback_m = inf\n",
            "[movement]\ndata = \"\"\n",
            "[keys]\nmove_crouch = \"LSHIFT\"\n",
            "[keys]\nmove_jump = \"R\"\n",
            "[keys]\nmove_prone = \"E\"\n",
            "[keys]\nmove_sprint = \"F5\"\n",
            "[keys]\nmove_stance_hold = \"NOPE\"\n",
            "[first_person]\nyaw_forward_axis = \"x\"\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        // the AC-130 in-ride keys may overlap the blank list
        Config::parse("[keys]\nac130_next = \"Q\"\nac130_thermal = \"F\"\n").unwrap();
    }

    #[test]
    fn config_movement_data_is_cold() {
        let d = Config::default();
        let mut n = d.clone();
        n.movement.data = "elsewhere.json".into();
        n.movement.speed_scale = 1.2;
        let (m, ignored) = d.merge_hot(&n, false);
        assert_eq!(m.movement.data, d.movement.data);
        assert_eq!(m.movement.speed_scale, 1.2);
        assert!(ignored.contains(&"movement.data"));
        assert_eq!(d.changed_sections(&n), vec!["movement"]);
    }
}

#[cfg(test)]
mod m7_tests {
    use super::*;

    #[test]
    fn config_nuke_defaults_and_bounds() {
        let d = Config::default();
        let n = &d.nuke;
        assert!(n.enabled && !n.always_available && n.keep_earned_on_death && n.spare_friendly && n.player_dies && n.flash && n.shake && n.hud_icon);
        assert!(n.dry_run, "run 15: NPC kills only logged until the team offset is confirmed");
        assert!(!Config::parse("[nuke]
dry_run = false
").unwrap().nuke.dry_run);
        assert_eq!((n.streak_cost, n.radius_m, n.kill_mode.as_str(), n.kills_per_tick, n.bullets_per_tick), (25, 200.0, "speffect", 16, 8));
        assert_eq!(n.spare_teams, vec![0, 2, 8, 10, 12, 20, 26, 28, 30, 47]);
        assert_eq!(n.retry_s, vec![0.5, 1.5, 3.0, 5.0]);
        assert_eq!((n.player_delay_s, n.aftermath_hold_s, n.shake_scale, n.shake_s, n.horizon_count, n.horizon_max_live), (3.0, 8.0, 0.6, 10.0, 3, 160));
        assert_eq!((n.announcer.as_str(), n.assets_dir.as_str()), ("US", r"converted\nuke"));
        assert_eq!((d.spawn.emitter_warn, d.spawn.emitter_throttle, d.spawn.emitter_reserve), (192, 224, 32));
        let r = &d.regulation.nuke;
        assert_eq!((r.lethal_speffect_id, r.clone_speffect_from, r.lethal_bullet_id, r.horizon_bullet_id), (79_000_960, 17_039, 79_000_961, 79_000_962));
        assert!(n.allow_in_coop, "co-op C2: the nuke works in sessions by default (user decision 2026-10-19)");
        assert!(!Config::parse("[nuke]\nallow_in_coop = false\n").unwrap().nuke.allow_in_coop);
        let c = Config::parse("[nuke]\nalways_available = true\nspare_friendly = false\nkill_mode = \"both\"\n").unwrap();
        assert!(c.nuke.always_available && !c.nuke.spare_friendly);
        for bad in [
            "[nuke]\nkill_mode = \"hp\"\n",
            "[nuke]\nradius_m = inf\n",
            "[nuke]\nradius_m = 1.0\n",
            "[nuke]\nstreak_cost = 0\n",
            "[nuke]\nretry_s = [1.0, 0.5]\n",
            "[nuke]\nretry_s = [nan]\n",
            "[nuke]\nplayer_delay_s = 0.0\n",
            "[nuke]\nannouncer = \"RU\"\n",
            "[nuke]\nkills_per_tick = 0\n",
            "[nuke]\nbogus = 1\n",
            "[spawn]\nemitter_warn = 240\n",
            "[spawn]\nemitter_throttle = 300\n",
            "[spawn]\nemitter_reserve = 200\n",
            "[regulation.nuke]\nlethal_speffect_id = 17039\n",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted: {bad:?}");
        }
        // assets_dir is cold, the rest is hot
        let mut nn = d.clone();
        nn.nuke.assets_dir = "elsewhere".into();
        nn.nuke.radius_m = 300.0;
        let (m, ignored) = d.merge_hot(&nn, false);
        assert_eq!((m.nuke.assets_dir.as_str(), m.nuke.radius_m), (r"converted\nuke", 300.0));
        assert!(ignored.contains(&"nuke.assets_dir"));
        assert_eq!(d.changed_sections(&nn), vec!["nuke"]);
    }

    #[test]
    fn config_keys_m7_nuke_key_free() {
        let d = Config::default();
        assert_eq!(d.keys.nuke, "4", "MW2 +actionslot 4");
        // 4 is free: none of toggle / F6 / F7 / Back / Q / 1 / 2 / 3 / F / WASD / LSHIFT / SPACE / C / LCTRL / R
        d.validate().unwrap();
        for bad in ["F5", "F6", "F7", "Back", "Q", "1", "2", "3", "F", "W", "A", "S", "D", "LSHIFT", "SPACE", "C", "LCTRL", "R", "E", "X", "LALT", "F99"] {
            assert!(Config::parse(&format!("[keys]\nnuke = \"{bad}\"\n")).is_err(), "nuke key {bad} accepted");
        }
        assert!(Config::parse("[keys]\nnuke = \"5\"\n").is_ok());
    }

    #[test]
    fn config_mantle_defaults_bounds_and_unknown_key() {
        let d = Config::default();
        let m = &d.movement.mantle;
        assert!(m.enabled && m.key_mode == "mw2" && !m.require_forward && m.over == "thin" && m.vm == "drop");
        assert_eq!((m.max_height_m, m.max_rise_in, m.check_angle_deg, m.check_range_in, m.time_scale), (2.0, 60.0, 60.0, 20.0, 1.0));
        assert_eq!((m.over_max_drop_m, m.max_rays, m.refuse_cache_ms), (2.0, 24, 150));
        assert!(m.crouch_fallback && m.low_keeps_weapon && m.sound && m.log_refusals);
        assert!(!d.movement.ladder_er_attacks);
        // a missing block is the defaults; a full block parses
        let c = Config::parse("[movement.mantle]\nenabled = false\nkey_mode = \"auto\"\nmax_height_m = 2.5\n").unwrap();
        assert!(!c.movement.mantle.enabled && c.movement.mantle.key_mode == "auto");
        let sim = c.movement.mantle.sim();
        assert_eq!(sim.key_mode, crate::mantle::KeyMode::Auto);
        assert!((sim.max_height_in - 2.5 / 0.0254).abs() < 1e-3);
        for bad in [
            "max_height_m = 0.9", "max_height_m = 2.6", "max_rise_in = 39", "max_rise_in = 81", "check_angle_deg = 91",
            "check_range_in = 3", "check_range_in = 41", "time_scale = 0.4", "time_scale = 2.1", "over_max_drop_m = 3.1",
            "max_rays = 19", "max_rays = 41", "refuse_cache_ms = 1001", "key_mode = \"hold\"", "over = \"always\"",
            "vm = \"none\"", "max_height_m = nan", "bogus = 1",
        ] {
            assert!(Config::parse(&format!("[movement.mantle]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn config_rays_defaults_modes_and_bounds() {
        let d = Config::default();
        let r = &d.movement.rays;
        // 2026-10-20: the default is back to union (player mode stalled terrain movement in the solo test)
        assert_eq!((r.map_filter, r.object_filter, r.mode.as_str(), r.body_probes), (0x0200_0058, 0x5b, "union", true));
        assert_eq!(r.filters(), ([0x0200_0058, 0x5b], 2));
        let c = Config::parse("[movement.rays]\nmode = \"union\"\n").unwrap();
        assert_eq!(c.movement.rays.filters(), ([0x0200_0058, 0x5b], 2), "union = the run-18 behaviour");
        let c = Config::parse("[movement.rays]\nmode = \"map\"\n").unwrap();
        assert_eq!(c.movement.rays.filters(), ([0x0200_0058, 0], 1), "map = the M8 behaviour");
        let c = Config::parse("[movement.rays]\nmode = \"object\"\nobject_filter = 0x5d\n").unwrap();
        assert_eq!(c.movement.rays.filters(), ([0x5d, 0], 1));
        let c = Config::parse("[movement.rays]\nmode = \"union\"\nobject_filter = 0\n").unwrap();
        assert_eq!(c.movement.rays.filters(), ([0x0200_0058, 0], 1), "object_filter 0 = off");
        for bad in ["mode = \"both\"", "map_filter = 0", "mode = \"object\"\nobject_filter = 0", "object_filter = -1", "bogus = 1"] {
            assert!(Config::parse(&format!("[movement.rays]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    /// C3 (docs/C3_DESIGN.md 1.3): the player mode uses the live layer, falls back, and validates its keys.
    #[test]
    fn c3_rays_player_mode_config() {
        let d = Config::parse("[movement.rays]
mode = \"player\"
").unwrap();
        let r = &d.movement.rays;
        assert_eq!((r.player_fallback, r.player_layers.as_slice(), r.log_live_table), (0x3, &[2u8, 3, 4][..], true));
        assert_eq!(r.filters_with(0x2), ([0x2, 0], 1), "the live layer wins");
        assert_eq!(r.filters_with(0), ([0x3, 0], 1), "no live layer: the fallback");
        assert!(r.is_player());
        // the rollback modes ignore the live layer
        let c = Config::parse("[movement.rays]\nmode = \"union\"\n").unwrap();
        assert_eq!(c.movement.rays.filters_with(0x3), ([0x0200_0058, 0x5b], 2));
        let c = Config::parse("[movement.rays]\nmode = \"player\"\nplayer_fallback = 0x4\nplayer_layers = [3]\n").unwrap();
        assert_eq!(c.movement.rays.filters(), ([0x4, 0], 1));
        for bad in ["player_fallback = 0x80", "player_fallback = 0", "player_fallback = 0x2000003", "player_layers = []", "player_layers = [128]"] {
            assert!(Config::parse(&format!("[movement.rays]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn c3_assist_config_defaults_and_bounds() {
        let a = Config::default().movement.assist;
        assert_eq!((a.enabled, a.stall_speed, a.stall_s, a.hold_s, a.max_per_min, a.after_refusal), (true, 0.15, 0.4, 0.8, 8, true));
        let p = a.params();
        assert_eq!((p.hold_s, p.max_per_min), (0.8f32 as f64, 8));
        for bad in ["hold_s = 0.1", "hold_s = 2.5", "max_per_min = 0", "stall_s = nan", "bogus = 1"] {
            assert!(Config::parse(&format!("[movement.assist]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn c3_fp_talk_config_defaults_and_bounds() {
        let f = Config::default().first_person;
        assert_eq!((f.talk_stale_s, f.talk_menu_idx, f.fade_stale_s, f.f6_overrides_latches), (2.0, 3, 3.0, true));
        assert_eq!(Config::parse("[first_person]\ntalk_menu_idx = -1\n").unwrap().first_person.talk_menu_idx, -1);
        for bad in ["talk_stale_s = 0.1", "fade_stale_s = 100.0", "talk_menu_idx = 128", "talk_menu_idx = -2"] {
            assert!(Config::parse(&format!("[first_person]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn fp_talk_interact_window_default_and_bounds() {
        assert_eq!(Config::default().first_person.talk_interact_window_s, 8.0);
        assert_eq!(Config::parse("[first_person]\ntalk_interact_window_s = 0.0\n").unwrap().first_person.talk_interact_window_s, 0.0);
        for bad in ["talk_interact_window_s = -1.0", "talk_interact_window_s = 61.0", "talk_interact_window_s = nan"] {
            assert!(Config::parse(&format!("[first_person]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn c3_uiyield_config_defaults_and_bounds() {
        let d = Config::default();
        let y = &d.ui_yield;
        assert_eq!((y.enabled, y.mode.as_str(), y.drop_in, y.hud_dim_alpha), (true, "lower", [-2.0, 0.0, -9.0], 0.35));
        assert_eq!(y.indices, vec![15, 24, 66]);
        assert_eq!(y.toast_anchor, [0.012, 0.30]);
        assert!(!d.ui.probe_scaleform, "the Scaleform probe is off by default");
        // C5: cutout defaults and bounds
        assert_eq!(y.cutout_feather, 0.025);
        assert_eq!(y.cutout_idx, vec![[0.32, 0.70, 0.68, 0.86], [0.12, 0.86, 0.88, 0.97]]);
        assert_eq!(y.cutout_popup, vec![[0.28, 0.30, 0.72, 0.70]]);
        assert_eq!(y.cutout_blink, vec![[0.08, 0.36, 0.92, 0.64]]);
        assert!(!y.cutout() && y.active());
        let c = Config::parse("[ui_yield]\nmode = \"cutout\"\ncutout_status = []\ncutout_feather = 0.0\n").unwrap();
        assert!(c.ui_yield.cutout() && c.ui_yield.cutout_status.is_empty());
        assert_eq!(c.ui_yield.cutout_regions().map(|(b, r)| (b, r.len())), [(1, 1), (2, 1), (4, 0), (8, 1), (16, 1), (32, 2)]);
        for bad in [
            "cutout_feather = 0.5",
            "cutout_popup = [[0.5, 0.5, 0.5, 0.9]]",
            "cutout_idx = [[0.1, 0.2, 1.2, 0.3]]",
            "cutout_blink = [[0.1, 0.2, 0.3]]",
            "cutout_summon = [[0.0, 0.0, 0.1, 0.1], [0.0, 0.0, 0.1, 0.1], [0.0, 0.0, 0.1, 0.1], [0.0, 0.0, 0.1, 0.1], [0.0, 0.0, 0.1, 0.1]]",
        ] {
            assert!(Config::parse(&format!("[ui_yield]\n{bad}\n")).is_err(), "{bad} accepted");
        }
        for bad in ["[ui_yield]\nmode = \"fade\"\n", "[ui_yield]\nhud_dim_alpha = 2.0\n", "[ui_yield]\nindices = [70]\n", "[ui_yield]\ndrop_in = [0.0, 0.0, -90.0]\n", "[ui]\nbogus = 1\n"] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // [ui] is cold
        let mut n = d.clone();
        n.ui.probe_scaleform = true;
        let (m, ignored) = d.merge_hot(&n, false);
        assert!(!m.ui.probe_scaleform && ignored.contains(&"ui.probe_scaleform"));
    }

    #[test]
    fn c4_cfg_defaults_off() {
        let d = Config::default();
        assert_eq!(d.ui, UiCfg { probe_scaleform: false, inject_viewmodel: false, inject_ui_layer: false, probe_layer_sample: false, inject_ui_thread: false });
        assert!(!d.ui.ui_layer_effective() && d.ui.warnings().is_empty());
        let c = Config::parse("[ui]\nprobe_scaleform = true\ninject_viewmodel = true\ninject_ui_layer = true\n").unwrap();
        assert!(c.ui.probe_scaleform && c.ui.inject_viewmodel && c.ui.ui_layer_effective());
        assert!(Config::parse("[ui]\ninject_viewmodel = 1\n").is_err(), "a bool, not a number");
        assert!(Config::parse("[ui]\ninject_gun = true\n").is_err(), "unknown keys are refused");
    }

    #[test]
    fn c4_cfg_cold_on_hot_off() {
        let off = Config::default();
        let mut on = off.clone();
        on.ui.inject_viewmodel = true;
        on.ui.inject_ui_layer = true;
        // false -> true after boot: ignored (cold), both keys
        let (m, ignored) = off.merge_hot(&on, false);
        assert!(!m.ui.inject_viewmodel && !m.ui.inject_ui_layer);
        assert!(ignored.contains(&"ui.inject_viewmodel (turning it on)") && ignored.contains(&"ui.inject_ui_layer (turning it on)"));
        // true -> false: hot (today's behaviour back without a restart)
        let (m, ignored) = on.merge_hot(&off, false);
        assert!(!m.ui.inject_viewmodel && !m.ui.inject_ui_layer && ignored.is_empty(), "{ignored:?}");
        // and once off it stays off for the session (false -> true again is the cold case)
        let (m2, ignored) = m.merge_hot(&on, false);
        assert!(!m2.ui.inject_viewmodel && ignored.len() == 2);
        // the probe stays cold in both directions, independent of the injection keys
        let mut p = on.clone();
        p.ui.probe_scaleform = true;
        let (m, ignored) = on.merge_hot(&p, false);
        assert!(!m.ui.probe_scaleform && m.ui.inject_viewmodel && ignored == vec!["ui.probe_scaleform"]);
    }

    #[test]
    fn c4_cfg_ui_layer_warning() {
        let c = Config::parse("[ui]\ninject_ui_layer = true\n").unwrap();
        assert!(!c.ui.ui_layer_effective(), "inject_ui_layer alone is ignored");
        assert_eq!(c.ui.warnings(), vec!["ui.inject_ui_layer = true without ui.inject_viewmodel is ignored".to_string()]);
        let c = Config::parse("[ui]\ninject_viewmodel = true\n").unwrap();
        assert!(!c.ui.ui_layer_effective() && c.ui.warnings().is_empty());
    }

    #[test]
    fn c4_cfg_probe_layer_sample() {
        let d = Config::default();
        assert!(!d.ui.probe_layer_sample, "P2 is off by default");
        let c = Config::parse("[ui]\nprobe_scaleform = true\nprobe_layer_sample = true\n").unwrap();
        assert!(c.ui.probe_layer_sample && c.ui.warnings().is_empty());
        let c = Config::parse("[ui]\nprobe_layer_sample = true\n").unwrap();
        assert_eq!(c.ui.warnings(), vec!["ui.probe_layer_sample = true without ui.probe_scaleform is ignored".to_string()]);
        assert!(Config::parse("[ui]\nprobe_layer_sample = 1\n").is_err(), "a bool, not a number");
        // cold both ways
        let mut on = d.clone();
        on.ui.probe_layer_sample = true;
        let (m, ignored) = d.merge_hot(&on, false);
        assert!(!m.ui.probe_layer_sample && ignored == vec!["ui.probe_layer_sample"]);
        let (m, ignored) = on.merge_hot(&d, false);
        assert!(m.ui.probe_layer_sample && ignored == vec!["ui.probe_layer_sample"]);
    }

    #[test]
    fn c4_cfg_inject_ui_thread() {
        let d = Config::default();
        assert!(!d.ui.inject_ui_thread && !d.ui.ui_thread_effective(), "C4.1 is off by default");
        let c = Config::parse("[ui]\ninject_viewmodel = true\ninject_ui_thread = true\n").unwrap();
        assert!(c.ui.ui_thread_effective() && c.ui.warnings().is_empty());
        let c = Config::parse("[ui]\ninject_ui_thread = true\n").unwrap();
        assert!(!c.ui.ui_thread_effective(), "inject_ui_thread alone is ignored");
        assert_eq!(c.ui.warnings(), vec!["ui.inject_ui_thread = true without ui.inject_viewmodel is ignored".to_string()]);
        assert!(Config::parse("[ui]\ninject_ui_thread = 1\n").is_err(), "a bool, not a number");
        // cold on: false -> true after boot is ignored
        let mut base = d.clone();
        base.ui.inject_viewmodel = true;
        let mut on = base.clone();
        on.ui.inject_ui_thread = true;
        let (m, ignored) = base.merge_hot(&on, false);
        assert!(!m.ui.inject_ui_thread && ignored == vec!["ui.inject_ui_thread (turning it on)"]);
        // true -> false is taken; the master switch alone turns the UI-thread pass off at once (hot)
        let (m, ignored) = on.merge_hot(&base, false);
        assert!(!m.ui.inject_ui_thread && ignored.is_empty(), "{ignored:?}");
        let mut master_off = on.clone();
        master_off.ui.inject_viewmodel = false;
        let (m, ignored) = on.merge_hot(&master_off, false);
        assert!(!m.ui.ui_thread_effective() && ignored.is_empty(), "{ignored:?}");
    }

    #[test]
    fn c3_dmgnum_config_defaults_and_bounds() {
        let h = Config::default().hud;
        assert_eq!((h.damage_numbers, h.damage_numbers_s, h.damage_numbers_merge_s, h.damage_numbers_value.as_str()), (true, 0.9, 0.25, "er"));
        assert_eq!((h.damage_numbers_head_guess, h.damage_numbers_max, h.damage_numbers_scale, h.damage_numbers_suppress_with_er_tag), (true, 24, 1.0, false));
        assert!(!Config::parse("[hud]\ndamage_numbers = false\n").unwrap().hud.damage_numbers);
        for bad in ["damage_numbers_value = \"hp\"", "damage_numbers_max = 0", "damage_numbers_s = 0.0", "damage_numbers_scale = 9.0"] {
            assert!(Config::parse(&format!("[hud]\n{bad}\n")).is_err(), "{bad} accepted");
        }
    }

    /// C3 PASS B (docs/C3_DESIGN.md 4.6): the gamepad defaults are MW2's console numbers and the design's mapping.
    #[test]
    fn c3_padcfg_defaults() {
        let g = Config::default().gamepad;
        assert!(g.enabled && !g.rumble && !g.invert_y && g.accel);
        assert_eq!((g.pad_index, g.look_route.as_str(), g.input_graph, g.look_sensitivity), (-1, "mouse", 3, 1.0));
        assert_eq!((g.turnrate_yaw, g.turnrate_pitch, g.turnrate_yaw_ads, g.turnrate_pitch_ads), (260.0, 90.0, 90.0, 55.0));
        assert_eq!((g.accel_lerp, g.accel_max_yaw, g.rs_deadzone, g.rs_outer, g.ls_deadzone, g.ls_outer), (1200.0, 400.0, 0.18, 0.98, 0.20, 0.98));
        assert_eq!((g.trigger_threshold, g.hold_ms, g.mouse_deg_per_count, g.gunship_counts_per_deg, g.log_every_s), (30, 300, 0.0, 0.0, 10));
        assert_eq!(g.er_interact, "y");
        let b = g.binds().unwrap();
        use crate::pad_logic::*;
        assert_eq!(b.masks[ACT_TOGGLE], XI_L3 | XI_R3);
        assert_eq!((b.masks[ACT_FIRE], b.masks[ACT_ADS], b.masks[ACT_RELOAD], b.masks[ACT_INTERACT]), (VB_RT, VB_LT, XI_X, XI_X));
        assert_eq!((b.masks[ACT_WEAPON_NEXT], b.masks[ACT_WEAPON_PREV], b.masks[ACT_JUMP], b.masks[ACT_STANCE]), (XI_Y, XI_LB, XI_A, XI_B));
        assert_eq!((b.masks[ACT_SPRINT], b.masks[ACT_MELEE], b.masks[ACT_AC130], b.masks[ACT_NUKE], b.masks[ACT_FP]), (XI_L3, XI_R3, XI_DPAD_UP, XI_DPAD_DOWN, XI_DPAD_LEFT));
        assert_eq!((b.masks[ACT_GS_FIRE], b.masks[ACT_GS_NEXT], b.masks[ACT_GS_THERMAL], b.masks[ACT_GS_EXIT]), (VB_RT, XI_Y, XI_X, XI_BACK));
        assert!(b.blank_rs && b.blank_ls && b.tap_hold_pair());
        assert_eq!(b.blank & (XI_START | XI_BACK), 0, "start and back stay ER's");
        assert_eq!(b.blank & XI_DPAD_RIGHT, XI_DPAD_RIGHT, "review C3: d-pad right hidden in MW2 mode");
        assert_eq!(b.er_melee, crate::pad_logic::XI_RB, "review C3: R3 melee pulses ER's RB");
        assert_eq!(b.er_interact, XI_Y);
        let l = g.look();
        assert_eq!((l.yaw, l.pitch, l.yaw_ads, l.pitch_ads, l.graph), (260.0, 90.0, 90.0, 55.0, 3));
        // the shipped toml parses with the same values
        let c = Config::parse("[gamepad]\nenabled = true\n[gamepad.bind]\ntoggle = \"l3+r3\"\n[gamepad.gunship]\nexit = \"back\"\n").unwrap();
        assert_eq!(c.gamepad, g);
        assert!(Config::parse("[gamepad.bind]\nfp_toggle = \"\"\n").unwrap().gamepad.binds().unwrap().masks[ACT_FP] == 0, "unbound");
        assert!(Config::parse("[gamepad]\nlook_route = \"er\"\n").is_ok());
    }

    /// 2026-10-21 (user: "pressing E/use with controller is buggy"): X at ER's prompt (ui_states idx 19) interacts.
    #[test]
    fn pad_prompt_indices_default_and_bounds() {
        let d = Config::default();
        assert_eq!(d.gamepad.prompt_indices, vec![19]);
        d.validate().unwrap();
        assert!(Config::parse("[gamepad]
prompt_indices = []
").unwrap().gamepad.prompt_indices.is_empty());
        Config::parse("[gamepad]
prompt_indices = [19, 65]
").unwrap();
        for bad in ["[gamepad]
prompt_indices = [128]
", "[gamepad]
prompt_indices = [1, 2, 3, 4, 5, 6, 7, 8, 9]
"] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    /// 2026-10-21 (user: "a way to use heal pots while in MW2 mode"): Q / D-pad right press ER's use-item (R / X) once.
    #[test]
    fn menu_hud_off_indices_default_and_bounds() {
        let d = Config::default();
        assert_eq!((d.menu.menu_indices.as_slice(), d.menu.hud_off_indices.as_slice()), (&[28][..], &[7][..]));
        d.validate().unwrap();
        assert!(Config::parse("[menu]
hud_off_indices = []
").unwrap().menu.hud_off_indices.is_empty());
        Config::parse("[menu]
hud_off_indices = [7, 69]
").unwrap();
        for bad in ["[menu]
hud_off_indices = [128]
", "[menu]
hud_off_indices = [28]
", "[menu]
menu_indices = [7]
"] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn heal_cfg_defaults_and_conflicts() {
        let d = Config::default();
        assert_eq!((d.keys.heal.as_str(), d.keys.er_use_item.as_str()), ("Q", "R"));
        assert_eq!((d.gamepad.bind.heal.as_str(), d.gamepad.er_use_item.as_str()), ("dpad_right", "x"));
        let b = d.gamepad.binds().unwrap();
        use crate::pad_logic::*;
        assert_eq!((b.masks[ACT_HEAL], b.er_use_item), (XI_DPAD_RIGHT, XI_X));
        assert_eq!(b.blank & XI_DPAD_RIGHT, XI_DPAD_RIGHT, "still hidden: the heal never swaps the right-hand weapon");
        d.validate().unwrap();
        // Q is also the gunship's next-gun key and in movement.blank_er (ER's lock-on): both allowed
        assert_eq!(d.keys.ac130_next, "Q");
        assert!(d.movement.blank_er.iter().any(|k| k == "Q"));
        // off
        Config::parse("[keys]\nheal = \"\"\n").unwrap();
        assert!(Config::parse("[gamepad]\ner_use_item = \"\"\n").unwrap().gamepad.binds().unwrap().er_use_item == 0);
        Config::parse("[keys]\nheal = \"G\"\ner_use_item = \"T\"\n").unwrap();
        for bad in [
            "[keys]\nheal = \"E\"\n",
            "[keys]\nheal = \"NOPE\"\n",
            "[keys]\nheal = \"R\"\n",
            "[keys]\nheal = \"4\"\n",
            "[keys]\nheal = \"1\"\n",
            "[keys]\nheal = \"C\"\n",
            "[keys]\nheal = \"F7\"\n",
            "[keys]\nheal = \"G\"\ner_use_item = \"G\"\n",
            "[keys]\ner_use_item = \"E\"\n",
            "[keys]\ner_use_item = \"\"\n",
            "[keys]\nnuke = \"Q\"\n",
            "[gamepad.bind]\nheal = \"dpad_down\"\n",
            "[gamepad.bind]\nheal = \"start\"\n",
            "[gamepad]\ner_use_item = \"lt\"\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn c3_padcfg_rejects_unknown_start_duplicates() {
        for bad in [
            "[gamepad.bind]\nfire = \"r2\"\n",
            "[gamepad.bind]\nfire = \"start\"\n",
            "[gamepad.bind]\ntoggle = \"l3+start\"\n",
            "[gamepad.gunship]\nexit = \"start\"\n",
            "[gamepad.bind]\njump = \"rt\"\n",
            "[gamepad.bind]\nweapon_next = \"lb\"\n",
            "[gamepad.bind]\nreload = \"a\"\n",
            "[gamepad.bind]\ntoggle = \"l3+r3+a\"\n",
            "[gamepad.bind]\ntoggle = \"l3+l3\"\n",
            "[gamepad.gunship]\nnext = \"x\"\n",
            "[gamepad]\nblank_er = [\"start\"]\n",
            "[gamepad]\nblank_er = [\"paddle1\"]\n",
            "[gamepad]\ner_interact = \"rt\"\n",
            "[gamepad]\ner_interact = \"a+b\"\n",
            "[gamepad]\nlook_route = \"stick\"\n",
            "[gamepad]\ninput_graph = 4\n",
            "[gamepad]\nrs_deadzone = 0.95\n",
            "[gamepad]\nls_outer = 0.2\n",
            "[gamepad]\ntrigger_threshold = 0\n",
            "[gamepad]\nhold_ms = 10\n",
            "[gamepad]\npad_index = 4\n",
            "[gamepad]\nrumble = true\n",
            "[gamepad]\nturnrate_yaw = 0.0\n",
            "[gamepad]\nlook_sensitivity = nan\n",
            "[gamepad]\nbogus = 1\n",
            "[gamepad.bind]\nbogus = \"a\"\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // the reload / interact_hold tap-hold pair is the only shared button; the gunship may reuse MW2 buttons
        assert!(Config::parse("[gamepad.bind]\nreload = \"rb\"\ninteract_hold = \"rb\"\n").is_ok());
        assert!(Config::parse("[gamepad.bind]\nreload = \"rb\"\n").is_ok(), "reload apart from the hold");
        assert!(Config::parse("[gamepad.bind]\nsprint = \"\"\nmelee = \"l3\"\n").is_ok());
        assert!(Config::parse("[gamepad.bind]\ntoggle = \"back+start\"\n").is_err());
        assert!(Config::parse("[gamepad.bind]\ntoggle = \"view\"\n").is_ok(), "the Xbox name of back");
    }

    /// C3 PASS C (docs/C3_DESIGN.md 3.8): the co-op visibility and co-op nuke defaults, and what is not built.
    #[test]
    fn c3_coopcfg_defaults_and_bounds() {
        let d = Config::default();
        let c = &d.coop;
        assert!(c.visual_twins && c.peer_effects && c.peer_sounds && c.peer_tracers && c.world_guns && c.world_gun_self_tp);
        assert!(c.markers && c.hide_er_weapon && c.peer_drop_guard && c.world_gun_occlusion);
        assert_eq!((c.twin_min_interval_ms, c.hand_bone, c.marker_refresh_s, c.peer_drop_factor), (40, -1, 5.0, 1.5));
        assert_eq!((c.world_gun_lod1_m, c.twin_gun_fallback_s), (10.0, 3.0));
        assert!(c.bone_probe && !c.peer_flashes);
        assert_eq!((c.tracer_occlusion, c.tracer_occlusion_rays), (true, 32), "review 2026-10-24: tracers behind walls hidden");
        assert!(!Config::parse("[coop]\ntracer_occlusion = false\n").unwrap().coop.tracer_occlusion);
        assert!(Config::parse("[coop]\ntracer_occlusion_rays = 513\n").is_err());
        assert_eq!(Config::parse("[coop]\ntracer_occlusion_rays = 0\n").unwrap().coop.tracer_occlusion_rays, 0);
        assert_eq!(Config::parse("[coop]\nhand_bone = 22\n").unwrap().coop.hand_bone, 22, "review C3: stage B accepted");
        let n = &d.nuke;
        assert_eq!((n.coop_mode.as_str(), n.coop_everyone, n.coop_window_s, n.probe_forward), ("owned", true, 6.0, false));
        assert_eq!(n.coop_client_fallback_s, 1.0);
        assert_eq!(Config::parse("[nuke]\ncoop_client_fallback_s = 0.0\n").unwrap().nuke.coop_client_fallback_s, 0.0);
        assert_eq!((d.movement.fp_anim, d.movement.fp_anim_speed_scale, d.movement.fp_anim_rm_mps), (FpAnim::Hybrid, false, 4.1));
        assert_eq!(Config::parse("[movement]\nfp_anim = \"blank\"\n").unwrap().movement.fp_anim, FpAnim::Blank);
        assert_eq!(Config::parse("[nuke]\ncoop_mode = \"local\"\n").unwrap().nuke.coop_mode, "local");
        for bad in [
            "[coop]\nhand_bone = 1024\n",
            "[coop]\nhand_bone = -2\n",
            "[coop]\ntwin_min_interval_ms = 5000\n",
            "[coop]\nsound_max_m = 5.0\n",
            "[coop]\npeer_drop_factor = 0.5\n",
            "[coop]\nmarker_refresh_s = 0.1\n",
            "[coop]\nworld_gun_offset_m = [0.0, 0.0, 9.0]\n",
            "[coop]\nbogus = 1\n",
            "[nuke]\ncoop_mode = \"off\"\n",
            "[nuke]\ncoop_window_s = 0.0\n",
            "[nuke]\ncoop_client_fallback_s = -1.0\n",
            "[nuke]\ncoop_client_fallback_s = nan\n",
            "[nuke]\nprobe_forward = true\n",
            "[movement]\nfp_anim = \"er\"\n",
            "[movement]\nfp_anim_speed_scale = true\n",
            "[movement]\nfp_anim_rm_mps = nan\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // every [coop] key is hot
        let mut m = d.clone();
        m.coop.world_guns = false;
        m.nuke.coop_mode = "local".into();
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(!merged.coop.world_guns && merged.nuke.coop_mode == "local" && ignored.is_empty());
    }

    /// C6 (docs/C6_HOLD_DESIGN.md 4.4): every new key defaults to today's behaviour; ranges; per-gun grips.
    #[test]
    fn c6_holdcfg_defaults_and_bounds() {
        let d = Config::default();
        let c = &d.coop;
        assert_eq!((c.hold_mode.as_str(), c.er_stance.as_str(), c.tp_ads_hold.as_str()), ("offset", "off", "hand"));
        assert!(!c.tp_ads_er_aim && !c.hold_hand() && !c.stance_on());
        assert_eq!((c.hand_bone, c.hand_axes.as_str(), c.grip_offset_m, c.grip_rot_deg, c.hand_smooth), (-1, "+x,+z,+y", [0.0; 3], [0.0; 3], 0.0));
        assert_eq!((c.stance_refresh.as_str(), c.stance_min_switch_s, c.stance_peers, c.stance_peer_delay_s), ("chord", 1.5, true, 1.5));
        assert!(c.hand_bone_auto && c.grip.is_empty() && c.hold_mounted == "hand");
        let ok = Config::parse(
            "[coop]\nhold_mode = \"hand\"\ner_stance = \"crossbow\"\nhand_bone = 300\nhand_axes = \"+y,+x,+z\"\ngrip_offset_m = [0.01, 0.0, -0.02]\n\
             [coop.grip.m4]\noffset_m = [0.0, 0.02, -0.03]\n[coop.grip.famas_mp]\nrot_deg = [0.0, 0.0, 10.0]\n",
        )
        .unwrap();
        let c = &ok.coop;
        assert!(c.hold_hand() && c.stance_on());
        // per gun, key by key over the globals; by short or full name
        assert_eq!(c.grip_for("m4_mp"), ([0.0, 0.02, -0.03], [0.0; 3]));
        assert_eq!(c.grip_for("famas_mp"), ([0.01, 0.0, -0.02], [0.0, 0.0, 10.0]));
        assert_eq!(c.grip_for("scar_mp"), ([0.01, 0.0, -0.02], [0.0; 3]));
        for bad in [
            "[coop]\nhold_mode = \"bone\"\n",
            "[coop]\nhand_axes = \"+x,+y,+z\"\n",
            "[coop]\nhand_axes = \"+x,+x,+y\"\n",
            "[coop]\ngrip_offset_m = [0.6, 0.0, 0.0]\n",
            "[coop]\ngrip_rot_deg = [0.0, 181.0, 0.0]\n",
            "[coop]\nhand_smooth = 0.95\n",
            "[coop]\nhold_mounted = \"off\"\n",
            "[coop]\ntp_ads_hold = \"camera\"\n",
            "[coop]\ner_stance = \"bow\"\n",
            "[coop]\nstance_refresh = \"write\"\n",
            "[coop]\nstance_min_switch_s = 0.1\n",
            "[coop]\nstance_peer_delay_s = nan\n",
            "[coop.grip.notagun]\noffset_m = [0.0, 0.0, 0.0]\n",
            "[coop.grip.m4]\nbogus = 1\n",
            "[coop.grip.m4]\noffset_m = [0.0, 0.0, 1.1]\n",
            "[coop.grip.m4]\noffset_m = [0.0, 0.0, 0.0]\n[coop.grip.m4_mp]\nrot_deg = [0.0, 0.0, 0.0]\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // hot like every [coop] key
        let mut m = d.clone();
        m.coop.er_stance = "crossbow".into();
        m.coop.hold_mode = "hand".into();
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(merged.coop.stance_on() && merged.coop.hold_hand() && ignored.is_empty());
    }

    /// C6 attach 2 / hide / scope peek: the new keys, their bounds, and that the defaults stay inert (offset mode:
    /// no weapon read, no row patch; no peek).
    #[test]
    fn c6_attach_hide_peek_cfg() {
        let d = Config::default();
        let c = &d.coop;
        assert_eq!((c.hold_source.as_str(), c.weapon_axes.as_str(), c.hide_state_patch), ("weapon", "-y,+z,+x", true));
        assert!(!c.weapon_source() && !c.hide_patch_wanted(), "defaults (offset, stance off): no weapon read, no row patch");
        assert!(!d.first_person.tp_scope_peek && d.first_person.scope_peek_arm_ticks == 1);
        let h = Config::parse("[coop]\nhold_mode = \"hand\"\n").unwrap().coop;
        assert!(h.weapon_source() && h.hide_patch_wanted());
        let s = Config::parse("[coop]\ner_stance = \"crossbow\"\nhold_source = \"bone\"\n").unwrap().coop;
        assert!(!s.weapon_source() && s.hide_patch_wanted(), "the stance alone hides the weapon, offset mode reads no weapon model");
        assert!(!Config::parse("[coop]\nhold_mode = \"hand\"\nhide_er_weapon = false\n").unwrap().coop.hide_patch_wanted());
        assert!(!Config::parse("[coop]\nhold_mode = \"hand\"\nhide_state_patch = false\n").unwrap().coop.hide_patch_wanted());
        assert_eq!(Config::parse("[coop]\nweapon_axes = \"-z,+x,+y\"\n").unwrap().coop.weapon_axes, "-z,+x,+y");
        let p = Config::parse("[first_person]\ntp_scope_peek = true\nscope_peek_arm_ticks = 30\n").unwrap().first_person;
        assert!(p.tp_scope_peek && p.scope_peek_arm_ticks == 30);
        for bad in [
            "[coop]\nhold_source = \"hand\"\n",
            "[coop]\nweapon_axes = \"+x,+y,+z\"\n",
            "[coop]\nweapon_axes = \"Auto\"\n",
            "[coop]\nweapon_rake_deg = 50.0\n",
            "[coop]\nweapon_rake_deg = nan\n",
            "[coop]\nstance_exit_defer_s = -1.0\n",
            "[coop]\nstance_exit_defer_s = 11.0\n",
            "[first_person]\nscope_peek_arm_ticks = 0\n",
            "[first_person]\nscope_peek_arm_ticks = 31\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // hot
        let mut m = d.clone();
        m.coop.hold_source = "bone".into();
        m.first_person.tp_scope_peek = true;
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(merged.coop.hold_source == "bone" && merged.first_person.tp_scope_peek && ignored.is_empty());
    }

    /// C6 grip 3 / pose 3 / depth: the weapon-frame grip (built-in table, config keys over it), the exit deferral, the
    /// occluders and the depth probe (off by default; the probe is cold).
    #[test]
    fn c6_grip3_pose3_depth_cfg() {
        let d = Config::default();
        let c = &d.coop;
        assert_eq!((c.weapon_rake_deg, c.stance_exit_defer_s, c.world_gun_body_occlusion, c.probe_er_depth), (-18.0, 0.0, false, false));
        // the built-in grip: the default rule for a gun without a row, the table for one with
        let (o, r, src) = c.weapon_grip_for("notagun_mp");
        assert!((o[0] - 0.020).abs() < 0.0015 && (o[1] + 0.300).abs() < 0.0015 && o[2].abs() < 1e-6 && r == [0.0, 0.0, -18.0] && src == "table", "{o:?}");
        let (o, _, _) = c.weapon_grip_for("striker_mp");
        assert!(o[0].abs() < 0.0015 && (o[1] + 0.304).abs() < 0.0015, "{o:?}");
        // config keys over the table, key by key; the bone-path grip is unchanged (the globals)
        let k = Config::parse(
            "[coop]\nweapon_rake_deg = 0.0\n[coop.grip.striker]\noffset_m = [0.0, -0.5, 0.01]\n[coop.grip.rpg_mp]\noffset_m = [-0.15, -0.7, 0.0]\nrot_deg = [0.0, 0.0, -10.0]\n",
        )
        .unwrap()
        .coop;
        assert_eq!(k.weapon_grip_for("striker_mp"), ([0.0, -0.5, 0.01], [0.0, 0.0, 0.0], "mixed"));
        assert_eq!(k.weapon_grip_for("rpg"), ([-0.15, -0.7, 0.0], [0.0, 0.0, -10.0], "config"));
        assert_eq!(k.grip_for("m4_mp"), (k.grip_offset_m, k.grip_rot_deg));
        let (o0, r0, _) = k.weapon_grip_for("m4_mp");
        assert_eq!(r0, [0.0; 3], "rake 0");
        assert!((o0[0] - 4.12 * 0.0254).abs() < 1e-4 && (o0[1] + 10.72 * 0.0254).abs() < 1e-4, "rake 0: o = -A*G: {o0:?}");
        // "auto" keeps the rig map for the table
        let a = Config::parse("[coop]\nweapon_axes = \"auto\"\n").unwrap().coop;
        assert_eq!(a.weapon_grip_for("m4_mp"), c.weapon_grip_for("m4_mp"));
        // hot, except the probe (cold both ways)
        let mut m = d.clone();
        m.coop.world_gun_body_occlusion = true;
        m.coop.stance_exit_defer_s = 3.0;
        m.coop.probe_er_depth = true;
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(merged.coop.world_gun_body_occlusion && merged.coop.stance_exit_defer_s == 3.0 && !merged.coop.probe_er_depth);
        assert_eq!(ignored, vec!["coop.probe_er_depth"]);
    }

    /// C6 depth 5: the barrier probe, the sample and the ER-depth prime are off by default; the probes are cold, the prime
    /// cold on / hot off; the bias / turn keys are hot and range-checked.
    #[test]
    fn c6_depth_state_cfg() {
        let d = Config::default();
        let c = &d.coop;
        assert_eq!((c.probe_er_depth_state, c.probe_er_depth_sample, c.world_gun_er_depth), (false, false, false));
        assert_eq!((c.er_depth_bias_m, c.er_depth_bias_rel, c.er_depth_max_turn_deg), (0.03, 0.004, 3.0));
        let k = Config::parse("[coop]
probe_er_depth_state = true
probe_er_depth_sample = true
world_gun_er_depth = true
er_depth_bias_m = 0.05
er_depth_max_turn_deg = 5.0
")
            .unwrap()
            .coop;
        assert!(k.probe_er_depth_state && k.probe_er_depth_sample && k.world_gun_er_depth && k.er_depth_bias_m == 0.05 && k.er_depth_max_turn_deg == 5.0);
        for bad in [
            "[coop]
er_depth_bias_m = -0.01
",
            "[coop]
er_depth_bias_m = 0.6
",
            "[coop]
er_depth_bias_rel = 0.06
",
            "[coop]
er_depth_max_turn_deg = 0.1
",
            "[coop]
er_depth_max_turn_deg = 31.0
",
            "[coop]
er_depth_max_turn_deg = nan
",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // hot: the bias / turn keys; cold: both probes; the prime cold on
        let mut m = d.clone();
        m.coop.er_depth_bias_m = 0.1;
        m.coop.er_depth_max_turn_deg = 10.0;
        m.coop.probe_er_depth_state = true;
        m.coop.probe_er_depth_sample = true;
        m.coop.world_gun_er_depth = true;
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(merged.coop.er_depth_bias_m == 0.1 && merged.coop.er_depth_max_turn_deg == 10.0);
        assert!(!merged.coop.probe_er_depth_state && !merged.coop.probe_er_depth_sample && !merged.coop.world_gun_er_depth);
        assert_eq!(ignored, vec!["coop.probe_er_depth_state", "coop.probe_er_depth_sample", "coop.world_gun_er_depth (turning it on)"]);
        // hot off
        let mut on = d.clone();
        on.coop.world_gun_er_depth = true;
        let (off, ignored) = on.merge_hot(&d, false);
        assert!(!off.coop.world_gun_er_depth && ignored.is_empty());
    }

    /// C6 aim 4: the third-person ADS pose is off by default, needs the stance, the old probe key is its alias; hot.
    #[test]
    fn c6_aim4_cfg() {
        let d = Config::default();
        let c = &d.coop;
        assert_eq!((c.tp_ads_pose.as_str(), c.tp_ads_patch.as_str(), c.probe_er_aim), ("off", "ads", false));
        assert!(!c.ads_pose_on() && !c.aim_patch_stance());
        let on = Config::parse("[coop]\ner_stance = \"crossbow\"\ntp_ads_pose = \"er_aim\"\n").unwrap().coop;
        assert!(on.ads_pose_on() && !on.aim_patch_stance());
        assert!(!Config::parse("[coop]\ntp_ads_pose = \"er_aim\"\n").unwrap().coop.ads_pose_on(), "no stance, no aim pose");
        assert!(Config::parse("[coop]\ner_stance = \"crossbow\"\ntp_ads_er_aim = true\n").unwrap().coop.ads_pose_on(), "the old key is an alias");
        let st = Config::parse("[coop]\ner_stance = \"crossbow\"\ntp_ads_pose = \"er_aim\"\ntp_ads_patch = \"stance\"\nprobe_er_aim = true\n").unwrap().coop;
        assert!(st.aim_patch_stance() && st.probe_er_aim);
        assert!(!Config::parse("[coop]\ntp_ads_patch = \"stance\"\n").unwrap().coop.aim_patch_stance());
        for bad in ["[coop]\ntp_ads_pose = \"aim\"\n", "[coop]\ntp_ads_pose = \"on\"\n", "[coop]\ntp_ads_patch = \"always\"\n"] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        let mut m = d.clone();
        m.coop.tp_ads_pose = "er_aim".into();
        m.coop.probe_er_aim = true;
        let (merged, ignored) = d.merge_hot(&m, false);
        assert!(merged.coop.tp_ads_pose == "er_aim" && merged.coop.probe_er_aim && ignored.is_empty());
    }

    /// C6 round 6: `tp_aim_cam` = keep (suppress ER's aim camera / reticle) is the aim pose's behaviour, "er" the hot
    /// escape hatch; nothing acts without the aim pose (code default off).
    #[test]
    fn c6_aim_cam_cfg() {
        let d = Config::default();
        assert_eq!(d.coop.tp_aim_cam, "keep");
        assert!(!d.coop.aim_cam_keep(), "the aim pose is off by default: nothing installed");
        let on = Config::parse("[coop]
er_stance = \"crossbow\"
tp_ads_pose = \"er_aim\"
").unwrap().coop;
        assert!(on.aim_cam_keep(), "keep is the aim pose's behaviour");
        let er = Config::parse("[coop]
er_stance = \"crossbow\"
tp_ads_pose = \"er_aim\"
tp_aim_cam = \"er\"
").unwrap().coop;
        assert!(er.ads_pose_on() && !er.aim_cam_keep(), "the escape hatch");
        assert!(!Config::parse("[coop]
tp_aim_cam = \"keep\"
").unwrap().coop.aim_cam_keep(), "no aim pose, nothing kept");
        assert!(Config::parse("[coop]
tp_aim_cam = \"suppress\"
").is_err());
        let mut m = Config::parse("[coop]
er_stance = \"crossbow\"
tp_ads_pose = \"er_aim\"
").unwrap();
        let base = m.clone();
        m.coop.tp_aim_cam = "er".into();
        let (merged, ignored) = base.merge_hot(&m, false);
        assert!(merged.coop.tp_aim_cam == "er" && ignored.is_empty(), "hot");
    }

    /// C6 part 16: the carrier keys default off (code), act only with the stance, are hot, and the crossbow grip table is
    /// its own (the sword's keys never apply to the crossbow frame).
    #[test]
    fn c6_carrier_cfg() {
        use crate::carrier::Mode;
        let d = Config::default().coop;
        assert_eq!((d.er_carrier.as_str(), d.carrier_give, d.carrier_load, d.weapon_axes_xbow.as_str()), ("off", false, false, "-y,+z,+x"));
        assert_eq!(d.carrier_mode(), Mode::Off);
        assert_eq!(Config::parse("[coop]\ner_carrier = \"crossbow\"\n").unwrap().coop.carrier_mode(), Mode::Off, "no stance, no carrier");
        let on = Config::parse("[coop]\ner_stance = \"crossbow\"\ner_carrier = \"crossbow\"\ncarrier_give = true\n").unwrap().coop;
        assert!(on.carrier_mode() == Mode::Crossbow && on.carrier_give);
        assert_eq!(Config::parse("[coop]\ner_stance = \"crossbow\"\ner_carrier = \"dry\"\n").unwrap().coop.carrier_mode(), Mode::Dry);
        for bad in [
            "[coop]\ner_carrier = \"bow\"\n",
            "[coop]\nweapon_axes_xbow = \"auto\"\n",
            "[coop]\nweapon_axes_xbow = \"+x,+y,+z\"\n",
            "[coop.grip.m4.xbow]\noffset_m = [0.0, 0.0, 1.5]\n",
            "[coop.grip.m4.xbow]\nbogus = 1\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // the crossbow grip: its own keys over the built-in grip on weapon_axes_xbow; the sword's keys never leak in
        let k = Config::parse("[coop]\nweapon_rake_deg = 0.0\n[coop.grip.m4]\noffset_m = [0.0, -0.5, 0.01]\n[coop.grip.m4.xbow]\nrot_deg = [0.0, 0.0, 5.0]\n").unwrap().coop;
        let (o, r, src) = k.weapon_grip_xbow("m4_mp");
        let (to, _, _) = Config::parse("[coop]\nweapon_rake_deg = 0.0\n").unwrap().coop.weapon_grip_xbow("m4_mp");
        assert_eq!((o, r, src), (to, [0.0, 0.0, 5.0], "mixed"));
        assert_eq!(k.weapon_grip_for("m4_mp").0, [0.0, -0.5, 0.01], "the sword table stays");
        // hot
        let base = Config::parse("[coop]\ner_stance = \"crossbow\"\n").unwrap();
        let mut m = base.clone();
        m.coop.er_carrier = "crossbow".into();
        m.coop.carrier_give = true;
        m.coop.carrier_load = true;
        let (merged, ignored) = base.merge_hot(&m, false);
        assert!(merged.coop.carrier_mode() == Mode::Crossbow && merged.coop.carrier_give && merged.coop.carrier_load && ignored.is_empty());
    }

    /// C6 part 17: the polearm stance and the guard ADS pose: off by default, the guard only with the polearm set, the
    /// crossbow aim / its camera filter / the carrier only with the crossbow set, the yaw sub-flag only with the guard; hot.
    #[test]
    fn c6_guard_cfg() {
        use crate::carrier::Mode;
        use crate::er_stance::StanceSet;
        let d = Config::default().coop;
        assert_eq!((d.tp_guard_yaw, d.tp_guard_align_max_deg), (false, crate::hold::AIM_ALIGN_MAX_DEG));
        assert!(!d.guard_pose_on() && !d.guard_yaw_on() && !d.stance_on());
        let g = Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"guard\"\ntp_guard_yaw = true\ntp_guard_align_max_deg = 45.0\ner_carrier = \"crossbow\"\ntp_aim_cam = \"keep\"\n").unwrap().coop;
        assert!(g.stance_on() && !g.stance_crossbow() && g.stance_set() == StanceSet::Polearm);
        assert!(g.guard_pose_on() && g.guard_yaw_on() && g.tp_guard_align_max_deg == 45.0);
        // the guard never arms the crossbow aim, its camera / reticle / fade filter or the carrier
        assert!(!g.ads_pose_on() && !g.aim_cam_keep() && !g.aim_patch_stance() && g.carrier_mode() == Mode::Off);
        // the guard needs the polearm set; the crossbow aim needs the crossbow set
        assert!(!Config::parse("[coop]\ner_stance = \"crossbow\"\ntp_ads_pose = \"guard\"\n").unwrap().coop.guard_pose_on());
        assert!(!Config::parse("[coop]\ntp_ads_pose = \"guard\"\n").unwrap().coop.guard_pose_on(), "no stance, no guard");
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"er_aim\"\n").unwrap().coop.ads_pose_on());
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_er_aim = true\n").unwrap().coop.ads_pose_on());
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"off\"\ntp_guard_yaw = true\n").unwrap().coop.guard_yaw_on());
        assert_eq!(Config::parse("[coop]\ner_stance = \"crossbow\"\n").unwrap().coop.stance_set(), StanceSet::Crossbow);
        for bad in ["[coop]\ner_stance = \"spear\"\n", "[coop]\ntp_ads_pose = \"block\"\n", "[coop]\ntp_guard_align_max_deg = 91.0\n", "[coop]\ntp_guard_align_max_deg = -1.0\n"] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // hot
        let base = Config::parse("[coop]\ner_stance = \"crossbow\"\n").unwrap();
        let mut m = base.clone();
        m.coop.er_stance = "polearm".into();
        m.coop.tp_ads_pose = "guard".into();
        m.coop.tp_guard_yaw = true;
        let (merged, ignored) = base.merge_hot(&m, false);
        assert!(merged.coop.guard_yaw_on() && ignored.is_empty());
    }

    /// C6 part 17 addendum: the polearm carry grips only with the polearm stance (`polearm_grip`), long / short by the
    /// muzzle's reach, `[coop.grip.<gun>.polearm]` keys over them; the guard frame only with the guard ("auto" by
    /// default = the built-in until learned, "off" = part 17, an explicit map + turn); bounds; hot.
    #[test]
    fn c6_polearm_grip_and_guard_frame_cfg() {
        let d = Config::default().coop;
        assert!(d.polearm_grip && d.guard_axes == "auto" && d.guard_rot_deg == [0.0; 3] && d.polearm_long_min_m == crate::hold::POLEARM_LONG_MIN_M);
        assert!(!d.polearm_grip_on() && !d.guard_frame_on() && d.guard_frame_fixed().is_none(), "inert without the polearm stance / the guard");
        let p = Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"guard\"\n").unwrap().coop;
        assert!(p.polearm_grip_on() && p.guard_frame_on());
        assert!(!Config::parse("[coop]\ner_stance = \"crossbow\"\n").unwrap().coop.polearm_grip_on(), "the crossbow set keeps the sword grips");
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\npolearm_grip = false\n").unwrap().coop.polearm_grip_on());
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"guard\"\nguard_axes = \"off\"\n").unwrap().coop.guard_frame_on());
        assert!(!Config::parse("[coop]\ner_stance = \"polearm\"\nguard_axes = \"auto\"\n").unwrap().coop.guard_frame_on(), "no guard, no frame");
        // the built-in grips: long along the shaft, short ahead; the grip point on the fist either way
        let (o, r, src) = p.weapon_grip_polearm("m4_mp");
        let rig = crate::hold::parse_axes(crate::hold::WEAPON_AXES_RIG).unwrap();
        assert_eq!((o, r), crate::hold::frame_grip(crate::hold::grip_point_in("m4_mp"), &crate::hold::polearm_frame(crate::hold::PolearmKind::Long), &rig));
        assert_eq!(src, "polearm_long");
        assert_eq!(p.weapon_grip_polearm("ranger").2, "polearm_short");
        assert_eq!(Config::parse("[coop]\npolearm_long_min_m = 0.3\n").unwrap().coop.weapon_grip_polearm("ranger").2, "polearm_long");
        // per-gun keys: both = config; a rot alone keeps the grip point in the fist
        let k = Config::parse("[coop.grip.m4.polearm]\noffset_m = [0.0, -0.2, 0.0]\nrot_deg = [0.0, 0.0, 5.0]\n[coop.grip.ranger.polearm]\nrot_deg = [0.0, 0.0, -18.0]\n").unwrap().coop;
        assert_eq!(k.weapon_grip_polearm("m4_mp"), ([0.0, -0.2, 0.0], [0.0, 0.0, 5.0], "config"));
        assert_eq!(k.weapon_grip_polearm("ranger_mp"), (k.weapon_grip_for("ranger_mp").0, [0.0, 0.0, -18.0], "mixed"));
        assert_eq!(k.weapon_grip_for("m4_mp").2, "table", "the polearm keys never leak into the sword grip");
        // an explicit guard frame
        let f = Config::parse("[coop]\nguard_axes = \"-z,-y,+x\"\nguard_rot_deg = [0.0, 0.0, 10.0]\n").unwrap().coop.guard_frame_fixed().unwrap();
        let want = crate::xform::m_mul(&crate::hold::rot_xyz_deg([0.0, 0.0, 10.0]), &crate::hold::parse_axes("-z,-y,+x").unwrap());
        assert_eq!(f, want);
        for bad in [
            "[coop]\nguard_axes = \"+x,+y,+z\"\n",
            "[coop]\nguard_axes = \"on\"\n",
            "[coop]\nguard_rot_deg = [0.0, 181.0, 0.0]\n",
            "[coop]\npolearm_long_min_m = 2.5\n",
            "[coop.grip.m4.polearm]\noffset_m = [0.0, 1.5, 0.0]\n",
            "[coop.grip.m4.polearm]\nbogus = 1\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad} accepted");
        }
        // hot
        let base = Config::parse("[coop]\ner_stance = \"polearm\"\ntp_ads_pose = \"guard\"\n").unwrap();
        let mut m = base.clone();
        m.coop.polearm_grip = false;
        m.coop.guard_axes = "off".into();
        let (merged, ignored) = base.merge_hot(&m, false);
        assert!(!merged.coop.polearm_grip_on() && !merged.coop.guard_frame_on() && ignored.is_empty());
    }

    #[test]
    fn config_unstick_defaults_and_bounds() {
        let d = Config::default();
        let u = &d.movement.unstick;
        assert!(u.enabled);
        assert_eq!((u.window_s, u.progress_m, u.wish_frac, u.evidence_frac), (2.5, 0.5, 0.8, 0.1));
        assert_eq!((u.hold_s, u.max_hold_s, u.max_per_window, u.backoff_window_s), (1.0, 8.0, 4, 60.0));
        assert_eq!(u.params(), crate::move_logic::UnstickParams::default());
        assert_eq!(d.movement.mantle.depenetrate_m, 0.30);
        assert!((d.movement.mantle.sim().depenetrate_in - 0.30 / 0.0254).abs() < 1e-3);
        assert!(d.first_person.cam_gate_close_ok);
        for bad in [
            "window_s = 0.4", "window_s = 11", "progress_m = 0.01", "wish_frac = 1.5", "evidence_frac = 0", "hold_s = 0.1",
            "max_hold_s = 0.5", "max_per_window = 0", "max_per_window = 21", "backoff_window_s = 1", "bogus = 1",
        ] {
            assert!(Config::parse(&format!("[movement.unstick]\n{bad}\n")).is_err(), "{bad} accepted");
        }
        assert!(Config::parse("[movement.mantle]\ndepenetrate_m = 0.5\n").is_err());
        assert!(Config::parse("[movement.mantle]\ndepenetrate_m = 0\n").is_ok());
        assert!(Config::parse("[movement.unstick]\nenabled = false\n").is_ok());
    }

    #[test]
    fn config_keys_m8_e_rejected() {
        for k in ["reload", "toggle", "fp_toggle", "ac130", "ac130_exit"] {
            assert!(Config::parse(&format!("[keys]\n{k} = \"E\"\n")).is_err(), "keys.{k} = E accepted");
        }
        assert!(Config::parse("[movement]\nladder_er_attacks = true\n").unwrap().movement.ladder_er_attacks);
    }
}
