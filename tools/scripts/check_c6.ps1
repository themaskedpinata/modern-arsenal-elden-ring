<#
.SYNOPSIS
  Runs the offline oracles of C6 (docs\C6_HOLD_DESIGN.md section 10: the MW2 gun in the ER character's hand, the
  crossbow stance) and prints PASS / FAIL per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3, Steam or Seamless; never writes a game folder or
  %APPDATA%\EldenRing; never rebuilds the regulation or the friend package. Reads the vanilla unpack and the unpacked
  staged package (work\regulation\...) as files. Writes only to work\check_c6\ plus cargo's target directory.
  Exit code 0 only if all 11 checks PASS. Check 6 also requires the part-16 carrier off (er_carrier = "off", carrier_give =
  false, carrier_load = false) in the shipped toml; part 17: tp_guard_yaw = false; part 18: [third_person] shoulder_offset_m = shoulder_ads_offset_m = [0, 0, 0] (er_stance = "off" and tp_ads_pose = "off" keep the polearm stance and the guard off).

  Checks: 1 core c6 tests  2 core tests (all)  3 DLL c6 tests  4 release build (0 warnings)  5 workspace check
          (all targets, 0 warnings)  6 config defaults (shipped mw2er.toml keeps every C6 key at its safe default; FAILs
          while the toml is set up for an in-game C6 test)  7 analyzer (analyze_c6.py --self-test)  8 regulation
          (check_c6_regulation.py + its self-test: the stance and the weapon hide are runtime patches, the package sha256
          is unchanged)  9 DLL proof strings (the HOLD / COOP / FP / DEPTH lines are in the built DLL)  10 vmtest
          --occluders --warp (C6 depth fallback: the body capsules in our world-gun depth, debug layer)  11 vmtest --erdepth
          --warp --gbv (C6 depth 5: ER's depth copied / sampled through our SRV, the prime, the real barrier detour, the
          GBV negative control, C6 round 6 ED8: two candidates, the control, the census; debug layer + GPU-based validation)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_c6.ps1
#>
[CmdletBinding()]
param()

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'   # always invoked with -I
$Mod = Join-Path $Root 'mod'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$LogDir = Join-Path $Root 'work\check_c6'
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

Run-Check 1 'core c6 tests' {
    $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib', 'c6_') 'core_c6' $Mod
    $ok = $r.Code -eq 0 -and $r.Out -match 'test result: ok\. (\d+) passed; 0 failed' -and [int]$Matches[1] -ge 56
    Add-Result 1 'core c6 tests' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out 'test result')
}

Run-Check 2 'core tests' {
    $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--lib') 'core_all' $Mod
    $ok = $r.Code -eq 0 -and $r.Out -match 'test result: ok\. \d+ passed; 0 failed'
    Add-Result 2 'core tests' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out 'test result')
}

Run-Check 3 'DLL c6 tests' {
    $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er', '--lib', 'c6_') 'dll_c6' $Mod
    $ok = $r.Code -eq 0 -and $r.Out -match 'test result: ok\. (\d+) passed; 0 failed' -and [int]$Matches[1] -ge 2
    Add-Result 3 'DLL c6 tests' $(if ($ok) { 'PASS' } else { 'FAIL' }) ((Last-Line $r.Out 'test result') + ' (stance + hide offsets = vendored EQUIP_PARAM_WEAPON_ST / SP_EFFECT_PARAM_ST)')
}

Run-Check 4 'release build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '-p', 'mw2er') 'build' $Mod
    $warn = @($r.Out -split "`n" | Where-Object { $_ -match '^warning' }).Count
    $ok = $r.Code -eq 0 -and $warn -eq 0 -and (Test-Path -LiteralPath $BuiltDll)
    Add-Result 4 'release build' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("exit={0} warnings={1} {2}" -f $r.Code, $warn, (Last-Line $r.Out 'Finished|error'))
}

Run-Check 5 'workspace check' {
    $r = Invoke-Tool 'cargo' @('check', '--workspace', '--all-targets') 'check_ws' $Mod
    $warn = @($r.Out -split "`n" | Where-Object { $_ -match '^warning' }).Count
    $ok = $r.Code -eq 0 -and $warn -eq 0
    Add-Result 5 'workspace check' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("exit={0} warnings={1}" -f $r.Code, $warn)
}

Run-Check 6 'config defaults' {
    # the shipped toml = the host's toml under the release policy (friend_toml.ps1); the host's own toml holds test values
    . (Join-Path $Root 'tools\coop_friend\friend_toml.ps1')
    $toml = ConvertTo-FriendToml (Get-Content -LiteralPath (Join-Path $Root 'mw2er.toml') -Raw)
    $rel = Join-Path $Root 'work\c6\release_mw2er.toml'
    New-Item -ItemType Directory -Force (Split-Path $rel) | Out-Null
    [IO.File]::WriteAllText($rel, $toml)
    $env:MW2ER_RELEASE_TOML = $rel
    try { $t = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_file' $Mod }
    finally { Remove-Item Env:MW2ER_RELEASE_TOML }
    $keys = @('hold_mode\s*=\s*"hand"', 'er_stance\s*=\s*"polearm"', 'tp_ads_er_aim\s*=\s*false', 'tp_ads_hold\s*=\s*"aim"', 'stance_refresh\s*=\s*"chord"', 'tp_scope_peek\s*=\s*true', 'world_gun_body_occlusion\s*=\s*true', 'probe_er_depth\s*=\s*false', 'tp_ads_pose\s*=\s*"guard"', 'probe_er_aim\s*=\s*false', 'probe_er_depth_state\s*=\s*false', 'probe_er_depth_sample\s*=\s*false', 'world_gun_er_depth\s*=\s*false', 'er_carrier\s*=\s*"off"', 'carrier_give\s*=\s*false', 'carrier_load\s*=\s*false', 'tp_guard_yaw\s*=\s*true')
    $missing = @($keys | Where-Object { $toml -notmatch "(?m)^\s*$_" })
    $ok = $t.Code -eq 0 -and $t.Out -match 'PASS: C6 release set' -and $missing.Count -eq 0
    Add-Result 6 'config defaults' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0}{1}" -f (Last-Line $t.Out 'PASS: C6|panicked'), $(if ($missing.Count) { ' missing: ' + ($missing -join ', ') } else { '' }))
}

Run-Check 7 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_c6.py'), '--self-test') 'analyze_c6_selftest'
    $ok = $r.Code -eq 0 -and $r.Out -match 'PASS analyze_c6 self-test: (\d+)/(\d+) mutations caught' -and $Matches[1] -eq $Matches[2]
    Add-Result 7 'analyzer' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out 'self-test')
}

Run-Check 8 'regulation' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'check_c6_regulation.py')) 'c6_regulation'
    $s = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'check_c6_regulation.py'), '--self-test') 'c6_regulation_selftest'
    $ok = $r.Code -eq 0 -and $r.Out -match 'C6 REGULATION: PASS' -and $s.Code -eq 0 -and $s.Out -match 'PASS check_c6_regulation self-test'
    Add-Result 8 'regulation' $(if ($ok) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f (Last-Line $r.Out 'C6 REGULATION|FAIL'), (Last-Line $s.Out 'mutations'))
}

Run-Check 9 'DLL proof strings' {
    if (-not (Test-Path -LiteralPath $BuiltDll)) { Add-Result 9 'DLL proof strings' 'FAIL' "missing $BuiltDll"; return }
    $bytes = [System.IO.File]::ReadAllBytes($BuiltDll)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    $want = @('cfg hold_mode=', 'hand ok who=', 'fallback who=', 'stance enter row=', 'stance exit row=', 'chord sent route=', 'stray_bullet row=', 'peer_stance peer=', 'bone probe t=', 'weapon ok who=', 'weapon miss who=', 'weapon probe t=', 'weapon axes auto axes=', 'src who=', 'hide row patched sp=', 'hide foreign sp=', 'marker renewed sp=', ' peek=1', ' tp_scope_peek=1 peek_arm=', 'grip who=self ', ' accepted=', 'stance resume row=', 'stance exit_defer s=', 'body bones found=', 'body ok who=', 'body miss who=', 'worldgun body occlusion first frame', 'worldgun occluders ready', 'chain rs=0x', 'probe verdict chain=', 'probe wrap t=', 'aim patch row=', 'aim on t=', 'aim fail reason=no_state', 'aim restore row=', 'aim align gun=', 'aim align_clamped gun=', 'aim probe anim=', ' ads_pose=', 'stance exit_chords gave_up', 'stance exit chords=0', ' kind=', 'state hook on target=0x', 'state hook off why=', 'state verdict ready=', 'state path seq=', 'probe sample n=', 'probe self n=', 'live on res=0x', 'live off why=', 'live trip why=', 'live first prime slot=', 'reader ready device=0x', 'worldgun er depth prime ready', ' depth_state=', 'aim cam keep installed getter=0x', 'aim cam kept ms=', 'aim cam refused reason=', ' aim_cam=', 'state control B ui=', 'census res=0x', ' ret_fade=0x', ' hits_fade=', ' snipe=', 'aim pose phase=', 'aim hold_fallback gun=', 'probe self_s k=', ' scale=', ' n_eff=', ' lev=', ' zd=', 'live on res=0x', ' near_rel=', 'chord verdict=', 'chord verdict=ignored head=', 'stance carrier enter row=', 'stance carrier exit row=', 'aim hold row=', ' carrier=', 'xbow dry slot=', 'xbow pins ok equip_item=0x', 'xbow call slot=', 'xbow on slot=', 'xbow restore why=', 'xbow restore call why=', 'xbow trip why=bolt_used qty=', 'xbow grip via=arm_style arm=', 'xbow grip fail row=', 'xbow foreign slot=', 'xbow restore_missing orig=', 'xbow off why=', 'xbow skip reason=', 'xbow refused reason=', 'xbow journal found slot=', 'xbow give id=', 'xbow parts phase=', 'xbow frame phase=', 'aim episode n=', ' aim_at_ms=', ' camtypes=[', 'chord done arm=', 'chord abort_after_played route=', 'xbow grip via=chord arm=', 'xbow load write slot=', 'xbow load method=write slot=', 'xbow load undo why=', 'xbow load wait reason=', 'xbow load lost loaded=', ' grip_via=', 'guard on row=', 'guard detected ms=', 'guard fallback ms=', 'guard episode n=', ' guard_anim=', 'guard yaw write ok=', 'guard align gun=', 'guard align_clamped gun=', 'guard align_unclamped gun=', ' carry_age=', 'guard hold_fallback gun=', 'stance set change new=', ' set=', ' guard_yaw=', 'guard frame src=', 'guard frame auto row=', ' fwd_err=', ' vs_builtin=', ' polearm_grip=', ' guard_axes=', ' corr=', 'shoulder on side=', 'shoulder off why=', 'shoulder pull frac=', 'shoulder pull end ms=', 'shoulder unavailable reason=no_cam_hook', ' margin=')
    $missing = @($want | Where-Object { -not $text.Contains($_) })
    Add-Result 9 'DLL proof strings' $(if ($missing.Count -eq 0) { 'PASS' } else { 'FAIL' }) $(if ($missing.Count -eq 0) { "$($want.Count)/$($want.Count) HOLD / COOP strings in mw2er.dll" } else { 'missing: ' + ($missing -join ', ') })
}

Run-Check 10 'vmtest occluders' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '-p', 'vmtest') 'build_vmtest' $Mod
    $exe = Join-Path $Mod 'target\release\vmtest.exe'
    if ($r.Code -ne 0 -or -not (Test-Path -LiteralPath $exe)) { Add-Result 10 'vmtest occluders' 'FAIL' "vmtest build exit=$($r.Code)"; return }
    $v = Invoke-Tool $exe @('--occluders', '--warp', '--out', (Join-Path $LogDir 'vm')) 'vmtest_occluders'
    $ok = $v.Code -eq 0 -and $v.Out -match 'PASS VMTEST OCCLUDERS (\d+)/(\d+) cases' -and $Matches[1] -eq $Matches[2]
    Add-Result 10 'vmtest occluders' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $v.Out 'VMTEST OCCLUDERS|FAIL')
}

Run-Check 11 'vmtest erdepth' {
    $exe = Join-Path $Mod 'target\release\vmtest.exe'
    if (-not (Test-Path -LiteralPath $exe)) { Add-Result 11 'vmtest erdepth' 'FAIL' "missing $exe (check 10 builds it)"; return }
    $v = Invoke-Tool $exe @('--erdepth', '--warp', '--gbv', '--out', (Join-Path $LogDir 'vm')) 'vmtest_erdepth'
    $ok = $v.Code -eq 0 -and $v.Out -match 'PASS VMTEST ERDEPTH (\d+)/(\d+) cases' -and $Matches[1] -eq $Matches[2] -and [int]$Matches[2] -ge 8
    Add-Result 11 'vmtest erdepth' $(if ($ok) { 'PASS' } else { 'FAIL' }) (Last-Line $v.Out 'VMTEST ERDEPTH|FAIL')
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the in-game run, docs\C6_HOLD_DESIGN.md 11): which weapon-model matrix ER refreshes (HOLD weapon ok src=), that stateInfo 184 hides the sword (H1), weapon_axes auto (HOLD weapon axes auto), the third-person scope peek, the hand bone (HOLD bone), that ER plays the crossbow set after the chord (I1), that it never fires a bolt (I2), the chord buttons (I4), both hands hidden (I3), peers (I7), ER aim (I8), the grip table on the ER weapon frame (HOLD grip, the screenshot), the body capsule bone names (HOLD body bones), the depth chain on the live game (DEPTH probe), that the ER crossbow aim follows the patched type + the held aim input (HOLD aim on / aim fail reason=no_state, C6 aim 4) and its camera (C6 round 6: the ER follow camera and no crossbow reticle with tp_aim_cam = keep, HOLD aim cam kept / aim probe prec=1 camtype=0; round 7: the body never fades, aim probe snipe=1.00, and whether ER moves the arms at all, HOLD aim pose arm=), the state ER leaves its scene depth in at UiBegin and the ER-depth prime (DEPTH state verdict / probe self / live, C6 depth 5; round 7: the fitted chest offset, DEPTH probe self_s o=; part 16.8: the line fit s / n_eff / lev and live on near=n_eff), and part 16: whether vanilla ER raises the arms in a third-person crossbow aim (probe 0: HOLD aim pose ... mode=0 row=430200xx arm=1), that ER takes the carrier through its own EquipItem and syncs it to peers (HOLD xbow on, P-X4), the grip write (HOLD xbow grip via=arm_style), the hidden bolt model (HOLD xbow parts), the crossbow frame (HOLD xbow frame), the bolt count never drops (no xbow trip), the journal restores after a crash (xbow restore why=journal ok=1).'
if ($fail -eq 0 -and $results.Count -eq 11) {
    Write-Output 'C6 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'C6 OFFLINE ORACLES: FAIL'
exit 1
