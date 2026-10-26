//! Elden Ring PARAM files (SoulsFormats layout, Format2D 0x85: offset param type + long offsets; Format2E 0x07: UTF-16
//! names), read generically and written for the 3 params we add rows to.
//!
//! Header 0x40: u32 strings offset, u16 unk04, i16 unk06, i16 paramdef data version, u16 row count, i32 0, i64 type
//! string offset, zeros, 0x2C: 00 (little-endian), Format2D, Format2E, paramdef format version, 0x30: i64 data start.
//! Row table 0x18 each (i32 id, i32 0, i64 data offset, i64 name offset), sorted by id; the row data is contiguous;
//! then the ASCII type string + NUL, the UTF-16 names (each NUL-terminated, deduplicated: one copy per distinct
//! string, in first-use row order) and 2 zero bytes. Untouched params are never re-serialised (56 vanilla params carry
//! header quirks a writer would not reproduce): they are copied as raw bytes.

use crate::bnd4::utf16z;

#[derive(Clone, Debug)]
pub struct Row<'a> {
    pub id: i32,
    /// None = a null name offset (never in the regulation's edited params; SoulsFormats writes it back as "").
    pub name: Option<String>,
    pub data: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct Param<'a> {
    pub raw: &'a [u8],
    pub param_type: String,
    pub strings_offset: u32,
    pub unk04: u16,
    pub unk06: i16,
    pub paramdef_data_version: i16,
    pub format2d: u8,
    pub format2e: u8,
    pub paramdef_format_version: u8,
    pub type_offset: usize,
    pub data_start: usize,
    pub row_size: usize,
    pub rows: Vec<Row<'a>>,
}

fn rd<const N: usize>(b: &[u8], o: usize) -> Result<[u8; N], String> {
    b.get(o..o + N).map(|s| s.try_into().unwrap()).ok_or_else(|| format!("PARAM truncated at 0x{o:x}"))
}

impl<'a> Param<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < 0x40 {
            return Err("PARAM too small".into());
        }
        if b[0x2C] != 0 {
            return Err("big-endian PARAM not supported".into());
        }
        let (f2d, f2e, pdfv) = (b[0x2D], b[0x2E], b[0x2F]);
        if f2d & 0x80 == 0 || f2d & 0x04 == 0 {
            return Err(format!("unsupported Format2D 0x{f2d:02x} (want offset param type + long offsets)"));
        }
        let strings_offset = u32::from_le_bytes(rd(b, 0)?);
        let unk04 = u16::from_le_bytes(rd(b, 4)?);
        let unk06 = i16::from_le_bytes(rd(b, 6)?);
        let pdver = i16::from_le_bytes(rd(b, 8)?);
        let nrows = u16::from_le_bytes(rd(b, 0xA)?) as usize;
        let type_offset = i64::from_le_bytes(rd(b, 0x10)?) as usize;
        let data_start = i64::from_le_bytes(rd(b, 0x30)?) as usize;
        let tend = b.get(type_offset..).and_then(|s| s.iter().position(|&c| c == 0)).ok_or("PARAM type string out of range")?;
        let param_type = String::from_utf8_lossy(&b[type_offset..type_offset + tend]).into_owned();
        let mut table = Vec::with_capacity(nrows);
        for i in 0..nrows {
            let e = 0x40 + i * 0x18;
            let id = i32::from_le_bytes(rd(b, e)?);
            let doff = i64::from_le_bytes(rd(b, e + 8)?) as usize;
            let noff = i64::from_le_bytes(rd(b, e + 16)?) as usize;
            table.push((id, doff, noff));
        }
        let data_end = table.iter().map(|r| r.2).filter(|&n| n > 0).chain([type_offset]).min().unwrap();
        let stride = match nrows {
            0 => 0,
            1 => data_end.checked_sub(table[0].1).ok_or("PARAM row data after the strings")?,
            _ => table[1].1.wrapping_sub(table[0].1),
        };
        for w in table.windows(2) {
            if w[1].1.wrapping_sub(w[0].1) != stride {
                return Err("PARAM: non-uniform row stride".into());
            }
        }
        if nrows > 0 && table[0].1 != data_start {
            return Err(format!("PARAM: first row data 0x{:x} != data start 0x{data_start:x}", table[0].1));
        }
        if nrows > 0 && table[nrows - 1].1 + stride > data_end {
            return Err("PARAM: row data overlaps the strings".into());
        }
        let mut rows = Vec::with_capacity(nrows);
        for (id, doff, noff) in table {
            let name = if noff == 0 {
                None
            } else if f2e & 1 != 0 {
                Some(utf16z(b, noff)?)
            } else {
                let end = b.get(noff..).and_then(|s| s.iter().position(|&c| c == 0)).ok_or("PARAM name out of range")?;
                Some(String::from_utf8_lossy(&b[noff..noff + end]).into_owned())
            };
            rows.push(Row { id, name, data: &b[doff..doff + stride] });
        }
        Ok(Param {
            raw: b,
            param_type,
            strings_offset,
            unk04,
            unk06,
            paramdef_data_version: pdver,
            format2d: f2d,
            format2e: f2e,
            paramdef_format_version: pdfv,
            type_offset,
            data_start,
            row_size: stride,
            rows,
        })
    }

    pub fn row(&self, id: i32) -> Option<&Row<'a>> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// The header facts compared by the harness and `check_added` (param_diff.HEADER_KEYS).
    pub fn header_key(&self) -> (String, u16, i16, i16, u8, u8, u8, usize) {
        (
            self.param_type.clone(),
            self.unk04,
            self.unk06,
            self.paramdef_data_version,
            self.format2d,
            self.format2e,
            self.paramdef_format_version,
            self.row_size,
        )
    }
}

/// One new row: id, name, data (exactly `row_size` bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct NewRow {
    pub id: i32,
    pub name: String,
    pub data: Vec<u8>,
}

/// Writes `rows` (id, name, data) with `p`'s header: the SoulsFormats writer as WitchyBND runs it (strings offset field =
/// type string offset, deduplicated names, 2 trailing zero bytes).
pub fn write(p: &Param, rows: &[(i32, &str, &[u8])]) -> Result<Vec<u8>, String> {
    if p.format2e & 1 == 0 {
        return Err(format!("{}: non-unicode names are not supported by the writer", p.param_type));
    }
    let n = rows.len();
    let count = u16::try_from(n).map_err(|_| format!("{}: {n} rows > 65535", p.param_type))?;
    let size = p.row_size;
    let data_start = 0x40 + n * 0x18;
    let type_off = data_start + n * size;
    let mut strings = Vec::with_capacity(p.param_type.len() + 1 + n * 2);
    strings.extend_from_slice(p.param_type.as_bytes());
    strings.push(0);
    let mut name_off: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut out = Vec::with_capacity(type_off + strings.len() + 4096);
    out.extend_from_slice(&p.raw[..0x40]);
    for (i, (id, name, data)) in rows.iter().enumerate() {
        if data.len() != size {
            return Err(format!("{} row {id}: {} bytes (row size {size})", p.param_type, data.len()));
        }
        let no = *name_off.entry(name).or_insert_with(|| {
            let at = type_off + strings.len();
            for c in name.encode_utf16().chain([0]) {
                strings.extend_from_slice(&c.to_le_bytes());
            }
            at
        });
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&((data_start + i * size) as i64).to_le_bytes());
        out.extend_from_slice(&(no as i64).to_le_bytes());
    }
    for (_, _, data) in rows {
        out.extend_from_slice(data);
    }
    out.extend_from_slice(&strings);
    out.extend_from_slice(&[0, 0]);
    let type_u32 = u32::try_from(type_off).map_err(|_| "PARAM > 4 GB")?;
    out[0..4].copy_from_slice(&type_u32.to_le_bytes());
    out[0xA..0xC].copy_from_slice(&count.to_le_bytes());
    out[0x10..0x18].copy_from_slice(&(type_off as i64).to_le_bytes());
    out[0x30..0x38].copy_from_slice(&(data_start as i64).to_le_bytes());
    Ok(out)
}

/// `p`'s rows with `new` (sorted by id) inserted as one block before the first vanilla row whose id is above the
/// largest new id (build_regulation.edit_xml's rule), written with [`write`].
pub fn insert_rows(p: &Param, new: &[NewRow]) -> Result<Vec<u8>, String> {
    if new.is_empty() {
        let rows: Vec<(i32, &str, &[u8])> = p.rows.iter().map(|r| (r.id, r.name.as_deref().unwrap_or(""), r.data)).collect();
        return write(p, &rows);
    }
    if new.windows(2).any(|w| w[0].id > w[1].id) {
        return Err(format!("{}: the planned rows are not sorted", p.param_type));
    }
    let hi = new.last().unwrap().id;
    let ins = p.rows.iter().position(|r| r.id > hi).unwrap_or(p.rows.len());
    let mut rows: Vec<(i32, &str, &[u8])> = Vec::with_capacity(p.rows.len() + new.len());
    rows.extend(p.rows[..ins].iter().map(|r| (r.id, r.name.as_deref().unwrap_or(""), r.data)));
    rows.extend(new.iter().map(|r| (r.id, r.name.as_str(), r.data.as_slice())));
    rows.extend(p.rows[ins..].iter().map(|r| (r.id, r.name.as_deref().unwrap_or(""), r.data)));
    write(p, &rows)
}

/// A PARAM of the regulation's shape (tests and fixtures): unk06 0, data version 4, Format2D 0x85, Format2E 0x07.
pub fn build_fixture(param_type: &str, row_size: usize, rows: &[(i32, &str, Vec<u8>)]) -> Vec<u8> {
    let mut h = vec![0u8; 0x40];
    h[8..10].copy_from_slice(&4i16.to_le_bytes());
    h[0x2D] = 0x85;
    h[0x2E] = 0x07;
    // a header-only template: the writer fills offsets and the count
    h.extend_from_slice(param_type.as_bytes());
    h.push(0);
    h[0x10..0x18].copy_from_slice(&0x40i64.to_le_bytes());
    let tmpl = Param {
        raw: &h,
        param_type: param_type.into(),
        strings_offset: 0,
        unk04: 0,
        unk06: 0,
        paramdef_data_version: 4,
        format2d: 0x85,
        format2e: 0x07,
        paramdef_format_version: 0,
        type_offset: 0x40,
        data_start: 0x40,
        row_size,
        rows: vec![],
    };
    write(&tmpl, &rows.iter().map(|(i, n, d)| (*i, *n, d.as_slice())).collect::<Vec<_>>()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        build_fixture("TEST_ST", 8, &[(1, "", vec![1; 8]), (5, "", vec![5; 8]), (90, "", vec![9; 8])])
    }

    #[test]
    fn parse_and_no_edit_round_trip() {
        let raw = fixture();
        let p = Param::parse(&raw).unwrap();
        assert_eq!(p.param_type, "TEST_ST");
        assert_eq!(p.row_size, 8);
        assert_eq!(p.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 5, 90]);
        assert_eq!(p.rows[1].data, &[5u8; 8]);
        assert_eq!(p.rows[2].name.as_deref(), Some(""));
        assert_eq!(p.strings_offset as usize, p.type_offset);
        assert_eq!(insert_rows(&p, &[]).unwrap(), raw);
        // one shared empty name, then the 2-byte tail
        assert_eq!(raw.len(), 0x40 + 3 * 0x18 + 24 + 8 + 2 + 2);
    }

    #[test]
    fn inserts_a_sorted_block_with_deduplicated_names() {
        let raw = fixture();
        let p = Param::parse(&raw).unwrap();
        let new = vec![
            NewRow { id: 10, name: "MW2 a".into(), data: vec![0xA; 8] },
            NewRow { id: 11, name: "MW2 b".into(), data: vec![0xB; 8] },
            NewRow { id: 12, name: "MW2 a".into(), data: vec![0xC; 8] },
        ];
        let out = insert_rows(&p, &new).unwrap();
        let q = Param::parse(&out).unwrap();
        assert_eq!(q.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 5, 10, 11, 12, 90]);
        assert_eq!(q.rows[4].name.as_deref(), Some("MW2 a"));
        assert_eq!(q.rows[5].data, &[9u8; 8]);
        assert_eq!(q.header_key(), p.header_key());
        // names: "" , "MW2 a", "MW2 b" (3 distinct strings) + tail
        let names_len = 2 + 12 + 12;
        assert_eq!(out.len(), 0x40 + 6 * 0x18 + 6 * 8 + 8 + names_len + 2);
        let unsorted = vec![new[1].clone(), new[0].clone()];
        assert!(insert_rows(&p, &unsorted).is_err());
        let bad = vec![NewRow { id: 10, name: String::new(), data: vec![0; 7] }];
        assert!(insert_rows(&p, &bad).is_err());
    }

    #[test]
    fn refuses_bad_files() {
        let raw = fixture();
        let mut r = raw.clone();
        r[0x2D] = 0x05;
        assert!(Param::parse(&r).is_err());
        assert!(Param::parse(&raw[..0x20]).is_err());
    }
}
