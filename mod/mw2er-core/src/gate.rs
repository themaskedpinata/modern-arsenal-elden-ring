//! Version gate decision (MODDING_PLAN safety rule 4; docs/M2_DESIGN.md section 6; port 2.7.2.0, MODLOG 2026-10-20).
//!
//! The DLL reads the host exe's path and version resource into plain values and asks this module
//! whether it may run. Anything but `eldenring.exe` with FileVersion == ProductVersion == one of
//! [`SUPPORTED`] (2.7.1.0, 2.7.2.0) and an English version resource keeps it inert. After this pure decision
//! the DLL still runs fromsoftware-rs's own detection and the build's address preflight
//! ([`crate::exe_build::preflight`]); any failure there keeps it inert too.

use crate::exe_build::ExeBuild;

/// A four-part version `major.minor.patch.build`.
pub type Quad = (u16, u16, u16, u16);

/// The supported builds: exe FileVersion 2.7.1.0 (Elden Ring 1.17.1, Steam build 25080141) and 2.7.2.0 (Steam
/// build 25827938, 2026-10-20). Each has its own fromsoftware-rs table and its own address table.
pub const SUPPORTED: [Quad; 2] = [ExeBuild::Ww2710.version(), ExeBuild::Ww2720.version()];
pub const WANT_EXE: &str = "eldenring.exe";
/// Primary language id of the version resource fromsoftware-rs's WW tables accept (English).
pub const WANT_PRIMARY_LANG: u16 = 0x0009;

pub fn fmt_quad(v: Quad) -> String {
    format!("{}.{}.{}.{}", v.0, v.1, v.2, v.3)
}

/// `2.7.1.0 or 2.7.2.0`
pub fn fmt_supported() -> String {
    SUPPORTED.iter().map(|v| fmt_quad(*v)).collect::<Vec<_>>().join(" or ")
}

/// File name part of a Windows path (`\` or `/` separated).
pub fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or("")
}

/// First half of the gate: the host process is `eldenring.exe` (checked before its image is parsed).
pub fn check_exe(exe_path: &str) -> Result<(), String> {
    let name = exe_name(exe_path);
    if name.eq_ignore_ascii_case(WANT_EXE) { Ok(()) } else { Err(format!("host is {name:?} ({exe_path}), not {WANT_EXE}")) }
}

/// The gate. `Ok` = the build whose tables apply; `Err` is the message logged as `VERSION FAIL <msg>; staying inert`.
pub fn decide(exe_path: &str, file_version: Quad, product_version: Quad, lang_id: u16) -> Result<ExeBuild, String> {
    check_exe(exe_path)?;
    let Some(build) = ExeBuild::from_version(file_version) else {
        return Err(format!("found {} (want {})", fmt_quad(file_version), fmt_supported()));
    };
    if product_version != file_version {
        return Err(format!(
            "file version {} but product version {} (want {})",
            fmt_quad(file_version),
            fmt_quad(product_version),
            fmt_quad(file_version)
        ));
    }
    if lang_id & 0x03FF != WANT_PRIMARY_LANG {
        return Err(format!("version resource language 0x{lang_id:04x} (want English, primary 0x{WANT_PRIMARY_LANG:02x})"));
    }
    Ok(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe";

    #[test]
    fn gate_passes_only_2710_and_2720() {
        assert_eq!(decide(EXE, (2, 7, 1, 0), (2, 7, 1, 0), 0x0409), Ok(ExeBuild::Ww2710));
        assert_eq!(decide(EXE, (2, 7, 2, 0), (2, 7, 2, 0), 0x0409), Ok(ExeBuild::Ww2720));
        assert_eq!(decide(r"C:\x\ELDENRING.EXE", (2, 7, 2, 0), (2, 7, 2, 0), 0x0409), Ok(ExeBuild::Ww2720), "case-insensitive name");
        assert_eq!(decide("D:/games/eldenring.exe", (2, 7, 1, 0), (2, 7, 1, 0), 0x0009), Ok(ExeBuild::Ww2710));
    }

    #[test]
    fn gate_rejects_other_versions() {
        assert!(decide(EXE, (2, 7, 0, 0), (2, 7, 0, 0), 0x0409).unwrap_err().contains("found 2.7.0.0 (want 2.7.1.0 or 2.7.2.0)"));
        assert!(decide(EXE, (2, 7, 1, 1), (2, 7, 1, 1), 0x0409).unwrap_err().contains("found 2.7.1.1"));
        assert!(decide(EXE, (2, 7, 2, 1), (2, 7, 2, 1), 0x0409).unwrap_err().contains("found 2.7.2.1"));
        assert!(decide(EXE, (2, 7, 3, 0), (2, 7, 3, 0), 0x0409).is_err());
        assert!(decide(EXE, (2, 6, 1, 0), (2, 7, 1, 0), 0x0409).is_err());
        assert!(decide(EXE, (2, 7, 1, 0), (2, 7, 1, 1), 0x0409).unwrap_err().contains("product"));
        assert!(decide(EXE, (2, 7, 2, 0), (2, 7, 1, 0), 0x0409).unwrap_err().contains("product"), "mixed 2.7.2.0 / 2.7.1.0");
        assert!(decide(EXE, (2, 7, 1, 0), (2, 7, 2, 0), 0x0409).unwrap_err().contains("product"), "mixed 2.7.1.0 / 2.7.2.0");
        assert!(decide(EXE, (0, 0, 0, 0), (0, 0, 0, 0), 0).is_err());
    }

    #[test]
    fn gate_rejects_other_hosts_and_languages() {
        assert!(decide(r"C:\games\DarkSoulsIII.exe", (2, 7, 1, 0), (2, 7, 1, 0), 0x0409).unwrap_err().contains("host"));
        assert!(decide(r"C:\me3\me3-launcher.exe", (2, 7, 2, 0), (2, 7, 2, 0), 0x0409).is_err());
        assert!(decide(r"C:\x\eldenring.exe.bak", (2, 7, 1, 0), (2, 7, 1, 0), 0x0409).is_err());
        assert!(decide("", (2, 7, 1, 0), (2, 7, 1, 0), 0x0409).is_err());
        assert!(decide(EXE, (2, 7, 1, 0), (2, 7, 1, 0), 0x0411).unwrap_err().contains("language"), "Japanese resource");
        assert!(decide(EXE, (2, 7, 2, 0), (2, 7, 2, 0), 0x0411).unwrap_err().contains("language"), "Japanese resource");
        assert_eq!(exe_name(r"a\b/c.exe"), "c.exe");
        assert_eq!(fmt_supported(), "2.7.1.0 or 2.7.2.0");
    }
}
