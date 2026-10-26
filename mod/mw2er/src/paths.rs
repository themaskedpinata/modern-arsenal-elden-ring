//! Where the DLL lives and where the project root is (docs/M2_DESIGN.md section 15.1).
//!
//! The DLL is staged to `<root>\build\natives\mw2er.dll`, so the root is `<dll_dir>\..\..`. The
//! root counts as valid only if `<root>\mw2er.toml` exists; otherwise the log goes next to the DLL
//! and config defaults are used. If the DLL cannot find its own absolute path at all, `resolve`
//! returns `None` and the boot thread stays inert (never a path relative to the process CWD).

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::core::PCWSTR;

#[derive(Clone, Debug)]
pub struct Paths {
    pub dll: PathBuf,
    pub dll_dir: PathBuf,
    pub root: PathBuf,
    pub root_valid: bool,
}

impl Paths {
    pub fn config_file(&self) -> PathBuf {
        self.root.join("mw2er.toml")
    }

    /// A path from the config, relative to the root (absolute paths are kept).
    pub fn rel(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) }
    }

    /// The log file: `<root>\<log_dir>\mw2er.log`, or `<dll_dir>\mw2er.log` without a valid root.
    pub fn log_file(&self, log_dir: &str) -> PathBuf {
        if self.root_valid { self.rel(log_dir).join("mw2er.log") } else { self.dll_dir.join("mw2er.log") }
    }
}

fn module_file_name(module: HMODULE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32_768];
    // SAFETY: the buffer is valid for its length; the handle is our own module (or null = exe).
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// Our own module handle from the address of one of our functions (fallback when DllMain's
/// handle is unavailable).
fn own_module() -> Option<HMODULE> {
    let mut h = HMODULE::default();
    let addr = own_module as *const c_void;
    // SAFETY: FROM_ADDRESS takes any address inside the module; UNCHANGED_REFCOUNT adds no ref.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(addr as *const u16),
            &mut h,
        )
        .ok()?;
    }
    Some(h)
}

/// Resolves the DLL path from the handle DllMain received.
///
/// `None` when the DLL cannot find its own absolute path. The caller then stays inert: every file
/// the DLL writes (the log) is derived from this path, and a relative fallback would resolve against
/// the process working directory, which for eldenring.exe is likely the game folder.
pub fn resolve(hmodule_raw: usize) -> Option<Paths> {
    let dll = (hmodule_raw != 0)
        .then(|| module_file_name(HMODULE(hmodule_raw as *mut c_void)))
        .flatten()
        .or_else(|| own_module().and_then(module_file_name))?;
    if !dll.is_absolute() {
        return None;
    }
    let dll_dir = dll.parent().map(Path::to_path_buf)?;
    let root = dll_dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| dll_dir.clone());
    let root_valid = root.join("mw2er.toml").is_file();
    Some(Paths { dll, dll_dir, root, root_valid })
}
