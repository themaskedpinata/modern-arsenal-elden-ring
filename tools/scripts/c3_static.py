"""C3 static pins (docs/C3_DESIGN.md S2 check 2): read-only byte checks of eldenring.exe and xinput1_4.dll.

Port 2.7.2.0 (MODLOG 2026-10-20): the pins exist per exe build (PINS, selected by the file's sha256; an unknown exe
FAILs). 2.7.1.0 keeps its original pins (that file is no longer on disk); 2.7.2.0's were re-found in the new file
(work/port2720/own_sites.json: the setter / base getter moved +0xe0, the filter vtable +0x20f0, the Scaleform pair
+0x16f0; the gate, caller and remap sites did not move). The addresses below are the 2.7.1.0 ones.

Never starts or writes anything; the exe and the system DLL are only READ as files.

  python -I c3_static.py            PASS / FAIL per pin
  python -I c3_static.py --self-test  the same + mutations (a wrong byte / address / ordinal must FAIL)

Pins:
- eldenring.exe sha256 (2.7.1.0, the build every C3 address belongs to).
- 0x1403a2d28: `test byte ptr [rsi+44h], 8` (F6 46 44 08), spawn_bullet's network-sync gate (bit 3) [V].
- 0x14045ff10: the proxy filter setter stores to +0x338, +0x348 and +0xC8 of CSChrPhysicsModule [V].
- Review C3: its only caller 0x1403cb1f2 (in 0x1403caeb0) passes edx = the remapped layer word; the remap starts from
  the BASE word read by the getter 0x14045ddc0 (`mov eax,[rcx+2E8h]`) masked `and ebx,7Fh`, and forces layer 3
  (`mov ebx,3` at 0x1403cb15a) for one state-flag case [V]. The base word itself is written at run time (no static
  constant found; the DLL logs it as `base2e8=` in `MOVE filter live`).
- 0x142b94df0: CSCollisionFilter's vtable: in .rdata, its first slot points into .text [V].
- 0x14115e6b0 / 0x14115e810: the Scaleform D3D12 HAL BeginScene / EndScene prologues [V] (the UI probe checks the same
  bytes at run time before hooking).
- XINPUT1_4.dll imported by ordinal 2 (and 3); xinput1_4.dll exports ordinal 2 as XInputGetState (PASS B).
"""
import hashlib
import os
import pathlib
import struct
import sys

ER_EXE = pathlib.Path(os.environ.get("MW2ER_ER_EXE", r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe"))
XINPUT = pathlib.Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "xinput1_4.dll"
BASE = 0x140000000
GATE = bytes.fromhex("f6464408")
SETTER_DISPS = (0x338, 0x348, 0xC8)
BEGIN = bytes.fromhex("40574883ec208b4158488bf9a8027508")
END = bytes.fromhex("415541564883ec48")
# review C3: the setter's caller and the layer remap it runs (see the module doc)
CALLER = bytes.fromhex("8bd3448bcfe8")
BASE_GETTER = bytes.fromhex("8b81e8020000c3")
LAYER3 = bytes.fromhex("bb03000000")
PINS = {
    "2.7.1.0": dict(sha="1a3547101327f65d0c76da2f9190ac0aa66871ea42bae2aecc61e11a8b597891",
                    gate=0x1403a2d28, setter=0x14045ff10, filter_vt=0x142b94df0, begin=0x14115e6b0, end=0x14115e810,
                    caller=0x1403cb1ed, base_getter=0x14045ddc0, base_call=0x1403caf5e, base_mask=0x1403caf68,
                    layer3=0x1403cb15a),
    "2.7.2.0": dict(sha="f9118fea2cfd8fa3901e8060bfadb1552cb2471771ded8a5b662c4e298ffb328",
                    gate=0x1403a2d28, setter=0x14045fff0, filter_vt=0x142b96ee0, begin=0x14115fda0, end=0x14115ff00,
                    caller=0x1403cb1ed, base_getter=0x14045dea0, base_call=0x1403caf5e, base_mask=0x1403caf68,
                    layer3=0x1403cb15a),
}


def pins_for(sha):
    """(version, pins) of the exe with this sha256; (None, the newest pins) for an unknown exe (its sha check FAILs)."""
    for v, p in PINS.items():
        if p["sha"] == sha:
            return v, p
    return None, PINS["2.7.2.0"]


class Pe:
    def __init__(self, data):
        self.d = data
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        nsec = struct.unpack_from("<H", data, pe + 6)[0]
        opt = pe + 24
        self.base = struct.unpack_from("<Q", data, opt + 24)[0]
        self.dirs = opt + 112
        so = struct.unpack_from("<H", data, pe + 20)[0]
        self.secs = []
        for i in range(nsec):
            o = opt + so + 40 * i
            name = data[o:o + 8].rstrip(b"\0").decode("latin1")
            vsize, va, rsize, raw = struct.unpack_from("<IIII", data, o + 8)
            ch = struct.unpack_from("<I", data, o + 36)[0]
            self.secs.append((name, va, max(vsize, rsize), raw, rsize, ch))

    def sec_of(self, rva):
        for s in self.secs:
            if s[1] <= rva < s[1] + s[2]:
                return s
        return None

    def off(self, rva):
        s = self.sec_of(rva)
        if not s or rva - s[1] >= s[4]:
            return None
        return s[3] + rva - s[1]

    def read(self, rva, n):
        o = self.off(rva)
        return None if o is None else self.d[o:o + n]

    def cstr(self, rva):
        o = self.off(rva)
        return self.d[o:self.d.index(b"\0", o)].decode("latin1") if o is not None else None

    def imports(self):
        rva = struct.unpack_from("<I", self.d, self.dirs + 8)[0]
        out = {}
        while True:
            o = self.off(rva)
            if o is None:
                break
            ilt, _, _, name, iat = struct.unpack_from("<IIIII", self.d, o)
            if name == 0:
                break
            dll = self.cstr(name).lower()
            ents = []
            t = ilt or iat
            while True:
                v = struct.unpack_from("<Q", self.d, self.off(t))[0]
                if v == 0:
                    break
                ents.append(("ord", v & 0xFFFF) if v >> 63 else ("name", self.cstr((v & 0x7FFFFFFF) + 2)))
                t += 8
            out.setdefault(dll, []).extend(ents)
            rva += 20
        return out

    def exports(self):
        rva = struct.unpack_from("<I", self.d, self.dirs)[0]
        o = self.off(rva)
        base_ord, nfun, nnames, afun, anames, aords = struct.unpack_from("<IIIIII", self.d, o + 16)
        names = {}
        for i in range(nnames):
            nrva = struct.unpack_from("<I", self.d, self.off(anames + 4 * i))[0]
            idx = struct.unpack_from("<H", self.d, self.off(aords + 2 * i))[0]
            names[base_ord + idx] = self.cstr(nrva)
        return names


def checks(exe_bytes, xin_bytes, mut=None, force=None):
    """[(name, ok, detail)]; `mut` corrupts one input for the self-test; `force` = use that build's pins."""
    res = []
    sha = hashlib.sha256(exe_bytes).hexdigest()
    ver, P = pins_for(sha)
    if force:
        ver, P = force, PINS[force]
    res.append(("eldenring.exe sha256", sha == P["sha"] and mut != "sha", f"{sha[:12]} = {ver or 'unknown build'} (pins: {', '.join(PINS)})"))
    GATE_VA, SETTER_VA, FILTER_VT_VA, BEGIN_VA, END_VA = P["gate"], P["setter"], P["filter_vt"], P["begin"], P["end"]
    CALLER_VA, BASE_GETTER_VA, BASE_CALL_VA, BASE_MASK_VA, LAYER3_VA = (P["caller"], P["base_getter"], P["base_call"],
                                                                        P["base_mask"], P["layer3"])
    pe = Pe(exe_bytes)
    gate = pe.read(GATE_VA - BASE, 4)
    if mut == "gate":
        gate = b"\xf6\x46\x44\x04"
    res.append((f"bit-3 sync gate 0x{GATE_VA:x}", gate == GATE, f"{gate.hex() if gate else None} (want f6464408 test byte [rsi+44h],8)"))
    body = pe.read(SETTER_VA - BASE, 0x180) or b""
    if mut == "setter":
        body = body.replace(struct.pack("<I", 0x338), b"\0\0\0\0")
    found = {d: struct.pack("<I", d) in body for d in SETTER_DISPS}
    res.append((f"filter setter 0x{SETTER_VA:x}", all(found.values()), f"disp32 +0x338/+0x348/+0xC8 in its first 0x180 bytes: {found}"))
    cb = pe.read(CALLER_VA - BASE, 10) or b""
    tgt = CALLER_VA + 10 + struct.unpack("<i", cb[6:10])[0] if len(cb) == 10 else 0
    if mut == "caller":
        tgt += 0x10
    res.append((f"setter caller 0x{CALLER_VA + 5:x} (edx = remapped layer word)", cb[:6] == CALLER and tgt == SETTER_VA, f"{cb.hex()} -> 0x{tgt:x}"))
    g = pe.read(BASE_GETTER_VA - BASE, len(BASE_GETTER))
    bc = pe.read(BASE_CALL_VA - BASE, 5) or b""
    btgt = BASE_CALL_VA + 5 + struct.unpack("<i", bc[1:5])[0] if len(bc) == 5 and bc[0] == 0xE8 else 0
    mask = pe.read(BASE_MASK_VA - BASE, 3)
    l3 = pe.read(LAYER3_VA - BASE, 5)
    if mut == "base":
        g = bytes.fromhex("8b8138030000c3")
    res.append((f"layer remap base word +0x2E8 (getter 0x{BASE_GETTER_VA:x}, and 7Fh, mov ebx,3)",
                g == BASE_GETTER and btgt == BASE_GETTER_VA and mask == bytes.fromhex("83e37f") and l3 == LAYER3,
                f"getter {g.hex() if g else None} call->0x{btgt:x} mask {mask.hex() if mask else None} layer3 {l3.hex() if l3 else None}"))
    vt = pe.read(FILTER_VT_VA - BASE, 8)
    sec = pe.sec_of(FILTER_VT_VA - BASE)
    slot = struct.unpack("<Q", vt)[0] if vt else 0
    tsec = pe.sec_of(slot - BASE) if slot else None
    if mut == "vtable":
        tsec = None
    res.append((f"CSCollisionFilter vtable 0x{FILTER_VT_VA:x}", bool(sec and sec[0] == ".rdata" and tsec and tsec[0] == ".text"),
                f"in {sec[0] if sec else None}, slot 0 -> 0x{slot:x} in {tsec[0] if tsec else None}"))
    b = pe.read(BEGIN_VA - BASE, len(BEGIN))
    e = pe.read(END_VA - BASE, len(END))
    if mut == "prologue":
        b = b"\x90" + b[1:]
    res.append(("Scaleform BeginScene prologue", b == BEGIN, b.hex() if b else "unreadable"))
    res.append(("Scaleform EndScene prologue", e == END, e.hex() if e else "unreadable"))
    imp = pe.imports().get("xinput1_4.dll", [])
    if mut == "import":
        imp = [x for x in imp if x != ("ord", 2)]
    res.append(("XINPUT1_4 import ordinal 2", ("ord", 2) in imp, f"{imp}"))
    if xin_bytes is None:
        res.append(("xinput1_4.dll export 2 = XInputGetState", False, f"{XINPUT} missing"))
    else:
        ex = Pe(xin_bytes).exports()
        name = ex.get(2)
        if mut == "export":
            name = "XInputSetState"
        res.append(("xinput1_4.dll export 2 = XInputGetState", name == "XInputGetState", f"ordinal 2 = {name}"))
    return res


def main(argv):
    if not ER_EXE.exists():
        print(f"FAIL eldenring.exe not found: {ER_EXE}")
        return 1
    exe = ER_EXE.read_bytes()
    xin = XINPUT.read_bytes() if XINPUT.exists() else None
    res = checks(exe, xin)
    for n, ok, d in res:
        print(f"{'PASS' if ok else 'FAIL'} {n}: {d}")
    ok = all(r[1] for r in res)
    if "--self-test" in argv:
        caught = 0
        muts = ["sha", "gate", "setter", "caller", "base", "vtable", "prologue", "import", "export"]
        for m in muts:
            if not all(r[1] for r in checks(exe, xin, m)):
                caught += 1
            else:
                print(f"MISSED mutation {m}")
        # port 2.7.2.0: the other build's pins must FAIL on this exe beyond the sha (the address tables differ)
        ver, _ = pins_for(hashlib.sha256(exe).hexdigest())
        for other in [v for v in PINS if v != ver]:
            muts.append(f"pins of {other}")
            bad = [r[0] for r in checks(exe, xin, None, other)[1:] if not r[1]]
            if bad:
                caught += 1
            else:
                print(f"MISSED mutation pins of {other}")
        print(f"self-test: {caught}/{len(muts)} mutations caught")
        ok = ok and caught == len(muts)
    print(f"{'PASS' if ok else 'FAIL'} c3_static: {sum(r[1] for r in res)}/{len(res)} pins (eldenring.exe and xinput1_4.dll read only)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
