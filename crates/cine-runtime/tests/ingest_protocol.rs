//! Importing media as a job, on real files.
//!
//! An import is where a file becomes an asset, and the asset is what every
//! other thing in the project refers to. These tests hold it to that: one
//! identity from content, the whole probe recorded, what this build cannot do
//! said rather than hidden, the derived work started as jobs of its own, and
//! nothing half-registered when it fails or is cancelled.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let jobs = c.call("jobs", json!({}));
        if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
            if matches!(
                job["state"].as_str().unwrap(),
                "succeeded" | "failed" | "cancelled"
            ) {
                return job.clone();
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn import(c: &mut Client, params: Value) -> Value {
    let job = c.call("job_submit", params);
    wait_for(c, job["id"].as_u64().unwrap())
}

fn media_view(c: &mut Client, id: &str) -> Value {
    c.call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap()
        .clone()
}

/// A movie with picture and sound, written by this build.
fn movie(path: &Path, seconds: u32) -> PathBuf {
    cine_media::testsignal::bars_and_pops(path, 160, 90, Rate::F24, seconds).unwrap();
    path.to_path_buf()
}

#[test]
fn an_import_is_a_job_that_makes_one_asset_and_starts_the_work_that_follows() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();

    let began = Instant::now();
    let done = import(
        &mut c,
        json!({ "kind": "media.ingest", "path": source.display().to_string() }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    println!(
        "MEASURED import of a 2 s 160x90 movie with sound, with thumbnail and waveform dispatched: {} ms (dev)",
        began.elapsed().as_millis()
    );

    // Everything the probe learned travels with the answer.
    let media = out["id"].as_str().unwrap().to_string();
    assert_eq!(out["sequence"], false);
    assert!(out["content_hash"].as_str().unwrap().len() >= 32, "{out}");
    assert_eq!(out["picture"]["width"], 160);
    assert_eq!(out["picture"]["height"], 90);
    assert_eq!(out["picture"]["decodable"], true);
    assert_eq!(out["picture"]["variable_frame_rate"], false);
    assert_eq!(out["sound"]["sample_rate"], 48_000);
    assert_eq!(out["sound"]["channels"], 2);
    assert_eq!(out["sound"]["decodable"], true);
    assert_eq!(out["rate_num"], 24);

    // The derived work was started as jobs, not done inline.
    let kinds: Vec<String> = out["dispatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap().to_string())
        .collect();
    assert!(kinds.contains(&"media.thumbnail".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"media.waveform".to_string()), "{kinds:?}");
    assert!(
        !kinds.contains(&"media.proxy".to_string()),
        "a proxy is minutes of work: never without being asked. {kinds:?}"
    );
    for d in out["dispatched"].as_array().unwrap() {
        let finished = wait_for(&mut c, d["job"].as_u64().unwrap());
        assert_eq!(finished["state"], "succeeded", "{finished}");
    }

    // And the asset carries them.
    let view = media_view(&mut c, &media);
    assert_eq!(view["waveform"]["state"], "current", "{view}");
    assert_eq!(view["online"], true);
    let (status, _, body) = h.get(&format!("/thumbnail?media={media}&index=0"));
    assert!(status.contains("200"), "the thumbnail is there: {status}");
    assert!(body.starts_with(b"\x89PNG"));

    // Importing the same file again is the same asset, not a second one.
    let again = import(
        &mut c,
        json!({ "kind": "media.ingest", "path": source.display().to_string(), "thumbnail": false, "waveform": false }),
    );
    assert_eq!(
        again["output"]["id"],
        json!(media),
        "one file, one identity"
    );
    assert_eq!(
        c.call("media", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["id"] == json!(media))
            .count(),
        1
    );
}

#[test]
fn a_proxy_is_made_on_import_only_when_it_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("take.mov"), 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let done = import(
        &mut c,
        json!({ "kind": "media.ingest", "path": source.display().to_string(),
                "thumbnail": false, "waveform": false, "proxy_height": 16 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let kinds: Vec<String> = done["output"]["dispatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        vec!["media.proxy".to_string()],
        "only what was asked for"
    );
    let job = done["output"]["dispatched"][0]["job"].as_u64().unwrap();
    assert_eq!(wait_for(&mut c, job)["state"], "succeeded");
    let media = done["output"]["id"].as_str().unwrap().to_string();
    assert_eq!(media_view(&mut c, &media)["proxies"][0]["state"], "current");
}

#[test]
fn an_import_that_cannot_be_done_leaves_no_asset_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let before = c.call("media", json!({})).as_array().unwrap().len();

    // A file that is not media at all.
    let rubbish = dir.path().join("notes.txt");
    std::fs::write(&rubbish, b"this is not a movie").unwrap();
    let refused = c.call_result(
        "job_submit",
        json!({ "kind": "media.ingest", "path": rubbish.display().to_string() }),
    );
    // The job is refused before it is queued, or it fails; either way nothing
    // is registered and the reason names the file.
    let message = match refused {
        Err(why) => why,
        Ok(job) => {
            let done = wait_for(&mut c, job["id"].as_u64().unwrap());
            assert_eq!(done["state"], "failed", "{done}");
            done["error"].as_str().unwrap().to_string()
        }
    };
    assert!(
        message.contains("notes.txt") || message.contains("extension"),
        "{message}"
    );
    assert_eq!(
        c.call("media", json!({})).as_array().unwrap().len(),
        before,
        "nothing was registered"
    );

    // A file that is not there.
    let missing = c.call_result(
        "job_submit",
        json!({ "kind": "media.ingest", "path": dir.path().join("gone.mov").display().to_string() }),
    );
    assert!(
        missing.unwrap_err().contains("no file at"),
        "it says which file"
    );
}

#[test]
fn an_import_cancelled_before_it_registers_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // Long enough that the import is still examining when the cancel lands.
    let source = movie(&dir.path().join("long.mov"), 6);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let before = c.call("media", json!({})).as_array().unwrap().len();
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.ingest", "path": source.display().to_string() }),
    );
    let id = job["id"].as_u64().unwrap();
    c.call("job_cancel", json!({ "id": id }));
    let done = wait_for(&mut c, id);
    // Either it was cancelled in time, or it finished first — both are
    // truthful states, and neither leaves a half-registered asset.
    let after = c.call("media", json!({})).as_array().unwrap().len();
    if done["state"] == "cancelled" || done["output"]["cancelled"] == true {
        assert_eq!(after, before, "a cancelled import registered nothing");
    } else {
        assert_eq!(done["state"], "succeeded", "{done}");
        assert_eq!(after, before + 1, "or it completed properly");
    }
}

#[test]
fn what_this_build_cannot_decode_is_said_at_import_rather_than_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();

    // A WAV of a layout this build reads: sound only, so no thumbnail.
    let sound = dir.path().join("room.wav");
    let frames = 48_000u32;
    let mut data = Vec::new();
    for n in 0..frames {
        let v = ((n as f32 / 40.0).sin() * 8_000_000.0) as i32;
        for _ in 0..2 {
            data.extend_from_slice(&v.to_le_bytes()[..3]);
        }
    }
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&(48_000u32 * 6).to_le_bytes());
    b.extend_from_slice(&6u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(&sound, b).unwrap();

    let done = import(
        &mut c,
        json!({ "kind": "media.ingest", "path": sound.display().to_string() }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    assert_eq!(out["picture"], Value::Null, "a WAV has no picture");
    assert_eq!(out["sound"]["channels"], 2);
    let notes = out["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("no thumbnail")),
        "it says why there is no thumbnail: {notes:?}"
    );
    let kinds: Vec<String> = out["dispatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        vec!["media.waveform".to_string()],
        "only the work that applies"
    );
    assert_eq!(
        wait_for(&mut c, out["dispatched"][0]["job"].as_u64().unwrap())["state"],
        "succeeded"
    );
}

#[test]
fn an_image_sequence_imports_as_one_asset_with_the_rate_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&plates).unwrap();
    for n in 1..=6 {
        let img =
            image::ImageBuffer::from_pixel(32, 18, image::Rgba([10u16 * n as u16, 20, 30, 65535]));
        image::DynamicImage::ImageRgba16(img)
            .save(plates.join(format!("plate.{n:04}.png")))
            .unwrap();
    }
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let done = import(
        &mut c,
        json!({ "kind": "media.ingest", "path": plates.join("plate.0001.png").display().to_string(),
                "sequence": true, "rate_num": 24000, "rate_den": 1001 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    assert_eq!(out["sequence"], true);
    assert_eq!(out["pattern"], "plate.####.png");
    assert_eq!(out["frames"], 6);
    assert_eq!(
        (out["rate_num"].as_u64(), out["rate_den"].as_u64()),
        (Some(24000), Some(1001))
    );
    let notes = out["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("no waveform")),
        "a picture sequence has no sound, and says so: {notes:?}"
    );
}
