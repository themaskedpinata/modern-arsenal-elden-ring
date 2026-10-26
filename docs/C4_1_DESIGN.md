# C4.1 design: the MW2 viewmodel under Elden Ring's UI, recorded on ER's render thread

Status: P41 run in game (2026-10-20, every row but `alpha` PASS); P2 (2.4) and PASS I41 (3.8) built offline on
2026-10-21, neither has run in game. The C4 probe (logs\mw2er.log, sessions
19:34, 19:38 and 19:43) refuted the C4 injection design (docs\C4_UI_DESIGN.md) on two points. UiBegin never runs on
the Present thread (`begin_tid_ok=0/12643`, `0/14493`). The UI-target getter is a real function, not a field read.
C4.1 keeps the C4 machinery: hooks, ECL splice, EarlyPass, frame state machine, teardown, vmtest `--c4`. It changes
three things: the thread rule, how the target is found, and what the early list writes into ER's UI layer.

Sources:
- `work\c4_1\optA_thread.md` (option A: ER's render thread, the read-only target chain, the adjacency miss);
- `work\c4_1\optB_layer.md` with raw dumps in `work\c4_1\optb\` (option B: the UI layer, the composite pass and its
  shader table, the HDR flag);
- the C4 probe lines in `logs\mw2er.log`;
- our code: `mw2er-core\src\c4.rs`, `mw2er\src\ui_probe.rs`, `ui_inject.rs`, `viewmodel.rs`, `mw2er-render\src\early.rs`.

This document builds on C4_UI_DESIGN.md and only states what differs from it. Section numbers there are written C4 n.n.

## Evidence labels

- **[V]** verified from bytes in eldenring.exe 2.7.2.0, from our code, or from our logs.
- **[I]** inferred. Every [I] the injection depends on gets a probe line (section 2) or a run-time self-check (3.6).

This document re-read the 2.7.2.0 exe file on 2026-10-20 with plain Python `-I` (read only, nothing installed or
launched). The header is `TimeDateStamp 0x6aabf7e7`. These bytes were confirmed:

- `[0x142f0af48]` = 0x1419f49a0 and `[0x142f0af50]` = 0x1419f4970 (secondary vtable slots 0 and 1);
- `[0x142f0ae60 + 0x38]` = 0x1419f3dc0 (primary vtable slot 7);
- the getter 0x1419f4970 starts `48 83 ec 28 48 83 79 d0 00 48 c7 44 24 30 00 00`. These are the same 16 bytes the
  probe logged (`bytes=4883ec28488379d00048c74424300000`): `cmp qword [rcx-0x30],0` tests `[sel+0x40]`;
- slot 7 0x1419f3dc0 reads `[rcx+0x40]`;
- at 0x140d7b777, `mov rax,[rcx+0x6d0]` / `mov edi,[rax+0x14]`. This is the render-thread id that ER passes to
  InitHAL;
- the UiBegin prologue is unchanged.

## Hard rules (unchanged from C4, apply to every C4.1 step)

- Never launch Elden Ring, Steam, me3 or Seamless. The exe is read only, as a file. Never write into a game folder or
  `%APPDATA%\EldenRing`. No downloads.
- **No game-memory writes.** Every game read is non-faulting (`game::read`). Any write ever added goes through
  `game::write`.
- Every hook is MinHook with a prologue byte check. C4.1 adds **no new hook on ER code**: it reuses UiBegin / UiEnd
  and hudhook's ECL callback (patch 0002).
- New behaviour is off by default and cold to turn on.
- Any doubt means that frame draws at Present, exactly as today (and with `[ui_yield] mode = "cutout"`, also cut out).
  A device-removed crash in co-op is the failure this design is built to avoid.

---

## 0. Decision at a glance

| # | Item | Decision | Label |
|---|------|----------|-------|
| 1 | Approach | **Option A**, the UI-thread early pass. It is used for ER's SDR output, and also for its HDR output behind a second cold flag. Option B (copying the UI layer at UiEnd and re-compositing it at Present) is **not built**. The C5 cut-out stays the fallback for every frame A does not draw. | design |
| 2 | Thread | Record in UiBegin's detour on ER's render thread. The gate requires `tid == [hal+0x50] == [[G+0x6d0]+0x14]` and `byte [G+0x11c] != 0` (multithreaded mode), **not** `tid == present_tid`. | thread [V code], UiBegin on it [I strong -> probe] |
| 3 | Target | The resource UiBegin itself barriers `0xC0 -> RENDER_TARGET`. On PC that is always the UI layer `S+0x100` (R8G8B8A8_UNORM, scene-sized). It is reached by a **pinned read-only chain** (5 vtable/slot pins), never by calling the getter. | chain [V], live values [I -> probe] |
| 4 | What we write | The gun as **opaque UI-layer content**, using the layer's alpha convention: "transmit" (`out = scene*a + ui`, empty = (0,0,0,1)) or "premul" (`out = scene*(1-a) + ui`, empty = (0,0,0,0)). The convention is pinned per build from a probe sample. Without a pin, nothing is drawn early. | convention [I -> probe P2] |
| 5 | Ordering | The existing seq-keyed state machine, plus a new `ahead` refusal: a UiBegin whose PRESENT_SEQ equals the previous UiBegin's seq never arms. Present arriving inside a UI frame cancels the frame; if our list was already spliced, it trips (`order`). | design; ordering [I strong -> probe] |
| 6 | Flags | New `[ui] inject_ui_thread = false` (cold), which requires `inject_viewmodel` (master: cold on, hot off). `inject_ui_layer` changes meaning to "also allow ER's HDR output (b13c=1)". New `[ui] probe_layer_sample = false` (cold, probe only). | design |
| 7 | Fallback | Every miss or trip falls back to today's Present path, plus the C5 cut-out when `[ui_yield] mode = "cutout"`. Each frame draws the gun once, never both and never neither. | design |
| 8 | Builds | 2.7.2.0 only. 2.7.1.0 has `scaleform_ui: None` (that exe is gone). | [V] |

**Order:** PASS P41 (probe extension + analyzer, read only) -> in-game probe run (user, about 5 min) -> pin the
alpha convention from the run -> PASS I41 (gate, chain, state, PSO variant, behind the flag; vmtest) -> solo
injection run (user) -> co-op run (user). Each pass ends with `check_c4.ps1` PASS (the main session runs it) and a
staged DLL. The friend package does not change.

### 0.1 Why A and not B

- **A uses ER's own composite.** The gun becomes UI-layer content, so ER's blend shader handles it like any other UI
  pixel:
  - SDR output uses `_BlendUI_SDR2SDR_F`.
  - HDR output uses `_BlendUI_F` / `_BlendUI_SDR2HDR_F`, with paper white and the PQ term [V shader choice, I math].
  - B has to re-create that blend itself on gun pixels at Present. The shader is in the encrypted `Data*.bdt`, so a
    brightness term in SDR2SDR would be reproduced wrong. HDR is not covered at all (rating: low).
- **A reuses built and tested code.** That covers EarlyPass (3 slots, own fence, depth, constant ring), the splice in
  hudhook patch 0002, the compare-exchange state machine with its exhaustive test, the PSO worker, teardown and the
  vmtest `--c4` cases. A changes the gate rule, the target resolver and one PSO blend variant.
- **B would need renderer surgery** in the files the C5 cut-out just changed:
  - a full-screen copy pass and a copy ring on the render thread;
  - D32_FLOAT -> D32_FLOAT_S8X24 depth for every gun PSO (a stencil mask);
  - a stencil-masked re-blend.
  B also double-blends translucent gun edges and the scope overlay.
- **Both need the layer's alpha convention.** B is not cheaper on that point.
- **A's residual risks are visual, not crash risks:**
  - ER clearing the layer after UiBegin would wipe the gun, and the canary in 3.6 detects that;
  - a wrong convention gives a ghost or black gun, and the probe sample plus the per-session check prevent that.
  The GPU-state risk is the same as in C4: our list moves `0xC0 -> RT -> 0xC0` on the resource whose `0xC0` state
  ER's own next barrier asserts [V].

---

## 1. What is known

### 1.1 Correction: the probe runs were on ER's SDR output (answer to "did not see how to turn off HDR")

There was nothing to turn off. All three probe sessions already ran on ER's **SDR** output [I strong]:

- `b13c` (`byte [S+0x13c]`) is ER's HDR-output test. Its builder `1419ef4e0` queries the swap chain's IDXGIOutput
  [V for the calls]. The screenshot path `141c568e0` uses the HDR2SDR blend exactly when `13c = 1` [V]. The value was
  `b13c=0` in every `UI probe mode` line.
- `b13e` only means "the UI layer exists". `1419ef8e2` sets it every time the targets are built [V]. That is why
  UiBegin selected `sel=layer`.
- No `SetColorSpace1` call was seen (`colorspace=none`, `VM output Srgb ... colorspace=-1`). Our gun is drawn with
  sRGB encoding into the 10-bit back buffer and looks right. In ER's HDR output it would look wrong.
- `R10G10B10A2_UNORM` is only ER's swap-chain format.

So the "HDR UI-layer path" wording in MODLOG (19:38 / 19:43 entries) and in the C4 probe's `mode` row is wrong. On
PC, ER always draws its UI into a separate layer and composites it. C4 9.1 step 4 (the SDR run) has effectively been
done three times. To test ER's **real** HDR output later [I]:
1. Turn on Windows HDR (Settings > System > Display > Use HDR, or Win+Alt+B) before starting the game.
2. Then turn on ER's own HDR option (Settings > Display), if ER shows it.
3. `UI probe41 output b13c=1` confirms it.

C4.1 needs no HDR run before its SDR injection.

### 1.2 Verified (2.7.2.0)

**Threads** [V, optA 1]:
- ER queues the UI job `140d80bc0` as a callback record in its per-frame render-command buffer (`140d7c26f` ->
  `141aedf00`, buffer `G+0x6e8[G+0x708]`, double-buffered).
- In multithreaded mode (`byte [G+0x11c] != 0`), the frame thread hands the buffer to ONE dedicated render thread
  (`1419ed8e0` -> `141af6750`). The loop `141af6973..a2f` runs the records in key order, one UI job per frame.
- That thread's id is `[[G+0x6d0]+0x14]`, the value ER passes to InitHAL as `[hal+0x50]`.
- In single-threaded mode the same buffer runs inline on the frame thread (`1419e9ce0`).

**Submission** [V, optA 2]:
- Before ER runs any callback, it submits the pending 3D batch (`141aee1f0`). So the scene is queued before UiBegin
  runs.
- ER's submit is `ECL(n)` then `Signal(frame fence)` (`141ea6300`).
- Scaleform's lists are its own (`[hal+0x1420]`, pool `140ebaef0`). SubmitPending `140eb9a10` passes one list per
  ECL, FIFO, on the same DIRECT queue and thread (`queue_eq_present=1`, `armed_idx_max=0 armed_n_max=1` [V log]).

**The UI layer** [V, optB 1]:
- S = the first 0x170-byte entry of `[G+0x128]`.
- The builder `1419ef0a0` creates "UI RenderTarget" at `S+0x100` unconditionally: GXRenderTarget2D, format 0x1C =
  R8G8B8A8_UNORM (ER's format enum is DXGI: its depth is 0x14 = D32_FLOAT_S8X24), size `[S+0x108] x [S+0x10C]` (the
  scene's), 1 mip, optimized clear (0,0,0,1).
- "Scene RenderTarget" is `S+0xF0`, in the back-buffer format.

**The read-only chain** [V, optA 3 + optB 1, pins re-read above]:

```
sel   = UiBegin's choice (C4 1.1 step 3): [S+0x100] when (b13c|b13e) && [S+0x100], else [S+0xF0]
pins  [sel] == base+0x2f0ae60                         primary vtable
      [[sel]+0x38] == base+0x19f3dc0                  its slot 7 (count)
      [sel+0x70] == base+0x2f0af48                    GXBS::GXRenderTarget2D (RTTI COL 0x1433B4CD8)
      [[sel+0x70]] == base+0x19f49a0, [[sel+0x70]+8] == base+0x19f4970     slots 0 / 1 (the "getter")
T     = [sel+0x40] != 0;  arr = [sel+0x78] != 0
count = [T+0x38] ? u32 [[[T+0x38]+8]+8] : u8 [T+0x46];   count >= 1
obj   = [arr];  res = [[obj+8]+0x20]  (ID3D12Resource*, what UiBegin barriers);  er_rtv = [obj+0x10]
check rt = [owner+0xDD0];  rtData = [rt+0x20];  u32 [rtData+0x20] == res desc Format;  [rtData+0x28] == er_rtv
```

- `owner+0xDD0` is built at HAL init from the same selection by `hal->vt[111] CreateRenderTarget`. Its writers are
  `140d778a0 / 140d78580 / 140d7a17f / 140d7c2a0`.
- A cross-check mismatch means the target was recreated, so that frame is skipped.

**The composite** [V, optB 2]:
- `1419ee870` draws one fullscreen pass into the back buffer from the scene SRV, the UI-layer SRV, LUT `[G+0xEC8]`
  and a 0x60-byte CB.
- Shader: `_BlendUI_SDR2SDR_F` when b13c = 0. When b13c = 1, `_BlendUI_F` or `_BlendUI_SDR2HDR_F` (chosen by
  `[G+0xEB9]`). The `_UPSAMPLE_` variants are used when render scale `G+0xED0/ED4` < 1.0.
- It returns at once (filling (0,0,0,1)) while `[[G+0x740]+8] == 0` (shaders not ready).

**State** [V, C4 1.1]: UiBegin barriers `res` 0xC0 -> 4 and UiEnd 4 -> 0xC0. So `res` is in 0xC0 when our spliced
list runs, and must be in 0xC0 when ER's list runs.

**The adjacency miss** [V, optA 4]:
- The once-per-session `between_other=1` came in the first 5 s window of every session. After that,
  `armed_first = armed - 1` held to the end (19:47:54: 14492/14493).
- FS InitHAL `140eb9460` queues a one-off init list without submitting it (`140eb96d4`). The first SubmitPending
  sends it just before the first armed list.

**Our side** [V code]:
- EarlyPass, the splice (`at == 0` only), the state machine, the PSO worker (`mw2er-c4pso`), `prepare_depth` and the
  `ui_layer` PSO variant exist.
- The `ui_layer` variant writes RGBA. Opaque writes A = 1; alpha blends `a = src + dst*(1-src)`. That is the
  **premul** convention only.
- In cut-out mode the early path draws with `cut_count = 0` (C5 entry).

### 1.3 Inferred (each gets a probe line in 2.2 or a run-time check in 3.6)

| id | Assumption | Check |
|----|------------|-------|
| J1 | UiBegin runs on `[hal+0x50] == [[G+0x6d0]+0x14]` in every frame, multithreaded mode is on, and the rtid is stable in a session. Today this rests on `armed_tid_ok == armed_hit` and on the inline submit's own `tid == [hal+0x50]` test. | `probe41 thread` |
| J2 | Strict order Present(N) -> PRESENT_SEQ++ -> UiBegin(N+1) -> UiEnd(N+1) -> composite(N+1) -> Present(N+1). Today: `per_present` max 1, `late=0`. | `ahead`, `present_in_ui` |
| J3 | The live pins hold, `res` is non-zero, inside the D3D12 module and stable, and the rtData cross-check matches. | `probe41 pins`, `layer` |
| J4 | The layer desc is R8G8B8A8_UNORM, w/h == back buffer, 1 mip, 1 sample, ALLOW_RENDER_TARGET, at render scale 1.0. | `probe41 layer`, `output` |
| J5 | The layer's alpha convention: transmit (empty a = 1) or premul (empty a = 0). The (0,0,0,1) clear value hints at transmit (weak). | `probe41 sample` (P2) |
| J6 | Nothing clears the layer on the GPU after UiBegin. If something does, the gun spliced ahead of the armed list would be wiped (visual only). | injection canary (3.6) |
| J7 | ER's own 2D draws into the layer (`141a5c6bd`) happen inside UiBegin..UiEnd, or after. If they happen before, those few elements would be under the gun. | solo visual test (8.2); no hook added |
| J8 | The `between_other` ECL is the one-off init list, once per HAL lifetime. | `probe41 between` |
| J9 | `[owner+0xDD0]`, `[sel]` and `res` change on a resolution change, and the gate sees it. | `probe41 recreate` |
| J10 | Present callbacks never overlap UiBegin..UiEnd, so the RENDERER lock rarely makes a `locked` miss. | `present_in_ui`, `locked` count |

---

## 2. Probe extension P41 (read only; one short run)

### 2.1 What it adds

`[ui] probe_scaleform = true` (cold, unchanged) keeps every C4 probe line and verdict as they are. The C4 analyzer
rows and the self-test do not change. P41 adds reads to the **same** UiBegin / UiEnd detours and the ECL callback.
There is no new hook. The detour still does no alloc, no log and no lock, and stores atomics that the task thread
logs.

- **UiBegin, after the original:**
  - `G = [base+g_render]`: `byte [G+0x11c]`, `rt_tid = u32 [[G+0x6d0]+0x14]`;
  - `tid`, `[hal+0x50]`;
  - for **both** candidates (layer and scene): the 5 pins, `T`, `arr`, `count`, `obj`, `res`, `er_rtv` (the 1.2
    chain, through a new pure `c4::chain41`);
  - `rt = [owner+0xDD0]`, `rtData`, `u32 [rtData+0x20]`, `[rtData+0x28]`;
  - `[G+0xEB9]`, `f32 [G+0xED0]`, `f32 [G+0xED4]`, `[[G+0x740]+8] != 0`;
  - the seq of the previous UiBegin (for `ahead`).
  - Every 5 s, `GetDesc` of both resources: the same `desc_of` / `desc_if_vtable` module rule as C4 (no loader lock
    on the render thread once the vtable is cached).
- **Present callback:** `present_in_ui` (+1 when the UI frame is open: `F_BEGIN_SEQ == seq && F_END_SEQ != seq`).
- **ECL callback:** at each `between_other` event, the frame index since the first `hal != 0` UiBegin and the list
  pointer (one slot, first 4 events).
- **P2, the layer sample** (`[ui] probe_layer_sample = true`, cold, off by default). It is the only part that issues
  GPU work, and it **reads** ER's layer only:
  - When: every 5 s, on one armed frame, if the C4.1 gate's thread / pins / desc rows pass.
  - What: UiBegin's detour records a small list of ours. It is a compute dispatch that reads 16 texels of `res`
    through our own SRV (format = the desc's format, 1 mip) and writes them to our own UAV buffer, then copies that
    buffer to a readback buffer.
  - No barrier is placed on ER's resource. `0xC0` = PIXEL | NON_PIXEL_SHADER_RESOURCE, and a compute read needs
    NON_PIXEL [V state from ER's barrier]. Why this is valid D3D12 usage, and why the alternatives were not taken:
    2.4 (built 2026-10-21, vmtest P2.1-P2.5 incl. GPU-based validation).
  - The list is spliced ahead of the armed list with the existing patch-0002 splice (`at == 0`, `armed_idx_max=0`
    [V log]), followed by our fence Signal. The task thread reads the buffer once the fence passes.
  - Points: a 4x4 grid over x 0.40..0.60, y 0.30..0.42 (above the gun, below the boss bar). A point is "empty" when
    rgb == 0. Its alpha 255 votes transmit, alpha 0 votes premul.
  - Objects: the sample's root signature, PSO, SRV/UAV heap and buffers are created on the PSO worker, never on the
    render thread.
  - Any HRESULT failure ends P2 for the session (`sample off why=..`). The probe itself goes on.

### 2.2 Exact log lines (all start `UI probe41`; the C4 parser ignores them because they do not contain `UI probe `)

```
UI probe41 on sample=0|1                                                                                   (boot, once)
UI probe41 pins which=layer|scene vt=0|1 vt7=0|1 sub=0|1 slot0=0|1 slot1=0|1 tex=0|1 arr=0|1 count=<n> res=0x.. res_mod=0|1 rtv=0x.. rtdata_fmt=<n> rtdata_rtv_eq=0|1     (first + on change)
UI probe41 thread mt=0|1 rt_tid=<n> hal_tid=<n> begin_tid=<n> present_tid=<n> rt_ok=<n>/<n> rtid_changes=<n> ahead=<n> present_in_ui=<n>          (5 s)
UI probe41 layer res=0x.. fmt=<n>:<NAME> w=<n> h=<n> mips=<n> samples=<n> flags=0x.. rt_ok=0|1 bb=<w>x<h>:<NAME> size_eq=0|1 res_changes=<n> rtdata_ok=<n>/<n>   (5 s)
UI probe41 scene res=0x.. fmt=<n>:<NAME> w=<n> h=<n> mips=<n> samples=<n> flags=0x.. rt_ok=0|1 bb=<w>x<h>:<NAME> size_eq=0|1 res_changes=<n> rtdata_ok=n/a           (5 s, info)
UI probe41 output b13c=0|1 b13e=0|1 eb9=0|1 scale=<x>x<y> shaders=0|1 blend=sdr2sdr|hdr|sdr2hdr[_upsample]                     (first + on change)
UI probe41 between n=<k> frame=<i> list=0x..                                                               (each event, first 4)
UI probe41 recreate what=sel|res|rtdata which=layer|scene old=0x.. new=0x.. t=<s>                          (each change)
UI probe41 sample n=<k> empty=<e>/16 a255=<n> a0=<n> other=<n> conv=transmit|premul|unknown px=<r,g,b,a;...4 points>             (5 s, P2 only)
UI probe41 sample off why=config|gate|hresult|pso                                                          (once, if P2 stops)
UI probe41 verdict ready=0|1 why=<rows or none> output=sdr|hdr alpha=transmit|premul|unknown              (once at 60 s, then every 60 s)
```

Fields:
- `rt_ok` counts UiBegins where `begin_tid == hal_tid == rt_tid`.
- `ahead` counts UiBegins whose seq equals the previous UiBegin's seq.
- `size_eq` = desc w/h equal to the back buffer's.
- `blend` is derived the way `141afce70` selects (optB 2).
- `conv` uses all samples so far: transmit or premul when >= 90% of the empty points (at least 48 of them) agree,
  otherwise unknown.

### 2.3 What each line proves (`analyze_c4.py` C4.1 rows; all PASS -> `C4.1 PROBE VERDICT: READY output=.. alpha=..`)

| row | assumption | PASS when |
|-----|------------|-----------|
| hooks41 | - | the C4 `hooks` row PASSes, plus `probe41 on` |
| thread41 | J1, J10 | `mt=1` in every line; `rt_ok` = all begins; `rtid_changes=0`; `present_tid != 0`. (`same_thread` / `begin_tid_ok` of C4 are not used.) |
| order | J2 | last `thread` line: `ahead=0`, `present_in_ui=0`; last C4 `ecl` line: `late=0`, `armed_hit == armed`, `armed_idx_max=0`, `armed_n_max=1` |
| adjacency41 | J8 | `between_other <= 1`, and every `between` line has `frame <= 2` (the first armed frames after the HAL appeared). Otherwise zero tolerance, as in C4. |
| pins | J3 | every `pins which=layer` line has all 7 flags = 1, `count >= 1`, `res_mod=1`, `rtdata_rtv_eq=1`, `rtdata_fmt` = the layer fmt; at most 2 distinct `res` values per session (one recreate) |
| layer | J4 | every `layer` line: `fmt=28:R8G8B8A8_UNORM`, `rt_ok=1`, `mips=1`, `samples=1`, `size_eq=1`, `rtdata_ok` = all |
| output | J4 | `shaders=1`, `scale=1x1` (no `_upsample`); `b13c=0` -> `output=sdr`; `b13c=1` -> `output=hdr` (PASS, but reported as "needs `inject_ui_layer`") |
| alpha | J5 | `sample` lines present, and the last one has `conv=transmit` or `conv=premul`. Without P2: FAIL "P2 off" |
| recreate | J9 | reported only: the count of `recreate` lines and whether a `res` change was followed by matching `pins` / `layer` lines |
| coverage | - | reported only: the C4 `nomovies` share |

The DLL computes the same verdict (`c4::verdict41`, pure, tested) and logs it. The analyzer compares it with its own
verdict: a disagreement means NOT READY, as in C4.

**analyze_c4.py changes (extend, do not break):**
- `ROWS`, `analyze()`, `report()`, `GOOD`, `BREAKS` and the `C4 PROBE VERDICT:` line stay byte-for-byte as they are,
  and so does the exit code without flags. check_c4 check 6 keeps passing.
- New: `ROWS41`, `analyze41(lines)` (None when there is no `UI probe41` line) and `report41()`.
  `report41()` prints `C4.1 PROBE VERDICT: READY output=sdr|hdr alpha=..`, `NOT READY`, or
  `C4.1 PROBE N/A: no 'UI probe41' lines`.
- `report()` is followed by `report41()`.
- Exit code: the new flag `--c41` exits with the C4.1 verdict (0 / 1 / 2). Without the flag it is unchanged.
- Self-test: the existing cases, plus `GOOD41` (READY) and one `BREAKS41` mutation per row:
  - `mt=1` -> `mt=0`;
  - `ahead=0` -> `ahead=3`;
  - a `between` line with `frame=900`;
  - `slot1=1` -> `slot1=0`;
  - `fmt=28:R8G8B8A8_UNORM` -> `fmt=10:R16G16B16A16_FLOAT`;
  - `scale=1x1` -> `scale=0.75x0.75`;
  - `conv=transmit` -> `conv=unknown`.
  It also covers a C4.1 N/A log and a DLL disagreement. The summary line keeps its form:
  `analyze_c4 self-test: n/n PASS`.
- Replaying today's log gives `C4 PROBE VERDICT: NOT READY` (as now) and `C4.1 PROBE N/A`.

### 2.4 P2 as built: why reading ER's layer without a barrier is correct D3D12 (2026-10-21)

The question: our compute list reads ER's UI layer `res`. D3D12 requires that, when our commands execute on the GPU,
the subresource is in a state that includes the access (NON_PIXEL_SHADER_RESOURCE for a compute SRV read), that the
data written before is visible, and that nothing writes it concurrently. The design's "no barrier on ER's resource"
holds **only because of where the list runs**, so that placement is enforced, not assumed:

1. **Placement.** The list is recorded in UiBegin's detour and executed only through hudhook patch 0002's splice of
   the ECL call that carries the armed Scaleform list (`[hal+0x1420]` after UiBegin's original) at index 0
   (`c4::sample_ecl41`: `at == 0` only; `at > 0` cancels), on the HAL queue, from ER's render thread
   (`tid == ARMED_RTID`), in the same PRESENT seq. hudhook runs `ECL(ours)`, `Signal(our fence)`, `ECL(ER's lists)`
   back to back on that queue.
2. **State.** The first command of the armed list is ER's own transition `0xC0 -> RENDER_TARGET` on `res` [V, C4 1.1].
   A legacy transition's StateBefore must equal the subresource's state at that point of the queue, so ER itself
   guarantees `res` is in `0xC0` immediately after our list; our list contains no barrier on `res` and changes no
   state, so `res` is in `0xC0` for our whole list. `0xC0` includes NON_PIXEL_SHADER_RESOURCE. `res` is a texture in an
   explicit state (not COMMON-promoted, not SIMULTANEOUS_ACCESS), so no implicit promotion / decay applies at the ECL
   boundaries.
3. **Visibility.** ER's previous writes to the layer (its last UI frame) were followed by ER's `RENDER_TARGET -> 0xC0`
   transition in UiEnd's list, which makes them visible to shader reads.
4. **Write-after-read.** The next writer is ER's own `0xC0 -> RENDER_TARGET` transition in the following ECL on the
   same queue; a transition barrier orders all earlier GPU work on the queue that accessed the subresource before the
   write. Nothing else writes the layer: the queue is ER's DIRECT HAL queue (checked equal to hudhook's present queue,
   `queue_ok`), and `queue_eq_present=1` / `armed_idx_max=0` / `armed_n_max=1` held in every probe run [V log].
5. **Lifetime.** `res` is validated in this UiBegin (7 pins, the D3D12-module vtable, a fresh GetDesc: 2D, 1 mip, 1
   sample, back-buffer size, R8G8B8A8 / B8G8R8A8 UNORM) and is referenced by ER's own armed list of the same ECL call,
   so it lives at least until that list completed. The SRV descriptor is written into OUR shader-visible heap at
   every record (a CPU descriptor write, no allocation), never cached across frames: a layer recreated at the same
   address (C4 review lesson) can never be read through a stale descriptor. The descriptor is rewritten only when the
   previous sample's fence passed or its list never ran (`SampleState` Idle).
6. **Our own objects.** The UAV buffer goes `COMMON -> UNORDERED_ACCESS -> COPY_SOURCE -> COMMON` with explicit
   barriers inside our list (no reliance on buffer promotion / decay); the readback buffer stays in COPY_DEST and is
   read by the task thread only after our fence value completed. Everything is created by a worker
   (`mw2er-c41sample`) and never released in-session.

**Alternatives considered and rejected:**
- *Sample at Present on our own list* (the layer is in `0xC0` after the composite): the Present thread and ER's render
  thread submit to the same queue from different threads, so the queue order of our list against the next UiBegin's
  list is only as good as J2 (ahead / present_in_ui), and a missed frame would put our read next to ER's
  `0xC0 -> RT` with no ordering argument from ER's own barrier. The splice ties the read to the one place where ER
  itself asserts the state.
- *A copy* (`CopyTextureRegion` of the layer) needs `COPY_SOURCE`, i.e. a barrier on ER's resource. Rejected: P2 must
  not change ER's state sequence at all.
- *A barrier pair `0xC0 -> NON_PIXEL -> 0xC0`*: valid but pointless (the state already includes NON_PIXEL) and it
  would put our barriers on ER's resource.

**What it reads:** the layer as ER's previous UI frame left it (what the previous composite blended) or ER's fresh
clear, whichever ER does first; both carry the empty-pixel convention.

**Proof offline (vmtest `--c4 --warp`, the DLL's own `LayerSampler`, debug layer):**
- P2.1 R8G8B8A8 "transmit" (empty (0,0,0,1)), P2.2 "premul" (empty (0,0,0,0)), P2.3 B8G8R8A8: the layer is left in `0xC0`
  by an earlier list, ours is spliced ahead of an ER-like list A (`0xC0 -> RT`, a UI box over every sample point,
  `RT -> 0xC0`). Results: the 16 x 4 texels equal a CPU readback taken before (B8G8R8A8 swizzled back to RGBA), A's
  box is never seen (queue order), A's box is in the layer afterwards (A's before-state `0xC0` held), the vote gives
  transmit / premul / transmit, and our record plus the spliced execution store **zero debug-layer messages of any
  severity**. The harness's own rect clears emit one known WARNING each (clear value != optimized clear value); those
  are drained separately and any other harness message fails the case.
- P2.4: a format the shader does not read is refused before any GPU work.
- With `--gbv` (GPU-based validation, which checks the SRV's resource state at the Dispatch): P2.1-P2.3 still 0
  messages, and **P2.5, the negative control**, samples a layer left in RENDER_TARGET and GBV reports
  `Incompatible texture barrier layout` at the Dispatch. So the zero count is a real check.

**Run-time guards (`ui_sample.rs`):** the gate `c4::sample_gate41` (due, Idle, HAL + movies + armed list, the layer
path, multithreaded mode + `tid == hal_tid == rt_tid`, HAL queue == present queue, chain + module, fresh desc; a valid
layer in another format ends P2 with `why=gate`). The `at == 0` splice only, and only in an ECL the injection did not
splice. UiEnd cancels an unsubmitted sample. A record HRESULT, a failed splice Signal, a removed device, or a fence not
passed after 10 s ends P2 (`why=hresult`), and then the verdict's alpha is unknown (`c4::sample_alpha41`). The render
thread does no allocation, creation, log or blocking lock (try_lock; one GetDesc, one descriptor write, the record).

Lines as built (in addition to 2.2):
```
UI probe41 on sample=1                                                                   (boot)
UI probe41 sample ready device=0x.. points=16 (every 5 s; our compute list, no barrier on ER's layer)   (worker)
UI probe41 sample n=<k> empty=<e>/16 a255=<n> a0=<n> other=<n> conv=transmit|premul|unknown px=<4 points>   (each sample)
UI probe41 sample none yet (miss=<label:n,..> locked=<n> cancelled=<n>)                  (60 s, while no sample was read)
UI probe41 sample off why=gate|hresult|pso (samples=<n> miss=.. [err=..])                (once)
```
`analyze_c4.py --c41` alpha row: PASS when the last sample line's `conv` is transmit / premul, equals the analyzer's own
vote from the line's counts, and no `sample off why=hresult` line exists.

---

## 3. Implementation (PASS I41, behind `[ui] inject_ui_thread = false`)

### 3.1 Config (built in PASS I41, 2026-10-21: the key exists, false in mw2er.toml)

```toml
[ui]
probe_scaleform    = false   # cold; C4 + C4.1 probe lines
probe_layer_sample = false   # cold; probe P2 (GPU read of 16 layer texels, no barrier on ER's resource)
inject_viewmodel   = false   # cold on / hot off; master switch (unchanged)
inject_ui_thread   = false   # NEW, cold; C4.1: UiBegin on ER's render thread, the pinned chain, the layer convention
inject_ui_layer    = false   # C4.1 meaning: also allow ER's HDR output (b13c = 1); SDR output needs only inject_ui_thread
```

- `inject_ui_thread = true` without `inject_viewmodel` gives a warning and is ignored. A hot false -> true is ignored
  (`UI inject on ignored (cold)`).
- Hot `inject_viewmodel = false` still turns everything off at once, as in C4 3.1.
- m2_static_checks c3wiring rule 10 then pins `impl Default for UiCfg` with all five fields false.
- With `inject_viewmodel` on and `inject_ui_thread` off, C4 behaves exactly as built. Its thread rule never passes on
  this PC, so every frame is a `notready` miss and draws at Present.

### 3.2 Per-build values (`ScaleformUi`, exe_build.rs; 2.7.2.0 only)

New fields, all RVAs [V, re-read above]:
- `g_mt: 0x11c`, `g_rt: 0x6d0`, `rt_tid: 0x14`;
- `sel_vt: 0x2f0ae60`, `sel_vt_count: 0x19f3dc0` (slot 7), `sub_vt: 0x2f0af48`, `sub_slot0: 0x19f49a0`,
  `sub_slot1: 0x19f4970`, `sel_tex: 0x40`, `sel_arr: 0x78`, `obj_rtv: 0x10`;
- `owner_rt: 0xdd0`, `rt_data: 0x20`, `rtdata_fmt: 0x20`, `rtdata_rtv: 0x28`;
- `g_eb9: 0xeb9`, `g_scale: 0xed0`, `g_shaders: 0x740`;
- `layer_alpha: Option<LayerAlpha>`, which is **None** until the probe run's `conv` is recorded in MODLOG. With None,
  every frame is a miss `alpha_unknown`.

Site rows: `scaleform_sel_vtable` and `scaleform_sub_vtable` are `Check::VtableNoRtti`, and the sub vtable's RTTI COL
is pinned too. `scaleform_sub_slot0/1` and `scaleform_sel_count` are `Check::Code(16 B)`. The ignored test
`c4_real_exe.rs` pins every value against the exe and gets one mutation per pin.

### 3.3 Gate (pure, `c4::gate41`; C4 `gate` stays for the C4 rule)

The order is the same as C4 3.3. Only these rows change or are new:

1. `enabled`, `build`, `hal != 0`, `movies > 0`, `vt_ok`, `hal_flags & 3 == 3`: unchanged.
2. **thread:** `mt != 0`, else miss `st_mode`. `tid == hal_tid == rt_tid` and `present_tid != 0`, else miss
   `notready`. Once the C4.1 probe verdict was ready, a mismatch trips with `thread`.
3. **ahead:** the previous UiBegin's seq != this seq, else miss `ahead`. Every UiBegin stores its seq, armed or not.
4. `first` (`begin_ok`), `wants`: unchanged.
5. **pins:** all 7 pins, `count >= 1`, `res != 0`, `res_mod`. A pin mismatch after a good frame trips with `pins`.
   A zero pointer is a miss `notarget`.
6. **recreated:** the rtData cross-check fails, or `res` differs from the last good frame's. That is a miss
   `recreated`, and the new `res` must pass two frames in a row before it is used. If this flaps 3 times in 10 s, it
   trips with `format_flap`, reusing FlapWindow.
7. **output:** `shaders != 0`, render scale == 1.0 (else miss `scale`). `b13c == 0`, or `inject_ui_layer`
   (else miss `hdr_off`).
8. **target:** a fresh GetDesc every UiBegin (unchanged). The new rule `target_rule41` accepts the layer only as
   R8G8B8A8_UNORM / B8G8R8A8_UNORM with w/h == the back buffer (else miss `size`). The scene path keeps C4 4.1. It
   only occurs before the targets are built.
9. **alpha:** `layer_alpha` is Some, and the session canary passed (3.6), else miss `alpha_unknown` / `canary`.
10. `device_ok`, `slot_free`, `pso_ready`: unchanged. PSOs are keyed by (format, convention).

New `Miss` values. Expected misses: `st_mode`, `ahead`, `recreated`, `scale`, `size`, `alpha_unknown`, `canary`,
`warmup`. Trips: `pins`, `order`, `alpha`, `cleared`. `Miss::ALL` grows, and so do the labels and the
`c4_gate_every_reason` test.

### 3.4 Frame state (unchanged machine, one new rule)

The `u64` state, `begin_ok` / `armed` / `ecl_submit` / `ui_end` / `present` stay as they are. They are correct for J2:
- UiBegin(N+1) reads the seq that Present(N) incremented after its callback.
- Present(N+1)'s callback sees that seq.

New:
- **`ahead`** (3.3 row 3). Without it, a missed frame N (state left Closed(s-2)) followed by a UiBegin(N+1) that
  arrives before Present(N)'s increment would arm for seq s-1. Present(N) would then skip the gun for image N
  (neither), and image N+1 would get it twice (both).
- **Present inside a UI frame:** a Present callback while `F_BEGIN_SEQ == seq && F_END_SEQ != seq`.
  - If the state is Armed, it is cancelled as today, and that frame draws at Present.
  - If the state is Submitted, Present **draws anyway** (a one-frame double gun at worst) and trips with `order`.
    J2 is broken, so the session falls back.
- New pure tests:
  - `c4_state_mt_two_threads`: an exhaustive interleaving of {Present callback, seq++, UiBegin, ECL(armed), UiEnd} for
    3 frames, plus misses. Each presented image gets the gun exactly once, where an image's UI frame is the one whose
    UiBegin read its seq.
  - `c4_state_ahead_refused`.
  - `c4_state_present_in_ui_trips`.

### 3.5 What the early list writes (mw2er-render, PASS I41 only)

- Target: `res` with ER's own RTV format. The RTV is ours: CreateRenderTargetView into our slot's descriptor, never
  ER's `er_rtv`. The viewport and depth are the target's (== the back buffer's).
- Barriers `0xC0 -> RT -> 0xC0` (unchanged).
- Output encoding: Srgb OETF into the UNORM layer, the same values as at Present today [V: SDR2SDR blends in gamma
  space, I identity].
- **The PSO convention variant** replaces the bool `ui_layer`:

  | convention | opaque PSO | alpha PSO (straight alpha α) | overlay |
  |------------|------------|------------------------------|---------|
  | premul (empty a=0) | RGBA write, A = 1 | color SRC_ALPHA / INV_SRC_ALPHA; alpha ONE / INV_SRC_ALPHA | as today's `ui_layer` |
  | transmit (empty a=1) | RGBA write, **A = 0** | color SRC_ALPHA / INV_SRC_ALPHA; alpha **ZERO / INV_SRC_ALPHA** | alpha ZERO / INV_SRC_ALPHA |

  With either convention, ER's composite shows the gun opaque where it is opaque. Scaleform's later draws blend over
  it like over any UI pixel.
- No allocation or resource creation on the render thread:
  - `prepare_depth(w, h)` is called by the worker as soon as the probe-style desc read has seen the layer size, so
    `ensure_depth` never creates a resource on the render thread. A size change is a miss `size` until the worker has
    re-prepared.
  - `record_early` copies the snapshot with `clone_from` into a render-thread buffer whose capacity was reserved on
    the worker. It no longer uses `clone()`.
  - The first UiBegin after going live is a miss `warmup`: it touches the thread-local `SCRATCH` (one allocation on
    first use) and records nothing.
  - The wiring check gets a rule for this: no `.clone()` / `Vec::new` / `CreateCommittedResource` reachable from
    `record_early` on the early path.
- Cut-out: the early path keeps `cut_count = 0` [V C5].

### 3.6 Run-time self-checks (injection only; they write only into ER's layer, as the gun does)

- **Canary (J6).** The first 3 armed frames after going live record only a 1x1 canary at pixel (2, 2), the top-left
  corner, outside every ER layout region. It is opaque in the pinned convention, value (1/255, 2/255, 3/255). The gun
  is not drawn in these frames: they are misses `canary`, drawn at Present.
  - A P2-style sample list, submitted from **UiEnd's** detour after the original, reads that texel back. The layer is
    in 0xC0 again [V]. The list is its own ECL on `[hal+0x1418]` from the render thread, followed by our Signal.
  - Canary present: log `UI inject canary ok`, and the early gun starts.
  - Canary absent: log `UI inject canary cleared`, trip with `cleared` (something clears the layer after UiBegin; the
    design does not hold).
  - The canary pixel is visible for 1 frame at most, as a dark dot in the corner.
- **Convention re-check (J5).** Every 60 s, the P2 sample at UiBegin. If `conv` disagrees with `layer_alpha` twice in
  a row, it trips with `alpha`.
- `GetDeviceRemovedReason`, `SPLICE_SIGNAL_FAILS` and `never_submitted` (30 in a row): unchanged.

### 3.7 Log lines (new or changed; C4 5.3 otherwise unchanged)

```
UI inject on build=2.7.2.0 path=layer mode=ui_thread output=sdr|hdr alpha=transmit|premul fmt=<NAME> size=<w>x<h> slots=3
UI inject off why=config|build|probe_verdict|hooks|alpha_unpinned
UI inject canary ok|cleared|unknown value=<r,g,b,a>
UI inject early=<n> present=<n> splice=<n> cancelled=<n> fallback_cut=<n> miss=<why:n,...>          (5 s)
UI inject trip why=<reason> frame=<n>
```

`fallback_cut` counts Present-drawn frames while `snap.ui_cut_n > 0` (5).

### 3.8 PASS I41 as built (2026-10-21; offline only, nothing has run in game)

Behind `[ui] inject_ui_thread = false` (cold on; `inject_viewmodel = false` or `inject_ui_thread = false` hot turns it
off). With the key off, every C4 path is byte-for-byte the old one (the PSO refactor below yields identical blend states,
proven by `c4_alpha_variants_blend_states` and vmtest C4.1-C4.7 / `--all`).

- **Boot.** `ui_probe::install_ui(probe, sample, inject41)` checks the C4.1 sites for the injection too (`INJ41`). If
  `inject_ui_thread` is on and the sites fail: `UI inject off why=build`, nothing installed (never the C4 rule instead).
- **Going live** (`c4::live41`, task thread). Probe off: the per-build pin `ScaleformUi.layer_alpha`, else
  `UI inject off why=alpha_unpinned`. The pin is still **None** for 2.7.2.0 (no P2 run yet), so today the injection only
  goes live with `[ui] probe_scaleform = true` + `probe_layer_sample = true` and a `UI probe41 verdict ready=1`. Probe on:
  wait for the verdict; ready -> its alpha (a pin that contradicts it -> `off why=probe_verdict`); not ready only on
  `alpha` -> the pin if any, else wait while P2 samples (at most 600 s), then `off why=alpha_unpinned`; any other row ->
  `off why=probe_verdict`. Log: `UI inject live mode=ui_thread alpha=.. (from ..)`.
- **Gate** `c4::gate41` (3.3 order; `GateIn41`): the render-thread rule `tid == [hal+0x50] == [[G+0x6d0]+0x14]` and
  `mt` (a mismatch trips `thread` only after a good armed frame), `ahead`, the layer path, the 7 pins (`pins` trip after a
  good frame), the resource module check (done by the TASK thread on the vtable the detour stores: no loader lock on ER's
  render thread; `None` = `notready`), `res_step41` (a new / recreated layer resource is used after two UiBegins in a row
  with the rtData cross-check; 3 changes in 10 s trip `format_flap` via the allocation-free `FlapRing`), render scale /
  shaders (`scale`), HDR output (`hdr_off` unless `inject_ui_layer`), `layer_rule41` (R8G8B8A8 / B8G8R8A8 UNORM, 1 mip,
  back-buffer size), the convention, `warmup`, the canary, device, slot, depth (`size` until the depth worker prepared that
  size), PSOs. The AC-130 / hidden / third-person rows come before any chain read.
- **Frame state.** Unchanged machine plus `c4::present41`: a Present inside an open UI frame (this seq's UiBegin ran, its
  UiEnd not yet) whose list was spliced draws at Present anyway and trips `order`. `c4_state_mt_two_threads` interleaves
  {Present callback, seq++} with {UiBegin (+gate / `ahead`), ECL, UiEnd} for 3 frames (5005 orders x 64 pass / ECL
  patterns): every presented image gets the gun exactly once, or twice only with the `order` trip.
- **PSO variant.** `mw2er_core::c4::EarlyMode {Scene, Premul, Transmit}` replaces the bool `ui_layer` (C4's `ui_layer` is
  `Premul`). `mw2er_render::layer_blend(mode, alpha)` is the 3.5 table; transmit opaque uses alpha factors ZERO / ZERO (the
  shader keeps writing A = 1). `c4::layer_write41` / `ui_over41` / `composite41` are the pixel math; a render test checks the
  blend states against it numerically, and core tests check composite(UI over gun over scene) for both conventions.
- **Nothing on ER's render thread allocates or creates.** `viewmodel::record_early41` copies the snapshot fields into
  `EARLY_SNAP` (world guns into a vector the worker reserved; at most MAX_WORLD_DRAWS), builds the world draws in a stack
  array, poses into `EARLY_SCRATCH` (reserved for 128 bones; `PoseScratch::fit` now refills in place), and opens the list
  with `EarlyPass::begin_prepared`, which refuses (never creates) a missing depth / PSO. Errors stay `VmError`s, formatted
  by the task. PSOs per (layer format, convention) are built by the `mw2er-c4pso` worker on request, the depth by the new
  `mw2er-c4depth` worker. `m2_static_checks c41wiring` (8 rules, 12 mutations) pins this. Review C4.1 fix: the
  mw2er-render functions reached from there (`EarlyPass::finish`, `Renderer::record_{gun,world,overlay}_early`) built
  `VmError::Invalid(format!(..))` inside `ok_or(..)`, which Rust evaluates even on success (a HeapAlloc / HeapFree pair
  every armed frame); every error there is now a `VmError::D3d` with a static str, and c41wiring rule 9 scans those
  functions too (10 rules, 16 mutations).
- **Canary (3.6), as built.** The first armed frame after going live records only the canary (`EarlyPass::record_canary`,
  shaders/solid.hlsl, one texel at (2, 2), value (1, 2, 3) with A opaque in the convention, no blend) between the usual
  `0xC0 -> RT -> 0xC0`; the gun of that frame is drawn at Present (`canary` miss). The readback runs in UiEnd's detour
  AFTER the original, only if (a) the frame was Submitted, (b) the ECL callback saw an ECL on the HAL queue from ER's render
  thread WHILE UiEnd's original ran (its EndFrame submit: ER's `RENDER_TARGET -> 0xC0` on the layer, recorded by UiEnd
  before EndFrame [V C4 1.1], is then queued ahead of us), and (c) the queue is hudhook's present queue. It records the
  sampler (a second `LayerSampler`, created by the early-pass worker) on the layer in `0xC0` (no barrier on ER's resource,
  as P2) and executes it with our Signal by our own `ExecuteCommandLists` on ER's queue from ER's render thread. The task
  reads it after the fence: exactly the canary -> `UI inject canary ok`, then `UI inject on .. mode=ui_thread ..`; empty ->
  `canary cleared` and trip `cleared`; anything else -> another canary frame, 3 tries, then `cleared`. A readback that
  could not go out (no EndFrame ECL seen) 3 times -> `canary unknown` and trip `cleared`.
- **Persistence check (review C4.1, 2026-10-21).** `canary ok` only shows that nothing clears the layer AFTER our splice
  inside UiBegin..UiEnd; a layer ER never clears between UI frames (or clears only where it redraws) would also read
  `ok`, and every early gun would then stay in it (trails, the canary dot for good, and a frame whose UiBegin does not
  run compositing the old gun while Present draws the new one). optB_layer.md 3 found no per-frame clear call site [I],
  so this is now checked at run time: after `canary ok` the canary goes to `Check` (`c4::CanaryGate::Check`), and the
  next armed frame records an EMPTY list (`viewmodel::record_canary41(q, None)`: the two barriers only; the gun of that
  frame draws at Present, a `canary` miss). Its UiEnd readback reads the canary texel as point 0
  (`readback_points41(.., true)`); `c4::canary_gone41`: empty in the pinned convention (rgb 0, a 255 transmit / 0
  premul) -> `UI inject canary gone`, the gun starts (`CANARY_PASSED` is set only here); still the canary -> `UI inject
  canary persisted` and trip `cleared`; anything else -> another check frame, 3 tries, then trip. The unreadable-readback
  counter restarts for the check. vmtest I41.6t/p: list A without a clear -> `persisted`, list A clearing at its frame
  start -> `gone`, 0 debug-layer messages. `m2_static_checks c41wiring` rule 10 pins it; the analyzer's `canary` row needs
  `canary gone` and no `persisted`.
- **Re-check.** Every 60 s after the canary passed, the same UiEnd readback on a Submitted gun frame reads the 16 grid
  texels (at UiEnd the layer holds this frame's UI; empty texels carry the convention). One sample votes with >= 8 empty
  texels at 90 %; two disagreeing votes in a row trip `alpha`. Line: `UI inject recheck vote=.. alpha=.. empty=<e>/16
  disagree=<n>`. (Design 3.6 said "the P2 sample at UiBegin": with the injection arming every frame, P2's splice never gets
  its own ECL, so the re-check moved to UiEnd.)
- **Present path.** Skips world guns, gun and overlay only on a Submitted gun frame (canary frames and `order` frames draw
  at Present). `fallback_cut` counts Present-drawn frames with a C5 cut-out region up while the injection is live. Early
  frames carry `cut_count = 0`; `[ui_yield] lower / hide` only on Present-drawn frames; AC-130 frames never arm (thermal owns
  them); the nuke post pass is unchanged.
- **Lines** (in addition to 3.7): `UI inject hooks ready mode=ui_thread|present_thread ..`, `UI inject live mode=ui_thread
  alpha=..`, `UI inject PSOs ready fmt=.. mode=..`, `UI inject depth ready WxH ..`, `UI inject canary ok|cleared|unknown
  value=r,g,b,a`, `UI inject recheck ..`; the 5 s line in C4.1 mode is `UI inject early=.. present=.. splice=..
  cancelled=.. fallback_cut=.. miss=..`.
- **Offline proof.** vmtest `--c4 --warp` I41.1 / I41.2 (transmit / premul): the gun list is recorded on a second thread
  with `begin_prepared`, spliced ahead of an ER-like list A drawing a half-transparent UI quad "over" in the convention;
  a CPU composite shows the scene where the layer is empty, the gun opaque (alpha 0 / 255) and UI over the gun inside A's
  box (213073 / 213073 px), with 0 debug-layer messages of any severity (also with `--gbv`). I41.3 the canary read back
  after A: ok; I41.4 A clears the layer: cleared; I41.5 the re-check votes the layer's own convention.
- **Still [I]:** that UiEnd's EndFrame submit carries the layer's `RT -> 0xC0` (guarded by the ECL-inside-UiEnd
  observation, not proven); J6 / J7 (the canary, its persistence check and the solo test answer them); that ER also
  clears the layer on frames whose UI job does not run (movies == 0, loading screens: the persistence check sees only
  frames with a UI job; watch for a stale or doubled gun on those transitions in 7.2); the layer convention itself (P2
  run); and **the composite math (3.5 table) against ER's real `_BlendUI_SDR2SDR_F`** (review C4.1). P2 pins only the
  empty pixels' alpha (a = 255 vs a = 0). It does not show that the layer stores premultiplied colour or that the blend
  is `scene*a + ui` / `scene*(1-a) + ui`: a straight-alpha composite `lerp(scene, ui.rgb, ui.a)` (empty a = 0) would
  also be labelled premul, and its twin `scene*a + ui*(1-a)` transmit. Opaque gun pixels look right under every one of
  them, but every alpha-blended gun draw (alpha_mode = 1: red-dot / holo reticles, lens glass, alpha-tested foliage on
  the hands) and the scope overlay's soft edge would composite as `scene*(1-a) + gun*a^2` (darker, fringed). The LUT
  `[G+0xEC8]` the composite binds could also shift the gun's colours. `composite41`, the I41.1 / I41.2 CPU composite and
  the render test all build in the same assumption, so they cannot catch it; only 7.2 step 4b can. If it differs: a
  third `EarlyMode` (straight: colour ONE / ZERO on alpha draws with A = alpha), or alpha_mode = 1 items drawn
  alpha-tested only on the early path.

## 4. Failure handling

- **Expected misses** (count only; that frame draws at Present, plus the cut-out): the C4 list plus `st_mode`,
  `ahead`, `recreated`, `scale`, `size`, `alpha_unknown`, `canary`, `warmup`, `hdr_off`.
- **Trips** (off for the session; the task thread runs `MH_DisableHook`, which is passthrough; trampolines are never
  freed, as in the review-C4 teardown): the C4 list plus `pins`, `order`, `alpha`, `cleared`. A thread mismatch trips
  after a ready verdict.
- **After a trip:** everything is today's Present path. `[ui_yield] mode = "cutout"` keeps cutting ER's message regions
  out of the gun, so the user still sees the messages.
- **Probe-verdict gate:** with `probe_scaleform` on, the injection waits for `verdict41` (60 s) and logs
  `off why=probe_verdict` if it is not ready. With the probe off, the 3.3 gates enforce the same facts every frame.
- **Never both, never neither:** the 3.4 state machine. A frame state that is not Submitted at Present always draws at
  Present.

## 5. Interaction with `[ui_yield] mode = "cutout"` (C5) and the other passes

C5 is built (MODLOG 2026-10-20 C5 entry). C4.1 does **not** edit `config.rs`, `viewmodel.rs` or the renderer in its
probe pass. The PASS I41 renderer and viewmodel edits come after C5 is merged and reviewed.

| pass | rule |
|------|------|
| C5 cut-out | Early-drawn frames: no cut-out (`cut_count = 0`): the gun is already under the UI, so a cut would punch a hole that shows the scene. Present-drawn (fallback) frames: the cut-out exactly as C5 does it. The snapshot's `ui_cut` is computed as today in both cases. |
| mixed frames | Isolated misses (`busy`, `locked`, `recreated`) show the cut for 1 frame. `fallback_cut` counts how often that happens while a message is up. The solo test watches for flicker in message regions. If it is visible, C4.2 adds a hold: after 30 Submitted frames in a row, a single fallback frame draws without the cut. |
| world guns / scope overlay | They follow the gun's path (unchanged). In cut-out mode the world guns are cut only on fallback frames. |
| ui_yield `lower` / `hide` | Applied only on Present-drawn frames (unchanged C4 rule). |
| AC-130 thermal, nuke post, ImGui HUD | Unchanged (C4 6). |
| ER screenshot path `141c568e0` | It blends the same layer, so the gun appears in ER's own screenshots / thumbnails. This is expected. |
| C4 rule (`inject_ui_thread = false`) | Unchanged. With this PC's threads it never arms. |

## 6. Offline checks (main session runs `check_c4.ps1`; nothing here runs it)

- `cargo test -p mw2er-core --lib`: new `c4_chain41_*` (pins, count, every zero-pointer step, rtData mismatch),
  `c4_gate41_*`, `c4_state_mt_*`, `c4_probe41_*` (line formats, `verdict41`), `c4_fmt41_*`, `c4_alpha_*` (the
  convention vote).
- `cargo build --release -p mw2er`: 0 warnings.
- `c4_real_exe.rs`: the 3.2 values against the exe, with mutations.
- `analyze_c4.py --self-test`: C4 + C4.1 cases. The replay of today's log: C4 NOT READY, C4.1 N/A.
- vmtest `--c4` (PASS I41), new cases on WARP with the debug layer:
  - the R8G8B8A8 "layer" with each convention: our list draws a gun-like quad, an ER-like list A draws a
    half-transparent "UI" quad over it, and a fake composite `scene*a + ui` / `scene*(1-a) + ui` must show
    UI-over-gun-over-scene;
  - the canary (a clear inside list A -> `cleared`);
  - the P2 compute read of a 0xC0 resource with no barrier (zero debug-layer messages).
- `m2_static_checks c4wiring`: the no-alloc rule (3.5); the canary and sample lists never barrier ER's resource; P2
  only when `probe_layer_sample`.

## 7. In-game test steps (user; logs in `logs\mw2er.log`)

### 7.1 Probe run (after PASS P41), solo, offline, about 5 minutes

1. `mw2er.toml`: `[ui] probe_scaleform = true`, `probe_layer_sample = true`, `inject_viewmodel = false`. Windows HDR
   as you normally play: **nothing needs to be switched off** (1.1).
2. Launch as usual and play at least 3 minutes:
   - stand at a grace until "Sit at grace" shows; rest, leave the menu;
   - pick up an item;
   - open and close the pause menu, the map and the inventory;
   - fire, ADS with a sniper, and call AC-130 once;
   - **stand still for 20 s in an open area**, looking at the sky or the ground, with no message on screen (P2 empty
     samples).
3. Last step (optional, J9): in ER's Settings > Display, change the resolution once and change it back. Then quit
   normally.
4. Run `python -I tools\scripts\analyze_c4.py --c41 logs\mw2er.log`.
   - `C4.1 PROBE VERDICT: READY output=sdr alpha=transmit|premul`: the main session records `layer_alpha` and starts
     PASS I41.
   - Any FAIL row: stop. The row names the J that broke.

### 7.2 Solo injection run (after PASS I41)

1. `inject_viewmodel = true`, `inject_ui_thread = true`, `probe_scaleform = true`, `probe_layer_sample = false`.
   `[ui_yield] mode = "cutout"` stays.
2. Expect `UI inject canary ok`, then `UI inject canary gone` (the persistence check, 3.8), then `UI inject on ...
   mode=ui_thread`. `UI inject canary persisted` + `trip why=cleared`: ER does not clear its layer between UI frames;
   stop (the gun stays at Present with the C5 cut-out). A 1-pixel dark dot at the top-left corner may then remain.
3. Check that "Sit at grace", item pick-up banners, subtitles, boss bars, the HUD and the pause-menu darkening all
   draw **over** the gun. Note any ER element that is still under it (J7).
4. Check the gun's brightness and colours against `inject_viewmodel = false` (a hot toggle in the toml). It should
   look the same.
   4b. **The composite math (still [I], 3.8; review C4.1).** In the same hot toggle, look at what is translucent, not
   only at overall brightness: a red-dot or holo reticle (ADS with such a sight), lens glass, and the scope overlay's
   soft edge (ADS with a sniper). Same with the toggle on and off -> record "composite: same" in MODLOG. Darker or
   fringed with the injection on -> ER's composite is not the 3.5 math; turn the injection off and record it (C4.2: a
   straight-alpha `EarlyMode`, or alpha items alpha-tested only on the early path). `analyze_c4.py --inject` prints this
   step as `INFO manual` (the log cannot show it).
5. Check that there is no flicker or hole in message regions (5, mixed frames), and that the scope overlay is under
   ER's messages.
6. AC-130 on / off, a nuke, the pause menu.
7. Hot `inject_viewmodel = false`: expect `UI inject off why=config`, the gun back on top, and the cut-out working.
8. Analyzer rows:
   - early >= 95% of the frames with UI;
   - `cancelled = 0`;
   - no trip;
   - `fallback_cut` small;
   - no device-removed line.

### 7.3 Co-op run (only after 7.2 is clean)

Only the host turns it on. The friend keeps the default (off; their package is unchanged). If the friend later wants
it, they first run 7.1 once: their `b13c` decides whether `inject_ui_layer` is needed. Any trip or device-removed
event: back to off.

## S1. New reads, GPU work and restore

| What | Kind | Where | Safety |
|------|------|-------|--------|
| G / S / sel / T / obj / rtData fields, the 5 pins | non-faulting reads | render thread (UiBegin detour) | read only; a pin mismatch means no use |
| `res->GetDesc` | read-only COM call, borrowed | render thread | vtable module check cached (no loader lock) |
| P2 sample list | our compute list that reads ER's layer (no barrier), our UAV / readback | spliced at UiBegin / own ECL at UiEnd | reads only ER state; probe-only flag; our objects created on the worker |
| canary + gun | our list that writes ER's layer `0xC0 -> RT -> 0xC0` | spliced at UiBegin | ER's own next barrier asserts 0xC0 [V]; trips per 4 |
| game memory | - | - | **no writes** |
| hooks | none new | - | UiBegin / UiEnd / ECL from C4; teardown unchanged |

## S2. Task list

**PASS P41 (probe, read only; do not edit config.rs / viewmodel / renderer files)**
1. mw2er-core `c4.rs`:
   - `Pins41` + `chain41(sel, layout, read)` (returns res, er_rtv, count, flags);
   - `RtData` check;
   - the `ahead` / `present_in_ui` counters' pure parts;
   - `ProbeStats41` + `verdict41`;
   - the `fmt_probe41_*` line formats (2.2);
   - the alpha vote `conv_vote`;
   - tests `c4_chain41_*`, `c4_probe41_*`, `c4_alpha_*`.
2. mw2er-core `exe_build.rs` / `exe_sites.rs`: the 3.2 `ScaleformUi` fields (`layer_alpha: None`), the site rows, and
   the `exe_build_versions_and_tables` extension. `tests/c4_real_exe.rs`: pins + mutations.
3. mw2er `ui_probe.rs`:
   - extend `UiFacts` / `read_facts` with the 2.1 reads, for both candidates;
   - `probe_begin`: the `rt_ok` / `ahead` / pins / recreate atomics;
   - `on_present`: `present_in_ui`;
   - `probe_ecl`: the `between` frame index;
   - `ProbeLog::tick_c4`: the `UI probe41` lines and the 60 s `verdict41`.
   No alloc, log or lock in the detours.
4. P2 (`probe_layer_sample`), deferred until C5 is merged, because it needs a cold `[ui]` key (config.rs) and a small
   compute pass (mw2er-render). Steps:
   - a compute root signature + PSO + SRV/UAV heap + UAV/readback buffers, created on the PSO worker;
   - recording from UiBegin through the existing splice (`at == 0`), then the fence;
   - readback on the task thread;
   - the `sample` line.
   Until then, the probe run reports `alpha` FAIL "P2 off", and the run cannot be READY.
   **Built 2026-10-21** (2.4): `[ui] probe_layer_sample` (cold), `mw2er-render/src/sample.rs` + `shaders/layer_sample.hlsl`,
   `mw2er/src/ui_sample.rs`, the core `sample_*41` functions, vmtest P2.1-P2.5, analyzer alpha cross-check.
5. `tools\scripts\analyze_c4.py`: `ROWS41` / `analyze41` / `report41` / `--c41` / `GOOD41` / `BREAKS41` (2.3), with the
   C4 rows, output and exit codes unchanged.
6. MODLOG entry (also correcting the "HDR UI-layer path" wording); staged DLL; the user runs 7.1.

**PASS I41 (injection, behind `[ui] inject_ui_thread = false`)**, only after `C4.1 PROBE VERDICT: READY`:

7. Config (3.1) with cold/hot rules, validation and tests. Wiring rule 10 covers 5 fields.
8. `ScaleformUi.layer_alpha = Some(..)` from the probe run [V log].
9. mw2er-core: `gate41`, the new `Miss` values, the 3.4 rules and the `c4_state_mt_*` tests.
10. mw2er-render: the PSO convention variant (3.5 table), `prepare_depth` driven by the worker, the canary record, and
    the UiEnd sample list.
11. mw2er `ui_inject.rs`:
    - the thread rule, `ahead`, pins, recreate, canary, convention re-check;
    - `order` on Present-in-UI;
    - the 3.7 lines.
    `viewmodel.rs`: `record_early` without allocation (`clone_from`, warm-up).
12. vmtest `--c4` C4.1 cases, `c4wiring` / `c4strings` (3.7 lines), check_c4 extensions.
13. MODLOG; staged DLL; the user runs 7.2, then 7.3.

**Built 2026-10-21 (3.8):** tasks 7, 9, 10, 11, 12 (vmtest I41.1-I41.5; `m2_static_checks c41wiring` in place of
`c4wiring`; analyzer `--inject`; check_c4 check 4 prefixes and check 7 key). Task 8 (`layer_alpha`) waits for the P2 run
(7.1); until then the injection goes live only on a ready probe verdict with P2's alpha. `c4strings` not built.
