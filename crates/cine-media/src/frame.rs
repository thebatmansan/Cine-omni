//! Frame and audio-buffer representations.
//!
//! A decoded video frame is deliberately *not* forced into RGB on decode.
//! Real camera media is YUV with chroma subsampling, and converting on the CPU
//! at decode time throws away precision and costs more than it saves — the
//! conversion belongs on the GPU, fused with the colour transform. So a frame
//! carries its planes and enough colour metadata to be interpreted correctly
//! later.

use crate::error::Result;
use crate::time::{Timecode, Timestamp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How samples are laid out in a decoded video frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    /// 8-bit 4:2:0, separate Y, U, V planes.
    Yuv420P8,
    /// 8-bit 4:2:2.
    Yuv422P8,
    /// 8-bit 4:4:4.
    Yuv444P8,
    /// 10-bit 4:2:0 in 16-bit words.
    Yuv420P10,
    /// 8-bit interleaved RGB.
    Rgb8,
    /// 8-bit interleaved RGBA.
    Rgba8,
    /// 16-bit interleaved RGB.
    Rgb16,
    /// 16-bit interleaved RGBA, native-endian: what a 16-bit PNG or TIFF holds.
    Rgba16,
    /// 32-bit float RGBA, the working representation for VFX and grading.
    RgbaF32,
}

impl PixelFormat {
    /// Number of separate planes.
    pub fn plane_count(&self) -> usize {
        match self {
            PixelFormat::Yuv420P8
            | PixelFormat::Yuv422P8
            | PixelFormat::Yuv444P8
            | PixelFormat::Yuv420P10 => 3,
            _ => 1,
        }
    }

    /// Bits per component.
    pub fn bit_depth(&self) -> u8 {
        match self {
            PixelFormat::Yuv420P10 | PixelFormat::Rgb16 | PixelFormat::Rgba16 => 16,
            PixelFormat::RgbaF32 => 32,
            _ => 8,
        }
    }

    pub fn has_alpha(&self) -> bool {
        matches!(
            self,
            PixelFormat::Rgba8 | PixelFormat::Rgba16 | PixelFormat::RgbaF32
        )
    }

    pub fn is_yuv(&self) -> bool {
        self.plane_count() == 3
    }

    /// Chroma subsampling as (horizontal, vertical) divisors.
    pub fn chroma_subsampling(&self) -> (u32, u32) {
        match self {
            PixelFormat::Yuv420P8 | PixelFormat::Yuv420P10 => (2, 2),
            PixelFormat::Yuv422P8 => (2, 1),
            PixelFormat::Yuv444P8 => (1, 1),
            _ => (1, 1),
        }
    }
}

/// Colour interpretation carried alongside the samples.
///
/// These are the ITU-T H.273 / ISO 23001-8 code points that containers record.
/// They are frequently absent or wrong in real files, so `None` is represented
/// honestly rather than silently defaulting to Rec.709.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColorMetadata {
    /// H.273 colour_primaries code point.
    pub primaries: Option<u8>,
    /// H.273 transfer_characteristics code point.
    pub transfer: Option<u8>,
    /// H.273 matrix_coefficients code point.
    pub matrix: Option<u8>,
    /// Full range (`true`) vs limited/video range (`false`).
    pub full_range: Option<bool>,
}

impl ColorMetadata {
    /// Whether enough is known to interpret the samples without guessing.
    pub fn is_complete(&self) -> bool {
        self.primaries.is_some() && self.transfer.is_some() && self.matrix.is_some()
    }
}

/// Whether alpha, if present, is already multiplied into the colour channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    None,
    Straight,
    Premultiplied,
}

/// One decoded video frame.
///
/// Plane data is `Arc`-shared so a frame can sit in the cache and be handed to
/// several consumers without copying.
#[derive(Clone)]
pub struct VideoFrame {
    /// Presentation timestamp in the source stream's own time base.
    pub pts: Timestamp,
    /// Frame index within the stream, when the index is known.
    pub frame_index: Option<i64>,
    /// Source timecode for this frame, when derivable.
    pub timecode: Option<Timecode>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub color: ColorMetadata,
    pub alpha: AlphaMode,
    /// One entry per plane; each is (data, bytes-per-row).
    pub planes: Vec<(Arc<Vec<u8>>, usize)>,
}

impl VideoFrame {
    /// Total bytes held, for cache accounting.
    pub fn byte_size(&self) -> usize {
        self.planes.iter().map(|(d, _)| d.len()).sum()
    }

    pub fn plane(&self, i: usize) -> Option<(&[u8], usize)> {
        self.planes.get(i).map(|(d, s)| (d.as_slice(), *s))
    }
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts", &self.pts)
            .field("frame_index", &self.frame_index)
            .field("size", &format_args!("{}x{}", self.width, self.height))
            .field("format", &self.format)
            .field("bytes", &self.byte_size())
            .finish()
    }
}

/// Decoded audio, planar f32, which is what `cine-audio` already works in.
///
/// This type exists only to carry decoded samples out of the media layer. All
/// mixing, panning, metering and DSP stays in `cine-audio`; duplicating any of
/// it here would be a mistake.
#[derive(Clone)]
pub struct AudioFrame {
    pub pts: Timestamp,
    pub sample_rate: u32,
    /// One `Vec<f32>` per channel, all the same length.
    pub channels: Vec<Arc<Vec<f32>>>,
}

impl AudioFrame {
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Samples per channel.
    pub fn frame_count(&self) -> usize {
        self.channels.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn byte_size(&self) -> usize {
        self.channels.iter().map(|c| c.len() * 4).sum()
    }

    /// Interleave into the layout `cine-audio`'s `AudioBuffer` expects.
    pub fn to_interleaved(&self) -> Vec<f32> {
        let n = self.frame_count();
        let ch = self.channel_count();
        let mut out = vec![0.0; n * ch];
        for (c, data) in self.channels.iter().enumerate() {
            for (i, s) in data.iter().enumerate() {
                out[i * ch + c] = *s;
            }
        }
        out
    }

    /// Peak absolute sample, useful for a quick sanity check on decoded audio.
    pub fn peak(&self) -> f32 {
        self.channels
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

impl std::fmt::Debug for AudioFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioFrame")
            .field("pts", &self.pts)
            .field("rate", &self.sample_rate)
            .field("channels", &self.channel_count())
            .field("frames", &self.frame_count())
            .finish()
    }
}

/// Either kind of decoded output.
#[derive(Clone, Debug)]
pub enum Frame {
    Video(VideoFrame),
    Audio(AudioFrame),
}

impl Frame {
    pub fn pts(&self) -> Timestamp {
        match self {
            Frame::Video(v) => v.pts,
            Frame::Audio(a) => a.pts,
        }
    }
    pub fn byte_size(&self) -> usize {
        match self {
            Frame::Video(v) => v.byte_size(),
            Frame::Audio(a) => a.byte_size(),
        }
    }
}

/// Convert a decoded audio frame into a `cine-audio` buffer.
///
/// The media layer owns decode; `cine-audio` owns everything after it.
pub fn to_audio_buffer(frame: &AudioFrame) -> Result<Vec<f32>> {
    Ok(frame.to_interleaved())
}
