//! Waveforms through the protocol, from real decoded sound.
//!
//! The envelope has to be the recording's own, aligned to its samples, usable
//! at any zoom, and honest about when it stops matching its source. These tests
//! write sound whose shape is known exactly — silence, then a loud passage,
//! then a quiet one — and check the peaks against it.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SR: u32 = 48_000;

/// A recording in three parts: two seconds of near-silence, one loud, one
/// quiet. The left channel is louder than the right throughout, so channels
/// cannot be confused with each other.
fn sound_at(n: u64, channel: usize) -> f32 {
    let second = n / u64::from(SR);
    let level = match second {
        0 | 1 => 0.002,
        2 => 0.9,
        _ => 0.2,
    };
    let level = if channel == 0 { level } else { level * 0.5 };
    let phase = (n % 120) as f32 / 120.0 * std::f32::consts::TAU;
    level * phase.sin()
}

/// A 24-bit stereo WAV of `seconds`, whose shape is `sound_at`.
fn wav(path: &Path, seconds: u64) -> PathBuf {
    let frames = seconds * u64::from(SR);
    let mut data = Vec::with_capacity(frames as usize * 6);
    for n in 0..frames {
        for c in 0..2 {
            let v = (sound_at(n, c).clamp(-1.0, 1.0) * 8_388_607.0) as i32;
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
    b.extend_from_slice(&SR.to_le_bytes());
    b.extend_from_slice(&(SR * 6).to_le_bytes());
    b.extend_from_slice(&6u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    std::fs::write(path, b).unwrap();
    path.to_path_buf()
}

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
            if matches!(job["state"].as_str().unwrap(), "succeeded" | "failed" | "cancelled") {
                return job.clone();
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn make(c: &mut Client, media: &str) -> Value {
    let job = c.call("job_submit", json!({ "kind": "media.waveform", "media": media }));
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

/// The largest absolute peak in a window, per channel.
fn loudest(peaks: &Value, channels: usize) -> Vec<i64> {
    let list = peaks.as_array().unwrap();
    let mut out = vec![0i64; channels];
    for (i, p) in list.iter().enumerate() {
        let c = i % channels;
        let lo = p[0].as_i64().unwrap().abs();
        let hi = p[1].as_i64().unwrap().abs();
        out[c] = out[c].max(lo.max(hi));
    }
    out
}

#[test]
fn a_waveform_is_the_recordings_own_shape_at_every_zoom() {
    let dir = tempfile::tempdir().unwrap();
    let source = wav(&dir.path().join("boom.wav"), 4);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &source);

    let done = make(&mut c, &media);
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    assert_eq!(out["channels"], 2);
    assert_eq!(out["sample_rate"], SR);
    assert_eq!(out["frames"], 4 * SR);
    println!(
        "MEASURED waveform of 4 s of 48 kHz 24-bit stereo: {} ms, {} bytes of peaks, {:.0}x real time (dev)",
        out["elapsed_ms"], out["bytes"], out["seconds_per_second"].as_f64().unwrap()
    );
    // The overview is a fraction of the sound it describes.
    let audio_bytes = 4 * u64::from(SR) * 6;
    assert!(
        out["bytes"].as_u64().unwrap() * 8 < audio_bytes,
        "peaks are much smaller than the audio: {} vs {audio_bytes}",
        out["bytes"]
    );

    let view = media_view(&mut c, &media);
    assert_eq!(view["waveform"]["state"], "current", "{}", view["waveform"]);
    assert_eq!(view["waveform"]["frames"], 4 * SR);

    // The loud second is loud, the quiet ones are quiet — in the peaks, per
    // channel, with the left louder than the right as it was written.
    let ask = |c: &mut Client, from: i64, to: i64, buckets: u64| -> Value {
        c.call(
            "waveform",
            json!({ "media": media, "from_sample": from, "to_sample": to, "buckets": buckets }),
        )
    };
    let loud = ask(&mut c, 2 * i64::from(SR), 3 * i64::from(SR), 200);
    let quiet = ask(&mut c, 0, i64::from(SR), 200);
    // Inside the quiet passage, not from its first sample: a window starts on
    // a bucket edge, so asking from exactly 3 s would include the bucket that
    // straddles the loud second before it.
    let after = ask(&mut c, 3 * i64::from(SR) + 10_000, 4 * i64::from(SR) - 10_000, 200);
    let loud_peak = loudest(&loud["peaks"], 2);
    let quiet_peak = loudest(&quiet["peaks"], 2);
    let after_peak = loudest(&after["peaks"], 2);
    assert!(loud_peak[0] > 29_000, "the loud passage is loud: {loud_peak:?}");
    assert!(quiet_peak[0] < 200, "the near-silence is near-silent: {quiet_peak:?}");
    assert!(
        (5_000..8_000).contains(&after_peak[0]),
        "and the quiet passage is in between: {after_peak:?}"
    );
    for w in [&loud_peak, &after_peak] {
        assert!(
            (w[0] as f64 / w[1] as f64 - 2.0).abs() < 0.1,
            "the left channel is twice the right, as written: {w:?}"
        );
    }

    // Zoom: a wide window uses a coarse level, a narrow one the finest, and
    // both start on a bucket edge so they can be drawn where they belong.
    let wide = ask(&mut c, 0, 4 * i64::from(SR), 100);
    let narrow = ask(&mut c, 96_000, 96_000 + 4_000, 400);
    assert!(
        wide["bucket"].as_u64().unwrap() > narrow["bucket"].as_u64().unwrap(),
        "a wider view uses coarser buckets: {} vs {}",
        wide["bucket"], narrow["bucket"]
    );
    assert!(
        wide["peaks"].as_array().unwrap().len() / 2 <= 101,
        "and fits what was asked for"
    );
    assert_eq!(narrow["bucket"], 256, "the finest level for a close view");
    let first = narrow["first_sample"].as_i64().unwrap();
    assert_eq!(first % narrow["bucket"].as_i64().unwrap(), 0, "windows start on a bucket edge");
    assert!(first <= 96_000);

    // Asking again does not make it again: the job system knows this work by
    // its key, and hands back the job that already did it.
    let before = c.call("jobs", json!({})).as_array().unwrap().len();
    let again = c.call("job_submit", json!({ "kind": "media.waveform", "media": media }));
    assert_eq!(again["state"], "succeeded", "{again}");
    assert_eq!(
        c.call("jobs", json!({})).as_array().unwrap().len(),
        before,
        "no second job was made for work already done"
    );
    assert_eq!(again["output"]["frames"], 4 * SR, "and it is the same overview: {again}");
}

#[test]
fn a_waveform_goes_stale_with_its_source_and_survives_a_relink() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    std::fs::create_dir_all(&cards).unwrap();
    let source = wav(&cards.join("take.wav"), 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = ingest(&mut c, &source);
    assert_eq!(make(&mut c, &media)["state"], "succeeded");
    assert_eq!(media_view(&mut c, &media)["waveform"]["state"], "current");

    // Moved and relinked: the content did not change, so the waveform is still
    // this recording's envelope.
    let moved = cards.join("archive/take.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&source, &moved).unwrap();
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.relink", "folder": cards.display().to_string() }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["output"]["relinked"], 1, "{done}");
    let view = media_view(&mut c, &media);
    assert_eq!(view["waveform"]["state"], "current", "a relink is not a content change: {}", view["waveform"]);
    // And it still answers.
    let peaks = c.call(
        "waveform",
        json!({ "media": media, "from_sample": 0, "to_sample": 48_000, "buckets": 50 }),
    );
    assert!(!peaks["peaks"].as_array().unwrap().is_empty());

    // Its own file damaged: stale, and it refuses to be drawn from.
    let waveform_file = walk(&project(dir.path()).join("derived").join("waveform"))
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "cinewav"))
        .expect("the overview is on disk");
    let kept = std::fs::read(&waveform_file).unwrap();
    std::fs::write(&waveform_file, [kept.clone(), vec![0u8; 16]].concat()).unwrap();
    let view = media_view(&mut c, &media);
    assert_eq!(view["waveform"]["state"], "stale", "{}", view["waveform"]);
    let refused = c
        .call_result(
            "waveform",
            json!({ "media": media, "from_sample": 0, "to_sample": 48_000, "buckets": 50 }),
        )
        .unwrap_err();
    assert!(refused.contains("stale"), "{refused}");

    // Made again, it is current again.
    std::fs::write(&waveform_file, &kept).unwrap();
    assert_eq!(make(&mut c, &media)["state"], "succeeded");
    assert_eq!(media_view(&mut c, &media)["waveform"]["state"], "current");
}

#[test]
fn media_with_no_sound_is_refused_and_a_cancelled_waveform_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path());
    let h = Harness::start_in(root.clone());
    let mut c = h.client();

    // A picture-only file has no envelope to draw.
    let silent = dir.path().join("silent.mov");
    cine_media::testsignal::bars_and_pops(&silent, 64, 36, ucg_core::Rate::F24, 1).unwrap();
    // Bars carry sound; a still image does not.
    let still = dir.path().join("plate.png");
    image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(8, 8, image::Rgba([1u8, 2, 3, 255])))
        .save(&still)
        .unwrap();
    let media = ingest(&mut c, &still);
    let refused = make(&mut c, &media);
    assert_eq!(refused["state"], "failed", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("no sound"), "{refused}");
    assert!(
        walk(&root.join("derived").join("waveform")).is_empty(),
        "a refusal left files"
    );

    // A long recording, cancelled while it decodes: nothing kept, no record.
    let long = wav(&dir.path().join("long.wav"), 180);
    let media = ingest(&mut c, &long);
    let job = c.call("job_submit", json!({ "kind": "media.waveform", "media": media }));
    let id = job["id"].as_u64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let jobs = c.call("jobs", json!({}));
        let j = jobs.as_array().unwrap().iter().find(|j| j["id"] == id).unwrap().clone();
        if j["state"] == "running" {
            break;
        }
        assert!(Instant::now() < deadline, "it never started: {j}");
        std::thread::sleep(Duration::from_millis(5));
    }
    c.call("job_cancel", json!({ "id": id }));
    let done = wait_for(&mut c, id);
    assert!(
        done["state"] == "cancelled" || done["output"]["cancelled"] == true,
        "{done}"
    );
    assert!(
        walk(&root.join("derived").join("waveform")).is_empty(),
        "a cancelled waveform left files"
    );
    assert_eq!(media_view(&mut c, &media)["waveform"], Value::Null);
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
