"""M7 rows of analyze_log.py (docs/M7_DESIGN.md section 5): the run-14 fixes (the bullet-emitter leak behind code 4 and
the AC-130's lost damage, reload / rechamber pairing) and the MW2 Tactical Nuke (streak, sequence, kills, the player's
death, the post pass, cancels). Imported by analyze_log.py; pure Python, read-only.

Rows that need an M7 DLL are N/A for older logs, except the A1 rows, which also judge an M6 log by its `SPAWN pool`
lines (the run-14 excerpt fixture FAILs them, as reported).
"""
import re

TICK = 1.0 / 60.0
EMITTER_WARN = 192
NUKE_TOL_S = 0.1
PLAYER_DEAD_MAX_MS = 2000.0
POST_GPU_MAX_MS = 1.0
CENSUS_MAX_US = 50000  # one census pass (main thread) must stay under 50 ms

M7_ROWS = ("emitter pool", "spawn causes", "ac130 damage", "reload pairing", "nuke streak", "nuke sequence", "nuke kills",
           "nuke player", "nuke post", "nuke cancel")


def msgs(entries, tag, pat):
    r = re.compile(pat)
    return [(e, m) for e in entries if e["tag"] == tag and (m := r.search(e["msg"]))]


def nuke_present(entries):
    return any(e["tag"] == "NUKE" and e["msg"].startswith("cfg ") for e in entries)


def arsenal_present(entries):
    return any(e["tag"] == "ARSENAL" and (e["msg"].startswith("data ") or e["msg"].startswith("loaded ")) for e in entries)


# ---- A1 ------------------------------------------------------------------------------------------------------------

def judge_emitter_pool(entries):
    em = msgs(entries, "SPAWN", r"^emitters live=(\d+) ours=(\d+) persistent=(\d+) max=(\d+)")
    pools = msgs(entries, "SPAWN", r"^pool bullets=\d+/\d+ sfx=(\d+)/(\d+) sfx_empty_spot=(\d)")
    warn_p = msgs(entries, "SPAWN", r"^WARN persistent=(\d+)")
    warn_e = msgs(entries, "SPAWN", r"^WARN emitters live=(\d+)")
    if not em and not pools:
        return "N/A", "no 'SPAWN emitters' / 'SPAWN pool' line (DLL before M6, or no shot fired)"
    heap = [int(m.group(1)) for _, m in pools]
    cap = max((int(m.group(2)) for _, m in pools), default=192)
    if not em:
        # an M6 DLL: the emitter (sfx) heap fills one per call and never shrinks before a reload (the leak)
        grew = len(heap) >= 2 and heap[-1] > heap[0] and all(b >= a for a, b in zip(heap, heap[1:]))
        if heap and (max(heap) >= cap or grew):
            return "FAIL", (f"no 'SPAWN emitters' line (pre-M7 DLL); the emitter heap grew {heap[0]} -> {max(heap)}/{cap} over "
                            f"{len(heap)} SPAWN pool lines and never shrank (the run-14 leak: auto-finish bit 0)")
        return "N/A", f"no 'SPAWN emitters' line (pre-M7 DLL) and no leak in the emitter heap {heap}"
    persistent = [int(m.group(3)) for _, m in em]
    mx = max(int(m.group(4)) for _, m in em)
    problems = []
    if any(persistent) or warn_p:
        problems.append(f"persistent emitters {max(persistent)} (the auto-finish bit is not applied: regression)")
    if mx >= EMITTER_WARN or warn_e:
        problems.append(f"emitters max {mx} (>= {EMITTER_WARN}: the pool is filling)")
    if heap and max(heap) >= cap:
        problems.append(f"emitter heap reached {max(heap)}/{cap}")
    if problems:
        return "FAIL", "; ".join(problems)
    return "PASS", (f"{len(em)} 'SPAWN emitters' line(s): persistent=0 everywhere, max {mx} live (< {EMITTER_WARN}); emitter "
                    f"heap {'n/a' if not heap else f'max {max(heap)}/{cap}'}")


def spawn_fail_lines(entries):
    out = []
    for e in entries:
        if e["tag"] == "SPAWN" and e["msg"].startswith("FAIL "):
            out.append(e)
        elif e["tag"] in ("AC130", "PROJ", "NUKE") and "SPAWN FAIL" in e["msg"]:
            out.append(e)
    return out


def judge_spawn_causes(entries):
    fails = spawn_fail_lines(entries)
    m7 = any(e["tag"] == "SPAWN" and e["msg"].startswith("emitters ") for e in entries)
    shots = sum(1 for e in entries if e["tag"] == "SHOT" and re.match(r"^n=\d+ mag=", e["msg"]))
    if not fails:
        if not m7 and shots == 0:
            return "N/A", "no shot and no SPAWN line"
        return "PASS", f"0 SPAWN FAIL over {shots} SHOT line(s)"
    no_cause = [e for e in fails if "cause=" not in e["msg"]]
    code4 = [e for e in fails if "code=4" in e["msg"]]
    causes = {}
    for e in fails:
        m = re.search(r"\bcause=(\w+)", e["msg"])
        if m:
            causes[m.group(1)] = causes.get(m.group(1), 0) + 1
    detail = f"{len(fails)} SPAWN FAIL ({len(code4)} code=4): causes {dict(sorted(causes.items()))}, {len(no_cause)} without a cause"
    if no_cause:
        return "FAIL", detail + f"; e.g. {no_cause[0]['tag']} {no_cause[0]['msg'][:80]}"
    if code4:
        return "FAIL", detail + (" in a session with the auto-finish fix (send the log)" if m7 else " (pre-M7 DLL: the emitter leak)")
    return "FAIL", detail


def judge_ac130_damage(entries):
    expl = msgs(entries, "AC130", r"^EXPLODE gun=(\w+) ids=(\S*) ")
    if not expl:
        return "N/A", "no 'AC130 EXPLODE' line (no ride with a shot)"
    empty = [e for e, m in expl if not m.group(2)]
    fails = [e for e in entries if e["tag"] == "AC130" and "SPAWN FAIL" in e["msg"]]
    hits = [m for _, m in msgs(entries, "HIT", r"\bdmg=(\d+)\b.*\bsrc=ac130\b") if int(m.group(1)) > 0]
    kills = msgs(entries, "AC130", r"^KILL ")
    detail = (f"{len(expl)} EXPLODE line(s), {len(empty)} with no row spawned, {len(fails)} AC-130 SPAWN FAIL; "
              f"{len(hits)} HIT src=ac130 with dmg > 0, {len(kills)} AC130 KILL")
    if empty or fails:
        return "FAIL", detail + " (the explosions did not spawn: no damage)"
    if not hits:
        return "NEEDS-DATA", detail + " (fire the 105 / 40 mm at an enemy)"
    return "PASS", detail


# ---- A3 ------------------------------------------------------------------------------------------------------------

def judge_reload_pairing(entries):
    if not arsenal_present(entries):
        return "N/A", "no 'ARSENAL data' line (DLL before M6)"
    rl = [e for e in entries if e["tag"] in ("RELOAD", "RECHAMBER", "SWITCH")]
    problems = []
    open_r, open_c = None, None
    n_r = n_c = 0
    last_switch = -1e9
    for e in rl:
        m = e["msg"]
        if e["tag"] == "SWITCH" and m.startswith("start "):
            last_switch = e["t"]
            continue
        if e["tag"] == "RELOAD":
            if m.startswith("start "):
                n_r += 1
                if abs(e["t"] - last_switch) < 0.02:
                    problems.append(f"{e['raw'][11:23]} RELOAD start in the switch's own tick (a reload restarted during the drop)")
                if open_r is not None:
                    problems.append(f"{e['raw'][11:23]} RELOAD start while one is open (started {open_r['raw'][11:23]})")
                open_r = e
            elif m.startswith("done ") or m.startswith("cancelled"):
                open_r = None
        elif e["tag"] == "RECHAMBER":
            if m.startswith("start "):
                n_c += 1
                if open_c is not None:
                    problems.append(f"{e['raw'][11:23]} RECHAMBER start while one is open")
                open_c = e
            elif m.startswith("done ") or m.startswith("cancelled "):
                open_c = None
    end_t = entries[-1]["t"] if entries else 0.0
    if open_r is not None and (end_t - open_r["t"]) > 6.0:
        problems.append(f"RELOAD start at {open_r['raw'][11:23]} never done / cancelled")
    if open_c is not None and (end_t - open_c["t"]) > 3.0:
        problems.append(f"RECHAMBER start at {open_c['raw'][11:23]} never done / cancelled")
    if not n_r and not n_c:
        return "NEEDS-DATA", "no RELOAD / RECHAMBER start"
    if problems:
        return "FAIL", f"{len(problems)} problem(s): " + "; ".join(problems[:3])
    return "PASS", f"{n_r} RELOAD and {n_c} RECHAMBER start(s), each ends in done or cancelled; none restarted during a drop"


# ---- the nuke ------------------------------------------------------------------------------------------------------

def na_nuke(entries):
    return None if nuke_present(entries) else "no 'NUKE cfg' line (DLL before M7 or [nuke] enabled = false)"


def judge_nuke_streak(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    nk = [e for e in entries if e["tag"] == "NUKE"]
    cost = None
    for e in nk:
        m = re.search(r"\bcost=(\d+)", e["msg"])
        if e["msg"].startswith("cfg ") and m:
            cost = int(m.group(1))
    problems, n, counted, earned, resets = [], 0, 0, 0, 0
    earned_life = False
    for e in nk:
        m = e["msg"]
        if (mm := re.match(r"^streak n=(\d+)/(\d+) src=(\w+)", m)):
            counted += 1
            got = int(mm.group(1))
            if got != n + 1:
                problems.append(f"{e['raw'][11:23]} streak {n} -> {got} (want +1)")
            n = got
        elif (mm := re.match(r"^streak reset reason=death n=(\d+)", m)):
            resets += 1
            if int(mm.group(1)) != n:
                problems.append(f"{e['raw'][11:23]} reset n={mm.group(1)} but the streak was {n}")
            n = 0
            earned_life = False
        elif (mm := re.match(r"^earned streak=(\d+)", m)):
            earned += 1
            if cost is not None and int(mm.group(1)) != cost:
                problems.append(f"earned at {mm.group(1)} (cost {cost})")
            if earned_life:
                problems.append("earned twice in one life")
            earned_life = True
        elif m.startswith("end reason="):
            n = 0
    if not counted and not earned:
        return "NEEDS-DATA", "no 'NUKE streak' line (kill enemies with MW2 weapons)"
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", f"{counted} streak count(s) (+1 per kill), {resets} reset(s) on death, earned {earned}x at the cost {cost}"


def sequences(entries):
    """[(activate entry, [the NUKE entries until the end / cancel])]."""
    out, cur = [], None
    for e in entries:
        if e["tag"] != "NUKE":
            continue
        if e["msg"].startswith("activate "):
            cur = (e, [])
            out.append(cur)
        elif cur is not None:
            cur[1].append(e)
            if e["msg"].startswith("end reason=") or e["msg"].startswith("cancel reason="):
                cur = None
    return out


def judge_nuke_sequence(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    seqs = sequences(entries)
    if not seqs:
        return "NEEDS-DATA", "no 'NUKE activate' line (press the nuke key with the nuke earned or nuke.always_available)"
    problems, done = [], 0
    for a, rest in seqs:
        t0 = a["t"]
        first = lambda p: next((e for e in rest if e["msg"].startswith(p)), None)  # noqa: E731
        cancel = first("cancel reason=")
        det = first("detonate ")
        if cancel is not None and det is None:
            continue
        inc, census = first("incoming "), first("census pass=0 ")
        stuck = first("WARN stuck")
        if det is None or inc is None:
            problems.append(f"{a['raw'][11:23]}: incoming {inc is not None} detonate {det is not None}")
            continue
        if abs(inc["t"] - t0 - 6.7) > NUKE_TOL_S:
            problems.append(f"incoming at {inc['t'] - t0:.2f} s (want 6.7)")
        if abs(det["t"] - t0 - 10.0) > NUKE_TOL_S:
            problems.append(f"detonate at {det['t'] - t0:.2f} s (want 10.0)")
        if census is None or abs(census["t"] - t0 - 11.5) > NUKE_TOL_S:
            problems.append(f"kills at {'none' if census is None else f'{census['t'] - t0:.2f} s'} (want 11.5)")
        if stuck is not None:
            problems.append("WARN stuck")
        done += 1
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", f"{len(seqs)} sequence(s), {done} detonated: incoming T-3.3 s, detonate 10.0 s, kills T+1.5 s (+-{NUKE_TOL_S} s)"


def spare_teams_of(entries):
    """(spare_friendly, spare_teams) from the last NUKE cfg line (None when not logged)."""
    cfg = [e for e in entries if e["tag"] == "NUKE" and e["msg"].startswith("cfg ")]
    if not cfg:
        return None
    m = re.search(r"spare_friendly=(\d) spare_teams=\[([\d, ]*)\]", cfg[-1]["msg"])
    if not m:
        return None
    return m.group(1) == "1", {int(x) for x in m.group(2).replace(" ", "").split(",") if x}


def judge_nuke_kills(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    sums = msgs(entries, "NUKE", r"^kills total=(\d+) killed=(\d+) survivors=(\d+) skipped=(\d+) in_radius=(\d+)(?: would_apply=(\d+) armed=(\S+))?")
    if not sums:
        return "NEEDS-DATA", "no 'NUKE kills' summary (a detonation with characters near)"
    problems = []
    for _, m in sums:
        killed, surv, skipped, inr = int(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5))
        would = int(m.group(6) or 0)
        if killed + surv + skipped + would != inr:
            problems.append(f"killed {killed} + survivors {surv} + skipped {skipped} + would-apply {would} != in_radius {inr}")
    kills = msgs(entries, "NUKE", r"^kill handle=\S+ .* hp=(-?\d+)->(-?\d+) ")
    bad = [m for _, m in kills if int(m.group(2)) != 0]
    none_reason = msgs(entries, "NUKE", r"^skip reason=none")
    if bad:
        problems.append(f"{len(bad)} 'NUKE kill' line(s) not ending at hp 0")
    if none_reason:
        problems.append("a skip with reason none")
    # run-15 safety: the team byte is [I]; an implausible histogram blocks the kills (the offset is wrong: FAIL)
    tb = msgs(entries, "NUKE", r"^blocked reason=team_unverified (.*)")
    if tb:
        problems.append(f"NPC kills blocked: team_unverified {tb[0][1].group(1)[:80]} (ChrIns team offset / mapping wrong?)")
    # nobody on a spared team may ever be applied to, or listed as would-apply (with spare_friendly)
    st = spare_teams_of(entries)
    if st and st[0]:
        hit = [(e, m) for e, m in msgs(entries, "NUKE", r"^kill (?:apply|would-apply) handle=\S+ npc=(\d+) chr=(c\d+) team=(\d+)")
               if int(m.group(3)) in st[1]]
        if hit:
            problems.append(f"{len(hit)} lethal/would-apply line(s) on a spared team (first: npc {hit[0][1].group(1)} "
                            f"{hit[0][1].group(2)} team {hit[0][1].group(3)})")
    cus = [int(m.group(1)) for _, m in msgs(entries, "NUKE", r"^census pass=\d+ .* census_us=(\d+)")]
    if cus and max(cus) > CENSUS_MAX_US:
        problems.append(f"census took {max(cus)} us (> {CENSUS_MAX_US}: a main-thread hitch)")
    if problems:
        return "FAIL", "; ".join(problems[:3])
    last = sums[-1][1]
    surv = msgs(entries, "NUKE", r"^survivor ")
    of = msgs(entries, "NUKE", r"^skip reason=other_frame n=(\d+)")
    of_txt = f", other havok frame {of[-1][1].group(1)} (counted, not killed)" if of else ""
    armed = [m for _, m in sums if m.group(7) == "1" or m.group(7) is None]
    if not armed:
        would = msgs(entries, "NUKE", r"^kill would-apply ")
        teams = sorted({m.group(1) for _, m in msgs(entries, "NUKE", r"^kill would-apply \S+ npc=\d+ chr=c\d+ team=(\d+)")})
        return "NEEDS-DATA", (f"dry run only (nuke.dry_run = true): {len(would)} would-apply line(s), teams {teams}, nobody on a spared "
                              f"team{of_txt}; check the would-apply / skip lines (Kale and merchants spared?), then set "
                              f"nuke.dry_run = false to verify real kills")
    return "PASS", (f"{len(sums)} kill summary(ies): killed {last.group(2)} + survivors {last.group(3)} + skipped {last.group(4)} "
                    f"== in_radius {last.group(5)}; {len(kills)} confirmed kill line(s), {len(surv)} survivor line(s); "
                    f"census max {max(cus) if cus else '?'} us{of_txt}")


def judge_nuke_player(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    if any(e["tag"] == "NUKE" and e["msg"].startswith("cfg ") and " player_dies=0" in e["msg"] for e in entries):
        return "N/A", "nuke.player_dies = false"
    prep = msgs(entries, "NUKE", r"^player prep ")
    lethal = msgs(entries, "NUKE", r"^player lethal ok=(\d) try=(\d+) id=\d+ protect_present=(\d)")
    dead = msgs(entries, "NUKE", r"^player dead t_ms=(\d+)")
    warns = msgs(entries, "NUKE", r"^WARN player_")
    if not prep and not lethal:
        return "NEEDS-DATA", "no 'NUKE player' line (let a sequence reach T+4.5 s)"
    problems = []
    if warns:
        problems.append(warns[0][0]["msg"][:80])
    for e, m in lethal:
        if m.group(3) != "0":
            problems.append("lethal applied with the protection present")
        if not any(p["t"] <= e["t"] for p, _ in prep):
            problems.append("lethal without the prep before it")
        if m.group(1) != "1":
            problems.append("the lethal call was refused (gate)")
    if lethal and not dead:
        problems.append("no 'NUKE player dead' after the lethal effect")
    for _, m in dead:
        if float(m.group(1)) > PLAYER_DEAD_MAX_MS:
            problems.append(f"player dead {m.group(1)} ms after the lethal (> {PLAYER_DEAD_MAX_MS:.0f})")
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", f"prep x{len(prep)} before the lethal x{len(lethal)} (protection absent), dead after {dead[0][1].group(1) if dead else '?'} ms"


def judge_nuke_post(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    fail = msgs(entries, "NUKE", r"^FAIL post ")
    if fail:
        return "FAIL", fail[0][0]["msg"][:120]
    gpu = [float(m.group(1)) for _, m in msgs(entries, "NUKE", r"^post gpu_ms=([\d.]+) \(max")]
    if not gpu:
        return "NEEDS-DATA", "no 'NUKE post gpu_ms' line (a detonation drawn on screen)"
    if max(gpu) >= POST_GPU_MAX_MS:
        return "FAIL", f"post pass gpu max {max(gpu):.3f} ms >= {POST_GPU_MAX_MS} ms"
    return "PASS", f"{len(gpu)} sequence(s) drawn, post pass gpu max {max(gpu):.4f} ms (< {POST_GPU_MAX_MS} ms)"


def judge_nuke_cancel(entries):
    na = na_nuke(entries)
    if na:
        return "N/A", na
    cancels = msgs(entries, "NUKE", r"^cancel reason=(\w+) refund=(\d)")
    ends = msgs(entries, "NUKE", r"^end reason=(\w+)")
    bare = [e for e in entries if e["tag"] == "NUKE" and (e["msg"].startswith("cancel ") or e["msg"].startswith("end ")) and "reason=" not in e["msg"]]
    seqs = sequences(entries)
    if not seqs:
        return "NEEDS-DATA", "no nuke sequence"
    problems = []
    if bare:
        problems.append("a cancel / end without a reason")
    for a, rest in seqs:
        test = " test=1" in a["msg"]
        for e in rest:
            m = re.match(r"^cancel reason=(\w+) refund=(\d)", e["msg"])
            if m and m.group(1) == "loading" and m.group(2) != ("0" if test else "1"):
                problems.append(f"loading cancel refund={m.group(2)} (want {'0 (test)' if test else '1'})")
    blocked = msgs(entries, "MODE", r"^toggle blocked reason=nuke") + msgs(entries, "AC130", r"^blocked reason=nuke")
    if problems:
        return "FAIL", "; ".join(problems[:3])
    return "PASS", (f"{len(cancels)} cancel(s) {[m.group(1) for _, m in cancels]}, {len(ends)} end(s) "
                    f"{[m.group(1) for _, m in ends]}, every one with a reason; {len(blocked)} F5/F7 refusal(s) during a sequence")


def rows(entries, ctx=None):
    return [
        ("emitter pool", *judge_emitter_pool(entries)),
        ("spawn causes", *judge_spawn_causes(entries)),
        ("ac130 damage", *judge_ac130_damage(entries)),
        ("reload pairing", *judge_reload_pairing(entries)),
        ("nuke streak", *judge_nuke_streak(entries)),
        ("nuke sequence", *judge_nuke_sequence(entries)),
        ("nuke kills", *judge_nuke_kills(entries)),
        ("nuke player", *judge_nuke_player(entries)),
        ("nuke post", *judge_nuke_post(entries)),
        ("nuke cancel", *judge_nuke_cancel(entries)),
    ]


# ---- the synthetic M7 session (self-test and fixture) -------------------------------------------------------------

def m7_session(streak_skip=False, earned_twice=False, incoming_at=6.7, detonate_at=10.0, kill_at=11.5, bad_sum=False,
               lethal_protected=False, no_prep=False, player_dead_ms=900, post_gpu=0.07, persistent=0, emitters_max=31,
               code4=False, fail_no_cause=False, reload_restart=False, rechamber_open=False, cancel_no_refund=False,
               stuck=False, ac_fail=False, dry_run=False, would_spared=False, team_block=False, census_us=1800):
    """A good M7 session as (t, tag, msg) lines; each keyword breaks one rule."""
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))

    a(0.0, "BOOT", "mw2er 0.3.0 dll=D:\\Mods\\MW2inER\\build\\natives\\mw2er.dll")
    a(0.1, "ARSENAL", "data n=34 ok=34 disabled=0")
    a(0.2, "NUKE", "cfg enabled=1 always_available=0 cost=25 radius_m=200 spare_friendly=1 spare_teams=[0, 2, 8, 10, 12, 20, 26, 28, 30, 47] "
                   f"dry_run={int(dry_run)} kill_mode=speffect kills_per_tick=16 retry_s=[0.5, 1.5, 3.0, 5.0] player_dies=1 player_delay_s=3 flash=1 shake=0.6x10s "
                   "horizon=3 key=4 data=1")
    a(1.0, "TASK", "world entered (player handle 0x1 owner_check=ok)")
    a(1.1, "SPAWN", f"pool bullets=0/128 sfx=0/192 sfx_empty_spot=1 unk40=0/28 counter=0 live_ours=0 live_all=0 spawns_world=0")
    a(1.1, "SPAWN", f"emitters live=3 ours=0 persistent=0 max=3 cap=256")
    a(1.2, "NUKE", "rows ok speffect=79000960 endurance=0 hp_rate=100 no_dead=0 bullet=79000961 horizon=79000962 sfx=524028")
    t = 2.0
    # 25 kills (gun / launcher / AC-130)
    n = 0
    for k in range(25):
        t += 1.0
        n += 1
        if streak_skip and k == 10:
            n += 1
        src = "ac130" if k % 7 == 6 else ("launcher" if k % 5 == 4 else "gun")
        a(t, "NUKE", f"streak n={n}/25 src={src}")
        if k == 24:
            a(t, "NUKE", "earned streak=25 life=0")
            if earned_twice:
                a(t + 0.1, "NUKE", "earned streak=25 life=0")
    a(t + 0.5, "SPAWN", f"pool bullets=0/128 sfx=0/192 sfx_empty_spot=1 unk40=0/28 counter=0 live_ours=1 live_all=1 spawns_world=500")
    a(t + 0.5, "SPAWN", f"emitters live=12 ours=2 persistent={persistent} max={emitters_max} cap=256")
    if code4:
        a(t + 0.6, "SPAWN", "FAIL code=4 (3=invalid bullet id, 4=construction failed) cause=pool_emitters_full bullets=0/128 sfx=192/192 "
                            "sfx_empty_spot=0 unk40=0/28 counter=0 live_ours=0 live_all=0 spawns_world=600 marker_probe=- id=79112000 gun=ump45_mp")
    if fail_no_cause:
        a(t + 0.6, "PROJ", "explode gun=rpg_mp id=79152602 SPAWN FAIL code=4 (3=invalid bullet id, 4=construction failed)")
    # reload / rechamber / switch
    a(t + 1.0, "RELOAD", "start empty=1 dur=3 mag=0 gun=stinger_mp")
    a(t + 2.0, "SWITCH", "start from=stinger_mp to=rpg_mp via=wheel drop_s=0.70")
    a(t + 2.0, "RELOAD", "cancelled (switch)")
    if reload_restart:
        a(t + 2.0, "RELOAD", "start empty=1 dur=3 mag=0 gun=stinger_mp")
    a(t + 3.0, "RECHAMBER", "start gun=model1887_mp t=1.000 ads=0")
    if not rechamber_open:
        a(t + 3.5, "RECHAMBER", "cancelled gun=model1887_mp reason=switch")
    # an AC-130 ride with damage
    a(t + 4.0, "AC130", "EXPLODE gun=105 ids=79000201+79000202 pos=(1.00,2.00,3.00) dist_m=135.9 late_ms=0")
    if ac_fail:
        a(t + 4.0, "AC130", "EXPLODE gun=105 id=79000201 SPAWN FAIL code=4 (3=invalid bullet id, 4=construction failed) cause=pool_emitters_full emitters=256")
        a(t + 4.0, "AC130", "EXPLODE gun=105 ids= pos=(1.00,2.00,3.00) dist_m=135.9 late_ms=0")
    a(t + 4.2, "HIT", "chr=c4311 hp=219->0 dmg=219 src=ac130 gun=105 tier=core kill=1")
    a(t + 4.2, "AC130", "KILL chr=c4311 gun=105")
    # the nuke
    t0 = t + 10.0
    a(t0, "NUKE", "activate test=0 streak=25 life=0")
    a(t0, "NUKE", "countdown t=10.0")
    a(t0 + incoming_at, "NUKE", f"incoming t={incoming_at:.2f}")
    a(t0 + detonate_at, "NUKE", f"detonate t={detonate_at:.2f} test=0")
    a(t0 + 10.0, "NUKE", "horizon spawned=1 k=0 at=(1.0,30.0,127.0) dist_m=127")
    a(t0 + 10.15, "NUKE", "horizon spawned=2 k=1 at=(1.0,30.0,127.0) dist_m=127")
    a(t0 + 10.3, "NUKE", "horizon skipped k=2 why=emitters live=170")
    a(t0 + kill_at, "NUKE", "census pass=0 by_distance=40 chrsets=55 union=60 in_radius=12 kill=4 deferred=1 skipped=7 other_frame=2 "
                            f"by_team={{6:3,26:1}} radius_m=200 spare_friendly=1 census_us={census_us}")
    a(t0 + kill_at, "NUKE", "skip reason=team n=2")
    a(t0 + kill_at, "NUKE", "skip reason=set n=2")
    a(t0 + kill_at, "NUKE", "skip reason=other_frame n=2 (another havok frame: distance unknown, not killed)")
    if team_block:
        a(t0 + kill_at, "NUKE", "blocked reason=team_unverified uniform team=0 n=9 (a spared team for every character) pass=0 by_team={0:9}: "
                                "no NPC gets the lethal effect this sequence")
    elif dry_run:
        a(t0 + kill_at, "NUKE", "kill dry_run=1 pass=0: no NPC gets the lethal effect (nuke.dry_run = true; would-apply lines only)")
    else:
        a(t0 + kill_at, "NUKE", "kill armed pass=0 (dry_run=0, teams plausible)")
    a(t0 + kill_at, "NUKE", "kill deferred handle=0x20 npc=21000000 chr=c2120 team=33 why=invincible pass=0")
    for k in range(4):
        if dry_run or team_block:
            team = 26 if (would_spared and k == 0) else 6
            a(t0 + kill_at, "NUKE", f"kill would-apply handle=0x{k + 10:x} npc=43110000 chr=c4311 team={team} hp=219 max_hp=219 "
                                    f"reason={'dry_run' if dry_run else 'team_unverified'} t_ms=0")
        else:
            a(t0 + kill_at, "NUKE", f"kill apply handle=0x{k + 10:x} npc=43110000 chr=c4311 team=6 hp=219 max_hp=219 via=speffect try=1 t_ms=0")
            a(t0 + kill_at + 0.05, "NUKE", f"kill handle=0x{k + 10:x} npc=43110000 chr=c4311 team=6 hp=219->0 max_hp=219 t_ms=50")
    if stuck:
        a(t0 + 12.0, "NUKE", "WARN stuck stage=killing t=60.0s; forcing the end")
    a(t0 + 13.5, "NUKE", "player prep fp=off move=off fire=off protect_present=0" if not no_prep else "player protect remove id=79000950 try=1 call_ok=1")
    a(t0 + 14.5, "NUKE", f"player lethal ok=1 try=1 id=79000960 protect_present={1 if lethal_protected else 0}")
    a(t0 + 14.5 + player_dead_ms / 1000.0, "NUKE", f"player dead t_ms={player_dead_ms}")
    a(t0 + 16.5, "NUKE", "survivor handle=0x20 npc=21000000 chr=c2120 team=33 hp=1 max_hp=9999 applied=0 why=invincible")
    in_r = 12 if not bad_sum else 13
    unarmed = dry_run or team_block
    killed, would = (0, 4) if unarmed else (4, 0)
    armed = "0(dry_run)" if dry_run else ("0(team_unverified)" if team_block else "1")
    a(t0 + 16.6, "NUKE", f"kills total=5 killed={killed} survivors=1 skipped=7 in_radius={in_r} would_apply={would} armed={armed} other_frame=2 "
                         f"census_us_max={census_us} horizon=2/3 t_ms=5100")
    a(t0 + 17.0, "TASK", "world left (loading / title)")
    a(t0 + 17.0, "NUKE", "end reason=loading t=17.0s")
    a(t0 + 17.5, "NUKE", f"post gpu_ms={post_gpu:.4f} (max of the sequence) frames=300")
    # a second activation cancelled by a load during the countdown
    t1 = t0 + 40.0
    a(t1, "NUKE", "activate test=0 streak=0 life=1")
    a(t1, "NUKE", "countdown t=10.0")
    a(t1 + 3.0, "NUKE", f"cancel reason=loading refund={0 if cancel_no_refund else 1}")
    a(t1 + 5.0, "MODE", "toggle blocked reason=nuke")
    return L


def to_entries(lines):
    out = []
    for t, tag, msg in lines:
        ms = int(round(t * 1000))
        raw = f"2026-10-18 12:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [task] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def to_log(lines):
    return "\n".join(e["raw"] for e in to_entries(lines)) + "\n"


MUTATIONS = [
    ("streak skips a count", dict(streak_skip=True), "nuke streak"),
    ("earned twice in one life", dict(earned_twice=True), "nuke streak"),
    ("incoming at 5.0 s", dict(incoming_at=5.0), "nuke sequence"),
    ("detonate at 10.5 s", dict(detonate_at=10.5), "nuke sequence"),
    ("kills at 12.5 s", dict(kill_at=12.5), "nuke sequence"),
    ("stuck warning", dict(stuck=True), "nuke sequence"),
    ("kill sum != in_radius", dict(bad_sum=True), "nuke kills"),
    ("lethal with the protection present", dict(lethal_protected=True), "nuke player"),
    ("lethal without the prep", dict(no_prep=True), "nuke player"),
    ("player dead after 3 s", dict(player_dead_ms=3000), "nuke player"),
    ("post pass 1.2 ms", dict(post_gpu=1.2), "nuke post"),
    ("loading cancel without the refund", dict(cancel_no_refund=True), "nuke cancel"),
    ("a persistent emitter", dict(persistent=3), "emitter pool"),
    ("emitters reach 200", dict(emitters_max=200), "emitter pool"),
    ("a code 4 with the fix", dict(code4=True), "spawn causes"),
    ("a SPAWN FAIL without a cause", dict(fail_no_cause=True), "spawn causes"),
    ("a reload restarted during the drop", dict(reload_restart=True), "reload pairing"),
    ("a rechamber never ends", dict(rechamber_open=True), "reload pairing"),
    ("an AC-130 explosion that did not spawn", dict(ac_fail=True), "ac130 damage"),
    ("NPC kills blocked: team_unverified", dict(team_block=True), "nuke kills"),
    ("a would-apply on a spared team (dry run)", dict(dry_run=True, would_spared=True), "nuke kills"),
    ("a census pass of 80 ms", dict(census_us=80000), "nuke kills"),
]


def self_test():
    """(ok, lines): the good session PASSes every M7 row; each mutation FAILs its row."""
    out = []
    good = dict((n, (s, d)) for n, s, d in rows(to_entries(m7_session())))
    bad_rows = [f"{n}: {s} {d[:80]}" for n, (s, d) in good.items() if s != "PASS"]
    ok = not bad_rows
    out.append(f"good M7 session: {sum(1 for s, _ in good.values() if s == 'PASS')}/{len(good)} rows PASS" + (f" ({bad_rows})" if bad_rows else ""))
    caught = 0
    for label, kw, row in MUTATIONS:
        r = dict((n, (s, d)) for n, s, d in rows(to_entries(m7_session(**kw))))
        if r[row][0] == "FAIL":
            caught += 1
        else:
            out.append(f"MISSED: {label} -> {row} {r[row][0]}")
            ok = False
    out.append(f"{caught}/{len(MUTATIONS)} M7 mutations FAIL their row")
    # a clean dry run is not a pass of the kill row: it asks for the armed run
    dr = dict((n, (s, d)) for n, s, d in rows(to_entries(m7_session(dry_run=True))))
    if dr["nuke kills"][0] != "NEEDS-DATA" or "would-apply" not in dr["nuke kills"][1]:
        out.append(f"MISSED: a clean dry run -> nuke kills {dr['nuke kills'][0]} (want NEEDS-DATA)")
        ok = False
    else:
        out.append("clean dry run: nuke kills NEEDS-DATA (would-apply only)")
    return ok, caught, out
