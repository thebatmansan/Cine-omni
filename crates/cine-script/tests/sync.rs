mod common;
use cine_script::prelude::*;
use common::*;

/// Deterministic LCG so the corruption is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn script_words() -> Vec<String> {
    let sp = fixture();
    sp.elements
        .iter()
        .filter_map(|e| match e {
            Element::Dialogue { text } => Some(word_list(text)),
            _ => None,
        })
        .flatten()
        .collect()
}

fn truth(words: &[String]) -> Vec<TimedWord> {
    let mut t = Vec::new();
    let mut clock = 0.0;
    for (i, w) in words.iter().enumerate() {
        let dur = 0.18 + 0.05 * w.len() as f64;
        t.push(TimedWord::new(w.clone(), clock, clock + dur));
        clock += dur + if i % 9 == 8 { 0.6 } else { 0.08 };
    }
    t
}

#[test]
fn perfect_transcript_aligns_exactly() {
    let words = script_words();
    assert!(
        words.len() > 100,
        "fixture has {} dialogue words",
        words.len()
    );
    let t = truth(&words);
    let a = align_words(&words, &t);
    assert_eq!(a.len(), words.len());
    for (i, w) in a.iter().enumerate() {
        assert_eq!(w.word_index, i);
        assert_eq!(w.word, words[i]);
        assert!((w.confidence - 1.0).abs() < 1e-12);
        assert!((w.start_s - t[i].start_s).abs() < 1e-9);
        assert!((w.end_s - t[i].end_s).abs() < 1e-9);
    }
    for pair in a.windows(2) {
        assert!(pair[1].start_s >= pair[0].start_s);
        assert!(pair[0].end_s >= pair[0].start_s);
    }
    let frames = align_to_frames(&a, 24.0);
    assert_eq!(frames.len(), words.len());
    for (k, (idx, range)) in frames.iter().enumerate() {
        assert_eq!(*idx, k);
        assert!(range.duration.value() >= 1.0);
        assert_eq!(range.start_time.rate_hz(), 24.0);
    }
}

#[test]
fn corrupted_transcript_recovers_timings() {
    let words = script_words();
    let t = truth(&words);
    let mut rng = Rng(0x5eed);
    let mut corrupted: Vec<TimedWord> = Vec::new();
    let (mut dropped, mut altered, mut inserted) = (0, 0, 0);
    for w in &t {
        if rng.chance(10) {
            dropped += 1;
            continue;
        }
        let mut w = w.clone();
        if rng.chance(10) {
            altered += 1;
            let mut chars: Vec<char> = w.word.chars().collect();
            let pos = (rng.next() as usize) % chars.len();
            chars[pos] = if chars[pos] == 'x' { 'q' } else { 'x' };
            chars.push('s');
            w.word = chars.into_iter().collect();
        }
        corrupted.push(w.clone());
        if rng.chance(5) {
            inserted += 1;
            corrupted.push(TimedWord::new("um", w.end_s, w.end_s + 0.05));
        }
    }
    assert!(
        dropped > 5 && altered > 5 && inserted > 2,
        "{dropped} {altered} {inserted}"
    );

    let a = align_words(&words, &corrupted);
    assert_eq!(a.len(), words.len());
    let within = a
        .iter()
        .zip(t.iter())
        .filter(|(w, truth)| (w.start_s - truth.start_s).abs() <= 0.5)
        .count();
    let ratio = within as f64 / words.len() as f64;
    assert!(
        ratio >= 0.85,
        "only {within}/{} words within 0.5 s ({ratio:.3})",
        words.len()
    );
    // Dropped words get confidence 0 but still have finite, monotone times.
    assert!(a.iter().any(|w| w.confidence == 0.0));
    for pair in a.windows(2) {
        assert!(pair[1].start_s >= pair[0].start_s - 1e-9);
        assert!(pair[0].start_s.is_finite() && pair[0].end_s.is_finite());
    }
}
