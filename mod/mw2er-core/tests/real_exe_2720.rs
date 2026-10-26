//! Port 2.7.2.0 (check_c3.ps1 check 15): the boot preflight run against the user's eldenring.exe 2.7.2.0 FILE mapped
//! section by section the way the loader maps it (read only; never launched). The 2.7.2.0 table must pass every site,
//! the 2.7.1.0 table must fail on it, and single-byte / single-address mutations must fail.
//! Ignored by default (it needs the game exe); run with
//! `cargo test -p mw2er-core --test real_exe_2720 -- --include-ignored` (`MW2ER_ER_EXE` overrides the path).
//! A missing or different exe prints `SKIP:` and returns, unless `MW2ER_REQUIRE_EXE=1` (check_c3 sets it), which turns
//! the SKIP into a test failure.

use mw2er_core::exe_build::{Check, ExeBuild, ImageView, check_header, check_site, preflight, site};
use mw2er_core::exe_sites::EXE_2720_SHA256;
use mw2er_core::sha256;

const DEFAULT_EXE: &str = r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe";

struct Sec {
    va: u32,
    vsize: u32,
    flags: u32,
}

/// The exe file mapped as an image at its preferred base (sections copied to their RVAs).
struct Mapped {
    mem: Vec<u8>,
    base: u64,
    secs: Vec<Sec>,
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn map(file: &[u8]) -> Mapped {
    let pe = u32_at(file, 0x3c) as usize;
    assert_eq!(&file[pe..pe + 4], b"PE\0\0");
    let nsec = u16_at(file, pe + 6) as usize;
    let opt = pe + 24;
    let so = u16_at(file, pe + 20) as usize;
    let base = u64::from_le_bytes(file[opt + 24..opt + 32].try_into().unwrap());
    let size = u32_at(file, opt + 56) as usize;
    let headers = u32_at(file, opt + 60) as usize;
    let mut mem = vec![0u8; size];
    mem[..headers].copy_from_slice(&file[..headers]);
    let mut secs = Vec::new();
    for i in 0..nsec {
        let o = opt + so + 40 * i;
        let (vsize, va, rsize, raw) = (u32_at(file, o + 8), u32_at(file, o + 12), u32_at(file, o + 16), u32_at(file, o + 20));
        let n = rsize.min(vsize.max(rsize)) as usize;
        let n = n.min(size - va as usize).min(file.len() - raw as usize);
        mem[va as usize..va as usize + n].copy_from_slice(&file[raw as usize..raw as usize + n]);
        secs.push(Sec { va, vsize: vsize.max(rsize), flags: u32_at(file, o + 36) });
    }
    Mapped { mem, base, secs }
}

impl ImageView for Mapped {
    fn base(&self) -> u64 {
        self.base
    }
    fn size_of_image(&self) -> u32 {
        self.mem.len() as u32
    }
    fn section_flags(&self, rva: u32) -> Option<u32> {
        self.secs.iter().find(|s| (s.va..s.va.saturating_add(s.vsize)).contains(&rva)).map(|s| s.flags)
    }
    fn read(&self, rva: u32, out: &mut [u8]) -> bool {
        let a = rva as usize;
        match self.mem.get(a..a.saturating_add(out.len())) {
            Some(s) => {
                out.copy_from_slice(s);
                true
            }
            None => false,
        }
    }
}

fn skip(msg: String) -> Option<Mapped> {
    assert!(std::env::var("MW2ER_REQUIRE_EXE").as_deref() != Ok("1"), "MW2ER_REQUIRE_EXE=1 but {msg}");
    println!("SKIP: {msg}");
    None
}

fn load() -> Option<(Mapped, u32)> {
    let p = std::env::var("MW2ER_ER_EXE").unwrap_or_else(|_| DEFAULT_EXE.to_string());
    let Ok(file) = std::fs::read(&p) else {
        skip(format!("{p} missing"));
        return None;
    };
    let sha = sha256::sha256_hex(&file);
    if sha != EXE_2720_SHA256 {
        skip(format!("{p} is sha256 {sha}, not the 2.7.2.0 file {EXE_2720_SHA256}"));
        return None;
    }
    let pe = u32_at(&file, 0x3c) as usize;
    Some((map(&file), u32_at(&file, pe + 8)))
}

#[test]
#[ignore = "needs the eldenring.exe 2.7.2.0 file"]
fn real_exe_2720_preflight() {
    let Some((mut img, tds)) = load() else { return };
    // the PE header pin of the build (the gate checks it before the sites)
    assert_eq!(check_header(ExeBuild::Ww2720, tds, img.size_of_image()), Ok(()));
    assert!(check_header(ExeBuild::Ww2710, tds, img.size_of_image()).is_err());
    println!("PASS 2.7.2.0 header timedatestamp=0x{tds:08x} sizeofimage=0x{:x}", img.size_of_image());
    let p = preflight(ExeBuild::Ww2720, &img);
    assert!(p.passed(), "2.7.2.0 preflight failures: {:?}", p.failures);
    assert!(p.warnings.is_empty(), "the exe file itself has no tolerated difference: {:?}", p.warnings);
    assert_eq!((p.checked, p.code, p.vmt), (113, 20, 83), "{}", p.summary());
    let mut inferred = p.inferred.clone();
    inferred.sort_unstable();
    assert_eq!(
        inferred,
        ["crypto_spi_registry", "cs_action_button_man_execute_action_button", "fd4param_repository_vmt", "obtain_dl_user_input_manager"]
    );
    println!("PASS 2.7.2.0 preflight on the exe file: {}", p.summary());

    // the 2.7.1.0 table must not pass on this exe (moved code / vtables are caught)
    let old = preflight(ExeBuild::Ww2710, &img);
    assert!(!old.passed());
    println!("2.7.1.0 table on the 2.7.2.0 exe: {} failures, e.g. {:?}", old.failures.len(), old.failures.first());
    assert!(old.failures.iter().any(|f| f.starts_with("scaleform_begin_scene")));
    assert!(old.failures.iter().any(|f| f.starts_with("cs_collision_filter_vmt")));
    assert!(old.failures.iter().any(|f| f.starts_with("chr_cam_vmt")));

    // mutations: one byte in cast_ray's window, one in the camera-step prologue, a vtable's COL pointer
    let mut caught = 0;
    for (name, off) in [("cs_phys_world_cast_ray", 3u32), ("camera_step", 9), ("chr_ins_apply_speffect", 0)] {
        let s = site(ExeBuild::Ww2720, name).unwrap();
        assert!(matches!(s.check, Check::Code(_)));
        let a = (s.rva + off) as usize;
        img.mem[a] ^= 0x01;
        let p = preflight(ExeBuild::Ww2720, &img);
        img.mem[a] ^= 0x01;
        assert!(!p.passed() && p.failures.len() == 1 && p.failures[0].starts_with(name), "{name}: {:?}", p.failures);
        caught += 1;
    }
    let v = site(ExeBuild::Ww2720, "chr_cam_vmt").unwrap();
    let a = (v.rva - 8) as usize;
    img.mem[a] ^= 0x10;
    assert!(check_site(&img, v).is_err(), "COL pointer moved");
    img.mem[a] ^= 0x10;
    caught += 1;
    let w = site(ExeBuild::Ww2720, "cam_wall_recovery").unwrap();
    let moved = mw2er_core::exe_build::Site { rva: 0x2a2ca18, ..*w };
    assert!(check_site(&img, &moved).is_err(), "a .data global moved into .rdata");
    caught += 1;
    // review 2026-10-20: a patched function mw2er never calls is a WARN, not inert
    let u = site(ExeBuild::Ww2720, "cs_ez_draw_draw_line").unwrap();
    let a = (u.rva + 2) as usize;
    img.mem[a] ^= 0x01;
    let p = preflight(ExeBuild::Ww2720, &img);
    img.mem[a] ^= 0x01;
    assert!(p.passed() && p.warnings.len() == 1 && p.warnings[0].starts_with("cs_ez_draw_draw_line"), "{p:?}");
    caught += 1;
    // another module's jmp rel32 detour over cast_ray (called by mw2er): WARN; over the camera-step hook: inert
    for (name, ok) in [("cs_phys_world_cast_ray", true), ("camera_step", false)] {
        let s = site(ExeBuild::Ww2720, name).unwrap();
        let a = s.rva as usize;
        let saved: Vec<u8> = img.mem[a..a + 5].to_vec();
        img.mem[a..a + 5].copy_from_slice(&[0xe9, 0x00, 0x10, 0x00, 0x00]);
        let p = preflight(ExeBuild::Ww2720, &img);
        img.mem[a..a + 5].copy_from_slice(&saved);
        assert_eq!(p.passed(), ok, "{name}: {p:?}");
        assert!(if ok { p.warnings[0].contains("detoured") } else { p.failures[0].starts_with(name) }, "{name}: {p:?}");
        caught += 1;
    }
    assert!(preflight(ExeBuild::Ww2720, &img).passed(), "restored image passes again");
    println!("PASS mutations caught: {caught}");
}

/// C6 part 16.3: the carrier's four functions (EquipItem / AddItem / CSGaitemImp::Create / Find) start with the pinned
/// bytes on the 2.7.2.0 exe file, and a one-byte change of each is caught (`HOLD xbow off why=prologue fn=..`).
#[test]
#[ignore = "needs the eldenring.exe 2.7.2.0 file"]
fn real_exe_2720_carrier_prologues() {
    use mw2er_core::carrier::{CARRIER_2720, validate};
    let Some((mut img, _)) = load() else { return };
    let base = img.base as usize;
    let a = CARRIER_2720;
    let check = |img: &Mapped| validate(&a, base, &|addr: usize, out: &mut [u8]| img.read((addr - base) as u32, out));
    assert_eq!(check(&img), Ok(()));
    for (name, rva) in [("equip_item", a.equip_item), ("add_item", a.add_item), ("create", a.create), ("find", a.find)] {
        img.mem[rva + 1] ^= 0x01;
        assert_eq!(check(&img), Err(name));
        img.mem[rva + 1] ^= 0x01;
    }
    // the GameDataMan / CSGaitemImp globals are in a writable data section
    for g in [a.game_data_man, a.gaitem_imp] {
        let f = img.section_flags(g as u32).expect("in a section");
        assert!(f & 0x8000_0000 != 0 && f & 0x2000_0000 == 0, "0x{g:x}: flags 0x{f:08x}");
    }
    println!("PASS carrier prologues + globals on the 2.7.2.0 exe file (4 functions, 4 mutations caught)");
}
