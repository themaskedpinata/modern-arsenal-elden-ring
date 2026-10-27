# Modern Arsenal for Elden Ring

Source code of **Modern Arsenal for Elden Ring**, an unofficial fan mod that brings modern firearms to Elden Ring:
34 guns with aim-down-sights and scopes, first or third person, sprint and mantling, an AC-130 gunship and a tactical
nuke, solo or in private Seamless Co-op.

- **Download and install:** https://www.nexusmods.com/eldenring/mods/11487
- **Showcase video:** https://www.youtube.com/watch?v=yhb4IFz8X7o
- **Discord** (help, bug reports, co-op partners): https://discord.gg/BHSB9S2ZmK

**No game files are in this repository or in the release.** The setup builds the mod's data on each player's PC from
their own Steam copies of Elden Ring and Call of Duty: Modern Warfare 2 (2009).

Not affiliated with, endorsed by or sponsored by Activision, Infinity Ward, FromSoftware or Bandai Namco. Offline and
private Seamless Co-op only; never use it in Elden Ring's official online mode.

## What is here

| Folder | Contents |
|---|---|
| `mod/mw2er` | `mw2er.dll`, the mod loaded into Elden Ring by me3 (hooks, guns, movement, HUD, AC-130, nuke, co-op) |
| `mod/mw2er-core` | game-independent logic and its unit tests (`cargo test -p mw2er-core`) |
| `mod/mw2er-render` | the D3D12 gun viewmodel renderer |
| `mod/installer` | `MW2inER-setup.exe`: the setup wizard, launcher, updater and uninstaller |
| `mod/regbuild` | the regulation builder (AES / DCX / BND4 / PARAM), used by the setup |
| `mod/sndconv` | converts MW2 sounds during the setup |
| `mod/vmtest`, `mod/movebench` | offline test harnesses (renderer, movement) |
| `tools/scripts` | the Python converters run by the setup (MW2 weapons, viewmodels, animations, sounds, AC-130, nuke), the release and check scripts |
| `tools/coop_friend` | the files staged into the release next to the setup (profiles, fallback scripts, install guide) |
| `tools/patches` | the patches applied to fromsoftware-rs and hudhook |
| `vendor/hudhook-0.9.3` | hudhook 0.9.3 with this mod's two patches |
| `docs` | design notes |

Building: see [BUILD.md](BUILD.md).

## Licence

The mod's own code is MIT-licensed (see [LICENSE](LICENSE), Copyright (c) 2026 maskedpinata). Third-party components
keep their own licences: me3 (MIT or Apache-2.0), CPython (PSF License), the OpenAssetTools Unlinker (GPL-3.0), hudhook
and fromsoftware-rs (MIT / Apache-2.0) and the Rust crates listed in each release's THIRD_PARTY_NOTICES.md.

## AI disclosure

This mod was built with Claude Code, Anthropic's coding agent, with the author (maskedpinata) directing the work and
testing it in the game.
