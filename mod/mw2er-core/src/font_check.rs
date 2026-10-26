//! Structure check of a user-supplied font (`hud.font_path`) before it reaches ImGui.
//!
//! ImGui's font loader (stb_truetype) asserts on fonts it cannot parse, and its asserts are compiled
//! into mw2er.dll: a failed assert calls `abort()`, which ends the game and cannot be caught. This
//! check mirrors what stb_truetype's `stbtt_GetFontOffsetForIndex` / `stbtt_InitFont` require
//! (sfnt magic, an in-bounds table directory, the cmap/head/hhea/hmtx tables, glyf+loca or CFF,
//! and a Unicode cmap subtable), so an empty file, a wrong file, a collection (.ttc) or a WOFF falls
//! back to the built-in font instead.
//!
//! It is not a full validation: a font with a valid directory but corrupt glyph data can still make
//! stb_truetype misbehave (it documents no security guarantee for untrusted fonts). Use a normal
//! TTF/OTF file from the system fonts folder.

/// Fonts larger than this are refused (ImGui keeps a copy of the data for every atlas build).
pub const MAX_FONT_BYTES: usize = 16 * 1024 * 1024;

fn be16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// `(offset, length)` of a table, checked to lie inside the file.
fn table(d: &[u8], num_tables: usize, tag: &[u8; 4]) -> Result<Option<(usize, usize)>, String> {
    for i in 0..num_tables {
        let rec = 12 + 16 * i;
        if &d[rec..rec + 4] == tag {
            let off = be32(d, rec + 8).ok_or("truncated table record")? as usize;
            let len = be32(d, rec + 12).ok_or("truncated table record")? as usize;
            let end = off.checked_add(len).ok_or("table size overflow")?;
            if end > d.len() {
                return Err(format!("table '{}' lies outside the file", String::from_utf8_lossy(tag)));
            }
            return Ok(Some((off, len)));
        }
    }
    Ok(None)
}

/// `Ok(kind)` ("TrueType" / "OpenType CFF") when ImGui can load the font, else the reason.
pub fn check_font(d: &[u8]) -> Result<&'static str, String> {
    if d.is_empty() {
        return Err("empty file".into());
    }
    if d.len() > MAX_FONT_BYTES {
        return Err(format!("{} bytes (max {MAX_FONT_BYTES})", d.len()));
    }
    if d.len() < 12 {
        return Err("too short for a font header".into());
    }
    match &d[0..4] {
        [0, 1, 0, 0] | b"true" | b"OTTO" => {}
        b"ttcf" => return Err("font collection (.ttc) is not supported; use a single .ttf/.otf".into()),
        b"wOFF" | b"wOF2" => return Err("WOFF is not supported; use a .ttf/.otf".into()),
        _ => return Err("not a TrueType/OpenType font (bad magic)".into()),
    }
    let num_tables = be16(d, 4).ok_or("truncated header")? as usize;
    if num_tables == 0 || 12 + 16 * num_tables > d.len() {
        return Err(format!("table directory ({num_tables} tables) does not fit the file"));
    }
    let req = |tag: &[u8; 4]| -> Result<(usize, usize), String> {
        table(d, num_tables, tag)?.ok_or_else(|| format!("required table '{}' missing", String::from_utf8_lossy(tag)))
    };
    let (cmap, cmap_len) = req(b"cmap")?;
    let (_, head_len) = req(b"head")?;
    let (_, hhea_len) = req(b"hhea")?;
    req(b"hmtx")?;
    if head_len < 54 {
        return Err("head table too short".into());
    }
    if hhea_len < 36 {
        return Err("hhea table too short".into());
    }
    let kind = match (table(d, num_tables, b"glyf")?, table(d, num_tables, b"loca")?, table(d, num_tables, b"CFF ")?) {
        (Some(_), Some(_), _) => "TrueType",
        (Some(_), None, _) => return Err("glyf table without loca".into()),
        (None, _, Some(_)) => "OpenType CFF",
        (None, _, None) => return Err("no glyph outlines (neither glyf+loca nor CFF)".into()),
    };
    if let Some((_, maxp_len)) = table(d, num_tables, b"maxp")?
        && maxp_len < 6
    {
        return Err("maxp table too short".into());
    }
    // cmap: a Unicode subtable (platform 0, or Microsoft 3 with encoding 1 / 10), inside the table.
    if cmap_len < 4 {
        return Err("cmap table too short".into());
    }
    let n = be16(d, cmap + 2).ok_or("truncated cmap")? as usize;
    if 4 + 8 * n > cmap_len {
        return Err("cmap encoding records do not fit the cmap table".into());
    }
    let unicode = (0..n).any(|i| {
        let r = cmap + 4 + 8 * i;
        let (Some(pid), Some(eid), Some(off)) = (be16(d, r), be16(d, r + 2), be32(d, r + 4)) else { return false };
        let usable = pid == 0 || (pid == 3 && (eid == 1 || eid == 10));
        usable && (off as usize).checked_add(4).is_some_and(|end| end <= cmap_len)
    });
    if !unicode {
        return Err("no usable Unicode cmap subtable".into());
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal well-formed TrueType file: header + 6 table records + table bodies.
    fn tiny_ttf() -> Vec<u8> {
        let tables: [(&[u8; 4], usize); 6] = [(b"cmap", 12), (b"glyf", 4), (b"head", 54), (b"hhea", 36), (b"hmtx", 4), (b"loca", 4)];
        let mut d = vec![0, 1, 0, 0];
        d.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        d.extend_from_slice(&[0; 6]);
        let mut off = 12 + 16 * tables.len();
        let mut bodies = Vec::new();
        for (tag, len) in tables {
            d.extend_from_slice(tag);
            d.extend_from_slice(&[0; 4]);
            d.extend_from_slice(&(off as u32).to_be_bytes());
            d.extend_from_slice(&(len as u32).to_be_bytes());
            let mut body = vec![0u8; len];
            if tag == b"cmap" {
                // version 0, 1 record: platform 3 encoding 1, subtable at +4 (inside the table)
                body[2..4].copy_from_slice(&1u16.to_be_bytes());
                body[4..6].copy_from_slice(&3u16.to_be_bytes());
                body[6..8].copy_from_slice(&1u16.to_be_bytes());
                body[8..12].copy_from_slice(&4u32.to_be_bytes());
            }
            bodies.extend_from_slice(&body);
            off += len;
        }
        d.extend_from_slice(&bodies);
        d
    }

    #[test]
    fn font_check_accepts_a_wellformed_ttf() {
        assert_eq!(check_font(&tiny_ttf()), Ok("TrueType"));
    }

    #[test]
    fn font_check_rejects_what_imgui_would_assert_on() {
        assert!(check_font(&[]).unwrap_err().contains("empty"));
        assert!(check_font(b"hello world, not a font at all").unwrap_err().contains("magic"));
        assert!(check_font(b"ttcf\0\x01\0\0\0\0\0\0").unwrap_err().contains(".ttc"));
        assert!(check_font(b"wOFF\0\x01\0\0\0\0\0\0").unwrap_err().contains("WOFF"));
        assert!(check_font(&[0, 1, 0, 0, 0, 0]).is_err(), "too short");
        assert!(check_font(&vec![0u8; MAX_FONT_BYTES + 1]).unwrap_err().contains("max"));
        // directory claims more tables than the file holds
        let mut d = tiny_ttf();
        d[4..6].copy_from_slice(&500u16.to_be_bytes());
        assert!(check_font(&d).is_err());
        // a table pointing past the end of the file
        let mut d = tiny_ttf();
        d[12 + 8..12 + 12].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
        assert!(check_font(&d).unwrap_err().contains("outside"));
        // truncated file
        let d = tiny_ttf();
        assert!(check_font(&d[..d.len() - 10]).is_err());
        // no Unicode cmap subtable (platform 1 = Mac Roman only)
        let mut d = tiny_ttf();
        let cmap = 12 + 16 * 6;
        d[cmap + 4..cmap + 6].copy_from_slice(&1u16.to_be_bytes());
        assert!(check_font(&d).unwrap_err().contains("cmap"));
        // a required table renamed away
        let mut d = tiny_ttf();
        d[12 + 16 * 2..12 + 16 * 2 + 4].copy_from_slice(b"xxxx"); // head
        assert!(check_font(&d).unwrap_err().contains("head"));
        // glyf without loca
        let mut d = tiny_ttf();
        d[12 + 16 * 5..12 + 16 * 5 + 4].copy_from_slice(b"zzzz"); // loca
        assert!(check_font(&d).unwrap_err().contains("loca"));
    }
}
