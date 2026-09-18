//! Relinking through the protocol, on real files.
//!
//! Identity is content. These tests move media, rename it, replace it, put a
//! different file under the same name, and leave two copies of the same take
//! in different folders — and check that the project follows the content every
//! time, refuses when it cannot establish identity, and keeps everything that
//! referenced the asset: clips, proxies, grades and renders.

mod common;

use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

fn picture(i: usize, seed: usize) -> YuvFrame {
    let (w, h) = (320usize, 180usize);
    let level = (40 + (i * 5 + seed * 37) % 160) as u8;
    let mut y = vec![level; w * h];
    let bar = (i * 11 + seed * 5) % (w - 16);
    for row in 0..h {
        for col in bar..bar + 16 {
            y[row * w + col] = 235;
        }
    }
    YuvFrame {
        width: w,
        height: h,
        y,
        u: vec![128; (w / 2) * (h / 2)],
        v: vec![128; (w / 2) * (h / 2)],
    }
}

/// A movie whose content depends on `seed`: two seeds are two different takes.
fn movie(path: &Path, frames: usize, seed: usize) -> PathBuf {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut w = MovWriter::create(
        path,
        320,
        180,
        Rate::F24,
        MovSettings {
            h264: H264Settings { qp: 18, gop: 12 },
            sound: Some(SoundFormat {
                sample_rate: 48_000,
                channels: 2,
            }),
            colour: ColourTags::BT709,
            timecode: None,
            first_picture: 0,
        },
    )
    .unwrap();
    for i in 0..frames {
        let due = w.sound_frames_due() as usize;
        let sound: Vec<f32> = (0..due * 2)
            .map(|k| ((k + seed * 991) % 2000) as f32 / 8_388_608.0)
            .collect();
        w.write_frame(&picture(i, seed), &sound).unwrap();
    }
    w.finish().unwrap();
    path.to_path_buf()
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

fn relink_scan(c: &mut Client, folder: &Path) -> Value {
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.relink", "folder": folder.display().to_string() }),
    );
    let done = wait_for(c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    done["output"].clone()
}

fn outcome<'a>(out: &'a Value, media: &str) -> &'a Value {
    out["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["media"] == media)
        .expect("an outcome for that media")
}

fn ingest(c: &mut Client, path: &Path) -> String {
    c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string()
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

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn media_that_moved_or_was_renamed_is_found_by_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("day1/A001C001.mov"), 12, 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);

    // Moved to another folder, and renamed on the way.
    let moved = cards.join("archive/2026/TAKE-ONE.mov");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&original, &moved).unwrap();
    c.call("check_media", json!({}));
    assert_eq!(media_view(&mut c, &media)["online"], false);

    let out = relink_scan(&mut c, &cards);
    assert_eq!(out["relinked"], 1, "{out}");
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "relinked", "{o}");
    assert_eq!(o["path"], json!(moved.display().to_string()));
    assert_eq!(o["confidence"], "fingerprint");

    let view = media_view(&mut c, &media);
    assert_eq!(view["online"], true);
    assert_eq!(view["path"], json!(moved.display().to_string()));
    assert_eq!(view["id"], json!(media), "the asset keeps its identity");
    assert_eq!(
        view["resolution"]["relinked_from"],
        json!(original.display().to_string())
    );

    // Scanning again says so, and changes nothing.
    let again = relink_scan(&mut c, &cards);
    assert_eq!(outcome(&again, &media)["state"], "already_here", "{again}");
}

#[test]
fn a_different_file_of_the_same_name_is_refused_and_the_reason_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("day1/A001C001.mov"), 12, 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);

    // The take is gone; another take of the same name is where it was.
    std::fs::remove_file(&original).unwrap();
    movie(&cards.join("day1/A001C001.mov"), 12, 2);
    let out = relink_scan(&mut c, &cards);
    assert_eq!(out["relinked"], 0, "{out}");
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "replaced", "{o}");
    let note = o["note"].as_str().unwrap();
    assert!(note.contains("different media now"), "{note}");

    let view = media_view(&mut c, &media);
    assert_eq!(
        view["online"], false,
        "it is not online because a namesake is there"
    );
    assert_eq!(
        view["path"],
        json!(original.display().to_string()),
        "and it still names its own file"
    );
    assert_eq!(view["resolution"]["state"], "replaced");
    assert!(view["resolution"]["note"]
        .as_str()
        .unwrap()
        .contains("separate decision"));

    // Relinking it by hand to that file is refused too, with both hashes.
    let refused = c.call_result(
        "relink",
        json!({ "media": media, "path": cards.join("day1/A001C001.mov").display().to_string() }),
    );
    let message = refused.unwrap_err();
    assert!(message.contains("is not `A001C001.mov`"), "{message}");
    assert!(
        message.contains("other pictures under every edit"),
        "{message}"
    );
}

#[test]
fn media_that_is_nowhere_stays_unresolved_and_says_what_was_looked_for() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("A001C001.mov"), 8, 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    std::fs::remove_file(&original).unwrap();

    let out = relink_scan(&mut c, &cards);
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "not_found", "{o}");
    assert!(o["note"].as_str().unwrap().contains("no file under"), "{o}");
    let view = media_view(&mut c, &media);
    assert_eq!(view["resolution"]["state"], "not_found");
    assert_eq!(view["online"], false);
}

#[test]
fn two_copies_of_the_same_take_are_not_an_ambiguity_and_the_other_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("day1/A001C001.mov"), 8, 1);
    let copy = cards.join("backup/A001C001.mov");
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(&original, &copy).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    std::fs::remove_file(&original).unwrap();

    let out = relink_scan(&mut c, &cards);
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "relinked", "{o}");
    assert_eq!(o["path"], json!(copy.display().to_string()));
    assert_eq!(
        o["duplicates"],
        json!([]),
        "there is only one copy left to find"
    );

    // Put the first one back: now both hold the content, and the scan says so.
    std::fs::copy(&copy, &original).unwrap();
    let out = relink_scan(&mut c, &cards);
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "already_here", "{o}");
    assert_eq!(
        o["duplicates"],
        json!([original.display().to_string()]),
        "the other copy is reported, not chosen: {o}"
    );
}

#[test]
fn a_verified_checksum_makes_a_relink_exact_rather_than_a_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("A001C001.mov"), 8, 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    // Establish the reference checksum, then move the file.
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.checksum", "media": media }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    let moved = cards.join("elsewhere/take.mov");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&original, &moved).unwrap();

    let out = relink_scan(&mut c, &cards);
    let o = outcome(&out, &media);
    assert_eq!(o["state"], "relinked", "{o}");
    assert_eq!(
        o["confidence"], "exact",
        "a verified checksum is checked in full: {o}"
    );
    assert_eq!(
        media_view(&mut c, &media)["resolution"]["confidence"],
        "exact"
    );
}

#[test]
fn everything_that_referenced_the_media_still_does_after_a_relink() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("A001C001.mov"), 24, 1);
    let root = project(dir.path());
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let media = ingest(&mut c, &original);

    // A cut, a proxy and a render of it.
    let made = c.call(
        "new_timeline",
        json!({ "name": "Cut", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let f24 = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": f24(0),
                "clip": { "media": media, "source_range": { "start_time": f24(0), "duration": f24(12) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.proxy", "media": media, "height": 90 }),
    );
    assert_eq!(
        wait_for(&mut c, job["id"].as_u64().unwrap())["state"],
        "succeeded"
    );
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "output": "png8", "start_frame": 0, "end_frame": 2 }),
    );
    assert_eq!(
        wait_for(&mut c, job["id"].as_u64().unwrap())["state"],
        "succeeded"
    );
    assert_eq!(
        c.call("renders", json!({ "timeline": tl }))["renders"][0]["state"],
        "current"
    );

    // Move the original and relink.
    let moved = cards.join("moved/A001C001.mov");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&original, &moved).unwrap();
    let out = relink_scan(&mut c, &cards);
    assert_eq!(out["relinked"], 1, "{out}");

    // The clip still plays it, from the new path.
    let shown = c.call("picture_at", json!({ "timeline": tl, "time": f24(3) }));
    assert_eq!(shown["asset"], json!(media));
    assert_eq!(shown["playable"], true, "{shown}");
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=3"));
    assert!(status.contains("200"), "{status}");
    assert!(body.starts_with(b"\x89PNG"));

    // The proxy is still this original's, and still current.
    let view = media_view(&mut c, &media);
    assert_eq!(view["proxies"][0]["state"], "current", "{view}");
    c.call("media_preference", json!({ "preference": "proxy" }));
    let shown = c.call("picture_at", json!({ "timeline": tl, "time": f24(3) }));
    assert!(
        shown["proxy"].is_string(),
        "the proxy still stands in: {shown}"
    );
    c.call("media_preference", json!({ "preference": "original" }));

    // And the render is still current: the media moved, it did not change.
    let renders = c.call("renders", json!({ "timeline": tl }));
    assert_eq!(
        renders["renders"][0]["state"], "current",
        "moving media must not invalidate a render: {renders}"
    );
}

/// Regression: "check media" asked only whether *a* file was at the path, so a
/// different take written over it reported the media as online, and the
/// application would have played the wrong pictures without a word.
#[test]
fn checking_media_does_not_call_a_namesake_online() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let original = movie(&cards.join("A001C001.mov"), 8, 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    assert_eq!(media_view(&mut c, &media)["online"], true);

    // Another take, written over it, with the same name and a different length.
    std::fs::remove_file(&original).unwrap();
    movie(&cards.join("A001C001.mov"), 16, 2);

    let checked = c.call("check_media", json!({}));
    let o = checked["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["media"] == json!(media))
        .unwrap();
    assert_eq!(o["state"], "replaced", "{o}");
    let view = media_view(&mut c, &media);
    assert_eq!(view["online"], false, "a namesake is not this media");
    assert_eq!(view["resolution"]["state"], "replaced");

    // Put the real take back: it is online again, with nothing left over.
    std::fs::remove_file(cards.join("A001C001.mov")).unwrap();
    movie(&cards.join("A001C001.mov"), 8, 1);
    c.call("check_media", json!({}));
    let view = media_view(&mut c, &media);
    assert_eq!(view["online"], true);
    assert_eq!(view["resolution"]["state"], Value::Null);
}
