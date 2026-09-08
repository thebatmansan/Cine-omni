//! Fountain (plain-text screenplay) parser and writer.
//!
//! The parser follows the Fountain 1.1 syntax: a title page block, scene
//! headings (`INT.`/`EXT.`/`EST.`/`INT./EXT.`/`I/E.` or forced with `.`),
//! scene numbers (`#12A#`), action (forced with `!`), character cues (all
//! caps or forced with `@`, extensions in parentheses, `^` for dual
//! dialogue), parentheticals, dialogue, transitions (all caps ending in
//! `TO:` or forced with `>`), centered text (`> text <`), sections (`#`),
//! synopses (`=`), notes (`[[...]]`), boneyards (`/* ... */`), page breaks
//! (`===`), lyrics (`~`) and the two-space line break. Emphasis markers are
//! kept verbatim in the text.
//!
//! The writer emits canonical Fountain that re-parses to the same element
//! list; it adds forcing markers only when the plain form would be
//! misclassified.

use crate::model::{has_heading_prefix, split_character_cue, Element, Screenplay};
use regex::Regex;
use std::sync::OnceLock;

fn scene_number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s*#([^#\s][^#]*?)#\s*$").expect("valid scene number regex"))
}

/// A blank line for block-separation purposes. A line made of two or more
/// spaces is *not* blank: it is the Fountain hard line break.
fn is_blank(line: &str) -> bool {
    line.trim().is_empty() && line.chars().filter(|c| *c == ' ').count() < 2
}

/// Is this whitespace-only line the two-space line break marker?
fn is_break_marker(line: &str) -> bool {
    line.trim().is_empty() && !is_blank(line)
}

/// True when the text is all caps: it has at least one letter and no
/// lowercase letter.
pub fn is_all_caps(text: &str) -> bool {
    let mut has_alpha = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            has_alpha = true;
            if c.is_lowercase() {
                return false;
            }
        }
    }
    has_alpha
}

fn is_page_break(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '=')
}

fn is_transition_plain(line: &str) -> bool {
    let t = line.trim();
    is_all_caps(t) && t.ends_with("TO:")
}

fn is_centered(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('>') && t.ends_with('<')
}

fn is_forced_heading(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('.') && !t.starts_with("..") && t.len() > 1
}

fn is_parenthetical_line(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('(') && t.ends_with(')')
}

/// Classification of the first line of a paragraph (a line preceded by a
/// blank line or the start of the document).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Start {
    Action,
    ForcedAction,
    Heading,
    Character,
    Transition,
    Centered,
    Section,
    Synopsis,
    Note,
    Boneyard,
    PageBreak,
    Lyric,
}

fn classify_start(line: &str, next_nonblank: bool) -> Start {
    let t = line.trim();
    if t.is_empty() {
        return Start::Action;
    }
    if t.starts_with('!') {
        return Start::ForcedAction;
    }
    if t.starts_with("/*") {
        return Start::Boneyard;
    }
    if t.starts_with("[[") {
        return Start::Note;
    }
    if is_page_break(t) {
        return Start::PageBreak;
    }
    if t.starts_with('#') {
        return Start::Section;
    }
    if t.starts_with('=') {
        return Start::Synopsis;
    }
    if t.starts_with('~') {
        return Start::Lyric;
    }
    if is_centered(t) {
        return Start::Centered;
    }
    if t.starts_with('>') {
        return Start::Transition;
    }
    if t.starts_with('@') {
        return Start::Character;
    }
    if is_forced_heading(t) {
        return Start::Heading;
    }
    if has_heading_prefix(t) {
        return Start::Heading;
    }
    if is_transition_plain(t) && !next_nonblank {
        return Start::Transition;
    }
    if is_all_caps(t.trim_end_matches('^').trim()) && next_nonblank && !t.starts_with('(') {
        return Start::Character;
    }
    Start::Action
}

/// Strip trailing whitespace of content lines but keep the two-space break.
fn clean_line(raw: &str) -> String {
    let l = raw.trim_end_matches(['\r', '\n']);
    if is_break_marker(l) {
        return "  ".to_string();
    }
    l.trim_end().to_string()
}

/// Parse a title page. Returns `(pairs, index of first body line)`.
fn parse_title_page(lines: &[String]) -> (Vec<(String, String)>, usize) {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    // A title page must start on the very first line with `Key:`.
    let first = match lines.first() {
        Some(l) => l,
        None => return (pairs, 0),
    };
    if !looks_like_title_key(first) {
        return (pairs, 0);
    }
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) {
            i += 1;
            break;
        }
        if let Some(colon) = title_key_end(line) {
            let key = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            pairs.push((key, value));
        } else if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, v)) = pairs.last_mut() {
                if !v.is_empty() {
                    v.push('\n');
                }
                v.push_str(line.trim());
            }
        } else {
            // Not part of a title page after all: stop here.
            break;
        }
        i += 1;
    }
    (pairs, i)
}

fn title_key_end(line: &str) -> Option<usize> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty()
        || key.len() > 40
        || !key
            .chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '_' || c == '-')
    {
        return None;
    }
    Some(colon)
}

fn looks_like_title_key(line: &str) -> bool {
    title_key_end(line).is_some()
}

/// Parse Fountain text into a [`Screenplay`].
pub fn parse_fountain(input: &str) -> Screenplay {
    let lines: Vec<String> = input.lines().map(clean_line).collect();
    let (title_page, start) = parse_title_page(&lines);
    let mut elements = Vec::new();
    let mut i = start;
    let n = lines.len();
    let next_nonblank = |idx: usize| idx + 1 < n && !is_blank(&lines[idx + 1]);

    while i < n {
        let line = &lines[i];
        if is_blank(line) {
            i += 1;
            continue;
        }
        match classify_start(line, next_nonblank(i)) {
            Start::PageBreak => {
                elements.push(Element::PageBreak);
                i += 1;
            }
            Start::Section => {
                let t = line.trim();
                let level = t.chars().take_while(|c| *c == '#').count().clamp(1, 255) as u8;
                let text = t.trim_start_matches('#').trim().to_string();
                elements.push(Element::Section { level, text });
                i += 1;
            }
            Start::Synopsis => {
                let text = line.trim().trim_start_matches('=').trim().to_string();
                elements.push(Element::Synopsis { text });
                i += 1;
            }
            Start::Note => {
                let (text, next) = collect_delimited(&lines, i, "[[", "]]");
                elements.push(Element::Note { text });
                i = next;
            }
            Start::Boneyard => {
                let (text, next) = collect_delimited(&lines, i, "/*", "*/");
                elements.push(Element::Boneyard { text });
                i = next;
            }
            Start::Lyric => {
                let mut parts = Vec::new();
                while i < n && lines[i].trim_start().starts_with('~') {
                    parts.push(lines[i].trim().trim_start_matches('~').trim().to_string());
                    i += 1;
                }
                elements.push(Element::Lyric {
                    text: parts.join("\n"),
                });
            }
            Start::Centered => {
                let t = line.trim();
                let text = t[1..t.len() - 1].trim().to_string();
                elements.push(Element::Centered { text });
                i += 1;
            }
            Start::Transition => {
                let t = line.trim();
                let text = t.strip_prefix('>').unwrap_or(t).trim().to_string();
                elements.push(Element::Transition { text });
                i += 1;
            }
            Start::Heading => {
                let t = line.trim();
                let t = if is_forced_heading(t) {
                    t[1..].trim()
                } else {
                    t
                };
                let (text, scene_number) = split_scene_number(t);
                elements.push(Element::scene_heading(text, scene_number));
                i += 1;
            }
            Start::Character => {
                let t = line.trim();
                let t = t.strip_prefix('@').unwrap_or(t).trim();
                let (dual, cue) = match t.strip_suffix('^') {
                    Some(c) => (true, c.trim()),
                    None => (false, t),
                };
                let (name, extension) = split_character_cue(cue);
                elements.push(Element::Character {
                    name,
                    extension,
                    dual,
                });
                i += 1;
                // Dialogue block: until a blank line.
                let mut dialogue: Vec<String> = Vec::new();
                while i < n && !is_blank(&lines[i]) {
                    let l = &lines[i];
                    if is_break_marker(l) {
                        dialogue.push(String::new());
                        i += 1;
                        continue;
                    }
                    let t = l.trim();
                    if is_parenthetical_line(t) {
                        flush_dialogue(&mut elements, &mut dialogue);
                        elements.push(Element::Parenthetical {
                            text: t.to_string(),
                        });
                    } else if t.starts_with('~') {
                        flush_dialogue(&mut elements, &mut dialogue);
                        let mut parts = vec![t.trim_start_matches('~').trim().to_string()];
                        while i + 1 < n && lines[i + 1].trim_start().starts_with('~') {
                            i += 1;
                            parts.push(lines[i].trim().trim_start_matches('~').trim().to_string());
                        }
                        elements.push(Element::Lyric {
                            text: parts.join("\n"),
                        });
                    } else {
                        dialogue.push(t.to_string());
                    }
                    i += 1;
                }
                flush_dialogue(&mut elements, &mut dialogue);
            }
            Start::Action | Start::ForcedAction => {
                let mut parts: Vec<String> = Vec::new();
                let first = match line.find('!') {
                    Some(p) if line[..p].trim().is_empty() => &line[p + 1..],
                    _ => line.as_str(),
                };
                parts.push(first.to_string());
                i += 1;
                while i < n && !is_blank(&lines[i]) {
                    let l = &lines[i];
                    if is_break_marker(l) {
                        parts.push(String::new());
                    } else {
                        parts.push(l.to_string());
                    }
                    i += 1;
                }
                elements.push(Element::Action {
                    text: parts.join("\n"),
                });
            }
        }
    }
    Screenplay {
        title_page,
        elements,
    }
}

fn flush_dialogue(elements: &mut Vec<Element>, dialogue: &mut Vec<String>) {
    if dialogue.is_empty() {
        return;
    }
    let text = dialogue.join("\n");
    dialogue.clear();
    if text.trim().is_empty() {
        return;
    }
    elements.push(Element::Dialogue { text });
}

/// Collect a possibly multi-line delimited block starting at `start`.
/// Returns the inner text (trimmed) and the index of the next line.
fn collect_delimited(lines: &[String], start: usize, open: &str, close: &str) -> (String, usize) {
    let mut buf = String::new();
    let mut i = start;
    while i < lines.len() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(lines[i].trim());
        i += 1;
        if buf.contains(close) {
            break;
        }
    }
    let inner = buf.trim();
    let inner = inner.strip_prefix(open).unwrap_or(inner);
    let inner = inner.strip_suffix(close).unwrap_or(inner);
    (inner.trim().to_string(), i)
}

/// Split `INT. HOUSE - DAY #12A#` into `("INT. HOUSE - DAY", Some("12A"))`.
pub fn split_scene_number(heading: &str) -> (String, Option<String>) {
    match scene_number_re().captures(heading) {
        Some(c) => {
            let whole = c.get(0).map(|m| m.start()).unwrap_or(heading.len());
            (
                heading[..whole].trim().to_string(),
                Some(c[1].trim().to_string()),
            )
        }
        None => (heading.trim().to_string(), None),
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Serialize a [`Screenplay`] as Fountain text.
pub fn write_fountain(sp: &Screenplay) -> String {
    let mut out = String::new();
    if !sp.title_page.is_empty() {
        for (k, v) in &sp.title_page {
            if v.contains('\n') || v.is_empty() {
                out.push_str(k);
                out.push_str(":\n");
                for line in v.lines() {
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
            } else {
                out.push_str(&format!("{k}: {v}\n"));
            }
        }
        out.push('\n');
    }

    let mut prev_in_block = false;
    for (idx, el) in sp.elements.iter().enumerate() {
        let in_block_continuation = prev_in_block
            && matches!(
                el,
                Element::Parenthetical { .. } | Element::Dialogue { .. } | Element::Lyric { .. }
            );
        if idx > 0 && !in_block_continuation {
            out.push('\n');
        }
        match el {
            Element::SceneHeading {
                text, scene_number, ..
            } => {
                if !has_heading_prefix(text) {
                    out.push('.');
                }
                out.push_str(text);
                if let Some(n) = scene_number {
                    out.push_str(&format!(" #{n}#"));
                }
                out.push('\n');
            }
            Element::Action { text } => {
                let first = text.lines().next().unwrap_or("");
                // After the first line the writer emits either the rest of
                // the paragraph or a blank line, which decides whether an
                // all-caps first line would be read as a character cue.
                let next_nonblank = text.lines().count() > 1;
                let needs_force = first.trim_start().starts_with('!')
                    || classify_start(first, next_nonblank) != Start::Action;
                if needs_force {
                    out.push('!');
                }
                push_multiline(&mut out, text);
            }
            Element::Character {
                name,
                extension,
                dual,
            } => {
                let mut cue = name.clone();
                if let Some(e) = extension {
                    cue.push(' ');
                    cue.push_str(e);
                }
                let plain_ok = is_all_caps(name)
                    && !has_heading_prefix(&cue)
                    && !cue.starts_with('.')
                    && !cue.starts_with('!')
                    && !cue.starts_with('#')
                    && !cue.starts_with('=')
                    && !cue.starts_with('~')
                    && !cue.starts_with('>')
                    && !cue.starts_with('[')
                    && !cue.starts_with('/')
                    && !cue.starts_with('(')
                    && !(cue.ends_with("TO:"));
                if !plain_ok {
                    out.push('@');
                }
                out.push_str(&cue);
                if *dual {
                    out.push_str(" ^");
                }
                out.push('\n');
            }
            Element::Parenthetical { text } => {
                let t = text.trim();
                if t.starts_with('(') && t.ends_with(')') {
                    out.push_str(t);
                } else {
                    out.push_str(&format!("({t})"));
                }
                out.push('\n');
            }
            Element::Dialogue { text } => {
                push_multiline(&mut out, text);
            }
            Element::Transition { text } => {
                if !is_transition_plain(text) || is_centered(text) {
                    out.push_str("> ");
                }
                out.push_str(text.trim());
                out.push('\n');
            }
            Element::Centered { text } => {
                out.push_str(&format!("> {} <\n", text.trim()));
            }
            Element::Section { level, text } => {
                out.push_str(&"#".repeat((*level).max(1) as usize));
                out.push(' ');
                out.push_str(text.trim());
                out.push('\n');
            }
            Element::Synopsis { text } => {
                out.push_str(&format!("= {}\n", text.trim()));
            }
            Element::Note { text } => {
                if text.contains('\n') {
                    out.push_str(&format!("[[\n{}\n]]\n", text.trim()));
                } else {
                    out.push_str(&format!("[[{}]]\n", text.trim()));
                }
            }
            Element::Lyric { text } => {
                for line in text.lines() {
                    out.push('~');
                    out.push_str(line.trim());
                    out.push('\n');
                }
            }
            Element::PageBreak => out.push_str("===\n"),
            Element::Boneyard { text } => {
                if text.contains('\n') {
                    out.push_str(&format!("/*\n{}\n*/\n", text.trim()));
                } else {
                    out.push_str(&format!("/* {} */\n", text.trim()));
                }
            }
        }
        prev_in_block = matches!(
            el,
            Element::Character { .. } | Element::Parenthetical { .. } | Element::Dialogue { .. }
        ) || (prev_in_block && matches!(el, Element::Lyric { .. }));
    }
    out
}

/// Write text lines, turning empty inner lines into the two-space break so
/// the paragraph is not split on re-parse.
fn push_multiline(out: &mut String, text: &str) {
    let lines: Vec<&str> = text.split('\n').collect();
    for line in &lines {
        if line.trim().is_empty() {
            out.push_str("  ");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_numbers() {
        assert_eq!(
            split_scene_number("INT. HOUSE - DAY #12A#"),
            ("INT. HOUSE - DAY".into(), Some("12A".into()))
        );
        assert_eq!(
            split_scene_number("INT. HOUSE - DAY"),
            ("INT. HOUSE - DAY".into(), None)
        );
    }

    #[test]
    fn title_page_multiline_values() {
        let src = "Title:\n    THE FILM\n    a story\nAuthor: Me\n\nINT. A - DAY\n";
        let sp = parse_fountain(src);
        assert_eq!(
            sp.title_page,
            vec![
                ("Title".into(), "THE FILM\na story".into()),
                ("Author".into(), "Me".into())
            ]
        );
        assert_eq!(sp.elements.len(), 1);
        let back = parse_fountain(&write_fountain(&sp));
        assert_eq!(back, sp);
    }

    #[test]
    fn forced_elements_and_dual() {
        let src = "\
.ON THE ROOF

!BANG BANG
It echoes.

@McTEAGUE (V.O.)
Hello.

MARY
(beat)
Yes.

BOB ^
No.

> Burn to white.

> CENTERED <

CUT TO:

# Act
## Sequence
= summary
[[note]]
/* gone */
===
~la la
~di da
";
        let sp = parse_fountain(src);
        let kinds: Vec<&str> = sp.elements.iter().map(|e| e.kind_name()).collect();
        assert_eq!(
            kinds,
            vec![
                "scene_heading",
                "action",
                "character",
                "dialogue",
                "character",
                "parenthetical",
                "dialogue",
                "character",
                "dialogue",
                "transition",
                "centered",
                "transition",
                "section",
                "section",
                "synopsis",
                "note",
                "boneyard",
                "page_break",
                "lyric",
            ]
        );
        assert_eq!(
            sp.elements[1],
            Element::Action {
                text: "BANG BANG\nIt echoes.".into()
            }
        );
        assert_eq!(
            sp.elements[2],
            Element::Character {
                name: "McTEAGUE".into(),
                extension: Some("(V.O.)".into()),
                dual: false
            }
        );
        assert_eq!(
            sp.elements[7],
            Element::Character {
                name: "BOB".into(),
                extension: None,
                dual: true
            }
        );
        assert_eq!(
            sp.elements[13],
            Element::Section {
                level: 2,
                text: "Sequence".into()
            }
        );
        assert_eq!(
            sp.elements[18],
            Element::Lyric {
                text: "la la\ndi da".into()
            }
        );
        let back = parse_fountain(&write_fountain(&sp));
        assert_eq!(back.elements, sp.elements);
    }

    #[test]
    fn two_space_line_break_in_dialogue_and_action() {
        let src = "MARY\nOne.\n  \nTwo.\n\nAction one.\n  \nAction two.\n";
        let sp = parse_fountain(src);
        assert_eq!(
            sp.elements[1],
            Element::Dialogue {
                text: "One.\n\nTwo.".into()
            }
        );
        assert_eq!(
            sp.elements[2],
            Element::Action {
                text: "Action one.\n\nAction two.".into()
            }
        );
        let back = parse_fountain(&write_fountain(&sp));
        assert_eq!(back.elements, sp.elements);
    }

    #[test]
    fn caps_line_followed_by_blank_is_action() {
        let sp = parse_fountain("BOOM.\n\nThe hull shudders.\n");
        assert_eq!(
            sp.elements[0],
            Element::Action {
                text: "BOOM.".into()
            }
        );
        let back = parse_fountain(&write_fountain(&sp));
        assert_eq!(back.elements, sp.elements);
    }
}
