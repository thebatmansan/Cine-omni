//! Captions: timed text on the cut, in the formats a delivery asks for.
//!
//! A caption is a stretch of **record time** and the lines shown during it.
//! Record time here is the sequence's own exact time, so a caption can sit on
//! a frame boundary and stay there — which matters, because the constraints a
//! delivery checks are counted in frames (two frames between events, no event
//! starting on a shot change's wrong side) and a caption that drifted half a
//! frame would fail them for a reason nobody could see.
//!
//! ## Formats, and what each costs
//!
//! * **IMSC 1.1 Text** ([`write_imsc`]) is TTML with frame-exact times: the
//!   document declares the sequence's frame rate as `ttp:frameRate` and
//!   `ttp:frameRateMultiplier`, and every time is an offset in frames. Nothing
//!   is rounded. This is the one a streaming delivery wants.
//! * **SRT** ([`write_srt`], [`read_srt`]) and **WebVTT** ([`write_vtt`]) store
//!   milliseconds. A frame at 24000/1001 is 41.708 3… ms, so writing rounds
//!   every time to the nearest millisecond and reading snaps each back to the
//!   nearest frame. Because the rounding is at most half a millisecond — it
//!   is exactly half on a tie, such as frame 12 at 24000/1001, which is
//!   500.5 ms — and a frame is tens of milliseconds, a caption written here and read back
//!   at the same rate lands on exactly the frame it left — the tests hold that
//!   over an hour at NTSC rates — but a file from elsewhere whose times fall
//!   between frames is moved, and [`ReadReport`] says how far.
//!
//! ## What is checked
//!
//! [`check`] applies the constraints most delivery specifications share —
//! lines, characters per line, minimum and maximum duration, the gap between
//! events, reading speed, overlap — with [`Rules::default`] as common values
//! rather than any one company's. A delivery's own numbers go in [`Rules`].
//! This is **not** a validator for the IMSC specification; the XML written
//! here is checked against an independent TTML reader in the tests instead.

use crate::error::{Error, Result};
use ucg_core::{Rate, RationalTime, TimeRange};

/// Where on the picture a caption sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    #[default]
    Bottom,
    /// Moved up, typically so as not to cover on-screen text.
    Top,
}

/// One caption event.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Caption {
    /// When it is on screen, in sequence time.
    pub record: TimeRange,
    /// The lines, as shown.
    pub lines: Vec<String>,
    #[serde(default)]
    pub placement: Placement,
    /// Who is speaking. Kept for review and for SDH; not written into the
    /// text, because whether and how to identify a speaker is a style
    /// decision the caption author makes in the lines themselves.
    #[serde(default)]
    pub speaker: Option<String>,
    /// A stable identity within its track, assigned when the caption is first
    /// stored. `None` for a caption that has never been stored — one just
    /// typed, or read from a file.
    ///
    /// Captions need an identity that is not their position: a caption is
    /// linked to the line of dialogue it transcribes, and inserting a caption
    /// earlier in the track must not move that link onto a different one.
    #[serde(default)]
    pub key: Option<String>,
}

impl Caption {
    pub fn new(record: TimeRange, text: &str) -> Caption {
        Caption {
            record,
            lines: text.lines().map(str::to_string).collect(),
            placement: Placement::Bottom,
            speaker: None,
            key: None,
        }
    }

    /// Characters shown, not counting line breaks.
    pub fn characters(&self) -> usize {
        self.lines.iter().map(|l| l.chars().count()).sum()
    }
}

/// A language's captions for one sequence.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CaptionTrack {
    /// BCP 47, e.g. `en`, `en-GB`, `fr-CA`.
    pub language: String,
    /// The sequence's frame rate: what every time is counted in.
    pub rate: Rate,
    pub captions: Vec<Caption>,
}

impl CaptionTrack {
    pub fn new(language: &str, rate: Rate) -> CaptionTrack {
        CaptionTrack {
            language: language.to_string(),
            rate,
            captions: Vec::new(),
        }
    }

    /// Captions in the order they appear.
    pub fn sort(&mut self) {
        self.captions.sort_by_key(|c| c.record.start_time);
    }
}

// ---------------------------------------------------------------------------
// time
// ---------------------------------------------------------------------------

/// Whole frames at `rate` for an exact time, rounding to the nearest frame.
fn frames_at(t: RationalTime, rate: Rate) -> i64 {
    let r = t.rate_exact();
    // t seconds = ticks * r.den / r.num; frames = seconds * rate.num / rate.den
    let num = i128::from(t.ticks()) * i128::from(r.den()) * i128::from(rate.num());
    let den = i128::from(r.num()) * i128::from(rate.den());
    round_div(num, den) as i64
}

/// Milliseconds for an exact time, rounded to the nearest.
fn millis(t: RationalTime) -> i64 {
    let r = t.rate_exact();
    let num = i128::from(t.ticks()) * i128::from(r.den()) * 1000;
    round_div(num, i128::from(r.num())) as i64
}

/// Nearest whole frame at `rate` to a time in milliseconds.
fn frame_of_millis(ms: i64, rate: Rate) -> i64 {
    round_div(
        i128::from(ms) * i128::from(rate.num()),
        1000 * i128::from(rate.den()),
    ) as i64
}

/// `num / den` rounded half away from zero, for `den > 0`.
fn round_div(num: i128, den: i128) -> i128 {
    if num >= 0 {
        (2 * num + den) / (2 * den)
    } else {
        -((-2 * num + den) / (2 * den))
    }
}

fn clock(ms: i64, separator: char) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

// ---------------------------------------------------------------------------
// SRT and WebVTT
// ---------------------------------------------------------------------------

/// What writing to a millisecond format rounded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteReport {
    pub captions: usize,
    /// The largest distance any time moved when rounded to a millisecond, in
    /// microseconds. At most 500 — exactly 500 on a tie — and reported so that
    /// nobody has to take that on trust.
    pub largest_rounding_us: i64,
}

fn rounding_us(t: RationalTime) -> i64 {
    // |exact - rounded| in microseconds, exactly.
    let r = t.rate_exact();
    let exact_us_num = i128::from(t.ticks()) * i128::from(r.den()) * 1_000_000;
    let rounded_us = i128::from(millis(t)) * 1000 * i128::from(r.num());
    ((exact_us_num - rounded_us).abs() / i128::from(r.num())) as i64
}

fn timed_text<F>(track: &CaptionTrack, header: &str, mut line: F) -> (String, WriteReport)
where
    F: FnMut(usize, &Caption, i64, i64) -> String,
{
    let mut report = WriteReport::default();
    let mut out = String::from(header);
    for (i, c) in track.captions.iter().enumerate() {
        let (start, end) = (c.record.start_time, c.record.end_time_exclusive());
        report.largest_rounding_us = report
            .largest_rounding_us
            .max(rounding_us(start))
            .max(rounding_us(end));
        out.push_str(&line(i, c, millis(start), millis(end)));
        report.captions += 1;
    }
    (out, report)
}

/// SubRip. Times are rounded to the millisecond; see the module note.
pub fn write_srt(track: &CaptionTrack) -> (String, WriteReport) {
    timed_text(track, "", |i, c, start, end| {
        format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            clock(start, ','),
            clock(end, ','),
            c.lines.join("\n")
        )
    })
}

/// WebVTT, for a browser's `<track>`. Top placement is written as a line
/// position, which is the only placement this model has.
pub fn write_vtt(track: &CaptionTrack) -> (String, WriteReport) {
    timed_text(track, "WEBVTT\n\n", |_, c, start, end| {
        let settings = match c.placement {
            Placement::Bottom => "",
            Placement::Top => " line:0",
        };
        format!(
            "{} --> {}{settings}\n{}\n\n",
            clock(start, '.'),
            clock(end, '.'),
            c.lines.join("\n")
        )
    })
}

/// What reading a millisecond format did to the times.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadReport {
    pub captions: usize,
    /// Times that were not already on a frame and were moved to the nearest.
    pub snapped: usize,
    /// The largest such move, in milliseconds.
    pub largest_snap_ms: f64,
    /// Blocks that could not be read, with why.
    pub skipped: Vec<String>,
}

/// Read SubRip at the sequence rate, snapping every time to the nearest frame.
///
/// Refused outright only when nothing at all could be read. Individual blocks
/// that are malformed are skipped and listed, because a subtitle file with one
/// bad cue is still a subtitle file.
pub fn read_srt(text: &str, language: &str, rate: Rate) -> Result<(CaptionTrack, ReadReport)> {
    let mut track = CaptionTrack::new(language, rate);
    let mut report = ReadReport::default();
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    for block in text.split("\n\n").map(str::trim).filter(|b| !b.is_empty()) {
        let mut lines = block.lines();
        let mut first = lines.next().unwrap_or_default();
        // The counter line is optional in practice.
        if !first.contains("-->") {
            first = lines.next().unwrap_or_default();
        }
        let Some((a, b)) = first.split_once("-->") else {
            report.skipped.push(format!(
                "no timing line in `{}`",
                block.lines().next().unwrap_or("")
            ));
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (parse_clock(a.trim()), parse_clock(b.trim())) else {
            report.skipped.push(format!("unreadable times `{first}`"));
            continue;
        };
        let mut snap = |ms: i64| {
            let f = frame_of_millis(ms, rate);
            let back = RationalTime::exact(f, rate);
            let exact_ms = back.to_seconds() * 1000.0;
            let moved = (exact_ms - ms as f64).abs();
            // A time written by `write_srt` is off its frame by under a
            // millisecond of rounding; only a real move counts as a snap.
            if moved >= 1.0 {
                report.snapped += 1;
                report.largest_snap_ms = report.largest_snap_ms.max(moved);
            }
            f
        };
        let (s, e) = (snap(start_ms), snap(end_ms));
        if e <= s {
            report.skipped.push(format!(
                "`{first}` ends before it starts at this frame rate"
            ));
            continue;
        }
        let body: Vec<String> = lines.map(strip_tags).collect();
        if body.iter().all(|l| l.trim().is_empty()) {
            report.skipped.push(format!("`{first}` has no text"));
            continue;
        }
        track.captions.push(Caption {
            record: TimeRange::new(
                RationalTime::exact(s, rate),
                RationalTime::exact(e - s, rate),
            ),
            lines: body,
            placement: Placement::Bottom,
            speaker: None,
            key: None,
        });
        report.captions += 1;
    }
    if track.captions.is_empty() {
        return Err(Error::Edit(match report.skipped.first() {
            Some(why) => format!("no captions could be read: {why}"),
            None => "the file has no captions".into(),
        }));
    }
    track.sort();
    Ok((track, report))
}

/// `HH:MM:SS,mmm` or `HH:MM:SS.mmm` (WebVTT's separator is accepted too).
fn parse_clock(s: &str) -> Option<i64> {
    let s = s.split_whitespace().next()?;
    let (hms, ms) = s.split_once([',', '.'])?;
    let parts: Vec<i64> = hms
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    let ms: i64 = ms.get(..3.min(ms.len()))?.parse().ok()?;
    if m >= 60 || sec >= 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

/// Drop the formatting SRT defines, and nothing else.
///
/// SRT has no escaping, so `<pepper>` in a subtitle is the word, not a tag.
/// Only the tags players actually honour — `<i>`, `<b>`, `<u>`, `<font …>` and
/// their closers — and `{\…}` override blocks are formatting; the model has no
/// styling, so those go. Everything else in brackets is kept as written. (An
/// earlier version dropped anything between `<` and `>`, and the independent
/// reader's output caught it deleting a word.)
fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find(['<', '{']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let close = if tail.starts_with('<') { '>' } else { '}' };
        let Some(j) = tail.find(close) else {
            out.push_str(tail);
            return out;
        };
        let inner = &tail[1..j];
        if is_formatting(inner, close) {
            rest = &tail[j + 1..];
        } else {
            out.push_str(&tail[..=j]);
            rest = &tail[j + 1..];
        }
    }
    out.push_str(rest);
    out
}

fn is_formatting(inner: &str, close: char) -> bool {
    if close == '}' {
        return inner.starts_with('\\');
    }
    let name = inner.trim_start_matches('/');
    let tag = name
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match tag.as_str() {
        "i" | "b" | "u" => name.len() == 1,
        "font" => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// IMSC 1.1 Text
// ---------------------------------------------------------------------------

/// `ttp:frameRate` and `ttp:frameRateMultiplier` for an exact rate.
///
/// TTML's frame rate is an integer with a rational multiplier, so 24000/1001
/// is `24` with `1000 1001`. A rate that cannot be written that way — whose
/// nearest integer multiplied back does not give it exactly — is refused
/// rather than approximated, because every time in the document is counted in
/// these frames.
fn ttml_frame_rate(rate: Rate) -> Result<(u64, Option<(u64, u64)>)> {
    let (num, den) = (u64::from(rate.num()), u64::from(rate.den()));
    if den == 1 {
        return Ok((num, None));
    }
    let whole = (num + den / 2) / den;
    if whole == 0 {
        return Err(Error::Edit(format!("{num}/{den} is not a frame rate")));
    }
    // rate = whole * m_num / m_den  =>  m_num / m_den = num / (whole * den)
    let (mut m_num, mut m_den) = (num, whole * den);
    let g = gcd(m_num, m_den);
    m_num /= g;
    m_den /= g;
    Ok((whole, Some((m_num, m_den))))
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// IMSC 1.1 Text profile, with every time an exact offset in frames.
pub fn write_imsc(track: &CaptionTrack) -> Result<String> {
    let (frame_rate, multiplier) = ttml_frame_rate(track.rate)?;
    let multiplier = multiplier
        .map(|(n, d)| format!(" ttp:frameRateMultiplier=\"{n} {d}\""))
        .unwrap_or_default();
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <tt xmlns=\"http://www.w3.org/ns/ttml\"\n\
         \x20   xmlns:ttp=\"http://www.w3.org/ns/ttml#parameter\"\n\
         \x20   xmlns:tts=\"http://www.w3.org/ns/ttml#styling\"\n\
         \x20   xmlns:ttm=\"http://www.w3.org/ns/ttml#metadata\"\n\
         \x20   ttp:contentProfiles=\"http://www.w3.org/ns/ttml/profile/imsc1.1/text\"\n\
         \x20   ttp:timeBase=\"media\" ttp:frameRate=\"{frame_rate}\"{multiplier}\n\
         \x20   ttp:cellResolution=\"32 15\" xml:lang=\"{lang}\">\n\
         \x20 <head>\n\
         \x20   <styling>\n\
         \x20     <style xml:id=\"s\" tts:textAlign=\"center\" tts:color=\"white\" \
         tts:backgroundColor=\"black\" tts:fontSize=\"100%\"/>\n\
         \x20   </styling>\n\
         \x20   <layout>\n\
         \x20     <region xml:id=\"bottom\" tts:origin=\"10% 10%\" tts:extent=\"80% 80%\" \
         tts:displayAlign=\"after\"/>\n\
         \x20     <region xml:id=\"top\" tts:origin=\"10% 10%\" tts:extent=\"80% 80%\" \
         tts:displayAlign=\"before\"/>\n\
         \x20   </layout>\n\
         \x20 </head>\n\
         \x20 <body style=\"s\">\n\
         \x20   <div>\n",
        lang = xml_escape(&track.language),
    );
    for (i, c) in track.captions.iter().enumerate() {
        let begin = frames_at(c.record.start_time, track.rate);
        let end = frames_at(c.record.end_time_exclusive(), track.rate);
        let region = match c.placement {
            Placement::Bottom => "bottom",
            Placement::Top => "top",
        };
        let agent = c
            .speaker
            .as_ref()
            .map(|s| format!(" ttm:role=\"dialog\" ttm:agent=\"{}\"", xml_id(s)))
            .unwrap_or_default();
        let body = c
            .lines
            .iter()
            .map(|l| xml_escape(l))
            .collect::<Vec<_>>()
            .join("<br/>");
        out.push_str(&format!(
            "      <p xml:id=\"c{}\" begin=\"{begin}f\" end=\"{end}f\" region=\"{region}\"{agent}>{body}</p>\n",
            i + 1
        ));
    }
    out.push_str("    </div>\n  </body>\n</tt>\n");
    Ok(out)
}

/// An `xml:id`-safe token for a speaker name.
fn xml_id(name: &str) -> String {
    let mut id: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if !id.starts_with(|c: char| c.is_ascii_alphabetic()) {
        id.insert(0, 'a');
    }
    id
}

// ---------------------------------------------------------------------------
// checks
// ---------------------------------------------------------------------------

/// The numbers a delivery checks captions against.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rules {
    pub max_lines: usize,
    pub max_chars_per_line: usize,
    /// Shortest an event may be on screen, in milliseconds.
    pub min_duration_ms: i64,
    /// Longest, in milliseconds.
    pub max_duration_ms: i64,
    /// Frames of clear screen required between two events that do not touch.
    pub min_gap_frames: i64,
    /// Characters per second a viewer is expected to read.
    pub max_chars_per_second: f64,
}

impl Default for Rules {
    /// Values most streaming and broadcast specifications share for adult
    /// programming in Latin-script languages. Not any one company's: a
    /// delivery's own numbers belong here instead.
    fn default() -> Rules {
        Rules {
            max_lines: 2,
            max_chars_per_line: 42,
            min_duration_ms: 833, // five sixths of a second
            max_duration_ms: 7000,
            min_gap_frames: 2,
            max_chars_per_second: 20.0,
        }
    }
}

/// What is wrong with a caption, in the words a caption editor uses.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Issue {
    /// Index into the track's captions.
    pub caption: usize,
    pub kind: IssueKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    TooManyLines,
    LineTooLong,
    TooShort,
    TooLong,
    GapTooSmall,
    Overlap,
    ReadingSpeed,
    Empty,
}

/// Check a track against `rules`. Captions are checked in time order.
pub fn check(track: &CaptionTrack, rules: &Rules) -> Vec<Issue> {
    let mut order: Vec<usize> = (0..track.captions.len()).collect();
    order.sort_by_key(|i| track.captions[*i].record.start_time);
    let mut issues = Vec::new();
    let mut push = |caption: usize, kind: IssueKind, message: String| {
        issues.push(Issue {
            caption,
            kind,
            message,
        })
    };
    for (k, &i) in order.iter().enumerate() {
        let c = &track.captions[i];
        let at = c.record.start_time.to_timecode();
        if c.lines.iter().all(|l| l.trim().is_empty()) {
            push(i, IssueKind::Empty, format!("{at}: no text"));
            continue;
        }
        if c.lines.len() > rules.max_lines {
            push(
                i,
                IssueKind::TooManyLines,
                format!("{at}: {} lines, at most {}", c.lines.len(), rules.max_lines),
            );
        }
        for line in &c.lines {
            let n = line.chars().count();
            if n > rules.max_chars_per_line {
                push(
                    i,
                    IssueKind::LineTooLong,
                    format!(
                        "{at}: a line of {n} characters, at most {}",
                        rules.max_chars_per_line
                    ),
                );
            }
        }
        let ms = millis(c.record.duration);
        if ms < rules.min_duration_ms {
            push(
                i,
                IssueKind::TooShort,
                format!(
                    "{at}: on screen {ms} ms, at least {}",
                    rules.min_duration_ms
                ),
            );
        }
        if ms > rules.max_duration_ms {
            push(
                i,
                IssueKind::TooLong,
                format!("{at}: on screen {ms} ms, at most {}", rules.max_duration_ms),
            );
        }
        if ms > 0 {
            let cps = c.characters() as f64 * 1000.0 / ms as f64;
            if cps > rules.max_chars_per_second {
                push(
                    i,
                    IssueKind::ReadingSpeed,
                    format!(
                        "{at}: {cps:.1} characters a second, at most {}",
                        rules.max_chars_per_second
                    ),
                );
            }
        }
        if let Some(&j) = order.get(k + 1) {
            let next = &track.captions[j];
            let end = frames_at(c.record.end_time_exclusive(), track.rate);
            let start = frames_at(next.record.start_time, track.rate);
            if start < end {
                push(
                    i,
                    IssueKind::Overlap,
                    format!("{at}: overlaps the next caption by {} frames", end - start),
                );
            } else if start > end && start - end < rules.min_gap_frames {
                push(
                    i,
                    IssueKind::GapTooSmall,
                    format!(
                        "{at}: {} frame gap to the next caption, at least {} — close it or open it",
                        start - end,
                        rules.min_gap_frames
                    ),
                );
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ntsc() -> Rate {
        Rate::new(24000, 1001).unwrap()
    }

    fn at(frames: i64, rate: Rate) -> RationalTime {
        RationalTime::exact(frames, rate)
    }

    fn cap(start: i64, len: i64, rate: Rate, text: &str) -> Caption {
        Caption::new(TimeRange::new(at(start, rate), at(len, rate)), text)
    }

    #[test]
    fn a_rate_is_written_as_ttml_wants_it_or_refused() {
        assert_eq!(
            ttml_frame_rate(Rate::whole(25).unwrap()).unwrap(),
            (25, None)
        );
        assert_eq!(ttml_frame_rate(ntsc()).unwrap(), (24, Some((1000, 1001))));
        assert_eq!(
            ttml_frame_rate(Rate::new(30000, 1001).unwrap()).unwrap(),
            (30, Some((1000, 1001)))
        );
        assert_eq!(
            ttml_frame_rate(Rate::new(60000, 1001).unwrap()).unwrap(),
            (60, Some((1000, 1001)))
        );
    }

    /// Writing to milliseconds and reading back lands on the same frame — for
    /// every frame of an hour at 24000/1001, not a sample of them.
    #[test]
    fn a_millisecond_round_trip_lands_on_the_frame_it_left() {
        let rate = ntsc();
        let hour = 24 * 3600;
        for f in 0..hour {
            let t = at(f, rate);
            assert_eq!(frame_of_millis(millis(t), rate), f, "frame {f}");
            assert!(
                rounding_us(t) <= 500,
                "frame {f} rounds by {}",
                rounding_us(t)
            );
        }
    }

    #[test]
    fn srt_round_trips_exactly_at_an_ntsc_rate() {
        let rate = ntsc();
        let mut track = CaptionTrack::new("en", rate);
        track.captions.push(cap(24, 48, rate, "We ride at dawn."));
        track
            .captions
            .push(cap(86_000, 50, rate, "Not before.\nNot ever."));
        let (text, report) = write_srt(&track);
        assert_eq!(report.captions, 2);
        assert!(report.largest_rounding_us <= 500);
        assert!(
            text.starts_with("1\n00:00:01,001 --> 00:00:03,003\nWe ride at dawn.\n"),
            "{text}"
        );

        let (back, read) = read_srt(&text, "en", rate).expect("read");
        assert_eq!(read.snapped, 0, "nothing had to move");
        assert_eq!(back, track, "and every time is the frame it was");
    }

    #[test]
    fn a_foreign_srt_is_snapped_to_frames_and_says_how_far() {
        let text = "1\r\n00:00:01,020 --> 00:00:02,000\r\n<i>Hello</i>\r\n\r\n\
                    garbage block\r\n\r\n\
                    3\r\n00:00:05,000 --> 00:00:04,000\r\nbackwards\r\n";
        let (track, report) = read_srt(text, "en", Rate::whole(24).unwrap()).expect("read");
        assert_eq!(track.captions.len(), 1);
        assert_eq!(track.captions[0].lines, vec!["Hello"], "tags are dropped");
        assert_eq!(
            strip_tags("Salt & <pepper>"),
            "Salt & <pepper>",
            "text is not a tag"
        );
        assert_eq!(strip_tags("<font color=\"#fff\">{\\an8}Top</font>"), "Top");
        assert_eq!(
            strip_tags("<B>loud</b> and {not an override}"),
            "loud and {not an override}"
        );
        assert_eq!(strip_tags("a < b > c"), "a < b > c");
        // 1.020 s at 24 fps is frame 24.48 -> 24, a move of 20 ms.
        assert_eq!(
            track.captions[0].record.start_time,
            at(24, Rate::whole(24).unwrap())
        );
        assert_eq!(report.snapped, 1);
        assert!(
            (report.largest_snap_ms - 20.0).abs() < 1e-9,
            "{}",
            report.largest_snap_ms
        );
        assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
        assert!(read_srt("nothing here", "en", ntsc()).is_err());
    }

    #[test]
    fn vtt_is_srt_with_a_header_a_dot_and_a_placement() {
        let rate = Rate::whole(24).unwrap();
        let mut track = CaptionTrack::new("en", rate);
        let mut c = cap(24, 24, rate, "SIGN: HARBOUR OFFICE");
        c.placement = Placement::Top;
        track.captions.push(c);
        let (text, _) = write_vtt(&track);
        assert_eq!(
            text,
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 line:0\nSIGN: HARBOUR OFFICE\n\n"
        );
    }

    #[test]
    fn imsc_times_are_whole_frames_and_text_is_escaped() {
        let rate = ntsc();
        let mut track = CaptionTrack::new("en-GB", rate);
        let mut c = cap(24, 48, rate, "Salt & <pepper>\nsecond line");
        c.speaker = Some("MAREN".into());
        track.captions.push(c);
        let xml = write_imsc(&track).unwrap();
        assert!(
            xml.contains("ttp:frameRate=\"24\" ttp:frameRateMultiplier=\"1000 1001\""),
            "{xml}"
        );
        assert!(xml.contains("begin=\"24f\" end=\"72f\""), "{xml}");
        assert!(
            xml.contains("Salt &amp; &lt;pepper&gt;<br/>second line"),
            "{xml}"
        );
        assert!(xml.contains("ttm:agent=\"MAREN\""), "{xml}");
        assert!(xml.contains("xml:lang=\"en-GB\""));
    }

    #[test]
    fn the_checks_say_what_is_wrong_in_a_caption_editors_words() {
        let rate = Rate::whole(24).unwrap();
        let mut track = CaptionTrack::new("en", rate);
        // Fine: two seconds, short text.
        track.captions.push(cap(0, 48, rate, "Fine."));
        // One frame gap after the first, and too short, and three lines.
        track.captions.push(cap(49, 12, rate, "one\ntwo\nthree"));
        // Overlaps the previous one? No: starts at 61 = end. Too long a line
        // and far too fast to read.
        track.captions.push(cap(
            61,
            24,
            rate,
            "This line is considerably longer than forty-two characters.",
        ));
        // Eight seconds on screen.
        track.captions.push(cap(100, 192, rate, "Long."));
        // Overlaps the previous.
        track.captions.push(cap(250, 48, rate, "Overlap."));

        let issues = check(&track, &Rules::default());
        let kinds = |i: usize| -> Vec<IssueKind> {
            issues
                .iter()
                .filter(|x| x.caption == i)
                .map(|x| x.kind)
                .collect()
        };
        assert_eq!(kinds(0), vec![IssueKind::GapTooSmall]);
        assert!(kinds(1).contains(&IssueKind::TooManyLines));
        assert!(kinds(1).contains(&IssueKind::TooShort));
        assert!(kinds(2).contains(&IssueKind::LineTooLong));
        assert!(kinds(2).contains(&IssueKind::ReadingSpeed));
        assert!(kinds(3).contains(&IssueKind::TooLong));
        assert!(kinds(3).contains(&IssueKind::Overlap));
        assert!(
            issues[0].message.contains("1 frame gap"),
            "{}",
            issues[0].message
        );
    }
}
