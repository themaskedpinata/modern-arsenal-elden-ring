//! Every Win32 call of the installer, in one place (the rest of the crate is plain Rust over the file system).
//!
//! - HKCU / HKLM string values: read, write, delete a key tree, list subkeys (Steam path, me3 uninstall entry,
//!   `HKCU\Software\MW2inER`, the Apps & Features entry). Writes are HKCU only (no admin rights).
//! - Known folders (Desktop, Downloads, Documents, Start menu programs) through `SHGetKnownFolderPath`.
//! - Process list (is `steam.exe` / `eldenring.exe` running?) through ToolHelp.
//! - Free space on the drive of a path.
//! - PE version strings (FileVersion / ProductVersion) through pelite (read-only file mapping; no code is run).
//! - Masked console line input (the co-op password; echo off, never printed).
//! - `.lnk` shortcuts through `IShellLinkW` + `IPersistFile`.
//! - The wizard / launcher bits: own-console detection and detach, `MessageBoxW`, `ShellExecuteW` (https / Steam store
//!   pages / folders only), the system open dialog (`IFileOpenDialog`) for folders and files.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_DWORD,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// Registry hive (HKLM is read-only for this program).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

fn hkey(h: Hive) -> HKEY {
    match h {
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
    }
}

/// A REG_SZ value, `None` if the key or value is missing or not a string.
pub fn reg_get_string(hive: Hive, subkey: &str, value: &str) -> Option<String> {
    let sk = HSTRING::from(subkey);
    let vn = HSTRING::from(value);
    let mut len: u32 = 0;
    // SAFETY: size query with valid wide strings; no buffer.
    let r = unsafe { RegGetValueW(hkey(hive), &sk, &vn, RRF_RT_REG_SZ, None, None, Some(&mut len)) };
    if r != ERROR_SUCCESS || len < 2 {
        return None;
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: the buffer is valid for `bytes` bytes.
    let r = unsafe {
        RegGetValueW(hkey(hive), &sk, &vn, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes))
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The names of the subkeys of `subkey` (empty when missing).
pub fn reg_subkeys(hive: Hive, subkey: &str) -> Vec<String> {
    let sk = HSTRING::from(subkey);
    let mut h = HKEY::default();
    // SAFETY: valid strings and out pointer.
    if unsafe { RegOpenKeyExW(hkey(hive), &sk, Some(0), KEY_READ, &mut h) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = vec![0u16; 512];
        let mut n = name.len() as u32;
        // SAFETY: the name buffer is valid for n characters.
        let r = unsafe { RegEnumKeyExW(h, i, Some(PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None) };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..n as usize]));
    }
    // SAFETY: h was opened above.
    unsafe {
        let _ = RegCloseKey(h);
    }
    out
}

/// A registry value to write.
pub enum RegValue<'a> {
    Str(&'a str),
    Dword(u32),
}

/// Creates `HKCU\<subkey>` if needed and sets the values. HKCU only.
pub fn reg_set_hkcu(subkey: &str, values: &[(&str, RegValue)]) -> std::io::Result<()> {
    let sk = HSTRING::from(subkey);
    let mut h = HKEY::default();
    // SAFETY: valid strings and out pointer.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &sk,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut h,
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(std::io::Error::other(format!("cannot create HKCU\\{subkey} ({r:?})")));
    }
    let mut result = Ok(());
    for (name, v) in values {
        let n = HSTRING::from(*name);
        let (ty, data): (_, Vec<u8>) = match v {
            RegValue::Str(s) => {
                let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                (REG_SZ, w.iter().flat_map(|c| c.to_le_bytes()).collect())
            }
            RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
        };
        // SAFETY: h is open for writing; data is a valid byte slice.
        let r = unsafe { RegSetValueExW(h, &n, None, ty, Some(&data)) };
        if r != ERROR_SUCCESS {
            result = Err(std::io::Error::other(format!("cannot set HKCU\\{subkey}\\{name} ({r:?})")));
            break;
        }
    }
    // SAFETY: h was created above.
    unsafe {
        let _ = RegCloseKey(h);
    }
    result
}

/// Deletes `HKCU\<subkey>` and everything under it (Ok when it does not exist). HKCU only.
pub fn reg_delete_tree_hkcu(subkey: &str) -> std::io::Result<()> {
    if subkey.trim_matches('\\').is_empty() {
        return Err(std::io::Error::other("refusing to delete an empty HKCU subkey"));
    }
    let sk = HSTRING::from(subkey);
    // SAFETY: valid string.
    let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &sk) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("cannot delete HKCU\\{subkey} ({r:?})")))
    }
}

/// A shell known folder.
#[derive(Clone, Copy, Debug)]
pub enum Known {
    Desktop,
    Downloads,
    Documents,
    Programs,
}

/// The path of a known folder (honours OneDrive / relocated folders).
pub fn known_folder(k: Known) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Programs, KF_FLAG_DEFAULT,
        SHGetKnownFolderPath,
    };
    let id = match k {
        Known::Desktop => &FOLDERID_Desktop,
        Known::Downloads => &FOLDERID_Downloads,
        Known::Documents => &FOLDERID_Documents,
        Known::Programs => &FOLDERID_Programs,
    };
    // SAFETY: valid GUID; the returned string is freed with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Lowercase exe names of every running process.
pub fn running_processes() -> Vec<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: snapshot handle is closed below; the entry struct has its size set.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// True when a process with this exe name (case-insensitive, e.g. `steam.exe`) runs.
pub fn is_running(exe: &str) -> bool {
    let exe = exe.to_lowercase();
    running_processes().contains(&exe)
}

/// Free bytes for this user on the drive of `p` (the nearest existing ancestor is asked).
pub fn free_space(p: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut q = crate::util::normalize(p);
    while !q.exists() {
        if !q.pop() {
            return None;
        }
    }
    let mut free = 0u64;
    let s = HSTRING::from(q.as_os_str());
    // SAFETY: valid string, valid out pointer.
    unsafe { GetDiskFreeSpaceExW(&s, Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// Version strings of a PE file: the StringFileInfo `FileVersion` / `ProductVersion` (as Explorer and PowerShell's
/// VersionInfo show them), falling back to the fixed version numbers. Read through a read-only mapping.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeVersion {
    pub file_version: Option<String>,
    pub product_version: Option<String>,
}

pub fn pe_version(path: &Path) -> Option<PeVersion> {
    let map = pelite::FileMap::open(path).ok()?;
    let bytes = map.as_ref();
    if let Ok(pe) = pelite::pe64::PeFile::from_bytes(bytes) {
        use pelite::pe64::Pe;
        return version_from_resources(pe.resources().ok()?);
    }
    let pe = pelite::pe32::PeFile::from_bytes(bytes).ok()?;
    use pelite::pe32::Pe;
    version_from_resources(pe.resources().ok()?)
}

fn version_from_resources(res: pelite::resources::Resources<'_>) -> Option<PeVersion> {
    let vi = res.version_info().ok()?;
    let mut v = PeVersion::default();
    for lang in vi.translation() {
        if v.file_version.is_none() {
            v.file_version = vi.value(*lang, "FileVersion").map(|s| s.trim().trim_end_matches('\0').to_string());
        }
        if v.product_version.is_none() {
            v.product_version = vi.value(*lang, "ProductVersion").map(|s| s.trim().trim_end_matches('\0').to_string());
        }
    }
    if let Some(f) = vi.fixed() {
        let fmt = |x: &pelite::image::VS_VERSION| format!("{}.{}.{}.{}", x.Major, x.Minor, x.Patch, x.Build);
        v.file_version.get_or_insert_with(|| fmt(&f.dwFileVersion));
        v.product_version.get_or_insert_with(|| fmt(&f.dwProductVersion));
    }
    Some(v)
}

/// Reads one line from the console with echo off (stdin redirected: a plain line). The caller wraps it in a
/// `Secret` at once; nothing here prints or keeps it.
pub fn read_line_masked() -> std::io::Result<String> {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    let mut line = String::new();
    // SAFETY: console handle queries; the old mode is restored before returning.
    unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE).ok();
        let mut old = CONSOLE_MODE::default();
        let console = h.is_some_and(|h| GetConsoleMode(h, &mut old).is_ok());
        if let (true, Some(h)) = (console, h) {
            let _ = SetConsoleMode(h, old & !ENABLE_ECHO_INPUT);
        }
        let r = std::io::stdin().read_line(&mut line);
        if let (true, Some(h)) = (console, h) {
            let _ = SetConsoleMode(h, old);
            eprintln!();
        }
        r?;
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

static CTRL_C: std::sync::OnceLock<&'static std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();

/// Ctrl+C / Ctrl+Break in the console set `flag` instead of killing this process, so an install stops cleanly
/// (the running tool's process tree is killed, the state is saved, the next run resumes). First flag wins.
pub fn on_ctrl_c(flag: &'static std::sync::atomic::AtomicBool) -> bool {
    unsafe extern "system" fn handler(ctrl: u32) -> windows::core::BOOL {
        // CTRL_C_EVENT = 0, CTRL_BREAK_EVENT = 1; close / logoff / shutdown keep the default handling
        if ctrl <= 1
            && let Some(f) = CTRL_C.get()
        {
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            return windows::core::BOOL(1);
        }
        windows::core::BOOL(0)
    }
    let _ = CTRL_C.set(flag);
    // SAFETY: registers a handler with a static lifetime; it only touches an atomic.
    unsafe { windows::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), true).is_ok() }
}

/// A `.lnk` shortcut to write.
#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    pub lnk: PathBuf,
    pub target: PathBuf,
    pub args: String,
    pub workdir: PathBuf,
    pub description: String,
    /// `path,index`; empty = the target's own icon.
    pub icon: Option<(PathBuf, i32)>,
}

/// Writes the shortcut through `IShellLinkW` (COM apartment initialised for the call).
pub fn write_shortcut(s: &Shortcut) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;
    if let Some(d) = s.lnk.parent() {
        std::fs::create_dir_all(d).map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
    }
    // SAFETY: COM initialised for this thread for the duration; all strings outlive the calls.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = (|| {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(s.target.as_os_str()))?;
            link.SetArguments(&HSTRING::from(s.args.as_str()))?;
            link.SetWorkingDirectory(&HSTRING::from(s.workdir.as_os_str()))?;
            link.SetDescription(&HSTRING::from(s.description.as_str()))?;
            if let Some((icon, idx)) = &s.icon {
                link.SetIconLocation(&HSTRING::from(icon.as_os_str()), *idx)?;
            }
            let pf: IPersistFile = link.cast()?;
            pf.Save(&HSTRING::from(s.lnk.as_os_str()), true)
        })();
        if init {
            CoUninitialize();
        }
        r
    }
}

// ---- the wizard's and the launcher's small Win32 needs ----------------------------------------------------------

/// True when this process is the only one attached to its console, i.e. Windows made the console for it
/// (double-click in Explorer, a shortcut, Apps & Features). Started from cmd / PowerShell the shell shares it.
pub fn owns_console() -> bool {
    use windows::Win32::System::Console::GetConsoleProcessList;
    let mut ids = [0u32; 4];
    // SAFETY: valid buffer; 0 = no console at all.
    let n = unsafe { GetConsoleProcessList(&mut ids) };
    n == 1
}

/// Hides and leaves the console Windows made for this process (the wizard / launcher run without it).
pub fn detach_own_console() {
    use windows::Win32::System::Console::{FreeConsole, GetConsoleWindow};
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    // SAFETY: plain console calls on this process' own console.
    unsafe {
        let w = GetConsoleWindow();
        if !w.is_invalid() {
            let _ = ShowWindow(w, SW_HIDE);
        }
        let _ = FreeConsole();
    }
}

/// Opens a new console for this process (the console fallback when the setup window cannot open).
pub fn alloc_console() -> bool {
    // SAFETY: no arguments.
    unsafe { windows::Win32::System::Console::AllocConsole().is_ok() }
}

/// The buttons of a [`message_box`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    OkCancel,
    RetryCancel,
}

/// The icon of a [`message_box`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxIcon {
    Info,
    Warning,
    Error,
}

/// A modal `MessageBoxW` (owner: this thread's active window, if any). True = OK / Retry.
pub fn message_box(title: &str, text: &str, buttons: Buttons, icon: BoxIcon) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDOK, IDRETRY, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_OKCANCEL, MB_RETRYCANCEL,
        MB_SETFOREGROUND, MessageBoxW,
    };
    let b = match buttons {
        Buttons::Ok => MB_OK,
        Buttons::OkCancel => MB_OKCANCEL,
        Buttons::RetryCancel => MB_RETRYCANCEL,
    };
    let i = match icon {
        BoxIcon::Info => MB_ICONINFORMATION,
        BoxIcon::Warning => MB_ICONWARNING,
        BoxIcon::Error => MB_ICONERROR,
    };
    let owner = active_window();
    // SAFETY: valid wide strings that outlive the call.
    let r = unsafe { MessageBoxW(owner, &HSTRING::from(text), &HSTRING::from(title), b | i | MB_SETFOREGROUND) };
    r == IDOK || r == IDRETRY
}

fn active_window() -> Option<windows::Win32::Foundation::HWND> {
    // SAFETY: no arguments; a null handle means none.
    let h = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() };
    (!h.is_invalid()).then_some(h)
}

/// What [`shell_open`] may open: an https page, a `steam://nav/` page (only on the player's click; it never runs a
/// game), or an existing local folder (Explorer).
pub fn may_open(target: &str) -> bool {
    let t = target.trim();
    (t.starts_with("https://") && !t.contains(char::is_whitespace))
        || (t.starts_with("steam://nav/") && t.chars().all(|c| c.is_ascii_alphanumeric() || "/:_-".contains(c)))
        || Path::new(t).is_dir()
}

/// Opens a web page in the default browser or a folder in Explorer (ShellExecuteW "open"); refuses anything
/// [`may_open`] does not allow (never a program).
pub fn shell_open(target: &str) -> std::io::Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !may_open(target) {
        return Err(std::io::Error::other(format!("not opened: {target}")));
    }
    // SAFETY: valid wide strings; the returned pseudo-HINSTANCE > 32 means success.
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(target.trim()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as usize > 32 { Ok(()) } else { Err(std::io::Error::other(format!("cannot open {target} ({})", r.0 as usize))) }
}

/// A file type filter of [`pick_file`]: (label, `*.zip;*.dll`).
pub type Filter<'a> = (&'a str, &'a str);

/// The system "open" dialog (IFileOpenDialog): a folder (`filters` empty, `folders` true) or a file. None =
/// cancelled. Modal to this thread's active window. Only returns a path; nothing is opened or run.
pub fn pick(title: &str, folders: bool, filters: &[Filter], start: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{
        FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    let names: Vec<(HSTRING, HSTRING)> = filters.iter().map(|(n, s)| (HSTRING::from(*n), HSTRING::from(*s))).collect();
    let specs: Vec<COMDLG_FILTERSPEC> = names.iter().map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) }).collect();
    let owner = active_window();
    // SAFETY: COM initialised for the call (balanced); every string outlives the calls; the result string is freed.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = (|| -> windows::core::Result<PathBuf> {
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let mut o = d.GetOptions()? | FOS_FORCEFILESYSTEM;
            o |= if folders { FOS_PICKFOLDERS } else { FOS_FILEMUSTEXIST };
            d.SetOptions(o)?;
            d.SetTitle(&HSTRING::from(title))?;
            if !folders && !specs.is_empty() {
                d.SetFileTypes(&specs)?;
            }
            if let Some(s) = start.filter(|s| s.is_dir())
                && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(s.as_os_str()), None)
            {
                let _ = d.SetFolder(&item);
            }
            d.Show(owner)?;
            let p = d.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let s = p.to_string();
            CoTaskMemFree(Some(p.0 as *const _));
            Ok(PathBuf::from(s.map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?))
        })();
        if init {
            CoUninitialize();
        }
        r.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn open_allow_list() {
        assert!(may_open("https://www.nexusmods.com/eldenring/mods/510?tab=files"));
        assert!(may_open("steam://nav/games/details/1245620"));
        assert!(!may_open("steam://run/1245620"), "never a game start");
        assert!(!may_open("steam://rungameid/1245620"));
        assert!(!may_open("http://example.com"));
        assert!(!may_open(r"C:\Windows\System32\cmd.exe"));
        assert!(!may_open("https://x y"));
        let t = TempDir::new("open");
        assert!(may_open(&t.0.display().to_string()));
        assert!(shell_open("file:///C:/Windows/notepad.exe").is_err());
    }

    #[test]
    fn pe_version_of_a_system_dll() {
        // read-only: a Windows system file, never a game file
        let k = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
            .join(r"System32\kernel32.dll");
        let v = pe_version(&k).expect("kernel32 has a version resource");
        let f = v.file_version.unwrap();
        assert!(f.split(['.', ' ']).next().unwrap().parse::<u32>().unwrap() >= 6, "{f}");
        let t = TempDir::new("pever");
        assert_eq!(pe_version(&t.write("x.exe", b"not a PE")), None);
    }

    #[test]
    fn processes_and_free_space() {
        let p = running_processes();
        assert!(!p.is_empty());
        assert!(p.iter().all(|n| *n == n.to_lowercase()));
        let t = TempDir::new("free");
        assert!(free_space(&t.0.join("not/yet/there")).unwrap() > 0);
    }

    #[test]
    fn known_folders_resolve() {
        assert!(known_folder(Known::Desktop).is_some());
        assert!(known_folder(Known::Programs).is_some());
    }

    #[test]
    fn shortcut_is_written() {
        let t = TempDir::new("lnk");
        let s = Shortcut {
            lnk: t.0.join("sub/Test.lnk"),
            target: t.0.join("MW2inER.exe"),
            args: "play solo".into(),
            workdir: t.0.clone(),
            description: "test".into(),
            icon: None,
        };
        write_shortcut(&s).unwrap();
        let b = std::fs::read(&s.lnk).unwrap();
        assert_eq!(&b[..4], &[0x4c, 0, 0, 0]); // ShellLinkHeader size
    }

    #[test]
    fn registry_reads_are_tolerant() {
        assert_eq!(reg_get_string(Hive::CurrentUser, r"Software\MW2inER-test-does-not-exist", "x"), None);
        assert!(reg_subkeys(Hive::CurrentUser, r"Software\MW2inER-test-does-not-exist").is_empty());
        assert!(reg_delete_tree_hkcu("").is_err());
    }
}
