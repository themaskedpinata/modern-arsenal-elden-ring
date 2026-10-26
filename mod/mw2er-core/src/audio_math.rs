//! Sample math for the audio thread (pure, so it is tested here rather than in the DLL).
//!
//! The UMP45 fire WAV is 2.5-3 s with its echo tail; at 11 shots/s that would stack ~30 voices.
//! It is truncated once at load to `fire_max_ms` with a linear fade over the last `fire_fade_ms`
//! (rodio's `TakeDuration` fade spans the whole take and would soften the attack).

/// Truncates interleaved samples to `max_ms` and fades the last `fade_ms` linearly to zero.
/// Returns the samples unchanged when they are already shorter than `max_ms`.
pub fn truncate_with_fade(samples: &mut Vec<f32>, channels: u16, sample_rate: u32, max_ms: u32, fade_ms: u32) {
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    let max_frames = (sample_rate as u64 * max_ms as u64 / 1000) as usize;
    if max_frames == 0 || frames <= max_frames {
        return;
    }
    samples.truncate(max_frames * ch);
    let fade = ((sample_rate as u64 * fade_ms as u64 / 1000) as usize).min(max_frames);
    if fade == 0 {
        return;
    }
    let start = max_frames - fade;
    for f in start..max_frames {
        let g = (max_frames - f) as f32 / fade as f32; // 1.0 .. 1/fade
        let g = g - 1.0 / fade as f32; // reaches exactly 0 on the last frame
        for c in 0..ch {
            samples[f * ch + c] *= g.max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_truncate_and_fade() {
        // 1 s stereo at 1 kHz of ones -> 600 ms with an 80 ms fade
        let mut s = vec![1.0f32; 2 * 1000];
        truncate_with_fade(&mut s, 2, 1000, 600, 80);
        assert_eq!(s.len(), 2 * 600);
        assert_eq!(s[0], 1.0, "attack untouched");
        assert_eq!(s[2 * 519], 1.0, "untouched before the fade");
        assert!(s[2 * 520] < 1.0 && s[2 * 520] > 0.95);
        assert_eq!(s[2 * 599], 0.0, "silent last frame");
        assert_eq!(s[2 * 599 + 1], 0.0);
        for f in 521..600 {
            assert!(s[2 * f] <= s[2 * (f - 1)], "monotonic fade");
        }
        let mut short = vec![0.5f32; 100];
        truncate_with_fade(&mut short, 1, 1000, 600, 80);
        assert_eq!(short.len(), 100);
        assert!(short.iter().all(|&x| x == 0.5));
    }
}
