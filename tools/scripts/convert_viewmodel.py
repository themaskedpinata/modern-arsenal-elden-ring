"""M3 converter: MW2 UMP45 viewmodel (gun + hands, colour maps, 12 base anims) -> converted/viewmodel/.

Usage (Python 3.14, isolated mode):
  python -I convert_viewmodel.py              convert dumps/ -> converted/viewmodel/ (manifest.json written last)
                                              and write work/m3/convert_report.json
  python -I convert_viewmodel.py --check      oracle 1 (docs/M3_DESIGN.md 12.1: 1a(Python), 1b-1d, 1f-1k) on the
                                              converted files read back + all 38 xanims; exit 0 = PASS
  python -I convert_viewmodel.py --self-test  oracle 1l: deliberate mutations must each be caught; exit 0 = PASS

Inputs are the user's own MW2 dumps (gitignored); outputs go only to converted/viewmodel/ and work/m3/.
The XAnim layout comes from xanim_read.py (independent implementation). No OAT/IW4L code is used.
"""
from __future__ import annotations

import copy
import hashlib
import json
import math
import os
import pathlib
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import vm_format as vf  # noqa: E402
import xanim_read  # noqa: E402
import convert_weapons  # noqa: E402  (WEAPONFILE splitter of M2)

ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"
ASSETS = DUMPS / "common_mp_assets"
GLB_HANDS = ASSETS / "model_export" / "viewmodel_base_viewhands_lod0.glb"
GLB_GUN = ASSETS / "model_export" / "viewmodel_ump45_lod0.glb"
MAT_DIR = ASSETS / "materials" / "mc"
IMG_DIR = ASSETS / "images"
XANIM_DIR = ASSETS / "xanim"
WEAPON_FILE = DUMPS / "common_mp" / "weapons" / "ump45_mp"
OUT = ROOT / "converted" / "viewmodel"
REPORT = ROOT / "work" / "m3" / "convert_report.json"
CONVERTER_VERSION = "convert_viewmodel 1.1"
XMODEL_DIR = ASSETS / "xmodel"
# OAT writes "type": "viewhands" only when the XModel's local `trans` array is all zero (HasNulledTrans in
# tools/oat/src/ObjWriting/XModel/XModelDumper.cpp.template, read only); "animated" = a non-null trans array.
EXPECT_XMODEL_TYPES = {"hands": ("viewmodel_base_viewhands", "viewhands"), "gun": ("viewmodel_ump45", "animated")}
PREFIX = "viewmodel_ump45_"

EXPECT_BONES = 91
EXPECT_GUN_TRIS = 2972
EXPECT_HAND_TRIS = 5442
EXPECT_XANIMS = 38
EXPECT_WHITELIST_HITS = 40
IBM_ROT_TOL = 1e-3
IBM_TRANS_TOL = 0.01      # inches; measured 4.5e-3 on finger joints (float chain), see MODLOG M3 deviations
WEAPON_ANIM_FIELDS = {"idle": "idleAnim", "fire": "fireAnim", "fire_ads": "adsFireAnim", "reload": "reloadAnim",
                      "reload_empty": "reloadEmptyAnim", "pullout": "raiseAnim", "putaway": "dropAnim",
                      "sprint_in": "sprintInAnim", "sprint_loop": "sprintLoopAnim", "sprint_out": "sprintOutAnim",
                      "ads_up": "adsUpAnim", "ads_down": "adsDownAnim"}
WEAPON_TIMES = ["fireTime", "reloadTime", "reloadEmptyTime", "raiseTime", "dropTime", "sprintInTime",
                "sprintLoopTime", "sprintOutTime", "adsTransInTime", "adsTransOutTime", "adsZoomFov"]
FINGER = ("index", "mid", "ring", "pinky", "thumb")
KNOWN_MOVERS = ("j_shoulder_", "tag_weapon", "tag_player")
# Known answers (docs/M3_DESIGN.md 12.1, from the mw2-rig finding).
J_GUN_VIEW_IDLE = (10.90, -2.62, -4.13)
TAG_ADS_END = (-4.238, 2.618, 1.025)
TAG_WEAPON_BIND = (10.528, 0.0, 51.87)


def whitelisted(name):
    if name.startswith("j_sleeveanim_"):
        return True
    for f in FINGER:
        for side in ("le", "ri"):
            if name == f"j_{f}_{side}_3":
                return True
    return False


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


# ------------------------------------------------------------------------------------------------ weapon
def load_weapon(path=WEAPON_FILE):
    text, _ = convert_weapons.read_text(path)
    pairs = dict(convert_weapons.parse_weaponfile(text))
    lower = {k.lower(): v for k, v in pairs.items()}
    w = {"gunModel": pairs["gunModel"], "handModel": pairs["handModel"],
         "hideTags": [t for t in pairs["hideTags"].split("\n") if t.strip()],
         "anims": {}, "times": {}}
    for clip, field in WEAPON_ANIM_FIELDS.items():
        w["anims"][clip] = lower[field.lower()].lower()
    for f in WEAPON_TIMES:
        w["times"][f] = float(lower[f.lower()])
    return w


# ------------------------------------------------------------------------------------------------ skeleton
def build_skeleton(hands, gun):
    """91 bones: hands skin joints, then gun skin joints; j_gun under tag_weapon; OAT root rotation dropped."""
    bones, problems, glb_ibm = [], [], []
    for part, g in (("hands", hands), ("gun", gun)):
        sk = g.json["skins"][0]
        joints = sk["joints"]
        jset = set(joints)
        base = len(bones)
        ibms = g.accessor(sk["inverseBindMatrices"])
        for k, ni in enumerate(joints):
            nd = g.nodes[ni]
            p = g.parent.get(ni)
            parent = base + joints.index(p) if (p is not None and p in jset) else -1
            t = list(nd.get("translation", [0.0, 0.0, 0.0]))
            q = list(nd.get("rotation", [0.0, 0.0, 0.0, 1.0]))
            if parent < 0:
                # The OAT root carries the -90 deg X turn (MW2 Z-up -> glTF Y-up); assert it, then drop it.
                if max(abs(a - b) for a, b in zip(q, [-0.7071068, 0.0, 0.0, 0.7071068])) > 1e-5:
                    problems.append(f"{part} root {nd['name']}: expected OAT quat (-0.7071,0,0,0.7071), got {q}")
                q = [0.0, 0.0, 0.0, 1.0]
            bones.append({"name": nd["name"], "parent": parent, "t": t, "q": q, "part": part})
            m = ibms[k]
            glb_ibm.append([[m[c * 4 + r] for c in range(4)] for r in range(4)])
    names = [b["name"] for b in bones]
    if len(set(names)) != len(names):
        problems.append("duplicate bone names in the combined skeleton")
    gun_root = next(i for i, b in enumerate(bones) if b["part"] == "gun" and b["parent"] < 0)
    if bones[gun_root]["name"] != "j_gun":
        problems.append(f"gun root is {bones[gun_root]['name']}, expected j_gun")
    bones[gun_root]["parent"] = names.index("tag_weapon")
    bones[gun_root]["t"] = [0.0, 0.0, 0.0]
    for i, b in enumerate(bones):
        if b["parent"] >= i:
            problems.append(f"bone {b['name']} appears before its parent")
    skel = {"bones": bones, "tags": {t: names.index(t) for t in vf.SKEL_TAGS}}
    world = vf.fk(skel, [(b["t"], b["q"]) for b in bones])
    r_inv = vf.m_inv_rigid(vf.R_OAT)
    rot_err = trans_err = 0.0
    for i, b in enumerate(bones):
        b["ibm"] = vf.m_inv_rigid(world[i])
        # GLB IBM = inverse(R_OAT . World_iw) = IBM_iw . R_OAT^-1 (gun: j_gun at the origin of its own GLB).
        w_local = world[i] if b["part"] == "hands" else vf.m_mul(vf.m_inv_rigid(world[gun_root]), world[i])
        mine = vf.m_mul(vf.m_inv_rigid(w_local), r_inv)
        g = glb_ibm[i]
        rot_err = max(rot_err, vf.m_maxdiff(mine, g, 3, 3))
        trans_err = max(trans_err, max(abs(mine[r][3] - g[r][3]) for r in range(3)))
    return skel, world, {"ibm_rot_err": rot_err, "ibm_trans_err": trans_err}, problems


# ------------------------------------------------------------------------------------------------ mesh
def _descendants(g, roots):
    out = set()
    stack = [i for i, nd in enumerate(g.nodes) if nd.get("name") in roots]
    while stack:
        i = stack.pop()
        if i in out:
            continue
        out.add(i)
        stack.extend(g.nodes[i].get("children", []))
    return out


def load_material(name):
    p = MAT_DIR / f"{name}.json"
    mj = json.loads(p.read_text(encoding="utf-8"))
    cm = [t["image"] for t in mj.get("textures", []) if t.get("name") == "colorMap"]
    return {"name": name, "color_map": cm[0] if cm else None,
            "blend": 1 if mj.get("cameraRegion") == "litTrans" else 0, "region": mj.get("cameraRegion")}


def build_mesh(skel, bind_world, hands, gun, hide_tags):
    """Gun vertices are in the gun GLB's own space (j_gun at its origin): they are moved into the combined model
    space by the bind world of j_gun, so that World_iw[b] . IBM_iw[b] is the identity at bind for every bone."""
    names = [b["name"] for b in skel["bones"]]
    verts, indices, surfaces, materials, textures, stats, problems = [], [], [], [], [], {}, []
    mat_index, tex_index = {}, {}
    for part_id, part, g, base_joint in ((1, "hands", hands, 0), (0, "gun", gun, names.index("j_gun"))):
        xf = bind_world[base_joint] if part == "gun" else vf.m_identity()
        sk = g.json["skins"][0]
        joints = sk["joints"]
        hidden_nodes = _descendants(g, set(hide_tags)) if part == "gun" else set()
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
                drawn += len(idx) // 3
                mname = g.json["materials"][pr["material"]]["name"].split("/")[-1]
                if mname not in mat_index:
                    m = load_material(mname)
                    if not m["color_map"]:
                        problems.append(f"material {mname} has no colorMap")
                        continue
                    if m["color_map"] not in tex_index:
                        tex_index[m["color_map"]] = len(textures)
                        textures.append(m["color_map"])
                    mat_index[mname] = len(materials)
                    materials.append({"name": mname, "texture": tex_index[m["color_map"]], "blend": m["blend"]})
                first = len(indices)
                for v in idx:
                    if v not in src["remap"]:
                        src["remap"][v] = len(verts)
                        verts.append(_vertex(src, v, base_joint, part_id, xf))
                    indices.append(src["remap"][v])
                surfaces.append({"first": first, "count": len(idx), "material": mat_index[mname], "part": part_id,
                                 "glb_surface": si})
        stats[f"{part}_tris"] = drawn
        stats[f"{part}_hidden_surfaces"] = hidden
    stats["vertices"] = len(verts)
    stats["drawn_surfaces"] = [(s["part"], s["glb_surface"]) for s in surfaces]
    if len(verts) >= 65536:
        problems.append(f"{len(verts)} vertices do not fit u16 indices")
    return {"verts": verts, "indices": indices, "surfaces": surfaces, "materials": materials, "textures": textures}, stats, problems


def _vertex(src, v, base_joint, part_id, xf):
    pos = vf.m_point(xf, vf.iw_from_glb(src["pos"][v]))
    n = vf.m_dir(xf, vf.iw_from_glb(src["nrm"][v]))
    ln = vf.v_len(n) or 1.0
    nrm = [max(-32767, min(32767, int(round(c / ln * 32767)))) for c in n] + [0]
    ws = [max(0.0, float(w)) for w in src["weights"][v]]
    js = [int(j) + base_joint for j in src["joints"][v]]
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
def xanim_files():
    return sorted(p for p in XANIM_DIR.iterdir() if p.name.startswith(PREFIX))


def parse_all_xanims(files):
    parsed, problems = {}, []
    for p in files:
        try:
            parsed[p.name] = xanim_read.read_xanim(str(p))
        except Exception as e:  # noqa: BLE001 - any parse failure is a check failure
            problems.append(f"1a {p.name}: {e}")
    return parsed, problems


def xmodel_types():
    """(types by part, problems): the OAT xmodel JSON `type` of the hands and gun models."""
    types, problems = {}, []
    for part, (model, want) in EXPECT_XMODEL_TYPES.items():
        try:
            t = json.loads((XMODEL_DIR / f"{model}.json").read_text(encoding="utf-8")).get("type")
        except (OSError, ValueError) as e:
            t = None
            problems.append(f"1m xmodel {model}.json: {e}")
        types[part] = t
        if t != want:
            problems.append(f"1m xmodel {model} type {t!r}, expected {want!r} (translation semantics depend on it)")
    return types, problems


def convert_clip(xa, clip_name, skel, gun_trans_delta=True):
    """Translation semantics (IW4 DObj, as iw4L fc3d8ea xmodel_runtime/src/dobj.rs composes it: local translation
    = XModel.trans[bone] + anim translation): the viewhands XModel has a nulled trans array, so its tracks are
    absolute local translations; the gun XModel ("animated") has real bind offsets, so its tracks are deltas and
    the bind local translation is added here (gun_trans_delta=False = the first M3 reading, kept for the
    self-test). Measured: tag_clip / j_bolt reload tracks start at |t| <= 0.002 in while their bind offsets are
    6.5 / 12.7 in."""
    names = {b["name"]: i for i, b in enumerate(skel["bones"])}
    tracks, dropped, unknown = [], [], []
    for b in xa["bones"]:
        if b["name"] not in names:
            (dropped if whitelisted(b["name"]) else unknown).append(b["name"])
            continue
        q, t = b["quat"], b["trans"]
        bone = skel["bones"][names[b["name"]]]
        base = bone["t"] if (gun_trans_delta and bone["part"] == "gun") else [0.0, 0.0, 0.0]
        tracks.append({
            "bone": names[b["name"]],
            "rot_frames": list(q["idx"]) if q else [],
            "rot": [[c / xanim_read.Q for c in k] for k in q["q"]] if q else [],
            "trans_frames": list(t["idx"]) if t else [],
            "trans": [[k[c] + base[c] for c in range(3)] for k in t["t"]] if t else []})
    tracks.sort(key=lambda tr: tr["bone"])
    clip = {"name": clip_name, "fps": xa["framerate"], "numframes": xa["numframes"], "looped": xa["looped"],
            "notes": [(n["name"], n["frame"]) for n in xa["notes"]], "tracks": tracks}
    return clip, dropped, unknown


def convert_anims(parsed, weapon, skel, gun_trans_delta=True):
    clips, problems, info = [], [], {"whitelist_hits": 0, "whitelist_by_clip": {}}
    for clip_name in vf.BASE_CLIPS:
        fname = weapon["anims"][clip_name]
        if fname != PREFIX + clip_name:
            problems.append(f"weapon file names {fname} for {clip_name}")
        xa = parsed.get(fname)
        if xa is None:
            problems.append(f"1a {fname} missing or unparsable")
            continue
        clip, dropped, unknown = convert_clip(xa, clip_name, skel, gun_trans_delta)
        if unknown:
            problems.append(f"1d {fname}: bones not in the skeleton nor whitelisted: {unknown}")
        info["whitelist_by_clip"][clip_name] = len(dropped)
        info["whitelist_hits"] += len(dropped)
        clips.append(clip)
    return clips, problems, info


# ------------------------------------------------------------------------------------------------ build
def build(paths=None):
    """Everything in memory: (bundle, problems). No files written."""
    problems = []
    weapon = load_weapon()
    hands, gun = vf.Glb(GLB_HANDS), vf.Glb(GLB_GUN)
    skel, bind_world, skel_stats, p = build_skeleton(hands, gun)
    problems += p
    mesh, mesh_stats, p = build_mesh(skel, bind_world, hands, gun, weapon["hideTags"])
    problems += p
    textures = {}
    for t in mesh["textures"]:
        try:
            textures[t] = vf.read_dds(IMG_DIR / f"{t}.dds")
        except (OSError, vf.FormatError) as e:
            problems.append(f"1k texture {t}: {e}")
    files = xanim_files()
    parsed, p = parse_all_xanims(files)
    problems += p
    clips, p, anim_info = convert_anims(parsed, weapon, skel)
    problems += p
    types, p = xmodel_types()
    problems += p
    glb_nodes = sorted({nd.get("name", "") for g in (hands, gun) for nd in g.nodes})
    return {"weapon": weapon, "skel": skel, "xmodel_types": types, "glb_node_names": glb_nodes, "bind_world": bind_world, "skel_stats": skel_stats, "mesh": mesh,
            "mesh_stats": mesh_stats, "textures": textures, "xanim_files": [f.name for f in files],
            "parsed": parsed, "clips": clips, "anim_info": anim_info}, problems


def serialize(b):
    skel_bytes = vf.write_skel(b["skel"]["bones"], b["skel"]["tags"])
    mesh_bytes = vf.write_mesh(b["mesh"]["verts"], b["mesh"]["indices"], b["mesh"]["surfaces"], b["mesh"]["materials"],
                               [f"{t}.vmtex" for t in b["mesh"]["textures"]])
    out = {"ump45.vmskel": skel_bytes, "ump45.vmmesh": mesh_bytes, "ump45.vmanim": vf.write_anim(b["clips"])}
    for t, dds in b["textures"].items():
        out[f"{t}.vmtex"] = vf.write_tex(dds)
    return out


def input_files(b):
    ins = [GLB_HANDS, GLB_GUN, WEAPON_FILE] + [XANIM_DIR / n for n in b["xanim_files"]]
    ins += [MAT_DIR / f"{m['name']}.json" for m in b["mesh"]["materials"]]
    ins += [IMG_DIR / f"{t}.dds" for t in b["mesh"]["textures"]]
    return ins


def make_manifest(files, b):
    return {
        "format_version": vf.FORMAT_VERSION,
        "converter": CONVERTER_VERSION,
        "files": {n: {"sha256": sha256_bytes(d), "size": len(d)} for n, d in sorted(files.items())},
        "inputs": {str(p.relative_to(ROOT)).replace("\\", "/"): {"sha256": sha256_bytes(p.read_bytes()), "size": p.stat().st_size}
                   for p in input_files(b)},
        "weapon_times": b["weapon"]["times"],
        "counts": {"bones": len(b["skel"]["bones"]), "vertices": len(b["mesh"]["verts"]),
                   "gun_tris": b["mesh_stats"]["gun_tris"], "hand_tris": b["mesh_stats"]["hands_tris"],
                   "clips": len(b["clips"])},
    }


def write_outputs(files, manifest):
    OUT.mkdir(parents=True, exist_ok=True)
    man = OUT / "manifest.json"
    if man.exists():
        man.unlink()  # a half-written conversion must have no manifest
    for n, d in files.items():
        tmp = OUT / (n + ".tmp")
        tmp.write_bytes(d)
        os.replace(tmp, OUT / n)
    for p in OUT.iterdir():  # stale outputs of an earlier conversion
        if p.is_file() and p.name not in files and p.name != "manifest.json":
            p.unlink()
    tmp = OUT / "manifest.json.tmp"
    tmp.write_text(json.dumps(manifest, indent=1, sort_keys=True), encoding="utf-8")
    os.replace(tmp, man)


def verify_manifest(out_dir=OUT):
    """Every listed file exists with its sha256/size; no unlisted file. Returns problems."""
    problems = []
    try:
        man = json.loads((out_dir / "manifest.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        return [f"manifest: {e}"]
    if man.get("format_version") != vf.FORMAT_VERSION:
        problems.append(f"manifest format_version {man.get('format_version')}")
    for n, meta in man.get("files", {}).items():
        p = out_dir / n
        if not p.is_file():
            problems.append(f"manifest lists missing {n}")
            continue
        d = p.read_bytes()
        if sha256_bytes(d) != meta["sha256"] or len(d) != meta["size"]:
            problems.append(f"manifest hash/size mismatch for {n}")
    extra = [p.name for p in out_dir.iterdir() if p.is_file() and p.name != "manifest.json" and p.name not in man.get("files", {})]
    if extra:
        problems.append(f"unlisted files in {out_dir}: {extra}")
    return problems


# ------------------------------------------------------------------------------------------------ oracle 1
def view_space(world, skel, i):
    """Position of bone i relative to tag_view, in MW2 axes (x fwd, y left, z up)."""
    v = vf.m_inv_rigid(world[skel["tags"]["tag_view"]])
    return vf.m_mul(v, world[i])


def check_all(b, files_bytes=None, translation_as_delta=False):
    """Returns (problems by check id, report numbers). `b` holds the in-memory conversion; when `files_bytes`
    is given, skeleton/mesh/anims/textures are re-read from those bytes (round trip)."""
    P = {k: [] for k in ("1a", "1b", "1c", "1d", "1f", "1g", "1h", "1i", "1j", "1k", "1m")}
    R = {}
    skel, mesh, clips, texs = b["skel"], b["mesh"], b["clips"], b["textures"]
    if files_bytes is not None:
        try:
            skel = vf.read_skel(files_bytes["ump45.vmskel"])
            mesh = vf.read_mesh(files_bytes["ump45.vmmesh"])
            clips = vf.read_anim(files_bytes["ump45.vmanim"])
            texs = {}
            for t in mesh["textures"]:
                if t not in files_bytes:
                    P["1k"].append(f"texture file {t} missing")
                    continue
                texs[t[:-len(".vmtex")]] = vf.read_tex(files_bytes[t])
        except (KeyError, vf.FormatError) as e:
            P["1a"].append(f"round trip: {e}")
            return P, R
    names = [x["name"] for x in skel["bones"]]
    cbn = {c["name"]: c for c in clips}

    # 1a: all 38 parse to exact EOF (Python reader).
    R["xanims"] = len(b["xanim_files"])
    if len(b["xanim_files"]) != EXPECT_XANIMS:
        P["1a"].append(f"{len(b['xanim_files'])} xanims, expected {EXPECT_XANIMS}")
    missing = [n for n in b["xanim_files"] if n not in b["parsed"]]
    if missing:
        P["1a"].append(f"unparsed: {missing}")

    # 1b: sanity of every parsed file; 12 base clips >= 1 frame; |q| - 1 <= 1e-4.
    qerr, zero = 0.0, []
    for n, xa in b["parsed"].items():
        if xa["framerate"] != 30:
            P["1b"].append(f"{n}: fps {xa['framerate']}")
        if xa["numframes"] == 0:
            zero.append(n)
        for bone in xa["bones"]:
            if bone["quat"]:
                for q in bone["quat"]["q"]:
                    qerr = max(qerr, abs(math.sqrt(sum(c * c for c in q)) / xanim_read.Q - 1.0))
    R["zero_frame_files"] = zero
    R["max_quat_norm_err"] = qerr
    if qerr > 1e-4:
        P["1b"].append(f"|q|-1 = {qerr:.2e} > 1e-4")
    durations = {}
    for c in clips:
        d = c["numframes"] / c["fps"] if c["fps"] else 0.0
        durations[c["name"]] = round(d, 4)
        if c["numframes"] < 1 or not (0.03 <= d <= 5.0):
            P["1b"].append(f"{c['name']}: {c['numframes']} frames, {d:.3f} s")
    R["durations"] = durations
    if set(cbn) != set(vf.BASE_CLIPS):
        P["1b"].append(f"clips {sorted(cbn)} != the 12 base clips")

    # 1c: reload durations vs the weapon file (+-10%); others reported.
    times = b["weapon"]["times"]
    for clip, field in (("reload", "reloadTime"), ("reload_empty", "reloadEmptyTime")):
        d, want = durations.get(clip, 0.0), times[field]
        if abs(d - want) > 0.10 * want:
            P["1c"].append(f"{clip} {d:.3f} s vs {field} {want}")
    R["vs_weapon_times"] = {c: {"clip_s": durations.get(c), "weapon_s": times[f]} for c, f in
                            (("reload", "reloadTime"), ("reload_empty", "reloadEmptyTime"), ("pullout", "raiseTime"),
                             ("putaway", "dropTime"), ("sprint_in", "sprintInTime"), ("sprint_loop", "sprintLoopTime"),
                             ("sprint_out", "sprintOutTime"))}

    # 1d: every track bone of every parsed file is in the skeleton or whitelisted.
    wl_all, wl_base, unknown = 0, 0, set()
    for n, xa in b["parsed"].items():
        for bone in xa["bones"]:
            if bone["name"] in names:
                continue
            if whitelisted(bone["name"]):
                wl_all += 1
                if n[len(PREFIX):] in vf.BASE_CLIPS:
                    wl_base += 1
            else:
                unknown.add(bone["name"])
    distinct = sorted({bone["name"] for xa in b["parsed"].values() for bone in xa["bones"]
                       if bone["name"] not in names})
    R["whitelist_hits_base_clips"] = wl_base
    R["whitelist_hits_all_files"] = wl_all
    R["whitelist_distinct_names"] = len(distinct)
    R["whitelist_dropped_names"] = distinct
    # The dropped names must really be absent from both GLBs (no node, so no skin joint and no vertex weight).
    glb_names = set(b.get("glb_node_names", []))
    in_glb = [n for n in distinct if n in glb_names]
    in_skel = [n for n in names if whitelisted(n)]
    if in_glb or in_skel:
        P["1d"].append(f"whitelisted (dropped) names present in the GLBs {in_glb} / skeleton {in_skel}")
    if unknown:
        P["1d"].append(f"bones neither in the skeleton nor whitelisted: {sorted(unknown)}")
    if len(distinct) != EXPECT_WHITELIST_HITS:
        P["1d"].append(f"{len(distinct)} distinct whitelisted names, expected {EXPECT_WHITELIST_HITS}")
    for c in clips:
        for tr in c["tracks"]:
            if not (0 <= tr["bone"] < len(names)):
                P["1d"].append(f"{c['name']}: track bone index {tr['bone']}")

    def pose_world(clip, t, ads=0.0):
        pose = vf.pose_for(skel, cbn, clip, t, ads)
        if translation_as_delta:  # self-test mutation: hands tracks read as bind + delta
            for tr in cbn[clip]["tracks"]:
                if tr["trans_frames"] and b["skel"]["bones"][tr["bone"]]["part"] == "hands":
                    i = tr["bone"]
                    pose[i] = ([pose[i][0][k] + skel["bones"][i]["t"][k] for k in range(3)], pose[i][1])
        return vf.fk(skel, pose)

    # 1f: idle ~ bind (median, never the max).
    idle = cbn.get("idle")
    if idle:
        diffs, movers = [], []
        for tr in idle["tracks"]:
            if not tr["trans_frames"]:
                continue
            nm = names[tr["bone"]]
            d = vf.v_len(vf.v_sub(tr["trans"][0], skel["bones"][tr["bone"]]["t"]))
            diffs.append(d)
            if d > 0.01 and nm.startswith(KNOWN_MOVERS):
                movers.append(f"{nm} {d:.2f}")
        med = statistics.median(diffs) if diffs else 1e9
        within = sum(1 for d in diffs if d <= 0.01)
        R["idle_vs_bind"] = {"tracks": len(diffs), "median_in": med, "within_0.01": within, "known_movers": movers}
        # Deviation (MODLOG M3): the design asked for >= 50 of the idle translation tracks within 0.01 in, but the
        # idle clip has 60 translation tracks of which 13 move by design: the 6 known movers and 7 sleeve-reshape
        # helpers (j_sleave_reshape_*, 0.13-0.9 in). So: median <= 0.01 in, >= 45 within 0.01 in, and every bone
        # beyond 0.01 in must be a known mover or a sleeve-reshape helper (no unexplained mover).
        unexplained = [names[tr["bone"]] for tr in idle["tracks"] if tr["trans_frames"]
                       and vf.v_len(vf.v_sub(tr["trans"][0], skel["bones"][tr["bone"]]["t"])) > 0.01
                       and not names[tr["bone"]].startswith(KNOWN_MOVERS + ("j_sleave_reshape_",))]
        R["idle_vs_bind"]["unexplained_movers"] = unexplained
        if med > 0.01 or within < 45 or unexplained:
            P["1f"].append(f"idle vs bind: median {med:.4f} in, {within} bones within 0.01 in, unexplained {unexplained}")

    # 1g: semantics known answers.
    try:
        w = pose_world("idle", 0.0)
        jg = view_space(w, skel, names.index("j_gun"))
        jgv = [jg[0][3], jg[1][3], jg[2][3]]
        R["j_gun_view_idle"] = [round(x, 3) for x in jgv]
        if max(abs(a - b) for a, b in zip(jgv, J_GUN_VIEW_IDLE)) > 0.05:
            P["1g"].append(f"j_gun in view space at idle {R['j_gun_view_idle']} != {J_GUN_VIEW_IDLE} +-0.05")
        up = cbn["ads_up"]
        tr = next(t for t in up["tracks"] if names[t["bone"]] == "tag_ads")
        ads_end = tr["trans"][-1]
        R["tag_ads_end"] = [round(x, 4) for x in ads_end]
        if max(abs(a - b) for a, b in zip(ads_end, TAG_ADS_END)) > 0.01:
            P["1g"].append(f"tag_ads at ADS end {R['tag_ads_end']} != {TAG_ADS_END} +-0.01")
        wa = pose_world("idle", 0.0, 1.0)
        pal = vf.palette(skel, wa)
        vinv = vf.m_inv_rigid(wa[skel["tags"]["tag_view"]])
        sight = None
        # The rear sight is part of gun surface 0 (the design said "surface 2", which is the front sight hood at
        # view x ~20 in): take the highest vertex of any gun surface in the view-x slab [5, 7] in.
        # Its top edge is a symmetric row of vertices (y = -0.255 .. +0.254 at the same height), so the lateral
        # test uses the centre of that top row (vertices within 0.005 in of the highest one).
        seen, slab = set(), []
        for s in [s for s in mesh["surfaces"] if s["part"] == 0]:
            for k in range(s["first"], s["first"] + s["count"]):
                vi = mesh["indices"][k]
                if vi in seen:
                    continue
                seen.add(vi)
                p = vf.m_point(vinv, vf.skin_point(pal, mesh["verts"][vi]))
                if 5.0 <= p[0] <= 7.0:
                    slab.append(p)
        if slab:
            top = max(p[2] for p in slab)
            row = [p for p in slab if p[2] >= top - 0.005]
            sight = [sum(p[0] for p in row) / len(row), sum(p[1] for p in row) / len(row), top]
            R["rear_sight_top_row_y"] = [round(min(p[1] for p in row), 3), round(max(p[1] for p in row), 3)]
        R["rear_sight_view_ads"] = [round(x, 3) for x in sight] if sight else None
        if sight is None or abs(sight[2]) > 0.1 or abs(sight[1]) > 0.25:
            P["1g"].append(f"rear sight at ADS end {R['rear_sight_view_ads']}: not within 0.1 in vertically / 0.25 in "
                           "laterally of the view axis")
    except (KeyError, StopIteration, ValueError, IndexError) as e:
        P["1g"].append(f"cannot evaluate: {e!r}")

    # 1h: loop closure and ADS continuity.
    sl = cbn.get("sprint_loop")
    if sl:
        err = 0.0
        for tr in sl["tracks"]:
            if len(tr["rot"]) > 1:
                a, z = tr["rot"][0], tr["rot"][-1]
                err = max(err, min(max(abs(x - y) for x, y in zip(a, z)), max(abs(x + y) for x, y in zip(a, z))))
            if len(tr["trans"]) > 1:
                err = max(err, max(abs(x - y) for x, y in zip(tr["trans"][0], tr["trans"][-1])))
        R["sprint_loop_closure"] = err
        if err > 1e-6:
            P["1h"].append(f"sprint_loop first != last key ({err:.2e})")
    try:
        dn = next(t for t in cbn["ads_down"]["tracks"] if names[t["bone"]] == "tag_ads")
        upt = next(t for t in cbn["ads_up"]["tracks"] if names[t["bone"]] == "tag_ads")
        d = vf.v_len(vf.v_sub(dn["trans"][0], upt["trans"][-1]))
        R["ads_down0_vs_ads_up_end"] = d
        if d > 0.01:
            P["1h"].append(f"ads_down frame 0 tag_ads differs from ads_up end by {d:.4f} in")
        others = [names[t["bone"]] for c in ("ads_up", "ads_down") for t in cbn[c]["tracks"] if names[t["bone"]] != "tag_ads"]
        if others:
            P["1h"].append(f"ADS clips drive more than tag_ads: {others}")
    except (KeyError, StopIteration) as e:
        P["1h"].append(f"ADS clips: {e!r}")

    # 1m: translation semantics per model (see convert_clip). Every gun-bone translation track must start and end
    # at the bind offset (the magazine / bolt sit in the gun before and after the reload) and move in between.
    R["xmodel_types"] = b.get("xmodel_types")
    gun_tracks = []
    for c in clips:
        for tr in c["tracks"]:
            bone = skel["bones"][tr["bone"]]
            if tr["trans_frames"] and b["skel"]["bones"][tr["bone"]]["part"] == "gun":
                d0 = vf.v_len(vf.v_sub(tr["trans"][0], bone["t"]))
                d1 = vf.v_len(vf.v_sub(tr["trans"][-1], bone["t"]))
                mx = max(vf.v_len(vf.v_sub(k, bone["t"])) for k in tr["trans"])
                gun_tracks.append(f"{c['name']}:{names[tr['bone']]} start {d0:.3f} end {d1:.3f} max {mx:.2f}")
                if d0 > 0.05 or d1 > 0.05 or mx < 0.5:
                    P["1m"].append(f"{c['name']} {names[tr['bone']]}: start {d0:.3f} in / end {d1:.3f} in from bind "
                                   f"(must be <= 0.05), max {mx:.2f} in (must move >= 0.5)")
    R["gun_translation_tracks"] = gun_tracks
    if not gun_tracks:
        P["1m"].append("no gun-bone translation track in the base clips (reload tag_clip expected)")

    # 1i: skeleton.
    R["bones"] = len(skel["bones"])
    if len(skel["bones"]) != EXPECT_BONES:
        P["1i"].append(f"{len(skel['bones'])} bones, expected {EXPECT_BONES}")
    if any(x["parent"] >= i for i, x in enumerate(skel["bones"])):
        P["1i"].append("parent after child")
    st = b["skel_stats"]
    R["ibm_rot_err"], R["ibm_trans_err_in"] = st["ibm_rot_err"], st["ibm_trans_err"]
    if st["ibm_rot_err"] > IBM_ROT_TOL or st["ibm_trans_err"] > IBM_TRANS_TOL:
        P["1i"].append(f"IBM vs GLB: rot {st['ibm_rot_err']:.2e} (tol {IBM_ROT_TOL}), trans {st['ibm_trans_err']:.2e} in "
                       f"(tol {IBM_TRANS_TOL})")
    bw = vf.fk(skel, [(x["t"], x["q"]) for x in skel["bones"]])
    tw = [[round(bw[names.index(n)][r][3], 3) for r in range(3)] for n in ("tag_weapon", "tag_weapon_left", "tag_weapon_right")]
    R["bind_tag_weapon"] = tw
    if any(max(abs(a - c) for a, c in zip(x, TAG_WEAPON_BIND)) > 0.01 for x in tw):
        P["1i"].append(f"bind tag_weapon/_left/_right {tw} not all at {TAG_WEAPON_BIND}")
    for i, x in enumerate(skel["bones"]):
        if vf.m_maxdiff(vf.m_mul(bw[i], x["ibm"]), vf.m_identity()) > 1e-4:
            P["1i"].append(f"{x['name']}: bind . IBM != I")
            break

    # 1j: mesh.
    gun_t = sum(s["count"] for s in mesh["surfaces"] if s["part"] == 0) // 3
    hand_t = sum(s["count"] for s in mesh["surfaces"] if s["part"] == 1) // 3
    R["gun_tris"], R["hand_tris"], R["vertices"] = gun_t, hand_t, len(mesh["verts"])
    if gun_t != EXPECT_GUN_TRIS or hand_t != EXPECT_HAND_TRIS:
        P["1j"].append(f"triangles gun {gun_t} hands {hand_t} != {EXPECT_GUN_TRIS}/{EXPECT_HAND_TRIS}")
    nv = len(mesh["verts"])
    if any(i >= nv for i in mesh["indices"]):
        P["1j"].append("index out of range")
    bad_w = sum(1 for v in mesh["verts"] if sum(v["weights"]) != 255)
    bad_j = sum(1 for v in mesh["verts"] if any(j >= len(names) for j in v["joints"]))
    nan = sum(1 for v in mesh["verts"] for x in v["pos"] + v["uv"] if not math.isfinite(x))
    if bad_w or bad_j or nan:
        P["1j"].append(f"{bad_w} weight sums != 255, {bad_j} joints >= {len(names)}, {nan} NaN")
    # bind-pose skinning reproduces the GLB positions (mesh conversion + IBM consistent)
    pal = vf.palette(skel, bw)
    err = max(vf.v_len(vf.v_sub(vf.skin_point(pal, v), v["pos"])) for v in mesh["verts"][::7])
    R["bind_skin_err_in"] = err
    if err > 0.01:
        P["1j"].append(f"bind-pose skinning moves vertices by {err:.4f} in")
    alpha = [m["name"] for m in mesh["materials"] if m["blend"] == 1]
    R["alpha_materials"] = alpha
    if alpha != ["mtl_us_army_alpha"]:
        P["1j"].append(f"blended materials {alpha} (expected only mtl_us_army_alpha)")

    # 1k: textures.
    R["textures"] = {}
    for m in mesh["materials"]:
        t = mesh["textures"][m["texture"]]
        key = t[:-len(".vmtex")] if t.endswith(".vmtex") else t
        tx = texs.get(key)
        if tx is None:
            P["1k"].append(f"colour map {t} of {m['name']} missing")
            continue
        fmt = {vf.DXGI_BC1_UNORM: "BC1", vf.DXGI_BC3_UNORM: "BC3"}.get(tx["format"])
        R["textures"][key] = f"{fmt} {tx['w']}x{tx['h']} mips={len(tx['mips'])}"
        full = int(math.log2(max(tx["w"], tx["h"]))) + 1
        if fmt is None or len(tx["mips"]) != full:
            P["1k"].append(f"{key}: format {tx['format']} mips {len(tx['mips'])}/{full}")
    return P, R


# ------------------------------------------------------------------------------------------------ CLI
def cmd_convert():
    b, problems = build()
    if problems:
        for p in problems:
            print("FAIL " + p)
        return 1
    files = serialize(b)
    man = make_manifest(files, b)
    write_outputs(files, man)
    mp = verify_manifest()
    P, R = check_all(b, files)
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps({"report": R, "problems": P, "mesh_stats": b["mesh_stats"],
                                  "anim_info": b["anim_info"], "manifest_counts": man["counts"]}, indent=1, default=str),
                      encoding="utf-8")
    total = sum(len(d) for d in files.values())
    for p in mp:
        print("FAIL " + p)
    print(f"converted {len(files)} files ({total} bytes) to {OUT}; bones={len(b['skel']['bones'])} "
          f"verts={len(b['mesh']['verts'])} gun_tris={b['mesh_stats']['gun_tris']} hand_tris={b['mesh_stats']['hands_tris']} "
          f"clips={len(b['clips'])} textures={len(b['textures'])}")
    print(("PASS" if not mp else "FAIL") + " convert: manifest.json written last, every sha256 verified")
    return 0 if not mp else 1


def cmd_check():
    b, problems = build()
    try:
        files = {p.name: p.read_bytes() for p in OUT.iterdir() if p.is_file() and p.name != "manifest.json"}
    except OSError as e:
        print(f"FAIL converted files: {e} (run the converter first)")
        return 1
    mp = verify_manifest()
    fresh = serialize(b)
    stale = [n for n, d in fresh.items() if files.get(n) != d]
    P, R = check_all(b, files)
    for p in problems:
        print("FAIL build: " + p)
    for p in mp:
        print("FAIL manifest: " + p)
    if stale:
        print(f"FAIL converted files differ from a fresh conversion: {stale}")
    for k in sorted(P):
        print(f"{'FAIL' if P[k] else 'PASS'} {k} " + ("; ".join(P[k]) if P[k] else ""))
    print("REPORT " + json.dumps(R, default=str))
    ok = not problems and not mp and not stale and not any(P.values())
    print(("PASS" if ok else "FAIL") + f" anim oracle: 1a(Python) 1b 1c 1d 1f 1g 1h 1i 1j 1k 1m "
          f"(reload {R.get('durations', {}).get('reload')} s, reload_empty {R.get('durations', {}).get('reload_empty')} s, "
          f"j_gun@idle {R.get('j_gun_view_idle')}, whitelist {R.get('whitelist_distinct_names')} dropped names absent "
          f"from both GLBs, gun translation tracks bind+delta: {R.get('gun_translation_tracks')})")
    return 0 if ok else 1


def cmd_self_test():
    """Oracle 1l: each deliberate mutation must make its check fail; the clean build must pass."""
    b, problems = build()
    if problems:
        print("FAIL self-test: clean build has problems: " + "; ".join(problems))
        return 1
    files = serialize(b)
    P, _ = check_all(b, files)
    if any(P.values()):
        print("FAIL self-test: clean conversion fails: " + json.dumps({k: v for k, v in P.items() if v}))
        return 1
    caught, missed = [], []

    def expect(label, check, Pm):
        (caught if Pm.get(check) else missed).append(label)

    # 1. a renamed bone in an anim -> 1d
    b1 = copy.deepcopy(b)
    b1["parsed"][PREFIX + "reload"]["bones"][3]["name"] = "j_renamed_bone"
    expect("renamed bone", "1d", check_all(b1, files)[0])
    # 2. hands translation read as delta (bind + track) -> 1g
    expect("hands translation as delta", "1g", check_all(b, files, translation_as_delta=True)[0])
    # 7. gun translation read as absolute (the first M3 reading) -> 1m
    b7 = copy.deepcopy(b)
    b7["clips"], _, _ = convert_anims(b7["parsed"], b7["weapon"], b7["skel"], gun_trans_delta=False)
    expect("gun translation as absolute", "1m", check_all(b7, serialize(b7))[0])
    # 8. a whitelisted (dropped) bone name present in a GLB -> 1d
    b8 = copy.deepcopy(b)
    b8["glb_node_names"] = b8["glb_node_names"] + ["j_index_le_3"]
    expect("dropped bone present in a GLB", "1d", check_all(b8, files)[0])
    # 3. a truncated xanim -> 1a (the reader must reject it)
    data = (XANIM_DIR / (PREFIX + "fire")).read_bytes()
    trunc = pathlib.Path(REPORT.parent / "selftest_truncated_xanim")
    trunc.parent.mkdir(parents=True, exist_ok=True)
    trunc.write_bytes(data[:-7])
    b3 = copy.deepcopy(b)
    parsed, p = parse_all_xanims([trunc])
    trunc.unlink()
    b3["parsed"].pop(PREFIX + "fire")
    b3["parsed"].update({k: v for k, v in parsed.items()})
    P3 = check_all(b3, files)[0]
    # caught only if the reader itself rejected the truncated bytes AND check 1a reports the file
    expect("truncated xanim", "1a", P3 if p else {})
    # 4. a missing texture -> 1k
    f4 = dict(files)
    f4.pop(next(n for n in f4 if n.endswith(".vmtex")))
    expect("missing texture", "1k", check_all(b, f4)[0])
    # 5. a wrong weight sum -> 1j
    b5 = copy.deepcopy(b)
    b5["mesh"]["verts"][10]["weights"] = [200, 0, 0, 0]
    expect("wrong weight sum", "1j", check_all(b5, serialize(b5))[0])
    # 6. a manifest hash mismatch -> verify_manifest (the Rust side has its own test in vm_assets)
    tmpdir = REPORT.parent / "selftest_manifest"
    tmpdir.mkdir(parents=True, exist_ok=True)
    for p in tmpdir.iterdir():
        p.unlink()
    small = {"ump45.vmskel": files["ump45.vmskel"]}
    man = {"format_version": 1, "files": {"ump45.vmskel": {"sha256": sha256_bytes(small["ump45.vmskel"]),
                                                           "size": len(small["ump45.vmskel"])}}}
    (tmpdir / "ump45.vmskel").write_bytes(small["ump45.vmskel"][:-1] + b"\x01")
    (tmpdir / "manifest.json").write_text(json.dumps(man), encoding="utf-8")
    (caught if verify_manifest(tmpdir) else missed).append("manifest hash mismatch")
    for p in tmpdir.iterdir():
        p.unlink()
    tmpdir.rmdir()
    if missed:
        print(f"FAIL self-test: mutations not caught: {missed}")
        return 1
    print(f"PASS self-test: {len(caught)} mutations caught ({', '.join(caught)})")
    return 0


def main(argv):
    if argv == ["--check"]:
        return cmd_check()
    if argv == ["--self-test"]:
        return cmd_self_test()
    if argv:
        print(__doc__)
        return 2
    return cmd_convert()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
