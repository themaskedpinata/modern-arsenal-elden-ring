<#
.SYNOPSIS
  Builds the Nexus Mods upload files of a release: dist\release\<version>\nexus\ (P4). Uploads nothing.

.DESCRIPTION
  Needs the finished release folder of make_release.ps1 (MW2inER-<version>-setup.exe + .sha256, release.json, LICENSE,
  THIRD_PARTY_NOTICES.md, stage\). Writes only <release>\nexus\ (rebuilt on every run; refused if it holds files this
  script did not make):
  1. release_tool.py nexus: the upload archive <Display-Name>-<version>.zip (deterministic: fixed times, sorted; one
     folder holding the setup, README.txt (docs\nexus\README.md), LICENSE, THIRD_PARTY_NOTICES.md, CHANGELOG.md), the
     drafts NEXUS_PAGE.md + NEXUS_PAGE.bbcode, NEXUS_FORM.md, MEDIA_CHECKLIST.md, CLEAN_PC_TEST.md (docs\nexus\
     templates filled from release.json and the hashes), SHA256SUMS.txt and, unless -NoOatSource, the GPL-3.0
     corresponding source of the shipped OpenAssetTools Unlinker (git archive of the LOCAL tools\oat clone at the
     shipped commit + submodules + local-build-fixes.patch; nothing is downloaded) for an optional Nexus file.
  2. release_tool.py nexus-lint (fails the step; log nexus\nexus_lint.log): the upload layout, the setup == the
     release's, licence / notices / changelog byte-identical to their sources, no PC paths / user name / Steam ids /
     passwords / placeholders, summary <= 350 characters, the page's sections, disclaimer, credits and AI disclosure,
     BBCode balanced and generated from the Markdown, link hosts.
  3. `um publish check` (the universal-modder lint, when the um CLI is on PATH; log nexus\um_publish_check.log) on the
     unpacked archive and on the release payload, with the Elden Ring and MW2 folders (read-only) to catch copied
     game files.
  Never launches a game, Steam, me3 or Seamless; never writes a game folder; never uploads or posts.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_nexus_package.ps1
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\scripts\make_nexus_package.ps1 -NoOatSource
#>
param(
    [string]$ReleaseDir,             # default dist\release\<workspace version>
    [switch]$NoOatSource,            # skip the GPL source zip of the Unlinker
    [switch]$NoUm                    # skip `um publish check`
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Tool = Join-Path $Root 'tools\scripts\release_tool.py'
function Step([string]$t) { Write-Output ''; Write-Output "== $t" }
function Fail([string]$t) { Write-Output "FAIL $t"; exit 1 }
function Invoke-Native([string]$Exe, [string[]]$ArgList, [string]$Log = $null) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = @(& $Exe @ArgList 2>&1 | ForEach-Object { $s = "$_"; Write-Host "     $s"; $s })
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $old }
    if ($Log) { Add-Content -LiteralPath $Log -Value (@("> $([IO.Path]::GetFileName($Exe)) $($ArgList -join ' ')") + $lines + @("exit $code")) -Encoding UTF8 }
    [pscustomobject]@{ Code = $code; Lines = $lines }
}

$ws = Get-Content -LiteralPath (Join-Path $Root 'mod\Cargo.toml') -Raw
if ($ws -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { Fail 'no [workspace.package] version in mod\Cargo.toml' }
$Version = $Matches[1]
if (-not $ReleaseDir) { $ReleaseDir = Join-Path $Root "dist\release\$Version" }
$ReleaseDir = [IO.Path]::GetFullPath($ReleaseDir)
$Out = Join-Path $ReleaseDir 'nexus'
if (-not (Test-Path -LiteralPath $Py)) { Fail "$Py is missing" }
Write-Output "Nexus upload files of release $Version -> $Out (nothing is uploaded)"

Step '1. archive + drafts'
$a = @('-I', $Tool, 'nexus', $ReleaseDir, '--version', $Version)
if (-not $NoOatSource) { $a += '--oat-source' }
$r = Invoke-Native $Py $a
if ($r.Code -ne 0) { Fail 'release_tool.py nexus' }

Step '2. nexus lint'
$lintLog = Join-Path $Out 'nexus_lint.log'
$r = Invoke-Native $Py @('-I', $Tool, 'nexus-lint', $ReleaseDir, '--version', $Version) $lintLog
if ($r.Code -ne 0) { Fail "the nexus lint found problems (see $lintLog)" }

Step '3. um publish check'
$um = Get-Command um -ErrorAction SilentlyContinue
if ($NoUm -or -not $um) {
    Write-Output 'SKIP um publish check (-NoUm, or the um CLI is not on PATH)'
} else {
    $umLog = Join-Path $Out 'um_publish_check.log'
    $steam = (Get-ItemProperty -Path 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamPath
    $libs = @()
    if ($steam) {
        $libs += $steam
        $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
        if (Test-Path -LiteralPath $vdf) { $libs += [regex]::Matches((Get-Content -LiteralPath $vdf -Raw), '"path"\s+"([^"]+)"') | ForEach-Object { $_.Groups[1].Value -replace '\\\\', '\' } }
    }
    $er = $libs | ForEach-Object { Join-Path $_ 'steamapps\common\ELDEN RING\Game' } | Where-Object { Test-Path -LiteralPath (Join-Path $_ 'eldenring.exe') } | Select-Object -First 1
    $mw2 = $libs | ForEach-Object { Join-Path $_ 'steamapps\common\Call of Duty Modern Warfare 2' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    $folder = @(Get-ChildItem -LiteralPath (Join-Path $Out 'stage') -Directory)[0].FullName
    $payload = Join-Path $ReleaseDir 'stage\MW2inER'
    $games = @($er, $mw2) | Where-Object { $_ }
    if ($games.Count -lt 2) { Write-Output 'NOTE Elden Ring / MW2 not found: the game-file comparison is partial' }
    $bad = 0
    foreach ($target in @($folder, $payload)) {
        if (-not (Test-Path -LiteralPath $target)) { Write-Output "SKIP $target (missing)"; continue }
        foreach ($g in $games) {
            $r = Invoke-Native $um.Source @('publish', 'check', $target, '--game', $g) $umLog
            if ($r.Code -ne 0) { $bad++ }
        }
    }
    # the log would name this PC's folders: replace them by placeholders (the release folder never holds PC paths)
    if (Test-Path -LiteralPath $umLog) {
        $clean = Get-Content -LiteralPath $umLog -Encoding UTF8 | ForEach-Object {
            $l = $_
            if ($er) { $l = $l -replace [regex]::Escape($er), '<Elden Ring>\Game' }
            if ($mw2) { $l = $l -replace [regex]::Escape($mw2), '<MW2>' }
            $l = $l -replace [regex]::Escape($Root), '<repo>'
            $l -replace [regex]::Escape((Split-Path -Parent $um.Source)), '<um>'
        }
        [IO.File]::WriteAllText($umLog, (($clean -join "`n") + "`n"))
    }
    if ($bad -gt 0) { Fail "um publish check failed (see $umLog)" }
}

$sums = Get-Content -LiteralPath (Join-Path $Out 'SHA256SUMS.txt')
Write-Output ''
Write-Output "NEXUS FILES READY (not uploaded): $Out"
$sums | ForEach-Object { Write-Output "  $_" }
exit 0
