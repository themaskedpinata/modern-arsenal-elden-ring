//! AC-130 HUD geometry and text (docs/M4_DESIGN.md section 11). Pure; hud.rs draws with ImGui, the thermal shader
//! gets the same 4:3 box.
//!
//! The MW2 menu `ac130_hud_hd` is laid out in a virtual 640x480 screen; it maps into the centred 4:3 box (2880x2160
//! at 3840x2160). Menu alignment codes [I: IW4 menu enums]: 1/8 = left/top (safe area), 2/7/9 = centre,
//! 3/10 = right/bottom, 4 = fullscreen (the whole screen, not the box).

/// The centred 4:3 box `[x0, y0, w, h]` in pixels.
pub fn box43(w: f32, h: f32) -> [f32; 4] {
    if w <= 0.0 || h <= 0.0 {
        return [0.0; 4];
    }
    if w / h >= 4.0 / 3.0 {
        let bw = h * 4.0 / 3.0;
        [(w - bw) * 0.5, 0.0, bw, h]
    } else {
        let bh = w * 3.0 / 4.0;
        [0.0, (h - bh) * 0.5, w, bh]
    }
}

/// The MW2 blink: `1 - sin(ms / period) * amp`, clamped to 0..1 (range [1 - amp, 1]).
pub fn blink_alpha(ms: f64, period_ms: f32, amp: f32) -> f32 {
    (1.0 - (ms / period_ms as f64).sin() as f32 * amp).clamp(0.0, 1.0)
}

/// `m:ss` of the remaining seconds (rounded up, like MW2's `secondsascountdown`).
pub fn timer_text(remaining_s: f64) -> String {
    let s = remaining_s.max(0.0).ceil() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// One axis of a menu rect: `align` code, `pos`/`size` in virtual units (640 or 480 wide), into the box
/// `[start, len]` (virtual unit = `len / virt`).
fn axis(align: f32, pos: f32, size: f32, start: f32, len: f32, virt: f32, screen: f32) -> (f32, f32) {
    let s = len / virt;
    match align as i32 {
        2 | 7 | 9 => (start + len * 0.5 + pos * s, size * s),
        3 | 10 => (start + len + pos * s, size * s),
        4 => (pos * screen / virt, size * screen / virt),
        _ => (start + pos * s, size * s),
    }
}

/// A menu `rect x y w h horzAlign vertAlign` in pixels `[x, y, w, h]` for a `w` x `h` screen.
pub fn menu_rect(r: [f32; 6], sw: f32, sh: f32) -> [f32; 4] {
    let b = box43(sw, sh);
    let (x, w) = axis(r[4], r[0], r[2], b[0], b[2], 640.0, sw);
    let (y, h) = axis(r[5], r[1], r[3], b[1], b[3], 480.0, sh);
    [x, y, w, h]
}

/// Pixel size of one virtual unit of the box (for text scales).
pub fn unit_px(sh: f32, sw: f32) -> f32 {
    box43(sw, sh)[3] / 480.0
}

/// Projects a world point through the gunship camera (rows r, u, f at `pos`, vertical fov) to pixels; `None` behind.
pub fn project(pos: [f32; 3], rows: [[f32; 3]; 3], vfov: f32, sw: f32, sh: f32, p: [f32; 3]) -> Option<[f32; 2]> {
    let d = [p[0] - pos[0], p[1] - pos[1], p[2] - pos[2]];
    let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    let (x, y, z) = (dot(rows[0]), dot(rows[1]), dot(rows[2]));
    if z <= 1e-3 || !(vfov > 0.0) {
        return None;
    }
    let ty = (vfov * 0.5).tan();
    let tx = ty * sw / sh.max(1.0);
    let nx = x / (z * tx);
    let ny = y / (z * ty);
    Some([(nx * 0.5 + 0.5) * sw, (0.5 - ny * 0.5) * sh])
}

/// The MW2 105 mm dark pulse: black alpha 0 -> `alpha` over `fade_in_s`, held until `hold_s`, then -> 0 over
/// `fade_out_s` (`_ac130.gsc` shotFiredDarkScreenOverlay numbers, read from the user's dump).
pub fn dark_pulse(t: f64, alpha: f64, fade_in_s: f64, hold_s: f64, fade_out_s: f64) -> f32 {
    if !(t >= 0.0) {
        return 0.0;
    }
    let a = if t < fade_in_s {
        alpha * t / fade_in_s.max(1e-6)
    } else if t < hold_s {
        alpha
    } else if t < hold_s + fade_out_s {
        alpha * (1.0 - (t - hold_s) / fade_out_s.max(1e-6))
    } else {
        0.0
    };
    a.clamp(0.0, 1.0) as f32
}

/// The MW2-mode HUD indicator line.
pub fn indicator_text(ind: &crate::ac130_logic::Indicator, key: &str) -> String {
    use crate::ac130_logic::Indicator;
    match ind {
        Indicator::Ready => format!("AC-130 [{key}] READY"),
        Indicator::Cooldown(s) => format!("AC-130 {}", timer_text(*s)),
        Indicator::Unavailable(b) => format!("AC-130 - {}", b.label().replace('_', " ")),
        Indicator::Busy => "AC-130 ...".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac130_logic::{Block, Indicator};

    #[test]
    fn ac130_hud_box_4k_and_1080() {
        assert_eq!(box43(3840.0, 2160.0), [480.0, 0.0, 2880.0, 2160.0]);
        assert_eq!(box43(1920.0, 1080.0), [240.0, 0.0, 1440.0, 1080.0]);
        assert_eq!(box43(1600.0, 1200.0), [0.0, 0.0, 1600.0, 1200.0]);
        assert_eq!(box43(1280.0, 1024.0), [0.0, 32.0, 1280.0, 960.0]);
        // the overlay rect -320 -240 640 480 aligned 2 2 is exactly the box
        assert_eq!(menu_rect([-320.0, -240.0, 640.0, 480.0, 2.0, 2.0], 3840.0, 2160.0), [480.0, 0.0, 2880.0, 2160.0]);
        // the grain rect 0 0 640 480 aligned 4 4 is the whole screen
        assert_eq!(menu_rect([0.0, 0.0, 640.0, 480.0, 4.0, 4.0], 3840.0, 2160.0), [0.0, 0.0, 3840.0, 2160.0]);
        // weapon labels 0 -30 bottom-left of the box
        let r = menu_rect([0.0, -30.0, 100.0, 30.0, 8.0, 10.0], 3840.0, 2160.0);
        assert_eq!(r, [480.0, 2160.0 - 135.0, 450.0, 135.0]);
        assert!((unit_px(2160.0, 3840.0) - 4.5).abs() < 1e-6);
    }

    #[test]
    fn ac130_hud_blink_range() {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for ms in 0..10_000 {
            let a = blink_alpha(ms as f64, 90.0, 0.65);
            lo = lo.min(a);
            hi = hi.max(a);
        }
        assert!((lo - 0.35).abs() < 1e-3 && (hi - 1.0).abs() < 1e-6, "{lo} {hi}");
    }

    #[test]
    fn ac130_hud_timer_text() {
        assert_eq!(timer_text(40.0), "0:40");
        assert_eq!(timer_text(39.2), "0:40");
        assert_eq!(timer_text(9.0), "0:09");
        assert_eq!(timer_text(0.0), "0:00");
        assert_eq!(timer_text(-3.0), "0:00");
        assert_eq!(timer_text(119.5), "2:00");
    }

    #[test]
    fn ac130_hud_indicator_strings() {
        assert_eq!(indicator_text(&Indicator::Ready, "F7"), "AC-130 [F7] READY");
        assert_eq!(indicator_text(&Indicator::Cooldown(105.0), "F7"), "AC-130 1:45");
        assert_eq!(indicator_text(&Indicator::Unavailable(Block::NoSky), "F7"), "AC-130 - no sky");
        assert_eq!(indicator_text(&Indicator::Busy, "F7"), "AC-130 ...");
    }

    #[test]
    fn ac130_hud_projection_and_pulse() {
        let rows = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let c = project([0.0; 3], rows, 0.6, 1920.0, 1080.0, [0.0, 0.0, 10.0]).unwrap();
        assert_eq!(c, [960.0, 540.0]);
        let top = project([0.0; 3], rows, 0.6, 1920.0, 1080.0, [0.0, (0.3f32).tan() * 10.0, 10.0]).unwrap();
        assert!(top[1].abs() < 1e-3);
        assert!(project([0.0; 3], rows, 0.6, 1920.0, 1080.0, [0.0, 0.0, -1.0]).is_none());
        assert_eq!(dark_pulse(0.0, 0.6, 0.2, 0.4, 0.8), 0.0);
        assert!((dark_pulse(0.1, 0.6, 0.2, 0.4, 0.8) - 0.3).abs() < 1e-6);
        assert!((dark_pulse(0.3, 0.6, 0.2, 0.4, 0.8) - 0.6).abs() < 1e-6);
        assert!((dark_pulse(0.8, 0.6, 0.2, 0.4, 0.8) - 0.3).abs() < 1e-6);
        assert_eq!(dark_pulse(1.3, 0.6, 0.2, 0.4, 0.8), 0.0);
    }
}
