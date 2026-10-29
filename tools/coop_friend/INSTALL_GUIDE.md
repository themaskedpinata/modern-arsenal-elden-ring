# Install guide: Modern Arsenal for Elden Ring

(Unofficial fan mod, not affiliated with Activision, Infinity Ward, FromSoftware or Bandai Namco. It contains no game
files: the setup builds the mod's data from your own Steam copies of Elden Ring and Call of Duty: Modern Warfare 2
(2009). Offline or private Seamless Co-op only, never Elden Ring's official online mode. Internally the mod is still
called MW2inER: install folder, `mw2er.dll`, `mw2er.toml`, the setup file name.)

This guide is for anyone who installs the mod: the friend who **joins** the host in co-op, or a player alone. You get
**one file**, `MW2inER-<version>-setup.exe` (about 35 MB). You double-click it, click through a small window, and then
start the game from a desktop shortcut. No command, no typed path, no admin rights.

Takes about 20-40 minutes the first time, most of it downloads and waiting.

(Did you get the older zip package `MW2inER-coop-friend.zip` with `Install.cmd` instead? See **"The zip package
(Install.cmd)"** at the end.)

---

## Step 1: What you need

You must own both games on Steam and have them installed. Steam can stay running throughout.

1. **Elden Ring**, updated to the current Steam version (eldenring.exe **2.7.2.0**, the 2026-10-20 update).
   - If the host owns the DLC *Shadow of the Erdtree*, you need it too.
   - Do not install any other Elden Ring mods into the game folder.
2. **Call of Duty: Modern Warfare 2 (2009)**, installed and up to date.
   - You never have to start MW2. The setup only reads its files.
3. **me3 0.13.0**, the mod loader. If you do not have it, the setup offers **"Install me3 0.13.0 (official
   installer)"**: it runs the official me3 installer it carries (checked by sha256 first).
4. **Python**: nothing to do. The setup carries the official Python 3.14.7 (embeddable package) and uses it only for
   the mod's build.
5. **No other tools.** The setup builds the game settings file (the regulation) itself; WitchyBND is no longer
   needed and nothing is downloaded. (Only if the host asks for it: **Advanced** on the Install page has a WitchyBND
   fallback; see "Advanced: the WitchyBND fallback" below.)
6. **For co-op only: Seamless Co-op 2.0.2**, from Nexus Mods (Elden Ring, mod "Seamless Co-op"). Just download the zip
   and leave it in your **Downloads** folder; the setup finds it and copies it into its own folder. If you have not
   downloaded it, the setup opens the Nexus page for you. It must be **exactly the same version as the host's** (the
   setup checks the file).
7. **For co-op only: the co-op password** the host gave you (any word you both agreed on).

---

## Step 2: Double-click the setup

1. Double-click **`MW2inER-<version>-setup.exe`** (it can stay in Downloads).
   - If a blue "Windows protected your PC" box appears, click **More info** > **Run anyway** (the file is not
     code-signed). You can compare the file with the `.sha256` the host or the release page lists.
   - No admin rights are needed and nothing asks for them.
2. A window **"Modern Arsenal for Elden Ring - Setup"** opens. Go through its pages with **Next**:

| Page | What to do |
|---|---|
| **Welcome** | Keep the install folder (`%LOCALAPPDATA%\MW2inER`) or choose another with **Change...** (never a game folder; a folder with other files in it gets a new `MW2inER` folder inside). Choose **Solo** or **Solo + co-op** |
| **Games** | Elden Ring, MW2 and this PC are found and checked by themselves. A red row has a button next to it: **Choose...** (pick the game folder), **Show the game in Steam** (update / verify it there), then **Check again** |
| **me3** | Green if me3 0.13.0 is installed. Otherwise click **Install me3 0.13.0 (official installer)**, finish that installer, and the page checks again |
| **Seamless Co-op** (co-op only) | Green when your download was found. Otherwise **Open the Nexus Mods page**, download the zip into Downloads, then **Check again** (or **Choose the zip or ersc.dll...**). Type the **co-op password** (hidden; **Show** reveals it). The setup writes it only into its own copy of `ersc_settings.ini`, together with `allow_invaders = 0` and `save_file_extension = mw2co2` |
| **Install** | Keep **"Back up my Elden Ring saves first"** and the shortcut boxes ticked. Leave **Advanced** closed. Click **Install** |
| **Done** | Shows the **co-op id**, the shortcuts and **Play** buttons |

3. Wait. The install takes **about 10-25 minutes** on a typical PC:
   - most of it is "MW2 dumps" (reading MW2); "Regulation" (building the game settings file) takes under a minute;
   - it needs about 6 GB of free space while it runs (checked first); about 0.4 GB stays.

   The setup only reads the game folders. It writes only into the install folder (and first copies your Elden Ring
   saves into its `backups\` folder). It never starts a game, me3, Steam or Seamless. **Cancel** stops cleanly; run
   the setup again and it continues where it stopped.

**You should see:** every step green, then **`INSTALL PASSED: co-op id xxxxxxxx`** and the Done page.
**For co-op: compare the co-op id with the host (by voice or chat). Equal ids = byte-identical installs.**

**If a step turns red**, the setup stops, says what is wrong in one sentence and offers **Check again and continue**.
Fix it and click that button (or run the setup again):

| Message (start) | What to do |
|---|---|
| `Elden Ring` / `MW2` not found | Install the game in Steam, or **Choose...** its folder on the Games page |
| `eldenring.exe ...` is not 2.7.2.0 | Update Elden Ring in Steam (**Show the game in Steam**) |
| `vanilla regulation.bin` differs | Steam > Elden Ring > Properties > Installed Files > **Verify integrity of game files**, then **Check again** |
| `me3 not found` / an older me3 | **Install me3 0.13.0 (official installer)** on the me3 page |
| Seamless not found / not 2.0.2 | Download the same Seamless version as the host into Downloads; never use a copy inside the game folder |
| WitchyBND not found (only with the Advanced fallback) | Untick the fallback, or tick its download box, or choose `WitchyBND-3.0.1.0-win-x64.zip` |
| `Visual C++ ... runtime missing` | **Get the Microsoft VC++ runtime** (opens https://aka.ms/vs/17/release/vc_redist.x64.exe) |
| `only ... GB free` | Free some disk space |
| the MW2 dump failed | Verify MW2's files in Steam; send the `logs\install` folder to the host |
| converted files / a file differs from the release | Your MW2 or Elden Ring is a different build than the release's. Send the `logs\install` folder (in the install folder; **Open the log folder**) to the host |

Yellow rows are not fatal (for example `MW2 build differs` is fine if the install still ends with `INSTALL PASSED`).

---

## Step 3: Start the game

1. Start **Steam** (and log in).
2. Double-click the desktop shortcut **"Modern Arsenal for Elden Ring - Co-op"** (co-op) or **"Modern Arsenal for
   Elden Ring"** (solo). They are also in the Start menu under "Modern Arsenal for Elden Ring". (Installs before 0.3.0
   named them "MW2 in Elden Ring ..."; updating replaces those shortcuts.)

   The shortcut checks the install, makes a save backup when your save changed, and starts the game through me3 (co-op
   with `--no-mem-patch`, which is required: without it the game closes about 8 seconds after it starts). If something
   is wrong (for example **Steam is not running**), a message box says so; fix it and click **Retry**. **Never use
   `ersc_launcher.exe`** (the Seamless launcher): me3 already loads Seamless.

**You should see:** Elden Ring starts (it takes a little longer than usual). Seamless shows its own messages. Shortly
after the game window appears, a message "mw2er ready - F5: MW2 mode" shows for 5 seconds.

**Solo** starts the mod alone, without Seamless, on its own save `ER0000.mw2er.sl2` (the first start makes a new
character; your normal save is never used).

---

## Step 4: First time in game

1. Choose **New Game** to make a new co-op character. Seamless keeps it in `ER0000.mw2co2`, separate from your
   normal character.
2. Play until the **first Site of Grace** and **rest** at it. Seamless then gives you the **Tiny Great Pot** and the
   **Effigy of Malenia**.
   - Before that first rest the multiplayer items are greyed out. That is normal.
3. **The host** uses the **Tiny Great Pot** to open the session.
4. **You (the joiner)** use the **Effigy of Malenia** to join the host's world.

**You should see:** a message that the host's session was joined, then you arrive in the host's world.

---

## Step 5: MW2 controls

| Key | What it does |
|---|---|
| **F5** | MW2 mode on / off (the MW2 guns replace Elden Ring's attacks) |
| **F6** | first person on / off (while MW2 mode is on) |
| Left mouse | fire |
| Right mouse | aim down sights (in third person: the character raises the gun in a two-handed guard; sniper scopes zoom in like first person) |
| **R** | reload |
| **Q** | drink a flask (Elden Ring's selected quick item) |
| **1** / **2** or mouse wheel | next / previous gun |
| **Shift** | sprint (tap) |
| **Space** | jump; climb onto ledges (mantle) |
| **C** | crouch |
| **Ctrl** | prone |
| **F7** | AC-130 gunship |
| in the AC-130: **1 / 2 / 3** or **Q** or wheel | 105 mm / 40 mm / 25 mm gun, next gun |
| in the AC-130: **F** | thermal: white hot / black hot |
| in the AC-130: **Backspace** | leave the gunship early |
| **4** | Tactical Nuke (after a 25-kill streak). In co-op either player can call it: each game kills the enemies it owns, then each player dies (C3 test) |

### With a controller (Xbox layout)

Xbox pads work directly. **PlayStation pads only work through Steam Input:** in Steam, right-click ELDEN RING >
Properties > Controller > "Enable Steam Input", then restart the game. With MW2 mode off every button does what Elden
Ring normally does.

| Button | What it does |
|---|---|
| **L3 + R3** (click both sticks together) | MW2 mode on / off (like F5) |
| **D-pad left** | first person on / off (like F6) |
| **RT** / **LT** | fire / aim down sights (third person: the two-handed guard, see below) |
| **X** tap / **X** hold | reload / talk, rest at a grace, open doors, pick up (Elden Ring's interact; while Elden Ring shows a prompt, one press interacts) |
| **Y** / **LB** | next / previous gun |
| **A** | jump; climb onto ledges |
| **B** tap / **B** hold | crouch / prone |
| **L3** | sprint (push the left stick fully forward) |
| **D-pad up** / **D-pad down** | AC-130 / Tactical Nuke |
| **R3** | melee: one Elden Ring light attack with your (hidden) weapon, in first person. In third person it does nothing (the polearm stance below is on) |
| **D-pad right** | drink a flask (Elden Ring's selected quick item) |
| **RB** | nothing in MW2 mode (Elden Ring's attack is blocked under the MW2 gun) |
| Right stick / left stick | look (MW2 console speed) / move |
| in the AC-130: **RT**, **Y**, **X**, **View** | fire, next gun, thermal, leave the gunship |
| **Start** | always Elden Ring's menu |

### Third person (new in this package; not yet tested in co-op)

In third person with MW2 mode on, your character holds the MW2 gun in its hands (Elden Ring's own weapon is hidden), in
a two-handed polearm carry. Aiming (right mouse / LT) raises it into Elden Ring's two-handed **guard**, which also
blocks hits like a normal guard (damage cut, stamina). Standing still while aiming, the body turns to the camera. The
camera sits over the right shoulder. Sniper rifles scope in like first person and go back to third person when you let
go. All of this was tested by the host alone; **it has not been tested in a co-op session yet**: if something looks
wrong for the other player, tell the host and send `logs\`. Everything is under `[coop]` / `[third_person]` in
`mw2er.toml`.

---

## Step 6: Troubleshooting

- **The game closes ~8 seconds after starting.** It was started without `--no-mem-patch`. Use the co-op shortcut
  (step 3), never the Seamless launcher.
- **Seamless asks for a session password or says it is empty.** Run the setup (shortcut **"Modern Arsenal for
  Elden Ring - Setup"**) and click **Change co-op password** on its Welcome page; use the same password as the host.
- **The multiplayer items are greyed out.** Rest at your first Site of Grace (step 4).
- **The setup says something differs from the release** (hash mismatch). See the table in step 2. Send the
  `logs\install` folder of the install folder to the host.
- **The shortcuts are missing.** Run the setup again (Repair) with the shortcut boxes ticked, or start
  `MW2inER.exe` in the install folder and use its **Play** buttons.
- **The setup window does not open** (very old graphics driver: it needs OpenGL 2). The setup then continues in a
  console window by itself. You can also run it from a command prompt:
  `MW2inER-<version>-setup.exe --cli install --mode coop`.
- **Something went wrong in game.** Quit the game, then zip the folder **`logs`** in the install folder (Explorer:
  `%LOCALAPPDATA%\MW2inER\logs`). Send that zip to the host. `logs\mw2er.log` is the mod's log. It contains no
  passwords.

---

## Repair, update, uninstall

- **Repair / change / verify**: the shortcut **"Modern Arsenal for Elden Ring - Setup"** opens the setup window on the installed
  copy: **Verify files**, **Repair / change** (steps whose inputs did not change are skipped), **Add co-op**, **Change
  co-op password**, **Open the install folder**.
- **Update**: see "Updating" below.
- **Uninstall**: Windows Settings > Apps > Installed apps > "Modern Arsenal for Elden Ring" > Uninstall (or **Uninstall...** in the
  setup window). It removes what the setup put in the install folder, the shortcuts and its registry entries. It keeps the `backups\` folder
  unless you tick "Also delete backups". It never touches the game folders, your Seamless download, me3 or
  `%APPDATA%\EldenRing`; the mod's own saves (`ER0000.mw2er.sl2`, `ER0000.mw2co2`) stay there, delete them in Explorer
  if you want. me3 has its own uninstaller.

---

## Updating

1. Close Elden Ring (the setup refuses to update while the game runs: the mod's files are in use).
2. Double-click the new `MW2inER-<new version>-setup.exe` (the host sends it, or you download it). It finds your
   install and the start page's button says, for example, **Update 0.2.0 -> 0.3.0**.
3. The Install page shows the **Update** card: the versions, the **co-op id before -> after**, **What changes** (the
   mod DLL, build scripts or tools, the regulation settings, your settings) and the **Steps that run**. Click **Update**.
   - Only what changed is copied or rebuilt: a release that only fixes the DLL takes a few seconds (no MW2 dumps, no
     converters, no regulation build); a release that changes the regulation rows rebuilds only the regulation (under
     a minute); a new gun also reruns the MW2 dumps and converters.
   - **Your settings are kept**: keys, mouse / pad sensitivity, HUD, camera offsets and every other setting you changed
     in `mw2er.toml` stay yours. A setting you never changed takes the new release's default; new settings get their
     default; settings the new version removed are dropped. The build settings (`[damage]`, `[regulation...]`, the gun
     list, `[launchers] explosion_tiers`) and the release's safety settings (test probes off) always come from the
     release, because co-op needs byte-identical files. Your old `mw2er.toml` is copied to `backups\settings\` first.
   - Your Seamless Co-op copy and its password stay as they are.
4. **Co-op**: when the co-op id changes, the setup says so ("both players must install 0.3.0"): host and friend install
   the same new setup before playing together, then compare the co-op id.
5. **Roll back**: the files the update replaced are kept. If the new version gives you trouble, open the setup (the
   shortcut **"Modern Arsenal for Elden Ring - Setup"**) and click **Roll back to 0.2.0** on the start page (it asks first). The
   previous version's files, its `mw2er.toml` and its install record come back; nothing is rebuilt. Only the version
   before the last update is kept. Co-op players roll back together (the co-op id changes back).
6. **Newer release?** The friend gets new versions from the host (nothing is checked online). A public release may show
   **"Check GitHub for a newer release"** on the start page: off unless you tick it; it asks GitHub's release list once
   and offers the release page in your browser. It never downloads anything by itself.

Command line: `MW2inER.exe --cli status` shows the update summary, `--cli install` updates, `--cli rollback [--yes]`
rolls back, `--cli check-update` asks GitHub (only for a release that names its GitHub page).

---

## Step 7: Safety notes

- **Private sessions only**: Seamless with a password only you and the host know.
- **Never use Elden Ring's official online mode with this mod.** Only start the game through the shortcuts (me3).
- **No invaders**: the setup sets `allow_invaders = 0`; keep it.
- **Your normal save stays separate.** The co-op character lives in `ER0000.mw2co2`, the solo one in
  `ER0000.mw2er.sl2`. The setup made a backup of all your Elden Ring saves in the install folder's `backups\`; keep it.
- Nothing was put into your game folders.

---

## The zip package (Install.cmd)

Older friend packages (`MW2inER-coop-friend.zip`) have no setup program. The same files are also in the install
folder of the setup program, as a fallback.

1. Make a folder of your own, for example **`D:\Mods`** (**never** under `steamapps\common`), right-click
   `MW2inER-coop-friend.zip` > **Extract All...** > choose `D:\Mods`. Do not run anything from inside the zip window.
2. This flow needs **Python 3.14** from https://www.python.org/downloads/windows/ ("Windows installer (64-bit)", tick
   **"Add python.exe to PATH"**) and me3 0.13.0 from https://github.com/garyttierney/me3/releases (v0.13.0).
3. Double-click **`Install.cmd`** in `D:\Mods\MW2inER-coop-friend`. A black window finds everything it can and asks
   with Windows boxes only for what is missing:

| Box | What to click |
|---|---|
| **Seamless Co-op 2.0.2 needed** | **Yes**: choose the zip (or `ersc.dll`). **No**: the Nexus page opens; download "Seamless Co-op" 2.0.2, leave it in Downloads, then OK |
| **Seamless settings** (your own unzipped Seamless folder only) | **Yes**: sets `allow_invaders = 0` and `save_file_extension = mw2co2` |
| **Co-op password** | **OK**: Notepad opens `ersc_settings.ini`. Type the host's password after `cooppassword =`, save, close Notepad. Setup never shows, logs or copies the password |
| **WitchyBND 3.0.1.0 needed** | **Yes** to download the official file (checked by sha256), or **No** to choose the zip you have |
| **me3 0.13.0 needed** / **Python needed** | **Yes** / **OK** opens the official download page; install it, then OK |
| **Elden Ring not found** / **MW2 not found** | OK, then choose `eldenring.exe` / `iw4sp.exe` in the game's folder |
| **Desktop shortcuts** (at the end) | **Yes**: "MW2 in Elden Ring (co-op)" and "MW2 in Elden Ring (solo)" |

4. Wait for **`SETUP PASSED: every file matches the host.`** (10-25 minutes, about 6 GB free needed). A red `FAIL`
   line names the problem (the same causes as in the table of step 2); fix it and double-click `Install.cmd` again.
   Logs are in `setup_logs\`.
5. Play with the desktop shortcuts, or **`Play co-op.cmd`** / **`Play solo.cmd`** in the package folder (Steam first).
   To remove everything, delete the package folder, the two desktop shortcuts and your Seamless download.

By hand (the same as `Install.cmd` without the boxes):

```
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Mods\MW2inER-coop-friend\setup.ps1" -SeamlessDll "D:\Mods\SeamlessCoop\ersc.dll" -WitchyZip "$env:USERPROFILE\Downloads\WitchyBND-3.0.1.0-win-x64.zip"
me3 launch -g eldenring -p "D:\Mods\MW2inER-coop-friend\profiles\mw2er-coop.me3" --no-mem-patch
```

Useful `setup.ps1` switches: `-Interactive`, `-NoGui`, `-ErGame`, `-Mw2Dir`, `-Me3`, `-Python`, `-DownloadWitchy`,
`-CheckOnly`, `-Shortcuts` / `-ShortcutsOnly`, `-KeepDumps`.

## Advanced: the setup's command line

`MW2inER-<version>-setup.exe --cli <command>` runs without a window (`help` lists everything):
`status` (checks only), `install [--mode solo|coop] [--root DIR] [--regulation-tool regbuild|witchy] [--download-witchy]
[--witchy-zip ZIP]`, `verify`,
`play solo|coop`, `uninstall`, `rollback [--yes] [--dry-run]`, `check-update`. Exit code 0 = passed, 1 = failed, 2 = usage. The co-op password is asked hidden (or
read from one line of standard input with `--password-stdin`).

### Advanced: the WitchyBND fallback

Since this release the setup builds the regulation with its own built-in builder: the same game settings as the
earlier WitchyBND build, value for value, but a different file, so the **co-op id changed once**. **Host and friend
must both install this release** (the same setup file); an older install and a new one show different co-op ids.

Only if the host asks (for example if the built-in build ever fails in game): tick **Advanced > "Build the regulation
with WitchyBND (fallback)"** on the Install page (command line: `--regulation-tool witchy`). The setup then needs
WitchyBND 3.0.1.0 again (it downloads the official 48 MB zip after you tick the box, checked by sha256, or you choose
the zip). It checks the result against the release by content (the same settings), but the co-op id differs from a
built-in install: **both players must use the same choice**.
