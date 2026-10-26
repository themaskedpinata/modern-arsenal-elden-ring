<#
Builds every compiled binary of Modern Arsenal for Elden Ring from this repository (Release, x64):
  mw2er.dll (the mod), MW2inER-setup.exe (setup / launcher / updater, without the embedded payload),
  sndconv.exe (sound converter) and regbuild.exe (regulation builder).
Fetches vendor\fromsoftware-rs first when missing. Remaps local paths out of the binaries like the release builds do.
Prints the sha256 of each output. See BUILD.md.
#>
$ErrorActionPreference = 'Stop'
$Root = $PSScriptRoot
& (Join-Path $Root 'tools\fetch_vendor.ps1')

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
# RUSTFLAGS replaces the rustflags of mod\.cargo\config.toml, so it repeats them.
$env:RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=/PDBALTPATH:%_PDB% " +
    "--remap-path-prefix=$cargoHome=cargo --remap-path-prefix=$rustupHome=rustup --remap-path-prefix=$Root=mw2iner"
$env:CFLAGS_x86_64_pc_windows_msvc = "/d1trimfile:$cargoHome\"
$env:CXXFLAGS_x86_64_pc_windows_msvc = "/d1trimfile:$cargoHome\"

Push-Location (Join-Path $Root 'mod')
try {
    # cargo writes progress to stderr, which Windows PowerShell 5.1 turns into errors under 'Stop'
    $ErrorActionPreference = 'Continue'
    cargo build --release --locked -p mw2er -p installer -p sndconv -p regbuild 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code) { throw "cargo build failed ($code)" }
} finally { Pop-Location }

$out = Join-Path $Root 'mod\target\release'
foreach ($f in 'mw2er.dll', 'MW2inER-setup.exe', 'sndconv.exe', 'regbuild.exe') {
    $p = Join-Path $out $f
    if (-not (Test-Path $p)) { throw "missing output $f" }
    "{0}  {1}" -f (Get-FileHash $p -Algorithm SHA256).Hash.ToLowerInvariant(), $f
}

