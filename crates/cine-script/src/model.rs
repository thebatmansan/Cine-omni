//! The in-memory screenplay model shared by every parser and writer.
//!
//! A [`Screenplay`] is a title page plus a flat, ordered list of
//! [`Element`]s. Scenes are not nested in the model; [`Screenplay::scenes`]
//! groups the flat list into [`SceneView`]s on demand so that parsers stay
//! simple and the element list round-trips exactly.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::sync::OnceLock;
use ucg_schema::{IntExt, SceneHeading};

/// A complete screenplay: title page pairs plus ordered elements.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Screenplay {
    /// Title page key/value pairs in document order (`("Title", "...")`).
    /// Values may contain newlines.
    pub title_page: Vec<(String, String)>,
    /// Every element of the body, in document order.
    pub elements: Vec<Element>,
}

/// One block of a screenplay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    /// `INT. KITCHEN - NIGHT #12A#`
    SceneHeading {
        /// The heading text without the scene number.
        text: String,
        /// Explicit scene number, if any (`12A`).
        scene_number: Option<String>,
        /// Structured parse of `text`.
        heading: SceneHeading,
    },
    /// Descriptive prose. Multi-line paragraphs keep `\n`; an empty inner
    /// line is a deliberate line break.
    Action { text: String },
    /// A character cue introducing a dialogue block.
    Character {
        /// Name as written (`MAREN`).
        name: String,
        /// Parenthesised extension(s) exactly as written (`(V.O.)`, `(CONT'D)`).
        extension: Option<String>,
        /// `true` when this cue is the right-hand column of a dual dialogue.
        dual: bool,
    },
    /// `(beat)` inside a dialogue block. Text includes the parentheses.
    Parenthetical { text: String },
    /// Spoken lines. Multi-line dialogue keeps `\n`.
    Dialogue { text: String },
    /// `CUT TO:`
    Transition { text: String },
    /// `> THE END <`
    Centered { text: String },
    /// `# Act One` (level = number of `#`).
    Section { level: u8, text: String },
    /// `= A summary line`
    Synopsis { text: String },
    /// `[[ a note ]]`
    Note { text: String },
    /// `~ a lyric line` (consecutive lines joined with `\n`).
    Lyric { text: String },
    /// `===`
    PageBreak,
    /// `/* commented out */`
    Boneyard { text: String },
}

impl Element {
    /// Short kind name, used for counting and diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Element::SceneHeading { .. } => "scene_heading",
            Element::Action { .. } => "action",
            Element::Character { .. } => "character",
            Element::Parenthetical { .. } => "parenthetical",
            Element::Dialogue { .. } => "dialogue",
            Element::Transition { .. } => "transition",
            Element::Centered { .. } => "centered",
            Element::Section { .. } => "section",
            Element::Synopsis { .. } => "synopsis",
            Element::Note { .. } => "note",
            Element::Lyric { .. } => "lyric",
            Element::PageBreak => "page_break",
            Element::Boneyard { .. } => "boneyard",
        }
    }

    /// The primary text of the element (empty for page breaks; the name for
    /// character cues).
    pub fn text(&self) -> &str {
        match self {
            Element::SceneHeading { text, .. }
            | Element::Action { text }
            | Element::Parenthetical { text }
            | Element::Dialogue { text }
            | Element::Transition { text }
            | Element::Centered { text }
            | Element::Section { text, .. }
            | Element::Synopsis { text }
            | Element::Note { text }
            | Element::Lyric { text }
            | Element::Boneyard { text } => text,
            Element::Character { name, .. } => name,
            Element::PageBreak => "",
        }
    }

    /// Convenience constructor that parses the heading text.
    pub fn scene_heading(text: impl Into<String>, scene_number: Option<String>) -> Element {
        let text = text.into();
        let heading = parse_scene_heading(&text);
        Element::SceneHeading {
            text,
            scene_number,
            heading,
        }
    }

    /// Is this element part of a dialogue block (cue, parenthetical, line, lyric)?
    pub fn is_dialogue_block(&self) -> bool {
        matches!(
            self,
            Element::Character { .. } | Element::Parenthetical { .. } | Element::Dialogue { .. }
        )
    }
}

/// A view over one scene of a [`Screenplay`]: the heading (if any) and the
/// elements that follow it up to the next heading.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneView<'a> {
    /// 0-based position among the screenplay's scenes.
    pub index: usize,
    /// Parsed heading; `None` for a headingless preamble scene.
    pub heading: Option<&'a SceneHeading>,
    /// The heading text (empty for a preamble).
    pub heading_text: &'a str,
    /// Explicit scene number, if any.
    pub number: Option<&'a str>,
    /// Range of `Screenplay::elements` covered by this scene (heading included).
    pub range: Range<usize>,
    /// The elements of the scene (heading included when present).
    pub elements: &'a [Element],
}

impl<'a> SceneView<'a> {
    /// Elements after the heading.
    pub fn body(&self) -> &'a [Element] {
        if self.heading.is_some() {
            &self.elements[1.min(self.elements.len())..]
        } else {
            self.elements
        }
    }

    /// Normalized character names (uppercase, extension stripped) in order of
    /// first appearance.
    pub fn characters(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.elements {
            if let Element::Character { name, .. } = e {
                let n = normalize_character_name(name);
                if !n.is_empty() && !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        out
    }

    /// `(speaker, line)` pairs for every dialogue element, speaker normalized.
    pub fn dialogue_lines(&self) -> Vec<(String, &'a str)> {
        let mut speaker = String::new();
        let mut out = Vec::new();
        for e in self.elements {
            match e {
                Element::Character { name, .. } => speaker = normalize_character_name(name),
                Element::Dialogue { text } => out.push((speaker.clone(), text.as_str())),
                _ => {}
            }
        }
        out
    }

    /// Stable key used by importers: the explicit number when present,
    /// otherwise the heading text plus the occurrence index supplied.
    pub fn stable_key(&self, occurrence: usize) -> String {
        match self.number {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => {
                if self.heading_text.is_empty() {
                    format!("(preamble)#{occurrence}")
                } else {
                    format!("{}#{occurrence}", self.heading_text)
                }
            }
        }
    }
}

impl Screenplay {
    /// Empty screenplay.
    pub fn new() -> Self {
        Self::default()
    }

    /// Value of a title page key, case-insensitively.
    pub fn title_value(&self, key: &str) -> Option<&str> {
        self.title_page
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The `Title:` entry (first line only), if present.
    pub fn title(&self) -> Option<&str> {
        self.title_value("title")
            .map(|t| t.lines().next().unwrap_or("").trim())
    }

    /// Group the flat element list into scenes. Elements that precede the
    /// first scene heading form a headingless preamble scene when present.
    pub fn scenes(&self) -> Vec<SceneView<'_>> {
        let mut starts: Vec<usize> = Vec::new();
        for (i, e) in self.elements.iter().enumerate() {
            if matches!(e, Element::SceneHeading { .. }) {
                starts.push(i);
            }
        }
        let mut out = Vec::new();
        let mut bounds: Vec<Range<usize>> = Vec::new();
        if starts.first().copied().unwrap_or(0) > 0
            || (starts.is_empty() && !self.elements.is_empty())
        {
            bounds.push(0..starts.first().copied().unwrap_or(self.elements.len()));
        }
        for (k, s) in starts.iter().enumerate() {
            let end = starts.get(k + 1).copied().unwrap_or(self.elements.len());
            bounds.push(*s..end);
        }
        for (index, range) in bounds.into_iter().enumerate() {
            let elements = &self.elements[range.clone()];
            let (heading, heading_text, number) = match elements.first() {
                Some(Element::SceneHeading {
                    text,
                    scene_number,
                    heading,
                }) => (Some(heading), text.as_str(), scene_number.as_deref()),
                _ => (None, "", None),
            };
            out.push(SceneView {
                index,
                heading,
                heading_text,
                number,
                range,
                elements,
            });
        }
        out
    }

    /// Count of elements of each kind, keyed by [`Element::kind_name`].
    pub fn counts(&self) -> indexmap::IndexMap<&'static str, usize> {
        let mut m = indexmap::IndexMap::new();
        for e in &self.elements {
            *m.entry(e.kind_name()).or_insert(0) += 1;
        }
        m
    }
}

/// Uppercase a character name and strip any parenthesised extension.
pub fn normalize_character_name(name: &str) -> String {
    let base = match name.find('(') {
        Some(p) => &name[..p],
        None => name,
    };
    base.trim().trim_end_matches('^').trim().to_uppercase()
}

/// Split a cue such as `MAREN (V.O.) (CONT'D)` into `("MAREN", Some("(V.O.) (CONT'D)"))`.
pub fn split_character_cue(cue: &str) -> (String, Option<String>) {
    let cue = cue.trim();
    match cue.find('(') {
        Some(p) => {
            let name = cue[..p].trim().to_string();
            let ext = cue[p..].trim().to_string();
            (name, if ext.is_empty() { None } else { Some(ext) })
        }
        None => (cue.to_string(), None),
    }
}

fn heading_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(INT\.?\s*/\s*EXT\.?|EXT\.?\s*/\s*INT\.?|I/E\.?|INT\.?|EXT\.?|EST\.?)(\s+|$)",
        )
        .expect("valid heading prefix regex")
    })
}

fn heading_sep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s+-{1,2}\s+").expect("valid heading separator regex"))
}

/// Words that mark the trailing segment of a heading as a time of day.
pub const TIME_OF_DAY_WORDS: &[&str] = &[
    "DAY",
    "NIGHT",
    "CONTINUOUS",
    "LATER",
    "DUSK",
    "DAWN",
    "MORNING",
    "EVENING",
    "AFTERNOON",
    "NOON",
    "MIDNIGHT",
    "SUNSET",
    "SUNRISE",
    "SAME",
    "MOMENTS",
    "TWILIGHT",
    "PRESENT",
    "FLASHBACK",
    "YEARS",
    "MAGIC HOUR",
];

/// Does the Fountain line start with a scene heading prefix (`INT.`, `EXT.`, ...)?
pub fn has_heading_prefix(line: &str) -> bool {
    heading_prefix_re().is_match(line.trim_start())
}

/// Parse `INT. KITCHEN - NIGHT` into its structured parts. Unknown prefixes
/// yield [`IntExt::Other`]; a trailing segment is only treated as a time of
/// day when it contains a known time word.
pub fn parse_scene_heading(raw: &str) -> SceneHeading {
    let trimmed = raw.trim();
    let (int_ext, rest) = match heading_prefix_re().find(trimmed) {
        Some(m) => {
            let p = m.as_str().trim().to_uppercase();
            let ie = if p.contains('/') {
                IntExt::IntExt
            } else if p.starts_with("INT") {
                IntExt::Int
            } else {
                // EXT and EST (establishing) are both exteriors.
                IntExt::Ext
            };
            (ie, trimmed[m.end()..].trim())
        }
        None => (IntExt::Other, trimmed),
    };
    let segments: Vec<&str> = heading_sep_re()
        .split(rest)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let (location, time_of_day) = match segments.len() {
        0 => (String::new(), String::new()),
        1 => (segments[0].to_string(), String::new()),
        n => {
            let last = segments[n - 1];
            if looks_like_time(last) {
                (segments[..n - 1].join(" - "), last.to_uppercase())
            } else {
                (segments.join(" - "), String::new())
            }
        }
    };
    SceneHeading {
        int_ext,
        location,
        time_of_day,
        raw: trimmed.to_string(),
    }
}

fn looks_like_time(segment: &str) -> bool {
    let up = segment.to_uppercase();
    TIME_OF_DAY_WORDS.iter().any(|w| {
        up.split(|c: char| !c.is_alphanumeric())
            .any(|tok| tok == *w)
            || up == *w
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings() {
        let h = parse_scene_heading("INT. KITCHEN - NIGHT");
        assert_eq!(h.int_ext, IntExt::Int);
        assert_eq!(h.location, "KITCHEN");
        assert_eq!(h.time_of_day, "NIGHT");

        let h = parse_scene_heading("INT./EXT. CAR -- MOVING - CONTINUOUS");
        assert_eq!(h.int_ext, IntExt::IntExt);
        assert_eq!(h.location, "CAR - MOVING");
        assert_eq!(h.time_of_day, "CONTINUOUS");

        let h = parse_scene_heading("I/E ALBATROSS - WHEELHOUSE");
        assert_eq!(h.int_ext, IntExt::IntExt);
        assert_eq!(h.location, "ALBATROSS - WHEELHOUSE");
        assert_eq!(h.time_of_day, "");

        let h = parse_scene_heading("EST. CITY SKYLINE - DAWN");
        assert_eq!(h.int_ext, IntExt::Ext);
        assert_eq!(h.location, "CITY SKYLINE");

        let h = parse_scene_heading("ON THE WATER - LATER");
        assert_eq!(h.int_ext, IntExt::Other);
        assert_eq!(h.location, "ON THE WATER");
        assert_eq!(h.time_of_day, "LATER");
        assert_eq!(h.raw, "ON THE WATER - LATER");
    }

    #[test]
    fn splits_cues() {
        assert_eq!(
            split_character_cue("MAREN (V.O.) (CONT'D)"),
            ("MAREN".into(), Some("(V.O.) (CONT'D)".into()))
        );
        assert_eq!(split_character_cue("ELIAS"), ("ELIAS".into(), None));
        assert_eq!(normalize_character_name("McAllister (V.O.)"), "MCALLISTER");
    }

    #[test]
    fn scenes_group_with_preamble() {
        let sp = Screenplay {
            title_page: vec![],
            elements: vec![
                Element::Section {
                    level: 1,
                    text: "ACT".into(),
                },
                Element::scene_heading("INT. A - DAY", None),
                Element::Action { text: "x".into() },
                Element::scene_heading("INT. B - DAY", Some("2".into())),
            ],
        };
        let scenes = sp.scenes();
        assert_eq!(scenes.len(), 3);
        assert!(scenes[0].heading.is_none());
        assert_eq!(scenes[1].range, 1..3);
        assert_eq!(scenes[2].number, Some("2"));
        assert_eq!(scenes[2].stable_key(0), "2");
        assert_eq!(scenes[1].stable_key(0), "INT. A - DAY#0");
    }
}
