//! Exact media time.
//!
//! **Floating-point seconds are never authoritative in this crate.** Every
//! quantity that must round-trip — a presentation timestamp, a frame index, a
//! timecode, an edit point — is held as an integer tick count over an exact
//! rational base.
//!
//! The reason is concrete rather than theoretical. At 23.976 fps the frame
//! duration is exactly `1001/24000` s, which is not representable in binary
//! floating point. Accumulate it and the error grows without bound; a two-hour
//! programme drifts past a frame, and the drift is silent. The same applies to
//! 29.97. Broadcast has lived with `1000/1001` since colour television, and any
//! system that stores seconds as `f64` inherits the problem.
//!
//! ```
//! use cine_media::time::{FrameRate, TimeBase, Timestamp};
//! // 240 frames at 23.976 fps is exactly 1001 * 240 / 24000 seconds.
//! let r = FrameRate::F23_976;
//! assert_eq!(r.as_rational(), (24000, 1001));
//! // Frame 240 in the stream's own 12288 Hz base, with no rounding anywhere.
//! let tb = TimeBase::new(1, 12288).unwrap();
//! let t = Timestamp::new(512 * 240, tb);
//! assert_eq!(t.ticks(), 122_880);
//! ```

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

use crate::error::{Error, Result};

/// Greatest common divisor, for keeping rationals in lowest terms.
const fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

// ---------------------------------------------------------------------------
// TimeBase
// ---------------------------------------------------------------------------

/// The unit one tick represents, in seconds: `num / den`.
///
/// This mirrors how containers actually store time. An MP4 video track might
/// use `1/12288`, its audio track `1/48000`, and an MPEG-TS stream `1/90000`.
/// Keeping the stream's own base avoids converting on ingest, which is where
/// rounding errors are usually introduced.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeBase {
    num: u32,
    den: u32,
}

impl TimeBase {
    /// Audio sample bases are just the sample rate.
    pub const AUDIO_48K: TimeBase = TimeBase {
        num: 1,
        den: 48_000,
    };
    /// The MPEG transport-stream clock.
    pub const MPEG_90K: TimeBase = TimeBase {
        num: 1,
        den: 90_000,
    };
    /// Milliseconds, the usual MP4 movie-header base.
    pub const MILLIS: TimeBase = TimeBase { num: 1, den: 1_000 };

    /// Construct a time base, reduced to lowest terms.
    pub fn new(num: u32, den: u32) -> Result<Self> {
        if num == 0 || den == 0 {
            return Err(Error::InvalidTimeBase { num, den });
        }
        let g = gcd(num as u64, den as u64) as u32;
        Ok(TimeBase {
            num: num / g,
            den: den / g,
        })
    }

    /// A base of `1/rate`, the common case for sample and media clocks.
    pub fn from_rate(rate: u32) -> Result<Self> {
        TimeBase::new(1, rate)
    }

    pub const fn num(&self) -> u32 {
        self.num
    }
    pub const fn den(&self) -> u32 {
        self.den
    }

    /// Seconds per tick, for display only. Never use this for arithmetic that
    /// has to round-trip.
    pub fn as_seconds_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl fmt::Debug for TimeBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

// ---------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------

/// An exact instant: `ticks * base` seconds.
///
/// Comparison and arithmetic between timestamps in different bases is done by
/// cross-multiplying in `i128`, so it is exact and cannot overflow for any
/// realistic media duration.
/// Equality, ordering and hashing are all by **value**, never by
/// representation. One second is one second whether it is stored as 48000
/// ticks of 1/48000 or 90000 ticks of 1/90000.
///
/// This cannot be derived. A derived `PartialEq` compares `(ticks, base)`
/// structurally, which would disagree with the exact `Ord` below: the same
/// instant in two bases would be `Ordering::Equal` and yet `!=`, and would hash
/// differently. That breaks the contract the standard library relies on, so a
/// `BTreeMap` and a `HashMap` keyed on the same instants would disagree about
/// whether they contain it, and `sort_unstable` followed by `dedup` would leave
/// duplicates behind.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Timestamp {
    ticks: i64,
    base: TimeBase,
}

impl Timestamp {
    pub const fn new(ticks: i64, base: TimeBase) -> Self {
        Timestamp { ticks, base }
    }

    pub const fn zero(base: TimeBase) -> Self {
        Timestamp { ticks: 0, base }
    }

    pub const fn ticks(&self) -> i64 {
        self.ticks
    }
    pub const fn base(&self) -> TimeBase {
        self.base
    }
    pub const fn is_zero(&self) -> bool {
        self.ticks == 0
    }

    /// This instant as a count of `base.num * ticks` over `base.den`, widened so
    /// cross-base comparison is exact.
    fn as_fraction(&self) -> (i128, i128) {
        (
            self.ticks as i128 * self.base.num as i128,
            self.base.den as i128,
        )
    }

    /// The instant as a fraction of a second in lowest terms, with the sign
    /// carried on the numerator. This is the canonical form: two timestamps are
    /// the same instant exactly when their reduced fractions are identical.
    fn as_reduced_fraction(&self) -> (i128, i128) {
        let (mut n, mut d) = self.as_fraction();
        // A denominator is always positive here, but normalise anyway so the
        // canonical form cannot depend on how the base was constructed.
        if d < 0 {
            n = -n;
            d = -d;
        }
        if n == 0 {
            // Every representation of zero must reduce to the same pair, or
            // zero at two bases would hash differently.
            return (0, 1);
        }
        let g = gcd128(n.unsigned_abs(), d.unsigned_abs()) as i128;
        (n / g, d / g)
    }

    /// Convert to another base, rounding to nearest and away from zero on ties.
    ///
    /// Rounding is explicit because it is lossy: converting a 48 kHz audio
    /// position into a 24 fps frame base cannot be exact in general.
    pub fn rescale(&self, to: TimeBase) -> Timestamp {
        // ticks * (num/den) / (to.num/to.den) = ticks * num * to.den / (den * to.num)
        let n = self.ticks as i128 * self.base.num as i128 * to.den as i128;
        let d = self.base.den as i128 * to.num as i128;
        let q = if (n < 0) != (d < 0) {
            (n - d / 2) / d
        } else {
            (n + d / 2) / d
        };
        Timestamp {
            ticks: q as i64,
            base: to,
        }
    }

    /// True when this instant is exactly representable in `to`.
    pub fn is_exact_in(&self, to: TimeBase) -> bool {
        let n = self.ticks as i128 * self.base.num as i128 * to.den as i128;
        let d = self.base.den as i128 * to.num as i128;
        d != 0 && n % d == 0
    }

    /// Seconds, for display and for handing to APIs that demand a float.
    /// Never store this back as authoritative state.
    pub fn as_seconds_f64(&self) -> f64 {
        self.ticks as f64 * self.base.num as f64 / self.base.den as f64
    }

    /// The frame index containing this instant at `rate`, flooring, which is
    /// the correct rule for "which frame is on screen at time t".
    pub fn frame_index(&self, rate: FrameRate) -> i64 {
        let (num, den) = rate.as_rational(); // frames = t * num/den
        let n = self.ticks as i128 * self.base.num as i128 * num as i128;
        let d = self.base.den as i128 * den as i128;
        let mut q = n / d;
        if n % d != 0 && (n < 0) != (d < 0) {
            q -= 1; // floor for negatives
        }
        q as i64
    }

    /// The exact instant of frame `index` at `rate`, in that rate's own base.
    pub fn from_frame(index: i64, rate: FrameRate) -> Timestamp {
        let (num, den) = rate.as_rational();
        // One frame is den/num seconds; use base den/num so a frame is 1 tick.
        let base = TimeBase::new(den, num).expect("frame rates are non-zero");
        Timestamp { ticks: index, base }
    }

    pub fn checked_add(&self, other: Timestamp) -> Option<Timestamp> {
        let o = other.rescale(self.base);
        self.ticks.checked_add(o.ticks).map(|t| Timestamp {
            ticks: t,
            base: self.base,
        })
    }

    pub fn checked_sub(&self, other: Timestamp) -> Option<Timestamp> {
        let o = other.rescale(self.base);
        self.ticks.checked_sub(o.ticks).map(|t| Timestamp {
            ticks: t,
            base: self.base,
        })
    }
}

/// Greatest common divisor in 128 bits, for reducing a fraction to lowest
/// terms. Distinct from the `u64` `gcd` above because `ticks * base.num` can
/// reach 2^95 and would overflow the narrower one.
fn gcd128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

impl PartialEq for Timestamp {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Timestamp {}

impl std::hash::Hash for Timestamp {
    /// Hash the value in lowest terms, so that two representations of the same
    /// instant land in the same bucket. Hashing `(ticks, base)` would put
    /// 1 s @ 48 kHz and 1 s @ 90 kHz in different buckets while `==` said they
    /// were the same key.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let (n, d) = self.as_reduced_fraction();
        n.hash(state);
        d.hash(state);
    }
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        let (an, ad) = self.as_fraction();
        let (bn, bd) = other.as_fraction();
        (an * bd).cmp(&(bn * ad))
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{:?}", self.ticks, self.base)
    }
}

// ---------------------------------------------------------------------------
// FrameRate
// ---------------------------------------------------------------------------

/// An exact frame rate, held as a rational `num/den` frames per second.
///
/// The `1000/1001` rates are the ones that matter: they exist because NTSC
/// colour needed to shift the field rate slightly, and every professional tool
/// has carried them since.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum FrameRate {
    /// 24000/1001 ≈ 23.976
    F23_976,
    F24,
    F25,
    /// 30000/1001 ≈ 29.97
    F29_97,
    F30,
    F50,
    /// 60000/1001 ≈ 59.94
    F59_94,
    F60,
    /// Anything else, as an exact rational.
    Other {
        num: u32,
        den: u32,
    },
}

impl FrameRate {
    /// Exact `(numerator, denominator)` in frames per second.
    pub const fn as_rational(&self) -> (u32, u32) {
        match self {
            FrameRate::F23_976 => (24_000, 1_001),
            FrameRate::F24 => (24, 1),
            FrameRate::F25 => (25, 1),
            FrameRate::F29_97 => (30_000, 1_001),
            FrameRate::F30 => (30, 1),
            FrameRate::F50 => (50, 1),
            FrameRate::F59_94 => (60_000, 1_001),
            FrameRate::F60 => (60, 1),
            FrameRate::Other { num, den } => (*num, *den),
        }
    }

    pub fn new(num: u32, den: u32) -> Result<Self> {
        if num == 0 || den == 0 {
            return Err(Error::InvalidFrameRate { num, den });
        }
        // Reduce first, so 48000/2002 is recognised as 23.976 rather than
        // falling through to `Other`.
        let g = gcd(num as u64, den as u64) as u32;
        let (num, den) = (num / g, den / g);
        Ok(match (num, den) {
            (24_000, 1_001) => FrameRate::F23_976,
            (24, 1) => FrameRate::F24,
            (25, 1) => FrameRate::F25,
            (30_000, 1_001) => FrameRate::F29_97,
            (30, 1) => FrameRate::F30,
            (50, 1) => FrameRate::F50,
            (60_000, 1_001) => FrameRate::F59_94,
            (60, 1) => FrameRate::F60,
            _ => FrameRate::Other { num, den },
        })
    }

    /// Nominal rate for display, e.g. 23.976023976…
    pub fn as_f64(&self) -> f64 {
        let (n, d) = self.as_rational();
        n as f64 / d as f64
    }

    /// The integer rate a timecode counter runs at: 23.976 → 24, 29.97 → 30.
    /// Timecode counts whole frames, so this is a ceiling of the nominal rate.
    pub fn timecode_rate(&self) -> u32 {
        let (n, d) = self.as_rational();
        n.div_ceil(d)
    }

    /// Whether SMPTE drop-frame timecode is defined for this rate. Drop-frame
    /// exists only for the `1000/1001` NTSC family.
    pub fn supports_drop_frame(&self) -> bool {
        matches!(self, FrameRate::F29_97 | FrameRate::F59_94)
    }

    /// Duration of a single frame, exactly.
    pub fn frame_duration(&self) -> Timestamp {
        let (num, den) = self.as_rational();
        Timestamp::new(1, TimeBase::new(den, num).expect("non-zero"))
    }
}

// ---------------------------------------------------------------------------
// Timecode
// ---------------------------------------------------------------------------

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
    pub rate: FrameRate,
}

impl Timecode {
    /// Convert a frame count since 00:00:00:00 into timecode.
    pub fn from_frame_number(mut n: i64, rate: FrameRate, drop_frame: bool) -> Result<Timecode> {
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
                let m = rem / per_min + 1;
                rem %= per_min;
                m
            };
            let total_minutes = tens * 10 + mins_in_block;
            let frames = rem % tc_rate;
            let seconds = rem / tc_rate;
            return Ok(Timecode {
                hours: ((total_minutes / 60) % 24) as u8,
                minutes: (total_minutes % 60) as u8,
                seconds: seconds as u8,
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
    pub fn parse(s: &str, rate: FrameRate) -> Result<Timecode> {
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

/// Convert to `ucg-core`'s OTIO-shaped `RationalTime` for the editorial layer.
///
/// This is deliberately one-directional at the boundary: media time is exact
/// integers, editorial time is OTIO's `(value, rate)` pair. Going media →
/// editorial is lossless when the timestamp is exact in the target rate, which
/// [`Timestamp::is_exact_in`] can check before converting.
pub fn to_ucg_time(ts: Timestamp, rate: FrameRate) -> ucg_core::RationalTime {
    let (num, den) = rate.as_rational();
    let base = TimeBase::new(den, num).expect("non-zero");
    let frames = ts.rescale(base).ticks();
    ucg_core::RationalTime::new(frames as f64, rate.as_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_rates_are_exact_rationals() {
        assert_eq!(FrameRate::F23_976.as_rational(), (24_000, 1_001));
        assert_eq!(FrameRate::F29_97.as_rational(), (30_000, 1_001));
        assert_eq!(FrameRate::F59_94.as_rational(), (60_000, 1_001));
        assert_eq!(FrameRate::new(24_000, 1_001).unwrap(), FrameRate::F23_976);
        // Reduced to lowest terms.
        assert_eq!(FrameRate::new(48_000, 2_002).unwrap(), FrameRate::F23_976);
    }

    #[test]
    fn timecode_rate_is_the_ceiling_not_the_nominal_rate() {
        assert_eq!(FrameRate::F23_976.timecode_rate(), 24);
        assert_eq!(FrameRate::F29_97.timecode_rate(), 30);
        assert_eq!(FrameRate::F59_94.timecode_rate(), 60);
        assert_eq!(FrameRate::F25.timecode_rate(), 25);
    }

    #[test]
    fn cross_base_comparison_is_exact() {
        let a = Timestamp::new(48_000, TimeBase::AUDIO_48K); // exactly 1 s
        let b = Timestamp::new(90_000, TimeBase::MPEG_90K); // exactly 1 s
        assert_eq!(a.cmp(&b), Ordering::Equal);
        let c = Timestamp::new(48_001, TimeBase::AUDIO_48K);
        assert!(c > b);
    }

    #[test]
    fn rescale_rounds_to_nearest_and_reports_exactness() {
        let one_second = Timestamp::new(48_000, TimeBase::AUDIO_48K);
        assert!(one_second.is_exact_in(TimeBase::MILLIS));
        assert_eq!(one_second.rescale(TimeBase::MILLIS).ticks(), 1_000);

        // One audio sample is not representable in milliseconds.
        let one_sample = Timestamp::new(1, TimeBase::AUDIO_48K);
        assert!(!one_sample.is_exact_in(TimeBase::MILLIS));
        assert_eq!(one_sample.rescale(TimeBase::MILLIS).ticks(), 0);
    }
}

#[cfg(test)]
mod value_identity_tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    /// The same instant expressed in two bases must be equal, order equal, and
    /// hash equal. Deriving any one of the three breaks the other two.
    #[test]
    fn one_second_is_one_second_in_any_base() {
        let a = Timestamp::new(48_000, TimeBase::from_rate(48_000).unwrap());
        let b = Timestamp::new(90_000, TimeBase::from_rate(90_000).unwrap());
        let c = Timestamp::new(24, TimeBase::from_rate(24).unwrap());

        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        assert_eq!(a, b, "equality must agree with ordering");
        assert_eq!(a, c);

        let hash = |t: &Timestamp| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            t.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&a), hash(&b), "equal values must hash equally");
        assert_eq!(hash(&a), hash(&c));
    }

    /// The defect in practice: a set keyed on instants must not hold the same
    /// instant twice merely because it arrived in a different base.
    #[test]
    fn a_hash_set_and_a_btree_set_agree_about_membership() {
        let a = Timestamp::new(48_000, TimeBase::from_rate(48_000).unwrap());
        let b = Timestamp::new(90_000, TimeBase::from_rate(90_000).unwrap());

        let hash_set: HashSet<Timestamp> = [a, b].into_iter().collect();
        let btree_set: BTreeSet<Timestamp> = [a, b].into_iter().collect();
        assert_eq!(hash_set.len(), 1, "the same instant twice is one entry");
        assert_eq!(
            hash_set.len(),
            btree_set.len(),
            "a HashMap and a BTreeMap must not disagree about the same key"
        );
        assert!(hash_set.contains(&b) && btree_set.contains(&b));
    }

    #[test]
    fn zero_is_the_same_instant_at_every_base() {
        let bases = [48_000u32, 90_000, 24, 12_288];
        let zeros: HashSet<Timestamp> = bases
            .iter()
            .map(|r| Timestamp::new(0, TimeBase::from_rate(*r).unwrap()))
            .collect();
        assert_eq!(zeros.len(), 1, "zero must not depend on its base");
    }

    #[test]
    fn different_instants_remain_different() {
        let a = Timestamp::new(48_000, TimeBase::from_rate(48_000).unwrap());
        let b = Timestamp::new(48_001, TimeBase::from_rate(48_000).unwrap());
        assert_ne!(a, b);
        assert!(a < b);
        let set: HashSet<Timestamp> = [a, b].into_iter().collect();
        assert_eq!(
            set.len(),
            2,
            "value equality must not collapse distinct instants"
        );
    }

    /// Sorting then de-duplicating must actually de-duplicate.
    #[test]
    fn sort_and_dedup_removes_duplicate_instants_across_bases() {
        let mut v = vec![
            Timestamp::new(90_000, TimeBase::from_rate(90_000).unwrap()),
            Timestamp::new(48_000, TimeBase::from_rate(48_000).unwrap()),
            Timestamp::new(0, TimeBase::from_rate(24).unwrap()),
        ];
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), 2, "one second appeared twice and should collapse");
    }

    /// Negative time is legitimate (a handle before a clip's start), and must
    /// obey the same rules.
    #[test]
    fn negative_instants_compare_and_hash_by_value() {
        let a = Timestamp::new(-48_000, TimeBase::from_rate(48_000).unwrap());
        let b = Timestamp::new(-90_000, TimeBase::from_rate(90_000).unwrap());
        assert_eq!(a, b);
        assert!(a < Timestamp::new(0, TimeBase::from_rate(24).unwrap()));
        let set: HashSet<Timestamp> = [a, b].into_iter().collect();
        assert_eq!(set.len(), 1);
    }

    /// NTSC bases are the ones that matter most and are the least forgiving.
    #[test]
    fn ntsc_frame_and_audio_bases_agree_on_a_shared_instant() {
        // One second at 24000/1001 fps is 24000/1001 frames — not an integer,
        // so use two seconds' worth of a common instant instead: 1001 frames
        // at 24000/1001 is exactly 1001*1001/24000 s.
        let frames = Timestamp::new(24_000, TimeBase::new(1001, 24_000).unwrap());
        let seconds = Timestamp::new(1001, TimeBase::from_rate(1).unwrap());
        assert_eq!(
            frames, seconds,
            "24000 frames at 24000/1001 fps is 1001 seconds"
        );
    }
}
