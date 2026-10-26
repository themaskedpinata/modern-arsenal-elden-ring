"""Decode Elden Ring PARAM rows straight from the binary .param, using a Paramdex def (no XML defaults involved).

Usage:
  python -I param_rows.py <file.param> <id>[,<id>...] [--defs DIR] [--reg 11730034] [--fields a,b,c] [--offsets]
  python -I param_rows.py <file.param> --find field=value[,field=value]   (list ids of matching rows)
Defaults: --defs = WitchyBND's Paramdex ER/Defs next to this repo; --reg = 11730034 (the 2.7.2.0 regulation; 1.17.1 was 11711000, same layouts).
Fields with FirstVersion/RemovedVersion are included only when active at --reg. Bitfields pack LSB-first into
units of their type; dummy8 bitfields share u8 units (SoulsFormats rule). The computed row size is checked
against the binary's row stride before anything is printed.
"""
import pathlib
import re
import struct
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import param_diff  # noqa: E402

DEFAULT_DEFS = pathlib.Path(__file__).resolve().parents[1] / "WitchyBND" / "app" / "Assets" / "Paramdex" / "ER" / "Defs"
SIZES = {"s8": 1, "u8": 1, "s16": 2, "u16": 2, "s32": 4, "u32": 4, "f32": 4, "f64": 8, "angle32": 4, "b32": 4,
         "dummy8": 1, "fixstr": 1, "fixstrW": 2, "s64": 8, "u64": 8}
FMT = {"s8": "b", "u8": "B", "s16": "h", "u16": "H", "s32": "i", "u32": "I", "f32": "f", "f64": "d",
       "angle32": "f", "b32": "i", "s64": "q", "u64": "Q"}


def load_def(defs_dir, param_type, reg):
    for p in pathlib.Path(defs_dir).glob("*.xml"):
        t = ET.parse(p).getroot()
        if t.findtext("ParamType") != param_type:
            continue
        layout = []  # (name, type, offset, bit_offset or None, bit_size or None, count)
        off = 0
        bit_off = -1
        bit_type = None
        for f in t.find("Fields"):
            fv, rv = f.get("FirstVersion"), f.get("RemovedVersion")
            if (fv and int(fv) > reg) or (rv and int(rv) <= reg):
                continue
            m = re.match(r"(\w+)\s+(\w+)\s*(?::\s*(\d+))?\s*(?:\[(\d+)\])?", f.get("Def"))
            typ, name, bw, arr = m.group(1), m.group(2), m.group(3), m.group(4)
            if bw:
                t2 = "u8" if typ == "dummy8" else typ
                if bit_off == -1 or t2 != bit_type or bit_off + int(bw) > SIZES[typ] * 8:
                    unit_off = off
                    off += SIZES[typ]
                    bit_off = 0
                    bit_type = t2
                layout.append((name, t2, unit_off, bit_off, int(bw), 1))
                bit_off += int(bw)
            else:
                bit_off = -1
                n = int(arr) if arr else 1
                layout.append((name, typ, off, None, None, n))
                off += SIZES[typ] * n
        return p.name, layout, off
    raise SystemExit(f"no def for {param_type} in {defs_dir}")


DEF_DEFAULT_RE = re.compile(r"(\w+)\s+(\w+)\s*(?::\s*\d+)?\s*(?:\[\d+\])?\s*(?:=\s*(\S+))?")


def load_defaults(defs_dir, param_type, reg):
    """{field: default} from the Paramdex def (`Def="s32 x = -1"`; no '= v' means 0), for the fields active at
    `reg`. WitchyBND leaves attributes that equal these defaults out of its XML."""
    for p in pathlib.Path(defs_dir).glob("*.xml"):
        t = ET.parse(p).getroot()
        if t.findtext("ParamType") != param_type:
            continue
        out = {}
        for f in t.find("Fields"):
            fv, rv = f.get("FirstVersion"), f.get("RemovedVersion")
            if (fv and int(fv) > reg) or (rv and int(rv) <= reg):
                continue
            m = DEF_DEFAULT_RE.match(f.get("Def"))
            if m:
                raw = m.group(3)
                try:
                    out[m.group(2)] = float(raw) if raw is not None else 0.0
                except ValueError:
                    out[m.group(2)] = raw
        return out
    raise SystemExit(f"no def for {param_type} in {defs_dir}")


def decode(data, layout):
    out = {}
    for name, typ, off, boff, bsize, n in layout:
        if boff is not None:
            unit = int.from_bytes(data[off:off + SIZES[typ]], "little")
            out[name] = (unit >> boff) & ((1 << bsize) - 1)
        elif typ == "dummy8":
            out[name] = data[off:off + n].hex() if n > 1 else data[off]
        elif typ == "fixstr":
            out[name] = data[off:off + n].split(b"\0")[0].decode("ascii", "replace")
        elif typ == "fixstrW":
            out[name] = data[off:off + 2 * n].decode("utf-16-le", "replace").split("\0")[0]
        else:
            vals = struct.unpack_from("<" + FMT[typ] * n, data, off)
            out[name] = vals[0] if n == 1 else list(vals)
        if isinstance(out[name], float):
            out[name] = round(out[name], 6)
    return out


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    path = argv[0]
    rest = argv[1:]
    ids = None
    find = None
    defs = DEFAULT_DEFS
    reg = 11730034
    only = None
    show_off = False
    while rest:
        a = rest.pop(0)
        if a == "--defs":
            defs = pathlib.Path(rest.pop(0))
        elif a == "--reg":
            reg = int(rest.pop(0))
        elif a == "--fields":
            only = rest.pop(0).split(",")
        elif a == "--offsets":
            show_off = True
        elif a == "--find":
            find = [kv.split("=", 1) for kv in rest.pop(0).split(",")]
        else:
            ids = [int(x) for x in a.split(",") if x]
    p = param_diff.read_param(path)
    defname, layout, size = load_def(defs, p["type"], reg)
    if p["rows"] and size != p["row_size"]:
        print(f"ERROR: def {defname} row size {size} != binary row size {p['row_size']}")
        return 1
    print(f"# {pathlib.Path(path).name}: type {p['type']} def {defname} row size {size} rows {len(p['rows'])}")
    if show_off:
        for name, typ, off, boff, bsize, n in layout:
            if only and name not in only:
                continue
            extra = f" bit {boff}:{bsize}" if boff is not None else (f" x{n}" if n > 1 else "")
            print(f"  +0x{off:03x} {typ} {name}{extra}")
    if find:
        hits = []
        for rid, name, data in p["rows"]:
            row = decode(data, layout)
            if all(str(row.get(k)) == v for k, v in find):
                hits.append(rid)
        print(f"# rows where {find}: {len(hits)}")
        print(", ".join(str(h) for h in hits[:200]))
        return 0
    if ids:
        by_id = {}
        for rid, name, data in p["rows"]:
            by_id.setdefault(rid, (name, data))
        for rid in ids:
            if rid not in by_id:
                print(f"[{rid}] NOT PRESENT")
                continue
            name, data = by_id[rid]
            row = decode(data, layout)
            print(f"[{rid}] binary name={name!r}")
            for k, v in row.items():
                if only and k not in only:
                    continue
                if not only and (k.startswith("pad") or k.startswith("reserve") or k.startswith("unk")) and not v:
                    continue
                print(f"  {k} = {v}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
