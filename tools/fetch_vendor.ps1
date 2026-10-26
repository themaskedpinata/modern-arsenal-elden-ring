<#
Recreates vendor\fromsoftware-rs: the upstream repository at the pinned commit plus this mod's patches
(tools\patches\fromsoftware-rs-*.patch). Needs git. Does nothing when the patched checkout already exists.
#>
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Dir = Join-Path $Root 'vendor\fromsoftware-rs'
$Commit = '59fbd3b3b7daaf14aca47c9f73530493dba6bc79'
$Done = Join-Path $Dir '.mw2er-patched'
# git writes progress to stderr, which Windows PowerShell 5.1 turns into errors under 'Stop'
function Invoke-Git([string[]]$ArgList) {
    $ErrorActionPreference = 'Continue'
    & git.exe @ArgList 2>&1 | ForEach-Object { "  $_" }
    if ($LASTEXITCODE) { throw "git $($ArgList -join ' ') failed ($LASTEXITCODE)" }
}
if (Test-Path $Done) { "vendor\fromsoftware-rs already present"; return }
if (Test-Path $Dir) { Remove-Item -Recurse -Force $Dir }
Invoke-Git @('clone', '--quiet', 'https://github.com/vswarte/fromsoftware-rs', $Dir)
Invoke-Git @('-C', $Dir, 'checkout', '--quiet', $Commit)
foreach ($p in Get-ChildItem (Join-Path $Root 'tools\patches\fromsoftware-rs-*.patch') | Sort-Object Name) {
    Invoke-Git @('-C', $Dir, 'apply', '--whitespace=nowarn', $p.FullName)
    "applied $($p.Name)"
}
Set-Content -LiteralPath $Done -Value $Commit
"vendor\fromsoftware-rs ready ($Commit + patches)"

