//! AC-130 sound selection (docs/M4_DESIGN.md section 10). Pure: the task picks the variant, volume and pitch with
//! the seeded `Rng` and the audio thread only plays what it is told.
//!
//! Volumes and pitches come from the MW2 aliases (`sounds_ac130.json`): a uniform draw in `[min, max]`. Explosions
//! are heard from the gunship: `gain = alias volume x max(dist_gain(slant), explosion_floor_gain)` with
//! `dist_gain` = 1 below the alias `dist_min`, linear to 0 at `dist_max` (metres) [I].

use crate::ac130_data::SoundCue;
use crate::rng::Rng;

/// The AC-130 cues, in `sounds_ac130.json` key order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cue {
    Fire105,
    Reload105,
    Fire40,
    Reload40,
    Fire25,
    Exp105,
    Exp40,
    Switch,
    Ambient,
    Exit,
    Announcer,
}

impl Cue {
    pub const ALL: [Cue; 11] = [
        Cue::Fire105,
        Cue::Reload105,
        Cue::Fire40,
        Cue::Reload40,
        Cue::Fire25,
        Cue::Exp105,
        Cue::Exp40,
        Cue::Switch,
        Cue::Ambient,
        Cue::Exit,
        Cue::Announcer,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Cue::Fire105 => "fire_105",
            Cue::Reload105 => "reload_105",
            Cue::Fire40 => "fire_40",
            Cue::Reload40 => "reload_40",
            Cue::Fire25 => "fire_25",
            Cue::Exp105 => "exp_105",
            Cue::Exp40 => "exp_40",
            Cue::Switch => "switch",
            Cue::Ambient => "ambient",
            Cue::Exit => "exit",
            Cue::Announcer => "announcer",
        }
    }

    pub fn fire(gun: usize) -> Cue {
        [Cue::Fire105, Cue::Fire40, Cue::Fire25][gun.min(2)]
    }

    /// The reload secondary alias of a gun (the 25 mm has none).
    pub fn reload(gun: usize) -> Option<Cue> {
        [Some(Cue::Reload105), Some(Cue::Reload40), None][gun.min(2)]
    }

    /// The explosion alias (MW2's 25 mm has no projExplosionSound).
    pub fn explosion(gun: usize) -> Option<Cue> {
        [Some(Cue::Exp105), Some(Cue::Exp40), None][gun.min(2)]
    }
}

/// One play request for the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Play {
    pub cue: Cue,
    pub variant: usize,
    pub gain: f32,
    pub speed: f32,
    pub delay_ms: u32,
}

/// 1 below `min_m`, linear to 0 at `max_m`.
pub fn dist_gain(d_m: f32, min_m: f32, max_m: f32) -> f32 {
    if !d_m.is_finite() {
        return 0.0;
    }
    if d_m <= min_m {
        return 1.0;
    }
    if max_m <= min_m || d_m >= max_m {
        return 0.0;
    }
    1.0 - (d_m - min_m) / (max_m - min_m)
}

fn uniform(rng: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * rng.next_f32()
}

/// Picks variant, volume and pitch of a cue. `extra_gain` multiplies the volume (distance, config gains).
pub fn pick(cue: Cue, c: &SoundCue, rng: &mut Rng, extra_gain: f32, delay_ms: u32) -> Play {
    let n = c.files.len().max(1);
    let variant = (rng.next_u64() % n as u64) as usize;
    let vol = uniform(rng, c.vol_min, c.vol_max);
    let speed = uniform(rng, c.pitch_min, c.pitch_max);
    Play { cue, variant, gain: (vol * extra_gain).max(0.0), speed, delay_ms }
}

/// The explosion gain heard from the gunship at slant distance `d_m` (alias distances are inches).
pub fn explosion_gain(c: &SoundCue, d_m: f32, floor: f32) -> f32 {
    dist_gain(d_m, c.dist_min_in * 0.0254, c.dist_max_in * 0.0254).max(floor.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(files: usize, v: (f32, f32), p: (f32, f32), delay: u32) -> SoundCue {
        SoundCue {
            files: (0..files).map(|i| format!("sounds/x{i}.wav")).collect(),
            vol_min: v.0,
            vol_max: v.1,
            pitch_min: p.0,
            pitch_max: p.1,
            dist_min_in: 1000.0,
            dist_max_in: 15000.0,
            startdelay_ms: delay,
            looping: false,
        }
    }

    #[test]
    fn ac130_audio_dist_gain_endpoints() {
        assert_eq!(dist_gain(10.0, 25.4, 381.0), 1.0);
        assert_eq!(dist_gain(381.0, 25.4, 381.0), 0.0);
        assert_eq!(dist_gain(500.0, 25.4, 381.0), 0.0);
        assert!((dist_gain(203.2, 25.4, 381.0) - 0.5).abs() < 1e-5);
        assert_eq!(dist_gain(f32::NAN, 1.0, 2.0), 0.0);
        // 105 boom from the gunship (157.8 m slant, alias 1000..15000 in): ~0.63; the 40 mm (200..2000 in) is 0
        let c = cue(5, (0.88, 0.92), (0.95, 1.1), 0);
        assert!((explosion_gain(&c, 157.8, 0.35) - 0.6274).abs() < 1e-3);
        let c40 = SoundCue { dist_min_in: 200.0, dist_max_in: 2000.0, ..c };
        assert_eq!(explosion_gain(&c40, 157.8, 0.35), 0.35, "floor keeps the 40 mm audible");
    }

    #[test]
    fn ac130_audio_pick_within_ranges() {
        let c = cue(5, (0.8835, 0.9207), (0.95, 1.1), 0);
        let mut rng = Rng::new(7);
        let mut seen = [0u32; 5];
        for _ in 0..10_000 {
            let p = pick(Cue::Exp105, &c, &mut rng, 1.0, 0);
            assert!((0.8835..=0.9207).contains(&p.gain));
            assert!((0.95..=1.1).contains(&p.speed));
            seen[p.variant] += 1;
        }
        assert!(seen.iter().all(|&n| n > 1500), "{seen:?}");
        let fixed = cue(1, (0.328815, 0.328815), (0.7, 0.8), 0);
        let p = pick(Cue::Fire105, &fixed, &mut rng, 0.5, 0);
        assert!((p.gain - 0.1644075).abs() < 1e-6 && p.variant == 0);
    }

    #[test]
    fn ac130_audio_reload_secondary_delays() {
        let r105 = cue(1, (0.51149, 0.51149), (0.9, 0.9), 2000);
        let r40 = cue(1, (0.21921, 0.21921), (0.95, 1.05), 1000);
        let mut rng = Rng::new(1);
        assert_eq!(pick(Cue::Reload105, &r105, &mut rng, 1.0, r105.startdelay_ms).delay_ms, 2000);
        assert_eq!(pick(Cue::Reload40, &r40, &mut rng, 1.0, r40.startdelay_ms).delay_ms, 1000);
        assert_eq!(Cue::reload(0), Some(Cue::Reload105));
        assert_eq!(Cue::reload(2), None);
        assert_eq!(Cue::explosion(2), None, "the 25 mm has no explosion sound");
        assert_eq!(Cue::fire(1).key(), "fire_40");
        let keys: Vec<&str> = Cue::ALL.iter().map(|c| c.key()).collect();
        assert_eq!(keys.len(), 11);
    }
}
