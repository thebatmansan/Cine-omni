//! RIFF/WAVE reading and writing (PCM 16/24-bit and IEEE float 32).
//!
//! Handles plain `WAVE_FORMAT_PCM`, `WAVE_FORMAT_IEEE_FLOAT` and
//! `WAVE_FORMAT_EXTENSIBLE` headers, odd-sized chunk padding, and unknown
//! chunks (skipped). No external dependencies.

use crate::buffer::AudioBuffer;
use crate::error::{Error, Result};
use std::path::Path;

/// Sample encoding used when writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavFormat {
    /// 16-bit signed integer PCM.
    Pcm16,
    /// 24-bit signed integer PCM.
    Pcm24,
    /// 32-bit IEEE float.
    Float32,
}

impl WavFormat {
    fn bits(self) -> u16 {
        match self {
            WavFormat::Pcm16 => 16,
            WavFormat::Pcm24 => 24,
            WavFormat::Float32 => 32,
        }
    }
    fn tag(self) -> u16 {
        match self {
            WavFormat::Float32 => 3,
            _ => 1,
        }
    }
}

/// Encode a buffer as a complete WAV file in memory.
pub fn encode_wav(buffer: &AudioBuffer, format: WavFormat) -> Vec<u8> {
    let channels = buffer.channels as u16;
    let bits = format.bits();
    let block_align = channels * (bits / 8);
    let byte_rate = buffer.sample_rate * block_align as u32;
    let data_len = buffer.data.len() * (bits as usize / 8);
    let mut out = Vec::with_capacity(44 + data_len + 1);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&0u32.to_le_bytes()); // patched below
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&format.tag().to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&buffer.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in &buffer.data {
        match format {
            WavFormat::Pcm16 => {
                let v = (s.clamp(-1.0, 1.0) * 32768.0)
                    .round()
                    .clamp(-32768.0, 32767.0) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            WavFormat::Pcm24 => {
                let v = (s.clamp(-1.0, 1.0) * 8_388_608.0)
                    .round()
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                let b = v.to_le_bytes();
                out.extend_from_slice(&b[..3]);
            }
            WavFormat::Float32 => out.extend_from_slice(&s.to_le_bytes()),
        }
    }
    if data_len % 2 == 1 {
        out.push(0);
    }
    let riff_len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_len.to_le_bytes());
    out
}

/// Decode a WAV file from memory.
pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Wav("not a RIFF/WAVE file".into()));
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (tag, channels, rate, bits)
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(Error::Wav("fmt chunk too short".into()));
                }
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if tag == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts at byte 24.
                    if body.len() < 26 {
                        return Err(Error::Wav("extensible fmt chunk too short".into()));
                    }
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        pos = body_start + len + (len & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or_else(|| Error::Wav("missing fmt chunk".into()))?;
    let data = data.ok_or_else(|| Error::Wav("missing data chunk".into()))?;
    if channels == 0 {
        return Err(Error::Wav("zero channels".into()));
    }
    let samples: Vec<f32> = match (tag, bits) {
        (1, 16) => data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|b| {
                let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (1, 8) => data.iter().map(|b| (*b as f32 - 128.0) / 128.0).collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        (3, 64) => data
            .chunks_exact(8)
            .map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32)
            .collect(),
        _ => {
            return Err(Error::Wav(format!(
                "unsupported format tag {tag} with {bits} bits"
            )))
        }
    };
    let ch = channels as usize;
    let whole = samples.len() / ch * ch;
    AudioBuffer::from_interleaved(rate, ch, samples[..whole].to_vec())
}

/// Read a WAV file from disk.
pub fn read_wav(path: impl AsRef<Path>) -> Result<AudioBuffer> {
    let bytes = std::fs::read(path)?;
    decode_wav(&bytes)
}

/// Write a WAV file to disk.
pub fn write_wav(path: impl AsRef<Path>, buffer: &AudioBuffer, format: WavFormat) -> Result<()> {
    std::fs::write(path, encode_wav(buffer, format))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signal() -> AudioBuffer {
        let mut b = AudioBuffer::sine(44_100, 2, 440.0, 0.75, 0.05);
        // Make channels differ and include an odd sample count.
        for i in 0..b.frames() {
            *b.sample_mut(i, 1) *= -0.5;
        }
        b.data.truncate((b.frames() - 1) * 2 + 2);
        b
    }

    #[test]
    fn round_trip_all_formats_in_memory() {
        let sig = test_signal();
        for (fmt, tol) in [
            (WavFormat::Pcm16, 1.0 / 32768.0 * 1.01),
            (WavFormat::Pcm24, 1.0 / 8_388_608.0 * 1.01),
            (WavFormat::Float32, 0.0),
        ] {
            let bytes = encode_wav(&sig, fmt);
            let back = decode_wav(&bytes).unwrap();
            assert_eq!(back.sample_rate, 44_100);
            assert_eq!(back.channels, 2);
            assert_eq!(back.frames(), sig.frames());
            let max_err = sig
                .data
                .iter()
                .zip(&back.data)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(max_err <= tol, "{fmt:?}: max err {max_err} > {tol}");
        }
    }

    #[test]
    fn round_trip_on_disk_with_odd_data_length() {
        let mono = AudioBuffer::from_interleaved(48_000, 1, vec![0.1, -0.2, 0.3]).unwrap();
        let path = std::env::temp_dir().join(format!("cine-audio-wav-{}.wav", std::process::id()));
        write_wav(&path, &mono, WavFormat::Pcm24).unwrap();
        let back = read_wav(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.frames(), 3);
        assert!((back.data[2] - 0.3).abs() < 1e-6);
        // 3 frames * 3 bytes = 9 bytes of data => padded, RIFF length must be even.
        let bytes = encode_wav(&mono, WavFormat::Pcm24);
        assert_eq!(bytes.len() % 2, 0);
    }

    #[test]
    fn decodes_extensible_and_rejects_garbage() {
        let sig = AudioBuffer::sine(48_000, 1, 100.0, 0.5, 0.01);
        let mut bytes = encode_wav(&sig, WavFormat::Float32);
        // Rewrite the fmt chunk as WAVE_FORMAT_EXTENSIBLE (40 byte body).
        let fmt_start = 12;
        let mut ext = Vec::new();
        ext.extend_from_slice(b"fmt ");
        ext.extend_from_slice(&40u32.to_le_bytes());
        ext.extend_from_slice(&0xFFFEu16.to_le_bytes());
        ext.extend_from_slice(&bytes[fmt_start + 10..fmt_start + 24]); // channels..bits
        ext.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        ext.extend_from_slice(&32u16.to_le_bytes()); // valid bits
        ext.extend_from_slice(&4u32.to_le_bytes()); // channel mask
        ext.extend_from_slice(&3u16.to_le_bytes()); // sub-format: float
        ext.extend_from_slice(&[0u8; 14]);
        let rest = bytes.split_off(fmt_start + 24);
        bytes.truncate(fmt_start);
        bytes.extend_from_slice(&ext);
        bytes.extend_from_slice(&rest);
        let back = decode_wav(&bytes).unwrap();
        assert_eq!(back.frames(), sig.frames());
        assert!(decode_wav(b"nope").is_err());
        assert!(decode_wav(b"RIFF\0\0\0\0WAVEjunk\0\0\0\0").is_err());
    }
}
