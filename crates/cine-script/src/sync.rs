//! ScriptSync: align script words to a timed transcript.
//!
//! [`align_words`] runs a global sequence alignment (Needleman–Wunsch with
//! affine gap penalties, i.e. Gotoh's algorithm) between the script words and
//! the words of a speech-to-text transcript. The match score is derived from
//! the normalized Levenshtein similarity of the two words, so substitutions
//! (mis-recognised words) still align to the right position. Script words
//! that end up unmatched (dropped by the recogniser) receive timings
//! interpolated between their matched neighbours and confidence 0.
//!
//! Complexity is O(n·m) in time and memory, which is fine for the per-scene
//! and per-dialogue sizes the pipeline uses (hundreds to a few thousand
//! words).

use crate::stats::word_list;
use ucg_core::{RationalTime, TimeRange};
use ucg_schema::WordAlignment;

/// A recognised word with its timing in seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedWord {
    pub word: String,
    pub start_s: f64,
    pub end_s: f64,
}

impl TimedWord {
    pub fn new(word: impl Into<String>, start_s: f64, end_s: f64) -> Self {
        TimedWord {
            word: word.into(),
            start_s,
            end_s,
        }
    }
}

/// Score for an aligned pair: `2 * similarity - 1`, in `[-1, 1]`.
const MATCH_SCALE: f64 = 2.0;
/// Cost of opening a gap (an unmatched word on either side).
pub const GAP_OPEN: f64 = -0.9;
/// Cost of extending a gap by one more word.
pub const GAP_EXTEND: f64 = -0.35;
/// Assumed spoken duration of a word with no timing information (seconds).
pub const DEFAULT_WORD_SECONDS: f64 = 0.3;

/// Levenshtein edit distance between two strings (by Unicode scalar).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Normalize a word the same way [`word_list`] does (lowercase, punctuation
/// stripped, inner apostrophes kept).
pub fn normalize_word(w: &str) -> String {
    word_list(w).join("")
}

/// Similarity in `[0, 1]` based on normalized Levenshtein distance.
pub fn word_similarity(a: &str, b: &str) -> f64 {
    let a = normalize_word(a);
    let b = normalize_word(b);
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 1.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / max as f64
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    M,
    X, // gap in transcript: script word i unmatched
    Y, // gap in script: transcript word j skipped
}

/// Align `script_words` to `transcript` and return one [`WordAlignment`] per
/// script word, in script order.
pub fn align_words(script_words: &[String], transcript: &[TimedWord]) -> Vec<WordAlignment> {
    let n = script_words.len();
    let m = transcript.len();
    if n == 0 {
        return Vec::new();
    }
    // matched[i] = Some(j) when script word i aligns with transcript word j.
    let mut matched: Vec<Option<usize>> = vec![None; n];

    if m > 0 {
        let w = m + 1;
        let neg = f64::NEG_INFINITY;
        let mut sm = vec![neg; (n + 1) * w];
        let mut sx = vec![neg; (n + 1) * w];
        let mut sy = vec![neg; (n + 1) * w];
        // Traceback: which state each cell came from.
        let mut tm = vec![State::M; (n + 1) * w];
        let mut tx = vec![State::M; (n + 1) * w];
        let mut ty = vec![State::M; (n + 1) * w];
        let idx = |i: usize, j: usize| i * w + j;

        sm[idx(0, 0)] = 0.0;
        for i in 1..=n {
            sx[idx(i, 0)] = GAP_OPEN + GAP_EXTEND * (i as f64 - 1.0);
            tx[idx(i, 0)] = if i == 1 { State::M } else { State::X };
        }
        for j in 1..=m {
            sy[idx(0, j)] = GAP_OPEN + GAP_EXTEND * (j as f64 - 1.0);
            ty[idx(0, j)] = if j == 1 { State::M } else { State::Y };
        }
        let sims: Vec<f64> = (0..n)
            .flat_map(|i| (0..m).map(move |j| (i, j)))
            .map(|(i, j)| word_similarity(&script_words[i], &transcript[j].word))
            .collect();

        for i in 1..=n {
            for j in 1..=m {
                let s = MATCH_SCALE * sims[(i - 1) * m + (j - 1)] - 1.0;
                // M: align i with j.
                let (best, from) = best3(
                    sm[idx(i - 1, j - 1)],
                    sx[idx(i - 1, j - 1)],
                    sy[idx(i - 1, j - 1)],
                );
                sm[idx(i, j)] = best + s;
                tm[idx(i, j)] = from;
                // X: script word i unmatched (move down).
                let (best, from) = best3(
                    sm[idx(i - 1, j)] + GAP_OPEN,
                    sx[idx(i - 1, j)] + GAP_EXTEND,
                    sy[idx(i - 1, j)] + GAP_OPEN,
                );
                sx[idx(i, j)] = best;
                tx[idx(i, j)] = from;
                // Y: transcript word j skipped (move right).
                let (best, from) = best3(
                    sm[idx(i, j - 1)] + GAP_OPEN,
                    sx[idx(i, j - 1)] + GAP_OPEN,
                    sy[idx(i, j - 1)] + GAP_EXTEND,
                );
                sy[idx(i, j)] = best;
                ty[idx(i, j)] = from;
            }
        }

        // Traceback from (n, m).
        let (_, mut state) = best3(sm[idx(n, m)], sx[idx(n, m)], sy[idx(n, m)]);
        let (mut i, mut j) = (n, m);
        while i > 0 || j > 0 {
            match state {
                State::M => {
                    if i == 0 || j == 0 {
                        break;
                    }
                    matched[i - 1] = Some(j - 1);
                    state = tm[idx(i, j)];
                    i -= 1;
                    j -= 1;
                }
                State::X => {
                    if i == 0 {
                        break;
                    }
                    state = tx[idx(i, j)];
                    i -= 1;
                }
                State::Y => {
                    if j == 0 {
                        break;
                    }
                    state = ty[idx(i, j)];
                    j -= 1;
                }
            }
        }
    }

    // Build the output, interpolating timings for unmatched words.
    let mut out: Vec<WordAlignment> = Vec::with_capacity(n);
    for (i, word) in script_words.iter().enumerate() {
        match matched[i] {
            Some(j) => {
                let t = &transcript[j];
                out.push(WordAlignment {
                    word_index: i,
                    word: word.clone(),
                    start_s: t.start_s,
                    end_s: t.end_s.max(t.start_s),
                    confidence: word_similarity(word, &t.word).clamp(0.0, 1.0),
                });
            }
            None => out.push(WordAlignment {
                word_index: i,
                word: word.clone(),
                start_s: f64::NAN,
                end_s: f64::NAN,
                confidence: 0.0,
            }),
        }
    }
    interpolate_gaps(&mut out);
    out
}

fn best3(m: f64, x: f64, y: f64) -> (f64, State) {
    if m >= x && m >= y {
        (m, State::M)
    } else if x >= y {
        (x, State::X)
    } else {
        (y, State::Y)
    }
}

/// Fill NaN timings by spreading unmatched words evenly between their
/// nearest timed neighbours (or extrapolating at either end).
fn interpolate_gaps(words: &mut [WordAlignment]) {
    let n = words.len();
    let mut i = 0;
    while i < n {
        if !words[i].start_s.is_nan() {
            i += 1;
            continue;
        }
        let gap_start = i;
        while i < n && words[i].start_s.is_nan() {
            i += 1;
        }
        let gap_end = i; // exclusive
        let count = (gap_end - gap_start) as f64;
        let prev_end = if gap_start > 0 {
            Some(words[gap_start - 1].end_s)
        } else {
            None
        };
        let next_start = if gap_end < n {
            Some(words[gap_end].start_s)
        } else {
            None
        };
        let (t0, t1) = match (prev_end, next_start) {
            (Some(a), Some(b)) if b > a => (a, b),
            (Some(a), Some(_)) => (a, a),
            (Some(a), None) => (a, a + count * DEFAULT_WORD_SECONDS),
            (None, Some(b)) => ((b - count * DEFAULT_WORD_SECONDS).max(0.0), b),
            (None, None) => (0.0, count * DEFAULT_WORD_SECONDS),
        };
        let slot = (t1 - t0) / count;
        for (k, w) in words[gap_start..gap_end].iter_mut().enumerate() {
            w.start_s = t0 + slot * k as f64;
            w.end_s = t0 + slot * (k as f64 + 1.0);
        }
    }
}

/// Convert an alignment to frame ranges at `rate` fps: `(word_index, range)`.
/// Starts are floored and ends are ceiled to whole frames; every word spans
/// at least one frame.
pub fn align_to_frames(alignment: &[WordAlignment], rate: f64) -> Vec<(usize, TimeRange)> {
    let rate = if rate > 0.0 { rate } else { 24.0 };
    alignment
        .iter()
        .map(|w| {
            let start = (w.start_s * rate).floor().max(0.0);
            let end = (w.end_s * rate).ceil().max(start + 1.0);
            (
                w.word_index,
                TimeRange::new(
                    RationalTime::new(start, rate),
                    RationalTime::new(end - start, rate),
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        word_list(s)
    }

    fn timed(ws: &[String]) -> Vec<TimedWord> {
        ws.iter()
            .enumerate()
            .map(|(i, w)| TimedWord::new(w.clone(), i as f64 * 0.4, i as f64 * 0.4 + 0.3))
            .collect()
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert!((word_similarity("Hello,", "hello") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn perfect_alignment() {
        let s = words("we ride at dawn and we do not look back");
        let a = align_words(&s, &timed(&s));
        assert_eq!(a.len(), s.len());
        for (i, w) in a.iter().enumerate() {
            assert_eq!(w.word_index, i);
            assert!((w.confidence - 1.0).abs() < 1e-12);
            assert!((w.start_s - i as f64 * 0.4).abs() < 1e-9);
        }
    }

    #[test]
    fn deletion_insertion_substitution() {
        let s = words("the quick brown fox jumps over the lazy dog tonight");
        let mut t = timed(&s);
        t.remove(3); // drop "fox"
        t[4].word = "lazzy".into(); // was index 5 before removal? careful: after removal index 4 = "over"
        t.insert(1, TimedWord::new("uh", 0.35, 0.38)); // insertion
        let a = align_words(&s, &t);
        assert_eq!(a.len(), s.len());
        // "fox" is unmatched and interpolated between "brown" (ends 1.1) and "jumps" (starts 1.6).
        let fox = &a[3];
        assert_eq!(fox.confidence, 0.0);
        assert!(fox.start_s >= 1.1 && fox.end_s <= 1.6 + 1e-9);
        for w in a.windows(2) {
            assert!(w[1].start_s >= w[0].start_s - 1e-9);
        }
        let frames = align_to_frames(&a, 24.0);
        assert_eq!(frames.len(), s.len());
        assert!(frames.iter().all(|(_, r)| r.duration.value >= 1.0));
    }

    #[test]
    fn empty_inputs() {
        assert!(align_words(&[], &[]).is_empty());
        let a = align_words(&words("a b c"), &[]);
        assert_eq!(a.len(), 3);
        assert!(a.iter().all(|w| w.confidence == 0.0 && !w.start_s.is_nan()));
    }
}
