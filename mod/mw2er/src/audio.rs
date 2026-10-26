//! Audio thread (docs/M2_DESIGN.md section 14; M4 section 10).
//!
//! Only the `mw2er-audio` thread touches cpal/rodio (cpal makes its calling thread a COM STA, which
//! must never be a game thread). Sounds are decoded once into `SamplesBuffer`s (cheap clones); the
//! game side only queues [`AudioMsg`]s with `try_send`. Any failure logs `AUDIO WARN` and degrades to
//! silence; nothing here can block or crash the game.
//!
//! M4: the AC-130 cues (variants, volume, pitch and delay picked by the task with `mw2er_core::ac130_audio`) and the
//! gunship ambient loop (a 60 s WAV decoded offline from MW2's MP3 by `sndconv`; the DLL stays WAV-only) with a
//! fade-out when the ride ends.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use mw2er_core::ac130_audio::Cue;
use mw2er_core::audio_math::truncate_with_fade;
use mw2er_core::weapon_sim::SfxCue;
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, DeviceSinkBuilder, Source};

use crate::guard::guard;
use crate::log::{log_i, log_w, set_thread_role};
use crate::shared::{self, AUDIO_OK, AudioMsg, SFX_TX};

const QUEUE: usize = 128;

/// Starts the audio thread. Returns false if the thread could not be spawned.
pub fn start(files: Vec<(SfxCue, PathBuf)>, ac_files: Vec<(Cue, Vec<PathBuf>)>, fire_max_ms: u32, fire_fade_ms: u32) -> bool {
    let (tx, rx) = sync_channel::<AudioMsg>(QUEUE);
    let spawned = std::thread::Builder::new().name("mw2er-audio".into()).spawn(move || {
        set_thread_role("audio");
        if guard("audio", || audio_thread(&files, &ac_files, rx, fire_max_ms, fire_fade_ms)).is_none() {
            AUDIO_OK.store(false, Ordering::SeqCst);
            log_w!("AUDIO", "WARN audio thread panicked; sounds disabled");
        }
    });
    match spawned {
        Ok(_) => {
            let _ = SFX_TX.set(tx);
            true
        }
        Err(e) => {
            log_w!("AUDIO", "WARN thread spawn failed: {e}; silent");
            false
        }
    }
}

fn load_wav(path: &Path, truncate: Option<(u32, u32)>) -> Result<SamplesBuffer, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dec = Decoder::new_wav(BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))?;
    let (ch, sr) = (dec.channels(), dec.sample_rate());
    let mut samples: Vec<f32> = dec.collect();
    if let Some((max_ms, fade_ms)) = truncate {
        truncate_with_fade(&mut samples, ch.get(), sr.get(), max_ms, fade_ms);
    }
    Ok(SamplesBuffer::new(ch, sr, samples))
}

/// The running ambient loop: its stop flag and gain (bits), polled by the source every 10 ms.
struct Ambient {
    stop: Arc<AtomicBool>,
    gain: Arc<AtomicU32>,
    fade_from: f32,
    fade_t0: Option<std::time::Instant>,
    fade_ms: u32,
}

fn audio_thread(files: &[(SfxCue, PathBuf)], ac_files: &[(Cue, Vec<PathBuf>)], rx: Receiver<AudioMsg>, fire_max_ms: u32, fire_fade_ms: u32) {
    // COM (STA) is initialised by cpal on THIS thread on first use.
    // The error callback runs on cpal's stream thread: catch panics there too.
    let opened = DeviceSinkBuilder::from_default_device()
        .map(|b| {
            b.with_error_callback(|e| {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    log_w!("AUDIO", "WARN stream error: {e}")
                }));
            })
        })
        .and_then(|b| b.open_stream());
    let mut sink = match opened {
        Ok(s) => s,
        Err(e) => {
            log_w!("AUDIO", "WARN no audio output: {e}; silent");
            for _ in rx {} // keep draining so the game side's try_send never matters
            return;
        }
    };
    sink.log_on_drop(false);

    let mut bank: HashMap<SfxCue, SamplesBuffer> = HashMap::new();
    for (cue, path) in files {
        let trunc = (*cue == SfxCue::Fire).then_some((fire_max_ms, fire_fade_ms));
        match load_wav(path, trunc) {
            Ok(b) => {
                bank.insert(*cue, b);
            }
            Err(e) => log_w!("AUDIO", "WARN {} not loaded: {e}", cue.name()),
        }
    }
    let mut ac_bank: HashMap<Cue, Vec<SamplesBuffer>> = HashMap::new();
    let mut ac_n = 0;
    for (cue, paths) in ac_files {
        let mut v = Vec::new();
        for p in paths {
            match load_wav(p, None) {
                Ok(b) => {
                    v.push(b);
                    ac_n += 1;
                }
                Err(e) => log_w!("AUDIO", "WARN ac130 {} not loaded: {e}", cue.key()),
            }
        }
        if !v.is_empty() {
            ac_bank.insert(*cue, v);
        }
    }
    // M6: every arsenal alias (variants decoded once per file; fire aliases truncated like the M2 fire sound)
    let t_gun = std::time::Instant::now();
    let mut gun_bank: Vec<Vec<(SamplesBuffer, [f32; 2], [f32; 2], Option<u16>)>> = Vec::new();
    let (mut gun_files, mut gun_fail) = (0usize, 0usize);
    if let Some(t) = crate::arsenal_rt::SOUNDS.get() {
        let mut decoded: HashMap<(PathBuf, bool), SamplesBuffer> = HashMap::new();
        for a in &t.aliases {
            let mut vs = Vec::new();
            for v in &a.variants {
                let key = (v.path.clone(), a.fire);
                let buf = match decoded.get(&key) {
                    Some(b) => Some(b.clone()),
                    None => match load_wav(&v.path, a.fire.then_some((fire_max_ms, fire_fade_ms))) {
                        Ok(b) => {
                            gun_files += 1;
                            decoded.insert(key, b.clone());
                            Some(b)
                        }
                        Err(e) => {
                            gun_fail += 1;
                            log_w!("AUDIO", "WARN arsenal alias {} not loaded: {e}", a.name);
                            None
                        }
                    },
                };
                if let Some(b) = buf {
                    vs.push((b, v.vol, v.pitch, v.secondary));
                }
            }
            gun_bank.push(vs);
        }
    }
    // M7: the nuke aliases (key order of nuke.json's sounds map = the index the task sends)
    let mut nuke_bank: Vec<Vec<SamplesBuffer>> = Vec::new();
    let mut nuke_n = 0;
    if let Some(d) = crate::data::NUKE_DATA.get() {
        for (key, paths) in &d.sound_files {
            let mut v = Vec::new();
            for p in paths {
                match load_wav(p, None) {
                    Ok(b) => {
                        v.push(b);
                        nuke_n += 1;
                    }
                    Err(e) => log_w!("AUDIO", "WARN nuke {key} not loaded: {e}"),
                }
            }
            nuke_bank.push(v);
        }
    }
    if nuke_n > 0 {
        log_i!("AUDIO", "nuke {nuke_n} files in {} aliases", nuke_bank.len());
    }
    AUDIO_OK.store(!bank.is_empty() || !gun_bank.is_empty() || nuke_n > 0, Ordering::SeqCst);
    log_i!(
        "AUDIO",
        "ready: {}/{} sounds loaded (fire truncated to {fire_max_ms} ms); ac130 {ac_n} files in {} cues; arsenal {} aliases {gun_files} files ({gun_fail} failed) ms={}",
        bank.len(),
        files.len(),
        ac_bank.len(),
        gun_bank.len(),
        t_gun.elapsed().as_millis()
    );
    let mut rng = mw2er_core::rng::Rng::new(0x9e37_79b9_7f4a_7c15 ^ std::process::id() as u64);

    let mut ambient: Option<Ambient> = None;
    // the generation of the last ambient start handled (shared::AMBIENT_GEN; review M4 minor 5)
    let mut ambient_gen = shared::AMBIENT_GEN.load(Ordering::SeqCst);
    loop {
        // the ambient state and its fade-out need a clock even without messages
        let msg = rx.recv_timeout(Duration::from_millis(20));
        let want = shared::AMBIENT_WANTED.load(Ordering::SeqCst);
        let generation = shared::AMBIENT_GEN.load(Ordering::SeqCst);
        if want && generation != ambient_gen {
            ambient_gen = generation;
            if let Some(a) = ambient.take() {
                a.stop.store(true, Ordering::Relaxed);
            }
            let cfg = shared::config();
            if cfg.audio.enabled
                && let Some(buf) = ac_bank.get(&Cue::Ambient).and_then(|v| v.first())
            {
                let gain = f32::from_bits(shared::AMBIENT_GAIN.load(Ordering::SeqCst));
                let g = cfg.audio.master_gain * cfg.ac130.audio.gain * cfg.ac130.audio.ambient_gain * gain;
                let stop = Arc::new(AtomicBool::new(false));
                let gbits = Arc::new(AtomicU32::new(g.max(0.0).to_bits()));
                let (s2, g2) = (stop.clone(), gbits.clone());
                let src = buf.clone().repeat_infinite().amplify(g.max(0.0)).stoppable().periodic_access(Duration::from_millis(10), move |s| {
                    s.inner_mut().set_factor(f32::from_bits(g2.load(Ordering::Relaxed)));
                    if s2.load(Ordering::Relaxed) {
                        s.stop();
                    }
                });
                sink.mixer().add(src);
                ambient = Some(Ambient { stop, gain: gbits, fade_from: g.max(0.0), fade_t0: None, fade_ms: 300 });
            }
        }
        if !want
            && let Some(a) = ambient.as_mut()
            && a.fade_t0.is_none()
        {
            a.fade_from = f32::from_bits(a.gain.load(Ordering::Relaxed));
            a.fade_ms = shared::AMBIENT_FADE_MS.load(Ordering::SeqCst);
            a.fade_t0 = Some(std::time::Instant::now());
        }
        if let Some(a) = ambient.as_mut()
            && let Some(t0) = a.fade_t0
        {
            let k = (t0.elapsed().as_millis() as f32 / a.fade_ms.max(1) as f32).min(1.0);
            a.gain.store((a.fade_from * (1.0 - k)).to_bits(), Ordering::Relaxed);
            if k >= 1.0 {
                a.stop.store(true, Ordering::Relaxed);
                ambient = None;
            }
        }
        let msg = match msg {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let cfg = shared::config();
        if !cfg.audio.enabled {
            continue;
        }
        match msg {
            AudioMsg::Sfx(cue) => {
                let Some(buf) = bank.get(&cue) else { continue };
                let gain = cfg.audio.master_gain * if cue == SfxCue::Fire { cfg.audio.fire_gain } else { cfg.audio.foley_gain };
                sink.mixer().add(buf.clone().amplify(gain.max(0.0)));
            }
            AudioMsg::Gun { alias, fire } => {
                let gain = cfg.audio.master_gain * if fire { cfg.audio.fire_gain } else { cfg.audio.foley_gain };
                let mut next = Some(alias);
                let mut depth = 0;
                while let Some(a) = next.take() {
                    let Some(vs) = gun_bank.get(a as usize).filter(|v| !v.is_empty()) else { break };
                    let (buf, vol, pitch, sec) = &vs[(rng.next_u64() % vs.len() as u64) as usize];
                    let uni = |r: &mut mw2er_core::rng::Rng, lo: f32, hi: f32| lo + (hi - lo) * r.next_f32();
                    let g = gain * uni(&mut rng, vol[0], vol[1]);
                    let sp = uni(&mut rng, pitch[0], pitch[1]).clamp(0.25, 4.0);
                    sink.mixer().add(buf.clone().speed(sp).amplify(g.max(0.0)));
                    depth += 1;
                    if depth < 2 {
                        next = *sec;
                    }
                }
            }
            AudioMsg::GunAt { alias, gain } => {
                // C3 PASS C: one variant of a peer's third-person alias (no secondary chain: the distant layer is the
                // alias's own secondary, played only up close would double it), distance gain from the task
                let Some(vs) = gun_bank.get(alias as usize).filter(|v| !v.is_empty()) else { continue };
                let (buf, vol, pitch, sec) = &vs[(rng.next_u64() % vs.len() as u64) as usize];
                let uni = |r: &mut mw2er_core::rng::Rng, lo: f32, hi: f32| lo + (hi - lo) * r.next_f32();
                let g = cfg.audio.master_gain * cfg.audio.fire_gain * gain * uni(&mut rng, vol[0], vol[1]);
                let sp = uni(&mut rng, pitch[0], pitch[1]).clamp(0.25, 4.0);
                sink.mixer().add(buf.clone().speed(sp).amplify(g.max(0.0)));
                // the far layer (e.g. weap_medium_fardist_4) under a quieter direct sound
                if gain < 0.5
                    && let Some(s) = sec
                    && let Some(fs) = gun_bank.get(*s as usize).filter(|v| !v.is_empty())
                {
                    let (fb, fvol, _, _) = &fs[(rng.next_u64() % fs.len() as u64) as usize];
                    let fg = cfg.audio.master_gain * cfg.audio.fire_gain * (1.0 - gain) * 0.6 * fvol[1];
                    sink.mixer().add(fb.clone().amplify(fg.max(0.0)));
                }
            }
            AudioMsg::Nuke(p) => {
                let Some(v) = nuke_bank.get(p.key as usize).filter(|v| !v.is_empty()) else { continue };
                let buf = &v[(p.variant as usize).min(v.len() - 1)];
                let gain = cfg.audio.master_gain * cfg.nuke.gain * p.gain;
                // a cancel (loading before the detonation) bumps NUKE_AUDIO_GEN: every source of an older sequence stops
                let my_gen = p.generation;
                let src = buf.clone().speed(p.speed.clamp(0.25, 4.0)).amplify(gain.max(0.0)).stoppable().periodic_access(Duration::from_millis(20), move |s| {
                    if shared::NUKE_AUDIO_GEN.load(Ordering::Relaxed) != my_gen {
                        s.stop();
                    }
                });
                if p.delay_ms > 0 {
                    sink.mixer().add(src.delay(Duration::from_millis(p.delay_ms as u64)));
                } else {
                    sink.mixer().add(src);
                }
            }
            AudioMsg::Ac(p) => {
                let Some(v) = ac_bank.get(&p.cue) else { continue };
                let Some(buf) = v.get(p.variant.min(v.len() - 1)) else { continue };
                let gain = cfg.audio.master_gain * cfg.ac130.audio.gain * p.gain;
                let src = buf.clone().speed(p.speed.clamp(0.25, 4.0)).amplify(gain.max(0.0));
                if p.delay_ms > 0 {
                    sink.mixer().add(src.delay(Duration::from_millis(p.delay_ms as u64)));
                } else {
                    sink.mixer().add(src);
                }
            }
        }
    }
}
