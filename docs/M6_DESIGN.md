# M6 design: the MW2 arsenal (34 guns, switching, per-class mechanics) + run-13 bug fixes

Status: design, offline only. Nothing here has been run in game. Labels: **[V]** verified (dump, code, log or byte
check), **[I]** inferred (reading, lineage, or fit; must be checked by an oracle or in game). Section numbers are
referenced by check_m6.ps1 and the analyzer.

Sources: the four M6 findings (gun-data-pipeline, special-mechanics, damage-and-rows, two-bugs), MODLOG through run 13,
docs\M2..M5_DESIGN.md, `work\m6\inventory.json` (written by `tools\scripts\inventory_m6.py`, read-only tool, 1.6 s).

Rules that bind this milestone (unchanged from M2-M5): never launch Elden Ring, MW2, me3 or Steam; never write a game
folder or %APPDATA%\EldenRing; no push/publish; MW2 data only under dumps\, work\ and converted\ (gitignored), never in
source; all runtime writes via `game::write`; downloads only through cargo; D:\Mods\MW2inER-m3 untouched; every new
behaviour that is [I] sits behind a config switch and logs enough to verify it.

---

## 1. Scope

1. 34 guns, base MP weapon files, no attachments, iron sights; snipers keep the scope. No pistols / machine pistols.
2. Switching with mouse wheel and keys, MW2 drop/raise anims, per-gun ammo kept for the session.
3. Per-class mechanics: burst, double barrel, bolt/pump/lever rechamber, shell-by-shell reload, pellets, sniper
   scope + sway + hold breath, launchers as simulated projectiles with explosions, simplified lock-on.
4. Viewkick (recoil) into the camera and the bullet direction.
5. Regulation: scalable id scheme, flat-damage fix on every gun row, target-damage inversion calibration that keeps
   UMP45 ~49 per close hit on c4311 and exact MW2 damage ratios on c4311.
6. HUD: weapon name, ammo per gun, switch state, scope overlay, lock box.
7. Bug fixes from run 13: AC-130 `reason=loading t_s=0.0` (root cause verified) and `SPAWN FAIL code=4` (cause
   inferred; instrumentation + mitigation + likely fix).
8. Offline oracles, `check_m6.ps1` (runs check_m5 first), analyzer rows, in-game test plan.

Out of scope: attachments, perks, killstreaks other than the AC-130, pistols, secondary/primary slot loadouts (the
arsenal is one cyclic list), MW2 self-damage from explosions (deliberate safety deviation, section 9.6).

---

## 2. Gun list, verified internal names, per-gun data

All 34 names exist as `dumps\common_mp\weapons\<name>` (671 fields each, no missing keys) and as `weapon, <name>` in
`dumps\lists\common_mp.txt` **[V]**. Every gun has `<gunModel>_lod0.glb` + xmodel JSON in common_mp_assets, every
colorMap DDS, every xanim its anim fields name, fire + reload sounds **[V]**. Hands: `viewmodel_base_viewhands` for all.

Columns: fireType | fireTime s | damage/minDamage @ maxRange/minRange (in) | pellets | clip/startAmmo/maxAmmo |
reload/reloadEmpty s | g (stable index, section 9.1) | gunModel (OAT type) | notes. All values [V] from the weapon files.

### 2.1 Assault rifles (weaponClass `rifle`, MW2 MP move x0.95)

| name | user name | fire | fT | dmg | clip/start/max | reload | g | gunModel | notes |
|---|---|---|---|---|---|---|---|---|---|
| m4_mp | M4A1 | Full Auto | .07 | 30/20 @1500/2000 | 30/90/180 | 2.03/2.36 | 01 | viewmodel_m4 (anim) | SP/MP fire WAV differ; use MP |
| famas_mp | FAMAS | 3-Round Burst | .06 | 40/30 @2000/2500 | 30/90/180 | 2.76/3.30 | 02 | viewmodel_famas (anim) | idle 0 frames |
| scar_mp | SCAR-H | Full Auto | .093 | 40/30 @1500/2000 | 20/60/120 | 2.833/2.833 | 03 | viewmodel_scar_h (anim) | empty uses reload_empty_alt |
| tavor_mp | TAR-21 | Full Auto | .08 | 40/30 @1500/2000 | 30/90/180 | 2.5/2.93 | 04 | viewmodel_tavor (anim) | idle 1 frame |
| fal_mp | FAL | Single Shot | .08 | 55/35 @1000/1500 | 20/60/120 | 2.7/3.267 | 05 | viewmodel_fn_fal (anim) | |
| m16_mp | M16A4 | 3-Round Burst | .065 | 40/30 @1500/2000 | 30/90/180 | 2.03/2.36 | 06 | viewmodel_m16 (anim) | |
| masada_mp | ACR | Full Auto | .077 | 30/20 @1800/2300 | 30/90/180 | 2.03/2.36 | 07 | viewmodel_magpul_masada_dust (anim) | no tag_sight_on |
| fn2000_mp | F2000 | Full Auto | .066 | 30/20 @1500/2000 | 30/90/180 | 3.0/3.43 | 08 | viewmodel_f2000 (anim) | no tag_sight_on |
| ak47_mp | AK-47 | Full Auto | .085 | 40/30 @1500/2000 | 30/90/180 | 2.5/3.25 | 09 | viewmodel_ak47_tactical (anim) | no tag_sight_on |

### 2.2 SMGs (`smg`, x1.0)

| name | user name | fire | fT | dmg | clip/start/max | reload | g | gunModel |
|---|---|---|---|---|---|---|---|---|
| mp5k_mp | MP5K | Full Auto | .067 | 40/20 @750/1000 | 30/90/180 | 2.53/3.03 | 11 | viewmodel_mp5k (rigid) |
| ump45_mp | UMP45 | Full Auto | .09 | 40/35 @600/850 | 32/96/192 | 2.5/3.03 | 12 | viewmodel_ump45 (anim) |
| kriss_mp | Vector | Full Auto | .055 | 25/20 @750/1000 | 30/90/180 | 2.2/2.76 | 13 | viewmodel_kriss_super_v (rigid) |
| p90_mp | P90 | Full Auto | .065 | 30/20 @750/1000 | 50/150/300 | 2.76/3.5 | 14 | viewmodel_p90 (rigid) |
| uzi_mp | Mini-Uzi | Full Auto | .063 | 30/20 @750/1000 | 32/96/192 | 2.4/3.5 | 15 | viewmodel_miniuzi (rigid) |

### 2.3 LMGs (`mg`, x0.875)

| name | user name | fire | fT | dmg | clip/start/max | reload | g | gunModel | notes |
|---|---|---|---|---|---|---|---|---|---|
| sa80_mp | L86 LSW | Full Auto | .075 | 40/40 | 100/200/300 | 5/5 | 21 | viewmodel_sa80_lmg (anim) | one xanim (3.77 s) for both reloads |
| rpd_mp | RPD | Full Auto | .085 | 40/40 | 100/200/300 | 9.7/9.7 | 22 | viewmodel_rpd (anim) | anims move j_bullet17-20 absent from model |
| mg4_mp | MG4 | Full Auto | .07 | 30/30 | 100/200/300 | 9.7/9.7 | 23 | viewmodel_mg4 (anim, 112 bones) | alias weap_rpd_mg4_plr missing everywhere: skip |
| aug_mp | AUG HBAR | Full Auto | .088 | 40/40 | 42/126/252 | 3.36/4.0 | 24 | viewmodel_steyr_lmg (anim) | |
| m240_mp | M240 | Full Auto | .065 | 30/30 | 100/200/300 | 7.75/7.75 | 25 | viewmodel_m240 (anim) | reload clip 9.7 s, scale to 7.75 |

LMGs have damage = minDamage, so one band each (5 rows total).

### 2.4 Snipers (`sniper`, x1.0; all 70/70 @4000/5000, locHead 1.5, adsZoomFov 15, overlay scope_overlay_m40a3, adsFireAnim = idle)

| name | user name | fire | fT | clip/start/max | reload | g | gunModel | notes |
|---|---|---|---|---|---|---|---|---|
| cheytac_mp | Intervention | Single Shot, boltAction 1 | .05 | 5/20/40 | 2.268/3.867 | 31 | viewmodel_cheytac (anim) | rechamberTime .866; MP WAV differs from SP |
| barrett_mp | Barrett .50cal | Single Shot | .05 | 10/30/60 | 3.917/4.25 | 32 | viewmodel_barrett (anim) | |
| wa2000_mp | WA2000 | Single Shot | .05 | 6/24/48 | 3.3/3.9 | 33 | viewmodel_wa2000 (anim) | |
| m21_mp | M21 EBR | Single Shot | .05 | 10/30/60 | 2.67/3.5 | 34 | viewmodel_m14_ebr (anim) | last-shot anim; rechamberTime 1.0 but boltAction 0 |

### 2.5 Shotguns (`spread`, x0.95; hipSpreadStandMin = Max, adsSpread 5.5, locHead 1)

| name | user name | fire | fT | dmg/pellet | pellets | clip/start/max | reload | g | gunModel | notes |
|---|---|---|---|---|---|---|---|---|---|---|
| spas12_mp | SPAS-12 | Single Shot, pump | .47 | 40/20 @400/700 | 8 | 8/32/64 | shell: 1.0 / .667 / 1.0 | 41 | viewmodel_spas12 (anim) | rechamber .467 (clip .933); fire alias weap_ranger_fire_plr |
| aa12_mp | AA-12 | Full Auto | .15 | 20/15 @300/500 | 8 | 8/16/32 | 2.75/3.583 (mag) | 42 | viewmodel_aa12 (anim) | ~53 spawns/s |
| striker_mp | Striker | Single Shot | .17 | 25/15 @300/600 | 6 | 12/36/60 | shell: 1.146 / .666 / .433 | 43 | viewmodel_striker (rigid) | |
| ranger_mp | Ranger | Double Barrel | .01 | 75/35 @200/400 | 6 | 2/20/40 | 2.53/2.53 (clip 3.17) | 44 | viewmodel_sawed_off_double_barrel (rigid) | ADS clip 1 frame |
| m1014_mp | M1014 | Single Shot | .2 | 40/20 @300/500 | 8 | 4/20/40 | shell: 1.0 / .567 / .36 | 45 | viewmodel_benelli_m4 (rigid) | last-shot anim |
| model1887_mp | Model 1887 | Single Shot, lever | .43 | 35/20 @200/600 | 8 | 7/28/56 | shell: 1.5 / .85 / 1.25 | 46 | viewmodel_model_1887 (rigid) | rechamber 1.0; fire alias weap_winch1200_fire_plr |

### 2.6 Launchers (playerDamage 0, minDamage 0, reloadEmptyTime 0; damageConeAngle 180, projImpactExplode 1)

| name | user name | fT | clip/start/max | speed in/s (up) | explosion inner/outer r(in) | impact dmg | lock | move | g | gunModel |
|---|---|---|---|---|---|---|---|---|---|---|
| at4_mp | AT4-HS | 1.06 | 1/1/1 | 2500 (500) | 155/55 r400 | 1000 | supported, not required (Sidewinder) | rocketlauncher x0.8 | 51 | viewmodel_at4 (rigid) |
| m79_mp | Thumper | .5 | 1/2/1 | 2400 (10), arms 375 in | 155/25 r300 | 135 | none | `grenade` -> x1.0 | 52 | viewmodel_m79 (anim) |
| rpg_mp | RPG-7 | .33 | 1/2/1 | 1500 (500) | 160/60 r400 | 1000 | none | x0.8 | 53 | viewmodel_rpg7 (rigid) + rocket at tag_clip |
| stinger_mp | Stinger | 1.06 | 1/2/1 | 4000, ignition 500 ms | 155/55 r400 | 1000 | required (air only in MW2) | x0.8 | 54 | viewmodel_stinger (rigid) |
| javelin_mp | Javelin | 1.06 | 1/1/1 | 600, ignition 500 ms | 300/50 r900 | 1000 | required, top attack | x0.8 | 55 | viewmodel_javelin (rigid) |

Name traps [V]: m21_mp = M14 EBR; sa80_mp = L86 LSW; aug_mp = AUG HBAR; kriss_mp = Vector; masada_mp = ACR;
tavor_mp = TAR-21; cheytac_mp = Intervention; m79_mp = Thumper.

### 2.7 Arsenal order (cycle order, = config list order)

AR (m4 famas scar tavor fal m16 masada fn2000 ak47), SMG (mp5k ump45 kriss p90 uzi), LMG (sa80 rpd mg4 aug m240),
sniper (cheytac barrett wa2000 m21), shotgun (spas12 aa12 striker ranger m1014 model1887), launcher (at4 m79 rpg
stinger javelin). `[arsenal] guns = [...]` in mw2er.toml holds this list; `start = "ump45_mp"` (the verified gun).
`gameplay.weapon` is kept as an alias for `arsenal.start` (old configs parse; both set = error).

---

## 3. Converter generalisation (Python, offline)

### 3.1 Hard-coded UMP45 assumptions to remove [V list from the pipeline finding]

convert_viewmodel.py (GLB_GUN, WEAPON_FILE, PREFIX check, EXPECT_* counts, output names ump45.*, single output dir
that deletes stale files); vm_format.BASE_CLIPS (12); vm_format.SKEL_TAGS requiring tag_sight_on; copy_sounds.py TABLE
(5 roles); convert_weapons.py DEFAULT_WEAPONS / KNOWN_ANSWERS (3 guns); xanim_cross test prefix viewmodel_ump45_;
vmtest paths. Rust side: vm_assets SKEL_FILE/MESH_FILE/ANIM_FILE consts, `ClipId` (12-entry enum), single
`ASSETS` OnceLock and `RENDERER` in viewmodel.rs, sounds.rs 5 fixed roles, data.rs one gun per session.

### 3.2 New driver `tools\scripts\convert_arsenal.py`

Input: the gun list (config `[arsenal] guns`) and `work\m6\inventory.json` (rebuilt if stale by sha of inputs).
Per gun, reusing the existing functions (parse_weaponfile, xanim_read, vm_format.Glb, build_skeleton, build_mesh,
wav_info):

- **Clip names from weapon fields**, case-insensitive xanim lookup (dumped files are lowercase; weapon files mix
  case: `viewmodel_spas12_HB_*`, `viewmodel_benelliM4_*`, `viewmodel_M14ebr_*`) [V].
- **Clip set** = the 12 M3 clips plus optional: `last_shot`, `rechamber`, `ads_rechamber`, `reload_start`,
  `reload_loop`, `reload_end`, `empty_idle`, `first_raise`, `reload_empty_alt` (scar). Absent clip = not written; the
  runtime falls back (section 5.4). Shared clips (LMG reload = reloadEmpty; sniper adsFire = idle; ranger
  adsUp = adsDown) are written once and referenced twice.
- **0-frame / 1-frame clips** (famas, rpg, javelin idle 0 frames; tavor, masada, fn2000, ump45, model1887, m79
  idle 1 frame) [V]: written as a 1-key static pose; duration 0 is stored as 0 and never divided by (runtime and
  converter both guard).
- **tag_sight_on optional** (missing on 19 models [V]): stored as 0xFFFF in vmskel; Rust vm_assets accepts it; the
  ADS pose then uses the anim (ADS clips already place the sights) [I: checked by the vmtest ADS render, 13.4].
- **OAT model type**: accept `rigid` or `animated` (12 rigid [V]); prove delta translation per gun as M3 check 1m
  does (a moving bone near 0 at frame 0 while its bind offset is not 0) instead of trusting the type.
- **Bone whitelist** per gun for anim bones missing from the model: rpd `j_bullet17..20` dropped with a logged note
  [V that they are missing].
- **RPG rocket part**: `rocketModel viewmodel_rpg7_rocket` merged as an extra mesh part parented to `tag_clip`,
  flagged `part=rocket`, hidden while mag = 0 and shown `reloadShowRocketTime` (0.1 s) into the reload [V fields].
- **Notetracks** -> `clips.json`: per clip `{name, frames, fps, duration, looped, notes:[{t_frac, name}]}` and the
  matching weapon time field (reloadTime, rechamberTime, ...) so the runtime scales `rate = duration / time_field`
  [V that clips need scaling: spas12 rechamber 0.933 vs 0.467, m240 9.7 vs 7.75, p90 3.1 vs 2.76, m79 3.1 vs 2.6,
  javelin 4.5 vs 3, ranger 3.17 vs 2.53]. Notes named viewmodel_small/medium/large are rumble, not sound: dropped.
- **Sounds**: resolve aliases in zone order `localized_common_mp` -> SP `common` -> `common_mp` -> `mp_rust` ->
  extracted iwd [V: MP zone is authoritative; m4 and cheytac differ from SP; stinger/javelin only in MP]. Follow one
  level of `secondaryaliasname` (mech layers, raise layer, rpg fire layer). `null.wav` targets are silent by design
  (cheytac chamber_close, javelin reload). `weap_rpd_mg4_plr` is in no dumped zone: skipped with a note.
  `exp_stinger_armor_destroy` is streamed (iwd): not extracted; launcher explosions reuse the AC-130 explosion SFX.
  Output PCM through `sndconv` (unchanged format), each alias once.
- **Weapon stats** -> `stats.json`: every field the runtime uses (section 4.1), plus weaponClass, sha256 of the
  weapon file.
- **Scope overlay**: `scope_overlay_m40a3_1024#0.dds` is DXT5 1024x1024 with 1 mip; `vf.read_dds` rejects it [V].
  Converted through the AC-130 overlay path (convert_ac130's BC3 handling) into
  `converted\guns\_overlay\scope_overlay_m40a3.vmtex` (BC3, 1 mip allowed for overlays only).

### 3.3 Outputs (converted\, gitignored)

```
converted\guns\index.json                 # [{name, g, class, manifest_sha, ok, problems[]}], schema 1
converted\guns\<name>\gun.vmskel          # skeleton incl. hands, optional tags
converted\guns\<name>\gun.vmmesh          # gun + hands (+ rocket part for rpg), material -> _tex refs
converted\guns\<name>\gun.vmanim          # all present clips
converted\guns\<name>\clips.json          # clip table, notetracks, time-field pairs, rates
converted\guns\<name>\stats.json          # runtime weapon stats (section 4.1)
converted\guns\<name>\manifest.json       # sha256 of every input and output, converter version
converted\guns\<name>\snd\*.pcm + sounds.json   # role -> alias -> files (+ layers)
converted\guns\_tex\<sha8>_<name>.vmtex   # deduplicated (60 unique, 51.5 MB DDS in) [V count]
converted\guns\_overlay\scope_overlay_m40a3.vmtex
```

The M3 `converted\viewmodel\ump45.*` stays in place until the M3 oracles are migrated (they keep passing on it).
Sizes [I estimate from dry run]: ~0.3-0.45 MB mesh + 0.1-0.6 MB anim per gun; ~100 MB total on disk.

Converter dry run with tag_sight_on optional: build_skeleton + build_mesh succeed for all 34 guns with 0 problems;
bones 70..112 (cap 256), vertices 5,496..10,388 (cap 65,535) [V].

---

## 4. Runtime data model (Rust)

### 4.1 `mw2er-core::arsenal`

```rust
pub struct GunSpec {            // pure data, from converted\guns\<name>\stats.json (+ clips.json)
    name, g: u8, class: GunClass,            // Rifle Smg Mg Sniper Spread Launcher(Grenade|Rocket)
    fire: FireMode,                          // Auto | Single | Burst(n) | DoubleBarrel
    fire_time, clip, start_ammo, max_ammo,
    reload: ReloadKind,                      // Mag{time, empty_time} | Segmented{start_s, start_add, start_add_s,
                                             //   loop_s, add, add_s, end_s}
    rechamber: Option<Rechamber{time, bolt_time, while_ads}>,   // only boltAction == 1
    pellets: u8, falloff: Falloff, loc_head: f32,
    spread: SpreadParams (hip stand/duck/prone min/max, fire add, decay, ads_spread),
    kick: KickParams (hip/ads pitch/yaw min/max, center speed; gun kick for vm_pose),
    ads: AdsParams (zoom_fov, trans_in, trans_out, overlay: Option<Overlay>, idle_amount, idle_speed),
    move_class: MoveClass,                   // from weaponClass (section 7)
    projectile: Option<ProjectileSpec>,      // launchers
    raise_s, drop_s, first_raise_s, clips: ClipTable,
}
```

`WeaponSpec` validation is extended per class [V that today's rules reject the arsenal]:
- `FireMode::parse` gains `"Double Barrel"` -> `DoubleBarrel` (weapons.rs:100-109 has no arm).
- `min_damage >= 1` only for non-launchers; launchers require explosion fields instead (weapons.rs:187).
- `reloadEmptyTime > 0` only for non-launchers; launchers use reloadTime for both (weapons.rs:202).
- `rechamber` from `boltAction == 1` only (ranger 0.467 and m21 1.0 have boltAction 0 [V]).

### 4.2 Registry and per-gun state

- `Arsenal { specs: Vec<GunSpec>, enabled: Vec<bool>, current: usize, sw: SwitchMachine, state: Vec<GunState> }`.
- `GunState { mag, reserve, chambered: bool, sim: WeaponSim, spread: SpreadState, kick: KickState }`; kept for the
  session, including across MODE off/on. `refill_on_mode_enable = true` refills **all** guns (as today for one).
- Reserve at spawn/refill: `reserve = max_ammo` (today's UMP45 behaviour, 32/192, verified in logs) [V current];
  `[arsenal] reserve_source = "max" | "start"` (start = startAmmo - clip) [I which matches MW2 MP]. Launchers:
  AT4/Javelin 1+1, RPG/Stinger/M79 1+1 with "max" (maxAmmo 1); 1+1 with "start" for at4/javelin (startAmmo 1 means
  only the loaded round: reserve 0) [I].
- A gun whose assets, stats or PARAMS check fail is `enabled = false`; switching skips it; logged once
  (`ARSENAL disable gun=<n> why=<assets|stats|params|rows>`). If the start gun is disabled, the first enabled gun is
  used. Zero enabled guns = MW2 firing disabled (as PARAMS FAIL does today).

### 4.3 Asset loading

Load all enabled guns at boot on the existing `mw2er-vmload` thread into a per-gun slot table:
`Vec<OnceLock<Arc<GunAssets>>>` + renderer GPU buffers indexed by gun; textures deduplicated across guns (one GPU
texture per `_tex` file, refcounted). Estimates [I]: ~65 MB VRAM, ~26 MB decoded PCM in RAM. Switching performs no
I/O. If a gun is not loaded yet when selected, the switch waits in `Dropping`/`Raising` hold and logs
`SWITCH wait gun=<n> loaded=0` (fallback design: load on demand + prefetch next/prev, config
`[arsenal] preload = "all" | "neighbours"`). Boot log: `ARSENAL loaded n=34 ok=<k> ms=<t> vram_mb=<m>`.

### 4.4 Shot pipeline per gun (task.rs:595-655 / 745-868 [V order])

The single `d.weapon / rows_weapon / sim / spread_p / falloff / vm params` set becomes `arsenal.current()`. `fire()`:
1. fire sound (gun's alias + layers), camera gate, owner check, origin;
2. `aim = er_pre_write_rows ⊕ kick ⊕ sway` (section 8); direction from the kicked, swayed forward;
3. spread cone `deg = lerp(hip_cone(stance, state), ads_spread_or_0, ads_frac)` (fixes today's hip-only cone,
   task.rs:779 [V]); for non-shotguns ADS uses the gun's ADS spread field (0 for most rifles [V field]); 
4. hitscan: one `map_ray` + chest proximity per shot, `falloff.damage_at(dist)` -> band row, `spawn` per pellet
   (pellets section 6.4); projectile: push a `Projectile` (section 6.6), no spawn;
5. breaker counts failures **per shot** (a shot fails only if all its pellets fail) — 8 pellets would otherwise open
   the 8-failure breaker in one shot [V breaker rule task.rs:56];
6. `SHOT` line gains `gun=<name> pellets=<n> bands=<ids>`; shot_hist gains pellet count.

---

## 5. Switching

### 5.1 Keys (no conflicts) [V binds, I ER side]

| Input | Default | MW2 bind [V] | Notes |
|---|---|---|---|
| `keys.weapon_next` | `"1"` | `bind 1 "weapnext"` | next enabled gun in list order (wraps) |
| `keys.weapon_prev` | `"2"` | `bind 2 "weapnext"` (MW2 has 2 guns, so next = prev) | previous gun; deviation, configurable |
| mouse wheel | down = next, up = prev | (no wheel bind in default_mp_controls.cfg) | `[arsenal] wheel = true`, `wheel_invert = false` |
| `keys.weapon_menu` | `""` (off) | — | reserved |

- Conflict check: `1`/`2` and the wheel are used by the AC-130 (`ac130_gun1/2`, wheel cycles guns) **only while the
  gunship is active**, when MW2 ground controls are suspended (`MOVE suspend reason=ac130`) and arsenal input is
  ignored. Same rule as M5 6.1 for Q/F. Validation: weapon keys may not equal `toggle`, `reload`, `fp_toggle`,
  `ac130`, `ac130_exit`, any `move_*` key, or `E`. They may equal `ac130_gun*`/`ac130_next`. Tested
  (`config_keys_m6_conflicts`).
- Q is deliberately **not** used (ER lock-on, M5 blank list, AC-130 next). 3 is not used (MW2 actionslot 3).
- While MW2 mode is on and the controller drives, `1`, `2` are added to `blank_er` (latched until release) and the
  DirectInput `lZ` is zeroed for ER (the hook already accumulates `WHEEL` [V input_hook.rs:171]; today it zeroes lZ
  only for the gunship). What ER does with 1/2/wheel on the user's preset is [I]; `keyprobe` gains `1`, `2`,
  `WHEEL_UP`, `WHEEL_DOWN` (MW2 off) and logs `MOVE keyprobe key=1 er_bits=...`.
- Wheel: `input_logic::wheel_steps` (already used by AC-130) converts lZ (120/notch) into steps; several notches in
  one tick queue one switch target `current + steps` (not several anims).

### 5.2 Switch state machine (`mw2er-core::switch_logic`, pure)

States: `Ready(g)` -> `Dropping{from, to, t}` -> `Raising{to, t}` -> `Ready(to)`.

- Request (key/wheel) in `Ready(g)`: allowed unless the AC-130 is active or MW2 mode off. Any in-progress reload,
  rechamber, burst, segmented reload or ADS is cancelled (ADS out instantly): magazine reload adds nothing
  (weapon_sim today adds at the end [V]); segmented reload keeps shells already committed; a cancelled rechamber
  leaves `chambered = false` and the gun rechambers after its next raise [I]. Burst in flight: the request waits for
  the burst to finish (≤ 3 x fireTime) [I].
- `Dropping`: play gun A's `drop` clip scaled to `dropTime` (0.33-1.25 s [V range]); no fire; sprint allowed.
- Request during `Dropping`: retarget `to` (no extra anim). Request during `Raising` to a different gun: go to
  `Dropping{from: to_current}` starting at the raise progress mirrored (`t_drop = (1 - raise_frac) * dropTime`) [I].
- `Raising`: gun B's `raise` clip scaled to `raiseTime`; first time a gun is raised in the session play
  `first_raise` (if present) scaled to `firstRaiseTime` [I MW2 first-raise semantics]. Fire is blocked; ADS press is
  buffered and starts when Ready.
- `Ready(to)`: if `!chambered && rechamber.is_some()` start rechamber. If mag 0 and reserve > 0 and
  `auto_reload_on_empty`, start reload.
- Movement class (section 7) switches at the **start of Raising** (MW2 updateMoveSpeedScale on weapon_change [V]).
- Sprint during switch: allowed (MW2 allows), the raise continues. Sprint start in `Ready` during a burst is refused.
- Death / world leave / MODE off: machine resets to `Ready(current)` with no anim.

Events: `SwitchStart{from,to,via}`, `SwitchRetarget{to}`, `SwitchRaised{gun}`, `SwitchBlocked{why}`.
Log: `SWITCH start from=ump45_mp to=p90_mp via=wheel|key1|key2 drop_s=0.50`, `SWITCH raised gun=p90_mp t=1.07
first=1`, `SWITCH blocked why=ac130|mode_off|disabled|only_one`.

### 5.3 vm_anim changes

- `ClipId` (12, fixed array vm_assets.rs:34-63) becomes a per-gun `ClipTable` of `Option<ClipRef>` keyed by role
  (the 12 + optional roles of 3.2). `VmPhase` gains `Drop`, `Raise`, `FirstRaise`, `Rechamber`, `ReloadStart`,
  `ReloadLoop`, `ReloadEnd`, `EmptyIdle`.
- **Every** timed clip is played at `rate = clip_duration / time_field` (today reload clips play at native rate,
  vm_anim.rs:167 [V]; it fit the UMP45 by coincidence). Duration 0 clips: rate 1, static pose.
- During `Dropping` the renderer draws gun A; at Raising start it binds gun B's GPU slot (no upload).

### 5.4 Clip fallbacks

| missing | fallback |
|---|---|
| last_shot | fire |
| ads_rechamber | rechamber |
| empty_idle | idle (rpg: rocket part hidden anyway) |
| first_raise | raise |
| reload_empty | reload |
| reload_start/loop/end on a segmented gun | the gun is disabled (`why=stats`), never seen in the inventory [V all 4 present] |

---

## 6. Per-class mechanics

### 6.1 Fire modes [V data, code exists for burst]

- **Full Auto / Single Shot**: as today.
- **Burst(3)** (famas .06, m16 .065): keep weapon_sim.rs:231-239 [V]: a press loads 3, the burst completes after
  release, stops at mag 0, next burst needs a new press, next shot ≥ fireTime after the last. No extra
  inter-burst cooldown (no field among 671 keys) [I]. Changes: reload press while `burst_left > 0` is deferred until
  the burst ends (today start_reload zeroes burst_left, line 279) [I MW2]; sprint start is refused while
  `burst_left > 0`; switch waits (5.2).
- **DoubleBarrel** (ranger): treated as Single Shot per press with fireTime 0.01, clip 2. Config
  `[arsenal.ranger] both_barrels = false` (true = one press fires both: 12 pellets) [I which MW2 does].

### 6.2 Rechamber (boltAction 1: cheytac .866, spas12 .467, model1887 1.0) [V data, I ordering]

`WeaponSim` gains `Rechamber{until}` and events `RechamberStart`, `RechamberDone`. After a shot that leaves
`mag > 0`: Firing for fireTime, then Rechambering for rechamberTime (`rechamber_mode = "after_fire"` default;
`"overlap"` starts it at the shot) [I]. Clip = `ads_rechamber` when `ads_frac >= 0.5` else `rechamber`, both scaled
(cheytac 1.077x, spas 2.0x, 1887 1.167x [V durations]). `rechamberWhileAds = 1` for all three [V]: ADS stays (scope
stays up). Fire is ignored until Ready and a new press is needed. A shot that empties the mag has no rechamber (the
reload chambers). Sprint or switch cancels: `chambered = false`, rechamber on next Ready. Sounds from notetracks
(cheytac bolt unlock 3/28, open 7/28, close 16/28, lock 19/28; spas lift 0, open 4/28, close 10/28; 1887 lift 0,
open 6/35, close 16/35 [V]). Cycle times: cheytac 0.916 s, spas 0.937 s, 1887 1.43 s.

### 6.3 Shell-by-shell reload (segmentedReload 1: spas12, striker, m1014, model1887) [V data, I state machine]

`ReloadKind::Segmented`: `ReloadStart` (start_s; adds `reloadStartAdd` at `reloadStartAddTime`) -> `ReloadLoop` x n
(loop_s each; adds `reloadAmmoAdd = 1` at `reloadAddTime`, clamped to the loop end — SPAS add time 1.0 > loop 0.667
[V]) while `mag < clip && reserve > 0` -> `ReloadEnd` (end_s) -> Ready.
- Shells are committed per segment (unlike today's whole-magazine add at the end, weapon_sim.rs:199-206).
- Fire press during Start/Loop sets `want_fire`; the current segment finishes (its add counts only if its add time has
  passed), then ReloadEnd plays and the shot fires on the first Ready tick. With `mag == 0` the reload continues.
- Cancel (switch, sprint, MODE off): no further shells. R while full: ignored (as today).
- Empty variant ignored for segmented guns [I]. reload_end carries the pump/lever sound: no rechamber after a
  shotgun reload [V notetracks].
- Events: `ShellAdded{mag, reserve}`, `ReloadEnd`. Clip rates: SPAS in 1.33x, loop 1.30x, out 1.0x; striker, m1014,
  1887 from clips.json.
- Total time oracle: `start + n*loop + end` (SPAS 8 shells from empty: 1.0 + 7*0.667 + 1.0 = 6.67 s with 1 shell
  in start).

### 6.4 Pellets (shotCount 6-8) [V data, I spread rule]

- N independent directions per shot from `spread::perturb` (uniform angle, uniform radius: centre-biased like IW
  [V code], pellet rule in MW2 [I]) inside `deg = lerp(hip_cone, adsSpread 5.5, ads_frac)`.
- One `map_ray` + one chest-proximity per shot (reused for all pellets); `[arsenal] pellet_bands = "shot"` (one band
  for all pellets, default) or `"pellet"` (per-pellet proximity) [I].
- Range cap `[arsenal] shotgun_range_cap = true` [I MW2]: if the hit distance > minDamageRange the pellets are not
  spawned (logged as `SHOT ... capped=1`); bullet rows are not changed (life*speed stays 150 m).
- Per-pellet damage = band of the per-pellet MW2 damage (falloff). Bands: ranger 75-35 compressed to 16 bands.
- Spawns: N calls of `spawn::spawn`, one row per pellet band. Budget: AA-12 53 spawns/s, ≤ 16 per tick
  (max_shots_per_tick 2). **Gate**: pellets are spawned only while the bug-2 pool check (10.2) reports headroom
  ≥ 2N (`SPAWN pool` free entries; each pellet also spawns a marker child); pellets that do not fit are dropped
  and counted (`SHOT ... pellets=8 dropped=<k> why=pool`).
- Rejected: one Bullet row with numShoot = N (box distribution, one band, isUseSharedHitList trap) [V field exists].
- Logs: `SHOT gun=spas12_mp pellets=8 bands=79141000x5,79141003x3 ...`; HitTracker allows up to N HIT rows per shot.

### 6.5 Snipers: overlay, FOV, sway, hold breath [V data, I model]

- **Overlay**: when `ads_frac >= 1` on a gun with `adsOverlayShader`, hide the viewmodel (adsFireAnim = idle [V]) and
  draw `scope_overlay_m40a3` full-height square centred (480x480 in 640x480 virtual [V]) with black side fill [I],
  alpha blended (srcalpha/invsrcalpha [V material]), in a new non-thermal full-screen pass in mw2er-render (thermal.rs
  BC3 upload code reused). Overlay drops instantly on ADS release [I].
- **FOV**: write `pers_cam_1+0x50` as the world FOV override does (first_person.rs:720-737 [V]), lerped by ads_frac
  to `adsZoomFov` (sniper 15, rifles 50, SMG/shotgun 55, Thumper 80, Stinger/Javelin 25 [V]). Rifles/SMGs get their
  ADS FOV too (`[arsenal] ads_fov = true`). Third person: same field [I].
- **Sensitivity**: scale mouse dx/dy by `tan(fov/2)/tan(hip_fov/2)` with residual carry in input_hook (the path the
  gunship already uses, input_hook.rs:165-174 [V]); `[arsenal] ads_sens_scale = true` [I].
- **Sway** [I]: Lissajous view offset, amplitude `k_sway * adsIdleAmount` (cheytac 60, others 40), speed
  adsIdleSpeed 1.5; `k_sway` calibrated so the Intervention wanders ~1 deg. Applied like viewkick (section 8) to the
  camera and the bullet direction.
- **Hold breath**: the sprint key (LSHIFT) pressed while `ads_frac >= 1` on an overlay gun (MW2 `+breath_sprint` on
  Shift [V cfg]; our sprint already refuses during ADS, sprint.rs:263 [V]). iw4sp defaults [V SP, I MP equal]:
  hold 4.5 s, gasp 1 s, gasp scale 4.5, hold lerp 4, gasp lerp 6. After ADS exit with Shift held, sprint needs a key
  up first. Logs `BREATH hold`, `BREATH gasp why=timeout|release`.

### 6.6 Launchers [V data, I flight model, decision on lock-on]

**Flight = simulated in the DLL** (`[launchers] flight = "dll"` default; `"engine"` = the engine projectile row,
section 9.5). Each tick: `pos += v*dt` (+ `g*dt` for the M79), segment tested with `havok::map_ray(prev, dir, |step|)`
and `ac130_aim::first_hit(...)` (ac130_aim.rs:82 [V]). At most 2 live projectiles (`max_live = 2`), 1 ray per tick
each.
- Rockets (AT4, RPG, Stinger, Javelin): straight line from the muzzle along the aim, no gravity [I].
  `projectileSpeedUp` (AT4/RPG 500 in/s) is ignored by default (`rocket_speed_up = "ignore"`); `"add"` adds it to
  v0 for A/B in game [I meaning in MW2].
- M79: ballistic, g = 800 in/s^2 = 20.32 m/s^2 [V iw4 constant], v0 = fwd*2400 + up*10 in/s; arms after 9.525 m;
  impact before arming = dud: no explosion; a direct character hit spawns the M79 impact row (MW2 135) [I].
- Lifetime: AT4/RPG/Stinger 7 s, Javelin/M79 30 s [V]; expiry = vanish [I].
- **Explosion** exactly as `Ac130::explode` (ac130.rs:1094-1131 [V]): spawn core + ring tier rows (or single) at
  `impact - dir*0.3` with the launcher's kind-6 rows (section 9.4). Direct character hit also spawns the impact row
  (kind 5 idx 0) [I MW2 rocket 1000 is vehicle impact; default `rocket_impact = false` = explode only].
- Visual: tracer streak + trail through the existing ac130_tracer / tracer.rs pass; explosion SFX from the AC-130.
- No self-damage (AtkParam opposeTarget only, selfTarget 0) — deliberate deviation, documented in HUD help.
- RPG rocket part hidden while mag 0; Javelin/RPG/AT4 `empty_idle` where present.

**Lock-on decision** (user-visible; MW2 Stinger only locks air killstreaks, ER has none [V _stinger.gsc:179-246]):
`[launchers] lock = true` (default) implements a simplified lock, logged as a design deviation:
- **Stinger**: while `ads_frac >= 0.95`, lock the nearest enemy character whose chest projects inside the reticle
  circle (65/75 of 640x480, scaled [V gsc]) with LOS by map_ray; 1.0 s lock (LOCK_LENGTH 1000 [V]), 0.5 s lost-sight
  grace [V]. Fire without lock: refused, HUD `NO LOCK` + beep (requireLockonToFire 1 [V]). Missile: 0.5 s
  ignition at low speed (eject 1/10 speed [I]), then proportional navigation (N = 3 [I]) capped at maxSteeringAccel
  10000 in/s^2 = 254 m/s^2 [V field].
- **Javelin**: point lock (12 samples at 25 ms within 10.16 m, ≥ 27.94 m away, 1.15 s [V gsc constants]) or an enemy
  character lock (target point follows the chest). Top attack: launch 15.24 m/s, ignite 0.5 s, climb to apex
  `h = clamp(0.35*range, 20, 120) m` [I, MW2 apex not in data], dive onto the point; direct attack if a map_ray up
  from the target hits within h [I]. Fire without lock refused.
- **AT4**: dumbfire (MW2-faithful in practice: no air targets).
- `lock = false`: Stinger and Javelin fire dumb (straight), `LAUNCH lock=off dumbfire=1` logged once.

Logs: `LOCK start gun=stinger_mp target=c4311#ptr`, `LOCK acquired t=1.00`, `LOCK lost why=sight|fov`,
`LAUNCH gun=at4_mp id=<n> v=63.5 lock=none|chr|point`, `PROJ hit kind=map|chr|expire|dud d=<m> t=<s>`,
`PROJ explode gun=at4_mp tiers=core+ring at=(x,y,z)`.

---

## 7. Movement speed per gun [V]

moveSpeedScale = 1 in all 34 files, so it is not used. MW2 MP `_weapons.gsc updateMoveSpeedScale` (lines
2513-2565) sets speed by weaponClass on every weapon_change: rifle 0.95, mg 0.875, smg 1.0, spread 0.95,
rocketlauncher 0.80, sniper 1.0, other (m79 `grenade`) 1.0. The converter writes `move_class` into stats.json; M5's
`[movement] weapon_move_class` becomes per gun (config value `"auto"` = from the gun; a fixed class still allowed for
tests). Change applies at Raising start. adsMoveSpeedScale (1 or 2) is not used [I meaning unknown]. Log
`MOVE class gun=<n> class=mg scale=0.875`.

---

## 8. Recoil (viewkick), sway, gun kick [V data, I model]

- **View kick** per shot: `vel += (U(pMin,pMax), U(yMin,yMax))` deg/s (min may exceed max), hip/ADS lerped by
  ads_frac; per frame `angle += vel*dt; vel -= sign(angle)*center_speed*dt`, never crossing zero. Peak per shot
  `v^2/(2*center)`: UMP 1.76 deg, M4 0.83, Intervention hip 10 / ADS 7.2, Barrett 4.5, SPAS 4.5, launchers ADS 1.8
  [I scale]. `[kick] scale = 1.0` hot; a pure test pins the formula.
- **Application**: in the camera-step detour **after** ER's step, rotate rows 0-2 by (pitch, yaw) about the camera
  and write rows 0-3 once (`game::write`), the pattern of `ac130_shake::apply_roll_offset` (ac130_shake.rs:118 [V]).
  ER rebuilds the rows every frame [V M4], so nothing accumulates. Kick angles are published to the detour as
  atomics. The detour saves `ER_ROWS` (pre-write) as it saves `ER_POS`; the task aims from `ER_ROWS ⊕ kick ⊕ sway`
  (task.rs:384-408 pattern) so bullets go where the kicked crosshair points. A counter of frames where ER did not
  rebuild the rows (like CHR_PERSISTED) is logged in PERF (`kick_stale`).
- Third person: `[kick] camera_tp = true` rotates the TP camera the same way; false = kick only the bullet direction
  and offset the HUD crosshair.
- Rejected: injecting mouse counts (unknown ER gain/smoothing, permanent aim change).
- **Gun kick** (hip/adsGunKick*, accel, speedMax, decay): viewmodel-only, added to vm_pose as a damped spring [I].
- bg_viewKick* dvars are damage flinch, not recoil [V description]: not used.

---

## 9. Regulation

### 9.1 Id scheme (Bullet and AtkParam_Pc share the id) [V free ranges]

`id = 79_100_000 + g*1000 + kind*100 + idx`, g from section 2 (stable, append-only, never reordered).

| kind | meaning | idx |
|---|---|---|
| 0 | hitscan falloff bands | band 0..15 (MAX_BANDS 16, band_damages()) |
| 1-3 | reserved: head / upper / lower torso band sets (built only if `[damage] dll_headshots = true`) | band |
| 5 | launcher: 0 impact/projectile row, 1 impact marker (atkId -1), 2 engine-projectile fallback chained to single | |
| 6 | explosion tiers | 1 core, 2 ring, 3 single |

Examples: UMP45 79112000..79112005; FAL 79105000..; Intervention 79131000; AT4 79151500/501/502, 79151601-603.
Totals [V counts]: hitscan 244 rows (AR 104, SMG 50, LMG 5, sniper 4, shotgun 81) + launchers 5x6 Bullet / 5x5 Atk ->
~274 Bullet + ~269 AtkParam_Pc added. PARAM row count is u16 (max 65535); vanilla 15475 / 11017 -> ~15763 / ~11299.
No vanilla id between 20010055 and 81000000 (Bullet) or 32100831 and 210030000 (AtkParam_Pc) [V].
The legacy block (79000100-105 UMP45, AC-130 79000201-203/301-303/401, marker 79000900, SpEffect 79000950) stays
byte-identical: check_m3b check 8 / m2rows keep passing; the DLL no longer uses 79000100-105 by default
(`[regulation] legacy_ump45 = false`; true = A/B fallback). `make_plan` drops the slot 1..8 rule
(build_regulation.py:306-308). `[regulation.weapons]` is replaced by `[regulation.arsenal]` reading the g table.

### 9.2 Flat-damage fix (every gun row) [V field values]

Gun AtkParam_Pc rows clone 10170000 (dagger, proven with spawn_bullet) with overrides:
`atkPhys = inv_er(s*mw2_band, d_ref)`; `isAddBaseAtk 1`; `atkPhys/Mag/Fire/Thun/Dark/StamCorrection 0`;
`guardAtkRateCorrection 0`; `guardBreakCorrection 0`; `isDisableBothHandsAtkBonus 1`;
`spEffectAtkPowerCorrectRate_byPoint 0` (byRate/byDmg stay 100 as in vanilla flat rows: buffs still scale damage,
calibrate with no buffs); `disableGuard 0` (shields still block); `atkAttribute 3` (Standard; the clone has 2 pierce,
c4311 thrust 1.1; the flat samples were measured with 3); selfTarget 0, opposeTarget 1, isHitBothTeam 0. Stagger
fields from the clone (atkStam 5, dmgLevel 8 ...) kept [I tuning].

Gun Bullet rows = today's UMP45 overrides with atkId changed: speed 999 m/s, life 0.15 s, dist 150, gravity 0,
homing 0, HitBulletID 79000900, createLimitGroupId 0 — **plus the bug-2 change** `sfxId_Hit = -1`,
`sfxId_Flick = -1` (section 10.2), behind `[regulation] bullet_sfx = "none" | "dagger"`.

### 9.3 Calibration: target-damage inversion replaces linear k [I fit]

`atkPhys = round(inv_er(s * mw2_damage, d_ref))`, inv_er = monotone bisection of the community curve
(`build_regulation.er_damage` / `mw2er_core::ac130_dmg::er_damage`). Defaults `[damage] mode = "flat_inv"`,
`s = 1.225` (= 49/40, so UMP45 close = 49), `d_ref = 85` (c4311 effective defence fitted from flat AC-130 hits:
25 mm atk 116 -> 61 measured twice, curve 61.3; 40 mm core+ring -> 196 = 142.3 x 1.38 opener).

| MW2 | 15 | 20 | 25 | 30 | 35 | 40 | 55 | 70 | 75 | 135 | 1000 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| atkPhys | 67 | 75 | 82 | 88 | 96 | 103 | 122 | 141 | 148 | 231 | 1361 |
| ER on c4311 | 18.2 | 24.4 | — | 36.4 | 43.2 | **49.3** | 67.0 | 85.4 | 92.2 | 165.3 | 1225 |

- Linear k = 2.566 (also 49 at 40) would break MW2 ratios by -71 % / +43 % [I fit]; inversion keeps them exact on
  c4311.
- **Visible change**: UMP45 far band becomes 43 (35 x 1.225) instead of 49: 6 hits to kill at range instead of 5.
  Not a regression; tell the user.
- Sensitivity: d_ref 75 / 95 -> UMP45 close 54.7 / 44.6 (±11 %).
- AC-130 rows keep their own linear k 0.65 (verified in game; untouched).
- `[damage] k` is kept only for the legacy rows; `mode = "linear"` rebuilds with k (A/B).
- rows.json schema 2: `guns.{name}.{g, class, bands[{id, mw2, atk}], launcher{...}}`, `atk_mode "flat_inv"`, `s`,
  `d_ref`. `mw2er_core::rows` checks atk with a Rust `inv_er` mirror instead of `round(mw2*k)` (rows.rs:116/195
  [V]); a real-data test compares the Python and Rust tables row by row.

### 9.4 Launcher rows [I]

- kind 5 idx 0 (impact): Bullet clone of the gun hitscan row (hitscan at the impact point, `flight = "dll"`) and,
  for `flight = "engine"`, idx 2 is a projectile Bullet: initVellocity = speed x 0.0254 (AT4 63.5, M79 61.0, RPG
  38.1, Stinger 101.6, Javelin 15.2 m/s), life = min(150 m / v, 10 s), gravity only M79, hitRadius 0.15,
  HitBulletID = kind-6 single row. Impact Atk: MW2 1000 -> 1361, M79 135 -> 231.
- kind 5 idx 1: marker (atkId -1), for the engine path's explosion detection.
- kind 6 explosion tiers (clone 10030001 Fire Pot, full AC130_FLAT_ATK incl. disableGuard 1, atkObj 100 + bush flag,
  M79 atkObj 60): ring = inv(s*dmg(3r/4)); core solved so er(core)+er(ring) = s*dmg(r/4); single = inv(s*dmg(r/2)).
  AT4/Stinger r 10.16 m ring 154 core 116 single 187; RPG 160/116/194; M79 r 7.62 m 126/135/167; Javelin
  r 22.86 m 198/217/284.

### 9.5 Headshots

M6 v1: no head rows; ER's NpcParam weakPartsDamageRate (c4311 1.5, c4600 2.0) applies to hit parts [I]; one run-13
sample 73 = 49 x 1.49 fits. Kinds 1-3 stay reserved (`[damage] dll_headshots = false`) to avoid stacking x1.4 x1.5.
Known deviation: MW2 shotguns have locHead 1.0 but ER weak parts still apply.

### 9.6 Builder and verify changes

- `build_regulation.py`: plan from `[arsenal] guns` + stats.json; `SAFETY_M6` field dict in verify() for gun and
  launcher rows; edit_xml unchanged (single sorted block below 81000000 [V]).
- Self-test mutations added: isAddBaseAtk 0 on a gun row; atkPhysCorrection 100; atkAttribute 2; band atk off by 1
  vs inversion; gun disableGuard 1; launcher marker atkId != -1; projectile selfTarget 1; duplicate g; sfxId_Hit
  != -1 with bullet_sfx = "none".
- check_m4 check 5 pins `REGULATION PASS bullet+14 atk+13 speffect+1`, `SELF-TEST PASS 18/18` and package sha
  6bcd3558... (check_m4.ps1:39,194 [V]); check_m5 runs check_m4. Generalise: the expected counts are read from
  rows.json (`REGULATION PASS bullet+<n> atk+<m> speffect+1`), self-test count from the builder, and the sha pin
  moves to `work\m6\regulation.sha256` written by check_m6 after a clean build (as was done in run 9). check_m4's
  standalone run keeps passing with the new counts.

### 9.7 In-DLL PARAMS check for N guns [V accessors exist]

For every gun in rows.json: atkId_Bullet, HitBulletID == marker, initVellocity, atkPhys == planned, plus
`is_add_base_atk()`, `atk_phys_correction() == 0`, `is_disable_both_hands_atk_bonus()`,
`sp_effect_atk_power_correct_rate_by_point() == 0`, `disable_guard()` as planned, atk_attribute 3, self_target false;
launchers: marker atkId -1, explosion hitRadius. Mismatch disables that gun only:
`PARAMS FAIL gun=<name> id=<id> field=<f> live=<v> want=<w>`. Summary
`PARAMS OK guns=<n> rows=<r> mode=flat_inv s=1.225 d_ref=85`. The legacy `PARAMS OK n=` line is kept for old
fixtures (logged when legacy rows are checked).

---

## 10. Run-13 bug fixes

### 10.1 Bug 1: F7 AC-130 ends at once with `AC130 off reason=loading t_s=0.0` [V root cause]

**Cause.** `Ac130Ctl.load_edge/mask_edge/talk_edge` (ac130.rs:158-160) are armed only in begin_ride (ac130.rs:767-771,
Starting -> Active) and never cleared (`end()` at ac130.rs:1171). `ride_tick` builds
`ExitIn.loading = load_edge.changed(load_counter)` (ac130.rs:692) also during Starting, and Starting ends on any exit
trigger (ac130_logic.rs:453-458). `EdgeWatch::changed` is true when the saved value is Some and differs
(fp_logic.rs:259). Run 13: last Active ride 20:19:06 with load=1; death reload raised the counter to 2 (20:19:45);
from 20:22:22.963 every F7 passed the gate (`AC130 ready`, PROTECT apply ok=1), then on the next tick compared stale 1
with 2 -> `reason=loading t_s=0.0`, 19 times to 20:22:43.843. Aborted starts do not charge the cooldown (charged only
if t_active is Some), so each press re-ran PROTECT apply/restore, FP off/on and MOVE suspend/resync. `FP blocked
reason=f6` is the user's own F6 (line 12106), unrelated. mask_edge/talk_edge have the same latent bug (values happened
to stay 0x0 / -1).

**Fix.**
1. New pure `mw2er-core::ac130_logic::RideEdges { load: EdgeWatch<i32>, mask: EdgeWatch<u32>, talk: EdgeWatch<i32> }`
   with `arm(load, mask, talk)`, `clear()`, `exit_flags(load, mask, talk) -> (bool, bool, bool)`.
2. Arm from this tick's FpWorld values in `try_start` right after `machine.start(...)` succeeds (ac130.rs:583), so a
   load during Starting is caught against a fresh value; begin_ride does not re-arm. fade_watch stays skipped during
   Starting.
3. `end()` clears RideEdges and fade_watch.
4. Debounce in `Ac130Machine`: an abort before Active with an environment reason (loading, cutscene, talk, camtype,
   read_fail, protect_fail) sets `start_backoff_until = now + 1.0 s` (`[ac130] start_backoff_s = 1.0`); `check()`
   returns `Block::Backoff`; the controller logs one `AC130 blocked reason=retry_backoff` per backoff and merges
   identical aborts: `AC130 start aborted reason=loading x<N> in <T>s` (flushed on the next different line or after
   5 s).

**Tests** (mw2er-core, pure):
- `ac130_edges_no_carry_over`: arm(1,0,-1); clear(); counter -> 2; new start arms at 2 -> flags all false; counter
  -> 3 -> loading true.
- `ac130_machine_start_after_reload_activates`: run-13 order (ride at load 1 -> end -> counter 2 -> start): reaches
  Active; a change during Starting ends with Reason::Loading.
- `ac130_mask_talk_no_carry_over`: same for mask and talk.
- `ac130_abort_backoff`: two F7 within 1 s -> one attempt + Block::Backoff; after 1 s a new attempt.
- `ac130_abort_log_merge`: 19 identical aborts -> 1 merged line.
- Wiring check (m2_static_checks `ac130wiring`): `end()` calls `edges.clear()`; `try_start` calls `edges.arm(`;
  begin_ride no longer assigns `load_edge`. Mutations: remove clear -> caught; arm in begin_ride only -> caught.
- Analyzer fixture: the run-13 session excerpt -> row `ac130 start` FAIL on the old log (flags `AC130 off
  reason=loading t_s=0.0` while `TASK world` in-world and no `AC130 on`), PASS on a fixed fixture.

### 10.2 Bug 2: `SPAWN FAIL code=4 (construction failed)` [V timeline, I cause]

**Evidence.** 36 fails 20:19:03.511-20:19:43.287 (run 13), 4 in run 6 (no interact). Last success n=214 at
20:19:02.328; the interact (20:19:02.345, anim_flags 0x11 = same as normal firing) is a coincidence. Fails survived
MODE off/on, reload, warp, a 2nd interact, death; rare successes inside the episode (run 13 n=218, run 6 n=256/257).
Cured only by the world reload (20:19:47.475): next 99 shots 0 fails, 49 per hit. Owner handle unchanged
(0xffffffff16f00000); ChrIns pointer changed. Per-world spawn counts: fail at ~250 total spawns (run 6 at UMP shot
255; run 13 at UMP shot 214 after 31 AC-130 shells + explosion bullets); a 216-spawn world had 0 fails.

**Hypothesis ranking.** H1 [I, leading]: a per-world list in CSBulletManager fills after ~250 bullets carrying the
dagger's hit effects (sfxId_Hit 300002 / Flick 300003, kept from the clone [V M2_DESIGN:795]) and only a world reload
empties it; CSBulletManager has `BufferAndAllocLinkedList<CSBulletIns,128,128>` at +0x00 and `<[u8;0x9d0],64,192>`
"sfx related" at +0x20 (both hold 256) [V vendored layout]. H2 (owner/ChrIns state) unlikely, not excluded. H3 origin
in geometry, H4 ammo, H5 bad row id: rejected with evidence.

**Instrumentation (always on, read-only by offset, no writes).**
- Fields: on every SPAWN FAIL, every 50 spawns and on world enter, read CSBulletManager (game::singleton_addr) lists at
  +0x00 / +0x20 / +0x40: head != 0, empty_spot (+0x10) == 0, allocated_count (+0x18) vs heap limit (128/192/28),
  allocations_counter (+0x1c). Walk live bullets (MAX_WALK) grouped by owner and param id (ours: main vs marker).
  Count spawns since world enter by family (gun main, pellet, AC-130 shell, explosion, launcher impact).
  Player state (hp, anim_flags, mod_flags, movement_limit), ChrIns ptr vs handle.
- Once per breaker opening: one diagnostic spawn of the inert marker 79000900 with the same owner
  (`marker_probe=ok|fail`).
- Line: `SPAWN FAIL code=4 cause=<pool_sfx_full|pool_bullet_full|owner|unknown> bullets=<alloc>/<cap>
  sfx=<alloc>/<cap> sfx_empty_spot=<0|1> live_ours=<n> live_all=<n> spawns_world=<n> marker_probe=<ok|fail|->`.
  `cause` rule: sfx list prealloc full and heap at limit -> pool_sfx_full; bullet list -> pool_bullet_full;
  both lists with headroom and marker_probe fails -> owner; else unknown.
- Periodic: `SPAWN pool bullets=<a>/<c> sfx=<a>/<c> unk40=<a>/<c> counter=<n> spawns_world=<n>`.

**Fix / mitigation.**
1. Likely fix [I]: `sfxId_Hit = -1`, `sfxId_Flick = -1` on all M6 gun, pellet and impact rows (9.2). The legacy UMP45
   rows stay byte-identical (oracle) and are unused by default. Visual impact sparks then come from our tracer pass
   only (MW2 impact effects are out of scope).
2. Mitigations whatever the cause (v1):
   (a) cause-classified FAIL line (above);
   (b) cause=owner: re-read the owner handle from the current ChrIns and retry the shot once on the next tick
       (`SPAWN retry cause=owner ok=<0|1>`);
   (c) pellets gated on pool headroom (6.4); launchers fly in the DLL (fewer engine bullets);
   (d) breaker stays (8 consecutive per-shot failures) and logs pool counters on every open:
       `SPAWN breaker open fails=8 cause=<c> pool=...`.
   **Decision**: no damage fallback by writing enemy HP (it would bypass ER death, aggro and drops). A non-bullet
   damage path is deferred until the cause is known.
3. If the next run shows H1 with sfx -1 still filling: switch impact detection to bullet disappearance and drop
   HitBulletID from gun rows (halves engine bullets) — designed, not built in M6.

**Tests.**
- `spawn_cause_classify` (pure, mw2er-core): list snapshots -> cause strings, incl. the rare-success pattern.
- `pool_reader_offsets`: offsets match the vendored struct (`offset_of!` on a mirror struct) [V layout].
- `breaker_per_shot`: an 8-pellet shot with 8 failed pellets counts 1 failure; 8 such shots open the breaker.
- Regulation round trip: every M6 gun row has sfxId_Hit/Flick -1; legacy rows unchanged.
- Analyzer fixture from run 13 (`SPAWN FAIL` x36) -> row `spawn` FAIL with `cause=?` (old lines) and a synthetic fixed
  fixture with cause and pool fields -> PASS.

---

## 11. HUD

- Bottom-right ammo block becomes `<WEAPON NAME>  mag / reserve` (names from section 2 user names; data, not MW2
  strings; launchers show `1 / 1`). Low-ammo colour at ≤ 25 % of clip (MW2 style [I]).
- Switch indicator: during Dropping/Raising show the target name and a small list `prev  [CUR]  next` for 1.5 s.
- Fire mode tag for burst/double barrel (`BURST`, `2x`).
- Segmented reload: per-shell count updates live.
- Crosshair: per-gun spread from SpreadState; hidden at full ADS; hidden while scoped.
- Scope overlay pass (6.5); `HOLD BREATH` hint not drawn (MW2 shows none) [I].
- Lock: brackets on the target (Stinger), point box (Javelin), `LOCKING` / `LOCKED` / `NO LOCK` text, beep SFX from
  MW2 aliases if present in the MP zone (`stinger_locking`, `stinger_locked` [I alias names, resolved by the
  inventory or skipped]).
- `[hud] weapon_name = true`, `switch_list = true`, `lock_ui = true`.

---

## 12. Logging and analyzer

### 12.1 New / changed log lines

| line | when |
|---|---|
| `ARSENAL loaded n=34 ok=<k> ms=<t> vram_mb=<m>` / `ARSENAL disable gun= why=` | boot / failure |
| `ARSENAL cfg guns=<n> start=<g> reserve_source= flight= lock= pellet_bands=` | config generation change |
| `SWITCH start/raised/blocked/wait ...` | 5.2 |
| `SHOT gun=<n> n=<shot#> pellets=<k> bands=<ids> ads=<f> kick=(p,y) capped=<0|1> ...` | every shot |
| `HIT ... gun=<n> pellet=<i>` | attribution |
| `RECHAMBER start/done gun= t=` | 6.2 |
| `RELOAD shell gun= mag= reserve=`, `RELOAD end gun=` | 6.3 |
| `BURST gun= n=3 t_ms=` | 6.1 |
| `BREATH hold / gasp why=` | 6.5 |
| `LOCK ...`, `LAUNCH ...`, `PROJ hit/explode ...` | 6.6 |
| `MOVE class gun= class= scale=` | 7 |
| `PARAMS OK guns= rows= mode=flat_inv s= d_ref=` / `PARAMS FAIL gun= id= field= live= want=` | 9.7 |
| `AC130 blocked reason=retry_backoff`, `AC130 start aborted reason=<r> x<N> in <T>s` | 10.1 |
| `SPAWN FAIL code= cause= ...`, `SPAWN pool ...`, `SPAWN breaker open ...` | 10.2 |
| PERF gains `kick_stale`, `proj_live`, `spawns_tick_max` | 1 Hz |

### 12.2 Analyzer (`tools\scripts\analyze_log.py`) rows

- `arsenal` (load ok count, disabled guns), `switching` (counts by via, drop/raise durations vs dropTime/raiseTime
  ±0.1 s, no SHOT while switching).
- `shots per gun` / `hits per gun`: SHOT/HIT counts, ER damage per HIT vs expected `er(atk)` for c4311
  (tagging ~1.4x first hits `opener`, ~1.5x/2.0x `weak-part`), fitted D_eff per character, suggested d_ref.
- `ump45 calibration`: close-band HITs on c4311 = 49 ± 1 (excluding opener/weak-part) -> PASS.
- `burst` (3 shots ≤ 3*fireTime+tick), `rechamber` (cycle vs fireTime+rechamberTime ±1 tick), `shell reload`
  (shell cadence vs reloadTime), `pellets` (HITs per shot ≤ N), `launchers` (LAUNCH -> PROJ hit/explode pairing,
  explosion HITs), `lock`.
- `ac130 start`: FAIL on `AC130 off reason=loading t_s=0.0` while in world and no `AC130 on`; counts merged aborts.
- `spawn`: SPAWN FAIL totals by cause, pool maxima, spawns_world at first fail; FAIL if any `cause=unknown` with
  pool data present, NEEDS-DATA for old lines without cause.
- Fixtures: `fixtures\m6_arsenal_session.log` (synthetic, every row), `fixtures\run13_bugs.log` (real excerpt, lines
  11079-11280 and 12021-12110 of mw2er.log, only mod-generated text). Mutations ≥ 10 caught.

---

## 13. Offline oracles (pass criteria)

### 13.1 Converter known answers (`convert_arsenal.py --check`, `m6_static_checks.py convert`)

For every gun:
- stats.json fields equal the weapon file (every column of section 2 table, exact for strings, ±1e-6 for floats);
  move_class from weaponClass per 7.
- every anim field resolves (case-insensitive) to a written clip or a documented fallback; 0 MISSING.
- clip duration vs time field: |duration/rate - time| ≤ 1 tick (1/60 s) after scaling; raw pairs listed in a table
  that must match the inventory (spas12 rechamber 0.933 vs 0.467, m240 9.7 vs 7.75, p90 3.1 vs 2.76, m79 3.1 vs 2.6,
  javelin 4.5 vs 3, ranger 3.17 vs 2.53 — known answers).
- bone sets: anim bones ⊆ model bones ∪ per-gun whitelist (rpd j_bullet17-20 only); bones ≤ 256, verts ≤ 65,535;
  delta-translation proof per gun (M3 1m rule) for rigid and animated types.
- tag_sight_on present exactly on the 15 guns that have it (known list); 0xFFFF on the other 19.
- sounds: fire alias resolves in localized_common_mp for all 34 (known answers: spas12 weap_ranger_fire_plr,
  model1887 weap_winch1200_fire_plr, m21 weap_m14sniper_fire_plr, rpd weap_degtyarev_rpd_fire_plr, p90
  weap_fnp90_fire_plr, striker weap_stryker_fire_plr); every PCM output's sample count = WAV data / block align;
  SHA of decoded PCM equals the source data chunk; UMP45 fire PCM byte-identical to the M3 copy.
- notetrack cues: UMP45 reload clipout 15/75 = 0.20, clipin 54/75 = 0.72; cheytac rechamber 3/28, 7/28, 16/28,
  19/28; spas12 loop shell 10/26.
- index.json: 34 entries, ok = true, manifest shas match files.
- RPG rocket part present and parented to tag_clip.
- scope overlay vmtex: 1024x1024 BC3, alpha centre transparent, edge opaque (sampled).
- Determinism: two runs give identical shas.

### 13.2 vmtest (offscreen D3D12, existing harness)

- All 34 guns render idle frame 0, ADS end frame, reload mid frame with the debug layer on: 0 errors, 0 warnings
  (except the known allowlist from M3), non-empty coverage.
- IoU vs Blender reference silhouettes ≥ 0.97 (M3 threshold) for one gun per class: m4, ump45, mg4, cheytac, spas12,
  striker (rigid), rpg (with rocket part), plus javelin (0-frame idle). Blender renders done by the existing
  `tools\scripts` Blender reference script with `--gun` (offline, Blender already used in M3).
- ADS alignment: on the 8 sample guns the iron sight / scope eye point projects within 2 % of screen centre at ADS
  end [I threshold, as M3].
- Switching render: drop of A then raise of B renders both with no resource rebuild (counter = 0 uploads).
- GPU memory after loading all 34 ≤ 128 MB (estimate 65 MB).

### 13.3 Pure-logic tests (mw2er-core, cargo test)

Fire modes: burst at 20/60/144 Hz exactly 3 shots spaced fireTime; held trigger one burst; 10 ms tap 3 rounds;
reload deferred mid-burst; sprint refused mid-burst; double barrel 2 presses -> 2 shots ≥ 0.01 s apart, both_barrels
variant. Rechamber: cycles 0.916/0.937/1.43 s; no rechamber on last round; ADS clip choice; cancel -> chambered false
-> rechamber after raise. Shell reload: SPAS from 0 -> 8 in 6.67 s ± tick; add-time clamp; fire interrupt finishes
segment then end then shot; cancel keeps committed shells; striker/m1014/1887 totals. Pellets: N directions, all
inside cone, mean radius within 5 % of the uniform-radius expectation over 10k samples (seeded); ADS lerp; range cap;
breaker per shot. Projectiles: rocket straight-line position at t; M79 apex and range (≈183 m at 45 deg) vs closed
form ±0.5 %; arming dud before 9.525 m; lifetime expiry; PN missile hits a crossing target with lateral accel ≤
254 m/s^2; Javelin top attack apex h formula and direct fallback; lock timers (1.0 s / 0.5 s grace; Javelin 12
samples 25 ms, 1.15 s, min dist). Switching: every transition of 5.2, retarget, wheel multi-notch, AC-130 active
block, per-gun ammo kept across 100 random switches and MODE off/on, disabled guns skipped, only-one-gun block.
Ammo: reserve_source max/start for all 34. Kick: peak = v^2/(2c) ± 1 %, returns to 0, never crosses. Sway / breath:
hold reduces amplitude to ≤ 5 % in 1 s, gasp x4.5, Shift key-up rule. Movement class per gun. Clip rate scaling incl.
duration 0. Regulation mirror: Rust inv_er == Python table for every planned row (real data). Bug 1 and bug 2 tests
of section 10. Config: `[arsenal]`, `[launchers]`, `[kick]`, new keys, conflicts, unknown key error, old
`gameplay.weapon` alias.

### 13.4 Regulation round trip

`build_regulation.py --check`: `REGULATION PASS bullet+<~288> atk+<~282> speffect+1` (exact numbers from rows.json),
self-test all mutations caught, 191 untouched params byte-identical, legacy 13 M2 rows byte-identical (check_m3b
check 8), AC-130 rows unchanged, every M6 row matches SAFETY_M6, UMP45 rows 79112000..005 atk 103..96, inversion
reproduces s*mw2 on d_ref 85 within ±0.5 ER damage for all bands, sfxId_Hit/Flick -1 on M6 rows.

### 13.5 Build, chain, proof strings, hygiene

- `cargo build --release --workspace`: 0 warnings. `cargo test --workspace`: 0 failed.
- check_m5.ps1 (which runs check_m4/m3b/m3/m2) PASS with the generalised check_m4 check 5.
- DLL proof strings in built == staged mw2er.dll: `ARSENAL loaded`, `SWITCH start`, `SWITCH raised`,
  `RECHAMBER start`, `RELOAD shell`, `BREATH hold`, `LOCK acquired`, `LAUNCH gun=`, `PROJ explode`,
  `PARAMS OK guns=`, `PARAMS FAIL gun=`, `MOVE class gun=`, `AC130 start aborted reason=`,
  `AC130 blocked reason=retry_backoff`, `SPAWN FAIL code=`, `cause=`, `SPAWN pool`, `marker_probe=`.
- Hygiene: `m2_static_checks.py hygiene` (no MW2 data, names-only tables allowed as in M5; the user-name map in
  section 2 is design text, the runtime reads names from config/stats.json), no WriteProcessMemory/VirtualQuery,
  no writes outside game::write, converted\ and work\ gitignored, no file in a game folder touched (mtime scan of
  the ER and MW2 folders as in check_m5 check 11).

---

## 14. check_m6.ps1

`tools\scripts\check_m6.ps1 [-AllowSkip] [-SkipM5]`, Windows PowerShell 5.1, same structure as check_m5 (PATH line,
`$Py -I`, results table, exit 0 only if all PASS). Writes only work\check_m6\, work\m6\ and what sub-checks write.

| # | check | pass |
|---|---|---|
| 1 | M5 suite (`check_m5.ps1`, runs M4/M3b/M3/M2) | exit 0 |
| 2 | inventory fresh (`inventory_m6.py`, 34 guns, sha of inputs unchanged or rebuilt) | 34 ok |
| 3 | converter (`convert_arsenal.py --check`, 13.1) | 34 guns PASS, deterministic |
| 4 | regulation (13.4) + sha pin `work\m6\regulation.sha256` | PASS |
| 5 | cargo build 0 warnings (workspace) | 0 warnings |
| 6 | cargo test (13.3 categories present by name + real-data tests) | 0 failed, categories ≥ listed |
| 7 | vmtest arsenal (13.2: all-render, IoU sample, ADS alignment, switch no-upload, VRAM) | PASS |
| 8 | config (`mw2er.toml` new sections/defaults, key conflicts, alias) via tomllib and DLL parser | PASS |
| 9 | wiring (`m2_static_checks.py arsenalwiring`, `ac130wiring` incl. edges arm/clear, spawn cause wiring) + mutations | all mutations caught |
| 10 | DLL proof strings (13.5) | all present, built == staged |
| 11 | analyzer (fixtures m6_arsenal_session.log, run13_bugs.log; mutations) | expected rows / ≥ 10 mutations caught |
| 12 | hygiene + no game-folder writes | PASS |

Final line: `M6 OFFLINE ORACLES: PASS`.

---

## 15. In-game test (run 14) — the user runs it; Claude never launches

Preparation (offline): check_m6 PASS; staged DLL + regulation via launch.ps1 as before.

1. **Boot**: expect `ARSENAL loaded n=34 ok=34`, `PARAMS OK guns=34 ...`. F5 MW2 mode: UMP45 raised.
2. **Calibration**: shoot a Godrick Soldier (c4311) close with the UMP45, then at > 22 m (850 in). Expect `HIT` 49
   close (first hit may be ~69 opener), ~43 far. Analyzer `ump45 calibration` PASS.
3. **Switching**: wheel down x3, wheel up x3, key 1 x2, key 2 x2. Expect `SWITCH start ... via=wheel|key1|key2` and
   `SWITCH raised`, drop/raise anims visible, no ER reaction to 1/2/wheel (else note `MOVE keyprobe` with MW2 off:
   press 1, 2, wheel once each). Switch during a reload: ammo state kept on return.
4. **One gun per class** on c4311: M4 full auto, FAMAS burst (`BURST n=3`), FAL single, MG4 (slower move: `MOVE class
   ... scale=0.875`), Intervention scoped (overlay, FOV 15, rechamber `RECHAMBER start/done`, Shift hold breath
   `BREATH hold`, gasp after 4.5 s), SPAS (pellets `SHOT pellets=8`, pump, shell reload `RELOAD shell`), Ranger
   double tap, AA-12 one full mag (watch `SPAWN pool`), AT4 at a wall and at an enemy (`LAUNCH`, `PROJ explode`,
   explosion HITs), Thumper close (< 9.5 m: dud) and far, Stinger ADS on an enemy 1 s (`LOCK acquired`, fire, homing),
   fire without lock (`NO LOCK`), Javelin lock and top attack.
5. **Recoil**: hold fire with M4 and Barrett single shots; kick visible and recentring; bullets follow the
   crosshair (`kick_stale` ≈ 0 in PERF).
6. **Bug 1**: AC-130 ride (F7), let it end, die or fast-travel (load), then F7 again: expect `AC130 on` (no
   `reason=loading t_s=0.0`). Spam F7 during a load screen: one `AC130 blocked reason=retry_backoff` and merged
   `AC130 start aborted ...` lines.
7. **Bug 2**: in one world fire > 400 rounds total (AA-12 + UMP45) with one AC-130 ride, without reloading the world.
   Expect 0 `SPAWN FAIL`; if any appear they carry `cause=` and pool counters (send the log).
8. Close the game normally; run `analyze_log.py` on the last session.

Review additions (section 18):
9. **Second calibration point**: >= 3 non-kill body hits on c4311 with the M4 (mw2 30 -> ~36) and with the
   Intervention (mw2 105 -> ~129, a one-shot kill of the 219 HP soldier is expected). `hits per gun` prints D_eff per atk
   level; NEEDS-CALIBRATION when the levels spread > 10 % (MW2 ratios not kept on c4311: report the numbers).
10. **Switch retarget**: press 1 then 2 quickly (and wheel down then up) during a drop: the same gun raises again
    (`SWITCH retarget` then `SWITCH raise gun=<same>`), ADS / scope work right after; never `SWITCH vm_unstuck`.
11. **AC-130 keys 1/2**: in a ride, 1 and 2 select the 105 / 40 mm (`AC130 SWITCH gun=105 via=key`, `gun=40 via=key`); no `SWITCH start` of the arsenal during the ride.
12. **Launcher self guard**: AT4 / RPG at a wall 2-3 m away: `PROJ hit kind=dud ... why=self_radius`, no player HP
    loss. Far blasts: `PROJ selfcheck ... lost=0`. Javelin locked on a target ~100 m away arrives in ~3 s (2-5 s).

Pass: every analyzer M6 row PASS or NEEDS-DATA only for steps skipped; no `SPAWN FAIL cause=unknown`; no
`AC130 off reason=loading t_s=0.0` in world.

---

## 16. Verified vs inferred summary

Verified: names, gunModels, every weapon-file number in section 2, anims/sounds present, converter dry run, sound
zone order, notetrack cue times, move classes from _weapons.gsc, MW2 binds 1/2 weapnext and Shift breath_sprint,
burst code path, validation rejections, sniper overlay format and material, lock-on constants and MW2 target rules,
free id ranges and row limits, flat field set of vanilla rows, bug-1 root cause and timeline, bug-2 timeline,
CSBulletManager layout, PARAMS accessors.

Inferred (config-switched, verified in run 14): rechamber ordering, shell-reload interrupt, pellet spread rule and
range cap, double barrel behaviour, viewkick model/scale, sway amplitude, MP hold-breath equal to SP, rocket flight
without gravity, M79 dud/impact, lock-on ER adaptation (deviation), Javelin apex, reserve rule, D_ref 85 / s 1.225
(UMP45 49 preserved by construction, other enemies approximate), weak-part headshots, x1.4 opener, bug-2 cause H1 and
the sfx -1 fix, ER use of 1/2/wheel, VRAM/boot estimates.

---

## 17. Implementation notes (as built, 2026-10-17; offline only, NOT run in game)

Labels: [V] verified offline by an oracle, [I] inferred (needs run 14).

Built as designed: converter `tools\scripts\convert_arsenal.py` (34/34 guns, `--check` known answers, `--self-test`
11 mutations), regulation rows 79100000 + g*1000 + kind*100 + idx with flat_inv damage (s 1.225, d_ref 85; UMP45
79112000..005 atk 103..96, c4311 close 49.3 / far 43.2 [V inversion, I in game]), mw2er-core modules `arsenal`,
`switch_logic`, `kick`, `projectile`, `spawn_cause` (+ weapon_sim / vm_anim / vm_assets / spread / rows / config /
sounds extensions), the DLL's `arsenal_rt` / `arsenal_task` and the task / hooks / HUD / audio / viewmodel wiring,
bug-1 (`RideEdges` armed in try_start, cleared in end, `Block::Backoff`, merged abort lines) and bug-2 (cause + pool
counters, `sfxId_Hit`/`sfxId_Flick` -1, breaker per shot, owner retry, pellet pool gate) fixes.

Deviations from the text above (each recorded in MODLOG):
1. rows.json keeps schema 1 and gains an additive `arsenal` block; the legacy UMP45 rows 79000100-105 and
   `[regulation.weapons]` stay in the package (`[regulation.arsenal] legacy_ump45` = A/B switch).
2. tag_sight_on exists on 16 guns, not 15 (the converter's known-answer list follows the data) [V].
3. The engine-projectile launcher row (kind 5 idx 2) is not built (launchers fly in the DLL): 5 Bullet + 4 AtkParam
   rows per launcher, so the package adds bullet+283 atk+277 (design: ~288 / ~282) [V].
4. Gun kick is a viewmodel push-back along the view axis [I]; the scope overlay is drawn as a black alpha mask over
   the frame (lens see-through, ER's zoomed world behind it) [V vmtest AR6].
5. Sounds are copied as WAV (not through sndconv); launcher explosions reuse the AC-130 40 mm explosion sound [I].
6. The Stinger's colour map is DXT3: BC2 support added to the converter and renderer [V].
7. reload_empty falls back to reload for the 7 guns without an empty-reload clip [V].
8. Projectile steps are clamped to 0.25 s; mouse sensitivity is scaled in the DirectInput mouse buffer; the lock cone
   uses ER's fov [I].
9. The camera gate is fed ER's own fov while the view-fx zoom is applied (a sniper's zoomed fov ~0.18 rad is below
   the gate's 0.2 rad floor) [V wiring 8]; a switch-machine reset mid-switch cancels the viewmodel drop
   (`VmAnim::cancel_switch`) [V test]; `disable_gameplay` hands the weapon keys, wheel, sensitivity and view fx back.
10. vmtest (13.2): IoU uses the M3 thresholds (>= 0.90 PASS, < 0.95 WARN); measured minimum 0.9995 on 31 masks of the
    8 guns, so >= 0.97 holds [V]. ADS alignment: tag_sight_on sits on the rear sight close to the eye, so the check is
    its column (<= 1 % of the width, worst 16.7 px on the Vector) rather than 2 % of the centre; the vertical sight
    picture is reported only (the post tips were checked by eye on the PNGs; the Ranger's ADS clip is one frame of
    tag_ads and leaves the gun low right, as converted). AT4 / Stinger / Javelin lower the launcher off screen in the
    middle of their reload, so a `reload_early` (10 %) pose is rendered too; masks empty in both renderers are not
    counted. blender_ref.py `--gun` applies hideTags per primitive; Blender's importer drops degenerate and repeated
    triangles (MG4: 12 repeated), which the triangle assertion counts.
11. Analyzer: `spawn` FAILs on any SPAWN FAIL that carries a cause (the design FAILs only `cause=unknown`); a line
    without a cause is NEEDS-DATA. In an arsenal session the M2 rows judge the MODE-on gun (its arsenal bands join
    the legacy ones; HIT lines with gun= go to `hits per gun` / `ump45 calibration`).
12. Converter checks live in `convert_arsenal.py --check` (not `m6_static_checks.py convert`); wiring in
    `m2_static_checks.py arsenalwiring` (10 checks, 15 mutations) / `arsenalstrings`.
13. check_m4.ps1 check 5 pins the new package (sha 4cee2081ec89..., bullet+283 atk+277, self-test >= 18) and
    check_m5.ps1 check 7 expects `weapon_move_class = "auto"` (the M6 default) - both updated in place.
14. PERF gains `kick_stale`, `viewfx_writes`, `proj_live`, `spawns_world` (no `spawns_tick_max`).

---

## 18. Review applied (2026-10-17; offline only, NOT run in game)

Fixed (each with a test, a wiring check or an analyzer mutation):
- Switching: a drop retargeted back to the dropping gun (1 then 2, wheel down then up) now ends in `RaiseStart` of that
  gun; before, the machine went Ready without an event and the viewmodel stayed in Drop (gun hidden, ADS / scope / fire
  anims dead). Safety net in task.rs: machine Ready + viewmodel switching + no raise queued -> `cancel_switch()` and
  `SWITCH vm_unstuck` (the analyzer FAILs on it) [V tests switch_logic + vm_anim, wiring 12].
- AC-130 keys 1/2 (= weapon keys): their raw edges are recorded before the weapon-key blanking, the hook never blanks the
  weapon keys while `WANT_GUNSHIP_INPUT`, and the task stores `WANT_BLANK_WEAPON` / `WANT_WHEEL` only when `!ac.active`
  [V wiring 11 + 3 mutations; I in game].
- Launcher self guard (deviation 16): an explosion whose largest sphere + `[launchers] self_guard_margin_m` (0.5 m)
  reaches the player's capsule is a dud (`PROJ hit kind=dud ... why=self_radius`); ER's handling of selfTarget 0 on the
  cloned Fire Pot rows is unverified. MW2 itself lets the shooter hurt himself; this guard is stricter. Every blast logs
  `PROJ selfcheck ... hp_before= hp_after= lost=` 0.6 s later; the analyzer `launchers` row FAILs on lost > 0.
- Projectiles of an old havok frame: a re-base (> 5 m jump) or a pos_w change expires them (`PROJ hit kind=expire ...
  why=rebase|frame`) and resets the lock.
- Bursts stop when the gun may no longer fire (menu, focus, AC-130 start, breaker): `SHOT burst cancelled why=`.
- A start gun whose viewmodel failed hands over to the first usable gun (`ARSENAL start gun disabled ... why=assets`).
- The viewmodel loader stops at a device-level failure (removed / hung / a 120 s stuck upload: `VmError::Stuck`) and
  marks the rest failed without trying.
- At most 64 guns (u64 flag masks): config error, and `arsenal_rt::load` disables extras (`why=slots`).
- Regulation reproducibility: WitchyBND runs `--singlethread` (its parallel mode failed 3 of 7 repacks of the M6-sized
  XMLs), one failed call is retried (`RETRY ...`), and build / self-test / rebaseline hold an exclusive lock file
  (`work\regulation\.build_regulation.lock`; a dead owner's lock is taken over). check_m4 / check_m6 no longer compare the
  sha pin of a package left by a failed build. Three concurrent builds + self-test: all PASS, same sha 838f6b9b7aa5.

Fidelity (deviations 15, 17, 18):
15. Body multiplier: ER has no hit locations, so every hit is a body hit at the weapon file's `locTorsoUpper` when > 1:
    Intervention / Barrett / WA2000 70 x 1.5 = 105 (atk 187, ~129 on c4311: the MW2 one-shot body kill), M21 70 x 1.1
    = 77 (atk 150); every other gun x1 (UMP45 rows unchanged, 103..96). locTorsoLower 1.1 and locNeck are not used
    (one body value). Rust `GunSpec::body_mult` == Python `body_mult`; converter known answer `KA_BODY_MULT`, regulation
    known answer + self-test mutation. Package sha 838f6b9b7aa5... (check_m4 pin + work\m6\regulation.sha256 updated).
17. Javelin flight [I]: after ignition it boosts from projectileSpeed (15.2 m/s) at 40 m/s^2 to 60 m/s
    (`javelin_cruise_mps` / `javelin_accel_mps2`; MW2's missile dvars are not in the dumps) and its turn radius is
    capped at 8 m (`JAVELIN_TURN_RADIUS_M`; without the cap the faster missile circled short targets). Time to target:
    30 / 60 / 100 / 200 m -> 1.75 / 2.1 / 2.9 / 5.1 s (before: ~10 s for 100 m) [V test].
18. Fire clips never play faster than 2.5x (`FIRE_MAX_RATE`) [I]: Ranger 58x -> 2.5x (0.23 s kick), snipers 4.7-9.2x ->
    2.5x, FAMAS / Vector / MG4 / M240 ~4x -> 2.5x; UMP45 1.48x and most ARs unchanged. Whether IW4 stretches fire anims
    to fireTime is unverified; a bolt gun's rechamber still starts at fireTime and cuts the kick.

Analyzer: pellets counts the HITs of each shotgun shot (<= spawned pellets); launchers compares `LAUNCH v=` with
projectileSpeed (fixture now 63.5 / 101.6 m/s) and the selfcheck HP; shell reload judges each gun against its own
reloadTime; burst uses (n-1) x fireTime + a tick with n from fireType; hits per gun groups D_eff by atk level
(NEEDS-CALIBRATION when the levels spread > 10 %). 19 M6 FAIL mutations + 1 NEEDS-CALIBRATION case.

Not changed: none of the findings was rejected. Still [I] until run 14: ER's reaction to the guard / selfTarget, the
c4311 damage of every atk level other than UMP45 close, the Javelin cruise speed, the fire-clip cap look.
