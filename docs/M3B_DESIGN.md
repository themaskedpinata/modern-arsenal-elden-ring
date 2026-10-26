# M3b design: a real first-person view while MW2 mode is on

Status: design, 2026-10-14. Built from the three research findings of this workflow (camera-write, hide-body,
yaw-and-handbacks), `MODDING_PLAN.md`, `MODLOG.md`, `docs/M2_DESIGN.md`, `docs/M3_DESIGN.md`, the vendored
fromsoftware-rs 59fbd3b and the run-3 log. Nothing here has run in game yet.

User feedback that started M3b (run 3): *"the gun showed up in first person but the character remained"*. ER's
third-person camera sits about 3.5 m behind the player (CAMFRAME `cam_chest=3.50m`), so the player character
stays visible behind the MW2 viewmodel.

M3b goals:
1. Put the render camera at the player's eye, keeping ER's mouse-look rotation.
2. Hide the local player's body, and only the local player, with no stealth side effect.
3. Turn the body to the aim direction where that does not fight ER's locomotion.
4. Hand back to the vanilla third-person camera, exactly, on every unsafe state.
5. Leave the world FOV as ER's by default and log it. The MW2 viewmodel keeps 65/55.
6. Fix the analyzer: classify shield blocks instead of failing on them.

## Evidence labels

Same labels as M2/M3:

- **[V] verified**: checked on this PC by a file read, a parse, a compile, a log line or an offline run.
- **[R] reported**: another mod's source (erfps2 57e92c1, MIT/Apache-2.0, read-only clone at
  `D:\Mods\_ext_ro\erfps2`) or a crate doc comment says so. Not tested here.
- **[I] inference**: our design choice or a guess. Each one has a log line that settles it in the user's first run.

---

## 0. Summary of the decisions

| Topic | Decision | Label |
|---|---|---|
| Camera write (default `camera_mode = "task"`, Tier A) | In our existing `ChrIns_PostPhysics` task (group 113, after `CameraStep` 106, before `Draw_Pre` 144): read `pers_cam_1` first, exactly as now, so the gate, aim and CAMFRAME see ER's own third-person camera. Then overwrite **only matrix row 3** (`CSCam+0x40`, position, w = 1) with the eye. ER's rotation rows stay, so mouse look keeps working | V offsets and order; I that the render reads this value |
| Camera write (fallback `camera_mode = "hook"`, Tier B) | MinHook on ER's camera-step update (RVA `0x3bc5f0` on 2.7.1.0, prologue bytes checked at runtime before hooking). The detour calls the original, then does the same row-3 write. Off by default, and a one-line config change | V RVA prologue on the user's exe; R erfps2; I behaviour |
| Camera fight | We never touch `chr_cam`, `ChrExFollowCam`, LockCamParam or the collision/wall-recovery state, so ER's distance and collision logic has nothing to fight. The regulation route (LockCamParam `camDistTarget` ~0) was evaluated and rejected | V/I (section 2.4) |
| Restore of the camera | ER is expected to rebuild `pers_cam_1` every frame, which makes the restore free. This is proven per session: the `FPCAM er_rewrite=1|0` self-check. If ER does not rewrite it, the first handback writes ER's last value back exactly, and first person is disabled for the session (`cam_persist`) | I, proven by log |
| Pitch | ±85° clamp in pure math. It is a no-op while ER's own pitch limit is narrower. If it ever triggers, rows 0..2 are rebuilt with det = +1 | I |
| Near plane | Untouched: ER's near plane is 0.0501 m in all 141 CAMFRAME lines of the log, inside the 0.10 m eye-forward offset | V log |
| Body hide (default `hide_method = "render_flag"`) | Clear `ChrIns.chr_flags1c5` bit 3 `enable_render` (ChrIns+0x1c5) on `WorldChrMan.main_player` only. Read-modify-write of that one bit. Re-cleared every tick if the engine sets it again (counted). On handback the saved bit goes back, only if the player pointer is unchanged | V field doc; I render-only effect |
| Body hide (alternative `"fade"`) | `base_transparency_modifier` (+0x250) = -1, so the engine fades `base_transparency` (+0x24c) to 0. Restore = +1 until the alpha is back to ~1, then the saved modifier | V field doc; I |
| SpEffect / camouflage | Rejected: every player-usable invisibility row also lowers enemy sight (`sightSearchEnemyRate` 0.1..0.5, `stateInfo` 8/95), and SpEffects sync over the network | V regulation scan |
| Body yaw | Write a yaw-only quaternion to `CSChrPhysicsModule.orientation` and `.interpolated_orientation`, only when no A/S/D key is held and no dodge/turn-lock/lock-on/throw is active. First a probe learns the character forward axis (+Z or -Z) while W is held. Handback = stop writing (release policy, nothing is written back) | V fields; I behaviour |
| Handbacks | A pure state machine in `mw2er-core::fp_logic` (Off -> Arming(30 ticks) -> On). Any trigger switches off in the same tick, with one reason chosen by a fixed priority. The reason is logged | I logic; V/I signals (section 5) |
| Writes | All writes go through `game::write`, which uses WriteProcessMemory on our own process (fails instead of faulting) and refuses until the version gate has passed. Every field is logged once per first-person episode with its address and old/new value | I |
| FOV | ER's world FOV is untouched by default and logged (48.0° in play). `world_fov_deg > 0` (default 0 = off) writes `pers_cam_1.fov` under the same restore model as row 3. The viewmodel keeps hip 65 / ADS 55 | V log; I |
| Shots in first person | Direction = ER's row 2 (as now). Origin = `bullet_origin(eye, fwd, chest, origin_ahead_m)`, so the bullet leaves the screen centre. The validity checks still use ER's third-person camera position | I |
| Analyzer | Hits on a reference enemy with dmg < 20% of the band baseline are `blocked?` and excluded from calibration. The run-3 close-range c4311 hits become the measured baseline of band 79000100. New `first person` row | V log; I rule |

---

## 1. Files M3b creates or changes

```
docs\M3B_DESIGN.md                              this file
mod\mw2er-core\src\fp_logic.rs                  (new) state machine, reasons, edge watches, fade-plate watch
mod\mw2er-core\src\fp_math.rs                   (new) eye position, pitch clamp, yaw quaternions, yaw probe, yaw gate, drift stats
mod\mw2er-core\src\fp_restore.rs                (new) save/restore models: persist watch, render bit, fade, yaw release
mod\mw2er-core\src\config.rs                    (changed) [first_person], keys.fp_toggle
mod\mw2er-core\src\lib.rs                       (changed) module list
mod\mw2er\src\game\mod.rs                       (changed) write<T> (WriteProcessMemory, version-gated)
mod\mw2er\src\game\fp_read.rs                   (new) every first-person signal read, plain data
mod\mw2er\src\first_person.rs                   (new) the controller: machine + writes + logs
mod\mw2er\src\cam_hook.rs                       (new) Tier B camera-step hook (installed only for camera_mode = "hook")
mod\mw2er\src\{task.rs,shared.rs,input_hook.rs,boot.rs,lib.rs}   (changed)
mw2er.toml                                      (changed) [first_person], keys.fp_toggle
tools\scripts\analyze_log.py                    (changed) blocked hits, band-100 baseline, first person row, real-log self-test
tools\scripts\m2_static_checks.py               (changed) new subcommand fpwiring (+ self-test)
tools\scripts\check_m3b.ps1                     (new) runs check_m3.ps1, then the M3b oracles
MODLOG.md                                       (changed) entry
```

No new regulation rows, no new vendored patch and no new dependency. The vendored crate already has every field
used here: `CSCamera`, `CSCam`, `ChrCam.camera_type`, `ChrIns.chr_flags1c5`, `base_transparency*`,
`is_locked_on`, the physics/ride/ladder/throw/action-flag modules, `CSFade`, `CSNowLoadingHelper`,
`CSPopupMenu.current_talk_id` and `CSPlayerMenuCtrl`. `ChrExFollowCam`, `CSRemo` and `LockTgtMan` are absent, and
M3b does not need them.

---

## 2. Camera

### 2.1 What is written [V layout, V order; I that it reaches the render]

- `CSCamera` singleton: +0x08 `pers_cam_1` (OwnedPtr<CSCam>). `CSCam`: +0x10 matrix, 4 rows of f32x4: right +0x10,
  up +0x20, forward +0x30, position +0x40. Then +0x50 fov (rad), +0x54 aspect, +0x58 near, +0x5C far
  (`vendor\fromsoftware-rs\crates\eldenring\src\cs\camera.rs:12-44`). Offsets come from `offset_of!(CSCam, matrix)`
  plus `3 * 16`, with a compile-time assert that `F32ViewMatrix` is 64 bytes.
- Written: row 3 = `(eye.x, eye.y, eye.z, w)`, 16 bytes, once per task tick while first person is On. `w` is kept
  as ER had it (expected 1.0), so only the position changes.
- Task order (`vendor\...\cs\task.rs` enum order; `mod\mw2er\src\task.rs` registers in `ChrIns_PostPhysics`):
  `CameraStep` (106) < **our task** `ChrIns_PostPhysics` (113) < `Draw_Pre` (144) < `DrawStep` (153). The task runs
  once per frame: 300 ticks per 5 s in the run-3 CAMFRAME lines **[V log]**.
- In-task order (new steps in **bold**):
  1. `read_camera()` exactly as today. The gate, CamSignTracker, CAMFRAME and aim see ER's third-person camera.
  2. Menus, toggles (F5 and **F6**), characters.
  3. **First-person controller step**: read the signals, run the state machine, apply or release the writes.
  4. Firing: in first person the origin comes from the eye (section 2.6).
- Never written in Tier A: `chr_cam.pers_cam`, `ChrExFollowCam`, `CSCamera.camera_mask`, the CAM_WALL_RECOVERY
  global. erfps2 writes `chr_cam.pers_cam` row 3 too, and then has to force `reset_camera` and clear the wall
  recovery global so ER's follow camera does not interpolate and collide from the eye point. That is the
  "camera fights the write" trap, and we avoid it by leaving ER's own camera state alone **[R erfps2 core.rs:339-363,
  hooks.rs:165-174]**.

### 2.2 Restore model and the self-check [I, proven by log]

- Inference: `CameraStep` rebuilds `pers_cam_1` from the chr cam every frame. erfps2 skips the original
  `CameraStep` precisely so it can own `pers_cam_1` **[R]**, and our log shows `pers_cam_1` tracking the chr cam
  continuously in play **[V log]**.
- Self-check (`fp_restore::PersistWatch`): before each write, row 3 is compared with what we wrote on the
  previous tick (exact bits).
  - It differs: ER rewrote it. Logged once per session as `FPCAM er_rewrite=1`. The restore needs nothing:
    stop writing, and the next frame is ER's.
  - It equals our last write: the write persists. Logged as `FPCAM WARN er_rewrite=0`. It would poison the gate
    read on the next tick. Handback with `reason=cam_persist`: ER's last pre-write value is written back once
    (`FP restore field=CSCamera.pers_cam_1+0x40 old=<eye> new=<er>`), and the camera write is disabled for the
    session. The log tells the user to try `camera_mode = "hook"`.
- The last pre-write ER value (`last_er`) is always kept, so a forced restore is possible from any state.

### 2.3 Tier B: camera-step hook [V RVA prologue; R erfps2; I behaviour]

- Target: `eldenring.exe` base + `0x3bc5f0` (erfps2 `src\rva\ww.rs CAMERA_STEP_UPDATE_RVA`), signature
  `unsafe extern "C" fn(*mut c_void, *const FD4Time)`.
- Safety:
  - installed only when `camera_mode = "hook"`, after the version gate (2.7.1.0 only);
  - the first 10 bytes must equal `48 89 5c 24 08 57 48 83 ec 30` (read from the user's exe by the researcher),
    checked with a non-faulting read, otherwise `FPHOOK FAIL prologue` and task mode is used;
  - MinHook through `hudhook::mh`, as `input_hook.rs` already does. This is the third MinHook user. It is the one
    deviation from the M3 "exactly one MinHook" rule, and only in the optional mode.
- The detour calls the original first. Then, only while the task armed it (`FPH_ACTIVE`) and the task heartbeat is
  under 250 ms old, it:
  1. reads the `pers_cam_1` matrix and the player's physics position, both from addresses the task published, with
     non-faulting reads;
  2. computes the same eye with the same pure function;
  3. checks the eye is finite and within 20 m of ER's camera;
  4. writes row 3 (the `world_fov_deg` override is written by the task in both modes, so the detour stays minimal);
  5. stores ER's pre-write position and the eye in atomics.

  It does no allocation, no lock, no log and no panic.
- The task then reads `pers_cam_1` and sees the eye. It substitutes the detour's saved ER position when the read
  position equals the detour's last eye, so the gate still sees ER's camera.
- Handback: `FPH_ACTIVE = false`, then the task writes ER's saved position back the same tick (the frame has not
  been drawn yet), and logs `FP restore`.
- Tier C (erfps2's own variant: skip the original and copy `chr_cam.pers_cam`) is **not built**. It skips everything
  else `CameraStep` does.

### 2.4 Rejected: LockCamParam regulation route [V regulation, V crate]

- LockCamParam has 166 rows (row 0: `camDistTarget=3.8`, `rotRangeMinX=-40`, `chrOrgOffset_Y=1.45`).
- The active row is switched at runtime by GameMan ids (EMEVD, TalkESD 138, TAE 150/151/155), and TAE 152 scales
  the distance.
- The pivot still trails the body (chase rate) and still collides.
- It cannot be toggled per frame or handed back for menus without runtime param writes across rows.

It is not used. The regulation is unchanged by M3b, so the M2 round trip stays as it is.

### 2.5 Eye position and pitch [I]

- `eye = physics_pos + (0, eye_height_m, 0) + normalize(fwd.x, 0, fwd.z) * eye_forward_m`:
  - `physics_pos` is at the feet (`aim.chest_height_m = 1.2` is added to it today);
  - `fwd` is ER's row 2 with sign +1 (M3);
  - defaults are 1.60 m and 0.10 m.
  - When the camera looks straight up or down, the horizontal forward is undefined, and the forward offset is 0.
- Optional `eye_smooth_s` (default 0 = off): an exponential filter on eye.y only, for stairs. A capsule-based eye
  does not jitter, unlike erfps2's head dummy poly.
- Pitch clamp: `pitch = asin(fwd.y)`. If |pitch| > `pitch_clamp_deg` (85):
  - rebuild fwd = (cos p · h, sin p);
  - right = normalize(cross(up_world, fwd)), with the sign matching the original right row;
  - up = cross(fwd, right), with the sign matching the original;
  - write rows 0..2 too (task mode only). The rows stay orthonormal with det = the original det.
- Logged as `pitch=` in the periodic FPCAM line. Expected a no-op: ER's own limit is narrower.

### 2.6 Interaction with the M2/M3 camera gate and aim [V source; I]

- The gate (`cam_gate.rs`, min 0.3 m to the chest) and the sign test stay on the pre-write sample. Eye-to-chest is
  about 0.41 m, so feeding the eye to the gate would flap. The eye is never fed to it.
- `cam_valid == false` is a handback trigger (the 145-176 m episodes and the identity placeholder after every
  load **[V log lines 773-786]**).
- Shots in first person:
  - `origin = bullet_origin(eye, fwd, chest, origin_ahead_m)`, so the bullet leaves the screen centre;
  - `check_shot` still receives ER's camera position for its distance test, plus the new origin.

### 2.7 FOV [V log]

- ER world FOV in play is 0.8378 rad = 48.0° (LockCamParam row 0 camFovY 48). It is logged in `FP on` and in the
  periodic FPCAM line.
- `world_fov_deg` (default 0.0 = keep ER's): when > 0, `pers_cam_1.fov` is written each tick with the same
  `PersistWatch` model (`FPCAM write field=CSCamera.pers_cam_1+0x50 ...`).
- The viewmodel projection keeps hip 65 / ADS 55. It never reads ER's camera.

---

## 3. Hiding the player's body

### 3.1 Primary: render-enable bit [V field and doc; I render-only]

- Field: `ChrIns.chr_flags1c5: ChrInsFlags1c5(u8)`, bit 3 `enable_render`, documented "Enables rendering for this
  character" (`chr_ins.rs:439-452`). The byte also holds `precision_shooting` (bit 1), `is_invincible` (bit 4) and
  `death_flag` (bit 7), so only bit 3 is ever changed.
- Address: `main_player + offset_of!(ChrIns, chr_flags1c5)`, with const asserts `== 0x1c5` and
  `offset_of!(PlayerIns, chr_ins) == 0`. Only `WorldChrMan.main_player` is ever touched: Torrent, enemies and NPCs
  never are.
- Enter:
  1. read the byte (a failure gives no write and `reason=read_fail field=chr_flags1c5`);
  2. save it as `RenderBit { player, saved }`;
  3. write `cur & !0x08`;
  4. log `FP hide write player=0x… field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) old=0x.. new=0x..`.
- Each tick while On:
  - re-read `main_player`. A different pointer means handback `reason=loading` (respawn/load), and nothing is
    written to the old address;
  - if bit 3 is set again, clear it again and count it. The first reassert is logged, the count again at handback.
- Handback: if the pointer is the same, write `(cur & !0x08) | (saved & 0x08)`. That is a pure function, tested for
  all 256 saved values (`fp_restore::RenderBit`). Logged `FP hide restore player=0x… old=0x.. new=0x.. reason=..`.
- Deferred restore: a handback while not in world (`main_player` null during a load) cannot read or write. The save
  is kept (`FP hide restore deferred`) and resolved on the first in-world tick, before the machine may arm again.
  If `main_player` is the same object, the bit is restored. If it is a new character, nothing is written. This
  covers a load that reuses the PlayerIns.
- Unverified until the user's run:
  - whether the engine reasserts the bit (the reassert counter shows it; if it is non-zero every tick, switch to
    `hide_method = "fade"`);
  - whether it also hides ER's weapon models (wanted) and the shadow (acceptable);
  - whether it has any non-render effect. No offline evidence of gameplay coupling was found.

  Stealth A/B: Godrick Soldiers should notice the player at the same distance with first person on and off.

### 3.2 Alternative: engine fade [V field docs; I]

- `ChrIns.base_transparency` f32 +0x24c and `base_transparency_modifier` f32 +0x250 (doc: negative fades to 0,
  positive to 1). Asserted with `offset_of!`.
- Hide: save both values, then write modifier = -1.0.
- Restore (`fp_restore::FadeRestore`, multi-tick):
  1. write +1.0;
  2. wait until `base_transparency >= 0.999` or 2 s have passed;
  3. write the saved modifier back.

  A pointer change drops the save, with no write.
- Never used: `opacity_keyframes_multiplier` (TAE 193 rewrites it), `tint_alpha_multiplier` (at 0 with
  ChrInsFlags1c7 bit 7 it disables gravity, `physics.rs:71`), `camouflage_transparency`, `force_unloaded`.

### 3.3 Rejected: SpEffect camouflage [V regulation scan]

| Row | Effect on enemies |
|---|---|
| 1467000/1467005 Unseen Form | `sightSearchEnemyRate=0.4`, `stateInfo=8`; its VFX keeps your own body at `camouflageMinAlpha=50` |
| 1767 Assassin's Gambit | 0.4 |
| 503040 Mimic's Veil | 0.5, `stateInfo` 95 |
| 99004 | 0.1 |

All of these are stealth side effects. SpEffects also sync over the network unless `dont_sync`.

---

## 4. Body yaw follows the aim [V fields; I behaviour]

- Facing lives in `CSChrPhysicsModule.orientation` (+0x50) and `.interpolated_orientation` (+0x60), both
  quaternion XYZW (`physics.rs:21-25`, `rotation.rs:6-7`).
- ER without lock-on turns the character toward the movement input and moves by root motion. Forcing
  yaw = camera yaw while A/S/D is held would make the character run forward instead of sideways **[I]**.
- Gate (pure, `fp_math::yaw_gate`). A write happens only when all of these hold:
  - first person is On;
  - the probe has resolved;
  - none of A/S/D is held (W alone or no key is fine). The raw keyboard state is read by the existing
    GetDeviceState detour, without blanking;
  - not locked on (`ChrIns.is_locked_on`, +0xC9);
  - `CSChrActionFlagModule.action_modifiers_flags` does not have `dodging` (bit 1), `disable_turning` (15),
    `disable_direction_change` (32) or `turn_lower_body` (46);
  - the throw state is not in 3..6.

  While A/S/D is held, ER turns the hidden body. The eye (capsule position + height) does not depend on body yaw, so
  the view moves sideways like an FPS strafe.
- Forward-axis probe (`fp_math::YawProbe`): the character's forward axis is not established (+Z or -Z; the crate's
  debug-line example uses -Z).
  - Samples are taken while On, with W only, planar speed > 1 m/s, not locked on and no yaw write yet.
  - Each sample is the dot of the camera's horizontal forward with q·(0,0,+1).
  - After `yaw_probe_samples` (60): a median > +0.9 gives `+z`, < -0.9 gives `-z`. Otherwise
    `FP yaw probe FAIL dot_med=…`, and a retry, at most 3 times per session.
  - The result is kept for the session.
- Write: `q = (0, sin(θ/2), 0, cos(θ/2))`, θ = atan2(f.x, f.z) for +z or atan2(-f.x, -f.z) for -z. Both quaternions
  are written (16 bytes each). They are logged once per episode with the old and new values.
- Drift oracle: before each write the current yaw is compared with our last written yaw. p50/p95 are logged every
  5 s with the pause counters (`FP yaw drift p50=… p95=… writes=N paused=M (lateral=… dodge=… turn_lock=…
  lockon=… throw=…)`). Success = p95 < 3° while idle. A large drift means ER re-turns the character before physics.
  The escalation (a second task in `ChrIns_PrePhysics`) is not built in v1.
- Restore policy = **release**: on handback nothing is written back. The facing is a dynamic game state, and
  writing an old facing back would snap the character. `fp_restore::YawRelease` documents and tests that the
  restore produces no write. The original quaternions of the episode are logged at the first write.
- Not built: fake lock-on strafing (ER's camera ignores mouse yaw while locked on) and the CS2-style kinematic
  controller.

---

## 5. Handback triggers (state machine `fp_logic`)

### 5.1 Machine [I]

- States: `Off`, `Arming(n)`, `On`.
- `wanted` = the F6 state for the session. It starts at `[first_person] enabled` and is sticky across F5. If
  `enabled = false`, F6 is refused and logged.
- Off -> Arming when `mode_on && wanted && enabled` and no trigger is active. Arming -> On after `arm_ticks` (30,
  ~0.5 s) consecutive clear ticks. Any trigger during Arming goes back to Off and is logged once per change of the
  blocking reason (`FP blocked reason=…`).
- On -> Off in the same tick as any trigger: restore everything, then log `FP off reason=<label> <detail>`.
  Transient reasons re-arm by themselves. `f5`/`f6`/`config` wait for the user.
- When several triggers fire together, the reason is the first in this fixed priority order:

| # | Label | Signal | Source | Label |
|---|---|---|---|---|
| 1 | `panic` | the task panicked (emergency restore in the panic path) | task.rs | I |
| 2 | `read_fail` | any required read returned None (`detail=field=<name>`) | game::read | V helper |
| 3 | `cam_persist` | ER did not rewrite pers_cam_1 (section 2.2) | PersistWatch | I |
| 4 | `loading` | `main_player` null (`TASK world left` **[V log 59/77]**), the player pointer changed, or `CSNowLoadingHelper.menu_load_counter` changed since On | player.rs, now_loading.rs:24 | V/I |
| 5 | `death` | `hp <= 0`, or `chr_flags1c5` bit 7, or `ChrCam.camera_type == 7` (DeathCam) | data.rs:29, chr_ins.rs:451, camera.rs:98/121 | V fields |
| 6 | `cutscene` | a `CSFade` plate's `current_color.a` rose above `fade_alpha_max` (0.5) since On, or `CSCamera.camera_mask` changed since On | fade.rs:13/33, camera.rs:25 | I |
| 7 | `torrent` | ride module: `ride_node->ride_state != 0` (0 none, 3 getting on, 5 riding, 7 dismounting; switch `use_ride_state_signal`), or `is_mounting`. `is_mounted` is logged as a probe only (its meaning after a dismount is unclear) | ride.rs:17/42/90 | V field; I values |
| 8 | `ladder` | `CSChrLadderModule.state` as i8 != -1 | ladder.rs:15/35 | V |
| 9 | `throw` | `throw_node->throw_state` as u32 in 3..=6 | throw.rs:14/43 | V |
| 10 | `talk` | `CSMenuManImp.popup_menu->current_talk_id` changed since On (this covers sites of grace, which are talk scripts, and NPCs) | menu_man.rs:51/129 | I |
| 11 | `menu` | the existing menu signal (`CSMouseMan.show_cursor`) **[V run 2: cursor flips on inventory]** | task.rs | V |
| 12 | `cam_gate` | `cam_valid == false` (`detail=why=<dist|identity|settling…>`) | cam_gate.rs | V |
| 13 | `f5` | MW2 mode off | shared MODE_ON | V |
| 14 | `f6` | the user turned first person off | new key | I |
| 15 | `config` | `[first_person] enabled = false` by hot reload | config | I |

- The edge-based signals (load counter, camera_mask, talk id, fade plates) compare against a baseline taken when
  the machine enters On (`fp_logic::EdgeWatch`, `FadeWatch`). This makes them robust to unknown idle values, such
  as a stuck plate at alpha 1 or a talk id that keeps its last value.
- Signals from optional singletons (`CSFade`, `CSNowLoadingHelper`, `popup_menu`) that are absent count as
  "unavailable": they are logged once and are not a read failure. Each one has a config switch
  (`use_fade_signal`, `use_cam_mask_signal`, `use_talk_signal`), all on by default, so a misbehaving inferred signal
  can be turned off without a rebuild.
- Anti-flap (`fp_logic::FlapGuard`): an inferred edge signal that hands back 5 times within 60 s is muted for the
  session (`FP WARN signal <name> muted ...`). The signals are `load_counter`, `camera_mask`, `talk_id` and `fade`.
  A signal that changes during normal play therefore cannot make first person flicker. The verified signals (menu
  cursor, in-world, cam gate, ride, ladder, death) are never muted.
- Latches (`fp_logic::EdgeLatch`, review fix, section 12): an edge handback stays active until the value is back at
  its On baseline for `latch_holdoff_s` (2 s), or has been stable with the cursor closed for `latch_stale_s` (20 s;
  load counter: max(hold-off, 2 s)). Without it an edge fired only once and first person re-armed 0.5 s later in the
  middle of the cutscene or talk.
- Lock-on is **not** a handback. It only pauses yaw writes.
- Probe line `FP probe ride=… mounting=… mounted=… ladder=… camtype=… mask=0x… talk=… pause_bit=… fade_max=…
  lockon=… throw=…` is logged on any raw change. It is rate-limited to once per 0.5 s with a final flush, so one
  user run maps every inferred signal.

### 5.2 Exact restoration: every field we change

| Field | Address | Saved | Restore | Model / test |
|---|---|---|---|---|
| `pers_cam_1` row 3 | `*(CSCamera+0x08)+0x40`, 16 B | ER's pre-write value, every tick | none if `er_rewrite=1` was seen. Otherwise the saved value, once. Hook mode: always the detour's saved ER position, once | `PersistWatch` round trip |
| `pers_cam_1` rows 0..2 (pitch clamp only) | `+0x10..+0x40` | ER's rows, every tick | as row 3 | `PersistWatch` |
| `pers_cam_1.fov` (`world_fov_deg > 0` only) | `+0x50`, 4 B | ER's value, every tick | as row 3 | `PersistWatch` |
| `ChrIns.chr_flags1c5` bit 3 | `main_player+0x1c5`, 1 B | the byte at enter | bit 3 from the save, the other bits as they are now; only if the pointer is unchanged | `RenderBit`, all 256 values |
| `ChrIns.base_transparency_modifier` (fade method) | `main_player+0x250`, 4 B | the value at enter | +1 until alpha >= 0.999 (max 2 s), then the saved value; only if the pointer is unchanged | `FadeRestore` |
| `CSChrPhysicsModule.orientation` / `.interpolated_orientation` | `physics+0x50` / `+0x60`, 16 B each | logged at the first write | release (never written back) | `YawRelease` |

Every new write is logged once per episode with the field name, address, old and new value. Every handback logs its
reason. No write ever happens before the version gate passes: `game::write` checks `WRITES_ALLOWED`, which only
`boot.rs` sets after `version_gate::check()` returned Ok.

---

## 6. Config (`mw2er.toml`)

```toml
[keys]
fp_toggle = "F6"           # first person on/off while MW2 mode is on (hidden from the game if blank_toggle_key)

[first_person]
enabled          = true            # master switch (hot)
camera_mode      = "task"          # task (Tier A) | hook (Tier B); not hot (the hook installs at boot)
eye_height_m     = 1.60            # 1.2..2.0, above the physics position (feet)
eye_forward_m    = 0.10            # 0..0.5 along the horizontal view direction
eye_smooth_s     = 0.0             # 0 = off; exponential filter on eye.y
pitch_clamp_deg  = 85.0            # 30..89.9
hide_method      = "render_flag"   # render_flag | fade | none
yaw_follow       = true
yaw_probe_samples = 60             # 10..600
world_fov_deg    = 0.0             # 0 = keep ER's FOV (48 deg); else 30..120
arm_ticks        = 30              # clear ticks before first person turns on (1..600)
fade_alpha_max   = 0.5             # cutscene trigger: a fade plate above this
use_fade_signal  = true
use_cam_mask_signal = true
use_talk_signal  = true
use_ride_state_signal = true       # false = Torrent via is_mounting only
latch_holdoff_s  = 2.0             # 0..30
latch_stale_s    = 20.0            # 0 (never) or 2..600
log_every_ticks  = 300
```

Every float is finite and bounded, and unknown keys are errors (M2 rules). `camera_mode` is cold in `merge_hot`.

---

## 7. Log lines that prove success in game

| Line | Proves |
|---|---|
| `FPCAM cfg enabled=1 mode=task eye_h=1.60 eye_fwd=0.10 pitch_clamp=85 hide=render_flag yaw=1 world_fov=off` | config loaded |
| `FPHOOK installed rva=0x3bc5f0 prologue=ok` / `FPHOOK FAIL …` | hook mode only |
| `FP on eye=(…) cam_gate=ok er_cam_dist=3.4m hp=… ride=0 ladder=-1 camtype=… mask=0x… talk=… fade_max=… world_fov=48.0deg near=0.0501` | the machine armed |
| `FPCAM write field=CSCamera.pers_cam_1+0x40 addr=0x… old=(x,y,z) new=(x,y,z)` | the first write of the episode |
| `FPCAM er_rewrite=1` (once per session) | the free restore holds |
| `FPCAM WARN er_rewrite=0 …` | failure: use `camera_mode = "hook"` |
| `FPCAM tick=N eye_to_er_cam=3.4m eye_to_chest=0.41m pitch=-12.0deg clamp=0 near=0.0501 world_fov=48.0deg writes=N mode=task` | every 300 ticks while On |
| `FP hide write player=0x… field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) old=0x.. new=0x..` | body hidden |
| `FP hide reasserted count=1 (first)` | the engine re-set the bit (watch the count) |
| `FP hide restore player=0x… old=0x.. new=0x.. reasserted=N reason=…` | exact restore |
| `FP yaw probe conv=+z dot_med=0.97 n=60` / `FP yaw probe FAIL …` | forward axis |
| `FP yaw write field=CSChrPhysicsModule.orientation addr=0x… old=(…) new=(…)` (+ `interpolated_orientation`) | yaw writes |
| `FP yaw drift p50=… p95=… writes=N paused=M (…)` | how much ER fights the yaw |
| `FP off reason=<panic|read_fail|cam_persist|loading|death|cutscene|torrent|ladder|throw|talk|menu|cam_gate|f5|f6|config> <detail>` | every handback |
| `FP restore field=… old=… new=…` | forced camera restore (only when needed) |
| `FP blocked reason=…` | why it is not arming |
| `FP probe …` | maps the inferred signals |
| `FP toggle F6 wanted=0|1` | hotkey |
| `FP latch set signal=<talk_id|camera_mask|fade|load_counter> ...` / `FP latch released signal=... why=returned|stale after N s` | edge handbacks hold for the whole cutscene/talk |
| `FPCAM persisted sample streak=1/3 ...` | ER skipped one camera step (tolerated; 3 in a row = `er_rewrite=0`) |
| `FP off reason=read_fail write_fail field=... why=page_not_plain_readwrite|...` | a write refused by the page check |

The analyzer's new `first person` row:
- PASS needs, in sessions whose DLL logs `FPCAM cfg`:
  - at least one `FP on`;
  - every `FP on` followed by an `FP off` before the next `TASK world left`;
  - `FPCAM er_rewrite=1` (task mode) or `FPHOOK installed` (hook mode);
  - an `FP hide write` when the hide method is not none;
  - no `FPCAM WARN`;
  - no `CAMFRAME valid=0 why=dist` while On.
- Older logs give `N/A` (not counted).
- The user's visual check is the final oracle: the body is gone from behind the viewmodel, the view is at eye
  height, mouse look works, and other characters stay visible.

---

## 8. Analyzer fix: shield blocks [V log; I rule]

Run 3 (`logs\mw2er.log:937-988`) logged seven c4311 hits of `dmg=4` at 14-18 m, between normal hits on the same
enemy. Godrick Soldiers carry shields, so these are most likely blocks.

Rule: for a reference enemy, a non-kill hit with `dmg < 0.2 × band baseline` is `blocked?`. The band baseline is
the measured baseline for a measured band, else the ER-formula scaled baseline.

- `blocked?` hits are excluded from calibration and counted in the detail line.
- Grouping is by enemy TYPE (cid): HIT lines carry no per-instance id, so "next to normal hits" means normal hits
  on the same cid in the session, not on the same individual.
- Guard: if every non-kill hit on that enemy type is `blocked?`, they are judged by the normal plausibility rules (so a
  lone `dmg=1` still FAILs, as the reviewer's case requires). A real block series always sits next to normal hits.
- No DLL-side guard flag is read in v1. The guard state was not found in the vendored crate, so the classification
  is by damage only, and is labelled `blocked?` (inferred).

Run 3 also gives the first close-range measurement on c4311: band 79000100 (atkPhys 160), d 0.9-8.8 m, non-kill
hits 64, 49 ×9 **[V log 1008-1020, 1134-1139, 1314-1326]**. KNOWN_TARGETS gains it as a second measured band
(baseline 49, PASS band 35..75), so the run-3 session gives PASS on hits. The ER-formula ratio predicted ×1.25
(61); the measurement says ×1.0. That is recorded as a finding: the formula is info only.

The self-test runs the real run-3 log when it exists. Session 3 must give `hits` PASS with 7 `blocked?`, and the
`first person` row must be N/A. This PASS is a **calibration** check, not a validation: band 79000100's baseline
was measured from run 3 itself. Only the second half (the same log FAILs without the blocked rule) is independent.
The next run that hits c4311 at close range is the validation.

---

## 9. Offline oracles

Pure logic tests in `mw2er-core` (categories required by check_m3b):
- `fp_logic`:
  - one test per trigger in the table, each returning that reason;
  - simultaneous triggers follow the priority order;
  - arming needs `arm_ticks` clear ticks;
  - a blocked reason resets arming;
  - no flapping: a trigger in Arming never yields On;
  - F6 / F5 / config waits;
  - transient triggers re-arm;
  - EdgeWatch and FadeWatch baselines, including a stuck plate.
- `fp_math`:
  - eye known answers;
  - straight up/down (no forward offset);
  - smoothing;
  - pitch clamp: no-op below the limit; at 89° rebuilt to 85° with orthonormal rows and det preserved;
  - yaw quaternion round trip for ±z;
  - the probe resolves +z / -z, FAILs on noise and gives up after 3 attempts;
  - the yaw gate denies on each pause cause;
  - drift percentiles.
- `fp_restore`:
  - PersistWatch rewrite/persist/restore round trips;
  - RenderBit restore == saved for all 256 bytes with arbitrary engine changes to the other bits;
  - a pointer change means no write;
  - FadeRestore stages and timeout;
  - YawRelease writes nothing.
- `config`: `[first_person]` defaults, bounds (inf/nan rejected) and `camera_mode` cold in `merge_hot`.

`tools\scripts\check_m3b.ps1` (PowerShell 5.1, never launches anything, writes only to `work\check_m3b\` plus what
check_m3 writes):

| # | Check | Pass criterion |
|---|---|---|
| 1 | M3 suite | `check_m3.ps1` exit 0, 18 PASS (which includes check_m2 21 PASS) |
| 2 | cargo build | release workspace build, 0 warnings under `mod\` |
| 3 | cargo test | `mw2er-core` tests pass, with ≥ 1 test in each category: `fp_logic` triggers, priority, arming, edge/fade watch; `fp_math` eye, pitch clamp, yaw quat, yaw probe, yaw gate; `fp_restore` persist, render bit (256), fade, yaw release; `config` first_person |
| 4 | config | `mw2er.toml` parses with tomllib and with the DLL parser; `[first_person]` present with the documented defaults; `keys.fp_toggle = "F6"` |
| 5 | wiring | `m2_static_checks.py fpwiring --self-test`: WriteProcessMemory appears only in `game\mod.rs`; `write()` checks `WRITES_ALLOWED`; only `boot.rs` sets it, after `version_gate::check`; no raw pointer writes in the first-person code; the hook RVA and prologue constants are as documented and the hook installs only for `camera_mode = "hook"`; the erfps2 attribution is present; the offset asserts exist (0x1c5, 0x24c, 0x250, PlayerIns.chr_ins 0); the self-test mutations must fail |
| 6 | DLL strings | the built DLL contains the proof log strings (`field=CSCamera.pers_cam_1+0x40`, `er_rewrite=`, `enable_render(+0x1c5 b3)`, `yaw probe conv=`, `off reason=`, `FPHOOK`) |
| 7 | analyzer | `analyze_log.py --self-test` exit 0, including the blocked-hit cases, the FP row cases and the real run-3 log (hits PASS with 7 `blocked?`) |
| 8 | regulation unchanged | `build\rows.json` still has 13 ids, and the package hash equals the one check_m2 verified (M3b adds no rows) |

---

## 10. What stays unverified until the user's run

- Whether a task-phase write of `pers_cam_1` row 3 reaches this frame's render (Tier A), or whether `hook` mode is
  needed (FPCAM er_rewrite, the user's eyes).
- Whether `enable_render` hides the body without side effects, and whether the engine reasserts it.
- The character forward axis and how much ER fights the yaw writes (probe + drift lines).
- Every inferred handback signal (FP probe lines): fade plates, camera_mask, talk id, ride_state values,
  `is_mounted`.
- Sound: the Wwise listener stays at ER's third-person camera, about 3.5 m behind (erfps2 hooks
  SET_WWISE_LISTENER). This is accepted for M3b.
- ER's camera auto-rotation option: if it is on, ER may swing the camera behind the character while moving.
  Recommend turning it off in ER's options for first person.
- Whether clearing `enable_render` also skips the player's bone/animation update (which would freeze the hurtboxes):
  watch whether enemy attacks still land on you while you move in first person.
- Whether spells and arrows still leave from the right place (the body is hidden, not moved).
- Whether the yaw writes fight a gamepad stick: the raw-key gate only knows W/A/S/D; the new `off_heading` pause
  (movement > 45 deg from the camera heading at > 1 m/s) is the fallback (yaw drift line `off_heading=N`).
- Whether ER's heap pages are plain PAGE_READWRITE (the new page check refuses anything else; a refusal shows as
  `FP off reason=read_fail write_fail ... why=...`).

## 11. Decisions the implementer must not change

1. Never launch the game, me3 or Steam. Never write into a game folder or `%APPDATA%\EldenRing`. Never publish.
2. Default `camera_mode = "task"`: never write `chr_cam`, `ChrExFollowCam`, LockCamParam or camera globals.
3. Read `pers_cam_1` for the gate before any write in the same tick. Never feed the eye to the gate.
4. Only bit 3 of `chr_flags1c5` is ever written, only on `main_player`, only to the address whose value was saved.
5. Every runtime write goes through `game::write` (version-gated, non-faulting) and is logged once per episode.
   Every handback logs its reason.
6. Pure first-person logic lives in `mw2er-core` (`forbid(unsafe_code)`) with tests.
7. erfps2 is reference only: RVAs and ideas are attributed in the source and in MODLOG. No code is copied.

---

## 12. Review fixes (2026-10-14, before the user's first M3b run)

| # | Finding | Fix | Oracle |
|---|---|---|---|
| M1 | Edge signals (fade, camera_mask, talk_id, load counter) fired once; first person re-armed 0.5 s later inside the cutscene/talk | `fp_logic::EdgeLatch`: the handback stays active until the value is back at the On baseline for `latch_holdoff_s`, or stable with the cursor closed for `latch_stale_s` | `fp_logic_edge_latch_keeps_handback_while_value_changed` (60 s simulated talk never reaches On; the unlatched variant does), `fp_logic_edge_latch_stale_release_needs_closed_cursor`; mutation (latch no-op) fails both |
| M2 | The hide restore waited for the whole `FpPlayer` read | `fp_read::read_hide_fields()` (main_player + the 3 hide fields, owner-checked) for every hide write, restore, deferred retry and fade step; a pending restore also blocks arming | fpwiring + build; runtime: `FP hide restore ...` |
| m3 | Panic path skipped a pending restore / fade-in | `emergency_off` always resolves `pending_hidden` and finishes the fade (saved value if visible, else leaves +1 and logs) | code review |
| m4 | Read-modify-write race on `+0x1c5` | the byte is re-read immediately before every write; only bit 3 changes from that value | `fp_restore_render_bit_all_256` (model) |
| m5 | `WriteProcessMemory` can write read-only/code pages | `game::write` first checks `VirtualQuery`: committed, not `MEM_IMAGE`, exactly `PAGE_READWRITE`, range inside the region; refusal code in the read_fail detail | fpwiring (3 new mutations) |
| m6 | One stale camera sample disabled first person for the session | `PersistWatch` streak: 3 persisted samples in a row; on a single one the gate gets ER's last position (`FpController::er_view_override`) | `fp_restore_persist_watch_write_persists` |
| m7 | Fade re-enter saved our own +1.0 | `FadeHide::enter_after` carries the running restore's saved value | `fp_restore_fade_stages` |
| m8 | Torrent idle value guessed | config `use_ride_state_signal` (default true) | `fp_logic_signal_rules` |
| m9 | Yaw gate only knows W/A/S/D | `YawPause::OffHeading`: planar velocity > 1 m/s and > 45 deg from the camera heading; documented in section 10 | `fp_math_yaw_gate_pauses` |
| m10 | Detour trusted a published camera address | the detour re-reads `CSCamera.pers_cam_1` every frame and requires it to equal the published CSCam | fpwiring mutation |
| m11 | Analyzer run-3 test partly circular; grouping wording | labelled calibration, not validation; wording says grouped by cid | analyzer self-test text |
| m12 | In-game checks | added to section 10 and the MODLOG test steps | user's run |

## 13. After run 5 (2026-10-14): write cost fix and erfps2's camera handling

- The run-5 FPS collapse (~16.8 ticks/s with first person on) and the ~70 ms yaw ticks were the cost of
  `game::write` itself: `VirtualQuery` + `WriteProcessMemory` both query the region, which takes time proportional to
  the region size (ER's heap: several GB, ~35 ms per write). `game::write` now caches the per-page verdict
  (`mw2er_core::write_cache`) and copies with `NtWriteVirtualMemory` (no region query, never changes protection).
  The yaw write is one 32-byte write. Details and numbers: MODLOG 2026-10-14 entry.
- Decision 11.2 ("never write chr_cam, ChrExFollowCam or camera globals") is superseded for hook mode by the user's
  request to mirror erfps2. These writes are now made, each with its own `[first_person]` switch (default true) and an
  exact restore on every handback:
  - `ChrCam.pers_cam` row 3 = eye, in the camera-step detour (`sync_chr_cam`);
  - `ChrExFollowCam.camera_auto_rotation` (+0x240) = 0, in a new follow-cam update detour (RVA 0x3b7570,
    `disable_auto_rotation`);
  - the exe's `CAM_WALL_RECOVERY` bool (RVA 0x3b19c80) = 0, in the same detour, through a dedicated validated
    exe-.data write path (`disable_wall_recovery`).

  Task mode still never writes them.
