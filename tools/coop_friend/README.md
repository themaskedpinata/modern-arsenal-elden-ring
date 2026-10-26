# Modern Arsenal for Elden Ring

An unofficial Elden Ring mod that adds modern firearms with first-person aiming, modern movement, an AC-130 gunship
and a tactical nuke kill streak. Solo, or co-op with a friend through Seamless Co-op (private sessions only).
**Requires your own Steam copies of Elden Ring and Call of Duty: Modern Warfare 2 (2009):** the guns, sounds,
animations and numbers are built on your PC from your own MW2 install.

> **Disclaimer.** This is an unofficial fan mod. It is not affiliated with, endorsed by or sponsored by Activision,
> Infinity Ward, FromSoftware or Bandai Namco. Elden Ring, Call of Duty and Modern Warfare are trademarks of their
> owners. **No game files are included:** the setup builds the mod's data from your own Steam copies of Elden Ring
> and Call of Duty: Modern Warfare 2 (2009). Play offline, or in private Seamless Co-op sessions with friends;
> **never use it in Elden Ring's official online mode.** Use at your own risk; back up your saves (the setup also
> backs them up first).

(Internally the mod is still called MW2inER: the install folder `%LOCALAPPDATA%\MW2inER`, `mw2er.dll`, `mw2er.toml`,
the logs and the setup file `MW2inER-<version>-setup.exe` keep that name, so updates keep working.)

**Quick start (the setup program):** double-click **`MW2inER-<version>-setup.exe`**, click through its window (it
finds the games, offers the official me3 installer it carries, finds your Seamless download, asks the co-op password,
builds and verifies everything), then start Steam and double-click **"Modern Arsenal for Elden Ring - Co-op"** (or
"Modern Arsenal for Elden Ring" for solo). Python is included; nothing to install by hand except the games and, for
co-op, the Seamless zip. Updating from 0.2.0: run the new setup; its start page says **Update 0.2.0 -> 0.3.0** and the
old "MW2 in Elden Ring" shortcuts are replaced by the new ones.

**It contains no game files.** You build the mod's data yourself, from your own Elden Ring and MW2 installs, and the
setup checks that every file it produced is byte-identical to the release's. Co-op needs identical files: both players
install the same release and see the same co-op id.

The rest of this file describes co-op with a friend (the **host**) and the zip package of earlier test builds.

**Quick start (the zip package, the fallback):** unzip the package into a folder of your own, double-click
**`Install.cmd`**, answer the Windows boxes (only for what setup cannot find by itself), say Yes to the desktop
shortcuts, then start Steam and double-click **"MW2 in Elden Ring (co-op)"** (or `Play co-op.cmd`). The prerequisites,
steps and contents below describe this zip package; the setup program installs the same files plus the official
CPython embeddable package and me3 installer, and keeps these scripts in its install folder.

For a step-by-step walkthrough, see **INSTALL_GUIDE.md** (or INSTALL_GUIDE.txt).

## Safety rules

- **Private sessions only.** Use Seamless Co-op with a password that only you and the host know. Set the same
  password on both PCs. Never use Elden Ring's official online mode with this mod.
- **No invaders.** Set `allow_invaders = 0` in your Seamless `ersc_settings.ini`.
- **Start the game only through me3**, with `--no-mem-patch` (the desktop shortcut / `Play co-op.cmd` run
  `launch_coop.ps1`, which does exactly this):
  `me3 launch -g eldenring -p <package>\profiles\mw2er-coop.me3 --no-mem-patch`.
  **Never use the Seamless launcher** (`ersc_launcher.exe`). me3 already loads Seamless, so the launcher would load it a
  second time.
- **Your normal save is not touched.** Seamless writes the co-op character to `ER0000.mw2co2` (solo: `ER0000.mw2er.sl2`).
  setup.ps1 also backs up `%APPDATA%\EldenRing` into `backups\` first.
- Nothing goes into the Elden Ring or MW2 folders. Unzip this package and Seamless into folders of your own.

## The nuke in co-op (C3 test build)

The Tactical Nuke (key 4) works in a session, and either player can call it:
- The caller's game sends a marker through Elden Ring's own co-op sync. Each player's game then runs the nuke itself:
  the countdown, the flash, and the deaths of the enemies **that game owns** (no radius in a session). Each player
  dies at the end.
- Enemies that the other player's game owns are skipped and rechecked for 6 seconds, because ownership can move.
- This is a **test**. The log shows whether the marker reached the other game (`NUKE marker from=... action=start`).

The `[nuke]` settings in `mw2er.toml`:
- `coop_mode = "owned"` is the behaviour described above. `"local"` is the old one: only the caller's own enemies die.
- `allow_in_coop = true`. `false` refuses the nuke while a friend is in your world.
- `always_available = false`: you need a 25-kill streak to call the nuke. The host may test with `true`.

## Seeing each other (C3 test build)

Both players need this exact package, because the regulation changed in C3.
- Your MW2 shots send a harmless copy to your friend's game. It deals no damage, but it shows as a tracer, plays the
  gun's distant sound, and shows launcher and AC-130 explosions.
- While MW2 mode is on, Elden Ring hides your own weapon on both screens. The MW2 gun model is drawn in your hands
  instead (`[coop] world_guns`). Its position is a first guess: `[coop] world_gun_offset_m`.
- In first person, Elden Ring now gets W/A/S/D too (`[movement] fp_anim = "hybrid"`), so your friend sees you walk and
  run instead of glide.
- All of this is under `[coop]` in `mw2er.toml` and can be switched off while the game runs.

## Third person (C6; host-tested, NOT yet tested in co-op)

In third person the character holds the MW2 gun in its hands in a two-handed polearm carry (`[coop] hold_mode = "hand"`,
`er_stance = "polearm"`); aiming raises Elden Ring's two-handed guard (`tp_ads_pose = "guard"`, it blocks hits like a
guard), the body faces the camera while aiming standing still (`tp_guard_yaw`), sniper scopes zoom in
(`[first_person] tp_scope_peek`), and the camera sits over the right shoulder (`[third_person]`). The host tested it
alone; nobody has played it in a co-op session yet. Report anything odd with `logs\`.

## Prerequisites

| What | Version | Where |
|---|---|---|
| Elden Ring (Steam) | the current Steam build 25827938 (eldenring.exe 2.7.2.0, the 2026-10-20 update). If the host owns Shadow of the Erdtree, you need it too | Steam |
| Call of Duty: Modern Warfare 2 (2009, Steam) | the September 2026 x64 build (Steam build 25374375). A different build gives a warning; setup step 7 decides | Steam |
| me3 | 0.13.0 | https://github.com/garyttierney/me3/releases (installer) |
| Seamless Co-op | 2.0.1, **the same file as the host** (setup checks ersc.dll by sha256). Leave the zip in Downloads: setup unpacks it into `seamless\` here | Nexus Mods, Elden Ring mod "Seamless Co-op" (https://www.nexusmods.com/eldenring/mods/510; setup opens it) |
| Python | 3.14 (3.11 or newer works) from python.org; tick "Add python.exe to PATH" | https://www.python.org/downloads/windows/ |
| WitchyBND | 3.0.1.0, the file `WitchyBND-3.0.1.0-win-x64.zip` (setup checks it by sha256). Setup offers to download it from the official release (kept only if the sha256 matches), or you choose the zip | https://github.com/ividyon/WitchyBND/releases/tag/v3.0.1.0 |
| Visual C++ 2015-2022 x64 runtime | any recent version (Steam games usually install it) | https://aka.ms/vs/17/release/vc_redist.x64.exe |
| Disk space | about 6 GB free during setup; about 0.3 GB stays | |

You need the official Python installer. The "embeddable" Python zip might also run these scripts, since they only use
the standard library, but that was not tested.

## What setup.ps1 does

Double-click `Install.cmd` (it runs `setup.ps1 -Interactive` with `-ExecutionPolicy Bypass`, so no policy error, and
keeps the window open at the end). In that mode setup finds everything it can by itself and asks with Windows boxes
for the rest: your Seamless zip or `ersc.dll` (or it opens the Nexus page), the two Seamless ini keys (for your own
Seamless folder), the co-op password (typed by you into Notepad; setup only checks that the line is not empty), the
WitchyBND download, and the desktop shortcuts. The command line of earlier packages still works unchanged:
`powershell -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SeamlessDll <your ersc.dll> -WitchyZip <zip>`.

1. **Checks the prerequisites.** It finds Elden Ring and MW2 through Steam's `libraryfolders.vdf`. It checks:
   - the exe versions and the vanilla `regulation.bin` hash;
   - the me3 version;
   - the Seamless `ersc.dll` hash;
   - three `ersc_settings.ini` keys (it never shows your password);
   - Python, the VC++ runtime, the WitchyBND zip hash and the free disk space.
2. **Backs up your saves.** It copies `%APPDATA%\EldenRing` into `backups\` and checks every hash.
3. **Unzips WitchyBND** into `tools\WitchyBND\app`. It sets offline mode and the Paramdex upgrader version of the current regulation (1_17_3_0034L), like
   the host has.
4. **Dumps the MW2 assets the mod needs** from your MW2 install. It uses the OpenAssetTools Unlinker on
   `zone\english\*.ff` and copies a few files out of `main\*.iwd`. This goes into `dumps\`.
5. **Runs the converters**, which write `converted\`:
   - weapons, sounds and the viewmodel;
   - the AC-130, movement, the 34 guns and the nuke.

   It then checks every converted file against the host's sha256.
6. **Rebuilds the regulation** from **your** vanilla `regulation.bin`, giving `build\package\regulation.bin` and
   `build\rows.json`.
7. **Verifies everything by sha256** against `manifest\host_manifest.json`:
   - every converted file;
   - `regulation.bin`;
   - `rows.json`, ignoring its build time;
   - `mw2er.dll`.

   Then it deletes the temporary `dumps\` and `work\` folders.
8. **Writes the me3 profiles** `profiles\mw2er-coop.me3` (Seamless + the mod) and `profiles\mw2er-solo.me3` (the mod
   alone, save `ER0000.mw2er.sl2`) with your paths and prints the launch command (also in `launch_command.txt`).
   With `-Interactive` it then offers the desktop shortcuts "MW2 in Elden Ring (co-op)" / "(solo)".

## What is in the package

- `build\natives\mw2er.dll`: the mod, the same file as the host's.
- `mw2er.toml`: the host's settings under the friend policy: every test probe and experiment off
  (`[ui] probe_*`, `[coop] probe_er_*`, `world_gun_er_depth`, the crossbow carrier, `[nuke] always_available = false`),
  the in-game-tested features on (the UI injection, the message cut-out, the heal key and button, the controller
  mapping, the third-person hand hold / polearm guard / shoulder camera).
- `tools\scripts\`: our converters and the regulation builder. They are Python and use only the standard library.
- `tools\oat\bin\Unlinker.exe`: OpenAssetTools, GPL-3.0. Its licence, source and build fixes are in `tools\oat\` and
  `tools\THIRD_PARTY.md`.
- `tools\bin\sndconv.exe`: our offline MP3-to-WAV tool.
- `manifest\host_manifest.json` and `manifest\host_pins.json`: the host's hashes and versions only. They contain no
  content.
- `Install.cmd`, `Play co-op.cmd`, `Play solo.cmd` (double-click), `setup.ps1` + `setup_ui.ps1` (its dialogs),
  `launch_coop.ps1`, `launch_solo.ps1`, `profiles\mw2er-coop.template.me3`, `profiles\mw2er-solo.template.me3`, this
  README and INSTALL_GUIDE.

## Troubleshooting

- **The game closes about 8 seconds after it starts.** The launch is missing `--no-mem-patch`: me3's memory patch
  crashes Seamless 2.0.1. Use the desktop shortcut or `Play co-op.cmd`.
- **Seamless asks for a password or reports a password error.** `cooppassword` in your `ersc_settings.ini` is empty.
  Set it to the host's password.
- **The multiplayer items are greyed out.** Seamless gives you the Tiny Great Pot and the Effigy of Malenia the first
  time you rest at a Site of Grace.
- **setup reports MISMATCH or "differs from the host's".** Your MW2 or Elden Ring is probably a different build. Send
  the `setup_logs\` folder to the host.
- **Something odd happens in game.** Send the host the `logs\` folder. `logs\mw2er.log` is the mod's log.

## Credits

- **me3** by garyttierney and contributors: the mod loader (MIT OR Apache-2.0; the setup carries its official installer).
- **Seamless Co-op** by LukeYui: the co-op mod (a separate download from Nexus Mods; never shipped here).
- **OpenAssetTools** by Laupetin and contributors, in the **iw4x-x64/oat** fork for the 64-bit MW2 (2009) build: the
  Unlinker that reads your MW2 zones (GPL-3.0; source details in `THIRD_PARTY_NOTICES.md`).
- **WitchyBND** by ividyon: the regulation tool of earlier builds, now only the optional fallback the setup can
  download on request (GPL-3.0; not shipped).
- **fromsoftware-rs** by vswarte and contributors: the Elden Ring game structures the mod uses (compiled in).
- **hudhook** by veeenu and contributors: the D3D12 overlay hook (compiled in, with one small patch).
- **erfps2** by Dasaav-dsv: the first-person camera research (camera handling ideas and addresses).
- **Paramdex** and **SoulsFormats** (the Souls modding community): the param layouts and the regulation container
  facts the built-in regulation builder follows.
- **IW4L** by vladtrc: the reference for MW2's animation (XAnim) rules (Apache-2.0; reference only).
- The Rust crates and other components listed in `THIRD_PARTY_NOTICES.md`.

## Made with AI

This mod was built with **Claude Code** (Anthropic's coding agent), with the author directing the work and testing it
in the game. The code, converters and documentation were written by the agent under that direction.

## Licence

The mod's own code is MIT-licensed (`LICENSE`, Copyright (c) 2026 maskedpinata). Third-party components keep their own
licences (`THIRD_PARTY_NOTICES.md`). The games' content belongs to its owners and is never included.
