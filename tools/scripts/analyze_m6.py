"""M6 rows of analyze_log.py (docs/M6_DESIGN.md section 12.2): the 34-gun arsenal, switching, per-gun shots and hits,
the UMP45 calibration of the flat_inv rows, burst / rechamber / shell reload / pellets / launchers / lock, and the two
run-13 bugs (ac130 start, spawn). Imported by analyze_log.py; pure Python, read-only.

Expectations come from the data files: build/rows.json's `arsenal` block (band ids, MW2 damage, atkPhys, launcher
rows) and converted/guns/<gun>/stats.json (fireTime, rechamberTime, reloadTime, dropTime, raiseTime, shotCount,
fireType). Rows that need an arsenal DLL are N/A for older logs; `ac130 start` and `spawn` also judge older logs (the
run-13 fixture is the bugs as they were reported).
"""
import json
import pathlib
import re
import statistics

TICK = 1.0 / 60.0
FIRE_TOL = 0.017
BURST_GAP_S = 0.25
SWITCH_TOL = 0.1 + TICK      # drop / raise durations vs the logged expectation (one task tick of slack)
SWITCH_DATA_TOL = 0.02       # the logged drop_s / raise_s vs dropTime / raiseTime
VRAM_MB_MAX = 128.0
UMP45 = "ump45_mp"
CAL_TARGET = 4311            # Limgrave Godrick Soldier (KNOWN_TARGETS of analyze_log.py)
CAL_CLOSE = 49               # s * 40 on d_ref 85 (docs/M6_DESIGN.md 9.3): the run-14 target
CAL_TOL = 1
CAL_MIN = 3
OPENER_RATIO = 1.25          # first hits ~1.4x (opener) / weak parts 1.5-2x are excluded from calibration [I]
PROJ_LIFE_S = 30.0
D_EFF_SPREAD = 0.10         # review M6: one D_eff must fit every atk level within 10 %

M6_ROWS = ("arsenal", "switching", "shots per gun", "hits per gun", "ump45 calibration", "burst", "rechamber",
           "shell reload", "pellets", "launchers", "lock", "ac130 start", "spawn")

SHOT_GUN_RE = re.compile(r"^n=(\d+) mag=(\d+)/(\d+) id=(\d+) mw2=(\d+) .*? gun=(\S+) pellets=(\d+) spawned=(\d+) "
                         r"dropped=(\d+) bands=(\S+) ads=([\d.]+)")
HIT_RE = re.compile(r"^chr=c(\d{4,}) hp (-?\d+)->(-?\d+) dmg=(-?\d+) band=(-?\d+) mw2=(-?\d+) .*?via=(\w+) n=(\d+) "
                    r"kill=([01]) gun=(\S+)")
HIT_LAUNCHER_RE = re.compile(r"^chr=(\S+) hp=(-?\d+)->(-?\d+) dmg=(-?\d+) src=launcher gun=(\S+) tier=(\w+) kill=([01])")


# ---- data -----------------------------------------------------------------------------------------------------------

def load_guns(root):
    """{name: stats} of every converted gun (empty when converted/guns is missing)."""
    out = {}
    d = pathlib.Path(root) / "converted" / "guns"
    if not d.is_dir():
        return out
    for p in sorted(d.iterdir()):
        f = p / "stats.json"
        if f.is_file():
            try:
                s = json.loads(f.read_text(encoding="utf-8"))
                out[s.get("name", p.name)] = s
            except (OSError, ValueError):
                pass
    return out


def num(ctx, gun, key, default=None):
    s = (ctx.get("guns") or {}).get(gun)
    if s is None:
        return default
    v = s.get("num", {}).get(key, default)
    return float(v) if isinstance(v, (int, float)) else default


def arsenal_rows(ctx):
    return ((ctx.get("rows") or {}).get("arsenal") or {}).get("guns") or {}


def bands_of(ctx, gun):
    return {int(b["id"]): b for b in arsenal_rows(ctx).get(gun, {}).get("bands", [])}


def weapon_view_m6(ctx, weapon, legacy):
    """The MODE-on weapon's expectations for an arsenal session: its arsenal bands (79100000+) next to the legacy
    rows (still in the package for the A/B switch), fireTime / clip from its stats.json."""
    if not arsenal_rows(ctx).get(weapon):
        return legacy, None if legacy else f"weapon {weapon!r} not in rows.json's arsenal block"
    bands = dict(legacy["bands"]) if legacy else {}
    for i, b in bands_of(ctx, weapon).items():
        bands[i] = {"id": i, "mw2_damage": b["mw2"], "atk_phys": b["atk"]}
    ft, clip = num(ctx, weapon, "fireTime"), num(ctx, weapon, "clipSize")
    view = dict(legacy) if legacy else {"name": weapon, "speed": float((ctx.get("rows") or {}).get("bullet", {}).get("speed_mps", 999.0))}
    view.update({"bands": bands, "params_n": None})
    if ft is not None:
        view["fire_time"] = ft
    if clip is not None:
        view["clip"] = int(clip)
    return view, None


# ---- helpers --------------------------------------------------------------------------------------------------------

def present(entries):
    return any(e["tag"] == "ARSENAL" and (e["msg"].startswith("data ") or e["msg"].startswith("loaded "))
               for e in entries)


def na_reason(entries):
    return None if present(entries) else "no 'ARSENAL data' line (DLL before M6 or [arsenal] enabled = false)"


def msgs(entries, tag, pat):
    r = re.compile(pat)
    return [(e, m) for e in entries if e["tag"] == tag and (m := r.search(e["msg"]))]


def gun_shots(entries):
    out = {}
    for e, m in msgs(entries, "SHOT", SHOT_GUN_RE.pattern):
        out.setdefault(m.group(6), []).append((e, m))
    return out


def fmt_ms(x):
    return f"{x * 1000:.0f} ms"


# ---- rows -----------------------------------------------------------------------------------------------------------

def judge_arsenal(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    data = msgs(entries, "ARSENAL", r"^data n=(\d+) ok=(\d+) disabled=(\d+)")
    loaded = msgs(entries, "ARSENAL", r"^loaded n=(\d+) ok=(\d+) ms=(\d+) vram_mb=([\d.]+)")
    params = msgs(entries, "PARAMS", r"^OK guns=(\d+) rows=(\d+) mode=(\w+)")
    disabled = msgs(entries, "ARSENAL", r"^disable gun=(\S+) why=(\w+)")
    fails = msgs(entries, "ARSENAL", r"^FAIL ")
    problems = []
    if not data:
        problems.append("no 'ARSENAL data' line")
    if not loaded:
        problems.append("no 'ARSENAL loaded' line (the viewmodel loader did not finish)")
    if not params:
        problems.append("no 'PARAMS OK guns=' line")
    if fails:
        problems.append(fails[0][0]["msg"][:80])
    if disabled:
        problems.append("disabled: " + ", ".join(f"{m.group(1)} ({m.group(2)})" for _, m in disabled[:6]))
    n_rows = len(arsenal_rows(ctx))
    if data:
        n, ok = int(data[0][1].group(1)), int(data[0][1].group(2))
        if ok != n:
            problems.append(f"data ok={ok} of n={n}")
        if n_rows and n != n_rows:
            problems.append(f"data n={n} but rows.json has {n_rows} guns")
    if loaded:
        lm = loaded[0][1]
        if int(lm.group(2)) != int(lm.group(1)):
            problems.append(f"viewmodel ok={lm.group(2)} of n={lm.group(1)}")
        if float(lm.group(4)) > VRAM_MB_MAX:
            problems.append(f"vram_mb={lm.group(4)} > {VRAM_MB_MAX:.0f}")
    if data and params and int(params[0][1].group(1)) != int(data[0][1].group(2)):
        problems.append(f"PARAMS OK guns={params[0][1].group(1)} != data ok={data[0][1].group(2)}")
    if params and params[0][1].group(3) != "flat_inv":
        problems.append(f"rows mode={params[0][1].group(3)} (want flat_inv)")
    if problems:
        return "FAIL", "; ".join(problems)
    lm = loaded[0][1]
    return "PASS", (f"{data[0][1].group(2)}/{data[0][1].group(1)} guns loaded (data), viewmodel {lm.group(2)}/{lm.group(1)} "
                    f"in {lm.group(3)} ms, {lm.group(4)} MB VRAM; {params[0][0]['msg'][:70]}")


def judge_switching(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    sw = [e for e in entries if e["tag"] == "SWITCH"]
    starts = [e for e in sw if e["msg"].startswith("start ")]
    if not starts:
        return "NEEDS-DATA", "no 'SWITCH start' line (switch guns with the wheel and keys 1 / 2)"
    problems, vias, durs = [], {}, []
    for i, e in enumerate(sw):
        m = re.match(r"^start from=(\S+) to=(\S+) via=(\w+) drop_s=([\d.]+)", e["msg"])
        if m:
            vias[m.group(3)] = vias.get(m.group(3), 0) + 1
            nxt = next((x for x in sw[i + 1:] if x["msg"].startswith(("raise ", "start "))), None)
            reversed_ = "reversed" in e["msg"]
            want_drop = num(ctx, m.group(1), "dropTime")
            if want_drop is not None and not reversed_ and abs(float(m.group(4)) - want_drop) > SWITCH_DATA_TOL:
                problems.append(f"drop_s={m.group(4)} for {m.group(1)} != dropTime {want_drop}")
            if nxt is not None and nxt["msg"].startswith("raise "):
                dt = nxt["t"] - e["t"]
                durs.append(("drop", dt))
                if abs(dt - float(m.group(4))) > SWITCH_TOL:
                    problems.append(f"drop {m.group(1)} took {dt:.2f} s (logged drop_s {m.group(4)})")
            continue
        m = re.match(r"^raise gun=(\S+) first=([01]) raise_s=([\d.]+)", e["msg"])
        if m:
            key = "firstRaiseTime" if m.group(2) == "1" else "raiseTime"
            want = num(ctx, m.group(1), key)
            if want is not None and want > 0 and abs(float(m.group(3)) - want) > SWITCH_DATA_TOL:
                problems.append(f"raise_s={m.group(3)} for {m.group(1)} != {key} {want}")
            nxt = next((x for x in sw[i + 1:] if x["msg"].startswith(("raised ", "start "))), None)
            if nxt is not None and nxt["msg"].startswith("raised "):
                dt = nxt["t"] - e["t"]
                durs.append(("raise", dt))
                if abs(dt - float(m.group(3))) > SWITCH_TOL:
                    problems.append(f"raise {m.group(1)} took {dt:.2f} s (logged raise_s {m.group(3)})")
    # no shot while a switch is in flight (start .. raised)
    windows, t0 = [], None
    for e in sw:
        if e["msg"].startswith("start ") and t0 is None:
            t0 = e["t"]
        elif e["msg"].startswith("raised ") and t0 is not None:
            windows.append((t0, e["t"]))
            t0 = None
    shots = [e["t"] for e in entries if e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"])]
    during = [t for t in shots if any(a < t < b for a, b in windows)]
    if during:
        problems.append(f"{len(during)} SHOT line(s) while switching")
    # review M6: the task's safety net fired (the machine was Ready while the viewmodel waited for a raise)
    unstuck = [e for e in sw if e["msg"].startswith("vm_unstuck ")]
    if unstuck:
        problems.append(f"{len(unstuck)} 'SWITCH vm_unstuck' (a switch ended without a raise event: {unstuck[0]['msg'][:60]})")
    raised = sum(1 for e in sw if e["msg"].startswith("raised "))
    blocked = sum(1 for e in sw if e["msg"].startswith("blocked "))
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", (f"{len(starts)} switch(es) via {dict(sorted(vias.items()))}, {raised} raised, {blocked} blocked; "
                    f"{len(durs)} drop/raise durations within {SWITCH_TOL:.2f} s of dropTime/raiseTime; no SHOT while switching")


def judge_shots_per_gun(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    by = gun_shots(entries)
    if not by:
        return "NEEDS-DATA", "no 'SHOT ... gun=' line"
    problems, parts = [], []
    for gun, rows in sorted(by.items()):
        ft = num(ctx, gun, "fireTime")
        times = [e["t"] for e, _ in rows]
        gaps = [b - a for a, b in zip(times, times[1:]) if b - a < max(BURST_GAP_S, 1.5 * (ft or 0))]
        fire_type = ((ctx.get("guns") or {}).get(gun) or {}).get("fireType", "?")
        part = f"{gun} {len(rows)}"
        if ft is not None and gaps:
            fastest = min(gaps)
            if fastest < ft - FIRE_TOL:
                problems.append(f"{gun}: an interval of {fmt_ms(fastest)} < fireTime {fmt_ms(ft)}")
            if fire_type == "Full Auto" and len(gaps) >= 5:
                med = statistics.median(gaps)
                part += f" (median {fmt_ms(med)} vs {fmt_ms(ft)})"
                if abs(med - ft) > FIRE_TOL:
                    problems.append(f"{gun}: median in-burst interval {fmt_ms(med)} vs fireTime {fmt_ms(ft)}")
        parts.append(part)
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", f"{sum(len(v) for v in by.values())} SHOT lines on {len(by)} gun(s): " + ", ".join(parts)


def d_eff(atk, dmg, er_damage):
    """Effective defence D with er_damage(atk, D) == dmg (bisection; None when out of the curve's range)."""
    lo, hi = 1.0, 5000.0
    if not (er_damage(atk, hi) <= dmg <= er_damage(atk, lo)):
        return None
    for _ in range(80):
        mid = 0.5 * (lo + hi)
        if er_damage(atk, mid) > dmg:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)


def judge_hits_per_gun(entries, ctx, er_damage):
    na = na_reason(entries)
    if na:
        return "N/A", na
    hits = msgs(entries, "HIT", HIT_RE.pattern)
    lhits = msgs(entries, "HIT", HIT_LAUNCHER_RE.pattern)
    if not hits and not lhits:
        return "NEEDS-DATA", "no 'HIT ... gun=' line"
    problems, per, deffs = [], {}, []
    for e, m in hits:
        gun, band, mw2, dmg = m.group(10), int(m.group(5)), int(m.group(6)), int(m.group(4))
        kill, n = m.group(9) == "1", int(m.group(8))
        per[gun] = per.get(gun, 0) + 1
        b = bands_of(ctx, gun).get(band)
        if b is None:
            problems.append(f"{gun}: band {band} is not one of its rows")
            continue
        if int(b["mw2"]) != mw2:
            problems.append(f"{gun}: mw2={mw2} but band {band} has {b['mw2']}")
            continue
        if dmg <= 0:
            problems.append(f"{gun}: dmg={dmg}")
            continue
        if int(m.group(1)) == CAL_TARGET and not kill and n == 1 and dmg <= OPENER_RATIO * er_damage(int(b["atk"]), 85.0):
            d = d_eff(int(b["atk"]), dmg, er_damage)
            if d is not None:
                deffs.append((int(b["atk"]), d))
    for e, m in lhits:
        per[m.group(5) + "(expl)"] = per.get(m.group(5) + "(expl)", 0) + 1
        if int(m.group(4)) <= 0:
            problems.append(f"{m.group(5)} explosion hit dmg={m.group(4)}")
    # review M6: the MW2 damage ratios between guns hold on c4311 only if one D_eff fits every atk level; with >= 2
    # levels of >= CAL_MIN hits each, a spread of the per-level medians above D_EFF_SPREAD fails (needs calibration)
    levels = {}
    for atk, d in deffs:
        levels.setdefault(atk, []).append(d)
    meds = {atk: statistics.median(v) for atk, v in levels.items() if len(v) >= CAL_MIN}
    if len(meds) >= 2:
        lo, hi = min(meds.values()), max(meds.values())
        if hi / lo - 1.0 > D_EFF_SPREAD:
            calib = (f"D_eff per atk level {dict(sorted((a, round(v)) for a, v in meds.items()))} "
                     f"spreads {100 * (hi / lo - 1):.0f} % > {100 * D_EFF_SPREAD:.0f} %: MW2 damage ratios are not kept on "
                     f"c{CAL_TARGET} (one d_ref cannot fit every gun: report the levels)")
            if not problems:
                return "NEEDS-CALIBRATION", calib
            problems.append(calib)
    if problems:
        return "FAIL", "; ".join(problems[:4])
    extra = ""
    if deffs:
        allv = [d for _, d in deffs]
        extra = (f"; fitted D_eff on c{CAL_TARGET} median {statistics.median(allv):.0f} over {len(allv)} hit(s), per atk level "
                 f"{dict(sorted((a, round(statistics.median(v))) for a, v in levels.items()))} "
                 f"(rows built with d_ref {((ctx.get('rows') or {}).get('arsenal') or {}).get('d_ref', '?')}; "
                 f"suggested d_ref {statistics.median(allv):.0f})")
        if len(meds) < 2:
            extra += f"; ratio check needs >= {CAL_MIN} hits at 2 atk levels (e.g. UMP45 close + an AR / sniper)"
    return "PASS", f"{len(hits)} bullet + {len(lhits)} explosion HIT(s): {dict(sorted(per.items()))}{extra}"


def judge_ump45_calibration(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    close = min(bands_of(ctx, UMP45), default=None)
    if close is None:
        return "FAIL", "rows.json has no ump45_mp arsenal bands"
    vals, excl = [], []
    for e, m in msgs(entries, "HIT", HIT_RE.pattern):
        if m.group(10) != UMP45 or int(m.group(1)) != CAL_TARGET or int(m.group(5)) != close:
            continue
        if m.group(9) == "1" or int(m.group(8)) != 1:
            continue
        dmg = int(m.group(4))
        if dmg > OPENER_RATIO * CAL_CLOSE:
            excl.append(dmg)    # opener / weak part
            continue
        vals.append(dmg)
    if len(vals) < CAL_MIN:
        return "NEEDS-DATA", (f"{len(vals)} single non-kill close-band ({close}) UMP45 hit(s) on c{CAL_TARGET} {vals} "
                              f"(want >= {CAL_MIN}); excluded opener/weak-part {excl}")
    med = statistics.median(vals)
    detail = (f"c{CAL_TARGET} close band {close}: {len(vals)} hit(s) {sorted(vals)[:10]} median {med:g} vs "
              f"{CAL_CLOSE} +- {CAL_TOL}; excluded opener/weak-part {excl}")
    if abs(med - CAL_CLOSE) <= CAL_TOL:
        return "PASS", detail
    return "FAIL", detail + " -> adjust [damage] d_ref (see 'hits per gun' D_eff) and rebuild the regulation"


def judge_burst(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    bs = msgs(entries, "BURST", r"^gun=(\S+) n=(\d+) t_ms=(\d+)")
    if not bs:
        return "NEEDS-DATA", "no 'BURST' line (fire a burst gun: FAMAS / M16 / ...)"
    problems = []
    for e, m in bs:
        gun, n, t_ms = m.group(1), int(m.group(2)), int(m.group(3))
        ft = num(ctx, gun, "fireTime")
        # review M6: the round count comes from fireType ("3-Round Burst"); the first-to-last span of n rounds is
        # (n - 1) x fireTime (+ one task tick)
        fm = re.match(r"^(\d+)-Round Burst", ((ctx.get("guns") or {}).get(gun) or {}).get("fireType", ""))
        want_n = int(fm.group(1)) if fm else 3
        if n != want_n:
            problems.append(f"{gun}: burst of {n} (fireType says {want_n})")
        if ft is not None and t_ms > ((want_n - 1) * ft + TICK) * 1000:
            problems.append(f"{gun}: burst span {t_ms} ms > {want_n - 1} x fireTime + a tick ({((want_n - 1) * ft + TICK) * 1000:.0f} ms)")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", f"{len(bs)} burst(s) with the fireType round count, first-to-last within (n-1) x fireTime + a tick"


def judge_rechamber(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    rc = [e for e in entries if e["tag"] == "RECHAMBER"]
    starts = [e for e in rc if e["msg"].startswith("start ")]
    if not starts:
        return "NEEDS-DATA", "no 'RECHAMBER start' line (fire a bolt-action gun)"
    problems = []
    for e in starts:
        m = re.match(r"^start gun=(\S+) t=([\d.]+)", e["msg"])
        if not m:
            problems.append(f"unparsed: {e['msg'][:60]}")
            continue
        gun, t = m.group(1), float(m.group(2))
        want = num(ctx, gun, "rechamberTime")
        if want is not None and abs(t - want) > TICK:
            problems.append(f"{gun}: t={t} vs rechamberTime {want}")
        # M7 A4: a rechamber ends in its done, or in `RECHAMBER cancelled gun=.. reason=..` (switch, sprint, mode off,
        # AC-130, nuke, a reload); only a done is timed
        done = next((x for x in rc if x["t"] >= e["t"] and (x["msg"].startswith(f"done gun={gun}") or x["msg"].startswith(f"cancelled gun={gun}"))), None)
        nxt = next((x for x in starts if x["t"] > e["t"]), None)
        if done is None or (nxt is not None and done["t"] > nxt["t"]):
            problems.append(f"{gun}: start without its done / cancelled")
        elif done["msg"].startswith("cancelled "):
            continue
        elif abs(done["t"] - e["t"] - t) > 0.05 + TICK:
            problems.append(f"{gun}: done after {done['t'] - e['t']:.3f} s (t={t})")
    # the next shot of the gun never comes before fireTime + rechamberTime after the previous one
    for gun, rows in gun_shots(entries).items():
        rt, ft = num(ctx, gun, "rechamberTime"), num(ctx, gun, "fireTime")
        if not rt or ft is None or not any(re.match(rf"^start gun={re.escape(gun)} ", e["msg"]) for e in starts):
            continue
        times = [e["t"] for e, _ in rows]
        fast = [b - a for a, b in zip(times, times[1:]) if b - a < ft + rt - FIRE_TOL]
        if fast:
            problems.append(f"{gun}: a shot {min(fast):.3f} s after the previous (< fireTime + rechamberTime {ft + rt:.3f})")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", f"{len(starts)} rechamber(s), each t == rechamberTime, done on time, no shot inside a cycle"


def judge_shell_reload(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    sh = msgs(entries, "RELOAD", r"^shell gun=(\S+) added=(\d+) mag=(\d+) reserve=(\d+)")
    if not sh:
        return "NEEDS-DATA", "no 'RELOAD shell' line (reload a shotgun with a segmented reload)"
    problems, runs = [], []
    prev = None
    for e, m in sh:
        gun, mag = m.group(1), int(m.group(3))
        if prev and prev[1] == gun and e["t"] - prev[0] < 3.0 and mag == prev[2] + int(m.group(2)):
            runs[-1][1].append(e["t"] - prev[0])
        else:
            runs.append((gun, []))
        if prev and prev[1] == gun and mag < prev[2] and e["t"] - prev[0] < 3.0:
            problems.append(f"{gun}: mag went {prev[2]} -> {mag}")
        clip = num(ctx, gun, "clipSize")
        if clip is not None and mag > clip:
            problems.append(f"{gun}: mag {mag} > clipSize {int(clip)}")
        prev = (e["t"], gun, mag)
    # review M6: the cadence is judged per gun against that gun's reloadTime (SPAS 0.667 vs Model 1887 0.85)
    by_gun = {}
    for gun, gaps_ in runs:
        by_gun.setdefault(gun, []).extend(gaps_)
    parts = []
    for gun, gaps in sorted(by_gun.items()):
        want = num(ctx, gun, "reloadTime")
        if not gaps or want is None:
            continue
        med = statistics.median(gaps)
        parts.append(f"{gun} median {med:.3f} s (reloadTime {want})")
        if abs(med - want) > TICK + 0.02:
            problems.append(f"{gun}: shell cadence median {med:.3f} s vs reloadTime {want}")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", f"{len(sh)} shell(s) added; cadence per gun: " + (", ".join(parts) or "no consecutive shells")


def judge_pellets(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    multi = [(e, m) for g, rows in gun_shots(entries).items() for e, m in rows if (num(ctx, g, "shotCount") or 1) > 1]
    if not multi:
        return "NEEDS-DATA", "no shotgun SHOT line"
    problems = []
    for e, m in multi:
        gun, pel, sp, dr = m.group(6), int(m.group(7)), int(m.group(8)), int(m.group(9))
        sc = int(num(ctx, gun, "shotCount") or 1)
        if pel != sc:
            problems.append(f"{gun}: pellets={pel} != shotCount {sc}")
        if sp + dr > pel or sp > sc:
            problems.append(f"{gun}: spawned {sp} + dropped {dr} > pellets {pel}")
    # review M6: the HITs of a shotgun shot (its gun, between that SHOT and the next SHOT / LAUNCH) never exceed the
    # pellets it spawned, per enemy and in total
    bounds = sorted(e["t"] for e in entries if (e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"])) or
                    (e["tag"] == "LAUNCH" and e["msg"].startswith("gun=")))
    hits = msgs(entries, "HIT", HIT_RE.pattern)
    n_hits, worst = 0, 0
    for e, m in multi:
        gun, sp = m.group(6), int(m.group(8))
        t_next = next((t for t in bounds if t > e["t"]), float("inf"))
        mine = [hm for x, hm in hits if e["t"] <= x["t"] < t_next and hm.group(10) == gun]
        n_hits += len(mine)
        per = {}
        for hm in mine:
            per[hm.group(1)] = per.get(hm.group(1), 0) + 1
        worst = max([worst, *per.values()])
        if len(mine) > sp:
            problems.append(f"{gun}: {len(mine)} HIT(s) after one shot of {sp} spawned pellet(s)")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    return "PASS", (f"{len(multi)} shotgun shot(s): pellets == shotCount, spawned + dropped <= pellets; {n_hits} pellet "
                    f"HIT(s), at most {worst} on one enemy per shot (<= spawned)")


def judge_launchers(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    launches = msgs(entries, "LAUNCH", r"^gun=(\S+) id=(\d+) v=([\d.]+) (?:\S+ )*?lock=(\w+)")
    if not launches:
        return "NEEDS-DATA", "no 'LAUNCH gun=' line (fire a launcher)"
    hits = msgs(entries, "PROJ", r"^hit kind=(\w+) gun=(\S+) id=(\d+)")
    expl = msgs(entries, "PROJ", r"^explode gun=(\S+) tiers=(\S+) ids=(\S+)")
    efail = msgs(entries, "PROJ", r"SPAWN FAIL")
    problems = []
    for e, m in launches:
        pid = m.group(2)
        mine = [(x, hm) for x, hm in hits if hm.group(3) == pid and 0 <= x["t"] - e["t"] <= PROJ_LIFE_S]
        if not mine:
            problems.append(f"{m.group(1)} id={pid}: no PROJ hit within {PROJ_LIFE_S:.0f} s")
            continue
        h = mine[0]
        # a dud / expire line for the projectile (the self guard follows its 'hit kind=map' line) needs no explosion
        ended = any(hm.group(1) in ("dud", "expire") for _, hm in mine)
        if not ended and not any(0 <= x["t"] - h[0]["t"] <= 0.1 and xm.group(1) == m.group(1) for x, xm in expl):
            problems.append(f"{m.group(1)} id={pid}: hit kind={h[1].group(1)} without a 'PROJ explode'")
        rows = arsenal_rows(ctx).get(m.group(1), {}).get("launcher")
        if rows is None:
            problems.append(f"{m.group(1)}: not a launcher in rows.json")
        # review M6: the logged launch speed is the weapon file's projectileSpeed (in/s -> m/s, logged to 0.1)
        ps = num(ctx, m.group(1), "projectileSpeed")
        if ps is not None and ps > 0 and abs(float(m.group(3)) - ps * 0.0254) > 0.06:
            problems.append(f"{m.group(1)} id={pid}: v={m.group(3)} m/s != projectileSpeed {ps:g} in/s ({ps * 0.0254:.1f} m/s)")
    if efail:
        problems.append(f"{len(efail)} explosion SPAWN FAIL")
    # review M6: the player's HP 0.6 s after each launcher explosion (the self guard / selfTarget 0 must spare him)
    selfc = msgs(entries, "PROJ", r"^selfcheck gun=(\S+) id=(\d+) hp_before=(-?\d+) hp_after=(-?\d+) lost=(\d+)")
    hurt = [(m.group(1), m.group(2), int(m.group(5))) for _, m in selfc if int(m.group(5)) > 0]
    if hurt:
        problems.append("player HP lost within 0.6 s of a launcher blast: " + ", ".join(f"{g} id={i} -{n}" for g, i, n in hurt[:3]))
    duds = msgs(entries, "PROJ", r"^hit kind=dud gun=\S+ id=\d+ why=self_radius")
    lhits = msgs(entries, "HIT", HIT_LAUNCHER_RE.pattern)
    if problems:
        return "FAIL", "; ".join(problems[:4])
    kinds = {}
    for _, hm in hits:
        kinds[hm.group(1)] = kinds.get(hm.group(1), 0) + 1
    return "PASS", (f"{len(launches)} launch(es) at projectileSpeed, every one ended in a PROJ hit {dict(sorted(kinds.items()))}, "
                    f"{len(expl)} explosion(s), {len(lhits)} explosion HIT(s); {len(selfc)} selfcheck(s) without player HP "
                    f"loss, {len(duds)} self-radius dud(s)")


def judge_lock(entries, ctx):
    na = na_reason(entries)
    if na:
        return "N/A", na
    lk = [e for e in entries if e["tag"] == "LOCK"]
    launches = msgs(entries, "LAUNCH", r"^gun=(stinger_mp|javelin_mp) id=(\d+) v=[\d.]+ (?:\S+ )*?lock=(\w+)")
    refused = msgs(entries, "LAUNCH", r"^refused gun=(\S+) why=no_lock")
    if not lk and not launches:
        return "NEEDS-DATA", "no 'LOCK' line (aim the Stinger / Javelin at an enemy)"
    problems = []
    for e, m in launches:
        if m.group(3) in ("0", "none", "off"):
            if m.group(1) == "stinger_mp" and not any(x["msg"].startswith("lock=off") for x in entries if x["tag"] == "LAUNCH"):
                problems.append(f"stinger launched without a lock (lock={m.group(3)})")
            continue
        acq = [x for x in lk if x["t"] <= e["t"] and x["msg"].startswith(f"acquired gun={m.group(1)}")]
        if not acq:
            problems.append(f"{m.group(1)} id={m.group(2)}: lock={m.group(3)} without a 'LOCK acquired' before it")
    for e in lk:
        m = re.match(r"^acquired gun=(\S+) target=\S+ t=([\d.]+)", e["msg"])
        if m and float(m.group(2)) <= 0:
            problems.append(f"{m.group(1)}: acquired after t={m.group(2)}")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    acq = sum(1 for e in lk if e["msg"].startswith("acquired "))
    lost = sum(1 for e in lk if e["msg"].startswith("lost "))
    return "PASS", f"{acq} lock(s) acquired, {lost} lost, {len(launches)} guided launch(es), {len(refused)} 'no lock' refusal(s)"


def judge_ac130_start(entries, ctx):
    """Run 13 bug 1: 'AC130 off reason=loading t_s=0.0' in the world (a stale load edge aborted every start)."""
    ac = [e for e in entries if e["tag"] == "AC130" or (e["tag"] == "TASK" and "world " in e["msg"])]
    if not any(e["tag"] == "AC130" for e in ac):
        return "N/A", "no AC130 line"
    in_world, bad, ons = False, [], 0
    for e in ac:
        m = e["msg"]
        if e["tag"] == "TASK":
            in_world = "world entered" in m
            continue
        if m.startswith("on center="):
            ons += 1
        if in_world and re.match(r"^off reason=loading t_s=0\.0\b", m):
            bad.append(e)
    merged = [e for e in ac if e["tag"] == "AC130" and e["msg"].startswith("start aborted reason=")]
    backoff = [e for e in ac if e["tag"] == "AC130" and e["msg"].startswith("blocked reason=retry_backoff")]
    info = f"{ons} ride(s); {len(merged)} merged 'start aborted' line(s), {len(backoff)} retry_backoff block(s)"
    if bad:
        return "FAIL", f"{len(bad)} 'AC130 off reason=loading t_s=0.0' while in the world (run-13 bug 1); {info}"
    return "PASS", f"no start aborted by a stale load edge in the world; {info}"


def judge_spawn(entries, ctx):
    """Run 13 bug 2: SPAWN FAIL code=4 bursts. With M6 every failure names its cause and the pool counters."""
    fails = [e for e in entries if e["tag"] == "SPAWN" and e["msg"].startswith("FAIL ")]
    pools = msgs(entries, "SPAWN", r"^pool bullets=(\d+)/(\d+) sfx=(\d+)/(\d+) .*live_all=(\d+) spawns_world=(\d+)")
    pool_info = ""
    if pools:
        pool_info = (f"; SPAWN pool lines {len(pools)}: bullets max {max(int(m.group(1)) for _, m in pools)}, "
                     f"sfx max {max(int(m.group(3)) for _, m in pools)}, live_all max {max(int(m.group(5)) for _, m in pools)}")
    if not fails:
        shots = sum(1 for e in entries if e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"]))
        return "PASS", f"0 SPAWN FAIL over {shots} SHOT line(s){pool_info}"
    causes, no_cause, unknown_with_pool = {}, 0, 0
    first_sw = None
    for e in fails:
        m = re.search(r"\bcause=(\w+)", e["msg"])
        if not m:
            no_cause += 1
            continue
        causes[m.group(1)] = causes.get(m.group(1), 0) + 1
        if m.group(1) == "unknown" and "bullets=" in e["msg"]:
            unknown_with_pool += 1
        sw = re.search(r"spawns_world=(\d+)", e["msg"])
        if sw and first_sw is None:
            first_sw = int(sw.group(1))
    detail = (f"{len(fails)} SPAWN FAIL: causes {dict(sorted(causes.items()))}, {no_cause} without a cause"
              f"{'' if first_sw is None else f', spawns_world at the first {first_sw}'}{pool_info}")
    if unknown_with_pool:
        return "FAIL", detail + f" -> {unknown_with_pool} cause=unknown with pool data (send the log)"
    if causes:
        return "FAIL", detail + " -> run-13 bug 2 still happens (the cause says why)"
    return "NEEDS-DATA", detail + " (a DLL before M6: no cause logged)"


def rows(entries, ctx, er_damage):
    return [
        ("arsenal", *judge_arsenal(entries, ctx)),
        ("switching", *judge_switching(entries, ctx)),
        ("shots per gun", *judge_shots_per_gun(entries, ctx)),
        ("hits per gun", *judge_hits_per_gun(entries, ctx, er_damage)),
        ("ump45 calibration", *judge_ump45_calibration(entries, ctx)),
        ("burst", *judge_burst(entries, ctx)),
        ("rechamber", *judge_rechamber(entries, ctx)),
        ("shell reload", *judge_shell_reload(entries, ctx)),
        ("pellets", *judge_pellets(entries, ctx)),
        ("launchers", *judge_launchers(entries, ctx)),
        ("lock", *judge_lock(entries, ctx)),
        ("ac130 start", *judge_ac130_start(entries, ctx)),
        ("spawn", *judge_spawn(entries, ctx)),
    ]


# ---- synthetic session + context (self-test and fixture) -----------------------------------------------------------

SYN_GUNS = {
    "ump45_mp": {"fireType": "Full Auto", "num": {"fireTime": 0.09, "rechamberTime": 0.1, "reloadTime": 2.5, "dropTime": 0.8,
                                                  "raiseTime": 0.76, "firstRaiseTime": 0.0, "shotCount": 1, "clipSize": 32}},
    "famas_mp": {"fireType": "3-Round Burst", "num": {"fireTime": 0.06, "rechamberTime": 0.1, "reloadTime": 2.76,
                                                     "dropTime": 0.83, "raiseTime": 0.83, "firstRaiseTime": 0.83,
                                                     "shotCount": 1, "clipSize": 30}},
    "cheytac_mp": {"fireType": "Single Shot", "num": {"fireTime": 0.05, "rechamberTime": 0.866, "reloadTime": 2.268,
                                                      "dropTime": 1.2, "raiseTime": 1.0, "shotCount": 1, "clipSize": 5}},
    "spas12_mp": {"fireType": "Single Shot", "num": {"fireTime": 0.47, "rechamberTime": 0.467, "reloadTime": 0.667,
                                                     "dropTime": 0.83, "raiseTime": 1.075, "shotCount": 8, "clipSize": 8}},
    "at4_mp": {"fireType": "Single Shot", "num": {"fireTime": 0.5, "reloadTime": 0.0, "dropTime": 0.66, "raiseTime": 0.66,
                                                  "shotCount": 1, "clipSize": 1, "projectileSpeed": 2500}},
    "stinger_mp": {"fireType": "Full Auto", "num": {"fireTime": 1.06, "reloadTime": 3.0, "dropTime": 0.7, "raiseTime": 1.0,
                                                    "shotCount": 0, "clipSize": 1, "projectileSpeed": 4000}},
    "model1887_mp": {"fireType": "Single Shot", "num": {"fireTime": 0.43, "reloadTime": 0.85, "dropTime": 0.5, "raiseTime": 0.5,
                                                        "shotCount": 8, "clipSize": 7}},
}


OTHER_GUNS = ("m4_mp", "scar_mp", "tavor_mp", "fal_mp", "m16_mp", "masada_mp", "fn2000_mp", "ak47_mp", "mp5k_mp",
              "kriss_mp", "p90_mp", "uzi_mp", "sa80_mp", "rpd_mp", "mg4_mp", "aug_mp", "m240_mp", "barrett_mp",
              "wa2000_mp", "m21_mp", "aa12_mp", "striker_mp", "ranger_mp", "m1014_mp", "m79_mp",
              "rpg_mp", "javelin_mp")
N_GUNS = 34
assert len(OTHER_GUNS) + 7 == N_GUNS


def synthetic_arsenal():
    """rows.json's arsenal block for SYN_GUNS (UMP45 bands as built: 40..35 -> atk 103..96)."""
    def bands(g, mw2s, atks):
        return [{"band": i, "id": 79_100_000 + g * 1000 + i, "mw2": d, "atk": a} for i, (d, a) in enumerate(zip(mw2s, atks))]
    guns = {
        "ump45_mp": {"g": 12, "class": "smg", "bands": bands(12, [40, 39, 38, 37, 36, 35], [103, 101, 100, 98, 97, 96])},
        "famas_mp": {"g": 2, "class": "ar", "bands": bands(2, [40, 39], [103, 101])},
        # review M6: locTorsoUpper 1.5 -> the body band is 105 (atk 187 = inv_er(1.225 x 105, 85))
        "cheytac_mp": {"g": 31, "class": "sniper", "bands": bands(31, [105], [187]), "body_mult": 1.5},
        "model1887_mp": {"g": 46, "class": "shotgun", "bands": bands(46, [35, 34], [96, 94])},
        "spas12_mp": {"g": 41, "class": "shotgun", "bands": bands(41, [40, 39], [103, 101])},
        "at4_mp": {"g": 51, "class": "launcher", "bands": [], "launcher": {"impact": {"id": 79151500, "mw2": 1000, "atk": 1361},
                                                                           "marker_id": 79151501}},
        "stinger_mp": {"g": 54, "class": "launcher", "bands": [], "launcher": {"impact": {"id": 79154500, "mw2": 1000, "atk": 1361},
                                                                               "marker_id": 79154501}},
    }
    # the other 28 guns of the real list (names only; no shot of theirs in the synthetic session)
    for i, name in enumerate(OTHER_GUNS):
        guns.setdefault(name, {"g": 60 + i, "class": "?", "bands": []})
    return {"mode": "flat_inv", "s": 1.225, "d_ref": 85.0, "id_base": 79_100_000, "guns": guns}


def m6_session(guns_ok=N_GUNS, switch_drop=0.80, shot_while_switching=False, burst_span_ms=130, rechamber_t=0.866,
               shell_gap=0.667, pellets=8, launch_proj=True, lock_before=True, ump_close=49, ac_bad=False,
               spawn_fail=None, vram=61.5, band_bad=False, shot_inside=False, at4_v=63.5, pellet_hits=5, self_lost=0,
               sniper_dmg=129, shells_1887=True):
    """A made-up M6 session in the DLL's line shapes (inserted into the synthetic M2..M5 log)."""
    L = []

    def a(tag, msg, dt=0.0, thread="task"):
        L.append((tag, msg, dt, thread))

    a("TASK", "world entered (player handle 0xffffffff16f00000 owner_check=ok)", 0.1)
    a("ARSENAL", f"data n={N_GUNS} ok={guns_ok} disabled={N_GUNS - guns_ok} sound_aliases=150 sound_files=80 rows mode=flat_inv s=1.225 d_ref=85 ms=12", 0.1, "boot")
    if guns_ok < N_GUNS:
        a("ARSENAL", "disable gun=famas_mp why=stats (stats.json: missing field)", 0.0, "boot")
    a("ARSENAL", f"loaded n={N_GUNS} ok={N_GUNS} ms=1900 vram_mb={vram} gun_bytes=64000000 textures_shared=60 uploads={N_GUNS} "
                 f"fmt=R8G8B8A8_UNORM", 0.2, "vmload")
    a("PARAMS", f"OK guns={guns_ok} rows=269 mode=flat_inv s=1.225 d_ref=85 bullet_sfx=none", 0.1)
    a("MOVE", "class gun=ump45_mp class=smg scale=1", 0.1)
    # switching ump45 -> famas by the wheel
    a("SWITCH", f"start from=ump45_mp to=famas_mp via=wheel drop_s={switch_drop:.2f}", 0.5)
    if shot_while_switching:
        a("SHOT", "n=900 mag=20/150 id=79112000 mw2=40 d=8.0m src=chr spread=1.00 origin=(0,0,0) dir=(0,0,1) gun=ump45_mp "
                  "pellets=1 spawned=1 dropped=0 bands=79112000x1 ads=0.00 cone=1.00 kick=(0.00,0.00) capped=0", 0.2)
        a("SWITCH", "raise gun=famas_mp first=1 raise_s=0.83", 0.6)
    else:
        a("SWITCH", "raise gun=famas_mp first=1 raise_s=0.83", 0.8)
    a("SWITCH", "raised gun=famas_mp t=0.83 first=1", 0.83)
    a("MOVE", "class gun=famas_mp class=rifle scale=0.95", 0.0)
    # a FAMAS burst
    for k in range(3):
        a("SHOT", f"n={100 + k} mag={29 - k}/150 id=79102000 mw2=40 d=8.0m src=chr spread=1.00 origin=(0,0,0) dir=(0,0,1) "
                  f"gun=famas_mp pellets=1 spawned=1 dropped=0 bands=79102000x1 ads=0.00 cone=1.00 kick=(0.40,0.10) capped=0",
          0.5 if k == 0 else 0.06)
    a("BURST", f"gun=famas_mp n=3 t_ms={burst_span_ms}", 0.01)
    # UMP45 back by key 2 and its calibration hits on c4311
    a("SWITCH", "start from=famas_mp to=ump45_mp via=key2 drop_s=0.83", 0.5)
    a("SWITCH", "raise gun=ump45_mp first=0 raise_s=0.76", 0.83)
    a("SWITCH", "raised gun=ump45_mp t=0.76 first=0", 0.76)
    for k in range(6):
        a("SHOT", f"n={200 + k} mag={31 - k}/160 id=79112000 mw2=40 d=6.0m src=chr spread=1.00 origin=(0,0,0) dir=(0,0,1) "
                  f"gun=ump45_mp pellets=1 spawned=1 dropped=0 bands=79112000x1 ads=1.00 cone=0.00 kick=(0.30,0.05) capped=0",
          0.5 if k == 0 else 0.07)
        dmg = 69 if k == 0 else ump_close
        band = 79112001 if band_bad and k == 1 else 79112000
        a("HIT", f"chr=c4311 hp {600 - 60 * k}->{600 - 60 * k - dmg} dmg={dmg} band={band} mw2=40 d=6.0m via=impact n=1 "
                 f"kill=0 gun=ump45_mp", 0.02)
    # Intervention: shot, rechamber, shot
    a("SWITCH", "start from=ump45_mp to=cheytac_mp via=key1 drop_s=0.80", 0.5)
    a("SWITCH", "raise gun=cheytac_mp first=0 raise_s=1.00", 0.8)
    a("SWITCH", "raised gun=cheytac_mp t=1.00 first=0", 1.0)
    a("SHOT", "n=300 mag=4/35 id=79131000 mw2=105 d=40.0m src=chr spread=0.00 origin=(0,0,0) dir=(0,0,1) gun=cheytac_mp "
              "pellets=1 spawned=1 dropped=0 bands=79131000x1 ads=1.00 cone=0.00 kick=(1.20,0.30) capped=0", 0.5)
    a("HIT", f"chr=c4311 hp 400->{400 - sniper_dmg} dmg={sniper_dmg} band=79131000 mw2=105 d=40.0m via=impact n=1 kill=0 "
             f"gun=cheytac_mp", 0.02)
    a("RECHAMBER", f"start gun=cheytac_mp t={rechamber_t:.3f} ads=1", 0.03)
    if shot_inside:
        a("SHOT", "n=302 mag=3/35 id=79131000 mw2=105 d=40.0m src=chr spread=0.00 origin=(0,0,0) dir=(0,0,1) gun=cheytac_mp "
                  "pellets=1 spawned=1 dropped=0 bands=79131000x1 ads=1.00 cone=0.00 kick=(1.20,0.30) capped=0", 0.3)
        a("RECHAMBER", "done gun=cheytac_mp", 0.566)
    else:
        a("RECHAMBER", "done gun=cheytac_mp", 0.866)
    a("SHOT", "n=301 mag=3/35 id=79131000 mw2=105 d=40.0m src=chr spread=0.00 origin=(0,0,0) dir=(0,0,1) gun=cheytac_mp "
              "pellets=1 spawned=1 dropped=0 bands=79131000x1 ads=1.00 cone=0.00 kick=(1.20,0.30) capped=0",
      0.2)
    a("HIT", f"chr=c4311 hp 300->{300 - sniper_dmg} dmg={sniper_dmg} band=79131000 mw2=105 d=40.0m via=impact n=1 kill=0 "
             f"gun=cheytac_mp", 0.02)
    a("RECHAMBER", "start gun=cheytac_mp t=0.866 ads=1", 0.03)
    a("RECHAMBER", "done gun=cheytac_mp", 0.866)
    a("SHOT", "n=303 mag=2/35 id=79131000 mw2=105 d=40.0m src=chr spread=0.00 origin=(0,0,0) dir=(0,0,1) gun=cheytac_mp "
              "pellets=1 spawned=1 dropped=0 bands=79131000x1 ads=1.00 cone=0.00 kick=(1.20,0.30) capped=0", 0.2)
    a("HIT", f"chr=c4311 hp 200->{200 - sniper_dmg} dmg={sniper_dmg} band=79131000 mw2=105 d=40.0m via=impact n=1 kill=0 "
             f"gun=cheytac_mp", 0.02)
    a("RECHAMBER", "start gun=cheytac_mp t=0.866 ads=1", 0.03)
    a("RECHAMBER", "done gun=cheytac_mp", 0.866)
    # SPAS-12: a shot (8 pellets), shell reload
    a("SWITCH", "start from=cheytac_mp to=spas12_mp via=key1 drop_s=1.20", 0.5)
    a("SWITCH", "raise gun=spas12_mp first=0 raise_s=1.07", 1.2)
    a("SWITCH", "raised gun=spas12_mp t=1.07 first=0", 1.075)
    a("SHOT", f"n=400 mag=5/56 id=79141000 mw2=40 d=5.0m src=chr spread=4.00 origin=(0,0,0) dir=(0,0,1) gun=spas12_mp "
              f"pellets={pellets} spawned={pellets} dropped=0 bands=79141000x{pellets} ads=0.00 cone=4.00 kick=(0.80,0.20) capped=0", 0.5)
    # review M6: per-pellet HITs of that shot (two enemies), never more than the spawned pellets
    for k in range(pellet_hits):
        c = 4311 if k % 2 == 0 else 4300
        a("HIT", f"chr=c{c} hp {500 - 49 * k}->{451 - 49 * k} dmg=49 band=79141000 mw2=40 d=5.0m via=impact n=1 kill=0 "
                 f"gun=spas12_mp", 0.004)
    a("RECHAMBER", "start gun=spas12_mp t=0.467 ads=0", 0.47)
    a("RECHAMBER", "done gun=spas12_mp", 0.467)
    a("RELOAD", "segment gun=spas12_mp phase=start dur=1.000", 0.3)
    for k in range(3):
        a("RELOAD", f"shell gun=spas12_mp added=1 mag={6 + k} reserve={55 - k}", 1.2 if k == 0 else shell_gap)
    a("RELOAD", "segment gun=spas12_mp phase=end dur=1.000", 0.2)
    # review M6: a second segmented shotgun with its own reloadTime (0.85): the cadence is judged per gun
    if shells_1887:
        a("RELOAD", "segment gun=model1887_mp phase=start dur=0.900", 0.5)
        for k in range(3):
            a("RELOAD", f"shell gun=model1887_mp added=1 mag={4 + k} reserve={30 - k}", 0.9 if k == 0 else 0.85)
        a("RELOAD", "segment gun=model1887_mp phase=end dur=0.700", 0.2)
    # AT4 at a wall, Stinger with a lock
    a("SWITCH", "start from=spas12_mp to=at4_mp via=wheel drop_s=0.83", 0.5)
    a("SWITCH", "raise gun=at4_mp first=0 raise_s=0.66", 0.83)
    a("SWITCH", "raised gun=at4_mp t=0.66 first=0", 0.66)
    a("LAUNCH", f"gun=at4_mp id=1 v={at4_v:.1f} v0={at4_v:.1f} cruise={at4_v:.1f} lock=none mag=0/0 origin=(0,0,0) dir=(0,0,1)", 0.5)
    if launch_proj:
        a("PROJ", "hit kind=world gun=at4_mp id=1 d=30.1 t=0.13 armed=1", 0.13)
        a("PROJ", "explode gun=at4_mp tiers=core+ring ids=79151601+79151602 at=(0,0,30)", 0.0)
        a("HIT", "chr=4311 hp=400->284 dmg=116 src=launcher gun=at4_mp tier=core kill=0", 0.05)
        a("PROJ", f"selfcheck gun=at4_mp id=1 hp_before=500 hp_after={500 - self_lost} lost={self_lost}", 0.55)
    a("SWITCH", "start from=at4_mp to=stinger_mp via=wheel drop_s=0.66", 0.5)
    a("SWITCH", "raise gun=stinger_mp first=0 raise_s=1.00", 0.66)
    a("SWITCH", "raised gun=stinger_mp t=1.00 first=0", 1.0)
    if lock_before:
        a("LOCK", "start gun=stinger_mp target=chr#1f", 0.3)
        a("LOCK", "acquired gun=stinger_mp target=chr#1f t=1.00", 1.0)
    a("LAUNCH", "gun=stinger_mp id=2 v=101.6 v0=10.2 cruise=101.6 lock=chr mag=0/0 origin=(0,0,0) dir=(0,0,1)", 0.2)
    a("PROJ", "hit kind=chr gun=stinger_mp id=2 d=40.0 t=0.70 armed=1", 0.7)
    a("PROJ", "explode gun=stinger_mp tiers=core+ring ids=79154601+79154602 at=(0,0,40)", 0.0)
    # bug 1: an AC-130 start in the world (fixed DLL: a merged abort line, no 'off reason=loading t_s=0.0')
    a("AC130", "start aborted reason=loading x3 in 1.2s", 0.5)
    a("AC130", "blocked reason=retry_backoff (an aborted start waits 1.0 s)", 0.1)
    if ac_bad:
        a("AC130", "off reason=loading t_s=0.0 shots=105:0/40:0/25:0 explosions=0 hits=0 kills=0", 0.1)
    # bug 2: pool counters
    a("SPAWN", "pool bullets=40/256 sfx=60/192 sfx_empty_spot=1 unk40=3/128 counter=500 live_ours=12 live_all=40 spawns_world=500", 0.1)
    if spawn_fail:
        a("SPAWN", spawn_fail, 0.1)
    return L


def synthetic_ctx(base):
    ctx = dict(base)
    rows = dict(ctx.get("rows") or {})
    rows["arsenal"] = synthetic_arsenal()
    ctx["rows"] = rows
    ctx["guns"] = SYN_GUNS
    return ctx


def self_test(run, synthetic, base_ctx, fixture, prefix=()):
    """Good session PASSes all 13 rows; each mutation fails its row; a pre-M6 log is N/A on the arsenal rows.
    `prefix`: lines before the M6 ones (the M4 AC-130 ride, so the AC-130 rows of the fixture have a ride to judge)."""
    ctx = synthetic_ctx(base_ctx)
    prefix = list(prefix)
    good = run(synthetic(True, ac=prefix + m6_session()), ctx)
    st = {n: s for s, n, _ in good if n in M6_ROWS}
    assert all(v == "PASS" for v in st.values()), f"good M6 session: {st} " + str([d for s, n, d in good if n in M6_ROWS and s != 'PASS'])
    muts = [
        ("a gun disabled", dict(guns_ok=N_GUNS - 1), "arsenal"),
        ("VRAM 140 MB", dict(vram=140.0), "arsenal"),
        ("drop_s 0.50 for the UMP45 (dropTime 0.8)", dict(switch_drop=0.50), "switching"),
        ("a SHOT while switching", dict(shot_while_switching=True), "switching"),
        ("burst span 400 ms", dict(burst_span_ms=400), "burst"),
        ("rechamber t=0.5 (rechamberTime 0.866)", dict(rechamber_t=0.5), "rechamber"),
        ("a bolt shot inside the rechamber", dict(shot_inside=True), "rechamber"),
        ("shell cadence 0.3 s (reloadTime 0.667)", dict(shell_gap=0.3), "shell reload"),
        ("6 pellets (shotCount 8)", dict(pellets=6), "pellets"),
        ("a launch without a PROJ hit", dict(launch_proj=False), "launchers"),
        ("a Stinger launch without LOCK acquired", dict(lock_before=False), "lock"),
        ("UMP45 close hits 60 (want 49)", dict(ump_close=60), "ump45 calibration"),
        ("AC130 off reason=loading t_s=0.0 in the world", dict(ac_bad=True), "ac130 start"),
        ("SPAWN FAIL cause=unknown with pool data", dict(spawn_fail="FAIL code=4 (3=invalid bullet id, 4=construction failed) "
                                                      "cause=unknown bullets=40/256 sfx=60/192 sfx_empty_spot=1 unk40=3/128 "
                                                      "counter=500 live_ours=12 live_all=40 spawns_world=500 marker_probe=ok "
                                                      "id=79112000 gun=ump45_mp"), "spawn"),
        ("HIT band of another gun's row", dict(band_bad=True), "hits per gun"),
        # review M6
        ("burst 50 % slower than MW2 (180 ms for 3 rounds at fireTime 60 ms)", dict(burst_span_ms=180), "burst"),
        ("9 pellet HITs after one shot of 8 pellets", dict(pellet_hits=9), "pellets"),
        ("AT4 launched at 5 m/s (projectileSpeed 63.5 m/s)", dict(at4_v=5.0), "launchers"),
        ("player HP lost 30 within 0.6 s of an AT4 blast", dict(self_lost=30), "launchers"),
    ]
    # review M6: hits whose D_eff differs between atk levels need calibration, not a FAIL of the data path
    muts_cal = [("sniper hits imply another D_eff than the UMP45's (ratios not kept)", dict(sniper_dmg=150), "hits per gun")]
    for label, kw, row in muts:
        res = run(synthetic(True, ac=prefix + m6_session(**kw)), ctx)
        got = next(s for s, n, _ in res if n == row)
        assert got == "FAIL", f"M6 mutation {label!r}: {row} = {got}, want FAIL"
    for label, kw, row in muts_cal:
        res = run(synthetic(True, ac=prefix + m6_session(**kw)), ctx)
        got = next(s for s, n, _ in res if n == row)
        assert got == "NEEDS-CALIBRATION", f"M6 mutation {label!r}: {row} = {got}, want NEEDS-CALIBRATION"
    # an old-format SPAWN FAIL (no cause) is NEEDS-DATA, not FAIL
    res = run(synthetic(True, ac=prefix + m6_session(spawn_fail="FAIL code=4 (3=invalid bullet id, 4=construction failed) id=79000100")), ctx)
    assert next(s for s, n, _ in res if n == "spawn") == "NEEDS-DATA", "an old SPAWN FAIL without cause must be NEEDS-DATA"
    old = run(synthetic(True), ctx)
    arsenal_rows_ = [r for r in M6_ROWS if r not in ("ac130 start", "spawn")]
    assert [next(s for s, n, _ in old if n == r) for r in arsenal_rows_] == ["N/A"] * len(arsenal_rows_), \
        "a pre-M6 log must be N/A on the arsenal rows"
    if fixture is not None:
        assert fixture.exists(), f"{fixture} missing (python -I analyze_log.py --write-m6-fixture)"
        assert fixture.read_text(encoding="utf-8") == synthetic(True, ac=prefix + m6_session()), (
            f"{fixture.name} differs from m6_session() (regenerate with --write-m6-fixture)")
        fres = run(fixture.read_text(encoding="utf-8"), ctx)
        bad = [(n, s) for s, n, _ in fres if s not in ("PASS", "N/A")]
        assert not bad, f"the M6 fixture must PASS every row: {bad}"
    return len(muts) + len(muts_cal) + 1
