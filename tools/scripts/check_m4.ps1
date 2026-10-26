<#
.SYNOPSIS
  Runs every offline oracle of milestone M4 (the MW2 AC-130 killstreak) after the whole M3b suite (which runs M3 and
  M2 first) and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m4\ plus what the
  sub-checks write (check_m3b/m3/m2: work\, converted\viewmodel\, mod\target, build\natives; build_regulation.py:
  build\package, build\rows.json, work\regulation; convert_ac130.py --check / --self-test: temporary copies under
  work\). The game install folders are only listed (check 14). Exit code 0 only if all 14 checks PASS; a SKIP is a
  failure unless -AllowSkip. -SkipM3b skips check 1 for fast iteration but always exits non-zero.

  Checks (docs\M4_DESIGN.md section 14.4):
   1 M3b suite (incl. M3, M2)  2 cargo build (0 warnings, incl. sndconv)  3 cargo test (14.1 categories, real data,
     mw2er write-cost test)  4 converter  5 regulation  6 rows consistency  7 vmtest T (debug layer)
   8 vmtest T (GBV)  9 thermal perf  10 config  11 wiring  12 DLL proof strings  13 analyzer  14 hygiene
   15 real ER frame (T10; NEEDS-DATA until the user's screenshot exists)  16 vmtest tracers (run 8: TR1-TR4 with the
     debug layer and GBV, 4K perf < 0.2 ms)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m4.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM3b)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$StagedDll = Join-Path $Root 'build\natives\mw2er.dll'
$Vmtest = Join-Path $Mod 'target\release\vmtest.exe'
$LogDir = Join-Path $Root 'work\check_m4'
$OutDir = Join-Path $Root 'work\m4'
# The M4 regulation package (rebuilt 2026-10-17 after run 9: object destruction atkObj 200/100/40 + the bush flag,
# impact SFX burst + dust per tier with sfxPostureType 1; still flat damage (run 8), k 0.65, scale_25 0.7; the M2 rows
# unchanged). M6 (2026-10-17): the package also carries the arsenal rows 79100000+ (bullet+269 atk+264 more, flat_inv
# s 1.225 d_ref 85, sfxId_Hit/Flick -1); the M2/M4 rows are byte-identical, the pin and counts below follow it.
# M7 (2026-10-18): + the nuke rows (SpEffectParam 79000960, Bullet + AtkParam_Pc 79000961, Bullet 79000962); every M6 row
# byte-identical (m7_regulation_rows.py --check in check_m7); the pin and counts follow it.
$PackageSha = '7739289d06766baa878bea76eca3cf546d4969ea21fabe191dd2f3874b89e001'   # port 2.7.2.0: built from the new vanilla b8cdab84c077, rows unchanged (fc10c5eb2734 on 1.17.1 since C3 PASS C; 9a33f2524237 through C3 PASS B)
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

Write-Output "M4 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m4\"
Write-Output ''

# 1. the whole M3b suite first (it runs check_m3.ps1, which runs check_m2.ps1 and stages the DLL)
if ($SkipM3b) {
    Add-Result 1 'M3b suite' 'SKIP' '-SkipM3b given (the run cannot pass)'
} else {
    Run-Check 1 'M3b suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m3b.ps1')) 'check_m3b'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        $m3 = Last-Line $r.Out '^PASS\s+1 M3 suite'
        if ($r.Code -eq 0 -and $pass -eq 8 -and $bad.Count -eq 0 -and $r.Out -match 'M3b OFFLINE ORACLES: PASS' -and $m3 -match '18 PASS' -and $m3 -match '21 PASS') {
            Add-Result 1 'M3b suite' 'PASS' "check_m3b.ps1: 8 PASS (check_m3 18 PASS, check_m2 21 PASS), 0 FAIL/SKIP"
        } else { Add-Result 1 'M3b suite' 'FAIL' "exit $($r.Code); $pass PASS; $($bad -join ' | ')" }
    }
}

# 2. cargo build: the whole workspace incl. sndconv with 0 warnings under mod\
Run-Check 2 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest|sndconv)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest|sndconv)` \(' -or $_ -match '\\mod\\') })
    $errors = @($r.Out -split "`n" | Where-Object { $_ -match '(^|\s)error(\[|:)' })
    $snd = Join-Path $Mod 'target\release\sndconv.exe'
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $snd)) {
        Add-Result 2 'cargo build' 'PASS' "release workspace (mw2er, mw2er-core, mw2er-render, vmtest, sndconv); 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B"
    } else { Add-Result 2 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s); $($errors.Count) error line(s): $((@($ours) + @($errors) | Select-Object -First 2) -join ' | ')" }
}

# 3. cargo test: mw2er-core with >= 1 test per 14.1 category; the M4 real-data tests; the mw2er write-cost test
Run-Check 3 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core') 'cargo_test' $Mod
    $okLines = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { ($_ -split ' ')[1] })
    $cats = [ordered]@{
        'logic ready + gate order' = '^ac130_logic::tests::ac130_logic_ready_on_start_and_gate_order'
        'logic active only after protection' = '^ac130_logic::tests::ac130_logic_active_only_after_protection'
        'logic each exit reason' = '^ac130_logic::tests::ac130_logic_each_exit_reason'
        'logic priority' = '^ac130_logic::tests::ac130_logic_priority_order'
        'logic protection removal timing + shells' = '^ac130_logic::tests::ac130_logic_protection_removal_timing'
        'logic cooldown' = '^ac130_logic::tests::ac130_logic_cooldown_exact_and_disabled'
        'logic F7 exit / F5 off' = '^ac130_logic::tests::ac130_logic_f7_during_active_is_exit_key_and_f5_off'
        'logic FP blocker every path' = '^ac130_logic::tests::ac130_logic_fp_blocker_every_path'
        'logic never active without protection' = '^ac130_logic::tests::ac130_logic_never_active_without_protection'
        'cam orbit' = '^ac130_cam::tests::ac130_cam_orbit_positions'; 'cam theta0' = '^ac130_cam::tests::ac130_cam_theta0_behind_heading'
        'cam base view' = '^ac130_cam::tests::ac130_cam_base_view_pitch'; 'cam aim clamps' = '^ac130_cam::tests::ac130_cam_aim_clamps_hold'
        'cam +lY / invert' = '^ac130_cam::tests::ac130_cam_mouse_y_looks_down_and_invert'; 'cam zoom sens' = '^ac130_cam::tests::ac130_cam_zoom_sensitivity_scaling'
        'cam rows sweep' = '^ac130_cam::tests::ac130_cam_rows_orthonormal_sweep'; 'cam bounds' = '^ac130_cam::tests::ac130_cam_bounds_reject'
        'cam fov' = '^ac130_cam::tests::ac130_cam_fov_conversion'; 'cam CAMFRAME rebuild' = '^ac130_cam::tests::ac130_cam_camframe_forward_rebuilds_right_up'
        'guns cadence' = '^ac130_guns::tests::ac130_guns_cadence_per_gun'; 'guns reload while away' = '^ac130_guns::tests::ac130_guns_reload_continues_while_switched_away'
        'guns switch order' = '^ac130_guns::tests::ac130_guns_switch_order_wheel_q_and_keys'; 'guns catch-up cap' = '^ac130_guns::tests::ac130_guns_catch_up_capped'
        'guns fractional tick' = '^ac130_guns::tests::ac130_guns_fractional_tick_long_run_rate'
        'aim ray-capsule' = '^ac130_aim::tests::ac130_aim_ray_capsule_hit_miss_graze'; 'aim nearest' = '^ac130_aim::tests::ac130_aim_nearest_of_map_vs_chr'
        'aim via=none' = '^ac130_aim::tests::ac130_aim_via_none'; 'aim flight time' = '^ac130_aim::tests::ac130_aim_flight_time'
        'dmg endpoints' = '^ac130_dmg::tests::ac130_dmg_endpoints'; 'dmg table 8.2' = '^ac130_dmg::tests::ac130_dmg_table_8_2_exact'
        'dmg core + ring' = '^ac130_dmg::tests::ac130_dmg_additive_core_plus_ring'; 'dmg rounding / u16' = '^ac130_dmg::tests::ac130_dmg_rounding_and_u16'
        'dmg run-8 targets' = '^ac130_dmg::tests::ac130_dmg_run8_targets'; 'dmg ER curve' = '^ac130_dmg::tests::ac130_dmg_er_curve_continuous'
        'rows gun scale' = '^rows::tests::rows_ac130_gun_scale'; 'thermal gamma' = '^thermal::tests::thermal_gamma_lifts_midtones'
        'tracer ring' = '^ac130_tracer::tests::ac130_tracer_ring_overwrites_oldest'; 'tracer projection' = '^ac130_tracer::tests::ac130_tracer_projection_matches_hud_project'
        'tracer flight' = '^ac130_tracer::tests::ac130_tracer_head_moves_over_flight_time'; 'tracer widths' = '^ac130_tracer::tests::ac130_tracer_widths_per_gun_and_min_px'
        'tracer near clip / capacity' = '^ac130_tracer::tests::ac130_tracer_near_clip_and_capacity'; 'tracer shade' = '^ac130_tracer::tests::ac130_tracer_shade_profile'
        'protect round trip' = '^ac130_protect::tests::ac130_protect_round_trip_leaves_entries_identical'
        'protect stray cleanup' = '^ac130_protect::tests::ac130_protect_stray_cleanup_removes_only_ours'
        'protect retry / stuck' = '^ac130_protect::tests::ac130_protect_remove_retry_and_stuck'
        'protect refused' = '^ac130_protect::tests::ac130_protect_apply_refused_before_gate_or_row'
        'thermal film packing' = '^thermal::tests::thermal_film_packing_known_answers'; 'thermal shade_pixel' = '^thermal::tests::thermal_shade_pixel_known_answers'
        'thermal toggle lerp' = '^thermal::tests::thermal_toggle_lerp_endpoints'; 'thermal BC3 hand block' = '^thermal::tests::thermal_bc3_decoder_hand_block'
        'thermal bilinear' = '^thermal::tests::thermal_bilinear_reference_2x2'; 'thermal grain' = '^thermal::tests::thermal_grain_offsets_deterministic'
        'hud box' = '^ac130_hud::tests::ac130_hud_box_4k_and_1080'; 'hud blink' = '^ac130_hud::tests::ac130_hud_blink_range'
        'hud timer' = '^ac130_hud::tests::ac130_hud_timer_text'; 'hud indicator' = '^ac130_hud::tests::ac130_hud_indicator_strings'
        'audio dist gain' = '^ac130_audio::tests::ac130_audio_dist_gain_endpoints'; 'audio draws' = '^ac130_audio::tests::ac130_audio_pick_within_ranges'
        'audio reload delays' = '^ac130_audio::tests::ac130_audio_reload_secondary_delays'
        'input gunship capture' = '^input_logic::tests::input_gunship_capture'
        'fp_logic ac130 reason' = '^fp_logic::tests::fp_logic_ac130_blocks_and_rearms'
        'DrawGate ac130_active' = '^vm_anim::tests::vm_anim_draw_gate'
        'config [ac130]' = '^config::ac130_tests::'
        'rows ac130 block' = '^rows::tests::rows_ac130_block'
        # after run 9 (docs\M4_DESIGN.md section 20): camera shake, the bigger 105 flash, the rows' object/SFX fields
        'shake envelope' = '^ac130_shake::tests::ac130_shake_envelope_decays_linearly'
        'shake bounded' = '^ac130_shake::tests::ac130_shake_bounded_by_amplitude'
        'shake strongest wins' = '^ac130_shake::tests::ac130_shake_strongest_quake_wins'
        'shake MW2 numbers / falloff' = '^ac130_shake::tests::ac130_shake_mw2_numbers_and_falloff'
        'shake frame + aim unshaken' = '^ac130_shake::tests::ac130_shake_frame_orthonormal_and_aim_unshaken'
        'tracer 105 flash + bloom' = '^ac130_tracer::tests::ac130_tracer_105_flash_bigger_longer_with_bloom'
        'rows object fields' = '^rows::tests::rows_ac130_object_fields'
    }
    $missing = @($cats.Keys | Where-Object { $pat = $cats[$_]; -not ($okLines | Where-Object { $_ -match $pat }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    $env:MW2ER_ROOT = $Root
    try { $rd = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'real_data_m4', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data_m4' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $rdPass = @($rd.Out -split "`n" | Where-Object { $_ -match 'PASS:' }).Count
    $rdSkip = @($rd.Out -split "`n" | Where-Object { $_ -match '^SKIP:' })
    $g = Invoke-Tool 'cargo' @('test', '-p', 'mw2er', '--lib', '--release', '--', '--nocapture') 'cargo_test_mw2er' $Mod
    $gw = Last-Line $g.Out 'GAMEWRITE'
    $gok = $g.Code -eq 0 -and $g.Out -match 'game::tests::game_write_fast_and_safe \.\.\. ok' -and $g.Out -match 'hud::tests::hud_gunship_frame_uses_one_background_draw_list \.\.\. ok' -and $gw
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0 -and $rd.Code -eq 0 -and $rdPass -eq 2 -and $rdSkip.Count -eq 0 -and $gok) {
        Add-Result 3 'cargo test' 'PASS' "$($okLines.Count) core tests ok across $($cats.Count) M4 categories; real data: rows.json ac130 == ac130_dmg, Rust BC3 == Python; mw2er write-cost test ok ($gw); hud one-draw-list frame test ok"
    } else { Add-Result 3 'cargo test' 'FAIL' "exit $($c.Code); missing: $($missing -join ', '); failed: $($failed -join ' | '); real data exit $($rd.Code) PASS $rdPass SKIP $($rdSkip -join ' | '); mw2er test exit $($g.Code) ($gw)" }
}

# 4. converter: every known answer of section 13, the 5 self-test mutations, manifest hashes
Run-Check 4 'converter' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_ac130.py'), '--check') 'convert_check'
    $b = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_ac130.py'), '--self-test') 'convert_selftest'
    $ok = $a.Code -eq 0 -and $a.Out -match 'manifest hashes match' -and $b.Code -eq 0 -and $b.Out -match 'PASS self-test: 5/5 mutations caught'
    $detail = "{0} | {1}" -f (Last-Line $a.Out '^(PASS|FAIL)'), (Last-Line $b.Out '^(PASS|FAIL)')
    Add-Result 4 'converter' $(if ($ok) { 'PASS' } else { 'FAIL' }) $detail
}

# 5. regulation: build + WitchyBND round trip, safety + object/SFX asserts, >= 18 self-test mutations (M6: 27), the pinned package sha
Run-Check 5 'regulation' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py')) 'build_regulation'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py'), '--self-test') 'build_regulation_selftest'
    $pkg = Join-Path $Root 'build\package\regulation.bin'
    # review M6: the sha of a package left by an earlier build is not evidence when this build failed
    $sha = if ($r.Code -ne 0) { 'not-compared(build-failed)' } elseif (Test-Path -LiteralPath $pkg) { (Get-FileHash -LiteralPath $pkg -Algorithm SHA256).Hash.ToLowerInvariant() } else { 'missing' }
    $line = Last-Line $r.Out '^REGULATION'
    $obj = Last-Line $r.Out '^\s+objects \[I\]'
    $ok = $r.Code -eq 0 -and $line -match '^REGULATION PASS bullet\+342 atk\+278 speffect\+39 ' -and $obj -match '105/core 2934' -and $s.Code -eq 0 -and $s.Out -match 'REGULATION SELF-TEST PASS (\d+)/\1 ' -and [int]$Matches[1] -ge 18 -and $sha -eq $PackageSha
    Add-Result 5 'regulation' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}; {1}; package sha256 {2} (pinned {3}); {4}" -f $line, (Last-Line $s.Out 'SELF-TEST'), $sha.Substring(0, [Math]::Min(12, $sha.Length)), $PackageSha.Substring(0, 12), $obj.Substring(0, [Math]::Min(120, $obj.Length)))
}

# 6. rows consistency: rows.json ac130 block == ac130_dmg of weapons_ac130.json and k (Python here, Rust in check 3)
Run-Check 6 'rows consistency' {
    $code = @"
import json, sys
r = json.load(open(sys.argv[1], encoding='utf-8'))['ac130']; w = json.load(open(sys.argv[2], encoding='utf-8'))
k = r['k']; bad = []; n = 0
def dmg(g, d):
    f = min(max(1.0 - d / g['explosion_radius_in'], 0.0), 1.0)
    return g['explosion_outer'] + (g['explosion_inner'] - g['explosion_outer']) * f
def rhaz(x):
    return int(x + 0.5) if x >= 0 else -int(-x + 0.5)
for g in w['guns']:
    k = r['k'] * r.get('gun_scale', {}).get(g['id'], 1.0)   # run 8: per-gun scale of k
    rad = g['explosion_radius_in']; rm = rad * 0.0254
    if abs(g['explosion_radius_m'] - rm) > 1e-4: bad.append('radius_m ' + g['id'])
    want = {'single': (rm, dmg(g, rad / 2))}
    if 'core' in r['guns'][g['id']]:
        q1, q3 = dmg(g, rad / 4), dmg(g, 3 * rad / 4)
        want['core'] = (rm / 2, q1 - q3); want['ring'] = (rm, q3)
    got = r['guns'][g['id']]
    if sorted(want) != sorted(x for x in got if x in ('core', 'ring', 'single')): bad.append('tiers ' + g['id'])
    for t, (radius, mw2) in want.items():
        row = got.get(t, {}); n += 1
        if abs(row.get('radius_m', -1) - radius) > 1e-4 or abs(row.get('mw2', -1) - mw2) > 1e-9 or row.get('atk_phys') != rhaz(mw2 * k):
            bad.append('%s %s %s' % (g['id'], t, row))
ids = sorted(x['id'] for gg in r['guns'].values() for x in gg.values())
if any(not 79000200 <= i <= 79000499 for i in ids) or len(set(ids)) != len(ids): bad.append('ids %s' % ids)
if r.get('atk_mode') != 'flat': bad.append('atk_mode %r (run 8: flat)' % r.get('atk_mode'))
print(('FAIL ' + '; '.join(bad[:3])) if bad else 'PASS %d AC-130 rows == ac130_dmg (k=%s, gun_scale %s, flat); radii == MW2 radius x 0.0254; ids %s..%s' % (n, r['k'], r.get('gun_scale'), ids[0], ids[-1]))
sys.exit(1 if bad else 0)
"@
    $p = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'build\rows.json'), (Join-Path $Root 'converted\ac130\weapons_ac130.json')) 'rows_consistency'
    $rust = Last-Line (Get-Content -LiteralPath (Join-Path $LogDir 'cargo_test_real_data_m4.log') -Raw -ErrorAction SilentlyContinue) 'PASS: rows.json ac130'
    $ok = $p.Code -eq 0 -and $p.Out -match '^PASS 7 AC-130 rows' -and $rust
    Add-Result 6 'rows consistency' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("python: {0} | rust: {1}" -f $p.Out.Trim(), $(if ($rust) { $rust } else { 'no PASS line (check 3)' }))
}

# 7-8. vmtest thermal T cases, debug layer and GPU-based validation
Run-Check 7 'vmtest T (debug)' {
    $r = Invoke-Tool $Vmtest @('--thermal', '--out', $OutDir) 'vmtest_thermal'
    $t = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS T[1-9] ' }).Count
    $ok = $r.Code -eq 0 -and $r.Out -match 'THERMAL PASS \(0 FAIL\)' -and $t -eq 9 -and $r.Out -match 'PASS T11 ' -and $r.Out -match 'PASS Tinfoqueue 0 ERROR'
    Add-Result 7 'vmtest T (debug)' $(if ($ok) { 'PASS' } else { 'FAIL' }) "$t/9 T cases + T11 (gamma) PASS; $(Last-Line $r.Out 'Tinfoqueue'); $(Last-Line $r.Out '^THERMAL')"
    $script:T10Line = Last-Line $r.Out '^(PASS|FAIL|NEEDS-DATA) T10 '
}
Run-Check 8 'vmtest T (GBV)' {
    $r = Invoke-Tool $Vmtest @('--thermal', '--gbv', '--out', $OutDir) 'vmtest_thermal_gbv'
    $t = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS T[1-9] ' }).Count
    $ok = $r.Code -eq 0 -and $r.Out -match 'debug layer on \+ GBV' -and $r.Out -match 'THERMAL PASS \(0 FAIL\)' -and $t -eq 9 -and $r.Out -match 'PASS T11 ' -and $r.Out -match 'PASS Tinfoqueue 0 ERROR'
    Add-Result 8 'vmtest T (GBV)' $(if ($ok) { 'PASS' } else { 'FAIL' }) "$t/9 T cases PASS under GBV; $(Last-Line $r.Out 'Tinfoqueue'); $(Last-Line $r.Out '^THERMAL')"
}

# 9. GPU time at 4K: the thermal pass (p95 < 1 ms, median < 0.3 ms) and the M3 viewmodel (unchanged, < 1 ms)
Run-Check 9 'thermal perf' {
    $t = Invoke-Tool $Vmtest @('--thermal-perf', '--out', $OutDir) 'vmtest_thermal_perf'
    $v = Invoke-Tool $Vmtest @('--perf', '--out', (Join-Path $Root 'work\m3')) 'vmtest_vm_perf'
    $ok = $t.Code -eq 0 -and $t.Out -match 'THERMAL PERF PASS' -and $t.Out -match '@3840x2160' -and $v.Code -eq 0 -and $v.Out -match 'PERF PASS'
    Add-Result 9 'thermal perf' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | viewmodel: {1}" -f (Last-Line $t.Out 'PERF'), (Last-Line $v.Out '^PERF'))
}

# 10. config: tomllib + the DLL parser; the documented [ac130] defaults; an unknown [ac130] key is rejected (core test)
Run-Check 10 'config' {
    $code = "import tomllib,sys; d=tomllib.load(open(sys.argv[1],'rb')); a=d['ac130']; au=a['audio']; ra=d['regulation']['ac130']; k=d['keys']; " +
            "want={'enabled':True,'assets_dir':'converted\\ac130','cooldown_s':120.0,'duration_s':0.0,'scale':1.0,'base_view':'center','vision':'sp','thermal':True,'grain':True,'hide_er_hud':True,'require_open_sky':True,'max_player_move_m':25.0,'explosion_tiers':2,'shells_after_exit':True,'sync_chr_cam':False,'fov_105':0.0,'fov_40':0.0,'fov_25':0.0,'exposure':0.52,'gamma':0.75}; " +
            "bad=[x for x,v in want.items() if a.get(x)!=v]; " +
            "bad+=[] if isinstance(a.get('cooldown_enabled'),bool) else ['cooldown_enabled (a bool; the value set by the user, false since run 9, is accepted)']; " +
            "bad+=[x for x,v in {'gain':0.8,'ambient_gain':1.0,'explosion_gain':1.0,'explosion_floor_gain':0.35,'announce_ready':True}.items() if au.get(x)!=v]; " +
            "bad+=[x for x,v in {'k':0.65,'scale_105':1.0,'scale_40':1.0,'scale_25':0.7,'clone_bullet_from':10030001,'clone_atk_from':10030001,'clone_speffect_from':1540,'protect_speffect_id':79000950,'protect_endurance_s':90.0,'sfx_105':524028,'sfx_105_ring':530123,'sfx_40':300602,'sfx_40_ring':530133,'sfx_25':530133}.items() if ra.get(x)!=v]; " +
            "bad+=[x for x,v in {'enabled':True,'flash':True,'width_105':1.2,'width_40':0.6,'width_25':0.25,'flash_105':14.0,'flash_s_105':0.5,'flash_gain_105':1.6,'bloom_105':32.0}.items() if a.get('tracers',{}).get(x)!=v]; " +
            "bad+=['shake.'+x for x,v in {'enabled':True,'scale':1.0,'rot_deg':2.0,'roll_deg':0.6,'pos_m':1.5,'freq_hz':11.0,'mw2_falloff':False}.items() if a.get('shake',{}).get(x)!=v]; " +
            "bad+=[x for x,v in {'ac130':'F7','ac130_exit':'Back','ac130_next':'Q','ac130_gun1':'1','ac130_gun2':'2','ac130_gun3':'3','ac130_thermal':'F'}.items() if k.get(x)!=v]; " +
            "print('tomllib ok [ac130] %d keys (cooldown_enabled=%s, set by the user), [ac130.audio], [ac130.tracers], [ac130.shake], [regulation.ac130] k=%s sfx 105=%s/%s, keys.ac130=%s' % (len(a), a['cooldown_enabled'], ra['k'], ra['sfx_105'], ra['sfx_105_ring'], k['ac130'])) if not bad else (print('FAIL defaults differ: %s' % bad), sys.exit(1))"
    $t = Invoke-Tool $Py @('-I', '-c', $code, (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    $u = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'config::ac130_tests') 'config_ac130_tests' $Mod
    $uok = $u.Code -eq 0 -and $u.Out -match 'config_ac130_defaults_bounds_and_cold_keys \.\.\. ok'
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:' -and $uok) { Add-Result 10 'config' 'PASS' ("{0}; {1}; config::ac130_tests ok (defaults, bounds, cold keys, unknown [ac130] key rejected)" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 10 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code); ac130_tests exit $($u.Code)" }
}

# 11. wiring (section 14.5) with its self-test mutations
Run-Check 11 'wiring' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ac130wiring', '--self-test') 'ac130wiring'
    Add-Result 11 'wiring' $(if ($r.Code -eq 0 -and $r.Out -match 'self-test: \d+ mutations of the M4 sources caught') { 'PASS' } else { 'FAIL' }) (((@($r.Out -split "`n" | Where-Object { $_ -match '^(self-test|PASS|FAIL)' }) | ForEach-Object { $_.Trim() }) -join ' | '))
}

# 12. the built DLL == the staged copy, with the AC-130 proof strings and the thermal shaders, without symphonia
Run-Check 12 'DLL proof strings' {
    $a = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ac130strings', $BuiltDll) 'ac130strings'
    $pe = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'pe', $BuiltDll) 'pe'
    $same = (Test-Path -LiteralPath $StagedDll) -and ((Get-FileHash -LiteralPath $BuiltDll -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash)
    $noSym = $pe.Out -notmatch 'symphonia'
    if ($a.Code -eq 0 -and $pe.Code -eq 0 -and $same -and $noSym) { Add-Result 12 'DLL proof strings' 'PASS' ("{0}; imports carry no symphonia; build\natives\mw2er.dll == target (sha256 {1})" -f (Last-Line $a.Out '^PASS'), (Get-FileHash -LiteralPath $StagedDll -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant()) }
    else { Add-Result 12 'DLL proof strings' 'FAIL' ("{0}; pe exit {1}; staged copy identical: {2} (check_m2.ps1 stages it)" -f (Last-Line $a.Out '(PASS|FAIL)'), $pe.Code, $same) }
}

# 13. analyzer self-test incl. the M4 fixture and its mutations
Run-Check 13 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    $f = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), (Join-Path $Scripts 'fixtures\m4_ac130_session.log')) 'analyze_m4_fixture'
    # the six M4 rows (M6 adds 'ac130 start', judged by check_m6)
    $acRows = @($f.Out -split "`n" | Where-Object { $_ -match '^PASS\s+ac130 (lifecycle|protection|guns|explosions|perf|effects)\s' }).Count
    $ok = $r.Code -eq 0 -and $r.Out -match 'M4 fixture m4_ac130_session\.log: good ride PASS on all 6 AC-130 rows, \d+ mutations FAIL, older logs N/A' -and $f.Code -eq 0 -and $acRows -eq 6
    Add-Result 13 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("self-test exit {0}; fixture: {1} AC-130 rows PASS, {2}" -f $r.Code, $acRows, (Last-Line $f.Out 'IN-GAME ORACLES'))
}

# 14. hygiene: gitignore, no MW2 script/vision text in our sources, nothing new in the install folders (listing only)
Run-Check 14 'hygiene' {
    $problems = New-Object System.Collections.ArrayList
    $gi = Get-Content -LiteralPath (Join-Path $Root '.gitignore') -ErrorAction SilentlyContinue
    foreach ($p in @('converted/ac130/x', 'dumps/ac130/x')) {
        $top = '/' + ($p -split '/')[0] + '/'
        if (-not ($gi | Where-Object { $_.Trim() -eq $top })) { [void]$problems.Add("$p not ignored (no $top in .gitignore)") }
    }
    $code = @"
import os, pathlib, sys
root = pathlib.Path(sys.argv[1]); dumps = root / 'dumps' / 'ac130' / 'common_mp'
srcs = [dumps / 'maps' / 'mp' / 'killstreaks' / '_ac130.gsc', dumps / 'vision' / 'ac130.vision', dumps / 'vision' / 'ac130_inverted.vision']
lines = set()
for s in srcs:
    for l in s.read_text(encoding='cp1252', errors='replace').splitlines():
        t = ''.join(l.split())
        if len(t) >= 20: lines.add(t)
hits, n = [], 0
files = []
for base in (root / 'mod', root / 'tools' / 'scripts'):
    for dp, dn, fn in os.walk(base):
        dn[:] = [d for d in dn if d not in ('target', '__pycache__')]
        files += [pathlib.Path(dp) / f for f in fn]
for p in files:
        if p.suffix.lower() in ('.exe', '.dll', '.pdb', '.rlib', '.cso', '.wav', '.vmtex'):
            continue
        n += 1
        text = ''.join(p.read_text(encoding='utf-8', errors='replace').split())
        for t in lines:
            if t in text: hits.append('%s: %s' % (p.relative_to(root), t[:40]))
print(('FAIL ' + '; '.join(hits[:3])) if hits else 'PASS %d files under mod\\ and tools\\scripts\\: none contains any of the %d script/vision lines (>= 20 non-space chars)' % (n, len(lines)))
sys.exit(1 if hits else 0)
"@
    $t = Invoke-Tool $Py @('-I', '-c', $code, $Root) 'hygiene_text'
    if ($t.Code -ne 0) { [void]$problems.Add($t.Out.Trim()) }
    $h = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'hygiene') 'hygiene_magic'
    if ($h.Code -ne 0) { [void]$problems.Add((Last-Line $h.Out 'FAIL')) }
    # install folders (read-only listings; review M4 minor 13): the top level is compared with the checked-in vanilla
    # baseline fixtures\install_baseline.json (never a self-recorded one), and both folders are scanned RECURSIVELY for
    # loader / mod file names (dinput8.dll, mw2er*, me3*, *.me3, modengine*, regulation.bin.* and a second regulation.bin)
    $baseline = Get-Content -LiteralPath (Join-Path $Scripts 'fixtures\install_baseline.json') -Raw | ConvertFrom-Json
    $new = @()
    $risky = @()
    $nfiles = 0
    foreach ($d in @($ErGame, $Mw2Dir)) {
        if (-not (Test-Path -LiteralPath $d)) { [void]$problems.Add("install folder missing: $d"); continue }
        $old = @($baseline.$d)
        if ($old.Count -eq 0) { [void]$problems.Add("no baseline for $d in fixtures\install_baseline.json"); continue }
        $top = @(Get-ChildItem -LiteralPath $d -Force | ForEach-Object { $_.Name })
        $new += @($top | Where-Object { $old -notcontains $_ } | ForEach-Object { "$d\$_" })
        $all = @(Get-ChildItem -LiteralPath $d -Force -Recurse -File -ErrorAction SilentlyContinue)
        $nfiles += $all.Count
        foreach ($f in $all) {
            $n = $f.Name.ToLowerInvariant()
            $isGameReg = ($n -eq 'regulation.bin' -and $f.DirectoryName -eq $ErGame)
            if (($n -eq 'dinput8.dll') -or $n.StartsWith('mw2er') -or $n.StartsWith('me3') -or $n.EndsWith('.me3') -or
                $n.StartsWith('modengine') -or $n.StartsWith('regulation.bin.') -or ($n -eq 'regulation.bin' -and -not $isGameReg)) {
                $risky += $f.FullName
            }
        }
    }
    # co-op (MODLOG 2026-10-19): Seamless Co-op running in the game process creates <ER Game>\SeamlessCoop\crashdumps (empty) on
    # its first start. Accepted only while it holds no file at all (a crash dump there FAILs: it may contain mw2er memory).
    $new = @($new | Where-Object { -not ((Split-Path -Leaf $_) -eq 'SeamlessCoop' -and (Split-Path -Parent $_) -eq $ErGame -and @(Get-ChildItem -LiteralPath $_ -Recurse -File -Force -ErrorAction SilentlyContinue).Count -eq 0) })
    if ($new.Count -gt 0) { [void]$problems.Add("top-level entries not in the vanilla baseline: $($new -join ', ')") }
    if ($risky.Count -gt 0) { [void]$problems.Add("loader/mod files inside the install folders: $(($risky | Select-Object -First 5) -join ', ')") }
    $scan = "top level == checked-in vanilla baseline; recursive scan of $nfiles files: no dinput8/mw2er*/me3*/modengine*/regulation.bin.* files"
    $gf = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    if ($gf.Code -ne 0) { [void]$problems.Add((Last-Line $gf.Out 'FAIL')) }
    if ($problems.Count -eq 0) { Add-Result 14 'hygiene' 'PASS' ("converted\ac130 + dumps\ac130 ignored; {0}; {1}; {2}" -f $t.Out.Trim(), $scan, (Last-Line $gf.Out '^PASS')) }
    else { Add-Result 14 'hygiene' 'FAIL' ($problems -join ' | ') }
}

# 16. run-8 tracers: TR1-TR4 with the debug layer and with GBV (a tracer at a known world position lands at the expected
#     pixel, GPU == CPU, before the thermal film, the instance ring, the failure path), 4K worst-case GPU time < 0.2 ms
Run-Check 16 'vmtest tracers' {
    $a = Invoke-Tool $Vmtest @('--tracers', '--out', $OutDir) 'vmtest_tracers'
    $b = Invoke-Tool $Vmtest @('--tracers', '--gbv', '--out', $OutDir) 'vmtest_tracers_gbv'
    $p = Invoke-Tool $Vmtest @('--tracers-perf', '--out', $OutDir) 'vmtest_tracers_perf'
    $ta = @($a.Out -split "`n" | Where-Object { $_ -match '^PASS TR[1-4] ' }).Count
    $tb = @($b.Out -split "`n" | Where-Object { $_ -match '^PASS TR[1-4] ' }).Count
    $ok = $a.Code -eq 0 -and $ta -eq 4 -and $a.Out -match 'TRACERS PASS \(0 FAIL\)' -and $a.Out -match 'PASS TRinfoqueue 0 ERROR' -and
          $b.Code -eq 0 -and $tb -eq 4 -and $b.Out -match 'debug layer on \+ GBV' -and $b.Out -match 'TRACERS PASS \(0 FAIL\)' -and
          $p.Code -eq 0 -and $p.Out -match 'TRACERS PERF PASS' -and $p.Out -match '@3840x2160'
    Add-Result 16 'vmtest tracers' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("debug {0}/4, GBV {1}/4 TR cases PASS; {2}; {3}" -f $ta, $tb, (Last-Line $a.Out '^PASS TR1 ').Substring(0, [Math]::Min(150, (Last-Line $a.Out '^PASS TR1 ').Length)), (Last-Line $p.Out 'PERF'))
}

# 15. T10 (review M4 major 8): the thermal film on a REAL Elden Ring frame (work\m4\er_frame_ref.png, the user's own
#     screenshot) at the configured [ac130] exposure: clipped white/black under the limits. NEEDS-DATA (not a failure)
#     until that frame exists; it is reported in the summary.
Run-Check 15 'real ER frame (T10)' {
    $l = $script:T10Line
    if (-not $l) { Add-Result 15 'real ER frame (T10)' 'FAIL' 'no T10 line from check 7' }
    elseif ($l -match '^PASS ') { Add-Result 15 'real ER frame (T10)' 'PASS' $l }
    elseif ($l -match '^NEEDS-DATA ') { Add-Result 15 'real ER frame (T10)' 'NEEDS-DATA' $l }
    else { Add-Result 15 'real ER frame (T10)' 'FAIL' $l }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
$needs = @($results | Where-Object { $_.Status -eq 'NEEDS-DATA' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP, $needs NEEDS-DATA of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run, docs\M4_DESIGN.md section 15): the camera-step hook in mode 2 on the live CSCamera, ER''s explosion bullets and their damage on real enemies (HIT src=ac130), the protection SpEffect against fire/blast, the hud_state hide/restore, the thermal pass on the real swap chain (gpu_ms), ER''s own input path (er_yaw), the sky ray, ambient/loop audio; run 8: the flat explosion damage per tier (HIT src=ac130), the tracers in the real swap chain (AC130 tracer gpu_ms), the brighter thermal look (exposure 0.52, gamma 0.75); run 9 effects: ER assets breaking in the blast (atkObj 200/100/40 vs asset defence), the impact SFX (burst + dust) and their orientation (sfxPostureType 1), the camera shake feel (AC130 SHAKE), the bigger 105 flash/bloom.'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM3b -and $results.Count -eq 16) {
    Write-Output 'M4 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M4 OFFLINE ORACLES: FAIL'
exit 1
