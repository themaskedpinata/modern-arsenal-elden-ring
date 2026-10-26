//! DCX container (big-endian 0x4C header) with the ZSTD codec, as the ER 2.7.2.0 regulation uses (`DCX_ZSTD`, level 21).
//!
//! Header: `DCX\0` 00011000 00000018 00000024 00000044 0000004C | `DCS\0` usize csize | `DCP\0` `ZSTD` 00000020
//! level 00 00 00 + 12 zero bytes + 00010100 | `DCA\0` 00000008 | the zstd frame. The writer keeps the vanilla header
//! byte for byte and changes only the two sizes (WitchyBND does the same). Other codecs (DFLT, KRAK = Oodle) are refused:
//! this builder never needs Oodle.
//!
//! zstd frame of the output: libzstd (the pinned `zstd` crate) level 21, windowLog 16 (64 KiB, the window of
//! WitchyBND's game-proven frames; vanilla uses 64 MiB), no content size, no checksum, single thread.

use std::io::Write;

pub const HEADER_LEN: usize = 0x4C;
pub const LEVEL: i32 = 21;
pub const WINDOW_LOG: u32 = 16;

/// A parsed DCX: the header bytes (kept for the writer) and the compressed frame.
#[derive(Clone, Debug)]
pub struct Dcx<'a> {
    pub header: [u8; HEADER_LEN],
    pub usize: u32,
    pub frame: &'a [u8],
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}

impl<'a> Dcx<'a> {
    /// Parses the DCX at the start of `b` (trailing AES padding is ignored: `csize` delimits the frame).
    pub fn parse(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < HEADER_LEN || &b[0..4] != b"DCX\0" {
            return Err("not a DCX (decrypted regulation does not start with DCX)".into());
        }
        let fixed = [(4, 0x11000), (8, 0x18), (0xC, 0x24), (0x10, 0x44), (0x14, 0x4C), (0x2C, 0x20), (0x48, 8)];
        for (o, want) in fixed {
            if be32(b, o) != want {
                return Err(format!("DCX header field at 0x{o:x} is 0x{:x} (want 0x{want:x})", be32(b, o)));
            }
        }
        if &b[0x18..0x1C] != b"DCS\0" || &b[0x24..0x28] != b"DCP\0" || &b[0x44..0x48] != b"DCA\0" {
            return Err("DCX: missing DCS / DCP / DCA chunk".into());
        }
        let codec = &b[0x28..0x2C];
        if codec != b"ZSTD" {
            return Err(format!(
                "DCX codec {:?} is not supported (only DCX_ZSTD; Oodle KRAK is never used by this builder)",
                String::from_utf8_lossy(codec)
            ));
        }
        let usize = be32(b, 0x1C);
        let csize = be32(b, 0x20) as usize;
        let frame = b.get(HEADER_LEN..HEADER_LEN + csize).ok_or("DCX: compressed size past the end")?;
        Ok(Dcx { header: b[..HEADER_LEN].try_into().unwrap(), usize, frame })
    }

    pub fn level(&self) -> u8 {
        self.header[0x30]
    }

    /// Decompresses the frame (one shot into the known size: no 64 MiB window buffer).
    pub fn decompress(&self) -> Result<Vec<u8>, String> {
        let out = zstd::bulk::decompress(self.frame, self.usize as usize).map_err(|e| format!("zstd decompress: {e}"))?;
        if out.len() != self.usize as usize {
            return Err(format!("DCX: {} bytes decompressed (header says {})", out.len(), self.usize));
        }
        Ok(out)
    }
}

/// Compresses `data` with the output settings (level 21, windowLog 16, no content size, no checksum).
pub fn compress(data: &[u8]) -> Result<Vec<u8>, String> {
    compress_level(data, LEVEL)
}

/// `compress` at another level (tests use a fast level; the build always uses [`LEVEL`]).
pub fn compress_level(data: &[u8], level: i32) -> Result<Vec<u8>, String> {
    let e = |x: std::io::Error| format!("zstd compress: {x}");
    let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(data.len() / 16), level).map_err(e)?;
    enc.include_checksum(false).map_err(e)?;
    enc.include_contentsize(false).map_err(e)?;
    enc.window_log(WINDOW_LOG).map_err(e)?;
    // the pledged size only lets libzstd size its match tables to the 64 KiB window (peak ~170 MB instead of ~440 MB
    // measured); the frame bytes are the same (measured: identical sha256) and no content size is written
    enc.set_pledged_src_size(Some(data.len() as u64)).map_err(e)?;
    // fixed 1 MiB input chunks: the frame bytes depend only on the data and the libzstd version
    for chunk in data.chunks(1 << 20) {
        enc.write_all(chunk).map_err(e)?;
    }
    enc.finish().map_err(e)
}

/// The DCX bytes: `template`'s header with the new sizes, then the frame.
pub fn write(template: &[u8; HEADER_LEN], usize: usize, frame: &[u8]) -> Result<Vec<u8>, String> {
    let (u, c) = (u32::try_from(usize).map_err(|_| "DCX: data > 4 GB")?, u32::try_from(frame.len()).map_err(|_| "DCX: frame > 4 GB")?);
    let mut out = Vec::with_capacity(HEADER_LEN + frame.len());
    out.extend_from_slice(template);
    out[0x1C..0x20].copy_from_slice(&u.to_be_bytes());
    out[0x20..0x24].copy_from_slice(&c.to_be_bytes());
    out.extend_from_slice(frame);
    Ok(out)
}

/// The frame header facts of a zstd frame: (window size in bytes or None for single-segment, content size flag,
/// checksum flag). Used by the harness report.
pub fn frame_facts(frame: &[u8]) -> Option<(Option<u64>, bool, bool)> {
    if frame.len() < 6 || frame[..4] != [0x28, 0xB5, 0x2F, 0xFD] {
        return None;
    }
    let fhd = frame[4];
    let single = fhd & 0x20 != 0;
    let checksum = fhd & 0x04 != 0;
    let fcs = (fhd >> 6) != 0 || single;
    let window = (!single).then(|| {
        let wd = frame[5];
        let exp = (wd >> 3) as u32 + 10;
        let base = 1u64 << exp;
        base + (base / 8) * (wd & 7) as u64
    });
    Some((window, fcs, checksum))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> [u8; HEADER_LEN] {
        let mut h = Vec::new();
        h.extend_from_slice(b"DCX\0");
        for v in [0x11000u32, 0x18, 0x24, 0x44, 0x4C] {
            h.extend_from_slice(&v.to_be_bytes());
        }
        h.extend_from_slice(b"DCS\0");
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(b"DCP\0ZSTD");
        h.extend_from_slice(&0x20u32.to_be_bytes());
        h.extend_from_slice(&[21, 0, 0, 0]);
        h.extend_from_slice(&[0; 12]);
        h.extend_from_slice(&[0, 1, 1, 0]);
        h.extend_from_slice(b"DCA\0");
        h.extend_from_slice(&8u32.to_be_bytes());
        h.try_into().unwrap()
    }

    #[test]
    fn round_trip_small_and_frame_shape() {
        let data: Vec<u8> = (0..300_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        let frame = compress_level(&data, 3).unwrap();
        let (window, fcs, ck) = frame_facts(&frame).unwrap();
        assert_eq!((window, fcs, ck), (Some(1 << WINDOW_LOG), false, false), "64 KiB window, no size, no checksum");
        let dcx = write(&template(), data.len(), &frame).unwrap();
        let mut padded = dcx.clone();
        padded.extend_from_slice(&[0; 7]);
        let p = Dcx::parse(&padded).unwrap();
        assert_eq!(p.level(), 21);
        assert_eq!(p.header[..0x1C], template()[..0x1C]);
        assert_eq!(p.decompress().unwrap(), data);
        assert_eq!(compress_level(&data, 3).unwrap(), frame, "deterministic");
    }

    #[test]
    fn libzstd_is_the_pinned_version() {
        // the frame bytes (so the package sha256 and the co-op id) change with the libzstd version: an update must be
        // a deliberate, coordinated release (zstd-sys 2.1.2+zstd.1.5.7 in Cargo.lock)
        assert_eq!(zstd::zstd_safe::version_number(), 10507);
    }

    #[test]
    fn refuses_other_codecs_and_bad_headers() {
        let mut d = write(&template(), 0, &[]).unwrap();
        d[0x28..0x2C].copy_from_slice(b"KRAK");
        assert!(Dcx::parse(&d).unwrap_err().contains("KRAK"));
        let mut d = write(&template(), 0, &[]).unwrap();
        d[0x7] = 0x12;
        assert!(Dcx::parse(&d).is_err());
        assert!(Dcx::parse(b"DCX\0").is_err());
        let d = write(&template(), 10, &[1, 2, 3]).unwrap();
        assert!(Dcx::parse(&d[..HEADER_LEN + 2]).is_err(), "frame past the end");
    }
}
