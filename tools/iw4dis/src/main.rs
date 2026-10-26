//! iw4dis: static disassembly of iw4sp.exe (read as a file; never run).
//!
//! Usage:
//!   iw4dis <exe> site <va,va,...>      the instruction at each VA (text + the f32 a RIP-relative operand points at)
//!   iw4dis <exe> x <start>-<end>       every instruction in the VA range
//!   iw4dis <exe> xref <va,va,...>      every RIP-relative reference to one of the VAs
//!   iw4dis <exe> fval <f,f,...>        every RIP-relative load of an .rdata f32 with one of these values
//!
//! The file is mapped by hand from its section table (no loader), .text is decoded linearly with iced-x86.

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};

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
            secs.push((name, rd32(o + 12)? as u64, rd32(o + 8)? as u64, rd32(o + 20)? as u64, rd32(o + 16)? as u64));
            o += 40;
        }
        Ok(Pe { data, base, secs })
    }

    fn off(&self, va: u64) -> Option<usize> {
        let r = va.checked_sub(self.base)?;
        self.secs.iter().find(|s| r >= s.1 && r < s.1 + s.2.max(s.4) && r - s.1 < s.4).map(|s| (r - s.1 + s.3) as usize)
    }

    fn f32_at(&self, va: u64) -> Option<f32> {
        let o = self.off(va)?;
        self.data.get(o..o + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn text(&self) -> Option<(u64, &[u8])> {
        let t = self.secs.iter().find(|s| s.0 == ".text")?;
        Some((self.base + t.1, self.data.get(t.3 as usize..(t.3 + t.4) as usize)?))
    }
}

fn hexlist(s: &str) -> Vec<u64> {
    s.split(',').filter_map(|x| u64::from_str_radix(x.trim().trim_start_matches("0x"), 16).ok()).collect()
}

fn line(pe: &Pe, fmt: &mut IntelFormatter, ins: &Instruction) -> String {
    let mut s = String::new();
    fmt.format(ins, &mut s);
    if ins.is_ip_rel_memory_operand()
        && let Some(f) = pe.f32_at(ins.ip_rel_memory_address())
    {
        s.push_str(&format!("  ; f32={f}"));
    }
    format!("{:x}  {s}", ins.ip())
}

fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 4 {
        return Err("usage: iw4dis <exe> site|x|xref|fval <arg>".into());
    }
    let pe = Pe::open(&args[1])?;
    let mut fmt = IntelFormatter::new();
    let (tva, tbytes) = pe.text().ok_or("no .text")?;
    match args[2].as_str() {
        "site" => {
            for va in hexlist(&args[3]) {
                let o = pe.off(va).ok_or_else(|| format!("0x{va:x} not in the file"))?;
                let mut dec = Decoder::with_ip(64, &pe.data[o..(o + 16).min(pe.data.len())], va, DecoderOptions::NONE);
                let ins = dec.decode();
                println!("{}", line(&pe, &mut fmt, &ins));
            }
        }
        "x" => {
            let r = args[3].split('-').filter_map(|x| u64::from_str_radix(x.trim_start_matches("0x"), 16).ok()).collect::<Vec<_>>();
            if r.len() != 2 {
                return Err("x <start>-<end>".into());
            }
            let o = pe.off(r[0]).ok_or("start not in the file")?;
            let len = (r[1].saturating_sub(r[0]) as usize).min(1 << 20);
            let mut dec = Decoder::with_ip(64, &pe.data[o..(o + len).min(pe.data.len())], r[0], DecoderOptions::NONE);
            while dec.can_decode() {
                let ins = dec.decode();
                println!("{}", line(&pe, &mut fmt, &ins));
            }
        }
        "xref" | "fval" => {
            let vas = hexlist(&args[3]);
            let vals: Vec<f32> = args[3].split(',').filter_map(|x| x.parse().ok()).collect();
            let mut dec = Decoder::with_ip(64, tbytes, tva, DecoderOptions::NONE);
            while dec.can_decode() {
                let ins = dec.decode();
                if !ins.is_ip_rel_memory_operand() {
                    continue;
                }
                let m = ins.ip_rel_memory_address();
                let hit = if args[2] == "xref" { vas.contains(&m) } else { pe.f32_at(m).is_some_and(|f| vals.contains(&f)) };
                if hit {
                    println!("{}", line(&pe, &mut fmt, &ins));
                }
            }
        }
        m => return Err(format!("unknown mode {m}")),
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&args) {
        eprintln!("iw4dis: {e}");
        std::process::exit(1);
    }
}
