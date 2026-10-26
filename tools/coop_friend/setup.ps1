<#
.SYNOPSIS
  MW2 in Elden Ring - co-op friend setup. Builds YOUR copy of the mod's data from YOUR own games, checks that it is
  byte-identical to the host's, and writes the me3 profiles. Never starts a game, me3, Steam or Seamless.

.DESCRIPTION
  Everything is written inside this package folder (and the save backup folder; with -Interactive also the two
  Seamless ini keys after you agree, and the desktop shortcuts you ask for). The game folders and %APPDATA%\EldenRing
  are only READ (the saves are copied to the backup folder first).

  Steps: 1 prerequisites  2 save backup  3 WitchyBND  4 MW2 dumps (Unlinker)  5 converters  6 regulation
         7 verify every hash against the host's manifest  8 me3 profiles (co-op + solo) + the launch command.

  The friend just double-clicks Install.cmd, which runs this script with -Interactive: anything not found
  automatically is asked for with a dialog (file pickers; Yes/No boxes), WitchyBND can be downloaded from its official
  GitHub release (kept only if its sha256 equals the pin), and the desktop shortcuts are offered at the end.
  Without -Interactive the script behaves as before (command line, no questions).

.PARAMETER SeamlessDll   path of YOUR Seamless Co-op ersc.dll (2.0.1, unzipped anywhere outside the game folder).
.PARAMETER WitchyZip     path of WitchyBND-3.0.1.0-win-x64.zip downloaded from its GitHub release.
.PARAMETER ErGame        Elden Ring "Game" folder (auto-detected from Steam).
.PARAMETER Mw2Dir        MW2 (2009) install folder (auto-detected from Steam).
.PARAMETER Python        python.exe 3.11+ (3.14 recommended; auto-detected).
.PARAMETER Me3           me3.exe (auto-detected).
.PARAMETER BackupDir     where the save backup goes (default: <package>\backups).
.PARAMETER SkipSeamlessIni  do not look at ersc_settings.ini (the host's self-test uses this; never needed by a friend).
.PARAMETER KeepDumps     keep the raw MW2 dumps (dumps\, ~2 GB) after a successful run.
.PARAMETER Interactive   ask with dialogs for whatever is not found (Install.cmd passes this).
.PARAMETER NoGui         with -Interactive: typed answers in the console instead of dialogs.
.PARAMETER DownloadWitchy  download the pinned WitchyBND zip when it is not found (-Interactive asks first instead).
.PARAMETER DryRunDownload  print the WitchyBND download instead of doing it (tests).
.PARAMETER WitchyUrl     tests only: another source for the WitchyBND zip; the sha256 pin applies to any source.
.PARAMETER CheckOnly     run step 1 (prerequisites and inputs) only, then stop; builds nothing.
.PARAMETER Shortcuts     create the desktop shortcuts at the end without asking.
.PARAMETER ShortcutsOnly only (re)create the desktop shortcuts of a package that setup already finished.
.PARAMETER ShortcutDir   where the shortcuts go (default: your Desktop).

.EXAMPLE
  Double-click Install.cmd   (= powershell -NoProfile -ExecutionPolicy Bypass -STA -File .\setup.ps1 -Interactive)
.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File .\setup.ps1 -SeamlessDll "D:\Games\SeamlessCoop\ersc.dll" -WitchyZip "$env:USERPROFILE\Downloads\WitchyBND-3.0.1.0-win-x64.zip"
#>
[CmdletBinding()]
param(
    [string]$SeamlessDll = '',
    [string]$WitchyZip = '',
    [string]$ErGame = '',
    [string]$Mw2Dir = '',
    [string]$Python = '',
    [string]$Me3 = '',
    [string]$BackupDir = '',
    [switch]$SkipSeamlessIni,
    [switch]$KeepDumps,
    [switch]$Interactive,
    [switch]$NoGui,
    [switch]$DownloadWitchy,
    [switch]$DryRunDownload,
    [string]$WitchyUrl = 'https://github.com/ividyon/WitchyBND/releases/download/3.0.1.0/WitchyBND-3.0.1.0-win-x64.zip',
    [switch]$CheckOnly,
    [switch]$Shortcuts,
    [switch]$ShortcutsOnly,
    [string]$ShortcutDir = ''
)

$ErrorActionPreference = 'Stop'
$Root = $PSScriptRoot
$Pins = Get-Content -LiteralPath (Join-Path $Root 'manifest\host_pins.json') -Raw | ConvertFrom-Json
$Manifest = Join-Path $Root 'manifest\host_manifest.json'
$Scripts = Join-Path $Root 'tools\scripts'
$LogDir = Join-Path $Root 'setup_logs'
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$script:fails = 0
$script:warns = 0
# Seamless Co-op on Nexus Mods: mod id 510 [V: the Nexus download's file name "Seamless Co-op v2.0.1 510 2.0.1 ..."]
$NexusSeamless = 'https://www.nexusmods.com/eldenring/mods/510?tab=files'
$Me3Releases = 'https://github.com/garyttierney/me3/releases/tag/v0.13.0'
$PythonDownloads = 'https://www.python.org/downloads/windows/'
$VcRedist = 'https://aka.ms/vs/17/release/vc_redist.x64.exe'
$WitchyName = 'WitchyBND-3.0.1.0-win-x64.zip'
$InputsFile = Join-Path $Root 'setup_inputs.json'   # paths chosen last time (never the password); read only with -Interactive

function Say([string]$Status, [string]$Message) {
    $c = @{ 'OK' = 'Green'; 'WARN' = 'Yellow'; 'FAIL' = 'Red'; 'STEP' = 'Cyan'; 'INFO' = 'Gray' }[$Status]
    Write-Host ('{0,-4} {1}' -f $Status, $Message) -ForegroundColor $c
    # the log is best effort (another program may hold it open); the console line above is what counts
    try { Add-Content -LiteralPath (Join-Path $LogDir 'setup.log') -Value ('{0} {1,-4} {2}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Status, $Message) -Encoding UTF8 -ErrorAction Stop } catch { }
    if ($Status -eq 'FAIL') { $script:fails++ }
    if ($Status -eq 'WARN') { $script:warns++ }
}
function Stop-Setup([string]$Why) {
    Say 'FAIL' $Why
    Write-Host ''
    Write-Host "SETUP FAILED ($script:fails problem(s)). Nothing was written outside $Root. See INSTALL_GUIDE (troubleshooting) and setup_logs\setup.log." -ForegroundColor Red
    if ($Interactive) { Write-Host 'Fix the problem above, then double-click Install.cmd again (finished steps are simply redone).' -ForegroundColor Red }
    exit 1
}
function Sha([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Full([string]$p) { [IO.Path]::GetFullPath($p).TrimEnd('\') }
function Under([string]$Child, [string]$Parent) {
    if (-not $Child -or -not $Parent) { return $false }
    $c = (Full $Child) + '\'; $p = (Full $Parent) + '\'
    return $c.StartsWith($p, [StringComparison]::OrdinalIgnoreCase)
}

. (Join-Path $Root 'setup_ui.ps1')
$uiMode = Initialize-SetupUi ([bool]$Interactive) ([bool]$NoGui)

# the desktop shortcuts: "Play co-op.cmd" (+ "Play solo.cmd") with the working directory = this folder
function New-PlayShortcuts([string]$Dir, [string]$IconExe) {
    if (-not $Dir) { $Dir = [Environment]::GetFolderPath('Desktop') }
    $icon = if ($IconExe -and (Test-Path -LiteralPath $IconExe)) { "$IconExe,0" } else { '' }
    $made = @()
    $made += New-SetupShortcut $Dir 'MW2 in Elden Ring (co-op)' (Join-Path $Root 'Play co-op.cmd') $Root $icon 'MW2 in Elden Ring with Seamless Co-op (me3, --no-mem-patch). Start Steam first.'
    if ((Test-Path -LiteralPath (Join-Path $Root 'Play solo.cmd')) -and (Test-Path -LiteralPath (Join-Path $Root 'profiles\mw2er-solo.me3'))) {
        $made += New-SetupShortcut $Dir 'MW2 in Elden Ring (solo)' (Join-Path $Root 'Play solo.cmd') $Root $icon 'MW2 in Elden Ring alone (me3, own save ER0000.mw2er.sl2). Start Steam first.'
    }
    return $made
}

if ($ShortcutsOnly) {
    if (-not (Test-Path -LiteralPath (Join-Path $Root 'profiles\mw2er-coop.me3'))) { Stop-Setup 'profiles\mw2er-coop.me3 missing: run setup (Install.cmd) first' }
    $erExe = ''
    if (Test-Path -LiteralPath $InputsFile) { try { $erExe = Join-Path ((Get-Content -LiteralPath $InputsFile -Raw | ConvertFrom-Json).ErGame) 'eldenring.exe' } catch { } }
    foreach ($l in (New-PlayShortcuts $ShortcutDir $erExe)) { Say 'OK' "shortcut $l" }
    exit 0
}

Write-Host "MW2 in Elden Ring - co-op friend setup ($Root)" -ForegroundColor Cyan
Write-Host 'Never starts a game, me3, Steam or Seamless. Writes only inside this folder (+ the save backup folder).'
if ($Interactive) {
    Write-Host "Double-click mode (questions: $uiMode). Setup finds your games and tools by itself and only asks for what it cannot find."
    Write-Host 'It takes about 10-25 minutes and needs about 6 GB free while it runs. Your Elden Ring saves are backed up first.'
}
Write-Host ''

$Remembered = $null
if ($Interactive -and (Test-Path -LiteralPath $InputsFile)) { try { $Remembered = Get-Content -LiteralPath $InputsFile -Raw | ConvertFrom-Json } catch { $Remembered = $null } }
function Remembered([string]$Name) { if ($Remembered -and $Remembered.$Name) { return [string]$Remembered.$Name } else { return '' } }

# ---------------------------------------------------------------------------------------------------------------
Say 'STEP' '1/8 prerequisites'

# Steam libraries (read-only)
$libs = New-Object System.Collections.ArrayList
try {
    $steam = (Get-ItemProperty -Path 'HKCU:\Software\Valve\Steam' -Name SteamPath -ErrorAction Stop).SteamPath
    if ($steam) {
        [void]$libs.Add((Full ($steam -replace '/', '\')))
        $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
        if (Test-Path -LiteralPath $vdf) {
            foreach ($m in [regex]::Matches((Get-Content -LiteralPath $vdf -Raw), '"path"\s+"([^"]+)"')) {
                [void]$libs.Add((Full ($m.Groups[1].Value -replace '\\\\', '\')))
            }
        }
    }
} catch { Say 'WARN' 'Steam not found in the registry; pass -ErGame and -Mw2Dir' }
$libs = @($libs | Select-Object -Unique)

function Find-App([string]$Rel, [string]$AppId) {
    foreach ($l in $libs) {
        $p = Join-Path $l "steamapps\common\$Rel"
        if (Test-Path -LiteralPath $p) { return @{ dir = $p; acf = (Join-Path $l "steamapps\appmanifest_$AppId.acf") } }
    }
    return $null
}
function BuildId([string]$Acf) {
    if (-not (Test-Path -LiteralPath $Acf)) { return $null }
    $m = [regex]::Match((Get-Content -LiteralPath $Acf -Raw), '"buildid"\s+"(\d+)"')
    if ($m.Success) { return $m.Groups[1].Value } else { return $null }
}
# -Interactive: a file picker for a folder that was not found (the folder of the chosen file is returned)
function Pick-Folder([string]$What, [string]$FileName, [string]$Filter, [string]$Need) {
    for ($i = 0; $i -lt 3; $i++) {
        $a = Ui-Ask "$What not found" "$What was not found through Steam.`n`nClick OK and choose its $FileName (usually in <Steam library>\steamapps\common\...).`nCancel stops setup." 'OKCancel' 'Warning'
        if ($a -ne 'OK') { return '' }
        $f = Ui-PickFile "Choose $FileName ($What)" $Filter ''
        if (-not $f) { continue }
        $d = Split-Path -Parent $f
        if (Test-Path -LiteralPath (Join-Path $d $Need)) { return $d }
        [void](Ui-Ask $What "That folder has no $Need. Choose the $FileName of the Steam install." 'OK' 'Warning')
    }
    return ''
}

# Elden Ring
$erAcf = $null
if (-not $ErGame) { $f = Find-App 'ELDEN RING\Game' '1245620'; if ($f) { $ErGame = $f.dir; $erAcf = $f.acf } }
if ($Interactive -and (-not $ErGame -or -not (Test-Path -LiteralPath (Join-Path $ErGame 'eldenring.exe')))) {
    $r = Remembered 'ErGame'
    if ($r -and (Test-Path -LiteralPath (Join-Path $r 'eldenring.exe'))) { $ErGame = $r } else { $ErGame = Pick-Folder 'Elden Ring' 'eldenring.exe' 'eldenring.exe|eldenring.exe' 'eldenring.exe' }
}
if (-not $ErGame -or -not (Test-Path -LiteralPath (Join-Path $ErGame 'eldenring.exe'))) { Stop-Setup 'Elden Ring not found: pass -ErGame "<...>\ELDEN RING\Game"' }
$ErGame = Full $ErGame
$erVer = (Get-Item -LiteralPath (Join-Path $ErGame 'eldenring.exe')).VersionInfo.FileVersion
if ($erVer -eq $Pins.er_exe_version) { Say 'OK' "Elden Ring: $ErGame (eldenring.exe $erVer)" }
else { Stop-Setup "eldenring.exe is $erVer, the mod needs $($Pins.er_exe_version) (Elden Ring $($Pins.er_app_version)). Update the game in Steam (both players need the same version)." }
$erReg = Join-Path $ErGame 'regulation.bin'
if (-not (Test-Path -LiteralPath $erReg)) { Stop-Setup "missing $erReg (verify the game files in Steam)" }
$regSha = Sha $erReg
if ($regSha -eq $Pins.vanilla_regulation_sha256) { Say 'OK' "vanilla regulation.bin sha256 $($regSha.Substring(0,12)) == host's" }
else { Stop-Setup "your Elden Ring regulation.bin (sha256 $($regSha.Substring(0,12))) is not the vanilla file the host has ($($Pins.vanilla_regulation_sha256.Substring(0,12))). Steam > Elden Ring > Properties > Installed Files > Verify integrity, then run setup again." }
if (Test-Path -LiteralPath (Join-Path $ErGame 'SeamlessCoop\ersc.dll')) { Say 'WARN' 'a Seamless ersc.dll sits inside the game folder; this setup never uses it (use a copy outside the game folder, see INSTALL_GUIDE step 3)' }

# MW2
$mwAcf = $null
if (-not $Mw2Dir) { $f = Find-App 'Call of Duty Modern Warfare 2' '10180'; if ($f) { $Mw2Dir = $f.dir; $mwAcf = $f.acf } }
if ($Interactive -and (-not $Mw2Dir -or -not (Test-Path -LiteralPath (Join-Path $Mw2Dir 'zone\english\common_mp.ff')))) {
    $r = Remembered 'Mw2Dir'
    if ($r -and (Test-Path -LiteralPath (Join-Path $r 'zone\english\common_mp.ff'))) { $Mw2Dir = $r } else { $Mw2Dir = Pick-Folder 'Call of Duty: Modern Warfare 2 (2009)' 'iw4sp.exe' 'MW2 (iw4sp.exe, iw4mp.exe)|iw4sp.exe;iw4mp.exe' 'zone\english\common_mp.ff' }
}
if (-not $Mw2Dir -or -not (Test-Path -LiteralPath (Join-Path $Mw2Dir 'zone\english\common_mp.ff'))) { Stop-Setup 'MW2 (2009) not found: pass -Mw2Dir "<...>\Call of Duty Modern Warfare 2" (it needs zone\english\common_mp.ff)' }
$Mw2Dir = Full $Mw2Dir
if (-not $mwAcf) { $mwAcf = Join-Path (Split-Path -Parent (Split-Path -Parent $Mw2Dir)) 'appmanifest_10180.acf' }
$bid =if ($mwAcf) { BuildId $mwAcf } else { $null }
$exe = Join-Path $Mw2Dir 'iw4sp.exe'
$exeSha = if (Test-Path -LiteralPath $exe) { Sha $exe } else { '' }
if ($bid -eq $Pins.mw2_buildid -and $exeSha -eq $Pins.iw4sp_sha256) { Say 'OK' "MW2: $Mw2Dir (Steam build $bid, iw4sp.exe == host's)" }
else { Say 'WARN' "MW2 build differs from the host's (yours: build $bid, iw4sp.exe $($exeSha.Substring(0, [Math]::Min(12, $exeSha.Length))); host: build $($Pins.mw2_buildid), $($Pins.iw4sp_sha256.Substring(0,12))). Setup continues; step 7 decides (every converted file must still match)." }

# the package must not sit in a game folder (checked before anything below writes into it)
foreach ($g in @($ErGame, (Split-Path -Parent $ErGame), $Mw2Dir)) { if (Under $Root $g) { Stop-Setup "this package is inside a game folder ($g): unzip it somewhere else, e.g. D:\Mods\MW2inER-coop" } }

# me3
function Find-Me3 {
    foreach ($c in @((Join-Path $env:LOCALAPPDATA 'Programs\garyttierney\me3\bin\me3.exe'))) { if (Test-Path -LiteralPath $c) { return $c } }
    $cmd = Get-Command me3.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return ''
}
if (-not $Me3) { $Me3 = Find-Me3 }
if ($Interactive -and (-not $Me3 -or -not (Test-Path -LiteralPath $Me3))) {
    $r = Remembered 'Me3'
    if ($r -and (Test-Path -LiteralPath $r)) { $Me3 = $r }
    for ($i = 0; $i -lt 3 -and (-not $Me3 -or -not (Test-Path -LiteralPath $Me3)); $i++) {
        $a = Ui-Ask 'me3 0.13.0 needed' "me3 (the mod loader) was not found.`n`nYes: open the official me3 v0.13.0 download page. Download and run me3_installer.exe there, then come back and click OK.`nNo: I have me3 - choose me3.exe.`nCancel: stop setup." 'YesNoCancel' 'Warning'
        if ($a -eq 'Cancel') { break }
        if ($a -eq 'Yes') {
            Ui-OpenUrl $Me3Releases
            if ((Ui-Ask 'me3 0.13.0' 'Click OK when the me3 installer has finished.' 'OKCancel' 'Information') -ne 'OK') { break }
            $Me3 = Find-Me3
        } else { $Me3 = Ui-PickFile 'Choose me3.exe' 'me3.exe|me3.exe' (Join-Path $env:LOCALAPPDATA 'Programs') }
    }
}
if (-not $Me3 -or -not (Test-Path -LiteralPath $Me3)) { Stop-Setup 'me3 not found: install me3 0.13.0 (INSTALL_GUIDE step 1) or pass -Me3 "<...>\me3.exe"' }
$me3Ver = (Get-Item -LiteralPath $Me3).VersionInfo.ProductVersion
if ($me3Ver -eq $Pins.me3_version) { Say 'OK' "me3 ${me3Ver}: $Me3" } else { Say 'WARN' "me3 is $me3Ver (host: $($Pins.me3_version)); install me3 $($Pins.me3_version) if anything misbehaves" }

# Seamless
$SeamlessImport = Join-Path $Root 'seamless'   # the copy unpacked from your Seamless zip (-Interactive), never the game folder
if (-not $SeamlessDll -and $Interactive) {
    $dl = Get-DownloadsDir
    $auto = Find-SeamlessAuto $Pins.ersc_sha256 @((Remembered 'SeamlessDll'), (Join-Path $SeamlessImport 'SeamlessCoop\ersc.dll')) @($dl, ([Environment]::GetFolderPath('MyDocuments')), ([Environment]::GetFolderPath('Desktop')), (Split-Path -Parent $Root)) $dl $ErGame
    if ($auto.Dll) { $SeamlessDll = $auto.Dll; Say 'INFO' "Seamless Co-op $($Pins.seamless_version) found by itself: $SeamlessDll" }
    elseif ($auto.Zip) {
        $imp = Import-SeamlessZip $auto.Zip $SeamlessImport $Pins.ersc_sha256
        if ($imp.Ok) { $SeamlessDll = $imp.Dll; Say 'INFO' "Seamless Co-op $($Pins.seamless_version) unpacked from your download $($auto.Zip) into $SeamlessImport" }
    }
    for ($i = 0; $i -lt 4 -and -not $SeamlessDll; $i++) {
        $a = Ui-Ask 'Seamless Co-op 2.0.1 needed' ("Seamless Co-op $($Pins.seamless_version) (the co-op mod; both players need exactly the same version) was not found.`n`n" +
            "Yes: I have it - choose its ersc.dll or the zip I downloaded from Nexus Mods.`n" +
            "No: I don't have it - open its Nexus Mods page so I can download it.`nCancel: stop setup.") 'YesNoCancel' 'Question'
        if ($a -eq 'Cancel') { break }
        if ($a -eq 'No') {
            Ui-OpenUrl $NexusSeamless
            $b = Ui-Ask 'Download Seamless Co-op' ("On the page that opened (Nexus Mods, a free account is needed): Files > main file `"Seamless Co-op`" version $($Pins.seamless_version) > Manual download.`n" +
                "Leave the zip in your Downloads folder (do not unzip it into the game folder; setup unpacks it into this package).`n`nClick OK when the download has finished.") 'OKCancel' 'Information'
            if ($b -ne 'OK') { break }
            $auto = Find-SeamlessAuto $Pins.ersc_sha256 @() @() $dl $ErGame
            if ($auto.Zip) {
                $imp = Import-SeamlessZip $auto.Zip $SeamlessImport $Pins.ersc_sha256
                if ($imp.Ok) { $SeamlessDll = $imp.Dll; Say 'INFO' "Seamless Co-op unpacked from $($auto.Zip) into $SeamlessImport" } else { [void](Ui-Ask 'Seamless Co-op' $imp.Error 'OK' 'Warning') }
            } else { [void](Ui-Ask 'Seamless Co-op' "No Seamless Co-op $($Pins.seamless_version) zip was found in $dl. Choose it yourself next (Yes)." 'OK' 'Warning') }
            continue
        }
        $p = Ui-PickFile 'Choose Seamless Co-op ersc.dll (or the zip you downloaded)' 'Seamless Co-op (ersc.dll or its zip)|ersc.dll;*.zip|ersc.dll|ersc.dll|Zip files|*.zip' $dl
        if (-not $p) { continue }
        if ($p -match '(?i)\.zip$') {
            $imp = Import-SeamlessZip $p $SeamlessImport $Pins.ersc_sha256
            if ($imp.Ok) { $SeamlessDll = $imp.Dll; Say 'INFO' "Seamless Co-op unpacked from $p into $SeamlessImport" } else { [void](Ui-Ask 'Seamless Co-op' $imp.Error 'OK' 'Warning') }
            continue
        }
        if (Under $p (Split-Path -Parent $ErGame)) { [void](Ui-Ask 'Seamless Co-op' 'That ersc.dll is inside the Elden Ring folder. Choose the zip you downloaded instead (setup unpacks it into this package), or a copy in a folder of your own.' 'OK' 'Warning'); continue }
        $h = Sha $p
        if ($h -ne $Pins.ersc_sha256) { [void](Ui-Ask 'Seamless Co-op' ("That ersc.dll (sha256 {0}) is not Seamless Co-op {1} ({2}). Both players need the same version." -f $h.Substring(0, 12), $Pins.seamless_version, $Pins.ersc_sha256.Substring(0, 12)) 'OK' 'Warning'); continue }
        $SeamlessDll = $p
    }
}
if (-not $SeamlessDll) { Stop-Setup 'pass -SeamlessDll "<your Seamless folder>\ersc.dll" (Seamless Co-op 2.0.1, unzipped OUTSIDE the game folder)' }
if (-not (Test-Path -LiteralPath $SeamlessDll)) { Stop-Setup "Seamless not found: $SeamlessDll" }
$SeamlessDll = Full $SeamlessDll
if (Under $SeamlessDll $ErGame) { Stop-Setup 'your ersc.dll is inside the Elden Ring folder: unzip Seamless to a folder of your own (e.g. D:\Mods\SeamlessCoop) and pass that ersc.dll' }
$ersc = Sha $SeamlessDll
if ($ersc -eq $Pins.ersc_sha256) { Say 'OK' "Seamless ersc.dll == host's (sha256 $($ersc.Substring(0,12)), Seamless Co-op $($Pins.seamless_version))" }
else { Stop-Setup "your ersc.dll (sha256 $($ersc.Substring(0,12))) is not the host's Seamless Co-op $($Pins.seamless_version) ($($Pins.ersc_sha256.Substring(0,12))). Both players need the same version." }
if (-not $SkipSeamlessIni) {
    $ini = Join-Path (Split-Path -Parent $SeamlessDll) 'ersc_settings.ini'
    if (-not (Test-Path -LiteralPath $ini)) { Say 'FAIL' "missing $ini (it comes with Seamless; keep it next to ersc.dll)" }
    else {
        # only these three keys are looked at; the password VALUE is never printed or stored
        $st = Get-SeamlessIniState $ini
        if ($Interactive -and ($st.allow_invaders -ne '0' -or $st.save_file_extension -ne 'mw2co2')) {
            $ours = Under $ini $SeamlessImport
            $go = $ours
            if (-not $ours) {
                $go = (Ui-Ask 'Seamless settings' ("Your $ini needs two settings for MW2 co-op:`n  allow_invaders = 0   (no strangers invading)`n  save_file_extension = mw2co2   (your co-op character is kept apart from your normal save)`n`n" +
                    'Set them now? Nothing else in the file changes.') 'YesNo' 'Question') -eq 'Yes'
            }
            if ($go -and -not (Under $ini (Split-Path -Parent $ErGame)) -and -not (Under $ini $Mw2Dir)) {
                $n = Set-SeamlessIniKeys $ini
                Say 'INFO' "ersc_settings.ini: allow_invaders / save_file_extension set ($n of 2 keys found)"
                $st = Get-SeamlessIniState $ini
            }
        }
        for ($i = 0; $Interactive -and -not $st.password_set -and $i -lt 3; $i++) {
            if (-not (Edit-SeamlessPassword $ini)) { break }
            $st = Get-SeamlessIniState $ini
        }
        if ($st.allow_invaders -eq '0') { Say 'OK' 'ersc_settings.ini: allow_invaders = 0' } else { Say 'FAIL' 'ersc_settings.ini: set allow_invaders = 0 (no invaders in MW2 sessions)' }
        if ($st.save_file_extension -eq 'mw2co2') { Say 'OK' 'ersc_settings.ini: save_file_extension = mw2co2 (your MW2 co-op save is separate)' } else { Say 'FAIL' 'ersc_settings.ini: set save_file_extension = mw2co2 (keeps your normal saves apart)' }
        if ($st.password_set) { Say 'OK' 'ersc_settings.ini: cooppassword is set (value not shown)' } else { Say 'WARN' 'ersc_settings.ini: no non-empty cooppassword line found: set the password the host gave you (Seamless refuses to start a session without one)' }
        $st = $null
    }
}

# Python
function Py-Ok([string]$p) {
    if (-not $p -or -not (Test-Path -LiteralPath $p)) { return $false }
    if ($p -like '*\WindowsApps\*') { return $false }
    $ErrorActionPreference = 'Continue'
    # no double quotes in the argument: PowerShell 5.1 mangles them for native programs
    $v = & $p -I -c 'import sys; print(sys.version_info[0] * 100 + sys.version_info[1])' 2>$null
    return ($LASTEXITCODE -eq 0 -and $v -and [int]$v -ge 311)
}
function Find-Python {
    $cands = @((Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe'), (Join-Path $env:ProgramFiles 'Python314\python.exe'))
    $pyl = Get-Command py.exe -ErrorAction SilentlyContinue
    if ($pyl) { try { $e = & $pyl.Source -3 -c 'import sys; print(sys.executable)' 2>$null; if ($LASTEXITCODE -eq 0 -and $e) { $cands += $e } } catch { } }
    $pp = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($pp) { $cands += $pp.Source }
    foreach ($c in $cands) { if (Py-Ok $c) { return $c } }
    return ''
}
if (-not (Py-Ok $Python)) { $Python = Find-Python }
for ($i = 0; $Interactive -and -not $Python -and $i -lt 2; $i++) {
    $a = Ui-Ask 'Python needed' "Python 3.11 or newer was not found (3.14 recommended).`n`nOK: open python.org. Download the Windows installer (64-bit), tick `"Add python.exe to PATH`" on its first screen, click Install Now, then come back and click OK here.`nCancel: stop setup." 'OKCancel' 'Warning'
    if ($a -ne 'OK') { break }
    Ui-OpenUrl $PythonDownloads
    if ((Ui-Ask 'Python' 'Click OK when the Python installer has finished.' 'OKCancel' 'Information') -ne 'OK') { break }
    $Python = Find-Python
}
if (-not $Python) { Stop-Setup 'Python 3.11+ not found: install Python 3.14 from python.org (tick "Add python.exe to PATH") or pass -Python' }
$pyVer = & $Python -I -c 'import sys; print(sys.version.split()[0])'
Say 'OK' "Python ${pyVer}: $Python"

# Visual C++ runtime (the Unlinker and sndconv need it; Steam games normally install it)
$vc = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll') | Where-Object { -not (Test-Path -LiteralPath (Join-Path $env:WINDIR "System32\$_")) }
if ($vc -and $Interactive) {
    if ((Ui-Ask 'Visual C++ runtime needed' "The Microsoft Visual C++ 2015-2022 x64 runtime is missing.`n`nOK: download Microsoft's official installer ($VcRedist) in your browser; run it, then double-click Install.cmd again." 'OKCancel' 'Warning') -eq 'OK') { Ui-OpenUrl $VcRedist }
}
if ($vc) { Stop-Setup "Microsoft Visual C++ 2015-2022 x64 runtime missing ($($vc -join ', ')): install vc_redist.x64.exe from $VcRedist" } else { Say 'OK' 'Visual C++ x64 runtime present' }

# WitchyBND zip: next to this package, in Downloads, or (with -Interactive / -DownloadWitchy) the pinned official download
$witchyDl = Join-Path $Root "downloads\$WitchyName"
function Get-WitchyByDownload {
    Say 'INFO' "downloading $WitchyName (48 MB) from $WitchyUrl ..."
    $r = Get-PinnedDownload $WitchyUrl $witchyDl $Pins.witchybnd_zip_sha256 -DryRun:$DryRunDownload
    if ($r.Ok) { Say 'OK' "downloaded $witchyDl (sha256 $($r.Sha.Substring(0,12)) == the pinned 3.0.1.0 release)"; return $r.Path }
    Say 'WARN' "WitchyBND download: $($r.Error)"
    if ($Interactive -and $r.Error -ne 'dry run') { [void](Ui-Ask 'WitchyBND download' "The download did not work: $($r.Error)`n`nYou can choose the zip yourself next." 'OK' 'Warning') }
    return ''
}
function Get-WitchyAsk {
    for ($i = 0; $i -lt 3; $i++) {
        $a = Ui-Ask 'WitchyBND 3.0.1.0 needed' ("Setup needs WitchyBND 3.0.1.0 (a free tool that builds the game settings file). $WitchyName was not found next to this package or in your Downloads folder.`n`n" +
            "Yes: download it now from its official GitHub release (48 MB):`n$WitchyUrl`nThe file is checked against the pinned sha256 and deleted if it differs.`n`n" +
            "No: I have it - choose the zip myself (works offline).`nCancel: stop setup.") 'YesNoCancel' 'Question'
        if ($a -eq 'Cancel') { return '' }
        if ($a -eq 'Yes') { $p = Get-WitchyByDownload; if ($p) { return $p }; if ($DryRunDownload) { return '' }; continue }
        $p = Ui-PickFile "Choose $WitchyName" "WitchyBND zip|WitchyBND*.zip|Zip files|*.zip" (Get-DownloadsDir)
        if ($p) { return $p }
    }
    return ''
}
if (-not $WitchyZip) {
    $cands = @((Join-Path $Root $WitchyName), (Join-Path $env:USERPROFILE "Downloads\$WitchyName"))
    if ($Interactive -or $DownloadWitchy) { $cands += @($witchyDl, (Join-Path (Get-DownloadsDir) $WitchyName)) }
    if ($Interactive) { $cands = @(Remembered 'WitchyZip') + $cands }
    foreach ($c in $cands) { if ($c -and (Test-Path -LiteralPath $c)) { $WitchyZip = $c; break } }
}
$wz = ''
for ($i = 0; $i -lt 4; $i++) {
    if ($WitchyZip -and (Test-Path -LiteralPath $WitchyZip)) {
        $wz = Sha $WitchyZip
        if ($wz -eq $Pins.witchybnd_zip_sha256 -or -not $Interactive) { break }
        [void](Ui-Ask 'WitchyBND' ("$WitchyZip is not the official $WitchyName (sha256 {0}, want {1}). Choose another file or let setup download it." -f $wz.Substring(0, 12), $Pins.witchybnd_zip_sha256.Substring(0, 12)) 'OK' 'Warning')
        $wz = ''; $WitchyZip = ''
    }
    if ($Interactive) { $WitchyZip = Get-WitchyAsk }
    elseif ($DownloadWitchy -and $i -eq 0) { $WitchyZip = Get-WitchyByDownload }
    else { break }
    if (-not $WitchyZip) { break }
}
if (-not $WitchyZip -or -not (Test-Path -LiteralPath $WitchyZip)) { Stop-Setup 'WitchyBND-3.0.1.0-win-x64.zip not found: download it (INSTALL_GUIDE step 1) and pass -WitchyZip "<path>"' }
if (-not $wz) { $wz = Sha $WitchyZip }
if ($wz -eq $Pins.witchybnd_zip_sha256) { Say 'OK' "WitchyBND zip == the 3.0.1.0 release (sha256 $($wz.Substring(0,12)))" } else { Stop-Setup "WitchyBND zip sha256 $($wz.Substring(0,12)) is not the 3.0.1.0 win-x64 release ($($Pins.witchybnd_zip_sha256.Substring(0,12)))" }

# disk space
$drive = (Get-Item -LiteralPath $Root).PSDrive
if ($drive.Free -lt 6GB) { Stop-Setup ("only {0:N1} GB free on {1}: setup needs about 6 GB while it runs (about 0.3 GB stays)" -f ($drive.Free / 1GB), $drive.Name) } else { Say 'OK' ("{0:N0} GB free on {1}:" -f ($drive.Free / 1GB), $drive.Name) }

if ($script:fails -gt 0) { Stop-Setup 'fix the problems above, then run setup again' }

# every write below goes through this guard
$Forbidden = @($ErGame, (Split-Path -Parent $ErGame), $Mw2Dir, (Join-Path $env:APPDATA 'EldenRing'))
function Guard([string]$Path) { foreach ($f in $Forbidden) { if (Under $Path $f) { Stop-Setup "refusing to write $Path (inside $f)" } } }

# the paths found / chosen this time (never the password), so a second double-click asks nothing
if ($Interactive) {
    Guard $InputsFile
    $inputs = [ordered]@{ what = 'paths setup.ps1 used last time (no password)'; ErGame = $ErGame; Mw2Dir = $Mw2Dir; Me3 = $Me3; SeamlessDll = $SeamlessDll; WitchyZip = (Full $WitchyZip) }
    [IO.File]::WriteAllText($InputsFile, ($inputs | ConvertTo-Json) + "`r`n")
    # files of this package extracted from a downloaded zip carry the internet mark: Windows would ask before every Play
    Get-ChildItem -LiteralPath $Root -File | Where-Object { $_.Extension -in '.cmd', '.ps1' } | ForEach-Object { try { Unblock-File -LiteralPath $_.FullName } catch { } }
}
if ($CheckOnly) {
    Write-Host ''
    Write-Host "CHECK ONLY: step 1 passed ($script:warns warning(s)); nothing was built (run without -CheckOnly to install)." -ForegroundColor Green
    exit 0
}

# ---------------------------------------------------------------------------------------------------------------
Say 'STEP' '2/8 save backup (read-only copy of %APPDATA%\EldenRing)'
if (-not $BackupDir) { $BackupDir = Join-Path $Root 'backups' }
$BackupDir = Full $BackupDir
Guard $BackupDir
$saveRoot = Join-Path $env:APPDATA 'EldenRing'
if (Test-Path -LiteralPath $saveRoot) {
    $dest = Join-Path $BackupDir ('EldenRing-{0}' -f (Get-Date -Format 'yyyy-MM-dd_HHmmss'))
    $n = 0; $bad = 0
    foreach ($f in (Get-ChildItem -LiteralPath $saveRoot -Recurse -File)) {
        $rel = $f.FullName.Substring($saveRoot.Length).TrimStart('\')
        $to = Join-Path $dest $rel
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $to) | Out-Null
        Copy-Item -LiteralPath $f.FullName -Destination $to
        if ((Sha $f.FullName) -ne (Sha $to)) { $bad++ }
        $n++
    }
    if ($bad -eq 0) { Say 'OK' "$n save file(s) copied to $dest (hash-checked); keep this folder" } else { Stop-Setup "save backup: $bad file(s) differ after copying" }
} else { Say 'OK' "no $saveRoot yet (nothing to back up)" }

# ---------------------------------------------------------------------------------------------------------------
Say 'STEP' '3/8 WitchyBND (regulation tool)'
$wApp = Join-Path $Root 'tools\WitchyBND\app'
Guard $wApp
if (Test-Path -LiteralPath $wApp) { Remove-Item -LiteralPath $wApp -Recurse -Force }
New-Item -ItemType Directory -Force -Path $wApp | Out-Null
Expand-Archive -LiteralPath $WitchyZip -DestinationPath $wApp -Force
if (-not (Test-Path -LiteralPath (Join-Path $wApp 'WitchyBND.exe'))) {
    $inner = Get-ChildItem -LiteralPath $wApp -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'WitchyBND.exe') } | Select-Object -First 1
    if ($inner) { Get-ChildItem -LiteralPath $inner.FullName -Force | Move-Item -Destination $wApp; Remove-Item -LiteralPath $inner.FullName -Recurse -Force }
}
if ((Sha (Join-Path $wApp 'WitchyBND.exe')) -ne $Pins.witchybnd_exe_sha256) { Stop-Setup 'WitchyBND.exe after unzipping is not the expected 3.0.1.0 build' }
# WitchyBND unpacks its bundled Assets\Paramdex.zip on its first start (and that would undo the Paramdex upgrader bump below in
# the middle of the build): do it here first, as the host's copy has it
$pdz = Join-Path $wApp 'Assets\Paramdex.zip'
if (Test-Path -LiteralPath $pdz) {
    Expand-Archive -LiteralPath $pdz -DestinationPath (Join-Path $wApp 'Assets\Paramdex') -Force
    Remove-Item -LiteralPath $pdz -Force
}
# the two settings build_regulation.py requires (the host has the same): offline, strict, the host's Paramdex upgrader version (host_pins paramdex_version)
[IO.File]::WriteAllText((Join-Path $wApp 'appsettings.override.json'), "{`n  `"Offline`": true,`n  `"PauseOnError`": false,`n  `"EndDelay`": 0,`n  `"Flexible`": false`n}`n")
$vt = Join-Path $wApp 'Assets\Paramdex\ER\Upgrader\version.txt'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $vt) | Out-Null
[IO.File]::WriteAllText($vt, $Pins.paramdex_version)
Say 'OK' "WitchyBND 3.0.1.0 in tools\WitchyBND\app (Offline, Flexible=false, Paramdex upgrader $($Pins.paramdex_version))"

# ---------------------------------------------------------------------------------------------------------------
function Run-Py([string]$Name, [string[]]$PyArgs, [int]$Minutes) {
    $ErrorActionPreference = 'Continue'   # native stderr lines are not errors here (PowerShell 5.1)
    $log = Join-Path $LogDir "$Name.log"
    $t0 = Get-Date
    $out = & $Python -I @PyArgs 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $out | Set-Content -LiteralPath $log -Encoding UTF8
    $last = ($out | Where-Object { $_ -match '\S' } | Select-Object -Last 1)
    $dt = [int]((Get-Date) - $t0).TotalSeconds
    if ($code -eq 0) { Say 'OK' "$Name ($dt s): $last" } else { Say 'FAIL' "$Name exit $code ($dt s): $last (full log: setup_logs\$Name.log)" }
    return ($code -eq 0)
}

$env:MW2ER_IW4SP = $exe
$env:MW2ER_GAME_REGULATION = $erReg
$env:MW2ER_SNDCONV = Join-Path $Root 'tools\bin\sndconv.exe'
$env:PYTHONDONTWRITEBYTECODE = '1'

Say 'STEP' '4/8 MW2 dumps from YOUR MW2 install (OpenAssetTools Unlinker; 5-15 minutes, ~2 GB temporary)'
foreach ($d in @('dumps', 'converted', 'work', 'build\package', 'build\rows.json')) { $p = Join-Path $Root $d; Guard $p; if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Recurse -Force } }
if (-not (Run-Py 'dumps' @((Join-Path $Scripts 'friend_dump.py'), '--mw2', $Mw2Dir, '--unlinker', (Join-Path $Root 'tools\oat\bin\Unlinker.exe')) 20)) { Stop-Setup 'the MW2 dump failed (setup_logs\dumps.log, dumps\*.log)' }

Say 'STEP' '5/8 converters (MW2 data -> converted\; a few minutes)'
$steps = @(
    @('convert_weapons', 'convert_weapons.py'),
    @('copy_sounds', 'copy_sounds.py'),
    @('convert_viewmodel', 'convert_viewmodel.py'),
    @('convert_ac130', 'convert_ac130.py'),
    @('convert_movement', 'convert_movement.py'),
    @('convert_arsenal', 'convert_arsenal.py'),
    @('convert_nuke', 'convert_nuke.py')
)
foreach ($s in $steps) {
    if (-not (Run-Py $s[0] @((Join-Path $Scripts $s[1])) 10)) { Stop-Setup "$($s[1]) failed" }
}
if (-not (Run-Py 'verify_converted' @((Join-Path $Scripts 'coop_manifest.py'), 'verify', $Manifest, $Root, '--only', 'converted') 5)) {
    Stop-Setup "your converted files differ from the host's (setup_logs\verify_converted.log). Most likely your MW2 is a different build than the host's; tell the host."
}

Say 'STEP' '6/8 regulation (rebuilt from YOUR vanilla regulation.bin; 2-5 minutes)'
if (-not (Run-Py 'build_regulation' @((Join-Path $Scripts 'build_regulation.py')) 15)) { Stop-Setup 'build_regulation.py failed (setup_logs\build_regulation.log)' }

Say 'STEP' '7/8 verify everything against the host (sha256)'
if (-not (Run-Py 'verify_all' @((Join-Path $Scripts 'coop_manifest.py'), 'verify', $Manifest, $Root, '--only', 'all') 5)) {
    Stop-Setup "a file differs from the host's (setup_logs\verify_all.log): co-op needs byte-identical files. Send setup_logs\ to the host."
}
if (-not $KeepDumps) {
    foreach ($d in @('dumps', 'work')) { $p = Join-Path $Root $d; Guard $p; if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Recurse -Force } }
    Say 'OK' 'temporary dumps\ and work\ removed (use -KeepDumps to keep them)'
}

# ---------------------------------------------------------------------------------------------------------------
Say 'STEP' '8/8 me3 profiles'
$dllPath = Join-Path $Root 'build\natives\mw2er.dll'
$pkgPath = Join-Path $Root 'build\package'
$tpl = Get-Content -LiteralPath (Join-Path $Root 'profiles\mw2er-coop.template.me3') -Raw
$prof = $tpl.Replace('@@ERSC@@', $SeamlessDll).Replace('@@DLL@@', $dllPath).Replace('@@PACKAGE@@', $pkgPath)
$profPath = Join-Path $Root 'profiles\mw2er-coop.me3'
Guard $profPath
[IO.File]::WriteAllText($profPath, $prof)
Say 'OK' "wrote $profPath (Seamless first, then mw2er.dll, then the regulation package)"
$launch = "& `"$Me3`" launch -g eldenring -p `"$profPath`" --no-mem-patch"
$launchText = $launch + "`r`n"
$soloTpl = Join-Path $Root 'profiles\mw2er-solo.template.me3'
if (Test-Path -LiteralPath $soloTpl) {
    $soloPath = Join-Path $Root 'profiles\mw2er-solo.me3'
    Guard $soloPath
    [IO.File]::WriteAllText($soloPath, (Get-Content -LiteralPath $soloTpl -Raw).Replace('@@DLL@@', $dllPath).Replace('@@PACKAGE@@', $pkgPath))
    Say 'OK' "wrote $soloPath (solo: mw2er.dll + the regulation package, own save ER0000.mw2er.sl2, no Seamless)"
    $launchText += "# solo (no Seamless):`r`n& `"$Me3`" launch -g eldenring -p `"$soloPath`"`r`n"
}
[IO.File]::WriteAllText((Join-Path $Root 'launch_command.txt'), $launchText)

if ($Shortcuts -or ($Interactive -and (Ui-Ask 'Desktop shortcuts' "Setup finished. Create desktop shortcuts?`n`n  MW2 in Elden Ring (co-op)`n  MW2 in Elden Ring (solo)`n`nThey start the game through me3 the right way (Steam must be running)." 'YesNo' 'Question') -eq 'Yes')) {
    try { foreach ($l in (New-PlayShortcuts $ShortcutDir (Join-Path $ErGame 'eldenring.exe'))) { Say 'OK' "shortcut $l" } }
    catch { Say 'WARN' "could not create the shortcuts ($($_.Exception.Message)); double-click 'Play co-op.cmd' in this folder instead" }
}

Write-Host ''
if ($script:warns -gt 0) { Write-Host "SETUP PASSED with $script:warns warning(s) (see above)." -ForegroundColor Yellow } else { Write-Host 'SETUP PASSED: every file matches the host.' -ForegroundColor Green }
Write-Host 'Start the game (Steam running, Seamless launcher NOT used): double-click "Play co-op.cmd" in this folder'
Write-Host '(or the desktop shortcut "MW2 in Elden Ring (co-op)"), or run:'
Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $Root 'launch_coop.ps1')`""
Write-Host 'or directly:'
Write-Host "  $launch"
exit 0
