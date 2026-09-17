//! Rendering a cut to pictures through the protocol, on real H.264.
//!
//! The media is written by this build's encoder (`cine_media::encode`), so the
//! test does not depend on files a machine may not have. Each picture of a file
//! carries its index — a luma level and a bar that moves — so the render can
//! be checked against the file picture by picture: a render that showed the
//! wrong picture, a repeated one, or a black one where the cut has a clip would
//! fail, not just a render that produced files.

mod common;

use cine_media::encode::{write_h264_mp4, H264Settings, YuvFrame};
use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

fn pattern(w: usize, h: usize, i: usize, base: usize) -> YuvFrame {
    let level = (base + i * 5 % 120) as u8;
    let mut y = vec![level; w * h];
    let bar = (i * 7) % (w - 16);
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

fn movie(dir: &Path, name: &str, w: usize, h: usize, count: usize, base: usize) -> PathBuf {
    let path = dir.join(name);
    write_h264_mp4(
        &path,
        w,
        h,
        Rate::F24,
        H264Settings::default(),
        (0..count).map(|i| pattern(w, h, i, base)),
    )
    .expect("encoded");
    path
}

fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(300);
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

fn f24(ticks: i64) -> Value {
    json!({ "ticks": ticks, "rate_num": 24, "rate_den": 1 })
}

/// A sequence, its picture track, and an ingested file placed on it.
fn sequence(c: &mut Client) -> (String, String) {
    let made = c.call(
        "new_timeline",
        json!({ "name": "Screener", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let v1 = c.call("timeline", json!({ "id": tl }))["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    (tl, v1)
}

fn place(c: &mut Client, v1: &str, path: &Path, at: i64, from: i64, count: i64) -> String {
    let asset = c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    c.call(
        "place",
        json!({ "tracks": [v1], "time": f24(at), "mode": "overwrite",
                "clip": { "media": asset, "source_range": { "start_time": f24(from), "duration": f24(count) } } }),
    );
    asset
}

/// The file's own picture `index`, decoded and converted the reference way.
fn decoded(path: &Path, index: usize) -> Vec<u8> {
    let mut reader = cine_media::VideoReader::open_path(path).unwrap();
    let frame = reader.frame_at_index(index).unwrap().unwrap();
    cine_runtime::frames::rgba8(&frame).unwrap().0
}

fn rendered(dir: &Path, n: i64) -> (Vec<u8>, u32, u32) {
    let bytes = std::fs::read(dir.join(cine_runtime::render::frame_file(n))).unwrap();
    let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

#[test]
fn a_cut_renders_picture_for_picture_with_gaps_black_and_captions_burned_in() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mp4", 320, 180, 48, 40);
    let b = movie(dir.path(), "B.mp4", 320, 180, 24, 150);
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let h = Harness::start_in(project_dir.clone());
    let mut c = h.client();
    let (tl, v1) = sequence(&mut c);
    // A's pictures 10..34 at the head; a ten-frame gap; B's pictures 0..12.
    place(&mut c, &v1, &a, 0, 10, 24);
    place(&mut c, &v1, &b, 34, 0, 12);
    c.call(
        "set_caption",
        json!({ "timeline": tl, "language": "en", "start": f24(5), "end": f24(15),
                "text": "We ride at dawn." }),
    );

    let began = Instant::now();
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "captions": "en" }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    println!(
        "MEASURED render of 46 frames at 320x180 with captions: {} ms (dev profile)",
        began.elapsed().as_millis()
    );
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    assert!(
        out.starts_with(project_dir.join("derived").join("render")),
        "a render is derived media: {}",
        out.display()
    );
    assert_eq!(done["output"]["frames"], 46);
    assert_eq!(done["output"]["captions_burned"], 10);

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["look"]["kind"], "as_decoded");
    assert_eq!(manifest["sources"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["captions"]["language"], "en");
    assert!(!project_dir.join("derived/render").read_dir().unwrap().any(|d| d
        .unwrap()
        .path()
        .read_dir()
        .unwrap()
        .any(|e| e.unwrap().path().to_string_lossy().ends_with(".partial"))));

    // Record frame n of the head clip is picture 10 + n of A, exactly.
    for n in [0i64, 4, 15, 23] {
        let (px, w, hgt) = rendered(&out, n);
        assert_eq!((w, hgt), (320, 180));
        assert!(px == decoded(&a, 10 + n as usize), "frame {n} is A picture {}", 10 + n);
    }
    // A caption changes the bottom of the picture and nothing above the safe
    // area's caption rows.
    let (px, _, _) = rendered(&out, 5);
    let reference = decoded(&a, 15);
    let row_bytes = 320 * 4;
    let top = 90 * row_bytes;
    assert!(px[..top] == reference[..top], "the top half is untouched");
    assert!(px[top..] != reference[top..], "the caption is in the bottom half");
    // The gap is black, opaque.
    let (px, _, _) = rendered(&out, 28);
    assert!(px.as_chunks::<4>().0.iter().all(|p| p == &[0, 0, 0, 255]));
    // After the gap, B from its first picture.
    for n in [34i64, 45] {
        let (px, _, _) = rendered(&out, n);
        assert!(px == decoded(&b, (n - 34) as usize), "frame {n} is B picture {}", n - 34);
    }
}

#[test]
fn a_render_it_cannot_stand_behind_is_refused_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mp4", 320, 180, 24, 40);
    let small = movie(dir.path(), "small.mp4", 160, 90, 24, 90);
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let h = Harness::start_in(project_dir.clone());
    let mut c = h.client();
    let (tl, v1) = sequence(&mut c);
    place(&mut c, &v1, &a, 0, 0, 12);
    place(&mut c, &v1, &small, 12, 0, 12);

    let refused = |c: &mut Client, params: Value| -> String {
        let job = c.call("job_submit", params);
        let done = wait_for(c, job["id"].as_u64().unwrap());
        assert_eq!(done["state"], "failed", "{done}");
        done["error"].as_str().unwrap().to_string()
    };

    let mixed = refused(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl }),
    );
    assert!(mixed.contains("160x90") && mixed.contains("320x180"), "{mixed}");

    let no_captions = refused(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": tl, "captions": "fr",
                "start_frame": 0, "end_frame": 12 }),
    );
    assert!(no_captions.contains("`fr`"), "{no_captions}");

    // Nothing was left behind by any refusal.
    let renders = project_dir.join("derived").join("render");
    let left: Vec<_> = walk(&renders);
    assert!(left.is_empty(), "refusals left files: {left:?}");

    // And a render that can be made, is.
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "start_frame": 0, "end_frame": 12 }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["output"]["frames"], 12);
}

#[test]
fn a_render_through_the_project_view_is_the_windows_bake_applied_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mp4", 320, 180, 24, 40);
    let project_dir = dir.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let h = Harness::start_in(project_dir);
    let mut c = h.client();
    let (tl, v1) = sequence(&mut c);
    place(&mut c, &v1, &a, 0, 0, 12);

    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "look": "project_view" }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    let look = &done["output"]["look"];
    assert_eq!(look["kind"], "project_view");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());

    // The same bake the window uses, built independently here from the graph.
    let (config, input, display, view, working) = {
        let project = &h.runtime.project;
        let g = project.engine.read();
        let (config, _) = cine_color::view::project_config(&g, project.root).expect("config");
        let (display, view) = cine_color::view::default_view(&config).unwrap();
        let input = cine_color::view::input_colorspaces(&g)
            .get(&a.display().to_string())
            .cloned()
            .expect("ingest establishes an input colorspace");
        let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
        (config, input, display, view, working)
    };
    assert_eq!(look["display"], display.as_str());
    assert_eq!(look["view"], view.as_str());
    let baked = cine_color::view::bake_view(&config, &input, &working, None, &display, &view)
        .expect("bakeable");
    assert_eq!(look["worst_lattice_error"].as_f64().unwrap() as f32, baked.worst_error);

    let mut reader = cine_media::VideoReader::open_path(&a).unwrap();
    for n in [0usize, 7] {
        let frame = reader.frame_at_index(n).unwrap().unwrap();
        let (mut px, ..) = cine_runtime::frames::rgba_f32(&frame).unwrap();
        for p in px.as_chunks_mut::<4>().0 {
            let [r, g, b] = baked.lut.apply([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&[r, g, b]);
        }
        let expected: Vec<u8> = px
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        let (got, _, _) = rendered(&out, n as i64);
        assert!(got == expected, "frame {n} is the view applied to picture {n}");
        assert!(got != decoded(&a, n), "the view changed the picture");
    }
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
