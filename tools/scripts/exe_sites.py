"""Exe site tables for the boot preflight (port 2.7.2.0, MODLOG 2026-10-20 "port to 2.7.2.0").

Generates mod/mw2er-core/src/exe_sites.rs: for each supported eldenring.exe build, every address mw2er depends on
(the 107 fromsoftware-rs RVAs of that build's vendored table + our own hook / data / vtable addresses), each with the
check the DLL runs on the LOADED image at boot before it calls anything (mw2er_core::exe_build::preflight):
  Code(bytes)  the first bytes at the RVA equal the file's bytes (2.7.2.0: 16 bytes, read from the file here)
  Vmt(name)    vtable[-1] -> RTTI complete object locator (signature 1, pSelf == itself) -> type name == name
  Exec / Writable / ReadOnly   the RVA lies in a section with those characteristics (resolves, no bytes known)

Read-only: the game exe is only read as a file (never launched, never written). No network.

  python -I tools/scripts/exe_sites.py --write   regenerate exe_sites.rs from the 2.7.2.0 exe on disk
  python -I tools/scripts/exe_sites.py --check   regenerate in memory: must equal the file on disk; own sites re-found
                                                 by signature; prints `EXE SITES PASS ...`
  python -I tools/scripts/exe_sites.py --self-test  mutations of the inputs must be caught

2.7.1.0 (the exe is no longer on disk): own sites keep the prologue bytes pinned from that exe file earlier (cam_hook /
ui_probe / c3_static history); crate VMTs found by RTTI class name in mapper-profile.toml get Vmt(<the same class>);
all other crate entries only Exec / Writable / ReadOnly (section kind inferred from the 2.7.2.0 layout).
"""
import hashlib
import pathlib
import re
import struct
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXE = pathlib.Path(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe")
EXE_2720_SHA256 = "f9118fea2cfd8fa3901e8060bfadb1552cb2471771ded8a5b662c4e298ffb328"
EXE_2720_SIZE = 86_973_008
VENDOR = ROOT / "vendor" / "fromsoftware-rs" / "crates" / "eldenring"
PROFILE = VENDOR / "mapper-profile.toml"
TABLE_2710 = VENDOR / "src" / "rva" / "rva_ww.rs"
TABLE_2720 = VENDOR / "src" / "rva" / "rva_ww2720.rs"
OUT = ROOT / "mod" / "mw2er-core" / "src" / "exe_sites.rs"
IMAGE_BASE = 0x1_4000_0000
WIN = 16  # code window length (2.7.2.0)

SCN_EXEC, SCN_READ, SCN_WRITE = 0x2000_0000, 0x4000_0000, 0x8000_0000

# Crate entries whose pattern had more than one distinct match on 2.7.2.0 (work/port2720/mapcheck2720.tsv): the mapper
# takes the first match, as upstream always did. None of them is used by mw2er (grep: only the crate's dlcr.rs,
# action_button.rs, user_input_manager.rs use them; fd4param_repository_vmt is used by nothing). Kept, marked Inferred.
AMBIGUOUS_2720 = {"crypto_spi_registry", "cs_action_button_man_execute_action_button", "fd4param_repository_vmt",
                  "obtain_dl_user_input_manager"}

# Our own addresses. Each: (name, rva_2710, rva_2720, kind, 2710 pinned bytes or None, 2720 signature or None)
# kind: "code" | "data" | "vmt:<rtti name>". The 2.7.2.0 RVA is re-found here by signature (unique in .text) or by the
# RTTI name, never taken on trust; the 2.7.1.0 prologues are the ones pinned from the old exe file.
OWN = [
    ("camera_step", 0x3bc5f0, 0x3bc5f0, "code", "48 89 5c 24 08 57 48 83 ec 30",
     "48 89 5c 24 08 57 48 83 ec 30 f3 0f ? ? ? ? 9a 03 48 8b fa 0f 29 74 24 20 48 8b d9 0f 57 f6 0f 2e c6"),
    ("follow_cam", 0x3b7570, 0x3b7570, "code", "48 8b c4 53 48 81 ec e0 00 00 00 80 b9 40 02 00",
     "48 8b c4 53 48 81 ec e0 00 00 00 80 b9 40 02 00"),
    ("cam_wall_recovery", 0x3b19c80, 0x3b1cc80, "data", None, "0f b6 43 0d 88 05 ? ? ? ? 0f b6 43 10"),
    ("scaleform_begin_scene", 0x115e6b0, 0x115fda0, "code", "40 57 48 83 ec 20 8b 41 58 48 8b f9 a8 02 75 08",
     "40 57 48 83 ec 20 8b 41 58 48 8b f9 a8 02 75 08"),
    ("scaleform_end_scene", 0x115e810, 0x115ff00, "code", "41 55 41 56 48 83 ec 48",
     "41 55 41 56 48 83 ec 48 45 33 ed 4c 8b f1 44 89 6c 24 60 8b 41 58 83 e0"),
    ("cs_collision_filter_vmt", 0x2b94df0, 0x2b96ee0, "vmt:.?AVCSCollisionFilter@CS@@", None, None),
]


class Pe:
    def __init__(self, data):
        self.d = data
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        assert data[pe:pe + 4] == b"PE\0\0"
        nsec = struct.unpack_from("<H", data, pe + 6)[0]
        so = struct.unpack_from("<H", data, pe + 20)[0]
        opt = pe + 24
        self.image_base = struct.unpack_from("<Q", data, opt + 24)[0]
        self.size_of_image = struct.unpack_from("<I", data, opt + 56)[0]
        self.dll_chars = struct.unpack_from("<H", data, opt + 70)[0]
        ndirs = struct.unpack_from("<I", data, opt + 108)[0]
        self.dirs = [struct.unpack_from("<II", data, opt + 112 + 8 * i) for i in range(ndirs)]
        self.secs = []
        for i in range(nsec):
            o = opt + so + 40 * i
            name = data[o:o + 8].rstrip(b"\0").decode("latin1")
            vsize, va, rsize, raw = struct.unpack_from("<IIII", data, o + 8)
            ch = struct.unpack_from("<I", data, o + 36)[0]
            self.secs.append((name, va, vsize, raw, rsize, ch))

    def sec_of(self, rva):
        for s in self.secs:
            if s[1] <= rva < s[1] + max(s[2], s[4]):
                return s
        return None

    def read(self, rva, n):
        s = self.sec_of(rva)
        if not s or rva - s[1] + n > s[4]:
            return None
        o = s[3] + rva - s[1]
        return self.d[o:o + n]

    def u32(self, rva):
        b = self.read(rva, 4)
        return None if b is None else struct.unpack("<I", b)[0]

    def u64(self, rva):
        b = self.read(rva, 8)
        return None if b is None else struct.unpack("<Q", b)[0]

    def find(self, pat, secname=".text"):
        rx = re.compile(b"".join(b"." if t == "?" else re.escape(bytes([int(t, 16)])) for t in pat.split()), re.S)
        hits = []
        for s in self.secs:
            if secname and s[0] != secname:
                continue
            blob = self.d[s[3]:s[3] + s[4]]
            hits += [s[1] + m.start() for m in rx.finditer(blob)]
        return hits

    def func_starts(self):
        rva, size = self.dirs[3]  # exception directory (.pdata)
        out = set()
        for i in range(size // 12):
            b, e, _u = struct.unpack("<III", self.read(rva + 12 * i, 12))
            out.add(b)
        return out

    def reloc_rvas(self):
        rva, size = self.dirs[5]
        out = set()
        if not rva or not size:
            return out
        o = 0
        while o < size:
            page, blk = struct.unpack("<II", self.read(rva + o, 8))
            if blk < 8:
                break
            for i in range((blk - 8) // 2):
                e = struct.unpack("<H", self.read(rva + o + 8 + 2 * i, 2))[0]
                if e >> 12:
                    out.add(page + (e & 0xFFF))
            o += blk
        return out

    def rtti_name(self, vmt_rva):
        """vtable[-1] -> COL -> TypeDescriptor name, the same walk the DLL does on the loaded image."""
        col_va = self.u64(vmt_rva - 8)
        if col_va is None or col_va < self.image_base:
            return None
        col = col_va - self.image_base
        sig, td, self_rva = self.u32(col), self.u32(col + 12), self.u32(col + 20)
        if sig != 1 or self_rva != col or td is None:
            return None
        raw = self.read(td + 16, 256)
        if raw is None or b"\0" not in raw:
            return None
        return raw.split(b"\0", 1)[0].decode("latin1")

    def kind(self, rva):
        s = self.sec_of(rva)
        if not s:
            return None
        ch = s[5]
        if ch & SCN_EXEC:
            return "Exec"
        if ch & SCN_WRITE:
            return "Writable"
        if ch & SCN_READ:
            return "ReadOnly"
        return None


def parse_table(path):
    txt = path.read_text(encoding="utf-8")
    return {m.group(1): int(m.group(2), 16) for m in re.finditer(r"^\s*(\w+):\s*0x([0-9a-fA-F]+),", txt, re.M)}


def profile_vmts():
    prof = tomllib.loads(PROFILE.read_text(encoding="utf-8"))
    return {v["vftable"]: v["class"] for v in prof.get("vmts", [])}


def _qual(name):
    """`NS::Name` -> `Name@NS@` (MSVC qualified name, innermost first; templates as `?$Name@<args>@`)."""
    m = re.fullmatch(r"(.*?)(\w+)<(.+)>", name)
    if m:
        ns = [p for p in m.group(1).split("::") if p]
        args = "".join("V" + _qual(a.strip()) + "@" for a in m.group(3).split(","))
        head = "?$" + m.group(2) + "@" + args + "@"
        return head + "".join(p + "@" for p in reversed(ns))
    parts = name.split("::")
    return "".join(p + "@" for p in reversed(parts))


def mangled(cls):
    """`CS::ChrCam` -> `.?AVChrCam@CS@@`; `DLUID::PadDevice<DLKR::DLMultiThreadingPolicy>` ->
    `.?AV?$PadDevice@VDLMultiThreadingPolicy@DLKR@@@DLUID@@` (MSVC RTTI type names)."""
    return ".?AV" + _qual(cls) + "@"


def hexb(b):
    return ", ".join(f"0x{x:02x}" for x in b)


def build(data, t2710, t2720, vmts):
    """Returns (rust source, report lines, problems)."""
    problems, report = [], []
    pe = Pe(data)
    if pe.image_base != IMAGE_BASE:
        problems.append(f"image base 0x{pe.image_base:x}")
    starts = pe.func_starts()
    relocs = pe.reloc_rvas()
    if set(t2710) != set(t2720):
        problems.append(f"table fields differ: {sorted(set(t2710) ^ set(t2720))}")
    sites20, sites10 = [], []
    names_by_field = {}
    for f in sorted(t2720):
        rva = t2720[f]
        kind = pe.kind(rva)
        conf = "Inferred" if f in AMBIGUOUS_2720 else "Verified"
        if rva == 0 or kind is None:
            problems.append(f"crate {f}: rva 0x{rva:x} unresolved")
            continue
        if kind == "ReadOnly" and f.endswith("_vmt") and f not in vmts and pe.rtti_name(rva) is None:
            # FD4 / CSTask vtables found by pattern carry no RTTI locator: slots 0 and 1 must point into code
            slots = [pe.u64(rva), pe.u64(rva + 8)]
            if any(s is None or s < IMAGE_BASE or pe.kind(s - IMAGE_BASE) != "Exec" for s in slots):
                problems.append(f"crate {f}: 0x{rva:x} slots {slots} do not point into code")
                continue
            sites20.append((f, rva, "Check::VtableNoRtti", "Crate", conf))
            sites10.append((f, t2710[f], "Check::ReadOnly", "Crate", "Inferred"))
            continue
        if f in vmts or (kind == "ReadOnly" and f.endswith("_vmt")):
            name = pe.rtti_name(rva)
            if name is None:
                problems.append(f"crate {f}: 0x{rva:x} has no valid RTTI locator")
                continue
            if f in vmts and name != mangled(vmts[f]):
                problems.append(f"crate {f}: RTTI {name} != profile class {vmts[f]}")
            names_by_field[f] = name
            sites20.append((f, rva, f'Check::Vmt("{name}")', "Crate", conf))
        elif kind == "Exec":
            win = pe.read(rva, WIN)
            thunk = win[:3] == bytes.fromhex("488b89") and win[7] == 0xE9
            if rva not in starts and not thunk:
                problems.append(f"crate {f}: 0x{rva:x} is not a .pdata function start")
            bad = [r for r in relocs if rva - 8 < r < rva + WIN]
            if bad:
                problems.append(f"crate {f}: base relocation inside the code window ({[hex(r) for r in bad]})")
            sites20.append((f, rva, f"Check::Code(&[{hexb(win)}])", "Crate", conf))
        else:
            sites20.append((f, rva, f"Check::{kind}", "Crate", conf))
        # 2.7.1.0: same class name for the profile's RTTI vtables (the mapper found them by that name); else section kind
        old = t2710[f]
        if f in vmts:
            sites10.append((f, old, f'Check::Vmt("{mangled(vmts[f])}")', "Crate", "Verified"))
        else:
            k10 = "Exec" if kind == "Exec" else ("Writable" if kind == "Writable" else "ReadOnly")
            sites10.append((f, old, f"Check::{k10}", "Crate", "Inferred"))
    # own sites
    for name, r10, r20, kind, pin10, sig in OWN:
        if kind == "code":
            hits = pe.find(sig)
            if hits != [r20]:
                problems.append(f"own {name}: signature hits {[hex(h) for h in hits]} (want [0x{r20:x}])")
            if r20 not in starts:
                problems.append(f"own {name}: 0x{r20:x} is not a .pdata function start")
            win = pe.read(r20, WIN)
            if not win.startswith(bytes.fromhex(pin10)):
                problems.append(f"own {name}: 2.7.2.0 prologue {win.hex(' ')} does not start with the hook prologue {pin10}")
            bad = [r for r in relocs if r20 - 8 < r < r20 + WIN]
            if bad:
                problems.append(f"own {name}: base relocation inside the code window")
            sites20.append((name, r20, f"Check::Code(&[{hexb(win)}])", "Own", "Verified"))
            sites10.append((name, r10, f"Check::Code(&[{hexb(bytes.fromhex(pin10))}])", "Own", "Verified"))
        elif kind == "data":
            hits = pe.find(sig)
            targets = {h + 10 + struct.unpack("<i", pe.read(h + 6, 4))[0] for h in hits}
            if len(hits) != 1 or targets != {r20}:
                problems.append(f"own {name}: signature hits {[hex(h) for h in hits]} -> {[hex(t) for t in targets]} (want 0x{r20:x})")
            s = pe.sec_of(r20)
            if not s or s[0] != ".data" or pe.kind(r20) != "Writable" or pe.read(r20, 1) not in (b"\x00", b"\x01"):
                problems.append(f"own {name}: 0x{r20:x} is not a bool in the writable .data")
            sites20.append((name, r20, "Check::Writable", "Own", "Verified"))
            sites10.append((name, r10, "Check::Writable", "Own", "Verified"))
        else:
            want = kind.split(":", 1)[1]
            got = pe.rtti_name(r20)
            if got != want:
                problems.append(f"own {name}: RTTI at 0x{r20:x} is {got} (want {want})")
            sites20.append((name, r20, f'Check::Vmt("{want}")', "Own", "Verified"))
            sites10.append((name, r10, f'Check::Vmt("{want}")', "Own", "Verified"))
    report.append(f"2.7.2.0: {len(sites20)} sites ({sum(1 for s in sites20 if s[2].startswith('Check::Code'))} code, "
                  f"{sum(1 for s in sites20 if s[2].startswith('Check::Vmt'))} vmt, "
                  f"{sum(1 for s in sites20 if s[4] == 'Inferred')} inferred); "
                  f"2.7.1.0: {len(sites10)} sites; relocs={len(relocs)} dynbase={(pe.dll_chars & 0x40) != 0}")

    def table(sites):
        return "\n".join(f'    Site {{ name: "{n}", rva: 0x{r:x}, check: {c}, group: Group::{g}, conf: Conf::{cf} }},'
                         for n, r, c, g, cf in sites)

    src = f"""//! GENERATED by tools/scripts/exe_sites.py --write. DO NOT EDIT (check_c3 compares it with a fresh generation).
//!
//! The boot preflight's site tables (`crate::exe_build`): every eldenring.exe address mw2er depends on, per build.
//! 2.7.2.0: from the exe file sha256 {EXE_2720_SHA256} ({EXE_2720_SIZE} bytes, read only):
//! crate RVAs = vendor rva_ww2720.rs (patch 0004), code windows = the file's first {WIN} bytes, vtables = their RTTI
//! names; own sites re-found by signature / RTTI. 2.7.1.0 (exe no longer on disk): the vendored rva_ww.rs, own
//! prologues pinned from that exe file earlier, profile RTTI class names; other crate entries section kind only.

use crate::exe_build::{{Check, Conf, Group, Site}};

/// sha256 of the eldenring.exe 2.7.2.0 file the 2.7.2.0 table was generated from.
pub const EXE_2720_SHA256: &str = "{EXE_2720_SHA256}";

/// eldenring.exe 2.7.2.0 (Steam build 25827938, 2026-10-20 update).
pub const SITES_2720: &[Site] = &[
{table(sites20)}
];

/// eldenring.exe 2.7.1.0 (Elden Ring 1.17.1, Steam build 25080141).
pub const SITES_2710: &[Site] = &[
{table(sites10)}
];
"""
    return src, report, problems


def inputs():
    data = EXE.read_bytes()
    return data, parse_table(TABLE_2710), parse_table(TABLE_2720), profile_vmts()


def main(argv):
    mode = argv[1] if len(argv) > 1 else "--check"
    if mode == "--self-test":
        return self_test()
    data, t10, t20, vmts = inputs()
    sha = hashlib.sha256(data).hexdigest()
    if sha != EXE_2720_SHA256 or len(data) != EXE_2720_SIZE:
        print(f"EXE SITES FAIL eldenring.exe on disk is sha256 {sha} ({len(data)} bytes), not the 2.7.2.0 file {EXE_2720_SHA256[:12]}")
        return 1
    src, report, problems = build(data, t10, t20, vmts)
    for line in report:
        print(line)
    if problems:
        for p in problems:
            print("PROBLEM", p)
        print(f"EXE SITES FAIL {len(problems)} problems")
        return 1
    if mode == "--write":
        OUT.write_bytes(src.encode("utf-8"))
        print(f"wrote {OUT} sha256 {hashlib.sha256(src.encode()).hexdigest()[:12]}")
    on_disk = OUT.read_bytes().decode("utf-8") if OUT.exists() else ""
    if on_disk.replace("\r\n", "\n") != src:
        print(f"EXE SITES FAIL {OUT.name} differs from a fresh generation (run --write after reviewing)")
        return 1
    print(f"EXE SITES PASS exe 2.7.2.0 sha {sha[:12]}; {report[0]}; {OUT.name} == fresh generation")
    return 0


def self_test():
    data, t10, t20, vmts = inputs()
    _, _, base = build(data, t10, t20, vmts)
    if base:
        print(f"self-test FAIL: baseline has problems {base[:3]}")
        return 1
    muts = {
        "crate code rva +0x10": (data, t10, {**t20, "cs_phys_world_cast_ray": t20["cs_phys_world_cast_ray"] + 0x10}, vmts),
        "crate rva = old cast_ray": (data, t10, {**t20, "cs_phys_world_cast_ray": 0xc71e00}, vmts),
        "vmt rva +8": (data, t10, {**t20, "chr_cam_vmt": t20["chr_cam_vmt"] + 8}, vmts),
        "vmt = old chr_ins": (data, t10, {**t20, "chr_ins_vmt": t10["chr_ins_vmt"]}, vmts),
        "profile class renamed": (data, t10, t20, {**vmts, "chr_cam_vmt": "CS::ChrCamX"}),
        "field missing in 2.7.2.0": (data, t10, {k: v for k, v in t20.items() if k != "game_man"}, vmts),
        "unresolved rva": (data, t10, {**t20, "game_man": 0x7fff_0000}, vmts),
    }
    # an exe byte flip inside the camera-step signature
    flip = bytearray(data)
    pe = Pe(data)
    s = pe.sec_of(0x3bc5f0)
    flip[s[3] + 0x3bc5f0 - s[1] + 20] ^= 0xFF
    muts["camera-step body byte flipped"] = (bytes(flip), t10, t20, vmts)
    missed = [k for k, args in muts.items() if not build(*args)[2]]
    if missed:
        print(f"self-test FAIL: not caught: {missed}")
        return 1
    print(f"PASS self-test: {len(muts)} mutations caught")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
