"""C4 probe analyzer (docs/C4_UI_DESIGN.md 2.3 / 8.1 check 7): reads mw2er.log, judges the last session's `UI probe`
lines row by row and prints `C4 PROBE VERDICT: READY path=scene|layer` only if every row PASSes.
C4.1 (docs/C4_1_DESIGN.md 2.3): then judges the `UI probe41` lines (ROWS41) and prints
`C4.1 PROBE VERDICT: READY output=sdr|hdr alpha=transmit|premul`, `NOT READY`, or `C4.1 PROBE N/A`.

  python -I analyze_c4.py [logs\\mw2er.log]       the last BOOT session (exit 0 READY, 1 NOT READY, 2 probe N/A)
  python -I analyze_c4.py --c41 [logs\\mw2er.log] the same report; the exit code is the C4.1 verdict's (0 / 1 / 2)
  python -I analyze_c4.py --inject [logs\\mw2er.log] the same report; the exit code is the C4.1 injection run's
                                                  (`UI inject ... mode=ui_thread` lines, design 7.2: 0 CLEAN / 1 / 2 N/A)
  python -I analyze_c4.py --self-test             synthetic logs: READY, each row broken, N/A (C4, C4.1, C4.1 injection)

Independent of the DLL's own `UI probe verdict` / `UI probe41 verdict` lines (reported and compared, never trusted).
Stdlib only, read-only.
"""
import re
import sys

ACCEPT = {
    "scene": {"R10G10B10A2_UNORM", "R8G8B8A8_UNORM", "B8G8R8A8_UNORM", "R8G8B8A8_UNORM_SRGB", "B8G8R8A8_UNORM_SRGB"},
    "layer": {"R10G10B10A2_UNORM", "R8G8B8A8_UNORM", "B8G8R8A8_UNORM"},
}
ROWS = ["hooks", "hal", "thread", "getter", "target", "adjacency"]


def kv(line):
    return dict(re.findall(r"(\w+)=(\S+)", line))


def last_session(lines):
    start = 0
    for i, l in enumerate(lines):
        if re.search(r"\[boot\] BOOT mw2er ", l) or ("BOOT DONE" in l and start == 0):
            start = i
    return lines[start:]


def analyze(lines):
    """Returns (rows {name: (ok, note)}, info {path, coverage, dll}) or None when the probe did not run."""
    p = [l.split("UI probe ", 1)[1].strip() for l in lines if "UI probe " in l]
    if not p:
        return None
    rows = {}
    hooks = {m for l in p for m in re.findall(r"^hook ok fn=(\w+)", l)}
    skips = [l for l in p if l.startswith("skip")]
    ecl_ok = any(l.startswith("ecl hook=ok") for l in p)
    rows["hooks"] = ({"ui_begin", "ui_end"} <= hooks and ecl_ok and not skips,
                     f"hooks={sorted(hooks)} ecl={'ok' if ecl_ok else 'missing'} skips={len(skips)}")

    hal = [kv(l) for l in p if l.startswith("hal ")]
    rows["hal"] = (bool(hal) and all(h.get("vt_ok") == "1" and h.get("dev_eq") == "1" for h in hal),
                   f"lines={len(hal)} bad={sum(1 for h in hal if h.get('vt_ok') != '1' or h.get('dev_eq') != '1')}")

    frames = [kv(l) for l in p if l.startswith("frame ")]
    tid_bad = per_bad = begins = 0
    for f in frames:
        a, b = f.get("begin_tid_ok", "0/0").split("/")
        begins += int(b)
        tid_bad += int(a) != int(b)
        lo, hi = (int(x) for x in f.get("per_present", "0-0").split("-"))
        per_bad += not (0 <= lo and hi <= 1)
    same = bool(hal) and all(h.get("same_thread") == "1" for h in hal)
    per_ok = bool(frames) and per_bad * 100 <= len(frames)
    rows["thread"] = (same and begins > 0 and tid_bad == 0 and per_ok,
                      f"same_thread={int(same)} windows={len(frames)} tid_bad={tid_bad} per_present_bad={per_bad}")

    get = [kv(l) for l in p if l.startswith("getter ")]
    stable = len({(g.get("addr"), g.get("bytes")) for g in get}) == 1
    rows["getter"] = (bool(get) and stable and all(g.get("plain") == "1" for g in get), f"lines={len(get)} stable={int(stable)}")

    modes = [kv(l) for l in p if l.startswith("mode ")]
    path = modes[-1].get("sel", "?") if modes else "?"
    rts = [kv(l) for l in p if l.startswith("rt ") and "desc=none" not in l]
    bad = []
    for r in rts:
        name = r.get("fmt", ":").split(":", 1)[1]
        if not (r.get("rt_ok") == "1" and r.get("samples") == "1" and r.get("dim") == "3" and r.get("arr") == "1"):
            bad.append("desc")
        if name not in ACCEPT.get(r.get("which", path), set()):
            bad.append(f"fmt={name}")
    sizes = {(r.get("w"), r.get("h")) for r in rts}
    rows["target"] = (bool(rts) and not bad and len(sizes) <= 2,
                      f"lines={len(rts)} sizes={sorted(sizes)} bad={sorted(set(bad)) or 'none'}")

    ecl = [kv(l) for l in p if l.startswith("ecl total=")]
    if ecl:
        e = {k: int(v) for k, v in ecl[-1].items() if v.isdigit()}
        ok = e.get("armed", 0) > 0 and e.get("armed_hit") == e["armed"] and e.get("armed_first") == e["armed"] \
            and e.get("between_other", 1) == 0 and e.get("late", 1) == 0
        rows["adjacency"] = (ok, " ".join(f"{k}={e.get(k)}" for k in ["armed", "armed_hit", "armed_first", "between_other", "late"]))
    else:
        rows["adjacency"] = (False, "no ecl lines")

    cov = "?"
    if frames:
        nm = sum(int(f.get("nomovies", 0)) for f in frames)
        ub = sum(int(f.get("ui_begin", 0)) for f in frames)
        cov = f"nomovies {nm}/{ub + nm}"
    dll = [l for l in p if l.startswith("verdict ")]
    return rows, {"path": path, "coverage": cov, "dll": dll[-1] if dll else "none"}


def report(res):
    if res is None:
        print("C4 PROBE N/A: no 'UI probe' lines in the last session ([ui] probe_scaleform = false)")
        return 2
    rows, info = res
    for r in ROWS:
        ok, note = rows[r]
        print(f"{'PASS' if ok else 'FAIL'} {r:<10} {note}")
    print(f"INFO mode       path={info['path']} (scene = SDR; layer = HDR, needs [ui] inject_ui_layer)")
    print(f"INFO coverage   {info['coverage']} (frames that fall back to Present)")
    print(f"INFO dll        {info['dll']}")
    ready = all(rows[r][0] for r in ROWS)
    dll_ready = "ready=1" in info["dll"]
    if info["dll"] != "none" and dll_ready != ready:
        print("WARN the DLL's own verdict disagrees with this analyzer: treat as NOT READY")
        ready = False
    print(f"C4 PROBE VERDICT: {'READY path=' + info['path'] if ready else 'NOT READY'}")
    return 0 if ready else 1


GOOD = """2026-10-20 10:00:00.000 I [boot] BOOT mw2er 0.2.0 dll=x
I [boot] UI probe hook ok fn=ui_begin addr=0x7ff6 prologue=ok
I [boot] UI probe hook ok fn=ui_end addr=0x7ff7 prologue=ok
I [boot] UI probe ecl hook=ok
I [present] UI probe hal vt=0x1 vt_ok=1 dev_eq=1 queue_eq_present=1 rtid=5 present_tid=5 same_thread=1
I [present] UI probe mode b13c=0 b13e=0 layer=0 sel=scene
I [present] UI probe getter addr=0x1 bytes=488b81 plain=1 off=0x20
I [present] UI probe rt which=scene res=0x1 fmt=24:R10G10B10A2_UNORM w=3840 h=2160 dim=3 mips=1 arr=1 samples=1 flags=0x1 rt_ok=1 bb=3840x2160:R10G10B10A2_UNORM distinct=1
I [present] UI probe frame present=300 ui_begin=300 ui_end=300 nomovies=0 per_present=1-1 begin_tid_ok=300/300
I [present] UI probe ecl total=900 halq=600 otherq=300 pre=1 ui=1 post=1 armed=300 armed_hit=300 armed_first=300 armed_idx_max=0 armed_n_max=1 armed_tid_ok=300 between_other=0 late=0
I [present] UI probe verdict ready=1 why=none"""

BREAKS = {
    "hooks": ("UI probe ecl hook=ok", "UI probe ecl hook=missing"),
    "hal": ("vt_ok=1 dev_eq=1", "vt_ok=1 dev_eq=0"),
    "thread": ("same_thread=1", "same_thread=0"),
    "getter": ("plain=1", "plain=0"),
    "target": ("fmt=24:R10G10B10A2_UNORM", "fmt=10:R16G16B16A16_FLOAT"),
    "adjacency": ("between_other=0", "between_other=2"),
}


ROWS41 = ["hooks41", "thread41", "order", "adjacency41", "pins", "layer", "output", "alpha"]
PIN_FLAGS = ["vt", "vt7", "sub", "slot0", "slot1", "tex", "arr"]


def ratio(s):
    a, _, b = (s or "0/0").partition("/")
    return (int(a), int(b)) if a.isdigit() and b.isdigit() else (0, 0)


def conv_vote(empty, a255, a0):
    """mw2er_core::c4::conv_vote: transmit / premul when >= 90 % of at least 48 empty points agree, else unknown."""
    if empty < 48:
        return "unknown"
    if a255 * 10 >= empty * 9:
        return "transmit"
    if a0 * 10 >= empty * 9:
        return "premul"
    return "unknown"


def analyze41(lines):
    """C4.1 rows (design 2.3): (rows {name: (ok, note)}, info) or None when there is no `UI probe41` line."""
    p = [l.split("UI probe41 ", 1)[1].strip() for l in lines if "UI probe41 " in l]
    if not p:
        return None
    c4 = analyze(lines)
    c4p = [l.split("UI probe ", 1)[1].strip() for l in lines if "UI probe " in l]
    rows = {}
    on = [l for l in p if l.startswith("on ")]
    skips = [l for l in p if l.startswith("skip")]
    c4_hooks = c4 is not None and c4[0]["hooks"][0]
    rows["hooks41"] = (c4_hooks and bool(on) and not skips, f"c4_hooks={int(c4_hooks)} on={len(on)} skips41={len(skips)}")

    th = [kv(l) for l in p if l.startswith("thread ")]
    if th:
        last = th[-1]
        a, b = ratio(last.get("rt_ok"))
        mt_all = all(t.get("mt") == "1" for t in th)
        ok = mt_all and b > 0 and a == b and last.get("rtid_changes") == "0" and last.get("present_tid", "0") != "0"
        rows["thread41"] = (ok, f"lines={len(th)} mt_all={int(mt_all)} rt_ok={a}/{b} rtid_changes={last.get('rtid_changes')} "
                                f"present_tid={last.get('present_tid')}")
    else:
        rows["thread41"] = (False, "no thread lines")

    ecl = [kv(l) for l in c4p if l.startswith("ecl total=")]
    e = {k: int(v) for k, v in ecl[-1].items() if v.isdigit()} if ecl else {}
    if th and ecl:
        last = th[-1]
        ok = last.get("ahead") == "0" and last.get("present_in_ui") == "0" and e.get("late", 1) == 0 and e.get("armed", 0) > 0 \
            and e.get("armed_hit") == e["armed"] and e.get("armed_idx_max", 1) == 0 and e.get("armed_n_max") == 1
        rows["order"] = (ok, f"ahead={last.get('ahead')} present_in_ui={last.get('present_in_ui')} " +
                         " ".join(f"{k}={e.get(k)}" for k in ["late", "armed", "armed_hit", "armed_idx_max", "armed_n_max"]))
    else:
        rows["order"] = (False, f"thread lines={len(th)} ecl lines={len(ecl)}")

    betw = [kv(l) for l in p if l.startswith("between ")]
    bo = e.get("between_other")
    frames = [int(b.get("frame", "999999")) for b in betw if b.get("frame", "").isdigit()]
    ok = bo is not None and (bo == 0 or (bo == 1 and len(frames) == len(betw) and all(f <= 2 for f in frames)))
    rows["adjacency41"] = (ok, f"between_other={bo} between_frames={frames or 'none'}")

    layer_raw = [l for l in p if l.startswith("layer ")]
    layer = [kv(l) for l in layer_raw]
    layer_fmts = {x.get("fmt", ":").split(":", 1)[0] for x, raw in zip(layer, layer_raw) if "desc=none" not in raw}
    pins = [kv(l) for l in p if l.startswith("pins which=layer")]
    bad = []
    for x in pins:
        if not all(x.get(f) == "1" for f in PIN_FLAGS):
            bad.append("flags")
        if not (x.get("count", "0").isdigit() and int(x["count"]) >= 1):
            bad.append("count")
        if x.get("res_mod") != "1":
            bad.append("res_mod")
        if x.get("rtdata_rtv_eq") != "1":
            bad.append("rtdata_rtv")
        if x.get("rtdata_fmt") not in layer_fmts:
            bad.append("rtdata_fmt")
    res = {x.get("res") for x in pins if x.get("res") not in (None, "0x0")}
    rows["pins"] = (bool(pins) and not bad and 1 <= len(res) <= 2,
                    f"lines={len(pins)} res_distinct={len(res)} bad={sorted(set(bad)) or 'none'}")

    bad = []
    for x, raw in zip(layer, layer_raw):
        if "desc=none" in raw:
            bad.append("desc=none")
            continue
        a, b = ratio(x.get("rtdata_ok"))
        if x.get("fmt") != "28:R8G8B8A8_UNORM":
            bad.append(f"fmt={x.get('fmt')}")
        for k in ["rt_ok", "mips", "samples", "size_eq"]:
            if x.get(k) != "1":
                bad.append(k)
        if not (b > 0 and a == b):
            bad.append("rtdata_ok")
    rows["layer"] = (bool(layer) and not bad, f"lines={len(layer)} bad={sorted(set(bad)) or 'none'}")

    out = [kv(l) for l in p if l.startswith("output ")]
    output = "?"
    if out:
        o = out[-1]
        output = "hdr" if o.get("b13c") == "1" else "sdr"
        ok = o.get("shaders") == "1" and o.get("scale") == "1x1"
        note = f"b13c={o.get('b13c')} scale={o.get('scale')} shaders={o.get('shaders')} blend={o.get('blend')} output={output}"
        rows["output"] = (ok, note + (" (needs [ui] inject_ui_layer)" if output == "hdr" else ""))
    else:
        rows["output"] = (False, "no output lines")

    samples = [kv(l) for l in p if l.startswith("sample n=")]
    off = [l for l in p if l.startswith("sample off")]
    alpha = samples[-1].get("conv", "unknown") if samples else "unknown"
    if samples:
        # the analyzer's own vote from the last line's running counts (design 2.2: >= 90 % of >= 48 empty points)
        last = samples[-1]
        a255, a0, other = (int(last.get(k, "0")) if last.get(k, "0").isdigit() else 0 for k in ("a255", "a0", "other"))
        own = conv_vote(a255 + a0 + other, a255, a0)
        failed = [l for l in off if "why=hresult" in l]
        ok = alpha in ("transmit", "premul") and own == alpha and not failed
        note = f"samples={len(samples)} conv={alpha} own_vote={own} empty={a255 + a0 + other} a255={a255} a0={a0} other={other}"
        if own != alpha:
            note += " (the DLL's conv disagrees with the counts)"
        if failed:
            note += f"; P2 ended on a D3D12 failure: {failed[0]}"
            alpha = "unknown"
        rows["alpha"] = (ok, note)
    else:
        rows["alpha"] = (False, "P2 off (no sample lines" + (f"; {off[-1]}" if off else "") + ")")

    rec = [kv(l) for l in p if l.startswith("recreate ")]
    follow = n_res = 0
    for i, l in enumerate(p):
        r = kv(l)
        if l.startswith("recreate ") and r.get("what") == "res" and r.get("which") == "layer":
            n_res += 1
            pins_seen = {kv(x).get("res") for x in p[i + 1:] if x.startswith("pins which=layer")}
            layer_seen = {kv(x).get("res") for x in p[i + 1:] if x.startswith("layer ")}
            follow += r.get("new") in pins_seen and r.get("new") in layer_seen
    dll = [l for l in p if l.startswith("verdict ")]
    info = {
        "output": output,
        "alpha": alpha,
        "recreate": f"lines={len(rec)} layer_res_changes={n_res} followed_by_pins_or_layer={follow}/{n_res}",
        "coverage": c4[1]["coverage"] if c4 else "?",
        "dll": dll[-1] if dll else "none",
    }
    return rows, info


def report41(res):
    if res is None:
        print("C4.1 PROBE N/A: no 'UI probe41' lines in the last session ([ui] probe_scaleform = false, or a C4.1 DLL not run)")
        return 2
    rows, info = res
    for r in ROWS41:
        ok, note = rows[r]
        print(f"{'PASS' if ok else 'FAIL'} {r:<11} {note}")
    print(f"INFO recreate    {info['recreate']}")
    print(f"INFO coverage    {info['coverage']} (frames that fall back to Present)")
    print(f"INFO dll41       {info['dll']}")
    ready = all(rows[r][0] for r in ROWS41)
    dll_ready = "ready=1" in info["dll"]
    if info["dll"] != "none" and dll_ready != ready:
        print("WARN the DLL's own C4.1 verdict disagrees with this analyzer: treat as NOT READY")
        ready = False
    print(f"C4.1 PROBE VERDICT: {'READY output=' + info['output'] + ' alpha=' + info['alpha'] if ready else 'NOT READY'}")
    return 0 if ready else 1


GOOD41 = GOOD.replace("between_other=0 late=0", "between_other=1 late=0") + """
I [boot] UI probe41 sites sites=5 code=3 vmt=2 inferred=0 warn=0 (Ww2720)
I [boot] UI probe41 on sample=1
I [present] UI probe41 pins which=layer vt=1 vt7=1 sub=1 slot0=1 slot1=1 tex=1 arr=1 count=1 res=0x2a0 res_mod=1 rtv=0x77 rtdata_fmt=28 rtdata_rtv_eq=1
I [present] UI probe41 pins which=scene vt=1 vt7=1 sub=1 slot0=1 slot1=1 tex=1 arr=1 count=1 res=0x2b0 res_mod=1 rtv=0x78 rtdata_fmt=28 rtdata_rtv_eq=0
I [present] UI probe41 output b13c=0 b13e=1 eb9=0 scale=1x1 shaders=1 blend=sdr2sdr
I [present] UI probe41 between n=1 frame=1 list=0x7ff200001000
I [present] UI probe41 thread mt=1 rt_tid=5 hal_tid=5 begin_tid=5 present_tid=6 rt_ok=300/300 rtid_changes=0 ahead=0 present_in_ui=0
I [present] UI probe41 layer res=0x2a0 fmt=28:R8G8B8A8_UNORM w=3840 h=2160 mips=1 samples=1 flags=0x1 rt_ok=1 bb=3840x2160:R10G10B10A2_UNORM size_eq=1 res_changes=0 rtdata_ok=300/300
I [present] UI probe41 scene res=0x2b0 fmt=24:R10G10B10A2_UNORM w=3840 h=2160 mips=1 samples=1 flags=0x1 rt_ok=1 bb=3840x2160:R10G10B10A2_UNORM size_eq=1 res_changes=0 rtdata_ok=n/a
I [present] UI probe41 sample n=4 empty=16/16 a255=64 a0=0 other=0 conv=transmit px=0,0,0,255;0,0,0,255;0,0,0,255;0,0,0,255
I [present] UI probe41 verdict ready=1 why=none output=sdr alpha=transmit"""

BREAKS41 = {
    "hooks41": ("UI probe41 on sample=1", "UI probe41 skip why=sites scaleform_sub_slot1: bytes"),
    "thread41": ("mt=1", "mt=0"),
    "order": ("ahead=0", "ahead=3"),
    "adjacency41": ("frame=1 list=", "frame=900 list="),
    "pins": ("slot1=1 tex=1 arr=1 count=1 res=0x2a0", "slot1=0 tex=1 arr=1 count=1 res=0x2a0"),
    "layer": ("fmt=28:R8G8B8A8_UNORM", "fmt=10:R16G16B16A16_FLOAT"),
    "output": ("scale=1x1", "scale=0.75x0.75"),
    "alpha": ("conv=transmit", "conv=unknown"),
}

# more alpha mutations (design 2.4): counts that do not support the logged conv; P2 ended on a D3D12 failure; P2 off
EXTRA41 = [
    ("alpha counts", "alpha", "a255=64 a0=0 other=0 conv=transmit", "a255=40 a0=0 other=0 conv=transmit"),
    ("alpha hresult", "alpha", "I [present] UI probe41 verdict", "E [present] UI probe41 sample off why=hresult (samples=4 err=x)\nI [present] UI probe41 verdict"),
    ("alpha p2 off", "alpha", "I [present] UI probe41 sample n=4 empty=16/16 a255=64 a0=0 other=0 conv=transmit px=0,0,0,255;0,0,0,255;0,0,0,255;0,0,0,255\n", ""),
]


def self_test41():
    """The C4.1 cases: (passed, total)."""
    import io, contextlib
    fails, total = 0, 0
    total += 1
    res = analyze41(GOOD41.splitlines())
    with contextlib.redirect_stdout(io.StringIO()):
        rc = report41(res)
    if res is None or not all(res[0][r][0] for r in ROWS41) or rc != 0 or res[1]["alpha"] != "transmit":
        print("SELFTEST FAIL good C4.1 log not READY"); fails += 1
    for row, (a, b) in BREAKS41.items():
        total += 1
        if a not in GOOD41:
            print(f"SELFTEST FAIL mutation {row}: {a!r} not in the good log"); fails += 1
            continue
        res = analyze41(GOOD41.replace(a, b).splitlines())
        if res is None or res[0][row][0]:
            print(f"SELFTEST FAIL mutation {row} not caught"); fails += 1
    for name, row, a, b in EXTRA41:
        total += 1
        if a not in GOOD41:
            print(f"SELFTEST FAIL mutation {name}: {a!r} not in the good log"); fails += 1
            continue
        res = analyze41(GOOD41.replace(a, b).splitlines())
        if res is None or res[0][row][0]:
            print(f"SELFTEST FAIL mutation {name} not caught"); fails += 1
    # a premul run is READY too (the convention is data, not a pass condition)
    total += 1
    prem = GOOD41.replace("a255=64 a0=0 other=0 conv=transmit px=0,0,0,255;0,0,0,255;0,0,0,255;0,0,0,255",
                          "a255=0 a0=64 other=0 conv=premul px=0,0,0,0;0,0,0,0;0,0,0,0;0,0,0,0").replace("alpha=transmit", "alpha=premul")
    res = analyze41(prem.splitlines())
    if res is None or not all(res[0][r][0] for r in ROWS41) or res[1]["alpha"] != "premul":
        print("SELFTEST FAIL premul C4.1 log not READY"); fails += 1
    total += 1
    if analyze41(GOOD.splitlines()) is not None:
        print("SELFTEST FAIL C4.1 N/A not detected"); fails += 1
    total += 1
    with contextlib.redirect_stdout(io.StringIO()):
        rc = report41(analyze41(GOOD41.replace("verdict ready=1 why=none output", "verdict ready=0 why=alpha output").splitlines()))
    if rc != 1:
        print("SELFTEST FAIL C4.1 DLL disagreement not NOT READY"); fails += 1
    return total - fails, total


ROWS_INJ = ["live", "canary", "on", "early", "cancelled", "trip"]
# per-UiBegin misses that are no UI frame of the gun's (the menu, the AC-130, third person, no movie, start-up): they are
# excused from the early share (docs/C4_1_DESIGN.md 7.2 step 8: early >= 95 % of the frames with UI)
EXCUSED = {"config", "nohal", "nomovies", "hidden", "ac130", "notfp", "scoped_no_overlay", "notready", "alpha_unknown", "canary", "warmup", "st_mode"}


def analyze_inject41(lines):
    """C4.1 PASS I41 injection run (design 7.2): (rows, info) or None when the UI-thread injection did not run."""
    p = [l.split("UI inject ", 1)[1].strip() for l in lines if "UI inject " in l]
    if not any("mode=ui_thread" in l for l in p):
        return None
    rows = {}
    live = [l for l in p if l.startswith("live mode=ui_thread")]
    off = [l for l in p if l.startswith("off why=")]
    rows["live"] = (bool(live) and not off, f"live={len(live)} off={off[-1] if off else 'none'}")
    can = [l for l in p if l.startswith("canary ")]
    ok_can = [l for l in can if l.startswith("canary ok")]
    # review C4.1: the persistence check one UI frame later (the canary texel empty again: ER clears its layer)
    gone = [l for l in can if l.startswith("canary gone")]
    cleared = [l for l in can if l.startswith("canary cleared") or l.startswith("canary persisted")
               or (l.startswith("canary unknown") and "(try " not in l)]
    rows["canary"] = (bool(ok_can) and bool(gone) and not cleared,
                      f"lines={len(can)} ok={len(ok_can)} gone={len(gone)} cleared_persisted_or_unread={len(cleared)}")
    on = [kv(l) for l in p if l.startswith("on ") and "mode=ui_thread" in l]
    rows["on"] = (bool(on), "none" if not on else f"alpha={on[-1].get('alpha')} output={on[-1].get('output')} fmt={on[-1].get('fmt')} size={on[-1].get('size')}")
    stat = [l for l in p if l.startswith("early=") and "fallback_cut=" in l]
    info = {"fallback_cut": "?", "miss": "?", "share": "?", "recheck": "none", "trips": "none"}
    if stat:
        s = kv(stat[-1])
        n = {k: int(s.get(k, "0")) for k in ("early", "present", "splice", "cancelled", "fallback_cut") if s.get(k, "0").isdigit()}
        misses = dict((m.split(":")[0], int(m.split(":")[1])) for m in s.get("miss", "none").split(",") if ":" in m)
        excused = sum(v for k, v in misses.items() if k in EXCUSED)
        ui = max(n.get("early", 0) + n.get("present", 0) - excused, 0)
        share = n.get("early", 0) / ui if ui else 0.0
        rows["early"] = (n.get("early", 0) > 0 and share >= 0.95, f"early={n.get('early')} present={n.get('present')} excused={excused} "
                                                                    f"share={share:.3f} of the frames with UI (>= 0.95)")
        rows["cancelled"] = (n.get("cancelled", 1) == 0, f"cancelled={n.get('cancelled')} splice={n.get('splice')}")
        info.update(fallback_cut=str(n.get("fallback_cut")), miss=s.get("miss", "none"), share=f"{share:.3f}")
    else:
        rows["early"] = (False, "no `UI inject early=.. fallback_cut=..` line")
        rows["cancelled"] = (False, "no counter line")
    trips = [l for l in p if l.startswith("trip ")]
    removed = [l for l in lines if "device_removed" in l or "DeviceRemoved" in l]
    rows["trip"] = (not trips and not removed, f"trips={trips[0] if trips else 'none'} device_removed_lines={len(removed)}")
    rc = [kv(l) for l in p if l.startswith("recheck ")]
    if rc:
        info["recheck"] = f"lines={len(rc)} last vote={rc[-1].get('vote')} alpha={rc[-1].get('alpha')} disagree={rc[-1].get('disagree')}"
    return rows, info


def report_inject41(res):
    if res is None:
        print("C4.1 INJECT N/A: no `UI inject ... mode=ui_thread` lines in the last session ([ui] inject_ui_thread = false)")
        return 2
    rows, info = res
    for r in ROWS_INJ:
        ok, note = rows[r]
        print(f"{'PASS' if ok else 'FAIL'} {r:<11} {note}")
    print(f"INFO fallback_cut {info['fallback_cut']} (Present-drawn frames with a C5 cut-out region up; small is good)")
    print(f"INFO miss        {info['miss']}")
    print(f"INFO recheck     {info['recheck']}")
    # review C4.1 / design 3.8 "Still [I]": P2 pins only the empty pixels' alpha, not ER's composite math. A straight-alpha
    # composite would show translucent gun pixels as scene*(1-a)+gun*a^2 (darker, fringed); only the eye can tell
    print("INFO manual      design 7.2 step 4b (not in the log): with a hot `inject_viewmodel` toggle, compare a red-dot / "
          "holo reticle, lens glass and the scope overlay's soft edge (not only overall brightness); record same / darker in MODLOG")
    clean = all(rows[r][0] for r in ROWS_INJ)
    print(f"C4.1 INJECT VERDICT: {'CLEAN early_share=' + info['share'] + ' fallback_cut=' + info['fallback_cut'] if clean else 'NOT CLEAN'}")
    return 0 if clean else 1


GOOD_INJ = """2026-10-22 10:00:00.000 I [boot] BOOT mw2er 0.2.0 dll=x
I [boot] UI inject hooks ready mode=ui_thread (the early pass is created once the renderer is; probe verdict gate=0; the gun draws at Present until then)
I [present] UI inject early pass ready device=0x1 fmt=R10G10B10A2_UNORM depth=3840x2160 slots=3 psos=0 mode=ui_thread (PSOs per layer convention on request; readback sampler ready)
I [present] UI inject live mode=ui_thread alpha=transmit (from the per-build pin; the session canary is next, the gun draws at Present until it passed)
I [present] UI inject PSOs ready fmt=28:R8G8B8A8_UNORM mode=transmit
I [present] UI inject canary ok value=1,2,3,0 (the persistence check is next: one UI frame later the texel must be empty again)
I [present] UI inject canary gone value=0,0,0,255 (ER clears its layer between UI frames; the gun is drawn early from now on)
I [present] UI inject on build=2.7.2.0 path=layer mode=ui_thread output=sdr alpha=transmit fmt=R8G8B8A8_UNORM size=3840x2160 slots=3
I [present] UI inject recheck vote=transmit alpha=transmit empty=16/16 disagree=0
I [present] UI inject early=2900 present=160 splice=2902 cancelled=0 fallback_cut=3 miss=hidden:120,warmup:1,canary:1,pso_pending:2,locked:4"""

BREAKS_INJ = {
    "live": ("I [present] UI inject live mode=ui_thread", "W [present] UI inject off why=alpha_unpinned"),
    "canary": ("UI inject canary ok value=1,2,3,0", "UI inject canary cleared value=0,0,0,255"),
    "on": ("I [present] UI inject on build=", "I [present] UI inject no build="),
    "early": ("early=2900 present=160", "early=290 present=1600"),
    "cancelled": ("cancelled=0 fallback_cut", "cancelled=3 fallback_cut"),
    "trip": ("fallback_cut=3 miss=", "fallback_cut=3 miss=\nE [present] UI inject trip why=order frame=77 miss="),
}


def self_test_inject():
    """The C4.1 injection cases: (passed, total)."""
    import io, contextlib
    fails, total = 0, 1
    res = analyze_inject41(GOOD_INJ.splitlines())
    with contextlib.redirect_stdout(io.StringIO()):
        rc = report_inject41(res)
    if res is None or not all(res[0][r][0] for r in ROWS_INJ) or rc != 0:
        print("SELFTEST FAIL good C4.1 injection log not CLEAN"); fails += 1
    for row, (a, b) in BREAKS_INJ.items():
        total += 1
        if a not in GOOD_INJ:
            print(f"SELFTEST FAIL inject mutation {row}: {a!r} not in the good log"); fails += 1
            continue
        res = analyze_inject41(GOOD_INJ.replace(a, b).splitlines())
        if res is None or res[0][row][0]:
            print(f"SELFTEST FAIL inject mutation {row} not caught"); fails += 1
    total += 1
    if analyze_inject41(GOOD.splitlines()) is not None or analyze_inject41(GOOD41.splitlines()) is not None:
        print("SELFTEST FAIL C4.1 injection N/A not detected"); fails += 1
    # a canary retry (unknown, then ok) is still clean
    total += 1
    retry = GOOD_INJ.replace("I [present] UI inject canary ok", "W [present] UI inject canary unknown value=9,9,9,255 (try 1 of 3; another canary frame)\nI [present] UI inject canary ok")
    res = analyze_inject41(retry.splitlines())
    if res is None or not res[0]["canary"][0]:
        print("SELFTEST FAIL a canary retry counted as cleared"); fails += 1
    # review C4.1: the persistence check. A persisted canary, or no `canary gone` at all, is not clean; a check retry is
    total += 3
    persisted = GOOD_INJ.replace("I [present] UI inject canary gone value=0,0,0,255", "E [present] UI inject canary persisted value=1,2,3,0")
    res = analyze_inject41(persisted.splitlines())
    if res is None or res[0]["canary"][0]:
        print("SELFTEST FAIL a persisted canary not caught"); fails += 1
    no_gone = "\n".join(l for l in GOOD_INJ.splitlines() if "canary gone" not in l)
    res = analyze_inject41(no_gone.splitlines())
    if res is None or res[0]["canary"][0]:
        print("SELFTEST FAIL a canary without the persistence check counted as clean"); fails += 1
    retry = GOOD_INJ.replace("I [present] UI inject canary gone", "W [present] UI inject canary unknown value=9,9,9,255 (try 1 of 3; another persistence-check frame)\nI [present] UI inject canary gone")
    res = analyze_inject41(retry.splitlines())
    if res is None or not res[0]["canary"][0]:
        print("SELFTEST FAIL a persistence-check retry counted as a failure"); fails += 1
    return total - fails, total


def self_test():
    fails = 0
    res = analyze(GOOD.splitlines())
    if res is None or not all(res[0][r][0] for r in ROWS):
        print("SELFTEST FAIL good log not READY"); fails += 1
    for row, (a, b) in BREAKS.items():
        res = analyze(GOOD.replace(a, b).splitlines())
        if res is None or res[0][row][0]:
            print(f"SELFTEST FAIL mutation {row} not caught"); fails += 1
    if analyze(["2026 I [boot] BOOT DONE", "I [present] MOVE state"]) is not None:
        print("SELFTEST FAIL N/A not detected"); fails += 1
    disagree = GOOD.replace("verdict ready=1 why=none", "verdict ready=0 why=hal")
    import io, contextlib
    with contextlib.redirect_stdout(io.StringIO()):
        rc = report(analyze(disagree.splitlines()))
    if rc != 1:
        print("SELFTEST FAIL DLL disagreement not NOT READY"); fails += 1
    ok41, n41 = self_test41()
    fails += n41 - ok41
    oki, ni = self_test_inject()
    fails += ni - oki
    n = len(BREAKS) + 3 + n41 + ni
    print(f"analyze_c4 self-test: {n - fails}/{n} {'PASS' if not fails else 'FAIL'}")
    return 1 if fails else 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    path = next((a for a in sys.argv[1:] if not a.startswith("-")), r"D:\Mods\MW2inER\logs\mw2er.log")
    with open(path, encoding="utf-8", errors="replace") as f:
        session = last_session(f.read().splitlines())
    rc = report(analyze(session))
    rc41 = report41(analyze41(session))
    rci = report_inject41(analyze_inject41(session))
    sys.exit(rci if "--inject" in sys.argv else rc41 if "--c41" in sys.argv else rc)
