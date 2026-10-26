# Nexus Mods upload form: values for {{DISPLAY_NAME}} {{VERSION}}

A draft for the author. Nothing was uploaded, posted or published. Fill the form yourself on nexusmods.com (Elden
Ring, "Add a mod") and press the buttons yourself. Field names follow the current form as far as known; if a field is
named differently, use the closest one.

## 1. Mod details

| Field | Value |
|---|---|
| Game | Elden Ring |
| Mod name | **{{DISPLAY_NAME}}** |
| Alternative names (if you prefer) | Modern Warfare Arsenal for Elden Ring (note: "Modern Warfare" is an Activision trademark; the primary name avoids it); Modern Firearms and Killstreaks for Elden Ring |
| Language | English |
| Version | {{VERSION}} |
| Created by / Author | maskedpinata |
| Category | **Gameplay** (second choice: Weapons, if the dropdown offers it) |
| Adult content | No |
| Summary ({{SUMMARY_LEN}} of 350 characters) | see the block below |
| Description | paste **NEXUS_PAGE.bbcode** (the same text as NEXUS_PAGE.md) |

Summary (paste exactly):

<!-- summary:begin -->
{{SUMMARY}}
<!-- summary:end -->

Tags to suggest after publishing (Nexus lets players and authors vote tags; pick what the list offers): Gameplay,
Weapons, Guns, Firearms, First Person, Co-op / Multiplayer, Overhaul, Controller support, **AI-generated content**
(see section 6).

## 2. Media

Your own in-game screenshots and video only (see MEDIA_CHECKLIST.md). No Activision / Call of Duty logos, box art, key
art or marketing images, and no screenshots of MW2 itself. Primary image: an in-game shot of the Lands Between with a
gun in first person.

## 3. Files

Main file:

| Field | Value |
|---|---|
| File to upload | **{{ZIP_NAME}}** ({{ZIP_MB}} MB, sha256 {{ZIP_SHA256}}) |
| File name | {{DISPLAY_NAME}} setup |
| Version | {{VERSION}} |
| Category | Main files |
| Mod manager download button | **remove / disable it** (manual download only: this is a standalone setup program, not files for Vortex) |
| Description | see the block below |

File description (paste exactly; {{FILE_DESC_LEN}} characters):

<!-- file-description:begin -->
{{FILE_DESCRIPTION}}
<!-- file-description:end -->

Optional file (recommended, for the GPL-3.0 of the OpenAssetTools Unlinker that the setup carries):

| Field | Value |
|---|---|
| File to upload | **{{OAT_ZIP_NAME}}** {{OAT_ZIP_FACTS}} |
| File name | OpenAssetTools Unlinker source (GPL-3.0) |
| Version | {{VERSION}} |
| Category | Optional files (or Miscellaneous files) |
| Mod manager download button | remove / disable it |
| Description | The corresponding source of the OpenAssetTools Unlinker (GPL-3.0) included in the {{VERSION}} setup: iw4x-x64/oat commit {{OAT_COMMIT_SHORT}} with its submodules and the two local build fixes. Not needed to play. |

Uploading it next to the binary keeps the source available from the same place even if the GitHub repository goes
away (THIRD_PARTY_NOTICES.md describes it).

## 4. Requirements

| Kind | Entry | Note to show |
|---|---|---|
| Off-site requirement | Call of Duty: Modern Warfare 2 (2009) on Steam: https://store.steampowered.com/app/10180/ | Must be installed: the setup builds the mod's data from your own copy. Not Campaign Remastered, not Modern Warfare II (2022). |
| Off-site requirement | me3 {{ME3_VERSION}}: https://github.com/garyttierney/me3 | Bundled: the setup carries the official installer. |
| Off-site requirement | Microsoft Visual C++ 2015-2022 x64 runtime: https://aka.ms/vs/17/release/vc_redist.x64.exe | Usually present. The bundled MW2 file reader needs it; the setup checks for it and links this installer if it is missing. |
| Nexus requirement (optional) | Seamless Co-op (Elden Ring mod 510) | For co-op only, version {{SEAMLESS_VERSION}}; each player downloads it. Not needed for solo. |
| DLC requirement | none | Shadow of the Erdtree works; in co-op both players need the same DLC. |

Before saving, open the MW2 store link once and check that it is the 2009 game (Steam app 10180; its multiplayer
is app 10190 and installs into the same game folder, which is all the setup needs).

## 5. Permissions and credits

| Field | Value | Why |
|---|---|---|
| Credits / file credits | me3 (garyttierney), Seamless Co-op (LukeYui, separate download), OpenAssetTools (Laupetin, iw4x-x64/oat), WitchyBND (ividyon, optional fallback, not included), fromsoftware-rs (vswarte et al.), hudhook (veeenu), erfps2 (Dasaav-dsv), Paramdex / SoulsFormats community, IW4L (vladtrc) | as in NEXUS_PAGE |
| Upload permission | You can upload this file to other sites but you must credit me as the creator of the file | MIT allows redistribution with the notice. Author notes ask for a link to this page, because co-op needs identical releases |
| Modification permission | You are allowed to modify my files and release bug fixes or improve on the features so long as you credit me as the original creator | MIT |
| Conversion permission | You can convert this file to work with other games as long as you credit me as the creator of the file | MIT |
| Asset use permission | You are allowed to use the assets in this file without permission as long as you credit me | MIT, own code only |
| Asset use in mods / files that are being sold | You are allowed to use the assets in this file in mods/files that are being sold, for money, on Steam Workshop or other platforms | MIT permits it; choosing "not allowed" would contradict the LICENSE (the LICENSE wins either way) |
| Asset use in mods / files that earn donation points | You are allowed to earn Donation Points for your mods if they use my assets | MIT |
| Donation Points for this mod | **Author's choice; suggestion: opt out** | The mod depends on another company's game content (built on the player's PC). Not earning money from it keeps the fan-mod position clean. Not legal advice |

Author notes (paste):

> The mod's own code is MIT-licensed (LICENSE, Copyright (c) 2026 maskedpinata); third-party components keep their
> own licences (THIRD_PARTY_NOTICES.md). **No assets from other games are included**: the download contains only the
> setup program and its documentation; everything taken from Elden Ring and Call of Duty: Modern Warfare 2 (2009) is
> built on each player's PC from their own copies and cannot be redistributed by this permission. If you re-upload or
> fork it, please link to this page: co-op needs identical releases on both PCs.

## 6. AI content

| Field | Value |
|---|---|
| Contains AI-generated content (flag / tag) | **Yes** |
| What | Code, converters and documentation were written by Claude Code (Anthropic's coding agent) under the author's direction; the author directed and tested it in the game. No AI-generated images, models, sounds or voices. |

The description's "AI disclosure" section says the same.

## 7. Before you press Publish

- [ ] The mod name is the primary one (or your chosen alternative) everywhere: page, file name.
- [ ] Summary pasted exactly ({{SUMMARY_LEN}} characters, limit 350).
- [ ] Description pasted from NEXUS_PAGE.bbcode; preview looks right (headings, lists, links).
- [ ] Main file {{ZIP_NAME}} uploaded, sha256 checked, mod manager button removed.
- [ ] Optional GPL source file uploaded (recommended).
- [ ] Requirements, permissions, AI flag and Donation Points choice set.
- [ ] Images: own screenshots only, no logos or key art.
- [ ] CLEAN_PC_TEST.md passed on a second PC (or you accept publishing without it).
