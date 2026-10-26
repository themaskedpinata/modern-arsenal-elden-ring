"""Run a console program inside a Windows pseudo console (ConPTY) and capture its output.

Usage: python -I conpty_run.py [--timeout S] [--log FILE] [--cwd DIR] -- exe args...
Prints the child's output (ANSI escapes stripped) and exits with the child's exit code.
Needed because WitchyBND writes nothing when its stdout is a pipe or file and no console is attached.
"""
import ctypes
import ctypes.wintypes as wt
import os
import re
import subprocess
import sys
import threading
import time

k32 = ctypes.WinDLL("kernel32", use_last_error=True)

HPCON = wt.HANDLE
PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = 0x00020016
EXTENDED_STARTUPINFO_PRESENT = 0x00080000
CREATE_UNICODE_ENVIRONMENT = 0x00000400
INFINITE = 0xFFFFFFFF
WAIT_TIMEOUT = 0x102
STILL_ACTIVE = 259


class COORD(ctypes.Structure):
    _fields_ = [("X", wt.SHORT), ("Y", wt.SHORT)]


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wt.DWORD), ("lpReserved", wt.LPWSTR), ("lpDesktop", wt.LPWSTR), ("lpTitle", wt.LPWSTR),
        ("dwX", wt.DWORD), ("dwY", wt.DWORD), ("dwXSize", wt.DWORD), ("dwYSize", wt.DWORD),
        ("dwXCountChars", wt.DWORD), ("dwYCountChars", wt.DWORD), ("dwFillAttribute", wt.DWORD),
        ("dwFlags", wt.DWORD), ("wShowWindow", wt.WORD), ("cbReserved2", wt.WORD),
        ("lpReserved2", ctypes.c_void_p), ("hStdInput", wt.HANDLE), ("hStdOutput", wt.HANDLE),
        ("hStdError", wt.HANDLE),
    ]


class STARTUPINFOEXW(ctypes.Structure):
    _fields_ = [("StartupInfo", STARTUPINFOW), ("lpAttributeList", ctypes.c_void_p)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wt.HANDLE), ("hThread", wt.HANDLE), ("dwProcessId", wt.DWORD),
                ("dwThreadId", wt.DWORD)]


k32.CreatePipe.argtypes = [ctypes.POINTER(wt.HANDLE), ctypes.POINTER(wt.HANDLE), ctypes.c_void_p, wt.DWORD]
k32.CreatePseudoConsole.argtypes = [COORD, wt.HANDLE, wt.HANDLE, wt.DWORD, ctypes.POINTER(HPCON)]
k32.CreatePseudoConsole.restype = ctypes.c_long
k32.ClosePseudoConsole.argtypes = [HPCON]
k32.InitializeProcThreadAttributeList.argtypes = [ctypes.c_void_p, wt.DWORD, wt.DWORD, ctypes.POINTER(ctypes.c_size_t)]
k32.UpdateProcThreadAttribute.argtypes = [ctypes.c_void_p, wt.DWORD, ctypes.c_size_t, ctypes.c_void_p,
                                          ctypes.c_size_t, ctypes.c_void_p, ctypes.c_void_p]
k32.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
k32.CreateProcessW.argtypes = [wt.LPCWSTR, wt.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wt.BOOL, wt.DWORD,
                               ctypes.c_void_p, wt.LPCWSTR, ctypes.c_void_p, ctypes.c_void_p]
k32.ReadFile.argtypes = [wt.HANDLE, ctypes.c_void_p, wt.DWORD, ctypes.POINTER(wt.DWORD), ctypes.c_void_p]
k32.WriteFile.argtypes = [wt.HANDLE, ctypes.c_void_p, wt.DWORD, ctypes.POINTER(wt.DWORD), ctypes.c_void_p]
k32.WaitForSingleObject.argtypes = [wt.HANDLE, wt.DWORD]
k32.WaitForSingleObject.restype = wt.DWORD
k32.GetExitCodeProcess.argtypes = [wt.HANDLE, ctypes.POINTER(wt.DWORD)]
k32.TerminateProcess.argtypes = [wt.HANDLE, wt.UINT]
k32.CloseHandle.argtypes = [wt.HANDLE]
k32.SetStdHandle.argtypes = [wt.DWORD, wt.HANDLE]

ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()][A-Za-z0-9]|\x1b[=>]")


def main():
    argv = sys.argv[1:]
    timeout = 600.0
    log = None
    cwd = None
    stdmode = "usestd"
    while argv and argv[0] != "--":
        a = argv.pop(0)
        if a == "--timeout":
            timeout = float(argv.pop(0))
        elif a == "--log":
            log = argv.pop(0)
        elif a == "--cwd":
            cwd = argv.pop(0)
        elif a == "--stdmode":
            stdmode = argv.pop(0)
        else:
            sys.exit(f"unknown option {a}")
    if not argv:
        sys.exit("usage: conpty_run.py [--timeout S] [--log FILE] [--cwd DIR] -- exe args...")
    argv.pop(0)
    cmdline = subprocess.list2cmdline(argv)

    in_r, in_w, out_r, out_w = wt.HANDLE(), wt.HANDLE(), wt.HANDLE(), wt.HANDLE()
    if not k32.CreatePipe(ctypes.byref(in_r), ctypes.byref(in_w), None, 0):
        raise ctypes.WinError(ctypes.get_last_error())
    if not k32.CreatePipe(ctypes.byref(out_r), ctypes.byref(out_w), None, 0):
        raise ctypes.WinError(ctypes.get_last_error())
    hpc = HPCON()
    hr = k32.CreatePseudoConsole(COORD(220, 60), in_r, out_w, 0, ctypes.byref(hpc))
    if hr != 0:
        sys.exit(f"CreatePseudoConsole failed hr=0x{hr & 0xffffffff:08x}")
    k32.CloseHandle(in_r)
    k32.CloseHandle(out_w)

    size = ctypes.c_size_t(0)
    k32.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(size))
    attr = ctypes.create_string_buffer(size.value)
    if not k32.InitializeProcThreadAttributeList(attr, 1, 0, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    if not k32.UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hpc,
                                         ctypes.sizeof(HPCON), None, None):
        raise ctypes.WinError(ctypes.get_last_error())

    si = STARTUPINFOEXW()
    si.StartupInfo.cb = ctypes.sizeof(STARTUPINFOEXW)
    if stdmode == "usestd":
        si.StartupInfo.dwFlags |= 0x100  # STARTF_USESTDHANDLES with NULL handles
    elif stdmode == "nullparent":
        # Clear our own std handles so the child copies NULLs and the console attach fills them in.
        for n in (-10, -11, -12):
            k32.SetStdHandle(wt.DWORD(n & 0xffffffff), None)
    si.lpAttributeList = ctypes.cast(attr, ctypes.c_void_p)
    pi = PROCESS_INFORMATION()
    buf = ctypes.create_unicode_buffer(cmdline)
    if not k32.CreateProcessW(None, buf, None, None, False,
                              EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                              None, cwd, ctypes.byref(si), ctypes.byref(pi)):
        raise ctypes.WinError(ctypes.get_last_error())

    chunks = []

    def reader():
        b = ctypes.create_string_buffer(65536)
        n = wt.DWORD()
        while True:
            ok = k32.ReadFile(out_r, b, 65536, ctypes.byref(n), None)
            if not ok or n.value == 0:
                break
            chunks.append(b.raw[: n.value])

    t = threading.Thread(target=reader, daemon=True)
    t.start()

    deadline = time.time() + timeout
    timed_out = False
    while True:
        r = k32.WaitForSingleObject(pi.hProcess, 200)
        if r != WAIT_TIMEOUT:
            break
        if time.time() > deadline:
            timed_out = True
            k32.TerminateProcess(pi.hProcess, 0xDEAD)
            break
    code = wt.DWORD()
    k32.GetExitCodeProcess(pi.hProcess, ctypes.byref(code))
    time.sleep(0.3)
    k32.ClosePseudoConsole(hpc)
    t.join(5)
    k32.CloseHandle(in_w)
    k32.CloseHandle(out_r)
    k32.CloseHandle(pi.hProcess)
    k32.CloseHandle(pi.hThread)
    k32.DeleteProcThreadAttributeList(attr)

    raw = b"".join(chunks).decode("utf-8", "replace")
    text = ANSI.sub("", raw).replace("\r\n", "\n").replace("\r", "\n")
    text = "\n".join(line.rstrip() for line in text.split("\n"))
    text = re.sub(r"\n{3,}", "\n\n", text)
    sys.stdout.write(text + "\n")
    if log:
        try:
            with open(log, "w", encoding="utf-8") as f:
                f.write(text)
        except OSError as e:
            sys.stdout.write(f"[conpty_run] could not write log {log}: {e}\n")
    if timed_out:
        sys.stdout.write(f"[conpty_run] TIMEOUT after {timeout}s, child terminated\n")
        sys.exit(124)
    sys.stdout.write(f"[conpty_run] exit code {code.value} (0x{code.value:08x})\n")
    sys.exit(code.value if code.value < 256 else 255)


if __name__ == "__main__":
    main()
