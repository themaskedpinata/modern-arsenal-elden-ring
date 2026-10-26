# M2 design: first vertical slice (UMP45 in Elden Ring)

Status: design, 2026-10-13. Nothing in this document has run in game yet. Inputs: `MODDING_PLAN.md`, `MODLOG.md`,
the four research findings of this workflow (fromsoftware-rs, hudhook-dx12, regulation, local-recipes), and the
extra checks listed below that the designer ran while writing this.

## Evidence labels

Every decision carries one label.

- **[V] verified**: checked on this PC: a file read, a compile, a decoded param, an offline run.
- **[R] reported**: a field note (CS2 / Sunless / DSR / Let It Die) says it worked on the same ER build (or the named game). Not tested in this project.
- **[I] inference**: our design choice or a guess. Each one has a log line that confirms or refutes it on the first run.

Extra checks the designer ran (all offline, read-only):

- **API compile check [V].** Scratch crate `work\design_scratch\apicheck` (gitignored, path deps on the vendored crates). It passes `cargo check` and uses:
  - `SoloParamRepository::get::<AtkParam_Pc>(id).atk_phys()` and `get::<Bullet>(id).hit_bullet_id()/atk_id_bullet()/init_vellocity()`
  - `CSCamera.pers_cam_1.fov` and `CSCamExt::forward()/position()`
  - `CSBulletIns.fly_state.base.param.param_id`, `.targeting_owner.owner_chr_handle`, `.physics.velocity`
  - `WorldChrMan.chr_inses_by_distance` with `last_hit_by` and `hp`
  - `CSMenuManImp.ui_states[i].visible()` and `CSMouseMan.show_cursor`
  - `size_of::<FieldInsHandle>() == 8`
- **Param lookup trap [V].** `FD4ParamRepository::get::<ATK_PARAM_ST>` returns the *first* table whose struct is `ATK_PARAM_ST`, and that could be AtkParam_Npc (fd4/param_repository.rs `get_rescap` matches by struct name). Use `SoloParamRepository::get::<AtkParam_Pc>` / `get::<Bullet>`, which select by table index (solo_param_repository.rs:538-541).
- **Vanilla values [V].** Decoded from the vanilla 1.17.1 regulation (`work\regulation\unpacked`):
  - Bullet: 15,475 rows; 53 rows have `atkId_Bullet = -1` and 5,165 have `sfxId_Bullet = -1`; the fastest vanilla bullets use `initVellocity = 999`.
  - XML defaults: `homingAngle` 0 and `lockShootLimitAng` 0.
  - Throwing Dagger Bullet 10170000 extra fields: `isOwnerOverrideInitAngle`, `FollowType`, `EmittePosType`, `accelInRange` are all 0.
  - NpcParam 43110000 "Godrick Soldier": hp 192, def_phys 100, thrustDamageCutRate 1.1.
  - SpEffect 7010 "Area Scaling - Tier 02 (Limgrave)": maxHpRate 1.141, physicsDiffenceRate 1.013.
- **WEAPONFILE format [V].** The three dumps have 671 key/value pairs each, separated by `\`. Some values contain newlines (11-18 per file). The file ends with `...\0`.
- **UMP45 extra fields [V].** hipSpreadDecayRate 4, hipSpreadFireAdd 0.52, hipSpreadMoveAdd 4, reticleSideSize 8, lowAmmoWarningThreshold 0.33, reloadAddTime 1.8, startAmmo 96, locHead 1.4.

---

## 1. Files M2 creates

```
D:\Mods\MW2inER\
  mw2er.toml                       config (committed; no MW2 data)
  launch.ps1                       user-run launcher (the agent never runs it without -CheckOnly)
  profiles\mw2er.me3               me3 profile
  mod\                             Rust workspace
    Cargo.toml  Cargo.lock  .cargo\config.toml
    mw2er-core\                    pure logic (rlib), unit tests
    mw2er\                         the cdylib -> mw2er.dll
  tools\patches\fromsoftware-rs-0001-bullet-spawn-data.patch
  tools\scripts\convert_weapons.py   WEAPONFILE -> converted\weapons.json
  tools\scripts\copy_sounds.py       dumps -> converted\sounds\... + sounds.json
  tools\scripts\build_regulation.py  vanilla copy -> rows -> build\package\regulation.bin + build\rows.json
  tools\scripts\m2_static_checks.py  profile / ids / gitignore / package / PE checks
  tools\scripts\analyze_log.py       reads logs\mw2er.log after a user run, prints in-game PASS/FAIL
  tools\scripts\check_m2.ps1         runs every offline oracle, prints PASS/FAIL/SKIP
  docs\M2_DESIGN.md                this file
Generated (gitignored): converted\, build\natives\, build\package\, build\rows.json, logs\, work\, mod\target\
```

The existing `tools\scripts\{conpty_run,param_diff,param_rows,check_assets}.py` are reused.

**Python `-I` gotcha [V].** Scripts run with `python -I`, which drops the script's own folder from `sys.path`. Any script that imports a sibling must insert its own folder first, as `param_rows.py` already does.

---

## 2. Workspace and module layout

There are two crates. The boundary keeps game-memory code out of the tested logic [I, decision].

`mod\Cargo.toml`:

```toml
[workspace]
members = ["mw2er-core", "mw2er"]
resolver = "3"
```

### 2.1 `mw2er-core` (rlib): pure logic, no game memory

- `#![forbid(unsafe_code)]`.
- Dependencies: only `serde`, `serde_json`, `toml`. No `windows`, `eldenring`, `hudhook` or `rodio`, so it cannot touch game memory.
- It is unit-tested with `cargo test -p mw2er-core`.

| Module | Contents |
|---|---|
| `units` | `INCH_TO_M = 0.0254`; inch/metre conversion helpers |
| `vec3` | small `V3` math (dot, cross, normalize, len) |
| `weaponfile` | Rust WEAPONFILE parser. Used only by tests to cross-check the Python converter (two implementations must agree) |
| `weapons` | serde model + validation of `converted\weapons.json` (schema §16.2) |
| `sounds` | serde model of `converted\sounds\sounds.json` |
| `rows` | serde model of `build\rows.json`; `band_row_id(weapon, mw2_damage)` |
| `config` | serde model of `mw2er.toml` with `#[serde(default)]` everywhere; `Config::parse(&str) -> Result` |
| `weapon_sim` | fire cadence + ammo/reload state machine + reload sound cues (§10, §11) |
| `falloff` | MW2 damage at a distance in metres, and the falloff band (§10.5) |
| `spread` | hip-spread scale model, crosshair gap in pixels, random cone direction (§10.4, §13) |
| `aim` | camera forward-sign check, bullet origin, target-distance estimate from plain samples (§10.2) |
| `rng` | xorshift64* (deterministic in tests) |
| `input_logic` | key name → DIK code table; `BlankLatch` (keeps blanking until the key is released); press-edge counter deltas |
| `menu_logic` | menu-open decision from a 70-bit visible mask (u128) + baseline + allowlist; transition list |
| `hit_logic` | `HitTracker`: HP-sample diff → `Hit` / `Kill` / `HpDrop` events (§12) |
| `hud_geom` | crosshair, hitmarker and ammo layout in pixels from a snapshot + display size (§13) |
| `logfmt` | exact log line formatting (`HIT chr=c#### hp a->b dmg=N` etc.) |

### 2.2 `mw2er` (cdylib → `mw2er.dll`): touches the process

- `#![deny(unsafe_op_in_unsafe_fn)]`. Every `unsafe` block has a `// SAFETY:` comment.

| Module | Touches game memory? | Contents |
|---|---|---|
| `lib.rs` | no | `DllMain` only (§5.1) |
| `guard.rs` | no | `guard(name, f)` = `catch_unwind(AssertUnwindSafe(f))` + log + subsystem disable flag |
| `paths.rs` | no (own module only) | `GetModuleHandleExW(FROM_ADDRESS)` + `GetModuleFileNameW`; project root = `<dll_dir>\..\..` (§15.1) |
| `log.rs` | no | file logger, `tracing` bridge for hudhook, panic hook (§15) |
| `version_gate.rs` | **reads exe image** (PE resources) | §6 |
| `boot.rs` | indirectly | boot thread sequence (§5) |
| `singletons.rs` | **yes** (reflection scan of the exe's .data) | window wait + fresh `from_singleton::find` scan (§7) |
| `task.rs` | **yes** | `CSTask` recurring task: the per-tick orchestration (§8) |
| `game\player.rs`, `camera.rs`, `chars.rs`, `menu.rs`, `bullets.rs`, `params.rs`, `havok.rs` | **yes** | the ONLY places that read engine structs. Each returns plain-data samples (`PlayerSample`, `CameraSample`, `ChrSample`, `BulletSample`, `MenuSample`), never references |
| `spawn.rs` | **yes** (calls `spawn_bullet`) | builds `BulletSpawnData` via the vendored `new()` |
| `input_hook.rs` | **yes** (code hook in dinput8; edits the game's input buffer) | §9 |
| `hud.rs` | no (reads the snapshot only) | `ImguiRenderLoop` impl (§13) |
| `audio.rs` | no | rodio thread (§14) |
| `shared.rs` | no | atomics, `Mutex<HudSnapshot>`, `Mutex<Arc<Config>>`, monotonic clock |
| `config_watch.rs` | no | re-reads `mw2er.toml` every 1 s (hot reload) |

Rule [I, decision]: anything that can be expressed on plain numbers lives in `mw2er-core` and is tested there. The `game\*` readers stay thin: null-check, copy, return.

---

## 3. Dependencies (exact)

### 3.1 Versions

`mod\Cargo.toml` `[workspace.dependencies]`. Paths are relative to `mod\`. `Cargo.lock` is committed.

| Crate | Spec | Why / evidence |
|---|---|---|
| `eldenring` | `{ path = "../vendor/fromsoftware-rs/crates/eldenring" }` | vendored git rev `59fbd3b3b7daaf14aca47c9f73530493dba6bc79` + patch 0001 (§3.2). Ww2710 table matches this exe 107/107 [V]. Never crates.io `eldenring` 0.14.0, which panics on 2.7.1.0 [R] |
| `fromsoftware-shared` | `{ path = "../vendor/fromsoftware-rs/crates/shared" }` | FromStatic, run_recurring, GameVersion [V compile] |
| `from-singleton` | `"=3.0.1"` | the fresh reflection scan (`find::fd4_singletons`); same instance fromsoftware-shared uses [V cargo tree] |
| `pelite` | `"=0.10.0"` | in-memory version resource for the gate [V compile] |
| `hudhook` | `{ version = "=0.9.3", default-features = false, features = ["dx12"] }` | DX12 ImGui + MinHook (`hudhook::mh`) [V probe build]; includes the #258 swapchain-queue fix [V source] |
| `windows` | `{ version = "=0.62.2", features = [...] }` | the same version as hudhook/cpal, so features unify and types match hudhook's. Features: `Win32_Foundation`, `Win32_System_LibraryLoader`, `Win32_System_SystemInformation` (GetLocalTime), `Win32_System_Threading`, `Win32_UI_WindowsAndMessaging` (EnumWindows, GetForegroundWindow), `Win32_Devices_HumanInterfaceDevice` (DirectInput8Create). Our code never names windows 0.61 types; those stay inside `eldenring` [V: hudhook 0.9.3 → windows 0.62.2; eldenring → 0.61.3] |
| `rodio` | `{ version = "=0.23.0", default-features = false, features = ["playback", "hound"] }` | WAV via hound (Apache/MIT), not symphonia (MPL); cpal 0.18.2 [V probe build] |
| `tracing` | `"=0.1.44"` | hudhook logs through it [V lockfile] |
| `tracing-subscriber` | `{ version = "=0.3.23", default-features = false, features = ["fmt", "std", "env-filter"] }` | `fmt` is not enabled by hudhook, so we enable it [V] |
| `serde` / `serde_json` / `toml` | `"1"` / `"1"` / `"1"` (resolved versions pinned by Cargo.lock) | config/data. toml 1.1.8 is already in the cargo cache [V] |

Explicitly NOT used [V reasons in findings]:
- fromsoftware-rs `debug` crate / `tools/debug-eldenring`: they pull hudhook from git with no rev, and that would be a second MinHook.
- `ilhook`: second hook engine. We use `hudhook::mh`.
- `minhook-sys`: symbol clash with hudhook's MinHook.
- crates.io `eldenring`.
- `chrono` / `time`: local time comes from `GetLocalTime`; monotonic time from `std::time::Instant` (QPC-backed on Windows).

### 3.2 Vendored patches 0001 and 0002 (the only changes to the vendored code)

Patch 0002 (`tools\patches\fromsoftware-rs-0002-recurring-task-no-panic.patch`, added after review,
2026-10-14): `crates/shared/src/task.rs` `RecurringTask` vtable entries `get_runtime_class` and `destructor`
were `extern "C"` with `unimplemented!()` bodies. A panic cannot unwind out of an `extern "C"` fn, so a call
from the engine would abort the game. They now return 0 / do nothing [I: whether ER 2.7.1.0 ever calls
them is unknown; the CS2 precedent ran with the panicking versions]. `check_m2.ps1` check 2 verifies that
the vendor working tree differs from HEAD by exactly patch 0001 + patch 0002 (byte-for-byte).

Patch 0001:

**Reason [V].**
- `BulletSpawnData` (crates/eldenring/src/cs/bullet_manager.rs:170-200) has only private fields and no constructor. Setting fields from outside the crate fails with E0616, so `CSBulletManager::spawn_bullet` cannot be used.
- The empty `FieldInsHandle` cannot be built outside `cs/field_ins.rs`, because `FieldInsSelector`'s `u32` is private (bitfield macro).

**Files and hunks** (exactly two files):
1. `crates/eldenring/src/cs/field_ins.rs`: add
   `impl FieldInsHandle { pub const fn none() -> Self { Self { selector: FieldInsSelector(u32::MAX), block_id: BlockId::none() } } }`.
   - Test: `none().is_empty()`.
   - Test: its 8 bytes read as u64 equal `u64::MAX` (BlockId::none() = -1 [V block_id.rs:32]).
2. `crates/eldenring/src/cs/bullet_manager.rs`:
   - Make these fields `pub`: `owner, behavior_id, magic_id, bullet_id, goods_id, dummy_poly_id, target, acceleration_angle, angle, position`.
   - Add the constructor:
     ```rust
     impl BulletSpawnData {
         pub fn new(owner: FieldInsHandle, bullet_id: i32, position: F32Vector4, angle: F32Vector4) -> Self {
             let mut d: Self = unsafe { core::mem::zeroed() };
             d.owner = owner; d.bullet_id = bullet_id;
             d.behavior_id = -1; d.magic_id = -1; d.goods_id = -1; d.dummy_poly_id = -1;
             d.target = FieldInsHandle::none();
             d.position = position; d.angle = angle;
             d
         }
     }
     ```
   - Add `assert_eq!(0x110, size_of::<BulletSpawnData>())` to its test module.
   - The -1 defaults and zeroed `unk*` are [I]. The `angle` doc comment says "Forward vector" [V source].

**How it is applied [I, decision].**
- The vendored checkout stays at HEAD `59fbd3b` (no commit). The patch is a working-tree change.
- Its canonical copy is `tools\patches\fromsoftware-rs-0001-bullet-spawn-data.patch`.
- `check_m2.ps1` verifies all three of:
  - `git -C vendor\fromsoftware-rs rev-parse HEAD` = 59fbd3b…
  - `git diff --stat` lists exactly those two files.
  - `git apply --check --reverse <patch>` succeeds, i.e. the patch is applied and nothing else is.

### 3.3 Build configuration

- `mod\.cargo\config.toml`: `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]`.
  - This drops the VCRUNTIME140 / api-ms-win-crt imports [V probe dumpbin].
- `[profile.release]`: `panic = "unwind"` (never "abort"), `debug = 1` (line tables for backtraces), `incremental = false`.
  - The vendor workspace's `panic = "abort"` does not apply to path deps [V].
- The target dir is `mod\target` (short path; long paths broke link.exe with LNK1104 [V]).

---

## 4. Threads and shared state

| Thread | Created by | Runs |
|---|---|---|
| loader thread | Windows | `DllMain`: spawns `mw2er-boot`, returns 1 |
| `mw2er-boot` | us | boot sequence (§5), then becomes the config watcher + STATE logger (1 s loop, forever) |
| game main thread | ER | our `CSTask` recurring closure (§8): all gameplay |
| present thread | ER | hudhook `ImguiRenderLoop` methods (§13): read-only snapshot |
| input thread(s) | ER | `GetDeviceState` detour (§9): atomics only |
| `mw2er-audio` | us | rodio mixer (§14) |

Shared state (`shared.rs`). There is no other cross-thread state [I, decision].

- **Atomics:**
  - `MODE_ON`
  - `WANT_BLANK_FIRE`, `WANT_BLANK_RELOAD` (written by the task)
  - `RAW_FIRE_DOWN`, `RAW_RELOAD_DOWN`, `RAW_TOGGLE_DOWN` (written by the hook, pre-blank)
  - `PRESSES_TOGGLE`, `PRESSES_RELOAD` (u32 edge counters, hook → task)
  - `INPUT_HOOK_OK`, `HUD_OK`, `GAMEPLAY_DISABLED`, `HUD_DISABLED`
  - `LAST_GDS_MOUSE_MS`, `LAST_GDS_KBD_MS` (ms since boot; the mouse stamp also gates firing: `RAW_FIRE_DOWN`
    counts only while the game polled the mouse in the last 150 ms)
  - `TASK_HEARTBEAT_MS` (written every task tick; the detour applies `WANT_BLANK_*` only while it is at most
    250 ms old, so blanking ends by itself if the task stops ticking, e.g. on the title screen)
- `Mutex<HudSnapshot>`: the task writes it once per tick; render uses `try_lock` and keeps its last copy on contention (never blocks Present).
- `Mutex<Arc<Config>>`: config watcher swaps it; readers clone the Arc.
- `OnceLock<SyncSender<Sfx>>` (capacity 64): the task calls `try_send` (never blocks; drops if full).
- `OnceLock<Instant>` clock base: `now_s() -> f64` on every thread.

---

## 5. Boot sequence

### 5.1 DllMain [V pattern compiled in hudhook-probe; R Sunless/CS2]

```rust
#[no_mangle] pub unsafe extern "system" fn DllMain(h: HINSTANCE, reason: u32, _: *mut c_void) -> i32
```

- On `DLL_PROCESS_ATTACH`, and only once (`BOOT_STARTED.swap`), it stores `h.0 as usize` and calls `std::thread::Builder::new().name("mw2er-boot").spawn(..)`.
  - The spawn's Result is ignored.
  - The body is wrapped in `catch_unwind`.
- Returns 1. Nothing else happens under the loader lock: no singletons, no hooks, no file I/O, no panics.
- No `hudhook!` macro: its DllMain returns `()` and calls `eject()` on errors [V source].
- Nothing is done on DETACH: never join threads or unhook (loader-lock deadlock; `eject()` would also remove our DI hook).

### 5.2 Boot thread steps

The whole body runs inside `guard("boot", ..)`.

| # | Step | On failure |
|---|---|---|
| 1 | `paths::resolve(hmodule)` → dll path, root = `<dll_dir>\..\..`; root is valid if `root\mw2er.toml` exists | no valid root: log to `<dll_dir>\mw2er.log`, use config defaults. No absolute DLL path at all: `OutputDebugString` one line and stay inert (a relative log path would land in the process CWD, likely the game folder) |
| 2 | open log (append, flush per line), install panic hook, init tracing bridge; log `BOOT` (dll path, size, mtime, crate version, build timestamp from build.rs, pid). The project is not a git repo, so there is no git hash | if the log can't open: continue silently (never fatal) |
| 3 | **version gate** (§6) | log `VERSION FAIL …; staying inert` and **return**: no hooks, no threads, nothing else |
| 4 | load config (`CONFIG` line) and data (`weapons.json`, `sounds.json`, `rows.json`; `DATA` lines). Cross-check that `rows.json.k == config.damage.k` | parse error → defaults; data error → `DATA FAIL`, MW2 mode will refuse to enable (logged + HUD toast) |
| 5 | **apply hudhook DX12 now, before the window wait** (§13.1) | `HUD FAIL …`: no HUD; gameplay still allowed |
| 6 | **install the DirectInput hook** (§9) on hudhook's MinHook | `INPUT FAIL …`: MW2 mode will refuse to enable (we could not stop the game attacking) |
| 7 | start the audio thread (§14) | `AUDIO WARN …`: silent |
| 8 | **wait for the game window** (§7.1) | keeps waiting; `BOOTGATE wait` every 10 s |
| 9 | **fresh reflection scan** until all required names are present (§7.2) | keeps waiting; lists missing names every 10 s |
| 10 | `CSTaskImp::wait_for_instance(60 s)` → `run_recurring(closure, CSTaskGroupIndex::ChrIns_PostPhysics)`; `std::mem::forget` the handle | `TASK FAIL <err>`: gameplay off, HUD shows the error toast |
| 11 | log `BOOT DONE in N ms`, set `snapshot.booted`; the HUD shows "mw2er ready - F5: MW2 mode" for 5 s | — |
| 12 | loop forever every 1 s: config hot-reload, `STATE` line every `log.state_interval_s` while in world, log flush | errors logged, loop continues |

Why hudhook comes before the window wait [V source, I timing]:
- hudhook 0.9.3 captures ER's command queue most reliably from the game's own `CreateSwapChain*` call (#258). That only works if the hook exists before the swapchain is created.
- The window/reflection gate protects fromsoftware-rs, not hudhook.
- If the log shows repeated `Couldn't find command queue pointer` warnings, set `load_early = true` on the me3 native (§17) and retest.

Why the DI hook comes after hudhook [V source]:
- `Hudhook::builder()` runs `MH_Initialize`.
- If hudhook failed before that, `input_hook` calls `hudhook::mh::MH_Initialize()` itself and accepts `MH_ERROR_ALREADY_INITIALIZED`.

---

## 6. Version gate [V]

It runs on the boot thread before ANY fromsoftware-rs call. On an unknown exe, `rva::get()` panics inside a LazyLock and poisons it; every later address-based call then panics, including `as_subclass` [V source].

The decision for checks 1-2 (plus ProductVersion 2.7.1.0 and an English version resource, which check 3
also requires) is the pure function `mw2er_core::gate::decide`, unit-tested offline (2.7.1.0 passes;
2.7.0.0, 2.7.1.1, another exe and a Japanese resource fail). `m2_static_checks.py bootorder` (check_m2
check 21, with a self-test that mutates boot.rs/lib.rs) verifies that `boot()` calls the gate before data,
hudhook, the input hook, audio, the boot gate and task registration, that its Err arm returns, and that
DllMain only spawns the boot thread.

All three checks must pass:
1. `GetModuleFileNameW(None)` basename equals `eldenring.exe` (case-insensitive).
2. From the **loaded image** (`pelite::pe64::PeView::module(GetModuleHandleW(None))`): `resources()?.version_info()?.fixed()?.dwFileVersion` == 2.7.1.0.
   - Compare the integer components (Major, Minor, Patch, Build), never strings.
3. `catch_unwind(|| Er2710::detect(&view).is_ok())`, where:
   - `Er2710: fromsoftware_shared::game_version::GameVersion` has `NAME = "elden ring"`.
   - `from_lang_version` accepts `(LANG_ID_EN, "2.7.1.0")`.
   - This is exactly what the crate's Ww2710 table needs. `detect` unwraps internally, hence the catch_unwind.

Log lines:
- Pass: `VERSION PASS file=2.7.1.0 product=2.7.1.0 lang=0x0409 timedatestamp=0x… sizeofimage=0x…`.
- Fail: `VERSION FAIL found a.b.c.d (want 2.7.1.0); staying inert`, then return.

On this PC: FileVersion = ProductVersion = 2.7.1.0, English (US), and me3 attached to "ELDEN RING™ 1.17.1.0 Worldwide" with no SteamStub [V].

---

## 7. Boot gate [R CS2 gotchas 1-2, V source]

### 7.1 Window

Poll every 250 ms:
- `EnumWindows` for a visible top-level window whose `GetWindowThreadProcessId` equals our PID and whose class is not one of hudhook's dummy windows.
  - Our own hudhook dummy window is created and destroyed during apply. Wait for a window that exists on two consecutive polls [I].
- Store the HWND (for the foreground check).
- Log `BOOTGATE window hwnd=0x… after N ms`.

### 7.2 Fresh reflection scan

Required names, taken from `<T as FromStatic>::name()` at runtime, never typed by hand. The reflection names below are [V source]:

| Rust type | Reflection name |
|---|---|
| `CSTaskImp` | `CSTask` |
| `WorldChrMan` | `WorldChrMan` |
| `CSBulletManager` | `CSBulletManager` |
| `CSCamera` | `CSCamera` |
| `CSMenuManImp` | `CSMenuMan` |
| `CSHavokMan` | `CSHavokMan` |
| `SoloParamRepository` | `SoloParamRepository` |

Optional, logged only: `CSMouseMan`, `FD4PadManager`.

Scan loop:
- Every 250 ms (1 s after the first 60 s): `let p = from_singleton::find::fd4_singletons(PeView::module(base));`.
- If `!p.all_null()`, then `m = p.finish()`, unioned with `find::derived_singletons`.
- Pass when `m` contains every required name.

These calls build a new map each time and never touch from-singleton's cached `map()`. The cache is first filled by our first `instance()` call, which happens after the gate, when every name resolves [V from-singleton src lib.rs:42-62].

No timeout: the user may sit at the title screen. Log:
- `BOOTGATE PASS after N ms names=[…]`.
- `BOOTGATE waiting missing=[…]` every 10 s.

Hard rule: no `FromStatic::instance*()` and no `wait_for_instance` before PASS. Never from `DllMain`, the hook detour or any `ImguiRenderLoop` method, ever.

---

## 8. The game task (one tick)

**Group:** `CSTaskGroupIndex::ChrIns_PostPhysics` [V exists, used by the debug-line example].
- [I] At this point, the physics positions of this frame are final and `CameraStep` (index 382 < 389) has already run, so camera and player come from the same frame.
- Input blanking does not depend on the group: it happens in the DI detour.

The body is `guard("task", tick)`. If it panics:
- set `GAMEPLAY_DISABLED`;
- clear `MODE_ON`, `WANT_BLANK_*` first, so the game gets LMB/R back;
- log `PANIC …` + `ERROR task disabled`;
- every later tick returns immediately.

Per tick, in order:
1. `now = now_s()`, `dt = now - last` (clamped to [0, 0.25]).
2. Read `PlayerSample` (game\player.rs). `None` = not in world (loading, title, main_player None, modules null). Every pointer is read as an integer and null-checked first. Fields:
   - handle (u64)
   - position (x, y, z, w) and chest = position + (0, `aim.chest_height_m`, 0)
   - hp, max_hp
   - planar speed from the position delta: deltas > 5 m count as speed 0 (map re-basing [R CS2])
3. Read `CameraSample` (`gameplay.camera_source`, default `pers_cam_1`; `chr_cam` is the alternative):
   - row2 (forward candidate), position, fov raw
   - the first 3 in-world ticks log both cameras.
4. Read `MenuSample`: `ui_states` visible mask (u128 of 70 bits), `show_cursor`. Then `menu_logic` → `menu_open`; log `MENU idx=… 0->1` transitions (always, even with mode off).
5. `foreground = GetForegroundWindow() == game_hwnd`.
6. Consume `PRESSES_TOGGLE` delta → toggle MODE:
   - Allowed only if in world and no menu is open; otherwise `MODE toggle ignored (reason)`.
   - Refused with a reason if data, input hook or params are not OK.
   - On enable: capture the menu baseline; if `refill_on_mode_enable`, set mag = clipSize and reserve = maxAmmo.
   - Log `MODE on|off`.
7. Compute and store `WANT_BLANK_FIRE = WANT_BLANK_RELOAD = MODE_ON && in_world && !menu_open && foreground && !GAMEPLAY_DISABLED`.
8. Spread scale update (`spread::tick`, with move-add from planar speed).
9. `weapon_sim.step(now, trigger = RAW_FIRE_DOWN && can_fire, reload_presses, …)` → events:
   - `Shot` ×n
   - `ReloadStart{empty}`, `ReloadDone{added}`, `ReloadCancelled`
   - `DryFire`
   - `Sfx(id)`

   `can_fire` = blanking conditions && params OK && spawn breaker closed.
10. For each `Shot`: run the firing pipeline (§10). Audio `Sfx::Fire`.
11. Bullet poll (§12.2): new marker / first-seen band bullets → `IMPACT` / `BULLET` lines.
12. HP poll (§12.1) → `HIT` / `KILL` / `HPDROP` lines, hitmarker time.
13. First in-world tick only (repeated after each load screen): **PARAMS check** (§16.5) → `PARAMS OK n=7` or `PARAMS FAIL …` (firing disabled).
14. Publish `HudSnapshot`.

---

## 9. Input hook (DirectInput `GetDeviceState`)

Why GetDeviceState [R CS2, Sunless]: ER reads mouse and keyboard only via DirectInput `GetDeviceState` on this build, and nothing reaches ImGui through raw input.

### 9.1 Install [V compiled in hudhook-probe `install_dinput_hook`]

- `LoadLibraryW("dinput8.dll")`.
- For each of `IDirectInput8W` and `IDirectInput8A` (`DirectInput8Create(GetModuleHandleW(None), DIRECTINPUT_VERSION, iid, …)`):
  - `CreateDevice(GUID_SysKeyboard)` and `CreateDevice(GUID_SysMouse)`.
  - Read vtable slot 9 (`GetDeviceState`).
  - Collect the distinct addresses, at most 4 (keyboard and mouse often share one; the A/W hook-both rule is [R Let It Die]).
- For each address: `hudhook::mh::MhHook::new(addr, detour_k)`, then `MH_EnableHook(addr)`. Store each trampoline in an `AtomicUsize`.
- Release the probe devices and interfaces.
- Log `INPUT PASS hooked=N addrs=[…]`.
- Every Win32 / COM error returns `Err`. No `expect` (the reference `dinput.rs` expects dinput8 to be loaded).

### 9.2 Detour (no allocation, no locks, no logging, cannot panic)

1. Call the trampoline. If `hr` fails or `data` is null, return `hr`.
2. Act on `cbData`:
   - **16 / 20** (`DIMOUSESTATE` / `DIMOUSESTATE2`): `rgbButtons[b]` is at byte `12 + b`, with `b = keys.fire_mouse_button` (default 0 = LMB).
     - Store raw `down = byte & 0x80 != 0` into `RAW_FIRE_DOWN`.
     - If `fire_latch.update(WANT_BLANK_FIRE, down)`, zero the byte.
     - **Never touch lX/lY/lZ**: the camera is the aim.
   - **256** (keyboard, `DIK_*` index):
     - Raw toggle key (`DIK_F5 = 0x3F`) → `RAW_TOGGLE_DOWN`; a 0→1 edge increments `PRESSES_TOGGLE`.
     - Raw reload key (`DIK_R = 0x13`) → `RAW_RELOAD_DOWN`; an edge increments `PRESSES_RELOAD`.
     - Blank R via `reload_latch.update(WANT_BLANK_RELOAD, down)`.
     - Blank F5 always if `keys.blank_toggle_key`. R is bound to an item action in ER by default [I]; F5 is believed unbound [I].
   - **Any other size** (gamepads etc.): untouched. Zeroed pad axes read as full deflection [R Let It Die].
3. `BlankLatch` (core): `blank = want || (was_blanking && down)`. A button held when blanking ends stays blanked until released, so an ER attack does not leak after F5-off or when a menu opens [R Sunless gotcha 18].
   - `want` is `WANT_BLANK_*` only while `TASK_HEARTBEAT_MS` is at most 250 ms old (`input_logic::effective_want`): if the task stops ticking (title screen or loading, unverified whether `ChrIns_PostPhysics` runs there), LMB and R go back to the game without needing F5.
4. The whole body is inside `catch_unwind`; on Err it returns the trampoline result untouched. This is defensive, since the body has no panicking calls.

Keys are configurable (`keys.toggle`, `keys.reload`) through a fixed name→DIK table in `input_logic`: F1-F12, A-Z, 0-9. Unknown names → `CONFIG ERROR`, and the default is kept.

---

## 10. Firing pipeline

### 10.1 Cadence (`weapon_sim`, real time) [I]

Time is `now_s()` (QPC). Never frames: DSR's `Sleep(4)` was ~15.6 ms [R].

- **Full Auto:**
  - While the trigger is held and allowed: if `next_shot < now - fireTime`, set `next_shot = now` (no catch-up burst after a pause).
  - Then `while now >= next_shot && mag > 0 && shots_this_tick < gameplay.max_shots_per_tick (2)`: emit `Shot`, `mag -= 1`, `next_shot += fireTime`.
  - Releasing the trigger keeps `next_shot`, so tapping cannot beat the cadence.
- **Single Shot** (for later guns): fires only on the trigger's 0→1 edge, with the same `next_shot` rule.
- UMP45: `fireTime = 0.09 s` → 666.7 RPM. At 60 fps that is about 1 shot every 5.4 frames; never 2 per tick except after a hitch [V values].

### 10.2 Aim [I, every value logged on the first shots]

**Forward vector.** `row2` of the chosen camera matrix (`CSCamExt::forward`).

**Sign check.** The third-person camera looks toward the player, so `sign = sign(dot(row2, chest - cam_pos))`.
- Recompute every tick; keep the last sign while `|dot| < 0.2 m`.
- `fwd = sign * normalize(row2)`.
- Log `CAMERA sign=±1 cam_player_dist=… m fov_raw=…` when it first resolves and whenever it flips.
- Expected cam-player distance: 2-6 m. Outside 0.3-15 m logs `CAMERA odd`, a hint that the coordinate spaces differ.

**Origin.** On the crosshair ray, starting just in front of the player's depth:
- `t0 = max(dot(chest - cam_pos, fwd), 0.5)`
- `origin = cam_pos + fwd * (t0 + aim.origin_ahead_m (0.6))`

This gives no parallax between crosshair and bullet. It also keeps the spawn outside the player's capsule even if the engine did not exclude the owner.

**Basis.** `right = normalize(cross(fwd, Y))`, `up = cross(right, fwd)`. World up is +Y (ER is metres, Y-up [V plan]).

**Direction.** `dir = spread::perturb(fwd, right, up, spread_deg, rng)` if `gameplay.apply_spread_to_bullets`, else `fwd`.

**Sanity check before any engine call** (added after review; `aim::check_shot`, unit-tested). The shot is
skipped with `SHOT n=… skipped: <reason>` (ammo is still spent, like the other skip reasons) unless:
- the owner handle is not empty and its selector type is `Chr` (logged once as `owner_check=` on "world entered");
- camera position, chest, origin and direction are finite; the ray range is positive and finite;
- the camera-to-chest distance is inside 0.3-15 m (the old `CAMERA odd` window, now enforced per shot);
- the origin is at most `origin_ahead_m + 3 m` from the chest; the direction is a unit vector.
No shot is fired in the first 30 ticks after "world entered".
`[aim]` config values must be finite with upper bounds (max_range_m ≤ 1000, origin_ahead_m ≤ 5,
chest_height_m ≤ 3, char_radius_m ≤ 5), so a hot reload with `inf`/`nan` is rejected.

### 10.3 Bullet spawn parameters [V API; I values]

```text
BulletSpawnData::new(
  owner    = player.field_ins_handle,              // team rules + last_hit_by attribution [R CS2]
  bullet_id= rows.json band id (§10.5),
  position = F32Vector4(origin.x, origin.y, origin.z, player.position.w),  // w copied from HavokPosition [I]
  angle    = F32Vector4(dir.x, dir.y, dir.z, 0.0)) // "Forward vector" [V doc comment]
target = FieldInsHandle::none()  // no lock-on homing/aim override
behavior/magic/goods/dummy_poly = -1, unk* = 0      // [I]
CSBulletManager::instance_mut()?.spawn_bullet(&d)  // main thread only
```

**Errors.**
- `Err(code)` logs `SPAWN FAIL code=… (3=invalid bullet id, 4=construction failed)`.
- 3 consecutive failures open the spawn breaker: firing is disabled and `ERROR spawn breaker open` is logged.
- `gameplay.spawn_bullets = false` turns spawning off, for triage, while everything else keeps running.

**First shot of each session.** A `SPAWN try …` line is written and flushed *before* the call. If the game crashes, the last log line points to the spawn.

**Coordinate spaces.** Player physics position, camera position (`HavokPosition` per the crate) and bullet position are all treated as Havok space [I]. The `BULLET` line (§12.2) shows the spawned bullet's position next to the origin we passed, which verifies this.

### 10.4 Spread model (hip only; ADS is M4) [I: IW-engine behaviour reconstructed, not verified]

- State `s` in [0,1].
- On each shot: `s = min(1, s + hipSpreadFireAdd)` (UMP45 0.52).
- Each tick:
  - if planar speed > 0.5 m/s and `gameplay.use_move_spread`: `s += hipSpreadMoveAdd * clamp(speed/6.0, 0, 1) * dt`;
  - else `s -= hipSpreadDecayRate * dt` (UMP45 4/s).
- Clamp to [0,1].
- `spread_deg = hipSpreadStandMin + (hipSpreadMax - hipSpreadStandMin) * s` → UMP45 2°…5°.
  - Sustained fire oscillates s between about 0.64 and 1.0 (4.3°-5°).
  - Full recovery takes 0.25 s.
- `perturb`:
  - `theta = 2π·u1`, `r = u2` (IW style, centre-biased; not uniform by area);
  - `offset = tan(spread_rad) · r`;
  - `dir = normalize(fwd + right·offset·cos θ + up·offset·sin θ)`.
  - So the deviation is ≤ spread_deg: bullets stay inside the drawn crosshair.

### 10.5 Damage falloff and band choice [V field values; I formula]

**Falloff in metres** (`falloff::mw2_damage_at(d_m)`):
- UMP45: `maxDamageRange 600 in = 15.24 m`, `minDamageRange 850 in = 21.59 m` (×0.0254).
- `d ≤ 15.24` → 40; `d ≥ 21.59` → 35.
- In between: linear `40 + (35-40)·(d-15.24)/(21.59-15.24)`, rounded to the nearest integer (`f32::round`).
- Linear interpolation and the rounding are [I]: IW's exact rounding is unknown.

**Bands.** One regulation row per integer MW2 damage from `minDamage` to `damage` (UMP45: 40, 39, 38, 37, 36, 35 → 6 rows).
- If `damage - minDamage > 15`, use an even step so there are at most 16 bands (ACR 30→20 = 11 bands).
- `band = damage - band_damage`; row id = `79000000 + slot·100 + band` (§16.1).

**Distance estimate at spawn** (`aim::estimate_target_distance`):
- `t_chr`: over characters within `aim.max_range_m` (150), excluding the player, excluded ids and dead ones.
  - Chest point P; `t = dot(P - origin, dir) > 0`; perpendicular distance < `aim.char_radius_m` (0.75); take the minimum `t`.
- `t_map`: `CSHavokMan.phys_world.cast_ray(0x2000058, origin, dir·max_range, player)`. The filter value is [R CS2]; the API is [V compile in findings].
- `d_est = min(t_chr, t_map)`, else `max_range` (which gives the 35 band).
- Logged per shot as `d=…m src=chr|map|none`.

**Why fixed rows per band, not per-shot param edits** [I from R DSR/Sunless]:
- At 667 RPM several bullets are in flight; a shared row edited per shot would give some of them the wrong damage.
- Fixed rows are deterministic and round-trip-checkable offline.

### 10.6 Damage mapping and the default k

**Mapping.** `AtkParam_Pc.atkPhys(row) = round(mw2_band_damage × k)`.
- `k = damage.k` in `mw2er.toml`, default **4.0**.
- `build_regulation.py` bakes it into the rows (UMP45: 160, 156, 152, 148, 144, 140).
- At runtime the DLL only verifies it: the PARAMS check compares the live `atk_phys()` with `rows.json`. If `config.damage.k` ≠ `rows.json.k`, it logs `CONFIG WARN k changed; rerun build_regulation.py`.
- k is therefore not hot-reloadable. That is deliberate: no live param writes in M2.

**Why 4.0.**
- The goal is MW2's time-to-kill in hits [I]. MW2 UMP45 kills a 100-HP player in 3 hits at any range (40·3, 35·3 ≥ 100).
- Reference target: a Limgrave Godrick Soldier [V vanilla params]:
  - NpcParam 43110000: hp 192 × maxHpRate 1.141 = **219 HP**;
  - def_phys 100 × 1.013 = **101.3**;
  - thrustDamageCutRate 1.1 (our row's atkAttribute is 2, cloned from the Throwing Dagger; reading 2 as pierce is [I]).
- ER's damage-vs-defence curve (community formula [R/I], not verified here):
  - `ratio = atk/def`;
  - for 1 ≤ ratio < 2.5: `dmg = atk·(0.7 - (0.4/3)(2.5-ratio)²)`.
- With k = 4: atk 160 → **≈103 HP** per hit; atk 140 → **≈82 HP**. That is 3 hits at both ends of the falloff, like MW2.
  - k = 3.5 → 82 / 64 (3-4 hits).
  - k = 4.5 → 125 / 101 (2-3 hits).
- Unknown multiplier: the cloned row keeps `atkPhysCorrection = 120`. If that scales the bullet's damage by 1.2, k = 4 gives ≈137 / 112 (2 hits).

**First-run calibration.**
- Single-tap a Limgrave Godrick Soldier from under 15 m.
- Read `HIT … dmg=N band=79000100`:
  - ≈103 → keep k;
  - ≈137 → the ×1.2 applies; set k = 3.0 (≈86 / 68).
- Rebuild the regulation (offline), relaunch.
- A single global k makes late-game enemies spongier (Leyndell tier ≈ 8 hits); that is accepted for M2.

### 10.7 Not in M2

ADS, recoil/view kick, location multipliers (`locHead 1.4` is kept in weapons.json), penetration, the viewmodel (M3), tracers.

---

## 11. Ammo and reload state machine (`weapon_sim`) [V values; I behaviour]

**Start.** `mag = clipSize (32)`, `reserve = maxAmmo (192)`, as the task specifies.

| State | Event | Next | Effect |
|---|---|---|---|
| Ready (mag>0) | trigger held, cadence due | Ready | Shot, `mag-1` |
| Ready | mag reaches 0 and reserve>0 and `auto_reload_on_empty` | Reloading{empty=true, until=now+reloadEmptyTime 3.03} | ReloadStart |
| Ready | R pressed, mag<clipSize, reserve>0 | Reloading{empty = (mag==0), until = now + (empty ? 3.03 : 2.5)} | ReloadStart |
| Ready | R pressed, mag==clipSize or reserve==0 | Ready | log `RELOAD ignored (full / no reserve)` |
| Empty (mag==0, reserve==0) | trigger edge | Empty | DryFire (log once per press) |
| Reloading | now ≥ until | Ready | `add = min(clipSize - mag, reserve)`; mag+=add; reserve-=add; ReloadDone |
| Reloading | trigger / R | Reloading | ignored (cannot fire while reloading) |
| any | MODE off | Ready/Empty | ReloadCancelled if reloading (no ammo added) |
| any | MODE on + `refill_on_mode_enable` | Ready | mag=clipSize, reserve=maxAmmo |

**Reload sound cues.** These are fractions of the reload duration, `audio.reload_cues`: lift 0.0, clipout 0.15, clipin 0.55, chamber 0.80. Chamber plays on empty reloads only. The fractions are [I] placeholders until M3 reads the anim notetracks; the WEAPONFILE `notetrackSoundMap` lists lift/clipout/clipin/chamber [V].

**Not modelled in M2.** `reloadAddTime` (1.8 s) and reload cancel/interrupt are kept in weapons.json for M4. Ammo is per session and never saved.

---

## 12. Hit detection

### 12.1 Primary: HP polling (`hit_logic::HitTracker`) [I logic, R fields]

**Sampling.** Each tick:
- Take the first `hit.max_chars` (64) entries of `WorldChrMan.chr_inses_by_distance`.
- Skip null pointers, null `modules` and null `data`.
- Copy `ChrSample { handle: u64, character_id, hp, max_hp, last_hit_by: u64, chest }`.
- Exclude the player's own handle and `hit.exclude_character_ids` (default `[8000]` = Torrent [R Sunless]).
- `distance` in that list is not metres [R CS2]; compute distances from positions.

**Diff against the previous tick's map, keyed by handle.**
- **HIT**: hp decreased AND `last_hit_by == player_handle` AND `now - last_shot_t ≤ hit.window_s (0.6)`.
  - Logged as `HIT chr=c{character_id:04} hp {a}->{b} dmg={a-b} band={last band id} mw2={last mw2 dmg} d={last d_est}m`.
  - `hp ≤ 0` also logs `KILL chr=c####`.
  - Sets `last_hit_t` → hitmarker.
- **HPDROP** (diagnostic): hp decreased inside the window but `last_hit_by` ≠ player.
  - Logged as `HPDROP chr=c#### hp a->b last_hit_by=0x… (not attributed)`.
  - If these appear where HITs were expected, the attribution assumption is wrong.
- New handles are inserted silently; vanished handles are dropped; HP increases update silently.
- `last_hit_by` keeps the last attacker forever [R CS2 gotcha 16]. We only use it together with an HP decrease, never on its own.

**Batching.** Several bullets landing on the same target between two ticks become one HIT with the summed damage. For calibration, single-tap.

### 12.2 Secondary: marker and bullet diagnostics [R CS2; I]

**Marker row.** Every band row has `HitBulletID = 79000900`, an inert marker:
- `atkId_Bullet = -1` (53 vanilla rows use -1 [V]); no sfx/se; life 0.1 s.
- Its spawn is an engine-confirmed impact point.
- It also spawns when the bullet's life ends mid-air [R CS2 gotcha 8].

**Polling.** Each tick, iterate `CSBulletManager::bullets()` and keep the bullets with owner handle == player and `fly_state.base.param.param_id` in our id set:
- New marker handle → `IMPACT pos=(…) dist_from_origin=…m t=…s kind=hit|expired`.
  - `expired` when the distance is within 2 m of `speed × life`.
- First band bullet seen per session → `BULLET id=… pos=(…) origin=(…) speed=|velocity| m/s`.
  - Expected speed ≈ 999.
  - The pos/origin agreement checks the coordinate-space assumption.

These lines are diagnostics. The hitmarker comes from 12.1 only.

---

## 13. HUD (ImGui via hudhook DX12)

### 13.1 Setup [V compiled in hudhook-probe; source]

```rust
Hudhook::builder().with::<ImguiDx12Hooks>(Mw2Hud::new()).with_hmodule(HINSTANCE(raw as _)).build().apply()
```

- Called once, on the boot thread, inside `catch_unwind`.
- Results: `Ok(Ok)` → `HUD applied`; `Ok(Err(MH_STATUS))` → `HUD FAIL status`; `Err(panic)` → `HUD FAIL panic`.
- `hudhook::eject()` is never called.

`initialize()`:
- `io.config_flags |= NO_MOUSE | NO_MOUSE_CURSOR_CHANGE`; remove `NAV_ENABLE_KEYBOARD | NAV_ENABLE_GAMEPAD`; `mouse_draw_cursor = false`.
- Fonts:
  - built-in font at `hud.font_px` (default 26);
  - optional `hud.font_path` TTF (e.g. a Windows system font), read here; on failure fall back to built-in.
  - The file must pass `font_check::check_font` first (≤ 16 MB; sfnt magic 00010000/'true'/'OTTO', not
    'ttcf'/WOFF; in-bounds table directory; cmap/head/hhea/hmtx; glyf+loca or CFF; a Unicode cmap subtable).
    ImGui's IM_ASSERTs are compiled into the DLL and `abort()` the game on a font stb_truetype cannot parse
    [V: the DLL contains "imgui_draw.cpp" and "Assertion failed"]. The check is structural only; corrupt glyph
    data in a well-formed file is not caught.
- Log `HUD initialize display=WxH` (this line proves the pipeline is up).

Other overrides:
- `before_wnd_proc` → `BeforeWndProc::Break`. ImGui processes no messages; this avoids the WM_CHAR `unwrap` panic [V source]. The game still gets every message.
- `message_filter` → `MessageFilter::empty()`.

Every `ImguiRenderLoop` method body runs in `guard("hud.*")`. A panic sets `HUD_DISABLED` and later frames draw nothing.

### 13.2 What render() draws

- Only `ui.get_background_draw_list()`. No `ui.window()`.
- No reads of game memory and no `FromStatic` calls. Input is the last `HudSnapshot`.
- Geometry comes from `mw2er-core::hud_geom`, which is unit-tested.
- `H`/`W` = `io.display_size`.
- `s480 = H/480` (IW virtual screen height [I]); `s1080 = H/1080`.

| Element | When | Geometry |
|---|---|---|
| Crosshair, 4 lines | mode on, in world, no menu, snapshot younger than 0.5 s | `gap = max(hud.min_gap_px, (H/2)·tan(spread_rad)/tan(vfov/2))`; length `L = reticleSideSize(8)·s480·hud.crosshair_line_scale` (18 px at 1080p); thickness `max(1, round(2·s1080))`; colour `hud.crosshair_color` (white, α 0.85), with a 1 px dark outline drawn first |
| Hitmarker X | `now - last_hit_t < hud.hitmarker_s (0.2)` | 4 diagonal segments from radius `8·s1080` to `20·s1080`; alpha fades linearly; a KILL draws it red |
| Ammo `32 / 192` | mode on | bottom-right, margin `40·s1080`; the mag number turns red when `mag ≤ floor(clipSize·lowAmmoWarningThreshold)` (=10) |
| `MW2` badge | mode on | small filled rounded rect above the ammo text |
| Reload indicator | reloading | `RELOADING` centred at `cy + 60·s1080` with a 120·s1080 px progress bar; `NO AMMO` (red) when mag = reserve = 0 |
| Toast | 5 s after boot, or on a subsystem failure | top-left: `mw2er ready - F5: MW2 mode` or `mw2er: <subsystem> failed - see logs\mw2er.log` |

**FOV.** `vfov` comes from `CSCam.fov`, treated as vertical radians [I]:
- 0.2-3.0 → radians; 10-170 → degrees; anything else → `hud.fallback_vfov_deg` (48).
- The raw value is logged once.

**Visibility.** The HUD hides while any ER menu is open, because ImGui draws over ER's menus [V hudhook finding].

**HDR [I risk].** hudhook writes colours raw into HDR swapchains. Test with HDR off first.

---

## 14. Audio thread [V probe type-checked; V cpal COM source]

**Thread.** `audio::start(sounds_json, cfg)` spawns `mw2er-audio` inside `catch_unwind`. Only that thread touches cpal/rodio, because cpal makes the calling thread a COM STA.

On that thread:
1. `DeviceSinkBuilder::from_default_device()` → `.with_error_callback(log)` → `.open_stream()`, then `log_on_drop(false)`.
2. Preload each WAV once: `Decoder::new_wav(BufReader<File>)` → `Vec<f32>` → `rodio::buffer::SamplesBuffer::new(ch, sr, samples)`.
   - The fire buffer is truncated at preload to `audio.fire_max_ms` (600) with a linear fade over the last `audio.fire_fade_ms` (80). This is plain math on the `Vec<f32>`, done once, so each shot is a cheap clone.
   - rodio's `TakeDuration::set_filter_fadeout` is not used: it fades across the whole take, which would soften the attack [V rodio-0.23.0 take.rs:83-86].
3. Loop on `Receiver<Sfx>`:
   - `Fire` → `fire_buf.clone().amplify(master·fire_gain)`;
   - foley ids → amplify `master·foley_gain`;
   - `mixer().add(..)`.
   - The sink stays alive for the process lifetime.

**Why truncate the fire sound [I].** The fire WAV is 2.5-3.0 s with its tail [V MODLOG]. Untruncated, 11 shots/s would stack about 30 voices. Truncated to 600 ms, at most about 7 overlap.

**Sfx ids.** `Fire`, `ReloadLift`, `ReloadClipOut`, `ReloadClipIn`, `ReloadChamber`. Files are resolved from `converted\sounds\sounds.json`.

**Failure handling.** Any failure (no device, missing file, decode error) logs `AUDIO WARN …` once and drops that sound (or all sound). The game side only ever calls `try_send`.

**Teardown.** Never joined or stopped on detach.

---

## 15. Logging

### 15.1 Location

The project root is `<dll_dir>\..\..`. This is correct only because the DLL is **staged** to `D:\Mods\MW2inER\build\natives\mw2er.dll` (`check_m2.ps1` copies it there from `mod\target\release`).
- From `mod\target\release` the same rule would give `mod\`.
- me3 loads the staged copy, which also leaves `cargo build` free to overwrite `target` while the game runs.

Files:
- `root\logs\mw2er.log`, from `config.paths.log_dir` (default `logs`).
- If the root is invalid (no `mw2er.toml` there), fall back to `<dll_dir>\mw2er.log`.
- Size cap 20 MB: at boot, rename to `mw2er.1.log` (one backup).

### 15.2 Format [I, decision]

```
YYYY-MM-DD HH:MM:SS.mmm L [thread] TAG message
2026-10-14 20:01:02.345 I [boot] VERSION PASS file=2.7.1.0 product=2.7.1.0 lang=0x0409 ...
2026-10-14 20:03:10.120 I [task] SHOT n=1 mag=31/192 id=79000100 mw2=40 d=11.8m src=chr spread=2.00 origin=(..) dir=(..)
2026-10-14 20:03:10.152 I [task] HIT chr=c4311 hp 219->116 dmg=103 band=79000100 mw2=40 d=11.8m
```

- Local time from `GetLocalTime`.
- `L` is one of `D` / `I` / `W` / `E`.
- Each line is flushed (survives a crash).
- Writer: one `Mutex<BufWriter<File>>`.

Tags:
- Boot and setup: `BOOT`, `VERSION`, `CONFIG`, `DATA`, `HUD`, `HUDHOOK` (hudhook's tracing events), `INPUT`, `AUDIO`, `BOOTGATE`, `TASK`, `PARAMS`, `CAMERA`
- Gameplay: `MODE`, `MENU`, `SHOT`, `SPAWN`, `BULLET`, `IMPACT`, `HIT`, `KILL`, `HPDROP`, `RELOAD`, `AMMO`
- Status and errors: `STATE` (every 5 s in world), `WARN`, `ERROR`, `PANIC`

The `HIT chr=c#### hp a->b dmg=N` prefix is fixed. Extra fields only go after it.

### 15.3 hudhook / tracing bridge [V probe]

- `tracing_subscriber::fmt()` with our writer and timer, `EnvFilter::new("hudhook=debug")`. Never `trace`: that level logs every frame.
- A rate-limit layer per callsite: the first event is logged, then a count every 10 s. This is for hudhook's per-ExecuteCommandLists warn `Couldn't find command queue pointer`.

### 15.4 Panic hook

Installed on the boot thread, not in DllMain:
- logs `PANIC thread=… at file:line: msg` + `Backtrace::force_capture()`, then flushes;
- the `guard` that catches the unwind then logs `ERROR <subsystem> disabled`.

---

## 16. Regulation rows and pipeline

### 16.1 Id scheme [V free range; I scheme]

- No row in any of the 194 params uses 79,000,000-79,999,999 [V].
- Row id = `79000000 + slot·100 + band`. Slots come from `mw2er.toml [regulation.weapons]` (UMP45 = 1).
- The marker is `79000900` (slot 9 reserved for markers).
- Bullet and AtkParam_Pc rows share ids, and `atkId_Bullet = own id`.
- Rows are inserted in id order. They sit after the last vanilla id below 79000000. The game binary-searches (`find_index`), so order matters [V source].

### 16.2 Rows for M2 (UMP45 only; 13 rows)

All rows are cloned from the Throwing Dagger.
- **Clone sources:** Bullet 10170000, AtkParam_Pc 10170000; Goods 1700 → Bullet 10170000 → AtkParam_Pc 10170000 [V].
- **How the clone is written:** take the vanilla XML row element verbatim, then apply the overrides below.
- **Which fields are explicit:** fields omitted in XML mean the file's `<field defaultValue>`. So every overridden field is written explicitly, and the clone keeps the header untouched [V].

**Bullet 79000100-79000105** (bands: 40, 39, 38, 37, 36, 35):

| Field | Dagger (V) | Ours | Why |
|---|---|---|---|
| `atkId_Bullet` | 10170000 | own id | its own damage row |
| `sfxId_Bullet` | 300001 | -1 | no flying dagger model (5,165 vanilla rows use -1 [V]) |
| `sfxId_Hit` / `sfxId_Flick` | 300002 / 300003 | unchanged | visible impact feedback for M2 [I; check what 300002 looks like] |
| `life` | 0.85 | 0.15 (`regulation.bullet_life_s`) | 150 m range, about 9 frames at 60 fps (≥2 frames [R CS2 gotcha 7]) |
| `dist` | 15 | 150 | the in-range parameters apply over the whole flight |
| `gravityInRange` / `gravityOutRange` | 5 / 5 | 0 / 0 | MW2 bullets have no drop |
| `initVellocity` / `maxVellocity` / `minVellocity` | 32 | 999 (`regulation.bullet_speed_mps`) | near-hitscan; 999 is the fastest value vanilla uses [V] |
| `accelInRange` / `accelOutRange` | 0 / -30 | 0 / 0 | constant speed |
| `homingAngle` | 10 | 0 | no homing (0 is the XML default [V]) |
| `lockShootLimitAng` | 30 | 0 | no lock-on aim override (default 0 [V]) |
| `HitBulletID` | -1 | 79000900 | impact marker |
| `hitRadius` | 0.02 | unchanged | |
| `name` | (empty) | `MW2 ump45_mp band b dmg D` | readable in tools |

Everything else is unchanged:
- `isPenetrateChr` 0, `isPenetrateMap` 0, `isHitBothTeam` 0, `numShoot` 1;
- `atkAttribute` and the other attribute fields;
- `isOwnerOverrideInitAngle` 0 [V], `FollowType` 0, `EmittePosType` 0, `seId_*` -1 (we play our own sound).

**Bullet 79000900** (inert marker):
- Same clone source.
- Changed fields: `atkId_Bullet = -1`; `sfxId_Bullet/Hit/Flick = -1`; `life = 0.1`; `dist = 0`; all velocities 0; all gravity/accel 0; `homingAngle = 0`; `lockShootLimitAng = 0`; `HitBulletID = -1`; `isAttackSFX = 0`; `name = "MW2 impact marker"`.

**AtkParam_Pc 79000100-79000105**:
- `atkPhys = round(D·k)` → 160 / 156 / 152 / 148 / 144 / 140 at k = 4.0; `name = "MW2 ump45_mp dmg D"`.
- Everything else unchanged [V values]:
  - `atkPhysCorrection` 120, `atkStam` 5, `guardAtkRate` 20, `dmgLevel` 8, `atkAttribute` 2, `atkType` 2;
  - `isArrowAtk` 1, `isDisableParry` 1, `spEffectId0-4` -1, `atkSuperArmor` 1.
- `dmgLevel 8` (the dagger's hit reaction) is kept, so 11 hits/s should not stunlock the way a heavy hit level would [I; watch in game].

### 16.3 `build_regulation.py` steps

Inputs: `converted\weapons.json`, `mw2er.toml`.

1. **Source check.**
   - Hash the game's `regulation.bin` (read-only) and `work\regulation\vanilla\regulation.bin`. Both must equal the recorded SHA-256 `766521f9…12ca2ab`; the full hash lives as a constant in the script.
   - A mismatch → FAIL "game regulation changed; rerun with --rebaseline after review".
   - `--rebaseline` copies the game file afresh (read-only) and re-unpacks. It is never automatic.
2. **Preflight.** `tools\WitchyBND\app\Assets\Paramdex\ER\Upgrader\version.txt` must be `1_17_1_1000L`, else FAIL. A re-extracted WitchyBND zip undoes the bump [V].
3. **Pristine tree.** `work\regulation\unpacked\regulation-bin` (194 `.param` + `.param.xml` + `_witchy-bnd4.xml`). Re-create it from the vanilla copy if missing.
4. **Build tree.** Delete and re-create `work\regulation\build\regulation-bin` as a copy of the pristine tree.
   - Edit only `Bullet.param.xml` and `AtkParam_Pc.param.xml`: insert the rows (§16.2) in id order.
   - Delete those two `.param` files, `--repack` the two XMLs, then `--repack` the folder.
   - The other 192 `.param` files stay the vanilla binaries, so they are byte-identical.
   - WitchyBND runs via `conpty_run.py … WitchyBND.exe --passive …` with `appsettings.override.json` (Offline: true, Flexible: false) [V].
   - Assert `Operation completed on N items` with the expected N, and that the output exists. WitchyBND exits 0 on missing paths [V].
5. **Round trip.** Copy the built `regulation.bin` to `work\regulation\build_check\regulation.bin` and unpack it, plus its params in place [V gotcha]. Then:
   - (a) `param_diff.py pristine build_check --expect-added Bullet=79000100-79000105,79000900 --expect-added AtkParam_Pc=79000100-79000105`.
     - `--expect-added` is a new, stricter option. In the named params the ONLY allowed difference is exactly that set of added rows: no vanilla row changed, removed or reordered, and the ids sorted. Every other param must be same-data. Anything else is FAIL.
   - (b) Decode every added row from the **binary** with `param_rows.py` logic. Each field must equal the clone source's decoded value, except the override fields, which must equal the intended values (float tolerance 1e-6).
   - (c) `_witchy-bnd4.xml` identical to the pristine one (DCX_ZSTD, version 11711000); 194 params in and 194 out.
6. **Publish.** Only on PASS: copy to `build\package\regulation.bin` (temp file + rename). The package folder contains only `regulation.bin`.
7. **Manifest.** Write `build\rows.json` (§16.4) and `work\regulation\logs\build_<utc>.json` (full report).
8. **`--self-test`.** On temp copies, it proves the check can fail. Each case must FAIL inside the run; the run reports PASS only if all of them did:
   - (i) a flipped byte in a vanilla Bullet row;
   - (ii) an expected `atkPhys` off by 1;
   - (iii) a missing marker row;
   - (iv) a row appended out of order.

Exit codes: 0 PASS, 1 FAIL. A one-line `REGULATION PASS|FAIL …` summary is printed.

### 16.4 `build\rows.json`

```json
{ "schema": 1, "generator": "build_regulation.py", "built_utc": "...",
  "vanilla_sha256": "766521f9...", "package_regulation_sha256": "...",
  "k": 4.0, "id_base": 79000000, "marker_id": 79000900,
  "bullet": { "speed_mps": 999.0, "life_s": 0.15, "hit_radius_m": 0.02, "clone_from": 10170000 },
  "atk": { "clone_from": 10170000 },
  "weapons": { "ump45_mp": { "slot": 1, "bands": [
      { "band": 0, "id": 79000100, "mw2_damage": 40, "atk_phys": 160 },
      { "band": 5, "id": 79000105, "mw2_damage": 35, "atk_phys": 140 } ] } } }
```

(The example shows the first and last bands; the real file lists all six.)

### 16.5 Runtime PARAMS check (first in-world tick after each load)

Uses `SoloParamRepository::get::<Bullet>` / `get::<AtkParam_Pc>` [V compile]. Never `FD4ParamRepository::get::<ATK_PARAM_ST>`, which can return the Npc table.

For every id in rows.json plus the marker:
- the row exists;
- `atk_id_bullet` == id (marker: -1);
- `hit_bullet_id` == marker;
- `init_vellocity` == speed;
- `atk_phys` == `atk_phys`.

Result:
- Pass: `PARAMS OK n=13`.
- Fail: `PARAMS FAIL id=… field=… live=… want=…`; firing is disabled and a toast says "regulation rows missing/stale (launched without the package?)".

---

## 17. me3 profile and launch script

### 17.1 `profiles\mw2er.me3` [V default-profile shape; R CS2/Sunless for savefile]

```toml
profileVersion = "v1"
savefile = "ER0000.mw2er.sl2"

[[supports]]
game = "eldenring"

[[natives]]
path = 'D:\Mods\MW2inER\build\natives\mw2er.dll'
# load_early = true   # only if the log shows hudhook stuck on "Couldn't find command queue pointer"

[[packages]]
enabled = true
path = 'D:\Mods\MW2inER\build\package'
load_after = []
load_before = []
```

- `start_online` and `disable_arxan` are absent, never set.
- No `natives = []` line next to `[[natives]]` (that is a duplicate key).
- Paths are TOML literal strings (single quotes).
- me3 resolves relative package paths against the profile folder [V M0 log], but absolute paths are clearer.

### 17.2 `launch.ps1` (user-run; PowerShell 5.1 syntax)

Arguments:
- `-CheckOnly` runs the preflight and exits. This is the only mode the agent or `check_m2.ps1` ever runs.
- `-Profile` defaults to `profiles\mw2er.me3`.

Preflight. Every item prints OK/FAIL; any FAIL exits 1 without launching:
1. `$me3 = Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3\bin\me3.exe'` exists.
2. `build\natives\mw2er.dll` exists. Warn if it is older than `mod\target\release\mw2er.dll` ("run check_m2.ps1 to restage").
3. `build\package\regulation.bin` exists, and its SHA-256 equals `build\rows.json.package_regulation_sha256`.
4. The profile exists, and its natives/packages paths point at 2 and 3. `start_online` / `disable_arxan` are
   rejected as bare or quoted keys by a line rule, and (when Python is installed) by the full TOML key walk of
   `m2_static_checks.py profile`.
5. `converted\weapons.json`, `converted\sounds\sounds.json` and `mw2er.toml` exist. Missing sounds are a warning only.
6. Save backup (safety rule 3, changed after review): the SHA-256 of every live
   `%APPDATA%\EldenRing\<steamid>\ER0000.sl2` (read only) must equal some `ER0000.sl2` under `backups\`.
   `-CheckOnly` prints WARN when it does not; a real launch first copies that folder's `ER0000.sl2`,
   `ER0000.sl2.bak` and `steam_autocloud.vdf` into a new `backups\EldenRing-<yyyy-MM-dd_HHmmss>\<steamid>\`,
   verifies the hashes, and FAILs (no launch) if that copy fails. Old backups are never touched.

Launch (only without `-CheckOnly`):

```powershell
& $me3 launch -g eldenring -p $ProfilePath
```

`-g eldenring` is required together with `-p` [V MODLOG M0].

**Save note.** The profile uses a separate save, so the first launch starts a new character. To test against progressed areas, the **user** may copy their own backed-up save to `ER0000.mw2er.sl2` themselves. Neither the agent nor any script writes into `%APPDATA%\EldenRing` (the launcher only reads it for the backup).

---

## 18. Data conversion

### 18.1 `convert_weapons.py`

- Usage: `python -I tools\scripts\convert_weapons.py [--dumps dumps\common_mp\weapons] [--out converted\weapons.json] [--weapons ump45_mp,masada_mp,cheytac_mp] [--self-test]`.
- **Parsing:**
  - read as UTF-8 with latin-1 fallback;
  - require the leading `WEAPONFILE`;
  - split on `\` only (values may contain newlines [V]);
  - require an even number of tokens;
  - duplicate keys → error;
  - require exactly 671 keys (warn otherwise).
- **Numbers:** parsed as float; integer fields are checked to be integral.
- **Output:** for each weapon, a `mw2` object keeps the **original WEAPONFILE key names** (no renaming errors), plus a derived `si` object:

```json
{ "schema": 1, "generator": "convert_weapons.py", "inch_to_m": 0.0254,
  "source": { "ump45_mp": { "file": "dumps/common_mp/weapons/ump45_mp", "sha256": "...", "fields": 671 } },
  "weapons": { "ump45_mp": {
    "mw2": { "displayName": "WEAPON_UMP45", "fireType": "Full Auto", "damage": 40, "minDamage": 35,
             "maxDamageRange": 600, "minDamageRange": 850, "fireTime": 0.09, "clipSize": 32, "maxAmmo": 192,
             "startAmmo": 96, "reloadTime": 2.5, "reloadEmptyTime": 3.03, "reloadAddTime": 1.8,
             "hipSpreadStandMin": 2, "hipSpreadMax": 5, "hipSpreadDecayRate": 4, "hipSpreadFireAdd": 0.52,
             "hipSpreadMoveAdd": 4, "hipSpreadTurnAdd": 0, "adsSpread": 0, "reticleSideSize": 8,
             "reticleCenterSize": 8, "lowAmmoWarningThreshold": 0.33, "shotCount": 1, "penetrateType": "large",
             "locHead": 1.4, "locHelmet": 1.4, "fireSoundPlayer": "weap_ump45_fire_plr" },
    "si": { "maxDamageRange_m": 15.24, "minDamageRange_m": 21.59, "fireRate_rpm": 666.67 } } } }
```

- **`--self-test`:** synthetic WEAPONFILE strings (made-up values) covering a newline inside a value, an odd token count, a duplicate key and a missing required key. It must raise on the bad ones and parse the good one.
- **Known-answer check on the real output:**
  - UMP45: 40/35, 600/850 → 15.24/21.59 m, 0.09, 32, 192, 2.5, 3.03, 2/5, Full Auto;
  - ACR (`masada_mp`): 30/20, 0.077, 30;
  - Intervention (`cheytac_mp`): 70, 5, Single Shot.

  All of these are [V MODLOG/dumps].

### 18.2 `copy_sounds.py`

- A hand table maps roles to dump files [V M1 file names]:
  - `fire = common_sounds\sound\weapons\ump45\weap_ump45_slst_10b.wav`
  - `reload_lift / clipout / clipin / chamber = common_sounds\sound\foley\wpfoly_ump45_reload_{lift,clipout,clipin,chamber}_v1.wav`
- Each file is copied to `converted\sounds\ump45\<same name>` and checked:
  - RIFF/WAVE, fmt tag 1 (PCM), 44100 Hz, 16-bit;
  - destination SHA-256 equals the source.
- Writes `converted\sounds\sounds.json`: `{ "ump45_mp": { "fire": "sounds/ump45/weap_ump45_slst_10b.wav", ... , "durations_s": {...} } }`. Paths are relative to `converted\`.

---

## 19. `mw2er.toml` (full default; committed; contains no MW2 data)

```toml
schema = 1

[paths]                       # relative to the project root (<dll_dir>\..\..)
log_dir      = 'logs'
weapons_json = 'converted\weapons.json'
sounds_json  = 'converted\sounds\sounds.json'
rows_json    = 'build\rows.json'

[log]
hudhook_level    = "debug"    # never "trace"
shots            = "all"      # all | none
state_interval_s = 5.0

[keys]
toggle            = "F5"
reload            = "R"
fire_mouse_button = 0         # DIMOUSESTATE rgbButtons index; 0 = LMB
blank_toggle_key  = true

[gameplay]
weapon                  = "ump45_mp"
spawn_bullets           = true    # false = everything except spawn_bullet (triage)
refill_on_mode_enable   = true
auto_reload_on_empty    = true
max_shots_per_tick      = 2
apply_spread_to_bullets = true
use_move_spread         = true
camera_source           = "pers_cam_1"   # or "chr_cam"

[aim]
max_range_m     = 150.0
char_radius_m   = 0.75
chest_height_m  = 1.2
origin_ahead_m  = 0.6

[damage]
k = 4.0      # ER atkPhys = round(mw2_damage * k); baked by build_regulation.py (not hot)

[regulation]  # read by build_regulation.py only
id_base           = 79000000
marker_id         = 79000900
clone_bullet_from = 10170000
clone_atk_from    = 10170000
bullet_speed_mps  = 999.0
bullet_life_s     = 0.15
marker_life_s     = 0.1
[regulation.weapons]
ump45_mp = 1          # slot -> ids 79000000 + slot*100 + band

[hit]
window_s              = 0.6
max_chars             = 64
exclude_character_ids = [8000]   # Torrent

[menu]
hud_indices      = []     # ui_states indices visible in normal gameplay (fill from MENU logs); empty = baseline at F5
use_show_cursor  = false  # CSMouseMan.show_cursor as an extra "menu open" signal (unverified)
cursor_gate_at_enable = true  # while hud_indices is empty, refuse F5 when the cursor is shown (added after review)

[hud]
enabled                 = true
font_px                 = 26.0
font_path               = ''
crosshair_color         = [1.0, 1.0, 1.0, 0.85]
crosshair_line_scale    = 1.0
min_gap_px              = 4.0
fallback_vfov_deg       = 48.0
hitmarker_s             = 0.2
show_boot_toast         = true

[audio]
enabled     = true
master_gain = 0.5
fire_gain   = 0.6
foley_gain  = 0.8
fire_max_ms = 600
fire_fade_ms = 80
[audio.reload_cues]   # fraction of reload duration
lift = 0.0
clipout = 0.15
clipin = 0.55
chamber = 0.80        # empty reload only
```

**Hot reload.** The boot thread checks the file's mtime every 1 s.
- On change it re-parses and logs `CONFIG reloaded (changed: keys…)`.
- On a parse error: `CONFIG ERROR line:col msg; keeping previous`.
- Hot: `log`, `keys`, `gameplay` (the weapon only while mode is off), `aim`, `hit`, `menu`, `hud`, `audio`.
- Not hot: `paths`, `damage.k` (baked), `regulation.*` (build-time).

**Menu-open rule** [I; R CS2: ui_states works as a signal; mapping unknown]:
- `menu_open = (visible_mask & !allow) != 0 || (use_show_cursor && show_cursor)`.
- `allow` is `hud_indices` if non-empty; otherwise the visible mask captured when MW2 mode was enabled.
- `MENU` transitions are always logged, to build the index map.
- Enable gate (added after review): while `hud_indices` is empty, F5 is refused when `CSMouseMan.show_cursor`
  is set, whatever `use_show_cursor` says (`menu_logic::enable_refusal`). Otherwise a menu open at F5 would
  be captured into the baseline and count as HUD for the whole mode session. The signal is unverified, so
  `menu.cursor_gate_at_enable = false` (hot) turns the gate off. `MODE on` / `MODE toggle ignored` lines log
  `ui_states=[…] show_cursor=0|1|?` to learn what the signal does.

---

## 20. `.gitignore` additions

Current ignores (`/work/`, `/converted/`, `/dumps/`, `regulation.bin` …) stay. M2 adds:

```
/build/
/logs/
/mod/target/
/vendor/*/target/
```

`/target/` is anchored at the root, so it does not cover `mod/target` [V]. The vendor target holds ~796 MB of cache [V].

---

## 21. Offline oracles: `tools\scripts\check_m2.ps1`

The script:
- runs under Windows PowerShell 5.1;
- refreshes PATH from the registry;
- never launches ER, me3 or Steam;
- prints one line per check, `PASS|FAIL|SKIP <name> <detail>`, then a summary;
- exits 0 only if every check PASSed. SKIP counts as failure unless `-AllowSkip` is given.

| # | Check | Pass criterion |
|---|---|---|
| 1 | toolchain | `cargo`/`rustc` 1.99 found; Python 3.14 found at the full path |
| 2 | vendor pin + patches | vendor HEAD = 59fbd3b…; `git diff --name-only` lists exactly `cs/bullet_manager.rs`, `cs/field_ins.rs` and `shared/src/task.rs`; patches 0001 and 0002 each reverse-apply; `git diff` equals the two patch files concatenated |
| 3 | WitchyBND Paramdex bump | `version.txt` == `1_17_1_1000L` |
| 4 | converter self-test | `convert_weapons.py --self-test` exit 0 |
| 5 | weapons.json | conversion exit 0; known answers (§18.1) all match; schema valid |
| 6 | sounds | `copy_sounds.py` exit 0; 5 files, PCM 44.1 kHz 16-bit, hashes equal |
| 7 | cargo build | `cargo build --release` (workspace) exit 0, and no `warning:` line whose source span is under `mod\` (vendored crates' warnings are ignored) |
| 8 | cargo test | `cargo test -p mw2er-core` exit 0, ≥ 1 test in each of: weapons/json parse, weaponfile parse, cadence, ammo/reload FSM, falloff, spread/crosshair curve, blank latch, menu logic, hit tracker, log format, version gate, shot sanity, blank heartbeat, font check, config bounds |
| 9 | real-data cross-check | with `MW2ER_ROOT=D:\Mods\MW2inER`: `cargo test -p mw2er-core --test real_data -- --include-ignored` exit 0. The Rust WEAPONFILE parser over the dumps must equal `weapons.json` field by field. Missing dumps → SKIP (reported) |
| 10 | PE shape | `m2_static_checks.py pe <dll>`: exactly one export `DllMain`; no `VCRUNTIME140*` / `api-ms-win-crt-*` imports; x64 |
| 11 | stage | copy `mod\target\release\mw2er.dll` (+ `.pdb`) to `build\natives\`; hashes equal. A locked file → FAIL "is the game running?" |
| 12 | regulation | `build_regulation.py` exit 0 (build + round trip + field-level check) |
| 13 | regulation self-test | `build_regulation.py --self-test` exit 0: the unmutated build passes (round trip + XML read-back) and all 7 mutations are caught (vanilla Bullet byte, atkPhys off by 1, missing marker, rows out of order, a flipped byte in the untouched EquipParamWeapon checked without skip_untouched, missing BND manifest, an override attribute missing from the read-back XML) |
| 14 | ids consistency | `m2_static_checks.py ids`: rows.json k == mw2er.toml k; every band id = `id_base + slot·100 + band`; atk_phys == round(mw2_damage·k) using weapons.json; bands cover every integer minDamage..damage; rows.json `package_regulation_sha256` == SHA-256 of `build\package\regulation.bin`; package contains only `regulation.bin` |
| 15 | profile | `m2_static_checks.py profile`: TOML parses (tomllib); `profileVersion == "v1"`; supports eldenring; one `[[natives]]` whose path == the staged DLL and exists; one enabled `[[packages]]` whose path == `build\package` and exists; `savefile == "ER0000.mw2er.sl2"`; no `start_online`, no `disable_arxan`, no `natives = []` |
| 16 | launch preflight | `launch.ps1 -CheckOnly` exit 0. PowerShell `Parser::ParseFile` reports 0 errors for `launch.ps1` and `check_m2.ps1`. `launch.ps1` contains the full me3 path and `-g eldenring` |
| 17 | config | `mw2er.toml` parses with tomllib, AND the DLL's own parser accepts it (`cargo test -p mw2er-core --test config_file` reads the real file) |
| 18 | .gitignore | contains `/work/`, `/build/`, `/converted/`, `/logs/`, `/mod/target/`, `/vendor/*/target/`, `/dumps/`, `/backups/`, `/extracted/`, `/zones/`, `regulation.bin`, `*.sl2` |
| 19 | game folder untouched | SHA-256 of the game's `regulation.bin` == the recorded vanilla hash; no `mw2er*`, `dinput8.dll` or `regulation.bin.*` files added to the Game folder |
| 20 | log analyzer | `analyze_log.py --self-test`: a good synthetic log passes; PANIC fails; 11 HIT cases (dmg=1, dmg=0, x1.2, 30% off, unknown band, wrong mw2, no SHOT in the window, implausible, old format fail; unknown enemy only → NEEDS-CALIBRATION; min band on the reference enemy passes); fireTime / PARAMS count come from the data files |
| 21 | boot order | `m2_static_checks.py bootorder --self-test` (see §6) |

**Unit test content** (mw2er-core). These are known-answer tests; values in tests are fine, values in the DLL are not.

- **cadence:**
  - trigger held, ticks at k/60 for k = 0..=60 → exactly 12 shots (due at 0, 0.09, …, 0.99);
  - the same at 30 Hz, and with a 0.5 s hitch → no burst beyond 2 per tick;
  - tap faster than 0.09 s → still ≥ 0.09 s apart.
- **ammo/reload:**
  - 32 shots empty the mag;
  - auto-reload empty → 3.03 s, then mag 32 / reserve 160;
  - partial reload at mag 10 → 2.5 s, then 32 / 170;
  - reserve 5 → mag 15;
  - no firing while reloading or empty;
  - R at full mag ignored;
  - mode-off cancels a reload;
  - cue times 0 / 0.375 / 1.375 s (2.5 s reload) and a chamber cue only on empty.
- **falloff:**
  - 15.24 → 40, 21.59 → 35, 17.0 → 39, 20.0 → 36, 0 → 40, 1000 → 35 (avoid exact .5 midpoints in tests);
  - monotonic non-increasing;
  - every integer 35..40 maps to a band.
- **spread:**
  - min at rest;
  - +0.52 per shot, clamped;
  - decays to min in ≤ 0.25 s;
  - sustained fire stays within [min, max];
  - perturbed directions deviate ≤ spread (10k samples);
  - crosshair gap grows with spread;
  - gap at 1080p for 2° with vfov 48° = `540·tan(2°)/tan(24°)` ≈ 42.4 px.
- **blank latch:**
  - want → blank;
  - want drops while held → still blank until released.
- **menu:** baseline allowlist; a new visible index → open; transitions listed.
- **hit tracker:**
  - an attributed drop inside the window → HIT with the correct dmg;
  - outside the window → nothing;
  - a different attacker → HPDROP;
  - hp → 0 → KILL;
  - Torrent excluded;
  - a vanished handle is dropped.
- **logfmt:** `HIT chr=c4311 hp 219->116 dmg=103` exact prefix.
- **config:** defaults when keys are missing; a bad key name → error.

What the offline oracles do NOT cover (must be stated in the hand-off): every engine interaction. That means struct layouts at runtime, spawn_bullet's unknown fields, bullet collision, camera conventions, menu indices, HP attribution, hudhook's swapchain capture on native Windows 1.17.1, and audio output.

---

## 22. In-game oracles (first user run) and `analyze_log.py`

The user runs `launch.ps1`. Suggested test:
1. Load a character.
2. Press F5.
3. Single-tap a nearby enemy 5 times from under 15 m.
4. Hold LMB for 2 s.
5. Press R.
6. Open and close the inventory.
7. Press F5 again.
8. Quit.

`analyze_log.py logs\mw2er.log` reads the latest session (it splits on `BOOT` lines) and prints PASS/FAIL for each line below. These map to the M2 row of MODDING_PLAN.

| M2 criterion | Log evidence |
|---|---|
| DLL boots, version gate | `VERSION PASS file=2.7.1.0` |
| boot gate | `BOOTGATE PASS`, `TASK registered`, `PARAMS OK n=<2·bands+1 from rows.json>` (13 for the UMP45) |
| HUD drawn | `HUD initialize display=…`, no repeated `Couldn't find command queue pointer` summary; the user confirms crosshair, ammo and hitmarker visually |
| input blanking | `INPUT PASS`, and while MODE on the character did not swing (user report); `MENU` transitions present when the inventory opened |
| fires at MW2 rate | ≥ 20 `SHOT` lines inside held bursts with a median interval of 90 ms ± 17 ms (one frame at 60 fps); no `SPAWN FAIL` |
| engine bullets | `BULLET id=79000100… speed≈999`, `IMPACT … kind=hit` lines |
| hits with expected damage | every `HIT chr=c#### hp a->b dmg=N band=… mw2=…` line is consistent (N > 0; band and mw2 agree with rows.json; a SHOT within `hit.window_s` before it; 0.05·atkPhys ≤ N ≤ 2.5·atkPhys; on one enemy damage does not fall by >15% as atkPhys rises) AND at least half (and ≥ 1) of the hits on a reference enemy (`KNOWN_TARGETS`: c4311 Limgrave Godrick Soldier) are within ±15% of the §10.6 prediction computed from rows.json atkPhys. A x1.2 result FAILs with "set k = 3.0"; hits only on other enemies give NEEDS-CALIBRATION, which is not a PASS |
| reload | `RELOAD start empty=0 dur=2.5` → `RELOAD done mag=32` |
| no crashes | no `PANIC`; no `ERROR … disabled`; the session ends with the user quitting |

---

## 23. Failure handling summary

| Failure | Log | Behaviour |
|---|---|---|
| exe not 2.7.1.0 | `VERSION FAIL` | fully inert (no hooks at all) |
| mw2er.toml missing/bad | `CONFIG ERROR` | defaults / previous config |
| weapons/rows/sounds json bad | `DATA FAIL` | F5 refused with a reason; HUD toast |
| hudhook apply fails | `HUD FAIL` | no HUD; gameplay allowed |
| DI hook fails | `INPUT FAIL` | F5 refused (cannot stop ER attacking) |
| boot gate never passes | `BOOTGATE waiting missing=[…]` every 10 s | nothing registered; game unaffected |
| task registration fails | `TASK FAIL` | gameplay off |
| panic in task | `PANIC` + `ERROR task disabled` | blanking cleared first, then gameplay off for the session |
| panic in render | `PANIC` + `ERROR hud disabled` | HUD off |
| rows missing in game | `PARAMS FAIL` | firing off; toast |
| spawn_bullet Err ×3 | `SPAWN FAIL code` + `ERROR spawn breaker open` | firing off |
| no audio device / file | `AUDIO WARN` | silent |
| access violation | none (crash); the last flushed line dates it | user reports; next step decided from the log |

---

## 24. Open questions, each answered by the first run's log

1. Is `pers_cam_1` row 2 the view forward, and with which sign? Is the camera in Havok space? (`CAMERA` line, `BULLET` pos vs origin)
2. `CSCam.fov` units (radians vs degrees, vertical vs horizontal). Cosmetic: crosshair gap scale.
3. Are zeroed unknown fields and position w accepted by `spawn_bullet`? (`SPAWN`, `BULLET`)
4. Do 999 m/s bullets with radius 0.02 collide reliably, and do player-owned bullets ignore the player? (`IMPACT kind`, HIT count vs shots)
5. Is `last_hit_by` set to the player handle by our bullets? (`HIT` vs `HPDROP`)
6. The effective damage multiplier (`atkPhysCorrection` 120? defence curve) → k calibration.
7. The visual of `sfxId_Hit 300002`, and the hit reaction of `dmgLevel 8` at 11 hits/s.
8. The `ui_states` index map for HUD vs menus; whether `show_cursor` is useful.
9. Does hudhook capture the queue under me3's normal load point, or is `load_early` needed?
10. Is R bound and F5 unbound in ER's keyboard config? (Blanked either way while mode is on.)

---

## 25. Decisions the implementer must not change

1. `DllMain` only spawns the boot thread and returns 1. `panic = "unwind"`. Every FFI/hook boundary is wrapped in `catch_unwind` and logged: boot thread, task closure, every `ImguiRenderLoop` method, DI detour, audio thread, config watcher. A task panic clears the blanking flags before disabling gameplay.
2. The version gate (integer FileVersion 2.7.1.0 from the loaded image + `Er2710::detect` in `catch_unwind`) runs on the boot thread before any fromsoftware-rs call. On failure: no hooks, no threads, inert.
3. Order on the boot thread: version gate → hudhook apply → DI hook → audio → window wait → fresh reflection scan containing all 7 required names (taken from `<T as FromStatic>::name()`) → `wait_for_instance` → `run_recurring(ChrIns_PostPhysics)`. No `FromStatic` access anywhere before the gate passes, and never from DllMain, render or the detour.
4. Gameplay runs only in the CSTask closure on the main thread. render() reads only the snapshot. The detour reads and writes only atomics.
5. Dependencies: vendored fromsoftware-rs at `59fbd3b` by path, plus patch 0001 (two files) and nothing else. `hudhook =0.9.3` (dx12 only). Exactly one MinHook (`hudhook::mh`): no ilhook, no minhook-sys, no fromsoftware-rs `debug` crate, no crates.io `eldenring`. Our Win32 calls use windows 0.62.2.
6. Pure logic lives in `mw2er-core` with `#![forbid(unsafe_code)]` and no game, Win32 or hudhook dependencies. All unit tests are there.
7. Damage uses fixed rows per falloff band. No per-shot or runtime param writes in M2. `atkPhys = round(mw2_damage × k)`, k from `mw2er.toml` (default 4.0), baked by `build_regulation.py`. The DLL verifies the rows via `SoloParamRepository` (never `FD4ParamRepository::get::<ATK_PARAM_ST>`).
8. Row ids: `79000000 + slot·100 + band`, marker `79000900`. Bullet and AtkParam_Pc ids are equal. Rows are sorted. Only `Bullet` and `AtkParam_Pc` change, and only by added rows cloned from 10170000.
9. The regulation pipeline reads the game file read-only and writes only under `work\` and `build\`. `build\package\regulation.bin` is replaced only after the round trip and field-level check PASS.
10. Input blanking edits only the fire-button byte (`DIMOUSESTATE +12+b`) and the DIK bytes we own. It never touches mouse axes or other `cbData` sizes. It blanks only while mode is on, in world, with no menu open and the game in the foreground, with the release latch.
11. The DLL is staged to `build\natives\`, and the root is `<dll_dir>\..\..`. The log line format keeps the exact `HIT chr=c#### hp a->b dmg=N` prefix and flushes every line.
12. The me3 profile has `savefile = "ER0000.mw2er.sl2"`. `start_online` and `disable_arxan` are never set. The launch goes through the full me3 path with `-g eldenring -p`.
13. No MW2 data is compiled into the DLL: it loads `weapons.json`, `sounds.json` and `rows.json` at runtime. Converted files stay in gitignored folders.
14. Audio stays on its own thread. The game side only calls `try_send`. Audio failure never blocks or panics anything.
15. `check_m2.ps1` never launches ER, me3 or Steam (`launch.ps1 -CheckOnly` only), and it fails on any SKIP unless `-AllowSkip` is given.
