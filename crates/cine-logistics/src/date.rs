//! Calendar dates and times of day without an external date crate.
//!
//! Scheduling only needs civil dates, weekday arithmetic and minute-level
//! times. [`Date`] uses the proleptic Gregorian calendar with the
//! days-from-civil algorithm (Howard Hinnant), which is exact for every year
//! representable in an `i32`.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// Monday = 0 … Sunday = 6.
    pub fn index(self) -> u32 {
        self as u32
    }

    fn from_index(i: u32) -> Weekday {
        match i % 7 {
            0 => Weekday::Monday,
            1 => Weekday::Tuesday,
            2 => Weekday::Wednesday,
            3 => Weekday::Thursday,
            4 => Weekday::Friday,
            5 => Weekday::Saturday,
            _ => Weekday::Sunday,
        }
    }

    /// Three-letter abbreviation (`Mon`).
    pub fn short(self) -> &'static str {
        match self {
            Weekday::Monday => "Mon",
            Weekday::Tuesday => "Tue",
            Weekday::Wednesday => "Wed",
            Weekday::Thursday => "Thu",
            Weekday::Friday => "Fri",
            Weekday::Saturday => "Sat",
            Weekday::Sunday => "Sun",
        }
    }
}

/// A civil (Gregorian) calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Date {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub day: u32,
}

impl Default for Date {
    fn default() -> Self {
        Date {
            year: 1970,
            month: 1,
            day: 1,
        }
    }
}

impl Date {
    /// Build a date, validating month and day-of-month (leap years included).
    pub fn new(year: i32, month: u32, day: u32) -> Result<Date> {
        if !(1..=12).contains(&month) {
            return Err(Error::InvalidDate(format!("month {month} out of range")));
        }
        if day == 0 || day > Date::days_in_month(year, month) {
            return Err(Error::InvalidDate(format!(
                "day {day} out of range for {year}-{month:02}"
            )));
        }
        Ok(Date { year, month, day })
    }

    /// Whether `year` is a Gregorian leap year.
    pub fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
    }

    /// Number of days in `month` of `year`.
    pub fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if Date::is_leap_year(year) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }

    /// Parse `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Result<Date> {
        let parts: Vec<&str> = s.trim().split('-').collect();
        if parts.len() != 3 {
            return Err(Error::InvalidDate(format!(
                "expected YYYY-MM-DD, got `{s}`"
            )));
        }
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| Error::InvalidDate(format!("bad year in `{s}`")))?;
        let month: u32 = parts[1]
            .parse()
            .map_err(|_| Error::InvalidDate(format!("bad month in `{s}`")))?;
        let day: u32 = parts[2]
            .parse()
            .map_err(|_| Error::InvalidDate(format!("bad day in `{s}`")))?;
        Date::new(year, month, day)
    }

    /// Days since 1970-01-01 (negative before the epoch).
    pub fn to_days(self) -> i64 {
        let y = i64::from(if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        });
        let m = i64::from(self.month);
        let d = i64::from(self.day);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Inverse of [`Date::to_days`].
    pub fn from_days(days: i64) -> Date {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if m <= 2 { y + 1 } else { y };
        Date {
            year: year as i32,
            month: m as u32,
            day: d as u32,
        }
    }

    /// The date `n` days later (or earlier when negative).
    pub fn add_days(self, n: i64) -> Date {
        Date::from_days(self.to_days() + n)
    }

    /// Signed number of days from `self` to `other`.
    pub fn days_until(self, other: Date) -> i64 {
        other.to_days() - self.to_days()
    }

    /// Day of the week.
    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday (index 3).
        Weekday::from_index((self.to_days() + 3).rem_euclid(7) as u32)
    }

    /// Saturday or Sunday.
    pub fn is_weekend(self) -> bool {
        matches!(self.weekday(), Weekday::Saturday | Weekday::Sunday)
    }

    /// `YYYY-MM-DD`.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.iso())
    }
}

/// An inclusive range of dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DateRange {
    pub start: Date,
    /// Inclusive.
    pub end: Date,
}

impl DateRange {
    /// Build a range; `start` and `end` are swapped if given out of order.
    pub fn new(start: Date, end: Date) -> DateRange {
        if end < start {
            DateRange {
                start: end,
                end: start,
            }
        } else {
            DateRange { start, end }
        }
    }

    /// A single-day range.
    pub fn single(date: Date) -> DateRange {
        DateRange {
            start: date,
            end: date,
        }
    }

    /// Whether `date` falls inside the (inclusive) range.
    pub fn contains(&self, date: Date) -> bool {
        self.start <= date && date <= self.end
    }

    /// Number of calendar days covered (at least 1).
    pub fn len_days(&self) -> i64 {
        self.start.days_until(self.end) + 1
    }
}

/// A time of day with minute resolution (24-hour clock).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct Time {
    /// 0..=23
    pub hour: u8,
    /// 0..=59
    pub minute: u8,
}

impl Time {
    /// Build a time, validating the range.
    pub fn new(hour: u8, minute: u8) -> Result<Time> {
        if hour > 23 || minute > 59 {
            return Err(Error::InvalidTime(format!(
                "{hour:02}:{minute:02} out of range"
            )));
        }
        Ok(Time { hour, minute })
    }

    /// Time from minutes since midnight, wrapping around the day.
    pub fn from_minutes(minutes: i32) -> Time {
        let m = minutes.rem_euclid(24 * 60);
        Time {
            hour: (m / 60) as u8,
            minute: (m % 60) as u8,
        }
    }

    /// Minutes since midnight.
    pub fn minutes_since_midnight(self) -> i32 {
        i32::from(self.hour) * 60 + i32::from(self.minute)
    }

    /// Parse `HH:MM` or `H:MM` (24-hour), with an optional `am`/`pm` suffix.
    pub fn parse(s: &str) -> Result<Time> {
        let raw = s.trim().to_ascii_lowercase();
        let (body, meridiem) = if let Some(b) = raw.strip_suffix("am") {
            (b.trim().to_string(), Some(false))
        } else if let Some(b) = raw.strip_suffix("pm") {
            (b.trim().to_string(), Some(true))
        } else {
            (raw.clone(), None)
        };
        let (h, m) = body
            .split_once(':')
            .ok_or_else(|| Error::InvalidTime(format!("expected HH:MM, got `{s}`")))?;
        let mut hour: u8 = h
            .parse()
            .map_err(|_| Error::InvalidTime(format!("bad hour in `{s}`")))?;
        let minute: u8 = m
            .parse()
            .map_err(|_| Error::InvalidTime(format!("bad minute in `{s}`")))?;
        if let Some(pm) = meridiem {
            if hour == 0 || hour > 12 {
                return Err(Error::InvalidTime(format!(
                    "12-hour clock hour out of range in `{s}`"
                )));
            }
            hour %= 12;
            if pm {
                hour += 12;
            }
        }
        Time::new(hour, minute)
    }

    /// Add (or subtract, when negative) minutes, wrapping around midnight.
    pub fn add_minutes(self, minutes: i32) -> Time {
        Time::from_minutes(self.minutes_since_midnight() + minutes)
    }

    /// Signed minutes from `self` to `other` on the same day.
    pub fn minutes_until(self, other: Time) -> i32 {
        other.minutes_since_midnight() - self.minutes_since_midnight()
    }

    /// `HH:MM`.
    pub fn hhmm(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.hhmm())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_round_trip_and_weekday() {
        let epoch = Date::new(1970, 1, 1).unwrap();
        assert_eq!(epoch.to_days(), 0);
        assert_eq!(epoch.weekday(), Weekday::Thursday);
        assert_eq!(Date::from_days(0), epoch);
        assert_eq!(Date::new(2026, 1, 5).unwrap().weekday(), Weekday::Monday);
        assert_eq!(Date::new(2026, 9, 7).unwrap().weekday(), Weekday::Monday);
        assert_eq!(Date::new(2000, 2, 29).unwrap().weekday(), Weekday::Tuesday);
        assert_eq!(Date::new(1969, 12, 31).unwrap().to_days(), -1);
    }

    #[test]
    fn add_days_crosses_month_and_leap_boundaries() {
        let d = Date::new(2024, 2, 28).unwrap();
        assert_eq!(d.add_days(1), Date::new(2024, 2, 29).unwrap());
        assert_eq!(d.add_days(2), Date::new(2024, 3, 1).unwrap());
        assert_eq!(
            Date::new(2023, 2, 28).unwrap().add_days(1),
            Date::new(2023, 3, 1).unwrap()
        );
        assert_eq!(
            Date::new(2025, 12, 31).unwrap().add_days(1),
            Date::new(2026, 1, 1).unwrap()
        );
        assert_eq!(
            Date::new(2026, 1, 1).unwrap().add_days(-1),
            Date::new(2025, 12, 31).unwrap()
        );
        for n in -100_000..100_000i64 {
            let d = Date::from_days(n);
            assert_eq!(d.to_days(), n);
        }
    }

    #[test]
    fn validation_and_parse() {
        assert!(Date::new(2023, 2, 29).is_err());
        assert!(Date::new(2023, 13, 1).is_err());
        assert!(Date::new(2023, 4, 31).is_err());
        assert_eq!(
            Date::parse("2026-03-09").unwrap(),
            Date::new(2026, 3, 9).unwrap()
        );
        assert!(Date::parse("2026/03/09").is_err());
        assert_eq!(Date::new(2026, 3, 9).unwrap().to_string(), "2026-03-09");
    }

    #[test]
    fn date_range_contains() {
        let r = DateRange::new(
            Date::new(2026, 1, 10).unwrap(),
            Date::new(2026, 1, 5).unwrap(),
        );
        assert_eq!(r.start, Date::new(2026, 1, 5).unwrap());
        assert!(r.contains(Date::new(2026, 1, 5).unwrap()));
        assert!(r.contains(Date::new(2026, 1, 10).unwrap()));
        assert!(!r.contains(Date::new(2026, 1, 11).unwrap()));
        assert_eq!(r.len_days(), 6);
    }

    #[test]
    fn time_parse_format_add() {
        let t = Time::parse("06:30").unwrap();
        assert_eq!(t, Time::new(6, 30).unwrap());
        assert_eq!(t.to_string(), "06:30");
        assert_eq!(Time::parse("7:05").unwrap().hhmm(), "07:05");
        assert_eq!(Time::parse("7:05 pm").unwrap().hhmm(), "19:05");
        assert_eq!(Time::parse("12:00am").unwrap().hhmm(), "00:00");
        assert_eq!(Time::parse("12:00pm").unwrap().hhmm(), "12:00");
        assert!(Time::parse("25:00").is_err());
        assert!(Time::parse("630").is_err());
        assert_eq!(t.add_minutes(-90).hhmm(), "05:00");
        assert_eq!(t.add_minutes(30).hhmm(), "07:00");
        assert_eq!(Time::new(23, 30).unwrap().add_minutes(60).hhmm(), "00:30");
        assert_eq!(Time::new(0, 30).unwrap().add_minutes(-60).hhmm(), "23:30");
        assert_eq!(t.minutes_until(Time::new(8, 0).unwrap()), 90);
    }
}
