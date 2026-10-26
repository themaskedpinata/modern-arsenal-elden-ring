# M3 design: the gun on screen (UMP45 viewmodel in Elden Ring)

Status: design, 2026-10-14. Nothing here has been built or run in game yet. Inputs: `MODDING_PLAN.md`, `MODLOG.md`
(the 2026-10-14 entries), `docs/M2_DESIGN.md`, the four research findings of this workflow (renderer, xanim,
camera, mw2-rig), and the extra checks the designer ran while writing this (listed below).

M3 = draw MW2's first-person UMP45 viewmodel (gun + arms) inside Elden Ring's frame while MW2 mode is on, animated
from MW2's own animations (idle, fire, fire_ads, reload, reload_empty, pullout/putaway, sprint in/loop/out, ADS
in/out), lit simply (one directional light, hemispherical ambient, MW2 diffuse texture), for under 1 ms of GPU time
at 3840x2160. Also in scope: the hit-attribution fix and the analyzer recalibration (section 11), plus a camera
validity gate that removes the 170 m episodes (section 9).

Out of scope (M4 or later): the ER character's own weapon, procedural bob/sway/idle sway, ADS effects on spread or
world zoom, normal/specular/detail maps, capturing ER's depth buffer, world-space effects (tracers, impacts),
akimbo and attachments, camo variants.

## Evidence labels

The same labels as M2:

- **[V] verified**: checked on this PC: a file read, a parse, a compile, an offline run.
- **[R] reported**: a KB field note (CS2 conversion) or IW4L's reimplementation says so. Not tested here.
- **[I] inference**: our design choice or a guess. Each has an oracle or a log line that settles it.

Extra checks the designer ran (read-only):

- hudhook 0.9.3 `src/renderer/backend/dx12.rs` `render()` (lines 184-237) **[V]**:
  - The order is: fence wait (188-190), allocator and list Reset (192-193), CreateRenderTargetView into `rtv_heap_start` (195-203), barrier PRESENT->RENDER_TARGET, `OMSetRenderTargets(1, rtv, false, None)` (218), `SetDescriptorHeaps(srv_heap)` (219), `render_draw_data` (221), barrier RENDER_TARGET->COMMON, Close, Execute, Signal.
  - `frame_index` cycles over `NUM_FRAMES = 3` (line 28, 231).
- The dump has **38** `viewmodel_ump45_*` xanims **[V ls]**: 12 base, 8 attachment ADS, 18 akimbo. The mw2-rig finding's "37" was a miscount.
- Gun model variants: only `viewmodel_ump45_lod0.glb` and the camo variants exist as viewmodels. `viewmodel_base_viewhands_lod0.glb` is the hands **[V ls]**.
- `fxc.exe` exists at `C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe` **[V]**.
- The `png` crate is not in the local cargo cache **[V]**. It will be fetched from crates.io by cargo, which is allowed.
- The vendored IW4L is at `vendor\iw4L`, HEAD `fc3d8eaf6045cb131eb74cd41954f0c4922b9913`, with `NOTICE` and `LICENSE` present **[V]**.
- `mod\Cargo.toml` pins `windows =0.62.2` and `hudhook =0.9.3` (crates.io, stock) **[V]**. Current `hit_logic` attributes only via `last_hit_by` **[V source]**. The HUD gate is `hud.enabled && mode_on && in_world && !menu_open && now - s.t < SNAPSHOT_MAX_AGE_S` (hud.rs:96) **[V]**.

---

## 0. Summary of the decisions

| Topic | Decision | Label |
|---|---|---|
| Hook point | Vendored hudhook 0.9.3 + patch `hudhook-0001`: a pre-ImGui callback in `D3D12RenderEngine::render` between `OMSetRenderTargets` (218) and `SetDescriptorHeaps` (219). Our draw is recorded into hudhook's own command list on ER's present queue: after all of ER's frame work, under the ImGui HUD | V insertion point; I behaviour in game |
| Depth | Separate viewmodel depth range: our own D32_FLOAT buffer, reversed Z, cleared each frame, own projection. **No test against ER's depth, no ER depth capture in M3** | I (justified in section 5) |
| Space | Everything in MW2 space (inches, X forward, Y left, Z up) until the view matrix, which maps to D3D view space `(x,y,z) = (-Y, Z, X)` | V axes; I convention |
| Camera | The viewmodel needs no ER view matrix. Eye = posed `tag_view`. Projection = MW2 cg_fov 65 Hor+ (vertical 51.08°), blended to adsZoomFov 55 (42.65°) by the ADS fraction; aspect from the back buffer | I (IW4L-reported) |
| Anim data | Offline Python converter from OAT's compiled XAnim dumps to our own `.vmanim` (exact keys, no resampling). A second independent Rust reader cross-checks it | V format; I tooling |
| Anim semantics | A translation track replaces the bind translation. No translation track keeps the bind translation. No quat track gives identity rotation. iw4L's "delta" reading is rejected | V on data (idle = bind within 0.001 in) |
| Anim logic | New pure modules in `mw2er-core`: `vm_anim` (state machine), `vm_pose` (sampling, blending, FK, skinning palette), `vm_assets` (file parsers), `vm_proj` (projection math). All unit-tested | I |
| Renderer | New crate `mw2er-render` (windows =0.62.2, no hudhook): GPU skinning in the VS, 2 PSOs (opaque, alpha-blend), BC1/BC3 sRGB SRVs, root CBVs from a 3-slot upload ring indexed by hudhook's frame index. Used by both `mw2er.dll` and `vmtest.exe` | I |
| Shaders | HLSL compiled offline with `fxc` (SM 5.1, DXBC) in `mw2er-render/build.rs`, `include_bytes!` into both binaries. Runtime compile only as a fallback | V tool present; I choice |
| Colour | Shader-side output transform: sRGB OETF (default), PQ or scRGB, selected from the logged back-buffer format and a recorded `SetColorSpace1` value; `viewmodel.output` overrides | I (ER format unknown) |
| Failure | Any asset or renderer failure logs `VM FAIL reason=...`, sets `VM_DISABLED`, and M2 continues unchanged | I |
| Hits | Attribute an HP drop to us when one of our IMPACT markers landed within 0.25 s and near that character, or when `last_hit_by` is us. Log `HIT` for every attributed drop with `via=` and `kill=`; calibrate on non-kill hits: c4311 close range PASS band 35..75 (baseline 49) | V measurement; I logic |

---

## 1. Files M3 creates or changes

```
D:\Mods\MW2inER\
  docs\M3_DESIGN.md                          this file
  vendor\hudhook-0.9.3\                      copy of the cargo-registry crate + patch 0001 (MIT, LICENSE kept)
  tools\patches\hudhook-0001-pre-imgui-hook.patch
  tools\scripts\vm_format.py                 writer + reader of the converted formats (shared by converter and Blender)
  tools\scripts\convert_viewmodel.py         dumps -> converted\viewmodel\ (+ --check oracle 1, --self-test)
  tools\scripts\blender_ref.py               Blender 5.2 headless reference renders (+ bind self-check)
  tools\scripts\check_m3.ps1                 runs check_m2.ps1, then every M3 oracle
  tools\scripts\analyze_log.py               (changed) HIT via/kill fields, measured c4311 band, VM lines
  mod\Cargo.toml                             (changed) members + [patch.crates-io] hudhook
  mod\mw2er-core\src\vm_assets.rs  vm_anim.rs  vm_pose.rs  vm_proj.rs  xform.rs  cam_gate.rs   (new)
  mod\mw2er-core\src\hit_logic.rs  config.rs  logfmt.rs                                       (changed)
  mod\mw2er-core\tests\xanim_cross.rs        real-data cross-decoder (ignored unless MW2ER_ROOT is set)
  mod\mw2er-render\                          new lib crate: renderer + shaders + build.rs
  mod\vmtest\                                new bin crate: offscreen harness
  mod\mw2er\src\viewmodel.rs                 (new) wiring: PRE_IMGUI callback, loader thread, fallback
  mod\mw2er\src\{hud.rs,task.rs,shared.rs,input_hook.rs,game\camera.rs,game\chars.rs}  (changed)
  mw2er.toml                                 (changed) [viewmodel], [hit] and [keys] additions
Generated (gitignored): converted\viewmodel\, work\m3\, work\check_m3\
```

Converted assets go only to `converted\viewmodel\`, which is already gitignored (`/converted/`). Test renders and reports go to `work\m3\` (`/work/`).

---

## 2. Converter pipeline (offline)

### 2.1 Inputs [V]

| Input | Path |
|---|---|
| Hands mesh + skin | `dumps\common_mp_assets\model_export\viewmodel_base_viewhands_lod0.glb` (73 nodes, 66 joints) |
| Gun mesh + skin | `dumps\common_mp_assets\model_export\viewmodel_ump45_lod0.glb` (48 nodes, 25 joints) |
| Materials | `dumps\common_mp_assets\materials\mc\<material>.json` (colorMap, stateBits, blend) |
| Textures | `dumps\common_mp_assets\images\*.dds` (DXT1 = BC1, DXT5 = BC3, full mip chains) |
| Anims | `dumps\common_mp_assets\xanim\viewmodel_ump45_*` (OAT compiled XAnim v17, 38 files) |
| Weapon file | `dumps\common_mp\weapons\ump45_mp` (anim names, hideTags, times, ADS fields) |

### 2.2 Tool: `tools\scripts\convert_viewmodel.py` [I]

Run with Python 3.14 using `-I`, and insert the script's own folder into `sys.path` (the M2 gotcha). It reuses `xanim_read.py` for the XAnim layout and a small GLB reader. The Python process never imports OAT or IW4L code.

Steps:

1. **Weapon file.** Parse with the M2 WEAPONFILE splitter. Read `gunModel`, `handModel`, `hideTags`, every `*Anim` name (case-insensitive lookup, because the weapon file uses `viewmodel_ump45_ADS_up` and the dump file is lower case), and the times listed in section 6.
2. **Skeleton (91 bones).**
   - Order: the hands skin joints (0..65), then the gun skin joints (66..90). Every joint appears before its children (assert).
   - Parent of `j_gun` = `tag_weapon`, local = identity.
   - Root locals: `tag_origin` and `j_gun` local = identity, which drops OAT's −90° X quat `[-0.7071,0,0,0.7071]`. Assert that the quat is present before dropping it.
   - Every other local = the GLB node TRS (already in MW2 space).
   - Bind world by forward kinematics (FK) in MW2 space. `IBM_iw[b] = inverse(bindWorld_iw[b])`.
   - Check against the GLB inverse bind matrices: `max |IBM_iw[b] · R_oat − IBM_glb[b]| < 1e-3`, where `R_oat` maps MW2 space to GLB space: `(X, Y, Z) → (X, Z, −Y)`. Hands measured 3.7e-4, gun 5.5e-6 **[V mw2-rig]**. This proves the axis map.
3. **Mesh.**
   - Every vertex position and normal is converted to MW2 model space once: `v_iw = R_oat⁻¹ · v_glb`. Skinning then uses `World_iw_posed[b] · IBM_iw[b]`.
   - Drawn surfaces:
     - Gun: the primitives whose vertices are not weighted only to bones under a `hideTags` tag. That yields surfaces 0, 1 and 2, with 2,972 triangles **[V mw2-rig]**. The rule is computed, not hard-coded, and the count is asserted.
     - Hands: all 6 surfaces, 5,442 triangles.
   - Compact each GLB's shared vertex buffer to the used vertices. Then merge the gun and hands into one VB and one IB (u16; assert fewer than 65,536 vertices). Gun joint indices are offset by +66.
   - Weights are quantized to unorm8 with the sum forced to 255 (the largest weight absorbs the rounding). JOINTS_0 is mapped through the skin's joint list to the combined index.
   - The vertex colour (gun COLOR_0, greyscale) is stored but not used by default.
4. **Materials.**
   - Each material is resolved via `materials\mc\<name>.json` `colorMap`.
   - Blend mode: opaque, except `mtl_us_army_alpha`, which is blended (srcalpha/invsrcalpha, alpha test > 0, no depth write, drawn last) **[V material JSON]**.
   - Gun colour alpha is NOT opacity: those materials are opaque and alpha is ignored.
   - A material whose colorMap is missing is an error, unless the material is used only by hidden surfaces. That covers `$identitynormalmap`, which only the thermal attachment uses.
5. **Textures.**
   - For colour maps only in M3, copy the DDS mip chain into `.vmtex`. The DX9 header with FourCC DXT1/DXT5 maps to `BC1_UNORM`/`BC3_UNORM`.
   - Check: width and height are powers of two, the mip count matches the header, and the data size matches the sum of the BC block sizes.
6. **Anims.**
   - Convert the 12 base clips: idle, fire, fire_ads, reload, reload_empty, pullout, putaway, sprint_in, sprint_loop, sprint_out, ads_up, ads_down.
   - For each clip keep the exact keys: frame indices plus decoded values, rotations as f32 xyzw already sign-continuous, translations as f32.
   - Drop tracks whose bone is not in the skeleton. They must match the whitelist `j_sleeveanim_*` and `j_{index,mid,ring,pinky,thumb}_{le,ri}_3`; any other unknown bone is an error.
   - Notetracks are kept as (name, frame).
   - Parse all 38 files (oracle 1) even though only 12 are converted.
7. Write the outputs (section 2.3), then `manifest.json` last. It holds `format_version`, the converter version, and sha256 and size for every output and every input. Writing the manifest last means a half-written conversion has no manifest and the DLL refuses it.
8. Write `work\m3\convert_report.json`. It holds every number the oracles check: bone count, triangle counts, durations, whitelist hits, IBM error, bind-pose positions, ADS sight alignment, and so on.

### 2.3 Runtime formats (`converted\viewmodel\`) [I]

All files are little endian. Every file starts with a 32-byte header:

| Off | Type | Field |
|---|---|---|
| 0 | `[u8;4]` | magic `VMSK` / `VMME` / `VMAN` / `VMTX` |
| 4 | u32 | format version = **1** (`mw2er_core::vm_assets::FORMAT_VERSION`) |
| 8 | u32 | header size (32) |
| 12 | u32 | flags (0) |
| 16 | u64 | payload size (must equal the file size minus 32) |
| 24 | u32 | count (bones / vertices / clips / mips) |
| 28 | u32 | reserved 0 |

The files are:

- **`ump45.vmskel`**: for each bone: `u8 name_len, name utf8, i16 parent (-1 root), f32x3 bind_t, f32x4 bind_q (xyzw), f32[12] ibm (3x4 row-major, MW2 space)`. A trailing table gives the u16 indices of `tag_view, tag_ads, tag_weapon, j_gun, tag_flash, tag_sight_on, j_wrist_ri, j_wrist_le`.
- **`ump45.vmmesh`**:
  - Vertex stride 36: `f32x3 pos, i16x4 normal (snorm, w = 0), f32x2 uv, u8x4 joints, u8x4 weights, u8x4 color`.
  - Then the u32 index count and u16 indices.
  - Then `u16 surface count` and, per surface: `u32 first_index, u32 index_count, u16 material, u8 part (0 gun, 1 hands)`.
  - Then `u16 material count` and, per material: `u8 name_len, name, u16 texture, u8 blend (0 opaque, 1 alpha)`.
  - Then the texture file names.
- **`ump45.vmanim`**: for each clip: `name, u16 fps, u16 numframes, u8 looped, notetracks (u8 n; name, u16 frame), u16 track count`. Each track has `u16 bone, u16 nrot, u16 rot_frames[nrot], f32x4 rot[nrot], u16 ntrans, u16 trans_frames[ntrans], f32x3 trans[ntrans]`. Looped clips store numframes+1 keys, the last equal to the first (verified for sprint_loop).
- **`<texture>.vmtex`**: `u32 dxgi_format (71 BC1_UNORM / 77 BC3_UNORM), u32 w, u32 h, u32 mips`, then per mip `u32 row_pitch_bytes, u32 rows, u32 size` followed by the data.
- **`manifest.json`**: the names, hashes and format version above.

Parsers:

- The Rust parsers live in `mw2er_core::vm_assets` and are safe code. Every length is bounds-checked, so a truncated or corrupt file returns `Err`; it never panics or over-allocates (counts are capped: bones ≤ 256, vertices ≤ 65,535, clips ≤ 64, textures ≤ 16 MB).
- The DLL checks `FORMAT_VERSION` and the manifest hashes before upload. Any mismatch → `VM FAIL reason=assets: ...`.
- `vm_format.py` is the Python reader/writer of the same layout; Blender and the oracles use it.

---

## 3. Animation and pose logic (`mw2er-core`, pure, tested)

### 3.1 Sampling semantics [V on data unless marked]

- Keys sit at integer frames. Sampling at `f`:
  - binary-search the key span;
  - lerp translations;
  - nlerp rotations, with keys already sign-continuous from the converter;
  - before the first key or after the last, clamp to the nearest key.
- Non-looped clip: `f = clamp(t·fps·rate, 0, numframes)`.
- Looped clip: `f = (t·fps·rate) mod numframes`. Keys run 0..numframes inclusive.
- Local pose of each bone:
  - `T = track.trans if present else bind_t`
  - `R = track.rot if present else (bind_q if the bone has no track in this clip at all else identity)`

  Bones absent from a clip keep the full bind. A bone that has a translation track but no quat track gets identity rotation. This matches the data: no-quat never conflicts with a non-identity bind in these clips **[V mw2-rig]**. Unit test: idle `tag_weapon` gives (15.53, −2.62, −2.75).
- `tag_ads` is driven only by ads_up/ads_down, and those clips drive nothing else **[V]**. ADS is therefore a layer that overrides only `tag_ads`:
  - entering: `ads_up` sampled at `frac · duration`;
  - leaving: `ads_down` sampled at `(1 − frac) · duration`.

  This is IW4L-reported **[R]**. At `frac = 0` the layer is skipped and `tag_ads` = bind.
- Crossfade between base clips: per-bone lerp/nlerp of the local pose over `viewmodel.crossfade_s` (0.05 s) **[I]**. Fire restarts without a crossfade, as in MW2 **[I]**.
- FK runs in skeleton order. Skinning palette: `P[b] = World_iw[b] · IBM_iw[b]` (3x4).

### 3.2 State machine `vm_anim::VmAnim` [I]

**Inputs per task tick** (`VmInputs`, plain data):
- `now`, `mode_on`, `in_world`, `menu_open`
- `ads_held` (RMB, section 8)
- `planar_speed_mps`
- the `SimEvent`s of this tick: `Shot`, `ReloadStart{empty}`, `ReloadDone`, `ReloadCancelled`

**Output:** `VmState` (Copy, under 128 bytes, published in `HudSnapshot`):
- `visible`
- `base: ClipId`, `base_t0`, `base_rate`
- `prev: Option<(ClipId, t0, rate)>` and `fade_t0`, for the crossfade
- `ads_frac`, `ads_frac_t`, `ads_dir`
- `seq` (bumped on every change)

The present thread evaluates the pose at its own `now_s()`. Animation is therefore smooth at the present rate and independent of the tick.

**States and transitions:**

| State | Clip (rate) | Enter on | Leave to |
|---|---|---|---|
| Hidden | none | mode off and putaway done; not in world | Raise on `mode_on` rising |
| Raise | pullout (rate = 0.833/raiseTime 0.76) | mode on | Idle at end |
| Idle | idle (looped static pose) | default | any |
| Fire | fire, or fire_ads if `ads_frac ≥ 0.5` (native rate, restarted on every Shot) | `Shot` | Idle at clip end |
| Reload | reload (native, 2.500 s = reloadTime) | `ReloadStart{empty:false}` | Idle on `ReloadDone`/`ReloadCancelled` or clip end |
| ReloadEmpty | reload_empty (native, 3.033 s ≈ 3.03) | `ReloadStart{empty:true}` | as Reload |
| SprintIn → SprintLoop → SprintOut | rates 0.333/0.3, 1.0/0.7, 0.333/0.3 | speed > `viewmodel.sprint_speed_mps` for 0.2 s, not reloading, not ADS, no shot in the last 0.3 s | SprintOut when speed drops, or on Shot/ADS/Reload (Shot cuts straight to Fire) |
| Drop | putaway (rate 0.767/dropTime 0.8) | `mode_on` falling | Hidden at end |

Further rules:
- Reload has priority over Fire. `weapon_sim` already blocks shots while reloading.
- A menu opening or leaving the world sets `visible = false`, and the state continues underneath.
- ADS fraction: moves toward `ads_held && !reloading && !sprinting` at a rate of `1/adsTransInTime` (0.2 s) in and `1/adsTransOutTime` out.
- The rates come from the weapon file, as `native_duration / weapon_time` **[I: IW4L reports the engine scales to state times]**. reload and reload_empty play at native rate (they match within 0.1% **[V]**).

**Unit tests (category `vm_anim`, required by section 12.4):**
- the full transition table;
- a fire restart every 0.09 s keeps state Fire;
- a reload during sprint goes to Reload;
- putaway keeps `visible` until its end;
- ADS frac reaches 1 at 0.2 s and is clamped;
- the evaluation is deterministic for a given `(VmState, now)`.

### 3.3 Projection and view `vm_proj` [I, IW4L-reported]

- MW2 `cg_fov` is a 4:3 Hor+ value: `tan_half_y = tan(fov/2) · 0.75`, independent of aspect; `tan_half_x = tan_half_y · W/H`.
  - Hip: fov 65 → vertical 51.077° (0.8915 rad).
  - ADS: adsZoomFov 55 → 42.654°.
- The viewmodel fov blends linearly in cg_fov degrees by `ads_frac` (M3 simplification; adsZoomInFrac/OutFrac are M4).
- View: `V = M_axis · inverse(World_iw[tag_view]) · T(viewmodel.offset_in)`, with `M_axis` mapping `(X,Y,Z)_iw → (−Y, Z, X)_d3d` (det −1, a reflection).
- Projection: reversed Z, left-handed, `near = viewmodel.near_in` (0.1 in, MW2 `r_znear_depthhack` **[R]**), `far = viewmodel.far_in` (1000 in). Depth 1 at near, 0 at far.
- Units stay inches end to end.
- Tests:
  - the vfov numbers;
  - a point on the view axis projects to the NDC centre;
  - the near point → depth 1 and the far point → 0;
  - at idle, `tag_flash` projects to the lower right (x > 0, y < 0);
  - at the ADS end, the rear-sight point is within 0.01 NDC of the centre.

### 3.4 Winding [I, settled by vmtest]

The axis map is a reflection, and glTF is CCW-front in a right-handed space. So after the map, front faces appear clockwise on screen, and D3D's default `FrontCounterClockwise = FALSE` with `CULL_BACK` is the expected setting. vmtest proves this (section 12.2, case W). If it fails, the implementer must not ship CULL_NONE silently; they record the result and choose from the test.

---

## 4. Renderer architecture

### 4.1 Crates [I]

`mod\Cargo.toml` members: `["mw2er-core", "mw2er-render", "mw2er", "vmtest"]`.

- **`mw2er-render`** (lib):
  - Depends on `mw2er-core` and `windows =0.62.2`, with the workspace features plus `Win32_Graphics_Direct3D`, `Win32_Graphics_Direct3D12`, `Win32_Graphics_Dxgi`, `Win32_Graphics_Dxgi_Common`, `Win32_Graphics_Direct3D_Fxc` (fallback only) and `Win32_Security` (CreateEvent).
  - **No hudhook dependency**, so the harness does not pull MinHook.
  - `#![deny(unsafe_op_in_unsafe_fn)]`, and every `unsafe` block has a `// SAFETY:` comment.
- **`vmtest`** (bin `vmtest.exe`): depends on `mw2er-render`, `mw2er-core`, and `png` (exact pin `=0.17.x` resolved once by cargo and locked).
- **`mw2er`** gains a dependency on `mw2er-render`. The COM types from the hook context are windows 0.62.2 types, the same as hudhook's, so no conversion is needed **[V M2_DESIGN 3.1]**. Types from windows 0.61 (inside `eldenring`) never cross into the renderer.

API (identical for the DLL and vmtest):

```rust
pub struct Target<'a> { pub resource: &'a ID3D12Resource, pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
                        pub format: DXGI_FORMAT, pub width: u32, pub height: u32 }
pub struct FrameParams<'a> { pub palette: &'a [[f32; 12]], pub view_proj: [[f32; 4]; 4],
                             pub light: LightParams, pub out: OutputMode, pub debug: DebugMode }
impl Renderer {
    pub fn new(device: &ID3D12Device, rtv_format: DXGI_FORMAT) -> Result<Self, VmError>;  // PSOs, heaps, ring
    pub fn upload(&mut self, queue: &ID3D12CommandQueue, assets: &VmAssets) -> Result<(), VmError>; // blocking; loader thread only
    pub fn is_ready(&self) -> bool;
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, t: &Target, slot: usize, p: &FrameParams) -> Result<(), VmError>;
    pub fn timing(&mut self) -> Option<f32>; // gpu ms of the frame recorded 3 slots ago
}
```

`upload` uses its own direct queue and fence on the given device and blocks on that fence, so it runs only on the loader thread or in vmtest. `record` never waits on anything.

### 4.2 Hook point: vendored hudhook + patch 0001 [V insertion point; I behaviour]

**Vendoring.**
1. Copy `%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\hudhook-0.9.3\` to `vendor\hudhook-0.9.3\` (with `build.rs`, `vendor\` MinHook sources and `LICENSE`).
2. Add to `mod\Cargo.toml`:
   ```toml
   [patch.crates-io]
   hudhook = { path = "../vendor/hudhook-0.9.3" }
   ```
   The dependency spec stays `=0.9.3`.
3. Canonical patch: `tools\patches\hudhook-0001-pre-imgui-hook.patch`, made with `git diff --no-index` against a pristine copy. check_m3 check 2 verifies that the vendored tree equals the registry copy plus exactly this patch.

**The patch touches exactly two files.**

`src/renderer/backend/dx12.rs`:
- A new public API at the crate root, re-exported as `hudhook::pre_imgui`:
  ```rust
  pub struct PreImguiCtx<'a> {
      pub device: &'a ID3D12Device, pub command_list: &'a ID3D12GraphicsCommandList,
      pub command_queue: &'a ID3D12CommandQueue, pub render_target: &'a ID3D12Resource,
      pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE, pub rtv_format: DXGI_FORMAT,
      pub width: u32, pub height: u32, pub frame_index: usize, pub num_frames: usize,
  }
  pub static PRE_IMGUI: OnceLock<Box<dyn Fn(&PreImguiCtx) + Send + Sync>> = OnceLock::new();
  ```
- In `render()`, between line 218 (`OMSetRenderTargets`) and line 219 (`SetDescriptorHeaps`): if `PRE_IMGUI` is set, take width/height from `render_target.GetDesc()`, call the callback inside `catch_unwind(AssertUnwindSafe(..))` (a panic is swallowed and counted in an atomic `PRE_IMGUI_PANICS`), then re-issue `OMSetRenderTargets(1, Some(&self.rtv_heap_start), false, None)`.
- The existing `SetDescriptorHeaps` and `setup_render_state` (viewport, root signature, PSO, VB/IB, topology, blend factor; scissor per ImGui command) then restore ImGui's state.

`src/hooks/dx12.rs`:
- MinHook `IDXGISwapChain3::SetColorSpace1`. Take the address from the dummy swap chain that `get_target_addrs` already creates (vtable index 38), and install it next to Present/Present1.
- The detour stores the colour space in a `pub static LAST_COLOR_SPACE: AtomicI32` (initial −1 = never called) and calls the trampoline.
- Re-export as `hudhook::pre_imgui::LAST_COLOR_SPACE`.

**Properties our callback relies on [V source]:**
- The back buffer is in RENDER_TARGET when the callback runs; hudhook moves it to COMMON afterwards.
- The list was just Reset, so no scissor, viewport, heap or root signature is set. The callback sets all of these itself.
- The fence of `frame_index` was waited on just before, so per-frame upload memory indexed by `frame_index` is free.
- The callback runs even when ImGui draws nothing (the hook sits in `render()`, not in `render_draw_data`), but not on frames where `Pipeline::render` skips because `display_size` is 0.

**What our callback records (no resource barriers at all):**
1. `RSSetViewports` and `RSSetScissorRects` over the full target.
2. `OMSetRenderTargets(1, ctx.rtv, false, Some(&our_dsv))` and `ClearDepthStencilView(our_dsv, DEPTH, 0.0)`.
3. `SetDescriptorHeaps([our CBV_SRV heap])`, root signature, PSO opaque, topology TRIANGLELIST, VB, IB.
4. Root CBV b0 (frame constants) and b1 (bone palette) from the upload ring at `frame_index`.
5. One `DrawIndexedInstanced` per opaque surface (descriptor table t0 = its texture). Then PSO blend and the alpha surface.
6. If timing is on: `EndQuery` timestamps before step 1 and after step 5, then `ResolveQueryData` into the readback ring slot.

Our DSV never leaves `DEPTH_WRITE` and textures stay in `PIXEL_SHADER_RESOURCE`. A half-recorded callback therefore leaves no state behind that hudhook's restore does not cover.

### 4.3 Pipeline objects [I]

**Root signature** (v1.0, serialized with `D3D12SerializeRootSignature`, flag `ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT`):

| Param | Binding | Visibility | Contents |
|---|---|---|---|
| 0 | root CBV b0 | ALL | `FrameConsts` (256 B): `view_proj` float4x4, `light_dir_view` float3, `diffuse`, `ambient_sky` float3, `ambient_ground` float3, `exposure`, `out_mode` uint, `paper_white_nits`, `debug_mode` uint |
| 1 | root CBV b1 | VERTEX | bone palette, `float3x4 bones[128]` (91 used, 6,144 B reserved) |
| 2 | table SRV t0 | PIXEL | diffuse texture |
| 3 | root constants b2 (4 dwords) | PIXEL | `part_id`, `alpha_mode`, 2 spare |

One static sampler `s0`: anisotropic 8, WRAP.

**Input layout:**
- `POSITION R32G32B32_FLOAT`
- `NORMAL R16G16B16A16_SNORM`
- `TEXCOORD R32G32_FLOAT`
- `BLENDINDICES R8G8B8A8_UINT`
- `BLENDWEIGHT R8G8B8A8_UNORM`
- `COLOR R8G8B8A8_UNORM`

**PSO opaque:**
- `RTVFormats[0] = ctx.rtv_format`
- DSV `D32_FLOAT`, depth enable, write ALL, func GREATER (reversed Z)
- no blend
- cull BACK, FrontCounterClockwise FALSE (section 3.4)
- SampleDesc 1

**PSO alpha:** the same but with blend SRC_ALPHA/INV_SRC_ALPHA and depth write ZERO.

The PSOs are created per `rtv_format`. A format change recreates them on the next frame (old objects are retired after 3 frames).

**Shaders** (`mw2er-render/shaders/viewmodel.hlsl`, entry points `vs_main`, `ps_main`, targets `vs_5_1`/`ps_5_1`):
- VS:
  - `skin = Σ w_i · bones[j_i]` (3x4)
  - `p_iw = mul(skin, float4(pos,1))`, `n_iw = normalize(mul((float3x3)skin, n))`
  - `SV_Position = mul(view_proj, float4(p_iw,1))`
  - outputs the view-space normal (view_proj is split into view and proj in `FrameConsts`; the normal uses the view's 3x3) and the uv
- PS:
  - `albedo = t0.Sample(s0, uv).rgb`. The SRV is `BC1_UNORM_SRGB`/`BC3_UNORM_SRGB`, so the sample is linear.
  - `alpha_mode == 1` → `clip(a - 1/255)`, output a.
  - `L = max(dot(n, -light_dir),0)·diffuse + lerp(ambient_ground, ambient_sky, n.y·0.5+0.5)`
  - `c = albedo · L · exposure`, then the output transform `out_mode`:
    - 0 = sRGB OETF
    - 1 = PQ (BT.709→BT.2020 matrix, `c·paper_white/10000`, ST2084)
    - 2 = scRGB (`c·paper_white/80`)
  - `debug_mode` 1 = flat part colour (gun red, hands green) and 2 = white silhouette, both used only by vmtest.
- Defaults (`[viewmodel]`, hot-reloadable): `light_dir_view = normalize(0.35, -0.80, 0.50)` (from upper left, slightly behind the eye), diffuse 0.85, ambient_sky (0.42,0.42,0.45), ambient_ground (0.20,0.19,0.18), exposure 1.0.
- Normal, spec and detail maps are not used in M3. The normal maps use a two-channel encoding (R == B, not unit length) whose X channel is unresolved **[V]**; adding them needs a harness A/B first.

**Build:**
- `mw2er-render/build.rs` runs `fxc /nologo /T vs_5_1 /E vs_main /O3 /WX` (and the same for `ps_5_1`) into `OUT_DIR`.
- fxc is found via `MW2ER_FXC`, else the 10.0.26100.0 path above. If fxc is missing or a shader has an error, the build fails; it never falls back silently.
- `pub const VS: &[u8] = include_bytes!(..)`, the same for `PS`, and `pub const SHADER_SHA256: &str` (computed in build.rs).
- Runtime `D3DCompile` (d3dcompiler_47, already used in ER's process by hudhook **[V]**) exists only behind `viewmodel.runtime_shader_compile = false` for emergencies.

**Resources:**
- **DSV:** a DEFAULT-heap `D32_FLOAT` of the back-buffer size, `ALLOW_DEPTH_STENCIL | DENY_SHADER_RESOURCE`, initial `DEPTH_WRITE`, optimized clear 0.0, in a 1-entry DSV heap. It is recreated when the target size changes; the old one is retired after `num_frames` frames. 4K costs 33 MB.
- **VB, IB, textures:** DEFAULT heap, uploaded once by `upload()` through an UPLOAD staging buffer (`GetCopyableFootprints` + `CopyTextureRegion`), then barriers to `VERTEX_AND_CONSTANT_BUFFER`, `INDEX_BUFFER` and `PIXEL_SHADER_RESOURCE`.
- **SRV heap:** shader-visible `CBV_SRV_UAV`, 16 entries.
- **Upload ring:** one persistently mapped UPLOAD buffer, `num_frames` slots × (256 B frame + 6,144 B palette, each 256-aligned).
- **Timing:** `TIMESTAMP` query heap with 2 × num_frames entries, plus a READBACK buffer.

### 4.4 Colour output selection [I: ER's swapchain format and colour space are unknown]

At the first callback the DLL logs once: `VM target fmt=<DXGI name> WxH colorspace=<LAST_COLOR_SPACE or none>`. Then `viewmodel.output`:
- `"auto"` (default):
  - `R16G16B16A16_FLOAT` → scRGB;
  - `R10G10B10A2_UNORM` with colour space `G2084_NONE_P2020` (12) → PQ;
  - `R8G8B8A8_UNORM`/`B8G8R8A8_UNORM`/`R10G10B10A2_UNORM` with colour space none or `G22_NONE_P709` (0) → sRGB;
  - any other combination → `VM FAIL reason=unknown output fmt/colorspace` (viewmodel off, M2 continues).
- `"srgb"`, `"pq"` or `"scrgb"` force the mode.

`paper_white_nits` defaults to 200. **The first in-game test is done with HDR off.**

### 4.5 Per-frame data flow [I]

| Thread | Work |
|---|---|
| main (CSTask, `ChrIns_PostPhysics`) | `VmAnim::step(VmInputs)` after `weapon_sim.step` (M2 §8 step 9) and after the input atomics are read; publish `VmState` + `ads_frac` in `HudSnapshot` (step 14) |
| present (hudhook) | `Mw2Hud::render` copies the snapshot (already done in M2). The PRE_IMGUI callback (`viewmodel.rs`) runs after it within the same Present |
| present, in the callback | `guard("vm")`; return early unless `vm_should_draw(...)` (core: renderer ready && !VM_DISABLED && cfg.viewmodel.enabled && snapshot fresh < 0.5 s && in_world && !menu_open && vm_state.visible). Then `vm_pose::evaluate(&assets, &state, now_s()) → palette[91]`; `vm_proj` builds view_proj from the target W/H and `ads_frac`; `renderer.record(ctx.command_list, …, ctx.frame_index, …)` |
| `mw2er-vmload` (spawned on the first callback) | reads and validates `converted\viewmodel\`, creates its own direct queue and fence on `ctx.device`, calls `upload()`, then publishes `ready`. Logs `VM assets ready ms=… bytes=…` or `VM FAIL reason=…` |

- The renderer lives in `Mutex<Option<Renderer>>`, which only the present thread locks (the loader hands over the uploaded resources via a one-shot channel). The callback uses `try_lock` and skips the frame on contention.
- The pose CPU cost is about 91 bones × 2 clips, a few µs.
- The DLL never waits on a fence on the present thread.
- `VM gpu_ms p50=… p95=… n=…` is logged every `viewmodel.log_interval_s` (10 s) from the timestamp readback, 3 frames late.

### 4.6 Failure fallbacks (non-negotiable) [I]

| Failure | Log | Behaviour |
|---|---|---|
| `converted\viewmodel\manifest.json` missing / version or hash mismatch / parse error | `VM FAIL reason=assets: …` | viewmodel off; M2 (HUD, firing, input) unchanged |
| `Renderer::new` / `upload` HRESULT error | `VM FAIL reason=d3d: <call> 0x…` | as above |
| unknown output format/colour space (auto) | `VM FAIL reason=output …` | as above |
| panic in the callback | `PANIC …` + `ERROR vm disabled` | `VM_DISABLED` set; later frames return at once |
| `PRE_IMGUI` never called (hudhook not up) | `VM WARN no frames 30 s after HUD initialize` | nothing drawn |
| `record` error | `VM FAIL reason=record …` | disabled for the session |
| `viewmodel.enabled = false` | `VM off (config)` | no loader thread, nothing drawn |

The callback is registered at boot step "hudhook apply" (before `apply()`), so the hook exists only if the version gate passed.

---

## 5. Depth policy (decision and justification) [I]

**Decision:** separate viewmodel depth range. Use our own D32 buffer, reversed Z, cleared to 0 each frame, with the viewmodel's own projection. The viewmodel is drawn over ER's finished back buffer and **never tests against ER's scene depth**. M3 adds **no** `ClearDepthStencilView` / `CreateDepthStencilView` hooks.

Why:

1. **CoD semantics.** MW2 draws the viewmodel in its own depth band (viewport depth [0, 1/64]) with near 0.1 in, so the world never occludes it and the 27-inch gun never clips into walls **[R IW4L depth_range.rs]**. The CS2 note's DSV capture was for third-person world-space models, a different case.
2. **ER's camera is third person.** It sits about 3.5 m behind the character (log line 341, cam_chest 3.51 m **[V]**), so ER's depth between the eye and a gun 0.1-0.7 m in front contains the character's own back and nearby walls. A test against it would hide the gun most of the time.
3. **Different projection.** The viewmodel uses its own FOV (51° vs ER's 48°), so ER depth values are not comparable without reprojection.
4. **Fewer hazards.** A captured copy is one frame stale (CS2 gotcha 9), it adds hooks on ER's recording threads, and it depends on `pers_cam_1`, which is a placeholder for the first ticks after a load. None of that is needed.

The cost of the decision: the gun also draws over ER's own HUD elements in its screen area (bottom right) and gets no ER TAA, DOF or motion blur. Both are acceptable, and CoD keeps the viewmodel sharp too. ER depth capture remains the plan for world-space effects (M4+); the recipe is in the renderer finding.

**Harness proof:** vmtest case D1 (section 12.2).

---

## 6. Weapon-file fields used [V values]

| Field | Value | Use |
|---|---|---|
| gunModel / handModel | viewmodel_ump45 / viewmodel_base_viewhands | converter inputs |
| hideTags | tag_acog_2, tag_eotech, tag_thermal_scope, tag_red_dot, tag_heartbeat, tag_silencer | surface filter |
| idleAnim, fireAnim, adsFireAnim, reloadAnim, reloadEmptyAnim, raiseAnim, dropAnim, sprintIn/Loop/OutAnim, adsUpAnim, adsDownAnim | viewmodel_ump45_* | clip names (case-insensitive) |
| reloadTime / reloadEmptyTime | 2.5 / 3.03 | oracle 1 (±10%) |
| raiseTime / dropTime | 0.76 / 0.8 | anim rates |
| sprintInTime / sprintLoopTime / sprintOutTime | 0.3 / 0.7 / 0.3 | anim rates |
| adsTransInTime / adsTransOutTime | 0.2 / 0.2 | ADS frac rate |
| adsZoomFov | 55 | ADS viewmodel fov |
| fireTime | 0.09 | (M2 cadence) fire clip restarts |

`convert_weapons.py` already produces `weapons.json`. The M3 converter reads the weapon file directly, and the numbers are copied into `converted\viewmodel\manifest.json` under `weapon_times`. That keeps M2's `weapons.json` schema unchanged and check_m2 passing.

---

## 7. Camera and FOV handling [mixed]

- **Viewmodel:** no ER camera input (section 3.3). Its aspect is the back-buffer W/H from the hook context. M3 has no sway or bob, so no camera deltas are needed.
- **What ER structure holds the render matrices:** not established **[V: absent from the vendored crate]**.
  - `CSCamera.pers_cam_1` (`CSCam`: camera-to-world matrix rows right/up/forward/position at 0x10, fov 0x50, aspect 0x54, near 0x58, far 0x5C) is the composed gameplay camera **[V source; offsets of matrix/fov proven by M2]**.
  - fov is in radians: 0.83775806 = 48.000° in play; 0.7505 = 43.0° is only the placeholder value right after world entry **[V log]**. Vertical vs horizontal remains **[I]**, with vertical assumed.
  - The GPU-side view/projection constant buffer was not located. It is not needed for M3; finding it (RenderDoc/Ghidra) is a M4+ item that only matters for world-space draws.
- **CAMFRAME log** (diagnostic, new in M3):
  - on world entry for the first 10 ticks, then every 5 s, and on every validity change:
  - `CAMFRAME tick=N src=pers_cam_1 valid=0|1 why=<identity|ortho|det|fov|dist|settling|ok> pos=(…) fwd=(…) det=+1.000 ortho_err=… fov=…(…deg) aspect=… near=… far=… mask=0x… cam_chest=…m bb=WxH`.
  - This records the never-read aspect/near/far/mask fields.


---

## 8. Input: ADS button [I]

- New config `keys.ads_mouse_button = 1` (RMB). The value 255 disables ADS.
- The DI detour gets `RAW_ADS_DOWN` and blanks that button under the same conditions and release latch as the fire button (`WANT_BLANK_ADS = WANT_BLANK_FIRE`), via `input_logic::mouse_button_offset`. RMB in ER is guard/off-hand, so it must not leak while MW2 mode is on.
- In M3, ADS changes only the viewmodel (pose + fov). Spread, zoom and sensitivity are M4.
- `VmInputs.ads_held = RAW_ADS_DOWN && blanking conditions`.
- Unit tests: the offset for button 1 in cb 16/20, and the latch for ADS.

---

## 9. Camera validity gate (fixes the 170 m episodes) [I logic; V root cause in log]

Root cause **[V log, I reading]**: `CamSignTracker` resolves on the first sample after `TASK world entered`. At that moment `pers_cam_1` is either the identity placeholder at the origin or the camera from before the load. Every valid sample agrees with sign +1.

New `mw2er_core::cam_gate::CamGate`:
- A sample is invalid if:
  - any value is non-finite;
  - the matrix is exactly identity with position (0,0,0);
  - `| |row_i| − 1 | > 1e-3` or `|row_i·row_j| > 1e-3`;
  - `det(rows0..2)` is not in 0.99..1.01 (sign to be confirmed by CAMFRAME; if the first valid in-game log shows −1, flip the expected sign in config `aim.camera_det_sign`);
  - fov is outside 0.2..3.0;
  - cam-chest distance is outside 0.3..15 m.
- The camera counts as valid only after 3 consecutive valid ticks.
- Reset on world enter/leave and on any jump of the player or the camera > 5 m in one tick (a Havok re-base).
- The forward sign is fixed at +1. `CamSignTracker` stays in the code and its tests stay (check_m2 check 8 counts them), but the task no longer uses its result for aiming. A disagreement is logged as `WARN camsign`.
- Shots while the camera is invalid are skipped with `SHOT … skipped: camera settling` (this extends M2's existing skip reasons).
- Tests: the identity placeholder is rejected; the stale-camera sequence from session 1 settles only after the re-base; 3-tick hysteresis; reset on a jump.

---

## 10. HudSnapshot and shared additions

- `HudSnapshot` gains `vm: VmState`, `ads_frac: f32` and `cam_valid: bool`.
- New atomics: `VM_DISABLED`, `VM_READY`, `RAW_ADS_DOWN`, `WANT_BLANK_ADS`.
- `mw2er.toml` gains these sections (unknown keys are still errors; every float is bounded and finite):

```toml
[viewmodel]
enabled              = true
assets_dir           = 'converted\viewmodel'   # not hot
output               = "auto"                  # auto | srgb | pq | scrgb
paper_white_nits     = 200.0
hip_fov              = 65.0                    # MW2 cg_fov (4:3 Hor+)
ads_fov              = 55.0                    # adsZoomFov
near_in              = 0.1
far_in               = 1000.0
offset_in            = [0.0, 0.0, 0.0]         # cg_gun_x/y/z (fwd, left, up), inches
light_dir_view       = [0.35, -0.80, 0.50]
diffuse              = 0.85
ambient_sky          = [0.42, 0.42, 0.45]
ambient_ground       = [0.20, 0.19, 0.18]
exposure             = 1.0
crossfade_s          = 0.05
sprint_speed_mps     = 5.0                     # 0 = no sprint anims; ER sprint speed is unverified, tune from STATE speed logs
gpu_timing           = true
log_interval_s       = 10.0
runtime_shader_compile = false

[hit]                    # additions
impact_window_s  = 0.25
impact_radius_m  = 1.0   # distance from the impact to the character's vertical axis segment
capsule_height_m = 2.2

[keys]                   # addition
ads_mouse_button = 1     # 255 = none
```

---

## 11. Hit attribution change (replaces M2 section 12.1) [V measurement; I logic]

`hit_logic` changes (pure, tested):

- `ChrSample` gains `pos: [f32;3]` (ChrIns physics position, which is the feet). `chars.rs` already reads the position for distance; it now copies it in.
- New input `ImpactSample { t: f64, pos: [f32;3], used: bool }`. The task keeps a ring of our `kind=hit` markers from the last 1 s (from `poll_bullets`, which runs before `poll_hp` within the tick **[V task order]**). They are cleared on world enter/leave and on a re-base jump.
- An HP drop of a tracked character in this tick is **attributed** if either:
  1. `last_hit_by == player` (the old rule, `via=last_hit_by`), or
  2. an unused impact has `now − t ≤ impact_window_s` and lies within `impact_radius_m` of the segment `pos .. pos + (0, capsule_height_m, 0)` (`via=impact`).

  Each impact is consumed by the nearest such character. `n` = the number of impacts consumed.

  **Review fix (2026-10-14, measured on M2 run 2):** the IMPACT of the bullet that caused a drop is recorded one task tick *after* the drop (+16..18 ms on every close-range hit), so rule 2 as written above never matched the first hit and matched later drops to the previous shot's impact. Drops are now kept pending for `hit.impact_lag_s` (0.1 s) and matched to impacts with `t_imp - t_drop` in `[-impact_window_s, +impact_lag_s]`, near the position recorded at the drop; an impact eligible for several pending drops goes to the one nearest in time. Regression test `hit_logic_replay_m2_run2` replays the real run-2 timestamps (4 non-kill c4311 drops and 11 non-kill c4351 drops, each attributed with n=1); it fails with `impact_lag_s = 0`.
- Attributed drops log `HIT chr=c#### hp a->b dmg=N band=… mw2=… d=…m via=impact|last_hit_by n=K kill=0|1`. The prefix `HIT chr=c#### hp a->b dmg=N` is unchanged (M2 non-negotiable 11). `kill=1` when `b ≤ 0`, together with the existing `KILL` line.
- Unattributed drops inside the shot window are still logged as `HPDROP …` (diagnostic).
- Hitmarker: on any attributed drop.
- Tests:
  - the drop with an impact at the chest within 0.25 s → HIT `via=impact`;
  - an impact 0.3 s old → HPDROP;
  - an impact 2 m away → HPDROP;
  - two chars and one impact → only the nearer one;
  - two impacts and one drop → `n=2`;
  - a kill → `kill=1`;
  - the 2-HP ticks with no impact → HPDROP;
  - Torrent excluded.

`analyze_log.py` "hits" check:

- **Calibration set:** HIT lines with `kill=0`, `n=1`, on `KNOWN_TARGETS`.
- **c4311 Godrick Soldier (219 HP)** measured baseline **49** per hit at k=4 (first hit 69) **[V run 2]** — in the LAST falloff band 79000105 (atkPhys 140) at about 36 m, not at close range (review fix: the kill line reads `band=79000105 mw2=35 d=36.0m`). PASS band **35..75** applies to that band. Hits in other bands are compared with the band scaled by the ER-formula ratio `pred(atk)/pred(140)` (inference) and give at most NEEDS-CALIBRATION until a close-range c4311 measurement exists.
- **PASS** needs ≥ 3 calibration hits on c4311, a median within the band, and ≥ 75% of the hits within the band.
- The ER-formula prediction (103) is printed as info only and no longer gates.
- **Consistency rules from M2 stay:** dmg > 0, band/mw2 agree with rows.json, a SHOT within `hit.window_s`, and the plausibility bounds. A `kill=1` line is exempt from plausibility (its damage is truncated).
- **Self-test cases (new):**

| Case | Expected |
|---|---|
| 49, 49, 49, 69 non-kill on c4311 | PASS |
| a single hit of 103 | FAIL (outside band) |
| only `kill=1` hits | NEEDS-CALIBRATION |
| `n=2` 98 | excluded from calibration |
| an old-format HIT without `via` | still parsed (M2 logs) |
| 30, 30, 30 | FAIL |
| hits only on c4351 | NEEDS-CALIBRATION |

- **New in-game rows:** `viewmodel` (`VM assets ready`, `VM target fmt=`, no `VM FAIL`, `VM gpu_ms p95 < 1.0`), `vm anim` (`VM state` transitions appear after `RELOAD start` and `MODE on`/`off`), `camera` (no shot skipped for `cam_player_dist` after the first 3 valid ticks; CAMFRAME `det` and `aspect` sane).

---

## 12. Offline oracles (all must pass before the user runs M3)

### 12.1 Oracle 1: anim converter (`convert_viewmodel.py --check`, plus Rust tests) [criteria fixed here]

| # | Check | Pass criterion |
|---|---|---|
| 1a | parse all | all 38 `viewmodel_ump45_*` parse to exact EOF in Python AND in the Rust reader (`mw2er-core/tests/xanim_cross.rs`, an independent implementation written from section 2 of this document, not from OAT or IW4L code) |
| 1b | sanity | fps == 30 for all; numframes ≥ 1 for the 12 base clips (akimbo idle = 0 is allowed and reported); duration 0.03..5 s; \|q\|−1 ≤ 1e-4 for every decoded key |
| 1c | reload durations | reload duration within 10% of reloadTime 2.5 (expected 2.500) and reload_empty within 10% of 3.03 (expected 3.033). Other clips vs weapon times are reported, not failed (expected: they differ, so the rates scale) |
| 1d | bone names | every track bone in the combined 91-bone skeleton or in the whitelist (`j_sleeveanim_*`, `j_*_3`); whitelist hits == 40 (the reported number; any other count is reported and must be justified); any other name → FAIL |
| 1e | cross-decoder | Rust reader vs `.vmanim` on all 12 converted clips (requirement ≥ 3; reload, reload_empty and fire are mandatory): identical key frames; per key \|Δq\| ≤ 2e-5 (after canonical sign) and \|Δt\| ≤ 1e-4 in. Both readers also agree on notetracks |
| 1f | idle ≈ bind | over bones with an idle translation track: median \|t_idle − t_bind\| ≤ 0.01 in and ≥ 50 bones within 0.01 in. Known movers (j_shoulder_*, tag_weapon*, tag_player) are listed, not failed. A max-difference check is forbidden (it fails by design) |
| 1g | semantics | FK at idle: `j_gun` in view space (relative to tag_view) = (10.90, −2.62, −4.13) ± 0.05 in. At the ADS end (ads_up last frame over idle): `tag_ads` local = (−4.238, 2.618, 1.025) ± 0.01, and the highest rear-sight vertex (gun surface 2, view x in [5,7]) lies within 0.1 in vertically and 0.25 in laterally of the view axis. Delta semantics would put the gun about 30 in further forward, so this check fails under the wrong reading |
| 1h | loops and ADS | sprint_loop first key == last key (≤ 1e-6); ads_down frame 0 `tag_ads` == ads_up last frame within 0.01 in |
| 1i | skeleton | 91 bones; parent-before-child; IBM consistency vs GLB < 1e-3; bind FK `tag_weapon`, `tag_weapon_left`, `tag_weapon_right` coincide within 0.01 in (reported (10.528, 0, 51.87)) |
| 1j | mesh | gun 2,972 + hands 5,442 triangles; joint indices < 91; weight sums == 255; every index < vertex count; no NaN |
| 1k | textures | every colour map present; formats BC1/BC3 as listed by the mw2-rig finding; complete mip chains |
| 1l | self-test | `--self-test` catches these mutations: a renamed bone (1d fails), translation read as delta (1g fails), a truncated xanim (1a fails), a missing texture (1k fails), a wrong weight sum (1j fails), and a manifest hash mismatch (Rust `vm_assets` rejects it) |

### 12.2 Oracle 2: renderer harness (`vmtest.exe`) [criteria fixed here]

`mod\vmtest` uses **the same `mw2er_render::Renderer`, shaders and `mw2er_core::vm_pose/vm_proj`** as the DLL.

Setup:
- `D3D12GetDebugInterface → EnableDebugLayer`; `--gbv` adds `SetEnableGPUBasedValidation`.
- Hardware adapter (`--warp` for debugging only; perf is never measured on WARP).
- Its own direct queue.
- An `ID3D12InfoQueue` drained after every case. Any ERROR or CORRUPTION message → FAIL, with the message printed.

The "hook" is simulated exactly:
- a fresh command list;
- the target is created in RENDER_TARGET state and cleared to the key colour (255,0,255);
- an RTV in a separate heap;
- `record(list, target, slot, params)`;
- execute, fence-wait;
- copy the texture into a READBACK buffer (footprint copy), map, write `work\m3\<case>.png` and the raw mask `work\m3\<case>.mask` (`VMMK`, u32 w, u32 h, u8 per pixel, 1 = not key colour).

**Poses** (shared list in `mw2er_core::vm_pose::TEST_POSES`; vmtest writes it to `work\m3\poses.json` for Blender):

| Name | Clip | t | ADS frac |
|---|---|---|---|
| idle | idle | 0 | 0 |
| fire | fire | 0.05 | 0 |
| reload_a | reload | 0.5 | 0 |
| reload_b | reload | 1.2 | 0 |
| reload_c | reload | 2.0 | 0 |
| reload_empty_bolt | reload_empty | 2.3 | 0 |
| ads | idle + ads_up | end | 1 |
| pullout | pullout | 0.1 | 0 |
| sprint | sprint_loop | 0.5 | 0 |

The camera is the viewmodel eye (exactly as in game). Resolution 1920x1080 for comparisons, plus `idle` at 3840x2160.

**Blender reference** (`blender.exe -b --factory-startup --python tools\scripts\blender_ref.py -- --poses work\m3\poses.json`):
- **Import the original GLBs**, not our converted mesh. That keeps the skinning, mesh conversion and FK independent of ours.
- Delete gun faces whose vertices are all weighted only to bones under hideTags. This reimplements the rule; the triangle count must equal 2,972.
- Pose: for each bone, take the target world `W_t` from Python FK over the `.vmanim`/`.vmskel` (via `vm_format.py`; this is a separate FK from the Rust one). Blender world after glTF import == MW2 space, because importer (x,−z,y) ∘ OAT (X,Z,−Y) = identity **[I, asserted by the self-check below]**. Set `pose_bone.matrix = Arm⁻¹ · W_t · J_rest⁻¹ · Arm · bone.matrix_local`, where `J_rest` is the joint's rest world computed from the GLB node TRS. Go in parent-before-child order, with `bpy.context.view_layer.update()` between levels.
- **Self-check (must pass first):** applying the bind pose through this path moves no evaluated vertex by more than 0.01 in. The gun armature root `j_gun` is attached by its pose, not by parenting.
- Camera: at `World[tag_view]`, looking +X with up +Z, `sensor_fit = VERTICAL`, `angle = 2·atan(tan(32.5°)·0.75)` (at ADS: the blended fov), clip 0.1..1000. Resolution 1920x1080. Workbench engine, flat lighting, `render_aa = 'OFF'`, `film_transparent`.
- Output: `work\m3\ref_<pose>.png` plus `ref_<pose>.mask` (alpha > 0.5) and `work\m3\blender_report.json`.

**Cases and pass criteria:**

| Case | Pass criterion |
|---|---|
| S (silhouette) | for every pose: IoU(vmtest mask, Blender mask) ≥ **0.90** (fail loudly with the per-pose IoU table and a diff PNG `work\m3\diff_<pose>.png`). The bbox centroid offset ≤ 1% of the width. Expected ≥ 0.97 (only rasterization differences); 0.90..0.95 → PASS with WARN |
| D1 (scene depth does not occlude) | before `record`, a "scene" pass draws a magenta quad at view depth 2 in (nearer than any gun point) into the target with a separate scene DSV (D32 reversed, simulating ER's). Then `record`. Pass: ≥ 99.9% of the idle silhouette pixels are not magenta (the gun is drawn over it), pixels outside the silhouette are byte-identical to the quad/key, and the scene DSV readback is byte-identical before and after `record` (we never touch the host depth) |
| D2 (self-occlusion) | `debug_mode = 1` (gun red, hands green). At the screen projection of `j_wrist_ri` at idle (hand in front of the grip), the pixel is green with depth on. The same pixel is red in a build with `debug_no_depth` (depth off, gun drawn last), which proves the depth buffer decides |
| D3 (untouched outside) | for every pose, pixels outside the mask are exactly the key colour (no clear, no blend leakage) |
| D4 (ring) | 6 consecutive frames over slots 0,1,2,0,1,2 with different poses without waiting between them (fence-wait only at the end; each frame copies its result to its own readback). Every image equals the single-frame render of its pose |
| D5 (resize) | the same `Renderer` renders 1920x1080, then 3840x2160, then 1920x1080. Each matches its single-size reference; no InfoQueue errors |
| W (winding) | the depth buffer from CULL_BACK + FrontCCW=FALSE equals the CULL_NONE depth on ≥ 99% of silhouette pixels, while FrontCCW=TRUE differs on ≥ 20% of them. If not, FAIL with the numbers |
| O (output modes) | R10G10B10A2 + PQ and R16G16B16A16_FLOAT + scRGB renders of idle: no errors; at 16 sampled silhouette pixels the decoded linear value equals the sRGB render's decoded linear value within 2% (paper white consistent) |
| F (fallback) | `vmtest --assets <empty dir>` and `--assets <dir with a truncated .vmmesh>` and `--assets <version 2 header>` each exit with code 3 and print `VM FAIL reason=assets: …` (no panic, no InfoQueue error). Core parsers: truncating each converted file at every 97th byte returns `Err` (unit test, never panics) |

### 12.3 Oracle 3: performance

`vmtest --perf` at 3840x2160 on the hardware adapter:
- 20 warm-up frames, then 300 frames;
- each frame = `record` between two timestamp queries (exactly the in-game bracket), resolved and read after the fence;
- `GetTimestampFrequency` converts ticks to ms.

**Pass: p95 < 1.0 ms and median < 0.5 ms.** Print the median and p95, and write `work\m3\perf.json`. WARP → FAIL "perf needs hardware". The debug layer is off for the perf run (it inflates times) but on for every other case.

### 12.4 Oracle 4: build, tests, M2 still passing

- `cargo build --release` for the workspace (now four crates): exit 0 and no `warning:` with a span under `mod\`.
- `cargo test -p mw2er-core`: exit 0, with ≥ 1 test in each new category: `vm_assets` (incl. truncation), `vm_anim`, `vm_pose` (sampling, loop, crossfade, FK known answers 1g), `vm_proj`, `cam_gate`, `hit_logic` impact attribution, `input_logic` ADS, config `[viewmodel]` bounds.
- `cargo test -p mw2er-render` (pure helpers only: alignment, footprint math, output-mode selection table).
- `check_m2.ps1`: 21 PASS.
- `check_m3.ps1`: every check PASS.

### 12.5 Oracle 5: fallback

Covered by case F (renderer + parser) and core tests of `vm_should_draw`, which is false when `VM_DISABLED`, not ready, config off, the snapshot is stale or a menu is open.
- Static check (check_m3 #15): the DLL's `viewmodel.rs` callback body starts with `guard(` and the loader thread with `guard(`; `PRE_IMGUI.set` happens before `Hudhook::builder()`.
- In game: a run with `viewmodel.enabled = true` and `converted\viewmodel` renamed must show `VM FAIL reason=assets` and M2 PASS. That is an optional user step, listed in the hand-off.

---

## 13. `tools\scripts\check_m3.ps1`

Windows PowerShell 5.1, same style as check_m2:
- refresh PATH from the registry;
- never launch ER, me3, Steam or MW2;
- write only to `work\check_m3\`, `work\m3\`, `converted\viewmodel\`, `mod\target`, `build\natives`;
- one line per check: `PASS|FAIL|SKIP <id> <name> <detail>`;
- exit 0 only if all PASS (SKIP = failure unless `-AllowSkip`);
- `-SkipM2` exists for fast iteration but forces a non-zero exit (the final run must include M2).

| # | Check | Pass criterion |
|---|---|---|
| 1 | M2 suite | `powershell -File check_m2.ps1` exit 0 and its output has 21 `PASS` lines, 0 FAIL/SKIP |
| 2 | hudhook vendor | `vendor\hudhook-0.9.3` exists with LICENSE; a recursive compare against the registry copy differs only in `src/renderer/backend/dx12.rs` and `src/hooks/dx12.rs`; `git apply --check --reverse --directory=vendor/hudhook-0.9.3 tools\patches\hudhook-0001-pre-imgui-hook.patch` succeeds; reverse-applying it to a temp copy yields a tree identical to the registry copy; `cargo metadata` shows hudhook 0.9.3 with a `path` source under `vendor\hudhook-0.9.3` |
| 3 | iw4L pin | `git -C vendor\iw4L rev-parse HEAD` == fc3d8eaf…; NOTICE and LICENSE present; no `vendor\iw4L\target` (never built); not in `cargo metadata` of `mod\` |
| 4 | fxc + shaders | fxc found; `mw2er-render` build output has `viewmodel_vs.cso`/`viewmodel_ps.cso` starting with `DXBC`; the SHA-256s equal `SHADER_SHA256` printed by `vmtest --version` |
| 5 | converter self-test | `convert_viewmodel.py --self-test` exit 0 (all 6 mutations caught) |
| 6 | convert | `convert_viewmodel.py` exit 0; `converted\viewmodel\manifest.json` exists; every listed file's sha256 matches; only files under `converted\viewmodel\` were written (directory snapshot before/after) |
| 7 | anim oracle | `convert_viewmodel.py --check` exit 0: checks 1a(Python), 1b, 1c, 1d, 1f-1k with the report numbers printed |
| 8 | cargo build | workspace release build exit 0, 0 warnings under `mod\` (4 crates); `vmtest.exe` exists |
| 9 | cargo test | `cargo test -p mw2er-core` and `-p mw2er-render` exit 0, with each required category present (section 12.4) |
| 10 | cross-decoder | `MW2ER_ROOT=… cargo test -p mw2er-core --test xanim_cross -- --include-ignored` exit 0 (1a Rust, 1e). Missing dumps → SKIP |
| 11 | Blender reference | Blender 5.2 found; `blender_ref.py` exit 0; its bind self-check PASS; 9 ref masks written; gun triangle count 2,972 |
| 12 | vmtest render | `vmtest.exe --all --out work\m3` exit 0 with the debug layer (and `--gbv` for the idle case): cases D1-D5, W, O, F PASS; 0 InfoQueue errors |
| 13 | silhouette IoU | `vmtest.exe --compare work\m3` (pure Rust over the `.mask` files): every pose IoU ≥ 0.90 (table printed; WARN under 0.95) |
| 14 | performance | `vmtest.exe --perf` p95 < 1.0 ms, median < 0.5 ms at 3840x2160 on hardware |
| 15 | DLL shape + wiring | `m2_static_checks.py pe`: still exactly one export `DllMain`, no VCRUNTIME/api-ms-win-crt imports; the DLL bytes contain the exact `VS` and `PS` .cso bytes (same shaders as vmtest); `m2_static_checks.py vmwiring` (new subcommand): `PRE_IMGUI.set` precedes `Hudhook::builder()` in `hud.rs`, and the callback and loader bodies are wrapped in `guard(`, with a self-test that removes `guard(` and must fail |
| 16 | config | `mw2er.toml` parses with tomllib and with the DLL's parser (existing `config_file` test), including `[viewmodel]` |
| 17 | analyzer | `analyze_log.py --self-test` exit 0, including the new HIT via/kill cases and the c4311 35..75 band (section 11) |
| 18 | hygiene | `.gitignore` still covers `/converted/` `/work/` `/dumps/`; no file under `tools\`, `mod\` (excluding `target`) or `docs\` contains DDS/GLB/xanim magic or is larger than 5 MB; the game folder is untouched (reuse the M2 check 19 function) |

---

## 14. Implementation order (suggested)

1. Converter + `vm_format.py` + oracle 1 (checks 5-7).
2. Core: `vm_assets`, `vm_pose`, `vm_proj` with tests; Rust cross-decoder (check 10).
3. `mw2er-render` + shaders + `vmtest` (checks 4, 8, 12, 14).
4. `blender_ref.py` + IoU (checks 11, 13). Iterate until the IoU passes before touching the DLL.
5. hudhook vendoring + patch (check 2).
6. DLL wiring: `viewmodel.rs`, ADS input, `vm_anim`, snapshot, CAMFRAME + `cam_gate`, hit attribution, analyzer (checks 9, 15-17).
7. Full `check_m3.ps1`, a MODLOG entry, and the hand-off listing what is verified offline vs only in game.

---

## 15. What stays unverified until the user's run

- Whether hudhook's callback in ER's present queue draws visibly on native Windows 1.17.1, i.e. whether the frame order is as expected.
- ER's swapchain format and colour space (logged). HDR output correctness.
- The real GPU time in game (logged `VM gpu_ms`).
- How the viewmodel looks against ER's exposure and tonemapping (tune `exposure`/`ambient_*`).
- The ER sprint speed vs `sprint_speed_mps`.
- RMB blanking for ADS.
- The camera gate thresholds and the `det` sign (CAMFRAME).
- Impact attribution radius and window (HIT `via=impact` count vs IMPACT `kind=hit`).

---

## 16. Decisions the implementer must not change

1. No game launch, ever. No writes to game folders or `%APPDATA%\EldenRing`. Converted data only in `converted\viewmodel\`, test output only in `work\`. Nothing from `dumps\`/`converted\` is committed or copied elsewhere.
2. OAT stays an external offline tool: no OAT code is linked, copied or translated into `mod\` or the shipped DLL. The Rust XAnim reader is written from section 2 of this document. IW4L is reference only. If any IW4L code is ever copied, keep its Apache-2.0 `NOTICE` and mark the file. Never run cargo inside `vendor\iw4L`.
3. Depth: our own D32 reversed-Z buffer, cleared per frame, own projection. Never bind, read or test against ER's DSV, and add no D3D12 hooks in M3 other than the hudhook patch (pre-ImGui callback + SetColorSpace1).
4. The hook is the pre-ImGui callback inside `D3D12RenderEngine::render` (between OMSetRenderTargets and SetDescriptorHeaps), via a vendored hudhook 0.9.3 with one numbered patch verified by check_m3. hudhook stays `=0.9.3`. Exactly one MinHook.
5. The callback records no resource barriers on ER resources, sets its full pipeline state (viewport, **scissor**, heaps, root signature, PSO, RT+DSV), uses per-frame memory indexed by hudhook's `frame_index`, never waits on a fence on the present thread, and is wrapped in `guard`/`catch_unwind`.
6. The same `mw2er-render` code and the same precompiled shader bytes are used by `mw2er.dll` and `vmtest.exe`. `mw2er-render` depends on windows `=0.62.2` and not on hudhook or `eldenring`.
7. Pure animation, pose, projection, asset-parsing, camera-gate and hit-attribution logic lives in `mw2er-core` (`forbid(unsafe_code)`) with tests. Parsers never panic on bad input.
8. Animation semantics: a translation track replaces bind, no translation track keeps bind, no quat track = identity, bones absent from a clip keep bind; ADS is a `tag_ads`-only layer. Do not adopt iw4L's bind + delta composition.
9. Any viewmodel failure (assets, D3D, output mode, panic) logs `VM FAIL`/`PANIC`, disables only the viewmodel, and leaves M2 behaviour intact. Missing `converted\viewmodel` never stops the DLL from booting.
10. Viewmodel FOV is MW2's (65 hip / 55 ADS, 4:3 Hor+), not ER's camera FOV. Units are inches until the projection.
11. The `HIT chr=c#### hp a->b dmg=N` prefix is unchanged. Kill shots carry `kill=1` and are excluded from calibration. The analyzer's c4311 close-range PASS band is 35..75 (baseline 49 at k=4), scaled by atkPhys/160.
12. `check_m3.ps1` runs `check_m2.ps1` first and fails unless all 21 M2 checks and all M3 checks PASS. Oracle thresholds (IoU ≥ 0.90, p95 < 1.0 ms at 4K, 0 InfoQueue errors, reload durations ±10%) are not relaxed without a MODLOG entry that explains why.
13. The first in-game test is with HDR off. `viewmodel.output = "auto"` refuses unknown format/colour-space combinations instead of guessing.
