# C4 design: draw the MW2 viewmodel under Elden Ring's UI

Status: design only (2026-10-20). Nothing in this document has run in game. It answers the open item from C3 PART 2
and the 2026-10-20 solo tests: ER's messages ("Sit at grace", item pick-ups, subtitles, co-op messages) still appear
behind the gun. `[ui_yield] mode = "off"` is set (the user wants the gun to stay put), so the C3 workaround does nothing.

Sources: the three C4 research notes. `work\c4\hal_re.md` covers the Scaleform HAL static RE, with raw dumps in
`work\c4\*.txt` and the read-only tool `work\c4\haltool`. `work\c4\render_map.md` covers our renderer.
`work\c4\probe_map.md` covers the C3 probe, hudhook, ui_yield and the per-build tables. This document builds on
C3_DESIGN.md 2.1-2.4 and corrects 2.1 (see 1.1).

## Evidence labels

- **[V]** verified from bytes in eldenring.exe 2.7.2.0, from our code, or from our logs.
- **[I]** inferred. Every [I] that the injection depends on gets a probe line (section 2). One short run must confirm
  or refute it before the injection can be switched on.

The design re-read the 2.7.2.0 exe file on 2026-10-20 with plain Python `-I` (no capstone or pefile, nothing
installed, nothing launched) and confirmed these bytes:

- the UiBegin, UiEnd and UI job prologues;
- FS HAL vtable slots +0x18/+0x58/+0x60/+0x70/+0x78/+0x80/+0x390;
- UiBegin's full body (0x170 bytes);
- the sections of G `0x1447F6400` (.data), the direct queue global `0x144808348` (.data) and the FS vtable (.rdata).

## Hard rules (apply to every C4 step)

- Never launch Elden Ring, Steam, me3 or Seamless. eldenring.exe is read only, as a file. Never write into a game
  folder or `%APPDATA%\EldenRing`. Downloads only as cargo crates already in Cargo.lock. Do not pip install anything.
- C4 makes **no game-memory writes**. Every game read is non-faulting (`game::read`). If a write is ever added, it
  goes through `game::write`.
- Every hook is MinHook with a prologue byte check (the cam_hook.rs pattern). Every hook is disabled and removed at
  unload and when the feature trips (S1). C4 also fixes the C3 probe hooks, which are leaked today with `forget()`.
- A mistake here is a device-removed crash in co-op. Any doubt -> draw at Present that frame (today's behaviour) and log it.

## 0. Decision at a glance

| # | Item | Decision | Label |
|---|------|----------|-------|
| 1 | Where to inject | Our **own** command list (not Scaleform's), recorded in a detour on ER's UiBegin `0x140d80900`. It runs on the render thread just before Scaleform's first UI list. | hook point [V] |
| 2 | When it executes | Inside hudhook's ExecuteCommandLists hook (new patch 0002). It is split into the **same** ECL call that carries the Scaleform list armed at UiBegin, directly ahead of that list on the same queue. So the next thing the queue sees after our list is ER's own `0xC0 -> RENDER_TARGET` barrier on the target. | split mechanics [V D3D12 rules], adjacency [I -> probe] |
| 3 | Target | The resource UiBegin itself barriers. That is S+0xF0 (the scene image) on the SDR path, or S+0x100 (the UI layer) on the HDR path. Its state `0xC0` at that point is [V] from ER's own barrier. The SDR path is on with the flag; the HDR UI-layer path needs a second cold flag and a UNORM format. | [V] state, HDR visuals [I] |
| 4 | State save/restore | None needed on ER's or Scaleform's lists, because D3D12 lists do not inherit state. Our list barriers the target `0xC0 -> 4 -> 0xC0`, so the queue-order state is unchanged. | [V] |
| 5 | Fallback | Any precondition miss means no early draw that frame. Present draws the gun as today. Never both, never neither (a per-frame state machine). Unexpected failures trip the feature off for the session. | design |
| 6 | Switch | `[ui] inject_viewmodel = false`, **cold to turn on** (installs hooks at boot after the version gate) and **hot to turn off** (passthrough at once, hooks disabled). | design |
| 7 | Builds | 2.7.2.0 only. 2.7.1.0 has no values because that exe is gone (MODLOG 2026-10-20), so `UI inject off why=build` and the old behaviour apply. | [V] |
| 8 | Probe first | An extended read-only probe (`[ui] probe_scaleform = true`) runs once in game. `analyze_c4.py` turns its log into PASS/FAIL per [I]. The injection may only be turned on after `C4 PROBE VERDICT: READY`. | design |

**Order**: PASS P (probe + teardown fixes + patch 0002 in read-only mode) -> in-game probe run (user) ->
PASS I (renderer refactor + injection behind the flag, vmtest) -> solo in-game test (user) -> co-op test (user).
Each pass ends with `check_c4.ps1` PASS and a staged DLL. The friend package does not change unless the user wants
the friend to have it (the default is off either way).

---

## 1. What is known

### 1.1 Verified (2.7.2.0)

C3 2.1 was wrong about the "HAL vtable 0x142c09378". The live HAL object uses **FromSoftware's subclass vtable
`0x142c0bb40`** (ctor 0x140eb8d20, object 0x1870 bytes). It overrides BeginScene/EndScene (0x140eb9b80 / 0x140eb9c30),
which call the base functions 0x14115fda0 / 0x14115ff00 directly. That is why the C3 probe addresses are still right.

UiBegin `0x140d80900` (owner in rcx) does the following [V, bytes re-read]:

1. `hal = [owner+0x968]`; returns if `hal == 0` or `[owner+0xBE0] == 0` (no movies).
2. `hal->SetRenderTarget(owner+0xDD0, true)` (vt+0x18), then `hal->BeginFrame()` (vt+0x58).
3. `S = [0x1447F6400] + 0x128`. If (`byte [S+0x13c]` or `byte [S+0x13e]`) and `[S+0x100] != 0`, then `sel = S+0x100`;
   otherwise `sel = S+0xF0`. Then `obj = (sel+0x70)->vtbl[1](0)` and `res = [[obj+8]+0x20]`. The depth/stencil comes
   from `0x141ae26d0([S+0xE8], 0)`.
4. `[hal+0x1420]->ResourceBarrier(2, ...)` (`call [rax+0xD0]`):
   - colour `res`: all subresources, **0xC0 -> 4**;
   - DS: subresource 1, 0xC0 -> 0x10.

UiEnd `0x140d80a70` makes the reverse barriers, then calls EndFrame. Scaleform never clears the target. Scaleform
submits its own lists with `[hal+0x1418]->ExecuteCommandLists(1, ...)` inline at every EndScene and at EndFrame, on
the render thread (`GetCurrentThreadId() == [hal+0x50]`). The queue is ER's DIRECT queue `[0x144808348]`.

HAL fields: flags +0x58 (1 ModeSet, 2 InFrame, 4 InScene), render thread id +0x50, device +0x1410, queue +0x1418,
current list +0x1420.

Our side [V]: everything we draw records into hudhook's list at Present. That list executes after ER's frame, so it
is above ER's UI. The renderer sets all its own state (root signature, heaps, PSO, RTV/DSV, viewport) and has its own
D32 depth. The live back buffer is R10G10B10A2_UNORM 3840x2160. The colour space was never seen
(`render_map.md` 2). The DLL has **no unload path** today: `DllMain` only handles PROCESS_ATTACH, and the ui_probe
and pad hooks are `forget()`-ed.

### 1.2 Inferred (each gets a probe line in 2.3)

| id | Assumption | Why it matters |
|----|------------|----------------|
| I1 | UiBegin runs about once per Present, on ER's render thread, and that thread also calls Present (hudhook's callback thread). | The per-frame state machine (5.1) is only race-free on one thread. |
| I2 | The HAL vtable is 0x142c0bb40 at run time, `[hal+0x1418]` is the queue hudhook sees at Present, and `[hal+0x1410]` is the same device as ours. | Our list must be created on ER's device and submitted on Scaleform's queue. |
| I3 | `(sel+0x70)->vtbl[1]` is a plain getter, so we can read the chain without calling it. | We never call ER virtuals from the detour. |
| I4 | The selected resource is a single-sample 2D texture with ALLOW_RENDER_TARGET, in a known UNORM format, and its size is about the back-buffer size. | Format and size handling (section 4). |
| I5 | The list `[hal+0x1420]` read **after** UiBegin returns (the list holding ER's 0xC0->4 barrier) is submitted on `[hal+0x1418]`, from the render thread, before UiEnd. No other ECL on that queue comes between UiBegin and it. | This gives the adjacency: our list runs directly ahead of ER's barrier. |
| I6 | Which path the user runs: SDR (`sel = S+0xF0`) or HDR (`sel = S+0x100`). | Only the SDR path is on by default. |
| I7 | On the HDR path, nothing clears S+0x100 on the GPU after UiBegin, and the composite treats alpha 1 as opaque UI. | Visual only, not a crash risk. It is checked in the solo test. |
| I8 | Frames with `[owner+0xBE0] == 0` (no movies) are rare during play. | It only decides how often the fallback runs. |

Item (a) of hal_re (whether the scene is submitted before UiBegin) **no longer decides correctness**. Our list is
spliced directly ahead of the Scaleform list that carries ER's own `0xC0 -> 4` barrier. Whatever ER submitted
earlier is already ahead of us, and anything ER does after is unchanged. If ER wrote the scene after the UI, ER's own
UI would be hidden too, and it is not. A GPU wait or a scene ECL that falls between UiBegin and that list would show
up as I5 FAIL.

---

## 2. Extended read-only probe (PASS P)

### 2.1 What it hooks

`[ui] probe_scaleform = true` (cold, as in C3) installs the following. All of it is read-only, and there are no game writes.

- The existing C3 BeginScene/EndScene detours (unchanged behaviour). The hook handles are now stored and removed in teardown (S1).
- **New: UiBegin `0x140d80900`** (prologue 16 B: `40 53 48 83 ec 60 48 8b d9 48 8b 89 68 09 00 00`).
  **UiEnd `0x140d80a70`** (prologue 16 B: `48 89 74 24 10 57 48 83 ec 60 48 8b b1 68 09 00`).
  Neither prologue has a rip-relative or rel32 instruction. The detour calls the original, then reads the following
  non-faulting into atomics (no alloc, no log, no lock in the detour):
  - owner, `hal`, `[hal]`, `[hal+0x58]`, `[hal+0x50]`, `GetCurrentThreadId()`, `[owner+0xBE0]`;
  - `[hal+0x1420]` after the original, saved as `ARMED_LIST` for this frame, plus `[hal+0x1418]` and `[hal+0x1410]`;
  - the S bytes +0x13c/+0x13e, `[S+0x100]`, and the selected `sel`;
  - the getter `vtbl[1]` of `sel+0x70` and its first 16 bytes. The plain-getter test is that the bytes match
    `48 8b 41 xx c3` or `48 8b 81 xx xx xx xx c3`, or the `lea` forms. If it is plain, res is read through the chain.
  - Once per 5 s, only if the resource pointer's vtable lies in the same module as the vtable of our own depth
    texture (`GetModuleHandleExW(FROM_ADDRESS)`): `res->GetDesc()` (a read-only COM call through a non-owning
    `ManuallyDrop`). The same for the other candidate (S+0xF0 or S+0x100).
- **hudhook patch 0002, read-only in PASS P** (`tools\patches\hudhook-0002-ecl-callback.patch`, applied to
  `vendor\hudhook-0.9.3`). In `d3d12_command_queue_execute_command_lists_impl`, before the trampoline, it calls
  `pre_imgui::ECL_HOOK` (a `OnceLock<fn(...)>`). Present increments `pre_imgui::PRESENT_SEQ` and records the present
  thread id. In PASS P the callback only counts: per phase (pre / ui / post, relative to UiBegin, UiEnd and Present),
  per queue (hal queue or other), the thread, whether `ARMED_LIST` is among the lists, its index, and the number of lists.

### 2.2 Exact log lines (5 s cadence unless noted; all start `UI probe`)

```
UI probe hook ok fn=ui_begin addr=0x140d80900 prologue=ok
UI probe hook ok fn=ui_end addr=0x140d80a70 prologue=ok
UI probe skip why=prologue|mh_init|mh_create|mh_enable|build fn=<name> got=<hex16>
UI probe ecl hook=ok|missing                                   (once: patch 0002 present in the build)
UI probe hal vt=0x.. vt_ok=0|1 dev_eq=0|1 queue_eq_present=0|1 rtid=<n> present_tid=<n> same_thread=0|1      (first + on change)
UI probe mode b13c=0|1 b13e=0|1 layer=0|1 sel=scene|layer                                                     (first + on change)
UI probe getter addr=0x.. bytes=<hex16> plain=0|1 off=0x..                                                    (first + on change)
UI probe rt which=scene|layer res=0x.. fmt=<n>:<NAME> w=<n> h=<n> dim=<n> mips=<n> arr=<n> samples=<n> flags=0x.. rt_ok=0|1 bb=<w>x<h>:<NAME> distinct=<k>
UI probe frame present=<n> ui_begin=<n> ui_end=<n> nomovies=<n> per_present=<min>-<max> begin_tid_ok=<n>/<n>
UI probe ecl total=<n> halq=<n> otherq=<n> pre=<n> ui=<n> post=<n> armed=<n> armed_hit=<n> armed_first=<n> armed_idx_max=<n> armed_n_max=<n> armed_tid_ok=<n> between_other=<n> late=<n>
UI probe verdict ready=0|1 why=<comma list or none>                                                           (once at 60 s, then every 60 s)
```

The `ecl` fields:

- `armed` counts the frames with an armed list.
- `armed_hit` counts the frames where ARMED_LIST was submitted on the hal queue before UiEnd.
- `armed_first` counts the frames where that ECL was the first one on the hal queue after UiBegin.
- `between_other` counts ECLs on the hal queue, from any thread, that fell between UiBegin and the armed submit.
- `late` counts armed lists first seen after UiEnd or after Present.

### 2.3 What each line proves (analyze_c4.py rows; every row PASS -> `C4 PROBE VERDICT: READY`)

| row | assumption | PASS when |
|-----|------------|-----------|
| hooks | - | both `hook ok` lines, `ecl hook=ok`, no `skip` |
| hal | I2 | `vt_ok=1 dev_eq=1` in every `hal` line (queue_eq_present is reported, not required) |
| thread | I1 | `same_thread=1`; `begin_tid_ok` = all; `per_present` within 0-1 in >= 99% of windows |
| getter | I3 | `plain=1`, and the address/bytes stay the same for the whole run |
| target | I4 | `rt_ok=1`, `samples=1 dim=3 arr=1`, fmt in the accepted table (4.1), w/h equal to the bb or a stable size |
| adjacency | I5 | `armed_hit == armed`, `armed_first == armed`, `between_other = 0`, `late = 0` (zero tolerance) |
| mode | I6 | reported: `sel=scene` -> SDR path; `sel=layer` -> HDR path (needs `inject_ui_layer`, solo visual check) |
| coverage | I8 | reported: `nomovies / ui_begin + nomovies` ratio (fallback share) |

`why=` uses the row names. The DLL computes the same verdict so the user sees it without the analyzer.
I7 is visual only and is covered by the solo test (8.2).

---

## 3. Injection design (PASS I)

### 3.1 Config

```toml
[ui]
probe_scaleform  = false   # cold; PASS P probe (section 2)
inject_viewmodel = false   # cold to enable (hooks installed at boot); hot to disable (passthrough + hooks disabled)
inject_ui_layer  = false   # cold; also allow the HDR path (sel = S+0x100, UNORM only); off = HDR frames stay at Present
```

Why cold to enable: installing MinHook on ER code mid-frame, and creating our list, fence and PSOs, is boot work
gated by the version gate and preflight, like every other hook. A hot enable would add a code path no test covers.
Why hot to disable: the user must be able to get today's behaviour back without restarting a co-op session.
Disabling only stores an atomic: the detours become pure passthrough and the next frame draws at Present. The task
thread then calls `MH_DisableHook` (S1). A hot false->true after boot is ignored and logged
(`UI inject on ignored (cold)`), like other cold keys (`ignored.push`).

Validation: `inject_ui_layer = true` without `inject_viewmodel` gives a warning and is ignored.
`inject_viewmodel = true` on 2.7.1.0 gives `UI inject off why=build`.

### 3.2 Hooks

The same UiBegin/UiEnd detours as the probe (one install path; the probe and the injection can both be on). Patch
0002's ECL callback gets its active form:

```
ECL_HOOK(queue, lists: &[*mut ID3D12CommandList]) -> Option<Splice { at: usize, ours: ID3D12CommandList, fence: ID3D12Fence, value: u64 }>
```

hudhook does the splice, all in the original call on the original thread:
`trampoline(queue, at, lists)` if `at > 0`, then `trampoline(queue, 1, &ours)`, then `queue.Signal(fence, value)`,
then `trampoline(queue, n-at, lists+at)`.

The callback costs one atomic load when nothing is armed. It never allocates, logs or locks.

### 3.3 Where the viewmodel is drawn

**UiBegin detour, on the render thread:**

1. Call the original first. Our recording does not depend on Scaleform's list; we only need `ARMED_LIST` after it.
2. Gate. Every check is non-faulting, and any miss sets `why`, leaves the state Idle, and returns:
   - `cfg.inject_viewmodel` is live, not tripped, and the build has addresses;
   - `hal != 0`, `[owner+0xBE0] > 0`, `[hal] == base + fs_hal_vtable`, `[hal+0x58] & 3 == 3` (BeginFrame ran);
   - `GetCurrentThreadId() == [hal+0x50] == PRESENT_TID`;
   - the frame state is Idle for this `PRESENT_SEQ` (only the first UiBegin per present);
   - the snapshot says the gun is drawn this frame: first person, not AC-130 active, not DrawGate-hidden, renderer
     loaded (the same predicate the Present path uses, factored out as `vm_wants_draw(snap)`);
   - the getter bytes equal the ones pinned at the first good frame; the chain gives `res`; the `res` vtable passes
     the module check; the desc passes 4.1; the path is SDR, or HDR with `inject_ui_layer`;
   - `[hal+0x1410]` is our device (pointer equality);
   - an EarlyPass slot is free (fence completed >= the slot's value), else `why=busy`;
   - the PSO for this format exists (otherwise the loader thread is asked to build it: `why=pso_pending`).
3. `RENDERER.try_lock()` (`why=locked` on contention). Record into the EarlyPass list for slot k, in order:
   - barrier `res` 0xC0 -> RENDER_TARGET;
   - world guns (C3 PASS C), then the viewmodel, **or** the scope overlay instead of the gun when scoped. These are
     the same calls as the Present path (`record_world`, `record_gun`, `record_overlay`), now taking an
     `EarlyTarget { list, res, rtv, format, w, h, slot }`;
   - barrier `res` RENDER_TARGET -> 0xC0.
   Then `Close()`. A failure trips the feature (5.2).
4. Set the frame state to **Armed { seq, list_ptr = [hal+0x1420], slot k, value }**.

**ECL hook:** if the state is Armed, this is the hal queue, the thread is the render thread, and ARMED_LIST is among
the lists at index `at`, return the Splice. The state becomes **Submitted** and the slot's fence value is committed.
After UiEnd, an Armed state that was never submitted becomes **Cancelled**. Its allocator is reset on next use, which
is safe because it was never executed.

**Present (pre-ImGui callback), same thread:** read the state for `PRESENT_SEQ`:
- **Submitted** -> skip world guns, gun and scope overlay; still run thermal/tracers (gated as today), nuke and HUD.
- **Idle / Armed / Cancelled** -> draw them at Present exactly as today. Armed at Present means the UI never
  submitted, so it is set Cancelled first.

Then the state resets to Idle and `PRESENT_SEQ` increments. So each frame draws the gun once: at UiBegin if the
early list was submitted, otherwise at Present.

### 3.4 State save/restore

- **ER and Scaleform lists: nothing.** We never record into them. A D3D12 command list starts with no inherited
  state, and Scaleform's list sets its own root signature, heaps, RT, viewport and PSO from its own caches. None of
  those caches are touched.
- **Resource state:** our list takes `res` from 0xC0, the state ER's next barrier expects [V], and returns it to
  0xC0 exactly. The queue-order state sequence is unchanged.
- **ER's depth/stencil:** never bound or touched. We use our own D32 buffer.
- **Our list:** sets everything it uses (root signature, `SetDescriptorHeaps([our srv heap])`, PSO, own RTV + DSV,
  viewport/scissor = target, topology, VB/IB, root CBVs). Nothing is assumed from hudhook.
- **CPU state:** the snapshot and renderer locks are try_lock only. Pose scratch is thread_local, so the render
  thread uses the same code as the present thread (it is the same thread when I1 passes).

## 4. Format and size

### 4.1 Accepted targets (refuse anything else -> `why=format` fallback; same "don't guess" rule as `select_output`)

| target DXGI format | RTV format | shader output | alpha write | path |
|--------------------|------------|---------------|-------------|------|
| R10G10B10A2_UNORM, R8G8B8A8_UNORM, B8G8R8A8_UNORM | same | Srgb OETF (as today for the BB) | RGB only (scene) | SDR, sel = S+0xF0 |
| R8G8B8A8_UNORM_SRGB, B8G8R8A8_UNORM_SRGB | same | linear | RGB only | SDR |
| UNORM rows above, on the HDR path | same | Srgb OETF | **RGBA, A = 1** (new PSO variant `ui_layer`) | HDR, only with `inject_ui_layer` |
| *_TYPELESS, *_FLOAT, MSAA, arrays, no ALLOW_RENDER_TARGET | - | - | - | refused (C4.1 may add a rule once the probe shows the real format) |

The output mode for the early target comes from the **target format and path**, not from the swap chain or
`SetColorSpace1`. The HDR-toggle guard (live back-buffer format family vs hudhook's RTV) stays on the Present path only.

### 4.2 Size

The early depth buffer and the projection aspect (`vm_proj::build`) use the target's desc w/h. If w/h differ from
the back buffer, this is logged once (`UI inject size target=WxH bb=WxH`). Projection and the ADS fraction are
computed from the target, so the gun looks the same as on the back buffer. Depth is resized only on the render
thread, and the old one is retired by fence value (4.3).

### 4.3 Per-frame fencing and resource lifetime

`EarlyPass` (new, in mw2er-render; created on the loader thread with ER's device once the hal device is known):

| resource | count | rule |
|----------|-------|------|
| command allocators + one list per slot | 3 slots | slot k reusable only when `fence.GetCompletedValue() >= value[k]`; never waits on the render thread (`busy` fallback) |
| ID3D12Fence | 1 | `Signal` spliced right after our list (3.2); values increase monotonically |
| RTV heap (non-shader-visible) | 3 descriptors | slot k's RTV rewritten each record (CreateRenderTargetView on the target) |
| constant upload ring | 3 x 17 x 6400 B, **separate** from the Present ring | indexed by EarlyPass slot, so it cannot race hudhook's `frame_index` ring |
| D32 depth + 1-slot DSV heap | 1 live (+ retired) | retired buffers are released when the fence passes the value of the last list that used them (replaces the `frames` counter for the early path) |
| PSOs (opaque, alpha, ui_layer) per accepted format | lazy, loader thread | never created on the render thread; `pso_pending` until ready |
| SRV heap, root signature, textures | shared with Present | read-only after upload (unchanged) |

Timestamp queries on the early path: not in C4 (a `perf` line keeps the Present numbers only).

## 5. Failure handling

### 5.1 Expected misses (count, no trip): `nomovies`, `ac130`, `hidden`, `notfp`, `scoped_no_overlay`, `busy`,
`pso_pending`, `locked`, `hdr_off` (HDR path without `inject_ui_layer`), `second_begin`, `cancelled`. Each frame
with a miss draws at Present.

### 5.2 Unexpected misses (trip): the feature turns off for the session and the hooks are disabled by the task
thread. The causes are:

- vtable mismatch;
- thread mismatch while the probe verdict was ready;
- getter bytes changed;
- the `res` module check failed;
- a format/desc that changes mid-run to a refused one 3 times in 10 s;
- any HRESULT failure (Reset, Close, CreateRTV-desc validation, Signal);
- a panic caught in a detour (every detour body is wrapped in `catch_unwind`, and the original is always called first);
- Armed but not submitted 30 frames in a row (I5 broke);
- `device.GetDeviceRemovedReason() != S_OK`, checked at Present.

The line is `UI inject trip why=<reason> frame=<n>`, logged once. Today's behaviour continues.

### 5.3 Log lines

```
UI inject on build=2.7.2.0 path=scene|layer fmt=<NAME> size=<w>x<h> slots=3
UI inject off why=config|build|probe_verdict|hooks
UI inject early=<n> present=<n> splice=<n> cancelled=<n> miss=<why:n,...>          (5 s)
UI inject size target=<w>x<h> bb=<w>x<h>                                          (once / on change)
UI inject trip why=<reason> frame=<n>
UI inject teardown hooks=disabled (passthrough; trampolines kept, never freed in-session) inflight=0|busy fence=n/a released=0
```

The probe verdict also gates the injection at run time. If `probe_scaleform` and `inject_viewmodel` are both on, the
injection waits until the in-DLL verdict is ready (60 s) and logs `off why=probe_verdict` if it is not. With the
probe off, the gates in 3.3 enforce the same facts per frame.

## 6. Interaction with the other passes

| pass | where | C4 rule |
|------|-------|---------|
| ImGui HUD (crosshair, ammo, toasts, damage numbers) | Present | unchanged; stays above ER's UI (only the gun was the complaint) |
| World guns (C3 PASS C) | with the gun | move together: they share depth and the constant-ring slot; both under the UI |
| Scope overlay (ADS sniper) | with the gun | replaces the gun on the early path exactly as at Present, so its black vignette is also under ER's messages; the Present fallback makes the same choice; never gun + overlay or neither |
| ADS | - | pose only; unaffected (aspect from the target) |
| Thermal + tracers (AC-130) | Present | while `snap.ac130.active` the early path does nothing (`why=ac130`), as the gun is skipped then anyway |
| Nuke post | Present, last | unchanged; it copies the back buffer, which now already holds the gun under ER's UI |
| UI yield | Present only | `vm_offset`/hide are applied only on Present-drawn frames (fallback). Early-drawn frames do not need it. HUD dimming unchanged. User config stays `mode = "off"` |
| C3 Scaleform probe hooks | ER | coexist; all go through the same teardown |
| Seamless / other ECL hooks | - | the splice is inside our ECL detour; other hooks downstream see three calls instead of one, all with valid arguments |

## 7. Per-build addresses (prologue-checked)

New `OwnAddrs` field `scaleform_ui: Option<ScaleformUi>` (mw2er-core exe_build.rs):

```rust
pub struct ScaleformUi {
    pub ui_begin: usize, pub ui_begin_prologue: &'static [u8], // 16 B
    pub ui_end: usize,   pub ui_end_prologue: &'static [u8],   // 16 B
    pub fs_hal_vtable: usize,      // FS HAL subclass vtable
    pub g_render: usize,           // pointer global G; S = G + s_off
    pub direct_queue: usize,       // ER's direct queue object (+8 = ID3D12CommandQueue*), logged only
    pub layout: UiLayout,          // owner_hal 0x968, owner_movies 0xBE0, hal_flags 0x58, hal_rtid 0x50,
                                   // hal_device 0x1410, hal_queue 0x1418, hal_list 0x1420, s_off 0x128,
                                   // scene 0xF0, layer 0x100, hdr_a 0x13c, hdr_b 0x13e, obj 0x70, wrap 8, res 0x20
}
```

| build | ui_begin | ui_end | fs_hal_vtable | g_render | direct_queue | source |
|-------|----------|--------|---------------|----------|--------------|--------|
| 2.7.2.0 | 0xd80900 | 0xd80a70 | 0x2c0bb40 | 0x47f6400 | 0x4808340 | [V] exe bytes 2026-10-20 (hal_re.md 6 + this doc's re-read) |
| 2.7.1.0 | None | None | None | None | None | exe gone; no guessed shifts (code +0x16f0 does not hold for .rdata/.data) |

Site rows (exe_sites.rs SITES_2720, Group::Own, Conf::Verified): `scaleform_ui_begin` Check::Code(16 B),
`scaleform_ui_end` Check::Code(16 B), `scaleform_fs_hal_vtable` Check::VtableNoRtti, `scaleform_g_render` and
`scaleform_direct_queue` Check::Writable. The test at exe_build.rs (`exe_build_versions_and_tables`) is extended so
that every `Some` own address has a site in the same build and `None` has none. The preflight's existing jmp-detour
tolerance (`is_jmp_detour`) also applies: if another module detoured UiBegin, MinHook refuses (prologue mismatch)
and we log `skip why=prologue`.

`layout` offsets are pinned by instruction bytes inside UiBegin in `c4_static.py` (8.1), not only by the prologue:
- `48 8b 89 68 09 00 00` -> 0x968
- `48 83 bb e0 0b 00 00 00` -> 0xBE0
- `48 8b 88 28 01 00 00` -> 0x128
- `80 b9 3c 01 00 00 00` / `80 b9 3e 01 00 00 00` -> 0x13c / 0x13e
- `48 8b 81 00 01 00 00` / `48 8b 81 f0 00 00 00` -> 0x100 / 0xF0
- `48 8d 48 70` -> 0x70
- `48 8b 4b 08 48 8b 51 20` -> wrap 8, res 0x20
- `c7 44 24 34 c0 00 00 00 c7 44 24 38 04 00 00 00` -> 0xC0 -> 4
- `48 8b 8f 20 14 00 00` + `ff 90 d0 00 00 00` -> the list at +0x1420 and ResourceBarrier

## 8. Offline checks

### 8.1 `tools\scripts\check_c4.ps1`

Windows PowerShell 5.1, same skeleton as check_c3 (`Add-Result`, `Invoke-Tool`, `Run-Check`, logs in
`work\check_c4\`, `-AllowSkip`, `-SkipC3`; exit 0 only if every check PASSes; prints `C4 OFFLINE ORACLES: PASS`).
It launches nothing and only reads the game exe.

1. **C3 suite**: `check_c3.ps1` exit 0 (it runs check_m8 and the rest, and stages the DLL).
2. **ER static pin** (`c4_static.py`, Python -I, stdlib only, read-only; the exe path is passed as an argument):
   - the 2.7.2.0 sha256;
   - the UiBegin/UiEnd/UI-job prologues;
   - every `layout` instruction pin in section 7;
   - FS vtable slots +0x18/+0x58/+0x60/+0x70/+0x78/+0x80/+0x390 = 0x14115ece0/0x140eb9c80/0x140eb9ca0/0x140eb9a10/0x140eb9b80/0x140eb9c30/0x140eb9d20;
   - FS BeginScene/EndScene `call 14115FDA0h` / `call 14115FF00h`;
   - the HAL ctor's `lea rax,[142C0BB40h]`;
   - G and the queue in .data, the vtable in .rdata.
   Each pin has a mutation self-test (flip one byte -> FAIL).
3. **hudhook patches**: `make_hudhook_patch.py --check` shows 0001 + 0002 applied to `vendor\hudhook-0.9.3` and
   identical to `tools\patches`. The vendored ECL impl calls `ECL_HOOK` before the trampoline and the splice performs
   exactly the three trampoline calls + Signal.
4. **cargo build** (release, 0 warnings) and **cargo test** with at least one test per prefix:
   - `c4_addrs_`: tables, the Some/None rule, layout values;
   - `c4_cfg_`: defaults off; cold/hot rules; the inject_ui_layer warning;
   - `c4_gate_`: every miss and trip reason, from pure inputs;
   - `c4_state_`: frame state machine. Exhaustive event orders (UiBegin, ECL with or without the armed list, UiEnd,
     Present, a second UiBegin, a trip) never yield both or neither;
   - `c4_splice_`: index math, n=1, at=0, at=n-1, list absent;
   - `c4_fmt_`: the 4.1 table, unknown formats refused;
   - `c4_ring_`: slot reuse only after the fence, retire-by-value;
   - `c4_getter_`: plain-getter byte patterns accept/reject;
   - `c4_probe_`: line formats and the in-DLL verdict;
   - `c4_teardown_`: order of disable, remove, fence wait, release.
5. **vmtest** (`vmtest --c4`, WARP + debug layer as the existing cases):
   - create an "ER" texture in 0xC0 on a direct queue, record ER-like list A (barrier 0xC0->4, clear-free draw,
     barrier 4->0xC0);
   - submit through the splice helper with EarlyPass in front;
   - expect zero debug-layer messages, our pixels present and then overdrawn by A's draw (gun under "UI"), and the
     state sequence intact;
   - cases: R10G10B10A2, RGBA8, RGBA8_SRGB, the ui_layer variant (alpha 1), a refused FLOAT, a busy slot (no wait),
     a resize (old depth released only after its fence).
6. **Wiring** (`m2_static_checks.py c4wiring --self-test`, each rule with a mutation that must fail):
   - detours and the ECL callback have no alloc, log, lock (only try_lock), or game write;
   - the original is called first in the UiBegin detour;
   - every MinHook install has a matching disable/remove in `teardown`;
   - no `mem::forget` of a hook handle in ui_probe.rs, ui_inject.rs, pad.rs or cam_hook.rs;
   - Present skips the gun only on `Submitted`;
   - world guns and overlay follow the gun's path;
   - the early path checks `ac130`;
   - no `SetDescriptorHeaps`/`OMSetRenderTargets` on any list but ours.
7. **Analyzer**: `analyze_c4.py --self-test` (synthetic logs: READY, each row FAIL, N/A, a trip) plus replay of
   today's `logs\mw2er.log` (expect `probe N/A`, not FAIL).
8. **DLL proof strings** (`m2_static_checks.py c4strings`): every `UI probe`/`UI inject` format from 2.2 and 5.3 is
   in the built DLL; built == staged.
9. **Config**: `mw2er.toml` parses with the C4 defaults (`inject_viewmodel = false`, `inject_ui_layer = false`); the
   friend toml is unchanged.
10. **Hygiene**: `m2_static_checks.py hygiene` and `gamefolder` PASS.

## 9. In-game test steps (user runs them; logs in `logs\mw2er.log`)

### 9.1 Probe run (after PASS P), solo, offline, about 5 minutes

1. Set `[ui] probe_scaleform = true`, `inject_viewmodel = false`. Launch as usual.
2. In game for at least 2 minutes:
   - stand at a grace until "Sit at grace" shows;
   - rest, then leave the menu;
   - pick up an item;
   - open and close the pause menu, the map and the inventory;
   - fire a few shots, ADS with a sniper, and call AC-130 once.
3. Quit normally. Run `python -I tools\scripts\analyze_c4.py logs\mw2er.log`.
   - `C4 PROBE VERDICT: READY` (plus `path=scene` or `path=layer`) -> PASS I may be enabled.
   - Any FAIL row -> stop. Its line names the [I] that broke, and the design is revisited (C4.1).
4. In-game HDR setting: note whether it is on. If `path=layer` shows, run step 2 once more with HDR off, so both
   paths are logged.

### 9.2 Injection run (after PASS I and a READY verdict), solo, offline

1. Set `inject_viewmodel = true` (and `inject_ui_layer = true` only if the user plays with HDR and wants to test it).
   Keep `probe_scaleform = true`.
2. Check that "Sit at grace", item pick-up banners, subtitles and the "Hostile/You died" style messages draw **over** the gun.
3. Check that the scope overlay sits under ER's messages and that nothing doubles (no gun + overlay, no flicker
   between frames).
4. AC-130 on/off, a nuke, the pause menu (the gun disappears with ER's full-screen menu as before, or stays under it).
   Toggle HDR in ER's settings once.
5. Turn `inject_viewmodel = false` in the toml while in game. Expect `UI inject off why=config` and the gun back on
   top at once.
6. Analyzer rows:
   - `UI inject early/present`: early >= 95% of frames with UI;
   - `trip` absent;
   - `cancelled` 0;
   - no device-removed line.

### 9.3 Co-op run (only after 9.2 is clean)

The host and the friend both keep the default off. The host turns it on. A one-session run, then the same analyzer
rows. A trip or any device-removed event -> back to off and C4.1.

## S1. Every new hook, write and read, and its restore

| What | Kind | Where | Restore / safety |
|------|------|-------|------------------|
| UiBegin 0x140d80900 / UiEnd 0x140d80a70 | MinHook detour, ER image | render thread | 16 B prologue check; original first; `MH_DisableHook` in teardown (never removed) |
| BeginScene / EndScene (C3) | MinHook detour | render thread | `forget()` removed; handles stored; same teardown |
| hudhook ECL callback (patch 0002) | our vendored code | ER threads calling ECL | cleared (`ECL_ACTIVE=false`) before the hooks are disabled; hudhook removes its own ECL hook in `unhook` |
| HAL / owner / S fields, getter bytes, `res->GetDesc` | non-faulting read; read-only COM call | render thread | read-only; module check before the COM call |
| EarlyPass list, fence, heaps, ring, depth, PSOs | our D3D12 objects on ER's device | loader / render thread | released after fence wait (2 s timeout; on timeout they are leaked, never freed while possibly in use) |
| game memory | - | - | **no writes** |

**Teardown** (`ui_inject::teardown`, also used by ui_probe):
1. Set `ACTIVE=false` (the detours and the ECL callback become passthrough).
2. `MH_DisableHook` for each stored target. (Review C4: never `MH_RemoveHook`, in-session or at unload: a thread between a detour's entry and its in-flight increment is not counted, and freeing the trampoline would crash it. The trampolines are leaked.)
3. Wait for the in-flight detour counter to reach 0 (spin with yield, 1 s).
4. Wait on our fence (Event, 2 s).
5. Release.

Callers:
- the task thread on hot disable or a trip (steps 1-2 only; the objects stay, so a slot can never be in use by a
  re-armed frame);
- a **new** `DLL_PROCESS_DETACH` branch, only when `lpReserved == NULL` (FreeLibrary). At process exit it does
  nothing, because the process and device are going away and MinHook calls under the loader lock at exit are unsafe.
  me3 never FreeLibrary's, so the hot-disable path is the one normally used.

## S2. Implementation task list

**PASS P (probe, read-only)**
1. exe_build.rs/exe_sites.rs: `ScaleformUi` + `UiLayout`; 2.7.2.0 values, 2.7.1.0 None; site rows; test extension.
2. `tools\scripts\c4_static.py` (stdlib, read-only, mutation self-test) with the section 7 pins.
3. hudhook patch 0002 (ECL_HOOK + PRESENT_SEQ + present tid; splice helper present but unused in P); regenerate the
   vendor copy; `make_hudhook_patch.py --check`.
4. ui_probe.rs: UiBegin/UiEnd detours, the new atomics, the 2.2 lines, the in-DLL verdict. Remove `forget()` and add
   stored handles.
5. `ui_inject::teardown` + `DLL_PROCESS_DETACH` (FreeLibrary only); apply it to the ui_probe hooks (and list pad.rs and
   cam_hook.rs as follow-ups with the same pattern).
6. `analyze_c4.py` + self-test; `check_c4.ps1` checks 1-4, 6-10 (vmtest case SKIP until PASS I).
7. MODLOG entry; staged DLL; the user runs 9.1.

**PASS I (injection, behind `[ui] inject_viewmodel = false`)**, only after `C4 PROBE VERDICT: READY`:

8. Config keys (3.1) with cold/hot rules and tests.
9. mw2er-render: `EarlyTarget`; refactor `record_world` / `record_gun` / `record_overlay` to take (list, target,
   slot ring); `EarlyPass` (allocators, list, fence, RTV ring, separate constant ring, fence-retired depth); PSO
   variant `ui_layer`; format/output table 4.1; loader-thread PSO prepare on request.
10. ui_inject.rs: gate (`vm_wants_draw` factored out of viewmodel.rs), frame state machine (pure part in
    mw2er-core `c4_state`), UiBegin record, ECL splice, UiEnd cancel, trip handling, log lines 5.3.
11. viewmodel.rs Present path: skip world guns, gun and overlay only on `Submitted`; ui_yield offset only on
    Present-drawn frames.
12. vmtest `--c4` cases (8.1 item 5); `c4wiring`/`c4strings`; complete check_c4.ps1.
13. MODLOG; staged DLL; the user runs 9.2, then 9.3.
