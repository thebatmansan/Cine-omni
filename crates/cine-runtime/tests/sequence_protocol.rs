//! Image sequences as media, through the protocol.
//!
//! A numbered run of EXR or PNG files is one asset with one identity, a rate
//! it was ingested at, and a length; it cuts like any other media, its frames
//! reach the record monitor, and the paths that would have to lower its
//! precision refuse instead of doing it quietly.

mod common;

use common::{Client, Harness};
use exr::prelude as x;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// An EXR whose pixels say which frame they are, in scene values above 1.
fn exr(path: &Path, frame: usize) {
    let (w, h) = (32usize, 18usize);
    x::write_rgba_file(path, w, h, |px, _py| {
        (4.0 + frame as f32 + px as f32 / 100.0, 0.25, 0.0, 1.0)
    })
    .unwrap();
}

fn png16(path: &Path, value: u16) {
    let img = image::ImageBuffer::from_pixel(32, 18, image::Rgba([value, value / 2, value / 4, 65535]));
    image::DynamicImage::ImageRgba16(img).save(path).unwrap();
}

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let jobs = c.call("jobs", json!({}));
        if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
            if matches!(job["state"].as_str().unwrap(), "succeeded" | "failed" | "cancelled") {
                return job.clone();
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_numbered_run_is_ingested_as_one_asset_with_its_own_rate() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    for n in 1..=8 {
        exr(&plates.join(format!("shot_0010.{n:04}.exr")), n);
    }
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();

    let first = plates.join("shot_0010.0001.exr");
    let answer = c.call(
        "ingest",
        json!({ "path": first.display().to_string(), "rate_num": 24000, "rate_den": 1001 }),
    );
    assert_eq!(answer["sequence"], true, "{answer}");
    assert_eq!(answer["pattern"], "shot_0010.####.exr");
    assert_eq!((answer["first"].as_i64(), answer["last"].as_i64()), (Some(1), Some(8)));
    assert_eq!(answer["frames"], 8);
    assert_eq!(answer["missing"], json!([]));
    let media = answer["id"].as_str().unwrap().to_string();

    // One asset, whatever member of the run it was ingested from.
    let again = c.call("ingest", json!({ "path": plates.join("shot_0010.0005.exr").display().to_string(), "rate_num": 24000, "rate_den": 1001 }));
    assert_eq!(again["id"], json!(media), "any file of the run is the same asset");

    let view = c.call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(media))
        .unwrap()
        .clone();
    assert_eq!(view["container"], "ImageSequence(Sequence { dir: \"".to_string() + &plates.display().to_string() + "\", pattern: \"shot_0010.####.exr\", prefix: \"shot_0010.\", suffix: \".exr\", padding: 4, kind: Exr, first: 1, last: 8, missing: [] })");
    assert_eq!(view["video"]["codec"], "exr");
    assert_eq!(view["video"]["width"], 32);
    assert_eq!(view["video"]["frames"], 8);
    assert_eq!(view["video"]["rate_num"], 24000);
    assert_eq!(view["online"], true);

    // It cuts like any other media, and its pictures reach the monitor.
    let made = c.call("new_timeline", json!({ "name": "Comp", "rate_num": 24000, "rate_den": 1001 }));
    let tl = made["id"].as_str().unwrap().to_string();
    let v1 = c.call("timeline", json!({ "id": tl }))["tracks"][0]["id"].as_str().unwrap().to_string();
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24000, "rate_den": 1001 });
    let placed = c.call(
        "place",
        json!({ "tracks": [v1], "time": at(0),
                "clip": { "media": media, "source_range": { "start_time": at(2), "duration": at(4) } } }),
    );
    assert_eq!(placed["clips"].as_array().unwrap().len(), 1, "{placed}");
    let shown = c.call("picture_at", json!({ "timeline": tl, "time": at(1) }));
    assert_eq!(shown["black"], false, "{shown}");
    assert_eq!(shown["index"], 3, "frame 1 of the cut is picture 3 of the run");
    assert_eq!(shown["playable"], true);

    let (status, headers, body) = h.get(&format!("/frame?media={media}&index=3"));
    assert!(status.contains("200"), "{status}");
    assert!(body.starts_with(b"\x89PNG"));
    let colour = common::header(&headers, "x-cine-colour").unwrap();
    assert!(
        colour.contains("float") && colour.contains("clamped"),
        "a float picture shown as 8-bit says so: {colour}"
    );
}

#[test]
fn a_hole_in_the_run_is_part_of_it_and_refuses_rather_than_going_black() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    for n in [1, 2, 3, 5, 6] {
        exr(&plates.join(format!("p.{n:04}.exr")), n);
    }
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let answer = c.call("ingest", json!({ "path": plates.join("p.0001.exr").display().to_string() }));
    assert_eq!(answer["frames"], 6, "the run covers its whole range");
    assert_eq!(answer["missing"], json!([4]), "and says which number has no file");
    let media = answer["id"].as_str().unwrap().to_string();

    // The hole is not black: asking for it says which file is missing.
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=3"));
    let message = String::from_utf8_lossy(&body);
    // The frame service answers a picture it cannot produce with 422 and the
    // reason, never with 200 and a black image.
    assert!(status.contains("422"), "{status} {message}");
    assert!(message.contains("p.0004.exr") && message.contains("missing"), "{message}");
    // Its neighbours are fine.
    let (status, _, _) = h.get(&format!("/frame?media={media}&index=4"));
    assert!(status.contains("200"), "{status}");
}

#[test]
fn a_single_numbered_file_is_a_still_unless_a_run_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    png16(&plates.join("only.0001.png"), 513);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let answer = c.call("ingest", json!({ "path": plates.join("only.0001.png").display().to_string() }));
    assert_eq!(answer["sequence"], false, "one file is one still: {answer}");

    // Asked for as a run, one file is a run of one.
    png16(&plates.join("only.0002.png"), 1025);
    let answer = c.call(
        "ingest",
        json!({ "path": plates.join("only.0001.png").display().to_string(), "sequence": true }),
    );
    assert_eq!(answer["sequence"], true, "{answer}");
    assert_eq!(answer["frames"], 2);

    // A file with no number cannot be a run, and says so rather than guessing.
    std::fs::write(plates.join("plain.png"), std::fs::read(plates.join("only.0001.png")).unwrap()).unwrap();
    let refused = c
        .call_result(
            "ingest",
            json!({ "path": plates.join("plain.png").display().to_string(), "sequence": true }),
        )
        .unwrap_err();
    assert!(refused.contains("numbered run"), "{refused}");
}

#[test]
fn the_paths_that_would_lower_its_precision_refuse_instead() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    for n in 1..=4 {
        exr(&plates.join(format!("p.{n:04}.exr")), n);
    }
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = c.call("ingest", json!({ "path": plates.join("p.0001.exr").display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A proxy is 8-bit video: refused, with the reason.
    let job = c.call("job_submit", json!({ "kind": "media.proxy", "media": media, "height": 16 }));
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "failed", "{done}");
    let error = done["error"].as_str().unwrap();
    assert!(error.contains("image sequence") && error.contains("view transform"), "{error}");

    // So is a render of it, by the same rule.
    let made = c.call("new_timeline", json!({ "name": "Comp", "rate_num": 24, "rate_den": 1 }));
    let tl = made["id"].as_str().unwrap().to_string();
    let v1 = c.call("timeline", json!({ "id": tl }))["tracks"][0]["id"].as_str().unwrap().to_string();
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [v1], "time": at(0),
                "clip": { "media": media, "source_range": { "start_time": at(0), "duration": at(4) } } }),
    );
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "output": "png16" }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "failed", "{done}");
    assert!(
        done["error"].as_str().unwrap().contains("cannot be rendered by this path"),
        "{done}"
    );
}
