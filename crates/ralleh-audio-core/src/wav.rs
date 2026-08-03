//! Minimal mono PCM WAV helpers for CLI STT/TTS adapters.
//!
//! Intentionally tiny — only what WhisperCli / PiperCli need. Not a general
//! audio I/O library.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum WavError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or corrupt WAV: {0}")]
    Format(String),
}

/// Write 16-bit little-endian mono PCM as a RIFF WAVE file.
pub fn write_pcm16_mono(
    path: impl AsRef<Path>,
    samples: &[f32],
    sample_rate_hz: u32,
) -> Result<(), WavError> {
    let mut file = File::create(path)?;
    let n = samples.len() as u32;
    let data_bytes = n.saturating_mul(2);
    let byte_rate = sample_rate_hz.saturating_mul(2);
    // RIFF header + fmt(16) + data header = 44 bytes for PCM.
    let riff_size = 36u32.saturating_add(data_bytes);

    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // PCM fmt chunk size
    file.write_all(&1u16.to_le_bytes())?; // audio format = PCM
    file.write_all(&1u16.to_le_bytes())?; // channels = mono
    file.write_all(&sample_rate_hz.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?; // block align
    file.write_all(&16u16.to_le_bytes())?; // bits per sample
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;

    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i = (clamped * i16::MAX as f32) as i16;
        file.write_all(&i.to_le_bytes())?;
    }
    Ok(())
}

/// Decoded mono float PCM ([-1, 1]) plus sample rate.
#[derive(Debug, Clone)]
pub struct PcmMono {
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
}

/// Read a simple PCM16 mono (or first-channel-of-stereo) WAVE file.
pub fn read_pcm16(path: impl AsRef<Path>) -> Result<PcmMono, WavError> {
    let mut buf = Vec::new();
    File::open(path)?.read_to_end(&mut buf)?;
    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(WavError::Format("missing RIFF/WAVE".into()));
    }

    let mut offset = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;

    while offset + 8 <= buf.len() {
        let id = &buf[offset..offset + 4];
        let size = u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start.saturating_add(size);
        if end > buf.len() {
            return Err(WavError::Format("chunk overruns file".into()));
        }
        if id == b"fmt " {
            if size < 16 {
                return Err(WavError::Format("fmt too small".into()));
            }
            let format = u16::from_le_bytes(buf[start..start + 2].try_into().unwrap());
            if format != 1 {
                return Err(WavError::Format(format!("non-PCM format {format}")));
            }
            channels = u16::from_le_bytes(buf[start + 2..start + 4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(buf[start + 4..start + 8].try_into().unwrap());
            bits = u16::from_le_bytes(buf[start + 14..start + 16].try_into().unwrap());
        } else if id == b"data" {
            data = Some(&buf[start..end]);
        }
        offset = end + (size % 2); // word-align
    }

    let data = data.ok_or_else(|| WavError::Format("no data chunk".into()))?;
    if channels == 0 || bits != 16 {
        return Err(WavError::Format(format!(
            "need 16-bit PCM, got channels={channels} bits={bits}"
        )));
    }
    let frame = (channels as usize) * 2;
    if frame == 0 || data.len() % frame != 0 {
        return Err(WavError::Format("data length not aligned".into()));
    }
    let mut samples = Vec::with_capacity(data.len() / frame);
    for chunk in data.chunks_exact(frame) {
        let i = i16::from_le_bytes([chunk[0], chunk[1]]);
        samples.push(i as f32 / i16::MAX as f32);
    }
    Ok(PcmMono {
        samples,
        sample_rate_hz: sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn round_trip_silence() {
        let path = temp_dir().join("ralleh-wav-roundtrip.wav");
        let samples = vec![0.0_f32; 160];
        write_pcm16_mono(&path, &samples, 16_000).unwrap();
        let got = read_pcm16(&path).unwrap();
        assert_eq!(got.sample_rate_hz, 16_000);
        assert_eq!(got.samples.len(), 160);
        assert!(got.samples.iter().all(|s| s.abs() < 1e-4));
        let _ = std::fs::remove_file(&path);
    }
}
