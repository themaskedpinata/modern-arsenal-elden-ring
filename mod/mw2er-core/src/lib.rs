//! Pure logic of the MW2-in-Elden-Ring mod (milestone M2).
//!
//! Nothing in this crate touches game memory, Win32 or any FFI: it works on plain numbers and
//! strings so every rule can be unit-tested offline (`cargo test -p mw2er-core`). The DLL crate
//! (`mw2er`) reads the game into plain samples, calls into this crate, and writes the results back.
//!
//! Module map (docs/M2_DESIGN.md section 2.1):
//! - data models: [`weaponfile`], [`weapons`], [`sounds`], [`rows`], [`config`]
//! - simulation: [`weapon_sim`], [`falloff`], [`spread`], [`aim`], [`rng`]
//! - input/menus/hits: [`input_logic`], [`menu_logic`], [`hit_logic`]
//! - presentation: [`hud_geom`], [`logfmt`], [`audio_math`], [`font_check`]
//! - safety: [`gate`] (version gate decision)
//! - helpers: [`units`], [`vec3`]
//!
//! M3 (docs/M3_DESIGN.md): viewmodel assets [`vm_assets`] (+ [`sha256`]), animation [`vm_anim`], pose
//! [`vm_pose`], projection [`vm_proj`], math [`xform`], camera validity [`cam_gate`]; [`hit_logic`] gained
//! impact attribution.
//!
//! M3b (docs/M3B_DESIGN.md): first person: the handback state machine [`fp_logic`], eye/pitch/yaw math
//! [`fp_math`], and the save/restore models of every field it writes [`fp_restore`]. After run 5: the page-verdict
//! cache of the DLL's write helper [`write_cache`] (the per-write region query cost ~35 ms on ER's heap).

//!
//! M4 (docs/M4_DESIGN.md): the AC-130 killstreak: state machine [`ac130_logic`], gunship camera [`ac130_cam`], guns
//! [`ac130_guns`], shot path and HIT attribution [`ac130_aim`], explosion damage model [`ac130_dmg`], protection
//! model [`ac130_protect`], HUD [`ac130_hud`], sound picks [`ac130_audio`], the converted data files
//! [`ac130_data`] and the thermal pass's CPU reference [`thermal`].
//! Run 8: round tracers [`ac130_tracer`] (drawn by mw2er-render before the thermal pass).
//!
//! M5 (docs/M5_DESIGN.md): MW2 movement: the data model [`movement_data`], IW4 pmove maths [`pmove`], timed sprint
//! [`sprint`], stances [`stance`], landing and fall damage [`fall`], ray collision [`move_collide`], the controller
//! [`move_sim`], its handback / gravity / resync logic [`move_logic`], keys [`move_input`] and axes [`move_aim`].

#![forbid(unsafe_code)]

pub mod ac130_aim;
pub mod arsenal;
pub mod ac130_audio;
pub mod ac130_cam;
pub mod ac130_data;
pub mod ac130_dmg;
pub mod ac130_guns;
pub mod ac130_hud;
pub mod ac130_logic;
pub mod ac130_protect;
pub mod ac130_shake;
pub mod ac130_tracer;
pub mod aim;
pub mod audio_math;
/// C4: the viewmodel under ER's UI (getter test, target formats, the gate, the per-frame state machine, the splice,
/// the fence ring, the probe verdict and line formats, the teardown order).
pub mod c4;
pub mod cam_gate;
/// C3 (docs/C3_DESIGN.md): ER's collision-filter layer table and the `player` ray mode.
pub mod collision_layers;
pub mod config;
pub mod coop;
/// C3 PASS C: co-op visibility (twins, peer effects, markers, world guns, the peer-drop guard).
pub mod coop_vis;
/// C3 review 2026-10-24: tracers hidden behind map geometry (CPU map rays: clip at the first hit, camera visibility).
pub mod tracer_occl;
/// C3: floating damage numbers.
pub mod dmgnum;
pub mod emitter_guard;
/// C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance of the hidden ER weapon (fields, patch set, chords,
/// the stray-bullet guard).
pub mod er_stance;
/// C6 part 16: the real crossbow carrier (pure plan, journal, pick, bolt trip, 2.7.2.0 pins).
pub mod carrier;
/// C6 round 6: the filtered precision-shooting getter (ER's aim camera / reticle off, the aim pose kept).
pub mod aim_cam;
/// C6 depth probe (work/c6/depth3.md 1 / 5): the read-only chain to ER's scene depth buffer and its desc rule.
pub mod er_depth;
/// C6 depth 5 (docs/C6_HOLD_DESIGN.md part 15): ER's scene-depth state from its own barriers, the copy gate, the prime math.
pub mod er_depth_state;
/// Port 2.7.2.0: supported exe builds, our per-build addresses and the boot preflight over every site.
pub mod exe_build;
/// GENERATED (tools/scripts/exe_sites.py): the per-build site tables the preflight checks.
pub mod exe_sites;
pub mod emitter_pool;
pub mod fall;
pub mod falloff;
pub mod font_check;
/// C3: the talk / fade latch params and the F6 override.
pub mod fp_latch;
pub mod fp_logic;
pub mod fp_math;
pub mod fp_restore;
pub mod gate;
pub mod hit_logic;
/// C6 PASS H1/H3 (docs/C6_HOLD_DESIGN.md parts 4-6): the MW2 world gun on the hand bone (matrices, grip, axes,
/// validation, fallback log, third-person ADS blend).
pub mod hold;
/// C6 depth fallback (work/c6/depth3.md 3): depth-only body capsules that hide a world gun behind its character.
pub mod body_occl;
pub mod hud_geom;
pub mod input_logic;
pub mod killstreak;
pub mod ladder_log;
pub mod logfmt;
pub mod mantle;
pub mod menu_logic;
pub mod move_aim;
/// C3: `MOVE assist` (ER's movement takes over for a moment when ours stalls).
pub mod move_assist;
pub mod move_collide;
pub mod move_input;
pub mod move_logic;
pub mod move_sim;
pub mod movement_data;
/// C3 PASS B: gamepad (XInput) bindings, the detour's per-read step, MW2 look graph / turn rates, analog movement.
pub mod pad_logic;
pub mod nuke_data;
pub mod nuke_fx;
pub mod nuke_logic;
pub mod nuke_targets;
pub mod pmove;
pub mod rng;
pub mod rows;
pub mod sha256;
/// C6 part 18: the third-person over-the-shoulder camera of MW2 mode (`[third_person]`).
pub mod shoulder;
pub mod sounds;
pub mod spawn_cause;
pub mod projectile;
pub mod kick;
pub mod switch_logic;
pub mod spread;
pub mod sprint;
pub mod stance;
pub mod thermal;
/// C3: the UI yield (the gun drops / the HUD dims while ER shows a message).
pub mod ui_yield;
pub mod units;
pub mod vec3;
pub mod vm_anim;
pub mod vm_assets;
pub mod vm_pose;
pub mod vm_proj;
pub mod weapon_sim;
pub mod weaponfile;
pub mod weapons;
pub mod write_cache;
pub mod xform;
