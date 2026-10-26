//! HUD geometry in pixels (docs/M2_DESIGN.md section 13.2). The DLL's render loop only draws
//! what these functions return, so the crosshair/hitmarker/ammo layout is unit-tested offline.

/// A line segment in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: [f32; 2],
    pub b: [f32; 2],
}

/// How the raw `CSCam.fov` value was interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FovUnit {
    Radians,
    Degrees,
    Fallback,
}

/// Vertical FOV in radians from the raw camera value: 0.2-3.0 is radians, 10-170 degrees,
/// anything else uses the fallback (unit is inference until the first log shows the raw value).
pub fn vfov_rad(raw: f32, fallback_deg: f32) -> (f32, FovUnit) {
    if (0.2..=3.0).contains(&raw) {
        (raw, FovUnit::Radians)
    } else if (10.0..=170.0).contains(&raw) {
        (raw.to_radians(), FovUnit::Degrees)
    } else {
        (fallback_deg.clamp(1.0, 170.0).to_radians(), FovUnit::Fallback)
    }
}

/// IW virtual screen height scale (`H/480`) and 1080p scale (`H/1080`).
pub fn scales(h: f32) -> (f32, f32) {
    (h / 480.0, h / 1080.0)
}

/// Pixel distance from the screen centre at which a cone of `spread_deg` appears.
pub fn crosshair_gap(h: f32, spread_deg: f32, vfov: f32, min_gap_px: f32) -> f32 {
    let half = (vfov * 0.5).tan();
    let g = if half > 1e-4 { (h * 0.5) * spread_deg.to_radians().tan() / half } else { 0.0 };
    g.max(min_gap_px)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crosshair {
    /// left, right, up, down
    pub segs: [Seg; 4],
    pub gap: f32,
    pub len: f32,
    pub thickness: f32,
}

/// MW2-style four-line crosshair; the gap follows the hip spread.
pub fn crosshair(
    w: f32,
    h: f32,
    spread_deg: f32,
    vfov: f32,
    min_gap_px: f32,
    reticle_side: f32,
    line_scale: f32,
) -> Crosshair {
    let (cx, cy) = (w * 0.5, h * 0.5);
    let (s480, s1080) = scales(h);
    let gap = crosshair_gap(h, spread_deg, vfov, min_gap_px);
    let len = (reticle_side * s480 * line_scale).max(1.0);
    let thickness = (2.0 * s1080).round().max(1.0);
    let segs = [
        Seg { a: [cx - gap - len, cy], b: [cx - gap, cy] },
        Seg { a: [cx + gap, cy], b: [cx + gap + len, cy] },
        Seg { a: [cx, cy - gap - len], b: [cx, cy - gap] },
        Seg { a: [cx, cy + gap], b: [cx, cy + gap + len] },
    ];
    Crosshair { segs, gap, len, thickness }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hitmarker {
    pub segs: [Seg; 4],
    pub alpha: f32,
    pub thickness: f32,
}

/// The hitmarker X `age_s` after a hit, fading linearly over `dur_s`; `None` once expired.
pub fn hitmarker(w: f32, h: f32, age_s: f64, dur_s: f64) -> Option<Hitmarker> {
    if !(age_s >= 0.0 && age_s < dur_s && dur_s > 0.0) {
        return None;
    }
    let (cx, cy) = (w * 0.5, h * 0.5);
    let (_, s1080) = scales(h);
    let (r0, r1) = (8.0 * s1080, 20.0 * s1080);
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let mut segs = [Seg { a: [0.0; 2], b: [0.0; 2] }; 4];
    for (i, (sx, sy)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
        segs[i] = Seg {
            a: [cx + sx * r0 * d, cy + sy * r0 * d],
            b: [cx + sx * r1 * d, cy + sy * r1 * d],
        };
    }
    Some(Hitmarker {
        segs,
        alpha: (1.0 - age_s / dur_s) as f32,
        thickness: (2.0 * s1080).round().max(1.0),
    })
}

/// Bottom-right anchor of the ammo counter (right edge x, bottom y).
pub fn ammo_anchor(w: f32, h: f32) -> [f32; 2] {
    let (_, s1080) = scales(h);
    [w - 40.0 * s1080, h - 40.0 * s1080]
}

/// The magazine number turns red at or below the low-ammo count.
pub fn mag_is_low(mag: u32, low_count: u32) -> bool {
    mag <= low_count
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReloadBar {
    /// Centre of the "RELOADING" text.
    pub text_center: [f32; 2],
    pub outer_min: [f32; 2],
    pub outer_max: [f32; 2],
    pub fill_max: [f32; 2],
}

/// Reload indicator below the crosshair: text at `cy + 60*s1080`, a `120*s1080` px bar under it.
pub fn reload_bar(w: f32, h: f32, progress: f32, text_h: f32) -> ReloadBar {
    let (cx, cy) = (w * 0.5, h * 0.5);
    let (_, s1080) = scales(h);
    let ty = cy + 60.0 * s1080;
    let bw = 120.0 * s1080;
    let bh = (6.0 * s1080).max(2.0);
    let top = ty + text_h * 0.5 + 6.0 * s1080;
    let outer_min = [cx - bw * 0.5, top];
    let outer_max = [cx + bw * 0.5, top + bh];
    let p = progress.clamp(0.0, 1.0);
    ReloadBar { text_center: [cx, ty], outer_min, outer_max, fill_max: [outer_min[0] + bw * p, top + bh] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosshair_gap_known_answer_1080p() {
        let vfov = 48f32.to_radians();
        let g = crosshair_gap(1080.0, 2.0, vfov, 4.0);
        let expect = 540.0 * 2f32.to_radians().tan() / 24f32.to_radians().tan();
        assert!((g - expect).abs() < 1e-3);
        assert!((g - 42.35).abs() < 0.1, "gap {g}");
    }

    #[test]
    fn crosshair_gap_grows_with_spread() {
        let vfov = 48f32.to_radians();
        let mut prev = 0.0;
        for i in 0..50 {
            let g = crosshair_gap(1080.0, 2.0 + i as f32 * 0.1, vfov, 4.0);
            assert!(g > prev);
            prev = g;
        }
        assert_eq!(crosshair_gap(1080.0, 0.0, vfov, 4.0), 4.0, "minimum gap");
    }

    #[test]
    fn crosshair_lines_at_1080p() {
        let c = crosshair(1920.0, 1080.0, 2.0, 48f32.to_radians(), 4.0, 8.0, 1.0);
        assert!((c.len - 18.0).abs() < 1e-4, "8 * 1080/480");
        assert_eq!(c.thickness, 2.0);
        assert_eq!(c.segs[0].b, [960.0 - c.gap, 540.0]);
        assert_eq!(c.segs[3].a, [960.0, 540.0 + c.gap]);
    }

    #[test]
    fn crosshair_fov_units() {
        assert_eq!(vfov_rad(0.8, 48.0), (0.8, FovUnit::Radians));
        let (r, u) = vfov_rad(60.0, 48.0);
        assert!((r - 60f32.to_radians()).abs() < 1e-6 && u == FovUnit::Degrees);
        assert_eq!(vfov_rad(5.0, 48.0).1, FovUnit::Fallback);
        assert_eq!(vfov_rad(f32::NAN, 48.0).1, FovUnit::Fallback);
    }

    #[test]
    fn crosshair_hitmarker_fades_and_expires() {
        let h0 = hitmarker(1920.0, 1080.0, 0.0, 0.2).unwrap();
        assert_eq!(h0.alpha, 1.0);
        let h1 = hitmarker(1920.0, 1080.0, 0.1, 0.2).unwrap();
        assert!((h1.alpha - 0.5).abs() < 1e-6);
        assert!(hitmarker(1920.0, 1080.0, 0.2, 0.2).is_none());
        assert!(hitmarker(1920.0, 1080.0, -0.1, 0.2).is_none());
        let r = |s: Seg| ((s.b[0] - 960.0).powi(2) + (s.b[1] - 540.0).powi(2)).sqrt();
        assert!((r(h0.segs[0]) - 20.0).abs() < 1e-3);
    }

    #[test]
    fn crosshair_ammo_and_reload_layout() {
        assert_eq!(ammo_anchor(1920.0, 1080.0), [1880.0, 1040.0]);
        assert!(mag_is_low(10, 10) && !mag_is_low(11, 10));
        let b = reload_bar(1920.0, 1080.0, 0.5, 20.0);
        assert_eq!(b.text_center, [960.0, 600.0]);
        assert!((b.outer_max[0] - b.outer_min[0] - 120.0).abs() < 1e-4);
        assert!((b.fill_max[0] - b.outer_min[0] - 60.0).abs() < 1e-4);
    }
}

/// M5: a small bar figure of the stance (0 stand, 1 crouch, 2 prone) whose bounding box ends at `right` and sits on
/// `bottom`, `size` pixels tall: line segments (from, to). Anything else (255 = not driving) draws nothing.
pub fn stance_glyph(stance: u8, right: f32, bottom: f32, size: f32) -> Vec<([f32; 2], [f32; 2])> {
    let s = size.max(4.0);
    let x = right - 0.5 * s;
    match stance {
        0 => vec![
            ([x, bottom - s], [x, bottom - 0.82 * s]),
            ([x, bottom - 0.78 * s], [x, bottom - 0.4 * s]),
            ([x, bottom - 0.4 * s], [x - 0.2 * s, bottom]),
            ([x, bottom - 0.4 * s], [x + 0.2 * s, bottom]),
            ([x - 0.25 * s, bottom - 0.65 * s], [x + 0.25 * s, bottom - 0.65 * s]),
        ],
        1 => vec![
            ([x, bottom - 0.7 * s], [x, bottom - 0.55 * s]),
            ([x, bottom - 0.5 * s], [x, bottom - 0.25 * s]),
            ([x, bottom - 0.25 * s], [x + 0.25 * s, bottom - 0.2 * s]),
            ([x + 0.25 * s, bottom - 0.2 * s], [x + 0.2 * s, bottom]),
            ([x - 0.25 * s, bottom - 0.42 * s], [x + 0.25 * s, bottom - 0.42 * s]),
        ],
        2 => vec![
            ([right - s, bottom - 0.12 * s], [right - 0.85 * s, bottom - 0.12 * s]),
            ([right - 0.8 * s, bottom - 0.08 * s], [right, bottom - 0.04 * s]),
            ([right - 0.95 * s, bottom - 0.25 * s], [right - 0.55 * s, bottom - 0.25 * s]),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod stance_glyph_tests {
    use super::*;

    #[test]
    fn hud_stance_glyph_inside_box_and_heights() {
        for st in 0..3u8 {
            let segs = stance_glyph(st, 100.0, 50.0, 20.0);
            assert!(!segs.is_empty());
            for (a, b) in &segs {
                for p in [a, b] {
                    assert!(p[0] >= 79.99 && p[0] <= 100.01 && p[1] >= 29.99 && p[1] <= 50.01, "{st}: {p:?}");
                }
            }
        }
        let top = |st: u8| stance_glyph(st, 100.0, 50.0, 20.0).iter().map(|(a, b)| a[1].min(b[1])).fold(f32::MAX, f32::min);
        assert!(top(0) < top(1) && top(1) < top(2), "stand taller than crouch taller than prone");
        assert!(stance_glyph(255, 100.0, 50.0, 20.0).is_empty());
    }
}
