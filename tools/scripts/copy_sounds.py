"""Copy the MW2 gun sounds M2 uses into converted/sounds/ and write converted/sounds/sounds.json.

Usage: python -I copy_sounds.py [--dumps DIR] [--out DIR]

Sources are the user's own sound dumps (dumps/common_sounds/sound/..., SP common.ff, MODLOG M1). Each file
is copied byte-for-byte to converted/sounds/<gun>/<same name>, then checked: RIFF/WAVE, PCM (format tag 1),
44100 Hz, 16-bit, and destination SHA-256 == source SHA-256. sounds.json paths are relative to converted/
(forward slashes). converted/ is gitignored; nothing is committed or redistributed.
Exit code 0 = all sounds copied and valid, 1 = any problem. Pure Python.
"""
import hashlib
import json
import os
import pathlib
import shutil
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # -I drops the script dir from sys.path
import check_assets  # noqa: E402  (wav_info)

ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_DUMPS = ROOT / "dumps" / "common_sounds" / "sound"
DEFAULT_OUT = ROOT / "converted"

# role -> dump path (relative to dumps/common_sounds/sound), per gun. Names verified in MODLOG M1.
TABLE = {
    "ump45_mp": {
        "folder": "ump45",
        "fire": "weapons/ump45/weap_ump45_slst_10b.wav",
        "reload_lift": "foley/wpfoly_ump45_reload_lift_v1.wav",
        "reload_clipout": "foley/wpfoly_ump45_reload_clipout_v1.wav",
        "reload_clipin": "foley/wpfoly_ump45_reload_clipin_v1.wav",
        "reload_chamber": "foley/wpfoly_ump45_reload_chamber_v1.wav",
    },
}
ROLES = ["fire", "reload_lift", "reload_clipout", "reload_clipin", "reload_chamber"]


def sha256(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def main(argv):
    dumps, out = DEFAULT_DUMPS, DEFAULT_OUT
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--dumps":
            dumps = pathlib.Path(args.pop(0))
        elif a == "--out":
            out = pathlib.Path(args.pop(0))
        else:
            print(__doc__)
            return 2
    problems = []
    manifest = {"schema": 1, "generator": "copy_sounds.py", "base": "converted", "weapons": {}}
    copied = 0
    for gun, table in TABLE.items():
        entry = {"durations_s": {}, "sha256": {}, "format": {}}
        for role in ROLES:
            src = dumps / table[role]
            if not src.is_file():
                problems.append(f"{gun}.{role}: source missing {src}")
                continue
            rel = f"sounds/{table['folder']}/{src.name}"
            dst = out / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            fd, tmp = tempfile.mkstemp(prefix=dst.name + ".", suffix=".tmp", dir=dst.parent)
            os.close(fd)
            shutil.copyfile(src, tmp)
            os.replace(tmp, dst)
            info = check_assets.wav_info(dst)
            if "error" in info:
                problems.append(f"{gun}.{role}: {info['error']}")
                continue
            if info["tag"] != 1 or info["rate"] != 44100 or info["bits"] != 16:
                problems.append(f"{gun}.{role}: tag {info['tag']} rate {info['rate']} bits {info['bits']} "
                                "(want PCM 44100 Hz 16-bit)")
            hs, hd = sha256(src), sha256(dst)
            if hs != hd:
                problems.append(f"{gun}.{role}: copy hash differs")
            entry[role] = rel
            entry["durations_s"][role] = info["seconds"]
            entry["sha256"][role] = hd
            entry["format"][role] = f"PCM {info['rate']} Hz {info['bits']}-bit {info['channels']}ch"
            copied += 1
            print(f"{gun}.{role}: {rel} {info['seconds']} s {entry['format'][role]}")
        manifest["weapons"][gun] = entry
    sj = out / "sounds" / "sounds.json"
    sj.parent.mkdir(parents=True, exist_ok=True)
    sj.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {sj}")
    for p in problems:
        print("FAIL " + p)
    want = sum(len(ROLES) for _ in TABLE)
    if not problems and copied == want:
        print(f"PASS {copied} sounds copied, PCM 44.1 kHz 16-bit, hashes equal")
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
