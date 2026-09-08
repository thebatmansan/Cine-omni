//! Automatic breakdown tagging: characters, locations and props.
//!
//! * Characters come from dialogue cues, normalized (extension stripped,
//!   trimmed, upper-cased).
//! * Locations come from the parsed scene headings.
//! * Props come from action text: runs of ALL-CAPS words (length >= 3) that
//!   are not character names or screenplay vocabulary, plus a lexicon of
//!   common props matched case-insensitively as whole words.
//!
//! All lists are ordered by first appearance so results are deterministic.

use crate::model::{normalize_character_name, Element, Screenplay};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use ucg_schema::IntExt;

/// A character with the scenes it speaks in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterTag {
    /// Normalized name (`MAREN`).
    pub name: String,
    /// Scene indexes (into `Screenplay::scenes()`) in which the character speaks.
    pub scenes: Vec<usize>,
    /// Number of dialogue elements spoken.
    pub line_count: usize,
}

/// A location from scene headings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationTag {
    /// Upper-cased location text (`HARBOR OFFICE`).
    pub name: String,
    pub int_ext: IntExt,
    pub scenes: Vec<usize>,
}

/// A prop mentioned in action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropTag {
    /// Upper-cased prop name (`SATCHEL`).
    pub name: String,
    pub scenes: Vec<usize>,
}

/// Breakdown tags for a whole screenplay.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tags {
    pub characters: Vec<CharacterTag>,
    pub locations: Vec<LocationTag>,
    pub props: Vec<PropTag>,
}

/// Common props recognised in action text regardless of capitalisation.
pub const PROP_LEXICON: &[&str] = &[
    "gun",
    "pistol",
    "revolver",
    "rifle",
    "shotgun",
    "knife",
    "blade",
    "sword",
    "phone",
    "telephone",
    "cellphone",
    "letter",
    "envelope",
    "note",
    "key",
    "keys",
    "car",
    "truck",
    "bike",
    "bicycle",
    "motorcycle",
    "briefcase",
    "suitcase",
    "satchel",
    "bag",
    "backpack",
    "wallet",
    "purse",
    "watch",
    "clock",
    "camera",
    "laptop",
    "computer",
    "radio",
    "flashlight",
    "torch",
    "lamp",
    "lantern",
    "candle",
    "cigarette",
    "lighter",
    "matches",
    "bottle",
    "glass",
    "cup",
    "mug",
    "coffee",
    "book",
    "newspaper",
    "map",
    "photograph",
    "photo",
    "painting",
    "ring",
    "necklace",
    "badge",
    "handcuffs",
    "rope",
    "chain",
    "hammer",
    "wrench",
    "axe",
    "shovel",
    "umbrella",
    "hat",
    "coat",
    "jacket",
    "mask",
    "helmet",
    "sunglasses",
    "tape",
    "syringe",
    "pills",
    "money",
    "cash",
    "coin",
    "coins",
    "ticket",
    "passport",
    "document",
    "documents",
    "file",
    "folder",
    "box",
    "crate",
    "chest",
    "stamp",
    "manifest",
    "manifests",
    "handset",
    "gauge",
    "gauges",
    "throttle",
];

/// Words that appear in ALL CAPS in action lines but are not props.
const CAPS_STOPWORDS: &[&str] = &[
    "INT",
    "EXT",
    "EST",
    "THE",
    "AND",
    "BUT",
    "FOR",
    "NOT",
    "WITH",
    "FROM",
    "INTO",
    "ONTO",
    "THEN",
    "THAN",
    "THIS",
    "THAT",
    "CONT'D",
    "CONTD",
    "V.O",
    "O.S",
    "O.C",
    "DAY",
    "NIGHT",
    "LATER",
    "CONTINUOUS",
    "CUT",
    "FADE",
    "DISSOLVE",
    "SMASH",
    "MATCH",
    "TITLE",
    "SUPER",
    "CLOSE",
    "ANGLE",
    "POV",
    "WIDE",
    "INSERT",
    "BACK",
    "END",
    "BEGIN",
    "MONTAGE",
    "SERIES",
    "SHOTS",
    "INTERCUT",
    "BOOM",
    "BANG",
    "BANGS",
    "CRASH",
    "CRASHES",
    "SLAM",
    "SLAMS",
    "WHAM",
    "CRACK",
    "CRACKS",
    "THUD",
    "THUDS",
    "CLICK",
    "CLICKS",
    "RING",
    "RINGS",
    "SCREAM",
    "SHOUTS",
    "SHOUT",
    "HONKS",
    "ROARS",
    "SCREAMS",
    "SILENCE",
    "BEAT",
    "PAUSE",
    "OFF",
    "ON",
    "OVER",
    "UNDER",
    "OUT",
    "WE",
    "SEE",
    "HEAR",
    "HIS",
    "HER",
    "HIM",
    "SHE",
    "THEY",
    "THEM",
    "YOU",
    "ARE",
    "WAS",
    "HAS",
    "HAD",
    "ONE",
    "TWO",
    "ALL",
    "ANY",
    "YES",
    "NO",
    "MORE",
    "LESS",
    "VERY",
    "EVEN",
    "ONLY",
    "JUST",
    "NOW",
    "HERE",
    "THERE",
    "WHERE",
    "WHEN",
    "WHAT",
    "WHO",
    "HOW",
    "WHY",
    "MOMENTS",
    "SAME",
    "TIME",
    "STOP",
    "GO",
    "RUN",
    "WAIT",
    "LOOK",
    "LOOKS",
    "TURNS",
    "TURN",
    "DEAD",
    "GONE",
];

fn push_scene(v: &mut Vec<usize>, s: usize) {
    if v.last() != Some(&s) && !v.contains(&s) {
        v.push(s);
    }
}

/// Compute breakdown tags for a screenplay.
pub fn auto_tag(sp: &Screenplay) -> Tags {
    let mut tags = Tags::default();
    let scenes = sp.scenes();

    // Characters first: they are excluded from prop detection.
    for scene in &scenes {
        let mut speaker: Option<String> = None;
        for e in scene.elements {
            match e {
                Element::Character { name, .. } => {
                    let n = normalize_character_name(name);
                    if n.is_empty() {
                        speaker = None;
                        continue;
                    }
                    if !tags.characters.iter().any(|c| c.name == n) {
                        tags.characters.push(CharacterTag {
                            name: n.clone(),
                            scenes: vec![],
                            line_count: 0,
                        });
                    }
                    if let Some(c) = tags.characters.iter_mut().find(|c| c.name == n) {
                        push_scene(&mut c.scenes, scene.index);
                    }
                    speaker = Some(n);
                }
                Element::Dialogue { .. } => {
                    if let Some(n) = &speaker {
                        if let Some(c) = tags.characters.iter_mut().find(|c| c.name == *n) {
                            c.line_count += 1;
                        }
                    }
                }
                Element::Parenthetical { .. } | Element::Lyric { .. } => {}
                _ => speaker = None,
            }
        }
    }
    let character_names: HashSet<String> = tags.characters.iter().map(|c| c.name.clone()).collect();
    let character_words: HashSet<String> = character_names
        .iter()
        .flat_map(|n| n.split_whitespace().map(str::to_string))
        .collect();

    // Locations.
    for scene in &scenes {
        if let Some(h) = scene.heading {
            let name = h.location.trim().to_uppercase();
            if name.is_empty() {
                continue;
            }
            match tags
                .locations
                .iter_mut()
                .find(|l| l.name == name && l.int_ext == h.int_ext)
            {
                Some(l) => push_scene(&mut l.scenes, scene.index),
                None => tags.locations.push(LocationTag {
                    name,
                    int_ext: h.int_ext,
                    scenes: vec![scene.index],
                }),
            }
        }
    }

    // Props.
    let lexicon: HashSet<&str> = PROP_LEXICON.iter().copied().collect();
    let stop: HashSet<&str> = CAPS_STOPWORDS.iter().copied().collect();
    for scene in &scenes {
        for e in scene.elements {
            let Element::Action { text } = e else {
                continue;
            };
            for phrase in caps_phrases(text) {
                if character_names.contains(&phrase) {
                    continue;
                }
                let words: Vec<&str> = phrase.split_whitespace().collect();
                let all_stop = words
                    .iter()
                    .all(|w| stop.contains(w) || character_words.contains(*w));
                if all_stop {
                    continue;
                }
                // Drop leading/trailing stopwords or character words from the phrase.
                let trimmed: Vec<&str> = {
                    let mut ws = words.clone();
                    while ws
                        .first()
                        .is_some_and(|w| stop.contains(w) || character_words.contains(*w))
                    {
                        ws.remove(0);
                    }
                    while ws
                        .last()
                        .is_some_and(|w| stop.contains(w) || character_words.contains(*w))
                    {
                        ws.pop();
                    }
                    ws
                };
                if trimmed.is_empty() {
                    continue;
                }
                add_prop(&mut tags.props, trimmed.join(" "), scene.index);
            }
            for tok in text.split(|c: char| !c.is_alphanumeric() && c != '\'') {
                let lower = tok.trim_matches('\'').to_lowercase();
                if !lower.is_empty() && lexicon.contains(lower.as_str()) {
                    let up = lower.to_uppercase();
                    if !character_names.contains(&up) {
                        add_prop(&mut tags.props, up, scene.index);
                    }
                }
            }
        }
    }
    tags
}

fn add_prop(props: &mut Vec<PropTag>, name: String, scene: usize) {
    match props.iter_mut().find(|p| p.name == name) {
        Some(p) => push_scene(&mut p.scenes, scene),
        None => props.push(PropTag {
            name,
            scenes: vec![scene],
        }),
    }
}

/// Runs of consecutive ALL-CAPS words (>= 3 letters each) in `text`, in
/// order. Punctuation between words breaks a run.
pub fn caps_phrases(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let flush = |run: &mut Vec<String>, out: &mut Vec<String>| {
        if !run.is_empty() {
            out.push(run.join(" "));
            run.clear();
        }
    };
    for raw in text.split_whitespace() {
        let core: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '\'')
            .collect();
        let letters = core.chars().filter(|c| c.is_alphabetic()).count();
        let is_caps = letters >= 3 && core.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
        if is_caps {
            run.push(core);
        } else {
            flush(&mut run, &mut out);
        }
        // Trailing punctuation ends a phrase ("SATCHEL, then" / "KEY.").
        if raw.ends_with(|c: char| !c.is_alphanumeric() && c != '\'') {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fountain::parse_fountain;

    #[test]
    fn caps_runs() {
        assert_eq!(
            caps_phrases("She grabs the BRASS KEY. A GUN, and JO leaves."),
            vec!["BRASS KEY", "GUN"]
        );
    }

    #[test]
    fn tags_small_script() {
        let sp = parse_fountain(
            "INT. SHED - DAY\n\nMAREN grabs a RUSTY WRENCH and a phone.\n\nMAREN (V.O.)\nHi.\n\nEXT. SHED - NIGHT\n\nA gun.\n\nMAREN\nBye.\n\nBOB\nOk.\n",
        );
        let t = auto_tag(&sp);
        assert_eq!(t.characters.len(), 2);
        assert_eq!(t.characters[0].name, "MAREN");
        assert_eq!(t.characters[0].scenes, vec![0, 1]);
        assert_eq!(t.characters[0].line_count, 2);
        assert_eq!(t.locations.len(), 2);
        assert_eq!(t.locations[0].name, "SHED");
        assert_eq!(t.locations[0].int_ext, IntExt::Int);
        let names: Vec<&str> = t.props.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["RUSTY WRENCH", "WRENCH", "PHONE", "GUN"]);
        assert_eq!(t.props[3].scenes, vec![1]);
    }
}
