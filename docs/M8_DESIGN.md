# M8 design: MW2 mantling onto rocks and ledges, plus a ladder handback audit

Status: design, 2026-10-18. Nothing here has run in game. The baseline is run 17, where the user said the M5 controller
"works great".

User request (run 17): "I was having issues traversing across rocks and other terrain." MW2's 39 in (0.991 m) jump
and 18 in (0.457 m) step cannot clear Elden Ring rocks and ledges.

The user chose:
- **MW2 mantling.** Jump at a ledge or rock you cannot clear and pull yourself onto it with a short smooth climb, up
  to about 2 m (configurable), with the gun lowered as in MW2.
- **Not now:** a smoother rough-ground step-up, or a grappling hook.
- **Ladders:** keep ER's third person while climbing, with no behaviour change. Audit the ladder handback offline and
  add log lines and analyzer rows, so that the user's first ladder test settles the question.

Sources:
- `mw2-mantle` finding: a static byte read of the user's `iw4sp.exe` (sha256 2f8c9cfd…, equal to `PINNED_EXE_SHA`),
  46 byte-checked sites in `work\m8\mantle_sites.py`, and the disassembly of 0x140070940-0x140071D90 in
  `work\m5\text_all.txt`. It also used new xanim dumps, both 0 warnings and 0 errors:
  - `dumps\m8\code_post_gfx\xanim\player_mantle_*`: the SP set that the exe binds by name at 0x1403BCC30;
  - `dumps\m8\common\xanim\mp_mantle_*`: the MP set, recorded for information only.
- `ledge-probing` finding: a source read of `move_collide.rs`, `move_sim.rs`, `movement.rs`, `havok.rs`,
  `move_logic.rs` and `fp_logic.rs`, plus the run-17 PERF and resync lines in `logs\mw2er.log`.
- `ladder-audit` finding: a code read and the 27,908-line `logs\mw2er.log` covering 18 sessions.
- `docs\M5_DESIGN.md`, `M6_DESIGN.md`, `M7_DESIGN.md` and `MODLOG.md`.

## Evidence labels

The labels are the same as in M5:
- **[V] verified**: a byte or disassembly read, a dump parse, a compile, a log line or an offline run on this PC.
- **[D] derived**: a name or meaning taken from IW4L (`vendor\iw4L`, an MP reimplementation) or the vendor enum,
  consistent with V code but not checked byte for byte.
- **[I] inference**: our design choice or a guess. Each one has a log line or an oracle that settles it.

Rules (unchanged from M5 to M7):
- Never launch Elden Ring, MW2, me3 or Steam. `iw4sp.exe` and the zones are only read as files.
- Never write into a game folder or `%APPDATA%\EldenRing`. Never push or publish. Never browse the user's personal
  folders. Do not touch `D:\Mods\MW2inER-m3`. Downloads only through cargo from crates.io.
- Every runtime write goes through `game::write`.
- MW2 numbers reach the code only through `converted\movement.json`, written by `convert_movement.py`.

### Where the two research findings disagree, and what this design takes

| Topic | `ledge-probing` | `mw2-mantle` (exe-bound) | **M8 takes** |
|---|---|---|---|
| Animation timing | MP set: 333-833 ms by height, x and z together | SP set: **500 ms up for every height**. It rises first (0-333 ms) and moves forward after (333-500 ms). Over takes 667 ms | **SP set [V]**. The MP set is stored as `info` only |
| Dvar defaults | D-IW4L until byte-checked | byte-checked at the registration function 0x140071A00 | **V** |
| Forward input needed | yes (gate) | **no**: SP needs only jump held, a ledge ahead and the view within 60° of the wall. Auto-mantle on forward speed is the MP perk | **MW2 SP rule** by default. `require_forward` is an option, off by default |
| Start while crouched | allowed | **refused**: eFlags & 0xC, so only standing [V] | **standing only** (MW2). A crouch-only end is still allowed (see 4.6) |
| Fall-speed gate | 200 in/s | none: the check runs every tick and the 18..60 in window does the work | **none** (MW2) |

---

## 0. Decisions at a glance

| Topic | Decision | Label |
|---|---|---|
| Prerequisite P1 | **Fix the chunk re-base classification first (B5).** All 42 ER re-bases in the log took the Warp branch, which zeroes velocity and ends sprint, and would abort every mantle near a 32 m border | V bug; I fix |
| Prerequisite P2 | Analyzer fixes B6: the `nuke` FP reason, the re-base CAMFRAME grace, and the jump row filtered on `land_factor=1.00`. Without them the ladder and mantle results are hidden behind unrelated FAILs | V |
| Where | A new pure module `mw2er-core\src\mantle.rs` (detection, path, state). `move_sim::Player` gains `mantle: Option<Mantle>`. The check runs in `substep()` after the stance keys and **before** the jump block, as in `PmoveSingle` | V MW2 order; I placement |
| Trigger | MW2 SP: jump held (or pressed) while standing, on the ground or in the air; an obstacle face within reach (radius + 20 in); `fwd_h` within 60° of the face's inward normal. No forward-input condition by default | V |
| Heights | The ledge top must rise **18..60 in (0.457..1.524 m) above the current feet** (MW2), and be at most `max_height_m` = **2.0 m above the ground the player left**. Holding jump through the 39 in jump reaches 2.0 m (MW2 reaches about 2.5 m, I) | V window; I 2.0 m cap |
| Detection | Procedural, because ER has no mantle surface flags and `cast_ray` returns only a point. A fixed 16-ray pattern, spent only on gated ticks, with its own `RayBudget(16)`, at most one detection per tick, and a 150 ms negative cache | I |
| Motion | The scripted SP curve: a vertical rise on the exe's sigmoid (333 ms), then a forward move onto the top (167 ms, longer if the distance is longer). No collision-free root motion: every tick is re-validated with ≤ 4 rays. The entry blends the current upward velocity, so there is no snap. MW2's up to 3 in start snap is blended, not teleported | V curve; I blend |
| Over | Thin walls (top too shallow to stand on) with a floor on the far side: rise, cross 47 in (16 + 31) at the ledge height, then release into normal gravity for the drop (MW2's 18 in drop). Without a floor within 2.0 m: refuse | V numbers; I procedural decision |
| End | Grounded on the top, `vel.y = 0` (MW2 `mantle_max_vertical_end_velocity` 0), planar exit velocity = the last path step, clamped to walk speed. No Land event: no dip, no jump slowdown. A mantle started from a grounded press replaces the jump, so no slowdown starts | V; I clamp |
| Weapon | Rise ≥ 33 in table row: MW2 quick-drops to no weapon (UMP45 0.25 s), with no fire and no ADS, then quick-raises (0.55 s). 27/21 in rows keep the gun up. Sprint ends, ADS is cancelled, firing is blocked | V |
| Camera | ER owns the camera. The FP eye follows the feet path, which is C1 at entry and eases to zero speed. MW2's ±60° yaw cap is **not** applied (we do not write the camera); the path is fixed in world space at start | I (deviation recorded) |
| Safety | Never write an unvalidated position. Abort keeps `last_valid`. Every handback (death, menu, AC-130, nuke, loading, ladder, interact…) aborts the mantle inside `release()`. `air_top_y` follows the path, so an abort falls ≤ 2 m (MW2 fall_min is 5.08 m) | I |
| Ladder | Keep ER third person. Fix B1 (AC-130/nuke on a ladder), B2 (MW2 gun fires on a ladder; MW2 quick-drops on ladders too, V), B3 (the flap guard mutes E), B4 (slow return), plus offset asserts and the read-fail fallback. New `LADDER` lines and four analyzer rows | V bugs; I fixes |
| Config | `[movement.mantle]`, all hot except `max_rays`. Ranges validated | I |
| Oracles | Core tests on synthetic worlds, movebench b13-b20 at 30/60/144 Hz, analyzer fixtures, and `check_m8.ps1`, which runs `check_m7.ps1` first and prints `M8 OFFLINE ORACLES: PASS` | — |

Not in M8: rough-ground step smoothing, a grapple, the MW2 "Press [jump] to mantle" hint. The hint would need a
probe ray every tick without jump; it is possible later as `hint = true`.

---

## 1. MW2 mantle reference (to `converted\movement.json` → `"mantle"`)

Units are IW4 inches. The controller runs in inches (M5 1.4). **Rises and distances are not scaled** (jump 39 in =
0.991 m, V in game). Only the hull heights are scaled (stand 73.5 in = 1.867 m, crouch 52.5 in = 1.333 m; M5 section 3).

### 1.1 Dvars (registration 0x140071A00; flags 0x84 [D])

| Key | Default | Range | Source sites | Label |
|---|---|---|---|---|
| `mantle_enable` | 1 | bool | mov dl,1 @0x140071A11 | V |
| `mantle_debug` | 0 | bool | xor edx,edx @0x140071A2D | V |
| `mantle_check_range` | 20 in | 0..128 | @0x140071A50 → 0x140385AE4; max @0x140071A41 | V. This is the probe reach beyond the player radius, although the description says "minimum distance" |
| `mantle_check_radius` | 0.1 in | 0..15 | @0x140071A8A → 0x140385A0C | V |
| `mantle_check_angle` | 60° | 0..180 | @0x140071AC4 → 0x140385B08 | V |
| `mantle_view_yawcap` | 60° | 0..180 | @0x140071AFE | V (not applied in ER, see 6) |
| `mantle_max_vertical_end_velocity` | 0 | 0..FLT_MAX | xorps @0x140071B49 | V |

### 1.2 Geometry (Mantle_FindLedge 0x140070C10, CheckLedgeHeight 0x140070FE0, CalcPath 0x140070940)

| Number | Value | Meaning | Label |
|---|---|---|---|
| player radius in probes | 15 in | box half xy | V @0x140070C85 |
| front probe | box z 0..70, from origin − fwd·14.9 to origin + fwd·34.9 | the face must be within reach | V |
| facing | acos(fwd · −n_xy) ≤ 60° | view yaw vs face | V |
| test heights | 60, then 40, then 20 in | per-height ledge test | V @0x140070E8F/EB3/ED7 |
| forward step | 16 in | ledge point = start + dir·16 at height h | V @0x140071028 |
| ledge floor | drop to start.z + 18 in | **min rise 18 in** (= step_height) | V @0x140071107 |
| walkable top | trace walkable byte | normal.z ≥ 0.7 threshold is D | V byte / D threshold |
| clearance | box xy ±15, z 0..50 at the ledge | **crouch hull** headroom only | V @0x140071188 |
| over | forward 31 in at ledge height (box z 0..50), then drop 18 in. If the drop hits anything, OVER is cleared (mantle up) | V @0x1400709B8, 0x140070AAC |
| surface flags | mantleOn surf 0x2000000 / mantleOver 0x4000000, contents 0x1000000 | ER has none, so detection is procedural | V (`infoparms.py`) |
| start gates | `mantle_enable`, pm_type < 6, not PMF_MANTLE (pm_flags&4), not crouched or prone (eFlags&0xC) | V bits; D names |
| trigger | cmd.buttons & 0x400 (jump/gostand held) or script ForceMantle (0x20); checked **every non-mantling Pmove**, ground or air | V @0x140070970, call @0x14007D716 |
| max rise | 60 in above the **current** feet; about 99 in (2.5 m) above the ground when caught at the jump apex | V / I |

### 1.3 Transition table (0x1403BCBD0) and weapon [V]

| Row | Height | Up anim | Over anim | Weapon |
|---|---|---|---|---|
| 0 | 57 | up_57 | over_high | quick drop to none |
| 1 | 51 | up_51 | over_high | quick drop |
| 2 | 45 | up_45 | over_mid | quick drop |
| 3 | 39 | up_39 | over_mid | quick drop |
| 4 | 33 | up_33 | over_mid | quick drop |
| 5 | 27 | up_27 | over_low | **stays up** |
| 6 | 21 | up_21 | over_low | **stays up** |

The row is argmin |H − rise|. MW2 then snaps the origin so the anim ends exactly on the ledge (≤ ~3 in). M8 does
not snap; it blends (section 5). BG_IsWeaponInactive 0x1400716B0 = mantling and over anim ≠ 10 (V). The quick drop
uses the gun's `quickDropTime`/`quickRaiseTime` (UMP45 0.25 / 0.55 s, V). There is no mantle viewmodel animation.
On a ladder (pm_flags & 8) MW2 quick-drops the weapon as well (V @0x140082088; flag name D).
Foley: `gear_rattle_plr_mantle` (foly_gear_rattle1-5.wav, vol 0.26-0.28), V in `dumps\ac130\…\sound\foley`.

### 1.4 SP animation timing and curves (30 fps, delta translation only) [V, `mantle_curves.py`]

| Anim | Frames | Duration | Curve (normalized per frame) |
|---|---|---|---|
| `player_mantle_up_*` (all 7) | 15 | **500 ms** | frames 0-10 (0-333 ms) z only: 0, .045, .131, .247, .382, .526, .668, .797, .903, .974, 1.0 (up_57; the per-height curves are stored as dumped, up_21 is slightly more linear). Frames 10-15 (333-500 ms) x only, 16 in: 0, .073, .258, .507, .771, 1.0 |
| `player_mantle_over_*` (3) | 20 | **667 ms** | frames 0-10 x 31 in: 0, .110, .233, .365, .498, .626, .745, .848, .928, .981, 1.0; frames 10-20 z −18 in, easing in |

A mantle up takes 500 ms; up + over takes 1167 ms. The end velocity is the last root-motion step / dt, with
`vel.z = min(vel.z, 0)` (V, Mantle_Move 0x1400716E0).

MP set, `info` only (not used): up_21..57 = 333/433/500/600/600/800/833 ms, over 233/400/467 ms, Freerunner _fr
233-367 / 133-200 ms.

### 1.5 Converter work (`convert_movement.py`)

- Merge the 46 sites of `work\m8\mantle_sites.py` (`S` dict) into `SITES`/`KNOWN`. Each must byte-check, or the
  converter fails.
- Read the 10 SP xanims through `xanim_read.py`. Store each file's sha256, frame count, fps and normalized curves. The
  self-test checks the end deltas (16, 0, H) and (31, 0, −18) within 1 in, the same rule as the exe's load check at
  0x140071200.
- Emit `"mantle": { dvars, geometry, table, weapon_rows, curves: {up: {57: […], …}, over: {high,mid,low}},
  durations_ms, info_mp: {…}, src: {label per key} }`. Every key carries V/D/I.
- Mark `max_height_above_ground_m = 2.0` as **I** (the user's choice), `airborne_reach_in ≈ 99` as **I**, and the
  flag and event meanings (events 6/7, flags 2/4) as **D**. Events 6/7 are not ported.
- `movement.json`'s existing keys must stay byte-identical (self-test compares the old block).

---

## 2. Prerequisites (do these first, each with its own tests)

### P1 = B5: chunk re-bases are classified as warps [V bug]

- **Evidence.** In `logs\mw2er.log`, all 42 resyncs over 5 m are `why=warp`, with `chunk_d` ≈ `d` (32.00, 32.98,
  35.78, 40.00 m; 32 m plus multiples of 8 m on an 8 m grid). There are 0 `why=rebase`.
- **Cause.** `classify_jump` (move_logic.rs:411-419) assumes that `ChrIns.chunk_position` stays continuous across a
  re-base. In game it jumps by the same amount.
- **Effect.** Velocity goes to zero and sprint ends every ~32 m. A mantle would also be lost.
- **Fix (pure, `move_logic::classify_jump`).** It takes vectors: e = `r.pos − last_written` (3D) and c =
  `chunk_now − chunk_prev`. It returns **Rebase** iff:
  - |e − c| < 1.0 m + expected motion;
  - |c| ≤ 64 m;
  - no loading or world-change signal in the last 1 s;
  - the map/block id is the same or adjacent.

  Otherwise it returns **Warp**. The sign convention (e ≈ c or e ≈ −c) is not settled offline [I]. The function
  accepts either sign and logs which one matched (`sign=+|-`). The next run settles it.
- **Logging.** The `MOVE resync` line gains `e=(..) c=(..) block=..`.
- **Rebase action.** `pl.translate(e)` translates the position, `last_valid`, the mantle path (start, ledge, end,
  wall plane) and the negative cache. Velocity and sprint are kept. Mid-mantle it logs
  `MOVE mantle rebase d=…`, and the next tick's re-validation rays run as usual. If the classification was wrong, they
  abort the mantle.
- **Tests.**
  - The 42 logged (d, chunk_d) pairs, as synthetic vectors on the logged grid, give Rebase.
  - A synthetic fast travel gives Warp: 300 m with a loading flag, and 40 m with e ≠ c.
  - A re-base mid-mantle gives the same end relative to geometry.

### P2 = B6: analyzer rows that would hide the result [V]

- (a) Add `nuke` to `FP_REASONS` (analyze_log.py:394).
- (b) Ignore `CAMFRAME valid=0 why=dist` when a `MOVE resync why=warp|rebase` has the same timestamp and no
  `FP off reason=cam_gate` follows within `cam_gate_grace_ticks`.
- (c) The jump row judges only apexes whose `MOVE jump` has `land_factor=1.00`, or compares with v0²/2g.

  Run 17's 0.43-0.62 m apexes are MW2's landing penalty, not a bug (land_factor 1.76-2.50). That penalty probably
  added to the user's rock trouble. M8 does not change `jump_slowdown`, because the default is MW2. The mantle fixes
  the traversal, and a mantle never starts the slowdown.

Pass: on sessions 16-18 of the existing log, the first-person row and the jump row are no longer FAIL.

---

## 3. Trigger (zero-ray gate, then detection)

`Player::mantle_check` runs in `substep()` after the stance keys and before the jump block (V order: single.rs
175-193, call @0x14007D716). The zero-ray gate requires all of the following:

1. `cfg.mantle.enabled` and `movement.json` `mantle_enable` (1).
2. Not mantling. Not in a handback or arming state. Not within 0.3 s of a re-arm. Not while a `ground_disagree` hold or
   an ER push-out `ErMove` handback is active.
3. **Standing**: `stance.effective() == Stand` and no transition in progress (MW2 eFlags & 0xC, V).
4. **Jump input**, by `key_mode`:
   - `mw2` (default): jump held this tick. This needs a new `Input.jump_held` level from the RAW_KEYS `PRESS_JUMP`
     bit, like `sprint_held` (movement.rs:1003). A press edge counts as held.
   - `press`: only a press edge, on the ground or in the air.
   - `auto`: jump held, **or** forward input with forward speed into the face (the MW2 MP automantle perk, off by
     default).
5. If `require_forward` (default false), `cmd.forwardmove > 0`.
6. Not occupied: no `char_pushout` character within r + 0.45 m of the candidate top (checked after R2, still 0 extra
   rays).
7. Negative cache: after a refusal, skip checks for 150 ms unless the feet moved > 0.10 m (3D) or the yaw changed
   > 10°. While the player rises through a jump, the cache re-opens every ~4 ticks, so a ledge is caught as soon as
   the window allows.

The gate costs 0 rays. When it passes, detection (section 4) runs with `RayBudget::new(cfg.mantle.max_rays)`, at
most once per tick (not per substep).

**Grounded jump press against a ledge.** The check runs first. If it succeeds, the mantle **replaces** the jump:
there is no Jump event, no `slow_ms`, and no land factor (MW2: the Pmove takes the mantle branch). If it fails, the
normal jump happens, and the held key keeps the check alive in the air. This is how 1.6-2.0 m ledges are reached:
- the grounded check refuses `too_high` (> 60 in above the feet);
- the jump lifts the feet;
- an airborne tick passes once the top is ≤ 60 in above the current feet and ≤ 2.0 m above the take-off ground.

---

## 4. Ledge detection (`mantle::find_ledge`, pure, behind the `RayWorld` trait)

### 4.1 Definitions

All units are inches unless shown in metres.
- `p`: the feet.
- `r`: the capsule radius (ER `hit_radius` or `hull_radius_m`; about 15.7 in).
- `s`: the skin (0.03 m).
- `H_st` 73.5 and `H_cr` 52.5: the scaled hulls.
- `rise_min` = `step_height` 18.
- `rise_max` = `max_rise_in` (60, MW2).
- `cap_y` = `ground_y0 + max_height_m`, where `ground_y0` is the feet height at the last grounded tick (for a
  grounded check, `p.y`).
- `top_lim` = min(`p.y + rise_max`, `cap_y`).
- `reach` = `mantle_check_range` 20.
- `lift` = 0.05 m (2 in).
- `fwd_h`: the planar view forward.

**Axiom (from M5 5.1 and the synthetic world, I for ER):** a ray that starts inside a solid misses. So every ray in
the pattern below starts either inside the current capsule (free by the controller invariant) or on a segment that an
earlier ray proved free. A miss caused by a start inside a solid can only cause a **refusal**, never an acceptance.
The tests mutate each origin to prove this.

### 4.2 Ray pattern (in order, with an early exit at the first refusal; the maximum is 16)

| # | Ray | From → to | Result |
|---|---|---|---|
| R0 | front, knee | p + up·(rise_min + lift), along fwd_h, length r + reach + s | Miss: `no_face` (no ray spent on later steps). Hit point W, distance d. Reuse a same-tick sweep hit at knee or chest from `slide()` / `contacts[]` when it is within r + reach and the angle is OK. That costs 0 rays |
| R1, R1' | normal feelers | the same height at ±0.7r lateral (`wall_normal`, move_collide.rs:181-214) | n_xy; dir = −n_xy. Crease (two opposed planes): take the one most opposed to fwd_h. `angle(fwd_h, dir) > 60°`: `angle` |
| R1a | head up-ray (only if top_lim + lift > p.y + H_st − 1) | p.y + H_st − 1 → top_lim + lift + 1 | Hit: `ceiling` (the "over" line below would start in solid) |
| R2 | over-the-top line | p + up·(top_lim − p.y + lift), along dir, length d + X + r + s (X below) | Hit: `too_high`. The face continues above the reach. A miss proves the line free; it carries the R3 origins |
| R3 | top probe, centre | q0 = p_xz + dir·X at height top_lim + lift, down to p.y + rise_min | Miss: `no_ledge`. Hit: ledge_y. rise = ledge_y − p.y must be in (rise_min, rise_max], and ledge_y ≤ cap_y; otherwise `too_high` / `too_low` |
| R4, R4' | top probe, sides | q0 ± side·0.7r, same span | normal3(q0, q4, q4') must have n.y ≥ `walkable_normal_y` (0.7, D); otherwise `slope`. If a side misses or drops > rise_min: the top is narrow on that side and the lateral test fails, `edge` |
| R5 | depth probe | from R2's line at q0 + dir·r, down to ledge_y − rise_min | Hit within ledge_y ± rise_min: **deep** (kind Up). Otherwise **narrow**: go to the over path (4.4), or refuse `narrow` if `over = "off"` |
| R6 | rise column | p.y + H_st − 1 (inside the capsule) up to ledge_y + H_end + lift, at the current xz | Hit: `ceiling` (overhang or low roof over the climb) |
| R7 | end headroom (0 rays if R2's free column already covers ledge_y + H_st) | at q0 from top_lim + lift up to ledge_y + H_st + lift | Hit: retry with H_cr (a compare, no ray). It fits crouched and `crouch_fallback`: **kind Crouch**. Otherwise `no_headroom` |
| R8 | path low | from the current xz at ledge_y + lift (inside R6's free column or the current capsule), along dir, to X + r + s | Hit: `path_blocked` (a lip or a step on the top) |
| R9 | path high | the same at ledge_y + H_end − 4 | Hit: `path_blocked` (an overhang over the top) |
| R10, R10' | path width | ledge_y + H_end/2 at ±0.7r lateral, along dir to X | Hit: `path_blocked` (a branch or post; MW2 box half-width 15) |
| O1, O2 | over only (4.4) | — | — |

**X (the planar move onto the top) = max(16 in, d + r + s + 1 in).** The whole capsule footprint ends on the top.
MW2's 16 in ledge point is from the player origin, which with a 15 in box leaves the box hanging over the lip. Our
ground probe uses the centre, so a full footprint is the safe equivalent [I].

**Budget:** R0 1, R1 2, R1a 0-1, R2 1, R3 1, R4 2, R5 1, R6 1, R7 0-1, R8-R10 4 = **13-15** for kind Up/Crouch, and
≤ 16 with the over rays (R10' is dropped on the over path, see 4.4). If `RayBudget::cast` refuses, the result is
`budget`: never act on partial data.

### 4.3 Accept rules (all must hold)

- rise ∈ (18, 60] in above the current feet, and ledge_y ≤ ground_y0 + max_height_m.
- Facing within `check_angle` (60°).
- The top is walkable (n.y ≥ 0.7) on the 3-point triangle.
- The path is clear: R6, R8-R10 and (for Up) R7 or the crouch fallback.
- Not occupied by a character.
- The budget is not exhausted.

Refusal reasons (fixed set, logged): `no_face`, `angle`, `ceiling`, `too_high`, `too_low`, `no_ledge`, `slope`,
`edge`, `narrow`, `no_headroom`, `path_blocked`, `over_blocked`, `over_drop`, `occupied`, `budget`.

Rounded rocks: when the slope test fails and budget remains, retry R3/R4 once with X + 8 in, because a shoulder is
flatter further in. The 16-ray cap still holds; on overflow the result is `budget`.

### 4.4 Over (thin walls) [V numbers; I decision]

MW2 crosses over only on faces flagged mantleOver, and only when the far side drops more than 18 in. ER has no flags,
so the decision is: **narrow** (R5) and the far side is clear and drops.

- **X_over** = max(47 in, d + depth_est + r + s), where 47 in is MW2's 16 + 31 and depth_est is the R5 offset r.
- R8 and R9 are cast with length X_over + r + s instead of X + r + s. A hit gives `over_blocked`.
- **O1**: a down-ray at q_over = p_xz + dir·X_over, from ledge_y + lift (on R8's free line) to
  ledge_y − `over_max_drop_m` (2.0 m). Its range starts at ledge_y − 18 in:
  - a hit at or above ledge_y − 18 in means the far side is a ledge or step (MW2 clears OVER here), so the end is the
    O1 hit point. That is kind Over with no drop;
  - a hit below ledge_y − 18 in means kind Over with a gravity drop;
  - a miss gives `over_drop` (no floor within 2.0 m; refused, never jump blindly down a cliff).
- **O2**: a head ray at q_over from ledge_y + lift up to ledge_y + H_st (the capsule fits at the release point). A hit
  gives `over_blocked`.
- R10' is skipped on the over path to keep ≤ 16.
- The over path is allowed only when rise ≤ 57 in (MW2 has over anims only up to row 57). Always true at rise_max 60
  and table rounding.

### 4.5 Stance at the end

Kind Crouch forces `stance.request(Crouch)` at the start, so the eye lowers smoothly through the normal 200 ms
transition. Which stance MW2 leaves you in is I.

### 4.6 Result

```text
Ledge { kind: Up|Crouch|Over, rise, ledge_y, dir, d, X, end, row, drop_y?, rays, why? }
```

`row` is the table row (1.3) by nearest height. It selects the curve and the weapon rule.

---

## 5. Motion (`mantle::Mantle`, pure)

### 5.1 State

```text
Mantle { t, phase: Rise|Cross|Over, start, ledge_y, end, dir, kind, row, v0y, t_rise, t_cross,
         last_valid, gap0, checks, aborted? }
```

### 5.2 Path

All `t` values are multiplied by `time_scale` (default 1.0).

- **Rise** (0 … t_rise = 333 ms; MW2 up frames 0-10):
  - y(u) = y0 + Δ·S_row(u) + v0·t_rise·u(1−u)², with u = t/t_rise.
  - y0 = start.y; Δ = (ledge_y + lift) − y0.
  - S_row is the exe's per-row z curve, piecewise cubic (monotone, Fritsch-Carlson) through the 11 samples.
  - v0 = clamp(vel.y, 0, jump v0 249.8 in/s).
  - The Hermite term u(1−u)² is 0 at both ends with slope 1 at 0, so dy/dt(0) = v0 (no kink coming out of a jump)
    and dy/dt(t_rise) = 0.
  - y is clamped monotone non-decreasing and ≤ ledge_y + lift.
  - If `rise > 60 in` because of user config, t_rise scales by rise/57 [I].
  - No planar motion during Rise. This is the SP order and it makes "never in the wall" hold by construction: x
    stays at gap0 while y < ledge_y + lift.
- **Cross** (t_cross = 167 ms × max(1, X/16), capped at 2×; MW2 up frames 10-15):
  - planar = start_xz + dir·X·C_up(v), where C_up is the exe forward curve and v = (t − t_rise)/t_cross.
  - y is held at ledge_y + lift.
- **Over** (kind Over only; replaces Cross):
  - planar = start_xz + dir·X_over·C_over(v), where C_over is the 16 in up-forward curve followed by the 31 in over
    curve, renormalized.
  - Duration (167 + 333) ms × max(1, X_over/47), capped at 2×.
  - Then **release**: airborne, vel planar 0, vel.y 0, `jumping = false`. Normal gravity drops the player (MW2's
    −18 in over-anim drop; 18 in at 800 in/s² takes 0.21 s). Normal landing runs; the jump slowdown does not, since
    no jump started.
- **End (Up/Crouch)**:
  - pos = end; grounded = true; `normal_fresh = false` (the next substep's ground probe re-measures).
  - vel planar = last step/dt, clamped to walk speed 190 in/s. vel.y = 0 (`max_vertical_end_velocity` 0, V).
  - `air_top_y = pos.y`; no Land event (no dip, no slowdown, no fall damage).
  - Deferred crouch/prone presses apply now.

Totals at defaults: about 0.50-0.67 s for Up, and about 0.83-1.0 s + drop for Over (MW2 0.50 / 1.17 s).

### 5.3 Per-tick re-validation (at most 4 rays per substep, from the mantle budget)

| Phase | Rays | Abort if |
|---|---|---|
| Rise | 1 up ray from p.y + H − 1, length Δy_step + 1 in (ceiling) | hit: `blocked` |
| Cross/Over | 2 horizontal rays (feet + lift, head − 4 in), from p to p_next + dir·(r+s), only if planar step > 0.01 in | hit: `blocked` |
| at u ≥ 0.5 of the climb, and at t ≥ 90 % | 1 down ray at end from ledge_y + 4 to ledge_y − 4 | miss: `ledge_gone` |
| every tick, 0 rays | planar ER delta (`er_delta`, measured, **not** applied) > `er_move_handback_m` 0.30 m | `er_push` (then the existing ErMove handback) |
| every tick, 0 rays | read fail / phys_world 0 | `read_fail` (no write this tick) |

**Abort:**
- the mantle is cleared;
- position = `last_valid` (the last position that passed re-validation, never the blocked target);
- airborne, vel = 0, `air_top_y = pos.y`, `jumping = false`.

Gravity then drops the player at most about 2 m back down the face, below MW2's `fall_min` 200 in, so there is no
damage. An abort during Cross lands the player on the top, because the feet are already above ledge_y.

### 5.4 Writes

- The trigger tick writes `start` (the current position; no jump displacement).
- Each mantle tick writes the validated position through the existing position + proxy writes. Gravity stays owned
  (`OUR_MULT 0`, `OUR_DISABLED 1`). The path only rises or moves over validated space.
- `position_write_ok` (5 m) is unaffected (~3 cm per tick).
- `publish_feet` follows the path.

### 5.5 Events

- `SimEvent::MantleStart { kind, rise, row, dur, rays, trigger }`
- `MantleEnd { dt, kind }`
- `MantleAbort { why, t_frac }`
- `MantleRefuse { why, rise }`

The events are rate-limited in logging (section 9).

---

## 6. Weapon, sprint, ADS, viewmodel, camera

| Item | Behaviour | Label |
|---|---|---|
| Sprint | `sprint.force_end(EndWhy::Mantle)` at start (new variant). Sprint presses during the mantle are ignored. Press mode needs a new tap afterwards (MW2) | V rule (PM_UpdateSprint ends on mantle via the branch); I variant |
| ADS | Cancelled at start (rows ≥ 33). ADS presses are ignored until the raise ends | V (weapon to none) |
| Fire | `MoveOut.fire_blocked = true` from the start until the quick raise ends, for rows 0-4 (≥ 33 in). Rows 5-6 (27/21): the gun stays up and fire is allowed (BG_IsWeaponInactive false; whether MW2 lets it fire is I). Option `low_keeps_weapon = true` | V / I |
| Reload | Rows 0-4: a running reload or rechamber is cancelled (`RELOAD cancelled reason=mantle`), since MW2 switches to weapon none. Rows 5-6: reload continues | I |
| Viewmodel | `vm = "drop"` (default, MW2): a new `VmInputs.mantle_lowered` drives `VmPhase::Drop` then `Raise` with the gun's quick times (UMP45 0.25 / 0.55 s). It uses quick-drop/quick-raise anims if `converted\viewmodel` has them, and otherwise the normal putaway/pullout anims at the quick times [I]. It **must not** go through the M6 SwitchDrop/weapon-switch path (a separate input). `vm = "sprint"` uses the verified sprint pose as the low-risk fallback | V times; I anim choice |
| Stance pose | `vm_stance_pose` airborne = true while mantling. Spread: treat as airborne, `jumped = false` | I |
| Sound | Optional `gear_rattle_plr_mantle` at start via the existing 2D sound path (`sound = true`). Skipped if the WAVs are not converted | V asset; I |
| Camera | The FP eye = feet + stance eye (first_person.rs:665-668). The path is C1 at entry and eases to zero speed, so no filter is needed. `EyeSmoother`'s 1 m reset never fires (steps ~3 cm). `publish_max_m` 1.0 is never approached | I |
| Yaw cap | MW2 caps the view at ±60° from the mantle yaw (V). **Not applied**: ER owns the camera and we do not write it. Turning during the mantle does not change the world-space path (bench b18) | deviation, I |
| HUD | The stance icon shows an up-arrow while mantling (optional, `hud.stance_icon`) | I |

---

## 7. Handbacks, re-base and fall safety during a mantle

- **`Movement::release` (movement.rs:846-901)** calls `pl.mantle_abort(Handback(reason))` **before** setting
  vel = 0 and restoring gravity. It logs `MOVE mantle abort why=handback:<reason> t_frac=..`. Covered reasons: death,
  menu, loading, cutscene, AC-130, nuke, ladder, interact, torrent, throw, knockback, ErMove and read_fail.
- The position ER receives is `last_valid`, which is ray-validated. With gravity restored, ER drops it ≤ 2 m.
- **No resume.** A stale path can never resume after a re-arm, because `enter()` asserts `mantle.is_none()` (unit
  test).
- **F5 off / MW2 mode off mid-mantle:** abort, then release.
- **Re-base:** see P1. **Real warp:** abort `why=warp` before the resync.
- **`air_top_y`** is kept equal to pos.y every mantle tick, so an abort never measures a fall from an older apex.
- **Freefall/lethal-fall logic:** a mantle tick is not "airborne" for `freefall_handback_m`.
- ER fall death while we drive: gravity is off, and the mantle only rises or moves over validated floor. No new
  exposure [I].

---

## 8. Config `[movement.mantle]` (hot unless noted; validated in config.rs like other movement ranges; printed in `log_cfg`)

```toml
[movement.mantle]
enabled            = true
key_mode           = "mw2"     # mw2 = jump held/pressed (MW2 SP) | press | auto (MP automantle perk)
require_forward    = false     # MW2 SP has no forward condition
max_height_m       = 2.0       # 1.0..2.5, top above the ground you left (user choice, I)
max_rise_in        = 60        # 40..80, top above the CURRENT feet (MW2 60, V); >60 scales the rise time (I)
check_angle_deg    = 60        # 0..90 (MW2 60, V)
check_range_in     = 20        # 4..40 (MW2 20, V)
time_scale         = 1.0       # 0.5..2.0 (1.0 = MW2 SP timing)
over               = "thin"    # thin = cross thin walls with a floor behind | off
over_max_drop_m    = 2.0       # 0.5..3.0, far-side floor must be within this
crouch_fallback    = true      # end crouched when only crouch headroom (MW2 clearance = crouch hull)
low_keeps_weapon   = true      # rows 27/21 in keep the gun up (MW2, V)
vm                 = "drop"    # drop (MW2 quick drop/raise) | sprint (sprint pose)
sound              = true
max_rays           = 16        # 12..32, cold (detection + revalidation budget, separate from max_rays_per_tick)
refuse_cache_ms    = 150       # 0..1000
log_refusals       = true
```

Unknown keys are errors (M2 rule). A `[movement.mantle]` block missing from an old `mw2er.toml` means the defaults
above.

---

## 9. Log lines

| Line | Proves |
|---|---|
| `MOVE cfg … mantle=1 key=mw2 max_h=2.00m rise_max=60in angle=60 over=thin vm=drop rays=16` | config loaded |
| `MOVE data … mantle dvars=V rows=7 up=500ms over=667ms curves_sha=…` | data loaded |
| `MOVE mantle check ok=0 why=<reason> rise=..m d=..m rays=N` | refusal. At most 1 per 0.5 s per reason; aggregated in PERF |
| `MOVE mantle start kind=up\|crouch\|over rise=1.42m row=57 ledge_y=.. from=(..) end=(..) dir_deg=.. d=..m X=..m dur=0.67s rays=N trigger=held\|press\|auto air=0\|1 v0y=..` | mantle accepted |
| `MOVE mantle end dt=0.67s pos=(..) dist_end=0.000m kind=.. ground=1 er_ground=.. vel=(..)` | finished |
| `MOVE mantle release kind=over drop_to=..m` / then the normal `MOVE land …` line | over drop |
| `MOVE mantle abort why=blocked\|ledge_gone\|er_push\|warp\|read_fail\|handback:<reason> t_frac=0.42 pos=(..) last_valid=1` | abort |
| `MOVE mantle rebase d=32.00m sign=+` | survived a re-base |
| `MOVE resync why=rebase\|warp d=.. chunk_d=.. e=(..) c=(..) block=.. sign=..` | P1 |
| `ARSENAL lowered why=mantle\|ladder` / `ARSENAL raised why=…` | weapon rule |
| `RELOAD cancelled reason=mantle\|ladder` | reload rule |
| `PERF … mantle_checks=N mantle_rays_max=N mantle_us_max=.. mantles=N` | cost |

All new strings are DLL proof strings (check_m8 #11).

---

## 10. Ladder audit (keep ER third person) — fixes and log lines

### 10.1 Expected sequence [V code; ladder states other than −1 not yet seen in game]

1. **E press:** `MOVE suspend reason=interact E pressed (hold 1.5 s)` and gravity restored (ok=1 or skipped=changed).
   E always reaches ER (move_input.rs:150).
2. **Ladder state ≠ −1:** `MOVE blocked reason=ladder state=N` (new `state=`) and `FP off reason=ladder state=N`.
   ER drives in third person and the viewmodel is hidden (FP off). New `LADDER state from=-1(None) to=0(StartBottom) …`.
3. **Climb:** W/S reach ER (not blanked while not driving). The new B2 fix holds the weapon: `ARSENAL lowered
   why=ladder`.
4. **Dismount:** `LADDER end state=6|7|20 dur=..s dy=..m …`, then state −1. **B4:** the interact hold is cleared once
   the ladder was seen: `MOVE interact cleared why=ladder`.
5. **Re-arm:** after 10 grounded ticks, `MOVE resync why=handback after=ladder` and `MOVE on … after_s=..`. FP comes
   back after 30 clear ticks (`FP on … ladder=-1`), then `ARSENAL raised why=ladder` with the raise anim.

The analyzer accepts both orders: `suspend reason=interact` → `blocked reason=ladder`, or `suspend reason=ladder`.
M5 step 11 is corrected to say this.

### 10.2 Fixes

| Id | Fix | Test |
|---|---|---|
| B1 | `Block::Ladder` in `ac130_logic` (after Torrent, `AC130 blocked reason=ladder`) and in `nuke_logic` (`NUKE blocked reason=ladder`) | core gate tests |
| B2 | `weapon_lowered = on_ladder` (MW2 quick-drops on ladders, V). Blocks the trigger, ADS and reload in task.rs. A running reload or rechamber is cancelled (`RELOAD cancelled reason=ladder`). Crosshair hidden. `ARSENAL lowered/raised`. LMB stays blanked (ER ladder attacks off; `ladder_er_attacks = false` under `[movement]` hands LMB to ER while on a ladder) | core test, wiring check |
| B3 | Don't record an interact handback in the flap guard when an ER-confirmed reason (ladder, talk, cutscene, throw, torrent) took over during the hold. Analyzer FAIL on `WARN signal interact muted` | core test |
| B4 | Clear `self.interact` when the ladder trigger is seen after the E press | core test |
| — | `const` asserts: `offset_of!(ChrInsModuleContainer, ladder) == 0x78`, `CSChrLadderModule.state == 0x18`, `top == 0x20`, `bottom == 0x30` (fp_read.rs, next to lines 21-34) | compile |
| — | Consecutive `fp_player` read errors ≥ READ_FAIL_TICKS give `Reason::ReadFail` with `field=fp:<name>`, instead of silently reading the ladder, death and torrent as false (movement.rs:532, 849) | core test |
| — | config: reject E for `keys.reload`, `toggle`, `fp_toggle`, `ac130`, `ac130_exit` | config test |
| — | MOVE probe line gains `ladder=` | strings |

### 10.3 `LADDER` log (pure `mw2er_core::ladder_log::LadderLog`, fed by the existing `fp_player` every tick)

- `LADDER state from=<a>(<name>) to=<b>(<name>) pos=(..) top=(..) bottom=(..) er_ground=.. move=<MoveCtl> fp=0|1`.
  It is emitted on the start, the end, the first up, down or idle, an attack, a coast and damage. The climb steps 2-5
  and 8/9 are collapsed into counters.
- `LADDER end state=<n> dur=..s dy=..m ups=N downs=M attacks=K coast=0|1 pos=(..)`.
- Names come from the vendor enum [D]. 17 and 19 print as `?`.

---

## 11. Analyzer rows (`analyze_log.py` MOVE_ROWS + a new `analyze_m8.py` in the M7 pattern)

| Row | PASS | FAIL | NEEDS-DATA |
|---|---|---|---|
| `mantle` | For each `mantle start`, an `end` or `abort` follows: dt within ±10 % (+1 tick) of the planned dur; `dist_end` ≤ 0.05 m; `er_ground=1` within 10 ticks of end; no `position_insane`; no damaging `MOVE land`/`FALL` within 2 s; no `resync why=warp` within 1 s; every abort followed by a valid airborne or ground state | any of these missing | no `mantle start` |
| `mantle refusals` (INFO) | counts by reason; flags > 20 refusals/min of one reason as WARN | — | — |
| `mantle perf` | PERF windows with `mantle_checks > 0` have `ticks_per_s ≥ 55`, `mantle_rays_max ≤ max_rays`, and main `rays_max ≤ max_rays_per_tick` | — | no checks |
| `rebase` | `resync why=rebase` present for ≥ 30 m jumps without loading, with no sprint end at them | `why=warp` on a 32 m grid jump with no loading | no jump > 5 m |
| `ladder handback` | For each LADDER episode: a suspend (interact or ladder) at or before the start; no `MOVE on` while state ≠ −1; a gravity restore between the suspend and the start; `FP off reason=ladder` within 0.1 s if FP was on; `blocked reason=ladder` or `suspend reason=ladder` | any missing; FP on while ladder ≠ −1 | no LADDER line |
| `ladder dismount` | After `LADDER end`: `resync why=handback after=ladder` and `MOVE on er_ground=1` within 2.0 s (INFO latency; FAIL > 10 s); `FP on … ladder=-1` within 2 s if FP was on; no warp, `ground_disagree`, freefall or damaging land within 3 s | — | — |
| `ladder weapon` | no SHOT or RELOAD start between LADDER start and end; `ARSENAL lowered why=ladder` present | SHOT on a ladder | — |
| `ladder input` | E handbacks counted; no `WARN signal interact muted`; no `AC130 on`/`NUKE activate` during LADDER | any | — |
| `ladder signal` (INFO) | Flags an interact suspend with ER vertical motion > 1 m while ladder is still −1. That would point to a wrong ladder signal | — | — |

Plus the P2 fixes to the first-person and jump rows.

Fixtures:
- `fixtures\m8_mantle_session.log`, synthetic, PASS. Mutations: dt off, no end, damage after, end off by 0.2 m, warp
  after, rays over budget.
- `fixtures\m8_ladder_session.log`, synthetic, PASS. Mutations: missing restore, MOVE on mid-ladder, SHOT on ladder,
  FP not off, muted interact, AC130 on during ladder, slow dismount of 12 s.
- The run-17 log excerpt: rebase FAIL as expected (before the fix), mantle NEEDS-DATA, ladder NEEDS-DATA.

---

## 12. Offline oracles (pass criteria)

### 12.1 Pure logic (`cargo test -p mw2er-core`, 0 failed; at least 1 named test per category, matched by prefix)

**Synthetic world additions** (move_collide.rs):
- `Convex::prism(n, chamfer)` for rounded rocks;
- `Convex::wedge` for sloped tops;
- overhangs (aabb on aabb);
- thin walls;
- ceilings.

`inside()` becomes `pub(crate)`.

**Tests:**
- `mantle_detect_`:
  - ledges of 0.6/1.0/1.5 m from the ground, and 2.0 m from the jump apex, are accepted, with ledge_y ± 0.1 in and
    the right kind and row;
  - 2.5 m is refused `too_high` (ground and apex);
  - 0.40 m gives no mantle (step-up handles it);
  - a 1.5 m ledge with a ceiling 1.0 m above the top is refused `no_headroom`;
  - a ceiling 1.45 m above the top gives kind Crouch;
  - a 35° wedge top is refused `slope`;
  - a rounded prism: accepted only where n.y ≥ 0.7, and the retry-deeper path is exercised;
  - an overhanging lip is refused `path_blocked`;
  - a low roof over the player is refused `ceiling`;
  - a face at 70° to the view is refused `angle`;
  - an occupied top is refused `occupied`;
  - a budget of 10 is refused `budget` with no start.
- `mantle_over_`:
  - a 0.3 m thin wall, 1.0 m high, with a floor behind gives kind Over;
  - a 6 m drop behind is refused `over_drop`;
  - a floor 0.3 m below the far top gives Over with no drop;
  - a far-side obstacle gives `over_blocked`;
  - `over = off` gives `narrow`.
- `mantle_origin_` (axiom): for every ray in the pattern, a mutation that moves the origin into a solid can only turn
  an accept into a refuse, never a refuse into an accept (property test over 500 random synthetic rocks).
- `mantle_path_`:
  - C1 at entry: |dy/dt(0) − v0| < 1 %;
  - y is monotone;
  - no planar motion while y < ledge_y + lift;
  - the end is exact (≤ 0.001 in);
  - the duration equals the table (500 ms + cross) ± 1 tick at 30/60/144 Hz;
  - `time_scale` is honoured;
  - the curve samples equal `movement.json`.
- `mantle_state_`:
  - start, end, and abort at t_frac 0.1/0.5/0.9;
  - a mid-path box insert aborts `blocked` with `last_valid` outside all solids;
  - a ledge removed at 60 % aborts `ledge_gone`;
  - a 0.4 m `er_delta` aborts `er_push`;
  - a handback at every phase clears the mantle and `enter()` refuses a stale one;
  - re-base translation mid-mantle gives the same end relative to geometry;
  - a real warp aborts.
- `mantle_never_inside_`: 64 points along the capsule axis per tick stay outside every Convex, for all bench worlds
  and all aborts.
- `mantle_trigger_`:
  - crouched or prone refuses (0 rays);
  - jump held in the air catches;
  - a grounded press that mantles emits no Jump event and sets no `slow_ms`;
  - a failed grounded check jumps normally;
  - `key_mode` press/auto;
  - the negative cache skips and re-opens on 0.1 m of motion;
  - at most 1 detection per tick across substeps.
- `mantle_end_`: grounded, vel.y = 0, planar vel ≤ 190 in/s, no Land event, deferred stance keys applied.
- `mantle_weapon_`:
  - rows 0-4 lowered, fire_blocked until raise end, ADS cancelled, reload cancelled;
  - rows 5-6 keep the gun up;
  - sprint is force-ended with `EndWhy::Mantle`.
- `rebase_`: the P1 tests (42 logged pairs as vectors give Rebase; teleports give Warp; both signs).
- `ladder_`:
  - MoveCtl: Driving → Release(Interact) → Blocked(Ladder) → 20 ungrounded clear ticks still Arming → 10 grounded
    → On;
  - the ladder on the E tick → Release(Ladder);
  - B3 flap guard, B4 interact cleared;
  - FpMachine with [−1, 0, 2, 3, −1 (1 tick), 8, 6, −1×30] → exactly one Off(Ladder), no re-log on the flicker, On
    at clear tick 30;
  - LadderLog collapse and summary;
  - ac130 and nuke `Block::Ladder`;
  - `weapon_lowered` blocks trigger, ADS and reload;
  - an fp read error gives ReadFail.

### 12.2 movebench (`mod\movebench`, scenarios b13-b20, each at 30/60/144 Hz)

| Scenario | Pass |
|---|---|
| b13 mantle 1.0 / 1.5 m from a grounded press, 2.0 m by a held jump | kind Up; time = planned ± 1 tick; end on top ± 0.05 in; grounded next tick; no Jump event on the grounded mantle |
| b14 refuse 2.5 m | refusal `too_high`; ordinary jump and landing; no position change beyond the jump |
| b15 refuse no-headroom (ceiling 1.0 m over a 1.5 m top) | `no_headroom`; ceiling 1.45 m gives kind Crouch with stance crouch at end |
| b16 thin wall 0.3 m × 1.0 m, floor behind | kind Over; the player ends on the far side, never inside the wall, lands without damage |
| b17 thin wall with a 6 m drop behind | `over_drop`; the player stays on the near side |
| b18 turn 90° during the mantle | same end as b13 (± 0.01 in) |
| b19 rounded rock (prism) and 35° slope top | accept only on a walkable crown; slope refused |
| b20 abort: box inserted at 50 %; handback at 30 % | aborts, `last_valid` outside solids, falls ≤ 2 m, no fall damage |

Bench-wide for every M8 scenario:
- mantle rays per tick ≤ 16;
- total rays per tick ≤ 48 (main ≤ 32);
- no capsule-axis point inside geometry;
- b1-b12 unchanged (same numbers as check_m5).

### 12.3 Converter, config, wiring, strings

- `convert_movement.py --self-test`:
  - the 46 mantle sites byte-check;
  - the xanim end deltas are within 1 in;
  - the existing keys are byte-identical;
  - ≥ 6 mutations (wrong site byte, wrong curve, wrong duration, missing label, …) FAIL.
- Config: every `[movement.mantle]` key out of range is an error; an unknown key is an error; a missing block gives
  the defaults; E is rejected for the 5 extra keys.
- `m2_static_checks.py mantlewiring` (each self-test mutation must FAIL):
  - every mantle ray goes through `RayBudget::cast`;
  - `release()` calls `mantle_abort` before the gravity restore;
  - `enter()` asserts no mantle;
  - the mantle position write passes through `write_plan`/`position_write_ok`;
  - no `land()` call on mantle end;
  - `fire_blocked` reads the mantle;
  - the task fire gate reads `on_ladder`;
  - the ladder feeds the movement, FP, ac130 and nuke gates;
  - `classify_jump` gets vectors.

---

## 13. `tools\scripts\check_m8.ps1`

`check_m8.ps1 [-AllowSkip] [-SkipM7]`, PowerShell 5.1, with the same structure as check_m7. It writes only to
`work\check_m8\`, `work\m8\` (except the probe scripts, which it only reads) and `work\t`. It never launches anything.
It uses the PATH line from M7 and `%LOCALAPPDATA%\Programs\Python\Python314\python.exe -I`.

| # | Check | Pass |
|---|---|---|
| 1 | `check_m7.ps1`, which runs m6..m2 | exit 0 |
| 2 | exe static pin: sha256 = PINNED_EXE_SHA; `work\m8\mantle_sites.py` 46/46 OK | PASS |
| 3 | xanim dumps: `dumps\m8\code_post_gfx.log` and `common.log` report 0 warnings / 0 errors; the 10 SP files' sha256 match the movement.json pins | PASS |
| 4 | `convert_movement.py --self-test` (12.3) and a regenerate that is byte-identical to `converted\movement.json` | PASS |
| 5 | `cargo build --release` | 0 warnings |
| 6 | `cargo test --workspace`, plus the 12.1 categories by name | 0 failed; every category present |
| 7 | movebench all 20 scenarios × 3 rates (12.2) | all PASS |
| 8 | config (12.3) | PASS |
| 9 | wiring `mantlewiring` + mutations | all caught |
| 10 | analyzer: the M8 fixtures PASS, the mutations FAIL, the run-17 excerpt gives the expected rows, the P2 fixes on sessions 16-18 | PASS |
| 11 | DLL proof strings: `MOVE mantle start`, `MOVE mantle end`, `MOVE mantle abort`, `MOVE mantle check`, `MOVE mantle rebase`, `mantle_rays_max=`, `LADDER state`, `LADDER end`, `blocked reason=ladder state=`, `interact cleared why=ladder`, `ARSENAL lowered`, `AC130 blocked reason=ladder`, `NUKE blocked reason=ladder`, `resync why=rebase`; built DLL == staged | all present |
| 12 | hygiene + no game-folder or `%APPDATA%\EldenRing` writes (as M7) | PASS |

Final line: `M8 OFFLINE ORACLES: PASS`. The exit code is 0 only if all checks pass. A SKIP counts as a failure unless
`-AllowSkip` is given. `-SkipM7` always exits non-zero.

---

## 14. In-game test (run 18). The user runs it; Claude never launches anything

HDR off, launched with me3 through `launch.ps1`, MW2 mode on (F5), first person on (F6).

1. **Re-base:** walk or sprint 200 m in a straight line across open ground. Expect `MOVE resync why=rebase … sign=+|-`
   lines, no `sprint end why=handback` at them, and sprint not interrupted. (Settles P1.)
2. **Low rock (0.6-1.0 m), e.g. the rocks around the Church of Elleh:** walk at it and press Space. Expect `MOVE
   mantle start kind=up rise=0.7xm row=27|33 …` with no `MOVE jump` line. Row 27/21: the gun stays up. Row ≥ 33: the
   gun drops and raises (`ARSENAL lowered why=mantle`). Then `MOVE mantle end … ground=1`, with no landing dip.
3. **Waist-to-head ledge (1.2-1.5 m):** hold Space while walking into it. Expect the same as step 2, with a smooth rise
   and the gun lowered.
4. **Tall ledge (1.7-2.0 m):** run up and hold Space. Expect `MOVE jump`, then `MOVE mantle start … air=1 v0y=…`.
5. **Too tall (cliff > 2.5 m):** expect a normal jump and a rate-limited `MOVE mantle check ok=0 why=too_high`.
6. **Thin wall or fence of ruins:** hold Space. Expect `kind=over`, then `mantle release`, then `MOVE land` on the far
   side with no damage.
7. **Interrupt:** start a mantle, then open the menu (Esc) mid-climb. Expect `MOVE mantle abort why=handback:menu
   last_valid=1`, the player safe on or below the ledge, and no damage.
8. **Ladder:** at a ladder (e.g. a Stormveil or Limgrave tower ladder), press E. Climb up with W to the top. Then go
   back down to the bottom.
   - Expect in order: `MOVE suspend reason=interact`, `MOVE blocked reason=ladder state=N`, `FP off reason=ladder`,
     `LADDER state …`, `ARSENAL lowered why=ladder` and `LADDER end state=6`.
   - Then `MOVE interact cleared why=ladder`, `MOVE resync why=handback after=ladder`, `MOVE on … after_s=<2`,
     `FP on … ladder=-1` and `ARSENAL raised`.
   - Try LMB and F7 on the ladder. Expect no shot and `AC130 blocked reason=ladder`.
   - Do 3 more ladder trips; expect no `interact muted`.
9. Close normally. Run the analyzer. The rows `mantle`, `mantle perf`, `rebase`, `ladder handback`, `ladder
   dismount`, `ladder weapon` and `ladder input` should PASS. PERF should be about 60 ticks/s, with
   `mantle_rays_max` ≤ 16.

---

## 15. Verified vs inferred

**Verified:**
- all MW2 mantle numbers: dvars, geometry, gates, trigger (jump held, every tick, ground or air, no forward
  condition), transition table, SP 500/667 ms timing and curves, weapon rule (rows ≥ 33 drop, 27/21 keep), ladder
  quick-drop, end-velocity clamp and yaw cap;
- the `cast_ray` API (point only);
- the run-17 PERF budget (rays_max 28-30 / 32);
- the re-base bug (42/42 warp);
- the ladder code path and bugs B1-B4 in code;
- the analyzer FAILs (B6);
- the jump-slowdown cause of the short apexes;
- `ladder=-1` at rest.

**Inferred (run 18 settles each one by the named line):**
- procedural ledge detection matches what players see as climbable: the `mantle start/check` lines;
- the start-inside-solid ray miss in ER (the axiom): no `mantle abort why=blocked` right after a start;
- the 2.0 m cap and the airborne reach: `rise=` / `air=1`;
- the full-footprint landing X: `dist_end`, `er_ground=1`;
- the C1 blend feel (user report);
- the quick drop/raise anim choice: `ARSENAL lowered/raised`;
- whether MW2 fires during low mantles;
- the stance after a crouch mantle;
- the re-base sign and rule: `resync why=rebase sign=`;
- ladder state values ≠ −1 and their names: the `LADDER` lines;
- whether the ladder signal is correct at all: the `ladder signal` INFO row;
- that ER foliage or objects missed by filter 0x2000058 do not block a mantle: `abort why=er_push`.

---

## 16. Non-negotiables (the implementer must not change these)

1. **P1 (re-base) and P2 (analyzer) land first**, with their tests, before any mantle code is wired into the DLL.
2. **SP numbers only** (exe-bound `player_mantle_*`, 500 ms up / 667 ms over, rise then forward, 18..60 in window,
   60° angle, 20 in reach, rows and weapon rule). The MP set is `info` only. Every number goes through
   `movement.json` with a V/D/I label. The 46 sites byte-check, or the converter fails.
3. **MW2 trigger:** standing only, jump held or pressed, checked before the jump each tick, ground or air. A grounded
   mantle replaces the jump (no Jump event, no slowdown). No forward or fall-speed gate unless configured.
4. **Rays:** every mantle ray goes through `RayBudget::cast` on a **separate** budget of ≤ 16. There is at most one
   detection per tick, on gated ticks only. Detection never uses the main budget's vertical reserve. Every ray starts
   inside the current capsule or on a segment an earlier ray proved free. A budget refusal means no mantle.
5. **Never write an unvalidated position.** No planar motion while the feet are below ledge_y + lift. Re-validate every
   tick. An abort goes to `last_valid`, never to the blocked target. No collision-free root motion as in MW2.
6. **No Land event on a mantle end** (no dip, slowdown or damage). vel.y = 0 at the end. `air_top_y` follows the path.
7. **`release()` aborts the mantle before restoring gravity.** `enter()` never resumes a mantle. A real warp aborts. A
   re-base translates the mantle path.
8. **ER push-out is measured, not applied, mid-mantle.** A planar ER delta > 0.30 m aborts.
9. **Over only with a far-side floor within `over_max_drop_m`.** Never drop blindly over a thin wall.
10. **Weapon:** rows ≥ 33 lowered with fire, ADS and reload blocked until the raise ends; 27/21 keep the gun. The
    mantle viewmodel uses its own input, never the M6 weapon-switch path. Sprint is force-ended.
11. **Ladder stays ER third person.** The ladder weapon block (B2), the AC-130/nuke ladder gates (B1), the flap-guard
    fix (B3), the interact-clear (B4), the offset asserts, the read-fail fallback and the `LADDER` lines all ship in M8.
12. **PERF stays ~60 ticks/s.** Main `rays_max` ≤ 32 and `mantle_rays_max` ≤ 16 are reported separately.
13. `check_m8.ps1` runs `check_m7.ps1` first, with 0 build warnings. Print `M8 OFFLINE ORACLES: PASS` only when all
    checks pass. Never launch the games, me3 or Steam. Never write into game folders.

---

## 17. As built (2026-10-18, offline only: nothing has run in game)

Order kept: P1 (`move_logic::classify_jump` on vectors, tests `rebase_*`) and P2 (analyzer `nuke` reason, re-base CAMFRAME
grace, jump row against each jump's own v0) first, then the mantle and the ladder audit.

Files:
- core: `mw2er-core\src\mantle.rs` (+ `mantle_tests.rs`), `ladder_log.rs`; `move_sim.rs` (mantle gate, detection, path
  substep, abort, translate), `move_logic.rs` (`classify_jump`, `InteractFlap`, `ladder_*` tests), `movement_data.rs`
  (`MantleData`), `move_collide.rs` (`Convex::rock` / `wedge`, `inside`, `World::axis_inside`), `sprint.rs`
  (`EndWhy::Mantle`), `vm_anim.rs` (`VmInputs::lowered`), `ac130_logic.rs` / `nuke_logic.rs` (`Block::Ladder`),
  `config.rs` (`[movement.mantle]`, `ladder_er_attacks`, E rejected for 5 keys), `tests\real_data_m8.rs`.
- DLL: `movement.rs` (re-base / warp, mantle lines, release abort before the gravity restore, enter re-arm block, ER push
  measured not applied, B3 / B4, fp read-fail fallback, `LADDER` lines, PERF fields), `task.rs` (weapon gate: fire / ADS
  / reload / sim hold, quick drop / raise, `RELOAD cancelled reason=`, `ARSENAL lowered|raise|raised why=`, ladder LMB),
  `game\fp_read.rs` (ladder top / bottom + 4 offset asserts), `ac130.rs`, `nuke.rs`, `hud.rs` / `shared.rs` (no
  crosshair while the gun is down).
- tools: `convert_movement.py` (46 `MANTLE_SITES`, table, 10 SP xanims, 34 guns' quick times, MP info, pre-M8 sha pin,
  `--self-test-m8`), `analyze_m8.py` (9 rows, 2 fixtures, 13 mutations), `analyze_log.py` (P2, M8 rows, run-17 excerpt),
  `m2_static_checks.py` (`mantlewiring` 10 checks / 20 mutations, `m8strings` 23), `check_m8.ps1` (12 checks),
  `movebench --mantle` (B13-B20, `work\m8\bench_m8.json`), `mw2er.toml` (`[movement.mantle]`).

Deviations from this design [I unless noted]:
1. Ray pattern. R1' is cast only when R1 does not settle the normal. R1a and R6 are one rise-column ray, cast first. The
   top is probed at the lip (2 in + skin past the R0 hit point, which also handles a rock corner facing the player) and
   at the end centre (the design's R5 slot); the side probes sit half a radius ahead of the centre (the design's
   collinear R4 / R4' give no triangle). "Deep" = the centre is at most 4 in below the lip (or higher: a face leaning
   back). R8 (feet line) reaches only the end point (a crown rising ahead is ground); R10 / R10' fan out from the free
   axis. Typical 13-14 rays; the deeper retry fits 16 in the common case, else `budget`.
2. Re-validation in the forward move: 3 horizontal rays (feet: only the axis step + skin; waist and head: + radius) + the
   ledge check, at most 4 per substep.
3. Over always releases into gravity at the far point (the no-drop case is a <= 20 in step down with a normal Land,
   never damage) instead of ending grounded on the O1 point.
4. The 2 in lift is set down smoothly once the axis is past the lip (no snap at the end).
5. A 35 deg top is walkable under MW2's 0.7 (45.6 deg); tests and B19 refuse 50 deg and accept 35 deg.
6. The mantle sites stay in their own `MANTLE_SITES` (so the M5 constants are byte-identical); `--sites` prints all 101
   and iw4dis decodes all of them; the M8 converter mutations run under `--self-test-m8` (check_m5 pins `--self-test`).
   The transition and name tables are in `.data` (V), not `.rdata`.
7. Re-base: the map/block id is not read; the load rule is "no load-counter change within 1 s" (`load_age=` logged).
8. P2(b): run 15/16 (log sessions 16/17) still FAIL first person: their re-base CAMFRAMEs were followed by a real
   `FP off reason=cam_gate` (the bug fixed in run 16); run 17 (session 18) PASSes. Check 10 uses a verbatim run-17
   fixture (`fixtures\run17_excerpt.log`, sha pinned). The M8 rows are N/A on pre-M8 logs (not NEEDS-DATA), except
   `rebase`, which judges any log with > 5 m resyncs (run 17: FAIL, 12 grid re-bases logged as warps).
9. Not done: the gear-rattle foley (not converted; `sound` is reserved), the HUD up-arrow, the MW2 yaw cap (as designed).
10. `check_m5.ps1` check 7 updated in place for `ladder_er_attacks` and the `[movement.mantle]` subtable (judged by
    check_m8), as M6 did for `weapon_move_class`. New lines: `MOVE mantle settle er_ground= ticks=` (the 10-tick ER
    ground rule) and `pos=` on every `MOVE suspend|off` line (the ladder-signal row). `no_face` refusals are counted
    in PERF (`mantle_refusals=`) but never logged.
11. B3 is `move_logic::InteractFlap`: an interact handback is counted only when its hold ends with no ladder / talk /
    cutscene / throw / Torrent seen. The fp read-fail fallback maps the field to `fp:<name>` (fixed labels).
12. Quick drop / raise per gun from its weapon file (34 guns, V); the UMP45 times are the fallback.

Results (offline): cargo build --release 0 warnings; 481 workspace tests (13 M8 categories, 44 tests) + real_data_m8;
movebench B13-B20 at 30/60/144 Hz 219 checks 0 failed, B1-B12 unchanged; converter 46 sites + iw4dis 101 sites, 10/10 M8
mutations, regenerate byte-identical, pre-M8 keys byte-identical; mantlewiring 20 mutations; analyzer M8 fixtures 9/9
rows, 13 mutations; check_m8.ps1 (see MODLOG).

### 17.1 Review fixes (2026-10-18, offline only: still nothing has run in game)

10 findings (6 major, 4 minor); all applied, one part of a minor declined (item 10).

1. **Rock reaching into the climb** (major). The detection checked the capsule radius only at the knee. Now: a face
   nearer than r + skin/2 refuses `path_blocked`; **Rf**, one down-ray at the capsule's front (r - 0.25 in ahead of the
   axis) from R9's free line to the knee, sees any rock bulging toward the player anywhere over the climb (the review's
   26-32 / 45-55 in slabs, 12 or 8 in proud, and rock over the head at 80-90 in); the rise re-validation adds a head ray
   toward the face (r + skin/2) at the head's new height. New oracle: `World::capsule_inside` (the capsule radius, not
   only the axis) on every mantle tick, in the tests and in movebench B21.
2. **Raised rim** (major). The forward move now crosses at `y_cross = max(lip, end centre) + lift`; R8 and the
   forward feet ray run 0.25 in under it (the feet's own path); over a rim (lip > centre + 0.25 in) the planar move ends
   first and a 50 ms set-down runs straight down the end column (R5 proved it free) [I]. The lip probe moved from
   skin + 2 in to 1 in + skin/2 past the face, so a rim down to ~1.6 in deep is probed; a narrower one is refused by R8.
   Tests: rims 2.0-2.9 in high x 2-8 in deep at 30/60/144 Hz, a 1 in rim refused, a step on the top refused; B22.
3. **Hitch budget** (minor). The mantle budget per tick is `max_rays + 4 x (substeps - 1)` (`SUBSTEP_RAYS` = 4); an
   accepted detection that leaves fewer than 4 rays per remaining substep is deferred to the next tick (no refusal, no
   cache; counted as `mantle_deferred`). Default `max_rays` 24 (range 20..40; `DETECT_MAX_RAYS` = 20). B23 (dt 0.1 s).
4. **position_insane** (minor). The resync now aborts the mantle first and logs `mantle abort why=position_insane`
   (mantlewiring check 11 + mutation).
5. **check_m8 flake** (major). The libtest status and a test's PASS line could share a line; real-data tests now run
   `--test-threads=1` in check_m2..m8 and the PASS lines are matched anywhere in the line.
6. **Timing** (major). The forward move keeps MW2's 167 ms and is eased over the full-footprint X (~34 in) instead of
   167 ms x X/16, which always hit its 2x cap (0.667 s). A mantle up now takes MW2's 0.500 s (+50 ms over a rim); over:
   333 + 167 + 333 ms, then the gravity drop. Labelled I in movement.json (`mantle_cross_distance_eased`); tests and B13
   compare the plan with `md.up_s`, not with the code's own number. Peak forward speed ~6.7 m/s for ~0.1 s [I].
7. **Undercut faces and 18-20 in tops** (major). R0 sits at step + 0.25 in (was step + lift: a 19 in rock was neither
   stepped nor mantled); when it misses, **R0b** casts once more half way up the window (an undercut shelf, a slab or a
   cart bed). `no_face` is logged, sampled every 5 s with its count, the feet and the view (`fwd=`). R0 is now a
   free-on-miss ray (the axiom test lists R0b as the refusing one).
8. **Ladder rows** (major). `ladder signal` FAILs when an E handback moved the player > 1 m vertically with no LADDER
   episode; the other ladder rows then read `NEEDS-DATA (ladder signal suspect)`. New mutation: LADDER lines removed,
   the climb kept.
9. **Refuses everything** (minor). `mantle` FAILs with no start and >= 10 refusals (logged `n=` or PERF), naming the
   dominant reason; fixture `fixtures\m8_refuse_all_session.log` + mutation.
10. **Viewmodel clips / unused data** (minor). The converter reads each gun's `quickDropAnim` / `quickRaiseAnim` (V) and
    whether it is the same xanim as `dropAnim` / `raiseAnim`; 20 of the 34 guns reuse their putaway / pullout (our
    clip is MW2's exactly), 14 have dedicated `*_quick` xanims. `ARSENAL lowered|raise` print `mw2_drop= vm_drop=exact|
    putaway mw2_raise= vm_raise=exact|pullout`. **Declined:** converting and playing the 14 dedicated quick xanims (new
    clip slots through convert_arsenal / vm_assets / vm_anim and the M6 byte-identical pins); those guns keep the
    putaway / pullout at the quick times [I]. The end clamp now uses `max_vert_end_vel` (vel.y = min(vel.y, dvar),
    never downward on the top). `mantle_check_radius` (0.1 in, MW2's trace radius) is not applicable: ER's cast_ray is a
    point ray.

Ray counts: detection typically 13-15, at most 20 (R0b + a deeper retry + R7); re-validation <= 4 per substep.

## 18. Run 18 fixes (2026-10-18, offline only: nothing has run in game)

Run 18 (the first in-game M8 test): "the jumping turned to third person and I got stuck inside of a wagon". Evidence in
the run-18 session of `logs\mw2er.log` (14:10-14:15, now `tools\scripts\fixtures\run18_excerpt.log`, sha pinned).

### 18.1 Camera gate (the third-person flashes)

- [V log] 16x `FP off reason=cam_gate why=dist`, the CAMFRAME before each at cam_chest 0.08-0.29 m (< the 0.3 m bound),
  er_cam_dist 0.12-0.46 m at the re-arms: ER's own third-person camera collision pulls its camera next to the chest
  against the wagon; the 8-tick grace (run 16) was exceeded.
- Fix: `GateParams.close_ok` (pure, `cam_gate.rs`): while first person drives the render camera (`FirstPerson::
  close_cam_ok`: `[first_person] cam_gate_close_ok` (default true), FP enabled, On or wanted, MW2 mode on) a distance
  below `min_dist_m` is OK; > 15 m, the identity placeholder, non-finite and the > 5 m jump re-settle (re-base grace)
  are unchanged. CAMFRAME lines carry `close_ok=`.
- Shots: `aim::check_shot_fp` drops only the 0.3 m lower bound and only when the bullet leaves from the FP eye (`fp_eye`);
  the 15 m upper bound, the origin <= origin_ahead + 3 m from the chest, the unit direction and the range stay. In third
  person (and during FP arming, when the origin is still ER's camera) the M3 check is unchanged.

### 18.2 Collision filters (static read of eldenring.exe 2.7.1.0, `tools\erscan`, never run)

- [V] `CSPhysWorld::cast_ray` (0x140c71e00) has exactly one caller in the exe (0x1403fbc39), with `mov edx,2000058h`
  (the map filter M5-M8 used). The wrapper stores the filter in the hknp cast params at +0x14 (`mov [rsp+54h],r15d`,
  params at rsp+40: +0x00 shape-tag filter from hknpWorld+0xb00, +0x08 the collision filter CSHavokMan+0xC0, +0x10 u16
  0xFFFF, +0x18 user data = owner, +0x20 = 2, +0x24 = 0xFB), registers the owner in the collector (`mov [rcx+0A0h],rdx`
  at 0x140c5abe0; the collector's hit filter compares hit owners with it) and calls hknpWorld::castRay 0x14187F7D0.
- [V] ER's follow camera (0x1403b6xxx-0x1403bbxxx, the area erfps2 names FOLLOW_CAM_FOLLOW / UPDATE_FOLLOW_CAM) passes
  `edx = 0x5B` at 4 sites (0x1403b746f, 0x1403b8da8, 0x1403b97be, 0x1403bab31) to the sphere-cast wrappers
  0x140c73970 / 0x140c738e0 -> 0x140c73560, which store it unchanged in the same params slot +0x14 (`mov [rsp+34h],edx`
  ... `mov [rsp+74h],eax`) with the same collision filter object, then hknpWorld::castShape 0x14187F860.
- [V] Other filter immediates passed to CSPhysWorld wrappers: 0x5d (23 sites, AI / character code), 0x5c, 0x5e, 0xe0,
  0x15, 0x16, 0x1c, 0x3a, 0x61, 0x63, 0x68, 0x69, 0x6c, 0x70, 0x2000062, 0x2000064 (`work\m8\erscan\physworld_calls.txt`).
  erfps2's soft lock-on casts a sphere with 0x2000058 and receives character hits.
- [I] The low byte selects a query layer of FromSoft's collision filter (its table is built at run time; not decoded),
  bit 25 is a flag of unknown meaning. That 0x5b sees assets (wagons) is inferred from run 18: ER's camera collided with
  the wagon (cam_chest 0.08-0.29 m), while our 0x2000058 rays let the capsule walk into it.
- Fix: `[movement.rays]` `map_filter = 0x2000058`, `object_filter = 0x5b` (0 = off), `mode = "union"` (both, the nearer
  hit) | `"map"` (M8) | `"object"`. `HavokRays` casts the map filter, then the object filter only to 2 cm past the map
  hit; `hits=map:/obj:/both:` (both = the same point within 1 cm) on the state, er_delta, mantle check and mantle start
  lines; the cfg line prints `filters=union(map:0x2000058+object_cam:0x5b)`. The run decides: object-only hits = what
  the map filter misses; both >> map-only means `mode = "object"` would halve the rays.

### 18.3 Body probes (a slab between the probe heights)

- [I] A cart bed at ~0.6-1.0 m lies between the standing sweep's knee (0.51 m) and chest (1.72 m) rays: the controller
  walked the capsule into it even where a filter sees it; prone under it the up ray then blocked standing (`stance
  blocked to=stand why=headroom`), ER pushed the proxy out on ~15 % of the ticks (`pushout=1`, er_delta ~0.04 m,
  slope_er 86-94 deg): stuck for a minute. 34 mantle checks refused `path_blocked rays=3` = a face within r + s/2 of the
  axis (the capsule already inside).
- Fix (`[movement.rays] body_probes = true`): a waist ray (half way between knee and chest when > 0.45 m apart: standing
  1.11 m, crouched 0.85 m) and the front body column (`move_collide::body_column`: one down ray at the capsule's leading
  edge after the move from hull - 2 in to knee + 0.1 m, confirmed by one horizontal ray at the hit height). Stairs and
  walkable slopes stay under its lower end. `max_rays_per_tick` 32 -> 48 (bench B9 at 30 Hz peaks at 45).

### 18.4 Unstick

`move_logic::Unstick` (pure): W/A/S/D held >= 80 % of a 2.5 s window, the feet within 0.5 m of the window start, and ER's
push-out applied or a probe inside geometry (a sweep hit deeper than the skin, a backed-off mantle start) on >= 10 % of
its ticks -> `MOVE unstick n= hold= ...`, then `Reason::Unstick` (Suspend, priority after ground_disagree; gravity
restored, stance reset to stand): ER's physics resolves the overlap for the hold; the re-arm resyncs (`resync
why=handback`). Holds 1 / 2 / 4 / 8 s (doubling within 60 s, capped at `max_hold_s`), at most 4 per 60 s, then `WARN
unstick capped` and a rest until the window frees. Pressing into an ordinary wall gives no evidence (B26). Config
`[movement.unstick]` (9 keys).

### 18.5 Mantle re-tune (run-18 refusals: no_face 101, path_blocked 53, too_high 28, edge 13)

- **Depenetration**: a face within r + s/2 is backed off by up to `[movement.mantle] depenetrate_m` (0.30 m) when two
  rays from the axis (knee, chest - 4 in) prove the space behind free; the climb starts there (`depen=` on the start
  line; counts as unstick evidence). Deeper, or a wall behind: `path_blocked` as before.
- **Narrow tops**: an `edge` refusal (a side probe missed / dropped) retries the sides once at 0.35 r (a crest >= ~11 in
  wide carries the axis; `narrow_top=1`); a 6 in post is still refused.
- `DETECT_MAX_RAYS` 20 -> 24 (`max_rays` range 24..40, default 24). no_face (jumping in the open) and too_high (faces
  above the reach) are legitimate refusals and stay; > 2.0 m and no headroom stay refused (B25 2.3 m box, B15).

### 18.6 Oracles

cargo tests: `cam_gate_close_camera_ok_only_while_first_person_drives`, `aim_shot_fp_eye_accepts_a_close_er_camera`,
`move_collide_body_column_*`, `move_collide_layered_world_*`, `unstick_*` (3), `mantle_detect_narrow_top_retry_run18`,
`mantle_path_depenetrated_start_*`, `config_rays_*`, `config_unstick_*`. movebench `--mantle` B25 (object-layer cart:
the map filter alone walks into it, union stops and mantles onto the bed, 1.3 / 1.75 m crates mantle, 2.3 m refused,
the body probes stop a map-layer bed, prone stops at it) and B26 (stuck in an object with ER pushing: fires in 2.5 s,
holds 1/2/4/8, capped, never on a wall or the cart): 14/14 scenarios. Analyzer rows `camera close`, `ray filters`,
`unstick` (`analyze_r18.py`, 7 mutations; the real run-18 excerpt: camera close FAIL and mantle FAIL as found).

### 18.7 Needs the in-game run

That 0x5b hits assets through cast_ray (and what else it hits: characters? foliage? camera-only volumes), the hit split
map/object/both, ray cost with two filters (PERF ray_us_max), the camera gate against walls/wagons in first person,
mantles onto wagons / crates / rocks with the new filter, the unstick handback resolving a real overlap.
