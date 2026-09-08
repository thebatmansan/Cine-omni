//! Identify a file: what container, what streams, what can this build decode.
//!
//! Probing is deliberately separate from opening for decode. Ingest needs to
//! catalogue thousands of files quickly and record honestly what they are,
//! including the ones it cannot decode. A file this build cannot decode is
//! still a first-class media identity — it is offline capability, not an error.

use crate::error::{Error, Result};
use crate::identity::{
    AudioStreamInfo, ContainerKind, ContentHash, MediaFile, StreamInfo, StreamKind, VideoStreamInfo,
};
use crate::time::{FrameRate, TimeBase};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Codecs this build can genuinely decode to samples, as a human-readable list
/// used in error messages so a failure names the alternative.
pub const DECODABLE_CODECS: &str =
    "h264 (video); aac, mp3, flac, alac, pcm, vorbis (audio); png, tiff, jpeg (image)";

/// Whether the decode path in this build handles a codec token.
///
/// This is the single source of truth for capability. It reflects what is
/// actually wired up, not what a dependency could theoretically do.
pub fn is_decodable(codec: &str) -> bool {
    matches!(
        codec,
        "h264"
            | "aac"
            | "mp3"
            | "flac"
            | "alac"
            | "vorbis"
            | "pcm"
            | "pcm_s16le"
            | "pcm_s24le"
            | "pcm_f32le"
    )
}

/// Whether the H.264 decode backend can be expected to handle this track.
///
/// The gate is the profile, because that is what actually determines whether
/// the coding tools in the stream are ones the decoder implements. A profile
/// the container parser cannot even name is refused rather than attempted: it
/// is more honest to report no decoder than to emit a corrupt picture.
///
/// 4:2:2 and 4:4:4 High profiles (`profile_idc` 122 and 244) fail the profile
/// parse and are therefore correctly refused; only 4:2:0 8-bit is claimed.
fn h264_profile_supported(track: &mp4::Mp4Track) -> bool {
    matches!(
        track.video_profile(),
        Ok(mp4::AvcProfile::AvcConstrainedBaseline
            | mp4::AvcProfile::AvcBaseline
            | mp4::AvcProfile::AvcMain
            | mp4::AvcProfile::AvcHigh)
    )
}

/// Identify a media file without decoding it.
///
/// Uses the `mp4` crate for ISO-BMFF because it exposes the sample tables,
/// per-track timescales and frame counts that editorial needs, which the
/// streaming-oriented demuxer does not surface.
pub fn probe(path: &Path) -> Result<MediaFile> {
    if !path.exists() {
        return Err(Error::NotFound(path.display().to_string()));
    }
    let hash = ContentHash::fast(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "mp4" | "m4v" | "m4a" | "mov" => probe_isobmff(path, hash),
        "wav" | "wave" => probe_with_symphonia(path, hash, ContainerKind::Wave, "wav"),
        "flac" => probe_with_symphonia(path, hash, ContainerKind::IsoBmff, "flac"),
        "mp3" => probe_with_symphonia(path, hash, ContainerKind::IsoBmff, "mp3"),
        "mkv" | "webm" => probe_with_symphonia(path, hash, ContainerKind::Matroska, "mkv"),
        "png" | "tif" | "tiff" | "jpg" | "jpeg" => probe_image(path, hash),
        other => Err(Error::UnknownContainer(format!(
            "{} (extension `{other}`)",
            path.display()
        ))),
    }
}

/// ISO base media (MP4/MOV): read the sample tables for exact per-track timing.
fn probe_isobmff(path: &Path, hash: ContentHash) -> Result<MediaFile> {
    let f = File::open(path)?;
    let size = f.metadata()?.len();
    let reader = mp4::Mp4Reader::read_header(BufReader::new(f), size)
        .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;

    let mut streams = Vec::new();
    for (id, track) in reader.tracks() {
        let time_base = TimeBase::from_rate(track.timescale())?;
        let codec = match track.media_type() {
            Ok(mp4::MediaType::H264) => "h264",
            Ok(mp4::MediaType::H265) => "h265",
            Ok(mp4::MediaType::VP9) => "vp9",
            Ok(mp4::MediaType::AAC) => "aac",
            Ok(mp4::MediaType::TTXT) => "ttxt",
            Err(_) => "unknown",
        }
        .to_string();

        let (kind, video, audio) = match track.track_type() {
            Ok(mp4::TrackType::Video) => {
                // The container states a nominal rate; a stream whose sample
                // durations vary is VFR and must be conformed by editorial.
                let rate = FrameRate::new(track.timescale(), sample_delta(track)).ok();
                (
                    StreamKind::Video,
                    Some(VideoStreamInfo {
                        width: track.width() as u32,
                        height: track.height() as u32,
                        frame_rate: rate,
                        frame_count: Some(u64::from(track.sample_count())),
                        variable_frame_rate: rate.is_none(),
                    }),
                    None,
                )
            }
            Ok(mp4::TrackType::Audio) => (
                StreamKind::Audio,
                None,
                Some(AudioStreamInfo {
                    sample_rate: track
                        .sample_freq_index()
                        .map(|i| i.freq())
                        .unwrap_or(track.timescale()),
                    channels: track.channel_config().map(|c| c as u16).unwrap_or(2),
                    bits_per_sample: None,
                }),
            ),
            Ok(mp4::TrackType::Subtitle) => (StreamKind::Subtitle, None, None),
            Err(_) => (StreamKind::Data, None, None),
        };

        streams.push(StreamInfo {
            index: *id,
            kind,
            decodable: match kind {
                StreamKind::Audio => is_decodable(&codec),
                StreamKind::Video => codec == "h264" && h264_profile_supported(track),
                _ => false,
            },
            codec,
            time_base,
            duration_ticks: Some(
                track.duration().as_millis() as i64 * i64::from(track.timescale()) / 1000,
            ),
            video,
            audio,
        });
    }
    streams.sort_by_key(|s| s.index);

    Ok(MediaFile {
        path: path.to_path_buf(),
        hash,
        container: ContainerKind::IsoBmff,
        streams,
        start_timecode: None,
        source: None,
    })
}

/// Ticks between consecutive samples, used to derive the nominal frame rate.
fn sample_delta(track: &mp4::Mp4Track) -> u32 {
    let count = track.sample_count();
    if count == 0 {
        return 1;
    }
    // duration is in the track timescale; dividing by sample count gives the
    // mean sample duration, which for CFR media is the exact frame duration.
    let ticks = track.duration().as_millis() as u64 * u64::from(track.timescale()) / 1000;
    ((ticks / u64::from(count)).max(1)) as u32
}

/// Containers handled by the streaming demuxer.
fn probe_with_symphonia(
    path: &Path,
    hash: ContentHash,
    container: ContainerKind,
    hint_ext: &str,
) -> Result<MediaFile> {
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(hint_ext);
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;

    let mut streams = Vec::new();
    for t in probed.format.tracks() {
        let p = &t.codec_params;
        let codec = codec_token(p.codec);
        let rate = p.sample_rate.unwrap_or(48_000);
        streams.push(StreamInfo {
            index: t.id,
            kind: StreamKind::Audio,
            decodable: symphonia::default::get_codecs()
                .make(p, &Default::default())
                .is_ok(),
            codec,
            time_base: p
                .time_base
                .map(|tb| TimeBase::new(tb.numer, tb.denom))
                .transpose()?
                .unwrap_or(TimeBase::from_rate(rate)?),
            duration_ticks: p.n_frames.map(|n| n as i64),
            video: None,
            audio: Some(AudioStreamInfo {
                sample_rate: rate,
                channels: p.channels.map(|c| c.count() as u16).unwrap_or(2),
                bits_per_sample: p.bits_per_sample.map(|b| b as u16),
            }),
        });
    }

    Ok(MediaFile {
        path: path.to_path_buf(),
        hash,
        container,
        streams,
        start_timecode: None,
        source: None,
    })
}

/// A still image, which may be one frame of a sequence.
fn probe_image(path: &Path, hash: ContentHash) -> Result<MediaFile> {
    let reader = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;
    let format = reader
        .format()
        .map(|f| format!("{f:?}").to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| Error::Container(format!("{}: {e}", path.display())))?;

    Ok(MediaFile {
        path: path.to_path_buf(),
        hash,
        container: ContainerKind::Image,
        streams: vec![StreamInfo {
            index: 0,
            kind: StreamKind::Video,
            codec: format,
            // A still has no meaningful time base of its own; it takes the rate
            // of the sequence it belongs to. 1/1 is the honest placeholder.
            time_base: TimeBase::new(1, 1)?,
            duration_ticks: Some(1),
            video: Some(VideoStreamInfo {
                width: w,
                height: h,
                frame_rate: None,
                frame_count: Some(1),
                variable_frame_rate: false,
            }),
            audio: None,
            decodable: true,
        }],
        start_timecode: None,
        source: None,
    })
}

fn codec_token(c: symphonia::core::codecs::CodecType) -> String {
    use symphonia::core::codecs::*;
    match c {
        CODEC_TYPE_AAC => "aac",
        CODEC_TYPE_MP3 => "mp3",
        CODEC_TYPE_FLAC => "flac",
        CODEC_TYPE_ALAC => "alac",
        CODEC_TYPE_VORBIS => "vorbis",
        CODEC_TYPE_PCM_S16LE => "pcm_s16le",
        CODEC_TYPE_PCM_S24LE => "pcm_s24le",
        CODEC_TYPE_PCM_F32LE => "pcm_f32le",
        _ => "unknown",
    }
    .to_string()
}
