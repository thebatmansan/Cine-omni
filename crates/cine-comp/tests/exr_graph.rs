//! A comp graph that reads a sequence off disk and writes one back.
//!
//! The evaluator's file registry made graphs testable without a disk; this is
//! the other half, which is what a shot actually is: `plate.####.exr` read
//! frame by frame, and a render written frame by frame.

use cine_comp::eval::Evaluator;
use cine_comp::exr_io;
use cine_comp::graph_model::{CompGraph, CompNode, MergeOp, NodeKind, ReadSource};
use cine_comp::image::Image;
use cine_comp::sequence;

fn plate(frame: i64) -> Image {
    let mut img = Image::rgba(16, 12);
    for y in 0..12 {
        for x in 0..16 {
            // Frame-dependent, so reading the wrong frame is visible.
            img.set_rgba(
                x,
                y,
                [frame as f32 / 1000.0, x as f32 / 15.0, y as f32 / 11.0, 1.0],
            );
        }
    }
    img
}

#[test]
fn a_sequence_is_read_per_frame_and_a_render_is_written_per_frame() {
    let dir = std::env::temp_dir().join(format!("cine-exr-graph-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pattern = dir.join("plate.####.exr").to_string_lossy().into_owned();
    for frame in [1001, 1002, 1003] {
        exr_io::write(&sequence::frame_path(&pattern, frame), &plate(frame), None).unwrap();
    }
    assert_eq!(sequence::frames_on_disk(&pattern), vec![1001, 1002, 1003]);

    let mut g = CompGraph::new();
    let read = g
        .add(CompNode::read(
            "plate",
            ReadSource::File {
                path: pattern.clone(),
            },
        ))
        .unwrap();
    let grade = g
        .add(CompNode::new("grade", NodeKind::ColorCorrect).with_param("gain", 2.0f64))
        .unwrap();
    let out_pattern = dir.join("comp.%04d.exr").to_string_lossy().into_owned();
    let write = g.add(CompNode::write("write", &out_pattern)).unwrap();
    g.connect(read, 0, grade, 0).unwrap();
    g.connect(grade, 0, write, 0).unwrap();

    let mut ev = Evaluator::default();
    ev.write_files = true;
    for frame in [1001, 1002, 1003] {
        let out = ev.evaluate(&g, write, frame).unwrap();
        let source = plate(frame);
        assert!(
            (out.get_rgba(0, 0)[0] - source.get_rgba(0, 0)[0] * 2.0).abs() < 1e-5,
            "frame {frame} was read and graded"
        );
    }
    let written = ev.take_writes();
    assert_eq!(written.len(), 3, "one write per frame, cached per frame");
    assert_eq!(
        sequence::frames_on_disk(&out_pattern),
        vec![1001, 1002, 1003],
        "the render is on disk as a sequence"
    );
    // And what landed on disk is what the graph produced.
    for frame in [1001, 1002, 1003] {
        let from_disk = exr_io::read(&sequence::frame_path(&out_pattern, frame), None).unwrap();
        let in_memory = ev.evaluate(&g, write, frame).unwrap();
        for y in 0..12 {
            for x in 0..16 {
                let (a, b) = (from_disk.get_rgba(x, y), in_memory.get_rgba(x, y));
                for c in 0..4 {
                    assert!((a[c] - b[c]).abs() < 1e-6, "frame {frame} pixel {x},{y}");
                }
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A missing frame is named with the frame and the path it resolved to, not
/// reported as an empty image.
#[test]
fn a_missing_frame_says_which_frame_and_which_file() {
    let mut g = CompGraph::new();
    let read = g
        .add(CompNode::read(
            "plate",
            ReadSource::File {
                path: "/nowhere/plate.%04d.exr".into(),
            },
        ))
        .unwrap();
    let merge = g.add(CompNode::merge("m", MergeOp::Over)).unwrap();
    g.connect(read, 0, merge, 0).unwrap();
    let ev = Evaluator::default();
    let err = ev.evaluate(&g, read, 1007).unwrap_err().to_string();
    assert!(err.contains("1007"), "{err}");
    assert!(err.contains("plate.1007.exr"), "{err}");
}

/// Writing is an effect a caller asks for: evaluating a graph does not put
/// files on disk behind their back.
#[test]
fn evaluating_does_not_write_files_unless_asked() {
    let dir = std::env::temp_dir().join(format!("cine-exr-nowrite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("never.exr");
    let mut g = CompGraph::new();
    let src = g
        .add(CompNode::procedural(
            "src",
            cine_comp::graph_model::ProceduralKind::Checker,
            8,
            8,
        ))
        .unwrap();
    let write = g
        .add(CompNode::write("write", &out.to_string_lossy()))
        .unwrap();
    g.connect(src, 0, write, 0).unwrap();

    let ev = Evaluator::default();
    ev.evaluate(&g, write, 0).unwrap();
    assert!(!out.exists(), "nothing was written");
    assert_eq!(ev.writes().len(), 1, "but the write was recorded");
    std::fs::remove_dir_all(&dir).ok();
}
