"""M7 regulation guard: every row of the M6 build stays byte-identical; the only new rows are the nuke rows.

Usage:
  python -I m7_regulation_rows.py --record   run ONCE while work/regulation/build_check is the read-back of the M6 package
                                             (sha256 838f6b9b...): writes fixtures/m6_regulation_rows.json (hashes only:
                                             each of the 194 .param files, and every row of Bullet / AtkParam_Pc /
                                             SpEffectParam as sha256(id | name | row bytes); no game data)
  python -I m7_regulation_rows.py --check    the current build_check vs the fixture: all 191 untouched params
                                             byte-identical, every recorded row identical (UMP45, AC-130, arsenal,
                                             marker, protection, every vanilla row), exactly the planned nuke rows
                                             added (rows.json "nuke"), ids sorted
  python -I m7_regulation_rows.py --self-test  the check must catch a changed recorded row, a missing row, an extra
                                             row and an unsorted table
"""
import hashlib
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import param_diff  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
M6_PACKAGE_SHA = "838f6b9b7aa5"  # prefix of the M6 package (work/m6/regulation.sha256)
CHECK = ROOT / "work" / "regulation" / "build_check"
FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m6_regulation_rows.json"
ROWS_JSON = ROOT / "build" / "rows.json"
EDITED = ("Bullet", "AtkParam_Pc", "SpEffectParam")
# Port 2.7.2.0 (MODLOG 2026-10-20): the vanilla rows of the new regulation (BND version 11730034, sha256 b8cdab84c077).
# The fixture's digests are the 1.17.1 vanilla; the new vanilla differs in exactly one row of these three params,
# Bullet 20003309 (atkId_Bullet 0 -> 1, FromSoftware's change; verified row by row against the archived 1.17.1 unpack,
# work/port2720/vanilla_delta.py). Either the recorded 1.17.1 digest or this one is accepted, per param.
VANILLA_11730034 = {
    "Bullet": [15475, "71c9ca521ea22be33b2899d7396da1c476b31526caa5d298adce511878905abd"],
    "AtkParam_Pc": [11017, "229515f0a7be1de0ccf49dab3413b0e23ac572669c42d0248626cdc481e1ae15"],
    "SpEffectParam": [11354, "2078aeb1ec0a77a894a1c707c25eba9131665953ccee7fe958cc196d33f82548"],
}


def row_hash(rid, name, data):
    return hashlib.sha256(str(rid).encode() + b"|" + (name or "").encode("utf-8") + b"|" + data).hexdigest()


def read_rows(path):
    prm = param_diff.read_param(path)
    return [(r[0], row_hash(*r)) for r in prm["rows"]]


def is_ours(rid):
    return 79_000_000 <= rid <= 79_999_999


def vanilla_digest(rows):
    """(count, sha256 over the ordered vanilla row hashes)."""
    v = [f"{i}:{h}" for i, h in rows if not is_ours(i)]
    return [len(v), hashlib.sha256("\n".join(v).encode()).hexdigest()]


def record():
    got = hashlib.sha256((CHECK / "regulation.bin").read_bytes()).hexdigest()
    if not got.startswith(M6_PACKAGE_SHA):
        print(f"FAIL {CHECK / 'regulation.bin'} is {got[:12]}, not the M6 package {M6_PACKAGE_SHA}")
        return 1
    params = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((CHECK / "regulation-bin").glob("*.param"))}
    rows, vanilla = {}, {}
    for stem in EDITED:
        rr = read_rows(CHECK / "regulation-bin" / f"{stem}.param")
        rows[stem] = {str(i): h for i, h in rr if is_ours(i)}
        vanilla[stem] = vanilla_digest(rr)
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(json.dumps({"schema": 1, "package_sha256": got, "params": params, "rows": rows, "vanilla": vanilla}, indent=0) + "\n",
                       encoding="utf-8", newline="\n")
    print(f"PASS recorded {len(params)} param hashes, {sum(len(v) for v in rows.values())} mod row hashes and the vanilla row digests -> {FIXTURE}")
    return 0


def planned_new():
    r = json.loads(ROWS_JSON.read_text(encoding="utf-8"))
    n = r.get("nuke") or {}
    out = {"Bullet": {n.get("lethal_bullet_id"), n.get("horizon_bullet_id")}, "AtkParam_Pc": {n.get("lethal_bullet_id")},
           "SpEffectParam": {n.get("lethal_speffect_id")}}
    # C3 PASS C (docs/C3_DESIGN.md 3.3): the co-op twins + nuke marker bullet (Bullet only, no attack row) and the synced
    # marker SpEffects, as listed in rows.json "coop" (every M6 / M7 row stays byte-identical)
    c = r.get("coop")
    if c:
        out["Bullet"] |= {t["id"] for t in c.get("twins", [])} | {c.get("nuke_marker_bullet")}
        out["SpEffectParam"] |= {c.get("sp_mode"), c.get("sp_ac130"), c.get("sp_nuke")} | {c["sp_gun_base"] + k for k in range(c.get("sp_gun_count", 0))}
    return out


def check(tree=None, overrides=None):
    tree = tree or (CHECK / "regulation-bin")
    overrides = overrides or {}
    fx = json.loads(FIXTURE.read_text(encoding="utf-8"))
    problems = []
    for name, h in fx["params"].items():
        stem = name[:-len(".param")]
        if stem in EDITED:
            continue
        p = tree / name
        if not p.is_file() or hashlib.sha256(p.read_bytes()).hexdigest() != h:
            problems.append(f"{stem}: not byte-identical to the M6 build")
    want_new = planned_new()
    added = 0
    for stem in EDITED:
        rows = overrides.get(stem) or read_rows(tree / f"{stem}.param")
        ids = [i for i, _ in rows]
        if ids != sorted(ids):
            problems.append(f"{stem}: row ids not sorted")
        if vanilla_digest(rows) not in (fx["vanilla"][stem], VANILLA_11730034[stem]):
            problems.append(f"{stem}: the vanilla rows differ from the M6 build")
        cur = {str(i): h for i, h in rows if is_ours(i)}
        old = fx["rows"][stem]
        for rid, h in old.items():
            if rid not in cur:
                problems.append(f"{stem}[{rid}]: row missing")
            elif cur[rid] != h:
                problems.append(f"{stem}[{rid}]: row bytes / name changed")
        new = {int(r) for r in cur if r not in old}
        if new != {x for x in want_new[stem] if x}:
            problems.append(f"{stem}: new rows {sorted(new)} (want exactly the nuke rows {sorted(x for x in want_new[stem] if x)})")
        added += len(new)
    return problems, added, sum(len(v) for v in fx["rows"].values()) + sum(v[0] for v in fx["vanilla"].values())


def self_test():
    base = {stem: read_rows(CHECK / "regulation-bin" / f"{stem}.param") for stem in EDITED}
    ump = 79112000
    cases = []

    def changed():
        rows = [(i, h if i != ump else "0" * 64) for i, h in base["Bullet"]]
        return check(overrides={"Bullet": rows})[0]

    def missing():
        return check(overrides={"AtkParam_Pc": [r for r in base["AtkParam_Pc"] if r[0] != 79000201]})[0]

    def extra():
        return check(overrides={"SpEffectParam": sorted(base["SpEffectParam"] + [(79000999, "f" * 64)])})[0]

    def vanilla_changed():
        rows = [(i, h if i != 1540 else "0" * 64) for i, h in base["SpEffectParam"]]
        return check(overrides={"SpEffectParam": rows})[0]

    def unsorted():
        rows = list(base["Bullet"])
        rows[-1], rows[-2] = rows[-2], rows[-1]
        return check(overrides={"Bullet": rows})[0]

    cases = [("a changed UMP45 row", changed), ("a missing AC-130 row", missing), ("an unplanned SpEffect row", extra),
             ("an unsorted Bullet table", unsorted), ("a changed vanilla SpEffect row (1540)", vanilla_changed)]
    control = check()[0]
    if control:
        print("FAIL control: " + "; ".join(control[:3]))
        return 1
    caught = 0
    for label, fn in cases:
        p = fn()
        if p:
            caught += 1
            print(f"caught: {label} -> {p[0][:100]}")
        else:
            print(f"MISSED: {label}")
    if caught == len(cases):
        print(f"PASS self-test: {caught} mutations caught")
        return 0
    print(f"FAIL self-test: {caught}/{len(cases)}")
    return 1


def main(argv):
    if argv == ["--record"]:
        return record()
    if argv == ["--self-test"]:
        return self_test()
    if argv == ["--check"]:
        problems, added, n = check()
        if problems:
            for p in problems:
                print("FAIL " + p)
            return 1
        print(f"PASS m6rows: 191 untouched params byte-identical, all {n} M6 rows of Bullet / AtkParam_Pc / SpEffectParam "
              f"byte-identical (UMP45, AC-130, arsenal, marker, protection, vanilla), exactly the {added} planned nuke + C3 co-op rows added, sorted")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
