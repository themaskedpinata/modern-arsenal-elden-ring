# Clean-PC test plan: {{DISPLAY_NAME}} {{VERSION}}

For a tester on a second Windows PC that has never had this mod (the friend's PC). It checks that the public setup
works for someone who downloads it cold: no developer tools, no earlier install, no help except this sheet. Plan
about 1 to 1.5 hours (most of it is waiting for the install). Do every step in order and write down what you see where
it says **Note**. Send back the items in "What to send back" at the end, even when everything passed.

Release under test: **{{EXE_NAME}}**, sha256 `{{EXE_SHA256}}`, co-op id **{{COOP_ID}}**.

## 0. Before you start

- Windows 10 or 11, 64-bit. **Note** the Windows version (Settings > System > About) and the graphics card.
- Steam installed and logged in; **Elden Ring** installed and updated (eldenring.exe {{ER_EXE_VERSION}}).
- **Call of Duty: Modern Warfare 2 (2009)** installed and updated in Steam. You do not need to start it.
- Clean state, if you can: no me3 installed, no earlier MW2inER or "MW2 in Elden Ring" install, no
  `%LOCALAPPDATA%\MW2inER` folder. **Note** anything that was already there (me3 version, an old install).
- For the co-op part: Seamless Co-op {{SEAMLESS_VERSION}} downloaded from Nexus Mods into your **Downloads** folder (do not
  unzip it), and the co-op password agreed with the host.
- At least 6 GB free on the drive of `%LOCALAPPDATA%`.
- Close Elden Ring. Keep Steam running.

## 1. Download and check the file

1. Get **{{ZIP_NAME}}** (from the host or the Nexus page) and extract it into a new folder in Downloads.
2. Expected contents: `{{EXE_NAME}}`, `README.txt`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `CHANGELOG.md`.
3. Open a command prompt in that folder and run: `certutil -hashfile {{EXE_NAME}} SHA256`.
   **Expected:** `{{EXE_SHA256}}` (certutil prints it without dashes, same letters). **Note** if it differs and stop.
4. Open README.txt and skim it. **Note** anything unclear.

## 2. Start the setup

1. Double-click `{{EXE_NAME}}`.
2. **Expected:** possibly a blue "Windows protected your PC" box (the setup is not code-signed). Click **More info** >
   **Run anyway**. **Note** whether it appeared, and whether your antivirus said anything (exact text or a screenshot).
3. **Expected:** no administrator prompt. A dark window titled **"{{DISPLAY_NAME}}"**, "setup {{VERSION}}", with the steps
   Welcome, Games, me3, Co-op, Install, Done. **Note** if the window did not open, opened blank, or a console window
   opened instead (very old graphics drivers fall back to the console).

## 3. The pages

| Page | Expected | What you do |
|---|---|---|
| **Welcome** | Install folder `C:\Users\<you>\AppData\Local\MW2inER`, Mode Solo / **Solo + co-op**, Release **{{VERSION}}** with co-op id **{{COOP_ID}}** (green dots) | Choose **Solo + co-op** if you test co-op, else Solo. Next |
| **Games** | Elden Ring found, Game version **{{ER_EXE_VERSION}}**, Steam build, regulation.bin **vanilla**; Modern Warfare 2 found, MW2 build; Steam found, Python **bundled 3.14.7**, free space. All green | Next. If a row is red or yellow, take a screenshot and follow its button (Choose folder / Show the game in Steam / Check again) |
| **me3** | On a clean PC: **not installed**, button **Install me3 {{ME3_VERSION}}** | Click it, finish the me3 installer (its own window), then **Check again**: the row turns green. **Note** what the me3 installer showed |
| **Co-op** (co-op only) | Seamless Co-op **{{SEAMLESS_VERSION}}** found in Downloads (green); a password box | Type the agreed password. Next |
| **Install** | "Back up my Elden Ring saves first" and the shortcut boxes ticked; Advanced closed | Click **Install**; write down the start time |
| **Installing** | 13 steps: Checks, Save backup, Mod files, Python runtime, Seamless Co-op copy (co-op), MW2 dumps, Converters, Verify converted, Regulation, Verify everything, Clean up, me3 profiles, Shortcuts + record. "MW2 dumps" is the longest | Wait; do not use the PC heavily. **Note** the total time |
| **Done** | **Ready to play**, "{{DISPLAY_NAME}} {{VERSION}} is installed", co-op id **{{COOP_ID}}**, buttons Play co-op / Play solo, three green shortcuts | Screenshot this page |

**Expected result line** (Show log on the Installing page, and in the log file): `INSTALL PASSED: co-op id {{COOP_ID}}`.
If a step turns red: screenshot it, click **Open the log folder**, and continue with "What to send back".

## 4. Check what the setup made (Explorer, no changes)

- [ ] Desktop shortcuts **{{DISPLAY_NAME}}**, **{{DISPLAY_NAME}} - Co-op**, **{{DISPLAY_NAME}} - Setup** (the co-op one only
  in co-op mode), the same in the Start menu folder **{{DISPLAY_NAME}}**.
- [ ] Settings > Apps > Installed apps lists **{{DISPLAY_NAME}}**, publisher **maskedpinata**, version {{VERSION}}.
- [ ] `%LOCALAPPDATA%\MW2inER` has `MW2inER.exe`, `mw2er.toml`, `build`, `converted`, `profiles`, `runtime`,
  `backups`, `logs`, `state` (and `seamless` in co-op). No `dumps` or `work` folder is left.
- [ ] `%LOCALAPPDATA%\MW2inER\logs\install\install.log` exists and its last lines contain
  `INSTALL PASSED: co-op id {{COOP_ID}}`.
- [ ] Your Elden Ring and MW2 game folders have no new files (sort by date modified: nothing from today).

## 5. Verify (optional, 1 minute)

Start **{{DISPLAY_NAME}} - Setup**. **Expected:** the start page shows the installed {{VERSION}} with **Verify files**,
**Repair / change**, **Uninstall**. Click **Verify files**. **Expected:** all files pass (a count like 681/681) and the
co-op id {{COOP_ID}}. Close it.

## 6. First start, solo

1. Steam is running. Double-click **{{DISPLAY_NAME}}** (solo).
2. **Expected:** no message box (if one appears, e.g. "Steam is not running", screenshot it, fix it, click **Retry**).
   Elden Ring starts a little slower than usual. Shortly after the window appears, the message
   **"mw2er ready - F5: MW2 mode"** shows for about 5 seconds.
3. Solo uses its own save: the first start makes a new character (your normal character is not used). Make one quickly
   or skip the intro as far as the game lets you.
4. In game, test and tick:
   - [ ] **F5**: MW2 mode on (a gun appears). **F6**: first person on / off.
   - [ ] Left mouse fires (sound, hit on an enemy); right mouse aims down sights; **R** reloads; **1** / **2** or the
     wheel switch guns; try a sniper scope.
   - [ ] **Shift** sprint, **Space** jump and mantle onto a low wall, **C** crouch, **Ctrl** prone.
   - [ ] **F7** AC-130: thermal view, switch guns with 1 / 2 / 3, **Backspace** leaves.
   - [ ] Third person (F6 off): the character holds the gun; right mouse raises the guard; the camera is over the
     shoulder.
   - [ ] An item pickup or a message is readable over the gun.
   - [ ] A controller, if you have one: click both sticks for MW2 mode, RT fire, LT aim, D-pad up AC-130.
   - [ ] Frame rate and stability: **Note** any stutter, crash or freeze, with the time it happened.
5. Quit to the desktop through the game's menu.
6. **Expected log lines** in `%LOCALAPPDATA%\MW2inER\logs\mw2er.log` (open with Notepad, near the top):
   `BOOT mw2er 0.2.0 ...` (the mod file of this release reports 0.2.0; that is expected: gameplay is unchanged from
   0.2.0), `VERSION PASS file={{ER_EXE_VERSION}} ...`, `ARSENAL data n=34 ok=34 disabled=0 ...`. **Note** any line with
   `ERROR`, `FAIL` or `panic`.
7. `%LOCALAPPDATA%\MW2inER\logs\me3_launch.log` exists (me3's output of this start).

## 7. Co-op with the host (only if you installed Solo + co-op)

1. Compare co-op ids with the host by voice or chat: both must show **{{COOP_ID}}**.
2. Both start **{{DISPLAY_NAME}} - Co-op** (never `ersc_launcher.exe`). **Expected:** Seamless Co-op shows its own messages;
   the game does not close after about 8 seconds.
3. Make a co-op character, rest at the first Site of Grace (Seamless gives the Tiny Great Pot and the Effigy of
   Malenia). The host opens the session with the pot; you join with the effigy.
4. Tick:
   - [ ] Joined the host's world.
   - [ ] You see the host's gun, tracers and explosions and hear the host's shots (and the host sees yours).
   - [ ] Shooting enemies together works; note any enemy that does not die or behaves oddly.
   - [ ] One of you calls the Tactical Nuke only if the 25-kill streak is reached (optional).
   - [ ] Third person in co-op (untested before this test): **Note** what each player sees.
5. Quit both games through the menu.

## 8. Uninstall (last; save the logs first)

1. **Zip the logs first**: right-click `%LOCALAPPDATA%\MW2inER\logs` > Send to > Compressed (zipped) folder, and keep
   the zip on your Desktop. The uninstall removes the logs folder.
2. Settings > Apps > Installed apps > **{{DISPLAY_NAME}}** > Uninstall. **Expected:** the setup window asks first, then
   says it is uninstalled; leave "Also delete backups" unticked.
3. Tick:
   - [ ] The shortcuts and the Start menu folder are gone; the Apps entry is gone.
   - [ ] `%LOCALAPPDATA%\MW2inER` holds only `backups` (or is gone).
   - [ ] The game folders are unchanged; `%APPDATA%\EldenRing` still has your normal save.
   - [ ] me3 is still installed (it has its own uninstaller; leave it).

## What to send back

1. The **logs zip** from step 8 (or, if you stopped early, a zip of `%LOCALAPPDATA%\MW2inER\logs`). It contains
   `logs\install\` (the setup), `logs\mw2er.log` (the mod) and `logs\me3_launch.log` (the launch). The logs contain no
   password, but they show your Windows user folder name; that is fine to share with the host only.
2. Screenshots: the SmartScreen / antivirus box (if any), the Games page, the Done page, every red step or message box.
3. Your notes: Windows version, graphics card, total install time, me3 installer experience, every **Note** above and
   every unticked box.
4. Do **not** send: `ersc_settings.ini`, the co-op password, save files (`.sl2`, `.co2`, `.mw2co2`) or anything from the
   `backups` folder.

## Pass criteria (for the author)

- Hash matches; the setup opens without admin rights; the pages look as described.
- `INSTALL PASSED: co-op id {{COOP_ID}}` on a PC that never had the mod, me3 installed through the setup's button.
- Solo start: the ready message, MW2 mode, first person, firing, the AC-130 work; the log has `VERSION PASS` and
  `ARSENAL data n=34 ok=34`.
- (Co-op) equal co-op ids, a joined session, each other's shots visible.
- Uninstall leaves only `backups`; game folders and the normal save unchanged.
