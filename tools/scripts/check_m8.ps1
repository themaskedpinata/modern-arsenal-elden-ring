<#
.SYNOPSIS
  Runs every offline oracle of milestone M8 (MW2 mantling onto rocks and ledges, the chunk re-base fix, the ladder
  handback audit) after the whole M7 suite (which runs M6, M5, M4, M3b, M3 and M2 first) and prints PASS / FAIL / SKIP
  per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m8\, work\m8\ (the
  bench file; the research scripts there are only read and run read-only) and work\t\ (the iw4dis build) plus what the
  sub-checks write. iw4sp.exe and the xanim dumps are only READ as files; the install folders are only listed. Exit
  code 0 only if all 12 checks PASS; a SKIP is a failure unless -AllowSkip. -SkipM7 skips check 1 for fast iteration
  but always exits non-zero.

  Checks (docs\M8_DESIGN.md section 13):
   1 M7 suite  2 exe static pin (+ 46 mantle sites, iw4dis)  3 xanim dumps  4 converter  5 cargo build  6 cargo test
   7 movebench  8 config  9 wiring  10 analyzer  11 DLL proof strings  12 hygiene + no game-folder writes

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m8.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM7)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Bench = Join-Path $Mod 'target\release\movebench.exe'
$LogDir = Join-Path $Root 'work\check_m8'
$DisTarget = Join-Path $Root 'work\t'
$Mw2Dir = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2'
$Mw2Exe = Join-Path $Mw2Dir 'iw4sp.exe'
$ErGame = 'D:\SteamLibrary\steamapps\common\ELDEN RING\Game'
$PinnedExeSha = '2f8c9cfda1b8c0fb308fd9719577154e8222fc7654f5e8f1bf54579bca69d805'
$SpAnims = @('player_mantle_up_57', 'player_mantle_up_51', 'player_mantle_up_45', 'player_mantle_up_39', 'player_mantle_up_33',
             'player_mantle_up_27', 'player_mantle_up_21', 'player_mantle_over_high', 'player_mantle_over_mid', 'player_mantle_over_low')
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Root 'work\m8') | Out-Null

$results = New-Object System.Collections.ArrayList

function Add-Result([string]$Id, [string]$Name, [string]$Status, [string]$Detail) {
    [void]$results.Add([pscustomobject]@{ Id = $Id; Name = $Name; Status = $Status; Detail = $Detail })
    Write-Output ('{0,-4} {1,2} {2,-22} {3}' -f $Status, $Id, $Name, $Detail)
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

function Cut([string]$s, [int]$n = 160) { if ($s.Length -gt $n) { return $s.Substring(0, $n) } else { return $s } }

function Run-Check([string]$Id, [string]$Name, [scriptblock]$Body) {
    try { & $Body } catch { Add-Result $Id $Name 'FAIL' "exception: $_" }
}

Write-Output "M8 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m8\"
Write-Output ''

# 1. the whole M7 suite first (check_m6 -> check_m5 -> check_m4 -> check_m3b -> check_m3 -> check_m2, which stages the DLL)
if ($SkipM7) {
    Add-Result 1 'M7 suite' 'SKIP' '-SkipM7 given (the run cannot pass)'
} else {
    Run-Check 1 'M7 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m7.ps1')) 'check_m7'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        if ($r.Code -eq 0 -and $r.Out -match 'M7 OFFLINE ORACLES: PASS' -and $pass -ge 13 -and $bad.Count -eq 0) {
            Add-Result 1 'M7 suite' 'PASS' "check_m7.ps1: $pass PASS ($(Last-Line $r.Out '^SUMMARY'))"
        } else { Add-Result 1 'M7 suite' 'FAIL' "exit $($r.Code); $pass PASS; $(Cut ($bad -join ' | ') 400)" }
    }
}

# 2. exe static pin: iw4sp.exe sha256 == the pin; the 46 mantle sites byte-check (work\m8\mantle_sites.py, read-only) and
#    tools\iw4dis decodes every recorded operand (the converter's --sites list, M5 + M8)
Run-Check 2 'exe static pin' {
    $sha = if (Test-Path -LiteralPath $Mw2Exe) { (Get-FileHash -LiteralPath $Mw2Exe -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' }
    $s = Invoke-Tool $Py @('-I', (Join-Path $Root 'work\m8\mantle_sites.py')) 'mantle_sites'
    $siteLines = @($s.Out -split "`n" | Where-Object { $_ -match 'iw4sp\.exe @0x' })
    $siteFail = @($s.Out -split "`n" | Where-Object { $_ -match '\sFAIL\s' })
    $env:CARGO_TARGET_DIR = $DisTarget
    try { $bd = Invoke-Tool 'cargo' @('build', '--release', '--manifest-path', (Join-Path $Root 'tools\iw4dis\Cargo.toml')) 'iw4dis_build' }
    finally { Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
    $dis = Join-Path $DisTarget 'release\iw4dis.exe'
    $st = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--sites') 'convert_sites'
    $sites = @($st.Out -split "`n" | Where-Object { $_ -match '^[0-9a-f]+\t' } | ForEach-Object { $f = $_.Trim() -split "`t"; [pscustomobject]@{ Va = $f[0]; Want = $f[1]; Name = $f[2] } })
    $miss = @('iw4dis not built')
    if ($bd.Code -eq 0 -and (Test-Path -LiteralPath $dis) -and $sites.Count -gt 0) {
        $d = Invoke-Tool $dis @($Mw2Exe, 'site', (($sites | ForEach-Object { $_.Va }) -join ',')) 'iw4dis_sites'
        $lines = @($d.Out -split "`n")
        $miss = @($sites | Where-Object { $x = $_; -not ($lines | Where-Object { $_.StartsWith($x.Va + '  ') -and $_.Contains($x.Want) }) } | ForEach-Object { "$($_.Name)@$($_.Va)" })
        if ($d.Code -ne 0) { $miss += "iw4dis exit $($d.Code)" }
    }
    if ($sha -eq $PinnedExeSha -and $s.Code -eq 0 -and $siteLines.Count -eq 46 -and $siteFail.Count -eq 0 -and $sites.Count -ge 101 -and $miss.Count -eq 0) {
        Add-Result 2 'exe static pin' 'PASS' "iw4sp.exe sha256 $($sha.Substring(0, 12)) (pinned); work\m8\mantle_sites.py 46/46 sites OK; iw4dis: all $($sites.Count) instruction sites (M5 + M8) decode to the recorded operands"
    } else { Add-Result 2 'exe static pin' 'FAIL' "sha $($sha.Substring(0, [Math]::Min(12, $sha.Length))); sites $($siteLines.Count)/46, FAIL lines $($siteFail.Count); iw4dis misses: $(Cut ($miss -join ', ') 200)" }
}

# 3. the xanim dumps: both OAT logs report 0 warnings / 0 errors; the 10 SP files' sha256 == movement.json's pins
Run-Check 3 'xanim dumps' {
    $problems = New-Object System.Collections.ArrayList
    foreach ($lg in @('dumps\m8\code_post_gfx.log', 'dumps\m8\common.log')) {
        $p = Join-Path $Root $lg
        if (-not (Test-Path -LiteralPath $p)) { [void]$problems.Add("$lg missing"); continue }
        $last = (Get-Content -LiteralPath $p | Where-Object { $_ -match 'Finished with' } | Select-Object -Last 1)
        if ($last -notmatch 'Finished with 0 warnings, 0 errors') { [void]$problems.Add("${lg}: $last") }
    }
    $mj = Get-Content -LiteralPath (Join-Path $Root 'converted\movement.json') -Raw | ConvertFrom-Json
    $n = 0
    foreach ($a in $SpAnims) {
        $f = Join-Path $Root "dumps\m8\code_post_gfx\xanim\$a"
        $pin = $mj.mantle.anims.$a.sha256
        $h = if (Test-Path -LiteralPath $f) { (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' }
        if ($h -ne $pin) { [void]$problems.Add("$a sha $($h.Substring(0, [Math]::Min(12, $h.Length))) != pin $pin") } else { $n++ }
    }
    if ($problems.Count -eq 0) { Add-Result 3 'xanim dumps' 'PASS' "code_post_gfx.log + common.log: 0 warnings / 0 errors; $n/10 SP player_mantle_* files == the movement.json sha256 pins" }
    else { Add-Result 3 'xanim dumps' 'FAIL' ($problems -join ' | ') }
}

# 4. converter: known answers + the pre-M8 keys byte-identical (--check), the M5 and M8 self-test mutations, and a
#    regenerate byte-identical to converted\movement.json
Run-Check 4 'converter' {
    $mj = Join-Path $Root 'converted\movement.json'
    $before = (Get-FileHash -LiteralPath $mj -Algorithm SHA256).Hash
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--check') 'convert_check'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--self-test') 'convert_selftest'
    $m = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--self-test-m8') 'convert_selftest_m8'
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py')) 'convert_regenerate'
    $after = (Get-FileHash -LiteralPath $mj -Algorithm SHA256).Hash
    $ok = $a.Code -eq 0 -and $a.Out -match 'PASS check:' -and $a.Out -match 'PASS mantle: 46 byte-checked sites' -and $a.Out -match 'pre-M8 keys byte-identical' -and
          $b.Code -eq 0 -and $b.Out -match 'PASS self-test: 5/5 mutations caught' -and
          $m.Code -eq 0 -and $m.Out -match 'PASS self-test: (\d+)/\1 M8 mutations caught' -and [int]$Matches[1] -ge 6 -and
          $g.Code -eq 0 -and $before -eq $after
    Add-Result 4 'converter' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | {2} | regenerate byte-identical: {3}" -f (Cut (Last-Line $a.Out '^(PASS|FAIL) mantle') 220), (Last-Line $b.Out '(PASS|FAIL) self-test'), (Last-Line $m.Out '(PASS|FAIL) self-test'), ($before -eq $after))
}

# 5. cargo build: the whole workspace with 0 warnings under mod\
Run-Check 5 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)` \(' -or $_ -match '\\mod\\') })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Bench)) {
        Add-Result 5 'cargo build' 'PASS' "release workspace; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 5 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s): $(($ours | Select-Object -First 2) -join ' | ')" }
}

# 6. cargo test: every 12.1 category by name prefix, the M8 real-data test, the workspace
Run-Check 6 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '--workspace', '--release') 'cargo_test' $Mod
    $okNames = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { (($_ -split ' ')[1] -split '::')[-1] })
    $cats = @('mantle_detect_', 'mantle_over_', 'mantle_origin_', 'mantle_path_', 'mantle_state_', 'mantle_never_inside_', 'mantle_trigger_',
              'mantle_end_', 'mantle_weapon_', 'rebase_', 'ladder_', 'config_mantle_', 'config_keys_m8_')
    $missing = @($cats | Where-Object { $p = $_; -not ($okNames | Where-Object { $_.StartsWith($p) }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $env:MW2ER_ROOT = $Root
    try { $rd = Invoke-Tool 'cargo' @('test', '--release', '-p', 'mw2er-core', '--test', 'real_data_m8', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data_m8' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $rdPass = @($rd.Out -split "`n" | Where-Object { $_ -match 'PASS:' })
    $n = @($cats | ForEach-Object { $p = $_; @($okNames | Where-Object { $_.StartsWith($p) }).Count } | Measure-Object -Sum).Sum
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $rd.Code -eq 0 -and $rdPass.Count -eq 1) {
        Add-Result 6 'cargo test' 'PASS' "$($okNames.Count) workspace tests ok; all $($cats.Count) M8 categories ($n tests); real data: $(Cut $rdPass[0] 160)"
    } else { Add-Result 6 'cargo test' 'FAIL' "exit $($c.Code); missing categories: $($missing -join ', '); failed: $($failed -join ' | '); real data exit $($rd.Code)" }
}

# 7. movebench: B13-B20 at 30/60/144 Hz (+ the bench-wide ray / geometry rules), B1-B12 unchanged, the perf run
Run-Check 7 'movebench' {
    $m = Invoke-Tool $Bench @('--mantle', '--root', $Root) 'movebench_mantle'
    $c = Invoke-Tool $Bench @('--check', '--root', $Root) 'movebench_check'
    $p = Invoke-Tool $Bench @('--perf', '--root', $Root) 'movebench_perf'
    $lm = Last-Line $m.Out '^(PASS|FAIL) MOVEBENCH M8'
    $lc = Last-Line $c.Out '^(PASS|FAIL) MOVEBENCH'
    $ok = $m.Code -eq 0 -and $lm -match '^PASS MOVEBENCH M8 14/14 scenarios' -and $lm -match ' 0 failed' -and (Test-Path -LiteralPath (Join-Path $Root 'work\m8\bench_m8.json')) -and
          $c.Code -eq 0 -and $lc -match '^PASS MOVEBENCH 12/12 scenarios' -and $p.Code -eq 0 -and $p.Out -match 'PASS PERF movebench'
    Add-Result 7 'movebench' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | {2}" -f (Cut $lm 150), (Cut $lc 90), (Cut (Last-Line $p.Out 'PERF movebench') 120))
}

# 8. config: the shipped [movement.mantle] block has the design defaults; the DLL parser accepts the file; the M8 config
#    tests (bounds, unknown key, a missing block = defaults, E rejected for the 5 keys)
Run-Check 8 'config' {
    $code = @'
import sys, tomllib
d = tomllib.load(open(sys.argv[1], 'rb'))
m = d.get('movement', {})
mt = m.get('mantle', {})
want = {'enabled': True, 'key_mode': 'mw2', 'require_forward': False, 'max_height_m': 2.0, 'max_rise_in': 60, 'check_angle_deg': 60,
        'check_range_in': 20, 'time_scale': 1.0, 'over': 'thin', 'over_max_drop_m': 2.0, 'crouch_fallback': True, 'low_keeps_weapon': True,
        'vm': 'drop', 'sound': True, 'max_rays': 24, 'refuse_cache_ms': 150, 'log_refusals': True, 'depenetrate_m': 0.3}
bad = ['movement.mantle.%s=%r' % (k, mt.get(k)) for k, v in want.items() if mt.get(k) != v]
extra = sorted(set(mt) - set(want))
if extra: bad.append('unknown [movement.mantle] keys %s' % extra)
if m.get('ladder_er_attacks') is not False: bad.append('movement.ladder_er_attacks=%r' % m.get('ladder_er_attacks'))
# run 18: [movement.rays] / [movement.unstick] / first_person.cam_gate_close_ok at the defaults
# C3 (updated in place, docs\C3_DESIGN.md 1.3): mode player (the run-18 union stays a hot rollback) + the player keys
rw = {'map_filter': 0x2000058, 'object_filter': 0x5b, 'mode': 'union', 'body_probes': True, 'player_fallback': 0x3, 'player_layers': [2, 3, 4], 'log_live_table': True}
uw = {'enabled': True, 'window_s': 2.5, 'progress_m': 0.5, 'wish_frac': 0.8, 'evidence_frac': 0.1, 'hold_s': 1.0, 'max_hold_s': 8.0, 'max_per_window': 4, 'backoff_window_s': 60.0}
for name, want2 in (('rays', rw), ('unstick', uw)):
    got = m.get(name, {})
    bad += ['movement.%s.%s=%r' % (name, k, got.get(k)) for k, v in want2.items() if got.get(k) != v]
    if sorted(set(got) - set(want2)): bad.append('unknown [movement.%s] keys %s' % (name, sorted(set(got) - set(want2))))
if d.get('first_person', {}).get('cam_gate_close_ok') is not True: bad.append('first_person.cam_gate_close_ok')
k = d.get('keys', {})
for name in ('reload', 'toggle', 'fp_toggle', 'ac130', 'ac130_exit'):
    if str(k.get(name, '')).upper() == 'E': bad.append('keys.%s is E' % name)
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('tomllib ok: [movement.mantle] %d keys at the design defaults (mw2 key, 2.0 m cap, 60 in rise, 60 deg, 20 in, thin over, drop vm, 24 rays, depenetrate 0.3 m), [movement.rays] player (C3; rollback union 0x2000058+0x5b), [movement.unstick] defaults, cam_gate_close_ok, ladder_er_attacks=false, no key on E' % len(mt))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::') 'config_m8_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'config_mantle_defaults_bounds_and_unknown_key \.\.\. ok' -and $u.Out -match 'config_keys_m8_e_rejected \.\.\. ok' -and $u.Out -match 'config_rays_defaults_modes_and_bounds \.\.\. ok' -and $u.Out -match 'config_unstick_defaults_and_bounds \.\.\. ok'
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok) { Add-Result 8 'config' 'PASS' ("{0}; {1}; config M8 tests ok (defaults, 18 bound / enum / unknown-key errors, E rejected)" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 8 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code); config tests exit $($u.Code)" }
}

# 9. wiring: the M8 checks with their mutations + the M5 movement wiring they touch
Run-Check 9 'wiring' {
    $w = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'mantlewiring', '--self-test') 'mantlewiring'
    $m = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movewiring', '--self-test') 'movewiring'
    $ok = $w.Code -eq 0 -and $w.Out -match 'self-test: (\d+) mutations of the M8 sources caught' -and [int]$Matches[1] -ge 15 -and $m.Code -eq 0
    Add-Result 9 'wiring' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | move {2}" -f (Last-Line $w.Out 'self-test'), (Cut (Last-Line $w.Out '^(PASS|FAIL)') 160), (Last-Line $m.Out '^(PASS|FAIL)').Substring(0, 4))
}

# 10. analyzer: the self-test (M8 fixtures + mutations + the real run-17 excerpt + the P2 fixes), the two M8 fixtures
#     and the run-17 excerpt through the CLI
Run-Check 10 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $fm = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m8_mantle_session.log')) 'analyze_m8_mantle'
    $fl = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m8_ladder_session.log')) 'analyze_m8_ladder'
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\run17_excerpt.log')) 'analyze_run17'
    $mRows = @($fm.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(mantle|mantle refusals|mantle perf|rebase)\s' }).Count
    $lRows = @($fl.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(ladder handback|ladder dismount|ladder weapon|ladder input|ladder signal)\s' }).Count
    $rb = @($g.Out -split "`n" | Where-Object { $_ -match '^FAIL\s+rebase\s' }).Count
    $p2 = @($g.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(first person|jump)\s' }).Count
    $na = @($g.Out -split "`n" | Where-Object { $_ -match '^N/A\s+(mantle|ladder handback|ladder dismount|ladder weapon)\s{2,}' }).Count
    $okSelf = $r.Code -eq 0 -and $r.Out -match 'M8 fixtures m8_mantle_session\.log \+ m8_ladder_session\.log: .*?, (\d+) M8 mutations FAIL, older logs N/A' -and [int]$Matches[1] -ge 13 -and $r.Out -match 'real run-17 excerpt: rebase FAIL'
    $ok = $okSelf -and $mRows -eq 4 -and $lRows -eq 5 -and $rb -eq 1 -and $p2 -eq 2 -and $na -eq 4
    Add-Result 10 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0}; M8 mantle fixture {1}/4 rows PASS, ladder fixture {2}/5 rows PASS; run-17 excerpt: rebase FAIL={3} (as found), first person + jump PASS={4}/2 (P2), mantle / ladder N/A={5}/4" -f $r.Code, $mRows, $lRows, $rb, $p2, $na)
}

# 11. DLL proof strings (built == staged; check_m2 inside check 1 stages it)
Run-Check 11 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'm8strings', $BuiltDll) 'm8strings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    if ($a.Code -eq 0 -and $same) { Add-Result 11 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 11 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 12. hygiene: converted\ + work\ ignored, static hygiene, install folders == vanilla baseline (listing only), the game's
#     regulation untouched, no game path written by the converter / analyzers (nothing here ever writes %APPDATA%\EldenRing)
Run-Check 12 'hygiene' {
    $problems = New-Object System.Collections.ArrayList
    $gi = Get-Content -LiteralPath (Join-Path $Root '.gitignore') -ErrorAction SilentlyContinue
    foreach ($x in '/converted/', '/work/') { if (-not ($gi | Where-Object { $_.Trim() -eq $x })) { [void]$problems.Add("$x not in .gitignore") } }
    $h = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'hygiene') 'hygiene_static'
    if ($h.Code -ne 0) { [void]$problems.Add((Last-Line $h.Out 'FAIL')) }
    $baseline = Get-Content -LiteralPath (Join-Path $Scripts 'fixtures\install_baseline.json') -Raw | ConvertFrom-Json
    $new = @()
    foreach ($d in @($ErGame, $Mw2Dir)) {
        if (-not (Test-Path -LiteralPath $d)) { [void]$problems.Add("install folder missing: $d"); continue }
        $old = @($baseline.$d)
        $top = @(Get-ChildItem -LiteralPath $d -Force | ForEach-Object { $_.Name })
        $new += @($top | Where-Object { $old -notcontains $_ } | ForEach-Object { "$d\$_" })
    }
    # co-op (MODLOG 2026-10-19): Seamless Co-op running in the game process creates <ER Game>\SeamlessCoop\crashdumps (empty) on
    # its first start. Accepted only while it holds no file at all (a crash dump there FAILs: it may contain mw2er memory).
    $new = @($new | Where-Object { -not ((Split-Path -Leaf $_) -eq 'SeamlessCoop' -and (Split-Path -Parent $_) -eq $ErGame -and @(Get-ChildItem -LiteralPath $_ -Recurse -File -Force -ErrorAction SilentlyContinue).Count -eq 0) })
    if ($new.Count -gt 0) { [void]$problems.Add("top-level entries not in the vanilla baseline: $($new -join ', ')") }
    $gf = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    if ($gf.Code -ne 0) { [void]$problems.Add((Last-Line $gf.Out 'FAIL')) }
    # the converter and the M8 scripts read the exe / dumps as bytes and start nothing; no game path is opened for writing
    $code = @'
import pathlib, re, sys
root = pathlib.Path(sys.argv[1])
bad = []
for name in ('convert_movement.py', 'analyze_m8.py', 'analyze_log.py'):
    t = (root / 'tools' / 'scripts' / name).read_text(encoding='utf-8', errors='replace')
    if re.search(r'subprocess|os\.system|Popen|Start-Process|ShellExecute|steam://', t):
        bad.append(name + ' may start a process')
    for m in re.finditer(r'(?i)(SteamLibrary[^\n]*|APPDATA[^\n]*EldenRing[^\n]*)', t):
        if re.search(r'write_text|write_bytes|open\([^)]*.w|Set-Content|Out-File|Copy-Item|Remove-Item', m.group(0)):
            bad.append(name + ' writes a game path: ' + m.group(0)[:60])
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('PASS the converter and the analyzers start no process and write no game path')
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_m8'
    if ($t.Code -ne 0) { [void]$problems.Add($t.Out.Trim()) }
    if ($problems.Count -eq 0) { Add-Result 12 'hygiene' 'PASS' ("converted\ and work\ ignored; {0}; install folders == vanilla baseline; {1}; {2}" -f (Cut (Last-Line $h.Out '^PASS') 80), (Last-Line $gf.Out '^PASS'), $t.Out.Trim()) }
    else { Add-Result 12 'hygiene' 'FAIL' ($problems -join ' | ') }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run 18, docs\M8_DESIGN.md section 14): that the procedural ledge detection matches what looks climbable in ER, the start-inside-solid ray rule in ER, the 2.0 m cap and the airborne reach, the full-footprint landing, the quick drop/raise look, the re-base sign and rule (resync why=rebase sign=), the ladder state values and that the ladder signal is right at all, and that foliage / objects the map filter misses do not block a mantle (abort why=er_push).'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM7 -and $results.Count -eq 12) {
    Write-Output 'M8 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M8 OFFLINE ORACLES: FAIL'
exit 1
