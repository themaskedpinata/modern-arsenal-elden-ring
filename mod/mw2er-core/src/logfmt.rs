//! Exact log line formats (docs/M2_DESIGN.md section 15.2).
//!
//! `YYYY-MM-DD HH:MM:SS.mmm L [thread] TAG message`. The `HIT chr=c#### hp a->b dmg=N` prefix is
//! fixed (MODDING_PLAN M2 oracle, `tools/scripts/analyze_log.py`); extra fields only go after it.

use crate::vec3::V3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn letter(self) -> char {
        match self {
            Level::Debug => 'D',
            Level::Info => 'I',
            Level::Warn => 'W',
            Level::Error => 'E',
        }
    }
}

/// A wall-clock time stamp (local time in the DLL, UTC for build times).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

impl Stamp {
    pub fn date_time(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// UTC civil time from Unix seconds (Howard Hinnant's days-from-civil inverse).
pub fn utc_from_unix(secs: i64) -> Stamp {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Stamp {
        year: y as u16,
        month: m as u8,
        day: d as u8,
        hour: (sod / 3600) as u8,
        minute: (sod % 3600 / 60) as u8,
        second: (sod % 60) as u8,
        millis: 0,
    }
}

/// One complete log line (without the trailing newline).
pub fn line(ts: &Stamp, level: Level, thread: &str, tag: &str, msg: &str) -> String {
    format!(
        "{}.{:03} {} [{}] {} {}",
        ts.date_time(),
        ts.millis,
        level.letter(),
        thread,
        tag,
        msg
    )
}

/// `(x,y,z)` with two decimals.
pub fn v3(v: V3) -> String {
    format!("({:.2},{:.2},{:.2})", v.x, v.y, v.z)
}

/// `(x,y,z)` with three decimals for unit directions.
pub fn dir3(v: V3) -> String {
    format!("({:.3},{:.3},{:.3})", v.x, v.y, v.z)
}

/// A field-ins handle as 16 hex digits.
pub fn handle(h: u64) -> String {
    format!("0x{h:016x}")
}

/// How a HIT was attributed (M3): `via=impact|last_hit_by n=K kill=0|1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitAttr<'a> {
    pub via: &'a str,
    pub n: u32,
    pub kill: bool,
}

/// `HIT chr=c#### hp a->b dmg=N band=... mw2=... d=...m via=... n=... kill=...` (message part, after the tag).
/// The prefix up to `dmg=N` is fixed (M2 non-negotiable 11); the M3 fields only go after it.
pub fn hit_msg(character_id: u32, hp_before: i32, hp_after: i32, band_id: i32, mw2: i32, d_m: f32, attr: HitAttr<'_>) -> String {
    format!(
        "chr=c{:04} hp {}->{} dmg={} band={} mw2={} d={:.1}m via={} n={} kill={}",
        character_id,
        hp_before,
        hp_after,
        hp_before - hp_after,
        band_id,
        mw2,
        d_m,
        attr.via,
        attr.n,
        attr.kill as u8
    )
}

pub fn kill_msg(character_id: u32) -> String {
    format!("chr=c{character_id:04}")
}

pub fn hpdrop_msg(character_id: u32, hp_before: i32, hp_after: i32, last_hit_by: u64) -> String {
    format!(
        "chr=c{:04} hp {}->{} last_hit_by={} (not attributed)",
        character_id,
        hp_before,
        hp_after,
        handle(last_hit_by)
    )
}

/// Everything the SHOT line reports.
#[derive(Clone, Copy, Debug)]
pub struct ShotFields {
    pub n: u64,
    pub mag: u32,
    pub reserve: u32,
    pub row_id: i32,
    pub mw2: i32,
    pub d_m: f32,
    pub src: &'static str,
    pub spread_deg: f32,
    pub origin: V3,
    pub dir: V3,
}

pub fn shot_msg(s: &ShotFields) -> String {
    format!(
        "n={} mag={}/{} id={} mw2={} d={:.1}m src={} spread={:.2} origin={} dir={}",
        s.n,
        s.mag,
        s.reserve,
        s.row_id,
        s.mw2,
        s.d_m,
        s.src,
        s.spread_deg,
        v3(s.origin),
        dir3(s.dir)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logfmt_hit_prefix_is_exact() {
        let at = HitAttr { via: "impact", n: 1, kill: false };
        let m = hit_msg(4311, 219, 116, 79000100, 40, 11.83, at);
        let l = line(&Stamp::default(), Level::Info, "task", "HIT", &m);
        assert!(l.contains(" I [task] HIT chr=c4311 hp 219->116 dmg=103 band=79000100 mw2=40 d=11.8m via=impact n=1 kill=0"), "{l}");
        assert!(m.starts_with("chr=c4311 hp 219->116 dmg=103"));
        let k = HitAttr { via: "last_hit_by", n: 0, kill: true };
        assert_eq!(hit_msg(7, 5, 0, 1, 2, 3.0, k)[..24].to_string(), "chr=c0007 hp 5->0 dmg=5 ");
        assert!(hit_msg(7, 5, 0, 1, 2, 3.0, k).ends_with("via=last_hit_by n=0 kill=1"));
    }

    #[test]
    fn logfmt_line_layout() {
        let ts = Stamp { year: 2026, month: 10, day: 14, hour: 20, minute: 1, second: 2, millis: 45 };
        assert_eq!(
            line(&ts, Level::Warn, "boot", "VERSION", "PASS"),
            "2026-10-14 20:01:02.045 W [boot] VERSION PASS"
        );
    }

    #[test]
    fn logfmt_other_lines() {
        assert_eq!(kill_msg(4311), "chr=c4311");
        assert!(hpdrop_msg(4311, 10, 5, 0x1234).ends_with("last_hit_by=0x0000000000001234 (not attributed)"));
        let s = ShotFields {
            n: 1,
            mag: 31,
            reserve: 192,
            row_id: 79000100,
            mw2: 40,
            d_m: 11.84,
            src: "chr",
            spread_deg: 2.0,
            origin: V3::new(1.0, 2.0, 3.0),
            dir: V3::new(0.0, 0.0, 1.0),
        };
        assert_eq!(
            shot_msg(&s),
            "n=1 mag=31/192 id=79000100 mw2=40 d=11.8m src=chr spread=2.00 origin=(1.00,2.00,3.00) dir=(0.000,0.000,1.000)"
        );
    }

    #[test]
    fn logfmt_utc_from_unix() {
        assert_eq!(utc_from_unix(0).date_time(), "1970-01-01 00:00:00");
        assert_eq!(utc_from_unix(951_782_400).date_time(), "2000-02-29 00:00:00");
        assert_eq!(utc_from_unix(1_791_936_000 + 3661).date_time(), "2026-10-14 01:01:01");
    }
}
