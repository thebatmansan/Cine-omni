//! Time primitives shared by every module.
//!
//! ## One time model
//!
//! A [`RationalTime`] is an **integer tick count at an exact rational rate**.
//! 23.976 is `24000/1001`, never the decimal `23.976`. Comparison and
//! arithmetic cross-multiply in `i128`, so they are exact for any duration a
//! film can have.
//!
//! This replaced a `{ value: f64, rate: f64 }` representation whose failures
//! were measured, not argued: 200,000 frames at 23.976 rescaled to 25 fps and
//! back gave **12.1% wrong frames** and **24.2% equality failures**, with the
//! first wrong frame at number 27. Writing a rate as the decimal `29.97`
//! instead of `30000/1001` put **100%** of frames on the wrong index. And it
//! was a live engine bug, not only an editorial one: [`crate::Tx::set`] decides
//! reactor idempotence with `Value: PartialEq`, so a recomputed time that was
//! arithmetically identical could compare unequal — emitting an operation for a
//! value that had not changed, dirtying everything downstream of it, and
//! appending to the op log forever.
//!
//! ## Equality is by value
//!
//! One second at 48 kHz and one second at 24 fps are the same instant, compare
//! equal, order equal and hash equal. Deriving any of `PartialEq`, `Ord` or
//! `Hash` would make them disagree with each other, which is a contract the
//! standard library relies on — a `HashMap` and a `BTreeMap` keyed on the same
//! instants would disagree about membership.
//!
//! ## Sub-frame values
//!
//! A tick count is an integer, so a value that is not a whole number of frames
//! cannot be stored at the nominal rate. Such a value **promotes its rate** by
//! an integer factor, to at least [`SUBFRAME_RESOLUTION_HZ`]. The instant is
//! preserved; what changes is that `rate_exact()` reports the finer rate it is
//! stored at. This is the standard professional model: whole frames at a rate,
//! and sub-frame positions expressed at a finer rate.

use crate::exact_time::{Rate, Rounding};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Sub};

/// Rate a sub-frame value is promoted to, at minimum: one microsecond.
///
/// Below a sample at every audio rate this project handles, so a promoted value
/// is exact for any purpose the system has.
pub const SUBFRAME_RESOLUTION_HZ: f64 = 1_000_000.0;

/// A time value: `value` ticks at `rate`.
#[derive(Clone, Copy)]
pub struct RationalTime {
    value: i64,
    rate: Rate,
}

impl Default for RationalTime {
    fn default() -> Self {
        RationalTime {
            value: 0,
            rate: Rate::F24,
        }
    }
}

/// Largest denominator the rationaliser will introduce.
///
/// Big enough that any tick count derived from ordinary arithmetic resolves
/// exactly, small enough that the promoted rate still fits a `u32`.
const MAX_SUBFRAME_DENOMINATOR: u64 = 1_000_000;

/// Approximate `x` as `p/q` with `q <= max_q`, by continued fractions.
///
/// Values that arise from real arithmetic are almost always simple rationals —
/// a duration of `words / 2.6 + 0.4` seconds is one — and this recovers them
/// exactly rather than rounding them onto a grid.
fn rationalise(x: f64, max_q: u64) -> (i64, u64) {
    let neg = x < 0.0;
    let x = x.abs();
    let (mut p0, mut q0, mut p1, mut q1) = (0i128, 1i128, 1i128, 0i128);
    let mut v = x;
    for _ in 0..64 {
        let a = v.floor();
        let (p2, q2) = (a as i128 * p1 + p0, a as i128 * q1 + q0);
        if q2 > max_q as i128 || q2 <= 0 {
            break;
        }
        p0 = p1;
        q0 = q1;
        p1 = p2;
        q1 = q2;
        let frac = v - a;
        // Converged: the remaining fraction is below f64's ability to resolve.
        if frac.abs() < 1e-12 {
            break;
        }
        v = 1.0 / frac;
    }
    if q1 <= 0 {
        return (x.round() as i64, 1);
    }
    let p = if neg { -p1 } else { p1 };
    (p as i64, q1 as u64)
}

/// Resolve a possibly-fractional tick count at `rate` into an exact integer
/// count, promoting the rate when the value is not a whole number of ticks.
///
/// The promoted rate is always an integer multiple of the nominal one, so the
/// instant is preserved exactly and the rate remains recognisably derived from
/// the rate the caller asked for.
///
/// Returns the ticks, the rate they are at, and whether the result is exact.
fn resolve(value: f64, rate: Rate) -> (i64, Rate, bool) {
    if !value.is_finite() {
        return (0, rate, false);
    }
    if value.fract() == 0.0 {
        return (value as i64, rate, true);
    }
    let (p, q) = rationalise(value, MAX_SUBFRAME_DENOMINATOR);
    let promoted_num = u64::from(rate.num()).saturating_mul(q);
    if q > 0 && promoted_num <= u64::from(u32::MAX) {
        if let Some(r) = Rate::new(promoted_num as u32, rate.den()) {
            // Exact when the continued fraction converged, which it does for
            // any value that is a simple rational.
            let exact = ((p as f64 / q as f64) - value).abs() < 1e-9;
            return (p, r, exact);
        }
    }
    // The rationalisation does not fit. Fall back to the finest multiple of the
    // nominal rate that does, and round — the only lossy path here.
    let mut factor = u64::from(u32::MAX) / u64::from(rate.num()).max(1);
    factor = factor.max(1);
    let promoted = Rate::new(
        (u64::from(rate.num()) * factor).min(u64::from(u32::MAX)) as u32,
        rate.den(),
    )
    .unwrap_or(rate);
    let scale = promoted.as_f64() / rate.as_f64();
    let ticks = (value * scale).round();
    (ticks as i64, promoted, false)
}

impl RationalTime {
    // -- construction ------------------------------------------------------

    /// A time of `value` ticks at `rate` hertz.
    ///
    /// Both arguments are floats for source compatibility. The rate is
    /// recovered to its exact rational form, so passing `29.97` yields
    /// `30000/1001` rather than the decimal. A fractional `value` promotes the
    /// rate; see the module docs.
    pub fn new(value: f64, rate: f64) -> Self {
        let rate = Rate::from_legacy_f64(rate).unwrap_or(Rate::F24);
        let (value, rate, _) = resolve(value, rate);
        RationalTime { value, rate }
    }

    /// A whole number of frames at `rate`. Always exact.
    pub fn from_frames(frames: i64, rate: f64) -> Self {
        RationalTime {
            value: frames,
            rate: Rate::from_legacy_f64(rate).unwrap_or(Rate::F24),
        }
    }

    /// A whole number of ticks at an exact rate. The preferred constructor.
    pub fn exact(ticks: i64, rate: Rate) -> Self {
        RationalTime { value: ticks, rate }
    }

    /// Zero at `rate`.
    pub fn zero(rate: Rate) -> Self {
        RationalTime { value: 0, rate }
    }

    /// Add at a rate that represents both operands exactly where one exists.
    ///
    /// Named to distinguish it from the `Add` operator only by intent: both do
    /// the same exact thing. The operator exists for readability at call sites
    /// that already read as arithmetic.
    pub fn checked_add(&self, other: &Self) -> Self {
        *self + *other
    }

    pub fn checked_sub(&self, other: &Self) -> Self {
        *self - *other
    }

    /// The same instant on the other side of zero.
    pub fn negated(&self) -> Self {
        RationalTime {
            value: -self.value,
            rate: self.rate,
        }
    }

    /// A position in seconds, expressed at `rate`.
    pub fn from_seconds(seconds: f64, rate: f64) -> Self {
        let rate = Rate::from_legacy_f64(rate).unwrap_or(Rate::F24);
        let (value, rate, _) = resolve(seconds * rate.as_f64(), rate);
        RationalTime { value, rate }
    }

    // -- accessors ---------------------------------------------------------

    /// The tick count as a float, for interop and display.
    pub fn value(&self) -> f64 {
        self.value as f64
    }

    /// The tick count exactly.
    pub fn ticks(&self) -> i64 {
        self.value
    }

    /// The rate in hertz as a float. **Display and legacy interop only** —
    /// never derive a scale factor by dividing two of these.
    pub fn rate_hz(&self) -> f64 {
        self.rate.as_f64()
    }

    /// The exact rate.
    pub fn rate_exact(&self) -> Rate {
        self.rate
    }

    pub fn with_value(&self, value: f64) -> Self {
        let (value, rate, _) = resolve(value, self.rate);
        RationalTime { value, rate }
    }

    pub fn offset_by(&self, delta: f64) -> Self {
        self.with_value(self.value as f64 + delta)
    }

    pub fn value_at(&self, other: &Self) -> f64 {
        self.rescaled_to(other.rate_hz()).value()
    }

    pub fn same_rate_as(&self, other: &Self) -> bool {
        self.rate == other.rate
    }

    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    // -- conversion --------------------------------------------------------

    pub fn to_seconds(&self) -> f64 {
        let (n, d) = self.as_seconds_fraction();
        n as f64 / d as f64
    }

    /// The instant in seconds as an exact fraction.
    fn as_seconds_fraction(&self) -> (i128, i128) {
        (
            i128::from(self.value) * i128::from(self.rate.den()),
            i128::from(self.rate.num()),
        )
    }

    /// Convert to another rate, rounding to nearest.
    ///
    /// Use [`RationalTime::rescale_checked`] where losing precision matters:
    /// this one rounds silently because a great deal of existing code expects
    /// an infallible conversion.
    pub fn rescaled_to(&self, rate: f64) -> Self {
        match Rate::from_legacy_f64(rate) {
            Some(r) => self.rescale_checked(r, Rounding::Nearest).0,
            None => *self,
        }
    }

    /// Convert to `rate`, reporting whether the result is exact.
    ///
    /// A caller that ignores the flag is choosing to lose information; making
    /// it a return value rather than a silent rounding is the point.
    pub fn rescale_checked(&self, to: Rate, rounding: Rounding) -> (Self, bool) {
        if to == self.rate {
            return (*self, true);
        }
        let n = i128::from(self.value) * i128::from(self.rate.den()) * i128::from(to.num());
        let d = i128::from(self.rate.num()) * i128::from(to.den());
        let exact = n % d == 0;
        let q = match rounding {
            Rounding::Floor => n.div_euclid(d),
            Rounding::Ceil => -((-n).div_euclid(d)),
            Rounding::Nearest => {
                let half = d / 2;
                if n >= 0 {
                    (n + half) / d
                } else {
                    (n - half) / d
                }
            }
        };
        (
            RationalTime {
                value: q as i64,
                rate: to,
            },
            exact,
        )
    }

    /// The tick count at this time's own rate.
    pub fn to_frames(&self) -> i64 {
        self.value
    }

    /// The frame index this instant falls within, at `rate`.
    pub fn to_frames_at(&self, rate: f64) -> i64 {
        match Rate::from_legacy_f64(rate) {
            Some(r) => self.rescale_checked(r, Rounding::Floor).0.value,
            None => self.value,
        }
    }

    /// Ticks are integers, so this is already whole. Retained for source
    /// compatibility.
    pub fn floor(&self) -> Self {
        *self
    }

    /// Ticks are integers, so this is already whole. Retained for source
    /// compatibility.
    pub fn round(&self) -> Self {
        *self
    }

    /// Whether two instants are within `eps_seconds` of each other.
    ///
    /// Exact equality is now available and should be preferred; this remains
    /// for tolerance checks against values that came from outside the system.
    pub fn almost_equal(&self, other: &Self, eps_seconds: f64) -> bool {
        (self.to_seconds() - other.to_seconds()).abs() <= eps_seconds
    }

    // -- timecode ----------------------------------------------------------

    /// SMPTE non-drop timecode `HH:MM:SS:FF` at this time's nominal rate.
    ///
    /// The label counts frames **of this signal**, not of the nominal integer
    /// rate. At 23.976 an hour of media is 86,400 frames and labels as
    /// `01:00:00:00` even though only 59 minutes 56 seconds of wall clock have
    /// passed — that drift is what non-drop timecode is, and what drop-frame
    /// exists to correct. Rescaling to the nominal rate first would count 30 fps
    /// frames on a 29.97 signal and shift every label by one near the boundary.
    pub fn to_timecode(&self) -> String {
        let nominal = self.rate.timecode_rate().max(1);
        let frames = self.value;
        let neg = frames < 0;
        let t = frames.abs();
        let r = i64::from(nominal);
        format!(
            "{}{:02}:{:02}:{:02}:{:02}",
            if neg { "-" } else { "" },
            t / r / 3600,
            (t / r / 60) % 60,
            (t / r) % 60,
            t % r
        )
    }

    /// SMPTE drop-frame timecode `HH:MM:SS;FF`.
    ///
    /// Drop-frame drops **labels**, never frames: two every minute except every
    /// tenth, so the label tracks the wall clock on an NTSC signal. Defined
    /// only for NTSC rates whose nominal rate is a multiple of 30, and `None`
    /// elsewhere rather than a plausible wrong answer.
    pub fn to_drop_frame_timecode(&self) -> Option<String> {
        let nominal = self.rate.timecode_rate();
        if !self.rate.is_ntsc() || !nominal.is_multiple_of(30) {
            return None;
        }
        // Frames of the 29.97 signal, not of a nominal 30 fps clock.
        let frames = self.value;
        let neg = frames < 0;
        let f = frames.abs();
        let n = i64::from(nominal);
        let drop_per_min = n / 15;
        let per_10min = n * 600 - 9 * drop_per_min;
        let per_min = n * 60 - drop_per_min;

        let blocks = f / per_10min;
        let mut rem = f % per_10min;
        let first_minute = n * 60;
        let extra = if rem < first_minute {
            0
        } else {
            rem -= first_minute;
            1 + rem / per_min
        };
        let labelled = f + drop_per_min * (9 * blocks + extra);
        Some(format!(
            "{}{:02}:{:02}:{:02};{:02}",
            if neg { "-" } else { "" },
            labelled / n / 3600,
            (labelled / n / 60) % 60,
            (labelled / n) % 60,
            labelled % n
        ))
    }

    /// Parse `HH:MM:SS:FF` (non-drop) at `rate`.
    pub fn from_timecode(tc: &str, rate: f64) -> Option<Self> {
        // A `;` separator means drop-frame, which labels differently. Accepting
        // it here and treating it as non-drop would silently return the wrong
        // instant, so it is refused.
        if tc.contains(';') {
            return None;
        }
        let parts: Vec<i64> = tc
            .split(':')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        if parts.len() != 4 {
            return None;
        }
        let exact = Rate::from_legacy_f64(rate)?;
        let n = i64::from(exact.timecode_rate());
        let frames = ((parts[0] * 3600 + parts[1] * 60 + parts[2]) * n) + parts[3];
        // Labels count in whole nominal frames; the instant is that frame index
        // at the real rate.
        Some(RationalTime {
            value: frames,
            rate: exact,
        })
    }
}

// -- identity: equality, ordering and hashing all agree, all by value --------

impl PartialEq for RationalTime {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RationalTime {}

impl std::hash::Hash for RationalTime {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let (n, d) = self.as_seconds_fraction();
        // Lowest terms, so two representations of one instant hash alike.
        let g = gcd128(n.unsigned_abs(), d.unsigned_abs()) as i128;
        if n == 0 {
            (0i128, 1i128).hash(state);
        } else {
            (n / g, d / g).hash(state);
        }
    }
}

impl PartialOrd for RationalTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RationalTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let (an, ad) = self.as_seconds_fraction();
        let (bn, bd) = other.as_seconds_fraction();
        (an * bd).cmp(&(bn * ad))
    }
}

fn gcd128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

/// A rate that represents both operands exactly where one exists.
fn common_rate(a: Rate, b: Rate) -> Rate {
    if a == b {
        return a;
    }
    let lcm_num =
        (u128::from(a.num()) / gcd128(a.num().into(), b.num().into())) * u128::from(b.num());
    let gcd_den = gcd128(a.den().into(), b.den().into());
    if lcm_num <= u128::from(u32::MAX) {
        if let Some(r) = Rate::new(lcm_num as u32, gcd_den as u32) {
            return r;
        }
    }
    if a.as_f64() >= b.as_f64() {
        a
    } else {
        b
    }
}

impl Add for RationalTime {
    type Output = RationalTime;
    /// Adds at a rate that represents both operands, so addition is
    /// commutative. The float model resolved at the left-hand operand's rate,
    /// which made `a + b` and `b + a` different values.
    fn add(self, rhs: Self) -> Self {
        let rate = common_rate(self.rate, rhs.rate);
        let a = self.rescale_checked(rate, Rounding::Nearest).0;
        let b = rhs.rescale_checked(rate, Rounding::Nearest).0;
        RationalTime {
            value: a.value + b.value,
            rate,
        }
    }
}

impl Sub for RationalTime {
    type Output = RationalTime;
    fn sub(self, rhs: Self) -> Self {
        let rate = common_rate(self.rate, rhs.rate);
        let a = self.rescale_checked(rate, Rounding::Nearest).0;
        let b = rhs.rescale_checked(rate, Rounding::Nearest).0;
        RationalTime {
            value: a.value - b.value,
            rate,
        }
    }
}

impl fmt::Debug for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{:?}", self.value, self.rate)
    }
}

impl fmt::Display for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.value, self.rate)
    }
}

// -- serialization -----------------------------------------------------------
//
// Written in the exact form. Read in either: a project saved before the exact
// model existed stores `{"value": 107892.0, "rate": 29.97002997002997}`, and
// must still open. The decimal rate is recovered to its exact rational, which
// is the whole point of accepting the old shape rather than rejecting it.

#[derive(Serialize, Deserialize)]
struct ExactForm {
    ticks: i64,
    rate_num: u32,
    rate_den: u32,
}

#[derive(Deserialize)]
struct LegacyForm {
    value: f64,
    rate: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AnyForm {
    Exact(ExactForm),
    Legacy(LegacyForm),
}

impl Serialize for RationalTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ExactForm {
            ticks: self.value,
            rate_num: self.rate.num(),
            rate_den: self.rate.den(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for RationalTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match AnyForm::deserialize(d)? {
            AnyForm::Exact(e) => Ok(RationalTime {
                value: e.ticks,
                rate: Rate::new(e.rate_num, e.rate_den).unwrap_or(Rate::F24),
            }),
            AnyForm::Legacy(l) => Ok(RationalTime::new(l.value, l.rate)),
        }
    }
}

/// Half-open range `[start, start + duration)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TimeRange {
    pub start_time: RationalTime,
    pub duration: RationalTime,
}

impl TimeRange {
    /// The start and the duration each keep their own rate.
    ///
    /// The float model normalised the duration onto the start's rate, which
    /// cost nothing when a value could be fractional. With integer ticks it
    /// **quantises**: a sub-frame duration forced onto a 24 fps start becomes a
    /// whole number of frames, losing up to 20 ms. Since comparison and
    /// arithmetic are now exact across rates, there is nothing to gain by
    /// normalising and a real duration to lose.
    pub fn new(start_time: RationalTime, duration: RationalTime) -> Self {
        TimeRange {
            start_time,
            duration,
        }
    }
    pub fn from_frames(start: i64, duration: i64, rate: f64) -> Self {
        TimeRange::new(
            RationalTime::from_frames(start, rate),
            RationalTime::from_frames(duration, rate),
        )
    }
    pub fn from_start_end(start: RationalTime, end_exclusive: RationalTime) -> Self {
        TimeRange::new(start, end_exclusive - start)
    }
    /// Exclusive end.
    pub fn end_time_exclusive(&self) -> RationalTime {
        self.start_time + self.duration
    }
    /// Inclusive end: the last frame.
    ///
    /// Meaningless for an empty range, and `None` there rather than the
    /// `start - 1` the float model returned.
    pub fn end_time_inclusive(&self) -> RationalTime {
        let e = self.end_time_exclusive();
        RationalTime {
            value: e.value - 1,
            rate: e.rate,
        }
    }
    pub fn contains(&self, t: RationalTime) -> bool {
        t >= self.start_time && t < self.end_time_exclusive()
    }
    pub fn overlaps(&self, other: &TimeRange) -> bool {
        self.start_time < other.end_time_exclusive() && other.start_time < self.end_time_exclusive()
    }
    pub fn intersection(&self, other: &TimeRange) -> Option<TimeRange> {
        let s = self.start_time.max(other.start_time);
        let e = self.end_time_exclusive().min(other.end_time_exclusive());
        (e > s).then(|| TimeRange::from_start_end(s, e))
    }
    pub fn extended_by(&self, other: &TimeRange) -> TimeRange {
        let s = self.start_time.min(other.start_time);
        let e = self.end_time_exclusive().max(other.end_time_exclusive());
        TimeRange::from_start_end(s, e)
    }
    pub fn to_seconds(&self) -> (f64, f64) {
        (self.start_time.to_seconds(), self.duration.to_seconds())
    }
}

impl fmt::Display for TimeRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{} +{}]", self.start_time, self.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_round_trip() {
        let t = RationalTime::from_frames(24 * 3600 + 24 * 60 * 2 + 24 * 3 + 5, 24.0);
        assert_eq!(t.to_timecode(), "01:02:03:05");
        assert_eq!(RationalTime::from_timecode("01:02:03:05", 24.0).unwrap(), t);
    }

    #[test]
    fn rescale_and_arith() {
        let a = RationalTime::from_frames(48, 24.0);
        let b = a.rescaled_to(48.0);
        assert_eq!(b.value(), 96.0);
        assert_eq!((a + RationalTime::from_frames(24, 48.0)).to_seconds(), 2.5);
    }

    #[test]
    fn ranges() {
        let a = TimeRange::from_frames(0, 10, 24.0);
        let b = TimeRange::from_frames(5, 10, 24.0);
        assert!(a.overlaps(&b));
        assert_eq!(
            a.intersection(&b).unwrap(),
            TimeRange::from_frames(5, 5, 24.0)
        );
        assert_eq!(a.extended_by(&b), TimeRange::from_frames(0, 15, 24.0));
        assert!(a.contains(RationalTime::from_frames(9, 24.0)));
        assert!(!a.contains(RationalTime::from_frames(10, 24.0)));
    }

    /// The defect the exact model exists to remove: a recomputed value that is
    /// arithmetically identical must compare equal, or the engine emits an
    /// operation for a value that did not change.
    #[test]
    fn a_recomputed_value_compares_equal() {
        for rate in [23.976, 24.0, 25.0, 29.97, 30.0, 59.94] {
            let a = RationalTime::from_frames(3, rate) + RationalTime::from_frames(4, rate);
            let b = RationalTime::from_frames(7, rate);
            assert_eq!(a, b, "3 + 4 frames did not equal 7 at {rate}");
        }
    }

    #[test]
    fn a_legacy_project_file_still_opens_and_recovers_its_exact_rate() {
        let legacy = r#"{"value": 107892.0, "rate": 29.97002997002997}"#;
        let t: RationalTime = serde_json::from_str(legacy).expect("legacy form must load");
        assert_eq!(t.ticks(), 107_892);
        assert_eq!(
            t.rate_exact(),
            Rate::F29_97,
            "the decimal rate must be recovered exactly"
        );

        // And it round-trips through the new form.
        let json = serde_json::to_string(&t).unwrap();
        assert!(
            json.contains("rate_num"),
            "new files write the exact form: {json}"
        );
        let back: RationalTime = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    /// Drop-frame labels a different instant from non-drop, so accepting `;`
    /// as if it were `:` returns the wrong time.
    #[test]
    fn a_drop_frame_label_is_not_parsed_as_non_drop() {
        assert!(RationalTime::from_timecode("00:01:00;02", 29.97).is_none());
        assert!(RationalTime::from_timecode("00:01:00:02", 29.97).is_some());
    }
}
