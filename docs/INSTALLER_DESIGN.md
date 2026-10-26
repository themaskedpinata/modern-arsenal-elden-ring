# MW2 in Elden Ring: one-click installer (design)

Status update 2026-10-22: **P0 implemented** (tools\coop_friend\Install.cmd, Play co-op.cmd, Play solo.cmd, setup.ps1
-Interactive + setup_ui.ps1, friend_toml.ps1; MODLOG "installer P0"); P1 and later are still design only.
Status update 2026-10-22: **P1 core implemented** (mod\installer: detection, plan, state, executor, launcher, uninstall,
`--cli`; MODLOG "installer P1 core"); the wizard and the release packaging of P1 are still to do.
Status update 2026-10-22: **P1 install pipeline implemented** (MODLOG "installer P1 pipeline"): 14 steps incl. the
bundled CPython 3.14.7 embeddable runtime (sha256-pinned zip, never downloaded), the release toml policy in Rust,
resume / cancel / crash-safe step records, logs in `<root>\logs\install` (not `setup_logs\`), payload folder or zip;
real solo self-test on the host PC through the new exe on the embeddable Python: 681/681, co-op id 5b00e3bc.
Status update 2026-10-22: **P1 wizard implemented** (MODLOG "installer P1 wizard"): the eframe/egui setup window
(Welcome, Games, me3 with the bundled official me3 0.13.0 installer, Seamless Co-op with a masked password, Install
with progress / steps / log / cancel, Done; Uninstall, Repair / Update, Verify, Play), the shortcut launcher (message
boxes), exe icon + version block + asInvoker manifest; GUI end-to-end solo install on the host PC: 681/681, 5b00e3bc.
Status update 2026-10-22: **P1 release step implemented** (MODLOG "installer P1 release"): tools\scripts\make_release.ps1
+ release_tool.py: payload staged through make_friend_package.ps1 -OutDir (+ the pinned CPython zip, the official me3
installer, licences), manifest\release.json, the deny / allow lint, the payload zip appended to the exe (64-byte
trailer, payload.rs; one file MW2inER-0.2.0-setup.exe, 34.3 MB), and a real `--cli` self-test from the embedded payload
on the host PC: 681/681, co-op id 5b00e3bc, uninstall clean, game folders / saves / registry unchanged.
Still to do in P1: the co-op release self-test (`make_release.ps1 -SelfTestMode coop`, main session) and the
second-account test.
Status update 2026-10-22: **P2 implemented, integration done** (MODLOG "installer P2 builder" + "installer P2
integrate"): `mod\regbuild` (AES / DCX_ZSTD / BND4 / PARAM + the build_regulation.py row plan; 194/194 params, 180,017
rows and the whole BND4 byte-identical to the WitchyBND build) is the installer's default regulation step, in-process
(`installer::regulation`): no WitchyBND row, step, download or Browse, no build_regulation.py; ~17 s instead of 2-5 min.
Fallback until the in-game smoke test passed: `--regulation-tool witchy` / the wizard's Install page > Advanced (the
P1 WitchyBND path, unchanged); it is verified against the release by content (pins `package_bnd4_sha256`,
`rows_json_core_sha256`) and gets its own co-op id. release.json pins `regulation_builder` (regbuild | witchybnd; empty =
witchybnd, the releases before P2); the installer defaults to the release's builder. **The package regulation sha256
changes once (WitchyBND 7739289d... -> regbuild c4a149fc...), so the co-op id changes: host and friend must update
together.** make_release.ps1 (-RegulationBuilder regbuild by default) needs no WitchyBND. The P0 zip (setup.ps1) stays
on WitchyBND and needs a WitchyBND-built host package (make_friend_package refuses a regbuild one).
Still to do in P2 (main session / user): rebuild the host's build\package with regbuild, make_release + checks, the
in-game smoke test (rows hit, AC-130, nuke), then drop the fallback.
Status update 2026-10-22: **P3 implemented** (MODLOG "installer P3 update path"): the setup over an older install says
"Update 0.2.0 -> 0.2.1"; the plan reruns only steps whose inputs changed (payload copied by sha256 diff, the converters'
key narrowed to `[arsenal] guns`, so a `[damage]` / `[regulation*]` change rebuilds only the regulation; records of the
0.2.0 setup are accepted and rewritten); `mw2er.toml` is merged (hot settings kept, a default the player never changed
follows the release via `state\release_mw2er.toml`, build inputs / `locked_toml` / the friend policy / the file layout
from the release, the old file in `backups\settings\`); everything an update replaces goes into
`state\rollback\<old version>\` (files copied, rebuilt outputs moved) and **Roll back** (window + `--cli rollback`)
restores files, toml and record; refused while `eldenring.exe` runs; the Update card shows what changes, the steps
that run and the co-op id before -> after with "both players must update"; the opt-in GitHub check (`update_repo` in
release.json, empty = hidden; api.github.com only; opens the release page, never downloads; `--cli check-update`).
Real `--cli` proof on this PC (fake releases from the 0.2.0 stage, root under work\installer\p3): 0.2.0 -> DLL-only
0.2.1 in 1.8 s with no rebuild and 3 hot settings kept; Roll back restored the old DLL / toml / record (verify 681/681);
0.2.0 -> rows-change 0.2.2 rebuilt only the regulation (18.5 s, 681/681); 0.2.2 -> hot-default 0.2.3 applied the new
default to the untouched key only. Game folders, saves, the user's own install and the registry unchanged.
Status update 2026-10-22: **P4 polish done** (MODLOG "P4 polish"): version 0.3.0 (first public release, Nexus Mods
only, uploaded by the author); public display name "Modern Arsenal for Elden Ring" (`installer::DISPLAY_NAME`) in the
window, shortcuts (old "MW2 in Elden Ring" shortcuts replaced on update, removed on uninstall), Apps & Features and the
docs, internal ids unchanged; `LICENSE` (MIT, maskedpinata 2026) + `THIRD_PARTY_NOTICES.md` (release_tool.py
`notices`, with the GPL-3.0 source note of the Unlinker) in the payload and next to the exe; CHANGELOG.md,
docs\release_notes\0.3.0.md, docs\NEXUS_PAGE_DRAFT.md. Not done from the P4 row: signing (D8: unsigned), a clean second
account install by clicks, `um publish check`.
Status update 2026-10-22 (MODLOG "P4 Nexus drafts"): `tools\scripts\make_nexus_package.ps1` (release_tool.py `nexus` /
`nexus-lint`, templates in `docs\nexus\`) writes `dist\release\<version>\nexus\`: the deterministic upload zip, the
page (Markdown + BBCode), the form values, the media checklist, the clean-PC test plan and the Unlinker's GPL source
zip; nexus lint + `um publish check` PASS for 0.3.0. Still open: the clean-PC test by the friend (CLEAN_PC_TEST.md),
media, the upload (the author's step), signing.

Status: **design only** (2026-10-22). Nothing in this document is built. No source, script, config or package was
changed to write it. Goal (user, MODLOG 2026-10-22 and the memory note "public release: one-click install"): a near
one-click install for the public release, and the same installer for the co-op friend from now on.

Evidence tags: **[V]** verified on this PC's files (path given), **[R]** reported / from memory of public sources,
not re-checked here (no web access was used), **[I]** inferred or proposed, to be proven by the milestone that needs it.
Working notes and the inventory behind this document: `work\installer\notes.md`.

---

## 0. Summary

- One Rust program, **`mw2er-setup.exe`** (new crate `mod\installer`), is the installer, the launcher (shortcuts call
  it), the updater and the uninstaller. It needs no admin rights, no PowerShell and no typed paths.
- It finds Steam, Elden Ring and MW2 itself, checks the pinned versions, copies the player's own Seamless into the
  mod folder and writes its three settings, builds the mod data from the player's own games with a progress bar,
  verifies everything against the release manifest (the current host-manifest check, kept), and writes the shortcuts.
- Dependencies, recommended: **bundle** the embeddable CPython (PSF) and the OAT Unlinker (GPL-3.0, separate program,
  as today) in phase 1; **replace WitchyBND** with an in-house Rust regulation builder in phase 2 (until then, fetch the
  official zip at install time with the pinned sha256); **bundle the official me3 0.13.0 installer** (MIT/Apache-2.0)
  or guide to it; **never** redistribute Seamless (the player downloads it; the installer opens the Nexus page);
  rebuild the Unlinker with the static CRT so no VC++ runtime is needed.
- The friend gets a better install early: phase 0 (a double-click `.cmd` + file pickers around the existing
  `setup.ps1`, about half a day) and phase 1 (the Rust installer over the existing payload, about 1-1.5 weeks).

---

## 1. The current pipeline (friend package, as of 2026-10-21)

Sources: `tools\coop_friend\setup.ps1`, `make_friend_package.ps1`, `launch_coop.ps1`, `README.md`, `INSTALL_GUIDE.md`,
`tools\scripts\friend_dump.py`, `coop_manifest.py`, `build_regulation.py`, the converters; timings from
`work\friend_selftest_2720.log` (host PC, NVMe, everything cached) [V]. The guide quotes 10-25 minutes for a typical PC.

### 1.1 What the friend does by hand before `setup.ps1`

| # | Manual step | Pain (memory note) |
|---|---|---|
| H1 | Own and install Elden Ring 2.7.2.0 (Steam build 25827938) and MW2 2009 (build 25374375) on Steam | unavoidable |
| H2 | Download and run `me3_installer.exe` v0.13.0 from GitHub | separate tool |
| H3 | Download Seamless Co-op 2.0.1 from Nexus, unzip outside the game folder, edit `ersc_settings.ini` (3 keys) in Notepad | separate setup, error-prone |
| H4 | Install Python 3.14 from python.org with "Add to PATH" | extra runtime |
| H5 | Download exactly `WitchyBND-3.0.1.0-win-x64.zip` (48 MB [V]) and leave it in Downloads | hash-pinned file |
| H6 | VC++ 2015-2022 x64 runtime (only the Unlinker needs it: imports `MSVCP140.dll`, `VCRUNTIME140(_1).dll` [V]; `sndconv.exe` is static [V]) | usually present |
| H7 | Unzip the package, open PowerShell, paste `powershell -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SeamlessDll <path> -WitchyZip <path>` | execution policy + typed paths |
| H8 | Start with `launch_coop.ps1` (again `-ExecutionPolicy Bypass`) | no double-click |

### 1.2 What `setup.ps1` does

| Step | Inputs | Tool | Outputs | Runtime (host self-test) | Disk |
|---|---|---|---|---|---|
| 1 prerequisites | `HKCU\Software\Valve\Steam\SteamPath`, `libraryfolders.vdf`, `appmanifest_1245620/10180.acf`, `eldenring.exe` FileVersion, vanilla `regulation.bin` sha256 (`b8cdab84...`), `iw4sp.exe` sha256, me3 ProductVersion, `ersc.dll` sha256 (`fcd11a18...`), 3 ini keys, Python >= 3.11, VC++ DLLs, Witchy zip sha256, free space >= 6 GB | PowerShell | `setup_logs\setup.log` | < 5 s | - |
| 2 save backup | `%APPDATA%\EldenRing\**` (read) | PowerShell | `backups\EldenRing-<date>\` (hash-checked) | seconds | size of saves (8 files) |
| 3 WitchyBND | the zip | `Expand-Archive` | `tools\WitchyBND\app\` + `appsettings.override.json` (Offline, Flexible=false) + Paramdex upgrader `version.txt` = `1_17_3_0034L` | ~10 s | 145 MB [V] |
| 4 MW2 dumps | `<MW2>\zone\english\*.ff` (17 Unlinker runs over 11 zones), 27 members of `main\*.iwd` | `Unlinker.exe --game IW4MS` (OAT fork `df203f9` + 2 patches, GPL-3.0) via `friend_dump.py` | `dumps\` | 34 s (guide: 5-15 min) | 1.8 GB temporary [V] |
| 5 converters | `dumps\`, `iw4sp.exe` (static read), `mw2er.toml` (`[arsenal] guns`) | Python: `convert_weapons`, `copy_sounds`, `convert_viewmodel`, `convert_ac130` (+`sndconv.exe`), `convert_movement`, `convert_arsenal`, `convert_nuke` | `converted\` (678 files, 141 MB [V]), `work\m3..m7` | ~8 s | 141 MB stays |
| 5b verify converted | `manifest\host_manifest.json` | `coop_manifest.py verify --only converted` | PASS/FAIL | 3 s | - |
| 6 regulation | the player's vanilla `regulation.bin`, `converted\weapons.json`, `converted\guns`, `converted\ac130`, `mw2er.toml` `[damage]`/`[regulation*]` | `build_regulation.py` driving WitchyBND (`--passive --singlethread` under `conpty_run.py`), round trip + field checks | `build\package\regulation.bin` (1.9 MB), `build\rows.json`, `work\regulation\` | 46 s | ~670 MB temporary (host's `work\regulation` [V]) |
| 7 verify all | manifest (681 entries: 678 converted + rows.json canon + regulation + DLL) | `coop_manifest.py verify --only all` | PASS/FAIL; deletes `dumps\`, `work\` | 1 s | - |
| 8 profile | `profiles\mw2er-coop.template.me3` | PowerShell | `profiles\mw2er-coop.me3` (absolute paths), `launch_command.txt` | < 1 s | - |

Peak disk about 2.7 GB plus WitchyBND (the script asks for 6 GB); about 0.3 GB stays.

### 1.3 Facts the design depends on

- **The DLL finds its data from its own path**: root = the DLL's grandparent, valid only if `<root>\mw2er.toml`
  exists (`mod\mw2er\src\paths.rs:80-82` [V]). The install tree must keep `build\natives\mw2er.dll`, `mw2er.toml`,
  `converted\`, `build\package\`, `logs\` under one root.
- **`mw2er.toml` is both player settings and build input**: `[damage]`, `[regulation*]` (read by
  `build_regulation.py` only) and `[arsenal] guns` (read by `convert_arsenal.py`) change the regulation and the
  converted data, so they must equal the release's values or the hashes differ [V].
- **The regulation is `DCX_ZSTD` level 21**, BND4 "IDs, Names1, Names2, Compression", extended 0x04, version
  11730034, 194 params (`work\regulation\unpacked\regulation-bin\_witchy-bnd4.xml` [V]). No Oodle is needed for it.
- **The build is reproducible**: 3 concurrent builds gave the same sha256 (MODLOG M6 entry [V]); the friend's
  self-test matched the host's 681/681 (`work\friend_selftest_2720.log` [V]).
- Two manifest entries are canonicalised before hashing (`rows.json` without `built_utc`; `movement.json` with
  absolute paths cut to file names) (`coop_manifest.py` [V]).
- Seamless reads `ersc_settings.ini` next to `ersc.dll`; keys present in 2.0.1 include `allow_invaders`,
  `cooppassword`, `save_file_extension` [V]. The co-op launch needs `--no-mem-patch` (me3's allocator patch crashes
  Seamless 2.0.1) [V, C0 2026-10-19].

---

## 2. Target design

### 2.1 One program, five modes

`mw2er-setup.exe` (Rust, crate `mod\installer`, binary name `MW2inER.exe` once installed):

| Mode | How it starts | What it does |
|---|---|---|
| install / repair | double-click the downloaded `MW2inER-<ver>-setup.exe` | the wizard (2.3) |
| update | same, when an install exists (2.8) | the wizard, with the steps whose inputs did not change skipped |
| launch | `MW2inER.exe launch solo` / `launch coop` (the shortcuts) | preflight, then `me3 launch -g eldenring -p <profile> [--no-mem-patch]` |
| verify | `MW2inER.exe verify` (button in the wizard, also run by launch in a quick form) | re-hash the install against the manifest |
| uninstall | Apps & Features entry or `MW2inER.exe uninstall` | 2.9 |

Plus `--cli` (console output, no window) for every mode: the host's self-test, CI-style checks, and a fallback when
the GUI cannot start.

### 2.2 Install layout (per user, no admin)

Default root `%LOCALAPPDATA%\MW2inER` (the player may choose another folder; refused inside a Steam library's
`steamapps\common`, the ER/MW2 folders, `%APPDATA%\EldenRing`, the player's own Seamless folder or `Program Files`, and
refused when it already holds other files: it must be new, empty, an install (`state\install.json`) or hold only an
earlier install's `backups\` / `logs\`; the wizard turns a picked `D:\Games` into `D:\Games\MW2inER`). Uninstall
deletes only the entries the install created. The tree keeps today's package
layout so the DLL and the scripts need no change:

```
MW2inER\
  MW2inER.exe                    the installer/launcher itself (copied here)
  mw2er.toml                     player settings (build-input sections locked, 2.8)
  build\natives\mw2er.dll        the mod
  build\package\regulation.bin   built from the player's own regulation (never shipped)
  build\rows.json
  converted\                     built from the player's own MW2 (never shipped)
  profiles\mw2er.me3, mw2er-coop.me3        written with this PC's absolute paths
  seamless\SeamlessCoop\         COPY of the player's own Seamless download (ersc.dll, locale\, ini, ...)
  runtime\python\                embeddable CPython (phase 1 only)
  tools\oat\, tools\bin\sndconv.exe, tools\scripts\, tools\THIRD_PARTY.md, licenses\
  manifest\release.json          release manifest = today's host_manifest.json + host_pins.json + release info
  state\install.json             what was installed: version, paths, input hashes per step, timestamps
  logs\  setup_logs\  backups\
```

Every write goes through one guard function (the `Guard` of `setup.ps1`, ported, with unit tests): it refuses any
path under the ER folder, its parent, the MW2 folder, any Steam library's `steamapps`, and `%APPDATA%\EldenRing`.

### 2.3 Wizard pages

1. **Welcome**: version, what will happen, the safety rules (offline/private only, own save, nothing goes into the
   game folders), disk needed. "Solo" or "Solo + co-op" choice (co-op adds the Seamless page).
2. **Games** (automatic): Steam path from `HKCU\Software\Valve\Steam\SteamPath` (fallback
   `HKLM\SOFTWARE\WOW6432Node\Valve\Steam\InstallPath`), every library from `steamapps\libraryfolders.vdf` (a small
   KeyValues parser; the file also lists the app ids per library, so ER = app 1245620 and MW2 = app 10180 are found
   without guessing folder names), then `appmanifest_<id>.acf` for `installdir`, `buildid` and `StateFlags`
   (4 = fully installed). Checks, each shown as a green/yellow/red row with a fix button:
   - `eldenring.exe` FileVersion == 2.7.2.0 and buildid 25827938 (red: "update Elden Ring in Steam"; the DLL is inert
     on any other version anyway);
   - vanilla `regulation.bin` sha256 == the pin (red: "Steam > Properties > Installed Files > Verify integrity");
   - no `SeamlessCoop\ersc.dll` or `dinput8.dll`/`modengine` leftovers inside the ER folder (yellow, explanation);
   - MW2: `zone\english\common_mp.ff` present, buildid 25374375 and `iw4sp.exe` sha256 == pin (yellow on mismatch:
     "continue; the verify step decides", as today);
   - manual "Browse..." override for each folder.
3. **me3**: finds `me3.exe` (`%LOCALAPPDATA%\Programs\garyttierney\me3\bin\me3.exe` [V], then PATH, then the
   uninstall registry entry [I]), reads ProductVersion. Missing or not 0.13.0: "Install me3 0.13.0" runs the
   bundled official installer (if bundled, decision D2) or opens the official releases page, then re-detects.
4. **Seamless Co-op** (co-op only):
   - detection: a Seamless folder the player points to, a downloaded zip in `Downloads` (any zip containing
     `SeamlessCoop/ersc.dll`), or an existing copy inside the ER folder (read only, never modified);
   - missing: "Open the Nexus page" (`https://www.nexusmods.com/eldenring/mods/510` [R]) and a "Check again" button;
   - checks `ersc.dll` sha256 against the pin (2.0.1); a different version is red for co-op (both players need the same);
   - copies the whole `SeamlessCoop` folder (from the folder or straight out of the zip) into
     `<root>\seamless\SeamlessCoop\`, never into the game folder; the player's own Seamless copy is left untouched;
   - writes `ersc_settings.ini` in the copy: keeps every other line, sets `allow_invaders = 0`,
     `save_file_extension = mw2co2`, and `cooppassword = <typed>` from a password field (masked, with a "show"
     toggle; required, not empty). The password is stored only where Seamless needs it (that ini, plain text as
     Seamless requires) and is never written to a log, `state\install.json` or the screen afterwards. "Change
     co-op password" is available later from the launcher;
   - always reminds: never use `ersc_launcher.exe`.
5. **Save backup**: copies `%APPDATA%\EldenRing` to `<root>\backups\EldenRing-<date>\` with hash checks (read-only on
   the source). Shows where the mod's own saves will be (`ER0000.mw2er.sl2` solo, `ER0000.mw2co2` co-op).
6. **Build** (one progress bar, one sub-line per step, a "Details" log pane):
   1. free space check (about 3 GB needed during the build, about 0.3 GB after [I from 1.2]);
   2. MW2 dumps (Unlinker, 17 runs + iwd members) - progress = runs done / 17;
   3. converters (7 steps) - progress = steps done; then verify converted against the manifest;
   4. regulation (phase 1: WitchyBND under the existing `build_regulation.py`; phase 2 (implemented): the Rust
      builder in-process, WitchyBND only as the Advanced fallback) - then verify the regulation and `rows.json`;
   5. verify all (681 entries) and delete `dumps\` / `work\`.
   Each external process runs with a timeout, its output goes to `setup_logs\<step>.log`, and a failure stops with a
   plain sentence ("Your MW2 is a different build than the release; 12 converted files differ") plus a
   "Copy diagnostics" button (zips `setup_logs\`, no saves, no game data, no password).
7. **Shortcuts and finish**: profiles written, shortcuts created (2.6), "Launch now" button.

### 2.4 Versions checked (the pins)

The release manifest carries the same pins as today's `host_pins.json` [V]: `er_exe_version` 2.7.2.0,
`er_app_version` (build 25827938), `vanilla_regulation_sha256`, `package_regulation_sha256`, `mw2_buildid`
25374375, `iw4sp_sha256`, `me3_version` 0.13.0, `seamless_version` 2.0.1, `ersc_sha256`, `paramdex_version`,
`mw2er_dll_sha256`, `unlinker_sha256`, `oat_commit`; and, while WitchyBND is used, `witchybnd_zip_sha256` and
`witchybnd_exe_sha256`. New: `release_version`, `coop_id` (2.8) and the sha256 of every shipped payload file.
P2 (implemented): `regulation_builder` (the builder of the host's package; the installer's default),
`package_bnd4_sha256` and `rows_json_core_sha256` (builder-independent: the WitchyBND fallback is checked by them);
the WitchyBND pins stay while the fallback exists.

### 2.5 The launch path

`MW2inER.exe launch coop` (the "MW2 in Elden Ring - Co-op" shortcut):
1. quick preflight (< 1 s): `eldenring.exe` version, the DLL, `regulation.bin`, `converted\weapons.json` and the
   Seamless copy hashed against `state\install.json`; the profile has no `start_online`/`disable_arxan` (the check of
   `launch_coop.ps1`, kept); `ersc_settings.ini` still has the three values; Steam is running
   (`steam.exe` process; if not, "Start Steam first", the program never starts Steam itself);
2. the solo save backup rule of `launch.ps1` (an identical copy of `ER0000.sl2` exists under `backups\`, else copy);
3. `me3.exe launch -g eldenring -p <root>\profiles\mw2er-coop.me3 --no-mem-patch` (solo: `mw2er.me3`, no extra flag);
   the launcher exits after me3 starts.

A changed game version shows "Elden Ring was updated (2.7.x). This version of the mod needs 2.7.2.0; wait for the
mod update" instead of launching.

### 2.6 Shortcuts

Desktop and Start-menu (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\MW2 in Elden Ring\`) `.lnk` files through
`IShellLinkW` + `IPersistFile` (the `windows` crate, features `Win32_UI_Shell`, `Win32_System_Com`): "MW2 in Elden
Ring" (`launch solo`), "MW2 in Elden Ring - Co-op" (`launch coop`, only with Seamless), "MW2 in Elden Ring - Setup"
(repair/update/verify/uninstall). Icon from the exe's resources. Working directory = root.

### 2.7 GUI toolkit

| Option | For | Against | Verdict |
|---|---|---|---|
| **egui / eframe** | pure Rust, immediate mode: a wizard, progress bars, a scrolling log and a password field are a few hundred lines; one static exe; same language as the mod | exe +3-6 MB [R]; needs OpenGL 2+ (glow) or D3D/Vulkan (wgpu) - a PC that runs Elden Ring has that; non-native look; pulls winit etc. into `Cargo.lock` (kept out of `mw2er.dll`, as `check_m4` already asserts for symphonia) | **recommended** |
| native-windows-gui | native controls, small | maintenance has slowed down [R]; more boilerplate for a dynamic wizard; layout is manual | no |
| raw Win32 (`windows` crate, already pinned at 0.62.2 in the workspace) with `TaskDialogIndirect` / a property-sheet wizard | smallest, fully native, no new big dependency | 3-5x more code for the same pages; progress/log panes by hand; slow to change | fallback for message boxes only |
| console only (`--cli`) | trivial | not one-click for the public | kept as the second front end |

Design rule: all logic lives in a `core` module with no GUI types (steps report `Progress` events over a channel);
the eframe front end and the `--cli` front end both consume it. If eframe fails to create a window, the exe shows a
`MessageBoxW` and continues in a console.

### 2.8 Update path

- **Detection**: the new setup exe finds an existing install from `HKCU\Software\MW2inER\InstallRoot` (written at
  install) or the default root, reads `state\install.json`, and offers "Update 0.3.0 -> 0.3.1" / "Repair".
- **Incremental**: each build step records the sha256 of its inputs (scripts or builder version, Unlinker, the
  player's game files' pins, the build-input toml sections). A step reruns only if one changed; e.g. a DLL-only
  release replaces `mw2er.dll` and re-verifies in seconds; a new gun reruns dumps + converters + regulation.
  The final verify against the new manifest always runs.
- **Settings**: `mw2er.toml` is merged: the player's values of hot settings stay; the build-input sections
  (`[damage]`, `[regulation*]`, `[arsenal] guns`, `[launchers] explosion_tiers`), any key the release marks `locked`,
  the release policy keys (friend_toml FORCED / REQUIRED) and the file layout (`[paths]`, `dir`, `assets_dir`) come
  from the release. P3 (implemented, `installer::toml_merge`): a line-level merge on the release file's text (its
  comments kept, only kept values swapped; no toml_edit dependency), 3-way with the previous release's file
  (`state\release_mw2er.toml`) so an untouched default follows the release; values of another type and broken files
  fall back to the release's; the old file is copied to `backups\settings\`.
  Long term the build inputs move to a separate shipped file (source change, later; decision D10).
- **Rollback**: (P3, implemented) everything the update replaces is kept in `<root>\state\rollback\<old ver>\`
  (payload files, `mw2er.toml`, `MW2inER.exe`, profiles copied; `converted\`, `build\package\`, `build\rows.json`,
  `runtime\python` moved) with the old `install.json`; "Roll back" restores them; only the last update's store is kept
  (the next update or a rollback replaces / consumes it; uninstall removes it with `state\`).
- **Never while the game runs**: refuses to update if `eldenring.exe` is running (the DLL is locked).
- **Newer-release check** (decision D6): either manual (the host sends the new exe) or an opt-in check of the GitHub
  Releases API for the latest tag, comparing the asset's sha256 listed in the release notes; no auto-download
  without a click. P3 (implemented): release.json `update_repo` (`make_release.ps1 -UpdateRepo owner/name`; empty =
  the friend's releases, no check shown); the Welcome checkbox (off by default) / `--cli check-update` asks only
  `https://api.github.com/repos/<repo>/releases/latest`, shows the tag + the sha256 values of the release notes and
  opens the release page (under https://github.com/<repo>/releases) on a click; nothing is downloaded.
- **Co-op id**: a short code (first 8 hex of the sha256 over the manifest's regulation + DLL + converted hashes and
  the Seamless pin), shown on the launcher page and in the HUD log line at boot [I]. Two players compare it by voice
  before a session; equal codes = byte-identical installs. Today's host-manifest verification is kept exactly: the
  release manifest is `coop_manifest.py write`'s output plus the pins.

### 2.9 Uninstall

Removes the shortcuts, the `HKCU\...\Uninstall\MW2inER` entry, `HKCU\Software\MW2inER`, and the install root except
`backups\` (asked: keep / delete, default keep). Never touches the game folders, the player's own Seamless download,
me3 (it has its own uninstaller) or `%APPDATA%\EldenRing`; the final page lists where the mod saves are
(`ER0000.mw2er.sl2`, `ER0000.mw2co2`) so the player can delete them in Explorer if wanted. The running exe deletes
itself with the usual "copy to %TEMP% and schedule delete" trick [I].

### 2.10 Things that make "one click" fail and their answers

| Risk | Answer |
|---|---|
| SmartScreen "Windows protected your PC" on an unsigned exe | README screenshot of "More info > Run anyway"; optional code-signing certificate (decision D8) |
| Antivirus heuristics (a new exe that spawns processes and extracts files) | no packers, embedded resources with a version-info block, publish sha256; submit false positives |
| Steam library on another drive, non-English folder names, OneDrive-redirected Downloads/Desktop | read `libraryfolders.vdf`, use `SHGetKnownFolderPath` for Desktop/Downloads, wide-char paths throughout |
| Elden Ring auto-updates and breaks the pins | red row with a clear message; the mod DLL is inert on another version anyway |
| The player picks a different Seamless version | red for co-op with the version found and the version needed |
| Low disk space | checked before the build, on the drive of the root |
| Interrupted build | steps write into temporary folders and rename on success; rerun resumes from the first incomplete step |

---

## 3. Third-party dependencies: options and recommendations

Licence files found on disk [V]: me3 `LICENSE-MIT` + `LICENSE-APACHE` (`%LOCALAPPDATA%\Programs\garyttierney\me3\`);
CPython `LICENSE.txt` (`%LOCALAPPDATA%\Programs\Python\Python314\`); OAT `tools\oat\LICENSE` (GPL-3.0);
fromsoftware-rs `LICENSE-ASL2` + `LICENSE-MIT`; iw4L `LICENSE` (Apache-2.0). WitchyBND: only the README in the app
folder states GPL-3.0 and names SoulsFormats and PromptPlus licences; the `licenses\` files it refers to are **not on
disk** [V]. **Not on disk: the Seamless Co-op licence/terms, the Paramdex licence, the VC++ redistributable licence
terms** -> everything said about them below is [I]/[R].

### 3.1 Python converters (`friend_dump.py`, 7 converters + helpers, `coop_manifest.py`; about 6.9k lines, stdlib only [V])

| Option | Work | Risk |
|---|---|---|
| (a) port to Rust | 3-5 weeks [I]: binary parsers (WEAPONFILE, XAnim, DDS/BC3, WAV, GLB, PE reading for movement), many known-answer checks | drift; but the oracle is strong: 678 converted files must keep their sha256 |
| (b) bundle the embeddable CPython 3.14 (`python-3.14.x-embed-amd64.zip`, about 10-12 MB unzipped [R]) | 1 day: unzip into `runtime\python\`, run scripts with `-I`; check that `tomllib`, `zipfile`, `zlib`, `csv`, `ctypes` (used by `conpty_run.py`) are in it [R: they are pyd/zip members of the embed package] | low. PSF licence permits redistribution with the licence text and a notice of changes [V licence file on disk, terms R]. It ships its own `vcruntime140.dll` |
| (c) download at install time from python.org, pinned sha256 | 0.5 day | network, URL drift, firewall prompts |

**Recommendation: (b) now, (a) optionally later** (phase 5), one converter at a time, each port accepted only when its
outputs are byte-identical to the Python ones across the whole manifest. Note: the JSON canonicalisation in
`coop_manifest.py` stays in Python while Python is bundled; a Rust verifier must reproduce Python's
`json.dumps(sort_keys=True, separators=(",", ":"))` float formatting exactly or the two volatile files should stop
needing canonicalisation (source change, later) [I].

### 3.2 WitchyBND (regulation unpack/repack under `build_regulation.py`)

What it does for us: AES-256-CBC decrypt/encrypt of `regulation.bin` (ER's public regulation key [R]), DCX_ZSTD
decompress/compress, BND4 unpack/repack (194 `.param` files, extended hash table), PARAM <-> XML for the 3 edited
params (Bullet, AtkParam_Pc, SpEffectParam) using Paramdex defs, and a read-back of our rows.

| Option | Work | Risk / licence |
|---|---|---|
| (a) **port the subset to Rust** (`mod\regbuild`, a library used by the installer and a dev CLI) | 4-7 days [I]: AES-CBC (`aes` + `cbc`, RustCrypto, MIT/Apache [R]), DCX header + `zstd` crate (BSD/MIT; libzstd built from C with the MSVC toolchain already installed), BND4 read/write incl. the hash table, PARAM read (port of `param_diff.read_param`, 336 lines already written by us) and write (sorted row table, data, names), row cloning + field overrides by def offset. Field layouts: the vendored fromsoftware-rs `param\generated.rs` (MIT/Apache, has `BULLET_PARAM_ST` etc. [V]) or our own table of the overridden fields, cross-checked against Paramdex defs at dev time. Existing Rust crates for FromSoftware formats exist in the community [R, e.g. vswarte's format crates; none on disk, none verified]; the needed subset is small enough to own | the package `regulation.bin` sha256 changes once (a different zstd build and writer layout), so host and friend must both move to the Rust builder in the same release ("manifest epoch"); the AES IV handling must be deterministic (reuse the vanilla file's IV [I]); the game must accept the file (one in-game smoke test by the user). Proof: a differential test that decrypts + decompresses both the WitchyBND build and the Rust build and runs `param_diff.py` on the unpacked params (must be same-data for all 194), plus the existing round-trip checks and self-test mutations ported |
| (b) bundle WitchyBND | 0.5 day | GPL-3.0: allowed with the licence text, a source offer (tag v3.0.1.0) and its bundled licences (SoulsFormats, PromptPlus, SharpShell) [R]; the zip also contains `oo2core_9_win64.dll` (RAD/Epic Oodle, proprietary, not ours to redistribute [I]) - it would have to be removed (ER's regulation is ZSTD, so it is not needed), which breaks the zip hash pin (pin the exe instead). Adds 48 MB (zip) / 145 MB (unpacked) [V]; keeps the pseudo-console wrapper and its fragility |
| (c) download at install time | 0.5-1 day: fetch `https://github.com/ividyon/WitchyBND/releases/download/3.0.1.0/WitchyBND-3.0.1.0-win-x64.zip` [I URL pattern], check the pinned zip sha256 (`a3e6b2a0...` [V]) | network; the release could be deleted; 48 MB download; the user must accept a download (decision D1) |

**Recommendation: (c) in phase 1 (with "Browse for the zip" as the offline path, i.e. today's behaviour), then (a)
in phase 2** and drop WitchyBND, `conpty_run.py`, the Paramdex upgrader bump and 145 MB of tools. WitchyBND stays a
**developer** tool for the differential test and for inspecting params.

### 3.3 OAT Unlinker (MW2 zone dumps)

| Option | Work | Risk / licence |
|---|---|---|
| (a) port to Rust | weeks to months: x64 IW4 zone loading + 10 asset types + GLB/DDS/XAnim/sound writers; iw4L (Apache-2.0, vendored, has `fastfile_iw4` and asset crates [V]) is a possible base, but it reads zone memory, not OAT's file formats, and is not verified on the Sept 2026 x64 zones [V MODLOG] | every converter input changes shape; very high drift risk |
| (b) **bundle (as today)** | 0 days now; +0.5 day to rebuild with the static CRT (`/MT`, a third local build patch) so the VC++ runtime is not needed | GPL-3.0: run as a separate program (never linked), ship `LICENSE`, the commit, the build-fix patch and the build instructions (`make_friend_package.ps1` already writes `THIRD_PARTY.md` [V]); keep a copy of the exact source archive next to the release for 3 years (GPL 6(b)/(a)) [R] |
| (c) download at install time | there is no official binary of this fork + our patches | n/a |

**Recommendation: (b)**, rebuilt with the static CRT.

### 3.4 me3 0.13.0

| Option | Work | Risk / licence |
|---|---|---|
| (a) port | n/a (it is the loader) | - |
| (b) **bundle the official, unmodified `me3_installer.exe` v0.13.0** and run it on the player's click | 0.5 day | MIT OR Apache-2.0 [V licence files]: redistribution allowed with the licence text and copyright notice. me3 installs per user and has its own uninstaller [V]. Its maintainers handle bug reports from people, not tools (MODDING_PLAN safety rule 5): the installer must not file anything |
| (c) download from GitHub releases with a pinned sha256 | 0.5 day | network; GitHub rate limits are fine for one download |

**Recommendation: (b)** if the user agrees (decision D2), else (c), else today's guide. Detect first in every case.
Accept a newer me3 with a yellow warning (decision D11), since the profile format is `v1` and `--no-mem-patch` must
still exist [I].

### 3.5 VC++ 2015-2022 runtime

Only the Unlinker needs it [V]; the embeddable Python carries its own `vcruntime140.dll`; Rust binaries are built with
`+crt-static` (as `sndconv` already is [V]). **Recommendation**: rebuild the Unlinker with `/MT` and drop the
dependency; fallback: detect and offer `https://aka.ms/vs/17/release/vc_redist.x64.exe` (Microsoft's permanent link).
App-local copies of the runtime DLLs are allowed by Microsoft's redistribution terms [R], but those terms are not
on disk [I].

### 3.6 Seamless Co-op 2.0.1

Never bundled, never downloaded by the program, never patched (distribution terms not on disk; Nexus mods are
normally not re-uploadable without permission [R]). The installer detects, hash-checks and copies the player's own
download into the mod folder, writes three ini keys, and opens the Nexus page when it is missing. Option (a)/(b)/(c)
do not apply.

### 3.7 New crates for the installer (all to be pinned with `=x.y.z` like the rest of the workspace)

`eframe`/`egui` (MIT/Apache), `windows` (already pinned), `sha2`, `serde`/`serde_json`/`toml_edit`, `zip` (read the
Seamless zip and the embedded payload), `zstd`, `aes` + `cbc` (phase 2), `ureq` or `minreq` + `rustls` only if
downloads are allowed (decision D1). Licences of each go into `licenses\` and `THIRD_PARTY.md` (`cargo about` or a
hand list) [I].

---

## 4. What stays the player's responsibility, and how the installer guides it

| Player's job | Why | Guidance |
|---|---|---|
| Own Elden Ring and MW2 (2009) on Steam, installed and updated to the pinned builds | the mod's data is built from the player's own files; nothing is redistributed | the Games page shows each game's state with "Open in Steam" (`steam://nav/games/details/<appid>` [R]) and the "Verify integrity" instruction for a modified `regulation.bin` |
| Download Seamless Co-op 2.0.1 from Nexus (co-op only) | it cannot be redistributed | "Open the Nexus page", then "I downloaded it" auto-finds the zip in Downloads |
| Choose the co-op password and share it privately with the friend | safety rule: private sessions only | masked field, never logged |
| Install me3 (if not bundled) | separate project | one button (bundled installer or the releases page) |
| Start Steam before playing | Steam P2P for Seamless, ownership | the launcher says so; it does not start Steam |
| Never use `ersc_launcher.exe`, never use official online mode | safety rules | shown on the Welcome page, in the README and on the co-op shortcut's first run |

---

## 5. Packaging and release flow

### 5.1 `make_friend_package.ps1` becomes `tools\release\make_release.ps1` (or `cargo xtask release`)

1. **Preconditions**: the staged `build\natives\mw2er.dll` == the release build; `check_c3`/`check_c4`/`check_c6`
   pass (as today before a friend package); the host's own install verifies.
2. **Build**: `cargo build --release -p mw2er-installer` (static CRT; version from `[workspace.package] version`).
3. **Stage the payload** (same list as today minus WitchyBND and PowerShell): `mw2er.dll`, release `mw2er.toml`
   (friend defaults applied: `always_available = false`, as today), the scripts (phase 1), `Unlinker.exe` + GPL
   material, `sndconv.exe`, `python-embed` (phase 1), profile templates, README/INSTALL_GUIDE, `licenses\`,
   `THIRD_PARTY.md`, and `manifest\release.json` (= `coop_manifest.py write` + the pins + per-payload-file sha256).
4. **Pre-release lint** (fails the release on any hit; extends the publish-mod lint):
   - deny by extension/name: `*.ff`, `*.iwd`, `*.iwi`, `*.dds`, `*.glb`, `*.wav`, `*.mp3`, `*.vmtex`, `*.vmmesh`,
     `*.vmanim`, `*.vmskel`, `*.rgba`, `regulation.bin`, `*.param`, `*.sl2`, `*.mw2co2`, `*.mw2er.sl2`, `ersc.dll`,
     `ersc_settings.ini`, `SeamlessCoop\`, `oo2core*.dll`, `converted\`, `dumps\`, `work\`, `*.pdb`;
   - allow-list: every staged file must be listed in an explicit payload list with its expected origin;
   - content scan of text files for `D:\SteamLibrary`, `C:\Users\`, the user name, Steam ids, `cooppassword = <non-empty>`;
   - size sanity (no unexpected file > 20 MB except the three known binaries);
   - the manifest contains hashes only (no content) - as today.
5. **Pack**: the payload zip is embedded in the installer exe (`include_bytes!` of a zip produced in step 3, or
   appended overlay) -> one file `MW2inER-<ver>-setup.exe` (about 25-35 MB in phase 1 [I]: Python ~12, Unlinker ~8,
   DLL ~9, rest small). Also write `MW2inER-<ver>-setup.exe.sha256` and `RELEASE_NOTES.md`.
6. **Self-test**: run the new exe `--cli install --root work\installer\selftest\<ver>` on the host PC (the host's own
   games); expect 681/681 and the same `coop_id`; run `--cli uninstall` and check nothing outside the root changed.
7. **Record** in MODLOG: version, exe sha256, DLL sha256, package regulation sha256, `coop_id`.

### 5.2 Versioning

- Mod version: SemVer from the workspace (`0.2.0` today [V]); bump minor for anything that changes the regulation or
  the converted data (co-op peers must update together), patch for DLL-only fixes.
- `coop_id` (2.8) decides co-op compatibility, not the version string.
- The release manifest records the game pins; a new Elden Ring patch = a new mod release with new pins
  (the "port 2.7.2.0" process in MODLOG).

### 5.3 The friend from now on

The host sends one file (`MW2inER-<ver>-setup.exe`). The friend double-clicks it; on an existing install it runs
the update path (2.8), reuses the Seamless copy and the password, rebuilds only what changed and verifies against the
new manifest. The host-manifest verification stays the gate: a mismatch stops with "send setup_logs to the host".

---

## 6. Phased plan

| Phase | Deliverable | Acceptance checks | Effort [I] |
|---|---|---|---|
| **P0 friend quick win** | `Install.cmd` + `Play co-op.cmd` next to the existing scripts: they call `setup.ps1` / `launch_coop.ps1` with `-ExecutionPolicy Bypass`; `setup.ps1` gains file-picker dialogs (WinForms `OpenFileDialog`) when `-SeamlessDll`/`-WitchyZip` are missing, and an optional "write the 3 Seamless keys + ask the password" step | friend package self-test on the host PC passes 681/681 with zero typed paths; double-click only; the package lint of today still passes | 0.5-1 day |
| **P1 Rust installer over the existing payload** | `mod\installer`: core (Steam/VDF/ACF detection, pins, guard, save backup, Seamless copy + ini, me3 detection/installer, step runner with progress over the bundled embeddable Python + Unlinker + scripts, WitchyBND fetched or browsed, manifest verify via `coop_manifest.py`, profiles, shortcuts, launch mode, uninstall), eframe wizard + `--cli` | unit tests (VDF/ACF parser on real and synthetic files, guard refuses every forbidden root, ini writer keeps other lines and never logs the password); `--cli` self-test on the host PC 681/681 and identical `coop_id`; a second Windows user account (no Python, no WitchyBND, no PATH changes) installs from the single exe with clicks only; launch shortcuts start me3 with the right profile and flag (user starts the game); uninstall leaves game folders, saves and the Seamless download unchanged (hash listing before/after) | 6-9 days |
| **P2 Rust regulation builder** | `mod\regbuild` (AES/DCX/BND4/PARAM + the row plan of `build_regulation.py`), used by the installer; WitchyBND removed from the friend path | differential test: unpacked params of the Rust build == WitchyBND build for all 194 params (`param_diff.py` same-data) and the added rows field-equal; the round-trip and self-test mutations ported and failing when they should; deterministic (3 runs, same sha); user's in-game smoke test (rows hit, AC-130, nuke); new package sha recorded; host + friend both on the new release | 4-7 days |
| **P3 update path** | `state\install.json`, incremental steps, toml merge, rollback, "newer release" check (if D6 allows) | update from P1 release to a DLL-only release in < 30 s without rebuilding; update that changes rows rebuilds only regulation; player's hot settings survive; rollback restores the old DLL | 2-3 days |
| **P4 public release** | `make_release.ps1` with the lint, THIRD_PARTY/licences, README for Nexus/GitHub, SmartScreen notes, optional signing | lint catches planted bad files (a fake `.ff`, an `ersc.dll`, a `.sl2`, a path string) - self-test; clean VM-like account install; `um publish check` clean | 2-3 days |
| **P5 optional: Python ported** | converters in Rust, one at a time; then the embeddable Python is dropped | per converter: byte-identical outputs on the full manifest; check scripts still pass | 3-5 weeks |

Order rationale: P0 removes three of the four pains for the friend within a day; P1 removes the rest except the
WitchyBND download; P2 removes WitchyBND and needs one coordinated co-op update, best done together with the next
regulation-changing release.

---

## 7. Decisions for the user

| # | Decision | Recommendation |
|---|---|---|
| D1 | Allow install-time downloads (WitchyBND until P2; optionally me3, Python) with pinned sha256 and a confirmation click? | yes for WitchyBND only, always with a "Browse..." offline alternative |
| D2 | Bundle the official me3 0.13.0 installer (licence allows it) or only link to it? | bundle, run on click |
| D3 | GUI: eframe wizard + `--cli`, or console only, or raw Win32? | eframe + `--cli` |
| D4 | Python: bundle the embeddable CPython now and port later, or port before release? | bundle now; port optional (P5) |
| D5 | Replace WitchyBND with an in-house Rust regulation builder (one-time change of the package regulation sha256 for host and friend together)? | yes, in P2 |
| D6 | Update check: manual only, or opt-in GitHub Releases check? | manual for the friend; opt-in check for the public |
| D7 | Default install folder `%LOCALAPPDATA%\MW2inER` (no admin) or ask every time? | default + "Change..." |
| D8 | Buy a code-signing certificate (SmartScreen) or ship unsigned with instructions? | unsigned first; revisit after the public release |
| D9 | Rebuild the Unlinker with the static CRT (a third local build patch) to drop the VC++ runtime? | yes |
| D10 | Split `mw2er.toml` build inputs (`[damage]`, `[regulation*]`, `[arsenal] guns`) into a separate shipped file? | later; P3 merges with locked sections meanwhile |
| D11 | me3 version: exactly 0.13.0 (red) or newer allowed with a warning? | newer allowed with a warning; Seamless stays exact |
| D12 | Where to publish (GitHub Releases, Nexus, both) - affects the update check and the README | both, GitHub as the source of the exe + sha256 |
| D13 | Keep the solo-only install path (no Seamless page) for the public release? | yes |
