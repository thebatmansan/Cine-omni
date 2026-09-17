//! Encoding pictures to H.264 in an MP4.
//!
//! The encoder is `rusty_h264` — the same pure-Rust, BSD-2 codec the decoder
//! comes from, already in the build — writing Constrained Baseline, the profile
//! this build's own decoder advertises, so everything written here can be read
//! back by it. The container is the `mp4` crate's writer.
//!
//! Time is exact. The track's timescale is the frame rate's numerator and each
//! sample lasts its denominator, so 24000/1001 is a timescale of 24000 with
//! samples of 1001 ticks: no frame's time is rounded.
//!
//! What this is not, yet: audio, B-frames, colour signalling beyond the
//! encoder's defaults, or anything above 8-bit 4:2:0. A deliverable that needs
//! those is not one this writes.

use crate::error::{Error, Result};
use rusty_h264::{Encoder, EncoderConfig};
/// A picture to encode: 8-bit 4:2:0 planes.
pub use rusty_h264::YuvFrame;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use ucg_core::Rate;

/// What an encode wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EncodeReport {
    pub frames: u64,
    pub keyframes: u64,
    pub bytes: u64,
}

/// Quality settings a caller chooses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct H264Settings {
    /// Quantiser, 0 (best) to 51.
    pub qp: u8,
    /// Frames between keyframes at most.
    pub gop: u32,
}

impl Default for H264Settings {
    fn default() -> Self {
        H264Settings { qp: 23, gop: 48 }
    }
}

/// NAL units of an Annex-B access unit, without their start codes.
fn nal_units(annex_b: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 2 < annex_b.len() {
        let three = annex_b[i] == 0 && annex_b[i + 1] == 0 && annex_b[i + 2] == 1;
        let four = i + 3 < annex_b.len()
            && annex_b[i] == 0
            && annex_b[i + 1] == 0
            && annex_b[i + 2] == 0
            && annex_b[i + 3] == 1;
        if three || four {
            if let Some(s) = start {
                let mut end = i;
                while end > s && annex_b[end - 1] == 0 {
                    end -= 1;
                }
                out.push(&annex_b[s..end]);
            }
            i += if four { 4 } else { 3 };
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        if s < annex_b.len() {
            out.push(&annex_b[s..]);
        }
    }
    out
}

/// Encode `frames` (8-bit 4:2:0, all `width × height`) into an MP4 at `rate`.
pub fn write_h264_mp4(
    path: &Path,
    width: usize,
    height: usize,
    rate: Rate,
    settings: H264Settings,
    frames: impl IntoIterator<Item = YuvFrame>,
) -> Result<EncodeReport> {
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(Error::Unsupported(format!(
            "{width}x{height}: 4:2:0 needs an even, non-zero width and height"
        )));
    }
    let mut cfg = EncoderConfig::baseline(width, height);
    cfg.qp = settings.qp.min(51);
    cfg.gop_size = settings.gop.max(1);
    let mut encoder =
        Encoder::new(cfg).map_err(|e| Error::Unsupported(format!("the encoder refused: {e:?}")))?;

    let mut writer: Option<mp4::Mp4Writer<BufWriter<File>>> = None;
    let mut report = EncodeReport::default();
    for (index, frame) in frames.into_iter().enumerate() {
        if frame.width != width || frame.height != height {
            return Err(Error::Unsupported(format!(
                "frame {index} is {}x{}, the file is {width}x{height}",
                frame.width, frame.height
            )));
        }
        let unit = encoder.encode(&frame);
        let nals = nal_units(&unit);
        if writer.is_none() {
            let sps = nals.iter().find(|n| n.first().map(|b| b & 0x1F) == Some(7));
            let pps = nals.iter().find(|n| n.first().map(|b| b & 0x1F) == Some(8));
            let (Some(sps), Some(pps)) = (sps, pps) else {
                return Err(Error::Unsupported(
                    "the first access unit carries no SPS and PPS".into(),
                ));
            };
            let file = File::create(path).map_err(Error::Io)?;
            let config = mp4::Mp4Config {
                major_brand: "isom".parse().expect("fourcc"),
                minor_version: 512,
                compatible_brands: ["isom", "iso2", "avc1", "mp41"]
                    .iter()
                    .map(|b| b.parse().expect("fourcc"))
                    .collect(),
                timescale: rate.num(),
            };
            let mut w = mp4::Mp4Writer::write_start(BufWriter::new(file), &config)
                .map_err(|e| Error::Unsupported(format!("mp4: {e}")))?;
            w.add_track(&mp4::TrackConfig {
                track_type: mp4::TrackType::Video,
                timescale: rate.num(),
                language: "und".into(),
                media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                    width: u16::try_from(width).map_err(|_| Error::Unsupported("too wide".into()))?,
                    height: u16::try_from(height).map_err(|_| Error::Unsupported("too tall".into()))?,
                    seq_param_set: sps.to_vec(),
                    pic_param_set: pps.to_vec(),
                }),
            })
            .map_err(|e| Error::Unsupported(format!("mp4: {e}")))?;
            writer = Some(w);
        }
        // AVCC: each NAL length-prefixed, parameter sets and delimiters left to
        // the sample description where MP4 keeps them.
        let mut sample = Vec::with_capacity(unit.len() + 16);
        let mut sync = false;
        for nal in &nals {
            let kind = nal.first().map(|b| b & 0x1F).unwrap_or(0);
            if matches!(kind, 7..=9) {
                continue;
            }
            sync |= kind == 5;
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        report.bytes += sample.len() as u64;
        report.keyframes += u64::from(sync);
        writer
            .as_mut()
            .expect("started")
            .write_sample(
                1,
                &mp4::Mp4Sample {
                    start_time: report.frames * u64::from(rate.den()),
                    duration: rate.den(),
                    rendering_offset: 0,
                    is_sync: sync,
                    bytes: mp4::Bytes::from(sample),
                },
            )
            .map_err(|e| Error::Unsupported(format!("mp4: {e}")))?;
        report.frames += 1;
    }
    let mut writer = writer.ok_or_else(|| Error::Unsupported("no frames to encode".into()))?;
    writer
        .write_end()
        .map_err(|e| Error::Unsupported(format!("mp4: {e}")))?;
    Ok(report)
}
