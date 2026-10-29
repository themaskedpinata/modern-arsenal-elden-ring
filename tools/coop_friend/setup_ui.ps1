<#
.SYNOPSIS
  Helpers of setup.ps1 for the double-click install (Install.cmd -> setup.ps1 -Interactive): dialogs with a console
  fallback, the hash-pinned WitchyBND download, the Seamless zip import, the two Seamless ini keys, desktop shortcuts.

.DESCRIPTION
  Dot-sourced by setup.ps1 (and by the unit tests). Never starts a game, me3, Steam or Seamless. Never reads, prints or
  stores the co-op password: the ini helpers only test whether the cooppassword line is non-empty, and the password is
  typed by the player into Notepad.
  UI modes: gui (Windows Forms message boxes and file pickers; needs an STA thread, which powershell.exe -STA gives),
  console (typed answers; used with -NoGui, MW2ER_SETUP_NOGUI=1, or when Windows Forms cannot load), none (not
  interactive: every question answers Cancel, i.e. the old command-line behaviour).
#>

if (-not (Get-Command Say -ErrorAction SilentlyContinue)) {
    function Say([string]$Status, [string]$Message) { Write-Host ('{0,-4} {1}' -f $Status, $Message) }
}

$script:UiMode = 'none'

function Initialize-SetupUi([bool]$Interactive, [bool]$NoGui) {
    $script:UiMode = 'none'
    if (-not $Interactive) { return $script:UiMode }
    $script:UiMode = 'console'
    if ($NoGui -or $env:MW2ER_SETUP_NOGUI -eq '1') { return $script:UiMode }
    if (-not [Environment]::UserInteractive) { return $script:UiMode }
    if ([Threading.Thread]::CurrentThread.GetApartmentState() -ne [Threading.ApartmentState]::STA) { return $script:UiMode }
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
        [System.Windows.Forms.Application]::EnableVisualStyles()
        $script:UiMode = 'gui'
    } catch { $script:UiMode = 'console' }
    return $script:UiMode
}

function Read-SetupLine([string]$Prompt) {
    Write-Host $Prompt -NoNewline -ForegroundColor Yellow
    $l = [Console]::In.ReadLine()
    if ($null -eq $l) { Write-Host ''; return $null }
    return $l.Trim()
}

# Yes / No / Cancel / OK question. Returns 'Yes', 'No', 'Cancel' or 'OK'. Not interactive: 'Cancel'.
function Ui-Ask([string]$Title, [string]$Text, [string]$Buttons = 'YesNo', [string]$Icon = 'Question') {
    if ($script:UiMode -eq 'gui') {
        $owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true }
        try {
            $r = [System.Windows.Forms.MessageBox]::Show($owner, $Text, $Title, [System.Windows.Forms.MessageBoxButtons]::$Buttons, [System.Windows.Forms.MessageBoxIcon]::$Icon)
        } finally { $owner.Dispose() }
        return [string]$r
    }
    if ($script:UiMode -eq 'console') {
        $choices = @{ 'YesNo' = @('Yes', 'No'); 'YesNoCancel' = @('Yes', 'No', 'Cancel'); 'OKCancel' = @('OK', 'Cancel'); 'OK' = @('OK') }[$Buttons]
        Write-Host ''
        Write-Host "== $Title ==" -ForegroundColor Cyan
        Write-Host $Text
        if ($choices.Count -eq 1) { [void](Read-SetupLine '[press Enter] '); return 'OK' }
        $hint = '[' + (($choices | ForEach-Object { $_.Substring(0, 1).ToLowerInvariant() + ' = ' + $_ }) -join ', ') + '] '
        for ($i = 0; $i -lt 10; $i++) {
            $a = Read-SetupLine $hint
            if ($null -eq $a) { break }   # end of input: the safe answer below
            if ($a.Length -gt 0) { foreach ($c in $choices) { if ($c.StartsWith($a.Substring(0, 1), [StringComparison]::OrdinalIgnoreCase)) { return $c } } }
        }
        if ($choices -contains 'Cancel') { return 'Cancel' } else { return 'No' }
    }
    return 'Cancel'
}

# File picker. Returns the full path or '' (cancelled / not interactive).
function Ui-PickFile([string]$Title, [string]$Filter, [string]$InitialDir = '') {
    if ($script:UiMode -eq 'gui') {
        $d = New-Object System.Windows.Forms.OpenFileDialog
        $d.Title = $Title
        $d.Filter = $Filter
        $d.CheckFileExists = $true
        $d.Multiselect = $false
        $d.RestoreDirectory = $true
        if ($InitialDir -and (Test-Path -LiteralPath $InitialDir -PathType Container)) { $d.InitialDirectory = $InitialDir }
        $owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true }
        try {
            if ($d.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { return $d.FileName }
        } finally { $owner.Dispose(); $d.Dispose() }
        return ''
    }
    if ($script:UiMode -eq 'console') {
        Write-Host ''
        Write-Host "== $Title ==" -ForegroundColor Cyan
        Write-Host '(no file dialog here: paste the full path of the file and press Enter; just Enter = cancel)'
        $a = Read-SetupLine 'path> '
        if (-not $a) { return '' }
        $a = $a.Trim().Trim('"').Trim("'")
        $ok = $false
        try { $ok = Test-Path -LiteralPath $a -PathType Leaf } catch { $ok = $false }   # a typo with illegal characters is just "not a file"
        if ($ok) { return [IO.Path]::GetFullPath($a) }
        Write-Host "not a file: $a" -ForegroundColor Red
        return ''
    }
    return ''
}

# Opens an official web page in the default browser (never downloads anything by itself).
function Ui-OpenUrl([string]$Url) {
    Write-Host "opening $Url"
    if ($script:UiMode -eq 'none' -or $env:MW2ER_SETUP_NOBROWSER -eq '1') { return }
    try { Start-Process $Url } catch { Write-Host "could not open the browser: open $Url yourself" -ForegroundColor Yellow }
}

function Get-DownloadsDir {
    $d = Join-Path $env:USERPROFILE 'Downloads'
    try {
        $p = (New-Object -ComObject Shell.Application).NameSpace('shell:Downloads').Self.Path
        if ($p -and (Test-Path -LiteralPath $p -PathType Container)) { $d = $p }
    } catch { }
    return $d
}

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

# Download $Url to $Dest through $Dest.part; the file is kept ONLY if its sha256 equals $Sha256 (else deleted).
# Returns @{ Ok; Path; Sha; Error }. -DryRun prints what it would do and downloads nothing.
function Get-PinnedDownload([string]$Url, [string]$Dest, [string]$Sha256, [switch]$DryRun) {
    $res = @{ Ok = $false; Path = $Dest; Sha = ''; Error = '' }
    $pin = $Sha256.ToLowerInvariant()
    if ($pin -notmatch '^[0-9a-f]{64}$') { $res.Error = 'no valid sha256 pin: refusing to download'; return $res }
    if ($DryRun) {
        Say 'INFO' ("dry run: would download {0} -> {1} and keep it only if sha256 == {2}" -f $Url, $Dest, $pin.Substring(0, 12))
        $res.Error = 'dry run'
        return $res
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Dest) | Out-Null
    $part = "$Dest.part"
    if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $wc = New-Object System.Net.WebClient
        $wc.Headers['User-Agent'] = 'MW2inER-setup'
        try { $wc.DownloadFile($Url, $part) } finally { $wc.Dispose() }
    } catch {
        if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
        $res.Error = "download failed: $($_.Exception.Message)"
        return $res
    }
    $h = Get-Sha256 $part
    $res.Sha = $h
    if ($h -ne $pin) {
        Remove-Item -LiteralPath $part -Force
        $res.Error = ("the downloaded file has sha256 {0}, not the pinned {1}: deleted, not used" -f $h.Substring(0, 12), $pin.Substring(0, 12))
        return $res
    }
    if (Test-Path -LiteralPath $Dest) { Remove-Item -LiteralPath $Dest -Force }
    Move-Item -LiteralPath $part -Destination $Dest
    $res.Ok = $true
    return $res
}

# ---- Seamless Co-op ---------------------------------------------------------------------------------------------

function Get-SeamlessZipEntry($Zip) {
    # the entry ...SeamlessCoop/ersc.dll (a Nexus zip has it at the top: SeamlessCoop/ersc.dll)
    foreach ($e in $Zip.Entries) {
        $n = $e.FullName -replace '\\', '/'
        if ($n -match '(?i)^(.*/)?SeamlessCoop/ersc\.dll$') { return $e }
    }
    return $null
}

function Get-StreamSha([IO.Stream]$S) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return (($sha.ComputeHash($S) | ForEach-Object { $_.ToString('x2') }) -join '') } finally { $sha.Dispose() }
}

# sha256 of SeamlessCoop/ersc.dll inside a zip, '' if the zip has none or cannot be read
function Get-SeamlessZipSha([string]$ZipPath) {
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $z = [IO.Compression.ZipFile]::OpenRead($ZipPath)
        try {
            $e = Get-SeamlessZipEntry $z
            if (-not $e) { return '' }
            $s = $e.Open()
            try { return (Get-StreamSha $s) } finally { $s.Dispose() }
        } finally { $z.Dispose() }
    } catch { return '' }
}

# Unpacks the SeamlessCoop folder of the player's own Seamless zip into $ImportDir\SeamlessCoop (inside this package,
# never a game folder). An existing copy with the pinned ersc.dll is reused as it is (its ini may already hold the
# password). Returns @{ Ok; Dll; Error }.
function Import-SeamlessZip([string]$ZipPath, [string]$ImportDir, [string]$Pin) {
    $res = @{ Ok = $false; Dll = ''; Error = '' }
    $target = Join-Path $ImportDir 'SeamlessCoop'
    $dll = Join-Path $target 'ersc.dll'
    if ((Test-Path -LiteralPath $dll) -and (Get-Sha256 $dll) -eq $Pin) { $res.Ok = $true; $res.Dll = $dll; return $res }
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $z = [IO.Compression.ZipFile]::OpenRead($ZipPath)
    } catch { $res.Error = "cannot read the zip: $($_.Exception.Message)"; return $res }
    try {
        $e = Get-SeamlessZipEntry $z
        if (-not $e) { $res.Error = 'this zip has no SeamlessCoop\ersc.dll'; return $res }
        $s = $e.Open(); try { $h = Get-StreamSha $s } finally { $s.Dispose() }
        if ($h -ne $Pin) { $res.Error = ("the ersc.dll in this zip has sha256 {0}, not the pinned Seamless Co-op ({1})" -f $h.Substring(0, 12), $Pin.Substring(0, 12)); return $res }
        $prefix = ($e.FullName -replace '\\', '/').Substring(0, ($e.FullName.Length - 'ersc.dll'.Length))
        if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $target | Out-Null
        $full = [IO.Path]::GetFullPath($target).TrimEnd('\') + '\'
        foreach ($x in $z.Entries) {
            $n = $x.FullName -replace '\\', '/'
            if (-not $n.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or $n.EndsWith('/')) { continue }
            $rel = $n.Substring($prefix.Length)
            $to = [IO.Path]::GetFullPath((Join-Path $target ($rel -replace '/', '\')))
            if (-not $to.StartsWith($full, [StringComparison]::OrdinalIgnoreCase)) { continue }   # no path escapes
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $to) | Out-Null
            [IO.Compression.ZipFileExtensions]::ExtractToFile($x, $to, $true)
        }
    } finally { $z.Dispose() }
    if (-not (Test-Path -LiteralPath $dll) -or (Get-Sha256 $dll) -ne $Pin) { $res.Error = 'ersc.dll after unpacking does not match'; return $res }
    $res.Ok = $true; $res.Dll = $dll
    return $res
}

function Test-UnderDir([string]$Child, [string]$Parent) {
    if (-not $Child -or -not $Parent) { return $false }
    $c = [IO.Path]::GetFullPath($Child).TrimEnd('\') + '\'
    $p = [IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    return $c.StartsWith($p, [StringComparison]::OrdinalIgnoreCase)
}

# Looks for the pinned Seamless without asking: remembered paths and the copy inside this package first, then
# <base>\SeamlessCoop\ersc.dll and <base>\<folder>\SeamlessCoop\ersc.dll for each base, then a Seamless zip in Downloads.
# Never a copy inside the Elden Ring folder. Returns @{ Dll; Zip }.
function Find-SeamlessAuto([string]$Pin, [string[]]$Direct, [string[]]$Bases, [string]$ZipDir, [string]$ErGame) {
    $found = @{ Dll = ''; Zip = '' }
    $cands = New-Object System.Collections.ArrayList
    foreach ($d in $Direct) { if ($d) { [void]$cands.Add($d) } }
    foreach ($b in $Bases) {
        if (-not $b -or -not (Test-Path -LiteralPath $b -PathType Container)) { continue }
        [void]$cands.Add((Join-Path $b 'SeamlessCoop\ersc.dll'))
        foreach ($sub in @(Get-ChildItem -LiteralPath $b -Directory -ErrorAction SilentlyContinue)) { [void]$cands.Add((Join-Path $sub.FullName 'SeamlessCoop\ersc.dll')) }
    }
    foreach ($c in $cands) {
        if (-not (Test-Path -LiteralPath $c -PathType Leaf)) { continue }
        if ($ErGame -and (Test-UnderDir $c (Split-Path -Parent $ErGame))) { continue }
        if ((Get-Sha256 $c) -eq $Pin) { $found.Dll = [IO.Path]::GetFullPath($c); return $found }
    }
    if ($ZipDir -and (Test-Path -LiteralPath $ZipDir -PathType Container)) {
        $zips = @(Get-ChildItem -LiteralPath $ZipDir -File -Filter '*.zip' -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '(?i)seamless' } | Sort-Object LastWriteTime -Descending)
        foreach ($z in $zips) { if ((Get-SeamlessZipSha $z.FullName) -eq $Pin) { $found.Zip = $z.FullName; return $found } }
    }
    return $found
}

# Only these three keys are looked at; the password VALUE is never returned, printed or stored.
function Get-SeamlessIniState([string]$Ini) {
    $vals = @{}
    foreach ($line in (Get-Content -LiteralPath $Ini)) {
        if ($line -match '^\s*(allow_invaders|save_file_extension|cooppassword)\s*=\s*(.*?)\s*$') { $vals[$Matches[1]] = $Matches[2] }
    }
    $st = @{
        allow_invaders      = $vals['allow_invaders']
        save_file_extension = $vals['save_file_extension']
        password_line       = $vals.ContainsKey('cooppassword')
        password_set        = ($vals.ContainsKey('cooppassword') -and $vals['cooppassword'].Length -gt 0)
    }
    $vals = $null
    return $st
}

# Sets allow_invaders = 0 and save_file_extension = mw2co2 in place; every other byte of the file stays as it was
# (read and written as Latin-1 = byte for byte; the password line is not touched). Returns the number of keys found (0..2).
function Set-SeamlessIniKeys([string]$Ini) {
    $enc = [Text.Encoding]::GetEncoding(28591)
    $text = $enc.GetString([IO.File]::ReadAllBytes($Ini))
    $n = 0
    if ($text -match '(?m)^[ \t]*allow_invaders[ \t]*=') { $n++ }
    if ($text -match '(?m)^[ \t]*save_file_extension[ \t]*=') { $n++ }
    $text = [regex]::Replace($text, '(?m)^([ \t]*allow_invaders[ \t]*=)[^\r\n]*', '${1} 0')
    $text = [regex]::Replace($text, '(?m)^([ \t]*save_file_extension[ \t]*=)[^\r\n]*', '${1} mw2co2')
    [IO.File]::WriteAllBytes($Ini, $enc.GetBytes($text))
    return $n
}

# The player types the password into Notepad; setup only checks afterwards that the line is not empty.
function Edit-SeamlessPassword([string]$Ini) {
    $msg = "Seamless Co-op needs the co-op password the host gave you (both PCs use the same one).`n`n" +
           "Notepad opens your ersc_settings.ini now:`n" +
           "  1. find the line  cooppassword =  (under [PASSWORD])`n" +
           "  2. type the password after the = sign`n" +
           "  3. save (Ctrl+S) and close Notepad.`n`n" +
           "Setup never shows, logs or copies the password."
    $a = Ui-Ask 'Co-op password' $msg 'OKCancel' 'Information'
    if ($a -ne 'OK') { return $false }
    if ($env:MW2ER_SETUP_NONOTEPAD -eq '1') { return $true }   # tests: no editor
    try { Start-Process -FilePath 'notepad.exe' -ArgumentList ('"{0}"' -f $Ini) -Wait } catch { Write-Host "could not start Notepad: edit $Ini yourself" -ForegroundColor Yellow; [void](Ui-Ask 'Co-op password' "Edit $Ini, then click OK." 'OK' 'Information') }
    return $true
}

# ---- shortcuts ---------------------------------------------------------------------------------------------------

function New-SetupShortcut([string]$Dir, [string]$Name, [string]$Target, [string]$WorkDir, [string]$Icon, [string]$Description) {
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $lnk = Join-Path $Dir ($Name + '.lnk')
    $sh = New-Object -ComObject WScript.Shell
    try {
        $s = $sh.CreateShortcut($lnk)
        $s.TargetPath = $Target
        $s.WorkingDirectory = $WorkDir
        $s.Description = $Description
        if ($Icon) { $s.IconLocation = $Icon }
        $s.WindowStyle = 1
        $s.Save()
    } finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($sh) }
    return $lnk
}
