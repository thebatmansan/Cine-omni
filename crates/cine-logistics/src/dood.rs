//! Day Out of Days: which actor is needed on which day, and what that costs.
//!
//! The stripboard says what is shot when; the DOOD says what that means for the
//! people. It is the document a producer negotiates contracts against and an
//! assistant director hands to the production office, and its codes are fixed
//! by decades of practice:
//!
//! | code | meaning |
//! |------|---------|
//! | `SW`  | Start Work — the first day, and they work it |
//! | `W`   | Work |
//! | `H`   | Hold — not shooting, but not released either, and still paid |
//! | `WF`  | Work Finish — the last day they work |
//! | `SWF` | Start Work Finish — they start and finish on the same day |
//!
//! A hold day is the point of the document: an actor who works day 1 and day 5
//! is *held* on days 2, 3 and 4, because they cannot be released and cannot
//! take other work. Counting them is how a schedule change turns into money,
//! which is why [`CastRow::held`] is carried separately and never folded into
//! a single "days" total.
//!
//! Days are the schedule's own day numbers; nothing here re-derives who works
//! when. Change the stripboard and the DOOD follows, because it is a function
//! of it.

use crate::schedule::Schedule;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What one actor does on one day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not yet started, or already finished: not on the report for that day.
    Off,
    Start,
    Work,
    Hold,
    Finish,
    StartFinish,
}

impl Status {
    /// The code as a DOOD prints it.
    pub fn code(&self) -> &'static str {
        match self {
            Status::Off => "",
            Status::Start => "SW",
            Status::Work => "W",
            Status::Hold => "H",
            Status::Finish => "WF",
            Status::StartFinish => "SWF",
        }
    }

    /// Whether the actor is paid for this day. A hold day is paid; that is
    /// what distinguishes it from a day off.
    pub fn is_engaged(&self) -> bool {
        !matches!(self, Status::Off)
    }
}

/// One actor's line across the schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CastRow {
    pub name: String,
    /// One entry per shooting day, in day order.
    pub days: Vec<Status>,
    /// Day number of the first working day (1-based).
    pub start: usize,
    /// Day number of the last working day.
    pub finish: usize,
    /// Days actually worked.
    pub worked: usize,
    /// Days held between the first and last: not worked, not released.
    pub held: usize,
}

impl CastRow {
    /// Days the production is committed to: worked plus held.
    pub fn engaged(&self) -> usize {
        self.worked + self.held
    }
}

/// The report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DayOutOfDays {
    /// Day numbers, in order, so a row's entries line up with them.
    pub days: Vec<usize>,
    pub cast: Vec<CastRow>,
}

impl DayOutOfDays {
    /// Total days of work across the cast — what the schedule commits to.
    pub fn total_worked(&self) -> usize {
        self.cast.iter().map(|c| c.worked).sum()
    }

    /// Total hold days: the cost of the shape of the schedule, as opposed to
    /// the work in it. Re-ordering the stripboard moves this number and
    /// nothing else.
    pub fn total_held(&self) -> usize {
        self.cast.iter().map(|c| c.held).sum()
    }

    /// The report as a table, cast down the side and days across.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Day Out of Days\n\n| Cast |");
        for day in &self.days {
            out.push_str(&format!(" {day} |"));
        }
        out.push_str(" Work | Hold |\n|---|");
        for _ in &self.days {
            out.push_str("---|");
        }
        out.push_str("---|---|\n");
        for row in &self.cast {
            out.push_str(&format!("| {} |", row.name));
            for status in &row.days {
                out.push_str(&format!(" {} |", status.code()));
            }
            out.push_str(&format!(" {} | {} |\n", row.worked, row.held));
        }
        out.push_str(&format!(
            "\n{} cast, {} days of work, {} days on hold.\n",
            self.cast.len(),
            self.total_worked(),
            self.total_held()
        ));
        out
    }
}

/// Build the report from a schedule.
pub fn day_out_of_days(schedule: &Schedule) -> DayOutOfDays {
    let days: Vec<usize> = schedule.days.iter().map(|d| d.index).collect();
    // Who works when, by the schedule's own account.
    let mut working: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, day) in schedule.days.iter().enumerate() {
        for name in &day.cast_on_call {
            working.entry(name.clone()).or_default().push(position);
        }
    }
    let cast = working
        .into_iter()
        .filter(|(_, positions)| !positions.is_empty())
        .map(|(name, positions)| {
            let (first, last) = (positions[0], *positions.last().expect("non-empty"));
            let mut statuses = vec![Status::Off; days.len()];
            for (position, status) in statuses.iter_mut().enumerate() {
                let works = positions.contains(&position);
                *status = match (position, works) {
                    // One day only: started and finished on it.
                    (p, true) if p == first && p == last => Status::StartFinish,
                    (p, true) if p == first => Status::Start,
                    (p, true) if p == last => Status::Finish,
                    (_, true) => Status::Work,
                    // Between the first and last day and not working: held.
                    (p, false) if p > first && p < last => Status::Hold,
                    _ => Status::Off,
                };
            }
            let held = statuses.iter().filter(|s| **s == Status::Hold).count();
            CastRow {
                name,
                days: statuses,
                start: days.get(first).copied().unwrap_or(first + 1),
                finish: days.get(last).copied().unwrap_or(last + 1),
                worked: positions.len(),
                held,
            }
        })
        .collect();
    DayOutOfDays { days, cast }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{ShootDay, Strip};
    use ucg_core::EntityId;
    use ucg_schema::IntExt;

    fn day(index: usize, cast: &[&str]) -> ShootDay {
        let strip = Strip {
            scene: EntityId::from_key(&format!("scene:{index}")),
            number: index.to_string(),
            pages_eighths: 8,
            int_ext: IntExt::Int,
            location: "STAGE".into(),
            time_of_day: "DAY".into(),
            cast: cast.iter().map(|c| c.to_string()).collect(),
            est_minutes: 1.0,
        };
        let mut d = ShootDay::for_test(index, strip);
        d.cast_on_call = cast.iter().map(|c| c.to_string()).collect();
        d.cast_on_call.sort();
        d
    }

    /// The codes, and the thing the document exists for: hold days.
    #[test]
    fn a_gap_between_two_working_days_is_a_hold_not_a_day_off() {
        let schedule = Schedule {
            days: vec![
                day(1, &["MAREN", "TOBIAS"]),
                day(2, &["TOBIAS"]),
                day(3, &["TOBIAS"]),
                day(4, &["MAREN"]),
                day(5, &["ELSE"]),
            ],
            warnings: Vec::new(),
        };
        let dood = day_out_of_days(&schedule);
        let row = |name: &str| dood.cast.iter().find(|c| c.name == name).unwrap().clone();

        // MAREN works 1 and 4: held on 2 and 3, not released.
        let maren = row("MAREN");
        assert_eq!(
            maren.days.iter().map(|s| s.code()).collect::<Vec<_>>(),
            vec!["SW", "H", "H", "WF", ""]
        );
        assert_eq!((maren.worked, maren.held, maren.engaged()), (2, 2, 4));
        assert_eq!((maren.start, maren.finish), (1, 4));

        // TOBIAS works three days running: no hold at all.
        let tobias = row("TOBIAS");
        assert_eq!(
            tobias.days.iter().map(|s| s.code()).collect::<Vec<_>>(),
            vec!["SW", "W", "WF", "", ""]
        );
        assert_eq!((tobias.worked, tobias.held), (3, 0));

        // ELSE works one day: start and finish together, and no hold before it.
        let else_ = row("ELSE");
        assert_eq!(
            else_.days.iter().map(|s| s.code()).collect::<Vec<_>>(),
            vec!["", "", "", "", "SWF"]
        );
        assert_eq!((else_.worked, else_.held), (1, 0));

        assert_eq!(dood.total_worked(), 6);
        assert_eq!(dood.total_held(), 2, "the schedule's shape costs two days");
    }

    /// Re-ordering the board changes the hold days and nothing else: the
    /// reason a producer reads this document at all.
    #[test]
    fn grouping_an_actors_days_removes_the_hold() {
        let scattered = Schedule {
            days: vec![day(1, &["MAREN"]), day(2, &["TOBIAS"]), day(3, &["MAREN"])],
            warnings: Vec::new(),
        };
        let grouped = Schedule {
            days: vec![day(1, &["MAREN"]), day(2, &["MAREN"]), day(3, &["TOBIAS"])],
            warnings: Vec::new(),
        };
        let (a, b) = (day_out_of_days(&scattered), day_out_of_days(&grouped));
        assert_eq!(a.total_worked(), b.total_worked(), "the same work");
        assert_eq!(a.total_held(), 1);
        assert_eq!(b.total_held(), 0, "grouping released the hold day");
    }

    #[test]
    fn the_table_has_a_column_per_day_and_the_codes_in_it() {
        let schedule = Schedule {
            days: vec![day(1, &["MAREN"]), day(2, &["TOBIAS"])],
            warnings: Vec::new(),
        };
        let text = day_out_of_days(&schedule).to_markdown();
        assert!(text.contains("| Cast | 1 | 2 | Work | Hold |"), "{text}");
        assert!(text.contains("| MAREN | SWF |  | 1 | 0 |"), "{text}");
        assert!(
            text.contains("2 cast, 2 days of work, 0 days on hold."),
            "{text}"
        );
    }
}
