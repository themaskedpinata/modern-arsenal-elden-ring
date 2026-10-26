//! Windows resources of `MW2inER-setup.exe`, written as a `.res` file and handed to the linker (link.exe and
//! lld-link take `.res` inputs), so no `rc.exe` / Windows SDK and no build crate is needed:
//! - the app icon (group 1; 16-64 px images drawn by `src/icon_art.rs`, the same art as the window icon), used by
//!   Explorer, the shortcuts and Apps & Features;
//! - a version-info block (FileVersion / ProductVersion = the workspace version; docs/INSTALLER_DESIGN.md 2.10:
//!   antivirus heuristics like a version block);
//! - an application manifest: `asInvoker` (no admin rights, no UAC prompt; the "setup" in the file name never
//!   triggers installer detection), Windows 10/11, per-monitor DPI, long paths, common controls 6 (message boxes).
//!
//! Only the binary gets them (`rustc-link-arg-bins`); the library and the tests are unchanged.

#[path = "src/icon_art.rs"]
mod icon_art;

use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const LANG_EN_US: u16 = 0x0409;
const ICON_SIZES: [u32; 7] = [16, 20, 24, 32, 40, 48, 64];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("mw2er_setup.res");
    std::fs::write(&out, res_file(&version)).expect("write the .res file");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn res_file(version: &str) -> Vec<u8> {
    let mut r = Vec::new();
    entry(&mut r, 0, 0, 0, 0, &[]); // the empty first entry of every .res file
    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes()); // reserved
    group.extend(1u16.to_le_bytes()); // type: icon
    group.extend((ICON_SIZES.len() as u16).to_le_bytes());
    for (i, &s) in ICON_SIZES.iter().enumerate() {
        let img = dib(s, &icon_art::rgba(s));
        let id = i as u16 + 1;
        entry(&mut r, RT_ICON, id, LANG_EN_US, 0x1010, &img);
        group.push(s as u8); // width (0 would mean 256)
        group.push(s as u8);
        group.push(0); // colour count
        group.push(0); // reserved
        group.extend(1u16.to_le_bytes()); // planes
        group.extend(32u16.to_le_bytes()); // bits per pixel
        group.extend((img.len() as u32).to_le_bytes());
        group.extend(id.to_le_bytes());
    }
    entry(&mut r, RT_GROUP_ICON, 1, LANG_EN_US, 0x1030, &group);
    entry(&mut r, RT_VERSION, 1, LANG_EN_US, 0x0030, &version_info(version));
    entry(&mut r, RT_MANIFEST, 1, LANG_EN_US, 0x0030, MANIFEST.as_bytes());
    r
}

/// One resource: a fixed 32-byte header (numeric type and name), the data, padding to 4.
fn entry(r: &mut Vec<u8>, ty: u16, name: u16, lang: u16, flags: u16, data: &[u8]) {
    r.extend((data.len() as u32).to_le_bytes()); // DataSize
    r.extend(32u32.to_le_bytes()); // HeaderSize
    r.extend(0xFFFFu16.to_le_bytes());
    r.extend(ty.to_le_bytes());
    r.extend(0xFFFFu16.to_le_bytes());
    r.extend(name.to_le_bytes());
    r.extend(0u32.to_le_bytes()); // DataVersion
    r.extend(flags.to_le_bytes()); // MemoryFlags
    r.extend(lang.to_le_bytes()); // LanguageId
    r.extend(0u32.to_le_bytes()); // Version
    r.extend(0u32.to_le_bytes()); // Characteristics
    r.extend_from_slice(data);
    while !r.len().is_multiple_of(4) {
        r.push(0);
    }
}

/// An icon image as a 32-bit DIB: BITMAPINFOHEADER (height doubled for the AND mask), BGRA rows bottom-up, an all-zero
/// 1-bit AND mask (transparency comes from the alpha channel).
fn dib(s: u32, rgba: &[u8]) -> Vec<u8> {
    let mask_row = s.div_ceil(32) * 4;
    let mut b = Vec::new();
    b.extend(40u32.to_le_bytes());
    b.extend((s as i32).to_le_bytes());
    b.extend((2 * s as i32).to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(32u16.to_le_bytes());
    b.extend(0u32.to_le_bytes()); // BI_RGB
    b.extend((s * s * 4 + mask_row * s).to_le_bytes());
    b.extend([0u8; 16]); // resolution, colours used / important
    for y in (0..s).rev() {
        for x in 0..s {
            let i = ((y * s + x) * 4) as usize;
            b.extend([rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    b.extend(std::iter::repeat_n(0u8, (mask_row * s) as usize));
    b
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

fn pad4(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
}

/// A version-info node: wLength, wValueLength, wType, key, padding, value, then the children (each 4-aligned).
fn node(key: &str, text: bool, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend(value_len.to_le_bytes());
    b.extend(u16::from(text).to_le_bytes());
    b.extend(utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let n = b.len() as u16;
    b[..2].copy_from_slice(&n.to_le_bytes());
    b
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    node(key, true, &v, (v.len() / 2) as u16, &[])
}

fn version_info(version: &str) -> Vec<u8> {
    let mut n = version.split(['.', '-', '+']).map(|p| p.parse::<u16>().unwrap_or(0)).chain(std::iter::repeat(0));
    let (a, b, c) = (n.next().unwrap_or(0) as u32, n.next().unwrap_or(0) as u32, n.next().unwrap_or(0) as u32);
    let (ms, ls) = ((a << 16) | b, c << 16);
    let mut fixed = Vec::new();
    for v in [0xFEEF_04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend(v.to_le_bytes());
    }
    let four = format!("{a}.{b}.{c}.0");
    let strings = [
        // P4: the display name of installer::DISPLAY_NAME / PUBLISHER (internal names stay MW2inER)
        string("CompanyName", "maskedpinata"),
        string("FileDescription", "Modern Arsenal for Elden Ring setup and launcher"),
        string("FileVersion", &four),
        string("InternalName", "MW2inER-setup"),
        string("LegalCopyright", "Copyright (c) 2026 maskedpinata. MIT License"),
        string("OriginalFilename", "MW2inER-setup.exe"),
        string("ProductName", "Modern Arsenal for Elden Ring"),
        string("ProductVersion", version),
    ];
    let table = node("040904B0", true, &[], 0, &strings);
    let sfi = node("StringFileInfo", true, &[], 0, &[table]);
    let mut tr = Vec::new();
    tr.extend(LANG_EN_US.to_le_bytes());
    tr.extend(1200u16.to_le_bytes()); // Unicode
    let var = node("Translation", false, &tr, 4, &[]);
    let vfi = node("VarFileInfo", true, &[], 0, &[var]);
    node("VS_VERSION_INFO", false, &fixed, fixed.len() as u16, &[sfi, vfi])
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <description>Modern Arsenal for Elden Ring setup and launcher</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
