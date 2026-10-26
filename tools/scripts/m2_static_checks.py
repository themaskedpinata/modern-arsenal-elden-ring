"""Static (offline) checks for milestone M2. Read-only except where a subcommand says otherwise.

Usage: python -I m2_static_checks.py <check> [args]
  pe <dll>          x64 PE; exactly one export `DllMain`; no VCRUNTIME140* / api-ms-win-crt-* imports
  ids               build/rows.json agrees with mw2er.toml and converted/weapons.json (id scheme, k, atkPhys,
                    band coverage) and with build/package/regulation.bin (sha256); package holds only it
  profile [file]    profiles/mw2er.me3 shape (v1, eldenring, one native = the staged DLL, one enabled
                    package = build/package, savefile ER0000.mw2er.sl2, no start_online / disable_arxan)
  gitignore         .gitignore covers the build/work folders (/work/ /build/ /converted/ /logs/ /mod/target/
                    /vendor/*/target/) and everything from the games or the user (/dumps/ /backups/ /extracted/
                    /zones/ regulation.bin *.sl2)
  gamefolder        the game's regulation.bin still has the vanilla hash; no mw2er*, dinput8.dll or
                    regulation.bin.* files in the game folder (listing only)
  bootorder         mod/mw2er/src/boot.rs runs version_gate::check() before anything that touches the game
                    (data, hudhook, the input hook, audio, the boot gate, task registration) and returns on Err;
                    DllMain only spawns boot::run
  vmwiring          (M3) hud.rs registers the viewmodel callback (viewmodel::install -> PRE_IMGUI.set) before
                    Hudhook::builder(); the pre-ImGui callback body and the loader thread run inside guard(...);
                    --self-test: mutations of the sources must be caught
  shaders <dll>     (M3) the DLL contains the exact viewmodel_vs.cso / viewmodel_ps.cso bytes of the newest
                    mw2er-render build (same shaders as vmtest); prints their sha256
  hygiene           (M3) no file under tools/, mod/ (excluding target) or docs/ carries DDS/GLB/XAnim/converted
                    magic or is larger than 5 MB
  fpwiring          (M3b) game-memory writes: NtWriteVirtualMemory only in game/mod.rs (nt_copy), reached only
                    through write() (WRITES_ALLOWED first, then the cached per-page VirtualQuery check) and
                    write_exe_bool() (WRITES_ALLOWED, the one validated exe .data address); WriteProcessMemory
                    nowhere (run 5: it queries the region on every call, ~35 ms on ER's heap); WRITES_ALLOWED is set
                    only in boot() after version_gate::check() (Ok arm); no raw pointer writes in the first-person
                    code; the camera-step / follow-cam hook RVAs, prologues and the CAM_WALL_RECOVERY RVA are the
                    documented ones, prologues are checked before hooking, boot() installs the hooks only for
                    first_person.enabled && camera_mode = Hook (M4: or [ac130] enabled); the detours re-read the objects they write; erfps2
                    attribution in cam_hook.rs and MODLOG.md; the offset asserts exist.
                    --self-test: mutations of the sources must be caught
  fpstrings <dll>   (M3b) the DLL contains the log strings that prove first person in game
  ac130wiring       (M4) the 11 wiring checks of docs/M4_DESIGN.md section 14.5 (mode-2 detour: one game::write, no
                    allocation/log/lock/query; SpEffect engine calls only in ac130.rs behind WRITES_ALLOWED; every exit
                    reason reaches end(); nothing fallible between the thermal barriers; PRE_IMGUI once + snapshot
                    gate; the hook is installed for [ac130] enabled; no ChrCam/follow-cam writes in mode 2; guarded
                    hud_state restore; erfps2 + fromsoftware-rs attribution; symphonia only in mod/sndconv).
                    --self-test: mutations of the sources must be caught
  movewiring        (M5) the 9 wiring checks of docs/M5_DESIGN.md section 13.4 (+ --self-test mutations)
  movestrings <dll> (M5) the DLL contains the MOVE proof log strings and no iced / movebench / iw4dis string
  arsenalwiring     (M6) the 10 wiring checks of the arsenal + run-13 bug fixes (+ --self-test mutations)
  arsenalstrings <dll> (M6) the DLL contains the M6 proof log strings
  nukewiring        (M7) the 13 wiring checks of the emitter fix, the spawn guard and the Tactical Nuke (+ --self-test)
  nukestrings <dll> (M7) the DLL contains the M7 proof log strings
  ac130strings <dll> (M4) the DLL contains the AC-130 proof log strings and the thermal shader bytes, no "symphonia"
  c41wiring         (C4.1 PASS I41) ER's render-thread path of the UI-thread injection allocates, creates, logs and
                    blocks nothing; the canary / readback never barrier ER's layer; the Present path skips the gun
                    only on a Submitted frame; the key is cold and off by default; `ahead` is recorded every
                    UiBegin; the readback waits for UiEnd's own submit; the mw2er-render record functions on that
                    thread build no allocating error; the gun goes live only after the canary persistence check;
                    C6 depth 5 (rule 12): the read-only ResourceBarrier detour calls the original first and neither
                    allocates, logs nor locks, the hook only under its flag, recording off before the disable at
                    teardown, the fold before any splice, the copy recorded only in UiBegin and spliced only at 0 on a
                    proven readable state, no barrier of ours names ER's depth (+ --self-test mutations)
  m2rows            (M4) work/regulation/build_check (the read-back of build/package) keeps the 13 M2 rows and every
                    param M4 does not edit byte-identical to fixtures/m2_regulation_hashes.json (the M2 build)
Prints PASS/FAIL lines; exit 0 = PASS. Pure Python.
"""
import hashlib
import json
import math
import pathlib
import re
import struct
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
GAME_DIR = pathlib.Path(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game")
# port 2.7.2.0 (2026-10-20): the vanilla regulation of Steam build 25827938 (1.17.1 was 766521f9508d...)
VANILLA_SHA256 = "b8cdab84c0772f66811fdc5176a381877ca6657b37d420cc2c1562a90d63870e"
STAGED_DLL = ROOT / "build" / "natives" / "mw2er.dll"
PACKAGE_DIR = ROOT / "build" / "package"
GITIGNORE_REQUIRED = ["/work/", "/build/", "/converted/", "/logs/", "/mod/target/", "/vendor/*/target/",
                      "/dumps/", "/backups/", "/extracted/", "/zones/", "regulation.bin", "*.sl2"]
MOD_SRC = ROOT / "mod" / "mw2er" / "src"
# Calls in boot() that touch the game process or the data, in the order they must come after the gate.
AFTER_GATE = [("data load", "data::load_logged("), ("hudhook", "hud::apply("), ("input hook", "input_hook::install"),
              ("audio", "audio::start("), ("game window", "singletons::wait_for_window("),
              ("reflection scan", "singletons::wait_for_reflection("), ("task registration", "task::register(")]
# Nothing of these may appear in boot() before the gate.
BEFORE_GATE_FORBIDDEN = ["hud::", "input_hook::", "task::", "singletons::", "audio::", "data::", "game::",
                         "FromStatic", "instance", "eldenring::"]


def sha256(p):
    return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()


def rhaz(x):
    return int(math.floor(x + 0.5)) if x >= 0 else -int(math.floor(-x + 0.5))


def band_damages(damage, min_damage):
    hi, lo = max(damage, min_damage), min(damage, min_damage)
    if hi - lo + 1 <= 16:
        return list(range(hi, lo - 1, -1))
    out = []
    for i in range(16):
        v = rhaz(hi - (hi - lo) * i / 15)
        if not out or out[-1] != v:
            out.append(v)
    return out


def report(problems, ok_msg):
    for p in problems:
        print("FAIL " + p)
    if not problems:
        print("PASS " + ok_msg)
    return 0 if not problems else 1


# ---- pe -----------------------------------------------------------------------------------------------

def pe_info(path):
    b = pathlib.Path(path).read_bytes()
    if b[:2] != b"MZ":
        raise ValueError("not an MZ image")
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    if b[pe:pe + 4] != b"PE\0\0":
        raise ValueError("no PE signature")
    machine, nsec = struct.unpack_from("<HH", b, pe + 4)
    opt_size = struct.unpack_from("<H", b, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", b, opt)[0]
    if magic != 0x20B:
        raise ValueError(f"optional header magic 0x{magic:x} (want PE32+)")
    dd = opt + 112  # data directories for PE32+
    exp_rva, _ = struct.unpack_from("<II", b, dd)
    imp_rva, _ = struct.unpack_from("<II", b, dd + 8)
    sections = []
    for i in range(nsec):
        s = opt + opt_size + i * 40
        vsize, vaddr, rsize, rptr = struct.unpack_from("<IIII", b, s + 8)
        sections.append((vaddr, max(vsize, rsize), rptr))

    def off(rva):
        for va, size, ptr in sections:
            if va <= rva < va + size:
                return rva - va + ptr
        raise ValueError(f"rva 0x{rva:x} not in any section")

    def cstr(o):
        return b[o:b.index(b"\0", o)].decode("ascii", "replace")

    exports = []
    if exp_rva:
        e = off(exp_rva)
        nnames, _, names_rva = struct.unpack_from("<III", b, e + 24)
        for i in range(nnames):
            exports.append(cstr(off(struct.unpack_from("<I", b, off(names_rva) + 4 * i)[0])))
    imports = []
    if imp_rva:
        d = off(imp_rva)
        while True:
            name_rva = struct.unpack_from("<I", b, d + 12)[0]
            if name_rva == 0:
                break
            imports.append(cstr(off(name_rva)))
            d += 20
    return machine, exports, imports


def check_pe(path):
    try:
        machine, exports, imports = pe_info(path)
    except (OSError, ValueError, struct.error) as e:
        return report([f"{path}: {e}"], "")
    problems = []
    if machine != 0x8664:
        problems.append(f"machine 0x{machine:x} is not x64")
    if exports != ["DllMain"]:
        problems.append(f"exports {exports} (want exactly ['DllMain'])")
    bad = [i for i in imports if re.match(r"(?i)^(vcruntime140|api-ms-win-crt-)", i)]
    if bad:
        problems.append(f"CRT redistributable imports {bad} (static CRT expected)")
    return report(problems, f"pe: x64, exports={exports}, {len(imports)} imports, no CRT imports "
                            f"({', '.join(sorted(set(i.lower() for i in imports)))})")


# ---- ids ----------------------------------------------------------------------------------------------

def check_ids():
    problems = []
    try:
        cfg = tomllib.loads((ROOT / "mw2er.toml").read_text(encoding="utf-8"))
        rows = json.loads((ROOT / "build" / "rows.json").read_text(encoding="utf-8"))
        weapons = json.loads((ROOT / "converted" / "weapons.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        return report([f"inputs: {e}"], "")
    reg = cfg.get("regulation", {})
    k = float(cfg.get("damage", {}).get("k", 4.0))
    if abs(rows["k"] - k) > 1e-12:
        problems.append(f"rows.json k={rows['k']} but mw2er.toml damage.k={k} (rerun build_regulation.py)")
    if rows["id_base"] != reg.get("id_base") or rows["marker_id"] != reg.get("marker_id"):
        problems.append("rows.json id_base/marker_id differ from mw2er.toml [regulation]")
    slots = reg.get("weapons", {})
    if set(rows["weapons"]) != set(slots):
        problems.append(f"rows.json weapons {sorted(rows['weapons'])} != mw2er.toml {sorted(slots)}")
    n = 0
    for name, w in rows["weapons"].items():
        if w["slot"] != slots.get(name):
            problems.append(f"{name}: slot {w['slot']} != mw2er.toml {slots.get(name)}")
        mw2 = weapons["weapons"][name]["mw2"]
        want_bands = band_damages(int(mw2["damage"]), int(mw2["minDamage"]))
        got = [b["mw2_damage"] for b in w["bands"]]
        if got != want_bands:
            problems.append(f"{name}: bands {got} != {want_bands} from weapons.json")
        if int(mw2["damage"]) - int(mw2["minDamage"]) <= 15:
            cover = set(range(int(mw2["minDamage"]), int(mw2["damage"]) + 1))
            if set(got) != cover:
                problems.append(f"{name}: bands do not cover every integer {min(cover)}..{max(cover)}")
        for i, b in enumerate(w["bands"]):
            if b["band"] != i or b["id"] != rows["id_base"] + w["slot"] * 100 + b["band"]:
                problems.append(f"{name}: band {b['band']} id {b['id']} breaks id_base + slot*100 + band")
            if b["atk_phys"] != rhaz(b["mw2_damage"] * k):
                problems.append(f"{name}: band {b['band']} atk_phys {b['atk_phys']} != round({b['mw2_damage']}*{k})")
            n += 1
    pkg = PACKAGE_DIR / "regulation.bin"
    if not pkg.is_file():
        problems.append(f"{pkg} missing")
    else:
        if sha256(pkg) != rows.get("package_regulation_sha256"):
            problems.append("build/package/regulation.bin sha256 != rows.json package_regulation_sha256")
        others = [p.name for p in PACKAGE_DIR.iterdir() if p.name != "regulation.bin"]
        if others:
            problems.append(f"build/package contains more than regulation.bin: {others}")
    return report(problems, f"ids: {n} bands + marker {rows['marker_id']} consistent with mw2er.toml (k={k}), "
                            f"weapons.json and the package hash")


# ---- profile ------------------------------------------------------------------------------------------

def norm(p):
    return str(pathlib.Path(p)).rstrip("\\/").lower()


def check_profile(path):
    path = pathlib.Path(path)
    problems = []
    try:
        text = path.read_text(encoding="utf-8")
        prof = tomllib.loads(text)
    except (OSError, ValueError) as e:
        return report([f"{path}: {e}"], "")
    if prof.get("profileVersion") != "v1":
        problems.append(f"profileVersion {prof.get('profileVersion')!r} (want 'v1')")
    if [s.get("game") for s in prof.get("supports", [])] != ["eldenring"]:
        problems.append("[[supports]] must be exactly game = \"eldenring\"")
    natives = prof.get("natives", [])
    if not isinstance(natives, list) or len(natives) != 1:
        problems.append(f"expected exactly one [[natives]] (found {natives!r})")
    else:
        np_ = natives[0].get("path", "")
        if norm(np_) != norm(STAGED_DLL):
            problems.append(f"natives path {np_} != staged DLL {STAGED_DLL}")
        if not pathlib.Path(np_).is_file():
            problems.append(f"natives path {np_} does not exist (stage the DLL)")
    pkgs = prof.get("packages", [])
    if len(pkgs) != 1 or pkgs[0].get("enabled") is not True:
        problems.append("expected exactly one enabled [[packages]]")
    elif norm(pkgs[0].get("path", "")) != norm(PACKAGE_DIR) or not PACKAGE_DIR.is_dir():
        problems.append(f"packages path {pkgs[0].get('path')} != {PACKAGE_DIR} (or missing)")
    if prof.get("savefile") != "ER0000.mw2er.sl2":
        problems.append(f"savefile {prof.get('savefile')!r} (want 'ER0000.mw2er.sl2')")

    def walk(o, prefix=""):
        if isinstance(o, dict):
            for key, v in o.items():
                yield prefix + key
                yield from walk(v, prefix + key + ".")
        elif isinstance(o, list):
            for v in o:
                yield from walk(v, prefix)

    keys = list(walk(prof))
    for forbidden in ("start_online", "disable_arxan"):
        if any(k.split(".")[-1] == forbidden for k in keys):
            problems.append(f"{forbidden} must not be set")
    if re.search(r"(?m)^\s*natives\s*=\s*\[\s*\]", text):
        problems.append("'natives = []' next to [[natives]] (duplicate key)")
    return report(problems, f"profile: {path.name} v1, eldenring, native={STAGED_DLL.name} (exists), "
                            f"package=build\\package, savefile ER0000.mw2er.sl2, no start_online/disable_arxan")


# ---- gitignore / game folder -----------------------------------------------------------------------------

def check_gitignore():
    lines = {l.strip() for l in (ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()}
    missing = [p for p in GITIGNORE_REQUIRED if p not in lines]
    return report([f".gitignore lacks {m}" for m in missing], f"gitignore: {', '.join(GITIGNORE_REQUIRED)}")


def check_gamefolder():
    problems = []
    reg = GAME_DIR / "regulation.bin"
    if not reg.is_file():
        problems.append(f"{reg} missing")
    elif sha256(reg) != VANILLA_SHA256:
        problems.append("the game's regulation.bin no longer has the vanilla hash")
    if GAME_DIR.is_dir():
        for p in GAME_DIR.iterdir():
            n = p.name.lower()
            if n.startswith("mw2er") or n == "dinput8.dll" or n.startswith("regulation.bin."):
                problems.append(f"unexpected file in the game folder: {p.name}")
    return report(problems, "gamefolder: vanilla regulation.bin hash unchanged; no mw2er*, dinput8.dll or "
                            "regulation.bin.* files")


def strip_comments(src):
    """Rust source reduced to code: whole-line // comments (doc comments included) dropped and string literal
    contents emptied, so braces and words inside log messages do not count."""
    code = "\n".join(re.sub(r"^\s*//.*$", "", line) for line in src.splitlines())
    return re.sub(r'"(?:[^"\\]|\\.)*"', '""', code)


def fn_body(src, name):
    """Text of `fn name(` up to its matching closing brace (brace counting; good enough for this file)."""
    start = src.find(f"fn {name}(")
    if start < 0:
        return None
    i = src.index("{", start)
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[start:j + 1]
    return None


def bootorder_problems(boot_text, lib_text):
    problems = []
    boot_src, lib_src = strip_comments(boot_text), strip_comments(lib_text)
    body = fn_body(boot_src, "boot")
    if body is None:
        return ["boot.rs has no fn boot("]
    gate = body.find("version_gate::check()")
    if gate < 0:
        problems.append("boot() never calls version_gate::check()")
    else:
        before = body[:gate]
        for tok in BEFORE_GATE_FORBIDDEN:
            if tok in before:
                problems.append(f"'{tok}' is used in boot() before the version gate")
        arm = re.search(r"Err\([^)]*\)\s*=>\s*\{(.*?)\}", body[gate:], re.S)
        if not arm or "return" not in arm.group(1):
            problems.append("the version gate's Err arm does not return (the DLL must stay inert)")
        for label, tok in AFTER_GATE:
            pos = body.find(tok)
            if pos < 0:
                problems.append(f"{label} ({tok}) not found in boot()")
            elif pos < gate:
                problems.append(f"{label} ({tok}) runs before the version gate")
    dllmain = fn_body(lib_src, "DllMain")
    if dllmain is None:
        problems.append("lib.rs has no DllMain")
    else:
        calls = set(re.findall(r"\b([a-z_][a-z0-9_]*)::[a-z_]+\(", dllmain)) - {"std", "thread", "panic", "Builder"}
        if calls - {"boot"}:
            problems.append(f"DllMain calls into {sorted(calls - {'boot'})} (it may only spawn boot::run)")
        if "boot::run" not in dllmain:
            problems.append("DllMain does not spawn boot::run")
    return problems


def check_bootorder(self_test=False):
    try:
        boot_text = (MOD_SRC / "boot.rs").read_text(encoding="utf-8")
        lib_text = (MOD_SRC / "lib.rs").read_text(encoding="utf-8")
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = bootorder_problems(boot_text, lib_text)
    if self_test:
        # The check must be able to fail: each mutation of the real sources has to be caught.
        gate_arm = re.search(r"Err\(e\) => \{\s*log_e!\(\"VERSION\"[^\n]*\n\s*return;", boot_text)
        mutations = {
            "hudhook moved before the gate": boot_text.replace(
                "    // 3. Version gate", "    let _ = hud::apply(hmodule_raw);\n    // 3. Version gate", 1),
            "gate call removed": boot_text.replace("version_gate::check()", "Ok::<(), String>(())", 1),
            "Err arm no longer returns": (boot_text[:gate_arm.start()] + gate_arm.group(0).replace("return;", "")
                                          + boot_text[gate_arm.end():]) if gate_arm else "",
            "singleton lookup before the gate": boot_text.replace(
                "    // 3. Version gate", "    let _ = singletons::required_names();\n    // 3. Version gate", 1),
        }
        missed = [label for label, text in mutations.items()
                  if text == boot_text or not text or not bootorder_problems(text, lib_text)]
        dll_mut = lib_text.replace("boot::run(raw)", "{ hud::apply(raw); boot::run(raw) }", 1)
        if dll_mut == lib_text or not bootorder_problems(boot_text, dll_mut):
            missed.append("DllMain calling hud::apply")
        if missed:
            problems.append(f"self-test: mutations not caught: {missed}")
        else:
            print(f"self-test: {len(mutations) + 1} mutations of boot.rs/lib.rs caught")
    return report(problems, "bootorder: version_gate::check() precedes data/hudhook/input/audio/boot gate/task "
                            "in boot(), its Err arm returns, DllMain only spawns boot::run")


# ---- M3 ---------------------------------------------------------------------------------------------------

def vmwiring_problems(hud_text, vm_text):
    problems = []
    hud, vm = strip_comments(hud_text), strip_comments(vm_text)
    apply = fn_body(hud, "apply")
    if apply is None:
        problems.append("hud.rs has no fn apply(")
    else:
        i, b = apply.find("viewmodel::install("), apply.find("Hudhook::builder()")
        if i < 0 or b < 0 or i > b:
            problems.append("hud.rs apply(): viewmodel::install (PRE_IMGUI.set) does not precede Hudhook::builder()")
    inst = fn_body(vm, "install")
    if inst is None or "PRE_IMGUI.set(" not in inst:
        problems.append("viewmodel.rs install() does not call PRE_IMGUI.set(")
    cb = fn_body(vm, "on_pre_imgui")
    if cb is None:
        problems.append("viewmodel.rs has no fn on_pre_imgui(")
    else:
        body = cb[cb.index("{") + 1:].strip()
        first = body.split(";")[0]
        if "guard(" not in first:
            problems.append("the pre-ImGui callback body does not start with guard(")
    ld = fn_body(vm, "start_loader")
    if ld is None:
        problems.append("viewmodel.rs has no fn start_loader(")
    else:
        k = ld.find("spawn(move ||")
        closure = ld[k:] if k >= 0 else ""
        g, l = closure.find("guard("), closure.find("load(")
        if k < 0 or g < 0 or l < 0 or l < g:
            problems.append("the loader thread does not run load() inside guard(")
    return problems


def check_vmwiring(self_test=False):
    try:
        hud = (MOD_SRC / "hud.rs").read_text(encoding="utf-8")
        vm = (MOD_SRC / "viewmodel.rs").read_text(encoding="utf-8")
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = vmwiring_problems(hud, vm)
    if self_test:
        muts = {
            "callback without guard": (hud, vm.replace('if guard("vm.callback", || callback_body(ctx)).is_none()',
                                                        'if { callback_body(ctx); false }', 1)),
            "install after Hudhook::builder()": (hud.replace("let _ = guard(\"vm.install\", || crate::viewmodel::install(root));\n", "", 1)
                                                 .replace("            .apply()", "            .apply().inspect(|_| crate::viewmodel::install(root.clone()))", 1), vm),
            "loader without guard": (hud, vm.replace('let r = guard("vm.loader", || load(device, fmt, (width, height), num_frames, ts_freq, &dir));',
                                                      'let r = Some(load(device, fmt, (width, height), num_frames, ts_freq, &dir));', 1)),
            "PRE_IMGUI never set": (hud, vm.replace("PRE_IMGUI.set(", "PRE_IMGUI.get(", 1)),
        }
        missed = [k for k, (h, v) in muts.items() if (h, v) == (hud, vm) or not vmwiring_problems(h, v)]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of hud.rs/viewmodel.rs caught")
    return report(problems, "vmwiring: viewmodel::install (PRE_IMGUI.set) precedes Hudhook::builder() in hud.rs; "
                            "the pre-ImGui callback and the loader thread run inside guard()")


# ---- M3b -------------------------------------------------------------------------------------------------

FP_FILES = {"game": "game/mod.rs", "boot": "boot.rs", "fp": "first_person.rs", "hook": "cam_hook.rs",
            "read": "game/fp_read.rs", "task": "task.rs", "input": "input_hook.rs",
            # C6 part 18: the over-the-shoulder camera (task side; its write is cam_hook::apply_shoulder)
            "shoulder": "shoulder_cam.rs",
            # 2026-10-24 death gate ("can still shoot after death"): the other fire paths and the pure gate
            "pad": "pad.rs", "nuke": "nuke.rs", "ac130": "ac130.rs",
            "corefp": "../../mw2er-core/src/fp_logic.rs", "corepad": "../../mw2er-core/src/pad_logic.rs",
            # port 2.7.2.0: the per-build hook RVAs / prologues live in mw2er-core (relative to MOD_SRC)
            "exe": "../../mw2er-core/src/exe_build.rs"}
FP_RAW_WRITES = ["ptr::write", "write_volatile", "write_unaligned", "copy_nonoverlapping", "as_mut_ptr", "*mut u8",
                 "*mut f32", "*mut [f32"]
FP_OFFSET_ASSERTS = ["offset_of!(PlayerIns, chr_ins) == 0", "offset_of!(ChrIns, chr_flags1c5) == 0x1c5",
                     "offset_of!(ChrIns, base_transparency) == 0x24c",
                     "offset_of!(ChrIns, base_transparency_modifier) == 0x250",
                     "offset_of!(CSCam, matrix) == 0x10", "offset_of!(CSChrPhysicsModule, orientation) == 0x50"]
# Port 2.7.2.0: the hook RVAs per exe build in mw2er-core/src/exe_build.rs (OWN_2710 / OWN_2720); the camera step and
# the follow cam did not move, CAM_WALL_RECOVERY moved +0x3000 (re-found by erfps2's signature in the 2.7.2.0 file).
FP_HOOK_PROLOGUE = "const CAMERA_STEP_PROLOGUE: [u8; 10] = [0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x30];"
# erfps2's camera handling (after run 5): the follow-cam update hook and the CAM_WALL_RECOVERY global (the prologue
# bytes were read from the user's eldenring.exe files, 2.7.1.0 and 2.7.2.0).
FP_FOLLOW_PROLOGUE = "[0x48, 0x8b, 0xc4, 0x53, 0x48, 0x81, 0xec, 0xe0, 0x00, 0x00, 0x00, 0x80, 0xb9, 0x40, 0x02, 0x00];"
FP_OWN = {"OWN_2710": ["camera_step: 0x3bc5f0,", "follow_cam: 0x3b7570,", "cam_wall_recovery: 0x3b1_9c80,",
                       "camera_step_prologue: &CAMERA_STEP_PROLOGUE,", "follow_cam_prologue: &FOLLOW_CAM_PROLOGUE,"],
          "OWN_2720": ["camera_step: 0x3bc5f0,", "follow_cam: 0x3b7570,", "cam_wall_recovery: 0x3b1_cc80,",
                       "camera_step_prologue: &CAMERA_STEP_PROLOGUE,", "follow_cam_prologue: &FOLLOW_CAM_PROLOGUE,"]}


def own_block(text, name):
    i = text.find(f"pub const {name}: OwnAddrs = OwnAddrs {{")
    return text[i:text.find("};", i)] if i >= 0 else ""
FP_LOG_STRINGS = [b"field=CSCamera.pers_cam_1+0x40", b"er_rewrite=1", b"er_rewrite=0", b"enable_render(+0x1c5 b3)",
                  b"yaw probe conv=", b"off reason=", b"hide restore", b"installed rva=0x", b"cfg enabled=",
                  b"muted for this session", b"field=ChrCam.pers_cam+0x40", b"camera_auto_rotation(+0x240)",
                  b"CAM_WALL_RECOVERY", b"follow installed rva=0x", b"page_checks=", b"wall_recovery=",
                  b"chr_cam_writes="]


def fpwiring_problems(src, modlog):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    every = {}
    for path in MOD_SRC.rglob("*.rs"):
        rel = path.relative_to(MOD_SRC).as_posix()
        key = next((k for k, v in FP_FILES.items() if v == rel), None)
        every[rel] = code[key] if key else strip_comments(path.read_text(encoding="utf-8"))
    users = sorted(rel for rel, text in every.items() if "WriteProcessMemory" in text)
    if users:
        problems.append(f"WriteProcessMemory used in {users} (it queries the region on every call: ~35 ms on ER's heap, run 5)")
    users = sorted(rel for rel, text in every.items() if "NtWriteVirtualMemory" in text)
    if users != ["game/mod.rs"]:
        problems.append(f"NtWriteVirtualMemory used in {users} (only game/mod.rs nt_copy() may)")
    cbody = fn_body(code["game"], "nt_copy")
    if cbody is None or "NtWriteVirtualMemory(" not in cbody:
        problems.append("game/mod.rs nt_copy() does not use NtWriteVirtualMemory")
    spans = []
    for name in ("write<T: Copy>", "write_exe_bool", "nt_copy", "refusals_hold"):
        b = fn_body(code["game"], name)
        if b:
            i = code["game"].find(b)
            spans.append((i, i + len(b)))
    stray = [m.start() for m in re.finditer(r"\bnt_copy\(", code["game"]) if not any(a <= m.start() < e for a, e in spans)]
    if stray:
        problems.append("nt_copy() called outside write() / write_exe_bool()")
    wbody = fn_body(code["game"], "write<T: Copy>")
    if wbody is None:
        problems.append("game/mod.rs has no fn write<T: Copy>(")
    else:
        g, w = wbody.find("WRITES_ALLOWED"), wbody.find("nt_copy(")
        if g < 0 or w < 0 or g > w or "return false" not in wbody[g:w]:
            problems.append("write() does not refuse (return false) on !WRITES_ALLOWED before the copy")
        q = wbody.find("heap_rw(")
        if q < 0 or q > w or "return false" not in wbody[q:w] or "PAGE_CACHE.missing(" not in wbody[:q]:
            problems.append("write() does not refuse non-heap pages (cached heap_rw / VirtualQuery per page) before the copy")
    ebody = fn_body(code["game"], "write_exe_bool")
    if ebody is None:
        problems.append("game/mod.rs has no fn write_exe_bool(")
    else:
        g, a, w = ebody.find("WRITES_ALLOWED"), ebody.find("EXE_BOOL_ADDR.load("), ebody.find("nt_copy(")
        if min(g, a, w) < 0 or not (g < a < w) or "v > 1" not in ebody[a:w] or ebody[g:w].count("return false") < 2:
            problems.append("write_exe_bool() does not refuse a closed gate / any address but the validated one / non-bool values before the copy")
    vbody = fn_body(code["game"], "validate_exe_data_bool")
    if vbody is None or 'name != ".data"' not in src["game"] or not all(t in vbody for t in ("IMAGE_SCN_MEM_WRITE", "IMAGE_SCN_MEM_EXECUTE", "MEM_IMAGE",
                                                    "AllocationBase", "PAGE_WRITECOPY", "v > 1", "EXE_BOOL_ADDR.store(")):
        problems.append("validate_exe_data_bool() does not check .data / write / no-execute / MEM_IMAGE of the exe / protection / bool value")
    users = sorted(rel for rel, text in every.items() if "write_exe_bool(" in text and rel != "game/mod.rs")
    if users != ["cam_hook.rs"]:
        problems.append(f"write_exe_bool used in {users} (only cam_hook.rs may)")
    hbody = fn_body(code["game"], "heap_rw")
    if hbody is None or not all(t in hbody for t in ("VirtualQuery(", "MEM_COMMIT", "MEM_IMAGE", "PAGE_READWRITE")):
        problems.append("game/mod.rs heap_rw() does not check VirtualQuery commit/image/PAGE_READWRITE")
    abody = fn_body(code["hook"], "apply")
    if abody is None or "read_pers_cam1_ptr(" not in abody or "fresh != cam" not in abody:
        problems.append("cam_hook apply() does not re-read pers_cam_1 and compare it with the published address")
    # unit-test modules (#[cfg(test)], e.g. game/mod.rs's write-cost regression test) may open the gate
    setters = sorted(rel for rel, text in every.items() if "WRITES_ALLOWED.store(true" in text.split("#[cfg(test)]")[0])
    if setters != ["boot.rs"]:
        problems.append(f"WRITES_ALLOWED.store(true ...) in {setters} (only boot.rs may)")
    body = fn_body(code["boot"], "boot")
    if body is None:
        problems.append("boot.rs has no fn boot(")
    else:
        gate, st = body.find("version_gate::check()"), body.find("WRITES_ALLOWED.store(true")
        err = body.find("Err(", gate) if gate >= 0 else -1
        if gate < 0 or st < 0 or not gate < st < err:
            problems.append("WRITES_ALLOWED is not set inside the Ok arm of version_gate::check() in boot()")
        inst = body.find("cam_hook::install")
        cond = body.rfind("if ", 0, inst) if inst >= 0 else -1
        if inst < 0:
            problems.append("boot() never installs the camera-step hook (cam_hook::install)")
        elif inst < gate or cond < 0 or "CameraMode::Hook" not in body[cond:inst] or "first_person.enabled" not in body[cond:inst]:
            problems.append("cam_hook::install is not guarded by `if ...first_person.enabled && camera_mode == ...CameraMode::Hook` after the gate")
        fol = body.find("cam_hook::install_follow")
        if fol < 0 or fol < inst or "Some(Ok(" not in body[inst:fol]:
            problems.append("boot() does not install the follow-cam hook only after the camera-step hook succeeded")
    for k in ("fp", "read", "hook"):
        bad = [t for t in FP_RAW_WRITES if t in code[k]]
        if bad:
            problems.append(f"{FP_FILES[k]} writes game memory without game::write: {bad}")
    if FP_HOOK_PROLOGUE not in src["exe"] or FP_FOLLOW_PROLOGUE not in src["exe"]:
        problems.append("exe_build.rs: the camera-step / follow-cam prologue differs from the documented "
                        "48 89 5c 24 08 57 48 83 ec 30 / 48 8b c4 53 48 81 ec e0 00 00 00 80 b9 40 02 00")
    for blk, pins in FP_OWN.items():
        b = own_block(code["exe"], blk)
        miss = [p for p in pins if p not in b]
        if miss:
            problems.append(f"exe_build.rs {blk}: the hook RVAs differ from the documented ones: missing {miss}")
    ibody = fn_body(code["hook"], "install")
    if ibody is None or "version_gate::own()?" not in ibody or not (0 <= ibody.find("bytes != camera_step_prologue") < ibody.find("hook_at(")):
        problems.append("cam_hook::install() does not take the accepted build's address and check its prologue before hooking")
    hbody2 = fn_body(code["hook"], "hook_at")
    mh = hbody2.find("MhHook::new") if hbody2 else -1
    if hbody2 is None or not (0 <= hbody2.find("!= *prologue") < mh) or "return Err(" not in hbody2[:mh]:
        problems.append("cam_hook::hook_at() does not refuse a prologue mismatch before MhHook::new")
    fbody = fn_body(code["hook"], "install_follow")
    if fbody is None or "hook_at(target, own.follow_cam_prologue" not in fbody or "validate_exe_data_bool(own.cam_wall_recovery)" not in fbody:
        problems.append("cam_hook::install_follow() does not hook through hook_at with the build's follow_cam_prologue / validate the build's wall RVA")
    pbody = fn_body(code["hook"], "follow_pre")
    if pbody is None or not all(t in pbody for t in ("armed()", "read_chr_cam_ptr(", "read_follow_cam_ptr(chr) } != fc", "CHR_CAM_ADDR")):
        problems.append("cam_hook follow_pre() does not check armed + re-read WorldChrMan.chr_cam -> ex_follow_cam == this")
    dbody = fn_body(code["hook"], "follow_detour")
    if dbody is None or "catch_unwind(" not in dbody:
        problems.append("cam_hook follow_detour() is not panic-safe (catch_unwind)")
    if abody is None or "read_chr_cam_ptr(" not in abody or "fresh_chr == chr" not in abody:
        problems.append("cam_hook apply() does not re-read WorldChrMan.chr_cam before writing ChrCam.pers_cam")
    hb = fn_body(code["hook"], "handback")
    if hb is None or not (0 <= hb.find("ACTIVE.store(false") < hb.find("IN_DETOUR")) or "hook_row_restore(" not in hb or hb.count(".restore(") < 2:
        problems.append("cam_hook handback() does not stop the detours, wait, and restore ChrCam row 3 / auto rotation / wall recovery")
    if "cam_hook::handback()" not in code["fp"]:
        problems.append("first_person.rs never calls cam_hook::handback() (the erfps2-style fields would not be restored)")
    if not all(t in src["hook"] for t in ("Dasaav-dsv/erfps2", "57e92c1378e87670f011cdee4a715e7287995dac", "MIT OR Apache-2.0")):
        problems.append("cam_hook.rs lacks the erfps2 attribution (repo, commit, licence)")
    if "erfps2" not in modlog or "MIT" not in modlog:
        problems.append("MODLOG.md lacks the erfps2 attribution")
    missing = [a for a in FP_OFFSET_ASSERTS if f"const _: () = assert!({a});" not in src["read"]]
    if missing:
        problems.append(f"game/fp_read.rs lacks offset asserts: {missing}")
    if "fp.emergency" not in src["task"] or "emergency_off" not in code["task"]:
        problems.append("task.rs: the panic path does not restore first person (guard fp.emergency -> emergency_off)")
    problems += shoulder_problems(code)
    problems += deathfire_problems(code)
    return problems


def deathfire_problems(code):
    """2026-10-24 (user: "can still shoot after death"; host_c6_2026-10-24.log 13:27:52-13:30:44: 40 SHOT lines while
    dead in third person): the task's fp_logic::DeathGate is updated from this tick's player read before any fire input
    and gates hitscan / launchers (fire_raw, can_fire, the sim hold), ADS, reload, the AC-130 call, the nuke key and the
    pad / keyboard melee and heal pulses; a tick without a read never re-arms it."""
    problems = []
    tk = code["task"]
    upd, fr = tk.find("self.death_gate.update(now, death_read.map("), tk.find("let fire_raw = ")
    if not (0 <= upd < fr) or tk.find("let dead_block = self.death_gate.blocked();") < upd:
        problems.append("death: task.rs does not update the death gate from the player read before the fire inputs")
    need = ["let fire_raw = blank && fire_in && !ac.ump_block && !nk.fire_block && !dead_block;",
            "let ads_raw = blank && ads_in && !ac.ump_block && !nk.fire_block && !dead_block;",
            "self.world_ticks >= FIRE_WARMUP_TICKS && !dead_block;", "let reloads = if dead_block { 0 } else { reloads };",
            "sim.set_hold(switching || self.mv.fire_blocked || gate_block || dead_block);",
            "shared::WANT_DEAD_BLOCK.store(dead_block, Ordering::SeqCst);"]
    miss = [n for n in need if n not in tk]
    if miss:
        problems.append(f"death: task.rs fire / ADS / reload / burst / pulse paths not gated on the death gate: {miss}")
    if tk.count("player_dead: dead_block,") != 2:
        problems.append("death: task.rs does not hand the death gate to both the AC-130 and the nuke")
    if "dead: i.player_dead || p.is_some_and(|p| fp_logic::dead(" not in (fn_body(code["ac130"], "gate") or ""):
        problems.append("death: ac130.rs gate() ignores the task's death gate")
    if "dead: !alive || i.player_dead," not in code["nuke"]:
        problems.append("death: nuke.rs key gate ignores the task's death gate")
    if "dead: effective_want(crate::shared::WANT_DEAD_BLOCK.load(Ordering::Relaxed), now, hb)," not in code["pad"]:
        problems.append("death: pad.rs does not pass the death gate to the pad step (melee / heal pulses)")
    if "heal_key_step(&mut heal_t0, heal_edge, heal_ok && !heal_dead, now)" not in code["input"]:
        problems.append("death: input_hook.rs heal key pulses while dead")
    st = fn_body(code["corepad"], "step") or ""
    if st.count("&& !i.dead {") != 2 or "if i.melee_off || i.dead {" not in st:
        problems.append("death: pad_logic step() pulses melee / heal while dead (or keeps a running pulse)")
    up = fn_body(code["corefp"], "update") or ""
    if "Some(true) | None => self.alive_since = None," not in up or "now - t0 >= DEATH_REARM_S" not in up:
        problems.append("death: fp_logic DeathGate::update() re-arms without DEATH_REARM_S of gap-free alive reads")
    return problems


def shoulder_problems(code):
    """C6 part 18 (docs/C6_HOLD_DESIGN.md 18.6): the over-the-shoulder camera writes pers_cam_1 row 3 only, through
    game::write in the camera-step detour after first person and before the view kick, never while first person (the
    scope peek) or the AC-130 owns the camera, re-reads its camera, touches no ChrCam / follow cam / exe bool; the task
    side writes nothing, runs after first person and before the shots, and the panic path disarms it."""
    problems = []
    sb = fn_body(code["hook"], "apply_shoulder")
    if sb is None:
        return ["shoulder: cam_hook.rs has no fn apply_shoulder("]
    w = [m.start() for m in re.finditer(r"\bwrite\(", sb)]
    g = sb.find("ACTIVE.load(Ordering::SeqCst) || MODE.load(Ordering::SeqCst) == MODE_GUNSHIP")
    if len(w) != 1 or g < 0 or g > w[0] or "return;" not in sb[g:w[0]]:
        problems.append("shoulder: apply_shoulder() is not ONE game::write refused (return) while first person (ACTIVE) or the gunship owns the camera")
    if "read_pers_cam1_ptr(" not in sb or "fresh != cam" not in sb or not w or "CAM_ROW3" not in sb[w[0]:w[0] + 60]:
        problems.append("shoulder: apply_shoulder() does not re-read pers_cam_1 (== the published camera) and write row 3 only")
    if any(t in sb for t in ("read_chr_cam_ptr(", "CHR_CAM_ROW3", "FOLLOW_CAM_AUTO_ROTATION", "write_exe_bool(", "CAM_ROWS, ")):
        problems.append("shoulder: apply_shoulder() touches ChrCam / the follow cam / the exe bool / rows 0-2")
    det = fn_body(code["hook"], "detour")
    a, b, c = (det.find(t) if det else -1 for t in ("AssertUnwindSafe(apply))", "AssertUnwindSafe(apply_shoulder))", "AssertUnwindSafe(apply_view_fx))"))
    if not (0 <= a < b < c):
        problems.append("shoulder: the detour does not run apply_shoulder under catch_unwind after first person and before the view kick")
    if re.search(r"\bwrite(_exe_bool)?\(", code["shoulder"]) or any(t in code["shoulder"] for t in FP_RAW_WRITES):
        problems.append("shoulder: shoulder_cam.rs writes game memory (the write belongs to cam_hook::apply_shoulder)")
    tk = code["task"]
    fp, sh, shot = tk.find("self.fp_eye = self.fp.tick(&TickIn {"), tk.find("self.shoulder.tick(&"), tk.find("bullet_origin(")
    if not (0 <= fp < sh < shot):
        problems.append("shoulder: task.rs does not tick the shoulder camera after first person and before the shots")
    if "st.shoulder.emergency_off()" not in tk:
        problems.append("shoulder: the task's panic path does not disarm the shoulder camera")
    return problems


def check_fpwiring(self_test=False):
    try:
        src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in FP_FILES.items()}
        modlog = (ROOT / "MODLOG.md").read_text(encoding="utf-8")
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = fpwiring_problems(src, modlog)
    if self_test:
        def mut(key, old, new):
            m = dict(src)
            m[key] = src[key].replace(old, new, 1)
            return m
        muts = {
            "WriteProcessMemory in first_person.rs": mut("fp", "use std::collections::HashSet;",
                                                         "use std::collections::HashSet;\nfn x() { WriteProcessMemory(); }"),
            "write() without the WRITES_ALLOWED check": mut("game", "if addr == 0 || !crate::shared::WRITES_ALLOWED",
                                                             "if addr == 0 || !true"),
            "WRITES_ALLOWED set before the gate": mut("boot", "    // 3. Version gate",
                                                       "    shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);\n    // 3. Version gate"),
            "WRITES_ALLOWED set by the task": mut("task", "fn run_tick(st: &mut TaskState) {",
                                                  "fn run_tick(st: &mut TaskState) {\n    shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);"),
            "raw pointer write in first_person.rs": mut("fp", "use std::collections::HashSet;",
                                                         "use std::collections::HashSet;\nfn y(p: *mut f32) { unsafe { std::ptr::write(p, 1.0) } }"),
            "hook RVA changed": mut("exe", "camera_step: 0x3bc5f0,", "camera_step: 0x3bc600,"),
            "2.7.2.0 hook RVA changed": {**src, "exe": src["exe"].replace("camera_step: 0x3bc5f0,", "camera_step: 0x3bc600,")},
            "prologue check removed": mut("hook", "if bytes != camera_step_prologue {", "if false {"),
            "install without the accepted build": mut("hook", "let own = crate::version_gate::own()?;", "let own = &mw2er_core::exe_build::OWN_2710;"),
            "hook installed unconditionally": mut("boot", "if cfg.first_person.enabled && cfg.first_person.camera_mode == mw2er_core::config::CameraMode::Hook || cfg.ac130.enabled {", "{"),
            "offset assert removed": mut("read", "const _: () = assert!(offset_of!(ChrIns, chr_flags1c5) == 0x1c5);", ""),
            "attribution removed": mut("hook", "Dasaav-dsv/erfps2", "someone"),
            "panic path without restore": {**src, "task": src["task"].replace("emergency_off", "noop_off")},
            "write() without the page check": mut("game", "let verdict = heap_rw(page, PAGE);",
                                                  "let verdict = Ok::<(), u8>(());"),
            "WriteProcessMemory back in game/mod.rs": mut("game", "use std::time::Instant;",
                                                           "use std::time::Instant;\nfn z() { WriteProcessMemory(); }"),
            "NtWriteVirtualMemory in cam_hook.rs": mut("hook", "use std::ffi::c_void;",
                                                        "use std::ffi::c_void;\nfn z() { NtWriteVirtualMemory(); }"),
            "exe bool path accepts any address": mut("game", "addr != EXE_BOOL_ADDR.load(Ordering::SeqCst) || ", ""),
            "follow hook RVA changed": mut("exe", "follow_cam: 0x3b7570,", "follow_cam: 0x3b7580,"),
            "wall RVA changed": mut("exe", "cam_wall_recovery: 0x3b1_9c80,", "cam_wall_recovery: 0x3b1_9c88,"),
            "2.7.2.0 wall RVA changed": mut("exe", "cam_wall_recovery: 0x3b1_cc80,", "cam_wall_recovery: 0x3b1_9c80,"),
            "follow hook with the 2.7.1.0 wall RVA": mut("hook", "validate_exe_data_bool(own.cam_wall_recovery)",
                                                         "validate_exe_data_bool(0x3b19c80)"),
            "follow prologue check removed": mut("hook", "if bytes[..prologue.len()] != *prologue {", "if false {"),
            "hooks installed without first_person.enabled": mut("boot", "if cfg.first_person.enabled && cfg.first_person.camera_mode",
                                                                  "if cfg.first_person.camera_mode"),
            "follow detour trusts its argument": mut("hook", " || unsafe { read_follow_cam_ptr(chr) } != fc {", " {"),
            "follow detour not panic-safe": mut("hook", "let _ = catch_unwind(AssertUnwindSafe(|| follow_pre(fc)));", "follow_pre(fc);"),
            "chr cam written without re-read": mut("hook", "chr != 0 && fresh_chr == chr", "chr != 0"),
            "handback never called": {**src, "fp": src["fp"].replace("cam_hook::handback()", "cam_hook::nothing()")},
            "WRITES_ALLOWED opened outside a test module": mut("game", "pub(crate) fn flush_write_cache() {",
                                                                "pub(crate) fn flush_write_cache() {\n    crate::shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);"),
            "page check without PAGE_READWRITE": mut("game", "if mbi.Protect != PAGE_READWRITE {", "if false {"),
            "hook trusts the published camera": mut("hook", "if cam == 0 || fresh != cam {", "if cam == 0 {"),
            # C6 part 18: the shoulder camera
            "shoulder written in first person": mut("hook", "if ACTIVE.load(Ordering::SeqCst) || MODE.load(Ordering::SeqCst) == MODE_GUNSHIP {", "if false {"),
            "shoulder trusts the published camera": mut("hook", "if cam == 0 || fresh != cam {\n        SH_SKIPS", "if cam == 0 {\n        SH_SKIPS"),
            "shoulder after the view kick": mut("hook", "let _ = catch_unwind(AssertUnwindSafe(apply_shoulder));\n        let _ = catch_unwind(AssertUnwindSafe(apply_view_fx));",
                                                "let _ = catch_unwind(AssertUnwindSafe(apply_view_fx));\n        let _ = catch_unwind(AssertUnwindSafe(apply_shoulder));"),
            "shoulder writes the chr cam": mut("hook", "let pos = V3::new(m[3][0], m[3][1], m[3][2]);",
                                               "let pos = V3::new(m[3][0], m[3][1], m[3][2]);\n    let _ = unsafe { read_chr_cam_ptr(0) };"),
            "shoulder writes rows 0-2": mut("hook", "if unsafe { write(cam + CAM_ROW3, [new.x, new.y, new.z, m[3][3]]) } {",
                                            "if unsafe { write(cam + CAM_ROWS, [new.x, new.y, new.z, m[3][3]]) } {"),
            "shoulder_cam.rs writes memory": mut("shoulder", "use crate::cam_hook;", "use crate::cam_hook;\nfn z() { let _ = unsafe { crate::game::write(0, 0u8) }; }"),
            "shoulder never ticked": mut("task", "self.shoulder.tick(&", "self.shoulder.nothing(&"),
            "panic path without the shoulder": mut("task", "st.shoulder.emergency_off()", "st.shoulder.noop()"),
            # 2026-10-24: the death gate
            "death: fire not gated": mut("task", "&& !nk.fire_block && !dead_block;\n        let ads_raw", "&& !nk.fire_block;\n        let ads_raw"),
            "death: ADS not gated": mut("task", "ads_in && !ac.ump_block && !nk.fire_block && !dead_block;", "ads_in && !ac.ump_block && !nk.fire_block;"),
            "death: burst not cancelled": mut("task", "self.world_ticks >= FIRE_WARMUP_TICKS && !dead_block;", "self.world_ticks >= FIRE_WARMUP_TICKS;"),
            "death: gate never updated": mut("task", "self.death_gate.update(now, death_read.map(", "self.death_gate.nothing(now, death_read.map("),
            "death: AC-130 without the gate": mut("ac130", "dead: i.player_dead || p.is_some_and(", "dead: p.is_some_and("),
            "death: nuke key without the gate": mut("nuke", "dead: !alive || i.player_dead,", "dead: !alive,"),
            "death: pad pulses while dead": mut("pad", "dead: effective_want(crate::shared::WANT_DEAD_BLOCK.load(Ordering::Relaxed), now, hb),", "dead: false,"),
            "death: heal key drinks while dead": mut("input", "heal_ok && !heal_dead, now)", "heal_ok, now)"),
            "death: pad melee while dead": mut("corepad", "b.er_melee != 0 && !i.dead {", "b.er_melee != 0 {"),
            "death: a missing read re-arms": mut("corefp", "Some(true) | None => self.alive_since = None,", "Some(true) => self.alive_since = None,\n            None => {}"),
        }
        base = set(problems)  # each mutation must add a problem of its own
        missed = [k for k, m in muts.items() if m == src or set(fpwiring_problems(m, modlog)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the M3b sources caught")
    return report(problems, "fpwiring: NtWriteVirtualMemory only in game::nt_copy via write() (gate, cached page check) "
                            "and write_exe_bool() (gate, one validated .data address); no WriteProcessMemory; "
                            "WRITES_ALLOWED set only in boot()'s gate Ok arm; no raw pointer writes in the first-person "
                            "code; camera-step/follow-cam RVAs+prologues and the wall RVA as documented, prologues checked, "
                            "installed only for enabled && camera_mode=hook; detours re-read their objects; handback "
                            "restores; erfps2 attributed; offset asserts present; panic path restores; C6 part 18: the "
                            "shoulder camera is one game::write of pers_cam_1 row 3 in the detour (after first person, "
                            "before the view kick, refused while first person / the gunship own the camera, camera "
                            "re-read, no ChrCam / follow cam), the task side writes nothing, ticks after first person "
                            "and before the shots, and the panic path disarms it; 2026-10-24: the death gate (fp_logic::DeathGate, "
                            "updated from the player read before the fire inputs, a missing read never re-arms) gates "
                            "fire / ADS / reload / bursts / the AC-130 call / the nuke key / the pad and keyboard melee "
                            "and heal pulses")


def check_fpstrings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    missing = [x.decode() for x in FP_LOG_STRINGS if x not in data]
    return report([f"missing log strings: {missing}"] if missing else [],
                  f"fpstrings: the DLL contains all {len(FP_LOG_STRINGS)} first-person proof strings")


def newest_shader_dir():
    builds = ROOT / "mod" / "target" / "release" / "build"
    cands = [d / "out" for d in builds.glob("mw2er-render-*") if (d / "out" / "viewmodel_vs.cso").is_file()]
    return max(cands, key=lambda d: (d / "viewmodel_vs.cso").stat().st_mtime) if cands else None


def check_shaders(dll):
    out = newest_shader_dir()
    if out is None:
        return report(["no mw2er-render build output with viewmodel_vs.cso (cargo build --release first)"], "")
    problems, hashes = [], []
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    for n in ("viewmodel_vs.cso", "viewmodel_ps.cso"):
        b = (out / n).read_bytes()
        if not b.startswith(b"DXBC"):
            problems.append(f"{n} is not DXBC")
        if data.find(b) < 0:
            problems.append(f"{pathlib.Path(dll).name} does not contain the exact bytes of {n}")
        hashes.append(f"{n} {hashlib.sha256(b).hexdigest()}")
    return report(problems, f"shaders: DXBC, embedded byte-exact in {pathlib.Path(dll).name}: " + "; ".join(hashes))


MAGICS = [(b"DDS ", "DDS"), (b"glTF", "GLB"), (b"VMSK", "vmskel"), (b"VMME", "vmmesh"), (b"VMAN", "vmanim"), (b"VMTX", "vmtex"),
          (b"VMMK", "vmtest mask")]


def check_hygiene():
    problems, n = [], 0
    for top in ("tools", "mod", "docs"):
        base = ROOT / top
        for p in base.rglob("*"):
            if not p.is_file():
                continue
            rel = p.relative_to(ROOT)
            parts = rel.parts
            if top == "mod" and len(parts) > 1 and parts[1] == "target":
                continue
            if top == "tools" and len(parts) > 1 and parts[1] in ("oat", "WitchyBND"):
                continue  # third-party tool trees (their own build outputs live there, gitignored /tools/*/build/)
            n += 1
            size = p.stat().st_size
            if size > 5 * 1024 * 1024:
                problems.append(f"{rel} is {size} bytes (> 5 MB)")
                continue
            head = p.read_bytes()[:12]
            for magic, what in MAGICS:
                if head.startswith(magic):
                    problems.append(f"{rel} starts with {what} magic")
            if len(head) >= 10 and head[0] in (17, 18) and head[1] == 0 and head[8] == 30 and head[9] == 0 \
                    and p.suffix.lower() not in (".rs", ".py", ".md", ".toml", ".ps1", ".json", ".txt", ".hlsl", ".patch"):
                problems.append(f"{rel} looks like a compiled XAnim (v{head[0]}, 30 fps)")
    return report(problems, f"hygiene: {n} files under tools/ (own scripts), mod/ (no target) and docs/: no DDS/GLB/XAnim/"
                            "converted magic, none > 5 MB")


# ---- co-op friend package (dist/MW2inER-coop-friend) ---------------------------------------------------------

DIST_BINARIES = {"build/natives/mw2er.dll", "tools/oat/bin/Unlinker.exe", "tools/bin/sndconv.exe"}
DIST_BANNED_NAMES = re.compile(r"(?i)(regulation\.bin|\.sl2|\.co2|\.mw2co2|\.ff|\.iwd|\.iwi|\.dcx|\.bdt|\.bhd|\.dds|\.glb|\.wav|\.mp3|"
                               r"\.vmtex|\.vmmesh|\.vmskel|\.vmanim|\.rgba|ersc\.dll|ersc_settings\.ini|ersc_launcher\.exe)$")
DIST_BANNED_DIRS = {"converted", "dumps", "work", "backups", "coop", "SeamlessCoop", "logs", "setup_logs"}
# absolute paths of this PC that may appear only as documented defaults the friend's setup overrides
DIST_PATH_RE = re.compile(r"(?i)(C:\\Users\\Player|Player|D:\\Mods\\MW2inER(?!-coop)|D:\\SteamLibrary|Documents\\seamless)")
DIST_PATH_ALLOWED = {
    # MW2ER_GAME_REGULATION / MW2ER_IW4SP default when the env var is unset (setup.ps1 always sets it)
    "tools/scripts/build_regulation.py": {r"D:\SteamLibrary"},
    "tools/scripts/convert_movement.py": {r"D:\SteamLibrary"},
    # the compile-time source paths of the Unlinker's libtomcrypt asserts (GPL tool binary, no user name)
    "tools/oat/bin/Unlinker.exe": {r"D:\Mods\MW2inER"},
}


def check_dist(folder):
    base = pathlib.Path(folder)
    if not base.is_dir():
        return report([f"{base} is not a folder"], "")
    problems, n, notes = [], 0, []
    for p in sorted(base.rglob("*")):
        rel = p.relative_to(base).as_posix()
        if p.is_dir():
            if p.name in DIST_BANNED_DIRS:
                problems.append(f"{rel}/: a folder that must never ship ({p.name})")
            continue
        n += 1
        if DIST_BANNED_NAMES.search(p.name):
            problems.append(f"{rel}: banned file type (game / converted / save / Seamless file)")
        data = p.read_bytes()
        for magic, what in MAGICS:
            if data.startswith(magic):
                problems.append(f"{rel} starts with {what} magic")
        if data[:4] in (b"BND4", b"DCX\x00") or data[:4] == b"IWff" or data[:3] == b"IWi":
            problems.append(f"{rel}: FromSoftware / IW container magic")
        if len(data) > 5 * 1024 * 1024 and rel not in DIST_BINARIES:
            problems.append(f"{rel} is {len(data)} bytes (> 5 MB, not a known binary)")
        for text in (data.decode("latin-1"), data.decode("utf-16-le", "ignore")):
            for m in set(x.group(0) for x in DIST_PATH_RE.finditer(text)):
                allowed = DIST_PATH_ALLOWED.get(rel, set())
                if any(m.lower().startswith(a.lower()) for a in allowed):
                    notes.append(f"{rel}: {m} (documented default)")
                else:
                    problems.append(f"{rel}: contains a path of this PC ({m})")
        if re.search(rb"(?im)^\s*cooppassword\s*=\s*\S", data):
            problems.append(f"{rel}: contains a cooppassword value")
    for x in sorted(set(notes)):
        print("INFO " + x)
    return report(problems, f"dist: {n} files in {base.name}: no game / converted / save / Seamless files, no DDS/GLB/vm*/IW/BND "
                            f"magic, nothing > 5 MB but the 3 binaries, no path of this PC except {len(set(notes))} documented default(s), "
                            "no password")


# ---- M4 ----------------------------------------------------------------------------------------------------

AC_FILES = {"hook": "cam_hook.rs", "ac": "ac130.rs", "boot": "boot.rs", "task": "task.rs", "vm": "viewmodel.rs",
            "th": "thermal_present.rs", "game": "game/mod.rs", "acread": "game/ac130_read.rs", "hud": "hud.rs"}
AC_EXTRA = {"render": ROOT / "mod" / "mw2er-render" / "src" / "thermal.rs",
            "tracer": ROOT / "mod" / "mw2er-render" / "src" / "tracer.rs",
            "logic": ROOT / "mod" / "mw2er-core" / "src" / "ac130_logic.rs"}
AC_DETOUR_FORBIDDEN = ["VirtualQuery", "WriteProcessMemory", "Box::new", "Vec::", "vec!", "format!", "String", "to_string",
                       "log_i!", "log_w!", "log_e!", "lock(", "Mutex", "try_lock", "write_exe_bool", "CHR_CAM", "FOLLOW_CAM"]
AC_LOG_STRINGS = [b"on center=", b"off reason=", b"blocked reason=", b"PROTECT apply id=", b"PROTECT restore id=",
                  b"PROTECT cleanup removed=", b"field=CSFeManImp.hud_state(+0x78)", b"field=CSCamera.pers_cam_1+0x10..0x54",
                  b"SHOT gun=", b"EXPLODE gun=", b"EXPLODE seen id=", b"SWITCH gun=", b"RELOAD gun=", b"KILL gun=",
                  b"src=ac130", b"thermal ready ms=", b"thermal first frame", b"gpu_ms p50=", b"FAIL thermal reason=",
                  b"mouse_counts=", b"persisted=", b" ac130=", b"rows ok bullet=", b"AC-130 READY", b"AC130CAM",
                  b"player_hp_start=", b"tracers ready ms=", b"tracer gpu_ms p50=", b"FAIL tracers reason=",
                  b"tracers first frame instances=", b"SHAKE src=", b"effects shake=", b" sfx=", b"atkObj="]
AC_FORBIDDEN_STRINGS = [b"symphonia"]


def ac_sources():
    src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in AC_FILES.items()}
    src.update({k: p.read_text(encoding="utf-8") for k, p in AC_EXTRA.items()})
    return src


def enum_variants(src, name):
    m = re.search(r"pub enum " + name + r"\s*\{(.*?)\n\}", src, re.S)
    if not m:
        return []
    body = "\n".join(re.sub(r"//.*$", "", l) for l in m.group(1).splitlines())
    return re.findall(r"^\s*([A-Z]\w*)\s*,", body, re.M)


def symphonia_users():
    """(Cargo.toml files declaring symphonia, .rs files naming it outside mod/sndconv, lock packages depending on it)."""
    mod = ROOT / "mod"
    tomls, rs = [], []
    for p in mod.rglob("*"):
        rel = p.relative_to(mod).as_posix()
        if rel.startswith("target/") or not p.is_file():
            continue
        if p.name == "Cargo.toml":
            code = "\n".join(re.sub(r"#.*$", "", l) for l in p.read_text(encoding="utf-8").splitlines())
            if "symphonia" in code:
                tomls.append(rel)
        elif p.suffix == ".rs" and not rel.startswith("sndconv/") and "symphonia" in strip_comments(p.read_text(encoding="utf-8")):
            rs.append(rel)
    deps = []
    lock = mod / "Cargo.lock"
    if lock.is_file():
        for pkg in tomllib.loads(lock.read_text(encoding="utf-8")).get("package", []):
            if not pkg["name"].startswith("symphonia") and any(d.split(" ")[0].startswith("symphonia") for d in pkg.get("dependencies", [])):
                deps.append(pkg["name"])
    return sorted(tomls), sorted(rs), sorted(deps)


def ac130wiring_problems(src, modlog, sym=None):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    # 1. the mode-2 detour body: exactly one game::write, nothing that allocates, logs, locks or queries
    g = fn_body(code["hook"], "apply_gunship")
    if g is None:
        problems.append("1: cam_hook.rs has no fn apply_gunship(")
    else:
        n = len(re.findall(r"(?<![\w.])write\(", g))
        bad = [t for t in AC_DETOUR_FORBIDDEN if t in g]
        if n != 1 or bad:
            problems.append(f"1: apply_gunship() has {n} game::write call(s) (want 1) and forbidden {bad}")
    d = fn_body(code["hook"], "detour")
    if d is None or "catch_unwind(AssertUnwindSafe(apply_gunship))" not in d:
        problems.append("1: the camera-step detour does not run apply_gunship under catch_unwind")
    # 2. WriteProcessMemory absent, VirtualQuery only in game/mod.rs heap_rw (and the exe .data validation)
    every = {}
    for path in MOD_SRC.rglob("*.rs"):
        rel = path.relative_to(MOD_SRC).as_posix()
        key = next((k for k, v in AC_FILES.items() if v == rel), None)
        every[rel] = code[key] if key else strip_comments(path.read_text(encoding="utf-8"))
    wpm = sorted(r for r, t in every.items() if "WriteProcessMemory" in t)
    vq = sorted(r for r, t in every.items() if re.search(r"\bVirtualQuery\(", t))
    if wpm or vq != ["game/mod.rs"]:
        problems.append(f"2: WriteProcessMemory in {wpm}; VirtualQuery( in {vq} (only game/mod.rs may)")
    else:
        spans = [b for b in (fn_body(code["game"], "heap_rw"), fn_body(code["game"], "validate_exe_data_bool")) if b]
        outside = code["game"]
        for b in spans:
            outside = outside.replace(b, "")
        if re.search(r"\bVirtualQuery\(", outside):
            problems.append("2: VirtualQuery( used in game/mod.rs outside heap_rw() / validate_exe_data_bool()")
    # 3. the SpEffect engine calls: only ac130.rs, behind WRITES_ALLOWED
    users = sorted(r for r, t in every.items() if re.search(r"\.(apply|remove)_speffect\(", t))
    if users != ["ac130.rs"]:
        problems.append(f"3: apply_speffect/remove_speffect called in {users} (only ac130.rs may)")
    for name in ("apply_speffect", "remove_speffect"):
        b = fn_body(code["ac"], name)
        call = b.find(f"c.{name}(") if b else -1
        gate = b.find("WRITES_ALLOWED") if b else -1
        if b is None or call < 0 or gate < 0 or gate > call or "return false" not in b[gate:call]:
            problems.append(f"3: ac130.rs {name}() does not refuse (return false) on !WRITES_ALLOWED before the engine call")
    # 4. every exit reason reaches end()
    reasons = enum_variants(src["logic"], "Reason")
    only = fn_body(code["logic"], "only")
    first = fn_body(code["logic"], "first")
    if not reasons or only is None or first is None:
        problems.append("4: ac130_logic.rs Reason / ExitIn::only / ExitIn::first not found")
    else:
        if "_ =>" in only or [r for r in reasons if f"Reason::{r} =>" not in only]:
            problems.append("4: ExitIn::only is not one exhaustive match over Reason (a `_ =>` arm or a missing variant)")
        missing = [r for r in reasons if r != "Panic" and f"Reason::{r})" not in first]
        if missing:
            problems.append(f"4: ExitIn::first never yields {missing}")
    emer = fn_body(code["ac"], "emergency_off") or ""
    e0 = code["ac"].find(emer) if emer else -1
    pcs = fn_body(code["ac"], "panic_cleanup_step") or ""
    if not emer or "force_end(Reason::Panic" not in emer or "restore_er_frame(" not in emer \
            or "panic_cleanup_step(" not in emer or "remove_speffect(" not in pcs:
        problems.append("4: ac130.rs emergency_off() (the panic path's minimal end) does not end the ride and restore")
    # review M4 major 2: after a panic the guarded cleanup keeps running (run_tick, GAMEPLAY_DISABLED branch)
    rt = fn_body(src["task"], "run_tick") or ""  # raw source: the guard name is a string literal
    gd = rt.find("GAMEPLAY_DISABLED")
    ret = rt.find("return", gd) if gd >= 0 else -1
    if gd < 0 or ret < 0 or "guard(\"ac130.disabled\"" not in rt[gd:ret] or "st.ac130.disabled_tick()" not in rt[gd:ret]:
        problems.append("4: run_tick does not keep the guarded ac130 disabled_tick running while gameplay is disabled")
    dt = fn_body(code["ac"], "disabled_tick") or ""
    if "panic_cleanup_step(" not in dt or "hud_pending_tick(" not in dt:
        problems.append("4: Ac130Ctl::disabled_tick does not run the protection cleanup and the deferred HUD restore")
    for m in re.finditer(r"force_end\(", code["ac"]):
        if e0 <= m.start() < e0 + len(emer):
            continue  # the panic path ends the ride itself (no end(): it must not run the full sequence twice)
        if "self.end(plan" not in code["ac"][m.start():m.start() + 400]:
            problems.append("4: a machine.force_end(...) result does not reach self.end(plan, ...)")
            break
    ev = code["ac"].find("Some(Event::Ended(plan)) =>")
    if ev < 0 or "self.end(plan" not in code["ac"][ev:ev + 120]:
        problems.append("4: Event::Ended(plan) from machine.tick does not reach self.end(plan, ...)")
    if "ac130.emergency" not in src["task"] or "emergency_off" not in code["task"]:
        problems.append("4: task.rs: the panic path does not end the ride (guard ac130.emergency -> emergency_off)")
    # 5. ThermalPass::record: nothing fallible between the two ResourceBarrier calls
    r = fn_body(code["render"], "record")
    i1 = r.find("ResourceBarrier(") if r else -1
    i2 = r.find("ResourceBarrier(", i1 + 1) if i1 >= 0 else -1
    if i1 < 0 or i2 < 0 or re.search(r"\?|\breturn\b", r[i1:i2]):
        problems.append("5: ThermalPass::record has a `?` / return between the first and the second ResourceBarrier")
    # 6. PRE_IMGUI set once (viewmodel.rs); the thermal path runs only under the snapshot gate
    sets = sorted(r for r, t in every.items() if "PRE_IMGUI.set(" in t)
    if sets != ["viewmodel.rs"] or code["vm"].count("PRE_IMGUI.set(") != 1:
        problems.append(f"6: PRE_IMGUI.set( in {sets} (once, in viewmodel.rs)")
    cb = fn_body(code["vm"], "callback_body")
    if cb is None or not cb.lstrip().split("{", 1)[1].lstrip().startswith("match guard(") or "thermal_present::dispatch(ctx)" not in cb:
        problems.append("6: viewmodel callback_body does not start with guard(thermal_present::dispatch)")
    dp = fn_body(code["th"], "dispatch")
    recs = [i for i in ((dp.find(".record("), dp.find("record_tracers(")) if dp else ()) if i >= 0]
    rec = min(recs) if recs else -1
    if dp is None or rec < 0 or not all(0 <= dp.find(t) < rec for t in ("if !a.active", "SNAPSHOT_MAX_AGE_S", "!a.in_world", "a.menu_open")):
        problems.append("6: thermal_present::dispatch records without the snapshot gate (active, age, in_world, menu) first")
    # 7. the hook install condition includes [ac130] enabled
    body = fn_body(code["boot"], "boot")
    inst = body.find("cam_hook::install") if body else -1
    cond = body.rfind("if ", 0, inst) if inst >= 0 else -1
    if inst < 0 or cond < 0 or "cfg.ac130.enabled" not in body[cond:inst]:
        problems.append("7: boot() does not install the camera-step hook for [ac130] enabled")
    # 8. ChrCam / follow-cam writes unreachable in mode 2 (sync_chr_cam is not built)
    if d is None or not re.search(r"MODE_GUNSHIP\s*\{\s*if catch_unwind\(AssertUnwindSafe\(apply_gunship\)\)", d) \
            or "else" not in d[d.find("apply_gunship"):]:
        problems.append("8: the detour does not dispatch mode 2 to apply_gunship only (else: mode 1 apply)")
    fp = fn_body(code["hook"], "follow_pre")
    if fp is None or not 0 <= fp.find("MODE_GUNSHIP") < fp.find("return") < fp.find("write("):
        problems.append("8: follow_pre does not return in mode 2 before any write")
    # 9. hud_state: game::write only; the restore is guarded by "still HideAll (0)"
    for k in ("ac", "acread"):
        bad = [t for t in FP_RAW_WRITES if t in code[k]]
        if bad:
            problems.append(f"9: {AC_FILES[k]} writes game memory without game::write: {bad}")
    restores = [m.start() for m in re.finditer(r"write\(addr, old\)", code["ac"])]
    if len(restores) < 1 or any("cur == HUD_HIDE_ALL" not in code["ac"][max(0, s - 400):s] for s in restores):
        problems.append("9: a hud_state restore (write(addr, old)) is not guarded by `cur == HUD_HIDE_ALL`")
    # 10. attributions
    if not all(t in src["hook"] for t in ("Dasaav-dsv/erfps2", "MIT OR Apache-2.0")) or "erfps2" not in modlog:
        problems.append("10: the erfps2 attribution is missing (cam_hook.rs / MODLOG.md)")
    if "fromsoftware-rs" not in src["ac"] or "fromsoftware-rs" not in modlog:
        problems.append("10: the fromsoftware-rs attribution is missing (ac130.rs engine calls / MODLOG.md)")
    # 11. symphonia only in the offline sndconv tool
    tomls, rs, deps = sym if sym is not None else symphonia_users()
    if tomls != ["sndconv/Cargo.toml"] or rs or deps not in ([], ["sndconv"]):
        problems.append(f"11: symphonia declared in {tomls}, named in {rs}, depended on by {deps} (only mod/sndconv)")
    # 13. run-8 tracers: recorded BEFORE the thermal pass (the film sees them), without per-frame allocation: the snapshot
    # fields are copied (no clone of the snapshot), the instances go into the fixed scratch array, the pass's record has
    # no barrier (the back buffer stays in RENDER_TARGET)
    tr, th_rec = (dp.find("record_tracers(") if dp else -1), (dp.find("pass.record(") if dp else -1)
    fi, rt = fn_body(code["th"], "frame_in") or "", fn_body(code["th"], "record_tracers") or ""
    trec = fn_body(code["tracer"], "record") or ""
    alloc = [t for t in (".clone()", "Vec::", "vec!", ".collect", "to_vec(", "Box::new") if t in fi or t in rt]
    if tr < 0 or th_rec < 0 or tr > th_rec or alloc or "build_instances(" not in rt or "ResourceBarrier" in trec or not trec:
        problems.append(f"13: tracers must be recorded before the thermal pass ({tr} < {th_rec}) with no per-frame allocation "
                        f"{alloc} and no barrier in TracerPass::record")
    # 12. one ImGui background draw list per frame (run 8 PANIC: imgui-rs refuses a second live DrawListMut of the
    # same list): exactly one get_background_draw_list() call in the crate, inside Mw2Hud::draw; helpers take &dl.
    calls = {rel: len(re.findall(r"\.get_background_draw_list\(", c)) for rel, c in every.items()}
    calls = {k: v for k, v in calls.items() if v}
    fg = sum(len(re.findall(r"\.get_foreground_draw_list\(", c)) for c in every.values())
    hd = fn_body(code["hud"], "draw") or ""
    if calls != {"hud.rs": 1} or ".get_background_draw_list(" not in hd or fg > 1:
        problems.append(f"12: get_background_draw_list() calls {calls} (want exactly 1, in hud.rs fn draw), "
                        f"get_foreground_draw_list() {fg} (max 1)")
    return problems


def check_ac130wiring(self_test=False):
    try:
        src = ac_sources()
        modlog = (ROOT / "MODLOG.md").read_text(encoding="utf-8")
    except OSError as e:
        return report([f"sources: {e}"], "")
    sym = symphonia_users()
    problems = ac130wiring_problems(src, modlog, sym)
    if self_test:
        def mut(key, old, new, count=1):
            m = dict(src)
            m[key] = src[key].replace(old, new, count)
            return m
        muts = {
            "1 second write in the detour": mut("hook", "        store17(&LAST_WRITE, &out.m);",
                                                "        store17(&LAST_WRITE, &out.m);\n        let _ = unsafe { write(cam + CAM_ROWS, out.m) };"),
            "1 log in the detour": mut("hook", "        AC_WROTE.store(true, Ordering::Relaxed);",
                                       "        AC_WROTE.store(true, Ordering::Relaxed);\n        log_i!(\"X\", \"y\");"),
            "2 VirtualQuery in cam_hook.rs": mut("hook", "use std::ffi::c_void;", "use std::ffi::c_void;\nfn q() { VirtualQuery(); }"),
            "3 speffect call outside ac130.rs": mut("task", "fn run_tick(st: &mut TaskState) {",
                                                    "fn run_tick(st: &mut TaskState) {\n    c.apply_speffect(1, true);"),
            "3 speffect without the gate": mut("ac", "if !WRITES_ALLOWED.load(Ordering::SeqCst) || !crate::game::plausible(chr) {",
                                               "if !crate::game::plausible(chr) {"),
            "4 wildcard arm in ExitIn::only": mut("logic", "            Reason::Config => e.disabled = true,", "            _ => {}"),
            "4 Ended without end()": mut("ac", "Some(Event::Ended(plan)) => {\n                self.end(plan, i.in_world);",
                                         "Some(Event::Ended(plan)) => {\n                let _ = plan;"),
            "5 ? between the barriers": mut("render", "            list.CopyResource(&scene_res, t.resource);",
                                            "            list.CopyResource(&scene_res, t.resource);\n            self.pso_index(t.rtv_format)?;"),
            "6 dispatch without the menu gate": mut("th", "|| a.menu_open", ""),
            "13 snapshot cloned per frame": mut("th", "    let s = try_lock(&SNAPSHOT)?;", "    let s = try_lock(&SNAPSHOT)?.clone();"),
            "13 barrier in TracerPass::record": mut("tracer", "            list.DrawInstanced(6, instances.len() as u32, 0, 0);",
                                                    "            list.DrawInstanced(6, instances.len() as u32, 0, 0);\n            list.ResourceBarrier(&[]);"),
            "7 hook not installed for the AC-130": mut("boot", " || cfg.ac130.enabled {", " {"),
            "8 follow cam written in mode 2": mut("hook", "if MODE.load(Ordering::SeqCst) == MODE_GUNSHIP || !armed() {", "if !armed() {"),
            "9 unguarded hud restore": mut("ac", "Some((a2, cur)) if a2 == addr && cur == HUD_HIDE_ALL =>",
                                           "Some((a2, cur)) if a2 == addr =>"),
            "4 no post-panic cleanup": mut("task", "guard(\"ac130.disabled\", || st.ac130.disabled_tick())",
                                           "guard(\"ac130.disabled\", || ())"),
            "4 emergency_off without the protection cleanup": mut("ac", "        self.panic_cleanup_step(\"panic\");\n", ""),
            "10 fromsoftware-rs attribution removed": mut("ac", "fromsoftware-rs", "someone", count=-1),
            "12 second background draw list": mut("hud", "    let a = &s.ac130;\n", "    let a = &s.ac130;\n    let dl2 = ui.get_background_draw_list();\n"),
        }
        after = dict(src)
        call = "record_tracers(ctx, &a, &cfg.ac130.tracers.params(), slot, now);"
        after["th"] = src["th"].replace(call, "", 1).replace("    true\n}\n\n/// Projects", "    " + call + "\n    true\n}\n\n/// Projects", 1)
        muts["13 tracers drawn after the thermal pass"] = after
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(ac130wiring_problems(m, modlog, sym)) <= base]
        sym_mut = (["mw2er/Cargo.toml", "sndconv/Cargo.toml"], [], ["mw2er", "sndconv"])
        if set(ac130wiring_problems(src, modlog, sym_mut)) <= base:
            missed.append("11 symphonia in the DLL crate")
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts) + 1} mutations of the M4 sources caught")
    return report(problems, "ac130wiring: 13 checks (docs/M4_DESIGN.md 14.5): one game::write in the mode-2 detour, no "
                            "VirtualQuery/WriteProcessMemory outside the page cache, SpEffect calls only in ac130.rs behind "
                            "the gate, every exit reason reaches end(), no fallible step between the thermal barriers, "
                            "PRE_IMGUI once + snapshot gate, hook installed for [ac130], no ChrCam/follow-cam writes in mode "
                            "2, guarded hud_state restore, attributions, symphonia only in sndconv, one background draw list per frame, tracers before the film without per-frame allocation")


def check_ac130strings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    problems = []
    missing = [x.decode() for x in AC_LOG_STRINGS if x not in data]
    if missing:
        problems.append(f"missing AC-130 log strings: {missing}")
    found = [x.decode() for x in AC_FORBIDDEN_STRINGS if x in data.lower()]
    if found:
        problems.append(f"forbidden strings in the DLL: {found}")
    builds = ROOT / "mod" / "target" / "release" / "build"
    outs = [d / "out" for d in builds.glob("mw2er-render-*") if (d / "out" / "thermal_ps.cso").is_file()]
    hashes = []
    if not outs:
        problems.append("no mw2er-render build output with thermal_ps.cso")
    else:
        out = max(outs, key=lambda d: (d / "thermal_ps.cso").stat().st_mtime)
        for n in ("thermal_vs.cso", "thermal_ps.cso"):
            b = (out / n).read_bytes()
            if not b.startswith(b"DXBC") or data.find(b) < 0:
                problems.append(f"{pathlib.Path(dll).name} does not contain the exact DXBC bytes of {n}")
            hashes.append(f"{n} {hashlib.sha256(b).hexdigest()[:16]}")
    return report(problems, f"ac130strings: all {len(AC_LOG_STRINGS)} AC-130 proof strings present, no symphonia, thermal "
                            f"shaders embedded byte-exact ({'; '.join(hashes)})")


# ---- M5 movement wiring (docs/M5_DESIGN.md section 13.4) -------------------------------------------------------

MOVE_FILES = {"move": "movement.rs", "task": "task.rs", "hook": "cam_hook.rs", "input": "input_hook.rs",
              "read": "game/move_read.rs", "havok": "game/havok.rs"}
MOVE_CORE = {"logic": "move_logic.rs", "collide": "move_collide.rs", "sim": "move_sim.rs"}
MOVE_WRITE_CONSTS = ("PHYS_POSITION", "PHYS_PROXY_REQ", "PHYS_GRAVITY_MULT", "PHYS_GRAVITY_DISABLED", "DATA_HP")
MOVE_DETOUR_FORBIDDEN = ("format!", "log_i!", "log_w!", "log_e!", "lock(", "Vec::", "vec!", "String", "Box::new", "to_string")
# the message literals of the MOVE lines (the tag "MOVE" is its own literal in the binary)
MOVE_LOG_STRINGS = [b"MOVE", b"on pos=", b"off reason=", b"suspend reason=", b"restore field=gravity_multiplier",
                    b"restore field=gravity_disabled", b"field=CSChrPhysicsModule.position(+0x70)",
                    b"gravity_disabled(+0x1d5)", b"gravity_multiplier(+0x1cc)", b"chr_proxy_pos_update_requested(+0x91)",
                    b"sprint end why=", b"sprint start meter=", b"apex h=", b"land fall_h=", b"resync why=",
                    b"keyprobe key=", b"move_us_p95=", b"order detour_before_task=", b"er_delta p50=", b"cfg enabled=",
                    # M5 review
                    b"ground_disagree", b"fall_damage=skipped_er_ground", b"position_insane", b"ground_held_ticks="]
MOVE_FORBIDDEN_STRINGS = [b"iced", b"movebench", b"iw4dis"]


def move_sources():
    src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in MOVE_FILES.items()}
    core = ROOT / "mod" / "mw2er-core" / "src"
    src.update({k: (core / v).read_text(encoding="utf-8") for k, v in MOVE_CORE.items()})
    return src


def impl_body(src, header):
    """Text of `impl ... {` from `header` up to its matching closing brace."""
    start = src.find(header)
    if start < 0:
        return None
    i = src.index("{", start)
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[start:j + 1]
    return None


def movewiring_problems(src, mod_cargo=None, lock=None):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    # 1. the controller's fields are written only in movement.rs, only through game::write; no WriteProcessMemory
    every = {}
    for path in MOD_SRC.rglob("*.rs"):
        rel = path.relative_to(MOD_SRC).as_posix()
        key = next((k for k, v in MOVE_FILES.items() if v == rel), None)
        every[rel] = code[key] if key else strip_comments(path.read_text(encoding="utf-8"))
    wre = re.compile(r"(?<![\w.])write\(\s*[\w.]+\s*\+\s*(" + "|".join(MOVE_WRITE_CONSTS) + r")\b")
    users = sorted({rel for rel, text in every.items() if wre.search(text)})
    if users != ["movement.rs"]:
        problems.append(f"1: the controller's fields are written in {users} (only movement.rs, via game::write)")
    for rel, text in every.items():
        if "WriteProcessMemory" in text:
            problems.append(f"1: WriteProcessMemory in {rel}")
        if "VirtualQuery(" in text and rel != "game/mod.rs":
            problems.append(f"1: VirtualQuery in {rel} (only the page check in game/mod.rs)")
    if re.search(r"ptr::write|write_volatile|as \*mut f32\)|as \*mut u8\)", code["move"]):
        problems.append("1: raw pointer write in movement.rs")
    # 1b. (review M5 minor 4) the position write sits behind the finite / distance check
    dr = fn_body(code["move"], "drive")
    if dr is None or "position_write_ok(" not in dr or dr.find("position_write_ok(") > dr.find("PHYS_POSITION,"):
        problems.append("1: drive() writes the position without the position_write_ok() check before it")
    # 2. the gravity save precedes the gravity writes
    og = fn_body(code["move"], "own_gravity")
    if og is None or "self.gravity.own(" not in og or "write(" not in og or og.find("self.gravity.own(") > og.find("write("):
        problems.append("2: own_gravity() does not register the save in GravityOwner before its first write")
    # 3. every Reason reaches release(): one match over all variants, no wildcard arm
    variants = enum_variants(src["logic"], "Reason")
    rel_body = fn_body(code["move"], "release")
    if not variants:
        problems.append("3: move_logic::Reason has no variants")
    elif rel_body is None:
        problems.append("3: movement.rs has no fn release(")
    else:
        missing = [v for v in variants if f"Reason::{v} =>" not in rel_body]
        if missing or re.search(r"\b_\s*=>", rel_body):
            problems.append(f"3: release() does not match every Reason ({missing}) or has a wildcard arm")
    # 4. Havok rays only through the RayBudget
    rw = impl_body(code["move"], "impl RayWorld for HavokRays")
    n_all = len(re.findall(r"\bmap_ray_point\(", code["move"]))
    if rw is None or n_all != 1 or "map_ray_point(" not in rw:
        problems.append(f"4: movement.rs calls map_ray_point {n_all} time(s) outside the RayWorld impl")
    lib_collide = code["collide"].split("#[cfg(test)]")[0]  # the tests use the synthetic world directly
    rb = impl_body(lib_collide, "pub fn cast(&mut self, w: &mut dyn RayWorld")
    stray = [m.start() for m in re.finditer(r"\bw\.cast\(", lib_collide)]
    if rb is None or len(stray) != 1 or "w.cast(" not in rb or "self.used += 1" not in rb:
        problems.append("4: move_collide.rs casts rays outside RayBudget::cast (or the budget does not count)")
    if re.search(r"\b(w|world)\.cast\(", code["sim"].split("#[cfg(test)]")[0]):
        problems.append("4: move_sim.rs casts rays directly (must go through RayBudget::cast)")
    # 4b. (review M5 majors 2/3) rays only with a physics world, ignoring the written main_player; push-outs go through
    #     the controller's collide-and-slide, never straight into the position
    if dr is None or "phys == 0" not in dr or "player_ins: r.player" not in dr:
        problems.append("4: drive() casts without the phys_world() == 0 guard or not ignoring r.player")
    if re.search(r"\bpl\.pos\s*=\s*pl\.pos\s*\+", code["move"]):
        problems.append("4: movement.rs adds a push straight to the position (bypasses the collision)")
    # 5. the panic path: movement before the AC-130 and first person
    rt = fn_body(code["task"], "run_tick")
    if rt is None:
        problems.append("5: task.rs has no fn run_tick(")
    else:
        m, a, f = rt.find("st.movement.emergency_off("), rt.find("st.ac130.emergency_off("), rt.find("st.fp.emergency_off(")
        if m < 0 or a < 0 or f < 0 or not (m < a and m < f):
            problems.append("5: run_tick()'s panic path does not run movement.emergency_off() before the AC-130's and FP's")
        # (review M5 minor 6) after a panic the deferred gravity restore keeps being retried
        dis = rt.find("GAMEPLAY_DISABLED")
        if dis < 0 or "st.movement.disabled_tick()" not in rt[dis:rt.find("return;", dis) + 1]:
            problems.append("5: run_tick()'s disabled branch does not retry the deferred gravity restore (movement.disabled_tick)")
    # 6. the camera-step detour: MOVE_SEQ read twice around the feet, nothing that allocates, logs or locks
    mf = fn_body(code["hook"], "move_feet")
    ap = fn_body(code["hook"], "apply")
    if mf is None or ap is None or "move_feet(" not in ap:
        problems.append("6: cam_hook.rs apply() does not take the feet from move_feet()")
    else:
        s1 = mf.find("MOVE_SEQ.load")
        s2 = mf.find("MOVE_SEQ.load", s1 + 1)
        feet = mf.find("MOVE_FEET[")
        if s1 < 0 or s2 < 0 or not (s1 < feet < s2) or "s1 == s2" not in mf.replace(" ", " "):
            problems.append("6: move_feet() does not read MOVE_SEQ before and after the feet and compare")
        # (review M5 minor 5) a real seqlock: odd while publishing, the reader rejects odd values
        pf = fn_body(code["hook"], "publish_feet")
        if "s1 & 1 == 0" not in mf or pf is None or "| 1" not in pf or pf.find("| 1") > pf.find("MOVE_FEET["):
            problems.append("6: MOVE_SEQ is not an odd/even seqlock (odd before the feet stores, odd rejected by the reader)")
        bad = [t for t in MOVE_DETOUR_FORBIDDEN if t in mf or t in ap]
        if bad:
            problems.append(f"6: the camera-step detour path uses {bad}")
    # 7. the HP write: exactly one, in apply_fall_damage(), behind the fall_damage = off check
    hp_writes = re.findall(r"(?<![\w.])write\(\s*[\w.]+\s*\+\s*DATA_HP\b", code["move"])
    af = fn_body(code["move"], "apply_fall_damage")
    if len(hp_writes) != 1 or af is None or not re.search(r"write\(\s*[\w.]+\s*\+\s*DATA_HP", af):
        problems.append(f"7: {len(hp_writes)} HP write(s) in movement.rs (want exactly one, in apply_fall_damage)")
    elif "FallDamageCfg::Off" not in af or af.find("FallDamageCfg::Off") > af.find("DATA_HP"):
        problems.append("7: the HP write is not guarded by fall_damage = off")
    elif "!er_airborne" not in af or af.find("!er_airborne") > af.find("DATA_HP"):
        problems.append("7: the HP write is not guarded by ER's airborne confirmation (review M5 major 1)")
    # 8. the input detour records the movement presses before any blanking
    ps = fn_body(code["input"], "process_state")
    rk = fn_body(code["input"], "record_move_keys")
    if ps is None or rk is None or "PRESSES_MOVE[i].fetch_add" not in rk:
        problems.append("8: input_hook.rs has no record_move_keys() bumping PRESSES_MOVE")
    else:
        kb = ps.find("KEYBOARD_STATE_SIZE")
        rec = ps.find("record_move_keys(", kb)
        first_blank = min([i for i in (ps.find("= 0 };", kb), ps.find("blank_move_keys(", kb)) if i >= 0] or [-1])
        if rec < 0 or first_blank < 0 or rec > first_blank:
            problems.append("8: the keyboard branch blanks a key before record_move_keys() ran")
    # 9. movebench / iw4dis never linked into the DLL
    cargo = mod_cargo if mod_cargo is not None else (ROOT / "mod" / "mw2er" / "Cargo.toml").read_text(encoding="utf-8")
    if re.search(r"(?m)^\s*(movebench|iced|iced-x86|iw4dis)\b", cargo):
        problems.append("9: mw2er/Cargo.toml depends on movebench / iced / iw4dis")
    lk = lock if lock is not None else (ROOT / "mod" / "Cargo.lock").read_text(encoding="utf-8")
    pk = tomllib.loads(lk).get("package", [])
    deps = [p["name"] for p in pk if any(d.split(" ")[0] in ("movebench", "iced-x86", "iw4dis") for d in p.get("dependencies", []))]
    if deps or any(p["name"] in ("iced-x86", "iw4dis") for p in pk):
        problems.append(f"9: the mod workspace links movebench / iced-x86 / iw4dis into {deps or 'a package'}")
    return problems


def check_movewiring(self_test=False):
    try:
        src = move_sources()
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = movewiring_problems(src)
    if self_test:
        def mut(key, old, new, count=1):
            m = dict(src)
            if old not in src[key]:
                return src  # reported as "not applied"
            m[key] = src[key].replace(old, new, count)
            return m
        muts = {
            "1 position written in task.rs": mut("task", "fn run_tick(st: &mut TaskState) {",
                                                 "fn run_tick(st: &mut TaskState) {\n    unsafe { write(p + PHYS_POSITION, [0.0f32; 4]) };"),
            "1 WriteProcessMemory in movement.rs": mut("move", "use std::collections::HashSet;",
                                                       "use std::collections::HashSet;\nfn z() { WriteProcessMemory(); }"),
            "1 raw write in movement.rs": mut("move", "use std::collections::HashSet;",
                                              "use std::collections::HashSet;\nfn y(p: usize) { unsafe { std::ptr::write(p as *mut f32, 1.0) } }"),
            "2 gravity written before the save": mut("move", "        let (mult, dis) = self.gravity.own(r.physics, r.gravity_mult, r.gravity_disabled);\n",
                                                     "        let ok0 = unsafe { write(r.physics + PHYS_GRAVITY_MULT, 0.0f32) };\n        let (mult, dis) = self.gravity.own(r.physics, r.gravity_mult, r.gravity_disabled);\n"),
            "3 wildcard arm in release()": mut("move", "            Reason::Config => \"[movement] enabled = false\".into(),",
                                               "            _ => String::new(),"),
            "4 direct map ray in drive()": mut("move", "        let mut events = std::mem::take(&mut self.events);",
                                               "        let _ = map_ray_point(0, 0, V3::ZERO, 1.0, V3::ZERO);\n        let mut events = std::mem::take(&mut self.events);"),
            "4 rays ignore another player": mut("move", "player_ins: r.player", "player_ins: i.chars.len()"),
            "4 rays without the phys_world guard": mut("move", "        if phys == 0 || r.player == 0 {", "        if r.player == 0 {"),
            "4 push straight into the position": mut("move", "                                push_in = push_in + V3::new(v.x, 0.0, v.z) * IN_PER_M;",
                                                     "                                pl.pos = pl.pos + v * IN_PER_M;"),
            "1 position written without the sanity check": mut("move", "        if !position_write_ok(pos_m, r.pos, POS_WRITE_MAX_M) {", "        if false {"),
            "5 no deferred gravity retry after a panic": mut("task", "        if st.move_disabled_ok && guard(\"move.disabled\", || st.movement.disabled_tick()).is_none() {",
                                                             "        if st.move_disabled_ok && guard(\"move.disabled\", || ()).is_none() {"),
            "6 odd sequence accepted": mut("hook", "    if s1 & 1 == 0 && s1 == s2", "    if s1 == s2"),
            "7 HP write without the ER airborne gate": mut("move", "        if dmg_pct > 0.0 && !er_airborne {", "        if dmg_pct > 0.0 && false {"),
            "4 sim casts without the budget": mut("sim", "        let before = self.pos;", "        let _ = w.cast(self.pos, self.pos);\n        let before = self.pos;"),
            "5 movement emergency after FP": {**src, "task": src["task"].replace(
                "        let _ = guard(\"move.emergency\", || st.movement.emergency_off(shared::now_s()));\n", "").replace(
                "        let _ = guard(\"fp.emergency\", || st.fp.emergency_off(shared::now_s()));",
                "        let _ = guard(\"fp.emergency\", || st.fp.emergency_off(shared::now_s()));\n        let _ = guard(\"move.emergency\", || st.movement.emergency_off(shared::now_s()));")},
            "6 single MOVE_SEQ read": mut("hook", "    let s2 = MOVE_SEQ.load(Ordering::Acquire);", "    let s2 = s1;"),
            "6 log in the detour path": mut("hook", "        MOVE_FEET_USED.fetch_add(1, Ordering::Relaxed);",
                                            "        MOVE_FEET_USED.fetch_add(1, Ordering::Relaxed);\n        let _ = format!(\"x\");"),
            "7 second HP write": mut("move", "        self.last_written = Some(pos_m);",
                                     "        self.last_written = Some(pos_m);\n        unsafe { write(r.data_module + DATA_HP, 1i32) };"),
            "7 HP write without the off guard": mut("move", "        if mode == FallDamageCfg::Off {", "        if false {"),
            "8 blanking before the recording": mut("input", "        // M5: movement key levels and press counters first, before any key of this read is blanked.\n        // SAFETY: the 256-byte keyboard buffer of this call.\n        unsafe { record_move_keys(data) };\n", "") if False else
                {**src, "input": src["input"].replace(
                    "        // M5: movement key levels and press counters first, before any key of this read is blanked.\n        // SAFETY: the 256-byte keyboard buffer of this call.\n        unsafe { record_move_keys(data) };\n", "", 1).replace(
                    "        // ... then, in the gunship, ER sees every key released except Esc", "        unsafe { record_move_keys(data) };\n        // ... then, in the gunship, ER sees every key released except Esc", 1)},
        }
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(movewiring_problems(m)) <= base]
        cargo_mut = (ROOT / "mod" / "mw2er" / "Cargo.toml").read_text(encoding="utf-8") + "\nmovebench = { path = \"../movebench\" }\n"
        if set(movewiring_problems(src, mod_cargo=cargo_mut)) <= base:
            missed.append("9 movebench linked into mw2er")
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts) + 1} mutations of the M5 sources caught")
    return report(problems, "movewiring: 9 checks (docs/M5_DESIGN.md 13.4 + M5 review): position/proxy/gravity/HP written "
                            "only in movement.rs through game::write, the position behind position_write_ok, no "
                            "WriteProcessMemory, VirtualQuery only in the page check; the gravity save precedes its writes; "
                            "release() matches every Reason without a wildcard; Havok rays only through RayBudget::cast, only "
                            "with a physics world, ignoring r.player, no push straight into the position; the panic path "
                            "restores movement before the AC-130 and FP and retries a deferred restore; the detour reads an "
                            "odd/even MOVE_SEQ around the feet without alloc/log/lock; one HP write behind fall_damage = off "
                            "and ER's airborne confirmation; presses recorded before any blanking; movebench / iw4dis not linked")


def check_movestrings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    problems = []
    missing = [x.decode() for x in MOVE_LOG_STRINGS if x not in data]
    if missing:
        problems.append(f"missing MOVE log strings: {missing}")
    found = [x.decode() for x in MOVE_FORBIDDEN_STRINGS if x in data.lower()]
    if found:
        problems.append(f"forbidden strings in the DLL: {found}")
    return report(problems, f"movestrings: all {len(MOVE_LOG_STRINGS)} MOVE proof strings present; no iced / movebench / "
                            f"iw4dis string")

# ---- M6 arsenal wiring (docs/M6_DESIGN.md sections 10, 13.5 and 14 check 9) --------------------------------------

ARS_FILES = {"task": "task.rs", "hook": "cam_hook.rs", "ac": "ac130.rs", "input": "input_hook.rs", "shared": "shared.rs",
             "atask": "arsenal_task.rs", "art": "arsenal_rt.rs", "vm": "viewmodel.rs"}
ARS_CORE = {"cause": "spawn_cause.rs", "anim": "vm_anim.rs", "sw": "switch_logic.rs"}
ARS_DETOUR_FORBIDDEN = AC_DETOUR_FORBIDDEN
ARS_DISABLE_CLEARS = ("WANT_BLANK_WEAPON.store(false", "WANT_WHEEL.store(false", "VIEWFX_ON.store(false",
                      "SENS_SCALE_BITS.store(1.0f32.to_bits()")
# the message literals of the M6 lines (format strings are split at their placeholders; tags are their own literals)
ARS_LOG_STRINGS = [b"ARSENAL", b"loaded n=", b"data n=", b"disable gun=", b"SWITCH", b"start from=", b"raised gun=",
                   b"RECHAMBER", b"shell gun=", b"BREATH", b"hold gun=", b"LOCK", b"acquired gun=", b"LAUNCH", b"PROJ",
                   b"explode gun=", b"OK guns=", b"FAIL gun=", b"class gun=", b"start aborted reason=",
                   b"blocked reason=retry_backoff", b"code=", b"cause=", b"pool ", b"marker_probe=", b"BURST",
                   b"breaker open fails=", b"retry cause=owner",
                   # review M6
                   b"selfcheck gun=", b"why=self_radius", b"vm_unstuck gun=", b"burst cancelled why=",
                   b"start gun disabled gun=", b"not tried after the device failure", b"why=slots"]


def ars_sources():
    src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in ARS_FILES.items()}
    core = ROOT / "mod" / "mw2er-core" / "src"
    src.update({k: (core / v).read_text(encoding="utf-8") for k, v in ARS_CORE.items()})
    return src


def arsenalwiring_problems(src):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    # 1. view kick / zoom: one game::write of rows + fov in the detour path, nothing that allocates, logs or locks, run
    #    only outside the gunship and under catch_unwind
    vf = fn_body(code["hook"], "apply_view_fx")
    det = fn_body(code["hook"], "detour")
    if vf is None:
        problems.append("1: cam_hook.rs has no fn apply_view_fx(")
    else:
        n = len(re.findall(r"(?<![\w.])write\(", vf))
        bad = [t for t in ARS_DETOUR_FORBIDDEN if t in vf]
        if n != 1 or bad:
            problems.append(f"1: apply_view_fx() has {n} game::write call(s) (want 1) and forbidden {bad}")
    if det is None or "catch_unwind(AssertUnwindSafe(apply_view_fx))" not in det:
        problems.append("1: the camera-step detour does not run apply_view_fx under catch_unwind")
    elif det.find("apply_view_fx") < det.find("} else {"):
        problems.append("1: apply_view_fx runs in the gunship branch")
    # 2. run-13 bug 1: the exit edges are armed in try_start (after the gate) and cleared in end(); begin_ride arms none
    ts, en, br = fn_body(code["ac"], "try_start"), fn_body(code["ac"], "end"), fn_body(code["ac"], "begin_ride")
    if ts is None or "self.edges.arm(" not in ts or ts.find("self.edges.arm(") < ts.find("self.machine.start("):
        problems.append("2: try_start() does not arm the exit edges after the gate passed")
    if en is None or "self.edges.clear()" not in en:
        problems.append("2: end() does not clear the exit edges")
    if br is None or "edges.arm(" in br or re.search(r"load_counter\s*=", br):
        problems.append("2: begin_ride() sets an exit baseline (the stale-edge bug of run 13)")
    # 3. run-13 bug 2: every bullet spawn failure goes through on_spawn_fail (cause + pool counters); the SPAWN FAIL
    #    line exists only there; the breaker opens with the marker probe
    osf = fn_body(src["task"], "on_spawn_fail")  # raw source: the checked words are inside the log literal
    if osf is None or "cause=" not in osf or "pool_fields" not in osf or "marker_probe=" not in osf:
        problems.append("3: on_spawn_fail() does not log the cause, the pool counters and the marker probe")
    n_fail_lines = len(re.findall(r'"SPAWN",\s*"FAIL \{e\}', src["task"]))
    n_calls = len(re.findall(r"self\.on_spawn_fail\(", code["task"]))
    if n_fail_lines != 1 or n_calls < 2:
        problems.append(f"3: {n_fail_lines} SPAWN FAIL format(s) in task.rs (want 1, in on_spawn_fail) and {n_calls} "
                        f"on_spawn_fail call(s) (want the legacy and the arsenal fire paths)")
    obo = fn_body(code["task"], "on_breaker_open")
    if obo is None or "spawn::spawn(" not in obo or "classify(" not in obo:
        problems.append("3: on_breaker_open() does not probe the marker and classify")
    # 4. the breaker counts shots, not pellets; pellets spawn only while the pool has room
    fa = fn_body(code["task"], "fire_arsenal")
    if fa is None:
        problems.append("4: task.rs has no fn fire_arsenal(")
    else:
        if "tried > 0 && self.breaker.record(now, ok > 0," not in fa:
            problems.append("4: fire_arsenal() does not record one breaker result per shot (ok > 0 of its pellets)")
        if "cfg.arsenal.pellet_pool_gate { pellets_allowed(" not in fa:
            problems.append("4: fire_arsenal() spawns pellets without the pool gate")
    pa = fn_body(code["cause"], "pellets_allowed")
    if pa is None or "bullet_headroom" not in pa:
        problems.append("4: spawn_cause::pellets_allowed() does not use the bullet headroom")
    # 5. the weapon keys are counted before they are blanked
    ps = fn_body(code["input"], "process_state")
    if ps is None:
        problems.append("5: input_hook.rs has no fn process_state(")
    else:
        c, b = ps.find("PRESSES_WEAPON[i].fetch_add"), ps.find("*kp = 0")
        if c < 0 or b < 0 or c > b:
            problems.append("5: a weapon key is blanked before its press is counted")
    # 6. a disabled DLL hands the weapon keys, the wheel, the sensitivity and the camera back to ER
    dg = fn_body(code["shared"], "disable_gameplay")
    miss = [t for t in ARS_DISABLE_CLEARS if dg is None or t not in dg]
    if miss:
        problems.append(f"6: disable_gameplay() does not clear {miss}")
    # 7. a reset switch machine never leaves the viewmodel waiting in its drop
    resets = [m.start() for m in re.finditer(r"\.sw\.reset\(\)", code["task"])]
    stuck = [i for i in resets if "cancel_switch()" not in code["task"][i:i + 200]]
    if not resets or stuck:
        problems.append(f"7: {len(stuck)} of {len(resets)} sw.reset() call(s) without vm.cancel_switch() next to them")
    if "pub fn cancel_switch(" not in code["anim"]:
        problems.append("7: VmAnim has no cancel_switch()")
    # 8. the camera gate sees ER's fov while scoped (a sniper zoom is below the 0.2 rad floor)
    tk = code["task"]
    g8, u8 = tk.find("cam_hook::viewfx_er_fov("), tk.find("self.update_cam_gate(now, cam,")
    if g8 < 0 or u8 < 0 or g8 > u8:
        problems.append("8: the camera gate is fed the zoomed fov (no viewfx_er_fov() substitution before update_cam_gate)")
    # 9. the arsenal files never write game memory; no WriteProcessMemory / VirtualQuery
    for k in ("atask", "art"):
        t = code[k]
        if re.search(r"(?<![\w.])write\(|WriteProcessMemory|VirtualQuery|write_volatile|ptr::write", t):
            problems.append(f"9: {ARS_FILES[k]} writes memory or queries pages")
    # 10. one gun failing is skipped, not fatal: data (arsenal_rt) and viewmodel (viewmodel.rs) both disable per gun
    if "disabled.push((name.clone(), why));" not in code["art"]:
        problems.append("10: arsenal_rt::load() does not disable a failing gun on its own")
    if "VM_GUN_FAILED.fetch_or(" not in code["vm"] or "guns.push(None);" not in code["vm"]:
        problems.append("10: the viewmodel loader does not skip a gun whose bundle failed")
    # 11. review M6: the AC-130 gun keys 1/2 share the weapon keys: their raw edges are recorded before the weapon-key
    #     blanking, the weapon keys are never blanked in the gunship, and the task stores no weapon blank during a ride
    if ps is not None:
        a11, b11 = ps.find("AC_PRESSES[i].fetch_add"), ps.find("*kp = 0")
        if a11 < 0 or b11 < 0 or a11 > b11:
            problems.append("11: the AC-130 key edges are read after the weapon keys were blanked (keys 1/2 dead in the gunship)")
        if not re.search(r"let wwant = effective_want\(WANT_BLANK_WEAPON[^;]*&& !effective_want\(WANT_GUNSHIP_INPUT", ps):
            problems.append("11: the weapon-key blanking does not exclude the gunship")
    if not re.search(r"WANT_BLANK_WEAPON\.store\([^;]*!ac\.active", code["task"]):
        problems.append("11: task.rs stores WANT_BLANK_WEAPON without !ac.active")
    # 12. review M6: a drop always ends in a raise (a retarget back to the dropping gun used to stick in Drop) + the
    #     task's safety net
    tk12 = fn_body(code["sw"], "tick")
    if tk12 is None or "SwitchState::Ready { gun: from }" in tk12 or "self.begin_raise(to, t0 + dur, i, out);" not in tk12:
        problems.append("12: switch_logic tick() can end a drop without begin_raise (no SwitchRaise for the viewmodel)")
    if "vm.cancel_switch();" not in code["task"] or "why=ready_without_raise" not in src["task"]:
        problems.append("12: task.rs has no vm_unstuck safety net after switch_tick")
    # 13. review M6: launcher explosions next to the player are duds; the player's HP is logged after every blast
    tp = fn_body(code["task"], "tick_projectiles")
    if tp is None:
        problems.append("13: task.rs has no fn tick_projectiles(")
    else:
        g13, s13 = tp.find("self_blast_distance("), tp.find("spawn::spawn(p.field_ins_handle, r.id")
        if g13 < 0 or s13 < 0 or g13 > s13 or "cfg.launchers.self_guard" not in tp:
            problems.append("13: tick_projectiles() spawns the explosion rows without the self-radius guard")
        if "blast_hp.push(" not in tp or "selfcheck" not in fn_body(src["task"], "tick_projectiles"):
            problems.append("13: tick_projectiles() does not log the player's HP after a blast")
    # 14. review M6: a re-base / a new havok frame expires the live projectiles (never exploded in the wrong frame)
    tk = code["task"]
    m14 = re.search(r"self\.impacts\.clear\(\);\s*self\.cam_gate\.reset\(\);", tk)
    rb = m14.end() if m14 else -1
    if rb < 0 or "expire_projectiles(" not in tk[rb:rb + 400]:
        problems.append("14: the re-base branch keeps the launcher projectiles")
    if tp is not None and "proj_w.is_some_and(" not in tp:
        problems.append("14: tick_projectiles() does not check the projectiles' havok frame")
    return problems


def move_ac_loop(src):
    """Mutation: the AC-130 key-edge loop moved after the weapon-key blanking (the run-13+M6 'keys 1/2 dead' bug)."""
    t = src["input"]
    a = t.find("        for i in 0..AC_KEYS {")
    b = t.find("\n        }\n", a) + len("\n        }\n") if a >= 0 else -1
    anchor = "        if BLANK_TOGGLE_KEY.load(Ordering::Relaxed) {\n            let k = AC_DIK[AC_KEY_CALL]"
    if a < 0 or b < 0 or anchor not in t:
        return src
    block = t[a:b]
    t2 = t[:a] + t[b:]
    return {**src, "input": t2.replace(anchor, block + anchor, 1)}


def check_arsenalwiring(self_test=False):
    try:
        src = ars_sources()
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = arsenalwiring_problems(src)
    if self_test:
        def mut(key, old, new, count=1):
            if old not in src[key]:
                return src
            m = dict(src)
            m[key] = src[key].replace(old, new, count)
            return m
        muts = {
            "1 second write in apply_view_fx": mut("hook", "    if unsafe { write(cam + CAM_ROWS, out) } {",
                                                   "    let _ = unsafe { write(cam + CAM_ROWS, out) };\n    if unsafe { write(cam + CAM_ROWS, out) } {"),
            "1 log in apply_view_fx": mut("hook", "        VIEWFX_WRITES.fetch_add(1, Ordering::Relaxed);",
                                          "        VIEWFX_WRITES.fetch_add(1, Ordering::Relaxed);\n        log_i!(\"X\", \"y\");"),
            "1 view fx without catch_unwind": mut("hook", "        let _ = catch_unwind(AssertUnwindSafe(apply_view_fx));",
                                                  "        apply_view_fx();"),
            "2 edges armed in begin_ride": mut("ac", "        // (the load / mask / talk edges were armed in try_start; M6 run-13 bug 1)",
                                               "        if let Some(w) = w { self.edges.arm(w.load_counter, w.camera_mask, w.talk_id); }"),
            "2 edges not cleared in end()": mut("ac", "        self.edges.clear();\n        self.fade_watch.clear();",
                                                "        self.fade_watch.clear();"),
            "3 a spawn failure without its cause": mut("task", "                        self.on_spawn_fail(&e, id, p, origin, *d, &gname);",
                                                       "                        let _ = &e;"),
            "3 breaker opens without the marker probe": mut("task", "        let probe = spawn_guard::admit(shared::now_s(), SpawnKind::Gun, marker) && spawn::spawn(p.field_ins_handle, marker, origin, p.chr.pos_w, dir).is_ok();",
                                                            "        let probe = marker > 0;"),
            "4 breaker counted per pellet": mut("task", "tried > 0 && self.breaker.record(now, ok > 0,", "self.breaker.record(now, ok == tried,"),
            "4 pellets without the pool gate": mut("task", "cfg.arsenal.pellet_pool_gate { pellets_allowed(pool.as_ref(), n) }",
                                                   "cfg.arsenal.pellet_pool_gate { n }"),
            "5 weapon key blanked before counting": {**src, "input": src["input"].replace(
                "            if press_edge(WEAPON_RAW_DOWN[i].swap(down, Ordering::Relaxed), down) {\n                PRESSES_WEAPON[i].fetch_add(1, Ordering::Relaxed);\n            }\n", "", 1).replace(
                "                unsafe { *kp = 0 };\n            }\n        }", "                unsafe { *kp = 0 };\n            }\n            if press_edge(WEAPON_RAW_DOWN[i].swap(down, Ordering::Relaxed), down) {\n                PRESSES_WEAPON[i].fetch_add(1, Ordering::Relaxed);\n            }\n        }", 1)},
            "6 disable keeps the view fx": mut("shared", "    VIEWFX_ON.store(false, Ordering::SeqCst);\n", ""),
            "7 reset without cancel_switch": mut("task", "            if let Some(vm) = self.vm.as_mut() {\n                vm.cancel_switch();\n            }\n", ""),
            "8 gate fed the zoomed fov": mut("task", "&& let Some(er) = cam_hook::viewfx_er_fov(c.fov_raw)", "&& let Some(er) = None::<f32>"),
            "9 arsenal_task writes memory": mut("atask", "use std::sync::Arc;\n", "use std::sync::Arc;\nfn z(a: usize) { unsafe { crate::game::write(a, 1u8) }; }\n"),
            "10 a failing gun aborts the arsenal": mut("art", "disabled.push((name.clone(), why));", "return Err(why);"),
            "11 AC keys read after the weapon-key blanking": move_ac_loop(src),
            "11 weapon keys blanked in the gunship": mut("input", "\n            && !effective_want(WANT_GUNSHIP_INPUT.load(Ordering::Relaxed), now, heartbeat);", ";"),
            "11 WANT_BLANK_WEAPON during a ride": mut("task", "self.ars.is_some() && !ac.active, Ordering::SeqCst);", "self.ars.is_some(), Ordering::SeqCst);"),
            "12 retarget back sticks in Drop": mut("sw", "                    self.begin_raise(to, t0 + dur, i, out);\n                }\n                SwitchState::Raising",
                                                   "                    self.st = SwitchState::Ready { gun: from };\n                }\n                SwitchState::Raising"),
            "13 explosion without the self guard": mut("task", "&& let Some(dp) = self_blast_distance(", "&& let Some(dp) = no_guard("),
            "14 re-base keeps the projectiles": mut("task", "                        a.expire_projectiles(\"rebase\");", "                        let _ = a;"),
        }
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(arsenalwiring_problems(m)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the M6 sources caught")
    return report(problems, "arsenalwiring: 14 checks (docs/M6_DESIGN.md 10 + 13.5 + review): one game::write in the view-fx "
                            "detour path (no alloc/log/lock, catch_unwind, never in the gunship); run-13 bug 1 edges armed "
                            "in try_start after the gate and cleared in end(), none in begin_ride; run-13 bug 2 failures "
                            "only through on_spawn_fail (cause + pool + marker probe), breaker per shot, pellet pool gate; "
                            "weapon keys counted before blanking; disable_gameplay hands keys/wheel/sens/camera back; "
                            "sw.reset() always with cancel_switch(); camera gate on ER's fov; arsenal files write no "
                            "memory; a failing gun is skipped; AC-130 keys 1/2 recorded before the weapon-key blanking, never blanked in the "
                            "gunship; every drop ends in a raise (+ vm_unstuck net); launcher self-radius dud + HP selfcheck; "
                            "re-base / frame change expires projectiles")


def check_arsenalstrings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    missing = [x.decode() for x in ARS_LOG_STRINGS if x not in data]
    return report([f"missing M6 log strings: {missing}"] if missing else [],
                  f"arsenalstrings: all {len(ARS_LOG_STRINGS)} M6 proof strings present")


M2_ROWS_FIXTURE = pathlib.Path(__file__).resolve().parent / "fixtures" / "m2_regulation_hashes.json"
M4_EDITED = ("Bullet.param", "AtkParam_Pc.param", "SpEffectParam.param")


def check_m2rows():
    """The 13 M2 rows and every param M4 does not edit are byte-identical to the recorded M2 build."""
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    import param_diff
    from record_m2_rows import M2_ROWS, row_hash
    check = ROOT / "work" / "regulation" / "build_check"
    problems = []
    try:
        fx = json.loads(M2_ROWS_FIXTURE.read_text(encoding="utf-8"))
        pkg = sha256(PACKAGE_DIR / "regulation.bin")
        back = sha256(check / "regulation.bin")
    except (OSError, ValueError) as e:
        return report([f"inputs: {e}"], "")
    if back != pkg:
        problems.append(f"work/regulation/build_check is {back[:12]}, not the read-back of build/package ({pkg[:12]}): "
                        "rerun build_regulation.py")
        return report(problems, "")
    params = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((check / "regulation-bin").glob("*.param"))}
    if sorted(params) != sorted(fx["params"]):
        problems.append(f"param file set differs: +{sorted(set(params) - set(fx['params']))[:3]} "
                        f"-{sorted(set(fx['params']) - set(params))[:3]}")
    changed = sorted(n for n in fx["params"] if n in params and params[n] != fx["params"][n])
    unexpected = [n for n in changed if n not in M4_EDITED]
    if unexpected:
        problems.append(f"params changed that M4 does not edit: {unexpected[:5]}")
    n_rows = 0
    for stem, ids in M2_ROWS.items():
        prm = param_diff.read_param(check / "regulation-bin" / f"{stem}.param")
        by = {r[0]: r for r in prm["rows"]}
        for i in ids:
            n_rows += 1
            if i not in by:
                problems.append(f"M2 row {stem} {i} missing")
            elif row_hash(*by[i]) != fx["rows"][stem][str(i)]:
                problems.append(f"M2 row {stem} {i} changed")
    return report(problems, f"m2rows: package {pkg[:12]}; the {n_rows} M2 rows byte-identical to the M2 build "
                            f"{fx['package_sha256'][:12]}; {len(params) - len(changed)}/{len(params)} params identical, "
                            f"changed only {changed} (M4 adds rows there)")



# ---- M7 (docs/M7_DESIGN.md 6.5 / 7 check 10): the emitter fix, the spawn guard and the Tactical Nuke --------------

NUKE_FILES = {"nuke": "nuke.rs", "task": "task.rs", "ac": "ac130.rs", "input": "input_hook.rs", "shared": "shared.rs",
              "spawn": "spawn.rs", "sguard": "spawn_guard.rs", "present": "nuke_present.rs", "vm": "viewmodel.rs"}
NUKE_CORE = {"logic": "nuke_logic.rs", "targets": "nuke_targets.rs"}
VENDOR_BULLET = ROOT / "vendor" / "fromsoftware-rs" / "crates" / "eldenring" / "src" / "cs" / "bullet_manager.rs"
# the message literals of the M7 lines (format strings are split at their placeholders)
NUKE_LOG_STRINGS = [b"emitters live=", b"throttle why=emitters live=", b"WARN emitters live=", b"WARN persistent=",
                    b"cause=pool_emitters_full", b"RECHAMBER", b"cancelled gun=", b"NUKE", b"activate test=",
                    b"countdown t=", b"incoming t=", b"detonate t=", b"census pass=", b"kill apply handle=",
                    b"kill handle=", b"survivor handle=", b"kills total=", b"player prep fp=off",
                    b"player lethal ok=", b"player dead t_ms=", b"cancel reason=loading refund=", b"end reason=",
                    b"post gpu_ms=", b"blocked reason=", b"streak n=", b"earned streak=", b"streak reset reason=death",
                    b"horizon spawned=", b"toggle blocked reason=nuke", b"blocked reason=nuke",
                    b"kill would-apply handle=", b"kill armed pass=", b"kill deferred handle=", b"census_us=",
                    b"skip reason=other_frame", b"would_apply=", b"walked="]


def nuke_sources():
    src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in NUKE_FILES.items()}
    core = ROOT / "mod" / "mw2er-core" / "src"
    src.update({k: (core / v).read_text(encoding="utf-8") for k, v in NUKE_CORE.items()})
    src["vendor"] = VENDOR_BULLET.read_text(encoding="utf-8")
    return src


def nukewiring_problems(src):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    # 1. the auto-finish bit: bit 0 only, never bit 3 (network sync); pinned in the DLL and the vendored new()
    new = fn_body(code["vendor"], "new") or ""
    if not re.search(r"\bd\.unk44 = 1;", new) or re.search(r"unk44\s*=\s*(?:[2-9]|1\d|0x[0-9a-f]*[89a-f])", new) \
            or "| 8" in new:
        problems.append("1: BulletSpawnData::new does not set unk44 = 1 (bit 0 only)")
    if not re.search(r"pub const SPAWN_FLAGS: u32 = 1;", code["spawn"]) or "offset_of!(BulletSpawnData, unk44) == 0x44" not in code["spawn"]:
        problems.append("1: spawn.rs does not pin SPAWN_FLAGS = 1 / the +0x44 offset")
    # 2. every engine spawn asks the emitter guard first (task, AC-130, nuke): a `spawn::spawn(` call has
    #    `spawn_guard::admit(` (or the AC-130's crate path) within the 600 characters before it
    for k in ("task", "ac", "nuke"):
        t = code[k]
        for m in re.finditer(r"spawn::spawn\(", t):
            if "admit(" not in t[max(0, m.start() - 600):m.start()]:
                problems.append(f"2: {NUKE_FILES[k]}: a spawn::spawn( without the emitter guard before it")
                break
    if "Kind::Ac130" not in code["ac"] or code["nuke"].count("SpawnKind::Nuke") < 2:
        problems.append("2: the AC-130 / nuke spawns are not priority spawns (Kind::Ac130 / SpawnKind::Nuke)")
    adm = fn_body(code["sguard"], "admit") or ""
    if "Verdict::Full" not in adm or "log_e!" not in fn_body(src["sguard"], "admit") or "pool_emitters_full" not in fn_body(src["sguard"], "admit"):
        problems.append("2: spawn_guard::admit() refuses a full pool without its SPAWN FAIL line")
    # 3. the nuke never writes game memory: no game::write / raw pointer writes / HP fields in nuke.rs; its only
    #    engine calls are the gated SpEffect wrappers of ac130.rs
    n = code["nuke"]
    if re.search(r"(?<![\w.:])write\(|game::write|write_volatile|ptr::write|WriteProcessMemory|NtWriteVirtualMemory", n):
        problems.append("3: nuke.rs writes game memory")
    if re.search(r"\.hp\s*=[^=]|hp_write|set_hp\(", n):
        problems.append("3: nuke.rs stores an HP value")
    calls = re.findall(r"crate::ac130::(apply_speffect|remove_speffect)\(", n)
    if sorted(set(calls)) != ["apply_speffect", "remove_speffect"] or re.search(r"\.(apply|remove)_speffect\(", n):
        problems.append("3: nuke.rs must use only crate::ac130::apply_speffect / remove_speffect (the gated wrappers)")
    # 4. the player's lethal effect only after the protection is confirmed absent (pure order + the DLL's arm)
    st = fn_body(code["logic"], "step") or ""
    i_pres, i_ret, i_lethal = st.find("let present = protect_present != Some(false);"), st.find("RemoveProtect { try_n"), st.find("ApplyLethal { try_n: 1 }")
    if min(i_pres, i_ret, i_lethal) < 0 or not (i_pres < i_ret < i_lethal):
        problems.append("4: PlayerKill::step can apply the lethal effect before the protection is confirmed absent")
    stp = fn_body(code["nuke"], "step") or ""
    if "speffect_count(chr, protect)" not in stp or "self.pk.step(now, prep_due, lethal_due, present," not in stp:
        problems.append("4: the nuke does not feed this tick's protection read into PlayerKill::step")
    # 5. the kill pass re-reads each target by handle right before the engine call
    ab = fn_body(code["nuke"], "apply_batch") or ""
    r5, a5 = ab.find(".filter(|n| n.c.handle == h)"), ab.find("apply_speffect(")
    if r5 < 0 or a5 < 0 or r5 > a5:
        problems.append("5: apply_batch applies without re-reading the target by its handle")
    # 6. a panic inside the nuke tick ends the sequence, the task's panic path ends it too
    tk = fn_body(src["nuke"], "tick") or ""
    if 'guard("nuke"' not in tk or "emergency_off" not in tk:
        problems.append("6: NukeCtl::tick is not guarded with an emergency_off on panic")
    rt = fn_body(src["task"], "run_tick") or ""
    if 'guard("nuke.emergency", || st.nuke.emergency_off())' not in rt:
        problems.append("6: run_tick's panic path does not end a nuke sequence")
    # 7. key 4: counted before it is blanked; handed back by disable_gameplay
    ps = fn_body(code["input"], "process_state") or ""
    c7, b7 = ps.find("PRESSES_NUKE.fetch_add"), ps.find("*np = 0")
    if c7 < 0 or b7 < 0 or c7 > b7:
        problems.append("7: the nuke key is blanked before its press is counted")
    if "WANT_BLANK_NUKE.store(false" not in (fn_body(code["shared"], "disable_gameplay") or ""):
        problems.append("7: disable_gameplay() keeps the nuke key blanked")
    # 8. F5 off and F7 are refused while a sequence runs
    tg = fn_body(code["task"], "toggle") or ""
    if not 0 <= tg.find("self.nuke.busy()") < tg.find("MODE_ON.store(false"):
        problems.append("8: F5 can turn MW2 mode off during a nuke sequence")
    it = fn_body(code["ac"], "idle_tick") or ""
    if not 0 <= it.find("i.nuke_busy") < it.find("self.try_start("):
        problems.append("8: F7 can start an AC-130 ride during a nuke sequence")
    # 9. the post pass runs under its own guard, after the viewmodel / AC-130 passes, behind the snapshot gate
    op = fn_body(code["vm"], "on_pre_imgui") or ""
    if not 0 <= op.find("callback_body(ctx)") < op.find("nuke_present::dispatch(ctx)") or 'guard("nuke.callback"' not in src["vm"]:
        problems.append("9: the nuke post pass is not dispatched after the viewmodel callback under its own guard")
    dp = fn_body(code["present"], "dispatch") or ""
    g9, r9 = dp.find("SNAPSHOT_MAX_AGE_S"), dp.find(".record(")
    if g9 < 0 or r9 < 0 or g9 > r9 or "a.post.active()" not in dp:
        problems.append("9: nuke_present::dispatch records without the snapshot gate (active post state, age, in world)")
    # 10. target selection is a deny-list: friendly teams are spared only with spare_friendly; Torrent / spirits always
    cl = fn_body(code["targets"], "classify") or ""
    if "cfg.spare_friendly && cfg.spare_teams.contains(&c.team)" not in cl or "in_player_sets" not in cl or "exclude_character_ids" not in cl:
        problems.append("10: nuke_targets::classify is not the deny-list (spare_teams with spare_friendly; sets + excluded ids always)")
    if "return Err(Skip::OtherFrame);" not in cl:
        problems.append("10: nuke_targets::classify does not count other-frame characters (they would vanish from the census)")
    # 11. key 4 is released on every NukeCtl exit path: the disabled early return and emergency_off
    tk11 = fn_body(code["nuke"], "tick") or ""
    i_dis, i_rel, i_ret = tk11.find("if self.disabled {"), tk11.find("WANT_BLANK_NUKE.store(false"), tk11.find("return NukeOut::default();")
    if min(i_dis, i_rel, i_ret) < 0 or not (i_dis < i_rel < i_ret):
        problems.append("11: NukeCtl::tick's disabled return leaves key 4 blanked")
    if "WANT_BLANK_NUKE.store(false" not in (fn_body(code["nuke"], "emergency_off") or ""):
        problems.append("11: NukeCtl::emergency_off leaves key 4 blanked")
    # 12. NPC kills: the SpEffect only when armed (dry run / team check) and never over scripted invincibility; retries
    #     queue only the eligible-now list
    ab12 = fn_body(code["nuke"], "apply_batch") or ""
    a12 = ab12.find("apply_speffect(")
    if not (0 <= ab12.find("if n.invincible() {") < a12) or not (0 <= ab12.find("if !armed {") < a12) \
            or "let armed = matches!(self.npc_armed, Some(Ok(())));" not in ab12:
        problems.append("12: apply_batch can apply the lethal SpEffect unarmed (dry run / unverified teams) or over invincibility")
    kp = fn_body(code["nuke"], "kill_pass") or ""
    if "npc_kills_armed(cfg.nuke.dry_run," not in kp or "list.extend(sel.deferred" in kp or "for &h in &sel.kill {" not in kp:
        problems.append("12: kill_pass does not arm from dry_run + the team check, or queues the deferred (invincible) ones")
    na = fn_body(code["targets"], "npc_kills_armed") or ""
    if "if dry_run {" not in na or "team_check(by_team, spare_teams)" not in na:
        problems.append("12: npc_kills_armed ignores dry_run or the team check")
    # 13. the player's death needs a real sample; the emitter guard decides on the walked count when the bound is high
    stp13 = fn_body(code["nuke"], "step") or ""
    if "player_dead_input(i.player.map(|p| p.chr.hp))" not in stp13 or "is_none_or(|p| p.chr.hp <= 0)" in stp13:
        problems.append("13: a missing player sample can read as the player's death")
    est = fn_body(code["sguard"], "estimate") or ""
    if "needs_walk(upper" not in est or "emitter_count_walk()" not in est or "decide_est(now, upper, walked," not in (fn_body(code["sguard"], "admit") or ""):
        problems.append("13: the emitter guard decides on the counter bound alone (no walk correction)")
    return problems


def check_nukewiring(self_test=False):
    try:
        src = nuke_sources()
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = nukewiring_problems(src)
    if self_test:
        def mut(key, old, new, count=1):
            if old not in src[key]:
                return src
            m = dict(src)
            m[key] = src[key].replace(old, new, count)
            return m
        muts = {
            "1 network-sync bit": mut("vendor", "        d.unk44 = 1;", "        d.unk44 = 9;"),
            "1 flags not pinned": mut("spawn", "pub const SPAWN_FLAGS: u32 = 1;", "pub const SPAWN_FLAGS: u32 = 0;"),
            "2 nuke spawn without the guard": mut("nuke", "        if !spawn_guard::admit(i.now, SpawnKind::Nuke, d.rows.horizon_bullet_id) {",
                                                  "        if false {"),
            "2 AC-130 not a priority spawn": mut("ac", "mw2er_core::emitter_guard::Kind::Ac130", "mw2er_core::emitter_guard::Kind::Gun"),
            "2 full pool refused silently": mut("sguard", "cause=pool_emitters_full", "cause=x", count=-1),
            "3 nuke writes HP": mut("nuke", "    fn horizon(&mut self, i: &NukeIn, k: u32) {",
                                    "    fn horizon(&mut self, i: &NukeIn, k: u32) {\n        unsafe { crate::game::write(0usize, 0i32) };"),
            "3 direct engine call": mut("nuke", "unsafe { crate::ac130::apply_speffect(t.addr, d.rows.lethal_speffect_id) }",
                                        "unsafe { (&mut *(t.addr as *mut eldenring::cs::ChrIns)).apply_speffect(d.rows.lethal_speffect_id, true); true }"),
            "4 lethal before the protection check": mut("logic", "                let present = protect_present != Some(false);\n",
                                                        "                if lethal_due { self.stage = PkStage::Watching; return PkAction::ApplyLethal { try_n: 1 }; }\n                let present = protect_present != Some(false);\n"),
            "4 stale protection read": mut("nuke", "speffect_count(chr, protect)", "Some(0usize)"),
            "5 applied without a re-read": mut("nuke", ".filter(|n| n.c.handle == h)", ""),
            "6 nuke tick unguarded": mut("nuke", 'match guard("nuke", || self.tick_inner(i)) {', 'match Some(self.tick_inner(i)) {'),
            "6 panic path keeps the sequence": mut("task", 'let _ = guard("nuke.emergency", || st.nuke.emergency_off());', ""),
            "7 key blanked before counting": {**src, "input": src["input"].replace(
                "            if press_edge(crate::shared::NUKE_RAW_DOWN.swap(down, Ordering::Relaxed), down) {\n                crate::shared::PRESSES_NUKE.fetch_add(1, Ordering::Relaxed);\n            }\n", "", 1).replace(
                "                unsafe { *np = 0 };\n            }\n", "                unsafe { *np = 0 };\n            }\n            if press_edge(crate::shared::NUKE_RAW_DOWN.swap(down, Ordering::Relaxed), down) {\n                crate::shared::PRESSES_NUKE.fetch_add(1, Ordering::Relaxed);\n            }\n", 1)},
            "7 disable keeps the key": mut("shared", "    WANT_BLANK_NUKE.store(false, Ordering::SeqCst);\n", ""),
            "8 F5 off during a sequence": mut("task", "            if self.nuke.busy() {\n                log_i!(\"MODE\", \"toggle blocked reason=nuke\");",
                                              "            if false {\n                log_i!(\"MODE\", \"toggle blocked reason=nuke\");"),
            "8 F7 during a sequence": mut("ac", "            if i.nuke_busy {", "            if false {"),
            "9 post pass without the gate": mut("present", "if !a.post.active() || now - a.t > SNAPSHOT_MAX_AGE_S || !a.in_world {", "if false {"),
            "10 allow-list of teams": mut("targets", "if cfg.spare_friendly && cfg.spare_teams.contains(&c.team) {", "if !cfg.spare_teams.contains(&c.team) {"),
            "10 other frames dropped": mut("targets", "        return Err(Skip::OtherFrame);\n", "        return Err(Skip::Range);\n"),
            "11 disabled return keeps the key": mut("nuke", "            shared::WANT_BLANK_NUKE.store(false, Ordering::SeqCst);\n            return NukeOut::default();",
                                                    "            return NukeOut::default();"),
            "11 emergency_off keeps the key": mut("nuke", "        // key 4 back to ER on every exit path (input_hook keeps blanking while the flag is set and the task ticks)\n        shared::WANT_BLANK_NUKE.store(false, Ordering::SeqCst);\n", ""),
            "12 dry run ignored": mut("nuke", "            if !armed {\n", "            if false {\n"),
            "12 applied over invincibility": mut("nuke", "            if n.invincible() {\n", "            if false {\n"),
            "12 retries queue the deferred": mut("nuke", "        for &h in &sel.kill {", "        for &h in sel.kill.iter().chain(sel.deferred.iter()) {"),
            "12 arming without dry_run": mut("targets", "    if dry_run {\n        return Err(\"dry_run\".into());\n    }\n", ""),
            "13 missing sample = dead": mut("nuke", "player_dead_input(i.player.map(|p| p.chr.hp))", "Some(i.player.is_none_or(|p| p.chr.hp <= 0))"),
            "13 guard without the walk": mut("sguard", "    let v = bullets::emitter_count_walk();", "    let v = None;"),
        }
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(nukewiring_problems(m)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the M7 sources caught")
    return report(problems, "nukewiring: 13 checks (docs/M7_DESIGN.md 6.5 + 11.1): unk44 = 1 (bit 0 only) pinned; every engine spawn "
                            "behind the emitter guard, AC-130 / nuke priority, a full pool never silent; the nuke writes no "
                            "memory and no HP, only the gated SpEffect wrappers; the lethal effect only after the protection "
                            "is confirmed absent (this tick's read); targets re-read by handle; guarded tick + panic path; "
                            "key 4 counted before blanking and handed back; F5 / F7 refused during a sequence; the post pass "
                            "guarded and gated; deny-list target selection")


def check_nukestrings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    missing = [x.decode() for x in NUKE_LOG_STRINGS if x not in data]
    sha = [s for s in ("MW2ER_NUKE_PS",) if s.encode() in data]
    return report([f"missing M7 log strings: {missing}"] if missing else [],
                  f"nukestrings: all {len(NUKE_LOG_STRINGS)} M7 proof strings present{' ' + str(sha) if sha else ''}")


# ---- M8 mantle / ladder wiring (docs/M8_DESIGN.md section 12.3) ---------------------------------------------------

M8_FILES = {"move": "movement.rs", "task": "task.rs", "ac": "ac130.rs", "nuke": "nuke.rs", "fp": "first_person.rs",
            "fpread": "game/fp_read.rs"}
M8_CORE = {"mantle": "mantle.rs", "sim": "move_sim.rs", "logic": "move_logic.rs", "vm": "vm_anim.rs", "acl": "ac130_logic.rs",
           "nl": "nuke_logic.rs"}
M8_LOG_STRINGS = [b"mantle start kind=", b"mantle end dt=", b"mantle abort why=", b"mantle check ok=0 why=", b"mantle rebase d=",
                  b"mantle settle er_ground=", b"mantle release kind=over", b"mantle_rays_max=", b"mantle_checks=", b"LADDER",
                  b"state from=", b"end state=", b"blocked reason=ladder state=", b"interact cleared why=ladder",
                  b"ARSENAL", b"lowered why=", b"raised why=", b"blocked reason=ladder (ER drives the climb", b"blocked reason=ladder test=",
                  b"resync why=rebase d=", b"cancelled reason=", b"mantle=", b"ladder_er_attacks=",
                  # M8 review
                  b"mantle abort why=position_insane", b"mantle_budget_max=", b"mantle_deferred=", b" mw2_drop=", b" vm_drop=",
                  # run 18: the close-camera gate, the ray filters and their hit counts, the unstick handback
                  b" close_ok=", b" filters=", b" body_probes=", b" unstick=", b"hits=map:", b"unstick n=", b"WARN unstick capped n=",
                  b"no progress while ER pushed out", b" depen=", b" narrow_top=",
                  # co-op C2: the peer watch and the nuke's co-op rules
                  b"COOP", b"watch slots=", b" sess_players=", b"blocked reason=coop_session peers=", b"cancel reason=coop_session refund=",
                  b" coop_peers=", b" allow_in_coop=", b"coop note shown peers=", b"Nuke disabled in co-op",
                  b"Co-op nuke: only your game's enemies are affected; friends won't see it yet"]


def m8_sources():
    src = {k: (MOD_SRC / v).read_text(encoding="utf-8") for k, v in M8_FILES.items()}
    core = ROOT / "mod" / "mw2er-core" / "src"
    src.update({k: (core / v).read_text(encoding="utf-8") for k, v in M8_CORE.items()})
    return src


def mantlewiring_problems(src):
    problems = []
    code = {k: strip_comments(v) for k, v in src.items()}
    lib = {k: code[k].split("#[cfg(test)]")[0] for k in ("mantle", "sim", "logic")}
    # 1. every mantle ray through RayBudget::cast (the Caster wraps it); none straight into the world
    cb = impl_body(lib["mantle"], "impl Caster<'_, '_, '_>")
    if cb is None or "self.b.cast(" not in cb:
        problems.append("1: mantle.rs Caster::cast does not go through RayBudget::cast")
    if re.search(r"\b(w|world|self\.w)\.cast\(", lib["mantle"]):
        problems.append("1: mantle.rs casts a ray straight into the world")
    ms = fn_body(lib["sim"], "mantle_substep")
    tm = fn_body(lib["sim"], "try_mantle")
    if ms is None or tm is None or "mb.cast(" not in ms or "find_ledge(mb," not in tm.replace(" ", " "):
        problems.append("1: the mantle substep / detection does not use the mantle budget (mb)")
    tk = fn_body(lib["sim"], "tick")
    if tk is None or "RayBudget::new(cfg.mantle.max_rays + mantle::SUBSTEP_RAYS * (n - 1))" not in tk:
        problems.append("1: Player::tick has no separate mantle budget sized by the substeps (RayBudget::new(cfg.mantle.max_rays + ...))")
    if ms is not None and re.search(r"\bb\.cast\(", ms):
        problems.append("1: the mantle substep casts on the main budget")
    # 2. never an unvalidated position: the path point is taken only after the re-validation passed; no landing at the end
    first_pos = min([i.start() for i in re.finditer(r"self\.pos\s*=", ms)] or [-1]) if ms else -1
    if ms is None or "check(mb, w, &mut mm)" not in ms or first_pos < ms.find("check(mb, w, &mut mm)"):
        problems.append("2: mantle_substep moves the feet before (or without) the re-validation")
    if ms is not None and "self.land(" in ms:
        problems.append("2: mantle_substep calls land() (no Land event at a mantle end)")
    # 3. release() aborts the mantle before the gravity restore; enter() never resumes one; a warp aborts, a re-base
    #    translates
    rel = fn_body(code["move"], "release")
    if rel is None or "mantle_abort(" not in rel or "self.restore_gravity(" not in rel or rel.find("mantle_abort(") > rel.find("self.restore_gravity("):
        problems.append("3: release() does not abort the mantle before restoring gravity")
    en = fn_body(code["move"], "enter")
    if en is None or "mantle.is_none()" not in en or "REARM_BLOCK_S" not in en:
        problems.append("3: enter() does not assert the mantle is gone / block the check for 0.3 s")
    dr = fn_body(code["move"], "drive")
    if dr is None or "mantle_abort(AbortWhy::Warp)" not in dr or "pl.translate(e * IN_PER_M)" not in dr:
        problems.append("3: drive() does not abort on a warp / translate on a re-base")
    tr = fn_body(lib["sim"], "translate")
    if tr is None or "m.translate(by)" not in tr:
        problems.append("3: Player::translate does not move the mantle path")
    # 4. the position write stays behind position_write_ok (the M5 rule, now also for the mantle path)
    if dr is None or dr.find("position_write_ok(") < 0 or dr.find("position_write_ok(") > dr.find("PHYS_POSITION,"):
        problems.append("4: drive() writes the position without position_write_ok")
    # 5. ER push-out measured, never applied, during a mantle
    if dr is None or "if mc.er_pushout && !pl.mantling() {" not in dr or "mantle_er_in = ep * IN_PER_M;" not in dr:
        problems.append("5: drive() applies ER's push-out during a mantle (or does not measure it)")
    # 6. the weapon gate: fire / ADS / reload / the sim hold read it; the viewmodel gets its own input (never SwitchDrop)
    rt = code["task"]
    if "let mut trigger = can_fire && fire_raw && !self.mv.fire_blocked && !gate_block;" not in rt or "ads_raw && !self.mv.fire_blocked && !gate_block" not in rt:
        problems.append("6: the task's trigger / ADS do not read the weapon gate")
    if "self.wgate.update(now, self.mv.mantle_lowered, on_ladder, raise_s)" not in rt:
        problems.append("6: the weapon gate is not fed by the mantle and the ladder")
    if "let reloads = if gate_block { 0 } else { reloads };" not in rt or "sim.set_hold(switching || self.mv.fire_blocked || gate_block || dead_block);" not in rt:
        problems.append("6: reloads / the sim hold ignore the weapon gate")
    gate_blk = rt[rt.find("match self.wgate.update("):rt.find("let gate_block")] if "match self.wgate.update(" in rt else ""
    if "SwitchDrop" in gate_blk or "lowered: self.wgate.lowered()" not in rt:
        problems.append("6: the lowered gun goes through the weapon-switch path (or has no viewmodel input)")
    # 7. the ladder feeds the movement, first person, the AC-130 and the nuke gates
    if "t.set(Reason::Ladder, p.is_some_and(|p| fp_logic::on_ladder(p.ladder)));" not in code["move"]:
        problems.append("7: the movement controller does not hand back on a ladder")
    if "ladder: p.is_some_and(|p| fp_logic::on_ladder(p.ladder))" not in code["fp"]:
        problems.append("7: first person does not hand back on a ladder")
    if "ladder: p.is_some_and(|p| fp_logic::on_ladder(p.ladder))," not in code["ac"] or "(g.ladder, Block::Ladder)," not in code["acl"]:
        problems.append("7: the AC-130 gate ignores the ladder (B1)")
    if "ladder: i.ladder," not in code["nuke"] or "ladder: on_ladder," not in rt or "return Err(Block::Ladder);" not in code["nl"]:
        problems.append("7: the nuke gate ignores the ladder (B1)")
    # 8. classify_jump gets vectors (P1)
    if not re.search(r"pub fn classify_jump\(e: V3, c: Option<V3>, expected: f32, loading_recent: bool\)", lib["logic"]) or "classify_jump(e, chunk_v," not in code["move"]:
        problems.append("8: classify_jump does not take the position and chunk vectors")
    # 9. the ladder offsets pinned, the fp read failure handed back
    for a in ("offset_of!(ChrInsModuleContainer, ladder) == 0x78", "offset_of!(CSChrLadderModule, state) == 0x18",
              "offset_of!(CSChrLadderModule, top) == 0x20", "offset_of!(CSChrLadderModule, bottom) == 0x30"):
        if a not in code["fpread"]:
            problems.append(f"9: fp_read.rs lacks the assert {a}")
    if "self.fp_fail_streak >= READ_FAIL_TICKS" not in code["move"]:
        problems.append("9: a failing first-person read never hands back (read_fail fp:<field>)")
    # 10. B3 / B4
    if "self.interact_flap_m8.tick(i.now, er_confirmed(&t), interact)" not in code["move"] or "self.interact.clear();" not in code["move"]:
        problems.append("10: the interact flap guard / the interact clear on a ladder (B3 / B4) are not wired")
    # 11. (M8 review) the position_insane resync aborts and logs a running mantle first; the re-validation has its
    #     rise head ray; a start the tick cannot re-validate is deferred
    if dr is not None:
        ins = dr.find("if !position_write_ok(pos_m, r.pos, POS_WRITE_MAX_M)")
        ab = dr.find("pl.mantle_abort(AbortWhy::Insane)", ins)
        rs = dr.find("pl.resync(", ins)
        if ins < 0 or ab < 0 or rs < 0 or ab > rs or "mantle abort why=position_insane" not in src["move"]:
            problems.append("11: the position_insane path resyncs without aborting / logging the mantle")
    if ms is None or "mm.dir * (cfg.radius + 0.5 * cfg.skin)" not in ms:
        problems.append("11: the rise re-validation has no forward head ray")
    if tm is None or "mantle::SUBSTEP_RAYS * left" not in tm:
        problems.append("11: try_mantle starts a climb the rest of the tick cannot re-validate")
    return problems


def check_mantlewiring(self_test=False):
    try:
        src = m8_sources()
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = mantlewiring_problems(src)
    if self_test:
        def mut(key, old, new):
            if old not in src[key]:
                return src
            m = dict(src)
            m[key] = src[key].replace(old, new, 1)
            return m
        muts = {
            "1 a mantle ray straight into the world": mut("mantle", "    let used0 = b.used;\n    let mut k = Caster { b, w, hook };",
                                                         "    let used0 = b.used;\n    let _ = w.cast(V3::ZERO, V3::ZERO);\n    let mut k = Caster { b, w, hook };"),
            "1 Caster bypasses the budget": mut("mantle", "self.b.cast(self.w, RayKind::Vertical, o, delta)", "Ok(self.w.cast(o, delta))"),
            "1 no separate mantle budget": mut("sim", "RayBudget::new(cfg.mantle.max_rays + mantle::SUBSTEP_RAYS * (n - 1))", "RayBudget::new(cfg.max_rays)"),
            "1 the mantle budget not sized by the substeps": mut("sim", "RayBudget::new(cfg.mantle.max_rays + mantle::SUBSTEP_RAYS * (n - 1))", "RayBudget::new(cfg.mantle.max_rays)"),
            "11 position_insane drops the mantle silently": mut("move", "            if let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = pl.mantle_abort(AbortWhy::Insane) {",
                                                               "            if let Some(SimEvent::MantleAbort { t_frac, pos, .. }) = None::<SimEvent> {"),
            "11 no rise head ray": mut("sim", "mm.dir * (cfg.radius + 0.5 * cfg.skin)", "mm.dir * 0.0"),
            "11 no deferral": mut("sim", "mantle::SUBSTEP_RAYS * left", "0"),
            "1 detection on the main budget": mut("sim", "mantle::find_ledge(mb, w,", "mantle::find_ledge(b2, w,"),
            "2 feet moved before the check": mut("sim", "        let prev = self.pos;\n        let t1 = mm.t + h as f64;",
                                                 "        let prev = self.pos;\n        self.pos = mm.at(mm.t).0;\n        let t1 = mm.t + h as f64;"),
            "2 land() at the mantle end": mut("sim", "            self.ground_y0 = self.pos.y;\n            self.freefall_sent = false;",
                                              "            self.ground_y0 = self.pos.y;\n            self.land(d, cfg, self.pos.y, 0.0, t, out);\n            self.freefall_sent = false;"),
            "3 release restores gravity before the abort": {**src, "move": src["move"].replace(
                "        cam_hook::unpublish_feet();\n        self.restore_gravity(in_world);\n        self.last_release = Some(reason);",
                "        cam_hook::unpublish_feet();\n        self.last_release = Some(reason);", 1).replace(
                "        // M8: a running mantle is aborted BEFORE the gravity restore",
                "        self.restore_gravity(in_world);\n        // M8: a running mantle is aborted BEFORE the gravity restore", 1)},
            "3 enter without the no-mantle assert": mut("move", "        debug_assert!(pl.mantle.is_none());\n        if pl.mantle.is_some() {\n            pl.mantle = None;\n        }\n", ""),
            "3 a warp keeps the mantle": mut("move", "pl.mantle_abort(AbortWhy::Warp)", "pl.mantle_abort(AbortWhy::Blocked)"),
            "3 translate forgets the path": mut("sim", "            m.translate(by);", "            let _ = m;"),
            "5 push-out applied mid-mantle": mut("move", "if mc.er_pushout && !pl.mantling() {", "if mc.er_pushout {"),
            "6 trigger ignores the gate": mut("task", "!self.mv.fire_blocked && !gate_block;", "!self.mv.fire_blocked;"),
            "6 reload ignores the gate": mut("task", "let reloads = if gate_block { 0 } else { reloads };", "let reloads = reloads;"),
            "6 lowered via the switch path": mut("task", "                self.cancel_sim_vm(why, true);",
                                                 "                self.cancel_sim_vm(why, true);\n                self.vm_events.push(VmEvent::SwitchDrop);"),
            "6 the gate not fed by the ladder": mut("task", "self.wgate.update(now, self.mv.mantle_lowered, on_ladder, raise_s)", "self.wgate.update(now, self.mv.mantle_lowered, false, raise_s)"),
            "7 AC-130 ignores the ladder": mut("acl", "            (g.ladder, Block::Ladder),\n", ""),
            "7 nuke ignores the ladder": mut("task", "            ladder: on_ladder,", "            ladder: false,"),
            "8 classify_jump with lengths": mut("move", "classify_jump(e, chunk_v,", "classify_jump(e, chunk_v.map(|c| c * 0.0),"),
            "9 ladder offset unpinned": mut("fpread", "const _: () = assert!(offset_of!(CSChrLadderModule, state) == 0x18);\n", ""),
            "10 interact flap counts every E": mut("move", "self.interact_flap_m8.tick(i.now, er_confirmed(&t), interact)", "self.interact_flap_m8.tick(i.now, false, interact)"),
        }
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(mantlewiring_problems(m)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the M8 sources caught")
    return report(problems, "mantlewiring: 11 checks (docs/M8_DESIGN.md 12.3 + review): every mantle ray through RayBudget::cast on its own "
                            "budget (sized by the substeps); position_insane aborts and logs the mantle; the rise has a head ray; "
                            "a start the tick cannot re-validate is deferred; the feet move only after the re-validation and never land() at the end; release() aborts the "
                            "mantle before the gravity restore, enter() never resumes one, a warp aborts and a re-base translates the "
                            "path; the position stays behind position_write_ok; ER's push-out is measured, not applied, mid-mantle; "
                            "fire / ADS / reload / the sim hold read the weapon gate, fed by the mantle and the ladder, with its own "
                            "viewmodel input (never SwitchDrop); the ladder feeds movement, first person, the AC-130 and the nuke; "
                            "classify_jump takes vectors; the ladder offsets are pinned and a failing fp read hands back; B3 / B4 wired")


def check_m8strings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    missing = [x.decode() for x in M8_LOG_STRINGS if x not in data]
    return report([f"missing M8 log strings: {missing}"] if missing else [], f"m8strings: all {len(M8_LOG_STRINGS)} M8 proof strings present")


# ---- C3 PASS A (docs/C3_DESIGN.md S2 checks 10 / 12) ---------------------------------------------------------------

CORE_SRC = ROOT / "mod" / "mw2er-core" / "src"
C3_FILES = {
    "move": MOD_SRC / "movement.rs", "fp": MOD_SRC / "first_person.rs", "task": MOD_SRC / "task.rs", "hud": MOD_SRC / "hud.rs",
    "vm": MOD_SRC / "viewmodel.rs", "probe": MOD_SRC / "ui_probe.rs", "boot": MOD_SRC / "boot.rs", "c3read": MOD_SRC / "game" / "c3_read.rs",
    "layers": CORE_SRC / "collision_layers.rs", "config": CORE_SRC / "config.rs", "mlogic": CORE_SRC / "move_logic.rs",
    "fplatch": CORE_SRC / "fp_latch.rs",
    # PASS B
    "pad": MOD_SRC / "pad.rs", "inhook": MOD_SRC / "input_hook.rs", "padlogic": CORE_SRC / "pad_logic.rs",
    "minput": CORE_SRC / "move_input.rs",
    # PASS C
    "spawn": MOD_SRC / "spawn.rs", "ac130": MOD_SRC / "ac130.rs", "nuke": MOD_SRC / "nuke.rs", "coopvis": MOD_SRC / "coop_vis.rs",
    "corevis": CORE_SRC / "coop_vis.rs", "targets": CORE_SRC / "nuke_targets.rs", "render": ROOT / "mod" / "mw2er-render" / "src" / "lib.rs",
    "occl": ROOT / "mod" / "mw2er-render" / "src" / "occluder.rs",
    "breg": ROOT / "tools" / "scripts" / "build_regulation.py",
    # C6 aim 4 (rule 36)
    "stance": MOD_SRC / "er_stance.rs",
    # C6 depth 5 (rule 37)
    "erd": ROOT / "mod" / "mw2er-render" / "src" / "er_depth.rs", "dstate": MOD_SRC / "er_depth_state.rs",
    # C6 round 6 (rules 38 / 39)
    "aimcam": MOD_SRC / "aim_cam.rs", "coreaim": CORE_SRC / "aim_cam.rs", "corestance": CORE_SRC / "er_stance.rs", "lib": MOD_SRC / "lib.rs",
    # C6 round 7 (rule 38: the snipe fade read-only)
    "chars": MOD_SRC / "game" / "chars.rs",
    # C6 part 16 (rule 40: the real crossbow carrier)
    "carrier": MOD_SRC / "carrier.rs", "equip": MOD_SRC / "game" / "equip.rs", "corecarrier": CORE_SRC / "carrier.rs",
    "uiinject": MOD_SRC / "ui_inject.rs", "uiprobe": MOD_SRC / "ui_probe.rs",
    # review 2026-10-24 (rule 43: tracers behind walls)
    "tocc": CORE_SRC / "tracer_occl.rs",
    # review 2026-10-24 (rule 44: a peer's nuke marker on a dead / loading receiver)
    "nukelogic": CORE_SRC / "nuke_logic.rs",
}
C3_LOG_STRINGS = [b"filter live phys338=", b"filter table row=0x", b"blocked wish=", b"assist n=", b"WARN assist capped n=",
                  b"toggle F6 override reason=", b"closed", b"yield on reason=", b"yield off dur=", b"WARN yield signal stuck bit=",
                  b"dmgnum cfg on=", b"DMGNUM", b"probe scenes/s=", b"probe hook ok fn=", b"probe off ([ui] probe_scaleform = false)",
                  b"yield draw frac=", b"learn idx=", b"dmgnum_font=",
                  # PASS B (docs/C3_DESIGN.md 4.7)
                  b"hook ok ", b"xinput1_4=0x", b" first_byte=0x", b" overlay=GameOverlayRenderer64:", b"stat calls=",
                  b" look_counts=", b"calib deg_per_count=", b"look route fallback=er why=mouse_stale",
                  b"none seen in 60s (enable Steam Input for PlayStation pads)", b"interact hold n=", b"cfg enabled=",
                  b"off ([gamepad] enabled = false at boot; nothing hooked)",
                  # PASS C (docs/C3_DESIGN.md 3.9)
                  b"vis cfg twins=", b"twin sent gun=", b"twin seen peer=", b"peer fx muzzle=", b"marker sent sp=",
                  b"marker seen peer=", b"marker removed sp=", b"worldgun peer=", b"worldguns loaded guns=",
                  b"worldgun first frame drawn n=", b"FAIL worldgun record", b"peer_drop chr=c", b"marker from=",
                  b" action=start peers=", b"coop kills local=", b"skip reason=await_host pass=", b"coop window end passes=",
                  b"coop fallback pass=", b" held_before=",
                  b" bullet=", b" lethal_unsynced=1", b" upd={", b" fp_anim=", b" nuke_marker=", b"WARN rows ",
                  b"nuke marker bullet from=",
                  # review C3
                  b"worldgun skip peer=", b" reason=other_frame", b"peer fx skipped other_frame=", b"bone probe t=",
                  b"bone probe bones=", b"worldgun bone=", b" why=panic", b"toggle F6 deferred reason=", b" src=remote_nuke n=",
                  b" melee_synth=", b" contended=", b" base2e8=", b" vis=", b" flashes=",
                  # review 2026-10-24 (tracers behind walls)
                  b"tracer occl draws shown=", b" budget_out=", b" streaks_hidden=", b" tracer_occl=", b" tracer_rays=",
                  # review 2026-10-24 (rule 44)
                  b" spectate=", b" action=pending why=loading until_s=", b" action=ignored why=expired", b"via=repend"]


def c3wiring_problems(src):
    """src: {key: raw text}. Returns the problems of the PASS A wiring rules."""
    p = []
    s = {k: strip_comments(v) for k, v in src.items()}
    drive = fn_body(s["move"], "drive") or ""
    # 1 every controller cast uses the player-mode filter (the live layer)
    if "mc.rays.filters_with(self.player_cast)" not in drive or "HavokRays { world: phys, player_ins: r.player, w: r.pos_w, filters, n_filters" not in drive:
        p.append("1 the controller's rays do not cast with filters_with(self.player_cast)")
    if re.search(r"\.rays\.filters\(\)", drive):
        p.append("1 drive() casts with filters() (the fallback), not the live player layer")
    # 2 the player cast is the LAYER only (bits 0-6) of an accepted live word
    pc = fn_body(s["layers"], "player_cast") or ""
    if "(layer_of(v) as u32, CastSrc::Live)" not in pc or "accepted.contains(&layer_of(v))" not in pc:
        p.append("2 player_cast does not return the layer only of an accepted word")
    # 3 nothing writes ER's collision filter: no write in c3_read.rs, the physics filter words never written in movement.rs
    if re.search(r"\bwrite(_exe_bool)?\s*\(", s["c3read"]) or re.search(r"write\([^;]*PHYS_FILTER", s["move"]):
        p.append("3 a write into the collision filter / the physics filter words")
    # 4 the talk latch has its own params and the dialog index as its menu input
    fpt = fn_body(s["fp"], "tick") or ""
    if "fp_latch::talk_params(" not in fpt or not re.search(r"latch_tick\(&mut self\.talk_latch,[^;]*talk_menu, i\.now, lp_talk\)", fpt):
        p.append("4 the talk latch does not use talk_params / the dialog index")
    if not re.search(r"latch_tick\(&mut self\.fade_latch,[^;]*lp_fade\)", fpt):
        p.append("4 the fade latch does not use fade_params")
    # 5 F6 override keeps wanted = 1
    f6 = fn_body(s["fp"], "handle_f6") or ""
    ov = re.search(r"F6Action::Override\(r\) => \{(.*?)\}", f6, re.S)
    if "fp_latch::f6_action(" not in f6 or not ov or "self.wanted = true;" not in ov.group(1) or "self.wanted = false" in ov.group(1) or "release_latches()" not in ov.group(1):
        p.append("5 F6 during a non-user hold does not release the latches and keep wanted = 1")
    # 6 movement's talk reason follows the FP talk latch
    if "TALK_LATCHED.store(self.talk_latch.is_latched()" not in s["fp"] or not re.search(r"t\.set\(Reason::Talk,[^;]*\|\| fp_talk\)", s["move"]):
        p.append("6 movement's talk hold does not follow first person's talk latch")
    # 7 the UI yield is applied in the viewmodel offset and the HUD alpha, render-only
    vm = s["vm"]
    if "ui_yield::vm_offset(" not in vm or "+ drop[k]" not in vm:
        p.append("7 the viewmodel does not apply the UI yield offset")
    if "ui_yield::hud_alpha(" not in s["hud"]:
        p.append("7 the HUD does not dim with the UI yield")
    tick = fn_body(s["task"], "tick") or ""
    for line in tick.splitlines():
        if re.search(r"\b(let mut trigger|let can_fire|let fire_raw|let ads_raw)\b", line) and "ui_yield" in line:
            p.append("7 the UI yield gates a shot (it must be render-only)")
    # 8 one background draw list; the damage numbers after the hitmarker
    if s["hud"].count("get_background_draw_list()") != 1:
        p.append("8 hud.rs takes more than one background draw list")
    draw = fn_body(s["hud"], "draw") or ""
    hm, dn = draw.find("hitmarker(w, h, now - s.last_hit_t"), draw.find("dmgnum::style(")
    if hm < 0 or dn < 0 or dn < hm:
        p.append("8 the damage numbers are not drawn after the hitmarker")
    # 9 damage numbers only for our attributed hits (never from the HPDROP branch)
    ph = fn_body(s["task"], "poll_hp") or ""
    hp_arm = ph[ph.find("None => log_i!("):] if "None => log_i!(" in ph else ""
    if "self.on_hit(" not in ph or "on_hit" in hp_arm[:200]:
        p.append("9 damage numbers are pushed outside our attributed hits")
    if ph.count("self.on_hit(") != 5:
        p.append(f"9 on_hit at {ph.count('self.on_hit(')} sites (want the 5 hit sites)")
    # 10 the Scaleform probe: off by default, boot-gated, prologue-checked, no write
    # C4 added inject_viewmodel / inject_ui_layer, C4.1 P2 probe_layer_sample, C4.1 I41 inject_ui_thread: every [ui] field
    # off in the Default impl
    # itself (not the test's copy)
    if not re.search(r"impl Default for UiCfg \{[^}]*UiCfg \{ probe_scaleform: false, inject_viewmodel: false, inject_ui_layer: false, probe_layer_sample: false, inject_ui_thread: false \}", s["config"]):
        p.append("10 [ui] probe_scaleform is not off by default")
    if not re.search(r"if cfg\.ui\.probe_scaleform \{[^}]*ui_probe::install", s["boot"]):
        p.append("10 the probe is not installed only with [ui] probe_scaleform")
    if "if got != prologue" not in (fn_body(s["probe"], "hook") or ""):
        p.append("10 the probe hooks without the prologue check")
    if re.search(r"\bwrite(_exe_bool)?\s*\(", s["probe"]) or "MH_CreateHook" in s["probe"] and "prologue" not in s["probe"]:
        p.append("10 the probe writes game memory")
    # 11 the assist goes through the existing suspend path and is capped
    if not re.search(r"\| Reason::Unstick\s*\| Reason::Assist\s*\| Reason::Freefall", s["mlogic"]):
        p.append("11 Reason::Assist is not a Suspend reason")
    if "self.assist.tick(&ai, &mc.assist.params())" not in drive or "AssistEvent::Capped" not in drive:
        p.append("11 the assist detector is not ticked with the config params / its cap is not handled")
    if "pad" in src:
        p += c3padwiring_problems(src, s)
    if "coopvis" in src:
        p += c3coopwiring_problems(src, s)
    return p


def c3coopwiring_problems(src, s):
    """PASS C (docs/C3_DESIGN.md S6): co-op visibility and the co-op nuke."""
    p = []
    sp = s["spawn"]
    # 21 the damaging spawn never has bit 3; only twins / the nuke marker are synced, refused for any other id
    if "pub const SPAWN_FLAGS: u32 = 1;" not in sp or "pub const SPAWN_FLAGS_SYNCED: u32 = 0x9;" not in sp:
        p.append("21 the spawn flags changed (damaging 1, synced 0x9)")
    sds = fn_body(sp, "spawn_data_synced") or ""
    if "if !mw2er_core::coop_vis::syncable_bullet(bullet_id) {" not in sds or sds.find("syncable_bullet") > sds.find("d.unk44 = SPAWN_FLAGS_SYNCED;"):
        p.append("21 a synced spawn without the twin / marker id check first")
    if sp.count("SPAWN_FLAGS_SYNCED;") != 1 or "unk44 = SPAWN_FLAGS_SYNCED" in (fn_body(sp, "spawn_data") or ""):
        p.append("21 bit 3 set outside spawn_data_synced")
    for k in ("task", "ac130", "move", "fp"):
        if "spawn_synced(" in s[k]:
            p.append(f"21 {k} spawns synced bullets (only coop_vis.rs twins and the nuke marker may)")
    nk = s["nuke"]
    if nk.count("spawn::spawn_synced(") != 1 or "let id = mw2er_core::coop_vis::NUKE_MARKER_BULLET;" not in nk:
        p.append("21 the nuke syncs something other than its marker bullet")
    # 22 apply_speffect keeps dont_sync = true; the synced variant only for the C3 markers
    ac = s["ac130"]
    if "c.apply_speffect(id, true);" not in (fn_body(ac, "apply_speffect") or ""):
        p.append("22 apply_speffect no longer passes dont_sync = true")
    syn = fn_body(ac, "apply_speffect_synced") or ""
    if "mw2er_core::coop_vis::syncable_speffect(id)" not in syn or "c.apply_speffect(id, false);" not in syn:
        p.append("22 the synced SpEffect wrapper does not check the marker ids")
    if "apply_speffect_synced(" in s["nuke"] or "apply_speffect_synced(" in s["task"]:
        p.append("22 a synced SpEffect outside the co-op marker controller")
    sse = fn_body(s["corevis"], "syncable_speffect") or ""
    if "NUKE_LETHAL_SP" in sse or "79" in sse:
        p.append("22 the synced-SpEffect id rule names the lethal effect / raw ids")
    # 23 (review 2026-10-24) the co-op kill rule: chr_update_type is logged, never a kill decision; the host kills
    # first, a client holds until its fallback (then kills every enemy still alive); each PC only its own player
    cl = fn_body(s["targets"], "classify") or ""
    if "if cfg.hold {" not in cl or "Owner::" in cl:
        p.append("23 the target rule does not hold a client's kills, or decides on chr_update_type")
    ch = fn_body(s["targets"], "coop_hold") or ""
    if "CoopRole::Host => false," not in ch or "fallback_s.clamp(0.0, window_s.max(0.0))" not in ch:
        p.append("23 the hold rule: the host must never hold, a client's fallback must stay inside the window")
    if "owner: Owner::from_update_type(n.upd)," not in (fn_body(nk, "cand") or ""):
        p.append("23 the census upd histogram is not the set entry's chr_update_type")
    kp = fn_body(nk, "kill_pass") or ""
    if "TargetCfg::from_config(cfg).with_coop(" not in kp or "coop_hold(self.coop_role, since, cfg.nuke.coop_client_fallback_s, cfg.nuke.coop_window_s)" not in kp:
        p.append("23 the kill pass ignores the co-op rules (role / fallback)")
    # 24 peer effects are cosmetic: coop_vis.rs never spawns a damaging row, writes game memory or applies an unsynced SpEffect
    cv = s["coopvis"]
    if re.search(r"spawn::spawn\(|\bgame::write\(|super::write\(|write_exe_bool|ac130::apply_speffect\(", cv):
        p.append("24 coop_vis.rs reaches a damage / write path")
    # 25 the peer-drop guard before the gun HIT credit
    ph = fn_body(s["task"], "poll_hp") or ""
    g0, cont, hit = ph.find("coop_vis::peer_drop("), ph.find("continue;", ph.find("coop_vis::peer_drop(")), ph.find("DmgSrc::Gun, impact_pos, Some(mw2))")
    if g0 < 0 or cont < 0 or hit < 0 or not (g0 < cont < hit):
        p.append("25 the peer-drop guard does not skip the gun HIT credit")
    # 26 fp_anim hybrid through the one W/A/S/D rule, root motion compensated, no animation-speed write
    if "if ctx.fp { !ctx.fp_hybrid } else { !ctx.hybrid }" not in (fn_body(s["minput"], "wasd_blanked") or ""):
        p.append("26 the W/A/S/D rule ignores fp_anim")
    if "fp_hybrid: i.cfg.movement.fp_anim == mw2er_core::config::FpAnim::Hybrid," not in (fn_body(s["move"], "publish_blank") or ""):
        p.append("26 fp_anim does not reach the blank rule")
    if "let ec = rm_compensate(e, pl.last_dir, rm_max);" not in (fn_body(s["move"], "drive") or ""):
        p.append("26 ER's root motion is not compensated before it counts as a push")
    if "if self.movement.fp_anim_speed_scale {" not in s["config"]:
        p.append("26 fp_anim_speed_scale = true is accepted (no animation-speed write is built)")
    if "!crate::movement::FP_HYBRID_ACTIVE.load(Ordering::Relaxed)" not in s["fp"]:
        p.append("26 the yaw gate does not pause for A/S/D with fp_anim hybrid")
    # 27 twins only with a peer, through the watchdog's lowest priority
    st = fn_body(cv, "send_twin") or ""
    if "twin_wanted(peers, cfg.coop.visual_twins, src_rid)" not in st or "SpawnKind::Twin" not in st:
        p.append("27 twins are sent without the peer / config rule or the watchdog")
    if "if peers == 0 || !enabled {" not in (fn_body(s["corevis"], "twin_wanted") or ""):
        p.append("27 twin_wanted sends solo")
    # 28 the regulation keeps the lethal effect unsynced and every twin without an attack
    br = src["breg"]
    if '"iconId": -1, "isDisableNetSync": 1,' not in br or 'tov["atkId_Bullet"] = -1' not in br:
        p.append("28 build_regulation: the lethal SpEffect synced or a twin with an attack")
    # 29 the world guns: our own depth (cleared), drawn before the viewmodel, never ER's depth (C6: record_world is
    # record_world_occluded with no capsules; the capsules go into that same cleared depth, after the clear)
    rw = fn_body(s["render"], "record_world_occluded") or ""
    if ("list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);" not in rw or "self.dsv_heap.GetCPUDescriptorHandleForHeapStart()" not in rw
            or "self.record_world_occluded(list, t, slot, draws, f, &[], None)" not in (fn_body(s["render"], "record_world") or "")):
        p.append("29 the world guns do not use (and clear) our own depth")
    # C6 35: the body capsules: depth-only into OUR world-gun depth after its clear, the target re-bound before the guns,
    # third person only with the flag (default false); the capsule pass binds no render target and no ER resource
    clr, cap = rw.find("list.ClearDepthStencilView(dsv"), rw.find("caps_drawn = o.record(list, slot, dsv, &f.view_proj, caps);")
    rebind = rw.find("list.OMSetRenderTargets(1, Some(&t.rtv), false, Some(&dsv));", cap)
    oc = fn_body(s["occl"], "record") or ""
    if clr < 0 or cap < 0 or not (clr < cap < rebind) or "list.OMSetRenderTargets(0, None, false, Some(&dsv));" not in oc or "ResourceBarrier" in s["occl"]:
        p.append("35 the body capsules are not a depth-only pass into our own cleared depth before the guns")
    if "if cfg.coop.world_gun_body_occlusion && !snap.fp_on { &snap.world_occl } else { &[] }" not in (fn_body(s["vm"], "draw_world_guns") or "") \
            or "world_gun_body_occlusion: false," not in s["config"]:
        p.append("35 the body capsules are not off by default / not third person only")
    cb = fn_body(s["vm"], "callback_body") or ""
    wgi, gate = cb.find("draw_world_guns(ctx"), cb.find("let g = DrawGate {")
    if wgi < 0 or gate < 0 or wgi > gate:
        p.append("29 the world guns are not drawn before the viewmodel gate")
    # 30 the co-op tick is guarded (a panic turns only it off) and runs before the nuke
    tk = fn_body(s["task"], "tick") or ""
    cvt, nkt = tk.find('guard("", || {\n                self.coopvis.tick('), tk.find("self.nuke.tick(")
    if cvt < 0 or nkt < 0 or cvt > nkt or "self.coopvis_off = true;" not in tk:
        p.append("30 the co-op visibility tick is not guarded / not before the nuke")
    # review C3 31: the world-gun pass yields to ER's messages like the viewmodel
    if "ui_yield::world_hidden(" not in cb or "&& !world_yield" not in cb:
        p.append("31 the world-gun pass ignores the UI yield")
    # review C3 32: orphan markers are removed (MarkerCtl) and the panic path strips our markers before the reset
    mt = fn_body(s["corevis"], "tick") or ""
    if "if !syncable_speffect(id) || want.contains(&id) || self.applied.iter().any(|(x, _, _)| *x == id) {" not in s["corevis"]:
        p.append("32 MarkerCtl never removes a present but untracked marker")
    if "self.coopvis.strip_markers()" not in tk or "crate::ac130::remove_speffect(me, id)" not in (fn_body(cv, "strip_markers") or ""):
        p.append("32 the co-op panic path does not remove our markers")
    if "pub const NUKE_DEDUPE_S: f64 = 30.0;" in s["corevis"]:
        p.append("32 the nuke dedupe is not longer than the marker's life")
    # review C3 33: positions of a peer in another havok frame are never used (world guns, peer effects)
    wg = fn_body(cv, "world_guns_tick") or ""
    if "if pw != Some(w) {" not in wg or wg.find("if pw != Some(w) {") > wg.find("world_gun_matrix("):
        p.append("33 a world gun of a peer in another frame is placed in ours")
    if "self.tracker.update(now, &list, local, &other, &mut ev);" not in (fn_body(cv, "tick") or "") or "let Some(pl) = i.player" not in (fn_body(cv, "tick") or ""):
        p.append("33 peer effects without the frame / local-handle rule")
    # review C3 34: the F6 override never releases the load / camera-mask latches
    rl = fn_body(s["fp"], "release_latches") or ""
    if "self.load_latch.reset()" in rl or "self.mask_latch.reset()" in rl:
        p.append("34 F6 releases the load / camera-mask latch")
    # C6 36 (aim 4, docs/C6_HOLD_DESIGN.md part 14): the third-person ADS -> ER crossbow aim: off by default and only with
    # the stance; its two fields only through game::write; a stance restore puts them back first; no chord while an aim
    # episode runs; the keyboard RMB after the raw ADS was recorded, never in the gunship; the pad synth with its gates
    if "stance" in s:
        st = s["stance"]
        # (string literals are emptied in `s`: the config rule reads the raw source)
        # C6 part 17: the crossbow aim needs the crossbow set (`stance_crossbow`)
        if 'tp_ads_pose: "off".into(),' not in src["config"] or 'self.stance_crossbow() && (self.tp_ads_pose == "er_aim" || self.tp_ads_er_aim)' not in src["config"] \
                or 'self.er_stance == "crossbow"\n' not in src["config"]:
            p.append("36 the aim pose is on by default / without the stance")
        wa = fn_body(st, "write_aim") or ""
        if "AIM_FIELDS.iter()" not in wa or "crate::game::write::<u16>(addr + f.off, v[k])" not in wa:
            p.append("36 the aim fields are not written through game::write")
        if '.or(aiming.then_some(""))' not in (fn_body(st, "refresh") or ""):
            p.append("36 a grip chord may run during an aim episode")
        rr = fn_body(st, "restore_row") or ""
        ar, rm = rr.find("self.aim_restore_at(k, why)"), rr.find("let p = self.patched.remove(k);")
        if ar < 0 or rm < 0 or ar > rm:
            p.append("36 a stance restore does not put the aim fields back first")
        ps = fn_body(s["inhook"], "process_state") or ""
        ka, ab = ps.find("crate::shared::KB_AIM.load("), ps.find("RAW_ADS_DOWN.store(adown")
        if ka < 0 or ab < 0 or ka < ab or "&& !gunship" not in ps[ka:ka + 200]:
            p.append("36 the keyboard aim button is set before the raw ADS is recorded / in the gunship")
        if "if i.er_aim && i.mw2 && !i.gunship && !i.er_prompt {" not in (fn_body(s["padlogic"], "step") or ""):
            p.append("36 the pad aim synth ignores the mode / gunship / prompt")
    # C6 37 (depth 5, docs/C6_HOLD_DESIGN.md 15.2 / 15.4): the ER-depth prime is a depth-only pass into OUR cleared depth
    # after the clear and before the capsules, third person only, only for this Present's spliced copy, skipped on a fast
    # turn; the flag is off by default and cold to turn on; the prime pass binds no render target and barriers nothing
    if "erd" in s and "dstate" in s:
        pr = rw.find("self.last_primed = pr.record(list, slot, dsv, pi);")
        if pr < 0 or clr < 0 or cap < 0 or not (clr < pr < cap):
            p.append("37 the ER-depth prime is not between our depth clear and the capsules")
        dp = s["erd"][s["erd"].find("impl DepthPrime"):]
        prec = fn_body(dp, "record") or ""
        if "list.OMSetRenderTargets(0, None, false, Some(&dsv));" not in prec or "ResourceBarrier" in prec \
                or "DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS," not in dp or "NumRenderTargets: 0," not in dp:
            p.append("37 the prime is not a depth-only pass without a render target")
        pp = fn_body(s["dstate"], "present_prime") or ""
        fp_i, pair_i = pp.find("if snap.fp_on {"), pp.find("if !eds::prime_pair(")
        if fp_i < 0 or pair_i < 0 or fp_i > pair_i or "> cfg.coop.er_depth_max_turn_deg" not in pp or "cfg.coop.world_gun_er_depth" not in pp:
            p.append("37 the prime is not third person only / not paired with this Present's copy / not skipped on a fast turn")
        dw = fn_body(s["vm"], "draw_world_guns") or ""
        if "crate::er_depth_state::present_prime(snap, cfg)" not in dw or "&draws, &wf, caps, prime.as_ref())" not in dw:
            p.append("37 the world-gun pass does not take the prime from present_prime")
        if "world_gun_er_depth: false," not in s["config"] or "probe_er_depth_state: false," not in s["config"] \
                or 'ignored.push("coop.world_gun_er_depth (turning it on)")' not in src["config"]:
            p.append("37 world_gun_er_depth is not off by default / not cold to turn on")
    # C6 38 (round 6, docs/C6_HOLD_DESIGN.md 14.8): the filtered precision-shooting getter: only with the aim pose (code
    # default off) and tp_aim_cam = keep; the naked detour writes no memory and returns 0 only for the arm + our player +
    # ER's camera / reticle call sites (every other call tail-jumps to the trampoline); the sites validated before the
    # hook is created and the DLL pinned before it is enabled; every restore disarms first; first person never arms
    if "aimcam" in s:
        ac, rac = s["aimcam"], src["aimcam"]
        if 'tp_aim_cam: "keep".into(),' not in src["config"] or 'self.ads_pose_on() && self.tp_aim_cam == "keep"' not in src["config"] \
                or 'tp_ads_pose: "off".into(),' not in src["config"]:
            p.append("38 the aim-camera filter acts without the aim pose (off by default)")
        det = fn_body(rac, "getter_detour") or ""
        if re.search(r"\bmov\s+(byte|word|dword|qword)\s+ptr\s*\[", det) or "game::write" in ac or "write_exe" in ac \
                or "jmp qword ptr [rip + {tramp}]" not in det or "cmp rcx, qword ptr [rip + {player}]" not in det \
                or "cmp rax, qword ptr [rip + {ret_cam}]" not in det or "cmp rax, qword ptr [rip + {ret_ret}]" not in det \
                or "cmp rax, qword ptr [rip + {ret_fade}]" not in det \
                or "cmp byte ptr [rip + {armed}], 0" not in det:
            p.append("38 the getter detour writes memory / does not filter on the arm, our player and the three call sites")
        # C6 round 7 (work/c6/aim7.md 4 A): the snipe-fade site is validated (33 bytes, its call, both timer stores) and,
        # with tp_aim_cam = keep, no aim episode starts before the filter is installed (an invisible body otherwise);
        # +0x5D8 is never written
        va = fn_body(src["coreaim"], "validate") or ""
        if 'return Err("fade_site");' not in va or "a.fade_stores.iter().all(|s| same(*s, &FADE_STORE_2720))" not in va \
                or "call_target(base + a.fade_call, &b) != Some(base + a.getter)" not in va:
            p.append("38 the snipe-fade site is masked without validating its bytes / call / timer stores")
        if "(!c.aim_cam_keep() || crate::aim_cam::keep_ready())" not in (fn_body(s["stance"], "aim_tick") or "") \
                or "ensure_installed() && !REFUSED.load(Ordering::Relaxed)" not in (fn_body(ac, "keep_ready") or ""):
            p.append("38 an aim episode starts with tp_aim_cam = keep although the filter is refused (an invisible body)")
        if re.search(r"SNIPE_FADE_OFF[^;]*write|write[^;]*SNIPE_FADE_OFF|snipe_mode_draw_alpha_fade_timer\s*=", s["stance"] + ac + s["chars"]):
            p.append("38 the snipe fade timer is written (ER rewrites it every frame: read-only)")
        ins = fn_body(ac, "install") or ""
        v_i, h_i, pin_i, en_i = ins.find("validate(a, base, &rd)?;"), ins.find("MhHook::new("), ins.find("if !pin_self() {"), ins.find("MH_EnableHook(")
        if min(v_i, h_i, pin_i, en_i) < 0 or not (v_i < h_i < pin_i < en_i):
            p.append("38 the getter hook is created before its sites are validated / enabled before the DLL is pinned")
        tk = fn_body(ac, "tick") or ""
        if "let keep = keep_cfg && ensure_installed();" not in tk or ac.count("ensure_installed()") != 3 \
                or "c.aim_cam_keep() && self.disabled.is_none()" not in s["stance"]:
            p.append("38 the getter hook is installed without tp_aim_cam = keep under the aim pose")
        if "i.keep && i.tp && i.in_world && !i.dead && i.player" not in (fn_body(s["coreaim"], "armed") or "") \
                or "tp: !i.fp_on && !i.ac130 && !i.nuke," not in s["stance"]:
            p.append("38 the aim-camera arm does not need third person / our player / a live episode")
        rr = fn_body(s["stance"], "restore_row") or ""
        sa, ar = rr.find("self.aimcam.stop();"), rr.find("self.aim_restore_at(k, why)")
        ra = fn_body(s["stance"], "restore_all") or ""
        di = fn_body(s["stance"], "disable") or ""
        pt = s["task"].find('crate::aim_cam::disarm();\n                let _ = guard("", || self.stance.restore_all(""));')
        if sa < 0 or ar < 0 or sa > ar or not (0 <= ra.find("self.aimcam.stop();") < ra.find("self.restore_row(")) \
                or not (0 <= di.find("self.aimcam.stop();") < di.find("self.restore_row(")) or pt < 0 or "aim_cam::disarm();" not in s["lib"]:
            p.append("38 an aim restore point does not disarm the camera filter first")
    # C6 39 (round 6, the 21:25:30 refresh fail): no grip chord while ER still spawns the player (`spawn`, fed by the
    # task's spawn_settling), and an enter chord checked after its stance went off is void (never a refresh fail)
    if "corestance" in s:
        cs = s["corestance"]
        if "} else if settling {" not in (fn_body(cs, "chord_refusal") or "") or "i.dead, i.settling)" not in (fn_body(s["stance"], "refuse_now") or "") \
                or not ("settling: mw2er_core::er_stance::spawn_settling(" in s["task"]
                        or ("let st_settling = mw2er_core::er_stance::spawn_settling(" in s["task"] and "settling: st_settling," in s["task"])):
            p.append("39 grip chords are sent while ER still spawns the player")
        if "if dead_seen || (!exit && stance_left) {" not in (fn_body(cs, "chord_verdict") or "") \
                or "chord_verdict(took, f.dead_seen || i.dead, f.exit, !self.hyst.on, self.chord_fails)" not in (fn_body(s["stance"], "refresh") or ""):
            p.append("39 an enter chord checked after its stance went off can still disable the stance")
    # C6 40 (part 16, docs/C6_HOLD_DESIGN.md 16.3-16.5 / 16.9): the real crossbow carrier. ER's EquipItem / AddItem /
    # Create / Find only from carrier.rs, ticked by the task (the game thread: never Present, a detour or the render
    # thread); the journal is written before the swap's EquipItem; every call only after the pins were validated; no
    # ChrAsm::SetSlot address and no game::write in the carrier / equip reads (the grip probe's arm_style is the only
    # write, in er_stance.rs); R3 melee blocked by the carrier itself while its crossbow is in the slot; the chord's RB
    # only on an unloaded crossbow; off by default and only with the stance. 16.9: no chord while ER is busy, an
    # ignored chord never counts.
    if "carrier" in s and "equip" in s:
        cr, rcr = s["carrier"], src["carrier"]
        for k in ("vm", "hud", "uiinject", "uiprobe", "dstate", "inhook", "pad", "coopvis", "aimcam"):
            if k in s and re.search(r"crate::carrier::(?!CarrierView)|\bcall_equip\(|EquipItemFn|\.equip_item\b", s[k]):
                p.append(f"40 {k} reaches the carrier's ER calls (only the task's carrier tick may)")
        tk = fn_body(s["task"], "tick") or ""
        if "self.carrier.tick(&ci)" not in tk or s["task"].count("self.carrier.tick(") != 1:
            p.append("40 the carrier is not ticked once from the task's tick (the game thread)")
        for name in ("EquipItemFn", "AddItemFn", "CreateFn", "FindFn"):
            if cr.count(f"transmute::<usize, {name}>") != (1 if name != "EquipItemFn" else 1):
                p.append(f"40 {name} is transmuted at an unexpected number of sites")
        sw = fn_body(cr, "swap") or ""
        wj, ce, ad = sw.find("if !write_journal(&j) {"), sw.find("call_equip("), sw.find("self.addrs()")
        if wj < 0 or ce < 0 or ad < 0 or not (ad < wj < ce):
            p.append("40 the swap's EquipItem is not preceded by the pins and the journal write")
        rs = fn_body(cr, "restore") or ""
        if rs.find("self.addrs()") < 0 or rs.find("self.addrs()") > rs.find("call_equip("):
            p.append("40 the restore's EquipItem without the pins")
        if "validate(&a, base, &rd)" not in (fn_body(cr, "addrs") or ""):
            p.append("40 the carrier calls ER without validating the pinned bytes")
        if re.search(r"3_?bf_?3d0|CHR_ASM_SET_SLOT|SetSlot\(", cr + s["equip"] + s["stance"]):
            p.append("40 ChrAsm::SetSlot is reachable from the carrier (only EquipItem may change the slot)")
        # 16.16: the one write of the carrier is `write_bolt_loaded` (the slot's bolt_loaded_states byte, 0 / 1, on
        # EquipGameData's + PlayerIns's ChrAsm); nothing else in carrier.rs / equip.rs writes
        wb = fn_body(cr, "write_bolt_loaded") or ""
        if re.search(r"\bgame::write\b|\bwrite::<|write_exe|super::write\(", cr.replace(wb, "") + s["equip"]):
            p.append("40 the carrier / equip reads write game memory (the slot changes only through ER's EquipItem)")
        if wb.count("crate::game::write::<u8>(") != 2 or wb.count("equip::ASM_BOLT_LOADED + s, v)") != 2 or "if s >= 12 || v > 1 {" not in wb:
            p.append("40 write_bolt_loaded writes something other than the slot's bolt_loaded_states byte (16.16)")
        lw, lu = fn_body(cr, "load_write") or "", fn_body(cr, "load_undo") or ""
        if cr.count("write_bolt_loaded(") != 3 or "write_bolt_loaded(r.egd, i.player, s, 1)" not in lw or "write_bolt_loaded(r.egd, i.player, s, 0)" not in lu \
                or "r.handles[s] != h || pl_slot.is_none_or(|x| x.1 != h)" not in lw:
            p.append("40 the load is written outside load_write / load_undo or without the slot holding our crossbow (16.16)")
        ti, rsb = fn_body(cr, "tick_inner") or "", fn_body(cr, "restore") or ""
        lt, pl_ = ti.find("self.load_tick(i, &r);"), ti.find("let step = plan(&PlanIn {")
        if lt < 0 or pl_ < 0 or lt > pl_ or rsb.find("self.load_undo(i, r, why);") < 0 or rsb.find("self.load_undo(i, r, why);") > rsb.find("call_equip("):
            p.append("40 the load is not undone before the restore's EquipItem (16.16)")
        ls = fn_body(s["corecarrier"], "load_step") or ""
        if 'Some(false) if i.chord_keys => Some(""),' not in ls or 'Some(false) if i.arm != 3 => Some(""),' not in ls \
                or 'None if i.arm != 3 => LoadStep::Undo(""),' not in ls or 'None if i.chord_keys => LoadStep::Undo(""),' not in ls \
                or "let why_not = if !i.flag {" not in ls:
            p.append("40 the load may run without the flag, off the two-handed grip or inside a chord (16.16)")
        if "carrier_load: false," not in s["config"] or "load: cfg.coop.carrier_load," not in s["task"] \
                or "flag: i.load && i.mode == Mode::Crossbow," not in (fn_body(cr, "load_tick") or ""):
            p.append("40 the loaded crossbow is on by default or without the carrier (16.16)")
        cg = fn_body(s["stance"], "carrier_grip") or ""
        if cg.count("crate::game::write::<u32>(") != 1 or "arm.asm + ASM_ARM_STYLE, ARM_RIGHT_BOTH" not in cg:
            p.append("40 the carrier's grip writes something other than arm_style")
        if "crate::pad::set_carrier_melee_off(self.equipped());" not in (fn_body(cr, "tick") or "") \
                or "WANT_MELEE_OFF.load(Ordering::Relaxed) || WANT_MELEE_OFF_CARRIER.load(Ordering::Relaxed)" not in s["pad"]:
            p.append("40 R3 melee is not blocked while the carrier's crossbow is in the slot")
        rf = fn_body(s["stance"], "refresh") or ""
        if ".or(loaded.then_some(\"\"))" not in rf or "chord_rb_allowed(cv.loaded)" not in rf:
            p.append("40 the chord's RB may reach a loaded crossbow")
        if 'er_carrier: "off".into(),' not in src["config"] or "carrier_give: false," not in s["config"] \
                or "if self.stance_crossbow() { crate::carrier::Mode::of(&self.er_carrier) } else { crate::carrier::Mode::Off }" not in s["config"]:
            p.append("40 the carrier is on by default / without the stance")
        if ".or(self.er_busy_now.then_some(\"\"))" not in rf:
            p.append("40 a grip chord may be sent while ER is busy (16.9)")
        ig, cv_ = rf.find("if chord_ignored(took, played) && !(f.dead_seen || i.dead) {"), rf.find("chord_verdict(took, f.dead_seen || i.dead")
        if ig < 0 or cv_ < 0 or ig > cv_:
            p.append("40 an ignored chord (no grip-switch animation) is counted as a failure (16.9)")
        # part 16 addendum (the 10:00:35 swap test): an aborted chord ER had already played is finished (the keyboard keys
        # released first, its verdict follows); a refusal ends with `chord done` once the grip is the target; the aim
        # episode line only reads (no game write, called from the stance tick)
        kb_ab, kb_rel, kb_fin = rf.find("if let Some(why) = refuse_now(i, self.vy) {"), rf.find("KB_CHORD_T0.store(0"), rf.find("if abort_finishes(f.played) {")
        if "3 | 4 if abort_finishes(f.played) =>" not in rf or kb_fin < 0 or not (0 <= kb_ab < kb_fin) or rf.find("KB_CHORD_T0.store(0", kb_ab) > kb_fin or kb_rel < 0:
            p.append("40 an aborted chord ER had already played is refused (the 10:00:35 15 s refusal) or keeps its keys")
        if "if let Some(why) = self.refused_logged.take() {" not in rf:
            p.append("40 a chord refusal never ends when the grip reaches its target (`chord done`)")
        ep = fn_body(s["stance"], "episode_tick") or ""
        if not ep or "self.episode_tick(i, arm, own_row, prec_now, c.aim_cam_keep());" not in (fn_body(s["stance"], "tick") or "") \
                or re.search(r"\bgame::write\b|\bwrite::<|write_exe|write_aim\(|write_vals\(", ep):
            p.append("40 the aim episode line is missing from the stance tick or writes game memory")
    # C6 41 (part 17, docs/C6_HOLD_DESIGN.md 17): the polearm stance + the third-person guard ADS. The guard only with
    # er_stance = "polearm" and tp_ads_pose = "guard" (code default off), its yaw only with tp_guard_yaw (code default
    # false) inside the guard; the guard writes nothing on the row and never touches the aim-camera filter (no aim fields,
    # no ER aim camera / reticle / fade mask); it uses the aim pose's gates (third person, not a scope peek, no killstreak
    # / menu / prompt / mount / air, a fresh input, the polearm set posed); no grip chord while it holds the block input;
    # every stance stop (restore_all / disable / our row's restore) ends it; the yaw is one game::write of the physics
    # module's orientation pair behind its gate; a row is restored against the set it was written with.
    if "stance" in s and "corestance" in s:
        st, rcfg = s["stance"], src["config"]
        if "tp_guard_yaw: false," not in rcfg or 'self.er_stance == "polearm" && self.tp_ads_pose == "guard"' not in rcfg \
                or "self.guard_pose_on() && self.tp_guard_yaw" not in rcfg:
            p.append("41 the guard / its body yaw is on by default or without the polearm stance")
        gt, gf, gy = fn_body(st, "guard_tick") or "", fn_body(st, "guard_frame") or "", fn_body(st, "guard_yaw") or ""
        if not gt or re.search(r"write_vals\(|write_aim\(|\bgame::write\b|\bwrite::<|write_exe|aim_write\(", gt + gf):
            p.append("41 the guard writes game memory outside its yaw (nothing is written on the row)")
        if "aimcam" in gt + gf + gy or "crate::aim_cam" in gt + gf + gy or "aim_cam_keep" in gt + gf + gy:
            p.append("41 the guard touches ER's aim-camera / reticle / fade filter")
        if "let wanted = aim_wanted(&ai);" not in gt or "let flag = c.guard_pose_on() && self.disabled.is_none();" not in gt \
                or "posed: self.posed() && !self.cycle && own_k.is_some_and(|k| self.patched[k].set == StanceSet::Polearm)," not in gt \
                or "scoped: i.scoped," not in gt or "tp: !i.fp_on," not in gt or "input: route.is_some()," not in gt:
            p.append("41 the guard ignores the aim pose's gates (first person / scope peek / killstreaks / menus / the polearm set)")
        if "|| self.guard.active();" not in (fn_body(st, "refresh") or ""):
            p.append("41 a grip chord may run during a guard episode")
        ra, di, rr = fn_body(st, "restore_all") or "", fn_body(st, "disable") or "", fn_body(st, "restore_row") or ""
        if "self.guard_stop(0.0, why);" not in ra or "self.guard_stop(0.0, why);" not in di or "self.guard_stop(0.0, why);" not in rr \
                or 'self.guard_stop(i.now, "");' not in (fn_body(st, "tick") or ""):
            p.append("41 a stance stop does not end the guard")
        if gy.count("crate::game::write(") != 1 or "crate::game::write(p.physics + crate::game::fp_read::ORIENTATION, yaw_pair(" not in gy \
                or "if !c.guard_yaw_on() {" not in gy or "if !guard_yaw_wanted(&gi) {" not in gy or "let yaw = wanted && self.guard_yaw(" not in gt:
            p.append("41 the guard's body yaw is written outside its gate / to another field")
        pr = fn_body(st, "patch_row") or ""
        if "classify_set(&c, &p.orig, &p.set.vals())" not in rr or "let vals = set.vals();" not in pr or "write_vals(addr, &vals)" not in pr:
            p.append("41 a row is written / restored against another motion set")
        # part 17 review: the guard's carry only on ER's settled idle (not its guard lower), the weapon-frame detection
        # only standing in an episode started standing, the align clamp debounced and un-latching (hold::GuardClamp)
        if "if guard_carry_ok(standing, self.posed(), self.head_now, self.er_busy_now, i.now - self.guard_end_t) {" not in gf \
                or "if !(self.guard_frame_ok && standing) {" not in gf or "self.guard_frame_ok = standing;" not in gt:
            p.append("41 the guard's carry / weapon-frame detection is not gated on ER's settled idle and standing")
        cvs = s.get("coopvis", "")
        if "hs.guard_clamp.step(now, i.stance_guard_align, guard_over.map(|g| g.1))" not in cvs or "hs.guard_clamped = true" in cvs:
            p.append("41 the guard's align clamp latches on one frame (not hold::GuardClamp)")
    # C6 42 (part 17 addendum, docs/C6_HOLD_DESIGN.md 17.12): the polearm carry grips only with the polearm stance
    # (`polearm_grip`), the guard frame only with the guard (`guard_axes` not off) and only in a DETECTED guard on a
    # hand-path gun (never stage A, never first person: the self block's `!i.fp_on`), before the aim align (the clamp /
    # raise still judges the corrected gun), learned only from settled guards held standing; it is drawing only (no game
    # write); ER's guard lower (xx0191xx) is no guard animation.
    if "coopvis" in s and "corestance" in s:
        rcfg, cvs, cs = src["config"], s["coopvis"], s["corestance"]
        if 'self.er_stance == "polearm" && self.polearm_grip' not in rcfg or 'self.guard_pose_on() && self.guard_axes != "off"' not in rcfg:
            p.append("42 the polearm grips / the guard frame act without the polearm stance / the guard")
        if "if c.polearm_grip_on() { c.weapon_grip_polearm(gun) } else { c.weapon_grip_for(gun) }" not in (fn_body(cvs, "grips") or ""):
            p.append("42 the polearm stance does not draw its own carry grips")
        a0, a1 = cvs.find("hs.guard_corr = None;"), cvs.find("let guard_cfg = c.guard_pose_on();")
        al, tp = cvs.find("mw2er_core::hold::aim_align(&m, pivot, target, hs.aim_u, align_max)"), cvs.find("&& !i.fp_on\n")
        seg = cvs[a0:a1] if 0 <= a0 < a1 else ""
        if not seg or not (0 <= tp < a0 < a1 < al):
            p.append("42 the guard frame is not in the third-person self block before the aim align")
        if "let gframe = if c.guard_frame_on() && i.stance_guard_align {" not in seg or "gframe.is_some() && f.weapon.is_some() && hand_drawn" not in seg \
                or "&& hand_drawn\n" not in seg:
            p.append("42 the guard frame is drawn outside a detected guard / on a stage-A gun")
        lo = seg.find("hs.guard_learn.observe(")
        if lo < 0 or any(g not in seg[:lo] for g in ('c.guard_axes == ""', "&& i.stance_guard_align\n", "&& i.stance_guard_standing\n", "&& hs.guard_clamp.settled(now)\n")) \
                or '&& c.guard_axes == "auto"\n' not in src["coopvis"]:
            p.append("42 the guard frame learns outside settled, detected guards held standing")
        if re.search(r"\bgame::write|write_vals\(|write_exe|\.write\(", seg):
            p.append("42 the guard frame writes game memory")
        if "17_000..18_000 | 19_000..19_100 | 19_200..20_000" not in (fn_body(cs, "guard_anim") or ""):
            p.append("42 ER's guard lower (xx0191xx) counts as a guard animation")
    # review 2026-10-24 43: the HUD tracers (ImGui, no depth test) are clipped / hidden by map rays: the peers' tracers
    # through tracer_occl from the co-op tick (after the tracker, budgeted, rays only with a physics world), the head dot
    # only on the piece that ends at the bullet, an uncast point / no rays draws as before, the flashes and our launcher
    # streaks behind a wall are not drawn, on by default
    if "tocc" in s:
        cvs, to, hd = s["coopvis"], s["tocc"], src["hud"]
        tk = fn_body(cvs, "tick") or ""
        tu, ot = tk.find("self.tracker.update(now, &list, local, &other, &mut ev);"), tk.find("self.occlusion_tick(now, i, pl);")
        if tu < 0 or ot < tu:
            p.append("43 the tracer occlusion does not run after the tracker in the co-op tick")
        oc = fn_body(cvs, "occlusion_tick") or ""
        if "self.tocc.update(i.view_pos, &segs, c.tracer_occlusion_rays, CLIP_RANGE_M, if rays_ok { Some(&mut ray) } else { None });" not in oc                 or "let rays_ok = ins != 0 && havok::phys_world() != 0;" not in oc or "havok::map_ray(o, w, d, r, ins)" not in oc:
            p.append("43 the tracer rays are not budgeted / not ER's map rays / cast without a physics world")
        sg = fn_body(cvs, "segments") or ""
        if "let pc = self.tocc.pieces(&PathIn { key: s.key, a: s.a, b: s.b, dir: s.dir });" not in sg or "if !cfg.coop.tracer_occlusion {" not in sg:
            p.append("43 the HUD tracers are not the occlusion's pieces (or ignore the flag)")
        if "if *tip {" not in hd or "for (a, b, alpha, tip) in &s.peer_segs {" not in hd:
            p.append("43 the head dot is drawn on every piece")
        if "p.vis[k].is_none_or(|(x, _)| x)" not in (fn_body(to, "pieces") or "") or "ray(cam, dir, len).is_none_or(|t| t >= len - tol)" not in to:
            p.append("43 an uncast point or a miss hides a tracer")
        if (fn_body(to, "update") or "").count("if left == 0 {") != 2:
            p.append("43 the tracer rays ignore the per-tick budget")
        if "filter(|f| !(cfg.coop.tracer_occlusion && f.3 == Some(false)))" not in (fn_body(cvs, "flashes") or ""):
            p.append("43 a peer flash behind a wall is drawn")
        pb = fn_body(s["task"], "publish") or ""
        if "let streak_rays = self.cfg.coop.tracer_occlusion && cam_pos != [0.0; 3] && havok::phys_world() != 0;" not in pb                 or "mw2er_core::tracer_occl::point_visible(streak_eye, p.pos," not in pb:
            p.append("43 our launcher streaks are not occluded")
        if "tracer_occlusion: true," not in src["config"]:
            p.append("43 the tracer occlusion is off by default")
        # the sample span follows the bullet, capped at the clip (never the 400 m clip distance for a 20 m tracer)
        if "Clip::Hit(h) => grown.min(h.max(CLIP_BACK_M))," not in (fn_body(to, "span_for") or "") or (fn_body(to, "update") or "").count("span_for(p.clip, ") != 2:
            p.append("43 the tracer's sample span is not sized from the bullet's travel (capped at the clip)")
    # review 2026-10-24 44: a peer's nuke marker on a dead receiver starts a spectating sequence (its kill passes run, no
    # player death / screen effects), on a loading receiver it pends until the first tick in the world inside its window
    if "nukelogic" in s:
        nl, nk = s["nukelogic"], s["nuke"]
        rm = fn_body(nl, "remote_marker") or ""
        if "RemoteAct::Start { spectate: !r.alive }" not in rm or "} else if r.loading {\n        RemoteAct::Pend" not in rm or "alive" in rm.replace("spectate: !r.alive", ""):
            p.append("44 a dead / loading receiver drops a peer's nuke marker")
        tk = fn_body(nk, "tick_inner") or ""
        if "match remote_marker(&r) {" not in tk or 'Some("dead")' in tk or "self.pending_remote = Some(PendingMarker { from, via, until });" not in tk                 or "match p.poll(now, loading_now) {" not in tk:
            p.append("44 the nuke tick does not use remote_marker / keep a pending marker")
        st = fn_body(nk, "start") or ""
        if "let plan = if spectate { plan.spectating() } else { plan };" not in st or "self.player_dies = false;" not in (fn_body(nl, "spectating") or ""):
            p.append("44 a spectating sequence kills the (dead) local player")
        if "&& !self.spectate" not in (fn_body(nk, "hud") or "") or "!self.player_dead && !self.spectate" not in (fn_body(nk, "step") or ""):
            p.append("44 a spectating sequence draws the screen effects / shake")
        if 'self.pending_remote = Some(PendingMarker { from, via: "repend", until: self.remote_until });' not in (fn_body(src["nuke"], "tick_inner") or ""):
            p.append("44 a loading screen before the detonation drops a peer's sequence instead of re-pending it")
    return p


PAD_FORBIDDEN_IN_DETOUR = ("log_", "format!", "Vec", "String", "Box::", "lock(", ".to_string()", "println!", ".unwrap()",
                           ".expect(", "panic!", "vec!", "config()", "try_lock", "Mutex")


def c3padwiring_problems(src, s):
    """PASS B (docs/C3_DESIGN.md S6): the XInput detour rules."""
    p = []
    raw_pad = src["pad"]
    # 12 the system DLL's export, never ER's IAT / code, never a game-memory write
    if 'GetProcAddress(module, s!("XInputGetState"))' not in raw_pad or 'GetModuleHandleW(w!("xinput1_4.dll"))' not in raw_pad:
        p.append("12 the hook target is not xinput1_4!XInputGetState by GetProcAddress")
    if re.search(r"VirtualProtect|IMAGE_IMPORT|IMAGE_THUNK|\bgame::write|super::write|crate::game::write|write_exe", s["pad"]):
        p.append("12 pad.rs patches an import table or writes game memory")
    # 13 no allocation / lock / log / panic in the detour bodies; the unwind is caught
    for name in ("process", "detour", "mouse_look", "bump", "load_binds", "load_look", "step_selected", "contended_read"):
        body = fn_body(s["pad"], name) or ""
        if not body:
            p.append(f"13 pad.rs has no fn {name}")
        bad = [w for w in PAD_FORBIDDEN_IN_DETOUR if w in body]
        if bad:
            p.append(f"13 fn {name} (runs in a detour) uses {bad}")
    if "catch_unwind(AssertUnwindSafe(" not in (fn_body(s["pad"], "detour") or ""):
        p.append("13 the XInput detour does not catch unwinds")
    # 14 the caller's buffer only, only on success, only for the selected slot
    pr = fn_body(s["pad"], "process") or ""
    ret, wr, sel = pr.find("if ret != 0 {"), pr.find("(*st).pad = o.to_bytes()"), pr.find("if sel != idx as i32 {")
    if ret < 0 or wr < 0 or sel < 0 or not (ret < sel < wr):
        p.append("14 the state is edited without the success / selected-slot checks first")
    if len(re.findall(r"\(\*st\)\.\w+\s*=", pr)) != 1:
        p.append("14 process writes more than the gamepad bytes of the caller's state")
    # review C3: one reader at a time (a contended call steps nothing)
    if "Busy::try_take(&STEP_BUSY)" not in pr or "Busy::try_take(&LOOK_BUSY)" not in (fn_body(s["pad"], "mouse_look") or ""):
        p.append("14 the XInput / mouse-look state is stepped without the try-lock")
    # 15 the stick counts are added after the ADS scaling, once, before the gunship capture
    ps = fn_body(s["inhook"], "process_state") or ""
    ml, sens, gs = ps.find("crate::pad::mouse_look(data)"), ps.find("SENS_SCALE_BITS"), ps.find("if gunship {")
    if ps.count("crate::pad::mouse_look(") != 1 or not (0 <= sens < ml < gs):
        p.append("15 the stick's mouse counts are not added once, after the ADS scaling and before the gunship capture")
    # 16 start is never bound; the gunship hides everything but start
    if s["padlogic"].count("if b == XI_START {") < 2:
        p.append("16 a binding / the blank list may name start")
    if "(XI_ALL & !XI_START) | VB_LT | VB_RT" not in (fn_body(s["padlogic"], "step") or ""):
        p.append("16 the gunship blank does not keep start")
    # 17 the X hold synthesizes ER's interact button and hands the controller back (the interact counter)
    if "out.synth |= b.er_interact;" not in (fn_body(s["padlogic"], "step") or "") or "c(ACT_INTERACT, &PRESSES_MOVE[PRESS_INTERACT]);" not in (fn_body(s["pad"], "bump") or ""):
        p.append("17 the interact hold does not synthesize ER's interact button / count as an interact press")
    # 18 the left stick follows the W/A/S/D rule (one function)
    pb = fn_body(s["move"], "publish_blank") or ""
    if "crate::pad::set_blank_ls(mw2er_core::move_input::wasd_blanked(ctx));" not in pb or "if wasd_blanked(ctx) {" not in (fn_body(s["minput"], "blank_masks") or ""):
        p.append("18 the left stick is not blanked by the W/A/S/D rule function")
    # 19 MW2 defaults, rumble off
    for want in ("rumble: false,", "turnrate_yaw: 260.0,", "turnrate_pitch: 90.0,", "turnrate_yaw_ads: 90.0,", "turnrate_pitch_ads: 55.0,", "input_graph: 3,"):
        if want not in s["config"]:
            p.append(f"19 [gamepad] default changed: {want} missing")
    if "if g.rumble {" not in s["config"]:
        p.append("19 rumble = true is accepted (no XInputSetState hook exists)")
    # 20 the hook is installed only with [gamepad] enabled
    if not re.search(r"if cfg\.gamepad\.enabled \{[^}]*crate::pad::install", s["boot"]):
        p.append("20 the XInput hook is not gated on [gamepad] enabled")
    return p


def check_c3wiring(self_test=False):
    try:
        src = {k: v.read_text(encoding="utf-8") for k, v in C3_FILES.items()}
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = c3wiring_problems(src)
    if self_test:
        def mut(key, a, b):
            m = dict(src)
            m[key] = src[key].replace(a, b, 1)
            return m
        muts = {
            "1 rays with the fallback filter": mut("move", "mc.rays.filters_with(self.player_cast)", "mc.rays.filters()"),
            "2 the whole word, not the layer": mut("layers", "(layer_of(v) as u32, CastSrc::Live)", "(v, CastSrc::Live)"),
            "3 a write into the filter word": mut("c3read", "pub fn read_filter_words(physics: usize) -> FilterWords {",
                                                  "pub fn read_filter_words(physics: usize) -> FilterWords {\n    let _ = unsafe { super::write(physics + PHYS_FILTER_MAIN, 3u32) };"),
            "4 the talk latch on the general params": mut("fp", "w.talk_id, talk_menu, i.now, lp_talk);", "w.talk_id, i.menu_open, i.now, lp);"),
            "4 the fade latch on the general params": mut("fp", "fade_now, i.menu_open, i.now, lp_fade);", "fade_now, i.menu_open, i.now, lp);"),
            "5 F6 override turns FP off": mut("fp", "                    self.wanted = true;\n", "                    self.wanted = false;\n"),
            "6 movement ignores the FP talk latch": mut("move", "TALK_HOLD_S) || fp_talk);", "TALK_HOLD_S));"),
            "7 no yield offset": mut("vm", "+ snap.vm_offset_in[k] + drop[k]", "+ snap.vm_offset_in[k]"),
            "7 the yield gates the trigger": mut("task", "let mut trigger = can_fire && fire_raw", "let mut trigger = self.ui_yield_out.0 < 0.5 && can_fire && fire_raw"),
            "8 a second draw list": mut("hud", "        // C3 (docs/C3_DESIGN.md 2.6): damage numbers", "        let _dl2 = ui.get_background_draw_list();\n        // C3 (docs/C3_DESIGN.md 2.6): damage numbers"),
            "9 numbers for an HPDROP": mut("task", "None => log_i!(\"HPDROP\"", "None => { self.on_hit(now, handle, pos, 1, false, mw2er_core::dmgnum::DmgSrc::Gun, None, None); } _ if false => log_i!(\"HPDROP\""),
            "10 probe on by default": mut("config", "UiCfg { probe_scaleform: false,", "UiCfg { probe_scaleform: true,"),
            "10 probe without the prologue check": mut("probe", "if got != prologue {", "if false {"),
            "11 assist an Off reason": mut("mlogic", "            | Reason::Assist\n", ""),
            # PASS B
            "12 another export": mut("pad", 's!("XInputGetState")', 's!("XInputGetStateEx")'),
            "12 a game write in pad.rs": mut("pad", "fn pack(x: i16", "fn _w() { let _ = unsafe { crate::game::write(0usize, 0u8) }; }\nfn pack(x: i16"),
            "13 a log line in the detour": mut("pad", "    CALLS.fetch_add(1, Ordering::Relaxed);\n", "    CALLS.fetch_add(1, Ordering::Relaxed);\n    log_i!(\"PAD\", \"call\");\n"),
            "13 an allocation in the mouse path": mut("pad", "    let prev = LAST_LOOK_US.swap(now_us, Ordering::Relaxed);\n", "    let prev = LAST_LOOK_US.swap(now_us, Ordering::Relaxed);\n    let _v: Vec<u8> = Vec::new();\n"),
            "13 no catch_unwind": mut("pad", "let _ = catch_unwind(AssertUnwindSafe(|| {", "let _ = (|| {"),
            "14 edits a failed read": mut("pad", "if ret != 0 {", "if ret == 99 {"),
            "14 every slot": mut("pad", "if sel != idx as i32 {", "if false {"),
            "15 stick counts before the ADS scaling": mut("inhook", "            let s = f32::from_bits(SENS_SCALE_BITS.load(Ordering::Relaxed));", "            unsafe { crate::pad::mouse_look(data) };\n            let s = f32::from_bits(SENS_SCALE_BITS.load(Ordering::Relaxed));"),
            "16 start bindable": mut("padlogic", "if b == XI_START {", "if b == 0xFFFF_FFFF {"),
            "16 the gunship hides start": mut("padlogic", "(XI_ALL & !XI_START) | VB_LT | VB_RT", "XI_ALL | VB_LT | VB_RT"),
            "17 no synthesized interact": mut("padlogic", "out.synth |= b.er_interact;", "out.synth |= 0;"),
            "18 the stick on its own rule": mut("move", "crate::pad::set_blank_ls(mw2er_core::move_input::wasd_blanked(ctx));", "crate::pad::set_blank_ls(ctx.driving);"),
            "19 rumble on by default": mut("config", "            rumble: false,", "            rumble: true,"),
            "19 yaw 250": mut("config", "turnrate_yaw: 260.0,", "turnrate_yaw: 250.0,"),
            "20 the hook installed always": mut("boot", "if cfg.gamepad.enabled {", "if true {"),
            # PASS C
            "21 bit 3 on every spawn": mut("spawn", "pub const SPAWN_FLAGS: u32 = 1;", "pub const SPAWN_FLAGS: u32 = 9;"),
            "21 synced spawn without the id check": mut("spawn", "    if !mw2er_core::coop_vis::syncable_bullet(bullet_id) {\n        return Err(SpawnError::NotSyncable(bullet_id));\n    }\n", ""),
            "21 a damaging shot synced": mut("task", "match spawn::spawn(p.field_ins_handle, id, origin, p.chr.pos_w, *d) {", "match spawn::spawn_synced(p.field_ins_handle, id, origin, p.chr.pos_w, *d) {"),
            "22 apply_speffect synced": mut("ac130", "    c.apply_speffect(id, true);", "    c.apply_speffect(id, false);"),
            "22 synced wrapper without the id rule": mut("ac130", " || !mw2er_core::coop_vis::syncable_speffect(id) {", " {"),
            "23 a client never holds": mut("targets", "    if cfg.hold {\n        return Err(Skip::AwaitHost);\n    }\n", ""),
            "23 upd decides again": mut("targets", "    if cfg.hold {\n        return Err(Skip::AwaitHost);\n    }\n", "    if cfg.hold || c.owner == Owner::Remote {\n        return Err(Skip::AwaitHost);\n    }\n"),
            "23 the host holds": mut("targets", "        CoopRole::Host => false,", "        CoopRole::Host => since_first_pass_s < fallback_s,"),
            "23 fallback past the window": mut("targets", "fallback_s.clamp(0.0, window_s.max(0.0))", "fallback_s"),
            "23 the role ignored": mut("nuke", "coop_hold(self.coop_role, since, cfg.nuke.coop_client_fallback_s, cfg.nuke.coop_window_s)", "coop_hold(CoopRole::Client, since, cfg.nuke.coop_client_fallback_s, cfg.nuke.coop_window_s)"),
            "23 owner never read": mut("nuke", "owner: Owner::from_update_type(n.upd),", "owner: Owner::Unknown,"),
            "24 a peer effect spawns a damaging row": mut("coopvis", "            FxEvent::Impact { .. } => {}", "            FxEvent::Impact { .. } => { let _ = spawn::spawn(FieldInsHandle::none(), 0, V3::ZERO, 0.0, V3::ZERO); }"),
            "25 the peer drop credited": mut("task", "                                self.peer_drops\n                            );\n                            continue;", "                                self.peer_drops\n                            );"),
            "26 fp_anim ignored by the rule": mut("minput", "if ctx.fp { !ctx.fp_hybrid } else { !ctx.hybrid }", "ctx.fp || !ctx.hybrid"),
            "26 no root-motion compensation": mut("move", "let ec = rm_compensate(e, pl.last_dir, rm_max);", "let ec = e;"),
            "26 yaw gate ignores hybrid": mut("fp", " && !crate::movement::FP_HYBRID_ACTIVE.load(Ordering::Relaxed)", ""),
            "27 twins solo": mut("corevis", "    if peers == 0 || !enabled {", "    if !enabled {"),
            "28 the lethal SpEffect synced": mut("breg", '"iconId": -1, "isDisableNetSync": 1,', '"iconId": -1, "isDisableNetSync": 0,'),
            "29 world guns on ER's depth": mut("render", "            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);\n            // C6 depth 5", "            // C6 depth 5"),
            "29 record_world with a prime": mut("render", "self.record_world_occluded(list, t, slot, draws, f, &[], None)", "self.record_world_occluded(list, t, slot, draws, f, &[], self.prime_any())"),
            "35 capsules with the target bound": mut("occl", "            list.OMSetRenderTargets(0, None, false, Some(&dsv));", "            "),
            "35 capsules in first person": mut("vm", "if cfg.coop.world_gun_body_occlusion && !snap.fp_on { &snap.world_occl } else { &[] }", "if cfg.coop.world_gun_body_occlusion { &snap.world_occl } else { &[] }"),
            "35 capsules on by default": mut("config", "world_gun_body_occlusion: false,", "world_gun_body_occlusion: true,"),
            "30 the co-op tick unguarded": mut("task", "self.coopvis_off = true;", "self.coopvis_off = false;"),
            # review C3
            "31 world guns over ER's message": mut("vm", "        && !world_yield\n", ""),
            "32 orphan markers kept": mut("corevis", "if !syncable_speffect(id) || want.contains(&id) || self.applied.iter().any(|(x, _, _)| *x == id) {", "if true {"),
            "32 panic keeps the markers": mut("task", "self.coopvis.strip_markers()", "self.coopvis.reset()"),
            "32 nuke dedupe 30 s": mut("corevis", "pub const NUKE_DEDUPE_S: f64 = 75.0;", "pub const NUKE_DEDUPE_S: f64 = 30.0;"),
            "33 other-frame world gun": mut("coopvis", "            if pw != Some(w) {", "            if false {"),
            "33 peer effects without the frame rule": mut("coopvis", "self.tracker.update(now, &list, local, &other, &mut ev);", "self.tracker.update(now, &list, local, &[], &mut ev);"),
            "34 F6 releases the load latch": mut("fp", "        if self.talk_latch.is_latched() {\n            self.talk_latch.reset();", "        self.load_latch.reset();\n        if self.talk_latch.is_latched() {\n            self.talk_latch.reset();"),
            "14 no try-lock": mut("pad", "Busy::try_take(&STEP_BUSY)", "Some(Busy(&STEP_BUSY))"),
            # C6 aim 4
            "36 aim pose on by default": mut("config", 'tp_ads_pose: "off".into(),', 'tp_ads_pose: "er_aim".into(),'),
            # C6 depth 5
            "37 the prime in first person": mut("dstate", "    if snap.fp_on {\n        miss(CopyMiss::Fp);\n        return None;\n    }\n", ""),
            "37 the prime without the pairing": mut("dstate", "    if !eds::prime_pair(", "    if false && !eds::prime_pair("),
            "37 the prime on a fast turn": mut("dstate", "> cfg.coop.er_depth_max_turn_deg", "> 1000.0"),
            "37 the prime with the target bound": mut("erd", "            list.OMSetRenderTargets(0, None, false, Some(&dsv));\n            list.SetDescriptorHeaps(&self.heaps);", "            list.SetDescriptorHeaps(&self.heaps);"),
            "37 the prime tests depth": mut("erd", "DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,", "DepthFunc: D3D12_COMPARISON_FUNC_GREATER,"),
            "37 er depth on by default": mut("config", "world_gun_er_depth: false,", "world_gun_er_depth: true,"),
            "37 er depth hot on": mut("config", 'ignored.push("coop.world_gun_er_depth (turning it on)");', ""),
            "37 the world guns ignore the prime": mut("vm", "&draws, &wf, caps, prime.as_ref())", "&draws, &wf, caps, None)"),
            "36 aim pose without the stance": mut("config", 'self.stance_crossbow() && (self.tp_ads_pose == "er_aim" || self.tp_ads_er_aim)', '(self.tp_ads_pose == "er_aim" || self.tp_ads_er_aim)'),
            "36 aim pose with the polearm set": mut("config", 'self.er_stance == "crossbow"\n', 'self.stance_on()\n'),
            "36 aim fields of the motion table": mut("stance", "    for (k, f) in AIM_FIELDS.iter().enumerate() {\n        // SAFETY: a field of a live EquipParamWeapon row (heap); game::write checks the page, never faults, and is\n        // version-gated. Written only",
                                                     "    for (k, f) in STANCE_FIELDS.iter().enumerate() {\n        // SAFETY: a field of a live EquipParamWeapon row (heap); game::write checks the page, never faults, and is\n        // version-gated. Written only"),
            "36 chords during the aim": mut("stance", '.or(aiming.then_some("aim"))', ""),
            "36 the aim fields left at a restore": mut("stance", "            self.aim_restore_at(k, why)\n        } else {", "            Ok(())\n        } else {"),
            "36 keyboard aim in the gunship": mut("inhook", "if crate::shared::KB_AIM.load(Ordering::Relaxed)\n            && !gunship\n", "if crate::shared::KB_AIM.load(Ordering::Relaxed)\n"),
            "36 pad aim at a prompt": mut("padlogic", "if i.er_aim && i.mw2 && !i.gunship && !i.er_prompt {", "if i.er_aim && i.mw2 && !i.gunship {"),
            "38 aim cam without the aim pose": mut("config", 'self.ads_pose_on() && self.tp_aim_cam == "keep"', 'self.tp_aim_cam == "keep"'),
            "38 a write in the getter detour": mut("aimcam", '"lock inc qword ptr [rip + {hits_cam}]",', '"mov qword ptr [rip + {hits_cam}], rcx",'),
            "38 the detour without the player filter": mut("aimcam", '        "cmp rcx, qword ptr [rip + {player}]",\n        "jne 2f",\n', ""),
            "38 hooked before the validation": mut("aimcam", "    validate(a, base, &rd)?;\n", ""),
            "38 enabled without the pin": mut("aimcam", "    if !pin_self() {\n        return Err(\"pin\");\n    }\n", ""),
            "38 installed without keep": mut("aimcam", "let keep = keep_cfg && ensure_installed();", "let keep = ensure_installed();"),
            "38 armed in first person": mut("coreaim", "i.keep && i.tp && i.in_world && !i.dead && i.player", "i.keep && i.in_world && !i.dead && i.player"),
            "38 the arm left at a restore": mut("stance", "            // C6 round 6: the camera filter first (disarmed before the fields go back)\n            self.aimcam.stop();\n", ""),
            "38 the panic keeps the arm": mut("task", "                crate::aim_cam::disarm();\n", ""),
            "38 the fade site not masked": mut("aimcam", '        "cmp rax, qword ptr [rip + {ret_fade}]",\n        "je 5f",\n', ""),
            "38 the fade site unvalidated": mut("coreaim", '        return Err("fade_site");\n', ""),
            "38 aim episodes without the filter": mut("stance", "(!c.aim_cam_keep() || crate::aim_cam::keep_ready())", "true"),
            "38 a refused filter still ready": mut("aimcam", "ensure_installed() && !REFUSED.load(Ordering::Relaxed)", "ensure_installed() || true"),
            "39 chords during the spawn": mut("corestance", "    } else if settling {\n        Some(\"spawn\")\n", ""),
            "39 the spawn refusal not fed": mut("stance", "i.dead, i.settling)", "i.dead, false)"),
            "39 a stale enter chord disables": mut("corestance", "if dead_seen || (!exit && stance_left) {", "if dead_seen {"),
            # C6 part 16
            "40 the swap without the journal": mut("carrier", "if !write_journal(&j) {", "if !true {"),
            "40 EquipItem from the Present thread": mut("vm", "fn draw_world_guns(", "fn _x() { unsafe { crate::carrier::call_equip(0, &X, 0, 0, 0, 0) } }\nfn draw_world_guns("),
            "40 the carrier ticked from the HUD": mut("hud", "fn draw(", "fn _y(c: &mut crate::carrier::Carrier) { c.tick(&I) }\nfn draw("),
            "40 a SetSlot address": mut("carrier", "type EquipItemFn", "const _SET: usize = 0x1403bf3d0;\ntype EquipItemFn"),
            "40 a ChrAsm byte write in the carrier": mut("carrier", "fn delete_journal() -> bool {", "fn _w() { let _ = unsafe { crate::game::write::<u32>(0, 1) }; }\nfn delete_journal() -> bool {"),
            "40 called without the pins": mut("carrier", "validate(&a, base, &rd)", "Ok::<(), &str>(())"),
            "40 melee not blocked with the carrier": mut("carrier", "crate::pad::set_carrier_melee_off(self.equipped());", "crate::pad::set_carrier_melee_off(false);"),
            "40 the pad ignores the carrier's melee block": mut("pad", "WANT_MELEE_OFF.load(Ordering::Relaxed) || WANT_MELEE_OFF_CARRIER.load(Ordering::Relaxed)", "WANT_MELEE_OFF.load(Ordering::Relaxed)"),
            "40 the chord RB on a loaded crossbow": mut("stance", '            .or(loaded.then_some("loaded"))\n', ""),
            "40 carrier on by default": mut("config", 'er_carrier: "off".into(),', 'er_carrier: "crossbow".into(),'),
            "40 carrier without the stance": mut("config", "if self.stance_crossbow() { crate::carrier::Mode::of(&self.er_carrier) } else { crate::carrier::Mode::Off }", "crate::carrier::Mode::of(&self.er_carrier)"),
            "40 the grip writes the slot": mut("stance", "crate::game::write::<u32>(arm.asm + ASM_ARM_STYLE, ARM_RIGHT_BOTH)", "crate::game::write::<u32>(arm.asm + 0x7C, 43020000)"),
            "40 a chord while ER is busy": mut("stance", '            .or(self.er_busy_now.then_some("er_busy"))\n', ""),
            "40 an ignored chord counted": mut("stance", "if chord_ignored(took, played) && !(f.dead_seen || i.dead) {", "if false {"),
            # C6 part 16 addendum
            "40 an aborted keyboard chord ER played is refused": mut("stance", "if abort_finishes(f.played) {", "if false {"),
            "40 an aborted pad chord ER played is refused": mut("stance", "3 | 4 if abort_finishes(f.played) =>", "3 | 4 if false =>"),
            "40 the aborted chord keeps its keys": mut("stance", "                            // a prompt / menu appeared: E and LMB released together\n                            crate::shared::KB_CHORD_T0.store(0, std::sync::atomic::Ordering::Relaxed);\n",
                                                       "                            // a prompt / menu appeared\n"),
            "40 no chord done": mut("stance", "if let Some(why) = self.refused_logged.take() {", "if let Some(why) = self.refused_logged.filter(|_| false) {"),
            "40 the episode line writes": mut("stance", "        let ours = self.aim.active();\n", "        let ours = self.aim.active();\n        let _ = unsafe { crate::game::write::<u32>(0, 1) };\n"),
            "40 the episode line never called": mut("stance", "self.episode_tick(i, arm, own_row, prec_now, c.aim_cam_keep());", "let _ = (arm, own_row);"),
            # C6 part 16.16 (the loaded crossbow)
            "40 load on by default": mut("config", "            carrier_load: false,\n", "            carrier_load: true,\n"),
            "40 load inside the chord window": mut("corecarrier", '        Some(false) if i.chord_keys => Some("chord"),\n', ""),
            "40 load kept through a grip change": mut("corecarrier", '            None if i.arm != 3 => LoadStep::Undo("grip"),\n', ""),
            "40 load not undone before the restore": mut("carrier", "                self.load_undo(i, r, why);\n                // SAFETY: as in `swap`", "                // SAFETY: as in `swap`"),
            "40 load written to another byte": mut("carrier", "crate::game::write::<u8>(egd + equip::EGD_CHR_ASM + equip::ASM_BOLT_LOADED + s, v)", "crate::game::write::<u8>(egd + equip::EGD_CHR_ASM + equip::ASM_BOLT_LOADED + s + 1, v)"),
            "40 load without the slot check": mut("carrier", "r.handles[s] != h || pl_slot.is_none_or(|x| x.1 != h)", "false"),
            # C6 part 17 (the polearm stance + the guard ADS)
            "41 guard yaw on by default": mut("config", "            tp_guard_yaw: false,\n", "            tp_guard_yaw: true,\n"),
            "41 guard without the polearm set": mut("config", 'self.er_stance == "polearm" && self.tp_ads_pose == "guard"', 'self.tp_ads_pose == "guard"'),
            "41 guard yaw without the guard": mut("config", "self.guard_pose_on() && self.tp_guard_yaw", "self.tp_guard_yaw"),
            "41 a row write in the guard": mut("stance", "        let wanted = aim_wanted(&ai);\n        let move_input =", "        let wanted = aim_wanted(&ai);\n        let _ = write_vals(0, &[0; 5]);\n        let move_input ="),
            "41 the guard arms the camera filter": mut("stance", "        let wanted = aim_wanted(&ai);\n        let move_input =", "        let wanted = aim_wanted(&ai);\n        self.aimcam.stop();\n        let move_input ="),
            "41 the guard without the polearm set posed": mut("stance", "posed: self.posed() && !self.cycle && own_k.is_some_and(|k| self.patched[k].set == StanceSet::Polearm),", "posed: true,"),
            "41 the guard in a scope peek": mut("stance", "            scoped: i.scoped,\n            ac130: i.ac130,\n            nuke: i.nuke,\n            dead: i.dead,\n            menu: i.menu_open || i.latched || !i.in_world,\n            prompt: i.er_prompt,\n            mounted: i.mounted,\n            air: self.vy.abs() > AIR_VY_MPS,\n            input: route.is_some(),\n        };\n        let wanted = aim_wanted(&ai);\n        let move_input",
                                                "            scoped: false,\n            ac130: i.ac130,\n            nuke: i.nuke,\n            dead: i.dead,\n            menu: i.menu_open || i.latched || !i.in_world,\n            prompt: i.er_prompt,\n            mounted: i.mounted,\n            air: self.vy.abs() > AIR_VY_MPS,\n            input: route.is_some(),\n        };\n        let wanted = aim_wanted(&ai);\n        let move_input"),
            "41 chords during the guard": mut("stance", " || self.guard.active();", ";"),
            "41 the guard survives restore_all": mut("stance", "        // C6 part 17: the guard ends with the stance (its input is released just below)\n        self.guard_stop(0.0, why);\n", ""),
            "41 the guard survives our row's restore": mut("stance", "        if self.patched[k].peer == 0 {\n            self.guard_stop(0.0, why);\n        }\n", ""),
            "41 the yaw without its gate": mut("stance", "        if !guard_yaw_wanted(&gi) {\n            return false;\n        }\n", ""),
            "41 the yaw writes another field": mut("stance", "crate::game::write(p.physics + crate::game::fp_read::ORIENTATION, yaw_pair(", "crate::game::write(p.physics + crate::game::fp_read::INTERP_ORIENTATION, yaw_pair("),
            "41 restore against the crossbow set": mut("stance", "classify_set(&c, &p.orig, &p.set.vals())", "classify_set(&c, &p.orig, &StanceSet::Crossbow.vals())"),
            "41 the guard carry in ER's guard lower": mut("stance", "if guard_carry_ok(standing, self.posed(), self.head_now, self.er_busy_now, i.now - self.guard_end_t) {", "if standing && self.posed() {"),
            "41 the guard detects on the walk swing": mut("stance", "if !(self.guard_frame_ok && standing) {", "if false {"),
            "41 the guard clamp latches": mut("coopvis", "hs.guard_clamp.step(now, i.stance_guard_align, guard_over.map(|g| g.1))", "{ hs.guard_clamp.clamped |= guard_over.is_some_and(|g| g.1); hs.guard_clamp.clamped }"),
            # C6 42 (part 17 addendum)
            "42 polearm grips without the polearm set": mut("config", 'self.er_stance == "polearm" && self.polearm_grip', "self.polearm_grip"),
            "42 guard frame without the guard": mut("config", 'self.guard_pose_on() && self.guard_axes != "off"', 'self.guard_axes != "off"'),
            "42 the sword grips on the polearm carry": mut("coopvis", "if c.polearm_grip_on() { c.weapon_grip_polearm(gun) } else { c.weapon_grip_for(gun) }", "c.weapon_grip_for(gun)"),
            "42 guard frame before the detection": mut("coopvis", "let gframe = if c.guard_frame_on() && i.stance_guard_align {", "let gframe = if c.guard_frame_on() && i.stance_guard_active {"),
            "42 guard frame on stage A": mut("coopvis", "gframe.is_some() && f.weapon.is_some() && hand_drawn", "gframe.is_some() && f.weapon.is_some()"),
            "42 guard frame learns walking": mut("coopvis", "                        && i.stance_guard_standing\n", ""),
            "42 guard lower is a guard anim": mut("corestance", "17_000..18_000 | 19_000..19_100 | 19_200..20_000", "17_000..18_000 | 19_000..20_000"),
            # review 2026-10-24 43
            "43 no occlusion tick": mut("coopvis", "            self.occlusion_tick(now, i, pl);\n", ""),
            "43 rays without a physics world": mut("coopvis", "let rays_ok = ins != 0 && havok::phys_world() != 0;", "let rays_ok = true;"),
            "43 the whole segment always": mut("coopvis", "            if !cfg.coop.tracer_occlusion {\n                out.push", "            if true {\n                out.push"),
            "43 the head dot on every piece": mut("hud", "if *tip {", "if true || *tip {"),
            "43 uncast points hidden": mut("tocc", "p.vis[k].is_none_or(|(x, _)| x)", "p.vis[k].is_some_and(|(x, _)| x)"),
            "43 a miss hides": mut("tocc", "ray(cam, dir, len).is_none_or(|t| t >= len - tol)", "ray(cam, dir, len).is_some_and(|t| t >= len - tol)"),
            "43 no budget for the clips": mut("tocc", "            if left == 0 {\n                break;\n            }\n            left -= 1;", "            left = left.saturating_sub(1);"),
            "43 a hidden flash drawn": mut("coopvis", ".filter(|f| !(cfg.coop.tracer_occlusion && f.3 == Some(false)))", ".filter(|_| true)"),
            "43 streaks never occluded": mut("task", "let streak_rays = self.cfg.coop.tracer_occlusion && cam_pos", "let streak_rays = false && cam_pos"),
            "43 off by default": mut("config", "            tracer_occlusion: true,", "            tracer_occlusion: false,"),
            "43 span = the clip distance": mut("tocc", "Clip::Hit(h) => grown.min(h.max(CLIP_BACK_M)),", "Clip::Hit(h) => h.max(CLIP_BACK_M),"),
            # review 2026-10-24 44
            "44 dead receiver ignores": mut("nukelogic", "RemoteAct::Start { spectate: !r.alive }", "if !r.alive { RemoteAct::Ignore(\"dead\") } else { RemoteAct::Start { spectate: false } }"),
            "44 loading receiver ignores": mut("nukelogic", "        RemoteAct::Pend\n", "        RemoteAct::Ignore(\"loading\")\n"),
            "44 no pending": mut("nuke", "self.pending_remote = Some(PendingMarker { from, via, until });", "let _ = (from, via, until);"),
            "44 spectator dies": mut("nuke", "let plan = if spectate { plan.spectating() } else { plan };", "let plan = plan;"),
            "44 spectator shake": mut("nuke", "!self.player_dead && !self.spectate", "!self.player_dead"),
            "44 no repend": mut("nuke", 'self.pending_remote = Some(PendingMarker { from, via: "repend", until: self.remote_until });', "let _ = from;"),
        }
        # C6 35 (depth 5 layout): the depth clear moved after the capsules (two replacements)
        m35 = dict(src)
        t35 = src["render"].replace("            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);\n            // C6 depth 5", "            // C6 depth 5", 1)
        t35 = t35.replace("            if rebind {\n", "            list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);\n            if rebind {\n", 1)
        m35["render"] = t35
        muts["35 capsules before the depth clear"] = m35
        # C6 37: the prime after the capsules (the prime block moved behind the capsule block)
        m37 = dict(src)
        pb = ("            if let (Some(pr), Some(pi)) = (self.prime.as_ref(), prime) {\n                self.last_primed = pr.record(list, slot, dsv, pi);\n"
              "                rebind = self.last_primed;\n            }\n")
        t37 = src["render"].replace(pb, "", 1).replace("            if rebind {\n", pb + "            if rebind {\n", 1)
        m37["render"] = t37
        muts["37 the prime after the capsules"] = m37
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(c3wiring_problems(m)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the C3 PASS A + B + C sources caught")
    return report(problems, "c3wiring: 44 checks (docs/C3_DESIGN.md S2 check 10, PASS A 1-11, PASS B 12-20, PASS C 21-30, review C3 31-34, C6 35: the body capsules are a depth-only pass into our own cleared world-gun depth, third person, off by default, C6 36: the third-person ADS crossbow aim is off by default and needs the stance, its fields go through game::write and back before the motion fields, no chord during an aim episode, the keyboard RMB after the raw ADS and never in the gunship, C6 37: the ER-depth prime is a depth-only pass into our cleared depth before the capsules, third person only, only for this Present's spliced copy, skipped on fast turns, off by default and cold to turn on, C6 38: the aim-camera getter filter only with the aim pose and tp_aim_cam = keep, a naked detour that writes nothing and masks only ER's camera / reticle / snipe-fade call sites for our own player while armed (round 7: no aim episode with keep before it is installed), sites validated before the hook and the DLL pinned before it is enabled, every aim restore disarms first, C6 39: no grip chord while ER spawns the player, a stale enter chord is void, C6 40: the real crossbow carrier calls ER's EquipItem only from the task's tick, after the pins and the journal, never SetSlot / a slot byte write, blocks R3 melee itself, the chord's RB only unloaded, off by default and only with the stance; 16.16: the loaded-state byte only via write_bolt_loaded on our slot, flag off by default, never in a chord window or off the two-handed grip, undone before every restore, no chord while ER is busy and an ignored chord never counts; part 16 addendum: an aborted chord ER had played is finished with its keys released, a refusal ends with chord done, the aim episode line only reads, C6 41: the polearm stance + the third-person guard ADS only with er_stance = polearm and tp_ads_pose = guard (code default off), its body yaw only with tp_guard_yaw inside the guard and behind its gate (one write of the physics orientation pair), the guard writes nothing on the row and never touches the aim-camera filter, uses the aim pose's gates with the polearm set posed, no grip chord while it holds the block input, every stance stop ends it, a row is restored against the set it was written with, review 2026-10-24 43: the HUD tracers / flashes / launcher streaks are clipped and hidden by budgeted map rays (an uncast point draws), C6 42: the polearm carry grips only with the polearm stance, the guard frame only with the guard, only in a detected guard on a hand-path gun in the third-person self block before the aim align, learned only from settled guards held standing, no game write, the guard lower is no guard anim, review 2026-10-24 44: a peer's nuke marker on a dead receiver starts a spectating sequence (kill passes, no player death / screen effects), on a loading one it pends / re-pends inside its window; 43: the tracer sample span follows the bullet, capped at the clip): every controller ray casts with the live player "
                            "layer; the cast is the layer only of an accepted word; nothing writes the collision filter; the talk latch has "
                            "its own stale time and the dialog index, the fade latch its own; F6 during a hold releases the latches and keeps "
                            "wanted = 1; movement's talk hold follows first person's latch; the UI yield moves the viewmodel and dims the HUD "
                            "and never gates a shot; one background draw list, the damage numbers after the hitmarker, only from the 5 hit "
                            "sites (never an HPDROP); the Scaleform probe is off by default, boot-gated, prologue-checked and writes nothing; "
                            "the assist is a Suspend reason ticked with its capped config; PASS B: the hook is xinput1_4!XInputGetState "
                            "(no IAT, no game write), the detour bodies never log / allocate / lock / panic and catch unwinds, the state is "
                            "edited only on success for the selected slot, the stick counts come after the ADS scaling, start is never "
                            "bound and the gunship keeps it, the X hold synthesizes ER's interact button, the left stick follows the "
                            "W/A/S/D rule, MW2 defaults with rumble off, the hook only with [gamepad] enabled; PASS C: bit 3 only on "
                            "twins / the nuke marker (refused for any other id), apply_speffect keeps dont_sync and the synced wrapper "
                            "is marker-only, the co-op nuke: the host kills first, a client holds until its fallback (chr_update_type only logged), peer effects reach no damage path, the "
                            "peer-drop guard skips the credit, fp_anim through the one W/A/S/D rule with root motion compensated and "
                            "the yaw gate pausing, twins only with a peer through the watchdog, the lethal SpEffect unsynced and twins "
                            "without attack in the regulation, world guns on our own depth before the viewmodel, the co-op tick guarded")


def check_c3strings(dll):
    try:
        data = pathlib.Path(dll).read_bytes()
    except OSError as e:
        return report([f"{dll}: {e}"], "")
    missing = [x.decode() for x in C3_LOG_STRINGS if x not in data]
    same = STAGED_DLL.exists() and STAGED_DLL.read_bytes() == data
    problems = [f"missing C3 log strings: {missing}"] if missing else []
    return report(problems, f"c3strings: all {len(C3_LOG_STRINGS)} C3 PASS A + B + C proof strings present (staged copy identical: {same})")


# ---- C4.1 PASS I41 (docs/C4_1_DESIGN.md 3.5 / 3.6 / 6): the UI-thread injection's wiring ----------------------------------

C41_FILES = {
    "vm": MOD_SRC / "viewmodel.rs", "inject": MOD_SRC / "ui_inject.rs", "probe": MOD_SRC / "ui_probe.rs", "boot": MOD_SRC / "boot.rs",
    "config": CORE_SRC / "config.rs", "c4": CORE_SRC / "c4.rs", "pose": CORE_SRC / "vm_pose.rs",
    "early": ROOT / "mod" / "mw2er-render" / "src" / "early.rs", "sample": ROOT / "mod" / "mw2er-render" / "src" / "sample.rs",
    "depth": MOD_SRC / "er_depth.rs",
    # C6 depth 5 (rule 12)
    "state": MOD_SRC / "er_depth_state.rs", "erd": ROOT / "mod" / "mw2er-render" / "src" / "er_depth.rs",
    "eds": CORE_SRC / "er_depth_state.rs", "lib": MOD_SRC / "lib.rs",
}

# what ER's render thread must never reach in the C4.1 functions (allocation, resource / PSO creation, logging, a blocking
# lock, the creating begin)
C41_FORBIDDEN = (r"\.clone\(\)", r"Vec::new", r"vec!", r"format!", r"\.to_string\(\)", r"String::", r"Box::", r"\blog_[iwe]!",
                 r"\.collect", r"(?<!try_)\block\(", r"CreateCommittedResource", r"prepare_depth", r"ensure_depth", r"\.begin\(",
                 r"insert_psos", r"build_early_psos", r"LayerSampler::new", r"EarlyPass::new", r"thread::", r"\.reserve\(", r"\.push\(")
C41_RENDER_FNS = (("vm", "record_early41"), ("vm", "record_canary41"), ("inject", "on_ui_begin41"), ("inject", "readback41"),
                  ("inject", "note_ecl"), ("inject", "before_ui_end"), ("early", "begin_prepared"), ("early", "record_canary"),
                  ("sample", "record"), ("pose", "fit"), ("depth", "on_ui_begin"),
                  # C6 depth 5: UiBegin's copy / sample record, the ECL fold and splice, the barrier detour and its table
                  ("state", "on_ui_begin"), ("state", "on_ui_end"), ("state", "fold_ecl"), ("state", "on_ecl"),
                  ("erd", "resource_barrier_detour"), ("erd", "for_each_barrier"), ("eds", "record"), ("eds", "slot"),
                  # C6 round 7: the 22-texel sample points (the chest cross) built in UiBegin's detour
                  ("eds", "sample_points"), ("eds", "cross_px"))
# review C4.1: the mw2er-render functions ER's render thread reaches through record_early41 / record_canary41 / readback41
# (the rule above only scans the mw2er functions). `ok_or(x)` evaluates x even on success, so an allocating error
# argument allocates every frame; their errors must be `VmError::D3d` with a static str. (A COM `.clone()` is an AddRef,
# not an allocation, and is not listed here.)
C41_LIB_FNS = (("early", "begin_prepared"), ("early", "finish"), ("early", "record_canary"), ("early", "bind_common"),
               ("early", "psos_for"), ("early", "put_consts"), ("early", "draw_items"), ("early", "frame_consts"),
               ("early", "record_gun_early"), ("early", "record_world_early"), ("early", "record_overlay_early"),
               ("sample", "record"), ("erd", "record"))
C41_LIB_FORBIDDEN = (r"format!", r"\.into\(\)", r"\.to_string\(\)", r"to_owned\(\)", r"VmError::Invalid\(", r"VmError::Stuck\(",
                     r"String::", r"Vec::new", r"vec!", r"Box::", r"\.push\(", r"\.collect", r"\blog_[iwe]!", r"(?<!try_)\block\(",
                     r"CreateCommittedResource", r"ensure_depth")


def c41wiring_problems(src):
    p = []
    s = {k: strip_comments(v) for k, v in src.items()}
    # 1 nothing on ER's render thread allocates, creates, logs or blocks
    for key, name in C41_RENDER_FNS:
        body = fn_body(s[key], name)
        if body is None:
            p.append(f"1 {name} not found")
            continue
        hits = [pat for pat in C41_FORBIDDEN if re.search(pat, body)]
        if hits:
            p.append(f"1 {name} reaches {hits} on ER's render thread")
    # 2 the early record never creates the depth or a PSO: `begin_prepared`, which refuses what is not prepared
    for name in ("record_early41", "record_canary41"):
        if "begin_prepared(" not in (fn_body(s["vm"], name) or ""):
            p.append(f"2 {name} does not use begin_prepared")
    bi = fn_body(s["early"], "begin_inner") or ""
    if "if !create && (slot >= SLOTS || !self.has_depth(t.width, t.height) || !self.has_psos(t.format, t.mode))" not in bi:
        p.append("2 begin_prepared may create the depth")
    # 3 the canary / readback lists never barrier ER's layer (only our own UAV buffer)
    rec = fn_body(s["sample"], "record") or ""
    if [m for m in re.findall(r"transition\(([^,]+),", rec) if m.strip() != "&self.uav"]:
        p.append("3 the sampler barriers a resource that is not ours")
    if "ResourceBarrier" in (fn_body(s["early"], "record_canary") or ""):
        p.append("3 the canary record barriers the layer itself (begin / finish own the 0xC0 <-> RT pair)")
    # 4 the Present path skips the world guns / gun / overlay only when present_frame said Submitted
    cb = fn_body(s["vm"], "callback_body") or ""
    if "let early = crate::ui_inject::present_frame();" not in cb or "if early {\n        return;\n    }" not in cb or "if !early\n" not in cb:
        p.append("4 the Present path skips the gun on something other than present_frame's Submitted")
    pf = fn_body(s["inject"], "present_frame") or ""
    if "c4::present41(cur, seq, open)" not in pf or "trip(Miss::Order)" not in pf:
        p.append("4 a Present inside a UI frame neither draws nor trips order")
    if "d.early = false;" not in pf:
        p.append("4 a canary frame skips the gun at Present")
    # 5 cold and off by default, only with inject_viewmodel
    if not re.search(r"impl Default for UiCfg \{[^}]*inject_ui_thread: false \}", s["config"]) or "ignored.push(\"ui.inject_ui_thread (turning it on)\")" not in src["config"]:
        p.append("5 inject_ui_thread is not off by default and cold")
    if "self.inject_viewmodel && self.inject_ui_thread" not in s["config"] or "cfg.ui.ui_thread_effective()" not in s["boot"]:
        p.append("5 inject_ui_thread without inject_viewmodel")
    # 6 `ahead` is recorded by EVERY UiBegin (before the live check)
    ub = fn_body(s["inject"], "on_ui_begin41") or ""
    a, l = ub.find("LAST_BEGIN_SEQ.swap("), ub.find("if !LIVE.load(")
    if a < 0 or l < 0 or a > l:
        p.append("6 ahead is not recorded by every UiBegin")
    # 7 the readback only after UiEnd's own submit, on hudhook's present queue, for a Submitted frame
    rb = fn_body(s["inject"], "readback41") or ""
    for need in ("ECL_IN_UIEND.load", "ui_probe::PRESENT_QUEUE", "st.kind != Kind::Submitted"):
        if need not in rb:
            p.append(f"7 the readback without `{need}`")
    ud = fn_body(s["probe"], "ui_end_detour") or ""
    if ud.find("before_ui_end") < 0 or ud.find("before_ui_end") > ud.find("UI_END_TRAMP"):
        p.append("7 UiEnd's open flag is not set before the original")
    # 8 the AC-130 owns its frames: the gate refuses them before any chain read
    g41 = fn_body(s["c4"], "gate41") or ""
    if "Wants::Ac130 => return Err(Miss::Ac130)" not in g41 or g41.find("Wants::Ac130") > g41.find("g.chain.pins_ok()"):
        p.append("8 the C4.1 gate looks at the chain on an AC-130 frame")
    # 9 (review C4.1) the mw2er-render functions on ER's render thread: no allocating error, no allocation, no log / lock
    for key, name in C41_LIB_FNS:
        body = fn_body(s[key], name)
        if body is None:
            p.append(f"9 {name} not found")
            continue
        hits = [pat for pat in C41_LIB_FORBIDDEN if re.search(pat, body)]
        if hits:
            p.append(f"9 {key}.rs {name} reaches {hits} on ER's render thread")
    # 10 (review C4.1) the gun goes live only through the persistence check: `canary ok` arms the check, and only
    # `canary gone` (the texel empty one UI frame later) passes
    rb = fn_body(s["inject"], "read_back41") or ""
    ok_i, chk_i = rb.find("c4::canary_next41("), rb.find("c4::gone_next41(")
    if ok_i < 0 or chk_i < 0 or "CANARY.store(CANARY_PASSED" in rb[ok_i:chk_i] or "CANARY.store(CANARY_CHECK_DUE" not in rb[ok_i:chk_i] \
            or "CANARY.store(CANARY_PASSED" not in rb[chk_i:]:
        p.append("10 the canary passes without the persistence check")
    if "crate::viewmodel::record_canary41(&req, None)" not in (fn_body(s["inject"], "on_ui_begin41") or ""):
        p.append("10 the persistence-check frame draws something")
    # 11 (review C6 depth) the depth probe's GetDesc runs only in UiBegin's detour (ER's render thread, a due frame),
    # never on the task thread: the one desc call sits in er_depth::on_ui_begin, which after_ui_begin calls
    dsrc = s["depth"]
    ub = fn_body(dsrc, "on_ui_begin") or ""
    if "DESC_DUE.swap(false" not in ub or "desc_if_vtable(" not in ub:
        p.append("11 the depth probe's GetDesc is not in UiBegin's detour (a due frame only)")
    if dsrc.count("desc_if_vtable(") != 1 or re.search(r"\bdesc_of\(|GetDesc\(|desc_raw\(", dsrc):
        p.append("11 the depth probe calls GetDesc outside UiBegin's detour (the task thread)")
    if "crate::er_depth::on_ui_begin(base, sf, &f);" not in (fn_body(s["probe"], "after_ui_begin") or ""):
        p.append("11 UiBegin's detour never takes the depth probe's desc sample")
    # 12 (C6 depth 5, docs/C6_HOLD_DESIGN.md 15.1 / 15.2) the read-only ResourceBarrier probe and the ER-depth copy
    if "state" in s:
        st, erd = s["state"], s["erd"]
        det = fn_body(erd, "resource_barrier_detour") or ""
        tr, rec = det.find("TRAMP.load(Ordering::Acquire)"), det.find("for_each_barrier(")
        call = det.find("transmute::<usize, ResourceBarrierFn>(f)(this, n, p)")
        if tr < 0 or call < 0 or rec < 0 or not (tr < call < rec) or "catch_unwind(" not in det:
            p.append("12 the barrier detour does not call the original first (or records outside catch_unwind)")
        ti = fn_body(st, "try_install") or ""
        if "|| !CFG_STATE.load(Ordering::Relaxed) {" not in ti or "match install_hook() {" not in ti or st.count("match install_hook()") != 1 \
                or "if cfg.coop.probe_er_depth_state || cfg.coop.world_gun_er_depth {" not in s["boot"]:
            p.append("12 the barrier hook is installed without its flag")
        td = fn_body(st, "teardown") or ""
        on_i, dis_i = td.find("mw2er_render::er_depth::ON.store(false"), td.find("MH_DisableHook(")
        if on_i < 0 or dis_i < 0 or on_i > dis_i or "MH_RemoveHook" in st or "er_depth_state::teardown();" not in s["lib"]:
            p.append("12 the barrier hook's teardown is not recording-off then disable (or it frees the trampoline)")
        eh = fn_body(s["probe"], "ecl_hook") or ""
        fo, inj, cp = eh.find("crate::er_depth_state::fold_ecl(queue, lists, seq);"), eh.find("crate::ui_inject::on_ecl("), eh.find("crate::er_depth_state::on_ecl(")
        if fo < 0 or inj < 0 or cp < 0 or not (fo < inj < cp):
            p.append("12 the fold does not run before the splices (injection, then the copy)")
        if st.count("r.record(slot, res,") != 1 or "r.record(slot, res," not in (fn_body(st, "on_ui_begin") or ""):
            p.append("12 the copy is recorded outside UiBegin's detour")
        oe = fn_body(st, "on_ecl") or ""
        if "if idx != 0 {" not in oe or "if eds::splice_ok(at).is_err() {" not in oe or "Splice { at: 0," not in (fn_body(st, "splice_of") or ""):
            p.append("12 the copy is spliced at another index / without the fold's readable state")
        # (review 2026-10-21) the DLL pinned before the hook is enabled (a thread inside the original returns into the
        # detour: never unmapped), and a resized reader's fence starts at NEXT_VALUE with the armed copy cancelled
        # before the new lists are published
        ih = fn_body(st, "install_hook") or ""
        pin_i, en_i = ih.find("if !pin_self() {"), ih.find("MH_EnableHook(")
        if pin_i < 0 or en_i < 0 or pin_i > en_i or "GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN" not in (fn_body(st, "pin_self") or ""):
            p.append("12 the barrier hook is enabled without pinning the DLL first")
        rw = fn_body(st, "reader_worker") or ""
        cu_i, lr_i = rw.find("CUR.swap(0, Ordering::SeqCst)"), rw.find("for (k, a) in LIST_RAW.iter().enumerate() {")
        if "DepthReader::new(&dev, size.0, size.1, NEXT_VALUE.load(Ordering::SeqCst))" not in rw or cu_i < 0 or lr_i < 0 or cu_i > lr_i \
                or "SAMPLE_VALUE.store(0, Ordering::SeqCst);" not in rw[cu_i:lr_i]:
            p.append("12 a resized reader reuses old fence values (fence not started at NEXT_VALUE, or the armed copy / sample not cleared first)")
        rr = fn_body(erd, "record") or ""
        if [m for m in re.findall(r"transition\(([^,]+),", rr) if m.strip() not in ("&s.tex", "&self.out")] or "ResourceBarrier" in (fn_body(st, "on_ui_begin") or ""):
            p.append("12 a barrier of ours names a resource that is not ours (ER's depth)")
        # C6 round 6 (work/c6/depth6.md 4): every call counted after the original and before any gate; both candidates
        # recorded by the detour; the census lock- / allocation-free; B watched only from its pinned chain + the proven
        # D3D12 vtable (as A)
        ca, on_g = det.find("CALLS.fetch_add(1, Ordering::Relaxed);"), det.find("if !ON.load(Ordering::Relaxed)")
        if ca < 0 or on_g < 0 or not (call < ca < on_g) or "TABLE_B.record(list, tid, wb, b);" not in det or "CENSUS.record(res, sub, before, after, flags);" not in det:
            p.append("12 the detour's call count is behind a gate / a candidate or the census is not recorded")
        cen = fn_body(s["eds"][s["eds"].find("impl Census"):], "record") or ""
        hits = [pat for pat in C41_FORBIDDEN if re.search(pat, cen)]
        if not cen or hits:
            p.append(f"12 the census record reaches {hits or ['(not found)']} on a recording thread")
        ub = fn_body(st, "on_ui_begin") or ""
        if "chain_ok[k] = ok && proven_vt[k] != 0 && res_vt[k] == proven_vt[k];" not in ub or "let cand = [(c.ok(), c.a.res), (b.ok(), b.res)];" not in ub \
                or "let b = er_depth::dsb_chain(dsb, base, a, rdf);" not in (fn_body(s["depth"], "on_ui_begin") or ""):
            p.append("12 candidate B is watched without its pinned chain and the proven D3D12 vtable")
        # C6 round 7 (work/c6/depth7.md 1 item 3): the sample arm in UiBegin takes the chest cross from an array and
        # the turn / move snapshots through atomics only (the self check's "still" camera)
        arm = ub[ub.find("let pts = if what.sample {"):ub.find("match r.record(slot, res,")]
        if "SAMPLE_MOVE.store(MOVE.load(Ordering::Relaxed), Ordering::Relaxed);" not in arm or "Some(eds::sample_points(g.bb.0, g.bb.1, chest))" not in arm \
                or [pat for pat in C41_FORBIDDEN if re.search(pat, arm)]:
            p.append("12 the sample arm is not atomics-only (or misses the camera move snapshot)")
    return p


def check_c41wiring(self_test=False):
    try:
        src = {k: v.read_text(encoding="utf-8") for k, v in C41_FILES.items()}
    except OSError as e:
        return report([f"sources: {e}"], "")
    problems = c41wiring_problems(src)
    if self_test:
        def mut(key, a, b):
            m = dict(src)
            m[key] = src[key].replace(a, b, 1)
            return m
        muts = {
            "1 the snapshot cloned on the render thread": mut("vm", "        es.copy_from(&s);", "        let _c = s.clone();\n        es.copy_from(&s);"),
            "1 a log line in the detour": mut("inject", "    let ahead = c4::ahead41(", "    log_i!(\"UI\", \"x\");\n    let ahead = c4::ahead41("),
            "1 a blocking lock": mut("inject", "let Some(mut sg) = try_lock(&SAMPLER) else { return };", "let mut sg = crate::shared::lock(&SAMPLER);"),
            "1 the pose scratch reallocates": mut("pose", "                v.clear();\n                v.resize(n, [0.0; 3]);", "                *v = vec![[0.0; 3]; n];"),
            "2 the creating begin": mut("vm", "    e.begin_prepared(q.slot, q.value, &t).map_err(EarlyErr::Vm)?;\n    let now", "    e.begin(q.slot, q.value, &t).map_err(|x| EarlyErr::D3d(x.to_string()))?;\n    let now"),
            "3 a barrier on ER's layer": mut("sample", "self.list.ResourceBarrier(&[transition(&self.uav, D3D12_RESOURCE_STATE_COMMON,", "self.list.ResourceBarrier(&[transition(layer, D3D12_RESOURCE_STATE_COMMON,"),
            "4 the gun skipped without Submitted": mut("vm", "let early = crate::ui_inject::present_frame();", "let early = crate::ui_inject::present_frame() || true;"),
            "4 no order trip": mut("inject", "    if order {\n        trip(Miss::Order);\n    }", "    if order {}"),
            "5 on by default": mut("config", "inject_ui_thread: false }", "inject_ui_thread: true }"),
            "7 the readback without UiEnd's submit": mut("inject", "if !ECL_IN_UIEND.load(Ordering::SeqCst) || queue == 0", "if queue == 0"),
            "9 an eager allocating error in finish": mut("early", "ok_or(VmError::D3d { call: \"EarlyPass::finish (slot)\", hr: E_INVALIDARG })",
                                                          "ok_or(VmError::Invalid(format!(\"early slot {slot}\")))"),
            "9 an allocating error in record_gun_early": mut("early", "ok_or(VmError::D3d { call: \"record_gun_early (gun assets not uploaded)\", hr: E_INVALIDARG })",
                                                             "ok_or(VmError::Invalid(\"gun assets not uploaded\".into()))"),
            "9 a String error in the sampler record": mut("sample", "return Err(VmError::D3d { call: \"LayerSampler::record(format)\", hr: E_INVALIDARG });",
                                                          "return Err(VmError::Invalid(format!(\"layer format {}\", format.0)));"),
            "10 the canary passes at once": mut("inject", "CANARY_NOREAD.store(0, Ordering::SeqCst);\n                    CANARY.store(CANARY_CHECK_DUE, Ordering::SeqCst);",
                                                "CANARY.store(CANARY_PASSED, Ordering::SeqCst);"),
            "11 GetDesc back on the task thread": mut("depth", "        let b_mod = b.ok() && self.in_d3d12(b_vt, bb_vt);\n",
                                                      "        let b_mod = b.ok() && self.in_d3d12(b_vt, bb_vt);\n        let _d = crate::ui_probe::desc_if_vtable(b.res, b_vt);\n"),
            "11 the detour never samples": mut("probe", "    crate::er_depth::on_ui_begin(base, sf, &f);\n", ""),
            # C6 depth 5
            "12 the detour never calls the original": mut("erd", "    let f = TRAMP.load(Ordering::Acquire);\n", "    let f = 0usize;\n"),
            "12 a log line in the barrier detour": mut("erd", "    CALLS.fetch_add(1, Ordering::Relaxed);\n", "    CALLS.fetch_add(1, Ordering::Relaxed);\n    log_i!(\"DEPTH\", \"x\");\n"),
            "12 a lock in the table": mut("eds", "        self.recorded.fetch_add(1, Ordering::Relaxed);\n", "        self.recorded.fetch_add(1, Ordering::Relaxed);\n        let _g = crate::shared::lock(&LOCK);\n"),
            "12 an allocation in the fold's lookup": mut("eds", "        let h = hash(list);\n", "        let h = hash(list);\n        let _v: Vec<u8> = Vec::new();\n"),
            "12 the hook without its flag": mut("state", "if HOOK_TRIED.load(Ordering::Relaxed) || !CFG_STATE.load(Ordering::Relaxed) {", "if HOOK_TRIED.load(Ordering::Relaxed) {"),
            "12 teardown without recording off": mut("state", "    mw2er_render::er_depth::ON.store(false, Ordering::SeqCst);\n    WATCH.store(0", "    WATCH.store(0"),
            "12 the fold missing": mut("probe", "        crate::er_depth_state::fold_ecl(queue, lists, seq);\n", ""),
            "12 the copy spliced at at > 0": mut("state", "    if idx != 0 {", "    if false {"),
            "12 the copy without the readable state": mut("state", "    if eds::splice_ok(at).is_err() {", "    if false {"),
            "12 a barrier on ER's depth": mut("erd", "transition(&s.tex, COPY_REST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)", "transition(res, COPY_REST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)"),
            "12 the hook enabled without the pin": mut("state", "    if !pin_self() {\n        return Err(\"pin\");\n    }\n", ""),
            "12 the resized fence starts at 0": mut("state", "DepthReader::new(&dev, size.0, size.1, NEXT_VALUE.load(Ordering::SeqCst))", "DepthReader::new(&dev, size.0, size.1, 0)"),
            "12 the armed copy kept across a resize": mut("state", "                    if CUR.swap(0, Ordering::SeqCst) != 0 {\n                        miss(CopyMiss::Busy);\n                    }\n", ""),
            "12 the call count behind the gate": mut("erd", "    CALLS.fetch_add(1, Ordering::Relaxed);\n    if !ON.load(Ordering::Relaxed) || n == 0 || p.is_null() {\n        return;\n    }\n",
                                                       "    if !ON.load(Ordering::Relaxed) || n == 0 || p.is_null() {\n        return;\n    }\n    CALLS.fetch_add(1, Ordering::Relaxed);\n"),
            "12 candidate B never recorded": mut("erd", "                TABLE_B.record(list, tid, wb, b);\n", ""),
            "12 an allocation in the census": mut("eds", "        e.n.fetch_add(1, Ordering::Relaxed);\n        e.states.fetch_or(", "        let _v: Vec<u8> = Vec::new();\n        e.n.fetch_add(1, Ordering::Relaxed);\n        e.states.fetch_or("),
            "12 B without its proven vtable": mut("state", "chain_ok[k] = ok && proven_vt[k] != 0 && res_vt[k] == proven_vt[k];", "chain_ok[k] = ok;"),
            "12 an allocation in the sample points": mut("eds", "    p[16] = [w1 / 2, h1 / 2];\n", "    p[16] = [w1 / 2, h1 / 2];\n    let _v: Vec<u8> = Vec::new();\n"),
            "12 a log line in the sample arm": mut("state", "        SAMPLE_MOVE.store(MOVE.load(Ordering::Relaxed), Ordering::Relaxed);\n",
                                                   "        SAMPLE_MOVE.store(MOVE.load(Ordering::Relaxed), Ordering::Relaxed);\n        log_i!(\"DEPTH\", \"x\");\n"),
            "12 the sample arm without the move": mut("state", "        SAMPLE_MOVE.store(MOVE.load(Ordering::Relaxed), Ordering::Relaxed);\n", ""),
            "12 an allocating error in the reader": mut("erd", 'return Err(VmError::D3d { call: "DepthReader::record(slot)", hr: E_INVALIDARG });', 'return Err(VmError::Invalid(format!("slot {slot}")));'),
            "11 a log line in the depth detour": mut("depth", "    let Some(a) = ADDRS.get() else { return };\n", "    let Some(a) = ADDRS.get() else { return };\n    log_i!(\"DEPTH\", \"x\");\n"),
            "8 the chain before the AC-130 row": mut("c4", "        Wants::Ac130 => return Err(Miss::Ac130),\n        Wants::NotFp => return Err(Miss::NotFp),\n        Wants::ScopedNoOverlay => return Err(Miss::ScopedNoOverlay),\n        Wants::NotReady => return Err(Miss::NotReady),\n    }\n    if !g.layer_path",
                                                      "        Wants::NotFp => return Err(Miss::NotFp),\n        Wants::ScopedNoOverlay => return Err(Miss::ScopedNoOverlay),\n        Wants::NotReady => return Err(Miss::NotReady),\n        _ => {}\n    }\n    if !g.layer_path"),
        }
        # "6": the swap moved after the live check (built explicitly: two replacements)
        m6 = dict(src)
        t = src["inject"].replace("    let ahead = c4::ahead41(LAST_BEGIN_SEQ.swap(f.seq, Ordering::SeqCst), f.seq);\n", "", 1)
        t = t.replace("    let cur = STATE.load(Ordering::SeqCst);\n    let first = c4::begin_ok(cur, f.seq);\n    if first && c4::unpack(cur).kind == Kind::Armed {\n        miss(Miss::Cancelled);\n    }\n    let now = shared::now_s();\n    let hdr_ok",
                      "    let ahead = c4::ahead41(LAST_BEGIN_SEQ.swap(f.seq, Ordering::SeqCst), f.seq);\n    let cur = STATE.load(Ordering::SeqCst);\n    let first = c4::begin_ok(cur, f.seq);\n    if first && c4::unpack(cur).kind == Kind::Armed {\n        miss(Miss::Cancelled);\n    }\n    let now = shared::now_s();\n    let hdr_ok", 1)
        m6["inject"] = t
        muts["6 ahead only when live"] = m6
        base = set(problems)
        missed = [k for k, m in muts.items() if m == src or set(c41wiring_problems(m)) <= base]
        if missed:
            problems.append(f"self-test: mutations not caught or not applied: {missed}")
        else:
            print(f"self-test: {len(muts)} mutations of the C4.1 PASS I41 sources caught")
    return report(problems, "c41wiring: 12 checks (docs/C4_1_DESIGN.md 3.5 / 3.6 / 6, C6 depth 5): on ER's render thread the C4.1 record, canary, "
                            "readback, gate and pose allocate, create, log and block nothing; the early begin never creates the depth or a "
                            "PSO; the sampler and the canary never barrier ER's layer; Present skips the gun only on present_frame's "
                            "Submitted, trips order inside a UI frame and draws canary frames; inject_ui_thread is cold, off by default "
                            "and needs inject_viewmodel; every UiBegin records `ahead`; the readback waits for UiEnd's own submit on the "
                            "present queue; the AC-130 row comes before the chain; the mw2er-render record functions it reaches "
                            "build no allocating error (VmError::D3d only); the gun goes live only after the canary persistence "
                            "check (an empty list, the canary texel empty one UI frame later); the C6 depth probe's GetDesc runs "
                            "only in UiBegin's detour on ER's render thread, never on the task thread; C6 depth 5: the read-only "
                            "ResourceBarrier detour calls the original first and allocates / logs / locks nothing, the hook only "
                            "under probe_er_depth_state, recording off before MH_DisableHook (no MH_RemoveHook), the fold before "
                            "the splices, the ER-depth copy recorded only in UiBegin and spliced only at index 0 on a fold-proven "
                            "readable state, no barrier of ours on ER's depth, the DLL pinned before the hook is enabled, a resized reader's "
                            "fence started at NEXT_VALUE with the armed copy / sample cleared before its lists are published; C6 round 6: every "
                            "detour call counted before any gate, both candidates (A and B from their pinned chains + the proven "
                            "vtable) and the lock- / allocation-free census recorded")


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    cmd, rest = argv[0], argv[1:]
    if cmd == "c3wiring":
        return check_c3wiring(self_test=rest == ["--self-test"])
    if cmd == "c41wiring":
        return check_c41wiring(self_test=rest == ["--self-test"])
    if cmd == "c3strings" and len(rest) == 1:
        return check_c3strings(rest[0])
    if cmd == "pe" and len(rest) == 1:
        return check_pe(rest[0])
    if cmd == "ids":
        return check_ids()
    if cmd == "profile":
        return check_profile(rest[0] if rest else ROOT / "profiles" / "mw2er.me3")
    if cmd == "gitignore":
        return check_gitignore()
    if cmd == "gamefolder":
        return check_gamefolder()
    if cmd == "bootorder":
        return check_bootorder(self_test=rest == ["--self-test"])
    if cmd == "vmwiring":
        return check_vmwiring(self_test=rest == ["--self-test"])
    if cmd == "shaders" and len(rest) == 1:
        return check_shaders(rest[0])
    if cmd == "hygiene":
        return check_hygiene()
    if cmd == "dist" and len(rest) == 1:
        return check_dist(rest[0])
    if cmd == "fpwiring":
        return check_fpwiring(self_test=rest == ["--self-test"])
    if cmd == "fpstrings" and len(rest) == 1:
        return check_fpstrings(rest[0])
    if cmd == "ac130wiring":
        return check_ac130wiring(self_test=rest == ["--self-test"])
    if cmd == "ac130strings" and len(rest) == 1:
        return check_ac130strings(rest[0])
    if cmd == "movewiring":
        return check_movewiring(self_test=rest == ["--self-test"])
    if cmd == "movestrings" and len(rest) == 1:
        return check_movestrings(rest[0])
    if cmd == "m2rows":
        return check_m2rows()
    if cmd == "nukewiring":
        return check_nukewiring(self_test=rest == ["--self-test"])
    if cmd == "nukestrings" and len(rest) == 1:
        return check_nukestrings(rest[0])
    if cmd == "arsenalwiring":
        return check_arsenalwiring(self_test=rest == ["--self-test"])
    if cmd == "arsenalstrings" and len(rest) == 1:
        return check_arsenalstrings(rest[0])
    if cmd == "mantlewiring":
        return check_mantlewiring(self_test=rest == ["--self-test"])
    if cmd == "m8strings" and len(rest) == 1:
        return check_m8strings(rest[0])
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
