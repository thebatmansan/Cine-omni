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

/// A grey ramp in scene light: from deep shadow at the left to a highlight
/// several stops above white at the right, so a view's roll-off is visible and
/// a clamp is obvious. Each frame is a little brighter than the last, so the
/// frames differ.
fn scene_value(x: usize, frame: usize) -> f32 {
    0.005 * 2.0f32.powf(x as f32 / 3.0) * (1.0 + 0.05 * frame as f32)
}

fn exr(path: &Path, frame: usize) {
    let (w, h) = (32usize, 18usize);
    x::write_rgba_file(path, w, h, |px, _py| {
        let v = scene_value(px, frame);
        (v, v, v, 1.0)
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
fn scene_linear_delivers_through_a_view_or_as_exr_and_refuses_to_be_clamped() {
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
    let made = c.call("new_timeline", json!({ "name": "Comp", "rate_num": 24, "rate_den": 1 }));
    let tl = made["id"].as_str().unwrap().to_string();
    let v1 = c.call("timeline", json!({ "id": tl }))["tracks"][0]["id"].as_str().unwrap().to_string();
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [v1], "time": at(0),
                "clip": { "media": media, "source_range": { "start_time": at(0), "duration": at(4) } } }),
    );
    let render = |c: &mut Client, params: Value| -> Value {
        let job = c.call("job_submit", params);
        wait_for(c, job["id"].as_u64().unwrap())
    };

    // As decoded, scene light has no display values: refused, with the reason.
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl, "output": "png16" }),
    );
    assert_eq!(refused["state"], "failed", "{refused}");
    let why = refused["error"].as_str().unwrap();
    assert!(why.contains("scene-linear") && why.contains("clamp every highlight"), "{why}");

    // Through the project's view, the same pictures deliver.
    let began = Instant::now();
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl, "output": "png16", "look": "project_view" }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    println!(
        "MEASURED 4 frames of 32x18 scene-linear EXR through the exact view to PNG 16: {} ms (dev)",
        began.elapsed().as_millis()
    );
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let manifest: Value = serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["look"]["worst_lattice_error"], 0.0, "the exact path has no lattice error");
    assert!(
        manifest["look"]["transforms"][0].as_str().unwrap().contains("exact, scene-linear"),
        "the manifest says how it was transformed: {}", manifest["look"]["transforms"]
    );

    // The highlights rolled off rather than clipping: this picture's scene
    // values run from 5.0 up, and a clamp would make every pixel pure white.
    let img = image::open(out.join("frame_000000.png")).unwrap().to_rgba16();
    let ramp: Vec<u16> = (0..32).map(|x| img.get_pixel(x, 9)[1]).collect();
    assert!(ramp.windows(2).all(|w| w[1] >= w[0]), "brighter scene values stay brighter: {ramp:?}");
    assert!(ramp[0] < 6000, "the shadow end is dark, not lifted: {}", ramp[0]);
    assert!(ramp[31] > 40_000, "the highlight end is bright: {}", ramp[31]);
    assert!(
        ramp.iter().filter(|v| **v == u16::MAX).count() <= 8,
        "a clamped render would be white across most of the ramp: {ramp:?}"
    );

    // As EXR, the scene values themselves survive, unclamped.
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl, "output": "exr", "start_frame": 0, "end_frame": 2 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let written = out.join("frame_000000.exr");
    assert!(written.is_file(), "an EXR frame was written");
    let read = x::read_first_rgba_layer_from_file(
        &written,
        |size: x::Vec2<usize>, _| -> (Vec<f32>, usize) { (vec![0.0; size.0 * size.1 * 4], size.0) },
        |(pixels, width): &mut (Vec<f32>, usize), pos: x::Vec2<usize>, (r, g, b, a): (f32, f32, f32, f32)| {
            let i = (pos.1 * *width + pos.0) * 4;
            pixels[i] = r;
            pixels[i + 1] = g;
            pixels[i + 2] = b;
            pixels[i + 3] = a;
        },
    )
    .unwrap();
    let (pixels, _) = read.layer_data.channel_data.pixels;
    // The delivered EXR holds the source's scene values, not a display-encoded
    // version of them — including the highlight several stops above white.
    for x_pos in [0usize, 7, 31] {
        let got = pixels[x_pos * 4];
        let expected = scene_value(x_pos, 1);
        assert!(
            (got - expected).abs() <= expected * 1e-2,
            "pixel {x_pos}: {got} is not the scene value {expected}"
        );
    }
    assert!(
        pixels[31 * 4] > 1.0,
        "the highlight above display white survived delivery: {}",
        pixels[31 * 4]
    );
    assert!(
        pixels[0] < 0.01,
        "and the deep shadow survived too: {}",
        pixels[0]
    );

    // Captions cannot be burned into scene values, and that refuses too.
    c.call(
        "set_caption",
        json!({ "timeline": tl, "language": "en", "start": at(0), "end": at(2), "text": "no" }),
    );
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl, "output": "exr", "captions": "en" }),
    );
    assert_eq!(refused["state"], "failed", "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("display white"),
        "{refused}"
    );
}

#[test]
fn a_proxy_of_scene_linear_media_is_refused_because_it_would_bake_a_view() {
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
    let job = c.call("job_submit", json!({ "kind": "media.proxy", "media": media, "height": 16 }));
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "failed", "{done}");
    let error = done["error"].as_str().unwrap();
    assert!(error.contains("scene-linear") && error.contains("applied twice"), "{error}");
}

/// The monitor can look at scene-linear pictures through the project's view,
/// and says which view it used — the same function a render uses, so a preview
/// and a delivery cannot disagree about colour.
#[test]
fn a_picture_can_be_previewed_through_the_project_view() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    for n in 1..=2 {
        exr(&plates.join(format!("p.{n:04}.exr")), n);
    }
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = c.call("ingest", json!({ "path": plates.join("p.0001.exr").display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    // As decoded: clamped, and it says so.
    let (status, headers, _) = h.get(&format!("/frame?media={media}&index=0"));
    assert!(status.contains("200"), "{status}");
    let plain = common::header(&headers, "x-cine-colour").unwrap().to_string();
    assert!(plain.contains("clamped"), "{plain}");

    // Through the view: transformed, and it says which.
    let (status, headers, body) = h.get(&format!("/frame?media={media}&index=0&view=project"));
    assert!(status.contains("200"), "{status}");
    assert!(body.starts_with(b"\x89PNG"));
    let viewed = common::header(&headers, "x-cine-colour").unwrap().to_string();
    assert!(viewed.contains("exact%2C%20scene-linear") || viewed.contains("exact"), "{viewed}");
    assert!(viewed.contains("Display"), "the view is named: {viewed}");

    // The two pictures differ: a view is not a no-op.
    let (_, _, plain_body) = h.get(&format!("/frame?media={media}&index=0"));
    assert!(plain_body != body, "the view changed the picture");
}
