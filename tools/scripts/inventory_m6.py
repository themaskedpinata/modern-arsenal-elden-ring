"""M6 inventory: every requested MW2 MP gun in the user's dumps -> work/m6/inventory.json + a text table.

Usage: python -I inventory_m6.py [--guns a_mp,b_mp] [--out work/m6/inventory.json] [--quiet]

Read-only over dumps/ (the user's own Unlinker output). For each gun it records the weapon-file fields M6 needs,
the gunModel/handModel and whether their GLB/xmodel/material/colour-map dumps exist, every viewmodel anim field
with its xanim (frames, fps, duration, notetracks, bone count), every sound alias the gun uses (weapon-file
*SoundPlayer fields + notetrackSoundMap) resolved to its WAV(s) and the dump folder (zone) holding them, and the
ADS overlay material for scoped guns. Nothing is copied or converted; no MW2 values are stored in this file.
Exit 0 = inventory written (gaps are reported, not failures), 2 = usage error.
"""
from __future__ import annotations

import json
import os
import pathlib
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_assets  # noqa: E402
import convert_weapons  # noqa: E402
import vm_format as vf  # noqa: E402
import xanim_read  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
DUMPS = ROOT / "dumps"
WEAPONS = DUMPS / "common_mp" / "weapons"
ASSETS = DUMPS / "common_mp_assets"
XANIM = ASSETS / "xanim"
OUT = ROOT / "work" / "m6" / "inventory.json"

# Requested arsenal (user's M6 choice), by class. Internal names verified against dumps/lists/common_mp.txt.
GUNS = {
    "ar": ["m4_mp", "famas_mp", "scar_mp", "tavor_mp", "fal_mp", "m16_mp", "masada_mp", "fn2000_mp", "ak47_mp"],
    "smg": ["mp5k_mp", "ump45_mp", "kriss_mp", "p90_mp", "uzi_mp"],
    "lmg": ["sa80_mp", "rpd_mp", "mg4_mp", "aug_mp", "m240_mp"],
    "sniper": ["cheytac_mp", "barrett_mp", "wa2000_mp", "m21_mp"],
    "shotgun": ["spas12_mp", "aa12_mp", "striker_mp", "ranger_mp", "m1014_mp", "model1887_mp"],
    "launcher": ["at4_mp", "m79_mp", "rpg_mp", "stinger_mp", "javelin_mp"],
}

# Alias folders (dump of one zone each) and the WAV roots next to them, searched in this order.
ALIAS_ZONES = [
    ("localized_common_mp.ff", DUMPS / "ac130" / "localized_common_mp" / "soundaliases",
     DUMPS / "ac130" / "localized_common_mp" / "sound"),
    ("common.ff (SP)", DUMPS / "common_sounds" / "soundaliases", DUMPS / "common_sounds" / "sound"),
    ("common_mp.ff", ASSETS / "soundaliases", ASSETS / "sound"),
    ("mp_rust.ff", DUMPS / "ac130" / "mp_rust" / "soundaliases", DUMPS / "ac130" / "mp_rust" / "sound"),
]
EXTRA_WAV_ROOTS = [("iwd (extracted)", DUMPS / "ac130" / "iwd" / "Sound"), ("iwd (extracted)", DUMPS / "ac130" / "iwd" / "sound")]

STR_FIELDS = ["displayName", "weaponClass", "weaponType", "inventoryType", "fireType", "playerAnimType",
              "penetrateType", "impactType", "gunModel", "handModel", "worldModel", "rocketModel", "projectileModel",
              "projExplosionType", "projExplosionEffect", "projExplosionSound", "projTrailEffect", "projIgnitionEffect",
              "projIgnitionSound", "projectileSound", "guidedMissileType", "viewFlashEffect", "viewShellEjectEffect",
              "adsOverlayShader", "adsOverlayShaderLowRes", "adsOverlayReticle", "adsOverlayInterface", "hudIcon",
              "killIcon", "ammoCounterIcon", "ammoCounterClip", "reticleCenter", "reticleSide", "ammoName", "clipName",
              "altWeapon", "tracerType", "stance"]
NUM_FIELDS = [
    # ammo / damage
    "clipSize", "maxAmmo", "startAmmo", "shotCount", "clipOnly", "damage", "minDamage", "playerDamage",
    "minPlayerDamage", "meleeDamage", "maxDamageRange", "minDamageRange", "locHead", "locHelmet", "locNeck",
    "locTorsoUpper", "locTorsoLower", "locRightArmUpper", "locRightLegUpper", "locRightFoot", "rifleBullet",
    "armorPiercing", "bBulletExplosiveDamage", "bulletExplDmgMult", "bulletExplRadiusMult", "penetrateMultiplier",
    # timing
    "fireTime", "fireDelay", "holdFireTime", "rechamberTime", "rechamberTimeOneHanded", "rechamberBoltTime",
    "boltAction", "rechamberWhileAds", "reloadTime", "reloadEmptyTime", "reloadAddTime", "reloadStartTime",
    "reloadStartAddTime", "reloadEndTime", "reloadShowRocketTime", "segmentedReload", "reloadAmmoAdd",
    "reloadStartAdd", "noPartialReload", "dropTime", "raiseTime", "firstRaiseTime", "quickDropTime",
    "quickRaiseTime", "emptyRaiseTime", "emptyDropTime", "sprintInTime", "sprintLoopTime", "sprintOutTime",
    "adsTransInTime", "adsTransOutTime", "meleeTime",
    # spread
    "hipSpreadStandMin", "hipSpreadDuckedMin", "hipSpreadProneMin", "hipSpreadMax", "hipSpreadDuckedMax",
    "hipSpreadProneMax", "hipSpreadDecayRate", "hipSpreadFireAdd", "hipSpreadTurnAdd", "hipSpreadMoveAdd",
    "hipSpreadDuckedDecay", "hipSpreadProneDecay", "adsSpread", "playerSpread",
    # kick
    "hipGunKickReducedKickBullets", "hipGunKickReducedKickPercent", "hipGunKickPitchMin", "hipGunKickPitchMax",
    "hipGunKickYawMin", "hipGunKickYawMax", "hipGunKickAccel", "hipGunKickSpeedMax", "hipGunKickSpeedDecay",
    "hipGunKickStaticDecay", "hipViewKickPitchMin", "hipViewKickPitchMax", "hipViewKickYawMin", "hipViewKickYawMax",
    "hipViewKickCenterSpeed", "adsGunKickReducedKickBullets", "adsGunKickReducedKickPercent", "adsGunKickPitchMin",
    "adsGunKickPitchMax", "adsGunKickYawMin", "adsGunKickYawMax", "adsGunKickAccel", "adsGunKickSpeedMax",
    "adsGunKickSpeedDecay", "adsGunKickStaticDecay", "adsViewKickPitchMin", "adsViewKickPitchMax",
    "adsViewKickYawMin", "adsViewKickYawMax", "adsViewKickCenterSpeed",
    # ADS / sway / movement
    "aimDownSight", "adsFire", "adsZoomFov", "adsZoomInFrac", "adsZoomOutFrac", "adsOverlayWidth",
    "adsOverlayHeight", "adsBobFactor", "adsViewBobMult", "adsIdleAmount", "adsIdleSpeed", "hipIdleAmount",
    "hipIdleSpeed", "adsSwayMaxAngle", "adsSwayLerpSpeed", "adsSwayPitchScale", "adsSwayYawScale", "swayMaxAngle",
    "swayLerpSpeed", "thermalScope", "moveSpeedScale", "adsMoveSpeedScale", "sprintDurationScale",
    "noAdsWhenMagEmpty", "lowAmmoWarningThreshold", "reticleCenterSize", "reticleSideSize",
    # projectile / explosion
    "projectileSpeed", "projectileSpeedUp", "projectileSpeedForward", "projectileActivateDist", "projectileLifetime",
    "timeToAccelerate", "projectileCurvature", "projImpactExplode", "projIgnitionDelay", "maxSteeringAccel",
    "explosionRadius", "explosionRadiusMin", "explosionInnerDamage", "explosionOuterDamage", "damageConeAngle",
    "lockonSupported", "requireLockonToFire", "bigExplosion", "fuseTime", "stickiness", "destabilizationRateTime",
    "destabilizationCurvatureMax", "destabilizeDistance", "ricochetChance",
]
SOUND_FIELDS = ["fireSoundPlayer", "lastShotSoundPlayer", "emptyFireSoundPlayer", "loopFireSoundPlayer",
                "stopFireSoundPlayer", "rechamberSoundPlayer", "reloadSoundPlayer", "reloadEmptySoundPlayer",
                "reloadStartSoundPlayer", "reloadEndSoundPlayer", "raiseSoundPlayer", "firstRaiseSoundPlayer",
                "putawaySoundPlayer", "pullbackSoundPlayer", "projectileSound", "projExplosionSound",
                "projIgnitionSound"]
# Base (centre) anim fields; the R/L akimbo variants are ignored (no akimbo in M6).
ANIM_FIELDS = ["idleAnim", "emptyIdleAnim", "fireAnim", "holdFireAnim", "lastShotAnim", "rechamberAnim",
               "meleeAnim", "meleeChargeAnim", "reloadAnim", "reloadEmptyAnim", "reloadStartAnim", "reloadEndAnim",
               "raiseAnim", "dropAnim", "firstRaiseAnim", "altRaiseAnim", "altDropAnim", "quickRaiseAnim",
               "quickDropAnim", "emptyRaiseAnim", "emptyDropAnim", "sprintInAnim", "sprintLoopAnim", "sprintOutAnim",
               "adsFireAnim", "adsLastShotAnim", "adsRechamberAnim", "adsUpAnim", "adsDownAnim", "detonateAnim",
               "breachRaiseAnim", "nightVisionWearAnim", "nightVisionRemoveAnim", "stunnedAnimStart",
               "stunnedAnimLoop", "stunnedAnimEnd"]
# anim field -> time field it is played over (for the duration-vs-time comparison)
ANIM_TIME = {"fireAnim": "fireTime", "adsFireAnim": "fireTime", "rechamberAnim": "rechamberTime",
             "adsRechamberAnim": "rechamberTime", "reloadAnim": "reloadTime", "reloadEmptyAnim": "reloadEmptyTime",
             "reloadStartAnim": "reloadStartTime", "reloadEndAnim": "reloadEndTime", "raiseAnim": "raiseTime",
             "dropAnim": "dropTime", "firstRaiseAnim": "firstRaiseTime", "quickRaiseAnim": "quickRaiseTime",
             "quickDropAnim": "quickDropTime", "emptyRaiseAnim": "emptyRaiseTime", "emptyDropAnim": "emptyDropTime",
             "sprintInAnim": "sprintInTime", "sprintLoopAnim": "sprintLoopTime", "sprintOutAnim": "sprintOutTime",
             "adsUpAnim": "adsTransInTime", "adsDownAnim": "adsTransOutTime"}


def ci_index(folder):
    """lower-case name -> real path for one folder (Windows is case-insensitive; keep the real spelling)."""
    if not folder.is_dir():
        return {}
    return {p.name.lower(): p for p in folder.iterdir()}


def ci_walk(root):
    out = {}
    if root.is_dir():
        for dp, _dn, fn in os.walk(root):
            for f in fn:
                p = pathlib.Path(dp) / f
                out[p.relative_to(root).as_posix().lower()] = p
    return out


XANIMS = ci_index(XANIM)
GLBS = ci_index(ASSETS / "model_export")
XMODELS = ci_index(ASSETS / "xmodel")
MATS = ci_index(ASSETS / "materials" / "mc")
MATS_GFX = ci_index(ASSETS / "materials" / "gfx")
MATS_ALL = {}
for _sub in ("mc", "gfx", "wc", ""):
    MATS_ALL.update({k: v for k, v in ci_index(ASSETS / "materials" / _sub).items() if k not in MATS_ALL})
IMAGES = ci_index(ASSETS / "images")
ALIAS_IDX = [(zone, ci_index(adir), ci_walk(wroot), wroot) for zone, adir, wroot in ALIAS_ZONES]
EXTRA_WAVS = [(z, ci_walk(r)) for z, r in EXTRA_WAV_ROOTS]


def num(v):
    try:
        x = float(v)
    except (TypeError, ValueError):
        return v
    return int(x) if x.is_integer() else x


def parse_csv_rows(path):
    import csv
    with open(path, encoding="latin-1", newline="") as f:
        rows = list(csv.DictReader(f))
    return rows


def resolve_alias(alias):
    """alias -> {zone, rows:[{file, type, loop, vol, wav:{zone, path, fmt}|None}]} or {missing}."""
    if not alias:
        return None
    for zone, aidx, widx, _wroot in ALIAS_IDX:
        p = aidx.get(alias.lower() + ".csv")
        if not p:
            continue
        rows = []
        for r in parse_csv_rows(p):
            f = (r.get("file") or "").replace("\\", "/")
            ent = {"file": f, "type": r.get("type"), "loop": r.get("loop"), "seq": r.get("sequence"),
                   "vol": [num(r.get("vol_min")), num(r.get("vol_max"))],
                   "pitch": [num(r.get("pitch_min")), num(r.get("pitch_max"))],
                   "secondary": r.get("secondaryaliasname") or "", "chain": r.get("chainaliasname") or "",
                   "wav": None}
            # the alias zone first, then every other dump
            cands = [(zone, widx)] + [(z, w) for z, _a, w, _r in ALIAS_IDX if z != zone] + EXTRA_WAVS
            for wz, wi in cands:
                wp = wi.get(f.lower())
                if wp:
                    info = check_assets.wav_info(wp)
                    ent["wav"] = {"zone": wz, "path": wp.relative_to(ROOT).as_posix(), "size": wp.stat().st_size,
                                  "fmt": None if "error" in info else
                                  f"tag{info['tag']} {info['rate']}Hz {info['bits']}b {info['channels']}ch",
                                  "seconds": info.get("seconds"), "error": info.get("error")}
                    break
            rows.append(ent)
        return {"alias": alias, "zone": zone, "rows": rows}
    return {"alias": alias, "zone": None, "rows": [], "missing": True}


def material_colour(mname):
    p = MATS_ALL.get(mname.lower() + ".json")
    if not p:
        return {"material": mname, "missing": True}
    mj = json.loads(p.read_text(encoding="utf-8"))
    tex = {t.get("name"): t.get("image") for t in mj.get("textures", [])}
    cm = tex.get("colorMap")
    img = IMAGES.get((cm or "").lower() + ".dds") if cm else None
    out = {"material": mname, "json": p.relative_to(ROOT).as_posix(), "colorMap": cm, "techset": mj.get("techniqueSet"),
           "textures": tex, "dds": img.relative_to(ROOT).as_posix() if img else None,
           "dds_size": img.stat().st_size if img else None, "region": mj.get("cameraRegion")}
    if img:
        try:
            d = vf.read_dds(img)
            out["dds_fmt"] = {"format": d["format"], "w": d["w"], "h": d["h"], "mips": len(d["mips"])
                              if isinstance(d["mips"], list) else d["mips"]}
        except Exception as e:  # noqa: BLE001
            out["dds_error"] = str(e)
    return out


def model_info(name):
    if not name:
        return None
    glb = GLBS.get(name.lower() + "_lod0.glb")
    xm = XMODELS.get(name.lower() + ".json")
    out = {"model": name, "glb": glb.relative_to(ROOT).as_posix() if glb else None,
           "xmodel_type": json.loads(xm.read_text(encoding="utf-8")).get("type") if xm else None}
    if glb:
        g = vf.Glb(glb)
        skins = g.json.get("skins", [])
        tris, verts, mats = 0, 0, []
        seen_pos = set()
        for m in g.json.get("meshes", []):
            for pr in m["primitives"]:
                ia = g.json["accessors"][pr["indices"]]
                tris += ia["count"] // 3
                pa = pr["attributes"]["POSITION"]
                if pa not in seen_pos:
                    seen_pos.add(pa)
                    verts += g.json["accessors"][pa]["count"]
                mn = g.json["materials"][pr["material"]]["name"].split("/")[-1]
                if mn not in mats:
                    mats.append(mn)
        out.update({"size": glb.stat().st_size, "joints": len(skins[0]["joints"]) if skins else 0, "tris": tris,
                    "verts": verts, "node_names": sorted(nd.get("name", "") for nd in g.nodes),
                    "materials": [material_colour(m) for m in mats]})
    return out


def anim_info(name):
    if not name:
        return None
    p = XANIMS.get(name.lower())
    if not p:
        return {"name": name, "missing": True}
    try:
        xa = xanim_read.read_xanim(str(p))
    except Exception as e:  # noqa: BLE001
        return {"name": name, "file": p.name, "error": str(e)}
    return {"name": name, "file": p.name, "size": p.stat().st_size, "frames": xa["numframes"], "fps": xa["framerate"],
            "dur": round(xa["duration"], 4), "looped": xa["looped"], "bones": len(xa["bones"]),
            "bone_names": [b["name"] for b in xa["bones"]],
            "notes": [(n["name"], n["frame"], round(n["frame"] / xa["framerate"], 3) if xa["framerate"] else 0)
                      for n in xa["notes"]]}


def inventory_gun(name, cls):
    path = WEAPONS / name
    text, raw = convert_weapons.read_text(path)
    kv = dict(convert_weapons.parse_weaponfile(text))
    g = {"name": name, "class": cls, "fields_count": len(kv)}
    g["str"] = {k: kv.get(k, "") for k in STR_FIELDS}
    g["num"] = {k: num(kv[k]) for k in NUM_FIELDS if k in kv}
    g["missing_fields"] = [k for k in STR_FIELDS + NUM_FIELDS + SOUND_FIELDS + ANIM_FIELDS if k not in kv]
    g["hideTags"] = [t for t in kv.get("hideTags", "").split("\n") if t.strip()]
    nmap = []
    for line in kv.get("notetrackSoundMap", "").split("\n"):
        parts = line.split()
        if len(parts) == 2:
            nmap.append(parts)
    g["notetrackSoundMap"] = nmap
    g["gun"] = model_info(kv.get("gunModel"))
    g["hands"] = kv.get("handModel")
    g["anims"] = {}
    for f in ANIM_FIELDS:
        v = kv.get(f, "")
        if v:
            a = anim_info(v)
            tf = ANIM_TIME.get(f)
            if tf and isinstance(a, dict) and "dur" in a:
                a["time_field"] = tf
                a["time"] = num(kv.get(tf))
            g["anims"][f] = a
    # all xanims sharing the gun's anim prefix (to find extra clips, e.g. shotgun loops, rechamber_ads)
    sounds = {}
    for f in SOUND_FIELDS:
        v = kv.get(f, "")
        if v:
            sounds[f] = resolve_alias(v)
    for note, alias in nmap:
        if alias.startswith("melee_"):
            continue
        sounds["note:" + note] = resolve_alias(alias)
    g["sounds"] = sounds
    if kv.get("adsOverlayShader"):
        g["overlay"] = material_colour(kv["adsOverlayShader"])
    return g


def main(argv):
    out, guns, quiet = OUT, None, False
    a = list(argv)
    while a:
        x = a.pop(0)
        if x == "--out":
            out = pathlib.Path(a.pop(0))
        elif x == "--guns":
            guns = a.pop(0).split(",")
        elif x == "--quiet":
            quiet = True
        else:
            print(__doc__)
            return 2
    todo = [(n, c) for c, ns in GUNS.items() for n in ns if guns is None or n in guns]
    inv = {"schema": 1, "generator": "inventory_m6.py", "guns": {}}
    for n, c in todo:
        inv["guns"][n] = inventory_gun(n, c)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(inv, indent=1), encoding="utf-8")
    if not quiet:
        print(f"wrote {out} ({len(todo)} guns)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
