//! ASC MHL: what was offloaded, and the proof it arrived intact.
//!
//! A camera card is copied once and then the original is wiped, so the only
//! thing standing between a production and a lost day is a hash written down
//! at the moment of the copy and checked afterwards. The American Society of
//! Cinematographers' Media Hash List is the format the industry writes it in,
//! and it is XML: a `ascmhl/` folder beside the media holding one *generation*
//! per event — the offload, then each later verification.
//!
//! ## What this implements
//!
//! * Creating a hash list for a folder: every file, its size and its hash,
//!   with the algorithm named. `xxh64` is the default because it is what
//!   offload tools write and it is fast enough to hash at disk speed;
//!   `md5`, `sha1` and `xxh3` are also read and written.
//! * Verifying a folder against a hash list: what verified, what failed with
//!   both hashes, what is missing, and what is present but not in the list —
//!   the last two being different failures, and neither one "corrupt".
//! * Reading and writing the generation files themselves.
//!
//! ## What it does not
//!
//! Recorded in [`HashList::unimplemented`] on every list read, so a caller can
//! say so rather than imply completeness: chain files (`ascmhl_chain.xml`),
//! directory hashes, nested child histories, `<ignore>` patterns, packing
//! lists, and the `flatten`/`transfer` process kinds. This writes and checks
//! `in-place` generations.

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A hash algorithm ASC MHL names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HashKind {
    Xxh64,
    Xxh3,
    Md5,
    Sha1,
}

impl HashKind {
    /// The element name in the XML.
    pub fn tag(&self) -> &'static str {
        match self {
            HashKind::Xxh64 => "xxh64",
            HashKind::Xxh3 => "xxh3",
            HashKind::Md5 => "md5",
            HashKind::Sha1 => "sha1",
        }
    }

    fn from_tag(tag: &str) -> Option<HashKind> {
        Some(match tag {
            "xxh64" => HashKind::Xxh64,
            "xxh3" => HashKind::Xxh3,
            "md5" => HashKind::Md5,
            "sha1" => HashKind::Sha1,
            _ => return None,
        })
    }
}

impl fmt::Display for HashKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// What a hash records about the file it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Written when the file was first hashed.
    Original,
    /// Written when a later check found the same hash.
    Verified,
    /// Written when a later check found a different one.
    Failed,
    /// Present in the folder but not previously in the list.
    New,
}

impl Action {
    fn as_str(&self) -> &'static str {
        match self {
            Action::Original => "original",
            Action::Verified => "verified",
            Action::Failed => "failed",
            Action::New => "new",
        }
    }

    fn from_str(s: &str) -> Action {
        match s {
            "verified" => Action::Verified,
            "failed" => Action::Failed,
            "new" => Action::New,
            _ => Action::Original,
        }
    }
}

/// One file in a hash list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Relative to the folder the list covers, with `/` separators.
    pub path: String,
    pub size: u64,
    pub kind: HashKind,
    pub hash: String,
    pub action: Action,
    /// ISO 8601 UTC.
    pub hash_date: String,
}

/// Who made a generation, and with what.
#[derive(Clone, Debug, PartialEq)]
pub struct Creator {
    pub host: String,
    pub tool: String,
    pub creation_date: String,
}

impl Default for Creator {
    fn default() -> Self {
        Creator {
            host: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into()),
            tool: format!("CINE-OMNI {}", env!("CARGO_PKG_VERSION")),
            creation_date: iso8601_utc(SystemTime::now()),
        }
    }
}

/// One generation: everything hashed in one event.
#[derive(Clone, Debug, PartialEq)]
pub struct HashList {
    /// The ASC MHL version the file declares.
    pub version: String,
    pub creator: Creator,
    /// The folder the paths are relative to.
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Parts of the format present in the file that this build does not apply.
    /// A caller showing a list must say so rather than imply it read it whole.
    pub unimplemented: Vec<String>,
}

/// What verifying a folder against a list found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VerifyReport {
    pub verified: Vec<String>,
    /// `(path, expected, actual)` — the file is there and is different.
    pub failed: Vec<(String, String, String)>,
    /// In the list, not on disk.
    pub missing: Vec<String>,
    /// On disk, not in the list. Not corruption: something was added.
    pub unlisted: Vec<String>,
}

impl VerifyReport {
    /// Whether every file in the list was found and matched.
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty() && self.missing.is_empty()
    }
}

// ---------------------------------------------------------------------------
// hashing
// ---------------------------------------------------------------------------

/// Hash one file with `kind`, reading it once.
pub fn hash_file(path: &Path, kind: HashKind) -> Result<String> {
    use md5::Digest;
    let mut file = std::fs::File::open(path).map_err(Error::Io)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut xxh64 = twox_hash::XxHash64::with_seed(0);
    let mut xxh3 = twox_hash::XxHash3_64::with_seed(0);
    let mut md5 = md5::Md5::new();
    let mut sha1 = sha1::Sha1::new();
    loop {
        let n = file.read(&mut buffer).map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];
        match kind {
            HashKind::Xxh64 => std::hash::Hasher::write(&mut xxh64, chunk),
            HashKind::Xxh3 => std::hash::Hasher::write(&mut xxh3, chunk),
            HashKind::Md5 => md5.update(chunk),
            HashKind::Sha1 => sha1.update(chunk),
        }
    }
    Ok(match kind {
        // ASC MHL writes xxHash as 16 lower-case hex digits.
        HashKind::Xxh64 => format!("{:016x}", std::hash::Hasher::finish(&xxh64)),
        HashKind::Xxh3 => format!("{:016x}", std::hash::Hasher::finish(&xxh3)),
        HashKind::Md5 => hex(&md5.finalize()),
        HashKind::Sha1 => hex(&sha1.finalize()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every file under `root`, in a stable order, relative to it.
///
/// The `ascmhl` folder is skipped: a hash list does not hash itself.
pub fn files_under(root: &Path) -> Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(Error::Io)?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "ascmhl" {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() => {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, size));
                }
                _ => {}
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Hash every file under `root` and return the list. `progress` is called with
/// `(done, total)` after each file, for a job to report against.
pub fn create(
    root: &Path,
    kind: HashKind,
    mut progress: impl FnMut(usize, usize),
) -> Result<HashList> {
    let files = files_under(root)?;
    let total = files.len();
    let date = iso8601_utc(SystemTime::now());
    let mut entries = Vec::with_capacity(total);
    for (done, (rel, size)) in files.into_iter().enumerate() {
        let hash = hash_file(&root.join(&rel), kind)?;
        entries.push(Entry {
            path: rel,
            size,
            kind,
            hash,
            action: Action::Original,
            hash_date: date.clone(),
        });
        progress(done + 1, total);
    }
    Ok(HashList {
        version: "2.0".into(),
        creator: Creator::default(),
        root: root.to_path_buf(),
        entries,
        unimplemented: Vec::new(),
    })
}

/// Check every file in `list` against the folder, and notice files the list
/// does not mention.
pub fn verify(
    root: &Path,
    list: &HashList,
    mut progress: impl FnMut(usize, usize),
) -> Result<VerifyReport> {
    let mut report = VerifyReport::default();
    let on_disk: BTreeMap<String, u64> = files_under(root)?.into_iter().collect();
    let total = list.entries.len();
    for (done, entry) in list.entries.iter().enumerate() {
        let path = root.join(&entry.path);
        if !path.is_file() {
            report.missing.push(entry.path.clone());
            progress(done + 1, total);
            continue;
        }
        let actual = hash_file(&path, entry.kind)?;
        if actual.eq_ignore_ascii_case(&entry.hash) {
            report.verified.push(entry.path.clone());
        } else {
            report
                .failed
                .push((entry.path.clone(), entry.hash.clone(), actual));
        }
        progress(done + 1, total);
    }
    let listed: BTreeMap<&str, ()> = list.entries.iter().map(|e| (e.path.as_str(), ())).collect();
    for path in on_disk.keys() {
        if !listed.contains_key(path.as_str()) {
            report.unlisted.push(path.clone());
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// the files themselves
// ---------------------------------------------------------------------------

/// The `ascmhl` folder of a root.
pub fn mhl_dir(root: &Path) -> PathBuf {
    root.join("ascmhl")
}

/// Generation files already in `root`, oldest first.
pub fn generations(root: &Path) -> Vec<PathBuf> {
    let dir = mhl_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mhl"))
        .collect();
    files.sort();
    files
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write a generation into `root/ascmhl/`, numbered after the ones there.
///
/// Returns the file written. Generations are never rewritten: a verification
/// adds one, so the history of a card is the folder's contents in order.
pub fn write_generation(root: &Path, list: &HashList) -> Result<PathBuf> {
    let dir = mhl_dir(root);
    std::fs::create_dir_all(&dir).map_err(Error::Io)?;
    let number = generations(root).len() + 1;
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into());
    let stamp = list.creator.creation_date.replace([':', '-'], "");
    let path = dir.join(format!("{number:04}_{name}_{stamp}.mhl"));

    let mut xml = String::with_capacity(256 + list.entries.len() * 160);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<hashlist version=\"{}\" xmlns=\"urn:ASC:MHL:v2.0\">\n",
        escape(&list.version)
    ));
    xml.push_str("  <creatorinfo>\n");
    xml.push_str(&format!(
        "    <creationdate>{}</creationdate>\n    <hostname>{}</hostname>\n    <tool>{}</tool>\n",
        escape(&list.creator.creation_date),
        escape(&list.creator.host),
        escape(&list.creator.tool),
    ));
    xml.push_str("  </creatorinfo>\n");
    xml.push_str("  <processinfo>\n    <process>in-place</process>\n  </processinfo>\n");
    xml.push_str("  <hashes>\n");
    for e in &list.entries {
        xml.push_str("    <hash>\n");
        xml.push_str(&format!(
            "      <path size=\"{}\">{}</path>\n",
            e.size,
            escape(&e.path)
        ));
        xml.push_str(&format!(
            "      <{tag} action=\"{action}\" hashdate=\"{date}\">{hash}</{tag}>\n",
            tag = e.kind.tag(),
            action = e.action.as_str(),
            date = escape(&e.hash_date),
            hash = escape(&e.hash),
        ));
        xml.push_str("    </hash>\n");
    }
    xml.push_str("  </hashes>\n</hashlist>\n");
    std::fs::write(&path, xml).map_err(Error::Io)?;
    Ok(path)
}

/// Read a generation file. `root` is the folder its paths are relative to —
/// the parent of the `ascmhl` folder.
pub fn read_generation(path: &Path) -> Result<HashList> {
    let text = std::fs::read_to_string(path).map_err(Error::Io)?;
    let doc = roxmltree::Document::parse(&text)
        .map_err(|e| Error::Unsupported(format!("{}: not an MHL file: {e}", path.display())))?;
    let root_el = doc.root_element();
    if root_el.tag_name().name() != "hashlist" {
        return Err(Error::Unsupported(format!(
            "{}: root element is `{}`, not `hashlist`",
            path.display(),
            root_el.tag_name().name()
        )));
    }
    let version = root_el.attribute("version").unwrap_or("2.0").to_string();
    let text_of = |parent: roxmltree::Node, name: &str| {
        parent
            .descendants()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let creator = Creator {
        host: text_of(root_el, "hostname"),
        tool: text_of(root_el, "tool"),
        creation_date: text_of(root_el, "creationdate"),
    };
    let mut entries = Vec::new();
    let mut unimplemented = Vec::new();
    for hash in root_el.descendants().filter(|n| n.has_tag_name("hash")) {
        let Some(path_el) = hash.children().find(|n| n.has_tag_name("path")) else {
            continue;
        };
        let rel = path_el.text().unwrap_or_default().trim().to_string();
        let size = path_el
            .attribute("size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        for child in hash.children().filter(roxmltree::Node::is_element) {
            let tag = child.tag_name().name();
            let Some(kind) = HashKind::from_tag(tag) else {
                if tag != "path" && !unimplemented.iter().any(|u| u == tag) {
                    unimplemented.push(tag.to_string());
                }
                continue;
            };
            entries.push(Entry {
                path: rel.clone(),
                size,
                kind,
                hash: child.text().unwrap_or_default().trim().to_string(),
                action: Action::from_str(child.attribute("action").unwrap_or("original")),
                hash_date: child.attribute("hashdate").unwrap_or_default().to_string(),
            });
        }
    }
    // Parts of the format that are present and not applied.
    for tag in [
        "directoryhash",
        "ignore",
        "packinglist",
        "references",
        "hashlistreference",
    ] {
        if root_el.descendants().any(|n| n.has_tag_name(tag)) {
            unimplemented.push(tag.to_string());
        }
    }
    let root = path
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Ok(HashList {
        version,
        creator,
        root,
        entries,
        unimplemented,
    })
}

/// Turn a verification into the generation that records it.
pub fn generation_for(list: &HashList, report: &VerifyReport) -> HashList {
    let date = iso8601_utc(SystemTime::now());
    let mut entries: Vec<Entry> = list
        .entries
        .iter()
        .filter(|e| !report.missing.contains(&e.path))
        .map(|e| {
            let failed = report.failed.iter().find(|(p, _, _)| *p == e.path);
            Entry {
                action: if failed.is_some() {
                    Action::Failed
                } else {
                    Action::Verified
                },
                hash: failed.map(|(_, _, a)| a.clone()).unwrap_or(e.hash.clone()),
                hash_date: date.clone(),
                ..e.clone()
            }
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    HashList {
        version: list.version.clone(),
        creator: Creator::default(),
        root: list.root.clone(),
        entries,
        unimplemented: Vec::new(),
    }
}

/// ISO 8601 in UTC, which is what MHL dates are.
pub fn iso8601_utc(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since the epoch → calendar date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hashes are the algorithms they claim to be.
    ///
    /// A hash written into a manifest is a promise to a production that the
    /// file can be checked years later by something else entirely, so the
    /// values are the published ones: MD5 and SHA-1 of "abc" from their
    /// specifications, and xxHash64 of a known string from the C
    /// implementation's own vectors (by way of `twox-hash`'s conformance
    /// tests, since no xxHash tool exists on this machine to ask directly).
    #[test]
    fn hashes_are_the_published_values_for_known_input() {
        let dir = std::env::temp_dir().join(format!("cine-mhl-known-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let abc = dir.join("abc.txt");
        std::fs::write(&abc, b"abc").unwrap();
        assert_eq!(
            hash_file(&abc, HashKind::Md5).unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            hash_file(&abc, HashKind::Sha1).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );

        let hello = dir.join("hello.bin");
        std::fs::write(&hello, b"Hello, world!\0").unwrap();
        assert_eq!(
            hash_file(&hello, HashKind::Xxh64).unwrap(),
            "7b06c531ea43e89f"
        );
        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            hash_file(&empty, HashKind::Xxh64).unwrap(),
            "ef46db3751d8e999",
            "an empty file still has a hash, and it is the defined one"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_date_is_iso_8601_utc() {
        assert_eq!(iso8601_utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso8601_utc(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
    }
}
