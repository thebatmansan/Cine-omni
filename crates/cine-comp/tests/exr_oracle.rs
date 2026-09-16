//! OpenEXR: what this build reads, against what Blender reads.
//!
//! A round trip through one's own reader and writer proves they agree with
//! each other, which is not the same as being right. Blender reads EXR with
//! the Academy's own library, and it is a separate program on the command line
//! — nothing linked, copied or distributed — so it can say independently what
//! is in a file. `tests/data/exr-oracle.tsv` is its answer for one of its own
//! HDRIs, made by `tests/data/blender_exr_oracle.py`.
//!
//! The file itself is not committed: it belongs to Blender's installation.
//! Without it the test says so and skips, as the media tests do.

use cine_comp::exr_io;
use std::path::{Path, PathBuf};

fn oracle_file() -> Option<PathBuf> {
    let text = include_str!("data/exr-oracle.tsv");
    let first = text.lines().next()?;
    let path = PathBuf::from(first.rsplit("; ").next()?.trim());
    path.exists().then_some(path)
}

#[test]
fn a_third_party_exr_reads_as_blender_reads_it() {
    let Some(path) = oracle_file() else {
        eprintln!("SKIP: the EXR the oracle was made from is not on this machine");
        return;
    };
    let text = include_str!("data/exr-oracle.tsv");
    let size: Vec<usize> = text
        .lines()
        .find(|l| l.starts_with("# size"))
        .expect("size line")
        .split('\t')
        .skip(1)
        .map(|v| v.parse().unwrap())
        .collect();

    let info = exr_io::probe(&path).expect("probe");
    println!(
        "MEASURED {}: display window {:?}, {} layer(s), first {:?}",
        path.display(),
        info.display_window,
        info.layers.len(),
        info.layers
            .first()
            .map(|l| (&l.name, l.data_window, l.channels.len())),
    );
    let image = exr_io::read(&path, None).expect("read");
    assert_eq!((image.width, image.height), (size[0], size[1]));

    let mut worst = 0.0f32;
    let mut compared = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let (x, y): (usize, usize) = (f[0].parse().unwrap(), f[1].parse().unwrap());
        let want: Vec<f32> = f[2..6].iter().map(|v| v.parse().unwrap()).collect();
        let got = image.get_rgba(x, y);
        for c in 0..4 {
            worst = worst.max((got[c] - want[c]).abs());
        }
        compared += 1;
    }
    println!("MEASURED {compared} pixels against Blender: worst difference {worst:.3e}");
    assert!(
        worst < 1e-6,
        "this build reads different pixels from the same file than Blender does ({worst})"
    );
}

/// What was written is what comes back — including the windows, which is the
/// point of using EXR at all.
#[test]
fn a_comp_round_trips_with_its_data_and_display_windows() {
    use cine_comp::image::Image;

    let dir = std::env::temp_dir().join(format!("cine-exr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("comp.exr");

    // A data window that is neither at the origin nor the size of the frame:
    // what a transform or a merge produces.
    let mut img = Image::rgba(37, 21);
    for y in 0..21 {
        for x in 0..37 {
            img.set_rgba(
                x,
                y,
                [
                    x as f32 / 36.0,
                    y as f32 / 20.0,
                    (x + y) as f32 / 56.0,
                    if x < 18 { 0.25 } else { 1.0 },
                ],
            );
        }
    }
    let img = img.placed(-9, 14);
    exr_io::write(&path, &img, Some((0, 0, 1920, 1080))).expect("write");

    let info = exr_io::probe(&path).expect("probe");
    assert_eq!(info.display_window, (0, 0, 1920, 1080), "the frame");
    assert_eq!(info.layers.len(), 1);
    assert_eq!(
        info.layers[0].data_window,
        (-9, 14, 37, 21),
        "the data window"
    );
    assert!(info.layers[0].is_colour() && !info.layers[0].deep);

    let back = exr_io::read(&path, None).expect("read");
    assert_eq!(back.window(), img.window(), "the window survived the file");
    for y in 0..21 {
        for x in 0..37 {
            let (a, b) = (back.get_rgba(x, y), img.get_rgba(x, y));
            for c in 0..4 {
                assert!((a[c] - b[c]).abs() < 1e-6, "pixel {x},{y}: {a:?} vs {b:?}");
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A layer that is not there is named, not guessed at.
#[test]
fn asking_for_a_layer_that_is_not_there_says_what_is() {
    use cine_comp::image::Image;
    let dir = std::env::temp_dir().join(format!("cine-exr-layer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("beauty.exr");
    exr_io::write(&path, &Image::rgba(4, 4), None).unwrap();
    let err = exr_io::read(&path, Some("depth")).unwrap_err().to_string();
    assert!(err.contains("depth"), "{err}");
    assert!(err.contains("(unnamed)") || err.contains("it has"), "{err}");
    let _ = Path::new(&path);
    std::fs::remove_dir_all(&dir).ok();
}
