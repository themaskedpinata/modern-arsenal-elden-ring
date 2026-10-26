# M7 design: run-14 fixes (emitter leak = AC-130 damage loss, P90 double fire, reload/rechamber) + the MW2 Tactical Nuke

Labels: **[V]** verified (log, disassembly of the on-disk exe, dumps, code read), **[I]** inferred (needs run 15 in game).
Inputs: run-14 session of `logs\mw2er.log` (the last session, from line 12911), static read of `eldenring.exe` 2.7.1.0
(file only, project `iw4dis`), MW2 dumps in `dumps\m7` (Unlinker IW4MS, 0 warnings / 0 errors), vanilla regulation
decoded with `param_rows.py`, vendored `fromsoftware-rs`.

Hard rules (unchanged): never launch ER / MW2 / me3 / Steam; never write into a game folder or `%APPDATA%\EldenRing`;
no push / publish; downloads only via cargo; do not touch `D:\Mods\MW2inER-m3`; every runtime write through
`game::write` or an existing gated engine call; PATH / Python lines as in check_m6.

---

## 1. Scope and decisions at a glance

| item | decision |
|---|---|
| A1 code 4 / AC-130 no damage | Root cause found [V]: every `spawn_bullet` call creates one **bullet emitter** that we never let finish (BulletSpawnData+0x44 bit 0 = 0). Fix: set bit 0 (new vendored patch **0003**). Watchdog + emitter walk in the DLL. |
| A2 P90 17 ms | Cadence carry after a hitch [V]; clamp `next_shot` to `now - 0.15*ft`. |
| A3 reload / rechamber without done | Real bug [V]: auto-reload restarts on the outgoing gun during the drop; rechamber cancel is silent. Fix both + log `RECHAMBER cancelled`. |
| A4 analyzer | Accept ARSENAL sessions, cancelled rechamber, per-gun clip; new A1 / NUKE rows. |
| Nuke earning | 25 **consecutive kills** in one life (MW2 cost col4 = 25 [V]); gun, launcher and AC-130 kills count; nuke kills do not; player death resets the streak; earned nuke kept until used (also across death). `nuke.always_available` for testing. |
| Nuke key | **4** (MW2 `+actionslot 4`, default PC bind `4` [V]; unused in `mw2er.toml` [V]); hidden from the game like 1/2. |
| Sequence | MW2 `_nuke.gsc` timeline: 10 s countdown with 1 s timer beeps, `nuke_incoming` at T-3.3, detonation sounds at T, `mpnuke` vision (3 s blend at T+0.25), kills at T+1.5, `mpnuke_aftermath` (5 s blend at T+1.5). Slow motion skipped (user). |
| Visuals | One new D3D12 post pass in the present hook (same infrastructure as the AC-130 thermal / tracer pass): white flash (stands in for MW2's `nuke_flash` fx at T) + MW2 bloom + film tint ramp, then aftermath tint. Camera shake = our addition, defaulting to the commented-out MW2 line `earthquake(0.6, 10, ...)` at MW2's `nukeEarthquake` time T+1.5. 3 horizon explosions at 127 m (MW2 nukeDistance 5000 units). |
| Kill method | **Engine SpEffect call** (`apply_speffect`, the same gated call as the AC-130 protection [V in game]) with a new one-shot SpEffect **79000960** = 100 % max-HP damage (clone of vanilla 17039 [V]). No bullets, no emitters, no HP writes. Optional bullet route (`nuke.kill_mode`). Retries for survivors. |
| Radius / friends | Radius 200 m default (MW2 has no radius: map-wide [V]); hostiles only by default via a **deny-list** of runtime `team_type` + set membership + system-dummy rule; `nuke.spare_friendly = false` gives MW2's "everyone dies". |
| Bosses | % max HP ignores boss HP size; retries at +0.5/1.5/3/5 s; survivors logged; event-immortal phase changes are a known limit. |
| Player | Dies like in MW2 (`nuke.player_dies = true`) through the same lethal SpEffect, after the protection SpEffect is removed and verified absent, after FP / MOVE / input / AC-130 are handed back. Normal YOU DIED / grace respawn. Never HP writes, never save writes. |

---

## 2. Part A: fixes

### 2.1 A1: the "SFX pool" is the bullet-emitter pool, and we leak one emitter per call

#### 2.1.1 Root cause [V]

`CSBulletManager+0x20` (`BufferAndAllocLinkedList<[u8;0x9d0],64,192>`, vendored name `unk_bullet_sfx_related`,
logged by M6 as `sfx`) is ER's **bullet emitter** list, not an SFX pool. Every `CSBulletManager::spawn_bullet` call
(vendored RVA 0x3a2cb0) allocates one emitter (64 preallocated + 192 heap = 256).

Disassembly of `eldenring.exe` 2.7.1.0 (image base 0x140000000, read from disk only):

| address | what it does |
|---|---|
| `0x1403a5a20` | spawn worker; `call 0x1403a7cc0` allocates an entry (`imul rdi,rcx,9D0h; add rdi,[rsi+20h]`); on null writes `[r14]=4` at `0x1403a5bd9` = **our code 4** |
| `0x14038fe90` | emitter init, `mov word ptr [rcx+328h],101h` (+0x328 active, +0x329 emitting) |
| `0x14038e550` | copies BulletSpawnData into the request; at `0x14038e66a`: `movzx eax,byte ptr [rdi+44h]; and al,1; mov [rbx+194h],al` (request copied to emitter+0x330, so this is emitter **+0x4C4 auto-finish**) |
| `0x14038ba2f` | request constructor default `mov qword ptr [rsi+194h],1`: ER's own fire-and-forget spawns have auto-finish = 1 |
| `0x1403900c0` | emitter update: alive = (+0x329 != 0) or (u32 +0x320 > 0); clears +0x329 only if `+0x4C4 != 0` (`0x1403901a3`) |
| `0x1403a983f` | per-frame loop: destroys an emitter whose update returned false |

Our `BulletSpawnData::new` (vendored crate + `tools\patches\fromsoftware-rs-0001-bullet-spawn-data.patch`) sets
`unk44 = 0`, so our emitters keep "emitting" forever and are only freed by a world reload.

Log evidence [V]:
- first code 4 after 254 (run 6), 256 (run 13), 253 (run 14) spawn calls since world entry;
- run 14 `SPAWN pool` lines: heap +23..26 per 50 `spawns_world` (bullet + engine marker child = 2 per call, so one
  emitter per call), never decreasing;
- gun rows have had every SFX at -1 since M6 and still cost one slot per call: the M6 hypothesis H1 (dagger hit SFX) is
  **refuted**; no row SFX audit can fix this;
- the AC-130 105 mm failed with code 4 at 23:26:30 and 23:26:39 (run 14) = "the AC-130 stopped doing damage";
- run 13: a reload at 20:19:47 cleared it (99 shots, 0 failures). Death alone did not.

Other free paths, none used by us: destroy-by-id `0x1403a3960(bm, id)` [I start address] (id = the `unk_out` the
wrapper discards), owner destruction, create-limit group replacement.

Field map confirmed against the request fill [V]: +0x08 behavior, +0x0C magic, +0x14 bullet_id, +0x18 goods,
+0x1C dummy_poly, +0x40 unk40, +0x44 flags (bit 0 auto-finish, **bit 3 network sync: keep 0**,
spawn function `test byte ptr [rsi+44h],8`).

#### 2.1.2 Fix

1. New patch **`tools\patches\fromsoftware-rs-0003-bullet-spawn-autofinish.patch`** (0002 is already taken by
   `recurring-task-no-panic`): `BulletSpawnData::new` sets `unk44 = 1` (bit 0 only). Make the field `pub` (doc
   comment: bit 0 auto-finish, bit 3 net sync - never set). The patch applier / check_m6 patch list gains 0003.
2. `spawn_bullet` wrapper returns `Ok(unk_out)` (the emitter id; != -1 on success, so existing Ok/Err logic is
   unchanged). Store the last ids in the spawn ring for logs. Destroy-by-id is **not** wired in M7 (address [I]);
   reserved as a fallback.
3. Logs: the pool is called `emitters` (the `sfx=` key stays on `SPAWN pool` lines so old analyzer regexes match).
4. Expected in game [I]: `SPAWN pool ... sfx heap 0` with `sfx_empty_spot=1` for the whole session; peak emitters
   tens, not hundreds; AC-130 rows spawn and damage again; no code 4.

Gotchas: engine-made children (HitBulletID marker) do not use an emitter; only our top-level calls do (a launcher
core + ring = 2). Even after the fix, 256 is the cap on **simultaneously live** emitters, so the nuke and launchers
still stagger.

#### 2.1.3 Emitter walk + watchdog (DLL, read-only)

`game/bullets.rs` `pool_snapshot` gains a walk from `bm+0x28` through `node+0x998` (MAX_WALK 300; node chaining [V]
at `0x1403a5bb7..0x1403a5bd2`):

- `live` = nodes walked; `ours` = node+0x340 (owner FieldInsHandle [V]) == player handle and node+0x334 (bullet_id)
  in our id set; `persistent` = node+0x329 != 0 and node+0x4C4 == 0; `max` = session max.
- Line: `SPAWN emitters live=<n> ours=<m> persistent=<k> max=<session max> cap=256` with every `SPAWN pool` line
  and once on world enter.
- Watchdog (pure logic `mw2er-core::emitter_guard`, DLL applies it):
  - `live >= 192`: `SPAWN WARN emitters live=.. ours=.. persistent=..` once per crossing.
  - `live >= 224`: throttle low-priority spawns (shotgun pellets after the first, launcher ring tier), each refusal
    logged and merged per second: `SPAWN throttle why=emitters live=.. dropped=<n> kind=pellet|ring`.
  - 32 slots are reserved for priority spawns (AC-130 rows, nuke). A priority spawn is refused only at 256 and then
    logs `SPAWN FAIL code=4 cause=pool_emitters_full ...` (never silent).
  - `persistent > 0` after the fix = regression: `SPAWN WARN persistent=<k>` (analyzer FAIL).
- Thresholds in config `[spawn] emitter_warn = 192, emitter_throttle = 224, emitter_reserve = 32`.

#### 2.1.4 A1 oracles

1. Vendored unit test: `BulletSpawnData::new` as `[u8;0x110]`: byte 0x44 == 0x01, 0x45..0x48 == 0, every other
   unknown field 0.
2. Static exe pin in check_m7 (same precedent as check_m5's iw4dis): build `tools\iw4dis` with a **short**
   `CARGO_TARGET_DIR` (e.g. `work\t`; long scratchpad paths fail), run
   `iw4dis <eldenring.exe> x 0x14038e66a-0x14038e676` and `x 0x1403901a3-0x1403901b3`; expect
   `movzx eax,byte ptr [rdi+44h] / and al,1 / mov [rbx+194h],al` and `cmp byte ptr [rdi+4C4h],0 ... mov byte ptr
   [rdi+329h],0`. Mismatch = exe changed = FAIL. Read-only.
3. `mw2er-core::emitter_pool` model + long synthetic bench (`cargo test emitter_bench`): one emitter per call, freed
   when `auto_finish && live_bullets == 0`, bullet life from rows. 20 000 gun shots at 1000 rpm + 8-pellet shotgun
   mags + 50 launcher shots + 3 AC-130 rides + 1 nuke: with the flag peak < 64, 0 failures; without it call 257
   fails (calibrated: runs 6/13/14 failed at 254/256/253 counted calls; tolerance ±4 for pre-existing emitters).
4. Watchdog tests: warn/throttle/reserve transitions, priority spawn still allowed in 224..255, every refusal yields
   one log event.
5. Existing `bullet_sfx` checks stay unchanged (rows are not the cause).

### 2.2 A2: P90 interval 17 ms < fireTime 65 ms [V]

`mw2er-core/src/weapon_sim.rs:540-543` (`fire_due`) resets `next_shot` only when overdue by more than one fireTime.
Run 14: shot 234 at 40.178, 235 at 40.295 (117 ms hitch), 236 at 40.312. After 235, next_shot = 40.307 so the
tick at 40.312 fires again.

Fix: before the fire loop, `self.next_shot = self.next_shot.max(now - 0.15 * fire_time_s)`; no interval below
0.85 x fireTime. Tests: ticks at t, t + 1.8 ft, t + 1.8 ft + 1/60 s give an interval >= 0.85 ft for every gun's
fireTime; the existing median-within-one-frame cadence tests at 20/60/144 Hz still pass; `analyze_m6.py:243`
`shots_per_gun` stays strict.

### 2.3 A3: reload / rechamber start without done (real bug) [V]

(a) Switch: `task.rs:1513-1523` cancels the reload, but the outgoing sim keeps stepping during the drop with
`hold = true` (`task.rs:762`); auto-reload at `weapon_sim.rs:504` ignores `hold` and starts again in the same tick;
the gun is parked mid-reload (`task.rs:1491`). Run 14: `RELOAD cancelled (switch)` followed by `RELOAD start empty=1`
at 23:23:03.5 (stinger), 09.4 (rpg), 17.8 (m79), 27.8 (at4), 25:11 (cheytac); a later `RELOAD done` when the RPG
came back (23:23:18.7).
Fix: `&& !self.hold` in the auto-reload condition; `cancel_reload()` on the outgoing sim right before parking. On
raise, an empty gun starts its reload normally (logged `RELOAD start`, then `done`).

(b) `cancel_reload` (`weapon_sim.rs:362-364`) drops a rechamber silently (model1887 23:23:37.8, spas12 23:23:57).
Fix: emit `SimEvent::RechamberCancelled`; log `RECHAMBER cancelled gun=<g> reason=switch|sprint|mode_off|ac130|nuke`
(task.rs next to :1522). Gun stays `chambered=false` and rechambers after raise (M6 rule).

Also cancel cleanly with a log on: sprint start (existing), MODE off, AC-130 start, nuke player-death prep (reason
names above).

Tests: empty-mag switch -> no `RELOAD start` during the drop; switch during rechamber -> `RechamberCancelled`;
every start pairs with done or cancelled over 1000 random switch/sprint/fire sequences (property test).

### 2.4 A4: analyzer rows for arsenal sessions [V lines]

- `analyze_log.py:477-478`: accept `ARSENAL loaded n=.. ok=..` / `ARSENAL data` as "viewmodel assets ready".
- `analyze_log.py:505-506` "RELOAD start without ->reload": pair with the arsenal anim lines; accept starts during
  `SWITCH` raise and launchers without a reload clip; after A3 there must be no starts during a drop.
- `analyze_log.py:1550-1557`: compare `done mag=` with each gun's clip from the ARSENAL context, not UMP45's.
- `analyze_m6.py:398-401` `judge_rechamber`: start -> done **or** start -> `RECHAMBER cancelled`.
- New rows in section 5.

---

## 3. Part B: the Tactical Nuke

### 3.1 MW2 reference [V, `dumps\m7`]

| fact | value / source |
|---|---|
| cost | killstreaktable.csv row `nuke` col4 = **25** (col5 = 15 used only by commented-out code) |
| counting | patch_mp `_damage.gsc` 230-312: streak++ only while the attacker is alive; killstreak-weapon kills count only if that streak was earned in the current life; death resets (`cur_kill_streak = 0`); each streak earned once per life |
| key | `+actionslot 4`, PC default `bind 4 "+actionslot 4"`; hint `Press [{+actionslot 4}] for TACTICAL NUKE.` |
| refusals | `Tactical Nuke already inbound!` while one is in flight; refused while using a remote (AC-130) |
| timer | `scr_nukeTimer` 10 s; `scr_nukeCancelMode` 0 = caller's death does not cancel |
| beeps | `ui_mp_nukebomb_timer` every 1.0 s from activation |
| T-3.3 | `nuke_incoming` |
| T | `nuke_explosion` + `nuke_wave`, slow motion (skipped), killstreak vehicles destroyed, `nuke_flash` fx 5000 units (127 m) ahead of each player |
| T+0.25 | `visionSetNaked('mpnuke', 3)` |
| T+1.5 | `nuke_death`: every living player incl. the caller takes 999999 MOD_EXPLOSIVE through the normal damage function (no team check, no radius); `mpnuke_aftermath` 5 s blend |
| T+1.5 | `nukeEarthquake` thread (same time as `nuke_death`), but its body is commented out |
| shake | **none** in MW2: `nukeEarthquake()` body commented out (`earthquake(0.6, 10, nukepos, 100000)`, TODO) |
| white-out | no white quad: `mpnuke` bloom intensity 4.02646, cutoff 0.315552, radius 3.13324, desat 0.23623; film contrast 1.67, brightness 0.0189, desat 0.48, light tint (2, 1.3053, 0.672409), dark tint (0.749927, 0.909935, 1.08943) |
| aftermath | `mpnuke_aftermath` bloom 0.45 / cutoff 0.65 / radius 6.07651 / desat 0.65; film contrast 1.8, brightness 0.05, desat 0.58, light (1, .969, .9), dark (0.7, 0.3, 0.2) |
| HUD | countdown `S.t` (scorebar_hd.menu rect 112 42 100 30 on 640x480, textscale 0.3333, glow 1 0.25 0 1, no forecolor = IW4 itemDef default white); icon `dpad_nuke` 384x192; strings `Tactical Nuke`, `Friendly tactical nuke inbound!` |

ER adaptations (our decisions, not MW2 data): radius, friendly filter, camera shake, the flash drawn full-screen in
the post pass (MW2's flash is the `nuke_flash` fx 127 m ahead of each player at T: the flash is MW2 behaviour, its
full-screen form and rise/hold/decay shape are ours), horizon ER explosions, no endGame.

### 3.2 Killstreak counter (`mw2er-core::killstreak`, pure)

State: `streak: u32`, `earned: bool`, `earned_this_life: bool`, `life_id: u32`, `in_sequence: bool`.

Rules:
- +1 per kill event the DLL already attributes: `KILL` (arsenal gun incl. pellets, launcher/projectile explosions
  logged as kills) and `AC130 KILL`. One per victim handle (dedupe within 2 s by handle).
- Not counted: nuke kills; `HPDROP`-only deaths with no MW2 attribution (ER weapons, bleed from ER gear, falls);
  kills credited while the player is dead (hp == 0) - MW2 "attacker alive" rule; kills while MW2 mode is off (the
  MW2 guns are not out).
- AC-130 kills always count (ours is called by F7 within the same life by construction; MW2 lifeId rule satisfied).
- Player death (hp edge > 0 -> 0, the same detector as `FP off reason=death`) -> `streak = 0`, `life_id++`,
  `earned_this_life = false`. A loading screen without death does not reset.
- `streak >= cost` (`nuke.streak_cost`, default 25) and `!earned_this_life` and `!in_sequence` -> `earned = true`,
  `earned_this_life = true`, earn sounds + hint; log `NUKE earned streak=25 life=<id>`.
- `earned` is kept until used, also across death (MW2 keeps earned streaks) [I MW2 behaviour, config
  `nuke.keep_earned_on_death = true`]. Not persisted across game restarts (DLL memory only; no save writes).
- Activation -> `earned = false`, `in_sequence = true`; the streak keeps counting during the countdown but cannot
  earn; at the end of the sequence `streak = 0` (and the player death resets it anyway).
- `nuke.always_available = true`: activation always allowed when not in a sequence; HUD shows `READY (test)`;
  log lines carry `test=1`.

Tests: 25 kills -> earned once; 50 kills in one life -> earned once; death at 24 -> 0; kill while dead ignored;
duplicate kill events on one handle ignored; nuke kills ignored; earned survives death; always_available.

### 3.3 HUD (existing imgui HUD, `hud.rs` + `hud_geom.rs`)

- Counter while MW2 mode is on (not in the AC-130): bottom right, MW2 style `Nuke: 17/25` (white, shadow);
  when earned: `TACTICAL NUKE  [4]` in MW2 orange (glow 1 0.25 0); optional `dpad_nuke` icon (384x192 DDS from
  `dumps\m7\hud`, converted to the overlay texture format used by the sniper scope; `nuke.hud_icon = true`).
- Earn hint 4 s: `Press [4] for TACTICAL NUKE.`
- Countdown: top centre-left at the scorebar rect scaled from 640x480 to the swap chain (4K: x 720, y 189),
  `S.t` (e.g. `9.4`) in red 0.85 0.4 0.4 with orange glow; under it `Friendly tactical nuke inbound!` for 3 s.
- Refusals 2 s: `Tactical Nuke already inbound!`, `Not available in the AC-130`.
- Hidden from T (detonation) on; ER's own HUD untouched (no `hide_er_hud`).

### 3.4 Key

`[keys] nuke = "4"`, blanked from the game like weapon_next/prev (DirectInput blanking in `input_hook.rs`). The key
config conflict check (M5/M6) covers it: not among toggle/F6/F7/Back/Q/1/2/3/F/WASD/LSHIFT/SPACE/C/LCTRL/R. ER's
default keyboard layout does not use 4 [I: verify with `MOVE keyprobe` style line once with MW2 off].

### 3.5 State machine (`mw2er-core::nuke_logic`, pure, ticked by the DLL)

```
Idle --(key 4, earned|always_available, gates ok)--> Arming (0.0..0.8 s: detonator press sound)
Arming --> Countdown (t = 0 .. 10 s)
Countdown --(t >= 10)--> Detonate (T)
Detonate --> Flash (T .. T+1.5)          post pass flash/white-out, shake, horizon SFX
Flash --(T+1.5)--> Killing (T+1.5 .. T+6.5) kill pass + retries
Killing --(T+1.5+player_delay)--> PlayerDeath (prep, protection check, lethal, watch)
PlayerDeath/Killing done --> Aftermath (vision hold, fade) --(loading screen or aftermath end)--> Idle
```

Gates at activation (refusal logs `NUKE blocked reason=...`): `inbound` (sequence active), `ac130` (ride active or
starting), `loading`, `dead` (hp == 0), `mode_off` (MW2 mode off), `not_earned`. Allowed while sprinting, on Torrent,
in ER menus (ER does not pause).

Cancel paths (decision):

| event | during Arming/Countdown | after Detonate |
|---|---|---|
| loading screen / world exit / quit to title | cancel, **refund** (`earned = true`), stop sounds, `NUKE cancel reason=loading refund=1` | stop kill retries, end post pass, `NUKE end reason=loading` |
| player dies (enemy kill) | continue (MW2 cancelMode 0); if the death loading starts first -> loading row | continue |
| F5 MW2 mode off | blocked while a sequence is active: `MODE toggle blocked reason=nuke` | blocked until Idle |
| F7 AC-130 | blocked: `AC130 blocked reason=nuke` | blocked |
| config hot reload `nuke.enabled=false` | finishes the running sequence; new activations refused | - |
| DLL watchdog (any stage > 30 s) | force `Aftermath -> Idle`, restore all state, `NUKE WARN stuck stage=..` | same |

Tests: every transition, each cancel, refund, re-entry refused while active, time steps 20/60/144 Hz plus 500 ms
hitches (events fire once, in order, never skipped).

### 3.6 Timeline and sounds

| t (from activation) | event | sound (converted to `converted\nuke\`) |
|---|---|---|
| 0.0 | activation | `weap_c4detpack_trigger_plr` if found in the dumps (NEEDS-DATA: converter looks it up; skip if absent), `US_1mc_use_tnuke_02` (2.594 s, 22.05 kHz) |
| 0.0, 1.0 ... 9.0 | timer beep | `ui_mp_nukebomb_v2` (stereo 44.1 kHz, 0.997 s, vol 0.3255); the t = 10 beep is dropped (races with detonation) |
| 6.7 | incoming | `incoming_mp_nuke1` (mono, 4.115 s, vol 0.723) |
| 10.0 = T | explosion chain | random `exp_105mm_boom1..5` (shared with the AC-130 105 mm copies) + `exp_underground_low01..03` (+100 ms) + `exp_105mm_crack_debris1..3` (streamed, iw_06) + `exp_building_big_hit1..3` + `airlift_nuclear_exp` (+150 ms, streamed, 9.459 s, vol 0.93); IW4 secondary aliases start together with their own start delays [I] |
| T | wave | `airlift_nuclear_aftmath` (30 s stereo, +1500 ms start delay, vol 0.9207) |
| T+0.25 | `mpnuke` white-out, 3 s blend | - |
| T+1.5 | enemy kills; `mpnuke_aftermath` 5 s blend | - |
| T+1.5+player_delay | player death | - |
| earn | streak reached | `mp_killstrk_nuclearstrike` (1.834 s) + `US_1mc_achieve_tnuke_01|02` (random) |

Volume and pitch per alias from the dump CSVs (known answers in 6.4). The announcer voice set
`nuke.announcer = "US" | "UK" | "none"` (UK files under `battlechatter/TF`). Playback through the existing `audio.rs`
mixer (M4 AC-130 path); the 30 s wave streams from memory (5.3 MB PCM).

### 3.7 Post pass: flash, white-out, aftermath (`mw2er-render::nuke`, `shaders\nuke.hlsl`)

Runs in the present hook (`thermal_present.rs`) after the tracer/thermal slots, on the swap-chain back buffer,
only while the nuke post state is active. Reuses the thermal pass's copy/SRV/RTV plumbing, root signature style
and descriptor heap.

Per-frame constants from `nuke_logic` (pure, tested): `flash` (0..1), `vision` (0 = none, blend a between
`mpnuke` and `mpnuke_aftermath`), `fade` (overall amount).

Pipeline (target < 1 ms at 3840x2160 on the RTX 5090):
1. Bright pass + 2 downsample levels at 1/4 and 1/8 res (cutoff from the vision), separable 9-tap blur scaled by
   `r_glowRadius0`, desaturated by `r_glowBloomDesaturation`.
2. Composite: `c = scene + bloom * intensity`.
3. Film (MW2 `r_film*` model [I formula]): `l = luma(c)`; `c = lerp(c, l, desat)`; `c = (c - 0.5)*contrast + 0.5
   + brightness`; `c *= lerp(darkTint, lightTint, l)`.
4. Flash: `c = lerp(c, 1, flash)`; `flash` = 0 -> 1 in 0.08 s at T, hold 0.25 s, then decays over 2.5 s into the
   `mpnuke` white-out (`nuke.flash = true`; stands in for MW2's `nuke_flash` fx at T, the full-screen form is ours).
5. Vision ramps: `mpnuke` 0 -> 1 over 3 s from T+0.25; aftermath blend over 5 s from T+1.5; aftermath held
   `nuke.aftermath_hold_s` (8 s) then faded over 2 s; also ended at the death loading screen at the latest.

The pass draws after ER's UI (present-time), so YOU DIED is tinted too; that is accepted (MW2's vision stays into
the end screen) and ends at loading.

### 3.8 Camera shake

MW2 has none (commented out). Ours, `nuke.shake = true`: reuse `mw2er-core::ac130_shake` (MW2 earthquake model)
with the commented-out MW2 values scale 0.6, duration 10 s from T+1.5 (MW2 threads `nukeEarthquake` at
`level.nukeTimer + 1.5`; `convert_nuke.py` extracts that delay as `timeline.quake_delay_s`), applied in `cam_hook.rs`
to the player camera in first and third person. Config `nuke.shake_scale = 0.6`, `nuke.shake_s = 10.0`. The shake
stops at the player's death (`NUKE player dead`) and on loading.

### 3.9 Horizon explosions (emitter budget)

`nuke.horizon_count = 3` spawns at T, T+0.15, T+0.3: 127 m (5000 units) ahead of the camera, flattened forward
(MW2 placement), ±20 deg yaw spread, ground height = player height + 30 m [I visible]. New visual-only Bullet row
**79000962** (clone of the AC-130 105 core visuals, `atkId_Bullet = -1`, no damage [I accepted by ER: oracle checks
the row, run 15 checks it spawns]). Spawned with priority, and only if `emitters live < 160`; otherwise skipped with
`NUKE horizon skipped why=emitters live=..`. Total cost 3 emitters, freed with the fix.

### 3.10 Kill method (enemies)

Primary (`nuke.kill_mode = "speffect"`, default): `apply_speffect(chr, 79000960, dont_sync = true)` - the same engine
call (RVA 0x3e8dc0) used for the AC-130 protection, verified in run 14 (`AC130 PROTECT apply id=79000950 ...
present=1 ok=1`). Move `apply_speffect`/`remove_speffect` out of `ac130.rs` into `game/` (one gated wrapper:
`WRITES_ALLOWED`, `plausible(chr)`). ER then runs its own HP loss and death, like a lethal bleed proc [I]. No
bullets, no emitters, no HP writes.

Optional `nuke.kill_mode = "bullet" | "both"`: per-target lethal bullet **79000961** (Bullet + AtkParam_Pc, owner =
player, all SFX -1, `spEffectId0 = 79000960`, opposeTarget 1, friendlyTarget 0, selfTarget 0, isDisableNoDamage 1,
disableGuard 1, flat atkPhys 9999 as backup, tiny radius 0.5 m) so ER sets `last_hit_by`; costs 1 emitter per
target, at most `nuke.bullets_per_tick = 8` and never above `emitter_throttle`. `both` = bullets at T+1.5, SpEffect on
survivors at +0.5 s. Use only if run 15 shows no runes from SpEffect kills.

Stagger: `nuke.kills_per_tick = 16` (spreads ragdoll / item-lot work), nearest first.

Retries: survivors (hp > 0 and no death_flag) re-applied at +0.5, 1.5, 3 and 5 s; re-scan each time (phase-2
characters spawn as new ChrIns). After the last retry: `NUKE survivor handle= npc= chr=c#### team= hp= max_hp=
why=hp>0|invincible|immortal?`.

Bosses: 100 % max HP ignores large HP (flat atk is u16 max 65 535; vanilla NPC max 9999 [V]). Do not use HP-burn
rows (`hpBurnDamageRate` 50/35 on 47 boss rows [V]). Event-immortal phase transitions (Malenia, Godfrey/Hoarah Loux,
Rykard, Fire Giant, Radagon/Elden Beast) stop at 1 HP or heal; a cutscene phase change cannot be forced - known
limit, logged. Runes, drops and defeat flags come from ER's own death bookkeeping (`has_dropped_runes`) and event
scripts watching the death [I].

### 3.11 Target selection and friendly exclusion (`mw2er-core::nuke_targets`, pure)

Source: `WorldChrMan.chr_inses_by_distance` read with `chars::read_nearby(max = 1024)` (not `hit.max_chars = 64`),
union by handle with `chr_sets[0..chr_set_holder_count]` + `open_field_chr_set` (entries with `chr_load_status ==
Active`). Distance from `CSChrPhysicsModule.position` (the by_distance field is not metres). Census at T+1.5:
`NUKE census by_distance=N chrsets=M union=K in_radius=R by_team={6:..,7:..}`.

Eligible = all of:
- plausible ChrIns, `hp > 0`, `max_hp > 0`, death_flag (1c5 bit 7) clear, is_active (1c8 bit 4) set;
- distance <= `nuke.radius_m` (200);
- not the main player; not in player / ghost / summon_buddy (spirit ashes, Torrent) / debug sets;
- `npc_param_id >= 20_000_000` (skips Bonfire, Talk Dummy incl. 10006050 team 33, BuddyStone, Caravan Dummy);
- character id not in `hit.exclude_character_ids` (8000 Torrent);
- `nuke.spare_friendly = true` (default): runtime `team_type` (ChrIns +0x6c [I offset]) not in `nuke.spare_teams =
  [0, 2, 8, 10, 12, 20, 26, 28, 30, 47]`; chr_type not a phantom/player type. Deny-list, not allow-list: bosses use
  15 team types (33 Malenia, 64 Ghostflame Dragons, 51, 48, 1 ...) [V NpcParam scan]. A merchant made hostile
  (26 -> 27) is killed - runtime value.
- `is_invincible` (1c5 bit 4) set -> deferred to the retry pass, not skipped.

`spare_friendly = false` = MW2 (everyone in radius dies, merchants included; Torrent and summons still skipped -
they are part of the player). Every skip logged once per handle with reason (merged counts per reason). Characters in
unloaded blocks have no ChrIns and cannot be killed (normal enemies respawn after the player's death anyway).

### 3.12 Player death and state restores

Order (all pure-logic steps in `nuke_logic`, DLL does the calls):
1. At T: if an AC-130 ride is somehow active (should be blocked), end it via the existing ride-end path (camera,
   HUD, input blanking, protection removal) - MW2 also destroys vehicles at detonation.
2. At `T+1.5+player_delay-0.5` (`nuke.player_delay_s = 3.0`, after enemy kills and their defeat events):
   cancel reload/rechamber (`reason=nuke`), release fire/ADS, hide the viewmodel, `FP off reason=nuke`,
   `MOVE blocked reason=nuke` (movement controller hands control back to ER; it holds no gravity write [V]),
   release input blanking except keys 4/F5.
3. Remove protection SpEffect 79000950 if present (noDead = 1 would block death) and verify `present=0`; abort the
   player kill with `NUKE WARN player_protected` if it is still present after 3 tries.
4. `apply_speffect(main_player, 79000960)` at `T+1.5+player_delay`.
5. Watch: hp == 0 then death_flag; if still alive after 1 s re-apply (max 3); then `NUKE WARN player_not_dead` and end
   the sequence normally (no HP write, ever).
6. ER's YOU DIED / grace respawn runs as usual; FP / MOVE re-arm through their existing post-death paths. Runes
   (incl. those from the nuke) drop as a bloodstain - normal ER rule, mentioned to the user.

`nuke.player_dies = false` skips 3-5 (sequence ends with the aftermath).

### 3.13 Regulation rows (`build_regulation.py`, `[regulation.nuke]`)

| row | content |
|---|---|
| SpEffectParam **79000960** | clone of vanilla 17039: effectEndurance 0, changeHpRate 100.0, changeHpPoint 0, effectTargetSelfTarget 1 + all effectTarget* 1 (as 13462), spAttribute 254, noDead 0, dontDeleteOnDead 0, isHpBurnEffect 0, isDestinedDeathHpMult 0, vfxId -1, stateInfo 0 |
| Bullet + AtkParam_Pc **79000961** | lethal bullet (kill_mode bullet/both), section 3.10 |
| Bullet **79000962** | horizon visual (atkId -1) |

SAFETY_M7 asserts the fields above; rows sorted; all existing rows (legacy M2, AC-130 79000201-401 + 79000950,
arsenal 791xxxxx, marker 79000900) byte-identical.

### 3.14 Config `[nuke]` (hot unless noted)

```
enabled = true
always_available = false
streak_cost = 25
keep_earned_on_death = true
radius_m = 200.0
spare_friendly = true
spare_teams = [0, 2, 8, 10, 12, 20, 26, 28, 30, 47]
kill_mode = "speffect"        # speffect | bullet | both
kills_per_tick = 16
bullets_per_tick = 8
retry_s = [0.5, 1.5, 3.0, 5.0]
player_dies = true
player_delay_s = 3.0
flash = true
aftermath_hold_s = 8.0
shake = true
shake_scale = 0.6
shake_s = 10.0
horizon_count = 3
announcer = "US"
hud_icon = true
```
plus `[keys] nuke = "4"`, `[spawn] emitter_warn/throttle/reserve`, `[regulation.nuke]` ids (not hot).

---

## 4. Log lines

A1: `SPAWN emitters live= ours= persistent= max= cap=256`, `SPAWN WARN emitters ...`, `SPAWN WARN persistent=`,
`SPAWN throttle why=emitters live= dropped= kind=`, `SPAWN FAIL code=4 cause=pool_emitters_full ...`.
A3: `RECHAMBER cancelled gun= reason=`, `RELOAD cancelled (reason)`.
Nuke: `NUKE streak n=17/25 src=gun|launcher|ac130`, `NUKE streak reset reason=death n=`, `NUKE earned streak= life=`,
`NUKE blocked reason=`, `NUKE activate test=0|1`, `NUKE countdown t=10`, `NUKE incoming`, `NUKE detonate`,
`NUKE horizon spawned=n` / `skipped`, `NUKE census ...`, `NUKE kill handle= npc= chr= team= hp=a->0 max_hp= t_ms=`,
`NUKE skip reason=<team|set|dummy|torrent> n=`, `NUKE survivor ...`, `NUKE kills total= killed= survivors=
skipped= t_ms=`, `NUKE player prep fp=off move=off protect_present=0`, `NUKE player lethal ok=1`, `NUKE player dead
t_ms=`, `NUKE WARN player_protected|player_not_dead|stuck`, `NUKE cancel reason= refund=`, `NUKE end reason=`,
`NUKE post gpu_ms=` (once per sequence, max).

## 5. Analyzer rows (`analyze_log.py`, `analyze_m7.py`)

| row | PASS when |
|---|---|
| emitter pool bounded | `persistent=0` everywhere; `max` < 192; `sfx heap` 0 or falling |
| no code 4 without cause | every `SPAWN FAIL` has `cause=`; 0 `code=4` in a session with the fix |
| AC-130 damage restored | every `AC130 EXPLODE` without `SPAWN FAIL`; >= 1 `AC130` HIT per ride with targets |
| P90 / cadence | no interval < 0.85 x fireTime (shots_per_gun) |
| reload pairing | every `RELOAD start` -> done/cancelled; no start during a drop |
| rechamber pairing | start -> done or `RECHAMBER cancelled` |
| ARSENAL viewmodel | `ARSENAL loaded n=34 ok=34` accepted |
| nuke streak | counts +1 per KILL/AC130 KILL, reset on death, earned at 25 once per life |
| nuke sequence | activate -> countdown -> incoming (~6.7 s) -> detonate (10.0 ± 0.1 s) -> kills (T+1.5 ± 0.1) |
| nuke kills | killed + survivors + skipped = in_radius; 0 skips with reason `none` |
| nuke player | `protect_present=0` before lethal; `player dead` within 2 s of lethal; no WARN |
| nuke post GPU | gpu_ms < 1.0 |
| nuke cancel | every cancel has a reason; refund on loading |

NEEDS-DATA when the step was not done.

## 6. Offline oracles (pass criteria)

### 6.1 Pure logic (`cargo test --workspace`, 0 failed)
`emitter_pool` bench + `emitter_guard` (2.1.4), A2 cadence clamp, A3 hold / RechamberCancelled / pairing property
test, `killstreak` (3.2), `nuke_logic` (every transition, cancel, refund, hitches, watchdog), `nuke_targets` (census
team table from the vanilla scan as fixtures: Torrent 80000000 team 10 skipped, Malenia team 33 killed, Talk Dummy
10006050 skipped by id rule, merchant 26 skipped / 27 killed, wolves 48 killed, spirit 47 skipped,
spare_friendly=false), stagger under caps (kills_per_tick, bullets_per_tick never above emitter throttle, horizon
gate at 160), retry schedule, player death order (no lethal before protection absent, prep before lethal),
post-pass ramps (flash/vision curves known values at T, T+0.25, T+1.5, T+3.25, T+6.5), shake values, config parse /
defaults / key conflicts (4 free) / unknown key error.

### 6.2 Regulation round trip
`build_regulation.py --check`: `REGULATION PASS ... speffect+2` (79000950, 79000960) + nuke bullet/atk rows; self-test
mutations caught (changeHpRate 99, endurance -1, noDead 1, isHpBurnEffect 1, dontDeleteOnDead 1); untouched params
byte-identical; UMP45 / AC-130 / arsenal rows unchanged; rows sorted.

### 6.3 vmtest nuke pass (offscreen D3D12)
3840x2160, debug layer on: 0 errors / 0 warnings; known-answer pixels: black input + flash 1 -> white; grey 0.5
input with `mpnuke` full -> orange-white (R > G > B, R >= 0.95); aftermath full on grey -> shadows R > G > B
(0.7, 0.3, 0.2 tint); flash 0 and vision 0 -> output == input; GPU timestamp < 1.0 ms median over 120 frames.

### 6.4 Sound conversion known answers (`convert_nuke.py --check`)
Every alias in 3.6 resolves; sample counts = data / block align; durations: incoming 4.115 s, timer 0.997 s,
aftmath 29.997 s, airlift_nuclear_exp 9.459 s, killstreak jingle 1.834 s, US use 2.594 s (±1 ms); rates 44.1 kHz
except announcer 22.05 kHz; PCM sha equals source data chunk; boom1..5 byte-identical to the AC-130 copies;
deterministic.

### 6.5 Build, proof strings, hygiene
cargo build 0 warnings; patch 0003 applies cleanly on the pinned vendored commit; DLL proof strings (built ==
staged): `SPAWN emitters`, `SPAWN throttle`, `RECHAMBER cancelled`, `NUKE activate`, `NUKE detonate`, `NUKE kill `,
`NUKE survivor`, `NUKE player lethal`, `NUKE cancel`; hygiene (no MW2 data in source, no WriteProcessMemory, writes
only via game::write / gated engine calls, no raw HP write in nuke code - grep oracle on `hp` stores), no game-folder
writes (mtime scan).

## 7. `tools\scripts\check_m7.ps1`

`check_m7.ps1 [-AllowSkip] [-SkipM6]`, PowerShell 5.1, same structure as check_m6; writes only `work\check_m7\`,
`work\m7\`.

| # | check | pass |
|---|---|---|
| 1 | `check_m6.ps1` (runs m5/m4/m3b/m3/m2) | exit 0 |
| 2 | vendored patches 0001..0003 apply; BulletSpawnData byte test (0x44 == 1) | PASS |
| 3 | exe static pin (iw4dis, short CARGO_TARGET_DIR, 2 ranges) | both match |
| 4 | sound conversion (6.4) | PASS |
| 5 | regulation (6.2) + sha pin `work\m7\regulation.sha256` | PASS |
| 6 | cargo build 0 warnings | 0 |
| 7 | cargo test incl. 6.1 categories by name and emitter bench | 0 failed |
| 8 | vmtest nuke (6.3) | PASS |
| 9 | config (`[nuke]`, `[spawn]`, key 4 conflict check) | PASS |
| 10 | wiring (`m2_static_checks.py nukewiring`: lethal only after protection-absent, no HP store, spawn priority path) + mutations | all caught |
| 11 | DLL proof strings | all present, built == staged |
| 12 | analyzer fixtures (`run14_leak.log` -> A1 rows FAIL as expected; synthetic `m7_nuke_session.log` -> PASS; mutations >= 10 caught) | PASS |
| 13 | hygiene + no game-folder writes | PASS |

Final line: `M7 OFFLINE ORACLES: PASS`.

## 8. In-game test (run 15) - the user runs it; Claude never launches

1. **Leak fixed**: in one world, without reloading, fire > 600 rounds (AA-12 + UMP45 + P90), 5 launcher shots, then
   F7 AC-130 and use all three guns on enemies. Expect `SPAWN emitters ... persistent=0`, `max` in the tens, 0
   `SPAWN FAIL`, AC-130 HITs and kills.
2. **P90**: hold fire a full mag; analyzer cadence row PASS.
3. **Reload/rechamber**: empty a launcher and switch away during the auto-reload; switch during an SPAS / 1887 pump.
   Expect `RELOAD cancelled (switch)` with no new start until raise, `RECHAMBER cancelled gun=.. reason=switch`.
4. **Test nuke, dry run first** (`nuke.dry_run = true`, the shipped default): set `nuke.always_available = true`
   near a group of enemies and a merchant (e.g. Kale at Church of Elleh). Press 4. Expect countdown HUD, beeps,
   incoming at ~6.7 s, flash, white-out, shake from T+1.5, horizon explosions; **no enemy dies**: the log shows
   `NUKE kill dry_run=1`, `NUKE kill would-apply ... team=` for each enemy and `NUKE skip reason=team n=` for Kale; you
   die ~3 s later (YOU DIED, respawn at grace). Send the log; the analyzer's `nuke kills` row is NEEDS-DATA (dry run).
4b. **Armed nuke**: only after 4 shows Kale (and other friendlies) under `skip reason=team` and no would-apply on a
   spared team, set `nuke.dry_run = false` (hot) and repeat: enemies die at T+1.5 (`NUKE kill armed`, `NUKE kill
   handle= ... hp=..->0`), Kale alive. A `NUKE blocked reason=team_unverified` line means the team byte looks wrong:
   nothing was killed; send the log.
5. **Earned nuke**: always_available false; get 25 kills without dying (counter `Nuke: n/25`), check the earn sound,
   hint and `NUKE earned`; die once at a lower count to see the reset.
6. **Boss**: a field boss in range (e.g. Tree Sentinel) dies; check the defeat message / rune drop. Optionally a
   phase boss to see `NUKE survivor`.
7. **Cancels**: activate, then fast travel during the countdown (`NUKE cancel reason=loading refund=1`); F5 and F7
   during a countdown are refused.
8. Close normally; run the analyzer.

## 9. Verified vs inferred

Verified: emitter mechanism and code-4 source (disassembly), leak rate and calibration (logs), H1 refuted, AC-130
failures caused by it, A2 arithmetic, A3 code paths, A4 stale rows, all MW2 nuke data (timeline, cost, counting rules,
key, sounds, visions, HUD), `apply_speffect` works in game, protection row has noDead 1, vanilla lethal row fields,
NpcParam team distribution, FP/MOVE death hand-back in game.

Inferred (run 15): AC-130 damage restored after the fix; the emitter counter bound vs the walked count (both on
`SPAWN WARN emitters` / `SPAWN FAIL ... upper= walked=`); +0x320 = live-bullet count; destroy-by-id address; lethal
SpEffect gives a normal death with runes, drops and boss flags; team_type offset 0x6c; by_distance completeness;
phase-boss behaviour; player YOU DIED via SpEffect; visual-only bullet with atkId -1; ER ignores key 4; MW2 film
formula; IW4 secondary alias timing; earned streak kept across death (MW2 behaviour, not read from the dumps).

## 10. Housekeeping
The A1 research build left `C:\t\erdis` (iw4dis build output, outside the project); a safety check blocked its
removal - the user should delete `C:\t\erdis` and the empty `C:\t`. check_m7 uses `work\t` as its short target dir.

## 11. As built (2026-10-18) and deviations

Offline result: `tools\scripts\check_m7.ps1` 13/13 PASS, `M7 OFFLINE ORACLES: PASS` (check_m6 12/12 inside, which
runs m5..m2). Nothing here has run in game yet; run 15 (section 8) is next.

Deviations from the sections above:
1. The kill / SpEffect wrappers stay in `ac130.rs` as `pub(crate)` (shared by the nuke) instead of moving to `game/`.
2. A2 clamp `next_shot = max(next_shot, now - 0.15 * fireTime)`: at 30 Hz the UMP45 now fires 11 shots/s (was a 12-shot
   burst test); the old 30 Hz test is replaced by `cadence_30hz_never_beats_085_fire_time`; the burst test accepts gaps
   0.85 x ft .. ft + one tick.
3. Countdown timer text is white (MW2 menu data's forecolor), not red; drawn at the normal HUD font size with a glow.
4. Bloom: one 1/8-resolution downsample + 9-tap tent instead of two levels (N3 known answers and 4K p95 0.07 ms).
5. The analyzer's orange-white known answer checks R >= G >= B instead of an exact tint.
6. check_m2 check 2 now replays `git show HEAD:<file>` + patches 0001..0003 and compares with the working tree
   (the old byte compare could not include 0003).
7. The M6 regulation pin moved (package 9a33f2524237..., speffect+2): check_m4's sha / regex and
   `work\m7\regulation.sha256`. `effectTargetSelfTarget` is not overridden on 79000960 (WitchyBND default already
   matches; an explicit override failed the read-back).
8. Nuke HUD icon is loaded through hudhook's texture loader (`dpad_nuke.rgba` from convert_nuke.py).
9. check_m7 check 3 pins four exe instruction ranges with `tools\iw4dis` (`x` ranges; `site` mode is not used).
10. check_m6 check 11 counts its 13 M6 rows with `spawn)\s(?!causes)` (the new M7 `spawn causes` row matched the old pattern).

### 11.1 Review fixes (2026-10-18, after the M7 review; offline only)

Safety:
1. `nuke.dry_run = true` (default for run 15): census + selection run, every NPC that would die is logged as
   `NUKE kill would-apply handle= npc= chr= team= ... reason=dry_run`, no lethal effect on any NPC. The player still
   dies (ER's normal death, nothing permanent). Independently, `nuke_targets::team_check` refuses every NPC kill of the
   sequence (`NUKE blocked reason=team_unverified ...`, sticky) when a census team value is outside the vanilla NpcParam
   `teamType` set (35 values, scan of `work\regulation\unpacked`, 7045 rows) and `spare_teams`, or when 4+ characters
   all read one spared team (a zeroed / wrong byte). Analyzer: `nuke kills` FAILs on `team_unverified` and on any
   apply / would-apply line on a spared team; a clean dry run is NEEDS-DATA (asks for the armed run).
2. Key 4 is released (`WANT_BLANK_NUKE = false`) in the disabled early return and in `emergency_off` (nukewiring 11).
3. Retries queue only `sel.kill` (eligible now); a deferred (invincible) character is queued only once a pass reads
   it as not invincible; `apply_batch` also skips a target that reads invincible at the re-read (`NUKE kill deferred
   ... why=invincible`); it stays a survivor otherwise (nukewiring 12).
4. Emitter guard: once the counter bound reaches `warn`, a count-only walk (<= 300 reads, cached 15 ms) corrects it and
   the guard decides on min(bound, walked) (`decide_est`; a truncated walk leaves the bound). `SPAWN WARN emitters
   live= upper= walked=` and `SPAWN FAIL ... upper= walked=` carry both. Test `emitter_guard_walk_corrects_a_stale_counter_bound`.
   The nuke's bullet / horizon budgets use the same estimate.
5. The player's death needs a real sample (`player_dead_input`): a missing sample is no decision this tick.
6. Bounded cost: the death watch runs at 10 Hz with at most 32 re-reads per pass (round-robin, `WatchSched`); a
   target with every pass applied is given up 2 s after its last application. `census()` is timed (`census_us=` on the
   census line, `census_us_max=` on the summary; analyzer FAIL > 50 ms); ChrSet entries read the load status first.

Fidelity:
7. Other havok frames are counted and logged (`skip reason=other_frame n=`, `other_frame=` on census / summary), not
   silently dropped. Not killed: their distance cannot be compared without a frame conversion that is not known [I];
   run 15 shows how many there are. Test `nuke_targets_census_other_frame_counted_not_dropped`.
8. The shake starts at T+1.5 (`timeline.quake_delay_s` from `_nuke.gsc`, converter known answer + mutation) and stops
   at the player's death.
9. The unit fixture uses synthetic vision values and a white forecolor; the real MW2 visions are checked only by
   `tests/real_data_m7.rs` from `converted\nuke`.
10. Docs / comments: the flash stands in for MW2's `nuke_flash` fx at T (the full-screen form is ours).
