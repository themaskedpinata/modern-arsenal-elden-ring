# C6 design: the ER character holds the MW2 gun in third person

Status: implemented (passes H1-H3, 2026-10-21); first in-game test 16:14-16:18 (stance worked; gun not in the hand; ER's
sword visible); round-2 test 17:31-17:35 (sword hidden, gun on ER's weapon model but misplaced, churn, gun over the body);
round-3 test 18:49-18:54 (guns line up; no third-person ADS pose; a refresh fail after a nuke death). **Part 14** (ER's
crossbow aim for third-person ADS, the refresh after a death; offline, NOT run in game yet) supersedes section 6's
`tp_ads_er_aim` probe. **Round 6** (after the rounds 4+5 test 20:57-21:25; offline, NOT run in game): 14.8 keeps only ER's
aim BODY animation (our camera, no ER crossbow reticle), 14.9 stops the respawn's refresh fail, 15.8 corrects the barrier
probe (it watched an idle resource and could not say so).
**Part 16** (2026-10-22, built offline 2026-10-22 (16.14), NOT run in game): the user's decision after the round-7 test: a
REAL crossbow as the hidden MW2-mode weapon (ER's own EquipItem, journal + restore), probe 0 first because round 7 never
captured a vanilla crossbow aim (16.1 corrects the `aim arms PASS` misreading); the depth near fit (16.8) and the ignored
chord (16.9). The test toml runs `er_carrier = "dry"` (P-X1) until probe 0 is read.
**Part 13** (grip 3 / pose 3 / depth, offline, NOT run in game yet) supersedes 12.2's `weapon_axes = "auto"` default and
zero grip offsets. **Part 12** (attach 2 / hide / scope peek, offline, NOT run in game yet) supersedes 4.1-4.2 (the gun now
goes on ER's own weapon model), 7 "solo hide" (the marker row is patched into a real hide) and 6 (third-person scoped
ADS now scopes in). Original status: design only (2026-10-21). It answers the request from the five-fix
test (MODLOG 2026-10-21 14:42-14:50): "third person - make the ER character actually hold the gun". Today the self
world gun floats at the fixed `[coop] world_gun_offset_m = [0.45, 0.20, 1.15]` while ER plays its own pose for the
hidden weapon.

Sources: the two C6 research notes, `work\c6\stance.md` (regulation, motion fields, two-handing, aim) and
`work\c6\bones.md` (the bone array, the chr matrix, the read chain; exe 2.7.2.0 read as a file). Earlier designs this
builds on: C3 part 3 (3.3 marker rows, 3.6 held gun and stage B, 3.9 logs), C3 part 4 (pad pulses), M3/M3B (first
person), M4 (AC-130), M7 (nuke). Code as of today: `mw2er\src\coop_vis.rs` (`world_guns_tick`, `hand_anchor`,
`bone_probe`), `mw2er-core\src\coop_vis.rs` (`world_gun_matrix`, `bone_anchor`, `wanted_markers`),
`mw2er\src\game\chars.rs` (`read_palette`, `read_pose`, `MODEL_ITEM_PALETTE`), `mw2er\src\pad.rs` +
`mw2er-core\src\pad_logic.rs` (the `er_melee` / `er_interact` / `er_use_item` pulses).

## Evidence labels

- **[V]** verified: from bytes in eldenring.exe 2.7.2.0, from the vanilla regulation unpack
  (`work\regulation\unpacked`, decoded by `param_rows.py`), from our code, or from logs.
- **[L]** seen in `logs\mw2er.log`.
- **[I]** inferred: nothing has proven it yet. Every [I] the code depends on gets a log line (section 9) so the next
  run confirms it or proves it wrong, and every [I] on the draw path has a fallback.

## Hard rules (apply to every C6 pass)

- Never launch Elden Ring, Steam, me3 or Seamless. Never write into a game folder or `%APPDATA%\EldenRing`. No
  downloads. Do NOT run `check_c3.ps1` / `check_c4.ps1` and do NOT rebuild the friend package in C6 implementation
  passes (the user does that after review).
- Every runtime write to game memory goes through `game::write` (the param patch of section 3 included: the row's
  address comes from a read-only lookup, the bytes go through `game::write`; never `get_equip_param_mut` / `&mut`
  writes). Edits of the caller's buffer inside the XInput / DirectInput detours stay the existing precedent.
- Reads from detours and from the present thread are non-faulting (`game::read`).
- Everything new is behind flags that default OFF. With the defaults, behaviour, logs (apart from one `HOLD cfg` line)
  and the regulation package are byte-for-byte today's.
- First person is never affected: viewmodel, ADS, scope, C5 cut-out, C4.1 injection, AC-130, nuke.
- `cargo test -p mw2er-core --lib` and `cargo build --release -p mw2er` pass with no new warnings; a renderer change
  needs a vmtest case (C6 as designed has none, section 4.6). Dated MODLOG.md section per pass.

---

## 0. Decisions at a glance

| # | Item | Decision | Label |
|---|------|----------|-------|
| 1 | Which ER weapon gets the stance | There is no dedicated hidden weapon: MW2 mode applies marker SpEffect 79001090 (vfx 8060, `isInvisibleWeapon`), which hides whatever the active right slot holds. C6 patches the **motion fields of that row in memory** (option B), it does not add an item (option A, deferred) | [V] fact, choice by design |
| 2 | Stance fields | `wepmotionCategory` 46, `wepmotionOneHandId` 0, `wepmotionBothHandId` 16, `spAtkcategory` 0, `guardmotionCategory` 2 (the values all 12 vanilla crossbows share). `weaponCategory` and `wepType` stay the original weapon's | values [V], effect [I] |
| 3 | Regulation | **No regulation change in C6.** The patch is runtime, hot-toggled by `[coop] er_stance` (default `"off"`). The package sha256 is unchanged. Option A's rows exist only as a documented build-time fallback (`build_regulation.py --stance`, default package unchanged) | by design |
| 4 | Two-handed | ER's own two-hand chord (Y held + RB pulse) through the pad synth, never during an ER prompt, RB never without Y; `arm_style` write only as a probe | chord [I], state [V] |
| 5 | Attachment | `[coop] hold_mode = "offset" \| "hand"` (default `"offset"` = today). Hand: gun = chr matrix x FLVER bone entry x grip x axis map x inches->m, every frame, per player | chain [V], space [I strong] |
| 6 | Hand bone | The C3 probe read one pointer short (`+0x640` is the Mtx43ArrayEntity, matrices at `[arr+0x70]`). Fixed probe + optional name lookup (`R_Weapon`) find the FLVER index; `[coop] hand_bone` keeps a manual override | cause [V][L], index [I] |
| 7 | Grip | Global `grip_offset_m` / `grip_rot_deg` / `hand_axes`, per-gun `[coop.grip.<gun>]` overrides; defaults ~0 (MW2 world model origin is near the grip) | [I], tuned in game |
| 8 | ADS in TP | Default: no ER aim pose (gun stays on the hand). Probe `[coop] tp_ads_er_aim = false` holds ER's aim button while MW2 ADS is held in TP. Optional `tp_ads_hold = "aim"` blends the gun to the camera-aimed offset pose while ADS | [I] |
| 9 | ER melee | While the stance is active, `er_melee` (R3 -> RB pulse) is suppressed: RB on a crossbow-bank weapon is unknown territory. Documented; FP and stance-off keep R3 melee | by design |
| 10 | Co-op | Peers draw our gun on our hand bone (same chain via `p.addr`, same FLVER index). Each PC with `er_stance` on also patches its copy of a peer's active right row when it sees the peer's 79001090, so the peer's body plays the crossbow set there too | [I] |
| 11 | Failure | Any read / validation miss -> the fixed offset (stage A) for that player for that frame, logged rate-limited; stance errors -> restore originals, stance off for the session | by design |

**Order** (each pass offline only, ends with core tests + release build, no package rebuild):

- **PASS H1** = hand attachment: fixed probe, read chain, matrix composition, grip config, fallback (`hold_mode`).
  Testable solo with any weapon (the gun follows the sword pose). No regulation change.
- **PASS H2** = stance: param patch + restore, two-hand chord, `er_melee` suppression, FP/TP transitions, peer patch
  (`er_stance`). Testable solo; the peer part needs both PCs.
- **PASS H3** = ADS handling (`tp_ads_hold`, probe `tp_ads_er_aim`) and the analyzer.

---

## 1. What is verified

- MW2 mode equips nothing. `wanted_markers(markers, mode_on, hide_er_weapon, ...)` puts SpEffect 79001090 on our
  character; its vfx is vanilla SpEffectVfx 8060 with `isInvisibleWeapon` (build_regulation.py `C3_SP_MODE_VFX`) [V].
- The build edits only Bullet, AtkParam_Pc and SpEffectParam (build_regulation.py:93); EquipParamWeapon and
  BehaviorParam_PC in the package are vanilla [V].
- All 12 wepType-55 rows (43000000 Soldier's, 43020000 Light, 43030000 Heavy, 43050000 Pulley, 43060000 Full Moon,
  43080000 Arbalest, 43110000 Crepus's, 43500000 Repeating, 43510000 Spread, 43690000, 43900000, 99050000):
  `weaponCategory` 11, `wepmotionCategory` 46 (Paramdex: Crossbow), `wepmotionOneHandId` 0, `wepmotionBothHandId` 16,
  `guardmotionCategory` 2, `behaviorVariationId` 4300, `arrowBoltEquipId` 47010000, `isDualBlade` 0, `enableGuard` 0,
  `bothHandEquipable` 1. `spAtkcategory` 0 except Pulley 233, Full Moon 236, Repeating 935, Spread 235 [V].
- Row size 664; `weaponCategory` +0x0E6 u8, `wepmotionCategory` +0x0E7 u8, `guardmotionCategory` +0x0E8 u8,
  `spAtkcategory` +0x0EE u16, `wepmotionOneHandId` +0x0F0 u8, `wepmotionBothHandId` +0x0F1 u8, `wepType` +0x1A6 u16 [V].
- EquipParamWeapon lookups round the item id down to 100 (vendored `get_equip_param`: `(id / 100) * 100`), so the
  row is shared by every upgrade level of that weapon + affinity [V].
- `ChrAsm.equipment.arm_style` (0 empty, 1 one-handed, 2 left both, 3 right both), `selected_slots.right_weapon_slot`,
  `equipment_param_ids[22]`, `bolt_loaded_states[12]`; in `PlayerIns.chr_asm` (readable for peers) [V].
- The C3 bone probe's garbage (`n=2`, `n=7`, `0:0.00,0.00,0.00|0.00,-12.22,0.00`) [L] is `read_palette` treating
  `[item+0x640]` as the matrices; it is a `CSFD4LocationMtx43ArrayEntity` (vtable RVA 0x2B70C78): count u32 `+0x68`,
  entries at `[+0x70]`, stride 0x30, entry = 3 x float4 `[R row | T]`, built from the FLVER (count `[flver+0x53C]`,
  `flver = [item+0x68]`), so the index is a c0000 **FLVER** bone index [V].
- `ChrCtrl+0x2F8` = `CSFD4LocationMtx44ChrEntity` (vtable RVA 0x2B70D48), 4x4 at `+0x70`, row-major, translation in
  row 3: the eased matrix the body is drawn with [V].
- The game's model-to-world modifier multiplies an array entry by that chr matrix (0x140B4D230); the pose source is
  `hkaPose::getSyncedPoseModelSpace` [V]. Hence model space [I, strong].
- In MW2 mode `blank_er` hides every ER attack / aim button; ER only receives the pulses `er_melee` (RB from R3),
  `er_use_item` (X), `er_interact` (Y) [V]. The XInput synth only acts when a pad is connected
  (`detour` -> `process` checks `ret`) [V].
- The world gun draw takes a 3x4 column-form matrix (`WorldGunDraw.model`), and stage A's `world_gun_matrix` is a
  reflection (det = -1: MW2 right-handed -> ER left-handed) [V].

## 2. What is inferred (each gets a probe line, section 9)

| # | Inference | Probe / guard |
|---|-----------|---------------|
| I1 | ER resolves the motion bank from `wepmotionCategory` / `wepmotion*HandId` of the active right row, and re-resolves it on a grip or equipment change | `HOLD stance refresh ... arm=a->b` + the user sees the crossbow idle |
| I2 | Keeping `weaponCategory` / `wepType` makes HKS never take the crossbow shoot / aim / reload path, so no bolt is fired or consumed | `HOLD stray_bullet` (section 6) + `er_melee` suppressed |
| I3 | `isInvisibleWeapon` hides both hands' models after two-handing (left weapon goes to the back) | user check; `HOLD stance ... left=<row>` |
| I4 | ER's two-hand chord on the pad is Y held + RB; on keyboard it is E held + LMB | `HOLD chord ... arm=a->b` |
| I5 | The bone entries are model space; hand world = chr matrix x entry | runtime space detection (4.3), `src=local|world` |
| I6 | R_Weapon keeps animating while the weapon is invisible | probe translations move during the R3 attack / walk |
| I7 | A peer animates our character from its own copy of our active row (so it must patch it too); arm_style replicates | 2-PC test; `HOLD peer_stance` lines on both logs |
| I8 | ER's crossbow aim on the pad is LB held while two-handed, and switches to ER's aim camera | `tp_ads_er_aim` probe only |
| I9 | The havok skeleton bone names are at `[[ChrIns+0x398]+0x48]+0x28` as `{char*, bool}` x 0x10, count u32 `+0x38` | name lookup is optional; a miss logs `name=none` and the manual index is used |
| I10 | The chr matrix carries no scale other than ~1 (player model) | column lengths logged in `HOLD bone ok` |

---

## 3. The stance: runtime patch of the active right row (option B)

### 3.1 Why B, not A

A (a dedicated EquipParamWeapon row 79000000 cloned from 43020000, given + equipped by the DLL) needs reverse
engineering of give-item and change-equipment (neither is in the mod or the vendored crate), mutates the save (the
item stays in the inventory; a crash mid-mode leaves a visible crossbow equipped), needs an FMG name and adds
EquipParamWeapon / BehaviorParam_PC to the edited params and round-trip checks. B needs no regulation change, toggles
at runtime and restores exactly. B is tried first; A is only built if H2's in-game test shows ER ignores the patched
bank (I1 refuted).

### 3.2 Fields and values

| Field | Offset | Type | Stance value | Kept |
|-------|--------|------|--------------|------|
| wepmotionCategory | +0x0E7 | u8 | 46 | |
| guardmotionCategory | +0x0E8 | u8 | 2 | |
| spAtkcategory | +0x0EE | u16 | 0 | |
| wepmotionOneHandId | +0x0F0 | u8 | 0 | |
| wepmotionBothHandId | +0x0F1 | u8 | 16 | |
| weaponCategory | +0x0E6 | u8 | | original (never 11) |
| wepType | +0x1A6 | u16 | | original (never 55) |
| behaviorVariationId, attackBase*, arrowBoltEquipId | | | | original |

The values live in `mw2er-core` as a const table (`STANCE_FIELDS: [(off, width, value)]`) with a unit test pinning
them to the vanilla crossbow rows (section 10). Offsets are pinned with `offset_of!` against the vendored
`EQUIP_PARAM_WEAPON_ST` where the crate exposes the fields (else asserted consts, as `MODEL_ITEM_PALETTE`).

Rows that are already a crossbow / bow / ballista (`wepType` 50..56) or a shield in the right hand are not patched
(`HOLD stance skip reason=ranged|shield`): a real ranged row must keep its own logic. Unarmed (110000) is patched like
any other row.

### 3.3 Patch, restore, and the patch set

- Row address: `SoloParamRepository::get::<EquipParamWeapon>((param_id / 100) * 100)` (read-only), pointer -> u64;
  the original bytes of the five fields are read and stored with the row id; the stance bytes go through
  `game::write`. Before every write and every restore the current bytes are re-read: if they are neither the original
  nor the stance bytes, something else changed the row and we leave it alone (`HOLD stance foreign row=..`).
- The **patch set** is computed each tick by a pure function in core:
  `stance_patch_set(own: Option<OwnStance>, peers: &[PeerStance]) -> PatchPlan { patch, restore, skip }`
  - our active right row when the stance is wanted for us (4 conditions: `er_stance = "crossbow"`, MW2 mode on,
    third person, not riding the AC-130 / nuke sequence / in a talk or menu hold);
  - each in-frame peer's active right row when that peer carries 79001090 (co-op, section 7);
  - **skip** a peer row that equals ANY weapon row we have equipped (the six `equipment_param_ids`: left hand and
    inactive slots included) other than our own stanced active right row (we would get the crossbow bank on our own
    weapon, e.g. a left-hand attack / powerstance / two-handing the left weapon, or after switching slots):
    `HOLD peer_stance ... patched=0 reason=shared_row` (review 2026-10-21; first version only checked the active right);
  - our own stance is refused (`HOLD stance skip reason=shared_left`) while any left slot uses the active right row
    (the same weapon in both hands, any upgrade level);
  - a row whose patch soft-fails (missing, unreadable, already the crossbow values) is refused for the session:
    `HOLD stance refused row=.. who=self|<h> reason=row_missing|read|foreign`, logged once, never retried; our stance
    stays off for it (`stance skip reason=refused`);
  - every row in the previous set that is not in the new set is restored.
- Restore points (always, even with `er_stance` turned off live): mode off, FP on, AC-130 on, world leave (the C3
  `strip_markers` path), DLL unload / panic path, `er_stance` switched to `"off"`, and a weapon switch (the old row is
  restored, the new row patched, one refresh).
- The patched row is shared by every character that uses that row [I]: NPC c0000 humanoids with the same weapon and
  the **Mimic Tear** spirit ash (it copies our equipment, so while our stance is on it holds our right weapon in the
  crossbow motion set too; it uses its own AI and never fires bolts: wepType / weaponCategory are unchanged). Low risk,
  logged once per row (`HOLD stance row=.. shared_note`), and gone at restore.

### 3.4 Refresh: the two-hand chord

ER only rebuilds the bank on a grip / equipment change [I1]. The patch is therefore followed by a grip change:

- **Enter**: save `arm_style`; patch; if `arm_style != 3`, send the chord (one-handed -> two-handed); if it was 3
  already, send the chord twice 400 ms apart (3 -> 1 -> 3) so the bank re-resolves.
- **Exit**: restore the fields first, then the chord(s) back to the saved `arm_style`.
- **Chord** (pad synth, new `pad_logic::Chord` state machine): Y synth from t0; RB synth from t0+100 ms to t0+220 ms
  **only while Y is synthesized**; Y released at t0+300 ms. Refused (retried every 0.5 s, `HOLD chord refused
  reason=..`) while ER's prompt is on screen (`er_prompt`, Y would interact), while any other pulse
  (`er_melee` / `er_interact` / `er_use_item`) is active, during a menu / talk / FP-talk hold, mid-air, or while the
  user holds a button that maps to Y or RB. The user's own Y/RB stay blanked as today.
- **No pad connected**: the keyboard equivalent (E held + LMB pulse) through the DirectInput detour's buffer edit
  (`input_hook.rs` precedent) [I4]; if neither route is available, `stance_refresh = "arm_style"` (probe) writes
  `arm_style = 3` through `game::write` and logs whether the anim changed; `"none"` leaves the grip alone.
- Hysteresis: at most one enter/exit transition per `stance_min_switch_s = 1.5` (F6 / FP toggles do not spam chords).
- The resulting `arm_style` is read 0.5 s after each chord and logged; if it did not change, one retry, then
  `HOLD stance refresh fail` and the stance stays patched without a refresh (the user can toggle TP to retry).

### 3.5 No bolts, no ammo, no ER damage

- `weaponCategory` / `wepType` are not crossbow values, so HKS has no reason to take the shoot / reload path [I2].
- While the stance is active **`er_melee` is suppressed** (no RB pulse from R3 in TP stance). RB is ER's only attack
  route in MW2 mode, so no ER attack of any kind reaches the patched weapon. FP and stance-off keep R3 melee.
  INSTALL_GUIDE line: "R3 melee does nothing in third person while `er_stance` is on".
- Guard: our own bullets in the C3 bullet-list read that are outside every mod range (twins, M6, AC-130, nuke marker)
  and owned by us are logged `HOLD stray_bullet row=..` (rate-limited) and, after 2 in 10 s, the stance is
  restored and disabled for the session (`HOLD stance disabled reason=stray_bullet`).
- `bolt_loaded_states` and the bolt slots are never written.

### 3.6 Regulation fallback (only if B fails in game)

`build_regulation.py --stance` (off by default; the default package is byte-identical): EquipParamWeapon 79000000
cloned from 43020000 with `behaviorVariationId` 7900 (no BehaviorParam_PC rows -> no attack [I]), `attackBase*` 0,
`wepType`/`weaponCategory` kept at crossbow (needed for a real item) and `isAutoEquip` 0; EquipParamWeapon and
BehaviorParam_PC added to EDITED and the param_diff round-trip; the DLL side needs give/equip RE. Out of C6 scope
unless H2 refutes I1.

---

## 4. Attachment: the gun on the hand bone

### 4.1 Read chain (per player per frame, all `game::read`)

```
model_ins = [chr + 0x50]                 ChrIns.chr_model_ins (offset_of!)
item      = [model_ins + 0x10]           CSModelIns.model_item (offset_of!)
arr       = [item + 0x640]               MODEL_ITEM_PALETTE (unchanged)
  [arr] == exe_base + OwnAddrs.mtx43_array_vtable   (0x2B70C78 on 2.7.2.0)
n         = u32 [arr + 0x68]             8 <= n <= 1024
data      = [arr + 0x70]                 non-null, 16-aligned
E         = read::<[f32;12]>(data + 0x30 * bone)      bone < n
ctrl      = [chr + 0x58]                 ChrIns.chr_ctrl
m44e      = [ctrl + 0x2F8]
  [m44e] == exe_base + OwnAddrs.mtx44_chr_vtable    (0x2B70D48 on 2.7.2.0)
M         = read::<[f32;16]>(m44e + 0x70)
```

`chars::read_palette` is fixed to take the extra hop (the probe uses it); a new `chars::read_hand(chr, bone) ->
Option<HandRead { e: [f32;12], m: [f32;16], n: u32 }>` reads one entry plus the matrix (112 bytes per player).
The two vtable RVAs go into `mw2er_core::exe_build::OwnAddrs` as `Option<usize>`, `Some` only in `OWN_2720`
(the `filter_vtable` precedent); on 2.7.1.0 hand mode falls back with `reason=exe`. `ChrCtrl+0x2F8` / `+0x70`
become asserted consts derived from a public neighbour of the vendored `ChrCtrl` (as `MODEL_ITEM_PALETTE`).

### 4.2 Finding the bone

1. **Fixed probe** (`[coop] bone_probe`, already on by default, once per session for 10 s after mode on): logs `n`,
   both vtables, and per bone the model-space T plus the three row lengths; it additionally runs while the R3 light
   attack plays (stance off) so the weapon bone is the one that sweeps.
2. **Name lookup** (`[coop] hand_bone_auto = true` once H1 lands; off-by-construction when `hold_mode = "offset"`):
   read the havok skeleton names (I9), find `R_Weapon` (else `R_Hand`), match its model-pose translation
   (`[importer+0x60] + 0x30*k`, hkQsTransform T at +0) against the FLVER entries; the entry within 2 cm is the FLVER
   index. `HOLD bone name=R_Weapon hk=<k> flver=<i> err=<mm>`. Cached per session (same c0000 FLVER on every PC).
3. **Manual** `[coop] hand_bone = <i>` overrides 2 (`-1` = use the lookup; with the lookup off and -1, hand mode
   falls back with `reason=no_bone`).

### 4.3 Matrix math

Conventions: the raw entry E is already column form: `p_model = R_E p + T_E` with `R_E` = the 3x3 of the three float4
rows and `T_E` = their 4th floats. M is row-major with the translation in row 3 (`p' = p M`); its column form is
`C = M^T`, i.e. `R_C[r][c] = M[c][r]`, `T_C = (M[12], M[13], M[14])`.

```
space = local   if |T_E| < 2.5 m  and |T_C - physics pos| < 1.0 m
        world   if |T_E - feet| < 2.5 m and |feet| > 5 m           (re-based array; kept from bone_anchor)
        else    -> fallback reason=space
H (hand -> world, 3x4) = C * E        (local)      or  E   (world)
R_H normalised column-wise (scale logged, I10)
G (gun -> hand)        = T(grip_offset_m) * R_xyz(grip_rot_deg) * A(hand_axes)
W_gun                  = H * G * S(IN_TO_M)                    -> WorldGunDraw.model (3x4, same format as stage A)
```

- `A(hand_axes)` is a signed permutation that maps MW2 model X (forward), Y (left), Z (up) onto hand-local axes,
  written as a string `hand_axes = "+x,+z,+y"` (MW2 X -> +x, ...). Config validation requires `det(A) = -1` (the
  MW2 right-handed -> ER left-handed reflection stage A already uses), so the renderer's culling winding is unchanged.
  At runtime `det(R_H)` is checked to be +1 +- 0.1; a -1 fails with `reason=det` (never an inside-out gun).
- `grip_rot_deg` is applied as X then Y then Z in hand-local axes; `grip_offset_m` is in hand-local metres.
- The default `hand_axes` is a best guess and is tuned from the probe (section 9: `HOLD axes` lines give each hand
  axis in world while ADS / walking; `analyze_c6.py --axes` prints the signed permutation whose X best matches the
  camera forward and Z the world up).
- Pure functions in `mw2er-core::hold`: `entry_to_col`, `chr_to_col`, `hand_world`, `parse_axes`, `grip_matrix`,
  `gun_from_hand`, `validate_hand` (section 5), all unit-tested.

### 4.4 Per-gun grip config

```toml
[coop]
hold_mode = "offset"            # offset (today) | hand
hand_bone = -1                  # FLVER index; -1 = name lookup (hand mode) / stage A (offset mode)
hand_bone_auto = true           # name lookup of R_Weapon (hand mode only)
hand_axes = "+x,+z,+y"          # MW2 X,Y,Z -> hand-local axes; det must be -1
grip_offset_m = [0.0, 0.0, 0.0] # hand-local metres
grip_rot_deg = [0.0, 0.0, 0.0]
hand_smooth = 0.0               # 0 = none; else 1-frame lerp factor 0..0.9 on the anchor
tp_ads_hold = "hand"            # hand | aim (blend to the camera-aimed offset pose while MW2 ADS in TP)
tp_ads_er_aim = false           # probe: hold ER's aim button while MW2 ADS in TP (stance only)
er_stance = "off"               # off | crossbow
stance_refresh = "chord"        # chord | arm_style (probe) | none
stance_min_switch_s = 1.5
stance_peers = true             # patch a peer's row when it carries 79001090 (needs er_stance)

[coop.grip.m4]                  # optional, per gun (arsenal name); missing keys = the global values
offset_m = [0.0, 0.02, -0.03]
rot_deg = [0.0, 0.0, 0.0]
```

Validation: offsets -0.5..0.5 m, angles -180..180, `hand_smooth` 0..0.9, gun names must be arsenal names
(unknown names are an error like other gun tables), `hand_axes` six-token grammar with det -1. Defaults keep
`hold_mode = "offset"` and `er_stance = "off"`, so today's behaviour is unchanged.

### 4.5 What is drawn in hand mode

- **Self (TP)**: unchanged gates (`world_gun_self_tp && mode_on && !fp_on && !ac130_active`). Hand mode replaces the
  anchor + yaw-to-camera + pitch 0 of stage A with W_gun. Bullets still leave from the camera (unchanged).
- **Peers**: unchanged gates (marker / twin gun, not on the AC-130, same havok frame, `world_gun_max_m`). Hand mode
  replaces anchor + last-shot aim with W_gun. Occlusion rays reuse `m_point(&W_gun, ...)`.
- **Mounted** (Torrent): the bone is valid [I]; if it looks wrong, `hold_mounted = "hide"` hides it (logged).
- **FP**: nothing changes (the self world gun is gated by `!fp_on`; peers are drawn from their ChrIns as today).

### 4.6 Renderer

No renderer change: `WorldGunDraw.model` keeps the 3x4 column format and det sign of stage A. If the in-game test
needs a per-draw winding flip, it becomes a renderer change with a vmtest case (`vmtest worldgun_flip`); not
planned.

---

## 5. Failure handling

Per player per frame `validate_hand` returns `Ok(W_gun)` or a reason; any reason draws **the fixed offset (stage A,
today's matrix) for that player for that frame**:

| reason | condition |
|--------|-----------|
| `exe` | exe table has no vtable RVAs (2.7.1.0) |
| `no_bone` | no index (lookup failed, `hand_bone = -1`) |
| `read` | any pointer / read in 4.1 failed |
| `vtable` | either vtable mismatch |
| `range` | `n` outside 8..1024 or bone >= n |
| `nan` | a non-finite float |
| `ortho` | R_E rows not a rotation times a uniform scale: lengths differ by more than 0.05 x mean, mean outside 0.5..2 (body-proportion sliders give a uniform scale != 1, accepted and removed: `scale=` in `HOLD hand ok`), or normalised |ri . rj| >= 0.05 |
| `space` | neither local nor world (4.3) |
| `chr_far` | `|T_C - physics pos| >= 1 m` (mid re-base, wrong matrix) |
| `det` | det(R_H) not ~+1 |
| `jump` | anchor moved > 3 m since this player's last good frame: hold the last good W_gun for 1 frame, then fallback |

Logging (rate-limited, per player): `HOLD fallback who=self|peer=<h> reason=<r>` on the first frame of a new reason,
then `HOLD fallback who=.. reasons=<r>:<k>,<r>:<k> frames=<k>/<total> 30s` every 30 s while any fallback occurs;
`HOLD hand ok who=.. src=local|world bone=<i>` once when a player returns to hand frames. No log per frame.

Stance failures: a write refused by `game::write`, a foreign row, a chord that never changes `arm_style` twice, or a
stray bullet -> restore every patched row, `HOLD stance disabled reason=..` for the session (hand attachment keeps
running).

---

## 6. ADS pose handling

- **Default (`tp_ads_hold = "hand"`, `tp_ads_er_aim = false`)**: MW2 ADS in TP does not change ER's pose; the gun
  stays on the hand (two-handed crossbow idle = held across the chest, muzzle forward [I]). Documented: "no ER aim
  pose in third-person ADS".
- **`tp_ads_hold = "aim"`**: while MW2 ADS is held in TP, the self gun blends in 0.12 s (out 0.12 s) from W_gun to the
  stage-A matrix aimed along the camera (offset `world_gun_offset_m`, camera pitch); pure `blend_mat34` in core
  (orthonormalised lerp of the columns + translation lerp). Peers are unaffected (no ADS signal is synced).
- **Probe `tp_ads_er_aim = true`** (stance active, two-handed, pad connected only): a *held* LB synth (new
  `pad_logic` held-synth, not a pulse) while MW2 ADS is held in TP; released at ADS release, mode off, FP on, or any
  prompt. Logged `HOLD er_aim on|off cam_yaw_delta=<deg> body_yaw_delta=<deg>` so the run shows whether ER's aim
  camera fights the mod's TP camera. If it does, the flag stays false and the doc keeps the "no aim pose" note. It
  must never affect MW2 input: the MW2 ADS state is read before the synth, and ER's aim button never reaches MW2.

---

## 7. Co-op behaviour

- **Gun on the peer's hand**: the same chain with `p.addr`; the bone index is the same on every PC (same c0000
  FLVER); `src=fallback` per peer as in section 5 (far peers update their pose less often: stale but valid; the jump
  guard catches teleports).
- **Peer stance**: with `er_stance = "crossbow"` and `stance_peers = true`, each PC adds the active right row of every
  in-frame peer carrying 79001090 to its patch set (section 3.3). The peer's own chord replicates as a grip change
  [I7], which re-resolves the bank here. Timing: when peers >= 1, our enter chord waits `stance_peer_delay_s = 1.5`
  after 79001090 is on (> one marker read at 4 Hz on the peer), so the peer's copy is patched before our grip change
  arrives. If a peer still shows the sword pose, toggling TP (F6 twice) re-chords.
- **Same row, different state**: a peer row equal to any of our equipped weapon rows (left and inactive slots included)
  other than our own stanced active row is skipped (`shared_row`), so none of our weapons inherits a peer's stance.
- **Solo hide**: the MW2-mode marker (79001090, vfx 8060 `isInvisibleWeapon`) is also applied SOLO (only that marker,
  not the gun / AC-130 / nuke markers) while `hold_mode = "hand"` or `er_stance = "crossbow"` and in third person, so
  ER's own weapon is hidden there too; with the defaults solo applies nothing (as before). In first person solo the
  marker is removed (first person unchanged; up to 0.25 s of ER weapon after F6 to third person).
- **Mixed configs**: a PC with `er_stance = "off"` never patches; it shows a peer's ER pose for its own copy of the
  row (likely the sword set) with the gun still on the hand. The friend package ships the same `mw2er.toml`, so both
  PCs normally agree.
- No new synced marker in C6 (it would change the package); a dedicated "stance on" marker (79001091) is a
  `--stance` package item if needed later.

---

## 8. First person and other modes (must stay unaffected)

- The stance is wanted only in TP; FP on = restore + exit chord (hysteresis 1.5 s). `fp_anim = "hybrid"` and the
  `er_delta` root-motion compensation therefore always see the original bank in FP.
- AC-130 ride, nuke sequence, talk / menu holds, ui_yield and the C4.1 injection: stance is not wanted (restore);
  no chord is sent while they are active. The viewmodel, scope and cut-out code paths are not touched; the hand
  read runs only in `world_guns_tick` and only for drawn players.
- Death / respawn / world leave: `strip_markers` path calls `stance.restore_all()`.

---

## 9. Log lines for the in-game test

| Line | When |
|------|------|
| `HOLD cfg hold_mode=.. hand_bone=.. auto=.. axes=.. grip=[..] rot=[..] smooth=.. tp_ads_hold=.. er_aim=.. er_stance=.. refresh=.. peers=.. vt43=0x.. vt44=0x..` | once at config load / change |
| `COOP bone probe t=.. n=<n> arr=0x.. vt=ok|bad data=0x.. chr_vt=ok|bad M_T=[..] feet=[..]` + `COOP bone probe bones=a-b i:Tx,Ty,Tz|l0,l1,l2 ...` | fixed probe |
| `HOLD bone name=R_Weapon hk=<k> flver=<i> err=<mm>` / `HOLD bone name=none reason=..` | name lookup |
| `HOLD hand ok who=self|peer=<h> src=local|world bone=<i> scale=<s> det=<d>` | first hand frame / after a fallback |
| `HOLD fallback who=.. reason=<r>` / `HOLD fallback who=.. reasons=.. frames=<k>/<n> 30s` | section 5 |
| `HOLD axes hx=[..] hy=[..] hz=[..] cam_fwd=[..] ads=<0|1>` | every 2 s for 20 s after hand mode first succeeds (once per session, only with `bone_probe = true`) |
| `HOLD stance enter row=<id> orig=[cat,guard,spatk,one,both] arm=<a> left=<row>` / `HOLD stance exit row=.. restored=1 arm=<a>` | transitions |
| `HOLD stance skip reason=ranged|shield|exe|off` | not patched |
| `HOLD chord sent route=pad|kb seq=<k> arm=<a>-><b> dt=<ms>` / `HOLD chord refused reason=prompt|pulse|air|hold|menu` | refresh |
| `HOLD stance refresh fail row=.. arm=<a>` / `HOLD stance foreign row=..` / `HOLD stance disabled reason=..` | failures |
| `HOLD melee suppressed n=<k>/30s` | R3 pressed with the stance active |
| `HOLD stray_bullet row=<id> n=<k>` | a non-mod bullet of ours |
| `HOLD peer_stance peer=<h> row=<id> patched=1|0 reason=..` | peer patch set changes |
| `HOLD er_aim on|off cam_yaw_delta=.. body_yaw_delta=..` | probe only |

---

## 10. Offline checks to add

Core tests (`cargo test -p mw2er-core --lib`):

- `c6_hold_`: `entry_to_col` / `chr_to_col` against hand-computed matrices (rotation + translation, row-major vs
  column); `hand_world` local and world cases; `gun_from_hand` with identity hand equals `A * S` (det -1); grip offset
  and rotation order; `parse_axes` accepts the 24 signed permutations with det -1, rejects the 24 with det +1, duplicates, junk; per-gun
  override falls back to globals key by key; `validate_hand` rejects each reason in section 5 (one mutation per
  reason: NaN, 1.2 row length, 0.1 cross dot, 3 m local T, chr 1.5 m off, det -1, 4 m jump -> hold one frame then
  fallback); the fallback matrix equals today's `world_gun_matrix` exactly (offset mode = byte-identical).
- `c6_stance_`: `STANCE_FIELDS` equals the vanilla crossbow values (46/2/0/0/16) and never touches +0x0E6 / +0x1A6;
  `stance_patch_set` (own only; own + peers; peer same row as ours while we are off -> skip; removal -> restore;
  weapon switch -> restore old + patch new); ranged / shield skip; wanted conditions (mode, TP, AC-130, nuke, talk);
  hysteresis.
- `c6_chord_`: the chord never emits RB without Y in any tick at 30/60/144 Hz; refused while prompt / other pulse /
  air / hold; enter from arm 1 = one chord, from arm 3 = two chords 400 ms apart; exit returns to the saved arm.
- `c6_melee_`: `er_melee` synth is zero while the stance is active and unchanged otherwise.
- `c6_ads_`: `blend_mat34` endpoints, orthonormal midpoint, 0.12 s timing; `tp_ads_er_aim` held synth releases on
  every exit edge.
- config: defaults (`hold_mode = "offset"`, `er_stance = "off"`, `tp_ads_er_aim = false`), ranges, unknown gun in
  `[coop.grip.x]` is an error, `hand_axes` det check.
- exe_build: `OWN_2720` vtable RVAs 0x2B70C78 / 0x2B70D48; `OWN_2710` has `None`.

Release build: `cargo build --release -p mw2er` with 0 warnings; `cargo check --workspace --all-targets` clean. No
vmtest change (section 4.6).

Scripts (new, offline, not run by C6 passes against the package):

- `tools\scripts\analyze_c6.py` with `--self-test`: rows `hand` (hand frames >= 95% of self TP frames when
  `hold_mode = "hand"`), `fallback` (reasons tally), `stance` (every enter has an exit with `restored=1` before
  session end / mode off / FP on), `chord` (no `refused` loop > 10 s), `no ER fire` (0 `stray_bullet`, 0 HIT from a
  non-mod row), `melee` (suppressed count), `axes` (`--axes` prints the best signed permutation), `peer` (pair mode,
  `--skew 3600` as analyze_c3).
- `tools\scripts\check_c6.ps1` (for the user's later run, mirrors check_c3 style): core tests filtered `c6_`, release
  build, config defaults grep, `analyze_c6.py --self-test`, and the regulation package sha256 unchanged versus the
  current staged package (C6 makes no regulation change).

---

## 11. In-game test steps (user runs; solo first, then 2 PCs)

Setup: staged DLL from the H1/H2 build, the unchanged package. Edit `mw2er.toml` between steps only.

1. **Probe (defaults + `bone_probe = true`)**: enter MW2 mode in TP, walk 5 s, press R3 twice (light attack). Expect
   `COOP bone probe ... n=<~100-200> vt=ok chr_vt=ok` and `HOLD bone name=R_Weapon ... flver=<i>` (or pick the bone
   that sweeps in the attack). Send the log.
2. **Hand mode, no stance** (`hold_mode = "hand"`, `hand_bone` = the index if the lookup missed): TP, idle / walk /
   run / roll / jump / R3 attack. The gun must follow the right hand (sword pose). Expect `HOLD hand ok who=self
   src=local`, no steady `HOLD fallback`. If the gun points the wrong way, run `analyze_c6.py --axes` on the log and
   set `hand_axes`; tune `grip_offset_m` / `grip_rot_deg` for one gun, then per gun if needed.
3. **FP regression**: F6 to FP: viewmodel, ADS, scope, cut-out, a C4.1 message, AC-130 call and ride, nuke: all as
   before, no `HOLD` line other than fallbacks of peers.
4. **Stance** (`er_stance = "crossbow"`, pad connected): TP, MW2 mode on. Expect `HOLD stance enter`, `HOLD chord
   sent ... arm=1->3`; the character holds a crossbow-style two-handed idle / walk / run with the MW2 gun in the hand.
   Press R3: nothing happens (`HOLD melee suppressed`). Fire every gun class 10 s: no ER bolt, no extra hits (0
   `stray_bullet`). F6 to FP and back: `exit` then `enter`, FP feels as in step 3. Mode off: `exit restored=1`,
   the original weapon and grip are back. Repeat once with the keyboard only (no pad) to see the `route=kb` chord.
5. **ADS** (`tp_ads_hold = "aim"`): TP ADS blends the gun to the aimed pose and back. Optional probe
   `tp_ads_er_aim = true`: note whether ER's aim camera appears or the body turns; send the log.
6. **Two PCs** (both `hold_mode = "hand"`, `er_stance = "crossbow"`): each sees the other's gun in the other's
   hand, crossbow stance on the other's body; `HOLD peer_stance peer=.. patched=1` on both logs; one player switches
   guns and weapons, one goes to FP, one leaves MW2 mode: the other's view follows; `analyze_c6.py --pair` PASS.

Pass criteria for switching defaults (a later decision, not C6): steps 1-6 PASS on both PCs, 0 `stray_bullet`, 0
`stance disabled`, fallbacks < 5% of drawn frames.

---

## 12. Attach 2, the real weapon hide, the third-person scope (2026-10-21, after the first in-game test)

Inputs: the 16:14-16:18 test (MODLOG "C6 first in-game test"), `work\c6\attach2.md`, `work\c6\hide.md`,
`work\c6\scope3p.md`. User: "place the gun where the sword is so he is naturally holding it, not floating"; "ADS with
the sniper in third person should scope in like normal"; first person unchanged.

### 12.1 What the test showed [L] and why

| Seen | Cause | Fix |
|------|-------|-----|
| `HOLD bone name=none reason=bones tries=20`, then `fallback reason=no_bone` every frame: the gun at the fixed offset above the shoulder | hkaSkeleton bones hkArray is `{data +0x30, size +0x38}` (the importer ctor reads the count at +0x38 [V 0x140b4b8e1]); the DLL read +0x28 (the parent-index array's size word) | `SKEL_BONES = 0x30`, count +0x38 only, hkStringPtr bit 0 masked; while / after a failed lookup FLVER **57** (the probe's right hand [L]) instead of no bone |
| The bone probe: bone 0 == the chr matrix translation | ER's chr bone array is WORLD space [L] | `hand_world` tries world first (the first build tried local first: a player within 2.5 m of the tile origin would have been drawn at twice its position) |
| ER's Longsword visible although `COOP marker sent sp=79001090 call_ok=1` | vfx 8060's `isInvisibleWeapon` is not a hide; the only vanilla weapon hide is SpEffect 1466000 Unseen Blade: `stateInfo` 184 "Hide Weapon" + all vow bits; our row (a clone of the inert template row 0) has neither [V decoded params] | 12.3 |
| The marker re-sent exactly every 30 s | re-applying an id ER still lists does not reset its timer: it expired, gap <= 0.25 s | renewal 3 s before the endurance ends (12.3) |
| Third-person ADS with the Intervention: zoom, no scope | the overlay only draws with first person on (`vm_should_draw`); the TP camera looks through the character | 12.4 |

### 12.2 The gun on ER's own weapon model (`[coop] hold_source = "weapon"`, hand mode)

Read chain per player per frame (all `game::read`; vtables RTTI-verified on 2.7.2.0 again 2026-10-21, `OwnAddrs`
`chr_asm_model_vtable` 0x2B37A70, `parts_model_vtable` 0x2B38808, `mtx44_entity_vtable` 0x2B70E68; `None` on 2.7.1.0):

```
asm   = [PlayerIns + 0x648]           [asm] == CSChrAsmModelIns vt, [asm+8] == [PlayerIns+0x50]
parts = [asm + 0x28 + 8*11]           slot 11 = model 0 of the active RIGHT weapon; 0 -> `no_weapon`
                                      [parts] == CSPartsModelIns vt, [parts+0x150] == [PlayerIns+0x50]
root  = [[parts+0x20] + 0x70]         4x4 row-major (chr_to_col), vt CSFD4LocationMtx44Entity
e0    = [[[parts+0x10]+0x640] + 0x70] bone 0 of the weapon FLVER ([R row|T], n = [arr+0x68] in 1..64), vt Mtx43Array
```

`hold::weapon_world` takes the first of e0 / root / root x e0 that is finite, a rotation times a uniform scale, proper
(det +1) and within `WEAPON_NEAR_M` 0.35 m of the hand bone (feet + 2.5 m when the hand did not read). Which matrix ER
refreshes per frame is still [I]: the frame's choice is logged (`HOLD weapon ok who=.. src=e0|root|root_e0 d=..`) and a
pose that stays bit-identical for 3 frames while the hand moved > 2 cm is `weapon_stale` (the hide might freeze the model).

Gun per frame (first that works), every miss logged rate-limited, never per frame:

1. **weapon**: `W_gun = W_weapon * T(grip offset) * R(grip rot) * A(weapon_axes) * S(in->m)`; while the hand bone reads
   too, `G_hand = inv(H) * W_weapon` is learned per weapon row (session).
2. **bone_learned**: `H * G_hand` (the sword's pose rebuilt on the hand bone) with the same grip; the row's own G_hand,
   else the last learned one.
3. **bone**: `H * grip(hand_axes)` (the first build's path, now with a real bone).
4. **offset**: stage A (`HOLD fallback`).

`weapon_axes`: an explicit signed permutation (det -1) or `"auto"` (default): the built-in guess `"+y,+x,+z"` until the
map is learned **while the crossbow stance holds the weapon** (`ErStance::posed`: active, grip 3, no chord in flight;
the first 1.0 s skipped): per frame, MW2 X = the signed weapon axis within 41 deg of the character's facing, MW2 Z = the
remaining one within 60 deg of world up, MW2 Y mirrors; 45 votes with 70 % for one map ->
`HOLD weapon axes auto axes=<map> votes=n/m (set [coop] weapon_axes = "<map>" to keep it)`. The MW2 world model origin
is its grip / tag point, so the per-gun grip offsets default to 0 (`[coop.grip.<gun>]` tunes one gun).

### 12.3 The real weapon hide (`[coop] hide_state_patch = true`)

With `hide_er_weapon` and hand mode or the stance (`CoopCfg::hide_patch_wanted`; the defaults never read the row), the
MW2-mode marker row 79001090 is patched in memory (row address from a read-only `SoloParamRepository::get`, bytes through
`game::write`) into an Unseen Blade-style effect, `mw2er_core::coop_vis::HIDE_FIELDS` (pinned against Paramdex, the
package row and vanilla 1466000 by `check_c6_regulation.py hide`, against the vendored setters by the DLL test
`c6_hide_offsets_match_vendored_layout`):

| Field | Offset | Package | Patch |
|-------|--------|---------|-------|
| motionInterval | +0x00C f32 | 0.0 | -1.0 |
| fall / soul / equipWeight / allItemWeight rates | +0x0DC..+0x0E8 f32 | 0.0 | 1.0 (neutral once the row is live) |
| soulSteal / lifeReduction / hpRecover rates | +0x118..+0x120 f32 | 0.0 | 1.0 |
| stateInfo | +0x156 u16 | 0 | 184 "Hide Weapon" |
| wepParamChange | +0x158 u8 | 0 | 1 |
| vowType0..7 / 8..15 | +0x165 / +0x166 u8 | 0 | 0xFF |

Kept: spCategory 0 (Unseen Blade's 162 would let a grease replace the marker and our re-apply strip it), endurance 30,
net sync on, icon -1, targets, vfx 8060; the other 36 markers stay inert. Bytes that are neither the package's nor the
patch are never written (`HOLD hide foreign`, session); a failed write is put back (`hide refused reason=write`); the
row is restored when no longer wanted (`HOLD hide row restored`). A present marker is removed once after the patch so it
is re-applied against the new row. `check_coop` accepts stateInfo 184 on the mode row only. Each PC patches its own copy
(a peer sees our sword hidden where its DLL runs with the same config). First person: solo the marker is removed (as
before); with a peer the body is hidden anyway.

Renewal (`MarkerCtl::tick_renew`, only with hand mode or the stance): a present marker is removed + applied in the
same tick `marker_endurance_s - 3 s` after ER created its entry (`COOP marker renewed sp=..`): no 30 s blink.

### 12.4 Third-person scoped ADS: the scope peek (`[first_person] tp_scope_peek`, code default false)

While MW2 ADS is held in third person with a gun that has a scope overlay (`fp_logic::peek_wanted`: flag, first person
and viewmodel enabled, overlay gun, no switch, MW2 mode live, no AC-130 / nuke, ADS allowed), the existing first-person
machine runs a real episode: `unwanted = !(wanted || scope_peek)`; the arm is `scope_peek_arm_ticks` (1) only after a
user-chosen third person (last block F6 / none, `fp_logic::arm_ticks_for`), any other block keeps `arm_ticks` (anti-flap).
Everything else is first person as it is: eye camera, body hidden, ADS-in from the eye, the overlay at full ADS (Present
and C4.1 paths), zoom / sway / hold breath / sensitivity, bullets from the eye. Release -> `FP off reason=f6` -> third
person. F6 during a peek keeps first person after the release. `FP on ... peek=1` marks the episode; the FPCAM cfg line
gains ` tp_scope_peek=1 peek_arm=1` only when on.

Third person for: the stance (`StanceIn.fp_on = fp_on && !peeking`: no exit chord per scope-in), the solo hide marker
(kept through a peek), movement's `fp_only` / `tp_anim = "er"` reasons (`MoveIn.fp_peek`). The self world gun follows the
real fp_on (not drawn in the peek). The user's own first person is byte-identical (`wanted` = 1 makes the peek inert).

### 12.5 Config (all hot)

```toml
[coop]
hold_source      = "weapon"   # weapon | bone
weapon_axes      = "auto"     # auto | a det -1 signed permutation ("+y,+x,+z")
hide_state_patch = true       # acts only with hide_er_weapon and (hand mode or the stance)
[first_person]
tp_scope_peek        = false  # code default; the test toml sets true
scope_peek_arm_ticks = 1      # 1..30
```

### 12.6 Log lines (in addition to section 9)

| Line | When |
|------|------|
| `HOLD cfg ... source=weapon weapon_axes=auto hide_patch=1 vtw=0x2b37a70/0x2b38808/0x2b70e68` | config load / change |
| `HOLD bone name=R_Weapon hk=.. flver=..` / `HOLD bone name=none reason=.. tries=20 using flver=57` | name lookup (fixed) |
| `HOLD weapon ok who=self src=e0|root|root_e0 d=<m> row=<id>` / `HOLD weapon miss who=.. reason=no_weapon|weapon_read|weapon_vtable|weapon_bad|weapon_far|weapon_det|weapon_stale row=..` / `... reasons=.. frames=k/n 30s` | weapon model used / not |
| `HOLD src who=.. src=weapon|bone_learned|bone|offset was=..` | the source changes (at most every 2 s) |
| `HOLD hand ok who=.. src=weapon_e0 d=.. bone=57 row=..` (or `src=bone_learned` / `src=bone space=world ...`) | first drawn frame |
| `HOLD weapon axes auto axes=<map> votes=n/m` | the stance taught the map |
| `HOLD weapon probe t=.. posed=.. parts=0x.. vt=asm:ok,parts:ok,ent:ok,arr:ok own=1 n=.. root=[16] e0=[12] used=[..] b57=[12] b58=.. b73=.. b74=..` | `bone_probe`: every 0.5 s for 10 s from the first posed frame |
| `HOLD hide row patched sp=79001090 state=184 ...` / `hide row restored` / `hide foreign` / `hide refused reason=..` | the row patch |
| `COOP marker renewed sp=79001090 remove_ok=1 apply_ok=1` | every ~27 s while the marker is on |
| `FP on ... peek=1` ... `FP off reason=f6` | each third-person scope-in |

### 12.7 Offline checks

Core: `c6_attach_*` (world-first hand array, weapon_world candidates and misses, stale guard + learned grip, axes text +
vote), `c6_hide_fields_are_unseen_blade`, `c6_marker_renewed_before_it_expires`, `c6_scope3p_*` (peek gates, arm ticks,
machine on/off), `c6_attach_hide_peek_cfg`, exe_build pins; DLL `c6_hide_offsets_match_vendored_layout`.
`analyze_c6.py` rows `weapon`, `hide`, `scope3p` (+ the fp / stance rows skip peeks), self-test 17/17.
`check_c6_regulation.py` check 5 `hide` (9/9 mutations). `check_c6.ps1`: 37 core c6 tests, 2 DLL, 19 proof strings.
No renderer change (the gun keeps the 3x4 / det -1 form of stage A): no vmtest case.

### 12.8 In-game test (solo, pad connected; the test toml: hold_mode = "hand", er_stance = "crossbow", tp_scope_peek = true)

1. Load in, F5 (MW2 mode) in third person, stand 3 s, walk 5 s. Expect `HOLD hide row patched sp=79001090 state=184`,
   `COOP marker sent sp=79001090`, the Longsword (and ideally the back weapon) GONE; `HOLD stance enter`, `HOLD chord
   sent ... arm=1->3 took=1`; `HOLD weapon ok who=self src=<e0|root|root_e0>`; after ~2 s of the crossbow idle
   `HOLD weapon axes auto axes=...`; the MW2 gun in the hands where the sword was, barrel forward. Stay 40 s: one
   `COOP marker renewed` and no flash of the sword at 30 s.
2. If the gun points the wrong way after the axes line: send the log (the `HOLD weapon probe` lines give the matrices);
   meanwhile try `weapon_axes = "<the logged map>"` variants. If it sits offset from the hands: `[coop.grip.<gun>]
   offset_m` (weapon-local metres).
3. Hold LT / RMB with the Intervention (also Barrett, WA2000, M21) in third person: the view goes to the eye, ADS-in,
   the scope overlay with sway / hold breath (L3 / Shift); release: back to third person. Expect `FP on ... peek=1` and
   `FP off reason=f6` per scope-in, NO `HOLD stance exit` / `HOLD chord sent` in between. F6 while scoped: stays first
   person after the release.
4. F6 to first person: everything as before (viewmodel, ADS, scope, C4.1, AC-130, nuke); `COOP marker removed` solo.
5. F5 off: `HOLD stance exit ... restored=1`, the sword back (the marker removed); the row stays patched while hand mode
   is configured (`hide_state_patch = false` hot -> `HOLD hide row restored`).
6. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: hand / weapon / hide / stance / fp / scope3p / chord PASS.

## 13. Grip 3, pose 3, depth (2026-10-21, after the round-2 in-game test 17:31-17:35; offline, NOT run in game)

Inputs: the round-2 test (MODLOG "C6 round-2 in-game test"), `work\c6\grip3.md` (+ `grip3\final.json`), `work\c6\pose3.md`,
`work\c6\depth3.md`. User: "the gun is not placed correctly in the hands ... the gun does not cut out behind the player;
first person felt the same". First person stays byte-identical: every change below acts in third person only or behind a
flag that is off by default.

### 13.1 Grip on ER's weapon frame (fixes "stock forward, barrel down/back, below/behind the hand")

- **Axis map is a rig constant** [L 21 probe frames]: `e0 == root^T == R_Weapon (b73) x Rz(180)`; weapon -Y = the facing (muzzle
  +18.8 deg up), +X = the hilt axis, +Z = the character's left. `hold::WEAPON_AXES_RIG = "-y,+z,+x"` is the shipped
  `[coop] weapon_axes` (code default too). The first build's pre-vote guess "+y,+x,+z" (barrel backward / 8-27 deg down, rolled
  90 deg: the screenshot) is gone. `"auto"` stays as a diagnostic: the vote still runs while the stance is posed, but
  `hold::axes_auto_accept` takes only the rig map (`HOLD weapon axes auto axes=.. votes=.. accepted=0|1`); the rig map is
  drawn from the first frame either way, so a one-hand pose or a mid-chord blend can never pick a backwards map.
- **Position**: the MW2 world model's origin is on the handguard; its pistol grip is G ~ (-11, 0, -4.4) in (`hold::GRIP_IN`, 34
  rows from the world-vs-viewmodel tag fit; javelin = the default G [I], at4 / rpg / stinger 1-tag fits [I]). The weapon-frame
  grip is computed, never guessed: `gun = W * T(o) * Rz(rake) * A * S`, `o = -Rz(rake) * A * G * 0.0254`
  (`hold::weapon_grip`), so G lands exactly on ER's weapon origin (the fist) and the bore is ~level along the facing.
  Default gun: offset [0.020, -0.300, 0], rot [0, 0, -18]; the grip3.md per-gun table is reproduced within 1.5 mm
  (core test `c6_grip_table_matches_the_research_and_lands_in_the_fist`).
- **Config**: `[coop] weapon_rake_deg = -18.0` (-45..45) is the one global knob; `[coop.grip.<gun>]` `offset_m` / `rot_deg`
  override the table key by key on the weapon frame (`CoopCfg::weapon_grip_for`; per-gun offsets now -1..1 m for launchers).
  `grip_offset_m` / `grip_rot_deg` stay the HAND-BONE grip (`hand_axes` path), as before (`CoopCfg::grip_for`). The
  `bone_learned` path rebuilds ER's weapon pose, so it uses the weapon-frame grip.
- Lines: `HOLD grip who=self gun=<g> src=table|config|mixed offset=[..] rot=[..] rake=..` once per gun / value change; the
  `HOLD cfg` line gains ` weapon_rake=.. grip_table=34 grip_cfg=<n> exit_defer_s=.. body_occl=.. probe_er_depth=..`.

### 13.2 Pose: a stable two-handed carry

- What the run showed [pose3.md 1-3]: the crossbow bank DID take effect (the hand hangs, the weapon level forward = ER's
  crossbow carry; ER has no shouldered crossbow locomotion [I]); the "one-hand idle" came from churn: every F6 / F5 toggle
  restored the row + 2 chords (3 -> 1 -> 3) and re-patched + 2 chords (~0.82 s of grip-change animation each).
- **Deferred exit chords** (`[coop] stance_exit_defer_s`; review C6 pose: code default 0 = the first build, chords at once,
  first person identical; the in-game test toml opts in with 3.0): an exit caused by first person on
  (`er_stance::exit_deferrable`; MW2 mode off (F5), AC-130, nuke, latches, flag off, world leave and disables chord at once;
  F5 is not deferred because in third person the player would hold ER's own weapon in the crossbow carry) restores
  the five row bytes immediately (first person and mode off see the original row as before), but its chords wait the
  window (`ExitDefer`, `HOLD stance exit_defer s=.. arm=.. row=.. resumable=0|1`). A re-enter inside the window with the grip
  AND the weapon row unchanged (review C6 pose: a weapon switched in first person had its bank resolved unpatched, so another
  row cycles 3 -> 1 -> 3) and the stance settled before the exit (its enter chords done) re-patches the row and sends **no chord**: no
  grip change happened, so ER still holds the crossbow bank it resolved at the enter (`HOLD stance resume row=.. arm=..`).
  A longer first person sends the exit chords when the window runs out (invisible: the body is hidden), but never while
  MW2 fire / ADS is held or the player moved horizontally (> 0.5 m/s) in the last 0.5 s (`HOLD chord refused reason=busy`,
  `er_stance::exit_chord_busy`; only for a ran-out deferral while first person is on). FP difference (opt-in only): ER's
  hidden body keeps the crossbow locomotion bank up to the window longer after F6; the viewmodel, ADS, scope, the
  root-motion compensation constant and the camera are untouched. With the shipped 0 first person is identical.
- **Diagnostics**: `HOLD axes ... arm=<a> lh_d=<m> posed=0|1 rearm=<k>` (lh_d = left hand FLVER 29 to ER's weapon origin:
  ~0.3 m with both hands on the weapon, ~0.5 m for the one-hand carry); the axes window re-arms each time the stance settles
  again (at most 3); `HOLD weapon probe` adds b29 / b45 (the left palm pair). A true shoulder pose needs ER's aim state
  (category 11 / type 55 + held LB): not built (pose3.md 4.5 risks); `tp_ads_hold = "aim"` stays the ADS option. A one-run probe
  with `stance_refresh = "none"` (config only) shows whether the 3 -> 1 -> 3 enter cycle can go.

### 13.3 Depth: the gun behind the player

- Cause [V depth3.md 0]: world guns use our own D32 depth, cleared every frame; ER's depth is never read. `occlusion=1` is the
  C3 ray test (whole gun behind a wall).
- **Chosen design (depth3.md option 3: copy ER's DepthTexture at UiBegin, compare at Present) is NOT built live**: it needs the
  resource's state at the splice point, which only the probe-only ResourceBarrier hook (depth3.md 5 line 3) can measure, and
  that hook is an open decision for the user. Without a known state the C4.1 safety rules forbid the read. What is built:
  1. **Probe P-D, read-only part** (`[coop] probe_er_depth`, cold, default false; `mw2er\src\er_depth.rs`,
     `mw2er_core::er_depth`): boot checks `exe_build::er_depth_sites` (GXRenderingSystem / GXSceneContext x2 /
     GXDepthStencilBuffer RTTI vtables, the slot-7 count function, the `.data` global; the ignored real-exe test
     `c6_depth_addrs_real_exe_2720` pins the instructions). Task thread every 5 s: `DEPTH probe chain rs=.. pins=n/6 ..
     a88_eq=.. res=.. module=d3d12:0|1 .. changes=.. | B def_ds=..` (non-faulting reads, ER's getter never called),
     `DEPTH probe desc A|B fmt=.. WxH .. flags=.. deny_srv=.. rule=ok|format|shape|not_depth|deny_srv|size` (`GetDesc`, a pure
     query, only for a vtable in our back buffer's D3D12 module; review C6 depth, the C4.1 rule: the task only publishes the
     proven vtables and a due flag, UiBegin's detour (ER's render thread, `er_depth::on_ui_begin`) re-reads both chains for
     that frame and calls `GetDesc` there into atomics, the task logs the sample; without the UiBegin hook
     `DEPTH probe desc A|B unmeasured (..)` and no `GetDesc` at all; c41wiring rule 11), at 4 Hz `DEPTH probe wrap t=.. +00:..` on every change of the
     GX wrapper `[obj+8]` (at most 40 lines: a word taking D3D12 state values would be ER's tracked state), and at 60 s
     `DEPTH probe verdict chain=k/n desc=k/n res_changes=.. wrap_state_words=.. state=unmeasured`. Nothing is bound, barriered,
     sampled or written; no hook; no GPU work.
  2. **The visible fix now: body capsules** (depth3.md's own fallback; `[coop] world_gun_body_occlusion`, hot, default
     false): depth-only capsules on every drawn player's body bones (ours in third person, the drawn peers') are drawn into OUR
     world-gun depth right after its clear (`mw2er_render::occluder`, shaders/occluder.hlsl: VS-generated capsules, no pixel
     shader, no render target, GREATER = reversed-Z), then the guns test against it. Bones: `body_occl::BODY_SLOTS` (Pelvis,
     Spine, Spine1, Spine2, Neck, Head, L/R_UpperArm, L_Forearm, L_Hand, L/R_Thigh, L/R_Calf, L/R_Foot [I names]) resolved once
     per session by havok name -> FLVER index (as `hand_bone_auto`; `HOLD body bones found=k/17 flver=..`, the full name list
     once as `HOLD body names ..`); 14 capsules (hips, chest, head, arms except the gun hand's forearm, legs, shoulders and three
     BACK capsules behind the spine that cover a back-carried shield). Radii stay inside the body, and a capsule only hides what
     is behind it, so a gun held in front is never cut. Any miss (bones unresolved, unreadable, torso not upright) adds no
     capsule for that player: its gun draws as before (`HOLD body ok|miss who=.. `). First person: no capsules (Present path
     gated by `!fp_on`; the C4.1 early path is untouched). Loader: `COOP worldgun occluders ready`; first capsule frame:
     `COOP worldgun body occlusion first frame capsules=.. guns=..`.
- Renderer: `Renderer::prepare_occluders` (loader thread), `record_world_occluded(.., caps)`; `record_world` = it with no
  capsules (identical commands). vmtest `--occluders` OC0-OC4 (no capsules identical to record_world; a capsule in front hides
  the gun; one behind changes nothing; half cover keeps the far side bit-identical; no prepared pass ignores capsules; 0
  debug-layer messages, also with `--gbv`).
- Next step for the live ER-depth path (after this run): if `DEPTH probe` is all ok, the user decides on the probe-only
  ResourceBarrier state hook (depth3.md 5 line 3); only a measured compute-readable state (or a GX tracked-state word from the
  wrap dump) allows the copy pass of depth3.md 3, behind `world_gun_er_depth` (not added yet).

### 13.4 Checks

Core c6 tests 48 (+ grip table, auto accept, exit defer, body capsules x2, depth chain / desc, cfg, exe pins); renderer
`c6_occluder_consts_layout`; vmtest `--occluders` 6/6 (WARP and hardware + GBV); `analyze_c6.py` rows `grip`, `churn`, `body`,
`depth probe` (self-test 27/27); `check_c6.ps1` check 10 (vmtest occluders), check 6 keys `weapon_axes = "-y,+z,+x"`,
`world_gun_body_occlusion = false`, `probe_er_depth = false`; `m2_static_checks c3wiring` rule 29 follows
`record_world_occluded`, new rule 35 (depth-only capsules into our cleared depth before the guns, third person, off by default;
5 mutations).

### 13.5 In-game test (solo, pad; test toml: hand mode, stance, tp_scope_peek, world_gun_body_occlusion = true, probe_er_depth = true)

1. Load in, F5 in third person, stand 3 s, walk, run (Striker, then M4, Barrett, RPG). Expect `HOLD grip who=self gun=striker_mp
   src=table offset=[0.000,-0.304,0.000] rot=[0.000,0.000,-18.000]`, the pistol grip in the right fist, barrel level forward,
   top up, from the very first frame (no flipped gun before a vote). `HOLD weapon ok who=self src=e0`.
2. Turn the camera to look at the character from behind and from the side: the gun parts behind the body / back shield are
   hidden, the parts sticking out still show. Expect `HOLD body bones found=../17`, `HOLD body ok who=self capsules=..`,
   `COOP worldgun body occlusion first frame`. If a body part does not hide the gun, send the `HOLD body names` lines.
3. F6 to first person and back within 3 s, twice; F5 off / on within 3 s. Expect `HOLD stance exit_defer ... resumable=1`
   then `HOLD stance resume`, no `HOLD chord sent` around them, no visible grip-change on return. A long first person (> 3 s):
   the chords after 3 s (invisible). First person itself: viewmodel, ADS, scope, AC-130, nuke as before.
4. Stand still 30 s in third person (the depth probe): `DEPTH probe on sites ..`, `DEPTH probe chain ... pins=6/6 ... a88_eq=1
   ... module=d3d12:1`, `DEPTH probe desc A fmt=20 <your resolution> ... rule=ok`, after 60 s `DEPTH probe verdict`. Open the map
   and a menu, rest at a grace, one load screen (the chain may change: `changes=`).
5. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: hand / weapon / grip / hide / stance / churn / body / depth probe PASS.
   Tuning if a gun still sits off: `[coop.grip.<gun>] offset_m = [..]` (weapon-local: +x up the hilt, -y forward, +z left).

## 14. Third-person ADS: ER's crossbow aim pose; the refresh after a death (2026-10-21, after the round-3 test 18:49-18:54; offline, NOT run in game)

Inputs: the round-3 test (MODLOG "C6 round-3 in-game test": "most guns lined up well"; "would like an animation for aim down
sight in third person - currently no change in stance, still the sword-holding stance"; `HOLD stance refresh fail row=-1 arm=1`
at 18:54:21), `work\c6\aim4.md`. Supersedes section 6's probe `tp_ads_er_aim` (it held LB without the type: a Longsword guard
[I]) and 13.2's "no shoulder pose". First person is byte-identical (every change acts in third person behind a flag that is off
by default, or only in the chord refresh around a death).

### 14.1 What ER needs for the aim [I unless marked]

- ER's crossbow aim is two-handed (arm_style 3) + the aim input HELD (pad L1/LB; keyboard RMB = guard on ER's default layout);
  released = back to the carry. The behaviour script picks the aim branch by the weapon TYPE (wepType 55 crossbow), which the
  stance never wrote (it keeps weaponCategory / wepType so ER has no crossbow logic in the carry).
- `ChrIns.chr_flags1c5` bit 1 `precision_shooting` is set by ER every frame while it aims [V vendored]: the free "ER aims now"
  signal (the same byte as the first-person hide's bit 3 and the death flag bit 7; read only here).
- The aim camera is ER's over-the-shoulder precision camera; the body turns to the camera yaw. Our third-person shots already
  use ER's final camera and the camera gate (0.3-15 m, fov 0.2-3 rad, 5 m jump) accepts it [V cam_gate.rs].

### 14.2 The ADS episode (`[coop] tp_ads_pose = "er_aim"`, code default "off"; `mw2er_core::er_stance::AimCtl`)

Wanted (`er_stance::aim_wanted`): the flag (or the old key `tp_ads_er_aim = true`, now an alias; both need `er_stance =
"crossbow"`: `CoopCfg::ads_pose_on`), the stance posed (active, grip 3, no chord in flight, the enter's 3->1->3 done, our row
patched), MW2 ADS held and allowed (not sprinting, the gun not lowered), third person, NOT a scoped gun taken by `tp_scope_peek`,
no AC-130 / nuke / death / menu / latch / prompt / Torrent / mid-air, and a fresh input route (pad, else the keyboard / mouse).

| Step | What | Log |
|------|------|-----|
| ADS edge | write `AIM_FIELDS` on OUR stanced row: weaponCategory +0x0E6 = 11, wepType +0x1A6 = 55 (`game::write`; originals stored on the row's `Patched`, re-read first; already 11/55 = never ours) | `HOLD aim patch row=.. orig=[cat,type] new=[11,55] route=pad|kb when=ads` |
| next tick | hold ER's aim input: pad = the existing LB synth (`pad_logic::ER_AIM_BUTTON`, only with MW2 mode driving, never in the gunship or at a prompt); keyboard = RMB (`shared::KB_AIM`, rgbButtons[1]) written in ER's mouse buffer after the ADS blank (MW2's raw ADS is recorded first), never in the gunship | `HOLD er_aim on cam_yaw=.. body_yaw=..` |
| <= 0.5 s | `precision_shooting` seen -> on | `HOLD aim on t=<ms>ms row=.. route=..` |
| else | release the input, restore when clear; one try per ADS press; 2 failures = off for the session (the stance and the hand pose keep working). Review C6 aim 4: only before the first `aim on` (`AimCtl::ever_on`); after it a failure is ER busy (roll / flask / stagger), never disables, and the same press re-arms `AIM_RETRY_S` = 1.0 s later | `HOLD aim fail reason=no_state n=k/2`, `HOLD aim disabled reason=no_state` |
| release | ADS up / any reason above: input released FIRST, the fields restored once `precision_shooting` stayed clear 0.1 s, at most 0.4 s; a re-press inside that window keeps the fields and holds again | `HOLD er_aim off cam_yaw_delta=.. body_yaw_delta=..`, `HOLD aim off ms=.. why=release|timeout`, `HOLD aim restore row=.. restored=1 orig=.. why=..` |
| hard exits | every stance restore point (F6 / first person, F5, AC-130, nuke, world leave, a weapon switch, flag off, panic, a disable) restores the aim fields BEFORE the motion fields, input released at once | `HOLD aim restore ... why=exit|world_leave|panic|..` |

Guards while the fields are written: no grip chord (refusal `aim`: Y + RB with a crossbow type would fire; review C6 aim 4:
while ANY patched row holds the fields, the stance fallback included); R3 melee stays
suppressed (no RB reaches ER at all); the stray-bullet guard (a non-mod bullet of ours = restore + stance off) is the backstop
against a real bolt; a foreign value at the restore is left alone and disables the stance (`HOLD aim foreign`); a failed write
is put back (`HOLD aim refused reason=write|read|foreign`, aim off for the session). Peers' rows never get the fields (a peer
sees our synced aim animations with its own crossbow motion bank). The row is shared [I]: a Mimic Tear / NPC with the same
weapon sees the crossbow type for the seconds of our ADS (as `shared_note`). While the fields are written our own row reads as a
crossbow, so the stance's skip rule uses the stored type (`ErStance::kept`).

Fallback `[coop] tp_ads_patch = "stance"` (default "ads"): if the run shows only `aim fail reason=no_state` (ER caches the type
at the grip change like the motion bank), the fields are written for the whole posed stance (`HOLD aim patch ... when=stance`)
and restored at its exit; the ADS episodes then only hold the input (`already=1`). Review C6 aim 4: written only once the enter
chords settled (posed, no re-resolve cycle), restored whenever the stance is not posed (a grip change, a weapon switch) so that
no chord ever runs with the crossbow type on the row (Y + RB could fire a bolt). If ER caches the type only at a chord, this
fallback cannot help either and the aim pose stays unavailable.

### 14.3 The gun in the aim, camera, HUD

- ER's weapon frame (e0 = R_Weapon x Rz180) is a rig constant, so the grip table needs no aim variant: the pistol grip stays in
  the fist. Open [I]: whether the -18 deg rake (which cancels the carry's +18.8 deg muzzle-up) also suits the aim pose. So while
  ER's aim is confirmed the gun turns about its pistol grip (`hold::gun_point(m, GRIP_IN)`) onto the camera's aim point (camera
  position + forward x 30 m): `hold::aim_align`, the smallest rotation, blended in / out over 0.12 s (`ads_blend_step`), at most
  20 deg (larger = drawn on ER's weapon frame as is). Lines: `HOLD aim align gun=.. deg=<full angle> rake=..` once per episode (=
  probe P-B: the barrel's angle to the aim line on ER's pose; ~0 = the rake is right, ~18 = the rake is the carry's),
  `HOLD aim align_clamped gun=.. deg=..`. `tp_ads_hold = "aim"` (the stage-A blend) stays out of an aim episode. The left hand
  sits on ER's crossbow fore-end: on SMGs / pistols it may float ahead of the gun (no IK; cosmetic).
- Camera: ER's aim camera runs (a natural third-person ADS); no MW2 ADS FOV in third person (unchanged); MW2 aiming, firing,
  sway, spread and recoil stay ours and camera-based (shots from ER's final camera through the gate, as before). If the aim cam
  fights the look route, the `HOLD er_aim off ... cam_yaw_delta / body_yaw_delta` lines show it. Round 6 (14.8): the user
  wants only the body animation, so `tp_aim_cam = "keep"` (the default of the aim pose) masks ER's aim camera and its
  crossbow reticle; `"er"` brings this paragraph's behaviour back.
- HUD: MW2 fades the crosshair in ADS; in ER's crossbow aim (no scope) a small centre dot is drawn (`HudSnapshot.tp_aim_dot`).

### 14.4 Probe P-A (`[coop] probe_er_aim`, read-only, hot, default false)

While the stance tick runs (needs `er_stance = "crossbow"`; also with MW2 mode OFF, e.g. holding a real Light Crossbow two-handed):
`HOLD aim probe anim=<CSChrTimeActModule.anim_queue[read_idx].anim_id> prec=0|1 arm=<a> row=<id> type=<wepType> cat=<category>
camtype=<CSCamera type> mode=0|1 ads=0|1 aim=0|1 n=k/300` on every change (at most every 0.1 s, 300 lines per session). It gives
ER's aim animation ids, proves bit 1 and the LB / RMB route on this PC, and shows the camera type of the aim cam.

### 14.5 The refresh after a death (the 18:54 `stance refresh fail row=-1 arm=1`)

Cause [L + V, aim4.md 5]: the self-nuke's player sequence (15.43) exited the stance (saved 3, now 3: a 3->1->3 cycle; nuke exits
are not deferrable); seq 5 (3->1) went out before the death (15.95) and took; seq 6 / 7 went to the DEAD player (ER ignores grip
chords there): `took=0` twice -> `refresh fail` -> `disable("refresh_fail")` dropped the saved grip and the stance stayed off for
the session (the respawn was one-handed, no `stance enter` after 18:54:30). `row=-1`: the exit had already taken `own_row`.
Fixes (`er_stance::chord_refusal(.., dead)`, `chord_verdict`, `exit_drops_chords`):
1. `dead` refusal (hp <= 0, death_flag, the death camera: `fp_logic::dead`): `HOLD chord refused reason=dead`, retried, never
   counted; a chord whose flight / check saw the player dead is void: `HOLD chord sent ... took=void`.
2. An exit caused by the death, the nuke's player sequence or a world leave restores the bytes and drops its chords:
   `HOLD stance exit chords=0 row=.. arm=.. why=death|nuke|world (dropped=0|1 ...)`; pending exit chords die with the player too.
   The respawn's world enter does the normal enter.
3. Two failed EXIT chords on a live player give up: `HOLD stance exit_chords gave_up row=.. arm=..` (warning; the stance stays
   enabled). Only two failed ENTER chords on a live, unrefused player still `refresh fail` + disable.
4. Every chord line names its own row and kind: `HOLD chord sent route=.. arm=a->b dt=.. took=1|0|void row=<id> kind=enter|exit`
   (the exit's row is kept for its chords).

### 14.6 Config, checks

```toml
[coop]
tp_ads_pose  = "off"    # off | er_aim (code default off; the in-game test toml sets er_aim)
tp_ads_patch = "ads"    # ads | stance (fallback)
probe_er_aim = false    # the HOLD aim probe lines (read-only)
tp_ads_er_aim = false   # old key: alias of tp_ads_pose = "er_aim"
```

`HOLD cfg` gains ` ads_pose=off|er_aim ads_patch=.. probe_er_aim=0|1`. Core c6 tests 56 (+ `c6_chord_dead_void_and_exit_gave_up`,
`c6_aim_fields_are_the_crossbow_type`, `c6_aim_wanted_conditions`, `c6_aim_episode_write_hold_confirm_release_restore`,
`c6_aim_repress_inside_the_release_keeps_the_fields`, `c6_aim_no_state_fails_twice_then_disabled`,
`c6_aim_align_turns_the_barrel_about_the_grip`, `c6_aim4_cfg`). `analyze_c6.py`: new row `aim pose`; the chord row treats chords
to a dead player (a death span: `NUKE player dead` / `FP blocked reason=death` to the next world leave / enter, +1 s) as N/A,
`reason=dead` waits as by design and `exit_chords gave_up` as WARN (a new non-failing status); self-test 31/31 + the N/A / WARN
checks. `check_c6.ps1`: check 1 >= 56, check 6 + `tests/config_file.rs` expect `tp_ads_pose = "off"` / `probe_er_aim = false` in
a shipped toml, check 9 +11 proof strings. `m2_static_checks c3wiring` rule 36 (off by default and only with the stance; the
fields only through game::write and restored before the motion fields; no chord during an episode; the keyboard RMB after the
raw ADS and never in the gunship; the pad synth's gates; 7 mutations).

### 14.7 In-game test (solo, pad first, then keyboard only; test toml: hand mode, stance, tp_scope_peek, tp_ads_pose = "er_aim", probe_er_aim = true)

1. Load in, F5 in third person with the M4 (then UMP45, AA-12, Striker). Wait for the stance to settle
   (`HOLD stance enter`, the chords `took=1 ... kind=enter`). Hold LT 3 s, release, repeat 3 times, then walk while holding LT.
   Expect per press: `HOLD aim patch row=.. orig=[<cat>,<type>] new=[11,55] route=pad when=ads`, `HOLD er_aim on`, `HOLD aim on
   t=<~100-400>ms`, `HOLD aim align gun=m4_mp deg=<x>`; on release `HOLD er_aim off ... cam_yaw_delta=..`, `HOLD aim off ...
   why=release`, `HOLD aim restore ... restored=1`. On screen: the character shoulders the weapon two-handed, the MW2 gun's barrel
   on the aim line, ER's over-the-shoulder camera, a centre dot; release = back to the carry. Fire while aiming: MW2 shots as
   before, no ER bolt (`HOLD stray_bullet` none), no block / parry animation.
2. If instead `HOLD aim fail reason=no_state` (twice -> `HOLD aim disabled`): set `tp_ads_patch = "stance"` (hot), F6 twice
   (re-enter the stance: `HOLD aim patch ... when=stance`), repeat step 1, send the log.
3. Scoped gun (Intervention / Barrett): LT still scopes into first person (`FP on ... peek=1`), no `HOLD aim patch`.
4. While aiming: press F6 (first person: `HOLD aim restore ... why=exit` before `HOLD stance exit`), open the map / menu
   (`aim off ... why=release`), sprint (release), jump (release), mount Torrent (none). First person ADS / scope unchanged.
5. Keyboard only (pad unplugged / idle): hold RMB in third person: `route=kb`, the same pose.
6. Probe P-A (optional, 1 min): F5 off, equip a real Light Crossbow, two-hand it, hold LB / RMB, walk, release: the `HOLD aim
   probe anim=.. prec=1 ...` lines give ER's own aim ids to compare with step 1's.
7. Death: a self-nuke (or any death) while the stance is on: `HOLD stance exit chords=0 ... why=nuke|death`, no `refresh fail`,
   after the respawn `HOLD stance enter` again (the stance stays on).
8. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: aim pose / chord / stance / no ER fire PASS (chord may WARN only on
   `exit_chords gave_up`).

### 14.8 Round 6: only ER's aim BODY animation (`[coop] tp_aim_cam = "keep" | "er"`; offline, NOT run in game)

Report (rounds 4+5, 21:24-21:25): the aim pose works (8/8 `HOLD aim on`, median 33 ms), but ER also switches to its crossbow
AIM CAMERA (over the shoulder) and draws its crossbow RETICLE; wanted: only the body animation, our normal third-person camera,
no ER reticle. Research `work\c6\aim6.md` (eldenring.exe 2.7.2.0 read as a file):
- Everything keys off `ChrIns+0x1C5 bit 1` (`precision_shooting`), read through a 12-byte leaf getter 0x1403F4A90
  (`movzx eax,[rcx+1C5h]; shr al,1; and al,1; ret`) from 24 call sites [V]. Two of them make the "crossbow view":
  `ChrCam::Update` (0x1403B11D0; its only getter call 0x1403B14E5 drives the camera-type switch `ChrCam+0x7C`: 0 follow -> 1
  aim blend -> 2 aim -> 3 -> 0; the log's `camtype=1 -> 2`, release `3 -> 0`) and the FeMan reticle update (0x1407725C0;
  `CSFeManImp+0x3710 = 2` = the crossbow reticle only for the main player with the bit set; call 0x14077262E) [V].
- The others must keep the bit [I strong]: HKS `env()` (holds the aim state = the animation), the input handler's body turn
  (0x1403DA22E / 0x1403DC5CB), FE / draw. So clearing ER's bit (or TAE 88 "disable precision shooting") would drop the pose.

**Mechanism (`mw2er/src/aim_cam.rs`, `mw2er_core::aim_cam`).** One MinHook on the getter (2.7.2.0 only:
`OwnAddrs::aim_cam`, None on 2.7.1.0 = `HOLD aim cam unavailable reason=build`). The detour is a naked function (no frame, no
call, no store; the hit counters are `lock inc` of our own statics): it returns 0 only when ARMED, the getter's `rcx` is our
main PlayerIns and the return address is `ChrCam::Update`'s call site (0x1403B14EA) or the reticle's (0x140772633); every
other call tail-jumps to the trampoline (ER's real bit). ER's bit is never written. Result: ER's camera stays in type 0 (its
follow camera; our look route and the camera-step / follow-cam detours are unchanged), the reticle takes ER's non-aim branch,
HKS / the body turn still see the bit. Install (task thread, the first tick with `aim_cam_keep()`), non-faulting checks:
the getter's 12 bytes, both functions' prologues, both sites `E8 rel32` onto the getter (`aim_cam::validate`); the hook is
created only after them and the DLL is pinned before `MH_EnableHook` (ER calls the getter from many threads). Any mismatch =
nothing hooked, `HOLD aim cam refused reason=getter|prologue|site|minhook|pin`, ER's camera as round 5.

**Arm (`aim_cam::armed`, every stance tick).** `keep && third person (not first person / AC-130 / nuke) && in_world && !dead &&
our player known && (an aim episode runs (AimCtl::active, the ADS edge to the fields' restore) || ER's bit still set within
1.0 s after the episode (the release grace: no blend into the aim while ER clears the bit))`. Disarmed FIRST at every restore
point: `restore_row` (before the aim fields go back: F5 / F6 / AC-130 / nuke / weapon switch / a disable), `restore_all`
(world leave / panic / the C3 strip path), `disable`, the inert path (flag off), the task's stance panic path, DLL detach. A
hard stop also drops the grace. First person, scope peeks and peers never arm (first person identical); a stale arm can only
mean "ER's camera / reticle not in aim mode for our player" (no write, nothing to corrupt).

**Config.** `[coop] tp_aim_cam = "keep"` (code default; only with `tp_ads_pose = "er_aim"`, which is off by default, so nothing
is hooked by default) | `"er"` = the escape hatch (hot: the next episode uses ER's aim camera / reticle again; once hooked, the
detour stays a passthrough). `CoopCfg::aim_cam_keep()`. `HOLD cfg` + ` aim_cam=keep|er`. The HUD's own centre dot
(`tp_aim_dot`, 14.3) stays the only aim cue (ask the user whether it should go too).

**Lines.** `HOLD aim cam keep installed getter=0x.. sites=2 ret_cam=0x.. ret_ret=0x..` (once), `HOLD aim cam kept ms=..
hits_cam=.. hits_ret=..` (per armed span), `HOLD aim cam er (tp_aim_cam = "er": ..)` (once per switch), `HOLD aim cam
refused reason=..`, `HOLD aim cam unavailable reason=build`. The proof is the existing probe: `HOLD aim probe ... prec=1 ...
camtype=0` inside a kept span.

**Risks [I].** An aim-only behaviour may read the camera type rather than the bit (if the shouldered pose disappears with
"keep", set "er" and send the log); a getter called from 24 places costs 4 compares per call; Arxan may react like to any other
code hook (none so far); the return addresses are 2.7.2.0 constants (validated at install).

**Checks.** Core `c6_aim_cam_sites_validate`, `c6_aim_cam_arm_conditions`, `config::c6_aim_cam_cfg`; `m2_static_checks c3wiring`
rule 38 (off without the aim pose; the detour stores nothing and filters on the arm / our player / the two sites; validate
before MhHook, pin before enable; installed only under keep; first person never arms; every restore disarms first; 9
mutations); `analyze_c6.py` row `aim cam` (PASS: kept spans with hits_cam > 0 and every prec=1 probe camtype=0; FAIL:
refused, no install / span line, camtype 1/2/3 with prec=1 inside a kept span, a >= 0.2 s span with hits_cam=0; WARN:
unavailable; N/A: aim_cam=er); `check_c6.ps1` check 9 + 4 proof strings. Not in check 6 / `tests/config_file.rs`: "keep" is
inert without `tp_ads_pose = "er_aim"` (which they keep at "off").

### 14.9 Round 6: chords while ER spawns the player (the 21:25:30 `stance refresh fail row=2000000 arm=3`)

Cause [L 91913-92006]: the nuke death (21:25:13), exit chords dropped at the death (correct), the respawn's world enter
27.587. The stance entered in the same tick (27.591, `arm=3`: two enter chords) although ER was still spawning the player
(fade 1.0 -> 0.9 at 29.66, `MOVE blocked reason=cutscene`, the camera 194 m away = the camera gate's `dist`, the get-up
animations). The only refusal was `menu` until ~28.1; both chords then read `3->3 took=0` (ER ignores a grip chord there). The
second chord (started 29.417) got its verdict at 30.236, after first person (30.147) had exited the stance (30.203): it still
counted as an enter chord and disabled the stance for the session. The same shape at 21:24:14 (that chord happened to take).

Fixes (core `er_stance`, no flag: they only make the stance's chords wait or not count):
1. `spawn_settling(since_enter_s, fade, cam_far)`: settling for `SPAWN_SETTLE_S` = 2 s after the world enter (NaN = no enter
   yet), and up to `SPAWN_SETTLE_MAX_S` = 12 s while the fade (`fp_logic::fade_max`) is >= `SPAWN_FADE` = 0.05 or the camera
   gate says `dist`. `chord_refusal(.., settling)` gains `spawn` (after `menu` / `dead`): `HOLD chord refused reason=spawn`,
   retried every 0.5 s, never counted (`StanceIn.settling`, task: `self.world_enter_t` + `fp_world.fade` + `cam_why`).
2. `chord_verdict(.., stance_left, ..)`: an ENTER chord checked after the stance went off is `took=void` (never Retry /
   RefreshFail); exit chords unchanged (gave_up, never a disable).
3. No stance enter while settling (`HOLD stance skip reason=spawn`; an active stance is not dropped): a respawn into first
   person no longer flashes the stance in and out.
Checks: core `c6_chord_spawn_refusal_and_stale_enter_void` (the 21:25 replay); c3wiring rule 39 (3 mutations); analyze_c6's
chord row treats `reason=spawn` waits as by design.

### 14.10 Round 6 in-game test (solo; test toml: hand mode, stance, tp_ads_pose = "er_aim", tp_aim_cam = "keep", probe_er_aim = true)

1. Boot, load in, third person (M4, then UMP45). On the first stance tick: `HOLD aim cam keep installed getter=0x.. sites=2`
   and `HOLD cfg ... aim_cam=keep`. If instead `HOLD aim cam refused reason=..` / `unavailable`, send the log (ER's camera as
   round 5).
2. Hold LT / RMB 3 s, release; 3 times; then walk and strafe while aiming. Expect per press `HOLD aim patch`, `HOLD aim on`,
   `HOLD aim probe ... prec=1 ... camtype=0` (never camtype 1 / 2 / 3 while aiming), on release `HOLD aim cam kept ms=..
   hits_cam=N (N > 0) hits_ret=N (N > 0)`, `HOLD aim restore ... restored=1`. On screen: the shouldered crossbow pose, the camera stays our
   normal third-person follow camera (no zoom-in over the shoulder), no ER crossbow reticle (only our small dot), release = the
   carry. Fire while aiming: MW2 shots as before.
3. F6 while aiming: first person exactly as before (`HOLD aim restore ... why=exit`), no ER aim camera underneath; F6 back.
4. Escape hatch: set `tp_aim_cam = "er"` (hot), one ADS: `HOLD aim cam er`, ER's over-the-shoulder camera + crossbow reticle
   return (proves the switch); set it back to "keep".
5. Respawn: die (self-nuke or a fall) with the stance on, respawn: `HOLD chord refused reason=spawn` (for 2+ s after `TASK
   world entered`), `HOLD stance skip reason=spawn`, then `HOLD stance enter` + chords `took=1`; never `HOLD stance refresh
   fail`. Respawning into first person: no `HOLD stance enter` at all during the load-in.
6. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: aim pose / aim cam / chord PASS.

### 14.11 Round 7: the body stays visible, the gun shoulders (offline, NOT run in game)

Report (round 6, 22:42-22:45, `tp_aim_cam = "keep"`): "when ADS in third person, stance does not change and player goes
invisible. gun stays in same position"; addendum: the gun stayed in the low carry. Research `work\c6\aim7.md` (exe file
2.7.2.0 read offline, the 20:57 and 22:42 logs):
- **The invisible body is ER's own aim-time fade, driven by the bit, not by the camera [V].** `PlayerIns+0x5D8`
  (`snipe_mode_draw_alpha_fade_timer`) is run down to 0 by the PlayerIns update while `IsPrecisionShooting` is true (the
  DS3-style "aim = first-person view" hide; its getter call 0x14065B3DB, return 0x14065B3E0) and multiplied into the body's
  draw alpha (vtable slot 153) and the attached parts' visibility. Round 5 faded too: ER's aim camera sat at the shoulder,
  so only ER's reticle and our MW2 gun (drawn by us, never faded by ER) were seen = "a shouldered pose".
- **ER never played an arm aim pose in either run [L].** ER's weapon frame during / after an episode differs only by the
  logged body turn (125.4 deg vs `body_yaw_delta=-125.3`; 115 vs -114.7 with ER's camera), the tilt never changes, the
  animation ids stay the two-handed carry set. `HOLD aim on` only proves ER's bit. Hypotheses: h1 the weapon type cached,
  h2 a bolt needed, h3 more gates than type / category.
- Rejected: letting ER's aim camera run and overriding the final camera (the camera produced no pose; the fade is the bit).

**A. The body stays visible (zero writes, the same hook).** A third masked return address, `RET_FADE` = 0x14065B3E0
(`mw2er_core::aim_cam::AimCamAddrs::fade_*`): the detour returns 0 there too, only armed / our PlayerIns, so ER's timer
runs back to 1 and the body and its parts stay drawn; the input handler's body turn and every other caller keep ER's bit.
Validated at install (`validate`, non-faulting): the 33 bytes at 0x14065B3CF (`cmp byte [0x143B1EFD8],0; je; mov rcx,rdi;
call getter; test; jne; mov rcx,rdi; call 0x1403F4E10; test; je`), the `E8` at 0x14065B3DB onto the getter and the two
timer stores `movss [rdi+5D8h], xmm0` at 0x14065B417 / 0x14065B451. Any mismatch = `HOLD aim cam refused reason=fade_site`
(nothing hooked). With `tp_aim_cam = "keep"` an episode now starts only once the filter is installed
(`aim_cam::keep_ready()` in `AimIn.flag`): refused / unavailable = no aim pose for the session (an invisible body or ER's
aim camera would follow). `+0x5D8` is never written (ER rewrites it every frame); read-only `snipe=` in `HOLD aim probe`
(in quarters in the change key, so a fade shows as lines) and in `HOLD aim pose`.

**B. Camera / crosshair unchanged:** ER's follow camera (ChrCam mask), no ER reticle (FeMan mask), the MW2 four-line
crosshair (2026-10-21 hud.rs).

**C. The gun (the addendum; `hold::ads_hold_aim`).** ER keeps the carry, so the aim align (14.3) clamps (49-50 deg in both
runs, max 20). With `[coop] tp_ads_hold = "aim"` (code default "hand"; the test toml sets "aim") a clamp inside an episode
now shoulders OUR gun along the camera (the stage-A aimed pose, 0.12 s blend, the existing "aim" path) for the rest of the
episode and its release blend: `HOLD aim hold_fallback gun=.. deg=..` once per episode (the `align_clamped` line says
which). If ER's pose ever turns the weapon within 20 deg, the align draws it as before. "hand" = round 6 behaviour.

**D. Probe P-A7 (`probe_er_aim`, read-only).** `HOLD aim pose phase=carry rw_p=.. rw_x=.. rw_y=..` once at ER's bit's rise
(the last carry sample, every 0.25 s while the bit is clear) and `HOLD aim pose phase=aim rw_p=.. rw_x=.. rw_y=.. moved=..
turned=.. arm=0|1 lh_d=.. q=[10 anim ids] bolt=[3 bolt ids] snipe=.. mode=0|1` every 0.25 s while it lasts (ER's weapon
model frame relative to the feet in the body-yaw frame, so the body turn cancels; `arm=1` above 0.10 m or 15 deg,
`aim_cam::pose_moved`); <= 400 lines; also with MW2 mode off.

Lines: `HOLD aim cam keep installed getter=0x.. sites=3 ret_cam=0x.. ret_ret=0x.. ret_fade=0x..`, `HOLD aim cam kept ms=..
hits_cam=.. hits_ret=.. hits_fade=..`, `HOLD aim cam refused reason=getter|prologue|site|fade_site|minhook|pin (... the aim
pose stays off ...)`, `HOLD aim probe ... snipe=..`, `HOLD aim pose ...`, `HOLD aim hold_fallback ...`. No new config key
(check_c6 check 6 / `tests/config_file.rs` unchanged). 2.7.2.0 only; first person / scope peeks never arm.

Checks: core `c6_aim_cam_sites_validate` (+ the fade site: bytes, call, the second condition, the enable byte, both stores,
4 refusals), `c6_aim_pose_probe_body_frame`, `c6_ads_hold_aim_inside_an_episode_without_er_pose`; c3wiring rule 38 (+ the
detour filters `ret_fade`, the fade site validated, no episode with keep before `keep_ready`, `+0x5D8` never written; 4
mutations); analyze_c6 `aim cam` (a >= 0.2 s span with hits_fade=0 FAILs), new rows `aim fade` (every prec=1 `snipe=`
inside a kept span >= 0.5) and `aim arms` (P-A7: PASS when ER moved the arms, WARN when not; never FAIL); check_c6 check 9
+8 proof strings.

### 14.12 Round 7 in-game test (solo; test toml: hand mode, stance, tp_ads_pose = "er_aim", tp_aim_cam = "keep", tp_ads_hold = "aim", probe_er_aim = true)

1. Boot, load in, third person with the M4. On the first stance tick: `HOLD aim cam keep installed getter=0x.. sites=3 ...
   ret_fade=0x..` (if `HOLD aim cam refused reason=fade_site`: no aim episode at all; send the log).
2. Hold LT / RMB 3 s, release; 3 times; then walk while aiming. Expect per press `HOLD aim patch`, `HOLD aim on`, `HOLD aim
   pose phase=carry ...` then `phase=aim ... snipe=1.00 mode=1`, `HOLD aim probe ... prec=1 camtype=0 ... snipe=1.00`,
   `HOLD aim align_clamped ... (the gun shoulders along the camera: tp_ads_hold = aim)` + `HOLD aim hold_fallback gun=m4_mp
   deg=<~50>`; on release `HOLD aim cam kept ms=.. hits_cam=N hits_ret=N hits_fade=N` (all N > 0), `HOLD aim restore ...
   restored=1`. On screen: the body STAYS VISIBLE and turns to the aim, our normal follow camera, no ER crossbow reticle, the
   MW2 four-line crosshair, and the MW2 gun raised along the camera (the shouldered stage-A pose); release = back to the
   carry. Fire while aiming: MW2 shots as before.
3. P-A7 (1-2 min): F5 off (MW2 mode off), equip a real Light Crossbow, two-hand it, WITH bolts equipped: hold LB / RMB 3 s,
   release, 3 times. Then unequip the bolts and repeat. Then F5 on (the stance, step 2 again). The `HOLD aim pose phase=aim
   ... arm=.. bolt=[..] mode=0|1` lines decide (aim7.md 4 C): arms move only with bolts = h2; in vanilla but not in our setup
   = h1 / h3; not even in vanilla = ER has no third-person aim pose for the local player (then the shouldered MW2 gun of
   step 2 is the answer). Note whether ER fades the body in vanilla (`snipe=` < 1 with mode=0 is ER's own behaviour; our
   filter arms only in our own episodes).
4. F6 while aiming: first person exactly as before; F6 back. Escape hatch `tp_aim_cam = "er"` (hot): ER's aim camera +
   reticle + its body fade return (proves the switch); set it back.
5. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: aim pose / aim cam / aim fade PASS, aim arms PASS or WARN.

## 15. World guns depth-tested against ER's scene depth (2026-10-21, design only; nothing built, NOT run in game)

Asked for as "part 14"; part 14 is the aim pose, so this is part 15. User approved (2026-10-21): a read-only hook on
`ID3D12GraphicsCommandList::ResourceBarrier` to learn the state ER leaves its scene depth in, then ER's scene depth hides
world guns (self in third person, co-op peers) behind walls and scenery, together with the body capsules (13.3). Inputs:
part 13.3, `work\c6\depth3.md`, the round-3 probe [L 18:49-18:54]: `DEPTH probe desc A fmt=19` (R32G8X24_TYPELESS, the
D32_FLOAT_S8X24 family) `3840x2160 rule=ok`, 59 chains with 6/6 pins, `wrap_state_words=none`. The wrapper dump found no
GX tracked-state word, so the state can only be learnt from ER's own barriers. The C4.1 safety model applies unchanged.
ER's resources are only read, never bound for writing, never barriered, never written. Each read sits at a queue point where
the state is proven for that frame. `GetDesc` runs only on ER's render thread. Every new path is off by default and goes
probe first. Any surprise trips, for the whole session, back to today's drawing. First person, and every flag off, stay
byte-identical.

### 15.1 The barrier probe P-S (`[coop] probe_er_depth_state`, cold, code default false)

**What is hooked.** The function in slot 26 (`ResourceBarrier`) of the D3D12 graphics command list vtable. The address
comes from OUR Present list's vtable (`ui_probe::CL_VTABLE`). That is the runtime's one implementation for every
direct, compute and copy list of the device. The hook is a MinHook detour, installed by the task thread after boot,
only when `probe_er_depth_state && probe_er_depth && inject_viewmodel` (the UiBegin hook is needed) and the target lies
in the module of our back buffer's vtable. It refuses when the prologue starts with a `jmp` (E9 / FF 25 / EB: someone
else's hook): `DEPTH state hook off why=prologue`. ER's lists are covered only if their vtables point to the same slot-26
function. The ECL fold checks that per list (`vt_eq`, below). It is disabled and removed at teardown (`ON` cleared
first, `MH_DisableHook`, then `MH_RemoveHook` after the INFLIGHT counter reads 0, as with the UiBegin hook). It is never
re-armed in the session.

**Detour (every barrier call of the process, every thread; the hot path).** The detour calls the original FIRST. It then
reads the caller's barrier array, which the caller still owns, so it is valid after the call. It returns at once unless
`ON` is set and `WATCH` (the pinned `res`, an atomic) is non-zero. For each `D3D12_RESOURCE_BARRIER`:
- `Type == TRANSITION && pResource == WATCH`: record (Subresource 0, 1 or `ALL`; StateBefore / StateAfter; Flags
  BEGIN_ONLY / END_ONLY);
- `ALIASING` naming `WATCH` as before or after: count `alias`;
- `UAV` on `WATCH`: count `uav`.
Records go into a fixed lock-free table of 512 entries keyed by the list pointer (a CAS on the key, then atomics; no
lock of any kind, no allocation, no log, no wait, no COM call). Each entry holds: the first StateBefore and the last
StateAfter per plane (0 = depth, 1 = stencil; `ALL` writes both), the transition count, `split` (a BEGIN_ONLY without its
END_ONLY), the recording thread id, and `done` (folded). A barrier into a `done` entry starts a new recording of that list
(ER re-recorded it after executing it), so `Reset` needs no hook. A full table counts `table_full`. That makes the
verdict not ready; it never blocks.

**Fold (hudhook's ECL callback, `ui_probe::ecl_hook`, before the splices are decided; every queue, every thread).** For
each list of the call, in order, the fold looks up its entry. With `WATCH` set it also compares `[list]` slot 26 with the
hooked target (`vt_eq k/n`). An entry not yet `done` is applied to the per-plane queue state `Q[plane]`:
- `Q` unknown: `Q = last_after`.
- `Q` known and the list's first_before != `Q`: count `mismatch` (a barrier we did not see, an enhanced `Barrier()`
  call, another queue, a re-executed list), then `Q = last_after`.
The entry is then marked `done`, and its queue and the submitting thread are recorded (`queues`, `threads`: the distinct
queues and threads that executed a list touching `res`). D3D12 never promotes or decays a depth-stencil texture
implicitly, so the explicit transitions fully determine the state in queue order [V, D3D12 rules, depth3.md 2]. On the
armed UiBegin ECL (the call carrying the armed Scaleform list, the C4.1 rule), the fold stores `Q[0]` "at UiBegin" for
that frame (`STATE_AT_UI`, `STATE_SEQ = seq`) BEFORE the splice decision. That is the state our spliced list would see.
A frame's first 24 transitions go into a fixed per-frame ring (the "path"). The task logs it.

**Matching ER's depth.** `WATCH` = candidate A's `res`. It is published by UiBegin's detour only after that frame's
pinned chain (6/6) and the D3D12-module vtable check passed (13.3). A new `res` resets `Q` to unknown, and it must pass
2 frames in a row (the C4.1 `recreated` rule). Candidate B (DefDepthStencil) is not watched: it is the UI depth, and
UiBegin itself barriers it [V].

**Log lines (task thread, from atomics):**
```
DEPTH state hook on target=0x.. module=<name> slot=26 (read-only: transitions on ER's scene depth counted; original first)   (boot)
DEPTH state hook off why=flag|probe_off|no_ui_begin|no_vtable|module|prologue|minhook                                         (boot)
DEPTH state res=0x.. q=<state|unknown> at_ui={0xC0:n,0xE0:n,0x10:n,unknown:n,other:n} frames=<n> barriers=<n> lists=<n> queues=<n> threads=<n> split=<n> alias=<n> uav=<n> mismatch=<n> unmatched=<n> table_full=<n> vt_eq=<k>/<n>   (5 s)
DEPTH state path seq=<s> [0x10>0xC0 sub=all q=hal t=rt, ..] at_ui=0x..                                                       (once per distinct path, at most 20)
DEPTH state verdict ready=0|1 frames=<n> readable=<k>/<n> write_at_ui=<n> unknown=<n> queues=<n> threads=<n> mismatch=<n> split=<n> alias=<n> vt_eq=<k>/<n> why=<first failing row>   (60 s, then every 60 s)
```
"readable" = `Q[0]` contains NON_PIXEL_SHADER_RESOURCE (0x40) and no write bit (0x4 RT, 0x8 UAV, 0x10 DEPTH_WRITE,
0x400 COPY_DEST, 0x100 STREAM_OUT). Expected [I]: 0xC0 or 0xE0 (DEPTH_READ | PIXEL | NON_PIXEL).
`er_depth_state::verdict` (pure) is ready iff all of these hold:
- frames >= 600 UiBegin frames;
- readable == frames, write_at_ui = 0 and unknown = 0 after the first 2 s;
- queues = 1 (the HAL queue == the present queue) and threads = 1 (ER's render thread);
- mismatch = 0, split = 0, alias = 0, table_full = 0, and vt_eq = all.

**Probe P-S2, the GPU sample (`[coop] probe_er_depth_sample`, cold, code default false; needs P-S).** It runs every 5 s
on a frame whose fold says readable at UiBegin. It uses the LayerSampler pattern:
- the list is recorded in UiBegin's detour, from objects a worker created;
- it is spliced at `at == 0` ahead of the armed list, followed by our fence;
- the read goes through OUR SRV (R32_FLOAT_X8X24_TYPELESS, plane 0) into our readback;
- there is no barrier on `res`.
It reads 16 grid texels, the centre texel and the projected chest-bone pixel of our own character
(`body_occl::BODY_SLOTS` Spine2, from the frame's snapshot):
```
DEPTH probe sample n=<k> conv=reverse|forward|unknown sky=<d> center=<d> zeros=<n>/16 ones=<n>/16 state=0x..
DEPTH probe self n=<k> ok=<a>/<N> err_cm_p50=.. p95=.. near_cam=.. near_fit=.. far_cam=.. lag=0|1
DEPTH probe sample off why=gate|hresult|pso|state (samples=<n>)
```
- `conv`: reverse when the sky texels read 0 and nearer texels read larger values.
- `self`: ER's linearised depth at the chest pixel against the chest's view-space z minus 0.15 m. Pass: ok >= 90 %,
  p95 < 35 cm. This proves the right resource, live contents (no aliasing), the linearisation and the frame match.
- `near_fit` = the median of `z_view * d` (reverse-Z, infinite or far plane); it must agree with CSCam `near_plane`
  (`CameraSample.near`) within 5 %.
- `lag`: the error measured while turning, comparing this frame's camera with the previous one.

### 15.2 The depth-read path chosen: copy at UiBegin, prime our depth at Present

The design is depth3.md option 3, made exact by the fold. Sampling ER's depth at Present is rejected. Present runs on
hudhook's thread, so its ECL can land while frame N+1's 3D work holds the depth in DEPTH_WRITE (depth3.md 0). Splicing
our world-gun draw into ER's frame is also rejected: it moves third-person guns into the UI layer (depth3.md option 1).
1. **Copy (`world_gun_er_depth`, UiBegin detour, ER's render thread).** The per-frame gate `er_depth_state::copy_gate`
   (pure) requires:
   - live (15.3), and the C4.1 thread / queue facts (`tid == hal_tid == rt_tid`, HAL queue == present queue);
   - the 6 pins, `res == WATCH` stable for 2 frames, and a fresh GetDesc that passes `desc_rule` (fmt 19 / 20, 1 mip,
     1 sample, no DENY_SHADER_RESOURCE, == back-buffer size);
   - a third-person world gun due (snapshot: `!fp_on`, world guns non-empty, `in_world`, `!menu_open`,
     `!ac130.active`, no nuke flash);
   - a descriptor and copy slot free (its fence passed), the PSO ready;
   - camera turn < `er_depth_max_turn_deg` since the last frame.
   It then records OUR compute list:
   - our SRV on `res` plane 0, written into our shader-visible heap at every record, in a 3-slot ring that is reused
     only after its fence passed;
   - `CopyDepth` (`Load` -> our R32_FLOAT texture, the size of `res`);
   - our texture `0xC0 -> UAV`, the dispatch, then `UAV -> 0xC0`;
   - nothing on `res`.
   The ECL callback splices it at `at == 0` of the armed call, but only when that call's fold says `Q[0]` readable for
   this seq (else miss `state`), followed by our fence Signal. The `ahead` / `present_in_ui` rules of C4.1 apply. One
   splice per call: the injection's splice first, then this copy, then the P2 sample. With two due, the copy waits a
   frame (miss `busy`).

   **D3D12 validity:**
   - state: the fold proves `res` is readable at that queue point;
   - write-after-read: ER's next writer comes later on the same queue, and its transition orders after our read;
   - visibility: ER's own transition into the readable state made its depth writes visible;
   - lifetime: `res` is pinned in this UiBegin, and ER can release it only after its fence, which is signalled after
     ER's lists that queue behind ours;
   - our objects are worker-created and never released in-session.
2. **Prime (Present, `viewmodel::draw_world_guns`, third person only).** If the copy's seq == this Present's seq (the C4
   3.4 frame pairing) and its splice executed (no `SPLICE_SIGNAL_FAILS` change), `record_world_occluded` gains a
   step: after our depth clear, and BEFORE the capsules and the guns, a full-screen depth-only pass (`DepthPrime`: no
   render target, SV_Depth, depth func ALWAYS on the cleared 0) reads OUR copy. For each pixel it takes the FARTHEST of
   the 2x2 `Gather` (so occluder edges shrink by at most 1 px rather than growing: no false cuts at silhouettes). A sky
   value (0) is discarded. It linearises (`z = n*f / (d*(f-n) + n)`, `f = inf` -> `z = n / d`, with ER's near / far from
   the frame's snapshot), pushes the result back by `er_depth_bias_m + er_depth_bias_rel * z` (code defaults 0.03 m,
   0.004), and writes it in OUR projection (`ndc = A + B / z` from `coop_vis::world_view_proj`'s 0.05 / 3000).
   The capsules and the guns then test GREATER against it, as today. The gun PSOs are unchanged. The compare is planar
   view z against planar view z with the same camera sample, so no radial correction is needed.
3. **Jitter / near / far / lag.** ER's depth is TAA-jittered (VelocityRT, depth3.md 1); our guns are unjittered. The
   error is <= 0.5 px at edges, and the farthest-of-2x2 absorbs it. Near / far come from CSCam (`CameraSample.near/far`,
   added to the HudSnapshot) and are checked live by `near_fit`. A one-frame camera lag (P-S2 `lag=1`) shows as halos
   on fast turns, so frames turning faster than `er_depth_max_turn_deg` (code default 3.0 per frame) skip the prime
   (capsules only) rather than mis-cut.
4. **Contingency (not built):** the fold might show `res` NOT readable at UiBegin while the path has a readable window
   earlier in the frame. The only valid option then is to splice next to ER's own barrier (C4.1 style): before the
   first list whose first transition on `res` has a readable StateBefore, so that ER itself asserts the state. That
   means recording our list inside the ECL callback, which is a new decision for the user. Until then the verdict is
   `not ready why=write_at_ui` and capsules are the answer.

### 15.3 The live gate (like C4.1 `live41`)

`world_gun_er_depth` (cold on, hot off; code default false) goes live, `DEPTH live on res=.. fmt=.. size=.. conv=reverse
near=.. bias=..`, only when all of these hold in this session:
- `DEPTH state verdict ready=1` (P-S on);
- P-S2's last `sample conv=reverse` and `self` pass;
- no `sample off why=hresult`.
A per-build pin (`ErDepthAddrs.state_pin: Option<StatePin>`, holding the state class, conv and the near source) stays
None until a passing run is recorded in MODLOG. With None the probe flags are required, otherwise `DEPTH live off
why=probe_verdict|unpinned|hook|ui_hooks|config`. While live, the hook and the fold keep running: they are the per-frame
proof. A not-ready verdict, a later verdict going not-ready, or any trip falls back automatically.
- Expected misses: `fp`, `nogun`, `state`, `busy`, `recreated`, `size`, `turn`, `slot`, `ahead`. Counted in
  `DEPTH live n=.. copies=.. primed=.. miss=<why:n,..>` every 5 s.
- Trips (off for the session, `DEPTH live trip why=..`, then capsules only): `mismatch` (any fold mismatch after
  live), `queue` (a second queue / thread touches `res`), `split`, `alias`, a record HRESULT, a Signal failure, device
  removed, a fence not passed after 10 s, a desc change on the same `res`, `table_full`, a panic.

### 15.4 Fallbacks and interactions

- The order per frame is: ER-depth prime (when live and paired), then the body capsules (`world_gun_body_occlusion`,
  unchanged, hot), then the guns. Each frame takes the best layer available and falls back to the next: both layers,
  capsules only, then today's C3 drawing (`occlusion=1` ray test unchanged). A frame never draws a gun twice or skips
  one because of this path.
- First person: the copy gate needs `!fp_on`, and so does the prime. Peers' world guns in first person draw exactly as
  today; extending ER depth to first person is not in scope. C4.1's early pass (first person, `notfp` in third person)
  is untouched. The copy is a separate splice in a call the injection left alone, and it never shares a slot or a fence
  with the injection.
- C5 cut-out: unchanged. The cut applies to the gun PSOs as before. The prime writes depth only.
- AC-130 thermal: the thermal pass owns the frame and world guns are not drawn (`!ac130.active`), so no copy, no prime.
  Nuke post pass: unchanged (after the guns).
- HDR / SDR: depth only, so the output encoding is irrelevant.
- Render scale or a resize: `size` / `recreated` misses until the worker re-prepared the copy texture (never created
  on the render or present thread).

### 15.5 Config, renderer, checks (task list in the summary)

```toml
[coop]
probe_er_depth_state  = false  # cold; the read-only ResourceBarrier hook + ECL fold (DEPTH state lines); needs probe_er_depth + [ui] inject_viewmodel
probe_er_depth_sample = false  # cold; P-S2: 16 texels + the chest pixel of ER's depth through our SRV (needs probe_er_depth_state)
world_gun_er_depth    = false  # cold on / hot off; ER's scene depth hides world guns in third person (gated live on the verdict)
er_depth_bias_m       = 0.03   # hot, 0..0.5
er_depth_bias_rel     = 0.004  # hot, 0..0.05
er_depth_max_turn_deg = 3.0    # hot, 0.5..30 (per frame)
```
The test toml turns the three flags on (dated comments). check_c6 check 6 and `tests/config_file.rs` expect all three
flags `= false` in a shipped toml. `HOLD cfg` gains ` er_depth=0|1 depth_state=0|1 depth_sample=0|1 bias=..`.
Renderer: `mw2er_render::er_depth` with `DepthCopier` (compute, LayerSampler-style) and `DepthPrime` (a full-screen
depth-only PSO); `shaders/er_depth.hlsl`. `record_world_occluded` gains an `Option<&Prime>`, and with None it records the
same commands as today.

vmtest `--erdepth`, on WARP with the debug layer, and again with `--gbv`; 0 messages of any severity:
- ED0: an ER-like D32S8 texture is written, then transitioned to 0xC0 by an ER-like list; our copy is spliced ahead of
  an ER-like list whose first barrier is 0xC0 -> DEPTH_WRITE; the copy equals the CPU values.
- ED1: a wall nearer than the gun hides it; a wall behind the gun leaves it bit-identical.
- ED2: no prime gives the same image as `record_world`.
- ED3: the bias edges: +2 cm stays visible, +10 cm is hidden.
- ED4: the prime and the capsules make a union.
- ED5: the real detour on WARP's list vtable and a scripted barrier sequence over 3 lists and 2 ECLs (ALL / plane,
  split, re-record): the fold equals the script.
- ED6: the negative control. The copy with the depth left in DEPTH_WRITE, and GBV must report it.
Core:
- `mw2er_core::er_depth_state`: the table model, `fold`, `readable`, `verdict`, `copy_gate` (every miss / trip), `lin`,
  `prime_ndc`, `turn_deg`; tests `c6_depth_state_*`.
- `analyze_c6.py` rows `depth state`, `depth sample`, `depth self`, `depth live`, plus the verdict line
  `C6 DEPTH VERDICT: READY|NOT READY why=..`, with self-test cases.
- `m2_static_checks` c41wiring rule 12: the detour calls the original first; it has no alloc / log / lock / COM call;
  it is installed only under its flag and removed at teardown; the fold runs in `ecl_hook` before the splices; the copy
  is recorded only in `er_depth::on_ui_begin` (the no-alloc scan) and spliced only at `at == 0`; no `ResourceBarrier` in
  our lists names ER's `res`.
- `m2_static_checks` c3wiring rule 37: the prime only when `!fp_on` and paired, after the clear and before the capsules.
  Both rules get mutations.

### 15.6 In-game test (solo first; test toml: hand mode, stance, body occlusion, probe_er_depth + _state + _sample, world_gun_er_depth = true)

1. Boot: `DEPTH state hook on ... module=D3D12Core.dll`, `DEPTH probe on ... hook=1`. If you see `hook off why=..`,
   send the log; the guns still draw with capsules.
2. Third person, 2 min: stand, walk, turn the camera fast, look at the sky, put your back to a wall, open the map / a
   menu, rest at a grace, one load screen. Expect `DEPTH state ... queues=1 threads=1 mismatch=0 vt_eq=n/n`, `at_ui=`
   only 0xC0 / 0xE0, `DEPTH probe sample conv=reverse`, `DEPTH probe self ok>=90% p95<35`, then `DEPTH state verdict
   ready=1` and `DEPTH live on`.
3. With the gun drawn, stand behind a pillar or wall corner and let the camera swing so that scenery passes between the
   camera and the gun: the gun hides behind scenery, and the parts in front still show. The same with your back to a
   wall and the camera pushed in. `DEPTH live ... primed=..` keeps rising.
4. Fast camera whips: `miss=turn:..`, no visible halo cut into the gun.
5. F6 to first person: everything as before, and `DEPTH live` misses `fp`. AC-130 and the nuke as before.
6. Co-op (2 PCs, after 1-5 are clean): a peer walks behind a rock or house. Their gun hides with them.
7. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: depth state / sample / self / live PASS, verdict READY. A
   NOT READY run (for example `why=write_at_ui`) still plays with capsules; send the `DEPTH state path` lines.

### 15.7 As built (2026-10-21, offline; NOT run in game)

Built behind the three flags (code default false; the in-game test toml turns them on). Flags off and first person: the
commands of before (vmtest ED2; `record_world_occluded(.., None)` records exactly `record_world`'s list).
- **Core** `mw2er_core::er_depth_state`: the lock-free `Table` (512 entries, CAS on the list key, per plane first-before /
  last-after, split / alias / uav / odd-subresource counts, `full`), the `Fold` (Q per plane, the epoch reset, `at_ui(seq)`
  stored at the armed call BEFORE its lists, mismatch / unmatched / queues / threads / vt_eq, the per-frame path ring),
  `verdict` (rows in 15.1 order + `subresource`), `copy_gate` (every miss), `splice_ok`, `prime_pair`, `lin`, `prime_ndc`,
  `turn_deg`, `copy_ref`, `sample_points`, `conv_of`, `SelfTally` (ok = within 20 cm; the better of the infinite / finite far
  model), `live_step`, `trip_of`, `STATE_PIN_2720 = None`. Tests `c6_depth_state_*` (10) + the DLL's pack test.
- **Renderer** `mw2er_render::er_depth` + shaders/er_depth.hlsl: `resource_barrier_detour` (original first, then the
  parse of the caller's array into the static `TABLE`; `catch_unwind`, no alloc / log / lock / COM), `DepthReader` (3 slots,
  each its own allocator, list and copy texture; SRV `R32_FLOAT_X8X24_TYPELESS` plane 0 of ER's depth, our UAV; barriers only
  on our texture `0xC0 -> UAV -> 0xC0` and our sample buffer), `DepthPrime` (full-screen, no render target, depth ALWAYS,
  SV_Depth, one SRV per frame slot), `Renderer::prepare_er_depth` (loader thread) and `record_world_occluded(.., prime)`.
- **DLL** `er_depth_state.rs`: the task installs the hook after the first Present published our list's vtable (`DEPTH state
  hook on target=.. module=.. slot=26`), UiBegin's detour (via `er_depth::on_ui_begin(base, sf, &f)`) publishes the watched
  resource and records the copy / sample, `ui_probe::ecl_hook` runs `fold_ecl` first, then the injection's splice, then
  `er_depth_state::on_ecl` (the copy / sample at index 0 on a readable `at_ui`), then the C4.1 P2 sample; UiEnd cancels an
  unspliced one (miss `busy`); `callback_body` publishes the frame's `want` (`note_want`), `draw_world_guns` takes the
  prime from `present_prime`. `HudSnapshot` gained `er_near` / `er_far` (CSCam); `coop_vis` publishes our Spine2
  (`self_chest`) for the chest pixel.
- **Deviations from 15.1-15.3 (all on the safe side):**
  1. The farthest-of-2x2 is taken by the COPY into a half-resolution texture (the prime reads one texel per 2x2 block):
     the same <= 1 px shrink, a quarter of the memory (3 x 8 MB at 4K) and no Gather in the prime.
  2. Teardown is recording off, then `MH_DisableHook` (passthrough); `MH_RemoveHook` is never called, as with every C4
     hook (review C4: a thread preempted inside the detour returns through the trampoline). The hook is also disabled
     when the UI hooks go passthrough (a hot `inject_viewmodel = false` / a C4 trip): `DEPTH state hook disabled why=ui_hooks`.
  3. The turn rule is applied at Present (miss `turn`, the copy is still made); the copy gate takes the frame's `want`
     from Present (`fp` / `nogun` / a gun, fresh within 250 ms).
  4. The copy needs the typeless format 19 (an SRV of the typed 20 is not allowed): fmt 20 is a miss `format`.
  5. Extra misses / rows: `pair` (no spliced copy of this Present's seq), `noframe` (the armed list not at index 0),
     `pso` (no reader yet); the verdict row `subresource` (a transition on a subresource other than 0 / 1 / ALL).
- **Lines as built:** `DEPTH state hook on|off|disabled ..`, `DEPTH state res=..` (5 s, + `bad_sub=`), `DEPTH state path ..`
  (<= 20 distinct), `DEPTH state verdict ..` (60 s), `DEPTH reader ready device=.. size=.. copy=..`, `DEPTH probe sample n=..
  conv=.. state=.. chest=0|1 d_chest=..`, `DEPTH probe self n=.. ok=a/N err_cm_p50=.. p95=.. near_cam=.. near_fit=.. far_cam=..
  model=inf|finite lag=.. pass=0|1`, `DEPTH probe sample off why=hresult ..`, `DEPTH live on res=.. fmt=.. size=.. conv=reverse
  near=.. far_model=.. bias=.. max_turn=..`, `DEPTH live off why=config|ui_hooks|unpinned|hook|sample|probe_verdict|conv`,
  `DEPTH live first prime slot=..`, `DEPTH live n=.. copies=.. primed=.. miss=.. attempts=.. locked=.. state_at_ui=.. live=..
  tripped=..` (5 s), `DEPTH live trip why=mismatch|queue|split|alias|table_full|state|hresult|desc|panic|self` (round 7 review: `self` = the live candidate's later self check failed with scale, or offset / spread over >= 12 judged),
  `COOP worldgun er depth prime ready`; `HOLD cfg` + ` er_depth= depth_state= depth_sample= bias=..+..z max_turn=..`.
- **Offline proof:** vmtest `--erdepth` ED0-ED5 (+ ED6 with `--gbv`) on WARP with the debug layer, with `--gbv`, and on the
  RTX 5090 + GBV: 0 messages of any severity from our records and their execution; ED6 (the copy of a depth left in
  DEPTH_WRITE) is reported by GBV ("Incompatible texture barrier layout"). ED5 calls the real detour with its trampoline =
  slot 26 of the device's list vtable (the MinHook install itself only runs in game). `analyze_c6.py` rows `depth state`,
  `depth sample`, `depth self`, `depth live` and `C6 DEPTH VERDICT: READY | NOT READY why=..` (self-test 39/39);
  `m2_static_checks c41wiring` rule 12 (11 mutations), `c3wiring` rule 37 (9 mutations, rules 29 / 35 follow the new block);
  `check_c6.ps1` check 11 (vmtest erdepth `--warp --gbv`), check 6 / `tests/config_file.rs` expect the three flags false.

### 15.8 Round 6: the corrected barrier probe P-S v2 (offline; NOT run in game)

Report (rounds 4+5, 20:57-21:25): `DEPTH state verdict ready=0 frames=3476 readable=0 unknown=3476 queues=0 threads=0
vt_eq=0/0`, `barriers=0 lists=0`: the hook (target 0x7ffb502cb280, D3D12Core.dll slot 26) recorded no transition on the
watched resource. Research `work\c6\depth6.md` (D3D12Core.dll 10.0.26100.9549 and eldenring.exe read as files) [V unless
marked]:
- The hook is on the right function: all 20 D3D12Core command-list vtables (stride 0x2b0 from RVA 0x30dd70) share slot 26 =
  RVA 0x23b280, the logged target minus it is 64 KiB aligned; no Agility SDK / proxy in the game folder; eldenring.exe has the
  IIDs of ID3D12GraphicsCommandList and List4 only (no List7 = no enhanced `Barrier`); hudhook never disables it.
- The watched resource is idle [I]: candidate A (`[RS+0x50]+0xA70`, "DepthTexture") is an optional texture created only
  behind a local flag; the scene context's frame buffer is {Scene RT, DefDepthStencil} = candidate B (`S[i]+0xE8`, the
  builder 141a20880 stores it at ctx+0x248). Same format and size, so the desc rule could not tell them apart. B gets ER's own
  barriers every UI frame (UiBegin 140d80a52 `B plane 1 0xC0 -> 0x10`, UiEnd 140d80b94 `0x10 -> 0xC0`, the HAL list): a
  guaranteed positive control.
- Our probe hid it: `CALLS` was counted after the watch gate and never logged; one candidate; the table drops every other
  resource; `why=unknown` covers both a dead hook and an idle resource.

**As built (same hook, read-only, same C4.1 rules, no new flag: under `probe_er_depth_state`).**
1. The detour counts `CALLS` (every call, right after the original, before any gate), `BARS` and `TRANS`; recording (`ON`)
   now starts with the hook (a table without a watched resource records nothing).
2. `er_depth_state::Census` (32 entries, CAS insert keyed by the resource, atomics only): every transition with DEPTH_WRITE /
   DEPTH_READ in its before / after: n, the OR of the states, the last before / after per plane, sub_all, split; a full
   census counts `full`.
3. Both candidates watched: `WATCH` / `TABLE` (A, as before) and `WATCH_B` / `TABLE_B` (B from UiBegin's pinned dsb chain
   `S[0]+0xE8` + the vtable the task proved in the D3D12 module, the same 2-frames-stable rule, its own epoch); one `Fold` each
   (`FOLDS[2]`), folded in the same ECL callback before any splice.
4. The positive control (`Fold::ctl_hits`): an armed UiBegin frame counts when the candidate folded a transition since the
   previous armed frame; for B that is its own UI barrier pair. `control_ok`: >= 60 B frames and >= 90 % hits and calls > 0.
5. `verdict_cand`: `hook` (no call at all, or the control failing), `control` (not judged: B never watched), `idle` (the
   control passes and the candidate never transitioned in >= 600 frames), else the 15.1 rows. Live / the sample read the
   candidate the fold proved readable (`pick_cand`: the live one, else the one readable at the last armed UiBegin, A first);
   the armed record carries the candidate (`CUR` bit 60); `world_gun_er_depth` goes live on the first candidate whose
   verdict + sample + self pass (A first, then B; `live on ... cand=B`), its trips / verdict follow that candidate.
6. No GetDesc for a third resource (it is in no pinned chain this frame: querying it could touch a freed object); the census
   line's `vt_ok` (a non-faulting read of the resource's first qword == a proven D3D12 resource vtable) is all the task says
   about it. If the census shows a third full-resolution resource with 0x10 traffic and B stays at its 2 UI barriers per
   frame, retarget (a new chain: a new decision).

**Lines.** `DEPTH state res=.. ... cand=A|B` (5 s, B once watched), `DEPTH state control B ui=<hits>/<frames> calls=.. bars=..
trans=.. census_full=.. ok=0|1|-` (5 s), `DEPTH census res=.. n=.. states=0x.. p0=a>b p1=a>b sub_all=.. split=.. is=A|B|-
vt_ok=0|1` (top 8, at 10 s and every 60 s; `DEPTH census empty calls=.. trans=..` when none), `DEPTH state verdict ready=..
... lists=.. transitions=.. why=..|hook|idle|control cand=A|B` (60 s, both), `DEPTH state path ... cand=..`, `DEPTH probe
sample ... cand=..`, `DEPTH probe self ... cand=..`, `DEPTH live on ... cand=..`.

**Expected next run [I].** calls ~10^4 / s, `control B ui=n/n ok=1`, A `why=idle` (census n=0 for A), B transitions
between 0x10 and 0xC0 (and its plane 0 readable at UiBegin: inferred, not read); then the sample / self on B
(`cand=B`) and `DEPTH live on ... cand=B`.

**Checks.** Core `c6_depth_state_census_rows_and_full`, `c6_depth_state_control_and_cand_verdict` (incl. the 20:57 replay:
`unknown` -> `idle`); vmtest `--erdepth` ED8 (calls counted with nothing watched; A 3D + B UI pair + an unwatched C on the
real detour: each fold == script, B's control 2/3, the census holds all three; 0 debug-layer / GBV messages); c41wiring rule
12 (+ the call count before the gate, both tables and the census in the detour, the census lock / allocation free, B only
from its pinned chain + the proven vtable; 4 mutations); analyze_c6 rows `depth state` (per candidate) and `depth control`.

### 15.9 Round 6 in-game test (the depth part; test toml as 15.6)

1. Boot / load in: `DEPTH state hook on ... module=D3D12Core.dll`; within 5 s `DEPTH state control B ui=.. calls=<large>`.
2. Third person 2 min as 15.6 step 2. Expect `control B ... ok=1`, the `DEPTH census` lines (B with `is=B`, and every other
   depth resource ER transitions), at 60 s `DEPTH state verdict ... why=idle cand=A` and B's verdict. If B is `ready=1`, the
   sample / self lines carry `cand=B` and `DEPTH live on ... cand=B` follows; then 15.6 steps 3-6.
3. `control ... ok=0` = the hook sees nothing (send the log); a census top row with `is=-`, n far above B's and 0x10 in
   `states` = ER's real scene depth is a third resource (send the census lines).
4. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: depth state / depth control PASS.

### 15.10 Round 7: the self check fits the surface offset (offline, NOT run in game)

Round-6 result (22:42-22:45): state / control / sample PASS on B (0xC0 at UI every frame, conv=reverse), self check FAIL
`ok=18/25 err_cm_p50=19.0 p95=82.8 near_cam=0.0501 near_fit=0.0531` -> live stayed off. Research `work\c6\depth7.md`:
- A steady ~19 cm bias, not noise: the check expected the texel at `z_bone - CHEST_FRONT_M (0.15)` and counted ok under 20
  cm; p50 19.0 sits on the threshold, so ok flipped per sample. near_fit +6 % is the same fact (6 % of ~3.2 m = 19 cm).
- From one camera distance (d_chest +-4 %) "the surface is 0.34 m in front of the bone" and "near is 6 % off" fit equally;
  most likely the offset [I]: the third-person camera sees the character's BACK (armour, cape, ER's stowed back weapon),
  not the chest front the 0.15 m guess was named for.
- p95 82.8 cm = 2-3 real misses in 25 (a texel on something nearer / off the torso edge; "still" filtered only the turn).
- Not the cause: the half-res copy (the sample reads ER's full-res plane 0), the far model (finite_proven never at 3 m),
  lag (lag=0).

**Fix (core `er_depth_state`; never trusts ER's depth nearer than proven):**
1. Fitted offset: per still sample `o = z_bone - lin(d)`; pass needs the median `o` in [0, 0.50] m (the surface in front of
   the bone; behind = a scale / model error), >= 90 % of the judged within 0.10 m of it, the p95 residual < 25 cm, >= 6
   judged (`self_why`: few | edge | offset | spread | p95 | scale).
2. Scale kept separate: only when the inliers' bone z spans >= 0.6 m, the Theil-Sen slope of z_bone against lin(d) must
   be 1 within 5 % (`scale=proven`, else `bad` = FAIL). Without the span `scale=unproven`: live may go on, but the prime's
   relative bias is raised to `margin_rel = median(z_bone / lin(d)) - 1` (the bone assumed ON the surface; ~0.11 here,
   ~37 cm at 3.4 m): it can only let the gun show through a nearby wall, never hide a gun in front of one
   (`bias_rel_eff`; `MARGIN[cand]`, never lowered while live; `DEPTH live on ... margin_rel=..`).
3. A sturdier chest sample (GPU): 5 texels, the chest pixel + a +-8 px cross (at 4K, scaled by the width:
   `cross_px`), their median; a sample whose 5 spread by > 0.15 m (or one is the sky) is an `edge` (not judged), more than
   25 % edges of the still samples FAIL (`why=edge`); "still" also needs the camera to move < 5 cm per task tick
   (`MOVE` from `publish_chest`'s camera position, snapshotted with the turn at the sample arm). `SAMPLE_N` 18 -> 22, the
   reader's root constants 40 -> 48 DWORDs (`READER_CONSTS`), the shader's `pts[11]`.
4. Diagnosability: `DEPTH probe self_s k=.. z=.. d=.. zl=.. o=.. spread=.. edge=.. turn=.. mv=..` for the first 32 still
   samples; `DEPTH probe self n=.. ok=a/J edges=e/S o_cm=.. err_cm_p50=.. p95=.. span_m=.. scale=.. slope=.. margin_rel=..
   near_cam=.. far_cam=.. model=.. lag=.. why=.. pass=..`; `DEPTH probe sample ... d_med=..`.
5. Cosmetic: `DEPTH live ... state_at_ui=-` until live picked a candidate (it read A's fold before).

Checks: core `c6_depth_state_sample_conv_and_self_check` (the cross, the median / spread, proven scale, a wrong near =
offset, moving = not still, the self_s line), `c6_depth_state_self_fit_offset_scale_edges` (a: the 22:42 shape passes with
scale unproven and the margin; b: 6 % over a 1.5 m span = scale; c: a negative offset; d: 3 edge outliers judged right,
the same as near hits = spread; e: > 25 % edges), the finite / infinite tests kept; vmtest `--erdepth` ED0 (22 texels ==
CPU, the cross median == CPU) + ED0e (a cross over a wall edge = an edge); c41wiring rule 12 (+ the sample arm atomics-only
with the move snapshot, `sample_points` / `cross_px` in the render-thread scan; 3 mutations); analyze_c6 `depth self`
(why= / o / span / scale / margin, the self_s offsets; an older line judged as before).

### 15.11 Round 7 in-game test (the depth part; test toml as 15.6)

1. As 15.9 steps 1-2. Stand still 30 s in third person in the open, then 30 s with the camera pulled in against a wall /
   slope (ER moves the camera closer: the span that proves the scale), then walk.
2. Expect `DEPTH probe self_s k=1..` lines with `o=` about the same value per sample (0.2-0.4 = the offset), `edge=1` on
   a few, and `DEPTH probe self ... why=none pass=1` with `scale=unproven margin_rel=~0.1` (or `scale=proven` after the
   wall part), then `DEPTH live on ... margin_rel=..`, `DEPTH live n=.. primed=>0`. A gun behind a wall / pillar is hidden;
   in front of it stays visible.
3. `why=offset` (o < 0 or > 0.5) or `why=scale` = the linearisation is wrong: send the log (the self_s lines say which).
4. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: depth self PASS, depth live PASS.

## 16. The real crossbow carrier, the depth near fix, the ignored chord (2026-10-22, design only; nothing built, NOT run in game)

Asked as "part 16 (real crossbow) + the depth / chord fixes" after the round-7 test 23:39-23:43 (user: "the gun raised but
no character animation"). USER DECISION (MODLOG 2026-10-21 round-7 test): in MW2 mode the hidden right-hand ER weapon is a
REAL crossbow (hidden by the Unseen-Blade hide row as the sword is now, the MW2 gun on its weapon model), so ER plays its
genuine crossbow carry and aim; the original weapon comes back when MW2 mode ends, at first person, and after death /
reload / quit / crash; ER never fires or consumes a bolt. Research: `work\c6\xbow8.md` (+ raw dumps `work\c6\xbow8\`),
`work\c6\depth8.md`; the exe 2.7.2.0 read as a file, the game not run. Supersedes for the carrier: 3 (option A deferred ->
now built in a different form, see 16.2), 3.4 (the 3->1->3 cycle), 14.2's aim type patch, 14.11 D's reading; 15.10's
`margin_rel` (by 16.8).

### 16.1 Correction of the round-7 reading first [L]

The MODLOG round-7 entry and analyze_c6's `aim arms PASS ... mode=1: 33/65 arm=1` are a **false positive**:
- all 300 `HOLD aim probe` lines of the block have `row=2000000` (the longsword); no `row=43xxxxxx` anywhere in the log =
  a real crossbow was never the active right weapon;
- all 87 `HOLD aim pose phase=aim` lines have `mode=1` (MW2 mode on = the patched sword); none after `MODE off` 23:42:16.5:
  the vanilla P-A7 step was not captured;
- `arm=1` came from walking (`1602xxxx`) and rolls / backsteps (`5100`, `5200`); standing aims (`q=[16000000,..]`) gave
  arm=0 31 / 35 times, the 4 exceptions are the first sample after a roll or turn;
- `bolt=[52000000,-1,-1]` is the save's bolt slot, equipped all session with the sword.

So it is **still unproven that vanilla ER raises the arms in a third-person crossbow aim for the local player**. The
exe gives the reason the patched row never could [V, xbow8 2.4]: HKS learns the weapon through ER's weapon-changed
notify, raised by the change tracker 0x1404D1D60 only when the right / left param ids or the two-hand grip change; the
ADS-time 11/55 type patch changes neither. A genuine equip raises it with real crossbow values. That is the argument for
the carrier, and probe 0 (16.6) must pass before the mod touches the inventory.

### 16.2 Decision

| # | Item | Decision | Label |
|---|------|----------|-------|
| X1 | How to equip | ER's own `EquipGameData::EquipItem` 0x140249160 on the MAIN player's EquipGameData (`[[GameDataMan 0x143D64FB8]+8]+0x2B0`), with the inventory menu's arguments `(egd, slot, &handle, inv_index, 1, 1, 0)`, called from the mod's `ChrIns_PostPhysics` task (game thread; precedent ac130.rs `apply_speffect`), never from Present / a detour | [V] path, effect [I] |
| X2 | Never | ChrAsm / EquipGameData byte writes for the swap, `ChrAsm::SetSlot` 0x1403BF3D0 (skips the inventory index list, the PlayerIns copy and the notify) | by design |
| X3 | Which crossbow | the first OWNED, not-otherwise-equipped instance whose base row is in `CARRIER_ROWS` = 43020000 Light (preferred), 43000000, 43030000, 43080000, 43110000 (wepType 55, spAtkcategory 0, all wepmotion 46 / both-hand 16). Never Pulley / Full Moon / Repeating / Spread (own spAtk banks) | [V] rows |
| X4 | None owned | `[coop] carrier_give = true` (code default false): ER's own starting-gear recipe in the MOD save only: `Find` 0x14024C560 -> `CSGaitemImp::Create` 0x1406735C0 (item id `0 << 28 | 43020000`, weapon type 0) -> `AddItem` 0x140246480 (`+0x3FC` result checked) -> EquipItem. A vanilla Light Crossbow, never a mod-only row (peers without the friend package would resolve a missing EquipParamWeapon row). Given items stay in the mod save's inventory (documented, harmless). Off = `HOLD xbow skip reason=none_owned` -> the stance (part 3) as the fallback | [V] recipe, [I] use |
| X5 | Regulation / package | **No change.** No new row, no FMG; the default package sha256 unchanged; the friend package policy unchanged (it gains nothing; the carrier is a vanilla item) | by design |
| X6 | Two-handing | EquipItem leaves arm_style 1 [V 0x1403BF3D0]. First the never-run `stance_refresh = "arm_style"` write (arm_style 3 on PlayerIns ChrAsm through `game::write`): the tracker watches the grip, so it should raise the same notify as ER's own toggle with no button at all [I, probe P-X2]. Fallback: the pad chord (Y + RB) only while the slot's `bolt_loaded_states` is 0 (EquipItem clears it [V 0x1403BF47A], so a lone RB can only reload); the keyboard chord (E + LMB) under the same rule | [I] |
| X7 | Bolts | Unchanged slots unless probe 0 says bolts are required AND both bolt slots are empty (then `carrier_give` adds Bolt 52000000 x1 and equips Bolt1, restored at exit). A shot is the only consumer [V]; see 16.4 | [I] until probe 0 |
| X8 | Stance / aim patches | Not used while the carrier is on: a real crossbow row is skipped as ranged (`stance skip reason=ranged`), `aim_write` already returns `foreign` on 11/55. `tp_ads_pose = "er_aim"` (the LB / RMB synth, the aim cam / fade filter) is kept unchanged | [V] code |
| X9 | Hide | Unchanged (marker 79001090, Unseen Blade stateInfo 184). Whether it also hides the loaded-bolt model (ChrAsm `right_bolt_slot`, its own parts slot): probe line | [I] |
| X10 | Grip | The crossbow's weapon-model (e0) frame differs from the sword's: grips keyed by carrier (16.7) | [I] |
| X11 | Fallback | The carrier refused / failed for the session = today's stance (er_stance) path, unchanged | by design |
| X12 | Versions | 2.7.2.0 only (`CarrierAddrs` pinned; prologues in 16.3); 2.7.1.0 = `HOLD xbow off why=version` | [V] |

### 16.3 Mechanism (`mw2er_core::carrier` pure plan + DLL `carrier.rs` on the task thread)

Addresses (2.7.2.0) and the install-time pin (non-faulting read of the first bytes; any mismatch = `HOLD xbow off
why=prologue fn=..` for the session):

| VA | Ours | Signature | First bytes |
|---|---|---|---|
| 0x140249160 | EquipItem | `(egd, i32 slot, *const u32 handle, i32 inv_index, bool, bool, bool)` | `40 53 55 56 57 41 56 48 81 EC 90 00 00 00` |
| 0x140246480 | AddItem (give only) | `(egd, *const u32 handle, i32 qty, bool, bool) -> i32`; `+0x3FC` 0 ok / 2 unique dup / 4 full | `48 8B C4 55 41 54 41 55 41 56 41 57 48 8D 68 A9` |
| 0x1406735C0 | CSGaitemImp::Create (give only) | `(imp [0x143D70920], *mut u32 out, *const u32 id)` | `48 89 54 24 10 53 48 83 EC 30` |
| 0x14024C560 | EquipInventoryData::Find | `(egd+0x158, *const u32 id) -> i32 / -1` | `40 56 48 83 EC 20 83 CE FF 83 3A FF` |

Per task tick, `carrier::plan(&CarrierIn) -> CarrierStep { Hold | Swap{..} | Grip | Restore{..} | Refuse(why) }` (pure,
tested) from a non-faulting snapshot:
1. **Gate (swap):** `er_carrier = "crossbow"`, MW2 mode on, third person (a scope peek counts as third person), alive, in
   world, `spawn_settling` over, not menu / talk / ladder / mount / mid-air, ER idle (the `er_busy` gate of 16.9), no ER
   prompt, not AC-130 / nuke, 1.5 s since the last swap or restore (hysteresis), no pending journal for another character.
2. **Read:** active right slot `s = 2 * sel + 1` (1 / 3 / 5), its handle / param id / inventory index (`egd+8+4s`),
   arm_style, the bolt slots 7 / 9 and quantities. Original already wepType 50..56 = `skip reason=ranged` (nothing to do: the
   real ranged weapon carries; hide + grip as below).
3. **Pick** (X3 / X4). Bolts (X7).
4. **Journal** (16.5) written and flushed BEFORE the call.
5. **Call** `EquipItem(egd, s, &h, idx, 1, 1, 0)`; within 1 s re-read: PlayerIns ChrAsm param id[s] == the carrier, the
   parts model of the slot changed (`HOLD weapon ok ... row=430200xx`): `HOLD xbow on`. Else restore + `refused
   reason=verify` for the session.
6. **Grip:** X6 (`HOLD xbow grip via=arm_style|chord arm=1->3 ok=0|1`); two failures = carry one-handed (the gun still on
   the weapon model) + `HOLD xbow grip fail`.

ER's PlayerIns update then copies EquipGameData -> PlayerIns+0x638 under its own lock, raises the weapon-changed notify,
drops ER's aim bit (`SetPrecisionShooting(chr,0)`, so no swap inside an aim episode: the gate refuses while `stance_aim_on`)
[V xbow8 2.3].

### 16.4 Never a bolt (the guards, in order)

1. RB / R2 / LMB attack never reach ER in MW2 mode (`blank_er`, unchanged).
2. `er_melee` (R3 -> RB pulse) suppressed while the carrier is on (as for the stance; RB is the crossbow shot).
3. The chord's RB / LMB only while `bolt_loaded_states[s] == 0` (a stray press = the reload animation, never a shot); the
   preferred grip path (arm_style) presses nothing.
4. `HOLD stray_bullet` guard kept.
5. New tripwire: the equipped bolt quantity is read every tick while the carrier is on; any decrease = `HOLD xbow trip
   why=bolt_used qty=a->b`, restore at once, carrier off for the session.
6. `tp_ads_pose = "er_aim"` holds only LB / RMB (aim), never fire.

### 16.5 Restore and the crash story

**Journal** `<root>\state\carrier.json` (the mod's own folder via `paths.rs`; never the game folder or
`%APPDATA%\EldenRing`; no valid root = `refused reason=no_root`), atomic (write tmp, flush, rename): version, character
name + the mod save slot, slot `s`, original full item id + gaitem handle + inventory index, arm_style, bolt slots, the
carrier id / handle / index, items given, timestamp.

**Restore** = `EquipItem(egd, s, &orig_h, orig_idx, 1, 1, 0)` (re-`Find` the index if the handle no longer matches it),
then the saved grip (arm_style write, else the chord back), bolts back if we changed them; verified like 16.3 step 5,
then the journal is deleted. Points: MW2 mode off, first-person entry (not a scope peek), AC-130 / nuke PREP, world leave,
`er_carrier` set to off, the DLL detach (best effort: only if the game thread is alive; else the journal stays), and at
the next world enter while a journal exists (crash, quit to title, Alt-F4, death during loading).

**Rules:** no ER call while dead, loading or with menus open (it waits; the respawn's world enter restores or re-swaps);
if slot `s` no longer holds our carrier (the user changed gear meanwhile), `HOLD xbow foreign`, change nothing, drop the
journal; if the original item is no longer in the inventory, `restore_missing`, leave the carrier, keep the journal and
toast once. ER's autosave can store the swapped state at any moment; the journal is what makes that harmless: the mod
save is the only save involved (never the user's real save), and the next world enter fixes it.

**Review 2026-10-22 (journal v2):** the journal names its owner by save flavour (`solo` = ER0000.mw2er.sl2, `coop` =
Seamless's ER0000.mw2co2, from `ersc.dll` being loaded [I]), `GameMan.save_slot` and the character name; a journal is
applied (restored, or dropped as done / foreign) only when all three match the loaded character, else it is kept and no
swap runs (`xbow journal other owner=.. me=..`, the stance keeps the patched sword). A world leave carries nothing in
memory: each world enter re-reads the file. While this character's journal waits for its restore, or after
`restore_missing`, the crossbow counts as equipped (no R3 melee, no chord RB on a loaded crossbow, the bolt tripwire).
The grip before the swap comes back after every verified restore the stance's own exit did not cover (`stance grip_back`;
a re-swap before the grip is back records the journal's grip, not 1). A stance disabled for the session disables the
carrier at once (no new swap). With the carrier on, the round-7 `hold_fallback` never shoulders the gun along the camera.

### 16.6 Probes (probe-first: each step needs the previous one's log)

- **Probe 0 (no writes; existing `probe_er_aim`):** MW2 mode OFF, the user equips a Light Crossbow in the menu and
  two-hands it; stand still, hold LB / RMB 3 s, 3 times (a) with bolts, (b) without. Needs `HOLD aim probe ... row=430200xx
  type=55 cat=11 prec=1` and `HOLD aim pose phase=aim ... arm=1 ... mode=0` with an idle / aim anim head on >= 2 samples in
  a row. **If vanilla does not move the arms either, stop: the carrier buys only the carry, and the user decides before
  the inventory is touched.**
- **P-X1 `er_carrier = "dry"`:** everything of 16.3 except the call: `HOLD xbow dry slot=.. orig=.. idx=.. pick=..
  idx=.. bolts=.. journal=<path>` + the prologue pins. No game write.
- **P-X2 grip:** after the first real swap, the arm_style write: `HOLD xbow grip via=arm_style ... ok=` (the grip-switch
  anims 29060 / 29070 in the queue within 0.3 s, or the parts / anim change).
- **P-X3 hide:** `HOLD xbow parts right=.. bolt_model=.. hidden=0|1` once per swap (the bolt model's parts slot read).
- **P-X4 peers (2 PCs):** the peer's `read_arm` of our PlayerIns shows 43020000 and `HOLD peer ... row=430200xx`.

### 16.7 Co-op, grips, first person

- **Co-op:** peers get our real crossbow through ER's own player sync (a vanilla menu swap is how co-op shows a new weapon
  [I, P-X4]); the peer draws our MW2 gun on that weapon model through the same read path; `stance_peers` skips the row
  (ranged); nothing to patch on their side. A peer without the mod sees a visible Light Crossbow (vanilla item).
- **Grips:** `[coop.grip.<gun>]` gains carrier-keyed tables `[coop.grip.<gun>.xbow]` and `[coop] weapon_axes_xbow` (code
  default = the sword's map until P-X5 derives it); the sword table stays. P-X5 (under `probe_er_aim`): `HOLD xbow frame
  e0=<3x4> hand=<3x4> rel=<axes,offset>` once per swap while carrying, from which the axes / grip are derived offline (the
  existing `weapon_axes = "auto"` only ever accepts the rig map, so it cannot learn this frame).
- **First person / C4.1:** restore at FP entry (before the first FP frame is drawn the swap waits; the FP viewmodel path is
  not touched), swap again in third person after the hysteresis. Scope peeks do not restore.

### 16.8 (D) The depth self check: the near is ~10 % short, the fit was wrong

From the 17 `self_s` samples (depth8.md): `z_bone * d` is flat across 2.0-4.5 m, median 0.0553 = 1.104 x near_cam (0.0501);
the 0.34 m "offset" of 15.10 is ~10 % of 3.4 m, not an offset; the round-7 `slope=0.925` is the offset filter throwing away
the two short-range samples plus 2-4 cm noise over 0.10 m pair gaps (reproduced: 0.919). The 23:41:39 `scale=proven 1.026`
was the same noise; live then ran 45 s with near 0.0501 (ER surfaces ~10 % too near: up to ~35 cm phantom occlusion). The
trip was right. Fix (core `er_depth_state`, behind the existing `probe_er_depth_sample` / `world_gun_er_depth`):
1. **Two-parameter fit:** Theil-Sen slope `s` of z_bone against lin(d, near_cam, inf) over ALL judged still samples with
   pairs >= 0.30 m apart (`SCALE_PAIR_M`), `o = median(z - s * zl)`; residuals about the line; no offset pre-filter.
2. **Leverage:** `proven` only with >= 3 judged samples <= median - 0.3 m AND >= 3 >= median + 0.3 m, and the 25-75 % pair
   slopes within +-3 %. One far sample never proves a scale.
3. **near_eff** in the PrimeTicket instead of `snap.er_near`, rounded only to the safe side: unproven = `near_cam * max(1,
   median(z_bone * d) / near_cam)` (the bone-on-surface bound, 0.0553 here: can only under-occlude); proven = `near_cam *
   max(1, s_p75)`, `o` added to the bias only when negative. Gate `s` in [0.95, 1.25], `o` in [-0.10, 0.50] m, else `why=scale
   |offset`. Replaces `margin_rel`.
4. **Trip** only when the slope's low end beats the live `near_eff / near_cam` by > 5 %.
5. **Optional cause probe** `[coop] probe_er_depth_floor` (cold, code default false): 4 floor texels 1-4 m ahead of the
   character in one frame, fit `z = a / d + b` (a = the true near, b != 0 = an eye offset); SAMPLE_N 22 -> 26, root
   constants + 8 DWORDs, vmtest ED0 updated; plus one line with near / fov / pos of `pers_cam_1..4` and `chr_cam`.
Lines: `DEPTH probe self ... scale=proven|unproven|bad s=.. s_iqr=.. o_cm=.. n_eff=.. lev=a/b ...`, `DEPTH live on ...
near=<n_eff>`, `DEPTH probe floor a=.. b=.. res_cm=.. n=..`, `DEPTH cams p1=near/fov/pos .. chr=..`. With this run's samples:
unproven (lev 2 / 12), live allowed with near 0.0553, no phantom wall.

### 16.9 (E) The ignored chord (`HOLD stance refresh fail row=2000000 arm=3`)

[L] every successful chord shows ER's grip-switch anims (29040 / 29050 to one hand, 29060 / 29070 to two) within ~0.3 s;
every failure (18:54, 21:25:28 / 30, 23:42:22 / 24) shows none: ER ignored the input (23:42: a jump-family anim 10202040
since 23:42:19.1 after Space; 21:25: the respawn get-up). The gate (`refuse_now`) never looks at ER's action state, and two
ignored chords disable the stance for the session. Fix (pad_logic / er_stance, under `er_stance` / `er_carrier`):
1. `refused reason=er_busy` (not counted): chord only while the anim head is idle / locomotion (`id % 1_000_000` in the
   logged idle / 20xxx / 22xxx families) and unchanged for >= 0.3 s.
2. Verdict per chord: `played` (a 2904x-2907x id in the window), `ignored` (none: retry with backoff 0.5 / 1 / 2 / 4 s, cap 6
   in 20 s, not counted), `fail` (played, grip read wrong: counts toward refresh_fail). Line `HOLD chord verdict=..
   head=.. arm=a->b`.
3. With the carrier the 3 -> 1 -> 3 cycle is gone (one 1 -> 3 after the equip, or the arm_style write).

### 16.10 Flags (all code default off / unchanged; the test toml sets them with a dated comment)

```toml
[coop]
er_carrier           = "off"   # off | dry | crossbow; acts at the next third-person stance enter, off restores
carrier_give         = false   # allow giving a vanilla Light Crossbow (and 1 bolt if probe 0 needs it) into the MOD save
weapon_axes_xbow     = "-y,+z,+x"  # the carrier's weapon-frame map (P-X5 derives it)
probe_er_depth_floor = false   # cold; the floor-fit near probe (16.8 step 5)
```
check_c6 check 6 and `tests/config_file.rs` safe defaults: `er_carrier = "off"`, `carrier_give = false`,
`probe_er_depth_floor = false`. `HOLD cfg` gains ` carrier=off|dry|crossbow give=0|1`. Test toml for the next run:
`er_carrier = "dry"` first, `"crossbow"` only after probe 0 + P-X1 are read; `carrier_give` stays false unless the user
owns no crossbow and agrees.

### 16.11 Log lines

```
HOLD xbow off why=flag|version|prologue fn=..|no_root
HOLD xbow dry slot=.. orig=<id> idx=.. arm=.. pick=<id> idx=..|none bolts=[..] journal=<path>
HOLD xbow skip reason=ranged|none_owned|gate:<why>
HOLD xbow give id=.. idx=.. result=<+0x3FC>
HOLD xbow on slot=.. orig=.. carrier=.. ms=..          HOLD xbow refused reason=verify|journal|call
HOLD xbow grip via=arm_style|chord arm=a->b ok=0|1     HOLD xbow parts right=.. bolt_model=.. hidden=0|1
HOLD xbow frame e0=.. hand=.. rel=..                   HOLD xbow trip why=bolt_used qty=a->b
HOLD xbow restore why=mode_off|fp|ac130|nuke|world_leave|flag|detach|journal ok=0|1 ms=..
HOLD xbow foreign slot=.. now=..                       HOLD xbow restore_missing orig=..
HOLD chord verdict=played|ignored|fail head=.. arm=a->b   HOLD stance refused reason=er_busy
DEPTH probe self ... s=.. s_iqr=.. o_cm=.. n_eff=.. lev=a/b   DEPTH probe floor ..   DEPTH cams ..
```

### 16.12 Offline checks

- Core: `c6_carrier_plan_*` (gate rows, ranged skip, pick order + exclusions, give only with the flag, journal before call,
  every restore point, foreign / missing, hysteresis, no call while dead / loading / in an aim episode), `c6_carrier_journal_
  roundtrip` (+ a torn tmp file ignored), `c6_carrier_bolt_trip`, `c6_chord_verdict_*` (played / ignored / fail, er_busy,
  backoff cap), `c6_depth_self_fit_*` (depth8.md 2 a-f: synthetic 1.10, this log's 17 samples verbatim -> s in [1.08,
  1.12], unproven, n_eff 0.0553; one far sample never proven; o 0.30 / s 1.00 proven; s 0.90 keeps near_eff >= near_cam;
  the prime uses ticket.near = n_eff), the prologue table pinned against `xbow8\equip_249160.txt`.
- m2_static_checks c3wiring new rule 40: EquipItem / AddItem / Create / Find only from `carrier.rs` called by the task
  (never Present, a detour or the render thread); the journal write precedes every EquipItem; no `ChrAsm::SetSlot` VA and
  no `game::write` into ChrAsm / EquipGameData except the arm_style probe; `er_melee` suppressed with the carrier; the chord
  RB / LMB gated on bolt_loaded; mutations for each. c41wiring rule 12: the floor sample stays atomics-only. Run both
  `--self-test`.
- analyze_c6: `aim arms` fixed (only `mode=0`, a ranged row, idle head, >= 2 consecutive samples, print the row; else N/A "no
  vanilla crossbow sample"; the round-7 log must turn N/A); new rows `carrier` (on / restore pairs, every journal deleted,
  no foreign, no trip) and `chord` reporting `ignored` vs `fail`; `depth self` prints s / o / n_eff / lev and z*d per
  self_s; `depth live` checks `near=` == n_eff; self-test cases for each.
- vmtest `--erdepth` (ED0 26 texels) only if the floor probe is built. check_c6 check 9 proof strings + the new rows.

### 16.13 In-game test (solo; user runs; send the log after each step)

1. **Probe 0** (16.6), test toml as round 7, `er_carrier = "dry"`: MW2 off, Light Crossbow two-handed via the menu, stand,
   LB / RMB 3 s x3 with bolts, then x3 without bolts. analyze_c6 `aim arms` must say PASS with `row=430200xx mode=0`.
   N/A or WARN = stop and decide.
2. Still `dry`, MW2 on, third person: `HOLD xbow dry ... pick=..` with sensible slot / ids, no `xbow on`.
3. `er_carrier = "crossbow"`: MW2 on in third person: `HOLD xbow on`, `HOLD xbow grip ... ok=1`; on screen the
   crossbow carry, no crossbow visible, the MW2 gun on it (expect grip off until P-X5 is applied). Aim (LT / RMB): the arms
   raise; fire: MW2 shots only, bolt count unchanged (no `xbow trip`).
4. F6 to first person (`restore why=fp`, the original weapon), F6 back (swap again); F5 off (`restore why=mode_off`):
   the original weapon, grip and bolts as before.
5. Crash story: with the carrier on, quit to title; load in: `restore why=journal ok=1`. Die with it on: no call while dead,
   restore or re-swap after the respawn.
6. Depth (16.8): stand 30 s in the open, 30 s camera against a wall, walk: `scale=unproven|proven s=~1.10 n_eff=~0.055`,
   `DEPTH live on ... near=<n_eff>`, no `trip why=self`; a gun in front of a wall stays visible.
7. Chords: press Space then F5 on at once: `chord verdict=ignored` + retry, never `stance refresh fail`.
8. Co-op (2 PCs, after 1-7): P-X4 lines on both logs; the peer sees our crossbow carry / aim with the gun on it.

### 16.14 As built (2026-10-22, offline; NOT run in game)

Code (all behind `[coop] er_carrier`, code default "off"; the carrier acts only with `er_stance = "crossbow"`):
- **Core** `mw2er_core::carrier`: `Mode` (off / dry / crossbow), `CARRIER_ROWS` (Light first), `pick` (owned, not equipped in
  any of the 22 slots, by rank then index), `plan` (`PlanIn` -> `Step::{Idle, Swap, Restore(why), Wait(why)}`: a pending
  journal restores first; restores never wait the hysteresis; a swap needs the full gate), `restore_kind` (equip / done /
  foreign / missing), `Journal` (serde, `deny_unknown_fields`, version + slot + handle checks; a torn / foreign / newer
  file is ignored), `BoltTrip`, `chord_rb_allowed`, `grip_write_ok`, `CARRIER_2720` + `validate` (the four prologues).
  `OwnAddrs::carrier` (2.7.2.0 only). `er_stance` core gained `er_idle_head`, `er_busy`, `HeadAge`, `chord_played`,
  `chord_ignored`, `chord_verdict_label`, `IgnoredBackoff`. `er_depth_state` core: `fit_line`, the new `SelfOut` (s, s_lo,
  s_hi, o, lev, ratio, near_rel, bias_add, far_n), `self_trips(o, live_rel)`; `margin_rel` / `bias_rel_eff` / `near_ok` /
  `SCALE_SPAN_M` removed.
- **DLL** `carrier.rs` (`Carrier`, ticked by the task BEFORE the stance under its own `guard("carrier")`; the stance reads
  `CarrierView`, the carrier reads the stance's last-tick `carrier_feed()`), `game/equip.rs` (non-faulting EquipGameData /
  inventory / bolt reads, layout pinned by const asserts: 0x2B0 / 0x8 / 0x6C / 0x158 / 0x3FC, entry 24 bytes),
  `game::read_bytes`, `pad::set_carrier_melee_off` (the carrier's own R3 block, OR'd into the detour's melee wish), boot
  sets `<root>\state` only for a valid root, `HOLD cfg` + ` carrier=.. give=.. axes_xbow=..`.
- **Stance with the carrier**: `stance skip reason=carrier` while a swap / restore is pending (the sword is never patched
  under it), `stance carrier enter / exit` (no row patched; the exit target is the journal's grip), the aim episode holds
  ER's aim input without writing a field (`HOLD aim hold row=.. carrier=1`), the grip: `arm_style` write first (P-X2,
  `HOLD xbow grip via=arm_style ... ok= played= try=`), a revert / second attempt = the chord; two failed ER-played chords =
  `HOLD xbow grip fail` (carried one-handed, no disable); exit chords wait (`chord refused reason=carrier`) until the
  original is back; the chord's RB only while `bolt_loaded_states[slot] == 0` (`reason=loaded`).
- **Grips** (16.7): `[coop.grip.<gun>.xbow]` (`GripXbowCfg`) + `weapon_axes_xbow`; `coop_vis::hold_frame` uses them for a
  carrier row (ours or a peer's), the sword table is unchanged. P-X5 = `HOLD xbow frame phase=sword|xbow e0=[12] root=[16]
  n=` (with `probe_er_aim`), P-X3 = `HOLD xbow parts phase=before|after slots=[k:0x..] hidden=?`.
- **Chord 16.9**: `chord refused reason=er_busy` (the design's `stance refused` wording kept as the existing `chord refused`
  line; never counted, exempt in the analyzer), the grip-switch animation tracked through the queue during the chord
  window, `HOLD chord verdict=played|ignored|fail|void head=.. arm=a->b`, ignored chords retried 0.5 / 1 / 2 / 4 s (6 in 20 s
  cap) and never counted.
- **Depth 16.8**: as designed with one addition: the fit skips gross outliers (> 0.30 m from the pure-scale line
  `z = median(z/zl) * zl`) because with one cluster plus a far outlier every 0.3 m pair is an outlier pair (the 23:42:39 k16);
  they still count in ok / far / p95. `ok` counts NEAR-side residuals only (a surface behind the line can only under-occlude;
  `far=` reports the others). On this log: s 1.126 (1.098 with k16 kept), o -0.06 m, lev 2/2 unproven, n_eff 0.0553,
  pass (13/14). The prime's ticket near = near_cam x near_rel (`NEAR_REL[cand]`, never lowered while live) and its bias +
  `bias_add`; the live line prints `near=<n_eff> near_cam= near_rel=`.

Not built (documented): the bolt give / Bolt1 equip of X7 (only if probe 0 shows bolts are required); the optional floor
probe `probe_er_depth_floor` and the `DEPTH cams` line (16.8 step 5: no flag added, vmtest ED0 unchanged); a carrier
restore at DLL detach (detach runs under the loader lock off the game thread: the journal stays and the next world enter
restores); the gaitem handle release after `carrier_give`'s Create (not pinned: at most one extra reference until reload).
The crash recovery runs even with the flags off: one `state\carrier.json` existence check per world enter (no game write
unless a journal of this character exists).

### 16.15 Addendum: the first real swap (2026-10-22 09:58-10:01) - the grip, the abort, the aim A/B (offline; NOT run in game)

**What the log says [L]** (`logs\mw2er.log`, BOOT 09:58:48; `analyze_c6` before this addendum: `chord FAIL aborted for 15 s`,
`aim arms WARN carrier 0/12`):
- The swap and both restores worked (`xbow on ... carrier=43020000` x2, `restore why=fp ok=1` x2, no trip, no error).
- P-X2 answered: the `arm_style` write does not stick (`xbow grip via=arm_style arm=1->1 ok=0 played=0` at 10:00:35.462).
- **The carrier did become two-handed.** The keyboard chord (E + LMB) followed at ~35.48 (`MENU idx=23 0->1` 35.512 = ER's
  chord HUD, as at every chord of the run); ER played its grip switch (`aim probe anim=29060` 35.612) and the grip read
  `arm=3` from 35.713 on - every `aim probe` of the carrier (41.28-46.21) has `arm=3 row=43020000`, and the carrier exit
  logs `arm=3 target=3` (50.48). The `aim pose ... arm=0` field is the probe's "the weapon frame moved" flag, not the grip.
- The `chord refused reason=aborted` at 35.696 is the user's own menu (`MENU idx=28 0->1` + cursor, the same 28 -> 29 /
  HUD-hidden pattern as the user's menus at 31.14, 09:59:26, :53, 10:00:07, :10, :57) opening 0.2 s into the 0.3 s
  chord, AFTER ER had taken it. The keyboard branch threw the in-flight chord away (no verdict, no grip line), the grip
  was already the target, so nothing more was sent or logged and the analyzer saw a 15 s refusal. Root cause: an abort
  ignored that ER had already played the switch; a refusal had no "over" line.
- ER's crossbow aim animation never played with the carrier. Every carrier episode: ER's aim bit on (`prec=1 aim=1`),
  grip 3, `camtype=0` (our `keep` mask), snipe 1.00, the queue all 16000000. ADS presses were short (0.2-0.9 s: ads=1 ->
  ads=0 within 0.3 s six times). In probe 0 (vanilla, MW2 off, also arm=3, bolts 52000000 equipped) 46037130 / 46037110
  / 46037120 / 46037100 appeared only in the one long hold (09:55:36.7-40.5, 3.8 s, first 0.37 s after the bit, with
  `camtype=2` = ER's aim camera), NOT in the 1.1 s hold at 41.8 (camtype 2 too) nor in the short ones. So the remaining
  differences are (a) ER's aim camera (masked by `keep`), (b) the hold length / camera turning while held, (c) the
  loaded state (unread so far; EquipItem clears it [V], the menu equip of probe 0 too [I]). The input route is the same
  (RMB in ER's mouse buffer, ours or the user's), the grip is the same (3).

**Built (all under the existing flags; code defaults unchanged):**
1. `mw2er_core::er_stance::abort_finishes(played)`: an abort (keyboard: a menu / prompt from `refuse_now`; pad: the
   detour's code 3 / 4) of a chord whose grip-switch animation ER already played is **done**: the keys are released as
   at any end (`KB_CHORD_T0 = 0` first), `HOLD chord abort_after_played route=.. why=..`, and the normal verdict follows
   CHORD_CHECK_S later (`chord sent ... took=1` + `chord verdict=played`). Not played = refused `aborted` and retried as
   before (every guard unchanged: menu / prompt / mid-air / dead / spawn / aim / er_busy / carrier / loaded).
2. `HOLD chord done arm=.. target=.. after=<why>`: once the grip is the target while a refusal was the last word.
3. With the carrier: `HOLD xbow grip via=chord arm=a->b ok=0|1 played=..` after the chord's verdict; the first
   `arm_style` revert is remembered for the session (`xgrip_reverted`): later carrier enters chord at once
   (`stance carrier enter ... grip_via=chord|arm_style|none`). The write itself (P-X2) and its restore are unchanged.
4. `HOLD aim episode n=.. ms=.. carrier= mode= row= cam=keep|er|keep>er arm=a->b aim=<46037xxx|none> aim_at_ms= aim_last=
   aim_ticks= head=a->b prec_ms= prec_at_ms= ours_ms= ads_ms= camtypes=[..] snipe_min= loaded=a->b bolts=a->b route=`:
   one line per aim episode (ours or ER's own bit, MW2 mode off included; ends 0.3 s after both are off; 200 per
   session), with `probe_er_aim` or the carrier on. Pure tracker `EpisodeLog` / `aim_anim` (`id % 1e6` in 37000..38000)
   in core; the DLL reads the queue / snipe / the active right slot's `bolt_loaded_states` / the equipped bolt count
   (`game::equip::main_bolt_qty`, 2.7.2.0 only) only while an episode runs. Read-only.
5. `tp_aim_cam` was already hot (`config_hot_merge`, `AimCam::tick` reads it every tick; the next episode starts with the
   new value, a flip inside an episode shows as `cam=keep>er`). Verified, unchanged.

**Analyzer:** new row `aim anim` (per episode: aim anim yes / no, grouped by carrier / vanilla, cam, arm; PASS = a carrier
episode had it, WARN = carrier episodes without it, N/A = none; never FAIL). `chord`: closed by `chord done` /
`chord abort_after_played`, and for a DLL before them an `aborted` refusal during an enter is closed by the probe's
`arm=3` (the 09:58 log: chord PASS, "1 aborted refusal closed by the probe"). Static rule 40 (c3wiring) gained the abort
/ done / episode checks (+6 mutations, 110); check_c6 check 9 + 7 proof strings.

**Next in-game test (one run; the A/B is the hot toml edit):** see MODLOG 2026-10-22 "part 16 addendum". Decision rule:
`aim anim PASS` with `cam=er` only = ER's aim body animation needs its aim camera (then a narrower mask is the next design
step: keep the camera call site, mask only the reticle / fade, or move our camera after ER's); `cam=keep` PASS too = it was
the short holds; WARN under both with long holds and `loaded=0->0` = the loaded state is the remaining suspect (16.2 X7).

Checks: core 727 tests (c6_ 88: carrier pick / plan / journal / restore kind / bolt trip / prologues / the dump pin,
`c6_chord_verdict_ignored_er_busy_backoff`, `c6_depth_state_self_fit_offset_scale_edges`, `c6_depth_state_self_fit_round7_log`
(the 17 samples verbatim), `c6_carrier_cfg`); `tests/real_exe_2720.rs real_exe_2720_carrier_prologues` (the exe file:
4 prologues + 4 mutations + the globals' sections) PASS; DLL `c6_carrier_journal_file_atomic`; analyze_c6 56/56 (rows
`aim arms` fixed, `carrier`, chord verdicts, depth self z*d / n_eff, depth live near = n_eff); m2_static_checks c3wiring
rule 40 (104 mutations), c41wiring unchanged (40); check_c6 10/11 (check 6 = the test toml, expected).

### 16.16 The loaded crossbow (`[coop] carrier_load`; 2026-10-22, after the aim A/B 10:18-10:25; offline, NOT run in game)

**Why [L]** (`logs\mw2er.log` BOOT 10:18:45, `analyze_c6`): 17 carrier episodes with the real Light Crossbow, two-handed
(arm=3), bolts 4, ER's aim bit on, `cam=keep` holds up to 6.6 s and `cam=er` holds up to 1.6 s with ER's aim camera types
0-3: ER's aim animation 46037xxx never played (`aim anim WARN 0/17`). Every episode `loaded=0->0`. Probe 0 (vanilla, MW2 off)
had it in a 3.8 s hold. **User decision: one round with a LOADED crossbow; if the aim animation still does not play, the
aim-animation work stops.** (Note for the reading: probe 0 also showed no reload animation 4603xxxx in its anim samples, so
the vanilla crossbow may not have been loaded either; and its four 46037100-130 ids came with 50-98 degree turns. Loaded is
the user's last candidate, not a proven cause.)

**How ER loads a crossbow [V 2.7.2.0, the exe read as a file with work\c6\aim6 erdis; tags as 16.2]:**
- The state is `ChrAsm.bolt_loaded_states[12]` (+0xDC in a ChrAsm; EquipGameData +0x148). Readers: `ChrAsm::IsBoltLoaded`
  0x1403BEC40 (`[asm + slot(hand) + 0xDC] != 0`; callers 0x1402606D7, 0x140411DD5, 0x14077473B / 914, 0x1409EF2A7 / FE33 /
  FF1D [I: HKS env / menu / HUD]). The net record packs the 12 bits (0x1403BE906 into `+0xB4`) [I: peers see it].
- Writers: (a) `ChrAsm::SetBoltLoaded(asm, hand, bool)` 0x1403BF4B0 (`slot = ChrAsm::SlotOfHand(asm, hand)` 0x1403BE440:
  hand -1 = `2 * right_weapon_slot + 1` = the active right weapon slot, -2 = the active left one, 0..11 = itself). Its only
  callers: the save / net loader 0x14025ED2D (12 bits from a stream) and two **TimeAct event handlers** in the event switch
  0x14042E7B9 (event id byte table 0x14042F380, jump table 0x14042F2C8): **event 35 -> 0x14042C280 = loaded = 1**, **event 34
  -> 0x14042C360 = loaded = 0**, each `SetBoltLoaded(chr->vt+0x220 (PlayerIns+0x638 ChrAsm), arg byte ? -1 : -2, v)`, then
  0x140659FC0 (main player: `0x1403BF4F0` copies the PlayerIns ChrAsm into EquipGameData's, bolt bytes included) and
  0x140659F90 [I: a sync / notify]. So ER loads a crossbow ONLY through its reload animation's TAE event 35 (the first R1
  on an unloaded crossbow plays the reload [I vendor `bolt_loaded_states` doc + C6 stance.md]; the shot's TAE event 34
  unloads it). (b) `ChrAsm::SetSlot` 0x1403BF47A clears the slot's byte on every equip (our swap and restore). (c) `ChrAsm::Copy`
  0x1403BF300 copies the 12 bytes; the PlayerIns update 0x140661543 runs `Copy(PlayerIns asm <- EquipGameData asm)` every
  update of the main player: EquipGameData's byte is the master (the same copy reverted the P-X2 `arm_style` write, [L]
  10:00:35.462). Not a param flag, not the equip itself (the equip clears it), not two-handing.

**Decision: write ER's TAE-event-35 end state, not ER's load path.** ER's own path needs an R1 to reach ER (the reload
animation), which MW2 mode forbids (RB / LMB never reach ER; a buffered second press during the reload would be a shot). The
end state is two bytes, both ER keeps consistently: `bolt_loaded_states[s] = 1` in EquipGameData's ChrAsm (the master; the
per-frame copy carries it into PlayerIns's) and in PlayerIns's ChrAsm (what event 35 writes first and what this tick's
readers - the stance's chord gate, the episode line - see at once). Not called: 0x140659F90 (peers learn the state from the
regular net record [I]). **No bolt is consumed** (only a shot consumes one [V xbow8 2]; the load is a byte); bolts before /
after are logged, the bolt tripwire stays.

**Mechanism (`mw2er_core::carrier::load_step` pure; DLL `carrier.rs` `load_tick` / `load_write` / `load_undo` /
`write_bolt_loaded`, task thread, inside the carrier tick BEFORE the plan / restore and before the stance's tick):**
- Load only when ALL hold: `carrier_load` and `er_carrier = "crossbow"`, the carrier not disabled, `xbow on` verified (no swap /
  restore in flight), in world, MW2 mode on, third person, no AC-130 / nuke, alive; EquipGameData's grip = 3 for >= 1.0 s
  (`LOAD_SETTLE_S`: the chord's verdict and any grip retry are over); no chord (the stance's feed `busy=chord` = in flight,
  and the keyboard chord window `KB_CHORD_T0 != 0`); ER idle (the feed: er_busy / air / mount / prompt / aim); the slot
  readable and unloaded on both ChrAsms; equipped bolts > 0; the slot holding our crossbow's gaitem handle on BOTH ChrAsms.
  At most 3 loads per swap (`LOAD_MAX_PER_SWAP`).
- Verify 0.5 s later (`LOAD_VERIFY_S`): both bytes read 1 = `ok=1`; else `ok=0`, undone, no more loads this swap.
- **Undo (write 0, our restore) first** at every one of: flag off, carrier disabled (trip / refused / stance off), world
  leave (best effort; the journal restore's EquipItem clears it anyway), MW2 off, first person, AC-130, nuke, dead, the
  carrier not on, the grip not 3 (a chord may follow), the chord keys down; and inside `restore()` right before the
  restore's EquipItem (which clears it again [V 0x1403BF47A]). A slot that no longer holds our crossbow is not written
  (`already=1`). `xbow load lost` = ER unloaded it itself (event 34 = a shot, or an equip): forgotten, not loaded again.
- Never fire (16.4 unchanged and re-checked): RB / R2 / LMB never reach ER in MW2 mode (`blank_er`); R3 melee stays blocked
  (`set_carrier_melee_off(equipped)`); the chord is refused `loaded` while the byte is 1 (`chord_rb_allowed`: the view's
  `loaded` comes from PlayerIns's byte, written in the same tick before the stance reads it), and the load waits for the
  grip to be two-handed first; the aim synth holds only LB / RMB; the bolt tripwire (drop -> restore + off) unchanged.
- Residual risks (documented): with a panic in the carrier tick (`carrier_off`) a load is not undone; MW2 mode still blanks
  R1 and the next session's journal restore clears the byte. If the user turns MW2 off, the undo runs in the same tick as
  the restore (before the EquipItem). A save written while loaded stores loaded=1 for the crossbow slot (the save serializer
  0x14025ED2D); the restore's EquipItem clears it.

**Config / lines / checks:**
- `[coop] carrier_load` (code default false, hot; `HOLD cfg ... load=0|1`). Test toml: true (dated 2026-10-22). check_c6
  check 6 and tests\config_file.rs require `carrier_load = false` in the shipped toml.
- `HOLD xbow load wait reason=grip|settle|chord|no_bolts|already|unreadable|slot|<busy> arm= loaded= bolts=`, `HOLD xbow load
  write slot= handle= arm= loaded= egd= bolts= try=n/3 wrote_egd= wrote_pl=`, `HOLD xbow load method=write slot= ok=0|1|-
  loaded=a->b egd=a->b bolts=a->b ms= arm=`, `HOLD xbow load undo why=.. slot= ok= egd=a->0`, `HOLD xbow load lost`, `HOLD xbow
  load cap`. The per-aim `HOLD aim episode ... loaded=a->b` reads PlayerIns's byte: `loaded=1->1` with a load in place.
- analyze_c6: new row `load` (PASS a verified load, undone / restored, bolts unchanged; FAIL `load lost`, a bolt drop across
  the load, an undo with ok=0; WARN ok=0 only / still loaded at the end / waits only; N/A off), `aim anim` grouped by
  `loaded=` (its WARN says "LOADED ... stop the aim-animation work" when loaded carrier episodes had no aim anim).
  Self-test 59/59. m2_static_checks c3wiring rule 40 extended (the only carrier write is `write_bolt_loaded`: 2 u8 writes
  of `ASM_BOLT_LOADED + s`, values 0 / 1, called only from `load_write` (1, with the slot check) / `load_undo` (0); the load
  tick before the plan; the undo before the restore's EquipItem; the core gate's chord / grip rules; default off) + 6
  mutations (116). Core test `c6_carrier_load_step`.

**In-game test (solo, one round; test toml: er_carrier = "crossbow", carrier_load = true, tp_aim_cam = "keep"):** see MODLOG
2026-10-22 "part 16.16". Decision: `aim anim PASS ... loaded=1` = the loaded state was the missing piece (keep it, design
the default); `WARN ... LOADED ... stop` = stop the aim-animation work (user decision).

## 17. The polearm stance and the third-person GUARD as the ADS pose (2026-10-22, after the loaded-crossbow test; offline, NOT run in game)

Inputs: the loaded-crossbow test (MODLOG "loaded-crossbow test (10:46-10:50)": user "animation didnt work", `aim anim WARN
0/8` all loaded: the crossbow aim-animation work STOPPED), the user decisions of 2026-10-22 (back to the disguised sword:
`er_carrier = "off"`, `carrier_load = false`; try the two-handed polearm GUARD as the third-person ADS pose) and
`work\c6\guard9.md` (guardmotionCategory is only a shield enum; the two-handed pose comes from the motion banks; the polearm
two-handed base set 13 holds the hands spread along a shaft; the stance patch + chord already swapped banks in MW2 mode).
First person and the scope peek are byte-identical (every change acts in third person behind flags that are off by default).

### 17.1 What is verified, what is inferred

- [V regulation, `param_rows.py` on the vanilla EquipParamWeapon, reg 11730034] Spear 16010000 / Crystal Spear 16020000 /
  Clayman's Harpoon 16030000: wepmotionCategory 36, guardmotionCategory 2, spAtkcategory 0, wepmotionOneHandId 3,
  wepmotionBothHandId 13 (Short Spear 16000000: spAtk 203; Lance 17060000: motion 37; Halberd 18000000 / Pest's Glaive
  18010000: motion 38, both 13). The user's Longsword 2000000: 23 / 2 / 0 / 0 / 10, weaponCategory 1, wepType 3.
- [V log] the stance + the two-hand chord re-resolve the banks of the hidden Longsword (crossbow two-handed idle / walk /
  run 16000000 / 16020120 / 16022100 sampled with row 2000000 in MW2 mode).
- [I] ER's guard is the normal melee path (L1 / RMB held while two-handed): the Longsword is a melee type already, so
  no type write is needed, and the guard animation comes from the patched banks (the polearm set's). Residual [I]: HKS
  may pick the guard from weaponCategory / wepType instead (then the Longsword's own two-handed guard plays: still a
  guard, only less rifle-like) - the `HOLD guard episode heads=[..]` ids tell which bank played (a013_ / a036_ vs a010_).
- [I] ER's guard animation ids: `xx017xxx` (`er_stance::guard_anim`); never logged yet, NOT needed for the detection (the
  weapon frame moving off the carry is).

### 17.2 Decision: the whole stance uses the polearm set (`[coop] er_stance = "polearm"`)

Two options: (a) keep the crossbow carry and switch the banks only for the ADS, (b) the whole stance on the polearm set.
(a) needs a bank change per ADS press: a write of the five fields + a 3 -> 1 -> 3 chord (two grip-switch animations,
~0.8-1.2 s with the 0.4 s gap and the 0.5 s checks, a one-handed flash in between, refused while ER is busy) and the
same back at the release - far too slow and too visible for an ADS. **(b) is built**: `er_stance = "polearm"` writes the
vanilla Spear's five values (`er_stance::POLEARM_VALS` = [36, 2, 0, 3, 13]; `StanceSet::Polearm`) instead of the crossbow's
(the same five fields, the same patch / restore / chord / hysteresis / peers / skip rules of part 3; weaponCategory /
wepType never written). The carry is then ER's two-handed polearm idle / walk / run, and the guard is just the block
input held. Consequences:
- `tp_ads_pose = "er_aim"` (the crossbow aim, part 14) and the carrier (part 16) need the crossbow set
  (`CoopCfg::ads_pose_on` / `carrier_mode` use `stance_crossbow()`); `tp_ads_pose = "guard"` needs the polearm set
  (`guard_pose_on` = `er_stance == "polearm" && tp_ads_pose == "guard"`). A mismatch is inert (the `HOLD cfg` line shows
  the effective `ads_pose=`). So the aim-camera getter filter (14.8 / 14.11) is never armed or installed with the guard
  (`aim_cam_keep` needs `ads_pose_on`).
- Every patched row remembers the set it was written with (`Patched.set`); the restore re-reads against that set
  (`classify_set`). A hot `er_stance` change crossbow <-> polearm: our row exits with its old set restored (`HOLD stance set
  change new=..`, the exit chords as any exit), the re-enter writes the new set after the hysteresis (never resumed
  across a set change: the deferral is cleared); a peer's row is restored and re-patched with the new set in the same
  tick. A row that already holds the set's values (a real spear) is refused `foreign` as before (the original unknown).
- Peers: each PC patches a marked peer's row with ITS OWN set (both PCs should use the same toml).
- `stance enter row=.. orig=.. arm=.. left=.. set=polearm new=[36,2,0,3,13]`.

### 17.3 The guard episode (`[coop] tp_ads_pose = "guard"`, code default "off"; `mw2er_core::er_stance::GuardCtl`)

Wanted = the aim pose's gates (`aim_wanted` with the guard flag): the guard flag (and the stance not disabled), the
polearm stance POSED (active, grip 3, no chord in flight, the enter's re-resolve done, our own row patched with the polearm
set), MW2 ADS held and allowed (not sprinting, the gun not lowered), third person, NOT a scoped gun taken by
`tp_scope_peek` (the scope peek stays first person exactly as before), no AC-130 / nuke / death / menu / latch / prompt /
Torrent / mid-air, a fresh input route (pad, else keyboard / mouse).

| Step | What | Log |
|------|------|-----|
| ADS edge | ER's block input held from this tick: pad = the existing LB synth (`pad_logic::ER_AIM_BUTTON`, only with MW2 mode driving, never in the gunship / at a prompt), keyboard = RMB (`shared::KB_AIM`) after the raw ADS is recorded, never in the gunship. Nothing is written on the row. | `HOLD guard on row=.. route=pad|kb arm=3 head=.. carry=0|1` |
| detect | ER's weapon frame (`aim_cam::weapon_rel`, body-yaw frame, read-only) leaves the last standing carry (`pose_moved`: > 0.10 m or > 15 deg) on 2 consecutive 0.1 s samples (`via=frame`), or a head animation of the guard family (`via=anim`) | `HOLD guard detected ms=.. via=frame|anim head=.. moved=m/deg` |
| 0.6 s | not detected (`GUARD_CONFIRM_S`): the fallback until it is (17.5) | `HOLD guard fallback ms=.. carry=..` |
| release | ADS up / any gate: the input is released at once (nothing to restore); one line per episode | `HOLD guard episode n=.. ms= row= route= arm=a->b head0= heads=[<= 8 distinct ids] guard_anim=<id>|none detected=0|1 via= det_ms= moved_max= turned_max= align=<deg>|none align_min= clamped=0|1 fallback=0|1 fb_ms= yaw=<writes> why=release|fp|scoped|stance|killstreak|dead|menu|prompt|mounted|air|input|flag|<stop>` |
| hard stops | every stance restore point (our row's restore, `restore_all`: world leave / panic / the C3 strip, `disable`, the flag off): the episode ends, the input released by the same path | the episode line with `why=<restore reason>` |

No failure count and no disable: an undetected guard only costs the fallback. No grip chord while a guard episode holds the
block input (the chord refusal `aim`: Y + RB would be a guard attack). RB never reaches ER (no guard counter; RT / R2 are in
`blank_er`), R3 melee stays suppressed while the stance is on, no parry (the right weapon is two-handed: L1 is its guard;
L2 / LT is blanked; a left shield is on the back). The carry sample is taken only while standing still with the stance
posed and no guard held; walking guards are judged against the standing carry (a walking detection is possible, logged).

### 17.4 The body faces the camera (`[coop] tp_guard_yaw`, code default false; test toml true)

ER does not turn the body to the camera while guarding (the crossbow aim did) [I]. With the flag, while the guard is
held AND standing still (no move key / stick, no horizontal movement in the last 0.5 s), not locked on, turning allowed
(`action_modifiers_flags` readable, `AF_DISABLE_TURNING` clear), no throw, alive (`er_stance::guard_yaw_wanted`), and the
body more than 1 deg off the camera yaw: the M3B first-person yaw write (one 32-byte `game::write` of
`CSChrPhysicsModule.orientation` + `interpolated_orientation`, `fp_math::quat_from_yaw(yaw_of(cam, axis))`, the axis of
`[first_person] yaw_forward_axis`, "auto" = -z). ER's own facing that it rewrites itself: nothing to restore (as M3B).
Never in first person (no guard there). `HOLD guard yaw write ok=0|1 off_deg=.. axis=..` once per episode, the writes
counted in the episode line (`yaw=`). The FP yaw follow is untouched.

### 17.5 The gun in the guard; grips; HUD

- ER's weapon frame (e0) is the weapon model's rig constant (14.3), so the MW2 gun stays on it with the same grip table
  (no separate polearm grip set in this round). Once the guard is detected the gun turns about its pistol grip onto the
  camera's aim point (`hold::aim_align`, 0.12 s blend) by at most `[coop] tp_guard_align_max_deg` (code default 20 = the aim
  pose's, test toml 45): `HOLD guard align gun=.. deg=.. max=.. rake=..` once per episode.
- Fallback = the round-7 gun raise (`tp_ads_hold = "aim"`: the camera-aimed stage-A pose): while the guard is held and NOT
  detected after 0.6 s, or detected but more than the max off the aim line (`HOLD guard align_clamped`, latched for the
  episode) - `hold::guard_nopose`; `HOLD guard hold_fallback gun=.. why=not_detected|clamp`. A late detection hands back to
  the align. `tp_ads_hold = "hand"` = the gun stays on ER's weapon frame as is.
- If the episode lines show a constant `align=` well above 0 on every detected episode, a `[coop.grip.<gun>]` rake / a
  polearm grip set follows (the angle IS the correction); not before a log.
- HUD: the MW2 four-line crosshair stays at full alpha in the guard (`tp_aim_dot`, as the crossbow aim); our normal
  third-person camera (no ER aim camera exists for a guard; nothing is masked).

### 17.6 Side effects (documented for the user)

Guarding blocks: frontal physical hits are cut by the Longsword row's guard rate (45 %; the polearm fields do not change it)
and cost stamina; at 0 stamina a guard break (stagger, the gun jolts); a blocked hit plays a guard recoil; stamina
regenerates slower while guarding [I]. ER allows a guard-walk; a dash / sprint drops it (MW2 sprint ends ADS anyway). Not
locked on, ER turns the body toward the move input while guard-walking (17.4 only acts standing). The Roundtable Hold /
no-combat areas may refuse the guard (then the fallback raise). Peers see our guard (synced animation) with their copy of
our row (stance_peers).

### 17.7 Config (all hot; `HOLD cfg ... ads_pose=off|er_aim|guard ... guard_yaw=0|1 guard_max=..`)

```toml
[coop]
er_stance              = "off"   # off | crossbow | polearm (part 17)
tp_ads_pose            = "off"   # off | er_aim (crossbow) | guard (polearm)
tp_guard_yaw           = false   # the body faces the camera while the guard is held standing
tp_guard_align_max_deg = 20.0    # 0..90
```
Test toml (dated 2026-10-22): `er_stance = "polearm"`, `tp_ads_pose = "guard"`, `tp_guard_yaw = true`,
`tp_guard_align_max_deg = 45.0`, `tp_ads_hold = "aim"` (the fallback), `er_carrier = "off"`, `carrier_load = false`.
check_c6 check 6 / `tests\config_file.rs` keep `er_stance = "off"`, `tp_ads_pose = "off"` and now `tp_guard_yaw = false` in
a shipped toml.

### 17.8 Checks (as built, offline)

- Core: `c6_polearm_set_is_the_vanilla_spear`, `c6_guard_episode_hold_detect_release`, `c6_guard_fallback_then_late_detect`,
  `c6_guard_yaw_gate`, `c6_guard_wanted_gates`, `hold::c6_guard_nopose_raises_only_without_the_guard_pose`,
  `config::c6_guard_cfg` (the guard never arms `ads_pose_on` / `aim_cam_keep` / `aim_patch_stance` / the carrier).
- `m2_static_checks c3wiring` rule 41 (+13 mutations; rule 36 / 40 strings follow `stance_crossbow`, +1 mutation):
  the guard / yaw off by default and only with the polearm stance; the guard writes nothing on the row and never touches
  the aim-camera filter; the aim gates with the polearm set posed; no chord during a guard; every stance stop ends it; the
  yaw is one write of the physics orientation pair behind its gate; a row restored against the set it was written with.
- `analyze_c6.py` row `guard` (17.9 lines; FAIL on any crossbow-aim line with ads_pose=guard, a chord or a stray bullet in
  a guard span; WARN never detected / yaw refused) + 4 mutations; the stance / hide / pair rows accept `er_stance=polearm`.
- `check_c6.ps1` check 9 + the guard strings; check 6 + `tp_guard_yaw = false`.

### 17.9 In-game test (solo; user runs; send the log after each step; test toml as 17.7)

1. Boot, load in at an open-world grace (not the Roundtable), third person, the M4 (non-scoped). F5 on. Expect `HOLD cfg
   ... er_stance=polearm ... ads_pose=guard ... guard_yaw=1 guard_max=45`, `HOLD stance enter row=2000000 orig=[23,2,0,0,10]
   arm=1 left=.. set=polearm new=[36,2,0,3,13]`, the chord `chord sent ... took=1 ... kind=enter` + `chord verdict=played`.
   On screen: the two-handed polearm carry (hands spread along the hidden sword), the MW2 gun on it. No `HOLD aim cam ...`
   line at all.
2. Stand still, hold LT / RMB 3 s, release; 3 times. Expect per press `HOLD guard on row=2000000 route=pad|kb arm=3 ...
   carry=1`, `HOLD guard detected ms=<~150-400> via=frame ...`, `HOLD guard align gun=m4_mp deg=<x> max=45`, `HOLD guard
   yaw write ok=1 ...` (first press), on release `HOLD guard episode n=k ... detected=1 ... align=.. fallback=0 yaw=.. why=
   release`. On screen: the body raises the weapon into the two-handed guard, faces the camera, our normal camera, the MW2
   crosshair, the gun's barrel on the crosshair; release = the carry. Fire while guarding: MW2 shots only.
   If instead `HOLD guard fallback ms=600` + `HOLD guard hold_fallback gun=m4_mp why=not_detected` and `detected=0`: no
   guard pose (send the log: the `heads=[..]` ids and `moved_max=` say what ER played).
3. Turn the camera while guarding standing (the body follows: yaw writes), then guard-walk forward / sideways (the body
   follows the move input, no yaw write), dash (the guard drops: `why=release` or the ADS ends).
4. Let an enemy hit you while guarding (the block: stamina drops, no damage line oddities), and once without stamina (a
   guard break). Note how it looks.
5. F6 into first person while guarding: `HOLD guard episode ... why=stance|fp`, first person exactly as before; F6 back.
   Scoped gun (Intervention): LT scopes into first person (`FP on ... peek=1`), NO `HOLD guard on`.
6. Keyboard only (pad idle): RMB: `route=kb`, the same pose.
7. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: `guard PASS ... detected ...`, stance / chord / no ER fire PASS,
   aim pose / aim cam N/A.
Decision: `guard PASS` with small `align=` = keep (then decide whether the guard's block buff is acceptable / a default);
`WARN never detected` = send the log (the head ids decide: a Longsword guard a010_ = HKS ignores the banks -> the
carrier with a real vanilla Spear, guard9.md 2).

### 17.10 As built (2026-10-22, offline; NOT run in game)

Core `er_stance` (`StanceSet`, `POLEARM_VALS`, `classify_set`, `GuardCtl` / `GuardSample` / `GuardEpisode`, `guard_anim`,
`GuardYawIn` / `guard_yaw_wanted`), `hold::guard_nopose`, `config` (`er_stance` polearm, `tp_ads_pose` guard,
`tp_guard_yaw`, `tp_guard_align_max_deg`, `stance_crossbow` / `stance_set` / `guard_pose_on` / `guard_yaw_on`). DLL
`er_stance.rs` (`guard_tick` / `guard_frame` / `guard_yaw` / `guard_stop`, `Patched.set`, the set change, the guard in
the chord refusal and in every stop), `coop_vis.rs` (the guard align / clamp / fallback, `guard_align()`, the cfg line),
`task.rs` (the inputs, `tp_aim_dot` with the guard). No new hook, no new param field, no regulation / package change.

### 17.11 Review fixes (2026-10-22, offline; NOT run in game)

- The align clamp no longer latches for the episode on one frame (it caught ER's guard raise in motion: detection fires
  0.1 s after the frame leaves the carry, and a low carry is far off the aim line partway up; or a block-hit reaction).
  `hold::GuardClamp`: the align is ignored for `GUARD_ALIGN_SETTLE_S` 0.25 s after detection, clamps only after
  `GUARD_CLAMP_ON_S` 0.2 s over `tp_guard_align_max_deg`, and unclamps after `GUARD_CLAMP_OFF_S` 0.15 s back under it, so
  the round-7 raise covers only the off-line stretch. The episode line's `align` / `align_min` count from the settle on;
  `clamped=1` = the debounced clamp held once. Lines `HOLD guard align_clamped` / `guard align_unclamped` (at most 4 per
  episode).
- The guard's standing carry is sampled only on ER's settled idle: `er_stance::guard_carry_ok` = standing, posed, ER not
  busy (idle head for 0.3 s), the head the base-set idle (`id % 1_000_000 == 0`, never a guard anim; an unreadable head
  does not block) and `GUARD_CARRY_QUIET_S` 0.5 s after the last guard episode ended (ER's guard lower would be stored as
  the carry, so a quick re-ADS would see no move and fall back).
- Weapon-frame detection only in an episode started standing and only on standing samples (the walk / run arm swing
  passed 0.10 m / 15 deg against the standing carry: a false `via=frame`, no raise, a false analyzer PASS); a moving
  start relies on the guard animation id. `HOLD guard on ... carry_age=<s> frame=0|1`.

### 17.12 Addendum: the guard frame and the polearm carry grips (2026-10-22, after the guard-pose test 11:46-11:51; offline, NOT run in game)

Input: MODLOG "guard-pose in-game test (11:46-11:51)": the polearm guard plays (11/30 detected via the frame, det 267 ms
median) but `align` 79.9..96.2 deg in EVERY detected guard -> clamp (max 45) -> the round-7 raise ("the gun hovers over
the right shoulder"); the polearm carry holds some guns badly (Ranger low at the right hip, barrel back / left). The row
was 18080000 (a halberd, orig [38,2,211,3,13]). Blocking stays (user decision).

**What the logged frames show [L]** (`HOLD axes` = b73's columns, e0 = b73 x Rz(180) = [-hx, -hy, hz] again; the weapon
frame is ER's R_Weapon bone, so these are constants of ER's polearm ANIMATIONS, not of the hidden weapon model):
- Guard (6 `HOLD axes ... ads=1 posed=1` lines, the body yaw-aligned to the camera by `tp_guard_yaw`): the body's level
  forward in weapon coordinates = (-0.091, -0.048, -0.994) (~ -Z, spread 0.3..3.2 deg), world up = (0.982, 0.162, -0.097)
  (~ +X); the left hand (b29) 0.58 m along -Y = the character's LEFT, the right hand bone b57 8.7 cm along +Z. ER's polearm
  guard holds the shaft ACROSS the chest; the sword-carry grip (barrel along -Y raked -18 deg about Z) pointed the barrel
  left / down: 65.9..86.1 deg off the camera forward on those six frames = the logged 79.9..96.2 clamps.
- Standing polearm carry (probe frames t=0.0 / 0.5 / 7.1 / 7.6): the left hand at (0.010, -0.534, 0.090) (0.541 m along
  -Y: the shaft runs from the right fist at the hip forward / up / LEFT across the body), up = (0.777, -0.557, -0.293), the
  body's level forward ~ (-0.613, -0.610, -0.501) (two estimates 13 deg apart [I]: the guard's yaw before a release and the
  -70.8 deg yaw-write offset). The sword grip's barrel: forward 0.77 / up 0.29 / left 0.57 in the body frame (~21 deg off
  the shaft, neither along it nor ahead). Walking / running carry frames were never logged [I: the gun follows ER's weapon
  frame there, as before].
- Anim ids: 13019000 was the head at all 11 frame detections, 13019200 / 13019500 in guard episodes, 13019100 right after
  every release (`aim probe ... ads=0`: ER's guard LOWER). The 9 fallbacks were guards started walking (frame detection only
  standing) whose heads held 13019000.

**A. The guard frame (`[coop] guard_axes`, code default "auto"; `guard_rot_deg`).** In a DETECTED guard the gun is drawn
with a guard frame on ER's weapon pose: MW2 X (barrel) along the body's level forward, MW2 Z (top) world up, the pistol
grip G still on ER's weapon origin (the right fist; no translation: the stock then runs back toward the shoulder).
`hold::guard_frame_builtin` = `gun_frame(GUARD_FWD_W, GUARD_UP_W)`; on the six logged frames the barrel is 1.5..8.1 deg
off the camera forward (the camera pitch; 53 deg on the one frame with the camera 53 deg down) and <= 7 deg off the level
facing (core test `c6_guard_frame_fixes_the_logged_guards`). Pinned: `guard_axes = "-z,-y,+x"` + a small turn
(`hold::frame_pin`). As a grip it is `hold::frame_grip(G, frame, weapon_axes)` = (offset, rot) on the usual
`grip_matrix(offset, rot, axes)` path (so `T(offset) * frame` with G at the origin).
- Blend: `ads_blend_step` (0.12 s) in once the episode is detected, out at the release (the last frame kept for the blend
  out); only on a hand-path gun (never stage A), only third person (the self block), only `guard_frame_on()` =
  `guard_pose_on() && guard_axes != "off"`.
- The align: `aim_align` runs AFTER the correction, so it only turns the residual (camera pitch, parallax); the debounced
  clamp of 17.11 still judges the corrected gun: over `tp_guard_align_max_deg` = the round-7 raise, as before.
- Auto learn (`guard_axes = "auto"`, `hold::GuardLearn`): every 0.1 s of a detected, settled (0.25 s), STANDING guard (the
  stance's `guard_standing`: no move input / movement) the body's level facing and world up are taken in weapon
  coordinates and summed; after 20 samples with both mean lengths >= 0.97 the row's frame is learned (session; a new row
  starts again) and used instead of the built-in: `HOLD guard frame auto row=.. axes=<map> rot=[..] n=20 spread=<deg>
  vs_builtin=<deg> (... set [coop] guard_axes = ".." and guard_rot_deg = [..] to keep it)`.
- Lines: `HOLD guard frame src=builtin|auto|fixed gun=.. row=.. offset=[..] rot=[..] fwd_err=<deg>` once per episode (the
  barrel vs the level facing this frame), `HOLD guard align ... corr=<src>`, the episode line ends ` corr=<src>|off`, the
  cfg line gains ` polearm_grip=0|1 long_min=.. guard_axes=<effective>|off guard_rot=[..]`.
- Guard anim family (`er_stance::guard_anim`): + `xx019000..xx019099` and `xx019200..xx019999` [L], never `xx0191xx` (the
  lower): a guard started walking is now detected `via=anim` (the 9 fallbacks of that run) and gets the frame too.

**B. The polearm carry grips (`[coop] polearm_grip`, code default true, acts only with `er_stance = "polearm"`;
`polearm_long_min_m` 0.50; `[coop.grip.<gun>.polearm]` offset_m / rot_deg).** `CoopCfg::weapon_grip_polearm` replaces the
sword grip table for every player drawn with the polearm stance (`grips()`; peers carry our set through `stance_peers`):
- long guns (`hold::MUZZLE_AHEAD_M` >= 0.50 m, from grip3/final.json `muzzle_ahead_G_m`; unknown / javelin = long): the
  pistol grip in the right fist, the barrel toward the left hand along the shaft (`gun_frame(POLEARM_LH_W, POLEARM_UP_W)`,
  <= 3 deg off the logged hand-to-hand line), top up: port arms across the body; the left hand lands on the fore-end.
- short guns (famas, tavor, fn2000, mp5k, ump45, kriss, p90, uzi, ranger, m79: muzzle < 0.50 m, the left hand would
  float past the muzzle): the pistol grip in the right fist, the barrel level along the body's facing
  (`gun_frame(POLEARM_FWD_W, POLEARM_UP_W)`) - the "Ranger at the hip, barrel back / left" case now points ahead.
- `HOLD grip who=self gun=.. src=polearm_long|polearm_short|config|mixed offset=[..] rot=[..]`; a `rot_deg` key alone
  keeps G in the fist (the offset follows). `polearm_grip = false` = the sword table (part 17).

**Unchanged:** blocking (user decision), the guard's gates / detection / fallback / clamp / yaw, the crossbow set and the
carrier grips, first person and the scope peek (the self block needs `!fp_on`; nothing here runs in first person), no game
write (drawing only). Code defaults: `er_stance = "off"` / `tp_ads_pose = "off"` keep all of it inert.

**Checks:** core `c6_guard_frame_fixes_the_logged_guards`, `c6_polearm_carry_from_the_logged_carry`,
`c6_frame_grip_round_trips`, `c6_guard_learn_from_the_logged_guards` (the logged matrices as fixtures),
`config::c6_polearm_grip_and_guard_frame_cfg`, `c6_guard_fallback_then_late_detect` (+ the 019 family, the lower
excluded), `c6_guard_episode_hold_detect_release` (+ corr=); `tests\config_file.rs` + `!polearm_grip_on() &&
!guard_frame_on()`; c3wiring rule 42 (+7 mutations); analyze_c6 guard row: `corr <src> on k/n: align a..b deg, clamped
c; fwd_err ..; learned axes= rot= vs_builtin=`, WARN when every corrected guard clamped, FAIL on corr in an undetected
episode or a `guard frame src=` line outside a guard span (+2 mutations); grip row `polearm_grip=`; check_c6 check 9 + 7
strings.

**In-game test (solo; test toml as 17.7 + `guard_axes = "auto"`, `polearm_grip = true`):** see MODLOG 2026-10-22 "C6 part
17 addendum". Decision: `guard PASS ... corr builtin|auto ... align <= ~10 deg, clamped 0` = keep and pin the learned
frame; a large `fwd_err=` or `vs_builtin=` = send the log.

## 18. The over-the-shoulder camera (`[third_person]`, 2026-10-22, after the guard alignment test; offline, NOT run in game)

User (12:28-12:32 test): in MW2 mode third person the crosshair (screen centre) sits on the character's head, so enemies
behind it are hard to aim at; wanted an over-the-shoulder camera (right and / or higher).

### 18.1 Design
- **What moves:** only `CSCamera.pers_cam_1` row 3 (the render camera's position), in the existing camera-step detour
  (0x3bc5f0), after first person's `apply` and before the M6 view kick (`apply_view_fx` writes row 3 back as read, so
  the kick keeps the offset). ONE `game::write` of 16 bytes per frame. ER rebuilds the row every frame [V M4]: nothing is
  restored; a frame where the row still holds our last write (ER skipped its step) is offset from ER's saved position
  (`persisted=`), never compounded. `ChrCam.pers_cam`, the `ChrExFollowCam` (auto rotation, +0x240) and
  `CAM_WALL_RECOVERY` are never touched, so ER's own camera collision, auto rotation and lock-on run on ER's unmoved
  camera exactly as without the mod; the follow-cam suppression of first person is not needed.
- **Offset:** `[right, up, back]` metres (`mw2er_core::shoulder::world_offset`): right = the horizontal right of the view
  (`UP x fwd_h`; a camera looking +z has its right at +x), up = world up, back = `-fwd` (pitch included; negative = closer).
  Hip `shoulder_offset_m`, ADS `shoulder_ads_offset_m`, lerped by the MW2 ADS fraction (1 while the C6 guard / crossbow
  aim holds); `shoulder_side = "left"` mirrors the right component. Blend (`ShoulderCtl`): exponential, ~95 % after
  `shoulder_blend_s` (0.15 s) in / out / hip <-> ADS.
- **Spring arm:** the task casts a map ray (`MAP_FILTER`) and an object ray (`OBJECT_FILTER` 0x5b, the filter ER's own
  follow camera uses) from ER's own camera position of this frame (the detour's saved pre-offset position) to the offset
  position, range = length + margin; the offset is scaled to `(hit - shoulder_margin_m) / length` (clamped 0..1), pulled
  in at once, let out with the blend (`step_arm`). One frame of latency (the detour applies the task's latest fraction to
  this frame's rows); the margin covers it.
- **Gates** (`shoulder::gate`, first match): offsets zero / no camera-step hook (cfg), not in the world, a loading /
  cutscene / NPC-talk latch, AC-130 (ride or call-in), nuke, scope peek, first person, MW2 mode off, dead, menu (incl. a
  hidden HUD = site of grace), on Torrent (unless `shoulder_mounted`), camera gate invalid beyond `[first_person]
  cam_gate_grace_ticks`. Actions: cut at once (world, cutscene, AC-130, nuke, first person), hold (scope peek, a camera
  blip: not applied, back at once afterwards), blend out (cfg, mode off, dead, menu, mounted). The detour refuses on its
  own while first person is armed (`ACTIVE`, incl. the peek) or in camera mode 2 (counted `conflicts=`, must stay 0).
- **Shots / HUD:** the camera step runs before the task (M5 1.1), so the task's camera read already is the offset
  camera: the shot is `bullet_origin` on the offset camera's centre ray (at the chest's depth + `origin_ahead_m`) along
  that ray = exactly the crosshair (test `c6_shoulder_shot_stays_on_the_offset_camera_ray`; the chest's off-axis distance
  only moves where on the ray the bullet starts). The HUD projection (`view_pos` / `cam_pos` + `cam_rows`), damage numbers,
  hitmarkers, tracers, the world-gun body occlusion, `er_depth_state::publish_chest` and the coop view all read the same
  shown camera. First person, the scope peek and the AC-130 are unchanged (gated off; the detour refuses).
- **Hook:** the camera-step hook is installed as before (`[first_person] enabled` + `camera_mode = "hook"`, or `[ac130]
  enabled`); without it `CAM shoulder unavailable reason=no_cam_hook`. No hot key for the side (hot via the toml).

### 18.2 Config (`[third_person]`, hot; code defaults = off)
`shoulder_offset_m = [0,0,0]`, `shoulder_ads_offset_m = [0,0,0]` (ranges right +-1.5, up +-1.0, back +-2.0),
`shoulder_side = "right"`, `shoulder_blend_s = 0.15` (0..2), `shoulder_margin_m = 0.20` (0..1), `shoulder_mounted =
false`. Test toml (2026-10-22): `[0.55, 0.25, 0.0]` / `[0.45, 0.15, -0.6]`.

### 18.3 Lines (tag `CAM`)
`shoulder cfg on= off=[..] ads=[..] side= blend= margin= mounted= hook=` (on every change), `shoulder on side= ads=
local=[..] er_cam=[..] fwd=[..]`, `shoulder pull frac= want=..m hit=..m by=map|obj margin=` / `shoulder pull end ms=
min_frac=` (at most one per second; all counted), `shoulder off why=cfg|world|cutscene|ac130|nuke|peek|fp|mode_off|dead|
menu|mounted|camera ms= writes= skips= persisted= conflicts= pulls= min_frac=`, `shoulder unavailable
reason=no_cam_hook`, `shoulder off why=panic` (error; off for the session).

### 18.4 Checks (as built, offline)
Core `shoulder` tests (7, `c6_shoulder_*`: basis, ADS lerp / side, spring arm + margin + pull-in / let-out, blend timing
in / out, cut / hold / gates, cfg, the shot on the offset camera ray); m2_static_checks fpwiring `shoulder_problems` (+8
mutations: written in first person, camera not re-read, after the view kick, ChrCam touched, rows 0-2 written, the task
module writing memory, never ticked, panic path); analyze_c6 row `shoulder` (+3 mutations); check_c6 check 6 (the shipped
toml keeps both offsets `[0, 0, 0]`) and check 9 (+6 strings); tests\config_file.rs (shoulder off in the shipped toml).

### 18.5 In-game test (solo; test toml as 18.2)
1. MW2 mode on (F5) in third person, stand: `CAM shoulder on side=right ...`; the character sits left of the crosshair,
   the camera ~0.55 m right / 0.25 m up. Shoot a wall / an enemy at 5, 20, 50 m: hits on the crosshair.
2. Hold ADS (RMB / LT): the camera moves closer (`-0.6` back) and lower within ~0.15 s; release = back.
3. Walk with the right shoulder along a wall / into a corner / through a doorway: `CAM shoulder pull frac=.. by=map|obj`,
   never a view inside the wall; `pull end` after leaving it.
4. F6 (first person) -> `shoulder off why=fp`, first person identical; F6 back -> `shoulder on`. Sniper ADS (scope peek)
   -> `shoulder off why=peek`, back at the release. F7 AC-130 -> `why=ac130`, ride identical. Lock on to an enemy: ER's
   lock-on camera as before, offset. Open the map / inventory -> `why=menu`; die -> `why=dead`; Torrent -> `why=
   mounted`; F5 off -> `why=mode_off` after a blend.
5. `python -I tools\scripts\analyze_c6.py logs\mw2er.log`: `shoulder PASS N episode(s), writes .., pulls ..`; FAIL / WARN
   = send the log. Taste: tune the offsets in the toml while playing (hot).

## S1. Every new hook, write and read, and its restore

| What | Kind | Where | Restore / safety |
|------|------|-------|------------------|
| Mtx43 entry + Mtx44 chr matrix | read (`game::read`) | present thread, `world_guns_tick` | vtable + ortho + chr checks; fallback stage A |
| havok skeleton names / model pose | read | once per session | ASCII <= 64, count <= 1024; miss = manual index |
| EquipParamWeapon stance fields (5) | write (`game::write`) | main tick (`stance` module) | originals stored per row; restored at mode off / FP / AC-130 / world leave / unload / panic / flag off; foreign bytes never overwritten |
| Y + RB chord / LB held (probe) | synth in the XInput detour buffer | pad.rs | never RB without Y; refused at prompts; released on every exit edge |
| E + LMB chord (no pad) | DirectInput buffer edit | input_hook.rs | same rules; probe-labelled [I4] |
| `arm_style` (probe only) | write (`game::write`) | `stance_refresh = "arm_style"` | saved value restored at exit |
| ER weapon model (part 12.2) | read (`game::read`) | `world_guns_tick`, hand mode + `hold_source = "weapon"` | 4 vtables + owner back-pointers + near the hand; misses fall to the bone |
| SpEffectParam 79001090, 12 fields (part 12.3) | write (`game::write`) | marker tick (4 Hz) | package bytes checked first (foreign never written); restored when not wanted |
| Marker renewal / one re-apply after the patch | SpEffect remove + apply (existing gated wrappers) | marker tick | hand mode / stance only |
| Body bone names / entries (part 13.3) | read (`game::read`) | `world_guns_tick`, `world_gun_body_occlusion` | name lookup once per session; per-entry finite / near-feet / upright checks; a miss adds no capsule |
| Body capsules (part 13.3) | GPU: our own depth only (depth-only PSO, our ring) | Present, before the world guns | no ER resource bound / barriered; third person only; flag off by default |
| EquipParamWeapon weaponCategory / wepType of OUR stanced row (part 14) | write (`game::write`) | main tick (`er_stance::aim_tick`), `tp_ads_pose = "er_aim"` | originals stored, re-read first; restored at the ADS release (once `precision_shooting` is clear, <= 0.4 s) and before the motion fields at every stance restore point; foreign = left alone + stance off |
| ER's aim input: LB synth / RMB in ER's mouse buffer (part 14) | synth in the XInput detour / DirectInput buffer edit | pad.rs / input_hook.rs | only while the episode holds it; never in the gunship / at a prompt (pad); RMB after the raw ADS is recorded; released on every exit edge |
| `chr_flags1c5` bit 1, `CSChrTimeActModule` anim id, camera type (part 14) | read (`game::read`) | main tick | aim confirmation / the read-only `probe_er_aim` lines |
| ER depth chain / desc / wrapper words (part 13.3) | read (`game::read`) + `GetDesc` | chain / words: task thread; `GetDesc`: UiBegin's detour on ER's render thread (review C6 depth, C4.1 rule), `probe_er_depth` (cold) | 6 pins + module check (task) + the frame's own chain re-read and vtable equality (detour) before GetDesc; no UiBegin hook = unmeasured; nothing bound, sampled or written |
| `ID3D12GraphicsCommandList::ResourceBarrier` (part 15) | MinHook on d3d12's slot-26 function, read-only (original first; transitions on the watched `res` recorded into a lock-free table; round 6: every call counted, both candidates A / B, a depth census) + the fold in hudhook's ECL callback | every thread (detour), ECL callers (fold); `probe_er_depth_state` (cold) | no alloc / log / lock / COM call; refused on a foreign prologue / outside the D3D12 module; recording off, then `MH_DisableHook` at teardown (trampoline kept, never `MH_RemoveHook`: 15.7); ER's barriers pass unchanged |
| ER's precision-shooting getter 0x1403F4A90 (part 14.8; 14.11) | MinHook, naked detour: returns 0 only for ER's camera / reticle / snipe-fade call sites (round 7: + 0x14065B3E0), our own PlayerIns, while armed; every other call tail-jumps to the original | every thread that calls the getter (detour); task thread (install, arm) | no store at all (only our own hit counters); 2.7.2.0 only, bytes / prologues / both `E8` sites (round 7: + the fade writer's 33 bytes, its `E8`, both timer stores) validated before the hook, the DLL pinned before enable; disarmed first at every aim restore point, never removed (a passthrough); with keep, refused = no aim episode |
| `PlayerIns+0x5D8` snipe fade, all 10 `anim_queue` ids, ChrAsm bolt ids 7 / 9 / 11, ER's weapon model frame (part 14.11) | read (`game::read`) | main tick, `probe_er_aim` | read-only probe lines (`snipe=`, `HOLD aim pose`); `+0x5D8` never written |
| ER scene depth read (part 15) | GPU: our SRV on `res` plane 0 in our compute list, spliced at `at == 0` of the armed UiBegin ECL | ER's render thread; `probe_er_depth_sample` / `world_gun_er_depth` (cold, live only on the verdict) | only when the fold proves a readable state at that queue point for that frame; no barrier on or write to `res`; any trip = capsules only for the session |

| ER's `EquipGameData::EquipItem` (+ `Find`; `Create` / `AddItem` only with `carrier_give`) on the main player (part 16) | engine call | task thread (`ChrIns_PostPhysics`), `er_carrier = "crossbow"` | prologues pinned (2.7.2.0 only); journal in `<root>\state` written first; restored by the same call at mode off / FP / AC-130 / nuke / world leave / flag off / detach / next world enter; foreign gear never touched; given items stay in the mod save |
| `arm_style` of PlayerIns ChrAsm after the carrier swap (part 16, P-X2) | write (`game::write`) | task thread | the saved grip restored with the original weapon |
| Equipped bolt quantity, `bolt_loaded_states[s]` (part 16) | read (`game::read`) | task thread | a decrease trips the carrier off + restore |
| `bolt_loaded_states[s]` = 1 of the carrier slot in EquipGameData's + PlayerIns's ChrAsm (part 16.16, `carrier_load`) | write (`game::write`, u8) | task thread, carrier tick | only on a verified, settled two-handed carrier with no chord / busy ER and bolts equipped, the slot holding our crossbow on both ChrAsms; back to 0 first at every exit / restore / grip change / chord window (and the restore's EquipItem clears it); no bolt used |

| EquipParamWeapon stance fields (5) with the polearm set (part 17, `er_stance = "polearm"`) | write (`game::write`) | main tick (`stance` module) | as the crossbow set; the set stored per row, the restore re-reads against it; a hot set change restores first |
| ER's block input: LB synth / RMB in ER's mouse buffer (part 17, `tp_ads_pose = "guard"`) | synth in the XInput detour / DirectInput buffer edit (the aim pose's routes) | pad.rs / input_hook.rs | only while the guard episode holds it; never in the gunship / at a prompt (pad); released on every exit edge; nothing written on the row |
| `CSChrPhysicsModule.orientation` + `interpolated_orientation` (part 17, `tp_guard_yaw`) | write (`game::write`, 32 bytes) | main tick, inside a guard episode, standing | ER's own facing it rewrites every frame (as the M3B FP yaw follow): no restore; gated (not locked on, turning allowed, no throw, alive) |
| ER's weapon model frame (part 17) | read (`game::read`) | main tick, with the guard flag | the carry / guard detection only |
| `CSCamera.pers_cam_1` row 3 (part 18, `[third_person]` shoulder offset) | write (`game::write`, 16 bytes) | camera-step detour (`apply_shoulder`), after first person, before the view kick | ER rebuilds it every frame: no restore; refused while first person / the gunship own the camera; camera re-read every frame; ChrCam / follow cam never touched |

## S2. Verified vs inferred (summary)

Verified: no dedicated hidden weapon; crossbow motion fields and offsets; row rounding to 100; ChrAsm fields; the
C3 probe's missing hop; both entity layouts and vtables on 2.7.2.0; the render matrix at ChrCtrl+0x2F8; pad synth
needs a connected pad; stage A matrix det -1.
Inferred (probed): I1 bank re-resolve on grip change, I2 no crossbow logic without category 11 / type 55, I3 both
hands hidden, I4 chord buttons, I5 model space, I6 R_Weapon animates while hidden, I7 peers use their own row copy,
I8 aim button / camera, I9 skeleton name layout, I10 unit scale.
