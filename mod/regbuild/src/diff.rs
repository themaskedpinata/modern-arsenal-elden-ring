//! The equivalence harness (`regbuild diff A B`): decrypts and unpacks two regulation.bin and compares them layer by
//! layer: container (IV, DCX header, zstd frame shape), BND4 (version, entries, names, ids, flags, hash table, data
//! layout), then every param: header fields, row ids in order, row data bytes (equal bytes = every field equal; on a
//! difference the fields are named from the field table, else byte offsets), row names. Optional rows.json comparison
//! without the volatile keys.

use crate::bnd4::Bnd4;
use crate::param::Param;
use crate::{dcx, fields};

#[derive(Clone, Debug, Default)]
pub struct DiffReport {
    pub lines: Vec<String>,
    pub params_total: usize,
    pub params_identical_bytes: usize,
    pub params_same_data: usize,
    /// params whose data (header fields, ids, row bytes, names) differ
    pub params_differ: Vec<String>,
    pub rows_compared: usize,
    pub bnd_identical: bool,
    pub bnd_header_same: bool,
}

impl DiffReport {
    pub fn equivalent(&self) -> bool {
        self.params_differ.is_empty() && self.bnd_header_same && self.params_total > 0
    }
}

fn container(tag: &str, u: &crate::Unpacked, out: &mut Vec<String>) {
    let d = &u.dcx_header;
    let be = |o: usize| u32::from_be_bytes(d[o..o + 4].try_into().unwrap());
    let ff = dcx::frame_facts(&u.frame);
    out.push(format!(
        "{tag}: {} B sha256 {} | IV {} | DCX {} level {} usize {} csize {} | zstd window {} content-size {} checksum {} | AES trailer {} B",
        u.file_len,
        u.file_sha256,
        if u.iv == [0; 16] { "zero".to_string() } else { u.iv.iter().map(|b| format!("{b:02x}")).collect() },
        String::from_utf8_lossy(&d[0x28..0x2C]),
        d[0x30],
        be(0x1C),
        be(0x20),
        ff.and_then(|f| f.0).map(|w| w.to_string()).unwrap_or("-".into()),
        ff.is_some_and(|f| f.1),
        ff.is_some_and(|f| f.2),
        u.trailer.len()
    ));
}

/// Compares one param pair that is not byte-identical; returns the difference lines ([] = same data).
pub fn compare_param(stem: &str, a: &Param, b: &Param, out_rows: &mut usize) -> Vec<String> {
    let mut d = Vec::new();
    if a.header_key() != b.header_key() {
        d.push(format!("header {:?} -> {:?}", a.header_key(), b.header_key()));
    }
    let ida: Vec<i32> = a.rows.iter().map(|r| r.id).collect();
    let idb: Vec<i32> = b.rows.iter().map(|r| r.id).collect();
    if ida != idb {
        let sa: std::collections::BTreeSet<i32> = ida.iter().copied().collect();
        let sb: std::collections::BTreeSet<i32> = idb.iter().copied().collect();
        let added: Vec<i32> = sb.difference(&sa).copied().collect();
        let removed: Vec<i32> = sa.difference(&sb).copied().collect();
        if !added.is_empty() {
            d.push(format!("rows only in B: {} {:?}", added.len(), &added[..added.len().min(20)]));
        }
        if !removed.is_empty() {
            d.push(format!("rows only in A: {} {:?}", removed.len(), &removed[..removed.len().min(20)]));
        }
        if added.is_empty() && removed.is_empty() {
            d.push("row order differs".into());
        }
    }
    let layout = fields::layout_of(stem).filter(|l| l.row_size == a.row_size);
    let mut by_b: std::collections::HashMap<i32, Vec<&crate::param::Row>> = std::collections::HashMap::new();
    for r in &b.rows {
        by_b.entry(r.id).or_default().push(r);
    }
    let mut seen_a: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    let (mut data_rows, mut name_rows) = (Vec::new(), Vec::new());
    let mut field_hits: Vec<(String, usize)> = Vec::new();
    for ra in &a.rows {
        let n = seen_a.entry(ra.id).or_default();
        let Some(rb) = by_b.get(&ra.id).and_then(|v| v.get(*n)) else { continue };
        *n += 1;
        *out_rows += 1;
        if ra.data != rb.data {
            data_rows.push(ra.id);
            for k in 0..ra.data.len().min(rb.data.len()) {
                if ra.data[k] != rb.data[k] {
                    let names = layout.map(|l| l.fields_at(k)).unwrap_or_default();
                    let key = if names.is_empty() { format!("+0x{k:03x}") } else { names.join("/") };
                    match field_hits.iter_mut().find(|x| x.0 == key) {
                        Some(x) => x.1 += 1,
                        None => field_hits.push((key, 1)),
                    }
                }
            }
        }
        if ra.name.as_deref().unwrap_or("") != rb.name.as_deref().unwrap_or("") {
            name_rows.push(ra.id);
        }
    }
    if !data_rows.is_empty() {
        d.push(format!(
            "row data differs in {} rows {:?}; fields: {}",
            data_rows.len(),
            &data_rows[..data_rows.len().min(10)],
            field_hits.iter().take(12).map(|(k, n)| format!("{k} x{n}")).collect::<Vec<_>>().join(", ")
        ));
    }
    if !name_rows.is_empty() {
        d.push(format!("row names differ in {} rows {:?}", name_rows.len(), &name_rows[..name_rows.len().min(10)]));
    }
    d
}

/// Decrypts + unpacks both files and compares them.
pub fn diff(a_file: &[u8], b_file: &[u8]) -> Result<DiffReport, String> {
    let ua = crate::unpack(a_file).map_err(|e| format!("A: {e}"))?;
    let ub = crate::unpack(b_file).map_err(|e| format!("B: {e}"))?;
    let mut r = DiffReport::default();
    container("A", &ua, &mut r.lines);
    container("B", &ub, &mut r.lines);
    let dh = |u: &crate::Unpacked| {
        let mut h = u.dcx_header;
        h[0x1C..0x24].fill(0);
        h
    };
    r.lines.push(format!("DCX header (without sizes) identical: {}", dh(&ua) == dh(&ub)));
    let (ba, bb) = (Bnd4::parse(&ua.bnd).map_err(|e| format!("A: {e}"))?, Bnd4::parse(&ub.bnd).map_err(|e| format!("B: {e}"))?);
    r.bnd_identical = ua.bnd == ub.bnd;
    r.bnd_header_same = ba.header_without_layout() == bb.header_without_layout() && ba.version == bb.version;
    r.lines.push(format!(
        "BND4: A {} B ({} files, version {}) B {} B ({} files, version {}); header/names/ids/flags/hash table identical: {}; canonical 16-byte data chain: A {} B {}; whole BND4 byte-identical: {}",
        ua.bnd.len(),
        ba.entries.len(),
        ba.version,
        ub.bnd.len(),
        bb.entries.len(),
        bb.version,
        r.bnd_header_same,
        ba.canonical_layout(),
        bb.canonical_layout(),
        r.bnd_identical
    ));
    let names_a: Vec<&str> = ba.entries.iter().map(|e| e.file_name()).collect();
    let names_b: Vec<&str> = bb.entries.iter().map(|e| e.file_name()).collect();
    for n in names_a.iter().filter(|n| !names_b.contains(n)) {
        r.params_differ.push(n.to_string());
        r.lines.push(format!("  {n}: only in A"));
    }
    for n in names_b.iter().filter(|n| !names_a.contains(n)) {
        r.params_differ.push(n.to_string());
        r.lines.push(format!("  {n}: only in B"));
    }
    for (ka, name) in names_a.iter().enumerate() {
        let Some(kb) = names_b.iter().position(|n| n == name) else { continue };
        r.params_total += 1;
        let stem = name.trim_end_matches(".param");
        let (da, db) = (ba.data(ka), bb.data(kb));
        if da == db {
            r.params_identical_bytes += 1;
            r.params_same_data += 1;
            if let Ok(p) = Param::parse(da) {
                r.rows_compared += p.rows.len();
            }
            continue;
        }
        match (Param::parse(da), Param::parse(db)) {
            (Ok(pa), Ok(pb)) => {
                let d = compare_param(stem, &pa, &pb, &mut r.rows_compared);
                if d.is_empty() {
                    r.params_same_data += 1;
                    let mut notes = Vec::new();
                    if pa.strings_offset != pb.strings_offset {
                        notes.push("strings-offset header field");
                    }
                    if da.len() != db.len() {
                        notes.push("file size");
                    }
                    r.lines.push(format!("  {stem}: same data, layout only ({})", notes.join(", ")));
                } else {
                    r.params_differ.push(stem.to_string());
                    r.lines.push(format!("  {stem}: DIFFERS: {}", d.join("; ")));
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                r.params_differ.push(stem.to_string());
                r.lines.push(format!("  {stem}: parse error {e}"));
            }
        }
    }
    r.lines.push(format!(
        "params: {} compared, {} byte-identical, {} same data, {} differ; {} rows compared",
        r.params_total,
        r.params_identical_bytes,
        r.params_same_data,
        r.params_differ.len(),
        r.rows_compared
    ));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bnd4, dcx, param};

    fn tmpl() -> [u8; dcx::HEADER_LEN] {
        let mut t = [0u8; dcx::HEADER_LEN];
        t[..4].copy_from_slice(b"DCX\0");
        for (o, v) in [(4usize, 0x11000u32), (8, 0x18), (0xC, 0x24), (0x10, 0x44), (0x14, 0x4C), (0x2C, 0x20), (0x48, 8)] {
            t[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        t[0x18..0x1C].copy_from_slice(b"DCS\0");
        t[0x24..0x2C].copy_from_slice(b"DCP\0ZSTD");
        t[0x30] = 21;
        t[0x44..0x48].copy_from_slice(b"DCA\0");
        t
    }

    fn reg(bullet_row: Vec<u8>) -> Vec<u8> {
        let b = param::build_fixture("BULLET_PARAM_ST", 272, &[(1, "", vec![0; 272]), (2, "", bullet_row)]);
        let o = param::build_fixture("OTHER_ST", 4, &[(1, "", vec![9; 4])]);
        let bnd = bnd4::build_fixture("11730034", &[("Bullet.param", &b), ("Other.param", &o)]);
        let frame = dcx::compress_level(&bnd, 1).unwrap();
        crate::crypto::encrypt(&dcx::write(&tmpl(), bnd.len(), &frame).unwrap())
    }

    #[test]
    fn identical_and_field_level_differences() {
        let a = reg(vec![0; 272]);
        let r = diff(&a, &a).unwrap();
        assert!(r.equivalent() && r.bnd_identical);
        assert_eq!((r.params_total, r.params_identical_bytes), (2, 2));
        let mut row = vec![0u8; 272];
        crate::fields::write(&mut row, crate::fields::BULLET.field("life").unwrap(), crate::fields::V::F(0.1)).unwrap();
        let b = reg(row);
        let r = diff(&a, &b).unwrap();
        assert!(!r.equivalent());
        assert_eq!(r.params_differ, ["Bullet"]);
        assert!(r.lines.iter().any(|l| l.contains("life x")), "{:#?}", r.lines);
    }
}
