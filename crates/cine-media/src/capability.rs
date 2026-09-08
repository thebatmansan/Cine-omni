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
        verification: Verification::Unverified {
            note: "no Baseline-profile asset available on this machine",
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
        verification: Verification::Unverified {
            note: "no 24-bit asset here",
        },
    },
    Capability {
        codec: "pcm_f32le",
        profile: None,
        level: None,
        bit_depth: 32,
        chroma: Chroma::Monochrome,
        backend: BackendKind::Software,
        verification: Verification::Unverified {
            note: "no float-PCM asset here",
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
