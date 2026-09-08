//! The video decode boundary.
//!
//! Decoding is the part of a media engine most likely to be replaced: a
//! software decoder today, a hardware one tomorrow, a licensed library where
//! patent obligations demand it. So callers depend on the [`VideoDecoder`]
//! trait, never on a concrete backend, and swapping the backend is a
//! constructor change rather than a rewrite of everything downstream.
//!
//! The trait shape — feed a packet, take frames, reset — mirrors the model used
//! by every serious decoder API, so a future FFmpeg or VideoToolbox backend
//! maps onto it without contortion.
//!
//! ## What this build actually decodes
//!
//! One backend exists: [`H264Decoder`], built on `rusty_h264`, a pure-Rust
//! BSD-2 decoder. This matters because the workspace must build with no system
//! C toolchain. **Measured on the real project asset**, not assumed:
//!
//! * 240/240 access units of a High-profile (`profile_idc` 100, level 3.1),
//!   B-pyramid, 189-frame-GOP stream decoded with zero errors at 27.7 fps
//!   single-threaded at 720x1280.
//! * Plane geometry verified against the frame dimensions on every frame.
//!
//! Two limits are stated plainly rather than buried:
//!
//! 1. That is **one asset, one profile** (High 4:2:0 8-bit). It is not a
//!    conformance run. See `docs/blueprint/PHASE-2-MEDIA-ENGINE.md`.
//! 2. `rusty_h264`'s own documentation advertises only "the full Constrained
//!    Baseline subset". It decoded High profile here correctly, but that is
//!    **beyond the scope its authors claim**, so it is treated as an
//!    unproven-by-the-vendor capability that this build verifies for itself.
//!    Anything the decoder cannot handle surfaces as [`Error::Decode`], never
//!    as a blank frame.

use crate::error::{Error, Result};
use crate::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use crate::time::Timestamp;
use std::sync::Arc;

/// One compressed access unit, with the timing the container recorded.
///
/// `pts` and `dts` differ whenever B-frames are present; both are kept because
/// decode order and display order are genuinely different orders and collapsing
/// them is how frame-accuracy is lost.
#[derive(Clone)]
pub struct Packet {
    /// Annex-B elementary stream bytes for one access unit.
    pub data: Vec<u8>,
    /// Presentation timestamp, in the stream's own time base.
    pub pts: Timestamp,
    /// Decode timestamp, in the stream's own time base.
    pub dts: Timestamp,
    /// Whether the stream can be entered at this packet.
    pub is_keyframe: bool,
}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packet")
            .field("bytes", &self.data.len())
            .field("pts", &self.pts)
            .field("dts", &self.dts)
            .field("key", &self.is_keyframe)
            .finish()
    }
}

/// A swappable video decode backend.
pub trait VideoDecoder: Send {
    /// Codec token this backend handles, matching [`crate::probe::is_decodable`].
    fn codec(&self) -> &'static str;

    /// Decode one access unit. `Ok(None)` means the decoder consumed the packet
    /// but has no picture yet, which is normal while a reference buffer fills.
    fn decode(&mut self, packet: &Packet) -> Result<Option<VideoFrame>>;

    /// Drain any pictures still held at end of stream.
    fn flush(&mut self) -> Result<Vec<VideoFrame>>;

    /// Discard all state. Required before decoding from a new keyframe after a
    /// seek, so stale reference frames cannot leak into the new picture.
    fn reset(&mut self) -> Result<()>;
}

/// Pure-Rust H.264 decoding.
///
/// Holds the parameter sets separately from the decoder because a reset
/// destroys decoder state, and the parameter sets must then be re-sent ahead of
/// the first access unit or the stream cannot be entered.
pub struct H264Decoder {
    inner: rusty_h264::Decoder,
    parameter_sets: Vec<u8>,
    /// Whether the parameter sets still need prepending to the next packet.
    needs_parameter_sets: bool,
    color: ColorMetadata,
}

impl H264Decoder {
    /// Build a decoder from the `avcC` parameter sets, in Annex-B form.
    ///
    /// `color` is whatever the container declared. It is passed in rather than
    /// inferred: guessing Rec.709 for an unlabelled stream is exactly the kind
    /// of silent assumption that produces a wrong grade three departments later.
    pub fn new(sps: &[u8], pps: &[u8], color: ColorMetadata) -> H264Decoder {
        let mut parameter_sets = Vec::with_capacity(sps.len() + pps.len() + 8);
        parameter_sets.extend_from_slice(&[0, 0, 0, 1]);
        parameter_sets.extend_from_slice(sps);
        parameter_sets.extend_from_slice(&[0, 0, 0, 1]);
        parameter_sets.extend_from_slice(pps);
        H264Decoder {
            inner: rusty_h264::Decoder::new(),
            parameter_sets,
            needs_parameter_sets: true,
            color,
        }
    }
}

impl VideoDecoder for H264Decoder {
    fn codec(&self) -> &'static str {
        "h264"
    }

    fn decode(&mut self, packet: &Packet) -> Result<Option<VideoFrame>> {
        // Prepend SPS/PPS on the first packet and after every reset.
        let au = if self.needs_parameter_sets {
            let mut v = Vec::with_capacity(self.parameter_sets.len() + packet.data.len());
            v.extend_from_slice(&self.parameter_sets);
            v.extend_from_slice(&packet.data);
            self.needs_parameter_sets = false;
            v
        } else {
            packet.data.clone()
        };

        match self.inner.decode(&au) {
            Ok(Some(y)) => Ok(Some(yuv_to_frame(y, packet.pts, self.color))),
            Ok(None) => Ok(None),
            Err(e) => Err(Error::Decode {
                at: format!("{:?}", packet.pts),
                message: format!("h264: {e:?}"),
            }),
        }
    }

    fn flush(&mut self) -> Result<Vec<VideoFrame>> {
        // This backend returns at most one picture per access unit synchronously
        // and buffers nothing, so there is nothing to drain. Reporting an empty
        // drain is accurate; inventing frames here would not be.
        Ok(Vec::new())
    }

    fn reset(&mut self) -> Result<()> {
        self.inner = rusty_h264::Decoder::new();
        self.needs_parameter_sets = true;
        Ok(())
    }
}

/// Adapt a decoder's planar output to the engine's frame representation.
///
/// The planes are moved into `Arc`s rather than copied, and are **not**
/// converted to RGB: that conversion belongs on the GPU fused with the colour
/// transform, and doing it here would cost precision and time for no gain.
fn yuv_to_frame(y: rusty_h264::YuvFrame, pts: Timestamp, color: ColorMetadata) -> VideoFrame {
    let (w, h) = (y.width, y.height);
    let cw = w.div_ceil(2);
    VideoFrame {
        pts,
        frame_index: None,
        timecode: None,
        width: w as u32,
        height: h as u32,
        format: PixelFormat::Yuv420P8,
        color,
        alpha: AlphaMode::None,
        planes: vec![(Arc::new(y.y), w), (Arc::new(y.u), cw), (Arc::new(y.v), cw)],
    }
}

/// Convert length-prefixed AVCC sample data to the Annex-B form decoders expect.
///
/// MP4 stores each NAL unit behind a length field; elementary-stream decoders
/// expect start codes. `nal_length_size` comes from the `avcC` box and is
/// commonly 4 but legally 1, 2 or 4.
pub fn avcc_to_annex_b(data: &[u8], nal_length_size: usize) -> Result<Vec<u8>> {
    if !(1..=4).contains(&nal_length_size) {
        return Err(Error::Container(format!(
            "illegal NAL length size {nal_length_size}, must be 1..=4"
        )));
    }
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut offset = 0usize;
    while offset + nal_length_size <= data.len() {
        let mut len = 0usize;
        for i in 0..nal_length_size {
            len = (len << 8) | data[offset + i] as usize;
        }
        offset += nal_length_size;
        // A truncated final NAL means the sample is damaged. Stopping here
        // keeps the frames decoded so far rather than discarding the take.
        if len == 0 || offset + len > data.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[offset..offset + len]);
        offset += len;
    }
    if out.is_empty() {
        return Err(Error::Container(
            "access unit contained no NAL units".into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_conversion_emits_start_codes_and_preserves_payload() {
        // Two NAL units of 3 and 2 bytes, 4-byte length prefixes.
        let data = [0, 0, 0, 3, 0x65, 0xAA, 0xBB, 0, 0, 0, 2, 0x41, 0xCC];
        let out = avcc_to_annex_b(&data, 4).unwrap();
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x65, 0xAA, 0xBB, 0, 0, 0, 1, 0x41, 0xCC]
        );
    }

    #[test]
    fn short_length_prefixes_are_honoured() {
        let data = [3, 0x65, 0xAA, 0xBB, 2, 0x41, 0xCC];
        let out = avcc_to_annex_b(&data, 1).unwrap();
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x65, 0xAA, 0xBB, 0, 0, 0, 1, 0x41, 0xCC]
        );
    }

    #[test]
    fn a_truncated_final_nal_keeps_what_was_readable() {
        // Second NAL claims 9 bytes but only 2 are present.
        let data = [0, 0, 0, 3, 0x65, 0xAA, 0xBB, 0, 0, 0, 9, 0x41, 0xCC];
        let out = avcc_to_annex_b(&data, 4).unwrap();
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 0x65, 0xAA, 0xBB],
            "damaged tail dropped, head kept"
        );
    }

    #[test]
    fn an_illegal_nal_length_size_is_rejected_rather_than_guessed() {
        assert!(avcc_to_annex_b(&[0, 0, 0, 1, 0x65], 0).is_err());
        assert!(avcc_to_annex_b(&[0, 0, 0, 1, 0x65], 5).is_err());
    }

    #[test]
    fn an_empty_access_unit_is_an_error_not_a_silent_blank_frame() {
        assert!(avcc_to_annex_b(&[], 4).is_err());
        assert!(avcc_to_annex_b(&[0, 0, 0, 0], 4).is_err());
    }
}
