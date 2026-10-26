"""Convert the MW2 AC-130 data for milestone M4 into converted/ac130/ (docs/M4_DESIGN.md section 13).

Usage:
  python -I convert_ac130.py              convert (writes converted/ac130/ + work/m4/), then check; exit 0 = PASS
  python -I convert_ac130.py --check      re-convert into work/m4/check_tmp, compare byte for byte with
                                          converted/ac130, run every known answer and the manifest hashes
  python -I convert_ac130.py --self-test  5 mutations of copied inputs must each be caught
  python -I convert_ac130.py --dump       report whether the Unlinker dumps exist (the dumps were made with the
                                          external GPL Unlinker, tools/oat/build/bin/Release_x64/Unlinker.exe
                                          --game IW4MS; this script never links or copies OAT code)

Inputs (read only, the user's own dumps, all gitignored):
  dumps/common_mp/weapons/ac130_{105,40,25}mm_mp            WEAPONFILE (cross-checked with dumps/ac130/common_mp)
  dumps/ac130/common_mp/maps/mp/killstreaks/_ac130.gsc      read for NUMBERS only (regex), cp1252
  dumps/ac130/common_mp/vision/*.vision                     read for numbers only
  dumps/ac130/common_mp/ui_mp/ac130_hud_hd.menu             rects / aligns / colours
  dumps/ac130/localized_common_mp_localize/.../localized_common_mp.str   the English HUD strings
  dumps/ac130/common_mp/images/ac130_overlay_*.dds          DXT5 512x512 (cross-checked with common_mp_assets)
  dumps/common_mp_assets/model_export/c130_zoomrig_lod0.glb rig geometry (tag_angle x tag_player)
  dumps/ac130/localized_common_mp/{soundaliases,sound}, dumps/ac130/mp_rust/soundaliases, dumps/ac130/iwd/Sound
Outputs: converted/ac130/{weapons_ac130,timing,vision,hud_layout,sounds_ac130,manifest}.json,
  ac130_overlay_{105mm,40mm,25mm,grain}.vmtex (BC3, payload byte-exact), sounds/*.wav (copied byte-exact; the
  ambient MP3 decoded to a 60 s PCM WAV by mod/target/release/sndconv.exe), work/m4/overlay_*.png +
  overlay_stats.json (decoded previews; the Rust thermal::bc3_decode must agree exactly).
No source text of .gsc / .vision / .menu is written anywhere: only the numbers and the localized HUD strings.
Pure Python (stdlib), run with -I.
"""
import csv
import hashlib
import io
import json
import math
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # -I drops the script dir from sys.path
import check_assets  # noqa: E402  (wav_info)
import convert_weapons  # noqa: E402  (parse_weaponfile)
import vm_format  # noqa: E402  (write_tex/read_tex, quaternion math)

VERSION = "convert_ac130.py 1.1"
ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"
OUT = ROOT / "converted" / "ac130"
WORK = ROOT / "work" / "m4"
# Co-op friend package: setup.ps1 points this at the shipped tools\bin\sndconv.exe with MW2ER_SNDCONV.
SNDCONV = pathlib.Path(os.environ["MW2ER_SNDCONV"]) if os.environ.get("MW2ER_SNDCONV") else ROOT / "mod" / "target" / "release" / "sndconv.exe"
GUNS = ["105", "40", "25"]
INCH = 0.0254


def default_sources():
    a = DUMPS / "ac130"
    return {
        "weapons": DUMPS / "common_mp" / "weapons",
        "weapons_alt": a / "common_mp" / "weapons",
        "gsc": a / "common_mp" / "maps" / "mp" / "killstreaks" / "_ac130.gsc",
        "vision": a / "common_mp" / "vision",
        "menu": a / "common_mp" / "ui_mp" / "ac130_hud_hd.menu",
        "localize": a / "localized_common_mp_localize" / "english" / "localizedstrings" / "localized_common_mp.str",
        "images": a / "common_mp" / "images",
        "images_alt": DUMPS / "common_mp_assets" / "images",
        "glb": DUMPS / "common_mp_assets" / "model_export" / "c130_zoomrig_lod0.glb",
        "aliases": a / "localized_common_mp" / "soundaliases",
        "aliases_rust": a / "mp_rust" / "soundaliases",
        "sounds": a / "localized_common_mp" / "sound",
        "iwd_sounds": a / "iwd" / "Sound",
    }


class ConvertError(Exception):
    pass


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def sha256_file(p):
    return sha256_bytes(pathlib.Path(p).read_bytes())


def write_atomic(path, data):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    with os.fdopen(fd, "wb") as f:
        f.write(data)
    os.replace(tmp, path)


def jdump(obj):
    return (json.dumps(obj, indent=2) + "\n").encode("utf-8")


# ---- weapons ----------------------------------------------------------------------------------------------

def read_weapon(path):
    text, _ = convert_weapons.read_text(path)
    return dict(convert_weapons.parse_weaponfile(text))


def num(d, k, name):
    try:
        return float(d[k])
    except (KeyError, ValueError) as e:
        raise ConvertError(f"{name}.{k}: {e}")


def convert_weapons_ac130(src):
    guns, inputs = [], {}
    for g in GUNS:
        name = f"ac130_{g}mm_mp"
        p = src["weapons"] / name
        d = read_weapon(p)
        inputs[f"weapons/{name}"] = p
        alt = src["weapons_alt"] / name
        if alt.is_file() and alt.read_bytes() != p.read_bytes():
            raise ConvertError(f"{name}: dumps/common_mp and dumps/ac130 copies differ")
        sp, r = num(d, "projectileSpeed", name), num(d, "explosionRadius", name)
        guns.append({
            "id": g, "weapon": name,
            "clip_size": int(num(d, "clipSize", name)),
            "fire_time_s": num(d, "fireTime", name),
            "reload_s": num(d, "reloadTime", name),
            "projectile_speed_ips": sp, "projectile_speed_mps": round(sp * INCH, 9),
            "explosion_radius_in": r, "explosion_radius_m": round(r * INCH, 9),
            "explosion_inner": num(d, "explosionInnerDamage", name),
            "explosion_outer": num(d, "explosionOuterDamage", name),
            "damage": num(d, "damage", name),
            "ads_zoom_fov": num(d, "adsZoomFov", name),
            "fire_sound": d.get("fireSoundPlayer", ""),
            "explosion_sound": d.get("projExplosionSound", ""),
            "big_explosion": int(num(d, "bigExplosion", name)),
            "impact_type": d.get("impactType", ""),
            "fire_type": d.get("fireType", ""),
        })
    return {"schema": 1, "generator": VERSION, "guns": guns}, inputs


# ---- gsc numbers (regex only) ----------------------------------------------------------------------------

def gsc_numbers(path):
    t = pathlib.Path(path).read_bytes().decode("cp1252", errors="replace")

    def one(pat, what):
        m = re.search(pat, t)
        if not m:
            raise ConvertError(f"_ac130.gsc: {what} not found")
        return m.groups()

    duration = float(one(r"level\.ac130_use_duration\s*=\s*([\d.]+)\s*;", "ac130_use_duration")[0])
    period = float(one(r'level\.ac130_Speed\[\s*"rotate"\s*\]\s*=\s*([\d.]+)\s*;', "ac130_Speed rotate")[0])
    ramp = float(one(r"rampupDegrees\s*=\s*([\d.]+)\s*;", "rampupDegrees")[0])
    arcs = [float(x) for x in one(r'PlayerLinkWeaponviewToDelta\(\s*level\.ac130\s*,\s*"tag_player"\s*,\s*[\d.]+\s*,'
                                   r'\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*\)', "view arcs")]
    reload = {m.group(1): float(m.group(2)) for m in
              re.finditer(r'level\.weaponReloadTime\[\s*"ac130_(\d+)mm_mp"\s*\]\s*=\s*([\d.]+)\s*;', t)}
    to_black = float(one(r'visionSetThermalForPlayer\(\s*"missilecam"\s*,\s*([\d.]+)\s*\)', "black hot time")[0])

    def body(name):
        """The text of the function DEFINED as `name(...)` at a line start, up to the next definition."""
        m = re.search(rf"^{re.escape(name)}\s*\([^)\n]*\)\s*$", t, re.M)
        if not m:
            raise ConvertError(f"_ac130.gsc: function {name} not found")
        nxt = re.search(r"^\w+\s*\([^)\n]*\)\s*$", t[m.end():], re.M)
        return t[m.end():m.end() + nxt.start()] if nxt else t[m.end():]

    # thermalVision(): the fade-in from black_bw (1 s) and the switch back to white hot (0.51 s)
    tv_sets = [float(m.group(1)) for m in
               re.finditer(r'visionSetThermalForPlayer\(\s*game\["thermal_vision"\]\s*,\s*([\d.]+)\s*\)', body("thermalVision"))]
    if len(tv_sets) < 2:
        raise ConvertError("_ac130.gsc: thermalVision() vision times not found")
    # earthquake(scale, duration, origin, radius): the fire quakes sit on the plane model, the impact quakes on the
    # impact position; radius in inches (1.1: radius + origin kept for the [ac130.shake] mw2_falloff option)
    quake = r"earthquake\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\w.\[\]]+)\s*,\s*([\d.]+)\s*\)"

    def q(m):
        origin = "plane" if "planemodel" in m.group(3).lower() else "impact"
        return {"scale": float(m.group(1)), "duration_s": float(m.group(2)), "radius_in": float(m.group(4)),
                "origin": origin}
    fire_q = {gun: q(m) for gun, m in zip(["105", "40"], re.finditer(quake, body("weaponFiredThread")))}
    imp_q = {gun: q(m) for gun, m in zip(["105", "40"], re.finditer(quake, body("shotFired")))}
    ds = body("shotFiredDarkScreenOverlay")
    dfade = [float(m.group(1)) for m in re.finditer(r"fadeOverTime\(\s*([\d.]+)\s*\)", ds)][:2]
    dwait = re.search(r"wait\s+([\d.]+)\s*;", ds)
    dalpha = [float(m.group(1)) for m in re.finditer(r"darkScreenOverlay\.alpha\s*=\s*([\d.]+)\s*;", ds)]
    if len(dfade) < 2 or not dwait or not dalpha:
        raise ConvertError("_ac130.gsc: dark screen overlay numbers not found")
    return {
        "duration_s": duration, "orbit_period_s": period, "ramp_deg": ramp, "arc_deg": arcs,
        "reload_s": {g: reload.get(g) for g in GUNS},
        "fade_in_s": tv_sets[0], "to_black_hot_s": to_black, "to_white_hot_s": tv_sets[1],
        "fire_quake": fire_q, "impact_quake": imp_q,
        "dark_pulse": {"alpha": max(dalpha), "fade_in_s": dfade[0], "hold_s": float(dwait.group(1)),
                       "fade_out_s": dfade[1]},
    }


# ---- rig geometry from the c130_zoomrig glb -----------------------------------------------------------------

def read_glb_json(path):
    b = pathlib.Path(path).read_bytes()
    if b[:4] != b"glTF":
        raise ConvertError(f"{path}: not a GLB")
    n = struct.unpack_from("<I", b, 12)[0]
    return json.loads(b[20:20 + n])


def rig_geometry(path):
    j = read_glb_json(path)
    nodes = {n.get("name"): n for n in j.get("nodes", [])}
    try:
        ang, ply = nodes["tag_angle"], nodes["tag_player"]
    except KeyError as e:
        raise ConvertError(f"{path}: node {e} missing")
    ma = vm_format.m_trs(ang.get("translation", [0, 0, 0]), ang.get("rotation", [0, 0, 0, 1]))
    mp = vm_format.m_trs(ply.get("translation", [0, 0, 0]), ply.get("rotation", [0, 0, 0, 1]))
    w = vm_format.m_mul(ma, mp)
    p = [w[i][3] for i in range(3)]
    fwd = vm_format.m_dir(w, [1.0, 0.0, 0.0])
    radius = math.hypot(p[0], p[1])
    return {
        "rig_radius_in": round(radius, 3),
        "rig_height_in": round(p[2], 3),
        "tag_pitch_deg": round(math.degrees(math.asin(max(-1.0, min(1.0, -fwd[2])))), 3),
        "center_pitch_deg": round(math.degrees(math.atan2(p[2], radius)), 3),
    }


# ---- visions --------------------------------------------------------------------------------------------------

VISIONS = ["ac130", "ac130_inverted", "thermal_mp", "missilecam", "black_bw"]


def parse_vision(path):
    t = pathlib.Path(path).read_text(encoding="latin-1")

    def v(key):
        m = re.search(rf'^\s*{key}\s+"([^"]*)"', t, re.M)
        if not m:
            raise ConvertError(f"{path.name}: {key} missing")
        return m.group(1)

    tint = lambda k: [float(x) for x in v(k).split()]  # noqa: E731
    out = {"contrast": float(v("r_filmContrast")), "brightness": float(v("r_filmBrightness")),
           "desaturation": float(v("r_filmDesaturation")), "invert": int(float(v("r_filmInvert"))),
           "light_tint": tint("r_filmLightTint"), "dark_tint": tint("r_filmDarkTint")}
    if len(out["light_tint"]) != 3 or len(out["dark_tint"]) != 3:
        raise ConvertError(f"{path.name}: tints must have 3 components")
    return out


# ---- HUD menu + localized strings ------------------------------------------------------------------------------

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
            out[ref] = m.group(1).replace("\\n", "\n")
            ref = None
    return out


def parse_menu(path, strings):
    t = pathlib.Path(path).read_text(encoding="latin-1")
    items = []
    for block in re.findall(r"itemDef\s*\{(.*?)\n\s*\}", t, re.S):
        it = {}
        m = re.search(r'^\s*text\s+"@?([^"]*)"', block, re.M)
        it["ref"] = m.group(1) if m else ""
        m = re.search(r"^\s*rect\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+(\d+)\s+(\d+)", block, re.M)
        if not m:
            raise ConvertError(f"{path.name}: itemDef without a 6-value rect")
        it["rect"] = [float(x) for x in m.groups()]
        for k in ("textalign", "textscale"):
            m = re.search(rf"^\s*{k}\s+([-\d.]+)", block, re.M)
            it[k] = float(m.group(1)) if m else 0.0
        m = re.search(r'^\s*background\s+"([^"]+)"', block, re.M)
        it["background"] = m.group(1) if m else ""
        m = re.search(r"^\s*forecolor\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)", block, re.M)
        it["forecolor"] = [float(x) for x in m.groups()] if m else None
        it["countdown"] = "secondsascountdown" in block
        it["blink"] = re.search(r"sin\(milliseconds\(\)\s*/\s*([\d.]+)\)\)?\s*\)?\s*\*\s*([\d.]+)", block)
        items.append(it)
    keys = {"AC130_HUD_WEAPON_105MM": "label_105", "AC130_HUD_WEAPON_40MM": "label_40",
            "AC130_HUD_WEAPON_25MM": "label_25", "AC130_HUD_TOP_BAR": "top_bar", "AC130_HUD_LEFT_BLOCK": "left_block",
            "AC130_HUD_RIGHT_BLOCK": "right_block", "AC130_HUD_BOTTOM_BLOCK": "bottom_block",
            "AC130_RELOADING": "reloading"}
    out_items, overlay_rect, grain_color, blink = [], None, None, None
    for it in items:
        if it["blink"] and blink is None:
            blink = (float(it["blink"].group(1)), float(it["blink"].group(2)))
        if it["background"].startswith("ac130_overlay_") and it["background"] != "ac130_overlay_grain":
            overlay_rect = overlay_rect or it["rect"]
            continue
        if it["background"] == "ac130_overlay_grain":
            grain_color = it["forecolor"]
            out_items.append({"key": "grain", "text": "", "rect": it["rect"], "textalign": 0, "textscale": 0.0})
            continue
        key = "countdown" if it["countdown"] else keys.get(it["ref"])
        if key is None:
            continue  # platform hint lines (key prompts) are not drawn
        out_items.append({"key": key, "text": strings.get(it["ref"], ""), "rect": it["rect"],
                          "textalign": int(it["textalign"]), "textscale": it["textscale"]})
    if overlay_rect is None or grain_color is None or blink is None:
        raise ConvertError(f"{path.name}: overlay rect / grain forecolor / blink expression not found")
    labels = {g: strings.get(f"AC130_HUD_WEAPON_{g}MM", "") for g in GUNS}
    if not all(labels.values()):
        raise ConvertError("localized gun labels missing")
    return {
        "schema": 1, "items": out_items, "overlay_rect": overlay_rect, "grain_forecolor": grain_color,
        "blink_ms": blink[0], "blink_amp": blink[1], "labels": labels,
        "strings": {"reloading": strings.get("AC130_RELOADING", ""), "whot": strings.get("AC130_HUD_THERMAL_WHOT", ""),
                    "bhot": strings.get("AC130_HUD_THERMAL_BHOT", ""), "agl": strings.get("AC130_HUD_AGL", "")},
    }


# ---- overlay textures ----------------------------------------------------------------------------------------

OVERLAYS = ["ac130_overlay_105mm", "ac130_overlay_40mm", "ac130_overlay_25mm", "ac130_overlay_grain"]


def read_dds_bc3_1mip(path):
    d = pathlib.Path(path).read_bytes()
    if d[:4] != b"DDS " or len(d) < 128:
        raise ConvertError(f"{path.name}: not a DDS file")
    h, w, _pitch, _depth, mips = struct.unpack_from("<IIIII", d, 12)
    if d[84:88] != b"DXT5":
        raise ConvertError(f"{path.name}: FourCC {d[84:88]!r} (want DXT5)")
    bw, bh = (w + 3) // 4, (h + 3) // 4
    size = bw * bh * 16
    if max(1, mips) != 1 or len(d) != 128 + size:
        raise ConvertError(f"{path.name}: {mips} mips / {len(d)} bytes (want 1 mip, {128 + size} bytes)")
    return {"format": vm_format.DXGI_BC3_UNORM, "w": w, "h": h,
            "mips": [{"row_pitch": bw * 16, "rows": bh, "data": d[128:]}]}, d


def bc3_alpha_palette(a0, a1):
    p = [a0, a1, 0, 0, 0, 0, 0, 0]
    if a0 > a1:
        for i in range(1, 7):
            p[i + 1] = ((7 - i) * a0 + i * a1 + 3) // 7
    else:
        for i in range(1, 5):
            p[i + 1] = ((5 - i) * a0 + i * a1 + 2) // 5
        p[6], p[7] = 0, 255
    return p


def expand565(c):
    r, g, b = (c >> 11) & 31, (c >> 5) & 63, c & 31
    return [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]


def bc3_decode(data, w, h):
    """An independent Python BC3 decoder (same integer rules as mw2er_core::thermal::bc3_decode)."""
    out = bytearray(w * h * 4)
    bw = w // 4
    for bi in range(len(data) // 16):
        b = data[bi * 16:bi * 16 + 16]
        bx, by = bi % bw, bi // bw
        ap = bc3_alpha_palette(b[0], b[1])
        abits = int.from_bytes(b[2:8], "little")
        c0, c1 = struct.unpack_from("<HH", b, 8)
        e0, e1 = expand565(c0), expand565(c1)
        cp = [e0, e1, [(2 * x + y + 1) // 3 for x, y in zip(e0, e1)], [(x + 2 * y + 1) // 3 for x, y in zip(e0, e1)]]
        cbits = int.from_bytes(b[12:16], "little")
        for k in range(16):
            px, py = k % 4, k // 4
            o = ((by * 4 + py) * w + bx * 4 + px) * 4
            c = cp[(cbits >> (2 * k)) & 3]
            out[o:o + 3] = bytes(c)
            out[o + 3] = ap[(abits >> (3 * k)) & 7]
    return bytes(out)


def png_bytes(rgba, w, h):
    raw = b"".join(b"\x00" + rgba[y * w * 4:(y + 1) * w * 4] for y in range(h))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def overlay_stats(rgba, w, h):
    n = w * h
    cov = sum(1 for i in range(n) if rgba[i * 4 + 3] > 0)
    means = [sum(rgba[i * 4 + c] for i in range(n)) / n for c in range(4)]
    return {"w": w, "h": h, "alpha_coverage": cov / n, "mean_rgba": [round(m, 4) for m in means],
            "rgba_sha256": sha256_bytes(rgba)}


# ---- sounds -----------------------------------------------------------------------------------------------------

# cue -> (alias folder key, alias name)
CUES = [
    ("fire_105", "aliases", "ac130_105mm_fire"),
    ("reload_105", "aliases", "ac130_105mm_reload"),
    ("fire_40", "aliases", "ac130_40mm_fire"),
    ("reload_40", "aliases", "ac130_40mm_reload"),
    ("fire_25", "aliases", "ac130_25mm_fire"),
    ("exp_105", "aliases", "exp_ac130_105mm"),
    ("exp_40", "aliases", "exp_ac130_40mm"),
    ("switch", "aliases", "ac130_weapon_switch"),
    ("ambient", "aliases_rust", "ambient_ac130_int1"),
    ("exit", "aliases", "veh_ac130_ext_medium_fade"),
    ("announcer", "aliases", "mp_killstreak_ac130"),
]


def read_alias(path):
    rows = list(csv.DictReader(io.StringIO(pathlib.Path(path).read_text(encoding="latin-1"))))
    if not rows:
        raise ConvertError(f"{path.name}: no alias rows")
    return rows


def find_sound(src, rel):
    for base in (src["sounds"], src["iwd_sounds"]):
        p = base / rel
        if p.is_file():
            return p
    raise ConvertError(f"sound file {rel} not found in the dumps")


def convert_sounds(src, out_dir, ambient_wav=None):
    """Copies the WAVs byte for byte; decodes the ambient MP3 with sndconv (or reuses `ambient_wav`)."""
    cues, inputs, files = {}, {}, {}
    snd = out_dir / "sounds"
    snd.mkdir(parents=True, exist_ok=True)
    for cue, folder, alias in CUES:
        ap = src[folder] / f"{alias}.csv"
        rows = read_alias(ap)
        inputs[f"aliases/{alias}.csv"] = ap
        r0 = rows[0]
        rels = []
        for r in rows:
            sp = find_sound(src, r["file"])
            if sp.suffix.lower() == ".mp3":
                name = sp.stem + "_60s.wav"
                dst = snd / name
                if ambient_wav is not None:
                    shutil.copyfile(ambient_wav, dst)
                else:
                    if not SNDCONV.is_file():
                        raise ConvertError(f"{SNDCONV} missing (cargo build --release -p sndconv first)")
                    p = subprocess.run([str(SNDCONV), str(sp), str(dst), "--seconds", "60"], capture_output=True, text=True)
                    if p.returncode != 0:
                        raise ConvertError(f"sndconv: {p.stdout.strip()} {p.stderr.strip()}")
            else:
                name = sp.name
                dst = snd / name
                shutil.copyfile(sp, dst)
            inputs[f"sound/{r['file']}"] = sp
            files[name] = {"source": r["file"], "decoded": sp.suffix.lower() == ".mp3"}
            rels.append(f"sounds/{name}")
        cues[cue] = {
            "alias": alias, "files": rels,
            "vol_min": float(r0["vol_min"]), "vol_max": float(r0["vol_max"]),
            "pitch_min": float(r0["pitch_min"]), "pitch_max": float(r0["pitch_max"]),
            "dist_min_in": float(r0["dist_min"]), "dist_max_in": float(r0["dist_max"]),
            "startdelay_ms": int(float(r0["startdelay"] or 0)),
            "looping": r0["loop"] == "looping",
        }
    # the reload secondaries are the fire aliases' secondaryaliasname (MW2 plays them with their startdelay)
    for gun in ("105", "40"):
        fire = read_alias(src["aliases"] / f"ac130_{gun}mm_fire.csv")[0]
        if fire["secondaryaliasname"] != f"ac130_{gun}mm_reload":
            raise ConvertError(f"ac130_{gun}mm_fire secondary is {fire['secondaryaliasname']!r}")
    return {"schema": 1, "generator": VERSION, "cues": cues}, inputs, files


# ---- conversion ---------------------------------------------------------------------------------------------------

def convert(src, out_dir, work_dir, ambient_wav=None):
    out_dir, work_dir = pathlib.Path(out_dir), pathlib.Path(work_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    work_dir.mkdir(parents=True, exist_ok=True)
    inputs, outputs = {}, {}
    weapons, wi = convert_weapons_ac130(src)
    inputs.update(wi)
    timing = {"schema": 1, "generator": VERSION}
    gn = gsc_numbers(src["gsc"])
    inputs["gsc/_ac130.gsc"] = src["gsc"]
    rig = rig_geometry(src["glb"])
    inputs["glb/c130_zoomrig_lod0.glb"] = src["glb"]
    timing.update({k: gn[k] for k in ("duration_s", "orbit_period_s", "ramp_deg", "arc_deg")})
    timing.update(rig)
    timing.update({k: gn[k] for k in ("reload_s", "fade_in_s", "to_black_hot_s", "to_white_hot_s", "fire_quake",
                                      "impact_quake", "dark_pulse")})
    visions = {}
    for v in VISIONS:
        p = src["vision"] / f"{v}.vision"
        visions[v] = parse_vision(p)
        inputs[f"vision/{v}.vision"] = p
    strings = parse_localize(src["localize"])
    inputs["localize/localized_common_mp.str"] = src["localize"]
    hud = parse_menu(src["menu"], strings)
    inputs["menu/ac130_hud_hd.menu"] = src["menu"]
    stats = {}
    for name in OVERLAYS:
        p = src["images"] / f"{name}.dds"
        dds, raw = read_dds_bc3_1mip(p)
        inputs[f"images/{name}.dds"] = p
        alt = src["images_alt"] / f"{name}.dds"
        if alt.is_file() and alt.read_bytes() != raw:
            raise ConvertError(f"{name}.dds: dumps/ac130 and dumps/common_mp_assets copies differ")
        tex = vm_format.write_tex(dds)
        write_atomic(out_dir / f"{name}.vmtex", tex)
        back = vm_format.read_tex(tex)
        if back["mips"][0]["data"] != raw[128:]:
            raise ConvertError(f"{name}.vmtex payload differs from the DDS payload")
        rgba = bc3_decode(raw[128:], dds["w"], dds["h"])
        write_atomic(work_dir / f"overlay_{name.split('_')[-1]}.png", png_bytes(rgba, dds["w"], dds["h"]))
        st = overlay_stats(rgba, dds["w"], dds["h"])
        st.update({"dds_bytes": len(raw), "dds_sha256": sha256_bytes(raw), "payload_sha256": sha256_bytes(raw[128:])})
        stats[name] = st
    sounds, si, sfiles = convert_sounds(src, out_dir, ambient_wav)
    inputs.update(si)
    for fname, obj in [("weapons_ac130.json", weapons), ("timing.json", timing),
                       ("vision.json", {"schema": 1, "generator": VERSION, "visions": visions}),
                       ("hud_layout.json", hud), ("sounds_ac130.json", sounds)]:
        write_atomic(out_dir / fname, jdump(obj))
    write_atomic(work_dir / "overlay_stats.json", jdump({"schema": 1, "overlays": stats}))
    for p in sorted(out_dir.rglob("*")):
        if p.is_file() and p.name != "manifest.json" and not p.name.endswith(".tmp"):
            outputs[p.relative_to(out_dir).as_posix()] = {"sha256": sha256_file(p), "size": p.stat().st_size}
    manifest = {"schema": 1, "generator": VERSION,
                "inputs": {k: {"sha256": sha256_file(v), "path": str(pathlib.Path(v).relative_to(ROOT)) if str(v).startswith(str(ROOT)) else str(v)}
                           for k, v in sorted(inputs.items())},
                "files": outputs}
    write_atomic(out_dir / "manifest.json", jdump(manifest))  # written last
    return {"weapons": weapons, "timing": timing, "visions": visions, "hud": hud, "sounds": sounds,
            "stats": stats, "sound_files": sfiles}


# ---- known answers -----------------------------------------------------------------------------------------------

KNOWN = {
    "clip_size": [1, 4, 20], "fire_time_s": [0.1, 0.3, 0.07], "reload_s": [5.0, 3.0, 1.5],
    "projectile_speed_ips": [3000.0, 5000.0, 10000.0], "explosion_radius_in": [600.0, 150.0, 32.0],
    "explosion_inner": [1200.0, 500.0, 500.0], "explosion_outer": [300.0, 50.0, 10.0],
    "ads_zoom_fov": [45.0, 20.0, 5.0], "damage": [1000.0, 1000.0, 500.0],
}
KNOWN_WAV = {  # cue: (first file channels, seconds)
    "fire_105": (2, 3.80), "reload_105": (1, 4.40), "fire_40": (2, 1.11), "reload_40": (1, 2.43),
    "fire_25": (2, 0.12), "announcer": (1, 6.55),
}


def check_outputs(out_dir, work_dir, src=None):
    """Every known answer of docs/M4_DESIGN.md section 13 on the files in `out_dir`. Returns problems."""
    out_dir, work_dir = pathlib.Path(out_dir), pathlib.Path(work_dir)
    pr = []
    try:
        w = json.loads((out_dir / "weapons_ac130.json").read_text(encoding="utf-8"))
        t = json.loads((out_dir / "timing.json").read_text(encoding="utf-8"))
        v = json.loads((out_dir / "vision.json").read_text(encoding="utf-8"))["visions"]
        h = json.loads((out_dir / "hud_layout.json").read_text(encoding="utf-8"))
        s = json.loads((out_dir / "sounds_ac130.json").read_text(encoding="utf-8"))["cues"]
        m = json.loads((out_dir / "manifest.json").read_text(encoding="utf-8"))
        st = json.loads((work_dir / "overlay_stats.json").read_text(encoding="utf-8"))["overlays"]
    except (OSError, ValueError, KeyError) as e:
        return [f"outputs unreadable: {e}"]
    guns = w.get("guns", [])
    if [g["id"] for g in guns] != GUNS:
        pr.append(f"weapons_ac130.json guns {[g.get('id') for g in guns]}")
    else:
        for k, want in KNOWN.items():
            got = [g[k] for g in guns]
            if got != want:
                pr.append(f"weapons {k} {got} (want {want})")
        for g in guns:
            if abs(g["projectile_speed_mps"] - g["projectile_speed_ips"] * INCH) > 1e-6 or \
                    abs(g["explosion_radius_m"] - g["explosion_radius_in"] * INCH) > 1e-6:
                pr.append(f"weapons {g['id']}: metres != inches x 0.0254")
        if [g["reload_s"] for g in guns] != [t["reload_s"].get(x) for x in GUNS]:
            pr.append("weapon reloadTime differs from the gsc weaponReloadTime")
    for k, want in (("duration_s", 40.0), ("orbit_period_s", 70.0), ("ramp_deg", 10.0), ("fade_in_s", 1.0),
                    ("to_black_hot_s", 0.62), ("to_white_hot_s", 0.51)):
        if t.get(k) != want:
            pr.append(f"timing {k}={t.get(k)} (want {want})")
    if t.get("arc_deg") != [35.0] * 4:
        pr.append(f"timing arc_deg {t.get('arc_deg')}")
    for k, want, tol in (("rig_radius_in", 4566.0, 1.0), ("rig_height_in", 4212.0, 1.0), ("tag_pitch_deg", 64.5, 0.2),
                         ("center_pitch_deg", 42.7, 0.2)):
        if not abs(float(t.get(k, 1e9)) - want) <= tol:
            pr.append(f"timing {k}={t.get(k)} (want {want} +- {tol})")
    if t.get("fire_quake") != {"105": {"scale": 0.2, "duration_s": 1.0, "radius_in": 1000.0, "origin": "plane"},
                               "40": {"scale": 0.1, "duration_s": 0.5, "radius_in": 1000.0, "origin": "plane"}} or \
            t.get("impact_quake") != {"105": {"scale": 0.4, "duration_s": 1.0, "radius_in": 3500.0, "origin": "impact"},
                                      "40": {"scale": 0.2, "duration_s": 0.5, "radius_in": 2000.0, "origin": "impact"}}:
        pr.append(f"timing earthquakes {t.get('fire_quake')} {t.get('impact_quake')}")
    if t.get("dark_pulse") != {"alpha": 0.6, "fade_in_s": 0.2, "hold_s": 0.4, "fade_out_s": 0.8}:
        pr.append(f"timing dark_pulse {t.get('dark_pulse')}")

    def vis(name, c, b, inv):
        x = v.get(name, {})
        if (x.get("contrast"), x.get("brightness"), x.get("invert")) != (c, b, inv):
            pr.append(f"vision {name} contrast/brightness/invert {x.get('contrast')}/{x.get('brightness')}/{x.get('invert')}")
    vis("ac130", 1.9, 0.5, 0)
    vis("ac130_inverted", 1.55, 0.13, 1)
    vis("thermal_mp", 1.0, 0.0, 0)
    vis("missilecam", 3.7, 1.0, 0)
    vis("black_bw", 0.0, -1.0, 0)
    if v.get("missilecam", {}).get("dark_tint") != [1.21215, 1.21039, 1.31139]:
        pr.append("vision missilecam dark tint")
    if h.get("overlay_rect") != [-320.0, -240.0, 640.0, 480.0, 2.0, 2.0]:
        pr.append(f"hud overlay rect {h.get('overlay_rect')}")
    if h.get("grain_forecolor") != [0.9, 1.0, 1.0, 0.3]:
        pr.append(f"hud grain forecolor {h.get('grain_forecolor')}")
    ys = {it["key"]: it["rect"][1] for it in h.get("items", [])}
    if [ys.get(f"label_{g}") for g in GUNS] != [-30.0, -60.0, -90.0]:
        pr.append(f"hud label y {[ys.get(f'label_{g}') for g in GUNS]}")
    if (h.get("blink_ms"), h.get("blink_amp")) != (90.0, 0.65):
        pr.append(f"hud blink {h.get('blink_ms')} / {h.get('blink_amp')}")
    if h.get("labels") != {"105": "105 mm", "40": "40 mm", "25": "25 mm"}:
        pr.append(f"hud labels {h.get('labels')}")
    for name in OVERLAYS:
        x = st.get(name)
        if not x:
            pr.append(f"overlay stats for {name} missing")
            continue
        if x["dds_bytes"] != 262272 or x["payload_sha256"] == "" or (x["w"], x["h"]) != (512, 512):
            pr.append(f"{name}: dds {x['dds_bytes']} B {x['w']}x{x['h']}")
        try:
            tex = vm_format.read_tex((out_dir / f"{name}.vmtex").read_bytes())
            if tex["format"] != vm_format.DXGI_BC3_UNORM or len(tex["mips"]) != 1 or \
                    sha256_bytes(tex["mips"][0]["data"]) != x["payload_sha256"]:
                pr.append(f"{name}.vmtex: format/mips/payload sha")
        except (OSError, vm_format.FormatError) as e:
            pr.append(f"{name}.vmtex: {e}")
        if src is not None:
            alt = src["images_alt"] / f"{name}.dds"
            own = src["images"] / f"{name}.dds"
            if not alt.is_file() or alt.read_bytes() != own.read_bytes():
                pr.append(f"{name}.dds is not byte-identical in dumps/common_mp_assets")
            elif sha256_file(own) != x["dds_sha256"]:
                pr.append(f"{name}.dds sha differs from overlay_stats.json")
    for cue, (ch, secs) in KNOWN_WAV.items():
        f = out_dir / s.get(cue, {}).get("files", ["?"])[0]
        info = check_assets.wav_info(f) if f.is_file() else {"error": "missing"}
        if "error" in info or info["tag"] != 1 or info["bits"] != 16 or info["rate"] != 44100 or \
                info["channels"] != ch or abs(info["seconds"] - secs) > 0.01:
            pr.append(f"sound {cue}: {info} (want PCM 16-bit 44100 Hz {ch} ch {secs} s)")
    for cue, c in s.items():
        for rel in c["files"]:
            info = check_assets.wav_info(out_dir / rel) if (out_dir / rel).is_file() else {"error": "missing"}
            if "error" in info or info["tag"] != 1 or info["bits"] != 16 or info["rate"] != 44100:
                pr.append(f"sound {cue} {rel}: {info}")
    amb = out_dir / s.get("ambient", {}).get("files", ["?"])[0]
    if amb.is_file():
        info = check_assets.wav_info(amb)
        frames = (amb.stat().st_size - 44) // 4
        data = amb.read_bytes()[44:44 + 4 * 44100]
        peak = max((abs(x) for x in struct.unpack_from(f"<{len(data) // 2}h", data)), default=0)
        if info.get("channels") != 2 or info.get("rate") != 44100 or frames != 2646000 or peak == 0:
            pr.append(f"ambient: {info} frames {frames} peak {peak} (want stereo 44100 Hz 2646000 frames, peak > 0)")
    else:
        pr.append("ambient WAV missing")
    if (s.get("reload_105", {}).get("startdelay_ms"), s.get("reload_40", {}).get("startdelay_ms")) != (2000, 1000):
        pr.append("reload secondary startdelay")
    if len(s.get("exp_105", {}).get("files", [])) != 5 or len(s.get("exp_40", {}).get("files", [])) != 7:
        pr.append("explosion variants")
    for rel, e in m.get("files", {}).items():
        p = out_dir / rel
        if not p.is_file() or sha256_file(p) != e["sha256"]:
            pr.append(f"manifest: {rel} hash differs")
    if src is not None:
        for k, e in m.get("inputs", {}).items():
            p = ROOT / e["path"] if not pathlib.Path(e["path"]).is_absolute() else pathlib.Path(e["path"])
            if not p.is_file() or sha256_file(p) != e["sha256"]:
                pr.append(f"manifest input {k} changed")
    return pr


def summary(res):
    t = res["timing"]
    g = res["weapons"]["guns"]
    return (f"converted 3 guns (clip {[x['clip_size'] for x in g]}, fireTime {[x['fire_time_s'] for x in g]}), "
            f"rig R={t['rig_radius_in']} in H={t['rig_height_in']} in tag pitch {t['tag_pitch_deg']} deg, "
            f"{len(res['visions'])} visions, {len(res['hud']['items'])} HUD items, 4 overlays, "
            f"{sum(len(c['files']) for c in res['sounds']['cues'].values())} sound files in {len(res['sounds']['cues'])} cues")


def run_convert():
    try:
        res = convert(default_sources(), OUT, WORK)
    except (ConvertError, OSError, ValueError) as e:
        print(f"FAIL convert: {e}")
        return 1
    print(summary(res))
    pr = check_outputs(OUT, WORK, default_sources())
    for p in pr:
        print("FAIL " + p)
    if pr:
        return 1
    print("PASS convert_ac130: every known answer holds; manifest written")
    return 0


def run_check():
    src = default_sources()
    tmp = WORK / "check_tmp"
    if tmp.exists():
        shutil.rmtree(tmp)
    try:
        amb = OUT / "sounds" / "amb_ac130_int0v2_lr_60s.wav"
        convert(src, tmp / "out", tmp / "work", ambient_wav=None)
    except (ConvertError, OSError, ValueError) as e:
        print(f"FAIL re-convert: {e}")
        return 1
    pr = []
    for p in sorted((tmp / "out").rglob("*")):
        if not p.is_file() or p.name == "manifest.json":
            continue
        rel = p.relative_to(tmp / "out")
        q = OUT / rel
        if not q.is_file() or q.read_bytes() != p.read_bytes():
            pr.append(f"{rel.as_posix()}: converted/ac130 differs from a fresh conversion")
    if not amb.is_file():
        pr.append("converted/ac130 has no ambient WAV")
    pr += check_outputs(OUT, WORK, src)
    for p in pr:
        print("FAIL " + p)
    if pr:
        return 1
    n = len(json.loads((OUT / "manifest.json").read_text(encoding="utf-8"))["files"])
    print(f"PASS check: converted/ac130 equals a fresh conversion ({n} files, manifest hashes match); weapons "
          "clip 1/4/20 fireTime 0.1/0.3/0.07 reload 5/3/1.5 speed 3000/5000/10000 radius 600/150/32 inner "
          "1200/500/500 outer 300/50/10 fov 45/20/5; rig R 4566 H 4212 tag 64.5 centre 42.7; visions 1.9/0.5, "
          "1.55/0.13 inv, 1/0, 3.7/1, 0/-1; HUD rect/grain/labels/blink; overlays BC3 512x512 byte-exact; "
          "WAVs PCM 44.1 kHz 16-bit with the known channels/durations; ambient 2646000 frames")
    return 0


def run_self_test():
    """Each mutation of a copied input must make the conversion or its known answers fail."""
    base = default_sources()
    tmp = WORK / "selftest"
    if tmp.exists():
        shutil.rmtree(tmp)
    tmp.mkdir(parents=True)
    amb = OUT / "sounds" / "amb_ac130_int0v2_lr_60s.wav"
    if not amb.is_file():
        print("FAIL self-test needs a converted ambient WAV (run the conversion first)")
        return 1

    def attempt(src, tag):
        try:
            convert(src, tmp / tag / "out", tmp / tag / "work", ambient_wav=amb)
        except (ConvertError, OSError, ValueError) as e:
            return [f"convert: {e}"]
        return check_outputs(tmp / tag / "out", tmp / tag / "work", src)

    control = attempt(dict(base), "control")
    if control:
        print("FAIL self-test control (unmutated inputs) does not pass: " + "; ".join(control[:3]))
        return 1

    def copy_tree(key, tag):
        d = tmp / tag / "in" / key
        shutil.copytree(base[key], d)
        return d

    def m_weapon():
        src = dict(base)
        d = copy_tree("weapons_alt", "w")  # unmodified copy for the cross-check
        src["weapons_alt"] = d
        wd = tmp / "w" / "in" / "weapons"
        wd.mkdir(parents=True)
        for g in GUNS:
            shutil.copyfile(base["weapons"] / f"ac130_{g}mm_mp", wd / f"ac130_{g}mm_mp")
        p = wd / "ac130_40mm_mp"
        p.write_bytes(p.read_bytes().replace(b"\\clipSize\\4\\", b"\\clipSize\\5\\"))
        # keep both copies equal so only the known answer can catch it
        shutil.copyfile(p, d / "ac130_40mm_mp")
        src["weapons"] = wd
        return src

    def m_vision():
        src = dict(base)
        d = copy_tree("vision", "v")
        p = d / "ac130.vision"
        p.write_text(p.read_text(encoding="latin-1").replace('"1.9"', '"2.1"'), encoding="latin-1")
        src["vision"] = d
        return src

    def m_dds():
        src = dict(base)
        d = tmp / "d" / "in" / "images"
        d.mkdir(parents=True)
        for n in OVERLAYS:
            shutil.copyfile(base["images"] / f"{n}.dds", d / f"{n}.dds")
        p = d / "ac130_overlay_40mm.dds"
        b = bytearray(p.read_bytes())
        b[128 + 4096] ^= 0xFF
        p.write_bytes(bytes(b))
        src["images"] = d
        return src

    def m_wav():
        src = dict(base)
        d = copy_tree("sounds", "s")
        p = d / "ac130" / "ac130_40mm_fire_c2.wav"
        b = bytearray(p.read_bytes())
        i = b.find(b"fmt ")
        struct.pack_into("<I", b, i + 12, 22050)
        p.write_bytes(bytes(b))
        src["sounds"] = d
        return src

    def m_glb():
        src = dict(base)
        p = tmp / "g" / "c130_zoomrig_lod0.glb"
        p.parent.mkdir(parents=True)
        b = base["glb"].read_bytes()
        n = struct.unpack_from("<I", b, 12)[0]
        j = json.loads(b[20:20 + n])
        for node in j["nodes"]:
            if node.get("name") == "tag_player":
                node["translation"] = [node["translation"][0] * 0.5, node["translation"][1], node["translation"][2]]
        js = json.dumps(j).encode("utf-8")
        js += b" " * ((4 - len(js) % 4) % 4)
        rest = b[20 + n:]
        p.write_bytes(b[:8] + struct.pack("<I", 12 + 8 + len(js) + len(rest)) + struct.pack("<I4s", len(js), b"JSON") + js + rest)
        src["glb"] = p
        return src

    cases = [("weapon clipSize 4 -> 5", m_weapon), ("ac130.vision contrast 1.9 -> 2.1", m_vision),
             ("DDS payload byte flip", m_dds), ("WAV sample rate 44100 -> 22050", m_wav),
             ("glb tag_player node moved", m_glb)]
    caught = 0
    for label, make in cases:
        tag = label.split()[0].lower().replace(".", "_")
        pr = attempt(make(), "case_" + tag)
        if pr:
            caught += 1
            print(f"caught: {label} -> {pr[0][:120]}")
        else:
            print(f"MISSED: {label}")
    shutil.rmtree(tmp, ignore_errors=True)
    if caught == len(cases):
        print(f"PASS self-test: {caught}/{len(cases)} mutations caught (control passed)")
        return 0
    print(f"FAIL self-test: {caught}/{len(cases)} mutations caught")
    return 1


def run_dump():
    src = default_sources()
    missing = [k for k, p in src.items() if not pathlib.Path(p).exists()]
    if not missing:
        print("PASS dumps present; nothing to re-run (the Unlinker is an external tool and is not invoked)")
        return 0
    print(f"FAIL dumps missing: {missing}. Re-run the Unlinker recipe recorded in MODLOG (M4 data) by hand: "
          "tools/oat/build/bin/Release_x64/Unlinker.exe --game IW4MS on common_mp, localized_common_mp, mp_rust, "
          "patch_mp and the iwd files, output under dumps/ac130/")
    return 1


def main(argv):
    if argv == ["--check"]:
        return run_check()
    if argv == ["--self-test"]:
        return run_self_test()
    if argv == ["--dump"]:
        return run_dump()
    if argv:
        print(__doc__)
        return 2
    return run_convert()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
