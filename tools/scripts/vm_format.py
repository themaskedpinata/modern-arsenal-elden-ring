"""Shared helpers for the M3 viewmodel pipeline (docs/M3_DESIGN.md sections 2-3).

Pure Python (no numpy) so it runs both in Python 3.14 (-I) and inside Blender 5.2. It contains:
  - small rigid-transform math (quaternions xyzw, 4x4 row-major matrices, column vectors);
  - a GLB reader (JSON chunk + BIN chunk + accessors);
  - the DDS header reader for the BC1/BC3 colour maps;
  - writers AND readers of the runtime formats in converted/viewmodel/ (.vmskel, .vmmesh, .vmanim, .vmtex),
    each with the 32-byte versioned header;
  - clip sampling + forward kinematics with the M3 animation semantics (a separate implementation from the Rust
    one in mw2er-core::vm_pose, used by the oracles and by blender_ref.py).

Animation semantics (section 3.1): a translation track replaces the bind translation; no translation track keeps
the bind translation; a bone listed in a clip without a quat track gets identity rotation; bones absent from a
clip keep the full bind. ADS is a tag_ads-only layer.

No OAT or IW4L code is used here.
"""
from __future__ import annotations

import json
import math
import struct

FORMAT_VERSION = 1
HEADER_SIZE = 32
MAGIC_SKEL, MAGIC_MESH, MAGIC_ANIM, MAGIC_TEX = b"VMSK", b"VMME", b"VMAN", b"VMTX"
VERTEX_STRIDE = 40  # 12 pos + 8 normal + 8 uv + 4 joints + 4 weights + 4 colour (the design said 36: arithmetic slip)
SKEL_TAGS = ["tag_view", "tag_ads", "tag_weapon", "j_gun", "tag_flash", "tag_sight_on", "j_wrist_ri", "j_wrist_le"]
DXGI_BC1_UNORM, DXGI_BC3_UNORM = 71, 77
BASE_CLIPS = ["idle", "fire", "fire_ads", "reload", "reload_empty", "pullout", "putaway",
              "sprint_in", "sprint_loop", "sprint_out", "ads_up", "ads_down"]

# MW2 space (X fwd, Y left, Z up) -> GLB space written by OAT: (X, Y, Z) -> (X, Z, -Y).
R_OAT = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]]


class FormatError(Exception):
    pass


# ---------------------------------------------------------------------------------------------- math
def q_norm(q):
    n = math.sqrt(sum(c * c for c in q))
    return [c / n for c in q] if n > 1e-12 else [0.0, 0.0, 0.0, 1.0]


def q_mul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return [aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz]


def q_dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def q_mat3(q):
    x, y, z, w = q
    return [[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]]


def m_trs(t, q):
    r = q_mat3(q)
    return [r[0] + [t[0]], r[1] + [t[1]], r[2] + [t[2]], [0.0, 0.0, 0.0, 1.0]]


def m_mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(4)) for j in range(4)] for i in range(4)]


def m_inv_rigid(m):
    r = [[m[j][i] for j in range(3)] for i in range(3)]
    t = [-sum(r[i][k] * m[k][3] for k in range(3)) for i in range(3)]
    return [r[0] + [t[0]], r[1] + [t[1]], r[2] + [t[2]], [0.0, 0.0, 0.0, 1.0]]


def m_identity():
    return [[1.0 if i == j else 0.0 for j in range(4)] for i in range(4)]


def m_point(m, p):
    return [m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3] for i in range(3)]


def m_dir(m, d):
    return [m[i][0] * d[0] + m[i][1] * d[1] + m[i][2] * d[2] for i in range(3)]


def m_maxdiff(a, b, rows=4, cols=4):
    return max(abs(a[i][j] - b[i][j]) for i in range(rows) for j in range(cols))


def v_sub(a, b):
    return [a[i] - b[i] for i in range(3)]


def v_len(a):
    return math.sqrt(sum(c * c for c in a))


def iw_from_glb(v):
    """GLB (x, y, z) -> MW2 (x, -z, y): the inverse of R_OAT."""
    return [v[0], -v[2], v[1]]


# ---------------------------------------------------------------------------------------------- GLB
_CT = {5126: ("f", 4), 5123: ("H", 2), 5121: ("B", 1), 5125: ("I", 4), 5122: ("h", 2), 5120: ("b", 1)}
_NC = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


class Glb:
    def __init__(self, path):
        d = open(path, "rb").read()
        if d[:4] != b"glTF":
            raise FormatError(f"{path}: not a GLB")
        n = struct.unpack_from("<I", d, 12)[0]
        self.json = json.loads(d[20:20 + n])
        off = 20 + n
        bl = struct.unpack_from("<I", d, off)[0]
        self.bin = d[off + 8:off + 8 + bl]
        self.nodes = self.json["nodes"]
        self.parent = {}
        for i, nd in enumerate(self.nodes):
            for c in nd.get("children", []):
                self.parent[c] = i

    def accessor(self, i):
        a = self.json["accessors"][i]
        bv = self.json["bufferViews"][a["bufferView"]]
        f, s = _CT[a["componentType"]]
        nc = _NC[a["type"]]
        st = bv.get("byteStride", s * nc)
        o = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        fmt = "<" + f * nc
        out = []
        for k in range(a["count"]):
            v = struct.unpack_from(fmt, self.bin, o + k * st)
            out.append(list(v) if nc > 1 else v[0])
        return out


# ---------------------------------------------------------------------------------------------- DDS
def read_dds(path):
    d = open(path, "rb").read()
    if d[:4] != b"DDS " or len(d) < 128:
        raise FormatError(f"{path}: not a DDS file")
    h, w, _pitch, _depth, mips = struct.unpack_from("<IIIII", d, 12)
    fcc = d[84:88]
    if fcc == b"DXT1":
        fmt, bs = DXGI_BC1_UNORM, 8
    elif fcc == b"DXT5":
        fmt, bs = DXGI_BC3_UNORM, 16
    else:
        raise FormatError(f"{path}: unsupported FourCC {fcc!r} (colour maps must be DXT1/DXT5)")
    mips = max(1, mips)
    if w & (w - 1) or h & (h - 1):
        raise FormatError(f"{path}: {w}x{h} is not a power of two")
    full = int(math.log2(max(w, h))) + 1
    if mips != full:
        raise FormatError(f"{path}: {mips} mips, a complete chain has {full}")
    levels, o = [], 128
    for m in range(mips):
        mw, mh = max(1, w >> m), max(1, h >> m)
        bw, bh = (mw + 3) // 4, (mh + 3) // 4
        size = bw * bh * bs
        if o + size > len(d):
            raise FormatError(f"{path}: truncated at mip {m}")
        levels.append({"row_pitch": bw * bs, "rows": bh, "data": d[o:o + size]})
        o += size
    if o != len(d):
        raise FormatError(f"{path}: {len(d) - o} trailing bytes after the mip chain")
    return {"format": fmt, "w": w, "h": h, "mips": levels}


# ---------------------------------------------------------------------------------------------- headers
def header(magic, count, payload_len):
    return magic + struct.pack("<IIIQII", FORMAT_VERSION, HEADER_SIZE, 0, payload_len, count, 0)


def parse_header(data, magic):
    if len(data) < HEADER_SIZE:
        raise FormatError("file shorter than the 32-byte header")
    if data[:4] != magic:
        raise FormatError(f"magic {data[:4]!r} != {magic!r}")
    ver, hs, flags, plen, count, res = struct.unpack_from("<IIIQII", data, 4)
    if ver != FORMAT_VERSION:
        raise FormatError(f"format version {ver} != {FORMAT_VERSION}")
    if hs != HEADER_SIZE or flags != 0 or res != 0:
        raise FormatError("bad header fields")
    if plen != len(data) - HEADER_SIZE:
        raise FormatError(f"payload size {plen} != file size - 32 ({len(data) - HEADER_SIZE})")
    return count


class Cur:
    def __init__(self, d, o=HEADER_SIZE):
        self.d, self.o = d, o

    def take(self, n):
        if self.o + n > len(self.d):
            raise FormatError("truncated")
        b = self.d[self.o:self.o + n]
        self.o += n
        return b

    def un(self, fmt):
        s = struct.calcsize(fmt)
        return struct.unpack("<" + fmt, self.take(s))

    def u8(self):
        return self.un("B")[0]

    def u16(self):
        return self.un("H")[0]

    def u32(self):
        return self.un("I")[0]

    def name(self):
        n = self.u8()
        return self.take(n).decode("utf-8")

    def end(self):
        if self.o != len(self.d):
            raise FormatError(f"{len(self.d) - self.o} trailing bytes")


def _name(s):
    b = s.encode("utf-8")
    if len(b) > 255:
        raise FormatError(f"name too long: {s}")
    return struct.pack("<B", len(b)) + b


# ---------------------------------------------------------------------------------------------- vmskel
def write_skel(bones, tags):
    """bones: list of {name, parent, t, q, ibm (3x4 rows)}; tags: name -> index for SKEL_TAGS."""
    p = bytearray()
    for b in bones:
        p += _name(b["name"]) + struct.pack("<h", b["parent"]) + struct.pack("<3f", *b["t"]) + struct.pack("<4f", *b["q"])
        p += struct.pack("<12f", *[b["ibm"][r][c] for r in range(3) for c in range(4)])
    for t in SKEL_TAGS:
        p += struct.pack("<H", tags[t])
    return header(MAGIC_SKEL, len(bones), len(p)) + bytes(p)


def read_skel(data):
    n = parse_header(data, MAGIC_SKEL)
    c = Cur(data)
    bones = []
    for i in range(n):
        name = c.name()
        parent = c.un("h")[0]
        t = list(c.un("3f"))
        q = list(c.un("4f"))
        f = c.un("12f")
        if parent >= i:
            raise FormatError(f"bone {name}: parent {parent} not before it")
        bones.append({"name": name, "parent": parent, "t": t, "q": q,
                      "ibm": [list(f[0:4]), list(f[4:8]), list(f[8:12]), [0.0, 0.0, 0.0, 1.0]]})
    tags = {t: c.u16() for t in SKEL_TAGS}
    c.end()
    return {"bones": bones, "tags": tags}


# ---------------------------------------------------------------------------------------------- vmmesh
def write_mesh(verts, indices, surfaces, materials, textures):
    """verts: list of {pos, nrm(i16x4), uv, joints, weights, color}; surfaces: {first, count, material, part};
    materials: {name, texture, blend}; textures: list of file names."""
    p = bytearray()
    for v in verts:
        p += struct.pack("<3f4h2f4B4B4B", *v["pos"], *v["nrm"], *v["uv"], *v["joints"], *v["weights"], *v["color"])
    p += struct.pack("<I", len(indices)) + struct.pack(f"<{len(indices)}H", *indices)
    p += struct.pack("<H", len(surfaces))
    for s in surfaces:
        p += struct.pack("<IIHB", s["first"], s["count"], s["material"], s["part"])
    p += struct.pack("<H", len(materials))
    for m in materials:
        p += _name(m["name"]) + struct.pack("<HB", m["texture"], m["blend"])
    p += struct.pack("<H", len(textures))
    for t in textures:
        p += _name(t)
    return header(MAGIC_MESH, len(verts), len(p)) + bytes(p)


def read_mesh(data):
    n = parse_header(data, MAGIC_MESH)
    c = Cur(data)
    verts = []
    raw = c.take(n * VERTEX_STRIDE)
    for i in range(n):
        f = struct.unpack_from("<3f4h2f4B4B4B", raw, i * VERTEX_STRIDE)
        verts.append({"pos": list(f[0:3]), "nrm": list(f[3:7]), "uv": list(f[7:9]), "joints": list(f[9:13]),
                      "weights": list(f[13:17]), "color": list(f[17:21])})
    ni = c.u32()
    indices = list(c.un(f"{ni}H")) if ni else []
    surfaces = []
    for _ in range(c.u16()):
        first, count, mat, part = c.un("IIHB")
        surfaces.append({"first": first, "count": count, "material": mat, "part": part})
    materials = []
    for _ in range(c.u16()):
        name = c.name()
        tex, blend = c.un("HB")
        materials.append({"name": name, "texture": tex, "blend": blend})
    textures = [c.name() for _ in range(c.u16())]
    c.end()
    return {"verts": verts, "indices": indices, "surfaces": surfaces, "materials": materials, "textures": textures}


# ---------------------------------------------------------------------------------------------- vmanim
def write_anim(clips):
    """clips: list of {name, fps, numframes, looped, notes[(name, frame)], tracks[{bone, rot_frames, rot,
    trans_frames, trans}]}."""
    p = bytearray()
    for cl in clips:
        p += _name(cl["name"]) + struct.pack("<HHB", cl["fps"], cl["numframes"], 1 if cl["looped"] else 0)
        p += struct.pack("<B", len(cl["notes"]))
        for nm, fr in cl["notes"]:
            p += _name(nm) + struct.pack("<H", fr)
        p += struct.pack("<H", len(cl["tracks"]))
        for tr in cl["tracks"]:
            nr, nt = len(tr["rot_frames"]), len(tr["trans_frames"])
            p += struct.pack("<HH", tr["bone"], nr) + struct.pack(f"<{nr}H", *tr["rot_frames"])
            for q in tr["rot"]:
                p += struct.pack("<4f", *q)
            p += struct.pack("<H", nt) + struct.pack(f"<{nt}H", *tr["trans_frames"])
            for t in tr["trans"]:
                p += struct.pack("<3f", *t)
    return header(MAGIC_ANIM, len(clips), len(p)) + bytes(p)


def read_anim(data):
    n = parse_header(data, MAGIC_ANIM)
    c = Cur(data)
    clips = []
    for _ in range(n):
        name = c.name()
        fps, nf, looped = c.un("HHB")
        notes = []
        for _ in range(c.u8()):
            nm = c.name()
            notes.append((nm, c.u16()))
        tracks = []
        for _ in range(c.u16()):
            bone, nr = c.un("HH")
            rf = list(c.un(f"{nr}H")) if nr else []
            rot = [list(c.un("4f")) for _ in range(nr)]
            nt = c.u16()
            tf = list(c.un(f"{nt}H")) if nt else []
            tr = [list(c.un("3f")) for _ in range(nt)]
            tracks.append({"bone": bone, "rot_frames": rf, "rot": rot, "trans_frames": tf, "trans": tr})
        clips.append({"name": name, "fps": fps, "numframes": nf, "looped": bool(looped), "notes": notes,
                      "tracks": tracks})
    c.end()
    return clips


# ---------------------------------------------------------------------------------------------- vmtex
def write_tex(dds):
    p = bytearray(struct.pack("<IIII", dds["format"], dds["w"], dds["h"], len(dds["mips"])))
    for m in dds["mips"]:
        p += struct.pack("<III", m["row_pitch"], m["rows"], len(m["data"])) + m["data"]
    return header(MAGIC_TEX, len(dds["mips"]), len(p)) + bytes(p)


def read_tex(data):
    n = parse_header(data, MAGIC_TEX)
    c = Cur(data)
    fmt, w, h, mips = c.un("IIII")
    if mips != n:
        raise FormatError("mip count mismatch")
    levels = []
    for _ in range(mips):
        rp, rows, size = c.un("III")
        if rp * rows != size:
            raise FormatError("mip size != row_pitch * rows")
        levels.append({"row_pitch": rp, "rows": rows, "data": c.take(size)})
    c.end()
    return {"format": fmt, "w": w, "h": h, "mips": levels}


# ---------------------------------------------------------------------------------------------- sampling + FK
def _span(frames, f):
    if not frames or f <= frames[0]:
        return 0, 0, 0.0
    if f >= frames[-1]:
        k = len(frames) - 1
        return k, k, 0.0
    lo, hi = 0, len(frames) - 1
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if frames[mid] <= f:
            lo = mid
        else:
            hi = mid
    a, b = frames[lo], frames[hi]
    return lo, hi, (f - a) / (b - a) if b > a else 0.0


def clip_frame(clip, t, rate=1.0):
    nf = clip["numframes"]
    f = t * clip["fps"] * rate
    if clip["looped"] and nf > 0:
        return f % nf
    return min(max(f, 0.0), float(nf))


def sample_track(tr, f):
    """(rot or None, trans or None) of one track at frame f."""
    rot = trans = None
    if tr["rot_frames"]:
        a, b, u = _span(tr["rot_frames"], f)
        qa, qb = tr["rot"][a], tr["rot"][b]
        rot = q_norm([qa[k] + u * (qb[k] - qa[k]) for k in range(4)])
    if tr["trans_frames"]:
        a, b, u = _span(tr["trans_frames"], f)
        ta, tb = tr["trans"][a], tr["trans"][b]
        trans = [ta[k] + u * (tb[k] - ta[k]) for k in range(3)]
    return rot, trans


def local_pose(skel, clip, f, base=None):
    """Local (t, q) per bone for `clip` at frame f, starting from `base` (default: bind)."""
    pose = [(list(b["t"]), list(b["q"])) for b in skel["bones"]] if base is None else [(list(t), list(q)) for t, q in base]
    for tr in clip["tracks"]:
        rot, trans = sample_track(tr, f)
        t0, _ = pose[tr["bone"]]
        pose[tr["bone"]] = (trans if trans is not None else t0, rot if rot is not None else [0.0, 0.0, 0.0, 1.0])
    return pose


def fk(skel, pose):
    world = []
    for i, b in enumerate(skel["bones"]):
        t, q = pose[i]
        m = m_trs(t, q)
        world.append(m if b["parent"] < 0 else m_mul(world[b["parent"]], m))
    return world


def pose_for(skel, clips_by_name, clip, t, ads_frac=0.0):
    """Local pose for (clip at t) plus the ADS layer (ads_up sampled at frac * duration), as in vm_pose: every bone
    the ADS clip drives changes (the UMP45's drive only tag_ads; M6: the RPG's also drive tag_torso)."""
    c = clips_by_name[clip]
    pose = local_pose(skel, c, clip_frame(c, t))
    if ads_frac > 0.0:
        up = clips_by_name["ads_up"]
        f = min(max(ads_frac, 0.0), 1.0) * up["numframes"]
        pose = local_pose(skel, up, f, base=pose)
    return pose


def palette(skel, world):
    return [m_mul(world[i], b["ibm"]) for i, b in enumerate(skel["bones"])]


def skin_point(pal, v):
    """Skinned MW2-space position of a decoded vertex (weights in unorm8)."""
    out = [0.0, 0.0, 0.0]
    for j, w in zip(v["joints"], v["weights"]):
        if w:
            p = m_point(pal[j], v["pos"])
            for k in range(3):
                out[k] += p[k] * (w / 255.0)
    return out
