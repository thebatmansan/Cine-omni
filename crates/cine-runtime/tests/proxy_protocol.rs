//! Proxies through the protocol, on real files: made as jobs, belonging to
//! their originals, read only when asked and only when current.
//!
//! Sources are written by this build's own movie writer, so each picture
//! carries its index and each sound sample a known integer: a proxy is checked
//! against its original picture for picture and sample for sample.

mod common;

use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use cine_media::{MediaReader, VideoReader};
use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

/// A picture whose bar sits at a fraction of the width that identifies it.
fn picture(w: usize, h: usize, i: usize) -> YuvFrame {
    let level = 40 + (i * 5 % 120) as u8;
    let mut y = vec![level; w * h];
    let bar = (i * 11) % (w - w / 8);
    for row in 0..h {
        for col in bar..bar + w / 16 {
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

fn code(n: i64, c: usize) -> i32 {
    let v = ((n * 37) % 1_000_000) as i32 - 500_000;
    if c == 0 { v } else { -v }
}

fn source(dir: &Path, name: &str, w: usize, h: usize, frames: usize, channels: u16) -> PathBuf {
    let path = dir.join(name);
    let mut writer = MovWriter::create(
        &path,
        w,
        h,
        Rate::F24,
        MovSettings {
            h264: H264Settings { qp: 16, gop: 12 },
            sound: (channels > 0).then_some(SoundFormat { sample_rate: 48_000, channels }),
            colour: ColourTags::BT709,
            first_picture: 0,
        },
    )
    .unwrap();
    let mut n = 0i64;
    for i in 0..frames {
        let due = writer.sound_frames_due() as i64;
        let mut sound = Vec::new();
        for k in 0..due {
            for c in 0..channels as usize {
                sound.push(code(n + k, c) as f32 / 8_388_608.0);
            }
        }
        n += due;
        writer.write_frame(&picture(w, h, i), &sound).unwrap();
    }
    writer.finish().unwrap();
    path
}

fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(900);
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

fn make_proxy(c: &mut Client, media: &str, height: u32) -> Value {
    let job = c.call("job_submit", json!({ "kind": "media.proxy", "media": media, "height": height }));
    wait_for(c, job["id"].as_u64().unwrap())
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

/// The bar's centre as a fraction of the picture's width.
fn bar_fraction(frame: &cine_media::VideoFrame) -> f64 {
    let (plane, stride) = &frame.planes[0];
    let (w, h) = (frame.width as usize, frame.height as usize);
    let row = h / 2;
    let cols: Vec<usize> = (0..w).filter(|&c| plane[row * stride + c] > 200).collect();
    assert!(!cols.is_empty(), "the picture has no bar");
    (cols.iter().sum::<usize>() as f64 / cols.len() as f64) / w as f64
}

fn sound_of(path: &Path) -> Vec<i64> {
    let mut r = MediaReader::open_path(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_audio_frame().unwrap() {
        for i in 0..f.frame_count() {
            out.push((f64::from(f.channels[0][i]) * 8_388_608.0).round() as i64);
        }
    }
    out
}

struct Seq {
    tl: String,
    v1: String,
    a1: String,
}

fn sequence_with(c: &mut Client, media: &str, frames: i64) -> Seq {
    let made = c.call("new_timeline", json!({ "name": "Cut", "rate_num": 24, "rate_den": 1 }));
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let (v1, a1) = (
        t["tracks"][0]["id"].as_str().unwrap().to_string(),
        t["tracks"][1]["id"].as_str().unwrap().to_string(),
    );
    let f24 = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [v1, a1], "time": f24(0),
                "clip": { "media": media, "source_range": { "start_time": f24(0), "duration": f24(frames) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    Seq { tl, v1, a1 }
}

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn a_proxy_is_made_as_a_job_and_is_its_originals_pictures_and_sound() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "A.mov", 640, 360, 48, 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);

    let done = make_proxy(&mut c, &media, 180);
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    assert_eq!((out["width"].as_u64(), out["height"].as_u64()), (Some(320), Some(180)));
    assert_eq!(out["frames"], 48);
    assert_eq!(out["sound"], true);
    println!(
        "MEASURED proxy 640x360 → 320x180 with sound: {} pictures in {} ms ({:.1} pictures/s, dev profile)",
        out["frames"], out["elapsed_ms"], out["pictures_per_second"].as_f64().unwrap()
    );

    // It belongs to its original, and is not media in its own right.
    let view = media_view(&mut c, &media);
    let proxy = view["proxies"][0].clone();
    assert_eq!(proxy["state"], "current", "{proxy}");
    assert_eq!(proxy["width"], 320);
    assert_eq!(proxy["height"], 180);
    assert_eq!(proxy["sound"], true);
    let proxy_path = PathBuf::from(proxy["path"].as_str().unwrap());
    let all = c.call("media", json!({}));
    assert!(
        all.as_array().unwrap().iter().all(|m| m["path"] != json!(proxy_path.display().to_string())),
        "a proxy is never media in its own right"
    );

    // Picture for picture, and the same sound.
    let mut o = VideoReader::open_path(&original).unwrap();
    let mut p = VideoReader::open_path(&proxy_path).unwrap();
    assert_eq!(p.frame_count(), o.frame_count());
    assert_eq!(p.time_base(), o.time_base());
    for i in [0usize, 17, 47] {
        let a = bar_fraction(&o.frame_at_index(i).unwrap().unwrap());
        let b = bar_fraction(&p.frame_at_index(i).unwrap().unwrap());
        assert!((a - b).abs() < 0.02, "picture {i}: bar at {a} of the original, {b} of the proxy");
    }
    let (so, sp) = (sound_of(&original), sound_of(&proxy_path));
    assert_eq!(sp.len(), so.len());
    assert!(sp == so, "the proxy's sound is the original's, sample for sample");

    // Asking again for the same proxy does not make it twice.
    let again = make_proxy(&mut c, &media, 180);
    assert_eq!(again["state"], "succeeded", "{again}");
    assert_eq!(again["output"]["already_current"], true, "{again}");
}

#[test]
fn playback_reads_a_proxy_only_when_asked_and_never_a_stale_one() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "A.mov", 640, 360, 24, 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    make_proxy(&mut c, &media, 180);
    let s = sequence_with(&mut c, &media, 24);
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });

    // By default the original plays, and nothing needs saying.
    assert_eq!(c.call("media_preference", json!({}))["preference"], "original");
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(5) }));
    assert_eq!(shown["proxy"], Value::Null);
    assert_eq!(shown["substitution"], Value::Null);
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=5"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 640);

    // Asked for proxies, the proxy plays — picture and sound.
    assert_eq!(
        c.call("media_preference", json!({ "preference": "proxy" }))["preference"],
        "proxy"
    );
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(5) }));
    let proxy = shown["proxy"].as_str().expect("the proxy plays").to_string();
    assert_eq!(shown["index"], 5, "the same picture of the same take");
    let list = c.call("edit_list", json!({ "timeline": s.tl }));
    assert_eq!(list["entries"][0]["proxy"], json!(proxy));
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=5&proxy={proxy}"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 320);

    // A proxy that changed on disk is never played, and says why.
    let proxy_path = PathBuf::from(media_view(&mut c, &media)["proxies"][0]["path"].as_str().unwrap());
    let kept = std::fs::read(&proxy_path).unwrap();
    std::fs::write(&proxy_path, [kept.clone(), vec![0u8; 64]].concat()).unwrap();
    let view = media_view(&mut c, &media);
    assert_eq!(view["proxies"][0]["state"], "stale", "{view}");
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(5) }));
    assert_eq!(shown["proxy"], Value::Null, "a stale proxy never plays");
    assert!(
        shown["substitution"].as_str().unwrap_or_default().contains("stale"),
        "and the reason is shown: {shown}"
    );
    let (status, _, _) = h.get(&format!("/frame?media={media}&index=5&proxy={proxy}"));
    assert!(status.contains("404"), "a stale proxy is not served: {status}");

    // Made again, it is current again.
    std::fs::write(&proxy_path, &kept).unwrap();
    let done = make_proxy(&mut c, &media, 180);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(media_view(&mut c, &media)["proxies"][0]["state"], "current");
}

#[test]
fn auto_reads_the_proxy_only_when_the_original_cannot_be_played() {
    let dir = tempfile::tempdir().unwrap();
    let small = source(dir.path(), "small.mov", 640, 360, 12, 2);
    let root = project(dir.path());
    let h = Harness::start_in(root);
    let mut c = h.client();
    let media = ingest(&mut c, &small);
    make_proxy(&mut c, &media, 180);
    let s = sequence_with(&mut c, &media, 12);
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call("media_preference", json!({ "preference": "auto" }));

    // An original this machine can play is played.
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(2) }));
    assert_eq!(shown["proxy"], Value::Null, "{shown}");

    // Offline, the proxy stands in — and says so.
    let moved = dir.path().join("moved.mov");
    std::fs::rename(&small, &moved).unwrap();
    let checked = c.call("check_media", json!({}));
    assert_eq!(checked["changed"][0]["online"], false, "{checked}");
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(2) }));
    assert!(shown["proxy"].is_string(), "{shown}");
    assert!(
        shown["substitution"].as_str().unwrap().contains("offline"),
        "{shown}"
    );
    assert_eq!(shown["playable"], true, "the cut plays on its proxies");

    // Back, and the original is played again.
    std::fs::rename(&moved, &small).unwrap();
    c.call("check_media", json!({}));
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(2) }));
    assert_eq!(shown["proxy"], Value::Null, "{shown}");
}

#[test]
fn auto_reads_the_proxy_of_an_original_larger_than_hd() {
    let dir = tempfile::tempdir().unwrap();
    // 1920×1082 is just past HD, so Auto prefers a proxy of it.
    let big = source(dir.path(), "big.mov", 1920, 1082, 4, 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &big);
    let began = Instant::now();
    let done = make_proxy(&mut c, &media, 540);
    assert_eq!(done["state"], "succeeded", "{done}");
    println!(
        "MEASURED proxy 1920x1082 → 958x540: {} pictures in {} ms ({:.2} pictures/s, dev profile), {} bytes",
        done["output"]["frames"],
        began.elapsed().as_millis(),
        done["output"]["pictures_per_second"].as_f64().unwrap(),
        done["output"]["bytes"]
    );
    println!("MEASURED per stage: {}", done["output"]["stages"]);
    let s = sequence_with(&mut c, &media, 4);
    c.call("media_preference", json!({ "preference": "auto" }));
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": json!({ "ticks": 1, "rate_num": 24, "rate_den": 1 }) }));
    assert!(shown["proxy"].is_string(), "{shown}");
    assert!(shown["substitution"].as_str().unwrap().contains("larger than HD"), "{shown}");
}

#[test]
fn a_proxy_of_many_channels_carries_no_sound_and_the_sound_stays_on_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "six.mov", 640, 360, 12, 6);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    let done = make_proxy(&mut c, &media, 180);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["output"]["sound"], false);
    let notes = done["output"]["notes"][0].as_str().unwrap();
    assert!(notes.contains("6 channels") && notes.contains("downmix"), "{notes}");

    let s = sequence_with(&mut c, &media, 12);
    c.call("media_preference", json!({ "preference": "proxy" }));
    let list = c.call("edit_list", json!({ "timeline": s.tl }));
    let picture = &list["entries"][0];
    assert!(picture["proxy"].is_string(), "the picture plays from the proxy: {picture}");
    let sound = &list["sound"][0];
    assert_eq!(sound["proxy"], Value::Null, "the sound stays on the original: {sound}");
    assert!(
        sound["substitution"].as_str().unwrap().contains("no sound"),
        "and says why: {sound}"
    );
}

#[test]
fn a_proxy_that_cannot_be_made_is_refused_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "A.mov", 640, 360, 12, 2);
    let root = project(dir.path());
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let media = ingest(&mut c, &original);

    // Not smaller is not a proxy.
    let refused = make_proxy(&mut c, &media, 360);
    assert_eq!(refused["state"], "failed");
    assert!(refused["error"].as_str().unwrap().contains("not be smaller"), "{refused}");

    // An original that is not there cannot be read.
    let moved = dir.path().join("moved.mov");
    std::fs::rename(&original, &moved).unwrap();
    let refused = make_proxy(&mut c, &media, 180);
    assert_eq!(refused["state"], "failed");
    assert!(refused["error"].as_str().unwrap().contains("offline"), "{refused}");
    assert!(walk(&root.join("derived")).is_empty(), "a refusal left files");
    assert_eq!(media_view(&mut c, &media)["proxies"], json!([]));

    // Retried once the original is back, the same job succeeds.
    let job = c.call("job_submit", json!({ "kind": "media.proxy", "media": media, "height": 180 }));
    let id = job["id"].as_u64().unwrap();
    let failed = wait_for(&mut c, id);
    assert_eq!(failed["state"], "failed", "{failed}");
    std::fs::rename(&moved, &original).unwrap();
    c.call("job_retry", json!({ "id": id }));
    let done = wait_for(&mut c, id);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(media_view(&mut c, &media)["proxies"][0]["state"], "current");
}

#[test]
fn a_cancelled_proxy_leaves_nothing_and_no_record_of_itself() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "long.mov", 1280, 720, 60, 2);
    let root = project(dir.path());
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    let job = c.call("job_submit", json!({ "kind": "media.proxy", "media": media, "height": 180 }));
    let id = job["id"].as_u64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let jobs = c.call("jobs", json!({}));
        let j = jobs.as_array().unwrap().iter().find(|j| j["id"] == id).unwrap().clone();
        if j["state"] == "running" && j["progress"].as_f64().unwrap_or(0.0) > 0.0 {
            break;
        }
        assert!(Instant::now() < deadline, "the proxy never started: {j}");
        std::thread::sleep(Duration::from_millis(10));
    }
    c.call("job_cancel", json!({ "id": id }));
    let done = wait_for(&mut c, id);
    assert!(done["state"] == "cancelled" || done["output"]["cancelled"] == true, "{done}");
    assert!(walk(&root.join("derived")).is_empty(), "a cancelled proxy left files");
    assert_eq!(media_view(&mut c, &media)["proxies"], json!([]));
}

#[test]
fn a_render_reads_the_original_whatever_is_being_watched() {
    let dir = tempfile::tempdir().unwrap();
    let original = source(dir.path(), "A.mov", 640, 360, 12, 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &original);
    make_proxy(&mut c, &media, 180);
    let s = sequence_with(&mut c, &media, 12);
    c.call("media_preference", json!({ "preference": "proxy" }));

    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "start_frame": 0, "end_frame": 2 }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["output"]["width"], 640, "a deliverable is made from originals");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let manifest: Value = serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["sources"][0]["path"],
        json!(original.display().to_string()),
        "the manifest names the original"
    );
    let _ = &s.v1;
    let _ = &s.a1;
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = dir.read_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
