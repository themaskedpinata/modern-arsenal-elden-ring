"""M6 converter: the MW2 arsenal (34 base MP guns) -> converted/guns/ (docs/M6_DESIGN.md section 3).

Usage (Python 3.14, isolated mode):
  python -I convert_arsenal.py              convert every gun of [arsenal] guns (mw2er.toml; default: all 34) into
                                            converted/guns/<name>/ + the shared _tex/, _snd/, _overlay/ and index.json
  python -I convert_arsenal.py --check      known answers (section 13.1) on the written files + a fresh in-memory
                                            conversion (deterministic: two conversions, same bytes); exit 0 = PASS
  python -I convert_arsenal.py --self-test  deliberate mutations must each be caught by the checks; exit 0 = PASS
  python -I convert_arsenal.py --guns a,b   restrict to some guns (convert or check)

Per gun (generalising the M3 UMP45-only pipeline, convert_viewmodel.py, which stays for the M3 oracles):
  gun.vmskel / gun.vmmesh / gun.vmanim (same formats as converted/viewmodel/), clips.json (roles, notetracks, the
  weapon time field each timed clip is played over and its rate), stats.json (the weapon-file numbers the runtime
  uses), sounds.json (sound aliases -> deduplicated WAV copies in _snd/, resolved MP zone first), manifest.json
  (sha256 of every output file and input, written last). Textures are deduplicated by content into _tex/; the sniper
  scope overlay goes to _overlay/. A gun whose conversion has problems is written as ok=false in index.json and gets
  no manifest (the DLL skips it); the other guns are unaffected.

Inputs are the user's own MW2 dumps (gitignored); outputs only under converted/guns/ and work/m6/. No MW2 data is
stored in this file: the tables below are internal names, stable indices and the user-facing gun names. No OAT /
IW4L code is used; the XAnim layout comes from xanim_read.py.
"""
from __future__ import annotations

import copy
import hashlib
import json
import math
import os
import pathlib
import shutil
import struct
import sys
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_assets  # noqa: E402
import convert_viewmodel as cv  # noqa: E402
import convert_weapons  # noqa: E402
import inventory_m6 as inv  # noqa: E402
import vm_format as vf  # noqa: E402
import xanim_read  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"
ASSETS = DUMPS / "common_mp_assets"
OUT = ROOT / "converted" / "guns"
TEX_DIR = OUT / "_tex"
SND_DIR = OUT / "_snd"
OVERLAY_DIR = OUT / "_overlay"
REPORT = ROOT / "work" / "m6" / "convert_report.json"
CONFIG = ROOT / "mw2er.toml"
CONVERTER_VERSION = "convert_arsenal 1.1"
SCHEMA = 1

# Stable gun index g (docs/M6_DESIGN.md 9.1: append-only, never reordered; it is part of the regulation row ids).
GUN_INDEX = {
    "m4_mp": 1, "famas_mp": 2, "scar_mp": 3, "tavor_mp": 4, "fal_mp": 5, "m16_mp": 6, "masada_mp": 7, "fn2000_mp": 8,
    "ak47_mp": 9,
    "mp5k_mp": 11, "ump45_mp": 12, "kriss_mp": 13, "p90_mp": 14, "uzi_mp": 15,
    "sa80_mp": 21, "rpd_mp": 22, "mg4_mp": 23, "aug_mp": 24, "m240_mp": 25,
    "cheytac_mp": 31, "barrett_mp": 32, "wa2000_mp": 33, "m21_mp": 34,
    "spas12_mp": 41, "aa12_mp": 42, "striker_mp": 43, "ranger_mp": 44, "m1014_mp": 45, "model1887_mp": 46,
    "at4_mp": 51, "m79_mp": 52, "rpg_mp": 53, "stinger_mp": 54, "javelin_mp": 55,
}
DEFAULT_ORDER = list(GUN_INDEX)
# The names the user asked for (the HUD shows these; not MW2 strings).
USER_NAMES = {
    "m4_mp": "M4A1", "famas_mp": "FAMAS", "scar_mp": "SCAR-H", "tavor_mp": "TAR-21", "fal_mp": "FAL", "m16_mp": "M16A4",
    "masada_mp": "ACR", "fn2000_mp": "F2000", "ak47_mp": "AK-47", "mp5k_mp": "MP5K", "ump45_mp": "UMP45",
    "kriss_mp": "Vector", "p90_mp": "P90", "uzi_mp": "Mini-Uzi", "sa80_mp": "L86 LSW", "rpd_mp": "RPD", "mg4_mp": "MG4",
    "aug_mp": "AUG HBAR", "m240_mp": "M240", "cheytac_mp": "Intervention", "barrett_mp": "Barrett .50cal",
    "wa2000_mp": "WA2000", "m21_mp": "M21 EBR", "spas12_mp": "SPAS-12", "aa12_mp": "AA-12", "striker_mp": "Striker",
    "ranger_mp": "Ranger", "m1014_mp": "M1014", "model1887_mp": "Model 1887", "at4_mp": "AT4-HS", "m79_mp": "Thumper",
    "rpg_mp": "RPG-7", "stinger_mp": "Stinger", "javelin_mp": "Javelin",
}
CLASS_OF = {n: c for c, ns in inv.GUNS.items() for n in ns}
# Per-gun anim bones that are absent from the model and dropped with a note (rpd: the belt's last links).
EXTRA_WHITELIST = {"rpd_mp": {"j_bullet17", "j_bullet18", "j_bullet19", "j_bullet20"}}
# Clip roles -> weapon-file anim field. The first 12 are the M3 base clips (required); the rest are optional.
BASE_ROLES = {"idle": "idleAnim", "fire": "fireAnim", "fire_ads": "adsFireAnim", "reload": "reloadAnim",
              "reload_empty": "reloadEmptyAnim", "pullout": "raiseAnim", "putaway": "dropAnim",
              "sprint_in": "sprintInAnim", "sprint_loop": "sprintLoopAnim", "sprint_out": "sprintOutAnim",
              "ads_up": "adsUpAnim", "ads_down": "adsDownAnim"}
OPT_ROLES = {"last_shot": "lastShotAnim", "rechamber": "rechamberAnim", "ads_rechamber": "adsRechamberAnim",
             "reload_start": "reloadStartAnim", "reload_end": "reloadEndAnim", "empty_idle": "emptyIdleAnim",
             "first_raise": "firstRaiseAnim"}
# role -> the weapon time field the clip is played over (rate = native duration / time; 0 or missing = rate 1)
ROLE_TIME = {"fire": "fireTime", "fire_ads": "fireTime", "last_shot": "fireTime", "reload": "reloadTime",
             "reload_empty": "reloadEmptyTime", "pullout": "raiseTime", "putaway": "dropTime",
             "first_raise": "firstRaiseTime", "sprint_in": "sprintInTime", "sprint_loop": "sprintLoopTime",
             "sprint_out": "sprintOutTime", "ads_up": "adsTransInTime", "ads_down": "adsTransOutTime",
             "rechamber": "rechamberTime", "ads_rechamber": "rechamberTime", "reload_start": "reloadStartTime",
             "reload_end": "reloadEndTime"}
# Sound roles -> weapon-file sound field.
SOUND_ROLES = {"fire": "fireSoundPlayer", "last_shot": "lastShotSoundPlayer", "empty": "emptyFireSoundPlayer",
               "raise": "raiseSoundPlayer", "first_raise": "firstRaiseSoundPlayer", "putaway": "putawaySoundPlayer",
               "rechamber": "rechamberSoundPlayer", "reload": "reloadSoundPlayer",
               "reload_empty": "reloadEmptySoundPlayer", "reload_start": "reloadStartSoundPlayer",
               "reload_end": "reloadEndSoundPlayer", "projectile": "projectileSound",
               "proj_ignition": "projIgnitionSound", "proj_explosion": "projExplosionSound",
               # C3 PASS C (docs/C3_DESIGN.md 3.5): the third-person fire sound peers hear (its secondary is the
               # distant layer, e.g. weap_medium_fardist_4)
               "fire_npc": "fireSound"}
# C3 PASS C (docs/C3_DESIGN.md 3.6): the world model (weapon file `worldModel`) every player is drawn holding.
WORLD_LODS = (0, 1)
# Base roles that may be empty in the weapon file and then share another role's clip (docs/M6_DESIGN.md 5.4).
FALLBACK_ROLES = {"reload_empty": "reload"}
RUMBLE_NOTES = ("viewmodel_small", "viewmodel_medium", "viewmodel_large")
STAT_STRINGS = ["fireType", "weaponClass", "weaponType", "inventoryType", "gunModel", "handModel", "rocketModel",
                "adsOverlayShader", "guidedMissileType", "projExplosionType", "penetrateType", "stickiness"]
PART_GUN, PART_HANDS, PART_ROCKET = 0, 1, 2
ROCKET_PREFIX = "rkt_"
# MW2 MP _weapons.gsc updateMoveSpeedScale by weaponClass (docs/M6_DESIGN.md section 7): the class names only.
MOVE_CLASS = {"rifle": "rifle", "mg": "mg", "smg": "smg", "spread": "spread", "rocketlauncher": "launcher",
              "sniper": "sniper", "grenade": "other", "pistol": "other"}
OPTIONAL_TAGS = {"tag_sight_on", "tag_flash", "j_wrist_ri", "j_wrist_le"}
# review M6 fidelity: the guns whose upper-torso multiplier is > 1 (the regulation's body-hit damage, M6_DESIGN dev. 15)
KA_BODY_MULT = {"cheytac_mp": 1.5, "barrett_mp": 1.5, "wa2000_mp": 1.5, "m21_mp": 1.1}
NO_TAG = 0xFFFF

# Known answers (section 13.1) checked by --check: facts from the dumps, recorded once (no data copied).
KA_SIGHT_ON = {"m4_mp", "famas_mp", "scar_mp", "tavor_mp", "fal_mp", "m16_mp", "mp5k_mp", "ump45_mp", "kriss_mp",
               "mg4_mp", "aug_mp", "m240_mp", "spas12_mp", "aa12_mp", "striker_mp", "m1014_mp"}
KA_FIRE_ALIAS = {"spas12_mp": "weap_ranger_fire_plr", "model1887_mp": "weap_winch1200_fire_plr",
                 "m21_mp": "weap_m14sniper_fire_plr", "rpd_mp": "weap_degtyarev_rpd_fire_plr",
                 "p90_mp": "weap_fnp90_fire_plr", "striker_mp": "weap_stryker_fire_plr"}
KA_RAW_PAIRS = {("spas12_mp", "rechamber"): (0.9333, 0.467), ("m240_mp", "reload"): (9.7, 7.75),
                ("p90_mp", "reload"): (3.1, 2.76), ("m79_mp", "reload"): (3.1, 2.6),
                ("javelin_mp", "reload"): (4.5, 3.0), ("ranger_mp", "reload"): (3.1667, 2.53)}
KA_NOTES = {("ump45_mp", "reload"): [("weap_ump45_clipout_plr", 15, 75), ("weap_ump45_clipin_plr", 54, 75)],
            ("cheytac_mp", "rechamber"): [("weap_cheytac_bolt_unlock_plr", 3, 28), ("weap_cheytac_bolt_open_plr", 7, 28),
                                          ("weap_cheytac_bolt_close_plr", 16, 28), ("weap_cheytac_bolt_lock_plr", 19, 28)],
            ("spas12_mp", "reload"): [("weap_spas12_loop_plr", 10, 26)]}
KA_SEGMENTED = {"spas12_mp", "striker_mp", "m1014_mp", "model1887_mp"}
KA_BOLT = {"cheytac_mp", "spas12_mp", "model1887_mp"}
M3_UMP45_FIRE = ROOT / "converted" / "sounds" / "ump45" / "weap_ump45_slst_10b.wav"
TOL_TICK = 1.0 / 60.0


class ConvError(Exception):
    pass


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def jdump(o):
    return (json.dumps(o, indent=1, sort_keys=True) + "\n").encode("utf-8")


def gun_list(argv_guns=None):
    if argv_guns:
        return argv_guns
    try:
        cfg = tomllib.loads(CONFIG.read_text(encoding="utf-8"))
        guns = cfg.get("arsenal", {}).get("guns")
        if guns:
            return list(guns)
    except (OSError, ValueError):
        pass
    return list(DEFAULT_ORDER)


# ------------------------------------------------------------------------------------------------ inputs
_XANIM = None
_GLB = None
_MATS = None
_IMAGES = None


def ci(folder):
    return {p.name.lower(): p for p in folder.iterdir()} if folder.is_dir() else {}


def indexes():
    global _XANIM, _GLB, _MATS, _IMAGES
    if _XANIM is None:
        _XANIM = ci(ASSETS / "xanim")
        _GLB = ci(ASSETS / "model_export")
        _MATS = {}
        for sub in ("mc", "gfx", "wc", ""):
            for k, v in ci(ASSETS / "materials" / sub).items():
                _MATS.setdefault(k, v)
        _IMAGES = ci(ASSETS / "images")
    return _XANIM, _GLB, _MATS, _IMAGES


def weapon_kv(name):
    text, _ = convert_weapons.read_text(DUMPS / "common_mp" / "weapons" / name)
    return dict(convert_weapons.parse_weaponfile(text)), DUMPS / "common_mp" / "weapons" / name


def num(v):
    try:
        x = float(v)
    except (TypeError, ValueError):
        return None
    return int(x) if x.is_integer() else x


def read_dds_any(path, allow_partial_mips=False):
    """DXT1/DXT3/DXT5 DDS -> {format (DXGI 71/74/77), w, h, mips[]}. Colour maps need a complete mip chain (the M3
    rule); the scope overlay has one mip (allow_partial_mips)."""
    d = pathlib.Path(path).read_bytes()
    if d[:4] != b"DDS " or len(d) < 128:
        raise vf.FormatError(f"{path}: not a DDS file")
    h, w, _pitch, _depth, mips = struct.unpack_from("<IIIII", d, 12)
    fcc = d[84:88]
    fmts = {b"DXT1": (71, 8), b"DXT3": (74, 16), b"DXT5": (77, 16)}
    if fcc not in fmts:
        raise vf.FormatError(f"{path}: unsupported FourCC {fcc!r}")
    fmt, bs = fmts[fcc]
    mips = max(1, mips)
    if w & (w - 1) or h & (h - 1) or w < 4 or h < 4:
        raise vf.FormatError(f"{path}: {w}x{h} is not a power of two >= 4")
    full = int(math.log2(max(w, h))) + 1
    if mips != full and not allow_partial_mips:
        raise vf.FormatError(f"{path}: {mips} mips, a complete chain has {full}")
    levels, o = [], 128
    for m in range(mips):
        mw, mh = max(1, w >> m), max(1, h >> m)
        bw, bh = (mw + 3) // 4, (mh + 3) // 4
        size = bw * bh * bs
        if o + size > len(d):
            raise vf.FormatError(f"{path}: truncated at mip {m}")
        levels.append({"row_pitch": bw * bs, "rows": bh, "data": d[o:o + size]})
        o += size
    if o != len(d):
        raise vf.FormatError(f"{path}: {len(d) - o} trailing bytes after the mip chain")
    return {"format": fmt, "w": w, "h": h, "mips": levels}


def load_material(name):
    _, _, mats, _ = indexes()
    p = mats.get(name.lower() + ".json")
    if not p:
        return None
    mj = json.loads(p.read_text(encoding="utf-8"))
    cm = [t["image"] for t in mj.get("textures", []) if t.get("name") == "colorMap"]
    return {"name": name, "color_map": cm[0] if cm else None, "json": p,
            "blend": 1 if mj.get("cameraRegion") == "litTrans" else 0, "region": mj.get("cameraRegion")}


# ------------------------------------------------------------------------------------------------ skeleton
def build_skeleton(hands, gun, rocket=None):
    """Hands skin joints, then gun skin joints (j_gun under tag_weapon), then the rocket part's joints (rpg: renamed
    rkt_*, root under the gun's tag_clip). The OAT root rotation is asserted and dropped (as in M3)."""
    bones, problems = [], []
    parts = [("hands", hands, ""), ("gun", gun, "")]
    if rocket is not None:
        parts.append(("rocket", rocket, ROCKET_PREFIX))
    for part, g, prefix in parts:
        sk = g.json["skins"][0]
        joints = sk["joints"]
        jset = set(joints)
        base = len(bones)
        for ni in joints:
            nd = g.nodes[ni]
            p = g.parent.get(ni)
            parent = base + joints.index(p) if (p is not None and p in jset) else -1
            t = list(nd.get("translation", [0.0, 0.0, 0.0]))
            q = list(nd.get("rotation", [0.0, 0.0, 0.0, 1.0]))
            if parent < 0:
                if max(abs(a - b) for a, b in zip(q, [-0.7071068, 0.0, 0.0, 0.7071068])) > 1e-5:
                    problems.append(f"{part} root {nd['name']}: expected the OAT root quat, got {q}")
                q = [0.0, 0.0, 0.0, 1.0]
            bones.append({"name": prefix + nd["name"], "parent": parent, "t": t, "q": q, "part": part})
    names = [b["name"] for b in bones]
    if len(set(names)) != len(names):
        problems.append("duplicate bone names in the combined skeleton")
    roots = {part: next((i for i, b in enumerate(bones) if b["part"] == part and b["parent"] < 0), None)
             for part in ("gun", "rocket")}
    gr = roots["gun"]
    if gr is None or bones[gr]["name"] != "j_gun":
        problems.append(f"gun root is {bones[gr]['name'] if gr is not None else None}, expected j_gun")
        return None, None, problems
    if "tag_weapon" not in names:
        problems.append("tag_weapon missing from the hands")
        return None, None, problems
    bones[gr]["parent"] = names.index("tag_weapon")
    bones[gr]["t"] = [0.0, 0.0, 0.0]
    if rocket is not None:
        rr = roots["rocket"]
        if rr is None or "tag_clip" not in names:
            problems.append("rocket part: no root or the gun has no tag_clip")
        else:
            bones[rr]["parent"] = names.index("tag_clip")
            bones[rr]["t"] = [0.0, 0.0, 0.0]
    for i, b in enumerate(bones):
        if b["parent"] >= i:
            problems.append(f"bone {b['name']} appears before its parent")
    tags = {}
    for t in vf.SKEL_TAGS:
        if t in names:
            tags[t] = names.index(t)
        elif t in OPTIONAL_TAGS:
            tags[t] = NO_TAG
        else:
            problems.append(f"required tag {t} missing")
    if len(bones) > 256:
        problems.append(f"{len(bones)} bones > 256")
    if problems:
        return None, None, problems
    skel = {"bones": bones, "tags": tags}
    world = vf.fk(skel, [(b["t"], b["q"]) for b in bones])
    for i, b in enumerate(bones):
        b["ibm"] = vf.m_inv_rigid(world[i])
    return skel, world, problems


# ------------------------------------------------------------------------------------------------ mesh
def build_mesh(skel, bind_world, hands, gun, hide_tags, rocket=None):
    names = [b["name"] for b in skel["bones"]]
    verts, indices, surfaces, materials, textures, problems, notes = [], [], [], [], [], [], []
    stats = {}
    mat_index, tex_index = {}, {}
    parts = [(PART_HANDS, "hands", hands, 0, ""), (PART_GUN, "gun", gun, names.index("j_gun"), "")]
    if rocket is not None:
        parts.append((PART_ROCKET, "rocket", rocket, names.index(ROCKET_PREFIX + "tag_clip"), ROCKET_PREFIX))
    for part_id, part, g, base_joint, prefix in parts:
        xf = bind_world[base_joint] if part != "hands" else vf.m_identity()
        sk = g.json["skins"][0]
        joints = sk["joints"]
        # joint k of this GLB -> bone index in the combined skeleton
        jmap = [names.index(prefix + g.nodes[ni]["name"]) for ni in joints]
        hidden_nodes = cv._descendants(g, set(hide_tags)) if part == "gun" else set()
        hidden_joints = {k for k, ni in enumerate(joints) if ni in hidden_nodes}
        cache = {}
        drawn = hidden = 0
        for si, mesh in enumerate(g.json["meshes"]):
            for pr in mesh["primitives"]:
                at = pr["attributes"]
                key = (at["POSITION"], at["NORMAL"], at["TEXCOORD_0"], at["JOINTS_0"], at["WEIGHTS_0"], at.get("COLOR_0"))
                if key not in cache:
                    cache[key] = {
                        "pos": g.accessor(at["POSITION"]), "nrm": g.accessor(at["NORMAL"]),
                        "uv": g.accessor(at["TEXCOORD_0"]), "joints": g.accessor(at["JOINTS_0"]),
                        "weights": g.accessor(at["WEIGHTS_0"]),
                        "color": g.accessor(at["COLOR_0"]) if at.get("COLOR_0") is not None else None,
                        "remap": {}}
                src = cache[key]
                idx = g.accessor(pr["indices"])
                used = sorted(set(idx))
                only_hidden = all(all((w <= 0.0) or (j in hidden_joints) for j, w in zip(src["joints"][v], src["weights"][v]))
                                  for v in used)
                if part == "gun" and only_hidden:
                    hidden += 1
                    continue
                mname = g.json["materials"][pr["material"]]["name"].split("/")[-1]
                if mname not in mat_index:
                    m = load_material(mname)
                    if m is None:
                        problems.append(f"material {mname} json missing")
                        continue
                    if not m["color_map"]:
                        notes.append(f"material {mname} has no colorMap: surface skipped")
                        continue
                    if m["color_map"] not in tex_index:
                        tex_index[m["color_map"]] = len(textures)
                        textures.append(m["color_map"])
                    mat_index[mname] = len(materials)
                    materials.append({"name": mname, "texture": tex_index[m["color_map"]], "blend": m["blend"],
                                      "json": m["json"]})
                drawn += len(idx) // 3
                first = len(indices)
                for v in idx:
                    if v not in src["remap"]:
                        src["remap"][v] = len(verts)
                        verts.append(_vertex(src, v, jmap, base_joint, part_id, xf))
                    indices.append(src["remap"][v])
                surfaces.append({"first": first, "count": len(idx), "material": mat_index[mname], "part": part_id,
                                 "glb_surface": si})
        stats[f"{part}_tris"] = drawn
        stats[f"{part}_hidden_surfaces"] = hidden
    stats["vertices"] = len(verts)
    if len(verts) >= 65536:
        problems.append(f"{len(verts)} vertices do not fit u16 indices")
    if not any(s["part"] == PART_GUN for s in surfaces):
        problems.append("no gun surface drawn")
    return {"verts": verts, "indices": indices, "surfaces": surfaces, "materials": materials,
            "textures": textures}, stats, problems, notes


def _vertex(src, v, jmap, base_joint, part_id, xf):
    pos = vf.m_point(xf, vf.iw_from_glb(src["pos"][v]))
    n = vf.m_dir(xf, vf.iw_from_glb(src["nrm"][v]))
    ln = vf.v_len(n) or 1.0
    nrm = [max(-32767, min(32767, int(round(c / ln * 32767)))) for c in n] + [0]
    ws = [max(0.0, float(w)) for w in src["weights"][v]]
    js = [jmap[int(j)] if int(j) < len(jmap) else base_joint for j in src["joints"][v]]
    tot = sum(ws) or 1.0
    q = [int(round(w / tot * 255)) for w in ws]
    big = max(range(4), key=lambda k: ws[k])
    q[big] += 255 - sum(q)
    js = [j if q[k] > 0 else base_joint for k, j in enumerate(js)]
    if src["color"] is not None:
        c = src["color"][v]
        col = [int(round(x * 255)) if isinstance(x, float) else int(x) for x in (list(c) + [1.0] * 4)[:4]]
    else:
        col = [255, 255, 255, 255]
    return {"pos": pos, "nrm": nrm, "uv": list(src["uv"][v]), "joints": js, "weights": q,
            "color": [max(0, min(255, x)) for x in col], "part": part_id}


# ------------------------------------------------------------------------------------------------ anims
def convert_clip(xa, clip_name, skel, extra_whitelist):
    """M3 semantics (convert_viewmodel.convert_clip): hand tracks are absolute local translations (viewhands model,
    nulled trans), gun tracks are deltas on the bind offset (the model has a trans array, rigid or animated)."""
    names = {b["name"]: i for i, b in enumerate(skel["bones"])}
    tracks, dropped, unknown = [], [], []
    for b in xa["bones"]:
        if b["name"] not in names:
            (dropped if (cv.whitelisted(b["name"]) or b["name"] in extra_whitelist) else unknown).append(b["name"])
            continue
        q, t = b["quat"], b["trans"]
        bone = skel["bones"][names[b["name"]]]
        base = bone["t"] if bone["part"] != "hands" else [0.0, 0.0, 0.0]
        tracks.append({
            "bone": names[b["name"]],
            "rot_frames": list(q["idx"]) if q else [],
            "rot": [[c / xanim_read.Q for c in k] for k in q["q"]] if q else [],
            "trans_frames": list(t["idx"]) if t else [],
            "trans": [[k[c] + base[c] for c in range(3)] for k in t["t"]] if t else []})
    tracks.sort(key=lambda tr: tr["bone"])
    clip = {"name": clip_name, "fps": xa["framerate"], "numframes": xa["numframes"], "looped": xa["looped"],
            "notes": [(n["name"], n["frame"]) for n in xa["notes"]][:255], "tracks": tracks}
    return clip, dropped, unknown


def build_anims(kv, skel, name):
    xidx, _, _, _ = indexes()
    lower = {k.lower(): v for k, v in kv.items()}
    roles, clips, problems, info = {}, [], [], {"dropped": {}, "parsed": {}}
    by_file = {}
    wl = EXTRA_WHITELIST.get(name, set())
    for role, field in list(BASE_ROLES.items()) + list(OPT_ROLES.items()):
        anim = (lower.get(field.lower()) or "").strip()
        if not anim:
            if role in BASE_ROLES and role not in FALLBACK_ROLES:
                problems.append(f"anim field {field} empty")
            continue
        fname = anim.lower()
        if fname not in by_file:
            p = xidx.get(fname)
            if p is None:
                problems.append(f"{field}: xanim {anim} missing")
                continue
            try:
                xa = xanim_read.read_xanim(str(p))
            except Exception as e:  # noqa: BLE001 - any parse failure is a conversion problem
                problems.append(f"{field}: xanim {anim}: {e}")
                continue
            clip, dropped, unknown = convert_clip(xa, fname, skel, wl)
            if unknown:
                problems.append(f"{fname}: bones neither in the skeleton nor whitelisted: {sorted(set(unknown))}")
            info["dropped"][fname] = sorted(set(dropped))
            info["parsed"][fname] = {"numframes": xa["numframes"], "fps": xa["framerate"], "file": p}
            by_file[fname] = clip
            clips.append(clip)
        roles[role] = fname
    # section 5.4: an empty reloadEmptyAnim (launchers, shell-by-shell shotguns) plays the reload clip
    for role, fb in FALLBACK_ROLES.items():
        if role not in roles and fb in roles:
            roles[role] = roles[fb]
    return roles, clips, problems, info


def clip_table(kv, roles, clips):
    by = {c["name"]: c for c in clips}
    lower = {k.lower(): v for k, v in kv.items()}
    out = {}
    for role, cname in sorted(roles.items()):
        c = by[cname]
        dur = c["numframes"] / c["fps"] if c["fps"] else 0.0
        tf = ROLE_TIME.get(role)
        t = float(lower[tf.lower()]) if tf and lower.get(tf.lower()) not in (None, "") else 0.0
        # Every timed clip plays over its weapon time field (rate = duration / time); a 0-frame clip or a 0 time keeps
        # rate 1 (never divided by zero). The sniper ADS fire clip is the idle loop (adsFireAnim = idle): rate 1.
        rate = dur / t if (dur > 0 and t > 0 and not c["looped"]) else 1.0
        if role == "sprint_loop" and dur > 0 and t > 0:
            rate = dur / t
        out[role] = {"clip": cname, "frames": c["numframes"], "fps": c["fps"], "duration": round(dur, 6),
                     "looped": bool(c["looped"]), "time_field": tf, "time": t, "rate": round(rate, 6),
                     "notes": [{"name": n, "frame": f, "t_frac": round(f / c["numframes"], 6) if c["numframes"] else 0.0}
                               for n, f in c["notes"]]}
    return out


# ------------------------------------------------------------------------------------------------ stats
def build_stats(name, kv, path, skel, roles):
    lower = {k.lower(): v for k, v in kv.items()}
    stats = {"name": name, "g": GUN_INDEX[name], "class": CLASS_OF.get(name, "?"), "display": USER_NAMES.get(name, name),
             "weapon_file_sha256": sha256_bytes(path.read_bytes())}
    for k in STAT_STRINGS:
        stats[k] = (lower.get(k.lower()) or "").strip()
    numbers = {}
    for k in inv.NUM_FIELDS:
        v = lower.get(k.lower())
        if v is None:
            continue
        x = num(v)
        if x is not None:
            numbers[k] = x
    stats["num"] = numbers
    stats["move_class"] = MOVE_CLASS.get(stats["weaponClass"], "other")
    stats["tags"] = {t: (skel["tags"][t] != NO_TAG) for t in vf.SKEL_TAGS}
    stats["roles"] = sorted(roles)
    return stats


# ------------------------------------------------------------------------------------------------ sounds
def note_names(clips):
    out = set()
    for c in clips:
        for n, _f in c["notes"]:
            if n in RUMBLE_NOTES or n.startswith("melee_"):
                continue
            out.add(n)
    return out


def build_sounds(kv, clips):
    """sounds.json content + the WAV files it needs ({rel: src path}). Aliases resolve in the MP zone first
    (inventory_m6.resolve_alias zone order); one level of secondary aliases (mech / raise / fire layers)."""
    lower = {k.lower(): v for k, v in kv.items()}
    nmap = {}
    for line in (lower.get("notetracksoundmap") or "").split("\n"):
        p = line.split()
        if len(p) == 2:
            nmap[p[0]] = p[1]
    aliases, files, problems, notes = {}, {}, [], []
    roles, note_alias = {}, {}

    def resolve(alias, depth=0):
        if alias in aliases or not alias:
            return alias in aliases
        r = inv.resolve_alias(alias)
        if r is None or r.get("missing"):
            notes.append(f"alias {alias} in no dumped zone: skipped")
            return False
        variants = []
        for row in r["rows"]:
            w = row.get("wav")
            if not w or w.get("error"):
                notes.append(f"alias {alias}: {row.get('file')} not found (silent by design if null.wav)")
                continue
            src = ROOT / w["path"]
            info = check_assets.wav_info(src)
            if "error" in info or info["tag"] != 1 or info["bits"] != 16:
                problems.append(f"alias {alias}: {w['path']} is not 16-bit PCM ({info})")
                continue
            data = src.read_bytes()
            rel = f"_snd/{sha256_bytes(data)[:8]}_{src.name.lower()}"
            files[rel] = src
            sec = (row.get("secondary") or "").strip()
            variants.append({"file": rel, "vol": [float(row["vol"][0] or 1.0), float(row["vol"][1] or 1.0)],
                             "pitch": [float(row["pitch"][0] or 1.0), float(row["pitch"][1] or 1.0)],
                             "secondary": sec or None, "rate": info["rate"], "channels": info["channels"]})
        if not variants:
            return False
        aliases[alias] = {"zone": r["zone"], "variants": variants}
        if depth == 0:
            for v in variants:
                if v["secondary"]:
                    if not resolve(v["secondary"], 1):
                        v["secondary"] = None
        return True

    for role, field in SOUND_ROLES.items():
        a = (lower.get(field.lower()) or "").strip()
        if a and resolve(a):
            roles[role] = a
    for n in sorted(note_names(clips)):
        a = nmap.get(n, n)
        if resolve(a):
            note_alias[n] = a
    snd = {"schema": SCHEMA, "roles": roles, "notes": note_alias,
           "aliases": {k: aliases[k] for k in sorted(aliases)}}
    return snd, files, problems, notes


# ------------------------------------------------------------------------------------------------ world model (C3)
def build_world(kv):
    """C3 PASS C: the gun's world model per LOD as a rigid mesh in MW2 model space (inches, the model origin =
    tag_weapon): every vertex on joint 0 with weight 255 (the renderer poses it as one piece on a character).
    Attachment surfaces hidden by the weapon file's hideTags are left out, as on the viewmodel. Returns
    ({lod: mesh}, {texture name: tex}, problems, notes, inputs); lod0 is required, lod1 optional."""
    _, glbs, _, images = indexes()
    lower = {k.lower(): v for k, v in kv.items()}
    wm = (lower.get("worldmodel") or "").strip()
    meshes, textures, problems, notes, inputs = {}, {}, [], [], []
    if not wm:
        return meshes, textures, ["worldModel field empty"], notes, inputs
    hide = set(t for t in (lower.get("hidetags") or "").split("\n") if t.strip())
    for lod in WORLD_LODS:
        p = glbs.get(f"{wm.lower()}_lod{lod}.glb")
        if not p:
            (problems if lod == 0 else notes).append(f"world model {wm} lod{lod} GLB missing")
            continue
        g = vf.Glb(p)
        inputs.append(p)
        skins = g.json.get("skins") or []
        if not skins:
            problems.append(f"{p.name}: no skin")
            continue
        joints = skins[0]["joints"]
        root = joints[0]
        q = g.nodes[root].get("rotation", [0.0, 0.0, 0.0, 1.0])
        if g.nodes[root].get("name") != "tag_weapon" or max(abs(a - b) for a, b in zip(q, [-0.7071068, 0.0, 0.0, 0.7071068])) > 1e-5:
            problems.append(f"{p.name}: root {g.nodes[root].get('name')} {q} (want tag_weapon with the OAT root quat)")
            continue
        hidden_nodes = cv._descendants(g, hide)
        hidden_joints = {k for k, ni in enumerate(joints) if ni in hidden_nodes}
        verts, indices, surfaces, materials = [], [], [], []
        mat_index, tex_list, tex_index = {}, [], {}
        cache = {}
        for mesh in g.json["meshes"]:
            for pr in mesh["primitives"]:
                at = pr["attributes"]
                key = (at["POSITION"], at["NORMAL"], at["TEXCOORD_0"], at.get("JOINTS_0"), at.get("WEIGHTS_0"))
                if key not in cache:
                    cache[key] = {"pos": g.accessor(at["POSITION"]), "nrm": g.accessor(at["NORMAL"]), "uv": g.accessor(at["TEXCOORD_0"]),
                                  "joints": g.accessor(at["JOINTS_0"]) if at.get("JOINTS_0") is not None else None,
                                  "weights": g.accessor(at["WEIGHTS_0"]) if at.get("WEIGHTS_0") is not None else None, "remap": {}}
                src = cache[key]
                idx = g.accessor(pr["indices"])
                used = sorted(set(idx))
                if src["joints"] is not None and all(
                        all((w <= 0.0) or (int(j) in hidden_joints) for j, w in zip(src["joints"][v], src["weights"][v])) for v in used):
                    continue  # an attachment the weapon file hides
                mname = g.json["materials"][pr["material"]]["name"].split("/")[-1]
                if mname not in mat_index:
                    m = load_material(mname)
                    if m is None or not m["color_map"]:
                        notes.append(f"world {wm} lod{lod}: material {mname} without a colour map: surface skipped")
                        continue
                    t = m["color_map"]
                    if t not in textures:
                        ip = images.get(t.lower() + ".dds")
                        if not ip:
                            notes.append(f"world {wm} lod{lod}: colour map {t} DDS missing: surface skipped")
                            continue
                        try:
                            dds = read_dds_any(ip)
                        except (OSError, vf.FormatError) as e:
                            notes.append(f"world {wm} lod{lod}: texture {t}: {e}: surface skipped")
                            continue
                        textures[t] = {"dds": dds, "file": f"{sha256_bytes(ip.read_bytes())[:8]}_{t.lower().replace('#', '_')}.vmtex", "src": ip}
                    if t not in tex_index:
                        tex_index[t] = len(tex_list)
                        tex_list.append(t)
                    mat_index[mname] = len(materials)
                    materials.append({"name": mname, "texture": tex_index[t], "blend": m["blend"], "json": m["json"]})
                first = len(indices)
                for v in idx:
                    if v not in src["remap"]:
                        src["remap"][v] = len(verts)
                        pos = vf.iw_from_glb(src["pos"][v])
                        n = vf.iw_from_glb(src["nrm"][v])
                        ln = vf.v_len(n) or 1.0
                        nrm = [max(-32767, min(32767, int(round(c / ln * 32767)))) for c in n] + [0]
                        verts.append({"pos": pos, "nrm": nrm, "uv": list(src["uv"][v]), "joints": [0, 0, 0, 0],
                                      "weights": [255, 0, 0, 0], "color": [255, 255, 255, 255], "part": 0})
                    indices.append(src["remap"][v])
                surfaces.append({"first": first, "count": len(idx), "material": mat_index[mname], "part": 0})
        if not surfaces:
            (problems if lod == 0 else notes).append(f"world {wm} lod{lod}: no surface drawn")
            continue
        if len(verts) >= 65536:
            problems.append(f"world {wm} lod{lod}: {len(verts)} vertices do not fit u16 indices")
            continue
        meshes[lod] = {"verts": verts, "indices": indices, "surfaces": surfaces, "materials": materials, "textures": tex_list,
                       "glb": p}
    return meshes, textures, problems, notes, inputs


# ------------------------------------------------------------------------------------------------ one gun
def convert_gun(name, hands, shared):
    """Everything for one gun in memory: (bundle, problems, notes)."""
    problems, notes = [], []
    _, glbs, _, _ = indexes()
    kv, path = weapon_kv(name)
    if name not in GUN_INDEX:
        return None, [f"{name} has no stable index g"], notes
    lower = {k.lower(): v for k, v in kv.items()}
    gm = (lower.get("gunmodel") or "").strip()
    gp = glbs.get(gm.lower() + "_lod0.glb")
    if not gp:
        return None, [f"gun model {gm} GLB missing"], notes
    gun = vf.Glb(gp)
    rocket = None
    rm = (lower.get("rocketmodel") or "").strip()
    rp = glbs.get(rm.lower() + "_lod0.glb") if rm else None
    if rm and rp:
        rocket = vf.Glb(rp)
    elif rm:
        notes.append(f"rocketModel {rm} GLB missing: no rocket part")
    skel, world, p = build_skeleton(hands, gun, rocket)
    problems += p
    if skel is None:
        return None, problems, notes
    hide = [t for t in (lower.get("hidetags") or "").split("\n") if t.strip()]
    mesh, mstats, p, n = build_mesh(skel, world, hands, gun, hide, rocket)
    problems += p
    notes += n
    textures = {}
    for t in mesh["textures"]:
        _, _, _, images = indexes()
        ip = images.get(t.lower() + ".dds")
        if not ip:
            problems.append(f"colour map {t} DDS missing")
            continue
        try:
            dds = read_dds_any(ip)
        except (OSError, vf.FormatError) as e:
            problems.append(f"texture {t}: {e}")
            continue
        raw = ip.read_bytes()
        textures[t] = {"dds": dds, "file": f"{sha256_bytes(raw)[:8]}_{t.lower().replace('#', '_')}.vmtex", "src": ip}
    roles, clips, p, ainfo = build_anims(kv, skel, name)
    problems += p
    for fname, dropped in ainfo["dropped"].items():
        extra = sorted(set(dropped) & EXTRA_WHITELIST.get(name, set()))
        if extra:
            notes.append(f"{fname}: dropped bones absent from the model {extra}")
    seg = num(lower.get("segmentedreload")) == 1
    if seg and not all(r in roles for r in ("reload_start", "reload_end")):
        problems.append("segmented reload without reload_start / reload_end clips")
    ctab = clip_table(kv, roles, clips) if not problems else {}
    stats = build_stats(name, kv, path, skel, roles)
    snd, sfiles, p, n = build_sounds(kv, clips)
    problems += p
    notes += n
    overlay = (lower.get("adsoverlayshader") or "").strip()
    if overlay:
        stats["overlay"] = {"material": overlay, "file": f"_overlay/{overlay.lower()}.vmtex",
                            "width": num(lower.get("adsoverlaywidth")), "height": num(lower.get("adsoverlayheight"))}
        shared["overlays"].add(overlay)
    if "fire" not in snd["roles"]:
        notes.append("no fire sound resolved")
    # C3 PASS C: the world model (a failure only means no world gun for this gun: notes, never a gun problem)
    wmeshes, wtex, wp, wn, winputs = build_world(kv)
    notes += wn + [f"world model: {x} (no world gun)" for x in wp]
    if wp:
        wmeshes, wtex, winputs = {}, {}, []
    return {"name": name, "kv": kv, "path": path, "gun_glb": gp, "rocket_glb": rp if rocket is not None else None,
            "skel": skel, "world": world, "mesh": mesh, "mesh_stats": mstats, "textures": textures, "roles": roles,
            "clips": clips, "anim_info": ainfo, "clip_table": ctab, "stats": stats, "sounds": snd,
            "sound_files": sfiles, "segmented": seg, "world_meshes": wmeshes, "world_textures": wtex,
            "world_inputs": winputs}, problems, notes


def serialize_gun(b):
    """{relative path under converted/guns: bytes} for one gun (textures and sounds under the shared dirs)."""
    out = {}
    d = b["name"]
    out[f"{d}/gun.vmskel"] = vf_write_skel(b["skel"])
    out[f"{d}/gun.vmmesh"] = vf.write_mesh(b["mesh"]["verts"], b["mesh"]["indices"], b["mesh"]["surfaces"],
                                           b["mesh"]["materials"],
                                           [b["textures"][t]["file"] for t in b["mesh"]["textures"]])
    out[f"{d}/gun.vmanim"] = vf.write_anim(b["clips"])
    out[f"{d}/clips.json"] = jdump({"schema": SCHEMA, "gun": d, "roles": b["clip_table"]})
    out[f"{d}/stats.json"] = jdump({"schema": SCHEMA, **b["stats"]})
    out[f"{d}/sounds.json"] = jdump(b["sounds"])
    for t in b["mesh"]["textures"]:
        tx = b["textures"][t]
        out[f"_tex/{tx['file']}"] = vf.write_tex(tx["dds"])
    for rel, src in b["sound_files"].items():
        out[rel] = src.read_bytes()
    # C3 PASS C: the world model LODs (rigid meshes) and their colour maps (shared _tex, deduplicated by content)
    for lod, m in sorted(b.get("world_meshes", {}).items()):
        out[f"{d}/world_lod{lod}.vmmesh"] = vf.write_mesh(m["verts"], m["indices"], m["surfaces"], m["materials"],
                                                         [b["world_textures"][t]["file"] for t in m["textures"]])
    for t, tx in sorted(b.get("world_textures", {}).items()):
        out[f"_tex/{tx['file']}"] = vf.write_tex(tx["dds"])
    return out


def vf_write_skel(skel):
    """vm_format.write_skel with the optional tags written as 0xFFFF."""
    p = bytearray()
    for b in skel["bones"]:
        p += vf._name(b["name"]) + struct.pack("<h", b["parent"]) + struct.pack("<3f", *b["t"]) + struct.pack("<4f", *b["q"])
        p += struct.pack("<12f", *[b["ibm"][r][c] for r in range(3) for c in range(4)])
    for t in vf.SKEL_TAGS:
        p += struct.pack("<H", skel["tags"][t])
    return vf.header(vf.MAGIC_SKEL, len(skel["bones"]), len(p)) + bytes(p)


def read_skel_any(data):
    n = vf.parse_header(data, vf.MAGIC_SKEL)
    c = vf.Cur(data)
    bones = []
    for i in range(n):
        name = c.name()
        parent = c.un("h")[0]
        t = list(c.un("3f"))
        q = list(c.un("4f"))
        f = c.un("12f")
        if parent >= i:
            raise vf.FormatError(f"bone {name}: parent {parent} not before it")
        bones.append({"name": name, "parent": parent, "t": t, "q": q,
                      "ibm": [list(f[0:4]), list(f[4:8]), list(f[8:12]), [0.0, 0.0, 0.0, 1.0]]})
    tags = {t: c.u16() for t in vf.SKEL_TAGS}
    c.end()
    return {"bones": bones, "tags": tags}


def gun_manifest(b, files):
    mine = {k: v for k, v in files.items() if k.startswith(b["name"] + "/")}
    shared = {k: v for k, v in files.items() if not k.startswith(b["name"] + "/")}
    ins = [b["path"], b["gun_glb"], cv.GLB_HANDS] + ([b["rocket_glb"]] if b["rocket_glb"] else [])
    ins += sorted({info["file"] for info in b["anim_info"]["parsed"].values()}, key=str)
    ins += sorted({m["json"] for m in b["mesh"]["materials"]}, key=str)
    ins += sorted({tx["src"] for tx in b["textures"].values()}, key=str)
    ins += sorted(set(b["sound_files"].values()), key=str)
    # C3 PASS C: the world model inputs (GLBs, their materials and colour maps)
    ins += sorted(set(b.get("world_inputs", [])), key=str)
    ins += sorted({m["json"] for wm in b.get("world_meshes", {}).values() for m in wm["materials"]} - set(ins), key=str)
    ins += sorted({tx["src"] for tx in b.get("world_textures", {}).values()} - set(ins), key=str)
    lower = {k.lower(): v for k, v in b["kv"].items()}
    times = {}
    for f in cv.WEAPON_TIMES + ["firstRaiseTime", "rechamberTime", "reloadStartTime", "reloadEndTime"]:
        x = num(lower.get(f.lower()))
        if x is not None:
            times[f] = x
    return {
        "format_version": vf.FORMAT_VERSION,
        "converter": CONVERTER_VERSION,
        # paths relative to the gun folder; shared files are "../_tex/..." / "../_snd/..."
        "files": {("../" + k if k in shared else k[len(b["name"]) + 1:]): {"sha256": sha256_bytes(v), "size": len(v)}
                  for k, v in sorted(files.items())},
        "inputs": {str(pathlib.Path(p).relative_to(ROOT)).replace("\\", "/"): sha256_bytes(pathlib.Path(p).read_bytes())
                   for p in ins},
        "weapon_times": times,
        "roles": {r: c for r, c in sorted(b["roles"].items())},
        "skel_file": "gun.vmskel", "mesh_file": "gun.vmmesh", "anim_file": "gun.vmanim",
        # C3 PASS C: the world model LOD meshes (rigid, joint 0) for the world gun drawn on characters
        "world_files": [f"world_lod{lod}.vmmesh" for lod in sorted(b.get("world_meshes", {}))],
        "counts": {"bones": len(b["skel"]["bones"]), "vertices": len(b["mesh"]["verts"]),
                   "gun_tris": b["mesh_stats"].get("gun_tris"), "hand_tris": b["mesh_stats"].get("hands_tris"),
                   "rocket_tris": b["mesh_stats"].get("rocket_tris", 0), "clips": len(b["clips"])},
    }


def build_overlay(name):
    _, _, mats, images = indexes()
    m = load_material(name)
    if m is None or not m["color_map"]:
        raise ConvError(f"overlay material {name}: no colorMap")
    ip = images.get(m["color_map"].lower() + ".dds")
    if not ip:
        raise ConvError(f"overlay {name}: DDS {m['color_map']} missing")
    dds = read_dds_any(ip, allow_partial_mips=True)
    return vf.write_tex(dds), dds, ip


# ------------------------------------------------------------------------------------------------ all guns
def build_all(guns):
    """Converts every gun in memory. Returns (bundles {name: b|None}, problems {name: [...]}, notes, files)."""
    hands = vf.Glb(cv.GLB_HANDS)
    shared = {"overlays": set()}
    bundles, problems, notes, files = {}, {}, {}, {}
    for name in guns:
        try:
            b, p, n = convert_gun(name, hands, shared)
        except (OSError, ValueError, KeyError, IndexError, vf.FormatError, ConvError) as e:
            b, p, n = None, [f"exception {type(e).__name__}: {e}"], []
        problems[name], notes[name] = p, n
        bundles[name] = b if (b is not None and not p) else None
        if bundles[name] is not None:
            try:
                f = serialize_gun(b)
                f[f"{name}/manifest.json"] = jdump(gun_manifest(b, f))
                files[name] = f
            except (OSError, ValueError, KeyError, struct.error, vf.FormatError) as e:
                problems[name].append(f"serialize: {e}")
                bundles[name] = None
    overlays = {}
    for ov in sorted(shared["overlays"]):
        try:
            data, dds, ip = build_overlay(ov)
            overlays[f"_overlay/{ov.lower()}.vmtex"] = (data, dds, ip)
        except (OSError, ConvError, vf.FormatError) as e:
            for name in guns:
                if bundles.get(name) is not None and bundles[name]["stats"].get("overlay", {}).get("material") == ov:
                    problems[name].append(f"overlay {ov}: {e}")
    index = {"schema": SCHEMA, "converter": CONVERTER_VERSION, "guns": []}
    for name in guns:
        b = bundles.get(name)
        man = files.get(name, {}).get(f"{name}/manifest.json")
        index["guns"].append({"name": name, "g": GUN_INDEX.get(name), "class": CLASS_OF.get(name),
                              "ok": b is not None and not problems[name],
                              "manifest_sha256": sha256_bytes(man) if man else None,
                              "problems": problems[name]})
    return bundles, problems, notes, files, overlays, index


def all_output_bytes(files, overlays, index):
    out = {}
    for f in files.values():
        out.update(f)
    for k, (data, _, _) in overlays.items():
        out[k] = data
    out["index.json"] = jdump(index)
    return out


def write_outputs(files, overlays, index):
    """Per gun: manifest removed first, files written (tmp + rename), manifest last. Shared dirs: written, stale files
    of earlier conversions removed. index.json last of all."""
    OUT.mkdir(parents=True, exist_ok=True)
    idx = OUT / "index.json"
    if idx.exists():
        idx.unlink()
    wanted = set()
    for name, f in files.items():
        man = OUT / name / "manifest.json"
        if man.exists():
            man.unlink()
    allf = all_output_bytes(files, overlays, index)
    for rel, data in sorted(allf.items()):
        if rel.endswith("/manifest.json") or rel == "index.json":
            continue
        p = OUT / rel
        wanted.add(p.resolve())
        p.parent.mkdir(parents=True, exist_ok=True)
        if p.is_file() and p.stat().st_size == len(data) and p.read_bytes() == data:
            continue
        tmp = p.with_name(p.name + ".tmp")
        tmp.write_bytes(data)
        os.replace(tmp, p)
    for rel, data in sorted(allf.items()):
        if rel.endswith("/manifest.json"):
            p = OUT / rel
            wanted.add(p.resolve())
            tmp = p.with_name(p.name + ".tmp")
            tmp.write_bytes(data)
            os.replace(tmp, p)
    wanted.add(idx.resolve())
    # stale files: only inside the folders this converter owns
    owned = [OUT / "_tex", OUT / "_snd", OUT / "_overlay"] + [OUT / n for n in files]
    for d in owned:
        if d.is_dir():
            for p in d.iterdir():
                if p.is_file() and p.resolve() not in wanted:
                    p.unlink()
    # guns that failed: their manifest is gone (the DLL skips them)
    for g in index["guns"]:
        if not g["ok"]:
            m = OUT / g["name"] / "manifest.json"
            if m.exists():
                m.unlink()
    tmp = idx.with_name("index.json.tmp")
    tmp.write_bytes(allf["index.json"])
    os.replace(tmp, idx)


# ------------------------------------------------------------------------------------------------ checks
def bc3_alpha_block(block):
    """Decoded 16 alpha values (0..255) of one BC3 block (first 8 bytes)."""
    a0, a1 = block[0], block[1]
    bits = int.from_bytes(block[2:8], "little")
    pal = [a0, a1]
    if a0 > a1:
        pal += [((6 - i) * a0 + (i + 1) * a1 + 3) // 7 for i in range(6)]
    else:
        pal += [((4 - i) * a0 + (i + 1) * a1 + 2) // 5 for i in range(4)] + [0, 255]
    return [pal[(bits >> (3 * k)) & 7] for k in range(16)]


def check_gun(name, b, files, inv_gun):
    """Known answers of one gun (section 13.1). Returns problems."""
    P = []
    st = b["stats"]
    # stats == weapon file (through the inventory's independent read of the same file)
    for k, v in inv_gun["num"].items():
        got = st["num"].get(k)
        if isinstance(v, (int, float)) and (got is None or abs(float(got) - float(v)) > 1e-6):
            P.append(f"stats.num.{k}={got} != weapon file {v}")
    for k in ("fireType", "weaponClass", "gunModel"):
        if st[k] != inv_gun["str"].get(k, ""):
            P.append(f"stats.{k}={st[k]!r} != weapon file {inv_gun['str'].get(k)!r}")
    if st["move_class"] != MOVE_CLASS.get(st["weaponClass"], "other"):
        P.append("move_class does not follow weaponClass")
    # every anim field of the inventory resolves to a written clip (or is an unused field)
    for role, field in list(BASE_ROLES.items()) + list(OPT_ROLES.items()):
        a = inv_gun["anims"].get(field)
        if a and not a.get("missing") and role not in b["roles"]:
            P.append(f"anim {field} ({a.get('name')}) not written")
        if a and a.get("missing"):
            P.append(f"anim {field} missing in the dumps")
    # clip rates: the scaled duration equals the time field (within one tick)
    for role, c in b["clip_table"].items():
        if c["time"] > 0 and c["duration"] > 0 and not c["looped"]:
            if abs(c["duration"] / c["rate"] - c["time"]) > TOL_TICK:
                P.append(f"clip {role}: {c['duration']} s at rate {c['rate']} != {c['time']} s")
        if not math.isfinite(c["rate"]) or c["rate"] <= 0:
            P.append(f"clip {role}: rate {c['rate']}")
    for (gname, role), (dur, t) in KA_RAW_PAIRS.items():
        if gname == name:
            c = b["clip_table"].get(role)
            if c is None or abs(c["duration"] - dur) > 0.02 or abs(c["time"] - t) > 1e-6:
                P.append(f"known pair {role}: {c and (c['duration'], c['time'])} != {(dur, t)}")
    # bones
    nb = len(b["skel"]["bones"])
    if nb > 256 or len(b["mesh"]["verts"]) > 65535:
        P.append(f"{nb} bones / {len(b['mesh']['verts'])} vertices over the caps")
    bnames = {x["name"] for x in b["skel"]["bones"]}
    for cl in b["clips"]:
        for tr in cl["tracks"]:
            if not (0 <= tr["bone"] < nb):
                P.append(f"{cl['name']}: track bone {tr['bone']}")
    for fname, dropped in b["anim_info"]["dropped"].items():
        bad = [d for d in dropped if d in bnames]
        if bad:
            P.append(f"{fname}: dropped bones present in the skeleton {bad}")
    # bind . IBM == I and the bind-pose skin reproduces the vertices
    bw = vf.fk(b["skel"], [(x["t"], x["q"]) for x in b["skel"]["bones"]])
    for i, x in enumerate(b["skel"]["bones"]):
        if vf.m_maxdiff(vf.m_mul(bw[i], x["ibm"]), vf.m_identity()) > 1e-4:
            P.append(f"{x['name']}: bind . IBM != I")
            break
    pal = vf.palette(b["skel"], bw)
    err = max(vf.v_len(vf.v_sub(vf.skin_point(pal, v), v["pos"])) for v in b["mesh"]["verts"][::11])
    if err > 0.01:
        P.append(f"bind-pose skinning moves vertices by {err:.4f} in")
    # translation semantics (M3 check 1m generalised): at least one gun track that starts at bind (delta ~0) while
    # its bind offset is >= 0.5 in, where any reload track exists [I for guns without one]
    proof = []
    for cl in b["clips"]:
        for tr in cl["tracks"]:
            bone = b["skel"]["bones"][tr["bone"]]
            if bone["part"] == "gun" and tr["trans"] and vf.v_len(bone["t"]) >= 0.5:
                if vf.v_len(vf.v_sub(tr["trans"][0], bone["t"])) <= 0.05:
                    proof.append(f"{cl['name']}:{bone['name']}")
    b.setdefault("report", {})["delta_proof"] = proof[:3]
    # review M6 fidelity: the body multiplier the regulation uses (locTorsoUpper > 1 only on these snipers)
    tu = float(st["num"].get("locTorsoUpper", 0) or 0)
    if st["class"] != "launcher" and (tu > 1.0) != (name in KA_BODY_MULT):
        P.append(f"locTorsoUpper {tu} > 1 = {tu > 1.0} (known: {name in KA_BODY_MULT})")
    if name in KA_BODY_MULT and abs(tu - KA_BODY_MULT[name]) > 1e-6:
        P.append(f"locTorsoUpper {tu} != known {KA_BODY_MULT[name]}")
    # tags
    has = b["skel"]["tags"]["tag_sight_on"] != NO_TAG
    if has != (name in KA_SIGHT_ON):
        P.append(f"tag_sight_on present={has} (known: {name in KA_SIGHT_ON})")
    # rocket part (rpg)
    if b["rocket_glb"] is not None:
        names = [x["name"] for x in b["skel"]["bones"]]
        r = names.index(ROCKET_PREFIX + "tag_clip") if ROCKET_PREFIX + "tag_clip" in names else -1
        if r < 0 or names[b["skel"]["bones"][r]["parent"]] != "tag_clip" or \
                not any(s["part"] == PART_ROCKET for s in b["mesh"]["surfaces"]):
            P.append("rocket part not parented to tag_clip / no rocket surface")
    if name == "rpg_mp" and b["rocket_glb"] is None:
        P.append("rpg without its rocket part")
    # mechanics flags
    seg = st["num"].get("segmentedReload") == 1
    if seg != (name in KA_SEGMENTED):
        P.append(f"segmentedReload={seg} (known {name in KA_SEGMENTED})")
    bolt = st["num"].get("boltAction") == 1
    if bolt != (name in KA_BOLT):
        P.append(f"boltAction={bolt} (known {name in KA_BOLT})")
    if bolt and "rechamber" not in b["roles"]:
        P.append("bolt-action gun without a rechamber clip")
    # notetracks
    for (gname, role), want in KA_NOTES.items():
        if gname != name:
            continue
        c = b["clip_table"].get(role, {})
        got = [(n["name"], n["frame"], c.get("frames")) for n in c.get("notes", []) if n["name"] not in RUMBLE_NOTES]
        for w in want:
            if w not in got:
                P.append(f"notetrack {w} not in {role} {got}")
    # sounds: fire resolves in the MP zone; known aliases; each file 16-bit PCM, data == source
    snd = b["sounds"]
    fa = snd["roles"].get("fire")
    if not fa or snd["aliases"][fa]["zone"] != "localized_common_mp.ff":
        P.append(f"fire alias {fa} not resolved in localized_common_mp")
    if name in KA_FIRE_ALIAS and fa != KA_FIRE_ALIAS[name]:
        P.append(f"fire alias {fa} != {KA_FIRE_ALIAS[name]}")
    for rel, src in b["sound_files"].items():
        data = files.get(rel)
        if data is None:
            P.append(f"sound {rel} not written")
            continue
        info = check_assets.wav_info(src)
        if sha256_bytes(data) != sha256_bytes(src.read_bytes()):
            P.append(f"sound {rel} differs from its source")
        if info.get("tag") != 1:
            P.append(f"sound {rel}: format tag {info.get('tag')}")
    # C3 PASS C: the third-person fire sound peers hear (an MW2 *_npc alias, written like every other alias)
    fn = snd["roles"].get("fire_npc")
    if not fn or not fn.endswith("_npc"):
        P.append(f"fire_npc alias {fn!r} not resolved (weapon file fireSound)")
    # C3 PASS C: the world model: lod0 written, rigid (joint 0, weight 255), a plausible gun length, round trip
    wl = b.get("world_meshes", {})
    if 0 not in wl or f"{name}/world_lod0.vmmesh" not in files:
        P.append("world model lod0 not written")
    for lod, wm in wl.items():
        data = files.get(f"{name}/world_lod{lod}.vmmesh")
        if data is None:
            P.append(f"world lod{lod} missing from the files")
            continue
        if any(v["joints"] != [0, 0, 0, 0] or v["weights"] != [255, 0, 0, 0] for v in wm["verts"]):
            P.append(f"world lod{lod}: not rigid (joint 0 / weight 255 on every vertex)")
        xs = [v["pos"][0] for v in wm["verts"]]
        if not (10.0 <= max(xs) - min(xs) <= 90.0):
            P.append(f"world lod{lod}: length {max(xs) - min(xs):.1f} in (want 10..90)")
        try:
            me = vf.read_mesh(data)
            if len(me["verts"]) != len(wm["verts"]):
                P.append(f"world lod{lod}: round trip vertex count differs")
            for tf in me["textures"]:
                if f"_tex/{tf}" not in files:
                    P.append(f"world lod{lod}: texture {tf} not written")
        except vf.FormatError as e:
            P.append(f"world lod{lod}: round trip {e}")
    if name == "ump45_mp" and fa:
        f0 = snd["aliases"][fa]["variants"][0]["file"]
        if M3_UMP45_FIRE.is_file() and files.get(f0) != M3_UMP45_FIRE.read_bytes():
            P.append("UMP45 fire PCM differs from the M3 copy")
    # round trip of the binary files
    try:
        sk = read_skel_any(files[f"{name}/gun.vmskel"])
        me = vf.read_mesh(files[f"{name}/gun.vmmesh"])
        an = vf.read_anim(files[f"{name}/gun.vmanim"])
        if len(sk["bones"]) != nb or len(me["verts"]) != len(b["mesh"]["verts"]) or len(an) != len(b["clips"]):
            P.append("binary round trip counts differ")
        for t in me["textures"]:
            if f"_tex/{t}" not in files:
                P.append(f"texture {t} not written")
    except (KeyError, vf.FormatError) as e:
        P.append(f"round trip: {e}")
    return P


def check_overlays(overlays):
    P = []
    for rel, (data, dds, _ip) in overlays.items():
        tx = vf.read_tex(data)
        if (tx["format"], tx["w"], tx["h"]) != (77, 1024, 1024):
            P.append(f"{rel}: {tx['format']} {tx['w']}x{tx['h']} (want BC3 1024x1024)")
            continue
        m0 = tx["mips"][0]
        bw = m0["row_pitch"] // 16

        def block(bx, by):
            o = by * m0["row_pitch"] + bx * 16
            return m0["data"][o:o + 16]
        # the lens (off the reticle lines, 40 % across) is see-through, the corner is the opaque surround; the centre
        # carries the reticle cross (measured: lens ~16-24, corner 255, centre column 47-81)
        lens = bc3_alpha_block(block(int(bw * 0.4), int(m0["rows"] * 0.4)))
        corner = bc3_alpha_block(block(0, 0))
        if sum(lens) / 16 > 64 or min(corner) < 223:
            P.append(f"{rel}: lens alpha mean {sum(lens) / 16:.0f} (want < 64, see-through), corner min {min(corner)} "
                     "(want opaque)")
    return P


def verify_written(files, overlays, index):
    """Every output on disk equals the fresh conversion; every gun manifest lists exactly its files."""
    P = []
    allf = all_output_bytes(files, overlays, index)
    for rel, data in allf.items():
        p = OUT / rel
        if not p.is_file():
            P.append(f"{rel} not on disk (run the converter)")
        elif p.read_bytes() != data:
            P.append(f"{rel} differs from a fresh conversion")
    for name, f in files.items():
        man = json.loads(f[f"{name}/manifest.json"])
        for k, meta in man["files"].items():
            rel = (k[3:] if k.startswith("../") else f"{name}/{k}")
            d = allf.get(rel)
            if d is None or sha256_bytes(d) != meta["sha256"] or len(d) != meta["size"]:
                P.append(f"{name} manifest entry {k} does not match")
    return P


def run_checks(guns, bundles, problems, files, overlays, index, inv_data):
    P = {}
    for name in guns:
        p = list(problems.get(name, []))
        b = bundles.get(name)
        if b is not None:
            p += check_gun(name, b, files[name], inv_data[name])
        P[name] = p
    P["_overlay"] = check_overlays(overlays)
    if len(index["guns"]) != len(guns) or any(not g["ok"] for g in index["guns"]):
        P["_index"] = [f"index: {sum(1 for g in index['guns'] if g['ok'])}/{len(guns)} ok"]
    return P


# ------------------------------------------------------------------------------------------------ CLI
def cmd_convert(guns):
    bundles, problems, notes, files, overlays, index = build_all(guns)
    write_outputs(files, overlays, index)
    ok = [g["name"] for g in index["guns"] if g["ok"]]
    total = sum(len(d) for f in files.values() for d in f.values())
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps({"index": index, "notes": notes,
                                  "counts": {n: (b and gun_manifest(b, files[n])["counts"]) for n, b in bundles.items()}},
                                 indent=1, default=str), encoding="utf-8")
    for g in index["guns"]:
        if not g["ok"]:
            print(f"FAIL {g['name']}: " + "; ".join(g["problems"]))
    print(f"converted {len(ok)}/{len(guns)} guns ({total} bytes incl. shared textures/sounds) to {OUT}; overlays "
          f"{sorted(overlays)}; report {REPORT}")
    return 0 if len(ok) == len(guns) else 1


def cmd_check(guns, self_test=False):
    bundles, problems, notes, files, overlays, index = build_all(guns)
    inv_data = {n: inv.inventory_gun(n, CLASS_OF.get(n, "?")) for n in guns}
    P = run_checks(guns, bundles, problems, files, overlays, index, inv_data)
    if not self_test:  # (the self-test converts a sample of guns only: the disk holds all of them)
        P["_disk"] = verify_written(files, overlays, index)
        # determinism: a second conversion gives the same bytes
        _b2, _p2, _n2, files2, overlays2, index2 = build_all(guns)
        a, b = all_output_bytes(files, overlays, index), all_output_bytes(files2, overlays2, index2)
        if a != b:
            P["_determinism"] = [f"{len([k for k in a if a.get(k) != b.get(k)])} files differ between two conversions"]
    bad = {k: v for k, v in P.items() if v}
    for k in sorted(P):
        if P[k]:
            print(f"FAIL {k}: " + "; ".join(P[k][:6]))
    n_ok = sum(1 for n in guns if not P.get(n))
    rows = []
    for n in guns:
        bb = bundles.get(n)
        if bb is not None:
            rows.append(f"{n}:bones={len(bb['skel']['bones'])},verts={len(bb['mesh']['verts'])},clips={len(bb['clips'])},"
                        f"snd={len(bb['sounds']['aliases'])}")
    print("REPORT " + " ".join(rows))
    ok = not bad
    n_world = sum(1 for n in guns if bundles.get(n) is not None and 0 in bundles[n].get("world_meshes", {}))
    n_npc = sum(1 for n in guns if bundles.get(n) is not None and "fire_npc" in bundles[n]["sounds"]["roles"])
    print(("PASS" if ok else "FAIL") + f" arsenal converter: {n_ok}/{len(guns)} guns PASS (stats == weapon files, every "
          f"anim field resolved, clip rates == time fields, known pairs/notes/aliases, bone sets, tag_sight_on on "
          f"{len(KA_SIGHT_ON)} guns, rocket part, PCM == source, overlay alpha; C3: world models {n_world}/{len(guns)}, "
          f"npc fire sounds {n_npc}/{len(guns)}), manifests match the disk, deterministic")
    if self_test:
        return P, bundles, problems, files, overlays, index, inv_data
    return 0 if ok else 1


def cmd_self_test(guns):
    """Each deliberate mutation must make a check fail; the clean conversion must pass."""
    sample = [g for g in ("ump45_mp", "cheytac_mp", "spas12_mp", "rpg_mp") if g in guns] or guns[:2]
    P, bundles, problems, files, overlays, index, inv_data = cmd_check(sample, self_test=True)
    if any(P.values()):
        print("FAIL self-test: the clean conversion fails")
        return 1
    caught, missed = [], []

    def expect(label, gun, mutate):
        b = copy.deepcopy(bundles[gun])
        f = dict(files[gun])
        mutate(b, f)
        (caught if check_gun(gun, b, f, inv_data[gun]) else missed).append(label)

    if "ump45_mp" in sample:
        expect("stats number changed", "ump45_mp", lambda b, f: b["stats"]["num"].__setitem__("fireTime", 0.1))
        expect("clip rate off", "ump45_mp", lambda b, f: b["clip_table"]["reload"].__setitem__("rate", 1.3))
        expect("anim role dropped", "ump45_mp", lambda b, f: b["roles"].pop("sprint_in"))
        expect("UMP45 fire PCM changed", "ump45_mp",
               lambda b, f: f.__setitem__(b["sounds"]["aliases"][b["sounds"]["roles"]["fire"]]["variants"][0]["file"], b"RIFF"))
        expect("inverse bind matrix corrupted", "ump45_mp",
               lambda b, f: b["skel"]["bones"][5]["ibm"][0].__setitem__(3, b["skel"]["bones"][5]["ibm"][0][3] + 1.0))
        # C3 PASS C: the world model and the third-person fire sound
        expect("C3: world model lost", "ump45_mp", lambda b, f: (b.__setitem__("world_meshes", {}), f.pop("ump45_mp/world_lod0.vmmesh", None)))
        expect("C3: world model skinned (not rigid)", "ump45_mp",
               lambda b, f: b["world_meshes"][0]["verts"][0].__setitem__("weights", [128, 127, 0, 0]))
        expect("C3: npc fire sound lost", "ump45_mp", lambda b, f: b["sounds"]["roles"].pop("fire_npc"))
    if "cheytac_mp" in sample:
        expect("notetrack moved", "cheytac_mp",
               lambda b, f: b["clip_table"]["rechamber"]["notes"].__setitem__(0, {**b["clip_table"]["rechamber"]["notes"][0], "frame": 4}))
        expect("tag_sight_on invented", "cheytac_mp", lambda b, f: b["skel"]["tags"].__setitem__("tag_sight_on", 3))
        expect("sniper body multiplier lost", "cheytac_mp", lambda b, f: b["stats"]["num"].__setitem__("locTorsoUpper", 1.0))
    if "spas12_mp" in sample:
        expect("fire alias wrong", "spas12_mp", lambda b, f: b["sounds"]["roles"].__setitem__("fire", "weap_spas12_loop_plr"))
        expect("known raw pair", "spas12_mp", lambda b, f: b["clip_table"]["rechamber"].__setitem__("time", 0.5))
    if "rpg_mp" in sample:
        expect("rocket part lost", "rpg_mp",
               lambda b, f: b["mesh"].__setitem__("surfaces", [s for s in b["mesh"]["surfaces"] if s["part"] != PART_ROCKET]))
    # overlay alpha
    if overlays:
        k = next(iter(overlays))
        data, dds, ip = overlays[k]
        flipped = bytearray(data)
        hdr = 32 + 16 + 12
        pitch = dds["mips"][0]["row_pitch"]
        o = hdr + int(dds["mips"][0]["rows"] * 0.4) * pitch + int(pitch // 16 * 0.4) * 16
        flipped[o:o + 8] = b"\xff\xff\x00\x00\x00\x00\x00\x00"
        (caught if check_overlays({k: (bytes(flipped), dds, ip)}) else missed).append("overlay lens opaque")
    if missed:
        print(f"FAIL self-test: mutations not caught: {missed}")
        return 1
    print(f"PASS self-test: {len(caught)} mutations caught ({', '.join(caught)})")
    return 0


def main(argv):
    a = list(argv)
    guns = None
    if "--guns" in a:
        i = a.index("--guns")
        guns = a[i + 1].split(",")
        del a[i:i + 2]
    guns = gun_list(guns)
    unknown = [g for g in guns if g not in GUN_INDEX]
    if unknown:
        print(f"FAIL unknown guns {unknown} (no stable index g)")
        return 2
    if a == ["--check"]:
        return cmd_check(guns)
    if a == ["--self-test"]:
        return cmd_self_test(guns)
    if a:
        print(__doc__)
        return 2
    return cmd_convert(guns)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
