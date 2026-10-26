"""C6 regulation check (docs/C6_HOLD_DESIGN.md 3, 10): the crossbow stance is a RUNTIME patch, never a regulation row.

  python -I check_c6_regulation.py               all checks (read-only: the vanilla unpack, the unpacked staged package,
                                                 the staged package's sha256, the sources)
  python -I check_c6_regulation.py --self-test   the checks against injected bad inputs (each must FAIL)

Checks:
1. fields     mw2er-core er_stance.rs STANCE_FIELDS = the Paramdex offsets / widths of wepmotionCategory,
              guardmotionCategory, spAtkcategory, wepmotionOneHandId, wepmotionBothHandId; ROW_SIZE = the row size; the
              OFF_* constants = weaponCategory / wepType / arrowBoltEquipId / behaviorVariationId.
2. values     the vanilla rows with wepmotionCategory 46 are exactly the 12 wepType-55 crossbows, and the stance values
              equal theirs (spAtkcategory 0 = the plain crossbows; 4 have a per-weapon override).
3. no fire    the stance never writes a byte of weaponCategory, wepType, arrowBoltEquipId, behaviorVariationId, any
              attackBase* / spEffectBehaviorId* / bullet / atk field (the hidden weapon keeps its own attack logic and
              the crossbow shoot / reload / bolt path has no reason to run [I2]); R3 melee is suppressed while the stance
              is active (pad_logic `melee_off`) and the stray-bullet guard exists (er_stance.rs `stray_guard`); the DLL
              never forms a `&mut` param row (`get_equip_param_mut` / `get_mut::<EquipParamWeapon>`).
4. package    no stance row is in the default package: build_regulation.py edits only Bullet / AtkParam_Pc /
              SpEffectParam; the unpacked staged package (work\\regulation\\build_check) has EquipParamWeapon and
              BehaviorParam_PC byte-identical to the vanilla unpack and no EquipParamWeapon row in 79000000..79999999;
              build\\package\\regulation.bin is the C6-time package (sha256 below) and equals build_check's copy.
5. hide       (C6 hide, work/c6/hide.md) the in-memory patch of the MW2-mode marker row: mw2er-core coop_vis.rs HIDE_FIELDS
              offsets / widths = Paramdex (vowType0 / vowType8 = whole bytes of 8 vow bits); `from` = the staged package's
              row 79001090, `to` = vanilla SpEffect 1466000 "Unseen Blade" (stateInfo 184); SP_ROW_SIZE = the row size;
              the patch never writes spCategory / vfxId / effectEndurance / isDisableNetSync / iconId / effectTarget*.
There is no opt-in package for C6 (option B needs none); option A (`build_regulation.py --stance`, design 3.6) is only
built if the in-game test refutes I1.
"""
import hashlib
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import param_diff  # noqa: E402
import param_rows  # noqa: E402

VANILLA = ROOT / "work" / "regulation" / "unpacked" / "regulation-bin"
STAGED_UNPACKED = ROOT / "work" / "regulation" / "build_check"
PACKAGE = ROOT / "build" / "package" / "regulation.bin"
CORE = ROOT / "mod" / "mw2er-core" / "src" / "er_stance.rs"
CORE_VIS = ROOT / "mod" / "mw2er-core" / "src" / "coop_vis.rs"
PAD = ROOT / "mod" / "mw2er-core" / "src" / "pad_logic.rs"
DLL = ROOT / "mod" / "mw2er" / "src"
BUILD = HERE / "build_regulation.py"
# the staged package at C6 time (MODLOG 2026-10-21 C6): C6 changes no regulation byte
EXPECTED_PACKAGE_SHA = "7739289d06766baa878bea76eca3cf546d4969ea21fabe191dd2f3874b89e001"
NAMES = ["wepmotionCategory", "guardmotionCategory", "spAtkcategory", "wepmotionOneHandId", "wepmotionBothHandId"]
KEPT = {"OFF_WEAPON_CATEGORY": "weaponCategory", "OFF_WEP_TYPE": "wepType", "OFF_ARROW_BOLT": "arrowBoltEquipId",
        "OFF_BEHAVIOR_VARIATION": "behaviorVariationId"}
NO_TOUCH = re.compile(r"^(weaponCategory|wepType|arrowBoltEquipId|behaviorVariationId|attackBase\w*|spEffectBehaviorId\w*|"
                      r"\w*[Bb]ullet\w*|\w*[Aa]tk\w*|originEquipWep\w*|reinforceTypeId|wepType)$")
CROSSBOW_IDS = {43000000, 43020000, 43030000, 43050000, 43060000, 43080000, 43110000, 43500000, 43510000, 43690000, 43900000,
                99050000}


def sha(p):
    return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()


def core_consts(text):
    fields = [(n, int(o, 16), int(w), int(v)) for n, o, w, v in
              re.findall(r'Field \{ name: "(\w+)", off: (0x[0-9A-Fa-f]+), width: (\d+), value: (\d+) \}', text)]
    offs = {k: int(v, 16) for k, v in re.findall(r"pub const (OFF_\w+): usize = (0x[0-9A-Fa-f]+);", text)}
    m = re.search(r"pub const ROW_SIZE: usize = (\d+);", text)
    return fields, offs, int(m.group(1)) if m else None


def layout():
    p = param_diff.read_param(str(VANILLA / "EquipParamWeapon.param"))
    _, lay, size = param_rows.load_def(param_rows.DEFAULT_DEFS, p["type"], 11730034)
    return p, {name: (off, param_rows.SIZES[typ] * n, boff) for name, typ, off, boff, bsize, n in lay}, size


def check_fields(fields, offs, row_size, lay, size):
    probs = []
    if [f[0] for f in fields] != NAMES:
        probs.append(f"STANCE_FIELDS names {[f[0] for f in fields]} != {NAMES}")
    for n, o, w, _ in fields:
        if n in lay and (lay[n][0], lay[n][1]) != (o, w):
            probs.append(f"{n}: core +0x{o:x}/{w} != Paramdex +0x{lay[n][0]:x}/{lay[n][1]}")
    for k, name in KEPT.items():
        if offs.get(k) != lay[name][0]:
            probs.append(f"{k} = {offs.get(k)} != Paramdex {name} +0x{lay[name][0]:x}")
    if row_size != size:
        probs.append(f"ROW_SIZE {row_size} != {size}")
    return probs


def check_values(fields, p, lay):
    probs = []
    _, layf, _ = param_rows.load_def(param_rows.DEFAULT_DEFS, p["type"], 11730034)
    rows = {rid: param_rows.decode(data, layf) for rid, _, data in p["rows"]}
    motion46 = {rid for rid, r in rows.items() if r["wepmotionCategory"] == 46}
    type55 = {rid for rid, r in rows.items() if r["wepType"] == 55}
    if motion46 != type55 or motion46 != CROSSBOW_IDS:
        probs.append(f"wepmotionCategory=46 rows {sorted(motion46)} vs wepType=55 {sorted(type55)}")
    plain = [rid for rid in sorted(type55) if rows[rid]["spAtkcategory"] == 0]
    for rid in plain:
        got = [rows[rid][n] for n in NAMES]
        if got != [f[3] for f in fields]:
            probs.append(f"crossbow {rid}: {got} != stance {[f[3] for f in fields]}")
    if 43020000 not in plain:
        probs.append("Light Crossbow 43020000 is not a plain (spAtkcategory 0) crossbow")
    return probs, len(plain)


def check_no_fire(fields, lay, pad_text, dll_texts):
    probs = []
    written = set()
    for _, o, w, _ in fields:
        written.update(range(o, o + w))
    for name, (off, width, _) in lay.items():
        if name not in NAMES and NO_TOUCH.match(name) and written & set(range(off, off + width)):
            probs.append(f"the stance writes a byte of {name} (+0x{off:x})")
    if "melee_off" not in pad_text or "out.melee_suppressed = true" not in pad_text:
        probs.append("pad_logic has no melee_off suppression")
    joined = "\n".join(dll_texts.values())
    if "fn stray_guard" not in dll_texts.get("er_stance.rs", "") or "read_owned_foreign" not in joined:
        probs.append("the DLL has no stray-bullet guard")
    for name, t in dll_texts.items():
        if "get_equip_param_mut" in t or re.search(r"get_mut::<EquipParamWeapon>", t):
            probs.append(f"{name} forms a &mut param row")
    return probs


def check_package(build_text):
    probs = []
    m = re.search(r"^EDITED = \(([^)]*)\)", build_text, re.M)
    edited = re.findall(r'"(\w+)"', m.group(1)) if m else None
    if edited != ["Bullet", "AtkParam_Pc", "SpEffectParam"]:
        probs.append(f"build_regulation.py EDITED = {edited} (EquipParamWeapon / BehaviorParam_PC must stay vanilla)")
    if "--stance" in build_text:
        probs.append("build_regulation.py has a --stance option (C6 ships none; option A is a later decision)")
    for nm in ("EquipParamWeapon.param", "BehaviorParam_PC.param"):
        a, b = STAGED_UNPACKED / "regulation-bin" / nm, VANILLA / nm
        if not a.exists() or not b.exists():
            probs.append(f"missing {a if not a.exists() else b}")
        elif sha(a) != sha(b):
            probs.append(f"{nm} of the staged package differs from vanilla")
    try:
        p = param_diff.read_param(str(STAGED_UNPACKED / "regulation-bin" / "EquipParamWeapon.param"))
        mod_rows = [rid for rid, _, _ in p["rows"] if 79_000_000 <= rid <= 79_999_999]
        if mod_rows:
            probs.append(f"EquipParamWeapon has mod rows {mod_rows[:5]}")
    except Exception as e:  # noqa: BLE001
        probs.append(f"staged EquipParamWeapon unreadable: {e}")
    if not PACKAGE.exists():
        probs.append(f"missing {PACKAGE}")
    else:
        s = sha(PACKAGE)
        if s != EXPECTED_PACKAGE_SHA:
            probs.append(f"package sha256 {s[:16]}.. != the C6-time package {EXPECTED_PACKAGE_SHA[:16]}.. (rebuilt since? re-run the C3 / C4 checks)")
        bc = STAGED_UNPACKED / "regulation.bin"
        if bc.exists() and sha(bc) != s:
            probs.append("build_check's regulation.bin is not the staged package (its unpack is stale)")
    return probs


HIDE_KEEP = re.compile(r"^(spCategory|vfxId\d?|effectEndurance|isDisableNetSync|iconId|effectTarget\w*)$")
SP_MODE, UNSEEN_BLADE = 79001090, 1466000


def hide_consts(text):
    fields = [(n, int(o, 16), int(w), int(f, 16), int(t, 16)) for n, o, w, f, t in
              re.findall(r'HideField \{ name: "(\w+)", off: (0x[0-9A-Fa-f]+), width: (\d+), from: (0x[0-9A-Fa-f]+), to: (0x[0-9A-Fa-f]+) \}', text)]
    m = re.search(r"pub const SP_ROW_SIZE: usize = (\d+);", text)
    return fields, int(m.group(1)) if m else None


def check_hide(hfields, sp_size):
    probs = []
    pv = param_diff.read_param(str(VANILLA / "SpEffectParam.param"))
    ps = param_diff.read_param(str(STAGED_UNPACKED / "regulation-bin" / "SpEffectParam.param"))
    _, lay, size = param_rows.load_def(param_rows.DEFAULT_DEFS, pv["type"], 11730034)
    by = {name: (off, typ, boff, bsize, n) for name, typ, off, boff, bsize, n in lay}
    if sp_size != size:
        probs.append(f"SP_ROW_SIZE {sp_size} != Paramdex {size}")
    if len(hfields) != 12:
        probs.append(f"HIDE_FIELDS has {len(hfields)} entries (want 12)")
    vrows = {rid: data for rid, _, data in pv["rows"]}
    srows = {rid: data for rid, _, data in ps["rows"]}
    if SP_MODE not in srows or UNSEEN_BLADE not in vrows:
        return probs + [f"row {SP_MODE} (package) or {UNSEEN_BLADE} (vanilla) missing"]
    written = set()
    for name, off, width, frm, to in hfields:
        if name not in by:
            probs.append(f"{name}: not a Paramdex field")
            continue
        o, typ, boff, bsize, _ = by[name]
        if boff is not None:
            k = int(re.sub(r"\D", "", name) or 0)
            ok = width == 1 and o == off and boff == 0 and all(by.get(f"vowType{k + j}", (None,))[0] == off for j in range(8))
            if not ok:
                probs.append(f"{name}: core +0x{off:x}/{width} is not a whole byte of 8 vow bits at Paramdex +0x{o:x}")
        elif (o, param_rows.SIZES[typ]) != (off, width):
            probs.append(f"{name}: core +0x{off:x}/{width} != Paramdex +0x{o:x}/{param_rows.SIZES[typ]}")
        written.update(range(off, off + width))
        pk = int.from_bytes(srows[SP_MODE][off:off + width], "little")
        ub = int.from_bytes(vrows[UNSEEN_BLADE][off:off + width], "little")
        if pk != frm:
            probs.append(f"{name}: package row {SP_MODE} holds 0x{pk:x}, core from=0x{frm:x}")
        if ub != to:
            probs.append(f"{name}: Unseen Blade {UNSEEN_BLADE} holds 0x{ub:x}, core to=0x{to:x}")
    for name, (o, typ, boff, bsize, n) in by.items():
        if HIDE_KEEP.match(name) and written & set(range(o, o + param_rows.SIZES[typ] * n)):
            probs.append(f"the hide patch writes a byte of {name} (+0x{o:x})")
    if not any(n == "stateInfo" and t == 184 for n, _, _, _, t in hfields):
        probs.append("stateInfo is not patched to 184 (Hide Weapon)")
    return probs


def run(fields=None, offs=None, row_size=None, pad_text=None, dll_texts=None, build_text=None, quiet=False, hfields=None):
    core = core_consts(CORE.read_text(encoding="utf-8"))
    fields = core[0] if fields is None else fields
    offs = core[1] if offs is None else offs
    row_size = core[2] if row_size is None else row_size
    pad_text = PAD.read_text(encoding="utf-8") if pad_text is None else pad_text
    if dll_texts is None:
        dll_texts = {p.name: p.read_text(encoding="utf-8") for p in DLL.rglob("*.rs")}
    build_text = BUILD.read_text(encoding="utf-8") if build_text is None else build_text
    p, lay, size = layout()
    res = []
    res.append(("fields", check_fields(fields, offs, row_size, lay, size)))
    vp, n_plain = check_values(fields, p, lay)
    res.append(("values", vp))
    res.append(("no fire", check_no_fire(fields, lay, pad_text, dll_texts)))
    res.append(("package", check_package(build_text)))
    hc = hide_consts(CORE_VIS.read_text(encoding="utf-8"))
    res.append(("hide", check_hide(hc[0] if hfields is None else hfields, hc[1])))
    if not quiet:
        detail = {"fields": f"{len(fields)} motion fields at {[hex(f[1]) for f in fields]} = Paramdex; kept fields untouched",
                  "values": f"12 crossbow rows, {n_plain} plain ones = {[f[3] for f in fields]}",
                  "no fire": "no attack / bullet / category byte written; R3 suppressed; stray guard; no &mut row",
                  "package": f"EquipParamWeapon + BehaviorParam_PC vanilla in the staged package, sha256 {EXPECTED_PACKAGE_SHA[:16]}..",
                  "hide": f"{SP_MODE} patch = Paramdex offsets; from = the package row, to = Unseen Blade {UNSEEN_BLADE} (stateInfo 184); spCategory / vfx / endurance untouched"}
        for n, probs in res:
            print(f"{'PASS' if not probs else 'FAIL':<6} {n:8} {detail[n] if not probs else '; '.join(probs[:3])}")
    return all(not probs for _, probs in res), res


def self_test():
    ok, _ = run(quiet=True)
    out = [f"real inputs: {'PASS' if ok else 'FAIL'}"]
    core = core_consts(CORE.read_text(encoding="utf-8"))
    hc = hide_consts(CORE_VIS.read_text(encoding="utf-8"))
    muts = [
        ("stance writes weaponCategory", {"fields": core[0][:-1] + [("wepmotionBothHandId", 0xE6, 1, 16)]}, "fields"),
        ("wrong both-hand value", {"fields": core[0][:-1] + [("wepmotionBothHandId", 0xF1, 1, 14)]}, "values"),
        ("a 2-byte write over 0x1A6 (wepType)", {"fields": core[0] + [("x", 0x1A5, 2, 0)]}, "no fire"),
        ("melee not suppressed", {"pad_text": "fn step() {}"}, "no fire"),
        ("a &mut param row", {"dll_texts": {"er_stance.rs": "fn stray_guard read_owned_foreign get_equip_param_mut"}}, "no fire"),
        ("EquipParamWeapon edited", {"build_text": 'EDITED = ("Bullet", "AtkParam_Pc", "SpEffectParam", "EquipParamWeapon")\n'}, "package"),
        ("hide stateInfo at a wrong offset", {"hfields": [h if h[0] != "stateInfo" else ("stateInfo", 0x154, 2, 0, 184) for h in hc[0]]}, "hide"),
        ("hide patch to spCategory 162", {"hfields": hc[0] + [("spCategory", 0x13E, 2, 0, 162)]}, "hide"),
        ("hide rate not Unseen Blade's", {"hfields": [h if h[0] != "soulRate" else ("soulRate", 0xE0, 4, 0, 0) for h in hc[0]]}, "hide"),
    ]
    caught = 0
    for label, kw, row in muts:
        _, res = run(quiet=True, **kw)
        failed = dict(res)[row]
        if failed:
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} PASS")
    out.append(f"{caught}/{len(muts)} mutations FAIL their check")
    return ok and caught == len(muts), out


def main(argv):
    if argv == ["--self-test"]:
        ok, lines = self_test()
        for line in lines:
            print("  " + line)
        print(f"{'PASS' if ok else 'FAIL'} check_c6_regulation self-test")
        return 0 if ok else 1
    if argv:
        print(__doc__)
        return 2
    ok, _ = run()
    print(f"C6 REGULATION: {'PASS' if ok else 'FAIL'} (runtime stance + hide patch only; no C6 row in the default package)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
