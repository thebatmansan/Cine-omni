//! Exact rational time.
//!
//! ## Why this exists
//!
//! [`crate::time::RationalTime`] stores `{ value: f64, rate: f64 }`. That is
//! wrong in ways that were measured against the real crates, not argued:
//!
//! * 200,000 frames at 24000/1001 rescaled to 25 fps and back gave **24,201
//!   wrong frames (12.1%)** and **48,408 equality failures (24.2%)**. The first
//!   wrong frame was number **27** — this is not an end-of-reel problem.
//! * Writing the rate as the decimal `29.97` instead of `30000/1001` puts
//!   **100%** of frames on the wrong index, from frame 1.
//! * It is not an NTSC-only problem: 25 and 50 fps lose 5.1% across a rescale
//!   round trip, because `to_frames()` floors a value one ULP below the integer.
//! * It is a live correctness bug in the engine, not only in editorial.
//!   `Tx::set` decides idempotence with `Value: PartialEq`, i.e. `f64`
//!   equality, so a recomputed time that is arithmetically identical can
//!   compare unequal — emitting an operation for a value that did not change,
//!   dirtying everything downstream of it, and appending to the op log forever.
//!   Reactor convergence is defined in terms of that equality.
//!
//! ## The model
//!
//! A [`Rate`] is an exact rational: 23.976 is `24000/1001`, never a decimal. An
//! [`ExactTime`] is an integer count of ticks at a rate. Comparison and
//! arithmetic cross-multiply in `i128`, so they are exact for any duration a
//! film can have.
//!
//! Equality, ordering and hashing all agree and are all **by value**: the same
//! instant expressed at two rates is one instant. Deriving any of the three
//! would break the other two, which is a defect this project has already had
//! once and fixed.
//!
//! ## Status
//!
//! This type has no callers yet. It is stage 1 of the migration in
//! `docs/blueprint/EXACT-TIME-MIGRATION.md`: land the exact core and prove it
//! against the adversarial suite before anything depends on it.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// Greatest common divisor.
const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    if a == 0 {
        1
    } else {
        a
    }
}

/// An exact frame or sample rate, in hertz, as a rational `num/den`.
///
/// Always stored in lowest terms with a non-zero denominator, so two rates that
/// mean the same thing are the same value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rate {
    num: u32,
    den: u32,
}

impl Rate {
    // The rates a film actually uses. Every NTSC rate is an exact ratio; the
    // decimals everyone writes are rounded, and rounding them is the defect.
    pub const F23_976: Rate = Rate {
        num: 24_000,
        den: 1001,
    };
    pub const F24: Rate = Rate { num: 24, den: 1 };
    pub const F25: Rate = Rate { num: 25, den: 1 };
    pub const F29_97: Rate = Rate {
        num: 30_000,
        den: 1001,
    };
    pub const F30: Rate = Rate { num: 30, den: 1 };
    pub const F47_952: Rate = Rate {
        num: 48_000,
        den: 1001,
    };
    pub const F48: Rate = Rate { num: 48, den: 1 };
    pub const F50: Rate = Rate { num: 50, den: 1 };
    pub const F59_94: Rate = Rate {
        num: 60_000,
        den: 1001,
    };
    pub const F60: Rate = Rate { num: 60, den: 1 };
    pub const AUDIO_44100: Rate = Rate {
        num: 44_100,
        den: 1,
    };
    pub const AUDIO_48K: Rate = Rate {
        num: 48_000,
        den: 1,
    };
    pub const AUDIO_96K: Rate = Rate {
        num: 96_000,
        den: 1,
    };

    /// An exact rate `num/den` hertz, reduced to lowest terms.
    ///
    /// Returns `None` for a zero numerator or denominator: a rate of zero has
    /// no meaning and would make every conversion a division by zero.
    pub fn new(num: u32, den: u32) -> Option<Rate> {
        if num == 0 || den == 0 {
            return None;
        }
        let g = gcd(num as u128, den as u128) as u32;
        Some(Rate {
            num: num / g,
            den: den / g,
        })
    }

    /// A whole-number rate, e.g. 24 or 48000.
    pub fn whole(hz: u32) -> Option<Rate> {
        Rate::new(hz, 1)
    }

    pub fn num(&self) -> u32 {
        self.num
    }
    pub fn den(&self) -> u32 {
        self.den
    }

    /// The rate as a float. **For display only** — never feed this back into a
    /// constructor, which is exactly how the decimal-rate defect happens.
    pub fn as_f64(&self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// Whether this is an NTSC-family rate, whose denominator is 1001.
    pub fn is_ntsc(&self) -> bool {
        self.den == 1001
    }

    /// The nominal integer rate a timecode label counts in: 30 for 29.97,
    /// 24 for 23.976. SMPTE labels count whole frames per nominal second.
    pub fn timecode_rate(&self) -> u32 {
        // Round to nearest rather than truncate: 24000/1001 is 23.976, whose
        // label rate is 24, and truncation would give 23.
        (self.num + self.den / 2) / self.den
    }

    /// Recover a rate from a float, if it is recognisably one of the standard
    /// ones. This is a *migration* aid for reading old float-based data, not a
    /// constructor for new values.
    pub fn from_legacy_f64(hz: f64) -> Option<Rate> {
        const KNOWN: &[Rate] = &[
            Rate::F23_976,
            Rate::F24,
            Rate::F25,
            Rate::F29_97,
            Rate::F30,
            Rate::F47_952,
            Rate::F48,
            Rate::F50,
            Rate::F59_94,
            Rate::F60,
            Rate::AUDIO_44100,
            Rate::AUDIO_48K,
            Rate::AUDIO_96K,
        ];
        if !hz.is_finite() || hz <= 0.0 {
            return None;
        }
        // A decimal like 29.97 is 0.0003 away from 30000/1001; a genuinely
        // different rate is not. The tolerance is tight enough to distinguish
        // 29.97 from 30 and loose enough to absorb a written-down decimal.
        if let Some(r) = KNOWN.iter().find(|r| (r.as_f64() - hz).abs() < 0.005) {
            return Some(*r);
        }
        // Otherwise accept a whole number, and refuse anything else rather than
        // inventing a denominator.
        if (hz - hz.round()).abs() < 1e-9 && hz.round() <= f64::from(u32::MAX) {
            return Rate::whole(hz.round() as u32);
        }
        None
    }
}

impl fmt::Debug for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// How a conversion that cannot be exact should resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    /// Toward negative infinity — the frame a time falls within.
    Floor,
    /// To the nearest, ties away from zero.
    Nearest,
    /// Toward positive infinity.
    Ceil,
}

/// An exact instant or duration: `value` ticks at `rate`.
///
/// Equality, ordering and hashing are by **value**, so one second at 48 kHz and
/// one second at 24 fps are the same instant and the same hash key.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ExactTime {
    value: i64,
    rate: Rate,
}

impl ExactTime {
    pub fn new(value: i64, rate: Rate) -> ExactTime {
        ExactTime { value, rate }
    }

    /// A count of frames at a frame rate.
    pub fn from_frames(frames: i64, rate: Rate) -> ExactTime {
        ExactTime {
            value: frames,
            rate,
        }
    }

    pub fn zero(rate: Rate) -> ExactTime {
        ExactTime { value: 0, rate }
    }

    pub fn value(&self) -> i64 {
        self.value
    }
    pub fn rate(&self) -> Rate {
        self.rate
    }
    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    /// The instant in seconds as an exact fraction `(numerator, denominator)`.
    ///
    /// `value` ticks at `num/den` hertz is `value * den / num` seconds.
    fn as_seconds_fraction(&self) -> (i128, i128) {
        (
            i128::from(self.value) * i128::from(self.rate.den),
            i128::from(self.rate.num),
        )
    }

    /// The same fraction in lowest terms, sign on the numerator. This is the
    /// canonical form: two instants are equal exactly when these match.
    fn canonical(&self) -> (i128, i128) {
        let (mut n, mut d) = self.as_seconds_fraction();
        if d < 0 {
            n = -n;
            d = -d;
        }
        if n == 0 {
            return (0, 1);
        }
        let g = gcd(n.unsigned_abs(), d.unsigned_abs()) as i128;
        (n / g, d / g)
    }

    /// Seconds as a float. **Display only.**
    pub fn as_seconds_f64(&self) -> f64 {
        let (n, d) = self.as_seconds_fraction();
        n as f64 / d as f64
    }

    /// Convert to another rate.
    ///
    /// Returns the converted value and whether it was **exact**. A caller that
    /// ignores the flag is choosing to lose information; making it a return
    /// value rather than a silent rounding is the point.
    pub fn rescale(&self, to: Rate, rounding: Rounding) -> (ExactTime, bool) {
        if to == self.rate {
            return (*self, true);
        }
        // value * (den/num) * (to.num/to.den)
        let n = i128::from(self.value) * i128::from(self.rate.den) * i128::from(to.num);
        let d = i128::from(self.rate.num) * i128::from(to.den);
        let exact = n % d == 0;
        let q = match rounding {
            Rounding::Floor => n.div_euclid(d),
            Rounding::Ceil => -((-n).div_euclid(d)),
            Rounding::Nearest => {
                // Ties away from zero, matching SMPTE convention.
                let half = d / 2;
                if n >= 0 {
                    (n + half) / d
                } else {
                    (n - half) / d
                }
            }
        };
        (
            ExactTime {
                value: q as i64,
                rate: to,
            },
            exact,
        )
    }

    /// The frame index this instant falls within, at `rate`.
    pub fn to_frames_at(&self, rate: Rate) -> i64 {
        self.rescale(rate, Rounding::Floor).0.value
    }

    /// Add, in the rate that loses nothing.
    ///
    /// Mixed-rate addition resolves at a rate that represents both exactly
    /// where one exists, rather than at whichever operand happened to be on the
    /// left. The float model used the left-hand rate, which made `a + b` and
    /// `b + a` different values.
    pub fn add(&self, other: &ExactTime) -> ExactTime {
        let rate = common_rate(self.rate, other.rate);
        let (a, _) = self.rescale(rate, Rounding::Nearest);
        let (b, _) = other.rescale(rate, Rounding::Nearest);
        ExactTime {
            value: a.value + b.value,
            rate,
        }
    }

    pub fn sub(&self, other: &ExactTime) -> ExactTime {
        let rate = common_rate(self.rate, other.rate);
        let (a, _) = self.rescale(rate, Rounding::Nearest);
        let (b, _) = other.rescale(rate, Rounding::Nearest);
        ExactTime {
            value: a.value - b.value,
            rate,
        }
    }

    pub fn neg(&self) -> ExactTime {
        ExactTime {
            value: -self.value,
            rate: self.rate,
        }
    }

    /// SMPTE non-drop timecode, `HH:MM:SS:FF`.
    pub fn to_timecode(&self) -> String {
        let rate = self.rate.timecode_rate().max(1) as i64;
        let total = self.to_frames_at(self.rate);
        format_timecode(total, rate, false)
    }

    /// SMPTE drop-frame timecode, `HH:MM:SS;FF`.
    ///
    /// Drop-frame is a **labelling** scheme, not a rate change: it drops two
    /// labels every minute except every tenth, so that the label tracks wall
    /// clock on a 30000/1001 signal. It is only defined for NTSC rates whose
    /// nominal rate is a multiple of 30.
    pub fn to_drop_frame_timecode(&self) -> Option<String> {
        let nominal = self.rate.timecode_rate();
        if !self.rate.is_ntsc() || !nominal.is_multiple_of(30) {
            return None;
        }
        let frames = self.to_frames_at(self.rate);
        Some(format_drop_frame(frames, nominal))
    }
}

/// A rate that represents both inputs exactly, if one exists within `u32`.
///
/// For rates `a/b` and `c/d` the exact common rate is `lcm(a,c) / gcd(b,d)`.
/// When that overflows, the higher of the two is used — which is OTIO's rule,
/// and loses less than the lower would.
fn common_rate(a: Rate, b: Rate) -> Rate {
    if a == b {
        return a;
    }
    let lcm_num = (a.num as u128 / gcd(a.num as u128, b.num as u128)) * b.num as u128;
    let gcd_den = gcd(a.den as u128, b.den as u128);
    if lcm_num <= u32::MAX as u128 && gcd_den <= u32::MAX as u128 {
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

fn format_timecode(total: i64, rate: i64, drop: bool) -> String {
    let neg = total < 0;
    let t = total.abs();
    let ff = t % rate;
    let s = t / rate;
    let sep = if drop { ';' } else { ':' };
    format!(
        "{}{:02}:{:02}:{:02}{}{:02}",
        if neg { "-" } else { "" },
        s / 3600,
        (s / 60) % 60,
        s % 60,
        sep,
        ff
    )
}

/// Convert a frame count to a drop-frame label.
fn format_drop_frame(frames: i64, nominal: u32) -> String {
    let neg = frames < 0;
    let f = frames.abs();
    // Two labels are dropped per minute at 30, scaled for 60 etc.
    let drop_per_min = i64::from(nominal / 15);
    let frames_per_10min = i64::from(nominal) * 600 - 9 * drop_per_min;
    let frames_per_min = i64::from(nominal) * 60 - drop_per_min;

    let ten_minute_blocks = f / frames_per_10min;
    let mut remainder = f % frames_per_10min;
    // The first minute of each ten drops nothing.
    let first_minute = i64::from(nominal) * 60;
    let extra_minutes = if remainder < first_minute {
        0
    } else {
        remainder -= first_minute;
        1 + remainder / frames_per_min
    };
    let dropped = drop_per_min * (9 * ten_minute_blocks + extra_minutes);
    let labelled = f + dropped;

    let n = i64::from(nominal);
    let ff = labelled % n;
    let s = labelled / n;
    format!(
        "{}{:02}:{:02}:{:02};{:02}",
        if neg { "-" } else { "" },
        s / 3600,
        (s / 60) % 60,
        s % 60,
        ff
    )
}

impl PartialEq for ExactTime {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ExactTime {}

impl std::hash::Hash for ExactTime {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let (n, d) = self.canonical();
        n.hash(state);
        d.hash(state);
    }
}

impl PartialOrd for ExactTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExactTime {
    fn cmp(&self, other: &Self) -> Ordering {
        let (an, ad) = self.as_seconds_fraction();
        let (bn, bd) = other.as_seconds_fraction();
        (an * bd).cmp(&(bn * ad))
    }
}

impl fmt::Debug for ExactTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{:?}", self.value, self.rate)
    }
}

impl fmt::Display for ExactTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.value, self.rate)
    }
}
