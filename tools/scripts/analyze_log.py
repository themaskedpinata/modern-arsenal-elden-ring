"""Turn logs/mw2er.log from a user's in-game run into PASS/FAIL lines for the M2 + M3 + M3b + M4 criteria.

Usage:
  python -I analyze_log.py [logs/mw2er.log] [--session N]   (N is 1-based; default: the last session)
  python -I analyze_log.py --self-test

Sessions are split on `BOOT mw2er` lines. Expected values come from the data files, not from this script:
converted/weapons.json (fireTime), build/rows.json (band ids, MW2 damage, atkPhys, row count) and
mw2er.toml ([hit] window_s). The weapon is the one named on the session's `MODE on weapon=` line
(else mw2er.toml gameplay.weapon). Criteria (docs/M2_DESIGN.md section 22, MODDING_PLAN M2 row):
  version gate     VERSION PASS: 2.7.2.0 (or post-port 2.7.1.0) with its table, PE header and address preflight (a pre-port 2.7.1.0 line without table= also passes)
  boot gate        BOOTGATE PASS, TASK registered, PARAMS OK n=<2 x bands + 1 from rows.json>
  HUD              HUD initialize display=...; no repeated 'Couldn't find command queue pointer'
                   (the user still has to confirm crosshair / ammo / hitmarker visually)
  input            INPUT PASS; MENU transitions present (inventory opened at least once)
  fire rate        >= 20 SHOT lines in held bursts, median interval = fireTime +- one 60 Hz frame; no SPAWN FAIL
  engine bullets   BULLET id=... speed ~ rows.json speed; at least one IMPACT ... kind=hit
  hits             'HIT chr=c#### hp a->b dmg=N band=<id> mw2=<D> [via=impact|last_hit_by n=K kill=0|1]' lines,
                   each checked: dmg > 0, band id and mw2 agree with rows.json, a SHOT within hit.window_s before
                   it, dmg plausible for the band's atkPhys (kill=1 lines are exempt: their damage is the remaining
                   HP). Calibration (docs/M3_DESIGN.md section 11) uses single (n=1), non-kill hits on a reference
                   enemy (KNOWN_TARGETS). The only measurement is M2 run 2: c4311 Godrick Soldier took 69, 49, 49,
                   49 (+ a 3 HP kill) at k=4 in the LAST falloff band 79000105 (atkPhys 140, d ~ 36 m), NOT at close
                   range. PASS band 35..75 applies to hits in that measured band: PASS needs >= 3 such hits, their
                   median in the band and >= 75% of them in it. Hits in other bands (e.g. close-range 79000100,
                   atkPhys 160) are compared with the band scaled by the ER-formula ratio pred(atk)/pred(140)
                   (inference, unverified) and can only give NEEDS-CALIBRATION, never PASS or FAIL, until a
                   close-range c4311 measurement exists. Hits only on other enemies also give NEEDS-CALIBRATION.
  viewmodel (M3)   VM assets ready, VM target fmt=, no VM FAIL / PANIC, VM gpu_ms p95 < 1.0 ms
  vm anim (M3)     VM state ->raise after MODE on, ->reload|reload_empty after RELOAD start, ->drop after MODE off
  camera (M3)      CAMFRAME lines with |det| ~ 1 and a sane aspect; no SHOT skipped for the camera distance
                   after the gate first reported valid=1
  hits (M3b)       a non-kill hit on a reference enemy with dmg < 20% of its band baseline is 'blocked?' (shield
                   block) and excluded from calibration, but only when the same enemy TYPE (cid) also took normal
                   hits (HIT lines carry no per-instance id, so blocks are grouped by cid, not by individual enemy);
                   a lone tiny hit is still judged normally. The run-3 self-test PASS is a calibration check (band
                   79000100's baseline was measured from run 3 itself), not an independent validation. Band 79000100 (atkPhys 160) is measured since M3 run 3 (49/hit).
  first person (M3b) needs (when the DLL logs 'FPCAM cfg'): >= 1 'FP on'; each FP on ends with 'FP off reason=...'
                   (reason=loading right after 'TASK world left'); 'FPCAM er_rewrite=1' (task mode) or 'FPHOOK
                   installed' (hook mode); 'FP hide write' unless hide=none; no FPCAM/FPHOOK WARN/FAIL; no CAMFRAME
                   valid=0 why=dist while on. Logs of a DLL before M3b give N/A (not counted against the verdict).
  reload           RELOAD start ... -> RELOAD done mag=<clip>
  ac130 (M4)       docs/M4_DESIGN.md section 12.1, six rows (N/A for logs without AC130 lines or with [ac130] disabled):
                   lifecycle (PROTECT apply ok=1 before every 'AC130 on', exactly one 'off' per ride, a camera restore
                   line and the HUD restore before it, 'PROTECT restore ... present=0 ok=1' after it, world left ->
                   off within 0.2 s); protection (player_hp_min == player_hp_start, no protect_stuck); guns (per gun:
                   clip never exceeded, in-clip median within 15% of fireTime and never faster, across a reload never
                   faster than reloadTime - 10% (MW2 gsc: from the shot); expectations from weapons_ac130.json);
                   explosions (EXPLODE lines, an 'EXPLODE seen' for every id used, >= 1 HIT src=ac130 with dmg>0,
                   every AC130 KILL after its HIT kill=1 src=ac130 (same cid), kills per gun and HIT totals per tier/cid
                   as info, no SPAWN FAIL); perf (PERF ac130=1 average >= 50 ticks/s, thermal gpu_ms p95 < 1 ms, no
                   'FAIL thermal', AC130CAM persisted=0, skips < 1% of writes). Input (mouse_counts grows, er_yaw
                   steady) is INFO. AC-130 HITs (src=ac130) are excluded from the UMP45 hits / calibration row.
                   effects (after run 9, section 20; N/A for a DLL before it): an 'AC130 effects shake=' line per ride;
                   with the shake on, one 'SHAKE src=fire' per 105/40 SHOT and one 'SHAKE src=impact' per 105/40
                   EXPLODE (none for the 25 mm: MW2 has no 25 mm earthquake), amp > 0 unless mw2_falloff; the 'rows ok'
                   line's obj= (105 core atkObj > 199) and sfx= as info. Asset breaks are not logged (check by eye).
  movement (M5)    docs/M5_DESIGN.md section 13.3, six rows (N/A for logs without 'MOVE cfg'): movement (>= 1 'MOVE on',
                   each released with a reason before the next world left, every gravity write restored ok=1 / stale,
                   no PANIC); movement speeds (steady 'MOVE state' lines with ground=1 pushout=0, keyed by stance and wish:
                   speed_world within 5% of 4.83 / 3.86 / 3.38 / 7.24 / 3.14 / 0.72 m/s and of speed_sim; < 5 samples:
                   NEEDS-DATA); sprint (why=time dur 4.00 +- 0.05 s, no start with meter <= 1 s, no SHOT within 0.3 s of
                   'sprint end why=fire'); jump (apex 0.991 +- 0.03 m); fall damage (dmg_pct from fall_h by the 200/350
                   in rule, hp delta = dmg_pct x max_hp, lethal -> 'suspend reason=death' within 2 s, no death within 3 s
                   of a non-lethal landing); move perf (PERF move=1: ticks_per_s >= 55 or >= 95% of move=0, move_us_p95
                   < 1000, rays_max <= the cfg's rays_max). 'MOVE keyprobe' results are INFO.
  arsenal (M6)     docs/M6_DESIGN.md section 12.2, thirteen rows in analyze_m6.py (N/A for logs without 'ARSENAL data'
                   except 'ac130 start' and 'spawn', which judge any log): arsenal (every gun loaded, VRAM <= 128 MB,
                   PARAMS OK guns= == loaded guns, mode flat_inv), switching (drop/raise durations vs dropTime/raiseTime,
                   no SHOT while switching), shots per gun (never faster than fireTime; full-auto median within a frame),
                   hits per gun (band ids / MW2 damage of the gun's rows, fitted D_eff on c4311), ump45 calibration
                   (close-band c4311 hits 49 +- 1 without openers / weak parts; < 3: NEEDS-DATA), burst, rechamber,
                   shell reload, pellets, launchers (LAUNCH -> PROJ hit -> PROJ explode), lock, ac130 start (run-13 bug 1:
                   'off reason=loading t_s=0.0' in the world FAILs), spawn (run-13 bug 2: any SPAWN FAIL with a cause
                   FAILs, without a cause NEEDS-DATA). In an arsenal session the M2 rows judge the MODE-on gun: its
                   arsenal bands join the legacy ones, SHOT lines of other guns are left out, HIT lines with gun= are
                   judged by 'hits per gun' (the M2 'hits' row is N/A when only those exist), PARAMS OK guns= counts.
  no crashes       no PANIC, no 'ERROR ... disabled'
Also prints what the run answered about the open questions (camera sign/FOV, menu indices, HPDROP).
Exit 0 only when every criterion PASSed. Pure Python, read-only.
"""
import json
import pathlib
import re
import statistics
import sys
import tomllib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # -I drops the script's folder from sys.path
import analyze_m6  # noqa: E402
import analyze_m7  # noqa: E402
import analyze_m8  # noqa: E402
import analyze_r18  # noqa: E402
import analyze_coop  # noqa: E402
import analyze_c3  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
LINE = re.compile(r"^(\d{4}-\d\d-\d\d) (\d\d):(\d\d):(\d\d)\.(\d{3}) ([DIWE]) \[([^\]]*)\] (\S+) ?(.*)$")
FIRE_TOL = 0.017          # one 60 Hz frame: shots land on task ticks
BURST_GAP_S = 0.25
HIT_SLACK_S = 0.1          # log stamps vs the task clock
DMG_TOL = 0.15             # prediction tolerance
DMG_MIN_RATIO = 0.05       # ER's curve floor is 0.1 x atk before cut rates
DMG_MAX_RATIO = 2.5        # 0.9 x atk x cut rate x counter-hit / two bullets in one tick, generously
SCALE_TOL = 0.15           # same enemy: a higher atkPhys band may not do >15% less damage
NEEDS = "NEEDS-CALIBRATION"
NEEDS_DATA = "NEEDS-DATA"   # too few samples to judge (never a PASS for the overall verdict, never a FAIL)
FIRE_MIN_SHOTS = 20

# Reference enemies with vanilla NpcParam values (docs/M2_DESIGN.md section 10.6; verified from the
# vanilla regulation): physical defence and the cut rate for our rows' attack attribute. M3: the measured
# per-hit damage at k=4 and its PASS band (docs/M3_DESIGN.md section 11). Measured in M2 run 2
# (logs/mw2er.log, 2026-10-14 09:35:33): 219->150->101->52->3->0; the kill line reads band=79000105 mw2=35
# d=36.0m, i.e. the last falloff band (atkPhys 140) at about 36 m. There is no close-range measurement yet.
KNOWN_TARGETS = {
    4311: {"name": "Limgrave Godrick Soldier", "def_phys": 101.3, "cut": 1.1, "baseline": 49, "band": (35, 75),
           "atk": 140, "measured_band": 79000105, "measured_d_m": 36.0,
           # M3b: every band with a real measurement. 79000105: M2 run 2 (69, 49, 49, 49 at ~36 m). 79000100: M3
           # run 3 (logs/mw2er.log 11:58:50.8-11:59:44.9, tools/scripts/fixtures/m3_run3_session.log): non-kill
           # hits 64, 49 x 9 at 0.9-8.8 m. The ER-formula ratio predicted x1.25 (61) for atkPhys 160 vs 140; the
           # measurement says x1.0, so the formula stays info only.
           "measured": {79000105: {"baseline": 49, "band": (35, 75), "atk": 140, "d": "~36 m", "src": "M2 run 2"},
                        79000100: {"baseline": 49, "band": (35, 75), "atk": 160, "d": "0.9-8.8 m", "src": "M3 run 3"}}},
}
# M3b: a non-kill hit on a reference enemy below this fraction of its band baseline is classified "blocked?"
# (Godrick Soldiers carry shields: run 3 logged seven dmg=4 hits at atkPhys 160 between normal 49s) and is excluded
# from calibration. Guard: if EVERY non-kill hit on that enemy TYPE (cid; HIT lines have no per-instance id) is tiny,
# they are judged normally (a lone dmg=1 still FAILs). No guard flag is read from the game in M3b (none found in the vendored crate): damage only, inferred.
BLOCK_RATIO = 0.2
CAL_MIN_HITS = 3
CAL_IN_BAND = 0.75
VM_GPU_P95_MS = 1.0


def er_damage(atk, defence):
    """Community ER attack-vs-defence curve (docs/M2_DESIGN.md 10.6; [R], not verified in game)."""
    if atk <= 0 or defence <= 0:
        return float(max(atk, 0))
    r = atk / defence
    if r > 8:
        m = 0.9
    elif r > 2.5:
        m = 0.9 - 0.8 / 121 * (r - 8) ** 2
    elif r > 1:
        m = 0.7 - 0.4 / 3 * (r - 2.5) ** 2
    elif r > 0.125:
        m = 0.1 + 19.2 / 49 * (r - 0.125) ** 2
    else:
        m = 0.1
    return atk * m


def predict(atk, target, correction=1.0):
    return er_damage(atk * correction, target["def_phys"]) * target["cut"]


# ---- inputs ---------------------------------------------------------------------------------------------

def load_context(root=ROOT):
    """Expected values from the project's data files. Missing files leave None (criteria then FAIL)."""
    ctx = {"weapons": None, "rows": None, "window_s": 0.6, "default_weapon": None, "missing": [], "ac130": None}
    try:
        ctx["weapons"] = json.loads((root / "converted" / "weapons.json").read_text(encoding="utf-8"))["weapons"]
    except (OSError, ValueError, KeyError) as e:
        ctx["missing"].append(f"converted/weapons.json ({e})")
    try:
        ctx["rows"] = json.loads((root / "build" / "rows.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        ctx["missing"].append(f"build/rows.json ({e})")
    try:
        cfg = tomllib.loads((root / "mw2er.toml").read_text(encoding="utf-8"))
        ctx["window_s"] = float(cfg.get("hit", {}).get("window_s", 0.6))
        ctx["default_weapon"] = cfg.get("gameplay", {}).get("weapon")
    except (OSError, ValueError) as e:
        ctx["missing"].append(f"mw2er.toml ({e})")
    try:
        ctx["ac130"] = json.loads((root / "converted" / "ac130" / "weapons_ac130.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        ctx["missing"].append(f"converted/ac130/weapons_ac130.json ({e})")
    ctx["guns"] = analyze_m6.load_guns(root)
    return ctx


def weapon_view(ctx, weapon):
    """fireTime, bands (id -> band dict) and the expected PARAMS count for `weapon`, or None + reason."""
    if ctx["weapons"] is None or ctx["rows"] is None:
        return None, "missing " + ", ".join(ctx["missing"])
    w = ctx["weapons"].get(weapon)
    rw = ctx["rows"].get("weapons", {}).get(weapon)
    if w is None or rw is None:
        return None, f"weapon {weapon!r} not in weapons.json/rows.json"
    bands = {int(b["id"]): b for b in rw["bands"]}
    return {
        "name": weapon,
        "fire_time": float(w["mw2"]["fireTime"]),
        "clip": int(w["mw2"]["clipSize"]),
        "bands": bands,
        "params_n": 2 * len(bands) + 1,
        "speed": float(ctx["rows"].get("bullet", {}).get("speed_mps", 999.0)),
    }, None


# ---- log parsing ------------------------------------------------------------------------------------------

def parse(text):
    out = []
    for raw in text.splitlines():
        m = LINE.match(raw)
        if not m:
            continue  # backtrace continuation lines etc.
        d, hh, mm, ss, ms, lvl, thread, tag, msg = m.groups()
        t = int(hh) * 3600 + int(mm) * 60 + int(ss) + int(ms) / 1000.0
        out.append({"day": d, "t": t, "level": lvl, "thread": thread, "tag": tag, "msg": msg, "raw": raw})
    # Make times monotonic across midnight.
    offset, prev = 0.0, None
    for e in out:
        if prev is not None and e["t"] + offset < prev - 3600:
            offset += 86400.0
        e["t"] += offset
        prev = e["t"]
    return out


def sessions(entries):
    s = []
    for e in entries:
        if e["tag"] == "BOOT" and e["msg"].startswith("mw2er "):
            s.append([])
        if s:
            s[-1].append(e)
    return s


# ---- the hits criterion -----------------------------------------------------------------------------------

HIT_RE = re.compile(r"^chr=c(\d{4,}) hp (-?\d+)->(-?\d+) dmg=(-?\d+)(?: band=(-?\d+) mw2=(-?\d+))?")
HIT_M3_RE = re.compile(r"via=(\w+) n=(\d+) kill=([01])")


AC_RIDE_TAIL_S = 3.0       # shells keep landing (and enemies keep dying) this long after 'AC130 off'


def ac130_ride_windows(entries):
    """[(t_on, t_off + tail)] of every AC-130 ride ('AC130 on center=' .. 'AC130 off reason=')."""
    out, t_on = [], None
    for e in entries:
        if e["tag"] != "AC130":
            continue
        if e["msg"].startswith("on center="):
            t_on = e["t"]
        elif e["msg"].startswith("off reason=") and t_on is not None:
            out.append((t_on, e["t"] + AC_RIDE_TAIL_S))
            t_on = None
    if t_on is not None:
        out.append((t_on, float("inf")))
    return out


def is_ac130_hit(e, windows):
    """An AC-130 HIT: 'src=ac130', or an unattributed ER last_hit_by drop ('band=0 ... via=last_hit_by', the run-8
    DLL logged two kills like that) inside a ride window. Such lines never reach the UMP45 bands."""
    if "src=ac130" in e["msg"]:
        return True
    return (bool(re.search(r"\bband=0\b", e["msg"])) and "via=last_hit_by" in e["msg"]
            and any(a <= e["t"] <= b for a, b in windows))


def judge_hits(entries, wv, window_s):
    """Returns (status, detail) for the hits criterion."""
    # M4: AC-130 HITs are judged by the ac130 explosions row, never by the UMP45 bands
    windows = ac130_ride_windows(entries)
    hits = [(e, m) for e in entries if e["tag"] == "HIT" and not is_ac130_hit(e, windows) and (m := HIT_RE.match(e["msg"]))]
    hpdrops = [e for e in entries if e["tag"] == "HPDROP"]
    if not hits:
        n_ac = sum(1 for e in entries if e["tag"] == "HIT" and is_ac130_hit(e, windows))
        return "FAIL", (f"no UMP45 HIT lines ({len(hpdrops)} HPDROP: "
                        f"{'attribution failed' if hpdrops else 'no damage seen'}; {n_ac} AC-130 HIT(s) not counted)")
    if wv is None:
        return "FAIL", f"{len(hits)} HIT line(s) but no expectations (rows.json / weapons.json missing)"
    shot_times = [e["t"] for e in entries if e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"])]
    problems, cal, others, info = [], {}, [], []
    excluded = {"kill": 0, "multi": 0, "blocked": 0}
    blocked, normal = {}, {}
    for e, m in hits:
        cid, hp_a, hp_b, dmg = int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
        band_id = int(m.group(5)) if m.group(5) else None
        mw2 = int(m.group(6)) if m.group(6) else None
        m3 = HIT_M3_RE.search(e["msg"])
        via = m3.group(1) if m3 else None
        n = int(m3.group(2)) if m3 else 1
        kill = (m3.group(3) == "1") if m3 else (hp_b <= 0)
        where = f"c{cid:04d} dmg={dmg}"
        if dmg <= 0:
            problems.append(f"{where}: damage must be > 0")
            continue
        b = wv["bands"].get(band_id)
        if b is None:
            problems.append(f"{where}: band {band_id} is not a {wv['name']} row in rows.json")
            continue
        if mw2 != int(b["mw2_damage"]):
            problems.append(f"{where}: mw2={mw2} but rows.json band {band_id} has {b['mw2_damage']}")
            continue
        if not any(0 <= e["t"] - s_ <= window_s + HIT_SLACK_S for s_ in shot_times):
            problems.append(f"{where}: no SHOT within {window_s} s before it")
            continue
        atk = int(b["atk_phys"])
        target = KNOWN_TARGETS.get(cid)
        if target is not None and not kill and dmg < BLOCK_RATIO * band_baseline(target, band_id, atk) * max(n, 1):
            blocked.setdefault(cid, []).append((dmg, atk, band_id, n, where))
            continue
        if target is not None and not kill:
            normal[cid] = normal.get(cid, 0) + 1
        if not kill and not (DMG_MIN_RATIO * atk <= dmg <= DMG_MAX_RATIO * atk * max(n, 1)):
            problems.append(f"{where}: implausible for atkPhys {atk} (allowed {DMG_MIN_RATIO * atk:.0f}.."
                            f"{DMG_MAX_RATIO * atk * max(n, 1):.0f})")
            continue
        if kill:
            excluded["kill"] += 1
            continue
        if n != 1:
            excluded["multi"] += 1
            continue
        if target is None:
            others.append(f"{where} atk={atk}")
            continue
        cal.setdefault(cid, []).append((dmg, atk, band_id))
    for cid, items in blocked.items():
        if normal.get(cid, 0) > 0:
            excluded["blocked"] += len(items)   # shield blocks next to normal hits on the same enemy TYPE (cid)
            continue
        for dmg, atk, band_id, n, where in items:  # nothing normal to compare with: judge them as ordinary hits
            if not (DMG_MIN_RATIO * atk <= dmg <= DMG_MAX_RATIO * atk * max(n, 1)):
                problems.append(f"{where}: implausible for atkPhys {atk} (allowed {DMG_MIN_RATIO * atk:.0f}.."
                                f"{DMG_MAX_RATIO * atk * max(n, 1):.0f}; tiny like a shield block, but no normal hit "
                                f"on c{cid:04d} to compare with)")
            elif n == 1:
                cal.setdefault(cid, []).append((dmg, atk, band_id))
    if problems:
        return "FAIL", f"{len(problems)} bad HIT line(s): " + "; ".join(problems[:4])
    vias = {}
    for e, m in hits:
        m3 = HIT_M3_RE.search(e["msg"])
        k = m3.group(1) if m3 else "m2-format"
        vias[k] = vias.get(k, 0) + 1
    head = (f"{len(hits)} HIT ({', '.join(f'{k}={v}' for k, v in sorted(vias.items()))}), {len(hpdrops)} HPDROP; "
            f"excluded from calibration: {excluded['kill']} kill, {excluded['multi']} n>1, {excluded['blocked']} blocked? "
            f"(dmg < {BLOCK_RATIO:.0%} of the band baseline next to normal hits on the same cid: shield blocks)")
    if not cal:
        return NEEDS, (head + f"; no single non-kill hit on a reference enemy "
                       f"({'/'.join(t['name'] for t in KNOWN_TARGETS.values())})"
                       + (f"; others: {'; '.join(others[:4])}" if others else ""))
    verdicts = []
    for cid, rows in cal.items():
        t = KNOWN_TARGETS[cid]
        by_band = {}
        for d, atk, band_id in rows:
            by_band.setdefault((band_id, atk), []).append(d)
        for (band_id, atk), dmgs in sorted(by_band.items()):
            dmgs = sorted(dmgs)
            mb = t["measured"].get(band_id)
            measured = mb is not None
            # Measured bands: as measured. Other bands: scaled by the ER-formula ratio (inference).
            scale = 1.0 if measured else predict(atk, t) / predict(t["atk"], t)
            lo, hi = (mb["band"] if measured else (t["band"][0] * scale, t["band"][1] * scale))
            med = statistics.median(dmgs)
            inband = sum(1 for d in dmgs if lo <= d <= hi) / len(dmgs)
            kind = (f"measured band (atkPhys {mb['atk']}, d {mb['d']}, {mb['src']})" if measured else
                    f"extrapolated x{scale:.2f} from band {t['measured_band']}, unverified")
            info.append(f"c{cid:04d} {t['name']} band {band_id} atkPhys {atk}: {len(dmgs)} hit(s) {dmgs[:8]} median "
                        f"{med:g} vs {lo:.0f}..{hi:.0f} [{kind}] ({inband:.0%} in band; ER-formula prediction "
                        f"{predict(atk, t):.0f}, info only)")
            ok = lo <= med <= hi and inband >= CAL_IN_BAND
            if not measured:
                verdicts.append(NEEDS)   # no ground truth outside the measured band: never PASS, never FAIL
            elif not ok:
                verdicts.append("FAIL")
            elif len(dmgs) < CAL_MIN_HITS:
                verdicts.append(NEEDS)
            else:
                verdicts.append("PASS")
    detail = head + "; " + "; ".join(info)
    if "FAIL" in verdicts:
        return "FAIL", detail + " -> per-hit damage outside the measured band: recalibrate damage.k"
    if "PASS" in verdicts:
        return "PASS", detail
    return NEEDS, detail + (f" -> need >= {CAL_MIN_HITS} single non-kill c4311 hits in band "
                            f"{KNOWN_TARGETS[4311]['measured_band']} (or a user-confirmed close-range measurement)")


def band_baseline(target, band_id, atk):
    """Expected per-hit damage of a reference enemy in a band: measured, else ER-formula scaled (inference)."""
    mb = target["measured"].get(band_id)
    if mb is not None:
        return mb["baseline"]
    return target["baseline"] * predict(atk, target) / predict(target["atk"], target)


FP_REASONS = ("panic", "read_fail", "cam_persist", "loading", "death", "cutscene", "torrent", "ladder", "throw", "talk",
              "menu", "cam_gate", "f5", "f6", "config", "ac130", "nuke")  # M8 P2 (a): the M7 nuke reason
# M8 P2 (b): a chunk re-base moves the camera one tick before the player (run 16: cam_chest=31 m with a MOVE resync
# at the same time stamp); first person rides it out for cam_gate_grace_ticks (8 ticks at 60 Hz)
CAM_REBASE_SAME_S = 0.05
CAM_GATE_GRACE_S = 8 / 60.0 + 0.02


def rebase_camframe(entries, e):
    """A CAMFRAME valid=0 why=dist at a chunk re-base (a MOVE resync at the same time) that first person rode out."""
    t = e["t"]
    rebased = any(x["tag"] == "MOVE" and re.match(r"resync why=(warp|rebase) ", x["msg"]) and abs(x["t"] - t) <= CAM_REBASE_SAME_S
                  for x in entries)
    gate_off = any(x["tag"] == "FP" and x["msg"].startswith("off reason=cam_gate") and 0.0 <= x["t"] - t <= CAM_GATE_GRACE_S
                   for x in entries)
    return rebased and not gate_off


def judge_first_person(entries):
    """M3b first person (docs/M3B_DESIGN.md section 7). N/A for logs of a DLL before M3b (no FPCAM cfg line)."""
    cfgs = [e for e in entries if e["tag"] == "FPCAM" and e["msg"].startswith("cfg ")]
    if not cfgs:
        return "N/A", "no 'FPCAM cfg' line (DLL before M3b)"
    cfg = cfgs[-1]["msg"]
    if "enabled=0" in cfg:
        return "N/A", "first_person.enabled = false in this session"
    mm = re.search(r"mode=(\w+)", cfg)
    mode = mm.group(1) if mm else "task"
    # run 8: the cfg line can be written a few ms before the camera hook is installed ("task (hook requested but not
    # installed)"); the DLL decides the mode per tick from cam_hook::INSTALLED, so a later 'FPHOOK installed' means hook
    if "hook requested but not installed" in cfg and any(e["tag"] == "FPHOOK" and e["msg"].startswith("installed")
                                                          for e in entries):
        mode = "hook"
    hm = re.search(r"hide=(\w+)", cfg)
    hide = hm.group(1) if hm else "render_flag"
    problems, on, n_on, n_off, reasons, t_left = [], False, 0, 0, {}, None
    for e in entries:
        tag, msg = e["tag"], e["msg"]
        if t_left is not None and on and e["t"] - t_left > 0.2:
            problems.append(f"world left at {t_left:.3f} with no 'FP off reason=loading' within 0.2 s")
            t_left = None
        if tag == "FP" and msg.startswith("on "):
            if on:
                problems.append("FP on while already on")
            on, n_on, t_left = True, n_on + 1, None
        elif tag == "FP" and msg.startswith("off reason="):
            r = msg.split("=", 1)[1].split(" ", 1)[0]
            if r not in FP_REASONS:
                problems.append(f"unknown handback reason {r!r}")
            if not on:
                problems.append(f"FP off ({r}) while not on")
            if t_left is not None and r != "loading":
                problems.append(f"world left at {t_left:.3f} while FP on, then 'FP off reason={r}'")
            on, t_left, n_off = False, None, n_off + 1
            reasons[r] = reasons.get(r, 0) + 1
        elif tag == "TASK" and msg.startswith("world left") and on:
            t_left = e["t"]
        elif on and tag == "CAMFRAME" and " valid=0 " in msg and "why=dist" in msg and not rebase_camframe(entries, e):
            problems.append("CAMFRAME valid=0 why=dist while FP on (the eye reached the camera gate)")
    if t_left is not None:
        problems.append(f"world left at {t_left:.3f} with no 'FP off reason=loading' after it")
    # CAM_WALL_RECOVERY not validated is the documented safe degrade (run 6: Arxan page protection; the DLL never touches
    # it and first person works without it): INFO, not a problem
    warns = [e["msg"] for e in entries if e["tag"] in ("FPCAM", "FPHOOK") and ("WARN" in e["msg"] or e["msg"].startswith("FAIL"))
             and "CAM_WALL_RECOVERY not validated" not in e["msg"]]
    problems += [w[:90] for w in warns[:2]]
    blocked = sorted({m.group(1) for e in entries if e["tag"] == "FP" and (m := re.match(r"blocked reason=(\w+)", e["msg"]))})
    if n_on == 0:
        return "FAIL", (f"FPCAM cfg present ({cfg[:60]}...) but no 'FP on' (turn MW2 mode on with F5 in the world); "
                        f"blocked reasons seen: {blocked}")
    if mode == "task" and not any(e["tag"] == "FPCAM" and e["msg"].startswith("er_rewrite=1") for e in entries):
        problems.append("no 'FPCAM er_rewrite=1' (the free restore of pers_cam_1 is unproven)")
    if mode == "hook" and not any(e["tag"] == "FPHOOK" and e["msg"].startswith("installed") for e in entries):
        problems.append("camera_mode=hook but no 'FPHOOK installed'")
    if hide != "none" and not any(e["tag"] == "FP" and e["msg"].startswith("hide write ") for e in entries):
        problems.append(f"hide={hide} but no 'FP hide write'")
    if problems:
        return "FAIL", "; ".join(problems[:4])
    probe = [e["msg"] for e in entries if e["tag"] == "FP" and e["msg"].startswith("yaw probe")]
    reassert = [e for e in entries if e["tag"] == "FP" and e["msg"].startswith("hide reasserted")]
    return "PASS", (f"{n_on} FP on / {n_off} off (reasons {dict(sorted(reasons.items()))}), mode={mode}, hide={hide}, "
                    f"{probe[-1] if probe else 'yaw probe not resolved'}, hide reasserts seen={bool(reassert)} "
                    "(confirm visually: body gone, eye-height view, mouse look, other characters visible)")


def judge_viewmodel(entries):
    vm = [e for e in entries if e["tag"] == "VM"]
    ready = [e for e in vm if e["msg"].startswith("assets ready")]
    # M7 A4: an arsenal session loads its viewmodels as 'ARSENAL loaded n=.. ok=..' (no 'VM assets ready')
    ready += [e for e in entries if e["tag"] == "ARSENAL" and re.match(r"loaded n=(\d+) ok=\1\b", e["msg"])]
    target = [e for e in vm if e["msg"].startswith("target fmt=")]
    fails = [e for e in vm if e["msg"].startswith("FAIL")]
    gpu = [float(m.group(1)) for e in vm if (m := re.search(r"gpu_ms p50=[\d.]+ p95=([\d.]+)", e["msg"]))]
    drawn = [e for e in vm if e["msg"].startswith("first frame drawn")]
    if fails:
        return "FAIL", fails[0]["msg"][:160]
    if not vm:
        return "FAIL", "no VM lines (M2 DLL, or viewmodel.enabled = false)"
    if not target:
        return "FAIL", "no 'VM target fmt=' line: the pre-ImGui callback never ran"
    if not ready:
        return "FAIL", f"{target[0]['msg']}; no 'VM assets ready'"
    if not drawn:
        return "FAIL", f"{target[0]['msg']}; assets ready but no frame drawn (turn MW2 mode on in the world)"
    if not gpu:
        return "FAIL", f"{target[0]['msg']}; drawn, but no 'VM gpu_ms' line yet (play >= viewmodel.log_interval_s)"
    worst = max(gpu)
    if worst >= VM_GPU_P95_MS:
        return "FAIL", f"VM gpu_ms p95 up to {worst:.3f} ms >= {VM_GPU_P95_MS} ms"
    return "PASS", (f"{target[0]['msg']}; {ready[0]['msg'][:60]}; gpu p95 max {worst:.3f} ms over {len(gpu)} line(s) "
                    "(confirm the gun + arms on screen)")


def judge_vm_anim(entries):
    states = [(e["t"], m.group(1), m.group(2)) for e in entries if e["tag"] == "VM"
              and (m := re.match(r"state (\w+)->(\w+)", e["msg"]))]
    if not states:
        return "FAIL", "no 'VM state' lines"

    def after(t0, targets, within=1.0):
        return any(t0 - 0.05 <= t <= t0 + within and to in targets for t, _, to in states)

    misses = []
    # M7 A4: a start in a switch's own tick is the A3 bug (a reload restarted on the outgoing gun during its drop);
    # a start shortly after a switch (an empty gun reloading at its raise) may wait for the raise to finish
    switches = [e["t"] for e in entries if e["tag"] == "SWITCH" and e["msg"].startswith("start ")]
    for e in entries:
        if e["tag"] == "RELOAD" and e["msg"].startswith("start "):
            if any(abs(e["t"] - s) < 0.02 for s in switches):
                misses.append("RELOAD start during a switch drop (A3)")
                continue
            if any(0 <= e["t"] - s <= 2.5 for s in switches):
                if not after(e["t"], {"reload", "reload_empty"}, within=2.5):
                    misses.append("RELOAD start without ->reload")
                continue
        if e["tag"] == "MODE" and e["msg"].startswith("on weapon=") and not after(e["t"], {"raise"}):
            misses.append("MODE on without ->raise")
        if e["tag"] == "MODE" and e["msg"] == "off" and not after(e["t"], {"drop"}):
            misses.append("MODE off without ->drop")
        if e["tag"] == "RELOAD" and e["msg"].startswith("start ") and not after(e["t"], {"reload", "reload_empty"}):
            misses.append("RELOAD start without ->reload")
    kinds = sorted({to for _, _, to in states})
    if misses:
        return "FAIL", f"{len(misses)} missing transition(s): {misses[:3]}; seen {kinds}"
    return "PASS", f"{len(states)} VM state lines, phases {kinds}"


def judge_camera(entries):
    frames = [e for e in entries if e["tag"] == "CAMFRAME"]
    if not frames:
        return "FAIL", "no CAMFRAME lines"
    bad = []
    first_valid = None
    for e in frames:
        if " valid=1 " in e["msg"]:
            first_valid = first_valid if first_valid is not None else e["t"]
            det = re.search(r"det=([-+\d.]+)", e["msg"])
            asp = re.search(r"aspect=([\d.]+)", e["msg"])
            if det and not 0.99 <= abs(float(det.group(1))) <= 1.01:
                bad.append(f"det {det.group(1)}")
            if asp and not 0.5 <= float(asp.group(1)) <= 4.0:
                bad.append(f"aspect {asp.group(1)}")
    if first_valid is None:
        return "FAIL", f"{len(frames)} CAMFRAME lines, never valid=1"
    skipped = [e for e in entries if e["tag"] == "SHOT" and "skipped" in e["msg"] and "cam" in e["msg"]
               and e["t"] > first_valid + 0.1 and "settling" not in e["msg"]]
    if bad or skipped:
        return "FAIL", f"{bad[:3]} {len(skipped)} SHOT skipped for the camera after it was valid: {[e['msg'][:80] for e in skipped[:2]]}"
    dets = sorted({m.group(1)[0] for e in frames if " valid=1 " in e["msg"] and (m := re.search(r"det=([-+])", e["msg"]))})
    return "PASS", f"{len(frames)} CAMFRAME lines, valid from {first_valid:.1f} s, det sign(s) {dets}"


# ---- M4 AC-130 rows (docs/M4_DESIGN.md section 12.1) --------------------------------------------------------

AC_GPU_P95_MS = 1.0
AC_TICKS_MIN = 50.0
AC_IN_CLIP_TOL = 0.15
AC_RELOAD_TOL = 0.10
AC_RESTORE_WITHIN_S = 0.2
AC_SHOT_RE = re.compile(r"^SHOT gun=(\d+) n=(\d+) clip=(\d+)(?:/(\d+))? ")
AC_OFF_RE = re.compile(r"^off reason=(\w+) t_s=([\d.]+)")
AC_ROWS = ("ac130 lifecycle", "ac130 protection", "ac130 guns", "ac130 explosions", "ac130 perf", "ac130 effects")
AC_SHAKE_RE = re.compile(r"^SHAKE src=(fire|impact) gun=(\d+) mw2_scale=([\d.]+) dur_s=([\d.]+) radius_m=([\d.]+) "
                         r"dist_m=([\d.]+) gain=([\d.]+) amp=([\d.]+) on=([01])")
AC_EFFECTS_RE = re.compile(r"^effects shake=([01]) scale=([\d.]+) .*mw2_falloff=([01])")


def ac_present(entries):
    """None when the AC-130 rows apply, else the N/A reason (a DLL before M4, or [ac130] disabled)."""
    ac = [e for e in entries if e["tag"] in ("AC130", "AC130CAM")]
    if not ac:
        return "no AC130 lines (DLL before M4)"
    if any(e["msg"].startswith("disabled by config") for e in ac):
        return "[ac130] enabled = false in this session"
    return None


def judge_ac130_lifecycle(entries):
    na = ac_present(entries)
    if na:
        return "N/A", na
    problems, n_on, n_off, reasons, deferred = [], 0, 0, {}, 0
    applied_ok, on, t_left = False, False, None
    cam_restored, hud_written, hud_restored, wait_protect = False, False, False, None
    for e in entries:
        tag, msg = e["tag"], e["msg"]
        if t_left is not None and on and e["t"] - t_left > AC_RESTORE_WITHIN_S:
            problems.append(f"world left at {t_left:.3f} with no 'AC130 off' within {AC_RESTORE_WITHIN_S} s")
            t_left = None
        if tag == "TASK" and msg.startswith("world left") and on:
            t_left = e["t"]
        if tag != "AC130":
            continue
        if msg.startswith("PROTECT apply ") and " ok=1" in msg:
            applied_ok = True
        elif msg.startswith("on center="):
            if on:
                problems.append("AC130 on while a ride is already on")
            if not applied_ok:
                problems.append("AC130 on without a preceding 'PROTECT apply ... ok=1'")
            if wait_protect is not None:
                problems.append(f"ride ended ({wait_protect}) without 'PROTECT restore ... present=0 ok=1' before the next on")
            on, n_on, applied_ok, t_left = True, n_on + 1, False, None
            cam_restored, hud_written, hud_restored, wait_protect = False, False, False, None
        elif msg.startswith("write field=CSFeManImp.hud_state") and on:
            hud_written = True
        elif msg.startswith("restore field=CSCamera.pers_cam_1") and on:
            cam_restored = True
        elif msg.startswith("restore field=CSFeManImp.hud_state") and on:
            hud_restored = True
        elif (m := AC_OFF_RE.match(msg)):
            r = m.group(1)
            if not on:
                problems.append(f"AC130 off ({r}) while no ride was on")
            if not cam_restored:
                problems.append(f"off reason={r} without a camera restore line (ok or skipped)")
            if hud_written and not hud_restored:
                problems.append(f"off reason={r}: the ER HUD was hidden but no hud_state restore line")
            if t_left is not None and r not in ("world_left", "loading"):
                problems.append(f"world left at {t_left:.3f} during a ride, then 'off reason={r}'")
            reasons[r] = reasons.get(r, 0) + 1
            on, n_off, t_left, wait_protect = False, n_off + 1, None, r
        elif msg.startswith("PROTECT restore deferred"):
            deferred += 1
            wait_protect = None
        elif msg.startswith("PROTECT restore id=") and ("present=0 ok=1" in msg or "reason=panic" in msg):
            wait_protect = None
    if on:
        problems.append("the session ended with a ride still on (no 'AC130 off')")
    if wait_protect is not None:
        problems.append(f"the last ride ({wait_protect}) has no 'PROTECT restore ... present=0 ok=1'")
    if n_on == 0:
        return "FAIL", "no 'AC130 on' (call in the AC-130 with F7 at least once)"
    if problems:
        return "FAIL", f"{len(problems)} problem(s): " + "; ".join(problems[:3])
    return "PASS", (f"{n_on} ride(s), {n_off} off (reasons {dict(sorted(reasons.items()))}), camera/HUD restored, protection "
                    f"removed{f' ({deferred} deferred to the next session cleanup)' if deferred else ''}")


def judge_ac130_protection(entries):
    na = ac_present(entries)
    if na:
        return "N/A", na
    offs = [e for e in entries if e["tag"] == "AC130" and AC_OFF_RE.match(e["msg"])]
    stuck = [e for e in entries if e["tag"] == "AC130" and "protect_stuck" in e["msg"]]
    if not offs:
        return "FAIL", "no 'AC130 off' line with player_hp_start / player_hp_min"
    bad, seen = [], []
    for e in offs:
        a = re.search(r"player_hp_start=(-?\d+) player_hp_min=(-?\d+)", e["msg"])
        if not a:
            if "(task panicked)" not in e["msg"]:
                bad.append(f"off line without player_hp_start/min: {e['msg'][:60]}")
            continue
        s0, mn = int(a.group(1)), int(a.group(2))
        seen.append(f"{s0}->{mn}")
        if mn < s0:
            bad.append(f"player_hp_min {mn} < player_hp_start {s0} (the protection let damage through)")
    if stuck:
        bad.append(f"protect_stuck: {stuck[0]['msg'][:80]}")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", f"player HP start->min per ride {seen}; no protect_stuck"


def ac_expect(ctx):
    w = ctx.get("ac130")
    if not w:
        return None
    return {g["id"]: {"fire": float(g["fire_time_s"]), "reload": float(g["reload_s"]), "clip": int(g["clip_size"])}
            for g in w.get("guns", [])}


def judge_ac130_guns(entries, ctx):
    na = ac_present(entries)
    if na:
        return "N/A", na
    exp = ac_expect(ctx)
    if exp is None:
        return "FAIL", "no expectations (converted/ac130/weapons_ac130.json missing)"
    shots = []  # (t, gid, n, clip, ride index); SWITCH lines break the interval chain
    ride = -1
    for e in entries:
        if e["tag"] != "AC130":
            continue
        if e["msg"].startswith("on center="):
            ride += 1
        elif (m := AC_SHOT_RE.match(e["msg"])):
            shots.append((e["t"], m.group(1), int(m.group(2)), int(m.group(3)), ride))
        elif e["msg"].startswith("SWITCH "):
            shots.append((e["t"], "switch", 0, 0, ride))
    real = [s for s in shots if s[1] != "switch"]
    if not real:
        return "FAIL", "no 'AC130 SHOT gun=' lines (fire each gun a few times)"
    problems, info, per = [], [], {}
    for a, b in zip(shots, shots[1:]):
        if a[1] == "switch" or b[1] == "switch" or a[1] != b[1] or a[4] != b[4]:
            continue
        per.setdefault(a[1], {"clip": [], "reload": []})["reload" if a[3] == 0 else "clip"].append(b[0] - a[0])
    for gid in sorted({s[1] for s in real}, key=lambda g: -int(g)):
        x = exp.get(gid)
        n = sum(1 for s in real if s[1] == gid)
        if x is None:
            problems.append(f"gun {gid} is not in weapons_ac130.json")
            continue
        over = [s for s in real if s[1] == gid and s[3] >= x["clip"]]
        if over:
            problems.append(f"gun {gid}: clip {over[0][3]} after a shot exceeds clipSize {x['clip']}")
        d = per.get(gid, {"clip": [], "reload": []})
        held = [v for v in d["clip"] if v < 2.0 * x["fire"]]
        fast = [v for v in d["clip"] if v < (1 - AC_IN_CLIP_TOL) * x["fire"]]
        if fast:
            problems.append(f"gun {gid}: in-clip interval {min(fast) * 1000:.0f} ms < fireTime {x['fire'] * 1000:.0f} ms - 15%")
        # MW2 _ac130.gsc weaponReload(): reloadTime from the shot (review M4 minor 10)
        want_r = x["reload"]
        rfast = [v for v in d["reload"] if v < (1 - AC_RELOAD_TOL) * want_r]
        if rfast:
            problems.append(f"gun {gid}: interval across a reload {min(rfast):.2f} s < reloadTime "
                            f"{want_r:.2f} s - 10%")
        part = f"gun {gid}: {n} shot(s)"
        if n >= 3 and held:
            med = statistics.median(held)
            part += f", in-clip median {med * 1000:.0f} ms (fireTime {x['fire'] * 1000:.0f})"
            if abs(med - x["fire"]) > AC_IN_CLIP_TOL * x["fire"]:
                problems.append(f"gun {gid}: in-clip median {med * 1000:.0f} ms not within 15% of fireTime "
                                f"{x['fire'] * 1000:.0f} ms")
        if d["reload"]:
            mn = min(d["reload"])
            part += f", across reload min {mn:.2f} s (reloadTime {want_r:.2f})"
            if mn > (1 + AC_RELOAD_TOL) * want_r:
                part += " [fire not held through the reload: INFO]"
        info.append(part)
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", "; ".join(info)


def judge_ac130_explosions(entries):
    na = ac_present(entries)
    if na:
        return "N/A", na
    ac = [e for e in entries if e["tag"] == "AC130"]
    ex = [e for e in ac if e["msg"].startswith("EXPLODE gun=") and " ids=" in e["msg"]]
    impacts = [e for e in ac if AC_SHOT_RE.match(e["msg"]) and "impact=none" not in e["msg"]]
    seen = {m.group(1) for e in ac if (m := re.match(r"EXPLODE seen id=(\d+)", e["msg"]))}
    fails = [e for e in ac if "SPAWN FAIL" in e["msg"]]
    if not ex:
        return "FAIL", f"no 'AC130 EXPLODE gun=... ids=' line ({len(impacts)} shot(s) with an impact)"
    per, used = {}, set()
    for e in ex:
        m = re.match(r"EXPLODE gun=(\d+) ids=(\S*)", e["msg"])
        per[m.group(1)] = per.get(m.group(1), 0) + 1
        used |= {i for i in m.group(2).split("+") if i}
    missing = sorted(used - seen)
    hits = [e for e in entries if e["tag"] == "HIT" and "src=ac130" in e["msg"]]
    windows = ac130_ride_windows(entries)
    legacy = [e for e in entries if e["tag"] == "HIT" and "src=ac130" not in e["msg"] and is_ac130_hit(e, windows)]
    tiers, cids = {}, {}
    for e in hits:
        t = re.search(r"tier=(\w+)", e["msg"])
        d = re.search(r"dmg=(-?\d+)", e["msg"])
        c = re.match(r"chr=c(\d+)", e["msg"])
        dmg = int(d.group(1)) if d else 0
        k = t.group(1) if t else "?"
        tiers[k] = tiers.get(k, 0) + dmg
        if c:
            cids.setdefault(f"c{c.group(1)}", []).append(dmg)
    # review M4 major 7: the explosions must really hurt ER enemies, and every AC130 KILL must follow its HIT
    hurt = [e for e in hits if (d := re.search(r"dmg=(-?\d+)", e["msg"])) and int(d.group(1)) > 0]
    kill_gun, unpaired = {}, []
    pending = {}  # cid -> HIT kill=1 src=ac130 lines not yet claimed by a KILL
    for e in entries:
        if e["tag"] == "HIT" and "src=ac130" in e["msg"] and re.search(r"\bkill=1\b", e["msg"]):
            c = re.match(r"chr=(c\d+)", e["msg"])
            if c:
                pending[c.group(1)] = pending.get(c.group(1), 0) + 1
        elif e["tag"] == "AC130" and (k := AC_KILL_RE.match(e["msg"])):
            kill_gun[k.group(1)] = kill_gun.get(k.group(1), 0) + 1
            if pending.get(k.group(2), 0) > 0:
                pending[k.group(2)] -= 1
            else:
                unpaired.append(k.group(2))
    detail = (f"EXPLODE per gun {dict(sorted(per.items()))}, ids used {sorted(used)}, seen {sorted(seen & used)}; "
              f"{len(hits)} HIT src=ac130, {len(hurt)} with dmg>0 (dmg per tier {tiers}; per cid "
              f"{dict(list(cids.items())[:4])}); AC130 KILL per gun {dict(sorted(kill_gun.items()))}"
              + (f"; {len(legacy)} last_hit_by drop(s) during rides logged without src=ac130 (DLL before run 9)"
                 if legacy else "")
              + (f"; {len(fails)} SPAWN FAIL" if fails else ""))
    if missing:
        return "FAIL", f"no 'EXPLODE seen' for id(s) {missing}: the engine bullet never appeared; " + detail
    if fails:
        return "FAIL", detail
    if not hurt:
        return "FAIL", ("0 'HIT ... src=ac130' with dmg>0: no explosion damaged an ER enemy (land a 105/40 round on an "
                        "enemy); " + detail)
    if unpaired:
        return "FAIL", f"AC130 KILL without a preceding 'HIT ... kill=1 src=ac130' for {sorted(set(unpaired))}; " + detail
    return "PASS", detail


def judge_ac130_effects(entries):
    """After run 9 (docs/M4_DESIGN.md section 20): the MW2 earthquakes reach the gunship camera, the rows carry the
    object damage / impact SFX. N/A for a DLL before the effects update."""
    na = ac_present(entries)
    if na:
        return "N/A", na
    ac = [e for e in entries if e["tag"] == "AC130"]
    eff = [m for e in ac if (m := AC_EFFECTS_RE.match(e["msg"]))]
    shakes = [m for e in ac if (m := AC_SHAKE_RE.match(e["msg"]))]
    if not eff and not shakes:
        return "N/A", "no 'AC130 effects' / 'AC130 SHAKE' lines (a DLL before the run-9 effects update)"
    if not eff:
        return "FAIL", "SHAKE lines without an 'AC130 effects shake=' line at the ride start"
    on, falloff = eff[-1].group(1) == "1", eff[-1].group(3) == "1"
    shots, explodes = {}, {}
    for e in ac:
        if (m := AC_SHOT_RE.match(e["msg"])):
            shots[m.group(1)] = shots.get(m.group(1), 0) + 1
        elif (m := re.match(r"EXPLODE gun=(\d+) ids=", e["msg"])):
            explodes[m.group(1)] = explodes.get(m.group(1), 0) + 1
    fire, impact, amp_max = {}, {}, {}
    for m in shakes:
        d = fire if m.group(1) == "fire" else impact
        d[m.group(2)] = d.get(m.group(2), 0) + 1
        k = f"{m.group(2)}/{m.group(1)}"
        amp_max[k] = max(amp_max.get(k, 0.0), float(m.group(8)))
    problems = []
    for gid in ("105", "40"):
        if fire.get(gid, 0) < shots.get(gid, 0):
            problems.append(f"gun {gid}: {fire.get(gid, 0)} 'SHAKE src=fire' for {shots.get(gid, 0)} SHOT line(s)")
        if impact.get(gid, 0) != explodes.get(gid, 0):
            problems.append(f"gun {gid}: {impact.get(gid, 0)} 'SHAKE src=impact' for {explodes.get(gid, 0)} EXPLODE line(s)")
    if fire.get("25") or impact.get("25"):
        problems.append("SHAKE for the 25 mm (MW2's gsc has no 25 mm earthquake)")
    if on:
        for k, a in amp_max.items():
            if a <= 0 and not (falloff and k.endswith("/impact")):
                problems.append(f"{k} quake amp 0 with the shake on")
    elif any(m.group(9) == "1" for m in shakes):
        problems.append("SHAKE on=1 while the ride's effects line says shake=0")
    rows = [e["msg"] for e in ac if e["msg"].startswith("rows ok ") and " obj=" in e["msg"]]
    obj_info = ""
    if rows:
        om = re.search(r" obj=(\S+)", rows[-1])
        objs = dict(x.split(":", 1) for x in om.group(1).split(",") if ":" in x) if om else {}
        core = re.match(r"(\d+)", objs.get("201", "0"))
        if not core or int(core.group(1)) <= 199:
            problems.append(f"rows ok obj=: the 105 core atkObj {objs.get('201')} does not beat asset defence 199")
        sm = re.search(r" sfx=(\S+)", rows[-1])
        obj_info = f"; rows obj={om.group(1) if om else '?'} sfx={sm.group(1) if sm else '?'}"
    detail = (f"shake={'on' if on else 'off'}{' (mw2_falloff)' if falloff else ''}; SHAKE fire per gun "
              f"{dict(sorted(fire.items()))} for SHOT {dict(sorted(shots.items()))}, impact {dict(sorted(impact.items()))} "
              f"for EXPLODE {dict(sorted(explodes.items()))}, amp max {dict(sorted(amp_max.items()))}{obj_info}; asset "
              "breaks and the SFX look are not logged (check by eye)")
    if problems:
        return "FAIL", "; ".join(problems[:3]) + " | " + detail
    return "PASS", detail


def judge_ac130_perf(entries):
    na = ac_present(entries)
    if na:
        return "N/A", na
    perf = [e for e in entries if e["tag"] == "PERF" and re.search(r"\bac130=1\b", e["msg"])]
    tps = [float(m.group(1)) for e in perf if (m := re.search(r"ticks_per_s=([\d.]+)", e["msg"]))]
    gpu = [float(m.group(1)) for e in entries if e["tag"] == "AC130"
           and (m := re.search(r"^gpu_ms p50=[\d.]+ p95=([\d.]+)", e["msg"]))]
    th_fail = [e for e in entries if e["tag"] == "AC130" and e["msg"].startswith("FAIL thermal")]
    cams = [e for e in entries if e["tag"] == "AC130CAM" and e["msg"].startswith("t=")]
    problems = []
    if not tps:
        problems.append("no PERF line with ac130=1 (ride for >= one PERF interval)")
    elif statistics.mean(tps) < AC_TICKS_MIN:
        problems.append(f"PERF ac130=1 average {statistics.mean(tps):.1f} ticks/s < {AC_TICKS_MIN:.0f}")
    if th_fail:
        problems.append(th_fail[0]["msg"][:100])
    elif not gpu:
        problems.append("no 'AC130 gpu_ms' line (thermal off, or the ride was shorter than 10 s)")
    elif max(gpu) >= AC_GPU_P95_MS:
        problems.append(f"thermal gpu_ms p95 up to {max(gpu):.3f} ms >= {AC_GPU_P95_MS} ms")
    persisted = [e for e in cams if not re.search(r"\bpersisted=0\b", e["msg"])]
    if persisted:
        problems.append(f"AC130CAM persisted != 0: {persisted[0]['msg'][:80]}")
    sk = [(int(m.group(1)), int(m.group(2))) for e in cams if (m := re.search(r"writes=(\d+) skips=(\d+)", e["msg"]))]
    skip_info = ""
    if sk:
        w_, s_ = sk[-1]
        skip_info = f", writes={w_} skips={s_}"
        if s_ and s_ >= 0.01 * max(w_, 1):
            problems.append(f"AC130CAM skips={s_} >= 1% of writes={w_}")
    # review M4 major 1: frames in which ER left the fov (CSCam+0x50) holding our write (INFO: which case applies;
    # the per-float restore handles both)
    fp = [int(m.group(1)) for e in cams if (m := re.search(r"\bfov_persisted=(\d+)", e["msg"]))]
    if fp:
        skip_info += f", fov_persisted={fp[-1]}" + (" (ER does not rebuild the fov: per-float restore needed)" if fp[-1] else "")
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", (f"{len(tps)} PERF ac130=1 window(s) avg {statistics.mean(tps):.1f} ticks/s, gpu p95 max "
                    f"{max(gpu):.3f} ms{skip_info}")


def ac130_input_info(entries):
    """INFO only: mouse_counts grows while ER's own camera yaw stays within 1 deg (ER does not also read the mouse)."""
    rows = [(int(m.group(1)), float(m.group(2))) for e in entries if e["tag"] == "AC130CAM"
            and (m := re.search(r"mouse_counts=(\d+) er_yaw=([-\d.]+)", e["msg"]))]
    if len(rows) < 2:
        return None
    grow = rows[-1][0] > rows[0][0]
    span = max(r[1] for r in rows) - min(r[1] for r in rows)
    ok = grow and span <= 1.0
    return (f"ac130 input {'OK' if ok else 'WARN'}: mouse_counts {rows[0][0]}->{rows[-1][0]}, er_yaw span {span:.1f} deg"
            + ("" if ok else " (ER may also read the mouse another way; the third-person camera turned)"))


# The synthetic M4 ride (tools/scripts/fixtures/m4_ac130_session.log is this generator's output with the defaults;
# the self-test checks they are identical, then mutates the generator).
def ac130_ride(gap_40=0.3, hp_min=500, protect_restore=True, cam_restore=True, gpu_p95=0.045, tps=59.8):
    """(tag, msg, dt, thread) lines of one good AC-130 ride, written after MODE on by synthetic(ac=...)."""
    L = []

    def a(msg, dt=0.0, tag="AC130", thread="task"):
        L.append((tag, msg, dt, thread))

    size = {"105": 1, "40": 4, "25": 20}

    quake = {"105": ("0.20", "1.00", "25.4", "0.40", "1.00", "88.9"), "40": ("0.10", "0.50", "25.4", "0.20", "0.50", "50.8")}

    def shake(src, gid, dt=0.0):
        # after run 9: 'SHAKE src=fire gun=105 mw2_scale=0.20 dur_s=1.00 radius_m=25.4 dist_m=0.0 gain=1.00 amp=0.200 on=1'
        sc, du, ra = quake[gid][:3] if src == "fire" else quake[gid][3:]
        dist = "0.0" if src == "fire" else "157.8"
        a(f"SHAKE src={src} gun={gid} mw2_scale={sc} dur_s={du} radius_m={ra} dist_m={dist} gain=1.00 amp={float(sc):.3f} on=1",
          dt, thread="present" if src == "fire" else "task")

    def shot(gid, n, clip, dt, flight):
        # the DLL's shape (run 8): 'SHOT gun=105 n=1 clip=0/1 o=(..) dir=(..) impact=(..) dist_m=167.4 via=map flight_s=2.20'
        if gid in quake:
            shake("fire", gid, dt)
            dt = 0.0
        a(f"SHOT gun={gid} n={n} clip={clip}/{size[gid]} o=(-107.16,111.55,2.32) dir=(0.718,-0.670,-0.189) "
          f"impact=(13.05,-0.50,-29.32) dist_m=167.4 via=map flight_s={flight:.2f}", dt, thread="present")

    a("rows ok bullet=7 atk=7 speffect=1 protect_id=79000950 obj=201:200b,202:100b,203:200b,301:100b,302:60,303:100b,401:40 "
      "sfx=201:524028,202:530123,203:524028,301:300602,302:530133,303:300602,401:530133", 0.5)
    a("ready", 0.5)
    # first person was on: the ride hands it back with reason ac130 (run 8 shape) and resumes it after the ride
    L.append(("FP", GOOD_FP[1][1], 0.5, "task"))
    a("PROTECT apply id=79000950 chr=0x7ff000010000 present_before=0 present=1 ok=1 call=1", 1.0)
    L.append(("FP", "off reason=ac130  ticks_on=200 cam_writes=200 (re-arms when clear)", 0.016, "task"))
    a("on center=(10.0,2.0,3.0) theta0_deg=12.0 gun=105 fp_was_on=1 camtype=Some(0) er_fov=48.0deg gun_fov=29.86deg "
      "R=116.0 H=107.0 slant=157.8 listener=render_cam(inferred) hp=500", 0.1)
    a("effects shake=1 scale=1.00 rot_deg=2.00 roll_deg=0.60 pos_m=1.50 freq_hz=11.0 mw2_falloff=0 flash_105=14.0m/0.50s "
      "gain=1.60 bloom_105=32.0m")
    a("write field=CSCamera.pers_cam_1+0x10..0x54 addr=0x7ff000020010 old_pos=(1.00,4.00,0.00) new_pos=(100.0,110.0,3.0) "
      "old_fov=0.8378 new_fov=0.5212 (camera-step hook mode 2, one 68-byte write per frame)", 0.016, tag="AC130CAM")
    a("write field=CSFeManImp.hud_state(+0x78) addr=0x7ff000030078 old=3 new=0 ok=1")
    a("thermal ready ms=40 overlays=4 bytes=1048576 fmt=R8G8B8A8_UNORM 1920x1080 ts_freq=10000000 (shaders 0123456789ab..)",
      thread="thload")
    a("thermal first frame 1920x1080 fmt=R8G8B8A8_UNORM gun=0", 0.016, thread="present")
    # 105: one shot, reload, one more
    shot("105", 1, 0, 0.5, 1.97)
    a("RELOAD gun=105 s=5.00")
    a("EXPLODE gun=105 ids=79000201+79000202 pos=(10.0,2.0,3.0) dist_m=150.0 late_ms=8", 1.97)
    shake("impact", "105")
    a("EXPLODE seen id=79000201 pos=(10.0,2.0,3.0) alive=0.016s", 0.016)
    a("EXPLODE seen id=79000202 pos=(10.0,2.0,3.0) alive=0.016s")
    L.append(("HIT", "chr=c4311 hp 219->0 dmg=219 band=79000201 mw2=450 d=3.0m via=ac130 n=1 kill=1 src=ac130 gun=105 tier=core",
              0.05, "task"))
    a("KILL gun=105 cid=c4311 d_m=3.0 tier=core")
    a("RELOAD done gun=105 clip=1", 5.0 - 1.97 - 0.066)
    shot("105", 2, 0, 0.0, 1.97)
    a("RELOAD gun=105 s=5.00")
    # 40: a clip of 4, reload, one more
    a("SWITCH gun=40 via=key fov=12.69deg", 0.5)
    t40 = [0.3, gap_40, gap_40, gap_40]
    for k in range(4):
        shot("40", k + 1, 3 - k, t40[k], 1.18)
    a("RELOAD gun=40 s=3.00")
    a("EXPLODE gun=40 ids=79000301+79000302 pos=(10.0,2.0,3.0) dist_m=150.0 late_ms=5", 1.18 - 0.0)
    shake("impact", "40")
    a("EXPLODE seen id=79000301 pos=(10.0,2.0,3.0) alive=0.016s", 0.016)
    a("EXPLODE seen id=79000302 pos=(10.0,2.0,3.0) alive=0.016s")
    # run 8: a kill ER attributed through last_hit_by, logged by the old DLL without src=ac130 (band 0)
    L.append(("HIT", "chr=c4070 hp 88->0 dmg=88 band=0 mw2=0 d=0.0m via=last_hit_by n=0 kill=1", 0.1, "present"))
    L.append(("KILL", "chr=c4070", 0.0, "present"))
    a("RELOAD done gun=40 clip=4", 3.0 - 1.18 - 0.016 - 0.1)
    shot("40", 5, 3, 0.0, 1.18)
    # 25: a burst of 6
    a("SWITCH gun=25 via=wheel fov=3.16deg", 0.5)
    for k in range(6):
        shot("25", k + 1, 19 - k, 0.3 if k == 0 else 0.07, 0.59)
    a("EXPLODE gun=25 ids=79000401 pos=(10.0,2.0,3.0) dist_m=150.0 late_ms=3", 0.59)
    a("EXPLODE seen id=79000401 pos=(10.0,2.0,3.0) alive=0.016s", 0.016)
    a("t=10.0 slant_m=157.8 cam_to_player=157.8 er_cam_to_player=3.5 writes=600 skips=0 persisted=0 fov_persisted=0 fov_deg=3.16 gun=25 "
      "aim_yaw=3.0 aim_pitch=-2.0 mouse_counts=1200 er_yaw=45.0 last_reject=none", 1.0, tag="AC130CAM")
    a(f"gpu_ms p50=0.0400 p95={gpu_p95:.4f} max=0.0600 n=600", thread="present")
    L.append(("PERF", f"ticks_per_s={tps:.1f} task_us p50=300 p95=600 max=900 fp_us avg=40 max=90 writes=600 write_us_max=8.0 "
              "slow_writes=0 page_checks=2 page_check_ms_max=0.05 page_check_ms_total=0.1 cached_pages=2 ac130=1", 0.0, "task"))
    a("t=20.0 slant_m=157.8 cam_to_player=157.8 er_cam_to_player=3.5 writes=1200 skips=0 persisted=0 fov_persisted=0 fov_deg=3.16 gun=25 "
      "aim_yaw=8.0 aim_pitch=-4.0 mouse_counts=2400 er_yaw=45.2 last_reject=none", 10.0, tag="AC130CAM")
    if cam_restore:
        a("restore field=CSCamera.pers_cam_1+0x10..0x54 addr=0x7ff000020010 old=<ours> new=<er> floats=17 fov_restored=1 fov_persisted_frames=0 ok=1", 18.0)
    a("restore field=CSFeManImp.hud_state old=0 new=3 ok=1", 0.0 if cam_restore else 18.0)
    a(f"off reason=timeout t_s=40.0 shots=105:2/40:5/25:6 explosions=5 hits=1 kills=1 shells_dropped=0 player_hp_start=500 "
      f"player_hp_min={hp_min} moved_m=0.2 fp_resume=1 cooldown_s=120")
    if protect_restore:
        a("PROTECT restore id=79000950 chr=0x7ff000010000 present=0 ok=1 reason=timeout", 0.6)
    a("cooldown remaining_s=119.4", 0.1)
    L.append(("FP", GOOD_FP[1][1], 0.1, "task"))
    return L


AC130_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m4_ac130_session.log"
AC_KILL_RE = re.compile(r"KILL gun=(\d+) cid=(c\d+) d_m=[\d.]+ tier=(\w+)")


def ac130_self_test(run):
    """M4 rows: the fixture's good ride passes every AC-130 row; each design mutation FAILs its row; logs without
    AC130 lines are N/A."""
    good = synthetic(True, ac=ac130_ride())
    assert AC130_FIXTURE.read_text(encoding="utf-8") == good, (
        f"{AC130_FIXTURE.name} differs from ac130_ride() (regenerate with --write-m4-fixture)")
    res = run(good)
    bad = [r for r in res if r[0] != "PASS" and not ((r[1] in MOVE_ROWS or r[1] in analyze_m6.M6_ROWS or r[1] in analyze_m7.M7_ROWS or r[1] in analyze_m8.M8_ROWS or r[1] in analyze_r18.R18_ROWS or r[1] in analyze_coop.COOP_ROWS or r[1] in analyze_c3.C3_ROWS) and r[0] == "N/A")]  # M5-M8 rows: N/A without their lines
    assert not bad, f"the M4 fixture failed: {bad}"
    cases = [
        ("missing PROTECT restore", dict(protect_restore=False), "ac130 lifecycle"),
        ("player_hp_min < start", dict(hp_min=420), "ac130 protection"),
        ("missing camera restore", dict(cam_restore=False), "ac130 lifecycle"),
        ("gpu p95 1.4 ms", dict(gpu_p95=1.4), "ac130 perf"),
        ("PERF ac130=1 avg 30", dict(tps=30.0), "ac130 perf"),
        ("40 mm interval 0.2 s", dict(gap_40=0.2), "ac130 guns"),
    ]
    for label, kw, row in cases:
        got = status_of(run(synthetic(True, ac=ac130_ride(**kw))), row)
        assert got == "FAIL", f"M4 mutation {label!r}: {row} is {got}, want FAIL"
    t = good.replace("EXPLODE seen id=79000302", "EXPLODE seen id=79000999")
    assert status_of(run(t), "ac130 explosions") == "FAIL", "a missing 'EXPLODE seen' was not detected"
    t = good.replace("clip=19/20 ", "clip=20/20 ")
    assert status_of(run(t), "ac130 guns") == "FAIL", "clip size exceeded was not detected"
    t = good.replace("persisted=0", "persisted=3", 1)
    assert status_of(run(t), "ac130 perf") == "FAIL", "persisted != 0 was not detected"
    # after run 9: the effects row (earthquakes per shot/impact, the rows' object damage)
    t = "".join(ln for ln in good.splitlines(keepends=True) if "SHAKE src=impact gun=105" not in ln)
    assert status_of(run(t), "ac130 effects") == "FAIL", "a 105 EXPLODE without its impact SHAKE was not detected"
    t = good.replace("obj=201:200b,", "obj=201:100b,")
    assert status_of(run(t), "ac130 effects") == "FAIL", "a 105 core atkObj <= 199 was not detected"
    t = good.replace("amp=0.400 on=1", "amp=0.000 on=1")
    assert status_of(run(t), "ac130 effects") == "FAIL", "a zero 105 impact quake with the shake on was not detected"
    t = "".join(ln for ln in good.splitlines(keepends=True) if "AC130 effects shake=" not in ln and "AC130 SHAKE" not in ln)
    assert status_of(run(t), "ac130 effects") == "N/A", "a DLL before the effects update must be N/A"
    # review M4 major 7: a ride whose explosions hurt nothing must FAIL; a KILL without its HIT must FAIL
    t = "".join(ln for ln in good.splitlines(keepends=True) if "src=ac130" not in ln and "AC130 KILL" not in ln)
    assert status_of(run(t), "ac130 explosions") == "FAIL", "0 HIT src=ac130 was not detected"
    t = good.replace("hp 219->0 dmg=219", "hp 219->219 dmg=0")
    assert status_of(run(t), "ac130 explosions") == "FAIL", "HIT src=ac130 with dmg 0 only was not detected"
    t = good.replace("n=1 kill=1 src=ac130", "n=1 kill=0 src=ac130")
    assert status_of(run(t), "ac130 explosions") == "FAIL", "an AC130 KILL without its HIT was not detected"
    # AC-130 HITs never reach the UMP45 calibration (their band ids are not UMP45 rows), including the old
    # band-0 last_hit_by kill during the ride; outside a ride the same line is still judged (and FAILs)
    assert status_of(res, "hits") == "PASS", "an AC-130 HIT leaked into the UMP45 hits row"
    assert status_of(res, "first person") == "PASS", "FP off reason=ac130 was not accepted"
    t = good.replace("SHOT gun=105 n=1 clip=0/1 ", "SHOT gun=105 n=1 clip=0 ")
    assert status_of(run(t), "ac130 guns") == "PASS", "the old clip=<n> shape must still parse"
    lone = synthetic(True, hit=GOOD_HITS + ["chr=c4070 hp 88->0 dmg=88 band=0 mw2=0 d=0.0m via=last_hit_by n=0 kill=1"])
    assert status_of(run(lone), "hits") == "FAIL", "a band-0 HIT outside any ride must still FAIL the hits row"
    t = good.replace("off reason=ac130 ", "off reason=bogus ")
    assert status_of(run(t), "first person") == "FAIL", "an unknown FP handback reason was accepted"
    # older logs: N/A
    plain = run(synthetic(True))
    na = [status_of(plain, r) for r in AC_ROWS]
    assert na == ["N/A"] * len(AC_ROWS), f"a log without AC130 lines must be N/A for the AC-130 rows: {na}"
    off = run(synthetic(True, ac=[("AC130", "disabled by config ([ac130] enabled = false)", 0.0, "boot")]))
    assert [status_of(off, r) for r in AC_ROWS] == ["N/A"] * len(AC_ROWS), "[ac130] disabled must be N/A"
    return len(cases) + 14


# The real run-8 session (logs/mw2er.log, 2026-10-17 15:45-15:49, copied verbatim: our own DLL log, no game data):
# one AC-130 ride, the imgui PANIC at its start, AC130 SHOT lines with clip=<n>/<size>, two band-0 last_hit_by kills
# and 'FP off reason=ac130'.
RUN8_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m4_run8_session.log"
RUN8_SHA256 = "315b0611bc226c4b13238865f7e4647294ea2a3c1fe8a16917381143a0e13b53"


def real_run8_self_test():
    import hashlib
    raw = RUN8_FIXTURE.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == RUN8_SHA256, f"{RUN8_FIXTURE.name} changed (sha256)"
    ctx = load_context()
    assert not ctx["missing"], f"real-log self-test needs the project data files: {ctx['missing']}"
    entries = sessions(parse(raw.decode("utf-8")))[-1]
    res = analyze(entries, ctx)[0]
    got = {n: s_ for s_, n, _ in res}
    want = {"fire rate": NEEDS_DATA, "hits": "PASS", "first person": "PASS", "ac130 guns": "PASS",
            "ac130 explosions": "PASS", "ac130 lifecycle": "PASS", "no crashes": "FAIL"}
    bad = {k: (got.get(k), v) for k, v in want.items() if got.get(k) != v}
    assert not bad, f"run 8 rows (got, want): {bad}"
    fp = next(d for s_, n, d in res if n == "first person")
    assert "unknown handback reason" not in fp and "'ac130': 1" in fp, f"run 8 first person: {fp[:120]}"
    guns = next(d for s_, n, d in res if n == "ac130 guns")
    assert "gun 105: 3 shot(s)" in guns and "gun 25: 16 shot(s)" in guns, guns[:200]
    return ("real run-8 log: fire rate NEEDS-DATA (12 UMP45 shots), hits PASS (2 band-0 AC-130 kills excluded), "
            "first person PASS (reason ac130, hook mode), ac130 guns PASS (31 SHOT lines), no crashes FAIL (the imgui PANIC)")


# ---- M5 movement (docs/M5_DESIGN.md section 13.3) ------------------------------------------------------------

MOVE_ROWS = ("movement", "movement speeds", "sprint", "jump", "fall damage", "move perf")
MOVE_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m5_move_session.log"
# hand values of section 13.3 (m/s), keyed by (stance, wish speed in in/s rounded)
MOVE_SPEEDS = {("stand", 190): ("stand W", 4.83), ("stand", 152): ("strafe / ADS", 3.86), ("stand", 133): ("back", 3.38),
               ("stand", 285): ("sprint", 7.24), ("crouch", 124): ("crouch", 3.14), ("prone", 29): ("prone", 0.72)}
MOVE_STATE_RE = re.compile(r"^state t=[\d.]+ stance=(\w+) frac=[\d.]+ speed_sim=([\d.]+)in/s speed_world=([\d.]+)m/s "
                           r"wish=([\d.]+) sprint=([01]) .*ground=([01]) .*pushout=([01])")
MOVE_LAND_RE = re.compile(r"^land fall_h=([\d.]+)in .* dmg_pct=([\d.]+) hp=(-?\d+)->(-?\d+) max_hp=(\d+)")
SPRINT_OUT_S = 0.3
MOVE_SAMPLES_MIN = 5


def move_present(entries):
    if not any(e["tag"] == "MOVE" and e["msg"].startswith("cfg ") for e in entries):
        return "no 'MOVE cfg' line (DLL before M5)"
    return None


def judge_movement(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    mv = [e for e in entries if e["tag"] == "MOVE" or (e["tag"] == "TASK" and "world left" in e["msg"])]
    ons = [e for e in mv if e["tag"] == "MOVE" and e["msg"].startswith("on ")]
    if not ons:
        return "FAIL", "no 'MOVE on' (the controller never took over)"
    problems = []
    driving = False
    for e in mv:
        m = e["msg"]
        if e["tag"] == "MOVE" and m.startswith("on "):
            if driving:
                problems.append("'MOVE on' twice without a release")
            driving = True
        elif e["tag"] == "MOVE" and (m.startswith("off ") or m.startswith("suspend ")):
            if not re.match(r"^(off|suspend) reason=\w+", m):
                problems.append(f"release without a reason: {m[:60]}")
            driving = False
        elif e["tag"] == "TASK" and driving:
            problems.append("'TASK world left' while driving without a release first")
            driving = False
    # every gravity write has its restore: ok=1, or deferred followed by ok=1 / stale
    gw = [e for e in mv if e["tag"] == "MOVE" and e["msg"].startswith("write field=CSChrPhysicsModule.gravity_multiplier")]
    rs = [e for e in mv if e["tag"] == "MOVE" and e["msg"].startswith("restore field=gravity_multiplier")]
    for w_ in gw:
        later = [r for r in rs if r["t"] >= w_["t"]]
        done = [r for r in later if "ok=1" in r["msg"] or "skipped=stale" in r["msg"] or "skipped=changed" in r["msg"]]
        if not done:
            problems.append(f"gravity write at {w_['raw'][11:23]} has no restore (ok=1 / stale)")
    if any(e["tag"] == "PANIC" for e in entries):
        problems.append("PANIC")
    reasons = sorted({m.group(1) for e in mv if (m := re.match(r"^(?:off|suspend) reason=(\w+)", e["msg"]))})
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", f"{len(ons)} 'MOVE on', every one released (reasons {reasons}); {len(gw)} gravity write(s) all restored"


def judge_move_speeds(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    samples = {}
    for e in entries:
        if e["tag"] != "MOVE" or not (m := MOVE_STATE_RE.match(e["msg"])):
            continue
        stance, sim, world, wish, _, ground, push = m.groups()
        sim, world, wish = float(sim), float(world), float(wish)
        if ground != "1" or push != "0" or wish <= 0 or abs(sim - wish) > 0.01 * wish:
            continue
        key = (stance, int(round(wish)))
        if key in MOVE_SPEEDS:
            samples.setdefault(key, []).append((sim, world))
    judged, bad, few = [], [], []
    for key, (label, want) in MOVE_SPEEDS.items():
        s = samples.get(key, [])
        if len(s) < MOVE_SAMPLES_MIN:
            if s:
                few.append(f"{label} {len(s)}")
            continue
        med_world = statistics.median(w for _, w in s)
        med_sim = statistics.median(x * 0.0254 for x, _ in s)
        ok = abs(med_world - want) <= 0.05 * want and abs(med_world - med_sim) <= 0.05 * med_sim
        (judged if ok else bad).append(f"{label} {med_world:.2f} m/s (want {want}, sim {med_sim:.2f}, n={len(s)})")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    if not judged:
        return NEEDS_DATA, f"fewer than {MOVE_SAMPLES_MIN} steady 'MOVE state' samples per state ({', '.join(few) or 'none'})"
    return "PASS", "; ".join(judged) + (f"; too few: {', '.join(few)}" if few else "")


def judge_sprint(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    mv = [e for e in entries if e["tag"] == "MOVE"]
    ends = [e for e in mv if e["msg"].startswith("sprint end ")]
    starts = [e for e in mv if e["msg"].startswith("sprint start ")]
    if not starts:
        return NEEDS_DATA, "no 'MOVE sprint start' (tap Shift while holding W)"
    bad = []
    for e in ends:
        m = re.match(r"sprint end why=(\w+) dur=([\d.]+)s", e["msg"])
        if m and m.group(1) == "time" and abs(float(m.group(2)) - 4.0) > 0.05:
            bad.append(f"why=time dur={m.group(2)}s (want 4.00 +- 0.05)")
    for e in starts:
        m = re.search(r"meter=([\d.]+)s", e["msg"])
        if m and float(m.group(1)) <= 1.0:
            bad.append(f"sprint started with meter={m.group(1)}s (min 1.0)")
    shots = [e["t"] for e in entries if e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"])]
    for e in ends:
        if "why=fire" in e["msg"]:
            early = [t for t in shots if 0.0 <= t - e["t"] < SPRINT_OUT_S - 0.017]
            if early:
                bad.append(f"SHOT {early[0] - e['t']:.3f} s after 'sprint end why=fire' (sprint_out {SPRINT_OUT_S} s)")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    whys = sorted({m.group(1) for e in ends if (m := re.match(r"sprint end why=(\w+)", e["msg"]))})
    return "PASS", f"{len(starts)} sprint(s), ends {whys}; time ends at 4.00 +- 0.05 s; no shot within sprint_out"


def judge_jump(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    ap = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("apex ")]
    free = [e for e in ap if "ceiling=1" not in e["msg"]]
    if not free:
        return NEEDS_DATA, "no 'MOVE apex' without a ceiling (jump once in the open)"
    # M8 P2 (c): each apex against its own jump's take-off speed (v0^2 / 2g; MW2's landing penalty lowers v0 with
    # land_factor > 1, run 17: 0.43-0.62 m apexes at land_factor 1.76-2.50 are MW2, not a bug)
    jumps = [(e["t"], m) for e in entries if e["tag"] == "MOVE" and (m := re.match(r"jump y0=[-\d.]+ v0=([\d.]+) land_factor=([\d.]+)", e["msg"]))]
    bad = []
    hs = []
    slowed = 0
    for e in free:
        m = re.match(r"apex h=([\d.]+)m", e["msg"])
        h = float(m.group(1)) if m else -1.0
        prev = [j for t, j in jumps if t <= e["t"] + 1e-6]
        v0 = float(prev[-1].group(1)) if prev else 249.8
        lf = float(prev[-1].group(2)) if prev else 1.0
        want = v0 * v0 / (2 * 800.0) * 0.0254
        if lf > 1.0:
            slowed += 1
        hs.append(h)
        if abs(h - want) > 0.03:
            bad.append(f"apex {h:.3f} m (want {want:.3f} +- 0.03 for v0 {v0:.1f}, land_factor {lf:.2f})")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", (f"{len(free)} apex line(s), h {min(hs):.3f}..{max(hs):.3f} m (MW2 39 in = 0.991 m at land_factor 1.00"
                    + (f"; {slowed} after a landing penalty, judged against their v0" if slowed else "") + ")")


def judge_fall(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    lands = [(e, m) for e in entries if e["tag"] == "MOVE" and (m := MOVE_LAND_RE.match(e["msg"]))]
    if not lands:
        return NEEDS_DATA, "no 'MOVE land' line"
    deaths = [e["t"] for e in entries if e["tag"] == "MOVE" and re.match(r"^suspend reason=death", e["msg"])]
    bad, dmg_n, skipped_n = [], 0, 0
    for e, m in lands:
        fh, pct, a, b, mx = float(m.group(1)), float(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5))
        want = 0.0 if fh <= 200 else 100.0 if fh >= 350 else (fh - 200) / 150 * 100
        if abs(pct - want) > 0.5:
            bad.append(f"fall_h {fh} in -> dmg {pct}% (want {want:.1f})")
        if "fall_damage=off" in e["msg"]:
            continue
        if "fall_damage=skipped_er_ground" in e["msg"]:
            # review M5 major 1: ER reported ground during the fall: no HP write may follow
            skipped_n += 1
            if b != a:
                bad.append(f"hp {a}->{b} written for a fall ER did not confirm (fall_damage=skipped_er_ground)")
            continue
        if pct >= 100.0:
            if b != 0:
                bad.append(f"lethal landing wrote hp {b}")
            if not any(0 <= t - e["t"] <= 2.0 for t in deaths):
                bad.append("lethal_fall no_death_seen (no 'suspend reason=death' within 2 s)")
        elif pct > 0:
            dmg_n += 1
            exp = max(1, a - round(pct / 100 * mx))
            if abs(b - exp) > 1:
                bad.append(f"hp {a}->{b} for {pct}% of {mx} (want {exp})")
            if any(0 <= t - e["t"] <= 3.0 for t in deaths):
                bad.append(f"death within 3 s after a non-lethal landing ({pct}%)")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    gd = [e for e in entries if e["tag"] == "MOVE" and re.match(r"^(?:suspend|off) reason=ground_disagree", e["msg"])]
    return "PASS", (f"{len(lands)} landing(s), {dmg_n} damaging, dmg_pct = MW2 200/350 in formula, hp deltas match"
                    + (f"; {skipped_n} skipped (ER saw ground during the fall)" if skipped_n else "")
                    + (f"; {len(gd)} ground_disagree handback(s)" if gd else ""))


def judge_move_perf(entries):
    na = move_present(entries)
    if na:
        return "N/A", na
    cfg = next((e["msg"] for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("cfg ")), "")
    m = re.search(r"rays_max=(\d+)", cfg)
    rays_cap = int(m.group(1)) if m else 32
    on, off = [], []
    bad = []
    for e in entries:
        if e["tag"] != "PERF":
            continue
        tps = re.search(r"ticks_per_s=([\d.]+)", e["msg"])
        mv = re.search(r"move=([01]) move_us_p50=(\d+) move_us_p95=(\d+) move_us_max=(\d+) rays_avg=([\d.]+) rays_max=(\d+)", e["msg"])
        if not (tps and mv):
            continue
        if mv.group(1) == "1":
            on.append(float(tps.group(1)))
            if int(mv.group(3)) >= 1000:
                bad.append(f"move_us_p95={mv.group(3)}")
            if int(mv.group(6)) > rays_cap:
                bad.append(f"rays_max={mv.group(6)} > {rays_cap}")
        else:
            off.append(float(tps.group(1)))
    if not on:
        return NEEDS_DATA, "no PERF window with move=1"
    avg = statistics.mean(on)
    base = statistics.mean(off) if off else None
    if not (avg >= 55.0 or (base and avg >= 0.95 * base)):
        bad.append(f"ticks_per_s avg {avg:.1f} with move=1 (want >= 55 or >= 95% of {base})")
    if bad:
        return "FAIL", "; ".join(bad[:3])
    return "PASS", f"{len(on)} PERF window(s) with move=1: ticks_per_s avg {avg:.1f}" + (f" (move=0 {base:.1f})" if base else "")


def move_keys_info(entries):
    kp = [e["msg"] for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("keyprobe ")]
    return ("move keys: " + "; ".join(k[len("keyprobe "):] for k in kp)) if kp else None


def move_session(ok_restore=True, sprint_dur=4.0, apex=0.991, sprint_speed=7.24, tps=59.8, death_after_nonlethal=False,
                 shot_after_fire=None, skipped_hp_written=False):
    """A made-up M5 session (the shapes of the DLL's MOVE lines), inserted into the synthetic M2..M4 log."""
    L = []

    def a(msg, dt=0.0, tag="MOVE", thread="task"):
        L.append((tag, msg, dt, thread))

    a("cfg enabled=1 tp_anim=hybrid sprint=press crouch=toggle jump_slowdown=1 fall=mw2 eye=1.600/1.067/0.293 "
      "hull=1.867/1.333/0.800 r=er rays_max=32 data=converted\\movement.json sha=2f8c9cfda1b8", dt=0.1)
    a("data g_speed=190 sprint=1.5x4.0s min=1.0 jump=39in grav=800 accel=9/12/19 air=1 fric=5.5/100 step=18/10 "
      "fall=200/350 ump45 move=1 ads=2 dur=1 src_exe_sha=2f8c9cfda1b8")
    a("keyprobe key=SPACE er_bits=backstep|rolling", dt=0.5)
    a("resync why=world_enter d=0.00m chunk_d=0.00m", dt=0.5)
    a("on pos=(1.00,2.00,3.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1")
    a("write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=0x1000 old=1.000 new=0.000 ok=1")
    a("write field=CSChrPhysicsModule.gravity_disabled(+0x1d5) addr=0x1009 old=0 new=1 ok=1")
    a("write field=CSChrPhysicsModule.position(+0x70) addr=0x1070 old=(1.00,2.00,3.00) new=(1.00,2.00,3.01)")
    a("write field=CSChrPhysicsModule.chr_proxy_pos_update_requested(+0x91) addr=0x1091 old=? new=1")
    for k in range(8):
        a(f"state t={10 + k:.2f} stance=stand frac=1.00 speed_sim=190.0in/s speed_world=4.83m/s wish=190.0 sprint=0 "
          f"meter=4.00s ground=1 er_ground=1/1/1 slope_rays=0.0deg slope_er=0.0deg rays=12 er_delta=0.010m pushout=0", dt=0.25)
    for k in range(6):
        a(f"state t={20 + k:.2f} stance=stand frac=1.00 speed_sim=152.0in/s speed_world=3.86m/s wish=152.0 sprint=0 "
          f"meter=4.00s ground=1 er_ground=1/1/1 slope_rays=0.0deg slope_er=0.0deg rays=12 er_delta=0.010m pushout=0", dt=0.25)
    a("sprint start meter=4.00s stance_from=stand", dt=0.5)
    for k in range(6):
        a(f"state t={30 + k:.2f} stance=stand frac=1.00 speed_sim=285.0in/s speed_world={sprint_speed:.2f}m/s wish=285.0 "
          f"sprint=1 meter={3.5 - 0.5 * k:.2f}s ground=1 er_ground=1/1/1 slope_rays=0.0deg slope_er=0.0deg rays=12 "
          f"er_delta=0.010m pushout=0", dt=0.5)
    a(f"sprint end why=time dur={sprint_dur:.2f}s meter=0.00s", dt=1.0)
    a("sprint refused why=meter meter=0.48s", dt=0.5)
    a("sprint start meter=1.05s stance_from=stand", dt=0.6)
    a("sprint end why=fire dur=0.40s meter=0.65s", dt=0.4)
    if shot_after_fire is not None:
        a("n=99 mag=20/150 id=79000100 mw2=40 d=8.0m src=chr", dt=shot_after_fire, tag="SHOT")
    a("jump y0=2.000 v0=249.8 land_factor=1.00", dt=1.0)
    a(f"apex h={apex:.3f}m t=0.312s", dt=0.31)
    a("land fall_h=39.0in (0.99m) v_impact=249.8 slow=0.67*0.65 dmg_pct=0.0 hp=1000->1000 max_hp=1000 land_ms=0", dt=0.31)
    a("land fall_h=236.2in (6.00m) v_impact=614.8 slow=0.37*1.00 dmg_pct=24.1 hp=1000->759 max_hp=1000 land_ms=1344",
      dt=2.0)
    if death_after_nonlethal:
        a("suspend reason=death hp=0 death_flag=1 ticks=900", dt=1.0)
        a("restore field=gravity_multiplier addr=0x1000 old=0.000 new=1.000 ok=1")
        a("restore field=gravity_disabled addr=0x1009 old=1 new=0 ok=1")
        a("on pos=(1.00,2.00,3.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1", dt=3)
        a("write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=0x1000 old=1.000 new=0.000 ok=1")
        a("write field=CSChrPhysicsModule.gravity_disabled(+0x1d5) addr=0x1009 old=0 new=1 ok=1")
    # review M5 major 1: a fall ER did not confirm (it reported ground during it) writes no HP
    a("land fall_h=275.6in (7.00m) v_impact=664.0 slow=0.33*1.00 dmg_pct=50.4 hp=759->%d max_hp=1000 "
      "fall_damage=skipped_er_ground er_ground_ticks=12 land_ms=2000" % (500 if skipped_hp_written else 759), dt=2.0)
    a("ground_disagree ray_miss_ticks=30 er_ground=1 pos=(1.00,2.00,3.00) (ER stands on something our map rays miss; "
      "ER drives 3 s)", dt=1.0)
    a("suspend reason=ground_disagree ground ray missed 30 ticks while ER reported ground (hold 3 s, held_ticks=30) "
      "ticks=4000", dt=0.02)
    a("restore field=gravity_multiplier addr=0x1000 old=0.000 new=1.000 ok=1")
    a("restore field=gravity_disabled addr=0x1009 old=1 new=0 ok=1")
    a("on pos=(1.00,2.00,3.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.80 fp=1", dt=3.2)
    a("write field=CSChrPhysicsModule.gravity_multiplier(+0x1cc) addr=0x1000 old=1.000 new=0.000 ok=1")
    a("write field=CSChrPhysicsModule.gravity_disabled(+0x1d5) addr=0x1009 old=0 new=1 ok=1")
    a("stance from=stand to=crouch dur_ms=200", dt=1.0)
    a("stance from=crouch to=stand dur_ms=200", dt=1.0)
    a(f"ticks_per_s={tps:.1f} task_us p50=600 p95=900 max=2000 fp_us avg=100 max=300 writes=9000 write_us_max=2.0 "
      f"slow_writes=0 page_checks=12 page_check_ms_max=4.00 page_check_ms_total=20.0 cached_pages=30 ac130=0 move=1 "
      f"move_us_p50=60 move_us_p95=120 move_us_max=400 rays_avg=13.0 rays_max=24 ray_us_max=8.0 move_writes=600",
      dt=1.0, tag="PERF")
    a("er_delta p50=0.010 p95=0.030 max=0.120 applied=3 handbacks=0 budget_hits=0 gravity_reasserts=2 "
      "ground_disagree_streaks=0", dt=0.1)
    a("order detour_before_task=600/600 (camera-step detour calls between task ticks; feet used=600 rejected=0)")
    a("suspend reason=menu cursor ticks=1500", dt=1.0)
    if ok_restore:
        a("restore field=gravity_multiplier addr=0x1000 old=0.000 new=1.000 ok=1")
        a("restore field=gravity_disabled addr=0x1009 old=1 new=0 ok=1")
    return L


def move_self_test(run):
    """M5 rows: the good session passes all six, each mutation of section 13.3 fails its row, older logs are N/A."""
    good = run(synthetic(True, ac=move_session()))
    st = {r: status_of(good, r) for r in MOVE_ROWS}
    assert all(v == "PASS" for v in st.values()), f"good M5 session: {st} " + str([d for s, n, d in good if n in MOVE_ROWS])
    muts = [
        ("missing gravity restore", move_session(ok_restore=False), "movement"),
        ("sprint dur=4.6", move_session(sprint_dur=4.6), "sprint"),
        ("apex 1.3 m", move_session(apex=1.3), "jump"),
        ("sprint speed 6.0 m/s", move_session(sprint_speed=6.0), "movement speeds"),
        ("PERF avg 40 with move=1", move_session(tps=40.0), "move perf"),
        ("non-lethal landing followed by death", move_session(death_after_nonlethal=True), "fall damage"),
        ("SHOT 0.1 s after a sprint end by fire", move_session(shot_after_fire=0.1), "sprint"),
        ("HP written on a fall ER did not confirm", move_session(skipped_hp_written=True), "fall damage"),
    ]
    for label, sess, row in muts:
        got = status_of(run(synthetic(True, ac=sess)), row)
        assert got == "FAIL", f"M5 mutation {label!r}: {row} = {got}, want FAIL"
    old = run(synthetic(True))
    assert [status_of(old, r) for r in MOVE_ROWS] == ["N/A"] * len(MOVE_ROWS), "a log without MOVE cfg must be N/A"
    fx = sessions(parse(MOVE_FIXTURE.read_text(encoding="utf-8")))[-1] if MOVE_FIXTURE.exists() else None
    assert fx is not None, f"{MOVE_FIXTURE} missing (python -I analyze_log.py --write-m5-fixture)"
    fres = analyze(fx, synthetic_context())[0]
    assert all(status_of(fres, r) == "PASS" for r in MOVE_ROWS), "the M5 fixture must PASS every movement row"
    for name in ("m4_ac130_session.log", "m4_run8_session.log", "m3_run3_session.log"):
        p = MOVE_FIXTURE.parent / name
        if p.exists():
            r = analyze(sessions(parse(p.read_text(encoding="utf-8", errors="replace")))[-1], synthetic_context())[0]
            assert [status_of(r, x) for x in MOVE_ROWS] == ["N/A"] * len(MOVE_ROWS), f"{name} must be N/A on the M5 rows"
    return len(muts)


# ---- all criteria -----------------------------------------------------------------------------------------

# Version gate PASS lines (port 2.7.2.0; review 2026-10-20). Per build: its table name and PE header, and the preflight
# summary. A 2.7.1.0 line WITHOUT any table= is the pre-port DLL (archived run logs / fixtures) and stays accepted; a
# post-port 2.7.1.0 line must carry table=Ww2710 and its preflight like the 2.7.2.0 one.
VERSION_BUILDS = {
    "2.7.1.0": ("Ww2710", "timedatestamp=0x6a96b418 sizeofimage=0x5e0da00"),
    "2.7.2.0": ("Ww2720", "timedatestamp=0x6aabf7e7 sizeofimage=0x5dfd600"),
}


def version_line_ok(msg):
    for ver, (table, header) in VERSION_BUILDS.items():
        if not msg.startswith(f"PASS file={ver} product={ver} "):
            continue
        if ver == "2.7.1.0" and "table=" not in msg:
            return "timedatestamp=" not in msg or header in msg
        return f" table={table} preflight sites=" in msg and header in msg
    return False

def analyze(entries, ctx):
    res = []

    def add(name, status, detail):
        res.append((status, name, detail))

    def lines(tag, pat=""):
        r = re.compile(pat)
        return [e for e in entries if e["tag"] == tag and r.search(e["msg"])]

    mode_on = [m.group(1) for e in lines("MODE", r"^on weapon=") if (m := re.match(r"on weapon=(\S+)", e["msg"]))]
    weapon = mode_on[0] if mode_on else ctx["default_weapon"]
    wv, why = weapon_view(ctx, weapon) if weapon else (None, "no weapon (no MODE on line, no mw2er.toml)")
    m6 = analyze_m6.present(entries)
    if m6 and weapon:
        wv, why2 = analyze_m6.weapon_view_m6(ctx, weapon, wv)
        why = why2 or why

    v = lines("VERSION")
    if any(version_line_ok(e["msg"]) for e in v):
        warns = sum(1 for e in v if e["msg"].startswith("WARN preflight"))
        add("version gate", "PASS", v[0]["msg"] + (f" (+{warns} WARN preflight lines)" if warns else ""))
    else:
        add("version gate", "FAIL", v[0]["msg"] if v else "no VERSION line (DLL not loaded?)")

    bg, task, params = lines("BOOTGATE", r"^PASS"), lines("TASK", r"^registered"), lines("PARAMS")
    want_n = wv["params_n"] if wv else None
    ok_params = [e for e in params if want_n is not None and re.match(rf"OK n={want_n}\b", e["msg"])]
    if m6:
        ok_params += [e for e in params if re.match(r"OK guns=[1-9]\d* ", e["msg"])]
    if bg and task and ok_params:
        add("boot gate", "PASS", f"{bg[0]['msg'][:60]}...; TASK registered; PARAMS {ok_params[0]['msg']}")
    else:
        miss = [n for n, x in (("BOOTGATE PASS", bg), ("TASK registered", task),
                               (f"PARAMS OK n={want_n if want_n is not None else '?'}", ok_params)) if not x]
        detail = "missing " + ", ".join(miss)
        fails = [e["msg"] for e in params if e["msg"].startswith("FAIL")]
        if fails:
            detail += f"; PARAMS {fails[0]}"
        if wv is None:
            detail += f"; {why}"
        add("boot gate", "FAIL", detail)

    hud_init = lines("HUD", r"^initialize display=")
    queue_warn = [e for e in entries if e["tag"] == "HUDHOOK" and "command queue pointer" in e["msg"]]
    repeated = [e for e in queue_warn if "repeats" in e["msg"]]
    if hud_init and not repeated:
        add("HUD", "PASS", f"{hud_init[0]['msg']} (confirm crosshair, ammo, hitmarker on screen)")
    elif hud_init:
        add("HUD", "FAIL", f"initialised, but hudhook kept missing the command queue ({len(queue_warn)} lines): "
                           "try load_early = true on the me3 native")
    else:
        hf = lines("HUD", r"FAIL")
        add("HUD", "FAIL", hf[0]["msg"] if hf else "no 'HUD initialize' line (hudhook never saw a Present)")

    inp, menus = lines("INPUT", r"^PASS"), lines("MENU")
    if inp and menus:
        idx = sorted({int(m.group(1)) for e in menus if (m := re.match(r"idx=(\d+)", e["msg"]))})
        add("input + menus", "PASS", f"{inp[0]['msg']}; MENU transitions for ui_states {idx} "
                                     "(confirm the character did not swing while MW2 mode was on)")
    else:
        add("input + menus", "FAIL", ("no INPUT PASS" if not inp else "no MENU transitions (open the inventory once)"))

    shots = lines("SHOT", r"^n=\d+ mag=")  # "n=.. skipped: ..." lines are diagnostics, not shots
    if m6 and weapon:  # M6: the fire-rate row judges the MODE-on gun; the others are judged by 'shots per gun'
        shots = [e for e in shots if " gun=" not in e["msg"] or f" gun={weapon} " in e["msg"]]
    skipped = lines("SHOT", r"^n=\d+ skipped")
    spawn_fail = lines("SPAWN", r"^FAIL")
    times = [e["t"] for e in shots]
    gaps = [b - a for a, b in zip(times, times[1:]) if b - a < BURST_GAP_S]
    med = statistics.median(gaps) if gaps else None
    fire_time = wv["fire_time"] if wv else None
    want = f"fireTime {fire_time * 1000:.0f} ms" if fire_time else "fireTime unknown"
    in_tol = med is not None and fire_time and abs(med - fire_time) <= FIRE_TOL
    if fire_time and len(shots) >= FIRE_MIN_SHOTS and in_tol and not spawn_fail:
        add("fire rate", "PASS", f"{len(shots)} SHOT lines, median in-burst interval {med * 1000:.1f} ms ({want})")
    elif fire_time and 0 < len(shots) < FIRE_MIN_SHOTS and (med is None or in_tol) and not spawn_fail:
        # run 8: an AC-130 session with 12 UMP45 shots (the AC130 SHOT lines are judged by 'ac130 guns')
        add("fire rate", NEEDS_DATA, f"only {len(shots)} UMP45 SHOT lines (want >= {FIRE_MIN_SHOTS} in held bursts); "
                                     f"median interval {'n/a' if med is None else f'{med * 1000:.1f} ms'} ({want}), "
                                     f"0 SPAWN FAIL")
    else:
        add("fire rate", "FAIL", f"{len(shots)} SHOT lines, median interval "
                                 f"{'n/a' if med is None else f'{med * 1000:.1f} ms'} ({want}), "
                                 f"{len(spawn_fail)} SPAWN FAIL, {len(skipped)} skipped"
                                 + (f": {skipped[0]['msg'][:90]}" if skipped else ""))

    bullets, impacts = lines("BULLET"), lines("IMPACT")
    speeds = [float(m.group(1)) for e in bullets if (m := re.search(r"speed=([\d.]+)", e["msg"]))]
    hits_kind = [e for e in impacts if "kind=hit" in e["msg"]]
    want_speed = wv["speed"] if wv else 999.0
    if speeds and abs(speeds[0] - want_speed) <= 0.05 * want_speed and hits_kind:
        add("engine bullets", "PASS", f"BULLET speed {speeds[0]:.1f} m/s; {len(hits_kind)} IMPACT kind=hit, "
                                      f"{len(impacts) - len(hits_kind)} expired")
    else:
        add("engine bullets", "FAIL", f"BULLET lines {len(bullets)} (speed {speeds[:1]}, want ~{want_speed:.0f}), "
                                      f"IMPACT kind=hit {len(hits_kind)}")

    if m6:
        legacy = [e for e in entries if not (e["tag"] == "HIT" and " gun=" in e["msg"])]
        if any(e["tag"] == "HIT" for e in legacy):
            add("hits", *judge_hits(legacy, wv, ctx["window_s"]))
        else:
            add("hits", "N/A", "arsenal session: every HIT names its gun (judged by 'hits per gun' / 'ump45 calibration')")
    else:
        add("hits", *judge_hits(entries, wv, ctx["window_s"]))
    add("viewmodel", *judge_viewmodel(entries))
    add("vm anim", *judge_vm_anim(entries))
    add("camera", *judge_camera(entries))
    add("first person", *judge_first_person(entries))
    add("ac130 lifecycle", *judge_ac130_lifecycle(entries))
    add("ac130 protection", *judge_ac130_protection(entries))
    add("ac130 guns", *judge_ac130_guns(entries, ctx))
    add("ac130 explosions", *judge_ac130_explosions(entries))
    add("ac130 perf", *judge_ac130_perf(entries))
    add("ac130 effects", *judge_ac130_effects(entries))
    add("movement", *judge_movement(entries))
    add("movement speeds", *judge_move_speeds(entries))
    add("sprint", *judge_sprint(entries))
    add("jump", *judge_jump(entries))
    add("fall damage", *judge_fall(entries))
    add("move perf", *judge_move_perf(entries))
    for name, st_, detail in analyze_m6.rows(entries, ctx, er_damage):
        add(name, st_, detail)
    # M7 (docs/M7_DESIGN.md section 5): the run-14 fixes and the Tactical Nuke
    for name, st_, detail in analyze_m7.rows(entries, ctx):
        add(name, st_, detail)
    # M8 (docs/M8_DESIGN.md section 11): mantling, the chunk re-base fix and the ladder audit
    for name, st_, detail in analyze_m8.rows(entries, ctx):
        add(name, st_, detail)
    # run 18: the close-camera gate, the Havok ray filters and the unstick handback
    for name, st_, detail in analyze_r18.rows(entries, ctx):
        add(name, st_, detail)
    # co-op C2: the peer watch and the nuke's co-op rules
    for name, st_, detail in analyze_coop.rows(entries, ctx):
        add(name, st_, detail)
    # C3 PASS A: traversal (player filter, assist), first person stuck off, the UI yield, the damage numbers
    for name, st_, detail in analyze_c3.rows(entries, ctx):
        add(name, st_, detail)

    rs, rd = lines("RELOAD", r"^start "), lines("RELOAD", r"^done mag=")
    clip_ok = wv is not None and any(re.match(rf"done mag={wv['clip']}\b", e["msg"]) for e in rd)
    if m6 and rd:
        # M7 A4: each 'done' against the clip of the gun in hand at that time (its gun= field, else the last SWITCH
        # target / MODE on weapon); a reserve that ran out may fill less
        cur, ok_any = weapon, False
        for e in entries:
            if e["tag"] == "SWITCH" and (sm := re.search(r"\bto=(\S+)", e["msg"])):
                cur = sm.group(1)
            if e["tag"] == "MODE" and (mm := re.match(r"on weapon=(\S+)", e["msg"])):
                cur = mm.group(1)
            if e["tag"] == "RELOAD" and (dm := re.match(r"done mag=(\d+) reserve=(\d+)", e["msg"])):
                g = (re.search(r"\bgun=(\S+)", e["msg"]) or [None, cur])[1]
                clip = analyze_m6.num(ctx, g, "clipSize")
                if clip is not None and (int(dm.group(1)) == int(clip) or int(dm.group(2)) == 0):
                    ok_any = True
        clip_ok = clip_ok or ok_any
    if rs and rd and clip_ok:
        add("reload", "PASS", f"{rs[0]['msg']} -> {rd[0]['msg']}")
    else:
        add("reload", "FAIL", f"RELOAD start {len(rs)}, RELOAD done {len(rd)}"
                              f"{'' if clip_ok or not rd else ' (none refilled to the clip size)'} "
                              "(press R with a partly empty mag)")

    panics = [e for e in entries if e["tag"] == "PANIC"]
    disabled = [e for e in entries if e["tag"] == "ERROR" and "disabled" in e["msg"]]
    if not panics and not disabled:
        add("no crashes", "PASS", "no PANIC, no subsystem disabled (did the session end with the user quitting?)")
    else:
        add("no crashes", "FAIL", "; ".join(e["raw"][24:140] for e in (panics + disabled)[:3]))

    info = []
    if weapon:
        info.append(f"weapon {weapon}" + (f" (expectations: {why})" if wv is None else
                                          f": fireTime {wv['fire_time']}, {len(wv['bands'])} bands, "
                                          f"PARAMS n={wv['params_n']}"))
    cam = lines("CAMERA", r"^sign=")
    if cam:
        info.append("camera: " + cam[0]["msg"])
    odd = lines("CAMERA", r"^odd")
    if odd:
        info.append("camera: " + odd[0]["msg"])
    modes = lines("MODE")
    if modes:
        info.append("mode: " + ", ".join(e["msg"][:60] for e in modes[:4]))
    ai = ac130_input_info(entries)
    if ai:
        info.append(ai)
    mk = move_keys_info(entries)
    if mk:
        info.append(mk)
    ign = lines("RELOAD", r"^ignored")
    if ign:
        info.append(f"reload ignored x{len(ign)}")
    return res, info


def report(res, info):
    for status, name, detail in res:
        print(f"{status:<17} {name:16} {detail}")
    for i in info:
        print(f"INFO {i}")
    passed = all(s in ("PASS", "N/A") for s, _, _ in res)
    counted = [r for r in res if r[0] != "N/A"]
    print(f"M2+M3+M3b+M4+M5+M6+M7+M8 IN-GAME ORACLES: {'PASS' if passed else 'FAIL'} "
          f"({sum(s == 'PASS' for s, _, _ in counted)}/{len(counted)} PASS, "
          f"{sum(s == NEEDS for s, _, _ in res)} {NEEDS}, {sum(s == NEEDS_DATA for s, _, _ in res)} {NEEDS_DATA}, "
          f"{len(res) - len(counted)} N/A)")
    return passed


# ---- self-test ----------------------------------------------------------------------------------------------

def synthetic_context(fire_time=0.09, bands=6):
    """Expectations shaped like the real files (UMP45: 40..35 damage, atkPhys = 4 x damage)."""
    rows = {"bullet": {"speed_mps": 999.0}, "weapons": {"ump45_mp": {"slot": 1, "bands": [
        {"band": b, "id": 79000100 + b, "mw2_damage": 40 - b, "atk_phys": 4 * (40 - b)} for b in range(bands)]}}}
    weapons = {"ump45_mp": {"mw2": {"fireTime": fire_time, "clipSize": 32}}}
    ac130 = {"guns": [{"id": "105", "clip_size": 1, "fire_time_s": 0.1, "reload_s": 5.0},
                      {"id": "40", "clip_size": 4, "fire_time_s": 0.3, "reload_s": 3.0},
                      {"id": "25", "clip_size": 20, "fire_time_s": 0.07, "reload_s": 1.5}]}
    return {"weapons": weapons, "rows": rows, "window_s": 0.6, "default_weapon": "ump45_mp", "missing": [], "ac130": ac130,
            "guns": {}}


# The real M2 run 2 engagement (logs/mw2er.log 09:35:33.169..33.886: HPDROP 219->150->101->52->3 and the kill
# line "HIT chr=c4311 hp 3->0 dmg=3 band=79000105 mw2=35 d=36.0m"), rewritten in the M3 HIT format.
GOOD_HITS = [
    "chr=c4311 hp 219->150 dmg=69 band=79000105 mw2=35 d=36.0m via=impact n=1 kill=0",
    "chr=c4311 hp 150->101 dmg=49 band=79000105 mw2=35 d=36.0m via=impact n=1 kill=0",
    "chr=c4311 hp 101->52 dmg=49 band=79000105 mw2=35 d=36.0m via=impact n=1 kill=0",
    "chr=c4311 hp 52->3 dmg=49 band=79000105 mw2=35 d=36.0m via=impact n=1 kill=0",
    "chr=c4311 hp 3->0 dmg=3 band=79000105 mw2=35 d=36.0m via=impact n=1 kill=1",
]


# M3b: a good first-person episode pair (FP on -> menu handback -> FP on -> world left -> loading handback).
GOOD_FP = [
    ("FPCAM", "cfg enabled=1 mode=task eye_h=1.60 eye_fwd=0.10 eye_smooth_s=0.00 pitch_clamp=85.0 hide=render_flag yaw=1 "
              "world_fov=off arm_ticks=30 fade_alpha_max=0.50 signals fade=1 cam_mask=1 talk=1 key=F6"),
    ("FP", "on eye=(1.00,2.60,3.10) cam_gate=ok er_cam_dist=3.45m eye_to_chest=0.41m hp=500 ride=Some(0) mounted=0 "
           "ladder=-1 camtype=Some(0) mask=0x0 talk=Some(-1) fade_max=0.00 world_fov=48.0deg near=0.0501 mode=task "
           "hide=render_flag player=0x7ff000010000"),
    ("FP", "hide write player=0x7ff000010000 field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) addr=0x7ff0000101c5 "
           "old=0x08 new=0x00"),
    ("FPCAM", "write field=CSCamera.pers_cam_1+0x40 addr=0x7ff000020050 old=(1.00,4.00,0.00) new=(1.00,2.60,3.10)"),
    ("FPCAM", "er_rewrite=1: ER rebuilt pers_cam_1 row 3 between two ticks ((1.00,2.60,3.10) -> (1.02,4.00,0.01)); "
              "handback restore is free"),
    ("FP", "yaw probe conv=+z dot_med=0.981 n=60"),
    ("FP", "hide restore player=0x7ff000010000 field=ChrIns.chr_flags1c5.enable_render(+0x1c5 b3) old=0x00 new=0x08 "
           "saved=0x08 reasserted=0 ok=1 reason=menu"),
    ("FP", "off reason=menu  ticks_on=120 cam_writes=120 (re-arms when clear)"),
    ("FP", "on eye=(1.00,2.60,3.10) cam_gate=ok er_cam_dist=3.45m eye_to_chest=0.41m hp=500 ride=Some(0) mounted=0 "
           "ladder=-1 camtype=Some(0) mask=0x0 talk=Some(-1) fade_max=0.00 world_fov=48.0deg near=0.0501 mode=task "
           "hide=render_flag player=0x7ff000010000"),
    ("TASK", "world left (loading / title)"),
    ("FP", "off reason=loading world left ticks_on=300 cam_writes=300 (re-arms when clear)"),
]


def synthetic(ok=True, hit=None, hit_dt=0.01, vm=True, fp=None, ac=None):
    """A made-up log in the DLL's format (used only by --self-test). `hit` is one HIT message or a list. `fp` is a
    list of (tag, msg) first-person lines written after MODE on (default GOOD_FP; [] = a DLL before M3b). `ac` is a
    list of (tag, msg, dt, thread) M4 lines written after the UMP45 reload (default none = a DLL before M4)."""
    fp = GOOD_FP if fp is None else fp
    hits = GOOD_HITS if hit is None else ([hit] if isinstance(hit, str) else list(hit))
    L = []
    t = [12 * 3600.0]

    def w(tag, msg, lvl="I", thread="task", dt=0.0):
        t[0] += dt
        s_ = t[0]
        ms = int(round(s_ * 1000))  # format from whole milliseconds (float sums like 14.9999 must print 15.000)
        L.append(f"2026-10-14 {ms // 3600000:02d}:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}."
                 f"{ms % 1000:03d} {lvl} [{thread}] {tag} {msg}")

    w("BOOT", "mw2er 0.2.0 dll=x", thread="boot")
    w("VERSION", "PASS file=2.7.1.0 product=2.7.1.0 lang=0x0409", thread="boot")
    w("INPUT", "PASS hooked=2 addrs=[0x1,0x2]", thread="boot")
    w("HUD", "initialize display=1920x1080 font=built-in 26px", thread="present", dt=1)
    if vm:
        w("VM", "target fmt=R8G8B8A8_UNORM 1920x1080 colorspace=none frames=3", thread="present")
        w("VM", "assets ready ms=120 bytes=3500000 bones=91 verts=7489 clips=12 fmt=R8G8B8A8_UNORM", thread="vmload")
    w("BOOTGATE", "PASS after 900 ms names=[CSTask]", thread="boot")
    w("TASK", "registered group=ChrIns_PostPhysics", thread="boot")
    w("PARAMS", "OK n=13", dt=5)
    w("CAMERA", "sign=+1 cam_player_dist=3.80m fov_raw=0.84 source=pers_cam_1")
    w("CAMFRAME", "tick=4 src=pers_cam_1 valid=1 why=ok pos=(1,2,3) fwd=(0,0,1) det=+1.000 ortho_err=1e-6 "
                  "fov=0.8378(48.0deg) aspect=1.7778 near=0.1000 far=5000.0 cam_chest=3.50m")
    w("MODE", "on weapon=ump45_mp mag=32/192 menu_baseline=[1] show_cursor=0")
    for tag, msg in fp:
        w(tag, msg, dt=0.016)
    if vm:
        w("VM", "state hidden->raise clip=pullout rate=1.096 ads=0.00 seq=1", dt=0.016)
        w("VM", "first frame drawn 1920x1080 slot=0 out=Srgb", thread="present", dt=0.016)
    for i in range(25):
        w("SHOT", f"n={i + 1} mag={31 - i}/192 id=79000100 mw2=40 d=8.0m src=chr", dt=0.09 if i else 1)
        if i == 0:
            w("BULLET", "id=79000100 pos=(1,2,3) origin=(1,2,2) speed=999.0 m/s alive=0.016s", dt=0.016)
        if i < len(hits):
            w("IMPACT", "pos=(1,2,14) dist_from_origin=8.0m t=0.030s kind=hit", dt=0.01)
            if hits[i]:
                w("HIT", hits[i], dt=hit_dt if i == 0 else 0.01)
    w("RELOAD", "start empty=0 dur=2.5 mag=7", dt=1)
    if vm:
        w("VM", "state idle->reload clip=reload rate=1.000 ads=0.00 seq=9", dt=0.016)
    w("RELOAD", "done mag=32 reserve=167 added=25", dt=2.5)
    for tag, msg, dt, thread in (ac or []):
        w(tag, msg, thread=thread, dt=dt)
    w("MENU", "idx=12 0->1", dt=1)
    w("MENU", "idx=12 1->0", dt=1)
    if vm:
        w("VM", "gpu_ms p50=0.0130 p95=0.0150 max=0.0200 n=600", thread="present")
    if not ok:
        w("PANIC", "thread=task at src\\task.rs:1: boom", lvl="E")
        w("ERROR", "task disabled: gameplay off for this session", lvl="E")
    w("MODE", "off", dt=1)
    if vm:
        w("VM", "state idle->drop clip=putaway rate=0.958 ads=0.00 seq=12", dt=0.016)
    return "\n".join(L) + "\n"


def status_of(res, name):
    return next(s for s, n, _ in res if n == name)


def self_test():
    ctx = synthetic_context()
    run = lambda text, c=ctx: analyze(sessions(parse(text))[-1], c)[0]  # noqa: E731
    # The prediction reproduces docs/M2_DESIGN.md 10.6 (k = 4: atk 160 -> ~103, 140 -> ~82; x1.2 -> 137 / 112).
    g = KNOWN_TARGETS[4311]
    for (atk, corr), want in {(160, 1.0): 103, (140, 1.0): 82, (160, 1.2): 137, (140, 1.2): 112}.items():
        assert abs(predict(atk, g, corr) - want) <= 1.0, f"prediction for atk {atk} x{corr} drifted from the design"

    res = run(synthetic(True))
    bad = [r for r in res if r[0] != "PASS" and not ((r[1] in AC_ROWS or r[1] in MOVE_ROWS or r[1] in analyze_m6.M6_ROWS
                                                      or r[1] in analyze_m7.M7_ROWS or r[1] in analyze_m8.M8_ROWS or r[1] in analyze_r18.R18_ROWS or r[1] in analyze_coop.COOP_ROWS or r[1] in analyze_c3.C3_ROWS)
                                                     and r[0] == "N/A")]
    assert not bad, f"good synthetic log failed: {bad}"
    # port 2.7.2.0: the new gate line passes only with its table and preflight; a 2.7.2.0 line without them, an
    # unsupported version and the inert line FAIL
    new_ok = ("PASS file=2.7.2.0 product=2.7.2.0 lang=0x0409 timedatestamp=0x6aabf7e7 sizeofimage=0x5dfd600 table=Ww2720 "
              "preflight sites=113 code=20 vmt=83 inferred=4")
    old_ok = "PASS file=2.7.1.0 product=2.7.1.0 lang=0x0409"
    old_new = ("PASS file=2.7.1.0 product=2.7.1.0 lang=0x0409 timedatestamp=0x6a96b418 sizeofimage=0x5e0da00 table=Ww2710 "
               "preflight sites=113 code=4 vmt=79 inferred=30 warn=0")
    for msg, want in ((new_ok, "PASS"), (new_ok + " warn=1", "PASS"), (new_ok.replace(" table=Ww2720", ""), "FAIL"),
                      (new_ok.replace("preflight sites", "sites"), "FAIL"), (old_ok.replace("2.7.1.0", "2.7.3.0"), "FAIL"),
                      (new_ok.replace("0x6aabf7e7", "0x6a96b418"), "FAIL"),
                      (old_ok + " timedatestamp=0x6a96b418 sizeofimage=0x5e0da00", "PASS"),
                      (old_ok + " timedatestamp=0x6aabf7e7 sizeofimage=0x5dfd600", "FAIL"),
                      (old_new, "PASS"), (old_new.replace("table=Ww2710", "table=Ww2720"), "FAIL"),
                      (old_new.replace(" preflight", ""), "FAIL"),
                      ("FAIL found 2.7.2.0 (want 2.7.1.0); staying inert", "FAIL")):
        got = status_of(run(synthetic(True).replace(old_ok, msg)), "version gate")
        assert got == want, f"version gate {msg!r}: {got}, want {want}"
    res = run(synthetic(False))
    assert status_of(res, "no crashes") == "FAIL", "PANIC was not detected"
    assert len(sessions(parse(synthetic(True) + synthetic(False)))) == 2, "sessions are split on BOOT lines"
    res, _ = analyze(parse("2026-10-14 10:00:00.000 I [boot] BOOT mw2er 0.2.0\n"), ctx)
    not_failed = [r[1] for r in res if r[0] != "FAIL"]
    assert not_failed == ["first person", *AC_ROWS, *MOVE_ROWS, *analyze_m6.M6_ROWS, *analyze_m7.M7_ROWS, *analyze_m8.M8_ROWS, *analyze_r18.R18_ROWS, *analyze_coop.COOP_ROWS, *analyze_c3.C3_ROWS, "no crashes"],         f"an empty session must fail every positive criterion: {not_failed}"
    assert status_of(res, "first person") == "N/A", "a log without FPCAM cfg (pre-M3b DLL) is N/A for first person"

    def c(dmg, a=219, cid="4311", band="79000105", mw2=35, via="impact", n=1, kill=0):
        return f"chr=c{cid} hp {a}->{a - dmg} dmg={dmg} band={band} mw2={mw2} d=36.0m via={via} n={n} kill={kill}"

    hit_cases = [
        ("49, 49, 49, 69 non-kill on c4311 (band 105, as measured)", [c(49), c(49), c(49), c(69)], 0.01, "PASS"),
        ("a single hit of 103", [c(103)], 0.01, "FAIL"),
        ("only kill=1 hits", [c(23, a=23, kill=1), c(40, a=40, kill=1)], 0.01, NEEDS),
        ("n=2 98 excluded from calibration", [c(98, n=2)], 0.01, NEEDS),
        ("old-format HIT without via is still parsed", ["chr=c4311 hp 219->170 dmg=49 band=79000105 mw2=35 d=36.0m"] * 3,
         0.01, "PASS"),
        ("30, 30, 30", [c(30), c(30), c(30)], 0.01, "FAIL"),
        ("hits only on c4351", [c(50, a=657, cid="4351")] * 3, 0.01, NEEDS),
        ("two in-band hits only", [c(49), c(52)], 0.01, NEEDS),
        ("dmg=1 on the reference enemy", [c(1)], 0.01, "FAIL"),
        ("dmg=0", ["chr=c4311 hp 219->219 dmg=0 band=79000100 mw2=40 d=8.0m via=impact n=1 kill=0"], 0.01, "FAIL"),
        ("band not in rows.json", [c(49, band="79000177")], 0.01, "FAIL"),
        ("mw2 disagrees with the band", [c(49, mw2=37)], 0.01, "FAIL"),
        ("HIT long after the last SHOT", [c(49)], 30.0, "FAIL"),
        ("unknown enemy, implausible dmg", [c(1, a=500, cid="3100")], 0.01, "FAIL"),
        ("old format without band", ["chr=c4311 hp 219->170 dmg=49"], 0.01, "FAIL"),
        ("band 103 only: extrapolated, never PASS", [c(55, band="79000103", mw2=37)] * 3, 0.01, NEEDS),
        ("band 103 far off: still only NEEDS", [c(20, band="79000103", mw2=37)] * 3, 0.01, NEEDS),
        ("close-range band 100 (measured in M3 run 3): 49 x 3 PASS", [c(49, band="79000100", mw2=40)] * 3, 0.01, "PASS"),
        ("close-range band 100 far off now FAILs", [c(20, band="79000100", mw2=40)] * 3, 0.01, "FAIL"),
        ("blocked? (dmg=4, shield) next to normal hits: excluded, PASS",
         [c(49), c(4), c(49), c(4), c(49), c(4)], 0.01, "PASS"),
        ("blocked? at band 100 next to normal band-100 hits: PASS",
         [c(49, band="79000100", mw2=40), c(4, band="79000100", mw2=40)] * 3, 0.01, "PASS"),
        ("only tiny hits on c4311 (no normal hit): judged normally, FAIL", [c(4), c(4), c(4)], 0.01, "FAIL"),
        ("tiny but plausible-looking only hits (dmg=8 at atk 140): no normal hit, outside band, FAIL",
         [c(8), c(8), c(8)], 0.01, "FAIL"),
        ("measured band PASS + close band hits", [c(49), c(49), c(49), c(60, band="79000100", mw2=40)], 0.01, "PASS"),
        ("dmg=1 in band 100", [c(1, band="79000100", mw2=40)], 0.01, "FAIL"),
        ("kill=1 with truncated damage is exempt from plausibility", [c(49), c(49), c(49), c(2, a=2, kill=1)], 0.01, "PASS"),
    ]
    for label, hits_, dt, want in hit_cases:
        got = status_of(run(synthetic(True, hit=hits_, hit_dt=dt)), "hits")
        assert got == want, f"HIT case {label!r}: {got}, want {want}"
    # Session numbers are 1-based and range-checked.
    two = synthetic(True) + synthetic(False)
    assert pick_session(sessions(parse(two)), None) == (1, None), "default must be the last session (index 1)"
    assert pick_session(sessions(parse(two)), 1) == (0, None)
    assert pick_session(sessions(parse(two)), 3)[1] and pick_session(sessions(parse(two)), 0)[1], "range check"
    # Expectations come from the data files: a different fireTime or band count changes the verdict.
    assert status_of(run(synthetic(True), synthetic_context(fire_time=0.12)), "fire rate") == "FAIL", \
        "fireTime is not read from weapons.json"
    assert status_of(run(synthetic(True), synthetic_context(bands=11)), "boot gate") == "FAIL", \
        "PARAMS count is not derived from rows.json"
    no_data = {"weapons": None, "rows": None, "window_s": 0.6, "default_weapon": "ump45_mp", "missing": ["x"], "ac130": None}
    res = run(synthetic(True), no_data)
    assert status_of(res, "hits") == "FAIL" and status_of(res, "fire rate") == "FAIL", "missing data must fail"
    # A NEEDS-CALIBRATION criterion is not a PASS for the overall verdict.
    res = run(synthetic(True, hit=["chr=c3100 hp 500->420 dmg=80 band=79000100 mw2=40 d=11.8m via=impact n=1 kill=0"]))
    assert not all(s == "PASS" for s, _, _ in res)
    # M3 rows: an M2 log (no VM lines) fails viewmodel / vm anim; a VM FAIL line fails; slow GPU fails.
    res = run(synthetic(True, vm=False))
    assert status_of(res, "viewmodel") == "FAIL" and status_of(res, "vm anim") == "FAIL", "M2 log passed the VM rows"
    res = run(synthetic(True).replace("assets ready ms=120", "FAIL reason=assets: manifest.json missing; x"))
    assert status_of(res, "viewmodel") == "FAIL", "VM FAIL not detected"
    res = run(synthetic(True).replace("p95=0.0150", "p95=1.2500"))
    assert status_of(res, "viewmodel") == "FAIL", "gpu p95 >= 1 ms not detected"
    res = run(synthetic(True).replace("state idle->reload", "state idle->fire"))
    assert status_of(res, "vm anim") == "FAIL", "missing reload transition not detected"
    res = run(synthetic(True).replace("det=+1.000", "det=+0.500"))
    assert status_of(res, "camera") == "FAIL", "bad CAMFRAME det not detected"
    fp_cases = fp_self_test(run)
    ac_cases = ac130_self_test(run)
    blocked_real = real_run3_self_test()
    run8 = real_run8_self_test()
    move_cases = move_self_test(run)
    m6_cases = analyze_m6.self_test(lambda text, c: analyze(sessions(parse(text))[-1], c)[0], synthetic, ctx, M6_FIXTURE, prefix=ac130_ride())
    run13 = real_run13_self_test()
    m7_ok, m7_caught, m7_lines = analyze_m7.self_test()
    assert m7_ok and m7_caught >= 10, f"M7 self-test: {m7_lines}"
    m7_fx = m7_fixture_self_test()
    run14 = real_run14_self_test()
    m8_ok, m8_caught, m8_lines = analyze_m8.self_test()
    assert m8_ok and m8_caught >= 13, f"M8 self-test: {m8_lines}"
    m8_fx = m8_fixture_self_test()
    run17 = real_run17_self_test()
    r18_ok, r18_caught, r18_lines = analyze_r18.self_test()
    assert r18_ok and r18_caught >= 7, f"run-18 self-test: {r18_lines}"
    run18 = real_run18_self_test()
    coop_ok, coop_caught, coop_lines = analyze_coop.self_test()
    assert coop_ok and coop_caught >= 8, f"co-op self-test: {coop_lines}"
    c3_ok, c3_caught, c3_lines = analyze_c3.self_test()
    assert c3_ok and c3_caught >= 11, f"C3 self-test: {c3_lines}"
    print(f"PASS self-test: good log {len(res)}/{len(res)} PASS, PANIC detected, sessions split, empty log fails, "
          f"{len(fp_cases)} first-person cases, real run-3 log: {blocked_real}, "
          f"{len(hit_cases)} HIT cases (c4311 band 79000105 35..75 from real run 2: 69+49x3 PASS, 103 FAIL, "
          f"kill-only/n=2/c4351/band-103-only NEEDS; band 79000100 measured in run 3; blocked? hits excluded only next to normal hits on the same cid), 1-based range-checked session numbers, "
          f"M3 rows (viewmodel, vm anim, camera) fail on their mutations, expectations read from the data, "
          f"M4 fixture {AC130_FIXTURE.name}: good ride PASS on all {len(AC_ROWS)} AC-130 rows, {ac_cases} mutations FAIL, "
          f"older logs N/A; {run8}; "
          f"M5 fixture {MOVE_FIXTURE.name}: good session PASS on all {len(MOVE_ROWS)} movement rows, {move_cases} mutations "
          f"FAIL, older logs N/A; "
          f"M6 fixture {M6_FIXTURE.name}: good session PASS on all {len(analyze_m6.M6_ROWS)} M6 rows, {m6_cases} mutations "
          f"FAIL, older logs N/A on the arsenal rows; {run13}; "
          f"M7 fixture {M7_FIXTURE.name}: {m7_fx}, {m7_caught} M7 mutations FAIL; {run14}; "
          f"M8 fixtures {M8_MANTLE_FIXTURE.name} + {M8_LADDER_FIXTURE.name}: {m8_fx}, {m8_caught} M8 mutations FAIL, older logs N/A; {run17}; "
          f"run-18 rows: {r18_lines[0]}, {r18_caught} run-18 mutations FAIL, older logs N/A; {run18}; "
          f"co-op C2 rows: {coop_lines[0]}, {coop_lines[1]}, {coop_caught} co-op mutations FAIL, older logs N/A; C3 rows: {c3_lines[0]}, {c3_caught} C3 mutations FAIL, {c3_lines[-1]}")
    return 0


def fp_self_test(run):
    """M3b first-person row: the good episode passes, each mutation is caught, a pre-M3b log is N/A."""
    good = [m for m in GOOD_FP]
    cases = [
        ("good episodes", good, "PASS"),
        ("pre-M3b DLL (no FPCAM cfg)", [], "N/A"),
        ("first person disabled in the config", [("FPCAM", GOOD_FP[0][1].replace("enabled=1", "enabled=0"))], "N/A"),
        ("cfg but never on", good[:1], "FAIL"),
        ("world left with no loading handback", good[:-1], "FAIL"),
        ("world left, then a non-loading reason", good[:-1] + [("FP", "off reason=f6  ticks_on=3 cam_writes=3")], "FAIL"),
        ("er_rewrite=0 warning", good + [("FPCAM", "WARN er_rewrite=0: pers_cam_1 still holds our write")], "FAIL"),
        ("no er_rewrite=1 proof in task mode", [m for m in good if not m[1].startswith("er_rewrite=1")], "FAIL"),
        ("no hide write", [m for m in good if not m[1].startswith("hide write")], "FAIL"),
        ("unknown handback reason", good[:7] + [("FP", "off reason=bogus  ticks_on=1 cam_writes=1")], "FAIL"),
        ("eye fed to the camera gate", good[:3] + [("CAMFRAME", "tick=99 src=pers_cam_1 valid=0 why=dist pos=(0,0,0)")]
         + good[3:], "FAIL"),
        ("hook mode without FPHOOK installed", [(t, m.replace("mode=task", "mode=hook")) for t, m in good], "FAIL"),
        ("hook mode with FPHOOK installed", [("FPHOOK", "installed rva=0x3bc5f0 addr=0x1 prologue=ok")]
         + [(t, m.replace("mode=task", "mode=hook")) for t, m in good], "PASS"),
    ]
    for label, lines_, want in cases:
        got = status_of(run(synthetic(True, fp=lines_)), "first person")
        assert got == want, f"first-person case {label!r}: {got}, want {want}"
    return cases


# The real M3 run-3 session (logs/mw2er.log lines 725-1600, copied verbatim: our own DLL log, no game data). It has
# seven c4311 dmg=4 hits at atkPhys 160 between normal 49s (shield blocks) and the first close-range measurement.
RUN3_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m3_run3_session.log"
RUN3_SHA256 = "27a9c19d4699001bacaeb2d36638a665c98a11ad61e16a8d4e26a8185caf7604"


def real_run3_self_test():
    import hashlib
    global BLOCK_RATIO
    raw = RUN3_FIXTURE.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == RUN3_SHA256, f"{RUN3_FIXTURE.name} changed (sha256)"
    ctx = load_context()
    assert not ctx["missing"], f"real-log self-test needs the project data files: {ctx['missing']}"
    entries = sessions(parse(raw.decode("utf-8")))[-1]
    res = analyze(entries, ctx)[0]
    st, detail = next((s_, d) for s_, n, d in res if n == "hits")
    assert st == "PASS" and "7 blocked?" in detail, f"run 3 hits: {st} {detail[:200]}"
    assert status_of(res, "first person") == "N/A", "run 3 predates M3b"
    assert [status_of(res, r) for r in AC_ROWS] == ["N/A"] * len(AC_ROWS), "run 3 predates M4"
    saved = BLOCK_RATIO
    try:
        BLOCK_RATIO = 0.0  # without the blocked rule the same log FAILs exactly as reported in run 3
        st0, d0 = next((s_, d) for s_, n, d in analyze(entries, ctx)[0] if n == "hits")
    finally:
        BLOCK_RATIO = saved
    assert st0 == "FAIL" and "dmg=4" in d0, f"run 3 without the blocked rule: {st0} {d0[:120]}"
    # Honesty (review M3b minor 11): band 79000100's baseline was MEASURED from this same run, so the PASS half is a
    # calibration (self-consistency) check, not an independent validation; only "FAIL without the rule" is
    # independent. A later run is the validation.
    return ("hits PASS with 7 blocked? (calibration, not validation: band 79000100's baseline comes from this same "
            "run; independent half: FAIL without the rule), first person N/A")


# M6: the synthetic arsenal session (every M6 row PASS) and the real run-13 bug excerpt (logs/mw2er.log of 2026-10-17:
# the session's BOOT line 9040, world entered 9157, lines 11079-11280 (SPAWN FAIL code=4 bursts without a cause),
# world left/entered 11456/11476 and lines 12021-12110 (AC130 off reason=loading t_s=0.0 in the world); our own DLL
# log, no game data).
M6_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m6_arsenal_session.log"
RUN13_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "run13_bugs.log"
RUN13_LINES = [(9040, 9040), (9157, 9157), (11079, 11280), (11456, 11456), (11476, 11476), (12021, 12110)]


def real_run13_self_test():
    raw = RUN13_FIXTURE.read_text(encoding="utf-8")
    res = analyze(sessions(parse(raw))[-1], load_context())[0]
    got = {n: s_ for s_, n, _ in res}
    want = {"ac130 start": "FAIL", "spawn": "NEEDS-DATA", "arsenal": "N/A", "switching": "N/A"}
    bad = {k: (got.get(k), v) for k, v in want.items() if got.get(k) != v}
    assert not bad, f"run 13 rows (got, want): {bad}"
    ac = next(d for s_, n, d in res if n == "ac130 start")
    sp = next(d for s_, n, d in res if n == "spawn")
    m = re.match(r"(\d+) 'AC130 off", ac)
    n_sp = re.match(r"(\d+) SPAWN FAIL", sp)
    assert m and int(m.group(1)) >= 3 and n_sp and int(n_sp.group(1)) >= 10, f"run 13: {ac[:80]} | {sp[:80]}"
    return (f"real run-13 excerpt: ac130 start FAIL ({m.group(1)} stale-load aborts in the world), spawn NEEDS-DATA "
            f"({n_sp.group(1)} SPAWN FAIL without a cause)")


# M7: the synthetic nuke session (every M7 row PASS) and the real run-14 excerpt (logs/mw2er.log, the session starting
# 2026-10-17 23:18:28: the boot / arsenal / spawn / AC-130 / reload / rechamber / switch / shot / hit lines; our own DLL
# log, no game data): the emitter leak (51 code=4, the AC-130 105 mm without damage) and the reload restarted during
# switch drops.
M7_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m7_nuke_session.log"
RUN14_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "run14_leak.log"
RUN14_SHA256 = "2f0cdac1f3af72210fde692f074cf22c4dac7031d0e942fa769700e0ab6d5b18"


def m7_fixture_self_test():
    raw = M7_FIXTURE.read_text(encoding="utf-8")
    entries = sessions(parse(raw))[-1]
    got = {n: s_ for n, s_, _ in analyze_m7.rows(entries)}
    bad = {k: v for k, v in got.items() if v != "PASS"}
    assert not bad, f"M7 fixture rows not PASS: {bad}"
    return f"good session PASS on all {len(got)} M7 rows"


def real_run14_self_test():
    import hashlib
    raw = RUN14_FIXTURE.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == RUN14_SHA256, f"{RUN14_FIXTURE.name} changed (sha256)"
    res = analyze(sessions(parse(raw.decode("utf-8")))[-1], load_context())[0]
    got = {n: s_ for s_, n, _ in res}
    want = {"emitter pool": "FAIL", "spawn causes": "FAIL", "ac130 damage": "FAIL", "reload pairing": "FAIL",
            "nuke streak": "N/A", "nuke sequence": "N/A", "viewmodel": "FAIL", "shots per gun": "FAIL"}
    bad = {k: (got.get(k), v) for k, v in want.items() if got.get(k) != v}
    assert not bad, f"run 14 rows (got, want): {bad}"
    vm = next(d for s_, n, d in res if n == "viewmodel")
    assert "no 'VM assets ready'" not in vm, f"run 14 viewmodel still wants 'VM assets ready': {vm[:120]}"
    em = next(d for s_, n, d in res if n == "emitter pool")
    return f"real run-14 excerpt: emitter pool / spawn causes / ac130 damage / reload pairing FAIL as reported ({em[:70]}...), nuke rows N/A, viewmodel judged on ARSENAL loaded"


# M8: the synthetic mantle / ladder sessions (every M8 row PASS) and the real run-17 session (logs/mw2er.log, the session
# booted 2026-10-18 11:53:33, lines 25647-27907, copied verbatim: our own DLL log, no game data). Run 17 predates M8:
# every chunk re-base was logged as a warp (rebase FAIL as reported), the mantle / ladder rows are N/A, and with the P2
# fixes its first-person and jump rows PASS.
M8_MANTLE_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m8_mantle_session.log"
M8_LADDER_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m8_ladder_session.log"
# M8 review: a session whose every mantle check was refused (no start): the 'mantle' row must FAIL, not NEEDS-DATA
M8_REFUSE_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m8_refuse_all_session.log"
RUN17_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "run17_excerpt.log"
RUN17_LINES = (25647, 27907)
RUN17_PIN = pathlib.Path(__file__).resolve().parent / "fixtures" / "run17_excerpt.sha256"


def m8_fixture_self_test():
    out = []
    for p, want in ((M8_MANTLE_FIXTURE, analyze_m8.MANTLE_ROWS), (M8_LADDER_FIXTURE, analyze_m8.LADDER_ROWS)):
        assert p.exists(), f"{p} missing (python -I analyze_log.py --write-m8-fixtures)"
        got = {n: s_ for n, s_, _ in analyze_m8.rows(sessions(parse(p.read_text(encoding="utf-8")))[-1])}
        bad = {k: got[k] for k in want if got[k] != "PASS"}
        assert not bad, f"M8 fixture {p.name} rows not PASS: {bad}"
        out.append(f"{p.name} {len(want)}/{len(want)} PASS")
    assert M8_REFUSE_FIXTURE.exists(), f"{M8_REFUSE_FIXTURE} missing (python -I analyze_log.py --write-m8-fixtures)"
    got = {n: (s_, d) for n, s_, d in analyze_m8.rows(sessions(parse(M8_REFUSE_FIXTURE.read_text(encoding="utf-8")))[-1])}
    assert got["mantle"][0] == "FAIL" and "refuses everything" in got["mantle"][1], f"{M8_REFUSE_FIXTURE.name}: {got['mantle']}"
    out.append(f"{M8_REFUSE_FIXTURE.name} mantle FAIL (detection refuses everything)")
    return ", ".join(out)


def real_run17_self_test():
    import hashlib
    raw = RUN17_FIXTURE.read_bytes()
    pin = RUN17_PIN.read_text(encoding="utf-8").split()[0] if RUN17_PIN.exists() else ""
    assert hashlib.sha256(raw).hexdigest() == pin, f"{RUN17_FIXTURE.name} changed (sha256, pin {RUN17_PIN.name})"
    res = analyze(sessions(parse(raw.decode("utf-8")))[-1], load_context())[0]
    got = {n: s_ for s_, n, _ in res}
    want = {"rebase": "FAIL", "mantle": "N/A", "mantle perf": "N/A", "ladder handback": "N/A", "ladder dismount": "N/A",
            "ladder weapon": "N/A", "first person": "PASS", "jump": "PASS", "movement": "PASS"}
    bad = {k: (got.get(k), v) for k, v in want.items() if got.get(k) != v}
    assert not bad, f"run 17 rows (got, want): {bad}"
    rb = next(d for s_, n, d in res if n == "rebase")
    m = re.match(r"(\d+) problem", rb)
    assert m and int(m.group(1)) >= 10, f"run 17 rebase: {rb[:100]}"
    return (f"real run-17 excerpt: rebase FAIL ({m.group(1)} chunk re-bases logged as warps, as found), first person and jump "
            f"PASS with the P2 fixes, mantle / ladder rows N/A (pre-M8)")


RUN18_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "run18_excerpt.log"
RUN18_PIN = pathlib.Path(__file__).resolve().parent / "fixtures" / "run18_excerpt.sha256"


def real_run18_self_test():
    """The real run-18 session (M8's first in-game test, the DLL before the run-18 fixes): the close ER camera turned
    first person off (camera close FAIL, as found), every mantle check was refused (mantle FAIL, as found), the run-18
    rows that need the new DLL fields are N/A."""
    import hashlib
    raw = RUN18_FIXTURE.read_bytes()
    pin = RUN18_PIN.read_text(encoding="utf-8").split()[0] if RUN18_PIN.exists() else ""
    assert hashlib.sha256(raw).hexdigest() == pin, f"{RUN18_FIXTURE.name} changed (sha256, pin {RUN18_PIN.name})"
    res = analyze(sessions(parse(raw.decode("utf-8")))[-1], load_context())[0]
    got = {n: (s_, d) for s_, n, d in res}
    want = {"camera close": "FAIL", "mantle": "FAIL", "ray filters": "N/A", "unstick": "N/A"}
    bad = {k: (got.get(k, (None,))[0], v) for k, v in want.items() if got.get(k, (None,))[0] != v}
    assert not bad, f"run 18 rows (got, want): {bad}"
    m = re.match(r"(\d+) close-camera", got["camera close"][1])
    assert m and int(m.group(1)) >= 10, f"run 18 camera close: {got['camera close'][1][:100]}"
    return (f"real run-18 excerpt: camera close FAIL ({m.group(1)} close-camera handbacks, as found), mantle FAIL (every check "
            f"refused, as found), ray filters / unstick N/A (pre-fix DLL)")


def pick_session(ss, number):
    """(0-based index, error) for a 1-based session `number` (None = the last one)."""
    if not ss:
        return None, "no sessions"
    if number is None:
        return len(ss) - 1, None
    if not 1 <= number <= len(ss):
        return None, f"--session {number} is out of range: the log has {len(ss)} session(s), numbered 1..{len(ss)}"
    return number - 1, None


def main(argv):
    if argv == ["--write-m5-fixture"]:
        MOVE_FIXTURE.write_text(synthetic(True, ac=move_session()), encoding="utf-8", newline="\n")
        print(f"wrote {MOVE_FIXTURE}")
        return 0
    if argv == ["--write-m6-fixture"]:
        M6_FIXTURE.write_text(synthetic(True, ac=ac130_ride() + analyze_m6.m6_session()), encoding="utf-8", newline="\n")
        print(f"wrote {M6_FIXTURE}")
        return 0
    if argv == ["--write-run13-fixture"]:
        src = (ROOT / "logs" / "mw2er.log").read_text(encoding="utf-8", errors="replace").splitlines()
        out = [ln for a, b in RUN13_LINES for ln in src[a - 1:b]]
        RUN13_FIXTURE.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
        print(f"wrote {RUN13_FIXTURE} ({len(out)} lines)")
        return 0
    if argv == ["--write-m7-fixture"]:
        M7_FIXTURE.write_text("2026-10-18 12:00:00.000 I [boot] BOOT mw2er 0.3.0\n" + analyze_m7.to_log(analyze_m7.m7_session()), encoding="utf-8", newline="\n")
        print(f"wrote {M7_FIXTURE}")
        return 0
    if argv == ["--write-m8-fixtures"]:
        M8_MANTLE_FIXTURE.write_text("2026-10-18 13:00:00.000 I [boot] BOOT mw2er 0.3.0\n" + analyze_m8.to_log(analyze_m8.m8_mantle_session()), encoding="utf-8", newline="\n")
        M8_LADDER_FIXTURE.write_text("2026-10-18 13:00:00.000 I [boot] BOOT mw2er 0.3.0\n" + analyze_m8.to_log(analyze_m8.m8_ladder_session()), encoding="utf-8", newline="\n")
        M8_REFUSE_FIXTURE.write_text("2026-10-18 13:00:00.000 I [boot] BOOT mw2er 0.3.0\n" + analyze_m8.to_log(analyze_m8.m8_mantle_session(refuse_all=True)), encoding="utf-8", newline="\n")
        print(f"wrote {M8_MANTLE_FIXTURE}, {M8_LADDER_FIXTURE} and {M8_REFUSE_FIXTURE}")
        return 0
    if argv == ["--write-run17-fixture"]:
        import hashlib
        src = (ROOT / "logs" / "mw2er.log").read_text(encoding="utf-8", errors="replace").splitlines()
        a, b = RUN17_LINES
        out = src[a - 1:b]
        if not out or "BOOT mw2er" not in out[0]:
            print(f"FAIL logs/mw2er.log line {a} is not the run-17 BOOT line")
            return 1
        RUN17_FIXTURE.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
        RUN17_PIN.write_text(hashlib.sha256(RUN17_FIXTURE.read_bytes()).hexdigest() + "  run17_excerpt.log\n", encoding="utf-8")
        print(f"wrote {RUN17_FIXTURE} ({len(out)} lines) and its sha256 pin")
        return 0
    if argv == ["--write-m4-fixture"]:
        AC130_FIXTURE.write_text(synthetic(True, ac=ac130_ride()), encoding="utf-8", newline="\n")
        print(f"wrote {AC130_FIXTURE}")
        return 0
    if argv == ["--self-test"]:
        try:
            return self_test()
        except AssertionError as e:
            print(f"FAIL self-test: {e}")
            return 1
    path, which = ROOT / "logs" / "mw2er.log", None
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--session":
            try:
                which = int(args.pop(0))
            except (IndexError, ValueError):
                print("FAIL --session needs a 1-based session number")
                return 2
        elif a.startswith("-"):
            print(__doc__)
            return 2
        else:
            path = pathlib.Path(a)
    if not path.is_file():
        print(f"FAIL no log at {path} (run the game through launch.ps1 first)")
        return 1
    ss = sessions(parse(path.read_text(encoding="utf-8", errors="replace")))
    if not ss:
        print(f"FAIL {path} has no 'BOOT mw2er' line")
        return 1
    ctx = load_context()
    for m in ctx["missing"]:
        print(f"WARN cannot read {m}")
    idx, err = pick_session(ss, which)
    if err:
        print(f"FAIL {err}")
        return 2
    print(f"{path}: {len(ss)} session(s); analysing session {idx + 1} of {len(ss)}")
    res, info = analyze(ss[idx], ctx)
    return 0 if report(res, info) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
