"""Semantic diff of Elden Ring PARAM binaries (two folders of *.param files).

Reads each .param with the SoulsFormats PARAM layout used by Elden Ring (Format2D 0x85: offset param type,
long data offsets; row names UTF-16) and compares, per param:
  - header: param type, Unk06, ParamdefDataVersion, Format2D/2E, ParamdefFormatVersion
  - row ids (order included), raw row data bytes, row names (NULL name == empty name, see below)
Byte layout differences that carry no data are ignored and counted: the header's strings-offset field
(vanilla points 8 bytes past the type string, SoulsFormats writes the type string offset), and NULL row-name
offsets that SoulsFormats rewrites as an empty UTF-16 name.

Usage:
  python -I param_diff.py <dirA> <dirB> [--expect-changed Param1,Param2 ...]
                          [--expect-added Param=id-id,id ...] [--json out.json]
--expect-changed P   P may differ in any way.
--expect-added P=ids  (strict) the ONLY allowed difference in P is exactly that set of added rows: no vanilla
                     row changed, removed or reordered, the added rows carry those ids, B is sorted by id.
                     Repeatable. Every param not named by either option must be same-data.
Exit code 0 = no unexpected differences; 1 = unexpected differences (printed); 2 = usage / parse error.
Pure Python, no third-party modules; reads only, writes only the optional --json report.
"""
import json
import pathlib
import struct
import sys


class ParamError(Exception):
    pass


def _cstr_ascii(b, off):
    end = b.index(b"\0", off)
    return b[off:end].decode("ascii")


def _cstr_utf16(b, off):
    i = off
    while i + 1 < len(b):
        if b[i] == 0 and b[i + 1] == 0:
            return b[off:i].decode("utf-16-le")
        i += 2
    raise ParamError(f"unterminated UTF-16 name at 0x{off:x}")


def read_param(path):
    b = pathlib.Path(path).read_bytes()
    if len(b) < 0x40:
        raise ParamError("file too small")
    if b[0x2C] != 0:
        raise ParamError("big-endian PARAM not supported")
    f2d, f2e, pdfv = b[0x2D], b[0x2E], b[0x2F]
    strings_off, = struct.unpack_from("<I", b, 0)
    unk04, unk06, pdver, nrows = struct.unpack_from("<HhhH", b, 4)
    if not (f2d & 0x80 and f2d & 0x04):
        raise ParamError(f"unsupported Format2D 0x{f2d:02x} (expected offset-type + long offsets)")
    zero, type_off = struct.unpack_from("<iq", b, 0x0C)
    ptype = _cstr_ascii(b, type_off)
    data_start, = struct.unpack_from("<q", b, 0x30)
    unicode_names = bool(f2e & 0x01)
    rows = []
    pos = 0x40
    for _ in range(nrows):
        rid, pad, doff, noff = struct.unpack_from("<iiqq", b, pos)
        pos += 0x18
        rows.append([rid, doff, noff])
    # Row size: uniform stride between data offsets; data ends before the type string / names.
    name_offs = [r[2] for r in rows if r[2] > 0]
    data_end = min([type_off] + name_offs)
    if nrows >= 2:
        stride = rows[1][1] - rows[0][1]
        for i in range(1, nrows):
            if rows[i][1] - rows[i - 1][1] != stride:
                raise ParamError("non-uniform row stride")
    elif nrows == 1:
        stride = data_end - rows[0][1]
    else:
        stride = 0
    if nrows and rows[0][1] != data_start:
        raise ParamError(f"first row data 0x{rows[0][1]:x} != data start 0x{data_start:x}")
    if nrows and rows[-1][1] + stride > data_end:
        raise ParamError("row data overlaps strings")
    out_rows = []
    null_names = 0
    for rid, doff, noff in rows:
        data = b[doff:doff + stride]
        if noff == 0:
            name = None
            null_names += 1
        else:
            name = _cstr_utf16(b, noff) if unicode_names else _cstr_ascii(b, noff)
        out_rows.append((rid, name, data))
    return {
        "type": ptype, "unk04": unk04, "unk06": unk06, "paramdef_data_version": pdver,
        "format2d": f2d, "format2e": f2e, "paramdef_format_version": pdfv,
        "row_size": stride, "strings_offset_field": strings_off, "type_offset": type_off,
        "null_names": null_names, "rows": out_rows, "size": len(b),
    }


def compare_param(a, b):
    """Return (list of data differences, list of layout-only notes)."""
    diffs, notes = [], []
    for k in ("type", "unk04", "unk06", "paramdef_data_version", "format2d", "format2e",
              "paramdef_format_version", "row_size"):
        if a[k] != b[k]:
            diffs.append(f"header {k}: {a[k]!r} -> {b[k]!r}")
    ida = [r[0] for r in a["rows"]]
    idb = [r[0] for r in b["rows"]]
    if ida != idb:
        sa, sb = set(ida), set(idb)
        added = sorted(sb - sa)
        removed = sorted(sa - sb)
        if added:
            diffs.append(f"rows added: {len(added)} {added[:20]}")
        if removed:
            diffs.append(f"rows removed: {len(removed)} {removed[:20]}")
        if not added and not removed:
            diffs.append("row order differs")
        if idb != sorted(idb):
            diffs.append("rows in B are not sorted by id")
    ma = {}
    for rid, name, data in a["rows"]:
        ma.setdefault(rid, []).append((name, data))
    mb = {}
    for rid, name, data in b["rows"]:
        mb.setdefault(rid, []).append((name, data))
    changed_data, changed_name = [], []
    for rid in ma:
        if rid not in mb:
            continue
        for (na, da), (nb, db) in zip(ma[rid], mb[rid]):
            if da != db:
                changed_data.append(rid)
            if (na or "") != (nb or ""):
                changed_name.append(rid)
            elif na != nb:
                notes.append("null-vs-empty name")
    if changed_data:
        diffs.append(f"row data changed: {len(changed_data)} rows {changed_data[:20]}")
    if changed_name:
        diffs.append(f"row names changed: {len(changed_name)} rows {changed_name[:20]}")
    if a["strings_offset_field"] != b["strings_offset_field"]:
        notes.append("strings-offset header field")
    if a["size"] != b["size"]:
        notes.append("file size")
    return diffs, sorted(set(notes))


HEADER_KEYS = ("type", "unk04", "unk06", "paramdef_data_version", "format2d", "format2e",
               "paramdef_format_version", "row_size")


def parse_id_set(spec):
    """'79000100-79000105,79000900' -> {79000100, ..., 79000105, 79000900}"""
    ids = set()
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part[1:]:
            lo, hi = part.split("-", 1)
            ids.update(range(int(lo), int(hi) + 1))
        else:
            ids.add(int(part))
    return ids


def check_added(a, b, expected_ids):
    """Strict check that B == A plus exactly the rows `expected_ids`. Returns a list of problems ([] = OK).

    - header fields equal;
    - B's ids are sorted (non-decreasing) and contain each expected id exactly once more than A (A has none);
    - removing the added rows from B leaves A's rows unchanged, in A's order (id, name, data);
    - nothing else was added or removed.
    """
    problems = []
    for k in HEADER_KEYS:
        if a[k] != b[k]:
            problems.append(f"header {k}: {a[k]!r} -> {b[k]!r}")
    ida = [r[0] for r in a["rows"]]
    idb = [r[0] for r in b["rows"]]
    if any(idb[i] > idb[i + 1] for i in range(len(idb) - 1)):
        bad = next(i for i in range(len(idb) - 1) if idb[i] > idb[i + 1])
        problems.append(f"rows in B are not sorted by id (at index {bad}: {idb[bad]} > {idb[bad + 1]})")
    present_a = set(ida)
    clash = sorted(present_a & expected_ids)
    if clash:
        problems.append(f"expected-added ids already exist in A: {clash[:10]}")
    count_b = {}
    for i in idb:
        count_b[i] = count_b.get(i, 0) + 1
    missing = sorted(i for i in expected_ids if count_b.get(i, 0) == 0)
    dup = sorted(i for i in expected_ids if count_b.get(i, 0) > 1)
    if missing:
        problems.append(f"expected rows missing in B: {missing}")
    if dup:
        problems.append(f"expected rows duplicated in B: {dup}")
    rest_b = [r for r in b["rows"] if r[0] not in expected_ids]
    if len(rest_b) != len(a["rows"]):
        extra = sorted(set(r[0] for r in rest_b) - present_a)
        gone = sorted(present_a - set(r[0] for r in rest_b))
        problems.append(f"unexpected row count change: A {len(a['rows'])}, B minus added {len(rest_b)}; "
                        f"unexpected added {extra[:10]}, removed {gone[:10]}")
    else:
        changed = []
        for (ia, na, da_), (ib, nb, db_) in zip(a["rows"], rest_b):
            if ia != ib:
                problems.append(f"vanilla row order changed at id {ia} (B has {ib})")
                break
            if da_ != db_ or (na or "") != (nb or ""):
                changed.append(ia)
        if changed:
            problems.append(f"vanilla rows changed: {len(changed)} {changed[:10]}")
    return problems


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    da, db = pathlib.Path(argv[0]), pathlib.Path(argv[1])
    expect = set()
    expect_added = {}
    jout = None
    rest = argv[2:]
    while rest:
        a = rest.pop(0)
        if a == "--expect-changed":
            expect |= {x for x in rest.pop(0).split(",") if x}
        elif a == "--expect-added":
            name, _, spec = rest.pop(0).partition("=")
            try:
                expect_added.setdefault(name, set()).update(parse_id_set(spec))
            except ValueError:
                print(f"bad --expect-added {name}={spec}")
                return 2
        elif a == "--json":
            jout = rest.pop(0)
        else:
            print(f"unknown option {a}")
            return 2
    fa = {p.name for p in da.glob("*.param")}
    fb = {p.name for p in db.glob("*.param")}
    report = {"dir_a": str(da), "dir_b": str(db), "only_a": sorted(fa - fb), "only_b": sorted(fb - fa),
              "params": {}}
    unexpected = []
    if report["only_a"]:
        unexpected.append(f"params missing in B: {report['only_a']}")
    if report["only_b"]:
        unexpected.append(f"params only in B: {report['only_b']}")
    stats = {"identical_bytes": 0, "same_data": 0, "data_differs": 0, "layout_only": 0}
    total_rows = 0
    for name in sorted(fa & fb):
        pa, pb = da / name, db / name
        stem = name[:-len(".param")]
        if stem in expect_added:
            try:
                a, b = read_param(pa), read_param(pb)
            except (ParamError, ValueError, struct.error) as e:
                unexpected.append(f"{stem}: parse error {e}")
                report["params"][stem] = {"status": "parse-error", "error": str(e)}
                continue
            total_rows += len(a["rows"])
            problems = check_added(a, b, expect_added[stem])
            stats["data_differs"] += 1
            if problems:
                unexpected.append(f"{stem}: " + "; ".join(problems))
                report["params"][stem] = {"status": "added-rows-wrong", "diffs": problems}
            else:
                report["params"][stem] = {"status": "added-rows-exact",
                                          "added": sorted(expect_added[stem]), "rows_a": len(a["rows"]),
                                          "rows_b": len(b["rows"])}
            continue
        if pa.read_bytes() == pb.read_bytes():
            stats["identical_bytes"] += 1
            stats["same_data"] += 1
            total_rows += len(read_param(pa)["rows"])
            report["params"][stem] = {"status": "identical-bytes"}
            continue
        try:
            a, b = read_param(pa), read_param(pb)
        except (ParamError, ValueError, struct.error) as e:
            unexpected.append(f"{stem}: parse error {e}")
            report["params"][stem] = {"status": "parse-error", "error": str(e)}
            continue
        total_rows += len(a["rows"])
        diffs, notes = compare_param(a, b)
        if diffs:
            stats["data_differs"] += 1
            report["params"][stem] = {"status": "data-differs", "diffs": diffs, "layout_notes": notes}
            if stem not in expect:
                unexpected.append(f"{stem}: " + "; ".join(diffs))
        else:
            stats["same_data"] += 1
            stats["layout_only"] += 1
            report["params"][stem] = {"status": "layout-only", "layout_notes": notes}
    for stem in sorted(expect):
        st = report["params"].get(stem, {}).get("status")
        if st != "data-differs":
            unexpected.append(f"{stem}: expected to change but status is {st}")
    for stem in sorted(expect_added):
        if stem not in report["params"]:
            unexpected.append(f"{stem}: expected added rows but the param is missing")
    report["stats"] = stats
    report["rows_compared"] = total_rows
    report["unexpected"] = unexpected
    print(f"params A={len(fa)} B={len(fb)} common={len(fa & fb)} rows_compared={total_rows}")
    print("stats: " + ", ".join(f"{k}={v}" for k, v in stats.items()))
    for stem, r in report["params"].items():
        if r["status"] == "data-differs":
            tag = "EXPECTED" if stem in expect else "UNEXPECTED"
            print(f"  {tag} {stem}: " + "; ".join(r["diffs"]))
        elif r["status"] == "added-rows-exact":
            print(f"  EXPECTED {stem}: exactly {len(r['added'])} rows added "
                  f"({r['rows_a']} -> {r['rows_b']}), every vanilla row unchanged and in order")
        elif r["status"] == "added-rows-wrong":
            print(f"  UNEXPECTED {stem}: " + "; ".join(r["diffs"]))
    notes_count = {}
    for r in report["params"].values():
        for n in r.get("layout_notes", []):
            notes_count[n] = notes_count.get(n, 0) + 1
    if notes_count:
        print("layout-only notes (no data change): " + ", ".join(f"{k} x{v}" for k, v in sorted(notes_count.items())))
    if jout:
        pathlib.Path(jout).write_text(json.dumps(report, indent=1), encoding="utf-8")
    if unexpected:
        print("FAIL: unexpected differences:")
        for u in unexpected:
            print("  " + u)
        return 1
    print("PASS: no unexpected differences")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
