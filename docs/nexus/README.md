# {{DISPLAY_NAME}} {{VERSION}}

An unofficial Elden Ring mod: modern firearms with aim-down-sights and scopes, first or third person, modern movement
with sprint and mantling, an AC-130 gunship and a tactical nuke kill streak. Solo, or with a friend in a private
Seamless Co-op session.

DISCLAIMER: unofficial fan mod, not affiliated with, endorsed by or sponsored by Activision, Infinity Ward,
FromSoftware or Bandai Namco. Elden Ring, Call of Duty and Modern Warfare are trademarks of their respective owners.
No game files are included: the setup builds the mod's data from your own Steam copies of Elden Ring and Call of Duty:
Modern Warfare 2 (2009). Offline and private Seamless Co-op sessions only; never use it in Elden Ring's official
online mode. Use at your own risk; the setup backs up your saves first.

## What is in this archive

- {{EXE_NAME}}: the setup program (everything else it needs is inside it).
- README.txt: this file.
- LICENSE: the MIT licence of the mod's own code.
- THIRD_PARTY_NOTICES.md: every bundled third-party component and its licence, and what is not included.
- CHANGELOG.md: what changed in each version.

## Requirements

- Windows 10 or 11, 64-bit; about 6 GB free during setup (about 0.4 GB stays).
- Elden Ring on Steam, eldenring.exe {{ER_EXE_VERSION}} ({{ER_APP_VERSION}}).
- Call of Duty: Modern Warfare 2 (2009) on Steam, installed and updated (Steam build {{MW2_BUILD}}). You never need to
  start it; the setup only reads its files.
- me3 {{ME3_VERSION}} (the mod loader): the setup carries the official installer and runs it when you click.
- Microsoft Visual C++ 2015-2022 x64 runtime (usually present): the MW2 file reader needs it; the setup checks for it
  and links https://aka.ms/vs/17/release/vc_redist.x64.exe if it is missing.
- For co-op only: Seamless Co-op {{SEAMLESS_VERSION}} by LukeYui, downloaded by each player from Nexus Mods. Leave the
  zip in your Downloads folder; the setup finds it.

## Install

1. Double-click {{EXE_NAME}}. If Windows says "Windows protected your PC", click More info, then Run anyway (the setup
   is not code-signed). Its sha256 is {{EXE_SHA256}}.
2. Go through the pages with Next: Welcome (install folder, Solo or Solo + co-op), Games (found by itself), me3
   (Install me3 if missing), Co-op (your Seamless download and the co-op password both players agreed on), Install.
3. Wait 10 to 40 minutes. The setup ends with "INSTALL PASSED: co-op id ..." and the Done page. In co-op, compare the
   co-op id with your friend: it must be the same.
4. Start Steam, then double-click the desktop shortcut "{{DISPLAY_NAME}}" (solo) or "{{DISPLAY_NAME}} - Co-op".
5. In game: F5 turns MW2 mode on, F6 switches to first person. Never start the game with ersc_launcher.exe.

The setup never changes the game folders and never starts a game, Steam or me3 by itself. It installs into
%LOCALAPPDATA%\MW2inER (you can choose another folder). The internal name MW2inER stays in folder and file names so
updates keep working. The full install guide is INSTALL_GUIDE.txt in the install folder.

## Update

Close the game and run the new setup: its first page says "Update <old> -> <new>". Your settings are kept, only what
changed is rebuilt, and "Roll back" returns to the previous version. When the co-op id changes, both players install
the same new release.

## Uninstall

Windows Settings > Apps > Installed apps > "{{DISPLAY_NAME}}" > Uninstall. It keeps your save backups unless you tick
the box; the mod's own save files (ER0000.mw2er.sl2, ER0000.mw2co2) stay in your Elden Ring save folder.

## Troubleshooting and logs

- The setup's logs: <install folder>\logs\install (the setup's "Open the log folder" button).
- The launch log: <install folder>\logs\me3_launch.log.
- The mod's log: <install folder>\logs\mw2er.log.
- The earlier zip package (Install.cmd): <package folder>\setup_logs.
- When reporting a problem, zip the whole logs folder (before uninstalling: the uninstall removes it). The logs
  contain no passwords but do contain your Windows folder names.
- The game closes about 8 seconds after start: it was not started with the mod's shortcut.

## Credits

me3 (garyttierney and contributors), Seamless Co-op (LukeYui, separate download), OpenAssetTools (Laupetin and
contributors; the iw4x-x64/oat fork), WitchyBND (ividyon; optional fallback, not included), fromsoftware-rs (vswarte
and contributors), hudhook (veeenu and contributors), erfps2 (Dasaav-dsv; first-person camera research), Paramdex and
SoulsFormats (the Souls modding community), IW4L (vladtrc; MW2 animation reference), and the Rust crates listed in
THIRD_PARTY_NOTICES.md.

## Made with AI

Built with Claude Code, Anthropic's coding agent, with the author (maskedpinata) directing the work and testing it in
the game.

## Licence

The mod's own code: MIT (LICENSE, Copyright (c) 2026 maskedpinata). Third-party components keep their own licences
(THIRD_PARTY_NOTICES.md).
