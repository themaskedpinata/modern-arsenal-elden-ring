"""M1 oracle: sanity-check dumped MW2 assets (sounds and models) before any conversion.

Usage: py -3.14 -I tools/scripts/check_assets.py <dumps_dir> [name_filter ...]

- WAV: format tag, channels, rate, bits, duration. Non-PCM (tag != 1) needs a decoder before ER can use it.
- GLB: bounding box of all POSITION accessors, reported in the file's units and in metres assuming inches
  (IW4 unit = 1 inch, unverified until a known-size model checks out).
"""
import json
import pathlib
import struct
import sys

INCH = 0.0254


def wav_info(path):
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        return {"error": "not RIFF/WAVE"}
    pos, fmt, data_len = 12, None, None
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack_from("<I", data, pos + 4)[0]
        body = data[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            tag, ch, rate, byte_rate, align, bits = struct.unpack_from("<HHIIHH", body)
            fmt = {"tag": tag, "channels": ch, "rate": rate, "byte_rate": byte_rate, "bits": bits}
        elif cid == b"data":
            data_len = size
        pos += 8 + size + (size & 1)
    if not fmt or data_len is None:
        return {"error": "missing fmt or data chunk"}
    fmt["seconds"] = round(data_len / fmt["byte_rate"], 3) if fmt["byte_rate"] else None
    return fmt


def glb_bounds(path):
    data = path.read_bytes()
    magic, version, _ = struct.unpack_from("<4sII", data, 0)
    if magic != b"glTF":
        return {"error": "not GLB"}
    json_len = struct.unpack_from("<I", data, 12)[0]
    gltf = json.loads(data[20:20 + json_len])
    lo, hi = [float("inf")] * 3, [float("-inf")] * 3
    for mesh in gltf.get("meshes", []):
        for prim in mesh.get("primitives", []):
            acc = gltf["accessors"][prim["attributes"]["POSITION"]]
            if "min" in acc and "max" in acc:
                lo = [min(a, b) for a, b in zip(lo, acc["min"])]
                hi = [max(a, b) for a, b in zip(hi, acc["max"])]
    if lo[0] == float("inf"):
        return {"error": "no POSITION min/max"}
    size = [h - l for h, l in zip(hi, lo)]
    return {
        "size_units": [round(s, 2) for s in size],
        "longest_m_if_inches": round(max(size) * INCH, 3),
        "nodes": len(gltf.get("nodes", [])),
        "skins": len(gltf.get("skins", [])),
        "animations": len(gltf.get("animations", [])),
    }


def main():
    root = pathlib.Path(sys.argv[1])
    filters = [f.lower() for f in sys.argv[2:]]
    keep = lambda p: not filters or any(f in p.name.lower() for f in filters)
    bad = 0
    for p in sorted(root.rglob("*.wav")):
        if keep(p):
            info = wav_info(p)
            bad += "error" in info or info.get("tag") != 1
            print("WAV", p.relative_to(root), info)
    for p in sorted(root.rglob("*.glb")):
        if keep(p):
            info = glb_bounds(p)
            bad += "error" in info
            print("GLB", p.relative_to(root), info)
    print(f"problems: {bad}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
