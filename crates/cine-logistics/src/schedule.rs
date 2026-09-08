//! Stripboard scheduling.
//!
//! Every scene becomes a [`Strip`]; strips are ordered to minimise company
//! moves (night block, then location, INT/EXT, time of day, script order) and
//! packed greedily into [`ShootDay`]s under page and hour limits. Night scenes
//! never share a day with day scenes when
//! [`ScheduleConstraints::night_shoots_together`] is set, and cast
//! availability windows are honoured by deferring strips to the next date on
//! which every actor is free (with a 60-day give-up that places the strip
//! anyway and records a warning).
//!
//! The scheduler is deterministic: the same breakdowns and constraints always
//! produce the same [`Schedule`].

use crate::breakdown::{eighths_str, SceneBreakdown};
use crate::date::{Date, DateRange, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use ucg_core::EntityId;
use ucg_schema::IntExt;

/// Shooting-time heuristic: hours of set time per minute of screen time.
/// (Five pages ≈ five screen minutes ≈ a twelve-hour day with moves.)
pub const SHOOT_HOURS_PER_SCREEN_MINUTE: f64 = 2.0;
/// Fallback when a scene has no duration estimate: hours per script page.
pub const SHOOT_HOURS_PER_PAGE: f64 = 2.0;
/// Hours lost to each additional location on the same day.
pub const COMPANY_MOVE_HOURS: f64 = 1.0;
/// How far past the last scheduled date the scheduler looks for a date on
/// which every cast member of a strip is available before giving up.
pub const AVAILABILITY_HORIZON_DAYS: i64 = 60;

/// Limits and preferences that drive the scheduler.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduleConstraints {
    /// Script pages per day (1 page = 8 eighths).
    pub max_pages_per_day: f64,
    pub max_hours_per_day: f64,
    /// 5 = Mon–Fri, 6 = Mon–Sat, 7 = every day.
    pub shooting_days_per_week: u32,
    pub start_date: Date,
    /// Dates on which each named cast member IS available. Names absent from
    /// the map are always available; a present name with no ranges is never.
    #[serde(default)]
    pub cast_availability: HashMap<String, Vec<DateRange>>,
    /// Group strips by location / INT-EXT to minimise company moves.
    pub group_by_location: bool,
    /// Never mix night and day scenes on one shoot day.
    pub night_shoots_together: bool,
}

impl Default for ScheduleConstraints {
    fn default() -> Self {
        ScheduleConstraints {
            max_pages_per_day: 5.0,
            max_hours_per_day: 12.0,
            shooting_days_per_week: 5,
            start_date: Date {
                year: 2026,
                month: 1,
                day: 5,
            },
            cast_availability: HashMap::new(),
            group_by_location: true,
            night_shoots_together: true,
        }
    }
}

impl ScheduleConstraints {
    /// Page limit expressed in eighths.
    pub fn max_eighths_per_day(&self) -> f64 {
        (self.max_pages_per_day * 8.0).max(1.0)
    }

    /// Whether `date` is a shooting day under the days-per-week rule.
    pub fn is_shooting_day(&self, date: Date) -> bool {
        let n = self.shooting_days_per_week.clamp(1, 7);
        date.weekday().index() < n
    }

    /// First shooting day on or after `date`.
    pub fn next_shooting_date(&self, mut date: Date) -> Date {
        while !self.is_shooting_day(date) {
            date = date.add_days(1);
        }
        date
    }

    /// First shooting day strictly after `date`.
    pub fn following_shooting_date(&self, date: Date) -> Date {
        self.next_shooting_date(date.add_days(1))
    }

    /// Whether every name in `cast` is available on `date`.
    pub fn cast_available(&self, cast: &[String], date: Date) -> bool {
        cast.iter()
            .all(|name| match self.cast_availability.get(name) {
                None => true,
                Some(ranges) => ranges.iter().any(|r| r.contains(date)),
            })
    }
}

/// One scene on the board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Strip {
    pub scene: EntityId,
    pub number: String,
    pub pages_eighths: u32,
    pub int_ext: IntExt,
    pub location: String,
    pub time_of_day: String,
    pub cast: Vec<String>,
    /// Estimated screen minutes (drives the hours-per-day limit).
    pub est_minutes: f64,
}

impl Strip {
    /// Build a strip from a breakdown sheet.
    pub fn from_breakdown(bd: &SceneBreakdown) -> Strip {
        Strip {
            scene: bd.scene,
            number: bd.number.clone(),
            pages_eighths: bd.page_eighths,
            int_ext: bd.int_ext,
            location: bd.location.clone(),
            time_of_day: bd.time_of_day.clone(),
            cast: bd.cast.clone(),
            est_minutes: bd.est_minutes,
        }
    }

    /// Night / dusk / evening.
    pub fn is_night(&self) -> bool {
        crate::breakdown::is_night_time(&self.time_of_day)
    }

    /// Estimated hours of set time.
    pub fn shoot_hours(&self) -> f64 {
        if self.est_minutes > 0.0 {
            self.est_minutes * SHOOT_HOURS_PER_SCREEN_MINUTE
        } else {
            f64::from(self.pages_eighths) / 8.0 * SHOOT_HOURS_PER_PAGE
        }
    }
}

/// One shooting day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShootDay {
    /// 1-based day number.
    pub index: usize,
    pub date: Date,
    pub strips: Vec<Strip>,
    pub total_eighths: u32,
    /// Sorted, unique cast names working that day.
    pub cast_on_call: Vec<String>,
    /// Locations in the order they are visited.
    pub locations: Vec<String>,
    pub is_night: bool,
}

impl ShootDay {
    fn new(index: usize, date: Date) -> ShootDay {
        ShootDay {
            index,
            date,
            strips: Vec::new(),
            total_eighths: 0,
            cast_on_call: Vec::new(),
            locations: Vec::new(),
            is_night: false,
        }
    }

    fn push(&mut self, strip: Strip) {
        if self.strips.is_empty() {
            self.is_night = strip.is_night();
        }
        self.total_eighths += strip.pages_eighths;
        if !self.locations.contains(&strip.location) {
            self.locations.push(strip.location.clone());
        }
        for c in &strip.cast {
            if !self.cast_on_call.contains(c) {
                self.cast_on_call.push(c.clone());
            }
        }
        self.cast_on_call.sort();
        self.strips.push(strip);
    }

    /// Estimated hours including company-move overhead.
    pub fn shoot_hours(&self) -> f64 {
        let moves = self.locations.len().saturating_sub(1) as f64;
        self.strips.iter().map(Strip::shoot_hours).sum::<f64>() + moves * COMPANY_MOVE_HOURS
    }

    /// Pages as `"4 3/8"`.
    pub fn pages_str(&self) -> String {
        eighths_str(self.total_eighths)
    }

    /// Whether the scene is shot on this day.
    pub fn contains_scene(&self, scene: EntityId) -> bool {
        self.strips.iter().any(|s| s.scene == scene)
    }

    /// One-line stripboard row.
    pub fn to_line(&self) -> String {
        let scenes: Vec<&str> = self.strips.iter().map(|s| s.number.as_str()).collect();
        format!(
            "Day {:>2} | {} {} | {:<5} | {:>6} pp | Sc {} | {} | Cast: {}",
            self.index,
            self.date.weekday().short(),
            self.date,
            if self.is_night { "NIGHT" } else { "DAY" },
            self.pages_str(),
            scenes.join(", "),
            self.locations.join(" > "),
            self.cast_on_call.join(", ")
        )
    }
}

/// A complete stripboard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Schedule {
    pub days: Vec<ShootDay>,
    /// Constraint violations the scheduler had to accept.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Schedule {
    /// Number of location changes between consecutive strips across the
    /// whole schedule (a move overnight still counts as a move).
    pub fn company_moves(&self) -> usize {
        let mut moves = 0;
        let mut last: Option<&str> = None;
        for strip in self.days.iter().flat_map(|d| d.strips.iter()) {
            if let Some(prev) = last {
                if prev != strip.location {
                    moves += 1;
                }
            }
            last = Some(strip.location.as_str());
        }
        moves
    }

    /// Day numbers (1-based) on which `name` works.
    pub fn cast_days(&self, name: &str) -> Vec<usize> {
        self.days
            .iter()
            .filter(|d| d.cast_on_call.iter().any(|c| c == name))
            .map(|d| d.index)
            .collect()
    }

    /// Day number a scene is scheduled on, if any.
    pub fn day_of_scene(&self, scene: EntityId) -> Option<usize> {
        self.days
            .iter()
            .find(|d| d.contains_scene(scene))
            .map(|d| d.index)
    }

    /// Total eighths across all days.
    pub fn total_eighths(&self) -> u32 {
        self.days.iter().map(|d| d.total_eighths).sum()
    }

    /// Every scene in schedule order.
    pub fn scenes(&self) -> Vec<EntityId> {
        self.days
            .iter()
            .flat_map(|d| d.strips.iter().map(|s| s.scene))
            .collect()
    }

    /// One line per day, then warnings.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("```\n");
        for d in &self.days {
            out.push_str(&d.to_line());
            out.push('\n');
        }
        out.push_str("```\n");
        out.push_str(&format!(
            "\n{} days, {} pages, {} company moves\n",
            self.days.len(),
            eighths_str(self.total_eighths()),
            self.company_moves()
        ));
        if !self.warnings.is_empty() {
            out.push_str("\nWarnings:\n");
            for w in &self.warnings {
                out.push_str(&format!("- {w}\n"));
            }
        }
        out
    }
}

/// `(night block, location, INT/EXT rank, time of day, script order)`.
type SortKey = (u8, String, u8, String, usize);

/// Sort key that clusters strips to minimise company moves.
fn sort_key(strip: &Strip, order: usize, c: &ScheduleConstraints) -> SortKey {
    let night = if c.night_shoots_together {
        u8::from(strip.is_night())
    } else {
        0
    };
    if c.group_by_location {
        let ie = match strip.int_ext {
            IntExt::Int => 0,
            IntExt::IntExt => 1,
            IntExt::Ext => 2,
            IntExt::Other => 3,
        };
        (
            night,
            strip.location.clone(),
            ie,
            strip.time_of_day.clone(),
            order,
        )
    } else {
        (night, String::new(), 0, String::new(), order)
    }
}

/// Build a stripboard from breakdown sheets.
pub fn schedule(breakdowns: &[SceneBreakdown], constraints: &ScheduleConstraints) -> Schedule {
    let mut keyed: Vec<(SortKey, Strip)> = breakdowns
        .iter()
        .enumerate()
        .map(|(i, bd)| {
            let strip = Strip::from_breakdown(bd);
            (sort_key(&strip, i, constraints), strip)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut remaining: VecDeque<Strip> = keyed.into_iter().map(|(_, s)| s).collect();

    let max_eighths = constraints.max_eighths_per_day();
    let max_hours = constraints.max_hours_per_day.max(0.1);
    let mut days: Vec<ShootDay> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut date = constraints.next_shooting_date(constraints.start_date);
    let mut last_active = date;

    while !remaining.is_empty() {
        let mut day = ShootDay::new(days.len() + 1, date);
        let mut deferred: Vec<Strip> = Vec::new();
        let force = last_active.days_until(date) > AVAILABILITY_HORIZON_DAYS;

        while let Some(strip) = remaining.pop_front() {
            let available = constraints.cast_available(&strip.cast, date);
            if !available && !(force && day.strips.is_empty()) {
                deferred.push(strip);
                continue;
            }
            if !day.strips.is_empty()
                && constraints.night_shoots_together
                && day.is_night != strip.is_night()
            {
                deferred.push(strip);
                continue;
            }
            let fits_pages = f64::from(day.total_eighths + strip.pages_eighths) <= max_eighths;
            let fits_hours = {
                let mut probe = day.clone();
                probe.push(strip.clone());
                probe.shoot_hours() <= max_hours
            };
            if day.strips.is_empty() {
                if !fits_pages || !fits_hours {
                    warnings.push(format!(
                        "scene {} ({} pp, {:.1} h) exceeds the daily limit on its own; scheduled on day {}",
                        strip.number,
                        eighths_str(strip.pages_eighths),
                        strip.shoot_hours(),
                        day.index
                    ));
                }
                if !available {
                    warnings.push(format!(
                        "scene {} placed on {} although cast is unavailable (no free date within {} days)",
                        strip.number, date, AVAILABILITY_HORIZON_DAYS
                    ));
                }
                day.push(strip);
                continue;
            }
            if fits_pages && fits_hours {
                day.push(strip);
            } else {
                // Day is full: keep script/location order by stopping here.
                deferred.push(strip);
                break;
            }
        }

        // Deferred strips go back to the front, preserving their relative order.
        while let Some(s) = deferred.pop() {
            remaining.push_front(s);
        }

        if !day.strips.is_empty() {
            last_active = date;
            days.push(day);
        }
        date = constraints.following_shooting_date(date);
    }

    Schedule { days, warnings }
}

/// A scene whose day changed between two schedules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovedScene {
    pub scene: EntityId,
    pub number: String,
    /// `None` when the scene was not in the old schedule.
    pub from_day: Option<usize>,
    pub from_date: Option<Date>,
    /// `None` when the scene is not in the new schedule.
    pub to_day: Option<usize>,
    pub to_date: Option<Date>,
}

impl MovedScene {
    /// Human sentence: `scene 12 moved from Day 2 to Day 4 (2026-01-08)`.
    pub fn describe(&self) -> String {
        match (self.from_day, self.to_day) {
            (Some(f), Some(t)) => format!(
                "scene {} moved from Day {} to Day {} ({})",
                self.number,
                f,
                t,
                self.to_date.map(|d| d.to_string()).unwrap_or_default()
            ),
            (None, Some(t)) => format!("scene {} added on Day {}", self.number, t),
            (Some(f), None) => format!("scene {} removed from Day {}", self.number, f),
            (None, None) => format!("scene {} unchanged", self.number),
        }
    }
}

/// Difference between two schedules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ScheduleDiff {
    /// Scenes that changed day (including added / removed ones).
    pub moved: Vec<MovedScene>,
    pub old_day_count: usize,
    pub new_day_count: usize,
}

impl ScheduleDiff {
    /// True when no scene changed day and the day count is unchanged.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.old_day_count == self.new_day_count
    }

    /// One sentence per change.
    pub fn describe(&self) -> Vec<String> {
        let mut out: Vec<String> = self.moved.iter().map(MovedScene::describe).collect();
        if self.old_day_count != self.new_day_count {
            out.push(format!(
                "schedule is now {} days (was {})",
                self.new_day_count, self.old_day_count
            ));
        }
        out
    }
}

/// Compare two schedules scene by scene.
pub fn diff(old: &Schedule, new: &Schedule) -> ScheduleDiff {
    let index = |s: &Schedule| -> BTreeMap<EntityId, (usize, Date, String)> {
        s.days
            .iter()
            .flat_map(|d| {
                d.strips
                    .iter()
                    .map(move |st| (st.scene, (d.index, d.date, st.number.clone())))
            })
            .collect()
    };
    let a = index(old);
    let b = index(new);
    let scenes: BTreeSet<EntityId> = a.keys().chain(b.keys()).copied().collect();
    let mut moved = Vec::new();
    for scene in scenes {
        let o = a.get(&scene);
        let n = b.get(&scene);
        let same_day = match (o, n) {
            (Some(x), Some(y)) => x.0 == y.0,
            _ => false,
        };
        if same_day {
            continue;
        }
        let number = n.or(o).map(|t| t.2.clone()).unwrap_or_default();
        moved.push(MovedScene {
            scene,
            number,
            from_day: o.map(|t| t.0),
            from_date: o.map(|t| t.1),
            to_day: n.map(|t| t.0),
            to_date: n.map(|t| t.1),
        });
    }
    // Stable, human order: by new day then old day.
    moved.sort_by(|x, y| {
        x.to_day
            .unwrap_or(usize::MAX)
            .cmp(&y.to_day.unwrap_or(usize::MAX))
            .then_with(|| x.number.cmp(&y.number))
    });
    ScheduleDiff {
        moved,
        old_day_count: old.days.len(),
        new_day_count: new.days.len(),
    }
}

/// Convenience: is `weekday` a working day for `days_per_week`?
pub fn is_working_weekday(weekday: Weekday, days_per_week: u32) -> bool {
    weekday.index() < days_per_week.clamp(1, 7)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bd(n: u32, heading: &str, cast: &[&str], eighths: u32) -> SceneBreakdown {
        let h = crate::breakdown::parse_heading(heading);
        SceneBreakdown {
            scene: EntityId::from_key(&format!("scene:{n}")),
            number: n.to_string(),
            heading: heading.to_string(),
            int_ext: h.int_ext,
            location: h.location,
            time_of_day: h.time_of_day,
            cast: cast.iter().map(|s| s.to_string()).collect(),
            props: vec![],
            extras_hint: None,
            page_eighths: eighths,
            est_minutes: f64::from(eighths) / 8.0,
            vehicles: false,
            animals: false,
            sfx: false,
            vfx: false,
        }
    }

    fn twenty() -> Vec<SceneBreakdown> {
        let locs = ["INT. KITCHEN", "EXT. STREET", "INT. OFFICE", "EXT. FOREST"];
        let tods = ["DAY", "NIGHT"];
        let names = ["ANNA", "BEN", "CARLA", "DAVE", "EVE"];
        (1..=20u32)
            .map(|i| {
                let heading = format!(
                    "{} - {}",
                    locs[(i as usize * 7) % 4],
                    tods[(i as usize / 3) % 2]
                );
                let cast: Vec<&str> = (0..(i % 3 + 1))
                    .map(|k| names[((i + k) % 5) as usize])
                    .collect();
                bd(i, &heading, &cast, (i % 7) + 2)
            })
            .collect()
    }

    fn constraints() -> ScheduleConstraints {
        ScheduleConstraints {
            max_pages_per_day: 4.0,
            max_hours_per_day: 100.0,
            ..Default::default()
        }
    }

    #[test]
    fn day_count_is_close_to_the_page_bound() {
        let bds = twenty();
        let c = constraints();
        let s = schedule(&bds, &c);
        let total: u32 = bds.iter().map(|b| b.page_eighths).sum();
        let lower = (f64::from(total) / c.max_eighths_per_day()).ceil() as usize;
        assert!(s.days.len() >= lower, "{} < {}", s.days.len(), lower);
        assert!(
            s.days.len() <= lower + 2,
            "{} > {} + 2",
            s.days.len(),
            lower
        );
        assert_eq!(s.scenes().len(), 20);
        assert_eq!(s.total_eighths(), total);
        for d in &s.days {
            assert!(f64::from(d.total_eighths) <= c.max_eighths_per_day());
            assert!(!d.date.is_weekend());
        }
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }

    #[test]
    fn nights_are_never_mixed_with_days() {
        let s = schedule(&twenty(), &constraints());
        for d in &s.days {
            let nights: BTreeSet<bool> = d.strips.iter().map(Strip::is_night).collect();
            assert_eq!(nights.len(), 1, "day {} mixes day and night", d.index);
            assert_eq!(d.is_night, d.strips[0].is_night());
        }
        // Mixing is allowed when the flag is off (some day should mix).
        let mut c = constraints();
        c.night_shoots_together = false;
        let s = schedule(&twenty(), &c);
        assert!(s.days.iter().any(|d| {
            let nights: BTreeSet<bool> = d.strips.iter().map(Strip::is_night).collect();
            nights.len() == 2
        }));
    }

    #[test]
    fn grouping_reduces_company_moves() {
        let mut c = constraints();
        let grouped = schedule(&twenty(), &c);
        c.group_by_location = false;
        c.night_shoots_together = false;
        let raw = schedule(&twenty(), &c);
        assert!(
            grouped.company_moves() < raw.company_moves(),
            "{} vs {}",
            grouped.company_moves(),
            raw.company_moves()
        );
    }

    #[test]
    fn cast_unavailability_is_respected() {
        let mut c = constraints();
        // BEN is only available in the second week.
        c.cast_availability.insert(
            "BEN".into(),
            vec![DateRange::new(
                Date::new(2026, 1, 12).unwrap(),
                Date::new(2026, 1, 16).unwrap(),
            )],
        );
        let s = schedule(&twenty(), &c);
        for d in &s.days {
            if d.cast_on_call.iter().any(|n| n == "BEN") {
                assert!(
                    d.date >= Date::new(2026, 1, 12).unwrap()
                        && d.date <= Date::new(2026, 1, 16).unwrap()
                );
            }
        }
        assert_eq!(s.scenes().len(), 20);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(!s.cast_days("BEN").is_empty());
    }

    #[test]
    fn never_available_cast_is_placed_with_warning() {
        let mut c = constraints();
        c.cast_availability.insert("EVE".into(), vec![]);
        let s = schedule(&twenty(), &c);
        assert_eq!(s.scenes().len(), 20);
        assert!(
            s.warnings.iter().any(|w| w.contains("unavailable")),
            "{:?}",
            s.warnings
        );
    }

    #[test]
    fn deterministic() {
        let a = schedule(&twenty(), &constraints());
        let b = schedule(&twenty(), &constraints());
        assert_eq!(a, b);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn diff_reports_moved_scenes() {
        let bds = twenty();
        let a = schedule(&bds, &constraints());
        let mut changed = bds.clone();
        changed[0].page_eighths = 30;
        let b = schedule(&changed, &constraints());
        let d = diff(&a, &b);
        assert!(!d.moved.is_empty());
        assert!(d.moved.iter().all(|m| m.from_day != m.to_day));
        let text = d.describe().join("\n");
        assert!(text.contains("moved from Day"), "{text}");
        assert!(text.contains("scene 1 ") || d.new_day_count != d.old_day_count);
    }

    #[test]
    fn oversized_scene_gets_its_own_day_and_a_warning() {
        let bds = vec![
            bd(1, "INT. HALL - DAY", &["A"], 80),
            bd(2, "INT. HALL - DAY", &["A"], 2),
        ];
        let s = schedule(&bds, &constraints());
        assert_eq!(s.days.len(), 2);
        assert_eq!(s.days[0].strips[0].number, "1");
        assert!(s.warnings.iter().any(|w| w.contains("exceeds")));
    }

    #[test]
    fn hours_limit_splits_days() {
        let bds: Vec<SceneBreakdown> = (1..=4)
            .map(|i| bd(i, "INT. HALL - DAY", &["A"], 4))
            .collect();
        // Each strip is 0.5 screen minutes = 1 h; limit 2.5 h → 2 strips per day.
        let c = ScheduleConstraints {
            max_pages_per_day: 10.0,
            max_hours_per_day: 2.5,
            ..Default::default()
        };
        let s = schedule(&bds, &c);
        assert_eq!(s.days.len(), 2);
    }

    #[test]
    fn six_day_weeks_skip_only_sunday() {
        let c = ScheduleConstraints {
            shooting_days_per_week: 6,
            ..Default::default()
        };
        let sat = Date::new(2026, 1, 10).unwrap();
        assert!(c.is_shooting_day(sat));
        assert_eq!(
            c.next_shooting_date(sat.add_days(1)),
            Date::new(2026, 1, 12).unwrap()
        );
        assert!(is_working_weekday(Weekday::Saturday, 6));
        assert!(!is_working_weekday(Weekday::Saturday, 5));
    }

    #[test]
    fn markdown_lists_every_day() {
        let s = schedule(&twenty(), &constraints());
        let md = s.to_markdown();
        for d in &s.days {
            assert!(md.contains(&format!("Day {:>2}", d.index)));
        }
        assert!(md.contains("company moves"));
    }
}
