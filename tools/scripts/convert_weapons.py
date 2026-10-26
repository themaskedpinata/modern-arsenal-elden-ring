"""Convert IW4 (MW2) WEAPONFILE dumps into converted/weapons.json for the mw2er DLL.

Usage:
  python -I convert_weapons.py [--dumps DIR] [--out FILE] [--weapons a,b,c] [--verify]
  python -I convert_weapons.py --self-test

Reads the user's own dumps (dumps/common_mp/weapons/<name>, written by the OAT unlinker) and writes the
fields the mod needs: an `mw2` object under the ORIGINAL WEAPONFILE key names (MW2 units: inches,
seconds, degrees) and a derived `si` object (metres via x0.0254, rounds per minute). The output lives in
the gitignored converted/ folder; nothing from MW2 is compiled into the DLL.

Format facts (verified on the M2 dumps): `WEAPONFILE\\key\\value\\...`, tokens separated by backslash
only (values may contain newlines), no trailing newline, 671 key/value pairs per file.

--verify   checks the MODLOG M1 known answers on the written file (UMP45, ACR, Intervention).
--self-test  parses synthetic strings (made-up values) and proves the parser rejects bad input.
Exit code 0 = OK, 1 = failure. Pure Python, no third-party modules.
"""
import hashlib
import json
import math
import os
import pathlib
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_DUMPS = ROOT / "dumps" / "common_mp" / "weapons"
DEFAULT_OUT = ROOT / "converted" / "weapons.json"
DEFAULT_WEAPONS = ["ump45_mp", "masada_mp", "cheytac_mp"]
EXPECTED_FIELDS = 671
INCH_TO_M = 0.0254

# Keys copied into the `mw2` object, by type. Missing required keys are an error.
STRING_KEYS = ["displayName", "fireType", "penetrateType", "fireSoundPlayer", "weaponClass", "weaponType",
               "inventoryType"]
INT_KEYS = ["damage", "minDamage", "clipSize", "maxAmmo", "startAmmo", "shotCount"]
NUM_KEYS = ["maxDamageRange", "minDamageRange", "fireTime", "reloadTime", "reloadEmptyTime", "reloadAddTime",
            "rechamberTime", "hipSpreadStandMin", "hipSpreadMax", "hipSpreadDecayRate", "hipSpreadFireAdd",
            "hipSpreadMoveAdd", "hipSpreadTurnAdd", "adsSpread", "reticleSideSize", "reticleCenterSize",
            "lowAmmoWarningThreshold", "locHead", "locHelmet"]
REQUIRED = ["displayName", "fireType", "damage", "minDamage", "maxDamageRange", "minDamageRange", "fireTime",
            "clipSize", "maxAmmo", "reloadTime", "reloadEmptyTime", "hipSpreadStandMin", "hipSpreadMax",
            "hipSpreadDecayRate", "hipSpreadFireAdd", "reticleSideSize", "lowAmmoWarningThreshold"]

# MODLOG M1 oracle values (test data for --verify; the DLL never contains these).
KNOWN_ANSWERS = {
    "ump45_mp": {"damage": 40, "minDamage": 35, "maxDamageRange": 600, "minDamageRange": 850, "fireTime": 0.09,
                 "clipSize": 32, "maxAmmo": 192, "reloadTime": 2.5, "reloadEmptyTime": 3.03,
                 "hipSpreadStandMin": 2, "hipSpreadMax": 5, "fireType": "Full Auto",
                 "si.maxDamageRange_m": 15.24, "si.minDamageRange_m": 21.59},
    "masada_mp": {"damage": 30, "minDamage": 20, "fireTime": 0.077, "clipSize": 30},
    "cheytac_mp": {"damage": 70, "clipSize": 5, "fireType": "Single Shot"},
}


class WeaponFileError(Exception):
    pass


def parse_weaponfile(text):
    """Return an ordered list of (key, value) pairs. Raises WeaponFileError on malformed input."""
    tokens = text.split("\\")
    if not tokens or tokens[0] != "WEAPONFILE":
        raise WeaponFileError("does not start with WEAPONFILE")
    rest = tokens[1:]
    if len(rest) % 2:
        raise WeaponFileError(f"odd number of key/value tokens ({len(rest)})")
    pairs, seen = [], set()
    for i in range(0, len(rest), 2):
        k, v = rest[i], rest[i + 1]
        if not k:
            raise WeaponFileError(f"empty key at pair {i // 2}")
        if k in seen:
            raise WeaponFileError(f"duplicate key {k}")
        seen.add(k)
        pairs.append((k, v))
    return pairs


def read_text(path):
    raw = path.read_bytes()
    try:
        return raw.decode("utf-8"), raw
    except UnicodeDecodeError:
        return raw.decode("latin-1"), raw


def number(key, value):
    try:
        x = float(value.strip())
    except ValueError:
        raise WeaponFileError(f"{key}={value!r} is not a number") from None
    if not math.isfinite(x):
        raise WeaponFileError(f"{key}={value!r} is not finite")
    return x


def compact(x):
    """Integral floats as int (600 rather than 600.0), others unchanged."""
    return int(x) if float(x).is_integer() else x


def convert_one(name, pairs):
    kv = dict(pairs)
    missing = [k for k in REQUIRED if k not in kv]
    if missing:
        raise WeaponFileError(f"{name}: missing required keys {missing}")
    mw2 = {}
    for k in STRING_KEYS:
        if k in kv:
            mw2[k] = kv[k]
    for k in INT_KEYS:
        if k in kv:
            x = number(k, kv[k])
            if not x.is_integer():
                raise WeaponFileError(f"{name}: {k}={kv[k]!r} is not an integer")
            mw2[k] = int(x)
    for k in NUM_KEYS:
        if k in kv:
            mw2[k] = compact(number(k, kv[k]))
    fire_time = float(mw2["fireTime"])
    si = {
        "maxDamageRange_m": round(mw2["maxDamageRange"] * INCH_TO_M, 9),
        "minDamageRange_m": round(mw2["minDamageRange"] * INCH_TO_M, 9),
        "fireRate_rpm": round(60.0 / fire_time, 3) if fire_time > 0 else 0.0,
    }
    return {"mw2": mw2, "si": si}


def convert(dumps, names):
    out = {"schema": 1, "generator": "convert_weapons.py", "inch_to_m": INCH_TO_M, "source": {}, "weapons": {}}
    for name in names:
        path = dumps / name
        if not path.is_file():
            raise WeaponFileError(f"dump not found: {path}")
        text, raw = read_text(path)
        pairs = parse_weaponfile(text)
        if len(pairs) != EXPECTED_FIELDS:
            print(f"WARN {name}: {len(pairs)} fields (expected {EXPECTED_FIELDS})")
        try:
            rel = path.resolve().relative_to(ROOT).as_posix()
        except ValueError:
            rel = str(path)
        out["source"][name] = {"file": rel, "sha256": hashlib.sha256(raw).hexdigest(), "fields": len(pairs)}
        out["weapons"][name] = convert_one(name, pairs)
    return out


def write_atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
        f.write(data)
    os.replace(tmp, path)


def verify(path):
    data = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    problems = []
    if data.get("schema") != 1 or data.get("inch_to_m") != INCH_TO_M:
        problems.append("schema / inch_to_m wrong")
    for name, answers in KNOWN_ANSWERS.items():
        w = data.get("weapons", {}).get(name)
        if w is None:
            problems.append(f"{name} missing")
            continue
        for key, want in answers.items():
            obj, k = (w["si"], key[3:]) if key.startswith("si.") else (w["mw2"], key)
            got = obj.get(k)
            ok = (abs(float(got) - float(want)) < 1e-9) if isinstance(want, (int, float)) and got is not None \
                else got == want
            if not ok:
                problems.append(f"{name}.{key}: got {got!r}, want {want!r}")
        for k in ("maxDamageRange", "minDamageRange"):
            if abs(w["si"][k + "_m"] - w["mw2"][k] * INCH_TO_M) > 1e-6:
                problems.append(f"{name}: si.{k}_m is not mw2.{k} x 0.0254")
    for p in problems:
        print("FAIL " + p)
    if not problems:
        n = sum(len(a) for a in KNOWN_ANSWERS.values())
        print(f"PASS known answers: {n} values over {len(KNOWN_ANSWERS)} weapons")
    return not problems


def self_test():
    good = "WEAPONFILE\\displayName\\WEAPON_TEST\\hideTags\\tag_a\ntag_b\n\\damage\\12\\fireTime\\0.25"
    pairs = parse_weaponfile(good)
    assert dict(pairs)["hideTags"] == "tag_a\ntag_b\n", "newline inside a value must be kept"
    assert len(pairs) == 4
    bad_cases = {
        "odd token count": "WEAPONFILE\\a\\1\\b",
        "duplicate key": "WEAPONFILE\\a\\1\\a\\2",
        "missing header": "WEAPONFIL\\a\\1",
        "empty key": "WEAPONFILE\\\\1",
    }
    for label, text in bad_cases.items():
        try:
            parse_weaponfile(text)
        except WeaponFileError:
            continue
        raise AssertionError(f"self-test: {label} was accepted")
    # A parsed file without a required key must be rejected by the converter.
    try:
        convert_one("testgun_mp", parse_weaponfile("WEAPONFILE\\displayName\\X\\damage\\10"))
    except WeaponFileError:
        pass
    else:
        raise AssertionError("self-test: missing required keys were accepted")
    # Synthetic complete weapon converts with metric values.
    kv = {k: "1" for k in REQUIRED}
    kv.update({"displayName": "WEAPON_TEST", "fireType": "Full Auto", "maxDamageRange": "100",
               "minDamageRange": "200", "fireTime": "0.5", "lowAmmoWarningThreshold": "0.25"})
    text = "WEAPONFILE\\" + "\\".join(f"{k}\\{v}" for k, v in kv.items())
    w = convert_one("testgun_mp", parse_weaponfile(text))
    assert abs(w["si"]["maxDamageRange_m"] - 2.54) < 1e-9 and w["si"]["fireRate_rpm"] == 120.0
    assert w["mw2"]["fireTime"] == 0.5 and w["mw2"]["damage"] == 1
    print("PASS self-test: 1 good string parsed, 5 bad inputs rejected, metric conversion checked")
    return True


def main(argv):
    dumps, out, names, do_verify = DEFAULT_DUMPS, DEFAULT_OUT, list(DEFAULT_WEAPONS), False
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--self-test":
            try:
                return 0 if self_test() else 1
            except AssertionError as e:
                print(f"FAIL {e}")
                return 1
        elif a == "--dumps":
            dumps = pathlib.Path(args.pop(0))
        elif a == "--out":
            out = pathlib.Path(args.pop(0))
        elif a == "--weapons":
            names = [n for n in args.pop(0).split(",") if n]
        elif a == "--verify":
            do_verify = True
        else:
            print(__doc__)
            return 2
    try:
        data = convert(dumps, names)
    except WeaponFileError as e:
        print(f"FAIL {e}")
        return 1
    write_atomic(out, json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    for n, w in data["weapons"].items():
        m, s = w["mw2"], w["si"]
        print(f"{n}: {m['fireType']} dmg {m['damage']}/{m['minDamage']} range {m['maxDamageRange']}/"
              f"{m['minDamageRange']} in = {s['maxDamageRange_m']}/{s['minDamageRange_m']} m, fireTime "
              f"{m['fireTime']} ({s['fireRate_rpm']} rpm), clip {m['clipSize']}, maxAmmo {m['maxAmmo']}, reload "
              f"{m['reloadTime']}/{m['reloadEmptyTime']} s, hip spread {m['hipSpreadStandMin']}-{m['hipSpreadMax']}")
    print(f"wrote {out}")
    if do_verify and not verify(out):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
