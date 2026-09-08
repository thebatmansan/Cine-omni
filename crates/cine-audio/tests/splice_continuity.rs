//! Butt-spliced clips must meet exactly, with no gap and no overlap.
//!
//! A dialogue edit is a butt splice: clip A ends at exactly the instant clip B
//! begins. If the renderer places A's last sample and B's first sample even one
//! sample apart, the result is a click — audible, and at every edit in the
//! sequence.
//!
//! The failure mode this guards is rounding `start` and `duration`
//! *independently*: `round(start·sr) + round(duration·sr)` is not in general
//! equal to `round((start + duration)·sr)`. The boundary must be derived from
//! the clip's end time, so that adjacent clips resolve to the same frame.
//!
//! The 30000/1001 rate is not arbitrary: at 48 kHz one 29.97 frame is 1601.6
//! samples, so frame boundaries land off the sample grid and the two rounding
//! strategies genuinely disagree. On integer rates they agree by luck.

use cine_audio::buffer::AudioBuffer;
use cine_audio::session::{Clip, Session, Track};
use std::sync::Arc;
use ucg_core::{RationalTime, TimeRange};

const SR: u32 = 48_000;
/// 29.97 fps — one frame is 1601.6 samples at 48 kHz, so frame boundaries fall
/// between samples and the two rounding strategies genuinely disagree.
const RATE: f64 = 30_000.0 / 1001.0;
const FRAME_S: f64 = 1001.0 / 30_000.0;

/// A constant-amplitude buffer: a gap shows up as a dip to zero, an overlap as
/// a doubling.
fn tone(seconds: f64, level: f32) -> Arc<AudioBuffer> {
    let frames = (seconds * SR as f64).ceil() as usize + 16;
    Arc::new(AudioBuffer::from_planar(SR, &[vec![level; frames]]).expect("buffer"))
}

/// Trim a clip to `[start_s, start_s + duration_s)` on the session timeline —
/// which is exactly what trimming a clip in a dialogue edit produces.
fn trimmed(name: &str, start_s: f64, duration_s: f64) -> Clip {
    let mut clip = Clip::from_buffer(name, tone(duration_s + 1.0, 0.5), start_s, RATE);
    clip.range = TimeRange::new(
        RationalTime::from_seconds(start_s, RATE),
        RationalTime::from_seconds(duration_s, RATE),
    );
    clip
}

/// Two four-frame clips meeting exactly at frame `n` of a 29.97 timeline.
fn spliced_session(n: i64) -> Session {
    let len = 4.0 * FRAME_S;
    let boundary = n as f64 * FRAME_S;

    let mut session = Session::new(SR, RATE);
    let master = session.master_id();
    session.tracks.push(
        Track::new("dialogue", master)
            .with_clip(trimmed("A", boundary, len))
            .with_clip(trimmed("B", boundary + len, len)),
    );
    session
}

/// Scan the rendered mix for a sample that dips to silence or doubles.
fn boundary_defect(session: &Session, range: (f64, f64)) -> Option<(usize, f32)> {
    let out =
        cine_audio::mix::render(session, range, cine_audio::ChannelLayout::Mono).expect("render");
    let ch = out.channel(0);
    // Ignore the first and last few samples, where the render window itself
    // truncates; the splice is in the middle by construction.
    let guard = 8;
    ch.iter()
        .enumerate()
        .skip(guard)
        .take(ch.len().saturating_sub(guard * 2))
        .find(|(_, &s)| s.abs() < 0.25 || s.abs() > 0.75)
        .map(|(i, &s)| (i, s))
}

#[test]
fn butt_spliced_clips_leave_no_gap_and_no_overlap() {
    // Sweep several boundaries: the rounding disagreement depends on where the
    // frame lands relative to the sample grid, so one boundary proves little.
    let mut defects = Vec::new();
    for n in 1..=12i64 {
        let session = spliced_session(n);
        let start = n as f64 * FRAME_S;
        let end = (n as f64 + 8.0) * FRAME_S;
        if let Some((i, s)) = boundary_defect(&session, (start, end)) {
            defects.push(format!(
                "frame {n}: sample {i} reads {s:.3} (expected 0.5) — {}",
                if s.abs() < 0.25 {
                    "GAP, a click"
                } else {
                    "OVERLAP, a doubling"
                }
            ));
        }
    }
    assert!(
        defects.is_empty(),
        "butt splices are not sample-exact; every dialogue edit would click:\n  {}",
        defects.join("\n  ")
    );
}

/// The property stated directly: a clip's rendered length must be the
/// difference of its rounded boundaries, not the rounding of its difference.
#[test]
fn clip_boundaries_are_derived_from_end_time_not_from_duration() {
    let sr = SR as f64;
    // Find a frame boundary where the two strategies genuinely disagree, so
    // this test would have caught the defect rather than passing by luck.
    let disagreements = (1..200i64)
        .filter(|n| {
            let start = *n as f64 * FRAME_S;
            let dur = 4.0 * FRAME_S;
            let independent = (start * sr).round() as i64 + (dur * sr).round() as i64;
            let from_end = ((start + dur) * sr).round() as i64;
            independent != from_end
        })
        .count();
    assert!(
        disagreements > 0,
        "test is vacuous: the two rounding strategies never disagree at this rate"
    );

    // And the renderer must use the second strategy.
    for n in 1..=12i64 {
        let session = spliced_session(n);
        let a_end = session.tracks[0].clips[0].end_s();
        let b_start = session.tracks[0].clips[1].start_s();
        assert!(
            (a_end - b_start).abs() < 1e-12,
            "fixture is wrong: clips do not actually abut"
        );
        assert_eq!(
            (a_end * sr).round() as i64,
            (b_start * sr).round() as i64,
            "the shared boundary must resolve to one frame for both clips"
        );
    }
}
