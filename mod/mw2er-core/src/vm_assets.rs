//! Parsers of the converted viewmodel files in `converted\viewmodel\` (docs/M3_DESIGN.md section 2.3) and, for M6,
//! `converted\guns\<name>\` (docs/M6_DESIGN.md section 3: role -> clip table in the manifest, optional tags,
//! shared textures in `..\_tex\`, the RPG's rocket part, BC2 colour maps).
//!
//! Every file starts with the same 32-byte header (magic, format version, header size, flags, payload size,
//! count, reserved). The parsers are safe code: every length is bounds-checked and every count is capped, so
//! a truncated or corrupt file returns `Err` and never panics or over-allocates. `manifest.json` (written last
//! by the converter) lists the sha256 and size of every file; [`VmAssets::from_files`] refuses any mismatch.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use crate::sha256::sha256_hex;
use crate::xform::{Mat34, Quat, Vec3};

pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 32;
pub const VERTEX_STRIDE: usize = 40;
pub const MAX_BONES: usize = 256;
pub const MAX_VERTICES: usize = 65_535;
pub const MAX_CLIPS: usize = 64;
pub const MAX_TEXTURE_BYTES: usize = 16 << 20;
pub const MAX_FILE_BYTES: u64 = 64 << 20;
pub const DXGI_BC1_UNORM: u32 = 71;
/// M6: the Stinger's colour map is DXT3.
pub const DXGI_BC2_UNORM: u32 = 74;
pub const DXGI_BC3_UNORM: u32 = 77;
/// M6: an optional tag the model does not have (tag_sight_on on 18 of the 34 guns).
pub const NO_TAG: u16 = 0xFFFF;
/// Mesh parts: 0 gun, 1 hands, 2 the RPG's rocket (hidden while the magazine is empty).
pub const PART_GUN: u8 = 0;
pub const PART_HANDS: u8 = 1;
pub const PART_ROCKET: u8 = 2;
pub const SKEL_FILE: &str = "ump45.vmskel";
pub const MESH_FILE: &str = "ump45.vmmesh";
pub const ANIM_FILE: &str = "ump45.vmanim";
pub const MANIFEST_FILE: &str = "manifest.json";

/// The 12 base clips (docs/M3_DESIGN.md section 2.2 step 6), in this order in [`VmAssets::clips`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ClipId {
    Idle = 0,
    Fire,
    FireAds,
    Reload,
    ReloadEmpty,
    Pullout,
    Putaway,
    SprintIn,
    SprintLoop,
    SprintOut,
    AdsUp,
    AdsDown,
    // M6 optional roles (docs/M6_DESIGN.md 3.2); a gun without one falls back (`ClipId::fallback`)
    LastShot,
    Rechamber,
    AdsRechamber,
    ReloadStart,
    ReloadEnd,
    EmptyIdle,
    FirstRaise,
}

/// Every clip role (the 12 M3 base clips + the 7 M6 optional roles).
pub const N_CLIPS: usize = 19;

impl ClipId {
    pub const EXTRA: [ClipId; 7] = [
        ClipId::LastShot,
        ClipId::Rechamber,
        ClipId::AdsRechamber,
        ClipId::ReloadStart,
        ClipId::ReloadEnd,
        ClipId::EmptyIdle,
        ClipId::FirstRaise,
    ];

    pub fn every() -> impl Iterator<Item = ClipId> {
        ClipId::ALL.into_iter().chain(ClipId::EXTRA)
    }

    /// The role played when a gun has no clip for this one (section 5.4).
    pub fn fallback(self) -> ClipId {
        match self {
            ClipId::LastShot => ClipId::Fire,
            ClipId::Rechamber => ClipId::Idle,
            ClipId::AdsRechamber => ClipId::Rechamber,
            ClipId::ReloadStart | ClipId::ReloadEnd => ClipId::Reload,
            ClipId::EmptyIdle => ClipId::Idle,
            ClipId::FirstRaise => ClipId::Pullout,
            ClipId::ReloadEmpty => ClipId::Reload,
            o => o,
        }
    }

    pub const ALL: [ClipId; 12] = [
        ClipId::Idle,
        ClipId::Fire,
        ClipId::FireAds,
        ClipId::Reload,
        ClipId::ReloadEmpty,
        ClipId::Pullout,
        ClipId::Putaway,
        ClipId::SprintIn,
        ClipId::SprintLoop,
        ClipId::SprintOut,
        ClipId::AdsUp,
        ClipId::AdsDown,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ClipId::Idle => "idle",
            ClipId::Fire => "fire",
            ClipId::FireAds => "fire_ads",
            ClipId::Reload => "reload",
            ClipId::ReloadEmpty => "reload_empty",
            ClipId::Pullout => "pullout",
            ClipId::Putaway => "putaway",
            ClipId::SprintIn => "sprint_in",
            ClipId::SprintLoop => "sprint_loop",
            ClipId::SprintOut => "sprint_out",
            ClipId::AdsUp => "ads_up",
            ClipId::AdsDown => "ads_down",
            ClipId::LastShot => "last_shot",
            ClipId::Rechamber => "rechamber",
            ClipId::AdsRechamber => "ads_rechamber",
            ClipId::ReloadStart => "reload_start",
            ClipId::ReloadEnd => "reload_end",
            ClipId::EmptyIdle => "empty_idle",
            ClipId::FirstRaise => "first_raise",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

// ---- cursor -----------------------------------------------------------------------------------------

/// Bounds-checked little-endian reader.
pub struct Cur<'a> {
    d: &'a [u8],
    o: usize,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8], o: usize) -> Self {
        Cur { d, o }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.o.checked_add(n).ok_or("length overflow")?;
        let s = self.d.get(self.o..end).ok_or_else(|| format!("truncated at byte {} (need {n})", self.o))?;
        self.o = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn f32(&mut self) -> Result<f32, String> {
        let v = f32::from_bits(self.u32()?);
        if v.is_finite() { Ok(v) } else { Err(format!("non-finite float at byte {}", self.o - 4)) }
    }

    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let mut a = [0.0f32; N];
        for v in a.iter_mut() {
            *v = self.f32()?;
        }
        Ok(a)
    }

    pub fn name(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        let b = self.take(n)?;
        std::str::from_utf8(b).map(str::to_string).map_err(|_| "name is not UTF-8".to_string())
    }

    pub fn end(&self) -> Result<(), String> {
        if self.o == self.d.len() { Ok(()) } else { Err(format!("{} trailing bytes", self.d.len() - self.o)) }
    }

    pub fn remaining(&self) -> usize {
        self.d.len() - self.o
    }
}

/// Checks the 32-byte header; returns (count, cursor after the header).
pub fn parse_header<'a>(data: &'a [u8], magic: &[u8; 4]) -> Result<(u32, Cur<'a>), String> {
    let mut c = Cur::new(data, 0);
    let m = c.take(4).map_err(|_| "file shorter than the 32-byte header".to_string())?;
    if m != magic {
        return Err(format!("magic {:?} != {:?}", String::from_utf8_lossy(m), String::from_utf8_lossy(magic)));
    }
    let ver = c.u32()?;
    if ver != FORMAT_VERSION {
        return Err(format!("format version {ver} != {FORMAT_VERSION}"));
    }
    let hs = c.u32()?;
    let flags = c.u32()?;
    let plen = c.u64()?;
    let count = c.u32()?;
    let res = c.u32()?;
    if hs as usize != HEADER_SIZE || flags != 0 || res != 0 {
        return Err("bad header fields".into());
    }
    if plen != (data.len() - HEADER_SIZE) as u64 {
        return Err(format!("payload size {plen} != file size - 32 ({})", data.len() - HEADER_SIZE));
    }
    Ok((count, c))
}

// ---- skeleton ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    /// -1 = root; always before the bone itself.
    pub parent: i16,
    pub t: Vec3,
    pub q: Quat,
    /// Inverse bind matrix (MW2 space).
    pub ibm: Mat34,
}

/// Indices of the tags the runtime needs. M6: `sight_on`, `flash`, `wrist_ri`, `wrist_le` may be [`NO_TAG`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkelTags {
    pub view: u16,
    pub ads: u16,
    pub weapon: u16,
    pub j_gun: u16,
    pub flash: u16,
    pub sight_on: u16,
    pub wrist_ri: u16,
    pub wrist_le: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    pub tags: SkelTags,
}

impl Skeleton {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }
}

pub fn parse_skel(data: &[u8]) -> Result<Skeleton, String> {
    let (n, mut c) = parse_header(data, b"VMSK")?;
    let n = n as usize;
    if n == 0 || n > MAX_BONES {
        return Err(format!("{n} bones (1..={MAX_BONES})"));
    }
    let mut bones = Vec::with_capacity(n);
    for i in 0..n {
        let name = c.name()?;
        let parent = c.i16()?;
        if parent < -1 || parent as isize >= i as isize {
            return Err(format!("bone {name}: parent {parent} not before it"));
        }
        let t = c.f32s::<3>()?;
        let q = c.f32s::<4>()?;
        let ibm = c.f32s::<12>()?;
        bones.push(Bone { name, parent, t, q, ibm });
    }
    let mut tag = |optional: bool| -> Result<u16, String> {
        let v = c.u16()?;
        if (v as usize) < n || (optional && v == NO_TAG) { Ok(v) } else { Err(format!("tag index {v} >= {n}")) }
    };
    let tags = SkelTags {
        view: tag(false)?,
        ads: tag(false)?,
        weapon: tag(false)?,
        j_gun: tag(false)?,
        flash: tag(true)?,
        sight_on: tag(true)?,
        wrist_ri: tag(true)?,
        wrist_le: tag(true)?,
    };
    c.end()?;
    Ok(Skeleton { bones, tags })
}

// ---- mesh -------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u16,
    /// 0 gun, 1 hands, 2 rocket (M6, the RPG).
    pub part: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    pub name: String,
    pub texture: u16,
    /// 0 opaque, 1 alpha blended.
    pub blend: u8,
}

/// One decoded vertex (CPU-side checks; the GPU gets [`Mesh::vertex_bytes`] as is).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub pos: Vec3,
    pub normal: [i16; 4],
    pub uv: [f32; 2],
    pub joints: [u8; 4],
    pub weights: [u8; 4],
    pub color: [u8; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub vertex_count: usize,
    /// `vertex_count * VERTEX_STRIDE` bytes: f32x3 pos, i16x4 normal, f32x2 uv, u8x4 joints, u8x4 weights,
    /// u8x4 colour.
    pub vertex_bytes: Vec<u8>,
    pub indices: Vec<u16>,
    pub surfaces: Vec<Surface>,
    pub materials: Vec<Material>,
    pub textures: Vec<String>,
}

impl Mesh {
    pub fn vertex(&self, i: usize) -> Option<Vertex> {
        let b = self.vertex_bytes.get(i * VERTEX_STRIDE..(i + 1) * VERTEX_STRIDE)?;
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let s = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        Some(Vertex {
            pos: [f(0), f(4), f(8)],
            normal: [s(12), s(14), s(16), s(18)],
            uv: [f(20), f(24)],
            joints: [b[28], b[29], b[30], b[31]],
            weights: [b[32], b[33], b[34], b[35]],
            color: [b[36], b[37], b[38], b[39]],
        })
    }

    pub fn triangles(&self, part: u8) -> usize {
        self.surfaces.iter().filter(|s| s.part == part).map(|s| s.index_count as usize / 3).sum()
    }
}

pub fn parse_mesh(data: &[u8], bone_count: usize) -> Result<Mesh, String> {
    let (n, mut c) = parse_header(data, b"VMME")?;
    let n = n as usize;
    if n == 0 || n > MAX_VERTICES {
        return Err(format!("{n} vertices (1..={MAX_VERTICES})"));
    }
    let vertex_bytes = c.take(n * VERTEX_STRIDE)?.to_vec();
    let ni = c.u32()? as usize;
    if ni == 0 || ni % 3 != 0 || ni > c.remaining() / 2 {
        return Err(format!("index count {ni}"));
    }
    let raw = c.take(ni * 2)?;
    let indices: Vec<u16> = raw.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
    if let Some(bad) = indices.iter().find(|&&i| i as usize >= n) {
        return Err(format!("index {bad} >= vertex count {n}"));
    }
    let ns = c.u16()? as usize;
    let mut surfaces = Vec::with_capacity(ns.min(256));
    for _ in 0..ns {
        let s = Surface { first_index: c.u32()?, index_count: c.u32()?, material: c.u16()?, part: c.u8()? };
        let end = s.first_index as usize + s.index_count as usize;
        if end > ni || s.index_count % 3 != 0 || s.part > PART_ROCKET {
            return Err(format!("surface {s:?} outside the index buffer"));
        }
        surfaces.push(s);
    }
    let nm = c.u16()? as usize;
    let mut materials = Vec::with_capacity(nm.min(256));
    for _ in 0..nm {
        let name = c.name()?;
        let texture = c.u16()?;
        let blend = c.u8()?;
        if blend > 1 {
            return Err(format!("material {name}: blend {blend}"));
        }
        materials.push(Material { name, texture, blend });
    }
    let nt = c.u16()? as usize;
    let mut textures = Vec::with_capacity(nt.min(256));
    for _ in 0..nt {
        let t = c.name()?;
        if t.contains(['/', '\\']) || t.contains("..") || !t.ends_with(".vmtex") {
            return Err(format!("texture file name {t:?}"));
        }
        textures.push(t);
    }
    c.end()?;
    if surfaces.iter().any(|s| s.material as usize >= materials.len()) {
        return Err("surface material out of range".into());
    }
    if materials.iter().any(|m| m.texture as usize >= textures.len()) {
        return Err("material texture out of range".into());
    }
    let mesh = Mesh { vertex_count: n, vertex_bytes, indices, surfaces, materials, textures };
    for i in 0..n {
        let v = mesh.vertex(i).ok_or("vertex out of range")?;
        if v.pos.iter().chain(v.uv.iter()).any(|x| !x.is_finite()) {
            return Err(format!("vertex {i}: non-finite value"));
        }
        if v.weights.iter().map(|&w| w as u32).sum::<u32>() != 255 {
            return Err(format!("vertex {i}: weights do not sum to 255"));
        }
        if v.joints.iter().any(|&j| j as usize >= bone_count) {
            return Err(format!("vertex {i}: joint >= {bone_count}"));
        }
    }
    Ok(mesh)
}

// ---- anims ------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub bone: u16,
    pub rot_frames: Vec<u16>,
    pub rot: Vec<Quat>,
    pub trans_frames: Vec<u16>,
    pub trans: Vec<Vec3>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub name: String,
    pub fps: u16,
    pub numframes: u16,
    pub looped: bool,
    pub notes: Vec<(String, u16)>,
    pub tracks: Vec<Track>,
}

impl Clip {
    /// Native duration in seconds.
    pub fn duration(&self) -> f32 {
        if self.fps == 0 { 0.0 } else { self.numframes as f32 / self.fps as f32 }
    }

    pub fn track_of(&self, bone: usize) -> Option<&Track> {
        self.tracks.iter().find(|t| t.bone as usize == bone)
    }
}

fn frames(c: &mut Cur<'_>, numframes: u16) -> Result<Vec<u16>, String> {
    let n = c.u16()? as usize;
    if n > c.remaining() / 2 {
        return Err("key count larger than the file".into());
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(c.u16()?);
    }
    if v.windows(2).any(|w| w[0] >= w[1]) || v.last().is_some_and(|&f| f > numframes) {
        return Err("key frames not increasing or past the clip end".into());
    }
    Ok(v)
}

pub fn parse_anim(data: &[u8], bone_count: usize) -> Result<Vec<Clip>, String> {
    let (n, mut c) = parse_header(data, b"VMAN")?;
    let n = n as usize;
    if n == 0 || n > MAX_CLIPS {
        return Err(format!("{n} clips (1..={MAX_CLIPS})"));
    }
    let mut clips = Vec::with_capacity(n);
    for _ in 0..n {
        let name = c.name()?;
        let fps = c.u16()?;
        let numframes = c.u16()?;
        let looped = match c.u8()? {
            0 => false,
            1 => true,
            v => return Err(format!("clip {name}: looped flag {v}")),
        };
        if fps == 0 || fps > 1000 {
            return Err(format!("clip {name}: fps {fps}"));
        }
        let nn = c.u8()? as usize;
        let mut notes = Vec::with_capacity(nn);
        for _ in 0..nn {
            let nm = c.name()?;
            notes.push((nm, c.u16()?));
        }
        let nt = c.u16()? as usize;
        if nt > bone_count {
            return Err(format!("clip {name}: {nt} tracks > {bone_count} bones"));
        }
        let mut tracks = Vec::with_capacity(nt);
        for _ in 0..nt {
            let bone = c.u16()?;
            if bone as usize >= bone_count {
                return Err(format!("clip {name}: track bone {bone}"));
            }
            let rot_frames = frames(&mut c, numframes)?;
            let mut rot = Vec::with_capacity(rot_frames.len());
            for _ in 0..rot_frames.len() {
                rot.push(c.f32s::<4>()?);
            }
            let trans_frames = frames(&mut c, numframes)?;
            let mut trans = Vec::with_capacity(trans_frames.len());
            for _ in 0..trans_frames.len() {
                trans.push(c.f32s::<3>()?);
            }
            tracks.push(Track { bone, rot_frames, rot, trans_frames, trans });
        }
        clips.push(Clip { name, fps, numframes, looped, notes, tracks });
    }
    c.end()?;
    Ok(clips)
}

// ---- textures ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mip {
    pub row_pitch: u32,
    pub rows: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    /// `DXGI_FORMAT_BC1_UNORM` (71), `BC2_UNORM` (74, M6) or `BC3_UNORM` (77); the renderer views them as `_SRGB`.
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Mip>,
    /// sha256 of the file (M6 dedup check; empty for the M3 loader).
    pub sha256: String,
}

pub fn parse_tex(data: &[u8]) -> Result<Texture, String> {
    if data.len() > MAX_TEXTURE_BYTES + 4096 {
        return Err(format!("texture file larger than {MAX_TEXTURE_BYTES} bytes"));
    }
    let (n, mut c) = parse_header(data, b"VMTX")?;
    let format = c.u32()?;
    let width = c.u32()?;
    let height = c.u32()?;
    let mips = c.u32()?;
    let block = match format {
        DXGI_BC1_UNORM => 8u32,
        DXGI_BC2_UNORM | DXGI_BC3_UNORM => 16,
        f => return Err(format!("texture format {f} (BC1 71 / BC2 74 / BC3 77 only)")),
    };
    if mips != n || mips == 0 || mips > 15 || width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(format!("texture {width}x{height} mips {mips}/{n}"));
    }
    let mut levels = Vec::with_capacity(mips as usize);
    for m in 0..mips {
        let (w, h) = ((width >> m).max(1), (height >> m).max(1));
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let row_pitch = c.u32()?;
        let rows = c.u32()?;
        let size = c.u32()?;
        if row_pitch != bw * block || rows != bh || size != row_pitch * rows {
            return Err(format!("mip {m}: pitch {row_pitch} rows {rows} size {size} (want {} {bh})", bw * block));
        }
        levels.push(Mip { row_pitch, rows, data: c.take(size as usize)?.to_vec() });
    }
    c.end()?;
    Ok(Texture { format, width, height, mips: levels, sha256: String::new() })
}

// ---- manifest + bundle ------------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestEntry {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    #[serde(default)]
    pub converter: String,
    pub files: BTreeMap<String, ManifestEntry>,
    #[serde(default)]
    pub weapon_times: BTreeMap<String, f64>,
    /// M6: clip role -> clip name in the .vmanim (absent in the M3 manifest: the role names are the clip names).
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    #[serde(default)]
    pub skel_file: Option<String>,
    #[serde(default)]
    pub mesh_file: Option<String>,
    #[serde(default)]
    pub anim_file: Option<String>,
    /// C3 PASS C: the world-model LOD meshes (`world_lod<n>.vmmesh`, rigid: every vertex on joint 0).
    #[serde(default)]
    pub world_files: Vec<String>,
}

/// C3 PASS C (docs/C3_DESIGN.md 3.6): one LOD of a gun's world model, drawn on characters by the renderer.
#[derive(Clone, Debug)]
pub struct WorldMesh {
    pub lod: u8,
    pub mesh: Mesh,
    pub textures: Vec<Arc<Texture>>,
    pub bytes: u64,
}

/// The LOD of a `world_lod<n>.vmmesh` file name (0..=3).
pub fn world_lod_of(name: &str) -> Option<u8> {
    let n: u8 = name.strip_prefix("world_lod")?.strip_suffix(".vmmesh")?.parse().ok()?;
    (n <= 3).then_some(n)
}

/// Weapon-file times copied by the converter (docs/M3_DESIGN.md section 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponTimes {
    pub fire: f32,
    pub reload: f32,
    pub reload_empty: f32,
    pub raise: f32,
    pub drop: f32,
    pub sprint_in: f32,
    pub sprint_loop: f32,
    pub sprint_out: f32,
    pub ads_in: f32,
    pub ads_out: f32,
    pub ads_zoom_fov: f32,
    /// M6 (0 = the gun has none): firstRaiseTime, rechamberTime, reloadStartTime, reloadEndTime.
    pub first_raise: f32,
    pub rechamber: f32,
    pub reload_start: f32,
    pub reload_end: f32,
}

impl Default for WeaponTimes {
    /// The UMP45 values (used by tests and when the manifest has no times).
    fn default() -> Self {
        WeaponTimes {
            fire: 0.09,
            reload: 2.5,
            reload_empty: 3.03,
            raise: 0.76,
            drop: 0.8,
            sprint_in: 0.3,
            sprint_loop: 0.7,
            sprint_out: 0.3,
            ads_in: 0.2,
            ads_out: 0.2,
            ads_zoom_fov: 55.0,
            first_raise: 0.0,
            rechamber: 0.0,
            reload_start: 0.0,
            reload_end: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VmAssets {
    pub skel: Skeleton,
    pub mesh: Mesh,
    /// Indexed by [`ClipId::index`]: all `N_CLIPS` roles (a missing optional role holds its fallback's clip).
    pub clips: Vec<Clip>,
    /// M6: the gun has its own clip for this role (not a fallback).
    pub native: [bool; N_CLIPS],
    /// Same order as `mesh.textures` (shared between guns: M6 deduplicates them by file).
    pub textures: Vec<Arc<Texture>>,
    pub weapon_times: WeaponTimes,
    pub total_bytes: u64,
}

/// Texture files already parsed, by file name (M6: the guns share hands / attachment textures).
pub type TextureCache = BTreeMap<String, Arc<Texture>>;

impl VmAssets {
    pub fn clip(&self, id: ClipId) -> &Clip {
        &self.clips[id.index()]
    }

    /// M6: the gun has its own clip for `id`.
    pub fn has(&self, id: ClipId) -> bool {
        self.native[id.index()]
    }

    /// M6: the mesh has a rocket part (the RPG).
    pub fn has_rocket(&self) -> bool {
        self.mesh.surfaces.iter().any(|s| s.part == PART_ROCKET)
    }

    /// M6: reads `converted\guns\<name>\` (manifest written last by convert_arsenal.py). Only the files the viewmodel
    /// needs are read and hashed (skeleton, mesh, anims, colour maps); textures named `../_tex/<file>` are shared and
    /// parsed once through `cache`.
    pub fn load_gun_dir(dir: &Path, cache: &mut TextureCache) -> Result<VmAssets, String> {
        let mtext = read_capped(&dir.join(MANIFEST_FILE), 1 << 20)?;
        let mtext = String::from_utf8(mtext).map_err(|_| "manifest.json is not UTF-8".to_string())?;
        let man: Manifest = serde_json::from_str(&mtext).map_err(|e| format!("manifest.json: {e}"))?;
        let skel_f = man.skel_file.clone().unwrap_or_else(|| SKEL_FILE.into());
        let mesh_f = man.mesh_file.clone().unwrap_or_else(|| MESH_FILE.into());
        let anim_f = man.anim_file.clone().unwrap_or_else(|| ANIM_FILE.into());
        let mut files = BTreeMap::new();
        let mut shared: BTreeMap<String, Arc<Texture>> = BTreeMap::new();
        let mut total = 0u64;
        let mut need = |name: &str, files: &mut BTreeMap<String, Vec<u8>>| -> Result<(), String> {
            let e = man.files.get(name).ok_or_else(|| format!("{name} not listed in the manifest"))?;
            check_rel_name(name)?;
            let d = read_capped(&dir.join(name.replace('/', "\\")), MAX_FILE_BYTES)?;
            if d.len() as u64 != e.size || !sha256_hex(&d).eq_ignore_ascii_case(&e.sha256) {
                return Err(format!("{name}: size/sha256 differ from the manifest"));
            }
            total += e.size;
            files.insert(name.to_string(), d);
            Ok(())
        };
        for n in [&skel_f, &mesh_f, &anim_f] {
            need(n, &mut files)?;
        }
        let skel = parse_skel(&files[&skel_f]).map_err(|e| format!("{skel_f}: {e}"))?;
        let mesh = parse_mesh(&files[&mesh_f], skel.bones.len()).map_err(|e| format!("{mesh_f}: {e}"))?;
        for t in &mesh.textures {
            let rel = if man.files.contains_key(t) { t.clone() } else { format!("../_tex/{t}") };
            if let Some(tx) = cache.get(&rel) {
                // verified against this gun's manifest too (same sha by construction of the dedup)
                let e = man.files.get(&rel).ok_or_else(|| format!("{rel} not listed in the manifest"))?;
                if !tx.sha256.eq_ignore_ascii_case(&e.sha256) {
                    return Err(format!("{rel}: cached texture sha differs from this manifest"));
                }
                shared.insert(t.clone(), tx.clone());
                continue;
            }
            need(&rel, &mut files)?;
            let mut tx = parse_tex(&files[&rel]).map_err(|e| format!("{t}: {e}"))?;
            tx.sha256 = man.files[&rel].sha256.to_ascii_lowercase();
            let tx = Arc::new(tx);
            cache.insert(rel.clone(), tx.clone());
            shared.insert(t.clone(), tx);
            files.remove(&rel);
        }
        let mut a = Self::assemble(&man, skel, mesh, &files[&anim_f], &anim_f, |name| shared.get(name).cloned())?;
        a.total_bytes = total;
        Ok(a)
    }

    /// C3 PASS C: the world-model LODs listed in a gun's manifest (`world_files`), each checked against the manifest's
    /// size / sha256, parsed as a ONE-joint mesh (a skinned mesh is refused) with its colour maps through `cache`
    /// (`../_tex`, shared with the viewmodels). An older conversion without world files gives an empty list.
    pub fn load_gun_world(dir: &Path, cache: &mut TextureCache) -> Result<Vec<WorldMesh>, String> {
        let mtext = read_capped(&dir.join(MANIFEST_FILE), 1 << 20)?;
        let mtext = String::from_utf8(mtext).map_err(|_| "manifest.json is not UTF-8".to_string())?;
        let man: Manifest = serde_json::from_str(&mtext).map_err(|e| format!("manifest.json: {e}"))?;
        let read_checked = |name: &str| -> Result<Vec<u8>, String> {
            let e = man.files.get(name).ok_or_else(|| format!("{name} not listed in the manifest"))?;
            check_rel_name(name)?;
            let d = read_capped(&dir.join(name.replace('/', "\\")), MAX_FILE_BYTES)?;
            if d.len() as u64 != e.size || !sha256_hex(&d).eq_ignore_ascii_case(&e.sha256) {
                return Err(format!("{name}: size/sha256 differ from the manifest"));
            }
            Ok(d)
        };
        let mut out = Vec::new();
        for f in &man.world_files {
            let lod = world_lod_of(f).ok_or_else(|| format!("world file name {f:?}"))?;
            let data = read_checked(f)?;
            let mesh = parse_mesh(&data, 1).map_err(|e| format!("{f}: {e}"))?;
            let mut bytes = data.len() as u64;
            let mut textures = Vec::with_capacity(mesh.textures.len());
            for t in &mesh.textures {
                let rel = if man.files.contains_key(t) { t.clone() } else { format!("../_tex/{t}") };
                let e = man.files.get(&rel).ok_or_else(|| format!("{rel} not listed in the manifest"))?;
                if let Some(tx) = cache.get(&rel) {
                    if !tx.sha256.eq_ignore_ascii_case(&e.sha256) {
                        return Err(format!("{rel}: cached texture sha differs from this manifest"));
                    }
                    textures.push(tx.clone());
                    continue;
                }
                let d = read_checked(&rel)?;
                bytes += d.len() as u64;
                let mut tx = parse_tex(&d).map_err(|e| format!("{t}: {e}"))?;
                tx.sha256 = e.sha256.to_ascii_lowercase();
                let tx = Arc::new(tx);
                cache.insert(rel, tx.clone());
                textures.push(tx);
            }
            out.push(WorldMesh { lod, mesh, textures, bytes });
        }
        out.sort_by_key(|w| w.lod);
        Ok(out)
    }

    /// Shared by the M3 and M6 loaders: clips by role (+ fallbacks), textures, weapon times.
    fn assemble(
        man: &Manifest,
        skel: Skeleton,
        mesh: Mesh,
        anim_bytes: &[u8],
        anim_f: &str,
        tex: impl Fn(&str) -> Option<Arc<Texture>>,
    ) -> Result<VmAssets, String> {
        let nb = skel.bones.len();
        let all = parse_anim(anim_bytes, nb).map_err(|e| format!("{anim_f}: {e}"))?;
        let mut clips: Vec<Option<Clip>> = vec![None; N_CLIPS];
        let mut native = [false; N_CLIPS];
        for id in ClipId::every() {
            let want = man.roles.get(id.name()).map(String::as_str).unwrap_or(id.name());
            if let Some(c) = all.iter().find(|c| c.name == want) {
                clips[id.index()] = Some(c.clone());
                native[id.index()] = true;
            } else if ClipId::ALL.contains(&id) && id != ClipId::ReloadEmpty {
                return Err(format!("clip {} missing", id.name()));
            }
        }
        // fallbacks (two passes: ads_rechamber -> rechamber -> idle)
        for _ in 0..2 {
            for id in ClipId::every() {
                if clips[id.index()].is_none() {
                    clips[id.index()] = clips[id.fallback().index()].clone();
                }
            }
        }
        let clips: Vec<Clip> = clips.into_iter().map(|c| c.ok_or("clip fallback missing")).collect::<Result<_, _>>()?;
        let mut textures = Vec::with_capacity(mesh.textures.len());
        for t in &mesh.textures {
            textures.push(tex(t).ok_or_else(|| format!("{t} missing"))?);
        }
        let wt = |k: &str, d: f32| -> f32 {
            man.weapon_times.get(k).map(|&v| v as f32).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(d)
        };
        let d = WeaponTimes::default();
        let weapon_times = WeaponTimes {
            fire: wt("fireTime", d.fire),
            reload: wt("reloadTime", d.reload),
            // M6: a launcher's reloadEmptyTime 0 means "the reload time"; an M3 manifest without the key keeps the default
            reload_empty: wt("reloadEmptyTime", if man.weapon_times.contains_key("reloadEmptyTime") { wt("reloadTime", d.reload) } else { d.reload_empty }),
            raise: wt("raiseTime", d.raise),
            drop: wt("dropTime", d.drop),
            sprint_in: wt("sprintInTime", d.sprint_in),
            sprint_loop: wt("sprintLoopTime", d.sprint_loop),
            sprint_out: wt("sprintOutTime", d.sprint_out),
            ads_in: wt("adsTransInTime", d.ads_in),
            ads_out: wt("adsTransOutTime", d.ads_out),
            ads_zoom_fov: wt("adsZoomFov", d.ads_zoom_fov),
            first_raise: wt("firstRaiseTime", 0.0),
            rechamber: wt("rechamberTime", 0.0),
            reload_start: wt("reloadStartTime", 0.0),
            reload_end: wt("reloadEndTime", 0.0),
        };
        Ok(VmAssets { skel, mesh, clips, native, textures, weapon_times, total_bytes: 0 })
    }

    /// Reads and validates a converted folder: manifest first (format version, every hash and size), then
    /// every file. Errors are one-line reasons for the `VM FAIL reason=assets: ...` log line.
    pub fn load_dir(dir: &Path) -> Result<VmAssets, String> {
        let mpath = dir.join(MANIFEST_FILE);
        let mtext = read_capped(&mpath, 1 << 20)?;
        let mtext = String::from_utf8(mtext).map_err(|_| "manifest.json is not UTF-8".to_string())?;
        let man: Manifest = serde_json::from_str(&mtext).map_err(|e| format!("manifest.json: {e}"))?;
        let mut files = BTreeMap::new();
        for name in man.files.keys() {
            if name.contains(['/', '\\']) || name.contains("..") {
                return Err(format!("manifest file name {name:?}"));
            }
            files.insert(name.clone(), read_capped(&dir.join(name), MAX_FILE_BYTES)?);
        }
        Self::from_files(&mtext, &files)
    }

    /// Validates the manifest text against the file contents and parses everything.
    pub fn from_files(manifest_json: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<VmAssets, String> {
        let man: Manifest = serde_json::from_str(manifest_json).map_err(|e| format!("manifest.json: {e}"))?;
        if man.format_version != FORMAT_VERSION {
            return Err(format!("manifest format_version {} != {FORMAT_VERSION}", man.format_version));
        }
        let mut total = 0u64;
        for (name, e) in &man.files {
            let d = files.get(name).ok_or_else(|| format!("{name} listed in the manifest but missing"))?;
            if d.len() as u64 != e.size {
                return Err(format!("{name}: size {} != manifest {}", d.len(), e.size));
            }
            if !sha256_hex(d).eq_ignore_ascii_case(&e.sha256) {
                return Err(format!("{name}: sha256 differs from the manifest"));
            }
            total += e.size;
        }
        let get = |n: &str| -> Result<&Vec<u8>, String> {
            if !man.files.contains_key(n) {
                return Err(format!("{n} not listed in the manifest"));
            }
            files.get(n).ok_or_else(|| format!("{n} missing"))
        };
        let skel = parse_skel(get(SKEL_FILE)?).map_err(|e| format!("{SKEL_FILE}: {e}"))?;
        let nb = skel.bones.len();
        let mesh = parse_mesh(get(MESH_FILE)?, nb).map_err(|e| format!("{MESH_FILE}: {e}"))?;
        let mut texs = BTreeMap::new();
        for t in &mesh.textures {
            texs.insert(t.clone(), Arc::new(parse_tex(get(t)?).map_err(|e| format!("{t}: {e}"))?));
        }
        // the M3 clips must have frames (the M6 loader accepts the 0-frame static poses of some guns)
        let all = parse_anim(get(ANIM_FILE)?, nb).map_err(|e| format!("{ANIM_FILE}: {e}"))?;
        for id in ClipId::ALL {
            if all.iter().any(|c| c.name == id.name() && c.numframes == 0) {
                return Err(format!("clip {} has no frames", id.name()));
            }
        }
        let mut a = Self::assemble(&man, skel, mesh, get(ANIM_FILE)?, ANIM_FILE, |n| texs.get(n).cloned())?;
        a.total_bytes = total;
        Ok(a)
    }
}

/// A manifest file name: a plain name, or `../_tex/<name>` / `../_snd/<name>` (M6 shared folders).
fn check_rel_name(name: &str) -> Result<(), String> {
    let plain = |s: &str| !s.is_empty() && !s.contains(['/', '\\', ':']) && !s.contains("..");
    let ok = plain(name) || ["../_tex/", "../_snd/"].iter().any(|p| name.strip_prefix(p).is_some_and(plain));
    if ok { Ok(()) } else { Err(format!("manifest file name {name:?}")) }
}

fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = Vec::new();
    f.take(cap + 1).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.len() as u64 > cap {
        return Err(format!("{}: larger than {cap} bytes", path.display()));
    }
    Ok(buf)
}

#[cfg(test)]
mod c3_world_tests {
    use super::*;

    #[test]
    fn c3_worldgun_files_and_rigid_rule() {
        assert_eq!(world_lod_of("world_lod0.vmmesh"), Some(0));
        assert_eq!(world_lod_of("world_lod1.vmmesh"), Some(1));
        assert_eq!(world_lod_of("world_lod9.vmmesh"), None);
        assert_eq!(world_lod_of("gun.vmmesh"), None);
        assert_eq!(world_lod_of("../world_lod0.vmmesh"), None);
        // a skinned mesh (joints > 0) is refused as a world mesh (one joint), accepted with its skeleton
        let m = testdata::mesh_bytes();
        let n = parse_skel(&testdata::skel_bytes()).unwrap().bones.len();
        assert!(parse_mesh(&m, n).is_ok());
        assert!(parse_mesh(&m, 1).is_err(), "world meshes are rigid");
        // an older manifest without world files parses (empty list)
        let man: Manifest = serde_json::from_str(r#"{"format_version":1,"files":{}}"#).unwrap();
        assert!(man.world_files.is_empty());
    }
}

// ---- builders for tests (the converter is the real writer) -------------------------------------------

#[cfg(test)]
pub(crate) mod testdata {
    use super::*;

    pub fn header(magic: &[u8; 4], count: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = magic.to_vec();
        v.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        v.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn name(v: &mut Vec<u8>, s: &str) {
        v.push(s.len() as u8);
        v.extend_from_slice(s.as_bytes());
    }

    fn f(v: &mut Vec<u8>, xs: &[f32]) {
        for x in xs {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }

    /// A 9-bone chain with every SKEL tag present: root, tag_view (z=60), tag_ads, tag_weapon (x=10), j_gun,
    /// tag_flash (x=16 under j_gun), tag_sight_on, j_wrist_ri, j_wrist_le.
    pub fn skel_bytes() -> Vec<u8> {
        let bones: [(&str, i16, [f32; 3]); 9] = [
            ("tag_origin", -1, [0.0, 0.0, 0.0]),
            ("tag_view", 0, [0.0, 0.0, 60.0]),
            ("tag_ads", 1, [0.0, 0.0, 0.0]),
            ("tag_weapon", 2, [10.0, -2.0, -4.0]),
            ("j_gun", 3, [0.0, 0.0, 0.0]),
            ("tag_flash", 4, [16.0, 0.0, 0.0]),
            ("tag_sight_on", 4, [12.0, 0.0, 2.0]),
            ("j_wrist_ri", 3, [0.0, -2.0, -2.0]),
            ("j_wrist_le", 3, [6.0, 0.0, -2.0]),
        ];
        let mut world = [[0.0f32; 3]; 9];
        let mut p = Vec::new();
        for (i, (n, par, t)) in bones.iter().enumerate() {
            let base = if *par >= 0 { world[*par as usize] } else { [0.0; 3] };
            world[i] = [base[0] + t[0], base[1] + t[1], base[2] + t[2]];
            name(&mut p, n);
            p.extend_from_slice(&par.to_le_bytes());
            f(&mut p, t);
            f(&mut p, &[0.0, 0.0, 0.0, 1.0]);
            let w = world[i];
            f(&mut p, &[1.0, 0.0, 0.0, -w[0], 0.0, 1.0, 0.0, -w[1], 0.0, 0.0, 1.0, -w[2]]);
        }
        for i in [1u16, 2, 3, 4, 5, 6, 7, 8] {
            p.extend_from_slice(&i.to_le_bytes());
        }
        header(b"VMSK", 9, &p)
    }

    /// One triangle per part, two materials (one alpha), one texture.
    pub fn mesh_bytes() -> Vec<u8> {
        let mut p = Vec::new();
        let verts: [([f32; 3], u8); 6] = [
            ([20.0, -2.0, 50.0], 4),
            ([30.0, -2.0, 50.0], 4),
            ([20.0, -2.0, 54.0], 4),
            ([10.0, -4.0, 52.0], 7),
            ([12.0, -4.0, 52.0], 7),
            ([10.0, -4.0, 54.0], 7),
        ];
        for (pos, j) in verts {
            f(&mut p, &pos);
            for s in [0i16, -32767, 0, 0] {
                p.extend_from_slice(&s.to_le_bytes());
            }
            f(&mut p, &[0.5, 0.5]);
            p.extend_from_slice(&[j, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        }
        p.extend_from_slice(&6u32.to_le_bytes());
        for i in 0u16..6 {
            p.extend_from_slice(&i.to_le_bytes());
        }
        p.extend_from_slice(&2u16.to_le_bytes());
        for (first, mat, part) in [(0u32, 0u16, 0u8), (3, 1, 1)] {
            p.extend_from_slice(&first.to_le_bytes());
            p.extend_from_slice(&3u32.to_le_bytes());
            p.extend_from_slice(&mat.to_le_bytes());
            p.push(part);
        }
        p.extend_from_slice(&2u16.to_le_bytes());
        for (n, blend) in [("mtl_a", 0u8), ("mtl_b", 1)] {
            name(&mut p, n);
            p.extend_from_slice(&0u16.to_le_bytes());
            p.push(blend);
        }
        p.extend_from_slice(&1u16.to_le_bytes());
        name(&mut p, "t.vmtex");
        header(b"VMME", 6, &p)
    }

    /// Every base clip: 10 frames at 30 fps; tag_weapon translates x 10 -> 12; fire/reload also rotate j_gun;
    /// ads_up/ads_down move tag_ads only; sprint_loop and idle are looped.
    pub fn anim_bytes() -> Vec<u8> {
        let mut p = Vec::new();
        for id in ClipId::ALL {
            name(&mut p, id.name());
            p.extend_from_slice(&30u16.to_le_bytes());
            p.extend_from_slice(&10u16.to_le_bytes());
            p.push(matches!(id, ClipId::SprintLoop | ClipId::Idle) as u8);
            p.push(0);
            let (bone, keys): (u16, [[f32; 3]; 2]) = match id {
                ClipId::AdsUp => (2, [[0.0, 0.0, 0.0], [-4.0, 2.0, 1.0]]),
                ClipId::AdsDown => (2, [[-4.0, 2.0, 1.0], [0.0, 0.0, 0.0]]),
                ClipId::SprintLoop | ClipId::Idle => (3, [[10.0, -2.0, -4.0], [10.0, -2.0, -4.0]]),
                _ => (3, [[10.0, -2.0, -4.0], [12.0, -2.0, -4.0]]),
            };
            let rot = matches!(id, ClipId::Fire | ClipId::Reload);
            p.extend_from_slice(&(1u16 + rot as u16).to_le_bytes());
            // track on `bone`: no rotation, 2 translation keys at frames 0 and 10
            p.extend_from_slice(&bone.to_le_bytes());
            p.extend_from_slice(&0u16.to_le_bytes());
            p.extend_from_slice(&2u16.to_le_bytes());
            p.extend_from_slice(&0u16.to_le_bytes());
            p.extend_from_slice(&10u16.to_le_bytes());
            f(&mut p, &keys[0]);
            f(&mut p, &keys[1]);
            if rot {
                // j_gun: 90 degrees about Z at frame 10, no translation track
                p.extend_from_slice(&4u16.to_le_bytes());
                p.extend_from_slice(&2u16.to_le_bytes());
                p.extend_from_slice(&0u16.to_le_bytes());
                p.extend_from_slice(&10u16.to_le_bytes());
                let h = std::f32::consts::FRAC_1_SQRT_2;
                f(&mut p, &[0.0, 0.0, 0.0, 1.0]);
                f(&mut p, &[0.0, 0.0, h, h]);
                p.extend_from_slice(&0u16.to_le_bytes());
            }
        }
        header(b"VMAN", 12, &p)
    }

    pub fn tex_bytes() -> Vec<u8> {
        let mut p = Vec::new();
        for v in [71u32, 4, 4, 3] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..3 {
            p.extend_from_slice(&8u32.to_le_bytes());
            p.extend_from_slice(&1u32.to_le_bytes());
            p.extend_from_slice(&8u32.to_le_bytes());
            p.extend_from_slice(&[0xFF; 8]);
        }
        header(b"VMTX", 3, &p)
    }

    pub fn bundle() -> (String, BTreeMap<String, Vec<u8>>) {
        let mut files = BTreeMap::new();
        files.insert(SKEL_FILE.to_string(), skel_bytes());
        files.insert(MESH_FILE.to_string(), mesh_bytes());
        files.insert(ANIM_FILE.to_string(), anim_bytes());
        files.insert("t.vmtex".to_string(), tex_bytes());
        let entries: Vec<String> = files
            .iter()
            .map(|(n, d)| format!("\"{n}\": {{\"sha256\": \"{}\", \"size\": {}}}", sha256_hex(d), d.len()))
            .collect();
        let man = format!(
            "{{\"format_version\": 1, \"converter\": \"test\", \"files\": {{{}}}, \"weapon_times\": {{\"reloadTime\": 2.5}}}}",
            entries.join(", ")
        );
        (man, files)
    }

    pub fn assets() -> VmAssets {
        let (m, f) = bundle();
        VmAssets::from_files(&m, &f).expect("test bundle parses")
    }
}

#[cfg(test)]
mod tests {
    use super::testdata::*;
    use super::*;

    #[test]
    fn vm_assets_bundle_parses() {
        let a = assets();
        assert_eq!(a.skel.bones.len(), 9);
        assert_eq!(a.skel.tags.view, 1);
        assert_eq!(a.mesh.vertex_count, 6);
        assert_eq!(a.mesh.triangles(0), 1);
        assert_eq!(a.mesh.triangles(1), 1);
        assert_eq!(a.clips.len(), N_CLIPS, "12 base clips + 7 M6 roles (fallbacks)");
        assert!(a.has(ClipId::Reload) && !a.has(ClipId::Rechamber));
        assert_eq!(a.clip(ClipId::Rechamber).name, "idle", "rechamber falls back to idle");
        assert_eq!(a.clip(ClipId::FirstRaise).name, "pullout");
        assert_eq!(a.clip(ClipId::Reload).name, "reload");
        assert_eq!(a.textures[0].mips.len(), 3);
        assert_eq!(a.weapon_times.reload, 2.5);
        assert_eq!(a.weapon_times.reload_empty, 3.03, "missing times fall back to the UMP45 values");
    }

    #[test]
    fn vm_assets_truncation_never_panics() {
        // Truncating each file at every 97th byte (and at the last byte) returns Err, never panics.
        for data in [skel_bytes(), mesh_bytes(), anim_bytes(), tex_bytes()] {
            let mut cuts: Vec<usize> = (0..data.len()).step_by(97).collect();
            cuts.push(data.len() - 1);
            for cut in cuts {
                let d = &data[..cut];
                let r = match &data[..4] {
                    b"VMSK" => parse_skel(d).map(|_| ()),
                    b"VMME" => parse_mesh(d, 9).map(|_| ()),
                    b"VMAN" => parse_anim(d, 9).map(|_| ()),
                    _ => parse_tex(d).map(|_| ()),
                };
                assert!(r.is_err(), "cut at {cut} accepted");
            }
        }
    }

    #[test]
    fn vm_assets_corrupt_payload_rejected() {
        // payload size field lying about the size (a truncated file whose header was patched)
        let mut s = skel_bytes();
        s.truncate(s.len() - 2);
        let plen = (s.len() - HEADER_SIZE) as u64;
        s[16..24].copy_from_slice(&plen.to_le_bytes());
        assert!(parse_skel(&s).is_err());
        // version 2 header
        let mut m = mesh_bytes();
        m[4] = 2;
        assert!(parse_mesh(&m, 9).unwrap_err().contains("version"));
        // a joint index beyond the skeleton
        assert!(parse_mesh(&mesh_bytes(), 5).unwrap_err().contains("joint"));
        // random garbage
        let g: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        assert!(parse_anim(&g, 9).is_err() && parse_tex(&g).is_err() && parse_skel(&g).is_err());
        // a huge count in an otherwise valid header does not allocate it
        let mut a = anim_bytes();
        a[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_anim(&a, 9).is_err());
    }

    #[test]
    fn vm_assets_manifest_hash_mismatch_rejected() {
        let (m, mut f) = bundle();
        let skel = f.get_mut(SKEL_FILE).unwrap();
        let last = skel.len() - 1;
        skel[last] ^= 1;
        let e = VmAssets::from_files(&m, &f).unwrap_err();
        assert!(e.contains("sha256"), "{e}");
        // a listed file that is missing
        let (m, mut f) = bundle();
        f.remove("t.vmtex");
        assert!(VmAssets::from_files(&m, &f).unwrap_err().contains("missing"));
        // wrong manifest version
        let (m, f) = bundle();
        let m2 = m.replace("\"format_version\": 1", "\"format_version\": 2");
        assert!(VmAssets::from_files(&m2, &f).unwrap_err().contains("format_version"));
        // missing folder
        assert!(VmAssets::load_dir(Path::new("Z:\\definitely\\not\\here")).is_err());
    }
}
