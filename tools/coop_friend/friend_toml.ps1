<#
.SYNOPSIS
  The friend package's mw2er.toml policy (MODLOG 2026-10-22 "installer P0"): the friend toml is the host's mw2er.toml
  with every test probe / unfinished experiment forced OFF, and the in-game-tested features required ON.
  Dot-sourced by make_friend_package.ps1 (not shipped). ConvertTo-FriendToml returns the friend text or throws.

.DESCRIPTION
  Forced (rewritten line by line, section-aware; a key absent from the host toml is left to its code default, which is
  the same safe value): $FriendForced below. Required ON (the host toml must already have them, else the package is
  refused, so the friend never silently gets a different feature set): $FriendRequired. Run this file directly with
  -SelfTest for its unit tests.
#>
param([switch]$SelfTest)

# section -> key -> forced value (TOML literal), with the reason written into the friend toml's comment
$FriendForced = [ordered]@{
    'nuke'  = [ordered]@{ always_available = @('false', 'the nuke needs the 25-kill streak (the host may test with true)'); probe_forward = @('false', 'not built; must stay false') }
    'ui'    = [ordered]@{ probe_scaleform = @('false', 'test probe off'); probe_layer_sample = @('false', 'test probe off') }
    'movement' = [ordered]@{ write_last_update = @('false', 'probe off'); fp_anim_speed_scale = @('false', 'not built; must stay false') }
    'coop'  = [ordered]@{
        probe_er_depth        = @('false', 'test probe off')
        probe_er_depth_state  = @('false', 'test probe off')
        probe_er_depth_sample = @('false', 'test probe off')
        probe_er_aim          = @('false', 'test probe off')
        world_gun_er_depth    = @('false', 'ER-depth occlusion is a test; body capsules + map rays stay on')
        er_carrier            = @('"off"', 'crossbow carrier experiment stopped (2026-10-22)')
        carrier_give          = @('false', 'never give an item into the save')
        carrier_load          = @('false', 'carrier experiment off')
        tp_ads_er_aim         = @('false', 'old probe alias of tp_ads_pose = "er_aim"')
        tp_aim_cam            = @('"keep"', 'acts only with tp_ads_pose = "er_aim"; keep = our normal camera')
    }
}

# section -> key -> required value (TOML literal as written in the host toml) = the in-game-tested features shipped ON
$FriendRequired = [ordered]@{
    'keys'        = [ordered]@{ heal = '"Q"' }
    'ui'          = [ordered]@{ inject_viewmodel = 'true'; inject_ui_thread = 'true' }
    'ui_yield'    = [ordered]@{ mode = '"cutout"' }
    'gamepad'     = [ordered]@{ enabled = 'true'; er_use_item = '"x"' }
    'gamepad.bind' = [ordered]@{ heal = '"dpad_right"' }
    'first_person' = [ordered]@{ tp_scope_peek = 'true' }
    'nuke'        = [ordered]@{ allow_in_coop = 'true' }
    'coop'        = [ordered]@{
        hold_mode = '"hand"'; er_stance = '"polearm"'; tp_ads_pose = '"guard"'; tp_guard_yaw = 'true'; tp_ads_hold = '"aim"'
        hide_er_weapon = 'true'; hide_state_patch = 'true'; world_gun_body_occlusion = 'true'; world_gun_occlusion = 'true'; world_guns = 'true'
    }
}
# [third_person] shoulder offsets must be non-zero (the accepted shoulder camera), checked separately
$FriendShoulderKeys = @('shoulder_offset_m', 'shoulder_ads_offset_m')

# One entry per key line: Index (line number, 0-based), Section, Key, Value (text before any comment, trimmed).
function Get-TomlKeyLines([string[]]$Lines) {
    $out = New-Object System.Collections.ArrayList
    $sec = ''
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        $t = $Lines[$i].Trim()
        if ($t -match '^\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$') { $sec = $Matches[1]; continue }
        if ($t -match '^([A-Za-z0-9_]+)\s*=\s*(.*)$') {
            $k = $Matches[1]; $rest = $Matches[2]
            # value up to a '#' that is outside a "..." string
            $v = ''; $inq = $false
            foreach ($ch in $rest.ToCharArray()) { if ($ch -eq '"') { $inq = -not $inq }; if ($ch -eq '#' -and -not $inq) { break }; $v += $ch }
            [void]$out.Add([pscustomobject]@{ Index = $i; Section = $sec; Key = $k; Value = $v.Trim() })
        }
    }
    return $out
}

function ConvertTo-FriendToml([string]$Toml) {
    $nl = if ($Toml.Contains("`r`n")) { "`r`n" } else { "`n" }
    $lines = $Toml -split "`r?`n"
    $kl = Get-TomlKeyLines $lines
    $problems = New-Object System.Collections.ArrayList
    $changed = New-Object System.Collections.ArrayList
    foreach ($sec in $FriendForced.get_Keys()) {
        foreach ($key in $FriendForced[$sec].get_Keys()) {
            $val, $why = $FriendForced[$sec][$key]
            $hits = @($kl | Where-Object { $_.Section -eq $sec -and $_.Key -eq $key })
            if ($hits.Count -gt 1) { [void]$problems.Add("[$sec] $key appears $($hits.Count) times"); continue }
            if ($hits.Count -eq 0) { continue }   # absent = the code default, which is this same safe value
            $h = $hits[0]
            if ($h.Value -ne $val) { [void]$changed.Add("[$sec] $key $($h.Value) -> $val") }
            $lines[$h.Index] = ('{0,-20} = {1,-8} # friend package: {2}' -f $key, $val, $why)
        }
    }
    foreach ($sec in $FriendRequired.get_Keys()) {
        foreach ($key in $FriendRequired[$sec].get_Keys()) {
            $want = $FriendRequired[$sec][$key]
            $hits = @($kl | Where-Object { $_.Section -eq $sec -and $_.Key -eq $key })
            if ($hits.Count -ne 1) { [void]$problems.Add("[$sec] $key must appear once (found $($hits.Count))"); continue }
            if ($hits[0].Value -ne $want) { [void]$problems.Add("[$sec] $key = $($hits[0].Value) (the friend package ships the tested $want)") }
        }
    }
    foreach ($key in $FriendShoulderKeys) {
        $hits = @($kl | Where-Object { $_.Section -eq 'third_person' -and $_.Key -eq $key })
        if ($hits.Count -ne 1) { [void]$problems.Add("[third_person] $key must appear once"); continue }
        $nums = @([regex]::Matches($hits[0].Value, '-?\d+(\.\d+)?') | ForEach-Object { [double]$_.Value })
        if ($nums.Count -ne 3 -or -not ($nums | Where-Object { $_ -ne 0 })) { [void]$problems.Add("[third_person] $key = $($hits[0].Value) (the friend package ships the accepted non-zero shoulder camera)") }
    }
    if ($problems.Count) { throw ('mw2er.toml does not fit the friend policy: ' + ($problems -join '; ')) }
    $script:FriendTomlChanges = @($changed)
    return ($lines -join $nl)
}

if ($SelfTest) {
    $fails = 0
    function T([string]$Name, [bool]$Ok) { if ($Ok) { Write-Output "ok   $Name" } else { Write-Output "FAIL $Name"; $script:fails++ } }
    $root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $host_ = Get-Content -LiteralPath (Join-Path $root 'mw2er.toml') -Raw
    $f = ConvertTo-FriendToml $host_
    $kl = Get-TomlKeyLines ($f -split "`r?`n")
    function V([string]$s, [string]$k) { ($kl | Where-Object { $_.Section -eq $s -and $_.Key -eq $k } | Select-Object -First 1).Value }
    T 'host toml converts' ($f.Length -gt 1000)
    foreach ($sec in $FriendForced.get_Keys()) { foreach ($key in $FriendForced[$sec].get_Keys()) { $v = V $sec $key; T "forced [$sec] $key = $($FriendForced[$sec][$key][0]) (got $v)" ($null -eq $v -or $v -eq $FriendForced[$sec][$key][0]) } }
    foreach ($sec in $FriendRequired.get_Keys()) { foreach ($key in $FriendRequired[$sec].get_Keys()) { T "required [$sec] $key" ((V $sec $key) -eq $FriendRequired[$sec][$key]) } }
    T 'gamepad heal untouched by [keys] heal' ((V 'gamepad.bind' 'heal') -eq '"dpad_right"' -and (V 'keys' 'heal') -eq '"Q"')
    T 'line count unchanged' (($f -split "`r?`n").Count -eq ($host_ -split "`r?`n").Count)
    T 'idempotent' ((ConvertTo-FriendToml $f) -eq $f)
    # mutations: each must be refused
    $muts = @(
        @('tp_ads_pose off', '(?m)^tp_ads_pose\s*=\s*"guard"', 'tp_ads_pose = "off"'),
        @('er_stance off', '(?m)^er_stance\s*=\s*"polearm"', 'er_stance = "off"'),
        @('cutout off', '(?m)^mode\s*=\s*"cutout"', 'mode = "lower"'),
        @('inject off', '(?m)^inject_ui_thread\s*=\s*true', 'inject_ui_thread = false'),
        @('shoulder zero', '(?m)^shoulder_offset_m\s*=\s*\[[^\]]*\]', 'shoulder_offset_m = [0.0, 0.0, 0.0]'),
        @('duplicate probe', '(?m)^(probe_er_aim\s*=[^\r\n]*)', "`$1`r`nprobe_er_aim = true"),
        @('pad heal gone', '(?m)^heal\s*=\s*"dpad_right"', 'heal = ""')
    )
    foreach ($m in $muts) {
        $bad = [regex]::Replace($host_, $m[1], $m[2])
        $caught = $false
        if ($bad -eq $host_) { T "mutation $($m[0]) applies" $false; continue }
        try { [void](ConvertTo-FriendToml $bad) } catch { $caught = $true }
        T "mutation $($m[0]) refused" $caught
    }
    # a probe turned on in the host is forced off again
    $on = [regex]::Replace($host_, '(?m)^probe_scaleform\s*=\s*false', 'probe_scaleform = true')
    $kl = Get-TomlKeyLines ((ConvertTo-FriendToml $on) -split "`r?`n")
    T 'probe_scaleform true in host -> false in friend' ((V 'ui' 'probe_scaleform') -eq 'false')
    if ($fails) { Write-Output "FAIL friend_toml self-test: $fails failed"; exit 1 }
    Write-Output 'PASS friend_toml self-test'
    exit 0
}
