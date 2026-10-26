//! Version gate (docs/M2_DESIGN.md section 6; port 2.7.2.0, MODLOG 2026-10-20). Runs on the boot thread before ANY
//! fromsoftware-rs call: on an unknown exe the crate's `rva::get()` panics inside a LazyLock and poisons it, after
//! which every address-based call panics. All three checks must pass, or the DLL stays inert:
//! 1. the pure decision `mw2er_core::gate::decide` (exe name, FileVersion == ProductVersion in {2.7.1.0, 2.7.2.0},
//!    language; unit-tested offline) on values read from the host exe; it names the build;
//! 2. fromsoftware-rs's own detection logic, mirrored for its Ww2710 / Ww2720 tables (vendor patch 0004), must pick
//!    the same build;
//! 3. the build's address preflight (`mw2er_core::exe_build::preflight`) on the LOADED image: every crate RVA and
//!    every address of ours resolves, every code window equals the exe file's bytes, every vtable's RTTI names its
//!    class. One failing site keeps the DLL inert (it is never guessed around). Exceptions (review 2026-10-20,
//!    logged as WARN): a code window of a crate function mw2er never calls, and another module's `jmp rel32` detour
//!    over a function mw2er only calls (`exe_build::Role`). Before it, the PE header (TimeDateStamp, SizeOfImage)
//!    must be the build's own.
//!
//! The build is then published ([`build`], [`own`]) for the hooks, which use only that build's addresses.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use fromsoftware_shared::game_version::{GameVersion, LANG_ID_EN};
use mw2er_core::exe_build::{self, ExeBuild, ImageView, OwnAddrs, Preflight};
use mw2er_core::gate::{self, Quad, fmt_quad};
use pelite::pe64::{Pe, PeView};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

use crate::game::read;

#[derive(Clone, Debug)]
pub struct GateInfo {
    pub file_version: Quad,
    pub product_version: Quad,
    pub lang: u16,
    pub time_date_stamp: u32,
    pub size_of_image: u32,
    pub build: ExeBuild,
    pub preflight: String,
    /// Sites whose RVA is only inferred (they passed their check; none is used by mw2er's own code).
    pub inferred: Vec<&'static str>,
    /// Tolerated differences (a code site mw2er never calls, or another module's detour on one it calls).
    pub warnings: Vec<String>,
}

impl GateInfo {
    pub fn pass_line(&self) -> String {
        format!(
            "PASS file={} product={} lang=0x{:04x} timedatestamp=0x{:08x} sizeofimage=0x{:x} table={} preflight {}",
            fmt_quad(self.file_version),
            fmt_quad(self.product_version),
            self.lang,
            self.time_date_stamp,
            self.size_of_image,
            self.build.table_name(),
            self.preflight
        )
    }
}

/// The build the gate accepted (set once, before `WRITES_ALLOWED`; `None` = inert).
static BUILD: OnceLock<ExeBuild> = OnceLock::new();

/// The accepted build, `None` before / without a passed gate.
pub fn build() -> Option<ExeBuild> {
    BUILD.get().copied()
}

/// The accepted build's own addresses (`Err` = the gate did not pass: callers install nothing).
pub fn own() -> Result<&'static OwnAddrs, String> {
    build().map(ExeBuild::own).ok_or_else(|| "no exe build accepted by the version gate".to_string())
}

/// The same check fromsoftware-rs runs internally to pick its Ww2710 / Ww2720 RVA tables
/// (`ERGameVersion` is private in the crate, so its WW arms are mirrored here; vendor patch 0004).
struct ErWw(ExeBuild);

impl GameVersion for ErWw {
    const NAME: &'static str = "elden ring";

    fn from_lang_version(lang_id: u16, version: &str) -> Option<Self> {
        match (lang_id, version) {
            (LANG_ID_EN, "2.7.1.0") => Some(ErWw(ExeBuild::Ww2710)),
            (LANG_ID_EN, "2.7.2.0") => Some(ErWw(ExeBuild::Ww2720)),
            _ => None,
        }
    }
}

const _: () = assert!(LANG_ID_EN == gate::WANT_PRIMARY_LANG);

/// The loaded exe image, read without ever faulting.
struct LiveImage<'a> {
    base: usize,
    view: PeView<'a>,
}

impl ImageView for LiveImage<'_> {
    fn base(&self) -> u64 {
        self.base as u64
    }
    fn size_of_image(&self) -> u32 {
        self.view.optional_header().SizeOfImage
    }
    fn section_flags(&self, rva: u32) -> Option<u32> {
        self.view
            .section_headers()
            .as_slice()
            .iter()
            .find(|h| (h.VirtualAddress..h.VirtualAddress.saturating_add(h.VirtualSize)).contains(&rva))
            .map(|h| h.Characteristics)
    }
    fn read(&self, rva: u32, out: &mut [u8]) -> bool {
        let a = self.base + rva as usize;
        for (i, b) in out.iter_mut().enumerate() {
            // SAFETY: non-faulting read of one byte of the exe image.
            match unsafe { read::<u8>(a + i) } {
                Some(v) => *b = v,
                None => return false,
            }
        }
        true
    }
}

/// C4 (docs/C4_UI_DESIGN.md 7): checks extra sites (`exe_build::scaleform_ui_sites`) on the loaded image, with the
/// same rules as the boot preflight. Only called when `[ui]` asks for the C4 hooks; a failure skips them only.
pub fn check_sites(sites: &[exe_build::Site]) -> Result<Preflight, String> {
    // SAFETY: `None` = the exe module, which stays loaded for the process lifetime.
    let base = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    // SAFETY: base is the mapped exe image.
    let view = unsafe { PeView::module(base.0 as *const u8) };
    let img = LiveImage { base: base.0 as usize, view };
    Ok(exe_build::preflight_sites(sites, &img))
}

/// Runs the gate. `Err` carries the message for `VERSION FAIL ...; staying inert`.
pub fn check() -> Result<GateInfo, String> {
    // The host exe's path.
    let mut buf = vec![0u16; 32_768];
    // SAFETY: valid buffer; `None` = the exe module.
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    let exe_path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    // Name first: nothing is parsed out of a host that is not eldenring.exe.
    gate::check_exe(&exe_path)?;

    // Version resource of the loaded image.
    // SAFETY: `None` = the exe module, which stays loaded for the process lifetime.
    let base = unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    // SAFETY: base is the mapped exe image.
    let view = unsafe { PeView::module(base.0 as *const u8) };
    let resources = view.resources().map_err(|e| format!("no resources: {e}"))?;
    let vi = resources.version_info().map_err(|e| format!("no version info: {e}"))?;
    let fixed = vi.fixed().ok_or("no VS_FIXEDFILEINFO")?;
    let fv = fixed.dwFileVersion;
    let pv = fixed.dwProductVersion;
    let file_version = (fv.Major, fv.Minor, fv.Patch, fv.Build);
    let product_version = (pv.Major, pv.Minor, pv.Patch, pv.Build);
    let lang = vi.translation().first().map_or(0, |l| l.lang_id);
    let build = gate::decide(&exe_path, file_version, product_version, lang)?;

    // The crate's own detection logic must accept it too, as the same build (detect unwraps internally).
    let detected = catch_unwind(AssertUnwindSafe(|| ErWw::detect(&view).map(|d| d.0).map_err(|e| e.to_string())));
    match detected {
        Ok(Ok(b)) if b == build => {}
        Ok(Ok(b)) => return Err(format!("found {} but fromsoftware-rs detection picked {}", fmt_quad(file_version), b.table_name())),
        Ok(Err(e)) => return Err(format!("found {} but fromsoftware-rs detection failed: {e}", fmt_quad(file_version))),
        Err(_) => return Err(format!("found {} but fromsoftware-rs detection panicked", fmt_quad(file_version))),
    }

    let time_date_stamp = view.file_header().TimeDateStamp;
    let size_of_image = view.optional_header().SizeOfImage;
    // The build itself: same version number but other code (another Steam build) must not pass.
    exe_build::check_header(build, time_date_stamp, size_of_image)
        .map_err(|e| format!("found {} (table {}) but its {e}", fmt_quad(file_version), build.table_name()))?;

    // Every address of this build, on the loaded image.
    let img = LiveImage { base: base.0 as usize, view };
    let pf: Preflight = exe_build::preflight(build, &img);
    if !pf.passed() {
        let first: Vec<&str> = pf.failures.iter().take(6).map(String::as_str).collect();
        return Err(format!(
            "found {} (table {}) but the address preflight failed {}/{} sites: {}",
            fmt_quad(file_version),
            build.table_name(),
            pf.failures.len(),
            pf.checked,
            first.join(" | ")
        ));
    }
    let _ = BUILD.set(build);
    Ok(GateInfo {
        file_version,
        product_version,
        lang,
        time_date_stamp,
        size_of_image,
        build,
        preflight: pf.summary(),
        inferred: pf.inferred.clone(),
        warnings: pf.warnings.clone(),
    })
}
