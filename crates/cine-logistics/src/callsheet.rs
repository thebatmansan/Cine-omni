//! Call sheets for one shoot day.
//!
//! Cast numbers are assigned once per document in order of first appearance
//! ([`CastRoster::from_breakdowns`]) so a character keeps the same number on
//! every call sheet. Makeup calls are 90 minutes before general call for
//! leads and 60 minutes for the rest; pickups are 30 minutes before makeup;
//! everyone is due on set 30 minutes after general call.

use crate::breakdown::{eighths_str, SceneBreakdown};
use crate::date::{Date, Time};
use crate::schedule::ShootDay;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ucg_core::EntityId;

/// Makeup lead time for leads, minutes before general call.
pub const LEAD_MAKEUP_MINUTES: i32 = 90;
/// Makeup lead time for supporting cast, minutes before general call.
pub const SUPPORT_MAKEUP_MINUTES: i32 = 60;
/// Pickup lead time before makeup, minutes.
pub const PICKUP_MINUTES: i32 = 30;
/// Minutes after general call at which cast is due on set.
pub const ON_SET_MINUTES: i32 = 30;
/// Fraction of a document's scenes a character must appear in to be a lead.
pub const LEAD_SCENE_FRACTION: f64 = 0.25;

/// A cast member with their stable call-sheet number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CastMember {
    pub name: String,
    /// 1-based, in order of first appearance.
    pub number: u32,
    pub is_lead: bool,
    /// Number of scenes the character appears in.
    pub scene_count: usize,
}

/// Cast numbering for a document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct CastRoster {
    pub members: Vec<CastMember>,
}

impl CastRoster {
    /// Number characters by first appearance across the document; a character
    /// is a lead when they appear in at least [`LEAD_SCENE_FRACTION`] of scenes.
    pub fn from_breakdowns(breakdowns: &[SceneBreakdown]) -> CastRoster {
        let mut members: Vec<CastMember> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for bd in breakdowns {
            for name in &bd.cast {
                match index.get(name) {
                    Some(&i) => members[i].scene_count += 1,
                    None => {
                        index.insert(name.clone(), members.len());
                        members.push(CastMember {
                            name: name.clone(),
                            number: members.len() as u32 + 1,
                            is_lead: false,
                            scene_count: 1,
                        });
                    }
                }
            }
        }
        let threshold = ((breakdowns.len() as f64) * LEAD_SCENE_FRACTION)
            .ceil()
            .max(1.0) as usize;
        for m in &mut members {
            m.is_lead = m.scene_count >= threshold;
        }
        CastRoster { members }
    }

    /// Look up a member by name.
    pub fn member(&self, name: &str) -> Option<&CastMember> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Cast number for a name.
    pub fn number_of(&self, name: &str) -> Option<u32> {
        self.member(name).map(|m| m.number)
    }

    /// Mark a character as lead (or not) explicitly.
    pub fn set_lead(&mut self, name: &str, is_lead: bool) {
        if let Some(m) = self.members.iter_mut().find(|m| m.name == name) {
            m.is_lead = is_lead;
        }
    }
}

/// A scene line on the call sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallSheetScene {
    pub scene: EntityId,
    pub number: String,
    pub heading: String,
    /// `"1 3/8"`.
    pub pages: String,
    pub cast_numbers: Vec<u32>,
    pub location: String,
}

/// One cast member's calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CastCall {
    pub name: String,
    pub number: u32,
    pub pickup: Time,
    pub makeup: Time,
    pub on_set: Time,
}

/// A department's call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrewCall {
    pub department: String,
    pub call: Time,
}

/// The call sheet for one shoot day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallSheet {
    /// 1-based day number.
    pub day: usize,
    pub date: Date,
    pub general_call: Time,
    pub scenes: Vec<CallSheetScene>,
    pub cast_calls: Vec<CastCall>,
    pub crew_calls: Vec<CrewCall>,
    pub locations: Vec<String>,
    pub weather_placeholder: String,
    pub notes: Vec<String>,
}

/// Build the call sheet for `day`.
pub fn build_call_sheet(
    day: &ShootDay,
    breakdowns: &[SceneBreakdown],
    cast_roster: &CastRoster,
    general_call: Time,
) -> CallSheet {
    let by_scene: HashMap<EntityId, &SceneBreakdown> =
        breakdowns.iter().map(|b| (b.scene, b)).collect();
    let mut notes: Vec<String> = Vec::new();
    let scenes: Vec<CallSheetScene> = day
        .strips
        .iter()
        .map(|strip| {
            let bd = by_scene.get(&strip.scene).copied();
            let cast_numbers: Vec<u32> = strip
                .cast
                .iter()
                .filter_map(|n| cast_roster.number_of(n))
                .collect::<std::collections::BTreeSet<u32>>()
                .into_iter()
                .collect();
            if let Some(bd) = bd {
                for flag in bd.flags() {
                    let note = match flag {
                        "VEH" => format!("Sc {}: picture vehicles on set", bd.number),
                        "ANI" => format!("Sc {}: animal wrangler required", bd.number),
                        "SFX" => format!("Sc {}: SFX supervisor / safety meeting", bd.number),
                        _ => format!("Sc {}: VFX supervisor on set", bd.number),
                    };
                    notes.push(note);
                }
                if let Some(h) = &bd.extras_hint {
                    notes.push(format!("Sc {}: {}", bd.number, h));
                }
            }
            CallSheetScene {
                scene: strip.scene,
                number: strip.number.clone(),
                heading: bd.map(|b| b.heading.clone()).unwrap_or_default(),
                pages: eighths_str(strip.pages_eighths),
                cast_numbers,
                location: strip.location.clone(),
            }
        })
        .collect();

    let mut cast_calls: Vec<CastCall> = day
        .cast_on_call
        .iter()
        .map(|name| {
            let (number, is_lead) = cast_roster
                .member(name)
                .map(|m| (m.number, m.is_lead))
                .unwrap_or((0, false));
            let lead_time = if is_lead {
                LEAD_MAKEUP_MINUTES
            } else {
                SUPPORT_MAKEUP_MINUTES
            };
            let makeup = general_call.add_minutes(-lead_time);
            CastCall {
                name: name.clone(),
                number,
                pickup: makeup.add_minutes(-PICKUP_MINUTES),
                makeup,
                on_set: general_call.add_minutes(ON_SET_MINUTES),
            }
        })
        .collect();
    cast_calls.sort_by(|a, b| a.number.cmp(&b.number).then_with(|| a.name.cmp(&b.name)));

    let earliest_makeup = cast_calls
        .iter()
        .map(|c| c.makeup)
        .min()
        .unwrap_or(general_call);
    let crew_calls = vec![
        CrewCall {
            department: "Assistant Directors".into(),
            call: general_call.add_minutes(-30),
        },
        CrewCall {
            department: "Art / Set Dressing".into(),
            call: general_call.add_minutes(-60),
        },
        CrewCall {
            department: "Hair / Makeup".into(),
            call: earliest_makeup.add_minutes(-15),
        },
        CrewCall {
            department: "Grip / Electric".into(),
            call: general_call.add_minutes(-30),
        },
        CrewCall {
            department: "Camera".into(),
            call: general_call,
        },
        CrewCall {
            department: "Sound".into(),
            call: general_call,
        },
    ];

    if day.is_night {
        notes.push("Night shoot: turnaround and lighting per night schedule".into());
    }

    CallSheet {
        day: day.index,
        date: day.date,
        general_call,
        scenes,
        cast_calls,
        crew_calls,
        locations: day.locations.clone(),
        weather_placeholder: "Weather: TBD (sunrise / sunset / forecast)".into(),
        notes,
    }
}

impl CallSheet {
    /// Markdown rendering of the sheet.
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# Call Sheet — Day {} — {} {}\n\nGeneral call: **{}**\n\n{}\n\n",
            self.day,
            self.date.weekday().short(),
            self.date,
            self.general_call,
            self.weather_placeholder
        );
        out.push_str(
            "## Scenes\n\n| Sc | Set | Pages | Cast | Location |\n|---|---|---|---|---|\n",
        );
        for s in &self.scenes {
            let cast: Vec<String> = s.cast_numbers.iter().map(|n| n.to_string()).collect();
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                s.number,
                s.heading,
                s.pages,
                cast.join(", "),
                s.location
            ));
        }
        out.push_str(
            "\n## Cast\n\n| # | Character | Pickup | Makeup | On set |\n|---|---|---|---|---|\n",
        );
        for c in &self.cast_calls {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                c.number, c.name, c.pickup, c.makeup, c.on_set
            ));
        }
        out.push_str("\n## Crew\n\n| Department | Call |\n|---|---|\n");
        for c in &self.crew_calls {
            out.push_str(&format!("| {} | {} |\n", c.department, c.call));
        }
        out.push_str("\n## Locations\n\n");
        for l in &self.locations {
            out.push_str(&format!("- {l}\n"));
        }
        if !self.notes.is_empty() {
            out.push_str("\n## Notes\n\n");
            for n in &self.notes {
                out.push_str(&format!("- {n}\n"));
            }
        }
        out
    }
}

/// The call sheet as a printable PDF page.
///
/// One page, US Letter, laid out the way a call sheet is read on set: the day
/// and the general call at the top, then what is being shot, then who is
/// needed and when, then the departments, then the notes. Everything on it
/// comes from the sheet; nothing is invented to fill a box, and a box with
/// nothing behind it says so rather than being left to look complete.
pub fn call_sheet_pdf(sheet: &CallSheet, production: &str) -> Vec<u8> {
    use crate::pdf::{Font, PageSize, Pdf};

    let size = PageSize::LETTER;
    let (left, right) = (48.0, size.width - 48.0);
    // ASCII in the metadata: a standard font has no em dash, and a title that
    // reads "Untitled ? day 1" in every reader is worse than a hyphen.
    let mut pdf = Pdf::new(format!("{production} - day {}", sheet.day));
    let page = pdf.page(size);
    let mut y = size.height - 56.0;

    // Banner: the production, and the day it is.
    page.rect(left, y - 6.0, right - left, 30.0, 0.12);
    page.text(left + 8.0, y + 6.0, 15.0, Font::HelveticaBold, production);
    page.text_right(
        right - 8.0,
        y + 6.0,
        13.0,
        Font::HelveticaBold,
        &format!("DAY {}  ·  {}", sheet.day, sheet.date),
    );
    y -= 28.0;
    page.text(
        left,
        y,
        11.0,
        Font::HelveticaBold,
        &format!("GENERAL CALL  {}", sheet.general_call),
    );
    page.text_right(
        right,
        y,
        10.0,
        Font::Helvetica,
        &if sheet.locations.is_empty() {
            "location not set".to_string()
        } else {
            sheet.locations.join("  ·  ")
        },
    );
    y -= 10.0;
    page.line(left, y, right, y, 1.0, 0.4);
    y -= 18.0;

    // What is being shot.
    let heading = |page: &mut crate::pdf::Page, y: &mut f32, text: &str| {
        page.text(left, *y, 10.0, Font::HelveticaBold, text);
        *y -= 12.0;
    };
    heading(page, &mut y, "SCENES");
    page.text(left, y, 8.0, Font::HelveticaBold, "SC");
    page.text(left + 40.0, y, 8.0, Font::HelveticaBold, "SET");
    page.text(left + 300.0, y, 8.0, Font::HelveticaBold, "CAST");
    page.text_right(right, y, 8.0, Font::HelveticaBold, "PAGES");
    y -= 4.0;
    page.line(left, y, right, y, 0.5, 0.6);
    y -= 11.0;
    for scene in &sheet.scenes {
        page.text(left, y, 9.0, Font::Helvetica, &scene.number);
        page.text(left + 40.0, y, 9.0, Font::Helvetica, &scene.heading);
        page.text(
            left + 300.0,
            y,
            9.0,
            Font::Helvetica,
            &scene
                .cast_numbers
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
        page.text_right(right, y, 9.0, Font::Helvetica, &scene.pages);
        y -= 12.0;
    }
    if sheet.scenes.is_empty() {
        page.text(
            left,
            y,
            9.0,
            Font::Helvetica,
            "nothing scheduled for this day",
        );
        y -= 12.0;
    }
    y -= 8.0;

    // Who is needed, and when.
    heading(page, &mut y, "CAST");
    page.text(left, y, 8.0, Font::HelveticaBold, "#");
    page.text(left + 24.0, y, 8.0, Font::HelveticaBold, "NAME");
    page.text(left + 240.0, y, 8.0, Font::HelveticaBold, "PICKUP");
    page.text(left + 320.0, y, 8.0, Font::HelveticaBold, "MAKE-UP");
    page.text(left + 400.0, y, 8.0, Font::HelveticaBold, "ON SET");
    y -= 4.0;
    page.line(left, y, right, y, 0.5, 0.6);
    y -= 11.0;
    for call in &sheet.cast_calls {
        page.text(left, y, 9.0, Font::Helvetica, &call.number.to_string());
        page.text(left + 24.0, y, 9.0, Font::Helvetica, &call.name);
        page.text(
            left + 240.0,
            y,
            9.0,
            Font::Helvetica,
            &call.pickup.to_string(),
        );
        page.text(
            left + 320.0,
            y,
            9.0,
            Font::Helvetica,
            &call.makeup.to_string(),
        );
        page.text(
            left + 400.0,
            y,
            9.0,
            Font::Helvetica,
            &call.on_set.to_string(),
        );
        y -= 12.0;
    }
    y -= 8.0;

    // Departments.
    heading(page, &mut y, "CREW");
    let mut x = left;
    for call in &sheet.crew_calls {
        page.text(
            x,
            y,
            9.0,
            Font::Helvetica,
            &format!("{}  {}", call.department, call.call),
        );
        x += 130.0;
        if x > right - 100.0 {
            x = left;
            y -= 12.0;
        }
    }
    if x != left {
        y -= 12.0;
    }
    y -= 8.0;

    // Notes, and the boxes that have nothing behind them yet.
    heading(page, &mut y, "NOTES");
    for note in &sheet.notes {
        page.text(left, y, 9.0, Font::Helvetica, note);
        y -= 11.0;
    }
    page.text(left, y, 9.0, Font::Helvetica, &sheet.weather_placeholder);
    y -= 11.0;
    page.text(
        left,
        y,
        9.0,
        Font::Helvetica,
        "Hospital, unit base and transport are not in this project yet.",
    );

    page.line(left, 40.0, right, 40.0, 0.5, 0.6);
    page.text(
        left,
        30.0,
        7.5,
        Font::Helvetica,
        "Generated by CINE-OMNI from the schedule. Times are derived from the general call.",
    );
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{schedule, ScheduleConstraints};

    fn bd(n: u32, heading: &str, cast: &[&str], eighths: u32) -> SceneBreakdown {
        let h = crate::breakdown::parse_heading(heading);
        SceneBreakdown {
            scene: EntityId::from_key(&format!("cs:{n}")),
            number: n.to_string(),
            heading: heading.to_string(),
            int_ext: h.int_ext,
            location: h.location,
            time_of_day: h.time_of_day,
            cast: cast.iter().map(|s| s.to_string()).collect(),
            props: vec!["CAR".into()],
            extras_hint: None,
            page_eighths: eighths,
            est_minutes: 1.0,
            vehicles: true,
            animals: false,
            sfx: false,
            vfx: false,
        }
    }

    fn doc() -> Vec<SceneBreakdown> {
        vec![
            bd(1, "INT. KITCHEN - DAY", &["ANNA", "BEN"], 6),
            bd(2, "INT. KITCHEN - DAY", &["BEN", "CARLA"], 4),
            bd(3, "EXT. STREET - DAY", &["ANNA", "DAVE"], 8),
            bd(4, "EXT. STREET - NIGHT", &["ANNA"], 3),
        ]
    }

    #[test]
    fn cast_numbers_follow_first_appearance_and_are_stable() {
        let roster = CastRoster::from_breakdowns(&doc());
        assert_eq!(roster.number_of("ANNA"), Some(1));
        assert_eq!(roster.number_of("BEN"), Some(2));
        assert_eq!(roster.number_of("CARLA"), Some(3));
        assert_eq!(roster.number_of("DAVE"), Some(4));
        assert!(roster.member("ANNA").unwrap().is_lead);
        assert!(roster.member("BEN").unwrap().is_lead);
        assert!(roster.member("CARLA").unwrap().is_lead); // 1 of 4 = 25 %
        let again = CastRoster::from_breakdowns(&doc());
        assert_eq!(roster, again);
    }

    #[test]
    fn makeup_arithmetic_and_markdown() {
        let bds = doc();
        let mut roster = CastRoster::from_breakdowns(&bds);
        roster.set_lead("CARLA", false);
        let s = schedule(
            &bds,
            &ScheduleConstraints {
                max_pages_per_day: 10.0,
                ..Default::default()
            },
        );
        let day = &s.days[0];
        let sheet = build_call_sheet(day, &bds, &roster, Time::new(7, 0).unwrap());
        assert_eq!(sheet.day, 1);
        let anna = sheet.cast_calls.iter().find(|c| c.name == "ANNA").unwrap();
        assert_eq!(anna.makeup.hhmm(), "05:30");
        assert_eq!(anna.pickup.hhmm(), "05:00");
        assert_eq!(anna.on_set.hhmm(), "07:30");
        let carla = sheet.cast_calls.iter().find(|c| c.name == "CARLA").unwrap();
        assert_eq!(carla.makeup.hhmm(), "06:00");
        assert_eq!(carla.number, 3);
        // Sorted by cast number.
        let numbers: Vec<u32> = sheet.cast_calls.iter().map(|c| c.number).collect();
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        assert_eq!(numbers, sorted);
        let md = sheet.to_markdown();
        for sc in &day.strips {
            assert!(md.contains(&format!("| {} |", sc.number)), "{md}");
        }
        assert!(md.contains("General call: **07:00**"));
        assert!(md.contains("picture vehicles"));
        let hmu = sheet
            .crew_calls
            .iter()
            .find(|c| c.department.starts_with("Hair"))
            .unwrap();
        assert_eq!(hmu.call.hhmm(), "05:15");
    }
}
