//! Rendering movies with sound, at other sizes, at 16 bits, and knowing later
//! whether a render is still current — through the protocol, on real files.
//!
//! Source media is written by this build's own movie writer: every picture
//! carries its index (a luma level and a bar that moves) and every sound sample
//! a known integer, so a rendered movie is read back and checked picture by
//! picture and sample by sample against the files it came from.

mod common;

use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use cine_media::{MediaReader, VideoReader};
use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

const W: usize = 320;
const H: usize = 180;
/// Samples per picture at 24 fps and 48 kHz.
const SPF: i64 = 2000;

fn picture(i: usize) -> YuvFrame {
    let level = 40 + (i * 5 % 120) as u8;
    let mut y = vec![level; W * H];
    let bar = (i * 11) % (W - 16);
    for row in 0..H {
        for col in bar..bar + 16 {
            y[row * W + col] = 235;
        }
    }
    YuvFrame {
        width: W,
        height: H,
        y,
        u: vec![128; (W / 2) * (H / 2)],
        v: vec![128; (W / 2) * (H / 2)],
    }
}

/// The integer a coded source holds at sample `n`, channel `c`.
fn code(seed: i64, n: i64, c: usize) -> i32 {
    let v = ((n * 37 + seed * 100_003) % 1_000_000) as i32 - 500_000;
    if c == 0 {
        v
    } else {
        -v
    }
}

enum Sound {
    Coded(i64),
    /// One impulse at the first sample of every picture.
    Ticks,
}

fn movie(dir: &Path, name: &str, frames: usize, sound: Sound) -> PathBuf {
    let path = dir.join(name);
    let mut w = MovWriter::create(
        &path,
        W,
        H,
        Rate::F24,
        MovSettings {
            h264: H264Settings { qp: 16, gop: 12 },
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
    let mut n = 0i64;
    for i in 0..frames {
        let due = w.sound_frames_due() as i64;
        let mut s = Vec::with_capacity(due as usize * 2);
        for k in 0..due {
            for c in 0..2 {
                s.push(match sound {
                    Sound::Coded(seed) => code(seed, n + k, c) as f32 / 8_388_608.0,
                    Sound::Ticks => {
                        if k == 0 {
                            0.8
                        } else {
                            0.0
                        }
                    }
                });
            }
        }
        n += due;
        w.write_frame(&picture(i), &s).unwrap();
    }
    w.finish().unwrap();
    path
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

fn render(c: &mut Client, params: Value) -> Value {
    let job = c.call("job_submit", params);
    wait_for(c, job["id"].as_u64().unwrap())
}

fn f24(ticks: i64) -> Value {
    json!({ "ticks": ticks, "rate_num": 24, "rate_den": 1 })
}

struct Seq {
    tl: String,
    v1: String,
    a1: String,
}

fn sequence(c: &mut Client) -> Seq {
    let made = c.call(
        "new_timeline",
        json!({ "name": "Deliverable", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    Seq {
        v1: t["tracks"][0]["id"].as_str().unwrap().to_string(),
        a1: t["tracks"][1]["id"].as_str().unwrap().to_string(),
        tl,
    }
}

/// Place pictures `from..from+count` of `path` at record frame `at`, on
/// picture and sound together. Returns the placed clip ids.
fn place(c: &mut Client, s: &Seq, path: &Path, at: i64, from: i64, count: i64) -> Vec<String> {
    let asset = c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let placed = c.call(
        "place",
        json!({ "tracks": [s.v1, s.a1], "time": f24(at), "mode": "overwrite",
                "clip": { "media": asset, "source_range": { "start_time": f24(from), "duration": f24(count) } } }),
    );
    placed["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().or(v.as_str()).unwrap().to_string())
        .collect()
}

fn bar_centre(frame: &cine_media::VideoFrame) -> Option<f64> {
    let (plane, stride) = &frame.planes[0];
    let (w, h) = (frame.width as usize, frame.height as usize);
    let row = h / 2;
    let cols: Vec<usize> = (0..w).filter(|&c| plane[row * stride + c] > 200).collect();
    (!cols.is_empty()).then(|| cols.iter().sum::<usize>() as f64 / cols.len() as f64)
}

fn expected_bar(i: usize) -> f64 {
    ((i * 11) % (W - 16)) as f64 + 7.5
}

/// The movie's sound as integers, per channel.
fn movie_sound(path: &Path) -> (Vec<i64>, Vec<i64>) {
    let mut r = MediaReader::open_path(path).unwrap();
    let (mut l, mut rr) = (Vec::new(), Vec::new());
    while let Some(f) = r.next_audio_frame().unwrap() {
        for i in 0..f.frame_count() {
            l.push((f64::from(f.channels[0][i]) * 8_388_608.0).round() as i64);
            rr.push((f64::from(f.channels[1][i]) * 8_388_608.0).round() as i64);
        }
    }
    (l, rr)
}

#[test]
fn a_movie_of_the_cut_has_its_pictures_in_order_and_its_sound_bit_exact() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mov", 48, Sound::Coded(1));
    let b = movie(dir.path(), "B.mov", 24, Sound::Coded(2));
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let s = sequence(&mut c);
    // A's pictures 10..34 at the head; a ten-frame gap; B's pictures 0..12.
    place(&mut c, &s, &a, 0, 10, 24);
    place(&mut c, &s, &b, 34, 0, 12);

    let began = Instant::now();
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie" }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    println!(
        "MEASURED movie of 46 frames 320x180 with 48 kHz stereo sound: {} ms (dev profile)",
        began.elapsed().as_millis()
    );
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let mov = out.join("movie.mov");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["sound"]["frames"], 46 * SPF);
    assert_eq!(manifest["sound"]["clipped_samples"], 0);

    let mut v = VideoReader::open_path(&mov).unwrap();
    assert_eq!(v.frame_count(), 46);
    for n in 0..46usize {
        let f = v.frame_at_index(n).unwrap().unwrap();
        let bar = bar_centre(&f);
        match n {
            0..=23 => {
                let got = bar.expect("A has a bar");
                assert!(
                    (got - expected_bar(10 + n)).abs() <= 2.0,
                    "frame {n}: A picture {}",
                    10 + n
                );
            }
            24..=33 => assert!(bar.is_none(), "frame {n} is the gap: black"),
            _ => {
                let got = bar.expect("B has a bar");
                assert!(
                    (got - expected_bar(n - 34)).abs() <= 2.0,
                    "frame {n}: B picture {}",
                    n - 34
                );
            }
        }
    }

    let (l, r) = movie_sound(&mov);
    assert_eq!(l.len() as i64, 46 * SPF);
    for sample in 0..46 * SPF {
        let (el, er) = match sample {
            s if s < 24 * SPF => (code(1, 10 * SPF + s, 0), code(1, 10 * SPF + s, 1)),
            s if s < 34 * SPF => (0, 0),
            s => (code(2, s - 34 * SPF, 0), code(2, s - 34 * SPF, 1)),
        };
        assert_eq!(
            (l[sample as usize], r[sample as usize]),
            (i64::from(el), i64::from(er)),
            "sample {sample}"
        );
    }

    // A picture of the cut's own media travels with its colour declared, not
    // assumed, in a header a browser reads exactly.
    let asset = c.call("ingest", json!({ "path": a.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, headers, _) = h.get(&format!("/frame?media={asset}&index=3"));
    assert_eq!(
        common::header(&headers, "x-cine-colour-assumed"),
        Some("false"),
        "{headers:?}"
    );
    assert!(common::header(&headers, "x-cine-colour")
        .unwrap()
        .is_ascii());

    // What the application serves: the movie itself, and a picture decoded
    // from it; nothing outside the project's renders.
    let name = out.file_name().unwrap().to_string_lossy().into_owned();
    let (status, headers, body) = h.get(&format!(
        "/render-file?timeline={}&render={name}&file=movie.mov",
        s.tl
    ));
    assert!(status.contains("200"), "{status}");
    assert_eq!(
        body,
        std::fs::read(&mov).unwrap(),
        "the served movie is the file"
    );
    assert_eq!(
        common::header(&headers, "content-type"),
        Some("video/quicktime")
    );
    let (status, _, body) = h.get(&format!(
        "/render-frame?timeline={}&render={name}&index=40",
        s.tl
    ));
    assert!(
        status.contains("200") && body.starts_with(b"\x89PNG"),
        "{status}"
    );
    for bad in [
        format!("/render-file?timeline={}&render=..&file=movie.mov", s.tl),
        format!(
            "/render-file?timeline={}&render={name}&file=../../project.cine",
            s.tl
        ),
        format!(
            "/render-file?timeline={}&render={name}/..&file=manifest.json",
            s.tl
        ),
    ] {
        let (status, _, _) = h.get(&bad);
        assert!(status.contains("404"), "{bad} was served: {status}");
    }

    // Marked frames 30..40: ten pictures, and exactly their sound.
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie", "start_frame": 30, "end_frame": 40 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let mov = PathBuf::from(done["output"]["path"].as_str().unwrap()).join("movie.mov");
    assert_eq!(VideoReader::open_path(&mov).unwrap().frame_count(), 10);
    let (l, _) = movie_sound(&mov);
    assert_eq!(l.len() as i64, 10 * SPF);
    assert!(
        l[..(4 * SPF) as usize].iter().all(|v| *v == 0),
        "frames 30–33 are the gap"
    );
    for k in 0..(6 * SPF) {
        assert_eq!(
            l[(4 * SPF + k) as usize],
            i64::from(code(2, k, 0)),
            "B sample {k}"
        );
    }

    // Without sound, by choice: no sound track at all.
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie", "sound": false, "start_frame": 0, "end_frame": 4 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let mov = PathBuf::from(done["output"]["path"].as_str().unwrap()).join("movie.mov");
    assert!(cine_media::probe::probe(&mov)
        .unwrap()
        .first_audio()
        .is_none());

    // Sound asked for where the cut has none is refused, not written silent.
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie", "sound": true, "start_frame": 25, "end_frame": 33 }),
    );
    assert_eq!(refused["state"], "failed");
    assert!(
        refused["error"].as_str().unwrap().contains("no sound"),
        "{refused}"
    );
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "sound": true }),
    );
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("PNG frames carry no sound"),
        "{refused}"
    );
}

#[test]
fn a_retimed_clip_renders_its_pictures_and_its_sound_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    let t = movie(dir.path(), "T.mov", 96, Sound::Ticks);
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root);
    let mut c = h.client();
    let s = sequence(&mut c);
    let clips = place(&mut c, &s, &t, 0, 0, 96);
    // Twice speed, varispeed: 96 pictures in 48 frames.
    for clip in &clips {
        c.call("retime", json!({ "clip": clip, "played": f24(48) }));
    }
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie" }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let mov = PathBuf::from(done["output"]["path"].as_str().unwrap()).join("movie.mov");

    let mut v = VideoReader::open_path(&mov).unwrap();
    assert_eq!(v.frame_count(), 48);
    for n in 0..48usize {
        let f = v.frame_at_index(n).unwrap().unwrap();
        let got = bar_centre(&f).unwrap();
        assert!(
            (got - expected_bar(2 * n)).abs() <= 2.0,
            "frame {n} shows picture {}",
            2 * n
        );
    }

    // Each source picture's tick sounds where that picture is shown: source
    // sample 2000·k is heard at record sample 1000·k.
    let (l, _) = movie_sound(&mov);
    assert_eq!(l.len() as i64, 48 * SPF);
    let mut worst = 0i64;
    for k in 1..95i64 {
        let expected = 1000 * k;
        let lo = (expected - 400) as usize;
        let hi = ((expected + 400) as usize).min(l.len());
        let peak = (lo..hi).max_by_key(|i| l[*i].abs()).unwrap() as i64;
        worst = worst.max((peak - expected).abs());
    }
    println!(
        "MEASURED retimed (2x varispeed) tick placement: worst {worst} samples from the picture"
    );
    assert!(worst <= 1, "sound drifts from picture by {worst} samples");
}

#[test]
fn a_render_scales_only_as_asked_and_says_how() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mov", 24, Sound::Coded(1));
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root);
    let mut c = h.client();
    let s = sequence(&mut c);
    place(&mut c, &s, &a, 0, 0, 12);

    // Same shape, half size: scaled, nothing else.
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "width": 160, "height": 90 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let img = image::open(out.join("frame_000005.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(img.dimensions(), (160, 90));
    let row = 45;
    let cols: Vec<u32> = (0..160)
        .filter(|x| img.get_pixel(*x, row)[0] > 200)
        .collect();
    let centre = cols.iter().sum::<u32>() as f64 / cols.len() as f64;
    assert!(
        (centre - (expected_bar(5) + 0.5) / 2.0 + 0.5).abs() <= 1.5,
        "bar at {centre}"
    );

    // A different shape needs a decision.
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "width": 320, "height": 320 }),
    );
    assert_eq!(refused["state"], "failed");
    assert!(
        refused["error"].as_str().unwrap().contains("letterbox"),
        "{refused}"
    );

    // Letterboxed: the picture whole, bars above and below.
    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "width": 320, "height": 320, "fit": "letterbox", "start_frame": 0, "end_frame": 2 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let img = image::open(out.join("frame_000000.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(img.get_pixel(100, 30).0, [0, 0, 0, 255], "bar above");
    assert_eq!(img.get_pixel(100, 300).0, [0, 0, 0, 255], "bar below");
    assert!(img.get_pixel(100, 160)[0] > 20, "picture in the middle");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["sources"][0]["placement"]["covers"],
        json!([0, 70, 320, 180])
    );
    assert_eq!(manifest["fit"], "letterbox");

    // An odd-sized movie is refused before anything is written.
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie", "width": 161, "height": 91, "fit": "stretch" }),
    );
    assert!(
        refused["error"].as_str().unwrap().contains("even"),
        "{refused}"
    );
}

#[test]
fn sixteen_bit_frames_keep_what_the_view_computed() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mov", 12, Sound::Coded(1));
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root);
    let mut c = h.client();
    let s = sequence(&mut c);
    place(&mut c, &s, &a, 0, 0, 12);

    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png16", "look": "project_view", "start_frame": 3, "end_frame": 4 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = PathBuf::from(done["output"]["path"].as_str().unwrap());
    let img = image::open(out.join("frame_000003.png"))
        .unwrap()
        .to_rgba16();

    // Independently: the file's picture 3 through the same bake.
    let (config, input, display, view, working) = {
        let project = &h.runtime.project;
        let g = project.engine.read();
        let (config, _) = cine_color::view::project_config(&g, project.root).unwrap();
        let (display, view) = cine_color::view::default_view(&config).unwrap();
        let input = cine_color::view::input_colorspaces(&g)[&a.display().to_string()].clone();
        let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
        (config, input, display, view, working)
    };
    let baked =
        cine_color::view::bake_view(&config, &input, &working, None, &display, &view).unwrap();
    let frame = VideoReader::open_path(&a)
        .unwrap()
        .frame_at_index(3)
        .unwrap()
        .unwrap();
    let (px, ..) = cine_runtime::frames::rgba_f32(&frame).unwrap();
    let mut off_grid = 0usize;
    for (i, p) in px.chunks(4).enumerate() {
        let [r, g, b] = baked.lut.apply([p[0], p[1], p[2]]);
        let got = img.as_raw()[i * 4..i * 4 + 3].to_vec();
        for (k, v) in [r, g, b].into_iter().enumerate() {
            let want = (f64::from(v.clamp(0.0, 1.0)) * 65535.0).round() as u16;
            assert_eq!(got[k], want, "pixel {i} channel {k}");
            if !want.is_multiple_of(257) {
                off_grid += 1;
            }
        }
    }
    // An 8-bit image widened to 16 bits has every value on multiples of 257.
    // These frames do not: the view's precision was kept, not quantised away.
    println!(
        "MEASURED 16-bit samples not representable in 8 bits: {off_grid} of {}",
        px.len() / 4 * 3
    );
    assert!(
        off_grid > px.len() / 4,
        "the 16-bit render was quantised to 8 bits"
    );
}

#[test]
fn a_cancelled_movie_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mov", 48, Sound::Coded(1));
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let s = sequence(&mut c);
    for k in 0..8 {
        place(&mut c, &s, &a, k * 48, 0, 48);
    }
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "movie", "width": 1280, "height": 720 }),
    );
    let id = job["id"].as_u64().unwrap();
    // Cancel once it has visibly started writing.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let jobs = c.call("jobs", json!({}));
        let j = jobs
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id)
            .unwrap()
            .clone();
        if j["state"] == "running" && j["progress"].as_f64().unwrap_or(0.0) > 0.0 {
            break;
        }
        assert!(Instant::now() < deadline, "the render never started: {j}");
        std::thread::sleep(Duration::from_millis(10));
    }
    c.call("job_cancel", json!({ "id": id }));
    let done = wait_for(&mut c, id);
    assert!(
        done["state"] == "cancelled" || done["output"]["cancelled"] == true,
        "{done}"
    );
    let renders = root.join("derived").join("render");
    let left: Vec<_> = walk(&renders);
    assert!(left.is_empty(), "a cancelled render left files: {left:?}");
}

#[test]
fn a_render_knows_when_it_is_stale_and_only_then() {
    let dir = tempfile::tempdir().unwrap();
    let a = movie(dir.path(), "A.mov", 48, Sound::Coded(1));
    let b = movie(dir.path(), "B.mov", 24, Sound::Coded(2));
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let h = Harness::start_in(root);
    let mut c = h.client();
    let s = sequence(&mut c);
    place(&mut c, &s, &a, 0, 10, 24);
    place(&mut c, &s, &b, 34, 0, 12);

    let done = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "start_frame": 0, "end_frame": 12 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let state = |c: &mut Client| -> Value {
        let r = c.call("renders", json!({ "timeline": s.tl }));
        r["renders"][0].clone()
    };
    assert_eq!(state(&mut c)["state"], "current", "{}", state(&mut c));

    // Unrelated: another sequence, and a trim of A's tail and of B, both
    // outside frames 0–11.
    c.call("new_timeline", json!({ "name": "Other" }));
    c.call(
        "edit",
        json!({ "track": s.v1, "op": { "op": "ripple_trim", "index": 0, "edge": "tail", "delta": f24(-2) } }),
    );
    let st = state(&mut c);
    assert_eq!(
        st["state"], "current",
        "an edit outside the range is not a change to it: {st}"
    );

    // Related: trimming A's head changes which pictures frames 0–11 show.
    c.call(
        "edit",
        json!({ "track": s.v1, "op": { "op": "ripple_trim", "index": 0, "edge": "head", "delta": f24(2) } }),
    );
    let st = state(&mut c);
    assert_eq!(st["state"], "stale", "{st}");
    assert!(
        st["reasons"][0].as_str().unwrap().contains("the cut"),
        "{st}"
    );

    // Undo it and the render is current again: the same inputs.
    c.call("undo", json!({}));
    let st = state(&mut c);
    assert_eq!(st["state"], "current", "{st}");

    // The file itself changed on disk.
    let original = std::fs::read(&a).unwrap();
    let mut changed = original.clone();
    let n = changed.len();
    changed[n / 2] ^= 0xFF;
    std::fs::write(&a, &changed).unwrap();
    let st = state(&mut c);
    assert_eq!(st["state"], "stale", "{st}");
    assert!(
        st["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("changed on disk")),
        "{st}"
    );
    std::fs::write(&a, &original).unwrap();
    assert_eq!(state(&mut c)["state"], "current");

    // A frame of the render removed: the render cannot be trusted.
    let out = PathBuf::from(state(&mut c)["path"].as_str().unwrap());
    std::fs::remove_file(out.join("frame_000004.png")).unwrap();
    let st = state(&mut c);
    assert_eq!(st["state"], "invalid", "{st}");

    // A whole-cut render records the frames it holds, and goes stale when the
    // cut is longer or shorter — while a ranged render of frames the change
    // did not reach stays current.
    let whole = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8" }),
    );
    assert_eq!(whole["state"], "succeeded", "{whole}");
    let whole_name = PathBuf::from(whole["output"]["path"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let find = |c: &mut Client, name: &str| -> Value {
        let r = c.call("renders", json!({ "timeline": s.tl }));
        r["renders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(find(&mut c, &whole_name)["state"], "current");
    c.call(
        "edit",
        json!({ "track": s.v1, "op": { "op": "ripple_trim", "index": 2, "edge": "tail", "delta": f24(-4) } }),
    );
    let st = find(&mut c, &whole_name);
    assert_eq!(st["state"], "stale", "{st}");
    assert!(
        st["reasons"][0].as_str().unwrap().contains("frames 0–43"),
        "the reason names the frames rendered (A was ripple-trimmed by two above): {st}"
    );
    c.call("undo", json!({}));

    // A failed render is listed as failed.
    let refused = render(
        &mut c,
        json!({ "kind": "render.sequence", "timeline": s.tl, "output": "png8", "captions": "de" }),
    );
    assert_eq!(refused["state"], "failed");
    let r = c.call("renders", json!({ "timeline": s.tl }));
    assert_eq!(r["failed"][0]["state"], "failed", "{r}");
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
            out.push(p);
        } else {
            out.push(p);
        }
    }
    out
}
