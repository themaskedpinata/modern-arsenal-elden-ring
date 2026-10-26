"""Co-op friend package: re-creates the MW2 dumps the converters read, from the friend's OWN MW2 install.

    python -I friend_dump.py --mw2 "<MW2 folder>" --unlinker <Unlinker.exe> [--only name,name] [--list]
    python -I friend_dump.py --compare <reference dumps folder>     (diagnostics: file list + sha256 per plan step)

Writes only under <root>\\dumps (root = two folders above this script). Reads the MW2 install (zone\\english\\*.ff,
main\\*.iwd) read-only. The plan below is the recipe this project's dumps were made with (MODLOG M1/M4/M5/M7/M8; the
Unlinker command lines were reconstructed from the dump logs' "Dumped <type>" counts and folder contents, then proven
by the converted outputs' sha256 matching the host's manifest). The Unlinker is the external GPL-3.0 OpenAssetTools
build (iw4x-x64/oat df203f9 + 2 build-fix patches); it is run as a separate program, never linked.
Pure stdlib (zipfile, subprocess, hashlib).
"""
import hashlib
import pathlib
import shutil
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"

# (name, zone, output folder under dumps\, --include-assets, extra flags, explicit search path to <MW2>\main)
UNLINK = [
    ("common_mp", "common_mp", "common_mp", "weapon", ["--skip-obj"], False),
    ("common_mp_assets", "common_mp", "common_mp_assets", "xmodel,xanim,sound,loadedsound,material,image",
     ["--model-format", "GLB", "--image-format", "DDS"], False),
    ("common_sounds", "common", "common_sounds", "sound,loadedsound", [], False),
    ("ac130_common_mp", "common_mp", "ac130/common_mp", "image,material,menu,menulist,rawfile,weapon", ["--image-format", "DDS"], True),
    ("ac130_localized_common_mp", "localized_common_mp", "ac130/localized_common_mp", "sound,loadedsound,rawfile", [], True),
    ("ac130_localize", "localized_common_mp", "ac130/localized_common_mp_localize", "localize", [], True),
    ("ac130_mp_rust", "mp_rust", "ac130/mp_rust", "sound,loadedsound", [], True),
    ("m5_controls", "localized_code_pre_gfx_mp", "m5/localized_code_pre_gfx_mp", "rawfile", [], False),
    ("m5_gamesettings", "code_pre_gfx_mp", "m5/code_pre_gfx_mp", "rawfile", [], False),
    ("m5_common_mp", "common_mp", "m5/common_mp", "rawfile", [], False),
    ("m5_patch_mp", "patch_mp", "m5/patch_mp", "rawfile", [], False),
    ("m7_common_mp", "common_mp", "m7/common_mp", "fx,rawfile,weapon", [], True),
    ("m7_stringtable", "code_post_gfx_mp", "m7/code_post_gfx_mp", "stringtable", [], False),
    ("m7_localized_common_mp", "localized_common_mp", "m7/localized_common_mp", "sound,loadedsound", [], True),
    ("m7_localize", "localized_code_post_gfx_mp", "m7/localized_code_post_gfx_mp_localize", "localize", [], False),
    ("m8_code_post_gfx", "code_post_gfx", "m8/code_post_gfx", "xanim", [], False),
    ("m8_common", "common", "m8/common", "xanim", [], False),
]

# Files taken straight out of the MW2 .iwd archives (zip): (output folder under dumps\, keep the iwd name as a folder,
# member paths). The iwd holding each member is found by search (main\iw_*.iwd, main\localized_english_iw*.iwd).
IWD = [
    ("ac130/iwd", False, ["images/ac130_hud_target.iwi", "images/ac130_overlay_nofire.iwi",
                          "Sound/ambient/amb_ac130_int0v2_lr.mp3", "Sound/mp/mp_killstrk_ac130.wav",
                          "Sound/vehicles/veh_ac130_ext_dist1.wav", "Sound/vehicles/veh_ac130_ext_dist1_fade.wav"]),
    ("m7/iwd", True, ["images/death_nuke.iwi", "images/dpad_nuke.iwi", "images/map_nuke_selector.iwi", "images/nuke_glow.iwi",
                      "images/specialty_nuke.iwi",
                      "Sound/explosions/exp_105mm_crack_debris1.wav", "Sound/explosions/exp_105mm_crack_debris2.wav",
                      "Sound/explosions/exp_105mm_crack_debris3.wav", "Sound/level/airlift_nuclear_aftmath.wav",
                      "Sound/level/airlift_nuclear_exp.wav", "Sound/mp/mp_killstrk_nuclearstrike.wav",
                      "Sound/music/hz_mp_defeat_01.mp3", "Sound/music/hz_mp_nuke.mp3",
                      "Sound/battlechatter/TF/MP/UK_1mc_achieve_tnuke_01.wav", "Sound/battlechatter/TF/MP/UK_1mc_achieve_tnuke_02.wav",
                      "Sound/battlechatter/TF/MP/UK_1mc_enemy_tinuke_02.wav", "Sound/battlechatter/TF/MP/UK_1mc_use_tnuke_02.wav",
                      "Sound/battlechatter/US/MP/US_1mc_achieve_tnuke_01.wav", "Sound/battlechatter/US/MP/US_1mc_achieve_tnuke_02.wav",
                      "Sound/battlechatter/US/MP/US_1mc_enemy_tinuke_02.wav", "Sound/battlechatter/US/MP/US_1mc_use_tnuke_02.wav"]),
]

# dumps\m7\hud: copies of files the ac130_common_mp step dumped (the nuke converter reads them from one folder).
HUD = [("ac130/common_mp/images/" + n, "m7/hud/" + n) for n in ("death_nuke.dds", "dpad_nuke.dds", "map_nuke_selector.dds", "specialty_nuke.dds")] + \
      [("ac130/common_mp/materials/" + n, "m7/hud/" + n) for n in ("death_nuke.json", "dpad_killstreak_nuke.json", "gfx_distortion_nuke.json",
                                                                  "map_nuke_selector.json", "specialty_nuke.json")] + \
      [("ac130/common_mp/ui_mp/scorebar_hd.menu", "m7/hud/scorebar_hd.menu")]


def unlink(unlinker, mw2, step):
    name, zone, out, inc, extra, sp = step
    ff = mw2 / "zone" / "english" / f"{zone}.ff"
    if not ff.is_file():
        print(f"FAIL {name}: {ff} not found")
        return False
    dst = DUMPS / out
    if dst.exists():
        shutil.rmtree(dst)
    dst.mkdir(parents=True)
    log = DUMPS / (out + ".log")
    cmd = [str(unlinker), "--game", "IW4MS"]
    if sp:
        cmd += ["--search-path", str(mw2 / "main")]
    cmd += ["--include-assets", inc, *extra, "-o", str(dst), str(ff)]
    with open(log, "wb") as f:
        r = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, cwd=str(DUMPS))
    tail = log.read_text(encoding="utf-8-sig", errors="replace").strip().splitlines()[-1:] or ["(no output)"]
    ok = r.returncode == 0 and tail[0].startswith("Finished with") and ", 0 errors" in tail[0]
    nfiles = sum(1 for p in dst.rglob("*") if p.is_file())
    print(f"{'OK  ' if ok else 'FAIL'} {name}: {zone}.ff [{inc}] -> dumps\\{out} ({nfiles} files; {tail[0]})")
    return ok


def iwd_index(mw2):
    idx = {}
    main = mw2 / "main"
    for z in sorted(main.glob("*.iwd")):
        try:
            with zipfile.ZipFile(z) as zf:
                for n in zf.namelist():
                    idx.setdefault(n.lower(), (z, n))
        except zipfile.BadZipFile:
            print(f"WARN {z.name}: not a zip")
    return idx


def extract(mw2):
    idx = iwd_index(mw2)
    ok = True
    for out, by_iwd, members in IWD:
        base = DUMPS / out
        if base.exists():
            shutil.rmtree(base)
        n = 0
        for m in members:
            hit = idx.get(m.lower())
            if hit is None:
                print(f"FAIL iwd member {m} not found in {mw2 / 'main'}\\*.iwd")
                ok = False
                continue
            z, real = hit
            dst = base / (z.name if by_iwd else "") / m
            dst.parent.mkdir(parents=True, exist_ok=True)
            with zipfile.ZipFile(z) as zf, zf.open(real) as src, open(dst, "wb") as f:
                shutil.copyfileobj(src, f)
            n += 1
        print(f"{'OK  ' if ok else 'FAIL'} iwd -> dumps\\{out}: {n}/{len(members)} files")
    return ok


def hud():
    ok = True
    for src, dst in HUD:
        s, d = DUMPS / src, DUMPS / dst
        if not s.is_file():
            print(f"FAIL hud copy: dumps\\{src} missing")
            ok = False
            continue
        d.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(s, d)
    print(f"{'OK  ' if ok else 'FAIL'} hud: {len(HUD)} files copied into dumps\\m7\\hud")
    return ok


def tree(base):
    return {p.relative_to(base).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in base.rglob("*") if p.is_file() and p.parent.name != "zone_source"}


def compare(ref):
    ref = pathlib.Path(ref)
    folders = [s[2] for s in UNLINK] + [o for o, _, _ in IWD] + ["m7/hud"]
    allok = True
    for f in folders:
        a, b = tree(DUMPS / f) if (DUMPS / f).is_dir() else {}, tree(ref / f) if (ref / f).is_dir() else {}
        same = sum(1 for k in a if b.get(k) == a[k])
        diff = sorted(k for k in a if k in b and b[k] != a[k])
        only_new, only_ref = sorted(set(a) - set(b)), sorted(set(b) - set(a))
        ok = not diff and not only_ref
        allok &= ok
        print(f"{'SAME' if ok and not only_new else ('OK+ ' if ok else 'DIFF')} dumps\\{f}: {same} identical, {len(diff)} differ, "
              f"{len(only_new)} only new, {len(only_ref)} only in the reference" + (f" e.g. {(diff + only_ref)[:2]}" if not ok else ""))
    print("PASS dumps equal the reference" if allok else "INFO dumps differ from the reference (the converted-output hashes decide)")
    return 0


def main(argv):
    if len(argv) == 2 and argv[0] == "--compare":
        return compare(argv[1])
    args = dict(zip(argv[::2], argv[1::2]))
    if "--list" in argv:
        for s in UNLINK:
            print(s)
        return 0
    if "--mw2" not in args or "--unlinker" not in args:
        print(__doc__)
        return 2
    mw2, unlinker = pathlib.Path(args["--mw2"]), pathlib.Path(args["--unlinker"])
    only = set(args["--only"].split(",")) if "--only" in args else None
    if not unlinker.is_file():
        print(f"FAIL Unlinker not found: {unlinker}")
        return 1
    DUMPS.mkdir(parents=True, exist_ok=True)
    ok = True
    for step in UNLINK:
        if only is None or step[0] in only:
            ok &= unlink(unlinker, mw2, step)
    if only is None or "iwd" in only:
        ok &= extract(mw2)
    if only is None or "hud" in only:
        ok &= hud()
    print("PASS dumps ready" if ok else "FAIL dumps incomplete (see the lines above)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
