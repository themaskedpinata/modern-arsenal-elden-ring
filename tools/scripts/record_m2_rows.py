"""Record the M2 regulation build as hashes (M4 prerequisite, run ONCE while build/package holds the M2 package).

Writes tools/scripts/fixtures/m2_regulation_hashes.json: the M2 package sha256, the sha256 of each of its 194 unpacked
.param files and of each of the 13 M2 rows (id, name and row bytes; Bullet 79000100-105 + 79000900, AtkParam_Pc
79000100-105). Only hashes are stored (no game data). check_m3b.ps1 check 8 ("M2 rows unchanged", via
m2_static_checks.py m2rows) compares every later build with it.

Usage: python -I record_m2_rows.py   (refuses unless work/regulation/build_check is the read-back of the M2 package)
"""
import hashlib
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import param_diff  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
M2_PACKAGE_SHA = "8f376e5af59c3938d3b248f03523f90e0cd1fe67a309efe5ceea005dab525e6b"
CHECK = ROOT / "work" / "regulation" / "build_check"
OUT = pathlib.Path(__file__).resolve().parent / "fixtures" / "m2_regulation_hashes.json"
M2_ROWS = {"Bullet": list(range(79000100, 79000106)) + [79000900], "AtkParam_Pc": list(range(79000100, 79000106))}


def row_hash(rid, name, data):
    return hashlib.sha256(str(rid).encode() + b"|" + (name or "").encode("utf-8") + b"|" + data).hexdigest()


def main():
    got = hashlib.sha256((CHECK / "regulation.bin").read_bytes()).hexdigest()
    if got != M2_PACKAGE_SHA:
        print(f"FAIL {CHECK / 'regulation.bin'} is {got[:12]}, not the M2 package {M2_PACKAGE_SHA[:12]}")
        return 1
    params = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((CHECK / "regulation-bin").glob("*.param"))}
    rows = {}
    for stem, ids in M2_ROWS.items():
        prm = param_diff.read_param(CHECK / "regulation-bin" / f"{stem}.param")
        by = {r[0]: r for r in prm["rows"]}
        rows[stem] = {str(i): row_hash(*by[i]) for i in ids}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"schema": 1, "package_sha256": M2_PACKAGE_SHA, "params": params, "rows": rows}, indent=1) + "\n",
                   encoding="utf-8", newline="\n")
    print(f"PASS recorded {len(params)} param hashes and {sum(len(v) for v in rows.values())} M2 row hashes -> {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
