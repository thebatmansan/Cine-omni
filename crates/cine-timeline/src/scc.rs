//! SCC: CEA-608 pop-on captions in the Scenarist file format.
//!
//! Broadcast in North America still delivers line-21 captions, and SCC is how
//! they travel: a list of 29.97 fps drop-frame timecodes, each followed by the
//! byte pairs a caption encoder transmits, one pair per frame. Everything that
//! makes SCC awkward is inherited from that wire:
//!
//! * **A caption appears when its End Of Caption code arrives**, not at the
//!   line's timecode. A pop-on caption is loaded off screen first — Resume
//!   Caption Loading, Erase Non-displayed Memory, a Preamble Address Code per
//!   row, the text — and shown by EOC. So a line starts as many frames before
//!   the caption should appear as there are pairs ahead of its EOC, and the
//!   writer computes that rather than stamping the display time on the line.
//! * **The picture is 32 columns by 15 rows.** A row longer than 32 characters
//!   cannot be shown; it is refused, naming the caption, rather than truncated.
//!   Bottom captions use rows 14–15 (or 12–15 for up to four lines), top ones
//!   rows 1–4.
//! * **Characters come from 608's own sets.** Most ASCII maps directly, a few
//!   ASCII codes are accented letters instead, and the rest — curly quotes,
//!   dashes, braces, most accents — are two-byte *special* or *extended*
//!   characters. An extended character is preceded by a basic fallback that a
//!   decoder replaces, as 608 requires. A character with no 608 form at all is
//!   refused.
//! * **Time is 29.97 fps drop-frame.** A caption track at another rate is
//!   re-quantised onto 30000/1001 frames, and [`SccReport`] says by how much.
//!
//! Every byte has odd parity. Control codes are sent twice, as encoders do, so
//! a lost pair does not lose the caption.
//!
//! The writer is checked against ttconv, an independent SCC reader, in
//! `captions_oracle.rs`: timing and every mapped character.

use crate::captions::{Caption, CaptionTrack, Placement};
use crate::error::{Error, Result};
use ucg_core::{Rate, RationalTime, TimeRange};

/// 29.97 fps: what SCC is timed in.
pub fn scc_rate() -> Rate {
    Rate::new(30000, 1001).expect("30000/1001")
}

/// What writing SCC did to the track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SccReport {
    pub captions: usize,
    /// Times moved onto the 29.97 frame grid, and the largest move in
    /// milliseconds. Zero for a track already at 30000/1001.
    pub requantised: usize,
    pub largest_move_ms: f64,
    /// Captions whose load had to start before 00:00:00;00 and so appear a few
    /// frames late.
    pub clamped_at_start: usize,
    /// Captions that follow the previous one so closely that their load could
    /// not begin until its line had been sent, and so appear late — and by how
    /// many frames at most.
    pub late: usize,
    pub latest_frames: i64,
}

fn with_parity(b: u8) -> u8 {
    let b = b & 0x7F;
    if b.count_ones().is_multiple_of(2) {
        b | 0x80
    } else {
        b
    }
}

fn pair(a: u8, b: u8) -> String {
    format!("{:02x}{:02x}", with_parity(a), with_parity(b))
}

const RCL: (u8, u8) = (0x14, 0x20);
const ENM: (u8, u8) = (0x14, 0x2E);
const EOC: (u8, u8) = (0x14, 0x2F);
const EDM: (u8, u8) = (0x14, 0x2C);

/// First byte and second-byte base of the Preamble Address Code for a row.
fn pac_row(row: u8) -> (u8, u8) {
    match row {
        1 => (0x11, 0x40),
        2 => (0x11, 0x60),
        3 => (0x12, 0x40),
        4 => (0x12, 0x60),
        5 => (0x15, 0x40),
        6 => (0x15, 0x60),
        7 => (0x16, 0x40),
        8 => (0x16, 0x60),
        9 => (0x17, 0x40),
        10 => (0x17, 0x60),
        11 => (0x10, 0x40),
        12 => (0x13, 0x40),
        13 => (0x13, 0x60),
        14 => (0x14, 0x40),
        _ => (0x14, 0x60),
    }
}

/// How one character goes on the wire.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Glyph {
    Basic(u8),
    /// 0x11 special character.
    Special(u8),
    /// 0x12 or 0x13 extended character, with the basic fallback sent first.
    Extended(u8, u8, u8),
}

/// Basic-set codes that are not the ASCII character of the same code.
const BASIC_NON_ASCII: &[(u8, char)] = &[
    (0x2A, 'á'),
    (0x5C, 'é'),
    (0x5E, 'í'),
    (0x5F, 'ó'),
    (0x60, 'ú'),
    (0x7B, 'ç'),
    (0x7C, '÷'),
    (0x7D, 'Ñ'),
    (0x7E, 'ñ'),
    (0x7F, '█'),
];

const SPECIAL: &[(u8, char)] = &[
    (0x30, '®'),
    (0x31, '°'),
    (0x32, '½'),
    (0x33, '¿'),
    (0x34, '™'),
    (0x35, '¢'),
    (0x36, '£'),
    (0x37, '♪'),
    (0x38, 'à'),
    (0x3A, 'è'),
    (0x3B, 'â'),
    (0x3C, 'ê'),
    (0x3D, 'î'),
    (0x3E, 'ô'),
    (0x3F, 'û'),
];

/// Extended characters: (first byte, second byte, character, basic fallback).
const EXTENDED: &[(u8, u8, char, char)] = &[
    (0x12, 0x20, 'Á', 'A'),
    (0x12, 0x21, 'É', 'E'),
    (0x12, 0x22, 'Ó', 'O'),
    (0x12, 0x23, 'Ú', 'U'),
    (0x12, 0x24, 'Ü', 'U'),
    (0x12, 0x25, 'ü', 'u'),
    (0x12, 0x26, '‘', '\''),
    (0x12, 0x27, '¡', '!'),
    (0x12, 0x28, '*', '\''),
    (0x12, 0x29, '’', '\''),
    (0x12, 0x2A, '—', '-'),
    (0x12, 0x2B, '©', 'c'),
    (0x12, 0x2C, '℠', 's'),
    (0x12, 0x2D, '•', '.'),
    (0x12, 0x2E, '“', '"'),
    (0x12, 0x2F, '”', '"'),
    (0x12, 0x30, 'À', 'A'),
    (0x12, 0x31, 'Â', 'A'),
    (0x12, 0x32, 'Ç', 'C'),
    (0x12, 0x33, 'È', 'E'),
    (0x12, 0x34, 'Ê', 'E'),
    (0x12, 0x35, 'Ë', 'E'),
    (0x12, 0x36, 'ë', 'e'),
    (0x12, 0x37, 'Î', 'I'),
    (0x12, 0x38, 'Ï', 'I'),
    (0x12, 0x39, 'ï', 'i'),
    (0x12, 0x3A, 'Ô', 'O'),
    (0x12, 0x3B, 'Ù', 'U'),
    (0x12, 0x3C, 'ù', 'u'),
    (0x12, 0x3D, 'Û', 'U'),
    (0x12, 0x3E, '«', '"'),
    (0x12, 0x3F, '»', '"'),
    (0x13, 0x20, 'Ã', 'A'),
    (0x13, 0x21, 'ã', 'a'),
    (0x13, 0x22, 'Í', 'I'),
    (0x13, 0x23, 'Ì', 'I'),
    (0x13, 0x24, 'ì', 'i'),
    (0x13, 0x25, 'Ò', 'O'),
    (0x13, 0x26, 'ò', 'o'),
    (0x13, 0x27, 'Õ', 'O'),
    (0x13, 0x28, 'õ', 'o'),
    (0x13, 0x29, '{', '('),
    (0x13, 0x2A, '}', ')'),
    (0x13, 0x2B, '\\', '/'),
    (0x13, 0x2C, '^', '\''),
    (0x13, 0x2D, '_', '-'),
    (0x13, 0x2E, '|', '!'),
    (0x13, 0x2F, '~', '-'),
    (0x13, 0x30, 'Ä', 'A'),
    (0x13, 0x31, 'ä', 'a'),
    (0x13, 0x32, 'Ö', 'O'),
    (0x13, 0x33, 'ö', 'o'),
    (0x13, 0x34, 'ß', 's'),
    (0x13, 0x35, '¥', 'Y'),
    (0x13, 0x36, '¤', 'o'),
    (0x13, 0x37, '¦', '!'),
    (0x13, 0x38, 'Å', 'A'),
    (0x13, 0x39, 'å', 'a'),
    (0x13, 0x3A, 'Ø', 'O'),
    (0x13, 0x3B, 'ø', 'o'),
    (0x13, 0x3C, '┌', '+'),
    (0x13, 0x3D, '┐', '+'),
    (0x13, 0x3E, '└', '+'),
    (0x13, 0x3F, '┘', '+'),
];

fn glyph(c: char) -> Option<Glyph> {
    if let Some((code, _)) = BASIC_NON_ASCII.iter().find(|(_, ch)| *ch == c) {
        return Some(Glyph::Basic(*code));
    }
    if let Some((_, _, _, _)) = EXTENDED.iter().find(|(_, _, ch, _)| *ch == c) {
        let (a, b, _, fallback) = EXTENDED.iter().find(|(_, _, ch, _)| *ch == c)?;
        let fb = glyph(*fallback).and_then(|g| match g {
            Glyph::Basic(x) => Some(x),
            _ => None,
        })?;
        return Some(Glyph::Extended(*a, *b, fb));
    }
    if let Some((code, _)) = SPECIAL.iter().find(|(_, ch)| *ch == c) {
        return Some(Glyph::Special(*code));
    }
    let b = u32::from(c);
    if (0x20..0x7F).contains(&b)
        && !BASIC_NON_ASCII
            .iter()
            .any(|(code, _)| u32::from(*code) == b)
    {
        return Some(Glyph::Basic(b as u8));
    }
    None
}

fn decode_basic(b: u8) -> Option<char> {
    let b = b & 0x7F;
    if let Some((_, c)) = BASIC_NON_ASCII.iter().find(|(code, _)| *code == b) {
        return Some(*c);
    }
    (0x20..0x7F).contains(&b).then_some(b as char)
}

/// The pairs that load and show one caption, in *groups* that must stay
/// together on the wire — a control code and its repeat, an extended character
/// and the fallback it replaces — so that something inserted between pairs is
/// only ever inserted between groups. The last group is the doubled EOC.
fn caption_groups(c: &Caption, index: usize) -> Result<Vec<Vec<String>>> {
    let lines: Vec<&String> = c.lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Err(Error::Edit(format!("caption {} has no text", index + 1)));
    }
    if lines.len() > 4 {
        return Err(Error::Edit(format!(
            "caption {} has {} lines; 608 pop-on shows at most 4",
            index + 1,
            lines.len()
        )));
    }
    let twice = |p: String| vec![p.clone(), p];
    let mut groups = vec![twice(pair(RCL.0, RCL.1)), twice(pair(ENM.0, ENM.1))];
    let first_row: u8 = match c.placement {
        Placement::Top => 1,
        Placement::Bottom => 16 - lines.len() as u8,
    };
    for (i, line) in lines.iter().enumerate() {
        let glyphs: Vec<Glyph> = line
            .chars()
            .map(|ch| {
                glyph(ch).ok_or_else(|| {
                    Error::Edit(format!(
                        "caption {}: `{ch}` has no CEA-608 character, so SCC cannot carry it",
                        index + 1
                    ))
                })
            })
            .collect::<Result<_>>()?;
        let width = glyphs.len();
        if width > 32 {
            return Err(Error::Edit(format!(
                "caption {}: a line of {width} characters; 608 rows hold 32",
                index + 1
            )));
        }
        // Centred: the PAC indents in fours, a tab offset makes up the rest.
        let column = (32 - width) / 2;
        let (row_byte, base) = pac_row(first_row + i as u8);
        let indent = (column / 4) * 4;
        groups.push(twice(pair(row_byte, base + 0x10 + (indent / 2) as u8)));
        let tab = column - indent;
        if tab > 0 {
            groups.push(twice(pair(0x17, 0x20 + tab as u8)));
        }
        let mut pending: Option<u8> = None;
        let flush = |groups: &mut Vec<Vec<String>>, pending: &mut Option<u8>| {
            if let Some(b) = pending.take() {
                groups.push(vec![format!("{:02x}80", with_parity(b))]);
            }
        };
        for g in glyphs {
            match g {
                Glyph::Basic(b) => match pending.take() {
                    Some(a) => groups.push(vec![pair(a, b)]),
                    None => pending = Some(b),
                },
                Glyph::Special(code) => {
                    flush(&mut groups, &mut pending);
                    groups.push(twice(pair(0x11, code)));
                }
                Glyph::Extended(first, code, fallback) => {
                    flush(&mut groups, &mut pending);
                    let p = pair(first, code);
                    groups.push(vec![
                        format!("{:02x}80", with_parity(fallback)),
                        p.clone(),
                        p,
                    ]);
                }
            }
        }
        flush(&mut groups, &mut pending);
    }
    groups.push(twice(pair(EOC.0, EOC.1)));
    Ok(groups)
}

/// Pairs of a caption, flat, and the index of its first EOC.
#[cfg(test)]
fn caption_pairs(c: &Caption, index: usize) -> Result<(Vec<String>, usize)> {
    let groups = caption_groups(c, index)?;
    let flat: Vec<String> = groups.concat();
    let eoc_at = flat.len() - 2;
    Ok((flat, eoc_at))
}

/// 29.97 frame of an exact time, rounded to the nearest.
fn frame_2997(t: RationalTime) -> i64 {
    let r = t.rate_exact();
    let rate = scc_rate();
    let num = i128::from(t.ticks()) * i128::from(r.den()) * i128::from(rate.num());
    let den = i128::from(r.num()) * i128::from(rate.den());
    ((2 * num + den) / (2 * den)) as i64
}

fn timecode(frame: i64) -> String {
    RationalTime::exact(frame, scc_rate())
        .to_drop_frame_timecode()
        .expect("29.97 supports drop frame")
}

/// Write pop-on SCC for a caption track.
///
/// Each caption is one line, starting early enough that its EOC lands on the
/// frame it should appear. Clearing it is an Erase Displayed Memory at the
/// frame it should go — on its own line when nothing is being sent then, and
/// otherwise *inside* the next caption's load, which is being built in the
/// other memory and is not disturbed by it. Spliced in, it goes between whole
/// groups of pairs and at the latest one not after its frame, so a caption can
/// clear up to a few frames early but never late; the line starts two frames
/// sooner to keep the next EOC on time.
pub fn write_scc(track: &CaptionTrack) -> Result<(String, SccReport)> {
    let mut report = SccReport::default();
    let mut sorted = track.clone();
    sorted.sort();
    struct Planned {
        show: i64,
        hide: i64,
        groups: Vec<Vec<String>>,
    }
    let mut planned = Vec::with_capacity(sorted.captions.len());
    for (i, c) in sorted.captions.iter().enumerate() {
        let groups = caption_groups(c, i)?;
        let show = frame_2997(c.record.start_time);
        let hide = frame_2997(c.record.end_time_exclusive());
        if track.rate != scc_rate() {
            for (t, f) in [
                (c.record.start_time, show),
                (c.record.end_time_exclusive(), hide),
            ] {
                let exact = t.to_seconds() * 1000.0;
                let now = RationalTime::exact(f, scc_rate()).to_seconds() * 1000.0;
                let moved = (exact - now).abs();
                if moved > 1e-9 {
                    report.requantised += 1;
                    report.largest_move_ms = report.largest_move_ms.max(moved);
                }
            }
        }
        planned.push(Planned { show, hide, groups });
    }

    let edm = || vec![pair(EDM.0, EDM.1), pair(EDM.0, EDM.1)];
    let mut out = String::from("Scenarist_SCC V1.0\n\n");
    // A clear owed by the previous caption: the frame it is due.
    let mut owed: Option<i64> = None;
    // The first frame after the last pair already written: lines cannot overlap,
    // because one pair goes out per frame.
    let mut busy_until: i64 = 0;
    for p in &planned {
        let pairs_before_eoc = |groups: &[Vec<String>]| -> i64 {
            groups[..groups.len() - 1]
                .iter()
                .map(|g| g.len() as i64)
                .sum()
        };
        let mut groups = p.groups.clone();
        let mut start = p.show - pairs_before_eoc(&groups);
        match owed.take() {
            // Due before this load begins: a line of its own.
            Some(due) if due < start && due >= busy_until => {
                out.push_str(&format!("{}\t{}\n\n", timecode(due), edm().join(" ")));
                busy_until = due + 2;
            }
            // Due while this caption loads: spliced in between groups.
            Some(due) if due < p.show => {
                start -= 2;
                let mut at = start;
                let mut slot = 0;
                for (k, g) in groups.iter().enumerate().take(groups.len() - 1) {
                    if at > due {
                        break;
                    }
                    slot = k;
                    at += g.len() as i64;
                }
                // `slot` is the last group starting at or before `due`; insert
                // before it so the erase is never late.
                groups.insert(slot, edm());
            }
            // Due when this caption appears, or after: its EOC replaces the
            // display, and an erase would blank the new caption.
            _ => {}
        }
        if start < 0 {
            report.clamped_at_start += 1;
            start = 0;
        }
        if start < busy_until {
            report.late += 1;
            report.latest_frames = report.latest_frames.max(busy_until - start);
            start = busy_until;
        }
        let flat = groups.concat();
        busy_until = start + flat.len() as i64;
        out.push_str(&format!("{}\t{}\n\n", timecode(start), flat.join(" ")));
        owed = Some(p.hide);
        report.captions += 1;
    }
    if let Some(due) = owed {
        let due = due.max(busy_until);
        out.push_str(&format!("{}\t{}\n\n", timecode(due), edm().join(" ")));
    }
    Ok((out, report))
}

/// Frame number of a drop-frame (or non-drop) SCC timecode at 29.97.
fn parse_timecode(tc: &str) -> Option<i64> {
    let drop = tc.contains(';');
    let parts: Vec<i64> = tc
        .split([':', ';', '.'])
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [h, m, s, f] = parts.as_slice() else {
        return None;
    };
    let total_minutes = h * 60 + m;
    let labelled = ((h * 60 + m) * 60 + s) * 30 + f;
    Some(if drop {
        labelled - 2 * (total_minutes - total_minutes / 10)
    } else {
        labelled
    })
}

/// Read the pop-on subset of SCC this writes: RCL, ENM, PACs, tab offsets,
/// basic, special and extended characters, EOC and EDM. Roll-up and paint-on
/// captions are refused by name rather than guessed at.
pub fn read_scc(text: &str, language: &str) -> Result<CaptionTrack> {
    let mut track = CaptionTrack::new(language, scc_rate());
    let mut lines = text.lines();
    if !lines
        .next()
        .is_some_and(|l| l.trim().starts_with("Scenarist_SCC"))
    {
        return Err(Error::Edit(
            "not an SCC file: no Scenarist_SCC header".into(),
        ));
    }
    let mut loading: Vec<(u8, String)> = Vec::new();
    let mut shown: Option<(i64, Vec<(u8, String)>)> = None;
    let mut last: Option<(u8, u8)> = None;
    let mut row: u8 = 15;
    let finish =
        |track: &mut CaptionTrack, shown: &mut Option<(i64, Vec<(u8, String)>)>, at: i64| {
            if let Some((from, rows)) = shown.take() {
                if at > from {
                    let mut rows = rows;
                    rows.sort_by_key(|(r, _)| *r);
                    let top = rows.first().is_some_and(|(r, _)| *r <= 4);
                    let mut c = Caption::new(
                        TimeRange::new(
                            RationalTime::exact(from, scc_rate()),
                            RationalTime::exact(at - from, scc_rate()),
                        ),
                        "",
                    );
                    c.lines = rows
                        .into_iter()
                        .map(|(_, t)| t.trim().to_string())
                        .collect();
                    c.placement = if top {
                        Placement::Top
                    } else {
                        Placement::Bottom
                    };
                    track.captions.push(c);
                }
            }
        };
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((tc, data)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let start = parse_timecode(tc)
            .ok_or_else(|| Error::Edit(format!("unreadable SCC timecode `{tc}`")))?;
        for (k, word) in data.split_whitespace().enumerate() {
            let frame = start + k as i64;
            if word.len() != 4 {
                return Err(Error::Edit(format!("unreadable SCC byte pair `{word}`")));
            }
            let a = u8::from_str_radix(&word[..2], 16)
                .map_err(|_| Error::Edit(format!("unreadable SCC byte pair `{word}`")))?
                & 0x7F;
            let b = u8::from_str_radix(&word[2..], 16)
                .map_err(|_| Error::Edit(format!("unreadable SCC byte pair `{word}`")))?
                & 0x7F;
            if (0x10..=0x1F).contains(&a) {
                // A control or two-byte character: a repeat is redundancy.
                if last == Some((a, b)) {
                    last = None;
                    continue;
                }
                last = Some((a, b));
                match (a, b) {
                    (0x14, 0x20) => {}
                    (0x14, 0x2E) => loading.clear(),
                    (0x14, 0x2F) => {
                        finish(&mut track, &mut shown, frame);
                        shown = Some((frame, std::mem::take(&mut loading)));
                    }
                    (0x14, 0x2C) => finish(&mut track, &mut shown, frame),
                    (0x14, 0x25..=0x27) | (0x14, 0x29) => {
                        return Err(Error::Edit(
                            "roll-up or paint-on captions: this reads pop-on SCC only".into(),
                        ))
                    }
                    (0x17, 0x21..=0x23) => {
                        if let Some((_, t)) = loading.last_mut() {
                            t.push_str(&" ".repeat(usize::from(b - 0x20)));
                        }
                    }
                    (0x11, 0x30..=0x3F) => {
                        if let Some(ch) = SPECIAL.iter().find(|(c, _)| *c == b).map(|(_, ch)| *ch) {
                            if let Some((_, t)) = loading.last_mut() {
                                t.push(ch);
                            }
                        }
                    }
                    (0x12 | 0x13, 0x20..=0x3F) => {
                        if let Some(ch) = EXTENDED
                            .iter()
                            .find(|(x, y, _, _)| *x == a && *y == b)
                            .map(|(_, _, ch, _)| *ch)
                        {
                            if let Some((_, t)) = loading.last_mut() {
                                t.pop(); // the basic fallback it replaces
                                t.push(ch);
                            }
                        }
                    }
                    (_, 0x40..=0x7F) => {
                        row = match a {
                            0x11 => 1 + u8::from(b >= 0x60),
                            0x12 => 3 + u8::from(b >= 0x60),
                            0x15 => 5 + u8::from(b >= 0x60),
                            0x16 => 7 + u8::from(b >= 0x60),
                            0x17 => 9 + u8::from(b >= 0x60),
                            0x10 => 11,
                            0x13 => 12 + u8::from(b >= 0x60),
                            _ => 14 + u8::from(b >= 0x60),
                        };
                        loading.push((row, String::new()));
                    }
                    _ => {}
                }
            } else {
                last = None;
                if loading.is_empty() {
                    loading.push((row, String::new()));
                }
                for byte in [a, b] {
                    if byte == 0 {
                        continue;
                    }
                    if let Some(ch) = decode_basic(byte) {
                        loading.last_mut().expect("a row").1.push(ch);
                    }
                }
            }
        }
    }
    if shown.is_some() {
        return Err(Error::Edit(
            "the last caption is never cleared, so how long it shows is unknown".into(),
        ));
    }
    track.sort();
    Ok(track)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(frames: i64) -> RationalTime {
        RationalTime::exact(frames, scc_rate())
    }

    #[test]
    fn control_codes_have_the_parity_every_scc_file_shows() {
        assert_eq!(pair(RCL.0, RCL.1), "9420");
        assert_eq!(pair(ENM.0, ENM.1), "94ae");
        assert_eq!(pair(EOC.0, EOC.1), "942f");
        assert_eq!(pair(EDM.0, EDM.1), "942c");
        let (r15, b15) = pac_row(15);
        assert_eq!(pair(r15, b15 + 0x10), "9470", "row 15, indent 0");
        let (r14, b14) = pac_row(14);
        assert_eq!(pair(r14, b14 + 0x10), "94d0", "row 14, indent 0");
    }

    #[test]
    fn a_caption_appears_when_its_eoc_arrives() {
        let mut track = CaptionTrack::new("en", scc_rate());
        track
            .captions
            .push(Caption::new(TimeRange::new(at(60), at(90)), "Hello"));
        let (text, report) = write_scc(&track).unwrap();
        assert_eq!(report.captions, 1);
        assert_eq!(report.requantised, 0);
        let (pairs, eoc_at) = caption_pairs(&track.captions[0], 0).unwrap();
        // The line starts `eoc_at` frames early so EOC lands on frame 60.
        let first = text.lines().nth(2).unwrap();
        let (tc, _) = first.split_once('\t').unwrap();
        assert_eq!(parse_timecode(tc), Some(60 - eoc_at as i64));
        assert_eq!(pairs[eoc_at], "942f");
        assert!(text.contains("00:00:05;00\t942c 942c"), "{text}");
        let back = read_scc(&text, "en").unwrap();
        assert_eq!(back.captions[0].record, TimeRange::new(at(60), at(90)));
        assert_eq!(back.captions[0].lines, vec!["Hello"]);
    }

    /// The bug the independent reader found: a caption whose successor starts
    /// loading before it should clear stayed up until the successor appeared.
    /// It must clear on time, the erase spliced into the successor's load.
    #[test]
    fn a_caption_clears_on_time_even_while_the_next_one_loads() {
        let mut track = CaptionTrack::new("en", scc_rate());
        // Shown at 60 (its line is sent by 62), due to clear at 110.
        track
            .captions
            .push(Caption::new(TimeRange::new(at(60), at(50)), "First."));
        // Long, accented text: a load of dozens of frames, starting before 130.
        track.captions.push(Caption::new(
            TimeRange::new(at(150), at(60)),
            "ÁÉÓÚÜü ÀÂÇÈÊËëÎÏïÔÙùÛ«»",
        ));
        let (pairs, eoc_at) = caption_pairs(&track.captions[1], 1).unwrap();
        let load = 150 - eoc_at as i64 - 2;
        assert!(
            load > 62 && load < 110,
            "the load starts while the first caption is up: {load}"
        );
        let (text, report) = write_scc(&track).unwrap();
        assert_eq!(report.late, 0);
        let back = read_scc(&text, "en").unwrap();
        let first_end = back.captions[0].record.end_time_exclusive().ticks();
        assert!(
            (107..=110).contains(&first_end),
            "cleared at {first_end}, due at 110, never late:\n{text}"
        );
        assert_eq!(
            back.captions[1].record.start_time,
            at(150),
            "and the next is still on time"
        );
        assert_eq!(back.captions[1].lines, vec!["ÁÉÓÚÜü ÀÂÇÈÊËëÎÏïÔÙùÛ«»"]);
        let _ = pairs;
    }

    #[test]
    fn drop_frame_timecodes_count_real_frames() {
        assert_eq!(parse_timecode("00:01:00;02"), Some(1800));
        assert_eq!(parse_timecode("00:10:00;00"), Some(17982));
        assert_eq!(timecode(1800), "00:01:00;02");
        assert_eq!(timecode(17982), "00:10:00;00");
    }

    #[test]
    fn what_608_cannot_show_is_refused_by_name() {
        let mut track = CaptionTrack::new("en", scc_rate());
        track.captions.push(Caption::new(
            TimeRange::new(at(0), at(30)),
            "This line is far too long for a 608 row to hold",
        ));
        let e = write_scc(&track).unwrap_err();
        assert!(e.to_string().contains("608 rows hold 32"), "{e}");
        track.captions[0] = Caption::new(TimeRange::new(at(0), at(30)), "日本");
        let e = write_scc(&track).unwrap_err();
        assert!(e.to_string().contains("no CEA-608 character"), "{e}");
    }

    #[test]
    fn every_mapped_character_round_trips() {
        let all: String = BASIC_NON_ASCII
            .iter()
            .map(|(_, c)| *c)
            .chain(SPECIAL.iter().map(|(_, c)| *c))
            .chain(EXTENDED.iter().map(|(_, _, c, _)| *c))
            .collect();
        let mut track = CaptionTrack::new("en", scc_rate());
        for (i, chunk) in all.chars().collect::<Vec<_>>().chunks(20).enumerate() {
            let text: String = chunk.iter().collect();
            track.captions.push(Caption::new(
                TimeRange::new(at(100 + 200 * i as i64), at(90)),
                &text,
            ));
        }
        let (scc, _) = write_scc(&track).unwrap();
        let back = read_scc(&scc, "en").unwrap();
        let got: String = back.captions.iter().map(|c| c.lines.join("")).collect();
        assert_eq!(got, all);
    }
}
