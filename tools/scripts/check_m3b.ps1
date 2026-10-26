<#
.SYNOPSIS
  Runs every offline oracle of milestone M3b (first person) after the whole M3 suite (which runs M2 first) and
  prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m3b\ plus what
  check_m3.ps1 / check_m2.ps1 write (work\, converted\viewmodel\, mod\target, build\natives). Exit code 0 only if
  check_m3.ps1 passed (18 PASS, including check_m2.ps1 21 PASS) and all 8 M3b checks PASS; a SKIP is a failure
  unless -AllowSkip. -SkipM3 skips check 1 for fast iteration but always exits non-zero.

  Checks (docs\M3B_DESIGN.md section 9):
   1 M3 suite (incl. M2)  2 cargo build (0 warnings)  3 cargo test (first-person categories + the mw2er write-cost
     regression test)  4 config [first_person]
   5 write wiring         6 DLL proof strings          7 analyzer (blocked hits, FP row, real run 3)
   8 M2 rows unchanged (M4 changes the regulation on purpose: docs\M4_DESIGN.md 14.3 replaced the package-sha pin
     with the 13 M2 rows + every untouched param byte-identical to the recorded M2 build; the M4 package sha is
     pinned in check_m4.ps1)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m3b.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM3)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$LogDir = Join-Path $Root 'work\check_m3b'
# The regulation package verified by M2 (MODLOG 2026-10-14: sha256 8f376e5a...), recorded as hashes in
# fixtures\m2_regulation_hashes.json (record_m2_rows.py). Check 8 compares the current build with it.
$M2PackageSha = '8f376e5af59c3938d3b248f03523f90e0cd1fe67a309efe5ceea005dab525e6b'
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

$results = New-Object System.Collections.ArrayList

function Add-Result([string]$Id, [string]$Name, [string]$Status, [string]$Detail) {
    [void]$results.Add([pscustomobject]@{ Id = $Id; Name = $Name; Status = $Status; Detail = $Detail })
    Write-Output ('{0,-4} {1,2} {2,-24} {3}' -f $Status, $Id, $Name, $Detail)
}

function Invoke-Tool([string]$Exe, [string[]]$Arguments, [string]$LogName, [string]$Cwd = $Root) {
    Push-Location -LiteralPath $Cwd
    try {
        $out = & $Exe @Arguments 2>&1 | ForEach-Object { "$_" }
        $code = $LASTEXITCODE
    } catch {
        $out = @("$_")
        $code = 9009
    } finally {
        Pop-Location
    }
    $text = ($out -join "`n")
    if ($LogName) { Set-Content -LiteralPath (Join-Path $LogDir "$LogName.log") -Value $text -Encoding UTF8 }
    return @{ Code = $code; Out = $text }
}

function Last-Line([string]$Text, [string]$Pattern = '.') {
    $lines = @($Text -split "`n" | Where-Object { $_ -match $Pattern })
    if ($lines.Count -gt 0) { return $lines[-1].Trim() } else { return '' }
}

function Run-Check([string]$Id, [string]$Name, [scriptblock]$Body) {
    try { & $Body } catch { Add-Result $Id $Name 'FAIL' "exception: $_" }
}

Write-Output "M3b offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m3b\"
Write-Output ''

# 1. the whole M3 suite first (it runs check_m2.ps1 as its check 1)
if ($SkipM3) {
    Add-Result 1 'M3 suite' 'SKIP' '-SkipM3 given (the run cannot pass)'
} else {
    Run-Check 1 'M3 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m3.ps1')) 'check_m3'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        $m2 = Last-Line $r.Out '^PASS\s+1 M2 suite'
        if ($r.Code -eq 0 -and $pass -eq 18 -and $bad.Count -eq 0 -and $r.Out -match 'M3 OFFLINE ORACLES: PASS' -and $m2 -match '21 PASS') {
            Add-Result 1 'M3 suite' 'PASS' "check_m3.ps1: 18 PASS (its check 1: check_m2.ps1 21 PASS), 0 FAIL/SKIP"
        } else { Add-Result 1 'M3 suite' 'FAIL' "exit $($r.Code); $pass PASS; $($bad -join ' | ')" }
    }
}

# 2. cargo build: the workspace (incl. the first-person code) with 0 warnings under mod\
Run-Check 2 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest)` \(' -or $_ -match '\\mod\\') })
    $errors = @($r.Out -split "`n" | Where-Object { $_ -match '(^|\s)error(\[|:)' })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll)) {
        Add-Result 2 'cargo build' 'PASS' "release workspace; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 2 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s); $($errors.Count) error line(s): $((@($ours) + @($errors) | Select-Object -First 2) -join ' | ')" }
}

# 3. cargo test: mw2er-core with every first-person category (docs\M3B_DESIGN.md section 9)
Run-Check 3 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core') 'cargo_test' $Mod
    $okLines = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { ($_ -split ' ')[1] })
    $cats = [ordered]@{
        'fp_logic arming' = '^fp_logic::tests::fp_logic_arming'; 'fp_logic every trigger' = '^fp_logic::tests::fp_logic_each_trigger'
        'fp_logic priority' = '^fp_logic::tests::fp_logic_priority'; 'fp_logic edge/fade watch' = '^fp_logic::tests::fp_logic_edge_and_fade'
        'fp_logic signal rules' = '^fp_logic::tests::fp_logic_signal_rules'; 'fp_logic anti-flap' = '^fp_logic::tests::fp_logic_flap_guard'
        'fp_logic edge latch (review major 1)' = '^fp_logic::tests::fp_logic_edge_latch_keeps_handback'; 'fp_logic latch stale release' = '^fp_logic::tests::fp_logic_edge_latch_stale_release'
        'fp_math eye' = '^fp_math::tests::fp_math_eye_known'; 'fp_math smoothing' = '^fp_math::tests::fp_math_eye_smoothing'
        'fp_math pitch clamp' = '^fp_math::tests::fp_math_pitch_clamp'; 'fp_math yaw quat' = '^fp_math::tests::fp_math_yaw_quat'
        'fp_math yaw probe' = '^fp_math::tests::fp_math_yaw_probe'; 'fp_math yaw gate' = '^fp_math::tests::fp_math_yaw_gate'
        'fp_math drift' = '^fp_math::tests::fp_math_drift'
        'fp_restore persist (rewrite)' = '^fp_restore::tests::fp_restore_persist_watch_engine'; 'fp_restore persist (stays)' = '^fp_restore::tests::fp_restore_persist_watch_write'
        'fp_restore render bit x256' = '^fp_restore::tests::fp_restore_render_bit_all_256'; 'fp_restore fade' = '^fp_restore::tests::fp_restore_fade'
        'fp_restore yaw release' = '^fp_restore::tests::fp_restore_yaw_release'; 'config [first_person]' = '^config::first_person_tests::'
        'fp_restore hook row restore (run 5)' = '^fp_restore::tests::fp_restore_hook_row_restore'
        'fp_restore cleared byte (auto rotation / wall recovery)' = '^fp_restore::tests::fp_restore_cleared_byte_round_trip'
        'write_cache steady state (run 5 regression)' = '^write_cache::tests::write_cache_steady_state_needs_no_checks'
        'write_cache hit/miss/flush' = '^write_cache::tests::write_cache_hit_miss_flush'
    }
    $missing = @($cats.Keys | Where-Object { $pat = $cats[$_]; -not ($okLines | Where-Object { $_ -match $pat }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    # The run-5 regression guard needs Win32: game::write on a 1 GiB committed region must stay microseconds per write
    # (the old VirtualQuery + WriteProcessMemory helper cost ~3 ms there, ~35 ms on ER's heap), and the refusals hold.
    $g = Invoke-Tool 'cargo' @('test', '-p', 'mw2er', '--lib', '--release', '--', '--nocapture') 'cargo_test_mw2er' $Mod
    $gw = Last-Line $g.Out 'GAMEWRITE'
    $gok = $g.Code -eq 0 -and $g.Out -match 'game::tests::game_write_fast_and_safe \.\.\. ok' -and $gw
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $gok) { Add-Result 3 'cargo test' 'PASS' "$($okLines.Count) core tests ok across $($cats.Count) M3b categories; mw2er write-cost test ok ($gw)" }
    else { Add-Result 3 'cargo test' 'FAIL' "exit $($c.Code); missing: $($missing -join ', '); failed: $($failed -join ' | '); mw2er test exit $($g.Code) ($gw)" }
}

# 4. config: [first_person] with the documented defaults, parsed by tomllib and by the DLL's parser
Run-Check 4 'config' {
    $code = "import tomllib,sys; d=tomllib.load(open(sys.argv[1],'rb')); f=d['first_person']; k=d['keys']; " +
            "want={'enabled':True,'camera_mode':'hook','eye_height_m':1.6,'eye_forward_m':0.1,'pitch_clamp_deg':85.0,'hide_method':'render_flag','yaw_follow':True,'world_fov_deg':0.0,'arm_ticks':30,'sync_chr_cam':True,'disable_wall_recovery':True,'disable_auto_rotation':True}; " +
            "bad=[x for x,v in want.items() if f.get(x)!=v] + ([] if k.get('fp_toggle')=='F6' else ['keys.fp_toggle']); " +
            "print('tomllib ok first_person=' + ','.join('%s=%s' % (x, f[x]) for x in want) + ' fp_toggle=' + str(k.get('fp_toggle'))) if not bad else (print('FAIL defaults differ: %s' % bad), sys.exit(1))"
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:') { Add-Result 4 'config' 'PASS' ("{0}; {1}" -f $t.Out.Trim(), (Last-Line $c.Out '^PASS:')) }
    else { Add-Result 4 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code)" }
}

# 5. write wiring: version-gated, non-faulting writes only; hook constants; attribution; offset asserts (+ self-test)
Run-Check 5 'write wiring' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'fpwiring', '--self-test') 'fpwiring'
    Add-Result 5 'write wiring' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (((@($r.Out -split "`n" | Where-Object { $_ -match '^(self-test|PASS|FAIL)' }) | ForEach-Object { $_.Trim() }) -join ' | '))
}

# 6. the built DLL and the staged copy (what me3 loads) carry the proof log strings and are the same file
Run-Check 6 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'fpstrings', $BuiltDll) 'fpstrings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    if ($a.Code -eq 0 -and $same) { Add-Result 6 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 6 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (run check_m2.ps1 to stage)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 7. analyzer self-test: blocked hits (incl. the real run-3 session), the first-person row, the M2/M3 rows
Run-Check 7 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $ok = $r.Code -eq 0 -and $r.Out -match 'first-person cases' -and $r.Out -match 'hits PASS with 7 blocked\?' -and $r.Out -match 'c4311 band 79000105 35\.\.75'
    Add-Result 7 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 8. M2 rows unchanged: the 13 M2 ids present and byte-identical to the M2 build, every param M4 does not edit identical
Run-Check 8 'M2 rows unchanged' {
    $ids = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ids') 'ids'
    $rows = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'm2rows') 'm2rows'
    $n = Invoke-Tool $Py @('-I', '-c', "import json,sys; r=json.load(open(sys.argv[1])); w=r['weapons']; print(sum(2*len(v['bands']) for v in w.values())+1)", (Join-Path $Root 'build\rows.json')) 'rows_count'
    $fx = Invoke-Tool $Py @('-I', '-c', "import json,sys; print(json.load(open(sys.argv[1]))['package_sha256'])", (Join-Path $Scripts 'fixtures\m2_regulation_hashes.json')) 'm2_fixture'
    if ($ids.Code -eq 0 -and $rows.Code -eq 0 -and $n.Out.Trim() -eq '13' -and $fx.Out.Trim() -eq $M2PackageSha) { Add-Result 8 'M2 rows unchanged' 'PASS' "13 M2 row ids; $(Last-Line $rows.Out '^PASS'); $(Last-Line $ids.Out '^PASS')" }
    else { Add-Result 8 'M2 rows unchanged' 'FAIL' "ids exit $($ids.Code); m2rows exit $($rows.Code) ($(Last-Line $rows.Out '(PASS|FAIL)')); M2 rows $($n.Out.Trim()); fixture package $($fx.Out.Trim().Substring(0, [Math]::Min(12, $fx.Out.Trim().Length)))" }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run): PERF ticks_per_s with first person on (must be near the FP-off value now that a write costs microseconds; page_check_ms_max shows the one-time cost per page), the follow-cam hook and the ChrCam/auto-rotation/wall-recovery writes (FPCAM write/restore lines), whether enable_render hides the body without side effects or is reasserted, the character forward axis and yaw drift, every inferred handback signal (FP probe lines).'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM3 -and $results.Count -eq 8) {
    Write-Output 'M3b OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M3b OFFLINE ORACLES: FAIL'
exit 1
