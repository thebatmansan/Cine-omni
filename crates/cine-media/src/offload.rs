//! Offload: copying a camera card so that it is provably the same afterwards.
//!
//! The card is the only copy until this finishes, so the order of operations is
//! the whole point:
//!
//! 1. every file is read once and hashed **as it is read**, so the hash is of
//!    what was actually on the card at that moment;
//! 2. the copy is written and flushed to disk;
//! 3. the copy is flushed and `fsync`ed, then read back and hashed again;
//! 4. the two hashes are compared, and an [`ascmhl`](crate::mhl) generation is
//!    written into the destination recording the result.
//!
//! What step 3 proves, exactly: the bytes reached the filesystem, were
//! acknowledged by the device (`fsync`), and read back identical. The read may
//! still be served from the operating system's cache — reading around it needs
//! `O_DIRECT` or dropping caches, neither of which is portable — so this
//! catches a truncated copy, a wrong-file copy and corruption in transit, and
//! does not by itself prove the data survives a power cut. A later `cine
//! verify` on a cold machine does.
//!
//! A destination file that already exists is never overwritten silently: the
//! offload refuses and names it, because the one thing worse than a failed
//! offload is one that destroyed the previous one.

use crate::error::{Error, Result};
use crate::mhl::{self, Action, Creator, Entry, HashKind, HashList};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// What an offload did, file by file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OffloadReport {
    /// Files copied and verified: `(relative path, bytes, hash)`.
    pub copied: Vec<(String, u64, String)>,
    /// Files whose copy did not match the source: `(path, source, copy)`.
    pub mismatched: Vec<(String, String, String)>,
    /// Files already present at the destination, left untouched.
    pub refused: Vec<String>,
    pub bytes: u64,
    /// The generation written into the destination.
    pub hash_list: Option<PathBuf>,
    pub took: std::time::Duration,
}

impl OffloadReport {
    /// Whether every file arrived and matched.
    pub fn is_clean(&self) -> bool {
        self.mismatched.is_empty() && self.refused.is_empty()
    }

    /// Throughput of the copy, counting both the read and the verification.
    pub fn megabytes_per_second(&self) -> f64 {
        let secs = self.took.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.bytes as f64 / 1e6 / secs
    }
}

/// What an offload is doing, for a caller that wants to show it.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Reading and hashing the source.
    Copying {
        path: String,
        done: usize,
        total: usize,
    },
    /// Reading the copy back from disk and hashing it.
    Verifying {
        path: String,
        done: usize,
        total: usize,
    },
}

/// Copy `source` to `destination`, verifying every file from the disk it
/// landed on, and write the hash list that proves it.
pub fn offload(
    source: &Path,
    destination: &Path,
    kind: HashKind,
    mut progress: impl FnMut(Step),
) -> Result<OffloadReport> {
    let started = Instant::now();
    let files = mhl::files_under(source)?;
    let total = files.len();
    let mut report = OffloadReport::default();
    let mut entries = Vec::with_capacity(total);
    let date = mhl::iso8601_utc(SystemTime::now());

    for (done, (rel, size)) in files.iter().enumerate() {
        let from = source.join(rel);
        let to = destination.join(rel);
        if to.exists() {
            report.refused.push(rel.clone());
            continue;
        }
        progress(Step::Copying {
            path: rel.clone(),
            done,
            total,
        });
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(Error::Io)?;
        }
        // Read once, hashing what is read and writing it as it goes: the card
        // is not read twice, and the hash is of the bytes that were copied.
        let source_hash = copy_hashing(&from, &to, kind)?;

        progress(Step::Verifying {
            path: rel.clone(),
            done,
            total,
        });
        // Read back after the fsync: catches a truncated or wrong copy. Not a
        // proof against power loss — see the module note.
        let copy_hash = mhl::hash_file(&to, kind)?;
        if copy_hash == source_hash {
            report.copied.push((rel.clone(), *size, copy_hash.clone()));
            report.bytes += size;
        } else {
            report
                .mismatched
                .push((rel.clone(), source_hash.clone(), copy_hash.clone()));
        }
        entries.push(Entry {
            path: rel.clone(),
            size: *size,
            kind,
            hash: copy_hash.clone(),
            action: if copy_hash == source_hash {
                Action::Original
            } else {
                Action::Failed
            },
            hash_date: date.clone(),
        });
    }

    if !entries.is_empty() {
        let list = HashList {
            version: "2.0".into(),
            creator: Creator::default(),
            root: destination.to_path_buf(),
            entries,
            unimplemented: Vec::new(),
        };
        report.hash_list = Some(mhl::write_generation(destination, &list)?);
    }
    report.took = started.elapsed();
    Ok(report)
}

/// Copy one file, hashing the bytes as they pass.
fn copy_hashing(from: &Path, to: &Path, kind: HashKind) -> Result<String> {
    use md5::Digest;
    let mut input = std::fs::File::open(from).map_err(Error::Io)?;
    let mut output = std::fs::File::create(to).map_err(Error::Io)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut xxh64 = twox_hash::XxHash64::with_seed(0);
    let mut xxh3 = twox_hash::XxHash3_64::with_seed(0);
    let mut md5 = md5::Md5::new();
    let mut sha1 = sha1::Sha1::new();
    loop {
        let n = input.read(&mut buffer).map_err(Error::Io)?;
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
        output.write_all(chunk).map_err(Error::Io)?;
    }
    // Flushed and synced before it is read back: the device has acknowledged
    // the write by the time anything claims the copy is good.
    output.flush().map_err(Error::Io)?;
    output.sync_all().map_err(Error::Io)?;
    Ok(match kind {
        HashKind::Xxh64 => format!("{:016x}", std::hash::Hasher::finish(&xxh64)),
        HashKind::Xxh3 => format!("{:016x}", std::hash::Hasher::finish(&xxh3)),
        HashKind::Md5 => md5
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        HashKind::Sha1 => sha1
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    })
}
