"""Regenerates / checks the vendored hudhook 0.9.3 patches (M3 patch 0001, C4 patch 0002).

vendor/hudhook-0.9.3 = the pristine cargo-registry copy + tools/patches/hudhook-0001-pre-imgui-hook.patch (M3: the
pre-ImGui callback, SetColorSpace1) + tools/patches/hudhook-0002-ecl-callback.patch (C4, docs/C4_UI_DESIGN.md 2.1 /
3.2: the ExecuteCommandLists callback with the splice, PRESENT_SEQ, PRESENT_TID). Each patch is the
`git diff --no-index` of the patched files against the stage before it, with paths relative to the crate root (apply
in order with `git -c core.autocrlf=false apply --directory=vendor/hudhook-0.9.3`).

Usage: python -I make_hudhook_patch.py           rewrite patch 0002 from the vendored copy (0001 is never rewritten)
       python -I make_hudhook_patch.py --check   0001 + 0002 applied to the registry copy == the vendored copy, both
                                                 patch files equal a fresh generation, and the vendored ECL impl has
                                                 the callback before the trampoline and exactly 3 trampoline calls +
                                                 one Signal on the splice path
Read-only except the 0002 .patch file (and a temp folder under %TEMP% that is deleted).
"""
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
REG = pathlib.Path(os.environ["USERPROFILE"]) / ".cargo" / "registry" / "src" / "index.crates.io-1949cf8c6b5b557f" / "hudhook-0.9.3"
VENDOR = ROOT / "vendor" / "hudhook-0.9.3"
P0001 = ROOT / "tools" / "patches" / "hudhook-0001-pre-imgui-hook.patch"
P0002 = ROOT / "tools" / "patches" / "hudhook-0002-ecl-callback.patch"
FILES = ["src/hooks/dx12.rs", "src/renderer/backend/dx12.rs"]


def diff(a_root, b_root):
    """The patch text of FILES from a_root to b_root (None on a git error)."""
    parts = []
    for f in FILES:
        r = subprocess.run(["git", "-c", "core.autocrlf=false", "diff", "--no-index", "--no-color", str(a_root / f), str(b_root / f)],
                           capture_output=True)
        if r.returncode not in (0, 1):
            print(f"FAIL git diff {f}: {r.stderr.decode(errors='replace')}")
            return None
        lines = r.stdout.decode("utf-8").replace("\r\n", "\n").splitlines(keepends=True)
        for l in lines:
            if l.startswith("diff --git "):
                l = f"diff --git a/{f} b/{f}\n"
            elif l.startswith("--- "):
                l = f"--- a/{f}\n"
            elif l.startswith("+++ "):
                l = f"+++ b/{f}\n"
            parts.append(l)
    return "".join(parts)


def stage(tmp, patches):
    """Copies the registry FILES into tmp and applies `patches` in order; returns an error text or None."""
    for f in FILES:
        (tmp / f).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REG / f, tmp / f)
    for p in patches:
        r = subprocess.run(["git", "-c", "core.autocrlf=false", "apply", str(p)], cwd=str(tmp), capture_output=True)
        if r.returncode != 0:
            return f"git apply {p.name}: {r.stderr.decode(errors='replace').strip()}"
    return None


def ecl_source_problems(text):
    """Static rules of the vendored ECL impl (design 8.1 item 3)."""
    p = []
    m = re.search(r"unsafe extern \"system\" fn d3d12_command_queue_execute_command_lists_impl\(.*?\n}\n", text, re.S)
    if not m:
        return ["no d3d12_command_queue_execute_command_lists_impl"]
    body = m.group(0)
    hook = body.find("pre_imgui::ECL_HOOK.get()")
    calls = [x.start() for x in re.finditer(r"d3d12_command_queue_execute_command_lists\(", body)]
    if hook < 0 or not calls or hook > calls[0]:
        p.append("the ECL callback does not run before the trampoline")
    arm = re.search(r"Ok\(Some\(pre_imgui::Splice \{.*?return;", body, re.S)
    if not arm:
        p.append("no splice arm ending in return")
    else:
        a = arm.group(0)
        if len(re.findall(r"d3d12_command_queue_execute_command_lists\(", a)) != 3 or a.count(".Signal(") != 1:
            p.append("the splice is not exactly 3 trampoline calls + 1 Signal")
        if a.find(".Signal(") > a.rfind("d3d12_command_queue_execute_command_lists(") or "if at > 0" not in a:
            p.append("the Signal does not follow our list before the game's remaining lists / at == 0 not skipped")
    if "catch_unwind" not in body:
        p.append("the ECL callback is not inside catch_unwind")
    pre = re.search(r"unsafe extern \"system\" fn dxgi_swap_chain_present_impl\(.*?\n}\n", text, re.S)
    pre1 = re.search(r"unsafe extern \"system\" fn dxgi_swap_chain_present1_impl\(.*?\n}\n", text, re.S)
    for name, b in (("Present", pre), ("Present1", pre1)):
        if not b or "PRESENT_TID.store(" not in b.group(0) or "PRESENT_SEQ.fetch_add(1" not in b.group(0):
            p.append(f"{name} does not record PRESENT_TID / PRESENT_SEQ")
        elif b.group(0).find("PRESENT_SEQ.fetch_add(1") < b.group(0).find("let result ="):
            p.append(f"{name} counts the frame before its trampoline returned")
    return p


def main(argv):
    check = argv == ["--check"]
    if argv and not check:
        print(__doc__)
        return 2
    with tempfile.TemporaryDirectory(prefix="mw2er_hudhook_") as td:
        tmp = pathlib.Path(td)
        s1 = tmp / "s1"
        err = stage(s1, [P0001])
        if err:
            print(f"FAIL {err}")
            return 1
        gen1 = diff(REG, s1)
        gen2 = diff(s1, VENDOR)
        if gen1 is None or gen2 is None:
            return 1
        if not check:
            P0002.write_bytes(gen2.encode("utf-8"))
            print(f"wrote {P0002} ({len(gen2.splitlines())} lines); 0001 unchanged")
            return 0
        problems = []
        if gen1.encode("utf-8") != P0001.read_bytes():
            problems.append("patch 0001 differs from a fresh generation (registry -> registry + 0001)")
        if not P0002.exists() or gen2.encode("utf-8") != P0002.read_bytes():
            problems.append("patch 0002 differs from a fresh generation (registry + 0001 -> vendor)")
        s2 = tmp / "s2"
        err = stage(s2, [P0001, P0002])
        if err:
            problems.append(err)
        else:
            for f in FILES:
                if (s2 / f).read_bytes() != (VENDOR / f).read_bytes():
                    problems.append(f"registry + 0001 + 0002 != vendor for {f}")
        problems += ecl_source_problems((VENDOR / FILES[0]).read_text(encoding="utf-8"))
        if problems:
            print("FAIL hudhook patches: " + " | ".join(problems))
            return 1
        print(f"PASS hudhook patches: registry + 0001 ({len(gen1.splitlines())} lines) + 0002 ({len(gen2.splitlines())} lines) == "
              f"vendor/hudhook-0.9.3 ({', '.join(FILES)}); the ECL callback runs before the trampoline; the splice = 3 "
              f"trampoline calls + 1 Signal; Present / Present1 record PRESENT_TID and count PRESENT_SEQ after the trampoline")
        return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
