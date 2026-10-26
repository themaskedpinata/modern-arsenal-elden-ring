//! BND4 container (little-endian), the subset the ER regulation uses: format 0x74 (IDs | Names1 | Names2 | Compression
//! field), entry header 0x24, UTF-16 names, uncompressed files.
//!
//! Layout: header 0x40 (`BND4`, count @0x0C, version @0x18, entry size @0x20, headers end @0x28, unicode @0x30,
//! format @0x31, extended @0x32, hash table @0x38), entries (flags u8, 3 pad, -1, csize i64, usize i64, data offset u32,
//! id i32, name offset u32), names, the hash table, then the data: the first file at headers end, each next one at the
//! end of the previous rounded up to 16, zero padding, nothing after the last file. The writer keeps every header byte
//! (names, ids, flags, hash table) and changes only the sizes and data offsets, as WitchyBND does.

pub const ENTRY_SIZE: usize = 0x24;
pub const FORMAT: u8 = 0x74;
pub const DATA_ALIGN: usize = 16;

#[derive(Clone, Debug)]
pub struct Entry {
    pub flags: u8,
    pub id: i32,
    pub name: String,
    pub offset: usize,
    pub size: usize,
}

impl Entry {
    /// The file name after the last `\` (e.g. `Bullet.param`).
    pub fn file_name(&self) -> &str {
        self.name.rsplit(['\\', '/']).next().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct Bnd4<'a> {
    pub raw: &'a [u8],
    pub version: String,
    pub headers_end: usize,
    pub entries: Vec<Entry>,
}

fn u32le(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| format!("BND4 truncated at 0x{o:x}"))
}

fn i64le(b: &[u8], o: usize) -> Result<i64, String> {
    b.get(o..o + 8).map(|s| i64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| format!("BND4 truncated at 0x{o:x}"))
}

/// A NUL-terminated UTF-16LE string at `o`.
pub fn utf16z(b: &[u8], o: usize) -> Result<String, String> {
    let mut u = Vec::new();
    let mut i = o;
    loop {
        let c = b.get(i..i + 2).ok_or_else(|| format!("unterminated UTF-16 string at 0x{o:x}"))?;
        let c = u16::from_le_bytes([c[0], c[1]]);
        if c == 0 {
            break;
        }
        u.push(c);
        i += 2;
    }
    String::from_utf16(&u).map_err(|_| format!("bad UTF-16 string at 0x{o:x}"))
}

impl<'a> Bnd4<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < 0x40 || &b[0..4] != b"BND4" {
            return Err("not a BND4".into());
        }
        let count = u32le(b, 0x0C)? as usize;
        let entry_size = i64le(b, 0x20)?;
        let headers_end = i64le(b, 0x28)? as usize;
        let (unicode, format) = (b[0x30], b[0x31]);
        if entry_size != ENTRY_SIZE as i64 || format != FORMAT || unicode != 1 || i64le(b, 0x10)? != 0x40 {
            return Err(format!("unsupported BND4 layout (entry size {entry_size}, format 0x{format:02x}, unicode {unicode})"));
        }
        if headers_end > b.len() || 0x40 + count * ENTRY_SIZE > headers_end {
            return Err("BND4 headers end out of range".into());
        }
        let version = String::from_utf8_lossy(&b[0x18..0x20]).trim_end_matches('\0').to_string();
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let e = 0x40 + i * ENTRY_SIZE;
            let flags = b[e];
            let csize = i64le(b, e + 8)?;
            let usize = i64le(b, e + 0x10)?;
            let offset = u32le(b, e + 0x18)? as usize;
            let id = u32le(b, e + 0x1C)? as i32;
            let name = utf16z(b, u32le(b, e + 0x20)? as usize)?;
            if flags & 0x80 != 0 || csize != usize || csize < 0 {
                return Err(format!("BND4 file {name}: compressed entries are not supported (flags 0x{flags:02x})"));
            }
            let size = csize as usize;
            if offset < headers_end || offset + size > b.len() {
                return Err(format!("BND4 file {name}: data out of range"));
            }
            entries.push(Entry { flags, id, name, offset, size });
        }
        Ok(Bnd4 { raw: b, version, headers_end, entries })
    }

    pub fn data(&self, i: usize) -> &'a [u8] {
        let e = &self.entries[i];
        &self.raw[e.offset..e.offset + e.size]
    }

    pub fn find(&self, file_name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.file_name() == file_name)
    }

    /// The header bytes with the per-file sizes and data offsets zeroed: equal for two BND4 that differ only in their
    /// file contents (the harness and the verify use it for "manifest unchanged").
    pub fn header_without_layout(&self) -> Vec<u8> {
        let mut h = self.raw[..self.headers_end].to_vec();
        for i in 0..self.entries.len() {
            let e = 0x40 + i * ENTRY_SIZE;
            h[e + 8..e + 0x1C].fill(0);
        }
        h
    }

    /// True when the data is laid out as this writer lays it out (16-byte chain from headers end, zero padding, no
    /// trailing bytes): the condition under which `write` reproduces an unedited BND4 byte for byte.
    pub fn canonical_layout(&self) -> bool {
        let mut pos = self.headers_end;
        for e in &self.entries {
            let at = pos.next_multiple_of(DATA_ALIGN);
            if e.offset != at || self.raw[pos..at].iter().any(|&b| b != 0) {
                return false;
            }
            pos = at + e.size;
        }
        pos == self.raw.len()
    }

    /// A new BND4 with this one's header and `files` (one per entry, in entry order) as the data.
    pub fn write(&self, files: &[&[u8]]) -> Result<Vec<u8>, String> {
        if files.len() != self.entries.len() {
            return Err(format!("BND4 write: {} files for {} entries", files.len(), self.entries.len()));
        }
        let total = files.iter().map(|f| f.len() + DATA_ALIGN).sum::<usize>() + self.headers_end;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&self.raw[..self.headers_end]);
        for (i, f) in files.iter().enumerate() {
            let at = out.len().next_multiple_of(DATA_ALIGN);
            out.resize(at, 0);
            let off = u32::try_from(at).map_err(|_| "BND4 write: data past 4 GB")?;
            let e = 0x40 + i * ENTRY_SIZE;
            out[e + 8..e + 0x10].copy_from_slice(&(f.len() as i64).to_le_bytes());
            out[e + 0x10..e + 0x18].copy_from_slice(&(f.len() as i64).to_le_bytes());
            out[e + 0x18..e + 0x1C].copy_from_slice(&off.to_le_bytes());
            out.extend_from_slice(f);
        }
        Ok(out)
    }
}

/// A minimal BND4 of the regulation's shape (tests and fixtures): names `N:\GR\data\Param\param\GameParam\<n>`,
/// ids 0.., flags 0x40, an empty hash table area of 16 bytes.
pub fn build_fixture(version: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let n = files.len();
    let mut names = Vec::new();
    let mut name_offs = Vec::new();
    let names_start = 0x40 + n * ENTRY_SIZE;
    for (name, _) in files {
        name_offs.push(names_start + names.len());
        for c in format!("N:\\GR\\data\\Param\\param\\GameParam\\{name}").encode_utf16().chain([0]) {
            names.extend_from_slice(&c.to_le_bytes());
        }
    }
    let hash_off = names_start + names.len();
    let headers_end = hash_off + 16;
    let mut h = vec![0u8; 0x40];
    h[0..4].copy_from_slice(b"BND4");
    h[0x0A] = 1;
    h[0x0C..0x10].copy_from_slice(&(n as u32).to_le_bytes());
    h[0x10..0x18].copy_from_slice(&0x40i64.to_le_bytes());
    let v = version.as_bytes();
    h[0x18..0x18 + v.len().min(8)].copy_from_slice(&v[..v.len().min(8)]);
    h[0x20..0x28].copy_from_slice(&(ENTRY_SIZE as i64).to_le_bytes());
    h[0x28..0x30].copy_from_slice(&(headers_end as i64).to_le_bytes());
    h[0x30] = 1;
    h[0x31] = FORMAT;
    h[0x32] = 4;
    h[0x38..0x40].copy_from_slice(&(hash_off as i64).to_le_bytes());
    for (i, _) in files.iter().enumerate() {
        let mut e = [0u8; ENTRY_SIZE];
        e[0] = 0x40;
        e[4..8].copy_from_slice(&(-1i32).to_le_bytes());
        e[0x1C..0x20].copy_from_slice(&(i as i32).to_le_bytes());
        e[0x20..0x24].copy_from_slice(&(name_offs[i] as u32).to_le_bytes());
        h.extend_from_slice(&e);
    }
    h.extend_from_slice(&names);
    h.resize(headers_end, 0);
    let tmp = Bnd4 {
        raw: &h,
        version: version.into(),
        headers_end,
        entries: files.iter().map(|(n, _)| Entry { flags: 0x40, id: 0, name: (*n).into(), offset: 0, size: 0 }).collect(),
    };
    tmp.write(&files.iter().map(|f| f.1).collect::<Vec<_>>()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_parses_and_round_trips() {
        let a = b"first file".to_vec();
        let b2 = vec![7u8; 33];
        let raw = build_fixture("11730034", &[("A.param", &a), ("B.param", &b2), ("C.param", b"")]);
        let p = Bnd4::parse(&raw).unwrap();
        assert_eq!(p.version, "11730034");
        assert_eq!(p.entries.len(), 3);
        assert_eq!(p.entries[1].file_name(), "B.param");
        assert_eq!(p.data(0), &a[..]);
        assert_eq!(p.data(1), &b2[..]);
        assert!(p.canonical_layout());
        assert_eq!(p.entries[1].offset % 16, 0);
        // no-edit rebuild is byte-identical
        let files: Vec<&[u8]> = (0..3).map(|i| p.data(i)).collect();
        assert_eq!(p.write(&files).unwrap(), raw);
        // an edit changes only sizes / offsets in the header
        let big = vec![1u8; 100];
        let raw2 = p.write(&[&big, &b2, b""]).unwrap();
        let p2 = Bnd4::parse(&raw2).unwrap();
        assert_eq!(p2.data(0), &big[..]);
        assert_eq!(p2.data(1), &b2[..]);
        assert_eq!(p2.header_without_layout(), p.header_without_layout());
        assert!(p2.canonical_layout());
        assert!(p.write(&[&big]).is_err());
    }

    #[test]
    fn refuses_unknown_layouts() {
        let raw = build_fixture("1", &[("A.param", b"x")]);
        let mut r = raw.clone();
        r[0x31] = 0x54;
        assert!(Bnd4::parse(&r).is_err());
        let mut r = raw.clone();
        r[0x40] = 0xC0; // compressed flag
        assert!(Bnd4::parse(&r).is_err());
        assert!(Bnd4::parse(b"BND3").is_err());
    }
}
