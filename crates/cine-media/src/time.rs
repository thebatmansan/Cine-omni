//! Timecode, over the project's one time model.
//!
//! ## There is no second time type here
//!
//! This module used to define `Timestamp`, `TimeBase` and `FrameRate` — a
//! complete parallel implementation of exact rational time, with its own
//! arithmetic, its own comparison and its own drop-frame conversion. It existed
//! because `ucg_core::RationalTime` was float-based and unusable for media.
//! `RationalTime` is exact now, so the parallel model was two implementations
//! of one idea, free to drift apart in behaviour.
//!
//! It is gone. Media time **is** [`RationalTime`] at an exact [`Rate`], the
//! same values the graph and editorial use, so a frame position crosses between
//! layers without conversion.
//!
//! Note the representation flip that migration required: the old `TimeBase` was
//! a *period* (`1/48000` meant 48 kHz) while [`Rate`] is a *rate* (`48000/1`).
//! Aliasing one to the other would have inverted every base in the codebase
//! silently, so each construction site was moved by hand.
//!
//! What remains is [`Timecode`], which is a genuinely different thing: a parsed
//! SMPTE *label* with hours, minutes, seconds and frames, and rules about which
//! labels are legal. `RationalTime` renders and parses timecode strings;
//! `Timecode` is the structured form, and validates.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

pub use ucg_core::{Rate, RationalTime, Rounding};

/// A media position: an integer tick count at an exact rate.
///
/// An alias, not a new type — this is the project's one time model under the
/// name the media layer reads naturally.
pub type Timestamp = RationalTime;

/// SMPTE timecode.
///
/// Drop-frame is a *labelling* scheme, not a change to the media: at 29.97 the
/// clock would otherwise drift about 3.6 seconds per hour against wall time, so
/// two frame *numbers* are skipped at the start of every minute except every
/// tenth minute. No frames are removed. Getting this wrong is one of the
/// classic sources of a one-frame conform error.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
    pub rate: Rate,
}

impl Timecode {
    /// Convert a frame count since 00:00:00:00 into timecode.
    pub fn from_frame_number(mut n: i64, rate: Rate, drop_frame: bool) -> Result<Timecode> {
        if drop_frame && !rate.supports_drop_frame() {
            return Err(Error::DropFrameUnsupported(rate));
        }
        let tc_rate = i64::from(rate.timecode_rate());
        let day = tc_rate * 60 * 60 * 24;

        if drop_frame {
            // Frames dropped per ten-minute block: 9 minutes drop `d` each.
            let d = if tc_rate == 30 { 2 } else { 4 }; // 29.97 -> 2, 59.94 -> 4
            let per_10min = tc_rate * 60 * 10 - 9 * d;
            let per_min = tc_rate * 60 - d;
            let day_df = per_10min * 6 * 24;
            n = n.rem_euclid(day_df);

            let tens = n / per_10min;
            let mut rem = n % per_10min;
            // The first minute of each ten-minute block drops nothing.
            let mins_in_block = if rem < tc_rate * 60 {
                0
            } else {
                rem -= tc_rate * 60;
                1 + rem / per_min
            };
            // Add the skipped labels back before decomposing. Decomposing the
            // raw frame count instead produces labels that do not exist: frame
            // 1800 is the first frame of minute one, whose `;00` and `;01`
            // labels are precisely the ones drop-frame skips, so it is `;02`.
            // The earlier version answered `;00` — a label its own
            // `to_frame_number` correctly rejects as invalid.
            let labelled = n + d * (9 * tens + mins_in_block);
            let frames = labelled % tc_rate;
            let total_seconds = labelled / tc_rate;
            return Ok(Timecode {
                hours: ((total_seconds / 3600) % 24) as u8,
                minutes: ((total_seconds / 60) % 60) as u8,
                seconds: (total_seconds % 60) as u8,
                frames: frames as u8,
                drop_frame,
                rate,
            });
        }

        n = n.rem_euclid(day);
        let frames = n % tc_rate;
        let total_seconds = n / tc_rate;
        Ok(Timecode {
            hours: ((total_seconds / 3600) % 24) as u8,
            minutes: ((total_seconds / 60) % 60) as u8,
            seconds: (total_seconds % 60) as u8,
            frames: frames as u8,
            drop_frame,
            rate,
        })
    }

    /// Frame count since 00:00:00:00. Inverse of [`Timecode::from_frame_number`].
    pub fn to_frame_number(&self) -> Result<i64> {
        let tc_rate = i64::from(self.rate.timecode_rate());
        if i64::from(self.frames) >= tc_rate {
            return Err(Error::InvalidTimecode(self.to_string()));
        }
        let total_minutes = i64::from(self.hours) * 60 + i64::from(self.minutes);
        if self.drop_frame {
            if !self.rate.supports_drop_frame() {
                return Err(Error::DropFrameUnsupported(self.rate));
            }
            let d = if tc_rate == 30 { 2 } else { 4 };
            // A dropped label must never appear in valid drop-frame timecode.
            if self.seconds == 0 && !self.minutes.is_multiple_of(10) && i64::from(self.frames) < d {
                return Err(Error::InvalidTimecode(self.to_string()));
            }
            let dropped = d * (total_minutes - total_minutes / 10);
            Ok(
                (total_minutes * 60 + i64::from(self.seconds)) * tc_rate + i64::from(self.frames)
                    - dropped,
            )
        } else {
            Ok((total_minutes * 60 + i64::from(self.seconds)) * tc_rate + i64::from(self.frames))
        }
    }

    /// Parse `HH:MM:SS:FF` (non-drop) or `HH:MM:SS;FF` (drop-frame).
    pub fn parse(s: &str, rate: Rate) -> Result<Timecode> {
        let drop_frame = s.contains(';');
        let parts: Vec<&str> = s.split([':', ';']).collect();
        if parts.len() != 4 {
            return Err(Error::InvalidTimecode(s.to_string()));
        }
        let v: Vec<u8> = parts
            .iter()
            .map(|p| {
                p.trim()
                    .parse::<u8>()
                    .map_err(|_| Error::InvalidTimecode(s.to_string()))
            })
            .collect::<Result<_>>()?;
        let tc = Timecode {
            hours: v[0],
            minutes: v[1],
            seconds: v[2],
            frames: v[3],
            drop_frame,
            rate,
        };
        // Validate by round-tripping through the frame number.
        tc.to_frame_number()?;
        if tc.minutes > 59 || tc.seconds > 59 || tc.hours > 23 {
            return Err(Error::InvalidTimecode(s.to_string()));
        }
        Ok(tc)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

impl fmt::Debug for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self} @{:?}", self.rate)
    }
}

// ---------------------------------------------------------------------------
// Bridging to the UCG's editorial time type
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The structured label and the string form must agree. Two drop-frame
    /// implementations now exist — `Timecode` here, and `RationalTime`'s
    /// renderer in `ucg-core` — and the whole hazard of keeping both is that
    /// they drift. This pins them together across the boundaries where
    /// drop-frame actually goes wrong: the first minute, the tenth, and an
    /// hour.
    #[test]
    fn the_structured_and_rendered_drop_frame_labels_agree() {
        for frame in [0i64, 1, 1798, 1799, 1800, 1801, 17_982, 107_892] {
            let tc = Timecode::from_frame_number(frame, Rate::F29_97, true).expect("label");
            let rendered = RationalTime::exact(frame, Rate::F29_97)
                .to_drop_frame_timecode()
                .expect("29.97 supports drop frame");
            assert_eq!(
                tc.to_string(),
                rendered,
                "the two drop-frame implementations disagree at frame {frame}"
            );
        }
    }

    #[test]
    fn non_drop_labels_agree_too() {
        for frame in [0i64, 23, 24, 86_400] {
            let tc = Timecode::from_frame_number(frame, Rate::F24, false).unwrap();
            assert_eq!(
                tc.to_string(),
                RationalTime::exact(frame, Rate::F24).to_timecode()
            );
        }
    }

    #[test]
    fn a_label_round_trips_to_the_frame_it_came_from() {
        for (rate, drop) in [
            (Rate::F24, false),
            (Rate::F25, false),
            (Rate::F29_97, true),
            (Rate::F29_97, false),
            (Rate::F59_94, true),
        ] {
            for frame in [0i64, 1, 1801, 17_982, 50_000] {
                let tc = Timecode::from_frame_number(frame, rate, drop).expect("label");
                assert_eq!(
                    tc.to_frame_number().expect("frame number"),
                    frame,
                    "{tc} at {rate:?} did not round trip"
                );
            }
        }
    }

    /// Drop-frame skips two labels a minute; those labels are not valid
    /// timecode and must be rejected rather than silently accepted.
    #[test]
    fn a_dropped_label_is_refused() {
        assert!(Timecode::parse("00:01:00;00", Rate::F29_97).is_err());
        assert!(Timecode::parse("00:01:00;01", Rate::F29_97).is_err());
        assert!(Timecode::parse("00:01:00;02", Rate::F29_97).is_ok());
        // The tenth minute drops nothing.
        assert!(Timecode::parse("00:10:00;00", Rate::F29_97).is_ok());
    }

    #[test]
    fn drop_frame_is_refused_where_it_has_no_meaning() {
        assert!(Timecode::from_frame_number(0, Rate::F24, true).is_err());
        assert!(Timecode::from_frame_number(0, Rate::F25, true).is_err());
        assert!(Timecode::from_frame_number(0, Rate::F30, true).is_err());
        assert!(Timecode::from_frame_number(0, Rate::F29_97, true).is_ok());
    }

    #[test]
    fn parsing_accepts_both_separators_and_reports_which_it_saw() {
        let nd = Timecode::parse("01:02:03:04", Rate::F24).unwrap();
        assert!(!nd.drop_frame);
        assert_eq!((nd.hours, nd.minutes, nd.seconds, nd.frames), (1, 2, 3, 4));
        let df = Timecode::parse("01:02:03;04", Rate::F29_97).unwrap();
        assert!(df.drop_frame);
    }

    /// A frame number at or beyond the label rate cannot be a valid label.
    #[test]
    fn an_out_of_range_frame_field_is_refused() {
        assert!(Timecode::parse("00:00:00:24", Rate::F24).is_err());
        assert!(Timecode::parse("00:00:00:23", Rate::F24).is_ok());
    }
}
