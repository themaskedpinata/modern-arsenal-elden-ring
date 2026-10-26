//! sndconv: decodes the first N seconds of an MP3 into a 16-bit PCM WAV (docs/M4_DESIGN.md section 10).
//!
//! The MW2 AC-130 ambient (`amb_ac130_int0v2_lr.mp3`, 192 kbps joint stereo, 44.1 kHz, ~247.6 s) is the only MP3 the
//! mod uses. It is decoded offline by this tool (called from `tools\scripts\convert_ac130.py`) so the DLL keeps its
//! WAV-only audio stack. Reads only the given input, writes only the given output.
//!
//! Usage: `sndconv <in.mp3> <out.wav> [--seconds 60]`. Prints one line:
//! `SNDCONV ok channels=2 rate=44100 frames=2646000 peak=0.912 source_frames=10917882`.
//! Exit 0 = written, 1 = error, 2 = usage.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

struct Pcm {
    channels: u16,
    rate: u32,
    samples: Vec<i16>,
    source_frames: u64,
}

fn decode(path: &PathBuf, max_frames: u64) -> Result<Pcm, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("probe: {e}"))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no default track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("codec: {e}"))?;
    let (mut channels, mut rate) = (0u16, 0u32);
    let mut samples: Vec<i16> = Vec::new();
    let mut source_frames = 0u64;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("packet: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode: {e}")),
        };
        let spec = *decoded.spec();
        let ch = spec.channels.count() as u16;
        if channels == 0 {
            channels = ch;
            rate = spec.rate;
        } else if ch != channels || spec.rate != rate {
            return Err(format!("stream changes format mid-way ({ch} ch {} Hz)", spec.rate));
        }
        let mut sb = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        sb.copy_interleaved_ref(decoded);
        let frames = (sb.samples().len() / ch.max(1) as usize) as u64;
        source_frames += frames;
        let have = samples.len() as u64 / ch.max(1) as u64;
        if have < max_frames {
            let take = (max_frames - have).min(frames) as usize * ch as usize;
            samples.extend_from_slice(&sb.samples()[..take]);
        }
    }
    if channels == 0 || rate == 0 {
        return Err("no audio decoded".into());
    }
    Ok(Pcm { channels, rate, samples, source_frames })
}

fn write_wav(path: &PathBuf, p: &Pcm) -> Result<(), String> {
    let data_len = (p.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    let block = p.channels as u32 * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&p.channels.to_le_bytes());
    out.extend_from_slice(&p.rate.to_le_bytes());
    out.extend_from_slice(&(p.rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &p.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    let tmp = path.with_extension("wav.tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(&out).map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn run() -> Result<String, (u8, String)> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || (2u8, "usage: sndconv <in.mp3> <out.wav> [--seconds N]".to_string());
    if args.len() != 2 && args.len() != 4 {
        return Err(usage());
    }
    let seconds: f64 = if args.len() == 4 {
        if args[2] != "--seconds" {
            return Err(usage());
        }
        args[3].parse().map_err(|_| usage())?
    } else {
        60.0
    };
    if !(seconds > 0.0 && seconds <= 600.0) {
        return Err((2, format!("--seconds {seconds} outside (0, 600]")));
    }
    let (inp, outp) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
    // the sample rate is known only after decoding starts; decode at most 600 s of 48 kHz worth of frames, then trim
    let pcm = decode(&inp, (seconds * 48_000.0) as u64).map_err(|e| (1, e))?;
    let want = (seconds * pcm.rate as f64).round() as usize * pcm.channels as usize;
    let mut pcm = pcm;
    if pcm.samples.len() < want {
        return Err((1, format!("source has only {} frames (< {seconds} s)", pcm.samples.len() / pcm.channels as usize)));
    }
    pcm.samples.truncate(want);
    let peak = pcm.samples.iter().map(|s| (*s as i32).unsigned_abs()).max().unwrap_or(0) as f64 / 32768.0;
    write_wav(&outp, &pcm).map_err(|e| (1, e))?;
    Ok(format!(
        "SNDCONV ok channels={} rate={} frames={} peak={peak:.3} source_frames={}",
        pcm.channels,
        pcm.rate,
        pcm.samples.len() / pcm.channels as usize,
        pcm.source_frames
    ))
}

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err((code, e)) => {
            println!("SNDCONV FAIL {e}");
            ExitCode::from(code)
        }
    }
}
