# {{DISPLAY_NAME}}

{{SUMMARY}}

> **Unofficial fan mod.** Not affiliated with, endorsed by or sponsored by Activision, Infinity Ward, FromSoftware or
> Bandai Namco. **No game files are included:** the setup builds the mod's data on your PC from your own Steam copies
> of Elden Ring and Call of Duty: Modern Warfare 2 (2009). Offline and private Seamless Co-op sessions only; never use
> it in Elden Ring's official online mode.

---

## Features

- **34 modern firearms**: assault rifles, SMGs, LMGs, snipers, shotguns and launchers, with hitscan shots,
  damage falloff, recoil, spread, reloads and the original sounds, all converted from your own MW2 install.
- **Aim down sights and scopes**: aim through the gun's own sights in first person; sniper scopes zoom in, also from
  third person.
- **First person** with a full gun viewmodel, or **third person**, where your character holds the gun in a two-handed
  carry, raises it into a guard to aim and the camera sits over the shoulder.
- **Modern movement**: sprint, jump, crouch, prone and mantling onto ledges.
- **AC-130 gunship** kill streak with the thermal view and the 25 mm, 40 mm and 105 mm guns.
- **Tactical Nuke** after a 25-kill streak.
- **Elden Ring stays readable**: item pickups, messages and prompts are drawn over the gun.
- **Controller support**: Xbox pads directly, PlayStation pads through Steam Input.
- **Seamless Co-op**: play with a friend in a private session, both with the mod's guns and movement. Also built in,
  but **not yet played in a two-player session**: seeing and hearing the other player's gun, tracers, shots and
  explosions, and a nuke either player can call for both worlds (see Compatibility).
- **One-file setup**: finds your games, installs the mod loader, builds and checks every file, makes shortcuts and
  registers in Apps & Features. Updates rerun only what changed and can be rolled back.

## Requirements

- Windows 10 or 11, 64-bit, and about 6 GB of free disk space during setup (about 0.4 GB stays).
- **Elden Ring** on Steam, updated to eldenring.exe **{{ER_EXE_VERSION}}** ({{ER_APP_VERSION}}).
- **Call of Duty: Modern Warfare 2 (2009)** on Steam, installed and updated (Steam build {{MW2_BUILD}}). You own and
  install it yourself; you never need to start it. Not Modern Warfare 2 Campaign Remastered and not Modern Warfare II
  (2022).
- **me3 {{ME3_VERSION}}**, the mod loader. The setup carries the official installer and runs it when you click.
- **Microsoft Visual C++ 2015-2022 x64 runtime** (most PCs with Steam games have it): the MW2 file reader the setup
  runs needs it. The setup checks for it and links Microsoft's installer (https://aka.ms/vs/17/release/vc_redist.x64.exe)
  if it is missing.
- **For co-op only: Seamless Co-op {{SEAMLESS_VERSION}}** by LukeYui, a separate download from Nexus Mods, for each
  player. Both players install the same release of this mod.

## Install

1. Download the main file and extract it anywhere (for example your Downloads folder).
2. Double-click **{{EXE_NAME}}**. If Windows shows "Windows protected your PC", click **More info**, then **Run anyway**:
   the setup is not code-signed. You can compare its sha256 with the one below.
3. Click through the setup window: **Welcome** (install folder and Solo or Solo + co-op), **Games** (found by
   itself), **me3** (click **Install me3** if it is missing), **Co-op** (your Seamless download and a co-op password),
   **Install**.
4. Wait. The first install takes 10 to 40 minutes: it reads your MW2 files, converts what the mod needs, builds the
   game settings and checks every file. It ends with **INSTALL PASSED** and a **co-op id**.
5. Start Steam, then double-click the desktop shortcut **{{DISPLAY_NAME}}** (solo) or **{{DISPLAY_NAME}} - Co-op**.
   In game, press **F5** for MW2 mode and **F6** for first person.

The setup never changes your game folders and never starts a game. It installs into your user folder
(%LOCALAPPDATA%\MW2inER by default; the internal name MW2inER stays so updates keep working) and backs up your Elden
Ring saves first. The mod plays on its own save files, so your normal character is not used.

Main file sha256: {{EXE_SHA256}} ({{EXE_NAME}}, {{EXE_MB}} MB).

## Update

Close the game, then run the new setup. Its first page says **Update** with the old and new version. Your settings
are kept, only what changed is rebuilt, and **Roll back** returns to the previous version. If the co-op id changes,
the setup says so: both players install the same new release.

## Uninstall

Windows Settings, Apps, Installed apps, **{{DISPLAY_NAME}}**, Uninstall (or **Uninstall** in the setup window). It
removes what the setup installed, its shortcuts and its registry entries, and keeps your save backups unless you tick
the box. The mod's own save files stay in your Elden Ring save folder. me3 has its own uninstaller.

## Controls

- **F5**: MW2 mode on or off. **F6**: first person on or off.
- Left mouse: fire. Right mouse: aim. **R**: reload. **1** / **2** or the mouse wheel: switch guns. **Q**: flask.
- **Shift**: sprint. **Space**: jump and mantle. **C**: crouch. **Ctrl**: prone.
- **F7**: AC-130. **4**: Tactical Nuke (after a 25-kill streak).
- Controller: click both sticks for MW2 mode, D-pad left for first person, RT / LT to fire and aim, D-pad up / down
  for the AC-130 and the nuke. The full list is in the install guide.

## Troubleshooting

- **The setup stops with a red step.** It says what is wrong in one sentence (a game not updated, me3 missing, low disk
  space). Fix it and click **Check again and continue**. The setup's logs are in **<install folder>\logs\install**
  (**Open the log folder** in the setup).
- **"differs from the release" or a hash mismatch.** Your Elden Ring or MW2 is a different build. Update both games in
  Steam, or verify their files (Steam, Properties, Installed Files, Verify integrity), then run the setup again.
- **The game closes about 8 seconds after it starts.** It was started without the mod's shortcut. Always start through
  the shortcuts; never use the Seamless launcher (ersc_launcher.exe).
- **A message box at start** (for example "Steam is not running"): fix it and click **Retry**. The launch log is
  **<install folder>\logs\me3_launch.log**.
- **Something odd in game.** Quit, then zip the **<install folder>\logs** folder (logs\mw2er.log is the mod's log) and
  attach it to a bug report. The logs contain no passwords, but they do contain your Windows folder names.
- **Earlier zip package (Install.cmd)**: its setup logs are in the package's **setup_logs** folder.
- **The setup window does not open** (very old graphics driver): it continues in a console window, or run
  **{{EXE_NAME}} --cli install** from a command prompt.

## Compatibility

- **Offline and private Seamless Co-op only.** Never use Elden Ring's official online mode with this mod.
- **Elden Ring {{ER_EXE_VERSION}} only.** After a game update, wait for a matching release of this mod.
- **Co-op needs identical installs**: both players run the same release and the setup shows the same co-op id. Use
  Seamless with a private password and no invaders (the setup sets that up).
- **Other mods**: the mod loads through its own me3 profiles and builds its own game settings file (regulation). Mods
  that replace the regulation or change weapons and movement will conflict; other me3 or DLL mods are untested. Do
  not install other mods into the game folder.
- **Shadow of the Erdtree** works; in co-op, both players need the same DLC.
- **Not yet played with two players in this release**: the co-op visuals and sounds (the other player's gun,
  tracers, shot sounds and explosions), the shared co-op nuke, and third person in co-op. They are built and checked
  offline; the last two-player session was with an earlier build (both players with MW2 guns; the nuke then cleared
  only the caller's own enemies). Please report what you see, with the logs folder.
- The gun model is not hidden behind walls in third person.
- Steam Deck and Linux are untested.

## FAQ

- **Does it include any Call of Duty or Elden Ring files?** No. The download is the setup program, documentation and
  licences. Everything taken from the games is built on your PC from your own installs and checked against the
  release's hashes.
- **Do I need to own MW2?** Yes: Call of Duty: Modern Warfare 2 (2009) on Steam, installed. You never need to start it.
- **Can I use it online?** No. Offline, or private Seamless Co-op sessions with friends.
- **Will it touch my save?** The setup backs up your saves first, and the mod plays on separate save files (the solo
  mode on its own save, co-op on Seamless's save), so your normal character is not used.
- **Why does the first install take so long?** It reads and converts MW2's weapons, models, sounds and animations. Updates
  rerun only what changed.
- **Why does Windows SmartScreen warn?** The setup is not code-signed. Compare the sha256 above, then **More info**, **Run
  anyway**.
- **My friend's co-op id differs.** You installed different releases, or one game is a different build. Both players
  install the same release with the same game versions.
- **Is the source available?** The converters and the regulation builder scripts ship as Python source in the install
  folder. The mod's own code is MIT-licensed; third-party parts keep their licences (THIRD_PARTY_NOTICES.md).

## Credits

- **me3** by garyttierney and contributors: the mod loader.
- **Seamless Co-op** by LukeYui: the co-op mod (separate download, never included).
- **OpenAssetTools** by Laupetin and contributors, in the **iw4x-x64/oat** fork: the Unlinker that reads your MW2
  files (GPL-3.0).
- **WitchyBND** by ividyon: an optional fallback for building the game settings (not included; downloaded only if you
  ask for it).
- **fromsoftware-rs** by vswarte and contributors: Elden Ring game structures.
- **hudhook** by veeenu and contributors: the overlay hook the gun is drawn with.
- **erfps2** by Dasaav-dsv: first-person camera research.
- **Paramdex** and **SoulsFormats** (the Souls modding community): param layouts and the regulation format.
- **IW4L** by vladtrc: the reference for MW2's animation rules.
- The Rust crates and other components listed in THIRD_PARTY_NOTICES.md.

## License

The mod's own code is MIT-licensed (Copyright (c) 2026 maskedpinata). Third-party components keep their own licences:
me3 (MIT or Apache-2.0), CPython (PSF License), the OpenAssetTools Unlinker (GPL-3.0; its corresponding source is
described in THIRD_PARTY_NOTICES.md and offered as an optional file on this page) and the Rust crates (see
THIRD_PARTY_NOTICES.md). The games' content belongs to its owners and is never included.

## AI disclosure

This mod was built with **Claude Code**, Anthropic's coding agent, with the author (maskedpinata) directing the work
and testing it in the game. The code, the converters and the documentation were written by the agent under that
direction. No AI-generated images, models, sounds or voices are included: the guns, sounds and animations come from
your own copy of MW2.

## Disclaimer

This is an unofficial fan mod. It is not affiliated with, endorsed by or sponsored by Activision, Infinity Ward,
FromSoftware or Bandai Namco. Elden Ring, Call of Duty and Modern Warfare are trademarks of their respective owners.
No game files are included: the setup builds the mod's data from your own Steam copies of Elden Ring and Call of Duty:
Modern Warfare 2 (2009). For offline play and private Seamless Co-op sessions only; never use it in Elden Ring's
official online mode. Use at your own risk; the setup backs up your saves first.
