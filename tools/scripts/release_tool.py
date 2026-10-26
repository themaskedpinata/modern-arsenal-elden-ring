"""MW2 in Elden Ring: helpers of the release step (docs/INSTALLER_DESIGN.md 5.1; driven by tools\\scripts\\make_release.ps1).

    python -I release_tool.py manifest   <stage> --version V --pin KEY=SHA256 ... [--note KEY=TEXT ...] [--update-repo OWNER/NAME]
                                                                                    write <stage>\\manifest\\release.json
    python -I release_tool.py lint       <stage> --pin KEY=SHA256 ...                the pre-release deny / allow lint
    python -I release_tool.py licenses   <Cargo.toml> <out-dir> --roots installer mw2er sndconv   (runs cargo metadata)
    python -I release_tool.py notices    <stage> --version V                         write <stage>\\THIRD_PARTY_NOTICES.md (P4)
    python -I release_tool.py zip        <stage> <out.zip>                           deterministic payload zip
    python -I release_tool.py pack       <setup.exe> <payload.zip> <out.exe>         append the payload + trailer
    python -I release_tool.py check-pack <out.exe>                                   read the trailer back, re-hash
    python -I release_tool.py snapshot   <out.json> <dir> ...                        names / sizes / mtimes (read-only)
    python -I release_tool.py compare    <before.json> <after.json>                  exit 1 when anything changed
    python -I release_tool.py binary-lint <exe|dll> ...                            P4 review: no PC path / user name
                                                                                    (ASCII + UTF-16), no VC++ runtime imports
    python -I release_tool.py self-test                                              lint / pack / co-op id checks
    python -I release_tool.py nexus      <release-dir> --version V [--oat-source]    P4: <release-dir>\\nexus\\ (upload zip,
                                                                                    page / form / media / clean-PC drafts)
    python -I release_tool.py nexus-lint <release-dir> --version V                   P4: the lint of <release-dir>\\nexus\\

`--pin` keys: me3_installer_sha256 (tools\\me3\\me3_installer.exe), python_embed_sha256
(runtime\\python-3.14.7-embed-amd64.zip); P2 (installer regulation.rs): package_bnd4_sha256, rows_json_core_sha256 (from
`MW2inER-setup.exe --cli regulation-info`). `--note` keys (short text pins): regulation_builder (regbuild | witchybnd).
The trailer format is the one of mod\\installer\\src\\payload.rs (OVERLAY_MAGIC):
[16] magic [8] u64 LE zip offset [8] u64 LE zip length [32] sha256 of the zip; offset + length = file size - 64.
The co-op id is release.rs Release::coop_id: first 8 hex of sha256 over "<path> <hash>\\n" (sorted) + "seamless <ersc>\\n".
Read-only except for each command's named output. Never reads a password: the lint only tests whether a
`cooppassword = ...` line has a value and prints the file name, never the value.
"""
import fnmatch
import hashlib
import io
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
MAGIC = b"MW2inER-payload1"
TRAILER = 64
ZIP_TIME = (2026, 1, 1, 0, 0, 0)
PYTHON_EMBED_NAME = "python-3.14.7-embed-amd64.zip"
WITCHY_URL = "https://github.com/ividyon/WitchyBND/releases/download/3.0.1.0/WitchyBND-3.0.1.0-win-x64.zip"
ME3_URL = "https://github.com/garyttierney/me3/releases/download/v0.13.0/me3_installer.exe"
PYTHON_URL = "https://www.python.org/ftp/python/3.14.7/" + PYTHON_EMBED_NAME
LOCKED_TOML = ["damage", "regulation*", "arsenal.guns", "launchers.explosion_tiers"]

# ---- the payload allow-list: every staged file and where it must come from ----------------------------------------
# ("same", repo path)  byte-identical to that file of this repository ({name} = the staged file name)
# ("pinned", pin key)  sha256 == the --pin value (official third-party files, unmodified)
# ("generated", why)   written by the release step; text only, content-scanned
# The scripts are the list of make_friend_package.ps1 step 2 (keep both in step).
SCRIPTS = ["convert_weapons.py", "copy_sounds.py", "convert_viewmodel.py", "vm_format.py", "convert_ac130.py",
           "convert_movement.py", "convert_arsenal.py", "inventory_m6.py", "convert_nuke.py", "xanim_read.py",
           "check_assets.py", "build_regulation.py", "param_diff.py", "param_rows.py", "conpty_run.py", "friend_dump.py",
           "coop_manifest.py"]
PACKAGE_FILES = ["README.md", "INSTALL_GUIDE.md", "INSTALL_GUIDE.txt", "Install.cmd", "Play co-op.cmd", "Play solo.cmd",
                 "setup.ps1", "setup_ui.ps1", "launch_coop.ps1", "launch_solo.ps1"]
ALLOW = [
    ("build/natives/mw2er.dll", ("same", "build/natives/mw2er.dll")),
    ("mw2er.toml", ("generated", "the host's mw2er.toml under the release policy (friend_toml.ps1)")),
    ("manifest/host_manifest.json", ("generated", "hashes only (coop_manifest.py write)")),
    ("manifest/host_pins.json", ("generated", "version pins")),
    ("manifest/release.json", ("generated", "pins + hashes (release_tool.py manifest)")),
    *[(f"tools/scripts/{s}", ("same", f"tools/scripts/{s}")) for s in SCRIPTS],
    ("tools/oat/bin/Unlinker.exe", ("same", "tools/oat/build/bin/Release_x64/Unlinker.exe")),
    ("tools/oat/LICENSE", ("same", "tools/oat/LICENSE")),
    ("tools/oat/local-build-fixes.patch", ("generated", "git diff of tools/oat (GPL corresponding-source note)")),
    ("tools/bin/sndconv.exe", ("same", "mod/target/release/sndconv.exe")),
    ("tools/THIRD_PARTY.md", ("generated", "third-party notes")),
    ("tools/WitchyBND/PUT_NOTHING_HERE.txt", ("generated", "placeholder")),
    ("tools/me3/me3_installer.exe", ("pinned", "me3_installer_sha256")),
    ("tools/me3/LICENSE-MIT", ("same", "work/installer/cache/me3-0.13.0/LICENSE-MIT")),
    ("tools/me3/LICENSE-APACHE", ("same", "work/installer/cache/me3-0.13.0/LICENSE-APACHE")),
    ("tools/me3/SOURCE.md", ("generated", "where the me3 installer came from")),
    (f"runtime/{PYTHON_EMBED_NAME}", ("pinned", "python_embed_sha256")),
    ("licenses/python/LICENSE.txt", ("same", "work/installer/cache/python-3.14.7-embed-amd64/LICENSE.txt")),
    ("licenses/*", ("generated", "licence texts / notices")),
    *[(f, ("same", f"tools/coop_friend/{f}")) for f in PACKAGE_FILES],
    ("profiles/mw2er-coop.template.me3", ("same", "tools/coop_friend/mw2er-coop.template.me3")),
    ("profiles/mw2er-solo.template.me3", ("same", "tools/coop_friend/mw2er-solo.template.me3")),
    # P4: the mod's own licence and the notices of every bundled third-party component
    ("LICENSE", ("same", "LICENSE")),
    ("THIRD_PARTY_NOTICES.md", ("generated", "third-party notices (release_tool.py notices)")),
]
# > 5 MB is allowed only for these; nothing may exceed 20 MB
BIG_OK = {"build/natives/mw2er.dll", "tools/oat/bin/Unlinker.exe", "tools/me3/me3_installer.exe", f"runtime/{PYTHON_EMBED_NAME}"}
DENY_NAME = re.compile(r"(?i)(^regulation\.bin$|\.sl2$|\.co2$|\.mw2co2$|\.ff$|\.iwd$|\.iwi$|\.dcx$|\.bdt$|\.bhd$|\.dds$|\.glb$|"
                       r"\.wav$|\.mp3$|\.vmtex$|\.vmmesh$|\.vmskel$|\.vmanim$|\.rgba$|\.param$|\.pdb$|^ersc.*\.dll$|"
                       r"^ersc_settings\.ini$|^ersc_launcher\.exe$|^oo2core.*\.dll$|^steam_api.*\.dll$|^dinput8\.dll$|"
                       r"^setup_inputs\.json$|^install\.json$|^launch_command\.txt$)")
DENY_DIRS = {"converted", "dumps", "work", "backups", "seamless", "seamlesscoop", "logs", "setup_logs", "state", "downloads",
             "coop", "app"}
DENY_MAGIC = [(b"DDS ", "DDS texture"), (b"glTF", "GLB model"), (b"BND4", "FromSoftware BND4"), (b"DCX\x00", "FromSoftware DCX"),
              (b"IWff", "IW fastfile"), (b"IWi", "IW image"), (b"RIFF", "RIFF/WAV audio"), (b"ID3", "MP3 audio"),
              (b"VMSK", "vmskel"), (b"VMME", "vmmesh"), (b"VMAN", "vmanim"), (b"VMTX", "vmtex")]
# paths of this PC allowed only as documented defaults (the same exceptions as m2_static_checks.py check_dist)
PATH_ALLOWED = {
    "tools/scripts/build_regulation.py": [r"D:\SteamLibrary"],    # MW2ER_GAME_REGULATION default (the setup always sets it)
    "tools/scripts/convert_movement.py": [r"D:\SteamLibrary"],    # MW2ER_IW4SP default (the setup always sets it)
    "tools/oat/bin/Unlinker.exe": [str(ROOT)],                    # compile-time source paths of libtomcrypt asserts
}


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def is_sha(s):
    return isinstance(s, str) and re.fullmatch(r"[0-9a-fA-F]{64}", s) is not None


def staged_files(stage):
    stage = pathlib.Path(stage)
    return sorted((p.relative_to(stage).as_posix(), p) for p in stage.rglob("*") if p.is_file())


def coop_id(files, ersc):
    s = "".join(f"{k} {(e.get('sha256') or e.get('canon_sha256')).lower()}\n" for k, e in sorted(files.items()))
    s += f"seamless {ersc.lower()}\n"
    return hashlib.sha256(s.encode("utf-8")).hexdigest()[:8]


def parse_pins(items):
    pins = {}
    for it in items:
        k, _, v = it.partition("=")
        if not is_sha(v):
            raise SystemExit(f"--pin {k}: not a sha256")
        pins[k] = v.lower()
    return pins


NOTE_KEYS = {"regulation_builder": ("regbuild", "witchybnd")}


def parse_notes(items):
    """--note KEY=TEXT: the text pins (only the known keys and values)."""
    notes = {}
    for it in items:
        k, _, v = it.partition("=")
        if k not in NOTE_KEYS or v not in NOTE_KEYS[k]:
            raise SystemExit(f"--note {k}={v}: allowed {NOTE_KEYS}")
        notes[k] = v
    return notes


def check_regulation_pins(pins, problems):
    """P2: a release made with regbuild carries the builder-independent pins (the WitchyBND fallback is verified by them)."""
    b = pins.get("regulation_builder", "")
    if b not in ("", "regbuild", "witchybnd"):
        problems.append(f"manifest/release.json: regulation_builder {b!r} is neither regbuild nor witchybnd")
    if b == "regbuild" and not (is_sha(pins.get("package_bnd4_sha256")) and is_sha(pins.get("rows_json_core_sha256"))):
        problems.append("manifest/release.json: a regbuild release without package_bnd4_sha256 / rows_json_core_sha256")


# ---- manifest ----------------------------------------------------------------------------------------------------

UPDATE_REPO = re.compile(r"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}")


def write_manifest(stage, version, pins, notes=None, update_repo=""):
    stage = pathlib.Path(stage)
    host_pins = json.loads((stage / "manifest/host_pins.json").read_text(encoding="utf-8-sig"))
    host_man = json.loads((stage / "manifest/host_manifest.json").read_text(encoding="utf-8-sig"))
    allpins = dict(host_pins)
    allpins.update(pins)
    allpins.update(notes or {})
    problems = []
    check_regulation_pins(allpins, problems)
    if update_repo and (not UPDATE_REPO.fullmatch(update_repo) or ".." in update_repo):
        problems.append(f"--update-repo {update_repo!r} is not owner/name")
    if problems:
        raise SystemExit("; ".join(problems))
    payload = {rel: sha256_file(p) for rel, p in staged_files(stage) if rel != "manifest/release.json"}
    sources = [
        {"name": "witchybnd", "url": WITCHY_URL, "sha256": host_pins.get("witchybnd_zip_sha256", ""),
         "file_name": WITCHY_URL.rsplit("/", 1)[1],
         "how": ("only for the WitchyBND fallback (--regulation-tool witchy; the regulation is built in): " if allpins.get("regulation_builder") == "regbuild" else "")
                + "downloaded at install time only after a click, or browsed (offline)"},
        {"name": "me3", "url": ME3_URL, "sha256": pins.get("me3_installer_sha256", ""), "file_name": "me3_installer.exe",
         "how": "bundled unmodified as tools/me3/me3_installer.exe; run only on the player's click"},
        {"name": "cpython", "url": PYTHON_URL, "sha256": pins.get("python_embed_sha256", ""), "file_name": PYTHON_EMBED_NAME,
         "how": "bundled unmodified as runtime/" + PYTHON_EMBED_NAME + "; unpacked into <root>/runtime/python"},
    ]
    doc = {
        "what": "MW2 in Elden Ring release manifest: version pins, the host's hash manifest (hashes only, no game content), "
                "the pinned sources and the sha256 of every shipped payload file",
        "format": 1,
        "release_version": version,
        "coop_id": coop_id(host_man["files"], host_pins.get("ersc_sha256", "")),
        "pins": allpins,
        "manifest": host_man,
        "payload": payload,
        "locked_toml": LOCKED_TOML,
        "sources": sources,
        # P3: the GitHub repository of the installer's opt-in "newer release" check (api.github.com only; empty = off)
        "update_repo": update_repo,
    }
    out = stage / "manifest/release.json"
    out.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(f"PASS wrote {out}: release {version}, {len(payload)} payload files, {len(host_man['files'])} manifest entries, "
          f"co-op id {doc['coop_id']}")
    return doc


# ---- lint --------------------------------------------------------------------------------------------------------

def rule_for(rel):
    for pat, origin in ALLOW:
        if rel == pat or ("*" in pat and fnmatch.fnmatchcase(rel, pat) and rel.count("/") >= pat.count("/")):
            return origin
    return None


def pc_path_patterns(repo):
    pats = [re.escape(r"D:\SteamLibrary"), r"[A-Za-z]:\\Users\\[^\\\s\"'<>|*?%]{2,}", re.escape(r"Documents\seamless"),
            re.escape(str(repo)) + r"(?!-coop)"]
    for env in ("USERPROFILE", "LOCALAPPDATA", "APPDATA"):
        v = os.environ.get(env)
        if v and len(v) > 3:
            pats.append(re.escape(v))
    user = os.environ.get("USERNAME", "")
    if len(user) >= 3 and user.lower() not in ("user", "admin", "administrator"):
        pats.append(r"(?<![A-Za-z0-9])" + re.escape(user) + r"(?![A-Za-z0-9])")
    return re.compile("(?i)(" + "|".join(pats) + ")")


# P4 review: our own binaries are built with the static CRT (mod\.cargo\config.toml; cargo must run from mod\): they
# must not need the VC++ redistributable. The Unlinker (tools/oat, GPL, built by its own premake) does; the setup checks
# for that runtime before the dumps (detect.rs vcredist row).
CRT_IMPORT = re.compile(r"(?i)^(vcruntime\d|msvcp\d|api-ms-win-crt-|ucrtbase)")
OWN_BINARIES = {"build/natives/mw2er.dll", "tools/bin/sndconv.exe"}


def pe_imports(b):
    """The DLL names of a PE32+ image's import table (raises ValueError when it is not one)."""
    try:
        if b[:2] != b"MZ":
            raise ValueError("not an MZ image")
        pe = struct.unpack_from("<I", b, 0x3C)[0]
        if b[pe:pe + 4] != b"PE\0\0":
            raise ValueError("no PE signature")
        nsec, opt_size = struct.unpack_from("<H", b, pe + 6)[0], struct.unpack_from("<H", b, pe + 20)[0]
        opt = pe + 24
        if struct.unpack_from("<H", b, opt)[0] != 0x20B:
            raise ValueError("not PE32+")
        imp_rva = struct.unpack_from("<I", b, opt + 112 + 8)[0]
        secs = []
        for i in range(nsec):
            o = opt + opt_size + i * 40
            vsize, vaddr, rsize, rptr = struct.unpack_from("<IIII", b, o + 8)
            secs.append((vaddr, max(vsize, rsize), rptr))

        def off(rva):
            for va, size, ptr in secs:
                if va <= rva < va + size:
                    return rva - va + ptr
            raise ValueError(f"rva 0x{rva:x} outside the sections")
        out = []
        d = off(imp_rva) if imp_rva else None
        while d is not None:
            name_rva = struct.unpack_from("<I", b, d + 12)[0]
            if name_rva == 0:
                break
            o = off(name_rva)
            out.append(b[o:b.index(b"\0", o)].decode("ascii", "replace"))
            d += 20
        return out
    except struct.error as e:
        raise ValueError(f"truncated PE ({e})") from None


def program_image(b):
    """A packed setup's program part (the payload zip + trailer cut off); other files unchanged."""
    t = b[-TRAILER:]
    if len(b) > TRAILER and t[:16] == MAGIC:
        off, ln = struct.unpack("<QQ", t[16:32])
        if off + ln == len(b) - TRAILER:
            return b[:off]
    return b


def path_hits(data, pathre):
    """Paths / the user name of this PC in bytes: ASCII / Latin-1 and UTF-16 LE at both byte alignments."""
    hits = set()
    for text in (data.decode("latin-1"), data.decode("utf-16-le", "ignore"), data[1:].decode("utf-16-le", "ignore")):
        hits |= {m.group(0) for m in pathre.finditer(text)}
    return hits


def binary_problems(where, data, pathre, crt=True):
    """P4 review: the scan of a shipped program (the setup exe, our DLL / exes): no path or user name of this PC
    (panic / file!() strings, PDB paths) and, for our own binaries, no VC++ runtime imports."""
    problems = []
    img = program_image(data)
    if path_hits(img, pathre):
        problems.append(f"{where}: the program contains a path or the user name of this PC "
                        f"({len(path_hits(img, pathre))} distinct string(s); build from mod\\ so .cargo\\config.toml remaps them)")
    if crt:
        try:
            bad = [i for i in pe_imports(img) if CRT_IMPORT.match(i)]
        except ValueError as e:
            problems.append(f"{where}: {e}")
        else:
            if bad:
                problems.append(f"{where}: imports the VC++ runtime {sorted(set(bad))} (static CRT expected: build from mod\\)")
    return problems


def fake_pe(imports, extra=b""):
    """A minimal PE32+ image with an import table naming `imports` (+ `extra` bytes): self-test input."""
    names, blob = [], b""
    base = 0x1000 + 20 * (len(imports) + 1)
    for n in imports:
        names.append(base + len(blob))
        blob += n.encode("ascii") + b"\0"
    sec = b"".join(struct.pack("<IIIII", 0, 0, 0, r, 0) for r in names) + b"\0" * 20 + blob + extra
    opt = bytearray(240)
    struct.pack_into("<H", opt, 0, 0x20B)
    struct.pack_into("<II", opt, 112 + 8, 0x1000 if imports else 0, len(sec))
    hdr = bytearray(0x40)
    hdr[:2] = b"MZ"
    struct.pack_into("<I", hdr, 0x3C, 0x40)
    coff = b"PE\0\0" + struct.pack("<HHIIIHH", 0x8664, 1, 0, 0, 0, 240, 0x22)
    sh = b".rdata\0\0" + struct.pack("<IIIIIIHHI", len(sec), 0x1000, len(sec), 0x200, 0, 0, 0, 0, 0x40000040)
    img = bytes(hdr) + coff + bytes(opt) + sh
    return img + b"\0" * (0x200 - len(img)) + sec


def cmd_binary_lint(paths):
    pathre = pc_path_patterns(ROOT)
    problems = []
    for p in paths:
        problems += binary_problems(pathlib.Path(p).name, pathlib.Path(p).read_bytes(), pathre)
    for x in problems:
        print("FAIL " + x)
    print(f"{'PASS' if not problems else 'FAIL'} binary lint: {len(paths)} program(s); {len(problems)} problem(s); "
          "paths / user name of this PC (ASCII + UTF-16), VC++ runtime imports")
    return 0 if not problems else 1


STEAM_ID = re.compile(r"(?<!\d)7656119\d{10}(?!\d)")
PASSWORD_LINE = re.compile(rb"(?im)^[ \t]*cooppassword[ \t]*=[ \t]*[^\s;#]")


def check_manifest_doc(rel, doc, problems):
    """Hashes only: no content, no unexpected keys."""
    files = doc.get("files", {})
    extra = set(doc) - {"what", "files", "counts"}
    if extra:
        problems.append(f"{rel}: unexpected keys {sorted(extra)}")
    for k, e in files.items():
        ok = (set(e) == {"sha256", "size"} and is_sha(e["sha256"]) and isinstance(e["size"], int)) or \
             (set(e) == {"canon_sha256", "drops"} and is_sha(e["canon_sha256"]) and isinstance(e["drops"], list))
        if not ok:
            problems.append(f"{rel}: entry {k} is not a hash entry")
        if not (k.startswith("converted/") or k in ("build/rows.json", "build/package/regulation.bin", "build/natives/mw2er.dll")):
            problems.append(f"{rel}: unexpected entry {k}")


def lint(stage, pins, repo=ROOT):
    """Returns (problems, notes, n_files). Never prints file content."""
    stage, repo = pathlib.Path(stage), pathlib.Path(repo)
    problems, notes = [], []
    pathre = pc_path_patterns(repo)
    files = staged_files(stage)
    for d in sorted(p for p in stage.rglob("*") if p.is_dir()):
        if d.name.lower() in DENY_DIRS:
            problems.append(f"{d.relative_to(stage).as_posix()}/: a folder that must never ship")
    host_pins = {}
    if (stage / "manifest/host_pins.json").is_file():
        host_pins = json.loads((stage / "manifest/host_pins.json").read_text(encoding="utf-8-sig"))
    for rel, p in files:
        name = p.name
        data = p.read_bytes()
        if DENY_NAME.search(name):
            problems.append(f"{rel}: banned file (game / converted / save / Seamless / private file)")
        origin = rule_for(rel)
        if origin is None:
            problems.append(f"{rel}: not in the release allow-list (release_tool.py ALLOW)")
            continue
        kind, what = origin
        if kind == "same":
            src = repo / what.replace("{name}", name)
            if not src.is_file():
                problems.append(f"{rel}: its source {what} is missing in the repository")
            elif sha256_file(src) != hashlib.sha256(data).hexdigest():
                problems.append(f"{rel}: differs from its source {what}")
        elif kind == "pinned":
            want = pins.get(what, "")
            if not is_sha(want):
                problems.append(f"{rel}: no --pin {what} given")
            elif hashlib.sha256(data).hexdigest() != want.lower():
                problems.append(f"{rel}: sha256 is not the official {what}")
            continue  # an official file fixed by its hash: no content scan, no size rule
        else:
            try:
                data.decode("utf-8-sig")
            except UnicodeDecodeError:
                problems.append(f"{rel}: generated file is not UTF-8 text")
        for magic, what_m in DENY_MAGIC:
            if data.startswith(magic):
                problems.append(f"{rel}: {what_m} magic")
        if data[:4] == b"PK\x03\x04":
            problems.append(f"{rel}: an archive that is not a pinned official file")
        size = len(data)
        if size > 20 * 1024 * 1024 or (size > 5 * 1024 * 1024 and rel not in BIG_OK):
            problems.append(f"{rel}: {size} bytes (> 5 MB and not a known binary, or > 20 MB)")
        allowed = PATH_ALLOWED.get(rel, [])
        hits = path_hits(data, pathre)
        for text in (data.decode("latin-1"), data.decode("utf-16-le", "ignore")):
            if STEAM_ID.search(text):
                problems.append(f"{rel}: contains a Steam id")
        if rel in OWN_BINARIES and data[:2] == b"MZ":
            try:
                bad = [i for i in pe_imports(data) if CRT_IMPORT.match(i)]
            except ValueError as e:
                bad = []
                problems.append(f"{rel}: {e}")
            if bad:
                problems.append(f"{rel}: imports the VC++ runtime {sorted(set(bad))} (static CRT expected)")
        for h in sorted(hits):
            if any(h.lower().startswith(a.lower()) for a in allowed):
                notes.append(f"{rel}: documented default path")
            else:
                problems.append(f"{rel}: contains a path or the user name of this PC")
        if PASSWORD_LINE.search(data):
            problems.append(f"{rel}: contains a cooppassword value")
    # the manifests: hashes only, consistent with the payload and the DLL
    man_p = stage / "manifest/host_manifest.json"
    rel_p = stage / "manifest/release.json"
    if not man_p.is_file():
        problems.append("manifest/host_manifest.json is missing")
    else:
        check_manifest_doc("manifest/host_manifest.json", json.loads(man_p.read_text(encoding="utf-8-sig")), problems)
    if not rel_p.is_file():
        problems.append("manifest/release.json is missing")
    else:
        rj = json.loads(rel_p.read_text(encoding="utf-8"))
        extra = set(rj) - {"what", "format", "release_version", "coop_id", "pins", "manifest", "payload", "locked_toml", "sources", "update_repo"}
        if extra:
            problems.append(f"manifest/release.json: unexpected keys {sorted(extra)}")
        ur = rj.get("update_repo", "")
        if ur and (not isinstance(ur, str) or not UPDATE_REPO.fullmatch(ur) or ".." in ur):
            problems.append("manifest/release.json: update_repo is not owner/name")
        check_manifest_doc("manifest/release.json manifest", rj.get("manifest", {}), problems)
        check_regulation_pins(rj.get("pins", {}), problems)
        if any(not (is_sha(v) or (isinstance(v, str) and re.fullmatch(r"[\w .,;:()\-]{0,200}", v))) for v in rj.get("pins", {}).values()):
            problems.append("manifest/release.json: a pin that is neither a hash nor a short version note")
        have = {rel: hashlib.sha256(p.read_bytes()).hexdigest() for rel, p in files if rel != "manifest/release.json"}
        if rj.get("payload") != have:
            diff = sorted(set(have.items()) ^ set(rj.get("payload", {}).items()))
            problems.append(f"manifest/release.json: payload hashes differ from the staged files ({len(diff)} difference(s), "
                            f"e.g. {diff[0][0] if diff else '?'})")
        dll = stage / "build/natives/mw2er.dll"
        if dll.is_file():
            h = sha256_file(dll)
            if h != host_pins.get("mw2er_dll_sha256") or rj.get("manifest", {}).get("files", {}).get("build/natives/mw2er.dll", {}).get("sha256") != h:
                problems.append("build/natives/mw2er.dll: differs from the pinned / manifest DLL")
    return problems, sorted(set(notes)), len(files)


def cmd_lint(stage, pins):
    problems, notes, n = lint(stage, pins)
    for x in notes:
        print("INFO " + x)
    for x in problems:
        print("FAIL " + x)
    print(f"{'PASS' if not problems else 'FAIL'} release lint: {n} files; {len(problems)} problem(s); "
          "deny names / folders / magic, allow-list with origins, PC paths / user name / Steam ids / passwords, sizes, "
          "manifests hashes only")
    return 0 if not problems else 1


# ---- licences of the Rust crates linked into the shipped binaries ---------------------------------------------------

MIT_TEXT = """MIT License

Copyright (c) {who}

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE."""
LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|notice|copyright|unlicense)([-_.].*)?$")


def write_licenses(cargo_toml, out_dir, roots):
    r = subprocess.run(["cargo", "metadata", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc", "--locked",
                        "--offline", "--manifest-path", str(cargo_toml)], capture_output=True)
    if r.returncode != 0:
        raise SystemExit("cargo metadata failed: " + r.stderr.decode("utf-8", "replace")[-2000:])
    meta = json.loads(r.stdout.decode("utf-8"))
    pk = {p["id"]: p for p in meta["packages"]}
    members = set(meta["workspace_members"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    todo = [i for i in members if pk[i]["name"] in roots]
    if len(todo) != len(roots):
        raise SystemExit(f"roots not found in the workspace: {roots}")
    seen = set()
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k.get("kind") is None for k in d["dep_kinds"]):
                todo.append(d["pkg"])
    crates = sorted((pk[i] for i in seen if i not in members), key=lambda p: (p["name"], p["version"]))
    out = pathlib.Path(out_dir)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    rows, texts, missing, apache = [], [], [], None
    for c in crates:
        d = pathlib.Path(c["manifest_path"]).parent
        lic = sorted(p for p in d.iterdir() if p.is_file() and LICENSE_FILE.match(p.name)) if d.is_dir() else []
        # licences of embedded assets one folder down (e.g. epaint_default_fonts: fonts\OFL.txt, UFL.txt, ...)
        lic += sorted(p for p in d.glob("*/*") if p.is_file() and re.search(r"(?i)(licen[cs]e|copying|notice|ofl|ufl)", p.name)
                      or (p.parent.name == "fonts" and p.suffix.lower() == ".txt"))
        up = d
        while not lic and c.get("source") is None and len(up.parents) > 1 and up.parent.name != "mod":
            up = up.parent  # a vendored path crate: the licence sits at its repository's root (vendor/<repo>)
            lic = sorted(p for p in up.iterdir() if p.is_file() and LICENSE_FILE.match(p.name))
        repo = c.get("repository") or c.get("homepage") or ""
        rows.append(f"| {c['name']} | {c['version']} | {c.get('license') or c.get('license_file') or '?'} | {repo} |")
        for p in lic:
            body = p.read_text(encoding="utf-8", errors="replace").strip()
            texts.append(f"==== {c['name']} {c['version']}: {p.relative_to(up if up != d else d).as_posix()} ====\n\n{body}\n")
            if apache is None and "Apache License" in body and "Version 2.0, January 2004" in body and "END OF TERMS" in body:
                apache = body[:body.index("END OF TERMS") + len("END OF TERMS AND CONDITIONS")]
        if not lic:
            expr = c.get("license") or ""
            head = f"==== {c['name']} {c['version']}: no licence file in the crate; licence {expr}; source {repo} ===="
            if "Apache-2.0" in expr:
                missing.append(c)
                texts.append(head + "\n(used under its Apache-2.0 option; the text is at the end of this file)\n")
            elif re.fullmatch(r"MIT", expr.strip()):
                who = ", ".join(c.get("authors") or [f"the {c['name']} authors"])
                texts.append(head + "\n\n" + MIT_TEXT.format(who=who) + "\n")
            elif expr.strip() == "MPL-2.0":
                texts.append(head + "\n\nThis Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. "
                             "If a copy of the MPL was not distributed with this file, You can obtain one at "
                             f"https://mozilla.org/MPL/2.0/. Unmodified; its source code: {repo} (crates.io {c['name']} "
                             f"{c['version']}).\n")
            else:
                raise SystemExit(f"{c['name']} {c['version']}: no licence file and licence {expr!r} has no rule here")
    if missing:
        texts.append("==== Apache License 2.0 (for " + ", ".join(f"{c['name']} {c['version']}" for c in missing) + ") ====\n\n"
                     + (apache or "Apache License 2.0: https://www.apache.org/licenses/LICENSE-2.0") + "\n")
    (out / "README.md").write_text(
        "# Rust crates in the shipped binaries\n\n"
        f"Compiled into {', '.join(roots)} (mw2er.dll, the setup / MW2inER.exe, sndconv.exe) for x86_64-pc-windows-msvc, "
        "from `cargo metadata` of the workspace (normal dependencies). Each crate's licence files follow in "
        "LICENSE-TEXTS.txt.\n\n| crate | version | licence | source |\n|---|---|---|---|\n" + "\n".join(rows) + "\n",
        encoding="utf-8", newline="\n")
    (out / "LICENSE-TEXTS.txt").write_text("\n".join(texts), encoding="utf-8", newline="\n")
    print(f"PASS licences of {len(crates)} crates written to {out}")
    return 0


# ---- THIRD_PARTY_NOTICES.md (P4) -------------------------------------------------------------------------------------

DISPLAY_NAME = "Modern Arsenal for Elden Ring"   # = mod/installer/src/lib.rs DISPLAY_NAME


def write_notices(stage, version):
    """<stage>/THIRD_PARTY_NOTICES.md: the mod's licence, every bundled component (where, version, licence, source,
    licence text in the payload), the GPL-3.0 corresponding-source note of the OAT Unlinker, the Rust crates (the
    table of licenses/rust/README.md; texts in licenses/rust/LICENSE-TEXTS.txt) and what is NOT shipped."""
    stage = pathlib.Path(stage)
    pins = json.loads((stage / "manifest/host_pins.json").read_text(encoding="utf-8-sig"))
    commit = pins.get("oat_commit", "")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise SystemExit("host_pins.json has no oat_commit: the Unlinker's source cannot be named")
    for need in ("LICENSE", "tools/oat/LICENSE", "tools/oat/local-build-fixes.patch", "tools/me3/LICENSE-MIT",
                 "tools/me3/LICENSE-APACHE", "licenses/python/LICENSE.txt", "licenses/rust/README.md",
                 "licenses/rust/LICENSE-TEXTS.txt", f"runtime/{PYTHON_EMBED_NAME}", "tools/oat/bin/Unlinker.exe"):
        if not (stage / need).is_file():
            raise SystemExit(f"notices: {need} is not staged")
    rust = (stage / "licenses/rust/README.md").read_text(encoding="utf-8")
    table = [l for l in rust.splitlines() if l.startswith("|")]
    n_crates = max(0, len(table) - 2)
    text = f"""# Third-party notices: {DISPLAY_NAME} {version}

{DISPLAY_NAME} (internal name MW2inER) is an unofficial fan mod. Its own code (mw2er.dll, the setup / MW2inER.exe,
sndconv.exe, the Python converters and scripts, the docs) is licensed under the MIT License, Copyright (c) 2026
maskedpinata: see `LICENSE`. The components below keep their own licences. No file of Elden Ring or Call of Duty:
Modern Warfare 2 (2009) is included: the setup builds the mod's data on the player's PC from the player's own copies.

## Bundled components

| Component | In this release | Version | Licence | Licence text here | Source |
|---|---|---|---|---|---|
| me3 installer (the mod loader) | `tools/me3/me3_installer.exe` (official, unmodified) | 0.13.0 | MIT OR Apache-2.0 | `tools/me3/LICENSE-MIT`, `tools/me3/LICENSE-APACHE` | https://github.com/garyttierney/me3 (tag v0.13.0); `tools/me3/SOURCE.md` |
| CPython embeddable package | `runtime/{PYTHON_EMBED_NAME}` (official, unmodified) | 3.14.7 | PSF License Agreement (+ the licences of the code it contains) | `licenses/python/LICENSE.txt` | {PYTHON_URL} |
| OpenAssetTools Unlinker (iw4x-x64/oat fork, IW4MS) | `tools/oat/bin/Unlinker.exe` (built from source, 2 build fixes) | commit {commit[:12]} | GPL-3.0 | `tools/oat/LICENSE` | https://github.com/iw4x-x64/oat (see the source note below) |
| Rust crates ({n_crates}) | compiled into `build/natives/mw2er.dll`, `MW2inER.exe` (the setup) and `tools/bin/sndconv.exe` | see the table | see the table (MIT, Apache-2.0, BSD, Zlib, MPL-2.0, Unicode, OFL / UFL fonts, ...) | `licenses/rust/LICENSE-TEXTS.txt` | crates.io / the repositories in the table |

The Unlinker runs as a separate program (the setup starts it to read the player's own MW2 zone files); no OpenAssetTools
code is linked into the mod. Third-party code inside the Unlinker binary (zlib, libtomcrypt, libtommath, minilzo, lz4,
salsa20, json, eigen, stb, ...) is under its own licences, listed in the oat repository's `thirdparty/` folder.

## GPL-3.0: corresponding source of the OpenAssetTools Unlinker

`tools/oat/bin/Unlinker.exe` is object code of OpenAssetTools (GPL-3.0, https://github.com/Laupetin/OpenAssetTools) in
the fork https://github.com/iw4x-x64/oat. Its corresponding source is:

1. https://github.com/iw4x-x64/oat at commit `{commit}`, with its git submodules at the commits that commit pins;
2. plus `tools/oat/local-build-fixes.patch` (shipped in this release): two build fixes for MSVC 14.44, no behaviour
   change;
3. built with `premake5 vs2022` (premake 5.0.0-beta8, fetched by the repository's own scripts), then msbuild of the
   generated solution, Release x64. Details: `tools/THIRD_PARTY.md`.

To get it: `git clone https://github.com/iw4x-x64/oat`, `git -C oat checkout {commit}`,
`git -C oat submodule update --init --recursive`, `git -C oat apply local-build-fixes.patch`. If that repository is no
longer available, ask the mod's author (maskedpinata, on the mod's Nexus Mods page) for the source of this exact
version. You may redistribute the Unlinker under the GPL-3.0; the GPL does not extend to the separate programs of the
mod.

## Not included

- **Seamless Co-op** (by LukeYui): never shipped and never downloaded by the setup. Each player downloads it from Nexus
  Mods; the setup copies the player's own download into the install folder.
- **WitchyBND** (by ividyon, GPL-3.0, https://github.com/ividyon/WitchyBND): not shipped and not needed (the setup
  builds the regulation with its own Rust code). Only the optional fallback (Advanced in the setup window, or
  `--regulation-tool witchy`) downloads the official WitchyBND-3.0.1.0-win-x64.zip after a click, checked by sha256.
- **Game files**: no Elden Ring or MW2 file, no converted data, no regulation.bin, no saves.

## Credits (research and reference, no code shipped unless listed above)

- fromsoftware-rs (vswarte and contributors): Elden Ring structures, compiled in as Rust crates (listed in the table).
- hudhook (veeenu and contributors): the D3D12 hook, compiled in with one patch (listed in the table).
- erfps2 (Dasaav-dsv, MIT OR Apache-2.0): first-person camera research (ideas and addresses).
- Paramdex and SoulsFormats (the Souls modding community): param layouts and the regulation container facts.
- IW4L (vladtrc, Apache-2.0): reference for MW2's XAnim rules.

## Rust crates

Compiled into the shipped binaries (from `cargo metadata`, normal dependencies, x86_64-pc-windows-msvc). Each crate's
licence files are in `licenses/rust/LICENSE-TEXTS.txt`.

""" + "\n".join(table) + "\n"
    out = stage / "THIRD_PARTY_NOTICES.md"
    out.write_text(text, encoding="utf-8", newline="\n")
    print(f"PASS wrote {out}: {n_crates} crates, oat {commit[:12]}")
    return 0


# ---- zip / pack ----------------------------------------------------------------------------------------------------

def write_zip(stage, out):
    out = pathlib.Path(out)
    with zipfile.ZipFile(out, "w") as z:
        for rel, p in staged_files(stage):
            zi = zipfile.ZipInfo(rel, ZIP_TIME)
            zi.external_attr = 0o644 << 16
            zi.compress_type = zipfile.ZIP_STORED if rel.lower().endswith(".zip") else zipfile.ZIP_DEFLATED
            z.writestr(zi, p.read_bytes(), compresslevel=9)
    print(f"PASS payload zip {out}: {out.stat().st_size / 1048576:.1f} MB, sha256 {sha256_file(out)}")
    return 0


def pack(exe, zp, out):
    image = pathlib.Path(exe).read_bytes()
    z = pathlib.Path(zp).read_bytes()
    if image[:2] != b"MZ":
        raise SystemExit(f"{exe} is not a program")
    if image[-TRAILER:][:16] == MAGIC:
        raise SystemExit(f"{exe} already carries a payload")
    trailer = MAGIC + struct.pack("<QQ", len(image), len(z)) + hashlib.sha256(z).digest()
    assert len(trailer) == TRAILER
    pathlib.Path(out).write_bytes(image + z + trailer)
    return 0


def read_pack(exe):
    """(offset, length, sha256 hex, zip bytes) or raises."""
    b = pathlib.Path(exe).read_bytes()
    t = b[-TRAILER:]
    if len(b) < TRAILER or t[:16] != MAGIC:
        raise ValueError("no embedded payload")
    off, ln = struct.unpack("<QQ", t[16:32])
    if off + ln != len(b) - TRAILER:
        raise ValueError("trailer does not fit the file")
    z = b[off:off + ln]
    if hashlib.sha256(z).digest() != t[32:]:
        raise ValueError("embedded payload sha256 mismatch")
    return off, ln, t[32:].hex(), z


def check_pack(exe):
    off, ln, sha, z = read_pack(exe)
    with zipfile.ZipFile(io.BytesIO(z)) as zf:
        names = zf.namelist()
        bad = zf.testzip()
        rj = json.loads(zf.read("manifest/release.json"))
    if bad:
        raise SystemExit(f"FAIL {exe}: damaged member {bad}")
    print(f"PASS {exe}: program {off / 1048576:.1f} MB + payload {ln / 1048576:.1f} MB ({len(names)} files, sha256 "
          f"{sha[:12]}), release {rj['release_version']}, co-op id {rj['coop_id']}")
    return 0


# ---- snapshots (read-only: names, sizes, modification times) ------------------------------------------------------

def snapshot(out, dirs):
    snap = {}
    for d in dirs:
        d = pathlib.Path(d)
        entries = {}
        if d.is_dir():
            for dp, dn, fn in os.walk(d):
                for f in fn:
                    p = pathlib.Path(dp) / f
                    try:
                        st = p.stat()
                    except OSError:
                        continue
                    entries[p.relative_to(d).as_posix()] = [st.st_size, st.st_mtime_ns]
        snap[str(d)] = {"exists": d.is_dir(), "files": entries}
    pathlib.Path(out).write_text(json.dumps(snap, sort_keys=True), encoding="utf-8")
    print(f"PASS snapshot {out}: " + ", ".join(f"{pathlib.Path(k).name or k} {len(v['files'])}" for k, v in snap.items()))
    return 0


def compare(a, b):
    A = json.loads(pathlib.Path(a).read_text(encoding="utf-8"))
    B = json.loads(pathlib.Path(b).read_text(encoding="utf-8"))
    bad = 0
    for d in sorted(set(A) | set(B)):
        x, y = A.get(d, {"exists": False, "files": {}}), B.get(d, {"exists": False, "files": {}})
        if x["exists"] != y["exists"]:
            print(f"CHANGED {d}: exists {x['exists']} -> {y['exists']}")
            bad += 1
        fx, fy = x["files"], y["files"]
        for k in sorted(set(fx) | set(fy)):
            if fx.get(k) != fy.get(k):
                bad += 1
                if bad <= 20:
                    print(f"CHANGED {d}\\{k}: {'added' if k not in fx else 'removed' if k not in fy else 'size/time differ'}")
    print(f"{'PASS' if bad == 0 else 'FAIL'} unchanged: {', '.join(pathlib.Path(d).name or d for d in sorted(A))} "
          f"({bad} difference(s))")
    return 0 if bad == 0 else 1


# ---- self-test -----------------------------------------------------------------------------------------------------

def self_test():
    fails = []

    def expect(cond, what):
        if not cond:
            fails.append(what)
        print(("ok   " if cond else "FAIL ") + what)

    with tempfile.TemporaryDirectory(prefix="mw2er-release-selftest-") as tmp:
        tmp = pathlib.Path(tmp)
        repo, stage = tmp / "repo", tmp / "stage"

        def put(base, rel, data):
            p = base / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data if isinstance(data, bytes) else data.encode("utf-8"))
            return p

        dll = fake_pe(["KERNEL32.dll"], b"fake mw2er.dll")
        for base in (repo, stage):
            put(base, "build/natives/mw2er.dll", dll)
            put(base, "tools/scripts/coop_manifest.py", "# coop_manifest\n")
        put(repo, "tools/coop_friend/README.md", "# readme\n")
        put(stage, "README.md", "# readme\n")
        put(stage, "mw2er.toml", "[nuke]\nalways_available = false\n")
        put(stage, "licenses/python/LICENSE.txt", "PSF")
        put(repo, "work/installer/cache/python-3.14.7-embed-amd64/LICENSE.txt", "PSF")
        pyzip = put(stage, f"runtime/{PYTHON_EMBED_NAME}", b"PK\x03\x04 fake embeddable python")
        pins = {"python_embed_sha256": sha256_file(pyzip), "me3_installer_sha256": "0" * 64}
        dsha = hashlib.sha256(dll).hexdigest()
        man = {"what": "t", "counts": {"converted": 1, "total": 3}, "files": {
            "build/natives/mw2er.dll": {"sha256": dsha, "size": len(dll)},
            "build/rows.json": {"canon_sha256": "a" * 64, "drops": ["built_utc"]},
            "converted/weapons.json": {"sha256": "b" * 64, "size": 2}}}
        put(stage, "manifest/host_manifest.json", json.dumps(man))
        put(stage, "manifest/host_pins.json", json.dumps({"mw2er_dll_sha256": dsha, "ersc_sha256": "c" * 64,
                                                          "er_exe_version": "2.7.2.0", "witchybnd_zip_sha256": "d" * 64}))
        try:
            write_manifest(stage, "0.0.1", pins, {"regulation_builder": "regbuild"})
            expect(False, "a regbuild release without its content pins is refused")
        except SystemExit:
            expect(True, "a regbuild release without its content pins is refused")
        doc = write_manifest(stage, "0.0.1", dict(pins, package_bnd4_sha256="e" * 64, rows_json_core_sha256="f" * 64),
                             parse_notes(["regulation_builder=regbuild"]))
        expect(doc["pins"]["regulation_builder"] == "regbuild" and "fallback" in doc["sources"][0]["how"], "regbuild pins + note")
        try:
            write_manifest(stage, "0.0.1", pins, None, "../evil")
            expect(False, "a bad --update-repo is refused")
        except SystemExit:
            expect(True, "a bad --update-repo is refused")
        doc = write_manifest(stage, "0.0.1", pins, None, "owner/MW2inER")
        expect(doc["update_repo"] == "owner/MW2inER", "P3: update_repo in release.json")
        problems, _, _ = lint(stage, pins, repo)
        expect(not problems, f"a release with update_repo passes the lint {problems}")
        doc = write_manifest(stage, "0.0.1", pins)
        expect(doc["update_repo"] == "", "no update check by default (the friend's releases)")
        expect(doc["coop_id"] == coop_id(man["files"], "c" * 64) and len(doc["coop_id"]) == 8, "co-op id in release.json")
        # the Rust formula on a known input (release.rs Release::coop_id)
        expect(coop_id({"a": {"sha256": "AB"}}, "CD") == hashlib.sha256(b"a ab\nseamless cd\n").hexdigest()[:8], "co-op id formula")
        problems, _, n = lint(stage, pins, repo)
        expect(not problems, f"clean stage passes the lint ({n} files) {problems}")

        def planted(desc, fn, needle):
            s2 = tmp / "s2"
            if s2.exists():
                shutil.rmtree(s2)
            shutil.copytree(stage, s2)
            fn(s2)
            pr, _, _ = lint(s2, pins, repo)
            expect(any(needle in x for x in pr), f"lint catches {desc}")

        user = os.environ.get("USERNAME", "someone")
        planted("a game fastfile", lambda s: put(s, "zone/english/common_mp.ff", b"IWffu100"), "common_mp.ff: banned")
        planted("a Seamless dll", lambda s: put(s, "SeamlessCoop/ersc.dll", b"MZ"), "SeamlessCoop/: a folder")
        planted("a save", lambda s: put(s, "ER0000.sl2", b"BND4"), "ER0000.sl2: banned")
        planted("converted data", lambda s: put(s, "converted/weapons.json", "{}"), "converted/: a folder")
        planted("the regulation", lambda s: put(s, "build/package/regulation.bin", b"x"), "regulation.bin: banned")
        planted("an unknown file", lambda s: put(s, "notes.txt", "x"), "notes.txt: not in the release allow-list")
        planted("a changed script", lambda s: put(s, "tools/scripts/coop_manifest.py", "# changed\n"), "differs from its source")
        planted("a path of this PC", lambda s: put(s, "licenses/x.txt", f"see C:\\Users\\{user}\\Downloads"), "licenses/x.txt: contains a path")
        planted("the user name", lambda s: put(s, "licenses/y.txt", f"built by {user} today"), "licenses/y.txt: contains a path or the user name")
        planted("a Steam id", lambda s: put(s, "licenses/z.txt", "76561198000000001"), "Steam id")
        planted("a password", lambda s: put(s, "licenses/w.txt", "cooppassword = hunter2\n"), "cooppassword value")
        planted("a DDS texture", lambda s: put(s, "licenses/v.txt", b"DDS |"), "DDS texture magic")
        planted("an unpinned Python zip", lambda s: put(s, f"runtime/{PYTHON_EMBED_NAME}", b"PK other"), "not the official")
        planted("a stale release.json", lambda s: put(s, "mw2er.toml", "[nuke]\nalways_available = true\n"), "payload hashes differ")
        def bad_repo(s):
            rj = json.loads((s / "manifest/release.json").read_text(encoding="utf-8"))
            rj["update_repo"] = "https://evil.example/x"
            put(s, "manifest/release.json", json.dumps(rj))
        planted("a bad update_repo", bad_repo, "update_repo is not owner/name")
        try:
            write_notices(stage, "0.0.1")
            expect(False, "P4: notices refused without the Unlinker's source commit")
        except SystemExit:
            expect(True, "P4: notices refused without the Unlinker's source commit")
        expect(rule_for("LICENSE") == ("same", "LICENSE") and rule_for("THIRD_PARTY_NOTICES.md")[0] == "generated",
               "P4: LICENSE + THIRD_PARTY_NOTICES.md in the allow-list")
        planted("a big file", lambda s: put(s, "licenses/big.txt", b"a" * (5 * 1024 * 1024 + 1)), "> 5 MB")
        planted("a content entry in the manifest", lambda s: put(s, "manifest/host_manifest.json", json.dumps(
            {"files": {"converted/a.json": {"sha256": "b" * 64, "size": 2, "data": "x"}}})), "is not a hash entry")
        # pack / read back / damage
        zp, exe, out = tmp / "p.zip", put(tmp, "setup.exe", b"MZ program image"), tmp / "out.exe"
        write_zip(stage, zp)
        pack(exe, zp, out)
        off, ln, sha, z = read_pack(out)
        expect(off == 16 and z == zp.read_bytes() and sha == sha256_file(zp), "pack trailer round trip")
        b = bytearray(out.read_bytes())
        b[off + 30] ^= 0x55
        put(tmp, "bad.exe", bytes(b))
        try:
            read_pack(tmp / "bad.exe")
            expect(False, "damaged payload refused")
        except ValueError:
            expect(True, "damaged payload refused")
        zbytes = zp.read_bytes()
        write_zip(stage, tmp / "p2.zip")
        expect((tmp / "p2.zip").read_bytes() == zbytes, "payload zip is deterministic")
        snapshot(tmp / "a.json", [stage, tmp / "none"])
        put(stage, "README.md", "# readme 2\n")
        expect(compare(tmp / "a.json", tmp / "a.json") == 0 and snapshot(tmp / "b.json", [stage, tmp / "none"]) == 0
               and compare(tmp / "a.json", tmp / "b.json") == 1, "snapshot compare sees a change")
        # P4 review: the binary scan of the shipped programs
        pre = pc_path_patterns(repo)
        expect(pe_imports(fake_pe(["KERNEL32.dll", "VCRUNTIME140.dll"])) == ["KERNEL32.dll", "VCRUNTIME140.dll"], "P4: PE import table read")
        expect(not binary_problems("ok.exe", fake_pe(["KERNEL32.dll"]), pre), "P4: a static, clean program passes the binary scan")
        for desc, img, needle in [
                ("a VC++ runtime import", fake_pe(["KERNEL32.dll", "VCRUNTIME140.dll", "api-ms-win-crt-heap-l1-1-0.dll"]), "imports the VC++ runtime"),
                ("msvcp140", fake_pe(["MSVCP140.dll"]), "imports the VC++ runtime"),
                ("a cargo registry path (ASCII)", fake_pe(["KERNEL32.dll"], f"C:\\Users\\{user}\\.cargo\\registry\\src\\ui.rs".encode()), "a path or the user name"),
                ("a path in UTF-16 at an odd offset", fake_pe(["KERNEL32.dll"], b"x" + f"C:\\Users\\{user}\\x.rs".encode("utf-16-le")), "a path or the user name"),
                ("the project root", fake_pe(["KERNEL32.dll"], (str(repo) + r"\mod\target\release\build").encode()), "a path or the user name"),
                ("a non-PE program", b"MZ nothing", "truncated PE")]:
            expect(any(needle in x for x in binary_problems("t.exe", img, pre)), f"P4: the binary scan catches {desc}")
        zq = tmp / "q.zip"
        zq.write_bytes(b"PK\x03\x04 payload C:\\Users\\" + user.encode() + b"\\x")
        pack_src = put(tmp, "q.exe", fake_pe(["KERNEL32.dll"]))
        pack(pack_src, zq, tmp / "q-packed.exe")
        expect(not binary_problems("q.exe", (tmp / "q-packed.exe").read_bytes(), pre), "P4: the binary scan reads only the program part of a packed setup")
        crt_dll = fake_pe(["VCRUNTIME140.dll"])
        planted("our DLL with the VC++ runtime", lambda s: put(s, "build/natives/mw2er.dll", crt_dll) and put(repo, "build/natives/mw2er.dll", crt_dll),
                "build/natives/mw2er.dll: imports the VC++ runtime")
        put(repo, "build/natives/mw2er.dll", dll)
        nexus_self_test(tmp, expect)
    print(f"{'PASS' if not fails else 'FAIL'} release_tool self-test: {len(fails)} failure(s)")
    return 0 if not fails else 1


# ---- P4: the Nexus Mods upload archive and page drafts (make_nexus_package.ps1) ------------------------------------
# Sources: docs/nexus/*.md / *.txt with {{KEY}} placeholders (hand-written; the release folder is rebuilt by
# make_release.ps1, so nothing hand-written lives there). Output: <release>/nexus/. Nothing is uploaded or posted.

NEXUS_DRAFTS = ["NEXUS_PAGE.md", "NEXUS_FORM.md", "MEDIA_CHECKLIST.md", "CLEAN_PC_TEST.md"]
NEXUS_ZIP_DOCS = ["README.txt", "LICENSE", "THIRD_PARTY_NOTICES.md", "CHANGELOG.md"]
NEXUS_SECTIONS = ["Features", "Requirements", "Install", "Update", "Uninstall", "Controls", "Troubleshooting",
                  "Compatibility", "FAQ", "Credits", "License", "AI disclosure", "Disclaimer"]
NEXUS_MUST_SAY = ["not affiliated", "Activision", "Infinity Ward", "FromSoftware", "Bandai Namco", "No game files",
                  "official online mode", "Claude Code", "Anthropic", "MIT", "maskedpinata", "THIRD_PARTY_NOTICES.md",
                  "logs\\install", "logs\\me3_launch.log", "setup_logs", "logs\\mw2er.log"]
NEXUS_CREDITS = ["me3", "garyttierney", "Seamless Co-op", "LukeYui", "OpenAssetTools", "Laupetin", "iw4x-x64/oat",
                 "WitchyBND", "ividyon", "fromsoftware-rs", "vswarte", "hudhook", "erfps2", "Paramdex", "SoulsFormats",
                 "IW4L"]
NEXUS_URL_HOSTS = {"github.com", "www.nexusmods.com", "nexusmods.com", "store.steampowered.com", "www.python.org",
                   "aka.ms"}
NEXUS_SUMMARY_MAX = 350        # the Nexus "summary" field
NEXUS_FILE_DESC_MAX = 255      # kept short for the file description field
NEXUS_OUT_EXTRA = {"stage", "nexus_lint.log", "um_publish_check.log", "SHA256SUMS.txt", "NEXUS_PAGE.bbcode"}
BB_SELF_CLOSING = {"line", "*"}
PLACEHOLDER = re.compile(r"\{\{(\w+)\}\}")


def nexus_names(version):
    folder = DISPLAY_NAME.replace(" ", "-") + f"-{version}"
    return folder, folder + ".zip"


def oat_source_name(commit):
    return f"OpenAssetTools-Unlinker-source-{commit[:8]}.zip"


def render(text, vals, what):
    out = PLACEHOLDER.sub(lambda m: str(vals[m.group(1)]) if m.group(1) in vals else m.group(0), text)
    left = sorted(set(PLACEHOLDER.findall(out)))
    if left:
        raise SystemExit(f"{what}: unknown placeholder(s) {left}")
    return out


def md_to_txt(md, repo=ROOT):
    """The INSTALL_GUIDE converter of the friend package (tools/coop_friend/md2txt.py), loaded by path."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("md2txt", pathlib.Path(repo) / "tools/coop_friend/md2txt.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.convert(md)


def _bb_inline(s):
    keep = []

    def hold(text):
        keep.append(text)
        return f"\x00{len(keep) - 1}\x00"
    s = re.sub(r"`([^`]+)`", lambda m: hold(f"[font=Courier New]{m.group(1)}[/font]"), s)
    s = re.sub(r"\[([^\]]+)\]\((https?://[^)\s]+)\)", lambda m: hold(f"[url={m.group(2)}]{m.group(1)}[/url]"), s)
    s = re.sub(r"https?://[^\s)]*[^\s).,;:]", lambda m: hold(f"[url]{m.group(0)}[/url]"), s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"[b]\1[/b]", s)
    s = re.sub(r"(?<![\w*])\*([^*\s][^*]*?)\*(?![\w*])", r"[i]\1[/i]", s)
    return re.sub("\x00(\\d+)\x00", lambda m: keep[int(m.group(1))], s)


def md_to_bbcode(md):
    """The small Markdown subset of docs/nexus/NEXUS_PAGE.md -> Nexus BBCode: headings, paragraphs (hard-wrapped
    lines joined), '>' quotes, '-' / '1.' lists with one nesting level and continuation lines, '---', inline bold /
    italic / code / links. Tables and HTML comments are not used on the page (a table row becomes a list item)."""
    lines = md.splitlines()
    blocks, i = [], 0
    item_re = re.compile(r"^( *)(-|\d+\.) (.*)$")
    while i < len(lines):
        ln = lines[i]
        if not ln.strip() or ln.lstrip().startswith("<!--"):
            i += 1
            continue
        m = re.match(r"^(#{1,3}) (.*)$", ln)
        if m:
            size = {1: 6, 2: 5, 3: 4}[len(m.group(1))]
            blocks.append(f"[size={size}][b]{_bb_inline(m.group(2).strip())}[/b][/size]")
            i += 1
            continue
        if ln.strip() == "---":
            blocks.append("[line]")
            i += 1
            continue
        if ln.startswith(">"):
            q = []
            while i < len(lines) and lines[i].startswith(">"):
                q.append(lines[i][1:].strip())
                i += 1
            blocks.append("[quote]" + _bb_inline(" ".join(x for x in q if x)) + "[/quote]")
            continue
        if item_re.match(ln) or ln.startswith("|"):
            items = []
            while i < len(lines) and lines[i].strip():
                cur = lines[i]
                mm = item_re.match(cur)
                if cur.startswith("|"):
                    if not re.fullmatch(r"\|[-| :]+\|", cur.strip()):
                        cells = [c.strip() for c in cur.strip().strip("|").split("|")]
                        items.append([0, False, cells[0] + ": " + " / ".join(cells[1:])])
                elif mm:
                    items.append([min(len(mm.group(1)) // 2, 1), mm.group(2) != "-", mm.group(3).strip()])
                elif items:
                    items[-1][2] += " " + cur.strip()
                i += 1
            out, stack = [], []
            for level, ordered, text in items:
                while len(stack) > level + 1:
                    stack.pop()
                    out.append("[/list]")
                while len(stack) < level + 1:
                    stack.append(ordered)
                    out.append("[list=1]" if ordered else "[list]")
                out.append("[*]" + _bb_inline(text))
            out += ["[/list]"] * len(stack)
            blocks.append("\n".join(out))
            continue
        para = []
        while i < len(lines) and lines[i].strip() and not re.match(r"^(#{1,3} |>|---$|\||<!--)", lines[i]) \
                and not item_re.match(lines[i]):
            para.append(lines[i].strip())
            i += 1
        blocks.append(_bb_inline(" ".join(para)))
    return "\n\n".join(blocks) + "\n"


def bbcode_problems(bb):
    probs, stack = [], []
    for m in re.finditer(r"\[(/?)([a-z*]+)(?:=[^\]]*)?\]", bb):
        close, tag = m.group(1) == "/", m.group(2)
        if tag in BB_SELF_CLOSING:
            continue
        if not close:
            stack.append(tag)
        elif not stack or stack.pop() != tag:
            probs.append(f"unbalanced [/{tag}] at {m.start()}")
    probs += [f"[{t}] is never closed" for t in stack]
    if "**" in bb or "](" in bb:
        probs.append("Markdown left in the BBCode")
    return probs


def write_oat_source(out_zip, oat_dir, commit, patch, version):
    """The GPL-3.0 corresponding source of the shipped Unlinker, from the LOCAL clone (git archive; nothing is
    downloaded): the oat commit, every submodule at the commit the checkout records (refused when a submodule is not
    at its recorded commit), the local build fixes and a SOURCE.txt. Deterministic (fixed times, sorted)."""
    import tarfile
    oat_dir = pathlib.Path(oat_dir)

    def git(*a, cwd=oat_dir):
        return subprocess.run(["git", "-C", str(cwd), *a], capture_output=True, check=True).stdout

    if git("rev-parse", "HEAD").decode().strip() != commit:
        raise SystemExit(f"{oat_dir} is not at the shipped commit {commit}")
    entries, skipped = {}, []

    def add(repo_dir, rev, prefix):
        with tarfile.open(fileobj=io.BytesIO(git("archive", "--format=tar", rev, cwd=repo_dir))) as t:
            for m in t.getmembers():
                if m.isfile():
                    entries[prefix + m.name] = t.extractfile(m).read()
                elif m.issym():
                    skipped.append(prefix + m.name)

    add(oat_dir, commit, "oat/")
    subs = []
    for line in git("submodule", "status", "--recursive").decode().splitlines():
        if not line.strip():
            continue
        flag, rest = line[0], line[1:].split()
        if flag in "+-U":
            raise SystemExit(f"submodule {rest[1]} is not at its recorded commit ({line.strip()})")
        add(oat_dir / rest[1], rest[0], f"oat/{rest[1]}/")
        subs.append(f"{rest[1]} {rest[0]}")
    entries["local-build-fixes.patch"] = pathlib.Path(patch).read_bytes()
    entries["SOURCE.txt"] = (
        f"Corresponding source of the OpenAssetTools Unlinker (GPL-3.0) shipped in {DISPLAY_NAME} {version}\n"
        f"(tools/oat/bin/Unlinker.exe in the setup).\n\n"
        f"oat/                     https://github.com/iw4x-x64/oat at commit {commit}\n"
        f"                         (a fork of https://github.com/Laupetin/OpenAssetTools), with its submodules:\n"
        + "".join(f"                         {s}\n" for s in subs) +
        "local-build-fixes.patch  two build fixes for MSVC 14.44 (no behaviour change): git -C oat apply local-build-fixes.patch\n\n"
        "Build: in oat/, run the repository's generate script (premake5 vs2022, premake 5.0.0-beta8), then build the\n"
        "generated solution with msbuild, configuration Release, platform x64. The Unlinker is build/bin/Release_x64/Unlinker.exe.\n"
        "Licence: GPL-3.0 (oat/LICENSE); the third-party code under oat/thirdparty keeps its own licences.\n"
        + (f"Symbolic links not included: {', '.join(skipped)}\n" if skipped else "")).encode("utf-8")
    with zipfile.ZipFile(out_zip, "w") as z:
        for name in sorted(entries):
            zi = zipfile.ZipInfo(name, ZIP_TIME)
            zi.external_attr = 0o644 << 16
            zi.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(zi, entries[name], compresslevel=9)
    print(f"PASS GPL source {out_zip}: {len(entries)} files, {pathlib.Path(out_zip).stat().st_size / 1048576:.1f} MB, "
          f"{len(subs)} submodules")


def nexus_values(rel, version, repo=ROOT):
    rel = pathlib.Path(rel)
    rj = json.loads((rel / "release.json").read_text(encoding="utf-8"))
    p = rj["pins"]
    exe = rel / f"MW2inER-{version}-setup.exe"
    folder, zipname = nexus_names(version)
    vals = {"DISPLAY_NAME": DISPLAY_NAME, "VERSION": version, "EXE_NAME": exe.name, "EXE_SHA256": sha256_file(exe),
            "EXE_MB": f"{exe.stat().st_size / 1048576:.1f}", "COOP_ID": rj["coop_id"],
            "ER_EXE_VERSION": p["er_exe_version"], "ER_APP_VERSION": p["er_app_version"], "MW2_BUILD": p["mw2_buildid"],
            "ME3_VERSION": p["me3_version"], "SEAMLESS_VERSION": p["seamless_version"], "OAT_COMMIT": p["oat_commit"],
            "OAT_COMMIT_SHORT": p["oat_commit"][:12], "OAT_ZIP_NAME": oat_source_name(p["oat_commit"]),
            "ZIP_NAME": zipname, "FOLDER": folder}
    src = pathlib.Path(repo) / "docs/nexus"
    vals["SUMMARY"] = render((src / "SUMMARY.txt").read_text(encoding="utf-8").strip(), vals, "SUMMARY.txt")
    vals["SUMMARY_LEN"] = len(vals["SUMMARY"])
    vals["FILE_DESCRIPTION"] = render((src / "FILE_DESCRIPTION.txt").read_text(encoding="utf-8").strip(), vals,
                                      "FILE_DESCRIPTION.txt")
    vals["FILE_DESC_LEN"] = len(vals["FILE_DESCRIPTION"])
    return rj, vals


def cmd_nexus(rel, version, oat_source=False, repo=ROOT):
    """Build <rel>/nexus/: the upload zip (one folder: the setup, README.txt, LICENSE, THIRD_PARTY_NOTICES.md,
    CHANGELOG.md), the page / form / media / clean-PC drafts, NEXUS_PAGE.bbcode, SHA256SUMS.txt and optionally the
    GPL source zip of the Unlinker. Writes only <rel>/nexus/."""
    rel, repo = pathlib.Path(rel), pathlib.Path(repo)
    exe = rel / f"MW2inER-{version}-setup.exe"
    for need in (exe, rel / f"{exe.name}.sha256", rel / "release.json", rel / "LICENSE", rel / "THIRD_PARTY_NOTICES.md"):
        if not need.is_file():
            raise SystemExit(f"{need} is missing: run make_release.ps1 first")
    if (rel / f"{exe.name}.sha256").read_text(encoding="utf-8").split()[0] != sha256_file(exe):
        raise SystemExit(f"{exe.name} does not match its .sha256")
    _, _, _, z = read_pack(exe)
    with zipfile.ZipFile(io.BytesIO(z)) as zf:
        if json.loads(zf.read("manifest/release.json"))["release_version"] != version:
            raise SystemExit(f"{exe.name} carries another release than {version}")
    rj, vals = nexus_values(rel, version, repo)
    out = rel / "nexus"
    if out.exists():
        known = set(NEXUS_DRAFTS) | NEXUS_OUT_EXTRA | {vals["ZIP_NAME"], vals["OAT_ZIP_NAME"]}
        odd = [x.name for x in out.iterdir() if x.name not in known]
        if odd:
            raise SystemExit(f"{out} holds files this step did not make ({odd[:3]}): refused")
        shutil.rmtree(out)
    stage = out / "stage" / vals["FOLDER"]
    stage.mkdir(parents=True)
    src = repo / "docs/nexus"
    shutil.copyfile(exe, stage / exe.name)
    shutil.copyfile(rel / "LICENSE", stage / "LICENSE")
    shutil.copyfile(rel / "THIRD_PARTY_NOTICES.md", stage / "THIRD_PARTY_NOTICES.md")
    shutil.copyfile(repo / "CHANGELOG.md", stage / "CHANGELOG.md")
    readme = md_to_txt(render((src / "README.md").read_text(encoding="utf-8"), vals, "README.md"), repo)
    (stage / "README.txt").write_text(readme, encoding="utf-8", newline="\r\n")
    zp = out / vals["ZIP_NAME"]
    write_zip(out / "stage", zp)
    vals.update(ZIP_SHA256=sha256_file(zp), ZIP_MB=f"{zp.stat().st_size / 1048576:.1f}")
    sums = [f"{vals['ZIP_SHA256']}  {zp.name}", f"{vals['EXE_SHA256']}  {vals['FOLDER']}/{exe.name}"]
    if oat_source:
        oz = out / vals["OAT_ZIP_NAME"]
        patch = rel / "stage/MW2inER/tools/oat/local-build-fixes.patch"
        if not patch.is_file():
            raise SystemExit(f"{patch} is missing (the release stage)")
        write_oat_source(oz, repo / "tools/oat", vals["OAT_COMMIT"], patch, version)
        vals["OAT_ZIP_FACTS"] = f"({oz.stat().st_size / 1048576:.1f} MB, sha256 {sha256_file(oz)})"
        sums.append(f"{sha256_file(oz)}  {oz.name}")
    else:
        vals["OAT_ZIP_FACTS"] = "(not built in this run: make_nexus_package.ps1 builds it unless -NoOatSource)"
    for name in NEXUS_DRAFTS:
        (out / name).write_text(render((src / name).read_text(encoding="utf-8"), vals, name), encoding="utf-8",
                                newline="\n")
    (out / "NEXUS_PAGE.bbcode").write_text(md_to_bbcode((out / "NEXUS_PAGE.md").read_text(encoding="utf-8")),
                                           encoding="utf-8", newline="\n")
    (out / "SHA256SUMS.txt").write_text("\n".join(sums) + "\n", encoding="utf-8", newline="\n")
    print(f"PASS nexus drafts {out}: {zp.name} {vals['ZIP_MB']} MB sha256 {vals['ZIP_SHA256']}; summary "
          f"{vals['SUMMARY_LEN']} chars; co-op id {rj['coop_id']}")
    return 0


def _between(text, key):
    m = re.search(rf"<!-- {key}:begin -->\n(.*?)\n<!-- {key}:end -->", text, re.S)
    return m.group(1) if m else None


def nexus_lint(rel, version, repo=ROOT):
    """Returns (problems, n_checked). Read-only."""
    rel, repo = pathlib.Path(rel), pathlib.Path(repo)
    out = rel / "nexus"
    problems = []
    if not out.is_dir():
        return [f"{out} is missing"], 0
    rj, vals = nexus_values(rel, version, repo)
    folder, zipname = vals["FOLDER"], vals["ZIP_NAME"]
    known = set(NEXUS_DRAFTS) | NEXUS_OUT_EXTRA | {zipname, vals["OAT_ZIP_NAME"]}
    for x in sorted(out.iterdir()):
        if x.name not in known:
            problems.append(f"nexus/{x.name}: not a file of the Nexus step")
    for name in NEXUS_DRAFTS + ["NEXUS_PAGE.bbcode", "SHA256SUMS.txt", zipname]:
        if not (out / name).is_file():
            problems.append(f"nexus/{name} is missing")
    if problems:
        return problems, 0
    pathre = pc_path_patterns(repo)
    texts = {}

    def scan_text(where, data, links=True):
        try:
            t = data.decode("utf-8-sig")
        except UnicodeDecodeError:
            problems.append(f"{where}: not UTF-8 text")
            return ""
        if PLACEHOLDER.search(t):
            problems.append(f"{where}: unfilled placeholder {PLACEHOLDER.search(t).group(0)}")
        if pathre.search(t):
            problems.append(f"{where}: contains a path or the user name of this PC")
        if STEAM_ID.search(t):
            problems.append(f"{where}: contains a Steam id")
        if PASSWORD_LINE.search(data):
            problems.append(f"{where}: contains a cooppassword value")
        for u in re.findall(r"https?://([^/\s\])>|`*]+)", t) if links else []:
            if u.lower() not in NEXUS_URL_HOSTS:
                problems.append(f"{where}: link to an unexpected host {u}")
        return t

    # the upload archive
    exe = rel / vals["EXE_NAME"]
    want = {f"{folder}/{n}" for n in [exe.name] + NEXUS_ZIP_DOCS}
    zp = out / zipname
    n = 0
    with zipfile.ZipFile(zp) as zf:
        names = zf.namelist()
        if set(names) != want or len(names) != len(want):
            problems.append(f"{zipname}: members {sorted(set(names) ^ want)} differ from the upload layout "
                            f"({folder}/ + setup, {', '.join(NEXUS_ZIP_DOCS)})")
        if zf.testzip():
            problems.append(f"{zipname}: damaged member {zf.testzip()}")
        for zi in zf.infolist():
            n += 1
            data = zf.read(zi)
            base = zi.filename.rsplit("/", 1)[-1]
            if zi.date_time != ZIP_TIME:
                problems.append(f"{zipname}/{zi.filename}: not the fixed zip time (not deterministic)")
            if DENY_NAME.search(base):
                problems.append(f"{zipname}/{zi.filename}: banned file name")
            staged = out / "stage" / zi.filename
            if not staged.is_file() or sha256_file(staged) != hashlib.sha256(data).hexdigest():
                problems.append(f"{zipname}/{zi.filename}: differs from nexus/stage")
            if base == exe.name:
                if hashlib.sha256(data).hexdigest() != sha256_file(exe):
                    problems.append(f"{zipname}/{zi.filename}: is not the release's {exe.name}")
                problems += binary_problems(f"{zipname}/{zi.filename}", data, pathre, True)
                continue
            for magic, what_m in DENY_MAGIC:
                if data.startswith(magic):
                    problems.append(f"{zipname}/{zi.filename}: {what_m} magic")
            # the notices link every crate's own repository; the hand-written texts link only the known hosts
            texts[base] = scan_text(f"{zipname}/{zi.filename}", data, base == "README.txt")
            same = {"LICENSE": rel / "LICENSE", "THIRD_PARTY_NOTICES.md": rel / "THIRD_PARTY_NOTICES.md",
                    "CHANGELOG.md": repo / "CHANGELOG.md"}.get(base)
            if same is not None and (not same.is_file() or sha256_file(same) != hashlib.sha256(data).hexdigest()):
                problems.append(f"{zipname}/{zi.filename}: differs from {same}")
    if (rel / "LICENSE").is_file() and (repo / "LICENSE").is_file() and sha256_file(rel / "LICENSE") != sha256_file(repo / "LICENSE"):
        problems.append("the release's LICENSE differs from the repository's")
    lic = texts.get("LICENSE", "")
    if not lic.startswith("MIT License") or "2026 maskedpinata" not in lic:
        problems.append("LICENSE: not the MIT licence of maskedpinata 2026")
    if f"## {version}" not in texts.get("CHANGELOG.md", ""):
        problems.append(f"CHANGELOG.md: no '## {version}' section")
    rd = texts.get("README.txt", "")
    for s in (DISPLAY_NAME, version, "not affiliated", "No game files", vals["EXE_SHA256"], "MIT", "Claude Code"):
        if s not in rd:
            problems.append(f"README.txt: does not say '{s}'")
    try:
        read_pack(exe)
    except ValueError as e:
        problems.append(f"{exe.name}: {e}")
    # the drafts
    for name in NEXUS_DRAFTS + ["NEXUS_PAGE.bbcode", "SHA256SUMS.txt"]:
        texts[name] = scan_text(f"nexus/{name}", (out / name).read_bytes())
        n += 1
    page, form, bb = texts["NEXUS_PAGE.md"], texts["NEXUS_FORM.md"], texts["NEXUS_PAGE.bbcode"]
    for bad in ("Call of Duty", "Modern Warfare", "MW2", "Activision"):
        if bad.lower() in DISPLAY_NAME.lower():
            problems.append(f"the display name uses '{bad}'")
    if not page.startswith(f"# {DISPLAY_NAME}\n"):
        problems.append("NEXUS_PAGE.md: does not start with the display name")
    for sec in NEXUS_SECTIONS:
        if not re.search(rf"(?m)^## {re.escape(sec)}$", page):
            problems.append(f"NEXUS_PAGE.md: no '## {sec}' section")
    for s in NEXUS_MUST_SAY + NEXUS_CREDITS:
        if s not in page:
            problems.append(f"NEXUS_PAGE.md: does not say '{s}'")
    if vals["SUMMARY_LEN"] > NEXUS_SUMMARY_MAX:
        problems.append(f"summary: {vals['SUMMARY_LEN']} characters (Nexus allows {NEXUS_SUMMARY_MAX})")
    if vals["SUMMARY"] not in page or _between(form, "summary") != vals["SUMMARY"]:
        problems.append("summary: NEXUS_PAGE.md / NEXUS_FORM.md do not carry SUMMARY.txt exactly")
    if vals["FILE_DESC_LEN"] > NEXUS_FILE_DESC_MAX or _between(form, "file-description") != vals["FILE_DESCRIPTION"]:
        problems.append(f"file description: {vals['FILE_DESC_LEN']} characters (max {NEXUS_FILE_DESC_MAX}) or not in NEXUS_FORM.md")
    zsha = sha256_file(zp)
    for s in (zsha, zipname, "No assets from other games are included", "AI-generated content", "maskedpinata"):
        if s not in form:
            problems.append(f"NEXUS_FORM.md: does not say '{s}'")
    if not re.search(r"Contains AI-generated content[^|\n]*\| \*\*Yes\*\*", form):
        problems.append("NEXUS_FORM.md: the AI content flag is not Yes")
    problems += [f"NEXUS_PAGE.bbcode: {x}" for x in bbcode_problems(bb)]
    if bb != md_to_bbcode(page):
        problems.append("NEXUS_PAGE.bbcode: not generated from NEXUS_PAGE.md")
    for s in ("showcase-video", "key art", "logos"):
        if s not in texts["MEDIA_CHECKLIST.md"]:
            problems.append(f"MEDIA_CHECKLIST.md: does not say '{s}'")
    for s in (vals["EXE_SHA256"], f"INSTALL PASSED: co-op id {vals['COOP_ID']}", "logs\\install", "me3_launch.log",
              "mw2er.log", "What to send back"):
        if s not in texts["CLEAN_PC_TEST.md"]:
            problems.append(f"CLEAN_PC_TEST.md: does not say '{s}'")
    sums = {}
    for line in texts["SHA256SUMS.txt"].splitlines():
        h, _, name = line.partition("  ")
        sums[name] = h
    if sums.get(zipname) != zsha or sums.get(f"{folder}/{exe.name}") != sha256_file(exe):
        problems.append("SHA256SUMS.txt: does not match the archive / the setup")
    oz = out / vals["OAT_ZIP_NAME"]
    if oz.is_file():
        n += 1
        if sums.get(oz.name) != sha256_file(oz):
            problems.append(f"SHA256SUMS.txt: does not match {oz.name}")
        with zipfile.ZipFile(oz) as zf:
            nm = set(zf.namelist())
            if not {"SOURCE.txt", "local-build-fixes.patch", "oat/premake5.lua", "oat/LICENSE"} <= nm:
                problems.append(f"{oz.name}: not the Unlinker source layout")
            elif vals["OAT_COMMIT"] not in zf.read("SOURCE.txt").decode("utf-8"):
                problems.append(f"{oz.name}: SOURCE.txt does not name commit {vals['OAT_COMMIT']}")
    return problems, n


def cmd_nexus_lint(rel, version):
    problems, n = nexus_lint(rel, version)
    for x in problems:
        print("FAIL " + x)
    print(f"{'PASS' if not problems else 'FAIL'} nexus lint: {n} files; {len(problems)} problem(s); upload layout, "
          "release setup hash, licence / notices / changelog sources, PC paths / user name / Steam ids / passwords, "
          "placeholders, summary <= 350, page sections / disclaimer / credits / AI disclosure, BBCode, link hosts")
    return 0 if not problems else 1


def nexus_self_test(tmp, expect):
    """Builds a fake release + repo (the real docs/nexus templates and md2txt.py) and checks the Nexus step."""
    repo, rel = tmp / "nrepo", tmp / "nrel"
    shutil.copytree(ROOT / "docs/nexus", repo / "docs/nexus")
    (repo / "tools/coop_friend").mkdir(parents=True)
    shutil.copyfile(ROOT / "tools/coop_friend/md2txt.py", repo / "tools/coop_friend/md2txt.py")
    rel.mkdir()
    lic = MIT_TEXT.format(who="2026 maskedpinata")
    for base in (repo, rel):
        (base / "LICENSE").write_text(lic + "\n", encoding="utf-8")
    (repo / "CHANGELOG.md").write_text("# Changelog\n\n## 0.0.1 - test\n\n- first\n", encoding="utf-8")
    (rel / "THIRD_PARTY_NOTICES.md").write_text("# Third-party notices\n", encoding="utf-8")
    pins = {"er_exe_version": "2.7.2.0", "er_app_version": "Steam build 1", "mw2_buildid": "2", "me3_version": "0.13.0",
            "seamless_version": "2.0.1", "oat_commit": "a" * 40}
    rj = {"release_version": "0.0.1", "coop_id": "1234abcd", "pins": pins}
    (rel / "release.json").write_text(json.dumps(rj), encoding="utf-8")
    zb = io.BytesIO()
    with zipfile.ZipFile(zb, "w") as z:
        z.writestr("manifest/release.json", json.dumps(rj))
    (tmp / "n.zip").write_bytes(zb.getvalue())
    (tmp / "n.exe").write_bytes(fake_pe(["KERNEL32.dll"]))
    exe = rel / "MW2inER-0.0.1-setup.exe"
    pack(tmp / "n.exe", tmp / "n.zip", exe)
    (rel / f"{exe.name}.sha256").write_text(f"{sha256_file(exe)}  {exe.name}\n", encoding="utf-8")
    cmd_nexus(rel, "0.0.1", False, repo)
    pr, _ = nexus_lint(rel, "0.0.1", repo)
    expect(not pr, f"P4 nexus: a clean build passes the nexus lint {pr}")
    folder, zipname = nexus_names("0.0.1")
    zbytes = (rel / "nexus" / zipname).read_bytes()
    cmd_nexus(rel, "0.0.1", False, repo)
    expect((rel / "nexus" / zipname).read_bytes() == zbytes, "P4 nexus: the upload zip is deterministic")
    with zipfile.ZipFile(rel / "nexus" / zipname) as zf:
        expect(sorted(zf.namelist()) == sorted(f"{folder}/{x}" for x in [exe.name] + NEXUS_ZIP_DOCS),
               "P4 nexus: upload layout = one folder with the setup + 4 documents")
    bb = md_to_bbcode("# T\n\nA **b** `c` [d](https://github.com/x)\nwrapped.\n\n- one\n  more\n  - two\n- three\n")
    expect(bb == "[size=6][b]T[/b][/size]\n\nA [b]b[/b] [font=Courier New]c[/font] [url=https://github.com/x]d[/url] "
                 "wrapped.\n\n[list]\n[*]one more\n[list]\n[*]two\n[/list]\n[*]three\n[/list]\n" and not bbcode_problems(bb),
           "P4 nexus: Markdown -> BBCode")
    user = os.environ.get("USERNAME", "someone")

    def planted(desc, fn, needle, rebuild=False):
        fn()
        if rebuild:
            try:
                cmd_nexus(rel, "0.0.1", False, repo)
            except SystemExit as e:
                pr2 = [str(e)]
            else:
                pr2, _ = nexus_lint(rel, "0.0.1", repo)
        else:
            pr2, _ = nexus_lint(rel, "0.0.1", repo)
        expect(any(needle in x for x in pr2), f"P4 nexus lint catches {desc}")

    out = rel / "nexus"
    keep_page = (out / "NEXUS_PAGE.md").read_text(encoding="utf-8")
    planted("a path of this PC", lambda: (out / "MEDIA_CHECKLIST.md").write_text(f"C:\\Users\\{user}\\x showcase-video key art logos\n", encoding="utf-8"),
            "MEDIA_CHECKLIST.md: contains a path")
    planted("an unfilled placeholder", lambda: (out / "CLEAN_PC_TEST.md").write_text("{{NOPE}}\n", encoding="utf-8"), "unfilled placeholder")
    planted("broken BBCode", lambda: (out / "NEXUS_PAGE.bbcode").write_text("[b]x\n", encoding="utf-8"), "never closed")
    planted("a missing page section", lambda: (out / "NEXUS_PAGE.md").write_text(keep_page.replace("## FAQ", "## Questions"), encoding="utf-8"),
            "no '## FAQ' section")
    planted("a stray file", lambda: (out / "notes.txt").write_text("x", encoding="utf-8"), "not a file of the Nexus step")
    (out / "notes.txt").unlink()
    planted("a refused rebuild over a stray file", lambda: (out / "notes.txt").write_text("x", encoding="utf-8"), "refused", True)
    (out / "notes.txt").unlink()
    s = (repo / "docs/nexus/SUMMARY.txt")
    planted("a summary over 350 characters", lambda: s.write_text("x" * 351 + "\n", encoding="utf-8"), "summary: 351 characters", True)
    s.write_text((ROOT / "docs/nexus/SUMMARY.txt").read_text(encoding="utf-8"), encoding="utf-8")
    cmd_nexus(rel, "0.0.1", False, repo)
    keep_exe = exe.read_bytes()
    (tmp / "nleak.exe").write_bytes(fake_pe(["KERNEL32.dll", "VCRUNTIME140.dll"], f"C:\\Users\\{user}\\.cargo\\x.rs".encode()))
    pack(tmp / "nleak.exe", tmp / "n.zip", exe)
    (rel / f"{exe.name}.sha256").write_text(f"{sha256_file(exe)}  {exe.name}\n", encoding="utf-8")
    cmd_nexus(rel, "0.0.1", False, repo)
    pr4, _ = nexus_lint(rel, "0.0.1", repo)
    expect(any("the program contains a path" in x for x in pr4) and any("imports the VC++ runtime" in x for x in pr4),
           "P4 nexus lint catches a setup with a PC path / the VC++ runtime")
    exe.write_bytes(keep_exe)
    (rel / f"{exe.name}.sha256").write_text(f"{sha256_file(exe)}  {exe.name}\n", encoding="utf-8")
    cmd_nexus(rel, "0.0.1", False, repo)
    b = bytearray((tmp / "n.exe").read_bytes())
    exe.write_bytes(bytes(b) + b"x")
    pr3, _ = nexus_lint(rel, "0.0.1", repo)
    expect(any("is not the release's" in x for x in pr3), "P4 nexus lint catches a setup that differs from the release")


def main(argv):
    def opt_list(name):
        out, i = [], 0
        while i < len(argv):
            if argv[i] == name and i + 1 < len(argv):
                out.append(argv[i + 1])
                i += 2
            else:
                i += 1
        return out

    pos = []
    i = 0
    while i < len(argv):
        if argv[i] in ("--pin", "--version", "--note", "--update-repo"):
            i += 2
        elif argv[i] == "--roots":
            break
        elif argv[i] == "--oat-source":
            i += 1
        else:
            pos.append(argv[i])
            i += 1
    cmd = pos[0] if pos else ""
    if cmd == "manifest" and len(pos) == 2:
        v = opt_list("--version")
        if len(v) != 1:
            raise SystemExit("--version V")
        ur = opt_list("--update-repo")
        write_manifest(pos[1], v[0], parse_pins(opt_list("--pin")), parse_notes(opt_list("--note")), ur[0] if ur else "")
        return 0
    if cmd == "notices" and len(pos) == 2:
        v = opt_list("--version")
        if len(v) != 1:
            raise SystemExit("--version V")
        return write_notices(pos[1], v[0])
    if cmd == "lint" and len(pos) == 2:
        return cmd_lint(pos[1], parse_pins(opt_list("--pin")))
    if cmd == "licenses" and len(pos) == 3 and "--roots" in argv:
        return write_licenses(pos[1], pos[2], argv[argv.index("--roots") + 1:])
    if cmd == "zip" and len(pos) == 3:
        return write_zip(pos[1], pos[2])
    if cmd == "pack" and len(pos) == 4:
        return pack(pos[1], pos[2], pos[3])
    if cmd == "check-pack" and len(pos) == 2:
        return check_pack(pos[1])
    if cmd == "snapshot" and len(pos) >= 3:
        return snapshot(pos[1], pos[2:])
    if cmd == "compare" and len(pos) == 3:
        return compare(pos[1], pos[2])
    if cmd in ("nexus", "nexus-lint") and len(pos) == 2:
        v = opt_list("--version")
        if len(v) != 1:
            raise SystemExit("--version V")
        if cmd == "nexus":
            return cmd_nexus(pos[1], v[0], "--oat-source" in argv)
        return cmd_nexus_lint(pos[1], v[0])
    if cmd == "binary-lint" and len(pos) >= 2:
        return cmd_binary_lint(pos[1:])
    if cmd == "self-test":
        return self_test()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
