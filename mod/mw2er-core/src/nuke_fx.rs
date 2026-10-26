//! M7 nuke post pass: CPU reference and constant packing (docs/M7_DESIGN.md 3.7). `mw2er-render::nuke` runs the same
//! math on the GPU (shaders/nuke.hlsl); vmtest compares the two.
//!
//! Per frame (`PostState` from `nuke_logic`): the vision = `mpnuke` blended towards `mpnuke_aftermath` by `blend`
//! (glow numbers linearly, the film through the packed constants of `thermal::pack`, the AC-130 film model [I]).
//! 1. bloom: the scene box-filtered to 1/8 resolution, bright-passed (`k = max(l - cutoff, 0) / l`, luma Rec. 709),
//!    desaturated by `r_glowBloomDesaturation`; read back with a 9-tap tent whose spacing follows `r_glowRadius0`
//!    (640x480 virtual pixels scaled to the target height);
//! 2. `c = saturate(scene + bloom x r_glowBloomIntensity0)`;
//! 3. film (`thermal::shade_pixel`: desaturation, contrast/brightness, dark -> light tint by luma);
//! 4. `out = lerp(scene, film, amount)`, then `out = lerp(out, 1, flash)` (the white flash).
//! With `amount = 0` and `flash = 0` the output equals the input.

use crate::nuke_data::{NukeFile, Vision};
use crate::nuke_logic::PostState;
use crate::thermal::{Film, LUMA_709, lerp_film, pack, shade_pixel};

/// The bloom texture is 1/BLOOM_DIV of the target in each axis.
pub const BLOOM_DIV: u32 = 8;
/// Root constants of the composite pass (c0..c5).
pub const NUKE_CONSTANTS: usize = 24;

/// The blended vision of one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vis {
    pub film: Film,
    pub cutoff: f32,
    pub glow_desat: f32,
    pub intensity: f32,
    pub radius: f32,
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

fn film_def(v: &Vision) -> crate::ac130_data::VisionDef {
    crate::ac130_data::VisionDef {
        contrast: v.film_contrast,
        brightness: v.film_brightness,
        desaturation: v.film_desaturation,
        invert: v.film_invert as u32,
        light_tint: v.film_light_tint,
        dark_tint: v.film_dark_tint,
    }
}

pub fn blend_visions(a: &Vision, b: &Vision, t: f32) -> Vis {
    let t = t.clamp(0.0, 1.0);
    Vis {
        film: lerp_film(&pack(&film_def(a)), &pack(&film_def(b)), t),
        cutoff: mix(a.glow_cutoff, b.glow_cutoff, t),
        glow_desat: mix(a.glow_desaturation, b.glow_desaturation, t),
        intensity: mix(a.glow_intensity, b.glow_intensity, t),
        radius: mix(a.glow_radius, b.glow_radius, t),
    }
}

/// The frame's vision from the converted file.
pub fn vis_at(f: &NukeFile, post: &PostState) -> Vis {
    blend_visions(&f.vision("mpnuke"), &f.vision("mpnuke_aftermath"), post.blend)
}

fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA_709[0] + c[1] * LUMA_709[1] + c[2] * LUMA_709[2]
}

/// The bright pass of one (downsampled) colour.
pub fn bright(c: [f32; 3], cutoff: f32, desat: f32) -> [f32; 3] {
    let l = luma(c);
    let k = (l - cutoff).max(0.0) / l.max(1e-4);
    let b = c.map(|x| x * k);
    let lb = luma(b);
    b.map(|x| x + (lb - x) * desat)
}

/// The composite of one pixel given its bloom sample (CPU reference of `ps_nuke`).
pub fn composite(scene: [f32; 3], bloom: [f32; 3], v: &Vis, post: &PostState) -> [f32; 3] {
    let c = [0, 1, 2].map(|i| (scene[i] + bloom[i] * v.intensity).clamp(0.0, 1.0));
    let film = shade_pixel(c, &v.film, LUMA_709, 1.0);
    let a = post.amount.clamp(0.0, 1.0);
    let f = post.flash.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| {
        let x = scene[i] + (film[i] - scene[i]) * a;
        x + (1.0 - x) * f
    })
}

/// A uniform input image: the bloom is the bright pass of the colour itself (every downsample and tap agree).
pub fn uniform(c: [f32; 3], v: &Vis, post: &PostState) -> [f32; 3] {
    composite(c, bright(c, v.cutoff, v.glow_desat), v, post)
}

/// Tap spacing of the bloom read in bloom texels for a target of `height` pixels.
pub fn tap_spacing(radius: f32, height: u32) -> f32 {
    (radius * height as f32 / 480.0 / BLOOM_DIV as f32).clamp(0.5, 16.0)
}

/// The root constants of the passes (shader layout):
/// c0 = film base.rgb, offset; c1 = delta.rgb, desat; c2 = quad.rgb, amount; c3 = cutoff, glow desat, intensity,
/// flash; c4 = bloom texel size (1 / bloom w, 1 / bloom h), tap spacing, 0; c5 = target w, h, bloom w, bloom h.
pub fn constants(v: &Vis, post: &PostState, w: u32, h: u32) -> [f32; NUKE_CONSTANTS] {
    let (bw, bh) = bloom_size(w, h);
    let f = &v.film;
    let mut c = [0.0f32; NUKE_CONSTANTS];
    c[0..3].copy_from_slice(&f.base);
    c[3] = f.offset;
    c[4..7].copy_from_slice(&f.delta);
    c[7] = f.desat;
    c[8..11].copy_from_slice(&f.quad);
    c[11] = post.amount.clamp(0.0, 1.0);
    c[12] = v.cutoff;
    c[13] = v.glow_desat;
    c[14] = v.intensity;
    c[15] = post.flash.clamp(0.0, 1.0);
    c[16] = 1.0 / bw as f32;
    c[17] = 1.0 / bh as f32;
    c[18] = tap_spacing(v.radius, h);
    c[20] = w as f32;
    c[21] = h as f32;
    c[22] = bw as f32;
    c[23] = bh as f32;
    c
}

pub fn bloom_size(w: u32, h: u32) -> (u32, u32) {
    (w.div_ceil(BLOOM_DIV).max(1), h.div_ceil(BLOOM_DIV).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuke_data::tests::fixture;

    fn full(blend: f32, amount: f32, flash: f32) -> (Vis, PostState) {
        let f = fixture();
        let p = PostState { flash, amount, blend };
        (vis_at(&f, &p), p)
    }

    #[test]
    fn nuke_fx_known_answers() {
        // black input + flash 1 -> white
        let (v, p) = full(0.0, 0.0, 1.0);
        assert_eq!(uniform([0.0; 3], &v, &p), [1.0; 3]);
        // grey 0.5 with mpnuke full -> orange-white: R >= G >= B, R >= 0.95, B well below R
        let (v, p) = full(0.0, 1.0, 0.0);
        let o = uniform([0.5; 3], &v, &p);
        assert!(o[0] >= o[1] && o[1] >= o[2] && o[0] >= 0.95 && o[2] <= o[0] - 0.1, "mpnuke on grey: {o:?}");
        // the aftermath on grey: shadows R > G > B (the fixture's orange dark tint)
        let (v, p) = full(1.0, 1.0, 0.0);
        let o = uniform([0.5; 3], &v, &p);
        assert!(o[0] > o[1] && o[1] > o[2], "aftermath on grey: {o:?}");
        // flash 0 and amount 0: output == input
        let (v, p) = full(0.3, 0.0, 0.0);
        for c in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.9], [1.0, 1.0, 1.0]] {
            assert_eq!(composite(c, [0.7, 0.7, 0.7], &v, &p), c);
        }
    }

    #[test]
    fn nuke_fx_bright_pass_and_constants() {
        assert_eq!(bright([0.2; 3], 0.315552, 0.0), [0.0; 3], "below the cutoff");
        let b = bright([1.0, 0.0, 0.0], 0.1, 1.0);
        assert!((b[0] - b[1]).abs() < 1e-6, "fully desaturated");
        let (v, p) = full(0.5, 0.75, 0.25);
        let c = constants(&v, &p, 3840, 2160);
        assert_eq!((c[11], c[15]), (0.75, 0.25));
        assert_eq!((c[20], c[21], c[22], c[23]), (3840.0, 2160.0, 480.0, 270.0));
        assert!((c[16] - 1.0 / 480.0).abs() < 1e-9);
        // r_glowRadius0 3.13 (640x480 pixels) at 2160: 14.1 px = 1.76 bloom texels
        assert!((tap_spacing(3.13324, 2160) - 1.762).abs() < 1e-3);
        assert_eq!(bloom_size(1, 1), (1, 1));
        assert_eq!(bloom_size(1921, 1081), (241, 136));
    }
}
