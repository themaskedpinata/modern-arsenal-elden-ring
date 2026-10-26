"""M8 rows of analyze_log.py (docs/M8_DESIGN.md section 11): MW2 mantling (mantle, mantle refusals, mantle perf), the
chunk re-base fix P1 (rebase) and the ladder handback audit (ladder handback, ladder dismount, ladder weapon, ladder
input, ladder signal). Imported by analyze_log.py; pure Python, read-only.

Rows that need an M8 DLL (its `MOVE cfg ... mantle=` field) are N/A for older logs, except `rebase`, which also judges
an M5-M7 log that has > 5 m resyncs (the run-17 excerpt FAILs it: every chunk re-base was logged as a warp).
INFO rows (mantle refusals, ladder signal) PASS with their counts; a suspicious pattern is a `WARN` in the detail.
"""
import re

TICK = 1.0 / 60.0
DIST_END_MAX_M = 0.05
SETTLE_TICKS = 10
DAMAGE_AFTER_S = 2.0
WARP_AFTER_S = 1.0
ABORT_SETTLE_S = 3.0
TPS_MIN = 55.0
REFUSALS_PER_MIN_WARN = 20
# M8 review: this many refusals with no start at all is "detection refuses everything", not "not tried"
REFUSE_ALL_MIN = 10
REBASE_MIN_M = 30.0
REBASE_MAX_M = 65.0
LOAD_GRACE_S = 1.0
LADDER_FP_OFF_S = 0.1
DISMOUNT_INFO_S = 2.0
DISMOUNT_FAIL_S = 10.0
DISMOUNT_CLEAN_S = 3.0
SIGNAL_DY_M = 1.0

M8_ROWS = ("mantle", "mantle refusals", "mantle perf", "rebase", "ladder handback", "ladder dismount", "ladder weapon",
           "ladder input", "ladder signal")

V3_RE = r"\((-?[\d.]+),(-?[\d.]+),(-?[\d.]+)\)"
START_RE = re.compile(r"^mantle start kind=(\w+) rise=([-\d.]+)m row=([\d.]+) .* dur=([\d.]+)s rays=(\d+) trigger=(\w+) air=([01])")
END_RE = re.compile(r"^mantle end dt=([\d.]+)s pos=" + V3_RE + r" dist_end=([\d.]+)m kind=(\w+) ground=([01]) er_ground=([01])")
ABORT_RE = re.compile(r"^mantle abort why=(\S+) t_frac=([\d.]+)")
SETTLE_RE = re.compile(r"^mantle settle er_ground=([01]) ticks=(\d+)")
CHECK_RE = re.compile(r"^mantle check ok=0 why=(\w+) .*?n=(\d+)")
RESYNC_RE = re.compile(r"^resync why=(warp|rebase) d=([\d.]+)m chunk_d=([\d.a-zA-Z]+)m")
LAND_RE = re.compile(r"^land fall_h=([\d.]+)in .* dmg_pct=([\d.]+)")
LADDER_STATE_RE = re.compile(r"^state from=(-?\d+)\(\w*\??\) to=(-?\d+)")
LADDER_END_RE = re.compile(r"^end state=(-?\d+)")
POS_RE = re.compile(r"pos=" + V3_RE)


def msgs(entries, tag, pat):
    r = re.compile(pat)
    return [(e, m) for e in entries if e["tag"] == tag and (m := r.search(e["msg"]))]


def m8_present(entries):
    return any(e["tag"] == "MOVE" and e["msg"].startswith("cfg ") and " mantle=" in e["msg"] for e in entries)


def cfg_int(entries, key, default):
    for e in entries:
        if e["tag"] == "MOVE" and e["msg"].startswith("cfg ") and (m := re.search(r"\b" + key + r"=(\d+)", e["msg"])):
            return int(m.group(1))
    return default


# ---- mantling ----------------------------------------------------------------------------------------------------

def refusal_totals(entries):
    """(total refusals, {reason: logged count}): the larger of the logged `n=` sums and PERF `mantle_refusals=`."""
    logged = {}
    for _, m in msgs(entries, "MOVE", CHECK_RE.pattern):
        logged[m.group(1)] = logged.get(m.group(1), 0) + int(m.group(2))
    perf = sum(int(m.group(1)) for e in entries if e["tag"] == "PERF" and (m := re.search(r"mantle_refusals=(\d+)", e["msg"])))
    return max(perf, sum(logged.values())), logged


def judge_mantle(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    starts = msgs(entries, "MOVE", START_RE.pattern)
    if not starts:
        # M8 review: many checks and not one start is the detection refusing everything in ER (the start-inside-solid
        # axiom, the ray filter, the geometry), not "not tried"
        total, logged = refusal_totals(entries)
        if total >= REFUSE_ALL_MIN:
            top = max(logged.items(), key=lambda kv: kv[1]) if logged else ("?", 0)
            return "FAIL", (f"detection refuses everything: {total} refusals and no 'MOVE mantle start' (dominant {top[0]} x{top[1]}; "
                            f"by reason {dict(sorted(logged.items()))})")
        return "NEEDS-DATA", (f"no 'MOVE mantle start' ({total} refusal(s)): jump at a 0.6-2.0 m ledge while standing, MW2: hold Space")
    mv = [e for e in entries if e["tag"] in ("MOVE", "FALL")]
    problems, kinds, ok_n = [], {}, 0
    for k, (s, sm) in enumerate(starts):
        t0 = s["t"]
        t_next = starts[k + 1][0]["t"] if k + 1 < len(starts) else float("inf")
        dur = float(sm.group(4))
        kind = sm.group(1)
        kinds[kind] = kinds.get(kind, 0) + 1
        after = [e for e in mv if t0 < e["t"] < t_next]
        end = next(((e, m) for e in after if (m := END_RE.match(e["msg"]))), None)
        abort = next(((e, m) for e in after if (m := ABORT_RE.match(e["msg"]))), None)
        if end is None and abort is None:
            problems.append(f"start at {t0:.2f} s: no 'mantle end' or 'mantle abort'")
            continue
        if any(e["msg"].startswith("WARN position_insane") for e in after if e["t"] - t0 <= dur + 1.0):
            problems.append(f"start at {t0:.2f} s: position_insane")
        if any((m := RESYNC_RE.match(e["msg"])) and m.group(1) == "warp" and e["t"] - t0 <= WARP_AFTER_S for e in after):
            problems.append(f"start at {t0:.2f} s: resync why=warp within {WARP_AFTER_S} s")
        if end is not None and (abort is None or end[0]["t"] <= abort[0]["t"]):
            e, m = end
            dt = float(m.group(1))
            if abs(dt - dur) > 0.1 * dur + TICK + 1e-6:
                problems.append(f"start at {t0:.2f} s: dt {dt:.2f} s vs planned {dur:.2f} s")
            if float(m.group(5)) > DIST_END_MAX_M:
                problems.append(f"start at {t0:.2f} s: end {m.group(5)} m off the planned point")
            if kind != "over":
                er = m.group(8) == "1"
                st = next((sm2 for x in after if x["t"] >= e["t"] and (sm2 := SETTLE_RE.match(x["msg"]))), None)
                if not er and not (st and st.group(1) == "1" and int(st.group(2)) <= SETTLE_TICKS):
                    problems.append(f"start at {t0:.2f} s: ER never reported ground within {SETTLE_TICKS} ticks of the end")
            t_end = e["t"]
            ok_n += 1
        else:
            t_end = abort[0]["t"]
            # an abort is followed by a valid state: a landing, a state line or a handback
            later = [x for x in mv if t_end <= x["t"] <= t_end + ABORT_SETTLE_S]
            if not any(LAND_RE.match(x["msg"]) or x["msg"].startswith(("state ", "suspend ", "off ", "on ")) for x in later):
                problems.append(f"abort at {t_end:.2f} s: no landing / state / handback within {ABORT_SETTLE_S} s")
        for x in mv:
            if t_end <= x["t"] <= t_end + DAMAGE_AFTER_S and (lm := LAND_RE.match(x["msg"])) and float(lm.group(2)) > 0:
                problems.append(f"start at {t0:.2f} s: a damaging landing {lm.group(2)} % within {DAMAGE_AFTER_S} s")
            if t_end <= x["t"] <= t_end + DAMAGE_AFTER_S and x["tag"] == "FALL":
                problems.append(f"start at {t0:.2f} s: a FALL line within {DAMAGE_AFTER_S} s")
    if problems:
        return "FAIL", "; ".join(problems[:3])
    aborts = sum(1 for _ in msgs(entries, "MOVE", ABORT_RE.pattern))
    return "PASS", (f"{len(starts)} mantle(s) {dict(sorted(kinds.items()))}: {ok_n} ended on time on the planned point with ER ground, "
                    f"{aborts} abort(s) settled safely; no warp, damage or position_insane after them")


def judge_mantle_refusals(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    checks = msgs(entries, "MOVE", CHECK_RE.pattern)
    counts = {}
    for _, m in checks:
        counts[m.group(1)] = counts.get(m.group(1), 0) + int(m.group(2))
    if not counts:
        return "PASS", "INFO: no logged refusals (no_face is sampled every 5 s with its count)"
    span_min = max((entries[-1]["t"] - entries[0]["t"]) / 60.0, 1.0)
    warn = [f"{k} {v / span_min:.0f}/min" for k, v in counts.items() if v / span_min > REFUSALS_PER_MIN_WARN]
    return "PASS", (f"INFO refusals by reason {dict(sorted(counts.items()))}" + (f"; WARN frequent: {', '.join(warn)}" if warn else ""))


def judge_mantle_perf(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    cap = cfg_int(entries, "rays", 24)
    main_cap = cfg_int(entries, "rays_max", 32)
    wins, bad = [], []
    for e in entries:
        if e["tag"] != "PERF":
            continue
        mc = re.search(r"mantle_checks=(\d+) mantle_rays_max=(\d+)", e["msg"])
        if not mc or int(mc.group(1)) == 0:
            continue
        tps = re.search(r"ticks_per_s=([\d.]+)", e["msg"])
        rm = re.search(r"\brays_max=(\d+)", e["msg"])
        # M8 review: a hitch tick's budget grows by 4 per extra substep (the DLL logs the window's largest)
        bm = re.search(r"mantle_budget_max=(\d+)", e["msg"])
        wcap = max(cap, int(bm.group(1))) if bm else cap
        wins.append(e)
        if tps and float(tps.group(1)) < TPS_MIN:
            bad.append(f"ticks_per_s {tps.group(1)} with mantle checks")
        if int(mc.group(2)) > wcap:
            bad.append(f"mantle_rays_max {mc.group(2)} > {wcap}")
        if rm and int(rm.group(1)) > main_cap:
            bad.append(f"main rays_max {rm.group(1)} > {main_cap}")
    if not wins:
        return "NEEDS-DATA", "no PERF window with mantle_checks > 0"
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", f"{len(wins)} PERF window(s) with mantle checks: ticks_per_s >= {TPS_MIN:.0f}, mantle_rays_max <= {cap}, main rays_max <= {main_cap}"


# ---- P1 re-base ----------------------------------------------------------------------------------------------------

def judge_rebase(entries):
    rs = msgs(entries, "MOVE", RESYNC_RE.pattern)
    big = [(e, m) for e, m in rs if float(m.group(2)) > 5.0]
    if not big:
        if m8_present(entries):
            return "NEEDS-DATA", "no resync over 5 m (walk or sprint 200 m in a straight line: ER re-bases every ~32 m)"
        return "N/A", "no resync over 5 m and no M8 DLL"
    loads = [e["t"] for e in entries if (e["tag"] == "TASK" and "world" in e["msg"]) or (e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=loading"))]
    bad, rebases = [], 0
    for e, m in big:
        d = float(m.group(2))
        try:
            cd = float(m.group(3))
        except ValueError:
            cd = float("nan")
        grid = REBASE_MIN_M <= d <= REBASE_MAX_M and cd == cd and abs(cd - d) < 9.0
        loading = any(0.0 <= e["t"] - t <= LOAD_GRACE_S for t in loads)
        if m.group(1) == "warp" and grid and not loading:
            bad.append(f"why=warp at a {d:.1f} m chunk-grid jump (chunk_d {cd:.1f}) without a load")
        if m.group(1) == "rebase":
            rebases += 1
            if any(x["tag"] == "MOVE" and x["msg"].startswith("sprint end why=handback") and abs(x["t"] - e["t"]) <= 0.05 for x in entries):
                bad.append(f"sprint ended at the re-base at {e['t']:.2f} s")
    if bad:
        return "FAIL", f"{len(bad)} problem(s): " + "; ".join(bad[:3])
    signs = sorted({sm.group(1) for e, _ in big if (sm := re.search(r"sign=([+-])", e["msg"]))})
    return "PASS", f"{rebases} re-base(s) kept velocity and sprint (sign {'/'.join(signs) or '?'}), {len(big) - rebases} warp(s) with a load or off the grid"


# ---- ladders -----------------------------------------------------------------------------------------------------

def ladder_episodes(entries):
    """[(t_start, t_end or None, start entry, end entry or None)] from the LADDER lines."""
    eps, cur = [], None
    for e in entries:
        if e["tag"] != "LADDER":
            continue
        m = LADDER_STATE_RE.match(e["msg"])
        if m and m.group(1) == "-1" and cur is None:
            cur = [e["t"], None, e, None]
        elif m and m.group(2) == "-1" and cur is not None:
            cur[1] = e["t"]
        elif LADDER_END_RE.match(e["msg"]) and cur is not None:
            cur[1] = cur[1] or e["t"]
            cur[3] = e
            eps.append(tuple(cur))
            cur = None
    if cur is not None:
        eps.append(tuple(cur))
    return eps


def states_at(entries):
    """Functions: driving(t) from MOVE on / suspend / off, fp_on(t) from FP on / off."""
    mv = [(e["t"], e["msg"].startswith("on ")) for e in entries if e["tag"] == "MOVE" and e["msg"].startswith(("on ", "suspend ", "off "))]
    fp = [(e["t"], e["msg"].startswith("on ")) for e in entries if e["tag"] == "FP" and e["msg"].startswith(("on ", "off "))]

    def at(seq, t):
        v = False
        for tt, on in seq:
            if tt > t:
                break
            v = on
        return v
    return (lambda t: at(mv, t)), (lambda t: at(fp, t))


def judge_ladder_handback(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    eps = ladder_episodes(entries)
    if not eps:
        return suspect_needs_data(entries, "no LADDER line (press E at a ladder, climb up and down)")
    driving, fp_on = states_at(entries)
    bad = []
    for t0, t1, s, _ in eps:
        t1x = t1 if t1 is not None else float("inf")
        if driving(t0):
            bad.append(f"ladder at {t0:.2f} s while the controller drives")
        if any(e["tag"] == "MOVE" and e["msg"].startswith("on ") and t0 < e["t"] < t1x for e in entries):
            bad.append(f"MOVE on during the ladder at {t0:.2f} s")
        if any(e["tag"] == "FP" and e["msg"].startswith("on ") and t0 < e["t"] < t1x for e in entries):
            bad.append(f"FP on during the ladder at {t0:.2f} s")
        # the handback before it: the last suspend at or before the start, with its gravity restore before the start
        sus = [e for e in entries if e["tag"] == "MOVE" and re.match(r"^suspend reason=(interact|ladder)", e["msg"]) and t0 - 10.0 <= e["t"] <= t0 + 0.05]
        blk = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("blocked reason=ladder") and t0 - 0.05 <= e["t"] <= t1x]
        if not sus and not any(e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=ladder") and t0 - 0.05 <= e["t"] <= t1x for e in entries):
            bad.append(f"no 'MOVE suspend reason=interact|ladder' before the ladder at {t0:.2f} s")
        if sus:
            ts = sus[-1]["t"]
            if not any(e["tag"] == "MOVE" and e["msg"].startswith("restore field=gravity_multiplier") and ts <= e["t"] <= t0 + 0.05 for e in entries):
                bad.append(f"no gravity restore between the suspend at {ts:.2f} s and the ladder")
        if not blk and not any(e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=ladder") for e in entries if t0 - 0.05 <= e["t"] <= t1x):
            bad.append(f"no 'MOVE blocked reason=ladder' / 'suspend reason=ladder' for the ladder at {t0:.2f} s")
        if fp_on(t0 - 1e-6):
            if not any(e["tag"] == "FP" and e["msg"].startswith("off reason=ladder") and t0 - 0.05 <= e["t"] <= t0 + LADDER_FP_OFF_S for e in entries):
                bad.append(f"FP was on at the ladder at {t0:.2f} s and no 'FP off reason=ladder' within {LADDER_FP_OFF_S} s")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", f"{len(eps)} ladder trip(s): handed back first (gravity restored), ER third person throughout, no MOVE on / FP on on the ladder"


def judge_ladder_dismount(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    eps = [ep for ep in ladder_episodes(entries) if ep[3] is not None]
    if not eps:
        return suspect_needs_data(entries, "no 'LADDER end' (get off a ladder at the top or the bottom)")
    _, fp_on = states_at(entries)
    bad, lat = [], []
    for t0, t1, _, end in eps:
        te = end["t"]
        on = next((e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("on ") and e["t"] >= te), None)
        if on is None or on["t"] - te > DISMOUNT_FAIL_S:
            # a menu / F5 / death right after is not the controller's fault
            other = any(e["tag"] == "MOVE" and re.match(r"^(blocked|suspend|off) reason=(menu|f5|death|loading|config|world_left)", e["msg"]) and te <= e["t"] <= te + DISMOUNT_FAIL_S for e in entries)
            if not other:
                bad.append(f"no 'MOVE on' within {DISMOUNT_FAIL_S:.0f} s of the ladder end at {te:.2f} s")
            continue
        lat.append(on["t"] - te)
        if "er_ground=1" not in on["msg"]:
            bad.append(f"MOVE on after the ladder without ER ground ({on['msg'][:40]})")
        if not any(e["tag"] == "MOVE" and e["msg"].startswith("resync why=handback") and te <= e["t"] <= on["t"] + 0.05 for e in entries):
            bad.append(f"no 'resync why=handback' before the re-arm at {on['t']:.2f} s")
        if fp_on(t0 - 1e-6) and not any(e["tag"] == "FP" and e["msg"].startswith("on ") and te <= e["t"] <= on["t"] + DISMOUNT_INFO_S for e in entries):
            bad.append(f"FP was on before the ladder and not back within {DISMOUNT_INFO_S} s of the re-arm")
        for e in entries:
            if not (te <= e["t"] <= te + DISMOUNT_CLEAN_S) or e["tag"] != "MOVE":
                continue
            m = e["msg"]
            lm = LAND_RE.match(m)
            if m.startswith("resync why=warp") or m.startswith("ground_disagree") or m.startswith("freefall") or (lm and float(lm.group(2)) > 0):
                bad.append(f"after the ladder end at {te:.2f} s: {m[:40]}")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    slow = [x for x in lat if x > DISMOUNT_INFO_S]
    return "PASS", (f"{len(eps)} dismount(s): re-armed in {min(lat):.2f}..{max(lat):.2f} s with ER ground, clean afterwards"
                    + (f"; INFO {len(slow)} re-arm(s) slower than {DISMOUNT_INFO_S} s" if slow else "") if lat else f"{len(eps)} dismount(s), handed to the user's own handbacks")


def judge_ladder_weapon(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    eps = ladder_episodes(entries)
    if not eps:
        return suspect_needs_data(entries, "no LADDER line")
    bad = []
    for t0, t1, _, _ in eps:
        t1x = t1 if t1 is not None else float("inf")
        if any(e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"]) and t0 < e["t"] < t1x for e in entries):
            bad.append(f"SHOT on the ladder at {t0:.2f} s")
        if any(e["tag"] == "RELOAD" and e["msg"].startswith("start ") and t0 < e["t"] < t1x for e in entries):
            bad.append(f"RELOAD start on the ladder at {t0:.2f} s")
        if not any(e["tag"] == "ARSENAL" and e["msg"].startswith("lowered why=ladder") and t0 - 0.1 <= e["t"] <= t1x for e in entries):
            bad.append(f"no 'ARSENAL lowered why=ladder' for the ladder at {t0:.2f} s")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", f"{len(eps)} ladder trip(s): the gun lowered (MW2 quick drop), no shot or reload on the ladder"


def judge_ladder_input(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    eps = ladder_episodes(entries)
    e_n = sum(1 for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=interact"))
    bad = []
    if any(e["tag"] == "MOVE" and "WARN signal interact muted" in e["msg"] for e in entries):
        bad.append("WARN signal interact muted (E would no longer hand back)")
    for t0, t1, _, _ in eps:
        t1x = t1 if t1 is not None else float("inf")
        if any((e["tag"] == "AC130" and e["msg"].startswith("on ")) or (e["tag"] == "NUKE" and e["msg"].startswith("activate")) for e in entries if t0 < e["t"] < t1x):
            bad.append(f"AC-130 / nuke started on the ladder at {t0:.2f} s")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    refused = sum(1 for e in entries if e["tag"] in ("AC130", "NUKE") and e["msg"].startswith("blocked reason=ladder"))
    return "PASS", f"{e_n} E handback(s), never muted; {refused} AC-130 / nuke start(s) refused on a ladder"


def ladder_signal_suspects(entries):
    """E handbacks after which ER moved the player > 1 m vertically while no LADDER episode was seen: the unverified
    fp_player ladder-state read is probably wrong (always -1). (sus, eps, suspects)."""
    eps = ladder_episodes(entries)
    sus = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=interact")]
    bad = []
    for s in sus:
        sm = POS_RE.search(s["msg"])
        on = next((e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("on ") and e["t"] > s["t"]), None)
        if not sm or on is None:
            continue
        om = POS_RE.search(on["msg"])
        if not om:
            continue
        dy = abs(float(om.group(2)) - float(sm.group(2)))
        ladder_seen = any(t0 >= s["t"] - 0.05 and t0 <= on["t"] for t0, _, _, _ in eps)
        if dy > SIGNAL_DY_M and not ladder_seen:
            bad.append(f"E at {s['t']:.2f} s moved the player {dy:.1f} m vertically with ladder=-1")
    return sus, eps, bad


def suspect_needs_data(entries, what):
    """M8 review: with no LADDER line, say whether that is 'not climbed' or 'the ladder signal looks broken'."""
    _, _, bad = ladder_signal_suspects(entries)
    if bad:
        return "NEEDS-DATA", f"NEEDS-DATA (ladder signal suspect): no LADDER line but {bad[0]}; see 'ladder signal'"
    return "NEEDS-DATA", what


def judge_ladder_signal(entries):
    if not m8_present(entries):
        return "N/A", "no 'MOVE cfg ... mantle=' (DLL before M8)"
    sus, eps, bad = ladder_signal_suspects(entries)
    if bad:
        # M8 review: a broken ladder read also leaves B3 without ER confirmation (E mutes after 5 trips): FAIL, not INFO
        return "FAIL", f"the ladder signal looks wrong (fp_player ladder state stayed -1 on a climb): {'; '.join(bad[:2])}"
    return "PASS", f"{len(sus)} E handback(s), {len(eps)} ladder trip(s); no vertical move without a ladder state"


def rows(entries, ctx=None):
    return [
        ("mantle", *judge_mantle(entries)),
        ("mantle refusals", *judge_mantle_refusals(entries)),
        ("mantle perf", *judge_mantle_perf(entries)),
        ("rebase", *judge_rebase(entries)),
        ("ladder handback", *judge_ladder_handback(entries)),
        ("ladder dismount", *judge_ladder_dismount(entries)),
        ("ladder weapon", *judge_ladder_weapon(entries)),
        ("ladder input", *judge_ladder_input(entries)),
        ("ladder signal", *judge_ladder_signal(entries)),
    ]


# ---- the synthetic M8 sessions (self-test and fixtures) ------------------------------------------------------------

CFG = ("cfg enabled=1 tp_anim=hybrid sprint=press crouch=toggle jump_slowdown=1 fall=mw2 lethal_fall=handback eye=1.600/1.067/0.293 "
       "hull=1.867/1.333/0.800 r=er rays_max=32 data=converted\\movement.json sha=2f8c9cfda1b8 mantle=1 key=mw2 max_h=2.00m "
       "rise_max=60in angle=60 over=thin vm=drop rays=24 ladder_er_attacks=0")


def m8_mantle_session(dt_off=False, no_end=False, damage_after=False, end_off=False, warp_after=False, rays_over=False, refuse_all=False):
    """A good M8 mantle session as (t, tag, msg) lines; each keyword breaks one rule."""
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))

    a(0.0, "BOOT", "mw2er 0.3.0 dll=D:\\Mods\\MW2inER\\build\\natives\\mw2er.dll")
    a(0.1, "MOVE", CFG)
    a(1.0, "TASK", "world entered (player handle 0x1 owner_check=ok)")
    a(1.5, "MOVE", "resync why=world_enter d=0.00m chunk_d=0.00m")
    a(1.5, "MOVE", "on pos=(1.00,2.00,3.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1")
    # a re-base while sprinting (P1)
    a(5.0, "MOVE", "resync why=rebase d=32.00m chunk_d=32.98m e=(32.00,8.00,0.00) c=(32.00,8.00,0.00) block=None sign=+ load_age=99.0s")
    # an up mantle from a grounded press (1.0 m)
    a(8.0, "MOVE", "mantle start kind=up rise=1.00m row=39 ledge_y=3.000 from=(1.00,2.00,3.00) end=(1.00,3.00,4.20) dir_deg=0.0 "
                   f"d=0.76m X=1.22m dur=0.50s rays={30 if rays_over else 13} trigger=press air=0 v0y=0.0 retried=0 weapon=drop")
    a(8.0, "ARSENAL", "lowered why=mantle gun=ump45_mp quick_drop_s=0.25")
    if warp_after:
        a(8.3, "MOVE", "resync why=warp d=40.00m chunk_d=nanm e=(40.00,0.00,0.00) c=? block=None sign=none load_age=0.2s")
    if not no_end:
        dt = 0.70 if dt_off else 0.50
        dist = 0.2 if end_off else 0.0
        a(8.0 + dt, "MOVE", f"mantle end dt={dt:.2f}s pos=(1.00,3.00,4.20) dist_end={dist:.3f}m kind=up ground=1 er_ground=0 vel=(0.00,0.00,1.20)")
        a(8.0 + dt + 0.05, "MOVE", "mantle settle er_ground=1 ticks=3 ground=1 pos=(1.00,3.00,4.20)")
        a(8.0 + dt, "ARSENAL", "raise why=mantle gun=ump45_mp quick_raise_s=0.55")
        a(8.0 + dt + 0.55, "ARSENAL", "raised why=mantle gun=ump45_mp")
    if damage_after:
        a(9.5, "MOVE", "land fall_h=236.2in (6.00m) v_impact=614.8 slow=0.37*1.00 dmg_pct=24.1 hp=1000->759 max_hp=1000 land_ms=1344")
    # a 1.8 m ledge caught in the air after the jump
    a(12.0, "MOVE", "jump y0=3.000 v0=249.8 land_factor=1.00")
    a(12.3, "MOVE", "mantle start kind=up rise=1.20m row=45 ledge_y=4.800 from=(1.00,3.60,4.20) end=(1.00,4.80,5.40) dir_deg=0.0 "
                    "d=0.70m X=1.20m dur=0.50s rays=14 trigger=held air=1 v0y=120.0 retried=0 weapon=drop")
    a(12.80, "MOVE", "mantle end dt=0.50s pos=(1.00,4.80,5.40) dist_end=0.000m kind=up ground=1 er_ground=1 vel=(0.00,0.00,1.10)")
    # a thin wall: over, release, the drop
    a(15.0, "MOVE", "mantle start kind=over rise=1.00m row=39 ledge_y=5.800 from=(1.00,4.80,6.00) end=(1.00,5.85,7.40) dir_deg=0.0 "
                    "d=0.50m X=1.34m dur=0.83s rays=14 trigger=press air=0 v0y=0.0 retried=0 weapon=drop drop_to=4.80m")
    a(15.83, "MOVE", "mantle end dt=0.83s pos=(1.00,5.85,7.40) dist_end=0.000m kind=over ground=0 er_ground=0 vel=(0.00,0.00,0.00)")
    a(15.83, "MOVE", "mantle release kind=over pos=(1.00,5.85,7.40) drop_to=4.80m")
    a(16.3, "MOVE", "land fall_h=41.3in (1.05m) v_impact=257.0 slow=0.67*1.00 dmg_pct=0.0 hp=1000->1000 max_hp=1000 land_ms=0")
    # an abort by a handback (menu) mid-climb
    a(20.0, "MOVE", "mantle start kind=up rise=1.40m row=57 ledge_y=6.200 from=(1.00,4.80,8.00) end=(1.00,6.20,9.20) dir_deg=0.0 "
                    "d=0.70m X=1.20m dur=0.50s rays=13 trigger=held air=0 v0y=0.0 retried=0 weapon=drop")
    a(20.3, "MOVE", "mantle abort why=handback:menu t_frac=0.45 pos=(1.00,5.70,8.00) last_valid=1")
    a(20.3, "MOVE", "suspend reason=menu cursor ticks=1100 pos=(1.00,5.70,8.00)")
    a(20.3, "MOVE", "restore field=gravity_multiplier addr=0x1000 old=0.000 new=1.000 ok=1")
    a(20.3, "MOVE", "restore field=gravity_disabled addr=0x1009 old=1 new=0 ok=1")
    a(22.0, "MOVE", "on pos=(1.00,4.80,8.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1")
    # refusals (rate-limited lines) and PERF
    a(24.0, "MOVE", "mantle check ok=0 why=too_high rise=1.70m rays=5 n=1 pos=(1.00,4.80,8.00)")
    a(24.6, "MOVE", "mantle check ok=0 why=no_headroom rise=1.20m rays=12 n=2 pos=(1.00,4.80,8.00)")
    for k, t in enumerate((10.0, 15.0, 20.0, 25.0)):
        a(t, "PERF", f"ticks_per_s=59.9 task_us p50=300 p95=500 max=900 fp_us avg=20 max=40 writes=100 ac130=0 move=1 move_us_p50=60 "
                     f"move_us_p95=120 move_us_max=300 rays_avg=13.0 rays_max=28 ray_us_max=8.0 move_writes=600 mantle_checks={6 + k} "
                     f"mantle_rays_max={30 if rays_over else 15} mantle_budget_max=24 mantle_deferred=0 mantle_us_max=90 mantles=1 mantle_refusals=2 ladder_trips=0")
    if refuse_all:
        # M8 review: the detection refused every check (no start at all): the 'mantle' row must not read as 'not tried'
        L = [x for x in L if x[1] != "ARSENAL" and not (x[1] == "MOVE" and x[2].startswith(("mantle start", "mantle end", "mantle release", "mantle abort", "mantle settle")))]
        for k in range(4):
            a(26.0 + 5.0 * k, "MOVE", f"mantle check ok=0 why=no_face rise=0.00m rays=2 n={3 + k} pos=(1.00,4.80,8.00) fwd=(0.00,1.00)")
    a(30.0, "TASK", "world left (loading / title)")
    return L


def m8_ladder_session(no_restore=False, move_on_mid=False, shot_on_ladder=False, fp_not_off=False, muted=False, ac130_on=False,
                      slow_dismount=False, no_ladder_lines=False):
    """A good ladder session (the run-18 step 8 sequence) as (t, tag, msg) lines; each keyword breaks one rule."""
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))

    a(0.0, "BOOT", "mw2er 0.3.0 dll=D:\\Mods\\MW2inER\\build\\natives\\mw2er.dll")
    a(0.1, "MOVE", CFG)
    a(1.0, "TASK", "world entered (player handle 0x1 owner_check=ok)")
    a(1.5, "MOVE", "resync why=world_enter d=0.00m chunk_d=0.00m")
    a(1.5, "MOVE", "on pos=(1.00,2.00,3.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1")
    a(2.0, "FP", "on eye=(1.00,3.60,3.00) cam_gate=ok er_cam_dist=3.5m eye_to_chest=0.4m hp=500 ride=Some(0) mounted=0 ladder=-1")
    for trip in range(4):
        t = 10.0 + 30.0 * trip
        a(t, "MOVE", "suspend reason=interact E pressed (hold 1.5 s) ticks=500 pos=(1.00,2.00,3.00)")
        if not (no_restore and trip == 0):
            a(t, "MOVE", "restore field=gravity_multiplier addr=0x1000 old=0.000 new=1.000 ok=1")
            a(t, "MOVE", "restore field=gravity_disabled addr=0x1009 old=1 new=0 ok=1")
        a(t + 0.4, "MOVE", "blocked reason=ladder state=0")
        if not (fp_not_off and trip == 0):
            a(t + 0.4, "FP", "off reason=ladder state=0 ticks_on=480 cam_writes=0 (re-arms when clear)")
        a(t + 0.4, "LADDER", "state from=-1(None) to=0(StartBottom) pos=(1.00,2.00,3.00) top=(1.00,9.00,3.50) bottom=(1.00,2.00,3.50) er_ground=1 move=suspended fp=1")
        a(t + 0.4, "ARSENAL", "lowered why=ladder gun=ump45_mp quick_drop_s=0.25")
        a(t + 0.45, "MOVE", "interact cleared why=ladder state=0")
        a(t + 1.0, "LADDER", "state from=0(StartBottom) to=2(UpRight) pos=(1.00,2.40,3.20) top=(1.00,9.00,3.50) bottom=(1.00,2.00,3.50) er_ground=0 move=suspended fp=0")
        if move_on_mid and trip == 0:
            a(t + 2.0, "MOVE", "on pos=(1.00,5.00,3.20) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=0")
        if shot_on_ladder and trip == 0:
            a(t + 2.5, "SHOT", "n=5 mag=24/150 id=79112000 mw2=40 d=8.0m src=chr gun=ump45_mp")
        if ac130_on and trip == 0:
            a(t + 3.0, "AC130", "on center=(1.0,2.0,3.0) radius_m=120")
        else:
            a(t + 3.0, "AC130", "blocked reason=ladder (ER drives the climb; F7 again on the ground)")
        a(t + 5.0, "LADDER", "state from=3(UpLeft) to=6(EndTop) pos=(1.00,8.80,3.20) top=(1.00,9.00,3.50) bottom=(1.00,2.00,3.50) er_ground=0 move=suspended fp=0")
        a(t + 6.0, "LADDER", "state from=6(EndTop) to=-1(None) pos=(1.00,9.00,3.80) top=? bottom=? er_ground=1 move=arming fp=0")
        a(t + 6.0, "LADDER", "end state=6(EndTop) dur=5.60s dy=7.00m ups=12 downs=0 attacks=0 coast=0 pos=(1.00,9.00,3.80)")
        a(t + 6.0, "ARSENAL", "raise why=ladder gun=ump45_mp quick_raise_s=0.55")
        a(t + 6.55, "ARSENAL", "raised why=ladder gun=ump45_mp")
        t_on = t + 6.0 + (12.0 if slow_dismount and trip == 0 else 0.25)
        a(t_on, "MOVE", "resync why=handback d=0.00m chunk_d=0.00m")
        a(t_on, "MOVE", "on pos=(1.00,9.00,3.80) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=0")
        a(t_on, "MOVE", "write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=0x1000 old=1.000 new=0.000 ok=1")
        a(t_on + 0.5, "FP", "on eye=(1.00,10.60,3.80) cam_gate=ok er_cam_dist=3.5m eye_to_chest=0.4m hp=500 ride=Some(0) mounted=0 ladder=-1")
    if muted:
        a(140.0, "MOVE", "WARN signal interact muted for this session after 5 unconfirmed handbacks in 60 s")
    a(150.0, "TASK", "world left (loading / title)")
    if no_ladder_lines:
        # M8 review: the fp_player ladder read broken (always -1): no LADDER / ladder-state lines, the climb itself
        # (E handback, 7 m up, re-arm) still in the log
        L = [x for x in L if x[1] != "LADDER" and "reason=ladder" not in x[2] and "why=ladder" not in x[2]]
    return L


def to_entries(lines):
    out = []
    for t, tag, msg in sorted(lines, key=lambda x: x[0]):
        ms = int(round(t * 1000))
        raw = f"2026-10-18 13:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [task] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def to_log(lines):
    return "\n".join(e["raw"] for e in to_entries(lines)) + "\n"


MANTLE_MUTATIONS = [
    ("dt off by 40 %", dict(dt_off=True), "mantle"),
    ("no end after a start", dict(no_end=True), "mantle"),
    ("damage after the end", dict(damage_after=True), "mantle"),
    ("end 0.2 m off", dict(end_off=True), "mantle"),
    ("warp right after the start", dict(warp_after=True), "mantle"),
    ("mantle rays over the budget", dict(rays_over=True), "mantle perf"),
    ("detection refuses everything (M8 review)", dict(refuse_all=True), "mantle"),
]
LADDER_MUTATIONS = [
    ("missing gravity restore", dict(no_restore=True), "ladder handback"),
    ("MOVE on mid-ladder", dict(move_on_mid=True), "ladder handback"),
    ("SHOT on the ladder", dict(shot_on_ladder=True), "ladder weapon"),
    ("FP not off on the ladder", dict(fp_not_off=True), "ladder handback"),
    ("interact muted", dict(muted=True), "ladder input"),
    ("AC-130 on during the ladder", dict(ac130_on=True), "ladder input"),
    ("slow dismount 12 s", dict(slow_dismount=True), "ladder dismount"),
    ("ladder signal broken: no LADDER lines, the climb kept (M8 review)", dict(no_ladder_lines=True), "ladder signal"),
]
MANTLE_ROWS = ("mantle", "mantle refusals", "mantle perf", "rebase")
LADDER_ROWS = ("ladder handback", "ladder dismount", "ladder weapon", "ladder input", "ladder signal")


def self_test():
    """(ok, caught, lines): the good sessions PASS their rows; each mutation FAILs its row; a pre-M8 log is N/A."""
    out, ok, caught = [], True, 0
    for name, sess, want_rows, muts in (("mantle", m8_mantle_session, MANTLE_ROWS, MANTLE_MUTATIONS),
                                        ("ladder", m8_ladder_session, LADDER_ROWS, LADDER_MUTATIONS)):
        good = {n: (s, d) for n, s, d in rows(to_entries(sess()))}
        bad = [f"{n}: {good[n][0]} {good[n][1][:90]}" for n in want_rows if good[n][0] != "PASS"]
        if bad:
            ok = False
        out.append(f"good {name} session: {sum(1 for n in want_rows if good[n][0] == 'PASS')}/{len(want_rows)} rows PASS" + (f" ({bad})" if bad else ""))
        for label, kw, row in muts:
            r = {n: (s, d) for n, s, d in rows(to_entries(sess(**kw)))}
            if r[row][0] == "FAIL":
                caught += 1
            else:
                ok = False
                out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:80]}")
    # M8 review: with the ladder signal broken, the other ladder rows say so instead of a plain "not climbed"
    r = {n: (s, d) for n, s, d in rows(to_entries(m8_ladder_session(no_ladder_lines=True)))}
    for n in ("ladder handback", "ladder dismount", "ladder weapon"):
        if r[n][0] != "NEEDS-DATA" or "ladder signal suspect" not in r[n][1]:
            ok = False
            out.append(f"MISSED: {n} with a broken ladder signal: {r[n]}")
    # and an honest "not tried" (no E, no mantle) stays NEEDS-DATA without the suspect note / the refuse-all FAIL
    quiet = [x for x in m8_ladder_session() if x[1] not in ("LADDER",) and not x[2].startswith(("suspend reason=interact", "blocked reason=ladder"))][:6]
    rq = {n: (s, d) for n, s, d in rows(to_entries(quiet))}
    if rq["ladder handback"][0] != "NEEDS-DATA" or "suspect" in rq["ladder handback"][1] or rq["mantle"][0] != "NEEDS-DATA":
        ok = False
        out.append(f"MISSED: a quiet session must be plain NEEDS-DATA: {rq['ladder handback']} {rq['mantle']}")
    # a log of an older DLL: N/A on every M8 row except rebase (N/A without > 5 m resyncs)
    old = [(0.0, "BOOT", "mw2er 0.2.0"), (0.1, "MOVE", CFG.split(" mantle=")[0]), (1.0, "MOVE", "resync why=world_enter d=0.00m chunk_d=0.00m")]
    st = {n: s for n, s, _ in rows(to_entries(old))}
    if any(v != "N/A" for v in st.values()):
        ok = False
        out.append(f"MISSED: a pre-M8 log must be N/A on every M8 row: {st}")
    # the same older log with a chunk-grid warp (run 17): rebase FAILs
    old.append((5.0, "MOVE", "resync why=warp d=32.00m chunk_d=32.00m block=Some([1.0, 2.0, 3.0, 0.0])"))
    if {n: s for n, s, _ in rows(to_entries(old))}["rebase"] != "FAIL":
        ok = False
        out.append("MISSED: a chunk-grid warp without a load must FAIL rebase")
    out.append(f"{caught}/{len(MANTLE_MUTATIONS) + len(LADDER_MUTATIONS)} M8 mutations FAIL their row")
    return ok, caught, out
