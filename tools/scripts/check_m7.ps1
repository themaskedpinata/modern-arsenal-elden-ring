<#
.SYNOPSIS
  Runs every offline oracle of milestone M7 (the run-14 fixes + the MW2 Tactical Nuke) after the whole M6 suite (which
  runs M5, M4, M3b, M3 and M2 first) and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m7\, work\m7\ and
  work\t\ (the iw4dis build, a short target path) plus what the sub-checks write. eldenring.exe is only READ as a file
  (two instruction ranges disassembled by tools\iw4dis); the install folders are only listed. Exit code 0 only if all
  13 checks PASS; a SKIP is a failure unless -AllowSkip. -SkipM6 skips check 1 for fast iteration but always exits
  non-zero.

  Checks (docs\M7_DESIGN.md section 7):
   1 M6 suite  2 vendored patches + spawn-data bytes  3 exe static pin  4 sound conversion  5 regulation + sha pin
   6 cargo build  7 cargo test (6.1 categories + real data)  8 vmtest nuke (+ perf)  9 config  10 wiring
   11 DLL proof strings  12 analyzer  13 hygiene + no game-folder writes

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m7.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM6)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Vmtest = Join-Path $Mod 'target\release\vmtest.exe'
$LogDir = Join-Path $Root 'work\check_m7'
$VmOut = Join-Path $Root 'work\m7\vm'
$RegPin = Join-Path $Root 'work\m7\regulation.sha256'
$DisTarget = Join-Path $Root 'work\t'
$Vendor = Join-Path $Root 'vendor\fromsoftware-rs'
$VendorRev = '59fbd3b3b7daaf14aca47c9f73530493dba6bc79'
$ErGame = 'D:\SteamLibrary\steamapps\common\ELDEN RING\Game'
$ErExe = Join-Path $ErGame 'eldenring.exe'
$Mw2Dir = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2'
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
New-Item -ItemType Directory -Force -Path $VmOut | Out-Null

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

Write-Output "M7 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m7\"
Write-Output ''

# 1. the whole M6 suite first (check_m5 -> check_m4 -> check_m3b -> check_m3 -> check_m2, which stages the DLL)
if ($SkipM6) {
    Add-Result 1 'M6 suite' 'SKIP' '-SkipM6 given (the run cannot pass)'
} else {
    Run-Check 1 'M6 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m6.ps1')) 'check_m6'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        if ($r.Code -eq 0 -and $r.Out -match 'M6 OFFLINE ORACLES: PASS' -and $pass -ge 12 -and $bad.Count -eq 0) {
            Add-Result 1 'M6 suite' 'PASS' "check_m6.ps1: $pass PASS ($(Last-Line $r.Out '^SUMMARY'))"
        } else { Add-Result 1 'M6 suite' 'FAIL' "exit $($r.Code); $pass PASS; $(Cut ($bad -join ' | ') 400)" }
    }
}

# 2. vendored patches 0001..0003 replay onto the pinned commit == the working tree; the spawn-data byte test (0x44 == 1)
Run-Check 2 'patches + spawn bytes' {
    $files = @('crates/eldenring/src/cs/bullet_manager.rs', 'crates/eldenring/src/cs/field_ins.rs', 'crates/shared/src/task.rs')
    $patches = @('0001-bullet-spawn-data', '0002-recurring-task-no-panic', '0003-bullet-spawn-autofinish') | ForEach-Object { Join-Path $Root "tools\patches\fromsoftware-rs-$_.patch" }
    $head = (Invoke-Tool 'git' @('-C', $Vendor, 'rev-parse', 'HEAD') '').Out.Trim()
    $replay = Join-Path $LogDir 'vendor_replay'
    if (Test-Path -LiteralPath $replay) { Remove-Item -LiteralPath $replay -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $replay | Out-Null
    foreach ($f in $files) {
        $dst = Join-Path $replay ($f -replace '/', '\')
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        cmd /c "git -C `"$Vendor`" show HEAD:$f > `"$dst`""
    }
    $codes = @($patches | ForEach-Object { (Invoke-Tool 'git' @('apply', '--whitespace=nowarn', $_) ('replay_' + [IO.Path]::GetFileNameWithoutExtension($_)) $replay).Code })
    $mism = @($files | Where-Object { (Get-FileHash -LiteralPath (Join-Path $replay ($_ -replace '/', '\'))).Hash -ne (Get-FileHash -LiteralPath (Join-Path $Vendor ($_ -replace '/', '\'))).Hash })
    $p3 = [IO.File]::ReadAllText($patches[2])
    $p3ok = $p3 -match '\+\s+d\.unk44 = 1;' -and $p3 -notmatch '\+\s+d\.unk44 = (9|8|0x8)'
    $t = Invoke-Tool 'cargo' @('test', '--release', '-p', 'mw2er', '--lib', 'spawn::tests::spawn_data_bytes_autofinish_only') 'spawn_bytes' $Mod
    $tok = $t.Code -eq 0 -and $t.Out -match 'spawn_data_bytes_autofinish_only \.\.\. ok'
    if ($head -eq $VendorRev -and @($codes | Where-Object { $_ -ne 0 }).Count -eq 0 -and $mism.Count -eq 0 -and $p3ok -and $tok) {
        Add-Result 2 'patches + spawn bytes' 'PASS' "HEAD $($head.Substring(0, 7)) + 0001/0002/0003 replayed == the working tree; 0003 sets unk44 = 1 (bit 0 only); BulletSpawnData bytes: +0x44 = 01, +0x45..0x48 = 0, bit 3 clear, unknown fields 0"
    } else { Add-Result 2 'patches + spawn bytes' 'FAIL' "HEAD=$head apply=[$($codes -join ',')] mismatch=[$($mism -join ',')] 0003 bit0-only=$p3ok byte test exit $($t.Code)" }
}

# 3. static exe pin: the two instruction ranges behind the auto-finish rule (read from the file, never run)
Run-Check 3 'exe static pin' {
    $env:CARGO_TARGET_DIR = $DisTarget
    try { $b = Invoke-Tool 'cargo' @('build', '--release', '--manifest-path', (Join-Path $Root 'tools\iw4dis\Cargo.toml')) 'iw4dis_build' }
    finally { Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
    $dis = Join-Path $DisTarget 'release\iw4dis.exe'
    if ($b.Code -ne 0 -or -not (Test-Path -LiteralPath $dis) -or -not (Test-Path -LiteralPath $ErExe)) {
        Add-Result 3 'exe static pin' 'FAIL' "iw4dis build exit $($b.Code); exe present: $(Test-Path -LiteralPath $ErExe)"
        return
    }
    $a = Invoke-Tool $dis @($ErExe, 'x', '0x14038e66a-0x14038e676') 'iw4dis_copy'
    $c = Invoke-Tool $dis @($ErExe, 'x', '0x1403901a3-0x1403901b3') 'iw4dis_update'
    $d = Invoke-Tool $dis @($ErExe, 'x', '0x14038ba2f-0x14038ba3a') 'iw4dis_ctor'
    $e4 = Invoke-Tool $dis @($ErExe, 'x', '0x1403a5bd9-0x1403a5be0') 'iw4dis_code4'
    $want = @(
        @($a.Out, '14038e66a\s+movzx eax,byte ptr \[rdi\+44h\]'), @($a.Out, '14038e66e\s+and al,1'), @($a.Out, '14038e670\s+mov \[rbx\+194h\],al'),
        @($c.Out, '1403901a3\s+cmp byte ptr \[rdi\+4C4h\],0'), @($c.Out, '1403901ac\s+mov byte ptr \[rdi\+329h\],0'),
        @($d.Out, '14038ba2f\s+mov qword ptr \[rsi\+194h\],1'), @($e4.Out, '1403a5bd9\s+mov dword ptr \[r14\],4')
    )
    $miss = @($want | Where-Object { $_[0] -notmatch $_[1] } | ForEach-Object { $_[1] })
    if ($miss.Count -eq 0) {
        Add-Result 3 'exe static pin' 'PASS' 'eldenring.exe (file): 0x14038e66a movzx eax,[rdi+44h] / and al,1 / mov [rbx+194h],al; 0x1403901a3 cmp [rdi+4C4h],0 .. mov [rdi+329h],0; ctor default [rsi+194h]=1; code 4 at 0x1403a5bd9'
    } else { Add-Result 3 'exe static pin' 'FAIL' "exe changed? not found: $($miss -join ' | ')" }
}

# 4. sound conversion: converted\nuke known answers, fresh conversion == disk, deterministic, PCM == source, self-test
Run-Check 4 'sound conversion' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_nuke.py'), '--check') 'convert_nuke_check'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_nuke.py'), '--self-test') 'convert_nuke_selftest'
    $line = Last-Line $a.Out '^(PASS|FAIL) nuke converter'
    $ok = $a.Code -eq 0 -and $line -match '^PASS nuke converter' -and $line -match 'deterministic' -and $s.Code -eq 0 -and $s.Out -match 'PASS self-test: (\d+) mutations caught' -and [int]$Matches[1] -ge 8
    Add-Result 4 'sound conversion' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f (Cut $line 300), (Last-Line $s.Out '(PASS|FAIL) self-test'))
}

# 5. regulation: build + round trip + SAFETY_M7 + self-test mutations; every M6 row byte-identical; sha pin
Run-Check 5 'regulation' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py')) 'build_regulation'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py'), '--self-test') 'build_regulation_selftest'
    $rr = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm7_regulation_rows.py'), '--check') 'm6rows_check'
    $rs = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm7_regulation_rows.py'), '--self-test') 'm6rows_selftest'
    $pkg = Join-Path $Root 'build\package\regulation.bin'
    $line = Last-Line $r.Out '^REGULATION'
    $built = $r.Code -eq 0 -and $line -match '^REGULATION PASS bullet\+342 atk\+278 speffect\+39 '
    $sha = if (-not $built) { 'not-compared(build-failed)' } elseif (Test-Path -LiteralPath $pkg) { (Get-FileHash -LiteralPath $pkg -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' }
    $pin = if (Test-Path -LiteralPath $RegPin) { ((Get-Content -LiteralPath $RegPin -Raw).Trim() -split '\s+')[0].ToLowerInvariant() } else { 'no pin file' }
    $nk = Last-Line $r.Out '^\s+nuke: SpEffectParam'
    $ok = $built -and $s.Code -eq 0 -and $s.Out -match 'REGULATION SELF-TEST PASS (\d+)/\1 ' -and [int]$Matches[1] -ge 37 -and $sha -eq $pin -and $rr.Code -eq 0 -and $rs.Code -eq 0 -and $nk -ne ''
    Add-Result 5 'regulation' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; {1}; package sha256 {2} (pin {3}); {4}; {5}; {6}" -f $line, (Last-Line $s.Out 'SELF-TEST'), $sha.Substring(0, [Math]::Min(12, $sha.Length)), $pin.Substring(0, [Math]::Min(12, $pin.Length)), (Cut (Last-Line $rr.Out '^(PASS|FAIL)') 200), (Last-Line $rs.Out '(PASS|FAIL) self-test'), (Cut $nk 200))
}

# 6. cargo build: the whole workspace with 0 warnings under mod\
Run-Check 6 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)` \(' -or $_ -match '\\mod\\') })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Vmtest)) {
        Add-Result 6 'cargo build' 'PASS' "release workspace; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 6 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s): $(($ours | Select-Object -First 2) -join ' | ')" }
}

# 7. cargo test: every 6.1 category by name prefix, the M7 real-data tests, the workspace
Run-Check 7 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '--workspace', '--release') 'cargo_test' $Mod
    $okNames = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { (($_ -split ' ')[1] -split '::')[-1] })
    $cats = @('emitter_bench_', 'emitter_pool_', 'emitter_guard_', 'cadence_clamp_', 'cadence_median_within_one_frame', 'reload_switch_empty_mag',
              'rechamber_cancel_emits', 'reload_rechamber_pairing_property', 'killstreak_', 'nuke_logic_timeline', 'nuke_logic_events_once',
              'nuke_logic_countdown_phase_and_cancel', 'nuke_logic_post_ramps', 'nuke_logic_shake', 'nuke_logic_gate_order',
              'nuke_logic_player_death_order', 'nuke_targets_census', 'nuke_targets_stagger', 'nuke_targets_torrent',
              'nuke_fx_known_answers', 'nuke_fx_bright_pass', 'nuke_data_', 'config_nuke_', 'config_keys_m7_', 'spawn_data_bytes',
              'hud_nuke_timer', 'render_nuke_shaders', 'spawn_cause_classify')
    $missing = @($cats | Where-Object { $p = $_; -not ($okNames | Where-Object { $_.StartsWith($p) }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $env:MW2ER_ROOT = $Root
    try { $rd = Invoke-Tool 'cargo' @('test', '--release', '-p', 'mw2er-core', '--test', 'real_data_m7', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data_m7' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $rdPass = @($rd.Out -split "`n" | Where-Object { $_ -match 'PASS:' })
    $n = @($cats | ForEach-Object { $p = $_; @($okNames | Where-Object { $_.StartsWith($p) }).Count } | Measure-Object -Sum).Sum
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $rd.Code -eq 0 -and $rdPass.Count -eq 2) {
        Add-Result 7 'cargo test' 'PASS' "$($okNames.Count) workspace tests ok; all $($cats.Count) M7 categories ($n tests); real data: $(($rdPass | ForEach-Object { Cut $_ 90 }) -join ' | ')"
    } else { Add-Result 7 'cargo test' 'FAIL' "exit $($c.Code); missing categories: $($missing -join ', '); failed: $($failed -join ' | '); real data exit $($rd.Code)" }
}

# 8. vmtest nuke (debug layer, then GPU-based validation) + the 4K perf
Run-Check 8 'vmtest nuke' {
    $v = Invoke-Tool $Vmtest @('--nuke', '--out', $VmOut) 'vmtest_nuke'
    $g = Invoke-Tool $Vmtest @('--nuke', '--gbv', '--out', (Join-Path $VmOut 'gbv')) 'vmtest_nuke_gbv'
    $p = Invoke-Tool $Vmtest @('--nuke-perf', '--out', $VmOut) 'vmtest_nuke_perf'
    $cases = @($v.Out -split "`n" | Where-Object { $_ -match '^PASS (N\d|Ninit|infoqueue)' }).Count
    $pngs = @(Get-ChildItem -LiteralPath $VmOut -Filter 'nuke_*.png' -ErrorAction SilentlyContinue).Count
    $ok = $v.Code -eq 0 -and $v.Out -match 'VMTEST NUKE PASS' -and $cases -eq 8 -and $g.Code -eq 0 -and $g.Out -match 'VMTEST NUKE PASS' -and $p.Code -eq 0 -and $p.Out -match 'NUKE PERF PASS' -and $pngs -ge 7
    Add-Result 8 'vmtest nuke' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} cases PASS (debug layer) + GBV {1}; {2}; {3} PNGs in work\m7\vm" -f $cases, (Last-Line $g.Out '^VMTEST NUKE'), (Cut (Last-Line $p.Out '^NUKE PERF') 200), $pngs)
}

# 9. config: the shipped mw2er.toml has the M7 sections with their defaults; the DLL parser accepts it; the M7 config tests
Run-Check 9 'config' {
    $code = @'
import sys, tomllib
d = tomllib.load(open(sys.argv[1], 'rb'))
n, s, k, r = d.get('nuke', {}), d.get('spawn', {}), d.get('keys', {}), d.get('regulation', {}).get('nuke', {})
bad = []
want = {'enabled': True, 'streak_cost': 25, 'keep_earned_on_death': True, 'radius_m': 200.0,
        'spare_friendly': True, 'spare_teams': [0, 2, 8, 10, 12, 20, 26, 28, 30, 47], 'kill_mode': 'speffect', 'player_dies': True,
        'player_delay_s': 3.0, 'flash': True, 'shake': True, 'shake_scale': 0.6, 'shake_s': 10.0, 'horizon_count': 3, 'announcer': 'US'}
for key, w in want.items():
    if n.get(key) != w: bad.append('nuke.%s=%r' % (key, n.get(key)))
# user testing switches (like ac130.cooldown_enabled): any bool is accepted
for key in ('always_available', 'dry_run'):
    if not isinstance(n.get(key), bool): bad.append('nuke.%s=%r (want a bool)' % (key, n.get(key)))
if (s.get('emitter_warn'), s.get('emitter_throttle'), s.get('emitter_reserve')) != (192, 224, 32): bad.append('[spawn] %r' % s)
if k.get('nuke') != '4': bad.append('keys.nuke=%r' % k.get('nuke'))
used = {str(v) for kk, v in k.items() if kk != 'nuke' and v}
if '4' in used: bad.append('key 4 is used by another binding')
if (r.get('lethal_speffect_id'), r.get('lethal_bullet_id'), r.get('horizon_bullet_id')) != (79000960, 79000961, 79000962): bad.append('[regulation.nuke] %r' % r)
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('tomllib ok: [nuke] defaults (cost 25, radius 200, deny-list spare teams, speffect kills, player dies, shake 0.6x10), [spawn] 192/224/32, keys.nuke = 4 (free), [regulation.nuke] 79000960..962')
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::') 'config_m7_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'config_nuke_defaults_and_bounds \.\.\. ok' -and $u.Out -match 'config_keys_m7_nuke_key_free \.\.\. ok' -and $u.Out -match 'config_bad_key_name_is_error \.\.\. ok'
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok) { Add-Result 9 'config' 'PASS' ("{0}; {1}; config M7 tests ok (defaults, bounds, unknown key, key 4 conflicts)" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 9 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code); config tests exit $($u.Code)" }
}

# 10. wiring: the M7 checks with their mutations + the M6 / M4 / M5 wiring they touch
Run-Check 10 'wiring' {
    $n = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'nukewiring', '--self-test') 'nukewiring'
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'arsenalwiring', '--self-test') 'arsenalwiring'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ac130wiring', '--self-test') 'ac130wiring'
    $m = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movewiring', '--self-test') 'movewiring'
    $ok = $n.Code -eq 0 -and $n.Out -match 'self-test: (\d+) mutations of the M7 sources caught' -and [int]$Matches[1] -ge 10 -and $a.Code -eq 0 -and $b.Code -eq 0 -and $m.Code -eq 0
    Add-Result 10 'wiring' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | arsenal {2} ac130 {3} move {4}" -f (Last-Line $n.Out 'self-test'), (Cut (Last-Line $n.Out '^(PASS|FAIL)') 200), (Last-Line $a.Out '^(PASS|FAIL)').Substring(0, 4), (Last-Line $b.Out '^(PASS|FAIL)').Substring(0, 4), (Last-Line $m.Out '^(PASS|FAIL)').Substring(0, 4))
}

# 11. DLL proof strings (built == staged; check_m2 inside check 1 stages it)
Run-Check 11 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'nukestrings', $BuiltDll) 'nukestrings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    if ($a.Code -eq 0 -and $same) { Add-Result 11 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 11 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 12. analyzer: the self-test (M7 fixture + mutations + the real run-14 excerpt), and the two fixtures through the CLI
Run-Check 12 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $f = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m7_nuke_session.log')) 'analyze_m7_fixture'
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\run14_leak.log')) 'analyze_run14_fixture'
    $m7Rows = @($f.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(emitter pool|spawn causes|ac130 damage|reload pairing|nuke streak|nuke sequence|nuke kills|nuke player|nuke post|nuke cancel)\s' }).Count
    $a1 = @($g.Out -split "`n" | Where-Object { $_ -match '^FAIL\s+(emitter pool|spawn causes|ac130 damage|reload pairing)\s' }).Count
    $okSelf = $r.Code -eq 0 -and $r.Out -match 'M7 fixture m7_nuke_session\.log: good session PASS on all 10 M7 rows, (\d+) M7 mutations FAIL' -and [int]$Matches[1] -ge 10 -and $r.Out -match 'real run-14 excerpt'
    $ok = $okSelf -and $m7Rows -eq 10 -and $a1 -eq 4
    Add-Result 12 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0}; M7 fixture: {1}/10 M7 rows PASS; run-14 excerpt: {2}/4 A1/A3 rows FAIL as reported ({3})" -f $r.Code, $m7Rows, $a1, (Last-Line $g.Out 'IN-GAME ORACLES'))
}

# 13. hygiene: converted\ + work\ ignored, no MW2 gsc / weapon-file text in our sources, static hygiene, install folders ==
#     vanilla baseline (listing only), the game's regulation untouched, nothing written under the game folders
Run-Check 13 'hygiene' {
    $problems = New-Object System.Collections.ArrayList
    $gi = Get-Content -LiteralPath (Join-Path $Root '.gitignore') -ErrorAction SilentlyContinue
    foreach ($x in '/converted/', '/work/') { if (-not ($gi | Where-Object { $_.Trim() -eq $x })) { [void]$problems.Add("$x not in .gitignore") } }
    $code = @'
import os, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
gsc = (root / 'dumps' / 'm7' / 'common_mp' / 'maps' / 'mp' / 'killstreaks' / '_nuke.gsc').read_text(encoding='latin-1')
lines = {ln.strip() for ln in gsc.splitlines() if len(re.sub(r'\s', '', ln)) >= 20}
files = []
for base in (root / 'mod', root / 'tools' / 'scripts', root / 'docs'):
    for dp, dn, fn in os.walk(base):
        dn[:] = [d for d in dn if d not in ('target', '__pycache__', 'fixtures')]
        files += [pathlib.Path(dp) / f for f in fn if pathlib.Path(f).suffix.lower() in ('.rs', '.py', '.ps1', '.toml', '.hlsl', '.md', '.json')]
hits = []
for p in files:
    t = p.read_text(encoding='utf-8', errors='replace')
    for ln in lines:
        if ln in t:
            hits.append('%s: %s' % (p.relative_to(root), ln[:40]))
            break
out = root / 'converted' / 'nuke' / 'nuke.json'
if out.is_file() and any(ln in out.read_text(encoding='utf-8') for ln in lines):
    hits.append('converted/nuke/nuke.json carries _nuke.gsc text')
if hits:
    print('FAIL ' + '; '.join(hits[:4])); sys.exit(1)
print('PASS %d source files under mod\\, tools\\scripts\\ and docs\\ + nuke.json: none contains any of the %d lines (>= 20 chars) of _nuke.gsc' % (len(files), len(lines)))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_gsc'
    if ($t.Code -ne 0) { [void]$problems.Add($t.Out.Trim()) }
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
    if ($problems.Count -eq 0) { Add-Result 13 'hygiene' 'PASS' ("converted\ and work\ ignored; {0}; {1}; install folders == vanilla baseline; {2}" -f (Cut $t.Out.Trim() 200), (Cut (Last-Line $h.Out '^PASS') 80), (Last-Line $gf.Out '^PASS')) }
    else { Add-Result 13 'hygiene' 'FAIL' ($problems -join ' | ') }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run 15, docs\M7_DESIGN.md section 8): that the auto-finish bit keeps the emitter pool bounded in game and the AC-130 deals damage again, that a one-shot 100% max-HP SpEffect applied through the engine kills with runes / drops / boss flags, the team offset, the player''s normal YOU DIED, that ER ignores key 4, the visual-only horizon row, the post pass and shake on the real swap chain, and every nuke sound in game.'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM6 -and $results.Count -eq 13) {
    Write-Output 'M7 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M7 OFFLINE ORACLES: FAIL'
exit 1
