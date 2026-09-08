//! Time primitives shared by every module, modelled on OpenTimelineIO's
//! `RationalTime` / `TimeRange` so the NLE serializes losslessly.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

/// A time value expressed as `value / rate` seconds. Integer frame counts at a
/// given rate are exact, which is what editorial needs.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RationalTime {
    pub value: f64,
    pub rate: f64,
}

impl Default for RationalTime {
    fn default() -> Self {
        RationalTime {
            value: 0.0,
            rate: 24.0,
        }
    }
}

impl RationalTime {
    pub const fn new(value: f64, rate: f64) -> Self {
        RationalTime { value, rate }
    }
    pub fn from_frames(frames: i64, rate: f64) -> Self {
        RationalTime {
            value: frames as f64,
            rate,
        }
    }
    pub fn from_seconds(seconds: f64, rate: f64) -> Self {
        RationalTime {
            value: seconds * rate,
            rate,
        }
    }
    pub fn to_seconds(&self) -> f64 {
        if self.rate == 0.0 {
            0.0
        } else {
            self.value / self.rate
        }
    }
    pub fn rescaled_to(&self, rate: f64) -> Self {
        if rate == self.rate {
            *self
        } else {
            RationalTime {
                value: self.to_seconds() * rate,
                rate,
            }
        }
    }
    pub fn to_frames(&self) -> i64 {
        self.value.floor() as i64
    }
    pub fn to_frames_at(&self, rate: f64) -> i64 {
        self.rescaled_to(rate).to_frames()
    }
    pub fn floor(&self) -> Self {
        RationalTime {
            value: self.value.floor(),
            rate: self.rate,
        }
    }
    pub fn round(&self) -> Self {
        RationalTime {
            value: self.value.round(),
            rate: self.rate,
        }
    }
    pub fn is_zero(&self) -> bool {
        self.value == 0.0
    }
    pub fn almost_equal(&self, other: &Self, eps_seconds: f64) -> bool {
        (self.to_seconds() - other.to_seconds()).abs() <= eps_seconds
    }

    /// SMPTE non-drop timecode string `HH:MM:SS:FF` at this time's rate.
    pub fn to_timecode(&self) -> String {
        let rate = self.rate.round().max(1.0) as i64;
        let total = self.value.round() as i64;
        let neg = total < 0;
        let total = total.abs();
        let ff = total % rate;
        let s = total / rate;
        let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
        format!(
            "{}{:02}:{:02}:{:02}:{:02}",
            if neg { "-" } else { "" },
            h,
            m,
            s,
            ff
        )
    }

    /// Parse `HH:MM:SS:FF` (non-drop) at `rate`.
    pub fn from_timecode(tc: &str, rate: f64) -> Option<Self> {
        let parts: Vec<i64> = tc
            .split([':', ';'])
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        if parts.len() != 4 {
            return None;
        }
        let r = rate.round() as i64;
        let frames = ((parts[0] * 3600 + parts[1] * 60 + parts[2]) * r) + parts[3];
        Some(RationalTime::from_frames(frames, rate))
    }
}

impl Add for RationalTime {
    type Output = RationalTime;
    fn add(self, rhs: Self) -> Self {
        let rhs = rhs.rescaled_to(self.rate);
        RationalTime {
            value: self.value + rhs.value,
            rate: self.rate,
        }
    }
}
impl Sub for RationalTime {
    type Output = RationalTime;
    fn sub(self, rhs: Self) -> Self {
        let rhs = rhs.rescaled_to(self.rate);
        RationalTime {
            value: self.value - rhs.value,
            rate: self.rate,
        }
    }
}
impl fmt::Display for RationalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.value, self.rate)
    }
}

/// Half-open range `[start, start + duration)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TimeRange {
    pub start_time: RationalTime,
    pub duration: RationalTime,
}

impl TimeRange {
    pub fn new(start_time: RationalTime, duration: RationalTime) -> Self {
        TimeRange {
            start_time,
            duration: duration.rescaled_to(start_time.rate),
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
    /// Inclusive end (last frame).
    pub fn end_time_inclusive(&self) -> RationalTime {
        let e = self.end_time_exclusive();
        RationalTime {
            value: e.value - 1.0,
            rate: e.rate,
        }
    }
    pub fn contains(&self, t: RationalTime) -> bool {
        let t = t.rescaled_to(self.start_time.rate);
        t.value >= self.start_time.value && t.value < self.end_time_exclusive().value
    }
    pub fn overlaps(&self, other: &TimeRange) -> bool {
        let o_start = other.start_time.rescaled_to(self.start_time.rate).value;
        let o_end = other
            .end_time_exclusive()
            .rescaled_to(self.start_time.rate)
            .value;
        self.start_time.value < o_end && o_start < self.end_time_exclusive().value
    }
    pub fn intersection(&self, other: &TimeRange) -> Option<TimeRange> {
        let rate = self.start_time.rate;
        let s = self
            .start_time
            .value
            .max(other.start_time.rescaled_to(rate).value);
        let e = self
            .end_time_exclusive()
            .value
            .min(other.end_time_exclusive().rescaled_to(rate).value);
        if e > s {
            Some(TimeRange::new(
                RationalTime::new(s, rate),
                RationalTime::new(e - s, rate),
            ))
        } else {
            None
        }
    }
    pub fn extended_by(&self, other: &TimeRange) -> TimeRange {
        let rate = self.start_time.rate;
        let s = self
            .start_time
            .value
            .min(other.start_time.rescaled_to(rate).value);
        let e = self
            .end_time_exclusive()
            .value
            .max(other.end_time_exclusive().rescaled_to(rate).value);
        TimeRange::new(RationalTime::new(s, rate), RationalTime::new(e - s, rate))
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
        assert_eq!(b.value, 96.0);
        assert_eq!((a + RationalTime::from_frames(24, 48.0)).value, 60.0);
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
}
