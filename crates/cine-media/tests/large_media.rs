//! What this build does with media too large to hold.
//!
//! Professional media is bigger than memory. Two things follow, and both are
//! measured here rather than asserted in a comment: identifying and probing a
//! file must not read it, and a picture this machine cannot hold must be
//! refused by name rather than attempted — an allocation the kernel answers by
//! killing the process takes the project's unsaved work with it, and is not a
//! failure the application can report or a person can act on.
//!
//! This file holds one test that sets `CINE_PICTURE_BUDGET`, which is
//! process-wide, so nothing else belongs in it.

use cine_media::{budget, ContentHash};
use std::io::Write;
use std::time::Instant;

/// Resident memory of this process, in bytes.
fn resident() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4096
}

#[test]
fn identity_and_the_picture_budget_both_refuse_to_hold_what_they_cannot() {
    // A file far larger than any picture, written in pieces so writing it does
    // not need the memory either.
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("card.bin");
    const BYTES: u64 = 512 * 1024 * 1024;
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
        let block = vec![0xA5u8; 1 << 20];
        for i in 0..(BYTES >> 20) {
            // Different bytes at the front and the back, so head and tail are
            // not the same content.
            let mut b = block.clone();
            b[0] = i as u8;
            f.write_all(&b).unwrap();
        }
        f.flush().unwrap();
    }

    let before = resident();
    let began = Instant::now();
    let hash = ContentHash::fast(&big).unwrap();
    let took = began.elapsed();
    let grew = resident().saturating_sub(before);
    assert!(hash.hex().len() >= 32);
    assert_eq!(hash.size(), BYTES);
    println!(
        "MEASURED identity of a {} file: {:.1} ms, resident memory grew {} (dev)",
        budget::human(BYTES),
        took.as_secs_f64() * 1000.0,
        budget::human(grew)
    );
    assert!(
        grew < 16 * 1024 * 1024,
        "identifying a {} file read {} of it into memory; it is meant to read only its ends",
        budget::human(BYTES),
        budget::human(grew)
    );

    // Arithmetic that overflows is an answer, not a panic.
    assert_eq!(budget::picture_bytes(u64::MAX, 2, 16), None);
    assert_eq!(budget::picture_bytes(1920, 1080, 4), Some(1920 * 1080 * 4));

    // A budget says what may be attempted, and a refusal names both sizes.
    let refused = budget::check_picture("huge.exr", 60_000, 60_000, 16).err();
    let budget_now = budget::picture_budget();
    match refused {
        Some(e) => {
            let said = e.to_string();
            assert!(
                said.contains("huge.exr") && said.contains("60000x60000"),
                "{said}"
            );
            assert!(
                said.contains("CINE_PICTURE_BUDGET"),
                "it says how to raise it: {said}"
            );
        }
        // Only a machine that really could decode a 57 GB picture may say yes.
        None => assert!(
            budget_now >= 60_000u64 * 60_000 * 16,
            "a picture larger than the budget was allowed"
        ),
    }

    // Held to a small budget, a real EXR is refused rather than decoded — the
    // same path a 60 000-pixel-wide frame takes on a real machine.
    let exr = dir.path().join("frame.0001.exr");
    let pixels: Vec<(f32, f32, f32, f32)> =
        (0..64 * 64).map(|i| (i as f32, 0.5, 0.25, 1.0)).collect();
    exr::prelude::write_rgba_file(&exr, 64, 64, |x, y| pixels[y * 64 + x]).unwrap();

    // Safety: this is the only test in this binary, so nothing else can be
    // reading the environment while it changes.
    unsafe { std::env::set_var("CINE_PICTURE_BUDGET", "1024") };
    assert_eq!(budget::picture_budget(), 1024);
    let mut reader = cine_media::sequence::SequenceReader::open(&exr, ucg_core::Rate::F24).unwrap();
    let Err(refused) = reader.frame_at_index(0) else {
        panic!("a picture over budget is refused, not decoded")
    };
    let said = refused.to_string();
    assert!(
        said.contains("64x64"),
        "the refusal names the real size: {said}"
    );
    assert!(
        said.contains("1.0 kB") || said.contains("1024"),
        "and the real budget: {said}"
    );

    unsafe { std::env::remove_var("CINE_PICTURE_BUDGET") };
    // With the machine's own budget, the same file decodes.
    let mut reader = cine_media::sequence::SequenceReader::open(&exr, ucg_core::Rate::F24).unwrap();
    let frame = reader.frame_at_index(0).unwrap().expect("a picture");
    assert_eq!((frame.width, frame.height), (64, 64));
}
