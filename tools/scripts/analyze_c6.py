"""C6 rows (docs/C6_HOLD_DESIGN.md 9-10): the MW2 gun on the ER character's hand and the crossbow stance. Pure Python,
read-only, no game, no package.

  python -I analyze_c6.py <log>                  rows of the last session (a log without a BOOT line = one session)
  python -I analyze_c6.py --self-test            synthetic sessions per row (PASS / FAIL / N/A, one mutation per row),
                                                 the --axes recovery of every det -1 permutation and the pair rows
  python -I analyze_c6.py --axes <log>           the signed permutation for `[coop] hand_axes` from the `HOLD axes` lines
                                                 (MW2 X = the hand axis along the camera forward, MW2 Z = the one along
                                                 world up, MW2 Y the remaining axis with the sign that mirrors: det -1)
  python -I analyze_c6.py --pair <host log> <friend log> --skew 3600   cross-log rows (the friend's clock is `skew`
                                                 seconds ahead: its times are shifted back by it)

Rows (N/A when the feature is off in the session's `HOLD cfg` line, or for a DLL before C6: no `HOLD cfg` line):
- hand        hold_mode=hand: `HOLD hand ok who=self` seen and the self fallback frames in the `HOLD fallback who=self
              reasons=.. frames=k/n 30s` summaries <= 5 % of their frames; FAIL: no hand frame at all although the self
              gun was wanted (MODE on in third person), or more fallbacks.
- fallback    the reasons tally (self and peers); FAIL only on `det` (an inside-out gun was about to be drawn: the axes or
              the chr matrix convention are wrong) or `vtable` (wrong build constants). N/A: no fallback line.
- stance      er_stance=crossbow: every `HOLD stance enter row=X` has `HOLD stance exit row=X restored=1` before the next
              `MODE off` / `FP on` (+1.5 s) and before the session ends; no exit with restored=0, no `stance foreign`.
- fp          no `HOLD stance enter` while first person is on (between `FP on` and `FP off`).
- chord       (C6 part 16.9: reports `HOLD chord verdict=ignored` vs `fail`: an ignored chord is ER busy, retried, never a
              failure; `er_busy` waits are by design; part 16 addendum: `HOLD chord done` / `chord abort_after_played`
              close a refusal) no `HOLD chord refused` episode longer than 10 s without a `HOLD chord sent` or a stance exit (reason=busy, a
              ran-out deferred exit waiting while the player fires / aims / moves in first person, and reason=dead, the
              death screen, are by design); FAIL on `HOLD stance refresh fail` (C6 aim 4: not inside a death span: the
              player's death (`NUKE player dead` / `FP blocked reason=death`) to the respawn's world leave / enter, +1 s:
              chords to a dead player and `took=void` are N/A); WARN (not a failure) on `HOLD stance exit_chords gave_up`.
              N/A: no chord line.
- no ER fire  0 counted `HOLD stray_bullet` lines and no `HOLD stance disabled reason=stray_bullet`. N/A: never entered.
- dead fire   (2026-10-24, "can still shoot after death") no MW2 shot while the player is dead: no `SHOT n=..` line (a
              spawned shot or a `skipped:` one, both used ammo) inside a death span: `SHOT dead gate on` / `FP blocked|off
              reason=death` / `MOVE blocked|suspend reason=death` / `NUKE player dead|streak reset reason=death` to `SHOT dead gate off` (a DLL with
              the gate) or else the respawn's `TASK world entered`. N/A: no death in the session.
- melee       the R3 presses suppressed (`HOLD melee suppressed n=`); informational PASS. N/A: none.
- weapon      (C6 attach 2) hold_mode=hand + source=weapon: `HOLD weapon ok who=self` seen (the gun on ER's own weapon
              model); reports the source used (`HOLD src`), the miss tally (`HOLD weapon miss`) and the learned
              `HOLD weapon axes auto`. FAIL: never ok while the self gun was wanted, or a `weapon_vtable` miss (wrong build
              constants).
- hide        (C6 hide) hide_patch=1 with hand mode or the stance: `HOLD hide row patched` (or `already patched`) before
              the MW2-mode marker works; FAIL on `HOLD hide foreign / refused / restore failed`, on a marker applied with the
              row never patched, and on an expiry gap (two `COOP marker sent sp=79001090` 28..32 s apart with no removal
              between: the hidden weapon blinked back; the renewal `COOP marker renewed` prevents it).
- scope3p     (C6 scope peek) every `FP on ... peek=1` episode: no `HOLD stance exit` / `HOLD chord sent` inside it (the
              stance stays through a scope-in); reports how many ended with reason f6 (ADS released). N/A: no peek.
- grip        (C6 grip 3) hand mode + source=weapon with a grip-3 DLL (`weapon_rake=` in `HOLD cfg`): a `HOLD grip who=self
              gun=.. src=table|config|mixed` line for the gun ER's weapon model carried, `weapon_axes` the rig map
              "-y,+z,+x" or "auto"; FAIL on the first build's backwards guess "+y,+x,+z", on a `HOLD weapon axes auto`
              line without `accepted=` (an old DLL) or with `accepted=1` for another map, and on weapon frames without
              any `HOLD grip` line.
- churn       (C6 pose 3) every `HOLD stance exit_defer ... resumable=1` followed by a stance enter inside the window
              (`exit_defer_s` of the cfg) has `HOLD stance resume` and no `HOLD chord sent` between the exit and 1.5 s
              after the enter; reports the chords per stance enter. N/A: no deferred exit.
- body        (C6 depth fallback) body_occl=1: `HOLD body bones found=` (the pelvis matched), `HOLD body ok who=self` and
              `COOP worldgun body occlusion first frame` while our gun was drawn in third person; FAIL on `HOLD body bones
              none` or no capsule frame although the self gun was drawn. N/A: off.
- depth probe (C6 depth probe P-D, read-only) probe_er_depth=1: `DEPTH probe on`, every `DEPTH probe chain` pins=6/6 with
              a88_eq=1 and module=d3d12:1, `desc A ... rule=ok`, and the 60 s verdict; FAIL on `probe off`, a broken chain or
              a desc rule miss (format / shape / deny_srv / size). Reports desc B and the wrapper's state-like words.
- depth state (C6 depth 5, docs part 15.1) depth_state=1: `DEPTH state hook on` (the read-only ResourceBarrier hook), the 5 s
              `DEPTH state res=..` lines and the last `DEPTH state verdict ready=1`; FAIL on `DEPTH state hook off why=..`, a
              verdict NOT ready (why= the first failing row: write_at_ui / unknown / queues / mismatch / split / ..., then send
              the `DEPTH state path` lines; the world guns keep the capsules); N/A: never in the world, or a verdict on too
              few frames (`why=frames`).
- depth sample (P-S2) depth_sample=1: the samples' conv=reverse; FAIL on `probe sample off why=hresult` or a conv=forward
              sample; WARN when never reverse. N/A: no sample.
- depth self  (P-S2) depth_sample=1: the last `DEPTH probe self ... pass=1`. C6 part 16.8: the line fit z = s*lin(d)+o (s=,
              s_iqr=, o_cm=, lev=lo/hi, n_eff=) and the `zd=` (z * d) of every self_s sample (flat = a pure near scale). C6 round 7 (depth7.md): the surface offset is
              fitted (o_cm in 0..50 cm, >= 90 % of the judged within 10 cm of it, p95 < 25 cm, <= 25 % edges) and the scale
              is proven over a >= 0.6 m span or unproven (live then with margin_rel); FAIL on pass=0 with why= edge / offset /
              spread / p95 / scale (and, an older DLL, pass=0 with >= 6 judged). N/A: no self line / why=few. Reports the
              `DEPTH probe self_s` samples (o= per sample).
- depth live  (15.3) er_depth=1: `DEPTH live on` and the 5 s `DEPTH live ... primed=` > 0; FAIL on `DEPTH live trip` (why=..|self)
              and (part 16.8) on a `live on ... near=` that is not the last passing self line's `n_eff=` (CSCam's near is ~10 %
              short: phantom occlusion); WARN on
              `DEPTH live off why=..` (the prime stayed off: capsules only) or live without a primed frame. N/A: no line.
              The verdict line `C6 DEPTH VERDICT: READY | NOT READY why=..` sums the four depth rows.
- aim pose    (C6 aim 4) ads_pose=er_aim in `HOLD cfg`: every `HOLD aim patch` (not already=1) has its `HOLD aim restore
              ... restored=1` before the next patch / within 1 s of the session end, no `HOLD chord sent` inside an episode
              (when=ads or when=stance), no stray bullet inside it (+1 s), at least one `HOLD aim on` when an episode ran;
              FAIL on `aim foreign / refused / restore failed`, restored=0, `HOLD aim disabled` or only `aim fail
              reason=no_state` (ER's aim state never showed); WARN on `HOLD aim disabled` after an `aim on` (ER busy at two
              presses, an old DLL). Reports the on times, the `HOLD aim align deg=` range and clamps. N/A: off / no episode.
- aim cam     (C6 round 6, docs 14.8) ads_pose=er_aim with aim_cam=keep in `HOLD cfg`: after an `aim on`, `HOLD aim cam keep
              installed` and per armed span `HOLD aim cam kept ms=.. hits_cam=.. hits_ret=..`; no `HOLD aim probe ... prec=1
              camtype=1|2|3` inside a kept span (ER's aim camera ran); FAIL on `aim cam refused`, no install / span line, or a
              span of >= 0.2 s with hits_cam=0 (the filter never matched ER's camera call); WARN on `aim cam unavailable`.
              N/A: aim_cam=er (the escape hatch), a DLL before round 6, no episode. C6 round 7: a `sites=3` install also masks
              ER's snipe body fade: a >= 0.2 s span with hits_fade=0 FAILs; with keep a refused install keeps the aim pose off.
- aim fade    (C6 round 7, aim7.md 1) ads_pose=er_aim + aim_cam=keep: every `snipe=` reading (`HOLD aim probe` / `HOLD aim pose`)
              with prec=1 inside a kept span is >= 0.5 (ER did not fade our body out); FAIL below. N/A: no snipe= field (an
              older DLL) / no kept span.
- aim arms    (C6 round 7 probe P-A7, probe_er_aim; part 16.1 fix) `HOLD aim pose phase=aim ... arm=0|1 ... mode=0|1 row=..`:
              counts ONLY samples of a real ranged row (40..44 million; the row from the pose line, else the last `HOLD aim
              probe row=`) taken standing (the head of q= not walk 20xxx / 22xxx, roll / backstep 5xxx, jump 2xxxxx): mode=0
              = vanilla (probe 0), carrier=1 = the part-16 carrier. PASS when >= 2 consecutive such samples have arm=1
              (prints the row); WARN when they exist but never moved the arms (stop and decide: docs 16.6); N/A "no vanilla
              crossbow sample" otherwise (the round-7 log: every sample was the patched longsword with mode=1). Never FAIL.
- aim anim    (C6 part 16 addendum) every `HOLD aim episode n=.. carrier= mode= row= cam=keep|er|keep>er arm=a->b aim=<id>|none
              aim_at_ms= prec_ms= ads_ms= camtypes=[..] loaded=a->b bolts=a->b` line: per episode whether ER's crossbow aim
              animation (46037xxx) played and under which tp_aim_cam / grip; groups (kind, cam, arm) with hits/episodes.
              PASS when a carrier episode had it (the cam= values printed); WARN when carrier episodes ran without it; N/A
              without a carrier episode (vanilla episodes reported). Never FAIL. Part 16.16: grouped by loaded= too (1 when
              the slot read loaded at the start or the end of the episode); WARN with loaded=1 carrier episodes and no aim
              anim = the loaded round failed too (user decision 2026-10-22: stop the aim-animation work).
- load        (C6 part 16.16, `[coop] carrier_load`, `load=1` in `HOLD cfg`) `HOLD xbow load write ...` then `HOLD xbow load
              method=write slot=.. ok=0|1|- loaded=a->b egd=a->b bolts=a->b ms=..`, `xbow load undo why=..`, `xbow load wait
              reason=..`, `xbow load lost`. PASS = a load verified ok=1 and every load undone (or its swap restored) with the
              bolt count unchanged; FAIL on `xbow load lost` (ER unloaded it itself), a bolt drop across a load, an undo with
              ok=0 (a loaded crossbow left); WARN on ok=0 only (ER does not keep the write), a load still on at the session
              end, or only wait lines (the reasons printed). N/A: load off and no line.
- guard       (C6 part 17, `tp_ads_pose = "guard"` with `er_stance = "polearm"`: `ads_pose=guard` in `HOLD cfg`) every `HOLD
              guard episode n=.. ms= row= route= arm=a->b head0= heads=[..] guard_anim= detected=0|1 via=frame|anim det_ms=
              moved_max= turned_max= align=<deg>|none align_min= clamped= fallback=0|1 fb_ms= yaw=<writes> why=..` line.
              PASS when an episode detected ER's guard (its weapon frame left the standing carry / a guard anim); reports
              det_ms, the align range, clamps (review: debounced, un-latching; clamped=1 = it held >= 0.2 s once), fallbacks
              (the gun raise), yaw writes, the head ids and guard anim ids. Part 17 addendum (docs 17.12): the episode's
              `corr=builtin|auto|fixed|off` (the guard frame: the barrel along the body's forward from the right fist) -> the
              align range / clamps of the corrected guards, the `HOLD guard frame src=.. fwd_err=` values and the learned
              `HOLD guard frame auto axes= rot= vs_builtin=`; WARN when every corrected guard still clamped; FAIL when an
              undetected episode has corr != off or a `guard frame src=` line falls outside a guard span. WARN:
              episodes ran but none detected (the gun raise only), or `guard yaw write ok=0`. FAIL with ads_pose=guard on any
              `HOLD aim patch` / `aim cam keep installed` / `aim cam kept` (the crossbow aim must stay out), a `HOLD chord
              sent` inside a guard span (`guard on` to its episode line) or a counted stray bullet in it (+1 s). N/A: guard
              off and no line, or no episode.
- shoulder    (C6 part 18, `[third_person]`: the last `CAM shoulder cfg on=1`) the over-the-shoulder camera episodes
              (`CAM shoulder on` .. `CAM shoulder off why= ms= writes= skips= persisted= conflicts= pulls= min_frac=`):
              reports episodes, detour writes, pulls (spring arm) and the off reasons. FAIL: a `shoulder on` strictly inside
              a first-person episode (incl. the scope peek: the offset must be cut / held), or while the latest cfg says
              on=0. WARN: `shoulder unavailable reason=no_cam_hook`, conflicts > 0 (armed while first person / the gunship
              owned the camera; refused by the detour), or an episode >= 1 s with writes=0. N/A: on=0 and no episode.
- carrier     (C6 part 16) carrier=dry|crossbow in `HOLD cfg`: dry = a `HOLD xbow dry ... pick=<id>` line (WARN on pick=none);
              crossbow = every `HOLD xbow on` is followed by a `HOLD xbow restore why=.. ok=1` (or a world leave whose journal a
              later `restore why=journal ok=1` closes); FAIL on `xbow trip`, `xbow refused`, `xbow foreign`, `xbow
              restore_missing`, `xbow restore ... ok=0` (not the world leave's pending line), `xbow off why=prologue|version`
              with the carrier wanted; WARN on a last swap still on at the session end (its journal restores at the next world
              enter) and on `xbow grip fail`. Reports the grip path (arm_style / chord). N/A: carrier off and no xbow line.
- depth control (C6 round 6, docs 15.8) depth_state=1: the last `DEPTH state control B ui=hits/frames calls=.. ok=1` (B's own
              UiBegin barrier seen in >= 90 % of the UI frames: the hook works); FAIL on ok=0 (calls=0 or B's barriers missing:
              the hook sees nothing). N/A: no control line (an older DLL) / not judged yet (ok=-). Reports the top census row.
              The depth state row reads the per-candidate verdicts (`... why=.. cand=A|B`): PASS when a candidate is ready,
              FAIL on why=hook, N/A while only frames / control are missing.
The fp row skips the peek episodes (a peek is third person for the stance); a peek's `FP on` is no stance stop.
Statuses: PASS, FAIL, N/A and WARN (reported, not a failure); `C6 ROWS: PASS` = no FAIL.
Pair rows (`--pair`): peer stance (each side's `HOLD stance enter` while the other has er_stance on and stance_peers=1
is answered by `HOLD peer_stance ... patched=1` or a `shared_row` skip on the other log within 10 s), peer hand (with
hold_mode=hand on a side and the other side in MW2 mode: `HOLD hand ok who=peer=` on that side).
"""
import itertools
import pathlib
import re
import sys

LINE = re.compile(r"^(\d{4}-\d\d-\d\d) (\d\d):(\d\d):(\d\d)\.(\d{3}) ([DIWE]) \[([^\]]*)\] (\S+) ?(.*)$")
ROWS = ("hand", "weapon", "grip", "hide", "fallback", "stance", "fp", "scope3p", "chord", "churn", "no ER fire", "dead fire", "melee", "body", "depth probe", "aim pose",
        "aim cam", "aim fade", "aim arms", "aim anim", "carrier", "load", "guard", "shoulder", "depth state", "depth control", "depth sample", "depth self", "depth live")
DEPTH_ROWS = ("depth state", "depth control", "depth sample", "depth self", "depth live")
# C6 aim 4: WARN = reported, not a failure (an exit chord gave up)
OK_STATES = ("PASS", "N/A", "WARN")
RIG_AXES = "-y,+z,+x"
PAIR_ROWS = ("peer stance", "peer hand")
# C6 part 17: the motion sets of `[coop] er_stance`
STANCE_SETS = ("crossbow", "polearm")
VEC = r"\[(-?[\d.]+),(-?[\d.]+),(-?[\d.]+)\]"
AXES_RE = re.compile(rf"axes hx={VEC} hy={VEC} hz={VEC} cam_fwd={VEC} ads=(\d)")
EXIT_GRACE_S = 1.5
CHORD_STUCK_S = 10.0
PAIR_WINDOW_S = 10.0


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


def hold(entries, prefix):
    return [e for e in entries if e["tag"] == "HOLD" and e["msg"].startswith(prefix)]


def cfg(entries):
    """The last `HOLD cfg` line as a dict (None for a DLL before C6)."""
    c = hold(entries, "cfg ")
    if not c:
        return None
    return dict(kv.split("=", 1) for kv in c[-1]["msg"][4:].split() if "=" in kv)


def spans(entries, tag, on, off):
    """[(t_on, t_off)] of `<tag> on...` / `<tag> off...` lines (open at the end = to the last line)."""
    out, t0 = [], None
    for e in entries:
        if e["tag"] != tag:
            continue
        if e["msg"] == on or e["msg"].startswith(on + " "):
            if t0 is None:
                t0 = e["t"]
        elif (e["msg"] == off or e["msg"].startswith(off + " ")) and t0 is not None:
            out.append((t0, e["t"]))
            t0 = None
    if t0 is not None and entries:
        out.append((t0, entries[-1]["t"]))
    return out


def is_on(msg):
    return msg == "on" or msg.startswith("on ")


def is_peek(msg):
    return is_on(msg) and re.search(r"\bpeek=1\b", msg) is not None


def fp_spans(entries, peek=None):
    """[(t_on, t_off, off_msg)] of the FP episodes: peek=True only the scope peeks (`FP on ... peek=1`), False only the
    user's own first person, None all."""
    out, t0, pk = [], None, False
    for e in entries:
        if e["tag"] != "FP":
            continue
        m = e["msg"]
        if is_on(m):
            if t0 is None:
                t0, pk = e["t"], is_peek(m)
        elif (m == "off" or m.startswith("off ")) and t0 is not None:
            if peek is None or pk == peek:
                out.append((t0, e["t"], m))
            t0 = None
    if t0 is not None and entries and (peek is None or pk == peek):
        out.append((t0, entries[-1]["t"], None))
    return out


def inside(t, sp, pad=0.0):
    return any(a - pad <= t <= b + pad for a, b in sp)


def kv(msg, key):
    m = re.search(rf"\b{re.escape(key)}=(\S+)", msg)
    return m.group(1) if m else None


def judge_hand(entries, c):
    if c is None or c.get("hold_mode") != "hand":
        return "N/A", "hold_mode is not hand"
    ok_lines = [e for e in hold(entries, "hand ok who=self")]
    bad = tot = 0
    for e in hold(entries, "fallback who=self reasons="):
        m = re.search(r"frames=(\d+)/(\d+)", e["msg"])
        if m:
            bad += int(m.group(1))
            tot += int(m.group(2))
    mode = spans(entries, "MODE", "on", "off")
    fp = spans(entries, "FP", "on", "off")
    wanted = sum(max(0.0, b - a) for a, b in mode) - sum(max(0.0, b - a) for a, b in fp)
    if not ok_lines:
        if wanted > 5.0:
            return "FAIL", f"no `HOLD hand ok who=self` in {wanted:.0f} s of MW2 mode in third person ({len(hold(entries, 'fallback who=self reason='))} fallback reasons)"
        return "N/A", "MW2 mode never on in third person"
    frac = bad / tot if tot else 0.0
    src = kv(ok_lines[0]["msg"], "src")
    if frac > 0.05:
        return "FAIL", f"self fallback {bad}/{tot} frames ({frac:.1%}) in the 30 s summaries (> 5 %)"
    return "PASS", f"hand ok src={src} bone={kv(ok_lines[0]['msg'], 'bone')}; self fallback {bad}/{tot} frames in summaries"


def judge_fallback(entries, c):
    tally = {}
    for e in hold(entries, "fallback who="):
        r = kv(e["msg"], "reason")
        if r:
            tally[r] = tally.get(r, 0) + 1
        rs = kv(e["msg"], "reasons")
        if rs:
            for part in rs.split(","):
                k, _, n = part.partition(":")
                tally[k] = tally.get(k, 0) + (int(n) if n.isdigit() else 1)
    if not tally:
        return "N/A", "no fallback line"
    txt = ",".join(f"{k}:{v}" for k, v in sorted(tally.items()))
    if "det" in tally or "vtable" in tally:
        return "FAIL", f"reasons {txt} (det: the axes / chr matrix convention; vtable: wrong build constants)"
    return "PASS", f"reasons {txt}"


def judge_stance(entries, c):
    if c is None or c.get("er_stance") not in STANCE_SETS:
        return "N/A", "er_stance is off"
    enters = hold(entries, "stance enter row=")
    if not enters:
        return "N/A", "the stance never entered"
    exits = hold(entries, "stance exit row=")
    for e in exits:
        if kv(e["msg"], "restored") != "1":
            return "FAIL", f"exit with restored=0 at {e['raw'][:23]}: {e['msg'][:100]}"
    # a row already holding the stance values at first sight is now `stance refused ... reason=foreign` (review fix)
    foreign = hold(entries, "stance foreign") + [e for e in hold(entries, "stance refused") if kv(e["msg"], "reason") == "foreign"]
    if foreign:
        return "FAIL", f"a foreign row: {foreign[0]['msg'][:100]}"
    end = entries[-1]["t"]
    # a scope peek's `FP on ... peek=1` is no stop: the stance stays through a third-person scope-in
    stops = [e["t"] for e in entries if (e["tag"] == "MODE" and e["msg"].startswith("off")) or (e["tag"] == "FP" and is_on(e["msg"]) and not is_peek(e["msg"]))]
    for en in enters:
        row = kv(en["msg"], "row")
        nxt = [x["t"] for x in exits if x["t"] >= en["t"] and kv(x["msg"], "row") == row]
        stop = next((s for s in stops if s >= en["t"]), None)
        limit = (stop + EXIT_GRACE_S) if stop is not None else end + 1e-6
        if not nxt or nxt[0] > limit:
            why = f"before {'the next MODE off / FP on' if stop is not None else 'the session end'}"
            return "FAIL", f"enter row={row} at {en['raw'][11:23]} has no `exit row={row} restored=1` {why}"
    return "PASS", f"{len(enters)} enter(s), {len(exits)} exit(s), every one restored"


def judge_fp(entries, c):
    enters = hold(entries, "stance enter row=")
    if not enters:
        return "N/A", "the stance never entered"
    fp = fp_spans(entries, peek=False)
    bad = [e for e in enters if any(a + 0.3 < e["t"] < b for a, b, _ in fp)]
    if bad:
        return "FAIL", f"stance entered in first person at {bad[0]['raw'][11:23]}"
    return "PASS", f"{len(enters)} enter(s), none in first person (scope peeks skipped)"


def tally_reasons(lines):
    t = {}
    for e in lines:
        r = kv(e["msg"], "reason")
        if r:
            t[r] = t.get(r, 0) + 1
        rs = kv(e["msg"], "reasons")
        if rs:
            for part in rs.split(","):
                k, _, n = part.partition(":")
                t[k] = t.get(k, 0) + (int(n) if n.isdigit() else 1)
    return t


def judge_weapon(entries, c):
    if c is None or c.get("hold_mode") != "hand" or c.get("source") != "weapon":
        return "N/A", "hold_mode is not hand or source is not weapon"
    ok = hold(entries, "weapon ok who=self")
    miss = tally_reasons(hold(entries, "weapon miss who=self"))
    txt = ",".join(f"{k}:{v}" for k, v in sorted(miss.items())) or "none"
    if "weapon_vtable" in miss:
        return "FAIL", f"weapon_vtable misses (wrong build constants of the weapon-model chain): {txt}"
    if not ok:
        if hold(entries, "hand ok who=self") or hold(entries, "fallback who=self"):
            return "FAIL", f"ER's weapon model never used for our gun (misses {txt}); the gun sat on the hand bone / offset"
        return "N/A", "the self gun was never drawn"
    srcs = {}
    for e in hold(entries, "src who=self "):
        s = kv(e["msg"], "src")
        srcs[s] = srcs.get(s, 0) + 1
    auto = hold(entries, "weapon axes auto")
    ax = kv(auto[-1]["msg"], "axes") if auto else ("not learned" if c.get("weapon_axes") == "auto" else c.get("weapon_axes"))
    return "PASS", (f"weapon ok src={kv(ok[0]['msg'], 'src')} d={kv(ok[0]['msg'], 'd')}; misses {txt}; src lines "
                    f"{','.join(f'{k}:{v}' for k, v in sorted(srcs.items())) or 'none'}; weapon_axes {ax}")


def judge_hide(entries, c):
    if c is None or c.get("hide_patch") != "1" or (c.get("hold_mode") != "hand" and c.get("er_stance") not in STANCE_SETS):
        return "N/A", "hide_patch off (or neither hand mode nor the stance)"
    bad = hold(entries, "hide foreign") + hold(entries, "hide refused") + hold(entries, "hide restore failed")
    if bad:
        return "FAIL", f"{bad[0]['msg'][:110]}"
    patched = hold(entries, "hide row patched") + hold(entries, "hide row already patched")
    sent = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("marker sent sp=79001090")]
    if not patched:
        if sent:
            return "FAIL", "the MW2-mode marker was applied but the row was never patched (it hides nothing)"
        return "N/A", "the hide marker was never wanted"
    removed = [e["t"] for e in entries if e["tag"] == "COOP" and (e["msg"].startswith("marker removed sp=79001090") or e["msg"].startswith("marker renewed sp=79001090"))]
    gaps = [y for x, y in zip(sent, sent[1:]) if 28.0 <= y["t"] - x["t"] <= 32.0 and not any(x["t"] < r < y["t"] for r in removed)]
    if gaps:
        return "FAIL", f"{len(gaps)} marker expiry gap(s) (first at {gaps[0]['raw'][11:23]}): the hidden weapon blinked back every 30 s"
    renewed = sum(1 for e in entries if e["tag"] == "COOP" and e["msg"].startswith("marker renewed sp=79001090"))
    restored = len(hold(entries, "hide row restored"))
    return "PASS", f"row patched (stateInfo 184); {len(sent)} apply(s), {renewed} renewal(s), no expiry gap; {restored} restore(s)"


def judge_scope3p(entries, c):
    peeks = fp_spans(entries, peek=True)
    if not peeks:
        return "N/A", "no scope peek (FP on ... peek=1)"
    for a, b, _ in peeks:
        inner = [e for e in entries if e["tag"] == "HOLD" and a < e["t"] < b and (e["msg"].startswith("stance exit") or e["msg"].startswith("chord sent"))]
        if inner:
            return "FAIL", f"{inner[0]['msg'][:80]} inside the scope peek at {inner[0]['raw'][11:23]} (a peek must stay third person for the stance)"
    f6 = sum(1 for _, _, m in peeks if m and kv(m, "reason") == "f6")
    mean = sum(b - a for a, b, _ in peeks) / len(peeks)
    return "PASS", f"{len(peeks)} peek(s), {f6} ended by the ADS release (reason f6), mean {mean:.1f} s; no stance exit / chord inside"


def death_spans(entries):
    """C6 aim 4: [(t_dead, t_back)] from the player's death (`NUKE player dead`, `FP blocked reason=death`) to the next
    world leave / enter (the respawn load); open at the end = to the last line."""
    out, t0 = [], None
    for e in entries:
        dead = (e["tag"] == "NUKE" and e["msg"].startswith("player dead")) or (e["tag"] == "FP" and e["msg"].startswith("blocked reason=death"))
        back = e["tag"] == "TASK" and (e["msg"].startswith("world left") or e["msg"].startswith("world entered"))
        if dead and t0 is None:
            t0 = e["t"]
        elif back and t0 is not None:
            out.append((t0, e["t"]))
            t0 = None
    if t0 is not None and entries:
        out.append((t0, entries[-1]["t"]))
    return out


def judge_chord(entries, c):
    lines = hold(entries, "chord ") + hold(entries, "stance refresh fail") + hold(entries, "stance exit") + hold(entries, "stance disabled")
    lines.sort(key=lambda e: e["t"])
    if not any(e["msg"].startswith("chord ") for e in lines):
        return "N/A", "no chord line"
    # C6 aim 4 (aim4.md 5): chords to a dead player are N/A (ER ignores them; the 18:54 nuke death): a refresh fail inside a
    # death span (the old DLL) or right after it is not judged; `took=void` chords are not failures
    dead = death_spans(entries)
    fail = [e for e in hold(entries, "stance refresh fail") if not any(a - 0.1 <= e["t"] <= b + 1.0 for a, b in dead)]
    if fail:
        return "FAIL", f"refresh fail: {fail[0]['msg'][:100]}"
    open_t, why = None, None
    end = entries[-1]["t"]
    sent = 0
    # `busy` (a ran-out deferred exit in first person), `dead` (the death screen) and, C6 round 6, `spawn` (ER still
    # spawning the player after a world enter, at most 12 s) waits are by design
    exempt = ("busy", "dead", "spawn", "er_busy")
    # C6 part 16 addendum: a DLL before `chord done` (the 10:00:35 log): an `aborted` refusal of an enter whose grip the
    # probe then reads two-handed (`aim probe ... arm=3`) was over (ER had taken the chord before the menu opened)
    probes = hold(entries, "aim probe ") + hold(entries, "stance enter") + hold(entries, "stance carrier enter")
    lines = sorted(lines + probes, key=lambda e: e["t"])
    entering, probe_closed = False, 0
    for e in lines:
        m = e["msg"]
        if m.startswith(("stance enter", "stance carrier enter")):
            entering = True
            continue
        if m.startswith(("stance exit", "stance carrier exit")):
            entering = False
        if m.startswith("aim probe "):
            if open_t is not None and why == "aborted" and entering and kv(m, "arm") == "3":
                open_t = None
                probe_closed += 1
            continue
        if m.startswith("chord refused"):
            if open_t is None:
                open_t, why = e["t"], kv(m, "reason")
        # C6 part 16 addendum: `chord done` (the grip reached its target while refused) and `chord abort_after_played`
        # (ER had played the grip switch when a menu aborted the chord: judged as done) close a refusal too
        elif m.startswith(("chord sent", "chord done", "chord abort_after_played", "stance exit", "stance disabled")):
            sent += m.startswith("chord sent")
            if open_t is not None and why not in exempt and e["t"] - open_t > CHORD_STUCK_S:
                return "FAIL", f"chord refused reason={why} for {e['t'] - open_t:.0f} s"
            open_t = None
    if open_t is not None and why not in exempt and end - open_t > CHORD_STUCK_S:
        return "FAIL", f"chord refused reason={why} for {end - open_t:.0f} s until the end"
    took = sum(1 for e in hold(entries, "chord sent") if kv(e["msg"], "took") == "1")
    void = sum(1 for e in hold(entries, "chord sent") if kv(e["msg"], "took") == "void" or (kv(e["msg"], "took") != "1" and inside(e["t"], dead)))
    na = len(hold(entries, "stance refresh fail"))
    extra = (f", {void} to a dead player (N/A)" if void else "") + (f", {na} refresh fail(s) during a death (N/A)" if na else "")
    # C6 part 16.9: the verdicts (an ignored chord = ER busy, retried, not counted; fail = ER played it and the grip is wrong)
    vd = {}
    for e in hold(entries, "chord verdict="):
        v = kv(e["msg"], "verdict")
        vd[v] = vd.get(v, 0) + 1
    if vd:
        extra += "; verdicts " + ",".join(f"{k}:{n}" for k, n in sorted(vd.items()))
    if probe_closed:
        extra += f"; {probe_closed} aborted refusal(s) closed by the probe's arm=3 (a DLL before `chord done`)"
    ap = len(hold(entries, "chord abort_after_played"))
    if ap:
        extra += f"; {ap} abort(s) after ER played the grip switch (judged as done)"
    gf = hold(entries, "xbow grip fail")
    if gf:
        return "WARN", f"the carrier's grip failed ({gf[0]['msg'][:70]}): carried one-handed; {sent} chord(s) sent{extra}"
    gave = hold(entries, "stance exit_chords gave_up")
    if gave:
        return "WARN", f"{len(gave)} exit chord gave_up ({gave[0]['msg'][:70]}): the grip stayed as ER left it; {sent} chord(s) sent{extra}"
    return "PASS", f"{sent} chord(s) sent, {took} changed the grip{extra}"


def judge_aim(entries, c):
    """C6 aim 4 (docs part 14): third-person ADS -> ER's crossbow aim."""
    if c is None or c.get("ads_pose") != "er_aim":
        return "N/A", "tp_ads_pose off (or no stance)"
    bad = hold(entries, "aim foreign") + hold(entries, "aim restore failed") + hold(entries, "aim refused")
    if bad:
        return "FAIL", bad[0]["msg"][:110]
    patches = [e for e in hold(entries, "aim patch ") if kv(e["msg"], "already") != "1"]
    if not patches:
        return "N/A", "no third-person ADS episode with the stance posed (non-scoped gun)"
    restores = hold(entries, "aim restore ")
    for r in restores:
        if kv(r["msg"], "restored") != "1":
            return "FAIL", f"aim restore with restored=0 at {r['raw'][11:23]}"
    ev = sorted(patches + restores, key=lambda e: e["t"])
    # windows of the written fields: per ADS (when=ads) and, review C6 aim 4, the `tp_ads_patch = "stance"` fallback too
    # (written only after the enter chords and restored before any later chord: no chord may fall inside either)
    windows, open_t, open_when = [], None, None
    for e in ev:
        if e["msg"].startswith("aim patch "):
            if open_t is not None:
                return "FAIL", f"aim patch at {e['raw'][11:23]} while the previous episode's fields were never restored"
            open_t, open_when = e["t"], kv(e["msg"], "when")
        else:
            if open_t is not None and open_when in ("ads", "stance"):
                windows.append((open_t, e["t"]))
            open_t = None
    end = entries[-1]["t"]
    if open_t is not None and end - open_t > 1.0:
        return "FAIL", f"the aim fields written at {next(e['raw'][11:23] for e in patches if e['t'] == open_t)} were never restored"
    chords = [e for e in hold(entries, "chord sent ") if inside(e["t"], windows)]
    if chords:
        return "FAIL", f"a grip chord inside an aim episode at {chords[0]['raw'][11:23]} (Y + RB with the crossbow type)"
    stray = [e for e in hold(entries, "stray_bullet ") if "ignored=" not in e["msg"] and inside(e["t"], windows, pad=1.0)]
    if stray:
        return "FAIL", f"a stray bullet in an aim episode (ER fired a bolt?): {stray[0]['msg'][:80]}"
    ons = hold(entries, "aim on ")
    fails = hold(entries, "aim fail ")
    dis = hold(entries, "aim disabled")
    # review C6 aim 4: a disable after ER's aim state was already seen is ER being busy (a roll, a flask, a stagger) at
    # two presses (an old DLL counted those for the session), not a cached type
    if dis and ons and ons[0]["t"] < dis[0]["t"]:
        return "WARN", (f"aim disabled at {dis[0]['raw'][11:23]} after {len(ons)} good episode(s) ({len(fails)} no_state: ER busy at the "
                        f"press, a roll / flask / stagger; an old DLL counted those for the session)")
    if dis or (fails and not ons):
        return "FAIL", (f"ER's aim state never showed ({len(fails)} no_state, disabled={1 if dis else 0}): the type may be cached at the grip "
                        f"change: try [coop] tp_ads_patch = \"stance\"")
    ms = sorted(int(kv(e["msg"], "t").rstrip("ms")) for e in ons if kv(e["msg"], "t"))
    med = ms[len(ms) // 2] if ms else None
    al = [float(kv(e["msg"], "deg")) for e in hold(entries, "aim align ") if kv(e["msg"], "deg")]
    clamp = len(hold(entries, "aim align_clamped"))
    alt = f"; align {min(al):.1f}..{max(al):.1f} deg" if al else "; no align line"
    return "PASS", (f"{len(patches)} episode(s), {len(ons)} on (median {med} ms), {len(fails)} no_state, all restored, no chord inside"
                    f"{alt}, {clamp} clamped")


def judge_aim_cam(entries, c):
    """C6 round 6 (docs 14.8): only ER's aim BODY animation: its aim camera / crossbow reticle masked."""
    if c is None or c.get("ads_pose") != "er_aim":
        return "N/A", "tp_ads_pose off (or no stance)"
    if "aim_cam" not in c:
        return "N/A", "a DLL before C6 round 6 (no aim_cam= in HOLD cfg)"
    if c.get("aim_cam") == "er":
        return "N/A", "tp_aim_cam = er (the escape hatch: ER's aim camera / reticle by choice)"
    ref = hold(entries, "aim cam refused")
    if ref:
        return "FAIL", ref[0]["msg"][:110]
    ons = hold(entries, "aim on ")
    un = hold(entries, "aim cam unavailable")
    if un:
        return "WARN", f"{un[0]['msg'][:90]} ({len(ons)} aim episode(s) with ER's aim camera)"
    if not ons:
        return "N/A", "no confirmed aim episode"
    if not hold(entries, "aim cam keep installed"):
        return "FAIL", "aim episodes ran without `HOLD aim cam keep installed` (the filter never hooked)"
    kept = hold(entries, "aim cam kept ")
    if not kept:
        return "FAIL", "no `HOLD aim cam kept` span although the aim pose ran"
    spans, cam, ret, fade, zero, zfade = kept_spans(entries), 0, 0, 0, [], []
    for e in kept:
        ms = int(kv(e["msg"], "ms") or 0)
        hc, hr = int(kv(e["msg"], "hits_cam") or 0), int(kv(e["msg"], "hits_ret") or 0)
        cam, ret = cam + hc, ret + hr
        if ms >= 200 and hc == 0:
            zero.append(e)
        # C6 round 7: a sites=3 DLL masks ER's snipe body fade too (its writer runs every frame)
        if kv(e["msg"], "hits_fade") is not None:
            hf = int(kv(e["msg"], "hits_fade") or 0)
            fade += hf
            if ms >= 200 and hf == 0:
                zfade.append(e)
    if zero:
        return "FAIL", f"a {kv(zero[0]['msg'], 'ms')} ms span with hits_cam=0 at {zero[0]['raw'][11:23]} (the filter never matched ER's camera call)"
    if zfade:
        return "FAIL", f"a {kv(zfade[0]['msg'], 'ms')} ms span with hits_fade=0 at {zfade[0]['raw'][11:23]} (ER's snipe body fade never masked: an invisible body)"
    probes = [e for e in hold(entries, "aim probe ") if kv(e["msg"], "prec") == "1" and inside(e["t"], spans)]
    bad = [e for e in probes if kv(e["msg"], "camtype") in ("1", "2", "3")]
    if bad:
        return "FAIL", f"ER's aim camera ran while kept: {bad[0]['msg'][:90]} at {bad[0]['raw'][11:23]}"
    return "PASS", (f"{len(kept)} kept span(s), hits_cam={cam} hits_ret={ret} hits_fade={fade}, {len(probes)} prec=1 probe line(s) all camtype=0 "
                    f"(ER's follow camera, no crossbow reticle)")


def kept_spans(entries):
    """[(t0, t1)] of the `HOLD aim cam kept ms=..` spans (the line closes the span)."""
    return [(e["t"] - int(kv(e["msg"], "ms") or 0) / 1000.0, e["t"]) for e in hold(entries, "aim cam kept ")]


def judge_aim_fade(entries, c):
    """C6 round 7 (aim7.md 1): ER's snipe-mode body fade stays at 1 while our filter is kept."""
    if c is None or c.get("ads_pose") != "er_aim" or c.get("aim_cam") != "keep":
        return "N/A", "not tp_ads_pose = er_aim with tp_aim_cam = keep"
    sp = kept_spans(entries)
    if not sp:
        return "N/A", "no kept span"
    reads = [e for e in hold(entries, "aim probe ") + hold(entries, "aim pose phase=aim")
             if kv(e["msg"], "snipe") not in (None, "?") and (e["msg"].startswith("aim pose") or kv(e["msg"], "prec") == "1") and inside(e["t"], sp)]
    if not reads:
        return "N/A", "no snipe= reading inside a kept span (a DLL before round 7)"
    low = [e for e in reads if float(kv(e["msg"], "snipe")) < 0.5]
    if low:
        return "FAIL", f"ER faded our body out while kept: snipe={kv(low[0]['msg'], 'snipe')} at {low[0]['raw'][11:23]} ({len(low)}/{len(reads)} readings < 0.5)"
    return "PASS", f"{len(reads)} snipe reading(s) inside {len(sp)} kept span(s), min {min(float(kv(e['msg'], 'snipe')) for e in reads):.2f} (the body stayed drawn)"


RANGED_ROWS = (40_000_000, 45_000_000)


def moving_head(q):
    """The head of an `aim pose ... q=[..]` queue is a move (walk / run 20xxx / 22xxx, roll / backstep 5xxx, a jump
    2xxxxx), not a standing aim (part 16.1: the round-7 arm=1 hits were walks and rolls)."""
    try:
        h = int(q.strip("[]").split(",")[0])
    except (ValueError, AttributeError):
        return False
    m = h % 1_000_000
    return 20_000 <= m < 23_000 or 5_000 <= m < 6_000 or 200_000 <= m < 300_000


def judge_aim_arms(entries, c):
    """C6 round 7 probe P-A7 (aim7.md 4 C) with the part 16.1 fix: does ER's aim of a REAL crossbow move the arms?"""
    lines = hold(entries, "aim pose phase=aim")
    if not lines:
        return "N/A", "no `HOLD aim pose` line (probe_er_aim off / never aimed)"
    probes = hold(entries, "aim probe ")
    def row_of(e):
        r = kv(e["msg"], "row")
        if r is None:
            prev = [p for p in probes if p["t"] <= e["t"]]
            r = kv(prev[-1]["msg"], "row") if prev else None
        try:
            return int(r)
        except (TypeError, ValueError):
            return -1
    groups = {"vanilla": [], "carrier": []}
    other = 0
    for e in lines:
        row = row_of(e)
        ranged = RANGED_ROWS[0] <= row < RANGED_ROWS[1]
        mode = kv(e["msg"], "mode")
        kind = "vanilla" if mode == "0" else ("carrier" if kv(e["msg"], "carrier") == "1" else None)
        if not ranged or kind is None:
            other += 1
            continue
        groups[kind].append((e, row, kv(e["msg"], "arm") == "1" and not moving_head(kv(e["msg"], "q"))))
    fb = len(hold(entries, "aim hold_fallback"))
    def best_run(s):
        run = best = 0
        for _, _, ok in s:
            run = run + 1 if ok else 0
            best = max(best, run)
        return best
    rep_ = []
    for kind in ("vanilla", "carrier"):
        s = groups[kind]
        if s:
            rows_ = sorted({r for _, r, _ in s})
            rep_.append(f"{kind}: {sum(1 for x in s if x[2])}/{len(s)} standing arm=1, best run {best_run(s)}, row={','.join(map(str, rows_))}")
    txt = "; ".join(rep_)
    for kind in ("vanilla", "carrier"):
        if best_run(groups[kind]) >= 2:
            return "PASS", f"ER's {kind} crossbow aim moved the arms ({txt}); {other} other sample(s), {fb} hold_fallback episode(s)"
    if groups["vanilla"] or groups["carrier"]:
        return "WARN", f"a real crossbow aimed but the arms never moved standing ({txt}): stop and decide (docs 16.6); {other} other sample(s)"
    return "N/A", f"no vanilla crossbow sample (mode=0 with a ranged row): {other} sample(s) of the patched / melee row"


def judge_aim_anim(entries, c):
    """C6 part 16 addendum: per `HOLD aim episode` line, did ER's crossbow aim animation (46037xxx, aim=) play, under
    which tp_aim_cam (cam=keep|er|keep>er) and grip (arm=a->b)? PASS when a carrier episode had it; WARN when carrier
    episodes ran and none had it (the A/B decides: docs 16.15); N/A without a carrier episode (vanilla reported)."""
    eps = hold(entries, "aim episode ")
    if not eps:
        return "N/A", "no `HOLD aim episode` line (a DLL before the part 16 addendum, probe_er_aim off without the carrier, or never aimed)"
    groups, items = {}, []
    for e in eps:
        m = e["msg"]
        row = int(kv(m, "row") or -1)
        kind = "carrier" if kv(m, "carrier") == "1" else ("vanilla" if kv(m, "mode") == "0" and RANGED_ROWS[0] <= row < RANGED_ROWS[1] else "other")
        cam, arm, aim = kv(m, "cam") or "?", (kv(m, "arm") or "?").split("->")[-1], kv(m, "aim") or "none"
        # part 16.16: loaded at the start or the end of the episode
        ld = "1" if "1" in (kv(m, "loaded") or "?").split("->") else ("0" if kv(m, "loaded") else "?")
        hit = aim != "none"
        g = groups.setdefault((kind, cam, arm, ld), [0, 0])
        g[0] += 1
        g[1] += hit
        items.append(f"#{kv(m, 'n')} {kind} cam={cam} arm={kv(m, 'arm')} prec={kv(m, 'prec_ms')}ms ams={kv(m, 'ads_ms')} "
                     f"loaded={kv(m, 'loaded')} {('aim=' + aim + '@' + str(kv(m, 'aim_at_ms')) + 'ms') if hit else 'aim=none'}")
    summ = "; ".join(f"{k[0]} cam={k[1]} arm={k[2]} loaded={k[3]}: {v[1]}/{v[0]}" for k, v in sorted(groups.items()))
    more = f" (+{len(items) - 12} more)" if len(items) > 12 else ""
    txt = f"{summ} | " + ", ".join(items[:12]) + more
    car = [(k, v) for k, v in groups.items() if k[0] == "carrier"]
    if not car:
        return "N/A", f"no carrier episode: {txt}"
    if any(v[1] for _, v in car):
        best = sorted({k[1] for k, v in car if v[1]})
        bl = sorted({k[3] for k, v in car if v[1]})
        return "PASS", f"ER's aim anim played with the carrier under cam={','.join(best)} loaded={','.join(bl)}: {txt}"
    nl = sum(v[0] for k, v in car if k[3] == "1")
    stop = f" ({nl} of them LOADED: the loaded round failed too -> stop the aim-animation work, user decision 2026-10-22)" if nl else ""
    return "WARN", f"ER's aim anim never played in {sum(v[0] for _, v in car)} carrier episode(s){stop}: {txt}"


def judge_carrier(entries, c):
    """C6 part 16: the real crossbow carrier (dry probe P-X1 or the swap / restore pairs)."""
    mode = (c or {}).get("carrier", "off")
    xl = hold(entries, "xbow ")
    if mode == "off" and not xl:
        return "N/A", "er_carrier off (or a DLL before part 16)"
    bad = [e for e in xl if e["msg"].startswith(("xbow trip", "xbow refused", "xbow foreign", "xbow restore_missing"))]
    bad += [e for e in hold(entries, "xbow restore why=") if kv(e["msg"], "ok") == "0" and kv(e["msg"], "why") != "world_leave"]
    if mode != "off":
        bad += [e for e in hold(entries, "xbow off why=") if kv(e["msg"], "why") in ("prologue", "version")]
    if bad:
        return "FAIL", f"{bad[0]['msg'][:110]} at {bad[0]['raw'][11:23]}"
    if mode == "dry":
        dry = hold(entries, "xbow dry ")
        if not dry:
            return "N/A", "dry: no swap was wanted yet (MW2 mode on in third person)"
        picks = [kv(e["msg"], "pick") for e in dry]
        if all(p == "none" for p in picks):
            return "WARN", f"dry: no crossbow owned ({len(dry)} line(s)): carrier_give = true or acquire one (docs 16.2 X4)"
        return "PASS", f"dry: {len(dry)} line(s), pick={next(p for p in picks if p != 'none')} slot={kv(dry[-1]['msg'], 'slot')} (no call, no write)"
    ons = hold(entries, "xbow on ")
    rest = [e for e in hold(entries, "xbow restore why=") if kv(e["msg"], "ok") == "1"]
    if not ons and not rest:
        sk = hold(entries, "xbow skip")
        return "N/A", f"no swap ({sk[-1]['msg'][:60] if sk else 'never wanted'})"
    open_on = None
    pairs = 0
    for e in sorted(ons + rest, key=lambda x: x["t"]):
        if e["msg"].startswith("xbow on "):
            open_on = e
        elif open_on is not None:
            pairs += 1
            open_on = None
    grips = {}
    for e in hold(entries, "xbow grip via="):
        k = f"{kv(e['msg'], 'via')}:ok={kv(e['msg'], 'ok')}"
        grips[k] = grips.get(k, 0) + 1
    gtxt = ",".join(f"{k}x{n}" for k, n in sorted(grips.items())) or "none"
    if open_on is not None:
        return "WARN", f"{len(ons)} swap(s), {pairs} restored; the last ({open_on['raw'][11:23]}) still on at the end (its journal restores at the next world enter); grip {gtxt}"
    return "PASS", f"{len(ons)} swap(s), {pairs} restored ok (why {','.join(sorted({kv(e['msg'], 'why') for e in rest}))}); grip {gtxt}"


def judge_load(entries, c):
    """C6 part 16.16: the loaded crossbow (`[coop] carrier_load`): ER's loaded state written once the carrier is two-handed."""
    on = (c or {}).get("load", "0") == "1"
    ll = hold(entries, "xbow load ")
    if not on and not ll:
        return "N/A", "carrier_load off (or a DLL before part 16.16)"
    lost = hold(entries, "xbow load lost")
    if lost:
        return "FAIL", f"ER unloaded our loaded crossbow itself (a shot?): {lost[0]['msg'][:90]} at {lost[0]['raw'][11:23]}"
    res = hold(entries, "xbow load method=")
    for e in res:
        b = (kv(e["msg"], "bolts") or "?->?").split("->")
        if len(b) == 2 and b[0].isdigit() and b[1].isdigit() and int(b[1]) < int(b[0]):
            return "FAIL", f"a bolt was used across the load: {e['msg'][:100]} at {e['raw'][11:23]}"
    und = hold(entries, "xbow load undo ")
    bad = [e for e in und if kv(e["msg"], "ok") == "0"]
    if bad:
        return "FAIL", f"an undo did not write (a loaded crossbow may be left): {bad[0]['msg'][:100]} at {bad[0]['raw'][11:23]}"
    oks = [e for e in res if kv(e["msg"], "ok") == "1"]
    ok0 = [e for e in res if kv(e["msg"], "ok") == "0"]
    waits = {}
    for e in hold(entries, "xbow load wait "):
        w = kv(e["msg"], "reason")
        waits[w] = waits.get(w, 0) + 1
    wtxt = ",".join(f"{k}x{n}" for k, n in sorted(waits.items())) or "none"
    whys = {}
    for e in und:
        w = kv(e["msg"], "why")
        whys[w] = whys.get(w, 0) + 1
    utxt = ",".join(f"{k}x{n}" for k, n in sorted(whys.items())) or "none"
    if not res:
        return ("WARN" if waits else "N/A"), f"no load written (waits {wtxt})"
    if not oks:
        return "WARN", f"{len(ok0)} load(s) not kept by ER (ok=0): {ok0[0]['msg'][:100] if ok0 else res[0]['msg'][:100]}"
    # every verified load must be followed by an undo or a restore of its swap
    ends = sorted(und + [e for e in hold(entries, "xbow restore why=") if kv(e["msg"], "ok") == "1"], key=lambda x: x["t"])
    last_ok = oks[-1]["t"]
    if not any(e["t"] >= last_ok for e in ends):
        return "WARN", f"{len(oks)} load(s) ok, the last ({oks[-1]['raw'][11:23]}) still loaded at the session end (the next restore clears it); undo {utxt}"
    return "PASS", f"{len(oks)} load(s) ok (loaded={kv(oks[0]['msg'], 'loaded')} egd={kv(oks[0]['msg'], 'egd')} bolts={kv(oks[0]['msg'], 'bolts')} ms={kv(oks[0]['msg'], 'ms')}), {len(ok0)} not kept; undo {utxt}; waits {wtxt}"


def guard_spans(entries):
    """C6 part 17: [(t_on, t_end)] of the guard episodes (`HOLD guard on` to its `HOLD guard episode` line; open at the
    end = to the last line)."""
    out, t0 = [], None
    for e in entries:
        if e["tag"] != "HOLD":
            continue
        if e["msg"].startswith("guard on "):
            t0 = e["t"] if t0 is None else t0
        elif e["msg"].startswith("guard episode ") and t0 is not None:
            out.append((t0, e["t"]))
            t0 = None
    if t0 is not None and entries:
        out.append((t0, entries[-1]["t"]))
    return out


def judge_guard(entries, c):
    """C6 part 17: the third-person guard ADS (`tp_ads_pose = "guard"` with the polearm stance)."""
    on = (c or {}).get("ads_pose") == "guard"
    eps = hold(entries, "guard episode ")
    if not on and not eps:
        return "N/A", "tp_ads_pose is not guard (or a DLL before part 17)"
    # nothing of the crossbow aim may run with the guard: no aim fields, no ER aim-camera / reticle / fade filter
    bad = hold(entries, "aim patch row=") + hold(entries, "aim cam keep installed") + hold(entries, "aim cam kept ")
    if on and bad:
        return "FAIL", f"the crossbow aim ran with the guard: {bad[0]['msg'][:90]} at {bad[0]['raw'][11:23]}"
    sp = guard_spans(entries)
    for e in hold(entries, "chord sent "):
        if inside(e["t"], sp):
            return "FAIL", f"a grip chord inside a guard episode (Y + RB = a guard attack): {e['msg'][:80]} at {e['raw'][11:23]}"
    for e in hold(entries, "stray_bullet row="):
        if "ignored=" not in e["msg"] and inside(e["t"], sp, 1.0):
            return "FAIL", f"an ER bullet during a guard episode: {e['msg'][:80]} at {e['raw'][11:23]}"
    if not eps:
        return "N/A", "no guard episode (hold MW2 ADS in third person with the polearm stance posed)"
    det = [e for e in eps if kv(e["msg"], "detected") == "1"]
    fb = [e for e in eps if kv(e["msg"], "fallback") == "1"]
    clamped = [e for e in det if kv(e["msg"], "clamped") == "1"]
    dms = sorted(int(kv(e["msg"], "det_ms")) for e in det if (kv(e["msg"], "det_ms") or "").isdigit())
    al = [float(kv(e["msg"], "align")) for e in det if re.match(r"^-?[\d.]+$", kv(e["msg"], "align") or "")]
    vias = sorted({kv(e["msg"], "via") for e in det})
    heads = []
    for e in eps:
        for h in (kv(e["msg"], "heads") or "[]").strip("[]").split(","):
            if h and h not in heads:
                heads.append(h)
    ganim = sorted({kv(e["msg"], "guard_anim") for e in eps} - {None, "none"})
    yaw = sum(int(kv(e["msg"], "yaw") or 0) for e in eps)
    yaw_bad = [e for e in hold(entries, "guard yaw write ") if kv(e["msg"], "ok") == "0"]
    # part 17 addendum (docs 17.12): the guard frame (corr=builtin|auto|fixed; off / absent = part 17) only in a detected
    # guard and only inside a guard span; the align after it; the learned frame (`guard frame auto`)
    for e in eps:
        if kv(e["msg"], "detected") == "0" and kv(e["msg"], "corr") not in (None, "off"):
            return "FAIL", f"the guard frame was used in an undetected guard: {e['msg'][:90]} at {e['raw'][11:23]}"
    frames = hold(entries, "guard frame src=")
    for e in frames:
        if not inside(e["t"], sp):
            return "FAIL", f"the guard frame was drawn outside a guard episode: {e['msg'][:80]} at {e['raw'][11:23]}"
    corr_det = [e for e in det if kv(e["msg"], "corr") not in (None, "off")]
    corr_al = [float(kv(e["msg"], "align")) for e in corr_det if re.match(r"^-?[\d.]+$", kv(e["msg"], "align") or "")]
    corr_cl = [e for e in corr_det if kv(e["msg"], "clamped") == "1"]
    learned = hold(entries, "guard frame auto ")
    fwd_err = [kv(e["msg"], "fwd_err") for e in frames]
    ctxt = ""
    if corr_det:
        srcs = sorted({kv(e["msg"], "corr") for e in corr_det})
        ctxt = (f"; corr {','.join(srcs)} on {len(corr_det)}/{len(det)}: align {f'{min(corr_al):.1f}..{max(corr_al):.1f} deg' if corr_al else 'none'}, "
                f"clamped {len(corr_cl)}; fwd_err {','.join(x for x in fwd_err[:6] if x) or '-'}")
    if learned:
        m = learned[-1]["msg"]
        ctxt += f"; learned axes={kv(m, 'axes')} rot={kv(m, 'rot')} vs_builtin={kv(m, 'vs_builtin')}"
    rep = (f"{len(det)}/{len(eps)} episode(s) detected (via {','.join(vias) or '-'}, det_ms median {dms[len(dms) // 2] if dms else '-'}); "
           f"align {f'{min(al):.1f}..{max(al):.1f} deg' if al else 'none'}, clamped {len(clamped)}; fallback {len(fb)}; yaw writes {yaw}; "
           f"guard_anim {','.join(ganim) or 'none'}; heads {','.join(heads[:10]) or '?'}{ctxt}")
    if yaw_bad:
        return "WARN", f"the body-yaw write was refused ({yaw_bad[0]['msg'][:60]}); {rep}"
    if not det:
        return "WARN", f"the guard pose was never detected (ER's weapon frame never left the carry): the gun raise only; {rep}"
    if corr_det and len(corr_cl) == len(corr_det):
        return "WARN", f"the guard frame left the gun over guard_max in every corrected guard (the raise; pin the learned frame / send the log); {rep}"
    return "PASS", rep


def cam_lines(entries, prefix):
    return [e for e in entries if e["tag"] == "CAM" and e["msg"].startswith(prefix)]


def judge_shoulder(entries, c):
    """C6 part 18: the third-person over-the-shoulder camera (`CAM shoulder ..` lines; independent of `HOLD cfg`)."""
    cfgs = cam_lines(entries, "shoulder cfg ")
    ons, offs = cam_lines(entries, "shoulder on "), cam_lines(entries, "shoulder off ")
    if not ons and not any(kv(e["msg"], "on") == "1" for e in cfgs):
        return "N/A", "[third_person] shoulder offsets zero (or a DLL before part 18)"
    fps = fp_spans(entries)
    for e in ons:
        if any(a < e["t"] < b for a, b, _ in fps):
            return "FAIL", f"the shoulder offset went on inside first person / the scope peek: {e['msg'][:80]} at {e['raw'][11:23]}"
        prev = [x for x in cfgs if x["t"] <= e["t"]]
        if prev and kv(prev[-1]["msg"], "on") != "1":
            return "FAIL", f"the shoulder offset went on with cfg on=0: {e['msg'][:80]} at {e['raw'][11:23]}"
    if not ons:
        return "N/A", "no shoulder episode (MW2 mode on in third person, in the world)"
    num = lambda e, k: float(kv(e["msg"], k) or 0)
    writes = sum(int(num(e, "writes")) for e in offs)
    conflicts = sum(int(num(e, "conflicts")) for e in offs)
    pulls = sum(int(num(e, "pulls")) for e in offs)
    mins = [num(e, "min_frac") for e in offs if kv(e["msg"], "min_frac")]
    whys = {}
    for e in offs:
        w = kv(e["msg"], "why") or "?"
        whys[w] = whys.get(w, 0) + 1
    rep = (f"{len(ons)} episode(s), writes {writes}, pulls {pulls}" + (f" (min frac {min(mins):.2f})" if mins else "")
           + f", off {','.join(f'{k}x{v}' for k, v in sorted(whys.items())) or 'none (still on)'}")
    # 2026-10-22: the DLL logs `unavailable` on its first tick, ~0.1 s before the camera-step hook is installed at boot (the
    # cfg line then repeats with hook=1): only a run whose episodes wrote nothing lacks the hook
    if cam_lines(entries, "shoulder unavailable") and writes == 0:
        return "WARN", f"no camera-step hook (needs [first_person] camera_mode = hook or [ac130] enabled at boot); {rep}"
    if conflicts:
        return "WARN", f"{conflicts} frame(s) armed while first person / the gunship owned the camera (refused); {rep}"
    dead = [e for e in offs if num(e, "ms") >= 1000 and kv(e["msg"], "writes") == "0"]
    if dead:
        return "WARN", f"an episode wrote nothing (the detour never ran / camera changed): {dead[0]['msg'][:80]}; {rep}"
    return "PASS", rep


def judge_no_fire(entries, c):
    if not hold(entries, "stance enter row=") and not hold(entries, "stance carrier enter") and not hold(entries, "xbow on "):
        return "N/A", "the stance never entered"
    # C6 part 16.4: a used bolt with the carrier on is ER firing
    trip = hold(entries, "xbow trip")
    if trip:
        return "FAIL", f"a bolt was used with the carrier on: {trip[0]['msg'][:90]}"
    stray = [e for e in hold(entries, "stray_bullet ") if "ignored=" not in e["msg"]]
    dis = [e for e in hold(entries, "stance disabled") if kv(e["msg"], "reason") == "stray_bullet"]
    if stray or dis:
        return "FAIL", f"{len(stray)} stray bullet(s) of a non-mod row ({stray[0]['msg'][:80] if stray else ''}){' + disabled' if dis else ''}"
    ign = len(hold(entries, "stray_bullet ")) - len(stray)
    return "PASS", f"0 stray bullets ({ign} use-item bullet(s) ignored)"


DEAD_ON = (("SHOT", "dead gate on"), ("FP", "blocked reason=death"), ("FP", "off reason=death"), ("MOVE", "blocked reason=death"),
           ("MOVE", "suspend reason=death"), ("NUKE", "player dead"),
           ("NUKE", "streak reset reason=death"))
SHOT_USED = re.compile(r"^n=\d+ (mag=|skipped:)")


def dead_fire_spans(entries):
    """2026-10-24: [(t_dead, t_alive)] from the player's death to `SHOT dead gate off` (a DLL with the death gate) or else
    the respawn's `TASK world entered`; open at the end = to the last line."""
    gate = any(e["tag"] == "SHOT" and e["msg"].startswith("dead gate ") for e in entries)
    out, t0 = [], None
    for e in entries:
        if t0 is None and any(e["tag"] == tg and e["msg"].startswith(m) for tg, m in DEAD_ON):
            t0 = e["t"]
        elif t0 is not None and ((gate and e["tag"] == "SHOT" and e["msg"].startswith("dead gate off"))
                                 or (not gate and e["tag"] == "TASK" and e["msg"].startswith("world entered"))):
            out.append((t0, e["t"]))
            t0 = None
    if t0 is not None and entries:
        out.append((t0, entries[-1]["t"]))
    return out


def judge_dead_fire(entries, c):
    sp = dead_fire_spans(entries)
    if not sp:
        return "N/A", "no death in the session"
    shots = [e for e in entries if e["tag"] == "SHOT" and SHOT_USED.match(e["msg"]) and inside(e["t"], sp)]
    refused = sum(int(kv(e["msg"], "refused") or 0) for e in entries if e["tag"] == "SHOT" and e["msg"].startswith("dead gate off"))
    if shots:
        live = sum(1 for e in shots if "mag=" in e["msg"][:12])
        return "FAIL", f"{len(shots)} MW2 shot(s) while dead ({live} spawned), first: {shots[0]['raw'][:110]}"
    return "PASS", f"{len(sp)} death(s), 0 shots while dead, {refused} fire press(es) refused"


def judge_melee(entries, c):
    n = sum(int(kv(e["msg"], "n").split("/")[0]) for e in hold(entries, "melee suppressed"))
    if n == 0:
        return "N/A", "no R3 press suppressed"
    return "PASS", f"{n} R3 press(es) suppressed while the stance was on (documented)"


def judge_grip(entries, c):
    if c is None or c.get("hold_mode") != "hand" or c.get("source") != "weapon" or "weapon_rake" not in c:
        return "N/A", "hand mode with source=weapon on a grip-3 DLL (weapon_rake= in HOLD cfg) only"
    wa = c.get("weapon_axes")
    if wa == "+y,+x,+z":
        return "FAIL", "weapon_axes = \"+y,+x,+z\": the first build's guess (barrel backward / down)"
    for e in hold(entries, "weapon axes auto"):
        acc = kv(e["msg"], "accepted")
        ax = kv(e["msg"], "axes")
        if acc is None:
            return "FAIL", "a `weapon axes auto` line without accepted= (a pre-grip-3 DLL: the vote could pick a backwards map)"
        if acc == "1" and ax != RIG_AXES:
            return "FAIL", f"auto accepted {ax}, not the rig map {RIG_AXES}"
    grips = hold(entries, "grip who=self ")
    if not grips:
        if hold(entries, "weapon ok who=self"):
            return "FAIL", "the gun sat on ER's weapon frame but no `HOLD grip who=self` line (which grip?)"
        return "N/A", "the self gun was never on ER's weapon frame"
    guns = sorted({f"{kv(e['msg'], 'gun')}:{kv(e['msg'], 'src')}" for e in grips})
    auto = hold(entries, "weapon axes auto")
    av = f"; auto {kv(auto[-1]['msg'], 'axes')} accepted={kv(auto[-1]['msg'], 'accepted')}" if auto else ""
    # part 17 addendum: the polearm carry grips (src=polearm_long|polearm_short) when polearm_grip=1
    pg = f"; polearm_grip={c['polearm_grip']} long_min={c.get('long_min')}" if "polearm_grip" in c else ""
    return "PASS", f"weapon_axes {wa} rake={c.get('weapon_rake')}; grips {','.join(guns)}{av}{pg}"


def judge_churn(entries, c):
    defers = hold(entries, "stance exit_defer ")
    if not defers:
        return "N/A", "no deferred stance exit"
    win = float(c.get("exit_defer_s", "3.0")) if c else 3.0
    enters = hold(entries, "stance enter row=")
    resumes = hold(entries, "stance resume ")
    chords = hold(entries, "chord sent ")
    checked = 0
    for d in defers:
        if kv(d["msg"], "resumable") != "1":
            continue
        en = next((e for e in enters if d["t"] < e["t"] < d["t"] + win), None)
        if en is None:
            continue
        checked += 1
        if not any(abs(r["t"] - en["t"]) <= 0.1 for r in resumes):
            return "FAIL", f"re-enter at {en['raw'][11:23]} inside the exit's deferral without `HOLD stance resume`"
        bad = [x for x in chords if d["t"] < x["t"] <= en["t"] + 1.5]
        if bad:
            return "FAIL", f"a chord around a resumed toggle at {bad[0]['raw'][11:23]}: {bad[0]['msg'][:60]}"
    per = len(chords) / max(1, len(enters))
    return "PASS", f"{len(defers)} deferred exit(s), {checked} resumed without chords; {len(chords)} chord(s) for {len(enters)} enter(s) ({per:.1f} per enter)"


def judge_body(entries, c):
    if c is None or c.get("body_occl") != "1":
        return "N/A", "world_gun_body_occlusion off"
    none = hold(entries, "body bones none")
    if none:
        return "FAIL", f"{none[0]['msg'][:100]}"
    found = hold(entries, "body bones found=")
    ok = hold(entries, "body ok who=self")
    first = [e for e in entries if e["tag"] == "COOP" and e["msg"].startswith("worldgun body occlusion first frame")]
    drawn = hold(entries, "weapon ok who=self") or hold(entries, "hand ok who=self")
    if not found or not ok or not first:
        if drawn:
            return "FAIL", f"our gun was drawn but bones={len(found)} body_ok={len(ok)} capsule_frames={len(first)}"
        return "N/A", "our gun was never drawn in third person"
    miss = tally_reasons(hold(entries, "body miss who="))
    return "PASS", (f"{kv(found[0]['msg'], 'found')} bones, {kv(ok[0]['msg'], 'capsules')} capsules ({kv(first[0]['msg'], 'capsules')} in the first "
                    f"frame); misses {','.join(f'{k}:{v}' for k, v in sorted(miss.items())) or 'none'}")


def judge_depth(entries, c):
    if c is None or c.get("probe_er_depth") != "1":
        return "N/A", "probe_er_depth off"
    dl = [e for e in entries if e["tag"] == "DEPTH"]
    off = [e for e in dl if e["msg"].startswith("probe off")]
    if off:
        return "FAIL", off[0]["msg"][:100]
    if not any(e["msg"].startswith("probe on") for e in dl):
        return "FAIL", "no `DEPTH probe on` line although probe_er_depth=1"
    chains = [e for e in dl if e["msg"].startswith("probe chain ")]
    if not chains:
        return "N/A", "no chain read (never in the world)"
    for e in chains:
        if kv(e["msg"], "pins") != "6/6" or kv(e["msg"], "a88_eq") != "1" or kv(e["msg"], "module") != "d3d12:1":
            return "FAIL", f"chain at {e['raw'][11:23]}: pins={kv(e['msg'], 'pins')} a88_eq={kv(e['msg'], 'a88_eq')} module={kv(e['msg'], 'module')}"
    # review C6 depth: GetDesc runs only in UiBegin's detour; `unmeasured` lines (no hook / no UiBegin frame) are not judged
    da = [e for e in dl if e["msg"].startswith("probe desc A ") and " unmeasured " not in e["msg"]]
    if not da and any(e["msg"].startswith("probe desc A ") for e in dl):
        return "N/A", "desc A unmeasured (no UiBegin sample: GetDesc only on ER's render thread)"
    bad = [e for e in da if kv(e["msg"], "rule") != "ok"]
    if not da or bad:
        return "FAIL", f"desc A: {(bad or [{'msg': 'none'}])[0]['msg'][:110]}"
    db = [e for e in dl if e["msg"].startswith("probe desc B ") and " unmeasured " not in e["msg"]]
    v = [e for e in dl if e["msg"].startswith("probe verdict ")]
    words = kv(v[-1]["msg"], "wrap_state_words") if v else "?"
    return "PASS", (f"{len(chains)} chain read(s) 6/6, desc A fmt={kv(da[-1]['msg'], 'fmt')} {da[-1]['msg'].split()[4]} rule=ok; desc B "
                    f"{kv(db[-1]['msg'], 'rule') if db else 'none'}; res changes {kv(chains[-1]['msg'], 'changes')}; state words {words}"
                    + ("" if v else " (no 60 s verdict yet)"))


def depth_lines(entries, prefix):
    return [e for e in entries if e["tag"] == "DEPTH" and e["msg"].startswith(prefix)]


def judge_depth_state(entries, c):
    """C6 depth 5 (15.1): the read-only barrier probe and its verdict."""
    if c is None or c.get("depth_state") != "1":
        return "N/A", "probe_er_depth_state off"
    off = depth_lines(entries, "state hook off")
    if off:
        return "FAIL", off[0]["msg"][:110]
    on = depth_lines(entries, "state hook on")
    st = depth_lines(entries, "state res=")
    if not on:
        if st:
            return "FAIL", "`DEPTH state` lines without `DEPTH state hook on`"
        return "N/A", "the hook was never installed (no Present / never in the world)"
    if not st:
        return "N/A", "no `DEPTH state res=` line (never in the world)"
    v = depth_lines(entries, "state verdict ")
    paths = len(depth_lines(entries, "state path "))
    last = st[-1]["msg"]
    info = (f"module={kv(on[0]['msg'], 'module')} at_ui={kv(last, 'at_ui')} queues={kv(last, 'queues')} threads={kv(last, 'threads')} "
            f"mismatch={kv(last, 'mismatch')} vt_eq={kv(last, 'vt_eq')}; {paths} path line(s)")
    if not v:
        return "N/A", f"no 60 s verdict yet; {info}"
    # C6 round 6: one verdict per candidate (`cand=A|B`; an older DLL's line without it is candidate A)
    by = {}
    for e in v:
        by[kv(e["msg"], "cand") or "A"] = e["msg"]
    ready = [k for k, m in sorted(by.items()) if kv(m, "ready") == "1"]
    if ready:
        m = by[ready[0]]
        others = ", ".join(f"{k} why={kv(x, 'why')}" for k, x in sorted(by.items()) if k not in ready)
        return "PASS", f"verdict ready=1 cand={ready[0]} frames={kv(m, 'frames')} readable={kv(m, 'readable')}{'; ' + others if others else ''}; {info}"
    whys = {k: kv(m, "why") for k, m in sorted(by.items())}
    txt = " ".join(f"{k}:{w}" for k, w in whys.items())
    if "hook" in whys.values():
        return "FAIL", f"verdict why=hook ({txt}): the barrier hook saw neither the calls nor B's own UI barriers; {info}"
    if all(w in ("frames", "control") for w in whys.values()):
        return "N/A", f"verdict not judged yet ({txt}); {info}"
    return "FAIL", f"verdict NOT ready ({txt}) (capsules only; send the `DEPTH state path` / `DEPTH census` lines); {info}"


def judge_depth_control(entries, c):
    """C6 round 6 (15.8): B's own UiBegin barrier as the positive control of the barrier hook; the census."""
    if c is None or c.get("depth_state") != "1":
        return "N/A", "probe_er_depth_state off"
    cl = depth_lines(entries, "state control ")
    if not cl:
        return "N/A", "no `DEPTH state control` line (a DLL before C6 round 6, or never in the world)"
    m = cl[-1]["msg"]
    cen = depth_lines(entries, "census res=")
    top = f"; top census {cen[-1]['msg'][7:110]}" if cen else "; no census row"
    detail = f"ui={kv(m, 'ui')} calls={kv(m, 'calls')} bars={kv(m, 'bars')} trans={kv(m, 'trans')}{top}"
    ok = kv(m, "ok")
    if ok == "1":
        return "PASS", detail
    if ok == "0":
        return "FAIL", f"the control failed (no call / B's UI barriers missing: the hook sees nothing): {detail}"
    return "N/A", f"not judged yet: {detail}"


def judge_depth_sample(entries, c):
    if c is None or c.get("depth_sample") != "1":
        return "N/A", "probe_er_depth_sample off"
    off = [e for e in depth_lines(entries, "probe sample off") if kv(e["msg"], "why") == "hresult"]
    if off:
        return "FAIL", off[0]["msg"][:110]
    sm = depth_lines(entries, "probe sample n=")
    if not sm:
        return "N/A", "no sample (no readable frame / never in the world)"
    convs = [kv(e["msg"], "conv") for e in sm]
    if "forward" in convs:
        return "FAIL", f"conv=forward in {convs.count('forward')} sample(s): not ER's reverse-Z scene depth"
    if "reverse" not in convs:
        return "WARN", f"{len(sm)} sample(s), conv never reverse; last {sm[-1]['msg'][:90]}"
    return "PASS", f"{len(sm)} sample(s) conv=reverse state={kv(sm[-1]['msg'], 'state')} zeros={kv(sm[-1]['msg'], 'zeros')}"


def judge_depth_self(entries, c):
    if c is None or c.get("depth_sample") != "1":
        return "N/A", "probe_er_depth_sample off"
    sl = depth_lines(entries, "probe self ")
    if not sl:
        return "N/A", "no self check (no chest pixel sampled)"
    m = sl[-1]["msg"]
    ok = kv(m, "ok") or "0/0"
    judged = int(ok.split("/")[1]) if "/" in ok else 0
    why = kv(m, "why")
    if why is not None:
        # C6 round 7: the fitted offset, residuals, edges and the scale
        ss = depth_lines(entries, "probe self_s ")
        os_ = [float(kv(e["msg"], "o")) for e in ss if kv(e["msg"], "o") not in (None, "?")]
        st = f"; self_s o={min(os_):.2f}..{max(os_):.2f} m over {len(os_)} still sample(s)" if os_ else ""
        # C6 part 16.8: z*d per judged still sample (flat across ranges = a pure near scale; median = n_eff)
        zd = [float(kv(e["msg"], "zd")) for e in ss if kv(e["msg"], "zd") not in (None, "?", "NaN") and kv(e["msg"], "edge") == "0"]
        if not zd:
            zd = [float(kv(e["msg"], "z")) * float(kv(e["msg"], "d")) for e in ss if kv(e["msg"], "d") not in (None, "NaN") and kv(e["msg"], "edge") == "0"]
        zd.sort()
        zt = f"; z*d {zd[0]:.4f}..{zd[-1]:.4f} median {zd[len(zd) // 2]:.4f} over {len(zd)}" if zd else ""
        if kv(m, "n_eff") is not None:
            # C6 part 16.8: the two-parameter line, the leverage and the near the prime uses
            detail = (f"ok={ok} far={kv(m, 'far')} edges={kv(m, 'edges')} s={kv(m, 's')} s_iqr={kv(m, 's_iqr')} o={kv(m, 'o_cm')} cm lev={kv(m, 'lev')} "
                      f"scale={kv(m, 'scale')} n_eff={kv(m, 'n_eff')} near_cam={kv(m, 'near_cam')} p95={kv(m, 'p95')} cm model={kv(m, 'model')} why={why}{st}{zt}")
        else:
            detail = (f"ok={ok} edges={kv(m, 'edges')} o={kv(m, 'o_cm')} cm p50={kv(m, 'err_cm_p50')} cm p95={kv(m, 'p95')} cm span={kv(m, 'span_m')} m "
                      f"scale={kv(m, 'scale')} slope={kv(m, 'slope')} margin_rel={kv(m, 'margin_rel')} model={kv(m, 'model')} lag={kv(m, 'lag')} why={why}{st}{zt}")
    else:
        detail = (f"ok={ok} p50={kv(m, 'err_cm_p50')} cm p95={kv(m, 'p95')} cm near_cam={kv(m, 'near_cam')} near_fit={kv(m, 'near_fit')} "
                  f"model={kv(m, 'model')} lag={kv(m, 'lag')}")
    if kv(m, "pass") == "1":
        return "PASS", detail
    if why == "few" or (why is None and judged < 6):
        return "N/A", f"too few judged samples: {detail}"
    return "FAIL", f"self check failed (the resource, its linearisation or the frame match): {detail}"


def judge_depth_live(entries, c):
    if c is None or c.get("er_depth") != "1":
        return "N/A", "world_gun_er_depth off"
    trip = depth_lines(entries, "live trip")
    if trip:
        return "FAIL", f"{trip[0]['msg'][:110]}"
    on = depth_lines(entries, "live on ")
    # C6 part 16.8: live linearises with the self check's n_eff (>= CSCam's near), never CSCam's near alone
    for e in on:
        sl = [x for x in depth_lines(entries, "probe self ") if x["t"] <= e["t"] + 0.6 and kv(x["msg"], "pass") == "1" and kv(x["msg"], "n_eff") is not None]
        if sl and kv(e["msg"], "near") is not None:
            want, got = float(kv(sl[-1]["msg"], "n_eff")), float(kv(e["msg"], "near"))
            if got + 0.0002 < want:
                return "FAIL", f"live on near={got:.4f} below the self check's n_eff={want:.4f} at {e['raw'][11:23]} (phantom occlusion)"
    if not on:
        off = depth_lines(entries, "live off")
        if off:
            return "WARN", f"the prime stayed off ({off[-1]['msg'][:80]}): capsules only"
        return "N/A", "no live decision yet"
    ln = depth_lines(entries, "live n=")
    primed = int(kv(ln[-1]["msg"], "primed") or 0) if ln else 0
    miss = kv(ln[-1]["msg"], "miss") if ln else "?"
    if primed == 0:
        return "WARN", f"live but no primed frame yet (miss={miss})"
    return "PASS", f"live, primed={primed} copies={kv(ln[-1]['msg'], 'copies')} miss={miss}"


def depth_verdict(res):
    """`C6 DEPTH VERDICT: READY | NOT READY why=..` from the four depth rows (None when the probe was off)."""
    d = {n: (st, x) for n, st, x in res if n in DEPTH_ROWS}
    if d.get("depth state", ("N/A", ""))[0] == "N/A" and d.get("depth live", ("N/A", ""))[0] == "N/A":
        return None
    for n in DEPTH_ROWS:
        st, x = d.get(n, ("N/A", ""))
        if st == "FAIL":
            return f"C6 DEPTH VERDICT: NOT READY why={n.replace(' ', '_')} ({x[:80]})"
    if d["depth state"][0] != "PASS":
        return f"C6 DEPTH VERDICT: NOT READY why=state_{d['depth state'][0].lower().replace('/', '')}"
    if d["depth sample"][0] != "PASS" or d["depth self"][0] != "PASS":
        return "C6 DEPTH VERDICT: NOT READY why=sample"
    return "C6 DEPTH VERDICT: READY"


def rows(entries):
    c = cfg(entries)
    return [("hand", *judge_hand(entries, c)), ("weapon", *judge_weapon(entries, c)), ("grip", *judge_grip(entries, c)), ("hide", *judge_hide(entries, c)),
            ("fallback", *judge_fallback(entries, c)), ("stance", *judge_stance(entries, c)), ("fp", *judge_fp(entries, c)),
            ("scope3p", *judge_scope3p(entries, c)), ("chord", *judge_chord(entries, c)), ("churn", *judge_churn(entries, c)),
            ("no ER fire", *judge_no_fire(entries, c)), ("dead fire", *judge_dead_fire(entries, c)), ("melee", *judge_melee(entries, c)), ("body", *judge_body(entries, c)),
            ("depth probe", *judge_depth(entries, c)), ("aim pose", *judge_aim(entries, c)),
            ("aim cam", *judge_aim_cam(entries, c)), ("aim fade", *judge_aim_fade(entries, c)), ("aim arms", *judge_aim_arms(entries, c)), ("aim anim", *judge_aim_anim(entries, c)),
            ("carrier", *judge_carrier(entries, c)), ("load", *judge_load(entries, c)), ("guard", *judge_guard(entries, c)), ("shoulder", *judge_shoulder(entries, c)),
            ("depth state", *judge_depth_state(entries, c)),
            ("depth control", *judge_depth_control(entries, c)), ("depth sample", *judge_depth_sample(entries, c)),
            ("depth self", *judge_depth_self(entries, c)), ("depth live", *judge_depth_live(entries, c))]


# ---- --axes ---------------------------------------------------------------------------------------------------------

def det3(cols):
    (a, b, c) = cols
    return (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]))


def axes_samples(entries):
    out = []
    for e in hold(entries, "axes "):
        m = AXES_RE.search(e["msg"])
        if m:
            v = [float(x) for x in m.groups()[:12]]
            out.append(([v[0:3], v[3:6], v[6:9]], v[9:12], int(m.group(13))))
    return out


def best_axes(samples):
    """(text, score_fwd, score_up): MW2 X -> the hand axis along cam_fwd, MW2 Z -> along world up, Y: det -1."""
    if not samples:
        return None
    use = [s for s in samples if s[2] == 1] or samples
    dot = lambda a, b: a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    score = {}
    for j in range(3):
        score[("f", j)] = sum(dot(h[j], f) for h, f, _ in use) / len(use)
        score[("u", j)] = sum(dot(h[j], [0.0, 1.0, 0.0]) for h, _, _ in use) / len(use)
    best = None
    for jx, jz in itertools.permutations(range(3), 2):
        sx = 1 if score[("f", jx)] >= 0 else -1
        sz = 1 if score[("u", jz)] >= 0 else -1
        val = abs(score[("f", jx)]) + abs(score[("u", jz)])
        if best is None or val > best[0]:
            best = (val, jx, sx, jz, sz)
    _, jx, sx, jz, sz = best
    jy = 3 - jx - jz
    unit = lambda j, s: [s if k == j else 0 for k in range(3)]
    sy = 1
    if det3([unit(jx, sx), unit(jy, sy), unit(jz, sz)]) > 0:
        sy = -1
    name = "xyz"
    sg = lambda s: "+" if s > 0 else "-"
    txt = f"{sg(sx)}{name[jx]},{sg(sy)}{name[jy]},{sg(sz)}{name[jz]}"
    return txt, abs(score[("f", jx)]), abs(score[("u", jz)])


# ---- pair mode ------------------------------------------------------------------------------------------------------

def shift(entries, dt):
    return [{**e, "t": e["t"] - dt} for e in entries]


def pair_rows(a, b):
    out = []
    probs, checked = [], 0
    for x, y, side in ((a, b, "host"), (b, a, "friend")):
        cy = cfg(y)
        if cy is None or cy.get("er_stance") not in STANCE_SETS or cy.get("peers") != "1":
            continue
        for en in hold(x, "stance enter row="):
            checked += 1
            row = kv(en["msg"], "row")
            ans = [e for e in hold(y, "peer_stance ") if kv(e["msg"], "row") == row and en["t"] - 1.0 <= e["t"] <= en["t"] + PAIR_WINDOW_S
                   and (kv(e["msg"], "patched") == "1" or kv(e["msg"], "reason") == "shared_row")]
            if not ans:
                probs.append(f"{side} enter row={row} at {en['raw'][11:23]} has no peer_stance on the other log")
    if checked == 0:
        out.append(("peer stance", "N/A", "no stance enter with er_stance + stance_peers on the other side"))
    elif probs:
        out.append(("peer stance", "FAIL", "; ".join(probs[:2])))
    else:
        out.append(("peer stance", "PASS", f"{checked} enter(s) answered on the other PC"))
    probs, checked = [], 0
    for x, y, side in ((a, b, "host"), (b, a, "friend")):
        cx = cfg(x)
        if cx is None or cx.get("hold_mode") != "hand" or not spans(y, "MODE", "on", "off"):
            continue
        checked += 1
        if not hold(x, "hand ok who=peer="):
            probs.append(f"{side}: hold_mode=hand but no `HOLD hand ok who=peer=` while the other was in MW2 mode")
    if checked == 0:
        out.append(("peer hand", "N/A", "hold_mode=hand on no side with the other in MW2 mode"))
    elif probs:
        out.append(("peer hand", "FAIL", "; ".join(probs)))
    else:
        out.append(("peer hand", "PASS", f"{checked} side(s) drew the peer's gun on its hand"))
    return out


# ---- self-test ------------------------------------------------------------------------------------------------------

def to_entries(lines):
    out = []
    for t, tag, msg in sorted(lines, key=lambda x: x[0]):
        ms = int(round(t * 1000))
        raw = f"2026-10-21 {ms // 3600000 % 24:02d}:{ms // 60000 % 60:02d}:{ms // 1000 % 60:02d}.{ms % 1000:03d} I [present] {tag} {msg}"
        out.append({"t": t, "tag": tag, "msg": msg, "raw": raw})
    return out


def CFG(hold_mode="hand", er_stance="crossbow", peers=1, weapon_axes="-y,+z,+x"):
    return (f"cfg hold_mode={hold_mode} hand_bone=-1 auto=1 axes=+x,+z,+y grip=[0.000,0.000,0.000] rot=[0.0,0.0,0.0] smooth=0.00 "
            f"grips=0 mounted=hand tp_ads_hold=hand er_aim=0 er_stance={er_stance} refresh=chord min_switch_s=1.5 peers={peers} "
            f"peer_delay_s=1.5 vt43=0x2b70c78 vt44=0x2b70d48 source=weapon weapon_axes={weapon_axes} hide_patch=1 vtw=0x2b37a70/0x2b38808/0x2b70e68 "
            f"weapon_rake=-18.0 grip_table=34 grip_cfg=0 exit_defer_s=3.0 body_occl=1 probe_er_depth=1 ads_pose=er_aim ads_patch=ads probe_er_aim=1 "
            f"er_depth=1 depth_state=1 depth_sample=1 bias=0.030+0.0040z max_turn=3.0 aim_cam=keep")


def session(no_hand=False, many_fallback=False, det=False, no_exit=False, restored0=False, fp_enter=False, stuck_chord=False,
            refresh_fail=False, stray=False, foreign=False, hold_mode="hand", er_stance="crossbow", wvt=False, no_weapon=False,
            hide_foreign=False, blink=False, peek_exit=False, old_guess=False, auto_old=False, no_grip=False, churn=False,
            no_resume=False, body_none=False, no_body=False, depth_off=False, depth_pins=False, depth_desc=False,
            aim_left=False, aim_chord=False, aim_nostate=False, aim_foreign=False, death_refresh=False, gave_up=False,
            aim_late_disable=False, aim_stance_chord=False, ds_hook_off=False, ds_write=False, ds_mismatch=False,
            ds_sample_fail=False, ds_forward=False, ds_self_fail=False, ds_trip=False, cam_aim=False, cam_refused=False,
            cam_zero=False, ds_dead=False, ds_idle_a=False, spawn_wait=False, fade_low=False, fade_zero=False, ds_scale_bad=False,
            ds_edge=False, ds_trip_self=False, ds_near_cam=False, chord_ignored=False, dead_shot=False):
    L = []
    a = lambda t, tag, msg: L.append((t, tag, msg))
    # C6 round 6: the aim-camera filter (installed at the stance's first tick, one kept span over the ADS episode)
    if cam_refused:
        a(11.0, "HOLD", "aim cam refused reason=site (nothing hooked; ER's aim camera / reticle stay as round 5)")
    else:
        a(11.0, "HOLD", "aim cam keep installed getter=0x7ff6003f4a90 sites=3 ret_cam=0x7ff6003b14ea ret_ret=0x7ff600772633 ret_fade=0x7ff60065b3e0 (read-only)")
    a(20.6, "HOLD", "aim probe anim=40000 prec=0 arm=3 row=2000000 type=55 cat=11 camtype=0 mode=1 ads=1 aim=0 snipe=1.00 n=5/300")
    a(20.73, "HOLD", f"aim probe anim=40100 prec=1 arm=3 row=2000000 type=55 cat=11 camtype={'2' if cam_aim else '0'} mode=1 ads=1 aim=1 snipe=1.00 n=6/300")
    # C6 round 7: ER's snipe body fade (a fade = the round-6 invisible body) and the P-A7 pose probe
    a(21.2, "HOLD", f"aim probe anim=40100 prec=1 arm=3 row=2000000 type=55 cat=11 camtype=0 mode=1 ads=1 aim=1 snipe={'0.25' if fade_low else '1.00'} n=7/300")
    a(20.72, "HOLD", "aim pose phase=carry rw_p=[0.210,1.050,0.180] rw_x=[0.120,0.980,-0.150] rw_y=[0.700,-0.190,-0.680] n=1/400")
    a(20.97, "HOLD", "aim pose phase=aim rw_p=[0.210,1.051,0.180] rw_x=[0.120,0.980,-0.150] rw_y=[0.700,-0.190,-0.680] moved=0.001 turned=0.1 arm=0 lh_d=0.312 q=[16000000,-1,-1,-1,-1,-1,-1,-1,-1,-1] bolt=[-1,-1,-1] snipe=1.00 mode=1 n=2/400")
    a(20.98, "HOLD", "aim hold_fallback gun=m4_mp deg=48.2 max=20 (ER's aim pose did not turn the weapon: tp_ads_hold = \"aim\" shoulders our gun along the camera)")
    a(21.75, "HOLD", f"aim cam kept ms=1250 hits_cam={0 if cam_zero else 75} hits_ret=75 hits_fade={0 if fade_zero else 75} (ER's aim camera / crossbow reticle / snipe fade masked; the body turn saw the real bit)")
    # C6 round 6 (the 21:25 respawn): chords wait `spawn` up to 12 s after a world enter (by design, never stuck)
    if spawn_wait:
        a(11.05, "HOLD", "chord refused reason=spawn (retried every 0.5s)")
    # C6 aim 4: one third-person ADS episode in ER's crossbow aim (patch, on, align, off, restore)
    a(20.5, "HOLD", "aim patch row=2000000 orig=[3,9] new=[11,55] route=pad when=ads (ER's crossbow aim: the type for this ADS)")
    if aim_nostate:
        a(21.05, "HOLD", "aim fail reason=no_state n=1/2 row=2000000 (precision_shooting not seen within 0.5s; input released)")
        a(21.2, "HOLD", "aim off row=2000000 ms=150 why=release")
        a(21.2, "HOLD", "aim restore row=2000000 restored=1 orig=[3,9] why=release")
        a(22.0, "HOLD", "aim patch row=2000000 orig=[3,9] new=[11,55] route=pad when=ads (ER's crossbow aim: the type for this ADS)")
        a(22.55, "HOLD", "aim fail reason=no_state n=2/2 row=2000000 (precision_shooting not seen within 0.5s; input released)")
        a(22.7, "HOLD", "aim off row=2000000 ms=150 why=release")
        a(22.7, "HOLD", "aim restore row=2000000 restored=1 orig=[3,9] why=disabled")
        a(22.7, "HOLD", "aim disabled reason=no_state (tp_ads_pose off for this session; the stance and the hand pose keep working)")
    else:
        a(20.72, "HOLD", "aim on t=210ms row=2000000 route=pad (precision_shooting set: ER holds the crossbow aim)")
        a(20.75, "HOLD", "aim align gun=m4_mp deg=6.5 rake=-18.0 (the barrel vs the camera's aim point at 30 m on ER's aim pose; turned about the grip)")
        if aim_chord:
            a(21.0, "HOLD", "chord sent route=pad seq=9 arm=3->1 dt=820ms took=1 row=2000000 kind=enter")
        if not aim_left:
            a(21.6, "HOLD", "aim off row=2000000 ms=120 why=release")
            a(21.6, "HOLD", "aim restore row=2000000 restored=1 orig=[3,9] why=release")
    if aim_foreign:
        a(21.7, "HOLD", "aim foreign row=2000000 cur=[3,55] orig=[3,9] (left alone)")
    # review C6 aim 4: two busy presses (rolls) after a good episode on an old DLL: WARN, not the cached-type FAIL
    if aim_late_disable:
        for k, t0 in enumerate((23.0, 25.0), 1):
            a(t0, "HOLD", "aim patch row=2000000 orig=[3,9] new=[11,55] route=pad when=ads (ER's crossbow aim: the type for this ADS)")
            a(t0 + 0.55, "HOLD", f"aim fail reason=no_state n={k}/2 row=2000000 (precision_shooting not seen within 0.5s; input released)")
            a(t0 + 0.7, "HOLD", "aim off row=2000000 ms=150 why=release")
            a(t0 + 0.7, "HOLD", f"aim restore row=2000000 restored=1 orig=[3,9] why={'disabled' if k == 2 else 'release'}")
        a(25.7, "HOLD", "aim disabled reason=no_state (tp_ads_pose off for this session; the stance and the hand pose keep working)")
    # review C6 aim 4: the `tp_ads_patch = "stance"` fields must never be on the row during a chord
    if aim_stance_chord:
        a(30.0, "HOLD", "aim patch row=2000000 orig=[3,9] new=[11,55] when=stance (after the enter chords; kept while posed)")
        a(30.5, "HOLD", "chord sent route=pad seq=11 arm=3->1 dt=820ms took=1 row=2000000 kind=enter")
        a(31.0, "HOLD", "aim restore row=2000000 restored=1 orig=[3,9] why=release")
    # C6 aim 4 (the 18:54 replay): the player died with the exit's chords pending (old DLL: refresh fail): N/A, no FAIL
    if death_refresh:
        a(52.0, "NUKE", "player dead t_ms=17")
        a(53.0, "HOLD", "chord sent route=pad seq=6 arm=1->1 dt=817ms took=0")
        a(54.0, "HOLD", "chord refused reason=prompt (retried every 0.5s)")
        a(56.0, "HOLD", "chord sent route=pad seq=7 arm=1->1 dt=817ms took=0")
        a(56.0, "HOLD", "stance refresh fail row=-1 arm=1 (two chords did not change the grip)")
        a(58.0, "TASK", "world left (loading / title)")
    if gave_up:
        a(52.0, "HOLD", "stance exit_chords gave_up row=2000000 arm=1 (two exit chords did not change the grip; the grip is the player's, the stance stays enabled)")
    a(0.0, "BOOT", "mw2er 0.3.0")
    a(0.5, "HOLD", CFG(hold_mode, er_stance, weapon_axes="+y,+x,+z" if old_guess else "-y,+z,+x"))
    # C6 depth probe (read-only)
    if depth_off:
        a(0.6, "DEPTH", "probe off why=sites er_depth_dsb_vtable: rtti")
    else:
        a(0.6, "DEPTH", "probe on sites 6 checked (WW 2.7.2.0; read-only: chain / desc / wrapper words, task thread, nothing bound or sampled)")
        a(12.0, "DEPTH", f"probe chain rs=0x7ff100000000 pins={'5' if depth_pins else '6'}/6 rs_vt=1 ctx_vt=1/1 a88_eq=1 dsb=0x7ff1 vt=1 slot7=1 count=1 obj=0x1 wrap=0x2 res=0x3 module=d3d12:1 tile=0x4 changes=0 | B def_ds=0x5 vt=1 count=1 res=0x6")
        a(12.0, "DEPTH", f"probe desc A fmt=20 3840x2160 dim=3 array=1 mips=1 samples=1 flags=0x{'a' if depth_desc else '2'} deny_srv={1 if depth_desc else 0} bb=3840x2160 rule={'deny_srv' if depth_desc else 'ok'}")
        a(12.0, "DEPTH", "probe desc B fmt=20 3840x2160 dim=3 array=1 mips=1 samples=1 flags=0x2 deny_srv=0 bb=3840x2160 rule=ok")
        a(12.1, "DEPTH", "probe wrap t=0.25 wrap=0x2 +00:0 +08:c0")
        a(70.0, "DEPTH", "probe verdict chain=12/12 desc=12/12 res_changes=0 wrap_state_words=none state=unmeasured")
    # C6 depth 5: the barrier probe, the sample / self check, the live prime
    if ds_hook_off:
        a(0.7, "DEPTH", "state hook off why=prologue (nothing hooked; world guns keep the capsules)")
    else:
        a(0.7, "DEPTH", "state hook on target=0x7ffa00001234 module=D3D12Core.dll slot=26 (read-only: transitions on ER's scene depth counted; original first) sample=1 er_depth=1")
        a(13.0, "DEPTH", f"state res=0x3 q=0xc0 at_ui={{0xC0:300,0xE0:0,0x10:{'4' if ds_write else '0'},unknown:0,other:0}} frames=304 barriers=1200 lists=600 queues=1 threads=1 split=0 alias=0 uav=0 mismatch={'2' if ds_mismatch else '0'} unmatched=0 table_full=0 bad_sub=0 vt_eq=600/600")
        a(13.0, "DEPTH", "state path seq=12 [0xc0>0x10 sub=all q=hal t=rt, 0x10>0xc0 sub=all q=hal t=rt] at_ui=0xc0")
        if ds_sample_fail:
            a(14.0, "DEPTH", "probe sample off why=hresult (samples=0 err=d3d: fence(er depth sample) never passed 0x00000000)")
        a(15.0, "DEPTH", f"probe sample n=1 conv={'forward' if ds_forward else 'reverse'} sky=0.000000 center=0.020000 zeros=6/16 ones=0/16 state=0xc0 chest=1 d_chest=0.035088")
        a(15.0, "DEPTH", "probe self_s k=1 z=3.512 d=0.015800 zl=3.171 o=0.341 spread=0.012 edge=0 turn=0.20 mv=0.010 zd=0.05549 cand=B")
        a(15.0, "DEPTH", "probe self n=1 ok=1/1 edges=0/1 o_cm=34.1 err_cm_p50=0.0 p95=0.0 span_m=0.00 scale=unproven slope=NaN margin_rel=0.108 near_cam=0.0501 far_cam=5000.0 model=inf lag=0 why=few pass=0 cand=B")
        why = "spread" if ds_self_fail else ("scale" if ds_scale_bad else ("edge" if ds_edge else "none"))
        a(50.0, "DEPTH", f"probe self n=8 ok={'3' if ds_self_fail else '8'}/8 far=0 edges={'4' if ds_edge else '0'}/{'12' if ds_edge else '8'} o_cm=2.0 err_cm_p50={'40.0' if ds_self_fail else '1.5'} p95=6.0 "
                         f"span_m={'1.40' if ds_scale_bad else '0.20'} scale={'bad' if ds_scale_bad else 'unproven'} s={'0.900' if ds_scale_bad else '1.098'} s_iqr=0.700..1.140 "
                         f"n_eff={'0.0501' if ds_scale_bad else '0.0553'} near_rel={'1.000' if ds_scale_bad else '1.104'} bias_add_cm=0.0 lev=2/2 ratio=1.104 "
                         f"near_cam=0.0501 far_cam=5000.0 model=inf lag=0 why={why} pass={'1' if why == 'none' else '0'} cand=B")
        why = "write_at_ui" if ds_write else ("mismatch" if ds_mismatch else "none")
        # C6 round 6: the positive control (B's own UI barriers) and the census
        a(13.0, "DEPTH", f"state control B ui={'0' if ds_dead else '300'}/300 calls={'0' if ds_dead else '91000'} bars=180000 trans=120000 census_full=0 ok={'0' if ds_dead else '1'}")
        a(13.0, "DEPTH", "census res=0x6 n=600 states=0xd0 p0=unknown>unknown p1=0x10>0xc0 sub_all=0 split=0 is=B vt_ok=1")
        if ds_dead:
            a(55.0, "DEPTH", "state verdict ready=0 frames=3600 readable=0/3600 write_at_ui=0 unknown=3600 queues=0 threads=0 mismatch=0 split=0 alias=0 table_full=0 vt_eq=0/0 lists=0 transitions=0 why=hook cand=A")
        elif ds_idle_a:
            # the 20:57 replay with the corrected probe: A idle, B ready
            a(55.0, "DEPTH", "state verdict ready=0 frames=3600 readable=0/3600 write_at_ui=0 unknown=3600 queues=0 threads=0 mismatch=0 split=0 alias=0 table_full=0 vt_eq=0/0 lists=0 transitions=0 why=idle cand=A")
            a(55.0, "DEPTH", "state verdict ready=1 frames=3600 readable=3600/3600 write_at_ui=0 unknown=0 queues=1 threads=1 mismatch=0 split=0 alias=0 table_full=0 vt_eq=7200/7200 lists=7200 transitions=14400 why=none cand=B")
        else:
            a(55.0, "DEPTH", f"state verdict ready={'0' if why != 'none' else '1'} frames=3600 readable=3600/3600 write_at_ui={'4' if ds_write else '0'} unknown=0 queues=1 threads=1 mismatch={'2' if ds_mismatch else '0'} split=0 alias=0 table_full=0 vt_eq=7200/7200 why={why}")
        a(56.0, "DEPTH", f"live on res=0x3 fmt=19 size=3840x2160 conv=reverse near={'0.0501' if ds_near_cam else '0.0553'} near_cam=0.0501 near_rel={'1.000' if ds_near_cam else '1.104'} far_model=inf bias=0.030+0.0040z bias_add_cm=0.0 max_turn=3.0 cand=B (ER's depth primes our world-gun depth in third person with near = n_eff; capsules stay)")
        a(58.0, "DEPTH", "live n=400 copies=380 primed=370 miss=turn:5,fp:12 attempts=400 locked=0 state_at_ui=0xc0 live=1 tripped=0")
        if ds_trip:
            a(59.0, "DEPTH", "live trip why=mismatch (capsules only for the session; the probe keeps logging)")
        if ds_trip_self:
            # round 7 review: live on the unproven scale, then the span proves it bad: the live candidate trips `self`
            a(58.5, "DEPTH", "probe self n=30 ok=28/30 edges=0/30 o_cm=15.0 err_cm_p50=2.0 p95=8.0 span_m=1.50 scale=bad slope=1.060 margin_rel=0.000 "
                             "near_cam=0.0501 far_cam=5000.0 model=inf lag=0 why=scale pass=0 cand=B")
            a(58.5, "DEPTH", "live trip why=self (capsules only for the session; the probe keeps logging)")
    # C6 depth fallback: the body capsules
    if body_none:
        a(10.5, "HOLD", "body bones none reason=no_pelvis_name tries=20 (no body capsules this session)")
    elif not no_body:
        a(10.5, "HOLD", "body bones found=17/17 flver=Pelvis:18,Spine:19 (skeleton n=150)")
        a(10.6, "HOLD", "body ok who=self capsules=14 bones=Pelvis,Spine")
        a(10.6, "COOP", "worldgun body occlusion first frame capsules=14 guns=1 (our depth only; ER's depth never bound)")
    # C6 grip 3
    if not no_grip:
        a(10.1, "HOLD", "grip who=self gun=m4_mp src=table offset=[0.016,-0.291,0.000] rot=[0.000,0.000,-18.000] rake=-18.0 (weapon frame)")
    a(10.0, "MODE", "on weapon=m4_mp mag=30/90")
    # C6 hide: the row patch, then the marker (renewed 27 s later; `blink` = the old 30 s expiry instead)
    a(10.03, "HOLD", "hide foreign sp=79001090 fields=stateInfo:0x5 (neither the package nor the patch)" if hide_foreign
      else "hide row patched sp=79001090 state=184 vow=0xffff wep_param_change=1 rates=1.0 motion_interval=-1")
    a(10.06, "COOP", "marker sent sp=79001090 call_ok=1 peers=0")
    if blink:
        a(40.06, "COOP", "marker sent sp=79001090 call_ok=1 peers=0")
    else:
        a(37.06, "COOP", "marker renewed sp=79001090 remove_ok=1 apply_ok=1")
    if not no_hand:
        src = "src=bone space=world bone=57 scale=1.000 det=1.000" if no_weapon else "src=weapon_e0 d=0.052 bone=57 row=2000000"
        a(10.1, "HOLD", f"hand ok who=self {src}")
        if not no_weapon:
            a(10.1, "HOLD", "weapon ok who=self src=e0 d=0.052 row=2000000")
            a(10.1, "HOLD", "src who=self src=weapon was=none")
    else:
        a(10.1, "HOLD", "fallback who=self reason=space")
    if wvt:
        a(10.15, "HOLD", "weapon miss who=self reason=weapon_vtable row=2000000")
    a(10.2, "HOLD", "fallback who=self reason=read" if not det else "fallback who=self reason=det")
    if not no_hand:
        a(10.25, "HOLD", f"hand ok who=self {'src=bone space=world bone=57 scale=1.000 det=1.000' if no_weapon else 'src=weapon_e0 d=0.050 bone=57 row=2000000'}")
    if auto_old:
        a(13.0, "HOLD", "weapon axes auto axes=-z,+y,+x votes=45/45 (set [coop] weapon_axes = \"-z,+y,+x\" to keep it)")
    else:
        a(13.0, "HOLD", "weapon axes auto axes=-y,+z,+x votes=45/45 accepted=1 (the rig map is -y,+z,+x; confirmed)")
    a(40.0, "HOLD", "weapon miss who=self reasons=weapon_stale:3 frames=3/1800 30s")
    # C6 scope peek: a third-person scoped ADS while the stance is on (it stays on)
    a(42.0, "FP", "on eye=(1.00,2.60,3.10) cam_gate=ok mode=hook hide=render_flag player=0x1 peek=1")
    if peek_exit:
        a(42.5, "HOLD", "stance exit row=2000000 restored=1 arm=3 why=exit")
        a(44.0, "HOLD", "stance enter row=2000000 orig=[23,1,0,3,10] arm=1 left=-1")
    a(43.5, "FP", "off reason=f6")
    a(40.0, "HOLD", f"fallback who=self reasons=read:{30 if many_fallback else 2} frames={300 if many_fallback else 2}/1800 30s")
    a(11.0, "HOLD", "stance enter row=2000000 orig=[23,1,0,3,10] arm=1 left=-1")
    a(11.0, "HOLD", "stance row=2000000 shared_note (every character using this row shares the motion set until the restore)")
    if stuck_chord:
        a(11.1, "HOLD", "chord refused reason=prompt (retried every 0.5s)")
        a(25.0, "HOLD", "chord sent route=pad seq=1 arm=1->3 dt=820ms took=1")
    else:
        a(11.9, "HOLD", "chord sent route=pad seq=1 arm=1->3 dt=820ms took=1")
    if refresh_fail:
        a(13.0, "HOLD", "stance refresh fail row=2000000 arm=1 (two chords did not change the grip)")
    # C6 part 16.9: ER busy (a jump) ignored a chord: retried with backoff, never a failure
    if chord_ignored:
        a(12.2, "HOLD", "chord sent route=pad seq=2 arm=3->3 dt=820ms took=0 row=2000000 kind=enter")
        a(12.2, "HOLD", "chord verdict=ignored head=10202040 arm=3->3 retry_s=0.5 n=1 (no grip-switch animation: ER ignored the input; not counted)")
        a(12.3, "HOLD", "chord refused reason=er_busy (retried every 0.5s)")
        a(12.9, "HOLD", "chord sent route=pad seq=3 arm=3->1 dt=820ms took=1 row=2000000 kind=enter")
        a(12.9, "HOLD", "chord verdict=played head=16000000 arm=3->1 row=2000000 played=1")
    if stray:
        a(15.0, "HOLD", "stray_bullet row=52000000 n=1 arm=3")
    a(16.0, "HOLD", "stray_bullet row=10170000 ignored=use_item")
    a(20.0, "HOLD", "melee suppressed n=2/30s (R3 does nothing in third person while er_stance is on)")
    a(30.0, "FP", "on yaw=0.0")
    if not no_exit:
        a(30.1, "HOLD", f"stance exit row=2000000 restored={0 if restored0 else 1} arm=3 why=exit")
    if foreign:
        a(30.2, "HOLD", "stance foreign row=2000000 cur=[1,1,1,1,1] orig=[23,1,0,3,10] (left alone)")
    if fp_enter:
        a(31.0, "HOLD", "stance enter row=2000000 orig=[23,1,0,3,10] arm=3 left=-1")
        a(31.5, "HOLD", "stance exit row=2000000 restored=1 arm=3 why=exit")
    a(30.1, "HOLD", "stance exit_defer s=3.0 arm=3 resumable=1")
    a(35.0, "FP", "off reason=f6")
    a(37.0, "HOLD", "stance enter row=2000000 orig=[23,1,0,3,10] arm=1 left=-1")
    # C6 pose 3: a quick F6 toggle: exit (deferred), back 1.5 s later: resumed, no chords
    a(45.0, "FP", "on yaw=0.0")
    a(45.1, "HOLD", "stance exit row=2000000 restored=1 arm=3 why=exit")
    a(45.1, "HOLD", "stance exit_defer s=3.0 arm=3 resumable=1")
    a(46.6, "FP", "off reason=f6")
    a(46.6, "HOLD", "stance enter row=2000000 orig=[23,1,0,3,10] arm=3 left=-1")
    if not no_resume:
        a(46.6, "HOLD", "stance resume row=2000000 arm=3 (no chords)")
    if churn:
        a(47.5, "HOLD", "chord sent route=pad seq=2 arm=3->1 dt=820ms took=1")
    a(50.0, "MODE", "off")
    a(50.1, "HOLD", "stance exit row=2000000 restored=1 arm=3 why=exit")
    a(51.0, "HOLD", "chord sent route=pad seq=3 arm=3->1 dt=700ms took=1")
    # 2026-10-24: a third-person death with the death gate (host_c6_2026-10-24.log 13:27:52 had 25 shots in it)
    a(52.0, "SHOT", "dead gate on hp=0 death_flag=0 camtype=Some(0) fp=0 mode=1 (no MW2 fire / ADS / reload / AC-130 / nuke / melee / heal until alive after the respawn)")
    a(52.0, "MOVE", "suspend reason=death hp=0 death_flag=0 ticks=2854 pos=(-23.82,-2.62,12.91)")
    a(53.0, "SHOT", "refused why=dead n=1 fp=0 (until alive after the respawn)")
    if dead_shot:
        a(54.0, "SHOT", "n=971 mag=14/60 id=79103000 mw2=40 d=2.7m src=map spread=3.00 origin=(-24.20,-1.43,12.32) dir=(-0.038,-0.297,-0.954) gun=scar_mp pellets=1 spawned=1")
    a(57.5, "SHOT", "dead gate off dead_s=5.5 refused=1 (alive 0.5s after the respawn)")
    a(58.0, "SHOT", "n=972 mag=13/60 id=79103000 mw2=40 d=2.4m src=map spread=4.07 origin=(-24.22,-1.46,12.32) dir=(0.004,-0.352,-0.936) gun=scar_mp pellets=1 spawned=1")
    a(60.0, "TASK", "end")
    return L


MUTATIONS = [
    ("no hand frame", {"no_hand": True}, "hand"),
    ("too many self fallbacks", {"many_fallback": True}, "hand"),
    ("a det fallback (inside-out gun)", {"det": True}, "fallback"),
    ("an enter without exit before FP on", {"no_exit": True}, "stance"),
    ("an exit with restored=0", {"restored0": True}, "stance"),
    ("a foreign row", {"foreign": True}, "stance"),
    ("stance entered in first person", {"fp_enter": True}, "fp"),
    ("a chord refused for 14 s", {"stuck_chord": True}, "chord"),
    ("refresh fail", {"refresh_fail": True}, "chord"),
    ("a stray bullet", {"stray": True}, "no ER fire"),
    # 2026-10-24
    ("a MW2 shot while dead", {"dead_shot": True}, "dead fire"),
    ("a weapon-model vtable miss", {"wvt": True}, "weapon"),
    ("ER's weapon model never used", {"no_weapon": True}, "weapon"),
    ("a foreign marker row", {"hide_foreign": True}, "hide"),
    ("the marker expired after 30 s (blink)", {"blink": True}, "hide"),
    ("a stance exit inside a scope peek", {"peek_exit": True}, "scope3p"),
    ("weapon_axes = the old backwards guess", {"old_guess": True}, "grip"),
    ("an auto vote without accepted= (old DLL)", {"auto_old": True}, "grip"),
    ("no HOLD grip line on the weapon frame", {"no_grip": True}, "grip"),
    ("chords around a resumed F6 toggle", {"churn": True}, "churn"),
    ("a re-enter in the window without resume", {"no_resume": True}, "churn"),
    ("body bones none", {"body_none": True}, "body"),
    ("no body capsules although our gun was drawn", {"no_body": True}, "body"),
    ("the depth probe's sites failed", {"depth_off": True}, "depth probe"),
    ("a depth chain pin missed", {"depth_pins": True}, "depth probe"),
    ("the depth desc denies shader reads", {"depth_desc": True}, "depth probe"),
    # C6 aim 4
    ("the aim fields never restored", {"aim_left": True}, "aim pose"),
    ("a grip chord inside an aim episode", {"aim_chord": True}, "aim pose"),
    ("a grip chord with the stance-mode aim fields written", {"aim_stance_chord": True}, "aim pose"),
    ("ER's aim state never showed (disabled)", {"aim_nostate": True}, "aim pose"),
    ("a foreign aim field", {"aim_foreign": True}, "aim pose"),
    # C6 depth 5
    ("the barrier hook refused (prologue)", {"ds_hook_off": True}, "depth state"),
    ("a write state at UiBegin (verdict not ready)", {"ds_write": True}, "depth state"),
    ("a fold mismatch (verdict not ready)", {"ds_mismatch": True}, "depth state"),
    ("the sample's D3D12 failure", {"ds_sample_fail": True}, "depth sample"),
    ("a forward-Z sample", {"ds_forward": True}, "depth sample"),
    ("the chest self check failed", {"ds_self_fail": True}, "depth self"),
    ("a live trip", {"ds_trip": True}, "depth live"),
    ("unproven-then-live, then the scale bad (trip self)", {"ds_trip_self": True}, "depth live"),
    # C6 part 16.8
    ("live on CSCam's near instead of n_eff", {"ds_near_cam": True}, "depth live"),
    # C6 round 6
    ("ER's aim camera ran while kept (camtype 2, prec 1)", {"cam_aim": True}, "aim cam"),
    ("the aim-camera filter refused", {"cam_refused": True}, "aim cam"),
    ("a kept span that never matched ER's camera call", {"cam_zero": True}, "aim cam"),
    # C6 round 7
    ("ER's snipe fade never masked (hits_fade=0)", {"fade_zero": True}, "aim cam"),
    ("our body faded out while kept (snipe 0.25)", {"fade_low": True}, "aim fade"),
    ("the self check's scale is bad over a span", {"ds_scale_bad": True}, "depth self"),
    ("too many edge samples", {"ds_edge": True}, "depth self"),
    ("the barrier hook saw nothing (control ok=0)", {"ds_dead": True}, "depth control"),
    ("a verdict why=hook", {"ds_dead": True}, "depth state"),
]


def carrier_session(mode="crossbow", trip=False, refused=False, foreign=False, restore0=False, missing=False, prologue=False,
                    open_end=False, pick_none=False, grip_fail=False):
    """C6 part 16: a synthetic carrier session (MW2 on in third person: swap, grip, F6 restore, swap again, F5 restore)."""
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG().replace("aim_cam=keep", f"aim_cam=keep carrier={mode} give=0 axes_xbow=-y,+z,+x"))]
    a = lambda t, tag, msg: L.append((t, tag, msg))
    a(10.0, "MODE", "on weapon=m4_mp mag=30/90")
    if prologue:
        a(10.1, "HOLD", "xbow off why=prologue fn=equip_item (the carrier stays off for the session; the stance keeps the patched sword)")
    if mode == "dry":
        a(10.2, "HOLD", f"xbow dry slot=1 orig=2000000 idx=410 handle=0x80800001 arm=1 pick={'none' if pick_none else '43020000 idx=413 handle=0x80800004'} bolts=[52000000,-1] journal=D:\\x\\state\\carrier.json (P-X1: no call, no write) n_inv=120 give=0")
        a(60.0, "TASK", "end")
        return L
    a(10.1, "HOLD", "xbow pins ok equip_item=0x140249160 add_item=0x140246480 create=0x1406735c0 find=0x14024c560 (2.7.2.0, first bytes checked)")
    a(10.2, "HOLD", "xbow call slot=1 orig=2000000 carrier=43020000 handle=0x80800004 idx=413 arm=1 (EquipItem, menu args 1,1,0; verifying)")
    if refused:
        a(11.2, "HOLD", "xbow refused reason=verify ms=1000 row_now=2000000 (ER did not take the crossbow: restored, off for the session)")
    a(10.3, "HOLD", "xbow on slot=1 orig=2000000 carrier=43020000 ms=33 (ER's real crossbow: its carry and aim; hidden by the marker row)")
    a(10.4, "HOLD", "stance carrier enter row=43020000 arm=1 orig_arm=1 (the real crossbow: nothing patched; grip to two-handed)")
    a(10.9, "HOLD", f"xbow grip via=arm_style arm=1->{1 if grip_fail else 3} ok={0 if grip_fail else 1} played=0 try=1 (P-X2)")
    if grip_fail:
        a(12.0, "HOLD", "xbow grip fail row=43020000 arm=1 (two chords ER played did not change the grip: carried one-handed; no more chords)")
    if trip:
        a(15.0, "HOLD", "xbow trip why=bolt_used qty=20->19 (a bolt was used: restore now, the carrier off for the session)")
    if foreign:
        a(19.9, "HOLD", "xbow foreign slot=1 now=0x80800099 carrier=0x80800004 (the gear changed meanwhile: nothing touched, journal dropped)")
    a(20.0, "FP", "on yaw=0.0")
    a(20.0, "HOLD", "xbow restore call why=fp slot=1 orig=2000000 idx=410 from_disk=0 (verifying)")
    a(20.05, "HOLD", f"xbow restore why=fp ok={0 if restore0 else 1} ms=33")
    a(25.0, "FP", "off reason=f6")
    a(26.6, "HOLD", "xbow on slot=1 orig=2000000 carrier=43020000 ms=33 (ER's real crossbow)")
    if missing:
        a(39.9, "HOLD", "xbow restore_missing orig=2000000 handle=0x80800001 (the original is no longer in the inventory)")
    a(40.0, "MODE", "off")
    if not open_end:
        a(40.05, "HOLD", "xbow restore why=mode_off ok=1 ms=33")
    a(60.0, "TASK", "end")
    return L


def load_session(lost=False, bolt_drop=False, undo0=False, keep0=False, open_end=False, wait_only=False):
    """C6 part 16.16: a carrier swap, the chord to 3, the load (verified), an aim episode loaded, F5 off (undo, restore)."""
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG().replace("aim_cam=keep", "aim_cam=keep carrier=crossbow give=0 axes_xbow=-y,+z,+x load=1"))]
    a = lambda t, tag, msg: L.append((t, tag, msg))
    a(10.0, "MODE", "on weapon=m4_mp mag=30/90")
    a(10.2, "HOLD", "xbow on slot=1 orig=2000000 carrier=43020000 ms=33 (ER's real crossbow)")
    a(10.3, "HOLD", "xbow load wait reason=grip arm=1 loaded=0 bolts=4 (16.16: no load now; retried every tick)")
    a(11.0, "HOLD", "xbow grip via=chord arm=1->3 ok=1 played=1")
    a(11.1, "HOLD", "xbow load wait reason=settle arm=3 loaded=0 bolts=4 (16.16: no load now; retried every tick)")
    if wait_only:
        a(40.0, "MODE", "off")
        a(40.05, "HOLD", "xbow restore why=mode_off ok=1 ms=33")
        a(60.0, "TASK", "end")
        return L
    a(12.1, "HOLD", "xbow load write slot=1 handle=0x80800141 arm=3 loaded=0 egd=0 bolts=4 try=1/3 wrote_egd=1 wrote_pl=1 (verifying)")
    a(12.6, "HOLD", f"xbow load method=write slot=1 ok={0 if keep0 else 1} loaded=0->{0 if keep0 else 1} egd=0->{0 if keep0 else 1} bolts=4->{3 if bolt_drop else 4} ms=517 arm=3 (..)")
    if keep0:
        a(12.6, "HOLD", "xbow load undo why=verify slot=1 ok=1 egd=0->0 wrote_egd=1 wrote_pl=1 (our load restored: unloaded)")
    if lost:
        a(20.0, "HOLD", "xbow load lost loaded=1->0 bolts=3 (ER unloaded the crossbow itself; not loaded again this swap)")
    a(25.0, "HOLD", f"aim episode n=1 ms=3000 carrier=1 mode=1 row=43020000 cam=keep arm=3->3 aim=46037130 aim_at_ms=370 aim_last=46037100 aim_ticks=9 head=16000000->16000000 "
                    f"prec_ms=2900 prec_at_ms=33 ours_ms=3000 ads_ms=2900 camtypes=[0] snipe_min=1.00 loaded=1->1 bolts=4->4 route=kb")
    if not open_end:
        a(40.0, "MODE", "off")
        if not keep0:
            a(40.0, "HOLD", f"xbow load undo why=mode_off slot=1 ok={0 if undo0 else 1} egd=1->0 wrote_egd={0 if undo0 else 1} wrote_pl=1 (our load restored: unloaded)")
        a(40.05, "HOLD", "xbow restore why=mode_off ok=1 ms=33")
    a(60.0, "TASK", "end")
    return L


def arms_session(kind="vanilla", moved=True, walking=False):
    """C6 part 16.1: probe-0 samples: a real Light Crossbow (row 43020000) aimed with MW2 mode off (or the carrier)."""
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG())]
    mode, car = ("0", "0") if kind == "vanilla" else ("1", "1")
    head = "16020120" if walking else "16000000"
    for k in range(4):
        L.append((10.0 + 0.25 * k, "HOLD", f"aim pose phase=aim rw_p=[0.2,1.3,0.3] rw_x=[0,1,0] rw_y=[1,0,0] moved={'0.250' if moved else '0.010'} turned=1.0 "
                                           f"arm={'1' if moved else '0'} lh_d=0.3 q=[{head},-1] bolt=[52000000,-1,-1] snipe=1.00 mode={mode} row=43020000 carrier={car} n={k + 1}/400"))
    L.append((20.0, "TASK", "end"))
    return L


def episode_session(eps):
    """C6 part 16 addendum: `HOLD aim episode` lines; eps = [(carrier, mode, cam, arm, aim)]."""
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG())]
    for k, (car, mode, cam, arm, aim) in enumerate(eps, 1):
        L.append((10.0 + 3 * k, "HOLD", f"aim episode n={k} ms=1800 carrier={car} mode={mode} row=43020000 cam={cam} arm={arm}->{arm} aim={aim} "
                                        f"aim_at_ms={'370' if aim != 'none' else '-'} aim_last={aim if aim != 'none' else '?'} aim_ticks=3 head=16000000->16000000 "
                                        f"prec_ms=1700 prec_at_ms=100 ours_ms=1700 ads_ms=1800 camtypes=[0] snipe_min=1.00 loaded=0->0 bolts=20->20 route=kb"))
    L.append((60.0, "TASK", "end"))
    return L


def abort_session(done=True, probe=False):
    """C6 part 16 addendum (the 10:00:35 replay): the chord aborted by the user's menu after ER played 29060; the grip
    reached 3 (a new DLL logs `chord done`), the carrier exit 15 s later."""
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG().replace("aim_cam=keep", "aim_cam=keep carrier=crossbow give=0 axes_xbow=-y,+z,+x"))]
    a = lambda t, tag, msg: L.append((t, tag, msg))
    a(10.0, "HOLD", "xbow on slot=1 orig=2000000 carrier=43020000 ms=16 (ER's real crossbow)")
    a(10.0, "HOLD", "stance carrier enter row=43020000 arm=1 orig_arm=3 grip_via=arm_style (the real crossbow: nothing patched; grip to two-handed)")
    a(10.5, "HOLD", "xbow grip via=arm_style arm=1->1 ok=0 played=0 try=1 (P-X2: reverted: the chord follows)")
    a(10.73, "HOLD", "chord refused reason=aborted (retried every 0.5s)")
    if done:
        a(10.75, "HOLD", "chord done arm=3 target=3 after=aborted (the grip is the target; nothing more to send)")
    if probe:
        a(10.75, "HOLD", "aim probe anim=29060 prec=0 arm=3 row=43020000 type=55 cat=11 camtype=0 mode=1 ads=0 aim=0 snipe=1.00 n=78/300")
    a(25.5, "HOLD", "stance carrier exit row=43020000 arm=3 target=3 (the carrier restores the original; its grip follows)")
    a(25.5, "HOLD", "xbow restore why=fp ok=1 ms=16")
    a(60.0, "TASK", "end")
    return L


def guard_session(detected=True, aim_patch=False, cam_install=False, chord_in=False, stray_in=False, yaw_bad=False, open_end=False,
                  corr=False, corr_clamped=False, corr_undetected=False, frame_outside=False):
    """C6 part 17: the polearm stance enters (set=polearm), three guard episodes (detected via the weapon frame, one
    clamped -> the gun raise, one late), F5 off. Part 17 addendum: corr = the guard frame on every detected episode (+ its
    `guard frame src=` lines and a learned frame); corr_clamped = every corrected one clamped; corr_undetected = an
    undetected episode with corr=builtin; frame_outside = a `guard frame src=` line outside the guard spans."""
    cfg_line = CFG(er_stance="polearm").replace("ads_pose=er_aim", "ads_pose=guard").replace("aim_cam=keep", "aim_cam=keep carrier=off give=0 axes_xbow=-y,+z,+x load=0")
    L = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", cfg_line)]
    a = lambda t, tag, msg: L.append((t, tag, msg))
    a(10.0, "MODE", "on weapon=m4_mp mag=30/90")
    a(10.1, "HOLD", "stance enter row=2000000 orig=[23,2,0,0,10] arm=1 left=-1 set=polearm new=[36,2,0,3,13]")
    a(10.2, "HOLD", "chord sent route=kb arm=1->3 dt=812ms took=1 row=2000000 kind=enter")
    if aim_patch:
        a(11.0, "HOLD", "aim patch row=2000000 orig=[1,3] new=[11,55] route=kb when=ads (ER's crossbow aim: the type for this ADS)")
    if cam_install:
        a(11.0, "HOLD", "aim cam keep installed getter=0x1403f4a90 sites=3 ret_cam=0x1403b14ea ret_ret=0x140772633 ret_fade=0x14065b3e0")
    det = "1" if detected else "0"
    for k, (t0, ms, align, clamped, fb) in enumerate([(12.0, 2500, "6.5", "0", "0"), (16.0, 1800, "31.0", "1", "0"), (20.0, 3000, "12.0", "0", "1")], 1):
        if corr_clamped:
            clamped = "1"
        a(t0, "HOLD", f"guard on row=2000000 route=kb arm=3 head=13000000 carry=1 (ER's block input held)")
        if corr and detected:
            a(t0 + 0.4, "HOLD", "guard frame src=builtin gun=m4_mp row=2000000 offset=[0.011,-0.002,0.279] rot=[88.150,5.600,9.370] fwd_err=2.1 (..)")
        if chord_in and k == 2:
            a(t0 + 0.5, "HOLD", "chord sent route=kb arm=3->1 dt=300ms took=1 row=2000000 kind=exit")
        if stray_in and k == 1:
            a(t0 + 1.0, "HOLD", "stray_bullet row=52000000 n=1 arm=3")
        if open_end and k == 3:
            break
        a(t0 + ms / 1000.0, "HOLD", f"guard episode n={k} ms={ms} row=2000000 route=kb arm=3->3 head0=13000000 heads=[13000000,13017000,13017010] "
                                   f"guard_anim={'13017000' if detected else 'none'} detected={det} via={'frame' if detected else '-'} det_ms={'200' if detected else '-'} "
                                   f"moved_max={'0.250' if detected else '0.020'} turned_max={'35.0' if detected else '2.0'} align={align if detected else 'none'} "
                                   f"align_min={align if detected else 'none'} clamped={clamped if detected else '0'} fallback={fb if detected else '1'} "
                                   f"fb_ms={'600' if (fb == '1' or not detected) else '-'} yaw=40"
                                   f"{' corr=builtin' if (corr and detected) or (corr_undetected and not detected) else (' corr=off' if corr else '')} why=release")
    if corr and detected:
        a(14.0, "HOLD", "guard frame auto row=2000000 axes=-z,-y,+x rot=[1.200,-0.800,2.100] n=20 spread=1.4 vs_builtin=1.6 (..)")
    if frame_outside:
        a(30.0, "HOLD", "guard frame src=builtin gun=m4_mp row=2000000 offset=[0.011,-0.002,0.279] rot=[88.150,5.600,9.370] fwd_err=2.1 (..)")
    if yaw_bad:
        a(12.1, "HOLD", "guard yaw write ok=0 off_deg=35.0 axis=-z (..)")
    if not open_end:
        a(40.0, "MODE", "off")
        a(40.0, "HOLD", "stance exit row=2000000 restored=1 arm=3 why=exit")
    a(60.0, "TASK", "end")
    return L


# C6 part 17: every one FAILs the guard row
GUARD_MUTATIONS = [
    ("the crossbow aim fields written with the guard", {"aim_patch": True}),
    ("the aim-camera filter installed with the guard", {"cam_install": True}),
    ("a grip chord inside a guard episode", {"chord_in": True}),
    ("an ER bullet during a guard episode", {"stray_in": True}),
    # part 17 addendum: the guard frame only in a detected guard, only inside a guard span
    ("the guard frame in an undetected guard", {"detected": False, "corr_undetected": True}),
    ("the guard frame drawn outside a guard episode", {"corr": True, "frame_outside": True}),
]


def shoulder_session(on_in_fp=False, on_in_peek=False, cfg_off=False, conflicts=0, no_writes=False, unavailable=False):
    """C6 part 18: the shoulder camera on in third person, a pull, cut by first person (F6), back, cut through a scope
    peek, blended out at MW2 off."""
    L = [(0.0, "BOOT", "mw2er 0.3.0")]
    a = lambda t, tag, msg: L.append((t, tag, msg))
    on = "0" if cfg_off else "1"
    a(0.5, "CAM", f"shoulder cfg on={on} off=[0.55,0.25,0.00] ads=[0.45,0.15,-0.60] side=right blend=0.15 margin=0.20 mounted=0 hook=1")
    if unavailable:
        a(0.6, "CAM", "shoulder unavailable reason=no_cam_hook (needs ...)")
    a(10.0, "MODE", "on weapon=m4_mp mag=30/90")
    a(10.0, "CAM", "shoulder on side=right ads=0.00 local=[0.08,0.04,0.00] er_cam=[1.00,2.00,3.00] fwd=[0.00,-0.10,0.99]")
    a(12.0, "CAM", "shoulder pull frac=0.40 want=0.60m hit=0.44m by=map margin=0.20")
    a(12.8, "CAM", "shoulder pull end ms=800 min_frac=0.31")
    w = "0" if no_writes or unavailable else "300"
    w2 = "0" if unavailable else None
    a(15.0, "FP", "on reason=f6")
    a(15.0, "CAM", f"shoulder off why=fp ms=5000 writes={w} skips=0 persisted=0 conflicts={conflicts} pulls=1 min_frac=0.31")
    if on_in_fp:
        a(16.0, "CAM", "shoulder on side=right ads=0.00 local=[0.08,0.04,0.00] er_cam=[1.00,2.00,3.00] fwd=[0.00,-0.10,0.99]")
    a(18.0, "FP", "off reason=f6")
    a(18.0, "CAM", "shoulder on side=right ads=0.00 local=[0.08,0.04,0.00] er_cam=[1.00,2.00,3.00] fwd=[0.00,-0.10,0.99]")
    a(20.0, "FP", "on reason=peek peek=1")
    a(20.0, "CAM", f"shoulder off why=peek ms=2000 writes={w2 or "120"} skips=0 persisted=0 conflicts=0 pulls=0 min_frac=1.00")
    if on_in_peek:
        a(20.5, "CAM", "shoulder on side=right ads=1.00 local=[0.45,0.15,-0.60] er_cam=[1.00,2.00,3.00] fwd=[0.00,-0.10,0.99]")
    a(21.0, "FP", "off reason=peek_release")
    a(21.0, "CAM", "shoulder on side=right ads=0.80 local=[0.47,0.17,-0.48] er_cam=[1.00,2.00,3.00] fwd=[0.00,-0.10,0.99]")
    a(30.0, "MODE", "off")
    a(30.2, "CAM", f"shoulder off why=mode_off ms=9200 writes={w2 or "550"} skips=1 persisted=0 conflicts=0 pulls=0 min_frac=1.00")
    return L


SHOULDER_MUTATIONS = [
    ("the offset on inside first person", {"on_in_fp": True}),
    ("the offset on inside a scope peek", {"on_in_peek": True}),
    ("the offset on with cfg on=0", {"cfg_off": True}),
]


CARRIER_MUTATIONS = [
    ("a bolt used with the carrier on", {"trip": True}),
    ("the swap not verified", {"refused": True}),
    ("the slot's gear changed under the carrier", {"foreign": True}),
    ("a restore that did not take", {"restore0": True}),
    ("the original weapon missing at the restore", {"missing": True}),
    ("the pinned bytes differ", {"prologue": True}),
]


# C6 part 16.16: every one FAILs the load row
LOAD_MUTATIONS = [
    ("ER unloaded our load itself", {"lost": True}),
    ("a bolt used across the load", {"bolt_drop": True}),
    ("an undo that did not write", {"undo0": True}),
]


# every mutation the self-test counts: the session rows, the 2 pair rows, the carrier rows, the load rows
N_MUT = None


def pair_logs(peer_missing=False, hand_missing=False):
    a = session()
    b = [(0.0, "BOOT", "mw2er 0.3.0"), (0.5, "HOLD", CFG()), (9.0, "MODE", "on weapon=famas_mp mag=30/90"), (55.0, "MODE", "off")]
    if not peer_missing:
        b += [(12.5, "HOLD", "peer_stance peer=0x1 row=2000000 patched=1 orig=[23,1,0,3,10]"),
              (38.0, "HOLD", "peer_stance peer=0x1 row=2000000 patched=1 orig=[23,1,0,3,10]"),
              (47.0, "HOLD", "peer_stance peer=0x1 row=2000000 patched=1 orig=[23,1,0,3,10]")]
    if not hand_missing:
        b.append((10.3, "HOLD", "hand ok who=peer=0x1 src=local bone=87 scale=1.000 det=1.000"))
    a.append((10.4, "HOLD", "hand ok who=peer=0x2 src=local bone=87 scale=1.000 det=1.000"))
    return to_entries(a), to_entries(b)


def self_test():
    out, ok, caught = [], True, 0
    global N_MUT
    N_MUT = len(MUTATIONS) + 2 + len(CARRIER_MUTATIONS) + len(LOAD_MUTATIONS) + len(GUARD_MUTATIONS) + len(SHOULDER_MUTATIONS)
    good = {n: (s, d) for n, s, d in rows(to_entries(session()))}
    bad = [f"{n}: {good[n][0]} {good[n][1][:90]}" for n in ROWS if good[n][0] != "PASS" and (n, good[n][0]) not in (("aim arms", "N/A"), ("aim anim", "N/A"), ("carrier", "N/A"), ("load", "N/A"), ("guard", "N/A"), ("shoulder", "N/A"))]
    if bad:
        ok = False
    out.append(f"good C6 session: {sum(1 for n in ROWS if good[n][0] == 'PASS')}/{len(ROWS)} rows PASS (aim arms / carrier N/A by design: no real crossbow, carrier off)" + (f" ({bad})" if bad else ""))
    off = {n: s for n, s, _ in rows(to_entries(session(hold_mode="offset", er_stance="off")))}
    if (off["hand"], off["stance"]) != ("N/A", "N/A"):
        ok = False
        out.append(f"MISSED: offset / stance off must be N/A: {off}")
    pre = {n: s for n, s, _ in rows(to_entries([(0.0, "BOOT", "mw2er 0.2.0"), (1.0, "MODE", "on weapon=m4_mp")]))}
    if any(v != "N/A" for v in pre.values()):
        ok = False
        out.append(f"MISSED: a pre-C6 log must be N/A everywhere: {pre}")
    for label, kw, row in MUTATIONS:
        r = {n: (s, d) for n, s, d in rows(to_entries(session(**kw)))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: {label} -> {row} {r[row][0]} {r[row][1][:90]}")
    # C6 aim 4: chords to a dead player are N/A (the 18:54 replay is no FAIL); a gave-up exit is a WARN, not a FAIL
    dr = {n: (s, d) for n, s, d in rows(to_entries(session(death_refresh=True)))}
    if dr["chord"][0] != "PASS" or "N/A" not in dr["chord"][1]:
        ok = False
        out.append(f"MISSED: a refresh fail during the death must not FAIL the chord row: {dr['chord']}")
    gv = {n: (s, d) for n, s, d in rows(to_entries(session(gave_up=True)))}
    if gv["chord"][0] != "WARN":
        ok = False
        out.append(f"MISSED: an exit_chords gave_up must WARN: {gv['chord']}")
    out.append("chord row: a death's refresh fail N/A, gave_up WARN")
    ld = {n: (s, d) for n, s, d in rows(to_entries(session(aim_late_disable=True)))}
    if ld["aim pose"][0] != "WARN":
        ok = False
        out.append(f"MISSED: an aim disable after good episodes must WARN: {ld['aim pose']}")
    out.append("aim row: a disable after good episodes WARN")
    # C6 round 6: A idle + B ready passes the depth state row; spawn waits never FAIL the chord row; the escape hatch is N/A
    ia = {n: (s, d) for n, s, d in rows(to_entries(session(ds_idle_a=True)))}
    if ia["depth state"][0] != "PASS" or "cand=B" not in ia["depth state"][1]:
        ok = False
        out.append(f"MISSED: A idle + B ready must PASS on B: {ia['depth state']}")
    sw = {n: (s, d) for n, s, d in rows(to_entries(session(spawn_wait=True)))}
    if sw["chord"][0] != "PASS":
        ok = False
        out.append(f"MISSED: a spawn wait must not FAIL the chord row: {sw['chord']}")
    er = [(t, tag, m.replace("aim_cam=keep", "aim_cam=er") if m.startswith("cfg ") else m) for t, tag, m in session(cam_aim=True)]
    ec = {n: (s, d) for n, s, d in rows(to_entries(er))}
    if ec["aim cam"][0] != "N/A":
        ok = False
        out.append(f"MISSED: tp_aim_cam = er must be N/A: {ec['aim cam']}")
    out.append("round 6: A idle + B ready PASS (cand=B), a spawn wait PASS, aim_cam=er N/A")
    # C6 round 7: an older self line (no why=) still judged as before; the pose probe never FAILs; aim fade N/A without snipe=
    old = [(t, tag, m.split(" edges=")[0] + " near_cam=0.1000 near_fit=0.0990 far_cam=5000.0 model=inf lag=0 pass=0" if m.startswith("probe self n=8") else m)
           for t, tag, m in session(ds_self_fail=True)]
    od = {n: (s_, d) for n, s_, d in rows(to_entries(old))}
    nf = [(t, tag, m.replace(" snipe=1.00", "") if m.startswith("aim p") else m) for t, tag, m in session()]
    nd = {n: (s_, d) for n, s_, d in rows(to_entries([x for x in nf if not x[2].startswith("aim pose")]))}
    if od["depth self"][0] != "FAIL" or nd["aim fade"][0] != "N/A" or nd["aim arms"][0] != "N/A" or good["aim arms"][0] != "N/A":
        ok = False
        out.append(f"MISSED: round 7: old self line {od['depth self']}, no snipe {nd['aim fade']}, no pose {nd['aim arms']}, good arms {good['aim arms']}")
    out.append("round 7: an old self line FAIL, no snipe= N/A, no pose line N/A, the good session's arms N/A (part 16.1: the patched sword is no crossbow sample)")
    # C6 part 16.1: probe 0's verdicts (vanilla standing arm=1 PASS with the row; never moved WARN; walking only WARN), the carrier
    av = {n: (s_, d) for n, s_, d in rows(to_entries(arms_session()))}
    aw = {n: (s_, d) for n, s_, d in rows(to_entries(arms_session(moved=False)))}
    ak = {n: (s_, d) for n, s_, d in rows(to_entries(arms_session(walking=True)))}
    ac = {n: (s_, d) for n, s_, d in rows(to_entries(arms_session(kind="carrier")))}
    if av["aim arms"][0] != "PASS" or "row=43020000" not in av["aim arms"][1] or aw["aim arms"][0] != "WARN" or ak["aim arms"][0] != "WARN" \
            or ac["aim arms"][0] != "PASS" or "carrier" not in ac["aim arms"][1]:
        ok = False
        out.append(f"MISSED: aim arms: vanilla {av['aim arms']} still {aw['aim arms']} walking {ak['aim arms']} carrier {ac['aim arms']}")
    out.append("part 16.1: aim arms vanilla PASS (row printed), never moved WARN, walking-only WARN, carrier PASS")
    # C6 part 16: the carrier row (good swap / restore pairs PASS, dry PASS / WARN on pick=none, open at the end WARN,
    # grip fail WARN on the chord row; every mutation FAILs the carrier row)
    cg = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session()))}
    cd = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(mode="dry")))}
    cn = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(mode="dry", pick_none=True)))}
    co = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(open_end=True)))}
    gf = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(grip_fail=True)))}
    if gf["carrier"][0] != "PASS":
        ok = False
        out.append(f"MISSED: a one-handed carry after a grip fail still restores cleanly: {gf['carrier']}")
    if cg["carrier"][0] != "PASS" or cd["carrier"][0] != "PASS" or cn["carrier"][0] != "WARN" or co["carrier"][0] != "WARN" or good["carrier"][0] != "N/A":
        ok = False
        out.append(f"MISSED: carrier: good {cg['carrier']} dry {cd['carrier']} none {cn['carrier']} open {co['carrier']} off {good['carrier']}")
    for label, kw in CARRIER_MUTATIONS:
        r = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(**kw)))}
        if r["carrier"][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: carrier {label} -> {r['carrier']}")
    tr = {n: (s_, d) for n, s_, d in rows(to_entries(carrier_session(trip=True)))}
    if tr["no ER fire"][0] != "FAIL":
        ok = False
        out.append(f"MISSED: a bolt trip must FAIL no ER fire: {tr['no ER fire']}")
    out.append(f"part 16: carrier good PASS, dry PASS, dry pick=none WARN, open at the end WARN, {len(CARRIER_MUTATIONS)} mutations FAIL, a bolt trip FAILs no ER fire")
    # C6 part 16.9: an ignored chord (ER busy) is reported, never a failure
    ci = {n: (s_, d) for n, s_, d in rows(to_entries(session(chord_ignored=True)))}
    if ci["chord"][0] != "PASS" or "ignored:1" not in ci["chord"][1]:
        ok = False
        out.append(f"MISSED: an ignored chord must PASS and be reported: {ci['chord']}")
    out.append("part 16.9: an ignored chord + an er_busy wait PASS (verdicts reported)")
    # C6 part 16 addendum: the aborted chord whose grip reached 3 (`chord done`) PASSes; the 10:00 log (no done line) FAILs
    ad = {n: (s_, d) for n, s_, d in rows(to_entries(abort_session()))}
    ao = {n: (s_, d) for n, s_, d in rows(to_entries(abort_session(done=False)))}
    ap = {n: (s_, d) for n, s_, d in rows(to_entries(abort_session(done=False, probe=True)))}
    if ad["chord"][0] != "PASS" or ao["chord"][0] != "FAIL" or ap["chord"][0] != "PASS" or "closed by the probe" not in ap["chord"][1]:
        ok = False
        out.append(f"MISSED: an aborted chord closed by `chord done` PASS / the old 15 s refusal FAIL: {ad['chord']} {ao['chord']} {ap['chord']}")
    # the aim anim row: keep never + er once PASS (cam=er printed); carrier never WARN; vanilla only N/A; no line N/A
    eh = {n: (s_, d) for n, s_, d in rows(to_entries(episode_session([("1", "1", "keep", 3, "none"), ("1", "1", "er", 3, "46037130"), ("0", "0", "er", 3, "46037100")])))}
    en = {n: (s_, d) for n, s_, d in rows(to_entries(episode_session([("1", "1", "keep", 3, "none"), ("1", "1", "keep", 3, "none")])))}
    ev = {n: (s_, d) for n, s_, d in rows(to_entries(episode_session([("0", "0", "er", 3, "46037130")])))}
    if eh["aim anim"][0] != "PASS" or "cam=er" not in eh["aim anim"][1] or "carrier cam=keep arm=3 loaded=0: 0/1" not in eh["aim anim"][1] \
            or en["aim anim"][0] != "WARN" or ev["aim anim"][0] != "N/A" or "vanilla cam=er arm=3 loaded=0: 1/1" not in ev["aim anim"][1] or good["aim anim"][0] != "N/A":
        ok = False
        out.append(f"MISSED: aim anim: hit {eh['aim anim']} never {en['aim anim']} vanilla {ev['aim anim']} none {good['aim anim']}")
    out.append("part 16 addendum: an aborted chord + `chord done` PASS (no done line FAIL, an old log closed by the probe arm=3 PASS); aim anim er-hit PASS, never WARN, vanilla-only N/A")
    # C6 part 16.16: the load row (good PASS, ER not keeping it WARN, open at the end WARN, waits only WARN, off N/A; the
    # mutations FAIL) and the aim anim row grouped by loaded (a loaded hit PASS with loaded=1; loaded misses WARN + stop)
    lg = {n: (s_, d) for n, s_, d in rows(to_entries(load_session()))}
    lk = {n: (s_, d) for n, s_, d in rows(to_entries(load_session(keep0=True)))}
    lo = {n: (s_, d) for n, s_, d in rows(to_entries(load_session(open_end=True)))}
    lw = {n: (s_, d) for n, s_, d in rows(to_entries(load_session(wait_only=True)))}
    if lg["load"][0] != "PASS" or lk["load"][0] != "WARN" or lo["load"][0] != "WARN" or lw["load"][0] != "WARN" or "settle" not in lw["load"][1] \
            or good["load"][0] != "N/A" or lg["aim anim"][0] != "PASS" or "loaded=1" not in lg["aim anim"][1]:
        ok = False
        out.append(f"MISSED: load: good {lg['load']} keep0 {lk['load']} open {lo['load']} waits {lw['load']} off {good['load']} anim {lg['aim anim']}")
    for label, kw in LOAD_MUTATIONS:
        r = {n: (s_, d) for n, s_, d in rows(to_entries(load_session(**kw)))}
        if r["load"][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: load {label} -> {r['load']}")
    lm = [(t, tag, m.replace("aim=46037130", "aim=none").replace("aim_at_ms=370", "aim_at_ms=-") if m.startswith("aim episode") else m) for t, tag, m in load_session()]
    lmd = {n: (s_, d) for n, s_, d in rows(to_entries(lm))}
    if lmd["aim anim"][0] != "WARN" or "LOADED" not in lmd["aim anim"][1] or "carrier cam=keep arm=3 loaded=1: 0/1" not in lmd["aim anim"][1]:
        ok = False
        out.append(f"MISSED: a loaded miss must WARN with the stop note: {lmd['aim anim']}")
    out.append(f"part 16.16: load good PASS, not kept / open / waits-only WARN, off N/A, {len(LOAD_MUTATIONS)} mutations FAIL; aim anim loaded hit PASS, loaded miss WARN + stop note")
    # C6 part 17: the guard row (good PASS with the align / clamp / fallback reported and the polearm stance PASS; never
    # detected WARN; a refused yaw write WARN; an open last episode still PASS; the er_aim session N/A; the mutations FAIL)
    gg = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session()))}
    gn = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(detected=False)))}
    gy = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(yaw_bad=True)))}
    go = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(open_end=True)))}
    if gg["guard"][0] != "PASS" or "3/3 episode(s) detected" not in gg["guard"][1] or "align 6.5..31.0 deg, clamped 1; fallback 1" not in gg["guard"][1] \
            or "guard_anim 13017000" not in gg["guard"][1] or gg["stance"][0] != "PASS" or gg["aim pose"][0] != "N/A" or gg["aim cam"][0] != "N/A" \
            or gn["guard"][0] != "WARN" or gy["guard"][0] != "WARN" or go["guard"][0] != "PASS" or good["guard"][0] != "N/A":
        ok = False
        out.append(f"MISSED: guard: good {gg['guard']} stance {gg['stance']} never {gn['guard']} yaw {gy['guard']} open {go['guard']} er_aim {good['guard']}")
    for label, kw in GUARD_MUTATIONS:
        r = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(**kw)))}
        if r["guard"][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: guard {label} -> {r['guard']}")
    out.append(f"part 17: guard good PASS (polearm stance PASS, aim pose / aim cam N/A), never detected WARN, yaw refused WARN, open end PASS, er_aim session N/A, {len(GUARD_MUTATIONS)} mutations FAIL")
    # part 17 addendum: the guard frame (corr=) reported with its align / fwd_err / learned frame; all corrected clamped WARN
    gc = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(corr=True)))}
    gk = {n: (s_, d) for n, s_, d in rows(to_entries(guard_session(corr=True, corr_clamped=True)))}
    if gc["guard"][0] != "PASS" or "corr builtin on 3/3: align 6.5..31.0 deg, clamped 1; fwd_err 2.1,2.1,2.1" not in gc["guard"][1] \
            or "learned axes=-z,-y,+x rot=[1.200,-0.800,2.100] vs_builtin=1.6" not in gc["guard"][1] or gk["guard"][0] != "WARN":
        ok = False
        out.append(f"MISSED: guard frame: corr {gc['guard']} all clamped {gk['guard']}")
    out.append("part 17 addendum: guard frame corr PASS (align / fwd_err / learned reported), every corrected guard clamped WARN")
    # C6 part 18: the shoulder row (good PASS with episodes / writes / pulls / off reasons; unavailable / conflicts / no
    # writes WARN; the mutations FAIL)
    sg = {n: (s_, d) for n, s_, d in rows(to_entries(shoulder_session()))}
    sw = [{n: (s_, d) for n, s_, d in rows(to_entries(shoulder_session(**kw)))}["shoulder"][0]
          for kw in ({"unavailable": True}, {"conflicts": 2}, {"no_writes": True})]
    if sg["shoulder"][0] != "PASS" or "3 episode(s), writes 970, pulls 1 (min frac 0.31), off fpx1,mode_offx1,peekx1" not in sg["shoulder"][1] \
            or sw != ["WARN"] * 3 or good["shoulder"][0] != "N/A":
        ok = False
        out.append(f"MISSED: shoulder: good {sg['shoulder']} warns {sw} c6 session {good['shoulder']}")
    for label, kw in SHOULDER_MUTATIONS:
        r = {n: (s_, d) for n, s_, d in rows(to_entries(shoulder_session(**kw)))}
        if r["shoulder"][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: shoulder {label} -> {r['shoulder']}")
    out.append(f"part 18: shoulder good PASS, unavailable / conflicts / no writes WARN, the C6 session N/A, {len(SHOULDER_MUTATIONS)} mutations FAIL")
    # C6 depth 5: the summary verdict line (READY on the good session, NOT READY on a write state, none with the probe off)
    gv_line = depth_verdict(rows(to_entries(session())))
    bad_line = depth_verdict(rows(to_entries(session(ds_write=True))))
    off_line = depth_verdict(rows(to_entries([(0.0, "BOOT", "mw2er 0.2.0")])))
    if gv_line != "C6 DEPTH VERDICT: READY" or not (bad_line or "").startswith("C6 DEPTH VERDICT: NOT READY why=depth_state") or off_line is not None:
        ok = False
        out.append(f"MISSED: the depth verdict line: good {gv_line!r} write {bad_line!r} off {off_line!r}")
    out.append("depth verdict line: READY / NOT READY why=depth_state / none with the probe off")
    # --axes recovers every det -1 permutation from synthetic hand axes (the camera forward along MW2 X, world up Z)
    n_ax = 0
    for perm in itertools.permutations(range(3)):
        for signs in itertools.product((1, -1), repeat=3):
            cols = [[signs[j] if perm[j] == r else 0 for r in range(3)] for j in range(3)]
            if det3(cols) > 0:
                continue
            want = ",".join(("+" if signs[j] > 0 else "-") + "xyz"[perm[j]] for j in range(3))
            # hand axis k in world: MW2 basis j maps to hand axis perm[j] with sign; MW2 X = fwd (1,0,0)... choose world =
            # fwd (0,0,1), left (-1,0,0), up (0,1,0): hand axis perm[j] = signs[j] * world image of MW2 basis j
            world = [[0, 0, 1], [-1, 0, 0], [0, 1, 0]]
            h = [None, None, None]
            for j in range(3):
                h[perm[j]] = [signs[j] * x for x in world[j]]
            txt = "axes hx=[{:.3f},{:.3f},{:.3f}] hy=[{:.3f},{:.3f},{:.3f}] hz=[{:.3f},{:.3f},{:.3f}] cam_fwd=[0.000,0.000,1.000] ads=1".format(*h[0], *h[1], *h[2])
            got = best_axes(axes_samples(to_entries([(1.0, "HOLD", txt)])))
            if got is None or got[0] != want:
                ok = False
                out.append(f"MISSED: --axes for {want} gave {got}")
            n_ax += 1
    out.append(f"--axes recovers {n_ax}/24 det -1 permutations")
    pg = {n: (s, d) for n, s, d in pair_rows(*pair_logs())}
    badp = [f"{n}: {pg[n][0]} {pg[n][1][:80]}" for n in PAIR_ROWS if pg[n][0] != "PASS"]
    if badp:
        ok = False
        out.append(f"MISSED: a good pair must PASS: {badp}")
    for label, kw, row in [("no peer_stance on the friend", {"peer_missing": True}, "peer stance"),
                           ("no hand ok who=peer on the friend", {"hand_missing": True}, "peer hand")]:
        r = {n: (s, d) for n, s, d in pair_rows(*pair_logs(**kw))}
        if r[row][0] == "FAIL":
            caught += 1
        else:
            ok = False
            out.append(f"MISSED: pair {label} -> {row} {r[row][0]}")
    out.append(f"{caught}/{N_MUT} C6 mutations FAIL their row")
    return ok, caught, out


def main(argv):
    if argv == ["--self-test"]:
        ok, caught, lines = self_test()
        for line in lines:
            print("  " + line)
        print(f"{'PASS' if ok else 'FAIL'} analyze_c6 self-test: {caught}/{N_MUT} mutations caught")
        return 0 if ok else 1
    if len(argv) == 2 and argv[0] == "--axes":
        s = axes_samples(last_session(parse(pathlib.Path(argv[1]).read_text(encoding="utf-8", errors="replace"))))
        b = best_axes(s)
        if b is None:
            print("no `HOLD axes` lines (hold_mode = \"hand\", bone_probe = true, hand mode must succeed once)")
            return 1
        print(f"hand_axes = \"{b[0]}\"   # from {len(s)} HOLD axes line(s): |fwd| {b[1]:.2f}, |up| {b[2]:.2f} (1.00 = exact)")
        return 0
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
            print(f"{s:<10} {n:12} {d}")
        passed = all(s in ("PASS", "N/A") for _, s, _ in res)
        print(f"C6 PAIR ROWS: {'PASS' if passed else 'FAIL'} (friend clock shifted by {skew:.0f} s)")
        return 0 if passed else 1
    if len(argv) != 1:
        print(__doc__)
        return 2
    entries = last_session(parse(pathlib.Path(argv[0]).read_text(encoding="utf-8", errors="replace")))
    res = rows(entries)
    for n, s, d in res:
        print(f"{s:<10} {n:12} {d}")
    dv = depth_verdict(res)
    if dv:
        print(dv)
    passed = all(s in OK_STATES for _, s, _ in res)
    print(f"C6 ROWS: {'PASS' if passed else 'FAIL'}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
