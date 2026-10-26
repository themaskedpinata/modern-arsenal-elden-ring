<#
Exports the public source tree of Modern Arsenal for Elden Ring into -OutDir (default D:\Mods\modern-arsenal-src).

Copies only the mod's own source, build scripts, patches, docs and test fixtures. Never copies game files, converted
data, dumps, logs, saves, backups, cargo targets, the vendored fromsoftware-rs checkout (tools\fetch_vendor.ps1 rebuilds it
from upstream + tools\patches), the OpenAssetTools / WitchyBND binaries, Seamless Co-op, or the dev log (MODLOG.md).
Replaces this PC's user folder in the copied text with a placeholder and writes a generic mod\.cargo\config.toml.
Ends with a scan that fails on personal data or game-file types left in the export. Keeps an existing .git folder.
#>
param([string]$OutDir = 'D:\Mods\modern-arsenal-src')
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$UserName = Split-Path -Leaf $env:USERPROFILE

New-Item -ItemType Directory -Force $OutDir | Out-Null
Get-ChildItem $OutDir -Force | Where-Object Name -notin '.git', 'vendor' | Remove-Item -Recurse -Force
# keep the fetched (git-ignored) vendor\fromsoftware-rs and the mod\target cache out of the wipe
Get-ChildItem (Join-Path $OutDir 'vendor') -Force -ErrorAction SilentlyContinue | Where-Object Name -ne 'fromsoftware-rs' | Remove-Item -Recurse -Force

$xd = @('target', '__pycache__', '.git', 'build', 'out', (Join-Path $PSScriptRoot 'public'))
$dirs = @('mod', 'tools\scripts', 'tools\patches', 'tools\iw4dis', 'tools\erscan', 'tools\coop_friend', 'vendor\hudhook-0.9.3', 'docs')
foreach ($d in $dirs) {
    robocopy (Join-Path $Root $d) (Join-Path $OutDir $d) /E /NFL /NDL /NJH /NJS /NP /XD @xd /XF '*.pyc' '*.log.bak' | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed for $d ($LASTEXITCODE)" }
}
foreach ($f in 'LICENSE', 'CHANGELOG.md') { Copy-Item (Join-Path $Root $f) (Join-Path $OutDir $f) }
# mw2er.toml as the release ships it: the host's toml under the release policy (test probes off), not the host's test values
. (Join-Path $Root 'tools\coop_friend\friend_toml.ps1')
[IO.File]::WriteAllText((Join-Path $OutDir 'mw2er.toml'), (ConvertTo-FriendToml ([IO.File]::ReadAllText((Join-Path $Root 'mw2er.toml')))))
Copy-Item (Join-Path $PSScriptRoot 'public\*') $OutDir -Recurse -Force

# Generic cargo config: same flags, no path of this PC (build.ps1 adds the path remaps for the current user).
@'
# Static CRT: mw2er.dll must not import VCRUNTIME140 / api-ms-win-crt-* (checked by m2_static_checks.py pe).
# The release builds (build.ps1) also remap the cargo registry and repository paths so no local path is baked into the
# binaries; a plain `cargo build` works without them.
[target.x86_64-pc-windows-msvc]
rustflags = [
    "-C", "target-feature=+crt-static",
    "-C", "link-arg=/PDBALTPATH:%_PDB%",
]
'@ | Set-Content -Encoding utf8 (Join-Path $OutDir 'mod\.cargo\config.toml')

# Placeholder for this PC's user name in copied text (test fixtures, path guards).
$text = '.rs', '.py', '.ps1', '.md', '.txt', '.toml', '.json', '.log', '.cmd', '.me3', '.yml'
Get-ChildItem $OutDir -Recurse -File -Force | Where-Object { $_.FullName -notmatch '\\\.git\\' -and $_.Extension -in $text } | ForEach-Object {
    $s = [IO.File]::ReadAllText($_.FullName)
    if ($s.Contains($UserName)) { [IO.File]::WriteAllText($_.FullName, $s.Replace($UserName, 'Player')) }
}

# Final scan: personal data and game-file types must not be in the export.
$bad = @()
$files = Get-ChildItem $OutDir -Recurse -File -Force | Where-Object { $_.FullName -notmatch '\\\.git\\' }
$bad += $files | Where-Object { $_.Extension -in '.ff', '.iwd', '.iwi', '.bin', '.dcx', '.bdt', '.bhd', '.sl2', '.dll', '.exe', '.zip', '.xanim', '.xmodel' } | ForEach-Object { "game/binary file: $($_.FullName)" }
$bad += $files | Where-Object { $_.Name -eq 'ersc_settings.ini' } | ForEach-Object { "Seamless settings (password): $($_.FullName)" }
# The author's mail address, assembled so this script does not contain it.
$mail = 'carson' + 'daniels'
$pats = @([regex]::Escape($UserName), $mail, '7656119(?!8000000001)\d{10}')
foreach ($p in $pats) {
    $files | Where-Object { $_.Extension -in $text } | Select-String -Pattern $p | ForEach-Object { $bad += "personal data /$p/: $($_.Path):$($_.LineNumber)" }
}
if ($bad) { $bad | ForEach-Object { Write-Host "FAIL $_" }; throw "export scan failed: $($bad.Count) problem(s)" }
$sum = ($files | Measure-Object Length -Sum)
"PASS export: $($sum.Count) files, {0:N1} MB -> $OutDir" -f ($sum.Sum / 1MB)
exit 0
