//! An offload, verified: the ASC MHL a card carries, and what it catches.

use cine_media::mhl::{self, Action, HashKind};
use std::path::PathBuf;

struct Card(PathBuf);

impl Card {
    fn new(name: &str) -> Card {
        let dir = std::env::temp_dir().join(format!("cine-mhl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("DCIM/100CINE")).unwrap();
        std::fs::write(dir.join("DCIM/100CINE/A001C001.mov"), vec![7u8; 4096]).unwrap();
        std::fs::write(dir.join("DCIM/100CINE/A001C002.mov"), vec![9u8; 8192]).unwrap();
        std::fs::write(dir.join("notes.txt"), b"roll A001").unwrap();
        Card(dir)
    }
}

impl Drop for Card {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_card_is_hashed_written_and_verified() {
    let card = Card::new("clean");
    let list = mhl::create(&card.0, HashKind::Xxh64, |_, _| {}).unwrap();
    assert_eq!(list.entries.len(), 3, "every file, and only the files");
    assert!(list.entries.iter().all(|e| e.action == Action::Original));
    assert!(
        list.entries
            .iter()
            .any(|e| e.path == "DCIM/100CINE/A001C001.mov" && e.size == 4096),
        "paths are relative to the card, with sizes: {:?}",
        list.entries
    );

    let written = mhl::write_generation(&card.0, &list).unwrap();
    assert!(written.starts_with(mhl::mhl_dir(&card.0)));
    assert!(
        written
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("0001_"),
        "the first generation: {written:?}"
    );

    // Read it back: the same list, and the hashes are not re-derived.
    let read = mhl::read_generation(&written).unwrap();
    assert_eq!(read.entries.len(), list.entries.len());
    for (a, b) in read.entries.iter().zip(&list.entries) {
        assert_eq!(
            (&a.path, a.size, a.kind, &a.hash),
            (&b.path, b.size, b.kind, &b.hash)
        );
    }
    assert!(read.unimplemented.is_empty(), "{:?}", read.unimplemented);

    // Verify the card as it stands: everything matches.
    let report = mhl::verify(&card.0, &read, |_, _| {}).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.verified.len(), 3);
    assert!(report.unlisted.is_empty());

    // The verification is itself a generation, and does not overwrite the first.
    let second = mhl::write_generation(&card.0, &mhl::generation_for(&read, &report)).unwrap();
    assert!(second
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("0002_"));
    assert_eq!(mhl::generations(&card.0).len(), 2, "the card's history");
    let back = mhl::read_generation(&second).unwrap();
    assert!(back.entries.iter().all(|e| e.action == Action::Verified));
}

#[test]
fn corruption_a_copy_would_not_notice_is_named_with_both_hashes() {
    let card = Card::new("bitrot");
    let list = mhl::create(&card.0, HashKind::Xxh64, |_, _| {}).unwrap();
    let generation = mhl::write_generation(&card.0, &list).unwrap();

    // One byte, in the middle, with the size unchanged: what a copy and a
    // file listing both miss.
    let target = card.0.join("DCIM/100CINE/A001C002.mov");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes[4000] ^= 0x01;
    std::fs::write(&target, &bytes).unwrap();
    // And a file that was never in the list, and one that has gone.
    std::fs::write(card.0.join("DCIM/100CINE/A001C003.mov"), vec![3u8; 16]).unwrap();
    std::fs::remove_file(card.0.join("notes.txt")).unwrap();

    let read = mhl::read_generation(&generation).unwrap();
    let report = mhl::verify(&card.0, &read, |_, _| {}).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.failed.len(), 1, "{report:?}");
    let (path, expected, actual) = &report.failed[0];
    assert_eq!(path, "DCIM/100CINE/A001C002.mov");
    assert_ne!(
        expected, actual,
        "both hashes are reported, not just a verdict"
    );
    assert_eq!(report.missing, vec!["notes.txt".to_string()]);
    assert_eq!(
        report.unlisted,
        vec!["DCIM/100CINE/A001C003.mov".to_string()],
        "a file nobody listed is not the same as a file that changed"
    );
    assert_eq!(report.verified.len(), 1);

    // The generation recording it says which file failed.
    let next = mhl::generation_for(&read, &report);
    let failed: Vec<&str> = next
        .entries
        .iter()
        .filter(|e| e.action == Action::Failed)
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(failed, vec!["DCIM/100CINE/A001C002.mov"]);
    // The missing file is not carried forward as though it were fine.
    assert!(next.entries.iter().all(|e| e.path != "notes.txt"));
}

#[test]
fn every_algorithm_round_trips_through_the_file() {
    for kind in [
        HashKind::Xxh64,
        HashKind::Xxh3,
        HashKind::Md5,
        HashKind::Sha1,
    ] {
        let card = Card::new(&format!("alg-{kind}"));
        let list = mhl::create(&card.0, kind, |_, _| {}).unwrap();
        let path = mhl::write_generation(&card.0, &list).unwrap();
        let read = mhl::read_generation(&path).unwrap();
        assert!(read.entries.iter().all(|e| e.kind == kind), "{kind}");
        assert!(
            mhl::verify(&card.0, &read, |_, _| {}).unwrap().is_clean(),
            "{kind}"
        );
    }
}

/// A file the format has and this build does not apply is named, not ignored.
#[test]
fn what_is_not_applied_from_a_list_is_reported() {
    let card = Card::new("unimplemented");
    let dir = mhl::mhl_dir(&card.0);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("0001_card_20240101T000000Z.mhl");
    std::fs::write(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<hashlist version="2.0">
  <creatorinfo><tool>other</tool></creatorinfo>
  <hashes>
    <hash><path size="9">notes.txt</path><xxh64 action="original">0000000000000000</xxh64></hash>
    <directoryhash><path>DCIM</path></directoryhash>
  </hashes>
  <ignore><pattern>.DS_Store</pattern></ignore>
</hashlist>
"#,
    )
    .unwrap();
    let read = mhl::read_generation(&path).unwrap();
    assert_eq!(read.entries.len(), 1);
    assert!(
        read.unimplemented.contains(&"directoryhash".to_string()),
        "{:?}",
        read.unimplemented
    );
    assert!(
        read.unimplemented.contains(&"ignore".to_string()),
        "{:?}",
        read.unimplemented
    );
}

/// The offload itself: copied, read back from disk, compared, and recorded.
#[test]
fn an_offload_copies_verifies_from_disk_and_writes_its_proof() {
    let card = Card::new("offload-src");
    let to = std::env::temp_dir().join(format!("cine-offload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    std::fs::create_dir_all(&to).unwrap();

    let mut steps = Vec::new();
    let report = cine_media::offload::offload(&card.0, &to, HashKind::Xxh64, |s| steps.push(s))
        .expect("offload");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.copied.len(), 3);
    assert_eq!(report.bytes, 4096 + 8192 + 9);
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, cine_media::offload::Step::Verifying { .. })),
        "the copy is read back, and says so"
    );

    // The files are there, and the proof with them.
    assert!(to.join("DCIM/100CINE/A001C001.mov").is_file());
    let generation = report.hash_list.clone().expect("a hash list");
    assert!(generation.starts_with(mhl::mhl_dir(&to)));

    // And the destination verifies against its own list, from the disk.
    let list = mhl::read_generation(&generation).unwrap();
    assert!(mhl::verify(&to, &list, |_, _| {}).unwrap().is_clean());

    // A second offload into the same place refuses rather than overwriting.
    let again = cine_media::offload::offload(&card.0, &to, HashKind::Xxh64, |_| {}).unwrap();
    assert!(!again.is_clean());
    assert_eq!(again.refused.len(), 3, "nothing was overwritten");
    assert!(again.copied.is_empty());

    std::fs::remove_dir_all(&to).ok();
}
