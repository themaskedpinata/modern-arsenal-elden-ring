"""Convert MW2's movement numbers into converted/movement.json for the mw2er DLL (docs/M5_DESIGN.md section 12).

Usage:
  python -I convert_movement.py             convert (writes converted/movement.json atomically)
  python -I convert_movement.py --check     re-derive every known answer and compare with converted/movement.json
  python -I convert_movement.py --self-test the 5 mutations of section 12 must FAIL --check (temporary copies under
                                            work/m5/selftest/; the MW2 install is never written)

Inputs (all read only):
  dumps/common_mp/weapons/ump45_mp                                    WEAPONFILE fields (movement, sprint, spread, pose)
  dumps/m5/localized_code_pre_gfx_mp/default_mp_controls.cfg          the MW2 PC binds (checked, recorded)
  dumps/m5/code_pre_gfx_mp/default_mp_gamesettings.cfg                scr_player_sprinttime (cross-check of 4 s)
  dumps/m5/common_mp/maps/mp/gametypes/_weapons.gsc                   updateMoveSpeedScale class scales
  dumps/m5/patch_mp/maps/mp/perks/_perkfunctions.gsc                  the Lightweight scaler
  <MW2>/iw4sp.exe                                                     static PE read only: the instruction sites and
                                                                      .rdata floats below, the eye-height tables

The exe is opened with open(path, 'rb') and parsed as bytes. It is never run, loaded or attached to. Each exe constant
is checked at its instruction site: the bytes there must reference the recorded .rdata address (RIP-relative disp32)
or carry the recorded immediate, and the value is read from the file. The static disassembler tools/iw4dis prints the
same sites as text (check_m5.ps1 check 6). No line of the .cfg/.gsc inputs is copied into this script: the values are
found with patterns and compared with the numbers below.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# Co-op friend package: setup.ps1 points this at the friend's own MW2 with MW2ER_IW4SP (read only; the sha pin below applies).
EXE = Path(os.environ.get("MW2ER_IW4SP") or r"D:\SteamLibrary\steamapps\common\Call of Duty Modern Warfare 2\iw4sp.exe")
# sha256 of the user's iw4sp.exe when the sites below were read (2026-10-17)
PINNED_EXE_SHA = "2f8c9cfda1b8c0fb308fd9719577154e8222fc7654f5e8f1bf54579bca69d805"
WEAPON = ROOT / "dumps" / "common_mp" / "weapons" / "ump45_mp"
CONTROLS = ROOT / "dumps" / "m5" / "localized_code_pre_gfx_mp" / "default_mp_controls.cfg"
GAMESETTINGS = ROOT / "dumps" / "m5" / "code_pre_gfx_mp" / "default_mp_gamesettings.cfg"
WEAPONS_GSC = ROOT / "dumps" / "m5" / "common_mp" / "maps" / "mp" / "gametypes" / "_weapons.gsc"
PERKS_GSC = ROOT / "dumps" / "m5" / "patch_mp" / "maps" / "mp" / "perks" / "_perkfunctions.gsc"
OUT = ROOT / "converted" / "movement.json"
SELFTEST = ROOT / "work" / "m5" / "selftest"
IMAGE_BASE = 0x140000000

# ---- exe sites (VA, kind, data) -------------------------------------------------------------------------------
# kind 'rip': an instruction whose RIP-relative disp32 addresses `target` (the f32 there is the value);
# kind 'imm': the bytes `prefix` then a little-endian int32 immediate;
# kind 'immf': the bytes `prefix` then a little-endian f32 immediate (mov dword ptr [..], imm32).
SITES = {
    "g_speed": (0x140172B9D, "imm", ("ba",)),  # mov edx,0BEh (int dvar g_speed)
    "sprint_speed_scale": (0x140075001, "rip", (0x140385A78,)),  # movss xmm10 (player_sprintSpeedScale default)
    "back_speed_scale": (0x14007550F, "rip", (0x140385A2C,)),  # movss xmm1 before player_backSpeedScale
    "strafe_speed_scale": (0x1400754D9, "rip", (0x140385A34,)),  # movss xmm1 before player_strafeSpeedScale
    "sprint_strafe_speed_scale": (0x1400756EF, "rip", (0x1403242E4,)),  # movss xmm1 after player_sprintStrafeSpeedScale
    "crouch_speed_scale": (0x1400786AA, "rip", (0x14031F8C4,)),  # PM_CmdScaleForStance 0x1400785D0
    "prone_speed_scale": (0x1400786A1, "rip", (0x14031F740,)),
    "gravity": (0x140075FBE, "rip", (0x140324340,)),  # movss xmm6 (g_gravity default)
    "jump_height": (0x1400706A6, "rip", (0x14031F8CC,)),  # Jump_ClampVelocity addss 39
    "jump_min_interval_ms": (0x14007033A, "imm", ("3d",)),  # cmp eax,1F4h
    "slowdown_low_scale": (0x1400702B7, "rip", (0x14031F8C4,)),
    "slowdown_low_ms": (0x1400702BF, "imm", ("c74108",)),  # mov dword ptr [rcx+8],708h
    "slowdown_high_scale": (0x1400702C8, "rip", (0x140385A24,)),
    "slowdown_high_ms": (0x1400702D0, "imm", ("c74108",)),  # mov dword ptr [rcx+8],4B0h
    "land_factor_full_ms": (0x1400703DC, "imm", ("81f9",)),  # cmp ecx,6A4h
    "land_factor_max": (0x1400703E4, "rip", (0x140385AA4,)),
    "land_factor_slope": (0x1400703F5, "rip", (0x140385A78,)),  # mulss 1.5
    "friction": (0x1400799E1, "rip", (0x1403242F4,)),  # PM_Friction mulss 5.5
    "stopspeed": (0x1400799A5, "rip", (0x140385B14,)),  # movss 100
    "land_friction_scale": (0x1400799B6, "rip", (0x14031F744,)),  # mulss 0.3 (PMF_TIME_LAND)
    "accel_stand": (0x14007CB6F, "rip", (0x1403242F8,)),  # PM_WalkMove
    "accel_crouch": (0x14007CB79, "rip", (0x14031F750,)),
    "accel_prone": (0x14007CB83, "rip", (0x14032464C,)),
    "time_land_accel_scale": (0x14007CB9C, "rip", (0x140385A18,)),
    "air_accel": (0x1400773ED, "rip", (0x140385A54,)),
    "step_height": (0x14007E92F, "rip", (0x14031F8C8,)),
    "step_height_prone": (0x14007E925, "rip", (0x140385AD4,)),
    "hull_half_crouch": (0x140077D00, "immf", ("c78718010000",)),  # mov dword ptr [rdi+118h],41C80000h (25)
    "hull_half_prone": (0x140077D72, "immf", ("c78718010000",)),  # 15
    "hull_half_stand": (0x140077D88, "immf", ("c78718010000",)),  # 35
    "stance_lea_200": (0x14007C5A3, "imm8", ("448d6d",)),  # lea r13d,[rbp+64h] after lea ebp,[r14+64h]
    "crouch_prone_ms": (0x14007C5AC, "imm", ("41bf",)),  # mov r15d,190h
    "sprint_time_s": (0x140075288, "rip", (0x140385ABC,)),  # movss xmm7 4.0 (player_sprintTime)
    "sprint_min_time_s": (0x140074F06, "rip", (0x140385A54,)),  # movss xmm12 1.0
    "fall_min": (0x140078CD9, "rip", (0x140324320,)),  # PM_CrashLand 0x140078B90
    "fall_max": (0x140078CF0, "rip", (0x140324334,)),
    "fall_range": (0x140078D07, "rip", (0x14031F760,)),
    "land_slow_scale": (0x140078E5A, "rip", (0x140324640,)),
    "aim_spread_move_threshold": (0x14007513E, "rip", (0x1403242FC,)),
    "run_threshold": (0x140075445, "rip", (0x140324318,)),
    "sprint_cancel_mask": (0x14007B50E, "imm", ("f7c2",)),  # test edx,0CF35h (PM_UpdateSprint 0x14007B400)
    "sprint_start_mask": (0x14007B5F4, "imm", ("41f7c0",)),  # test r8d,0CC35h
    "jump_slowdown_sp_default": (0x14007089F, "bytes", ("33d2",)),  # xor edx,edx before jump_slowdownEnable
    # review M5 (provenance): the two decoded sprint dvars now byte-checked at their registration
    # player_sprintForwardMinimum: RegisterInt(name rcx, default edx, min r8d 0, max r9d 0FFh) @0x1400755E0; edx =
    # rdi + 69h with rdi = 0 from xor edi,edi @0x140074C85 (straight-line code up to 0x1400755B9: no branch, no write
    # to rdi; the calls between preserve rdi, a Win64 non-volatile register)
    "sprint_forward_min": (0x1400755B9, "imm8", ("8d57",)),  # lea edx,[rdi+69h]
    "sprint_forward_min_rdi0": (0x140074C85, "bytes", ("33ff", "xor edi,edi")),
    "sprint_forward_min_name": (0x1400755CE, "str", (0x140322E98, "player_sprintForwardMinimum")),
    # player_sprintRechargePause: RegisterFloat(name rcx @0x1400756B3, default xmm1 = xorps xmm1,xmm1, min xmm2 0)
    "sprint_recharge_pause_s": (0x1400756CB, "bytes", ("0f57c9", "xorps xmm1,xmm1")),
    "sprint_recharge_pause_name": (0x1400756B3, "str", (0x140323050, "player_sprintRechargePause")),
    # PM_CrashLand's second fall-damage branch (review M5 minor 10), taken when bit 12 (0x1000) of [r14+48h] is set:
    # dmg% = (h - 128) / 172 x 10, 10 % at h >= 300 (lea esi,[rbx+0Ah] with ebx = 0 from xor ebx,ebx @0x140078C6F)
    "fall_flag_bit": (0x140078C91, "imm8", ("0fbae0",)),  # bt eax,0Ch (eax = [r14+48h] @0x140078C55)
    "fall_min_alt": (0x140078C97, "rip", (0x140385B20,)),  # movss 128
    "fall_max_alt": (0x140078CAE, "rip", (0x14032432C,)),  # comiss 300
    "fall_range_alt": (0x140078CC3, "rip", (0x14032465C,)),  # divss 172
    "fall_scale_alt": (0x140078CCB, "rip", (0x140385AD4,)),  # mulss 10
    "fall_cap_alt": (0x140078CB7, "imm8", ("8d73",)),  # lea esi,[rbx+0Ah]
    "fall_alt_rbx0": (0x140078C6F, "bytes", ("33db", "xor ebx,ebx")),
}
# Sites that only prove another site's reading (a zeroed register, a dvar name): checked, not stored as constants.
HELPER_SITES = ("sprint_forward_min_rdi0", "sprint_forward_min_name", "sprint_recharge_pause_name", "fall_alt_rbx0")
# Sites stored as info_<name> rows (not read by the DLL).
INFO_SITES = ("land_factor_slope", "fall_range", "sprint_cancel_mask", "sprint_start_mask", "jump_slowdown_sp_default",
              "fall_flag_bit", "fall_min_alt", "fall_max_alt", "fall_range_alt", "fall_scale_alt", "fall_cap_alt")
ALT_FALL_NOTE = ("PM_CrashLand second branch, taken when bit 12 (0x1000) of [r14+48h] is set (bt eax,0Ch @0x140078c91; "
                 "the bit's meaning is unknown): dmg%% = (h - 128) / 172 x 10, capped 10 %% at h >= 300 in; %s")
# The branch the DLL uses (fall_min/fall_max) assumes the bit is clear for a normal player [I].
MAIN_FALL_NOTE = " (branch with bit 12 of [r14+48h] clear, assumed for a normal player [I]; see info_fall_*_alt)"

# ---- M8 mantle (docs/M8_DESIGN.md section 1) ----------------------------------------------------------------------
# The 46 sites of the static read of iw4sp.exe (work/m8/mantle_sites.py): the dvar registration 0x140071A00,
# Mantle_FindLedge 0x140070C10, Mantle_CheckLedgeHeight 0x140070FE0, Mantle_CalcPath 0x140070940, the start gates,
# BG_IsWeaponInactive 0x1400716B0. Each must byte-check, or the converter fails. They go to the "mantle" block only
# (the M5 constants and every existing key stay byte-identical).
MANTLE_SITES = {
    "mantle_check_range": (0x140071A50, "rip", (0x140385AE4,)),
    "mantle_check_range_max": (0x140071A41, "rip", (0x140385B20,)),
    "mantle_check_radius": (0x140071A8A, "rip", (0x140385A0C,)),
    "mantle_check_radius_max": (0x140071A7B, "rip", (0x14031FD10,)),
    "mantle_check_angle": (0x140071AC4, "rip", (0x140385B08,)),
    "mantle_check_angle_max": (0x140071AB5, "rip", (0x14031F764,)),
    "mantle_view_yawcap": (0x140071AFE, "rip", (0x140385B08,)),
    "mantle_view_yawcap_max": (0x140071AEF, "rip", (0x14031F764,)),
    "mantle_max_vert_end_vel": (0x140071B49, "bytes", ("0f57c9", "xorps xmm1,xmm1")),
    "mantle_max_vert_end_vel_max": (0x140071B29, "rip", (0x140385BA0,)),
    "mantle_enable_default": (0x140071A11, "imm8", ("b2",)),
    "mantle_debug_default": (0x140071A2D, "bytes", ("33d2", "xor edx,edx")),
    "mantle_enable_name": (0x140071A13, "str", (0x14031F9F8, "mantle_enable")),
    "mantle_check_range_name": (0x140071A49, "str", (0x14031FA90, "mantle_check_range")),
    "mantle_check_radius_name": (0x140071A83, "str", (0x14031FAE0, "mantle_check_radius")),
    "mantle_check_angle_name": (0x140071ABD, "str", (0x14031FB48, "mantle_check_angle")),
    "mantle_view_yawcap_name": (0x140071AF7, "str", (0x14031FBA0, "mantle_view_yawcap")),
    "mantle_mvev_name": (0x140071B31, "str", (0x14031FBF8, "mantle_max_vertical_end_velocity")),
    "ledge_h_60": (0x140070E8F, "rip", (0x140385B08,)),
    "ledge_h_40": (0x140070EB3, "rip", (0x14031E698,)),
    "ledge_h_20": (0x140070ED7, "rip", (0x140385AE4,)),
    "ledge_forward": (0x140071028, "rip", (0x140385AE0,)),
    "ledge_floor": (0x140071107, "rip", (0x14031F8C8,)),
    "ledge_box_half_x": (0x140071091, "immf", ("c745ff",)),
    "ledge_box_half_z": (0x140071098, "immf", ("c74503",)),
    "ledge_clear_mid_z": (0x140071188, "immf", ("c745f7",)),
    "ledge_clear_half_z": (0x140071193, "immf", ("c74503",)),
    "front_half_z_a": (0x140070CA2, "immf", ("c745df",)),
    "front_half_z_b": (0x140070CC3, "immf", ("c745eb",)),
    "front_player_radius": (0x140070C85, "rip", (0x14031FD10,)),
    "front_contents": (0x140070D1C, "imm", ("c7442430",)),
    "front_surf_mask": (0x140070DB7, "imm", ("f745ff",)),
    "front_surf_over": (0x140070E61, "imm", ("f745ff",)),
    "front_normal_min": (0x140070DF6, "rip", (0x1403859C4,)),
    "over_forward": (0x1400709B8, "rip", (0x14031FD14,)),
    "over_drop": (0x140070AAC, "rip", (0x14031FD20,)),
    "over_clear_mid_z": (0x1400709EF, "immf", ("c745bf",)),
    "over_clear_half_z": (0x140070A08, "immf", ("c745cb",)),
    "gate_pm_type": (0x140070C59, "imm8", ("837904",)),
    "gate_mantling": (0x140070C63, "imm8", ("f6410c",)),
    "gate_stance": (0x140070C6D, "imm8", ("f681ac000000",)),
    "gate_jump": (0x140070970, "imm", ("f7410c",)),
    "gate_force": (0x140070979, "imm8", ("41f687e4010000",)),
    "ev_mantle": (0x140071D2C, "imm", ("ba",)),
    "weapon_inactive_over_low": (0x1400716D5, "imm8", ("833c88",)),
    "quick_raise_flag": (0x1400719E4, "imm8", ("838fe4010000",)),
}
MANTLE_KNOWN = {
    "mantle_check_range": 20, "mantle_check_range_max": 128, "mantle_check_radius": 0.1, "mantle_check_radius_max": 15,
    "mantle_check_angle": 60, "mantle_check_angle_max": 180, "mantle_view_yawcap": 60, "mantle_view_yawcap_max": 180,
    "mantle_max_vert_end_vel": 0, "mantle_max_vert_end_vel_max": 3.40282e+38, "mantle_enable_default": 1,
    "mantle_debug_default": 0, "ledge_h_60": 60, "ledge_h_40": 40, "ledge_h_20": 20, "ledge_forward": 16, "ledge_floor": 18,
    "ledge_box_half_x": 15, "ledge_box_half_z": 15, "ledge_clear_mid_z": 25, "ledge_clear_half_z": 25, "front_half_z_a": 35,
    "front_half_z_b": 35, "front_player_radius": 15, "front_contents": 0x1000000, "front_surf_mask": 0x6000000,
    "front_surf_over": 0x4000000, "front_normal_min": 0.0001, "over_forward": 31, "over_drop": -18, "over_clear_mid_z": 25,
    "over_clear_half_z": 25, "gate_pm_type": 6, "gate_mantling": 4, "gate_stance": 12, "gate_jump": 0x400, "gate_force": 32,
    "ev_mantle": 166, "weapon_inactive_over_low": 10, "quick_raise_flag": 16,
}
# The transition table {i32 up idx, i32 over idx, f32 height} x 7 and the anim name pointers (11 x u64), .data.
MANTLE_TABLE_VA = 0x1403BCBD0
MANTLE_NAMES_VA = 0x1403BCC30
MANTLE_TABLE_KNOWN = [(1, 8, 57.0), (2, 8, 51.0), (3, 9, 45.0), (4, 9, 39.0), (5, 9, 33.0), (6, 10, 27.0), (7, 10, 21.0)]
MANTLE_XANIM_DIR = ROOT / "dumps" / "m8" / "code_post_gfx" / "xanim"
MANTLE_MP_DIR = ROOT / "dumps" / "m8" / "common" / "xanim"
MANTLE_WEAPONS_DIR = ROOT / "dumps" / "common_mp" / "weapons"
MANTLE_SP_ANIMS = ["player_mantle_up_%d" % h for h in (57, 51, 45, 39, 33, 27, 21)] + \
    ["player_mantle_over_%s" % k for k in ("high", "mid", "low")]
# the 34 M6 guns: their quickDropTime / quickRaiseTime (weapon files, the gun quick-drops during a mantle)
MANTLE_GUNS = ("m4 famas scar tavor fal m16 masada fn2000 ak47 mp5k ump45 kriss p90 uzi sa80 rpd mg4 aug m240 cheytac "
               "barrett wa2000 m21 spas12 aa12 striker ranger m1014 model1887 at4 m79 rpg stinger javelin").split()
# known answers of the dumped SP curves (normalized, docs/M8_DESIGN.md 1.4) and the weapon times
MANTLE_CURVE_KNOWN = [("player_mantle_up_57", "z", 5, 0.526), ("player_mantle_up_57", "z", 9, 0.974),
                      ("player_mantle_up_57", "x", 13, 0.507), ("player_mantle_up_21", "z", 4, 0.496),
                      ("player_mantle_over_high", "x", 5, 0.626), ("player_mantle_over_high", "x", 10, 1.0)]
MANTLE_QUICK_KNOWN = {"ump45_mp": (0.25, 0.55), "rpd_mp": (0.4, 0.95), "javelin_mp": (0.35, 1.33)}
# M8 review: quickDropAnim / quickRaiseAnim and whether each equals dropAnim / raiseAnim (the weapon files)
MANTLE_QUICK_CLIPS_KNOWN = {"ump45_mp": ("viewmodel_ump45_putaway", "viewmodel_ump45_pullout", True, True),
                            "m4_mp": ("viewmodel_M4_quick_putaway", "viewmodel_M4_quick_pullout", False, False)}
# sha256 of converted/movement.json before M8 (every existing key must stay byte-identical: the file minus "mantle")
PRE_M8_SHA = "2c798c611388e0e672c38740148647051b659a62f71837ef5524d27c27f16837"

# The eye-height tables of PM_ViewHeightAdjust: {int percent, f32 inches, int pad} until percent -1.
CURVE_TABLES = {"stand_crouch": 0x1403BD470, "crouch_stand": 0x1403BD4E0, "crouch_prone": 0x1403BD550, "prone_crouch": 0x1403BD5E0}

# Known answers of table 2.1 / section 3 (the --check compares every derived value with these).
KNOWN = {
    "g_speed": 190, "sprint_speed_scale": 1.5, "back_speed_scale": 0.7, "strafe_speed_scale": 0.8,
    "sprint_strafe_speed_scale": 0.667, "crouch_speed_scale": 0.65, "prone_speed_scale": 0.15, "gravity": 800,
    "jump_height": 39, "jump_min_interval_ms": 500, "slowdown_low_scale": 0.65, "slowdown_low_ms": 1800,
    "slowdown_high_scale": 0.5, "slowdown_high_ms": 1200, "land_factor_full_ms": 1700, "land_factor_max": 2.5,
    "land_factor_slope": 1.5, "friction": 5.5, "land_friction_scale": 0.3, "stopspeed": 100, "accel_stand": 9, "accel_crouch": 12,
    "accel_prone": 19, "time_land_accel_scale": 0.25, "air_accel": 1, "step_height": 18, "step_height_prone": 10,
    "hull_half_crouch": 25, "hull_half_prone": 15, "hull_half_stand": 35, "stance_lea_200": 100, "crouch_prone_ms": 400,
    "sprint_time_s": 4, "sprint_min_time_s": 1, "fall_min": 200, "fall_max": 350, "fall_range": 150,
    "land_slow_scale": 0.67, "aim_spread_move_threshold": 11, "run_threshold": 110, "sprint_cancel_mask": 0xCF35,
    "sprint_start_mask": 0xCC35, "jump_slowdown_sp_default": 0,
    "sprint_forward_min": 105, "sprint_forward_min_rdi0": 0, "sprint_forward_min_name": 0, "sprint_recharge_pause_s": 0,
    "sprint_recharge_pause_name": 0, "fall_flag_bit": 12, "fall_min_alt": 128, "fall_max_alt": 300, "fall_range_alt": 172,
    "fall_scale_alt": 10, "fall_cap_alt": 10, "fall_alt_rbx0": 0,
}
CURVE_KNOWN = [("stand_crouch", 0.5, 51.2), ("crouch_prone", 0.5, 15.0), ("stand_crouch", 0.0, 60.0), ("stand_crouch", 1.0, 40.0),
               ("crouch_stand", 1.0, 60.0), ("crouch_prone", 1.0, 11.0), ("prone_crouch", 0.0, 11.0), ("prone_crouch", 1.0, 40.0)]
WEAPON_KNOWN = {"moveSpeedScale": 1, "adsMoveSpeedScale": 2, "sprintDurationScale": 1, "sprintInTime": 0.3,
                "sprintLoopTime": 0.7, "sprintOutTime": 0.3, "hipSpreadStandMin": 2, "hipSpreadDuckedMin": 1.75,
                "hipSpreadProneMin": 1.5, "hipSpreadMax": 5, "hipSpreadDuckedMax": 4.5, "hipSpreadProneMax": 4}
# MW2 PC binds the controller defaults use (the cfg is parsed generically and these commands looked up)
BINDS_WANT = {"+forward": "w", "+back": "s", "+moveleft": "a", "+moveright": "d", "+breath_sprint": "SHIFT",
              "+gostand": "SPACE", "togglecrouch": "c", "toggleprone": "CTRL"}
# Labels (review M5): V = byte-checked here (an exe site above, a curve table, a dump file); D = decoded by the
# research disassembly (work/m5/bgdvars_range.txt) but NOT byte-checked here, so no oracle can catch a wrong decode;
# I = inferred. The values below are D or I.
DECODED = {
    "sprint_time_max_s": (16.383, "s", "PM_UpdateSprint 0x14007B400 ms field clamp 0x3FFF (decode)", "D"),
    "ads_walk_scale": (0.4, "x", "PM_CmdScale_Walk 0x1400786C0 (pm_flag 0x40 = ADS inferred; not byte-checked)", "I"),
    "move_threshold": (10.0, "in/s", "player_moveThreshhold registration @0x14007546e (decode)", "D"),
    "sprint_camera_bob": (0.5, "x", "player_sprintCameraBob registration @0x140075722 (decode)", "D"),
    "jump_spread_add": (64.0, "/255", "jump_spreadAdd dvar decode", "D"),
    "time_land_base_ms": (500.0, "ms", "PM_CrashLand 0x140078B90 (500 + 35 x dmg, decode)", "D"),
    "time_land_per_pct_ms": (35.0, "ms", "PM_CrashLand 0x140078B90 (decode)", "D"),
    "time_land_max_ms": (2000.0, "ms", "PM_CrashLand 0x140078B90 (decode)", "D"),
    "land_vel_scale_short": (0.5, "x", "PM_CrashLand @0x140078E5A region (decode)", "D"),
    "land_vel_scale_long": (0.2, "x", "PM_CrashLand @0x140078E5A region (decode)", "D"),
    "land_slow_min": (12.0, "in", "PM_CrashLand (decode)", "D"),
    "view_dip_max": (24.0, "in", "PM_CrashLand view dip min(24, 4 + 4 (h - 12)/26) (decode)", "D"),
    "slowdown_low_band": (18.0, "in", "Jump_ApplySlowdown 0x1400702A0 region (origin + 18, decode)", "D"),
    "walkable_normal_y": (0.7, "cos", "Q3/CoD4 heritage; no 0.7 compare found in iw4sp", "I"),
    "cl_stance_hold_ms": (300.0, "ms", "cl_stanceHoldTime dvar decode", "D"),
    "bob_stand_x": (0.055, "x", "bg_weaponBobAmplitudeStanding (decode)", "D"),
    "bob_stand_y": (0.025, "x", "bg_weaponBobAmplitudeStanding (decode)", "D"),
    "bob_ducked_x": (0.045, "x", "bg_weaponBobAmplitudeDucked (decode)", "D"),
    "bob_ducked_y": (0.025, "x", "bg_weaponBobAmplitudeDucked (decode)", "D"),
    "bob_prone_x": (0.02, "x", "bg_weaponBobAmplitudeProne (decode)", "D"),
    "bob_prone_y": (0.005, "x", "bg_weaponBobAmplitudeProne (decode)", "D"),
    "bob_sprint_x": (0.02, "x", "bg_weaponBobAmplitudeSprinting (decode)", "D"),
    "bob_sprint_y": (0.014, "x", "bg_weaponBobAmplitudeSprinting (decode)", "D"),
    "bob_max": (8.0, "in", "bg_weaponBobMax (decode)", "D"),
    "bob_lag": (0.25, "x", "bg_weaponBobLag (decode)", "D"),
    "view_bob_stand_x": (0.007, "x", "bg_bobAmplitudeStanding (decode)", "D"),
    "view_bob_stand_y": (0.007, "x", "bg_bobAmplitudeStanding (decode)", "D"),
    "view_bob_ducked_x": (0.0075, "x", "bg_bobAmplitudeDucked (decode)", "D"),
    "view_bob_ducked_y": (0.0075, "x", "bg_bobAmplitudeDucked (decode)", "D"),
    "view_bob_prone_x": (0.02, "x", "bg_bobAmplitudeProne (decode)", "D"),
    "view_bob_prone_y": (0.005, "x", "bg_bobAmplitudeProne (decode)", "D"),
    "view_bob_sprint_x": (0.02, "x", "bg_bobAmplitudeSprinting (decode)", "D"),
    "view_bob_sprint_y": (0.014, "x", "bg_bobAmplitudeSprinting (decode)", "D"),
}
UNITS = {
    "g_speed": "in/s", "gravity": "in/s2", "jump_height": "in", "jump_min_interval_ms": "ms", "slowdown_low_ms": "ms",
    "slowdown_high_ms": "ms", "land_factor_full_ms": "ms", "friction": "1/s", "stopspeed": "in/s", "accel_stand": "1/s",
    "accel_crouch": "1/s", "accel_prone": "1/s", "air_accel": "1/s", "step_height": "in", "step_height_prone": "in",
    "sprint_time_s": "s", "sprint_min_time_s": "s", "fall_min": "in", "fall_max": "in", "fall_range": "in",
    "aim_spread_move_threshold": "in/s", "run_threshold": "in/s", "stand_crouch_ms": "ms", "crouch_prone_ms": "ms",
    "eye_stand": "in", "eye_crouch": "in", "eye_prone": "in", "hull_stand": "in", "hull_crouch": "in", "hull_prone": "in",
}


class Pe:
    """Static PE view of a file's bytes (no loader)."""

    def __init__(self, data):
        self.d = data
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        if data[pe:pe + 4] != b"PE\0\0":
            raise ValueError("not a PE file")
        n = struct.unpack_from("<H", data, pe + 6)[0]
        opt = struct.unpack_from("<H", data, pe + 20)[0]
        self.base = struct.unpack_from("<Q", data, pe + 24 + 24)[0]
        self.secs = []
        o = pe + 24 + opt
        for _ in range(n):
            name = data[o:o + 8].rstrip(b"\0").decode("ascii", "replace")
            vsz, va, rsz, raw = struct.unpack_from("<IIII", data, o + 8)
            self.secs.append((name, va, vsz, raw, rsz))
            o += 40

    def off(self, v):
        r = v - self.base
        for _, va, vsz, raw, rsz in self.secs:
            if va <= r < va + max(vsz, rsz) and r - va < rsz:
                return r - va + raw
        raise ValueError("VA 0x%x not in the file" % v)

    def section_of(self, v):
        r = v - self.base
        for name, va, vsz, _, rsz in self.secs:
            if va <= r < va + max(vsz, rsz):
                return name
        return "?"

    def f32(self, v):
        return struct.unpack_from("<f", self.d, self.off(v))[0]

    def bytes_at(self, v, n):
        o = self.off(v)
        return self.d[o:o + n]


def r6(x):
    return float("%.6g" % x)


def read_site(pe, name):
    va, kind, args = SITES[name] if name in SITES else MANTLE_SITES[name]
    b = pe.bytes_at(va, 16)
    if pe.section_of(va) != ".text":
        raise ValueError("%s: 0x%x not in .text" % (name, va))
    if kind in ("rip", "str"):
        target = args[0]
        for i in range(2, 8):
            disp = struct.unpack_from("<i", b, i)[0]
            if va + i + 4 + disp == target:
                if pe.section_of(target) != ".rdata":
                    raise ValueError("%s: target 0x%x not in .rdata" % (name, target))
                if kind == "str":
                    s = pe.bytes_at(target, 64).split(b"\0")[0].decode("ascii", "replace")
                    if s != args[1]:
                        raise ValueError("%s: the string at 0x%x is %r, want %r" % (name, target, s, args[1]))
                    return 0, "iw4sp.exe @0x%x -> .rdata 0x%x %r" % (va, target, s)
                return r6(pe.f32(target)), "iw4sp.exe @0x%x -> .rdata 0x%x" % (va, target)
        raise ValueError("%s: the instruction at 0x%x does not reference 0x%x (bytes %s)" % (name, va, target, b[:10].hex()))
    prefix = bytes.fromhex(args[0])
    if not b.startswith(prefix):
        raise ValueError("%s: bytes at 0x%x are %s, want %s.." % (name, va, b[:10].hex(), prefix.hex()))
    rest = b[len(prefix):]
    if kind == "imm":
        return struct.unpack_from("<i", rest, 0)[0], "iw4sp.exe @0x%x imm" % va
    if kind == "imm8":
        return rest[0], "iw4sp.exe @0x%x disp8" % va
    if kind == "immf":
        return r6(struct.unpack_from("<f", rest, 0)[0]), "iw4sp.exe @0x%x imm f32" % va
    if kind == "bytes":
        return 0, "iw4sp.exe @0x%x %s" % (va, prefix.hex())
    raise ValueError(kind)


def read_curve(pe, va):
    pts = []
    for k in range(16):
        pct, inch, _ = struct.unpack_from("<ifi", pe.d, pe.off(va + 12 * k))
        if pct < 0:
            break
        pts.append([pct, r6(inch)])
    return pts


def curve_at(pts, frac):
    pct = frac * 100.0
    for a, b in zip(pts, pts[1:]):
        if pct <= b[0]:
            t = (pct - a[0]) / (b[0] - a[0]) if b[0] > a[0] else 1.0
            return a[1] + (b[1] - a[1]) * t
    return pts[-1][1]


def parse_weaponfile(text):
    parts = text.strip().split("\\")
    if not parts or parts[0] != "WEAPONFILE":
        raise ValueError("not a WEAPONFILE")
    kv = {}
    for k, v in zip(parts[1::2], parts[2::2]):
        kv[k] = v
    return kv


WEAPON_FIELDS = ["moveSpeedScale", "adsMoveSpeedScale", "sprintDurationScale", "sprintInTime", "sprintLoopTime",
                 "sprintOutTime", "hipSpreadStandMin", "hipSpreadMax", "hipSpreadDuckedMin", "hipSpreadDuckedMax",
                 "hipSpreadProneMin", "hipSpreadProneMax", "hipSpreadDecayRate", "hipSpreadFireAdd", "hipSpreadMoveAdd",
                 "hipSpreadDuckedDecay", "hipSpreadProneDecay", "duckedOfsF", "duckedOfsR", "duckedOfsU", "proneOfsF",
                 "proneOfsR", "proneOfsU", "standMoveF", "standMoveR", "standMoveU", "duckedMoveF", "duckedMoveR",
                 "duckedMoveU", "proneMoveF", "proneMoveR", "proneMoveU", "strafeMoveF", "strafeMoveR", "strafeMoveU",
                 "strafeRotP", "strafeRotY", "strafeRotR", "proneRotP", "proneRotY", "proneRotR", "posMoveRate",
                 "posProneMoveRate", "posRotRate", "posProneRotRate", "idleCrouchFactor", "idleProneFactor",
                 "adsBobFactor", "adsViewBobMult"]


def parse_binds(text):
    out = {}
    for m in re.finditer(r'(?im)^\s*bind\s+(\S+)\s+"([^"]+)"', text):
        out.setdefault(m.group(2), m.group(1))
    return out


def class_scales(text):
    """updateMoveSpeedScale: case "<class>": followed by setMoveSpeedScale( <x> * ...)."""
    out = {}
    for m in re.finditer(r'case\s+"(\w+)"\s*:\s*\n\s*self\s+setMoveSpeedScale\(\s*([0-9.]+)\s*\*', text):
        out[m.group(1)] = float(m.group(2))
    return out


def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def convert(paths):
    """Builds the movement dict from the inputs. Returns (dict, problems, notes)."""
    problems, notes = [], []
    exe_bytes = Path(paths["exe"]).read_bytes()
    exe_sha = hashlib.sha256(exe_bytes).hexdigest()
    pe = Pe(exe_bytes)
    exe_changed = exe_sha != PINNED_EXE_SHA
    consts = {}

    def put(name, value, unit, src, label):
        consts[name] = {"value": value, "unit": unit, "src": src, "label": label}

    site_vals = {}
    for name in SITES:
        try:
            v, src = read_site(pe, name)
        except Exception as e:  # noqa: BLE001
            if exe_changed:
                notes.append("NEEDS-DATA %s: exe changed (%s); the pinned value is kept" % (name, e))
                v, src = KNOWN[name], "pinned (exe sha changed)"
            else:
                problems.append(str(e))
                continue
        site_vals[name] = (v, src)
    for name, (v, src) in site_vals.items():
        if name.startswith("hull_half_") or name in ("stance_lea_200", "sprint_forward_min", "sprint_recharge_pause_s") \
                or name in INFO_SITES or name in HELPER_SITES:
            continue
        put(name, v, UNITS.get(name, "x"), src + (MAIN_FALL_NOTE if name in ("fall_min", "fall_max") else ""), "V")
    # the two sprint dvars, each proven by its default operand + the dvar name (+ rdi = 0) sites
    if all(k in site_vals for k in ("sprint_forward_min", "sprint_forward_min_rdi0", "sprint_forward_min_name")):
        put("sprint_forward_min", float(site_vals["sprint_forward_min"][0]), "of 127",
            "%s; rdi=0: %s; name: %s" % (site_vals["sprint_forward_min"][1], site_vals["sprint_forward_min_rdi0"][1],
                                         site_vals["sprint_forward_min_name"][1]), "V")
    if all(k in site_vals for k in ("sprint_recharge_pause_s", "sprint_recharge_pause_name")):
        put("sprint_recharge_pause_s", 0.0, "s", "%s (xorps xmm1,xmm1 = default 0); name: %s"
            % (site_vals["sprint_recharge_pause_s"][1], site_vals["sprint_recharge_pause_name"][1]), "V")
    if "hull_half_stand" in site_vals:
        for st in ("stand", "crouch", "prone"):
            v, src = site_vals["hull_half_" + st]
            put("hull_" + st, r6(2 * v), "in", src + " (half size x 2)", "V")
    if "stance_lea_200" in site_vals:
        v, src = site_vals["stance_lea_200"]
        put("stand_crouch_ms", 2 * v, "ms", src + " (lea ebp,[r14+64h]; lea r13d,[rbp+64h]: 200)", "V")
    for extra in INFO_SITES:
        if extra in site_vals:
            v, src = site_vals[extra]
            if extra.endswith("_alt"):
                unit = "%" if extra in ("fall_scale_alt", "fall_cap_alt") else "in"
                if extra == "fall_cap_alt":
                    src += "; ebx=0: " + site_vals.get("fall_alt_rbx0", (0, "?"))[1]
                put("info_" + extra, v, unit, ALT_FALL_NOTE % src, "V")
            else:
                put("info_" + extra, v, "x", src, "V")
    # MP inference: jump_slowdownEnable is 0 in the SP exe; the MP default is assumed on (config default true)
    put("info_jump_slowdown_mp_default", 1, "bool", "inferred from MW2 MP behaviour (SP exe default 0)", "I")
    for name, (v, unit, src, label) in DECODED.items():
        put(name, v, unit, src, label)

    # eye curves
    curves = {}
    for name, va in CURVE_TABLES.items():
        try:
            curves[name] = read_curve(pe, va)
        except Exception as e:  # noqa: BLE001
            problems.append("curve %s: %s" % (name, e))
    if len(curves) == 4:
        put("eye_stand", curves["stand_crouch"][0][1], "in", "PM_ViewHeightAdjust table 0x1403BD470[0]", "V")
        put("eye_crouch", curves["crouch_stand"][0][1], "in", "PM_ViewHeightAdjust table 0x1403BD4E0[0]", "V")
        put("eye_prone", curves["prone_crouch"][0][1], "in", "PM_ViewHeightAdjust table 0x1403BD5E0[0]", "V")

    # weapon file
    wtext = Path(paths["weapon"]).read_text(encoding="latin-1")
    wkv = parse_weaponfile(wtext)
    wfields = {}
    for f in WEAPON_FIELDS:
        if f not in wkv:
            problems.append("weapon field %s missing" % f)
            continue
        wfields[f] = r6(float(wkv[f]))

    # cfg / gsc
    binds = parse_binds(Path(paths["controls"]).read_text(encoding="latin-1"))
    gs = Path(paths["gamesettings"]).read_text(encoding="latin-1")
    m = re.search(r"(?m)^\s*set\s+scr_player_sprinttime\s+(\S+)", gs)
    mp_sprint = float(m.group(1)) if m else None
    if mp_sprint is None:
        problems.append("scr_player_sprinttime not found")
    else:
        put("info_scr_player_sprinttime", mp_sprint, "s", "default_mp_gamesettings.cfg", "V")
    cls = class_scales(Path(paths["weapons_gsc"]).read_text(encoding="latin-1"))
    for k, src_k in (("smg", "smg"), ("rifle", "rifle"), ("mg", "mg"), ("launcher", "rocketlauncher")):
        if src_k in cls:
            put("class_" + k, cls[src_k], "x", "_weapons.gsc updateMoveSpeedScale case %s" % src_k, "V")
        else:
            problems.append("class scale %s not found" % src_k)
    perk = Path(paths["perks_gsc"]).read_text(encoding="latin-1")
    lw = re.search(r"setLightWeight\(\)\s*\{\s*self\.moveSpeedScaler\s*=\s*([0-9.]+)", perk)
    if lw:
        put("lightweight_scale", float(lw.group(1)), "x", "_perkfunctions.gsc setLightWeight", "V")
    else:
        problems.append("lightweight scaler not found")

    data = {
        "schema": 1,
        "generated_by": "tools/scripts/convert_movement.py",
        "exe_sha256": exe_sha,
        "inputs": {k: {"path": str(Path(p).resolve()), "sha256": sha(p)} for k, p in paths.items() if k in BASE_INPUTS},
        "constants": dict(sorted(consts.items())),
        "eye_curves": dict({"src": "iw4sp.exe PM_ViewHeightAdjust 0x14007C510 tables 0x1403BD470/4E0/550/5E0"}, **curves),
        "weapon": {"name": "ump45_mp", "src": "dumps/common_mp/weapons/ump45_mp", "fields": wfields},
        "binds": {cmd: binds.get(cmd, "") for cmd in BINDS_WANT},
        # M8: the mantle block, after every M5 key (which stay byte-identical)
        "mantle": convert_mantle(pe, paths, problems),
    }
    return data, problems, notes, {"mp_sprint": mp_sprint, "exe_changed": exe_changed, "pe": pe, "curves": curves}


BASE_INPUTS = ("exe", "weapon", "controls", "gamesettings", "weapons_gsc", "perks_gsc")


def default_paths():
    return {"exe": EXE, "weapon": WEAPON, "controls": CONTROLS, "gamesettings": GAMESETTINGS, "weapons_gsc": WEAPONS_GSC,
            "perks_gsc": PERKS_GSC, "xanim_dir": MANTLE_XANIM_DIR, "mp_dir": MANTLE_MP_DIR, "weapons_dir": MANTLE_WEAPONS_DIR}


def mconst(value, unit, src, label):
    return {"value": value, "unit": unit, "src": src, "label": label}


def convert_mantle(pe, paths, problems):
    """The "mantle" block (docs/M8_DESIGN.md 1.5): the 46 byte-checked sites, the transition table, the 10 SP xanims'
    normalized delta curves, the 34 guns' quick drop / raise times, the MP set as info. Appends to `problems`."""
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import xanim_read  # noqa: E402  (the same folder; -I drops it from sys.path)

    sv = {}
    for name in MANTLE_SITES:
        try:
            sv[name] = read_site(pe, name)
        except Exception as e:  # noqa: BLE001
            problems.append("mantle site %s: %s" % (name, e))
            continue
        if name in MANTLE_KNOWN:
            want, got = float(MANTLE_KNOWN[name]), float(sv[name][0])
            if abs(got - want) > 1e-6 * max(1.0, abs(want)):
                problems.append("mantle site %s = %r want %r" % (name, got, want))
    c = {}

    def site(name, key, unit, label="V", value=None, extra=""):
        if name not in sv:
            return
        v, src = sv[name]
        c[key] = mconst(v if value is None else value, unit, src + extra, label)

    site("mantle_enable_default", "mantle_enable", "bool", extra="; name: %s" % sv.get("mantle_enable_name", (0, "?"))[1])
    site("mantle_debug_default", "mantle_debug", "bool")
    site("mantle_check_range", "mantle_check_range", "in",
         extra=" (dvar default; max %s; the probe reach beyond the player radius); name: %s"
         % (sv.get("mantle_check_range_max", (0, "?"))[0], sv.get("mantle_check_range_name", (0, "?"))[1]))
    site("mantle_check_radius", "mantle_check_radius", "in", extra="; name: %s" % sv.get("mantle_check_radius_name", (0, "?"))[1])
    site("mantle_check_angle", "mantle_check_angle", "deg", extra="; name: %s" % sv.get("mantle_check_angle_name", (0, "?"))[1])
    site("mantle_view_yawcap", "mantle_view_yawcap", "deg", extra=" (not applied in ER: deviation, I); name: %s"
         % sv.get("mantle_view_yawcap_name", (0, "?"))[1])
    site("mantle_max_vert_end_vel", "mantle_max_vertical_end_velocity", "in/s", value=0.0,
         extra=" (xorps xmm1,xmm1 = default 0); name: %s" % sv.get("mantle_mvev_name", (0, "?"))[1])
    for h in (60, 40, 20):
        site("ledge_h_%d" % h, "ledge_h_%d" % h, "in", extra=" (Mantle_FindLedge test height)")
    site("ledge_forward", "ledge_forward", "in", extra=" (Mantle_CheckLedgeHeight: ledge point = start + dir x 16)")
    site("ledge_floor", "ledge_floor", "in", extra=" (the ledge floor: the top must rise more than this)")
    site("front_player_radius", "player_radius", "in", extra=" (box half xy of the probes)")
    if "ledge_clear_mid_z" in sv and "ledge_clear_half_z" in sv:
        c["ledge_clear_height"] = mconst(r6(sv["ledge_clear_mid_z"][0] + sv["ledge_clear_half_z"][0]), "in",
                                         "%s + %s (clearance box z 0..mid+half = the crouch hull)" % (sv["ledge_clear_mid_z"][1], sv["ledge_clear_half_z"][1]), "V")
    if "front_half_z_a" in sv and "front_half_z_b" in sv:
        c["front_box_height"] = mconst(r6(sv["front_half_z_a"][0] + sv["front_half_z_b"][0]), "in",
                                       "%s + %s (front probe box z 0..70)" % (sv["front_half_z_a"][1], sv["front_half_z_b"][1]), "V")
    site("over_forward", "over_forward", "in", extra=" (Mantle_CalcPath over: forward 31 in at the ledge height)")
    if "over_drop" in sv:
        c["over_drop"] = mconst(abs(sv["over_drop"][0]), "in", sv["over_drop"][1] + " (-18: the over drop, stored positive)", "V")
    if "over_clear_mid_z" in sv and "over_clear_half_z" in sv:
        c["over_clear_height"] = mconst(r6(sv["over_clear_mid_z"][0] + sv["over_clear_half_z"][0]), "in",
                                        "%s + %s" % (sv["over_clear_mid_z"][1], sv["over_clear_half_z"][1]), "V")
    for n, unit in (("gate_pm_type", "pm_type <"), ("gate_mantling", "pm_flags bit"), ("gate_stance", "eFlags mask"),
                    ("gate_jump", "cmd.buttons mask"), ("gate_force", "flag"), ("ev_mantle", "event"),
                    ("weapon_inactive_over_low", "over anim idx"), ("quick_raise_flag", "flag"), ("front_contents", "contents"),
                    ("front_surf_mask", "surf mask"), ("front_surf_over", "surf flag"), ("front_normal_min", "x"),
                    ("ledge_box_half_x", "in"), ("ledge_box_half_z", "in")):
        site(n, "info_" + n, unit)
    # meanings decoded, not byte-checked (D), and the user's choices (I)
    c["info_pmf_mantle_meaning"] = mconst(4, "pm_flags bit", "PMF_MANTLE (IW4L name; the bit is V at gate_mantling)", "D")
    c["info_eflags_low_meaning"] = mconst(12, "eFlags mask", "crouched | prone (IW4L names; the mask is V at gate_stance)", "D")
    c["info_mantle_events_not_ported"] = mconst(2, "events", "EV 6/7 mantle foley / camera (IW4L names), not ported", "D")
    c["max_height_above_ground_m"] = mconst(2.0, "m", "the user's choice (run 17): top at most 2.0 m above the ground left", "I")
    c["airborne_reach_in"] = mconst(99.0, "in", "60 in above the feet at the 39 in jump apex (MW2's airborne reach)", "I")
    c["mantle_cross_distance_eased"] = mconst(1, "bool", "M8 review: the forward move keeps MW2's 167 ms (up = 0.500 s) and eases over "
                                              "the full-footprint X = max(16, d + r + skin + 1) in instead of MW2's 16 in "
                                              "(not 167 ms x X/16, which always hit its 2x cap: 0.667 s); a raised rim adds a "
                                              "50 ms set-down", "I")

    # the transition table and its anim names (.rdata, bound by name at 0x1403BCC30)
    table = []
    try:
        # initialized data in the file (.data, like the eye-height tables)
        if pe.section_of(MANTLE_TABLE_VA) != ".data" or pe.section_of(MANTLE_NAMES_VA) != ".data":
            raise ValueError("table not in .data")
        names = []
        for i in range(11):
            p = struct.unpack_from("<Q", pe.d, pe.off(MANTLE_NAMES_VA + 8 * i))[0]
            names.append(pe.bytes_at(p, 64).split(b"\0")[0].decode("ascii", "replace"))
        over_low = sv.get("weapon_inactive_over_low", (10, ""))[0]
        for i in range(7):
            o = MANTLE_TABLE_VA + 12 * i
            up, ov = struct.unpack_from("<ii", pe.d, pe.off(o))
            h = r6(pe.f32(o + 8))
            table.append({"row": i, "height": h, "up": names[up], "over": names[ov],
                          "weapon": "keep" if ov == over_low else "drop",
                          "src": "iw4sp.exe .data 0x%x {up %d, over %d, h}; names 0x%x; weapon: BG_IsWeaponInactive over != %d" % (o, up, ov, MANTLE_NAMES_VA, over_low),
                          "label": "V"})
    except Exception as e:  # noqa: BLE001
        problems.append("mantle table: %s" % e)

    # the SP xanims (delta translation, normalized per frame)
    anims, inputs = {}, {}
    xdir = Path(paths.get("xanim_dir", MANTLE_XANIM_DIR))
    for name in MANTLE_SP_ANIMS:
        p = xdir / name
        try:
            a = xanim_read.read_xanim(str(p))
            tr = a["delta"]["trans"] if a["delta"] else None
            if tr is None:
                raise ValueError("no delta translation")
            nf = a["numframes"]
            end = [r6(v) for v in xanim_read.sample(tr, "t", nf)]
            want = (31.0, 0.0, -18.0) if "_over_" in name else (16.0, 0.0, float(name.rsplit("_", 1)[1]))
            if any(abs(e - w) > 1.0 for e, w in zip(end, want)):
                raise ValueError("end delta %s, want %s (+-1 in, the exe's load check 0x140071200)" % (end, want))
            xs = [r6(xanim_read.sample(tr, "t", f)[0] / end[0]) for f in range(nf + 1)]
            zs = [r6(xanim_read.sample(tr, "t", f)[2] / end[2]) for f in range(nf + 1)]
            anims[name] = {"frames": nf, "fps": a["framerate"], "dur_ms": r6(a["duration"] * 1000.0), "end": end, "x": xs, "z": zs,
                           "sha256": sha(p), "src": "dumps/m8/code_post_gfx/xanim/%s (OAT dump; xanim_read.py)" % name, "label": "V"}
            inputs[name] = {"path": str(p.resolve()), "sha256": sha(p)}
        except Exception as e:  # noqa: BLE001
            problems.append("mantle xanim %s: %s" % (name, e))
    if all(n in anims for n in ("player_mantle_up_57", "player_mantle_over_high")):
        up, ov = anims["player_mantle_up_57"], anims["player_mantle_over_high"]
        k = max(i for i, v in enumerate(up["x"]) if abs(v) < 1e-3)
        c["mantle_up_ms"] = mconst(up["dur_ms"], "ms", "player_mantle_up_* %d frames at %d fps" % (up["frames"], up["fps"]), "V")
        c["mantle_rise_ms"] = mconst(r6(k * 1000.0 / up["fps"]), "ms", "frames 0..%d of the up anims: z only" % k, "V")
        c["mantle_cross_ms"] = mconst(r6((up["frames"] - k) * 1000.0 / up["fps"]), "ms", "frames %d..%d: x only (16 in)" % (k, up["frames"]), "V")
        c["mantle_over_ms"] = mconst(ov["dur_ms"], "ms", "player_mantle_over_* %d frames at %d fps" % (ov["frames"], ov["fps"]), "V")

    # the guns' quick drop / raise (the gun goes to "none" for rows >= 33 in)
    quick = {}
    wdir = Path(paths.get("weapons_dir", MANTLE_WEAPONS_DIR))
    for g in MANTLE_GUNS:
        p = wdir / ("%s_mp" % g)
        try:
            kv = parse_weaponfile(p.read_text(encoding="latin-1"))
            # M8 review: the clips too (V names). MW2 plays quickDropAnim / quickRaiseAnim; 14 of the 34 guns have
            # dedicated *_quick xanims, the rest reuse dropAnim / raiseAnim (our putaway / pullout clip)
            quick["%s_mp" % g] = {"drop": r6(float(kv["quickDropTime"])), "raise": r6(float(kv["quickRaiseTime"])),
                                  "src": "dumps/common_mp/weapons/%s_mp quickDropTime / quickRaiseTime / quickDropAnim / quickRaiseAnim" % g,
                                  "label": "V", "drop_anim": kv["quickDropAnim"], "raise_anim": kv["quickRaiseAnim"],
                                  "drop_is_putaway": kv["quickDropAnim"] == kv.get("dropAnim"),
                                  "raise_is_pullout": kv["quickRaiseAnim"] == kv.get("raiseAnim")}
        except Exception as e:  # noqa: BLE001
            problems.append("mantle quick times %s: %s" % (g, e))

    # the MP set: information only (never read by the controller)
    info_mp = {}
    mdir = Path(paths.get("mp_dir", MANTLE_MP_DIR))
    if mdir.is_dir():
        for p in sorted(mdir.glob("mp_mantle_*")):
            try:
                a = xanim_read.read_xanim(str(p))
                info_mp[p.name] = {"frames": a["numframes"], "fps": a["framerate"], "dur_ms": r6(a["duration"] * 1000.0)}
            except Exception as e:  # noqa: BLE001
                info_mp[p.name] = {"error": str(e)[:80]}
    curves_sha = hashlib.sha256(json.dumps(anims, sort_keys=True).encode("utf-8")).hexdigest()
    return {
        "src": ("iw4sp.exe Mantle_* 0x140070940-0x140071D90 (46 byte-checked sites) + the SP xanims player_mantle_* the exe binds "
                "by name at 0x1403BCC30 (dumps/m8/code_post_gfx); the MP set (dumps/m8/common) is info only"),
        "constants": dict(sorted(c.items())),
        "table": table,
        "anims": anims,
        "weapon_quick": quick,
        "info_mp": info_mp,
        "curves_sha256": curves_sha,
        "inputs": inputs,
    }


def mantle_known_check(m):
    """Known answers of the mantle block (docs/M8_DESIGN.md section 1)."""
    bad = []
    if not m:
        return ["mantle block missing"]
    c = m.get("constants", {})
    want = {"mantle_enable": 1, "mantle_debug": 0, "mantle_check_range": 20, "mantle_check_radius": 0.1, "mantle_check_angle": 60,
            "mantle_view_yawcap": 60, "mantle_max_vertical_end_velocity": 0, "ledge_h_60": 60, "ledge_h_40": 40, "ledge_h_20": 20,
            "ledge_forward": 16, "ledge_floor": 18, "player_radius": 15, "ledge_clear_height": 50, "front_box_height": 70,
            "over_forward": 31, "over_drop": 18, "over_clear_height": 50, "mantle_up_ms": 500, "mantle_rise_ms": 333.333,
            "mantle_cross_ms": 166.667, "mantle_over_ms": 666.667, "max_height_above_ground_m": 2.0, "airborne_reach_in": 99,
            "info_gate_jump": 0x400, "info_gate_stance": 12, "info_gate_mantling": 4, "info_weapon_inactive_over_low": 10}
    for k, w in want.items():
        got = c.get(k, {}).get("value")
        if got is None or abs(float(got) - w) > 1e-3 * max(1.0, abs(w)):
            bad.append("mantle %s=%r want %r" % (k, got, w))
    for k, v in c.items():
        if v.get("label") not in ("V", "D", "I") or not str(v.get("src", "")).strip() or not str(v.get("unit", "")).strip():
            bad.append("mantle constant %s: label/src/unit" % k)
        if v.get("label") == "V" and "(decode" in v.get("src", ""):
            bad.append("mantle %s is V but decode only" % k)
    for k, w in (("max_height_above_ground_m", "I"), ("airborne_reach_in", "I"), ("info_pmf_mantle_meaning", "D"), ("mantle_check_range", "V")):
        if c.get(k, {}).get("label") != w:
            bad.append("mantle label %s %r want %r" % (k, c.get(k, {}).get("label"), w))
    t = m.get("table", [])
    got_t = [(r.get("up"), r.get("over"), r.get("height"), r.get("weapon")) for r in t]
    want_t = [("player_mantle_up_%d" % h, "player_mantle_over_%s" % ("high" if h >= 51 else "mid" if h >= 33 else "low"), float(h),
               "drop" if h >= 33 else "keep") for h in (57, 51, 45, 39, 33, 27, 21)]
    if got_t != want_t:
        bad.append("mantle table %s" % got_t)
    if any(r.get("label") not in ("V", "D", "I") for r in t):
        bad.append("mantle table label")
    a = m.get("anims", {})
    if sorted(a) != sorted(MANTLE_SP_ANIMS):
        bad.append("mantle anims %s" % sorted(a))
    for n, axis, f, w in MANTLE_CURVE_KNOWN:
        try:
            got = a[n][axis][f]
        except (KeyError, IndexError):
            got = None
        if got is None or abs(got - w) > 0.002:
            bad.append("mantle curve %s %s[%d]=%r want %r" % (n, axis, f, got, w))
    for n, v in a.items():
        want_ms = 666.667 if "_over_" in n else 500.0
        if abs(v.get("dur_ms", 0) - want_ms) > 1.0 or v.get("fps") != 30 or v.get("label") != "V":
            bad.append("mantle anim %s dur %r fps %r" % (n, v.get("dur_ms"), v.get("fps")))
    q = m.get("weapon_quick", {})
    if len(q) != len(MANTLE_GUNS):
        bad.append("mantle quick times for %d guns (want %d)" % (len(q), len(MANTLE_GUNS)))
    for g, (dr, ra) in MANTLE_QUICK_KNOWN.items():
        if (q.get(g, {}).get("drop"), q.get(g, {}).get("raise")) != (dr, ra):
            bad.append("mantle quick %s %r want %r" % (g, q.get(g), (dr, ra)))
    if any(v.get("label") != "V" for v in q.values()):
        bad.append("mantle quick label")
    # M8 review: the quick clips by name (V): the UMP45 reuses its putaway / pullout, the M4 has its own quick xanims
    for g, (da, ra, de, re_) in MANTLE_QUICK_CLIPS_KNOWN.items():
        v = q.get(g, {})
        if (v.get("drop_anim"), v.get("raise_anim"), v.get("drop_is_putaway"), v.get("raise_is_pullout")) != (da, ra, de, re_):
            bad.append("mantle quick clips %s %r want %r" % (g, (v.get("drop_anim"), v.get("raise_anim"), v.get("drop_is_putaway"),
                                                                 v.get("raise_is_pullout")), (da, ra, de, re_)))
    if any(not str(v.get("drop_anim", "")).startswith("viewmodel_") or not str(v.get("raise_anim", "")).startswith("viewmodel_") for v in q.values()):
        bad.append("mantle quick clip names missing")
    cr = c.get("mantle_cross_distance_eased", {})
    if cr.get("label") != "I" or cr.get("value") != 1:
        bad.append("mantle_cross_distance_eased must be 1 with label I (the M8 review timing rule)")
    if len(m.get("info_mp", {})) < 20:
        bad.append("mantle info_mp has %d MP anims (want 20)" % len(m.get("info_mp", {})))
    return bad


def known_answer_check(data, extra):
    bad = []
    c = data["constants"]
    for name, want in KNOWN.items():
        key = name
        if name.startswith("hull_half_"):
            key = "hull_" + name[len("hull_half_"):]
            want = 2 * want
        elif name == "stance_lea_200":
            key, want = "stand_crouch_ms", 200
        elif name in INFO_SITES:
            key = "info_" + name
        elif name in HELPER_SITES:
            continue  # read_site already compared the bytes / the dvar name
        got = c.get(key, {}).get("value")
        if got is None or abs(float(got) - float(want)) > 1e-6 * max(1.0, abs(float(want))):
            bad.append("%s=%r want %r" % (key, got, want))
    for name, frac, want in CURVE_KNOWN:
        pts = data["eye_curves"].get(name)
        got = curve_at(pts, frac) if pts else None
        if got is None or abs(got - want) > 1e-4:
            bad.append("curve %s at %.0f%% = %r want %r" % (name, frac * 100, got, want))
    w = data["weapon"]["fields"]
    for f, want in WEAPON_KNOWN.items():
        if abs(w.get(f, -999) - want) > 1e-6:
            bad.append("weapon %s=%r want %r" % (f, w.get(f), want))
    if extra["mp_sprint"] is not None and abs(extra["mp_sprint"] - c.get("sprint_time_s", {}).get("value", -1)) > 1e-6:
        bad.append("scr_player_sprinttime %r != player_sprintTime %r" % (extra["mp_sprint"], c.get("sprint_time_s", {}).get("value")))
    for cmd, key in BINDS_WANT.items():
        if data["binds"].get(cmd, "").lower() != key.lower():
            bad.append("bind %s = %r want %r" % (cmd, data["binds"].get(cmd), key))
    want_cls = {"class_smg": 1.0, "class_rifle": 0.95, "class_mg": 0.875, "class_launcher": 0.8, "lightweight_scale": 1.07}
    for k, v in want_cls.items():
        if abs(c.get(k, {}).get("value", -1) - v) > 1e-9:
            bad.append("%s=%r want %r" % (k, c.get(k, {}).get("value"), v))
    for k, v in c.items():
        if v["label"] not in ("V", "D", "I"):
            bad.append("label %s %r" % (k, v["label"]))
        # a V row must name its byte check (an exe site, a table, a dump file), never only the research decode
        if v["label"] == "V" and "(decode" in v["src"]:
            bad.append("label %s is V but its src is a decode only" % k)
    for k, want in (("ads_walk_scale", "I"), ("sprint_forward_min", "V"), ("sprint_recharge_pause_s", "V")):
        if c.get(k, {}).get("label") != want:
            bad.append("label %s %r want %r" % (k, c.get(k, {}).get("label"), want))
    bad += mantle_known_check(data.get("mantle"))
    return bad


def pre_m8_text(data):
    """converted/movement.json as it would be written without the M8 block (the bytes the pre-M8 file had)."""
    old = {k: v for k, v in data.items() if k != "mantle"}
    return (json.dumps(old, indent=1, sort_keys=False) + "\n").replace("\n", os.linesep).encode("utf-8")


def write_atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, indent=1, sort_keys=False) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def do_check(paths, out_path, verbose=True, post=None):
    data, problems, notes, extra = convert(paths)
    if post is not None:
        post(data)  # the self-test's output mutations (a label removed, ...)
    bad = problems + known_answer_check(data, extra)
    # M8: every pre-M8 key byte-identical (only for the real inputs: their paths are part of the bytes)
    if out_path is not None and not extra["exe_changed"] and hashlib.sha256(pre_m8_text(data)).hexdigest() != PRE_M8_SHA:
        bad.append("the pre-M8 part of movement.json is not byte-identical (sha %s, pinned %s)"
                   % (hashlib.sha256(pre_m8_text(data)).hexdigest()[:12], PRE_M8_SHA[:12]))
    if out_path is None:
        pass
    elif not out_path.exists():
        bad.append("%s missing (run the converter)" % out_path)
    else:
        cur = json.loads(out_path.read_text(encoding="utf-8"))
        if cur != data:
            diff = [k for k in set(cur) | set(data) if cur.get(k) != data.get(k)]
            bad.append("converted/movement.json differs from a fresh conversion in %s" % diff)
        for k, v in cur.get("inputs", {}).items():
            if Path(v["path"]).exists() and sha(v["path"]) != v["sha256"]:
                bad.append("input %s hash changed" % k)
    for n in notes:
        print(n)
    if bad:
        if verbose:
            print("FAIL check: " + "; ".join(bad[:6]))
        return False
    c = data["constants"]
    if verbose:
        print("PASS check: converted/movement.json equals a fresh conversion (%d constants, %d V byte-checked / %d D decoded only / %d I inferred; movement.json hashes match its %d inputs); "
              "g_speed %s, sprint x%s %ss min %ss, back %s strafe %s sprint-strafe %s, crouch %s prone %s, gravity %s, jump %s in, "
              "friction %s/%s, accel %s/%s/%s air %s, step %s/%s, eye %s/%s/%s, hull %s/%s/%s, stance %s/%s ms, fall %s/%s, "
              "curves 51.2/15 in at 50%%, ump45 move %s ads %s dur %s, binds MW2 PC, exe sha %s%s"
              % (len(c), sum(1 for v in c.values() if v["label"] == "V"), sum(1 for v in c.values() if v["label"] == "D"),
                 sum(1 for v in c.values() if v["label"] == "I"), len(data["inputs"]),
                 c["g_speed"]["value"], c["sprint_speed_scale"]["value"], c["sprint_time_s"]["value"], c["sprint_min_time_s"]["value"],
                 c["back_speed_scale"]["value"], c["strafe_speed_scale"]["value"], c["sprint_strafe_speed_scale"]["value"],
                 c["crouch_speed_scale"]["value"], c["prone_speed_scale"]["value"], c["gravity"]["value"], c["jump_height"]["value"],
                 c["friction"]["value"], c["stopspeed"]["value"], c["accel_stand"]["value"], c["accel_crouch"]["value"], c["accel_prone"]["value"],
                 c["air_accel"]["value"], c["step_height"]["value"], c["step_height_prone"]["value"], c["eye_stand"]["value"],
                 c["eye_crouch"]["value"], c["eye_prone"]["value"], c["hull_stand"]["value"], c["hull_crouch"]["value"], c["hull_prone"]["value"],
                 c["stand_crouch_ms"]["value"], c["crouch_prone_ms"]["value"], c["fall_min"]["value"], c["fall_max"]["value"],
                 data["weapon"]["fields"]["moveSpeedScale"], data["weapon"]["fields"]["adsMoveSpeedScale"], data["weapon"]["fields"]["sprintDurationScale"],
                 data["exe_sha256"][:12], " (NEEDS-DATA: exe changed)" if extra["exe_changed"] else " (pinned)"))
        m = data["mantle"]
        mc = m["constants"]
        print("PASS mantle: %d byte-checked sites, %d constants (%d V / %d D / %d I), table %s, %d SP anims (up %s ms: rise %s + "
              "forward %s, over %s ms; end deltas within 1 in), quick drop/raise for %d guns (ump45 %s/%s s), %d MP anims info only, "
              "pre-M8 keys byte-identical (sha %s), curves sha %s"
              % (len(MANTLE_SITES), len(mc), sum(1 for v in mc.values() if v["label"] == "V"), sum(1 for v in mc.values() if v["label"] == "D"),
                 sum(1 for v in mc.values() if v["label"] == "I"), "/".join("%g%s" % (r["height"], "d" if r["weapon"] == "drop" else "k") for r in m["table"]),
                 len(m["anims"]), mc["mantle_up_ms"]["value"], mc["mantle_rise_ms"]["value"], mc["mantle_cross_ms"]["value"],
                 mc["mantle_over_ms"]["value"], len(m["weapon_quick"]), m["weapon_quick"]["ump45_mp"]["drop"],
                 m["weapon_quick"]["ump45_mp"]["raise"], len(m["info_mp"]), PRE_M8_SHA[:12], m["curves_sha256"][:12]))
    return True


def self_test(m8=False):
    """Each mutation, on copies under work/m5/selftest, must make --check FAIL (the control must PASS)."""
    if SELFTEST.exists():
        shutil.rmtree(SELFTEST)
    SELFTEST.mkdir(parents=True)
    base = default_paths()
    copies = {}
    # the M8 folders: only the files the converter reads (the 10 SP xanims, the 34 guns' weapon files); the MP set is
    # info only and read in place
    dir_files = {"xanim_dir": MANTLE_SP_ANIMS, "weapons_dir": ["%s_mp" % g for g in MANTLE_GUNS]}

    def restore():
        for k, p in base.items():
            if k == "mp_dir":
                copies[k] = Path(p)
            elif k in dir_files:
                dst = SELFTEST / k
                dst.mkdir(exist_ok=True)
                for n in dir_files[k]:
                    shutil.copyfile(Path(p) / n, dst / n)
                copies[k] = dst
            else:
                dst = SELFTEST / ("%s_%s" % (k, Path(p).name))
                shutil.copyfile(p, dst)
                copies[k] = dst

    restore()
    # mutations are judged by the known answers alone (not by the changed input hashes)
    ctl_out = None
    if not do_check(copies, ctl_out, verbose=False):
        print("FAIL self-test: the control (unmodified copies) does not pass")
        return False

    def patch_text(key, pattern, repl):
        t = copies[key].read_text(encoding="latin-1")
        t2, n = re.subn(pattern, repl, t, count=1)
        if n != 1:
            raise ValueError("pattern not found in %s" % key)
        copies[key].write_text(t2, encoding="latin-1")
        return t

    def patch_exe(va, newbytes):
        d = bytearray(copies["exe"].read_bytes())
        pe = Pe(bytes(d))
        o = pe.off(va)
        old = bytes(d[o:o + len(newbytes)])
        d[o:o + len(newbytes)] = newbytes
        copies["exe"].write_bytes(bytes(d))
        return old

    def patch_file(key, name, off, xor):
        p = copies[key] / name
        d = bytearray(p.read_bytes())
        d[off] ^= xor
        p.write_bytes(bytes(d))

    def patch_u16(key, name, off, v):
        p = copies[key] / name
        d = bytearray(p.read_bytes())
        d[off:off + 2] = struct.pack("<H", v)
        p.write_bytes(bytes(d))

    def drop_label(d):
        d["mantle"]["constants"]["ledge_forward"].pop("label", None)

    def drop_curve(d):
        d["mantle"]["anims"].pop("player_mantle_over_mid", None)

    caught = 0
    if not m8:
        muts = [
            ("sprintDurationScale 1->2", lambda: patch_text("weapon", r"\\sprintDurationScale\\1\\", r"\\sprintDurationScale\\2\\"), None),
            ("adsMoveSpeedScale 2->1", lambda: patch_text("weapon", r"\\adsMoveSpeedScale\\2\\", r"\\adsMoveSpeedScale\\1\\"), None),
            ("scr_player_sprinttime 4->5", lambda: patch_text("gamesettings", r"(scr_player_sprinttime\s+)4", r"\g<1>5"), None),
            (".rdata 0.65 -> 0.6", lambda: patch_exe(0x14031F8C4, struct.pack("<f", 0.6)), None),
            ("curve point stand->crouch 50% 44 -> 45", lambda: patch_exe(0x1403BD470 + 12 * 4 + 4, struct.pack("<f", 45.0)), None),
        ]
    else:
        # M8 (docs/M8_DESIGN.md 12.3): the mantle block's mutations
        muts = [
            ("mantle site byte (mantle_check_range disp)", lambda: patch_exe(0x140071A50 + 4, b"\x00"), None),
            (".rdata ledge_forward 16 -> 17", lambda: patch_exe(0x140385AE0, struct.pack("<f", 17.0)), None),
            ("table row 0 height 57 -> 58", lambda: patch_exe(MANTLE_TABLE_VA + 8, struct.pack("<f", 58.0)), None),
            ("table row 5 over anim low -> mid (weapon rule)", lambda: patch_exe(MANTLE_TABLE_VA + 12 * 5 + 4, struct.pack("<i", 9)), None),
            ("xanim up_57 curve frame 5 z", lambda: patch_file("xanim_dir", "player_mantle_up_57", 74, 0x10), None),
            ("xanim up_57 framerate 30 -> 24 (duration)", lambda: patch_u16("xanim_dir", "player_mantle_up_57", 8, 24), None),
            ("ump45 quickDropTime 0.25 -> 0.3", lambda: patch_text_dir("weapons_dir", "ump45_mp", r"\\quickDropTime\\0\.25\\", r"\\quickDropTime\\0.3\\"), None),
            ("ump45 quickDropAnim renamed (M8 review)", lambda: patch_text_dir("weapons_dir", "ump45_mp", r"\\quickDropAnim\\viewmodel_ump45_putaway\\",
                                                                               r"\\quickDropAnim\\viewmodel_ump45_putaway_quick\\"), None),
            ("a missing label (ledge_forward)", lambda: None, drop_label),
            ("a missing SP anim (over_mid)", lambda: None, drop_curve),
            ("the name of a dvar site", lambda: patch_exe(0x14031F9F8, b"x"), None),
        ]

    def patch_text_dir(key, name, pattern, repl):
        p = copies[key] / name
        t = p.read_text(encoding="latin-1")
        t2, n = re.subn(pattern, repl, t, count=1)
        if n != 1:
            raise ValueError("pattern not found in %s/%s" % (key, name))
        p.write_text(t2, encoding="latin-1")

    for label, mut, post in muts:
        # restore every copy, apply one mutation, check against the control output
        restore()
        mut()
        ok = do_check(copies, ctl_out, verbose=False, post=post)
        print("%s mutation %s: %s" % ("PASS" if not ok else "FAIL", label, "caught" if not ok else "NOT caught"))
        caught += 0 if ok else 1
    shutil.rmtree(SELFTEST, ignore_errors=True)
    if caught == len(muts):
        print("PASS self-test: %d/%d %smutations caught (control passed)" % (caught, len(muts), "M8 " if m8 else ""))
        return True
    print("FAIL self-test: %d/%d mutations caught" % (caught, len(muts)))
    return False


def iced_hex(v):
    """An immediate as iced-x86's Intel formatter prints it (decimal below 10, else hex with an h suffix)."""
    if 0 <= v < 10:
        return str(v)
    h = "%X" % v
    return ("0" + h if h[0] in "ABCDEF" else h) + "h"


def print_sites():
    """VA and the text tools/iw4dis must show at it (check_m5.ps1 check 6 compares them)."""
    pe = Pe(EXE.read_bytes())
    for name, (va, kind, args) in list(SITES.items()) + list(MANTLE_SITES.items()):
        if kind in ("rip", "str"):
            want = "[%Xh]" % args[0]
        elif kind == "bytes":
            want = args[1] if len(args) > 1 else "xor edx,edx"
        elif kind == "immf":
            b = pe.bytes_at(va, 16)[len(bytes.fromhex(args[0])):]
            want = iced_hex(struct.unpack_from("<I", b, 0)[0])
        else:
            v, _ = read_site(pe, name)
            want = iced_hex(v)
        print("%x	%s	%s" % (va, want, name))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--self-test-m8", action="store_true")
    ap.add_argument("--sites", action="store_true")
    a = ap.parse_args()
    if a.sites:
        print_sites()
        return 0
    paths = default_paths()
    for k, p in paths.items():
        if not Path(p).exists():
            print("FAIL input %s missing: %s" % (k, p))
            return 1
    if a.self_test:
        return 0 if self_test() else 1
    if a.self_test_m8:
        return 0 if self_test(m8=True) else 1
    if a.check:
        return 0 if do_check(paths, OUT) else 1
    data, problems, notes, extra = convert(paths)
    for n in notes:
        print(n)
    bad = problems + known_answer_check(data, extra)
    if bad:
        print("FAIL convert: " + "; ".join(bad[:6]))
        return 1
    write_atomic(OUT, data)
    print("PASS wrote %s (%d constants, %d curves, %d weapon fields)" % (OUT, len(data["constants"]), 4, len(data["weapon"]["fields"])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
