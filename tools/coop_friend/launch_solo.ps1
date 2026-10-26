<#
.SYNOPSIS
  Starts Elden Ring with MW2 in Elden Ring ALONE (no Seamless) through me3, with the solo profile setup.ps1 wrote
  (own save ER0000.mw2er.sl2: the first start makes a new character; your normal ER0000.sl2 is never used).
  Same launch as the host's solo launch: me3 launch -g eldenring -p <profile> (no --no-mem-patch: that is only for
  Seamless). Steam must be running. "Play solo.cmd" and the desktop shortcut "MW2 in Elden Ring (solo)" run this.
  -NoSteamCheck skips the "is Steam running" check.
#>
param([switch]$NoSteamCheck)
$Root = $PSScriptRoot
$prof = Join-Path $Root 'profiles\mw2er-solo.me3'
if (-not (Test-Path -LiteralPath $prof)) { Write-Host 'profiles\mw2er-solo.me3 missing: run setup first (double-click Install.cmd)' -ForegroundColor Red; exit 1 }
foreach ($f in @('build\natives\mw2er.dll', 'build\package\regulation.bin', 'converted\weapons.json', 'mw2er.toml')) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root $f))) { Write-Host "$f missing: run setup first (double-click Install.cmd)" -ForegroundColor Red; exit 1 }
}
$me3 = Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3\bin\me3.exe'
if (-not (Test-Path -LiteralPath $me3)) { $c = Get-Command me3.exe -ErrorAction SilentlyContinue; if ($c) { $me3 = $c.Source } }
if (-not (Test-Path -LiteralPath $me3)) { Write-Host 'me3 not found (install me3 0.13.0)' -ForegroundColor Red; exit 1 }
$text = Get-Content -LiteralPath $prof -Raw
if ($text -match '(?m)^\s*["'']?(start_online|disable_arxan)["'']?\s*=') { Write-Host 'the profile must not set start_online / disable_arxan' -ForegroundColor Red; exit 1 }
if ($text -notmatch '(?m)^\s*savefile\s*=\s*"ER0000\.mw2er\.sl2"') { Write-Host 'the solo profile must set savefile = "ER0000.mw2er.sl2" (separate save)' -ForegroundColor Red; exit 1 }
if ($text -match '(?i)ersc\.dll') { Write-Host 'the solo profile must not load Seamless (ersc.dll)' -ForegroundColor Red; exit 1 }
if (-not $NoSteamCheck -and -not (Get-Process -Name 'steam' -ErrorAction SilentlyContinue)) { Write-Host 'Steam is not running: start Steam (and log in) first, then start this again.' -ForegroundColor Red; exit 1 }
Write-Host "Starting: $me3 launch -g eldenring -p `"$prof`""
& $me3 launch -g eldenring -p $prof
exit $LASTEXITCODE
