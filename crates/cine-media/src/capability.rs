//! The codec capability registry — the single auditable source of truth for
//! what this build may claim to decode.
//!
//! ## Why a registry rather than a `matches!` arm
//!
//! Capability claims rot. A decoder gets swapped, a feature flag changes, a
//! dependency quietly widens its support, and the claim scattered across
//! `probe`, the error messages and the documentation drifts out of agreement
//! with reality. Worse, the drift is always in the optimistic direction,
//! because nobody removes a claim.
//!
//! So capability lives in exactly one table, and three things derive from it:
//! the decodability flag written onto every stream, the human-readable list in
//! [`Error::NoDecoder`], and the capability matrix in the blueprint.
//!
//! ## The distinction that matters
//!
//! Every entry carries a [`Verification`] state, and the states are **not**
//! interchangeable:
//!
//! * [`Verification::Verified`] — a real file of this exact shape was decoded
//!   by the test suite on this machine, and the entry names the evidence.
//! * [`Verification::Unverified`] — the code path exists and is expected to
//!   work, but no real asset of this shape has been through it here. It is
//!   still *offered*, because refusing to try would be its own dishonesty; it
//!   simply may not be *claimed*.
//! * [`Verification::Refused`] — deliberately not attempted, with a reason.
//!   Refusing is a feature: emitting a corrupt picture is worse than reporting
//!   no decoder.
//!
//! `is_empty`-style convenience is deliberately absent here. Reading this table
//! should feel like reading an audit, because that is what it is.
//!
//! The test `every_verified_entry_names_its_evidence` enforces that a
//! `Verified` entry cannot be added without pointing at the test that proves
//! it, which is the mechanism that keeps this table honest over time.

use std::fmt;

/// Chroma sampling, as the property that actually decides whether a decoder's
/// coding tools apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chroma {
    Monochrome,
    Yuv420,
    Yuv422,
    Yuv444,
    /// RGB or RGBA, as still-image codecs produce.
    Rgb,
}

impl Chroma {
    pub fn as_str(&self) -> &'static str {
        match self {
            Chroma::Monochrome => "4:0:0",
            Chroma::Yuv420 => "4:2:0",
            Chroma::Yuv422 => "4:2:2",
            Chroma::Yuv444 => "4:4:4",
            Chroma::Rgb => "RGB",
        }
    }
}

/// Where the decoding happens.
///
/// `Hardware` exists in the type system so a future backend has a place to
/// declare itself. **No hardware entry exists in the registry**, and per the
/// project rule none may be added until it has been verified on real hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Software,
    Hardware,
}

/// How much confidence this build has earned for one capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Proven here against a real asset. `evidence` names the test.
    Verified { evidence: &'static str },
    /// Offered but unproven on this machine. `note` says what is missing.
    Unverified { note: &'static str },
    /// Deliberately not attempted.
    Refused { reason: &'static str },
}

impl Verification {
    /// Whether decode should be attempted at all.
    pub fn is_offered(&self) -> bool {
        !matches!(self, Verification::Refused { .. })
    }

    /// Whether this build has earned the right to *claim* the capability.
    ///
    /// Distinct from [`is_offered`](Verification::is_offered) on purpose: a
    /// great deal of code may be offered, and very little may be claimed.
    pub fn is_claimable(&self) -> bool {
        matches!(self, Verification::Verified { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verification::Verified { .. } => "VERIFIED",
            Verification::Unverified { .. } => "UNVERIFIED",
            Verification::Refused { .. } => "REFUSED",
        }
    }
}

/// One row of the capability table.
#[derive(Clone, Copy, Debug)]
pub struct Capability {
    /// Stable lowercase codec token, matching `StreamInfo::codec`.
    pub codec: &'static str,
    /// Profile constraint, or `None` when the codec has no profile axis.
    pub profile: Option<&'static str>,
    /// Highest level verified or expected. Informational.
    pub level: Option<&'static str>,
    pub bit_depth: u8,
    pub chroma: Chroma,
    pub backend: BackendKind,
    pub verification: Verification,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.codec)?;
        if let Some(p) = self.profile {
            write!(f, " {p}")?;
        }
        write!(
            f,
            " {}-bit {} [{}]",
            self.bit_depth,
            self.chroma.as_str(),
            self.verification.label()
        )
    }
}

/// The registry.
///
/// Ordered so that more specific rows precede more general ones; lookup takes
/// the first match. Every claim CINE-OMNI makes about decoding traces to a row
/// here.
pub const REGISTRY: &[Capability] = &[
    // ---------------------------------------------------------------- video
    Capability {
        codec: "h264",
        profile: Some("High"),
        level: Some("3.1"),
        bit_depth: 8,
        chroma: Chroma::Yuv420,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "real_media::decodes_real_h264_video_to_real_pictures \
                       (240/240 frames, profile_idc 100, level 3.1, B-pyramid)",
        },
    },
    // Same decoder, same coding tools, but no asset of these profiles has been
    // decoded here. Offered, not claimed.
    Capability {
        codec: "h264",
        profile: Some("Main"),
        level: None,
        bit_depth: 8,
        chroma: Chroma::Yuv420,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no Main-profile asset available on this machine",
        },
    },
    Capability {
        codec: "h264",
        profile: Some("Baseline"),
        level: None,
        bit_depth: 8,
        chroma: Chroma::Yuv420,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence:
                "encode_roundtrip::an_encoded_file_decodes_to_the_frames_that_went_in_in_order \
                       (Constrained Baseline written by this build, read back frame-exact)",
        },
    },
    // Refusals are entries too, so the reason travels with the decision.
    Capability {
        codec: "h264",
        profile: Some("High 4:2:2"),
        level: None,
        bit_depth: 10,
        chroma: Chroma::Yuv422,
        backend: BackendKind::Software,
        verification: Verification::Refused {
            reason: "decoder implements 4:2:0 8-bit only; attempting it would \
                     emit a corrupt picture rather than fail",
        },
    },
    Capability {
        codec: "h264",
        profile: Some("High 4:4:4"),
        level: None,
        bit_depth: 10,
        chroma: Chroma::Yuv444,
        backend: BackendKind::Software,
        verification: Verification::Refused {
            reason: "decoder implements 4:2:0 8-bit only",
        },
    },
    Capability {
        codec: "h265",
        profile: None,
        level: None,
        bit_depth: 8,
        chroma: Chroma::Yuv420,
        backend: BackendKind::Software,
        verification: Verification::Refused {
            reason: "no HEVC decoder in this build; no credible pure-Rust \
                     implementation exists",
        },
    },
    Capability {
        codec: "prores",
        profile: None,
        level: None,
        bit_depth: 10,
        chroma: Chroma::Yuv422,
        backend: BackendKind::Software,
        verification: Verification::Refused {
            reason: "no ProRes decoder in this build",
        },
    },
    // ---------------------------------------------------------------- audio
    Capability {
        codec: "aac",
        profile: Some("LC"),
        level: None,
        bit_depth: 16,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "real_media::decodes_real_aac_audio_from_the_mp4 \
                       (48 kHz stereo, verified non-silent)",
        },
    },
    Capability {
        codec: "pcm",
        profile: None,
        level: None,
        bit_depth: 16,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "real_media::decodes_a_real_wav_file",
        },
    },
    Capability {
        codec: "pcm_s16le",
        profile: None,
        level: None,
        bit_depth: 16,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "real_media::decodes_a_real_wav_file",
        },
    },
    Capability {
        codec: "pcm_s24le",
        profile: None,
        level: None,
        bit_depth: 24,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "sound_formats::twenty_four_bit_sound_is_heard_exactly \
                       (every code read back exactly, in WAV and in QuickTime)",
        },
    },
    Capability {
        codec: "pcm_f32le",
        profile: None,
        level: None,
        bit_depth: 32,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "sound_formats::float_sound_is_heard_at_both_widths \
                       (32 and 64-bit IEEE float WAV)",
        },
    },
    Capability {
        codec: "mp3",
        profile: None,
        level: None,
        bit_depth: 16,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no MP3 asset here",
        },
    },
    Capability {
        codec: "flac",
        profile: None,
        level: None,
        bit_depth: 24,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no FLAC asset here",
        },
    },
    Capability {
        codec: "alac",
        profile: None,
        level: None,
        bit_depth: 24,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no ALAC asset here",
        },
    },
    Capability {
        codec: "vorbis",
        profile: None,
        level: None,
        bit_depth: 16,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no Vorbis asset here",
        },
    },
    // ---------------------------------------------------------------- image
    Capability {
        codec: "png",
        profile: None,
        level: None,
        bit_depth: 8,
        chroma: Chroma::Rgb,
        backend: BackendKind::Software,
        verification: Verification::Verified {
            evidence: "real_media::decodes_a_real_image_as_a_video_frame",
        },
    },
    Capability {
        codec: "jpeg",
        profile: None,
        level: None,
        bit_depth: 8,
        chroma: Chroma::Rgb,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no JPEG asset exercised here",
        },
    },
    Capability {
        codec: "tiff",
        profile: None,
        level: None,
        bit_depth: 8,
        chroma: Chroma::Rgb,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no TIFF asset exercised here",
        },
    },
];

/// How far a media *path* — not a codec, a whole way of getting media in or
/// out — has actually got.
///
/// The codec table above answers "may we decode this stream". This one answers
/// "does this capability exist, and how much of it", for the paths an
/// application claims: containers, seeking, timecode, colour and HDR metadata,
/// precision, image sequences, proxies, relinking, thumbnails, sound output.
/// Both tables are read by the `capabilities` method, so no interface can
/// claim what neither table states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathState {
    /// Proven here against real media, by a named test.
    Verified,
    /// Built and used by the application, but not proven end to end by a test
    /// that reads real media.
    Implemented,
    /// Works within stated limits; the limits are the detail.
    Partial,
    /// Exists, but not on a path the application uses for real work.
    Prototype,
    /// Deliberately not attempted, with the reason.
    Refused,
    /// Not built.
    Missing,
    /// Not buildable until something else lands; the detail names it.
    Blocked,
}

impl PathState {
    pub fn label(&self) -> &'static str {
        match self {
            PathState::Verified => "verified",
            PathState::Implemented => "implemented",
            PathState::Partial => "partial",
            PathState::Prototype => "prototype",
            PathState::Refused => "refused",
            PathState::Missing => "missing",
            PathState::Blocked => "blocked",
        }
    }

    /// Whether the application may offer this path to a person at all.
    pub fn is_offered(&self) -> bool {
        matches!(
            self,
            PathState::Verified
                | PathState::Implemented
                | PathState::Partial
                | PathState::Prototype
        )
    }
}

/// One media path, and what is true of it.
#[derive(Clone, Copy, Debug)]
pub struct MediaPath {
    /// Stable dotted id, e.g. `container.isobmff.read`.
    pub id: &'static str,
    /// What a person would call it.
    pub name: &'static str,
    /// Grouping for the capability list.
    pub area: &'static str,
    pub state: PathState,
    /// What is true, including the limit when there is one.
    pub detail: &'static str,
    /// The test that proves it. Required for `Verified`.
    pub evidence: Option<&'static str>,
}

/// Every media path this build has an opinion about.
///
/// Audited 2026-09-18 against the code, not against memory: each row was
/// checked by reading the path it describes.
pub const MEDIA_PATHS: &[MediaPath] = &[
    // ----------------------------------------------------------- containers
    MediaPath {
        id: "container.isobmff.read",
        name: "MP4 and QuickTime, read",
        area: "Containers",
        state: PathState::Verified,
        detail: "Sample tables read exactly (per-track timescale, sample sizes, composition \
                 offsets, 64-bit offsets), so durations and picture counts are the file's own",
        evidence: Some("cine-media/tests/real_media.rs, mov_roundtrip.rs"),
    },
    MediaPath {
        id: "container.quicktime.write",
        name: "QuickTime, written",
        area: "Containers",
        state: PathState::Verified,
        detail: "H.264 picture and 24-bit LPCM sound, edit lists, colr, pasp, chan; read back by \
                 this build and by GStreamer's qtdemux, which returns the exact sound bytes",
        evidence: Some("cine-media/tests/mov_roundtrip.rs::gstreamer_reads_the_same_movie"),
    },
    MediaPath {
        id: "container.wave.read",
        name: "WAVE, read",
        area: "Containers",
        state: PathState::Verified,
        detail: "Every integer and float layout: 8, 16, 24, 32-bit and 32/64-bit float, signed \
                 and unsigned, read back as the samples written",
        evidence: Some("cine-media/tests/sound_formats.rs"),
    },
    MediaPath {
        id: "container.matroska.read",
        name: "Matroska, read",
        area: "Containers",
        state: PathState::Partial,
        detail: "Demuxed for sound only: the probe reports every track as audio, so a picture \
                 track in an MKV is neither described nor decoded",
        evidence: None,
    },
    MediaPath {
        id: "container.mxf.read",
        name: "MXF, read",
        area: "Containers",
        state: PathState::Missing,
        detail: "No MXF parser in this build; broadcast and camera-original MXF cannot be opened",
        evidence: None,
    },
    MediaPath {
        id: "container.image_sequence",
        name: "Image sequences as media",
        area: "Containers",
        state: PathState::Verified,
        detail: "A numbered run of EXR, PNG, TIFF or JPEG files is one asset: its identity is the \
                 run (pattern, range, ends), its length is the numbers it covers, and it takes the \
                 rate it was ingested at, because a run of files has none. A number inside the \
                 range with no file is a hole: it refuses by name and is never shown black",
        evidence: Some("cine-media/tests/image_sequence.rs, cine-runtime/tests/sequence_protocol.rs"),
    },
    // -------------------------------------------------------------- picture
    MediaPath {
        id: "picture.h264.8bit",
        name: "H.264 8-bit 4:2:0 decode",
        area: "Picture",
        state: PathState::Verified,
        detail: "Frame-accurate, in display order, from any entry point; B-pyramid handled",
        evidence: Some("cine-media/tests/real_media.rs, encode_roundtrip.rs"),
    },
    MediaPath {
        id: "picture.high_bit_depth",
        name: "High-bit-depth picture input",
        area: "Picture",
        state: PathState::Partial,
        detail: "16-bit PNG and TIFF stills and 32-bit float EXR reach the pipeline at their own \
                 precision, unquantised; 10 and 12-bit *moving* picture does not, because the \
                 only movie decoder here is 8-bit 4:2:0. The limit is the input, and no path \
                 pretends otherwise",
        evidence: Some("cine-media/tests/image_sequence.rs::a_sixteen_bit_still_sequence_is_not_quantised_to_eight"),
    },
    MediaPath {
        id: "picture.float.read",
        name: "Float picture as media (EXR)",
        area: "Picture",
        state: PathState::Verified,
        detail: "EXR is read as 32-bit float whatever it was stored as (half, float or unsigned), \
                 unclamped: scene values above 1 and below 0 reach the frame. It cuts like any \
                 other media and its pictures reach the monitor, shown clamped and saying so",
        evidence: Some("cine-media/tests/image_sequence.rs::exr_pixels_reach_the_frame_as_the_floats_they_are"),
    },
    MediaPath {
        id: "picture.float.deliver",
        name: "Delivering scene-linear pictures",
        area: "Picture",
        state: PathState::Verified,
        detail: "Through the project's view, scene-linear pictures deliver to any display-encoded \
                 output (the transform is applied exactly, not through a lattice: highlights roll \
                 off rather than clip); as EXR they deliver unchanged, scene values intact. As \
                 decoded into display output they refuse, because that would clamp every \
                 highlight to white, and a proxy of them refuses because the baked view would be \
                 applied twice wherever it stood in",
        evidence: Some("cine-runtime/tests/sequence_protocol.rs::scene_linear_delivers_through_a_view_or_as_exr_and_refuses_to_be_clamped"),
    },
    MediaPath {
        id: "delivery.exr",
        name: "EXR delivery",
        area: "Derived media",
        state: PathState::Verified,
        detail: "A render writes 32-bit float RGBA EXR frames, unclamped and untransformed, with \
                 the same frame numbering as a PNG sequence; a round trip returns the source's \
                 scene values. Captions refuse: burn-in draws display white, which is not a scene \
                 value. Half-float output, deep data, layers and metadata beyond RGBA are not \
                 written",
        evidence: Some("cine-runtime/tests/sequence_protocol.rs::scene_linear_delivers_through_a_view_or_as_exr_and_refuses_to_be_clamped"),
    },
    MediaPath {
        id: "picture.alpha",
        name: "Alpha in media",
        area: "Picture",
        state: PathState::Partial,
        detail: "Still images decode with alpha; no moving-picture codec in this build carries \
                 alpha, and the compositor's alpha is its own",
        evidence: None,
    },
    MediaPath {
        id: "picture.still.read",
        name: "PNG, TIFF and JPEG stills",
        area: "Picture",
        state: PathState::Verified,
        detail: "Decoded to 8-bit RGBA as one frame; a still has no rate of its own and takes \
                 the sequence's",
        evidence: Some("cine-media/tests/real_media.rs::decodes_a_real_image_as_a_video_frame"),
    },
    // ----------------------------------------------------------------- time
    MediaPath {
        id: "time.duration.exact",
        name: "Exact duration and media time",
        area: "Time",
        state: PathState::Verified,
        detail: "Every time is integer ticks at an exact rational rate, from the container's own \
                 timescale; no float seconds anywhere in storage or comparison",
        evidence: Some("cine-media/tests/real_media.rs, ucg-core time tests"),
    },
    MediaPath {
        id: "time.seek.frame_accurate",
        name: "Frame-accurate seeking",
        area: "Time",
        state: PathState::Verified,
        detail: "Any picture by index or timestamp, decoded from the preceding entry point; \
                 out-of-order requests return the same pictures as sequential reads",
        evidence: Some("cine-media/tests/mov_roundtrip.rs, playback_real_media.rs"),
    },
    MediaPath {
        id: "time.vfr",
        name: "Variable frame rate",
        area: "Time",
        state: PathState::Partial,
        detail: "Detected from the sample table and flagged on the stream; editorial has no \
                 conform for it, and proxy generation refuses it rather than guess a rate",
        evidence: Some("cine-runtime/tests/proxy_protocol.rs (refusal path)"),
    },
    MediaPath {
        id: "time.timecode",
        name: "Source timecode",
        area: "Time",
        state: PathState::Verified,
        detail: "A QuickTime `tmcd` track is read: the exact rate as the ratio the file states \
                 (30000/1001, never 29.97), the drop-frame flag, and the frame number, decomposed \
                 into a label by the one implementation of drop-frame rules. The movie writer \
                 writes one, so a proxy carries its original's timecode; the media list, the \
                 inspector and the viewer's readout all show it. A file with no timecode track \
                 answers absent, never zero. What is not claimed: discontinuous timecode reads as \
                 the first run's start and says how many runs there are; a counter track is \
                 reported as a counter rather than as a clock; a rendered cut carries no record \
                 timecode, because a sequence has no start timecode in this build yet; and no \
                 independent timecode reader was available to check the bytes, so the layout is \
                 held to the published field offsets by a test that walks the boxes separately",
        evidence: Some("cine-media/tests/timecode_track.rs"),
    },
    // --------------------------------------------------------------- colour
    MediaPath {
        id: "colour.scene_linear",
        name: "Scene-linear pictures through a view",
        area: "Colour",
        state: PathState::Verified,
        detail: "Scene light is unbounded, so its view transform is applied exactly per pixel \
                 rather than through a lattice over code values: highlights several stops above \
                 white roll off in order, deep shadows keep their order, and negative samples \
                 stay finite. Display-referred input still takes the measured lattice, which is \
                 what the window's shader interpolates",
        evidence: Some("cine-color/tests/scene_linear_view.rs"),
    },
    MediaPath {
        id: "colour.stream_metadata",
        name: "Colour from the picture stream",
        area: "Colour",
        state: PathState::Verified,
        detail: "H.264 SPS video signal type: primaries, transfer, matrix and range, with \
                 unspecified code points left absent rather than assumed",
        evidence: Some("cine-media/tests/sps_real.rs"),
    },
    MediaPath {
        id: "colour.container_metadata",
        name: "Colour from the container",
        area: "Colour",
        state: PathState::Verified,
        detail: "QuickTime `colr` (nclc and nclx) fills what the stream leaves undeclared; the \
                 stream wins where both speak",
        evidence: Some("cine-media/tests/mov_roundtrip.rs (declared, not assumed)"),
    },
    MediaPath {
        id: "colour.hdr.mastering",
        name: "HDR mastering metadata",
        area: "Colour",
        state: PathState::Missing,
        detail: "Mastering display and content light level are not read from any container, so \
                 no HDR master's limits are known to this build",
        evidence: None,
    },
    MediaPath {
        id: "colour.hdr.view",
        name: "HDR viewing transforms",
        area: "Colour",
        state: PathState::Partial,
        detail: "Scene-linear pictures go through Rec.2100 PQ and HLG views exactly, and the \
                 result differs from the SDR view as it must (measured: scene 4.0 → 0.6398 in PQ \
                 at 1000 nits, 0.8999 in sRGB). What no surface here can do is *present* HDR: the \
                 window and the browser are SDR, and a PQ picture shown on them is labelled as \
                 code values, not as an HDR presentation. No HDR media can be decoded either, and \
                 no mastering metadata is read or written",
        evidence: Some("cine-color/tests/scene_linear_view.rs::an_hdr_display_view_is_a_different_transform_from_the_sdr_one"),
    },
    // ---------------------------------------------------------------- sound
    MediaPath {
        id: "sound.pcm",
        name: "PCM sound, every layout",
        area: "Sound",
        state: PathState::Verified,
        detail: "8, 16, 24, 32-bit integer and 32/64-bit float, signed and unsigned, scaled by \
                 2^(bits-1) so full-scale negative is exactly -1",
        evidence: Some("cine-media/tests/sound_formats.rs"),
    },
    MediaPath {
        id: "sound.multichannel",
        name: "More than two channels",
        area: "Sound",
        state: PathState::Refused,
        detail: "There is no downmix: the mixer refuses a file with more than two channels and \
                 a proxy of one carries no sound, rather than inventing a fold-down",
        evidence: Some("cine-runtime/tests/proxy_protocol.rs::a_proxy_of_many_channels_carries_no_sound_and_the_sound_stays_on_the_original"),
    },
    MediaPath {
        id: "sound.output",
        name: "Sound output device",
        area: "Sound",
        state: PathState::Implemented,
        detail: "ALSA through a runtime-loaded libasound in the native window; measured \
                 consuming 48 kHz within 61 ppm and pacing picture within 0.3 ms, on the \
                 machine and media of that measurement",
        evidence: Some("docs/blueprint/AV-SYNC.md"),
    },
    // ------------------------------------------------------------- derived
    MediaPath {
        id: "derived.proxy.generate",
        name: "Proxy generation",
        area: "Derived media",
        state: PathState::Verified,
        detail: "Picture for picture and sample for sample from the original, registered as \
                 derived, cancellable, with current/stale/missing recomputed on request",
        evidence: Some("cine-runtime/tests/proxy_protocol.rs"),
    },
    MediaPath {
        id: "derived.proxy.playback",
        name: "Playback from proxies",
        area: "Derived media",
        state: PathState::Verified,
        detail: "One choice for every surface (Originals, Proxies, Auto); a stale or missing \
                 proxy is never read and the reason is shown; renders always read originals",
        evidence: Some("cine-runtime/tests/proxy_protocol.rs::playback_reads_a_proxy_only_when_asked_and_never_a_stale_one"),
    },
    MediaPath {
        id: "derived.thumbnail",
        name: "Thumbnails",
        area: "Derived media",
        state: PathState::Implemented,
        detail: "A cancellable job decodes one picture and writes it to the project's cache; its \
                 protocol test still needs media this machine does not have, so it is not proven \
                 here end to end",
        evidence: None,
    },
    MediaPath {
        id: "derived.waveform",
        name: "Waveform overviews",
        area: "Derived media",
        state: PathState::Verified,
        detail: "A cancellable job decodes the recording and keeps its peaks at four zoom levels \
                 (256 to 131072 samples a bucket), minimum and maximum per channel so asymmetry \
                 and one-sided clipping stay visible. The timeline draws a clip's own media \
                 range from them. It belongs to its original, goes stale when the content or the \
                 recipe changes, and survives a relink, which moves media without changing it. \
                 Measured: 43–78x real time, 6.9 kB of peaks for 4 s of 48 kHz stereo",
        evidence: Some("cine-runtime/tests/waveform_protocol.rs, cine-media/src/waveform.rs tests"),
    },
    MediaPath {
        id: "derived.render.input",
        name: "Render reads media",
        area: "Derived media",
        state: PathState::Verified,
        detail: "Renders read camera originals whatever is being watched, and refuse rather than \
                 substitute when a source cannot be read faithfully",
        evidence: Some("cine-runtime/tests/render_movie.rs, proxy_protocol.rs::a_render_reads_the_original_whatever_is_being_watched"),
    },
    // -------------------------------------------------------- identity
    MediaPath {
        id: "identity.ingest",
        name: "Importing media",
        area: "Identity",
        state: PathState::Verified,
        detail: "A job discovers one file or the run it belongs to, identifies it by content, \
                 probes it, evaluates it against this build's decoders, registers one asset in \
                 one transaction, and dispatches the derived work asked for. Cancellable before \
                 registration; nothing half-registered after a failure; re-importing the same \
                 file is the same asset",
        evidence: Some(
            "cine-runtime/tests/ingest_protocol.rs, ingest_proxy_workflow.rs::\
             an_import_that_asks_for_a_proxy_ends_with_that_proxy_being_watched",
        ),
    },
    MediaPath {
        id: "safety.picture_budget",
        name: "Refusing a picture too large to hold",
        area: "Large media",
        state: PathState::Verified,
        detail: "A picture's size is read from the file's header and checked against what this \
                 machine can hold before anything is allocated; one that does not fit is refused \
                 by name, with the real size, the real budget and how to raise it. \
                 CINE_PICTURE_BUDGET overrides the default of half of reported available memory. \
                 What is not claimed: nothing here bounds a decoder's own working memory, only \
                 the picture it is asked to produce",
        evidence: Some("cine-media/tests/large_media.rs"),
    },
    MediaPath {
        id: "identity.content_hash",
        name: "Content identity",
        area: "Identity",
        state: PathState::Verified,
        detail: "A file's identity is its content hash, not its path: the same file at another \
                 path is the same asset, and a different file at the same path is not",
        evidence: Some("cine-media/src/identity.rs::identity_is_content_derived_not_path_derived"),
    },
    MediaPath {
        id: "identity.presence",
        name: "Online and offline",
        area: "Identity",
        state: PathState::Verified,
        detail: "Checked on request; offline is a normal state that keeps the project editable, \
                 and Auto plays a proxy when the original is away",
        evidence: Some("cine-runtime/tests/proxy_protocol.rs::auto_reads_the_proxy_only_when_the_original_cannot_be_played"),
    },
    MediaPath {
        id: "identity.relink",
        name: "Relinking moved or replaced media",
        area: "Identity",
        state: PathState::Partial,
        detail: "A scan matches files by content (in full against a verified checksum), relinks \
                 what it finds while the asset keeps its identity, refuses a namesake with other \
                 content by name and hash, and keeps the reason on anything unresolved. Image \
                 sequences are not searched for: a run is found by ingesting it again",
        evidence: Some("cine-runtime/tests/relink_protocol.rs, ui/e2e/relink.mjs"),
    },
    MediaPath {
        id: "identity.metadata.camera",
        name: "Camera and lens metadata",
        area: "Identity",
        state: PathState::Missing,
        detail: "No camera, lens or shooting metadata is extracted from any container",
        evidence: None,
    },
    // ------------------------------------------------------------- scale
    MediaPath {
        id: "scale.bounded_decode",
        name: "Bounded decoding",
        area: "Scale",
        state: PathState::Partial,
        detail: "Playback decodes ahead on a worker thread within a byte budget checked before \
                 decoding; stills and EXR are read whole, and there is no tiled or partial \
                 decode",
        evidence: Some("cine-media/tests/decode_ahead.rs"),
    },
];

/// Look up one media path.
pub fn path(id: &str) -> Option<&'static MediaPath> {
    MEDIA_PATHS.iter().find(|p| p.id == id)
}

/// Look up the best matching capability for a codec token.
///
/// `profile` is matched when both sides state one; a caller that does not know
/// the profile gets the first entry for the codec, which is the most-verified
/// one because the table is ordered that way.
pub fn lookup(codec: &str, profile: Option<&str>) -> Option<&'static Capability> {
    match profile {
        Some(p) => REGISTRY
            .iter()
            .find(|c| c.codec == codec && c.profile == Some(p))
            .or_else(|| REGISTRY.iter().find(|c| c.codec == codec)),
        None => REGISTRY.iter().find(|c| c.codec == codec),
    }
}

/// Whether decode should be attempted for this codec and profile.
pub fn is_offered(codec: &str, profile: Option<&str>) -> bool {
    lookup(codec, profile)
        .map(|c| c.verification.is_offered())
        .unwrap_or(false)
}

/// Whether this build has earned the right to claim this capability.
pub fn is_claimable(codec: &str, profile: Option<&str>) -> bool {
    lookup(codec, profile)
        .map(|c| c.verification.is_claimable())
        .unwrap_or(false)
}

/// Why a codec was refused, if it was.
pub fn refusal_reason(codec: &str, profile: Option<&str>) -> Option<&'static str> {
    match lookup(codec, profile)?.verification {
        Verification::Refused { reason } => Some(reason),
        _ => None,
    }
}

/// Everything actually offered, as a human-readable list for error messages.
///
/// Built from the registry rather than written by hand, so it cannot drift out
/// of agreement with what the code does.
pub fn offered_summary() -> String {
    let mut video = Vec::new();
    let mut other = Vec::new();
    for c in REGISTRY.iter().filter(|c| c.verification.is_offered()) {
        let entry = match c.profile {
            Some(p) => format!("{} {}", c.codec, p),
            None => c.codec.to_string(),
        };
        if c.chroma == Chroma::Yuv420 || c.chroma == Chroma::Rgb {
            if !video.contains(&entry) {
                video.push(entry);
            }
        } else if !other.contains(&entry) {
            other.push(entry);
        }
    }
    format!(
        "{} (picture); {} (audio)",
        video.join(", "),
        other.join(", ")
    )
}

/// Only what may honestly be claimed, for documentation and UI.
pub fn claimable_summary() -> String {
    REGISTRY
        .iter()
        .filter(|c| c.verification.is_claimable())
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mechanism that keeps the table honest: a `Verified` row must point
    /// at the test that proves it, naming a real test function.
    #[test]
    fn every_verified_entry_names_its_evidence() {
        for c in REGISTRY {
            if let Verification::Verified { evidence } = c.verification {
                assert!(
                    evidence.contains("::"),
                    "{} claims VERIFIED but its evidence `{evidence}` does not name a test",
                    c.codec
                );
                assert!(
                    evidence.len() > 20,
                    "{} claims VERIFIED with an uselessly vague evidence string",
                    c.codec
                );
            }
        }
    }

    /// The same mechanism for media paths: a verified path names its test.
    #[test]
    fn every_verified_path_names_its_evidence() {
        for p in MEDIA_PATHS {
            if p.state == PathState::Verified {
                let evidence = p.evidence.unwrap_or("");
                assert!(
                    evidence.contains(".rs") || evidence.contains(".md"),
                    "{} claims verified but its evidence `{evidence}` names no file",
                    p.id
                );
            }
            assert!(
                p.detail.len() > 30,
                "{} says too little about what is true",
                p.id
            );
        }
    }

    /// Ids are how surfaces refer to a path; two rows with one id would make
    /// one of them unreachable.
    #[test]
    fn media_path_ids_are_unique_and_grouped() {
        let mut seen: Vec<&str> = Vec::new();
        for p in MEDIA_PATHS {
            assert!(!seen.contains(&p.id), "duplicate media path id {}", p.id);
            assert!(
                p.id.contains('.')
                    && p.id.chars().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '.'
                        || c == '_'),
                "{} is not a dotted lowercase id",
                p.id
            );
            seen.push(p.id);
        }
    }

    /// A refusal must explain itself, or it is indistinguishable from an
    /// oversight when someone reads it in six months.
    #[test]
    fn every_refusal_gives_a_reason() {
        for c in REGISTRY {
            if let Verification::Refused { reason } = c.verification {
                assert!(
                    reason.len() > 20,
                    "{} refused without a real reason",
                    c.codec
                );
            }
        }
    }

    /// The project rule is that hardware decode may not be claimed until it is
    /// verified on real hardware. This test enforces it mechanically.
    #[test]
    fn no_hardware_capability_is_claimed_without_hardware_verification() {
        for c in REGISTRY {
            assert_eq!(
                c.backend,
                BackendKind::Software,
                "{} declares a hardware backend; no hardware decode exists in this build, \
                 and none may be added to the registry until verified on real hardware",
                c.codec
            );
        }
    }

    #[test]
    fn refused_codecs_are_not_offered_and_explain_themselves() {
        assert!(!is_offered("h265", None));
        assert!(!is_offered("prores", None));
        assert!(refusal_reason("h265", None).unwrap().contains("HEVC"));
        // Refused is distinct from unknown: an unknown codec has no reason.
        assert!(refusal_reason("nonexistent", None).is_none());
        assert!(!is_offered("nonexistent", None));
    }

    #[test]
    fn offered_is_broader_than_claimable() {
        // MP3 is wired up but unproven here: attempt it, do not boast of it.
        assert!(is_offered("mp3", None));
        assert!(!is_claimable("mp3", None));
        // H.264 High was actually decoded here.
        assert!(is_offered("h264", Some("High")));
        assert!(is_claimable("h264", Some("High")));
    }

    #[test]
    fn h264_profile_lookup_distinguishes_verified_from_refused() {
        assert!(is_offered("h264", Some("High")));
        assert!(!is_offered("h264", Some("High 4:2:2")));
        assert!(!is_offered("h264", Some("High 4:4:4")));
        // An unknown profile falls back to the codec's best entry rather than
        // failing outright, because the container's profile naming varies.
        assert!(is_offered("h264", Some("Progressive High")));
    }

    #[test]
    fn summaries_are_derived_from_the_table_not_written_by_hand() {
        let offered = offered_summary();
        assert!(
            offered.contains("h264"),
            "offered summary lost video: {offered}"
        );
        assert!(
            offered.contains("aac"),
            "offered summary lost audio: {offered}"
        );
        assert!(
            !offered.contains("h265"),
            "refused codec leaked into the offered list"
        );
        assert!(
            !offered.contains("prores"),
            "refused codec leaked into the offered list"
        );

        let claimable = claimable_summary();
        assert!(claimable.contains("VERIFIED"));
        assert!(
            !claimable.contains("mp3"),
            "unverified codec leaked into claims"
        );
    }
}
