//! C4 (docs/C4_UI_DESIGN.md 7, check_c4.ps1 check 2): the C4 sites (`exe_build::scaleform_ui_sites`) checked with the
//! boot preflight's rules against the user's eldenring.exe 2.7.2.0 FILE mapped the way the loader maps it (read only;
//! never launched), plus single-byte mutations that must fail. The mapper is real_exe_2720.rs's.
//! Ignored by default (it needs the game exe); run with
//! `cargo test -p mw2er-core --test c4_real_exe -- --include-ignored` (`MW2ER_ER_EXE` overrides the path).
//! A missing or different exe prints `SKIP:` and returns, unless `MW2ER_REQUIRE_EXE=1` (check_c4 sets it), which turns
//! the SKIP into a test failure.
//! C4.1 (docs/C4_1_DESIGN.md 3.2): `c41_addrs_real_exe_2720` does the same for `exe_build::scaleform_ui41_sites`, the
//! vtable slots / RTTI locators the live chain pins compare, and one instruction per `Ui41` offset.

use mw2er_core::exe_build::{Check, ExeBuild, ImageView, SCALEFORM_UI_2720, Site, preflight_sites, scaleform_ui_sites, scaleform_ui41_sites};
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

/// UiBegin's instructions that pin `UiLayout` (design 7), as (VA, bytes); the same rows as c4_static.py.
const UI_BEGIN_PINS: [(u64, &[u8]); 12] = [
    (0x140d80909, &[0x48, 0x8b, 0x89, 0x68, 0x09, 0x00, 0x00]),
    (0x140d80915, &[0x48, 0x83, 0xbb, 0xe0, 0x0b, 0x00, 0x00, 0x00]),
    (0x140d80966, &[0x48, 0x8b, 0x88, 0x28, 0x01, 0x00, 0x00]),
    (0x140d8096d, &[0x80, 0xb9, 0x3c, 0x01, 0x00, 0x00, 0x00]),
    (0x140d80976, &[0x80, 0xb9, 0x3e, 0x01, 0x00, 0x00, 0x00]),
    (0x140d8097f, &[0x48, 0x8b, 0x81, 0x00, 0x01, 0x00, 0x00]),
    (0x140d8098b, &[0x48, 0x8b, 0x81, 0xf0, 0x00, 0x00, 0x00]),
    (0x140d80992, &[0x48, 0x8d, 0x48, 0x70]),
    (0x140d809ef, &[0x48, 0x8b, 0x4b, 0x08, 0x48, 0x8b, 0x51, 0x20]),
    (0x140d80a13, &[0xc7, 0x44, 0x24, 0x34, 0xc0, 0x00, 0x00, 0x00, 0xc7, 0x44, 0x24, 0x38, 0x04, 0x00, 0x00, 0x00]),
    (0x140d80a30, &[0x48, 0x8b, 0x8f, 0x20, 0x14, 0x00, 0x00]),
    (0x140d80a52, &[0xff, 0x90, 0xd0, 0x00, 0x00, 0x00]),
];

#[test]
#[ignore = "needs the eldenring.exe 2.7.2.0 file"]
fn c4_addrs_real_exe_2720() {
    let Some((mut img, _)) = load() else { return };
    let sites = scaleform_ui_sites(ExeBuild::Ww2720);
    let p = preflight_sites(&sites, &img);
    assert!(p.passed() && p.warnings.is_empty(), "C4 sites on the exe file: {:?} {:?}", p.failures, p.warnings);
    assert_eq!((p.checked, p.code, p.vmt), (5, 2, 1), "{}", p.summary());
    println!("PASS 2.7.2.0 C4 sites on the exe file: {}", p.summary());
    // the layout pins inside UiBegin
    let rva = |va: u64| (va - img.base) as u32;
    for (va, want) in UI_BEGIN_PINS {
        let mut got = vec![0u8; want.len()];
        assert!(img.read(rva(va), &mut got) && got == want, "pin at 0x{va:x}: {got:02x?} != {want:02x?}");
    }
    // G is the rip-relative operand of `mov rax,[1447F6400h]` at 0x140d80958 (48 8b 05 rel32)
    let mut g = [0u8; 7];
    assert!(img.read(rva(0x140d80958), &mut g) && g[..3] == [0x48, 0x8b, 0x05]);
    let target = 0x140d8095fu64.wrapping_add(i32::from_le_bytes(g[3..7].try_into().unwrap()) as i64 as u64);
    assert_eq!(target, img.base + SCALEFORM_UI_2720.g_render as u64);
    println!("PASS 2.7.2.0 UiBegin layout pins: {} instructions + G 0x{target:x}", UI_BEGIN_PINS.len());
    // mutations: one byte of each hook window, a vtable / global moved into the wrong section kind
    let mut caught = 0;
    for (i, s) in sites.iter().enumerate() {
        let bad: Site = match s.check {
            Check::Code(_) => {
                let a = s.rva as usize + 6;
                img.mem[a] ^= 0x01;
                let r = !preflight_sites(&sites, &img).passed();
                img.mem[a] ^= 0x01;
                assert!(r, "a byte flip in {} passed", s.name);
                caught += 1;
                continue;
            }
            Check::VtableNoRtti => Site { rva: SCALEFORM_UI_2720.g_render as u32, ..*s },
            _ => Site { rva: SCALEFORM_UI_2720.ui_begin as u32, ..*s },
        };
        let mut m = sites.clone();
        m[i] = bad;
        assert!(!preflight_sites(&m, &img).passed(), "{} at a wrong address passed", s.name);
        caught += 1;
    }
    assert!(preflight_sites(&sites, &img).passed(), "the image was restored");
    println!("PASS C4 mutations caught: {caught}");
}

/// C4.1 (docs/C4_1_DESIGN.md 1.2 / 3.2): the instructions that pin every `Ui41` offset, as (VA, bytes).
const UI41_PINS: [(u64, &[u8]); 22] = [
    // 0: multithreaded mode `cmp byte ptr [rcx+11Ch],0` (frame end 0x1419ed8e0)
    (0x1419ed8ea, &[0x80, 0xb9, 0x1c, 0x01, 0x00, 0x00, 0x00]),
    // 1: the render thread id ER passes to InitHAL `mov rax,[rcx+6D0h]; mov edi,[rax+14h]`
    (0x140d7b777, &[0x48, 0x8b, 0x81, 0xd0, 0x06, 0x00, 0x00, 0x8b, 0x78, 0x14]),
    // 2: the getter's `cmp qword ptr [rcx-30h],0` (this = sel+0x70, so T = [sel+0x40])
    (0x1419f4974, &[0x48, 0x83, 0x79, 0xd0, 0x00]),
    // 3: slot 0's `cmp qword ptr [rcx+8],0` (arr = [sel+0x78])
    (0x1419f49af, &[0x48, 0x83, 0x79, 0x08, 0x00]),
    // 4: slot 0's `mov rax,[rcx-70h]; add rcx,-70h` (the primary object = sel)
    (0x1419f49be, &[0x48, 0x8b, 0x41, 0x90, 0x48, 0x83, 0xc1, 0x90]),
    // 5: slot 0's `call qword ptr [rax+38h]` (the count, primary slot 7)
    (0x1419f49c6, &[0xff, 0x50, 0x38]),
    // 6: slot 0's `mov rax,[rbx+8]; mov rcx,[rax+rdi*8]` (obj = [arr + 8*index])
    (0x1419f49cd, &[0x48, 0x8b, 0x43, 0x08, 0x48, 0x8b, 0x0c, 0xf8]),
    // 7: slot 7's `mov rcx,[rcx+40h]` (T)
    (0x1419f3dd9, &[0x48, 0x8b, 0x49, 0x40]),
    // 8: slot 7's `add rcx,20h` (the desc copy gets T+0x20)
    (0x1419f3de2, &[0x48, 0x83, 0xc1, 0x20]),
    // 9: slot 7's `mov eax,[rsp+28h]` (the count is the copied desc's +8; the copy is at rsp+20h)
    (0x1419f3deb, &[0x8b, 0x44, 0x24, 0x28]),
    // 10: the desc copy's `mov rax,[rcx+18h]` ([T+0x38])
    (0x141e8985d, &[0x48, 0x8b, 0x41, 0x18]),
    // 11: `mov rax,[rax+8]` (the desc data)
    (0x141e8986c, &[0x48, 0x8b, 0x40, 0x08]),
    // 12: the fallback `movzx eax,byte ptr [rdi+26h]` stored at desc+8 (u8 [T+0x46])
    (0x141e89949, &[0x0f, 0xb6, 0x47, 0x26]),
    // 13: HAL init `mov rdi,[r15+8]` (obj+8)
    (0x140d7b8e3, &[0x49, 0x8b, 0x7f, 0x08]),
    // 14: HAL init `lea rdx,[r15+10h]` (ER's RTV handle at obj+0x10)
    (0x140d7b8f3, &[0x49, 0x8d, 0x57, 0x10]),
    // 15: HAL init `mov r8,[rdi+20h]` (the resource)
    (0x140d7b920, &[0x4c, 0x8b, 0x47, 0x20]),
    // 16: HAL init `mov [r13+0DD0h],rbx` (owner+0xdd0 = the HAL render target)
    (0x140d7b942, &[0x49, 0x89, 0x9d, 0xd0, 0x0d, 0x00, 0x00]),
    // 17: FS CreateRenderTarget `mov rbx,[rcx+20h]` (rtData); 18: `mov [rbx+20h],ebp` (format); 19: `mov [rbx+28h],rax` (RTV)
    (0x140ebb17e, &[0x48, 0x8b, 0x59, 0x20]),
    (0x140ebb203, &[0x89, 0x6b, 0x20]),
    (0x140ebb239, &[0x48, 0x89, 0x43, 0x28]),
    // 20: the composite `mov rax,[rdi+740h]; cmp byte ptr [rax+8],0` (shaders ready)
    (0x1419ee9d9, &[0x48, 0x8b, 0x87, 0x40, 0x07, 0x00, 0x00, 0x80, 0x78, 0x08, 0x00]),
    // 21: the composite `cmp byte ptr [rdi+0EB9h],0`
    (0x1419eea95, &[0x80, 0xbf, 0xb9, 0x0e, 0x00, 0x00, 0x00]),
];

/// The render-scale compares of the composite selector 0x141afce70 (`comiss xmm0,[r14+0ED0h]` / `[r14+0ED4h]`).
const UI41_SCALE_PINS: [(u64, &[u8]); 2] =
    [(0x141afcfb1, &[0x41, 0x0f, 0x2f, 0x86, 0xd0, 0x0e, 0x00, 0x00]), (0x141afcfbb, &[0x41, 0x0f, 0x2f, 0x86, 0xd4, 0x0e, 0x00, 0x00])];

fn d32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

#[test]
#[ignore = "needs the eldenring.exe 2.7.2.0 file"]
fn c41_addrs_real_exe_2720() {
    let Some((mut img, _)) = load() else { return };
    let u = SCALEFORM_UI_2720.ui41;
    let l = SCALEFORM_UI_2720.layout;
    let sites = scaleform_ui41_sites(ExeBuild::Ww2720);
    let p = preflight_sites(&sites, &img);
    assert!(p.passed() && p.warnings.is_empty(), "C4.1 sites on the exe file: {:?} {:?}", p.failures, p.warnings);
    assert_eq!((p.checked, p.code, p.vmt), (5, 3, 2), "{}", p.summary());
    println!("PASS 2.7.2.0 C4.1 sites on the exe file: {}", p.summary());
    let rva = |va: u64| (va - img.base) as u32;
    let q = |img: &Mapped, r: usize| {
        let mut b = [0u8; 8];
        assert!(img.read(r as u32, &mut b));
        u64::from_le_bytes(b)
    };
    let va = |r: usize| img.base + r as u64;
    // the vtable slots the live pins compare (design 1.2 chain) and both RTTI locators
    assert_eq!(q(&img, u.sel_vt + u.sel_count_slot), va(u.sel_vt_count), "primary slot 7");
    assert_eq!(q(&img, u.sub_vt), va(u.sub_slot0), "secondary slot 0");
    assert_eq!(q(&img, u.sub_vt + 8), va(u.sub_slot1), "secondary slot 1 (the C4 getter)");
    assert_eq!(q(&img, u.sel_vt - 8), va(0x33b_4bb8), "primary COL");
    assert_eq!(q(&img, u.sub_vt - 8), va(0x33b_4cd8), "secondary COL (design: RTTI COL 0x1433B4CD8)");
    // every offset pin
    for (v, want) in UI41_PINS.iter().chain(UI41_SCALE_PINS.iter()) {
        let mut got = vec![0u8; want.len()];
        assert!(img.read(rva(*v), &mut got) && got == *want, "pin at 0x{v:x}: {got:02x?} != {want:02x?}");
    }
    let b = |i: usize| UI41_PINS[i].1;
    assert_eq!(d32(b(0), 2), u.g_mt);
    assert_eq!((d32(b(1), 3), b(1)[9] as usize), (u.g_rt, u.rt_tid));
    assert_eq!(l.obj as i64 + b(2)[3] as i8 as i64, u.sel_tex as i64, "getter this-0x30");
    assert_eq!(l.obj + b(3)[3] as usize, u.sel_arr, "slot 0 this+8");
    assert_eq!(b(4)[3] as i8 as i64, -(l.obj as i64), "slot 0 this-0x70 = sel");
    assert_eq!(b(5)[2] as usize, u.sel_count_slot);
    assert_eq!(b(7)[3] as usize, u.sel_tex);
    // the desc copy reads [rcx+0x18] with rcx = T+0x20, and the count is the copy's +8 (rsp+0x28 - rsp+0x20)
    assert_eq!(b(8)[3] as usize + b(10)[3] as usize, u.tex_desc);
    assert_eq!(b(11)[3] as usize, u.desc_data);
    assert_eq!(b(9)[3] as usize - 0x20, u.desc_count);
    assert_eq!(b(8)[3] as usize + b(12)[3] as usize, u.tex_count);
    assert_eq!((b(13)[3] as usize, b(15)[3] as usize, b(14)[3] as usize), (l.wrap, l.res, u.obj_rtv));
    assert_eq!(d32(b(16), 3), u.owner_rt);
    assert_eq!((b(17)[3] as usize, b(18)[2] as usize, b(19)[3] as usize), (u.rt_data, u.rtdata_fmt, u.rtdata_rtv));
    assert_eq!((d32(b(20), 3), b(20)[9] as usize), (u.g_shaders, u.shaders_ready));
    assert_eq!(d32(b(21), 2), u.g_eb9);
    assert_eq!((d32(UI41_SCALE_PINS[0].1, 4), d32(UI41_SCALE_PINS[1].1, 4)), (u.g_scale, u.g_scale + 4));
    println!("PASS 2.7.2.0 C4.1 offset pins: {} instructions + 5 vtable slots / COLs", UI41_PINS.len() + UI41_SCALE_PINS.len());
    // mutations: one byte of each code window, each vtable moved off its RTTI locator, each slot / COL value
    let mut caught = 0;
    for (i, s) in sites.iter().enumerate() {
        match s.check {
            Check::Code(_) => {
                let a = s.rva as usize + 6;
                img.mem[a] ^= 0x01;
                let r = !preflight_sites(&sites, &img).passed();
                img.mem[a] ^= 0x01;
                assert!(r, "a byte flip in {} passed", s.name);
            }
            _ => {
                let mut m = sites.clone();
                m[i] = Site { rva: s.rva + 8, ..*s };
                assert!(!preflight_sites(&m, &img).passed(), "{} one slot off passed", s.name);
                let a = s.rva as usize - 8;
                img.mem[a] ^= 0x08;
                let r = !preflight_sites(&sites, &img).passed();
                img.mem[a] ^= 0x08;
                assert!(r, "a moved COL pointer of {} passed", s.name);
                caught += 1;
            }
        }
        caught += 1;
    }
    assert!(preflight_sites(&sites, &img).passed(), "the image was restored");
    println!("PASS C4.1 mutations caught: {caught}");
}
/// C6 depth probe (work/c6/depth3.md 1): the chain's sites on the exe file, the instructions that pin its offsets, and
/// one mutation per site.
#[test]
#[ignore = "needs the eldenring.exe 2.7.2.0 file"]
fn c6_depth_addrs_real_exe_2720() {
    use mw2er_core::exe_build::er_depth_sites;
    let Some((mut img, _)) = load() else { return };
    let a = mw2er_core::er_depth::ER_DEPTH_2720;
    let sites = er_depth_sites(ExeBuild::Ww2720);
    let p = preflight_sites(&sites, &img);
    assert!(p.passed() && p.warnings.is_empty(), "depth sites on the exe file: {:?} {:?}", p.failures, p.warnings);
    assert_eq!((p.checked, p.code, p.vmt), (6, 1, 4), "{}", p.summary());
    let rva = |va: u64| (va - img.base) as u32;
    let rd = |img: &Mapped, va: u64, n: usize| {
        let mut b = vec![0u8; n];
        assert!(img.read(rva(va), &mut b));
        b
    };
    // `mov r8,[1447F67C8h]` at 0x141a1e843 (4c 8b 05 rel32): the RS global
    let m = rd(&img, 0x141a1e843, 7);
    assert_eq!(m[..3], [0x4c, 0x8b, 0x05]);
    let t = 0x141a1e84au64.wrapping_add(i32::from_le_bytes(m[3..7].try_into().unwrap()) as i64 as u64);
    assert_eq!(t, img.base + a.rs_global as u64);
    // the getter 0x141ae26d0: `cmp qword ptr [rcx+70h],0` / `call qword ptr [rax+38h]` / `mov rax,[rbx+70h]` / `mov rax,[rax+rdi*8]`
    for (va, want) in [
        (0x141ae26da_u64, vec![0x48, 0x83, 0x79, a.arr as u8, 0x00]),
        (0x141ae26e9, vec![0xff, 0x50, a.count_slot as u8]),
        (0x141ae26f0, vec![0x48, 0x8b, 0x43, a.arr as u8]),
        (0x141ae26f4, vec![0x48, 0x8b, 0x04, 0xf8]),
        // UiBegin's `mov rcx,[rbx+8]; mov rdx,[rcx+20h]` on the getter's result (wrap / res)
        (0x140d809ef, vec![0x48, 0x8b, 0x4b, a.obj_wrap as u8, 0x48, 0x8b, 0x51, a.wrap_res as u8]),
        // UiBegin's `mov rcx,[rdx+0E8h]` (S+0xE8 DefDepthStencil)
        (0x140d809af, vec![0x48, 0x8b, 0x8a, a.def_ds as u8, 0x00, 0x00, 0x00]),
    ] {
        assert_eq!(rd(&img, va, want.len()), want, "pin at 0x{va:x}");
    }
    // the dsb vtable's slot 7 is the count function
    let s7 = rd(&img, img.base + (a.dsb_vt + a.count_slot) as u64, 8);
    assert_eq!(u64::from_le_bytes(s7.try_into().unwrap()), img.base + a.count_fn as u64);
    println!("PASS 2.7.2.0 depth sites + pins: {}", p.summary());
    let mut caught = 0;
    for (i, s) in sites.iter().enumerate() {
        if let Check::Code(_) = s.check {
            let at = s.rva as usize + 6;
            img.mem[at] ^= 1;
            assert!(!preflight_sites(&sites, &img).passed(), "byte flip in {}", s.name);
            img.mem[at] ^= 1;
        } else {
            let mut m = sites.clone();
            m[i] = Site { rva: if matches!(s.check, Check::Writable) { a.rs_vt as u32 } else { a.rs_global as u32 }, ..*s };
            assert!(!preflight_sites(&m, &img).passed(), "{} at a wrong address passed", s.name);
        }
        caught += 1;
    }
    // a vtable of the wrong class
    let mut m = sites.clone();
    m[4] = Site { rva: a.ctx_vt as u32, ..m[4] };
    assert!(!preflight_sites(&m, &img).passed());
    println!("PASS depth mutations caught: {}", caught + 1);
}
