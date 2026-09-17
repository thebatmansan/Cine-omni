//! Identify a file: what container, what streams, what can this build decode.
//!
//! Probing is deliberately separate from opening for decode. Ingest needs to
//! catalogue thousands of files quickly and record honestly what they are,
//! including the ones it cannot decode. A file this build cannot decode is
//! still a first-class media identity — it is offline capability, not an error.

use crate::capability;
use crate::error::{Error, Result};
use crate::identity::{
    AudioStreamInfo, ContainerKind, ContentHash, MediaFile, StreamInfo, StreamKind, VideoStreamInfo,
};
use crate::time::Rate;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Codecs this build offers, as a human-readable list used in error messages
/// so a failure names the alternative.
///
/// Derived from [`crate::capability::REGISTRY`] rather than written by hand, so
/// it cannot drift out of agreement with what the code actually does.
pub fn decodable_codecs() -> String {
    capability::offered_summary()
}

/// Whether the decode path in this build handles a codec token.
///
/// Delegates to the capability registry, which is the single source of truth.
pub fn is_decodable(codec: &str) -> bool {
    capability::is_offered(codec, None)
}

/// The profile name for an AVC track, in the vocabulary the registry uses.
///
/// A profile the container parser cannot even name returns `None`, and such a
/// track is refused: it is more honest to report no decoder than to emit a
/// corrupt picture. 4:2:2 and 4:4:4 High profiles (`profile_idc` 122 and 244)
/// fail this parse and are therefore correctly refused.
fn h264_profile_name(track: &mp4::Mp4Track) -> Option<&'static str> {
    match track.video_profile() {
        Ok(mp4::AvcProfile::AvcConstrainedBaseline) => Some("Baseline"),
        Ok(mp4::AvcProfile::AvcBaseline) => Some("Baseline"),
        Ok(mp4::AvcProfile::AvcMain) => Some("Main"),
        Ok(mp4::AvcProfile::AvcHigh) => Some("High"),
        Ok(mp4::AvcProfile::AvcExtended) | Err(_) => None,
    }
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
        let time_base = Rate::whole(track.timescale()).ok_or(Error::InvalidTimeBase {
            num: track.timescale(),
            den: 1,
        })?;
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
                let rate = Rate::new(track.timescale(), sample_delta(track));
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
                StreamKind::Video => match h264_profile_name(track) {
                    Some(profile) => capability::is_offered(&codec, Some(profile)),
                    None => false,
                },
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

    // The `mp4` crate names only AAC sound. Uncompressed sound in a QuickTime
    // movie (`lpcm`, `sowt`, `in24`, ...) is named and described by the
    // demuxer that decodes it, so the stream says what it is rather than
    // "unknown" and undecodable while it decodes perfectly well.
    if streams
        .iter()
        .any(|s| s.kind == StreamKind::Audio && s.codec == "unknown")
    {
        if let Ok(other) = probe_with_symphonia(path, hash.clone(), ContainerKind::IsoBmff, "mov") {
            let found: Vec<&StreamInfo> = other
                .streams
                .iter()
                .filter(|s| s.codec != "unknown")
                .collect();
            for s in streams
                .iter_mut()
                .filter(|s| s.kind == StreamKind::Audio && s.codec == "unknown")
            {
                // Symphonia numbers tracks by position from zero; MP4 track
                // ids conventionally count the same tracks from one.
                if let Some(m) = found.iter().find(|m| m.index + 1 == s.index) {
                    s.codec = m.codec.clone();
                    s.decodable = m.decodable;
                    s.audio = m.audio;
                    s.time_base = m.time_base;
                    s.duration_ticks = m.duration_ticks;
                }
            }
        }
    }

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
                .and_then(|tb| Rate::new(tb.denom, tb.numer))
                .unwrap_or(Rate::whole(rate).ok_or(Error::InvalidTimeBase { num: rate, den: 1 })?),
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
            time_base: Rate::whole(1).expect("1 Hz"),
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
