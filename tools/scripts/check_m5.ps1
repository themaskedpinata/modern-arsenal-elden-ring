<#
.SYNOPSIS
  Runs every offline oracle of milestone M5 (MW2 movement with sprinting) after the whole M4 suite (which runs M3b,
  M3 and M2 first) and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam: iw4sp.exe is only read as a file (the
  converter's static PE read and tools\iw4dis's disassembly). Writes only to work\check_m5\, work\m5\ (bench.json,
  the iw4dis target, the converter self-test copies) plus what the sub-checks write. The install folders are only
  listed. Exit code 0 only if all 11 checks PASS; a SKIP is a failure unless -AllowSkip. -SkipM4 skips check 1 for
  fast iteration but always exits non-zero.

  Checks (docs\M5_DESIGN.md section 13.5):
   1 M4 suite (incl. M3b, M3, M2)  2 cargo build (0 warnings, incl. movebench)  3 cargo test (13.1 categories, real
   data, the mw2er write-cost and HUD tests)  4 bench (B1-B12 at 30/60/144 Hz)  5 bench perf  6 converter (+ iw4dis
   provenance)  7 config  8 wiring  9 DLL proof strings  10 analyzer  11 hygiene

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m5.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM4)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mw2Exe = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2\iw4sp.exe'   # read only
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Bench = Join-Path $Mod 'target\release\movebench.exe'
$LogDir = Join-Path $Root 'work\check_m5'
$DisTarget = Join-Path $Root 'work\m5\iw4dis_target'
$ErGame = 'D:\SteamLibrary\steamapps\common\ELDEN RING\Game'
$Mw2Dir = 'D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2'
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

Write-Output "M5 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam (iw4sp.exe is read as a file only). Logs: work\check_m5\"
Write-Output ''

# 1. the whole M4 suite first (it runs check_m3b.ps1 -> check_m3.ps1 -> check_m2.ps1, which stages the DLL)
if ($SkipM4) {
    Add-Result 1 'M4 suite' 'SKIP' '-SkipM4 given (the run cannot pass)'
} else {
    Run-Check 1 'M4 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m4.ps1')) 'check_m4'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        $m3b = Last-Line $r.Out '^PASS\s+1 M3b suite'
        if ($r.Code -eq 0 -and $r.Out -match 'M4 OFFLINE ORACLES: PASS' -and $pass -ge 15 -and $bad.Count -eq 0 -and $m3b -match '8 PASS' -and $m3b -match '18 PASS' -and $m3b -match '21 PASS') {
            Add-Result 1 'M4 suite' 'PASS' "check_m4.ps1: $pass PASS + $(Last-Line $r.Out 'NEEDS-DATA 15') ($m3b)"
        } else { Add-Result 1 'M4 suite' 'FAIL' "exit $($r.Code); $pass PASS; $($bad -join ' | ')" }
    }
}

# 2. cargo build: the whole workspace incl. movebench with 0 warnings under mod\
Run-Check 2 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)` \(' -or $_ -match '\\mod\\') })
    $errors = @($r.Out -split "`n" | Where-Object { $_ -match '(^|\s)error(\[|:)' })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Bench)) {
        Add-Result 2 'cargo build' 'PASS' "release workspace (mw2er, mw2er-core, mw2er-render, vmtest, sndconv, movebench); 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 2 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s); $($errors.Count) error line(s): $((@($ours) + @($errors) | Select-Object -First 2) -join ' | ')" }
}

# 3. cargo test: >= 1 test per 13.1 category (name prefix), the real-data test, the mw2er lib tests
Run-Check 3 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core') 'cargo_test' $Mod
    $okNames = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { (($_ -split ' ')[1] -split '::')[-1] })
    $cats = @('pmove_cmd_scale_', 'pmove_accel_', 'pmove_friction_', 'pmove_jump_', 'pmove_land_', 'fall_', 'sprint_', 'stance_',
              'move_collide_', 'move_logic_', 'move_logic_gravity_', 'move_logic_resync_', 'move_logic_pushout_', 'move_input_',
              'move_axes_', 'move_aim_chest_', 'vm_anim_sprint_flag_', 'vm_pose_', 'spread_stance_', 'movement_data_',
              'config_movement_', 'fp_math_yaw_gate_controller_')
    $missing = @($cats | Where-Object { $p = $_; -not ($okNames | Where-Object { $_.StartsWith($p) }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $env:MW2ER_ROOT = $Root
    try { $rd = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'real_data_m5', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data_m5' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $rdPass = Last-Line $rd.Out 'PASS:'
    $g = Invoke-Tool 'cargo' @('test', '-p', 'mw2er', '--lib', '--release', '--', '--nocapture') 'cargo_test_mw2er' $Mod
    $gw = Last-Line $g.Out 'GAMEWRITE'
    $gok = $g.Code -eq 0 -and $g.Out -match 'game::tests::game_write_fast_and_safe \.\.\. ok' -and $g.Out -match 'hud::tests::hud_gunship_frame_uses_one_background_draw_list \.\.\. ok' -and $gw
    $n = @($cats | ForEach-Object { $p = $_; @($okNames | Where-Object { $_.StartsWith($p) }).Count } | Measure-Object -Sum).Sum
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $rd.Code -eq 0 -and $rdPass -and $gok) {
        Add-Result 3 'cargo test' 'PASS' "$($okNames.Count) core tests ok; all $($cats.Count) M5 categories ($n tests); real data: $($rdPass.Substring(0, [Math]::Min(110, $rdPass.Length))); mw2er write-cost ($gw) + hud draw-list tests ok"
    } else { Add-Result 3 'cargo test' 'FAIL' "exit $($c.Code); missing categories: $($missing -join ', '); failed: $($failed -join ' | '); real data exit $($rd.Code) ($rdPass); mw2er test exit $($g.Code) ($gw)" }
}

# 4. the headless bench: B1-B12 at 30/60/144 Hz against hand-computed MW2 numbers (writes work\m5\bench.json)
Run-Check 4 'bench' {
    $r = Invoke-Tool $Bench @('--check', '--root', $Root) 'movebench_check'
    $line = Last-Line $r.Out '^(PASS|FAIL) MOVEBENCH'
    $ok = $r.Code -eq 0 -and $line -match '^PASS MOVEBENCH 12/12 scenarios' -and $line -match '0 failed' -and (Test-Path -LiteralPath (Join-Path $Root 'work\m5\bench.json'))
    Add-Result 4 'bench' $(if ($ok) { 'PASS' } else { 'FAIL' }) $(if ($ok) { $line } else { "exit $($r.Code); $line; $((@($r.Out -split "`n" | Where-Object { $_ -match '^FAIL' }) | Select-Object -First 3) -join ' | ')" })
}

# 5. bench perf: 10,000 ticks of B9 at 30 Hz: pure CPU p95 < 50 us, rays_max <= max_rays_per_tick (48 since run 18)
Run-Check 5 'bench perf' {
    $r = Invoke-Tool $Bench @('--perf', '--root', $Root) 'movebench_perf'
    $line = Last-Line $r.Out 'PERF movebench'
    $ok = $r.Code -eq 0 -and $line -match '^PASS PERF'
    Add-Result 5 'bench perf' $(if ($ok) { 'PASS' } else { 'FAIL' }) $line
}

# 6. converter: known answers, the 5 self-test mutations, hashes; tools\iw4dis builds and prints every exe site as recorded
Run-Check 6 'converter' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--check') 'convert_check'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--self-test') 'convert_selftest'
    $env:CARGO_TARGET_DIR = $DisTarget
    try { $bd = Invoke-Tool 'cargo' @('build', '--release', '--manifest-path', (Join-Path $Root 'tools\iw4dis\Cargo.toml')) 'iw4dis_build' }
    finally { Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
    $dis = Join-Path $DisTarget 'release\iw4dis.exe'
    $st = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_movement.py'), '--sites') 'convert_sites'
    $sites = @($st.Out -split "`n" | Where-Object { $_ -match '^[0-9a-f]+\t' } | ForEach-Object { $f = $_.Trim() -split "`t"; [pscustomobject]@{ Va = $f[0]; Want = $f[1]; Name = $f[2] } })
    $prov = 'iw4dis not run'
    $provOk = $false
    if ($bd.Code -eq 0 -and (Test-Path -LiteralPath $dis) -and $sites.Count -gt 0) {
        $d = Invoke-Tool $dis @($Mw2Exe, 'site', (($sites | ForEach-Object { $_.Va }) -join ',')) 'iw4dis_sites'
        $lines = @($d.Out -split "`n")
        $miss = @($sites | Where-Object { $s = $_; -not ($lines | Where-Object { $_.StartsWith($s.Va + '  ') -and $_.Contains($s.Want) }) } | ForEach-Object { "$($_.Name)@$($_.Va)" })
        $provOk = $d.Code -eq 0 -and $miss.Count -eq 0
        $prov = if ($provOk) { "iw4dis: all $($sites.Count) instruction sites decode to the recorded operands" } else { "iw4dis exit $($d.Code); sites not matching: $($miss -join ', ')" }
    } else { $prov = "iw4dis build exit $($bd.Code); $($sites.Count) sites" }
    $needs = @($a.Out -split "`n" | Where-Object { $_ -match '^NEEDS-DATA' })
    $ok = $a.Code -eq 0 -and $a.Out -match 'PASS check:' -and $b.Code -eq 0 -and $b.Out -match 'PASS self-test: 5/5 mutations caught' -and $provOk
    $detail = "{0} | {1} | {2}{3}" -f (Last-Line $a.Out '^(PASS|FAIL)').Substring(0, [Math]::Min(160, (Last-Line $a.Out '^(PASS|FAIL)').Length)), (Last-Line $b.Out '^(PASS|FAIL) self-test'), $prov, $(if ($needs.Count) { " | NEEDS-DATA (exe changed): $($needs.Count) row(s)" } else { '' })
    Add-Result 6 'converter' $(if ($ok) { 'PASS' } else { 'FAIL' }) $detail
}

# 7. config: tomllib + the DLL parser; the documented [movement] / keys.move_* / yaw_forward_axis / stance_icon defaults;
#    unknown [movement] keys and duplicate movement keys rejected (core tests)
Run-Check 7 'config' {
    $code = @'
import tomllib, sys
d = tomllib.load(open(sys.argv[1], 'rb'))
m, k, fp, hud = d.get('movement', {}), d.get('keys', {}), d.get('first_person', {}), d.get('hud', {})
want = {'enabled': True, 'data': 'converted\\movement.json', 'fp_only': False, 'tp_anim': 'hybrid', 'sprint_mode': 'press',
        'crouch_mode': 'toggle', 'gostand_from_prone': 'stand', 'jump_slowdown': True, 'marathon': False, 'lightweight': False,
        'weapon_move_class': 'auto', 'speed_scale': 1.0, 'stance_eye': 'ratio', 'fall_damage': 'mw2', 'fall_damage_scale': 1.0,
        'lethal_fall': 'handback', 'freefall_handback_m': 30.0, 'hull_radius_m': 0.0, 'skin_m': 0.03, 'max_substep_s': 0.0167,
        'max_bumps': 3, 'max_rays_per_tick': 48, 'ground_normal': 'rays', 'walkable_normal_y': 0.7, 'er_pushout': True,
        'er_pushout_max_m': 0.3, 'er_move_handback_m': 0.3, 'er_move_hold_s': 0.5, 'interact_hold_s': 1.5, 'char_push_radius_m': 0.45,
        'use_movement_limit_signal': False, 'rearm_ticks': 10, 'publish_max_m': 1.0, 'strafe_sign': 1, 'proxy_sync_flag': False,
        'write_last_update': False, 'blank_er': ['SPACE', 'LSHIFT', 'C', 'LCTRL', 'F', 'X', 'Q', 'LALT'], 'view_bob': True,
        'land_dip': True, 'vm_stance_pose': True, 'keyprobe': True, 'log_every_ticks': 60,
        'ladder_er_attacks': False}  # M8 (updated in place): ladder_er_attacks; [movement.mantle] is judged by check_m8
bad = [x for x, v in want.items() if m.get(x) != v]
bad += ['keys.' + x for x, v in {'move_forward': 'W', 'move_back': 'S', 'move_left': 'A', 'move_right': 'D', 'move_sprint': 'LSHIFT',
        'move_jump': 'SPACE', 'move_crouch': 'C', 'move_prone': 'LCTRL', 'move_stance_hold': ''}.items() if k.get(x) != v]
bad += [] if fp.get('yaw_forward_axis') == '-z' else ['first_person.yaw_forward_axis']
bad += [] if hud.get('stance_icon') is True else ['hud.stance_icon']
extra = sorted(set(m) - set(want) - {'mantle', 'rays', 'unstick', 'assist', 'fp_anim', 'fp_anim_speed_scale', 'fp_anim_rm_mps'})  # run 18 (updated in place): rays_per_tick 48; [movement.rays] / [movement.unstick] judged by check_m8; C3: [movement.assist] and (PASS C) fp_anim* judged by check_c3
if extra:
    bad.append('unknown [movement] keys %s' % extra)
if bad:
    print('FAIL defaults differ: %s' % bad)
    sys.exit(1)
print('tomllib ok [movement] %d keys with the documented defaults, keys.move_* = MW2 PC binds, yaw_forward_axis=-z, stance_icon=true' % len(m))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::movement_tests') 'config_movement_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'config_movement_bounds_unknown_and_duplicates \.\.\. ok' -and $u.Out -match 'config_movement_data_is_cold \.\.\. ok' -and $u.Out -match 'config_movement_defaults_match_design \.\.\. ok'
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok) { Add-Result 7 'config' 'PASS' ("{0}; {1}; config::movement_tests ok (defaults, bounds, data cold, unknown [movement] key and duplicate movement keys rejected)" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 7 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code); movement_tests exit $($u.Code)" }
}

# 8. wiring (section 13.4) with its self-test mutations
Run-Check 8 'wiring' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movewiring', '--self-test') 'movewiring'
    Add-Result 8 'wiring' $(if ($r.Code -eq 0 -and $r.Out -match 'self-test: \d+ mutations of the M5 sources caught') { 'PASS' } else { 'FAIL' }) (((@($r.Out -split "`n" | Where-Object { $_ -match '^(self-test|PASS|FAIL)' }) | ForEach-Object { $_.Trim() }) -join ' | '))
}

# 9. the built DLL == the staged copy (check_m2 stages it), with the MOVE proof strings, without iced / movebench / iw4dis
Run-Check 9 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movestrings', $BuiltDll) 'movestrings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    if ($a.Code -eq 0 -and $same) { Add-Result 9 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 9 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 10. analyzer self-test incl. the M5 fixture and its mutations; the older fixtures keep their results
Run-Check 10 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $f = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m5_move_session.log')) 'analyze_m5_fixture'
    $f4 = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m4_ac130_session.log')) 'analyze_m4_fixture'
    $mvRows = @($f.Out -split "`n" | Where-Object { $_ -match '^PASS\s+(movement|movement speeds|sprint|jump|fall damage|move perf)\s' }).Count
    # the six M4 rows (M6 adds 'ac130 start', judged by check_m6)
    $acRows = @($f4.Out -split "`n" | Where-Object { $_ -match '^PASS\s+ac130 (lifecycle|protection|guns|explosions|perf|effects)\s' }).Count
    $na4 = @($f4.Out -split "`n" | Where-Object { $_ -match '^N/A\s+(movement|movement speeds|sprint|jump|fall damage|move perf)\s' }).Count
    $ok = $r.Code -eq 0 -and $r.Out -match 'M5 fixture m5_move_session\.log: good session PASS on all 6 movement rows, ([8-9]|\d\d) mutations FAIL, older logs N/A' -and $r.Out -match 'M4 fixture m4_ac130_session\.log: good ride PASS on all 6 AC-130 rows' -and $f.Code -eq 0 -and $mvRows -eq 6 -and $f4.Code -eq 0 -and $acRows -eq 6 -and $na4 -eq 6
    Add-Result 10 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0}; M5 fixture: {1}/6 movement rows PASS ({2}); M4 fixture: {3} AC-130 rows PASS, {4}/6 movement rows N/A" -f $r.Code, $mvRows, (Last-Line $f.Out 'IN-GAME ORACLES'), $acRows, $na4)
}

# 11. hygiene: movement.json ignored; no line of the MW2 .cfg/.gsc dumps in our sources; the exe opened read-only and
#     never launched; nothing new in the install folders (listing only)
Run-Check 11 'hygiene' {
    $problems = New-Object System.Collections.ArrayList
    $gi = Get-Content -LiteralPath (Join-Path $Root '.gitignore') -ErrorAction SilentlyContinue
    if (-not ($gi | Where-Object { $_.Trim() -eq '/converted/' })) { [void]$problems.Add('converted\movement.json not ignored (no /converted/ in .gitignore)') }
    $code = @'
import os, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
lines = set()
nsrc = 0
for dp, dn, fn in os.walk(root / 'dumps' / 'm5'):
    for f in fn:
        if f.lower().endswith(('.cfg', '.gsc')):
            nsrc += 1
            for l in (pathlib.Path(dp) / f).read_text(encoding='cp1252', errors='replace').splitlines():
                t = ''.join(l.split())
                # comment rulers (//-----) and other punctuation-only lines are not script content
                if len(t) >= 20 and sum(ch.isalnum() for ch in t) >= 10:
                    lines.add(t)
files = []
for base in (root / 'mod', root / 'tools' / 'scripts', root / 'tools' / 'iw4dis'):
    for dp, dn, fn in os.walk(base):
        dn[:] = [d for d in dn if d not in ('target', '__pycache__')]
        files += [pathlib.Path(dp) / f for f in fn]
hits, n = [], 0
for p in files:
    if p.suffix.lower() in ('.exe', '.dll', '.pdb', '.rlib', '.cso', '.wav', '.vmtex', '.lock'):
        continue
    n += 1
    text = ''.join(p.read_text(encoding='utf-8', errors='replace').split())
    for t in lines:
        if t in text:
            hits.append('%s: %s' % (p.relative_to(root), t[:40]))
conv = (root / 'tools' / 'scripts' / 'convert_movement.py').read_text(encoding='utf-8')
dis = (root / 'tools' / 'iw4dis' / 'src' / 'main.rs').read_text(encoding='utf-8')
bad = []
if re.search(r'\bsubprocess\b|os\.system|os\.startfile|Popen|CreateProcess', conv):
    bad.append('convert_movement.py may start a process')
write_open = 'open\\([^)]*EXE[^)]*[' + chr(34) + chr(39) + '][wa+]'
if 'read_bytes()' not in conv or re.search(write_open, conv):
    bad.append('convert_movement.py does not read the exe as bytes only')
if re.search(r'Command::new|process::Command|CreateProcess|ShellExecute|LoadLibrary', dis):
    bad.append('iw4dis may start or load a process/module')
if 'fs::read(' not in dis:
    bad.append('iw4dis does not read the exe as a file')
if hits or bad:
    print('FAIL ' + '; '.join((hits + bad)[:4]))
    sys.exit(1)
print('PASS %d files under mod\\, tools\\scripts\\ and tools\\iw4dis\\: none contains any of the %d lines (>= 20 non-space chars) of the %d MW2 .cfg/.gsc files under dumps\\m5; convert_movement.py and iw4dis read iw4sp.exe as bytes and never start a process' % (n, len(lines), nsrc))
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_text'
    if ($t.Code -ne 0) { [void]$problems.Add($t.Out.Trim()) }
    $baseline = Get-Content -LiteralPath (Join-Path $Scripts 'fixtures\install_baseline.json') -Raw | ConvertFrom-Json
    $new = @()
    foreach ($d in @($ErGame, $Mw2Dir)) {
        if (-not (Test-Path -LiteralPath $d)) { [void]$problems.Add("install folder missing: $d"); continue }
        $old = @($baseline.$d)
        if ($old.Count -eq 0) { [void]$problems.Add("no baseline for $d in fixtures\install_baseline.json"); continue }
        $top = @(Get-ChildItem -LiteralPath $d -Force | ForEach-Object { $_.Name })
        $new += @($top | Where-Object { $old -notcontains $_ } | ForEach-Object { "$d\$_" })
    }
    # co-op (MODLOG 2026-10-19): Seamless Co-op running in the game process creates <ER Game>\SeamlessCoop\crashdumps (empty) on
    # its first start. Accepted only while it holds no file at all (a crash dump there FAILs: it may contain mw2er memory).
    $new = @($new | Where-Object { -not ((Split-Path -Leaf $_) -eq 'SeamlessCoop' -and (Split-Path -Parent $_) -eq $ErGame -and @(Get-ChildItem -LiteralPath $_ -Recurse -File -Force -ErrorAction SilentlyContinue).Count -eq 0) })
    if ($new.Count -gt 0) { [void]$problems.Add("top-level entries not in the vanilla baseline: $($new -join ', ')") }
    $exeTime = (Get-Item -LiteralPath $Mw2Exe).LastWriteTime
    $gf = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    if ($gf.Code -ne 0) { [void]$problems.Add((Last-Line $gf.Out 'FAIL')) }
    if ($problems.Count -eq 0) { Add-Result 11 'hygiene' 'PASS' ("converted\ ignored; {0}; install folders == checked-in vanilla baseline (top level); iw4sp.exe last written {1:yyyy-MM-dd} (read only); {2}" -f $t.Out.Trim(), $exeTime, (Last-Line $gf.Out '^PASS')) }
    else { Add-Result 11 'hygiene' 'FAIL' ($problems -join ' | ') }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run, docs\M5_DESIGN.md sections 14-15): that the position + proxy-request write sticks in ChrIns_PostPhysics, that gravity 0 + gravity_disabled prevents ER''s fall death in this mod, the frame order (MOVE order), cast_ray cost and what the map filter misses (er_delta), the ray slope vs ER''s material normal, chunk_position across a rebase, HP 0 -> ER death, the live ER key preset (MOVE keyprobe), the hybrid third-person look at MW2 speeds, and the MP-only values (jump slowdown on, fall heights 200/350 from the SP exe).'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM4 -and $results.Count -eq 11) {
    Write-Output 'M5 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M5 OFFLINE ORACLES: FAIL'
exit 1
