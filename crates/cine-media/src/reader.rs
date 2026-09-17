//! Demux, decode and seek.
//!
//! ```text
//! MediaFile → MediaReader → demux packets → decode → Frame → FrameCache → consumer
//! ```
//!
//! The reader is synchronous and single-threaded by design at this stage. The
//! blueprint specifies the threaded pipeline with backpressure and cancellation
//! that comes later; building that before a correct synchronous path exists
//! would make the correctness questions harder to answer, not easier.
//!
//! **Seeking is frame-accurate by construction.** Compressed video can only be
//! entered at a keyframe, so seeking means: locate the keyframe at or before
//! the target, then decode forward, discarding frames until the target is
//! reached. Audio is seeked the same way. Nothing here rounds through seconds.

use crate::error::{Error, Result};
use crate::frame::AudioFrame;
use crate::identity::{MediaFile, StreamKind};
use crate::probe::{decodable_codecs, is_decodable};
use crate::time::{Rate, Rounding, Timestamp};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::Decoder;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

/// An open media file, ready to demux and decode.
pub struct MediaReader {
    info: MediaFile,
    format: Box<dyn FormatReader>,
    /// Lazily constructed decoder for the selected audio stream.
    audio_decoder: Option<Box<dyn Decoder>>,
    audio_track_id: Option<u32>,
    audio_time_base: Rate,
}

impl MediaReader {
    /// Open a previously probed file for reading.
    pub fn open(info: MediaFile) -> Result<MediaReader> {
        let file =
            File::open(&info.path).map_err(|_| Error::Offline(info.path.display().to_string()))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = info.path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &Default::default(), &Default::default())
            .map_err(|e| Error::Container(format!("{}: {e}", info.path.display())))?;
        let format = probed.format;

        // Pick the first track the codec registry can actually build a decoder
        // for. A track we cannot decode is not silently substituted.
        let (audio_track_id, audio_time_base) = format
            .tracks()
            .iter()
            .find(|t| {
                symphonia::default::get_codecs()
                    .make(&t.codec_params, &Default::default())
                    .is_ok()
            })
            .map(|t| {
                let tb = t
                    .codec_params
                    .time_base
                    .and_then(|tb| Rate::new(tb.denom, tb.numer))
                    .unwrap_or(Rate::AUDIO_48K);
                (Some(t.id), tb)
            })
            .unwrap_or((None, Rate::AUDIO_48K));

        Ok(MediaReader {
            info,
            format,
            audio_decoder: None,
            audio_track_id,
            audio_time_base,
        })
    }

    /// Convenience: probe then open.
    pub fn open_path(path: &Path) -> Result<MediaReader> {
        MediaReader::open(crate::probe::probe(path)?)
    }

    pub fn info(&self) -> &MediaFile {
        &self.info
    }

    /// The time base of the decodable audio stream.
    pub fn audio_time_base(&self) -> Rate {
        self.audio_time_base
    }

    /// Explain why a stream cannot be decoded, rather than returning silence.
    pub fn require_decodable(&self, kind: StreamKind) -> Result<&crate::identity::StreamInfo> {
        let stream = self
            .info
            .streams
            .iter()
            .find(|s| s.kind == kind)
            .ok_or(Error::NoSuchStream(0))?;
        if !stream.decodable {
            return Err(Error::NoDecoder {
                codec: stream.codec.clone(),
                available: decodable_codecs(),
            });
        }
        Ok(stream)
    }

    fn decoder(&mut self) -> Result<&mut Box<dyn Decoder>> {
        if self.audio_decoder.is_none() {
            let id = self.audio_track_id.ok_or_else(|| {
                let codec = self
                    .info
                    .first_audio()
                    .map(|s| s.codec.clone())
                    .unwrap_or_else(|| "none".into());
                Error::NoDecoder {
                    codec,
                    available: decodable_codecs(),
                }
            })?;
            let track = self
                .format
                .tracks()
                .iter()
                .find(|t| t.id == id)
                .ok_or(Error::NoSuchStream(id))?;
            let dec = symphonia::default::get_codecs()
                .make(&track.codec_params, &Default::default())
                .map_err(|e| Error::NoDecoder {
                    codec: format!("{:?} ({e})", track.codec_params.codec),
                    available: decodable_codecs(),
                })?;
            self.audio_decoder = Some(dec);
        }
        Ok(self.audio_decoder.as_mut().expect("just set"))
    }

    /// Decode the next audio frame, or `None` at end of stream.
    ///
    /// Packets belonging to other streams are skipped. Decode errors that
    /// symphonia marks as recoverable (a corrupt packet in an otherwise sound
    /// file) are skipped with the stream continuing, which is what a
    /// professional tool must do rather than abandoning the whole take.
    pub fn next_audio_frame(&mut self) -> Result<Option<AudioFrame>> {
        let track_id = match self.audio_track_id {
            Some(id) => id,
            None => {
                let codec = self
                    .info
                    .first_audio()
                    .map(|s| s.codec.clone())
                    .unwrap_or_else(|| "none".into());
                return Err(Error::NoDecoder {
                    codec,
                    available: decodable_codecs(),
                });
            }
        };
        let base = self.audio_time_base;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None)
                }
                Err(e) => {
                    return Err(Error::Decode {
                        at: "demux".into(),
                        message: e.to_string(),
                    })
                }
            };
            if packet.track_id() != track_id {
                continue;
            }
            let pts = Timestamp::exact(packet.ts() as i64, base);
            let decoder = self.decoder()?;
            match decoder.decode(&packet) {
                Ok(buf) => return Ok(Some(to_audio_frame(buf, pts))),
                // Recoverable: skip this packet, keep the stream alive.
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(symphonia::core::errors::Error::ResetRequired) => {
                    self.audio_decoder = None;
                    continue;
                }
                Err(e) => {
                    return Err(Error::Decode {
                        at: format!("{pts:?}"),
                        message: e.to_string(),
                    })
                }
            }
        }
    }

    /// Seek so the next decoded audio frame is at or before `target`.
    ///
    /// Returns the timestamp the demuxer actually landed on, which is the
    /// nearest entry point at or before the request. Callers wanting exactness
    /// decode forward from here and discard until they reach `target`; that is
    /// what [`MediaReader::decode_audio_at`] does.
    pub fn seek_audio(&mut self, target: Timestamp) -> Result<Timestamp> {
        let base = self.audio_time_base;
        let ts = target
            .rescale_checked(base, Rounding::Nearest)
            .0
            .ticks()
            .max(0) as u64;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    ts,
                    track_id: self.audio_track_id.unwrap_or(0),
                },
            )
            .map_err(|e| Error::SeekOutOfRange {
                target: format!("{target:?}"),
                start: "0".into(),
                end: e.to_string(),
            })?;
        // A seek invalidates decoder state.
        if let Some(d) = self.audio_decoder.as_mut() {
            d.reset();
        }
        Ok(Timestamp::exact(seeked.actual_ts as i64, base))
    }

    /// Seek then decode forward until the frame containing `target` is reached.
    ///
    /// This is the frame-accurate path: the demuxer can only enter at a sync
    /// point, so exactness comes from decoding forward and discarding.
    pub fn decode_audio_at(&mut self, target: Timestamp) -> Result<Option<AudioFrame>> {
        self.seek_audio(target)?;
        let base = self.audio_time_base;
        let want = target.rescale_checked(base, Rounding::Nearest).0.ticks();
        while let Some(frame) = self.next_audio_frame()? {
            let start = frame.pts.rescale_checked(base, Rounding::Nearest).0.ticks();
            let end = start + frame.frame_count() as i64;
            if end > want {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }

    /// Decode every audio frame from the current position, up to `limit`
    /// frames. Used by waveform generation and by tests.
    pub fn decode_audio_frames(&mut self, limit: usize) -> Result<Vec<AudioFrame>> {
        let mut out = Vec::new();
        while out.len() < limit {
            match self.next_audio_frame()? {
                Some(f) => out.push(f),
                None => break,
            }
        }
        Ok(out)
    }
}

/// Convert symphonia's decoded buffer into the crate's planar representation.
fn to_audio_frame(buf: AudioBufferRef<'_>, pts: Timestamp) -> AudioFrame {
    fn planar<F>(spec_rate: u32, chans: usize, len: usize, pts: Timestamp, get: F) -> AudioFrame
    where
        F: Fn(usize, usize) -> f32,
    {
        let channels = (0..chans)
            .map(|c| Arc::new((0..len).map(|i| get(c, i)).collect::<Vec<f32>>()))
            .collect();
        AudioFrame {
            pts,
            sample_rate: spec_rate,
            channels,
        }
    }

    match buf {
        AudioBufferRef::F32(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| b.chan(c)[i])
        }
        AudioBufferRef::S16(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| b.chan(c)[i] as f32 / 32_768.0)
        }
        AudioBufferRef::S32(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                b.chan(c)[i] as f32 / 2_147_483_648.0
            })
        }
        AudioBufferRef::U8(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                (b.chan(c)[i] as f32 - 128.0) / 128.0
            })
        }
        // Every sample type symphonia produces, and no wildcard: a type added
        // later fails to compile here rather than reading as silence. Integers
        // are scaled by 2^(bits-1), the convention the other types above use,
        // so a full-scale negative sample is exactly -1.
        AudioBufferRef::S8(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| f32::from(b.chan(c)[i]) / 128.0)
        }
        AudioBufferRef::S24(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                (f64::from(b.chan(c)[i].inner()) / 8_388_608.0) as f32
            })
        }
        AudioBufferRef::U16(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                (f32::from(b.chan(c)[i]) - 32_768.0) / 32_768.0
            })
        }
        AudioBufferRef::U24(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                ((f64::from(b.chan(c)[i].inner()) - 8_388_608.0) / 8_388_608.0) as f32
            })
        }
        AudioBufferRef::U32(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| {
                ((f64::from(b.chan(c)[i]) - 2_147_483_648.0) / 2_147_483_648.0) as f32
            })
        }
        AudioBufferRef::F64(b) => {
            let (rate, ch, len) = (b.spec().rate, b.spec().channels.count(), b.frames());
            planar(rate, ch, len, pts, |c, i| b.chan(c)[i] as f32)
        }
    }
}

/// Decode a still image into an 8-bit RGBA frame.
///
/// Image sequences are how VFX and finishing actually move picture, so this is
/// a real media path rather than a convenience.
pub fn decode_image(path: &Path) -> Result<crate::frame::VideoFrame> {
    use crate::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
    let img = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| Error::Decode {
            at: path.display().to_string(),
            message: e.to_string(),
        })?
        .decode()
        .map_err(|e| Error::Decode {
            at: path.display().to_string(),
            message: e.to_string(),
        })?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let stride = (w * 4) as usize;
    Ok(VideoFrame {
        pts: Timestamp::exact(0, Rate::whole(1).expect("1 Hz")),
        frame_index: Some(0),
        timecode: None,
        width: w,
        height: h,
        format: PixelFormat::Rgba8,
        // A PNG or TIFF carries no H.273 code points; claiming Rec.709 would be
        // a guess, so the metadata stays honestly empty.
        color: ColorMetadata::default(),
        alpha: AlphaMode::Straight,
        planes: vec![(Arc::new(rgba.into_raw()), stride)],
    })
}

/// Check that `is_decodable` and the actual codec registry agree.
///
/// Capability claims drift from reality unless something forces them together;
/// this is called by the test suite for exactly that reason.
pub fn capability_matches_registry(codec: &str, params_codec_supported: bool) -> bool {
    is_decodable(codec) == params_codec_supported
}
