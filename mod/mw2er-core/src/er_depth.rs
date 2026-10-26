//! C6 depth probe P-D, read-only part (work/c6/depth3.md 1 / 5 lines 1-2, docs/C6_HOLD_DESIGN.md part 13.3):
//! the pointer chain to Elden Ring's main scene depth buffer and the rules its desc must pass before any later pass may
//! read it. `[coop] probe_er_depth` (cold, default false). Nothing here binds, barriers or samples ER's resource; the DLL
//! reads the chain with non-faulting reads on the task thread; `GetDesc` (a pure query) runs only in UiBegin's detour
//! on ER's render thread (review C6 depth, the C4.1 rule), only through a vtable the task found in the D3D12 runtime
//! module of our own back buffer.
//!
//! ```text
//! RS   = [base + rs_global]                GXSR::GXRenderingSystem      [RS]     == base + rs_vt
//! ctx  = RS + ctx_off (inline)             GXSR::GXSceneContext         [ctx]    == base + ctx_vt, [ctx+8] == base + ctx_vt2
//! dsb  = [ctx + dsb] (== [ctx + dsb_alias]) GXBS::GXDepthStencilBuffer   [dsb]    == base + dsb_vt, [[dsb]+0x38] == base + count_fn
//! obj  = [[dsb + dsb_arr] + 0]             (getter 0x141ae26d0, index 0 < count)
//! res  = [[obj + 8] + 0x20]                ID3D12Resource*  (as UiBegin reads DefDepthStencil at 0x140d809ef)
//! ```
//! Candidate B = `S + 0xE8` "DefDepthStencil" (`S = [[base + g_render] + 0x128]`), the same chain from its dsb.

use crate::c4::{Rd, TexDesc};

/// 2.7.2.0 values [V exe file 2026-10-21: RTTI of every vtable re-read by work/c6/depth4/rtti.py; chain from depth3.md 1].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErDepthAddrs {
    /// `.data` global holding GXRenderingSystem* (`mov r8,[1447F67C8h]` at 0x141a1e843).
    pub rs_global: usize,
    pub rs_vt: usize,
    pub ctx_off: usize,
    pub ctx_vt: usize,
    pub ctx_vt2: usize,
    /// `[ctx+0xA70]` "DepthTexture", `[ctx+0xA88]` the same object, `[ctx+0xA78]` "DepthTileTexture".
    pub dsb: usize,
    pub dsb_alias: usize,
    pub tile: usize,
    pub dsb_vt: usize,
    /// GXDepthStencilBuffer vtable slot 7 (`+0x38`): the image count (0x1419f3dc0, shared with GXRenderTarget2D).
    pub count_slot: usize,
    pub count_fn: usize,
    /// count = `[T+0x38] ? u32 [[[T+0x38]+8]+8] : u8 [T+0x46]` with `T = [dsb+0x40]` (as `c4::chain41`).
    pub tex: usize,
    pub tex_desc: usize,
    pub desc_data: usize,
    pub desc_count: usize,
    pub tex_count: usize,
    /// The getter 0x141ae26d0: `arr = [dsb+0x70]`, `obj = [arr + 8*i]`.
    pub arr: usize,
    pub obj_wrap: usize,
    pub wrap_res: usize,
    /// `S+0xE8` DefDepthStencil (candidate B).
    pub def_ds: usize,
}

pub const ER_DEPTH_2720: ErDepthAddrs = ErDepthAddrs {
    rs_global: 0x47f_67c8,
    rs_vt: 0x2f1_0500,
    ctx_off: 0x50,
    ctx_vt: 0x2f1_2638,
    ctx_vt2: 0x2f1_2658,
    dsb: 0xa70,
    dsb_alias: 0xa88,
    tile: 0xa78,
    dsb_vt: 0x2f2_75c0,
    count_slot: 0x38,
    count_fn: 0x19f_3dc0,
    tex: 0x40,
    tex_desc: 0x38,
    desc_data: 8,
    desc_count: 8,
    tex_count: 0x46,
    arr: 0x70,
    obj_wrap: 8,
    wrap_res: 0x20,
    def_ds: 0xe8,
};

fn q(rd: Rd, p: usize, off: usize) -> usize {
    if p == 0 {
        return 0;
    }
    p.checked_add(off).and_then(|a| rd(a, 8)).unwrap_or(0) as usize
}

fn n(rd: Rd, p: usize, off: usize, len: usize) -> u64 {
    if p == 0 {
        return 0;
    }
    p.checked_add(off).and_then(|a| rd(a, len)).unwrap_or(0)
}

/// One depth-stencil object's chain from `dsb`: its vtable pin, the count slot pin, the count, the image object, the GX
/// wrapper and the resource (0 unless both pins hold and count >= 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DsbChain {
    pub dsb: usize,
    pub vt: bool,
    pub slot7: bool,
    pub count: u32,
    pub obj: usize,
    pub wrap: usize,
    pub res: usize,
}

impl DsbChain {
    pub fn ok(&self) -> bool {
        self.vt && self.slot7 && self.count >= 1 && self.res != 0
    }
}

pub fn dsb_chain(dsb: usize, base: usize, a: &ErDepthAddrs, rd: Rd) -> DsbChain {
    let mut c = DsbChain { dsb, ..DsbChain::default() };
    if dsb == 0 {
        return c;
    }
    let pin = |v: usize, rva: usize| v != 0 && base.checked_add(rva) == Some(v);
    let vt = q(rd, dsb, 0);
    c.vt = pin(vt, a.dsb_vt);
    c.slot7 = c.vt && pin(q(rd, vt, a.count_slot), a.count_fn);
    let t = q(rd, dsb, a.tex);
    if t != 0 {
        let d = q(rd, t, a.tex_desc);
        c.count = if d != 0 { n(rd, q(rd, d, a.desc_data), a.desc_count, 4) as u32 } else { n(rd, t, a.tex_count, 1) as u32 };
    }
    if c.vt && c.slot7 && c.count >= 1 {
        c.obj = q(rd, q(rd, dsb, a.arr), 0);
        c.wrap = q(rd, c.obj, a.obj_wrap);
        c.res = q(rd, c.wrap, a.wrap_res);
    }
    c
}

/// The main scene context's chain (candidate A) from the global's value `rs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneChain {
    pub rs: usize,
    pub rs_vt: bool,
    pub ctx_vt: bool,
    pub ctx_vt2: bool,
    /// `[ctx+0xA70] == [ctx+0xA88]`.
    pub alias_eq: bool,
    pub tile: usize,
    pub a: DsbChain,
}

impl SceneChain {
    /// The pins (RS vt, ctx vt, ctx +8 vt, dsb vt, count slot) and count >= 1 (`pins=n/6`).
    pub fn pins(&self) -> u32 {
        [self.rs_vt, self.ctx_vt, self.ctx_vt2, self.a.vt, self.a.slot7, self.a.count >= 1].iter().filter(|x| **x).count() as u32
    }

    pub fn ok(&self) -> bool {
        self.pins() == 6 && self.alias_eq && self.a.ok()
    }
}

pub fn scene_chain(rs: usize, base: usize, a: &ErDepthAddrs, rd: Rd) -> SceneChain {
    let mut c = SceneChain { rs, ..SceneChain::default() };
    if rs == 0 {
        return c;
    }
    let pin = |v: usize, rva: usize| v != 0 && base.checked_add(rva) == Some(v);
    c.rs_vt = pin(q(rd, rs, 0), a.rs_vt);
    let ctx = rs + a.ctx_off;
    c.ctx_vt = pin(q(rd, ctx, 0), a.ctx_vt);
    c.ctx_vt2 = pin(q(rd, ctx, 8), a.ctx_vt2);
    if !(c.rs_vt && c.ctx_vt && c.ctx_vt2) {
        return c;
    }
    let dsb = q(rd, ctx, a.dsb);
    c.alias_eq = dsb != 0 && q(rd, ctx, a.dsb_alias) == dsb;
    c.tile = q(rd, ctx, a.tile);
    c.a = dsb_chain(dsb, base, a, rd);
    c
}

/// DXGI 19 / 20: R32G8X24_TYPELESS / D32_FLOAT_S8X24_UINT (the builder's 0x14).
pub const FMT_R32G8X24_TYPELESS: u32 = 19;
pub const FMT_D32_FLOAT_S8X24_UINT: u32 = 20;
pub const DIM_TEXTURE2D: u32 = 3;
pub const FLAG_ALLOW_DEPTH_STENCIL: u32 = 0x2;
pub const FLAG_DENY_SHADER_RESOURCE: u32 = 0x8;

/// Design depth3.md 5 line 2: what a later copy pass needs of the scene depth: D32S8 (typed or typeless), 2D, one
/// array slice, one mip, single sample, a depth-stencil resource that allows shader reads, back-buffer sized.
pub fn desc_rule(d: &TexDesc, bb: (u32, u32)) -> Result<(), &'static str> {
    if !matches!(d.format, FMT_R32G8X24_TYPELESS | FMT_D32_FLOAT_S8X24_UINT) {
        return Err("format");
    }
    if d.dimension != DIM_TEXTURE2D || d.array != 1 || d.mips != 1 || d.samples != 1 {
        return Err("shape");
    }
    if d.flags & FLAG_ALLOW_DEPTH_STENCIL == 0 {
        return Err("not_depth");
    }
    if d.flags & FLAG_DENY_SHADER_RESOURCE != 0 {
        return Err("deny_srv");
    }
    if bb.0 == 0 || d.width != bb.0 as u64 || d.height != bb.1 {
        return Err("size");
    }
    Ok(())
}

/// `DEPTH probe desc A|B unmeasured (why)`: no `GetDesc` sample from ER's render thread (review C6 depth: the task
/// never calls `GetDesc`).
pub fn desc_unmeasured(tag: &str, why: &str) -> String {
    format!("desc {tag} unmeasured ({why})")
}

/// `DEPTH probe desc A|B ...` (one line per candidate).
pub fn desc_text(tag: &str, d: Option<&TexDesc>, bb: (u32, u32)) -> String {
    match d {
        None => format!("desc {tag} none (res unreadable / not a D3D12 runtime object) bb={}x{}", bb.0, bb.1),
        Some(d) => format!(
            "desc {tag} fmt={} {}x{} dim={} array={} mips={} samples={} flags=0x{:x} deny_srv={} bb={}x{} rule={}",
            d.format,
            d.width,
            d.height,
            d.dimension,
            d.array,
            d.mips,
            d.samples,
            d.flags,
            (d.flags & FLAG_DENY_SHADER_RESOURCE != 0) as u8,
            bb.0,
            bb.1,
            desc_rule(d, bb).err().unwrap_or("ok")
        ),
    }
}

/// `DEPTH probe chain ...`.
pub fn chain_text(c: &SceneChain, b: &DsbChain, res_mod: bool, changes: u32) -> String {
    format!(
        "chain rs=0x{:x} pins={}/6 rs_vt={} ctx_vt={}/{} a88_eq={} dsb=0x{:x} vt={} slot7={} count={} obj=0x{:x} wrap=0x{:x} res=0x{:x} module=d3d12:{} tile=0x{:x} changes={} | B def_ds=0x{:x} vt={} count={} res=0x{:x}",
        c.rs,
        c.pins(),
        c.rs_vt as u8,
        c.ctx_vt as u8,
        c.ctx_vt2 as u8,
        c.alias_eq as u8,
        c.a.dsb,
        c.a.vt as u8,
        c.a.slot7 as u8,
        c.a.count,
        c.a.obj,
        c.a.wrap,
        c.a.res,
        res_mod as u8,
        c.tile,
        changes,
        b.dsb,
        b.vt as u8,
        b.count,
        b.res
    )
}

/// The GX wrapper `[obj+8]` as 13 qwords (+0x00..+0x60): the probe logs it on every change to look for ER's own
/// tracked resource state (a word that takes D3D12 state values 0x10 / 0x40 / 0x80 / 0xC0 / 0xE0 between frames).
pub const WRAP_QWORDS: usize = 13;

pub fn wrap_text(w: &[u64; WRAP_QWORDS]) -> String {
    w.iter().enumerate().map(|(k, v)| format!("+{:02x}:{v:x}", k * 8)).collect::<Vec<_>>().join(" ")
}

/// The words of the wrapper that look like a D3D12 resource-state value (low dword in the state set, high dword 0).
pub fn state_like(w: &[u64; WRAP_QWORDS]) -> Vec<(usize, u32)> {
    const STATES: [u32; 8] = [0x10, 0x20, 0x40, 0x80, 0xC0, 0xE0, 0x800, 0x400];
    let mut v = Vec::new();
    for (k, &x) in w.iter().enumerate() {
        for half in [x as u32, (x >> 32) as u32] {
            if STATES.contains(&half) {
                v.push((k * 8, half));
                break;
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BASE: usize = 0x1_4000_0000;

    /// A fake process: qwords at addresses (u8 / u32 reads take the low bytes).
    fn mem() -> HashMap<usize, u64> {
        let a = ER_DEPTH_2720;
        let mut m = HashMap::new();
        let rs = 0x7000_0000usize;
        let dsb = 0x7100_0000usize;
        let vt = BASE + a.dsb_vt;
        let tex = 0x7200_0000usize;
        let arr = 0x7300_0000usize;
        let obj = 0x7400_0000usize;
        let wrap = 0x7500_0000usize;
        m.insert(BASE + a.rs_global, rs as u64);
        m.insert(rs, (BASE + a.rs_vt) as u64);
        m.insert(rs + a.ctx_off, (BASE + a.ctx_vt) as u64);
        m.insert(rs + a.ctx_off + 8, (BASE + a.ctx_vt2) as u64);
        m.insert(rs + a.ctx_off + a.dsb, dsb as u64);
        m.insert(rs + a.ctx_off + a.dsb_alias, dsb as u64);
        m.insert(rs + a.ctx_off + a.tile, 0x7600_0000);
        m.insert(dsb, vt as u64);
        m.insert(vt + a.count_slot, (BASE + a.count_fn) as u64);
        m.insert(dsb + a.tex, tex as u64);
        m.insert(tex + a.tex_count, 1);
        m.insert(dsb + a.arr, arr as u64);
        m.insert(arr, obj as u64);
        m.insert(obj + a.obj_wrap, wrap as u64);
        m.insert(wrap + a.wrap_res, 0x7700_0000);
        m
    }

    fn reader(m: &HashMap<usize, u64>) -> impl Fn(usize, usize) -> Option<u64> + '_ {
        move |addr, len| {
            m.get(&addr).map(|v| match len {
                1 => *v & 0xff,
                4 => *v & 0xffff_ffff,
                _ => *v,
            })
        }
    }

    #[test]
    fn c6_depth_chain_pins_and_each_mutation() {
        let a = ER_DEPTH_2720;
        let m = mem();
        let rs = m[&(BASE + a.rs_global)] as usize;
        let c = scene_chain(rs, BASE, &a, &reader(&m));
        assert!(c.ok(), "{c:?}");
        assert_eq!((c.pins(), c.a.res, c.a.count, c.tile), (6, 0x7700_0000, 1, 0x7600_0000));
        // every pin / link broken alone: never ok, and nothing past a failed pin is followed to the resource
        let dsb = 0x7100_0000usize;
        let vt = BASE + a.dsb_vt;
        for (what, addr, val) in [
            ("rs vt", rs, 1u64),
            ("ctx vt", rs + a.ctx_off, 1),
            ("ctx vt2", rs + a.ctx_off + 8, 1),
            ("alias", rs + a.ctx_off + a.dsb_alias, 0x7100_0008),
            ("dsb vt", dsb, (BASE + a.rs_vt) as u64),
            ("slot7", vt + a.count_slot, 0),
            ("count", 0x7200_0000 + a.tex_count, 0),
            ("res", 0x7500_0000 + a.wrap_res, 0),
        ] {
            let mut mm = m.clone();
            mm.insert(addr, val);
            let c = scene_chain(rs, BASE, &a, &reader(&mm));
            assert!(!c.ok(), "{what} mutated but ok");
            if matches!(what, "dsb vt" | "slot7" | "count") {
                assert_eq!(c.a.res, 0, "{what}: res followed past a failed pin");
            }
        }
        assert_eq!(scene_chain(0, BASE, &a, &reader(&m)), SceneChain::default());
        // candidate B: the same dsb chain from S+0xE8
        let b = dsb_chain(dsb, BASE, &a, &reader(&m));
        assert!(b.ok() && b.res == 0x7700_0000);
        assert!(chain_text(&c, &b, true, 0).contains("pins=6/6 rs_vt=1 ctx_vt=1/1 a88_eq=1"));
    }

    #[test]
    fn c6_depth_desc_rule() {
        let good = TexDesc { dimension: 3, width: 3840, height: 2160, array: 1, mips: 1, format: 20, samples: 1, flags: 0x2 };
        assert_eq!(desc_rule(&good, (3840, 2160)), Ok(()));
        assert_eq!(desc_rule(&TexDesc { format: 19, ..good }, (3840, 2160)), Ok(()));
        for (bad, why) in [
            (TexDesc { format: 40, ..good }, "format"),
            (TexDesc { mips: 2, ..good }, "shape"),
            (TexDesc { samples: 4, ..good }, "shape"),
            (TexDesc { dimension: 4, ..good }, "shape"),
            (TexDesc { flags: 0x0, ..good }, "not_depth"),
            (TexDesc { flags: 0xa, ..good }, "deny_srv"),
            (TexDesc { width: 1920, ..good }, "size"),
        ] {
            assert_eq!(desc_rule(&bad, (3840, 2160)), Err(why));
        }
        assert_eq!(desc_rule(&good, (0, 0)), Err("size"));
        assert!(desc_text("A", Some(&good), (3840, 2160)).ends_with("rule=ok"));
        assert!(desc_text("B", None, (3840, 2160)).contains("none"));
        // review C6 depth: no render-thread sample: the line says so (the analyzer skips it, never judges it ok)
        assert_eq!(desc_unmeasured("A", "no UiBegin hook"), "desc A unmeasured (no UiBegin hook)");
        let mut w = [0u64; WRAP_QWORDS];
        w[3] = 0xC0;
        w[5] = 0x40_0000_0000;
        assert_eq!(state_like(&w), vec![(0x18, 0xC0), (0x28, 0x40)]);
        assert!(wrap_text(&w).starts_with("+00:0 +08:0 +10:0 +18:c0"));
    }
}
