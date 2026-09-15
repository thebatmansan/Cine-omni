//! OCIO file rules: which colorspace a file is in, decided by its path.
//!
//! A config's `file_rules` are tried in order and the first that matches names
//! the colorspace; the last is always `Default`. The matching semantics here
//! were measured against OpenColorIO 2.5.2 (`tests/file_rules.rs`), because the
//! format's documentation does not pin them down:
//!
//! * A **pattern** rule matches the whole path against
//!   `^<pattern glob>\.<extension glob>$`. The pattern is case-sensitive; in
//!   the extension, plain letters match either case (`exr` matches `.EXR`) but
//!   letters inside `[...]` do not (`e[xy]` matches `.Ex`, not `.EX`). `*`
//!   crosses directory separators.
//! * A **regex** rule must match the whole path, not merely occur in it.
//! * **ColorSpaceNamePathSearch** picks the colorspace whose name or alias
//!   occurs in the path ending furthest right, case-insensitively, the longer
//!   name winning a tie; when none occurs, the next rule is tried.

use crate::error::{Error, Result};

/// One rule.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRule {
    pub name: String,
    pub kind: FileRuleKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileRuleKind {
    /// Glob over the path, and glob over its extension.
    Pattern {
        colorspace: String,
        pattern: String,
        extension: String,
    },
    /// Regular expression over the whole path.
    Regex { colorspace: String, regex: String },
    /// Search the path for colorspace names.
    ColorSpaceNamePathSearch,
    /// Matches everything; always last.
    Default { colorspace: String },
}

/// Which rule decided, and what it decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleMatch {
    pub rule: String,
    pub colorspace: String,
}

fn glob_to_regex(glob: &str, fold_letters: bool) -> String {
    let mut out = String::new();
    let mut in_class = false;
    for ch in glob.chars() {
        match ch {
            '[' if !in_class => {
                in_class = true;
                out.push('[');
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            _ if in_class => out.push(ch),
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c if fold_letters && c.is_ascii_alphabetic() => {
                out.push('[');
                out.push(c.to_ascii_lowercase());
                out.push(c.to_ascii_uppercase());
                out.push(']');
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out
}

fn full_match(re: &str, path: &str) -> Result<bool> {
    let anchored = format!("^(?:{re})$");
    regex::Regex::new(&anchored)
        .map(|r| r.is_match(path))
        .map_err(|e| Error::Invalid(format!("file rule regex `{re}`: {e}")))
}

impl FileRule {
    /// The colorspace this rule assigns to `path`, if it matches. `names`
    /// are every colorspace's name and aliases, paired with the name, for the
    /// path-search rule.
    pub fn apply(&self, path: &str, names: &[(String, String)]) -> Result<Option<String>> {
        Ok(match &self.kind {
            FileRuleKind::Default { colorspace } => Some(colorspace.clone()),
            FileRuleKind::Regex { colorspace, regex } => {
                full_match(regex, path)?.then(|| colorspace.clone())
            }
            FileRuleKind::Pattern {
                colorspace,
                pattern,
                extension,
            } => {
                let re = format!(
                    "{}\\.{}",
                    glob_to_regex(pattern, false),
                    glob_to_regex(extension, true)
                );
                full_match(&re, path)?.then(|| colorspace.clone())
            }
            FileRuleKind::ColorSpaceNamePathSearch => {
                let lower = path.to_lowercase();
                let mut best: Option<(usize, usize, &str)> = None;
                for (candidate, colorspace) in names {
                    let needle = candidate.to_lowercase();
                    if needle.is_empty() {
                        continue;
                    }
                    if let Some(at) = lower.rfind(&needle) {
                        let end = at + needle.len();
                        let better = match best {
                            None => true,
                            Some((e, len, _)) => end > e || (end == e && needle.len() > len),
                        };
                        if better {
                            best = Some((end, needle.len(), colorspace.as_str()));
                        }
                    }
                }
                best.map(|(_, _, c)| c.to_string())
            }
        })
    }

    /// The colorspace this rule names, if it names one.
    pub fn colorspace(&self) -> Option<&str> {
        match &self.kind {
            FileRuleKind::Pattern { colorspace, .. }
            | FileRuleKind::Regex { colorspace, .. }
            | FileRuleKind::Default { colorspace } => Some(colorspace),
            FileRuleKind::ColorSpaceNamePathSearch => None,
        }
    }
}

/// The first rule matching `path`.
pub fn match_path(
    rules: &[FileRule],
    path: &str,
    names: &[(String, String)],
) -> Result<Option<RuleMatch>> {
    for rule in rules {
        if let Some(colorspace) = rule.apply(path, names)? {
            return Ok(Some(RuleMatch {
                rule: rule.name.clone(),
                colorspace,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_fold_plain_letters_only() {
        assert_eq!(glob_to_regex("exr", true), "[eE][xX][rR]");
        assert_eq!(glob_to_regex("e[xy]", true), "[eE][xy]");
        assert_eq!(glob_to_regex("*shot?.", false), ".*shot.\\.");
    }
}
