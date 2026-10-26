# C3 design: playability, controller, co-op visibility and co-op nuke

Status: design only (2026-10-19). Nothing in this document has run in game. It answers the four feedback items from
the first two-player session (C2: `logs\coop\host_c2_2026-10-19.log`, host, lobby 3; `logs\coop\friend_c2_2026-10-19.log`,
client, lobby 6; **friend clock = host clock + 1 h**).

Sources: the four C3 research reports (traversal, UI and damage numbers, co-op visibility, controller). Scan
outputs are in `work\c3\erscan\` (layer table: `emu2.py` and `layers.json` only; `emu.py`, `layer_table.txt` and
`query_vs_layer3.txt` are WRONG and must not be used). Earlier designs this builds on: M3/M3B (viewmodel and first
person), M4 (AC-130), M5/M8 (movement, mantle, run-18 ray filters, section 18), M6 (arsenal), M7 (nuke), COOP_PLAN.

## Evidence labels

- **[V]** verified: from bytes in eldenring.exe 2.7.1.0 / iw4sp.exe, from our code, from param XML or from the C2 logs.
- **[I]** inferred: a reading of verified facts that nothing has proven yet. Every [I] that the code depends on gets a
  log line so the next run can confirm it or prove it wrong.

## Hard rules (apply to every pass)

- Never launch Elden Ring, MW2, me3, Steam or Seamless. Never write into a game folder or `%APPDATA%\EldenRing`.
  eldenring.exe, xinput1_4.dll and iw4sp.exe are only read as files (erscan, Python `-I`).
- Never push, publish or upload. `dist\MW2inER-coop-friend.zip` stays local; the user sends it.
- Never read or print the Seamless password; never copy `coop\*`. Never browse Downloads / Documents / Pictures.
- Downloads only via cargo / crates.io. Do not touch `D:\Mods\MW2inER-m3` or `D:\Mods\MW2inER-r17`.
- Every runtime write to game memory goes through `game::write`. Edits of a caller's buffer inside an OS-API detour
  (DirectInput today, XInput new) are the existing `input_hook.rs` precedent and are not game-memory writes.
- Network sync only through ER's own replication (bit-3 bullets, synced SpEffects). No sockets, no servers.
- PATH: `$env:Path = [Environment]::GetEnvironmentVariable('Path','User') + ';' + [Environment]::GetEnvironmentVariable('Path','Machine')`.
  Python: `%LOCALAPPDATA%\Programs\Python\Python314\python.exe -I`.

## 0. Decisions at a glance and implementation order

| # | Item | Decision | Label |
|---|------|----------|-------|
| 1a | Invisible walls | New ray mode `player`: cast with the player's own proxy filter (live `CSChrPhysicsModule+0x338`, layer only, accepted for layers 2/3/4, fallback 0x3). One cast per ray. `union`/`map`/`object` stay for hot rollback | table [V], player layer [I] |
| 1b | Traversal edge cases | `MOVE assist` hand-back: a short ER-owned window when W/A/S/D is held and no progress; plus slope/step benches | [I] |
| 1c | FP stuck off | Talk latch releases 2 s after the menu cursor closes; F6 during a non-user hold forces FP back on; fade stale 3 s | [V] root cause |
| 2a | Messages behind the gun | Ship "UI yield" (gun drops, our HUD dims while ER shows a message). Scaleform injection: probe only (off by default), decide in C4 | yield [I], hook point [V] |
| 2b | Damage numbers | Floating numbers at the hit, projected each frame, rise and fade, merge, kill / head-guess colours | [I] |
| 4 | Controller | MinHook detour on `xinput1_4!XInputGetState` (system DLL); full MW2 mapping; right stick feeds ER's mouse path through MW2's graph and turn rates; left stick is analog pmove | import [V], route [I] |
| 3a | Peers' shots | Damaging bullets stay local (bit 3 off). A no-attack "visual twin" row with bit 3 on is spawned beside each shot; peers' DLLs read twins from the bullet list and add tracers, MW2 npc sounds and explosions | send/receive path [V], twin damage-free [V data] |
| 3b | Held gun | Our renderer draws `weapon_*.glb` world models on every player (self in third person), anchored to the model matrix (stage A) then the hand bone (stage B, probe); synced marker SpEffects carry gun id and hide ER's weapon | [I] |
| 3c | Locomotion | `fp_anim = "hybrid"`: W/A/S/D (and the left stick) reach ER in first person, our position write discards ER's root motion, as third person already does | mechanism [V], effect [I] |
| 3d | Co-op nuke | Synced marker (SpEffect 79001190, fallback bit-3 marker bullet 79000963). Every peer runs the sequence and kills the enemies it owns (`chr_update_type == Local`) for the whole window, then its own player | [I], ownership evidence [V] |

**Order** (each pass ends with `check_c3.ps1` PASS, a staged DLL and, if the pass is played together, a rebuilt
friend package):

- **PASS A = items 1 + 2** (playability). Solo-testable. No regulation change.
- **PASS B = item 4** (controller). Solo-testable. No regulation change.
- **PASS C = item 3 + co-op nuke** (co-op visibility). Regulation rows, converted assets and the friend package
  all change; needs both PCs.

The regulation package sha256 (9a33f2524237...) stays unchanged through PASS A and PASS B and changes once in PASS C.

---

# PART 1 (PASS A): traversal and first-person stuck off

## 1.1 What is verified

- Filter word layout [V]: `layer (bits 0-6) | type (7-8) | group (9-24) | subsystem (25-28)`; bit 29 cleared; bit 30
  disables all. The table test is a 128x128 bitset at `CSCollisionFilter+0x20`, 16 bytes per row; the pair collides
  when bit `layerB` is set in row `layerA`, and not when both type fields are non-zero and equal.
- Static default table (`layers.json`, from `emu2.py`) [V]:
  - 0x58 (map filter 0x2000058) hits `1d 1e 1f 20 23 24 27 28 33 37 38 3a 3b 3c 3d 3f 40 41 42 44 45 46 47 49 4a 4d 52 53 54`.
  - 0x5b (camera filter) hits `1e 20 22 24 26 28 2a 30 33 38 39 3a 3b 3d 3e 3f 40 42 43 44 45 52 54`.
  - Layer 3 hits chars `2 3 4 e f 10`, terrain `1f..2a`, `31 35 38 39 3a 3d 3e 3f 42 43 44 47 48 49 4b 52 54 69`.
  - The union of 0x58 and 0x5b misses player-solid `21 25 29 31 35 48 4b 69` and hits query-only layers
    `1d 1e 30 33 37 3b 3c 40 41 45 46 4a 4d 53` that no character layer collides with.
- Player proxy filter location [V]: `ChrIns+0x190` -> modules, `+0x68` = CSChrPhysicsModule; setter 0x14045ff10 writes
  `+0x338` (main CsChrProxy), `+0x348` (second proxy), `+0xC8` (a u32 filter word; the vendor crate's `unkc8: f32` is
  wrong). The proxy init copies +0x338 into hknpCharacterProxyCinfo `m_collisionFilterInfo` (+0x88). The init struct
  zeroes the base, so the value only exists at run time.
- Blocked episodes cluster on slopes of 17-45 deg that ER walks; F5 was used to climb dy +0.8..+2.3 m [V logs].
- FP stuck off = the talk latch [V]: `CSPopupMenu.current_talk_id` is a last-talk id and never returns to the
  baseline, so every talk keeps FP off until `stale_s` (20 s) after the value settles (measured 20.5-107.5 s). Movement
  re-arms after `TALK_HOLD_S` 1.5 s (movement.rs:72), so the player runs MW2 mode in third person. F6 pressed during
  the latch flips `wanted` to 0 and keeps FP off afterwards (`FP blocked reason=f6`).

## 1.2 What is inferred

- The player is layer 3 (2 and 4 are siblings). Read live and logged; accepted only for 2/3/4.
- The layers 1d/1e/30/33/37/3b/3c/40/41/45/46/4a/4d/53 are the invisible walls (camera, sight and ray-only volumes,
  debris). What they are made of in the world is not verified.
- `CSHavokMan+0xC0` -> CSCollisionFilter (live table dump, 1.6). If the read fails, the line logs `skip` and nothing
  else depends on it.

## 1.3 Ray mode `player`

`mw2er-core::config::RaysCfgFile` gains:

```toml
[movement.rays]
mode            = "player"   # NEW default: player | union | map | object (hot; union/map/object = run-18 rollback)
player_fallback = 0x3        # cast filter when the live word is unusable
player_layers   = [2, 3, 4]  # accepted layers of the live physics+0x338 word
map_filter      = 0x2000058  # unchanged (used by map / union)
object_filter   = 0x5b       # unchanged (used by object / union)
body_probes     = true
log_live_table  = true       # one-shot live filter-table dump at MOVE on (1.6)
```

- `filters()` for `player` returns `([player_cast, 0], 1)`. `player_cast` is set by the task: on every `MOVE on` and
  every 5 s while driving, read `physics+0x338` (non-faulting read, never written). If `(v & 0x7F)` is in
  `player_layers`, `player_cast = v & 0x7F` (**layer only**: the type and group bits would disable pairs with bodies
  of the same non-zero type), else `player_fallback`.
- Validation: `mode` in the four names; `player_fallback & 0x7F` in 1..0x7F with bits 7-31 zero; `player_layers`
  non-empty, each 0..0x7F; `map_filter != 0` as today.
- Owner exclusion stays, so the own capsule is never hit. Layer-3 queries also hit the peer's proxy (layers 2/3/4)
  and Torrent / spirits (e, f, 10). This is what ER's own proxy collides with, so it is accepted; NPC layers 5/6 are
  not hit. Logged in co-op as part of the blocked-episode line.
- Zero-rebuild check before PASS A ships (user-side, optional): hot reload `mode = "object"`, `object_filter = 0x3`.

## 1.4 `MOVE assist` hand-back

Purpose: let ER's own proxy handle what our controller cannot (steep slopes, roots, small rocks), the way the user
used F5, without the user pressing anything.

Pure logic in `mw2er-core::move_logic::Assist` (tests 1.9):

- Trigger: driving, grounded, W/A/S/D (or the left stick, PASS B) held with wish >= 100, `speed_world < stall_speed`
  for `>= stall_s`; or immediately after a mantle refusal `no_face | path_blocked | edge` while the move key is
  still held (`after_refusal`).
- Action: the existing suspend path with `reason=assist` for `hold_s`; ER walks with W/A/S/D (blanking lifted for the
  window, FP stays on, the cam hook keeps following the character); then `MOVE resync why=handback` and `MOVE on`.
- No inside-evidence requirement and no 2.5 s window (unlike unstick). Cap `max_per_min`; a capped trigger logs
  `WARN assist capped`. Repeats within 3 s double `hold_s` up to 2.0 s.
- Never during a mantle, a ladder, the AC-130, the nuke, a menu or in the air.

```toml
[movement.assist]
enabled        = true
stall_speed    = 0.15   # m/s
stall_s        = 0.4
hold_s         = 0.8    # 0.3..2.0
max_per_min    = 8
after_refusal  = true
```

## 1.5 Slope, step and mantle re-tune

- Benches first: movebench gains b21 (walk up a 20 deg / 30 deg / 40 deg ramp), b22 (step over a 0.30 m root and a
  0.45 m rock), b23 (a ramp built from a "layer 0x21" body that the map filter cannot see: must block under `map`,
  must be walkable/collidable under `player`), b24 (assist trigger and cap). Pass: speed on 20/30 deg ramps >= 90% of
  flat, 40 deg >= 70%; 0.30 m root stepped without a stall; 0.45 m rock = step or mantle, never a stall > 0.4 s.
- If the benches show our slope limit below ER's, set pmove's walkable normal from MW2's (`MIN_WALK_NORMAL` 0.7,
  45.6 deg) and log it in `MOVE cfg`. The C2 stalls at 17-30 deg are expected to come mostly from the wrong filter
  (union hits query volumes; map misses 21/25/29 terrain) [I].
- Mantle: `no_face` (about 57/min in C2) is expected to fall with the player filter because faces on layers 21/25/29
  become visible. No numeric re-tune in PASS A beyond the filter; the analyzer row (1.8) records the refusal rate per
  minute and the next run decides.

## 1.6 Diagnostics (read-only)

- At `MOVE on` (first one per session, and on `mode` change): `MOVE filter live phys338=0x.. phys348=0x.. physc8=0x..
  layer=3 cast=0x3 src=live|fallback`.
- If `log_live_table`: read `CSHavokMan+0xC0` -> filter -> `+0x20`, 16-byte rows for 0x58, 0x5b and the cast layer.
  Compare against the rows embedded from `layers.json` at build time:
  `MOVE filter table row=0x03 live=<32 hex> static=match|DIFF` (three lines, once). A `DIFF` downgrades 1.1's table to
  "init default only" and is reported by the analyzer as WARN, not FAIL.
- Blocked-episode line (new, rate-limited 1/s): `MOVE blocked wish=.. speed=.. slope_er=..deg mode=player cast=0x3
  hits=map:a/obj:b/both:c dt=..s` (hit body layer is not available yet: the cast collector's body id is not located).

## 1.7 First person: talk latch, F6 and fade

- **Talk latch** (first_person.rs ~:412-435): talk gets its own `LatchParams { holdoff_s, stale_s: holdoff_s.max(2.0) }`
  like `lp_load`, so it releases once `talk_id` has been stable for 2 s **with the menu cursor closed**. On release,
  re-baseline `talk_edge` to the current value (so the same id does not fire again). Optional gate:
  `talk_menu_idx = 3` — while `ui_states[3]` is open the latch holds regardless of the timer.
- **Movement follows FP**: `TALK_HOLD_S` stays as the minimum, but movement's `Reason::Talk` also holds while FP
  publishes `TALK_LATCHED` (new atomic in `shared`). No more "MW2 in third person" after a talk.
- **F6** (`handle_f6`, :282-293): if FP is off for a non-user reason (any latch, cutscene, talk, menu, load, fade,
  cam_gate) and `wanted == 1`, F6 force-releases all edge latches, re-baselines the edges, keeps `wanted = 1` and
  toasts "First person ON"; log `FP toggle F6 override reason=<r> released=<latches>`. F6 toggles off only when FP is
  On, or when `wanted` is already 0. Real menus still block (the override does not open FP over an open menu; it
  clears the latch so FP returns when the menu closes).
- **Fade**: `[first_person] fade_stale_s = 3.0` (new, hot; was the general 20 s) once in-world with the cursor
  closed. Fixes the friend's 43 s case (plate 2 stuck at a=0.53).

Config: `[first_person] talk_stale_s = 2.0`, `talk_menu_idx = 3` (-1 = off), `fade_stale_s = 3.0`,
`f6_overrides_latches = true`.

## 1.8 Log lines and analyzer rows (part 1)

Lines: `MOVE cfg ... rays=player(cast=0x3)` | `MOVE filter live ...` | `MOVE filter table ...` | `MOVE blocked ...` |
`MOVE assist n=<k> why=stall|refusal:<r> hold=<s>s speed=<v> slope_er=<d>` | `MOVE suspend reason=assist` |
`WARN assist capped` | `FP latch released signal=talk_id why=closed after <s>s` | `FP toggle F6 override ...`.

New `tools\scripts\analyze_c3.py` (M7/M8 pattern, `C3_ROWS`, imported by `analyze_log.py` like `analyze_m8`):

| Row | PASS | FAIL | N/A |
|-----|------|------|-----|
| `ray filter` | `MOVE cfg` shows `rays=player`, `MOVE filter live` layer in {2,3,4} or `src=fallback` with WARN | mode union/map/object without a rollback note, or cast=0 | no MOVE lines |
| `filter table` | 3 `match` lines | (WARN only on DIFF) | `skip` / log_live_table off |
| `traversal` | blocked episodes (wish>=100, speed<0.15 m/s, >=0.4 s, no assist within 0.6 s) <= 2 per 10 driving minutes; F5 hand-backs used to climb (off->on < 10 s, dy > 0.5 m) <= 1 per 10 min | above | no driving |
| `assist` | each `MOVE assist` followed by `MOVE on` within hold+1.5 s; no `capped` more than 1/10 min | missing resync / cap storm | no assist lines |
| `mantle refusals` | `no_face` <= 15/min of driving | above | no mantle lines |
| `fp talk latch` | every `latch set signal=talk_id` released within 6 s after the dialog's `MENU idx=3 1->0` (or 6 s after the latch with no menu) | longer | no talk |
| `fp f6` | no `FP blocked reason=f6` within 30 s after an F6 pressed while FP was off for a non-user reason | present | no F6 |

**Known-bad fixtures**: the analyzer self-test replays `logs\coop\host_c2_2026-10-19.log` and must FAIL `fp talk
latch` (47.4 / 97.7 / 107.5 s) and `fp f6` (15:04:01.6); this proves the rows see the bug. (Read-only; the logs stay
where they are.)

## 1.9 Offline oracles (part 1)

`cargo test -p mw2er-core`, 0 failed, named tests by prefix:

- `c3_rays_` : `player` mode returns one filter; layer-only masking (`0x2000003 -> 0x3`, `0x183 -> 0x3`); unaccepted
  layer 5 -> fallback; validation rejects `player_fallback = 0x80`, empty `player_layers`; `union`/`map`/`object`
  unchanged (run-18 tests still pass).
- `c3_layers_` : the embedded rows for 0x58 / 0x5b / 3 equal the quoted lists in 1.1; `union` misses exactly
  `21 25 29 31 35 48 4b 69`; mutation (drop the helper branches = emu.py behaviour) must fail.
- `c3_assist_` : stall timer, refusal trigger, cap per minute, doubling, never in air / mantle / menu.
- `c3_fp_talk_` : latch released 2 s after cursor close; held while `talk_menu_idx` open; re-baseline stops refire;
  replay of the host's talk sequence (-1 -> 1000 -> 3000 -> 1000 -> 204101000 -> 1000 -> 302001000) releases each
  within 2.5 s of close. Mutation: `stale_s = 20` must fail.
- `c3_fp_f6_` : F6 during a talk / fade / cutscene latch keeps wanted = 1 and releases; F6 while On toggles off;
  F6 while wanted = 0 toggles on.
- movebench b21-b24 at 30/60/144 Hz (1.5).

---

# PART 2 (PASS A): ER messages above the gun, damage numbers

## 2.1 What is verified

- Everything we draw (thermal, viewmodel, scope, nuke, tracers, ImGui HUD) runs in hudhook's pre-ImGui callback at
  Present, after ER's finished frame (Scaleform UI included) is in the back buffer. So it is always above ER's UI.
- ER's UI is Scaleform GFx with a D3D12 HAL: BeginScene 0x14115e6b0 (slot 59 / +0x1d8, prologue
  `40 57 48 83 ec 20 8b 41 58 48 8b f9 a8 02 75 08`), EndScene 0x14115e810 (slot 60, prologue `41 55 41 56 48 83 ec 48`),
  BeginFrame 0x14115e360, PushRenderTarget 0x14115d710. D3D12 HAL vtable 0x142c09378, base 0x142cc3018.
- UI signals in the vendored fromsoftware-rs (@59fbd3b): CSFeManImp `hud_state` +0x78 (PopupMenu = 2), proc-status
  message ring, `proc_status_message_timer`, full-screen / blinking / area-welcome / subarea-name ids, summon message
  queue; CSMenuManImp `ui_states` +0x90 (0x46 bytes), `popup_menu` +0x80 -> `popup_messages` deque (size at deque+0x28),
  `system_announce_view_model` +0x860 -> view `is_visible` / `announce_play_state`.

## 2.2 Decision: yield now, inject later

Injecting our draws before Scaleform needs the UI command list, its target and proof that ER re-binds descriptor
heaps after us; none of it is knowable statically, and a mistake is a device-removed crash mid co-op [I]. C3 ships:

1. **UI yield** (default on): while ER shows a message, the gun slides down (render-only) and our HUD text dims.
2. **Scaleform probe** (default off): read-only detours that log what an injection needs; the decision moves to C4.

## 2.3 UI yield

Pure core `mw2er-core::ui_yield::UiYield`:

- `update(now, signals: u32, fire_or_ads: bool) -> f32` frac 0..1. Ease in 0.12 s, out 0.25 s, hold >= 0.4 s after the
  last signal. Fire or ADS input cancels (frac -> 0 in 0.08 s); re-yield only after 0.6 s without a shot.

Task (`task.rs`, every tick, non-faulting reads only, `offset_of!` + const asserts on the vendored structs):

- Signal bits: `popup` (popup deque size > 0 or hud_state == 2), `announce` (view visible or play state != 0),
  `status` (ring read != write, or timer < 3.0), `summon` (queue non-empty), `blink` (blinking / full-screen /
  area-welcome / subarea ids set), `idx<n>` (configured `ui_states` indices open).
- Never iterate the deque's map pointers; read `size` only.
- Publishes `snap.ui_yield` (frac) and `snap.ui_yield_why` (bitmask); logs transitions.

Render:

- Viewmodel: `frac * drop_in` added to `snap.vm_offset_in` (z down, x back) on the existing `ProjParams.offset_in`
  path. **Firing is never blocked** and the mantle `wgate` path is untouched. `mode = "hide"` returns early from
  `callback_body` when frac > 0.95 (still never blocks firing).
- HUD (`hud.rs`, same single background draw list): while frac > 0 the switch list (h*0.80), lock text (h*0.62),
  nuke hints (h*0.30-0.38) and the toast fade to `hud_dim_alpha`; crosshair, hitmarker and ammo stay.
- The toast moves from (24,24) (over ER's HP/FP gauges [I]) to `toast_anchor` (default left, y = 0.30h).
- World guns and peer effects (PASS C) also scale alpha by `1 - 0.65*frac`.

```toml
[ui_yield]
enabled      = true
mode         = "lower"        # lower | hide | off
drop_in      = [0.0, -9.0, -2.0]  # x, z(down), back, inches at frac 1
hud_dim_alpha = 0.35
indices      = [15, 24, 66]   # ui_states candidates [I] (banner / timed notification / co-op message)
use_popup = true
use_announce = true
use_status = true
use_summon = true
use_blink = true
learn        = false          # 60 s per-index duration statistics (2.5)
toast_anchor = [0.012, 0.30]  # fraction of w, h
```

### 2.3.1 Cutout (C5, `mode = "cutout"`; the C4 injection was refuted by its probe)

- The gun keeps its place. Each signal has its own regions (`cutout_popup` / `_announce` / `_status` / `_summon` /
  `_blink` / `_idx`: [x0, y0, x1, y1] screen fractions, <= 4 each) and its own 0..1 envelope (`UiYield` bit_frac:
  same ease in / hold / ease out); region strength = min(frac, bit envelope), so fire / ADS still cancel it.
  `ui_yield::cutout_rects` keeps the 4 strongest; the task publishes them (`snap.ui_cut`).
- Render: `Renderer::set_cutout` every frame; FrameConsts words 46 (`cut_count`) and 48..63 (`cut_rect[4]`: packed
  pixel rect, strength, feather px). viewmodel.hlsl (cutout.hlsli): `cut_count == 0` is the old path; otherwise
  inside a region the draw keeps 1 - strength, ramping back to 1 over the feather; opaque draws use a 4x4 ordered
  dither discard (their PSO has no blending, unchanged), alpha draws multiply alpha. World guns are cut, not hidden.
  The scope overlay is not cut (ADS cancels the yield); the C4 early path never applies it.
- Defaults [I] (ER 16:9 layout from memory): popup centre, announce / summon top-centre band, status lower centre,
  blink centre band, idx = interaction prompt box + subtitle band. `UI cutout on signal=.. regions=..` (<= 1/s).
- vmtest `--cutout` CU0-CU5 (WARP): NONE / strength 0 bit-identical, inside untouched, outside identical, feather
  band thinning, strength 0.5 ~half, R10G10B10A2 PQ.

## 2.4 Scaleform probe (off by default, PASS A, read-only + logging)

`[ui] probe_scaleform = false`. When true at boot (cold):

- MinHook detours on BeginScene 0x14115e6b0 and EndScene 0x14115e810, each installed only if the 16 / 8 prologue bytes
  match (cam_hook.rs pattern); mismatch -> `UI probe skip why=prologue fn=.. got=..`.
- Detours count, read `this` and HAL flags `+0x58`, then call the original. No writes.
- In BeginScene, a non-faulting scan of `this[0..0x800]` and one pointer level for a qword whose first qword equals
  the vtable of hudhook's own `ID3D12GraphicsCommandList`; log offset and pointer once per 5 s.
- hudhook patch `0002` (ExecuteCommandLists callback, read-only): per-frame ECL count, list counts, whether the list
  found above is submitted and where relative to Present.
- Lines: `UI probe scenes/s=.. thread=present|other this=0x.. flags=0x.. cl_off=0x..|none`, `UI probe ecl n=.. ui_list_in=k pos=before_present|..`.
- Restore: hooks disabled and removed at DLL unload; nothing written in game memory.

## 2.5 Learn mode

`learn = true` logs for 60 s per open `ui_states` index: `UI learn idx=<n> open=<k> med=<s> max=<s> with=<signal bits>`.
The user does one guided walk (pick up an item, approach an item, rest at a grace, get a co-op message) so the
`indices` list can be confirmed. Labels for 15/17/19/24/66 are [I] until then.

## 2.6 Damage numbers

Data path:

- `task.rs::poll_hits` folds its five hit sites (gun Hit, launcher in Hit, AC-130 in Hit, launcher in HpDrop, AC-130 in
  HpDrop) into one `on_hit(now, handle, chr_pos, dmg, kill, src, impact)` that keeps today's counters and pushes a
  `DmgNum { t, handle, anchor, dmg, kill, head_guess, src }` into a `VecDeque` capped at `damage_numbers_max`,
  published in `HudSnapshot`.
- `hit_logic::resolve` adds `impact_pos: Option<[f32;3]>` (last owned impact) to `HitEvent::Hit`.
- Anchor: impact position, else `chr_pos + (0, capsule_height_m*0.9, 0)` (ER is Y-up [V]).
- Merge: same handle within `merge_s` adds dmg and restarts the timer (bursts, pellets).
- Head guess [I]: `impact.y - chr_pos.y >= 0.82 * capsule_height_m`.
- Value: ER HP damage (`hp_before - hp_after`); `value = "mw2"` uses the shot's MW2 damage.
- Co-op: only our own attributed hits (HIT lines). HPDROP and peer drops never get numbers (see the peer-drop guard,
  3.7).
- Re-base: on `MOVE resync why=rebase` shift anchors by the re-base vector; on world leave clear the queue.

Drawing (`hud.rs`, after the hitmarker, same gate as the crosshair: mode on, in world, no menu, not AC-130):

- Projection with `ac130_hud::project(pos, rows, vfov, w, h, p)`; rows = CameraSample rows; position =
  `snap.view_pos = fp_eye.unwrap_or(cam.pos)` (FP eye in first person, ER's camera in third person).
- Age a, life L: world anchor + (0, 0.35*a/L m, 0), screen rise 18*s1080*a/L px, alpha 1 until 0.45 L then linear to 0,
  scale pop 1.3 -> 1.0 over 0.1 s with a second, larger ImGui font loaded at init.
- Colours: white, yellow for head_guess, red with a trailing `x` for a kill; 1-px dark shadow. Skip off-screen /
  behind camera. Dimmed by ui_yield like other HUD text. Optional suppression while ER's own enemy tag shows the
  same handle (`suppress_with_er_tag`, reads `CSFeManImp.enemy_chr_tag_displays[].field_ins_handle`).

```toml
[hud]
damage_numbers             = true
damage_numbers_s           = 0.9
damage_numbers_merge_s     = 0.25
damage_numbers_value       = "er"     # er | mw2
damage_numbers_head_guess  = true
damage_numbers_max         = 24
damage_numbers_scale       = 1.0
damage_numbers_suppress_with_er_tag = false
```

Log: `DMGNUM n=<live> handle=c.. dmg=.. kill=0|1 head=0|1 via=impact|chr` at debug level (one per pushed number,
merged ones log `merge=1`); `HUD dmgnum cfg on=1 life=0.9 ...` at boot/reload.

## 2.7 Analyzer rows and oracles (part 2)

Rows: `ui yield` (each `UI yield on` has a matching `off`; none longer than 30 s unless a menu was open; during a yield
no `SHOT` was blocked), `ui probe` (N/A unless enabled; PASS = both hooks installed and >= 1 `UI probe` line), `damage
numbers` (count of `DMGNUM` (merge counted) == count of `HIT` lines while `damage_numbers = true`, +-2%; 0 when false).

Tests:

- `c3_uiyield_` : ease in/out, hold, fire cancel and 0.6 s re-yield, mask & index bits; `hide` threshold.
- `c3_dmgnum_` : merge within 0.25 s, life/fade curve, rebase shift, head_guess threshold, cap 24, kill colour.
- `c3_project_` : `project()` round trip with FP-eye and TP-camera fixtures (a point 5 m in front lands in the screen
  centre; behind -> None).
- `c3_hitlogic_` : `impact_pos` set on owned impacts, unchanged counters (all M6/M7 hit tests still pass).
- `c3_offsets_` : const asserts compile (`hud_state` +0x78, `ui_states` +0x90, `popup_menu` +0x80, announce +0x860).
- Static: the two Scaleform prologues are byte-checked in eldenring.exe by `c3_static.py` (2.8 check 2).

---

# PART 4 (PASS B): controller

## 4.1 What is verified

- eldenring.exe imports `XINPUT1_4.dll` ordinals 2 (XInputGetState) and 3 (XInputSetState), `DINPUT8!DirectInput8Create`
  and five SetupDi functions. No ISteamInput / SteamController interface strings, no RawInput / GameInput /
  Windows.Gaming.Input. libScePad is loaded dynamically but not shipped.
- MW2 console look values from iw4sp.exe: yaw 260, pitch 90, ADS yaw 90, ADS pitch 55 deg/s; graph index 3; accel
  lerp 1200; graph `view_input_3.graph` = (0,0) (.1242,.0423) (.2238,.0885) (.3210,.1553) (.4118,.2419)
  (.5116,.3508) (1,1).
- pmove's `Cmd` is analog (f32, +-127); wishspeed scales by max(|fm|,|rm|)/127; sprint needs forwardmove > 105.

## 4.2 Hook

- MinHook detour on `xinput1_4!XInputGetState` (GetModuleHandleW + GetProcAddress), hudhook's MinHook, installed like
  `input_hook::install`. Own `XINPUT_STATE` struct (16 bytes). **No IAT patch** of eldenring.exe (Arxan), no writes
  to `PadDevice` in ER's heap.
- In the detour: call the original; act only on return 0 and the selected slot (`pad_index`, -1 = first slot with
  input). Record raw levels and press edges into atomics (`PAD_BUTTONS`, `PAD_PRESSES[n]`, sticks and triggers as f32
  bits, QPC time). Then blank, using `input_logic::{effective_want, latch_next, press_edge}`: while MW2 mode drives and
  the heartbeat is fresh, clear bound button bits, zero bound triggers and the stick pairs; per-button release latch.
  Gunship (`WANT_GUNSHIP_INPUT`): everything zero except START.
- Synthesis: X held past `hold_ms` ORs `XINPUT_GAMEPAD_A` (0x1000) so ER interacts (talk, graces, doors, pickups),
  extended like `interact_hold_s`.
- No allocation, lock, logging or panic in the detour; `catch_unwind` around the body.
- Install log: `PAD hook ok xinput1_4=0x.. first_byte=0xE9|0x48.. overlay=GameOverlayRenderer64:yes|no`.
- Restore: when MW2 mode is off, a menu is open, the window is unfocused or the heartbeat is stale, the detour passes
  state through unchanged (all ER controls back). Hooks removed at unload.

## 4.3 Mapping (defaults; covers PlayStation pads through Steam Input's XInput emulation)

| Pad | MW2 mode | Gunship |
|-----|----------|---------|
| RT | fire (> 30/255) | fire |
| LT | ADS | - |
| X | tap reload; hold 0.3 s = ER interact (synth A) | thermal |
| Y | next weapon | next gun |
| LB | previous weapon | - |
| RB | unbound, hidden from ER | - |
| A | jump / mantle | - |
| B | tap crouch, hold 300 ms prone | - |
| L3 | sprint (MW2 press mode) | - |
| R3 | melee (reserved, hidden) | - |
| D-pad up / down | AC-130 / nuke | - |
| D-pad left | first-person toggle (F6) | - |
| D-pad right | passes to ER | - |
| L3+R3 (same read) | MW2/ER mode toggle (F5) | - |
| Start | always ER | always ER |
| View/Back | ER map | exit gunship |
| Right stick | MW2 look (4.4) | gunship aim |
| Left stick | MW2 analog move (4.5) | - |

## 4.4 Right stick

Pure `mw2er-core::pad_logic`:

1. Radial deadzone: m = |v|/32767; 0 below `rs_deadzone` (0.18), rescaled to `rs_outer` (0.98).
2. Graph `input_graph` (default 3, embedded from `view_input_N.graph` at build time, source path recorded), linear
   between points.
3. Rates: yaw = gx * (ads ? 90 : 260) * look_sensitivity; pitch = gy * (ads ? 55 : 90) * look_sensitivity; `invert_y`.
4. Accel [I formula]: while |gx| >= 0.99, yaw rate ramps toward `accel_max_yaw` at `accel_lerp` deg/s^2, decays on
   release.
5. Route `mouse` (default): the DirectInput mouse detour adds `round(rate * dt / deg_per_count)` to lX/lY **after**
   the existing ADS `SENS_SCALE` step (ADS is not scaled twice), with a 1/1024 carry; dt = QPC since the previous
   mouse read, clamped 50 ms. ER's camera turns from it in first and third person; the cam hook moves only position
   rows. `deg_per_count`: `mouse_deg_per_count` if > 0, else auto-calibrated in the task (median ratio of CSCamera yaw
   change to real mouse counts over ticks without stick input) and logged.
   If mouse reads are stale > 100 ms (ER not polling the mouse while the pad is active [I]), fall back to route `er`
   (reshape ER's own stick in the XInput detour) and log `PAD look route fallback=er`.
6. Gunship: rate * dt * `gunship_counts_per_deg` into `MOUSE_DX/DY`.

## 4.5 Left stick

Radial deadzone 0.20 / 0.98; fm = round(ly*127), rm = round(lx*127), clamped. `move_sim::Input` gains `analog:
Option<(f32,f32)>`, used at the single `Cmd::from_keys` site (move_sim.rs:504) when outside the deadzone; keys win
when both are used. Sprint stays gated at 105. The left stick is blanked for ER by the **same rule function as WASD**
(`move_input`), so PASS C's `fp_anim = "hybrid"` changes one place. The `assist` trigger (1.4) uses the stick too.

## 4.6 Config

```toml
[gamepad]
enabled = true
pad_index = -1
look_route = "mouse"          # mouse | er
look_sensitivity = 1.0
invert_y = false
input_graph = 3
turnrate_yaw = 260.0
turnrate_pitch = 90.0
turnrate_yaw_ads = 90.0
turnrate_pitch_ads = 55.0
accel = true
accel_lerp = 1200.0
accel_max_yaw = 400.0         # our choice, not MW2
rs_deadzone = 0.18
rs_outer = 0.98
ls_deadzone = 0.20
ls_outer = 0.98
trigger_threshold = 30
hold_ms = 300
mouse_deg_per_count = 0.0     # 0 = auto-calibrate
gunship_counts_per_deg = 0.0  # 0 = from [ac130] mouse sensitivity
rumble = false
log_every_s = 10
blank_er = ["a","b","x","y","lb","rb","lt","rt","l3","r3","dpad_up","dpad_down","dpad_left","rs","ls_fp"]

[gamepad.bind]
toggle = "l3+r3"
fp_toggle = "dpad_left"
fire = "rt"
ads = "lt"
reload = "x"
interact_hold = "x"
weapon_next = "y"
weapon_prev = "lb"
jump = "a"
stance = "b"
sprint = "l3"
melee = "r3"
ac130 = "dpad_up"
nuke = "dpad_down"

[gamepad.gunship]
fire = "rt"
next = "y"
thermal = "x"
exit = "back"
```

Validation like `[keys]`: unknown names rejected, `start` can never be bound, no button with two MW2 actions (the one
allowed tap/hold pair is X reload + X interact_hold), chords only of two buttons.

## 4.7 Logs, analyzer, oracles (part 4)

Lines: `PAD hook ok ...` | `PAD cfg route=mouse graph=3 rates=260/90/90/55 dz=0.18/0.20` | every 10 s
`PAD stat calls=.. slot=.. connected=0|1 pkt=.. blanked=.. look_counts=.. deg_per_count=..` | `PAD calib
deg_per_count=.. n=..` | `PAD look route fallback=er why=mouse_stale` | `PAD none seen in 60s (enable Steam Input for
PlayStation pads)`.

Rows: `pad hook` (hook ok, and either a connected slot or the explicit `none seen` line), `pad blank` (no ER action
lines such as dodge / interact attributable to bound buttons while driving — measured as zero `MOVE suspend
reason=interact` caused by A while driving; N/A without a pad), `pad look` (calibration line present when route mouse
and deg_per_count 0).

Tests `c3_pad_`: deadzone edges; graph-3 at its 7 points; rates 260/90/90/55; accel ramp and decay; counts carry over
1000 frames sum exactly; analog Cmd (fm=127 full, sprint gated at 105); latch (RT held across mode-off stays hidden
until released); L3+R3 in the same read hides both; X tap vs 300 ms hold and synthesized A; gunship blank-all-but-Start.
Config `c3_padcfg_`: defaults, unknown button, `start`, duplicates rejected. Mutations (swap yaw/pitch, drop latch,
wrong graph index) must fail. Static: eldenring.exe imports XINPUT1_4 ordinal 2 and xinput1_4.dll exports
XInputGetState as ordinal 2.

---

# PART 3 (PASS C): seeing each other in co-op, and the co-op nuke

## 3.1 What is verified

- `spawn_bullet` 0x1403a2cb0 tests `BulletSpawnData+0x44` bit 3 at 0x1403a2d28; when set, 0x14038c860 builds a 0xC0
  record (position, angles, owner net id, bullet/behavior/goods ids, bits 0..3 of +0x44) pushed into
  `CSBulletManager+0x80`, sent as packet type 0x3F. The receiver (0x1403a9b40) checks the owner's net handle type
  (1 or 6, 0x14038dd70), creates the emitter directly at 0x1403a5a20 and never re-broadcasts; auto-finish travels.
- Damage without bit 3 already lands once and replicates (friend hit c2500 to 4937; host census read 4937). Totals:
  host 376 HIT, friend 441 HIT.
- 53 vanilla Bullet rows have `atkId_Bullet = -1`; our build has `marker_bullet` with atkId -1.
- `*_fire_npc` sound aliases exist in the dumps (452 files); world models `dumps\common_mp_assets\model_export\
  weapon_*_lod0/1/2.glb` exist; `converted\guns\<gun>` has only the viewmodel.
- First person blanks W/A/S/D for ER; third person already runs "hybrid" (ER animates, our position write discards
  root motion).
- Co-op nuke evidence: host nuke killed 7/51 (44 survivors at full HP); the friend saw exactly those 7 die; the
  friend's nuke 2 killed 40/40 including 11 of the host's survivors. 79000960 is `isDisableNetSync = 1`;
  `apply_speffect` always passes `dont_sync = true`.
- Side bug: the friend's DLL credited the host's nuke kill of c2500 as its own HIT (dmg 4937, kill=1) and a streak point.

## 3.2 What is inferred (each gets a probe line)

- A remote copy of a bit-3 bullet might deal damage on the receiver: avoided by design (twins have atkId -1).
- Our player-owned spawns map to a net handle type the receiver accepts (1 or 6) — probe `COOP twin seen`.
- Synced SpEffects appear in the remote player's SpEffect list on the peer — probe `COOP marker seen`.
- Enemies are owned per PC (`ChrSetEntry.chr_update_type`: Local 0, Remote 4), ownership can move.
- An atkId -1 twin still plays its `sfxId_Hit` on impact.
- `CSFD4ModelItem.mtx43_array_entity` (+0x640) is the bone palette; the right-hand index is unknown.

## 3.3 Regulation rows (build_regulation.py; self-test checks ids and fields)

| Rows | Purpose | Key fields |
|------|---------|-----------|
| 79800000 + g*1000 + kind*100 + idx | visual twin of every M6 shot row (same flight, model, sfx; `atkId_Bullet = -1`, hit SFX kept for launcher/explosion kinds) | atkId -1 |
| 79800200..79800299 | twins of the AC-130 105/40/25 rows (790002xx) | atkId -1, sfxId_Hit kept |
| 79000963 | nuke marker bullet (no model, atk -1, short life) | atkId -1 |
| SpEffect 79001090 + SpEffectVfx 79001090 (clone of vfx 8060, no sfx) | "MW2 mode" marker; `isInvisibleWeapon = 1` hides ER's weapon on every PC | isDisableNetSync 0 |
| SpEffect 79001100..79001149 | gun marker per arsenal index g (no-op) | isDisableNetSync 0 |
| SpEffect 79001150 | AC-130 ride marker | isDisableNetSync 0 |
| SpEffect 79001190 | nuke marker (COOP_PLAN C4 id) | isDisableNetSync 0 |

Self-test: no collision with 79100000..79199999 (M6), 790002xx (AC-130), 79000950/79000960-62 (AC-130 protect, nuke);
every twin `atkId_Bullet == -1`; every marker `isDisableNetSync == 0`; 79000960 unchanged (`isDisableNetSync == 1`);
damaging rows unchanged. The regulation package sha256 changes once here.

## 3.4 Visual twins (send)

- Every damaging spawn (M6 gun, launcher marker, AC-130 shell) also spawns its twin with `unk44 = 0x9` (auto-finish +
  sync). The **damaging spawn keeps bit 3 off** (non-negotiable). Pellet guns send one twin per shot (centre pellet),
  not per pellet. Launchers: a kind-5 twin at launch and an explosion twin at detonation.
- Emitter budget: twins count against our 256-emitter pool; the M7 watchdog (`[spawn]` 192/224/32) treats twins as
  lowest priority (dropped first at `warn`).
- `[coop] visual_twins = true` (hot), `twin_min_interval_ms = 40` per gun (caps an SMG at 25/s).
- Only sent while `COOP session peers >= 1`.

## 3.5 Peer effects (receive)

- `game\bullets.rs::read_remote(peer_handles, ranges)`: owner in the CoopCtl peer handles and param_id in the twin /
  nuke-marker ranges.
- First sighting of a bullet handle = muzzle: play `<gun>_fire_npc` (or `ac130_*_fire_npc`, `_dist` beyond 60 m) at the
  bullet position through our audio with distance attenuation; start a tracer (existing TracerPass). Last position
  before it vanishes = impact: flash; explosion sound for explosion kinds; AC-130 105 also triggers the existing
  ground shake scaled by distance.
- Gun id from the row id (`g`), so no extra sync for sounds and tracer style.
- Never compare raw coordinates across PCs (re-base, 0x140394820 frame data); everything is local to the receiver.
- Converter: `convert_arsenal.py --npc-sounds` converts `*_fire_npc*` and `_dist` aliases next to the `_plr` ones;
  `convert_ac130.py` adds the AC-130 npc set. Manifest counts grow; `friend_dump.py` already dumps the alias CSVs (check).

## 3.6 Held gun and locomotion

- **World models**: `convert_arsenal.py --world` converts `weapon_<gun>_lod0/1` .glb into rigid `gun.wmmesh` in
  `converted\guns\<gun>\`; `friend_dump.py` and the manifest add `dumps\common_mp_assets\model_export\weapon_*.glb`.
- **Draw**: our renderer draws, for every player with a known gun (peers; self when first person is off), the
  world mesh at stage A `ChrCtrl.model_matrix` + per-stance offset (hip / ADS / sprint), yaw = body yaw for peers and
  aim yaw for self; lod1 beyond 10 m. Own depth, plus an occlusion fade from 3 `havok::map_ray_point` rays (camera to
  muzzle / stock / centre); alpha scaled by ui_yield.
- **Stage B** (probe): `COOP bone probe` logs palette translations relative to model_matrix for 10 s while an ER
  attack plays; the right-hand index is then set in `[coop] hand_bone = -1` (-1 = stage A).
- **Gun id for peers**: per-gun marker SpEffect applied to our player with `dont_sync = false` through a new
  `apply_speffect_synced` (the old function keeps `dont_sync = true`), the previous one removed on switch, refreshed
  every 5 s. Peers read our SpEffect ids via `ac130_read::speffect_ids(chr)`. Fallback when markers are not seen: the
  last twin's `g`, shown for 3 s after a shot.
- **Hide ER's weapon**: marker 79001090 while MW2 mode is on (removed at mode off).
- **Locomotion**: `[movement] fp_anim = "hybrid"` (default after the bench passes; `"blank"` = today). W/A/S/D and the
  left stick reach ER in first person; Space/LShift/C/F/X/Q/LAlt and pad A/B/L3 stay blanked; the M3B A/S/D yaw-gate
  pause is restored in first person; `er_delta` compensation for expected root motion (M5 5.6) is ported into the
  unstick, assist and ground-disagree evidence **before** the switch is enabled. Optional
  `fp_anim_speed_scale = true` writes `CSChrBehaviorModule.animation_speed` = our speed / ER run speed (clamped
  0.8..1.6) through `game::write`; restored to the read value at mode off and at world leave.

## 3.7 Peer-drop guard

In co-op an impact-attributed HP drop larger than the row's maximum damage x pellets x 1.5, or one with no shot of ours
in the hit window, is logged `HIT peer_drop chr=.. dmg=..` and gets no kill credit, no streak point and no damage
number. Fixes the friend's c2500 credit.

## 3.8 Co-op nuke

- **Marker**: at activation (T-10) the caller applies SpEffect 79001190 to its own player with `dont_sync = false` and
  spawns one marker bullet 79000963 (bit 3) at its position. Peers detect either one and log `NUKE marker from=<peer>
  via=speffect|bullet dt=<ms>`, dedupe per peer for 30 s.
- **Sequence**: each peer runs NukeSeq locally with its countdown aligned to the marker time (flash, sound, shake).
- **Kills**: from detonation, census passes every 0.25 s for `coop_window_s`; every NPC with `chr_update_type ==
  Local(0)` on this PC is killed (existing kill path, our `game::write`/SpEffect path). With `coop_everyone = true` there
  is no radius. Remote(4) NPCs are logged `NUKE skip reason=remote_owner` and re-checked every pass (ownership may move).
  Census gains `upd=`, `net_auth=` (ChrIns.network_authority), `dist_auth=` (NetChrSyncFlags bit 5).
- **Players**: each PC kills its own player last (existing `player_delay_s`).
- Optional probe `[nuke] probe_forward = false`: a 79000960 twin with isDisableNetSync 0 applied to Remote NPCs with
  `dont_sync = false`, logging whether the owner kills them.
- The HUD note "friends won't see it yet" is removed once the 2-PC run passes.

```toml
[coop]
visual_twins = true
twin_min_interval_ms = 40
peer_effects = true
peer_sounds = true
world_guns = true
world_gun_self_tp = true
hand_bone = -1
markers = true
marker_refresh_s = 5.0
hide_er_weapon = true

[nuke]
coop_mode = "owned"        # owned (C3) | local (C2 behaviour) | off
coop_everyone = true       # no radius in co-op
coop_window_s = 6.0
probe_forward = false

[movement]
fp_anim = "hybrid"         # hybrid | blank
fp_anim_speed_scale = false
```

## 3.9 Logs, analyzer rows, oracles (part 3)

Lines: `COOP twin sent gun=<g> row=<id> n=<k>/5s` | `COOP twin seen peer=<h> row=<id> n=<k>/5s owner_type=<t>` |
`COOP peer fx muzzle=<k> impact=<k> sounds=<k>/5s` | `COOP marker sent sp=<id>` | `COOP marker seen peer=<h> sp=<id>
dt=<ms>` | `COOP worldgun peer=<h> gun=<g> src=marker|twin lod=<n> occl=<a>` | `COOP bone probe ...` | `HIT peer_drop
...` | `NUKE marker from=<h> via=..` | `NUKE census ... upd=.. net_auth=.. dist_auth=..` | `NUKE skip
reason=remote_owner chr=..` | `NUKE coop kills local=<k> remote_skipped=<k> window=<s>` | `MOVE cfg ... fp_anim=hybrid`.

Rows (`analyze_c3.py`, plus a new cross-log mode `analyze_c3.py --pair host.log friend.log --skew 3600`):

| Row | PASS |
|-----|------|
| `twins` | every `COOP twin sent` window has a `COOP twin seen` on the peer log within 1 s (pair mode) |
| `no double damage` | for every HIT of player X on chr C, the HP the other PC reads next equals hp_after (+-1) and no HP drop of 2x appears; peer HIT total change vs C2 within 15% |
| `peer fx` | muzzle count >= 80% of the peer's twin-sent count |
| `markers` | `COOP marker seen` for 79001090 and a gun marker within 10 s of the peer's mode on |
| `peer drop` | no `kill=1` HIT within 2 s after the other PC's nuke det; `HIT peer_drop` present instead |
| `coop nuke` | both logs: `NUKE marker`, `NUKE det`, `NUKE coop kills`; the union of kills covers every NPC in either census; no NPC with upd=0 alive at window end; each PC's own `NUKE playerkill ok` |
| `fp locomotion` | `fp_anim=hybrid`, unstick fires <= 1 per 10 driving minutes (no regression) |

Tests `c3_twin_`: row id mapping (g, kind, idx) both ways, rate limiter, pellet = one twin, watchdog priority.
`c3_remote_`: classification of bullet list entries (owner peer + range), muzzle/impact event extraction from a frame
sequence, pellet grouping. `c3_marker_`: dedupe, refresh, switch removes the old. `c3_nukecoop_`: census pass kills
Local only, re-checks Remote, window end, no radius when `coop_everyone`. `c3_peerdrop_`. `c3_fpanim_`: blank sets for
hybrid vs blank, er_delta compensation keeps unstick quiet on a simulated 4.1 m/s root motion. Regulation self-test
(3.3). Converter `--check` for world meshes and npc sounds (counts and hashes).

---

# Shared sections

## S1. Every new hook, write and read, and its restore

| What | Kind | Where | Pass | Restore / safety |
|------|------|-------|------|------------------|
| physics +0x338/+0x348/+0xC8 | read | task | A | read-only |
| CSHavokMan+0xC0 filter table rows | read | task, once | A | read-only; `skip` on fault |
| CSFeMan / CSMenuMan / popup / announce fields | read | task | A | read-only, no deque walk |
| Scaleform BeginScene / EndScene | MinHook detour (ER image) | present / render thread | A (off) | prologue-checked; disabled + removed at unload; no writes |
| hudhook ECL callback (patch 0002) | our vendored code | present | A (off) | logging only |
| viewmodel offset, HUD alpha | our renderer | present | A | render-only |
| movement suspend `assist` | existing suspend path | task | A | `MOVE resync` + `MOVE on`, cap per minute |
| xinput1_4!XInputGetState | MinHook detour (system DLL) | ER's pad thread | B | caller buffer only; passthrough when off/menu/stale; removed at unload |
| DirectInput mouse lX/lY (stick counts) | existing detour, caller buffer | ER's input thread | B | only while route mouse and stick outside deadzone |
| bit-3 twin spawns | `spawn_bullet` (existing call) | task | C | auto-finish; watchdog priority; only with peers |
| marker SpEffects (synced) | `apply_speffect_synced` | task | C | removed at mode off / switch / world leave; ids in our range |
| `CSChrBehaviorModule.animation_speed` | `game::write` | task | C (opt) | restored to read value at mode off / world leave |
| NPC kills on nuke (Local only) | existing nuke kill path | task | C | window-bounded; Remote skipped |
| remote bullets, peer SpEffect lists, model_matrix, bone palette | read | task | C | read-only |

## S2. `tools\scripts\check_c3.ps1`

Windows PowerShell 5.1, same skeleton as check_m8 (`Add-Result`, `Invoke-Tool`, `Run-Check`, logs in `work\check_c3\`,
`-AllowSkip`, `-SkipM8`; exit 0 only if every check PASSes; prints `C3 OFFLINE ORACLES: PASS`). Never launches
anything; game exes only read. Checks:

1. **M8 suite**: `check_m8.ps1` exit 0 (runs m7..m2, stages the DLL).
2. **ER static pin** (`c3_static.py`, Python -I, read-only): eldenring.exe sha256 pinned; bytes at 0x1403a2d28
   (`test byte [rsi+44h],8`), 0x14045ff10 stores to +0x338/+0x348/+0xC8, CSCollisionFilter vtable 0x142b94df0, the
   BeginScene/EndScene prologues, XINPUT1_4 import ordinal 2 + xinput1_4.dll export ordinal 2 = XInputGetState.
3. **Layer table**: `work\c3\erscan\emu2.py` regenerated output == `tools\scripts\fixtures\c3_layers.json` (sha) and
   rows 0x58/0x5b/3 equal the lists in 1.1; erscan run only if its exe exists (else SKIP).
4. **Converter**: `convert_arsenal.py --check` (PASS C: world meshes + npc sounds; PASS A/B: unchanged); `convert_ac130.py --check`.
5. **Regulation**: `build_regulation.py --self-test` (3.3 rules; PASS A/B: package sha still 9a33f2524237).
6. **cargo build** (release, 0 warnings).
7. **cargo test**: 0 failed; at least one test per prefix `c3_rays_ c3_layers_ c3_assist_ c3_fp_talk_ c3_fp_f6_
   c3_uiyield_ c3_dmgnum_ c3_project_ c3_hitlogic_ c3_offsets_` (A), `c3_pad_ c3_padcfg_` (B), `c3_twin_ c3_remote_
   c3_marker_ c3_nukecoop_ c3_peerdrop_ c3_fpanim_` (C). Prefixes of passes not yet built are SKIP (fail without -AllowSkip).
8. **movebench**: b13-b24 at 30/60/144 Hz, all PASS.
9. **Config**: `mw2er.toml` and the friend toml parse with the C3 defaults listed in this document; the rejection tests
   (ray fallback, pad names, start, duplicates).
10. **Wiring** (`m2_static_checks.py c3wiring --self-test`): rays `player` used by every cast site; talk latch params;
    F6 override; ui_yield applied in viewmodel and hud on the single draw list; damage numbers after the hitmarker;
    XInput detour has no alloc/log/lock; stick counts added after SENS_SCALE; twins never set bit 3 on damaging rows;
    `apply_speffect` still passes dont_sync=true; nuke kill gated on `chr_update_type == 0`. Each with a mutation that must fail.
11. **Analyzer**: `analyze_c3.py --self-test` (synthetic logs per row: PASS / FAIL / N/A) and the known-bad replay of
    the C2 host log (`fp talk latch` and `fp f6` FAIL).
12. **DLL proof strings** (`m2_static_checks.py c3strings`): the new log formats are in the built DLL; built == staged.
13. **Friend package current**: `dist\MW2inER-coop-friend\manifest\host_manifest.json` DLL sha == staged DLL sha and
    regulation sha == built package; `m2_static_checks.py dist` PASS. (FAIL means: rebuild the package, S3.)
14. **Hygiene**: `m2_static_checks.py hygiene` and `gamefolder` PASS.

## S3. Friend package rebuild

After `check_c3.ps1` passes checks 1-12 (end of each pass that is played together; mandatory after PASS C):

1. `powershell -ExecutionPolicy Bypass -File tools\coop_friend\make_friend_package.ps1`.
2. The builder gains (PASS C): the new scripts (`analyze_c3.py` is not needed by the friend; converter changes ship),
   `friend_dump.py` covering `model_export\weapon_*.glb` and the npc sound aliases, the friend toml with the C3
   defaults (`[nuke] always_available = false`, `allow_in_coop = true`, `coop_mode = "owned"`), and `coop_manifest.py`
   hashing the new converted files and the new regulation package.
3. Re-run check 13; record DLL sha, package sha and zip sha in MODLOG. Re-run the self-test into `work\friend_selftest`
   (SETUP PASSED, converted N/N == host, REGULATION PASS with the new package sha) and delete the temp output.
4. The zip stays in `dist\`; the user sends it. Nothing is uploaded.

## S4. Two-player test plan (user runs it; log lines on both PCs; friend clock +1 h)

Before: both PCs on the identical build (DLL sha and regulation sha in the boot lines must match across logs).

**PASS A (can be solo first, then together)**
1. Boot: `MOVE cfg ... rays=player(cast=0x3)`; first `MOVE on`: `MOVE filter live ... layer=3 src=live` and three
   `MOVE filter table ... static=match` on both PCs.
2. Walk the C2 problem spots (rocky slopes 20-45 deg, roots, the wagon area): no F5 needed; any stall gives
   `MOVE assist n=.. why=stall` then `MOVE on` within ~2 s; wagons and crates stay solid (no clipping).
3. Talk to an NPC and rest at a grace: `FP latch released signal=talk_id why=closed after ~2s`; FP back on within 3 s
   of closing the dialog; no `MOVE on ... fp=0`. Press F6 during a talk: `FP toggle F6 override`, FP returns after.
4. Pick up an item / get a co-op message: `UI yield on reason=.. idx=.. vis=[..]` then `off`; the message is readable;
   firing works during the yield. Review C3: `[ui_yield] learn = true` on both PCs for this run (`UI learn idx=..`
   lines every 60 s) so the toast indices 17/19/65 can be confirmed against the messages.
5. Shoot enemies in FP and TP: numbers at the hit, rising and fading; `DMGNUM` count matches `HIT` count.
6. Optional (one PC): `[ui] probe_scaleform = true`, play 2 minutes, set it back; send the `UI probe` lines.
7. Both: `python -I tools\scripts\analyze_log.py` -> C3 part-1/2 rows PASS.

**PASS B (each player with a pad)**
1. Boot: `PAD hook ok ... overlay=..`; within 10 s `PAD stat ... connected=1`. PlayStation pad without a connected slot
   -> `PAD none seen`, enable Steam Input for ER and restart.
2. MW2 mode by L3+R3; RT fires, LT aims, X reloads, X-hold talks / rests, Y/LB switch, A jumps/mantles, B crouch/prone,
   L3 sprints only near full forward, D-pad up AC-130, down nuke, left first person.
3. Right stick: turn feels like MW2 (yaw faster than pitch, slower ADS); `PAD calib deg_per_count=..` logged.
4. MW2 mode off: ER controls fully back (dodge on B, interact on Y, jump on A, lock-on on R3). Review C3: in MW2 mode
   R3 = one ER light attack (`PAD cfg ... melee_synth=RB`), D-pad right is hidden (no ER weapon swap under the MW2 gun).
5. Analyzer rows `pad hook`, `pad blank`, `pad look`.

**PASS C (together)**
1. Boot on both: same regulation package sha. Host opens, friend joins: `COOP session peers=1` on both.
2. MW2 mode on both: `COOP marker sent sp=79001090` and gun marker on each; on the other PC `COOP marker seen peer=..
   sp=79001090 dt=..` and `COOP worldgun peer=.. gun=.. src=marker`. Each sees the other holding the MW2 gun and ER's
   weapon hidden; walking shows walk/run animations (`MOVE cfg ... fp_anim=hybrid`).
3. Each fires one gun of each class at the same enemy: sender `COOP twin sent`, receiver `COOP twin seen` and
   `COOP peer fx`; tracers and the npc gun sound at the peer's position. Compare HP per hit across logs (no double
   damage). If `COOP twin seen` never appears: `owner_type` in the sent line tells why (receiver accepts 1 or 6).
4. Launcher shot and AC-130 ride by one player while the other watches from the ground: explosions and shells visible
   and audible.
5. Host calls the nuke: host `NUKE marker sent`, friend `NUKE marker from=<host> via=speffect|bullet dt=..`; both
   `NUKE det` and `NUKE coop kills local=.. remote_skipped=..`; no NPC with upd=0 alive at window end on either PC; both
   players die (`NUKE playerkill ok`) and respawn. Repeat with the friend calling (after a streak). Friend log has no
   `kill=1` HIT credit for the host's kills (`HIT peer_drop` instead).
6. Both run `analyze_log.py`; the host also runs `python -I tools\scripts\analyze_c3.py --pair <host log> <friend log>
   --skew 3600` -> `twins`, `no double damage`, `peer fx`, `markers`, `peer drop`, `coop nuke` PASS.

## S5. Verified vs inferred (summary)

Verified: the filter word layout and default table; the player proxy filter offsets; the talk-latch root cause and F6
effect; the present-time draw order; Scaleform HAL addresses; the vendored UI fields; the bit-3 send/receive path and
no re-broadcast; single damage today; twin rows can be damage-free; the dumped npc sounds and world models; ER's
XInput import and no ISteamInput; MW2 look numbers; analog pmove; nuke ownership evidence and the peer-kill credit bug.

Inferred (each with a probe or analyzer row): player = layer 3; the invisible-wall layers; `CSHavokMan+0xC0`; ui_states
index meanings; Scaleform injection feasibility; head-shot guess; receiver accepts our owner type; synced SpEffects
visible on peers; isInvisibleWeapon via marker hides the weapon on peers; atk -1 twin still plays hit SFX; bone palette
and hand index; hybrid FP locomotion looks right to peers; ER polls the mouse while a pad is active; MW2 accel formula;
ER deg/count; ownership per PC via chr_update_type; Steam Input behaviour under me3.

## S6. Non-negotiables

**PASS A**
- Ray mode `player` casts with the **layer only** (`v & 0x7F`), accepted layers 2/3/4, fallback 0x3; `union` / `map` /
  `object` remain selectable by hot reload; the own capsule stays excluded; physics+0xC8 is a u32, never written.
- The live filter words and table are read-only; nothing writes ER's collision filter.
- `MOVE assist` uses the existing suspend/resync path, is capped per minute, never fires in air / mantle / ladder /
  AC-130 / nuke / menu.
- The talk latch releases 2 s after the cursor closes and movement follows FP's talk hold; F6 while FP is off for a
  non-user reason never sets `wanted = 0`.
- UI yield is render-only and never blocks firing or touches `wgate`; the Scaleform probe is off by default,
  prologue-checked, read-only and removed at unload; no draw is injected into ER's command lists in C3.
- Damage numbers only for our own attributed HITs; all HUD drawing stays on the one background draw list.
- check_m8 keeps passing inside check_c3; the C2 host log replay must FAIL the two FP rows.

**PASS B**
- Hook the `xinput1_4.dll` export with MinHook; never patch eldenring.exe's IAT or code; never write PadDevice.
- Edit only the caller's XINPUT_STATE on success (return 0) for the selected slot; passthrough whenever MW2 mode is
  off, a menu is open, the window is unfocused or the heartbeat is stale; per-button release latch.
- No allocation, lock, log or panic inside the detour.
- Start is never bound; X-hold must synthesize A (interact) while A is taken; stick counts are added after the ADS
  scaling; the left stick is blanked by the same rule function as WASD.
- Rumble off by default; MW2 numbers (260/90/90/55, graph 3) are the defaults.

**PASS C**
- The damaging bullet never has bit 3; only atkId -1 twins and markers are synced. Peer effects are cosmetic only and
  never call damage paths.
- Peer drops never give kill credit, streak points or damage numbers.
- New rows only in the reserved ranges (twins 798xxxxx, marker bullet 79000963, SpEffects 79001090-79001190); 79000960
  stays isDisableNetSync 1; `apply_speffect` keeps dont_sync=true and the synced variant is separate.
- The nuke kills only NPCs owned locally (`chr_update_type == 0`) on each PC, re-checking Remote ones each pass; each
  PC kills only its own player.
- `fp_anim = "hybrid"` is enabled only after the er_delta compensation is ported and `c3_fpanim_` passes; any
  `animation_speed` write goes through `game::write` and is restored.
- The world gun is drawn by our renderer with its occlusion fade and ui_yield alpha; no ER depth hooks in C3.
- The friend package is rebuilt with `make_friend_package.ps1` so DLL, converted assets and the regulation package
  are identical on both PCs (check 13); the zip stays local.

---

# As built: PASS A (2026-10-19, offline only, NOT run in game)

Implemented: 1.3 ray mode `player` (live `physics+0x338`, layer only, accepted 2/3/4, fallback 0x3; union / map / object
remain hot rollbacks, logged `rollback=1` in `MOVE cfg`), 1.4 `MOVE assist`, 1.6 diagnostics (`MOVE filter live`,
`MOVE filter table`, `MOVE blocked wish=`), 1.7 talk latch / F6 override / fade stale, 2.3 UI yield, 2.4 Scaleform probe
(off by default), 2.6 damage numbers, `analyze_c3.py`, `c3_static.py`, `c3_layers_rs.py`, `m2_static_checks.py
c3wiring / c3strings`, movebench `--c3`, `check_c3.ps1` (checks 1-14; `$BuiltPasses = A`).

Deviations from the text above (each deliberate):
- movebench scenarios are C21-C24 (`movebench --c3`): B21-B24 already exist in the M8 bench.
- Assist `after_refusal`: a refusal only skips the `stall_s` wait while the player is already stalled; a held jump in
  the open is refused `no_face` on every press and must not hand back.
- `drop_in` is in the renderer's axes (x forward, y left, z up): default `[-2.0, 0.0, -9.0]` (2 in back, 9 in down).
- UI signals: `status` = ring read != write, or the banner timer in (0, 3) s (an idle 0 timer is no message); `summon` =
  the queue's current entry priority > 0 [I] (CSFixedList has no count); every signal has a stuck guard (`stuck_s` 20 s:
  a bit on longer than that is ignored until it clears, `WARN yield signal stuck`).
- The hudhook ExecuteCommandLists callback (patch 0002) is not built; the probe logs `ecl n/a`. Probe hooks stay for the
  process lifetime like every other hook of the DLL (the DLL has no unload path).
- Damage numbers use a second, larger built-in font (1.25 x font_px x scale); the 1.3 -> 1.0 pop is not drawn.
- `MOVE cfg` shows `filters=player(cast=.. fallback=.. layers=.. src=..)` (`rays=` already names the mantle's ray budget).
- The analyzer row `mantle refusals` of 1.8 is named `no_face rate` (the M8 analyzer owns `mantle refusals`).
- The talk edge is not re-baselined at the latch release: `enter()` re-arms every edge at the current value.
- Assist keeps the MW2 stance across the hand-back (like a menu), so a crouched / prone player is not stood up.
- `c3_layers_` mutation: a row mutation in Rust (the helper-built block dropped) instead of running emu.py.

---

# As built: PASS B (2026-10-19, offline only, NOT run in game)

Implemented: 4.2 the `xinput1_4!XInputGetState` detour (`mod\mw2er\src\pad.rs`; pure rules in
`mw2er-core\src\pad_logic.rs`), 4.3 the mapping, 4.4 the right stick (route `mouse` through `input_hook.rs`'s mouse read
after the ADS scaling, route `er` / fallback), 4.5 the left stick (`move_sim::Input::analog`, `Input::cmd()` /
`Input::wish()`, `move_input::wasd_blanked` for both W/A/S/D and the stick), 4.6 `[gamepad]` / `[gamepad.bind]` /
`[gamepad.gunship]`, 4.7 the `PAD` lines, the analyzer rows `pad hook` / `pad blank` / `pad look`, `c3_pad_` (16) and
`c3_padcfg_` (2) tests, `c3_padmut.ps1` (the three Rust mutations, run by check 7), c3wiring checks 12-20 (15 more
mutations), c3strings PASS B strings, `$BuiltPasses = A, B`.

How the pad reaches the existing code: every pad action edge bumps the SAME counter the keyboard detour bumps (F5 toggle,
F6, reload, weapon next / prev, nuke, AC-130 call / next / thermal / exit, the movement press counters incl. the
interact counter), and the levels (RT, LT, L3 / A / B held) are OR-ed into the task's fire / ADS and movement's
`RAW_KEYS`; so every existing gate (menu, gunship, arsenal, nuke, movement) applies unchanged. Action edges are counted
only while MW2 mode drives (the toggle always; the gunship actions only during a ride).

Deviations from the text above (each deliberate):
- **ER's interact button is Y, not A** (`[gamepad] er_interact = "y"`, configurable). On ER's default pad layout A is
  jump and Y is the event action (talk / rest / doors / pickups) [I: from ER's default control layout, not read from
  the exe]; synthesizing A would make the character jump. The X hold also bumps the movement interact counter, so the
  controller hands back exactly as for the keyboard E (`MOVE suspend reason=interact`), and logs `PAD interact hold n=..`.
- The toggle chord (L3+R3) is hidden from ER also while MW2 mode is off, with `[keys] blank_toggle_key` (like F5);
  everything else passes through when MW2 mode is off / a menu is open / unfocused / the heartbeat is stale.
- X: reload on release of a tap (< `hold_ms`), so a hold never also reloads. B tap / hold goes through the existing
  console stance key logic (`stance_hold_tap`, MW2's `cl_stanceHoldTime` from movement.json), not `hold_ms`.
- `rumble` is not implemented (no XInputSetState hook); `rumble = true` is rejected by the config parser.
- "Hooks removed at unload": the DLL has no unload path (like every other hook); a hot `[gamepad] enabled = false`
  makes the detour pass everything through. `enabled = false` at boot installs nothing (`PAD off ...`).
- `pad_index = -1` selects the first connected slot that shows input (a button, a trigger > 30, a stick > 0.25) and
  reselects after that slot reports not connected.
- The graph is applied per axis after the radial deadzone; the accel extra rate applies in hip only (blended out by the
  ADS fraction) and scales with the shaped yaw; all rates blend between hip and ADS by `vm_state.ads_at`.
- Route `er` (and the mouse-stale fallback) gives ER the right stick reshaped by the radial deadzone and the MW2 graph
  (ER's own response curve still applies on top [I]).
- Calibration (`PAD calib`): |ER camera yaw change| / |counts delivered on lX| (the user's mouse after the ADS scaling
  plus the stick's counts) per task tick with >= 20 counts and < 90 degrees, running median of 31 (first estimate after
  15). Magnitude only: ER's heading sign convention is not verified. Until then a provisional 0.08 deg/count [I] is used
  (the stick turns at a wrong but bounded speed until the first estimate). Logged at the first estimate and on a > 15 %
  change (>= 20 s apart).
- Left stick: the circle is stretched to the square (a full diagonal is full speed, as with two keys); sprint needs
  forwardmove > 105 (about 34 degrees off forward at full deflection). For the keyboard-level checks (wish, interaction
  holds, the yaw gate) the stick counts as W/A/S/D when an axis' share is > 0.38.
- The AC-130 aim uses the stick through `MOUSE_DX/DY` with dt between XInput reads; counts per degree default to
  1 / (`[ac130] mouse_deg_per_count` x `mouse_sens`).
- Analyzer `pad blank`: PASS when the detour hid buttons while a pad was connected and MW2 mode drove; interact suspends
  that do not follow a `PAD interact hold` within 1.5 s are a WARN (the keyboard E is not logged), not a FAIL.
- `PAD stat ... connected=1` means the selected slot was read in the last 150 ms (the levels' freshness rule).

Unverified until the PASS B test (S4): that ER reads the pad through xinput1_4 under Steam Input / me3 (`PAD stat
connected=1`, `first_byte` / `overlay=` show whether Steam's overlay hooks it too); that ER polls the DirectInput mouse
while a pad is used (else `PAD look route fallback=er`); ER's degrees per count and that it turns linearly; that
DirectInput +Y looks down in ER; that Y is ER's interact button; whether ER flips its button prompts between keyboard
and pad when stick counts arrive on the mouse; the MW2 look feel.

---

# As built: PASS C (2026-10-19, offline only, NOT run in game)

Implemented: 3.3 the regulation rows (`build_regulation.py make_coop_plan`, SAFETY_C3, 10 more self-test mutations), 3.4
twins out (`spawn::spawn_synced`, `coop_vis.rs CoopVis::send_twin`; hitscan / launcher launch / launcher explosion /
AC-130 explosion), 3.5 peer effects in (`bullets::read_remote`, `mw2er_core::coop_vis::RemoteTracker`, HUD tracers and
flashes, `AudioMsg::GunAt` for the peers' `fire_npc` aliases, the AC-130 explosion cues), 3.6 world guns stage A
(`convert_arsenal.py build_world` -> `world_lod0/1.vmmesh`, `VmAssets::load_gun_world`, `Renderer::upload_world /
record_world`, `viewmodel.rs draw_world_guns`), the synced markers (`ac130::apply_speffect_synced`, `MarkerCtl`,
`peer_markers`), `fp_anim = "hybrid"` (`BlankCtx.fp_hybrid`, `move_logic::rm_compensate`, the yaw gate's A/S/D pause),
3.7 the peer-drop guard, 3.8 the owned co-op nuke (marker SpEffect + marker bullet, remote start, `Owner` from
`ChrSetEntry.chr_update_type`, no radius, window passes, `NUKE coop kills`), `analyze_c3.py` PASS C rows + `--pair`,
`m2_static_checks.py c3wiring` checks 21-30 / `c3strings`, `check_c3.ps1` with `$BuiltPasses = A, B, C`.

Regulation package (changed once, as planned): fc10c5eb273429da0f1d32c2f0686ec909cbd636e7a3b66ca5f0c18c7528533a
(`REGULATION PASS bullet+342 atk+278 speffect+39`; was 9a33f2524237: +56 twins, +1 nuke marker bullet, +37 markers).

Deviations from the text above (each deliberate):
- Twins: one per gun for every hitscan band (slot `g*1000`, the twin carries no damage so the band does not matter), one
  per launcher launch (`g*1000+500`), one per explosion tier (`g*1000+601..603`), one per AC-130 tier
  (`79800201..79800401`): 56 rows, not one per M6 row. Every twin has `HitBulletID -1` (no impact marker on the peer).
  Only the first tier of an explosion is spawned as a twin.
- The MW2-mode marker 79001090 uses the vanilla SpEffectVfx row 8060 (already `isInvisibleWeapon = 1`, no sfx [V decoded])
  instead of a cloned SpEffectVfx row: SpEffectVfxParam stays untouched. The marker SpEffects clone vanilla row 0 (every
  field at its paramdef default), with a finite `effectEndurance = 30 s` (refreshed every `marker_refresh_s`), all
  `effectTarget*` of the protection row 1540 and `isDisableNetSync 0`. Gun markers are `79001100 + k`, k = the position
  of g in the 34 stable indices (both PCs agree whatever their `[arsenal] guns` order).
- Peer tracers are HUD lines on the one background draw list (not the AC-130 TracerPass); peer explosions add a HUD flash
  and the AC-130 explosion cue (exp_105 / exp_40; the 25 mm uses exp_40 at 0.4). No ground shake for a peer's 105 mm.
  `convert_ac130.py` is unchanged (its explosion cues are reused for peers).
- The peers' gun sound is the weapon file's `fireSound` alias (`*_fire_npc`, role `fire_npc` in sounds.json), with its
  secondary (the far layer) mixed in below half gain; distance gain `sound_min_m` .. `sound_max_m`.
- Stage B (the hand bone, `COOP bone probe`) is not built: `[coop] hand_bone` accepts only -1. The stage-A anchor is the
  character's feet + `world_gun_offset_m` (forward, right, up = 0.45 / 0.20 / 1.15 m [I]) along the physics orientation's
  forward axis (`[first_person] yaw_forward_axis`, -z); our own gun in third person faces ER's camera.
- The world-gun occlusion is binary (hidden when all three map rays are blocked), not a fade; world guns are not dimmed
  by the UI yield (no alpha path in the opaque PSO). They are drawn before the viewmodel with our own depth (cleared
  once), up to 16 per frame, each with its own constants (the ring gained 16 sub-slots per frame slot).
- `fp_anim_speed_scale` is not built (rejected when true; no animation-speed write). The er_delta compensation is
  `move_logic::rm_compensate` (ER's expected run root motion along our motion, at most `fp_anim_rm_mps` x dt, removed
  from ER's displacement) on the mantle's er_push measure and the er_move hand-back during a mantle; the push-out rule
  (`er_pushout`) already ignored root motion along our motion (c3_fpanim_ test at 30 / 60 / 144 Hz).
- `[nuke] coop_mode` accepts `owned` and `local` only (`off` is `allow_in_coop = false`). `probe_forward` is not built
  (rejected when true). A remote start needs `coop_mode = "owned"`, `allow_in_coop`, the nuke rows, no running sequence,
  a living player in the world; it never uses or refunds the streak. A character in no ChrSet the census walks has owner
  `unknown` and is tried like a local one (a no-op on a peer-owned one, as C2 showed). The co-op window passes are
  numbered `pass=100+`.
- Peer-drop guard: on the gun HIT path only (launcher / AC-130 attributions already need our own blast); the bound is the
  band row's flat attack x pellets x `peer_drop_factor` (1.5).
- Pair-mode `no double damage` compares HIT transitions claimed in both logs (the C2 logs show 4 such double credits);
  the single-log `peer drop` row FAILs a kill credit with dmg >= 1000 in a session (the C2 friend log's c2500 credit).
- `make_friend_package.ps1` is unchanged: the converter, the regulation builder and the friend toml come from the host
  files; the friend README / INSTALL_GUIDE describe the C3 co-op behaviour.

## As built: review C3 (findings applied after PASS C, offline only)

- World guns / peer effects: a peer whose havok frame differs from ours is skipped (`COOP worldgun skip peer=..
  reason=other_frame`); its twins give no tracer / flash / sound (`COOP peer fx skipped other_frame=n`); the nuke
  marker bullet still counts. Peer effects run only with a known local handle (a missing player sample is skipped,
  and the last known local handle is never a peer's). Peer guns point along the peer's last shot for 1.5 s (aim), else
  the body facing.
- World-gun pass: skipped while `ui_yield` is at 0.5 or more (no per-draw alpha in the renderer); HUD peer tracers
  already dim with the yield. Peer HUD flashes are off by default (`[coop] peer_flashes = false`: the explosion twin
  keeps its SFX, so ER draws it).
- Markers: `MarkerCtl` removes any present C3 marker that is neither wanted nor tracked (orphans of a reset / world
  change), at most once per second per id; the co-op panic path removes every C3 marker before the reset
  (`COOP marker removed sp=.. why=panic`). `NUKE_DEDUPE_S` 75 s and a per-peer entry kept until the marker has been
  absent 2 s (a persistent marker never starts a second remote nuke).
- First person: F6 overrides only talk / menu / camera gate / throw, and a cutscene held only by the fade latch; a
  load or a camera-mask cutscene defers (`FP toggle F6 deferred reason=..`, wanted stays 1); the load and mask latches
  are never released by F6.
- Pad: `process` / `mouse_look` are serialized by try-lock flags; a contended call passes the last read's hiding
  through without stepping (`PAD stat ... contended=n`). `[gamepad] er_melee = "rb"`: R3 pulses ER's RB for 120 ms
  (one light attack with the hidden ER weapon) [I]; `blank_er` gains `dpad_right`.
- Hit credit: the peer-drop guard also covers the launcher (impact + all tiers as the bound) and AC-130 (the largest
  gun's tiers) paths; while a peer's nuke runs here every HIT is `HIT peer_drop ... src=remote_nuke` and HP drops are
  not attributed.
- Stage B: `[coop] hand_bone` accepts 0..255; `[coop] bone_probe = true` logs ER's bone palette of our character for
  10 s after MW2 mode turns on (`COOP bone probe t=.. n=.. ptr=.. feet=.. q=..` + `COOP bone probe bones=a-b i:A|B`),
  palette `CSFD4ModelItem+0x640` [I]; an implausible bone falls back to stage A (`COOP worldgun bone=.. src=fallback`).
- A peer's 105 mm impact within 120 m shakes our view (distance-scaled, `[ac130.shake]` look).
- Player filter [V exe, review]: the setter 0x14045ff10 has one caller (0x1403cb1f2 in 0x1403caeb0), which builds the
  +0x338 word from the base word +0x2E8 (getter 0x14045ddc0), `and 7Fh`, remapped by state flags into variant layers
  (2 -> 0xF/0x15, 3/4 -> 0x10/0x16/0x17, 5/6 -> 0x11/..., and `mov ebx,3` for one case); pinned in c3_static.py. The
  base word is written at run time (no static constant found), so layer 3 for the player stays [I]; `MOVE filter live`
  now also logs `base2e8= base2f8= base_layer=`. The hit body's layer in `MOVE blocked` is NOT added: the vendored
  `cast_ray` returns only the hit point (no body id), and a custom collector is out of scope.
- Not done (reasoned): the sender still sees its damaging row's SFX plus its own twin's at the same point and frame
  (stripping SFX from damaging rows would make solo explosions depend on twins, which the emitter watchdog drops first,
  and would change every M4/M6/M7 regulation pin); check in the 2-PC test whether it is visible.
- Analyzer: `coop nuke` FAILs on survivors above the solo baseline (max(2, 10%)), also for pre-PASS-C co-op nukes (the
  C2 host log's 44 of 51 FAILs); pair mode FAILs when one game left R NPCs to the peer's game and the other killed fewer
  than R within 30 s (count match: handles are per PC [I]). c3_padmut uses a folder per run and checks the copy.

## As built: review 2026-10-24 (the two-player run; nuke kill ownership)

- Evidence (logs\coop\host_c6_2026-10-24.log / friend_c6_2026-10-24.log, release 0.3.0, co-op id 11c2066c): the host's
  nuke (13:15:15 `NUKE activate ... role=host coop_mode=owned`, marker sent ok=1 peers=1) reached the friend
  (14:15:16 `NUKE marker from=0xffffffff16f00001 via=bullet action=start`). Both censuses read every character as
  `upd={remote:N}` (host 20, friend 21; `chrsets=0`; `net_auth={0:19,8192:1}` / `{0:20,8192:1}`), so both skipped the
  same 8 live enemies (handles 0x0a000000100000bf, ..70, ..5d, ..72, ..04, ..ca, ..7b, ..06) as the other's:
  `kill=0 skipped=20` / `skipped=21`, `NUKE coop kills local=0` in both; both players then died as designed.
- `ChrSetEntry.chr_update_type == 4` is not "a peer's game owns it" (the 3.2 inference is refuted): offline solo
  censuses read `upd={remote:30}` / `{remote:31}` (logs\mw2er.1.log 2026-10-21 18:54 / 21:24 / 23:42, coop=0) and the
  solo nuke killed all of them (24/24, 22/22, 20/20). `network_authority` is 0 for every enemy in solo and in both
  co-op games: no readable per-enemy owner signal. What is known (C2): an application by a game that does not
  simulate the enemy does nothing (42 of 49 host applications left the enemy at full HP; the 7 that worked died on
  the peer too), and the client's applications took effect on 43/44 and 40/40.
- New rule (`nuke_targets::coop_hold`, `[nuke] coop_client_fallback_s = 1.0`): the host's game (`role=host`) applies
  the lethal effect to every eligible enemy from its first kill pass; a client (or an unknown role) holds its kills
  (`NUKE skip reason=await_host`) for `coop_client_fallback_s` after its own first kill pass (clamped to the window),
  then applies to every enemy still alive (the host's kills it observed read dead). Each enemy dies once, in the game
  that simulates it. `upd=` stays in the census as a histogram only. `coop_everyone` (no radius) is unchanged; solo
  and `coop_mode = "local"` never hold.
- Lines: census `... upd={..} net_auth={..} dist_auth=.. held=<n> role=<r> hold=<0|1> coop=1 no_radius=1`;
  `NUKE skip reason=await_host pass=<k> n=<n> role=<r> chr=[..]`; `NUKE coop fallback pass=<k> role=client since_s=..
  alive=<n> held_before=<n>`; `NUKE coop kills local=<k> left=<n> held=<n> role=<r> fallback_s=1.00 window=6.0s ..`;
  `NUKE coop window end passes=<n> held=<n> left=<n> role=<r> window=6.0s`.
- analyze_c3 `coop nuke`: FAIL a host census with `hold=1`, a `left>0` window end, and the pre-fix pattern
  (`coop kills local=0 remote_skipped=R>0`) on a single log; pair mode reads `left=` (else `remote_skipped=`).
- Static checks rule 23 rewritten (the classify hold without any `Owner::` test, the host never holds, the fallback
  clamped to the window, the kill pass passes the stored role); 5 mutations.

## As built: review 2026-10-24 (the two-player run; tracers through walls)

- Report: "no bullet occlusion, bullet trails can be seen through walls". The peers' tracers (3.5, `RemoteTracker`
  segments: the twin's first read -> its current / last read) and our own launcher streaks (M6) are ImGui lines on the
  HUD's one background draw list, drawn at Present over ER's finished frame: no depth test of any kind. ER's own
  bullets (our damaging rows, ER's VFX) are depth-tested by ER and were never the problem; the AC-130 rounds are drawn
  from the gunship camera to their map-ray impact (the line of sight is the shot line). ER's scene depth (C6 part 15) is
  not live in either log (no `DEPTH` line; it only primes the world-gun pass, never ImGui). Logs: `COOP vis cfg ...
  tracers=1 ... occlusion=1` on both PCs (the world guns already used 3 map rays; the tracers none), host 1462 peer
  muzzles (= tracers) in 13:04-13:35, friend 774.
- Fix (CPU, no ER GPU access; `mw2er_core::tracer_occl`, DLL `CoopVis::occlusion_tick` on the task thread right after
  the tracker): per tracer (key = the twin's handle mixed with its first position) one clip ray from the twin's start
  along its velocity (`CLIP_RANGE_M` 400): the tracer never reaches past the first map hit (- 0.05 m); up to 9 points
  along the clipped path (one per metre at least), each a map ray from the HUD's eye (`view_pos`): hidden when the map
  is hit more than 0.25 m before it. Drawn = the runs of visible points (a mixed pair cut at its middle); the head dot
  only on the piece that ends at the bullet. Budget `[coop] tracer_occlusion_rays = 32` per tick (clips first, then
  uncast points from the muzzle on, then points cast from a camera > 0.75 m away); an uncast point / clip draws
  (unclipped), no physics world (or `tracer_occlusion = false`) = drawn exactly as before. A peer HUD flash
  (`peer_flashes`, default off) gets one ray at its first tick (hidden when blocked > max(1 m, size / 2) before it).
  Our launcher streaks: one ray camera -> head per projectile (at most 8) in `publish`; a hidden head = no streak.
  Both flags hot, `tracer_occlusion` code default true. The twin path is the peer's shot, so a tracer of a peer in
  another havok frame is still never drawn (review C3).
- Line every 5 s: `COOP tracer occl draws shown=.. partial=.. hidden=.. clipped=.. rays=.. budget_out=.. no_rays=..
  flashes_hidden=.. streaks_hidden=../5s` (draws = tracer-ticks); `COOP vis cfg ... tracer_occl=1 tracer_rays=32`.
- analyze_c3 row `tracer occl` (single log; N/A for a DLL without the field, e.g. both 2026-10-24 logs); static checks
  c3wiring rule 43 (10 mutations).
