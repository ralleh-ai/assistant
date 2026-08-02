//! Capture default microphone input to a WAV file.
//!
//! ```text
//! cargo run -p ralleh-audio-core --features mic --bin mic-capture -- --seconds 5 --out capture.wav
//! ```
//!
//! Unset `RALLEH_SKIP_LIVE_AUDIO` if set. Requires a working default input
//! device and (on Windows) microphone permission for the terminal.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use ralleh_audio_core::{write_pcm16_mono, AudioSource, CpalMicSource};

fn usage() -> ! {
    eprintln!(
        "Usage: mic-capture [--seconds <n>] [--out <path.wav>]\n\n\
         Capture the default microphone to a 16-bit mono WAV.\n\
         Defaults: --seconds 5 --out mic-capture.wav"
    );
    std::process::exit(2);
}

fn parse_args() -> (f32, PathBuf) {
    let mut seconds = 5.0_f32;
    let mut out = PathBuf::from("mic-capture.wav");
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => usage(),
            "--seconds" | "-s" => {
                let v = args.next().unwrap_or_else(|| usage());
                seconds = v.parse().unwrap_or_else(|_| {
                    eprintln!("invalid --seconds: {v}");
                    usage();
                });
            }
            "--out" | "-o" => {
                out = PathBuf::from(args.next().unwrap_or_else(|| usage()));
            }
            other if other.starts_with('-') => {
                eprintln!("unknown flag: {other}");
                usage();
            }
            other => {
                // Positional output path convenience.
                out = PathBuf::from(other);
            }
        }
    }
    if !(0.1..=300.0).contains(&seconds) {
        eprintln!("--seconds must be between 0.1 and 300");
        usage();
    }
    (seconds, out)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

fn level_bar(rms: f32) -> String {
    // Rough visual: ~0.02 quiet room, ~0.2 speech, clip near 1.0
    let filled = ((rms * 40.0).clamp(0.0, 40.0)) as usize;
    format!("[{}{}] rms={rms:.4}", "#".repeat(filled), "-".repeat(40 - filled))
}

fn main() -> ExitCode {
    let (seconds, out) = parse_args();

    if env::var_os("RALLEH_SKIP_LIVE_AUDIO").is_some() {
        eprintln!("warning: RALLEH_SKIP_LIVE_AUDIO is set; unsetting for capture");
        env::remove_var("RALLEH_SKIP_LIVE_AUDIO");
    }

    let mut mic = match CpalMicSource::open_default() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("failed to open default microphone: {e}");
            eprintln!("check OS mic permissions and that a default input device exists");
            return ExitCode::FAILURE;
        }
    };

    let sample_rate = mic.sample_rate_hz();
    eprintln!(
        "recording {seconds:.1}s @ {sample_rate} Hz → {}",
        out.display()
    );

    let deadline = Instant::now() + Duration::from_secs_f32(seconds);
    let mut pcm: Vec<f32> = Vec::with_capacity((sample_rate as f32 * seconds) as usize + sample_rate as usize);
    let mut last_meter = Instant::now();
    let mut meter_window: Vec<f32> = Vec::new();

    while Instant::now() < deadline {
        match mic.next_frame() {
            Some(frame) => {
                meter_window.extend_from_slice(&frame.samples);
                pcm.extend_from_slice(&frame.samples);
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
        if last_meter.elapsed() >= Duration::from_millis(200) {
            eprint!("\r{}", level_bar(rms(&meter_window)));
            meter_window.clear();
            last_meter = Instant::now();
        }
    }
    eprintln!();

    if pcm.is_empty() {
        eprintln!("no samples captured — is the input muted or unavailable?");
        return ExitCode::FAILURE;
    }

    if let Err(e) = write_pcm16_mono(&out, &pcm, sample_rate) {
        eprintln!("failed to write {}: {e}", out.display());
        return ExitCode::FAILURE;
    }

    let secs = pcm.len() as f32 / sample_rate as f32;
    eprintln!(
        "wrote {} ({:.2}s, {} samples, peak-ish rms={:.4})",
        out.display(),
        secs,
        pcm.len(),
        rms(&pcm)
    );
    ExitCode::SUCCESS
}
