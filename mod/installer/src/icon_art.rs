//! The app icon, drawn in code (no image file in the repository): a dark rounded square, a gold ring (Elden Ring)
//! and a green scope crosshair (MW2's thermal sight). Shared by `build.rs` (the exe's icon resources, one image per
//! size) and the setup window (its title-bar / taskbar icon), so both always match.

/// Straight-alpha RGBA pixels, top row first, `size` x `size` (4 x 4 supersampling per pixel).
pub fn rgba(size: u32) -> Vec<u8> {
    const N: u32 = 4;
    let s = size.max(1);
    let mut out = Vec::with_capacity((s * s * 4) as usize);
    for py in 0..s {
        for px in 0..s {
            let mut acc = [0f32; 4];
            for sy in 0..N {
                for sx in 0..N {
                    let x = (px as f32 + (sx as f32 + 0.5) / N as f32) / s as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / N as f32) / s as f32;
                    let c = sample(x - 0.5, y - 0.5, s);
                    for i in 0..4 {
                        acc[i] += c[i];
                    }
                }
            }
            let k = (N * N) as f32;
            let a = acc[3] / k;
            let px_rgba = if a > 0.0 {
                [acc[0] / k / a, acc[1] / k / a, acc[2] / k / a, a]
            } else {
                [0.0; 4]
            };
            out.extend(px_rgba.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
        }
    }
    out
}

const BG: [f32; 3] = [0.10, 0.11, 0.12];
const EDGE: [f32; 3] = [0.45, 0.36, 0.14];
const GOLD: [f32; 3] = [0.86, 0.69, 0.27];
const GREEN: [f32; 3] = [0.49, 0.95, 0.42];

/// An opaque sample of `col` (each sample is fully inside or outside a shape; the 4 x 4 supersampling smooths edges).
fn solid(col: [f32; 3]) -> [f32; 4] {
    [col[0], col[1], col[2], 1.0]
}

/// One sample at (x, y), centred coordinates in -0.5..0.5; premultiplied RGBA.
fn sample(x: f32, y: f32, size: u32) -> [f32; 4] {
    // thin features get at least ~1 pixel at the small sizes
    let px = 1.0 / size as f32;
    // rounded square (signed distance)
    let half = 0.47;
    let rad = 0.16;
    let qx = x.abs() - (half - rad);
    let qy = y.abs() - (half - rad);
    let d = (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - rad;
    if d > 0.0 {
        return [0.0; 4];
    }
    let mut c = solid(if d > -0.035_f32.max(px) { EDGE } else { BG });
    let r = x.hypot(y);
    // the gold ring
    if (r - 0.255).abs() < 0.05_f32.max(px * 0.75) {
        c = solid(GOLD);
    }
    // crosshair: four ticks with a gap in the middle, and a centre dot
    let w = 0.028_f32.max(px * 0.55);
    let (ax, ay) = (x.abs(), y.abs());
    let tick = |along: f32, across: f32| across < w && (0.10..0.40).contains(&along);
    if tick(ay, ax) || tick(ax, ay) {
        c = solid(GREEN);
    }
    if r < 0.045_f32.max(px * 1.1) {
        c = solid(GREEN);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_corners_and_centre() {
        for s in [16u32, 32, 48, 64, 256] {
            let p = rgba(s);
            assert_eq!(p.len(), (s * s * 4) as usize);
            // transparent corners, opaque green centre, gold on the ring
            assert_eq!(p[3], 0, "corner alpha at {s}");
            let at = |x: u32, y: u32| &p[((y * s + x) * 4) as usize..((y * s + x) * 4 + 4) as usize];
            let c = at(s / 2, s / 2);
            assert!(c[3] == 255 && c[1] > 150 && c[1] > c[0] + 40, "centre {c:?} at {s}");
            let g = at(s / 2 + (s as f32 * 0.255) as u32, s / 2 + (s as f32 * 0.08) as u32);
            assert!(g[0] > 120 && g[0] > g[2] + 60, "ring {g:?} at {s}");
        }
    }
}
