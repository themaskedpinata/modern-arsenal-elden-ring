# Changelog: Modern Arsenal for Elden Ring (internal name MW2inER)

Versions follow the Rust workspace (`mod\Cargo.toml`). Details of every step are in `MODLOG.md`.

## 0.3.2 - 2026-10-28 (Seamless Co-op 2.0.2)

- Co-op now uses Seamless Co-op 2.0.2: 2.0.1 refuses to start since the 2.0.2 release ("Update required"), which broke
  0.3.1's co-op shortcut (solo was unaffected). The setup pins 2.0.2's `ersc.dll` (sha256 0a093d849096...) and finds the
  official Nexus zip in Downloads by itself. Only `ersc.dll` differs from 2.0.1; `ersc_settings.ini` keys are unchanged.
- Gameplay unchanged from 0.3.1 (same `mw2er.dll` code); both players must install 0.3.2 (the co-op id changes).

## 0.3.1 - 2026-10-24 (co-op fixes from the first two-player test of 0.3.0)

- Tactical Nuke in co-op: the host's game now kills every enemy in range; the joining player's game waits
  `[nuke] coop_client_fallback_s` (1 s) and then kills whatever is still alive, so enemies die exactly once. (0.3.0 left
  them all alive: both games treated every enemy as the other player's.) A player who is dead or loading when the
  nuke starts still runs the kills.
- No more shooting while dead: MW2 firing, launchers, the AC-130 / nuke keys, melee and heal are blocked from the death
  until 0.5 s after the respawn, in first and third person (0.3.0 could fire from the corpse while Seamless kept a dead
  player in the world).
- Tracers no longer show through walls: each tracer stops at the first wall along the shot, and parts hidden behind
  walls from your camera are not drawn (map rays, budgeted; `[coop] tracer_occlusion`, `tracer_occlusion_rays`).
- Both players must install 0.3.1 (the co-op id changes).

## 0.3.0 - 2026-10-22 (first public release)

- Public name **Modern Arsenal for Elden Ring** in the setup window, the shortcuts, Apps & Features and the docs. Internal
  names are unchanged (install folder `MW2inER`, registry keys, `state\install.json`, `mw2er.dll`, the me3 profiles, the
  co-op id), so a 0.2.0 install updates in place: the setup shows **Update 0.2.0 -> 0.3.0**, keeps your settings and
  replaces the old "MW2 in Elden Ring" shortcuts with the new ones (the uninstall removes either).
- Licence: the mod's own code is MIT (`LICENSE`, maskedpinata 2026); the setup carries `LICENSE` and
  `THIRD_PARTY_NOTICES.md` (every bundled component, the Rust crates, the GPL-3.0 source note of the OpenAssetTools
  Unlinker, what is not shipped).
- Disclaimer, credits and the AI disclosure in README / INSTALL_GUIDE; release notes per version.
- Gameplay unchanged from 0.2.0 (the same in-game-tested `mw2er.dll`).

## 0.2.0 - 2026-10-22 (setup program)

- One-file setup `MW2inER-0.2.0-setup.exe` (Rust, eframe window + `--cli`): finds Elden Ring / MW2 / me3 / Seamless,
  carries the official me3 0.13.0 installer and the CPython 3.14.7 embeddable package, backs up the saves, dumps the
  player's own MW2 assets (OpenAssetTools Unlinker), runs the converters, verifies every file by sha256, writes the me3
  profiles and shortcuts, registers in Apps & Features; launcher with preflight checks; repair / verify / uninstall.
- P2: the regulation is built by the setup's own Rust builder (regbuild); WitchyBND only as an optional fallback.
- P3: incremental updates (only changed steps rerun), settings merge (your hot settings kept), Roll back, the co-op id
  notice ("both players must install X"), an opt-in release check (off; no repository configured).
- Release tooling: `make_release.ps1` with the payload lint (no game files, converted data, saves, Seamless, PC paths)
  and a self-test install into a temporary root.

## 0.1.x - 2026-10-13 to 2026-10-22 (private test builds, summarized from MODLOG)

- **M0 / M1**: toolchain; vanilla me3 launch; reading the Steam x64 MW2 zones (the iw4x-x64/oat Unlinker): weapons,
  models, sounds, animations.
- **M2**: MW2 gunplay in Elden Ring (the first three guns): hitscan, damage, recoil, sounds; in-game PASS.
- **M3 / M3b**: the MW2 viewmodel drawn by our own D3D12 renderer, then a real first-person camera (erfps2 research);
  first person accepted after run 7.
- **M4**: the AC-130 kill streak (thermal view, 25 / 40 / 105 mm, explosions, object destruction).
- **M5**: MW2 movement with sprinting. **M6**: the 34 MW2 multiplayer guns. **M7**: the Tactical Nuke. **M8**:
  mantling and a ladder audit.
- **Co-op C0-C6** (Seamless Co-op 2.0.1 through me3, private sessions): peer detection, the co-op nuke on owned enemies,
  seeing each other's tracers / sounds / explosions (both not yet played with two players), controller support, ER's
  messages over the gun (C4.1 injection, C5 cut-out), heal key, the third-person hand hold with the polearm carry, the guard as third-person ADS, scope peek and
  the over-the-shoulder camera.
- **Port** to eldenring.exe 2.7.2.0 (the 2026-10-20 update).
- **Installer P0**: the double-click zip package (Install.cmd / Play co-op.cmd / Play solo.cmd) for the first friend.
