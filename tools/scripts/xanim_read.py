"""Reader for the compiled XAnim files OAT's Unlinker writes (dumps/.../xanim/<name>).

Independent Python implementation of the file layout (version 17/18, little endian):
  u16 version (17 = IW3-compatible, 18 = IW4 with 3D delta quat)
  u16 numFrames   (looped: numframes, non-looped: numframes + 1  -> "loop frames")
  u16 boneCount
  u8  flags       (1 = looped, 2 = delta, 4 = delta3D [v18 only])
  u8  assetType
  u16 framerate
  [delta part]    if flags & (2|4): quat track (3D if delta3D else 2D) + trans track
  [bones]         flipQuat bitmask, halfQuat bitmask (ceil(n/8) bytes each), n C-strings,
                  then per bone: quat track, trans track
  u8 notetrack count, then (C-string name, u16 frame) each

Quats are int16 / 32767; the stored track omits the last component (w for full quats,
w for half quats which are (0,0,z,w)), reconstructed as +sqrt(32767^2 - sum sq) and then
sign-continuity corrected. Values are LOCAL rotations (they replace the bind rotation);
translations are local translations (see the bind-vs-absolute check in the CLI report).

Usage (CLI): python -I xanim_read.py <xanim file or dir> [--glb a.glb --glb b.glb] [--json out]
"""
from __future__ import annotations

import json
import math
import os
import struct
import sys

HALF_TRANS_SIZE_SCALE = 0.003921568859368563  # 1/255 as the IW linker writes it
FULL_TRANS_SIZE_SCALE = 0.00001525902189314365  # 1/65535
Q = 32767.0


class Reader:
    def __init__(self, data: bytes):
        self.d = data
        self.o = 0

    def u8(self):
        v = self.d[self.o]
        self.o += 1
        return v

    def u16(self):
        v = struct.unpack_from("<H", self.d, self.o)[0]
        self.o += 2
        return v

    def i16(self):
        v = struct.unpack_from("<h", self.d, self.o)[0]
        self.o += 2
        return v

    def f32(self):
        v = struct.unpack_from("<f", self.d, self.o)[0]
        self.o += 4
        return v

    def cstr(self):
        e = self.d.index(b"\0", self.o)
        s = self.d[self.o:e].decode("ascii")
        self.o = e + 1
        return s

    def eof(self):
        return self.o == len(self.d)


def _w(sumsq: int) -> int:
    t = 0x3FFF0001 - sumsq
    return 0 if t <= 0 else int(math.floor(math.sqrt(float(t)) + 0.5))


def _full(r: Reader):
    x, y, z = r.i16(), r.i16(), r.i16()
    return [x, y, z, _w(x * x + y * y + z * z)]


def _half(r: Reader):
    z = r.i16()
    return [0, 0, z, _w(z * z)]


def _indices(r: Reader, n: int, byte_idx: bool, loop_frames: int):
    if n >= loop_frames:
        return list(range(n))
    if byte_idx:
        return [r.u8() for _ in range(n)]
    return [r.u16() for _ in range(n)]


def _continuity(frames, first_flip: bool):
    out = []
    for i, q in enumerate(frames):
        if i == 0:
            if first_flip:
                q = [-c for c in q]
        else:
            p = out[-1]
            if sum(a * b for a, b in zip(p, q)) < 0:
                q = [-c for c in q]
        out.append(q)
    return out


def read_quat_track(r: Reader, loop_frames, byte_idx, flip, half, delta=False):
    n = r.u16()
    if n == 0:
        return None
    rd = _half if half else _full
    if n == 1:
        q = rd(r)
        if flip:
            q = [-c for c in q]
        return {"kind": "half_const" if half else "full_const", "idx": [0], "q": [q]}
    idx = _indices(r, n, byte_idx, loop_frames)
    frames = [rd(r) for _ in range(n)]
    frames = _continuity(frames, flip)
    return {"kind": "half" if half else "full", "idx": idx, "q": frames}


def read_trans_track(r: Reader, loop_frames, byte_idx):
    n = r.u16()
    if n == 0:
        return None
    if n == 1:
        return {"kind": "const", "idx": [0], "t": [[r.f32(), r.f32(), r.f32()]]}
    idx = _indices(r, n, byte_idx, loop_frames)
    small = r.u8() != 0
    mins = [r.f32(), r.f32(), r.f32()]
    size = [r.f32() * (HALF_TRANS_SIZE_SCALE if small else FULL_TRANS_SIZE_SCALE) for _ in range(3)]
    t = []
    for _ in range(n):
        v = [r.u8(), r.u8(), r.u8()] if small else [r.u16(), r.u16(), r.u16()]
        t.append([mins[k] + size[k] * v[k] for k in range(3)])
    return {"kind": "small" if small else "full", "idx": idx, "t": t}


def read_xanim(path: str) -> dict:
    data = open(path, "rb").read()
    r = Reader(data)
    version = r.u16()
    if version not in (17, 18):
        raise ValueError(f"{path}: unsupported version {version}")
    nf_raw = r.u16()
    bone_count = r.u16()
    flags = r.u8()
    asset_type = r.u8()
    framerate = r.u16()
    looped = bool(flags & 1)
    has_delta = bool(flags & 2)
    has_delta3d = version == 18 and bool(flags & 4)
    loop_frames = nf_raw + 1 if looped else nf_raw
    numframes = loop_frames - 1
    byte_idx = numframes < 256
    delta = None
    if has_delta or has_delta3d:
        dq = read_quat_track(r, loop_frames, byte_idx, False, not has_delta3d, True)
        dt = read_trans_track(r, loop_frames, byte_idx)
        delta = {"quat": dq, "trans": dt}
    bones = []
    if bone_count:
        nb = (bone_count + 7) // 8
        flip = r.d[r.o:r.o + nb]
        r.o += nb
        half = r.d[r.o:r.o + nb]
        r.o += nb
        names = [r.cstr() for _ in range(bone_count)]
        for i, name in enumerate(names):
            fl = bool(flip[i >> 3] & (1 << (i & 7)))
            hf = bool(half[i >> 3] & (1 << (i & 7)))
            q = read_quat_track(r, loop_frames, byte_idx, fl, hf)
            t = read_trans_track(r, loop_frames, byte_idx)
            bones.append({"name": name, "quat": q, "trans": t})
    notes = []
    nn = r.u8()
    for _ in range(nn):
        name = r.cstr()
        fr = r.u16()
        notes.append({"name": name, "frame": fr, "time": fr / numframes if numframes else 0.0})
    if not r.eof():
        raise ValueError(f"{path}: {len(data) - r.o} trailing bytes")
    return {
        "name": os.path.basename(path),
        "version": version,
        "numframes": numframes,
        "looped": looped,
        "framerate": framerate,
        "asset_type": asset_type,
        "flags": flags,
        "delta": delta,
        "bones": bones,
        "notes": notes,
        "duration": numframes / framerate if framerate else 0.0,
    }


def _norm(q):
    l = math.sqrt(sum(c * c for c in q))
    return [c / l for c in q] if l > 1e-9 else [0.0, 0.0, 0.0, 1.0]


def _span(idx, frame):
    if len(idx) <= 1 or frame <= idx[0]:
        return 0, 0, 0.0
    if frame >= idx[-1]:
        return len(idx) - 1, len(idx) - 1, 0.0
    for i in range(len(idx) - 1):
        if frame <= idx[i + 1]:
            a, b = idx[i], idx[i + 1]
            return i, i + 1, (frame - a) / (b - a) if b > a else 0.0
    return len(idx) - 1, len(idx) - 1, 0.0


def sample(track, key, frame):
    """IW4-style sampling: linear (n)lerp between keyed frames."""
    if track is None:
        return None
    a, b, t = _span(track["idx"], frame)
    va, vb = track[key][a], track[key][b]
    v = [va[k] + t * (vb[k] - va[k]) for k in range(len(va))]
    return _norm([c / Q for c in v]) if key == "q" else v


# ---------------------------------------------------------------- GLB helpers
def glb_skeleton(path: str):
    d = open(path, "rb").read()
    n = struct.unpack_from("<I", d, 12)[0]
    j = json.loads(d[20:20 + n])
    nodes = j["nodes"]
    joints = set(j["skins"][0]["joints"]) if j.get("skins") else set()
    parent = {}
    for i, nd in enumerate(nodes):
        for c in nd.get("children", []):
            parent[c] = i
    out = {}
    for i in sorted(joints):
        nd = nodes[i]
        p = parent.get(i)
        out[nd["name"]] = {
            "parent": nodes[p]["name"] if p is not None and p in joints else None,
            "t": nd.get("translation", [0.0, 0.0, 0.0]),
            "r": nd.get("rotation", [0.0, 0.0, 0.0, 1.0]),
        }
    return out


def qangle(a, b):
    d = abs(sum(x * y for x, y in zip(a, b)))
    return math.degrees(2 * math.acos(min(1.0, d)))


def main(argv):
    paths, glbs, js = [], [], None
    it = iter(argv)
    for a in it:
        if a == "--glb":
            glbs.append(next(it))
        elif a == "--json":
            js = next(it)
        else:
            paths.append(a)
    files = []
    for p in paths:
        if os.path.isdir(p):
            files += [os.path.join(p, f) for f in sorted(os.listdir(p))]
        else:
            files.append(p)
    skel = {}
    for g in glbs:
        skel.update(glb_skeleton(g))
    results = []
    for f in files:
        a = read_xanim(f)
        missing = [b["name"] for b in a["bones"] if skel and b["name"] not in skel]
        kinds = {}
        for b in a["bones"]:
            k = (b["quat"]["kind"] if b["quat"] else "noquat") + "/" + (b["trans"]["kind"] if b["trans"] else "notrans")
            kinds[k] = kinds.get(k, 0) + 1
        print(f"{a['name']}: v{a['version']} frames={a['numframes']} fps={a['framerate']} dur={a['duration']:.3f}s "
              f"loop={int(a['looped'])} delta={'y' if a['delta'] else 'n'} bones={len(a['bones'])} "
              f"notes={[(n['name'], n['frame']) for n in a['notes']]}" + (f" MISSING={missing}" if missing else ""))
        results.append({k: v for k, v in a.items() if k not in ("bones", "delta")} | {"missing": missing, "kinds": kinds,
                        "bone_names": [b["name"] for b in a["bones"]]})
    if js:
        json.dump(results, open(js, "w"), indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
