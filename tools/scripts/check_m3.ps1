<#
.SYNOPSIS
  Runs every offline oracle of milestone M3 (after the whole M2 suite) and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, MW2, me3 or Steam. Writes only to work\check_m3\,
  work\m3\, converted\viewmodel\, mod\target and build\natives (via check_m2). Exit code 0 only if all 21 M2
  checks and all 18 M3 checks PASS; a SKIP is a failure unless -AllowSkip. -SkipM2 skips check 1 for fast
  iteration but always exits non-zero (the final run must include M2).

  Checks (docs\M3_DESIGN.md section 13):
   1 M2 suite            2 hudhook vendor + patch   3 iw4L pin            4 fxc + shaders
   5 converter self-test 6 convert                  7 anim oracle         8 cargo build (4 crates, 0 warnings)
   9 cargo test          10 cross-decoder           11 Blender reference  12 vmtest render (debug layer, GBV)
  13 silhouette IoU      14 performance (4K)        15 DLL shape + wiring 16 config
  17 analyzer            18 hygiene

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m3.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip, [switch]$SkipM2)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Mod = Join-Path $Root 'mod'
$Blender = 'C:\Program Files\Blender Foundation\Blender 5.2\blender.exe'
$Fxc = if ($env:MW2ER_FXC) { $env:MW2ER_FXC } else { 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe' }
$HudReg = Join-Path $env:USERPROFILE '.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\hudhook-0.9.3'
$HudVendor = Join-Path $Root 'vendor\hudhook-0.9.3'
$HudPatch = Join-Path $Root 'tools\patches\hudhook-0001-pre-imgui-hook.patch'
# C4 (docs\C4_UI_DESIGN.md 2.1): the ExecuteCommandLists callback, stacked on 0001 (same two files)
$HudPatch2 = Join-Path $Root 'tools\patches\hudhook-0002-ecl-callback.patch'
$HudPatchFiles = @('src/hooks/dx12.rs', 'src/renderer/backend/dx12.rs')
$Iw4l = Join-Path $Root 'vendor\iw4L'
$Iw4lRev = 'fc3d8eaf6045cb131eb74cd41954f0c4922b9913'
$Vmtest = Join-Path $Mod 'target\release\vmtest.exe'
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$M3 = Join-Path $Root 'work\m3'
$Converted = Join-Path $Root 'converted\viewmodel'
$LogDir = Join-Path $Root 'work\check_m3'
New-Item -ItemType Directory -Force -Path $LogDir, $M3 | Out-Null

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

# Relative file list (path|size|mtime) of the project, minus the folders M3 may write to.
function Snapshot-Tree {
    $skip = @('work\', 'converted\viewmodel\', 'mod\target\', 'logs\', 'build\', '.git\')
    Get-ChildItem -LiteralPath $Root -Recurse -File -Force -ErrorAction SilentlyContinue | ForEach-Object {
        $rel = $_.FullName.Substring($Root.Length + 1)
        if (-not ($skip | Where-Object { $rel.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) }) -and $rel -notmatch '^vendor\\[^\\]+\\target\\') {
            '{0}|{1}|{2}' -f $rel, $_.Length, $_.LastWriteTimeUtc.Ticks
        }
    }
}

Write-Output "M3 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, MW2, me3 or Steam. Logs: work\check_m3\"
Write-Output ''

# 1. the whole M2 suite first
if ($SkipM2) {
    Add-Result 1 'M2 suite' 'SKIP' '-SkipM2 given (the run cannot pass)'
} else {
    Run-Check 1 'M2 suite' {
        $r = Invoke-Tool 'powershell' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $Scripts 'check_m2.ps1')) 'check_m2'
        $pass = @($r.Out -split "`n" | Where-Object { $_ -match '^PASS\s' }).Count
        $bad = @($r.Out -split "`n" | Where-Object { $_ -match '^(FAIL|SKIP)\s' })
        if ($r.Code -eq 0 -and $pass -eq 21 -and $bad.Count -eq 0) { Add-Result 1 'M2 suite' 'PASS' "check_m2.ps1: 21 PASS, 0 FAIL/SKIP ($(Last-Line $r.Out '^SUMMARY'))" }
        else { Add-Result 1 'M2 suite' 'FAIL' "exit $($r.Code); $pass PASS; $($bad -join ' | ')" }
    }
}

# 2. hudhook vendored = registry copy + exactly patch 0001
Run-Check 2 'hudhook vendor' {
    $problems = @()
    if (-not (Test-Path -LiteralPath (Join-Path $HudVendor 'LICENSE'))) { $problems += 'vendor\hudhook-0.9.3\LICENSE missing' }
    if (-not (Test-Path -LiteralPath $HudReg)) { $problems += "registry copy $HudReg missing" }
    $regFiles = @(Get-ChildItem -LiteralPath $HudReg -Recurse -File -Force | ForEach-Object { $_.FullName.Substring($HudReg.Length + 1).Replace('\', '/') } | Sort-Object)
    $venFiles = @(Get-ChildItem -LiteralPath $HudVendor -Recurse -File -Force | Where-Object { $_.FullName -notmatch '\\target\\' } | ForEach-Object { $_.FullName.Substring($HudVendor.Length + 1).Replace('\', '/') } | Sort-Object)
    if (@(Compare-Object $regFiles $venFiles).Count -ne 0) { $problems += 'file lists differ from the registry copy' }
    $changed = @()
    foreach ($f in $regFiles) {
        $a = (Get-FileHash -LiteralPath (Join-Path $HudReg $f) -Algorithm SHA256).Hash
        $vb = Join-Path $HudVendor $f
        if (Test-Path -LiteralPath $vb) { if ((Get-FileHash -LiteralPath $vb -Algorithm SHA256).Hash -ne $a) { $changed += $f } }
    }
    if (@(Compare-Object @($changed | Sort-Object) @($HudPatchFiles | Sort-Object)).Count -ne 0) { $problems += "changed files [$($changed -join ', ')] != [$($HudPatchFiles -join ', ')]" }
    # C4: the vendored copy carries 0001 + 0002; 0002 reverse-applies first, then 0001
    $chk = Invoke-Tool 'git' @('-c', 'core.autocrlf=false', 'apply', '--check', '--reverse', '--directory=vendor/hudhook-0.9.3', $HudPatch2) 'hudhook_patch_check'
    if ($chk.Code -ne 0) { $problems += "git apply --check --reverse 0002 failed: $($chk.Out)" }
    $tmp = Join-Path $LogDir 'hudhook_pristine'
    if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
    Copy-Item -LiteralPath $HudVendor -Destination $tmp -Recurse
    $tmpRel = 'work/check_m3/hudhook_pristine'
    $rev2 = Invoke-Tool 'git' @('-c', 'core.autocrlf=false', 'apply', '--reverse', "--directory=$tmpRel", $HudPatch2) 'hudhook_patch_reverse_0002'
    $rev = Invoke-Tool 'git' @('-c', 'core.autocrlf=false', 'apply', '--reverse', "--directory=$tmpRel", $HudPatch) 'hudhook_patch_reverse'
    if ($rev2.Code -ne 0) { $problems += "reverse 0002 failed: $($rev2.Out)" }
    $diffAfter = @()
    foreach ($f in $HudPatchFiles) {
        if ((Get-FileHash -LiteralPath (Join-Path $tmp $f) -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath (Join-Path $HudReg $f) -Algorithm SHA256).Hash) { $diffAfter += $f }
    }
    Remove-Item -LiteralPath $tmp -Recurse -Force
    if ($rev.Code -ne 0 -or $diffAfter.Count -gt 0) { $problems += "reverse-applied tree differs from the registry: $($diffAfter -join ', ') $($rev.Out)" }
    $meta = Invoke-Tool 'cargo' @('metadata', '--format-version', '1') 'cargo_metadata' $Mod
    $hh = $null
    $json = @($meta.Out -split "`n" | Where-Object { $_.StartsWith('{') }) | Select-Object -First 1
    try { $hh = ($json | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'hudhook' } } catch { $problems += "cargo metadata not JSON: $_" }
    if (-not $hh -or $hh.version -ne '0.9.3' -or $hh.source -or ($hh.manifest_path -notmatch 'vendor\\hudhook-0\.9\.3\\Cargo\.toml$')) { $problems += "cargo metadata hudhook: version=$($hh.version) source=$($hh.source) manifest=$($hh.manifest_path)" }
    if ($problems.Count -eq 0) { Add-Result 2 'hudhook vendor' 'PASS' "vendor\hudhook-0.9.3 = registry copy + patch 0001 + C4 patch 0002 (exactly $($HudPatchFiles -join ', ')); reverse-apply (0002, then 0001) restores the registry bytes; cargo uses the path copy (0.9.3)" }
    else { Add-Result 2 'hudhook vendor' 'FAIL' ($problems -join ' | ') }
}

# 3. iw4L pinned, never built, not a dependency
Run-Check 3 'iw4L pin' {
    $head = (Invoke-Tool 'git' @('-C', $Iw4l, 'rev-parse', 'HEAD') '').Out.Trim()
    $notice = Test-Path -LiteralPath (Join-Path $Iw4l 'NOTICE')
    $license = Test-Path -LiteralPath (Join-Path $Iw4l 'LICENSE')
    $target = Test-Path -LiteralPath (Join-Path $Iw4l 'target')
    $meta = Invoke-Tool 'cargo' @('metadata', '--format-version', '1') '' $Mod
    $inMeta = $meta.Out -match 'vendor[\\/]+iw4L'
    if ($head -eq $Iw4lRev -and $notice -and $license -and -not $target -and -not $inMeta) { Add-Result 3 'iw4L pin' 'PASS' "HEAD $($head.Substring(0, 7)); NOTICE + LICENSE present; never built (no target\); not in cargo metadata" }
    else { Add-Result 3 'iw4L pin' 'FAIL' "HEAD=$head NOTICE=$notice LICENSE=$license target=$target in-metadata=$inMeta" }
}

# 8. cargo build (before 4/10-15 which need the binaries)
Run-Check 8 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--workspace', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core|mw2er-render|vmtest)[\\/]' -or $_ -match '`(mw2er|mw2er-core|mw2er-render|vmtest)` \(' -or $_ -match '\\mod\\') })
    $errors = @($r.Out -split "`n" | Where-Object { $_ -match '(^|\s)error(\[|:)' })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll) -and (Test-Path -LiteralPath $Vmtest)) {
        Add-Result 8 'cargo build' 'PASS' "4 crates (mw2er-core, mw2er-render, mw2er, vmtest) release; 0 warnings in mod\; mw2er.dll $((Get-Item $BuiltDll).Length) B, vmtest.exe present"
    } else { Add-Result 8 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s); $($errors.Count) error line(s): $((@($ours) + @($errors) | Select-Object -First 2) -join ' | ')" }
}

# 4. fxc + shader bytes (build output == what vmtest reports)
Run-Check 4 'fxc + shaders' {
    $outDirs = @(Get-ChildItem -LiteralPath (Join-Path $Mod 'target\release\build') -Directory -Filter 'mw2er-render-*' | ForEach-Object { Join-Path $_.FullName 'out' } | Where-Object { Test-Path (Join-Path $_ 'viewmodel_vs.cso') } | Sort-Object { (Get-Item (Join-Path $_ 'viewmodel_vs.cso')).LastWriteTime })
    $v = Invoke-Tool $Vmtest @('--version') 'vmtest_version'
    if (-not (Test-Path -LiteralPath $Fxc) -or $outDirs.Count -eq 0) { Add-Result 4 'fxc + shaders' 'FAIL' "fxc present=$(Test-Path -LiteralPath $Fxc); build outputs=$($outDirs.Count)"; return }
    $out = $outDirs[-1]
    $ok = $true; $det = @()
    foreach ($pair in @(@('viewmodel_vs.cso', 'VS_SHA256'), @('viewmodel_ps.cso', 'PS_SHA256'))) {
        $f = Join-Path $out $pair[0]
        $bytes = [IO.File]::ReadAllBytes($f)
        $magic = [Text.Encoding]::ASCII.GetString($bytes, 0, 4)
        $h = (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash.ToLowerInvariant()
        $m = [regex]::Match($v.Out, "$($pair[1])=([0-9a-f]{64})")
        if ($magic -ne 'DXBC' -or -not $m.Success -or $m.Groups[1].Value -ne $h) { $ok = $false }
        $det += "$($pair[0]) $magic $($h.Substring(0, 12))"
    }
    if ($ok -and $v.Code -eq 0) { Add-Result 4 'fxc + shaders' 'PASS' "fxc present; $($det -join '; '); equal to vmtest --version" }
    else { Add-Result 4 'fxc + shaders' 'FAIL' "$($det -join '; ') vs '$($v.Out.Trim())'" }
}

# 5. converter self-test
Run-Check 5 'converter self-test' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_viewmodel.py'), '--self-test') 'convert_selftest'
    Add-Result 5 'converter self-test' $(if ($r.Code -eq 0 -and $r.Out -match 'PASS self-test: 8 mutations') { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 6. convert (only converted\viewmodel\ and work\ may change)
Run-Check 6 'convert' {
    $before = @(Snapshot-Tree)
    $r = Invoke-Tool $Py @('-I', '-B', (Join-Path $Scripts 'convert_viewmodel.py')) 'convert'
    $after = @(Snapshot-Tree)
    $changed = @(Compare-Object $before $after | ForEach-Object { ($_.InputObject -split '\|')[0] } | Sort-Object -Unique)
    $man = Join-Path $Converted 'manifest.json'
    $hashOk = $false; $nfiles = 0
    if (Test-Path -LiteralPath $man) {
        $m = Get-Content -LiteralPath $man -Raw | ConvertFrom-Json
        $hashOk = $true
        foreach ($p in $m.files.PSObject.Properties) {
            $nfiles++
            $f = Join-Path $Converted $p.Name
            if (-not (Test-Path -LiteralPath $f) -or (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash.ToLowerInvariant() -ne $p.Value.sha256) { $hashOk = $false }
        }
    }
    if ($r.Code -eq 0 -and $hashOk -and $changed.Count -eq 0) { Add-Result 6 'convert' 'PASS' "$nfiles files + manifest.json in converted\viewmodel (every sha256 matches); nothing else changed. $(Last-Line $r.Out '^converted')" }
    else { Add-Result 6 'convert' 'FAIL' "exit $($r.Code); manifest hashes ok=$hashOk; changed outside converted\viewmodel and work: [$($changed -join ', ')]" }
}

# 7. anim oracle (1a Python, 1b, 1c, 1d, 1f-1k)
Run-Check 7 'anim oracle' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_viewmodel.py'), '--check') 'anim_oracle'
    $fails = @($r.Out -split "`n" | Where-Object { $_ -match '^FAIL' })
    Add-Result 7 'anim oracle' $(if ($r.Code -eq 0 -and $fails.Count -eq 0) { 'PASS' } else { 'FAIL' }) ((Last-Line $r.Out '(anim oracle|^FAIL)') + $(if ($fails) { ' | ' + ($fails -join ' | ') } else { '' }))
}

# 9. cargo test (core + render) with every required category
Run-Check 9 'cargo test' {
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '-p', 'mw2er-render') 'cargo_test' $Mod
    $okLines = @($c.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { ($_ -split ' ')[1] })
    $cats = [ordered]@{
        'vm_assets (truncation)' = '^vm_assets::tests::vm_assets_truncation'; 'vm_assets manifest' = '^vm_assets::tests::vm_assets_manifest'
        'vm_anim' = '^vm_anim::'; 'vm_pose sampling' = '^vm_pose::tests::vm_pose_sampling'; 'vm_pose loop' = '^vm_pose::tests::vm_pose_loop'
        'vm_pose crossfade' = '^vm_pose::tests::vm_pose_crossfade'; 'vm_pose FK' = '^vm_pose::tests::vm_pose_fk_known'
        'vm_proj' = '^vm_proj::'; 'cam_gate' = '^cam_gate::'; 'hit impact attribution' = '^hit_logic::tests::hit_logic_impact'
        'input ADS' = '^input_logic::tests::ads_'; 'hit replay (M2 run 2)' = '^hit_logic::tests::hit_logic_replay_m2_run2'; 'config [viewmodel]' = '^config::viewmodel_tests::'; 'render helpers' = '^tests::render_'
    }
    $missing = @($cats.Keys | Where-Object { $pat = $cats[$_]; -not ($okLines | Where-Object { $_ -match $pat }) })
    $failed = @($c.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    if ($c.Code -eq 0 -and $missing.Count -eq 0 -and $failed.Count -eq 0) { Add-Result 9 'cargo test' 'PASS' "$($okLines.Count) tests ok (core + render) across $($cats.Count) M3 categories" }
    else { Add-Result 9 'cargo test' 'FAIL' "exit $($c.Code); missing: $($missing -join ', '); failed: $($failed -join ' | ')" }
}

# 10. cross-decoder on the real data (1a Rust, 1e) + known answers
Run-Check 10 'cross-decoder' {
    if (-not (Test-Path -LiteralPath (Join-Path $Root 'dumps\common_mp_assets\xanim'))) { Add-Result 10 'cross-decoder' 'SKIP' 'dumps missing'; return }
    $env:MW2ER_ROOT = $Root
    try { $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'xanim_cross', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'xanim_cross' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    $passes = @($r.Out -split "`n" | Where-Object { $_ -match 'PASS:' } | ForEach-Object { $_.Trim() })
    if ($r.Code -eq 0 -and $passes.Count -eq 3) { Add-Result 10 'cross-decoder' 'PASS' ($passes -join ' | ') }
    else { Add-Result 10 'cross-decoder' 'FAIL' "exit $($r.Code); $($passes.Count)/3 PASS lines (see work\check_m3\xanim_cross.log)" }
}

# 11. Blender reference renders of the ORIGINAL GLBs
Run-Check 11 'Blender reference' {
    if (-not (Test-Path -LiteralPath $Blender)) { Add-Result 11 'Blender reference' 'SKIP' "Blender not found at $Blender"; return }
    $w = Invoke-Tool $Vmtest @('--write-poses', $M3) 'write_poses'
    Get-ChildItem -LiteralPath $M3 -Filter 'ref_*.mask' -ErrorAction SilentlyContinue | Remove-Item -Force
    $r = Invoke-Tool $Blender @('-b', '--factory-startup', '--python', (Join-Path $Scripts 'blender_ref.py'), '--', '--poses', (Join-Path $M3 'poses.json'), '--out', $M3) 'blender_ref'
    $masks = @(Get-ChildItem -LiteralPath $M3 -Filter 'ref_*.mask').Count
    $pass = Last-Line $r.Out '^PASS blender_ref'
    $self = $r.Out -match 'self-check 1 PASS'
    if ($w.Code -eq 0 -and $r.Code -eq 0 -and $pass -and $self -and $masks -eq 9 -and $pass -match '= 2972') { Add-Result 11 'Blender reference' 'PASS' "$pass" }
    else { Add-Result 11 'Blender reference' 'FAIL' "exit $($r.Code); masks $masks; $(Last-Line $r.Out '(FAIL|PASS)')" }
}

# 12. vmtest render cases with the D3D12 debug layer, then again with GPU-based validation
Run-Check 12 'vmtest render' {
    $a = Invoke-Tool $Vmtest @('--all', '--out', $M3) 'vmtest_all'
    $g = Invoke-Tool $Vmtest @('--all', '--gbv', '--out', $M3) 'vmtest_all_gbv'
    $cases = @($a.Out -split "`n" | Where-Object { $_ -match '^(PASS|FAIL) (D1|D2|D3|D4|D5|W|O|F|U|A1|A2|init|poses|infoqueue) ' } | ForEach-Object { ($_ -split ' ')[0] + ' ' + ($_ -split ' ')[1] })
    $fails = @($cases | Where-Object { $_ -match '^FAIL' })
    if ($a.Code -eq 0 -and $g.Code -eq 0 -and $fails.Count -eq 0 -and $cases.Count -ge 14 -and $a.Out -match 'VMTEST PASS' -and $g.Out -match 'VMTEST PASS') {
        Add-Result 12 'vmtest render' 'PASS' "$(Last-Line $a.Out '^adapter'): D1 D2 D3 D4 D5 W O F U A1 A2 PASS, 0 InfoQueue errors; again with GBV: PASS"
    } else { Add-Result 12 'vmtest render' 'FAIL' "exit $($a.Code)/$($g.Code); $($fails -join ', '); $(Last-Line $a.Out '^(FAIL|VMTEST)')" }
}

# 13. silhouette IoU vs Blender
Run-Check 13 'silhouette IoU' {
    $r = Invoke-Tool $Vmtest @('--compare', $M3) 'vmtest_compare'
    $rows = @($r.Out -split "`n" | Where-Object { $_ -match '^\w+\s+[\d.]+\s+\d+\s+\d+' } | ForEach-Object { ($_ -split '\s+')[0] + '=' + ($_ -split '\s+')[1] })
    Add-Result 13 'silhouette IoU' $(if ($r.Code -eq 0 -and $rows.Count -eq 9 -and $r.Out -match 'IOU PASS') { 'PASS' } else { 'FAIL' }) ("IoU (need >= 0.90): " + ($rows -join ' ') + " [covers mesh, skinning, projection only: Blender is posed from our own converted anims and camera convention, so it cannot catch anim decoding or framing errors; anim composition is covered by check 10's iw4L port]")
}

# 14. GPU time at 3840x2160 (hardware, no debug layer)
Run-Check 14 'performance' {
    $r = Invoke-Tool $Vmtest @('--perf', '--out', $M3) 'vmtest_perf'
    Add-Result 14 'performance' $(if ($r.Code -eq 0 -and $r.Out -match 'PERF PASS') { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '^PERF')
}

# 15. DLL shape + same shaders + viewmodel wiring
Run-Check 15 'DLL shape + wiring' {
    $pe = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'pe', $BuiltDll) 'pe_shape'
    $sh = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'shaders', $BuiltDll) 'shaders'
    $vw = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'vmwiring', '--self-test') 'vmwiring'
    if ($pe.Code -eq 0 -and $sh.Code -eq 0 -and $vw.Code -eq 0) { Add-Result 15 'DLL shape + wiring' 'PASS' "pe: only DllMain, no CRT imports | $(Last-Line $sh.Out '^PASS') | $(Last-Line $vw.Out '^self-test')" }
    else { Add-Result 15 'DLL shape + wiring' 'FAIL' "$(Last-Line $pe.Out '(PASS|FAIL)') | $(Last-Line $sh.Out '(PASS|FAIL)') | $(Last-Line $vw.Out '(PASS|FAIL)')" }
}

# 16. config incl. [viewmodel]
Run-Check 16 'config' {
    $t = Invoke-Tool $Py @('-I', '-c', "import tomllib,sys; d=tomllib.load(open(sys.argv[1],'rb')); v=d['viewmodel']; print('tomllib ok viewmodel.output=%s hip_fov=%s assets_dir=%s ads_button=%s' % (v['output'], v['hip_fov'], v['assets_dir'], d['keys']['ads_mouse_button']))", (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:') { Add-Result 16 'config' 'PASS' ("{0}; {1}" -f $t.Out.Trim(), (Last-Line $c.Out 'PASS:')) }
    else { Add-Result 16 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code)" }
}

# 17. analyzer self-test (HIT via/kill, c4311 35..75, VM rows)
Run-Check 17 'analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    Add-Result 17 'analyzer' $(if ($r.Code -eq 0 -and $r.Out -match 'c4311 band 79000105 35\.\.75') { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 18. hygiene: gitignore, no game data in tools/mod/docs, game folder untouched
Run-Check 18 'hygiene' {
    $gi = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gitignore') 'gitignore'
    $hy = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'hygiene') 'hygiene'
    $gf = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    if ($gi.Code -eq 0 -and $hy.Code -eq 0 -and $gf.Code -eq 0) { Add-Result 18 'hygiene' 'PASS' "gitignore covers /converted/ /work/ /dumps/; $(Last-Line $hy.Out '^PASS'); game folder untouched" }
    else { Add-Result 18 'hygiene' 'FAIL' "$(Last-Line $gi.Out '(PASS|FAIL)') | $(Last-Line $hy.Out '(PASS|FAIL)') | $(Last-Line $gf.Out '(PASS|FAIL)')" }
}

$sorted = $results | Sort-Object { [int]$_.Id }
$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks (order: $(($sorted | ForEach-Object { $_.Id + '=' + $_.Status }) -join ' '))"
Write-Output 'Not covered offline (needs the user''s run): the draw appearing in ER''s frame (hudhook callback on the present queue), ER''s swap-chain format / colour space / HDR, the in-game GPU time, brightness against ER''s image, ER''s sprint speed, RMB blanking for ADS, camera-gate thresholds and det sign, impact radius/window.'
Write-Output 'Not covered at all yet: framing against real MW2 (no MW2 screenshot of the UMP45 at a known cg_fov), the XAnim byte layout by a truly independent decoder (iw4L reads zone memory, not OAT files), the detail/normal/spec maps (colour map only).'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip) -and -not $SkipM2 -and $results.Count -eq 18) {
    Write-Output 'M3 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M3 OFFLINE ORACLES: FAIL'
exit 1
