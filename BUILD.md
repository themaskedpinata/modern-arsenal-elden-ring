# Building Modern Arsenal for Elden Ring

## 1. The compiled binaries (anyone can do this)

Every compiled file in the release is built from this repository:

| Release file | Built from |
|---|---|
| `MW2inER-<version>-setup.exe` (before the payload is appended, see 2.) | `mod/installer` |
| `build/natives/mw2er.dll` (inside the setup) | `mod/mw2er` |
| `tools/bin/sndconv.exe` (inside the setup) | `mod/sndconv` |
| the regulation builder (compiled into the setup) | `mod/regbuild` |

Requirements (Windows 10/11 x64):

- Rust 1.99.0 or newer, target `x86_64-pc-windows-msvc` (https://rustup.rs)
- Visual Studio 2022 Build Tools with "Desktop development with C++" (MSVC and the Windows SDK; the zstd and imgui C/C++
  sources are compiled by the `cc` crate)
- git

Steps, in PowerShell from the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File build.ps1
```

`build.ps1` does:

1. `tools/fetch_vendor.ps1`: clones https://github.com/vswarte/fromsoftware-rs at commit
   `59fbd3b3b7daaf14aca47c9f73530493dba6bc79` into `vendor/fromsoftware-rs` and applies
   `tools/patches/fromsoftware-rs-000*.patch` (git apply, in order).
2. `cargo build --release --locked -p mw2er -p installer -p sndconv -p regbuild` in `mod/`, with the static CRT
   (`mod/.cargo/config.toml`) and path remapping so no local path is embedded.
3. Prints the sha256 of `mod/target/release/mw2er.dll`, `MW2inER-setup.exe`, `sndconv.exe` and `regbuild.exe`.

Tests: `cd mod; cargo test --release -p mw2er-core -p regbuild -p installer`.

Rust builds embed some build-machine details, so a rebuild is not guaranteed to be byte-identical to the release.
The release's `manifest/release.json` lists the sha256 of every file the setup carries.

## 2. How the release setup is packaged

`tools/scripts/make_release.ps1` turns the binaries into the one-file setup. It runs on the author's PC because it
needs inputs that cannot be in a public repository:

- the hashes of the converted data built from the author's own Elden Ring 2.7.2.0 and MW2 installs
  (`manifest/host_manifest.json`), which every player's setup checks its own build against (only hashes, no game data);
- the pinned official downloads it bundles unmodified: the CPython 3.14.7 embeddable zip
  (python.org, sha256 pinned in `mod/installer/src/payload.rs`) and the me3 0.13.0 installer (github.com/garyttierney/me3,
  sha256 pinned in `mod/installer/src/release.rs`);
- the OpenAssetTools Unlinker, built from https://github.com/iw4x-x64/oat at commit
  `df203f9fe5820ea8f63e50d524fb437861f8e427` with its submodules and `tools/oat-local-build-fixes.patch` (two MSVC build
  fixes), `premake5 vs2022`, then msbuild Release x64. Its full source is also uploaded as an optional file on the Nexus
  page (GPL-3.0).

Packaging stages those files with `tools/coop_friend/make_friend_package.ps1`, writes `manifest/release.json`
(`tools/scripts/release_tool.py manifest`), lints the payload (no game files, no paths or ids of the build PC), appends
the payload zip to `MW2inER-setup.exe` with a 64-byte trailer (`mod/installer/src/payload.rs`) and self-tests the result.

## 3. What the setup does on a player's PC

It reads the player's MW2 files with the Unlinker, converts what the mod needs with the Python scripts in
`tools/scripts` (run by the bundled CPython), builds the mod's regulation from the player's vanilla `regulation.bin`
(`mod/regbuild`), checks every result against the release's hashes and writes me3 profiles. It never writes to the game
folders; it installs into `%LOCALAPPDATA%\MW2inER`.
