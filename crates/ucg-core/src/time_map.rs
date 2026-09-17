//! Speed that changes over time: a clip's media position as an exact function
//! of its time on the track.
//!
//! A [`TimeMap`] is a run of [`Ramp`] segments. Each lasts a length of *track*
//! time and moves its speed — media time per track time — linearly from one
//! value to another across that length. A constant speed is one segment whose
//! two ends are equal, so there is one kind of retime, not two.
//!
//! Within a segment of length `D` whose speed runs from `v₀` to `v₁`, the media
//! consumed after `u` of track time is the integral of the speed:
//!
//! ```text
//! m(u) = v₀·u + (v₁ − v₀)·u² / (2·D)
//! ```
//!
//! Every quantity here is an exact rational ([`Q`], `i128` over `i128`): the
//! position of a frame an hour into a ramped clip is the same number however it
//! is reached. The only rounding is the last step, onto a grid — a picture
//! index or a sample — and it is always a floor, because what plays at an
//! instant is the picture that has started.

use crate::exact_time::Rate;
use crate::time::RationalTime;
use serde::{Deserialize, Serialize};
// Method-call form (`a.add(b)`) reads better than operators in the longer
// expressions below; the traits make it available.
use std::ops::{Add, Div, Mul, Sub};

/// An exact rational, `num / den` with `den > 0`, kept in lowest terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Q {
    pub num: i128,
    pub den: i128,
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl Q {
    pub fn new(num: i128, den: i128) -> Q {
        assert!(den != 0, "a rational with a zero denominator");
        let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
        let g = gcd(num, den);
        Q {
            num: num / g,
            den: den / g,
        }
    }
    pub const ZERO: Q = Q { num: 0, den: 1 };
    pub fn int(n: i128) -> Q {
        Q { num: n, den: 1 }
    }
    /// Seconds of an exact time.
    pub fn seconds(t: RationalTime) -> Q {
        let r = t.rate_exact();
        Q::new(
            i128::from(t.ticks()) * i128::from(r.den()),
            i128::from(r.num()),
        )
    }
    pub fn floor(self) -> i128 {
        self.num.div_euclid(self.den)
    }
    /// The part past the floor, in `[0, 1)`, as a float for interpolation only.
    pub fn frac(self) -> f64 {
        self.num.rem_euclid(self.den) as f64 / self.den as f64
    }
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
    pub fn is_integer(self) -> bool {
        self.den == 1
    }
}

impl std::ops::Add for Q {
    type Output = Q;
    fn add(self, o: Q) -> Q {
        let g = gcd(self.den, o.den);
        Q::new(
            self.num * (o.den / g) + o.num * (self.den / g),
            self.den / g * o.den,
        )
    }
}

impl std::ops::Sub for Q {
    type Output = Q;
    fn sub(self, o: Q) -> Q {
        self + Q::new(-o.num, o.den)
    }
}

impl std::ops::Mul for Q {
    type Output = Q;
    fn mul(self, o: Q) -> Q {
        let g1 = gcd(self.num, o.den);
        let g2 = gcd(o.num, self.den);
        Q::new(
            (self.num / g1) * (o.num / g2),
            (self.den / g2) * (o.den / g1),
        )
    }
}

impl std::ops::Div for Q {
    type Output = Q;
    fn div(self, o: Q) -> Q {
        assert!(o.num != 0, "division by zero");
        self * Q::new(o.den, o.num)
    }
}

impl PartialOrd for Q {
    fn partial_cmp(&self, other: &Q) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Q {
    fn cmp(&self, other: &Q) -> std::cmp::Ordering {
        (self.num * other.den).cmp(&(other.num * self.den))
    }
}

/// A speed: media time per track time, as a ratio of integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    pub num: i64,
    pub den: i64,
}

impl Ratio {
    pub fn new(num: i64, den: i64) -> Ratio {
        assert!(den > 0 && num >= 0, "a speed is a non-negative ratio");
        let g = gcd(i128::from(num), i128::from(den)) as i64;
        Ratio {
            num: num / g,
            den: den / g,
        }
    }
    pub fn q(self) -> Q {
        Q::new(i128::from(self.num), i128::from(self.den))
    }

    /// Parse a speed as an editor types it — `150%`, `3/2`, `1.5`, `0.25x` —
    /// exactly: a decimal string is a rational already, so `33.3%` is 333/1000,
    /// never a float that is almost it.
    pub fn parse(text: &str) -> Option<Ratio> {
        let t = text.trim().trim_end_matches(['x', 'X']).trim();
        let (t, scale) = match t.strip_suffix('%') {
            Some(p) => (p.trim(), 100i64),
            None => (t, 1),
        };
        if let Some((n, d)) = t.split_once('/') {
            let (n, d): (i64, i64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            if d <= 0 || n < 0 {
                return None;
            }
            return Some(Ratio::new(n, d.checked_mul(scale)?));
        }
        let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
        if whole.is_empty() && frac.is_empty() {
            return None;
        }
        if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if frac.len() > 9 {
            return None;
        }
        let den = 10i64.pow(frac.len() as u32);
        let num = format!("{}{}", if whole.is_empty() { "0" } else { whole }, frac)
            .parse::<i64>()
            .ok()?;
        Some(Ratio::new(num, den.checked_mul(scale)?))
    }
}

/// One stretch of a time map: a length of track time over which the speed
/// moves linearly from `from` to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ramp {
    pub length: RationalTime,
    pub from: Ratio,
    pub to: Ratio,
}

impl Ramp {
    pub fn constant(length: RationalTime, speed: Ratio) -> Ramp {
        Ramp {
            length,
            from: speed,
            to: speed,
        }
    }

    /// Media seconds consumed `u` seconds into this segment (`0 ≤ u ≤ D`).
    fn consumed(&self, u: Q) -> Q {
        let (v0, v1) = (self.from.q(), self.to.q());
        let d = Q::seconds(self.length);
        if d.num == 0 {
            return Q::ZERO;
        }
        v0.mul(u)
            .add(v1.sub(v0).mul(u).mul(u).div(Q::int(2).mul(d)))
    }
}

/// A clip's media position as a function of its track time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeMap {
    pub segments: Vec<Ramp>,
    /// The media runs backwards: offsets are negated, for a caller to add to a
    /// start at the end of the range.
    #[serde(default)]
    pub reversed: bool,
}

impl TimeMap {
    /// A constant speed over `played` of track time.
    pub fn constant(played: RationalTime, speed: Ratio, reversed: bool) -> TimeMap {
        TimeMap {
            segments: vec![Ramp::constant(played, speed)],
            reversed,
        }
    }

    /// Track time the map covers.
    pub fn played(&self) -> Q {
        self.segments
            .iter()
            .fold(Q::ZERO, |acc, s| acc.add(Q::seconds(s.length)))
    }

    /// Media seconds the whole map consumes.
    pub fn consumed(&self) -> Q {
        self.segments
            .iter()
            .fold(Q::ZERO, |acc, s| acc.add(s.consumed(Q::seconds(s.length))))
    }

    /// The speed, when it is one speed throughout.
    pub fn constant_speed(&self) -> Option<Ratio> {
        self.is_constant()
            .then(|| self.segments.first().map(|s| s.from))
            .flatten()
    }

    /// Whether every segment runs at one speed, the same speed.
    pub fn is_constant(&self) -> bool {
        let first = self.segments.first().map(|s| s.from);
        self.segments
            .iter()
            .all(|s| Some(s.from) == first && Some(s.to) == first)
    }

    /// The fastest speed anywhere in the map, for choosing a filter that will
    /// not alias at its fastest.
    pub fn max_speed(&self) -> f64 {
        self.segments
            .iter()
            .flat_map(|s| [s.from.q().to_f64(), s.to.q().to_f64()])
            .fold(0.0, f64::max)
    }

    /// Exact media seconds consumed `t` of track time into the map. Before the
    /// start it is zero; past the end the last segment's end speed continues,
    /// so a frame read just past the end is the next one, not a repeat.
    pub fn media_seconds(&self, t: Q) -> Q {
        if t <= Q::ZERO {
            return Q::ZERO;
        }
        let mut at = Q::ZERO;
        let mut media = Q::ZERO;
        for s in &self.segments {
            let d = Q::seconds(s.length);
            if t <= at.add(d) {
                return media.add(s.consumed(t.sub(at)));
            }
            at = at.add(d);
            media = media.add(s.consumed(d));
        }
        let end = self.segments.last().map(|s| s.to.q()).unwrap_or(Q::int(1));
        media.add(end.mul(t.sub(at)))
    }

    /// Media offset `t` into the map, floored onto `grid`, negated when
    /// reversed.
    pub fn media_offset(&self, t: RationalTime, grid: Rate) -> RationalTime {
        let m = self.media_seconds(Q::seconds(t));
        let ticks = m
            .mul(Q::new(i128::from(grid.num()), i128::from(grid.den())))
            .floor();
        let ticks = if self.reversed { -ticks } else { ticks };
        RationalTime::exact(i64::try_from(ticks).unwrap_or(0), grid)
    }

    /// Where output sample `u` (at `out`) reads, in samples of a file at
    /// `file`: the whole sample and how far past it, before any reversal.
    pub fn sample_position(&self, u: i64, out: Rate, file: Rate) -> (i64, f64) {
        let t = Q::new(i128::from(u) * i128::from(out.den()), i128::from(out.num()));
        let s = self
            .media_seconds(t)
            .mul(Q::new(i128::from(file.num()), i128::from(file.den())));
        (s.floor() as i64, s.frac())
    }

    /// The speed at `t` into the map, for display.
    pub fn speed_at(&self, t: Q) -> Q {
        let mut at = Q::ZERO;
        for s in &self.segments {
            let d = Q::seconds(s.length);
            if t <= at.add(d) && d.num != 0 {
                let u = t.sub(at);
                return s.from.q().add(s.to.q().sub(s.from.q()).mul(u).div(d));
            }
            at = at.add(d);
        }
        self.segments.last().map(|s| s.to.q()).unwrap_or(Q::int(1))
    }
}

/// A time map anchored in a sequence: where the clip starts on the record and
/// the media shown there.
///
/// Every player of a retimed clip — the flattened picture list, the record
/// monitor, the native player, the mixer — asks this one question, "what media
/// plays at record time `t`", and gets it from the same arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timing {
    pub map: TimeMap,
    /// Record time at which the map starts: the clip's start on the track.
    pub record: RationalTime,
    /// Media shown at `record`, in whatever coordinates the owner measures
    /// media in. For picture played backwards, the last picture of the range;
    /// for sound played backwards, the end of the range.
    pub media: RationalTime,
}

impl Timing {
    /// The media at record time `t`, floored onto `grid`.
    pub fn media_at(&self, t: RationalTime, grid: Rate) -> RationalTime {
        self.media
            .checked_add(&self.map.media_offset(t.checked_sub(&self.record), grid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(n: i64) -> RationalTime {
        RationalTime::exact(n, Rate::whole(24).unwrap())
    }
    fn r24() -> Rate {
        Rate::whole(24).unwrap()
    }

    #[test]
    fn speeds_are_parsed_exactly_as_typed() {
        assert_eq!(Ratio::parse("150%"), Some(Ratio::new(3, 2)));
        assert_eq!(Ratio::parse("3/2"), Some(Ratio::new(3, 2)));
        assert_eq!(Ratio::parse("1.5"), Some(Ratio::new(3, 2)));
        assert_eq!(Ratio::parse("0.25x"), Some(Ratio::new(1, 4)));
        assert_eq!(Ratio::parse("33.3%"), Some(Ratio::new(333, 1000)));
        assert_eq!(Ratio::parse(".5"), Some(Ratio::new(1, 2)));
        assert_eq!(Ratio::parse("-1"), None);
        assert_eq!(Ratio::parse("fast"), None);
        assert_eq!(Ratio::parse("1/0"), None);
    }

    /// A constant map is exactly the constant speed it replaces.
    #[test]
    fn a_constant_map_is_the_constant_speed() {
        let m = TimeMap::constant(f(48), Ratio::new(8, 5), false);
        assert!(m.is_constant());
        assert_eq!(m.consumed(), Q::new(8 * 2, 5), "two seconds at 8/5");
        assert_eq!(m.media_offset(f(5), r24()), f(8));
        assert_eq!(m.media_offset(f(1), r24()), f(1), "1.6 floors to 1");
        let back = TimeMap::constant(f(48), Ratio::new(1, 1), true);
        assert_eq!(back.media_offset(f(7), r24()), f(-7));

        // Across rates, with no float anywhere: half a second of 30000/1001
        // record time at real time is twelve pictures of 24 fps media.
        let ntsc = RationalTime::exact(15, Rate::new(30000, 1001).unwrap());
        let real = TimeMap::constant(ntsc, Ratio::new(1, 1), false);
        assert_eq!(real.media_offset(ntsc, r24()), f(12));
    }

    /// A ramp from real time to double speed over two seconds consumes three
    /// seconds of media — the average speed times the length — exactly.
    #[test]
    fn a_ramp_consumes_its_average_speed() {
        let m = TimeMap {
            segments: vec![Ramp {
                length: f(48),
                from: Ratio::new(1, 1),
                to: Ratio::new(2, 1),
            }],
            reversed: false,
        };
        assert_eq!(m.consumed(), Q::int(3));
        // Halfway through: 1·1 + (1)·1²/(2·2) = 1.25 s.
        assert_eq!(m.media_seconds(Q::int(1)), Q::new(5, 4));
        assert_eq!(m.media_offset(f(24), r24()), f(30), "1.25 s is picture 30");
        // Speed is continuous and linear.
        assert_eq!(m.speed_at(Q::int(1)), Q::new(3, 2));
        assert!((m.max_speed() - 2.0).abs() < 1e-12);
    }

    /// Segments chain: the second starts where the first ended, in media too.
    #[test]
    fn segments_chain_in_track_and_media_time() {
        let m = TimeMap {
            segments: vec![
                Ramp::constant(f(24), Ratio::new(1, 1)),
                Ramp {
                    length: f(24),
                    from: Ratio::new(1, 1),
                    to: Ratio::new(3, 1),
                },
                Ramp::constant(f(24), Ratio::new(3, 1)),
            ],
            reversed: false,
        };
        assert_eq!(m.played(), Q::int(3));
        // 1 s at 1x, 1 s ramping 1→3 (2 s of media), 1 s at 3x.
        assert_eq!(m.consumed(), Q::int(6));
        assert_eq!(m.media_seconds(Q::int(1)), Q::int(1));
        assert_eq!(m.media_seconds(Q::int(2)), Q::int(3));
        assert_eq!(m.media_seconds(Q::new(5, 2)), Q::new(9, 2));
        // Monotone: every later instant reads later media.
        let mut last = RationalTime::exact(-1, r24());
        for i in 0..=72 {
            let now = m.media_offset(f(i), r24());
            assert!(now >= last, "frame {i}");
            last = now;
        }
    }

    /// Sample positions agree with the exact media seconds, and with the old
    /// constant stepper's arithmetic for a constant map.
    #[test]
    fn sample_positions_are_exact() {
        let out = Rate::whole(48_000).unwrap();
        let file = Rate::whole(44_100).unwrap();
        let m = TimeMap::constant(f(480), Ratio::new(2, 1), false);
        // 2x from 44.1 kHz onto 48 kHz: output 80 reads file sample 147.
        assert_eq!(m.sample_position(80, out, file), (147, 0.0));
        let (c, fr) = m.sample_position(81, out, file);
        assert_eq!(c, 148);
        assert!((fr - 0.8375).abs() < 1e-12, "{fr}");
    }
}
