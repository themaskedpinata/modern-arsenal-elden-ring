"""Co-op C2 rows of analyze_log.py (docs/COOP_PLAN.md; MODLOG 2026-10-19): the read-only peer watch and the nuke's
co-op rules. Imported by analyze_log.py; pure Python, read-only.

- coop session: N/A without a `COOP watch` line (a DLL before C2). Every `COOP session peers=<n> was=<m> joined=[..]
  left=[..] handles=[..]` line must be consistent: n == number of handles, was == the previous line's n,
  n == was + |joined| - |left|. PASS with the watch line (solo: `peers_seen=0` and no session line is a PASS too);
  INFO detail carries the role (lobby state) seen.
- coop nuke: N/A without a `COOP watch` line. Uses the NUKE cfg `allow_in_coop=` (absent: the C2 default 1).
  allow_in_coop=1: every `NUKE activate` carries `coop_peers=` equal to the peer count in force at that time, and
  with peers > 0 a `role=` that is host / client / unknown; a `blocked reason=coop_session` is a FAIL.
  allow_in_coop=0: an `activate` while peers > 0 is a FAIL; a sequence running when the count goes above 0 must
  get `cancel reason=coop_session refund=<1, 0 for test>` (or have detonated) within 1 s; key 4 with peers > 0 must
  log `blocked reason=coop_session peers=<n>` with the count in force.
  NEEDS-DATA when no nuke was called while a peer was present.
"""
import re

COOP_ROWS = ("coop session", "coop nuke")
CANCEL_S = 1.0

SESSION_RE = re.compile(r"^session peers=(\d+) was=(\d+) joined=\[([^\]]*)\] left=\[([^\]]*)\] handles=\[([^\]]*)\] npc=(\d+)"
                        r"(?: lobby=(\S+) role=(\w+) sess_players=(\S+))?")
ACT_RE = re.compile(r"^activate test=([01]) .*?(?: coop_peers=(\d+) role=(\w+))?$")
BLOCK_RE = re.compile(r"^blocked reason=coop_session peers=(\d+)")
CANCEL_RE = re.compile(r"^cancel reason=coop_session refund=([01])")


def _list(s):
    return [x for x in s.split(",") if x]


def watch_present(entries):
    return any(e["tag"] == "COOP" and e["msg"].startswith("watch ") for e in entries)


def peer_changes(entries):
    """[(t, n, match)] of the COOP session lines."""
    out = []
    for e in entries:
        if e["tag"] == "COOP" and (m := SESSION_RE.match(e["msg"])):
            out.append((e["t"], int(m.group(1)), m))
    return out


def peers_at(changes, t):
    n = 0
    for tc, nc, _ in changes:
        if tc <= t:
            n = nc
    return n


def judge_session(entries):
    if not watch_present(entries):
        return "N/A", "no 'COOP watch' line (a DLL before co-op C2)"
    ch = peer_changes(entries)
    bad = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("session ") and not SESSION_RE.match(e["msg"])]
    problems = [f"malformed: {e['msg'][:80]}" for e in bad]
    prev = 0
    roles = set()
    for t, n, m in ch:
        was, joined, left, handles = int(m.group(2)), _list(m.group(3)), _list(m.group(4)), _list(m.group(5))
        if n != len(handles):
            problems.append(f"t={t:.2f}: peers={n} but {len(handles)} handle(s)")
        if was != prev:
            problems.append(f"t={t:.2f}: was={was} but the previous line said peers={prev}")
        if n != was + len(joined) - len(left):
            problems.append(f"t={t:.2f}: peers={n} != was {was} + joined {len(joined)} - left {len(left)}")
        if m.group(8):
            roles.add(m.group(8))
        prev = n
    if problems:
        return "FAIL", f"{len(problems)} inconsistent COOP session line(s): {problems[0]}"
    w = next(e["msg"] for e in entries if e["tag"] == "COOP" and e["msg"].startswith("watch "))
    if not ch:
        return "PASS", f"solo session ({w}); no peer change"
    mx = max(n for _, n, _ in ch)
    return "PASS", f"{len(ch)} peer change(s), max peers {mx}, final {ch[-1][1]}; roles seen {sorted(roles) or ['-']}; {w}"


def allow_in_coop(entries):
    for e in reversed(entries):
        if e["tag"] == "NUKE" and (m := re.search(r"\ballow_in_coop=([01])", e["msg"])):
            return m.group(1) == "1"
    return True


def judge_nuke(entries):
    if not watch_present(entries):
        return "N/A", "no 'COOP watch' line (a DLL before co-op C2)"
    ch = peer_changes(entries)
    allow = allow_in_coop(entries)
    nuke = [e for e in entries if e["tag"] == "NUKE"]
    problems, with_peers = [], 0
    for e in nuke:
        n = peers_at(ch, e["t"])
        if (m := ACT_RE.match(e["msg"])):
            if m.group(2) is None:
                problems.append(f"t={e['t']:.2f}: activate without coop_peers= (C2 DLL)")
                continue
            logged = int(m.group(2))
            if logged != n:
                problems.append(f"t={e['t']:.2f}: activate coop_peers={logged} but {n} peer(s) in force")
            if n > 0:
                with_peers += 1
                if not allow:
                    problems.append(f"t={e['t']:.2f}: activate with {n} peer(s) and allow_in_coop=0")
                if m.group(3) not in ("host", "client", "unknown"):
                    problems.append(f"t={e['t']:.2f}: activate role={m.group(3)} with peers")
        elif (m := BLOCK_RE.match(e["msg"])):
            with_peers += 1
            if allow:
                problems.append(f"t={e['t']:.2f}: blocked reason=coop_session with allow_in_coop=1")
            if int(m.group(1)) != n or n == 0:
                problems.append(f"t={e['t']:.2f}: blocked peers={m.group(1)} but {n} peer(s) in force")
        elif (m := CANCEL_RE.match(e["msg"])):
            if allow:
                problems.append(f"t={e['t']:.2f}: cancel reason=coop_session with allow_in_coop=1")
    if not allow:
        # a countdown running when a peer arrives is cancelled (or had detonated) within CANCEL_S
        for t, n, m2 in ch:
            if int(m2.group(2)) != 0 or n == 0:
                continue
            # the sequence state at t
            running, test = None, False
            for e in nuke:
                if e["t"] > t:
                    break
                if (m := ACT_RE.match(e["msg"])):
                    running, test = e["t"], m.group(1) == "1"
                elif e["msg"].startswith(("end reason=", "cancel reason=", "det ", "detonate")):
                    running = None
            if running is None:
                continue
            after = [e for e in nuke if t <= e["t"] <= t + CANCEL_S]
            c = [e for e in after if CANCEL_RE.match(e["msg"])]
            if not c and not any(e["msg"].startswith(("det ", "detonate", "end reason=")) for e in after):
                problems.append(f"t={t:.2f}: a peer joined during a countdown and no 'cancel reason=coop_session' within {CANCEL_S} s")
            for e in c:
                want = "0" if test else "1"
                if CANCEL_RE.match(e["msg"]).group(1) != want:
                    problems.append(f"t={e['t']:.2f}: coop cancel refund={CANCEL_RE.match(e['msg']).group(1)} (want {want})")
    if problems:
        return "FAIL", f"{len(problems)} problem(s): {problems[0]}"
    if with_peers == 0:
        return "NEEDS-DATA", f"allow_in_coop={int(allow)}; no nuke key press while a peer was in the world (C2: host presses 4 with the friend in)"
    return "PASS", f"allow_in_coop={int(allow)}; {with_peers} nuke press(es) with peers, every line consistent with the peer count"


def rows(entries, ctx=None):
    return [("coop session", *judge_session(entries)), ("coop nuke", *judge_nuke(entries))]


# ---- synthetic sessions (self-test) -----------------------------------------------------------------------------

H1 = "0xffffffff00000002"
H2 = "0xffffffff00000003"


def coop_session(allow=True, bad_count=False, bad_was=False, act_no_peers_field=False, act_wrong_peers=False,
                 blocked_with_allow=False, act_blocked_mode=False, no_cancel=False, cancel_refund0=False):
    L = []

    def a(t, tag, msg):
        L.append((t, tag, msg))
    a(0.0, "BOOT", "mw2er 0.3.0")
    a(0.5, "NUKE", f"cfg enabled=1 always_available=0 allow_in_coop={int(allow)} cost=25 radius_m=200")
    a(1.0, "COOP", "watch slots=5 peers_seen=0 npc=0 lobby=0 role=unknown sess_players=0")
    # solo activation
    a(5.0, "NUKE", "activate test=0 streak=25 life=0 coop_peers=0 role=solo")
    a(30.0, "NUKE", "end reason=done t=25.0s")
    # the friend joins
    n1 = 2 if bad_count else 1
    a(40.0, "COOP", f"session peers={n1} was=0 joined=[{H1}] left=[] handles=[{H1}:type=1:st=2:upd=4] npc=0 lobby=3 role=host sess_players=2")
    if allow:
        if act_no_peers_field:
            a(50.0, "NUKE", "activate test=0 streak=25 life=1")
        else:
            a(50.0, "NUKE", f"activate test=0 streak=25 life=1 coop_peers={3 if act_wrong_peers else 1} role=host")
        a(50.0, "NUKE", "coop note shown peers=1: Co-op nuke: only your game's enemies are affected; friends won't see it yet")
        a(75.0, "NUKE", "end reason=done t=25.0s")
        if blocked_with_allow:
            a(80.0, "NUKE", "blocked reason=coop_session peers=1 test=0 (allow_in_coop=0)")
    else:
        if act_blocked_mode:
            a(50.0, "NUKE", "activate test=0 streak=25 life=1 coop_peers=1 role=host")
        else:
            a(50.0, "NUKE", "blocked reason=coop_session peers=1 test=0 (allow_in_coop=0)")
    # the friend leaves, comes back during a countdown
    a(90.0, "COOP", f"session peers=0 was={7 if bad_was else n1} joined=[] left=[{H1}] handles=[] npc=0 lobby=3 role=host sess_players=1")
    a(100.0, "NUKE", "activate test=0 streak=25 life=2 coop_peers=0 role=solo")
    a(102.0, "COOP", f"session peers=1 was=0 joined=[{H2}] left=[] handles=[{H2}:type=1:st=3:upd=4] npc=0 lobby=3 role=host sess_players=2")
    if allow:
        a(125.0, "NUKE", "end reason=done t=25.0s")
    elif not no_cancel:
        a(102.1, "NUKE", f"cancel reason=coop_session refund={0 if cancel_refund0 else 1} peers=1")
    return L


COOP_MUTATIONS = [
    ("peers= != number of handles", dict(bad_count=True), "coop session"),
    ("was= != the previous count", dict(bad_was=True), "coop session"),
    ("activate without coop_peers=", dict(act_no_peers_field=True), "coop nuke"),
    ("activate coop_peers= wrong", dict(act_wrong_peers=True), "coop nuke"),
    ("blocked with allow_in_coop=1", dict(blocked_with_allow=True), "coop nuke"),
    ("activate with peers and allow_in_coop=0", dict(allow=False, act_blocked_mode=True), "coop nuke"),
    ("no cancel when a peer joins a countdown (allow 0)", dict(allow=False, no_cancel=True), "coop nuke"),
    ("coop cancel without refund (allow 0)", dict(allow=False, cancel_refund0=True), "coop nuke"),
]


def to_entries(lines):
    out = []
    for t, tag, msg in sorted(lines, key=lambda x: x[0]):
        ms = int(round(t * 1000))
        raw = f"2026-10-19 16:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [task] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def to_log(lines):
    return "\n".join(e["raw"] for e in to_entries(lines)) + "\n"


def self_test():
    """(ok, caught, lines): both good sessions (allow 1 and 0) PASS both rows, each mutation FAILs its row, an older
    log is N/A, a solo C2 log PASSes the session row and is NEEDS-DATA on the nuke row."""
    out, ok, caught = [], True, 0
    for allow in (True, False):
        good = {n: (s, d) for n, s, d in rows(to_entries(coop_session(allow=allow)))}
        bad = [f"{n}: {good[n][0]} {good[n][1][:100]}" for n in COOP_ROWS if good[n][0] != "PASS"]
        if bad:
            ok = False
        out.append(f"good co-op session allow_in_coop={int(allow)}: {sum(1 for n in COOP_ROWS if good[n][0] == 'PASS')}/{len(COOP_ROWS)} rows PASS" + (f" ({bad})" if bad else ""))
    for label, kw, row in COOP_MUTATIONS:
        r = {n: (s, d) for n, s, d in rows(to_entries(coop_session(**kw)))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:80]}")
    old = [(0.0, "BOOT", "mw2er 0.3.0"), (1.0, "NUKE", "activate test=0 streak=25 life=0")]
    st = {n: s for n, s, _ in rows(to_entries(old))}
    if any(v != "N/A" for v in st.values()):
        ok = False
        out.append(f"MISSED: a pre-C2 log must be N/A: {st}")
    solo = [(0.0, "BOOT", "mw2er 0.3.0"), (1.0, "COOP", "watch slots=5 peers_seen=0 npc=0 lobby=0 role=unknown sess_players=0"),
            (5.0, "NUKE", "activate test=1 streak=0 life=0 coop_peers=0 role=solo")]
    st = {n: s for n, s, _ in rows(to_entries(solo))}
    if st != {"coop session": "PASS", "coop nuke": "NEEDS-DATA"}:
        ok = False
        out.append(f"MISSED: a solo C2 log must be session PASS / nuke NEEDS-DATA: {st}")
    out.append(f"{caught}/{len(COOP_MUTATIONS)} co-op mutations FAIL their row")
    return ok, caught, out
