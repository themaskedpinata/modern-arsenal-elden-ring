<#
.SYNOPSIS
  Builds dist\MW2inER-coop-friend\ (+ .zip next to it): everything a friend needs for co-op C2, WITHOUT any game data.
  Ships: the staged mw2er.dll, our converter / regulation scripts, the Unlinker (GPL-3.0, with its licence and source
  reference), sndconv.exe, the hash manifest of this PC's results (hashes only), host pins, setup / launch scripts
  (Install.cmd, Play co-op.cmd, Play solo.cmd = the double-click flow), docs, and the host's mw2er.toml under the friend
  policy of friend_toml.ps1. Never ships: converted\, dumps\, regulation.bin, saves, the Seamless folder or its ini.
  Read-only on the project except dist\ and the manifest it writes there. Never launches anything.
.PARAMETER OutDir
  Stage the same files into this folder instead of dist\MW2inER-coop-friend (tools\scripts\make_release.ps1 uses it
  for the installer payload; dist\MW2inER-coop-friend and its zip are then left untouched). The folder must be new,
  empty or an earlier stage (it has manifest\host_pins.json); it is cleared first.
.PARAMETER NoZip
  Do not write the .zip next to the package.
#>
param(
    [string]$OutDir,
    [switch]$NoZip
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Src = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Dist = Join-Path $Root 'dist'
$Pkg = Join-Path $Dist 'MW2inER-coop-friend'
$Zip = Join-Path $Dist 'MW2inER-coop-friend.zip'
if ($OutDir) {
    $Pkg = [IO.Path]::GetFullPath($OutDir)
    $Zip = $Pkg.TrimEnd('\') + '.zip'
    if ((Test-Path -LiteralPath $Pkg) -and @(Get-ChildItem -LiteralPath $Pkg -Force).Count -gt 0 -and -not (Test-Path -LiteralPath (Join-Path $Pkg 'manifest\host_pins.json'))) {
        throw "-OutDir $Pkg is not empty and is not an earlier package stage: refused"
    }
}
function Sha([string]$p) { (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant() }

if (Test-Path -LiteralPath $Pkg) { Remove-Item -LiteralPath $Pkg -Recurse -Force }
if (-not $NoZip -and (Test-Path -LiteralPath $Zip)) { Remove-Item -LiteralPath $Zip -Force }
foreach ($d in @('build\natives', 'profiles', 'manifest', 'tools\scripts', 'tools\oat\bin', 'tools\bin', 'tools\WitchyBND')) { New-Item -ItemType Directory -Force -Path (Join-Path $Pkg $d) | Out-Null }

# 1. the exact DLL (same sha256 as build\natives\mw2er.dll)
$dll = Join-Path $Root 'build\natives\mw2er.dll'
Copy-Item -LiteralPath $dll -Destination (Join-Path $Pkg 'build\natives\mw2er.dll')

# 2. scripts (ours, no game data)
$scripts = 'convert_weapons.py', 'copy_sounds.py', 'convert_viewmodel.py', 'vm_format.py', 'convert_ac130.py', 'convert_movement.py',
           'convert_arsenal.py', 'inventory_m6.py', 'convert_nuke.py', 'xanim_read.py', 'check_assets.py', 'build_regulation.py',
           'param_diff.py', 'param_rows.py', 'conpty_run.py', 'friend_dump.py', 'coop_manifest.py'
foreach ($s in $scripts) { Copy-Item -LiteralPath (Join-Path $Root "tools\scripts\$s") -Destination (Join-Path $Pkg "tools\scripts\$s") }

# 3. tools: the Unlinker (GPL-3.0; it needs no data files: the raw folder is for the Linker only), sndconv (ours, static CRT)
$oat = Join-Path $Root 'tools\oat'
Copy-Item -LiteralPath (Join-Path $oat 'build\bin\Release_x64\Unlinker.exe') -Destination (Join-Path $Pkg 'tools\oat\bin\Unlinker.exe')
Copy-Item -LiteralPath (Join-Path $oat 'LICENSE') -Destination (Join-Path $Pkg 'tools\oat\LICENSE')
$commit = (& git -C $oat rev-parse HEAD).Trim()
$diff = (& git -C $oat diff --no-color) -join "`n"
[IO.File]::WriteAllText((Join-Path $Pkg 'tools\oat\local-build-fixes.patch'), $diff + "`n")
Copy-Item -LiteralPath (Join-Path $Root 'mod\target\release\sndconv.exe') -Destination (Join-Path $Pkg 'tools\bin\sndconv.exe')
$srcmd = @"
# OpenAssetTools Unlinker (tools\oat\bin\Unlinker.exe)

- Licence: GPL-3.0 (tools\oat\LICENSE). This package runs it as a separate program; no OAT code is linked into the mod.
- Source: https://github.com/iw4x-x64/oat at commit $commit (the IW4MS = MW2 2009 x64 fork of OpenAssetTools),
  plus 2 local build fixes for MSVC 14.44 in tools\oat\local-build-fixes.patch (InfoString.h: drop one ``constexpr``;
  EfxReader.cpp: one ``static_cast<unsigned>``). No behaviour change.
- Build: ``premake5 vs2022`` (premake 5.0.0-beta8, fetched by the repository's own scripts), then msbuild of the whole
  generated solution, Release x64. The binary carries compile-time source paths of the host's build folder in its
  libtomcrypt assert strings (no user name).
- Third-party code inside the binary: see the oat repository's thirdparty\ folder (zlib, libtomcrypt, libtommath,
  minilzo, lz4, salsa20, json, eigen, stb ...), each under its own licence.
- To get the corresponding source: ``git clone https://github.com/iw4x-x64/oat && git -C oat checkout $commit &&
  git -C oat submodule update --init --recursive && git -C oat apply ..\local-build-fixes.patch``.

# sndconv (tools\bin\sndconv.exe)

Our own offline MP3 -> WAV tool (mod\sndconv of MW2 in Elden Ring), used once by convert_ac130.py for one ambient
sound of YOUR MW2. Built with Rust (static CRT) on symphonia 0.5.5 (MPL-2.0, https://github.com/pdeljanov/Symphonia).
"@
[IO.File]::WriteAllText((Join-Path $Pkg 'tools\THIRD_PARTY.md'), $srcmd)
[IO.File]::WriteAllText((Join-Path $Pkg 'tools\WitchyBND\PUT_NOTHING_HERE.txt'), "setup.ps1 unzips YOUR WitchyBND-3.0.1.0-win-x64.zip (from https://github.com/ividyon/WitchyBND/releases/tag/v3.0.1.0) into app\ here.`r`n")

# 4. config: the host's mw2er.toml under the friend policy (friend_toml.ps1, MODLOG 2026-10-22 installer P0): every test
#    probe / experiment forced off (probes, ER-depth occlusion, the crossbow carrier, always_available), the in-game-tested
#    features required on (C4.1 injection, C5 cut-out, heal, gamepad, C6 hand hold + polearm stance + guard ADS + guard yaw,
#    scope peek, weapon hide, body occlusion, the shoulder camera); a host toml without them refuses the package.
. (Join-Path $Src 'friend_toml.ps1')
$toml = Get-Content -LiteralPath (Join-Path $Root 'mw2er.toml') -Raw
$toml2 = ConvertTo-FriendToml $toml
foreach ($c in $script:FriendTomlChanges) { Write-Output "     friend toml: $c" }
[IO.File]::WriteAllText((Join-Path $Pkg 'mw2er.toml'), $toml2)

# 5. scripts and docs of the package (the .txt guide is regenerated from the .md)
& $Py -I (Join-Path $Src 'md2txt.py') (Join-Path $Src 'INSTALL_GUIDE.md') (Join-Path $Src 'INSTALL_GUIDE.txt')
if ($LASTEXITCODE -ne 0) { throw 'md2txt' }
# Install.cmd / Play co-op.cmd / Play solo.cmd = the double-click flow (installer P0); setup_ui.ps1 = its dialogs
foreach ($f in 'Install.cmd', 'Play co-op.cmd', 'Play solo.cmd', 'setup.ps1', 'setup_ui.ps1', 'launch_coop.ps1', 'launch_solo.ps1', 'README.md', 'INSTALL_GUIDE.md', 'INSTALL_GUIDE.txt') { Copy-Item -LiteralPath (Join-Path $Src $f) -Destination (Join-Path $Pkg $f) }
foreach ($f in 'mw2er-coop.template.me3', 'mw2er-solo.template.me3') { Copy-Item -LiteralPath (Join-Path $Src $f) -Destination (Join-Path $Pkg "profiles\$f") }

# 6. hashes only: this PC's converted data + regulation package + rows.json + DLL, and the version pins
& $Py -I (Join-Path $Root 'tools\scripts\coop_manifest.py') write (Join-Path $Pkg 'manifest\host_manifest.json')
if ($LASTEXITCODE -ne 0) { throw 'manifest' }
$rows = Get-Content -LiteralPath (Join-Path $Root 'build\rows.json') -Raw | ConvertFrom-Json
# P2 (docs\INSTALLER_DESIGN.md P2): the P0 zip's setup.ps1 still builds the regulation with WitchyBND, so its manifest
# must come from a WitchyBND build; the installer release (-OutDir, make_release.ps1) takes either builder.
$gen = "$($rows.generator)"
if (-not $OutDir -and $gen -notlike 'build_regulation.py*') {
    throw "build\rows.json was made by '$gen': the P0 friend zip (setup.ps1 builds with WitchyBND) needs a WitchyBND-built build\package (tools\scripts\build_regulation.py); the installer release is made by tools\scripts\make_release.ps1"
}
$wzip = Join-Path $Root 'tools\WitchyBND\WitchyBND-3.0.1.0-win-x64.zip'
$wexe = Join-Path $Root 'tools\WitchyBND\app\WitchyBND.exe'
$haveWitchy = (Test-Path -LiteralPath $wzip) -and (Test-Path -LiteralPath $wexe)
if ($haveWitchy) {
    $paramdex = [IO.File]::ReadAllText((Join-Path $Root 'tools\WitchyBND\app\Assets\Paramdex\ER\Upgrader\version.txt')).Trim()
    if ($paramdex -ne '1_17_3_0034L') { throw "Paramdex upgrader version.txt is '$paramdex' (want 1_17_3_0034L, the 2.7.2.0 regulation)" }
} elseif ($OutDir) {
    # the installer release with the built-in regulation builder needs no WitchyBND; without it on this PC the release
    # carries no WitchyBND pins (its --regulation-tool witchy fallback then has nothing to download)
    Write-Output 'NOTE tools\WitchyBND is not on this PC: no WitchyBND pins (the fallback cannot be used with this release)'
    $paramdex = '1_17_3_0034L'
} else {
    throw 'tools\WitchyBND (zip + app) is needed for the P0 friend package'
}
$exeVer = (Get-Item 'D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe').VersionInfo.FileVersion
if ($exeVer -ne '2.7.2.0') { throw "eldenring.exe is $exeVer (the package pins 2.7.2.0)" }
$pins = [ordered]@{
    what = 'versions and sha256 the host (the person who sent this package) has; setup.ps1 compares yours with these'
    er_exe_version = $exeVer
    er_app_version = 'Steam build 25827938, the 2026-10-20 update'   # port 2.7.2.0 (1.17.1 / exe 2.7.1.0 before)
    vanilla_regulation_sha256 = $rows.vanilla_sha256
    package_regulation_sha256 = $rows.package_regulation_sha256
    mw2_buildid = '25374375'
    iw4sp_sha256 = (Sha 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2\iw4sp.exe')
    me3_version = (Get-Item (Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3\bin\me3.exe')).VersionInfo.ProductVersion
    seamless_version = '2.0.1'
    ersc_sha256 = (Sha 'C:\Users\Player\Documents\seamless\SeamlessCoop\ersc.dll')
    witchybnd_zip_sha256 = $(if ($haveWitchy) { Sha $wzip } else { '' })
    witchybnd_exe_sha256 = $(if ($haveWitchy) { Sha $wexe } else { '' })
    paramdex_version = $paramdex   # the host's own bumped upgrader version (port 2.7.2.0: 1_17_3_0034L)
    mw2er_dll_sha256 = (Sha $dll)
    unlinker_sha256 = (Sha (Join-Path $oat 'build\bin\Release_x64\Unlinker.exe'))
    oat_commit = $commit
}
[IO.File]::WriteAllText((Join-Path $Pkg 'manifest\host_pins.json'), (($pins | ConvertTo-Json) -replace "`r`n", "`n") + "`n")

# 7. zip
$n = @(Get-ChildItem -LiteralPath $Pkg -Recurse -File).Count
$size = (Get-ChildItem -LiteralPath $Pkg -Recurse -File | Measure-Object Length -Sum).Sum
if ($NoZip) {
    Write-Output ("PASS package {0}: {1} files, {2:N1} MB (no zip)" -f $Pkg, $n, ($size / 1MB))
} else {
    Compress-Archive -Path $Pkg -DestinationPath $Zip -CompressionLevel Optimal
    Write-Output ("PASS package {0}: {1} files, {2:N1} MB; zip {3:N1} MB sha256 {4}" -f $Pkg, $n, ($size / 1MB), ((Get-Item $Zip).Length / 1MB), (Sha $Zip))
}
Write-Output ("     mw2er.dll sha256 {0} (== build\natives\mw2er.dll)" -f (Sha (Join-Path $Pkg 'build\natives\mw2er.dll')))
