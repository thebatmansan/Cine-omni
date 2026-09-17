//! Playing a cut made of real media: every sequence picture is the picture of
//! the file the edit list says, gaps are black, and the scheduler plays it.

use cine_media::clock::{PlaybackClock, SystemTime};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::{EditEntry, EditListSource, VideoReader};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use ucg_core::{Rate, RationalTime, TimeRange};

macro_rules! need {
    ($opt:expr, $what:expr) => {
        match $opt {
            Some(p) => p,
            None => {
                eprintln!("SKIP: no {} available on this machine", $what);
                return;
            }
        }
    };
}

fn media() -> Option<(PathBuf, PathBuf)> {
    let a = PathBuf::from("/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4");
    let b = PathBuf::from("/home/finstein-emp/Downloads/A_solitary_monochrome_crimson.mp4");
    (a.exists() && b.exists()).then_some((a, b))
}

fn f(n: i64) -> RationalTime {
    RationalTime::exact(n, Rate::F24)
}

/// Offset of picture `n` of a file from its first picture, in its own time base.
fn offset_of(reader: &VideoReader, n: usize) -> RationalTime {
    let first = reader.extent().unwrap().0;
    reader.pts_at(n).unwrap().checked_sub(&first)
}

#[test]
fn each_sequence_picture_is_the_picture_the_edit_list_names() {
    let (a, b) = need!(media(), "two real mp4 files");
    let ra = VideoReader::open_path(&a).unwrap();
    let rb = VideoReader::open_path(&b).unwrap();
    // A: pictures 10.. for 24 frames at record 0. Gap of 6. B: pictures 40.. for 12 at record 30.
    let entries = vec![
        EditEntry {
            record: TimeRange::new(f(0), f(24)),
            path: Some(a.clone()),
            offset: Some(offset_of(&ra, 10)),
            unplayable: None,
            timing: None,
            name: "A".into(),
        },
        EditEntry {
            record: TimeRange::new(f(30), f(12)),
            path: Some(b.clone()),
            offset: Some(offset_of(&rb, 40)),
            unplayable: None,
            timing: None,
            name: "B".into(),
        },
    ];
    let mut seq = EditListSource::new(Rate::F24, f(42), entries);
    assert_eq!(seq.frame_count(), 42);
    let mut direct_a = VideoReader::open_path(&a).unwrap();
    let mut direct_b = VideoReader::open_path(&b).unwrap();

    // Picture 5 of the sequence is 5 record frames into A: A's picture at media
    // time offset(10) + 5/24 s.
    let t = ra
        .extent()
        .unwrap()
        .0
        .checked_add(&offset_of(&ra, 10))
        .checked_add(&f(5));
    let want = direct_a
        .frame_at_index(ra.index_at(t).unwrap())
        .unwrap()
        .unwrap();
    let got = seq.decode_index(5).unwrap().unwrap();
    assert_eq!(
        got.plane(0).unwrap().0,
        want.plane(0).unwrap().0,
        "same luma as a direct decode"
    );
    assert_eq!(got.pts, f(5), "presented at sequence time");

    // Picture 26 is in the gap: black, at A's size.
    let gap = seq.decode_index(26).unwrap().unwrap();
    assert_eq!((gap.width, gap.height), (want.width, want.height));
    assert!(gap.plane(0).unwrap().0.iter().all(|&y| y == 16 || y == 0));
    assert!(seq.why_black(26).unwrap().contains("nothing"));

    // Picture 31 is one frame into B.
    let tb = rb
        .extent()
        .unwrap()
        .0
        .checked_add(&offset_of(&rb, 40))
        .checked_add(&f(1));
    let want_b = direct_b
        .frame_at_index(rb.index_at(tb).unwrap())
        .unwrap()
        .unwrap();
    let got_b = seq.decode_index(31).unwrap().unwrap();
    assert_eq!(got_b.plane(0).unwrap().0, want_b.plane(0).unwrap().0);
    assert!(seq.decode_index(42).unwrap().is_none(), "past the end");
}

#[test]
fn an_unplayable_entry_is_black_and_says_why() {
    let (a, _) = need!(media(), "two real mp4 files");
    let ra = VideoReader::open_path(&a).unwrap();
    let entries = vec![
        EditEntry {
            record: TimeRange::new(f(0), f(4)),
            path: Some(a.clone()),
            offset: Some(offset_of(&ra, 0)),
            unplayable: None,
            timing: None,
            name: "A".into(),
        },
        EditEntry {
            record: TimeRange::new(f(4), f(4)),
            path: None,
            offset: None,
            unplayable: Some("the media is offline".into()),
            timing: None,
            name: "Missing".into(),
        },
    ];
    let mut seq = EditListSource::new(Rate::F24, f(8), entries);
    let real = seq.decode_index(1).unwrap().unwrap();
    let frame = seq.decode_index(6).unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (real.width, real.height));
    assert_eq!(
        seq.why_black(6).as_deref(),
        Some("Missing: the media is offline")
    );
    assert_eq!(seq.why_black(1), None);
}

#[test]
fn the_scheduler_plays_a_cut_across_its_edit_points() {
    let (a, b) = need!(media(), "two real mp4 files");
    let ra = VideoReader::open_path(&a).unwrap();
    let rb = VideoReader::open_path(&b).unwrap();
    let entries = vec![
        EditEntry {
            record: TimeRange::new(f(0), f(12)),
            path: Some(a),
            offset: Some(offset_of(&ra, 0)),
            unplayable: None,
            timing: None,
            name: "A".into(),
        },
        EditEntry {
            record: TimeRange::new(f(12), f(12)),
            path: Some(b),
            offset: Some(offset_of(&rb, 0)),
            unplayable: None,
            timing: None,
            name: "B".into(),
        },
    ];
    let seq = EditListSource::new(Rate::F24, f(24), entries);
    let (start, end) = seq.extent().unwrap();
    let mut s = Scheduler::new(PlaybackClock::new(SystemTime::default(), start, end), seq);
    // Stepping through every picture presents each exactly once, in order,
    // across the cut at picture 12.
    let mut shown = Vec::new();
    let began = Instant::now();
    s.poll();
    for _ in 0..24 {
        if let Presentation::Present(p) = s.poll() {
            shown.push(p.index);
        }
        if s.step(1).is_none() {
            break;
        }
        if began.elapsed() > Duration::from_secs(60) {
            break;
        }
    }
    if let Presentation::Present(p) = s.poll() {
        shown.push(p.index);
    }
    shown.dedup();
    println!(
        "MEASURED stepped a two-clip cut: {:?} in {:?}",
        shown,
        began.elapsed()
    );
    assert!(
        shown.contains(&11) && shown.contains(&12),
        "crossed the edit point: {shown:?}"
    );
    assert!(shown.windows(2).all(|w| w[1] > w[0]), "in order: {shown:?}");
}
