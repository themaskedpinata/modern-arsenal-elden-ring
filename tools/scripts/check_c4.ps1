# C4 offline oracles (docs\C4_UI_DESIGN.md 8.1). Launches nothing; only READS the game exe.
#   powershell -ExecutionPolicy Bypass -File tools\scripts\check_c4.ps1 [-SkipC3]
# Exit 0 only when every check PASSes; prints C4 OFFLINE ORACLES: PASS.
# Not built yet (design 8.1): the c4wiring / c4strings rules of m2_static_checks.py (C4.1 PASS I41 adds c41wiring, check 8);
# c4_static.py is replaced by the ignored Rust test mw2er-core\tests\c4_real_exe.rs (same pins + mutations, MW2ER_REQUIRE_EXE=1).
param([switch]$SkipC3)
$ErrorActionPreference = 'Continue'
$Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Mod = Join-Path $Root 'mod'
$Logs = Join-Path $Root 'work\check_c4'
New-Item -ItemType Directory -Force $Logs | Out-Null
$results = @()
function Run-Check($n, $name, [scriptblock]$body) {
    $log = Join-Path $Logs ("{0:d2}_{1}.log" -f $n, ($name -replace '\W', '_'))
    $out = & $body 2>&1 | Out-String
    $ok = $LASTEXITCODE -eq 0
    Set-Content -Path $log -Value $out -Encoding utf8
    $last = (($out -split "`n") | Where-Object { $_.Trim() } | Select-Object -Last 1)
    $line = "{0} {1,2} {2,-22} {3}" -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), $n, $name, ("$last".Trim())
    Write-Output $line
    $script:results += $ok
}
Write-Output "C4 offline oracles ($Root). Never launches Elden Ring, me3, Steam or Seamless. Logs: work\check_c4\"
if (-not $SkipC3) {
    Run-Check 1 'C3 suite' { powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'check_c3.ps1') }
}
Run-Check 2 'ER static pins' { Push-Location $Mod; $env:MW2ER_REQUIRE_EXE = '1'; cargo test -p mw2er-core --test c4_real_exe -- --ignored --nocapture; $c = $LASTEXITCODE; Remove-Item Env:MW2ER_REQUIRE_EXE; Pop-Location; $global:LASTEXITCODE = $c }
Run-Check 3 'hudhook patches' { python -I (Join-Path $PSScriptRoot 'make_hudhook_patch.py') --check }
Run-Check 4 'cargo test c4' {
    Push-Location $Mod
    $o = cargo test -p mw2er-core --lib c4_ 2>&1 | Out-String
    $c = $LASTEXITCODE
    Pop-Location
    $miss = @('c4_addrs_', 'c4_cfg_', 'c4_gate_', 'c4_state_', 'c4_splice_', 'c4_fmt_', 'c4_ring_', 'c4_getter_', 'c4_probe_', 'c4_teardown_', 'c4_gate41_', 'c4_state_mt_', 'c4_alpha_', 'c4_live41_', 'c4_inject41_') | Where-Object { $o -notmatch [regex]::Escape($_) }
    $o
    if ($miss) { "missing prefixes: $($miss -join ',')"; $c = 1 }
    $global:LASTEXITCODE = $c
}
Run-Check 5 'vmtest c4 (WARP)' { Push-Location $Mod; cargo run --release -p vmtest -- --c4 --warp; $c = $LASTEXITCODE; Pop-Location; $global:LASTEXITCODE = $c }
Run-Check 6 'analyzer' {
    python -I (Join-Path $PSScriptRoot 'analyze_c4.py') --self-test; $a = $LASTEXITCODE
    python -I (Join-Path $PSScriptRoot 'analyze_c4.py') (Join-Path $Root 'logs\mw2er.log'); $b = $LASTEXITCODE
    "replay exit $b (0 READY / 1 NOT READY / 2 N/A; FAIL only on a crash)"
    $global:LASTEXITCODE = $(if ($a -eq 0 -and $b -in 0, 1, 2) { 0 } else { 1 })
}
Run-Check 7 'config defaults' {
    # 2026-10-21: the C4.1 injection is on (CLEAN in-game run, alpha pinned for 2.7.2.0); the probes and the C4 HDR layer stay off
    $t = Get-Content (Join-Path $Root 'mw2er.toml') -Raw
    $bad = @(@('probe_scaleform', 'inject_ui_layer', 'probe_layer_sample') | Where-Object { $t -notmatch "(?m)^$_\s*=\s*false" }) +
           @(@('inject_viewmodel', 'inject_ui_thread') | Where-Object { $t -notmatch "(?m)^$_\s*=\s*true" })
    if ($bad) { "unexpected [ui] values in mw2er.toml: $($bad -join ',')"; $global:LASTEXITCODE = 1 } else { "PASS [ui] probes / inject_ui_layer = false; inject_viewmodel / inject_ui_thread = true"; $global:LASTEXITCODE = 0 }
}
Run-Check 8 'c41 wiring' { python -I (Join-Path $PSScriptRoot 'm2_static_checks.py') c41wiring --self-test }
$pass = @($results | Where-Object { $_ }).Count
Write-Output ("SUMMARY: {0} PASS, {1} FAIL of {2} checks" -f $pass, ($results.Count - $pass), $results.Count)
if ($pass -eq $results.Count) { Write-Output 'C4 OFFLINE ORACLES: PASS'; exit 0 } else { Write-Output 'C4 OFFLINE ORACLES: FAIL'; exit 1 }
