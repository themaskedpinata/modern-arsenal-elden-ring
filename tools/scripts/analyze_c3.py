"""C3 PASS A rows of analyze_log.py (docs/C3_DESIGN.md 1.8 and 2.7): traversal, the player ray filter, MOVE assist,
first person stuck off (talk latch, F6), the UI yield, the Scaleform probe and the damage numbers. Pure Python,
read-only. Imported by analyze_log.py; standalone:

  python -I analyze_c3.py <log>          rows of the last session (a log without a BOOT line = one session)
  python -I analyze_c3.py --self-test    synthetic sessions per row (PASS / FAIL / N/A, mutations) + the known-bad
                                         replay of logs\\coop\\host_c2_2026-10-19.log (fp talk latch and fp f6 FAIL)
  python -I analyze_c3.py --pair <host log> <friend log> --skew 3600   PASS C cross-log rows (the friend's clock is
                                         `skew` seconds ahead: its times are shifted back by it)

Rows (a C3 DLL logs `MOVE filter live`; older logs are N/A on the rows that need C3 lines, the two first-person rows
judge any log that has talk latches / F6 presses):
- ray filter      PASS: `MOVE filter live ... mode=player` with layer 2/3/4 and src=live, or src=fallback (WARN); FAIL:
                  a rollback mode (union/map/object) in force or cast=0x0. N/A: no `MOVE filter live`.
- filter table    PASS: 3 `MOVE filter table row=.. static=match`; a DIFF is PASS with a WARN (the table is then the
                  init default only); N/A: skip lines / log_live_table off.
- traversal       blocked episodes (`MOVE blocked wish=` lines not followed by `MOVE assist` within 0.6 s) <= 2 per 10
                  driving minutes AND F5 used to climb (`MOVE off reason=f5` -> `MOVE on` within 10 s, dy > 0.5 m) <= 1 per
                  10 driving minutes. N/A: no C3 lines or no driving.
- assist          every `MOVE assist n=.. hold=<s>s` is followed by `MOVE on` within hold + 1.5 s; `WARN assist capped` at
                  most 1 per 10 driving minutes. N/A: no assist line.
- no_face rate    `MOVE mantle check ok=0 why=no_face ... n=<k>` summed per driving minute <= 15. N/A: no C3 lines / none.
- fp talk latch   every `FP latch set signal=talk_id` is released within 6 s after the dialog closed (the last
                  `MENU idx=3 1->0` or `MENU cursor Some(true) -> Some(false)` before the release, else the latch time).
                  N/A: no talk latch.
- fp f6           no `FP blocked reason=f6` within 30 s after an F6 press (`FP toggle F6 wanted=0`) made while first person
                  was off for a non-user reason (talk, cutscene, menu, loading, cam_gate, throw). N/A: no such press.
- ui yield        every `UI yield on` has an `off`; none longer than 30 s unless the cursor was open. N/A: no yield line.
- ui probe        N/A unless `UI probe` lines; PASS: both hooks ok and >= 1 `UI probe scenes/s=` line.
- damage numbers  with `HUD dmgnum cfg on=1`: DMGNUM lines == HIT lines (without peer_drop) +-2 % (at least +-1);
                  on=0: no DMGNUM. N/A: no `HUD dmgnum cfg` line.

PASS B rows (docs/C3_DESIGN.md 4.7; N/A without PAD lines, i.e. a DLL before PASS B or `[gamepad] enabled = false`):
- pad hook        PASS: `PAD hook ok` and either a `PAD stat ... connected=1` or the explicit `PAD none seen in 60s`
                  line; FAIL: `PAD hook FAIL` or neither line 60 s after the hook. N/A: no PAD line / `PAD off`.
- pad blank       with a pad connected while MW2 mode drove (`PAD stat ... connected=1 ... drive=1`): the detour hid
                  buttons (`blanked=` > 0); `MOVE suspend reason=interact` lines not within 1.5 s after a `PAD interact
                  hold` are reported (the keyboard E is not logged, so they are a WARN, not a FAIL). N/A: no such stat.
- pad look        with `PAD cfg ... route=mouse ... deg_per_count=auto` and stick look counts (`look_counts=` > 0): a `PAD
                  calib` line, or the explained `PAD look route fallback=er` line. N/A: route er, a configured
                  deg_per_count, or the stick never turned.

PASS C rows (docs/C3_DESIGN.md 3.9; N/A solo: they need a `COOP session peers=` line with peers >= 1):
- twins           with a peer and MW2 shots (SHOT / LAUNCH / AC130 EXPLODE) in the session: `COOP twin sent` lines;
                  FAIL: shots with a peer but no twin sent, or `COOP WARN rows` (the twin rows are missing).
- peer fx         every `COOP twin seen` window has a `COOP peer fx` line with muzzle + expl >= 80 % of the seen count.
                  N/A: no twin seen.
- markers         MW2 mode on with a peer: `COOP marker sent sp=79001090` (the MW2-mode marker) within 10 s; FAIL
                  otherwise. N/A: no peer while MW2 mode was on.
- peer drop       FAIL: a `HIT ... kill=1` with dmg >= 1000 while a peer is in the session (the C2 credit of the host's
                  nuke kill: dmg=4937); `HIT peer_drop` lines are reported. N/A: no peer.
- coop nuke       every co-op nuke (`NUKE activate ... coop_peers>=1 ... coop_mode=owned`, or `NUKE marker from=..
                  action=start`) has `NUKE coop kills local=` and a census with coop=1; the caller logs `NUKE marker
                  sent`. Review 2026-10-24 (host kills first, client fallback): FAIL a `role=host` census with
                  `hold=1`, a `coop kills ... left=L` with L > 0 (the fallback never applied), and a pre-fix window
                  where this game killed nothing and skipped every live enemy as `remote_owner` (`coop kills local=0
                  remote_skipped=R`, R > 0: chr_update_type read 'remote' for all). N/A: no co-op nuke (or a DLL
                  before PASS C: no coop_mode= field).
- fp locomotion   `MOVE cfg ... fp_anim=hybrid` and unstick fires <= 1 per 10 driving minutes. FAIL: fp_anim=blank
                  or more unsticks. N/A: no fp_anim field (a DLL before PASS C) or < 30 s driving.
- world guns      with a peer: `COOP worldguns loaded guns=N/M` with N > 0, and a `COOP worldgun peer=` line once a
                  peer's gun marker was seen. N/A: no peer.
- tracer occl     review 2026-10-24 ("bullet trails can be seen through walls"): with peer muzzles (`COOP peer fx
                  muzzle=M`, M > 0) and `COOP vis cfg ... tracer_occl=1`, `COOP tracer occl draws ...` lines with rays > 0.
                  FAIL: no such line (the occlusion never ran), or rays=0 with no_rays > 0 (no physics world: drawn
                  unchanged). N/A: no peer muzzle, `tracer_occl=0`, or no `tracer_occl=` field (a DLL before 2026-10-24:
                  the tracers were drawn through walls).

Pair mode (`--pair`): twins (each side's sent windows have a seen line on the other within 6 s), peer fx (the
receiver's muzzle + expl >= 80 % of the sender's twins), markers (each side's MW2-mode marker is seen by the other
within 10 s), no double damage (no HIT on the same character with the same hp transition in both logs within 1 s),
peer drop (no `kill=1` HIT within 6 s after the other PC's NUKE detonate), coop nuke (both logs: detonate, coop
kills, the caller's marker sent, the receiver's marker start, both players dead).
"""
import pathlib
import re
import sys

C3_ROWS = ("ray filter", "filter table", "traversal", "assist", "no_face rate", "fp talk latch", "fp f6", "ui yield",
           "ui probe", "damage numbers", "pad hook", "pad blank", "pad look", "twins", "peer fx", "markers", "peer drop",
           "coop nuke", "fp locomotion", "world guns", "tracer occl")
C3C_ROWS = ("twins", "peer fx", "markers", "peer drop", "coop nuke", "fp locomotion", "world guns", "tracer occl")
PAIR_ROWS = ("twins", "peer fx", "markers", "no double damage", "peer drop", "coop nuke")
SP_MODE = 79001090
LINE = re.compile(r"^(\d{4}-\d\d-\d\d) (\d\d):(\d\d):(\d\d)\.(\d{3}) ([DIWE]) \[([^\]]*)\] (\S+) ?(.*)$")
ROOT = pathlib.Path(__file__).resolve().parents[2]
C2_HOST_LOG = ROOT / "logs" / "coop" / "host_c2_2026-10-19.log"
TALK_RELEASE_S = 6.0
F6_WINDOW_S = 30.0
NON_USER = {"talk", "cutscene", "menu", "loading", "cam_gate", "throw"}
POS_RE = re.compile(r"pos=\((-?[\d.]+),(-?[\d.]+),(-?[\d.]+)\)")


def parse(text):
    out = []
    for raw in text.splitlines():
        m = LINE.match(raw)
        if not m:
            continue
        d, hh, mm, ss, ms, lvl, thread, tag, msg = m.groups()
        out.append({"day": d, "t": int(hh) * 3600 + int(mm) * 60 + int(ss) + int(ms) / 1000.0, "level": lvl, "thread": thread,
                    "tag": tag, "msg": msg, "raw": raw})
    off, prev = 0.0, None
    for e in out:
        if prev is not None and e["t"] + off < prev - 3600:
            off += 86400.0
        e["t"] += off
        prev = e["t"]
    return out


def last_session(entries):
    starts = [i for i, e in enumerate(entries) if e["tag"] == "BOOT" and e["msg"].startswith("mw2er ")]
    return entries[starts[-1]:] if starts else entries


def c3_present(entries):
    return any(e["tag"] == "MOVE" and e["msg"].startswith(("filter live ", "WARN filter live ")) for e in entries)


def driving_s(entries):
    """Seconds between each `MOVE on` and the next `MOVE off|suspend` (or the log end)."""
    tot, on = 0.0, None
    for e in entries:
        if e["tag"] != "MOVE":
            continue
        if e["msg"].startswith("on pos="):
            on = e["t"]
        elif e["msg"].startswith(("off reason=", "suspend reason=")) and on is not None:
            tot += e["t"] - on
            on = None
    if on is not None and entries:
        tot += entries[-1]["t"] - on
    return tot


def judge_ray_filter(entries):
    lines = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith(("filter live ", "WARN filter live "))]
    if not lines:
        return "N/A", "no 'MOVE filter live' line (a DLL before C3 or no MOVE on)"
    last = lines[-1]["msg"]
    mode = (re.search(r"\bmode=(\w+)", last) or [None, "?"])[1]
    cast = (re.search(r"\bcast=0x([0-9a-f]+)", last) or [None, "0"])[1]
    layer = (re.search(r"\blayer=(\S+)", last) or [None, "?"])[1]
    src = (re.search(r"\bsrc=(\w+)", last) or [None, "?"])[1]
    if mode != "player":
        return "FAIL", f"rays mode={mode} in force (a run-18 rollback: hot-reload [movement.rays] mode = \"player\"): {last[:120]}"
    if int(cast, 16) == 0:
        return "FAIL", f"cast=0x0: {last[:120]}"
    if src == "live" and layer in ("2", "3", "4"):
        return "PASS", f"player filter, live layer {layer}, cast=0x{cast} ({len(lines)} line(s))"
    if src == "fallback":
        return "PASS", f"WARN player filter on the fallback cast=0x{cast} (live layer {layer} not accepted / unreadable)"
    return "FAIL", f"unexpected live layer {layer} src={src}: {last[:120]}"


def judge_filter_table(entries):
    rows = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("filter table ")]
    if not rows or all(" skip " in f" {e['msg']} " or "skip why=" in e["msg"] for e in rows):
        return "N/A", "no live table rows (skip or log_live_table off)" if rows else "no 'MOVE filter table' line"
    match = [e for e in rows if "static=match" in e["msg"]]
    diff = [e for e in rows if "static=DIFF" in e["msg"]]
    if len(match) >= 3 and not diff:
        return "PASS", f"{len(match)} rows equal the static emu2 table"
    if diff:
        return "PASS", f"WARN {len(diff)} row(s) DIFF from the static table (the static table is the init default only): {diff[0]['msg'][:100]}"
    return "PASS", f"WARN only {len(match)} matching row(s) logged ({len(rows) - len(match)} skipped)"


def judge_traversal(entries):
    if not c3_present(entries):
        return "N/A", "no C3 lines (a DLL before C3)"
    drive = driving_s(entries)
    if drive < 30.0:
        return "N/A", f"only {drive:.0f} s of driving"
    mins = drive / 60.0
    blocked = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("blocked wish=")]
    assists = [e["t"] for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("assist n=")]
    unassisted = [e for e in blocked if not any(0 <= a - e["t"] <= 0.6 for a in assists)]
    climbs = 0
    mv = [e for e in entries if e["tag"] == "MOVE"]
    for i, e in enumerate(mv):
        if e["msg"].startswith("off reason=f5") and (p0 := POS_RE.search(e["msg"])):
            for f in mv[i + 1:]:
                if f["t"] - e["t"] > 10.0:
                    break
                if f["msg"].startswith("on pos=") and (p1 := POS_RE.search(f["msg"])):
                    if float(p1.group(2)) - float(p0.group(2)) > 0.5:
                        climbs += 1
                    break
    b_rate, c_rate = len(unassisted) / mins * 10.0, climbs / mins * 10.0
    detail = f"{len(unassisted)} unassisted blocked episode(s) ({b_rate:.1f}/10 min), {climbs} F5 climb(s) ({c_rate:.1f}/10 min) in {mins:.1f} driving min"
    if b_rate <= 2.0 and c_rate <= 1.0:
        return "PASS", detail
    return "FAIL", detail


def judge_assist(entries):
    lines = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("assist n=")]
    if not lines:
        return "N/A", "no 'MOVE assist' line"
    ons = [e["t"] for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("on pos=")]
    bad = []
    for e in lines:
        hold = float((re.search(r"\bhold=([\d.]+)s", e["msg"]) or [None, "0.8"])[1])
        if not any(0 <= t - e["t"] <= hold + 1.5 for t in ons):
            bad.append(e)
    capped = [e for e in entries if e["tag"] == "MOVE" and "WARN assist capped" in e["msg"]]
    mins = max(driving_s(entries) / 60.0, 1.0)
    if bad:
        return "FAIL", f"{len(bad)} of {len(lines)} assist(s) without 'MOVE on' within hold + 1.5 s (first t={bad[0]['t']:.1f})"
    if len(capped) / mins * 10.0 > 1.0:
        return "FAIL", f"{len(capped)} 'assist capped' in {mins:.1f} driving min (a cap storm)"
    return "PASS", f"{len(lines)} assist(s), each handed back; {len(capped)} capped"


def judge_no_face(entries):
    if not c3_present(entries):
        return "N/A", "no C3 lines"
    nf = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("mantle check ok=0 why=no_face")]
    if not nf:
        return "N/A", "no no_face refusal line"
    n = sum(int((re.search(r"\bn=(\d+)", e["msg"]) or [None, "1"])[1]) for e in nf)
    mins = max(driving_s(entries) / 60.0, 1e-3)
    rate = n / mins
    return ("PASS" if rate <= 15.0 else "FAIL"), f"{n} no_face refusals in {mins:.1f} driving min ({rate:.1f}/min, max 15)"


def judge_fp_talk(entries):
    sets = [i for i, e in enumerate(entries) if e["tag"] == "FP" and e["msg"].startswith("latch set signal=talk_id")]
    if not sets:
        return "N/A", "no talk latch"
    bad, worst = [], 0.0
    for i in sets:
        t0 = entries[i]["t"]
        rel = next((e for e in entries[i + 1:] if e["tag"] == "FP" and (e["msg"].startswith("latch released signal=talk_id")
                                                                         or e["msg"].startswith("latch dropped signal=talk_id")
                                                                         or (e["msg"].startswith("toggle F6 override") and "talk_id" in e["msg"]))), None)
        end_t = rel["t"] if rel else entries[-1]["t"]
        closes = [e["t"] for e in entries[i + 1:] if t0 <= e["t"] <= end_t and e["tag"] == "MENU"
                  and (e["msg"].startswith("idx=3 1->0") or e["msg"].startswith("cursor Some(true) -> Some(false)"))]
        ref = max(closes) if closes else t0
        late = end_t - ref
        worst = max(worst, late)
        if late > TALK_RELEASE_S and (rel or entries[-1]["t"] - ref > TALK_RELEASE_S):
            bad.append((t0, late, rel is not None))
    if bad:
        t0, late, released = bad[0]
        return "FAIL", f"{len(bad)} of {len(sets)} talk latch(es) held > {TALK_RELEASE_S:.0f} s after the dialog closed (first at t={t0:.1f}: {late:.1f} s{'' if released else ', never released'})"
    return "PASS", f"{len(sets)} talk latch(es), each released within {worst:.1f} s of the dialog closing"


def judge_fp_f6(entries):
    state = None   # (on: bool, reason)
    presses = []
    for e in entries:
        if e["tag"] != "FP":
            continue
        m = e["msg"]
        if m.startswith("on eye="):
            state = (True, None)
        elif (r := re.match(r"(?:off|blocked) reason=(\w+)", m)):
            if not (state and not state[0] and r.group(1) == "f6"):
                state = (False, r.group(1))
        elif m.startswith(("toggle F6 wanted=0", "toggle F6 override")) and state and not state[0] and state[1] in NON_USER:
            presses.append((e["t"], state[1]))
    if not presses:
        return "N/A", "no F6 press while first person was off for a non-user reason"
    blocked = [e["t"] for e in entries if e["tag"] == "FP" and e["msg"].startswith("blocked reason=f6")]
    bad = [(t, r) for t, r in presses if any(0 <= b - t <= F6_WINDOW_S for b in blocked)]
    if bad:
        return "FAIL", f"{len(bad)} F6 press(es) during a {bad[0][1]} hold turned first person off (FP blocked reason=f6 within {F6_WINDOW_S:.0f} s; first at t={bad[0][0]:.1f})"
    return "PASS", f"{len(presses)} F6 press(es) during non-user holds, none left first person off"


def judge_ui_yield(entries):
    ev = [e for e in entries if e["tag"] == "UI" and (e["msg"].startswith("yield on ") or e["msg"].startswith("yield off "))]
    if not ev:
        return "N/A", "no 'UI yield' line"
    cursor = []
    for e in entries:
        if e["tag"] == "MENU" and e["msg"].startswith("cursor "):
            cursor.append((e["t"], e["msg"].endswith("Some(true)")))

    def cursor_open(a, b):
        st = False
        for t, v in cursor:
            if t <= a:
                st = v
        return st or any(a <= t <= b and v for t, v in cursor)
    bad, on, n = [], None, 0
    for e in ev:
        if e["msg"].startswith("yield on "):
            if on is not None:
                bad.append(f"t={on:.1f}: 'on' without 'off'")
            on = e["t"]
            n += 1
        else:
            if on is not None and e["t"] - on > 30.0 and not cursor_open(on, e["t"]):
                bad.append(f"t={on:.1f}: yield lasted {e['t'] - on:.0f} s without a menu")
            on = None
    if on is not None and entries[-1]["t"] - on > 30.0:
        bad.append(f"t={on:.1f}: never ended")
    if bad:
        return "FAIL", f"{len(bad)} problem(s): {bad[0]}"
    return "PASS", f"{n} yield(s), each ended (<= 30 s or a menu)"


def judge_ui_probe(entries):
    lines = [e for e in entries if e["tag"] == "UI" and e["msg"].startswith("probe ")]
    if not lines or all(e["msg"].startswith("probe off") for e in lines):
        return "N/A", "probe off ([ui] probe_scaleform = false)"
    ok = {f for e in lines if (f := (re.match(r"probe hook ok fn=(\w+)", e["msg"]) or [None, None])[1])}
    scenes = [e for e in lines if e["msg"].startswith("probe scenes/s=")]
    if {"begin_scene", "end_scene"} <= ok and scenes:
        return "PASS", f"both hooks ok, {len(scenes)} probe line(s): {scenes[-1]['msg'][:100]}"
    skips = [e["msg"] for e in lines if e["msg"].startswith("probe skip")]
    return "FAIL", f"hooks ok {sorted(ok)}, {len(scenes)} scene line(s){'; ' + skips[0][:80] if skips else ''}"


def judge_dmgnum(entries):
    cfg = [e for e in entries if e["tag"] == "HUD" and e["msg"].startswith("dmgnum cfg ")]
    if not cfg:
        return "N/A", "no 'HUD dmgnum cfg' line (a DLL before C3)"
    on = "on=1" in cfg[-1]["msg"]
    hits = [e for e in entries if e["tag"] == "HIT" and not e["msg"].startswith("peer_drop")]
    nums = [e for e in entries if e["tag"] == "DMGNUM"]
    if not on:
        return ("PASS" if not nums else "FAIL"), f"damage_numbers off: {len(nums)} DMGNUM line(s)"
    if not hits:
        return "N/A", "damage numbers on, no HIT"
    tol = max(1, round(0.02 * len(hits)))
    if abs(len(nums) - len(hits)) <= tol:
        return "PASS", f"{len(nums)} DMGNUM for {len(hits)} HIT (merged {sum('merge=1' in e['msg'] for e in nums)})"
    return "FAIL", f"{len(nums)} DMGNUM for {len(hits)} HIT (tolerance {tol})"


def _num(msg, key, default=0):
    m = re.search(r"\b" + key + r"=(-?\d+)", msg)
    return int(m.group(1)) if m else default


def judge_pad_hook(entries):
    pad = [e for e in entries if e["tag"] == "PAD"]
    if not any(e["msg"].startswith("hook ") for e in pad):
        return "N/A", "no 'PAD hook' line (a DLL before C3 PASS B, or [gamepad] enabled = false: 'PAD off')"
    fail = [e for e in pad if e["msg"].startswith("hook FAIL")]
    if fail:
        return "FAIL", fail[-1]["msg"][:120]
    ok = [e for e in pad if e["msg"].startswith("hook ok ")]
    conn = [e for e in pad if e["msg"].startswith("stat ") and " connected=1" in e["msg"]]
    none = [e for e in pad if e["msg"].startswith("none seen in 60s")]
    if conn:
        return "PASS", f"hooked ({ok[-1]['msg'][8:80]}); a pad connected ({len(conn)} stat line(s) with connected=1)"
    if none:
        return "PASS", f"hooked; no XInput pad seen: '{none[-1]['msg'][:80]}' (PlayStation pads need Steam Input)"
    if entries[-1]["t"] - ok[-1]["t"] < 60.0:
        return "N/A", "hooked less than 60 s before the log end"
    return "FAIL", "hooked but neither a connected pad nor the 'PAD none seen' line"


def judge_pad_blank(entries):
    stats = [e for e in entries if e["tag"] == "PAD" and e["msg"].startswith("stat ") and " connected=1" in e["msg"] and " drive=1" in e["msg"]]
    if not stats:
        return "N/A", "no pad connected while MW2 mode drove"
    blanked = max(_num(e["msg"], "blanked") for e in stats)
    holds = [e["t"] for e in entries if e["tag"] == "PAD" and e["msg"].startswith("interact hold ")]
    sus = [e["t"] for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("suspend reason=interact")]
    unattr = [t for t in sus if not any(0 <= t - h <= 1.5 for h in holds)]
    if blanked <= 0:
        return "FAIL", f"{len(stats)} stat line(s) with the pad connected while driving, but blanked=0 (the detour hid nothing)"
    warn = f"; WARN {len(unattr)} interact suspend(s) not after a pad X-hold (keyboard E or a leak)" if unattr else ""
    return "PASS", f"blanked={blanked} reads; {len(holds)} X-hold interact(s), {len(sus) - len(unattr)} attributed suspend(s){warn}"


def judge_pad_look(entries):
    cfg = [e for e in entries if e["tag"] == "PAD" and e["msg"].startswith("cfg ")]
    if not cfg or " route=mouse " not in cfg[-1]["msg"] or "deg_per_count=auto" not in cfg[-1]["msg"]:
        return "N/A", "route er or a configured deg_per_count" if cfg else "no 'PAD cfg' line"
    stats = [e for e in entries if e["tag"] == "PAD" and e["msg"].startswith("stat ")]
    counts = max([_num(e["msg"], "look_counts") for e in stats] or [0])
    if counts <= 0:
        return "N/A", "the right stick never turned the camera through the mouse route"
    calib = [e for e in entries if e["tag"] == "PAD" and e["msg"].startswith("calib deg_per_count=")]
    fb = [e for e in entries if e["tag"] == "PAD" and e["msg"].startswith("look route fallback=er")]
    if calib:
        return "PASS", f"look_counts={counts}; {calib[-1]['msg'][:90]}"
    if fb:
        return "PASS", f"look_counts={counts}; WARN ER did not poll the mouse: {fb[-1]['msg'][:80]}"
    return "FAIL", f"look_counts={counts} but no 'PAD calib' line (deg_per_count stayed provisional)"


# ---- PASS C (docs/C3_DESIGN.md 3.9) -------------------------------------------------------------------------------

def peer_spans(entries):
    """[(t0, t1)] while `COOP session peers=<n>` says n >= 1 (the debounced watch)."""
    spans, start = [], None
    for e in entries:
        if e["tag"] == "COOP" and e["msg"].startswith("session peers="):
            n = _num(e["msg"], "peers")
            if n >= 1 and start is None:
                start = e["t"]
            elif n == 0 and start is not None:
                spans.append((start, e["t"]))
                start = None
    if start is not None and entries:
        spans.append((start, entries[-1]["t"]))
    return spans


def in_spans(t, spans, pad=0.0):
    return any(a - pad <= t <= b + pad for a, b in spans)


def vis_present(entries):
    """A PASS C DLL logs `COOP vis cfg ...` once per session."""
    return any(e["tag"] == "COOP" and e["msg"].startswith("vis cfg ") for e in entries)


def judge_twins(entries):
    spans = peer_spans(entries)
    if not spans:
        return "N/A", "no peer in the session"
    if not vis_present(entries):
        return "N/A", "no 'COOP vis cfg' line (a DLL before C3 PASS C)"
    rows_bad = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("WARN rows")]
    if rows_bad:
        return "FAIL", f"twin rows missing: {rows_bad[0]['msg'][:110]}"
    shots = [e for e in entries if in_spans(e["t"], spans) and ((e["tag"] == "SHOT" and e["msg"].startswith("n=")) or e["tag"] == "LAUNCH"
                                                                  or (e["tag"] == "AC130" and e["msg"].startswith("EXPLODE ")))]
    sent = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("twin sent ")]
    if not shots:
        return "N/A", f"a peer for {sum(b - a for a, b in spans):.0f} s but no MW2 shot meanwhile"
    n = sum(int((re.search(r"\bn=(\d+)/", e["msg"]) or [None, "0"])[1]) for e in sent)
    if n == 0:
        return "FAIL", f"{len(shots)} MW2 shot(s) with a peer, no 'COOP twin sent' (twins not spawned)"
    return "PASS", f"{n} twin(s) sent for {len(shots)} shot / launch / explosion line(s) with a peer"


def judge_peer_fx(entries):
    seen = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("twin seen ")]
    if not seen:
        return "N/A", "no peer twin seen"
    fx = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("peer fx ")]
    n_seen = sum(int((re.search(r"\bn=(\d+)/", e["msg"]) or [None, "0"])[1]) for e in seen)
    n_fx = sum(_num(e["msg"], "muzzle") + _num(e["msg"], "expl") + _num(e["msg"], "nuke_markers") for e in fx)
    if n_seen and n_fx >= 0.8 * n_seen:
        return "PASS", f"{n_fx} peer effect(s) for {n_seen} twin(s) seen"
    return "FAIL", f"{n_fx} peer effect(s) for {n_seen} twin(s) seen (< 80 %)"


def judge_markers(entries):
    spans = peer_spans(entries)
    ons = [e["t"] for e in entries if e["tag"] == "MODE" and e["msg"].startswith("on ") and in_spans(e["t"], spans)]
    # MW2 mode already on when the peer joined: the join counts as the start
    for a, _b in spans:
        last_mode = [e for e in entries if e["tag"] == "MODE" and e["t"] <= a and (e["msg"].startswith("on ") or e["msg"] == "off")]
        if last_mode and last_mode[-1]["msg"].startswith("on "):
            ons.append(a)
    if not ons:
        return "N/A", "no peer while MW2 mode was on"
    if not vis_present(entries):
        return "N/A", "no 'COOP vis cfg' line (a DLL before C3 PASS C)"
    sent = [e["t"] for e in entries if e["tag"] == "COOP" and e["msg"].startswith(f"marker sent sp={SP_MODE}")]
    bad = [t for t in ons if not any(0 <= s - t <= 10.0 for s in sent) and entries[-1]["t"] - t > 10.0]
    seen = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("marker seen ")]
    if bad:
        return "FAIL", f"{len(bad)} MW2-mode start(s) with a peer without 'COOP marker sent sp={SP_MODE}' within 10 s (first t={bad[0]:.1f})"
    return "PASS", f"{len(ons)} MW2-mode start(s) with a peer, marker sent each time; {len(seen)} peer marker(s) seen"


def judge_peer_drop(entries):
    spans = peer_spans(entries)
    if not spans:
        return "N/A", "no peer in the session"
    big = [e for e in entries if e["tag"] == "HIT" and not e["msg"].startswith("peer_drop") and " kill=1" in e["msg"]
           and _num(e["msg"], "dmg") >= 1000 and in_spans(e["t"], spans)]
    drops = [e for e in entries if e["tag"] == "HIT" and e["msg"].startswith("peer_drop")]
    if big:
        return "FAIL", f"{len(big)} kill credit(s) with dmg >= 1000 while a peer was there (a peer's kill credited to us): {big[0]['msg'][:100]}"
    return "PASS", f"no implausible kill credit; {len(drops)} 'HIT peer_drop' line(s)"


def nuke_survivors(entries, t0):
    """Review C3: the `NUKE kills total=N killed=K survivors=S` line of the sequence started at t0 -> (N, K, S) or None."""
    for e in entries:
        if e["tag"] == "NUKE" and e["msg"].startswith("kills total=") and t0 <= e["t"] <= t0 + 90.0:
            return _num(e["msg"], "total"), _num(e["msg"], "killed"), _num(e["msg"], "survivors")
    return None


def survivors_bad(n, k, s):
    """Review C3: more survivors than the solo baseline allows (solo runs left 0-1 immortal; C2's host nuke 44 of 51)."""
    return s > max(2, 0.1 * max(n, 1))


def judge_coop_nuke(entries):
    acts = [e for e in entries if e["tag"] == "NUKE" and ((e["msg"].startswith("activate ") and "coop_mode=owned" in e["msg"]
                                                            and _num(e["msg"], "coop_peers") >= 1)
                                                           or (e["msg"].startswith("marker from=") and "action=start" in e["msg"]))]
    # review C3: a pre-PASS-C co-op nuke (C2: `activate ... coop_peers=1 role=..` without coop_mode) is judged on its
    # survivors only (the C2 host nuke killed 7 of 51: FAIL)
    old = [e for e in entries if e["tag"] == "NUKE" and e["msg"].startswith("activate ") and "coop_mode=" not in e["msg"]
           and _num(e["msg"], "coop_peers") >= 1]
    if not acts and not old:
        return "N/A", "no co-op nuke (or a DLL before PASS C)"
    bad = []
    for a in old:
        sv = nuke_survivors(entries, a["t"])
        if sv and survivors_bad(*sv):
            bad.append(f"t={a['t']:.1f}: {sv[2]} survivor(s) of {sv[0]} (killed {sv[1]}; enemies a peer's game owns were not killed)")
    for a in acts:
        if a["msg"].startswith("marker from="):
            continue  # the activate line right after it is judged
        after = [e for e in entries if e["tag"] == "NUKE" and a["t"] <= e["t"] <= a["t"] + 60.0]
        remote = " remote=0 " not in f" {a['msg']} "
        sv = nuke_survivors(entries, a["t"])
        if sv and survivors_bad(*sv):
            bad.append(f"t={a['t']:.1f}: {sv[2]} survivor(s) of {sv[0]} (killed {sv[1]})")
        if not any(e["msg"].startswith("coop kills local=") for e in after):
            bad.append(f"t={a['t']:.1f}: no 'NUKE coop kills'")
        if not any(e["msg"].startswith("census pass=") and " coop=1" in e["msg"] for e in after):
            bad.append(f"t={a['t']:.1f}: no co-op census (coop=1)")
        if not remote and not any(e["msg"].startswith("marker sent ") for e in after):
            bad.append(f"t={a['t']:.1f}: the caller sent no marker")
        # review 2026-10-24: the host never holds; nothing is left alive unapplied at the window end; the pre-fix
        # 'every enemy remote' census (both games skipped everything) fails on its own
        if any(e["msg"].startswith("census pass=") and " role=host " in f" {e['msg']} " and " hold=1" in e["msg"] for e in after):
            bad.append(f"t={a['t']:.1f}: a host census held its kills (hold=1)")
        for e in after:
            if e["msg"].startswith("coop kills local="):
                if _num(e["msg"], "left") > 0:
                    bad.append(f"t={a['t']:.1f}: {_num(e['msg'], 'left')} enemy(ies) left alive unapplied at the window end ({e['msg'][:80]})")
                if "remote_skipped=" in e["msg"] and _num(e["msg"], "local") == 0 and _num(e["msg"], "remote_skipped") > 0:
                    bad.append(f"t={a['t']:.1f}: no kill, {_num(e['msg'], 'remote_skipped')} live enemy(ies) skipped as remote_owner "
                               "(chr_update_type is not the owner: a DLL before the 2026-10-24 fix)")
                break
    if bad:
        return "FAIL", f"{len(bad)} problem(s): {bad[0]}"
    kills = [e for e in entries if e["tag"] == "NUKE" and e["msg"].startswith("coop kills local=")]
    return "PASS", f"{len(acts) + len(old)} co-op nuke line(s), survivors within the solo baseline; {kills[-1]['msg'][:90] if kills else ''}"


def judge_fp_locomotion(entries):
    cfg = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("cfg ") and " fp_anim=" in e["msg"]]
    if not cfg:
        return "N/A", "no 'MOVE cfg ... fp_anim=' (a DLL before PASS C)"
    mode = (re.search(r"\bfp_anim=(\w+)", cfg[-1]["msg"]) or [None, "?"])[1]
    drive = driving_s(entries)
    if drive < 30.0:
        return "N/A", f"only {drive:.0f} s of driving"
    unst = [e for e in entries if e["tag"] == "MOVE" and e["msg"].startswith("unstick n=")]
    rate = len(unst) / (drive / 60.0) * 10.0
    if mode != "hybrid":
        return "FAIL", f"fp_anim={mode} (peers see no walk / run animations in first person)"
    if rate > 1.0:
        return "FAIL", f"fp_anim=hybrid but {len(unst)} unstick(s) in {drive / 60.0:.1f} driving min ({rate:.1f}/10 min > 1)"
    return "PASS", f"fp_anim=hybrid; {len(unst)} unstick(s) in {drive / 60.0:.1f} driving min"


def judge_world_guns(entries):
    if not peer_spans(entries):
        return "N/A", "no peer in the session"
    loaded = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("worldguns loaded guns=")]
    if not loaded:
        return "N/A", "no 'COOP worldguns loaded' line (the viewmodel loader did not run / a DLL before PASS C)"
    m = re.search(r"guns=(\d+)/(\d+)", loaded[-1]["msg"])
    if not m or int(m.group(1)) == 0:
        return "FAIL", f"no world model on the GPU: {loaded[-1]['msg'][:90]}"
    gun_seen = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("marker seen ")
                and 79001100 <= _num(e["msg"], "sp") <= 79001149]
    wg = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("worldgun peer=")]
    if gun_seen and not wg:
        return "FAIL", f"{len(gun_seen)} peer gun marker(s) seen but no 'COOP worldgun peer=' line"
    return "PASS", f"{m.group(1)}/{m.group(2)} guns loaded; {len(wg)} worldgun line(s), {len(gun_seen)} gun marker(s) seen"


def judge_tracer_occl(entries):
    """Review 2026-10-24: the peers' HUD tracers are clipped / hidden by map rays (`COOP tracer occl draws` every 5 s)."""
    muzzles = sum(_num(e["msg"], "muzzle") for e in entries if e["tag"] == "COOP" and e["msg"].startswith("peer fx "))
    if muzzles == 0:
        return "N/A", "no peer muzzle (no peer tracer drawn)"
    cfg = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("vis cfg ")]
    m = re.search(r"\btracer_occl=(\d)", cfg[-1]["msg"]) if cfg else None
    if not m:
        return "N/A", f"{muzzles} peer muzzle(s); no tracer_occl= field (a DLL before 2026-10-24: tracers drawn through walls)"
    if m.group(1) == "0":
        return "N/A", f"{muzzles} peer muzzle(s); [coop] tracer_occlusion = false (drawn as before)"
    occ = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("tracer occl draws ")]
    if not occ:
        return "FAIL", f"{muzzles} peer muzzle(s) with tracer_occl=1 but no 'COOP tracer occl draws' line (the occlusion never ran)"
    k = {f: sum(_num(e["msg"], f) for e in occ) for f in ("shown", "partial", "hidden", "clipped", "rays", "budget_out", "no_rays", "flashes_hidden", "streaks_hidden")}
    if k["rays"] == 0 and k["no_rays"] > 0:
        return "FAIL", f"no map ray cast (no_rays={k['no_rays']} ticks): every tracer drawn unchanged"
    return "PASS", (f"{muzzles} peer muzzle(s); draws shown={k['shown']} partial={k['partial']} hidden={k['hidden']} clipped={k['clipped']} "
                    f"rays={k['rays']} budget_out={k['budget_out']} no_rays={k['no_rays']} flashes_hidden={k['flashes_hidden']} streaks_hidden={k['streaks_hidden']}")


def rows(entries, ctx=None):
    return [("ray filter", *judge_ray_filter(entries)), ("filter table", *judge_filter_table(entries)),
            ("traversal", *judge_traversal(entries)), ("assist", *judge_assist(entries)),
            ("no_face rate", *judge_no_face(entries)), ("fp talk latch", *judge_fp_talk(entries)),
            ("fp f6", *judge_fp_f6(entries)), ("ui yield", *judge_ui_yield(entries)), ("ui probe", *judge_ui_probe(entries)),
            ("damage numbers", *judge_dmgnum(entries)), ("pad hook", *judge_pad_hook(entries)),
            ("pad blank", *judge_pad_blank(entries)), ("pad look", *judge_pad_look(entries)),
            ("twins", *judge_twins(entries)), ("peer fx", *judge_peer_fx(entries)), ("markers", *judge_markers(entries)),
            ("peer drop", *judge_peer_drop(entries)), ("coop nuke", *judge_coop_nuke(entries)),
            ("fp locomotion", *judge_fp_locomotion(entries)), ("world guns", *judge_world_guns(entries)),
            ("tracer occl", *judge_tracer_occl(entries))]


# ---- pair mode (two logs of one session) ---------------------------------------------------------------------------

def shift(entries, dt):
    return [{**e, "t": e["t"] - dt} for e in entries]


def pair_rows(a, b):
    """Cross-log rows of host log `a` and friend log `b` (b already shifted onto a's clock)."""
    out = []

    def sent_windows(x):
        return [(e["t"], int((re.search(r"\bn=(\d+)/", e["msg"]) or [None, "0"])[1])) for e in x
                if e["tag"] == "COOP" and e["msg"].startswith("twin sent ")]

    def seen_times(x):
        return [e["t"] for e in x if e["tag"] == "COOP" and e["msg"].startswith("twin seen ")]
    miss, total = [], 0
    for src, dst, name in ((a, b, "host->friend"), (b, a, "friend->host")):
        sw, st = sent_windows(src), seen_times(dst)
        total += len(sw)
        miss += [f"{name} t={t:.1f}" for t, n in sw if n > 0 and not any(abs(s - t) <= 6.0 for s in st)]
    if total == 0:
        out.append(("twins", "N/A", "no 'COOP twin sent' in either log"))
    else:
        out.append(("twins", "PASS" if not miss else "FAIL", f"{total} sent window(s); unseen: {len(miss)}{' ' + miss[0] if miss else ''}"))

    def sent_n(x):
        return sum(n for _, n in sent_windows(x))

    def fx_n(x):
        return sum(_num(e["msg"], "muzzle") + _num(e["msg"], "expl") for e in x if e["tag"] == "COOP" and e["msg"].startswith("peer fx "))
    pf = []
    for src, dst, name in ((a, b, "host->friend"), (b, a, "friend->host")):
        s, f = sent_n(src), fx_n(dst)
        if s > 0:
            pf.append((name, s, f, f >= 0.8 * s))
    if not pf:
        out.append(("peer fx", "N/A", "no twins sent"))
    else:
        out.append(("peer fx", "PASS" if all(x[3] for x in pf) else "FAIL", "; ".join(f"{n}: {f} effect(s) for {s} twin(s)" for n, s, f, _ in pf)))
    mk = []
    for src, dst, name in ((a, b, "host->friend"), (b, a, "friend->host")):
        for e in src:
            if e["tag"] == "COOP" and e["msg"].startswith(f"marker sent sp={SP_MODE}"):
                ok = any(f["tag"] == "COOP" and f["msg"].startswith("marker seen ") and f"sp={SP_MODE}" in f["msg"] and 0 <= f["t"] - e["t"] <= 10.0 for f in dst)
                mk.append((name, e["t"], ok))
    if not mk:
        out.append(("markers", "N/A", "no MW2-mode marker sent"))
    else:
        bad = [m for m in mk if not m[2]]
        out.append(("markers", "PASS" if not bad else "FAIL", f"{len(mk)} marker(s) sent, {len(bad)} not seen within 10 s{' (first ' + bad[0][0] + f' t={bad[0][1]:.1f})' if bad else ''}"))

    def hits(x):
        return [(e["t"], m.group(1), m.group(2), m.group(3)) for e in x if e["tag"] == "HIT"
                and (m := re.match(r"chr=(c\d+) hp (-?\d+)->(-?\d+)", e["msg"]))]
    ha, hb = hits(a), hits(b)
    dbl = [x for x in ha if any(y[1:] == x[1:] and abs(y[0] - x[0]) <= 1.0 for y in hb)]
    if not ha and not hb:
        out.append(("no double damage", "N/A", "no HIT in either log"))
    else:
        out.append(("no double damage", "PASS" if not dbl else "FAIL", f"{len(ha)}/{len(hb)} HIT(s); {len(dbl)} transition(s) credited in both logs"))
    pd, n_det = [], 0
    for src, dst, name in ((a, b, "host nuke"), (b, a, "friend nuke")):
        for e in src:
            if e["tag"] == "NUKE" and e["msg"].startswith("detonate "):
                n_det += 1
                pd += [f"{name} t={e['t']:.1f}" for f in dst if f["tag"] == "HIT" and not f["msg"].startswith("peer_drop") and " kill=1" in f["msg"] and 0 <= f["t"] - e["t"] <= 6.0]
    if not n_det:
        out.append(("peer drop", "N/A", "no nuke in either log"))
    else:
        out.append(("peer drop", "PASS" if not pd else "FAIL", f"{n_det} detonation(s); {len(pd)} kill credit(s) on the other PC within 6 s"))
    cn = []
    for x, name in ((a, "host"), (b, "friend")):
        dets = [e for e in x if e["tag"] == "NUKE" and e["msg"].startswith("detonate ")]
        if dets:
            have = {k: any(e["tag"] == "NUKE" and e["msg"].startswith(k) for e in x)
                    for k in ("coop kills local=", "player dead", "marker sent ", "marker from=")}
            cn.append((name, have))
    c_dll = any(e["tag"] == "NUKE" and e["msg"].startswith("activate ") and "coop_mode=" in e["msg"] for e in a + b)
    if not cn or not c_dll:
        out.append(("coop nuke", "N/A", "no detonation in either log" if not cn else "no PASS C nuke line (coop_mode=) in either log"))
    else:
        bad = [f"{n}: missing {[k for k, v in h.items() if not v and k in ('coop kills local=', 'player dead')]}" for n, h in cn
               if not (h["coop kills local="] and h["player dead"])]
        callers = [n for n, h in cn if h["marker sent "]]
        receivers = [n for n, h in cn if h["marker from="]]
        if len(cn) == 2 and not (callers and receivers):
            bad.append(f"callers {callers} receivers {receivers} (the marker did not cross)")
        # review C3: the union of kills covers what either census left: every NPC one game skipped as the peer's
        # (`left=R`, before 2026-10-24 `remote_skipped=R`, at the window end) must be matched by at least R local kills in the other game within 30 s,
        # and neither game may end with more survivors than the solo baseline [I: handles are per PC, so this is a count
        # match, not an NPC-by-NPC union]
        bad += coop_union_problems(a, b)
        out.append(("coop nuke", "PASS" if not bad else "FAIL", f"{len(cn)} log(s) with a detonation; {bad[0] if bad else 'coop kills + player death in each, marker crossed'}"))
    return out



def coop_union_problems(a, b):
    """Review C3 (pair): every remote-skipped NPC of one game killed by the other game; no survivor excess on either."""
    out = []

    def ck(x):
        return [(e["t"], _num(e["msg"], "local"), _num(e["msg"], "left" if " left=" in e["msg"] else "remote_skipped")) for e in x
                if e["tag"] == "NUKE" and e["msg"].startswith("coop kills local=")]
    for src, dst, name in ((a, b, "host"), (b, a, "friend")):
        for t, _local, r in ck(src):
            if r <= 0:
                continue
            other = [loc for t2, loc, _ in ck(dst) if abs(t2 - t) <= 30.0]
            if not other or max(other) < r:
                out.append(f"{name} t={t:.1f}: {r} NPC(s) left to the peer's game, which killed {max(other) if other else 'none'} (not everyone died)")
        for e in src:
            if e["tag"] == "NUKE" and e["msg"].startswith("detonate "):
                sv = nuke_survivors(src, e["t"] - 15.0)
                if sv and survivors_bad(*sv):
                    out.append(f"{name} t={e['t']:.1f}: {sv[2]} survivor(s) of {sv[0]}")
    return out


# ---- synthetic sessions (self-test) -----------------------------------------------------------------------------

def to_entries(lines):
    out = []
    for t, tag, msg in sorted(lines, key=lambda x: x[0]):
        ms = int(round(t * 1000))
        raw = f"2026-10-20 {ms // 3600000 % 24:02d}:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [task] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def to_log(lines):
    return "\n".join(e["raw"] for e in to_entries(lines)) + "\n"


def c3_session(mode="player", src="live", layer=3, table="match", blocked=1, f5_climb=False, assist_no_on=False,
               capped=0, no_face=200, talk_late=False, f6_bug=False, yield_stuck=False, dmg_missing=0, dmg_on=True,
               probe=None, pad="ok", pad_blanked=812, pad_calib=True):
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))
    a(0.0, "BOOT", "mw2er 0.3.0")
    a(0.5, "HUD", f"dmgnum cfg on={int(dmg_on)} life=0.90 merge=0.25 value=er head_guess=1 max=24 scale=1.00 suppress_er_tag=0")
    if probe == "ok":
        a(0.6, "UI", "probe hook ok fn=begin_scene addr=0x14115e6b0 prologue=ok (read-only; logs only)")
        a(0.6, "UI", "probe hook ok fn=end_scene addr=0x14115e810 prologue=ok (read-only; logs only)")
        a(10.0, "UI", "probe scenes/s=60.0 ends/s=60.0 thread=present this=0x7ff400000000 flags=0x2 cl_off=none")
    elif probe == "skip":
        a(0.6, "UI", "probe skip why=prologue fn=begin_scene got=[0, 0]")
    else:
        a(0.6, "UI", "probe off ([ui] probe_scaleform = false)")
    a(1.0, "MOVE", "resync why=world_enter d=0.00m chunk_d=0.00m")
    a(1.0, "MOVE", "on pos=(0.00,0.00,0.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
    a(1.0, "MOVE", f"{'WARN ' if src == 'fallback' else ''}filter live phys338=0x200000{layer:x} phys348=0x2000004 physc8=0x{layer:x} layer={layer} cast=0x{layer if src == 'live' else 3:x} src={src} mode={mode}")
    for row in ("0x58", "0x5b", "0x03"):
        a(1.0, "MOVE", f"filter table row={row} live=00 static={'match' if table == 'match' else 'DIFF want=11'}" if table != "skip" else f"filter table row={row} skip why=vtable")
    for k in range(blocked):
        a(100.0 + 50 * k, "MOVE", "blocked wish=190.0 speed=0.02 slope_er=31.0deg mode=player cast=0x3 hits=map:10/obj:0/both:0 dt=0.40s pos=(1.00,2.00,3.00)")
        a(100.3 + 50 * k, "MOVE", "assist n=1 why=stall hold=0.8s speed=0.02 slope_er=31.0 stalled=0.40s mode=player cast=0x3 pos=(1.00,2.00,3.00)")
        a(100.3 + 50 * k, "MOVE", "suspend reason=assist our controller stalled (hold 0.8 s) ticks=100 pos=(1.00,2.00,3.00)")
        if not assist_no_on:
            a(101.3 + 50 * k, "MOVE", "resync why=handback d=0.00m chunk_d=0.00m")
            a(101.3 + 50 * k, "MOVE", "on pos=(1.00,2.20,3.50) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
    for k in range(capped):
        a(300.0 + k, "MOVE", "WARN assist capped n=8 within 60 s: the detector rests until t=360.0")
    if f5_climb:
        for k in range(3):
            a(400.0 + 20 * k, "MOVE", "off reason=f5 MW2 mode off ticks=500 pos=(5.00,1.00,5.00)")
            a(405.0 + 20 * k, "MOVE", "on pos=(6.00,2.80,6.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
    a(500.0, "MOVE", f"mantle check ok=0 why=no_face rise=0.00m rays=4 n={no_face} pos=(0.00,0.00,0.00) fwd=(0.00,1.00) hits=map:1/obj:0/both:0")
    # a talk: latch at 600, dialog index 3 open 600.02-604, released 2 s after (or 20 s with the bug)
    a(600.0, "FP", "latch set signal=talk_id baseline=1000 now=Some(204101000)")
    a(600.0, "FP", "off reason=talk talk_id Some(1000)->Some(204101000) ticks_on=500 cam_writes=0 (re-arms when clear)")
    a(600.02, "MENU", "idx=3 0->1")
    a(604.0, "MENU", "idx=3 1->0")
    a(606.0 if not talk_late else 624.0, "FP", f"latch released signal=talk_id why={'closed' if not talk_late else 'stale'} after 6.0s value=Some(204101000)")
    a(606.6 if not talk_late else 624.6, "FP", "on eye=(0.00,1.60,0.00) cam_gate=ok")
    # F6 during a cutscene hold
    a(700.0, "FP", "latch set signal=fade plate=2 a=0.51 thr=0.50")
    a(700.0, "FP", "off reason=cutscene fade plate=2 a=0.51 ticks_on=100 cam_writes=0 (re-arms when clear)")
    if f6_bug:
        a(701.0, "FP", "toggle F6 wanted=0")
        a(703.0, "FP", "latch released signal=fade why=returned after 3.0s value=Some(false)")
        a(703.0, "FP", "blocked reason=f6 ")
    else:
        a(701.0, "FP", "toggle F6 override reason=cutscene released=fade wanted=1")
        a(701.6, "FP", "on eye=(0.00,1.60,0.00) cam_gate=ok")
    # UI yields
    a(800.0, "UI", "yield on reason=popup idx=[] popups=1 announce=0 status=0/0 mode=lower")
    if not yield_stuck:
        a(803.0, "UI", "yield off dur=3.00s why=clear")
    else:
        a(900.0, "UI", "yield off dur=100.00s why=clear")
    # hits and numbers
    for k in range(50):
        a(1000.0 + k, "HIT", f"chr=c4311 hp 219->170 dmg=49 band=79000100 mw2=40 d=10.0m via=impact n=1 kill=0")
        if k >= dmg_missing and dmg_on:
            a(1000.0 + k, "DMGNUM", f"n=1 handle=0x1 dmg=49 kill=0 head=0 via=impact src=gun")
    # PASS B: the pad
    if pad == "fail":
        a(0.7, "PAD", "hook FAIL MH_CreateHook 0x7ffb00001000: MH_ERROR_UNSUPPORTED_FUNCTION; no gamepad (keyboard and mouse unaffected)")
    elif pad is not None:
        a(0.7, "PAD", "hook ok xinput1_4=0x7ffb00001000 first_byte=0x48 overlay=GameOverlayRenderer64:yes")
        a(0.8, "PAD", "cfg enabled=1 hooked=1 route=mouse graph=3 rates=260/90/90/55 sens=1.00 accel=1(1200/400) dz=0.18/0.20 trig=30 hold_ms=300 deg_per_count=auto interact_synth=y toggle=l3+r3 fp=dpad_left blank_rs=1 blank_ls=1 rumble=0")
        if pad == "none":
            a(61.0, "PAD", "none seen in 60s (enable Steam Input for PlayStation pads) calls=0")
        elif pad == "silent":
            pass
        else:
            a(5.0, "PAD", "connected slots=0x1 selected=0")
            a(10.0, "PAD", "stat calls=600 slot=0 connected=1 pkt=100 blanked=0 look_counts=0 deg_per_count=0.0800 src=provisional drive=0 via_mouse=0")
            a(200.0, "PAD", f"stat calls=12000 slot=0 connected=1 pkt=2000 blanked={pad_blanked} look_counts=5400 deg_per_count=0.0450 src={'calib' if pad_calib else 'provisional'} drive=1 via_mouse=1")
            if pad_calib:
                a(150.0, "PAD", "calib deg_per_count=0.0450 n=15 (ER camera yaw / counts delivered, median)")
            a(1298.8, "PAD", "interact hold n=1 synth=y")
            a(1299.3, "MOVE", "suspend reason=interact E pressed (hold 1.5 s) ticks=100 pos=(1.00,2.00,3.00)")
    a(1300.0, "MOVE", "off reason=f5 MW2 mode off ticks=60000 pos=(0.00,0.00,0.00)")
    return L


C3_MUTATIONS = [
    ("rays rollback mode in force", dict(mode="union"), "ray filter"),
    ("blocked episodes not assisted (5 in 21 min)", dict(blocked=0, f5_climb=True), "traversal"),
    ("assist without MOVE on", dict(assist_no_on=True), "assist"),
    ("assist cap storm", dict(capped=6), "assist"),
    ("no_face 1000 in ~21 min", dict(no_face=1000), "no_face rate"),
    ("talk latch released 20 s after the dialog", dict(talk_late=True), "fp talk latch"),
    ("F6 during a cutscene hold turns FP off", dict(f6_bug=True), "fp f6"),
    ("yield stuck 100 s", dict(yield_stuck=True), "ui yield"),
    ("damage numbers missing for 5 of 50 hits", dict(dmg_missing=5), "damage numbers"),
    ("damage numbers off but drawn", dict(dmg_on=False, dmg_missing=0), None),
    ("probe installed without hooks", dict(probe="skip"), "ui probe"),
    ("pad hook failed", dict(pad="fail"), "pad hook"),
    ("pad hooked, no connected pad and no 'none seen' line", dict(pad="silent"), "pad hook"),
    ("pad connected while driving, nothing blanked", dict(pad_blanked=0), "pad blank"),
    ("stick look without a calibration", dict(pad_calib=False), "pad look"),
]


def c3c_session(twins=True, seen=True, fx_ratio=1.0, marker=True, big_kill=False, coop_kills=True, census_coop=True,
                caller_marker=True, fp_anim="hybrid", unsticks=0, wg_loaded=34, wg_line=True, rows_missing=False, remote=False,
                survivors=0, left=0, local_kills=60, host_hold=0, prefix_remote=False, tocc="on"):
    """A two-player PASS C session of one PC (host): a peer joins, MW2 mode on, shots, twins both ways, a co-op nuke."""
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))
    a(0.0, "BOOT", "mw2er 0.4.0")
    a(0.4, "COOP", "vis cfg twins=1 interval_ms=40 effects=1 sounds=1 tracers=1 markers=1 hide_er_weapon=1 world_guns=1 self_tp=1"
      + (" tracer_occl=1 tracer_rays=32" if tocc != "old" else ""))
    a(0.5, "COOP", "worldguns loaded guns=%d/34 lods=68 bytes=9000000 ms=900" % wg_loaded)
    a(1.0, "MOVE", f"cfg enabled=1 tp_anim=hybrid sprint=press unstick=2.5s assist=0.15mps fp_anim={fp_anim} rm_mps=4.1")
    a(1.0, "MOVE", "on pos=(0.00,0.00,0.00) vel0=0.0 er_ground=1/1/1 falling=0 grav_mult=1.000 grav_dis=0 r=0.40 h=1.50 fp=1")
    a(5.0, "COOP", "session peers=1 was=0 joined=[0xffffffff16f00001] left=[] handles=[0xffffffff16f00001:type=0:st=3:upd=4] npc=0 lobby=3 role=host sess_players=2")
    if rows_missing:
        a(5.1, "COOP", "WARN rows rows.json has no coop block: twins and markers off (rebuild the regulation)")
    a(10.0, "MODE", "on weapon=ump45_mp mag=25/250 menu_baseline=[] show_cursor=0")
    if marker:
        a(10.2, "COOP", "marker sent sp=79001090 call_ok=1 peers=1")
        a(10.2, "COOP", "marker sent sp=79001110 call_ok=1 peers=1")
    a(11.0, "COOP", "marker seen peer=0xffffffff16f00001 sp=79001090 dt=6000ms")
    a(11.0, "COOP", "marker seen peer=0xffffffff16f00001 sp=79001109 dt=6000ms")
    if wg_line:
        a(11.0, "COOP", "worldgun peer=0xffffffff16f00001 gun=ak47_mp g=9 src=marker lod=0 occl=0 dist=6.0m")
    for k in range(20):
        a(20.0 + 0.1 * k, "SHOT", f"n={k + 1} mag={24 - k}/250 id=79112000 mw2=40 d=12.0m src=chr spread=0.00 origin=(0.00,1.60,0.00) dir=(0.000,0.000,1.000) gun=ump45_mp")
    if twins:
        a(25.0, "COOP", "twin sent gun=ump45_mp row=79812000 n=20/5s refused=0 peers=1")
    if seen:
        a(25.0, "COOP", "twin seen peer=0xffffffff16f00001 row=79809000 n=30/5s known_peer=1")
        a(25.0, "COOP", f"peer fx muzzle={int(30 * fx_ratio)} impact={int(30 * fx_ratio)} expl=0 sounds={int(30 * fx_ratio)} nuke_markers=0/5s")
        if tocc == "on":
            a(25.0, "COOP", "tracer occl draws shown=40 partial=12 hidden=30 clipped=9 rays=260 budget_out=0 no_rays=0 flashes_hidden=0 streaks_hidden=0/5s")
        elif tocc == "norays":
            a(25.0, "COOP", "tracer occl draws shown=82 partial=0 hidden=0 clipped=0 rays=0 budget_out=0 no_rays=300 flashes_hidden=0 streaks_hidden=0/5s")
    for k in range(10):
        a(30.0 + k, "HIT", f"chr=c4311 hp 219->170 dmg=49 band=79112000 mw2=40 d=10.0m via=impact n=1 kill=0 gun=ump45_mp")
    a(41.0, "HIT", "peer_drop chr=c2500 hp=4937->0 dmg=4937 kill=1 band=79109000 atk=150 pellets=1 factor=1.5 peers=1 via=impact n=1")
    if big_kill:
        a(42.0, "HIT", "chr=c2500 hp 4937->0 dmg=4937 band=79109000 mw2=40 d=20.6m via=impact n=1 kill=1 gun=ak47_mp")
    for k in range(unsticks):
        a(50.0 + 10 * k, "MOVE", "unstick n=1 hold=1.0s window=2.50s moved=0.10m wish=150/150 pushed=20 inside=0")
    # the co-op nuke (the caller here)
    if remote:
        a(100.0, "NUKE", "marker from=0xffffffff16f00001 via=speffect action=start peers=1")
        a(100.0, "NUKE", "activate test=1 streak=3 life=1 coop_peers=1 role=host remote=0xffffffff16f00001 coop_mode=owned")
    else:
        a(100.0, "NUKE", "activate test=0 streak=25 life=1 coop_peers=1 role=host remote=0 coop_mode=owned")
        if caller_marker:
            a(100.0, "NUKE", "marker sent sp=79001190 bullet=79000963 ok=1 peers=1")
    a(110.0, "NUKE", "detonate t=10.00 test=0")
    a(111.5, "NUKE", f"census pass=0 by_distance=119 chrsets=1 union=120 in_radius=120 kill=60 deferred=0 skipped=60 other_frame=0 by_team={{6:60}} radius_m=200 spare_friendly=1 census_us=900 upd={{local:60,remote:40,unknown:20}} net_auth={{0:120}} dist_auth=0 held=0 role=host hold={host_hold} coop={int(census_coop)} no_radius=1")
    a(116.0, "NUKE", f"kills total={60 + survivors} killed={local_kills} survivors={survivors} skipped=60 in_radius=120 would_apply=0 armed=1 other_frame=0 census_us_max=900 horizon=3/3 t_ms=6000")
    if coop_kills:
        if prefix_remote:
            a(118.0, "NUKE", "coop kills local=0 remote_skipped=8 window=6.0s peers=1 remote=0")
        else:
            a(118.0, "NUKE", f"coop kills local={local_kills} left={left} held=0 role=host fallback_s=1.00 window=6.0s peers=1 remote=0")
    a(114.5, "NUKE", "player dead t_ms=300")
    a(200.0, "MOVE", "off reason=f5 MW2 mode off ticks=60000 pos=(0.00,0.00,0.00)")
    return L


C3C_MUTATIONS = [
    ("shots with a peer, no twin sent", dict(twins=False), "twins"),
    ("the twin rows missing", dict(rows_missing=True), "twins"),
    ("peer twins seen, no peer effects", dict(fx_ratio=0.5), "peer fx"),
    ("MW2 mode with a peer, no marker", dict(marker=False), "markers"),
    ("the host's nuke kill credited (dmg 4937 kill=1)", dict(big_kill=True), "peer drop"),
    ("co-op nuke without coop kills", dict(coop_kills=False), "coop nuke"),
    ("co-op nuke census without the co-op rules", dict(census_coop=False), "coop nuke"),
    ("the caller sent no marker", dict(caller_marker=False), "coop nuke"),
    ("review C3: the C2 failure (44 survivors of 51)", dict(survivors=44, local_kills=7), "coop nuke"),
    ("2026-10-24: every enemy skipped as remote_owner, nothing killed", dict(prefix_remote=True), "coop nuke"),
    ("2026-10-24 fix: the host held its kills", dict(host_hold=1), "coop nuke"),
    ("2026-10-24 fix: enemies left alive unapplied at the window end", dict(left=8), "coop nuke"),
    ("first person still blanks W/A/S/D", dict(fp_anim="blank"), "fp locomotion"),
    ("unstick storm with root motion", dict(unsticks=8), "fp locomotion"),
    ("no world model loaded", dict(wg_loaded=0), "world guns"),
    ("peer gun marker seen, no world gun", dict(wg_line=False), "world guns"),
    ("2026-10-24: tracer occlusion on but it never ran", dict(tocc="missing"), "tracer occl"),
    ("2026-10-24: tracer occlusion without a map ray", dict(tocc="norays"), "tracer occl"),
]


def pair_logs(**kw):
    """Host and friend logs of one PASS C session (the friend's clock 1 h ahead)."""
    host = c3c_session()
    # the friend shot other characters (its own transitions)
    friend = [(t + 3600.0 + 0.4, tag, msg.replace("role=host", "role=client").replace("chr=c4311 hp 219->170", "chr=c4300 hp 300->251"))
              for t, tag, msg in c3c_session(remote=True)]
    if kw.get("friend_no_seen"):
        friend = [x for x in friend if not x[2].startswith("twin seen ")]
    if kw.get("friend_no_marker_seen"):
        friend = [x for x in friend if not x[2].startswith("marker seen ")]
    if kw.get("double"):
        friend.append((3600.0 + 30.2, "HIT", "chr=c4311 hp 219->170 dmg=49 band=79109000 mw2=40 d=10.0m via=impact n=1 kill=0 gun=ak47_mp"))
    if kw.get("friend_credit"):
        friend.append((3600.0 + 111.6, "HIT", "chr=c4430 hp 268->0 dmg=268 band=79109000 mw2=40 d=20.0m via=last_hit_by n=1 kill=1 gun=ak47_mp"))
    if kw.get("friend_no_start"):
        friend = [x for x in friend if not x[2].startswith("marker from=")]
    if kw.get("host_left_40"):
        # the host left 40 NPCs to the friend's game, which killed only 5 of them
        host = [x for x in host if not x[2].startswith("coop kills local=")] + [(118.0, "NUKE", "coop kills local=60 remote_skipped=40 window=6.0s peers=1 remote=0")]
        friend = [x for x in friend if not x[2].startswith("coop kills local=")] + [(3600.0 + 118.4, "NUKE", "coop kills local=5 remote_skipped=0 window=6.0s peers=1 remote=0xffffffff16f00002")]
    if kw.get("both_remote"):
        # the 2026-10-24 run: both games skipped every live enemy as the other's (8 and 9), neither killed one
        host = [x for x in host if not x[2].startswith("coop kills local=")] + [(118.0, "NUKE", "coop kills local=0 remote_skipped=8 window=6.0s peers=1 remote=0")]
        friend = [x for x in friend if not x[2].startswith("coop kills local=")] + [(3600.0 + 118.4, "NUKE", "coop kills local=0 remote_skipped=28 window=6.0s peers=1 remote=0xffffffff16f00001")]
    return to_entries(host), shift(to_entries(friend), 3600.0)


PAIR_MUTATIONS = [
    ("the friend never sees the host's twins", dict(friend_no_seen=True), "twins"),
    ("the friend never sees the MW2-mode marker", dict(friend_no_marker_seen=True), "markers"),
    ("one hit credited on both PCs", dict(double=True), "no double damage"),
    ("the friend credits a kill of the host's nuke", dict(friend_credit=True), "peer drop"),
    ("the marker did not cross (no remote start)", dict(friend_no_start=True), "coop nuke"),
    ("review C3: NPCs left to the peer's game survive (40 skipped, 5 killed)", dict(host_left_40=True), "coop nuke"),
    ("2026-10-24: both games skip every enemy as the other's", dict(both_remote=True), "coop nuke"),
]


def self_test():
    out, ok, caught = [], True, 0
    good = {n: (s, d) for n, s, d in rows(to_entries(c3_session()))}
    want_na = {"ui probe", *C3C_ROWS}
    bad = [f"{n}: {good[n][0]} {good[n][1][:90]}" for n in C3_ROWS if good[n][0] != ("N/A" if n in want_na else "PASS")]
    if bad:
        ok = False
    out.append(f"good C3 session: {sum(1 for n in C3_ROWS if good[n][0] == 'PASS')}/{len(C3_ROWS) - len(want_na)} rows PASS, ui probe N/A (off)" + (f" ({bad})" if bad else ""))
    st = {n: s for n, s, _ in rows(to_entries(c3_session(probe="ok")))}
    if st["ui probe"] != "PASS":
        ok = False
        out.append(f"MISSED: probe with both hooks must PASS: {st['ui probe']}")
    st = {n: s for n, s, _ in rows(to_entries(c3_session(src="fallback", layer=5)))}
    if st["ray filter"] != "PASS":
        ok = False
        out.append(f"MISSED: a fallback cast must PASS with a WARN: {st['ray filter']}")
    st = {n: s for n, s, _ in rows(to_entries(c3_session(pad="none")))}
    if (st["pad hook"], st["pad blank"], st["pad look"]) != ("PASS", "N/A", "N/A"):
        ok = False
        out.append(f"MISSED: no pad seen (the explicit line) must be pad hook PASS, blank / look N/A: {st}")
    st = {n: s for n, s, _ in rows(to_entries(c3_session(pad=None)))}
    if (st["pad hook"], st["pad blank"], st["pad look"]) != ("N/A", "N/A", "N/A"):
        ok = False
        out.append(f"MISSED: a log without PAD lines must be N/A on the pad rows: {st}")
    st = {n: s for n, s, _ in rows(to_entries(c3_session(table="skip")))}
    if st["filter table"] != "N/A":
        ok = False
        out.append(f"MISSED: skipped table rows must be N/A: {st['filter table']}")
    for label, kw, row in C3_MUTATIONS:
        lines = c3_session(**kw)
        if row is None:  # numbers drawn while off
            lines = [x for x in lines if not (x[1] == "HUD")] + [(0.5, "HUD", "dmgnum cfg on=0 life=0.90")] + [(1000.5, "DMGNUM", "n=1 handle=0x1 dmg=49 kill=0 head=0 via=impact src=gun")]
            row = "damage numbers"
        r = {n: (s, d) for n, s, d in rows(to_entries(lines))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:90]}")
    old = [(0.0, "BOOT", "mw2er 0.3.0"), (1.0, "MOVE", "on pos=(0.00,0.00,0.00) vel0=0.0"), (2.0, "HIT", "chr=c4311 hp 219->170 dmg=49")]
    st = {n: s for n, s, _ in rows(to_entries(old))}
    if any(v != "N/A" for v in st.values()):
        ok = False
        out.append(f"MISSED: a pre-C3 log without talk / F6 must be N/A everywhere: {st}")
    # PASS C: a good two-player session of each PC passes every PASS C row, each mutation fails its row
    for remote in (False, True):
        g = {n: (s, d) for n, s, d in rows(to_entries(c3c_session(remote=remote)))}
        badc = [f"{n}: {g[n][0]} {g[n][1][:90]}" for n in C3C_ROWS if g[n][0] != "PASS"]
        if badc:
            ok = False
            out.append(f"MISSED: the good PASS C session ({'receiver' if remote else 'caller'}) must PASS every PASS C row: {badc}")
    for label, kw, row in C3C_MUTATIONS:
        r = {n: (s, d) for n, s, d in rows(to_entries(c3c_session(**kw)))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:90]}")
    pg = {n: (s, d) for n, s, d in pair_rows(*pair_logs())}
    badp = [f"{n}: {pg[n][0]} {pg[n][1][:80]}" for n in PAIR_ROWS if pg.get(n, ("?",))[0] != "PASS"]
    if badp:
        ok = False
        out.append(f"MISSED: a good pair must PASS every pair row: {badp}")
    for label, kw, row in PAIR_MUTATIONS:
        r = {n: (s, d) for n, s, d in pair_rows(*pair_logs(**kw))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: pair {label} -> {row} {r[row][0]} {r[row][1][:90]}")
    out.append(f"{caught}/{len(C3_MUTATIONS) + len(C3C_MUTATIONS) + len(PAIR_MUTATIONS)} C3 mutations FAIL their row "
               f"(PASS A/B {len(C3_MUTATIONS)}, PASS C {len(C3C_MUTATIONS)}, pair {len(PAIR_MUTATIONS)})")
    # the known-bad replay of the C2 host log (read-only)
    if C2_HOST_LOG.exists():
        st = {n: (s, d) for n, s, d in rows(last_session(parse(C2_HOST_LOG.read_text(encoding="utf-8", errors="replace"))))}
        fp_fail = st["fp talk latch"][0] == "FAIL" and st["fp f6"][0] == "FAIL"
        if not fp_fail or st["coop nuke"][0] != "FAIL":
            ok = False
        out.append(f"known-bad C2 host log replay: fp talk latch {st['fp talk latch'][0]} ({st['fp talk latch'][1][:90]}), fp f6 {st['fp f6'][0]} ({st['fp f6'][1][:90]}), coop nuke {st['coop nuke'][0]} ({st['coop nuke'][1][:90]})")
    else:
        ok = False
        out.append(f"known-bad C2 host log missing: {C2_HOST_LOG}")
    return ok, caught, out


def main(argv):
    if argv == ["--self-test"]:
        ok, caught, lines = self_test()
        for line in lines:
            print(("  " if ok else "  ") + line)
        n_mut = len(C3_MUTATIONS) + len(C3C_MUTATIONS) + len(PAIR_MUTATIONS)
        print(f"{'PASS' if ok else 'FAIL'} analyze_c3 self-test: {caught}/{n_mut} mutations caught")
        return 0 if ok else 1
    if argv and argv[0] == "--pair":
        args = argv[1:]
        skew = 3600.0
        if "--skew" in args:
            i = args.index("--skew")
            skew = float(args[i + 1])
            del args[i:i + 2]
        if len(args) != 2:
            print(__doc__)
            return 2
        a = last_session(parse(pathlib.Path(args[0]).read_text(encoding="utf-8", errors="replace")))
        b = shift(last_session(parse(pathlib.Path(args[1]).read_text(encoding="utf-8", errors="replace"))), skew)
        res = pair_rows(a, b)
        for n, s, d in res:
            print(f"{s:<10} {n:16} {d}")
        passed = all(s in ("PASS", "N/A") for _, s, _ in res)
        print(f"C3 PAIR ROWS (PASS C): {'PASS' if passed else 'FAIL'} (friend clock shifted by {skew:.0f} s)")
        return 0 if passed else 1
    if len(argv) != 1:
        print(__doc__)
        return 2
    entries = last_session(parse(pathlib.Path(argv[0]).read_text(encoding="utf-8", errors="replace")))
    res = rows(entries)
    for n, s, d in res:
        print(f"{s:<10} {n:16} {d}")
    passed = all(s in ("PASS", "N/A") for _, s, _ in res)
    print(f"C3 ROWS (PASS A + B + C): {'PASS' if passed else 'FAIL'}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
