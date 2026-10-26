//! The row fields regbuild reads or writes: a generated offset table (`fields_gen.rs`, from the Paramdex ER defs by
//! `tools/scripts/gen_regbuild_fields.py`; facts only) and the packing rules of build_regulation.py / param_rows.py.
//!
//! Whole rows are cloned, so no full paramdef is needed: unknown and padding fields keep the clone's bytes. Floats pack
//! as `value as f32` (== Python `struct.pack('<f', float)`), integers as their type (out of range = an error, as
//! `struct.pack` raises), bitfields LSB-first inside their unit.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    F32,
}

impl Kind {
    pub fn size(self) -> usize {
        match self {
            Kind::S8 | Kind::U8 => 1,
            Kind::S16 | Kind::U16 => 2,
            Kind::S32 | Kind::U32 | Kind::F32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub offset: usize,
    /// (bit offset, width) of a bitfield inside its unit of `kind`.
    pub bit: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamLayout {
    pub stem: &'static str,
    pub param_type: &'static str,
    pub row_size: usize,
    pub fields: &'static [Field],
}

impl ParamLayout {
    pub fn field(&self, name: &str) -> Option<&'static Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The field names covering byte `off` (diff reports).
    pub fn fields_at(&self, off: usize) -> Vec<&'static str> {
        self.fields.iter().filter(|f| off >= f.offset && off < f.offset + f.kind.size()).map(|f| f.name).collect()
    }
}

#[path = "fields_gen.rs"]
mod generated;
pub use generated::{ALL, ASSETENVIRONMENTGEOMETRYPARAM as ASSET, ATKPARAM_PC as ATK, BULLET, SPEFFECTPARAM as SPEFFECT};

pub fn layout_of(stem: &str) -> Option<&'static ParamLayout> {
    ALL.iter().copied().find(|l| l.stem == stem)
}

/// A planned or decoded value: Python int or float.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum V {
    I(i64),
    F(f64),
}

impl V {
    pub fn as_f64(self) -> f64 {
        match self {
            V::I(i) => i as f64,
            V::F(f) => f,
        }
    }

    /// Python `int(v)` (truncation toward zero).
    pub fn as_i64(self) -> i64 {
        match self {
            V::I(i) => i,
            V::F(f) => f.trunc() as i64,
        }
    }

    /// build_regulation.same_value: floats within 1e-6 relative (min 1), ints exactly.
    pub fn same(self, want: V) -> bool {
        match (self, want) {
            (V::I(a), V::I(b)) => a == b,
            (a, b) => (a.as_f64() - b.as_f64()).abs() <= 1e-6 * b.as_f64().abs().max(1.0),
        }
    }
}

impl std::fmt::Display for V {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            V::I(i) => write!(f, "{i}"),
            V::F(x) => f.write_str(&crate::pyfmt::float_repr(*x)),
        }
    }
}

fn unit(row: &[u8], f: &Field) -> u64 {
    let mut b = [0u8; 8];
    b[..f.kind.size()].copy_from_slice(&row[f.offset..f.offset + f.kind.size()]);
    u64::from_le_bytes(b)
}

/// Decodes one field (param_rows.decode; floats as the f32 widened, not rounded).
pub fn read(row: &[u8], f: &Field) -> V {
    let u = unit(row, f);
    if let Some((bo, w)) = f.bit {
        return V::I(((u >> bo) & ((1u64 << w) - 1)) as i64);
    }
    match f.kind {
        Kind::S8 => V::I(u as u8 as i8 as i64),
        Kind::U8 => V::I(u as u8 as i64),
        Kind::S16 => V::I(u as u16 as i16 as i64),
        Kind::U16 => V::I(u as u16 as i64),
        Kind::S32 => V::I(u as u32 as i32 as i64),
        Kind::U32 => V::I(u as u32 as i64),
        Kind::F32 => V::F(f32::from_bits(u as u32) as f64),
    }
}

/// Packs `v` into `row` (reg_rows.pack: bitfields masked, f32 = `as f32`, ints range-checked).
pub fn write(row: &mut [u8], f: &Field, v: V) -> Result<(), String> {
    let n = f.kind.size();
    if let Some((bo, w)) = f.bit {
        let mask = ((1u64 << w) - 1) << bo;
        let cur = (unit(row, f) & !mask) | (((v.as_i64() as u64) << bo) & mask);
        row[f.offset..f.offset + n].copy_from_slice(&cur.to_le_bytes()[..n]);
        return Ok(());
    }
    let bytes: Vec<u8> = match f.kind {
        Kind::F32 => (v.as_f64() as f32).to_le_bytes().to_vec(),
        k => {
            let i = v.as_i64();
            let (lo, hi) = match k {
                Kind::S8 => (i8::MIN as i64, i8::MAX as i64),
                Kind::U8 => (0, u8::MAX as i64),
                Kind::S16 => (i16::MIN as i64, i16::MAX as i64),
                Kind::U16 => (0, u16::MAX as i64),
                Kind::S32 => (i32::MIN as i64, i32::MAX as i64),
                _ => (0, u32::MAX as i64),
            };
            if !(lo..=hi).contains(&i) {
                return Err(format!("{} = {i} out of range for {k:?}", f.name));
            }
            i.to_le_bytes()[..n].to_vec()
        }
    };
    row[f.offset..f.offset + n].copy_from_slice(&bytes);
    Ok(())
}

/// Sets the bits `f` covers in `mask` (one bit per row bit): the "override" mask of the clone check.
pub fn mark(mask: &mut [u8], f: &Field) {
    match f.bit {
        Some((bo, w)) => {
            for b in bo..bo + w {
                mask[f.offset + (b / 8) as usize] |= 1 << (b % 8);
            }
        }
        None => mask[f.offset..f.offset + f.kind.size()].fill(0xFF),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_consistent() {
        for l in ALL {
            assert!(!l.fields.is_empty());
            for f in l.fields {
                assert!(f.offset + f.kind.size() <= l.row_size, "{}.{}", l.stem, f.name);
                if let Some((bo, w)) = f.bit {
                    assert!(bo + w <= 8 * f.kind.size() as u32, "{}.{}", l.stem, f.name);
                }
            }
        }
        assert_eq!((BULLET.row_size, ATK.row_size, SPEFFECT.row_size, ASSET.row_size), (272, 456, 912, 320));
        // research.md section 2.2 spot checks
        let f = BULLET.field("isAttackSFX").unwrap();
        assert_eq!((f.offset, f.bit), (154, Some((6, 1))));
        assert_eq!(ATK.field("atkPhys").unwrap().offset, 80);
        assert_eq!(SPEFFECT.field("isHpBurnEffect").unwrap().bit, Some((5, 1)));
        assert_eq!(ASSET.field("defense").unwrap().kind, Kind::U16);
        assert_eq!(layout_of("SpEffectParam").unwrap().param_type, "SP_EFFECT_PARAM_ST");
    }

    #[test]
    fn pack_and_read() {
        let mut row = vec![0xFFu8; 272];
        let f = BULLET.field("isHitBothTeam").unwrap();
        write(&mut row, f, V::I(0)).unwrap();
        assert_eq!(row[155], 0xFD);
        assert_eq!(read(&row, f), V::I(0));
        let f = BULLET.field("life").unwrap();
        write(&mut row, f, V::F(0.15)).unwrap();
        assert_eq!(&row[16..20], &0.15f32.to_le_bytes());
        assert!(read(&row, f).same(V::F(0.15)));
        let f = BULLET.field("homingAngle").unwrap();
        write(&mut row, f, V::I(-1)).unwrap();
        assert_eq!(read(&row, f), V::I(-1));
        assert!(write(&mut row, f, V::I(40000)).is_err());
        let f = BULLET.field("lockShootLimitAng").unwrap();
        assert!(write(&mut row, f, V::I(-1)).is_err());
        let mut m = vec![0u8; 272];
        mark(&mut m, BULLET.field("isUseSharedHitList").unwrap());
        mark(&mut m, BULLET.field("atkId_Bullet").unwrap());
        assert_eq!((m[155], &m[0..4], m[4]), (0x04, &[0xFFu8; 4][..], 0));
    }

    #[test]
    fn same_value_rule() {
        assert!(V::F(0.15000000596).same(V::F(0.15)));
        assert!(!V::F(0.16).same(V::F(0.15)));
        assert!(V::I(0).same(V::F(0.0)));
        assert!(!V::I(1).same(V::I(2)));
    }
}
