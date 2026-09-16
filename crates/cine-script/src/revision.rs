//! Revisions: coloured pages, and the marks that say what changed.
//!
//! Once a script is distributed, it stops being a document and becomes a
//! contract with a crew. Changes after that are issued as *revised pages* on
//! coloured paper, in a fixed order the industry has used for decades, with an
//! asterisk in the margin against every line that moved. Everybody on the
//! production can then tell at a glance whether the page in their hand is the
//! one being shot.
//!
//! This models it the only way that survives a live system: **a revision is a
//! snapshot**. Issuing one records what every element said at that moment; the
//! marks are then the difference between the script now and that snapshot,
//! computed on demand. Nothing is stamped onto the elements themselves, so an
//! edit and an undo both do the right thing without bookkeeping, and the marks
//! for an older revision can still be recovered from its snapshot.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The order revised pages are issued in.
///
/// White is the original. After goldenrod a production starts again at
/// "second blue" and so on; [`Colour::next`] says so rather than pretending
/// the list is endless.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colour {
    White,
    Blue,
    Pink,
    Yellow,
    Green,
    Goldenrod,
    Salmon,
    Cherry,
    Buff,
    Tan,
}

impl Colour {
    /// The standard order, first to last.
    pub const ORDER: [Colour; 10] = [
        Colour::White,
        Colour::Blue,
        Colour::Pink,
        Colour::Yellow,
        Colour::Green,
        Colour::Goldenrod,
        Colour::Salmon,
        Colour::Cherry,
        Colour::Buff,
        Colour::Tan,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Colour::White => "White",
            Colour::Blue => "Blue",
            Colour::Pink => "Pink",
            Colour::Yellow => "Yellow",
            Colour::Green => "Green",
            Colour::Goldenrod => "Goldenrod",
            Colour::Salmon => "Salmon",
            Colour::Cherry => "Cherry",
            Colour::Buff => "Buff",
            Colour::Tan => "Tan",
        }
    }

    /// The colour after this one, and how many times round the list that is.
    /// `(Blue, 2)` prints as "Second Blue", which is what a production calls
    /// the eleventh revision.
    pub fn next(after: Option<(Colour, u32)>) -> (Colour, u32) {
        let Some((colour, round)) = after else {
            return (Colour::White, 1);
        };
        let at = Colour::ORDER.iter().position(|c| *c == colour).unwrap_or(0);
        if at + 1 < Colour::ORDER.len() {
            (Colour::ORDER[at + 1], round)
        } else {
            (Colour::ORDER[1], round + 1)
        }
    }

    /// How a production says it: "Blue", "Second Blue", "Third Pink".
    pub fn spoken(&self, round: u32) -> String {
        let ordinal = match round {
            0 | 1 => return self.name().to_string(),
            2 => "Second",
            3 => "Third",
            4 => "Fourth",
            5 => "Fifth",
            _ => return format!("Revision {round} {}", self.name()),
        };
        format!("{ordinal} {}", self.name())
    }
}

/// What the script said when a revision was issued.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Element entity id → its text at that moment.
    pub text: BTreeMap<String, String>,
    /// Scene entity id → its heading, so a re-headed scene is marked too.
    #[serde(default)]
    pub headings: BTreeMap<String, String>,
}

/// One issued revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    pub colour: Colour,
    /// How many times round the colour list this is (1 the first time).
    pub round: u32,
    /// ISO date the pages went out.
    pub date: String,
    /// What a production office writes on the title page, if anything.
    #[serde(default)]
    pub note: String,
    pub snapshot: Snapshot,
}

impl Revision {
    /// The label printed on the page: "Blue — 2026-01-12".
    pub fn label(&self) -> String {
        format!("{} — {}", self.colour.spoken(self.round), self.date)
    }
}

/// What changed in one element since a revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Text differs from the snapshot.
    Changed,
    /// Not in the snapshot at all: written since.
    Added,
}

/// Which elements carry a revision mark, against the newest revision.
///
/// An element that is gone since the snapshot cannot be marked — there is
/// nothing left to put an asterisk against — so removals are reported
/// separately by [`removed_since`], which a production office uses to know a
/// page must be reissued even though nothing on it is starred.
pub fn marks(current: &Snapshot, revision: &Revision) -> BTreeMap<String, Mark> {
    let mut out = BTreeMap::new();
    for (id, text) in &current.text {
        match revision.snapshot.text.get(id) {
            None => {
                out.insert(id.clone(), Mark::Added);
            }
            Some(was) if was != text => {
                out.insert(id.clone(), Mark::Changed);
            }
            Some(_) => {}
        }
    }
    for (id, heading) in &current.headings {
        match revision.snapshot.headings.get(id) {
            None => {
                out.insert(id.clone(), Mark::Added);
            }
            Some(was) if was != heading => {
                out.insert(id.clone(), Mark::Changed);
            }
            Some(_) => {}
        }
    }
    out
}

/// Elements in the revision that are no longer in the script.
pub fn removed_since(current: &Snapshot, revision: &Revision) -> Vec<String> {
    revision
        .snapshot
        .text
        .keys()
        .chain(revision.snapshot.headings.keys())
        .filter(|id| !current.text.contains_key(*id) && !current.headings.contains_key(*id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(pairs: &[(&str, &str)]) -> Snapshot {
        Snapshot {
            text: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            headings: BTreeMap::new(),
        }
    }

    #[test]
    fn the_colours_run_in_the_order_a_production_uses_them() {
        assert_eq!(Colour::next(None), (Colour::White, 1));
        assert_eq!(Colour::next(Some((Colour::White, 1))), (Colour::Blue, 1));
        assert_eq!(Colour::next(Some((Colour::Blue, 1))), (Colour::Pink, 1));
        // After the last colour it starts again, and says which time round.
        assert_eq!(Colour::next(Some((Colour::Tan, 1))), (Colour::Blue, 2));
        assert_eq!(Colour::Blue.spoken(2), "Second Blue");
        assert_eq!(Colour::Blue.spoken(1), "Blue");
    }

    #[test]
    fn a_mark_is_the_difference_from_the_snapshot_not_a_stamp() {
        let issued = Revision {
            colour: Colour::Blue,
            round: 1,
            date: "2026-01-12".into(),
            note: String::new(),
            snapshot: snapshot(&[("a", "We ride at dawn."), ("b", "He says nothing.")]),
        };
        // Nothing has changed yet.
        let now = snapshot(&[("a", "We ride at dawn."), ("b", "He says nothing.")]);
        assert!(marks(&now, &issued).is_empty());

        // One line rewritten, one line new.
        let now = snapshot(&[
            ("a", "We ride at first light."),
            ("b", "He says nothing."),
            ("c", "She looks back."),
        ]);
        let m = marks(&now, &issued);
        assert_eq!(m.get("a"), Some(&Mark::Changed));
        assert_eq!(m.get("b"), None, "an untouched line carries no mark");
        assert_eq!(m.get("c"), Some(&Mark::Added));

        // Edited back to what it said: the mark goes, because the page is the
        // page that was distributed.
        let now = snapshot(&[("a", "We ride at dawn."), ("b", "He says nothing.")]);
        assert!(
            marks(&now, &issued).is_empty(),
            "an undone change is not a change"
        );

        // A cut line cannot be starred, so it is reported as removed.
        let now = snapshot(&[("a", "We ride at dawn.")]);
        assert_eq!(removed_since(&now, &issued), vec!["b".to_string()]);
    }
}
