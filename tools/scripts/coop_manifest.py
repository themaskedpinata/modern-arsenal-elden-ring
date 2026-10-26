"""Co-op C2 friend package: the hash manifest of this PC's results (hashes and sizes only, never content).

    python -I coop_manifest.py write  <out.json>            record converted\\**, build\\rows.json,
                                                          build\\package\\regulation.bin, build\\natives\\mw2er.dll
    python -I coop_manifest.py verify <manifest.json> <root> [--only converted|regulation|all]
                                                          compare <root>'s files with the manifest; exit 1 on a mismatch

The friend's setup.ps1 runs `verify` on its own package folder: every converted file and the rebuilt regulation must
be byte-identical to the host's (co-op needs the same regulation; the converted data drives the same guns).
Read-only except for `write`'s one output file.
"""
import hashlib
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXED = ["build/rows.json", "build/package/regulation.bin", "build/natives/mw2er.dll"]


def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# build/rows.json carries its build time (built_utc): compared without it.
# converted/movement.json records the absolute path of iw4sp.exe (differs per PC): compared with absolute paths cut to
# their file name.
VOLATILE = {"build/rows.json": ("built_utc",), "converted/movement.json": ()}


def _strip_abs(v):
    if isinstance(v, dict):
        return {k: _strip_abs(x) for k, x in v.items()}
    if isinstance(v, list):
        return [_strip_abs(x) for x in v]
    if isinstance(v, str) and len(v) > 3 and v[1:3] in (":\\", ":/"):
        return v.replace("/", "\\").rsplit("\\", 1)[-1]
    return v


def canon_sha256(p, drop):
    d = _strip_abs(json.loads(pathlib.Path(p).read_text(encoding="utf-8")))
    for k in drop:
        d.pop(k, None)
    return hashlib.sha256(json.dumps(d, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()


def entry(root, rel):
    p = root / rel
    if rel in VOLATILE:
        return {"canon_sha256": canon_sha256(p, VOLATILE[rel]), "drops": list(VOLATILE[rel])}
    return {"size": p.stat().st_size, "sha256": sha256(p)}


def matches(root, rel, want):
    p = root / rel
    if "canon_sha256" in want:
        try:
            return canon_sha256(p, want["drops"]) == want["canon_sha256"]
        except (OSError, ValueError):
            return False
    return p.stat().st_size == want["size"] and sha256(p) == want["sha256"]


def collect(root):
    files = {}
    conv = root / "converted"
    for p in sorted(conv.rglob("*")):
        if p.is_file():
            files[p.relative_to(root).as_posix()] = entry(root, p.relative_to(root).as_posix())
    for rel in FIXED:
        if (root / rel).is_file():
            files[rel] = entry(root, rel)
    return files


def write(out):
    files = collect(ROOT)
    for rel in FIXED:
        if rel not in files:
            print(f"FAIL {rel} missing on this PC")
            return 1
    doc = {
        "what": "MW2inER co-op friend package: sha256 of the host's converted data, regulation package, rows.json and DLL "
                "(hashes only; no game content)",
        "files": files,
        "counts": {"converted": sum(1 for k in files if k.startswith("converted/")), "total": len(files)},
    }
    pathlib.Path(out).write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(f"PASS wrote {out}: {doc['counts']['converted']} converted files + {len(FIXED)} build files")
    return 0


def verify(manifest, root, only="all"):
    want = json.loads(pathlib.Path(manifest).read_text(encoding="utf-8"))["files"]
    root = pathlib.Path(root)

    def wanted(k):
        if only == "converted":
            return k.startswith("converted/")
        if only == "regulation":
            return k in ("build/package/regulation.bin", "build/rows.json")
        return True
    keys = [k for k in sorted(want) if wanted(k)]
    bad, missing = [], []
    for k in keys:
        p = root / k
        if not p.is_file():
            missing.append(k)
            continue
        if not matches(root, k, want[k]):
            bad.append(k)
    extra = []
    if only in ("all", "converted"):
        conv = root / "converted"
        if conv.is_dir():
            have = {p.relative_to(root).as_posix() for p in conv.rglob("*") if p.is_file()}
            extra = sorted(have - set(want))
    ok = not bad and not missing
    for k in bad[:20]:
        print(f"MISMATCH {k}")
    for k in missing[:20]:
        print(f"MISSING  {k}")
    for k in extra[:20]:
        print(f"EXTRA    {k} (not in the host's manifest; harmless)")
    print(f"{'PASS' if ok else 'FAIL'} {only}: {len(keys) - len(bad) - len(missing)}/{len(keys)} files match the host's sha256"
          f" ({len(bad)} mismatched, {len(missing)} missing, {len(extra)} extra)")
    return 0 if ok else 1


def main(argv):
    if len(argv) == 2 and argv[0] == "write":
        return write(argv[1])
    if len(argv) >= 3 and argv[0] == "verify":
        only = "all"
        if len(argv) == 5 and argv[3] == "--only" and argv[4] in ("converted", "regulation", "all"):
            only = argv[4]
        return verify(argv[1], argv[2], only)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
