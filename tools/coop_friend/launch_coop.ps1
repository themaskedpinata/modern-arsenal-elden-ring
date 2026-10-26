<#
.SYNOPSIS
  Starts Elden Ring with Seamless Co-op + MW2 in Elden Ring through me3 (the profile setup.ps1 wrote).
  Steam must be running. Never use the Seamless launcher (ersc_launcher.exe) - it would load Seamless twice.
  --no-mem-patch is required: me3's memory patch crashes Seamless 2.0.1 about 8 seconds after start.
  "Play co-op.cmd" (and the desktop shortcut "MW2 in Elden Ring (co-op)") run this script.
  -NoSteamCheck skips the "is Steam running" check.
#>
param([switch]$NoSteamCheck)
$Root = $PSScriptRoot
$prof = Join-Path $Root 'profiles\mw2er-coop.me3'
if (-not (Test-Path -LiteralPath $prof)) { Write-Host 'profiles\mw2er-coop.me3 missing: run setup first (double-click Install.cmd)' -ForegroundColor Red; exit 1 }
foreach ($f in @('build\natives\mw2er.dll', 'build\package\regulation.bin', 'converted\weapons.json', 'mw2er.toml')) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root $f))) { Write-Host "$f missing: run setup first (double-click Install.cmd)" -ForegroundColor Red; exit 1 }
}
$me3 = Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3\bin\me3.exe'
if (-not (Test-Path -LiteralPath $me3)) { $c = Get-Command me3.exe -ErrorAction SilentlyContinue; if ($c) { $me3 = $c.Source } }
if (-not (Test-Path -LiteralPath $me3)) { Write-Host 'me3 not found (install me3 0.13.0)' -ForegroundColor Red; exit 1 }
$text = Get-Content -LiteralPath $prof -Raw
if ($text -match '(?m)^\s*["'']?(start_online|disable_arxan)["'']?\s*=') { Write-Host 'the profile must not set start_online / disable_arxan' -ForegroundColor Red; exit 1 }
if (-not $NoSteamCheck -and -not (Get-Process -Name 'steam' -ErrorAction SilentlyContinue)) { Write-Host 'Steam is not running: start Steam (and log in) first, then start this again.' -ForegroundColor Red; exit 1 }
Write-Host "Starting: $me3 launch -g eldenring -p `"$prof`" --no-mem-patch"
& $me3 launch -g eldenring -p $prof --no-mem-patch
exit $LASTEXITCODE
