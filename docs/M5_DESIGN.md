# M5 design: MW2 movement (walk, strafe, timed sprint, jump, crouch, prone) while MW2 mode is on

Status: design, 2026-10-17. Nothing here has run in game. Sources:
- the three research findings of this workflow: `mw2-numbers` (static read of the user's MW2 rawfiles, the ump45_mp
  weapon file and a read-only disassembly of `iw4sp.exe`), `er-physics-path` (vendored fromsoftware-rs 59fbd3b,
  offsets compiled with `offset_of!`, the CS2-conversion knowledge note) and `input-and-anim` (MW2 and ER key
  bindings, viewmodel fields, bob dvars);
- `MODDING_PLAN.md`, `MODLOG.md` (runs 1-10), `docs/M2_DESIGN.md`, `M3_DESIGN.md`, `M3B_DESIGN.md`, `M4_DESIGN.md`;
- the current sources `task.rs`, `first_person.rs`, `cam_hook.rs`, `input_hook.rs`, `game/havok.rs`,
  `game/fp_read.rs`, `mw2er-core` `input_logic.rs`, `vm_anim.rs`, `spread.rs`, and `mw2er.toml`.

The baseline is run 10, verified in game. It covers F5 MW2 mode (UMP45, engine bullets, MW2 viewmodel with sprint and
ADS animations), F6 first person through the camera-step hook (eye = physics position + 1.60 m, body hidden, yaw
follow as one 32-byte write), and the F7 AC-130. PERF runs at about 60 ticks/s. Movement today is Elden Ring's own
root-motion movement. The user says it "feels a bit off" and wants MW2 movement with sprinting.

User request this design implements: **MW2 movement with sprinting, in MW2 mode, with first person on or off.**

## Evidence labels

Same labels as M2-M4:

- **[V] verified**: checked on this PC by a file read, a parse, a disassembly, a compile, a log line or an offline
  run.
- **[R] reported**: another project's source, a crate doc comment or the CS2 knowledge note says so. Not tested here.
- **[I] inference**: our design choice or a guess. Each one has a log line or an oracle that settles it.

Rules that bind this milestone:
- Never launch Elden Ring, MW2, me3 or Steam. `iw4sp.exe` is read only as a file (static string, data and
  disassembly reads). It is never run, attached to or loaded.
- Never write into a game folder or `%APPDATA%\EldenRing`. Never push or publish. Never browse the user's personal
  folders. Do not touch `D:\Mods\MW2inER-m3`. Downloads only through cargo from crates.io.
- Every runtime write goes through `game::write` (page-cached check + `NtWriteVirtualMemory`, version-gated). It is
  logged once per episode with old and new values, and restored on every exit path. Section 4.4 names the one write
  that is never restored (fall damage to HP) and explains why.
- Numbers from `.cfg`, `.gsc` and the weapon file are converted into `converted\movement.json`. No line of those
  files is embedded in source.

---

## 0. Summary of the decisions

| Topic | Decision | Label |
|---|---|---|
| Scope | A kinematic MW2 controller drives the player whenever MW2 mode is on, in first **and** third person (`fp_only = false`). It hands control to ER for ladders, Torrent, throws/grabs, interactions, knockback, death, menus, cutscenes, loading and the AC-130 | user choice; I |
| Model | Pure IW4 pmove port in `mw2er-core` (`pmove`, `sprint`, `stance`, `fall`), run in **IW4 units (inches, seconds, y-up)**. Only the I/O boundary converts with ×0.0254. All MW2 numbers are therefore compared 1:1 with the disassembly | V numbers; I port |
| Where it runs | In the existing `ChrIns_PostPhysics` task, after the reads and handback checks, before the HUD snapshot. A new frame-order probe confirms the order (section 1.1) | V task exists; I order |
| Position write | `CSChrPhysicsModule.position` (+0x70, 16 B, w kept) plus `chr_proxy_pos_update_requested` (+0x91) = 1 every driving tick. Facing goes through the existing yaw pair (+0x50, 32 B) in first person only | V offsets compiled; R CS2; I that it sticks |
| Camera | The task publishes the controller's feet and the stance eye height in atomics. While the controller drives, the camera-step detour uses them instead of `physics.position` (so the eye is not one ER displacement behind), only if they are within 1 m of `physics.position` | I |
| Gravity / fall death | While driving: `gravity_multiplier` (+0x1cc) = 0 and `gravity_disabled` (+0x1d5) = 1, saved and restored on every handback, re-asserted when ER clears it. The controller does gravity (800 in/s²). Fall damage is ours, from MW2's 200/350 in thresholds | V offsets; R CS2 (the only fix that worked); V MW2 numbers |
| Collision | Only `CSPhysWorld::cast_ray` exists (hit point, no normal, no shape cast). A ray "capsule": 3 heights × 3 lateral offsets along the move, a ground ray with an optional 3-ray normal, a step-down ray, a headroom ray, a ceiling ray. Q3 clip-and-slide with ≤ 3 bumps, MW2 step 18 in (10 prone), walkable normal y ≥ 0.7. Budget `max_rays_per_tick` (32), measured in PERF | V API; I method |
| Things rays miss | Doors, objects and characters may not be on map filter 0x2000058. Two remedies: a pure circle push-out from nearby characters (`read_nearby`), and ER's own proxy push-out read back as `er_delta` (only the component against our motion, clamped to 0.3 m) | I |
| Warps | `|P_er − P_w| > 5 m` in one tick. `ChrIns.chunk_position` (+0x80) continuous → **rebase** (translate, keep velocity). Otherwise **warp** (resync, zero velocity, end sprint). Logged `MOVE resync why=` | V offset; I semantics |
| Sprint | MW2 rules from `PM_UpdateSprint` (V): **press Shift to start, releasing keeps sprinting, press again to stop**; 4 s × sprintDurationScale (1); 1 s minimum to start; 1:1 recharge; ×1.5 speed; strafe input ×0.667; ends on fire, ADS, reload, jump, crouch, prone, forward ≤ 105/127 (S or strafe-only), meter empty. Starting from crouch/prone stands you up if there is headroom. `sprint_mode = "hold"` is an option | V |
| Jump | 39 in (0.991 m) exactly, v0 = 249.8 in/s, ≥ 500 ms between jumps, air accel 1, apex clamp as `Jump_ClampVelocity`. Landing > 12 in: velocity ×0.67. `jump_slowdown = true` (MP default inferred): ×0.65 for 1800 ms (or ×0.5 / 1200 ms) and a land factor on the next jump | V numbers; I MP default |
| Stances | Stand / crouch / prone, eye 60/40/11 in via the exact MW2 curves, transitions 200/200/400/400 ms, hull 70/50/30 in, headroom checks. Speeds ×0.65 / ×0.15. Accel 9 / 12 / 19. Default eye mode `ratio` keeps the user's 1.60 m stand eye (crouch 1.067 m, prone 0.293 m) | V numbers; I ratio |
| Keys | MW2 PC defaults (V, `default_mp_controls.cfg`): W/A/S/D, **LShift** sprint, **Space** jump/stand up, **C** toggle crouch, **LCtrl** toggle prone. All configurable; `crouch_mode = "hold"` and a console-style `stance_hold` key (300 ms, `cl_stanceHoldTime`) are options | V |
| ER keys blanked | While the controller drives: Space (ER dodge/dash), LShift (ER modifier), C, LCtrl, F (ER jump), X (ER crouch), Q (ER lock-on), LAlt (ER walk). Latched until physical release. W/A/S/D are blanked in first person only and never latched. E (ER interact) and the mouse wheel pass | V ER params (presets B/C); I live preset → `MOVE keyprobe` |
| Third person | **Hybrid**: ER sees W/A/S/D, so it plays its own walk/run animation and turns the body toward the move input. We overwrite the position every tick, so ER's root motion is discarded. `tp_anim = "er"` hands third person back to ER movement completely | I (decision, section 7) |
| Viewmodel/HUD | The sprint anims are driven by our sprint flag (not by ER speed). Fire or ADS during sprint ends it, plays sprint_out 0.3 s, then fires or aims. Crouch/prone gun offsets come from the weapon file. MW2 per-stance bob. Hip spread uses stance min/max and jump_spreadAdd. Small stance icon | V fields; I blending |
| Data | `tools\scripts\convert_movement.py` writes `converted\movement.json`: every constant with value, unit, source and label, plus the UMP45 movement fields, eye curves and MW2 binds. With no data file the controller stays off (`MOVE blocked reason=no_data`); there are no typed fallbacks | V sources |
| Oracles | `mw2er-core` tests (section 13.1), the headless bench `movebench --check`, converter known answers + provenance via the static disassembler, analyzer rows, wiring checks, and `tools\scripts\check_m5.ps1`, which runs `check_m4.ps1` first | section 13 |

---

## 1. Controller architecture

### 1.1 Frame order and where the controller runs [V enum order in `cs/task.rs`; I that it equals execution order]

```
PadStep 285 -> ChrIns_PreBehavior 347 (ER reads the pad -> action requests)
            -> HavokBehavior 350 (animation, root motion)
            -> ChrIns_PrePhysics 354
            -> HavokWorldUpdate_Pre/Post 373-375 (proxy synced to our last write, + ER root motion, push-out)
            -> CameraStep 382 (our camera-step detour: eye)
            -> ChrIns_PostPhysics 389 (OUR TASK: reads, handbacks, controller, writes, publish)
```

- The controller runs inside the existing task tick (task.rs, `CSTaskGroupIndex::ChrIns_PostPhysics`). It needs no new
  task and no new hook.
- Tick N reads ER's post-physics position `P_er(N)`, runs pmove and writes `P_w(N)`, with the proxy sync request.
  Physics N+1 syncs the Havok proxy to `P_w(N)` and adds ER's own displacement. CameraStep N+1 then builds the eye.
- **Camera fix (Fix A in the findings).** Without a fix, the detour would read `physics.position = P_w(N) + ER
  displacement`, which puts ER's root motion and push-out into the eye. While the controller drives, the task
  therefore publishes `P_w(N)` and the eye height in atomics (section 1.6). The detour uses them as long as they are
  within `publish_max_m` (1.0 m) of the live `physics.position`. That guard covers a rebase between the publish and
  the detour. The eye then matches the proxy position that the frame renders. Input latency is one frame, the same
  as ER's own.
- **Frame-order probe [I → V by log].** The task and the detour each bump a frame counter. The first 600 driving
  ticks log `MOVE order detour_before_task=<n>/<m>`, which must be about 100 %. If the order differs, the published
  position is still correct (it is our newest), and the line records the fact.

### 1.2 One driving tick (task thread, order fixed)

1. **Reads** (once per tick, shared with first person and the AC-130 as in M4 minor 6): `main_player`, physics module
   address, `P_er` (+0x70), `is_touching_ground` (+0x1d1), `standing_on_solid_ground` (+0x92),
   `touching_solid_ground` (+0x93), `gravity_multiplier` (+0x1cc), `gravity_disabled` (+0x1d5), `hit_radius` (+0x2f4),
   `ChrIns.chunk_position` (+0x80), HP and max HP (`CSChrDataModule`, hp +0x138, max_hp per the crate's
   `offset_of!`), the existing FP signals (death, ride, ladder, throw, talk, fade, camera_mask, load counter, cursor),
   and the new probe signals (section 9.2). Also the camera rows from `er_view_matrix()`, which are always ER's
   camera, never the gunship.
2. **Handback evaluation** (`move_logic`, section 9). If any trigger fires, nothing is written this tick except the
   restores (section 4).
3. **Resync**: rebase or warp classification (section 5.5), then the `er_delta` push-out (section 5.6).
4. **Input**: consume the edge counters and levels from the input hook (section 6.3) and build an IW4-style usercmd:
   `forwardmove`, `rightmove` in −127..127, and the buttons sprint, jump, crouch, prone, fire, ADS, reload.
5. **Pmove**: `n = ceil(dt / max_substep_s)` substeps (default 1/60 s; at 60 Hz one substep, at 30 Hz two). `dt` comes
   from the task's QPC clock, clamped to [0, 0.1] s; a longer hitch counts as 0.1 s. Each substep runs
   sprint → stance → friction → accelerate → gravity → collide/step → ground → land (section 2).
6. **Writes**: position (+ the optional `last_update_position`), the proxy sync flag, gravity re-assert if ER cleared
   it, and the yaw pair (first person only). See section 4.
7. **Publish**: feet, eye height (stance curve + land dip + bob), the active flag and sequence for the detour; sprint,
   stance, airborne and land events for the viewmodel, spread and HUD snapshot.

### 1.3 States (`mw2er_core::move_logic::MoveCtl`) [I, unit-tested]

| State | Meaning | Leaves to |
|---|---|---|
| `Off(why)` | MW2 mode off, `[movement] enabled = false`, no data, not in world, or `fp_only` with FP off | `Arming` when every condition clears |
| `Arming(n)` | Every condition clear and ER reports the player grounded (`is_touching_ground` or our down-ray within the step height) | `Driving` after `rearm_ticks` (10) clear ticks; any trigger → `Off`/`Suspended` |
| `Driving` | The controller owns position, velocity and gravity | `Suspended(reason)` or `Off(reason)` in the same tick as a trigger |
| `Suspended(reason, since)` | ER drives (ladder, Torrent, interaction, knockback, menu ...) and gravity is restored | `Arming` once the reason clears (with its hold time); `Off` on a user reason |

Entering `Driving`:
- Resync `pos = P_er`. The velocity is seeded from ER's last planar displacement ÷ dt, clamped to 285 in/s, and zero if
  ER was airborne.
- Save the gravity fields and write ours (section 4.2).
- Set the key-blank want (section 6.2).
- Log `MOVE on ...`.

Stance is kept only across `menu`; every other suspension resets it to stand (logged). The sprint meter is time-based
and survives everything (MW2 recharges while you do anything).

### 1.4 Units and axes [V scale; I axis convention]

- 1 IW4 inch = 0.0254 m. Internally the controller keeps `pos_in = P_m / 0.0254` with ER's axes (y up). No
  Z-up/Y-up swap happens anywhere, so handedness cannot flip.
- Wish directions come from ER's camera. `fwd_h` is the horizontal part of the camera forward (`er_view_matrix()`
  row 2 with the existing `aim.camera_det_sign` handling), normalised. `right_h` is the horizontal unit vector that
  projects to **+x on screen**. Which cross product gives it is fixed by a unit test that uses the projection
  verified in game for the AC-130 tracers (`ac130_hud::project`, run 9) and a real CAMFRAME line from the fixtures
  (section 13.1, `move_axes_*`). `strafe_sign` (1/−1) stays as a hot override, and the in-game check is step 3 of
  section 14.
- `forwardmove = 127·(W) − 127·(S)` and `rightmove = 127·(D) − 127·(A)` (keyboard, full scale, as IW4's `CL_KeyMove`
  [I]).

### 1.5 Data file `converted\movement.json` [V sources]

Written by `tools\scripts\convert_movement.py` (section 12). The DLL loads it at boot and on hot reload; if it is
missing or fails validation, the controller stays `Off(no_data)`. Each constant is
`{ "value": …, "unit": …, "src": "<file or exe VA>", "label": "V|I" }`. The Rust model is
`mw2er_core::movement_data`. The json also records the exe sha256 and the source file hashes.

### 1.6 Published state (task → detour; atomics only)

`MOVE_ACTIVE: AtomicBool`, `MOVE_FEET: [AtomicU32; 3]` (f32 bits, metres, Havok frame), `MOVE_SEQ: AtomicU32`
(written last), and `EYE_H_BITS` (existing; now dynamic). The detour reads `MOVE_SEQ`, the feet, then `MOVE_SEQ`
again; on a mismatch it uses `physics.position`. It also uses `physics.position` if `MOVE_ACTIVE` is false or
`|feet − physics.position| > publish_max_m`. No allocation, lock or log happens in the detour (existing rule).

---

## 2. MW2 movement model (pure, `mw2er-core`)

### 2.1 Constants table (all into `movement.json`)

The exe addresses are virtual addresses in `iw4sp.exe` (image base 0x140000000), from the user's install, read
statically. "dump" means `D:\Mods\MW2inER\dumps\m5\...` or `dumps\common_mp\weapons\ump45_mp`.

| Constant | IW4 value | ER value | Source | Label |
|---|---|---|---|---|
| g_speed | 190 in/s | 4.826 m/s | `mov edx,190` @0x140172b9d (int dvar) | V |
| player_sprintSpeedScale | 1.5 → 285 in/s | 7.239 m/s | xmm10 = 1.5 @0x140075001, registered @0x140075614 | V |
| player_backSpeedScale | 0.7 → 133 in/s | 3.378 m/s | @0x140075517 | V |
| player_strafeSpeedScale | 0.8 → 152 in/s | 3.861 m/s | @0x1400754e1 | V |
| player_sprintStrafeSpeedScale | 0.667 (rightmove while sprinting) | — | dvar decode | V |
| crouch speed scale | 0.65 → 123.5 in/s | 3.137 m/s | .rdata @0x14031F8C4 (PM_CmdScaleForStance 0x1400785D0) | V |
| prone speed scale | 0.15 → 28.5 in/s | 0.724 m/s | .rdata @0x14031F740 | V |
| ADS walk factor | ×0.4 | — | PM_CmdScale_Walk 0x1400786C0 | V (formula); I (pm_flag 0x40 = ADS) |
| UMP45 moveSpeedScale / adsMoveSpeedScale | 1 / 2 → ADS 152 in/s | 3.861 m/s | weapon file | V |
| MP script weapon class scale (`weapon_move_class`) | smg 1.0, rifle 0.95, mg 0.875, launcher 0.8 | — | `_weapons.gsc` updateMoveSpeedScale | V |
| Lightweight (`lightweight`) | ×1.07 | — | `_perkfunctions.gsc:340` | V |
| g_gravity | 800 in/s² | 20.32 m/s² | xmm6 = 800 @0x140075FBE | V |
| Jump height | 39 in (constant; the `jump_height` dvar is never read) | 0.9906 m | Jump_Start 0x140070310 (`gravity*78`), clamp @0x1400706A6 | V |
| Jump v0 / apex time | 249.8 in/s / 0.3122 s | 6.345 m/s | derived √(2·800·39) | V |
| Min time between jumps | 500 ms | — | `cmp 1F4h` @0x14007033A | V |
| jump_slowdownEnable | SP default 0; MP default inferred 1 → config default **true** | — | xor edx,edx @0x14007089F | V (SP) / I (MP) |
| Slowdown on landing | below origin+18 in: ×0.65, pm_time 1800 ms; else ×0.5, 1200 ms | — | 0x1400702B7 / 0x1400702C8 | V |
| Land factor (next jump) | pm_time ≥ 1700 → 2.5, else 1 + 1.5·pm_time/1700; v_z² ÷= factor | — | 0x1400703DC..0x140070405 (Jump_GetLandFactor 0x1400707B0) | V |
| jump_spreadAdd | 64 (aimSpreadScale /255) | — | dvar decode | V |
| Friction | drop = max(stopspeed 100, speed) × 5.5 × dt (the dvars are never read) | — | 5.5 @0x1400799E1, 100 @0x1400799A5 | V |
| Land friction factor | ×0.3 while PMF_TIME_LAND | — | PM_Friction 0x1400798A0 | V |
| Ground accel | stand 9, crouch 12, prone 19; ×0.25 while TIME_LAND | — | PM_WalkMove @0x14007CB6F..9C | V |
| Air accel | 1 | — | @0x1400773ED | V |
| Accelerate | accelspeed = min(addspeed, accel·dt·max(wishspeed, 100)) | — | PM_Accelerate 0x140077130 | V |
| Step height | 18 in (prone 10 in) | 0.457 m / 0.254 m | @0x14007E925 / 0x14007E92F | V |
| Walkable normal | n.y ≥ 0.7 (45.6°) | — | Q3/CoD4 heritage; no 0.7 compare found | I |
| Eye heights | stand 60, crouch 40, prone 11 in | 1.524 / 1.016 / 0.279 m (×ratio, section 3) | PM_ViewHeightAdjust 0x14007C510, tables @0x1403BD470/4E0/550/5E0 | V |
| Hull heights | 70 / 50 / 30 in | 1.778 / 1.270 / 0.762 m | halfsizes @0x140077D00..92 | V |
| Hull half-width | 15 in | 0.381 m | — | I (the controller uses ER's `hit_radius`) |
| Stance transitions | stand↔crouch 200 ms, crouch↔prone 400 ms, stand↔prone via crouch 600 ms | — | r13 = 200, r15 = 0x190 @0x14007C5A3/5AC | V |
| player_sprintTime | 4 s (MP `scr_player_sprinttime 4`), max 16.383 s | — | xmm7 = 4.0 @0x140075288; 0x140073CD0; `default_mp_gamesettings.cfg` | V |
| sprintDurationScale (UMP45) | 1 | — | weapon file | V |
| player_sprintMinTime | 1 s | — | xmm12 = 1.0 @0x140074F06 | V |
| player_sprintRechargePause | 0 (applies only after an exhausted sprint) | — | xorps; PM_UpdateSprint 0x14007B400 | V |
| Recharge rate | 1 s per 1 s | — | PM_UpdateSprint | V |
| player_sprintForwardMinimum | 105 of 127 | — | `lea edx,[rdi+69h]`; rdi = 0 traced by one finding, not by the other | V/I (irrelevant for keyboard: W = 127, strafe-only = 0) |
| player_sprintUnlimited | false (Marathon = `marathon = true`) | — | `xor edx,edx` | V (Marathon effect I) |
| Sprint cancel / start masks | 0xCF35 / 0xCC35 | — | @0x14007B50E / 0x14007B5F4 | V masks; bit names I except sprint 0x2 and jump 0x400 (V) |
| Fall damage | h ≤ 200 in: 0; 200..350: (h−200)/150 of max HP; ≥ 350: lethal | 5.08 m / 8.89 m | PM_CrashLand 0x140078B90, @0x140078CD9/CF0/D07 | V (SP exe; MP inferred equal) |
| Landing effects | h ≥ 12 in: velocity ×0.67; damage landing (1..99 %, not ducked): TIME_LAND pm_time = min(500+35·dmg, 2000) ms, velocity ×(0.5 at 500 ms … 0.2 at ≥ 1500 ms) | — | @0x140078E5A | V |
| View dip | min(24, 4 + 4·(h−12)/26) in | — | PM_CrashLand | V (value); I (recovery curve) |
| Sprint anims (UMP45) | sprintIn 0.3 / loop 0.7 / out 0.3 s | — | weapon file | V |
| Hip spread (UMP45) | stand 2–5, ducked 1.75–4.5, prone 1.5–4; moveAdd 4, decayRate 4, fireAdd 0.52; ducked/prone decay 1 | — | weapon file | V |
| bg_aimSpreadMoveSpeedThreshold | 11 in/s | — | dvar decode | V |
| Bob amplitudes (weapon, x/y) | stand 0.055/0.025, ducked 0.045/0.025, prone 0.02/0.005, sprint 0.02/0.014; max 8, lag 0.25 | — | dvar decode | V values; I formula |
| View bob (x/y) | stand 0.007/0.007, ducked 0.0075/0.0075, prone 0.02/0.005, sprint 0.02/0.014; player_sprintCameraBob 0.5 | — | dvar decode | V values; I formula |
| player_runThreshhold / moveThreshhold | 110 / 10 in/s | — | dvar decode | V |

**Gotchas [V]:**
- `jump_height`, `friction`, `stopspeed` and `bg_fallDamageMin/MaxHeight` are registered but never read. The
  controller uses the inlined constants.
- Diagonal walking is **not** faster: wishspeed = g_speed·max(|fm·back|, |rm·0.8|)/127, so W+A = 190, A = 152,
  S = 133, S+A = 152 in/s.
- Accelerate uses max(wishspeed, 100) as its base, so prone (28.5 in/s, accel 19) feels snappy.
- The SP exe is the only one installed. Its bg_/player_ defaults are assumed to equal MP's
  (`scr_player_sprinttime 4` matches) [I].

### 2.2 Wish velocity (`pmove::cmd_scale`) [V formula]

```
fmv = fm · (fm < 0 ? 0.7 : 1);  rmv = |rm| · 0.8                (rm already ×0.667 while sprinting)
wishspeed = g_speed · max(|fmv|, rmv) / 127                     (0 if fm = rm = 0)
          × (sprinting ? 1.5 : 1)
          × stance_scale(eye_target, lerp)    (1 / 0.65 / 0.15; linear blend during crouch↔prone, 400 ms)
          × (ads ? 0.4 · adsMoveSpeedScale : moveSpeedScale)
          × weapon_class_scale × (lightweight ? 1.07 : 1) × speed_scale
wishdir   = normalize(fwd_h · fm + right_h · rm)
```

`speed_scale` (default 1.0) is the single non-MW2 knob, for the user's taste. While prone and ADS the 0.4 factor
also applies (as in PM_CmdScale_Walk).

### 2.3 Substep order (IW4 PM_WalkMove / PM_AirMove) [V parts; I exact order]

1. `sprint.update(cmd, now)` (section 2.5) and `stance.update(cmd, now, headroom)` (section 3).
2. Jump check (section 2.6). A jump sets airborne this substep.
3. **Grounded**: `friction` (stand/crouch/prone identical; ×0.3 during TIME_LAND; × land factor while the jump
   slowdown is active). Then `accelerate(wishdir, wishspeed, accel_stance × (TIME_LAND ? 0.25 : 1))`. The velocity is
   clipped to the ground plane (`clip_velocity(v, n, 1.001)`).
   **Airborne**: no friction, `accelerate(wishdir, wishspeed, 1.0)`.
4. Gravity (airborne): `v_end = v.y − g·h`, then move with `(v.y + v_end)/2` and set `v.y = v_end`. This is exact for
   constant gravity, so the apex does not depend on the frame rate. `Jump_ClampVelocity` caps a rising v.y at
   √(2g·(origin + 39 − y)), and sets it to 0 when that gap is < 0.1 in.
5. Collide and slide with step-up (section 5).
6. Ground probe and snap (section 5.3). A transition airborne → grounded runs the landing (section 2.7).

### 2.4 Friction and acceleration [V]

```
friction:  s = |v_h|; if s < 1 → v_h = 0
           control = max(100, s); drop = control · 5.5 · h (× 0.3 TIME_LAND) (× landFactor while slowed)
           v_h *= max(0, s − drop) / s
accelerate: add = wishspeed − dot(v, wishdir); if add ≤ 0 return
           v += wishdir · min(add, accel · h · max(wishspeed, 100))
```

Hand-computed known answers (60 Hz, stand, W from rest; used by the tests and `movebench`):
- speed after frames 1..11 = 28.5, 47.833, 67.167, 86.5, 105.833, 124.632, 141.707, 157.217, 171.305, 184.102,
  **190.0** in/s, so 190 in/s is reached at 0.183 s;
- release from 190 in/s: 172.583, 156.763, 142.393, 129.341, 117.485, 106.715, 96.933, then −9.1667 per frame, so
  speed is **0 at frame 18 (0.300 s)**.

### 2.5 Sprint (`sprint::Sprint`) [V rules from PM_UpdateSprint 0x14007B400]

- `max_ms = min(4000 × sprintDurationScale, 16383)`. With `marathon = true` the remaining time is always max.
- Remaining time: never sprinted → max. Otherwise `remaining_at_end + (now − last_end)` (1:1 recharge), minus
  `rechargePause` (0) only when the last sprint was exhausted. Clamped to [0, max].
- **Start** (on a new sprint press in `press` mode, or while held in `hold` mode), only if all of these hold:
  - no button-up is pending;
  - not on a ladder (the controller is suspended there anyway);
  - `forwardmove > 105`;
  - none of fire / ADS / reload / jump / grenade held (mask 0xCC35);
  - not mantling;
  - remaining > `sprintMinTime` (1 s);
  - when crouched or prone, a stand-up headroom ray passes, and the stance goes to stand (the normal 200 or 600 ms
    transition).
- **End**, each logged with `why=`:

  | why | Condition |
  |---|---|
  | `time` | remaining reaches 0; sets the exhausted flag |
  | `fire` | fire pressed |
  | `ads` | ADS pressed |
  | `reload` | reload pressed |
  | `jump` | jump/stand pressed |
  | `crouch` / `prone` | crouch or prone pressed |
  | `forward` | forwardmove ≤ 105 (S, strafe-only or W released) |
  | `toggle` | a new sprint press (press mode); then button-up is required before the next sprint |
  | `release` | Shift released (hold mode only) |
  | `handback` | the controller was suspended |

  **Releasing Shift does not end a press-mode sprint.**
- While sprinting: `rm ×= 0.667`, wishspeed ×1.5, and the weapon state is `sprint` (section 8).
- Reload (M2 `weapon_sim`) **[I, MW2 behaviour]**: starting a sprint during a reload cancels the reload. No ammo is
  added unless the reload had completed. The reload key during a sprint ends the sprint (`why=reload`) and the reload
  starts after sprint_out.

### 2.6 Jump (`pmove::jump_check`) [V]

- Needs all of these:
  - a **new** press of the jump key (`+gostand`);
  - grounded;
  - stance stand, with no transition running. In crouch or prone, Space stands up instead, toward
    `gostand_from_prone` (`stand` by default [I]);
  - ≥ 500 ms since the last jump;
  - the controller driving.
- Sets `v.y = 249.8` in/s. If the slowdown window is active: `v.y = √(v.y² / landFactor)`.
- Records `jump_origin_y` and `jump_time`, and sets the JUMPING flag. Ends any sprint (`why=jump`).
- Adds `jump_spreadAdd` 64/255 to the aim spread scale.
- Air control: accel 1 toward the wishdir (no friction). A W-held running jump keeps 190 in/s horizontally: the jump
  covers 118.6 in (3.01 m) on flat ground in 0.6245 s.
- Ceiling: a rising player whose up-ray hits gets `v.y = 0`.

### 2.7 Landing and fall damage (`fall`) [V numbers; I combination order]

At touchdown, fall height `h = v_impact² / (2·800)` (in inches, from the vertical speed just before the snap):
1. `h ≥ 12` → `v_h ×= 0.67`.
2. If JUMPING and `jump_slowdown`: landed below `jump_origin + 18` → `v_h ×= 0.65`, pm_time 1800 ms; else ×0.5, 1200 ms.
   The flat-ground running jump therefore lands at 190 × 0.67 × 0.65 = **82.8 in/s** (127.3 with slowdown off). The
   two factors are each [V]; applying both in this order is [I].
3. Damage %: `h ≤ 200 → 0`; `200 < h < 350 → (h − 200)/150 × 100`; `h ≥ 350 → 100`. If 1..99 % and not crouched:
   TIME_LAND for `min(500 + 35·dmg, 2000)` ms and `v_h ×= lerp(0.5 → 0.2, pm_time 500 → 1500 ms)`. A lethal or
   crouched damage landing uses ×0.67.
4. View dip `min(24, 4 + 4·(h − 12)/26)` in (scaled by the eye ratio). The eye drops by it and recovers over 150 ms
   with an ease-out [I].
5. HP effect (`fall_damage = "mw2"`, section 4.4): `ER damage = round(dmg% / 100 × max_hp × fall_damage_scale)`.
   Non-lethal results are clamped to leave at least 1 HP. Lethal (100 %) writes 0.

Free fall into nothing: if no ground ray has hit for `freefall_handback_m` (30 m) of fall, the controller hands back
(`reason=freefall`), so ER's own void and death handling runs.

---

## 3. Stances (`stance::Stance`) [V numbers; I rules marked]

- States: `Stand`, `Crouch`, `Prone`, plus a transition `{from, to, t0, dur}`. Stand↔prone always chains through
  crouch (200 + 400 = 600 ms).
- Eye height (inches) follows the exact piecewise-linear MW2 curves (percent of transition : height):

  | Transition | Curve |
  |---|---|
  | stand→crouch | 0:60, 1:59.5, 4:58.5, 30:56, 80:44, 90:41.5, 95:40.5, 100:40 |
  | crouch→stand | 0:40, 5:40.5, 10:41.5, 20:44, 70:56, 96:58.5, 99:59.5, 100:60 |
  | crouch→prone | 0:40, 11:38, 22:33, 34:25, 45:16, 50:15, 55:16, 70:18, 90:17, 100:11 |
  | prone→crouch | 0:11, 5:10, 30:21, 50:25, 67:31, 83:34, 100:40 |

  Known answers: stand→crouch at 100 ms (50 %) = **51.2 in**; crouch→prone at 200 ms (50 %) = **15 in**.
- `stance_eye = "ratio"` (default): `eye_m = eye_in / 60 × [first_person] eye_height_m`. That gives 1.600 / 1.067 /
  0.293 m and keeps the stand eye the user verified in run 7. `"mw2"` uses `eye_in × 0.0254` (1.524 / 1.016 /
  0.279 m). The hull heights scale with the same ratio (stand 1.867 m, crouch 1.333 m, prone 0.800 m with the
  defaults).
- Key behaviour (MW2 PC binds [V], semantics [I]):

  | Key | stand | crouch | prone |
  |---|---|---|---|
  | C (`togglecrouch`) | → crouch | → stand | → crouch |
  | LCtrl (`toggleprone`) | → prone | → prone | → stand |
  | Space (`+gostand`) | jump | → stand | → `gostand_from_prone` (stand) |
  | LShift (sprint start) | — | → stand (if headroom) | → stand (if headroom) |

  - `crouch_mode = "hold"`: crouch while C is held.
  - `stance_hold` key (empty by default): a tap toggles crouch; held ≥ 300 ms (`cl_stanceHoldTime` [V]) goes prone.
- Raising a stance needs headroom: an up-ray from the feet to the target hull height plus 0.05 m must not hit.
  Otherwise the stance stays and `MOVE stance blocked to=stand why=headroom` is logged (rate-limited).
- Going prone needs the player grounded and not mid-transition [I]. It ends a sprint.
- Speed and accel: crouch ×0.65 / accel 12, prone ×0.15 / accel 19. During crouch↔prone the speed scale blends
  linearly over the 400 ms (V). Stand↔crouch switches at the start of the transition (I).
- Step height while prone: 10 in. Jumping is not allowed while crouched or prone (Space stands up instead).
- First person: the body is already hidden. Third person: the ER body stays standing in every stance, because ER's
  own crouch is not driven in M5 (section 7). The player's hurtbox stays ER's standing capsule (known gap, section 15).

---

## 4. Game fields: read, written, restored

### 4.1 Offsets [V compiled with `offset_of!` against vendored fromsoftware-rs 59fbd3b; 0x50/0x60/0x70 V in game]

Path: `WorldChrMan.main_player → ChrIns.modules (+0x190) → ChrInsModuleContainer.physics (+0x68)`.

| Field | Offset | Use |
|---|---|---|
| `orientation` / `interpolated_orientation` | 0x50 / 0x60 | yaw pair (existing, first person) |
| `position` | 0x70 (f32x4, w kept) | read `P_er`, write `P_w` |
| `last_update_position` | 0x80 | optional write (`write_last_update`, probe) |
| `chr_proxy_pos_update_requested` | 0x91 (bool) | write 1 every driving tick |
| `standing_on_solid_ground` / `touching_solid_ground` | 0x92 / 0x93 | read (cross-check) |
| `material_info.normal_vector` | 0x1f0 + 0x40 | read (`ground_normal = "er"`) |
| `motion_multiplier` | 0x1c4 | read only (probe) |
| `gravity_multiplier` | 0x1cc (f32) | save / write 0 / restore |
| `is_falling` / `is_touching_ground` | 0x1d0 / 0x1d1 | read |
| `gravity_disabled` | 0x1d5 (bool; TAE ChrActionFlag 27) | save / write 1 / restore / re-assert |
| `hit_height` / `hit_radius` | 0x2f0 / 0x2f4 | read (hull radius) |
| `ChrIns.chunk_position` | ChrIns+0x80 | read (rebase vs warp) |
| `PlayerIns.block_position` | +0x6c0 | read (logged with chunk_position) |
| `ChrCtrl.chr_proxy_flags` | ChrIns.chr_ctrl (+0x58) → +0xfc, bit0 | fallback only (`proxy_sync_flag`) |
| `ChrCtrl.modifier → data.movement_limit` | +0xc8 → +0x30 | probe (handback candidate) |
| `CSChrActionFlagModule.animation_action_flags` / `action_modifiers_flags` | +0x10 / +0x40 | probe; dodging bit used today |
| `CSChrActionRequestModule.action_requests` | modules.action_request (+0x80) → +0x10 | key probe (section 6.4) |
| `CSChrDataModule.hp` (+0x138) / `max_hp` | data module | fall damage |

New compile-time asserts in `game/move_read.rs` cover every offset above, as `fp_read.rs` already does.

### 4.2 Writes and restores

| # | Field | When | Size | Restore | Logged |
|---|---|---|---|---|---|
| 1 | `physics.position` (+0x70), w kept; with `write_last_update` also +0x80 in the same 32-byte write | every driving tick (not on a trigger tick) | 16 B (32 B) | **release**: none (dynamic state, like yaw; handing back = stop writing) | first write per episode: `MOVE write field=CSChrPhysicsModule.position(+0x70) addr= old= new=` |
| 2 | `chr_proxy_pos_update_requested` (+0x91) = 1 | every driving tick, right after #1 | 1 B | none (a request flag ER consumes) | once per episode |
| 3 | orientation pair (+0x50, 32 B) | first person, existing `fp_math::yaw_pair` path | 32 B | release (M3b `YawRelease`) | existing |
| 4 | `gravity_multiplier` (+0x1cc) = 0.0 | on entering `Driving` | 4 B | the saved value, if the physics address is unchanged and the field still holds 0.0; deferred if not in world | `MOVE write ...gravity_multiplier(+0x1cc) old= new=0.000`, `MOVE restore field=gravity_multiplier ... ok=1|skipped=changed|deferred` |
| 5 | `gravity_disabled` (+0x1d5) = 1 | on entering `Driving`; re-asserted on any tick ER cleared it (counted) | 1 B | the saved value, same guards as #4 | as #4 plus `MOVE gravity reasserted count=N (first)` and every 5 s in the periodic line |
| 6 | `ChrCtrl.chr_proxy_flags` bit0 (read-modify-write) | only if `proxy_sync_flag = true` | 1 B | none (request bit) | once |
| 7 | `CSChrDataModule.hp` | once per damaging landing, `fall_damage = "mw2"` | 4 B | **never** (section 4.4) | each time: `MOVE land ... hp=old->new` |

Writes #1 + #2 cost two `game::write` calls per tick (about 0.3-0.5 µs each with the page cache, MODLOG run 6).

### 4.3 Exit paths that restore #4/#5

All of these go through one `MoveCtl::release(reason)`, which logs `MOVE off reason=` or
`MOVE suspend reason=`:
- F5 off;
- `[movement] enabled = false` (hot);
- every handback reason (section 9);
- AC-130 start;
- `TASK world left` and a changed main player (deferred: the save is kept with its physics address and restored on
  the next in-world tick only if the same module address is back and holds our values; otherwise it is dropped with
  `skipped=stale`, never written through a stale pointer, exactly like `pending_hidden`);
- task panic (the `emergency_off` pattern of task.rs: `move_ctl.emergency_off()` runs before FP's and the AC-130's).

The restore model is the pure `move_logic::GravityOwner`, tested as a round trip over every reason (section 13.1).

### 4.4 Fall damage write (the one non-reversible write) [I]

- An HP loss from a fall is a gameplay event like a hit, so it cannot be "restored".
- It is off when `fall_damage = "off"`.
- It is written through `game::write` (version gate, page check), at most once per landing, only while `Driving`.
- It never writes a value above the current HP.
- Each write is logged as `MOVE land ... dmg_pct= hp=a->b`.
- Lethal falls write 0. Whether ER then runs its normal death is **[I]**: the next 2 s are watched for the existing
  `death` signal, and `MOVE WARN lethal_fall no_death_seen` is logged if it does not come. The fallback is the
  config value `lethal_fall = "zero" | "handback"`. `handback` restores gravity 0.2 s before the predicted impact
  and lets ER resolve the fall.

### 4.5 What is never written

ER's root motion fields (`root_motion` 0xd0, `CSChrBehaviorModule.root_motion`), `motion_multiplier`,
`ChrCtrlModifierData`, the fall module (`fall_timer`), SpEffects, `is_falling` and `ChrCtrl.disable_move`. The CS2
note [R] lists `fall_timer`, `is_falling`, the material flag, SpEffects and `noDead` as tried and **not** working
against fall death.

---

## 5. Collision with rays

### 5.1 The only query [V crate + in-game use]

- `CSPhysWorld::cast_ray(filter 0x2000058, origin, delta, owner) -> Option<hit point>`, RVA 0xc71e00.
- The filter is the constant the game itself passes (`ba 58 00 00 02 e8`).
- It is already used in game by `game::havok::map_ray` (`SHOT ... src=map`, `AC130 SHOT ... via=map`).
- New wrapper `havok::map_ray_point(origin, delta) -> Option<V3>` returns the point. It counts every call and its QPC
  time for PERF (`rays=`, `ray_us_max=`).
- A ray starting inside a shape is assumed to miss that shape [I]. Hence the skin distance and the raised origins.

### 5.2 Capsule from rays (`move_collide`, pure, behind a `RayWorld` trait)

- `r` = `hull_radius_m`, or ER `physics.hit_radius` when 0, clamped to [0.2, 0.6] m. `skin` = 0.03 m.
- Horizontal sweep for a move `d` (planar) in a substep:
  - Heights above the feet: `low` 0.10 m, `knee` = step + 0.05 m (0.507 / prone 0.304), `chest` = hull − 0.15 m.
    Two heights closer than 0.1 m are merged (prone keeps two).
  - Lateral offsets: 0 and ±0.7r. A ray at offset `o` stops at `t − r_eff − skin` with `r_eff = √(r² − o²)`.
  - Rays are cast only when `|d| > 0.001 m`. Length `|d| + r + skin`.
- Wall normal: from two hits at the same height with different offsets, it is the horizontal perpendicular of
  `p2 − p1`, facing against the move. With only one hit, two feeler rays at ±25° yaw at that height give the second
  point. If there is still one point, the normal is `−d̂` (stop).
- Slide: Q3 `PM_ClipVelocity(v, n, 1.001)`, then move the remaining fraction. At most `max_bumps` (3) per substep.
  Two opposing planes (a corner) stop the horizontal motion.
- Step-up: the `low` ray hits within the travel, but the `knee` ray at the same offset is clear. Then a down-ray from
  `pos + d̂·(t_low + r) + up·(step + 0.05)` of length step + 0.1. A floor rise ≤ step (and a walkable 3-point normal
  when available) passes, the headroom ray from the new floor up to the hull height must be clear, and then
  `feet.y = floor`. Otherwise it is a wall.
- Ceiling: when `v.y > 0`, an up-ray from `feet + hull − 0.05` of length `v.y·h + 0.05`. A hit sets `v.y = 0`.

### 5.3 Ground

- Down-ray at the centre from `feet + step + 0.05` down to `feet − snap`. `snap` = step when grounded at the last
  substep and `v.y ≤ 0` (keeps the player on stairs and slopes going down); otherwise `|v.y|·h + 0.05`.
- Grounded if the ray hits within that range: `feet.y = hit.y`, `v.y = 0` (landing if the player was airborne).
- Normal, with `ground_normal = "rays"` (default): when grounded and moving, two more down-rays at `+fwd·r` and
  `+right·r` give a plane; otherwise the last normal is kept. With `"er"`: `material_info.normal_vector` from ER's
  step at our synced position (free; [I] semantics). Both are logged for calibration (`slope_rays=`, `slope_er=`).
- `n.y < 0.7` (steeper than 45.6°) is not walkable. The player is airborne on it: no friction, gravity, and the
  velocity clipped against that plane, so the player slides down it.
- Cross-check: ER's `is_touching_ground`, `standing_on_solid_ground` and `touching_solid_ground` after its step at
  our position are logged next to ours (`ground=1 er_ground=1/1/1`). A disagreement streak of more than 30 ticks is
  counted in the periodic line.

### 5.4 Tunnelling and budget

- Every ray is a sweep over the whole substep path, so a fast move cannot skip a wall that lies on one of the 3×3
  lines. Sprint at 30 Hz moves 0.24 m per substep. A 50 m/s fall casts `|v.y|·h + 0.05` down.
- Budget: `max_rays_per_tick` (32). When it runs out mid-tick, the horizontal part of the remaining substeps is
  dropped (vertical still uses the ground ray). This is logged in the periodic line (`budget_hits=`). The worst case
  with 2 substeps (30 Hz) sliding into a corner is about 2 × (9 + 2 feelers + 3 ground + 1 step + 1 headroom +
  1 ceiling) = 34, so the default allows ~1 bump without loss. PERF measures the real ray cost before anything
  is tuned.
- Idle cost: 1 ground ray per tick.

### 5.5 Rebase and warp (`move_logic::classify_jump`) [I]

- Each tick: `e = P_er − P_w_last` (`P_w_last` = our last written position).
- If `|e_planar| > 5 m` (the existing `REBASE_JUMP_M`): look at `chunk_position`.
  - It moved by less than 1 m plus our expected motion → **rebase**. Translate the controller's position by `e`,
    keep velocity, stance and sprint.
  - Otherwise → **warp**. Resync to `P_er`, zero the velocity, end the sprint (`why=handback`) and reset the jump
    state.
- Both log `MOVE resync why=rebase|warp d=… chunk_d=…`. `block_position` is logged next to it for calibration.
- On `Arming → Driving` the controller always resyncs (`why=handback`), and also on world entry (`why=world_enter`).

### 5.6 Things the map rays do not see [I]

- **Characters**: before the sweep, a pure circle push-out against `read_nearby` (existing `game/chars.rs`) using
  `aim.char_radius_m` + r, at most 0.2 m per tick. The velocity component into the circle is removed.
- **ER proxy push-out** (`er_pushout = true`): `e` from 5.5, below 5 m. Only its planar component opposite to our
  last move direction counts, when it is larger than 0.02 m. It is clamped to `er_pushout_max_m` (0.3 m), added to the
  position, and the velocity component into it is removed. In first person, W/A/S/D are blanked, so ER adds almost
  no root motion and `e` is mostly push-out. In third person (hybrid), ER's run root motion points roughly along our
  move, which this rule ignores.
- **Knockback, staggers, grabs**: a planar `e` larger than `er_move_handback_m` (0.30 m) in one tick, not counted as
  push-out → `Suspended(er_move)` for `er_move_hold_s` (0.5 s) after ER's per-tick displacement drops under 0.05 m.
  This covers run 10's "player pushed 6.8 m".
- The distribution is logged every 5 s: `MOVE er_delta p50= p95= max= applied= handbacks=`.

---

## 6. Input

### 6.1 Keys (`[keys]`, all configurable)

| Key | Default | MW2 bind [V] | Notes |
|---|---|---|---|
| `move_forward/back/left/right` | `W` / `S` / `A` / `D` | +forward/+back/+moveleft/+moveright | `MOVE_W..` already exist; now configurable |
| `move_sprint` | `LSHIFT` | +breath_sprint | press mode by default |
| `move_jump` | `SPACE` | +gostand | jump; stand up from crouch/prone |
| `move_crouch` | `C` | togglecrouch | `crouch_mode = "toggle" | "hold"` |
| `move_prone` | `LCTRL` | toggleprone | |
| `move_stance_hold` | `""` (off) | (console `+stance`, cl_stanceHoldTime 300) | tap = crouch, hold 300 ms = prone |

- `dik_from_name` gains: `SPACE` 0x39, `LSHIFT` 0x2A, `RSHIFT` 0x36, `LCTRL`/`LCONTROL` 0x1D, `RCTRL` 0x9D, `LALT`
  0x38, `RALT` 0xB8, `TAB` 0x0F, `CAPSLOCK` 0x3A. Letters and digits exist already (`C` 0x2E, `X` 0x2D, `F` 0x21,
  `Q` 0x10, `E` 0x12, `Z` 0x2C).
- Validation: no duplicates among the movement keys or between them and `toggle`, `reload`, `fp_toggle`, `ac130`.
- The `ac130_next` (Q), `ac130_thermal` (F) and `ac130_gun1-3` keys may overlap the blank list. They are used only
  while the gunship is active, when the controller is suspended (`reason=ac130`), so the contexts never mix (tested).

### 6.2 What is hidden from ER (`move_input::blank_set`) [V ER params; I live preset]

ER's own keyboard defaults, decoded from the user's regulation (`KeyAssignParam_TypeB/C` + `KeyAssignMenuItemParam`,
keyboardKeyId = DIK + 69, checked on 8 keys) [V]: Space = backstep/dodge/dash, F = jump, X = crouch, LAlt = movement
control (walk), Q = lock-on, E = event action, R = use item, G = map, modifier (probably Shift [I]) + LMB/RMB = strong
attack/skill. LCtrl, C and Z are free in the field. Preset A differs (crouch C, dodge LShift, jump Space). Which
preset is live, and any user rebinds, are in the save, which is not read → **the blank list covers the MW2 keys
whatever ER role they have**, and the first run's key probe (6.4) records what really collides.

| Context | Blanked from ER | Latch |
|---|---|---|
| `Driving`, first person | `blank_er` = SPACE, LSHIFT, C, LCTRL, F, X, Q, LALT (+ the configured movement keys) **and W/A/S/D** | latched (`latch_next`) until physical release; **W/A/S/D not latched** |
| `Driving`, third person, `tp_anim = "hybrid"` | `blank_er` only; W/A/S/D reach ER | as above |
| `Suspended` / `Arming` / `Off` | nothing new; a key still held from Driving stays blanked until released | — |
| MW2 mode off | nothing (existing behaviour) | — |

- Space must stay blanked until it is released, because ER dodges on the **release** of a short Space press [R
  community]; un-blanking it while held would roll.
- W/A/S/D are never latched, so a ladder or Torrent handback gives ER the held key at once.
- E is never blanked: interactions are ER's, and they are a handback (9.1).
- Blanking needs the fresh task heartbeat (< 250 ms) and in-world with no menu (the existing gates). It is a flag
  check in the detour, not a memory write.
- The gamepad is not blanked and the controller ignores it (known gap, section 15).

### 6.3 Reading the keys (input hook)

- `RAW_MOVE_KEYS` (u8) becomes `RAW_KEYS: AtomicU32`: a level bit per movement key plus E, F, X, Q and LAlt.
- New `PRESSES_MOVE: [AtomicU16; 6]`: press counters for sprint, jump, crouch, prone, stance_hold and E, bumped in
  the detour on each up→down edge. The detour runs on every ER poll, about once per frame. The task consumes the
  difference from its last read, so a tap shorter than a task tick is never lost.
- Levels and edges are recorded **before** blanking (existing rule for `RAW_MOVE_KEYS`).
- The task builds the usercmd from the levels plus the counted presses. Fire, ADS and reload come from the existing
  M2/M3 latches.

### 6.4 First-run key probe [I → V by log]

`keyprobe = true` (default for M5 run 1): while MW2 mode is **off**, each press of SPACE, LSHIFT, C, LCTRL, F, X, Z,
Q or LALT logs, once per key per session, which `CSChrActionRequestModule.action_requests` bits rose within 0.3 s
(bit names from the crate: `sp_move` 5, `jump` 6, `l3` 13 (crouch), `backstep` 16, `rolling` 17, ...):
`MOVE keyprobe key=SPACE er_bits=backstep|rolling`. A key with no bits logs `er_bits=none`. This gives the user's
live ER bindings. Any key the user binds to an ER action that MW2 keys do not cover can then be added to `blank_er`
without a rebuild.

---

## 7. Third-person animation (decision) [I]

Options compared:

| Option | Look in third person | Risk | Decision |
|---|---|---|---|
| A: blank W/A/S/D too, pure kinematic | the body slides in its idle pose ("statue") | none | rejected for third person; **used in first person** (the body is hidden) |
| **B: hybrid** | ER sees W/A/S/D: walk/run animation, footsteps, body turns toward the move input; we overwrite the position after physics, which discards ER's root-motion displacement | ER's animation speed does not match ours (sprint 7.24 m/s vs ER's run); ER root motion goes into `er_delta` (handled by 5.6) | **default** (`tp_anim = "hybrid"`) |
| C: drive ER's own root motion (write `physics.root_motion` 0xd0 in `ChrIns_PrePhysics`) | best collision (Havok proxy handles doors and enemies) | field semantics and frame unknown, needs a new task, TAE-owned | not built; probe only (the periodic line logs 0xd0, `motion_multiplier` and the per-tick ER displacement) |
| D: hand third person back to ER entirely | ER movement | no MW2 feel in third person | `tp_anim = "er"` fallback |

Why B:
- It is the only option that keeps a natural-looking body in third person without unknown writes.
- The M3b yaw design already relies on ER turning the body toward W/A/S/D [V run 4/7].
- ER's camera-relative input matches our camera-relative wishdir, so the body faces the direction we move.

Not done in M5:
- faking ER's dash for the sprint (it drains stamina and rolls on a short release);
- injecting ER's crouch key (X) for a crouch animation (state desync risk);
- changing ER's animation speed (`hks_animation_speed_multiplier` is HKS-owned).

All three are listed as follow-ups.

First-person yaw while driving:
- The yaw gate's A/S/D pause (`fp_math::yaw_gate`) does not apply while the controller drives in first person (ER no
  longer sees A/S/D, so it does not turn the body).
- The lock-on, dodge, turn-lock and throw pauses stay.
- The M3b forward-axis probe learns from ER running under W, so it cannot run while driving. A new
  `[first_person] yaw_forward_axis = "-z"` (default, **[V run 4]**: `-z` convention, drift p95 < 1°) is used when the
  probe has not resolved. `"auto"` keeps the probe for when the controller is off.

---

## 8. Viewmodel, fire gate, spread, HUD

### 8.1 Sprint animations and the fire/ADS gate (`vm_anim`) [V times; I gate behaviour]

- `VmInputs` gains `sprinting: Option<bool>` (Some while the controller drives), `stance`, `stance_frac`,
  `prone_moving`, `airborne` and `land_event`.
- With `Some`, `SprintIn`/`SprintLoop`/`SprintOut` follow the flag (0.3 / 0.7 / 0.3 s from the weapon file). The old
  `planar_speed > sprint_speed_mps` heuristic and its 0.3 s after-shot block apply only when `None` (controller not
  driving).
- **Fire/ADS during a sprint**: the press ends the sprint (`why=fire|ads`) and `SprintOut` plays. The M2 fire gate
  (`weapon_sim` / task.rs `can_fire`) stays closed until `sprint_out_until = t_end + sprintOutTime`. LMB still held
  then fires. ADS starts after the out time.
- Holding LMB blocks a sprint start (start mask).
- Prone crawling (prone and speed > 10 in/s): the weapon file's `proneMove` (−50, 3, −105) and `proneRot` push the
  gun out of view, and firing is blocked while crawling [I, MW2 behaviour].

### 8.2 Stance pose and bob (`vm_pose`) [V field values; I engine semantics]

- Offsets in view space (inches): `duckedOfs R 0.8` while crouched, `proneOfs R 1.5` while prone; moving adds
  `standMoveU −0.5` / `duckedMoveF −0.5`; strafing adds `strafeMoveR 0.5` and `strafeRotR 5`.
- Blend rates: `posMoveRate 6` / `posProneMoveRate 25` /s, `posRotRate 6` / `posProneRotRate 30`.
- Idle sway factor ×0.75 crouched, ×0.4 prone.
- Weapon bob: amplitude per stance (table 2.1) × speed / 190 in/s, clamped to `bobMax` 8, lag 0.25, cycle from the
  distance walked (IW3-like [I]). ADS multiplies by `adsBobFactor` 1. No bob in the air.
- View bob (`view_bob = true`): the eye moves by the MW2 view-bob amplitudes (sprint × player_sprintCameraBob 0.5),
  ADS × `adsViewBobMult` 0.25. Land dip from 2.7.4.
- Applied through `vm_proj` `offset_in`, which is static today and becomes per frame. vmtest needs no change: the
  pose is CPU-side.

### 8.3 Spread (`spread.rs`) [V values; I rule]

- min/max per stance: stand 2–5, crouched 1.75–4.5, prone 1.5–4 (UMP45; from `movement.json` `weapons.ump45_mp`).
  During a transition: the target stance's values.
- Move add (`hipSpreadMoveAdd` 4) applies when the controller's planar speed > 11 in/s (`bg_aimSpreadMoveSpeedThreshold`).
- While airborne, `jump_spreadAdd` 64/255 is added to the spread scale at the jump (clamped to 1).
- Decay: `hipSpreadDecayRate` 4 standing, `hipSpreadDuckedDecay` / `hipSpreadProneDecay` 1 as **multipliers** of
  the decay rate when crouched or prone [I].
- The crosshair already draws the spread scale (M2), so the stance change shows on the HUD at once.

### 8.4 HUD

- A small stance glyph next to the ammo counter (`hud.stance_icon = true`): a standing, crouching or prone bar
  figure, drawn with the existing single background draw list (M4 run-8 rule: exactly one
  `get_background_draw_list()`).
- MW2 shows no sprint meter, so none is drawn.

### 8.5 Aim geometry while crouched or prone [V code; I fix]

- The M2/M3 checks use `chest = feet + aim.chest_height_m` (1.2 m) and require camera-to-chest 0.3..15 m (task.rs
  `chest()`, `check_shot`, `cam_sign`).
- With a crouch eye of 1.07 m, that distance falls to about 0.15 m, so **every crouched shot would be skipped**.
- While the controller drives, the chest reference becomes `eye − (stand_eye − chest_height_m)` (a constant 0.40 m
  below the current eye; `move_aim::chest_ref`). Every M2/M3 distance rule then keeps its standing geometry.
- Tested in 13.1 (`move_aim_chest_*`).

---

## 9. Handback triggers (`move_logic`) [reuse V signals; new ones I]

### 9.1 Reasons, priority highest first

The action column says whether the reason leads to `Off` or `Suspended`, and what releases a suspension.

| # | Reason | Signal | Action |
|---|---|---|---|
| 1 | `panic` | task panic → `emergency_off` | Off (session) |
| 2 | `world_left` | `TASK world left` / main player changed | Off, deferred restore |
| 3 | `loading` | the existing load-counter signal | Suspended; released per the existing latch |
| 4 | `read_fail` / `write_fail` | a required read (physics, position, gravity fields) failed 3 ticks in a row, or `game::write` refused (`why=`) | Off |
| 5 | `no_data` | `movement.json` missing or invalid | Off |
| 6 | `death` | hp ≤ 0, chr_flags1c5 bit 7, camera_type 7 | Suspended until alive and grounded |
| 7 | `cutscene` | fade plate / camera_mask (existing latches) | Suspended |
| 8 | `torrent` | ride_state ≠ 0 or is_mounting | Suspended |
| 9 | `ladder` | ladder state ≠ −1 | Suspended |
| 10 | `throw` | throw_state 3..6 (grabs, critical hits) | Suspended |
| 11 | `talk` | talk_id edge (grace, NPC; existing latch) | Suspended |
| 12 | `interact` | raw E press: suspended for `interact_hold_s` (1.0 s), extended while ER moves the player > 0.05 m/tick or `movement_limit == 3` (if the probe proved it) | Suspended |
| 13 | `er_move` | 5.6 knockback rule | Suspended until settled + 0.5 s |
| 14 | `freefall` | 2.7 | Suspended until grounded per ER |
| 15 | `menu` | cursor shown (existing) | Suspended; stance kept |
| 16 | `ac130` | gunship Starting/Active/Ending | Suspended |
| 17 | `fp_only` | `fp_only = true` and FP not On | Off |
| 18 | `tp_er` | `tp_anim = "er"` and FP not On | Off |
| 19 | `f5` | MW2 mode off | Off |
| 20 | `config` | `[movement] enabled = false` (hot) | Off |

- Same-tick rule: on the tick of any trigger, no position, proxy or gravity write happens; the restore happens in
  that tick.
- Re-arming needs `rearm_ticks` (10) clear ticks **and** ER grounded. Taking over in mid-air while ER's own fall state
  runs could leave ER's fall death armed [R CS2].
- The anti-flap guard (`fp_logic::FlapGuard`) is reused for the inferred signals `interact`, `er_move`,
  `movement_limit`: 5 suspensions in 60 s mutes that signal with a WARN, never a verified one.

### 9.2 Probe signals (logged, not acted on until the user's run proves them)

`MOVE probe movement_limit=… anim_flags=0x… mod_flags=0x… possible_inputs=0x… motion_mult=… root_motion=(…)`, on any
change, rate-limited to 2/s. `interact` uses `movement_limit == 3` only with `use_movement_limit_signal = true`
(default false until a run shows it is 3 exactly during interactions).

---

## 10. Config (`mw2er.toml`)

```toml
[keys]
move_forward      = "W"
move_back         = "S"
move_left         = "A"
move_right        = "D"
move_sprint       = "LSHIFT"   # MW2 +breath_sprint
move_jump         = "SPACE"    # MW2 +gostand
move_crouch       = "C"        # MW2 togglecrouch
move_prone        = "LCTRL"    # MW2 toggleprone
move_stance_hold  = ""         # optional console-style key: tap = crouch, hold 300 ms = prone

[movement]                     # M5: MW2 movement while MW2 mode is on (docs\M5_DESIGN.md); hot unless noted
enabled             = true
data                = 'converted\movement.json'   # not hot; written by tools\scripts\convert_movement.py
fp_only             = false    # true = only while first person is On
tp_anim             = "hybrid" # hybrid = ER animates, we move | er = third person uses ER movement
sprint_mode         = "press"  # press = MW2 (tap to start, tap again to stop) | hold
crouch_mode         = "toggle" # toggle = MW2 C | hold
gostand_from_prone  = "stand"  # stand | crouch
jump_slowdown       = true     # MW2 MP jump_slowdownEnable (inferred on)
marathon            = false    # unlimited sprint
lightweight         = false    # x1.07
weapon_move_class   = "smg"    # smg 1.0 | rifle 0.95 | mg 0.875 | launcher 0.8 (MP updateMoveSpeedScale)
speed_scale         = 1.0      # 0.5..2.0; the only non-MW2 tuning knob
stance_eye          = "ratio"  # ratio = [first_person] eye_height_m x 60/40/11 | mw2 = inches x 0.0254
fall_damage         = "mw2"    # mw2 | off
fall_damage_scale   = 1.0      # 0..2, fraction of ER max HP per MW2 100 %
lethal_fall         = "handback" # review M5: handback is the default until ER death + ray coverage are verified | zero
freefall_handback_m = 30.0
hull_radius_m       = 0.0      # 0 = ER physics.hit_radius (clamped 0.2..0.6)
skin_m              = 0.03
max_substep_s       = 0.0167
max_bumps           = 3
max_rays_per_tick   = 32       # 8..128
ground_normal       = "rays"   # rays | er
walkable_normal_y   = 0.7
er_pushout          = true
er_pushout_max_m    = 0.30
er_move_handback_m  = 0.30
er_move_hold_s      = 0.5
interact_hold_s     = 1.5      # review M5 (was 1.0); extended while ER's action flags differ (section 17)
char_push_radius_m  = 0.45     # review M5: other characters' radius for the push-out
use_movement_limit_signal = false
rearm_ticks         = 10
publish_max_m       = 1.0
strafe_sign         = 1        # 1 | -1 override (section 1.4)
proxy_sync_flag     = false    # fallback: also set ChrCtrl.chr_proxy_flags bit0
write_last_update   = false    # probe: also write last_update_position (+0x80)
blank_er            = ["SPACE", "LSHIFT", "C", "LCTRL", "F", "X", "Q", "LALT"]
view_bob            = true
land_dip            = true
vm_stance_pose      = true
keyprobe            = true
log_every_ticks     = 60

[first_person]
yaw_forward_axis    = "-z"     # new: -z (run 4) | +z | auto (probe; only when the controller is off)

[hud]
stance_icon         = true     # new
```

- Every float must be finite and inside its documented range. Unknown keys are errors (M2 rules).
- `data` is cold.
- The existing `[viewmodel] sprint_speed_mps` stays for the controller-off heuristic.

---

## 11. Log lines

All new lines start with `MOVE`. One-time lines are once per episode unless noted.

| Line | Proves |
|---|---|
| `MOVE cfg enabled=1 tp_anim=hybrid sprint=press crouch=toggle jump_slowdown=1 fall=mw2 eye=1.600/1.067/0.293 hull=1.867/1.333/0.800 r=… rays_max=32 data=converted\movement.json sha=…` | config and data loaded (once per session and on reload) |
| `MOVE data g_speed=190 sprint=1.5x4.0s min=1.0 jump=39in grav=800 accel=9/12/19 air=1 fric=5.5/100 step=18/10 fall=200/350 ump45 move=1 ads=2 dur=1 src_exe_sha=…` | the numbers in use |
| `MOVE blocked reason=<…>` | why it does not arm (once per change) |
| `MOVE on pos=(…) vel0=… er_ground=1/1/1 grav_mult=… grav_dis=… r=… fp=0|1` | took over |
| `MOVE write field=CSChrPhysicsModule.position(+0x70) addr=0x… old=(…) new=(…)` | first position write |
| `MOVE write field=CSChrPhysicsModule.chr_proxy_pos_update_requested(+0x91) addr=… old=… new=1` | first proxy request |
| `MOVE write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=… old=1.000 new=0.000` | gravity owned |
| `MOVE write field=CSChrPhysicsModule.gravity_disabled(+0x1d5) addr=… old=0 new=1` | gravity owned |
| `MOVE gravity reasserted count=N (first)` | ER rewrites the TAE flag |
| `MOVE suspend reason=<…> <detail>` / `MOVE off reason=<…> <detail>` | every handback |
| `MOVE restore field=gravity_multiplier addr=… old=0.000 new=1.000 ok=1` (and `gravity_disabled`), or `skipped=changed|stale` / `deferred` | exact restore |
| `MOVE resync why=handback|rebase|warp|world_enter d=…m chunk_d=…m` | warps handled |
| `MOVE state t=… stance=… frac=… speed_sim=…in/s speed_world=…m/s wish=… sprint=0|1 meter=…s ground=1 er_ground=… slope_rays=…deg slope_er=…deg rays=… er_delta=…m pushout=…m` | every `log_every_ticks` while moving, and on every state change (rate-limited to 4/s) |
| `MOVE sprint start meter=4.00s stance_from=stand` / `MOVE sprint end why=<time|fire|ads|reload|jump|crouch|prone|forward|toggle|release|handback> dur=…s meter=…s` / `MOVE sprint refused why=meter|button_up|forward|mask|headroom meter=…` | sprint rules |
| `MOVE jump y0=… v0=249.8 land_factor=…` / `MOVE apex h=…m t=…s` | jump height |
| `MOVE land fall_h=…in (…m) v_impact=… slow=0.67*0.65 dmg_pct=… hp=a->b land_ms=…` | landing and fall damage |
| `MOVE WARN lethal_fall no_death_seen` | the fall-death fallback is needed |
| `MOVE stance from=stand to=crouch dur_ms=200` / `MOVE stance blocked to=stand why=headroom` | stances |
| `MOVE keys blank=… (fp|tp) latched=…` | the blank set on change |
| `MOVE keyprobe key=SPACE er_bits=backstep|rolling` | the live ER bindings |
| `MOVE probe movement_limit=… …` | the inferred interaction signals |
| `MOVE order detour_before_task=n/m` | frame order (first 600 driving ticks) |
| `MOVE er_delta p50=… p95=… max=… applied=… handbacks=… budget_hits=… gravity_reasserts=…` | every 5 s while driving |
| `PERF ... move=1 move_us_p50=… move_us_p95=… move_us_max=… rays_avg=… rays_max=… ray_us_max=… move_writes=…` | cost (extends the existing PERF line) |

---

## 12. Converter `tools\scripts\convert_movement.py` and the static disassembler

- Inputs (read only):
  - `dumps\common_mp\weapons\ump45_mp` (WEAPONFILE fields: moveSpeedScale, adsMoveSpeedScale, sprintDurationScale,
    sprintIn/Loop/OutTime, hipSpread*, duckedOfs*/proneOfs*/…Move*/…Rot*, pos*Rate, idle factors, adsBobFactor,
    adsViewBobMult);
  - `dumps\m5\localized_code_pre_gfx_mp\default_mp_controls.cfg` (binds, check only);
  - `dumps\m5\code_pre_gfx_mp\default_mp_gamesettings.cfg` (`scr_player_sprinttime`);
  - `dumps\m5\common_mp\maps\mp\gametypes\_weapons.gsc` and `patch_mp\maps\mp\perks\_perkfunctions.gsc` (class
    scales, 1.07);
  - `iw4sp.exe` (PE parse, `.rdata` floats at the recorded VAs, and the eye-curve tables @0x1403BD470/4E0/550/5E0
    via the logic of `work\m5\tables.py`).
- Instruction-site constants (immediates, xmm loads) are confirmed by the static disassembler. It moves from
  `work\m5\dis` into `tools\iw4dis` (its own Cargo project, iced-x86 1.21 from crates.io, not in the `mod` workspace,
  built with a short `CARGO_TARGET_DIR`). Modes: `fval <float>` (every load of that .rdata value),
  `x <start-end>`, `xref <addr>`. The tool only maps the file; it never starts or loads the exe.
- Output: `converted\movement.json` (gitignored under `/converted/`). It contains every constant of table 2.1 with
  value, unit, src and label, the eye curves, the UMP45 block, the MW2 binds, and the sha256 of every input. The file
  is written last, atomically.
- `--check`: every known answer in table 2.1 and section 3 (curve known answers 51.2 in and 15 in) is re-derived and
  compared. If the exe sha differs from the recorded one (an MW2 update), the exe-derived rows report `NEEDS-DATA`
  and keep the pinned values; this is not a failure.
- `--self-test`: on temporary copies under `work\m5\selftest\`, these mutations must FAIL: sprintDurationScale 1→2,
  adsMoveSpeedScale 2→1, `scr_player_sprinttime 4→5`, the .rdata 0.65 patched to 0.6 (in a copy of the exe in
  `work\`, never the install), and a curve point changed.

---

## 13. Offline oracles

### 13.1 Pure logic (`mw2er-core`), required test categories (≥ 1 named test each, matched by prefix)

- `pmove_cmd_scale_*`: wishspeed known answers in in/s:

  | Input | Wishspeed |
  |---|---|
  | W, W+A, W+D | 190 |
  | A, D | 152 |
  | S | 133 |
  | S+A | 152 |
  | sprint W, sprint W+A | 285 (sprint W+A direction atan(84.7/127) = 33.7°) |
  | crouch W | 123.5 |
  | crouch A | 98.8 |
  | crouch S | 86.45 |
  | prone W | 28.5 |
  | ADS UMP W | 152 |
  | ADS rifle W | 76 |
  | ADS crouch UMP | 98.8 |
  | lightweight W | 203.3 |
  | mg class W | 166.25 |

- `pmove_accel_*`: frames 1..11 at 60 Hz equal 2.4's list (±1e-3), 190 reached at frame 11; 30 Hz and 144 Hz reach
  190 within 0.20 s; air accel 1 changes a 190 in/s run by less than 1 in/s when W stays held.
- `pmove_friction_*`: stop from 190 at frame 18 (60 Hz); 0.30 ± 0.035 s at 30/144 Hz; speed < 1 → 0.
- `pmove_jump_*`:
  - apex = 39.0 ± 0.1 in and t_apex = 0.312 ± one tick at 30/60/144 Hz;
  - flat air time 0.6245 ± one tick;
  - a running jump covers 118.6 ± 2 in;
  - < 500 ms after the last jump is refused;
  - holding Space does not re-jump (a new press is needed);
  - crouch + Space stands up and does not jump;
  - a ceiling 20 in above the head stops the rise.
- `pmove_land_*`:
  - flat running jump lands at 82.8 in/s with slowdown, 127.3 without;
  - a re-jump at pm_time 1800 → v0² ÷ 2.5 (v0 = 158.0 in/s);
  - at pm_time 850 → factor 1.75 (v0 = 188.8 in/s);
  - landing below origin+18 → ×0.65/1800 ms, above → ×0.5/1200 ms.
- `fall_*`:
  - 200 in → 0 %, 275 in → 50 %, 349.9 → 99.9 %, 350 → 100 %;
  - h from v_impact (565.7 in/s → 200 in, 748.3 → 350);
  - TIME_LAND ms = min(500 + 35·dmg, 2000);
  - non-lethal never below 1 HP;
  - `fall_damage = off` → no HP change;
  - free fall 30 m → `freefall`.
- `sprint_*`:
  - full meter lasts 4.000 s ± one tick and ends `why=time` with the exhausted flag;
  - 1:1 recharge (stop at 2 s → 3 s left one second later);
  - refused below 1 s, accepted once remaining > 1 s;
  - press mode: release keeps sprinting, a second press stops, button-up is required;
  - hold mode: release stops;
  - every `why` in 2.5 ends it;
  - fire held blocks the start;
  - start from crouch/prone stands up (headroom ok) or is refused (`why=headroom`);
  - sprintDurationScale 2 → 8 s; marathon never ends by time;
  - strafe ×0.667 while sprinting;
  - meter keeps recharging while suspended.
- `stance_*`:
  - curve known answers (51.2 in at 50 % stand→crouch, 15 in at 50 % crouch→prone, endpoints exact);
  - durations 200/200/400/400 and stand→prone 600 via crouch;
  - every key table cell of section 3;
  - headroom blocks the raise;
  - ratio vs mw2 eye heights;
  - crouch↔prone speed blend linear;
  - prone step 10 in.
- `move_collide_*`, with a synthetic `RayWorld` of boxes, planes and ramps (exact ray-AABB and ray-plane hits):
  - walking 45° into a wall keeps the tangent (|v_t| within 1 %) and never comes closer than `skin` − 1 mm;
  - a 90° corner stops;
  - stairs 0.2 m rise / 0.3 m tread are climbed at walk and sprint speed;
  - a 0.5 m block is a wall standing and a 0.3 m block is a wall prone;
  - a 30° ramp is walkable both ways without leaving the ground going down (snap);
  - a 50° ramp slides down;
  - walking off a 2 m ledge falls and lands (fall_h ≈ 78.7 in, no damage);
  - a 0.05 m thin wall is not crossed at sprint speed at 15/30/60 Hz, nor at 3× sprint;
  - a 50 m/s fall onto a floor stops on it at 30 Hz;
  - a ceiling hit stops the rise;
  - the ray count per tick never exceeds `max_rays_per_tick`, and budget exhaustion drops only the horizontal part.
- `move_logic_*`:
  - Off → Arming → Driving after 10 clear grounded ticks; not while ER is airborne;
  - one test per reason of 9.1 returning that reason; simultaneous triggers follow the priority;
  - no position, proxy or gravity write on a trigger tick (the write plan is empty);
  - a stance reset per reason, kept only for `menu`.
- `move_logic_gravity_*` (`GravityOwner` round trip, table-driven over every reason):
  - save → own → restore leaves the fields equal to the saved values;
  - no restore if the field changed (`skipped=changed`);
  - deferred out of world, restored on return only at the same address, dropped on another address (`stale`);
  - re-assert counting;
  - the panic path restores.
- `move_logic_resync_*`: rebase (chunk continuous) keeps velocity; warp zeroes it and ends the sprint; 4.9 m is not a
  jump; handback and world entry resync.
- `move_logic_pushout_*`: an opposite component is applied and clamped to 0.3 m, a same-direction one is ignored,
  > 0.3 m gives `er_move` with a 0.5 s hold, and character circle push-out is limited to 0.2 m per tick.
- `move_input_*`:
  - `dik_from_name` for every new name;
  - duplicate-key rejection;
  - the AC-130 overlap allowed;
  - blank sets per context (FP driving incl. W/A/S/D, TP hybrid without, suspended, MW2 off);
  - Space held across a handback stays blanked until released, W/A/S/D released at once;
  - press counters: 3 taps inside one 33 ms tick count 3;
  - inert with a stale heartbeat.
- `move_axes_*`: `right_h` projects to +x through `ac130_hud::project` with a real CAMFRAME from
  `tools\scripts\fixtures\m3_run3_session.log`; W moves along the camera's horizontal forward; `strafe_sign = −1`
  flips it.
- `move_aim_chest_*`: crouch and prone keep the eye-to-chest distance at 0.40 m, so `check_shot` passes.
- `vm_anim_sprint_flag_*`: the flag drives in/loop/out; fire during sprint → out 0.3 s then the fire gate opens; ADS
  likewise; the heuristic applies only with `None`.
- `vm_pose_*`: stance offsets converge at posMoveRate (63 % after 1/6 s); prone uses 25; bob amplitude per stance and
  0 in the air.
- `spread_stance_*`: min/max per stance; the jump add; the move threshold of 11 in/s.
- `movement_data_*`: json model round trip, a missing field → `no_data`, label values only `V`/`I`.
- `config_movement_*`: defaults, bounds, `data` cold, unknown key → error, `yaw_forward_axis`, `hud.stance_icon`.
- `fp_math_yaw_gate_controller_*`: A/S/D do not pause the yaw while driving in FP; lock-on still does.

### 13.2 Headless bench `mod\movebench` (new workspace member, offline only, never linked into the DLL)

`movebench --check` runs the real `mw2er-core` controller (pmove + collide + stance + sprint + fall) on scripted
inputs in a synthetic test map, at 30, 60 and 144 Hz, and compares with hand-computed MW2 numbers. It writes
`work\m5\bench.json` and exits non-zero on any miss.

| Scenario | Expected (all three rates) |
|---|---|
| B1 walk W 5 s | steady 4.826 ± 0.5 % m/s; 190 in/s reached by 0.20 s |
| B2 strafe A / back S | 3.861 / 3.378 m/s ± 0.5 % |
| B3 sprint (walk W 1 s, then tap Shift, keep W) | 7.239 m/s ± 0.5 %; ends at 4.00 ± 0.034 s `why=time`; sprint distance 28.86 ± 0.3 m (1140 in minus the 190→285 ramp of ~4 in) |
| B4 sprint recharge | tap again at 4.5 s: refused; at 5.05 s: accepted |
| B5 crouch / prone walk | 3.137 / 0.724 m/s; eye 1.067 / 0.293 m after 0.2 / 0.6 s |
| B6 standing jump | apex 0.991 ± 0.003 m; air time 0.6245 s ± one tick |
| B7 running jump | landing speed 2.10 m/s (82.8 in/s) with slowdown; 3.01 ± 0.05 m covered |
| B8 stairs 10 × 0.2 m | climbed at walk in < 2.5 s; feet never below a tread |
| B9 corridor slide at 30° to a wall, sprint | no penetration; tangential speed ≥ 0.85 × sprint × cos 30° |
| B10 6 m drop / 9 m drop | 24.1 % damage (h = 236.2 in) / lethal (354.3 in) |
| B11 stop from sprint (release W at 285 in/s) | 60 Hz: 258.875, 235.145, 213.59 in/s for frames 1-3, below 100 at frame 11 (98.98), 0 at frame 22 (0.367 s); 30/144 Hz: 0.367 ± 0.035 s |
| B12 thin wall 5 cm at 3× sprint, 15 Hz | not crossed |

`movebench --perf`: 10,000 ticks of B9 at 30 Hz with zero-cost rays. Pure CPU p95 < 50 µs per tick, and
`rays_max ≤ max_rays_per_tick`.

### 13.3 Analyzer (`analyze_log.py`, new rows; logs without `MOVE cfg` give N/A)

| Row | PASS needs |
|---|---|
| `movement` | ≥ 1 `MOVE on`. Every `MOVE on` is followed by `MOVE off`/`suspend` before the next `TASK world left`. Every gravity write has a matching `MOVE restore ... ok=1` or `deferred` → `ok=1`/`stale`. Every suspend/off carries a reason. No PANIC |
| `movement speeds` | From `MOVE state` lines with `pushout=0` and ground=1 (≥ 5 samples per state that occurred): `speed_world` within 5 % of the hand values (stand W 4.83, strafe 3.86, back 3.38, sprint 7.24, crouch 3.14, prone 0.72, ADS 3.86) and within 5 % of `speed_sim`. States with < 5 samples → NEEDS-DATA |
| `sprint` | Every `sprint end why=time` has `dur` within 4.00 ± 0.05 s (× sprintDurationScale). No sprint starts with meter ≤ 1.0. No `MOVE SHOT` / `SHOT n=` within sprintOutTime after a `sprint end why=fire` |
| `jump` | Every `MOVE apex` with no ceiling has h = 0.991 ± 0.03 m |
| `fall damage` | `MOVE land` dmg_pct matches the formula from fall_h (±0.5 %); hp delta matches dmg_pct × max_hp (±1). Lethal → a `death` suspend within 2 s, else FAIL `lethal_fall no_death_seen`. No death within 3 s after a non-lethal landing |
| `move perf` | PERF windows with move=1: ticks_per_s avg ≥ 55 (or ≥ 95 % of the move=0 average in the same session), move_us_p95 < 1000, rays_max ≤ max_rays_per_tick |
| `move keys` | INFO: lists the `MOVE keyprobe` results and any ER bit seen for a non-blanked key |

The self-test fixture `tools\scripts\fixtures\m5_move_session.log` is synthetic and uses the exact line shapes.
These mutations must FAIL: a missing gravity restore, sprint `dur=4.6`, apex 1.3 m, sprint speed 6.0 m/s, PERF avg
40 with move=1, a non-lethal landing followed by death, a SHOT 0.1 s after a sprint end by fire. Older fixtures
(M2-M4, run 8) give N/A on the new rows.

### 13.4 Wiring checks (`m2_static_checks.py movewiring`; every self-test mutation must FAIL)

1. `physics + 0x70`, `+0x91`, `+0x1cc`, `+0x1d5` and `hp` are written only in `movement.rs`, only through
   `game::write`. `WriteProcessMemory` is still absent from the crate, and `VirtualQuery` still appears only in the
   page-cache check.
2. The gravity write function registers the save in `GravityOwner` before writing (the save precedes the write in
   source order).
3. Every `move_logic::Reason` variant reaches `release()` (one match, no `_ =>` arm).
4. `movement.rs` calls `map_ray_point` only through the budget counter (`RayBudget::cast`).
5. `task.rs` calls `move_ctl.emergency_off()` in the panic path, before FP's and the AC-130's.
6. The camera-step detour reads `MOVE_SEQ` twice around the feet, and does no allocation, log or lock.
7. The HP write appears in exactly one function, guarded by `fall_damage != Off`.
8. The input detour bumps the `PRESSES_MOVE` counters before any blanking.
9. `movebench` and `tools\iw4dis` are not dependencies of `mw2er` (the DLL contains no `iced` string).

### 13.5 `tools\scripts\check_m5.ps1` (PowerShell 5.1; never launches anything; writes only `work\check_m5\`, `work\m5\` and what the sub-checks write)

Same structure and helpers as `check_m4.ps1` (`Add-Result`, `Invoke-Tool`, `Run-Check`, `-AllowSkip`, `-SkipM4` for
fast iteration that always exits non-zero). PATH and Python at the top:

```powershell
$env:Path = [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mw2Exe = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2\iw4sp.exe'   # read only
```

| # | Check | Pass criterion |
|---|---|---|
| 1 | M4 suite | `check_m4.ps1` exit 0 and its last line `M4 OFFLINE ORACLES: PASS` (15 PASS + 1 NEEDS-DATA today, incl. check_m3b 8, m3 18, m2 21) |
| 2 | cargo build | `cargo build --release --workspace` (incl. `movebench`) with 0 warnings under `mod\` |
| 3 | cargo test | `cargo test -p mw2er-core` passes with ≥ 1 test per 13.1 category (name-prefix match); the `mw2er` lib tests `game_write_fast_and_safe` and `hud_gunship_frame_uses_one_background_draw_list` still pass |
| 4 | bench | `movebench --check` exit 0, every B1-B12 scenario PASS at 30/60/144 Hz (`work\m5\bench.json`) |
| 5 | bench perf | `movebench --perf` p95 < 50 µs and `rays_max ≤ 32` |
| 6 | converter | `tools\iw4dis` builds (short CARGO_TARGET_DIR); `convert_movement.py --check` exit 0 with every known answer (exe rows NEEDS-DATA only if the exe sha changed, reported); `--self-test` catches all 5 mutations; `movement.json` hashes match its inputs |
| 7 | config | `mw2er.toml` parses with tomllib and the DLL parser; `[movement]`, `keys.move_*`, `[first_person] yaw_forward_axis`, `[hud] stance_icon` present with the documented defaults; an unknown `[movement]` key is rejected; duplicate movement keys are rejected |
| 8 | wiring | `m2_static_checks.py movewiring --self-test` (13.4) |
| 9 | DLL proof strings | built == staged `mw2er.dll`; contains `MOVE on`, `MOVE off reason=`, `MOVE suspend reason=`, `MOVE restore field=gravity_multiplier`, `field=CSChrPhysicsModule.position(+0x70)`, `gravity_disabled(+0x1d5)`, `chr_proxy_pos_update_requested(+0x91)`, `MOVE sprint end why=`, `MOVE apex`, `MOVE land`, `MOVE resync why=`, `MOVE keyprobe`, `move_us_p95=`; no `iced` / `movebench` string |
| 10 | analyzer | `analyze_log.py --self-test` exit 0, incl. the M5 fixture and its mutations (13.3); the older fixtures still give their previous results |
| 11 | hygiene | `converted\movement.json` is gitignored; no line of ≥ 20 non-space characters from any `.cfg`/`.gsc` under `dumps\m5` appears under `mod\` or `tools\scripts\`; `convert_movement.py` and `iw4dis` open `iw4sp.exe` read-only (static check: no `subprocess`/`CreateProcess`/`Command::new` naming the exe, Python opens with `'rb'`); no new file in the ER or MW2 install folders since the last check (top-level mtime scan, read only) |

The final line is `M5 OFFLINE ORACLES: PASS` only when all 11 PASS (a NEEDS-DATA inside check 6 is reported and does
not fail).

---

## 14. In-game test steps (run 11; HDR off; me3 via `launch.ps1`)

1. Load in. Expect `MOVE cfg ...` and `MOVE data ...` once.
   - **Key probe, MW2 off**: press Space, Left Shift, C, Left Ctrl, F, X, Z, Q and Left Alt once each, standing
     still. Each logs `MOVE keyprobe key=… er_bits=…`. ER may roll on Space or crouch on X; that is expected with MW2
     mode off.
2. Press F5 (MW2 on). Within about 10 ticks: `MOVE on`, the four `MOVE write field=…` lines (position, proxy
   request, gravity_multiplier old=1.000 new=0.000, gravity_disabled old=0 new=1). First person comes on as before.
3. Walk with W for 3 s, then A, D and S. The view moves at a steady MW2 pace. **D must move you to the screen's
   right** (otherwise set `strafe_sign = -1`, hot). Look for `MOVE state ... speed_world≈4.83` (W), 3.86 (A/D) and
   3.38 (S). Stop: you stop in about 0.3 s, with no ER slide.
4. Tap Left Shift while holding W and let go of Shift: you keep sprinting (about 7.2 m/s) with the sprint viewmodel
   loop. After 4 s it ends (`MOVE sprint end why=time dur=4.00`). Tap Shift at once: `refused why=meter`. Wait 1 s
   and tap again: it starts.
5. Sprint, then click LMB: the sprint ends (`why=fire`), the gun lowers out of sprint (0.3 s), then fires. Repeat with
   RMB (ADS) and S (`why=forward`).
6. Space: one jump of about 1 m (`MOVE apex h≈0.99m`). Jump on the spot, and while running: the landing slows you
   briefly (`MOVE land ... slow=0.67*0.65`). Hold Space: no second jump until you press again.
7. C: the view lowers to crouch in 0.2 s, speed ≈ 3.14, crosshair tighter, gun shifts right. C again: stand. Left
   Ctrl: prone (0.6 s), view near the ground, crawl ≈ 0.72 m/s, gun out of view while crawling. Space from prone:
   stand. Under a low ceiling, standing up is refused (`stance blocked ... why=headroom`).
8. Walk up stairs (e.g. a church or a ruin), along a wall at an angle (you slide along it), and up and down a hillside.
   No sticking, no floating, no falling through. Watch `rays=` and `er_delta` in the state lines.
9. Drop 3-4 m from a ledge: no damage. Drop about 6-7 m: some damage (`MOVE land ... dmg_pct=…`). A drop of 8.9 m
   or more: `MOVE lethal fall predicted` then `MOVE suspend reason=freefall`, and ER resolves the fall itself (default
   `lethal_fall = "handback"` since the review; `"zero"` writes HP 0 instead).
10. Press F6 (third person). Moving now shows ER's walk/run animation, with the body facing the move direction, at
    MW2 speed. Sprint shows the run animation, faster. Note how it looks.
11. Handbacks, each must log `MOVE suspend reason=…`, `MOVE restore field=gravity_* ... ok=1`, and come back with
    `MOVE on` / `resync why=handback`:
    - climb a ladder (`ladder`);
    - summon Torrent and ride, then dismount (`torrent`);
    - open a door or pick up an item with E (`interact`);
    - rest at a grace (`talk`);
    - open the menu (`menu`, stance kept);
    - get hit hard by an enemy (`er_move` if knocked back);
    - F7 AC-130 ride (`ac130`).
12. Fast travel (`world_left` / `loading`, deferred restore, then `resync why=world_enter`). F5 off: `MOVE off
    reason=f5` with both gravity restores, and Space/Shift/C/Ctrl/F/X are ER's again. Space then dodges as normal.
13. Quit and run `python -I tools\scripts\analyze_log.py`. The new rows `movement`, `movement speeds`, `sprint`, `jump`,
    `fall damage` and `move perf` must PASS (NEEDS-DATA if a state was not tried), plus every older row.

The final oracle is the user's own: "does it feel like MW2?" Tune `speed_scale`, `jump_slowdown`, `view_bob` and
`tp_anim` hot.

---

## 15. What stays unverified until the user's run

- That `position` + `chr_proxy_pos_update_requested` written in `ChrIns_PostPhysics` sticks without ER
  interpolating back. If it does not, the fallbacks are `proxy_sync_flag`, then `write_last_update`.
- That gravity_multiplier 0 + gravity_disabled 1 prevents ER's fall death on 1.17.1 in this mod (CS2 [R]). Also how
  often ER rewrites `gravity_disabled` (`gravity_reasserts`).
- The frame order (`MOVE order`), and that the published feet remove the eye jitter.
- `cast_ray` cost per call, and whether filter 0x2000058 sees stairs, bridges, fog walls, doors and breakables.
  `er_delta` statistics will show what it misses.
- The slope normal from rays vs `material_info.normal_vector`, and the 0.7 walkable limit (inferred).
- `chunk_position` continuity across a rebase.
- Whether an HP write of 0 triggers ER's normal death.
- The live ER key preset and rebinds (`MOVE keyprobe`), and that blanking LShift has no side effect on ER's
  attacks (LMB/RMB are ours in MW2 mode anyway).
- How the hybrid third-person animation looks at MW2 speeds.
- The MP-only values: `jump_slowdownEnable` (assumed on) and the fall heights. iw4sp's PM_CrashLand has both
  branches: 200/350 in, lethal (bit 12 of `[r14+48h]` clear, used) and 128/300 in, at most 10 % (bit set; recorded as
  `info_fall_*_alt`, meaning of the bit unknown). `fall_damage_scale` stays the fallback if the feel differs.
- Known gaps, by design:
  - the gamepad is not blanked and is ignored by the controller;
  - ER's hurtbox and enemy sight stay standing while crouched or prone;
  - no mantling, leaning, Commando or dolphin dive;
  - ER's own crouch animation is not driven in third person.

---

## 16. Decisions the implementer must not change (non-negotiables)

1. Never launch ER, MW2, me3 or Steam. `iw4sp.exe` is read only as a file (static parse and disassembly). Never
   write into a game folder or `%APPDATA%\EldenRing`. Never publish or push. Do not touch `D:\Mods\MW2inER-m3`.
   Downloads only via cargo/crates.io. Converted data only in `converted\`, test output only in `work\`.
2. Every runtime write goes through `game::write` (page-cached check + `NtWriteVirtualMemory`, version-gated). The
   writes are exactly those of 4.2. Never `WriteProcessMemory`, and never `VirtualQuery` per write. Each write is
   logged once per episode with field, address, old and new. Gravity fields are saved before the first write and
   restored on **every** exit path of 4.3 (F5, config, every handback, AC-130, world left/loading deferred, panic),
   only on the same module address and only if the field still holds our value. The HP write (fall damage) is the
   only unrestored write; it is optional, once per landing, never raises HP, and is logged.
3. The controller runs in the existing `ChrIns_PostPhysics` task tick, with one read pass shared with FP and the
   AC-130. No new hooks and no writes from the camera detour. The detour only reads the published feet (seq-checked,
   within 1 m of `physics.position`).
4. On a tick with any handback trigger, no position, proxy or gravity write happens. Re-arming needs `rearm_ticks`
   clear ticks with ER grounded. Every resync takes ER's position, never a stale one across a > 5 m jump.
5. MW2 numbers come from `converted\movement.json` built from the user's dumps and exe with recorded sources. No
   typed fallbacks; no data means `Off(no_data)`. The pmove core runs in IW4 inches/seconds, and the inlined constants
   (39 in, 5.5, 100, 200/350) are used, not the unread dvars. `speed_scale` is the only non-MW2 tuning knob in the
   physics.
6. Sprint follows PM_UpdateSprint exactly in `press` mode: tap to start, release keeps it, tap again to stop, button-up
   required, 4 s × sprintDurationScale, 1 s minimum, 1:1 recharge, and every end reason of 2.5. Firing and ADS are
   blocked during the sprint and for sprintOutTime after it.
7. Default keys are the MW2 PC binds: LShift sprint, Space jump/stand, C crouch toggle, LCtrl prone toggle, W/A/S/D.
   ER-conflicting keys are blanked only while the controller drives, latched until release. W/A/S/D are never
   latched, and are blanked in first person only. E always reaches ER.
8. Third person default is the hybrid (ER animates, we move). First person blanks W/A/S/D and writes yaw without the
   A/S/D pause.
9. While the controller drives, the aim's chest reference follows the eye (8.5), so crouched and prone shots pass the
   M2/M3 distance checks.
10. Pure logic lives in `mw2er-core` (`forbid(unsafe_code)`) with every test category of 13.1. `movebench` and
    `iw4dis` are never linked into the DLL. `check_m5.ps1` must end in `M5 OFFLINE ORACLES: PASS`, with
    `check_m4`/`check_m3b`/`check_m3`/`check_m2` all PASS and `cargo build` at 0 warnings.
11. The CS2 note, erfps2 and fromsoftware-rs remain references or vendored. RVAs are attributed in source and MODLOG;
    no code is copied.

---

## 17. Review applied (2026-10-17; 4 major, 6 minor; none rejected)

Labels as in "Evidence labels": [V] offline-verified, [I] inferred / unverified in game.

1. **Ground disagreement (major 1).** `move_sim::Input.er_ground` = ER's `is_touching_ground` read this tick. While
   grounded, a ground-ray miss starts a fall only if ER also reports no ground; otherwise the feet are kept
   (`TickStats.ground_held`). 30 such ticks in a row (0.5 s) -> `MOVE ground_disagree ...` and the new suspend reason
   `ground_disagree` (priority after `er_move`, hold 3 s). Fall damage is written only if ER saw the player airborne for
   the fall: `Player.er_ground_air` counts ticks after the 3-tick grace with ER ground; >= 2 -> `MOVE land ...
   fall_damage=skipped_er_ground er_ground_ticks=N` (no HP write; analyzer FAILs if HP changes on such a line).
   `lethal_fall` default is now `"handback"`. Tests: `move_collide_ground_miss_kept_while_er_grounded`,
   `fall_damage_needs_er_airborne_for_the_fall`, `move_logic_ground_disagree_*` [V]; how often ER's flag disagrees on
   real ledges / lifts [I] (`ground_held_ticks=` on the er_delta line).
2. **No physics world (major 2).** `tick()` reads `phys_world()` once; 0 = a failed read (`read_fail` after 3 ticks,
   field `CSHavokMan.phys_world`), no sim and no write. `drive()` keeps the guard, and the rays ignore `r.player` (the
   main_player whose physics module is written; `MoveIn.player_ins` removed). Wiring check 4 [V].
3. **Push-outs through collision (major 3).** ER's push-out (Apply) and the character push-out go into
   `Input.push` (planar inches) and ride on the first substep's move through the collide-and-slide and its ray budget
   (a spent budget drops them). Character radius `[movement] char_push_radius_m = 0.45` (not `aim.char_radius_m`
   0.75). Test `move_collide_pushout_stopped_by_wall` at 30/60/144 Hz; wiring check 4 forbids `pl.pos = pl.pos +` [V].
4. **Position sanity (minor 4).** `move_logic::position_write_ok`: finite and within 5 m of ER's position, else
   `MOVE WARN position_insane ...`, resync to ER, no write, `write_fail` handback next tick [V offline].
5. **Seqlock (minor 5).** `publish_feet` stores an odd sequence, release fence, the feet, then the next even one;
   `move_feet` rejects odd values and changes (acquire fence before the second read). Wiring check 6 [V].
6. **Panic path (minor 6).** `MoveController::disabled_tick()` retries a deferred gravity restore every tick while
   gameplay is disabled (guarded, like the AC-130's). An unreadable module stays deferred quietly [V wiring].
7. **Interactions (minor 7).** `interact_hold_s` 1.0 -> 1.5 s, and the hold is extended while ER's
   `(anim_flags, mod_flags)` differ from their value while we last drove, with W/A/S/D released, at most 8 s after the
   press [I: which bits mean "interacting" is unverified; the probe line logs them].
8. **Per-tick allocations (minor 8).** cfg line and key masks are rebuilt only when the task's config generation
   changes; the probe line compares plain values before formatting; the state-line key is a Copy tuple; `others` and
   `events` are reused buffers.
9. **Provenance (major 9).** Labels V / D / I: D = decoded by the research disassembly, not byte-checked (31 rows:
   bob amplitudes, TIME_LAND formula, landing scales, view dip, cl_stanceHoldTime, jump_spreadAdd, ...).
   `ads_walk_scale` is I. `sprint_forward_min` (lea edx,[rdi+69h] @0x1400755B9, xor edi,edi @0x140074C85 with no
   branch or rdi write in between, name `player_sprintForwardMinimum` @0x1400755CE) and `sprint_recharge_pause_s`
   (xorps xmm1,xmm1 @0x1400756CB, name @0x1400756B3) are now byte-checked V sites. A V row whose src is a decode
   only fails `--check`. iw4dis checks 55 sites. Rust accepts V / D / I.
10. **Second fall branch (minor 10).** `info_fall_flag_bit` 12 (bt eax,0Ch @0x140078C91, eax = [r14+48h]),
    `info_fall_min_alt` 128, `info_fall_max_alt` 300, `info_fall_range_alt` 172, `info_fall_scale_alt` 10,
    `info_fall_cap_alt` 10 (lea esi,[rbx+0Ah], ebx = 0) [V bytes]; the meaning of the bit is unknown [I]. `fall_min` /
    `fall_max` src notes the assumption that the bit is clear for a normal player.