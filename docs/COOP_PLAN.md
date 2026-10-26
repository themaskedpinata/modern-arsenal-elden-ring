# COOP_PLAN: MW2 in Elden Ring with Seamless Co-op

Status: research only (2026-10-18). Nothing was built, launched, downloaded, or edited outside this file.
Nothing in `Documents\seamless` or the game folder was touched. The co-op password was never read or printed.

Evidence levels used below:
- **[V]** verified from bytes, files, or a fetched source
- **[I]** inferred
- **[C]** creator or community claim
- **[?]** unknown, needs a test

## 1. Verdict

**Feasible, with limits.** A first co-op build (C1-C2) is low risk. In it the guns work for each player on their own screen. **Nuke kills everyone** is a new design that a 2-PC probe has to prove first.

- **Safety boundary holds [V].**
  - ersc.dll reports itself as "Seamless Co-op v2.0.1 by Yui". It says it "does not connect to the matchmaking servers".
  - It uses Steam lobbies and SteamNetworkingMessages.
  - The me3 configuration reference states that `start_online` "is *not* needed for Seamless Co-op". me3 blocks official matchmaking by default.
- **Loading [V/I].**
  - ersc.dll exports only `modengine_ext_init`. It is 13,753,872 bytes, has PE timestamp 2026-09-02, and is WinLicense-packed.
  - me3 0.13.0's `me3_mod_host.dll` contains "modengine_ext_init ... loaded native with me2 compatibility shim". The me3 changelog lists "Support loading mods that rely on modengine_ext_init".
  - Loading ersc.dll as a me3 `[[natives]]` entry is therefore plausible, but it has not been tested next to mw2er.dll [?].
  - me3 issue #296 says me3 cannot pass an ini to a native, so it is unknown whether ersc finds its ini under me3 [?].
- **Game version [?].** No source ties v2.0.1 to ER 1.17.1 (exe 2.7.1.0). The DLL contains "The mod may not be compatible with the installed game version". Only a title-screen test can answer this.
- **Bullet sync [V].**
  - At 0x1403a2d28, the bytes `f6 46 44 08 74 66` are `test [rsi+44h],8 ; je`. Bit 3 of BulletSpawnData+0x44 gates the BULLET_SYNC_SETTING network record.
  - We never set bit 3, so every MW2 bullet, AC-130 round, and horizon explosion exists only on the shooter's PC.
- **SpEffect sync filter [V].**
  - 0x1403F6000 skips rows with isDisableNetSync (+0x259 bit 4) or with u16 297 at +0x156.
  - That a packet is actually sent, and that it reaches the peer, is [I]/[?].
- **Current nuke [V].** `chars.rs:194-205` excludes player, ghost, summon-buddy, and debug sets. `ac130.rs` applies with `dont_sync=true`. In co-op today, friends never die.
- **Enemy HP authority [I, unsupported].** NetChrSetSync replicates current_hp from the owner. Who owns which enemy (host only, or distance-based) is not traced.

## 2. Setup (user and friend)

**Before anything else**, the user does these by hand:
1. Back up `%APPDATA%\EldenRing\<steamid>\ER0000.sl2` and `ER0000.mw2er.sl2`.
2. Update `MODDING_PLAN.md` rule 2 ("never Seamless Co-op with these files") on purpose.

The `launch.ps1` and M2 check-15 gates need a co-op variant. A later build workflow writes that variant. The gates are not to be bypassed.

**Each player installs:**
- Elden Ring 1.17.1 (exe 2.7.1.0). Add Shadow of the Erdtree if the host owns it: Seamless says "You must own Shadow of the Erdtree to join this session" [V].
- me3 0.13.0.
- The identical `mw2er.dll`. Compare the sha256 and the same `build\package` regulation. Use the same `mw2er.toml`.
- MW2 assets converted from **their own** MW2 copy. Activision assets are not shared.
- The identical Seamless 2.0.1 `ersc.dll`. The user's copy has sha256 `fcd11a18...b251`.

**Seamless placement.** Make your own copy: `Documents\seamless\SeamlessCoop` → `D:\Mods\MW2inER\coop\SeamlessCoop`. The original stays untouched. Edit only the copy's `ersc_settings.ini`:
- `allow_invaders = 0`. **Recommended for MW2 sessions; your decision.** You currently have 1, and Seamless v2 has random invasions ("Invades a random world").
- `save_file_extension = mw2co2`. This keeps MW2 apart from your normal `.co2` saves.
- Use the same password and the same scaling on both PCs. Friendly fire and PvP (Judicator's Rulebook) stay off.
- Optional: `death_debuffs = 0`. Every nuke wipe would otherwise give Rot Essence.

**Do not use `ersc_launcher.exe`.** It injects ersc.dll itself via CreateRemoteThread [V], so running it would load Seamless twice.

**Co-op profile** (`profiles\mw2er-coop.me3`, to be created in a build run). The verifier's correction applies: **keep a profile savefile**. If ersc fails to start, the save is still redirected and the real ER0000.sl2 is never used.
```toml
profileVersion = "v1"
savefile = "ER0000.mw2coop.sl2"   # safety net if Seamless does not initialise
# start_online / disable_arxan absent
[[supports]]
game = "eldenring"
[[natives]]
path = 'D:\Mods\MW2inER\coop\SeamlessCoop\ersc.dll'
# if not detected: initializer = { function = "modengine_ext_init" }  (syntax unverified)
[[natives]]
path = 'D:\Mods\MW2inER\build\natives\mw2er.dll'
[[packages]]
enabled = true
path = 'D:\Mods\MW2inER\build\package'
```
It is unknown whether the me3 savefile or the Seamless extension wins [?]. Record which file appears in the save folder.

## 3. First co-op build: what works vs what needs work

| Feature | First build | Why / needs |
|---|---|---|
| Guns and damage, host | Works [I] | The host owns world enemies, and its kills replicate |
| Guns and damage, client on host enemies | **Unknown [?]** | Unsynced bullet. The damage may land, be reverted, or be rejected. The C3 test decides whether bit 3 is needed |
| Seamless HP scaling | Weaker guns | +35% enemy / +100% boss HP per extra player. The nuke uses %maxHP, so it is likely unaffected [I] |
| Viewmodel, HUD, audio, thermal, FP camera | Works, local [I] | Our own D3D12 renderer, our own audio thread |
| Body hide | Local only [I] | The friend sees your full ER body with your equipped ER weapon |
| Movement | Replicates position [I] | The friend sees you glide in an idle pose with no sprint, slide, or mantle animation. A mantle or re-base may look like a teleport |
| Mantle (M8, still in fix) | Same as movement | Fix it solo first (run 19) |
| Peer sees MW2 guns, tracers, sounds | **No** | Needs marker SpEffects (sounds and effects) or bit 3. A gun model on peers is high cost (D3D12 overlay or FLVER replacement) |
| AC-130 | Camera works locally; damage across owners [?] | Protection 79000950 stays local. Force-exit on nuke |
| Nuke kills everyone | **No** | Today only the caller dies, plus the enemies on the caller's screen. A client's nuke may desync. **Gate it in sessions** until C5 |

## 4. Milestones (each needs a 2-PC test)

- **C0: Seamless alone.** Use the co-op profile with mw2er and the package commented out. Proof: the Seamless welcome text appears, there is no "may not be compatible", and a `.mw2co2` or `mw2coop` save file appears.
- **C1: Both mods load, solo.** Proof: `logs\mw2er.log` shows a clean PARAMS check and hooks installed, and hudhook finds the command queue (try `load_early` if not). There is no Seamless kick or cheat message.
- **C2: Join plus vanilla play.** Both players walk and fight with ER weapons and keep MW2 off. Proof: Seamless "{} joined"; no desync.
- **C3: Damage authority probe.** Build a log line on both PCs: `COOP hp chr=<handle> hp=<n> owner=<network_authority>`. Test: the client shoots one host enemy, and both logs compare HP. Decide whether bit 3 is needed.
- **C4: Marker probe.** Use a no-op SpEffect row with isDisableNetSync=0, applied to your own player with `dont_sync=false`. Proof: the peer logs `COOP marker seen peer=<id> sp=79001190 dt=<ms>`. If it does not appear, the nuke stays local-only or disabled in sessions.
- **C5: Co-op nuke.** The caller applies the marker. Every peer runs NukeSeq locally, kills the NPCs it owns, then kills its own player. Proof on both PCs: `NUKE marker from=<peer>`, `NUKE det`, `NUKE playerkill ok`. Both die, then party wipe and respawn at grace.
- **C6: Peer visibility.** Fire, ADS, and gun-id markers are used to play MW2 sounds and tracers at the peer. Proof: `COOP peer state gun=<i> firing=1`.
- **C7: AC-130 in co-op.** Force-exit on a nuke marker. Proof: client `HIT src=ac130` matches the host's enemy HP.

## 5. Risks and open questions

- **Seamless on 1.17.1:** possible pattern-scan failure.
- **Cheat detection:** Seamless includes `CSCheatDetectionSpider::WhatIsPatchBytes` [V]. It may flag our hooks or the custom rows 79000xxx. The consequence could be a kick, or a restriction on Seamless's server.
- **Two packed DLLs hooking the game** could crash. Test step by step.
- **Regulation mismatch:** no check was found, but the DLL is packed. A mismatch likely causes a silent desync. Require byte-identical packages.
- **Bit 3 costs:** turning it on floods Steam P2P, can exhaust peers' emitter pools (the 256 cap), and may double-count damage.
- **Ownership:** enemy ownership may be distance-based. If it is, host-only logic is wrong.
- **Crash reports:** Seamless crashpad may upload dumps that contain mw2er.
- **Open questions:**
  - Does ersc read its ini next to the DLL under me3?
  - What is the native load order?
  - What does vfunc +0x120 at 0x14049e2e6 check?
  - Do Seamless peers appear in `player_chr_set` with a readable SpEffect list?
  - How does Seamless implement HP scaling?
  - Is party-wipe timing right for near-simultaneous deaths?

## 6. User decisions

1. Set `allow_invaders = 0` in your copied ini (recommended), and keep friendly fire and PvP off.
2. Update `MODDING_PLAN.md` rule 2 to allow Seamless (private P2P only, `start_online` never set).
3. Choose the save names (profile `savefile` plus `save_file_extension`), and whether to copy your MW2 character over.
4. Choose the nuke behaviour while C4 is unproven: disabled in sessions, or local-only.
5. Choose peer visuals: effects and sounds only first (recommended), or the gun model later (high cost).
6. Choose whether the nuke should also kill invaders and spirit summons.
7. Choose `death_debuffs` and the scaling values.

## 7. C2 build (2026-10-19): peer watch, nuke in sessions, friend package

**Peer detection (read-only, `game\chars.rs player_set_slots` + pure `mw2er_core::coop`).**
- At 4 Hz, the DLL reads `WorldChrMan.player_chr_set` (vendored 2.7.1.0 layout): every slot's ChrIns pointer,
  `chr_load_status`, `chr_update_type`, and the character's `field_ins_handle` and `chr_type`.
- A slot counts as a **peer** when all of these hold:
  - it is not `main_player`, matched by address or by handle;
  - it is not `sinner_hunter`;
  - its load status is 1..=4;
  - its chr_type is not an NPC phantom (5, 19, 20 or 21).
- Joins count at once. A leave counts only after the peer has been missing for 3 s, so loading screens do not flicker
  it.
- Every change is logged as `COOP session peers=<n> was=<m> joined=[..] left=[..] handles=[h:type=:st=:upd=] npc=<k>
  lobby=<CSSessionManager.lobby_state> role=<host|client|unknown> sess_players=<n>`.
- One `COOP watch slots= peers_seen= ...` line per boot shows the detector ran.
- Steam ids and names are never read.
- Status: [V] for the offsets (vendored crate, `offset_of!`). Whether Seamless peers really appear in
  `player_chr_set`, and with which chr_type / update type, is [I] until the C2 run. The role mapping (lobby_state 3 =
  host, 6 = client) is also [I]; Seamless may not drive the vanilla session manager, in which case the role is
  `unknown`.

**Nuke (user decision 2026-10-19, revised in the same session).**
- `[nuke] allow_in_coop = true` is the default, also in mw2er.toml and in the friend package. The nuke works with a
  peer present and stays local: the caller's enemies die, then the caller dies. Peers are never targeted, because the
  kill pass skips player-set members.
- The activation line carries `coop_peers=<n> role=`.
- A one-time HUD note appears per co-op session: "Co-op nuke: only your game's enemies are affected; friends won't see
  it yet".
- `allow_in_coop = false` restores the blocking path:
  - key 4 is refused with `NUKE blocked reason=coop_session peers=<n>` and the toast "Nuke disabled in co-op";
  - a countdown is cancelled with refund when a peer joins: `NUKE cancel reason=coop_session refund=1` (`0` for a
    test activation, like the loading cancel);
  - after the detonation the local sequence finishes.
- The host should call the nuke. "Everyone dies" is C4/C5.

**Friend package.** `dist\MW2inER-coop-friend` (+ .zip) is built by `tools\coop_friend\make_friend_package.ps1`.
- It contains no game data.
- The friend's `setup.ps1` re-dumps MW2 with the shipped Unlinker (`tools\scripts\friend_dump.py`, the reconstructed
  dump recipe) and runs our converters.
- It rebuilds the regulation from the friend's own vanilla `regulation.bin` (`MW2ER_GAME_REGULATION`).
- It then verifies every file against the host's hash manifest (`coop_manifest.py`).
