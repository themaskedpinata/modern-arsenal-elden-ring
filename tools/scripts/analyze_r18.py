"""Run-18 rows of analyze_log.py (MODLOG 2026-10-18 run 18): the first-person camera gate with ER's camera pulled close
by its collision (camera close), the controller's Havok filters (ray filters) and the unstick handback (unstick).
Imported by analyze_log.py; pure Python, read-only.

- camera close: FAIL when ER's camera closer than 0.3 m to the chest turned first person off (`FP off reason=cam_gate
  why=dist` next to a `CAMFRAME ... cam_chest<0.30m`, the run-18 bug) or, with a run-18 DLL (`close_ok=` on CAMFRAME),
  when a close frame was invalid while close_ok=1. Also judges older logs (the real run-18 excerpt FAILs).
- ray filters: the `hits=map:/obj:/both:` counts of the `MOVE er_delta` lines; FAIL when the object filter never hit
  in a union session with many map hits (cast_ray ignores that filter), INFO otherwise.
- unstick: every `MOVE unstick` is followed by `MOVE suspend reason=unstick` and a re-arm (`MOVE on`), the holds
  double (hold_s x 2^(n-1), capped), at most max_per_window within the backoff window.
"""
import re

R18_ROWS = ("camera close", "ray filters", "unstick")
CLOSE_M = 0.30
NEAR_S = 0.6
REARM_SLACK_S = 10.0
OBJ_FAIL_MAP_HITS = 200

CAM_RE = re.compile(r"valid=([01]) why=(\w+) .*cam_chest=([\d.]+)m(?: close_ok=([01]))?")
HITS_RE = re.compile(r"hits=map:(\d+)/obj:(\d+)/both:(\d+)")
UNSTICK_RE = re.compile(r"^unstick n=(\d+) hold=([\d.]+)s window=([\d.]+)s moved=([\d.]+)m")
CFG_UNSTICK_RE = re.compile(r"\bunstick=(?:0|([\d.]+)s/([\d.]+)m/(\d+)%/hold([\d.]+)-([\d.]+)s/max(\d+))(?:\s|$)")
CFG_FILTERS_RE = re.compile(r"\bfilters=(\w+)\(([^)]*)\)")


def cfg_line(entries):
    for e in entries:
        if e["tag"] == "MOVE" and e["msg"].startswith("cfg "):
            return e["msg"]
    return ""


def judge_camera_close(entries):
    cams = [(e, m) for e in entries if e["tag"] == "CAMFRAME" and (m := CAM_RE.search(e["msg"]))]
    offs = [e for e in entries if e["tag"] == "FP" and e["msg"].startswith("off reason=cam_gate why=dist")]
    if not cams:
        return "N/A", "no CAMFRAME lines with cam_chest"
    bad = []
    for o in offs:
        near = [m for e, m in cams if o["t"] - NEAR_S <= e["t"] <= o["t"] + 0.05 and m.group(1) == "0" and m.group(2) == "dist"]
        if near and float(near[-1].group(3)) < CLOSE_M:
            bad.append(f"FP off cam_gate why=dist at {o['t']:.2f} s with ER's camera {near[-1].group(3)} m from the chest")
    fixed = any(m.group(4) is not None for _, m in cams)
    for e, m in cams:
        if m.group(4) == "1" and m.group(1) == "0" and m.group(2) == "dist" and float(m.group(3)) < CLOSE_M:
            bad.append(f"CAMFRAME valid=0 why=dist cam_chest={m.group(3)}m with close_ok=1 at {e['t']:.2f} s")
    if bad:
        return "FAIL", f"{len(bad)} close-camera handback(s) (run-18 bug): {bad[0]}" + ("" if fixed else " (DLL before the run-18 fix)")
    if not fixed:
        return "N/A", "CAMFRAME without close_ok= (DLL before run 18) and no close-camera handback"
    close = [m for _, m in cams if m.group(4) == "1" and float(m.group(3)) < CLOSE_M]
    if not close:
        return "NEEDS-DATA", "no close ER camera seen while first person drove: stand against a wagon / wall in first person and jump"
    return "PASS", f"{len(close)} CAMFRAME line(s) with ER's camera < {CLOSE_M} m from the chest (min {min(float(m.group(3)) for m in close):.2f} m) while first person drove; no handback"


def judge_ray_filters(entries):
    cfg = cfg_line(entries)
    f = CFG_FILTERS_RE.search(cfg)
    if not f:
        return "N/A", "no 'MOVE cfg ... filters=' (DLL before run 18)"
    mode, tags = f.group(1), f.group(2)
    tot = [0, 0, 0]
    n = 0
    for e in entries:
        if e["tag"] == "MOVE" and e["msg"].startswith("er_delta ") and (m := HITS_RE.search(e["msg"])):
            n += 1
            for k in range(3):
                tot[k] += int(m.group(k + 1))
    if sum(tot) == 0:
        return "NEEDS-DATA", f"filters={mode}({tags}): no ray hits counted yet (walk around while MW2 mode drives)"
    detail = (f"filters={mode}({tags}): {n} period(s), hits map-only/nearer {tot[0]}, object-only/nearer {tot[1]} "
              f"(what the map filter misses), both {tot[2]} (the object filter also sees the map)")
    if mode == "union" and "+" in tags and tot[1] + tot[2] == 0 and tot[0] >= OBJ_FAIL_MAP_HITS:
        return "FAIL", f"the object filter never hit in {tot[0]} map hits: cast_ray ignores it? try another object_filter; {detail}"
    return "PASS", detail


def judge_unstick(entries):
    cfg = cfg_line(entries)
    c = CFG_UNSTICK_RE.search(cfg)
    if not c:
        return "N/A", "no 'MOVE cfg ... unstick=' (DLL before run 18)"
    if c.group(1) is None:
        return "PASS", "unstick disabled ([movement.unstick] enabled = false)"
    hold0, hold_max, max_n = float(c.group(4)), float(c.group(5)), int(c.group(6))
    us = [(e, m) for e in entries if e["tag"] == "MOVE" and (m := UNSTICK_RE.match(e["msg"]))]
    sus = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=unstick")]
    ons = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("on pos=")]
    capped = [e for e in entries if e["tag"] == "MOVE" and "unstick capped" in e["msg"]]
    if not us:
        if not ons:
            return "NEEDS-DATA", "the controller never drove"
        return "PASS", f"0 unsticks (never stuck){'; ' + str(len(capped)) + ' capped' if capped else ''}"
    bad = []
    for k, (e, m) in enumerate(us):
        n, hold = int(m.group(1)), float(m.group(2))
        nxt = us[k + 1][0]["t"] if k + 1 < len(us) else float("inf")
        want = min(hold0 * 2 ** (n - 1), hold_max)
        if abs(hold - want) > 0.05:
            bad.append(f"unstick n={n} hold {hold} s (want {want} s)")
        s = next((x for x in sus if e["t"] - 0.01 <= x["t"] <= e["t"] + 0.2), None)
        if s is None:
            bad.append(f"unstick at {e['t']:.2f} s without 'suspend reason=unstick'")
            continue
        # the re-arm after the hold, before the next unstick (an ER-driven hold that never ends is a stuck handback)
        if not any(s["t"] + hold - 0.05 <= o["t"] <= min(s["t"] + hold + REARM_SLACK_S, nxt) for o in ons):
            bad.append(f"no re-arm (MOVE on) within {hold + REARM_SLACK_S:.0f} s of the unstick at {s['t']:.2f} s")
        recent = [x for x, _ in us if e["t"] - 60.0 < x["t"] <= e["t"]]
        if len(recent) > max_n:
            bad.append(f"{len(recent)} unsticks within 60 s (max {max_n})")
    if bad:
        return "FAIL", f"{len(bad)} problem(s): {bad[0]}"
    holds = ",".join(m.group(2) for _, m in us)
    return "PASS", f"{len(us)} unstick(s) (holds {holds} s), each handed back and re-armed{'; ' + str(len(capped)) + ' capped rest(s)' if capped else ''}"


def rows(entries, ctx=None):
    return [
        ("camera close", *judge_camera_close(entries)),
        ("ray filters", *judge_ray_filters(entries)),
        ("unstick", *judge_unstick(entries)),
    ]


# ---- the synthetic run-18 session (self-test and fixture) ----------------------------------------------------------

CFG = ("cfg enabled=1 tp_anim=hybrid sprint=press crouch=toggle jump_slowdown=1 fall=mw2 lethal_fall=handback eye=1.600/1.067/0.293 "
       "hull=1.867/1.333/0.800 r=er rays_max=48 data=converted\\movement.json sha=2f8c9cfda1b8 mantle=1 key=mw2 max_h=2.00m "
       "rise_max=60in angle=60 over=thin vm=drop rays=24 ladder_er_attacks=0 filters=union(map:0x2000058+object_cam:0x5b) "
       "body_probes=1 depen=0.30m unstick=2.5s/0.50m/10%/hold1.0-8.0s/max4")


def cam(t, d, valid=1, why="ok", close=1):
    return (t, "CAMFRAME", f"tick={int(t * 60)} src=pers_cam_1 valid={valid} why={why} pos=(5.35,9.71,27.34) fwd=(0.985,-0.092,-0.145) "
                           f"det=+1.000 ortho_err=5.2e-9 fov=0.8378(48.0deg) aspect=1.7778 near=0.0501 far=10000.0 cam_chest={d:.2f}m close_ok={close}")


def r18_session(fp_off_close=False, close_invalid=False, no_obj_hits=False, no_suspend=False, hold_wrong=False, too_many=False, no_rearm=False):
    """A good run-18 session as (t, tag, msg) lines; each keyword breaks one rule."""
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))

    a(0.0, "BOOT", "mw2er 0.3.0 dll=D:\\Mods\\MW2inER\\build\\natives\\mw2er.dll")
    a(0.1, "MOVE", CFG)
    a(1.0, "TASK", "world entered (player handle 0x1 owner_check=ok)")
    a(1.5, "MOVE", "on pos=(5.00,8.20,26.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
    L.append(cam(2.0, 3.54))
    # against the wagon: ER's camera pulled in to 0.29 / 0.08 m, first person stays on
    L.append(cam(10.0, 0.29, valid=0 if close_invalid else 1, why="dist" if close_invalid else "ok"))
    L.append(cam(10.5, 0.08))
    if fp_off_close:
        L.append(cam(11.0, 0.27, valid=0, why="dist"))
        a(11.1, "FP", "off reason=cam_gate why=dist ticks_on=1089 cam_writes=0 (re-arms when clear)")
    obj = 0 if no_obj_hits else 412
    a(15.0, "MOVE", f"er_delta p50=0.001 p95=0.022 max=0.033 applied=1 handbacks=1 budget_hits=0 gravity_reasserts=900 "
                    f"ground_disagree_streaks=0 ground_held_ticks=0 unsticks=0 hits=map:{1500 if no_obj_hits else 900}/obj:{obj}/both:{0 if no_obj_hits else 600}")
    # stuck: two unsticks (1 s, then 2 s), each handed back and re-armed
    t = 30.0
    n_us = 5 if too_many else 2
    for n in range(1, n_us + 1):
        hold = min(1.0 * 2 ** (n - 1), 8.0)
        h_log = hold + 0.5 if (hold_wrong and n == 2) else hold
        a(t, "MOVE", f"unstick n={n} hold={h_log:.1f}s window=2.50s moved=0.03m wish=150/150 pushed=21 inside=0 pos=(6.00,8.20,27.00) "
                     f"er_pos=(6.00,8.20,27.00) stance=prone hits=map:40/obj:12/both:3")
        if not (no_suspend and n == 1):
            a(t + 0.017, "MOVE", f"restore field=gravity_multiplier addr=0x7ff223db74cc old=0.000 new=1.000 ok=1")
            a(t + 0.017, "MOVE", f"suspend reason=unstick no progress while ER pushed out / a probe was inside geometry (hold {hold:.1f} s, {n} of max 4 within 60 s; gravity back to ER) ticks=900 pos=(6.00,8.20,27.00)")
        if not (no_rearm and n == 1):
            a(t + hold + 0.2, "MOVE", "resync why=handback d=0.00m chunk_d=0.00m")
            a(t + hold + 0.2, "MOVE", "on pos=(5.60,8.20,26.40) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
        t += hold + 3.0
    return L


R18_MUTATIONS = [
    ("a close camera turned first person off (run 18)", dict(fp_off_close=True), "camera close"),
    ("a close camera invalid with close_ok=1", dict(close_invalid=True), "camera close"),
    ("the object filter never hit", dict(no_obj_hits=True), "ray filters"),
    ("an unstick without the handback", dict(no_suspend=True), "unstick"),
    ("the second hold not doubled", dict(hold_wrong=True), "unstick"),
    ("5 unsticks within 60 s (max 4)", dict(too_many=True), "unstick"),
    ("never re-armed after an unstick", dict(no_rearm=True), "unstick"),
]


def to_entries(lines):
    out = []
    for t, tag, msg in sorted(lines, key=lambda x: x[0]):
        ms = int(round(t * 1000))
        raw = f"2026-10-18 15:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [task] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def to_log(lines):
    return "\n".join(e["raw"] for e in to_entries(lines)) + "\n"


def self_test():
    """(ok, caught, lines): the good session PASSes the 3 rows; each mutation FAILs its row; an older log is N/A."""
    out, ok, caught = [], True, 0
    good = {n: (s, d) for n, s, d in rows(to_entries(r18_session()))}
    bad = [f"{n}: {good[n][0]} {good[n][1][:90]}" for n in R18_ROWS if good[n][0] != "PASS"]
    if bad:
        ok = False
    out.append(f"good run-18 session: {sum(1 for n in R18_ROWS if good[n][0] == 'PASS')}/{len(R18_ROWS)} rows PASS" + (f" ({bad})" if bad else ""))
    for label, kw, row in R18_MUTATIONS:
        r = {n: (s, d) for n, s, d in rows(to_entries(r18_session(**kw)))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:80]}")
    # an older DLL (no filters= / unstick= / close_ok=, no close-camera handback): N/A on every row
    old = [(0.0, "BOOT", "mw2er 0.3.0"), (0.1, "MOVE", CFG.split(" filters=")[0]), cam(2.0, 3.5)[0:2] + (cam(2.0, 3.5)[2].replace(" close_ok=1", ""),)]
    st = {n: s for n, s, _ in rows(to_entries(old))}
    if any(v != "N/A" for v in st.values()):
        ok = False
        out.append(f"MISSED: a pre-run-18 log must be N/A: {st}")
    out.append(f"{caught}/{len(R18_MUTATIONS)} run-18 mutations FAIL their row")
    return ok, caught, out
