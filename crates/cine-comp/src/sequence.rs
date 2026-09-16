//! Frame-numbered file paths: `plate.%04d.exr`, `plate.####.exr`, `plate.1001.exr`.
//!
//! An image sequence is a path with a frame number in it, and the two spellings
//! the industry actually uses are printf (`%04d`) and hashes (`####`). A path
//! with neither is a single file, used for every frame — which is what a still
//! reference is, and is not an error.
//!
//! Nothing here touches the filesystem: it is the naming rule alone, so it can
//! be tested without a disk and used to name a file that does not exist yet.

use std::path::{Path, PathBuf};

/// How a pattern carries its frame number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameToken {
    /// `%04d`, with the padding it asks for; `%d` is padding 1.
    Printf { at: usize, len: usize, pad: usize },
    /// A run of `#`, one per digit.
    Hash { at: usize, len: usize },
    /// No frame number: one file for every frame.
    None,
}

/// Find the frame token in `pattern`, if it has one.
pub fn token(pattern: &str) -> FrameToken {
    let bytes = pattern.as_bytes();
    if let Some(at) = pattern.find('%') {
        let rest = &bytes[at + 1..];
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if rest.get(digits) == Some(&b'd') {
            let pad = if digits == 0 {
                1
            } else {
                pattern[at + 1..at + 1 + digits].parse().unwrap_or(1)
            };
            return FrameToken::Printf {
                at,
                len: digits + 2,
                pad,
            };
        }
    }
    if let Some(at) = pattern.find('#') {
        let len = bytes[at..].iter().take_while(|b| **b == b'#').count();
        return FrameToken::Hash { at, len };
    }
    FrameToken::None
}

/// The path for one frame of a sequence.
pub fn frame_path(pattern: &str, frame: i64) -> PathBuf {
    let number = |pad: usize| {
        if frame < 0 {
            format!(
                "-{:0>pad$}",
                (-frame).to_string(),
                pad = pad.saturating_sub(1)
            )
        } else {
            format!("{frame:0>pad$}")
        }
    };
    match token(pattern) {
        FrameToken::None => PathBuf::from(pattern),
        FrameToken::Printf { at, len, pad } => PathBuf::from(format!(
            "{}{}{}",
            &pattern[..at],
            number(pad),
            &pattern[at + len..]
        )),
        FrameToken::Hash { at, len } => PathBuf::from(format!(
            "{}{}{}",
            &pattern[..at],
            number(len),
            &pattern[at + len..]
        )),
    }
}

/// Whether the pattern names a sequence rather than one file.
pub fn is_sequence(pattern: &str) -> bool {
    token(pattern) != FrameToken::None
}

/// The frames of a sequence that exist on disk, in order.
///
/// Scans the pattern's directory rather than probing frame by frame, so a
/// sequence that starts at 1001 is found without knowing where it starts.
pub fn frames_on_disk(pattern: &str) -> Vec<i64> {
    let t = token(pattern);
    let (at, len) = match t {
        FrameToken::None => return Vec::new(),
        FrameToken::Printf { at, len, .. } => (at, len),
        FrameToken::Hash { at, len } => (at, len),
    };
    let (prefix, suffix) = (&pattern[..at], &pattern[at + len..]);
    let dir = Path::new(prefix)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_prefix = Path::new(prefix)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut frames: Vec<i64> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let middle = name
                .strip_prefix(&file_prefix)?
                .strip_suffix(suffix.rsplit('/').next().unwrap_or(suffix))?;
            middle.parse::<i64>().ok()
        })
        .collect();
    frames.sort_unstable();
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_spellings_of_a_frame_number() {
        assert_eq!(
            frame_path("plate.%04d.exr", 7),
            PathBuf::from("plate.0007.exr")
        );
        assert_eq!(
            frame_path("plate.####.exr", 1001),
            PathBuf::from("plate.1001.exr")
        );
        assert_eq!(
            frame_path("plate.%d.exr", 12),
            PathBuf::from("plate.12.exr")
        );
        // More digits than the padding: the number wins, it is not truncated.
        assert_eq!(
            frame_path("plate.%02d.exr", 1234),
            PathBuf::from("plate.1234.exr")
        );
        // A path with no token is one file for every frame.
        assert_eq!(
            frame_path("reference.exr", 55),
            PathBuf::from("reference.exr")
        );
        assert!(!is_sequence("reference.exr"));
        assert!(is_sequence("plate.####.exr"));
    }

    #[test]
    fn frames_are_found_by_scanning_not_guessing() {
        let dir = std::env::temp_dir().join(format!("cine-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in [1001, 1002, 1005] {
            std::fs::write(dir.join(format!("plate.{f}.exr")), b"x").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let pattern = dir.join("plate.####.exr");
        let found = frames_on_disk(&pattern.to_string_lossy());
        assert_eq!(found, vec![1001, 1002, 1005], "the gap is real and is kept");
        std::fs::remove_dir_all(&dir).ok();
    }
}
