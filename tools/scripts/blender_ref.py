"""Blender 5.2 reference renders for the M3 silhouette oracle (docs/M3_DESIGN.md section 12.2).

Run headless (never opens a window, writes only to the --out folder):
  blender.exe -b --factory-startup --python tools\\scripts\\blender_ref.py -- --poses work\\m3\\poses.json
      [--out work\\m3] [--textured]

Independent of the Rust/D3D12 path on purpose:
  - it imports the ORIGINAL GLBs from dumps/ (Blender's own glTF importer and skinning), not our .vmmesh;
  - it deletes the gun primitives whose vertices are all weighted only to bones under the weapon file's
    hideTags (a reimplementation of the converter's rule; the gun triangle count must be 2,972);
  - the pose comes from a separate Python FK over the converted .vmskel/.vmanim (vm_format.py), applied to the
    imported armatures as world-space deltas: pose_bone.matrix = Arm^-1 . W_t . J_rest^-1 . Arm . bone.matrix_local,
    where J_rest is the joint's rest world from the GLB node TRS (OAT's root turn dropped, i.e. MW2 space);
    Blender's world after the glTF import equals MW2 space (importer (x,-z,y) . OAT (X,Z,-Y) = identity), which the
    self-checks below assert.
Self-checks (must pass before any render): (1) posing every bone to its own rest through that formula moves no
evaluated vertex by more than 0.01 in; (2) after posing, every bone head lies within 0.01 in of the Python FK.
Camera: at World[tag_view], looking along tag_view +X with +Z up, vertical FOV from MW2 cg_fov (4:3 Hor+), clip
0.1..1000, 1920x1080, Workbench, flat lighting, no AA, transparent film.
Outputs: ref_<pose>.png, ref_<pose>.mask (VMMK: u32 w, u32 h, u8 per pixel, alpha > 0.5) and blender_report.json.

M6: --gun <name> renders one arsenal gun instead (poses default <root>\\work\\m6\\vm\\<gun>\\poses.json from vmtest
--arsenal, outputs next to them): the gun GLB, rocketModel GLB (RPG) and hideTags come from the MW2 weapon file, the
pose from converted\\guns\\<gun>; the hideTags rule is applied per primitive (glTF mesh + material name; the Blender face count is asserted
equal to the GLB-side count); rocket bones rkt_<name> pose the imported rocket armature. Same camera and self-checks.
"""
import json
import math
import os
import struct
import sys

import bpy
import numpy as np
from mathutils import Matrix

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import vm_format as vf  # noqa: E402

ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
GLB_HANDS = os.path.join(ROOT, "dumps", "common_mp_assets", "model_export", "viewmodel_base_viewhands_lod0.glb")
GLB_GUN = os.path.join(ROOT, "dumps", "common_mp_assets", "model_export", "viewmodel_ump45_lod0.glb")
CONVERTED = os.path.join(ROOT, "converted", "viewmodel")
HIDE_TAGS = ["tag_acog_2", "tag_eotech", "tag_thermal_scope", "tag_red_dot", "tag_heartbeat", "tag_silencer"]
EXPECT_GUN_TRIS = 2972


def args():
    a = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    out = {"poses": os.path.join(ROOT, "work", "m3", "poses.json"), "out": os.path.join(ROOT, "work", "m3"),
           "textured": False}
    out["gun"] = None
    it = iter(a)
    poses_set = out_set = False
    for x in it:
        if x == "--poses":
            out["poses"] = next(it)
            poses_set = True
        elif x == "--out":
            out["out"] = next(it)
            out_set = True
        elif x == "--textured":
            out["textured"] = True
        elif x == "--gun":
            out["gun"] = next(it)
    if out["gun"]:
        # M6: vmtest --arsenal writes <root>\work\m6\vm\<gun>\poses.json next to its masks
        gdir = os.path.join(ROOT, "work", "m6", "vm", out["gun"])
        if not poses_set:
            out["poses"] = os.path.join(gdir, "poses.json")
        if not out_set:
            out["out"] = gdir
    return out


def M(m4):
    return Matrix([list(r) for r in m4])


def rest_worlds(glb_path):
    """name -> rest world (MW2 space) from the GLB node TRS, OAT root rotation dropped."""
    g = vf.Glb(glb_path)
    sk = g.json["skins"][0]
    joints = sk["joints"]
    jset = set(joints)
    world = {}
    for ni in joints:  # skin order is parent-before-child (asserted by the converter)
        nd = g.nodes[ni]
        t = nd.get("translation", [0.0, 0.0, 0.0])
        q = nd.get("rotation", [0.0, 0.0, 0.0, 1.0])
        p = g.parent.get(ni)
        if p is None or p not in jset:
            q = [0.0, 0.0, 0.0, 1.0]
            world[nd["name"]] = vf.m_trs(t, q)
        else:
            world[nd["name"]] = vf.m_mul(world[g.nodes[p]["name"]], vf.m_trs(t, q))
    return world, g


def import_glb(path):
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=path)
    new = [o for o in bpy.data.objects if o not in before]
    arm = [o for o in new if o.type == "ARMATURE"]
    # skinned surfaces only: the importer also creates a bone-display shape (an unparented Icosphere)
    meshes = [o for o in new if o.type == "MESH" and any(m.type == "ARMATURE" for m in o.modifiers)]
    for o in new:
        if o.type == "MESH" and o not in meshes:
            o.hide_render = True
    if len(arm) != 1:
        raise SystemExit(f"FAIL {path}: expected one armature, got {[o.name for o in arm]}")
    return arm[0], meshes


def hidden_bone_names(g):
    names = set()
    stack = [i for i, nd in enumerate(g.nodes) if nd.get("name") in HIDE_TAGS]
    while stack:
        i = stack.pop()
        if g.nodes[i]["name"] in names:
            continue
        names.add(g.nodes[i]["name"])
        stack.extend(g.nodes[i].get("children", []))
    return names


def drop_hidden_gun_surfaces(meshes, hidden):
    kept_tris = 0
    removed = 0
    for o in list(meshes):
        me = o.data
        groups = {vg.index: vg.name for vg in o.vertex_groups}
        used = set()
        for p in me.polygons:
            used.update(p.vertices)
        only_hidden = all(all(groups[ge.group] in hidden for ge in me.vertices[v].groups if ge.weight > 0.0) for v in used)
        if only_hidden:
            bpy.data.objects.remove(o, do_unlink=True)
            meshes.remove(o)
            removed += 1
        else:
            kept_tris += sum(len(p.vertices) - 2 for p in me.polygons)
    return kept_tris, removed


def evaluated_world_verts(objs):
    dg = bpy.context.evaluated_depsgraph_get()
    out = []
    for o in objs:
        eo = o.evaluated_get(dg)
        me = eo.to_mesh()
        mw = eo.matrix_world
        out.append(np.array([tuple(mw @ v.co) for v in me.vertices], dtype=np.float64))
        eo.to_mesh_clear()
    return out


def apply_pose(arms, order, target_world, rest):
    """target_world/rest: bone name -> 4x4 list (MW2 = Blender world)."""
    for name in order:
        arm = arms[name]
        pb = arm.pose.bones[name]
        bone = arm.data.bones[name]
        A = arm.matrix_world
        D = M(target_world[name]) @ M(rest[name]).inverted()
        pb.matrix = A.inverted() @ D @ A @ bone.matrix_local
        bpy.context.view_layer.update()


def main():
    a = args()
    a["out"] = os.path.abspath(a["out"])  # Blender resolves relative render paths itself; be explicit
    os.makedirs(a["out"], exist_ok=True)
    poses = json.load(open(a["poses"], encoding="utf-8"))
    report = {"poses": {}, "self_checks": {}}

    bpy.ops.wm.read_factory_settings(use_empty=True)
    rest_h, _gh = rest_worlds(GLB_HANDS)
    rest_g, gg = rest_worlds(GLB_GUN)
    arm_h, mesh_h = import_glb(GLB_HANDS)
    arm_g, mesh_g = import_glb(GLB_GUN)
    gun_tris, removed = drop_hidden_gun_surfaces(mesh_g, hidden_bone_names(gg))
    # Blender's importer drops triangles with a repeated vertex index; the GLB's drawn gun surfaces have
    # exactly such degenerate triangles, so the Blender face count is 2,972 minus them.
    degenerate = 0
    for mi in range(len(gg.json["meshes"])):
        if mi >= 3:
            break
        idx = gg.accessor(gg.json["meshes"][mi]["primitives"][0]["indices"])
        degenerate += sum(1 for k in range(0, len(idx), 3) if len(set(idx[k:k + 3])) < 3)
    report["gun_tris_blender"] = gun_tris
    report["gun_degenerate_dropped_by_importer"] = degenerate
    report["gun_surfaces_removed"] = removed
    if gun_tris + degenerate != EXPECT_GUN_TRIS or removed != len(gg.json["meshes"]) - 3:
        print(f"FAIL gun triangles after the hideTags rule: {gun_tris} (+{degenerate} degenerate) != {EXPECT_GUN_TRIS}, "
              f"{removed} surfaces removed")
        return 1
    meshes = mesh_h + mesh_g
    for o in meshes:
        o.hide_render = False

    skel = vf.read_skel(open(os.path.join(CONVERTED, "ump45.vmskel"), "rb").read())
    clips = {c["name"]: c for c in vf.read_anim(open(os.path.join(CONVERTED, "ump45.vmanim"), "rb").read())}
    names = [b["name"] for b in skel["bones"]]
    rest = {}
    arms = {}
    for n in names:
        if n in rest_h:
            rest[n], arms[n] = rest_h[n], arm_h
        else:
            rest[n], arms[n] = rest_g[n], arm_g
    missing = [n for n in names if n not in arms[n].pose.bones]
    if missing:
        print(f"FAIL bones missing in the imported armatures: {missing}")
        return 1

    # Self-check 1: the posing path at rest moves nothing.
    base = evaluated_world_verts(meshes)
    apply_pose(arms, names, rest, rest)
    moved = max(float(np.abs(x - y).max()) for x, y in zip(base, evaluated_world_verts(meshes)))
    report["self_checks"]["rest_through_formula_max_move_in"] = moved
    if moved > 0.01:
        print(f"FAIL self-check: posing to rest through the formula moved a vertex by {moved:.4f} in")
        return 1
    print(f"self-check 1 PASS: rest pose through the formula moves vertices by at most {moved:.2e} in")

    # Camera
    cam_data = bpy.data.cameras.new("vmcam")
    cam = bpy.data.objects.new("vmcam", cam_data)
    bpy.context.scene.collection.objects.link(cam)
    sc = bpy.context.scene
    sc.camera = cam
    cam_data.sensor_fit = "VERTICAL"
    cam_data.sensor_height = 24.0
    cam_data.clip_start = poses.get("near_in", 0.1)
    cam_data.clip_end = poses.get("far_in", 1000.0)
    sc.render.engine = "BLENDER_WORKBENCH"
    sc.render.resolution_x = poses["width"]
    sc.render.resolution_y = poses["height"]
    sc.render.resolution_percentage = 100
    sc.render.pixel_aspect_x = sc.render.pixel_aspect_y = 1.0
    sc.render.film_transparent = True
    sc.display.render_aa = "OFF"
    sc.display.shading.light = "FLAT"
    sc.display.shading.color_type = "TEXTURE" if a["textured"] else "SINGLE"
    sc.display.shading.single_color = (1.0, 1.0, 1.0)
    sc.view_settings.view_transform = "Standard"
    sc.render.image_settings.file_format = "PNG"
    sc.render.image_settings.color_mode = "RGBA"
    # camera local axes -> tag_view axes: -Z_cam = +X (forward), +Y_cam = +Z (up), +X_cam = -Y (right)
    cam_in_tag = Matrix(((0.0, 0.0, -1.0, 0.0), (-1.0, 0.0, 0.0, 0.0), (0.0, 1.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0)))

    head_err_max = 0.0
    tv = skel["tags"]["tag_view"]
    for p in poses["poses"]:
        pose = vf.pose_for(skel, clips, p["clip"], max(0.0, p["t"]), p["ads"])
        world = vf.fk(skel, pose)
        target = {n: world[i] for i, n in enumerate(names)}
        apply_pose(arms, names, target, rest)
        # Self-check 2: bone heads where the Python FK put them.
        err = 0.0
        for i, n in enumerate(names):
            arm = arms[n]
            head = (arm.matrix_world @ arm.pose.bones[n].matrix).translation
            err = max(err, max(abs(head[k] - world[i][k][3]) for k in range(3)))
        head_err_max = max(head_err_max, err)
        if err > 0.01:
            print(f"FAIL self-check 2 ({p['name']}): bone head off the FK by {err:.4f} in")
            return 1
        fov = poses["hip_fov"] + (poses["ads_fov"] - poses["hip_fov"]) * p["ads"]
        vfov = 2.0 * math.atan(math.tan(math.radians(fov) / 2.0) * 0.75)
        cam_data.lens = 12.0 / math.tan(vfov / 2.0)
        cam.matrix_world = M(world[tv]) @ cam_in_tag
        bpy.context.view_layer.update()
        png = os.path.join(a["out"], f"ref_{p['name']}{'_tex' if a['textured'] else ''}.png")
        sc.render.filepath = png
        bpy.ops.render.render(write_still=True)
        if not os.path.isfile(png):
            print(f"FAIL render did not write {png}")
            return 1
        img = bpy.data.images.load(png, check_existing=False)
        w, h = img.size
        px = np.empty(w * h * 4, dtype=np.float32)
        img.pixels.foreach_get(px)
        bpy.data.images.remove(img)
        alpha = px.reshape(h, w, 4)[::-1, :, 3]  # Blender rows start at the bottom
        mask = (alpha > 0.5).astype(np.uint8)
        if not a["textured"]:
            with open(os.path.join(a["out"], f"ref_{p['name']}.mask"), "wb") as f:
                f.write(b"VMMK" + struct.pack("<II", w, h) + mask.tobytes())
        report["poses"][p["name"]] = {"coverage": int(mask.sum()), "vfov_deg": math.degrees(vfov), "head_err_in": err}
        print(f"ref {p['name']:<18} coverage {int(mask.sum()):>7} px  vfov {math.degrees(vfov):.3f}  head err {err:.2e} in")
    report["self_checks"]["max_bone_head_err_in"] = head_err_max
    with open(os.path.join(a["out"], "blender_report.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1)
    print(f"PASS blender_ref: gun tris {gun_tris} + {degenerate} degenerate = {EXPECT_GUN_TRIS} ({removed} hidden surfaces "
          f"removed); self-checks: rest move "
          f"{moved:.2e} in, bone heads within {head_err_max:.2e} in; {len(poses['poses'])} reference masks")
    return 0


# ------------------------------------------------------------------------------------------------ M6: one arsenal gun
def gun_inputs(gun):
    """(gun GLB, rocket GLB or None, hideTags) of an arsenal gun, from its MW2 weapon file (as convert_arsenal.py)."""
    import convert_weapons  # noqa: E402 - plain-Python parser shared with the converters
    import pathlib  # noqa: E402
    path = pathlib.Path(ROOT) / "dumps" / "common_mp" / "weapons" / gun
    text, _ = convert_weapons.read_text(path)
    kv = {k.lower(): v for k, v in dict(convert_weapons.parse_weaponfile(text)).items()}
    mdir = os.path.join(ROOT, "dumps", "common_mp_assets", "model_export")
    files = {f.lower(): os.path.join(mdir, f) for f in os.listdir(mdir)}
    gm = (kv.get("gunmodel") or "").strip().lower()
    rm = (kv.get("rocketmodel") or "").strip().lower()
    gp = files.get(gm + "_lod0.glb")
    if not gp:
        raise SystemExit(f"FAIL {gun}: gun model {gm} GLB missing")
    rp = files.get(rm + "_lod0.glb") if rm else None
    hide = [t.strip() for t in (kv.get("hidetags") or "").split("\n") if t.strip()]
    return gp, rp, hide


def glb_kept_tris(g, hidden):
    """GLB side of the converter's per-primitive hideTags rule: (kept tris, of them degenerate or repeated, the hidden
    primitives as (mesh name, material name))."""
    joints = g.json["skins"][0]["joints"]
    hidden_joints = {k for k, ni in enumerate(joints) if g.nodes[ni].get("name") in hidden}
    kept = degen = 0
    prims = set()
    for mi, mesh in enumerate(g.json["meshes"]):
        for pr in mesh["primitives"]:
            at = pr["attributes"]
            jt, wt, idx = g.accessor(at["JOINTS_0"]), g.accessor(at["WEIGHTS_0"]), g.accessor(pr["indices"])
            if all(all(w <= 0.0 or j in hidden_joints for j, w in zip(jt[v], wt[v])) for v in set(idx)):
                # Blender's glTF importer names an unnamed mesh Mesh_<index>
                prims.add((mesh.get("name") or f"Mesh_{mi}", g.json["materials"][pr["material"]]["name"]))
                continue
            kept += len(idx) // 3
            # Blender's importer drops degenerate triangles and repeats of a triangle (mg4: 12 doubled faces)
            tris = [tuple(idx[k:k + 3]) for k in range(0, len(idx), 3)]
            good = [t for t in tris if len(set(t)) == 3]
            degen += len(tris) - len({frozenset(t) for t in good})
    return kept, degen, prims


def _base(name):
    """Blender's datablock name without a .001 duplicate suffix."""
    return name[:-4] if len(name) > 4 and name[-4] == "." and name[-3:].isdigit() else name


def drop_hidden_polys(meshes, prims):
    """Deletes the faces of the hidden primitives (glTF mesh name + material name); returns the kept face count."""
    import bmesh  # noqa: E402
    kept = 0
    for o in meshes:
        mats = [_base(sl.material.name) if sl.material else "" for sl in o.material_slots]
        mname = _base(o.data.name)
        bm = bmesh.new()
        bm.from_mesh(o.data)
        drop = [f for f in bm.faces if f.material_index < len(mats) and (mname, mats[f.material_index]) in prims]
        if drop:
            bmesh.ops.delete(bm, geom=drop, context="FACES")
        bm.to_mesh(o.data)
        kept += sum(len(f.verts) - 2 for f in bm.faces)
        bm.free()
        o.data.update()
    return kept


def main_gun(a):
    gun = a["gun"]
    a["out"] = os.path.abspath(a["out"])
    os.makedirs(a["out"], exist_ok=True)
    poses = json.load(open(a["poses"], encoding="utf-8"))
    gdir = os.path.join(ROOT, "converted", "guns", gun)
    man = json.load(open(os.path.join(gdir, "manifest.json"), encoding="utf-8"))
    report = {"gun": gun, "poses": {}, "self_checks": {}}
    gp, rp, hide = gun_inputs(gun)

    bpy.ops.wm.read_factory_settings(use_empty=True)
    glb_hands = os.path.join(ROOT, "dumps", "common_mp_assets", "model_export", "viewmodel_base_viewhands_lod0.glb")
    rest_h, _ = rest_worlds(glb_hands)
    rest_g, gg = rest_worlds(gp)
    arm_h, mesh_h = import_glb(glb_hands)
    arm_g, mesh_g = import_glb(gp)
    rest_r, arm_r, mesh_r = {}, None, []
    if rp:
        rest_r, _ = rest_worlds(rp)
        arm_r, mesh_r = import_glb(rp)
    hidden = set()
    # hideTags subtree names (the converter's cv._descendants)
    stack = [i for i, nd in enumerate(gg.nodes) if nd.get("name") in set(hide)]
    while stack:
        i = stack.pop()
        if gg.nodes[i]["name"] in hidden:
            continue
        hidden.add(gg.nodes[i]["name"])
        stack.extend(gg.nodes[i].get("children", []))
    glb_kept, glb_degen, hidden_prims = glb_kept_tris(gg, hidden)
    blender_tris = drop_hidden_polys(mesh_g, hidden_prims)
    report.update({"gun_tris_blender": blender_tris, "gun_tris_glb_kept": glb_kept, "gun_tris_glb_degenerate": glb_degen,
                   "gun_tris_converted": man["counts"]["gun_tris"], "hide_tags": hide})
    # Blender drops degenerate triangles; the converter also skips surfaces without a colour map (counted in the
    # manifest), so only the hideTags rule itself is asserted here
    if blender_tris + glb_degen != glb_kept:
        print(f"FAIL {gun}: gun triangles after the hideTags rule: Blender {blender_tris} + {glb_degen} degenerate/repeated != GLB {glb_kept}")
        return 1
    meshes = mesh_h + mesh_g + mesh_r
    for o in meshes:
        o.hide_render = False

    skel = vf.read_skel(open(os.path.join(gdir, man.get("skel_file", "gun.vmskel")), "rb").read())
    by_name = {c["name"]: c for c in vf.read_anim(open(os.path.join(gdir, man.get("anim_file", "gun.vmanim")), "rb").read())}
    clips = {role: by_name[x] for role, x in man["roles"].items() if x in by_name}
    names = [b["name"] for b in skel["bones"]]
    rest, arms, bname = {}, {}, {}
    for n in names:
        if n.startswith("rkt_") and arm_r is not None and n[4:] in rest_r:
            rest[n], arms[n], bname[n] = rest_r[n[4:]], arm_r, n[4:]
        elif n in rest_h:
            rest[n], arms[n], bname[n] = rest_h[n], arm_h, n
        elif n in rest_g:
            rest[n], arms[n], bname[n] = rest_g[n], arm_g, n
        else:
            print(f"FAIL {gun}: bone {n} in no imported armature")
            return 1

    def pose_bones(target):
        for n in names:
            arm = arms[n]
            pb = arm.pose.bones[bname[n]]
            bone = arm.data.bones[bname[n]]
            A = arm.matrix_world
            D = M(target[n]) @ M(rest[n]).inverted()
            pb.matrix = A.inverted() @ D @ A @ bone.matrix_local
            bpy.context.view_layer.update()

    base = evaluated_world_verts(meshes)
    pose_bones(rest)
    moved = max((float(np.abs(x - y).max()) for x, y in zip(base, evaluated_world_verts(meshes)) if x.size), default=0.0)
    report["self_checks"]["rest_through_formula_max_move_in"] = moved
    if moved > 0.01:
        print(f"FAIL {gun} self-check: posing to rest through the formula moved a vertex by {moved:.4f} in")
        return 1

    cam_data = bpy.data.cameras.new("vmcam")
    cam = bpy.data.objects.new("vmcam", cam_data)
    bpy.context.scene.collection.objects.link(cam)
    sc = bpy.context.scene
    sc.camera = cam
    cam_data.sensor_fit = "VERTICAL"
    cam_data.sensor_height = 24.0
    cam_data.clip_start = poses.get("near_in", 0.1)
    cam_data.clip_end = poses.get("far_in", 1000.0)
    sc.render.engine = "BLENDER_WORKBENCH"
    sc.render.resolution_x = poses["width"]
    sc.render.resolution_y = poses["height"]
    sc.render.resolution_percentage = 100
    sc.render.film_transparent = True
    sc.display.render_aa = "OFF"
    sc.display.shading.light = "FLAT"
    sc.display.shading.color_type = "SINGLE"
    sc.view_settings.view_transform = "Standard"
    sc.render.image_settings.file_format = "PNG"
    sc.render.image_settings.color_mode = "RGBA"
    cam_in_tag = Matrix(((0.0, 0.0, -1.0, 0.0), (-1.0, 0.0, 0.0, 0.0), (0.0, 1.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0)))
    tv = skel["tags"]["tag_view"]
    head_err_max = 0.0
    for p in poses["poses"]:
        pose = vf.pose_for(skel, clips, p["clip"], max(0.0, p["t"]), p["ads"])
        world = vf.fk(skel, pose)
        target = {n: world[i] for i, n in enumerate(names)}
        pose_bones(target)
        err = 0.0
        for i, n in enumerate(names):
            arm = arms[n]
            head = (arm.matrix_world @ arm.pose.bones[bname[n]].matrix).translation
            err = max(err, max(abs(head[k] - world[i][k][3]) for k in range(3)))
        head_err_max = max(head_err_max, err)
        if err > 0.01:
            print(f"FAIL {gun} self-check 2 ({p['name']}): bone head off the FK by {err:.4f} in")
            return 1
        fov = poses["hip_fov"] + (poses["ads_fov"] - poses["hip_fov"]) * p["ads"]
        vfov = 2.0 * math.atan(math.tan(math.radians(fov) / 2.0) * 0.75)
        cam_data.lens = 12.0 / math.tan(vfov / 2.0)
        cam.matrix_world = M(world[tv]) @ cam_in_tag
        bpy.context.view_layer.update()
        png = os.path.join(a["out"], f"ref_{p['name']}.png")
        sc.render.filepath = png
        bpy.ops.render.render(write_still=True)
        img = bpy.data.images.load(png, check_existing=False)
        w, h = img.size
        px = np.empty(w * h * 4, dtype=np.float32)
        img.pixels.foreach_get(px)
        bpy.data.images.remove(img)
        mask = (px.reshape(h, w, 4)[::-1, :, 3] > 0.5).astype(np.uint8)
        with open(os.path.join(a["out"], f"ref_{p['name']}.mask"), "wb") as f:
            f.write(b"VMMK" + struct.pack("<II", w, h) + mask.tobytes())
        report["poses"][p["name"]] = {"coverage": int(mask.sum()), "vfov_deg": math.degrees(vfov), "head_err_in": err}
        print(f"ref {gun} {p['name']:<11} coverage {int(mask.sum()):>7} px  head err {err:.2e} in")
    report["self_checks"]["max_bone_head_err_in"] = head_err_max
    with open(os.path.join(a["out"], "blender_report.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1)
    print(f"PASS blender_ref --gun {gun}: hideTags rule Blender {blender_tris} + {glb_degen} degenerate/repeated = GLB {glb_kept} "
          f"(converted {man['counts']['gun_tris']}{', rocket part' if rp else ''}); rest move {moved:.2e} in, bone heads "
          f"within {head_err_max:.2e} in; {len(poses['poses'])} reference masks")
    return 0


if __name__ == "__main__":
    try:
        code = main_gun(args()) if args()["gun"] else main()
    except Exception as e:  # noqa: BLE001 - any failure must give a non-zero exit in -b mode
        import traceback
        traceback.print_exc()
        print(f"FAIL blender_ref: {e!r}")
        code = 1
    sys.stdout.flush()
    os._exit(code)
