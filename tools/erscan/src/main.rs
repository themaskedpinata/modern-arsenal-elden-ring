//! erscan: static scan of eldenring.exe (read as a file; never run, loaded or attached).
//!
//! Usage:
//!   erscan <exe> calls <va> [n]        every `call rel32` / `jmp rel32` to <va> with the n (default 12) instructions before it
//!   erscan <exe> argimm <va> <reg>     per call to <va>: the last `mov <reg>,imm` before it (histogram + sites)
//!   erscan <exe> imm <v,v,...>         every instruction with one of these immediates (any operand)
//!   erscan <exe> x <start>-<end>       every instruction in the VA range
//!   erscan <exe> rtti <.?AV..@@>       C3: type descriptor, complete object locators and vtables of an RTTI name
//!   erscan <exe> xref <va,va,...>      C3: every instruction whose RIP-relative operand or immediate is one of the VAs
//!   erscan <exe> fn <va>               C3: the whole function (.pdata bounds) containing the VA, calls annotated
//!
//! The file is mapped by hand from its section table (no loader). Call sites are found by a byte scan for E8/E9 rel32
//! (no linear decode can desync on them); the instructions before a site are decoded from the first start offset
//! (64..8 bytes back) whose decode lands exactly on the site.

use std::collections::BTreeMap;

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, OpKind, Register};

struct Pe {
    data: Vec<u8>,
    base: u64,
    secs: Vec<(String, u64, u64, u64, u64)>,
}

impl Pe {
    fn open(path: &str) -> Result<Pe, String> {
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let rd32 = |o: usize| -> Result<u32, String> {
            data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "truncated".to_string())
        };
        let pe = rd32(0x3c)? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err("not a PE file".into());
        }
        let nsec = u16::from_le_bytes([data[pe + 6], data[pe + 7]]) as usize;
        let optsz = u16::from_le_bytes([data[pe + 20], data[pe + 21]]) as usize;
        let base = u64::from_le_bytes(data[pe + 48..pe + 56].try_into().map_err(|_| "truncated")?);
        let mut secs = Vec::new();
        let mut o = pe + 24 + optsz;
        for _ in 0..nsec {
            let name = String::from_utf8_lossy(&data[o..o + 8]).trim_end_matches('\0').to_string();
            // (name, rva, vsize, raw offset, raw size)
            secs.push((name, rd32(o + 12)? as u64, rd32(o + 8)? as u64, rd32(o + 20)? as u64, rd32(o + 16)? as u64));
            o += 40;
        }
        Ok(Pe { data, base, secs })
    }

    fn off(&self, va: u64) -> Option<usize> {
        let r = va.checked_sub(self.base)?;
        self.secs.iter().find(|s| r >= s.1 && r < s.1 + s.2.max(s.4) && r - s.1 < s.4).map(|s| (r - s.1 + s.3) as usize)
    }

    fn va(&self, off: usize) -> Option<u64> {
        let o = off as u64;
        self.secs.iter().find(|s| o >= s.3 && o < s.3 + s.4).map(|s| self.base + s.1 + (o - s.3))
    }

    /// Executable sections (raw offset, raw size): .text plus any packed/protected code sections.
    fn code(&self) -> Vec<(usize, usize, String)> {
        self.secs
            .iter()
            .filter(|s| s.0 == ".text" || s.0.starts_with(".arx") || s.0 == ".srdata" || s.0 == ".xcode")
            .map(|s| (s.3 as usize, s.4 as usize, s.0.clone()))
            .collect()
    }
}

impl Pe {
    /// Initialised non-code sections (raw offset, raw size): .rdata / .data where RTTI and vtables live.
    fn data_secs(&self) -> Vec<(usize, usize)> {
        self.secs
            .iter()
            .filter(|s| s.0 == ".rdata" || s.0 == ".data")
            .map(|s| (s.3 as usize, (s.4 as usize).min(self.data.len().saturating_sub(s.3 as usize))))
            .collect()
    }

    fn is_code(&self, off: usize) -> bool {
        self.code().iter().any(|(o, n, _)| off >= *o && off < o + n)
    }

    /// The .pdata RUNTIME_FUNCTION (begin, end VA) containing `va` (exception directory = data directory 3).
    fn func_of(&self, va: u64) -> Option<(u64, u64)> {
        let pe = rd32(&self.data, 0x3c) as usize;
        let dd = pe + 24 + 112 + 3 * 8;
        let (rva, size) = (rd32(&self.data, dd) as u64, rd32(&self.data, dd + 4) as usize);
        let o = self.off(self.base + rva)?;
        let r = (va - self.base) as u32;
        let n = size / 12;
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let b = rd32(&self.data, o + mid * 12);
            let e = rd32(&self.data, o + mid * 12 + 4);
            if r < b {
                hi = mid;
            } else if r >= e {
                lo = mid + 1;
            } else {
                return Some((self.base + b as u64, self.base + e as u64));
            }
        }
        None
    }
}

fn rd32(d: &[u8], o: usize) -> u32 {
    d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd64(d: &[u8], o: usize) -> u64 {
    d.get(o..o + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

fn fmt_ins(fmt: &mut IntelFormatter, ins: &Instruction) -> String {
    let mut s = String::new();
    fmt.format(ins, &mut s);
    format!("{:x}  {s}", ins.ip())
}

/// Every E8/E9 rel32 whose target is `target` (raw offsets of the opcode byte).
fn call_sites(pe: &Pe, target: u64) -> Vec<(usize, bool)> {
    let mut out = Vec::new();
    for (o0, n, _) in pe.code() {
        let end = (o0 + n).min(pe.data.len()).saturating_sub(5);
        let mut o = o0;
        while o < end {
            let b = pe.data[o];
            if b == 0xE8 || b == 0xE9 {
                let rel = i32::from_le_bytes([pe.data[o + 1], pe.data[o + 2], pe.data[o + 3], pe.data[o + 4]]) as i64;
                if let Some(va) = pe.va(o)
                    && (va as i64 + 5 + rel) as u64 == target
                {
                    out.push((o, b == 0xE8));
                }
            }
            o += 1;
        }
    }
    out
}

/// The instructions before the site at raw offset `o` (re-synced so the decode lands on `o`), plus the site itself.
fn before(pe: &Pe, o: usize, max_back: usize) -> Vec<Instruction> {
    for back in (8..=max_back).rev() {
        if back > o {
            continue;
        }
        let s = o - back;
        let Some(va) = pe.va(s) else { continue };
        let mut dec = Decoder::with_ip(64, &pe.data[s..o + 5], va, DecoderOptions::NONE);
        let mut v = Vec::new();
        let mut landed = false;
        while dec.can_decode() {
            let pos = dec.position();
            if s + pos == o {
                landed = true;
            }
            let ins = dec.decode();
            if ins.is_invalid() {
                break;
            }
            v.push(ins);
            if s + pos >= o {
                break;
            }
        }
        if landed && v.len() >= 2 {
            return v;
        }
    }
    Vec::new()
}

fn reg_of(name: &str) -> Option<Register> {
    Some(match name {
        "edx" | "rdx" => Register::EDX,
        "ecx" | "rcx" => Register::ECX,
        "r8d" | "r8" => Register::R8D,
        "r9d" | "r9" => Register::R9D,
        _ => return None,
    })
}

fn writes_reg(ins: &Instruction, r: Register) -> bool {
    ins.op_count() > 0 && ins.op0_kind() == OpKind::Register && same_reg(ins.op0_register(), r)
}

fn imm_of(ins: &Instruction, k: u32) -> Option<u64> {
    match ins.op_kind(k) {
        OpKind::Immediate8 => Some(ins.immediate8() as u64),
        OpKind::Immediate16 => Some(ins.immediate16() as u64),
        OpKind::Immediate32 => Some(ins.immediate32() as u64),
        OpKind::Immediate64 => Some(ins.immediate64()),
        OpKind::Immediate8to32 => Some(ins.immediate8to32() as u32 as u64),
        OpKind::Immediate32to64 => Some(ins.immediate32to64() as u64),
        _ => None,
    }
}

fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 4 {
        return Err("usage: erscan <exe> calls|argimm|imm|x <arg> [..]".into());
    }
    let pe = Pe::open(&args[1])?;
    let mut fmt = IntelFormatter::new();
    match args[2].as_str() {
        "calls" => {
            let t = hex(&args[3]).ok_or("bad va")?;
            let n: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(12);
            let sites = call_sites(&pe, t);
            println!("# {} sites to 0x{t:x}", sites.len());
            for (o, is_call) in sites {
                let v = before(&pe, o, 96);
                println!("--- site 0x{:x} {}", pe.va(o).unwrap_or(0), if is_call { "call" } else { "jmp" });
                for ins in v.iter().rev().take(n + 1).collect::<Vec<_>>().into_iter().rev() {
                    println!("  {}", fmt_ins(&mut fmt, ins));
                }
            }
        }
        "argimm" => {
            let t = hex(&args[3]).ok_or("bad va")?;
            let r = reg_of(args.get(4).map_or("edx", |s| s.as_str())).ok_or("reg: edx|ecx|r8d|r9d")?;
            let mut hist: BTreeMap<String, Vec<u64>> = BTreeMap::new();
            for (o, _) in call_sites(&pe, t) {
                let v = before(&pe, o, 96);
                let site = pe.va(o).unwrap_or(0);
                let mut val = "unknown".to_string();
                for ins in v.iter().rev().skip(1) {
                    if writes_reg(ins, r) {
                        val = match imm_of(ins, 1) {
                            Some(i) if ins.mnemonic() == iced_x86::Mnemonic::Mov => format!("0x{i:x}"),
                            _ => {
                                let mut s = String::new();
                                fmt.format(ins, &mut s);
                                format!("expr:{s}")
                            }
                        };
                        break;
                    }
                }
                hist.entry(val).or_default().push(site);
            }
            for (k, v) in &hist {
                let list: Vec<String> = v.iter().map(|s| format!("{s:x}")).collect();
                println!("{k:<40} n={:<4} {}", v.len(), list.join(","));
            }
        }
        "rangeimm" => {
            // every call into [lo, hi): target, site, and the last `mov reg,imm` of edx / r8d / r9d before it
            let lo = hex(&args[3]).ok_or("bad lo")?;
            let hi = hex(args.get(4).ok_or("hi")?).ok_or("bad hi")?;
            for (o0, n, _) in pe.code() {
                let end = (o0 + n).min(pe.data.len()).saturating_sub(5);
                for o in o0..end {
                    if pe.data[o] != 0xE8 {
                        continue;
                    }
                    let rel = i32::from_le_bytes([pe.data[o + 1], pe.data[o + 2], pe.data[o + 3], pe.data[o + 4]]) as i64;
                    let Some(va) = pe.va(o) else { continue };
                    let t = (va as i64 + 5 + rel) as u64;
                    if t < lo || t >= hi {
                        continue;
                    }
                    let v = before(&pe, o, 96);
                    if v.is_empty() {
                        continue;
                    }
                    let mut cols = Vec::new();
                    for (rn, r) in [("edx", Register::EDX), ("r8d", Register::R8D), ("r9d", Register::R9D)] {
                        let mut val = "-".to_string();
                        for ins in v.iter().rev().skip(1) {
                            if writes_reg(ins, r) {
                                if let Some(i) = imm_of(ins, 1).filter(|_| ins.mnemonic() == iced_x86::Mnemonic::Mov) {
                                    val = format!("0x{i:x}");
                                } else {
                                    val = "?".into();
                                }
                                break;
                            }
                        }
                        cols.push(format!("{rn}={val}"));
                    }
                    println!("{t:x} {va:x} {}", cols.join(" "));
                }
            }
        }
        "imm" => {
            let vals: Vec<u64> = args[3].split(',').filter_map(hex).collect();
            for (o0, n, name) in pe.code() {
                let va0 = pe.va(o0).ok_or("section")?;
                let mut dec = Decoder::with_ip(64, &pe.data[o0..(o0 + n).min(pe.data.len())], va0, DecoderOptions::NONE);
                while dec.can_decode() {
                    let ins = dec.decode();
                    for k in 0..ins.op_count() {
                        if imm_of(&ins, k).is_some_and(|i| vals.contains(&i)) {
                            println!("{name} {}", fmt_ins(&mut fmt, &ins));
                        }
                    }
                }
            }
        }
        "x" => {
            let r: Vec<u64> = args[3].split('-').filter_map(hex).collect();
            if r.len() != 2 {
                return Err("x <start>-<end>".into());
            }
            let o = pe.off(r[0]).ok_or("start not in the file")?;
            let len = (r[1].saturating_sub(r[0]) as usize).min(1 << 20);
            let mut dec = Decoder::with_ip(64, &pe.data[o..(o + len).min(pe.data.len())], r[0], DecoderOptions::NONE);
            while dec.can_decode() {
                let ins = dec.decode();
                println!("{}", fmt_ins(&mut fmt, &ins));
            }
        }
        "rtti" => {
            // C3: type descriptor -> complete object locators -> vtables of an MSVC RTTI name (e.g. .?AVCsChrProxy@CS@@)
            let name = args[3].as_bytes();
            let mut pos = 0;
            while let Some(i) = find(&pe.data[pos..], name) {
                let so = pos + i;
                pos = so + 1;
                if pe.data.get(so + name.len()) != Some(&0) {
                    continue;
                }
                let Some(td) = pe.va(so).map(|v| v - 0x10) else { continue };
                let td_rva = (td - pe.base) as u32;
                println!("TD {td:x} ({})", args[3]);
                for (o0, n) in pe.data_secs() {
                    let mut o = o0;
                    while o + 24 <= o0 + n {
                        if rd32(&pe.data, o + 12) == td_rva && rd32(&pe.data, o) == 1 {
                            let col = pe.va(o).unwrap_or(0);
                            let off = rd32(&pe.data, o + 4);
                            println!("  COL {col:x} offset=0x{off:x}");
                            for (v0, vn) in pe.data_secs() {
                                let mut q = v0;
                                while q + 8 <= v0 + vn {
                                    if rd64(&pe.data, q) == col {
                                        let vt = pe.va(q + 8).unwrap_or(0);
                                        let mut ents = Vec::new();
                                        for k in 0..48 {
                                            let e = rd64(&pe.data, q + 8 + 8 * k);
                                            if pe.off(e).is_none_or(|eo| !pe.is_code(eo)) {
                                                break;
                                            }
                                            ents.push(format!("[{k}]{e:x}"));
                                        }
                                        println!("    VT {vt:x} n={} {}", ents.len(), ents.join(" "));
                                    }
                                    q += 8;
                                }
                            }
                        }
                        o += 4;
                    }
                }
            }
        }
        "xref" => {
            // every instruction whose RIP-relative memory operand or immediate is one of the VAs (with its function)
            let vals: Vec<u64> = args[3].split(',').filter_map(hex).collect();
            for (o0, n, name) in pe.code() {
                let va0 = pe.va(o0).ok_or("section")?;
                let mut dec = Decoder::with_ip(64, &pe.data[o0..(o0 + n).min(pe.data.len())], va0, DecoderOptions::NONE);
                while dec.can_decode() {
                    let ins = dec.decode();
                    let mem = ins.is_ip_rel_memory_operand() && vals.contains(&ins.ip_rel_memory_address());
                    let imm = (0..ins.op_count()).any(|k| imm_of(&ins, k).is_some_and(|i| vals.contains(&i)));
                    if mem || imm {
                        let f = pe.func_of(ins.ip()).map_or("?".into(), |(a, b)| format!("{a:x}-{b:x}"));
                        println!("{name} fn={f} {}", fmt_ins(&mut fmt, &ins));
                    }
                }
            }
        }
        "disp" => {
            // every instruction with a [reg+disp] memory operand (not RIP-relative) whose displacement is one of the
            // values; `w` as the 5th argument keeps only stores (memory as operand 0)
            let vals: Vec<u64> = args[3].split(',').filter_map(hex).collect();
            let only_w = args.get(4).is_some_and(|s| s == "w");
            for (o0, n, name) in pe.code() {
                let va0 = pe.va(o0).ok_or("section")?;
                let mut dec = Decoder::with_ip(64, &pe.data[o0..(o0 + n).min(pe.data.len())], va0, DecoderOptions::NONE);
                while dec.can_decode() {
                    let ins = dec.decode();
                    let k = (0..ins.op_count()).find(|&k| ins.op_kind(k) == OpKind::Memory);
                    let Some(k) = k else { continue };
                    if ins.is_ip_rel_memory_operand() || ins.memory_base() == Register::None {
                        continue;
                    }
                    if !vals.contains(&ins.memory_displacement64()) || (only_w && k != 0) {
                        continue;
                    }
                    let f = pe.func_of(ins.ip()).map_or("?".into(), |(a, b)| format!("{a:x}-{b:x}"));
                    println!("{name} fn={f} {}", fmt_ins(&mut fmt, &ins));
                }
            }
        }
        "fn" => {
            // the whole function (from .pdata) containing the VA
            let va = hex(&args[3]).ok_or("bad va")?;
            let (a, b) = pe.func_of(va).ok_or("no .pdata entry")?;
            println!("# fn {a:x}-{b:x}");
            let o = pe.off(a).ok_or("start not in the file")?;
            let mut dec = Decoder::with_ip(64, &pe.data[o..o + (b - a) as usize], a, DecoderOptions::NONE);
            while dec.can_decode() {
                let ins = dec.decode();
                let mut s = fmt_ins(&mut fmt, &ins);
                if ins.mnemonic() == iced_x86::Mnemonic::Call && ins.op0_kind() == OpKind::NearBranch64 {
                    s += &format!("    ; -> {:x}", ins.near_branch_target());
                }
                println!("{s}");
            }
        }
        m => return Err(format!("unknown mode {m}")),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&args) {
        eprintln!("erscan: {e}");
        std::process::exit(1);
    }
}

/// The 32-bit register and its 64-bit and 16/8-bit forms count as one.
fn same_reg(a: Register, r32: Register) -> bool {
    let fam: &[Register] = match r32 {
        Register::EDX => &[Register::EDX, Register::RDX, Register::DX, Register::DL],
        Register::ECX => &[Register::ECX, Register::RCX, Register::CX, Register::CL],
        Register::R8D => &[Register::R8D, Register::R8, Register::R8W, Register::R8L],
        Register::R9D => &[Register::R9D, Register::R9, Register::R9W, Register::R9L],
        _ => &[],
    };
    fam.contains(&a)
}
