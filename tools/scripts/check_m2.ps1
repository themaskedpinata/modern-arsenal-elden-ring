<#
.SYNOPSIS
  Runs every offline oracle of milestone M2 and prints PASS / FAIL / SKIP per check.

.DESCRIPTION
  Windows PowerShell 5.1. Never launches Elden Ring, me3 or Steam: the launcher is only run with
  -CheckOnly. Writes only to gitignored folders (mod\target, converted, work, build) and stages the DLL to
  build\natives. Exit code 0 only if every check PASSed; a SKIP counts as a failure unless -AllowSkip.

  Checks (docs\M2_DESIGN.md section 21):
   1 toolchain          2 vendor pin + patches    3 WitchyBND Paramdex bump   4 converter self-test
   5 weapons.json       6 sounds                  7 cargo build (no warnings) 8 cargo test (core)
   9 real-data x-check 10 PE shape               11 stage DLL                12 regulation build
  13 regulation self-test 14 ids consistency      15 me3 profile              16 launch preflight
  17 config            18 .gitignore             19 game folder untouched    20 log analyzer (extra)
  21 boot order (version gate first; static check + self-test)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File D:\Mods\MW2inER\tools\scripts\check_m2.ps1
#>
[CmdletBinding()]
param([switch]$AllowSkip)

$env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$Scripts = $PSScriptRoot
$Py = Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'
$Mod = Join-Path $Root 'mod'
$Vendor = Join-Path $Root 'vendor\fromsoftware-rs'
$VendorRev = '59fbd3b3b7daaf14aca47c9f73530493dba6bc79'
$Patches = @(
    (Join-Path $Root 'tools\patches\fromsoftware-rs-0001-bullet-spawn-data.patch'),
    (Join-Path $Root 'tools\patches\fromsoftware-rs-0002-recurring-task-no-panic.patch'),
    # M7 (docs/M7_DESIGN.md 2.1.2): BulletSpawnData.unk44 bit 0 = auto-finish (the emitter leak behind code 4)
    (Join-Path $Root 'tools\patches\fromsoftware-rs-0003-bullet-spawn-autofinish.patch'),
    # port 2.7.2.0 (MODLOG 2026-10-20): the Ww2720 RVA table (a new file) + its detection arm in rva.rs
    (Join-Path $Root 'tools\patches\fromsoftware-rs-0004-ww2720-table.patch')
)
$PatchFiles = @('crates/eldenring/src/cs/bullet_manager.rs', 'crates/eldenring/src/cs/field_ins.rs', 'crates/shared/src/task.rs', 'crates/eldenring/src/rva.rs')
# files the patches create (untracked in the vendor checkout; replayed from nothing)
$PatchNewFiles = @('crates/eldenring/src/rva/rva_ww2720.rs')
$BuiltDll = Join-Path $Mod 'target\release\mw2er.dll'
$Natives = Join-Path $Root 'build\natives'
$LogDir = Join-Path $Root 'work\check_m2'
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

$results = New-Object System.Collections.ArrayList

function Add-Result([string]$Id, [string]$Name, [string]$Status, [string]$Detail) {
    [void]$results.Add([pscustomobject]@{ Id = $Id; Name = $Name; Status = $Status; Detail = $Detail })
    Write-Output ('{0,-4} {1,2} {2,-22} {3}' -f $Status, $Id, $Name, $Detail)
}

# Runs a native tool; returns @{ Code; Out } with stdout+stderr as text. Output is also saved to work\check_m2.
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

Write-Output "M2 offline oracles ($Root)"
Write-Output "Never launches Elden Ring, me3 or Steam. Logs: work\check_m2\"
Write-Output ''

# 1. toolchain
Run-Check 1 'toolchain' {
    $c = Invoke-Tool 'cargo' @('--version') ''
    $r = Invoke-Tool 'rustc' @('--version') ''
    $pyOk = Test-Path -LiteralPath $Py
    $p = if ($pyOk) { Invoke-Tool $Py @('-I', '--version') '' } else { @{ Code = 1; Out = 'missing' } }
    if ($c.Out -match 'cargo 1\.99' -and $r.Out -match 'rustc 1\.99' -and $p.Out -match 'Python 3\.14') {
        Add-Result 1 'toolchain' 'PASS' ("{0}; {1}; {2}" -f $c.Out.Trim(), $r.Out.Trim(), $p.Out.Trim())
    } else { Add-Result 1 'toolchain' 'FAIL' "cargo='$($c.Out.Trim())' rustc='$($r.Out.Trim())' python='$($p.Out.Trim())' ($Py)" }
}

# 2. vendor pin + patches 0001..0004 (exactly these files changed (0004 also creates rva/rva_ww2720.rs, the only untracked file), and the working tree IS the pinned commit with the
#    patches applied in order). M7: 0003 edits lines next to 0001's hunks, so the working-tree diff is no longer the
#    plain concatenation of the patch files; instead the pinned files (`git show HEAD:<file>`, byte-exact through
#    cmd redirection) are replayed with `git apply` 0001, 0002, 0003 in work\check_m2\vendor_replay and every patched
#    file must equal the working tree byte for byte (no extra hunk can hide in a patched file).
Run-Check 2 'vendor pin + patches' {
    $head = (Invoke-Tool 'git' @('-C', $Vendor, 'rev-parse', 'HEAD') '').Out.Trim()
    $diff = @((Invoke-Tool 'git' @('-C', $Vendor, 'diff', '--name-only') '').Out -split "`n" | Where-Object { $_.Trim() } | ForEach-Object { $_.Trim() } | Sort-Object)
    $untracked = @((Invoke-Tool 'git' @('-C', $Vendor, 'ls-files', '--others', '--exclude-standard') '').Out -split "`n" | Where-Object { $_.Trim() } | ForEach-Object { $_.Trim() } | Sort-Object)
    $sameNew = ($untracked.Count -eq $PatchNewFiles.Count) -and (@(Compare-Object $untracked @($PatchNewFiles | Sort-Object)).Count -eq 0)
    $want = @($PatchFiles | Sort-Object)
    $sameFiles = ($diff.Count -eq $want.Count) -and (@(Compare-Object $diff $want).Count -eq 0)
    $replay = Join-Path $LogDir 'vendor_replay'
    if (Test-Path -LiteralPath $replay) { Remove-Item -LiteralPath $replay -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $replay | Out-Null
    foreach ($f in $PatchFiles) {
        $dst = Join-Path $replay ($f -replace '/', '\')
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        cmd /c "git -C `"$Vendor`" show HEAD:$f > `"$dst`""
    }
    $applyCodes = @($Patches | ForEach-Object { (Invoke-Tool 'git' @('apply', '--whitespace=nowarn', $_) ('vendor_patch_replay_' + [IO.Path]::GetFileNameWithoutExtension($_)) $replay).Code })
    $mismatch = @(($PatchFiles + $PatchNewFiles) | Where-Object {
        $a = Join-Path $replay ($_ -replace '/', '\'); $b = Join-Path $Vendor ($_ -replace '/', '\')
        -not ((Test-Path -LiteralPath $a) -and (Get-FileHash -LiteralPath $a).Hash -eq (Get-FileHash -LiteralPath $b).Hash)
    })
    if ($head -eq $VendorRev -and $sameFiles -and $sameNew -and @($applyCodes | Where-Object { $_ -ne 0 }).Count -eq 0 -and $mismatch.Count -eq 0) {
        Add-Result 2 'vendor pin + patches' 'PASS' "HEAD $($head.Substring(0, 7)); diff = $($diff -join ', '); HEAD + patches 0001+0002+0003+0004 replayed == the working tree byte for byte (new: $($PatchNewFiles -join ', '))"
    } else { Add-Result 2 'vendor pin + patches' 'FAIL' "HEAD=$head diff=[$($diff -join ', ')] untracked=[$($untracked -join ', ')] replay apply codes=[$($applyCodes -join ',')] mismatching files=[$($mismatch -join ', ')]" }
}

# 3. WitchyBND Paramdex upgrader bump (ER 1.17.1: 1_17_1_1000L; port 2.7.2.0: regulation 11730034 = 1_17_3_0034L)
Run-Check 3 'WitchyBND Paramdex' {
    $vt = Join-Path $Root 'tools\WitchyBND\app\Assets\Paramdex\ER\Upgrader\version.txt'
    $v = if (Test-Path -LiteralPath $vt) { (Get-Content -LiteralPath $vt -Raw).Trim() } else { '<missing>' }
    if ($v -eq '1_17_3_0034L') { Add-Result 3 'WitchyBND Paramdex' 'PASS' "version.txt = $v" }
    else { Add-Result 3 'WitchyBND Paramdex' 'FAIL' "version.txt = $v (want 1_17_3_0034L)" }
}

# 4. converter self-test
Run-Check 4 'converter self-test' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_weapons.py'), '--self-test') 'convert_selftest'
    Add-Result 4 'converter self-test' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out)
}

# 5. weapons.json: conversion + MODLOG known answers + schema
Run-Check 5 'weapons.json' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'convert_weapons.py'), '--verify') 'convert_weapons'
    $ump = Last-Line $r.Out '^ump45_mp:'
    Add-Result 5 'weapons.json' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f (Last-Line $r.Out '(PASS|FAIL)'), $ump)
}

# 6. sounds
Run-Check 6 'sounds' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'copy_sounds.py')) 'copy_sounds'
    Add-Result 6 'sounds' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 7. cargo build --release, no warnings from our crates
Run-Check 7 'cargo build' {
    $r = Invoke-Tool 'cargo' @('build', '--release', '--message-format', 'short') 'cargo_build' $Mod
    $ours = @($r.Out -split "`n" | Where-Object { $_ -match 'warning' -and ($_ -match '^\s*(mw2er|mw2er-core)[\\/]' -or $_ -match '`mw2er(-core)?` \(' -or $_ -match '\\mod\\') })
    $errors = @($r.Out -split "`n" | Where-Object { $_ -match '(^|\s)error(\[|:)' })
    if ($r.Code -eq 0 -and $ours.Count -eq 0 -and (Test-Path -LiteralPath $BuiltDll)) {
        $size = (Get-Item -LiteralPath $BuiltDll).Length
        Add-Result 7 'cargo build' 'PASS' "mod\target\release\mw2er.dll $size bytes; 0 warnings in mod\"
    } else { Add-Result 7 'cargo build' 'FAIL' "exit $($r.Code); $($ours.Count) warning line(s) in mod\; $($errors.Count) error line(s): $((@($ours) + @($errors) | Select-Object -First 2) -join ' | ')" }
}

# 8. cargo test -p mw2er-core (every category covered)
Run-Check 8 'cargo test' {
    $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core') 'cargo_test' $Mod
    $okLines = @($r.Out -split "`n" | Where-Object { $_ -match '^test (\S+) \.\.\. ok' } | ForEach-Object { ($_ -split ' ')[1] })
    $cats = [ordered]@{
        'weapons.json parse' = '^weapons::'; 'weaponfile parse' = '^weaponfile::'; 'cadence' = '^weapon_sim::tests::cadence_'
        'ammo/reload FSM' = '^weapon_sim::tests::ammo_'; 'falloff' = '^falloff::'; 'spread/crosshair' = '^(spread|hud_geom)::'
        'blank latch' = '^input_logic::tests::latch_'; 'menu logic' = '^menu_logic::'; 'hit tracker' = '^hit_logic::'; 'log format' = '^logfmt::'
        'version gate' = '^gate::'; 'shot sanity' = '^aim::tests::aim_shot_sanity'; 'blank heartbeat' = '^input_logic::tests::latch_heartbeat'
        'font check' = '^font_check::'; 'config bounds' = '^config::tests::config_rejects_non_finite'
    }
    $missing = @($cats.Keys | Where-Object { $pat = $cats[$_]; -not ($okLines | Where-Object { $_ -match $pat }) })
    $summary = @($r.Out -split "`n" | Where-Object { $_ -match '^test result:' } | ForEach-Object { $_.Trim() })
    $failedTests = @($r.Out -split "`n" | Where-Object { $_ -cmatch '^test \S+ \.\.\. FAILED' })
    if ($r.Code -eq 0 -and $missing.Count -eq 0 -and $failedTests.Count -eq 0) {
        Add-Result 8 'cargo test' 'PASS' "$($okLines.Count) tests ok across $($cats.Count) categories ($($summary -join '; '))"
    } else { Add-Result 8 'cargo test' 'FAIL' "exit $($r.Code); missing categories: $($missing -join ', '); failed: $($failedTests -join ' | ')" }
}

# 10. PE shape of the built DLL
Run-Check 10 'PE shape' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'pe', $BuiltDll) 'pe_shape'
    Add-Result 10 'PE shape' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 11. stage the DLL (+ pdb) to build\natives (me3 loads the staged copy; cargo stays free to rebuild)
Run-Check 11 'stage DLL' {
    New-Item -ItemType Directory -Force -Path $Natives | Out-Null
    $staged = @()
    try {
        foreach ($ext in @('dll', 'pdb')) {
            $src = Join-Path $Mod "target\release\mw2er.$ext"
            $dst = Join-Path $Natives "mw2er.$ext"
            Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop
            $a = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
            $b = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
            if ($a -ne $b) { throw "hash mismatch for mw2er.$ext" }
            $staged += "mw2er.$ext"
        }
        Add-Result 11 'stage DLL' 'PASS' ("build\natives\{0} (sha256 {1})" -f ($staged -join ', '), (Get-FileHash -LiteralPath (Join-Path $Natives 'mw2er.dll') -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant())
    } catch {
        Add-Result 11 'stage DLL' 'FAIL' "copy failed: $_ (is the game running with the staged DLL?)"
    }
}

# 12. regulation build + round trip + field-level check
Run-Check 12 'regulation' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py')) 'build_regulation'
    Add-Result 12 'regulation' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) ("{0} | {1}" -f (Last-Line $r.Out '^REGULATION'), (Last-Line $r.Out '^round trip'))
}

# 13. regulation self-test: the round-trip checks catch 7 deliberate mutations (Bullet rows, an untouched param,
#     the BND manifest, the XML read-back)
Run-Check 13 'regulation self-test' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'build_regulation.py'), '--self-test') 'build_regulation_selftest'
    Add-Result 13 'regulation self-test' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '^REGULATION')
}

# 9. real-data cross-check (Rust WEAPONFILE parser vs weapons.json; Rust band rule vs rows.json)
Run-Check 9 'real-data x-check' {
    $env:MW2ER_ROOT = $Root
    try { $r = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'real_data', '--', '--include-ignored', '--nocapture', '--test-threads=1') 'cargo_test_real_data' $Mod }
    finally { Remove-Item Env:\MW2ER_ROOT -ErrorAction SilentlyContinue }
    # M8 review: the libtest status and a test's own PASS line can share a line ('test x ... PASS: ..'), so the PASS
    # lines are matched anywhere (the run is single-threaded as well)
    $passes = @($r.Out -split "`n" | Where-Object { $_ -match 'PASS:' } | ForEach-Object { $_.Substring($_.IndexOf('PASS:')).Trim() })
    $skips = @($r.Out -split "`n" | Where-Object { $_ -match 'SKIP:' } | ForEach-Object { $_.Substring($_.IndexOf('SKIP:')).Trim() })
    if ($r.Code -ne 0) { Add-Result 9 'real-data x-check' 'FAIL' "exit $($r.Code); see work\check_m2\cargo_test_real_data.log" }
    elseif ($skips.Count -gt 0) { Add-Result 9 'real-data x-check' 'SKIP' ($skips -join ' | ') }
    elseif ($passes.Count -eq 2) { Add-Result 9 'real-data x-check' 'PASS' ($passes -join ' | ') }
    else { Add-Result 9 'real-data x-check' 'FAIL' "expected 2 PASS lines, got $($passes.Count)" }
}

# 14. ids consistency (rows.json vs mw2er.toml vs weapons.json vs package hash)
Run-Check 14 'ids consistency' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'ids') 'ids'
    Add-Result 14 'ids consistency' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 15. me3 profile shape
Run-Check 15 'me3 profile' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'profile', (Join-Path $Root 'profiles\mw2er.me3')) 'profile'
    Add-Result 15 'me3 profile' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 16. launch preflight (-CheckOnly only) + both scripts parse + launcher uses the full me3 path and -g
Run-Check 16 'launch preflight' {
    $launch = Join-Path $Root 'launch.ps1'
    $parseErrors = 0
    foreach ($f in @($launch, (Join-Path $Scripts 'check_m2.ps1'))) {
        $tokens = $null; $errs = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$tokens, [ref]$errs)
        $parseErrors += @($errs).Count
    }
    $text = Get-Content -LiteralPath $launch -Raw
    $hasMe3 = $text.Contains('garyttierney\me3\bin\me3.exe') -and $text.Contains('-g eldenring -p')
    $out = & $launch -CheckOnly 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    Set-Content -LiteralPath (Join-Path $LogDir 'launch_checkonly.log') -Value ($out -join "`n") -Encoding UTF8
    $ok = @($out | Where-Object { $_ -match '^OK' }).Count
    $warn = @($out | Where-Object { $_ -match '^WARN' })
    if ($code -eq 0 -and $parseErrors -eq 0 -and $hasMe3) {
        Add-Result 16 'launch preflight' 'PASS' "launch.ps1 -CheckOnly: $ok OK, $($warn.Count) WARN; 0 parse errors; full me3 path + -g eldenring -p"
    } else { Add-Result 16 'launch preflight' 'FAIL' "exit $code; parse errors $parseErrors; me3 path/-g present=$hasMe3; $(@($out | Where-Object { $_ -match '^FAIL' }) -join ' | ')" }
}

# 17. config: tomllib + the DLL's own parser (cargo test --test config_file reads the real file)
Run-Check 17 'config' {
    $t = Invoke-Tool $Py @('-I', '-c', "import tomllib,sys; d=tomllib.load(open(sys.argv[1],'rb')); print('tomllib ok k=%s weapon=%s' % (d['damage']['k'], d['gameplay']['weapon']))", (Join-Path $Root 'mw2er.toml')) 'config_tomllib'
    $c = Invoke-Tool 'cargo' @('test', '-p', 'mw2er-core', '--test', 'config_file', '--', '--nocapture') 'config_rust' $Mod
    if ($t.Code -eq 0 -and $c.Code -eq 0 -and $c.Out -match 'PASS:') {
        Add-Result 17 'config' 'PASS' ("{0}; {1}" -f $t.Out.Trim(), (Last-Line $c.Out '^PASS:'))
    } else { Add-Result 17 'config' 'FAIL' "tomllib exit $($t.Code) ($($t.Out.Trim())); rust parser exit $($c.Code)" }
}

# 18. .gitignore
Run-Check 18 '.gitignore' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gitignore') 'gitignore'
    Add-Result 18 '.gitignore' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 19. game folder untouched
Run-Check 19 'game folder untouched' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'gamefolder') 'gamefolder'
    Add-Result 19 'game folder untouched' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 20. (extra) the in-game log analyzer itself works on synthetic logs (good log passes, PANIC fails, a HIT with
#     the wrong damage (dmg=1, x1.2, 30% off) fails, expectations come from weapons.json / rows.json)
Run-Check 20 'log analyzer' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'analyze_log.py'), '--self-test') 'analyze_log_selftest'
    Add-Result 20 'log analyzer' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (Last-Line $r.Out '(PASS|FAIL)')
}

# 21. boot order: the version gate runs before anything touches the game (static check that can fail: self-test)
Run-Check 21 'boot order' {
    $r = Invoke-Tool $Py @('-I', (Join-Path $Scripts 'm2_static_checks.py'), 'bootorder', '--self-test') 'bootorder'
    Add-Result 21 'boot order' $(if ($r.Code -eq 0) { 'PASS' } else { 'FAIL' }) (((@($r.Out -split "`n" | Where-Object { $_ -match '^(self-test|PASS|FAIL)' }) | ForEach-Object { $_.Trim() }) -join ' | '))
}

$pass = @($results | Where-Object { $_.Status -eq 'PASS' }).Count
$fail = @($results | Where-Object { $_.Status -eq 'FAIL' }).Count
$skip = @($results | Where-Object { $_.Status -eq 'SKIP' }).Count
Write-Output ''
Write-Output "SUMMARY: $pass PASS, $fail FAIL, $skip SKIP of $($results.Count) checks"
Write-Output 'Not covered offline (needs the first in-game run): struct layouts at runtime, spawn_bullet with zeroed unknown fields, bullet collision at 999 m/s, camera direction/FOV units, menu indices and the show_cursor signal, the player handle reading as type Chr, last_hit_by attribution, the damage k, hudhook swapchain capture under me3, audio output.'
if ($fail -eq 0 -and ($skip -eq 0 -or $AllowSkip)) {
    Write-Output 'M2 OFFLINE ORACLES: PASS'
    exit 0
}
Write-Output 'M2 OFFLINE ORACLES: FAIL'
exit 1
