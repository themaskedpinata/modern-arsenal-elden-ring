<#
.SYNOPSIS
  Runs every offline oracle of milestone M6 (the 34 MW2 multiplayer guns + the run-13 bug fixes) after the whole M5
  suite (which runs M4, M3b, M3 and M2 first) and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m6\ and work\m6\
  (vmtest renders, the inventory copy) plus what the sub-checks write; Blender (already used by M3) renders the
  reference silhouettes headless into work\m6\vm\<gun>\. The install folders are only listed. Exit code 0 only if all
  12 checks PASS; a SKIP is a failure unless -AllowSkip. -SkipM5 skips check 1 for fast iteration but always exits
  non-zero.

  Checks (docs\M6_DESIGN.md section 14):
   1 M5 suite  2 inventory fresh  3 converter  4 regulation + sha pin  5 cargo build  6 cargo test (13.3 categories +
   real data)  7 vmtest arsenal (+ Blender IoU on 8 guns)  8 config  9 wiring  10 DLL proof strings  11 analyzer
   12 hygiene + no game-folder writes

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m6.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM5)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Blender = 'C:\Program Files\Blender Foundation\Blender 5.2\blender.exe'
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Vmtest = Join-Path $Mod 'target\release\vmtest.exe'
$LogDir = Join-Path $Root 'work\check_m6'
$VmOut = Join-Path $Root 'work\m6\vm'
$RegPin = Join-Path $Root 'work\m6\regulation.sha256'
$ErGame = 'D:\SteamLibrary\steamapps\common\ELDEN RING\Game'
$Mw2Dir = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2'
$IouGuns = @('m4_mp', 'ump45_mp', 'mg4_mp', 'cheytac_mp', 'spas12_mp', 'striker_mp', 'rpg_mp', 'javelin_mp')
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

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

function Run-Check([string]$Id, [string]$Name, [scriptblock]$Body) {
    try { & $Body } catch { Add-Result $Id $Name 'FAIL' "exception: $_" }
}

Write-Output "M6 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m6\"
Write-Output ''

# 1. the whole M5 suite first (check_m4 -> check_m3b -> check_m3 -> check_m2, which stages the DLL)
if ($SkipM5) {
    Add-Result 1 'M5 suite' 'SKIP' '-SkipM5 given (the run cannot pass)'
} else {
    Run-Check 1 'M5 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m5.ps1')) 'check_m5'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        if ($r.Code -eq 0 -and $r.Out -match 'M5 OFFLINE ORACLES: PASS' -and $pass -ge 11 -and $bad.Count -eq 0) {
            Add-Result 1 'M5 suite' 'PASS' "check_m5.ps1: $pass PASS ($(Last-Line $r.Out '^SUMMARY'))"
        } else { Add-Result 1 'M5 suite' 'FAIL' "exit $($r.Code); $pass PASS; $($bad -join ' | ')" }
    }
}

# 2. inventory fresh: a new run over the dumps equals work\m6\inventory.json, 34 guns
Run-Check 2 'inventory fresh' {
    $fresh = Join-Path $LogDir 'inventory_fresh.json'
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'inventory_m6.py'), '--out', $fresh, '--quiet') 'inventory'
    $code = @'
import json, sys
a = json.load(open(sys.argv[1], encoding='utf-8')); b = json.load(open(sys.argv[2], encoding='utf-8'))
n = len(b.get('guns', {}))
if a != b:
    diff = sorted(k for k in set(a['guns']) | set(b['guns']) if a['guns'].get(k) != b['guns'].get(k))
    print('FAIL work\\m6\\inventory.json is stale for %s (rerun inventory_m6.py, then convert_arsenal.py)' % diff[:5]); sys.exit(1)
if n != 34:
    print('FAIL %d guns in the inventory (want 34)' % n); sys.exit(1)
print('PASS inventory_m6.py over the dumps == work\\m6\\inventory.json (%d guns)' % n)
'@
    $c = Invoke-Tool $Py @('-I', '-c', $code, $fresh, (Join-Path $Root 'work\m6\inventory.json')) 'inventory_compare'
    if ($r.Code -eq 0 -and $c.Code -eq 0) { Add-Result 2 'inventory fresh' 'PASS' $c.Out.Trim() }
    else { Add-Result 2 'inventory fresh' 'FAIL' "inventory exit $($r.Code); $($c.Out.Trim())" }
}

# 3. converter: known answers of section 13.1 on the written files, fresh conversion == disk, determinism, self-test
Run-Check 3 'converter' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_arsenal.py'), '--check') 'convert_check'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_arsenal.py'), '--self-test') 'convert_selftest'
    $line = Last-Line $a.Out '^(PASS|FAIL) arsenal converter'
    $ok = $a.Code -eq 0 -and $line -match '^PASS arsenal converter: 34/34 guns PASS' -and $line -match 'deterministic' -and $b.Code -eq 0 -and $b.Out -match 'PASS self-test'
    Add-Result 3 'converter' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f $line, (Last-Line $b.Out '(PASS|FAIL)'))
}

# 4. regulation: build + round trip + SAFETY_M6 + self-test mutations; package sha == work\m6\regulation.sha256
Run-Check 4 'regulation' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py')) 'build_regulation'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py'), '--self-test') 'build_regulation_selftest'
    $pkg = Join-Path $Root 'build\package\regulation.bin'
    # review M6: a failed build leaves the previous package behind; its sha is not evidence, so it is not compared
    $built = $r.Code -eq 0 -and (Last-Line $r.Out '^REGULATION') -match '^REGULATION PASS '
    $sha = if (-not $built) { 'not-compared(build-failed)' } elseif (Test-Path -LiteralPath $pkg) { (Get-FileHash -LiteralPath $pkg -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' }
    $pin = if (Test-Path -LiteralPath $RegPin) { ((Get-Content -LiteralPath $RegPin -Raw).Trim() -split '\s+')[0].ToLowerInvariant() } else { 'no pin file' }
    $code = @'
import json, sys
r = json.load(open(sys.argv[1], encoding='utf-8'))
a = r.get('arsenal') or {}
g = a.get('guns', {})
u = [(b['id'], b['atk']) for b in g.get('ump45_mp', {}).get('bands', [])]
bad = []
if len(g) != 34: bad.append('%d guns' % len(g))
if a.get('mode') != 'flat_inv' or a.get('bullet_sfx') != 'none': bad.append('mode %s sfx %s' % (a.get('mode'), a.get('bullet_sfx')))
if u != [(79112000 + i, x) for i, x in enumerate([103, 101, 100, 98, 97, 96])]: bad.append('ump45 bands %s' % u)
n = sum(len(v.get('bands', [])) for v in g.values()); nl = sum(1 for v in g.values() if v.get('launcher'))
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('rows.json arsenal: 34 guns, %d bands, %d launchers, flat_inv, sfx none, UMP45 79112000..005 atk 103..96' % (n, nl))
'@
    $j = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'build\rows.json')) 'rows_arsenal'
    $line = Last-Line $r.Out '^REGULATION'
    $ok = $r.Code -eq 0 -and $line -match '^REGULATION PASS ' -and $s.Code -eq 0 -and $s.Out -match 'REGULATION SELF-TEST PASS (\d+)/\1 ' -and $sha -eq $pin -and $j.Code -eq 0
    Add-Result 4 'regulation' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; {1}; package sha256 {2} (pin {3}); {4}" -f $line, (Last-Line $s.Out 'SELF-TEST'), $sha.Substring(0, [Math]::Min(12, $sha.Length)), $pin.Substring(0, [Math]::Min(12, $pin.Length)), $j.Out.Trim())
}

# 5. cargo build: the whole workspace with 0 warnings under mod\
Run-Check 5 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)` \(' -or $_ -match '\\mod\\') })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Vmtest)) {
        Add-Result 5 'cargo build' 'PASS' "release workspace; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 5 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s): $(($ours | Select-Object -First 2) -join ' | ')" }
}

# 6. cargo test: every 13.3 category by name prefix, the M6 real-data tests, the workspace
Run-Check 6 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '--workspace', '--release') 'cargo_test' $Mod
    $okNames = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { (($_ -split ' ')[1] -split '::')[-1] })
    $cats = @('fire_mode_burst_', 'fire_mode_double_barrel', 'rechamber_', 'shell_reload_', 'pellets_', 'projectile_', 'lock_',
              'switching_', 'kick_', 'breath_', 'sway_', 'arsenal_', 'spawn_cause_', 'spawn_pellet_gate', 'breaker_per_shot',
              'ac130_edges_', 'ac130_abort_', 'ac130_mask_talk_', 'regulation_', 'config_arsenal_', 'config_keys_m6_',
              'vm_params_rates_', 'vm_notes_', 'sounds_gun_')
    $missing = @($cats | Where-Object { $p = $_; -not ($okNames | Where-Object { $_.StartsWith($p) }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $env:MW2ER_ROOT = $Root
    try { $rd = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'real_data_m6', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data_m6' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $rdPass = @($rd.Out -split "`n" | Where-Object { $_ -match 'PASS:' })
    $n = @($cats | ForEach-Object { $p = $_; @($okNames | Where-Object { $_.StartsWith($p) }).Count } | Measure-Object -Sum).Sum
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $rd.Code -eq 0 -and $rdPass.Count -eq 2) {
        Add-Result 6 'cargo test' 'PASS' "$($okNames.Count) workspace tests ok; all $($cats.Count) M6 categories ($n tests); real data: $(($rdPass | ForEach-Object { $_.Substring(0, [Math]::Min(80, $_.Length)) }) -join ' | ')"
    } else { Add-Result 6 'cargo test' 'FAIL' "exit $($c.Code); missing categories: $($missing -join ', '); failed: $($failed -join ' | '); real data exit $($rd.Code)" }
}

# 7. vmtest arsenal (debug layer): AR1-AR6, then Blender reference silhouettes of 8 guns and the IoU compare
Run-Check 7 'vmtest arsenal' {
    $v = Invoke-Tool $Vmtest @('--arsenal', '--out', $VmOut) 'vmtest_arsenal'
    $bl = @()
    foreach ($g in $IouGuns) {
        $b = Invoke-Tool $Blender @('-b', '--factory-startup', '--python', (Join-Path $Scripts 'blender_ref.py'), '--', '--gun', $g) "blender_ref_$g"
        if ($b.Code -ne 0 -or -not ($b.Out -match "PASS blender_ref --gun $g")) { $bl += "$g ($(Last-Line $b.Out '^FAIL'))" }
    }
    $c = Invoke-Tool $Vmtest @('--compare-arsenal', $VmOut) 'vmtest_compare_arsenal'
    $iou = Last-Line $c.Out '^IOU ARSENAL'
    $cases = @($v.Out -split "`n" | Where-Object { $_ -match '^PASS (AR\d|infoqueue)' }).Count
    $ok = $v.Code -eq 0 -and $v.Out -match 'VMTEST ARSENAL PASS' -and $cases -eq 7 -and $bl.Count -eq 0 -and $c.Code -eq 0 -and $iou -match '^IOU ARSENAL PASS \((\d+) masks\)' -and [int]$Matches[1] -ge 24
    $ar1 = Last-Line $v.Out '^(PASS|FAIL) AR1'
    Add-Result 7 'vmtest arsenal' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} cases PASS ({1}); Blender refs failed: [{2}]; {3}" -f $cases, $ar1.Substring(0, [Math]::Min(140, $ar1.Length)), ($bl -join ', '), $iou)
}

# 8. config: the shipped mw2er.toml has the M6 sections with their defaults; the DLL parser accepts it; the M6 config tests
Run-Check 8 'config' {
    $code = @'
import json, sys, tomllib
d = tomllib.load(open(sys.argv[1], 'rb'))
idx = [g['name'] for g in json.load(open(sys.argv[2], encoding='utf-8'))['guns']]
a, l, k, dm, ra = d.get('arsenal', {}), d.get('launchers', {}), d.get('keys', {}), d.get('damage', {}), d.get('regulation', {}).get('arsenal', {})
bad = []
if a.get('guns') != idx: bad.append('[arsenal] guns != converted\\guns\\index.json order')
for key, want in {'enabled': True, 'dir': 'converted\\guns', 'start': 'ump45_mp', 'reserve_source': 'max', 'preload': 'all',
                  'pellet_pool_gate': True, 'owner_retry': True}.items():
    if a.get(key) != want: bad.append('arsenal.%s=%r' % (key, a.get(key)))
if l.get('flight') != 'dll' or l.get('explosion_tiers') != 2: bad.append('[launchers] %r' % l)
if not d.get('kick', {}).get('enabled'): bad.append('[kick] enabled')
if (k.get('weapon_next'), k.get('weapon_prev')) != ('1', '2'): bad.append('keys.weapon_next/prev')
if (dm.get('mode'), dm.get('s'), dm.get('d_ref')) != ('flat_inv', 1.225, 85.0): bad.append('[damage] mode/s/d_ref')
if (ra.get('id_base'), ra.get('bullet_sfx')) != (79100000, 'none'): bad.append('[regulation.arsenal]')
if d.get('ac130', {}).get('start_backoff_s') != 1.0: bad.append('ac130.start_backoff_s')
if d.get('movement', {}).get('weapon_move_class') != 'auto': bad.append('movement.weapon_move_class')
if not all(d.get('hud', {}).get(x) for x in ('weapon_name', 'switch_list', 'lock_ui')): bad.append('[hud] M6 flags')
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('tomllib ok: [arsenal] %d guns in index order, [launchers], [kick], keys 1/2, [damage] flat_inv s=1.225 d_ref=85, [regulation.arsenal], start_backoff_s, weapon_move_class=auto, hud flags' % len(idx))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml'), (Join-Path $Root 'converted\guns\index.json')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::tests::config_') 'config_m6_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'config_arsenal_sections_and_alias \.\.\. ok' -and $u.Out -match 'config_keys_m6_conflicts \.\.\. ok' -and $u.Out -match 'config_bad_key_name_is_error \.\.\. ok'
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok) { Add-Result 8 'config' 'PASS' ("{0}; {1}; config M6 tests ok (sections, alias, key conflicts, unknown key)" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 8 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code); config tests exit $($u.Code)" }
}

# 9. wiring: the M6 arsenal checks + the AC-130 / movement wiring they touch, each with its mutations
Run-Check 9 'wiring' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'arsenalwiring', '--self-test') 'arsenalwiring'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ac130wiring', '--self-test') 'ac130wiring'
    $m = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movewiring', '--self-test') 'movewiring'
    $ok = $a.Code -eq 0 -and $a.Out -match 'self-test: (\d+) mutations of the M6 sources caught' -and [int]$Matches[1] -ge 10 -and $b.Code -eq 0 -and $b.Out -match 'mutations of the M4 sources caught' -and $m.Code -eq 0 -and $m.Out -match 'mutations of the M5 sources caught'
    Add-Result 9 'wiring' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | ac130wiring {2} | movewiring {3}" -f (Last-Line $a.Out 'self-test'), (Last-Line $a.Out '^(PASS|FAIL)').Substring(0, [Math]::Min(160, (Last-Line $a.Out '^(PASS|FAIL)').Length)), (Last-Line $b.Out '^(PASS|FAIL)').Substring(0, 4), (Last-Line $m.Out '^(PASS|FAIL)').Substring(0, 4))
}

# 10. DLL proof strings (built == staged; check_m2 inside check 1 stages it)
Run-Check 10 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'arsenalstrings', $BuiltDll) 'arsenalstrings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    if ($a.Code -eq 0 -and $same) { Add-Result 10 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 10 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 11. analyzer: the self-test (M6 fixture + mutations + the real run-13 excerpt), and the two fixtures through the CLI
Run-Check 11 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $f = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m6_arsenal_session.log')) 'analyze_m6_fixture'
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\run13_bugs.log')) 'analyze_run13_fixture'
    $m6Rows = @($f.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(arsenal|switching|shots per gun|hits per gun|ump45 calibration|burst|rechamber|shell reload|pellets|launchers|lock|ac130 start|spawn)\s(?!causes)' }).Count   # M7 adds a 'spawn causes' row
    $bug1 = @($g.Out -split "`n" | Where-Object { $_ -match '^FAIL\s+ac130 start\s' }).Count
    $bug2 = @($g.Out -split "`n" | Where-Object { $_ -match '^NEEDS-DATA\s+spawn\s' }).Count
    $ok = $r.Code -eq 0 -and $r.Out -match 'M6 fixture m6_arsenal_session\.log: good session PASS on all 13 M6 rows, (\d+) mutations FAIL' -and [int]$Matches[1] -ge 10 -and $r.Out -match 'real run-13 excerpt: ac130 start FAIL' -and $f.Code -eq 0 -and $m6Rows -eq 13 -and $bug1 -eq 1 -and $bug2 -eq 1
    Add-Result 11 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0}; M6 fixture: {1}/13 M6 rows PASS ({2}); run-13 excerpt: ac130 start FAIL={3}, spawn NEEDS-DATA={4}" -f $r.Code, $m6Rows, (Last-Line $f.Out 'IN-GAME ORACLES'), $bug1, $bug2)
}

# 12. hygiene: converted\ + work\ ignored, no MW2 weapon-file key\value pair in our sources, static hygiene, install
#     folders == vanilla baseline (listing only), the vanilla regulation untouched
Run-Check 12 'hygiene' {
    $problems = New-Object System.Collections.ArrayList
    $gi = Get-Content -LiteralPath (Join-Path $Root '.gitignore') -ErrorAction SilentlyContinue
    foreach ($x in '/converted/', '/work/') { if (-not ($gi | Where-Object { $_.Trim() -eq $x })) { [void]$problems.Add("$x not in .gitignore") } }
    $code = @'
import os, pathlib, sys
root = pathlib.Path(sys.argv[1])
pairs = set()
nw = 0
for f in sorted((root / 'dumps' / 'common_mp' / 'weapons').glob('*_mp')):
    nw += 1
    t = f.read_bytes().decode('latin-1').split('\\')
    for k, v in zip(t[1::2], t[2::2]):
        if v.strip() and len(k) + len(v) >= 10:
            pairs.add(k + '\\' + v)
files = []
for base in (root / 'mod', root / 'tools' / 'scripts', root / 'docs'):
    for dp, dn, fn in os.walk(base):
        dn[:] = [d for d in dn if d not in ('target', '__pycache__', 'fixtures')]
        files += [pathlib.Path(dp) / f for f in fn if pathlib.Path(f).suffix.lower() in ('.rs', '.py', '.ps1', '.toml', '.hlsl', '.md', '.json')]
import re
tok = re.compile(r'[A-Za-z_0-9]+\\[^\\\r\n' + chr(34) + chr(39) + ']+')  # no double quote in -c code (PS 5.1 strips it)
hits = []
for p in files:
    text = p.read_text(encoding='utf-8', errors='replace')
    for m in tok.finditer(text):
        s = m.group(0)
        # a value may be followed by more text: try every prefix that ends where a weapon-file value could end
        if s in pairs or any(s[:i] in pairs for i in range(len(s) - 1, 0, -1) if s[i] in ' ,;)'):
            hits.append('%s: %s' % (p.relative_to(root), s[:40]))
            break
if hits:
    print('FAIL ' + '; '.join(hits[:4])); sys.exit(1)
print('PASS %d source files under mod\\, tools\\scripts\\ and docs\\: none contains any of the %d key\\value pairs of the %d MW2 MP weapon files' % (len(files), len(pairs), nw))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_text'
    if ($t.Code -ne 0) { [void]$problems.Add($t.Out.Trim()) }
    $h = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'hygiene') 'hygiene_static'
    if ($h.Code -ne 0) { [void]$problems.Add((Last-Line $h.Out 'FAIL')) }
    $baseline = Get-Content -LiteralPath (Join-Path $Scripts 'fixtures\install_baseline.json') -Raw | ConvertFrom-Json
    $new = @()
    foreach ($d in @($ErGame, $Mw2Dir)) {
        if (-not (Test-Path -LiteralPath $d)) { [void]$problems.Add("install folder missing: $d"); continue }
        $old = @($baseline.$d)
        if ($old.Count -eq 0) { [void]$problems.Add("no baseline for $d"); continue }
        $top = @(Get-ChildItem -LiteralPath $d -Force | ForEach-Object { $_.Name })
        $new += @($top | Where-Object { $old -notcontains $_ } | ForEach-Object { "$d\$_" })
    }
    # co-op (MODLOG 2026-10-19): Seamless Co-op running in the game process creates <ER Game>\SeamlessCoop\crashdumps (empty) on
    # its first start. Accepted only while it holds no file at all (a crash dump there FAILs: it may contain mw2er memory).
    $new = @($new | Where-Object { -not ((Split-Path -Leaf $_) -eq 'SeamlessCoop' -and (Split-Path -Parent $_) -eq $ErGame -and @(Get-ChildItem -LiteralPath $_ -Recurse -File -Force -ErrorAction SilentlyContinue).Count -eq 0) })
    if ($new.Count -gt 0) { [void]$problems.Add("top-level entries not in the vanilla baseline: $($new -join ', ')") }
    $gf = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    if ($gf.Code -ne 0) { [void]$problems.Add((Last-Line $gf.Out 'FAIL')) }
    if ($problems.Count -eq 0) { Add-Result 12 'hygiene' 'PASS' ("converted\ and work\ ignored; {0}; {1}; install folders == vanilla baseline; {2}" -f $t.Out.Trim(), (Last-Line $h.Out '^PASS').Substring(0, [Math]::Min(80, (Last-Line $h.Out '^PASS').Length)), (Last-Line $gf.Out '^PASS')) }
    else { Add-Result 12 'hygiene' 'FAIL' ($problems -join ' | ') }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run, docs\M6_DESIGN.md section 15): the in-game damage on c4311 with the flat_inv rows (49 close / ~43 far), that sfxId_Hit/Flick -1 and the pellet pool gate end the run-13 SPAWN FAIL bursts, the stale-edge fix against a real load, ER''s reaction to keys 1/2 and the wheel while blanked, the view kick / zoom write in ER''s frame, the scope overlay and sensitivity scaling, launcher flight / explosions / lock-on against ER characters, and every gun''s look and sound in game.'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM5 -and $results.Count -eq 12) {
    Write-Output 'M6 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M6 OFFLINE ORACLES: FAIL'
exit 1
