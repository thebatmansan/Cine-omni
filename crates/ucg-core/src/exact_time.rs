//! Exact rational rates.
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
//! A [`Rate`] is an exact rational: 23.976 is `24000/1001`, never a decimal.
//! [`crate::RationalTime`] is an integer count of ticks at such a rate.
//! Comparison and arithmetic cross-multiply in `i128`, so they are exact for
//! any duration a film can have.
//!
//! Equality, ordering and hashing all agree and are all **by value**: the same
//! instant expressed at two rates is one instant. Deriving any of the three
//! would break the other two, which is a defect this project has already had
//! once and fixed.
//!
//! ## Status
//!
//! This module owns the rate primitives. The time value itself lives in
//! [`crate::time`], which is the single model the whole project uses. An
//! earlier stage of the migration had a second `ExactTime` type here while the
//! float `RationalTime` was still in use; that duplication is gone.

use serde::{Deserialize, Serialize};
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
