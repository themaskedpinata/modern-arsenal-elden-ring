<#
.SYNOPSIS
  Builds the one-file release MW2inER-<version>-setup.exe (docs\INSTALLER_DESIGN.md 5.1, phase P1).

.DESCRIPTION
  1. Preconditions: the pinned official files in work\installer\cache (CPython 3.14.7 embeddable zip, me3 0.13.0
     installer + licences) match the pins compiled into the installer (payload.rs PYTHON_EMBED_SHA256, release.rs
     ME3_INSTALLER_SHA256); the host's build\package was made by -RegulationBuilder (build\rows.json `generator`:
     "regbuild ..." = the Rust builder, the default since P2; "build_regulation.py" = WitchyBND); only with
     -RegulationBuilder witchybnd: the WitchyBND zip for the self-test matches host_pins.
  2. cargo build --release -p installer (sndconv too when its exe is missing), run from mod\ so mod\.cargo\config.toml
     applies (static CRT, path remapping).
  3. Stage the payload: tools\coop_friend\make_friend_package.ps1 -OutDir <stage> -NoZip (the very file list of the
     friend package: DLL, release toml under the friend policy, scripts, Unlinker + GPL material, sndconv, P0 .cmd /
     .ps1 fallback, docs, manifest\host_manifest.json + host_pins.json; dist\MW2inER-coop-friend is NOT touched), then
     add runtime\python-3.14.7-embed-amd64.zip, tools\me3\me3_installer.exe + LICENSE-MIT / LICENSE-APACHE,
     licenses\python, licenses\rust (the crates in mw2er.dll / the setup / sndconv) and the third-party notes; P4: the
     mod's LICENSE (MIT) and THIRD_PARTY_NOTICES.md (release_tool.py notices) at the payload root, also copied next to
     the exe; RELEASE_NOTES.md = docs\release_notes\<version>.md + the generated release facts.
  4. manifest\release.json = host manifest + pins (+ me3 / CPython pins, + P2: regulation_builder and the
     builder-independent package_bnd4_sha256 / rows_json_core_sha256 from `MW2inER-setup.exe --cli regulation-info`, which
     the installer's WitchyBND fallback is verified against) + pinned sources + sha256 of every payload file + co-op id
     (release_tool.py manifest); P3: -UpdateRepo owner/name adds release.json `update_repo` (the installer's opt-in GitHub
     "newer release" check; empty by default = no check, the host sends new setups).
  5. Lint (release_tool.py lint; fails the release; P4 review: plus `binary-lint` of the built setup in step 2 and of
     the packed exe in step 6 - no path / user name of this PC in ASCII or UTF-16, no VC++ runtime imports): deny names / folders / magic (game files, converted data,
     regulation, saves, Seamless, ini, passwords), allow-list with the origin of every file (byte-identical to the
     repository file or to a pinned official file), paths / user name / Steam ids of this PC, sizes, manifests hashes only.
  6. Pack: a deterministic payload zip is appended to the built exe with a 64-byte trailer (payload.rs overlay) ->
     <OutDir>\MW2inER-<version>-setup.exe (+ .sha256, RELEASE_NOTES.md, release.json copy); read back and re-hashed.
     Embedded, not alongside: one file for the player (budget: fails above 64 MB).
  7. Self-test (unless -SkipSelfTest): the new exe --cli status / install / verify / uninstall into a temporary root
     with THIS PC's real games read-only: no --payload (the embedded release is used), no --python-zip (the bundled
     CPython), no WitchyBND (the regulation is built in; only -RegulationBuilder witchybnd passes --witchy-zip <local
     pinned zip>), --no-register --no-shortcuts --no-backup. It runs the MW2 dumps
     (Unlinker), the converters and the regulation build like work\friend_selftest_2720.log. It starts no game, no
     Steam, no Seamless, no me3 (the install never runs me3; `play` is not called). Before / after snapshots (names,
     sizes, modification times; read-only) of the Elden Ring and MW2 folders, %APPDATA%\EldenRing, the Desktop, the
     Start-menu folder and the me3 folder, plus HKCU\Software\MW2inER and the uninstall key, must be unchanged.
     -SelfTestMode coop also copies this PC's Seamless (found by the installer) and writes a random throwaway password
     (never printed) into the temporary root's own ersc_settings.ini copy.

  Read-only on the project except <OutDir>, the self-test root (removed by the uninstall) and the self-test log.
  Never writes dist\MW2inER-coop-friend. Never launches Elden Ring, MW2, Steam, Seamless or me3.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_release.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_release.ps1 -SelfTestMode coop
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_release.ps1 -SkipSelfTest
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_release.ps1 -RegulationBuilder witchybnd

  The host's build\package with the Rust builder (before a regbuild release; it replaces the host's regulation, so the
  package sha256 and the co-op id change once - host and friend update together):
  mod\target\release\regbuild.exe build --root . --vanilla work\regulation\vanilla\regulation.bin
      --out build\package\regulation.bin --rows build\rows.json
  Every workspace version bump needs this rebuild (with the newly built regbuild.exe): rows.json records
  generator "regbuild <version>", which the release manifest pins, so step 1 requires it to equal the workspace version.
#>
param(
    [string]$OutDir,                 # default dist\release\<version>
    [switch]$NoBuild,                # use mod\target\release\MW2inER-setup.exe as it is
    [switch]$SkipSelfTest,
    [ValidateSet('solo', 'coop')][string]$SelfTestMode = 'solo',
    [string]$SelfTestRoot,           # default work\installer\selftest_release
    [string]$WitchyZip,              # default tools\WitchyBND\WitchyBND-3.0.1.0-win-x64.zip (self-test, witchybnd only)
    # P2: the builder the host's build\package was made with (= the release's; the installer defaults to it)
    [ValidateSet('regbuild', 'witchybnd')][string]$RegulationBuilder = 'regbuild',
    # P3: owner/name of the GitHub repository the installer's opt-in "newer release" check asks (empty = no check;
    # the friend's releases leave it empty: the host sends the new setup, decision D6)
    [string]$UpdateRepo = ''
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Tool = Join-Path $Root 'tools\scripts\release_tool.py'
$Cache = Join-Path $Root 'work\installer\cache'
function Sha([string]$p) { (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant() }
function Step([string]$t) { Write-Output ''; Write-Output "== $t" }
function Fail([string]$t) { Write-Output "FAIL $t"; exit 1 }
# Runs a native program (stderr merged, printed live, PowerShell 5.1 safe); returns { Code, Lines }.
function Invoke-Native([string]$Exe, [string[]]$ArgList, [string]$StdIn = $null, [string]$Log = $null) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($null -ne $StdIn -and $StdIn -ne '') {
            $lines = @($StdIn | & $Exe @ArgList 2>&1 | ForEach-Object { $s = "$_"; Write-Host "     $s"; $s })
        } else {
            $lines = @(& $Exe @ArgList 2>&1 | ForEach-Object { $s = "$_"; Write-Host "     $s"; $s })
        }
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $old }
    if ($Log) { Add-Content -LiteralPath $Log -Value (@("> $([IO.Path]::GetFileName($Exe)) $($ArgList -join ' ')") + $lines + @("exit $code")) -Encoding UTF8 }
    [pscustomobject]@{ Code = $code; Lines = $lines }
}
function PyTool([string[]]$ArgList, [string]$Log = $null) { Invoke-Native $Py (@('-I', $Tool) + $ArgList) $null $Log }

# ---- 0. version, pins ------------------------------------------------------------------------------------------
$ws = Get-Content -LiteralPath (Join-Path $Root 'mod\Cargo.toml') -Raw
if ($ws -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { Fail 'no [workspace.package] version in mod\Cargo.toml' }
$Version = $Matches[1]
$src = Join-Path $Root 'mod\installer\src'
$pyPin = [regex]::Match((Get-Content -LiteralPath (Join-Path $src 'payload.rs') -Raw), 'PYTHON_EMBED_SHA256: &str = "([0-9a-f]{64})"').Groups[1].Value
$pyName = [regex]::Match((Get-Content -LiteralPath (Join-Path $src 'payload.rs') -Raw), 'PYTHON_EMBED_NAME: &str = "([^"]+)"').Groups[1].Value
$me3Pin = [regex]::Match((Get-Content -LiteralPath (Join-Path $src 'release.rs') -Raw), 'ME3_INSTALLER_SHA256: &str = "([0-9a-f]{64})"').Groups[1].Value
if (-not $pyPin -or -not $me3Pin -or -not $pyName) { Fail 'cannot read the CPython / me3 pins from payload.rs / release.rs' }
if (-not $OutDir) { $OutDir = Join-Path $Root "dist\release\$Version" }
$OutDir = [IO.Path]::GetFullPath($OutDir)
$Stage = Join-Path $OutDir 'stage\MW2inER'
$ExeOut = Join-Path $OutDir "MW2inER-$Version-setup.exe"
Write-Output "Modern Arsenal for Elden Ring (MW2inER) release $Version -> $ExeOut"

Step '1. preconditions'
$pyZip = Join-Path $Cache "python-3.14.7-embed-amd64\$pyName"
$pyLic = Join-Path $Cache 'python-3.14.7-embed-amd64\LICENSE.txt'
$me3Exe = Join-Path $Cache 'me3-0.13.0\me3_installer.exe'
foreach ($f in $pyZip, $pyLic, $me3Exe, (Join-Path $Cache 'me3-0.13.0\LICENSE-MIT'), (Join-Path $Cache 'me3-0.13.0\LICENSE-APACHE'), $Py) {
    if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { Fail "$f is missing (see work\installer\notes.md)" }
}
if ((Sha $pyZip) -ne $pyPin) { Fail "$pyZip is not the pinned official CPython embeddable package" }
if ((Sha $me3Exe) -ne $me3Pin) { Fail "$me3Exe is not the pinned official me3 0.13.0 installer" }
Write-Output "OK   CPython $pyName == pin $($pyPin.Substring(0, 12)); me3_installer.exe == pin $($me3Pin.Substring(0, 12))"
if (-not (Test-Path -LiteralPath (Join-Path $Root 'build\natives\mw2er.dll'))) { Fail 'build\natives\mw2er.dll is not staged' }
$hostRows = Get-Content -LiteralPath (Join-Path $Root 'build\rows.json') -Raw | ConvertFrom-Json
$hostBuilder = if ("$($hostRows.generator)" -like 'regbuild*') { 'regbuild' } else { 'witchybnd' }
if ($hostBuilder -ne $RegulationBuilder) {
    Fail ("build\package was made with $hostBuilder (rows.json generator '$($hostRows.generator)'), not $RegulationBuilder. " +
        'Rebuild it with the chosen builder first (main session; see the header), or pass -RegulationBuilder ' + $hostBuilder)
}
# The setup's built-in builder writes rows.json generator "regbuild <workspace version>" and the release manifest's
# rows.json entry drops only built_utc (coop_manifest.py VOLATILE): an older regbuild's package would fail every
# regbuild install's verify (strict, same builder) although regulation.bin is byte-identical.
if ($RegulationBuilder -eq 'regbuild' -and "$($hostRows.generator)" -ne "regbuild $Version") {
    Fail ("build\rows.json generator is '$($hostRows.generator)', not 'regbuild $Version': rebuild build\package with " +
        "mod\target\release\regbuild.exe (version $Version; main session; see the header)")
}
Write-Output "OK   build\package made with $hostBuilder (package regulation $("$($hostRows.package_regulation_sha256)".Substring(0, 12)))"
Write-Output 'NOTE check_c3 / check_c4 / check_c6 are the main session''s gate before a release (not run here)'

Step '2. build'
$setup = Join-Path $Root 'mod\target\release\MW2inER-setup.exe'
if (-not $NoBuild) {
    $pk = @('build', '--release', '-p', 'installer', '--manifest-path', (Join-Path $Root 'mod\Cargo.toml'))
    if (-not (Test-Path -LiteralPath (Join-Path $Root 'mod\target\release\sndconv.exe'))) { $pk += @('-p', 'sndconv') }
    # P4 review: cargo reads .cargo\config.toml from the CURRENT folder (not the manifest's): from mod\ only do the
    # static CRT, --remap-path-prefix and /d1trimfile of mod\.cargo\config.toml apply (no VC++ runtime imports, no
    # path of this PC in the setup's panic strings)
    Push-Location -LiteralPath (Join-Path $Root 'mod')
    try { $r = Invoke-Native 'cargo' $pk } finally { Pop-Location }
    if ($r.Code -ne 0) { Fail 'cargo build' }
}
if (-not (Test-Path -LiteralPath $setup)) { Fail "$setup missing" }
$pv = (Get-Item -LiteralPath $setup).VersionInfo.ProductVersion
if ($pv -ne $Version) { Fail "$setup has ProductVersion $pv (workspace $Version): rebuild" }
Write-Output ("OK   {0} {1:N1} MB, version {2}" -f $setup, ((Get-Item $setup).Length / 1MB), $pv)
# P4 review: the setup itself is scanned as bytes (the payload lint sees only the staged files): no path / user name
# of this PC (ASCII + UTF-16) and no VC++ runtime imports
$r = PyTool @('binary-lint', $setup)
if ($r.Code -ne 0) { Fail "$setup leaks a path of this PC or needs the VC++ runtime: build it from mod\ (nothing was packed)" }
# P2: the builder-independent regulation pins of the host's build (read only; the installer's own reader)
$r = Invoke-Native $setup @('--cli', 'regulation-info', '--root', $Root)
$ri = @{}
foreach ($l in $r.Lines) { if ($l -match '^(\w+)=(.*)$') { $ri[$Matches[1]] = $Matches[2] } }
if ($r.Code -ne 0 -or $ri['regulation_builder'] -ne $RegulationBuilder -or $ri['package_regulation_sha256'] -ne "$($hostRows.package_regulation_sha256)" -or
    $ri['package_bnd4_sha256'] -notmatch '^[0-9a-f]{64}$' -or $ri['rows_json_core_sha256'] -notmatch '^[0-9a-f]{64}$') {
    Fail 'regulation-info of the host build failed or does not match build\rows.json'
}
Write-Output "OK   regulation pins: builder $RegulationBuilder, BND4 $($ri['package_bnd4_sha256'].Substring(0, 12)), rows core $($ri['rows_json_core_sha256'].Substring(0, 12))"

Step '3. stage the payload'
if (Test-Path -LiteralPath $OutDir) {
    if (-not (Test-Path -LiteralPath (Join-Path $OutDir 'stage')) -and @(Get-ChildItem -LiteralPath $OutDir -Force).Count -gt 0) { Fail "$OutDir is not an earlier release folder: refused" }
    Remove-Item -LiteralPath $OutDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
& (Join-Path $Root 'tools\coop_friend\make_friend_package.ps1') -OutDir $Stage -NoZip
if (-not (Test-Path -LiteralPath (Join-Path $Stage 'manifest\host_pins.json'))) { Fail 'make_friend_package -OutDir did not stage the package' }
foreach ($d in 'runtime', 'tools\me3', 'licenses\python', 'licenses\rust') { New-Item -ItemType Directory -Force -Path (Join-Path $Stage $d) | Out-Null }
Copy-Item -LiteralPath $pyZip -Destination (Join-Path $Stage "runtime\$pyName")
Copy-Item -LiteralPath $pyLic -Destination (Join-Path $Stage 'licenses\python\LICENSE.txt')
Copy-Item -LiteralPath $me3Exe -Destination (Join-Path $Stage 'tools\me3\me3_installer.exe')
foreach ($f in 'LICENSE-MIT', 'LICENSE-APACHE') { Copy-Item -LiteralPath (Join-Path $Cache "me3-0.13.0\$f") -Destination (Join-Path $Stage "tools\me3\$f") }
$me3src = @"
# me3 0.13.0 installer (tools\me3\me3_installer.exe)

The official, unmodified installer of me3 (the mod loader, https://github.com/garyttierney/me3), from the release asset
https://github.com/garyttierney/me3/releases/download/v0.13.0/me3_installer.exe, sha256 $me3Pin (the digest GitHub
publishes for that asset; Authenticode-signed by SignPath Foundation). Licence: MIT OR Apache-2.0 (LICENSE-MIT,
LICENSE-APACHE here, from the v0.13.0 tag). The setup runs it only when you click "Install me3 0.13.0", after
checking its sha256 again. me3 installs per user and has its own uninstaller.
"@
[IO.File]::WriteAllText((Join-Path $Stage 'tools\me3\SOURCE.md'), ($me3src -replace "`r`n", "`n") + "`n")
$r = PyTool @('licenses', (Join-Path $Root 'mod\Cargo.toml'), (Join-Path $Stage 'licenses\rust'), '--roots', 'installer', 'mw2er', 'sndconv')
if ($r.Code -ne 0) { Fail 'licences of the Rust crates' }
# P4: the mod's own licence (MIT) at the payload root; THIRD_PARTY_NOTICES.md is written after THIRD_PARTY.md below
Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination (Join-Path $Stage 'LICENSE')
$witchyPin = (Get-Content -LiteralPath (Join-Path $Stage 'manifest\host_pins.json') -Raw | ConvertFrom-Json).witchybnd_zip_sha256
$witchyDl = 'https://github.com/ividyon/WitchyBND/releases/download/3.0.1.0/WitchyBND-3.0.1.0-win-x64.zip'
if ($RegulationBuilder -eq 'regbuild') {
    $witchyNote = "GPL-3.0, https://github.com/ividyon/WitchyBND. Not shipped and not needed: the setup builds the regulation itself
(its own Rust code, mod\regbuild; zstd from the zstd crate / libzstd 1.5.7, BSD; aes / cbc, MIT OR Apache-2.0). Only its
fallback (--regulation-tool witchy, or Advanced in the window) uses WitchyBND: it downloads the official
WitchyBND-3.0.1.0-win-x64.zip from $witchyDl
only after you click (sha256 $witchyPin; any other file is deleted), or you choose the zip you already have."
} else {
    $witchyNote = "GPL-3.0, https://github.com/ividyon/WitchyBND. Not shipped: the setup downloads the official
WitchyBND-3.0.1.0-win-x64.zip from $witchyDl
only after you click (sha256 $witchyPin; any other file is deleted), or you choose the zip you already have."
}
$third = @"

# The setup program (MW2inER-$Version-setup.exe, installed as MW2inER.exe) and mw2er.dll

Our own Rust code (MIT, LICENSE; Copyright (c) 2026 maskedpinata). The Rust crates compiled into mw2er.dll, the setup program and sndconv.exe are
listed in licenses\rust\README.md; their licence texts are in licenses\rust\LICENSE-TEXTS.txt. The setup's window
uses egui / eframe with its bundled fonts (OFL-1.1, Ubuntu Font Licence; texts included there).

# CPython 3.14.7 embeddable package (runtime\$pyName)

The official, unmodified Windows x64 embeddable package from https://www.python.org/ftp/python/3.14.7/$pyName,
sha256 $pyPin (the value python.org publishes). Licence: PSF License Agreement and the licences of the third-party
code it contains (licenses\python\LICENSE.txt, also LICENSE.txt inside the zip). The setup unpacks it into
<install folder>\runtime\python only when its sha256 equals that pin; the converters and the regulation build run on it.

# me3 0.13.0 (tools\me3\)

The official, unmodified me3 installer; see tools\me3\SOURCE.md. MIT OR Apache-2.0.

# WitchyBND 3.0.1.0 (NOT included)

$witchyNote

# Seamless Co-op (NOT included)

Never shipped, never downloaded by the setup: you download it yourself from Nexus Mods; the setup copies your own
download into the install folder.
"@
[IO.File]::AppendAllText((Join-Path $Stage 'tools\THIRD_PARTY.md'), ($third -replace "`r`n", "`n") + "`n")
$r = PyTool @('notices', $Stage, '--version', $Version)
if ($r.Code -ne 0) { Fail 'THIRD_PARTY_NOTICES.md' }
Write-Output "OK   staged $Stage"

Step '4. release manifest'
$pins = @('--pin', "me3_installer_sha256=$me3Pin", '--pin', "python_embed_sha256=$pyPin")
$regPins = @('--pin', "package_bnd4_sha256=$($ri['package_bnd4_sha256'])", '--pin', "rows_json_core_sha256=$($ri['rows_json_core_sha256'])",
    '--note', "regulation_builder=$RegulationBuilder")
$repoArg = @()
if ($UpdateRepo) { $repoArg = @('--update-repo', $UpdateRepo) }
$r = PyTool (@('manifest', $Stage, '--version', $Version) + $pins + $regPins + $repoArg)
if ($r.Code -ne 0) { Fail 'release.json' }
$rel = Get-Content -LiteralPath (Join-Path $Stage 'manifest\release.json') -Raw | ConvertFrom-Json
$coopId = $rel.coop_id

Step '5. lint'
$lintLog = Join-Path $OutDir 'lint.log'
$r = PyTool (@('lint', $Stage) + $pins) $lintLog
if ($r.Code -ne 0) { Fail "the release lint found problems (see $lintLog): nothing was packed" }

Step '6. pack'
$zip = Join-Path $OutDir 'stage\payload.zip'
$r = PyTool @('zip', $Stage, $zip)
if ($r.Code -ne 0) { Fail 'payload zip' }
$r = PyTool @('pack', $setup, $zip, $ExeOut)
if ($r.Code -ne 0) { Fail 'pack' }
$r = PyTool @('check-pack', $ExeOut)
if ($r.Code -ne 0) { Fail 'the packed exe does not read back' }
$r = PyTool @('binary-lint', $ExeOut)
if ($r.Code -ne 0) { Fail 'the packed exe fails the binary lint' }
$exeMb = (Get-Item -LiteralPath $ExeOut).Length / 1MB
if ($exeMb -gt 64) { Fail ("{0:N1} MB is over the 64 MB budget" -f $exeMb) }
$exeSha = Sha $ExeOut
[IO.File]::WriteAllText("$ExeOut.sha256", "$exeSha  MW2inER-$Version-setup.exe`n")
Copy-Item -LiteralPath (Join-Path $Stage 'manifest\release.json') -Destination (Join-Path $OutDir 'release.json')
$p = $rel.pins
foreach ($f in 'LICENSE', 'THIRD_PARTY_NOTICES.md') { Copy-Item -LiteralPath (Join-Path $Stage $f) -Destination (Join-Path $OutDir $f) }
# P4: the hand-written notes of this version (docs\release_notes\<version>.md: what it adds, requirements, known
# issues) come first; the release facts below are generated
$hand = Join-Path $Root "docs\release_notes\$Version.md"
$handText = if (Test-Path -LiteralPath $hand) { (Get-Content -LiteralPath $hand -Raw -Encoding UTF8).TrimEnd() + "`n`n## Release facts`n" } else { "# Modern Arsenal for Elden Ring $Version`n" }
$notes = @"

- File: MW2inER-$Version-setup.exe ({0:N1} MB), sha256 $exeSha
- Co-op id: $coopId (both players must show the same id)
- Needs: Elden Ring $($p.er_exe_version) ($($p.er_app_version)), Call of Duty: Modern Warfare 2 (2009) Steam build
  $($p.mw2_buildid), me3 $($p.me3_version) (the setup can install the bundled official installer), and for co-op Seamless
  Co-op $($p.seamless_version) downloaded from Nexus Mods by each player.
- mw2er.dll sha256 $($p.mw2er_dll_sha256); package regulation sha256 $($p.package_regulation_sha256)
  (built by $($p.regulation_builder); the setup builds the same file from your own regulation.bin).
- The regulation builder decides the package sha256 and so the co-op id: host and friend install the same release.
- Contains no game files, converted data, regulation, saves or Seamless: the setup builds the mod data from your own
  games and checks every file against the release manifest. See INSTALL_GUIDE.md / README.md inside.
"@ -f $exeMb
[IO.File]::WriteAllText((Join-Path $OutDir 'RELEASE_NOTES.md'), (($handText + "`n" + $notes.TrimStart()) -replace "`r`n", "`n") + "`n")
Write-Output ("OK   {0} {1:N1} MB sha256 {2}; co-op id {3}" -f $ExeOut, $exeMb, $exeSha, $coopId)

if ($SkipSelfTest) {
    Write-Output ''
    Write-Output "RELEASE BUILT (self-test skipped): $ExeOut"
    exit 0
}

# ---- 7. self-test ----------------------------------------------------------------------------------------------
Step "7. self-test ($SelfTestMode): install + verify + uninstall with this PC's games (read-only)"
if (-not $SelfTestRoot) { $SelfTestRoot = Join-Path $Root 'work\installer\selftest_release' }
$SelfTestRoot = [IO.Path]::GetFullPath($SelfTestRoot)
$witchyArgs = @()
if ($RegulationBuilder -eq 'witchybnd') {
    if (-not $WitchyZip) { $WitchyZip = Join-Path $Root 'tools\WitchyBND\WitchyBND-3.0.1.0-win-x64.zip' }
    if (-not (Test-Path -LiteralPath $WitchyZip) -or (Sha $WitchyZip) -ne $witchyPin) { Fail "$WitchyZip is missing or not the pinned WitchyBND zip" }
    $witchyArgs = @('--witchy-zip', $WitchyZip)
}
if (Test-Path -LiteralPath $SelfTestRoot) {
    if (-not (Test-Path -LiteralPath (Join-Path $SelfTestRoot 'state\install.json'))) { Fail "$SelfTestRoot exists and is not an earlier self-test install: refused" }
    Remove-Item -LiteralPath $SelfTestRoot -Recurse -Force
}
$Log = Join-Path $Root "work\installer\release_selftest_${Version}_$SelfTestMode.log"
[IO.File]::WriteAllText($Log, "MW2inER release $Version self-test ($SelfTestMode) $(Get-Date -Format s)`r`nexe $ExeOut sha256 $exeSha`r`n")
# the folders the install must not change (read-only listing)
$steam = (Get-ItemProperty -Path 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamPath
$libs = @()
if ($steam) {
    $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
    $libs += $steam
    if (Test-Path -LiteralPath $vdf) { $libs += [regex]::Matches((Get-Content -LiteralPath $vdf -Raw), '"path"\s+"([^"]+)"') | ForEach-Object { $_.Groups[1].Value -replace '\\\\', '\' } }
}
$er = $libs | ForEach-Object { Join-Path $_ 'steamapps\common\ELDEN RING' } | Where-Object { Test-Path -LiteralPath (Join-Path $_ 'Game\eldenring.exe') } | Select-Object -First 1
$mw2 = $libs | ForEach-Object { Join-Path $_ 'steamapps\common\Call of Duty Modern Warfare 2' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $er -or -not $mw2) { Fail 'Elden Ring / MW2 not found in the Steam libraries' }
$watch = @($er, $mw2, (Join-Path $env:APPDATA 'EldenRing'), [Environment]::GetFolderPath('Desktop'),
    (Join-Path ([Environment]::GetFolderPath('Programs')) 'MW2 in Elden Ring'), (Join-Path ([Environment]::GetFolderPath('Programs')) 'Modern Arsenal for Elden Ring'),
    (Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3'))
$regKeys = @('HKCU:\Software\MW2inER', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MW2inER')
function RegState { ($regKeys | ForEach-Object { "$_=$(Test-Path $_)" }) -join '; ' }
# the listings name files of this PC (Desktop): kept under work\, never in the release folder
$before = Join-Path $Root "work\installer\release_selftest_${Version}_before.json"
$after = Join-Path $Root "work\installer\release_selftest_${Version}_after.json"
$regBefore = RegState
$r = PyTool (@('snapshot', $before) + $watch) $Log
if ($r.Code -ne 0) { Fail 'snapshot' }
$common = @('--root', $SelfTestRoot, '--mode', $SelfTestMode)
$t0 = Get-Date
# status: reads only; the release must be the embedded one and its co-op id the manifest's
$r = Invoke-Native $ExeOut (@('--cli', 'status') + $common + $witchyArgs + @('--no-backup')) $null $Log
$stLine = $r.Lines | Where-Object { $_ -match '^release co-op id: (\w+)' } | Select-Object -First 1
if ($r.Code -ne 0 -or $stLine -notmatch $coopId) { Fail "status: not ready or co-op id differs from release.json ($coopId)" }
$wantTool = if ($RegulationBuilder -eq 'regbuild') { '^regulation builder: built in \(regbuild\)$' } else { '^regulation builder: WitchyBND' }
if (-not ($r.Lines -match $wantTool)) { Fail "status: the regulation builder is not $RegulationBuilder" }
if ($RegulationBuilder -eq 'regbuild' -and ($r.Lines -match '^(OK  |WARN|FAIL) WitchyBND')) { Fail 'status: a WitchyBND row with the built-in regulation builder' }
if (-not ($r.Lines -match '^release files:.*MW2inER-payload')) { Fail 'status did not use the release embedded in the exe' }
if (Test-Path -LiteralPath $SelfTestRoot) { Fail 'status wrote the root' }
$pw = $null
$inst = @('--cli', 'install') + $common + $witchyArgs + @('--no-register', '--no-shortcuts', '--no-backup')
if ($SelfTestMode -eq 'coop') { $inst += '--password-stdin'; $pw = [guid]::NewGuid().ToString('N') }
$r = Invoke-Native $ExeOut $inst $pw $Log
$pw = $null
if ($r.Code -ne 0 -or -not ($r.Lines -match "INSTALL PASSED: co-op id $coopId")) { Fail "install (log $Log)" }
$installed = Join-Path $SelfTestRoot 'MW2inER.exe'
if ((Sha $installed) -ne (Sha $setup)) { Fail 'the installed MW2inER.exe is not the program without its payload' }
if ($RegulationBuilder -eq 'regbuild' -and (Test-Path -LiteralPath (Join-Path $SelfTestRoot 'tools\WitchyBND\app'))) { Fail 'WitchyBND was unpacked with the built-in regulation builder' }
if ((Sha (Join-Path $SelfTestRoot 'build\package\regulation.bin')) -ne $rel.pins.package_regulation_sha256) { Fail 'the installed regulation.bin is not the release package' }
$r = Invoke-Native $installed @('--cli', 'verify', '--root', $SelfTestRoot) $null $Log
$vl = ($r.Lines | Where-Object { $_ -match 'coop_manifest.py: PASS all: (\d+)/(\d+)' } | Select-Object -First 1)
if ($r.Code -ne 0 -or -not $vl -or -not ($r.Lines -match "co-op id $coopId")) { Fail 'verify of the installed root' }
$verified = [regex]::Match($vl, '(\d+/\d+)').Groups[1].Value
if ($SelfTestMode -eq 'coop') {
    $ini = Join-Path $SelfTestRoot 'seamless\SeamlessCoop\ersc_settings.ini'
    if (-not (Test-Path -LiteralPath $ini)) { Fail 'co-op: no ersc_settings.ini copy in the root' }
}
$r = Invoke-Native $ExeOut @('--cli', 'uninstall', '--root', $SelfTestRoot, '--yes') $null $Log
if ($r.Code -ne 0 -or -not ($r.Lines -match '^UNINSTALLED')) { Fail 'uninstall' }
if (Test-Path -LiteralPath $SelfTestRoot) {
    $left = @(Get-ChildItem -LiteralPath $SelfTestRoot -Recurse -File)
    if ($left.Count -gt 0) { Fail "uninstall left $($left.Count) file(s) in $SelfTestRoot" }
}
$r = PyTool (@('snapshot', $after) + $watch) $Log
$r = PyTool @('compare', $before, $after) $Log
if ($r.Code -ne 0) { Fail 'a watched folder changed during the self-test' }
$regAfter = RegState
if ($regAfter -ne $regBefore) { Fail "registry changed: $regBefore -> $regAfter" }
# the payload the setup unpacked into %TEMP% (its own cache; removed so nothing stays behind)
$stageTmp = Join-Path $env:TEMP ('MW2inER-payload\' + (Sha $zip).Substring(0, 16))
if (Test-Path -LiteralPath $stageTmp) { Remove-Item -LiteralPath $stageTmp -Recurse -Force }
$stageParent = Split-Path -Parent $stageTmp
if ((Test-Path -LiteralPath $stageParent) -and @(Get-ChildItem -LiteralPath $stageParent -Force).Count -eq 0) { Remove-Item -LiteralPath $stageParent -Force }
$secs = [int]((Get-Date) - $t0).TotalSeconds
$summary = "SELF-TEST PASSED ($SelfTestMode, regulation $RegulationBuilder): embedded release installed, verify $verified, co-op id $coopId, uninstalled; game folders, saves, Desktop, Start menu, me3 and registry unchanged; $secs s"
Add-Content -LiteralPath $Log -Value $summary -Encoding UTF8
Write-Output ''
Write-Output $summary
Write-Output "RELEASE BUILT: $ExeOut (sha256 $exeSha; log $Log)"
exit 0
