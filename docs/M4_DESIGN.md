# M4 design: the MW2 AC-130 killstreak in Elden Ring

Status: design, 2026-10-17. Nothing here has run in game yet. Sources:
- the four research findings of this workflow: `mw2-ac130-data`, `orbit-camera`, `thermal-postfx` and
  `explosions-damage`;
- `MODDING_PLAN.md`, `MODLOG.md` (runs 1-7 and the write-cost fix), and `docs/M2_DESIGN.md`, `M3_DESIGN.md`,
  `M3B_DESIGN.md`;
- `mw2er.toml`;
- the current sources `cam_hook.rs`, `audio.rs`, `build_regulation.py` and `check_m3b.ps1`;
- the offline dump in `dumps\ac130\`.

The baseline is run 7, which the user verified in game ("it works great"). It covers MW2 mode with the UMP45 (F5),
engine bullets at k = 4 (about 49 damage per hit on c4311), the M3 viewmodel, and first person through the
camera-step hook (F6). PERF averaged 57.7 ticks/s.

User choices that this design implements as given:
1. AC-130 is always available on a key (default F7, configurable). It has a cooldown, configurable with a default
   of 120 s, and a config switch turns the cooldown off for testing.
2. The player is protected while in the gunship. The method is reversible and logged. Enemies may still aggro.
3. The gunship is complete: an orbiting thermal camera with mouse aim and per-gun zoom, three guns with MW2 stats
   whose explosions damage and kill through ER's own systems, ER explosion SFX, MW2 sounds, the MW2 overlays and
   HUD, and exact restore on every exit.

## Evidence labels

Same labels as M2/M3/M3b:

- **[V] verified**: checked on this PC by a file read, a parse, a decode, a compile, a log line or an offline run.
- **[R] reported**: another project's source or a crate doc comment says so. Sources: erfps2 57e92c1, the vendored
  fromsoftware-rs 59fbd3b, iw4L as a reference reimplementation, and the Paramdex meta. Not tested here.
- **[I] inference**: our design choice or a guess. Each one has a log line or an oracle that settles it.

Rules from the project that bind this milestone:
- Never launch Elden Ring, MW2, me3 or Steam.
- Never write into a game folder or `%APPDATA%\EldenRing`. Never publish or push.
- Download only through cargo from crates.io.
- Never touch `D:\Mods\MW2inER-m3`.
- `_ac130.gsc` and the `.vision` files are read for numbers only. Nothing from them is embedded in source.

---

## 0. Summary of the decisions

| Topic | Decision | Label |
|---|---|---|
| Availability | F7 calls in the AC-130 **only while MW2 mode (F5) is on**. Otherwise F7 is ignored and logged as `AC130 blocked reason=mw2_off`. Turning F5 off during a ride ends it (`reason=f5`). Why: the HUD, audio, input capture and fire pipeline all live in MW2 mode, and an MW2 killstreak without the MW2 loadout makes no sense | I (decision) |
| Cooldown | Default 120 s from the end of each ride (any exit reason). It is ready immediately after DLL load. `cooldown_enabled = false` disables it. The cooldown is per process and not saved. MW2 itself uses kill-streak cost 11, not a cooldown | V (MW2 cost); I (ours) |
| State machine | Pure `mw2er_core::ac130_logic`: `Unavailable -> Ready -> Starting -> Active -> Ending -> Cooldown -> Ready`. Every exit path goes through one `end(reason)`. The reason has a fixed priority and is logged | I, unit-tested |
| Duration | 40 s Active, which is MW2 `level.ac130_use_duration` × `killStreakScaler` 1. MW2's HUD timer shows 43 s because it adds the 3 s intro; ours shows 40 | V |
| Camera | New **mode 2** in the existing camera-step detour (RVA 0x3bc5f0). The detour calls ER's original first. It then saves ER's 17 floats (rows 0-3 + fov, `CSCam+0x10..0x54`) and writes the gunship matrix + fov as **one 68-byte `game::write` per frame**. `ChrCam`, `ChrExFollowCam` and camera globals are never written in mode 2, so the restore is free: stop, then write ER's saved frame back only if the field still holds ours | V layout + write path; I that the render uses it (FP precedent V) |
| Orbit | Centre = player feet at call-in. Radius 116 m, altitude 107 m (MW2 c130_zoomrig tag_player, 4566/4212 in), 360° per 70 s. Default view: look at the centre (42.7° down). The MW2 tag pitch of 64.5° is selectable. `scale` defaults to 1.0. Aim is clamped to ±35° yaw/pitch around the rotating base (MW2 `PlayerLinkWeaponviewToDelta` arcs) | V gsc/arcs; I glb-derived geometry |
| FOV | Per gun, from MW2 `adsZoomFov` 45/20/5 (cg_fov degrees, Hor+ 4:3). Converted with the existing `vm_proj::vfov_from_cg_fov` to vertical radians: 34.5° / 15.1° / 3.75°. All three are config values | V numbers; I that MW2 applies them; I vertical |
| Input | While Active and the task heartbeat is fresh, the DirectInput hook accumulates the mouse `lX/lY/lZ` and zeroes them. It records then zeroes every mouse button, and zeroes the whole keyboard buffer except Esc. Our own key edges are recorded before the zeroing. The detour consumes the mouse deltas every frame. Gamepad is not blanked (known gap) | V layout; I that ER reads axes only via GetDeviceState |
| First person | If FP is on, it is handed back in the same tick with the new `fp_logic` reason `ac130`, and it is blocked while the ride lasts. After the ride FP re-arms through its normal 30 clear ticks | I |
| Thermal | One new `ThermalPass` in `mw2er-render`, recorded from the existing pre-ImGui callback. Back buffer RENDER_TARGET->COPY_SOURCE, copied to our texture, COPY_SOURCE->RENDER_TARGET (balanced, nothing fallible in between). Then one full-screen triangle does film transform + gun overlay + grain into `ctx.rtv`. Budget < 1 ms at 4K; estimate 0.08-0.15 ms | V hook order; I cost |
| Look | Default `vision = "sp"`. White hot = `ac130.vision` (contrast 1.9, brightness 0.5, desat 1). Black hot = `ac130_inverted.vision` (1.55, 0.13, invert). `"mp"` selects `thermal_mp` / `missilecam`. Fade from black over 1.0 s at start; toggle times 0.62 s / 0.51 s (MW2 MP) | V numbers; I shader form |
| Overlays | MW2 `ac130_overlay_{105mm,40mm,25mm}` in a centred 4:3 box (2880×2160 at 4K), then `ac130_overlay_grain` fullscreen (tint 0.9,1,1, alpha 0.3). Converted byte-exact from the DXT5 dumps to `.vmtex` BC3 | V layout and images |
| ER HUD | Hidden while Active by writing `CSFeManImp.hud_state` (+0x78) = 0. Restored only if it still holds 0. If ER rewrites it, we stop and log a WARN. The saved option `GameDataMan…hud_type` is never written | V offset from crate source; I behaviour |
| Guns | 105 mm / 40 mm / 25 mm with MW2 stats from the dumps (table in section 7). Full auto on LMB. Each gun has its own clip and reload clock, and reloads continue in the background. Switch with the mouse wheel or Q (next), or 1/2/3 (direct). The order is 105 -> 40 -> 25 | V stats; I order (give order) |
| Shot path | "Route A". On fire, ray from the camera position along the camera forward: map ray (`havok::map_ray`) plus a ray-capsule test against nearby characters, nearest hit wins. Wait for the MW2 flight time `dist / projectileSpeed`. Then `spawn::spawn(owner = player, explosion row, impact - 0.3 m × dir)`. No long-range engine projectile | I |
| Explosions | New Bullet + AtkParam_Pc rows cloned from the player **Fire Pot [Hit] 10030001** chain (AoE sphere, penetrating, opposeTarget only). Two-tier "additive" model: the DLL spawns a core sphere (r/2) and a ring sphere (r) in the same tick. The ring carries dmg(3r/4); the core carries dmg(r/4) − dmg(3r/4), so an enemy inside the core receives dmg(r/4) in total. A fallback single sphere carries dmg(r/2). atkPhys = round(dmg × k), k = 4 | V clone rows; I falloff model and engine semantics |
| Self-damage | AtkParam `selfTarget 0`, `friendlyTarget 0`, Bullet `isHitBothTeam 0`, owner = player. The build asserts all of these | V fields; I engine semantics |
| Protection | New SpEffect **79000950**, cloned from SpEffectParam 1540 (all damage cuts 0, no stagger, `noDead 1`, permanent). Applied and removed with the vendored `ChrInsExt::apply_speffect / remove_speffect` (RVAs 0x3e8dc0 / 0x3ee2e0 on 2.7.1.0). Presence is checked in `ChrIns.special_effect.entries()` after apply (must be 1) and after remove (must be 0). A stray copy is cleaned up at the first in-world tick of each session | V crate + row; I runtime behaviour |
| Sounds | MW2 fire, reload, explosion, switch beep, ambient loop, exit flyby and announcer, played through the existing audio thread with alias volume/pitch ranges and a distance gain for explosions. The ambient is an MP3 (192 kbps, ~247.6 s). It is decoded **offline** to a 60 s PCM WAV by a new workspace tool using `symphonia` from crates.io. The DLL keeps rodio with `hound` only | V files; I mixing |
| Regulation | Bullet 79000201-203, 79000301-303, 79000401; AtkParam_Pc with the same 7 ids; SpEffectParam 79000950. All rows sorted and in the verified-free 79xxxxxx range. `EDITED` gains `SpEffectParam`. The 13 M2 rows must stay byte-identical | V free range |
| Oracles | `tools\scripts\check_m4.ps1` runs `check_m3b.ps1` first, then converter known answers, core tests, the regulation round trip, vmtest T cases (debug layer + GBV), thermal perf, config, wiring, DLL strings, the analyzer self-test and source hygiene | (section 14) |

---

## 1. Files M4 creates or changes

```
docs\M4_DESIGN.md                                   this file
mod\mw2er-core\src\ac130_logic.rs                   (new) killstreak state machine, exit reasons, priority, cooldown
mod\mw2er-core\src\ac130_cam.rs                     (new) orbit position, base view, aim integration + clamps, matrix, bounds
mod\mw2er-core\src\ac130_guns.rs                    (new) per-gun clip/fire cadence/reload clocks, switching, pending shells
mod\mw2er-core\src\ac130_aim.rs                     (new) ray vs capsule list, nearest-hit selection, flight time
mod\mw2er-core\src\ac130_dmg.rs                     (new) MW2 radius damage model, tier split, atkPhys mapping (mirrors the Python)
mod\mw2er-core\src\ac130_protect.rs                 (new) protection apply/verify/remove model (round trip), stray cleanup rule
mod\mw2er-core\src\ac130_hud.rs                     (new) 4:3 box, blink alphas, timer/ammo text, ready/cooldown indicator text
mod\mw2er-core\src\ac130_audio.rs                   (new) alias pick (volume/pitch ranges), distance gain, reload-secondary timing
mod\mw2er-core\src\thermal.rs                       (new) film constants, shade_pixel, BC3 decoder, D3D bilinear reference, grain offsets
mod\mw2er-core\src\{config.rs,fp_logic.rs,hit_logic.rs,input_logic.rs,vm_anim.rs,rows.rs,lib.rs}   (changed)
mod\mw2er-render\src\thermal.rs                     (new) ThermalPass (scene copy, SRV ping-pong, PSO per format, timestamps)
mod\mw2er-render\shaders\thermal.hlsl               (new) vs_fullscreen, ps_thermal (SM 5.1, fxc via build.rs)
mod\mw2er-render\{build.rs,src\lib.rs}              (changed) shader loop entry + sha env vars; module export
mod\mw2er\src\ac130.rs                              (new) the controller: machine, writes, protection, shots, logs
mod\mw2er\src\game\ac130_read.rs                    (new) signals: feet, camera_type, hud_state, speffect entries, sky ray
mod\mw2er\src\{cam_hook.rs,input_hook.rs,viewmodel.rs,hud.rs,audio.rs,task.rs,shared.rs,boot.rs,data.rs,first_person.rs,spawn.rs}   (changed)
mod\sndconv\                                        (new workspace member, offline tool only) MP3 -> PCM16 WAV via symphonia
mod\vmtest\src\main.rs                              (changed) T cases + thermal perf
mod\Cargo.toml                                      (changed) member sndconv; symphonia pinned for sndconv only
mw2er.toml                                          (changed) [ac130], keys.ac130*, [regulation.ac130]
tools\scripts\convert_ac130.py                      (new) weapons/vision/timing/hud/overlay/sound conversion + --check + --self-test
tools\scripts\build_regulation.py                   (changed) AC-130 rows, SpEffectParam, assertions, self-test mutations
tools\scripts\analyze_log.py                        (changed) AC130 rows + fixtures
tools\scripts\m2_static_checks.py                   (changed) new subcommand ac130wiring (+ self-test)
tools\scripts\check_m3b.ps1                         (changed, check 8 only; see 14.3)
tools\scripts\check_m4.ps1                          (new)
tools\scripts\fixtures\m4_ac130_session.log         (new, synthetic) analyzer self-test input
MODLOG.md                                           (changed) entry
```

No new vendored patch. The calls `apply_speffect`, `remove_speffect`, `special_effect.entries()`, `CSFeManImp`
and `CSCam` already exist in fromsoftware-rs 59fbd3b **[V source]**. The only new crate is `symphonia`, used by
the offline `sndconv` tool and never linked into `mw2er.dll` (wiring check 11 asserts this).

---

## 2. Killstreak state machine (`mw2er_core::ac130_logic`) [I, unit-tested]

### 2.1 States

| State | Meaning | Leaves to |
|---|---|---|
| `Unavailable(why)` | MW2 mode off, AC-130 disabled, rows missing, hook missing, or not in world | `Ready` when every condition clears (or `Cooldown` if one is still running) |
| `Ready` | F7 accepted if the start gate passes | `Starting` |
| `Starting { tick }` | Protection applied. Waiting for it to show in `special_effect.entries()`, at most `protect_verify_ticks` (5) | `Active` once present; `Ending(protect_fail)` on timeout |
| `Active { t0 }` | Camera mode 2, input captured, thermal on, guns live | `Ending(reason)` |
| `Ending { reason, linger_until }` | Camera, input and ER HUD restored in the first Ending tick. Pending shells finish only for `timeout`/`exit_key`. Protection is removed at `max(last pending explosion + 0.5 s, now)`, or immediately for `death`/`loading`/`world_left`/`panic`/`f5` | `Cooldown` (or `Ready` if the cooldown is disabled) after the protection-removal verification |
| `Cooldown { until }` | Counting down | `Ready` |

`Unavailable` is a gate, not a lost cooldown: the cooldown clock keeps running across MW2-off periods.

### 2.2 Start gate (F7 pressed in `Ready`)

Every check that fails logs `AC130 blocked reason=<r>` (rate-limited to once per press) and leaves the state
unchanged.

| Order | Reason | Condition |
|---|---|---|
| 1 | `disabled` | `[ac130] enabled = false` |
| 2 | `mw2_off` | MW2 mode off |
| 3 | `cooldown` | still cooling down (detail `remaining_s=`) |
| 4 | `no_cam_hook` | camera-step hook not installed or prologue mismatch |
| 5 | `rows_missing` | PARAMS check did not find all AC-130 Bullet/AtkParam rows and SpEffect 79000950 |
| 6 | `not_settled` | fewer than `arm_ticks` (30) ticks in world since load, or `pers_cam_1.fov` still the 43° placeholder |
| 7 | `menu` / `loading` / `cutscene` / `talk` / `dead` | the same signals `fp_read` already reads for first person |
| 8 | `camtype` | `ChrCam.camera_type != 0` (aim cam, dist view and death cam are refused) |
| 9 | `torrent` | mounted (ride state, same signal as FP) |
| 10 | `no_sky` | `require_open_sky = true` and a map ray from feet + 2 m straight up to the camera altitude hits something (legacy dungeons, caves, cliffs). A miss means open sky |
| 11 | `already` | another ride is in Starting/Active/Ending |

### 2.3 Exit reasons (priority, highest first)

`panic` > `world_left` > `loading` > `death` > `read_fail` > `cam_fail` > `protect_fail` > `cutscene` > `menu`
> `talk` > `camtype` > `moved` > `f5` > `exit_key` > `timeout` > `config`.

| Reason | Signal |
|---|---|
| `panic` | guard caught a panic in the controller or the detour reported a fault flag |
| `world_left` | `TASK world left` (existing) |
| `loading` | load counter or heartbeat (existing `fp_read`) |
| `death` | player HP 0 or death cam (`camera_type == 7`) |
| `read_fail` | a required read failed for 3 ticks in a row (feet, CSCamera, ChrIns) |
| `cam_fail` | more than 30 consecutive detour skips while Active (bounds, NaN, or CSCam address changed) |
| `protect_fail` | 79000950 absent from `entries()` during Active (checked every 30 ticks), or not present within the Starting window |
| `cutscene` / `menu` / `talk` | existing FP signals (edge latches are reused, not re-implemented) |
| `camtype` | `camera_type` differs from the value seen at start |
| `moved` | feet more than `max_player_move_m` (25 m) from the orbit centre (grab, teleport, fall) |
| `f5` | MW2 mode turned off |
| `exit_key` | F7 pressed again or `exit_key` (default Backspace) |
| `timeout` | `t - t0 >= duration_s` |
| `config` | `[ac130] enabled` turned false (hot) |

Unlike MW2, death ends the ride. MW2 commented out `removeAC130PlayerOnDeath` **[V gsc]**, but in ER a dead player
must get ER's camera back.

### 2.4 End sequence (`end(reason)`, task thread, one tick; order is fixed)

1. `cam_hook::ac130_stop()`: `MODE = 0`, `ACTIVE = false`, spin up to 2 ms for `IN_DETOUR == 0` (existing pattern).
2. Camera restore (section 4.6), logged.
3. Input release: clear `WANT_GUNSHIP_INPUT`, swap the mouse accumulators to 0, require a release edge before LMB
   counts for the UMP45 again (existing latch pattern).
4. ER HUD restore (section 6.7), logged.
5. Thermal off: the snapshot flag is cleared, so the next present skips the pass.
6. Sounds: stop the ambient with a 0.3 s fade and play the exit flyby (`veh_ac130_ext_dist1_fade`).
7. FP: clear the `ac130` blocker. FP re-arms through its normal 30 clear ticks.
8. Protection removal follows the Ending rules in 2.1 (`AC130 PROTECT restore ...`).
9. Cooldown starts. Log `AC130 off reason=… …` (section 12).

If the player is not in world (loading or world left), steps 2 and 4 are skipped: ER owns new objects next frame,
as `first_person.rs` already does. The protection removal is deferred to the stray cleanup of the next in-world
tick.

---

## 3. Keys and config

### 3.1 Keys (`[keys]`, all hidden from ER while used)

| Key | Default | MW2 equivalent | Notes |
|---|---|---|---|
| `ac130` | `"F7"` | killstreak slot | call in, or end early during a ride |
| `ac130_exit` | `"Back"` | none | extra exit key. Esc also exits through the `menu` reason, because the pause menu still opens |
| `ac130_next` | `"Q"` | `weapnext` (wheel) | the mouse wheel also cycles; one notch (lZ ±120) is one step |
| `ac130_gun1/2/3` | `"1"`, `"2"`, `"3"` | none | direct select |
| `ac130_thermal` | `"F"` | `+activate` (F on MW2 PC) toggles white/black hot | |
| fire | LMB (`fire_mouse_button`) | `+attack` | full auto, all guns (MW2 fireType Full Auto) **[V]** |

The parser's key set must accept `Back` (or the implementer picks another free key and documents it).

### 3.2 `[ac130]` section (`mw2er.toml`; hot unless noted)

```toml
[ac130]                       # M4: MW2 AC-130 killstreak (docs\M4_DESIGN.md)
enabled              = true
assets_dir           = 'converted\ac130'   # not hot; written by tools\scripts\convert_ac130.py
cooldown_s           = 120.0
cooldown_enabled     = true   # false = no cooldown (testing)
duration_s           = 0.0    # 0 = MW2 value from converted\ac130\timing.json (40 s)
scale                = 1.0    # multiplies radius and altitude (0.25..2.0)
radius_m             = 0.0    # 0 = MW2 rig (116.0 m = 4566 in)
altitude_m           = 0.0    # 0 = MW2 rig (107.0 m = 4212 in), above the call-in feet
orbit_period_s       = 0.0    # 0 = MW2 (70 s per 360 deg)
orbit_dir            = 1      # 1 = yaw increasing (MW2 rotateyaw +360), -1 = reverse
base_view            = "center"   # center = look at the call-in point | mw2_tag = fixed 64.5 deg down
aim_arc_deg          = 0.0    # 0 = MW2 (35 deg each side)
fov_105              = 45.0   # MW2 cg_fov degrees (4:3 Hor+), converted to ER vertical radians
fov_40               = 20.0
fov_25               = 5.0
fov_is_vertical      = true   # ER pers_cam_1.fov assumed vertical (settled by debug_markers)
mouse_deg_per_count  = 0.022  # MW2 m_yaw/m_pitch convention
mouse_sens           = 1.0
zoom_sens_scale      = true   # sensitivity x (fov_current / fov_105) like MW2 zoom sensitivity
invert_y             = false
vision               = "sp"   # sp = ac130/ac130_inverted.vision | mp = thermal_mp/missilecam
thermal              = true   # false = no post-process (ImGui HUD only)
start_black_hot      = false
grain                = true
luma_weights         = [0.2126, 0.7152, 0.0722]
exposure             = 0.6    # [I] pre-exposure of the film input (section 6.4a; review M4 major 8)
hide_er_hud          = true   # CSFeManImp.hud_state = HideAll while active (restored)
require_open_sky     = true
max_player_move_m    = 25.0
explosion_tiers      = 2      # 2 = core + ring (additive) | 1 = single sphere
shells_after_exit    = true   # timeout/exit_key: shells in flight still land
max_ray_m            = 600.0
tracers              = true   # ImGui tracer line from the screen centre to the impact while the shell flies
debug_markers        = false  # ImGui dot on each nearby enemy's projected chest (FOV-convention oracle)
sync_chr_cam         = false  # fallback 1 (section 4.7): also write ChrCam.pers_cam
log_every_ticks      = 300
[ac130.audio]
gain                 = 0.8
ambient_gain         = 1.0
explosion_gain       = 1.0
explosion_floor_gain = 0.35   # minimum distance gain for explosions heard from the gunship
announce_ready       = true   # play the MW2 announcer when the AC-130 becomes ready
```

The `[regulation.ac130]` section is read by `build_regulation.py`. The DLL parser must accept it too, because
unknown keys are errors:

```toml
[regulation.ac130]
k                    = 4.0        # ER atkPhys = round(mw2_dmg * k); same k as the UMP45 (calibrated, run 2/3/7)
clone_bullet_from    = 10030001   # Fire Pot [Hit]
clone_atk_from       = 10030001
clone_speffect_from  = 1540
protect_speffect_id  = 79000950
protect_endurance_s  = 90.0       # finite effectEndurance (section 8.5; review M4 major 2), 30..600
sfx_105              = 524023     # Giantsflame Take Thee burst (fallback 300602)
sfx_40               = 300602     # Explosive Stone / Explosive Bolt burst
sfx_25               = -1         # none, as MW2's 25mm (projExplosionEffect '', impactType bullet_ap; review M4 minor 11)
```

The bounds rules are the same as M2: every float must be finite and inside its documented range, and `inf`/`nan`
are rejected. `assets_dir` is cold.

---

## 4. Camera mode 2 (gunship)

### 4.1 Where and when [V write path; I frame order]

- Same detour as FP hook mode: `exe + 0x3bc5f0`, `fn(*mut c_void, *const FD4Time)`, prologue checked
  **[V, run 6/7]**.
- `boot.rs` installs it when `[first_person] enabled && camera_mode == "hook"` **or** `[ac130] enabled`. A failed
  install gives `AC130 blocked reason=no_cam_hook` on F7.
- New `MODE: AtomicU8`: 0 = off, 1 = FP, 2 = AC-130. Mode 1 behaviour is unchanged byte for byte.
- Frame order (inferred from run 4's `er_rewrite=1` and the run 6/7 hook success): our task (group 113) ->
  ER camera step + detour -> draw. State the task publishes is used by the detour in the same frame.
- Mode 2 never runs the follow-cam detour body (`follow_pre` returns early). Mode 2 never writes ChrCam,
  ChrExFollowCam or CAM_WALL_RECOVERY unless `sync_chr_cam = true`.

### 4.2 Published state (task -> detour, atomics only)

- Orbit: centre `c` (xyz), `theta0`, `t0_ms` (QPC), `R`, `H`, `period_s`, `dir`.
- View: base-view mode, `arc_rad`, `fov_rad` (current gun), `deg_per_count`, `zoom_scale`, `invert_y`.
- Bounds: player feet (refreshed every tick).
- Mouse: `MOUSE_DX/DY/DZ: AtomicI32`, written by the input hook and swapped to 0 by the detour.
- Aim: `AIM_YAW/AIM_PITCH` offsets (f32 bits), owned by the detour and reset to 0 by the task at start.
- Outputs written by the detour each frame:
  - `ER_MAT[17]`: ER's post-original values;
  - `LAST_WRITE[17]`;
  - `CAM_POS`, `CAM_FWD`: what the user sees, used by the next shot;
  - `ER_FWD`: ER's own forward, for the `er_yaw` log;
  - counters `AC_WRITES`, `AC_SKIPS`, `AC_PERSISTED`.

### 4.3 Orbit math (`ac130_cam`, pure) [V numbers; I geometry]

- MW2 rig geometry (c130_zoomrig glb, tag_angle 30° × tag_player local offset): horizontal 4566 in = **116.0 m**,
  height 4212 in = **107.0 m**, slant 157.8 m. The tag forward points 64.5° down; the line to the centre is 42.7°
  down **[I: depends on OAT exporting local transforms]**. The converter recomputes these numbers from the glb into
  `timing.json` and check 4 holds them as known answers.
- Scale decision: keep **1.0**. MW2 MP maps (~100-150 m across) match the scale of an ER fight around the call-in
  point. Streaming and AI stay centred on the player, who stays at the centre. `scale` is the knob if LOD or
  culling looks bad (section 4.7).
- Position: `p(t) = c + (R cos θ, H, R sin θ)`, `θ = θ0 + dir · 2π · t / period`. MW2's 10° ease-in (1.94 s) is not
  reproduced; the orbit starts at full speed **[I, cosmetic]**. The 1 s fade from black hides the jump.
- `θ0`: the gunship starts **behind** the player's current ER camera heading, so the first view looks the way the
  player was looking: `θ0 = atan2(-f_h.z, -f_h.x)`, with `f_h` ER's camera forward flattened.
- Forward convention: `f(yaw, pitch) = (cos p · sin y, sin p, cos p · cos y)`, with pitch positive up.
- Base view:
  - `center`: `f0 = normalize(c - p)` (pitch ≈ −42.7°);
  - `mw2_tag`: yaw of `c - p`, pitch −64.5°.
  - The base rotates with the rig, so the aim offsets are relative to it (MW2 delta link).
- Aim: `yaw = y0 + clamp(a_y, ±arc)`, `pitch = clamp(p0 + clamp(a_p, ±arc), −89°, −5°)`, with
  `a_y += dx · k`, `a_p -= dy · k` (`+lY` = mouse toward the user = look down; reversed with `invert_y`) and
  `k = deg_per_count · sens · (zoom_sens_scale ? fov_cur/fov_105 : 1)`.
- Matrix: `r = normalize(UP × f)`, `u = f × r`, rows 0-2 = (r, u, f), w kept as ER's `m[i][3]`, row 3 = (p, ER's
  w). This gives det = +1, the convention proven by `fp_math` and CAMFRAME `det=+1.000` **[V]**.
- FOV: `fov_rad = vfov_from_cg_fov(fov_gun, aspect)` when `fov_is_vertical`, else the horizontal value directly.
  The 17th float is the fov. Gun switches are instant (no lerp) **[I]**.

### 4.4 Detour, mode 2 (no allocation, lock, log or panic; inside `catch_unwind`)

1. Call the original. Check `armed()`: ACTIVE, heartbeat ≤ 250 ms, MODE == 2.
2. Re-read `CSCamera.pers_cam_1`, which must equal `CAM_ADDR`, else skip.
3. Read 17 floats at `CSCam+0x10`. If they equal `LAST_WRITE` bit for bit, ER did not rebuild: count
   `AC_PERSISTED` and keep the older `ER_MAT`. Else store them as `ER_MAT`.
4. Swap the mouse deltas to 0, integrate the aim, compute the matrix (pure `ac130_cam::frame`).
5. Bounds (replacing FP's `MAX_EYE_DIST_M = 20`, which would reject every gunship frame): all finite,
   `|p - feet| ≤ hypot(R, H) · scale + 25 m`, `det ∈ [0.99, 1.01]`, `fov ∈ (0.01, 2.5)` rad. On failure skip the
   write and count `AC_SKIPS`.
6. **One** `game::write` of `[f32; 17]` at `CSCam+0x10`: one `NtWriteVirtualMemory` with the page check cached
   (~0.3 µs). Store `LAST_WRITE`, `CAM_POS`, `CAM_FWD`.

### 4.5 Task-side read substitution [I, required]

The task runs before the camera step, so `read_pers_cam1()` returns the previous frame's gunship matrix. Generalise
`er_view_pos` to `er_view_matrix()`: when MODE == 2 and the read equals `LAST_WRITE`, return `ER_MAT`. CAMFRAME,
`cam_gate`, the UMP45 aim and the `θ0` choice all use ER's camera. Without this they see a camera 158 m away.

### 4.6 Exact restore [I, logged]

- Review M4 major 1: save and restore are PER FLOAT (`ac130_cam::merge_er` / `restore_plan` / `substitute_er`). The
  detour keeps ER's older saved value for every float that still holds our last write (e.g. a fov ER does not
  rebuild) and takes ER's fresh value for every other float; `AC_FOV_PERSISTED` counts frames in which the fov held
  our write (`AC130CAM ... fov_persisted=`). At the end, every float that still holds our write gets ER's saved
  value back; floats ER rebuilt are left alone. The log adds `floats=<n> fov_restored=0|1 fov_persisted_frames=<n>`.
- (original rule, superseded by the per-float rule) In `end()` step 2, after the detour has stopped: if the task's
  read of the 17 floats equals `LAST_WRITE`, write `ER_MAT` back. The frame has not been drawn yet.
- Log `AC130 restore field=CSCamera.pers_cam_1+0x10..0x54 addr=… old=<ours: pos,fov> new=<er: pos,fov> ok=1`.
- If the field already differs (ER rebuilt it), log `… skipped=er_rebuilt`.
- Not in world: skip.
- Nothing else of ER's camera state was touched, so nothing else needs restoring.
- `AC_PERSISTED > 0` in the periodic line means ER does **not** rebuild rows/fov every frame. The restore above
  covers that case.

### 4.7 Fallbacks if LOD, culling or popping is wrong [I; all off by default]

1. `sync_chr_cam = true`: also write the 17 floats into `ChrCam.pers_cam` (`WorldChrMan.chr_cam + 0x10`), with the
   same saved-value and restore model generalised from `CHR_ER_POS`.
2. Tier C: skip the original in mode 2 (erfps2's FP approach). Built only if 1 fails, behind
   `[ac130] skip_original = true`.
3. `scale = 0.5`.

---

## 5. Input handling [V layout; I axis behaviour]

- `DIMOUSESTATE`/`DIMOUSESTATE2`: `lX@0`, `lY@4`, `lZ@8`, `rgbButtons@12` **[V, input_logic tests]**.
- New want flag `WANT_GUNSHIP_INPUT`. It takes effect only through `effective_want(flag, now, heartbeat)`, the
  existing 250 ms heartbeat rule.
- Mouse branch, while the flag is in effect:
  1. Read the raw LMB edge/state into the gunship fire state.
  2. `MOUSE_DX/DY/DZ.fetch_add(lX, lY, lZ)`.
  3. Zero bytes 0..12 and every `rgbButtons` byte.
- Keyboard branch:
  1. Record raw edges of F5, F6, F7, `ac130_exit`, Q, 1, 2, 3 and F.
  2. Zero all 256 bytes **except DIK_ESCAPE**, so ER's pause menu still opens and ends the ride through `menu`.
  3. A held W reads as released, so the character stops. When capture ends, ER sees the real state again.
- Start and end swap the accumulators to 0. Counts piled up during a menu must not snap the aim.
- The UMP45 fire path ignores LMB while Active and needs a release edge after Ending.
- Known gaps, accepted and logged: XInput/gamepad and Raw Input are not blanked. The analyzer reports
  `AC130 WARN player_moved=Xm` when the feet move more than 1 m during a ride (the character should stand still).
- In-game proof: the periodic `AC130CAM` line shows `mouse_counts` growing while `er_yaw` (ER's own camera yaw) stays
  constant. If `er_yaw` moves with the mouse, ER also reads axes another way. The gunship still works; the third
  person camera has just turned when the ride ends.

---

## 6. Thermal pass and overlays

### 6.1 Hook point [V vendored hudhook-0.9.3 dx12.rs:188-255]

The pre-ImGui callback runs after hudhook's PRESENT->RENDER_TARGET barrier and `OMSetRenderTargets(ctx.rtv)`, and
before `SetDescriptorHeaps` and ImGui. hudhook re-binds its RTV after us and transitions RENDER_TARGET->COMMON.
Everything we draw lands on ER's finished frame (ER's HUD included) and under our ImGui HUD. `PRE_IMGUI` is a
`OnceLock` set by `viewmodel.rs`. Its `callback_body` becomes a dispatcher:

```
if snapshot.ac130.thermal_on && thermal ready && !THERMAL_DISABLED && in_world && !menu && snapshot_age < 0.5 s
    -> ThermalPass::record(...)            (viewmodel skipped)
else -> existing viewmodel path            (vm_anim::DrawGate gains !ac130_active + test)
```

`ThermalPass` lives in its own `Mutex<Option<ThermalPass>>`, used with `try_lock` only. It is built by the existing
loader thread (`mw2er-vmload`) from `converted\ac130\*.vmtex`.

### 6.2 Back-buffer read (record order) [I, D3D12 rules]

Before **any** command, validate:
- `0 < w, h ≤ 16384`;
- `base_format(live) == base_format(rtv_format)`, the same check as the viewmodel;
- device unchanged;
- the scene copy exists at w × h (else create it on the present thread and log `AC130 WARN created scene WxH on
  present thread`);
- PSO for the format exists;
- the overlays are uploaded.

Any failure returns without recording anything. Then record, with nothing fallible in between:
1. `ResourceBarrier([BB RENDER_TARGET->COPY_SOURCE, scene PIXEL_SHADER_RESOURCE->COPY_DEST])`.
2. `CopyResource(scene, BB)`. The scene copy has the BB size, 1 mip, SampleDesc 1 and `base_format(BB)`, in a
   DEFAULT heap, created in PIXEL_SHADER_RESOURCE.
3. `ResourceBarrier([BB COPY_SOURCE->RENDER_TARGET, scene COPY_DEST->PIXEL_SHADER_RESOURCE])`.
4. Viewport and scissor full target, `OMSetRenderTargets(ctx.rtv, no DSV)`, our heap, root signature, PSO,
   TRIANGLELIST, root constants, 3 tables, `DrawInstanced(3,1,0,0)` from `SV_VertexID`.
5. Optional timestamps around 1-4 into our own query heap, resolved like the viewmodel's (`lib.rs:888-923`).

The back buffer leaves the callback in RENDER_TARGET, which is what hudhook's next barrier requires. Never keep a
COM reference to `ctx.render_target` after the call, or ER's `ResizeBuffers` fails.

### 6.3 Descriptors, resize, rings [I]

- Shader-visible heap: [0,1] scene ping-pong, [2,3,4] overlays 105/40/25, [5] grain.
- A resize creates the new scene texture and writes its SRV into the **other** scene slot. A shader-visible
  descriptor is read at execution time, so the slot an in-flight list uses must not be rewritten. The old texture is
  retired for `num_frames + 1` records (the existing rule).
- All per-frame data is root constants (≤ 32 DWORDs), so no upload ring is needed. `frame_index` is used only for the
  timestamp slots.

### 6.4 Shader (`thermal.hlsl`, SM 5.1, fxc via `build.rs`, same DXBC in DLL and vmtest)

Root constants (b0): film offset/base/delta/quad (float4 ×4, desat in `.w`), luma weights, overlay box px
(x0, y0, w, h), overlay alpha, grain uv offset/scale/alpha/tint, fade (0..1 multiplier for the fade from black),
flags (overlay index valid, grain on). Static samplers: s0 linear clamp (overlay), s1 linear wrap (grain). The
scene is read with `Load` (exact 1:1).

Per pixel, in stored (encoded) space like MW2's 2D path; the back buffer is sRGB-encoded SDR **[V log 745/748]**:

```
g    = dot(c.rgb, lumW)
d    = desat_dark + (desat - desat_dark) * g
cd   = lerp(c.rgb, g, d)
tint = base + delta*g + quad*g*g
x    = saturate(cd * tint + offset) * fade
if inside 4:3 box: ov = overlay.Sample(s0, uv_box);  x = lerp(x, ov.rgb, ov.a * ovAlpha)
if grain:          gr = grain.Sample(s1, uv_screen*scale + offset); x = lerp(x, float3(0.9,1,1)*gr.rgb, gr.a*0.3)
out  = float4(x, 1)
```

Constant packing as reported by iw4L (`postfx_plan.rs:174-226`) **[R]**:
- `offset = brightness + 0.5 − 0.5 · contrast`;
- invert: `contrast = −contrast` and `offset += 1`;
- `base = dark · contrast`, `delta = 2 · contrast · (medium − dark)`, `quad = contrast · (light − 2·medium + dark)`;
- `medium = (light + dark) / 2`.

The per-pixel form is our inference **[I]**. Known answers: SP white hot `out = 1.9 g + 0.05` (g = 0.35 -> 0.715);
SP black hot `out = −1.55 g + 0.855`.

### 6.5 Visions and transitions [V numbers]

| Mode | White hot | Black hot |
|---|---|---|
| `sp` (default) | `ac130.vision`: contrast 1.9, brightness 0.5, desat 1, invert 0, tints 1 | `ac130_inverted.vision`: 1.55, 0.13, desat 1, invert 1 |
| `mp` | `thermal_mp`: contrast 1, brightness 0, desat 1, invert 0 (glow not reproduced) | `missilecam`: contrast 3.7, brightness 1, lightTint 0, darkTint 1.21215/1.21039/1.31139 |

- Start: fade from black over 1.0 s (MW2 `black_bw` -> thermal over 1 s).
- Toggle (F): linear lerp of the packed constants, 0.62 s to black hot and 0.51 s back.
- Overlay alpha while the current gun reloads: `1 − 0.65 · sin(ms/90)`, clamped to 0..1 (MW2 menu) **[V]**.
- Grain: a random whole-texel UV offset changed at 30 Hz **[I: MW2's grain_overlay animation is in the engine
  shader]**.

Fidelity gap, stated plainly: MW2 thermal shows living things white through per-material heat. ER's frame has no
heat channel, so ours is luminance-only, and enemies will not glow.

### 6.6 Failure handling

- A back-buffer format change or a non-sRGB output (PQ/scRGB) logs `AC130 FAIL thermal reason=…` and sets
  `THERMAL_DISABLED` for the session. The gunship continues with the ImGui HUD only.
- Snapshot stale, menu or not in world: the pass is skipped and ER's image shows.
- GPU time is logged every 10 s: `AC130 gpu_ms p50= p95= max= n=`.

### 6.7 ER HUD [V offset from crate source; I behaviour]

- The ER HUD is already in the back buffer and would be recoloured. With `hide_er_hud = true`, on entry write
  `CSFeManImp.hud_state` (u8 at +0x78; HideAll = 0) through `game::write`, saving the old value.
- Log `AC130 write field=CSFeManImp.hud_state(+0x78) addr=… old=3 new=0`.
- Every tick, re-read it. If ER rewrote it for more than 10 consecutive ticks, stop writing and log
  `AC130 WARN er_hud_rewritten`; the HUD then simply shows in thermal.
- Restore in `end()` only if it still holds 0: `AC130 restore field=CSFeManImp.hud_state old=0 new=<saved> ok=1`.
- Never write `GameDataMan` `hud_type`. It is the user's saved option, and an autosave could make the change
  permanent.
- The crate says `hud_state` hides HP/FP/stamina only. Remaining elements are acceptable.

---

## 7. Weapons and firing

### 7.1 MW2 stats (converted to `converted\ac130\weapons_ac130.json`) [V dumps]

| Field | 105 mm | 40 mm | 25 mm |
|---|---|---|---|
| clipSize | 1 | 4 | 20 |
| fireTime (s) | 0.1 | 0.3 | 0.07 |
| reloadTime (s) | 5.0 | 3.0 | 1.5 |
| projectileSpeed (in/s -> m/s) | 3000 -> 76.2 | 5000 -> 127.0 | 10000 -> 254.0 |
| explosionRadius (in -> m) | 600 -> 15.24 | 150 -> 3.81 | 32 -> 0.8128 |
| explosionInner / Outer damage | 1200 / 300 | 500 / 50 | 500 / 10 |
| direct damage (not used; the core sphere covers a direct hit) | 1000 | 1000 | 500 |
| adsZoomFov | 45 | 20 | 5 |
| fireSoundPlayer / projExplosionSound | ac130_105mm_fire / exp_ac130_105mm | ac130_40mm_fire / exp_ac130_40mm | ac130_25mm_fire / none |
| bigExplosion / impactType | 1 / none | 1 / none | 0 / bullet_ap |

Derived (review M4 minor 10: `_ac130.gsc` `weaponReload()` waits reloadTime from the `weapon_fired` notify, i.e.
from the shot): 105 = 1 shot per 5.0 s; 40 = 4 shots at 0.3 s, the next 3.0 s after the 4th; 25 = 20 rounds at
857 rpm (1.33 s), the next 1.5 s after the 20th.

### 7.2 Gun simulation (`ac130_guns`) [I]

- Each gun keeps `{clip, next_fire_t, reload_until}`. Reload starts when the clip reaches 0, lasts `reloadTime`
  FROM THAT SHOT (as the gsc; the analyzer's target is reloadTime), and runs even while another gun is selected
  (MW2's `weaponReload` thread waits on time). Clip becomes full at `reload_until`.
- Fire: while LMB is held, the selected gun is not reloading, `clip > 0` and `now ≥ next_fire_t`, emit a shot. Then
  `clip -= 1`, `next_fire_t += fireTime` (catch-up capped at `max_shots_per_tick` = 2).
- Switch: wheel/Q cycles 105 -> 40 -> 25 -> 105. 1/2/3 select directly. Switching plays `ac130_weapon_switch` and
  changes the published fov at once. The new gun can fire immediately; MW2 has no raise time in the gunship
  **[I]**.
- Start: 105 selected (MW2 `SwitchToWeapon(105mm)`), all clips full.

### 7.3 Shot path ("Route A") [I]

1. Origin `o = CAM_POS`, direction `f = CAM_FWD`: the detour's last write, exactly what the user sees.
2. `t_map = havok::map_ray(o, 1, f, max_ray_m, player_ins)`. `t_chr` = nearest ray-capsule hit over
   `chars::read_nearby` (excluding the player and Torrent; radius `aim.char_radius_m`, height
   `hit.capsule_height_m`; pure `ac130_aim::first_hit`). The impact is the nearer one (`via=map|chr`). If neither
   hits, log the shot with `via=none` and play the sound, with no explosion. The capsule test matters for the 25 mm:
   a ground-only ray passes through the enemy and lands about 1.2 m behind its feet at these angles.
3. Pending shell `{gun, impact, dir, due = now + dist / speed}`: flight 2.07 / 1.24 / 0.62 s at the 157.8 m slant.
   With `tracers`, ImGui draws a projected line from the screen centre toward the impact while it flies.
4. When due (task tick): `spawn::spawn(owner = player handle, id, impact − 0.3 m · f, 1.0, f)`. With
   `explosion_tiers = 2`, spawn the core id then the ring id in the same tick; with `explosion_tiers = 1`, spawn the
   single id; 25 mm always single. The existing per-shot sanity checks apply: owner is a Chr, finite values, unit
   direction. The UMP45 "camera 0.3-15 m from the chest" check does not apply to the AC-130.
5. Pending shells are dropped on every exit reason except `timeout`/`exit_key` with `shells_after_exit`. They are
   logged as `AC130 SHELL dropped n=`.
6. The existing SPAWN breaker (8 consecutive failures, closes after 3 s) is shared with the UMP45.

### 7.4 Feedback

- On fire: fire sound plus the reload secondary at its alias `startdelay` (105: 2000 ms, 40: 1000 ms).
- Camera shake from MW2's earthquakes: 105 fire 0.2 / 1 s, 40 fire 0.1 / 0.5 s; impacts 105 0.4 / 1 s, 40 0.2 /
  0.5 s. Applied as a small decaying random aim jitter in the detour (`shake_scale` in core, default 1.0) **[I]**.
- 105 impact: dark-screen pulse (black, alpha 0 -> 0.6 over 0.2 s, at 0.4 s fade to 0 over 0.8 s) drawn by ImGui
  **[V gsc numbers]**.

---

## 8. Regulation rows

### 8.1 Ids [V free range]

The whole 79xxxxxx range is empty in vanilla Bullet, AtkParam_Pc, AtkParam_Npc, SpEffectParam and
SpEffectVfxParam. Scheme: `79000000 + slot·100 + tier`. Slot 1 = UMP45 (unchanged), 2 = 105, 3 = 40, 4 = 25.
Tier 1 = core, 2 = ring, 3 = single. Slot 9 stays the UMP marker block.

| Param | New ids (ascending) |
|---|---|
| Bullet | 79000201, 79000202, 79000203, 79000301, 79000302, 79000303, 79000401 |
| AtkParam_Pc | 79000201, 79000202, 79000203, 79000301, 79000302, 79000303, 79000401 |
| SpEffectParam | 79000950 |

There are no projectile rows (Route A) and no new markers: the bullet-list scan sees the explosion ids themselves
during their 0.07-0.1 s life. Total rows: 13 (M2) + 15 = 28.

### 8.2 Damage model [V numbers; I model]

MW2 radius damage is assumed linear: `dmg(d) = outer + (inner − outer)(1 − d/r)`. Python (`build_regulation.py`)
and Rust (`ac130_dmg`) implement the same function, and check 6 compares them.

| Gun | dmg(r/4) | dmg(3r/4) | dmg(r/2) | core atkPhys = (dmg(r/4) − dmg(3r/4))·k | ring atkPhys = dmg(3r/4)·k | single atkPhys = dmg(r/2)·k |
|---|---|---|---|---|---|---|
| 105 | 975 | 525 | 750 | 1800 | 2100 | 3000 |
| 40 | 387.5 | 162.5 | 275 | 900 | 650 | 1100 |
| 25 | - | - | 255 | - | - | 1020 |

The additive split replaces the finding's chained shared-hit-list proposal. Why: it needs neither `HitBulletID`
chaining at life end nor `isUseSharedHitList` semantics, both inferred. ER's defence curve is not linear, so two
hits do not sum exactly to one hit of the summed atk. At these atk values (≥ 650 against ~100 defence) the gap is
small **[I]**. The analyzer reports the measured totals, not just a pass.

Expected effect: the run-7 calibration measured UMP45 atkPhys 140 -> 49 on c4311 (219 HP). Every AC-130 tier
one-shots normal soldiers, much as MW2's AC-130 one-shots a 100 HP player. Field bosses take several 105 hits.
This is intended killstreak power.

### 8.3 Bullet rows: clone `10030001` "Fire Pot [Hit]", every changed field

| id | atkId_Bullet | hitRadius | life | sfxId_Bullet | HitBulletID | notes |
|---|---|---|---|---|---|---|
| 79000201 | 79000201 | 7.62 | 0.10 | `sfx_105` (524023) | −1 | 105 core |
| 79000202 | 79000202 | 15.24 | 0.07 | −1 | −1 | 105 ring |
| 79000203 | 79000203 | 15.24 | 0.10 | `sfx_105` | −1 | 105 single |
| 79000301 | 79000301 | 1.905 | 0.10 | `sfx_40` (300602) | −1 | 40 core |
| 79000302 | 79000302 | 3.81 | 0.07 | −1 | −1 | 40 ring |
| 79000303 | 79000303 | 3.81 | 0.10 | `sfx_40` | −1 | 40 single |
| 79000401 | 79000401 | 0.8128 | 0.07 | `sfx_25` (−1: none, as MW2) | −1 | 25 single |

- Clone values: atkId 10030001, hitRadius 1.6, life 0.07, sfx 300122, HitBulletID 10030002 (18 embers, dropped).
- Asserted unchanged from the clone: `isPenetrateChr/Obj/Map = 1`, `isHitBothTeam = 0`, `isUseSharedHitList = 0`,
  `dmgHitRecordLifeTime = 5.0`, `hitRadiusMax = −1`, `spreadTime = 0`, `expDelay = 0`, all velocities 0, `dist = 0`,
  `sfxId_Hit = −1`, `createLimitGroupId` (clone value).
- The build fails if any other field differs from the clone.

### 8.4 AtkParam_Pc rows: clone `10030001`, every changed field

| id | atkPhys | atkFire | atkStam | atkSuperArmor | dmgLevel | knockbackDist | atkObj |
|---|---|---|---|---|---|---|---|
| clone | 0 | 230 | 20 | 3.0 | 2 | 1.0 | 10 |
| 79000201 | 1800 | 0 | 200 | 50 | 10 | 3.0 | 100 |
| 79000202 | 2100 | 0 | 100 | 25 | 7 | 2.0 | 50 |
| 79000203 | 3000 | 0 | 200 | 50 | 10 | 3.0 | 100 |
| 79000301 | 900 | 0 | 80 | 15 | 7 | 1.5 | 30 |
| 79000302 | 650 | 0 | 40 | 8 | 3 | 1.0 | 20 |
| 79000303 | 1100 | 0 | 80 | 15 | 7 | 1.5 | 30 |
| 79000401 | 1020 | 0 | 20 | 3.0 | 2 | 0.3 | 10 |

- atkPhys comes from section 8.2 (computed, never typed). Stamina, poise, stagger and knockback are tuning guesses
  **[I]**.
- atkFire goes to 0 because k was calibrated on atkPhys. Fire resistance would otherwise break the calibration.
- Asserted unchanged: `opposeTarget = 1`, `friendlyTarget = 0`, `selfTarget = 0`, `mapHitType = 2`,
  `atkAttribute = 3`, `spAttribute = 11` (hit-effect type only **[I]**), `isAddBaseAtk = 0`,
  `isDisableParry = 1`, `isDisableNoDamage = 0`, `guardAtkRate = 100`.

### 8.5 SpEffectParam 79000950 (protection): clone 1540, every changed field

| Field | 1540 | 79000950 |
|---|---|---|
| effectEndurance | 1.0 | `protect_endurance_s` = 90 (finite; review M4 major 2: expires on its own if the DLL cannot remove it; re-applied 20 s before expiry during long rides) |
| toughnessDamageCutRate | 1 | 0 |
| saReceiveDamageRate | 1 | 0 |
| dmgLv_None, _S, _M, _L, _BlowM, _Push, _Strike, _BlowS, _Min, _Uppercut, _BlowLL, _Breath | 0 | 1 (No Stagger) |
| noDead | 0 | 1 |
| fallDamageRate | 1 | 0 |
| spCategory | 20 | 0 |
| isDisableNetSync | 0 | 1 |

Asserted unchanged: all eight `*DamageCutRate` = 0.0, all status `*DefDamageRate` = 0.0, `stateInfo` 0,
`iconId` −1, `vfxId` −1, `effectTargetSelf/Player/Live` 1, `dontDeleteOnDead` 0. No vanilla row has `noDead = 1`
**[V]**, so its engine effect is unproven. The 100 % damage cut is the main protection and `noDead` is the safety
net.

### 8.6 Build pipeline changes

- `EDITED = ("Bullet", "AtkParam_Pc", "SpEffectParam")`. The round trip requires:
  - the 191 untouched params byte-identical;
  - each edited param to differ only by the added rows, inserted in sorted order;
  - the 13 M2 rows to decode identical to the M2 build;
  - every added row to decode equal to its clone except the documented override set (8.3-8.5);
  - WitchyBND to read them back.
- Self-test: the 7 existing mutations, plus 4 new ones that must each be caught:
  - `selfTarget = 1` on 79000201;
  - `isHitBothTeam = 1` on 79000302;
  - an extra override on 79000950;
  - an unsorted SpEffect insert.
- `build\rows.json` gains an `ac130` block: per gun, the tier ids, radius_m, atkPhys and sfx, plus the protect id and
  k. The DLL's PARAMS check and the analyzer read it; nothing is hard-coded twice.
- The DLL PARAMS check (first in-world tick after each load) also looks up the 7 Bullet, 7 AtkParam_Pc and 1
  SpEffect rows. If any is missing: `AC130 blocked reason=rows_missing ids=…` (the vanilla regulation is loaded).

---

## 9. Protection [V API; I runtime]

- Calls: `ChrInsExt::apply_speffect(79000950, dont_sync = true)` and `remove_speffect(79000950)` (vendored
  `chr_ins.rs:340/349`, RVAs 0x3e8dc0 / 0x3ee2e0 for 2.7.1.0 via `rva.rs` Ww2710). They are made only from the
  CSTask (game thread), only after the version gate (otherwise `rva::get()` panics on an unknown exe), and only
  after the PARAMS check found the row.
- These are engine calls, not memory writes. They are the justified narrow path outside `game::write`: the engine
  manages the SpEffect list itself, and a raw write into it would be unsafe.
- Verify: scan `ChrIns.special_effect.entries()` for `param_id == 79000950`.
  - Before apply: must be 0. If it is 1, the stray cleanup runs first.
  - After apply: must be 1 within `protect_verify_ticks`, else `Ending(protect_fail)`. A ride never runs
    unprotected.
  - During Active: re-checked every 30 ticks, else `protect_fail`.
  - After remove: must be 0. If not, retry once next tick, then `AC130 WARN protect_stuck` and keep retrying every
    60 ticks.
- Log lines:
  - `AC130 PROTECT apply id=79000950 chr=0x… present_before=0 present=1 ok=1`;
  - `AC130 PROTECT restore id=79000950 chr=0x… present=0 ok=1 reason=<exit reason>`.
- Removal timing: after the last pending explosion + 0.5 s (owner-safe even without protection, since
  `selfTarget = 0`), or immediately for death/loading/world_left/panic/f5.
- Stray cleanup: at the first in-world tick of every session and after each load, if 79000950 is present and no
  ride is active, remove it and log `AC130 PROTECT cleanup removed=1`. This covers a quit during a ride and an
  effect that might survive in the save (separate save `ER0000.mw2er.sl2`) **[I]**.
- Player HP is sampled for the whole ride. The `AC130 off` line carries `player_hp_start=… player_hp_min=…`, and
  the analyzer FAILs if the minimum is lower than the start.
- Second layer `ChrIns.debug_flags` bit 3 `disabled_hit`: documented, **not built**. Rejected:
  `WorldChrManDbgFlags.player_no_dead` (the debug singleton may be absent in retail) and the data-module bit (private
  field).
- Enemies keep their aggro and may still attack the standing player. Grabs could still move the player, which ends
  the ride with `moved` past 25 m.

---

## 10. Sounds [V files and aliases; I mixing]

`convert_ac130.py` copies the WAVs (all PCM 16-bit 44.1 kHz) into `converted\ac130\sounds\` and writes
`sounds_ac130.json`: cue -> file list, vol min/max, pitch min/max, dist min/max, startdelay. `audio.rs` gains the
AC-130 cues on the existing channel. The random pick, volume and pitch come from `ac130_audio` (seeded `rng.rs`).
Pitch is applied with `Source::speed`.

| Cue | Source alias -> file(s) | Played |
|---|---|---|
| fire_105 | ac130_105mm_fire -> `ac130_105canon_fire.wav` (vol 0.3288, pitch 0.7-0.8) | on shot |
| reload_105 | secondary ac130_105mm_reload -> `ac130_105canon_reload.wav` (0.5115, 0.9) | +2000 ms after shot |
| fire_40 | `ac130_40mm_fire_c2.wav` (0.548-0.585, 0.85-0.95) | on shot |
| reload_40 | `ac130_40mm_reload.wav` (0.219, 0.95-1.05) | +1000 ms after shot |
| fire_25 | `ac130_25mm_fire_rev01sh2.wav` (0.2704, 0.7) | on shot |
| exp_105 | exp_ac130_105mm -> `exp_105mm_boom1-5.wav` (0.88-0.92, 0.95-1.1, dist 1000-15000 in) | at explosion |
| exp_40 | exp_ac130_40mm -> `grenade_explod_a10-a16.wav` (0.69-0.72, 0.8-1.2, dist 200-2000 in) | at explosion |
| switch | ac130_weapon_switch -> `ui_text_beep1.wav` (pitch 1.5-1.6) | on gun change |
| ambient | ambient_ac130_int1 (mp_rust) -> `amb_ac130_int0v2_lr.mp3` vol 0.3729, loop | Active, fade-out 0.3 s |
| exit | `veh_ac130_ext_dist1_fade.wav` (iw_06.iwd) | 0.5 s after exit |
| announcer | `mp_killstrk_ac130.wav` (iw_06.iwd) | becoming Ready (`announce_ready`) |

- The 25 mm has no explosion sound (MW2 none) **[V]**.
- Explosion gain = alias volume × `dist_gain(slant_dist_m, min, max)`: 1 below min, linear to 0 at max, in metres
  (`ac130_audio`, tested). At 158 m the 105 boom (max 381 m) plays at about 0.6 and the 40 mm (max 50.8 m) would
  play at 0. Decision: the final gain is `max(dist_gain, explosion_floor_gain)` with config
  `[ac130.audio] explosion_floor_gain = 0.35`, so the 40 mm stays audible as it is for MW2's gunner **[I]**.
- The secondary chains of the explosions (dist_sub, debris, close, flash-bang layers) are **not** used in v1; their
  WAVs were not checked.
- Ambient: decoded offline by `sndconv` (symphonia, mp3 feature, pinned exact version) to
  `amb_ac130_int0v2_lr_60s.wav`: the first 60.0 s, stereo 16-bit 44.1 kHz = 2,646,000 frames, about 10.6 MB.
  The source measures 192 kbps joint stereo 44.1 kHz, about 247.6 s **[V header]**. A ride lasts at most 40 s, so
  60 s never loops audibly. The DLL keeps rodio `hound`-only.
- The radio dialog (US_/UK_… gunner/pilot chatter) is not in v1.
- ER's own listener follows the render camera (erfps2 hooks SET_WWISE_LISTENER **[R]**), so ER world sounds are heard
  from the gunship. This is accepted and noted in the log line `AC130 on … listener=render_cam(inferred)`.

---

## 11. HUD (ImGui, drawn after the thermal pass, unfiltered)

Layout: from `converted\ac130\hud_layout.json` (rects, aligns, forecolors and label strings from
`ac130_hud_hd.menu`). The strings are data read at runtime, never in source. Mapping: the 640×480 virtual menu
maps into the centred 4:3 box (`ac130_hud::box(w,h)` = 2880×2160 at 4K), with menu align semantics.

While Active:
- Gun labels `105 mm` / `40 mm` / `25 mm` at the bottom left (menu y −30/−60/−90, textscale 0.625). The selected
  one blinks with alpha `1 − sin(ms/90)·0.65`.
- Static text blocks (top bar, left, right, bottom) from the layout file.
- `Reloading` at the centre (0, 130), blinking while the selected gun reloads.
- Ammo `clip / clipSize` next to the gun label.
- Countdown at the bottom right as `m:ss` of the remaining 40 s, plus a thin timer bar under it.
- Top right: AGL `(cam.y − ground.y) × 1.5` like MW2's readout (ground from the last map ray at the centre, refreshed
  every 0.75 s), plus pseudo lat/long `|x|`, `|z|`.
- Optional tracers, the 105 dark pulse, `debug_markers`.
- The normal MW2 crosshair, hitmarker and ammo are hidden. Hitmarkers on AC-130 HIT lines stay on (MW2 shows them).

Normal MW2 HUD (MW2 mode on, not in a ride):
- an indicator under the ammo counter: `AC-130 [F7] READY` (white), `AC-130 1:45` (grey, cooldown), or
  `AC-130 –` with the blocked reason when unavailable (dim; e.g. `no sky`).
- On the transition to Ready: a 2 s toast `AC-130 READY` plus the announcer.

---

## 12. Log lines (all prefixed `AC130`; one-time lines once per ride unless noted)

| Line | Proves |
|---|---|
| `AC130 cfg enabled=1 key=F7 cooldown_s=120 cooldown=1 duration_s=40 R=116.0 H=107.0 period_s=70 arc=35 base=center fov=45/20/5(v=34.5/15.1/3.75deg) vision=sp tiers=2 hook=1` | config + data loaded |
| `AC130 rows ok bullet=7 atk=7 speffect=1` / `AC130 blocked reason=rows_missing ids=…` | regulation present |
| `AC130 ready` / `AC130 cooldown remaining_s=…` (on transitions) | state machine |
| `AC130 blocked reason=<r> …` | refusal on F7 |
| `AC130 PROTECT apply id=79000950 chr=0x… present_before=0 present=1 ok=1` | protection on |
| `AC130 on center=(x,y,z) theta0_deg=… gun=105 fp_was_on=1 camtype=0 er_fov=48.0deg listener=render_cam(inferred)` | ride started |
| `AC130CAM write field=CSCamera.pers_cam_1+0x10..0x54 addr=0x… old_pos=(…) new_pos=(…) old_fov=… new_fov=…` | first write of the ride |
| `AC130 write field=CSFeManImp.hud_state(+0x78) addr=0x… old=3 new=0` | ER HUD hidden |
| `AC130CAM t=12.3 slant_m=157.8 cam_to_player=157.9 er_cam_to_player=3.5 writes=N skips=0 persisted=0 fov_deg=34.5 gun=105 aim_yaw=… aim_pitch=… mouse_counts=N er_yaw=…` (every `log_every_ticks`) | detour running, mouse captured, ER cam untouched |
| `AC130 SWITCH gun=40 via=wheel|key` | switching |
| `AC130 SHOT gun=105 n=3 clip=0/1 o=(…) dir=(…) impact=(…) dist_m=151.2 via=map|chr|none flight_s=1.98` | shots per gun |
| `AC130 RELOAD gun=40 s=3.00` / `AC130 RELOAD done gun=40 clip=4` | reload timing |
| `AC130 EXPLODE gun=105 ids=79000201+79000202 pos=(…) late_ms=…` | explosion spawned |
| `AC130 EXPLODE seen id=79000201 pos=(…)` (first sighting per id per session, bullet scan) | the engine created it |
| `HIT cid=c4311 hp 219->0 dmg=219 kill=1 via=impact src=ac130 gun=105 tier=core d_m=3.1` | damage attribution (extends the M3 HIT line) |
| `AC130 KILL gun=105 cid=c4311 d_m=3.1` | kill count per gun |
| `AC130 gpu_ms p50=0.11 p95=0.14 max=0.30 n=600` (every 10 s while active) | thermal cost |
| `PERF … ac130=1` | frame rate while active (existing PERF line + field) |
| `AC130 off reason=<r> t_s=40.0 shots=105:7/40:16/25:60 explosions=79 hits=12 kills=9 shells_dropped=0 player_hp_start=… player_hp_min=… moved_m=0.0 fp_resume=1 cooldown_s=120` | the ride ended, every exit path |
| `AC130 restore field=CSCamera.pers_cam_1+0x10..0x54 … ok=1` or `skipped=er_rebuilt|not_in_world` | camera restored |
| `AC130 restore field=CSFeManImp.hud_state old=0 new=3 ok=1` | HUD restored |
| `AC130 PROTECT restore id=79000950 present=0 ok=1 reason=…` | protection removed |
| `AC130 PROTECT cleanup removed=1` | a stray was removed |
| `AC130 WARN …` (`er_hud_rewritten`, `player_moved=Xm`, `protect_stuck`, `created scene …`) / `AC130 FAIL thermal reason=…` | degraded but safe |

### 12.1 Analyzer (`analyze_log.py`, new rows; older logs give N/A)

- **ac130 lifecycle**. PASS needs:
  - every `AC130 on` preceded by a `PROTECT apply … ok=1` and followed by exactly one `AC130 off` before the next
    `TASK world left`;
  - every off followed by a camera `restore` line (ok or skipped) and a `PROTECT restore … present=0 ok=1` (or a
    `cleanup` in the next session);
  - the HUD restore when the HUD write happened.
- **ac130 protection**. PASS needs `player_hp_min == player_hp_start` on every ride, and no `protect_stuck`.
- **ac130 guns**. For each gun with ≥ 3 shots:
  - the median interval between consecutive in-clip shots is within ±15 % of fireTime (from `weapons_ac130.json`);
  - the interval across a reload is within ±10 % of reloadTime + fireTime;
  - the clip size is never exceeded.
- **ac130 explosions**. Counts EXPLODE per gun, and needs ≥ 1 `EXPLODE seen` per tier id that was used. Lists the HIT
  `src=ac130` totals per tier and per cid (info: the measured core/ring damage on c4311).
- **ac130 perf**. PASS when:
  - the PERF windows with `ac130=1` average ≥ 50 ticks/s (FP-on baseline 57.7-60);
  - `gpu_ms p95 < 1.0`;
  - `AC130CAM persisted=0` and `skips=0` (or skips < 1 % of writes, INFO).
- **ac130 input** (INFO): `mouse_counts` grows and `er_yaw` stays within 1°; else WARN.
- AC-130 HITs are excluded from the UMP45 calibration bands.

---

## 13. Converters and outputs (`converted\ac130\`, gitignored under `/converted/`)

`tools\scripts\convert_ac130.py` (Python 3.14, stdlib only, run with `-I`) reads only `dumps\ac130\…`,
`dumps\common_mp\weapons\…` and `dumps\common_mp_assets\…`, and writes only under `converted\ac130\` and `work\m4\`.
The Unlinker step is the finding's recipe (5 runs, already done, all exit 0). `convert_ac130.py --dump` re-runs it
only if the dump folder is missing. It invokes the external GPL Unlinker; OAT code is never linked or copied.

| Output | Content | Known answers (`--check`) |
|---|---|---|
| `weapons_ac130.json` | the 7.1 table per gun (raw MW2 units + metres) | clip 1/4/20, fireTime 0.1/0.3/0.07, reload 5/3/1.5, speed 3000/5000/10000, radius 600/150/32, inner 1200/500/500, outer 300/50/10, adsZoomFov 45/20/5, damage 1000/1000/500 |
| `timing.json` | duration 40 (gsc), orbit period 70, ramp 10°, arcs 35×4, rig R/H in inches from the glb, tag pitch, reload times from gsc (5/3/1.5 must equal the weapon files), fade/toggle times 1.0/0.62/0.51 | R = 4566 ± 1 in, H = 4212 ± 1 in, tag pitch 64.5 ± 0.2°, centre pitch 42.7 ± 0.2° |
| `vision.json` | film vars of ac130, ac130_inverted, thermal_mp, missilecam, black_bw | 1.9/0.5/0; 1.55/0.13/1; thermal_mp 1/0/0; missilecam 3.7/1, darkTint 1.21215 1.21039 1.31139; black_bw 0/−1 |
| `hud_layout.json` | rects/aligns/forecolors/label strings and positions from `ac130_hud_hd.menu`; blink constants | overlay rect −320 −240 640 480 align 2 2; grain forecolor 0.9 1 1 0.3; labels y −30/−60/−90; blink 90 ms / 0.65 |
| `ac130_overlay_{105mm,40mm,25mm,grain}.vmtex` | M3 `.vmtex`, BC3_UNORM (77), 512×512, 1 mip; DXT5 payload copied byte-exact | each source DDS 262,272 B (128 + 262,144); payload sha equal to the DDS; DDS byte-identical to `dumps\common_mp_assets` |
| `work\m4\overlay_*.png` | decoded previews (Python BC3 decoder) | per-image alpha>0 coverage and mean RGBA recorded in `overlay_stats.json`; the Rust `thermal::bc3_decode` must agree exactly (check 3) |
| `sounds\*.wav` + `sounds_ac130.json` | section 10 cues | every file RIFF PCM tag 1, 16-bit, 44.1 kHz; channels/durations: 105 fire stereo 3.80 s, 105 reload mono 4.40 s, 40 fire stereo 1.11 s, 40 reload mono 2.43 s, 25 fire stereo 0.12 s, announcer mono 6.55 s; sha equal to the dumps |
| `sounds\amb_ac130_int0v2_lr_60s.wav` | from `sndconv` | stereo, 44.1 kHz, 16-bit, 2,646,000 frames, peak > 0 |
| `manifest.json` | sha256 of every input and output, converter version; written last | present, and every listed hash matches |

- `.gsc` and `.vision` are parsed by regex for the specific numbers above. The converter writes numbers only, and
  no source text is copied.
- `_ac130.gsc` is read as cp1252.
- `--self-test` mutates copies of the inputs and must catch each mutation: a weapon `clipSize` change, a vision
  contrast change, a DDS payload byte flip, a WAV sample-rate header change, and a truncated glb node.

---

## 14. Offline oracles

### 14.1 Pure logic (`mw2er-core`), required test categories (≥ 1 named test each)

- `ac130_logic`:
  - Ready on start; `blocked` for each start-gate reason in table order;
  - start -> Starting -> Active only after protection is present; Starting timeout -> `protect_fail`;
  - one test per exit reason returning that reason;
  - simultaneous triggers follow the priority order;
  - Ending removes protection immediately for death/loading/world_left/panic/f5, and after the last shell + 0.5 s
    for timeout/exit_key;
  - pending shells dropped vs kept per reason;
  - cooldown 120 s exact, disabled -> Ready at once, clock runs through MW2-off;
  - F7 during Active = `exit_key`;
  - F5 off during Active = `f5`;
  - the FP blocker set on start and cleared on end, in every path (table-driven over all reasons);
  - no state allows Active without protection present.
- `ac130_cam`:
  - orbit position known answers (θ = 0, 90°, after 35 s = 180°), R/H/scale;
  - θ0 places the gunship opposite ER's heading;
  - base view `center` pitch = −atan(H/R) = −42.67°, `mw2_tag` −64.5°;
  - aim clamps ±35° hold under huge deltas, absolute pitch [−89, −5];
  - `+lY` looks down and `invert_y` flips it;
  - zoom sensitivity scaling;
  - rows orthonormal (1e-5), det = +1 ± 1e-5 over a 10,000-sample sweep;
  - bounds reject NaN, distance and det;
  - fov conversion 45/20/5 -> 34.5/15.1/3.75° vertical at 16:9 (±0.05°);
  - rebuilding a logged CAMFRAME forward reproduces its right/up.
- `ac130_guns`:
  - cadence per gun over 10 s held fire: 105 = 2 shots at 0 and 5.1 s; 40 = 4 at 0.3 s spacing, then +3.0 s;
    25 = 20 at 0.07 s, then +1.5 s;
  - reload continues while switched away;
  - switching order with wheel ±, Q and 1/2/3;
  - catch-up capped at 2 per tick;
  - a fractional-tick timing test at 16.7 ms ticks keeps the long-run rate exact.
- `ac130_aim`:
  - ray-capsule hit/miss/graze;
  - nearest of map vs chr;
  - `via=none`;
  - flight time = dist / speed.
- `ac130_dmg`:
  - dmg(d) endpoints (d = 0 -> inner, d = r -> outer);
  - table 8.2 values exactly;
  - additive core + ring = dmg(r/4)·k;
  - rounding;
  - u16 range;
  - equality with `build\rows.json`'s ac130 block (real-data test, skipped with a reason if the build is absent,
    which counts as FAIL in check_m4).
- `ac130_protect`:
  - apply/verify/remove round trip leaves `entries` identical to before (multiset of param_ids);
  - stray cleanup removes exactly 79000950 and nothing else;
  - remove retry and `protect_stuck` after the retry limit;
  - apply refused before the gate or without the row.
- `thermal`:
  - film constants for the 4 visions (packing known answers);
  - shade_pixel known answers (SP white hot g = 0.35 -> 0.715; black hot -> 0.3125; black_bw -> 0);
  - toggle lerp endpoints;
  - BC3 decoder against `overlay_stats.json`;
  - bilinear reference vs hand-computed 2×2;
  - grain offset determinism.
- `ac130_hud`:
  - 4:3 box at 3840×2160 = (480, 0, 2880, 2160) and at 1920×1080;
  - blink alpha range [0.35, 1];
  - timer text `0:40` -> `0:00`;
  - indicator strings per state.
- `ac130_audio`:
  - dist gain endpoints;
  - pitch/vol within ranges over 10,000 seeded draws;
  - reload secondary delays 2000/1000 ms.
- `input_logic`: gunship capture zeroes bytes 0..12 and the buttons, accumulates, lets Esc through, records our key
  edges first, and is inert with a stale heartbeat.
- `fp_logic`: new reason `ac130` blocks arming and re-arms after 30 clear ticks.
- `vm_anim::DrawGate`: not drawn while `ac130_active`.
- `config`: `[ac130]` defaults, bounds, `assets_dir` cold, `[regulation.ac130]` accepted, unknown key -> error.

### 14.2 Renderer harness (`vmtest`, same `ThermalPass` code, offscreen)

All T cases run with the debug layer and again with `--gbv`. Each requires 0 InfoQueue errors. Each reproduces
hudhook's exact sequence: target in PRESENT/COMMON -> barrier ->RT -> `OMSetRenderTargets` ->
`ThermalPass::record` -> barrier RT->COMMON -> readback via `record_copy`. The test image is synthetic and
deterministic: a 0..1 grey ramp, RGB bars, a checker and a fake "ER HUD" bar.

| Case | Pass criterion |
|---|---|
| T1 white hot | every pixel outside the 4:3 box equals `thermal::shade_pixel` within 1 LSB (10-bit on R10G10B10A2, 8-bit on R8G8B8A8) |
| T2 black hot | same, inverted vision |
| T3 overlays 105/40/25 | inside the box vs the CPU reference (BC3 decode + D3D bilinear with 8-bit fraction): ≤ 3 LSB with ≤ 0.5 % outliers at edges; alpha>0 coverage within 0.1 % of the reference; no pixel outside the box changed by the overlay |
| T4 grain on/off | difference mean/std equal to the CPU expectation within 1 % |
| T5 reload blink | overlay alpha factor at 3 sample times matches `ac130_hud` |
| T6 resize | 1920×1080 -> 3840×2160 -> 1920×1080 with 3 frames submitted without waiting: no GBV error, correct output after each resize (SRV ping-pong + retirement) |
| T7 formats | R8G8B8A8_UNORM and R10G10B10A2_UNORM both pass T1 |
| T8 state proof | after `record`, the hudhook-style RT->COMMON barrier gives no debug error; a deliberately unbalanced variant (test-only flag) does give one, which proves the check bites |
| T9 failure path | wrong format / missing PSO: `record` returns an error and records **no** command (list command count unchanged) |
| Tperf | `vmtest --perf` thermal at 3840×2160 R10G10B10A2, 320 frames, first 20 dropped: **p95 < 1.0 ms and median < 0.3 ms** (written to `perf.json`) |

Outputs `work\m4\thermal_*.png` are for eyeballing. `--thermal-image <png>` runs the pass on a user screenshot.
Existing D/W/O/F/U/A cases must still pass (check_m3).

### 14.3 Regulation and the M2 pin

- `check_m3b.ps1` check 8 currently pins 13 ids and the M2 package sha (8f376e5a…). M4 changes the regulation on
  purpose, so this check is **renamed and replaced, not loosened**: "M2 rows unchanged". It requires:
  - the 13 M2 ids present;
  - each of them decoding byte-identical to the M2 build;
  - the M2 package's untouched params identical.
- The new package sha is pinned in `check_m4.ps1` (`$PackageSha`, recorded on the first green build and written to
  MODLOG).
- `check_m2.ps1` checks that count rows (check 14 "ids consistency") read the count from `rows.json` and accept the
  `ac130` block. The implementer verifies that each still FAILs on its own self-test mutations.

### 14.4 `tools\scripts\check_m4.ps1` (PowerShell 5.1; never launches anything; writes only `work\check_m4\` + what sub-checks write)

Same structure and helpers as `check_m3b.ps1` (`Add-Result`, `Invoke-Tool`, `Run-Check`, `-AllowSkip`; any SKIP
without `-AllowSkip` = non-zero exit). PATH for cargo/python is set at the top:

```powershell
$env:Path = [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
```

| # | Check | Pass criterion |
|---|---|---|
| 1 | M3b suite | `check_m3b.ps1` exit 0, 8 PASS (which includes check_m3 18 and check_m2 21) |
| 2 | cargo build | `cargo build --release --workspace` (incl. `sndconv`), 0 warnings under `mod\` |
| 3 | cargo test | `cargo test -p mw2er-core` passes, with ≥ 1 test in every 14.1 category (the test names are matched by prefix); `mw2er` lib test `game_write_fast_and_safe` still passes |
| 4 | converter | `convert_ac130.py --check` exit 0 with every known answer of section 13; `--self-test` catches all 5 mutations; `manifest.json` hashes match |
| 5 | regulation | `build_regulation.py` build + round trip: 3 edited params differ only by the added rows (15 new, sorted, in 79000200-79000499 / 79000950); the 13 M2 rows identical; every new row = clone + exactly the documented overrides; safety asserts (selfTarget/friendlyTarget 0, isHitBothTeam 0, opposeTarget 1, SpEffect cuts 0, noDead 1); `--self-test` catches 11 mutations; package sha == `$PackageSha` once recorded |
| 6 | rows consistency | `rows.json` ac130 block == `ac130_dmg` values from `weapons_ac130.json` and k (Rust real-data test + Python), radii == MW2 radius × 0.0254 (±1e-4) |
| 7 | vmtest T (debug layer) | T1-T9 PASS, 0 InfoQueue errors |
| 8 | vmtest T (GBV) | T1-T9 PASS under GPU-based validation, 0 errors |
| 9 | thermal perf | `vmtest --perf` thermal p95 < 1.0 ms and median < 0.3 ms at 3840×2160; the viewmodel perf from M3 is unchanged (< 1 ms) |
| 10 | config | `mw2er.toml` parses with tomllib and the DLL parser; `[ac130]`, `[ac130.audio]`, `[regulation.ac130]`, `keys.ac130*` present with the documented defaults; an unknown `[ac130]` key is rejected |
| 11 | wiring | `m2_static_checks.py ac130wiring --self-test` (section 14.5) |
| 12 | DLL proof strings | the built and staged `mw2er.dll` are identical and contain: `AC130 on`, `AC130 off reason=`, `AC130 blocked reason=`, `PROTECT apply`, `PROTECT restore`, `PROTECT cleanup`, `field=CSCamera.pers_cam_1+0x10..0x54`, `field=CSFeManImp.hud_state(+0x78)`, `AC130CAM`, `AC130 SHOT`, `AC130 EXPLODE`, `src=ac130`, `AC130 gpu_ms`, `ac130=`; the DLL does **not** import or contain symphonia (no `symphonia` string) |
| 13 | analyzer | `analyze_log.py --self-test` exit 0, including the M4 fixture: the good ride PASSes; mutated rides FAIL (missing PROTECT restore, player_hp_min < start, missing camera restore, gpu p95 1.4 ms, PERF ac130=1 avg 30, 40 mm interval 0.2 s); older fixtures give N/A for the AC-130 rows |
| 14 | hygiene | `converted\ac130` and `dumps\ac130` are gitignored (`git check-ignore` semantics, emulated if git is absent); no line of `_ac130.gsc`, `ac130.vision` or `ac130_inverted.vision` of ≥ 20 non-space characters appears in any file under `mod\` or `tools\scripts\` (outside `dumps`/`converted`); no new file in the ER or MW2 install folders since the last check (mtime scan of top-level only, read-only) |

The final line is `M4 OFFLINE ORACLES: PASS` only when all 14 PASS.

### 14.5 Wiring checks (`m2_static_checks.py ac130wiring`; each self-test mutation must FAIL)

1. The mode-2 detour path calls `game::write` exactly once and calls no `VirtualQuery`, `WriteProcessMemory`,
   allocation, logging or lock.
2. `WriteProcessMemory` is still absent from the crate. `VirtualQuery` appears only inside the page-cache check in
   `game\mod.rs`.
3. `apply_speffect`/`remove_speffect` appear only in `ac130.rs`, in functions reached from the task tick, after
   the `WRITES_ALLOWED`/version-gate check.
4. Every exit reason in `ac130_logic::Reason` reaches `end()` (one match, no `_ =>` arm).
5. `ThermalPass::record`: no `?` or early return between the first `ResourceBarrier` and the second.
6. `PRE_IMGUI` is set once (`viewmodel.rs`), and the dispatcher calls the thermal path only under the snapshot gate.
7. The hook install condition includes `[ac130] enabled`.
8. `ChrCam`/follow-cam writes are unreachable in mode 2 unless `sync_chr_cam`.
9. `hud_state` is written only through `game::write`, and its restore is guarded by "still 0".
10. The erfps2 and fromsoftware-rs attributions are present.
11. `symphonia` appears only in `mod\sndconv\Cargo.toml`.

---

## 15. In-game test steps (user run; HDR off; me3 via `launch.ps1`)

1. Launch, load in, stand still 10 s with MW2 off. Note `PERF ticks_per_s` (about 55-60). Expect
   `AC130 cfg …` and `AC130 rows ok bullet=7 atk=7 speffect=1` once.
2. Press F7 with MW2 off: `AC130 blocked reason=mw2_off`. Press F5 (MW2 on); first person comes on as before. The
   HUD shows `AC-130 [F7] READY`.
3. In the open (Limgrave near Godrick Soldiers), press F7. Expect, in this order:
   - `AC130 PROTECT apply … present=1 ok=1`;
   - `FP off reason=ac130`;
   - `AC130 on …`, `AC130CAM write …`, `AC130 write field=CSFeManImp.hud_state …`.
   
   The screen fades in from black to a grayscale thermal view from high above, slowly orbiting, with the 105 mm
   overlay and grain. ER's HP bars are gone.
4. Move the mouse: the view turns, limited to about 35° each side. Look for `AC130CAM … mouse_counts` growing,
   `er_yaw` constant, `persisted=0`, `skips=0`. The character stays still.
5. Fire the 105 at an enemy group: a heavy boom, an ER fireball about 2 s later, enemies die. Look for
   `AC130 SHOT gun=105`, `AC130 EXPLODE gun=105`, `AC130 EXPLODE seen id=79000201`, then `HIT … src=ac130 gun=105`
   and `AC130 KILL`. Hold LMB: one shot every ~5.1 s with `RELOAD` lines.
6. Wheel or Q to the 40 mm (overlay changes, beep, narrower FOV): bursts of 4 then a 3 s pause. Then the 25 mm:
   20 fast rounds then 1.5 s, with small bursts on hit. Press 1 to go back to the 105.
7. Press F: black hot (0.6 s transition). Press F again: white hot.
8. Let enemies reach you. Fire at your own position with the 105: player HP does not change. The `AC130 off` line
   must show `player_hp_min == player_hp_start`.
9. Let the timer run out: `AC130 off reason=timeout t_s=40.0 …`, then `AC130 restore field=CSCamera…` (ok or
   `skipped=er_rebuilt`), `restore … hud_state … ok=1`, `PROTECT restore … present=0 ok=1`. Third-person ER camera
   and HUD come back. About 0.5 s later first person re-arms and the indicator shows `AC-130 1:59`.
10. Set `cooldown_enabled = false` (hot). Ride again and exit early: F7 (`reason=exit_key`), then once each with
    Esc (`reason=menu`), F5 (`reason=f5`) and, if easy, die by jumping off a cliff after exit or let the timer run
    in a fight (`death` is unlikely while protected; skip it if not reachable). Each gives a matching restore set.
11. Inside a cave or legacy dungeon, press F7: `AC130 blocked reason=no_sky`.
12. While in the gunship, check `PERF … ac130=1 ticks_per_s` is near the step-1 value and `AC130 gpu_ms p95` is
    under 1.0.
13. Fast travel during the cooldown (load): no stray protection (`AC130 PROTECT cleanup` must not be needed).
    Quit, run `python -I tools\scripts\analyze_log.py`. All AC-130 rows must PASS, plus the existing rows.

Visual oracle (the final one, by the user): thermal grayscale with the MW2 overlay centred, the view orbiting
counter-clockwise or clockwise (either is accepted; `orbit_dir`), explosions visible and lethal, no ER HUD, and
correct ER camera/HUD/first person after each exit.

---

## 16. What stays unverified until the user's run

- Whether the 158 m camera renders acceptably: LOD, foliage, culling from `pers_cam_1` versus ER's own
  third-person camera. Aim 35° away from ER's view and look for popping. Fallbacks are in 4.7.
- Whether ER rebuilds rows 0-2 and fov every frame (`persisted=0`), and whether `pers_cam_1.fov` is vertical
  (`debug_markers` dots on enemies at the screen edges).
- Whether ER reads mouse axes only through `GetDeviceState` (`er_yaw` constant). The gamepad is not blanked.
- `map_ray` at 150-300 m from a camera far from the player. The ray ends near the player, where collision is loaded.
- Whether the explosion rows deal damage as cloned:
  - AoE from a stationary spawned sphere;
  - the `dmgHitRecordLifeTime` behaviour;
  - the additive tiers (measured core/ring totals);
  - the SFX ids showing from altitude (they may be distance-culled).
- SpEffect 79000950: the 100 % cut, `noDead`, no stagger and removal (the `entries()` scan proves presence, and the
  player HP proves the effect).
- `CSFeManImp.hud_state`: whether it hides the HUD and whether ER rewrites it.
- Thermal cost on ER's real frame (estimate 0.08-0.15 ms) and how close the look is to MW2. No MW2 AC-130
  screenshot exists. Request: the user takes one in MW2 for a side-by-side.
- ER's audio listener at the gunship camera (inferred from erfps2).

---

## 17. Decisions the implementer must not change (non-negotiables)

1. Never launch ER, MW2, me3 or Steam. Never write into a game folder or `%APPDATA%\EldenRing`. Never publish or
   push. Do not touch `D:\Mods\MW2inER-m3`. Converted assets only in `converted\ac130\`; dumps only in `dumps\`.
   Downloads only via cargo/crates.io (`symphonia`, exact version, `sndconv` only).
2. Every runtime memory write goes through `game::write`: page-cached check + `NtWriteVirtualMemory`, version-gated.
   The writes are the camera (17 floats), `hud_state`, and `ChrCam` only if `sync_chr_cam`. Never call
   `WriteProcessMemory`, and never call `VirtualQuery` per write or per frame. Each write is logged once per ride
   with field, address, old and new, and restored only if the field still holds our value.
3. The camera write is **one** 68-byte write per frame, from the existing camera-step detour (mode 2). No task-side
   per-frame writes, no new hooks for the camera. ChrCam, ChrExFollowCam and CAM_WALL_RECOVERY are untouched in
   mode 2 by default. The task must use `er_view_matrix()` substitution so the gate and aim never see the gunship
   camera.
4. A ride is never Active without SpEffect 79000950 confirmed present in `special_effect.entries()`. It is removed
   and confirmed absent on every exit path. A stray copy is cleaned up at every session start and after each load.
   `apply_speffect`/`remove_speffect` are called only from the task, after the version gate and the PARAMS check.
5. Every exit path goes through `end(reason)` with the fixed order of 2.4, and logs `AC130 off reason=`. Death,
   loading, menu, F5, exit key and timeout all end the ride.
6. The thermal pass validates everything before the first barrier, records the barrier pair balanced, leaves the
   back buffer in RENDER_TARGET, never holds a back-buffer reference, ping-pongs the shader-visible scene SRV on
   resize, and disables itself (not the gunship) on any format or output problem. Budget: p95 < 1 ms at 4K
   (vmtest).
7. Regulation: only the 15 documented rows are added, sorted, in the free range. Every field differs from its clone
   only as listed in 8.3-8.5. Damage = MW2 numbers × k through the one shared formula. The safety fields
   (`selfTarget 0`, `friendlyTarget 0`, `isHitBothTeam 0`, `opposeTarget 1`) are asserted by the build. The 13 M2
   rows stay byte-identical.
8. Gameplay numbers (stats, timings, vision values, HUD layout and strings) come from `converted\ac130\*.json` built
   from the user's dumps. Nothing from `.gsc`, `.vision` or `.menu` is embedded in source; config holds overrides
   only.
9. Pure logic lives in `mw2er-core` (`forbid(unsafe_code)`) with the tests of 14.1. `check_m4.ps1` must end in
   `M4 OFFLINE ORACLES: PASS`, with `check_m3b`/`check_m3`/`check_m2` all PASS and `cargo build` at 0 warnings.
10. The AC-130 requires MW2 mode. First person is handed back with reason `ac130` and re-arms normally afterwards.
11. erfps2 and fromsoftware-rs remain reference/vendored. RVAs are attributed in source and MODLOG; no code is
    copied. OAT's Unlinker is an external tool, never linked or copied.

---

## 18. Review changes (2026-10-17, after the M4 review)

Labels as in the rest of this document. Nothing here has run in Elden Ring yet.

| Finding | Change | Label |
|---|---|---|
| major 1 camera restore all-or-nothing | Per-float save/restore/substitution (`ac130_cam::merge_er`, `restore_plan`, `substitute_er`, 6 tests); `AC_FOV_PERSISTED` counter, `fov_persisted=` in AC130CAM, `floats= fov_restored=` in the restore line; analyzer reports it (INFO) | V offline; I whether ER rebuilds the fov |
| major 2 permanent protection | SpEffect 79000950 `effectEndurance` = `protect_endurance_s` (90 s, 30..600; build + PARAMS check refuse -1; regulation self-test 12/12 incl. "permanent protection"); `ac130_protect::refresh_due` re-applies 20 s before expiry (a default 40 s ride never refreshes; `AC130 PROTECT refresh` line); after a task panic `run_tick` keeps calling the guarded `Ac130Ctl::disabled_tick` every tick (acts every 30 ticks only while something is pending): removes / verifies the protection (`PROTECT restore ... reason=panic (verified after ...)`) and gives back a deferred ER HUD value | V offline; I whether ER resets the timer or stacks a 2nd entry on re-apply (both safe) |
| minor 3 ER HUD left hidden out of world | The HUD restore no longer depends on in_world (CSFeManImp is persistent); unreadable -> kept as `hud_pending`, retried every Idle tick and by `disabled_tick` | I |
| minor 4 Esc under the capture | The input detour records raw `DIK_ESCAPE` edges (`ESC_PRESSES`); an Esc edge during Starting/Active ends the ride as `menu` (`AC130 esc pressed ...`) | V wiring; I in game |
| minor 5 dropped AmbientStop | The ambient is state, not messages: `AMBIENT_WANTED` / `AMBIENT_GEN` / gain / fade atomics polled by the audio thread every <= 20 ms; the AmbientStart/Stop messages are gone | V |
| minor 6 double FP reads | `read_player_fp` / `read_world_fp` once per tick in `TaskState::tick`, passed to both controllers (`Ac130In.fp_player/fp_world`, `TickIn.player_fp/world`); their time is added to PERF `fp_us` | V |
| major 7 analyzer passes a harmless ride | `ac130 explosions` FAILs with 0 `HIT ... src=ac130` with dmg > 0, parses `AC130 KILL gun= cid= tier=` (kills per gun), FAILs a KILL without a preceding `HIT ... kill=1 src=ac130` for the same cid; the DLL now logs the KILL line right after its HIT; 3 new self-test mutations (12 M4 mutations in all) | V |
| major 8 white-hot washout | `[ac130] exposure` (default 0.6 [I]) scales the luma weights = an exact pre-exposure for desaturation 1 (all four MW2 visions); `thermal::exposed_luma`, `clip_fractions` (test); `vmtest --thermal-image <png> [--exposure x]` prints an exposure sweep of clipped black/white and GPU == CPU at the configured value; T10 runs the film on a real ER frame `work\m4\er_frame_ref.png` (limits: white <= 10 %, black <= 10 %). No real ER screenshot exists offline yet (the only recent screenshots on this PC are of another game), so T10 is NEEDS-DATA and check_m4 check 15 reports it; the 0.6 default is [I] until then | I look; V math |
| minor 9 typed FOVs / MW2 fallbacks | `fov_105/40/25` default 0 = `weapons_ac130.json ads_zoom_fov` (`Ac130::resolve_fovs`, tested); geometry/fov without the converted data -> no ride (gate `rows_missing`), no typed 116/107/70/35/40 fallbacks. adsZoomFov as the gunship zoom stays [I] (aimDownSight 0 in all three dumps) | I |
| minor 10 reload timing | Reload from the shot (gsc), tests and analyzer target = reloadTime | V gsc reading |
| minor 11 25 mm burst SFX | `sfx_25 = -1` (MW2 has no 25 mm explosion effect); package re-pinned | V dump |
| minor 12 cooldown on aborted start | Only a ride that reached Active charges the cooldown (`EndPlan.cooldown_s`; `off ... cooldown_s=` logs the charged value); test `ac130_logic_aborted_start_charges_no_cooldown` | V offline |
| minor 13 weak install baseline | check 14 compares the top level with the checked-in `tools\scripts\fixtures\install_baseline.json` (reviewed vanilla listing) and scans both folders recursively for dinput8 / mw2er* / me3* / *.me3 / modengine* / regulation.bin.* / a second regulation.bin | V |

### 6.4a Pre-exposure [I]

MW2's film tweak ran on a thermal-material render; here it runs on ER's lit colour frame. `exposure` scales `g`
before `1.9 g + 0.05`, so with 0.6 every pixel with luma < 0.83 keeps a grey level (at 1.0 everything above 0.5 is
white). The shader and the root-constant layout are unchanged: the DLL passes `luma_weights x exposure`.

## 19. Run-8 changes (2026-10-17, after the first in-game ride)

Supersedes the numbers of 8.2/8.4 and 6.4a where they differ. Labels as above.

- **8.4 damage path [V log, I fix].** A bullet spawned with behavior/goods/magic = -1 is a weapon attack: ER computes
  damage from the player's weapon AR x `atk*Correction` and ignores `atkPhys` unless `isAddBaseAtk = 1`. Evidence: the
  UMP45 bands 79000100..105 (atkPhys 160..140, Throwing Dagger clone, correction 120) all did 49 on c4311 since run 2
  (the ER curve predicts x1.25 between them; MODLOG M3b), and the run-8 Fire Pot clones (correction 100, atkPhys
  650..2100) did 28..44 per sphere unguarded and 2..4 on a guarding soldier. The AC-130 AtkParam rows now use the
  vanilla flat-damage pattern of weapon-skill bullets (AtkParam_Pc 30000841, 30300870): `isAddBaseAtk 1`, every
  `*Correction` 0, `isDisableBothHandsAtkBonus 1`, `spEffectAtkPowerCorrectRate_byPoint 0`, plus `disableGuard 1`
  (no shield block for a shell from above). Built rows keep the Fire Pot clone otherwise (sphere, SFX, penetration).
  The build and its self-test assert the set (14/14 mutations). The UMP45 rows are unchanged (byte-identical pin):
  they still deal weapon-AR damage (noted, out of scope).
- **8.2 k [I].** `atkPhys = round(mw2 x k x scale_<gun>)`, k = 0.65, scale_105 = scale_40 = 1, scale_25 = 0.7 (rows.json
  `gun_scale`, `atk_mode = "flat"`; Rust `RowsAc130::k_gun`). Rows: 105 293/341/488, 40 146/106/179, 25 116. Predicted on
  c4311 (def ~110, community curve): 105 inside the core 461 (ring only 253), 40 direct 115 (ring only 40), 25 49 per hit.
- **6.4a gamma [I].** `[ac130] gamma` (0.75) lifts the film input after the exposure: `g = (exposure x luma)^gamma`
  (`thermal::lift`, shader `c8.x`, 36 root constants). Default exposure 0.52: on the vmtest card mid-tones 0.302 ->
  0.422 at a white clip of 15.3 % (old 0.6/1.0: 10.4 %; 0.8/1.0 would clip 34 %). vmtest T11 checks GPU == CPU.
- **Tracers (new) [V offline, I look].** `mw2er_core::ac130_tracer` (ring of 64 shots, flight interpolation,
  projection = `ac130_hud::project` with the camera rows/pos/fov the detour writes to pers_cam_1, near clip 0.5 m,
  muzzle offset (-1.5, -2.5, 0) m in camera space) and `mw2er_render::tracer::TracerPass` (`shaders/tracer.hlsl`:
  screen-space capsules + impact flashes, additive, one root SRV into a persistently mapped upload ring of
  num_frames x 64 x 32 bytes, no barrier, no per-frame allocation). Recorded BEFORE the thermal pass so the film shows
  a round white hot in WHOT and black in BHOT under the same grain and overlay. `[ac130.tracers]` (hot): width/length/
  brightness/flash per gun. vmtest `--tracers` TR1-TR4 + `--tracers-perf` (check_m4 check 16).
- **HUD panic [V].** `draw_gunship` took a second `get_background_draw_list()` while `Mw2Hud::draw` held one: the
  helper now borrows the frame's single `DrawListMut`; wiring check 12 + the mw2er lib test
  `hud_gunship_frame_uses_one_background_draw_list` keep it that way.

## 20. Run-9 effects (2026-10-17): object destruction, impact SFX, camera shake, bigger 105 flash

User request after run 9. Supersedes 8.3/8.4 (sfx, atkObj) and the shake note of 7.4. Labels as above.

- **Object destruction [V data, I engine rule].** ER assets (AssetEnvironmentGeometryParam) carry `hp` and `defense`;
  Paramdex's wiki of `isBushesForDamage` says assets are damaged by "object attack power > defense power", i.e.
  AtkParam `atkObj` vs asset `defense` [R]. Vanilla 1.17.1 (2,968 damage-breakable asset rows, hp > 0): defense 0
  (1,983), 1..9 (41), 90 (870), 95 (3, caravan wagons AEG100_100/103/120), 199 (37), 299 (6), 990/999 (28). Vanilla
  atkObj: AtkParam_Pc max 100 (11 rows; heavy swings 60/80, Fire Pot [Hit] 10030001 and every pot/explosive bolt/
  Meteorite/Rock Sling 10, ballista 20), AtkParam_Npc 100 (5,335) and 200 (529, the heaviest boss/giant attacks).
  The 63 assets with `isBushesForDamage = 1` also need `isEnableCalcDamageForBushesObj = 1` on the attack (304 NPC
  rows, e.g. dragon fire 4500100, all with atkObj 100; no player row). Bullet `isPenetrateObj` (clone 1) only decides
  whether the sphere stops at an asset; `mapHitType` stays the clone's 2. New AtkParam_Pc values
  (build_regulation.py `AC130_OBJECT`; was 100/50/100, 30/20/30, 10):

  | row | atkObj | bushes | breaks (defense <) | assets broken of 2,968 |
  |---|---|---|---|---|
  | 105 core / single | 200 | 1 | 200: everything any vanilla attack breaks | 2,934 |
  | 105 ring | 100 | 1 | 100 (barricades, wagons) | 2,897 |
  | 40 core / single | 100 | 1 | 100 | 2,897 |
  | 40 ring | 60 | 0 | 60 (crates, barrels) | 1,961 |
  | 25 | 40 | 0 | 40 | 1,961 |

  The 6 defense-299 and 28 defense-990/999 rows that no vanilla attack breaks stay intact (no event or progression
  asset is opened by a shell). The build asserts `atkObj > ` the defence each tier must beat and the bush flag, and
  re-derives the broken counts from the vanilla param (`objects [I]` line; check_m4 check 5). Self-test +2 mutations.
  The DLL PARAMS check compares atkObj / bush flag live (`AC130 rows ok ... obj=201:200b,...`). Asset breaks are not
  logged (no per-asset hook): verify by eye in the run.
- **Impact SFX [V ids, I look].** ER SFX ids from vanilla PLAYER Bullet rows (common effects, so loaded everywhere;
  character-specific 6xxxxx boss effects were rejected): the core/single row shows the burst, the ring row (spawned
  on the same spot) the dust/debris layer, so one impact shows both without an extra bullet.

  | row | sfxId_Bullet | source row |
  |---|---|---|
  | 105 core/single | 524028 | Giantsflame Take Thee [Charged] 10610051 (r 3.5 m; the uncharged 524023 was used to run 9) |
  | 105 ring | 530123 | Meteorite impact 10470003 (rock + dust) |
  | 40 core/single | 300602 | Explosive Stone [Hit] 10183001 (unchanged) |
  | 40 ring | 530133 | Rock Sling 10471009 sfxId_Hit (rock impact, dirt) |
  | 25 | 530133 | the same small dirt hit (was -1) |

  All seven rows set `sfxPostureType = 1` ("Global Y Axis", as the Meteorite impact) so the effect stands upright
  whatever the spawn direction (the clone's 0 = parent). `bulletSfxDeleteType_byLifeDead` stays 0 (emitter stopped:
  the particles outlive the 0.07-0.1 s sphere). Config `[regulation.ac130] sfx_105 / sfx_105_ring / sfx_40 /
  sfx_40_ring / sfx_25`. Self-test +2 mutations (ring sfx -1, posture 0); the PARAMS check compares sfx/posture live.
- **Camera shake [V numbers, I model].** `mw2er_core::ac130_shake` (5 tests) + `[ac130.shake]` (hot). Numbers from
  `_ac130.gsc` via convert_ac130.py 1.1 (timing.json now keeps radius_in and origin): fire 105 0.2 / 1 s, 40 0.1 /
  0.5 s on the plane (radius 1000 in); impact 105 0.4 / 1 s (radius 3500 in = 88.9 m), 40 0.2 / 0.5 s (2000 in);
  the 25 mm has none. Amplitude `scale x [ac130.shake] scale x gain`, linear decay over the duration, the strongest
  live quake drives (4 slots, no stacking); yaw/pitch `a x rot_deg` (2.0), roll `a x roll_deg` (0.6), camera offset
  `a x pos_m` (1.5 m) of smooth seeded noise at `freq_hz` (11). `mw2_falloff = false` by default: MW2's radius falloff
  would mute the impact quakes at the gunship (slant 158 m > 89 m), which is not the feel asked for. Applied in the
  mode-2 detour (`ac130_cam::frame`: yaw/pitch before the rows, roll about the forward row, offset in camera space;
  det stays 1, check_frame unchanged). The shells leave along the UNSHAKEN view (`FrameOut.aim_fwd/aim_pos`,
  `cam_hook::gunship_aim`), the tracer muzzle and projection use what is on screen. Log: `AC130 effects shake=...` at
  the ride start and `AC130 SHAKE src=fire|impact gun= ... amp= on=` per quake; analyzer row `ac130 effects`
  (6th AC-130 row, N/A for older logs, 4 mutations). The old `[ac130] shake_scale` still parses and multiplies.
- **Bigger 105 flash [V offline, I look].** `[ac130.tracers]` per gun `flash_s_<gun>` (0 = flash_s), `flash_gain_<gun>`
  and `bloom_<gun>` (a second, wide soft flash instance at 0.35 x the flash brightness, only when the 64-instance list
  has room). Defaults: 105 flash 14 m (was 6) / 0.5 s / gain 1.6 + 32 m bloom; 40 2.5 m / 0.3 s + 7 m bloom; 25 as
  before. vmtest TR1 now expects flash + bloom + 2 streaks (centroid 0.44 px, frame == CPU); 4K perf unchanged.
- `[ac130] cooldown_enabled` is false in mw2er.toml by the user's choice (the Rust default stays true); check_m4
  check 10 accepts the user's value.
