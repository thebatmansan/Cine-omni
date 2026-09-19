//! Where a project records that a media file is, and whether that is a place
//! this machine can open.
//!
//! A project stores one string per asset. Until now everything read it as a
//! filesystem path — `PathBuf::from(recorded)` — which is right for media this
//! build ingested and wrong for media that arrived in a document written
//! somewhere else. An OTIO clip's `target_url` is a URL; a cut conformed from
//! another facility is full of `file:///media/A001_C003.mov`, and an AAF or an
//! EDL may carry a volume name or a reel rather than a path at all.
//!
//! `PathBuf::from("file:///media/A001_C003.mov")` is a relative path called
//! `file:` with two empty components. It does not exist, so every check
//! answered "not there" — the right conclusion for the wrong reason, and the
//! reason is what a person needs in order to act. "That file has gone" and
//! "that is not a place on this machine" call for different work.
//!
//! So a recorded location is parsed once, here, into what it actually is.
//!
//! ## What this is not
//!
//! It is not a resolver. Nothing here searches for media, mounts a volume or
//! contacts a server; [`crate::identity`] and relinking do that, by content.
//! This only says what kind of location was written down, and hands back a
//! filesystem path when there is one.

use std::path::{Path, PathBuf};

/// A location as a project recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Locator {
    /// A path on this machine, either written as one or decoded from a
    /// `file://` URL.
    Path(PathBuf),
    /// A location this build cannot open: a remote URL, or a scheme it does
    /// not know. Carried verbatim, because it is what the document said and
    /// throwing it away would lose the only clue to where the media lives.
    Elsewhere {
        /// The scheme, lower-cased — `http`, `s3`, `omf`.
        scheme: String,
        /// The whole locator, exactly as recorded.
        recorded: String,
    },
    /// Nothing was recorded.
    Nothing,
}

impl Locator {
    /// Read a recorded location.
    pub fn parse(recorded: &str) -> Locator {
        let trimmed = recorded.trim();
        if trimmed.is_empty() {
            return Locator::Nothing;
        }
        let Some((scheme, rest)) = trimmed.split_once("://") else {
            // No scheme: a path, which is what this build writes itself.
            return Locator::Path(PathBuf::from(trimmed));
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "file" {
            return Locator::Elsewhere {
                scheme,
                recorded: trimmed.to_string(),
            };
        }
        // `file://host/path`: an empty host and `localhost` both mean here.
        // Any other host is another machine, and this one cannot open it.
        let (host, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        };
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return Locator::Elsewhere {
                scheme,
                recorded: trimmed.to_string(),
            };
        }
        if path.is_empty() {
            return Locator::Nothing;
        }
        Locator::Path(PathBuf::from(percent_decode(path)))
    }

    /// The filesystem path, when the location is one.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Locator::Path(p) => Some(p),
            _ => None,
        }
    }

    /// What was recorded, for showing a person.
    pub fn display(&self) -> String {
        match self {
            Locator::Path(p) => p.display().to_string(),
            Locator::Elsewhere { recorded, .. } => recorded.clone(),
            Locator::Nothing => String::new(),
        }
    }

    /// Why this location cannot be opened on this machine, if it cannot.
    pub fn why_not_local(&self) -> Option<String> {
        match self {
            Locator::Path(_) => None,
            Locator::Nothing => Some("the project records no location for it".into()),
            Locator::Elsewhere { scheme, .. } => Some(format!(
                "it is recorded as a `{scheme}:` location, which is not a file on this machine; \
                 relink it to where its content actually is"
            )),
        }
    }
}

/// Decode `%20` and friends. Anything malformed is left as it stands, because
/// a path that already contains a per-cent sign is likelier than a truncated
/// escape, and inventing bytes would be worse than keeping the string.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_path_is_a_path() {
        assert_eq!(
            Locator::parse("/card/A001C001.mov").as_path(),
            Some(Path::new("/card/A001C001.mov"))
        );
        assert_eq!(
            Locator::parse("  relative/take.mov  ").as_path(),
            Some(Path::new("relative/take.mov")),
            "surrounding space is not part of a path"
        );
    }

    #[test]
    fn a_file_url_is_the_path_it_names() {
        // The case that made every OTIO-imported clip read as missing.
        assert_eq!(
            Locator::parse("file:///media/A001_C003.mov").as_path(),
            Some(Path::new("/media/A001_C003.mov"))
        );
        assert_eq!(
            Locator::parse("file://localhost/media/x.mov").as_path(),
            Some(Path::new("/media/x.mov"))
        );
        assert_eq!(
            Locator::parse("file:///cards/day%201/A001%20C001.mov").as_path(),
            Some(Path::new("/cards/day 1/A001 C001.mov")),
            "escapes are decoded"
        );
    }

    #[test]
    fn a_location_this_machine_cannot_open_says_which_kind() {
        for (recorded, scheme) in [
            ("https://vault.example.com/a.mov", "https"),
            ("s3://bucket/key.mov", "s3"),
            ("file://otherbox/media/a.mov", "file"),
        ] {
            let l = Locator::parse(recorded);
            assert_eq!(l.as_path(), None, "{recorded}");
            assert_eq!(
                l,
                Locator::Elsewhere {
                    scheme: scheme.into(),
                    recorded: recorded.into()
                }
            );
            assert!(l.why_not_local().unwrap().contains(scheme), "{recorded}");
            assert_eq!(l.display(), recorded, "what was recorded is kept");
        }
    }

    #[test]
    fn nothing_recorded_is_its_own_answer() {
        assert_eq!(Locator::parse(""), Locator::Nothing);
        assert_eq!(Locator::parse("   "), Locator::Nothing);
        assert!(Locator::parse("")
            .why_not_local()
            .unwrap()
            .contains("no location"));
    }

    #[test]
    fn a_path_holding_a_per_cent_sign_survives() {
        assert_eq!(
            Locator::parse("/card/100%_take.mov").as_path(),
            Some(Path::new("/card/100%_take.mov")),
            "a malformed escape is left alone rather than guessed at"
        );
    }
}
