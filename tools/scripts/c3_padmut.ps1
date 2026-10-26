<#
.SYNOPSIS
  C3 PASS B mutation check of the gamepad unit tests (docs\C3_DESIGN.md 4.7): "swap yaw/pitch, drop latch, wrong graph
  index must fail".

.DESCRIPTION
  Windows PowerShell 5.1. Copies mod\mw2er-core (sources only) into work\check_c3\padmut-<PID>\ (review C3: a folder
  per run, so two concurrent or an interrupted run never share a mutated copy; the build output is shared in
  work\check_c3\padmut_target, which cargo locks), builds it as a one-member workspace with the project's Cargo.lock
  (--offline: no download), asserts the copied pad_logic.rs equals the source byte for byte, runs the c3_pad_ unit tests
  unmutated (must pass), then once per mutation of pad_logic.rs (each must make at least one c3_pad_ test fail). The copy
  is restored and removed in a finally block. The project sources are never modified. Never launches anything else.
  Prints "PASS c3_padmut: 3/3 mutations caught" and exits 0 on success.
#>
[CmdletBinding()]
param()

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Mod = Join-Path $Root 'mod'
$Work = Join-Path $Root ("work\check_c3\padmut-$PID")
$Core = Join-Path $Work 'mw2er-core'
$env:CARGO_TARGET_DIR = Join-Path $Root 'work\check_c3\padmut_target'
# folders of earlier runs whose process is gone (an interrupted run) are removed
Get-ChildItem -LiteralPath (Join-Path $Root 'work\check_c3') -Directory -Filter 'padmut-*' -ErrorAction SilentlyContinue | ForEach-Object {
    $old = 0
    if ([int]::TryParse($_.Name.Substring(7), [ref]$old) -and $old -ne $PID -and -not (Get-Process -Id $old -ErrorAction SilentlyContinue)) {
        Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# a fresh copy of the crate's sources
if (Test-Path -LiteralPath $Core) { Remove-Item -LiteralPath $Core -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Core | Out-Null
Copy-Item -LiteralPath (Join-Path $Mod 'mw2er-core\src') -Destination $Core -Recurse
Copy-Item -LiteralPath (Join-Path $Mod 'mw2er-core\Cargo.toml') -Destination $Core
# Copy-Item keeps the source mtimes: stamp the copy now, or cargo would reuse a mutated build of an earlier run
Get-ChildItem -LiteralPath $Core -Recurse -File | ForEach-Object { $_.LastWriteTime = Get-Date }
Copy-Item -LiteralPath (Join-Path $Mod 'Cargo.lock') -Destination $Work -Force
$ws = Get-Content -LiteralPath (Join-Path $Mod 'Cargo.toml') -Raw
$pkg = [regex]::Match($ws, '(?ms)^\[workspace\.package\].*?(?=^\[)').Value
$deps = @([regex]::Matches($ws, '(?m)^(serde|serde_json|toml) = .*$') | ForEach-Object { $_.Value }) -join "`n"
$manifest = "[workspace]`nmembers = [`"mw2er-core`"]`nresolver = `"3`"`n`n$pkg`n[workspace.dependencies]`n$deps`n`n[profile.release]`npanic = `"unwind`"`nincremental = false`n"
[IO.File]::WriteAllText((Join-Path $Work 'Cargo.toml'), $manifest)

$lib = Join-Path $Core 'src\pad_logic.rs'
$srcLib = Join-Path $Mod 'mw2er-core\src\pad_logic.rs'
# review C3: the copy must equal the project's pad_logic.rs byte for byte before anything runs
$origBytes = [IO.File]::ReadAllBytes($srcLib)
$copyBytes = [IO.File]::ReadAllBytes($lib)
if ($origBytes.Length -ne $copyBytes.Length -or [Convert]::ToBase64String($origBytes) -ne [Convert]::ToBase64String($copyBytes)) {
    Write-Output "FAIL c3_padmut: the copied pad_logic.rs differs from the source (a concurrent writer?)"
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
    exit 1
}
$orig = [IO.File]::ReadAllText($srcLib)

function Run-Tests([string]$Label) {
    Push-Location -LiteralPath $Work
    try {
        # the dumped-graph test reads ..\..\dumps relative to the crate, which is not there in this copy
        $out = & cargo test --release --offline --lib -p mw2er-core -- c3_pad_ --skip c3_pad_graphs_equal_the_dumped_files 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    $failed = @($out | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $ok = @($out | Where-Object { $_ -match '^test \S+ \.\.\. ok' }).Count
    $built = -not (@($out | Where-Object { $_ -match '^error\[|could not compile' }).Count -gt 0)
    return @{ Code = $code; Failed = $failed; Ok = $ok; Built = $built; Out = ($out -join "`n") }
}

$exitCode = 1
try {
$base = Run-Tests 'baseline'
if ($base.Code -ne 0 -or $base.Ok -lt 10) {
    Write-Output "FAIL c3_padmut: the unmutated copy does not pass (exit $($base.Code), $($base.Ok) ok): $(($base.Out -split "`n" | Select-Object -Last 5) -join ' | ')"
    exit 1
}

$muts = [ordered]@{
    'swap yaw / pitch'    = @('let base_yaw = lerp(p.yaw, p.yaw_ads) * p.sens;', 'let base_yaw = lerp(p.pitch, p.pitch_ads) * p.sens;')
    'drop the latch'      = @('if latch_next(st.latched & m != 0, want & m != 0, i.levels_lo & m != 0) {', 'if want & m != 0 {')
    'wrong graph index'   = @('        _ => &GRAPH_3,', '        _ => &GRAPH_2,')
}
$caught = 0
$lines = @()
foreach ($k in $muts.Keys) {
    $a, $b = $muts[$k]
    if (-not $orig.Contains($a)) { $lines += "NOT APPLIED $k"; continue }
    $i = $orig.IndexOf($a)
    [IO.File]::WriteAllText($lib, $orig.Substring(0, $i) + $b + $orig.Substring($i + $a.Length))
    $r = Run-Tests $k
    if ($r.Built -and $r.Failed.Count -gt 0) {
        $caught++
        $lines += "caught $k ($($r.Failed.Count) test(s) failed: $((($r.Failed | ForEach-Object { ($_ -split ' ')[1] -split '::' | Select-Object -Last 1 }) -join ', ')))"
    } else {
        $lines += "MISSED $k (exit $($r.Code), built $($r.Built), $($r.Failed.Count) failed)"
    }
}
$lines | ForEach-Object { Write-Output "  $_" }
if ($caught -eq $muts.Count) {
    Write-Output "PASS c3_padmut: $caught/$($muts.Count) mutations caught (baseline $($base.Ok) c3_pad_ tests ok)"
    $exitCode = 0
} else {
    Write-Output "FAIL c3_padmut: $caught/$($muts.Count) mutations caught"
}
} finally {
    # restore, then drop this run's copy (the shared target dir stays for the next run)
    if (Test-Path -LiteralPath $lib) { [IO.File]::WriteAllText($lib, $orig) }
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
exit $exitCode
