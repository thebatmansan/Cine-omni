//! Screenplay statistics: page eighths, estimated running time, word lists.
//!
//! The constants follow the standard "one page equals one minute" industry
//! heuristics used by breakdown software:
//!
//! * [`LINES_PER_PAGE`] = 55 printed lines per page (12 pt Courier, 1" margins).
//! * [`ACTION_WRAP`] = 61 characters per action line (6" column).
//! * [`DIALOGUE_WRAP`] = 35 characters per dialogue line (3.5" column).
//! * [`PARENTHETICAL_WRAP`] = 25 characters per parenthetical line.
//! * [`DIALOGUE_WORDS_PER_SECOND`] = 2.6 spoken words per second (a measured
//!   average for screen dialogue, ~155 wpm).
//! * [`ACTION_BLOCK_SECONDS`] = 1.5 seconds of screen time per action
//!   paragraph (a "beat"); the scene heading itself counts as one beat.

use crate::model::{Element, SceneView};

/// Printed lines per page.
pub const LINES_PER_PAGE: u32 = 55;
/// Characters per line in the action column.
pub const ACTION_WRAP: usize = 61;
/// Characters per line in the dialogue column.
pub const DIALOGUE_WRAP: usize = 35;
/// Characters per line in the parenthetical column.
pub const PARENTHETICAL_WRAP: usize = 25;
/// Spoken words per second used for dialogue timing.
pub const DIALOGUE_WORDS_PER_SECOND: f64 = 2.6;
/// Seconds of screen time attributed to each action paragraph and heading.
pub const ACTION_BLOCK_SECONDS: f64 = 1.5;

/// Number of printed lines `text` occupies when greedily word-wrapped at
/// `width` characters. Explicit newlines start new lines; empty lines count.
pub fn wrapped_lines(text: &str, width: usize) -> u32 {
    let width = width.max(1);
    let mut lines = 0u32;
    for raw in text.split('\n') {
        let mut cur = 0usize;
        let mut any = false;
        for word in raw.split_whitespace() {
            let w = word.chars().count();
            if !any {
                cur = w;
                any = true;
                lines += 1;
            } else if cur + 1 + w <= width {
                cur += 1 + w;
            } else {
                cur = w;
                lines += 1;
            }
            // Very long tokens spill over additional lines.
            while cur > width {
                cur -= width;
                lines += 1;
            }
        }
        if !any {
            lines += 1;
        }
    }
    lines
}

/// Printed lines an element occupies, including the blank line that
/// separates paragraphs.
pub fn element_lines(el: &Element) -> u32 {
    match el {
        Element::SceneHeading { .. } | Element::Transition { .. } | Element::Centered { .. } => 2,
        Element::Action { text } => wrapped_lines(text, ACTION_WRAP) + 1,
        Element::Character { .. } => 1,
        Element::Parenthetical { text } => wrapped_lines(text, PARENTHETICAL_WRAP),
        Element::Dialogue { text } => wrapped_lines(text, DIALOGUE_WRAP) + 1,
        Element::Lyric { text } => wrapped_lines(text, DIALOGUE_WRAP) + 1,
        Element::Section { .. }
        | Element::Synopsis { .. }
        | Element::Note { .. }
        | Element::PageBreak
        | Element::Boneyard { .. } => 0,
    }
}

/// Total printed lines of a scene.
pub fn scene_lines(scene: &SceneView) -> u32 {
    scene.elements.iter().map(element_lines).sum()
}

/// Length of a scene in eighths of a page (rounded up, minimum 1).
pub fn page_eighths(scene: &SceneView) -> u32 {
    let lines = scene_lines(scene);
    let eighths = (lines * 8).div_ceil(LINES_PER_PAGE);
    eighths.max(1)
}

/// Total dialogue words (dialogue and lyric elements) in a scene.
pub fn dialogue_word_count(scene: &SceneView) -> usize {
    scene
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Dialogue { text } | Element::Lyric { text } => Some(word_list(text).len()),
            _ => None,
        })
        .sum()
}

/// Estimated screen time in seconds: dialogue words at
/// [`DIALOGUE_WORDS_PER_SECOND`] plus [`ACTION_BLOCK_SECONDS`] per action
/// paragraph (and one for the heading).
pub fn estimate_duration_seconds(scene: &SceneView) -> f64 {
    let words = dialogue_word_count(scene) as f64;
    let blocks = scene
        .elements
        .iter()
        .filter(|e| {
            matches!(
                e,
                Element::Action { .. } | Element::SceneHeading { .. } | Element::Centered { .. }
            )
        })
        .count() as f64;
    words / DIALOGUE_WORDS_PER_SECOND + blocks * ACTION_BLOCK_SECONDS
}

/// Lowercase words with punctuation stripped; apostrophes inside a word are
/// kept (`don't`), emphasis markers and brackets are removed.
pub fn word_list(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace())
        .filter_map(|tok| {
            let cleaned: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '’')
                .map(|c| if c == '’' { '\'' } else { c })
                .collect::<String>()
                .to_lowercase();
            let cleaned = cleaned.trim_matches('\'').to_string();
            if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Screenplay;

    #[test]
    fn words() {
        assert_eq!(
            word_list("Don't *stop*, Mary! 'Tis 42."),
            vec!["don't", "stop", "mary", "tis", "42"]
        );
        assert!(word_list("  ...  ").is_empty());
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrapped_lines("", 10), 1);
        assert_eq!(wrapped_lines("aaa bbb ccc", 7), 2);
        assert_eq!(wrapped_lines("a\n\nb", 10), 3);
        assert_eq!(wrapped_lines(&"x".repeat(25), 10), 3);
    }

    #[test]
    fn eighths_and_duration() {
        let sp = Screenplay {
            title_page: vec![],
            elements: vec![
                Element::scene_heading("INT. A - DAY", None),
                Element::Action {
                    text: "Short.".into(),
                },
                Element::Character {
                    name: "A".into(),
                    extension: None,
                    dual: false,
                },
                Element::Dialogue {
                    text: "one two three four five six seven eight nine ten eleven twelve thirteen"
                        .into(),
                },
            ],
        };
        let scenes = sp.scenes();
        // 2 + 2 + 1 + 4 = 9 printed lines, i.e. more than one eighth (55 / 8 = 6.875 lines).
        assert_eq!(page_eighths(&scenes[0]), 2);
        let d = estimate_duration_seconds(&scenes[0]);
        assert!((d - (13.0 / 2.6 + 3.0)).abs() < 1e-9);
        let long = Screenplay {
            title_page: vec![],
            elements: vec![
                Element::scene_heading("INT. A - DAY", None),
                Element::Action {
                    text: "word ".repeat(700),
                },
            ],
        };
        assert!(page_eighths(&long.scenes()[0]) > 8);
    }
}
