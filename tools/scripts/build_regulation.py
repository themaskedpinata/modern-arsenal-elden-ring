"""Build build/package/regulation.bin for mw2er (M2) and prove it with a round trip.

Usage:
  python -I build_regulation.py              build + round trip + field-level check, publish on PASS
  python -I build_regulation.py --self-test  prove the checks can fail (7 deliberate mutations: 4 in Bullet.param,
                                             an untouched param, the BND manifest, the XML read-back)
  python -I build_regulation.py --rebaseline copy the game's regulation.bin afresh and re-unpack it
                                             (never automatic; the recorded hash must then be reviewed)

Inputs: the game's regulation.bin (READ ONLY, hash-checked), work/regulation (gitignored copies),
mw2er.toml ([damage] k, [regulation]) and converted/weapons.json (MW2 damage per gun).

What is added (docs/M2_DESIGN.md section 16), nothing else changes:
- Bullet rows   id_base + slot*100 + band (one per falloff band) cloned from the Throwing Dagger bullet,
                near-hitscan (999 m/s, 0.15 s life, no gravity/homing/lock-on), HitBulletID = marker;
- Bullet row    marker_id: an inert impact marker (no attack, no sfx, 0.1 s life);
- AtkParam_Pc   same ids as the band bullets, cloned from the Throwing Dagger attack, atkPhys = round(D*k).
M4 (docs/M4_DESIGN.md section 8) adds 15 rows, sorted, in the verified-free 79xxxxxx range:
- Bullet + AtkParam_Pc 79000201-203 (105 core/ring/single), 79000301-303 (40), 79000401 (25): AC-130 explosion
                spheres cloned from Fire Pot [Hit] 10030001; atkPhys = round(MW2 radius damage x k x scale_<gun>)
                through the same linear model as mw2er_core::ac130_dmg (inputs: converted/ac130/weapons_ac130.json,
                [regulation.ac130]). Run 8 fix: the AtkParam rows are FLAT damage (AC130_FLAT_ATK: isAddBaseAtk 1,
                every *Correction 0, disableGuard 1), the vanilla weapon-skill bullet pattern (e.g. 30000841); a bullet
                spawned with behavior/goods/magic -1 is a "weapon" attack, so with isAddBaseAtk 0 ER used the
                player's weapon AR x atkPhysCorrection and ignored atkPhys (run 8: 2..88 per hit at atkPhys 650..2100);
- SpEffectParam 79000950: the gunship protection (100% damage cut, no stagger, noDead), cloned from 1540, with a
                FINITE effectEndurance (protect_endurance_s, default 90 s; the DLL re-applies it while riding) so a
                copy the DLL failed to remove (panic, quit) expires on its own (review M4 major 2).
Every added row equals its clone except the documented override set; the self-damage safety fields are asserted.

Steps: source hash check -> Paramdex version preflight -> pristine tree == fresh unpack of the vanilla copy
-> build tree (vanilla .param binaries + the 2 edited XMLs) -> WitchyBND repack (2 XMLs, then the BND)
-> round trip: unpack the built regulation, then (a) every untouched param byte-identical, the 2 edited
params differ ONLY by exactly the added rows (param_diff.check_added), (b) every added row decoded from
the binary equals the clone source except the intended overrides, (c) the BND manifest is unchanged,
(d) WitchyBND itself reads the edited params back -> publish (temp file + rename) -> build/rows.json.
Exit 0 = PASS, 1 = FAIL. Prints one summary line `REGULATION PASS|FAIL ...`.
"""
import datetime
import hashlib
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
import time
import tomllib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # -I drops the script dir from sys.path
import param_diff  # noqa: E402
import param_rows  # noqa: E402

SCRIPTS = pathlib.Path(__file__).resolve().parent
ROOT = SCRIPTS.parents[1]
# The game's vanilla regulation.bin (only ever READ and copied). Co-op friend package: setup.ps1 points this at the
# friend's own install with the MW2ER_GAME_REGULATION environment variable; the VANILLA_SHA256 pin below still applies.
GAME_REGULATION = pathlib.Path(os.environ.get("MW2ER_GAME_REGULATION") or r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\regulation.bin")
# SHA-256 of the vanilla regulation.bin on this PC. Port 2.7.2.0 (recorded 2026-10-20, Steam build 25827938): BND
# version 11730034 (Paramdex upgrader string 1_17_3_0034L; WitchyBND's repack refuses a regulation newer than its
# upgrader version). Its only change from the 1.17.1 one is vanilla Bullet 20003309 atkId_Bullet 0 -> 1 (no row of
# ours is cloned from or refers to it); every def field version <= 11210015, so the layouts are unchanged.
# Before: 1.17.1 regulation 766521f9508de3a3532df61c45a1c2d93340f1ff7ed8306ab20df761712ca2ab, 1_17_1_1000L, 11711000
# (recorded 2026-10-13; its unpack is archived in work/regulation_1.17.1).
VANILLA_SHA256 = "b8cdab84c0772f66811fdc5176a381877ca6657b37d420cc2c1562a90d63870e"
PARAMDEX_VERSION = "1_17_3_0034L"
REGULATION_VERSION = 11730034

WITCHY_APP = ROOT / "tools" / "WitchyBND" / "app"
WITCHY = WITCHY_APP / "WitchyBND.exe"
CONPTY = SCRIPTS / "conpty_run.py"
WORK = ROOT / "work" / "regulation"
VANILLA = WORK / "vanilla" / "regulation.bin"
PRISTINE_PARENT = WORK / "unpacked"
PRISTINE = PRISTINE_PARENT / "regulation-bin"
FRESH_PARENT = WORK / "fresh"
BUILD_PARENT = WORK / "build"
BUILD = BUILD_PARENT / "regulation-bin"
CHECK_PARENT = WORK / "build_check"
CHECK = CHECK_PARENT / "regulation-bin"
LOGS = WORK / "logs"
PACKAGE_DIR = ROOT / "build" / "package"
PACKAGE = PACKAGE_DIR / "regulation.bin"
ROWS_JSON = ROOT / "build" / "rows.json"
CONFIG = ROOT / "mw2er.toml"
WEAPONS_JSON = ROOT / "converted" / "weapons.json"
AC130_JSON = ROOT / "converted" / "ac130" / "weapons_ac130.json"
GUNS_DIR = ROOT / "converted" / "guns"
EDITED = ("Bullet", "AtkParam_Pc", "SpEffectParam")
PARAM_COUNT = 194
ID_RANGE = (79_000_000, 79_999_999)  # verified free in all 194 vanilla params
AC130_RANGE = (79_000_200, 79_000_499)  # M4 explosion rows: slots 2..4
MAX_BANDS = 16
BOM = b"\xef\xbb\xbf"
INCH = 0.0254
# M4 tuning per gun (docs/M4_DESIGN.md table 8.4: stamina, poise, stagger, knockback, object damage are tuning
# guesses [I]); keys: tier -> (atkStam, atkSuperArmor, dmgLevel, knockbackDist, atkObj).
AC130_TUNING = {
    "105": {"core": (200, 50.0, 10, 3.0, 100), "ring": (100, 25.0, 7, 2.0, 50), "single": (200, 50.0, 10, 3.0, 100)},
    "40": {"core": (80, 15.0, 7, 1.5, 30), "ring": (40, 8.0, 3, 1.0, 20), "single": (80, 15.0, 7, 1.5, 30)},
    "25": {"single": (20, 3.0, 2, 0.3, 10)},
}
AC130_TIER_SLOT = {"core": 1, "ring": 2, "single": 3}
# Object (asset) destruction, user request after run 9 (docs/M4_DESIGN.md section 20). Evidence from the vanilla 1.17.1
# regulation (WitchyBND/Paramdex, scripts in the MODLOG entry): an asset row (AssetEnvironmentGeometryParam) breaks
# when an attack's AtkParam atkObj exceeds its `defense` (Paramdex wiki of isBushesForDamage: "object attack power >
# defense power" [R]); the 2,968 damage-breakable rows (hp > 0) use defense 0 (1,983), 1..9 (41), 90 (870), 95 (3,
# caravan wagons), 199 (37), 299 (6), 990/999 (28, never broken by an attack). Vanilla atkObj: AtkParam_Pc max 100
# (11 rows; heavy swings 60/80, Fire Pot [Hit] 10, ballista 20), AtkParam_Npc 100 (5,335) and 200 (529, the
# heaviest boss/giant attacks). Rows with isBushesForDamage 1 (63) also need isEnableCalcDamageForBushesObj 1 on the
# attack (304 NPC rows, e.g. dragon fire 4500100, all atkObj 100). Chosen [I until the next run]:
#   105 core/single 200 (= the strongest vanilla tier: breaks everything any vanilla attack breaks, defense <= 199;
#   the 299/990/999 rows that no vanilla attack breaks stay intact), 105 ring 100, 40 core/single 100 (breaks the
#   defense-90/95 barricades and wagons, = the strongest player tier), 40 ring 60, 25 40 (crates/barrels, defense
#   <= 9). Bush assets: the 105 rows and the 40 core/single (like dragon fire).
# tier -> (atkObj, isEnableCalcDamageForBushesObj, asset defence it must beat)
AC130_OBJECT = {
    "105": {"core": (200, 1, 199), "ring": (100, 1, 95), "single": (200, 1, 199)},
    "40": {"core": (100, 1, 95), "ring": (60, 0, 9), "single": (100, 1, 95)},
    "25": {"single": (40, 0, 9)},
}
# Impact visuals (user request after run 9): ER SFX ids taken from vanilla player Bullet rows (common effects, not a
# character's own ffxbnd, so they are loaded everywhere [I]); the ring row carries the dust/debris layer of the core
# row's fireball, so one impact shows both. sfxPostureType 1 ("Global Y Axis", as the Meteorite impact 10470003): the
# effect stands upright whatever the spawn direction (the clone's 0 = parent orientation) [I].
#   sfx_105      524028 Giantsflame Take Thee [Charged] burst (Bullet 10610051, r 3.5 m; the uncharged 524023 was used
#                       up to run 9)
#   sfx_105_ring 530123 Meteorite impact, rock + dust (Bullet 10470003)
#   sfx_40       300602 Explosive Stone burst (Bullet 10183001), unchanged
#   sfx_40_ring  530133 Rock Sling rock impact, dirt (Bullet 10471009 sfxId_Hit)
#   sfx_25       530133 Rock Sling rock impact: small dirt/sparks per 25 mm hit (was -1)
AC130_SFX_DEFAULT = {"105": 524_028, "105_ring": 530_123, "40": 300_602, "40_ring": 530_133, "25": 530_133}
AC130_SFX_POSTURE = 1
# Run 8 (MODLOG 2026-10-17 "run 8 fixes"): flat attack power for the explosion rows. With behavior/goods/magic -1
# (spawn_bullet) ER treats our bullet as a weapon attack: damage = weapon AR x atkPhysCorrection%, atkPhys unused
# unless isAddBaseAtk = 1 (UMP45 bands 140..160 all did 49 on c4311; AC-130 rows 650..2100 did 28..44). The vanilla
# weapon-skill bullets that deal a fixed amount use exactly this set (AtkParam_Pc 30000841 / 30300870: isAddBaseAtk 1,
# every correction 0, isDisableBothHandsAtkBonus 1, spEffectAtkPowerCorrectRate_byPoint 0). disableGuard 1: a shell
# from above is not blocked by a raised shield (run 8: 2..4 per hit on a guarding soldier, like the UMP45's dmg=4
# shield blocks). [I] until the next run's HIT lines.
AC130_FLAT_ATK = {
    "isAddBaseAtk": 1, "atkPhysCorrection": 0, "atkMagCorrection": 0, "atkFireCorrection": 0, "atkThunCorrection": 0,
    "atkDarkCorrection": 0, "atkStamCorrection": 0, "guardAtkRateCorrection": 0, "guardBreakCorrection": 0,
    "isDisableBothHandsAtkBonus": 1, "spEffectAtkPowerCorrectRate_byPoint": 0, "disableGuard": 1,
}
AC130_SCALE_DEFAULT = {"105": 1.0, "40": 1.0, "25": 0.7}   # == mw2er_core::config::RegAc130::default()
AC130_GUN_SLOT = {"105": 2, "40": 3, "25": 4}
# The 25 mm only ever uses one sphere (its radius is 0.81 m).
AC130_TWO_TIER = {"105": True, "40": True, "25": False}
# Section 8.3-8.5 "asserted unchanged" / safety values of the built rows.
SAFETY = {
    "Bullet": {"isHitBothTeam": 0, "isUseSharedHitList": 0, "isPenetrateChr": 1, "isPenetrateObj": 1, "isPenetrateMap": 1,
               "HitBulletID": -1, "initVellocity": 0.0, "dist": 0.0},
    "AtkParam_Pc": {"selfTarget": 0, "friendlyTarget": 0, "opposeTarget": 1, "atkFire": 0,
                    "mapHitType": 2, "isDisableNoDamage": 0, **AC130_FLAT_ATK},
    "SpEffectParam": {"noDead": 1, "slashDamageCutRate": 0.0, "blowDamageCutRate": 0.0,
                      "thrustDamageCutRate": 0.0, "neutralDamageCutRate": 0.0, "magicDamageCutRate": 0.0,
                      "fireDamageCutRate": 0.0, "thunderDamageCutRate": 0.0, "darkDamageCutRate": 0.0},
}
# effectEndurance is set per build from [regulation.ac130] protect_endurance_s (finite; review M4 major 2).
PROTECT_ENDURANCE_DEFAULT_S = 90.0
PROTECT_ENDURANCE_RANGE_S = (30.0, 600.0)
SPEFFECT_OVERRIDES = {
    "toughnessDamageCutRate": 0.0, "saReceiveDamageRate": 0.0,
    **{f"dmgLv_{x}": 1 for x in ("None", "S", "M", "L", "BlowM", "Push", "Strike", "BlowS", "Min", "Uppercut", "BlowLL",
                                 "Breath")},
    "noDead": 1, "fallDamageRate": 0.0, "spCategory": 0, "isDisableNetSync": 1,
}


# ---- M6 (docs/M6_DESIGN.md section 9): the arsenal rows ---------------------------------------------------------
# id = id_base + g*1000 + kind*100 + idx; kind 0 hitscan bands, 5 launcher (0 impact, 1 marker), 6 explosion tiers
# (1 core, 2 ring, 3 single). g comes from converted/guns/<name>/stats.json (convert_arsenal.GUN_INDEX: stable).
M6_RANGE = (79_100_000, 79_199_999)
M6_KIND_BANDS, M6_KIND_LAUNCHER, M6_KIND_EXPLOSION = 0, 5, 6
M6_TIER_IDX = {"core": 1, "ring": 2, "single": 3}
# Every M6 gun AtkParam_Pc row: FLAT damage (the run-8 fix for every gun: isAddBaseAtk 1, every correction 0) with
# disableGuard 0 (shields still block a bullet) and atkAttribute 3 (Standard; the dagger clone has 2 = pierce).
M6_GUN_FLAT = {
    "isAddBaseAtk": 1, "atkPhysCorrection": 0, "atkMagCorrection": 0, "atkFireCorrection": 0, "atkThunCorrection": 0,
    "atkDarkCorrection": 0, "atkStamCorrection": 0, "guardAtkRateCorrection": 0, "guardBreakCorrection": 0,
    "isDisableBothHandsAtkBonus": 1, "spEffectAtkPowerCorrectRate_byPoint": 0, "disableGuard": 0, "atkAttribute": 3,
    "selfTarget": 0, "friendlyTarget": 0, "opposeTarget": 1,
}
# The asserted values on the decoded binary of every M6 row (section 9.6 SAFETY_M6).
SAFETY_M6 = {
    "gun_bullet": {"isHitBothTeam": 0, "isUseSharedHitList": 0, "gravityInRange": 0.0, "homingAngle": 0.0},
    "gun_atk": dict(M6_GUN_FLAT),
    "expl_bullet": {"isHitBothTeam": 0, "isUseSharedHitList": 0, "HitBulletID": -1, "initVellocity": 0.0, "dist": 0.0},
    "expl_atk": {"selfTarget": 0, "friendlyTarget": 0, "opposeTarget": 1, **AC130_FLAT_ATK},
    "marker_bullet": {"atkId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1, "HitBulletID": -1},
}
# Explosion tiers of the launchers: atkObj (object damage) and the bush flag, as the AC-130 40 mm core [I]; the M79
# 40 mm grenade like the 40 mm ring.
M6_EXPL_OBJ = {"default": (100, 1), "m79_mp": (60, 0)}
M6_EXPL_SFX = {"core": 300_602, "ring": 530_133, "single": 300_602}

# ---- M7 (docs/M7_DESIGN.md 3.13): the Tactical Nuke rows -----------------------------------------------------------
# SpEffectParam 79000960: one-shot 100 % max-HP damage, cloned from vanilla 17039 (effectEndurance 0, changeHpRate 100,
# spAttribute 254 [V decoded]); every effectTarget flag on (as vanilla 13462) so the engine call applies it to any
# character; noDead 0, never permanent, never an HP-burn effect, no visual effect. Applied through the engine
# (apply_speffect) on enemies and, last, on the player; nothing writes HP.
NUKE_SPEFFECT = {
    "effectEndurance": 0.0, "changeHpRate": 100.0, "changeHpPoint": 0, "maxHpRate": 1.0,
    # effectTargetSelfTarget is 1 in the clone already (WitchyBND's XML omits it: its paramdef default differs)
    **{f"effectTarget{x}": 1 for x in ("Self", "Friend", "Enemy", "Player", "AI", "Live", "Ghost", "OpposeTarget",
                                        "FriendlyTarget")},
    "noDead": 0, "dontDeleteOnDead": 0, "isHpBurnEffect": 0, "isDestinedDeathHpMult": 0, "vfxId": -1, "stateInfo": 0,
    "iconId": -1, "isDisableNetSync": 1,
}
# The decoded binary must show these (SAFETY_M7, docs/M7_DESIGN.md 3.13 / 6.2).
SAFETY_M7 = {
    "speffect": {"effectEndurance": 0.0, "changeHpRate": 100.0, "noDead": 0, "dontDeleteOnDead": 0, "isHpBurnEffect": 0,
                 "isDestinedDeathHpMult": 0, "vfxId": -1, "stateInfo": 0, "effectTargetSelfTarget": 1},
    "lethal_bullet": {"isHitBothTeam": 0, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1, "HitBulletID": -1},
    "lethal_atk": {"selfTarget": 0, "friendlyTarget": 0, "opposeTarget": 1, "disableGuard": 1, "isAddBaseAtk": 1,
                   "atkPhys": 9999},
    "horizon_bullet": {"atkId_Bullet": -1, "isHitBothTeam": 0, "HitBulletID": -1},
}

# ---- C3 PASS C (docs/C3_DESIGN.md 3.3): co-op visual twins + synced marker rows ----------------------------------------
# A twin mirrors a damaging row's flight / model / SFX with NO attack (atkId_Bullet -1) and no hit bullet; the DLL spawns
# it with network-sync bit 3 (the damaging row never gets bit 3). Ids: 79800000 + (rid - 79100000) for the M6 rows (one
# twin per hitscan gun at its band-0 slot g*1000, the launcher launch g*1000+500, the explosion tiers g*1000+601..603)
# and 79800000 + (rid - 79000000) for the AC-130 tiers. Mirror of mw2er_core::coop_vis::twin_of.
C3_TWIN_BASE = 79_800_000
C3_TWIN_RANGE = (79_800_000, 79_899_999)
C3_NUKE_MARKER_BULLET = 79_000_963
# Synced marker SpEffects, cloned from vanilla row 0 (every field at its paramdef default = no effect), applied to the
# player only. effectEndurance is finite (an orphan marker expires on its own); isDisableNetSync 0 (ER syncs them).
C3_SP_MODE = 79_001_090           # MW2 mode on: vfxId 8060 (vanilla SpEffectVfx with isInvisibleWeapon, no sfx)
C3_SP_GUN_BASE = 79_001_100       # + k for the k-th stable gun index (mw2er_core::coop_vis::GUN_G)
C3_GUN_G = [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 21, 22, 23, 24, 25, 31, 32, 33, 34, 41, 42, 43, 44, 45, 46,
            51, 52, 53, 54, 55]
C3_SP_AC130 = 79_001_150
C3_SP_NUKE = 79_001_190
C3_SP_RANGE = (79_001_090, 79_001_190)
C3_SP_CLONE = 0
C3_SP_MODE_VFX = 8060
C3_MARKER_ENDURANCE_S = 30.0
C3_SP_TARGETS = {f"effectTarget{x}": 1 for x in ("Self", "Friend", "Enemy", "Player", "AI", "Live", "Ghost", "SelfTarget")}
# The decoded binary must show these (SAFETY_C3).
SAFETY_C3 = {
    "twin": {"atkId_Bullet": -1, "HitBulletID": -1, "isHitBothTeam": 0},
    "marker_bullet": {"atkId_Bullet": -1, "HitBulletID": -1, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1},
    "marker_sp": {"isDisableNetSync": 0, "effectEndurance": C3_MARKER_ENDURANCE_S, "changeHpRate": 0.0, "changeHpPoint": 0,
                  "slashDamageCutRate": 1.0, "magicDamageCutRate": 1.0, "noDead": 0, "iconId": -1, "stateInfo": 0},
}


class BuildError(Exception):
    pass


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def rhaz(x):
    """Round half away from zero (Rust f64::round), not Python's banker's rounding."""
    return int(math.floor(x + 0.5)) if x >= 0 else -int(math.floor(-x + 0.5))


def band_damages(damage, min_damage):
    """Mirror of mw2er-core falloff::band_damages (checked against Rust by the real_data test)."""
    hi, lo = max(damage, min_damage), min(damage, min_damage)
    if hi - lo + 1 <= MAX_BANDS:
        return list(range(hi, lo - 1, -1))
    out = []
    for i in range(MAX_BANDS):
        v = rhaz(hi - (hi - lo) * i / (MAX_BANDS - 1))
        if not out or out[-1] != v:
            out.append(v)
    return out


# ---- WitchyBND ---------------------------------------------------------------------------------------

# Review M6 (reproducibility): WitchyBND's parallel mode (its appsettings "Parallel": true) failed 3 of 7 standalone
# repacks of the M6-sized XMLs (LoadXml / File.WriteAllBytes exceptions, exit 255, "completed 2 item(s)"), and two
# concurrent builds shared the fixed work\regulation\ folders (WinError 32). Every call now runs --singlethread, one
# failed call is retried once (logged), and the whole build / self-test / rebaseline holds an exclusive lock file.
WITCHY_RETRIES = 1
LOCK = WORK / ".build_regulation.lock"
LOCK_WAIT_S = 900


def _pid_alive(pid):
    """True when process `pid` exists (Windows OpenProcess; elsewhere os.kill(pid, 0))."""
    if pid <= 0:
        return False
    if os.name == "nt":
        import ctypes
        k32 = ctypes.windll.kernel32
        h = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not h:
            return False
        code = ctypes.c_ulong()
        ok = k32.GetExitCodeProcess(h, ctypes.byref(code))
        k32.CloseHandle(h)
        return bool(ok) and code.value == 259  # STILL_ACTIVE
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


class BuildLock:
    """Exclusive lock on work\\regulation\\ (O_CREAT|O_EXCL file holding the owner pid). Waits for a live owner up to
    LOCK_WAIT_S, takes over a lock whose owner is gone (logged)."""

    def __enter__(self):
        WORK.mkdir(parents=True, exist_ok=True)
        t0 = time.monotonic()
        said = False
        while True:
            try:
                fd = os.open(LOCK, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
                with os.fdopen(fd, "w") as f:
                    f.write(str(os.getpid()))
                return self
            except FileExistsError:
                try:
                    owner = int(LOCK.read_text().strip() or "0")
                except (OSError, ValueError):
                    owner = 0
                if not _pid_alive(owner):
                    print(f"lock: {LOCK.name} held by gone pid {owner}: taken over")
                    try:
                        LOCK.unlink()
                    except OSError:
                        pass
                    continue
                if time.monotonic() - t0 > LOCK_WAIT_S:
                    raise BuildError(f"{LOCK} held by live pid {owner} for > {LOCK_WAIT_S} s (another build running)")
                if not said:
                    print(f"lock: another build_regulation.py (pid {owner}) is running; waiting")
                    said = True
                time.sleep(2.0)

    def __exit__(self, *exc):
        try:
            if LOCK.read_text().strip() == str(os.getpid()):
                LOCK.unlink()
        except OSError:
            pass
        return False


def witchy(args, log_name, expect_items, timeout=600):
    """Runs WitchyBND --passive --singlethread under a pseudo console (it prints nothing to pipes) and asserts the
    'Operation completed on N items' count: WitchyBND exits 0 even when it fails. One failed call is retried."""
    LOGS.mkdir(parents=True, exist_ok=True)
    for attempt in range(WITCHY_RETRIES + 1):
        log = LOGS / (f"{log_name}.log" if attempt == 0 else f"{log_name}.retry{attempt}.log")
        cmd = [sys.executable, "-I", str(CONPTY), "--timeout", str(timeout), "--log", str(log), "--",
               str(WITCHY), "--passive", "--singlethread", *[str(a) for a in args]]
        r = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
        out = r.stdout + r.stderr
        done = re.findall(r"Operation completed on (\d+) items?", out)
        if not (r.returncode != 0 or not done or int(done[-1]) != expect_items or re.search(r"exception", out, re.I)):
            return out
        tail = "\n".join(out.strip().splitlines()[-8:])
        msg = (f"WitchyBND {log_name}: exit {r.returncode}, completed {done[-1] if done else None} "
               f"item(s) (want {expect_items}); log {log}")
        if attempt < WITCHY_RETRIES:
            print(f"RETRY {msg}")
            time.sleep(1.0)
            continue
        raise BuildError(f"{msg}\n{tail}")


def unpack_regulation(parent, source):
    """Copies `source` to parent/regulation.bin and unpacks it to parent/regulation-bin (.param files)."""
    if parent.exists():
        shutil.rmtree(parent)
    parent.mkdir(parents=True)
    shutil.copyfile(source, parent / "regulation.bin")
    witchy(["--unpack", parent / "regulation.bin"], f"unpack_{parent.name}", 1)
    out = parent / "regulation-bin"
    n = len(list(out.glob("*.param")))
    if n != PARAM_COUNT:
        raise BuildError(f"{out}: {n} params after unpack (want {PARAM_COUNT})")
    return out


# ---- inputs and the row plan ---------------------------------------------------------------------------

def load_inputs():
    if not CONFIG.is_file():
        raise BuildError(f"{CONFIG} missing")
    if not WEAPONS_JSON.is_file():
        raise BuildError(f"{WEAPONS_JSON} missing (run convert_weapons.py first)")
    if not AC130_JSON.is_file():
        raise BuildError(f"{AC130_JSON} missing (run convert_ac130.py first)")
    cfg = tomllib.loads(CONFIG.read_text(encoding="utf-8"))
    weapons = json.loads(WEAPONS_JSON.read_text(encoding="utf-8"))
    weapons["ac130"] = json.loads(AC130_JSON.read_text(encoding="utf-8"))
    weapons["arsenal"] = load_arsenal(cfg)
    return cfg, weapons


def load_arsenal(cfg):
    """M6: stats.json of every gun in [arsenal] guns (default: every ok gun of converted/guns/index.json)."""
    idx_p = GUNS_DIR / "index.json"
    if not idx_p.is_file():
        raise BuildError(f"{idx_p} missing (run convert_arsenal.py first)")
    idx = json.loads(idx_p.read_text(encoding="utf-8"))
    names = cfg.get("arsenal", {}).get("guns") or [g["name"] for g in idx["guns"]]
    ok = {g["name"]: g for g in idx["guns"]}
    out = {}
    for n in names:
        g = ok.get(n)
        if g is None or not g.get("ok"):
            raise BuildError(f"arsenal gun {n} is not converted (index.json ok=false or missing)")
        out[n] = json.loads((GUNS_DIR / n / "stats.json").read_text(encoding="utf-8"))
    return out


def dmg_at(inner, outer, r, d):
    """MW2 radius damage, assumed linear [I] (the same function as mw2er_core::ac130_dmg::dmg_at)."""
    if r <= 0:
        return inner
    f = min(1.0, max(0.0, 1.0 - d / r))
    return outer + (inner - outer) * f


def ac130_tiers(gun, k, two_tier):
    """{tier: (radius_m, mw2, atk_phys)} (the Rust ac130_dmg::gun_tiers mirror)."""
    r, i, o = float(gun["explosion_radius_in"]), float(gun["explosion_inner"]), float(gun["explosion_outer"])
    r_m = r * INCH
    out = {"single": (r_m, dmg_at(i, o, r, r / 2), rhaz(dmg_at(i, o, r, r / 2) * k))}
    if two_tier:
        q1, q3 = dmg_at(i, o, r, r / 4), dmg_at(i, o, r, 3 * r / 4)
        out["core"] = (r_m / 2, q1 - q3, rhaz((q1 - q3) * k))
        out["ring"] = (r_m, q3, rhaz(q3 * k))
    for tier, (_, _, atk) in out.items():
        if not (0 < atk <= 65535):
            raise BuildError(f"ac130 {gun['id']} {tier}: atkPhys {atk} out of u16 range")
    return out


C4311_DEF = 110.0   # Limgrave Godrick Soldier physical defence incl. area scaling (NpcParam 43110000 def_phys 100) [I]


def er_damage(atk, defence):
    """Community ER attack-vs-defence curve (the same as analyze_log.er_damage and mw2er_core::ac130_dmg::er_damage;
    [R], info only)."""
    if atk <= 0 or defence <= 0:
        return float(max(atk, 0))
    r = atk / defence
    if r > 8:
        m = 0.9
    elif r > 2.5:
        m = 0.9 - 0.8 / 121 * (r - 8) ** 2
    elif r > 1:
        m = 0.7 - 0.4 / 3 * (r - 2.5) ** 2
    elif r > 0.125:
        m = 0.1 + 19.2 / 49 * (r - 0.125) ** 2
    else:
        m = 0.1
    return atk * m


def inv_er(target, d):
    """atkPhys whose er_damage on defence d is `target` (monotone bisection; mirror of mw2er_core::rows::inv_er)."""
    if not (target > 0 and math.isfinite(target)):
        return 0.0
    lo, hi = 0.0, 65535.0
    if er_damage(hi, d) < target:
        return hi
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if er_damage(mid, d) < target:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)


def planned_atk(mw2, s, d_ref):
    return rhaz(inv_er(s * mw2, d_ref))


def body_mult(st):
    """Review M6 fidelity: ER has no hit locations, so every hit is a body hit at the weapon file's locTorsoUpper when
    it is > 1 (Intervention / Barrett / WA2000 1.5 -> 105, M21 1.1 -> 77; every other gun 1). Launchers: 1.
    Mirror of mw2er_core::arsenal::body_mult (M6_DESIGN deviation 15)."""
    v = float(st["num"].get("locTorsoUpper", 0) or 0)
    return v if st["class"] != "launcher" and 1.0 < v < 10.0 else 1.0


def body_damage(d, mult):
    return d if mult == 1.0 else rhaz(d * mult)


def m6_rid(id_base, g, kind, idx):
    return id_base + g * 1000 + kind * 100 + idx


def launcher_tiers(st, s, d_ref, two_tier=True):
    """{tier: (radius_m, mw2, atk)} of a launcher explosion: ring = inv(s*dmg(3r/4)); core solved so that
    er(core) + er(ring) = s*dmg(r/4); single = inv(s*dmg(r/2)) (docs/M6_DESIGN.md 9.4) [I]."""
    n = st["num"]
    r, i, o = float(n["explosionRadius"]), float(n["explosionInnerDamage"]), float(n["explosionOuterDamage"])
    r_m = r * INCH
    single = dmg_at(i, o, r, r / 2)
    out = {"single": (r_m, single, planned_atk(single, s, d_ref))}
    if two_tier:
        q1, q3 = dmg_at(i, o, r, r / 4), dmg_at(i, o, r, 3 * r / 4)
        ring_atk = planned_atk(q3, s, d_ref)
        core_target = s * q1 - er_damage(ring_atk, d_ref)
        core_atk = rhaz(inv_er(core_target, d_ref))
        out["core"] = (r_m / 2, q1 - q3, core_atk)
        out["ring"] = (r_m, q3, ring_atk)
    for tier, (_, _, atk) in out.items():
        if not (0 < atk <= 65535):
            raise BuildError(f"{st['name']} {tier}: atkPhys {atk} out of u16 range")
    return out


def make_arsenal_plan(cfg, weapons, marker, speed, life, dist, clone_bullet, clone_atk):
    """M6 rows (Bullet, AtkParam_Pc lists of (id, name, overrides, clone)) + the rows.json arsenal block."""
    reg = cfg.get("regulation", {}).get("arsenal", {})
    dm = cfg.get("damage", {})
    mode = dm.get("mode", "flat_inv")
    if mode != "flat_inv":
        raise BuildError(f"damage.mode={mode!r}: only flat_inv builds the arsenal rows (linear is the legacy A/B)")
    s, d_ref = float(dm.get("s", 1.225)), float(dm.get("d_ref", 85.0))
    if not (0.05 <= s <= 20 and 1 <= d_ref <= 2000):
        raise BuildError(f"damage.s={s} / d_ref={d_ref} out of range")
    id_base = int(reg.get("id_base", 79_100_000))
    sfx_mode = reg.get("bullet_sfx", "none")
    if sfx_mode not in ("none", "dagger"):
        raise BuildError(f"regulation.arsenal.bullet_sfx={sfx_mode!r}")
    clone_expl = int(reg.get("clone_explosion_from", 10_030_001))
    tiers2 = int(cfg.get("launchers", {}).get("explosion_tiers", 2)) == 2
    sfx_off = {"sfxId_Hit": -1, "sfxId_Flick": -1} if sfx_mode == "none" else {}
    bullets, atks, guns = [], [], {}
    seen_g = set()
    for wname, st in weapons.get("arsenal", {}).items():
        g = int(st["g"])
        if g in seen_g:
            raise BuildError(f"arsenal: duplicate g {g} ({wname})")
        seen_g.add(g)
        n = st["num"]
        gun_bullet = {"sfxId_Bullet": -1, "life": life, "dist": dist, "gravityInRange": 0, "gravityOutRange": 0,
                      "initVellocity": speed, "maxVellocity": speed, "minVellocity": speed, "accelInRange": 0,
                      "accelOutRange": 0, "homingAngle": 0, "lockShootLimitAng": 0, "HitBulletID": marker, **sfx_off}
        entry = {"g": g, "class": st["class"], "bands": []}
        if st["class"] != "launcher":
            bm = body_mult(st)
            if bm != 1.0:
                entry["body_mult"] = bm
            for b, d in enumerate(band_damages(body_damage(int(n["damage"]), bm), body_damage(int(n["minDamage"]), bm))):
                rid = m6_rid(id_base, g, M6_KIND_BANDS, b)
                atk = planned_atk(d, s, d_ref)
                if not (0 < atk <= 65535):
                    raise BuildError(f"{wname} band {b}: atkPhys {atk} out of u16 range")
                bullets.append((rid, f"MW2 {wname} band {b} dmg {d}", {"atkId_Bullet": rid, **gun_bullet}, clone_bullet))
                atks.append((rid, f"MW2 {wname} dmg {d}", {"atkPhys": atk, **M6_GUN_FLAT}, clone_atk))
                entry["bands"].append({"band": b, "id": rid, "mw2": d, "atk": atk})
        else:
            imp = m6_rid(id_base, g, M6_KIND_LAUNCHER, 0)
            mk = m6_rid(id_base, g, M6_KIND_LAUNCHER, 1)
            impact = int(n["damage"])
            iatk = planned_atk(impact, s, d_ref)
            bullets.append((imp, f"MW2 {wname} impact dmg {impact}", {"atkId_Bullet": imp, **gun_bullet}, clone_bullet))
            atks.append((imp, f"MW2 {wname} impact dmg {impact}", {"atkPhys": iatk, **M6_GUN_FLAT}, clone_atk))
            bullets.append((mk, f"MW2 {wname} marker", {
                "atkId_Bullet": -1, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1, "life": 0.1, "dist": 0,
                "initVellocity": 0, "maxVellocity": 0, "minVellocity": 0, "gravityInRange": 0, "gravityOutRange": 0,
                "accelInRange": 0, "accelOutRange": 0, "homingAngle": 0, "lockShootLimitAng": 0, "HitBulletID": -1,
                "isAttackSFX": 0}, clone_bullet))
            obj, bush = M6_EXPL_OBJ.get(wname, M6_EXPL_OBJ["default"])
            lt = {"impact": {"band": 0, "id": imp, "mw2": impact, "atk": iatk}, "marker_id": mk}
            for tier, (radius_m, mw2, atk) in launcher_tiers(st, s, d_ref, tiers2).items():
                rid = m6_rid(id_base, g, M6_KIND_EXPLOSION, M6_TIER_IDX[tier])
                bullets.append((rid, f"MW2 {wname} explosion {tier}", {
                    "atkId_Bullet": rid, "hitRadius": round(radius_m, 6), "life": 0.07 if tier == "ring" else 0.1,
                    "sfxId_Bullet": M6_EXPL_SFX[tier], "sfxPostureType": AC130_SFX_POSTURE, "HitBulletID": -1}, clone_expl))
                atks.append((rid, f"MW2 {wname} explosion {tier} dmg {mw2:g}", {
                    "atkPhys": atk, "atkFire": 0, "atkObj": obj, "isEnableCalcDamageForBushesObj": bush,
                    "selfTarget": 0, "friendlyTarget": 0, "opposeTarget": 1, **AC130_FLAT_ATK}, clone_expl))
                lt[tier] = {"id": rid, "radius_m": round(radius_m, 6), "mw2": mw2, "atk": atk}
            entry["launcher"] = lt
        guns[wname] = entry
    for rid, *_ in bullets:
        if not (M6_RANGE[0] <= rid <= M6_RANGE[1]):
            raise BuildError(f"arsenal row {rid} outside {M6_RANGE}")
    block = {"mode": mode, "s": s, "d_ref": d_ref, "id_base": id_base, "bullet_sfx": sfx_mode,
             "clone": {"bullet": clone_bullet, "atk": clone_atk, "explosion": clone_expl}, "guns": guns}
    return bullets, atks, block


# Review M6 fidelity: the body-hit damage of the guns whose weapon file has locTorsoUpper > 1 (and two controls).
M6_BODY_KNOWN = {"cheytac_mp": 105, "barrett_mp": 105, "wa2000_mp": 105, "m21_mp": 77, "ump45_mp": 40, "m4_mp": 30}


def arsenal_known_answers(plan):
    """The first band's MW2 damage of the M6_BODY_KNOWN guns present in the plan (and its atk from the inversion)."""
    ar = plan["arsenal"]
    out = []
    for name, want in M6_BODY_KNOWN.items():
        g = ar["guns"].get(name)
        if g is None:
            continue
        b0 = g["bands"][0] if g["bands"] else None
        if b0 is None or b0["mw2"] != want or b0["atk"] != planned_atk(want, ar["s"], ar["d_ref"]):
            out.append(f"arsenal known answer: {name} first band {b0} (want mw2 {want}, body multiplier locTorsoUpper)")
    return out


def is_m6_row(rid):
    return M6_RANGE[0] <= rid <= M6_RANGE[1]


def m6_row_kind(rid, plan):
    """'gun' | 'marker' | 'expl' of an M6 row id."""
    _g, rest = divmod(rid - plan["arsenal"]["id_base"], 1000)
    kind, idx = divmod(rest, 100)
    if kind == M6_KIND_EXPLOSION:
        return "expl"
    if kind == M6_KIND_LAUNCHER and idx == 1:
        return "marker"
    return "gun"


def make_plan(cfg, weapons):
    reg = cfg.get("regulation", {})
    k = float(cfg.get("damage", {}).get("k", 4.0))
    if not (0 < k <= 100):
        raise BuildError(f"damage.k={k} out of range")
    id_base = int(reg.get("id_base", 79_000_000))
    marker = int(reg.get("marker_id", 79_000_900))
    speed = float(reg.get("bullet_speed_mps", 999.0))
    life = float(reg.get("bullet_life_s", 0.15))
    marker_life = float(reg.get("marker_life_s", 0.1))
    clone_bullet = int(reg.get("clone_bullet_from", 10_170_000))
    clone_atk = int(reg.get("clone_atk_from", 10_170_000))
    slots = reg.get("weapons", {"ump45_mp": 1})
    if not slots:
        raise BuildError("regulation.weapons is empty")
    dist = math.ceil(speed * life - 1e-9)  # in-range parameters apply over the whole flight (150 m)
    bullets, atks, rows_weapons = [], [], {}
    for wname, slot in sorted(slots.items(), key=lambda kv: kv[1]):
        if not (1 <= int(slot) <= 8):
            raise BuildError(f"regulation.weapons.{wname}={slot}: slot must be 1..8 (9 is the marker)")
        w = weapons.get("weapons", {}).get(wname)
        if w is None:
            raise BuildError(f"{wname} not in converted/weapons.json")
        bands = band_damages(int(w["mw2"]["damage"]), int(w["mw2"]["minDamage"]))
        lst = []
        for b, d in enumerate(bands):
            rid = id_base + int(slot) * 100 + b
            atk_phys = rhaz(d * k)
            if not (0 < atk_phys <= 65535):
                raise BuildError(f"{wname} band {b}: atkPhys {atk_phys} out of u16 range")
            bullets.append((rid, f"MW2 {wname} band {b} dmg {d}", {
                "atkId_Bullet": rid, "sfxId_Bullet": -1, "life": life, "dist": dist,
                "gravityInRange": 0, "gravityOutRange": 0, "initVellocity": speed, "maxVellocity": speed,
                "minVellocity": speed, "accelInRange": 0, "accelOutRange": 0, "homingAngle": 0,
                "lockShootLimitAng": 0, "HitBulletID": marker}, clone_bullet))
            atks.append((rid, f"MW2 {wname} dmg {d}", {"atkPhys": atk_phys}, clone_atk))
            lst.append({"band": b, "id": rid, "mw2_damage": d, "atk_phys": atk_phys})
        rows_weapons[wname] = {"slot": int(slot), "bands": lst}
    bullets.append((marker, "MW2 impact marker", {
        "atkId_Bullet": -1, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1, "life": marker_life,
        "dist": 0, "initVellocity": 0, "maxVellocity": 0, "minVellocity": 0, "gravityInRange": 0,
        "gravityOutRange": 0, "accelInRange": 0, "accelOutRange": 0, "homingAngle": 0, "lockShootLimitAng": 0,
        "HitBulletID": -1, "isAttackSFX": 0}, clone_bullet))
    # ---- M4: the AC-130 explosion rows + the protection SpEffect (docs/M4_DESIGN.md section 8) ----
    ra = reg.get("ac130", {})
    k_ac = float(ra.get("k", 0.65))
    if not (0 < k_ac <= 100):
        raise BuildError(f"regulation.ac130.k={k_ac} out of range")
    ac_clone_b = int(ra.get("clone_bullet_from", 10_030_001))
    ac_clone_a = int(ra.get("clone_atk_from", 10_030_001))
    sp_clone = int(ra.get("clone_speffect_from", 1540))
    protect = int(ra.get("protect_speffect_id", 79_000_950))
    # impact visuals (AC130_SFX_DEFAULT): core/single = the burst, ring = the dust/debris layer; -1 = none
    sfx = {key: int(ra.get(f"sfx_{key}", v)) for key, v in AC130_SFX_DEFAULT.items()}
    for key, v in sfx.items():
        if v != -1 and not (0 < v < 100_000_000):
            raise BuildError(f"regulation.ac130.sfx_{key}={v}: -1 or an SFX id")
    endurance = float(ra.get("protect_endurance_s", PROTECT_ENDURANCE_DEFAULT_S))
    if not (PROTECT_ENDURANCE_RANGE_S[0] <= endurance <= PROTECT_ENDURANCE_RANGE_S[1]):
        raise BuildError(f"regulation.ac130.protect_endurance_s={endurance} outside {PROTECT_ENDURANCE_RANGE_S}")
    guns = {g["id"]: g for g in weapons.get("ac130", {}).get("guns", [])}
    if sorted(guns) != sorted(AC130_GUN_SLOT):
        raise BuildError(f"weapons_ac130.json guns {sorted(guns)} (want 105/40/25)")
    scale = {gid: float(ra.get(f"scale_{gid}", AC130_SCALE_DEFAULT[gid])) for gid in AC130_GUN_SLOT}
    for gid, v in scale.items():
        if not (0.05 <= v <= 20.0):
            raise BuildError(f"regulation.ac130.scale_{gid}={v} outside 0.05..20")
    ac_rows = {}
    for gid, slot in AC130_GUN_SLOT.items():
        tiers = ac130_tiers(guns[gid], k_ac * scale[gid], AC130_TWO_TIER[gid])
        out = {}
        for tier in ("core", "ring", "single"):
            if tier not in tiers:
                continue
            radius_m, mw2, atk = tiers[tier]
            rid = 79_000_000 + slot * 100 + (1 if gid == "25" else AC130_TIER_SLOT[tier])
            row_life = 0.07 if (tier == "ring" or gid == "25") else 0.1
            row_sfx = sfx[f"{gid}_ring"] if tier == "ring" else sfx[gid]
            bullets.append((rid, f"MW2 AC-130 {gid}mm {tier}", {
                "atkId_Bullet": rid, "hitRadius": round(radius_m, 6), "life": row_life, "sfxId_Bullet": row_sfx,
                "sfxPostureType": AC130_SFX_POSTURE, "HitBulletID": -1}, ac_clone_b))
            st, sa, lvl, kb = AC130_TUNING[gid][tier][:4]
            obj, bushes, _beats = AC130_OBJECT[gid][tier]
            atks.append((rid, f"MW2 AC-130 {gid}mm {tier} dmg {mw2:g}", {
                "atkPhys": atk, "atkFire": 0, "atkStam": st, "atkSuperArmor": sa, "dmgLevel": lvl,
                "knockbackDist": kb, "atkObj": obj, "isEnableCalcDamageForBushesObj": bushes, **AC130_FLAT_ATK},
                ac_clone_a))
            out[tier] = {"id": rid, "radius_m": round(radius_m, 6), "mw2": mw2, "atk_phys": atk, "sfx": row_sfx,
                         "atk_obj": obj, "obj_bushes": bushes, "sfx_posture": AC130_SFX_POSTURE}
        ac_rows[gid] = out
    speffects = [(protect, "MW2 AC-130 gunship protection", {**SPEFFECT_OVERRIDES, "effectEndurance": endurance}, sp_clone)]
    # ---- M6: the arsenal rows (docs/M6_DESIGN.md section 9) ----
    m6_b, m6_a, arsenal = make_arsenal_plan(cfg, weapons, marker, speed, life, dist, clone_bullet, clone_atk)
    bullets += m6_b
    atks += m6_a
    # ---- M7: the nuke rows (docs/M7_DESIGN.md 3.13) ----
    rn = reg.get("nuke", {})
    lethal_sp = int(rn.get("lethal_speffect_id", 79_000_960))
    nuke_sp_clone = int(rn.get("clone_speffect_from", 17_039))
    lethal_b = int(rn.get("lethal_bullet_id", 79_000_961))
    horizon_b = int(rn.get("horizon_bullet_id", 79_000_962))
    horizon_sfx = int(rn.get("horizon_sfx", 524_028))
    if horizon_sfx != -1 and not (0 < horizon_sfx < 100_000_000):
        raise BuildError(f"regulation.nuke.horizon_sfx={horizon_sfx}: -1 or an SFX id")
    for rid in (lethal_sp, lethal_b, horizon_b):
        if not (79_000_000 <= rid <= 79_099_999) or AC130_RANGE[0] <= rid <= AC130_RANGE[1]:
            raise BuildError(f"regulation.nuke row id {rid} outside 79000000..79099999 or inside the AC-130 range")
    speffects.append((lethal_sp, "MW2 Tactical Nuke (one-shot 100% max HP)", dict(NUKE_SPEFFECT), nuke_sp_clone))
    bullets.append((lethal_b, "MW2 Tactical Nuke lethal bullet", {
        "atkId_Bullet": lethal_b, "hitRadius": 0.5, "life": 0.1, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1,
        "HitBulletID": -1}, ac_clone_b))
    atks.append((lethal_b, "MW2 Tactical Nuke lethal", {
        **AC130_FLAT_ATK, "atkPhys": 9999, "atkFire": 0, "atkObj": 0, "spEffectId0": lethal_sp, "selfTarget": 0,
        "friendlyTarget": 0, "opposeTarget": 1}, ac_clone_a))
    bullets.append((horizon_b, "MW2 Tactical Nuke horizon explosion (visual)", {
        "atkId_Bullet": -1, "hitRadius": 0.1, "life": 0.1, "sfxId_Bullet": horizon_sfx, "sfxId_Hit": -1, "sfxId_Flick": -1,
        "sfxPostureType": AC130_SFX_POSTURE, "HitBulletID": -1}, ac_clone_b))
    # ---- C3 PASS C: the co-op twins, the nuke marker bullet and the synced marker SpEffects (docs/C3_DESIGN.md 3.3) ----
    c3_b, c3_s, coop = make_coop_plan(bullets, ac_clone_b)
    bullets += c3_b
    speffects += c3_s
    speffects.sort(key=lambda r: r[0])
    bullets.sort(key=lambda r: r[0])
    atks.sort(key=lambda r: r[0])
    ids = [r[0] for r in bullets]
    if len(set(ids)) != len(ids) or len({r[0] for r in speffects}) != len(speffects):
        raise BuildError("duplicate row ids in the plan")
    for rid in ids + [r[0] for r in speffects]:
        if not (ID_RANGE[0] <= rid <= ID_RANGE[1]):
            raise BuildError(f"row id {rid} outside the reserved range {ID_RANGE}")
    for g in ac_rows.values():
        for r in g.values():
            if not (AC130_RANGE[0] <= r["id"] <= AC130_RANGE[1]):
                raise BuildError(f"ac130 row {r['id']} outside {AC130_RANGE}")
    return {
        "k": k, "id_base": id_base, "marker_id": marker, "speed": speed, "life": life, "dist": dist,
        "clone": {"Bullet": clone_bullet, "AtkParam_Pc": clone_atk, "SpEffectParam": sp_clone},
        "rows": {"Bullet": bullets, "AtkParam_Pc": atks, "SpEffectParam": speffects}, "weapons": rows_weapons,
        "ac130": {"k": k_ac, "gun_scale": scale, "atk_mode": "flat", "protect_speffect_id": protect,
                  "protect_endurance_s": endurance,
                  "clone": {"bullet": ac_clone_b, "atk": ac_clone_a, "speffect": sp_clone}, "guns": ac_rows},
        "arsenal": arsenal,
        "nuke": {"lethal_speffect_id": lethal_sp, "lethal_bullet_id": lethal_b, "horizon_bullet_id": horizon_b,
                 "horizon_sfx": horizon_sfx, "clone": {"speffect": nuke_sp_clone, "bullet": ac_clone_b, "atk": ac_clone_a}},
        "coop": coop,
    }


def c3_twin_of(rid):
    """Mirror of mw2er_core::coop_vis::twin_of (None = no twin)."""
    if M6_RANGE[0] <= rid <= M6_RANGE[1]:
        g, r = divmod(rid - M6_RANGE[0], 1000)
        kind, idx = divmod(r, 100)
        if g not in C3_GUN_G:
            return None
        if kind == M6_KIND_BANDS:
            return C3_TWIN_BASE + g * 1000
        if kind == M6_KIND_LAUNCHER and idx == 0:
            return C3_TWIN_BASE + g * 1000 + 500
        if kind == M6_KIND_EXPLOSION and 1 <= idx <= 3:
            return C3_TWIN_BASE + g * 1000 + 600 + idx
        return None
    if AC130_RANGE[0] <= rid <= AC130_RANGE[1]:
        slot, tier = divmod(rid - 79_000_000, 100)
        if 2 <= slot <= 4 and 1 <= tier <= 3:
            return C3_TWIN_BASE + slot * 100 + tier
    return None


def make_coop_plan(bullets, ac_clone_b):
    """C3 PASS C rows: (Bullet rows, SpEffect rows, rows.json coop block). Every twin is its source row's clone source +
    the source's overrides, with atkId_Bullet -1 and HitBulletID -1 (no damage, no impact marker)."""
    twins, seen = [], {}
    for rid, name, ov, clone in sorted(bullets, key=lambda r: r[0]):
        tid = c3_twin_of(rid)
        if tid is None or tid in seen:
            continue
        if ov.get("atkId_Bullet", -1) == -1:
            continue  # an inert row (a marker) needs no twin
        seen[tid] = rid
        tov = dict(ov)
        tov["atkId_Bullet"] = -1
        tov["HitBulletID"] = -1
        twins.append((tid, f"MW2 co-op twin of {rid} ({name.removeprefix('MW2 ')})"[:120], tov, clone))
    for tid, _n, _o, _c in twins:
        if not (C3_TWIN_RANGE[0] <= tid <= C3_TWIN_RANGE[1]):
            raise BuildError(f"twin {tid} outside {C3_TWIN_RANGE}")
    marker_b = (C3_NUKE_MARKER_BULLET, "MW2 co-op nuke marker (synced, visual-only)", {
        "atkId_Bullet": -1, "hitRadius": 0.1, "life": 0.5, "sfxId_Bullet": -1, "sfxId_Hit": -1, "sfxId_Flick": -1,
        "HitBulletID": -1}, ac_clone_b)
    sp = []

    def marker(sid, name, vfx=-1):
        sp.append((sid, name, {"effectEndurance": C3_MARKER_ENDURANCE_S, "isDisableNetSync": 0, "vfxId": vfx, "iconId": -1,
                               **C3_SP_TARGETS}, C3_SP_CLONE))
    marker(C3_SP_MODE, "MW2 co-op marker: MW2 mode (hides the ER weapon)", C3_SP_MODE_VFX)
    for k, g in enumerate(C3_GUN_G):
        marker(C3_SP_GUN_BASE + k, f"MW2 co-op marker: gun g={g}")
    marker(C3_SP_AC130, "MW2 co-op marker: AC-130 ride")
    marker(C3_SP_NUKE, "MW2 co-op marker: Tactical Nuke")
    for sid, *_ in sp:
        if not (C3_SP_RANGE[0] <= sid <= C3_SP_RANGE[1]):
            raise BuildError(f"marker SpEffect {sid} outside {C3_SP_RANGE}")
    block = {"twins": [{"id": t[0], "src": seen[t[0]]} for t in twins], "nuke_marker_bullet": C3_NUKE_MARKER_BULLET,
             "sp_mode": C3_SP_MODE, "sp_gun_base": C3_SP_GUN_BASE, "sp_gun_count": len(C3_GUN_G), "sp_ac130": C3_SP_AC130,
             "sp_nuke": C3_SP_NUKE, "sp_mode_vfx": C3_SP_MODE_VFX, "marker_endurance_s": C3_MARKER_ENDURANCE_S,
             "sp_clone": C3_SP_CLONE}
    return twins + [marker_b], sp, block


def coop_row_kind(stem, rid, plan):
    """The C3 row kind (SAFETY_C3 key) or None."""
    c = plan.get("coop") or {}
    if stem == "Bullet" and C3_TWIN_RANGE[0] <= rid <= C3_TWIN_RANGE[1]:
        return "twin"
    if stem == "Bullet" and rid == c.get("nuke_marker_bullet"):
        return "marker_bullet"
    if stem == "SpEffectParam" and C3_SP_RANGE[0] <= rid <= C3_SP_RANGE[1]:
        return "marker_sp"
    return None


def nuke_row_kind(stem, rid, plan):
    """The M7 row kind (SAFETY_M7 key) or None."""
    n = plan.get("nuke") or {}
    if stem == "SpEffectParam" and rid == n.get("lethal_speffect_id"):
        return "speffect"
    if rid == n.get("lethal_bullet_id"):
        return "lethal_bullet" if stem == "Bullet" else "lethal_atk"
    if stem == "Bullet" and rid == n.get("horizon_bullet_id"):
        return "horizon_bullet"
    return None


# ---- XML editing (text level: every vanilla line stays byte-identical) -------------------------------

ROW_RE = re.compile(r'^<row id="(-?\d+)"')
ATTR_RE = re.compile(r'([A-Za-z_][\w]*)="([^"]*)"')
FIELD_RE = re.compile(r'^<field name="([^"]+)"')


def fmt_value(v):
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return str(int(v)) if v.is_integer() else repr(v)
    return str(v)


def edit_xml(path, new_rows):
    """Inserts `new_rows` [(id, name, overrides, clone_id)] (sorted) as one block; each row is its clone's XML
    attributes + the overrides. Every vanilla line stays byte-identical."""
    raw = path.read_bytes()
    bom = raw.startswith(BOM)
    lines = raw.decode("utf-8-sig").splitlines(keepends=True)
    nl = "\r\n" if lines[0].endswith("\r\n") else "\n"
    fields = {m.group(1) for line in lines if (m := FIELD_RE.match(line))}
    rows = [(i, int(m.group(1))) for i, line in enumerate(lines) if (m := ROW_RE.match(line))]
    if not rows:
        raise BuildError(f"{path.name}: no rows")
    bases = {}
    for clone_id in sorted({r[3] for r in new_rows}):
        clone = [lines[i] for i, rid in rows if rid == clone_id]
        if len(clone) != 1:
            raise BuildError(f"{path.name}: clone source {clone_id} found {len(clone)} times")
        attrs = ATTR_RE.findall(clone[0])
        if not attrs or attrs[0][0] != "id":
            raise BuildError(f"{path.name}: unexpected row syntax for {clone_id}")
        bases[clone_id] = [(k, v) for k, v in attrs[1:] if k not in ("paramdexName", "name")]
    new_ids = [r[0] for r in new_rows]
    if new_ids != sorted(new_ids):
        raise BuildError(f"{path.name}: the planned rows are not sorted")
    lo, hi = min(new_ids), max(new_ids)
    clash = [rid for _, rid in rows if lo <= rid <= hi or ID_RANGE[0] <= rid <= ID_RANGE[1]]
    if clash:
        raise BuildError(f"{path.name}: vanilla rows already use ids in our range: {clash[:5]}")
    out_lines = []
    for rid, name, overrides, clone_id in new_rows:
        unknown = [k for k in overrides if k not in fields]
        if unknown:
            raise BuildError(f"{path.name}: override fields not in the paramdef: {unknown}")
        d = dict(bases[clone_id])
        for k, v in overrides.items():
            d[k] = fmt_value(v)
        parts = [f'id="{rid}"', f'name="{name}"'] + [f'{k}="{v}"' for k, v in d.items()]
        out_lines.append("<row " + " ".join(parts) + " />" + nl)
    ins = next((i for i, rid in rows if rid > hi), rows[-1][0] + 1)
    lines[ins:ins] = out_lines
    data = "".join(lines).encode("utf-8")
    path.write_bytes((BOM if bom else b"") + data)
    return len(out_lines)


# ---- verification ---------------------------------------------------------------------------------------

def decode_rows(param, ids, defs):
    _, layout, size = param_rows.load_def(defs, param["type"], REGULATION_VERSION)
    if param["rows"] and size != param["row_size"]:
        raise BuildError(f"{param['type']}: def row size {size} != binary {param['row_size']}")
    by_id = {}
    for rid, name, data in param["rows"]:
        if rid in ids and rid not in by_id:
            by_id[rid] = (name or "", param_rows.decode(data, layout))
    return by_id, {f[0] for f in layout}


def same_value(got, want):
    if isinstance(want, float) or isinstance(got, float):
        try:
            return abs(float(got) - float(want)) <= 1e-6 * max(1.0, abs(float(want)))
        except (TypeError, ValueError):
            return False
    return got == want


MANIFEST = "_witchy-bnd4.xml"
# WitchyBND leaves out an attribute equal to ITS paramdef default; where that default differs from the defs we decode
# with, an absent attribute means WitchyBND's value (M7 noted it for effectTargetSelfTarget: WitchyBND's default is 1).
# The binary itself is still checked field by field against the plan in verify().
XML_DEFAULT_QUIRKS = {("SpEffectParam", "effectTargetSelfTarget"): 1}


def is_ac130_row(stem, rid, plan):
    """The M4 rows: the AC-130 explosion rows (Bullet/AtkParam_Pc) and the protection SpEffect."""
    if stem == "SpEffectParam":
        return rid == plan["ac130"]["protect_speffect_id"]
    return AC130_RANGE[0] <= rid <= AC130_RANGE[1]


def ac130_gun_tier(rid):
    """(gun id, tier) of an AC-130 explosion row id (79000000 + slot*100 + tier; the 25 mm's single row is tier 1)."""
    slot, t = divmod(rid - 79_000_000, 100)
    gid = {v: k for k, v in AC130_GUN_SLOT.items()}[slot]
    if gid == "25":
        return gid, "single"
    return gid, {v: k for k, v in AC130_TIER_SLOT.items()}[t]


ASSET_PARAM = "AssetEnvironmentGeometryParam"


def asset_break_report(pristine):
    """Info + guard (section 20): how many vanilla damage-breakable assets (hp > 0) each AC-130 tier's atkObj beats,
    from the vanilla AssetEnvironmentGeometryParam (defense; isBushesForDamage needs the bush flag on the attack)."""
    p = param_diff.read_param(pristine / f"{ASSET_PARAM}.param")
    _, layout, size = param_rows.load_def(param_rows.DEFAULT_DEFS, p["type"], REGULATION_VERSION)
    if size != p["row_size"]:
        raise BuildError(f"{ASSET_PARAM}: def row size {size} != binary {p['row_size']}")
    assets = []
    for _rid, _name, data in p["rows"]:
        r = param_rows.decode(data, layout)
        if r["hp"] > 0:
            assets.append((r["defense"], r["isBushesForDamage"]))
    out = {}
    for gid, tiers in AC130_OBJECT.items():
        for tier, (obj, bushes, beats) in tiers.items():
            n = sum(1 for d, b in assets if obj > d and (bushes or not b))
            # the tier must break every asset of the defence it is meant to beat
            short = sum(1 for d, b in assets if d <= beats and obj <= d)
            if short:
                raise BuildError(f"ac130 {gid}mm {tier}: atkObj {obj} fails {short} asset(s) with defence <= {beats}")
            out[f"{gid}/{tier}"] = n
    return len(assets), out


def verify(pristine, check, plan, overrides_paths=None, skip_untouched=False, atk_off_by=0):
    """Returns (problems, report). `overrides_paths` maps a param stem (edited or untouched) or MANIFEST to a
    replacement file in place of the one in `check` (self-test mutations); `atk_off_by` shifts the expected
    atkPhys of the first band (self-test)."""
    overrides_paths = overrides_paths or {}

    def built(key, name):
        return pathlib.Path(overrides_paths[key]) if key in overrides_paths else check / name

    problems, report = [], {"untouched_identical": 0, "edited": {}, "rows_checked": 0}
    names_a = sorted(p.name for p in pristine.glob("*.param"))
    names_b = sorted(p.name for p in check.glob("*.param"))
    if names_a != names_b or len(names_a) != PARAM_COUNT:
        problems.append(f"param sets differ: {len(names_a)} vs {len(names_b)} (want {PARAM_COUNT} each)")
    # (a) untouched params byte-identical; edited params differ only by exactly the added rows.
    for name in names_a:
        stem = name[:-len(".param")]
        if stem in EDITED:
            continue
        if skip_untouched and stem not in overrides_paths:
            continue
        b = built(stem, name)
        if not b.is_file() or (pristine / name).read_bytes() != b.read_bytes():
            problems.append(f"{stem}: not byte-identical to vanilla")
        else:
            report["untouched_identical"] += 1
    defs = param_rows.DEFAULT_DEFS
    for stem in EDITED:
        try:
            a = param_diff.read_param(pristine / f"{stem}.param")
            b = param_diff.read_param(built(stem, f"{stem}.param"))
        except (param_diff.ParamError, ValueError, OSError) as e:
            problems.append(f"{stem}: cannot parse ({e})")
            continue
        rows = plan["rows"][stem]
        ids = {r[0] for r in rows}
        p = param_diff.check_added(a, b, ids)
        problems += [f"{stem}: {x}" for x in p]
        report["edited"][stem] = {"rows_vanilla": len(a["rows"]), "rows_built": len(b["rows"]), "added": len(ids)}
        # (b) field-level: each added row == its clone source except the intended overrides.
        clone_ids = {r[3] for r in rows}
        src, _ = decode_rows(a, clone_ids, defs)
        missing_clone = sorted(c for c in clone_ids if c not in src)
        if missing_clone:
            problems.append(f"{stem}: clone source(s) {missing_clone} missing in vanilla")
            continue
        got_rows, field_names = decode_rows(b, ids, defs)
        for idx, (rid, name, ov, clone_id) in enumerate(rows):
            if rid not in got_rows:
                problems.append(f"{stem}[{rid}]: row missing in the built binary")
                continue
            got_name, got = got_rows[rid]
            want = dict(src[clone_id][1])
            for k, v in ov.items():
                if k not in field_names:
                    problems.append(f"{stem}[{rid}]: override field {k} not in the paramdef")
                want[k] = v
            if stem == "AtkParam_Pc" and idx == 0 and atk_off_by:
                want["atkPhys"] = want["atkPhys"] + atk_off_by
            bad = [f"{k}={got.get(k)!r} (want {w!r})" for k, w in want.items() if not same_value(got.get(k), w)]
            if bad:
                problems.append(f"{stem}[{rid}]: " + ", ".join(bad[:6]))
            if got_name != name:
                problems.append(f"{stem}[{rid}]: binary name {got_name!r} (want {name!r})")
            # M6 safety assertions on the decoded binary (docs/M6_DESIGN.md 9.6 SAFETY_M6)
            if stem in ("Bullet", "AtkParam_Pc") and is_m6_row(rid):
                kind = m6_row_kind(rid, plan)
                key = {"gun": "gun", "expl": "expl", "marker": "marker"}[kind] + ("_bullet" if stem == "Bullet" else "_atk")
                for k, w in SAFETY_M6.get(key, {}).items():
                    if not same_value(got.get(k), w):
                        problems.append(f"{stem}[{rid}]: SAFETY_M6 {k}={got.get(k)!r} (must be {w!r})")
                if stem == "Bullet" and kind == "gun":
                    if got.get("HitBulletID") != plan["marker_id"]:
                        problems.append(f"{stem}[{rid}]: SAFETY_M6 HitBulletID={got.get('HitBulletID')!r} (the marker)")
                    if plan["arsenal"]["bullet_sfx"] == "none" and (got.get("sfxId_Hit") != -1 or got.get("sfxId_Flick") != -1):
                        problems.append(f"{stem}[{rid}]: SAFETY_M6 sfxId_Hit/Flick {got.get('sfxId_Hit')}/{got.get('sfxId_Flick')} "
                                        "(bullet_sfx none: -1, the bug-2 likely fix)")
            # M7 safety assertions on the decoded binary (docs/M7_DESIGN.md 3.13)
            nk = nuke_row_kind(stem, rid, plan)
            if nk:
                for k, w in SAFETY_M7[nk].items():
                    if not same_value(got.get(k), w):
                        problems.append(f"{stem}[{rid}]: SAFETY_M7 {k}={got.get(k)!r} (must be {w!r})")
                if nk == "lethal_atk" and got.get("spEffectId0") != plan["nuke"]["lethal_speffect_id"]:
                    problems.append(f"{stem}[{rid}]: SAFETY_M7 spEffectId0={got.get('spEffectId0')!r} (the lethal SpEffect)")
                # C3 PASS C: the lethal effect is never synced (79000960 stays isDisableNetSync 1)
                if nk == "speffect" and got.get("isDisableNetSync") != 1:
                    problems.append(f"{stem}[{rid}]: SAFETY_C3 isDisableNetSync={got.get('isDisableNetSync')!r} (the lethal SpEffect must never sync)")
            # C3 PASS C safety assertions on the decoded binary (docs/C3_DESIGN.md 3.3)
            ck = coop_row_kind(stem, rid, plan)
            if ck:
                for k, w in SAFETY_C3[ck].items():
                    if not same_value(got.get(k), w):
                        problems.append(f"{stem}[{rid}]: SAFETY_C3 {k}={got.get(k)!r} (must be {w!r})")
                if ck == "marker_sp":
                    want_vfx = C3_SP_MODE_VFX if rid == C3_SP_MODE else -1
                    if got.get("vfxId") != want_vfx:
                        problems.append(f"{stem}[{rid}]: SAFETY_C3 vfxId={got.get('vfxId')!r} (want {want_vfx})")
                if ck == "twin":
                    tsrc = next((s["src"] for s in plan["coop"]["twins"] if s["id"] == rid), None)
                    if tsrc is None or c3_twin_of(tsrc) != rid:
                        problems.append(f"{stem}[{rid}]: SAFETY_C3 twin id rule broken (src {tsrc})")
            # M4 safety assertions on the decoded binary (section 8: self-damage fields, protection cuts)
            if is_ac130_row(stem, rid, plan):
                for k, w in SAFETY[stem].items():
                    if not same_value(got.get(k), w):
                        problems.append(f"{stem}[{rid}]: SAFETY {k}={got.get(k)!r} (must be {w!r})")
                if stem == "AtkParam_Pc":
                    # object destruction (section 20): atkObj must beat the asset defence its tier is meant to break
                    gid, tier = ac130_gun_tier(rid)
                    _obj, bushes, beats = AC130_OBJECT[gid][tier]
                    if not (isinstance(got.get("atkObj"), int) and got["atkObj"] > beats):
                        problems.append(f"{stem}[{rid}]: OBJECT atkObj={got.get('atkObj')!r} must exceed asset defence {beats} "
                                        f"({gid}mm {tier})")
                    if got.get("isEnableCalcDamageForBushesObj") != bushes:
                        problems.append(f"{stem}[{rid}]: OBJECT isEnableCalcDamageForBushesObj="
                                        f"{got.get('isEnableCalcDamageForBushesObj')!r} (want {bushes})")
                if stem == "Bullet" and got.get("sfxPostureType") != AC130_SFX_POSTURE:
                    problems.append(f"{stem}[{rid}]: SFX sfxPostureType={got.get('sfxPostureType')!r} (want {AC130_SFX_POSTURE})")
                if stem == "SpEffectParam":
                    # the protection must expire on its own (review M4 major 2): finite, the planned value
                    want_e = plan["ac130"]["protect_endurance_s"]
                    got_e = got.get("effectEndurance")
                    if not (isinstance(got_e, (int, float)) and got_e > 0 and same_value(got_e, want_e)):
                        problems.append(f"{stem}[{rid}]: SAFETY effectEndurance={got_e!r} (must be finite {want_e!r})")
            report["rows_checked"] += 1
    # (c) the BND manifest (file list, compression, version) is unchanged. A missing manifest on either side
    # is a problem in itself, not a reason to skip the comparison.
    wa, wb = pristine / MANIFEST, built(MANIFEST, MANIFEST)
    if not wa.is_file():
        problems.append(f"pristine {MANIFEST} missing")
    elif not wb.is_file():
        problems.append(f"{MANIFEST} missing from the read-back unpack")
    elif wa.read_bytes() != wb.read_bytes():
        problems.append(f"{MANIFEST} differs from the pristine one")
    return problems, report


def verify_xml_readback(check, plan, xml_paths=None):
    """(d) WitchyBND's own XML of the built params shows our rows with the intended override values.
    WitchyBND leaves out attributes equal to the paramdef default, so a missing attribute must equal the
    default; anything else (or a non-number) is a problem. `xml_paths` maps a stem to a replacement XML
    (self-test)."""
    xml_paths = xml_paths or {}
    problems = []
    for stem in EDITED:
        xml = pathlib.Path(xml_paths.get(stem, check / f"{stem}.param.xml"))
        if not xml.is_file():
            problems.append(f"{stem}.param.xml was not produced by the read-back unpack")
            continue
        try:
            ptype = param_diff.read_param(check / f"{stem}.param")["type"]
            defaults = param_rows.load_defaults(param_rows.DEFAULT_DEFS, ptype, REGULATION_VERSION)
        except (param_diff.ParamError, ValueError, OSError, SystemExit) as e:
            problems.append(f"{stem}: cannot load the paramdef defaults ({e})")
            continue
        found = {}
        for line in xml.read_text(encoding="utf-8-sig").splitlines():
            m = ROW_RE.match(line)
            if m and int(m.group(1)) in {r[0] for r in plan["rows"][stem]}:
                found[int(m.group(1))] = dict(ATTR_RE.findall(line))
        for rid, name, ov, _clone in plan["rows"][stem]:
            a = found.get(rid)
            if a is None:
                problems.append(f"{stem}.param.xml: row {rid} missing")
                continue
            for k, v in ov.items():
                if k in a:
                    try:
                        got = float(a[k])
                    except ValueError:
                        problems.append(f"{stem}.param.xml[{rid}]: {k}={a[k]!r} is not a number (want {v})")
                        continue
                    if not same_value(got, float(v)):
                        problems.append(f"{stem}.param.xml[{rid}]: {k}={a[k]} (want {v})")
                elif k not in defaults:
                    problems.append(f"{stem}.param.xml[{rid}]: {k} absent and not a paramdef field")
                elif (stem, k) in XML_DEFAULT_QUIRKS and same_value(XML_DEFAULT_QUIRKS[(stem, k)], float(v)):
                    pass  # WitchyBND's own paramdef default differs from ours: absent = its default
                elif not same_value(defaults[k], float(v)):
                    problems.append(f"{stem}.param.xml[{rid}]: {k} absent (= default {defaults[k]}) but want {v}")
    return problems


# ---- steps ------------------------------------------------------------------------------------------------

def preflight():
    if not WITCHY.is_file():
        raise BuildError(f"{WITCHY} missing")
    vt = WITCHY_APP / "Assets" / "Paramdex" / "ER" / "Upgrader" / "version.txt"
    v = vt.read_text(encoding="utf-8").strip() if vt.is_file() else None
    if v != PARAMDEX_VERSION:
        raise BuildError(f"Paramdex upgrader version is {v!r} (want {PARAMDEX_VERSION}); a re-extracted "
                         "WitchyBND undoes the bump")
    ov = WITCHY_APP / "appsettings.override.json"
    try:
        s = json.loads(ov.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        s = {}
    if s.get("Offline") is not True or s.get("Flexible") is not False:
        raise BuildError(f"{ov} must set Offline=true and Flexible=false")


def source_check():
    if not GAME_REGULATION.is_file():
        raise BuildError(f"game regulation not found: {GAME_REGULATION}")
    g = sha256_file(GAME_REGULATION)
    if g != VANILLA_SHA256:
        raise BuildError(f"game regulation changed (sha256 {g}); rerun with --rebaseline after review")
    if not VANILLA.is_file():
        VANILLA.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(GAME_REGULATION, VANILLA)  # read-only copy of the game file
    v = sha256_file(VANILLA)
    if v != VANILLA_SHA256:
        raise BuildError(f"work copy {VANILLA} has sha256 {v}; rerun with --rebaseline after review")
    return g


def ensure_pristine():
    """Pristine tree = vanilla .param binaries + the XMLs of the edited params. Recreated if missing;
    always compared byte-for-byte with a fresh unpack of the hash-checked vanilla copy."""
    if not all((PRISTINE / f"{s}.param.xml").is_file() for s in EDITED) or \
            len(list(PRISTINE.glob("*.param"))) != PARAM_COUNT:
        print("pristine tree missing or incomplete: re-creating it from the vanilla copy")
        unpack_regulation(PRISTINE_PARENT, VANILLA)
        witchy(["--unpack", *[PRISTINE / f"{s}.param" for s in EDITED]], "unpack_pristine_xml", len(EDITED))
    fresh = unpack_regulation(FRESH_PARENT, VANILLA)
    drift = [p.name for p in fresh.glob("*.param") if (PRISTINE / p.name).read_bytes() != p.read_bytes()]
    if drift:
        raise BuildError(f"pristine tree drifted from the vanilla copy: {drift[:5]}; delete {PRISTINE_PARENT}")
    if (fresh / "_witchy-bnd4.xml").read_bytes() != (PRISTINE / "_witchy-bnd4.xml").read_bytes():
        raise BuildError("pristine _witchy-bnd4.xml differs from a fresh unpack")
    return fresh


def build_tree(plan):
    if BUILD_PARENT.exists():
        shutil.rmtree(BUILD_PARENT)
    BUILD.mkdir(parents=True)
    for p in PRISTINE.glob("*.param"):
        shutil.copyfile(p, BUILD / p.name)
    shutil.copyfile(PRISTINE / "_witchy-bnd4.xml", BUILD / "_witchy-bnd4.xml")
    added = {}
    for stem in EDITED:
        xml = BUILD / f"{stem}.param.xml"
        shutil.copyfile(PRISTINE / f"{stem}.param.xml", xml)
        added[stem] = edit_xml(xml, plan["rows"][stem])
        (BUILD / f"{stem}.param").unlink()
    witchy(["--repack", *[BUILD / f"{s}.param.xml" for s in EDITED]], "build_repack_params", len(EDITED))
    for stem in EDITED:
        if not (BUILD / f"{stem}.param").is_file():
            raise BuildError(f"repack did not produce {stem}.param")
        (BUILD / f"{stem}.param.xml").unlink()  # keep the folder to exactly 194 params + manifest
    out = BUILD_PARENT / "regulation.bin"
    witchy(["--repack", BUILD], "build_repack_regulation", 1)
    if not out.is_file():
        raise BuildError(f"repack did not produce {out}")
    return out, added


def round_trip(built):
    unpack_regulation(CHECK_PARENT, built)
    witchy(["--unpack", *[CHECK / f"{s}.param" for s in EDITED]], "check_unpack_params_xml", len(EDITED))
    return CHECK


def publish(built):
    PACKAGE_DIR.mkdir(parents=True, exist_ok=True)
    others = [p.name for p in PACKAGE_DIR.iterdir() if p.name != "regulation.bin"]
    if others:
        raise BuildError(f"{PACKAGE_DIR} must contain only regulation.bin (found {others})")
    fd, tmp = tempfile.mkstemp(prefix="regulation.", suffix=".tmp", dir=PACKAGE_DIR)
    os.close(fd)
    shutil.copyfile(built, tmp)
    os.replace(tmp, PACKAGE)
    return sha256_file(PACKAGE)


def write_rows_json(plan, built_utc, package_sha, hit_radius):
    data = {
        "schema": 1, "generator": "build_regulation.py", "built_utc": built_utc,
        "vanilla_sha256": VANILLA_SHA256, "package_regulation_sha256": package_sha,
        "k": plan["k"], "id_base": plan["id_base"], "marker_id": plan["marker_id"],
        "bullet": {"speed_mps": plan["speed"], "life_s": plan["life"], "hit_radius_m": hit_radius,
                   "clone_from": plan["clone"]["Bullet"]},
        "atk": {"clone_from": plan["clone"]["AtkParam_Pc"]},
        "weapons": plan["weapons"],
        # M6: the arsenal rows (docs/M6_DESIGN.md 9.3; the DLL PARAMS check and the analyzer read them)
        "arsenal": plan["arsenal"],
        # M7: the nuke rows (docs/M7_DESIGN.md 3.13; the DLL PARAMS check reads them)
        "nuke": plan["nuke"],
        # C3 PASS C: the co-op twins and the synced marker rows (docs/C3_DESIGN.md 3.3; the DLL checks them live)
        "coop": plan["coop"],
        # M4: per gun the tier rows (id, radius_m, mw2 damage, atkPhys, sfx), the protection SpEffect and k; the DLL
        # PARAMS check and the analyzer read it (nothing is hard-coded twice).
        "ac130": {"k": plan["ac130"]["k"], "gun_scale": plan["ac130"]["gun_scale"],
                  "atk_mode": plan["ac130"]["atk_mode"], "protect_speffect_id": plan["ac130"]["protect_speffect_id"],
                  "protect_endurance_s": plan["ac130"]["protect_endurance_s"],
                  "clone": plan["ac130"]["clone"], "guns": plan["ac130"]["guns"]},
    }
    ROWS_JSON.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix="rows.", suffix=".tmp", dir=ROWS_JSON.parent)
    with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps(data, indent=2) + "\n")
    os.replace(tmp, ROWS_JSON)


def build():
    t0 = datetime.datetime.now(datetime.timezone.utc)
    stamp = t0.strftime("%Y%m%dT%H%M%SZ")
    preflight()
    game_sha = source_check()
    cfg, weapons = load_inputs()
    plan = make_plan(cfg, weapons)
    ensure_pristine()
    built, added = build_tree(plan)
    check = round_trip(built)
    problems, report = verify(PRISTINE, check, plan)
    problems += verify_xml_readback(check, plan)
    problems += arsenal_known_answers(plan)
    full = {"built_utc": stamp, "game_regulation_sha256": game_sha, "built_sha256": sha256_file(built),
            "added_xml_rows": added, "verify": report, "problems": problems,
            "plan": {k: v for k, v in plan.items() if k != "rows"},
            "rows": {s: [[r[0], r[1], r[2], r[3]] for r in plan["rows"][s]] for s in EDITED}}
    LOGS.mkdir(parents=True, exist_ok=True)
    (LOGS / f"build_{stamp}.json").write_text(json.dumps(full, indent=1), encoding="utf-8")
    if problems:
        for p in problems:
            print("FAIL " + p)
        raise BuildError(f"round trip found {len(problems)} problem(s); package NOT updated "
                         f"(report {LOGS / ('build_' + stamp + '.json')})")
    package_sha = publish(built)
    a = param_diff.read_param(PRISTINE / "Bullet.param")
    src, _ = decode_rows(a, {plan["clone"]["Bullet"]}, param_rows.DEFAULT_DEFS)
    hit_radius = src[plan["clone"]["Bullet"]][1].get("hitRadius")
    write_rows_json(plan, stamp, package_sha, hit_radius)
    nb, na, ns = len(plan["rows"]["Bullet"]), len(plan["rows"]["AtkParam_Pc"]), len(plan["rows"]["SpEffectParam"])
    print(f"round trip: {report['untouched_identical']} untouched params byte-identical; Bullet "
          f"{report['edited']['Bullet']['rows_vanilla']}->{report['edited']['Bullet']['rows_built']}, AtkParam_Pc "
          f"{report['edited']['AtkParam_Pc']['rows_vanilla']}->{report['edited']['AtkParam_Pc']['rows_built']}, SpEffectParam "
          f"{report['edited']['SpEffectParam']['rows_vanilla']}->{report['edited']['SpEffectParam']['rows_built']}; "
          f"{report['rows_checked']} added rows field-checked against the clone source; WitchyBND read-back OK")
    for wname, w in plan["weapons"].items():
        print(f"  {wname} slot {w['slot']}: " + ", ".join(f"{b['id']}=mw2 {b['mw2_damage']}->atkPhys {b['atk_phys']}"
                                                          for b in w["bands"]))
    for gid, g in plan["ac130"]["guns"].items():
        print(f"  ac130 {gid}mm (k x scale {plan['ac130']['k'] * plan['ac130']['gun_scale'][gid]:g}, flat): "
              + ", ".join(f"{t}={r['id']} r={r['radius_m']}m mw2 {r['mw2']:g}->atkPhys {r['atk_phys']} sfx {r['sfx']}"
                          for t, r in g.items()))
        # info only: the community ER defence curve on a Limgrave Godrick Soldier (c4311, 219 HP, def ~110, cut 1.0)
        pred = {t: er_damage(r["atk_phys"], C4311_DEF) for t, r in g.items()}
        if "core" in pred:
            print(f"    predicted c4311 [I]: inside the core {pred['core'] + pred['ring']:.0f} (core+ring), ring only "
                  f"{pred['ring']:.0f}, single {pred['single']:.0f} (219 HP)")
        else:
            print(f"    predicted c4311 [I]: {pred['single']:.0f} per hit (219 HP)")
    n_assets, broken = asset_break_report(PRISTINE)
    print(f"  objects [I]: of {n_assets} vanilla damage-breakable assets (hp > 0) the atkObj of each tier beats: "
          + ", ".join(f"{k} {v}" for k, v in broken.items()) + " (the rest are defence 299/990/999, never broken by a "
          "vanilla attack)")
    ar = plan["arsenal"]
    n_hit = sum(len(g["bands"]) for g in ar["guns"].values())
    n_l = sum(1 for g in ar["guns"].values() if "launcher" in g)
    u = ar["guns"].get("ump45_mp")
    print(f"  arsenal (flat_inv s={ar['s']} d_ref={ar['d_ref']}, bullet_sfx={ar['bullet_sfx']}): {len(ar['guns'])} guns, "
          f"{n_hit} hitscan band rows, {n_l} launchers x (impact, marker, explosion tiers)" +
          (f"; ump45_mp {u['bands'][0]['id']}..{u['bands'][-1]['id']} atk {u['bands'][0]['atk']}..{u['bands'][-1]['atk']} "
           f"(predicted c4311 close {er_damage(u['bands'][0]['atk'], ar['d_ref']):.1f}, far {er_damage(u['bands'][-1]['atk'], ar['d_ref']):.1f})" if u else ""))
    print(f"  protection SpEffectParam {plan['ac130']['protect_speffect_id']} (clone {plan['clone']['SpEffectParam']}, "
          f"effectEndurance {plan['ac130']['protect_endurance_s']:g} s); "
          "safety fields asserted on every M4 row")
    n = plan["nuke"]
    print(f"  nuke: SpEffectParam {n['lethal_speffect_id']} (clone {n['clone']['speffect']}: one-shot, changeHpRate 100, noDead 0, "
          f"no HP burn, no vfx), lethal Bullet + AtkParam_Pc {n['lethal_bullet_id']} (atkPhys 9999 flat, spEffectId0 "
          f"{n['lethal_speffect_id']}), horizon Bullet {n['horizon_bullet_id']} (atkId -1, sfx {n['horizon_sfx']}); SAFETY_M7 asserted")
    co = plan["coop"]
    print(f"  coop (C3 PASS C): {len(co['twins'])} visual twins (atkId -1, synced at spawn), nuke marker Bullet "
          f"{co['nuke_marker_bullet']}, marker SpEffects {co['sp_mode']} (vfx {co['sp_mode_vfx']}), {co['sp_gun_base']}.."
          f"{co['sp_gun_base'] + co['sp_gun_count'] - 1}, {co['sp_ac130']}, {co['sp_nuke']} (clone {co['sp_clone']}, "
          f"isDisableNetSync 0, endurance {co['marker_endurance_s']:g} s); SAFETY_C3 asserted, 79000960 stays unsynced")
    print(f"wrote {PACKAGE} sha256 {package_sha}")
    print(f"wrote {ROWS_JSON}")
    print(f"REGULATION PASS bullet+{nb} atk+{na} speffect+{ns} k={plan['k']} ac130_k={plan['ac130']['k']} package={package_sha[:12]}")
    return 0


# ---- self-test: the checks must be able to fail ---------------------------------------------------------

def mutate_row_table(src, dst, fn):
    """Copies a .param and lets `fn(buf, rows)` edit the row table (rows = [(entry_offset, id)])."""
    buf = bytearray(pathlib.Path(src).read_bytes())
    nrows = int.from_bytes(buf[0x0A:0x0C], "little")
    rows = [(0x40 + i * 0x18, int.from_bytes(buf[0x40 + i * 0x18:0x44 + i * 0x18], "little", signed=True))
            for i in range(nrows)]
    fn(buf, rows)
    pathlib.Path(dst).write_bytes(bytes(buf))


def mutate_field(src, dst, rid, field, value):
    """Copies a .param and sets one field of row `rid` in its binary data (bitfields included)."""
    p = param_diff.read_param(src)
    _, layout, _size = param_rows.load_def(param_rows.DEFAULT_DEFS, p["type"], REGULATION_VERSION)
    name, typ, off, bit_off, bits, _count = next(x for x in layout if x[0] == field)
    buf = bytearray(pathlib.Path(src).read_bytes())
    nrows = int.from_bytes(buf[0x0A:0x0C], "little")
    doff = None
    for i in range(nrows):
        e = 0x40 + i * 0x18
        if int.from_bytes(buf[e:e + 4], "little", signed=True) == rid:
            doff = int.from_bytes(buf[e + 8:e + 16], "little")
            break
    if doff is None:
        raise BuildError(f"self-test setup: row {rid} not in {pathlib.Path(src).name}")
    o = doff + off
    if bit_off is not None:
        n = param_rows.SIZES[typ]
        cur = int.from_bytes(buf[o:o + n], "little")
        mask = ((1 << bits) - 1) << bit_off
        cur = (cur & ~mask) | ((int(value) << bit_off) & mask)
        buf[o:o + n] = cur.to_bytes(n, "little")
    else:
        struct.pack_into("<" + param_rows.FMT[typ], buf, o, value)
    pathlib.Path(dst).write_bytes(bytes(buf))


def self_test():
    need = ("Bullet.param", "Bullet.param.xml", "AtkParam_Pc.param.xml", "SpEffectParam.param.xml")
    prev = json.loads(ROWS_JSON.read_text(encoding="utf-8")) if ROWS_JSON.is_file() else {}
    has_ac130 = "ac130" in prev and "arsenal" in prev and "coop" in prev
    if not all((CHECK / n).is_file() for n in need) or not has_ac130:
        print("no previous (C3 PASS C) build: building first")
        build()
    cfg, weapons = load_inputs()
    plan = make_plan(cfg, weapons)
    marker = plan["marker_id"]
    first, second = plan["rows"]["Bullet"][0][0], plan["rows"]["Bullet"][1][0]
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="mw2er_selftest_", dir=WORK))
    try:
        control, _ = verify(PRISTINE, CHECK, plan, skip_untouched=True)
        control += verify_xml_readback(CHECK, plan)
        if control:
            print("FAIL control (unmutated) check does not pass: " + "; ".join(control[:3]))
            return 1
        print("control: the unmutated build passes (round trip + XML read-back)")

        def flip_vanilla_byte(buf, rows):
            off = int.from_bytes(buf[rows[0][0] + 8:rows[0][0] + 16], "little")  # data offset of row 0
            buf[off] ^= 0xFF

        def drop_marker(buf, rows):
            entry = next(o for o, rid in rows if rid == marker)
            buf[entry:entry + 4] = (marker + 1).to_bytes(4, "little", signed=True)

        def swap_ours(buf, rows):
            # Swap only the two ids: the data offsets stay ascending, so the file still parses and
            # the failure must come from the order check, not from a parse error.
            ea = next(o for o, rid in rows if rid == first)
            eb = next(o for o, rid in rows if rid == second)
            buf[ea:ea + 4], buf[eb:eb + 4] = buf[eb:eb + 4], buf[ea:ea + 4]

        def bullet_case(fn, atk_off=0):
            def run():
                overrides = {}
                if fn is not None:
                    dst = tmp / "Bullet.param"
                    mutate_row_table(CHECK / "Bullet.param", dst, fn)
                    overrides["Bullet"] = dst
                return verify(PRISTINE, CHECK, plan, overrides_paths=overrides, skip_untouched=True,
                              atk_off_by=atk_off)[0]
            return run

        def untouched_flipped():
            # An untouched param (not one we edit) with one byte changed, checked WITHOUT skip_untouched:
            # the "untouched params byte-identical" branch must catch it.
            stem = "EquipParamWeapon"
            if stem in EDITED or not (CHECK / f"{stem}.param").is_file():
                return [f"self-test setup: {stem}.param unavailable"]
            dst = tmp / f"{stem}.param"
            buf = bytearray((CHECK / f"{stem}.param").read_bytes())
            buf[len(buf) // 2] ^= 0xFF
            dst.write_bytes(bytes(buf))
            return verify(PRISTINE, CHECK, plan, overrides_paths={stem: dst})[0]

        def manifest_missing():
            return verify(PRISTINE, CHECK, plan, overrides_paths={MANIFEST: tmp / "absent" / MANIFEST},
                          skip_untouched=True)[0]

        def readback_attr_missing():
            # Drop HitBulletID (override 79000900, paramdef default -1) from the second band row's read-back XML.
            rid = plan["rows"]["Bullet"][1][0]
            src = (CHECK / "Bullet.param.xml").read_text(encoding="utf-8-sig")
            out, hits = [], 0
            for line in src.splitlines(keepends=True):
                m = ROW_RE.match(line)
                if m and int(m.group(1)) == rid:
                    line, n = re.subn(r' HitBulletID="[^"]*"', "", line)
                    hits += n
                out.append(line)
            if hits != 1:
                return [f"self-test setup: HitBulletID not found once in row {rid}"]
            dst = tmp / "Bullet.param.xml"
            dst.write_text("".join(out), encoding="utf-8")
            return verify_xml_readback(CHECK, plan, xml_paths={"Bullet": dst})

        def field_case(stem, rid, field, value):
            # M4: one field of one of our rows changed in the built binary (the clone/override + safety checks)
            def run():
                dst = tmp / f"{stem}.param"
                mutate_field(CHECK / f"{stem}.param", dst, rid, field, value)
                return verify(PRISTINE, CHECK, plan, overrides_paths={stem: dst}, skip_untouched=True)[0]
            return run

        protect = plan["ac130"]["protect_speffect_id"]

        def speffect_unsorted():
            # the protection row's id moved below its vanilla neighbour: no longer a sorted insert
            def fn(buf, rows):
                entry = next(o for o, rid in rows if rid == protect)
                buf[entry:entry + 4] = (1).to_bytes(4, "little", signed=True)
            dst = tmp / "SpEffectParam.param"
            mutate_row_table(CHECK / "SpEffectParam.param", dst, fn)
            return verify(PRISTINE, CHECK, plan, overrides_paths={"SpEffectParam": dst}, skip_untouched=True)[0]

        cases = [
            ("flipped byte in a vanilla Bullet row", bullet_case(flip_vanilla_byte)),
            ("expected atkPhys off by 1", bullet_case(None, 1)),
            ("missing marker row", bullet_case(drop_marker)),
            ("row out of order", bullet_case(swap_ours)),
            ("flipped byte in an untouched param (EquipParamWeapon)", untouched_flipped),
            ("BND manifest missing from the read-back", manifest_missing),
            ("override attribute missing from the XML read-back", readback_attr_missing),
            ("M4: selfTarget = 1 on AtkParam_Pc 79000201", field_case("AtkParam_Pc", 79000201, "selfTarget", 1)),
            ("M4: isHitBothTeam = 1 on Bullet 79000302", field_case("Bullet", 79000302, "isHitBothTeam", 1)),
            (f"M4: an extra override (iconId) on SpEffectParam {protect}", field_case("SpEffectParam", protect, "iconId", 123)),
            ("M4: unsorted SpEffect insert", speffect_unsorted),
            (f"M4: permanent protection (effectEndurance -1) on SpEffectParam {protect}",
             field_case("SpEffectParam", protect, "effectEndurance", -1.0)),
            ("run 8: weapon-AR damage again (isAddBaseAtk 0) on AtkParam_Pc 79000201",
             field_case("AtkParam_Pc", 79000201, "isAddBaseAtk", 0)),
            ("run 8: weapon AR x 100% added (atkPhysCorrection 100) on AtkParam_Pc 79000302",
             field_case("AtkParam_Pc", 79000302, "atkPhysCorrection", 100)),
            ("objects: the 105 core back to atkObj 100 (must beat asset defence 199) on AtkParam_Pc 79000201",
             field_case("AtkParam_Pc", 79000201, "atkObj", 100)),
            ("objects: no bush-asset damage (isEnableCalcDamageForBushesObj 0) on AtkParam_Pc 79000203",
             field_case("AtkParam_Pc", 79000203, "isEnableCalcDamageForBushesObj", 0)),
            ("sfx: the 105 ring's dust layer dropped (sfxId_Bullet -1) on Bullet 79000202",
             field_case("Bullet", 79000202, "sfxId_Bullet", -1)),
            ("sfx: parent-oriented effect (sfxPostureType 0) on Bullet 79000401",
             field_case("Bullet", 79000401, "sfxPostureType", 0)),
        ]
        # M6 (docs/M6_DESIGN.md 9.6): the arsenal rows
        ar = plan["arsenal"]["guns"]
        gun = next(iter(g for g in ar.values() if g["bands"]), None)
        lau = next(iter(g for g in ar.values() if "launcher" in g), None)
        if gun:
            b0 = gun["bands"][0]["id"]
            cases += [
                (f"M6: weapon-AR damage (isAddBaseAtk 0) on AtkParam_Pc {b0}", field_case("AtkParam_Pc", b0, "isAddBaseAtk", 0)),
                (f"M6: atkPhysCorrection 100 on AtkParam_Pc {b0}", field_case("AtkParam_Pc", b0, "atkPhysCorrection", 100)),
                (f"M6: pierce attribute (atkAttribute 2) on AtkParam_Pc {b0}", field_case("AtkParam_Pc", b0, "atkAttribute", 2)),
                (f"M6: band atkPhys off by 1 vs the inversion on AtkParam_Pc {b0}",
                 field_case("AtkParam_Pc", b0, "atkPhys", gun["bands"][0]["atk"] + 1)),
                (f"M6: shield-ignoring gun (disableGuard 1) on AtkParam_Pc {b0}", field_case("AtkParam_Pc", b0, "disableGuard", 1)),
                (f"M6: dagger hit effect (sfxId_Hit 300002) with bullet_sfx none on Bullet {b0}",
                 field_case("Bullet", b0, "sfxId_Hit", 300002)),
            ]
        if lau:
            lt = lau["launcher"]
            cases += [
                (f"M6: launcher marker with an attack (atkId_Bullet 0) on Bullet {lt['marker_id']}",
                 field_case("Bullet", lt["marker_id"], "atkId_Bullet", 0)),
                (f"M6: explosion hurts the player (selfTarget 1) on AtkParam_Pc {lt['single']['id']}",
                 field_case("AtkParam_Pc", lt["single"]["id"], "selfTarget", 1)),
            ]

        # M7 (docs/M7_DESIGN.md 6.2): the lethal SpEffect and the nuke bullets
        nk = plan["nuke"]
        ls, lb, hb = nk["lethal_speffect_id"], nk["lethal_bullet_id"], nk["horizon_bullet_id"]
        cases += [
            (f"M7: 99 % instead of 100 % (changeHpRate 99) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "changeHpRate", 99.0)),
            (f"M7: a permanent lethal effect (effectEndurance -1) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "effectEndurance", -1.0)),
            (f"M7: noDead 1 on SpEffectParam {ls}", field_case("SpEffectParam", ls, "noDead", 1)),
            (f"M7: an HP-burn effect (isHpBurnEffect 1) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "isHpBurnEffect", 1)),
            (f"M7: kept on the dead (dontDeleteOnDead 1) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "dontDeleteOnDead", 1)),
            (f"M7: a visual effect (vfxId 1) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "vfxId", 1)),
            (f"M7: the lethal bullet hurts the player (selfTarget 1) on AtkParam_Pc {lb}", field_case("AtkParam_Pc", lb, "selfTarget", 1)),
            (f"M7: the lethal bullet without its SpEffect (spEffectId0 -1) on AtkParam_Pc {lb}", field_case("AtkParam_Pc", lb, "spEffectId0", -1)),
            (f"M7: the horizon explosion attacks (atkId_Bullet {lb}) on Bullet {hb}", field_case("Bullet", hb, "atkId_Bullet", lb)),
        ]

        def duplicate_g():
            w2 = json.loads(json.dumps(weapons))
            names = list(w2["arsenal"])
            if len(names) < 2:
                return ["self-test setup: fewer than 2 arsenal guns"]
            w2["arsenal"][names[1]]["g"] = w2["arsenal"][names[0]]["g"]
            try:
                make_plan(cfg, w2)
            except BuildError as e:
                return [str(e)]
            return []
        cases.append(("M6: duplicate gun index g", duplicate_g))

        def body_mult_ignored():
            # review M6 fidelity: the Intervention built without its locTorsoUpper (70 for every sniper, the old bug)
            w2 = json.loads(json.dumps(weapons))
            if "cheytac_mp" not in w2.get("arsenal", {}):
                return ["self-test setup: cheytac_mp not in the arsenal"]
            w2["arsenal"]["cheytac_mp"]["num"]["locTorsoUpper"] = 1.0
            return arsenal_known_answers(make_plan(cfg, w2))
        cases.append(("M6: sniper body multiplier ignored (Intervention 70 instead of 105)", body_mult_ignored))

        # C3 PASS C (docs/C3_DESIGN.md 3.3): the twins never attack, the markers sync and expire, the lethal never syncs
        co = plan["coop"]
        tw_t = next((t for t in co["twins"] if t["id"] >= C3_TWIN_BASE + 1000), co["twins"][0])
        tw = tw_t["id"]
        tw_ac = next((t["id"] for t in co["twins"] if t["id"] < C3_TWIN_BASE + 1000), tw)
        cases += [
            (f"C3: a gun twin that attacks (atkId_Bullet {tw_t['src']}) on Bullet {tw}",
             field_case("Bullet", tw, "atkId_Bullet", tw_t["src"])),
            (f"C3: a twin that spawns the impact marker (HitBulletID {marker}) on Bullet {tw}", field_case("Bullet", tw, "HitBulletID", marker)),
            (f"C3: an AC-130 twin that attacks on Bullet {tw_ac}", field_case("Bullet", tw_ac, "atkId_Bullet", tw_ac - C3_TWIN_BASE + 79_000_000)),
            (f"C3: the nuke marker bullet attacks on Bullet {co['nuke_marker_bullet']}",
             field_case("Bullet", co["nuke_marker_bullet"], "atkId_Bullet", lb)),
            (f"C3: an unsynced marker (isDisableNetSync 1) on SpEffectParam {co['sp_mode']}", field_case("SpEffectParam", co["sp_mode"], "isDisableNetSync", 1)),
            (f"C3: a permanent marker (effectEndurance -1) on SpEffectParam {co['sp_gun_base']}",
             field_case("SpEffectParam", co["sp_gun_base"], "effectEndurance", -1.0)),
            (f"C3: the MW2-mode marker without the invisible-weapon vfx (vfxId -1) on SpEffectParam {co['sp_mode']}",
             field_case("SpEffectParam", co["sp_mode"], "vfxId", -1)),
            (f"C3: a marker that heals (changeHpRate -10) on SpEffectParam {co['sp_nuke']}", field_case("SpEffectParam", co["sp_nuke"], "changeHpRate", -10.0)),
            (f"C3: the lethal nuke SpEffect synced (isDisableNetSync 0) on SpEffectParam {ls}", field_case("SpEffectParam", ls, "isDisableNetSync", 0)),
        ]

        def twin_rule_broken():
            p2 = json.loads(json.dumps({"coop": plan["coop"]}))
            p2["coop"]["twins"][0]["src"] += 1000
            probs = []
            for t in p2["coop"]["twins"][:1]:
                if c3_twin_of(t["src"]) != t["id"]:
                    probs.append(f"twin {t['id']} of {t['src']} breaks the id rule")
            return probs
        cases.append(("C3: a twin listed for the wrong source row", twin_rule_broken))
        caught = 0
        for label, run in cases:
            problems = run()
            if problems and not problems[0].startswith("self-test setup"):
                caught += 1
                print(f"caught: {label} -> {problems[0][:140]}")
            else:
                print(f"MISSED: {label} passed the check" + (f" ({problems[0]})" if problems else ""))
        if caught == len(cases):
            print(f"REGULATION SELF-TEST PASS {caught}/{len(cases)} mutations caught (control passed)")
            return 0
        print(f"REGULATION SELF-TEST FAIL {caught}/{len(cases)} mutations caught")
        return 1
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def rebaseline():
    if not GAME_REGULATION.is_file():
        raise BuildError(f"game regulation not found: {GAME_REGULATION}")
    VANILLA.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(GAME_REGULATION, VANILLA)  # the game file is only read
    h = sha256_file(VANILLA)
    unpack_regulation(PRISTINE_PARENT, VANILLA)
    witchy(["--unpack", *[PRISTINE / f"{s}.param" for s in EDITED]], "unpack_pristine_xml", len(EDITED))
    print(f"rebaselined: {VANILLA} sha256 {h}")
    if h != VANILLA_SHA256:
        print("The game's regulation changed. Review the patch (Paramdex defs, clone rows, free id range), then "
              "update VANILLA_SHA256 in this script.")
        return 1
    return 0


def main(argv):
    if argv and argv not in (["--self-test"], ["--rebaseline"]):
        print(__doc__)
        return 2
    try:
        with BuildLock():
            if argv == ["--self-test"]:
                return self_test()
            if argv == ["--rebaseline"]:
                return rebaseline()
            return build()
    except BuildError as e:
        print(f"REGULATION FAIL {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
