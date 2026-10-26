//! CPU reference of the M4 thermal pass (docs/M4_DESIGN.md section 6): MW2 film-tweak constants packed from a
//! `.vision`, the per-pixel transform the shader applies, a BC3 (DXT5) decoder, a D3D-style bilinear sampler and the
//! grain offsets. vmtest compares the GPU output with these functions; `convert_ac130.py` has an independent Python
//! BC3 decoder that must agree with [`bc3_decode`] byte for byte (check_m4 check 3).
//!
//! Packing as reported by iw4L (`postfx_plan.rs`) [R]:
//! `offset = brightness + 0.5 - 0.5 * contrast`; invert: `contrast = -contrast`, `offset += 1`;
//! `medium = (light + dark) / 2`; `base = dark * contrast`; `delta = 2 * contrast * (medium - dark)`;
//! `quad = contrast * (light - 2 * medium + dark)`. The per-pixel form below is our inference [I]:
//! `g = dot(c, luma)`, `cd = lerp(c, g, desat)`, `tint = base + delta * g + quad * g^2`,
//! `x = saturate(cd * tint + offset) * fade` (in stored, sRGB-encoded space, like MW2's 2D path).

use crate::ac130_data::VisionDef;

/// The packed film constants (the shader's first three float4 root-constant slots).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Film {
    pub base: [f32; 3],
    pub offset: f32,
    pub delta: [f32; 3],
    pub desat: f32,
    pub quad: [f32; 3],
}

impl Film {
    /// Everything black (MW2 `black_bw`: contrast 0, brightness -1).
    pub const BLACK: Film = Film { base: [0.0; 3], offset: -0.5, delta: [0.0; 3], desat: 1.0, quad: [0.0; 3] };
}

pub fn pack(v: &VisionDef) -> Film {
    let mut c = v.contrast;
    let mut offset = v.brightness + 0.5 - 0.5 * c;
    if v.invert != 0 {
        c = -c;
        offset += 1.0;
    }
    let (l, d) = (v.light_tint, v.dark_tint);
    let m = [(l[0] + d[0]) * 0.5, (l[1] + d[1]) * 0.5, (l[2] + d[2]) * 0.5];
    Film {
        base: [d[0] * c, d[1] * c, d[2] * c],
        offset,
        delta: [0, 1, 2].map(|i| 2.0 * c * (m[i] - d[i])),
        desat: v.desaturation,
        quad: [0, 1, 2].map(|i| c * (l[i] - 2.0 * m[i] + d[i])),
    }
}

/// HLSL `lerp` (`x + s * (y - x)`), used per pixel exactly as the shader does.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Endpoint-exact blend for the film constants (t = 1 gives `b` bit for bit).
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// Linear blend of the packed constants (the black-hot / white-hot transition).
pub fn lerp_film(a: &Film, b: &Film, t: f32) -> Film {
    let t = t.clamp(0.0, 1.0);
    Film {
        base: [0, 1, 2].map(|i| mix(a.base[i], b.base[i], t)),
        offset: mix(a.offset, b.offset, t),
        delta: [0, 1, 2].map(|i| mix(a.delta[i], b.delta[i], t)),
        desat: mix(a.desat, b.desat, t),
        quad: [0, 1, 2].map(|i| mix(a.quad[i], b.quad[i], t)),
    }
}

/// The film at `now` of a transition from `from` to `to` that started at `t0` and lasts `dur` seconds.
pub fn film_at(from: &Film, to: &Film, t0: f64, dur: f64, now: f64) -> Film {
    if dur <= 0.0 || now >= t0 + dur {
        return *to;
    }
    lerp_film(from, to, ((now - t0) / dur).clamp(0.0, 1.0) as f32)
}

/// The fade-in multiplier (0 at the start of a ride, 1 after `fade_s`).
pub fn fade_at(t_since_start: f64, fade_s: f64) -> f32 {
    if fade_s <= 0.0 { 1.0 } else { (t_since_start / fade_s).clamp(0.0, 1.0) as f32 }
}

/// Rec. 709 luma weights (the default `[ac130] luma_weights`).
pub const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Pre-exposure of the film input (review M4 major 8) [I]: MW2's film tweak ran on a thermal-material render (cold
/// world dark, heat bright); here it runs on ER's lit colour frame, where `1.9 g + 0.05` clips every pixel with
/// luma > 0.5 to white. Scaling the luma weights by `exposure` scales `g` before the film step. With
/// `desaturation = 1` (all four MW2 visions) `cd = g`, so this is an exact pre-exposure; the shader is unchanged
/// (the weights are root constants).
pub fn exposed_luma(luma: [f32; 3], exposure: f32) -> [f32; 3] {
    luma.map(|w| w * exposure)
}

/// Mid-tone lift of the film input (run 8 feedback "a little bit too dark", [I]): `g = (c . luma)^gamma` after the
/// pre-exposure, before MW2's film step. gamma < 1 brightens the mid-tones much more than the highlights (0 and 1 stay
/// fixed), so the image reads brighter without moving the white-clip point as far as a bigger exposure would.
/// 1 = MW2's film as is.
pub fn lift(g: f32, gamma: f32) -> f32 {
    if gamma == 1.0 { g } else { g.max(0.0).powf(gamma) }
}

/// Fractions of `src` pixels the film alone maps to pure black (every channel 0) and pure white (every channel 1).
pub fn clip_fractions(src: &[[u8; 3]], f: &Film, luma: [f32; 3]) -> (f64, f64) {
    clip_fractions_g(src, f, luma, 1.0)
}

/// `clip_fractions` with the mid-tone `gamma` (see [`lift`]).
pub fn clip_fractions_g(src: &[[u8; 3]], f: &Film, luma: [f32; 3], gamma: f32) -> (f64, f64) {
    if src.is_empty() {
        return (0.0, 0.0);
    }
    let (mut lo, mut hi) = (0usize, 0usize);
    for c in src {
        let x = shade_pixel_g(c.map(|v| v as f32 / 255.0), f, luma, gamma, 1.0);
        if x.iter().all(|&v| v <= 0.0) {
            lo += 1;
        } else if x.iter().all(|&v| v >= 1.0) {
            hi += 1;
        }
    }
    (lo as f64 / src.len() as f64, hi as f64 / src.len() as f64)
}

/// The film transform of one pixel (stored values 0..1 in, 0..1 out).
pub fn shade_pixel(c: [f32; 3], f: &Film, luma: [f32; 3], fade: f32) -> [f32; 3] {
    shade_pixel_g(c, f, luma, 1.0, fade)
}

/// `shade_pixel` with the mid-tone `gamma` (the shader's `pow(max(g, 0), gamma)`).
pub fn shade_pixel_g(c: [f32; 3], f: &Film, luma: [f32; 3], gamma: f32, fade: f32) -> [f32; 3] {
    let g = lift(c[0] * luma[0] + c[1] * luma[1] + c[2] * luma[2], gamma);
    [0, 1, 2].map(|i| {
        let cd = lerp(c[i], g, f.desat);
        let tint = f.base[i] + f.delta[i] * g + f.quad[i] * g * g;
        (cd * tint + f.offset).clamp(0.0, 1.0) * fade
    })
}

/// The full per-pixel composition of the pass: film, then the gun overlay (premultiplied by `ov_alpha`), then the
/// grain (`tint * grain.rgb` blended by `grain.a * grain_alpha`). `ov` / `grain` are `None` outside the 4:3 box or
/// with grain off.
pub fn compose(
    c: [f32; 3],
    f: &Film,
    luma: [f32; 3],
    fade: f32,
    ov: Option<[f32; 4]>,
    ov_alpha: f32,
    grain: Option<[f32; 4]>,
    grain_tint: [f32; 3],
    grain_alpha: f32,
) -> [f32; 3] {
    compose_g(c, f, luma, 1.0, fade, ov, ov_alpha, grain, grain_tint, grain_alpha)
}

/// `compose` with the mid-tone `gamma`.
#[allow(clippy::too_many_arguments)]
pub fn compose_g(
    c: [f32; 3],
    f: &Film,
    luma: [f32; 3],
    gamma: f32,
    fade: f32,
    ov: Option<[f32; 4]>,
    ov_alpha: f32,
    grain: Option<[f32; 4]>,
    grain_tint: [f32; 3],
    grain_alpha: f32,
) -> [f32; 3] {
    let mut x = shade_pixel_g(c, f, luma, gamma, fade);
    if let Some(o) = ov {
        let a = o[3] * ov_alpha;
        x = [0, 1, 2].map(|i| lerp(x[i], o[i], a));
    }
    if let Some(g) = grain {
        let a = g[3] * grain_alpha;
        x = [0, 1, 2].map(|i| lerp(x[i], grain_tint[i] * g[i], a));
    }
    x
}

// ---- BC3 (DXT5) -------------------------------------------------------------------------------------------

fn expand565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1f) as u8;
    let g = ((c >> 5) & 0x3f) as u8;
    let b = (c & 0x1f) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// The 8 alpha palette entries of a BC3 alpha block (integer rounding; the Python decoder uses the same rule).
pub fn bc3_alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let (a, b) = (a0 as u32, a1 as u32);
    let mut p = [a0, a1, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for i in 1..7u32 {
            p[i as usize + 1] = (((7 - i) * a + i * b + 3) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            p[i as usize + 1] = (((5 - i) * a + i * b + 2) / 5) as u8;
        }
        p[6] = 0;
        p[7] = 255;
    }
    p
}

/// The 4 colour palette entries of a BC3 colour block (always the 4-colour mode in BC2/BC3).
pub fn bc3_color_palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
    let (a, b) = (expand565(c0), expand565(c1));
    let mix = |x: u8, y: u8| ((2 * x as u32 + y as u32 + 1) / 3) as u8;
    [a, b, [0, 1, 2].map(|i| mix(a[i], b[i])), [0, 1, 2].map(|i| mix(b[i], a[i]))]
}

/// Decodes a BC3 mip of `w` x `h` (multiples of 4) into tightly packed RGBA8.
pub fn bc3_decode(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 || w % 4 != 0 || h % 4 != 0 {
        return Err(format!("bc3 {w}x{h}: sizes must be non-zero multiples of 4"));
    }
    let (bw, bh) = ((w / 4) as usize, (h / 4) as usize);
    if data.len() != bw * bh * 16 {
        return Err(format!("bc3 {w}x{h}: {} bytes (want {})", data.len(), bw * bh * 16));
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let b = &data[(by * bw + bx) * 16..(by * bw + bx) * 16 + 16];
            let ap = bc3_alpha_palette(b[0], b[1]);
            let abits = b[2..8].iter().enumerate().fold(0u64, |acc, (i, &v)| acc | ((v as u64) << (8 * i)));
            let c0 = u16::from_le_bytes([b[8], b[9]]);
            let c1 = u16::from_le_bytes([b[10], b[11]]);
            let cp = bc3_color_palette(c0, c1);
            let cbits = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
            for py in 0..4 {
                for px in 0..4 {
                    let k = py * 4 + px;
                    let ci = ((cbits >> (2 * k)) & 3) as usize;
                    let ai = ((abits >> (3 * k)) & 7) as usize;
                    let x = bx * 4 + px;
                    let y = by * 4 + py;
                    let o = (y * w as usize + x) * 4;
                    out[o..o + 3].copy_from_slice(&cp[ci]);
                    out[o + 3] = ap[ai];
                }
            }
        }
    }
    Ok(out)
}

// ---- sampling ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Address {
    Clamp,
    Wrap,
}

/// D3D-style bilinear filtering of an RGBA8 image at normalized `(u, v)`: texel centres at `(i + 0.5) / w`, the
/// sub-texel fraction quantised to 8 bits (the D3D11+ minimum precision), result in 0..1.
pub fn bilinear(img: &[u8], w: u32, h: u32, u: f32, v: f32, addr: Address) -> [f32; 4] {
    let fx = u * w as f32 - 0.5;
    let fy = v * h as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let q = |f: f32| (f * 256.0).round() / 256.0;
    let (ax, ay) = (q(fx - x0), q(fy - y0));
    let idx = |x: i64, n: u32| -> usize {
        match addr {
            Address::Clamp => x.clamp(0, n as i64 - 1) as usize,
            Address::Wrap => x.rem_euclid(n as i64) as usize,
        }
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let px = |x: i64, y: i64| -> [f32; 4] {
        let i = (idx(y, h) * w as usize + idx(x, w)) * 4;
        [0, 1, 2, 3].map(|k| img[i + k] as f32 / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    [0, 1, 2, 3].map(|k| {
        let top = a[k] + (b[k] - a[k]) * ax;
        let bot = c[k] + (d[k] - c[k]) * ax;
        top + (bot - top) * ay
    })
}

/// Pixel `(px, py)` (integer pixel, centre at +0.5) of a `w` x `h` target inside the 4:3 overlay box
/// `[x0, y0, bw, bh]`: the overlay uv, or `None` outside the box.
pub fn box_uv(px: u32, py: u32, b: [f32; 4]) -> Option<[f32; 2]> {
    let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
    let u = (x - b[0]) / b[2];
    let v = (y - b[1]) / b[3];
    ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)).then_some([u, v])
}

// ---- grain ------------------------------------------------------------------------------------------------

/// Grain changes 30 times a second [I: MW2's grain_overlay animation is in the engine shader].
pub const GRAIN_HZ: f64 = 30.0;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The whole-texel uv offset of the grain texture (`size` texels square) for animation frame `frame`.
pub fn grain_offset(frame: u64, size: u32) -> [f32; 2] {
    let h = splitmix(frame);
    let n = size.max(1) as u64;
    [((h % n) as f32) / size as f32, (((h >> 32) % n) as f32) / size as f32]
}

/// The animation frame at `t` seconds.
pub fn grain_frame(t: f64) -> u64 {
    (t.max(0.0) * GRAIN_HZ) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vis(c: f32, b: f32, inv: u32, light: [f32; 3], dark: [f32; 3]) -> VisionDef {
        VisionDef { contrast: c, brightness: b, desaturation: 1.0, invert: inv, light_tint: light, dark_tint: dark }
    }

    #[test]
    fn thermal_film_packing_known_answers() {
        // ac130.vision: contrast 1.9, brightness 0.5 -> out = 1.9 g + 0.05
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        assert!((w.offset - 0.05).abs() < 1e-6 && (w.base[0] - 1.9).abs() < 1e-6 && w.delta == [0.0; 3] && w.quad == [0.0; 3]);
        // ac130_inverted.vision: 1.55 / 0.13 / invert -> out = -1.55 g + 0.855
        let b = pack(&vis(1.55, 0.13, 1, [1.0; 3], [1.0; 3]));
        assert!((b.offset - 0.855).abs() < 1e-6 && (b.base[1] + 1.55).abs() < 1e-6);
        // thermal_mp: 1 / 0 -> out = g
        let t = pack(&vis(1.0, 0.0, 0, [1.0; 3], [1.0; 3]));
        assert!(t.offset.abs() < 1e-6 && (t.base[2] - 1.0).abs() < 1e-6);
        // missilecam: lightTint 0, darkTint 1.21215 1.21039 1.31139, contrast 3.7, brightness 1
        let m = pack(&vis(3.7, 1.0, 0, [0.0; 3], [1.21215, 1.21039, 1.31139]));
        assert!((m.offset + 0.35).abs() < 1e-6);
        assert!((m.base[0] - 3.7 * 1.21215).abs() < 1e-5 && (m.delta[0] + 3.7 * 1.21215).abs() < 1e-5 && m.quad[0].abs() < 1e-5);
        // black_bw: everything 0
        let k = pack(&vis(0.0, -1.0, 0, [1.0; 3], [1.0; 3]));
        assert_eq!(k, Film { base: [0.0; 3], offset: -0.5, delta: [0.0; 3], desat: 1.0, quad: [0.0; 3] });
    }

    #[test]
    fn thermal_shade_pixel_known_answers() {
        let g = 0.35f32;
        let grey = [g, g, g];
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        let b = pack(&vis(1.55, 0.13, 1, [1.0; 3], [1.0; 3]));
        let k = pack(&vis(0.0, -1.0, 0, [1.0; 3], [1.0; 3]));
        let out = shade_pixel(grey, &w, LUMA_709, 1.0);
        assert!((out[0] - 0.715).abs() < 1e-5 && out[0] == out[1] && out[1] == out[2], "{out:?}");
        assert!((shade_pixel(grey, &b, LUMA_709, 1.0)[0] - 0.3125).abs() < 1e-5);
        assert_eq!(shade_pixel(grey, &k, LUMA_709, 1.0), [0.0; 3]);
        assert_eq!(shade_pixel(grey, &w, LUMA_709, 0.0), [0.0; 3], "fade 0 = black");
        // a saturated colour becomes grey (desaturation 1) and clamps at 1
        let c = shade_pixel([1.0, 0.0, 0.0], &w, LUMA_709, 1.0);
        assert!(c[0] == c[1] && c[1] == c[2] && (c[0] - (1.9 * 0.2126 + 0.05)).abs() < 1e-5);
        assert_eq!(shade_pixel([1.0; 3], &w, LUMA_709, 1.0), [1.0; 3]);
    }

    #[test]
    fn thermal_exposure_scales_film_input() {
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        // exposure 1 = MW2 as is: luma 0.6 clips
        assert_eq!(shade_pixel([0.6; 3], &w, exposed_luma(LUMA_709, 1.0), 1.0), [1.0; 3]);
        // exposure 0.5: 1.9 * 0.3 + 0.05 = 0.62
        let x = shade_pixel([0.6; 3], &w, exposed_luma(LUMA_709, 0.5), 1.0);
        assert!((x[0] - 0.62).abs() < 1e-5, "{x:?}");
        // desaturation 1: scaling the weights == scaling the colour
        let c = [0.3, 0.5, 0.2];
        let a = shade_pixel(c, &w, exposed_luma(LUMA_709, 0.7), 1.0);
        let b = shade_pixel(c.map(|v| v * 0.7), &w, LUMA_709, 1.0);
        assert!((a[0] - b[0]).abs() < 1e-6);
        // clip fractions: a ramp 0..255 -> white above luma ~0.5 at exposure 1, none at exposure 0.45
        let ramp: Vec<[u8; 3]> = (0..=255u8).map(|v| [v; 3]).collect();
        let (lo, hi) = clip_fractions(&ramp, &w, LUMA_709);
        assert!(lo == 0.0 && (hi - 0.5).abs() < 0.02, "{lo} {hi}");
        let (_, hi2) = clip_fractions(&ramp, &w, exposed_luma(LUMA_709, 0.45));
        assert!(hi2 < 0.01, "{hi2}");
        assert_eq!(clip_fractions(&[], &w, LUMA_709), (0.0, 0.0));
    }

    #[test]
    fn thermal_gamma_lifts_midtones() {
        // white hot: x = 1.9 g + 0.05 for grey input (base 1.9, desaturation 1)
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        let mid = [0.25f32; 3];
        assert_eq!(shade_pixel_g(mid, &w, LUMA_709, 1.0, 1.0), shade_pixel(mid, &w, LUMA_709, 1.0), "gamma 1 = MW2");
        // run-8 defaults: exposure 0.52, gamma 0.75 -> g = (0.52 * 0.25)^0.75
        let l = exposed_luma(LUMA_709, 0.52);
        let x = shade_pixel_g(mid, &w, l, 0.75, 1.0)[0];
        let want = 1.9 * (0.52f32 * 0.25).powf(0.75) + 0.05;
        assert!((x - want).abs() < 1e-5, "{x} {want}");
        let old = shade_pixel(mid, &w, exposed_luma(LUMA_709, 0.6), 1.0)[0];
        assert!(x > old * 1.3, "mid-tone 0.25 must read clearly brighter than at the old default: {x} vs {old}");
        // black stays black, white stays white, the curve is monotonic
        assert!((shade_pixel_g([0.0; 3], &w, l, 0.75, 1.0)[0] - 0.05).abs() < 1e-6);
        let ramp: Vec<f32> = (0..=100).map(|i| shade_pixel_g([i as f32 / 100.0; 3], &w, l, 0.75, 1.0)[0]).collect();
        assert!(ramp.windows(2).all(|p| p[1] >= p[0]));
        // the white-clip point moves less than an equal mid-tone lift by exposure alone would move it
        let ramp8: Vec<[u8; 3]> = (0..=255u8).map(|v| [v; 3]).collect();
        let (_, hi_g) = clip_fractions_g(&ramp8, &w, l, 0.75);
        let e_eq = (0.52f32 * 0.25).powf(0.75) / 0.25; // the exposure giving the same mid-tone without gamma
        let (_, hi_e) = clip_fractions(&ramp8, &w, exposed_luma(LUMA_709, e_eq));
        assert!(hi_g < hi_e, "gamma {hi_g} vs exposure-only {hi_e}");
        assert_eq!(lift(-0.1, 0.8), 0.0);
    }

    #[test]
    fn thermal_toggle_lerp_endpoints() {
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        let b = pack(&vis(1.55, 0.13, 1, [1.0; 3], [1.0; 3]));
        assert_eq!(film_at(&w, &b, 10.0, 0.62, 10.0), w);
        assert_eq!(film_at(&w, &b, 10.0, 0.62, 10.62), b);
        assert_eq!(film_at(&w, &b, 10.0, 0.62, 99.0), b);
        let mid = film_at(&w, &b, 10.0, 0.62, 10.31);
        assert!((mid.offset - (w.offset + b.offset) * 0.5).abs() < 1e-5);
        assert_eq!(lerp_film(&w, &b, 2.0), b, "clamped");
        assert_eq!(fade_at(0.0, 1.0), 0.0);
        assert_eq!(fade_at(0.5, 1.0), 0.5);
        assert_eq!(fade_at(3.0, 1.0), 1.0);
        assert_eq!(fade_at(0.0, 0.0), 1.0);
    }

    #[test]
    fn thermal_bc3_decoder_hand_block() {
        // alpha 255/0 (8-value mode), indices 0..7 then 1; colour white/black, index pattern 0,1,2,3
        let mut blk = [0u8; 16];
        blk[0] = 255;
        blk[1] = 0;
        let mut ab: u64 = 0;
        for k in 0..16u64 {
            ab |= (k % 8) << (3 * k);
        }
        blk[2..8].copy_from_slice(&ab.to_le_bytes()[..6]);
        blk[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        blk[10..12].copy_from_slice(&0x0000u16.to_le_bytes());
        let mut cb: u32 = 0;
        for k in 0..16u32 {
            cb |= (k % 4) << (2 * k);
        }
        blk[12..16].copy_from_slice(&cb.to_le_bytes());
        let px = bc3_decode(&blk, 4, 4).unwrap();
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
        assert_eq!(&px[8..12], &[170, 170, 170, 219], "2/3 white, alpha (6*255+0+3)/7");
        assert_eq!(&px[12..16], &[85, 85, 85, 182]);
        assert_eq!(bc3_alpha_palette(0, 255), [0, 255, 51, 102, 153, 204, 0, 255]);
        assert!(bc3_decode(&blk, 4, 8).is_err());
        assert!(bc3_decode(&blk, 3, 4).is_err());
    }

    #[test]
    fn thermal_bilinear_reference_2x2() {
        // 2x2 image: (0,0)=0, (1,0)=255, (0,1)=255, (1,1)=0 in red
        let mut img = vec![0u8; 16];
        img[4] = 255;
        img[8] = 255;
        for k in 0..4 {
            img[k * 4 + 3] = 255;
        }
        // texel centres sample exactly
        assert_eq!(bilinear(&img, 2, 2, 0.25, 0.25, Address::Clamp)[0], 0.0);
        assert_eq!(bilinear(&img, 2, 2, 0.75, 0.25, Address::Clamp)[0], 1.0);
        // the centre of the image is the average of the four: 0.5
        assert!((bilinear(&img, 2, 2, 0.5, 0.5, Address::Clamp)[0] - 0.5).abs() < 1e-6);
        // a quarter of the way from texel (0,0) towards (1,0): 0.25
        assert!((bilinear(&img, 2, 2, 0.375, 0.25, Address::Clamp)[0] - 0.25).abs() < 1e-6);
        // clamp vs wrap at the left edge
        assert_eq!(bilinear(&img, 2, 2, 0.0, 0.25, Address::Clamp)[0], 0.0);
        assert!((bilinear(&img, 2, 2, 0.0, 0.25, Address::Wrap)[0] - 0.5).abs() < 1e-6);
        assert_eq!(box_uv(479, 10, [480.0, 0.0, 2880.0, 2160.0]), None);
        let uv = box_uv(480, 0, [480.0, 0.0, 2880.0, 2160.0]).unwrap();
        assert!((uv[0] - 0.5 / 2880.0).abs() < 1e-7);
    }

    #[test]
    fn thermal_grain_offsets_deterministic() {
        let a: Vec<[f32; 2]> = (0..100).map(|f| grain_offset(f, 512)).collect();
        let b: Vec<[f32; 2]> = (0..100).map(|f| grain_offset(f, 512)).collect();
        assert_eq!(a, b);
        for o in &a {
            assert!((0.0..1.0).contains(&o[0]) && (0.0..1.0).contains(&o[1]));
            assert_eq!((o[0] * 512.0).fract(), 0.0, "whole texels");
        }
        assert!(a.windows(2).filter(|w| w[0] != w[1]).count() > 90, "changes every frame");
        assert_eq!(grain_frame(1.0), 30);
        assert_eq!(grain_frame(-1.0), 0);
        // compose: overlay alpha 0 and no grain = the film alone
        let w = pack(&vis(1.9, 0.5, 0, [1.0; 3], [1.0; 3]));
        let c = [0.2, 0.4, 0.6];
        assert_eq!(compose(c, &w, LUMA_709, 1.0, Some([1.0, 1.0, 1.0, 0.0]), 1.0, None, [0.9, 1.0, 1.0], 0.3), shade_pixel(c, &w, LUMA_709, 1.0));
        let full = compose(c, &w, LUMA_709, 1.0, Some([1.0, 0.0, 0.0, 1.0]), 1.0, None, [0.9, 1.0, 1.0], 0.3);
        assert_eq!(full, [1.0, 0.0, 0.0]);
    }
}
