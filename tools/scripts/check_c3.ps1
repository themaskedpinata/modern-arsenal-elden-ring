<#
.SYNOPSIS
  Runs every offline oracle of C3 (docs\C3_DESIGN.md section S2) after the whole M8 suite (which runs M7 .. M2 first)
  and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3, Steam or Seamless. eldenring.exe, xinput1_4.dll and
  iw4sp.exe are only READ as files. Writes only to work\check_c3\, work\c3\ (the bench file and emu2.py's own
  layers.json next to its inputs) plus what the sub-checks write. Exit code 0 only if all 15 checks PASS; a SKIP is a
  failure unless -AllowSkip. -SkipM8 skips check 1 for fast iteration but always exits non-zero.

  C3 is built in passes (A: traversal / first person / UI yield / damage numbers; B: controller; C: co-op visibility and
  the co-op nuke). $BuiltPasses lists the passes in this build: check 7 requires the test prefixes of those passes and
  reports the others as "not built yet" (each later pass adds its letter here).

  Checks: 1 M8 suite  2 ER static pin  3 layer table  4 converter  5 regulation  6 cargo build  7 cargo test
          8 movebench  9 config  10 wiring  11 analyzer  12 DLL proof strings  13 friend package current  14 hygiene
          15 exe port 2.7.2.0 (the per-build site tables and the boot preflight against the 2.7.2.0 exe file)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_c3.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM8)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Bench = Join-Path $Mod 'target\release\movebench.exe'
$LogDir = Join-Path $Root 'work\check_c3'
$Erscan = Join-Path $Root 'work\c3\erscan'
$FriendPkg = Join-Path $Root 'dist\MW2inER-coop-friend'
$PinnedPackageSha = '7739289d06766baa878bea76eca3cf546d4969ea21fabe191dd2f3874b89e001'   # port 2.7.2.0: rebuilt from the new vanilla (b8cdab84c077); our rows byte-identical (fc10c5eb2734 on 1.17.1 since PASS C, 9a33f2524237 through PASS A / B)
$BuiltPasses = @('A', 'B', 'C')
$Prefixes = [ordered]@{
    'A' = @('c3_rays_', 'c3_layers_', 'c3_assist_', 'c3_fp_talk_', 'c3_fp_f6_', 'c3_uiyield_', 'c3_dmgnum_', 'c3_project_', 'c3_hitlogic_', 'c3_offsets_')
    'B' = @('c3_pad_', 'c3_padcfg_')
    'C' = @('c3_twin_', 'c3_remote_', 'c3_marker_', 'c3_nukecoop_', 'c3_peerdrop_', 'c3_fpanim_')
}
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

function Cut([string]$s, [int]$n = 160) { if ($s.Length -gt $n) { return $s.Substring(0, $n) } else { return $s } }

function Sha([string]$p) { if (Test-Path -LiteralPath $p) { (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' } }

function Run-Check([string]$Id, [string]$Name, [scriptblock]$Body) {
    try { & $Body } catch { Add-Result $Id $Name 'FAIL' "exception: $_" }
}

Write-Output "C3 offline oracles ($Root), built passes: $($BuiltPasses -join ', ')"
Write-Output "Never launches Elden Ring, MW2, me3, Steam or Seamless. Logs: work\check_c3\"
Write-Output ''

# 1. the whole M8 suite first (check_m7 -> ... -> check_m2, which stages the DLL)
if ($SkipM8) {
    Add-Result 1 'M8 suite' 'SKIP' '-SkipM8 given (the run cannot pass)'
} else {
    Run-Check 1 'M8 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m8.ps1')) 'check_m8'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        if ($r.Code -eq 0 -and $r.Out -match 'M8 OFFLINE ORACLES: PASS' -and $pass -ge 12 -and $bad.Count -eq 0) {
            Add-Result 1 'M8 suite' 'PASS' "check_m8.ps1: $pass PASS ($(Last-Line $r.Out '^SUMMARY'))"
        } else { Add-Result 1 'M8 suite' 'FAIL' "exit $($r.Code); $pass PASS; $(Cut ($bad -join ' | ') 400)" }
    }
}

# 2. ER static pin: eldenring.exe sha256, the bit-3 gate, the filter setter, the filter vtable, the Scaleform prologues,
#    the XInput import / export (c3_static.py, read-only, with mutations)
Run-Check 2 'ER static pin' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'c3_static.py'), '--self-test') 'c3_static'
    $ok = $r.Code -eq 0 -and $r.Out -match 'PASS c3_static: (\d+)/\1 pins' -and $r.Out -match 'self-test: (\d+)/\1 mutations caught'
    Add-Result 2 'ER static pin' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f (Last-Line $r.Out 'c3_static'), (Last-Line $r.Out 'self-test'))
}

# 3. the layer table: emu2.py (read-only emulation of eldenring.exe's table init from the erscan dumps) regenerates
#    layers.json == tools\scripts\fixtures\c3_layers.json; the Rust rows equal the fixture (c3_layers_rs.py --check).
#    Port 2.7.2.0: the same emulation over the 2.7.2.0 dumps (work\port2720\c3emu, builder +0x4c0) must give the same table.
Run-Check 3 'layer table' {
    $fx = Join-Path $Scripts 'fixtures\c3_layers.json'
    if (-not (Test-Path -LiteralPath (Join-Path $Erscan 'emu2.py'))) {
        Add-Result 3 'layer table' 'SKIP' 'work\c3\erscan\emu2.py missing (the erscan dumps are research output)'
    } else {
        $e = Invoke-Tool $Py @('-I', (Join-Path $Erscan 'emu2.py'), '58', '5b', '3') 'emu2' $Erscan
        $gen = Sha (Join-Path $Erscan 'layers.json')
        $want = Sha $fx
        $rs = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'c3_layers_rs.py'), '--check') 'c3_layers_rs'
        $rows = ($e.Out -split "`n" | Where-Object { $_ -match '^0x(58|5b|03):' }) -join ' | '
        $emu20 = Join-Path $Root 'work\port2720\c3emu'
        $e20 = if (Test-Path -LiteralPath (Join-Path $emu20 'emu2_2720.py')) { Invoke-Tool $Py @('-I', (Join-Path $emu20 'emu2_2720.py'), '58', '5b', '3') 'emu2_2720' $emu20 } else { @{ Code = 1; Out = 'missing' } }
        $gen20 = Sha (Join-Path $emu20 'layers.json')
        $ok = $e.Code -eq 0 -and $gen -eq $want -and $rs.Code -eq 0 -and $e20.Code -eq 0 -and $gen20 -eq $want
        Add-Result 3 'layer table' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("emu2 layers.json sha {0} == fixture {1}: {2}; 2.7.2.0 dumps: {3} == fixture: {4}; {5}; {6}" -f $gen.Substring(0, 12), $want.Substring(0, 12), ($gen -eq $want), $gen20.Substring(0, [Math]::Min(12, $gen20.Length)), ($gen20 -eq $want), (Last-Line $rs.Out 'collision_layers'), (Cut $rows 120))
    }
}

# 4. converter: PASS C adds the world models and the third-person (npc) fire sounds of every gun (convert_arsenal.py
#    --check: 34/34 each, deterministic, on disk; --self-test: the C3 mutations caught), convert_ac130.py --check unchanged
Run-Check 4 'converter' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_arsenal.py'), '--check') 'convert_arsenal_check'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_arsenal.py'), '--self-test') 'convert_arsenal_selftest'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_ac130.py'), '--check') 'convert_ac130_check'
    $ok = $a.Code -eq 0 -and $a.Out -match 'C3: world models 34/34, npc fire sounds 34/34' -and $s.Code -eq 0 -and $s.Out -match 'C3: world model lost' -and $b.Code -eq 0
    Add-Result 4 'converter' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("arsenal: {0} | self-test: {1} | ac130: {2}" -f (Cut (Last-Line $a.Out '(PASS|FAIL)') 160), (Cut (Last-Line $s.Out '(PASS|FAIL)') 40), (Cut (Last-Line $b.Out '(PASS|FAIL)') 80))
}

# 5. regulation: the builder's self-test (PASS C: the twins never attack, the markers sync and expire, the lethal stays
#    unsynced), rows.json's coop block (56 twins, the reserved marker ids), the package sha pinned once for PASS C
Run-Check 5 'regulation' {
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py'), '--self-test') 'build_regulation_selftest'
    $rows = Get-Content -LiteralPath (Join-Path $Root 'build\rows.json') -Raw | ConvertFrom-Json
    $pkg = "$($rows.package_regulation_sha256)"
    $co = $rows.coop
    $coOk = $co -and @($co.twins).Count -eq 56 -and $co.nuke_marker_bullet -eq 79000963 -and $co.sp_mode -eq 79001090 -and $co.sp_gun_base -eq 79001100 -and $co.sp_gun_count -eq 34 -and $co.sp_ac130 -eq 79001150 -and $co.sp_nuke -eq 79001190 -and $co.sp_mode_vfx -eq 8060
    $twinsOk = $co -and -not (@($co.twins) | Where-Object { $_.id -lt 79800000 -or $_.id -gt 79899999 })
    $selfOk = $s.Code -eq 0 -and $s.Out -match 'REGULATION SELF-TEST PASS (\d+)/\1 ' -and [int]$Matches[1] -ge 47 -and $s.Out -match 'caught: C3: the lethal nuke SpEffect synced'
    $ok = $selfOk -and $pkg -eq $PinnedPackageSha -and $coOk -and $twinsOk
    Add-Result 5 'regulation' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0} ({1}); package sha {2} (pin {3}, PASS C); coop block {4} ({5} twins in 798xxxxx: {6})" -f $s.Code, (Cut (Last-Line $s.Out '(PASS|FAIL)') 100), $pkg.Substring(0, [Math]::Min(12, $pkg.Length)), $PinnedPackageSha.Substring(0, 12), $coOk, @($co.twins).Count, $twinsOk)
}

# 6. cargo build: the whole workspace with 0 warnings under mod\
Run-Check 6 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv|movebench)` \(' -or $_ -match '\\mod\\') })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Bench)) {
        Add-Result 6 'cargo build' 'PASS' "release workspace; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 6 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s): $(($ours | Select-Object -First 2) -join ' | ')" }
}

# 7. cargo test: 0 failed, at least one test per C3 prefix of the built passes; PASS B: the gamepad unit tests catch
#    the design's mutations (swap yaw / pitch, drop the latch, wrong graph index) in a copy (c3_padmut.ps1)
Run-Check 7 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '--workspace', '--release') 'cargo_test' $Mod
    $pm = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'c3_padmut.ps1')) 'c3_padmut'
    $pmOk = $pm.Code -eq 0 -and $pm.Out -match 'PASS c3_padmut: 3/3 mutations caught'
    $okNames = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { (($_ -split ' ')[1] -split '::')[-1] })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $missing = @(); $counts = @(); $notBuilt = @()
    foreach ($pass in $Prefixes.Keys) {
        foreach ($p in $Prefixes[$pass]) {
            $n = @($okNames | Where-Object { $_.StartsWith($p) }).Count
            if ($BuiltPasses -contains $pass) {
                if ($n -eq 0) { $missing += $p } else { $counts += "$p$n" }
            } else { $notBuilt += $p }
        }
    }
    $skipNote = if ($notBuilt.Count -gt 0) { "; not built yet (pass $((@($Prefixes.Keys | Where-Object { $BuiltPasses -notcontains $_ })) -join '/')): $($notBuilt -join ' ')" } else { '' }
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $pmOk) {
        Add-Result 7 'cargo test' 'PASS' "$($okNames.Count) workspace tests ok; $($counts -join ' ')$skipNote; $(Last-Line $pm.Out 'c3_padmut')"
    } else { Add-Result 7 'cargo test' 'FAIL' "exit $($c.Code); missing prefixes: $($missing -join ', '); failed: $($failed -join ' | '); $(Last-Line $pm.Out 'c3_padmut')" }
}

# 8. movebench: C21-C24 (slopes, root / rock, the layer filters, the assist) + the M8 (B13-B26) and M5 (B1-B12) benches
Run-Check 8 'movebench' {
    $c3 = Invoke-Tool $Bench @('--c3', '--root', $Root) 'movebench_c3'
    $m = Invoke-Tool $Bench @('--mantle', '--root', $Root) 'movebench_mantle'
    $k = Invoke-Tool $Bench @('--check', '--root', $Root) 'movebench_check'
    $l3 = Last-Line $c3.Out '^(PASS|FAIL) MOVEBENCH C3'
    $lm = Last-Line $m.Out '^(PASS|FAIL) MOVEBENCH M8'
    $lk = Last-Line $k.Out '^(PASS|FAIL) MOVEBENCH'
    $ok = $c3.Code -eq 0 -and $l3 -match '^PASS MOVEBENCH C3 4/4 scenarios' -and $l3 -match ' 0 failed' -and $m.Code -eq 0 -and $lm -match '^PASS' -and $k.Code -eq 0 -and $lk -match '^PASS'
    Add-Result 8 'movebench' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | {2}" -f (Cut $l3 110), (Cut $lm 70), (Cut $lk 60))
}

# 9. config: mw2er.toml and the friend toml (dist) carry the C3 defaults, the friend toml also the friend policy
#    (tools\coop_friend\friend_toml.ps1, 2026-10-22); the DLL parser accepts them; the C3 config tests
Run-Check 9 'config' {
    $code = @'
import sys, tomllib
bad, seen = [], []
for path in sys.argv[1:]:
    d = tomllib.load(open(path, 'rb'))
    m = d.get('movement', {})
    want = {
        ('movement', 'rays'): {'mode': 'union', 'player_fallback': 0x3, 'player_layers': [2, 3, 4], 'log_live_table': True, 'map_filter': 0x2000058, 'object_filter': 0x5b},
        ('movement', 'assist'): {'enabled': True, 'stall_speed': 0.15, 'stall_s': 0.4, 'hold_s': 0.8, 'max_per_min': 8, 'after_refusal': True},
        ('first_person',): {'talk_stale_s': 2.0, 'talk_menu_idx': 3, 'fade_stale_s': 3.0, 'f6_overrides_latches': True},
        ('hud',): {'damage_numbers': True, 'damage_numbers_s': 0.9, 'damage_numbers_merge_s': 0.25, 'damage_numbers_value': 'er', 'damage_numbers_max': 24},
        ('ui_yield',): {'enabled': True, 'hud_dim_alpha': 0.35, 'indices': [15, 17, 19, 24, 65, 66], 'learn': True, 'toast_anchor': [0.012, 0.30]},
        ('ui',): {'probe_scaleform': False},
        ('gamepad',): {'enabled': True, 'pad_index': -1, 'look_route': 'mouse', 'input_graph': 3, 'turnrate_yaw': 260.0, 'turnrate_pitch': 90.0,
                       'turnrate_yaw_ads': 90.0, 'turnrate_pitch_ads': 55.0, 'accel_lerp': 1200.0, 'rs_deadzone': 0.18, 'ls_deadzone': 0.20,
                       'hold_ms': 300, 'rumble': False, 'mouse_deg_per_count': 0.0, 'er_interact': 'y', 'er_melee': 'rb'},
        ('gamepad', 'bind'): {'toggle': 'l3+r3', 'fp_toggle': 'dpad_left', 'fire': 'rt', 'ads': 'lt', 'reload': 'x', 'interact_hold': 'x',
                              'weapon_next': 'y', 'weapon_prev': 'lb', 'jump': 'a', 'stance': 'b', 'sprint': 'l3', 'melee': 'r3',
                              'ac130': 'dpad_up', 'nuke': 'dpad_down'},
        ('gamepad', 'gunship'): {'fire': 'rt', 'next': 'y', 'thermal': 'x', 'exit': 'back'},
        ('coop',): {'visual_twins': True, 'twin_min_interval_ms': 40, 'peer_effects': True, 'peer_sounds': True, 'peer_tracers': True,
                    'world_guns': True, 'world_gun_self_tp': True, 'hand_bone': -1, 'markers': True, 'marker_refresh_s': 5.0,
                    'hide_er_weapon': True, 'peer_drop_guard': True, 'peer_drop_factor': 1.5, 'bone_probe': True, 'peer_flashes': False},
        ('nuke',): {'coop_mode': 'owned', 'coop_everyone': True, 'coop_window_s': 6.0, 'probe_forward': False, 'allow_in_coop': True},
        ('movement',): {'fp_anim': 'hybrid', 'fp_anim_speed_scale': False},
    }
    if any(b == 'start' for b in d.get('gamepad', {}).get('bind', {}).values()):
        bad.append('%s: start bound' % path)
    for keys, kv in want.items():
        sec = d
        for k in keys:
            sec = sec.get(k, {})
        for k, v in kv.items():
            if sec.get(k) != v:
                bad.append('%s: [%s] %s=%r (want %r)' % (path.split('\\')[-2], '.'.join(keys), k, sec.get(k), v))
    # C5: both tomls cut ER's messages out of the gun (mode cutout); since 2026-10-21 the friend package carries the host's
    # [ui_yield] unchanged (the cut-out and the C4.1 injection were verified in game)
    want_mode = 'cutout'
    if d.get('ui_yield', {}).get('mode') != want_mode:
        bad.append('%s: [ui_yield] mode=%r (want %r)' % (path.split('\\')[-2], d.get('ui_yield', {}).get('mode'), want_mode))
    # 2026-10-22 installer P0: the friend toml (dist) follows the friend policy of tools\coop_friend\friend_toml.ps1:
    # every test probe / experiment off (a key may be absent = its safe code default), the in-game-tested features on
    if path != sys.argv[1]:
        off = {('ui',): {'probe_scaleform': False, 'probe_layer_sample': False},
               ('nuke',): {'always_available': False, 'probe_forward': False},
               ('movement',): {'write_last_update': False, 'fp_anim_speed_scale': False},
               ('coop',): {'probe_er_depth': False, 'probe_er_depth_state': False, 'probe_er_depth_sample': False, 'probe_er_aim': False,
                           'world_gun_er_depth': False, 'er_carrier': 'off', 'carrier_give': False, 'carrier_load': False,
                           'tp_ads_er_aim': False, 'tp_aim_cam': 'keep'}}
        on = {('ui',): {'inject_viewmodel': True, 'inject_ui_thread': True}, ('keys',): {'heal': 'Q'},
              ('gamepad',): {'enabled': True, 'er_use_item': 'x'}, ('gamepad', 'bind'): {'heal': 'dpad_right'},
              ('first_person',): {'tp_scope_peek': True}, ('nuke',): {'allow_in_coop': True},
              ('coop',): {'hold_mode': 'hand', 'er_stance': 'polearm', 'tp_ads_pose': 'guard', 'tp_guard_yaw': True, 'tp_ads_hold': 'aim',
                          'hide_er_weapon': True, 'hide_state_patch': True, 'world_gun_body_occlusion': True, 'world_gun_occlusion': True}}
        for table, need in ((off, False), (on, True)):
            for keys, kv in table.items():
                sec = d
                for k in keys:
                    sec = sec.get(k, {})
                for k, v in kv.items():
                    if (k in sec or need) and sec.get(k) != v:
                        bad.append('friend toml: [%s] %s=%r (friend policy %r)' % ('.'.join(keys), k, sec.get(k), v))
        tp = d.get('third_person', {})
        for k in ('shoulder_offset_m', 'shoulder_ads_offset_m'):
            if not (isinstance(tp.get(k), list) and len(tp[k]) == 3 and any(x != 0 for x in tp[k])):
                bad.append('friend toml: [third_person] %s=%r (friend policy: the accepted non-zero shoulder camera)' % (k, tp.get(k)))
    seen.append(path)
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('tomllib ok: %d file(s) carry the C3 defaults (rays player 0x3 / [2,3,4], assist 0.15 m/s 0.4 s 0.8 s 8/min, talk 2 s idx 3, fade 3 s, F6 override, damage numbers 0.9 s, ui_yield lower 0.35 [15,17,19,24,65,66] learn on, probe off; gamepad mouse route graph 3 260/90/90/55, the MW2 mapping, R3 melee = ER RB pulse, start unbound, rumble off; coop twins 40 ms, effects, world guns stage A, markers 5 s, bone probe on, HUD flashes off, peer-drop guard 1.5, nuke owned / everyone / 6 s, fp_anim hybrid; friend toml: the friend policy (probes / ER depth / carrier off, C4.1 / C5 / heal / pad / C6 hold + guard + shoulder on))' % len(seen))
'@
    $files = @((Join-Path $Root 'mw2er.toml'))
    $friendToml = Join-Path $FriendPkg 'mw2er.toml'
    if (Test-Path -LiteralPath $friendToml) { $files += $friendToml }
    $t = Invoke-Tool $Py (@('-I', '-c', $code) + $files) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::') 'config_c3_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'c3_rays_player_mode_config \.\.\. ok' -and $u.Out -match 'c3_assist_config_defaults_and_bounds \.\.\. ok' -and $u.Out -match 'c3_fp_talk_config_defaults_and_bounds \.\.\. ok' -and $u.Out -match 'c3_uiyield_config_defaults_and_bounds \.\.\. ok' -and $u.Out -match 'c3_dmgnum_config_defaults_and_bounds \.\.\. ok' -and $u.Out -match 'c3_padcfg_defaults \.\.\. ok' -and $u.Out -match 'c3_padcfg_rejects_unknown_start_duplicates \.\.\. ok' -and $u.Out -match 'c3_coopcfg_defaults_and_bounds \.\.\. ok'
    $ok = $t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok
    Add-Result 9 'config' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; {1}; C3 config tests ok={2} (defaults, ray fallback / layers, assist, talk / fade, ui_yield, [ui] cold, damage numbers rejected values; gamepad defaults, unknown button / start / duplicates rejected)" -f (Cut $t.Out.Trim() 200), (Last-Line $c.Out 'PASS:'), $uok)
}

# 10. wiring: the C3 PASS A + B rules with their mutations (m2_static_checks.py c3wiring --self-test) + the M5/M8 movement wiring
Run-Check 10 'wiring' {
    $w = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'c3wiring', '--self-test') 'c3wiring'
    $m = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'movewiring', '--self-test') 'movewiring'
    $v = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'vmwiring', '--self-test') 'vmwiring'
    $ok = $w.Code -eq 0 -and $w.Out -match 'self-test: (\d+) mutations of the C3 PASS A \+ B \+ C sources caught' -and [int]$Matches[1] -ge 45 -and $m.Code -eq 0 -and $v.Code -eq 0
    Add-Result 10 'wiring' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | move {2} | vm {3}" -f (Last-Line $w.Out 'self-test'), (Cut (Last-Line $w.Out '^(PASS|FAIL)') 120), (Last-Line $m.Out '^(PASS|FAIL)').Substring(0, 4), (Last-Line $v.Out '^(PASS|FAIL)').Substring(0, 4))
}

# 11. analyzer: analyze_c3.py --self-test (synthetic sessions, mutations, the known-bad C2 host replay: fp talk latch and
#     fp f6 FAIL), analyze_log.py --self-test (all milestones, C3 rows wired), the C2 friend log through the C3 rows
Run-Check 11 'analyzer' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_c3.py'), '--self-test') 'analyze_c3_selftest'
    $l = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $f = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_c3.py'), (Join-Path $Root 'logs\coop\friend_c2_2026-10-19.log')) 'analyze_c3_friend_c2'
    $fp = @($f.Out -split "`n" | Where-Object { $_ -match '^FAIL\s+(fp talk latch|fp f6)\s' }).Count
    # PASS C: the C2 friend log's credit of the host's nuke kill (c2500, dmg 4937, kill=1) must FAIL 'peer drop'
    $pd = @($f.Out -split "`n" | Where-Object { $_ -match '^FAIL\s+peer drop\s' }).Count
    $ok = $a.Code -eq 0 -and $a.Out -match 'PASS analyze_c3 self-test: (\d+)/\1 mutations caught' -and [int]$Matches[1] -ge 32 -and $a.Out -match 'known-bad C2 host log replay: fp talk latch FAIL .*fp f6 FAIL .*coop nuke FAIL' -and $l.Code -eq 0 -and $l.Out -match 'C3 rows: good C3 session' -and $fp -eq 2 -and $pd -eq 1
    Add-Result 11 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; C2 host replay: fp talk latch + fp f6 + coop nuke (7 of 51) FAIL (as found); C2 friend log: {1}/2 FP rows FAIL, peer drop FAIL {2}/1 (the host's nuke kill credited, as found); analyze_log self-test exit {3}" -f (Last-Line $a.Out 'analyze_c3 self-test'), $fp, $pd, $l.Code)
}

# 12. DLL proof strings (built == staged; check_m2 inside check 1 stages it)
Run-Check 12 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'c3strings', $BuiltDll) 'c3strings'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Sha $BuiltDll) -eq (Sha $StagedDll))
    if ($a.Code -eq 0 -and $same) { Add-Result 12 'DLL proof strings' 'PASS' ("{0}; build\natives\mw2er.dll == target (sha256 {1})" -f (Cut (Last-Line $a.Out '^PASS') 90), (Sha $StagedDll).Substring(0, 12)) }
    else { Add-Result 12 'DLL proof strings' 'FAIL' ("{0}; staged copy identical: {1} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $same) }
}

# 13. friend package current: its manifest's DLL sha == the staged DLL, its regulation sha == the built package; dist hygiene
Run-Check 13 'friend package' {
    $man = Join-Path $FriendPkg 'manifest\host_manifest.json'
    if (-not (Test-Path -LiteralPath $man)) {
        Add-Result 13 'friend package' 'FAIL' 'dist\MW2inER-coop-friend\manifest\host_manifest.json missing: rebuild with tools\coop_friend\make_friend_package.ps1'
    } else {
        $mtext = Get-Content -LiteralPath $man -Raw
        $dllSha = Sha $StagedDll
        $pkgDll = Sha (Join-Path $FriendPkg 'build\natives\mw2er.dll')
        $rows = Get-Content -LiteralPath (Join-Path $Root 'build\rows.json') -Raw | ConvertFrom-Json
        $regOk = $mtext.Contains("$($rows.package_regulation_sha256)")
        $d = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'dist', $FriendPkg) 'dist'
        $ok = $mtext.Contains($dllSha) -and $pkgDll -eq $dllSha -and $regOk -and $d.Code -eq 0
        Add-Result 13 'friend package' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("manifest DLL sha == staged {0}: {1}; packaged DLL == staged: {2}; regulation sha in manifest: {3}; {4}{5}" -f $dllSha.Substring(0, 12), $mtext.Contains($dllSha), ($pkgDll -eq $dllSha), $regOk, (Cut (Last-Line $d.Out '^(PASS|FAIL)') 80), $(if ($ok) { '' } else { ' (rebuild: tools\coop_friend\make_friend_package.ps1)' }))
    }
}

# 14. hygiene: static hygiene and no game-folder writes (listing / hashes only)
Run-Check 14 'hygiene' {
    $h = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'hygiene') 'hygiene_static'
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    $code = @'
import pathlib, re, sys
root = pathlib.Path(sys.argv[1])
bad = []
for name in ('analyze_c3.py', 'c3_static.py', 'c3_layers_rs.py', 'exe_sites.py'):
    t = (root / 'tools' / 'scripts' / name).read_text(encoding='utf-8', errors='replace')
    if re.search(r'subprocess|os\.system|Popen|Start-Process|ShellExecute|steam://', t):
        bad.append(name + ' may start a process')
    if re.search(r'open\([^)]*eldenring[^)]*,\s*.w|write_bytes\([^)]*ER_EXE|ER_EXE\.write', t):
        bad.append(name + ' writes the exe')
if bad: print('FAIL ' + '; '.join(bad)); sys.exit(1)
print('PASS the C3 scripts start no process and only read eldenring.exe / xinput1_4.dll')
'@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_c3'
    $ok = $h.Code -eq 0 -and $g.Code -eq 0 -and $t.Code -eq 0
    Add-Result 14 'hygiene' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; {1}; {2}" -f (Cut (Last-Line $h.Out '^(PASS|FAIL)') 80), (Cut (Last-Line $g.Out '^(PASS|FAIL)') 100), $t.Out.Trim())
}

# 15. port 2.7.2.0 (MODLOG 2026-10-20): exe_sites.py regenerates mw2er-core\src\exe_sites.rs from the 2.7.2.0 exe file
#     (own sites re-found by signature / RTTI, crate RVAs = vendor rva_ww2720.rs) == the file on disk, its self-test
#     mutations are caught, and the DLL's preflight (mw2er_core::exe_build) passes all 113 sites on the exe file mapped as
#     an image while the 2.7.1.0 table and single-byte mutations FAIL (cargo test real_exe_2720, read-only)
Run-Check 15 'exe port 2.7.2.0' {
    $g = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'exe_sites.py'), '--check') 'exe_sites_check'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'exe_sites.py'), '--self-test') 'exe_sites_selftest'
    # review 2026-10-20: a missing / different exe FAILS the test itself (MW2ER_REQUIRE_EXE), not only this regex
    $env:MW2ER_REQUIRE_EXE = '1'
    try {
        $c = Invoke-Tool 'cargo' @('test', '--release', '-p', 'mw2er-core', '--test', 'real_exe_2720', '--', '--include-ignored', '--nocapture') 'real_exe_2720' $Mod
    } finally { Remove-Item Env:\MW2ER_REQUIRE_EXE -ErrorAction SilentlyContinue }
    # the gate's source guards: mirrored crate detection == vendored rva.rs; the "unused" crate RVAs are unused
    $v = Invoke-Tool 'cargo' @('test', '--release', '-p', 'mw2er-core', '--test', 'exe_gate_sources', '--', '--nocapture') 'exe_gate_sources' $Mod
    $okS = $s.Code -eq 0 -and $s.Out -match 'PASS self-test: (\d+) mutations caught' -and [int]$Matches[1] -ge 8
    $okC = $c.Code -eq 0 -and $c.Out -match 'PASS 2\.7\.2\.0 header timedatestamp=0x6aabf7e7 sizeofimage=0x5dfd600' -and $c.Out -match 'PASS 2\.7\.2\.0 preflight on the exe file: sites=113 code=20 vmt=83 inferred=4 warn=0' -and $c.Out -notmatch 'SKIP:' -and $c.Out -match 'PASS mutations caught: (\d+)' -and [int]$Matches[1] -ge 8
    $okV = $v.Code -eq 0 -and $v.Out -match 'PASS version gate mirrors' -and $v.Out -match 'PASS 11 unused crate code RVAs'
    $ok = $g.Code -eq 0 -and $g.Out -match '(?m)^EXE SITES PASS' -and $okS -and $okC -and $okV
    Add-Result 15 'exe port 2.7.2.0' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1} | {2} | {3} | {4}" -f (Cut (Last-Line $g.Out 'EXE SITES') 150), (Last-Line $s.Out 'self-test'), (Last-Line $c.Out 'preflight on the exe file'), (Last-Line $c.Out '(mutations caught|SKIP|panicked)'), $(if ($okV) { 'gate source guards 2/2' } else { 'gate source guards FAIL' }))
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the next in-game run, docs\C3_DESIGN.md S4): that the player proxy is layer 3 (MOVE filter live), the live table rows (MOVE filter table), that the player filter removes the invisible walls and keeps wagons solid, the assist feel, the talk latch in real dialogs, which ui_states indices are messages (UI learn), that the yield makes messages readable, the damage numbers look; PASS B: that ER reads the pad through xinput1_4 under Steam Input / me3 (PAD stat connected=1), that ER polls the mouse while a pad is used (else PAD look route fallback=er), ER''s degrees per count (PAD calib), that Y is ER''s interact button, the MW2 look feel; PASS C (both PCs): that peers receive our bit-3 twins (COOP twin seen) and synced markers (COOP marker seen), that the MW2-mode marker hides ER''s weapon, the world-gun placement (world_gun_offset_m), that ER plays walk / run in first person with fp_anim hybrid, that chr_update_type tells who owns an NPC (NUKE census upd=), that the peer''s game kills its own enemies on our marker.'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM8 -and $results.Count -eq 15) {
    Write-Output 'C3 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'C3 OFFLINE ORACLES: FAIL'
exit 1
