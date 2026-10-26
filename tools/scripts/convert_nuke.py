"""Convert the MW2 Tactical Nuke data for milestone M7 into converted/nuke/ (docs/M7_DESIGN.md sections 3.1, 3.6, 6.4).

Usage:
  python -I convert_nuke.py              convert (writes converted/nuke/), then check; exit 0 = PASS
  python -I convert_nuke.py --check      re-convert into work/m7/check_tmp, compare byte for byte with converted/nuke,
                                         run every known answer and the manifest hashes
  python -I convert_nuke.py --self-test  mutations of copied inputs must each be caught

Inputs (read only, the user's own dumps under dumps/m7 and dumps/ac130, made with the external Unlinker):
  common_mp/maps/mp/killstreaks/_nuke.gsc       the timeline NUMBERS only (regex), never copied
  code_post_gfx_mp/mp/killstreaktable.csv       the nuke row's cost
  common_mp/vision/mpnuke(_aftermath).vision    glow + film numbers
  hud/scorebar_hd.menu                          the countdown item (ui_bomb_timer == 4): rect, textscale, colours
  localized_code_post_gfx_mp_localize/.../localized_code_post_gfx_mp.str   the English strings
  localized_common_mp/soundaliases/*.csv, localized_common_mp/sound, iwd/*/Sound, dumps/ac130 (the 105 mm booms)
  hud/dpad_nuke.dds                             A8R8G8B8 384x192 -> dpad_nuke.rgba (RGBA8 + 16-byte header)
Outputs: converted/nuke/nuke.json, sounds/*.wav (copied byte-exact), dpad_nuke.rgba, manifest.json (written last).
No source text of .gsc / .vision / .menu is written anywhere: only numbers and the localized strings.
Pure Python (stdlib), run with -I.
"""
import csv
import hashlib
import io
import json
import pathlib
import re
import shutil
import struct
import sys
import tempfile

VERSION = "convert_nuke.py 1.0"
ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"
OUT = ROOT / "converted" / "nuke"
WORK = ROOT / "work" / "m7"
AC130_SOUNDS = ROOT / "converted" / "ac130" / "sounds"

# (key in nuke.json, alias csv name). Secondary aliases (IW4 secondaryaliasname) are followed and added too.
CUES = [
    ("activate_trigger", "weap_c4detpack_trigger_plr"),
    ("use_us", "US_1mc_use_tnuke"),
    ("use_uk", "UK_1mc_use_tnuke"),
    ("timer", "ui_mp_nukebomb_timer"),
    ("incoming", "nuke_incoming"),
    ("explosion", "nuke_explosion"),
    ("wave", "nuke_wave"),
    ("earned", "mp_killstreak_nuclearstrike"),
    ("achieve_us", "US_1mc_achieve_nuke"),
    ("achieve_uk", "UK_1mc_achieve_nuke"),
]
STRINGS = ["MP_TACTICAL_NUKE", "MP_FRIENDLY_TACTICAL_NUKE", "MP_NUKE_ALREADY_INBOUND", "MP_EARNED_NUKE",
           "KILLSTREAKS_TACTICAL_NUKE", "MP_TACTICAL_NUKE_CALLED"]
ICON_MAGIC = b"MW2RGBA1"


class ConvertError(Exception):
    pass


def default_sources():
    m = DUMPS / "m7"
    return {
        "gsc": m / "common_mp" / "maps" / "mp" / "killstreaks" / "_nuke.gsc",
        "ks_table": m / "code_post_gfx_mp" / "mp" / "killstreaktable.csv",
        "vision": m / "common_mp" / "vision",
        "menu": m / "hud" / "scorebar_hd.menu",
        "localize": m / "localized_code_post_gfx_mp_localize" / "english" / "localizedstrings" / "localized_code_post_gfx_mp.str",
        "aliases": m / "localized_common_mp" / "soundaliases",
        "sound_dirs": [m / "localized_common_mp" / "sound", m / "iwd" / "iw_06.iwd" / "Sound",
                       m / "iwd" / "localized_english_iw02.iwd" / "Sound", DUMPS / "ac130" / "localized_common_mp" / "sound"],
        "icon": m / "hud" / "dpad_nuke.dds",
    }


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def sha256_file(p):
    return sha256_bytes(pathlib.Path(p).read_bytes())


def write_atomic(path, data):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_bytes(data)
    tmp.replace(path)


def jdump(obj):
    return (json.dumps(obj, indent=2, sort_keys=True) + "\n").encode("utf-8")


# ---- the timeline (_nuke.gsc numbers) ---------------------------------------------------------------------------

def gsc_numbers(path):
    t = pathlib.Path(path).read_text(encoding="latin-1")

    def one(pattern, what):
        m = re.search(pattern, t, re.M)
        if not m:
            raise ConvertError(f"_nuke.gsc: {what} not found")
        return m.groups()

    timer = float(one(r'setDvarIfUninitialized\(\s*"scr_nukeTimer"\s*,\s*([\d.]+)\s*\)', "scr_nukeTimer")[0])
    cancel = int(one(r'setDvarIfUninitialized\(\s*"scr_nukeCancelMode"\s*,\s*(\d+)\s*\)', "scr_nukeCancelMode")[0])
    incoming = float(one(r"\(level\.nukeTimer\s*-\s*([\d.]+)\s*\)\s*,\s*::nukeSoundIncoming", "nukeSoundIncoming delay")[0])
    one(r"delaythread_nuke\(\s*level\.nukeTimer\s*,\s*::nukeSoundExplosion", "nukeSoundExplosion at the timer")
    vision = float(one(r"\(level\.nukeTimer\s*\+\s*([\d.]+)\s*\)\s*,\s*::nukeVision", "nukeVision delay")[0])
    death = float(one(r"\(level\.nukeTimer\s*\+\s*([\d.]+)\s*\)\s*,\s*::nukeDeath", "nukeDeath delay")[0])
    quake_delay = float(one(r"\(level\.nukeTimer\s*\+\s*([\d.]+)\s*\)\s*,\s*::nukeEarthquake", "nukeEarthquake delay")[0])
    vblend = float(one(r'visionSetNaked\(\s*"mpnuke"\s*,\s*([\d.]+)\s*\)', "mpnuke blend")[0])
    ablend = float(one(r'visionSetNaked\(\s*"mpnuke_aftermath"\s*,\s*([\d.]+)\s*\)', "mpnuke_aftermath blend")[0])
    beep = float(one(r'playSound\(\s*"ui_mp_nukebomb_timer"\s*\);\s*\n\s*wait\(\s*([\d.]+)\s*\)', "timer beep wait")[0])
    dist = float(one(r"^\s*nukeDistance\s*=\s*([\d.]+)\s*;", "nukeDistance")[0])
    q = re.search(r"^(\s*//)?\s*earthquake\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*\w+\s*,\s*([\d.]+)\s*\)", t, re.M)
    if not q:
        raise ConvertError("_nuke.gsc: the earthquake call not found")
    slowmo = bool(re.search(r"setSlowMotion\(", t))
    return {
        "timer_s": timer, "incoming_lead_s": incoming, "vision_delay_s": vision, "vision_blend_s": vblend,
        "death_delay_s": death, "aftermath_blend_s": ablend, "beep_every_s": beep, "nuke_distance_in": dist,
        "cancel_mode": cancel, "quake_delay_s": quake_delay, "quake_scale": float(q.group(2)), "quake_duration_s": float(q.group(3)),
        "quake_radius_in": float(q.group(4)), "quake_commented": q.group(1) is not None,
    }, slowmo


def ks_cost(path):
    for row in csv.reader(io.StringIO(pathlib.Path(path).read_text(encoding="latin-1"))):
        if len(row) > 4 and row[1] == "nuke":
            return int(row[4])
    raise ConvertError("killstreaktable.csv: no nuke row")


def parse_vision(path):
    t = pathlib.Path(path).read_text(encoding="latin-1")

    def v(key):
        m = re.search(rf'^\s*{key}\s+"([^"]*)"', t, re.M)
        if not m:
            raise ConvertError(f"{pathlib.Path(path).name}: {key} missing")
        return m.group(1)

    tint = lambda k: [float(x) for x in v(k).split()]  # noqa: E731
    out = {"glow_radius": float(v("r_glowRadius0")), "glow_cutoff": float(v("r_glowBloomCutoff")),
           "glow_desaturation": float(v("r_glowBloomDesaturation")), "glow_intensity": float(v("r_glowBloomIntensity0")),
           "film_contrast": float(v("r_filmContrast")), "film_brightness": float(v("r_filmBrightness")),
           "film_desaturation": float(v("r_filmDesaturation")), "film_invert": int(float(v("r_filmInvert"))),
           "film_light_tint": tint("r_filmLightTint"), "film_dark_tint": tint("r_filmDarkTint")}
    if len(out["film_light_tint"]) != 3 or len(out["film_dark_tint"]) != 3:
        raise ConvertError(f"{pathlib.Path(path).name}: tints must have 3 components")
    return out


def parse_localize(path):
    t = pathlib.Path(path).read_bytes().decode("latin-1")
    out, ref = {}, None
    for line in t.splitlines():
        m = re.match(r"^REFERENCE\s+(\S+)", line)
        if m:
            ref = m.group(1)
            continue
        m = re.match(r'^LANG_ENGLISH\s+"(.*)"\s*$', line)
        if m and ref:
            out[ref] = m.group(1)
            ref = None
    return {k: out[k] for k in STRINGS if k in out}


def parse_timer_item(path):
    """The countdown item that shows while `ui_bomb_timer == 4` (the nuke)."""
    t = pathlib.Path(path).read_text(encoding="latin-1")
    for block in re.findall(r"itemDef\s*\{(.*?)\n\s*\}", t, re.S):
        if not re.search(r"dvarint\(ui_bomb_timer\)\s*==\s*4", block):
            continue
        if "secondsascountdown" not in block:
            continue
        m = re.search(r"^\s*rect\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)", block, re.M)
        if not m:
            raise ConvertError("scorebar_hd.menu: the nuke timer item has no rect")
        rect = [float(x) for x in m.groups()]

        def color(key, default):
            c = re.search(rf"^\s*{key}\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)", block, re.M)
            return [float(x) for x in c.groups()] if c else default

        s = re.search(r"^\s*textscale\s+([-\d.]+)", block, re.M)
        # IW4 itemDef default forecolor is opaque white
        return {"timer_rect": rect, "timer_textscale": float(s.group(1)) if s else 0.55,
                "timer_forecolor": color("forecolor", [1.0, 1.0, 1.0, 1.0]), "timer_glowcolor": color("glowcolor", [0.0, 0.0, 0.0, 0.0])}
    raise ConvertError("scorebar_hd.menu: no ui_bomb_timer == 4 countdown item")


# ---- sounds -----------------------------------------------------------------------------------------------------

def wav_info(data):
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ConvertError("not RIFF/WAVE")
    pos, fmt, data_len, data_off = 12, None, None, None
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack_from("<I", data, pos + 4)[0]
        if cid == b"fmt ":
            tag, ch, rate, byte_rate, align, bits = struct.unpack_from("<HHIIHH", data, pos + 8)
            fmt = {"tag": tag, "channels": ch, "rate": rate, "align": align, "bits": bits}
        elif cid == b"data":
            data_len, data_off = size, pos + 8
        pos += 8 + size + (size & 1)
    if not fmt or data_len is None:
        raise ConvertError("missing fmt or data chunk")
    if fmt["tag"] != 1 or fmt["bits"] != 16:
        raise ConvertError(f"not 16-bit PCM (tag {fmt['tag']}, {fmt['bits']} bits)")
    frames = data_len // fmt["align"]
    fmt.update({"frames": frames, "seconds": frames / fmt["rate"], "pcm_sha256": sha256_bytes(data[data_off:data_off + data_len])})
    return fmt


def read_alias(path):
    rows = list(csv.DictReader(io.StringIO(pathlib.Path(path).read_text(encoding="latin-1"))))
    if not rows:
        raise ConvertError(f"{pathlib.Path(path).name}: no alias rows")
    return rows


def find_sound(src, rel):
    for base in src["sound_dirs"]:
        p = base / rel
        if p.is_file():
            return p
    raise ConvertError(f"sound file {rel} not found in the dumps")


def convert_sounds(src, out_dir):
    cues, inputs, infos = {}, {}, {}
    snd = out_dir / "sounds"
    snd.mkdir(parents=True, exist_ok=True)
    todo = list(CUES)
    done = set()
    alias_key = {a: k for k, a in CUES}
    while todo:
        key, alias = todo.pop(0)
        if alias in done:
            continue
        done.add(alias)
        ap = src["aliases"] / f"{alias}.csv"
        rows = read_alias(ap)
        inputs[f"aliases/{alias}.csv"] = ap
        r0 = rows[0]
        files = []
        for r in rows:
            sp = find_sound(src, r["file"])
            name = pathlib.Path(r["file"]).name
            dst = snd / name
            shutil.copyfile(sp, dst)
            inputs[f"sound/{r['file']}"] = sp
            info = wav_info(dst.read_bytes())
            infos[name] = info
            files.append(f"sounds/{name}")
        sec = r0.get("secondaryaliasname", "") or ""
        sec_key = ""
        if sec:
            sec_key = alias_key.get(sec) or sec.replace("nuke_", "").replace("explosion_", "explosion_")
            if sec not in alias_key:
                alias_key[sec] = sec_key
            todo.append((sec_key, sec))
        cues[key] = {
            "alias": alias, "files": files,
            "vol_min": float(r0["vol_min"]), "vol_max": float(r0["vol_max"]),
            "pitch_min": float(r0["pitch_min"]), "pitch_max": float(r0["pitch_max"]),
            "startdelay_ms": int(float(r0["startdelay"] or 0)), "secondary": sec_key,
            "duration_s": round(max(infos[pathlib.Path(f).name]["seconds"] for f in files), 6),
        }
    return cues, inputs, infos


def convert_icon(src, out_dir):
    b = pathlib.Path(src["icon"]).read_bytes()
    if b[:4] != b"DDS ":
        raise ConvertError("dpad_nuke.dds: not a DDS")
    h, w = struct.unpack_from("<II", b, 12)
    flags, = struct.unpack_from("<I", b, 80)
    bits, rm, gm, bm, am = struct.unpack_from("<IIIII", b, 88)
    if not (flags & 0x40) or bits != 32 or (rm, gm, bm, am) != (0xFF0000, 0xFF00, 0xFF, 0xFF000000):
        raise ConvertError(f"dpad_nuke.dds: not A8R8G8B8 (flags {flags:#x} bits {bits})")
    px = b[128:128 + w * h * 4]
    if len(px) != w * h * 4:
        raise ConvertError("dpad_nuke.dds: short pixel data")
    rgba = bytearray(len(px))
    rgba[0::4], rgba[1::4], rgba[2::4], rgba[3::4] = px[2::4], px[1::4], px[0::4], px[3::4]
    out = ICON_MAGIC + struct.pack("<II", w, h) + bytes(rgba)
    write_atomic(out_dir / "dpad_nuke.rgba", out)
    return w, h


def convert(src, out_dir):
    out_dir = pathlib.Path(out_dir)
    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True)
    timeline, slowmo = gsc_numbers(src["gsc"])
    visions = {n: parse_vision(src["vision"] / f"{n}.vision") for n in ("mpnuke", "mpnuke_aftermath")}
    hud = parse_timer_item(src["menu"])
    hud["strings"] = parse_localize(src["localize"])
    w, h = convert_icon(src, out_dir)
    hud["icon_file"] = "dpad_nuke.rgba"
    hud["icon_size"] = [w, h]
    sounds, inputs, infos = convert_sounds(src, out_dir)
    doc = {"schema": 1, "generator": VERSION, "cost": ks_cost(src["ks_table"]), "timeline": timeline,
           "visions": visions, "hud": hud, "sounds": sounds}
    write_atomic(out_dir / "nuke.json", jdump(doc))
    inputs.update({"gsc": src["gsc"], "ks_table": src["ks_table"], "menu": src["menu"], "localize": src["localize"],
                   "icon": src["icon"], **{f"vision/{n}": src["vision"] / f"{n}.vision" for n in visions}})
    files = sorted(p.relative_to(out_dir).as_posix() for p in out_dir.rglob("*") if p.is_file())
    manifest = {"generator": VERSION, "slow_motion_in_mw2": slowmo, "slow_motion_used": False,
                "inputs": {k: sha256_file(v) for k, v in sorted(inputs.items())},
                "files": {f: sha256_file(out_dir / f) for f in files},
                "wav": {k: {x: v[x] for x in ("channels", "rate", "frames", "seconds", "pcm_sha256")} for k, v in sorted(infos.items())}}
    write_atomic(out_dir / "manifest.json", jdump(manifest))
    return doc, manifest


# ---- known answers (docs/M7_DESIGN.md 6.4 + the 3.1 table) -------------------------------------------------------

KNOWN_TIMELINE = {"timer_s": 10.0, "incoming_lead_s": 3.3, "vision_delay_s": 0.25, "vision_blend_s": 3.0, "death_delay_s": 1.5,
                  "aftermath_blend_s": 5.0, "beep_every_s": 1.0, "nuke_distance_in": 5000.0, "cancel_mode": 0,
                  "quake_delay_s": 1.5, "quake_scale": 0.6, "quake_duration_s": 10.0, "quake_radius_in": 100000.0, "quake_commented": True}
KNOWN_VISION = {
    "mpnuke": {"glow_intensity": 4.02646, "glow_cutoff": 0.315552, "glow_radius": 3.13324, "glow_desaturation": 0.23623,
               "film_contrast": 1.67, "film_brightness": 0.0189287, "film_desaturation": 0.48,
               "film_light_tint": [2.0, 1.3053, 0.672409], "film_dark_tint": [0.749927, 0.909935, 1.08943]},
    "mpnuke_aftermath": {"glow_intensity": 0.45, "glow_cutoff": 0.65, "glow_radius": 6.07651, "glow_desaturation": 0.65,
                         "film_contrast": 1.8, "film_brightness": 0.05, "film_desaturation": 0.58,
                         "film_light_tint": [1.0, 0.969, 0.9], "film_dark_tint": [0.7, 0.3, 0.2]},
}
# key -> (seconds of the longest variant, sample rate); +-1 ms
KNOWN_SOUNDS = {"incoming": (4.115, 44100), "timer": (0.997, 44100), "wave": (29.997, 44100), "earned": (1.834, 44100),
                "use_us": (2.594, 22050)}
KNOWN_STRINGS = {"MP_TACTICAL_NUKE": "Tactical Nuke", "MP_FRIENDLY_TACTICAL_NUKE": "Friendly tactical nuke inbound!",
                 "MP_NUKE_ALREADY_INBOUND": "Tactical Nuke already inbound!",
                 "MP_EARNED_NUKE": "Press [{+actionslot 4}] for TACTICAL NUKE."}


def close(a, b, tol=1e-6):
    if isinstance(b, list):
        return isinstance(a, list) and len(a) == len(b) and all(close(x, y, tol) for x, y in zip(a, b))
    if isinstance(b, bool) or isinstance(a, bool):
        return a == b
    return abs(float(a) - float(b)) <= tol * max(1.0, abs(float(b)))


def known_answers(out_dir):
    out_dir = pathlib.Path(out_dir)
    problems = []
    d = json.loads((out_dir / "nuke.json").read_text(encoding="utf-8"))
    man = json.loads((out_dir / "manifest.json").read_text(encoding="utf-8"))
    if d.get("cost") != 25:
        problems.append(f"cost {d.get('cost')} (killstreaktable.csv col 4 = 25)")
    for k, v in KNOWN_TIMELINE.items():
        if not close(d["timeline"].get(k), v):
            problems.append(f"timeline.{k} = {d['timeline'].get(k)} (want {v})")
    for n, kv in KNOWN_VISION.items():
        for k, v in kv.items():
            if not close(d["visions"][n].get(k), v):
                problems.append(f"{n}.{k} = {d['visions'][n].get(k)} (want {v})")
    hud = d["hud"]
    if hud["timer_rect"][:4] != [112.0, 42.0, 100.0, 30.0] or not close(hud["timer_textscale"], 0.3333):
        problems.append(f"timer item rect {hud['timer_rect']} scale {hud['timer_textscale']}")
    if not close(hud["timer_glowcolor"], [1.0, 0.25, 0.0, 1.0]):
        problems.append(f"timer glow {hud['timer_glowcolor']}")
    for k, v in KNOWN_STRINGS.items():
        if hud["strings"].get(k) != v:
            problems.append(f"string {k} = {hud['strings'].get(k)!r}")
    wav = man["wav"]
    snd = d["sounds"]
    for key, (secs, rate) in KNOWN_SOUNDS.items():
        name = pathlib.Path(snd[key]["files"][0]).name
        w = wav[name]
        if abs(w["seconds"] - secs) > 0.001 or w["rate"] != rate:
            problems.append(f"{key} {name}: {w['seconds']:.4f} s {w['rate']} Hz (want {secs} s, {rate} Hz)")
    # the explosion chain: booms -> dist_sub (+100 ms) -> debris -> boom -> boom_local (airlift_nuclear_exp, +150 ms)
    chain, k = [], "explosion"
    while k and len(chain) < 8:
        chain.append(k)
        k = snd[k]["secondary"]
    if len(chain) != 5:
        problems.append(f"explosion chain {chain} (want 5 aliases)")
    else:
        last = snd[chain[-1]]
        lw = wav[pathlib.Path(last["files"][0]).name]
        if not (pathlib.Path(last["files"][0]).name == "airlift_nuclear_exp.wav" and abs(lw["seconds"] - 9.459) <= 0.001
                and last["startdelay_ms"] == 150 and snd[chain[1]]["startdelay_ms"] == 100):
            problems.append(f"explosion chain end {last['files']} {lw['seconds']} delay {last['startdelay_ms']}")
    if snd["wave"]["startdelay_ms"] != 1500 or not close(snd["wave"]["vol_min"], 0.9207):
        problems.append(f"wave delay {snd['wave']['startdelay_ms']} vol {snd['wave']['vol_min']}")
    if not close(snd["timer"]["vol_min"], 0.3255) or not close(snd["incoming"]["vol_min"], 0.723393):
        problems.append("timer / incoming volumes")
    for n in range(1, 6):
        a, b = out_dir / "sounds" / f"exp_105mm_boom{n}.wav", AC130_SOUNDS / f"exp_105mm_boom{n}.wav"
        if not a.is_file() or not b.is_file() or a.read_bytes() != b.read_bytes():
            problems.append(f"exp_105mm_boom{n}.wav differs from the AC-130 copy")
    # every manifest file hash matches the disk; every PCM chunk == the source file's
    for f, h in man["files"].items():
        if sha256_file(out_dir / f) != h:
            problems.append(f"manifest hash of {f}")
    icon = (out_dir / "dpad_nuke.rgba").read_bytes()
    if icon[:8] != ICON_MAGIC or struct.unpack_from("<II", icon, 8) != (384, 192) or len(icon) != 16 + 384 * 192 * 4:
        problems.append("dpad_nuke.rgba header / size")
    if man.get("slow_motion_used") is not False or man.get("slow_motion_in_mw2") is not True:
        problems.append("slow motion flags (MW2 has it, we skip it)")
    return problems


def compare_trees(a, b):
    fa = sorted(p.relative_to(a).as_posix() for p in pathlib.Path(a).rglob("*") if p.is_file())
    fb = sorted(p.relative_to(b).as_posix() for p in pathlib.Path(b).rglob("*") if p.is_file())
    if fa != fb:
        return [f"file lists differ: {sorted(set(fa) ^ set(fb))[:5]}"]
    return [f"{f} differs" for f in fa if (pathlib.Path(a) / f).read_bytes() != (pathlib.Path(b) / f).read_bytes()]


def source_pcm_check(src, out_dir):
    """Every copied WAV's PCM chunk equals the source file's."""
    man = json.loads((pathlib.Path(out_dir) / "manifest.json").read_text(encoding="utf-8"))
    d = json.loads((pathlib.Path(out_dir) / "nuke.json").read_text(encoding="utf-8"))
    problems = []
    for key, s in d["sounds"].items():
        rows = read_alias(src["aliases"] / f"{s['alias']}.csv")
        for r in rows:
            name = pathlib.Path(r["file"]).name
            got = man["wav"].get(name, {}).get("pcm_sha256")
            want = wav_info(find_sound(src, r["file"]).read_bytes())["pcm_sha256"]
            if got != want:
                problems.append(f"{key}: {name} PCM != source")
    return problems


def run_convert():
    try:
        doc, man = convert(default_sources(), OUT)
    except ConvertError as e:
        print(f"FAIL nuke converter: {e}")
        return 1
    p = known_answers(OUT) + source_pcm_check(default_sources(), OUT)
    if p:
        for x in p:
            print("FAIL " + x)
        return 1
    print(f"wrote {OUT}: {len(man['files'])} files, {len(doc['sounds'])} aliases, cost {doc['cost']}, timer {doc['timeline']['timer_s']} s")
    print("PASS nuke converter")
    return 0


def run_check():
    src = default_sources()
    tmp = WORK / "check_tmp"
    problems = []
    try:
        convert(src, tmp)
        problems += compare_trees(tmp, OUT)
        tmp2 = WORK / "check_tmp2"
        convert(src, tmp2)
        det = compare_trees(tmp, tmp2)
        shutil.rmtree(tmp2, ignore_errors=True)
        problems += [f"not deterministic: {x}" for x in det]
        problems += known_answers(OUT)
        problems += source_pcm_check(src, OUT)
    except ConvertError as e:
        problems.append(str(e))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if problems:
        for x in problems:
            print("FAIL " + x)
        print(f"FAIL nuke converter: {len(problems)} problem(s)")
        return 1
    d = json.loads((OUT / "nuke.json").read_text(encoding="utf-8"))
    m = json.loads((OUT / "manifest.json").read_text(encoding="utf-8"))
    print(f"PASS nuke converter: fresh conversion == converted/nuke ({len(m['files'])} files), deterministic, known answers "
          f"(cost {d['cost']}, timer {d['timeline']['timer_s']} s, incoming T-{d['timeline']['incoming_lead_s']} s, mpnuke / aftermath "
          f"visions, countdown item, strings, {len(d['sounds'])} aliases incl. the 5-alias explosion chain, durations / rates, "
          f"PCM == source, booms == the AC-130 copies)")
    return 0


def run_self_test():
    src0 = default_sources()
    base = pathlib.Path(tempfile.mkdtemp(prefix="nuke_selftest_", dir=WORK))
    caught, cases = 0, []
    try:
        def mutated(key, fn, name):
            def run():
                src = dict(src0)
                d = base / name
                d.mkdir()
                if key == "vision":
                    shutil.copytree(src0["vision"], d / "vision", ignore=shutil.ignore_patterns("[!m]*"))
                    src["vision"] = d / "vision"
                    fn(src["vision"])
                elif key == "aliases":
                    shutil.copytree(src0["aliases"], d / "aliases")
                    src["aliases"] = d / "aliases"
                    fn(src["aliases"])
                elif key == "sound_dirs":
                    dd = d / "snd"
                    shutil.copytree(src0["sound_dirs"][0], dd)
                    src["sound_dirs"] = [dd] + src0["sound_dirs"][1:]
                    fn(dd)
                else:
                    p = d / pathlib.Path(src0[key]).name
                    shutil.copyfile(src0[key], p)
                    src[key] = p
                    fn(p)
                out = d / "out"
                try:
                    convert(src, out)
                except ConvertError as e:
                    return [str(e)]
                return known_answers_against(out, src)
            return run

        def sub(path, a, b):
            t = pathlib.Path(path).read_text(encoding="latin-1")
            if a not in t:
                raise ConvertError(f"self-test setup: {a!r} not in {pathlib.Path(path).name}")
            pathlib.Path(path).write_text(t.replace(a, b, 1), encoding="latin-1")

        cases = [
            ("timer 10 -> 12 in _nuke.gsc", mutated("gsc", lambda p: sub(p, '"scr_nukeTimer", 10', '"scr_nukeTimer", 12'), "timer")),
            ("incoming lead 3.3 -> 2.3", mutated("gsc", lambda p: sub(p, "nukeTimer - 3.3", "nukeTimer - 2.3"), "incoming")),
            ("earthquake delay 1.5 -> 2.5", mutated("gsc", lambda p: sub(p, "(level.nukeTimer + 1.5), ::nukeEarthquake", "(level.nukeTimer + 2.5), ::nukeEarthquake"), "quake_delay")),
            ("earthquake uncommented", mutated("gsc", lambda p: sub(p, "//earthquake(", "earthquake("), "quake")),
            ("mpnuke bloom intensity changed", mutated("vision", lambda d: sub(d / "mpnuke.vision", "4.02646", "3.02646"), "vision")),
            ("aftermath dark tint changed", mutated("vision", lambda d: sub(d / "mpnuke_aftermath.vision", '"0.7 0.3 0.2"', '"0.2 0.3 0.7"'), "tint")),
            ("cost 25 -> 15", mutated("ks_table", lambda p: sub(p, "KILLSTREAKS_TACTICAL_NUKE_DESC,25,", "KILLSTREAKS_TACTICAL_NUKE_DESC,15,"), "cost")),
            ("timer beep volume changed", mutated("aliases", lambda d: sub(d / "ui_mp_nukebomb_timer.csv", "0.3255,0.3255", "0.5255,0.5255"), "vol")),
            ("explosion chain broken", mutated("aliases", lambda d: sub(d / "nuke_explosion_dist_sub.csv", "nuke_explosion_debris", ""), "chain")),
            ("incoming PCM changed", mutated("sound_dirs", lambda d: flip_pcm(d / "explosions" / "incoming_mp_nuke1.wav"), "pcm")),
        ]
        for label, run in cases:
            problems = run()
            if problems and not problems[0].startswith("self-test setup"):
                caught += 1
                print(f"caught: {label} -> {problems[0][:120]}")
            else:
                print(f"MISSED: {label}" + (f" ({problems[0]})" if problems else ""))
    finally:
        shutil.rmtree(base, ignore_errors=True)
    if caught == len(cases):
        print(f"PASS self-test: {caught} mutations caught")
        return 0
    print(f"FAIL self-test: {caught}/{len(cases)} mutations caught")
    return 1


def flip_pcm(p):
    b = bytearray(pathlib.Path(p).read_bytes())
    b[len(b) // 2] ^= 0x55
    pathlib.Path(p).write_bytes(bytes(b))


def known_answers_against(out_dir, src):
    """known_answers + the PCM-vs-source check against the ORIGINAL dumps (a changed copy must fail)."""
    p = known_answers(out_dir)
    p += source_pcm_check(default_sources(), out_dir)
    return p


def main(argv):
    if argv == ["--check"]:
        return run_check()
    if argv == ["--self-test"]:
        return run_self_test()
    if argv:
        print(__doc__)
        return 2
    return run_convert()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
