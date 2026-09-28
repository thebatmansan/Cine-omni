//! Grading: the colourist's stack, from the protocol to the picture.
//!
//! What these exist for. A `GradeStack` — layers of operations with masks and
//! qualifiers — has been persisted on every `color.grade` node since the
//! colour engine was built, and nothing ever read it into a picture: the
//! viewer evaluated the ASC CDL on the clip and stopped. The model was stored,
//! its maths was tested, and no frame was ever put through it. These hold the
//! whole path: read what the project has, change a real operation, evaluate it
//! through the real engine, see the picture change, undo it, and find the same
//! state after a reopen.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use ucg_core::Rate;

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// A cut of one real take: the sequence, the clip on it, and the media it
/// plays. The media id is returned because the test harness seeds an imported
/// cut whose media were never probed — a test that picks `media[0]` gets one of
/// those, not the take it just ingested.
fn a_clip(c: &mut Client, source: &Path) -> (String, String, String) {
    let media = c.call("ingest", json!({ "path": source.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let made = c.call(
        "new_timeline",
        json!({ "name": "Grade", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(0),
                "clip": { "media": media, "source_range": { "start_time": at(0), "duration": at(12) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    let clip = c.call("timeline", json!({ "id": tl }))["tracks"][0]["items"][0]["id"]
        .as_str()
        .expect("a clip on the track")
        .to_string();
    (tl, clip, media)
}

/// Run a job to completion and return its record.
fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        let jobs = c.call("jobs", json!({}));
        if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
            if matches!(
                job["state"].as_str().unwrap_or(""),
                "succeeded" | "failed" | "cancelled"
            ) {
                return job.clone();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job {id} did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A display and view this project's loaded config actually has.
///
/// Asked of the runtime rather than written down: the bundled config is a
/// released ACES config and its view names are not ours to invent. A test that
/// hard-codes one is testing its own spelling.
fn a_real_view(c: &mut Client) -> (String, String) {
    let colour = c.call("color", json!({}));
    let displays = colour["displays"]
        .as_array()
        .expect("the project has a config");
    for d in displays {
        if d["active"] != json!(true) {
            continue;
        }
        for v in d["views"].as_array().into_iter().flatten() {
            // `Raw` is a pass-through and grades nothing, so it cannot show
            // whether a grade reached the picture.
            if v["active"] == json!(true) && v["name"] != json!("Raw") {
                return (
                    d["name"].as_str().unwrap().to_string(),
                    v["name"].as_str().unwrap().to_string(),
                );
            }
        }
    }
    panic!("the bundled config offers no active display and view: {colour}")
}

fn take(dir: &Path) -> PathBuf {
    let p = dir.join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&p, 160, 90, Rate::F24, 1).unwrap();
    p
}

#[test]
fn a_clip_with_no_grade_has_none_and_looking_does_not_make_one() {
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, clip, _) = a_clip(&mut c, &source);

    // A reactor makes a `color.grade` node for every placed clip and puts the
    // clip's colour metadata on it, so the node is there from the start. What
    // an interface actually needs to know is whether anything is *in* it.
    let g = c.call("grade", json!({ "clip": clip }));
    assert_eq!(g["graded"], false, "nothing would change a picture: {g}");
    assert_eq!(g["layers"].as_array().unwrap().len(), 0);
    assert_eq!(g["cdl"], Value::Null, "and no CDL either");

    let before = c.call("project", json!({}))["revision"].clone();
    assert_eq!(c.call("grade", json!({ "clip": clip }))["graded"], false);
    assert_eq!(
        c.call("project", json!({}))["revision"],
        before,
        "reading a grade changes nothing: it is not a side effect"
    );
}

#[test]
fn a_real_operation_is_written_read_back_and_kept_across_a_reopen() {
    // A genuinely persisted project, not the test harness: `Harness::start_in`
    // builds an in-memory project (`OmniProject::new`) and its `root` is only
    // where jobs and renders land, so it cannot reopen anything from disk.
    // This creates a project on disk, works it through the same `rpc::dispatch`
    // the WebSocket transport calls, saves it, drops it, and opens the files
    // again.
    use cine_pipeline::{OmniProject, ProjectSetup};
    use cine_runtime::rpc::dispatch;

    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let root = dir.path().join("persisted");
    let clip_id;
    {
        let p = OmniProject::create(&root, ProjectSetup::default()).expect("create the project");
        let call = |method: &str, params: Value| -> Value {
            dispatch(&p, method, &params).unwrap_or_else(|e| panic!("{method}: {e}"))
        };
        let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });

        let media = call("ingest", json!({ "path": source.display().to_string() }))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let tl = call(
            "new_timeline",
            json!({ "name": "Grade", "rate_num": 24, "rate_den": 1 }),
        )["id"]
            .as_str()
            .unwrap()
            .to_string();
        let t = call("timeline", json!({ "id": tl }));
        call(
            "place",
            json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(0),
                    "clip": { "media": media,
                              "source_range": { "start_time": at(0), "duration": at(12) } } }),
        );
        let clip = call("timeline", json!({ "id": tl }))["tracks"][0]["items"][0]["id"]
            .as_str()
            .expect("a clip on the track")
            .to_string();
        clip_id = clip.clone();

        let after = call(
            "grade_set",
            json!({ "clip": clip, "ops": [
                { "op": "exposure", "stops": 0.5 },
                { "op": "saturation", "amount": 1.2 },
            ] }),
        );
        assert_eq!(after["graded"], true, "{after}");
        let ops = after["layers"][0]["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 2, "{after}");
        assert_eq!(ops[0]["op"], "exposure");
        assert_eq!(ops[0]["stops"], 0.5);
        assert_eq!(ops[1]["op"], "saturation");
        assert_eq!(ops[1]["amount"], 1.2);
        assert_eq!(after["layers"][0]["name"], "primary");

        p.save().expect("save the project to disk");
    }

    // The files are all that is left. Reopened, through the same protocol.
    assert!(
        root.join("graph.msgpack").is_file() || root.join("graph.json").is_file(),
        "the project was written to disk"
    );
    let reopened = OmniProject::open(&root).expect("open the saved project");
    let g = dispatch(&reopened, "grade", &json!({ "clip": clip_id }))
        .expect("the clip is still in the reopened project");
    assert_eq!(g["graded"], true, "{g}");
    let ops = g["layers"][0]["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 2, "{g}");
    assert_eq!(ops[0]["op"], "exposure");
    assert_eq!(ops[0]["stops"], 0.5, "the exposure survived the reopen");
    assert_eq!(ops[1]["op"], "saturation");
    assert_eq!(ops[1]["amount"], 1.2, "and so did the saturation");
}

#[test]
fn undo_and_redo_move_through_the_grade() {
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, clip, _) = a_clip(&mut c, &source);

    c.call(
        "grade_set",
        json!({ "clip": clip, "ops": [{ "op": "exposure", "stops": 1.0 }] }),
    );
    c.call(
        "grade_set",
        json!({ "clip": clip, "ops": [{ "op": "exposure", "stops": 2.0 }] }),
    );
    let stops = |c: &mut Client| -> Value {
        c.call("grade", json!({ "clip": clip }))["layers"][0]["ops"][0]["stops"].clone()
    };
    assert_eq!(stops(&mut c), 2.0);

    c.call("undo", json!({}));
    assert_eq!(stops(&mut c), 1.0, "undo steps back one grade");
    c.call("undo", json!({}));
    let back = c.call("grade", json!({ "clip": clip }));
    assert!(
        back["layers"].as_array().unwrap().is_empty()
            || back["layers"][0]["ops"].as_array().unwrap().is_empty(),
        "undoing the first grade leaves nothing graded: {back}"
    );

    c.call("redo", json!({}));
    assert_eq!(stops(&mut c), 1.0, "and redo comes forward again");
    c.call("redo", json!({}));
    assert_eq!(stops(&mut c), 2.0);
}

#[test]
fn the_picture_actually_changes_when_the_grade_does() {
    // The whole point of the slice. Before it, the stack was stored and no
    // frame was ever put through it, so a grade could be "set" and change
    // nothing at all.
    //
    // Proved on the **render** here. `/frame` carries a clip's grade too now,
    // and `the_browser_serves_each_clip_its_own_grade` holds that path; this
    // one holds the delivered master, which is the other place a grade has to
    // arrive and the one a facility receives.
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (tl, clip, _) = a_clip(&mut c, &source);
    let (display, view) = a_real_view(&mut c);
    c.call("color_view", json!({ "display": display, "view": view }));

    // One frame through the project's view, read off disk.
    let render_one = |c: &mut Client| -> Vec<u8> {
        let job = c.call(
            "job_submit",
            json!({ "kind": "render.sequence", "timeline": tl, "output": "png8",
                    "look": "project_view", "start_frame": 0, "end_frame": 1 }),
        );
        let done = wait_for(c, job["id"].as_u64().unwrap());
        assert_eq!(done["state"], "succeeded", "{done}");
        let path = std::path::PathBuf::from(done["output"]["path"].as_str().unwrap())
            .join("frame_000000.png");
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };

    let plain = render_one(&mut c);

    c.call(
        "grade_set",
        json!({ "clip": clip, "ops": [{ "op": "exposure", "stops": 2.0 }] }),
    );
    let graded = render_one(&mut c);
    assert_ne!(
        plain, graded,
        "two stops of exposure must change the picture the render delivers"
    );

    // Back to nothing, and the picture comes back exactly.
    c.call("grade_set", json!({ "clip": clip, "ops": [] }));
    let ungraded = render_one(&mut c);
    assert_eq!(
        plain, ungraded,
        "an empty grade is the ungraded picture, exactly"
    );
}

#[test]
fn an_operation_this_build_cannot_set_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, clip, _) = a_clip(&mut c, &source);

    for (ops, expect) in [
        (
            json!([{ "op": "wheels", "lift": [0.0, 0.0, 0.0] }]),
            "wheels",
        ),
        (json!([{ "op": "curves" }]), "curves"),
        (json!([{ "op": "lut3d" }]), "lut3d"),
    ] {
        let refused = c
            .call_result("grade_set", json!({ "clip": clip, "ops": ops }))
            .expect_err("an operation with no interface is refused");
        assert!(refused.contains(expect), "{refused}");
        assert!(
            refused.contains("exposure"),
            "and says what it can do instead: {refused}"
        );
    }

    // Values the engine cannot mean are refused too.
    for (ops, expect) in [
        (json!([{ "op": "saturation", "amount": -1.0 }]), "0 or more"),
        (
            json!([{ "op": "contrast", "amount": 1.0, "pivot": 0.0 }]),
            "greater than zero",
        ),
        (json!([{ "op": "exposure", "stops": "a lot" }]), "finite"),
    ] {
        let refused = c
            .call_result("grade_set", json!({ "clip": clip, "ops": ops }))
            .expect_err("a value the engine cannot mean is refused");
        assert!(refused.contains(expect), "{refused}");
    }

    // And nothing was written by any of the refusals.
    assert_eq!(c.call("grade", json!({ "clip": clip }))["graded"], false);
}

#[test]
fn the_protocol_says_what_it_cannot_do() {
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, clip, _) = a_clip(&mut c, &source);

    let g = c.call("grade", json!({ "clip": clip }));
    let supported: Vec<&str> = g["supported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        supported.contains(&"exposure") && supported.contains(&"printer_lights"),
        "{g}"
    );

    // The engine evaluates more than this carries, and the answer names each
    // one with a reason rather than implying the list is everything.
    let missing = g["unsupported"].as_array().unwrap();
    assert!(!missing.is_empty(), "{g}");
    for m in missing {
        assert!(!m["op"].as_str().unwrap().is_empty(), "{m}");
        assert!(
            m["reason"].as_str().unwrap().len() > 8,
            "each says why, not just that: {m}"
        );
    }
    let names: Vec<&str> = missing.iter().map(|m| m["op"].as_str().unwrap()).collect();
    assert!(names.contains(&"wheels") && names.contains(&"curves") && names.contains(&"mask"));
}

#[test]
fn a_grade_asked_for_on_something_that_is_not_a_clip_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (tl, _, _) = a_clip(&mut c, &source);

    // A sequence is not a clip, and a stale id is not anything.
    for id in [tl.as_str(), "00000000-0000-5000-8000-000000000000"] {
        let refused = c
            .call_result("grade", json!({ "clip": id }))
            .expect_err("only a clip has a grade");
        assert!(refused.contains("not a clip"), "{refused}");
    }
}

#[test]
fn a_render_made_before_a_regrade_is_stale() {
    // A master that says it is current after the grade changed is the worst
    // kind of wrong answer.
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (tl, clip, _) = a_clip(&mut c, &source);
    let (display, view) = a_real_view(&mut c);
    let chosen = c.call("color_view", json!({ "display": display, "view": view }));
    assert!(chosen.get("error").is_none(), "{chosen}");

    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "output": "png8",
                "look": "project_view", "start_frame": 0, "end_frame": 2 }),
    );
    let mut state = String::new();
    for _ in 0..600 {
        let jobs = c.call("jobs", json!({}));
        let j = jobs
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == job["id"])
            .cloned()
            .unwrap_or(Value::Null);
        state = j["state"].as_str().unwrap_or("").to_string();
        if matches!(state.as_str(), "succeeded" | "failed" | "cancelled") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // No escape hatch. A render that did not succeed cannot show anything
    // about staleness, and returning early here would be a silent pass.
    assert_eq!(
        state, "succeeded",
        "the render through the project view must run"
    );
    let renders = c.call("renders", json!({ "timeline": tl }));
    assert_eq!(renders["renders"][0]["state"], "current", "{renders}");

    c.call(
        "grade_set",
        json!({ "clip": clip, "ops": [{ "op": "exposure", "stops": 1.0 }] }),
    );
    let renders = c.call("renders", json!({ "timeline": tl }));
    assert_eq!(
        renders["renders"][0]["state"], "stale",
        "the grade is part of what the render read: {renders}"
    );
}

/// Two clips on one take, and the same helper as the other tests except that it
/// places the media twice. The point of the pair: one file, two grades, and
/// neither may be shown as the other's.
fn two_clips(c: &mut Client, source: &Path) -> (String, String, String, String) {
    let media = c.call("ingest", json!({ "path": source.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let made = c.call(
        "new_timeline",
        json!({ "name": "Grade", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    for start in [0, 12] {
        c.call(
            "place",
            json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(start),
                    "clip": { "media": media,
                              "source_range": { "start_time": at(0), "duration": at(12) } } }),
        );
    }
    c.call("focus", json!({ "timeline": tl }));
    let items = c.call("timeline", json!({ "id": tl }));
    let items = items["tracks"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "two clips on the track: {items:?}");
    (
        tl,
        items[0]["id"].as_str().unwrap().to_string(),
        items[1]["id"].as_str().unwrap().to_string(),
        media,
    )
}

/// One picture from the HTTP endpoint, with the clip it was coloured for.
fn frame(h: &Harness, query: &str) -> (String, Option<String>, Vec<u8>) {
    let (status, headers, body) = h.get(&format!("/frame?{query}"));
    let clip = common::header(&headers, "x-cine-clip").map(str::to_string);
    (status, clip, body)
}

#[test]
fn the_browser_serves_each_clip_its_own_grade() {
    // The defect this closes. `/frame` had no clip context at all, so the
    // browser's record monitor showed a decode with no grade and no view while
    // the render and the native window showed the grade — three surfaces, two
    // answers. It also could not have been fixed by keying on the media: one
    // file backs both clips here, and their grades are different.
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, a, b, media) = two_clips(&mut c, &source);
    let (display, view) = a_real_view(&mut c);
    c.call("color_view", json!({ "display": display, "view": view }));

    // Through the view, ungraded: the file, which is what a source viewer asks
    // for and what both clips look like before anybody grades them.
    let (status, clip, plain) = frame(&h, &format!("media={media}&index=0&view=project"));
    assert!(
        status.contains("200"),
        "{status}: {}",
        String::from_utf8_lossy(&plain)
    );
    assert_eq!(
        clip, None,
        "a media request is nobody's grade, and says so by carrying no clip"
    );

    c.call(
        "grade_set",
        json!({ "clip": a, "ops": [{ "op": "exposure", "stops": 2.0 }] }),
    );
    c.call(
        "grade_set",
        json!({ "clip": b, "ops": [{ "op": "exposure", "stops": -2.0 }] }),
    );

    let (sa, ca, pa) = frame(&h, &format!("media={media}&index=0&view=project&clip={a}"));
    let (sb, cb, pb) = frame(&h, &format!("media={media}&index=0&view=project&clip={b}"));
    assert!(sa.contains("200"), "{sa}: {}", String::from_utf8_lossy(&pa));
    assert!(sb.contains("200"), "{sb}: {}", String::from_utf8_lossy(&pb));
    assert_eq!(
        ca.as_deref(),
        Some(a.as_str()),
        "coloured for the clip asked for"
    );
    assert_eq!(cb.as_deref(), Some(b.as_str()));

    assert_ne!(pa, plain, "two stops up is not the ungraded picture");
    assert_ne!(pb, plain, "two stops down is not the ungraded picture");
    assert_ne!(
        pa, pb,
        "one file, two clips, two grades: the pictures must differ"
    );
    // The media's own picture did not change when a clip was graded, which is
    // the whole reason a media request carries no grade.
    let (_, _, plain_again) = frame(&h, &format!("media={media}&index=0&view=project"));
    assert_eq!(
        plain, plain_again,
        "grading a clip does not change what the file looks like"
    );

    // Cleared, and the picture comes back exactly.
    c.call("grade_set", json!({ "clip": a, "ops": [] }));
    let (_, _, restored) = frame(&h, &format!("media={media}&index=0&view=project&clip={a}"));
    assert_eq!(
        restored, plain,
        "clearing the grade restores the picture, byte for byte"
    );
}

#[test]
fn a_clip_cannot_colour_a_picture_of_media_it_does_not_play() {
    // Applying one clip's grade to another clip's file is exactly the
    // substitution the resolution model exists to prevent, so it is refused
    // rather than quietly ignored — ignoring it would serve an ungraded
    // picture in answer to a request for a graded one.
    let dir = tempfile::tempdir().unwrap();
    let first = take(dir.path());
    // A different take, and it has to be genuinely different: media identity is
    // content-derived, so the same signal written to another name is the same
    // media and there would be no second take to mistake it for.
    let second = dir.path().join("A001C002.mov");
    cine_media::testsignal::bars_and_pops(&second, 128, 72, Rate::F24, 1).unwrap();

    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (_, clip, media) = a_clip(&mut c, &first);
    let other = c.call("ingest", json!({ "path": second.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(other, media, "two takes, not one seen twice");
    let (display, view) = a_real_view(&mut c);
    c.call("color_view", json!({ "display": display, "view": view }));

    let (status, _, body) = frame(
        &h,
        &format!("media={other}&index=0&view=project&clip={clip}"),
    );
    let why = String::from_utf8_lossy(&body);
    assert!(status.contains("422"), "{status}: {why}");
    assert!(why.contains("plays other media"), "{why}");

    // And the clip's own media is served, so the refusal is about the pairing.
    let (ok, _, _) = frame(
        &h,
        &format!("media={media}&index=0&view=project&clip={clip}"),
    );
    assert!(ok.contains("200"), "{ok}");
}

#[test]
fn the_monitor_is_told_whether_a_picture_can_be_graded() {
    // The frontend must not guess. `/frame?view=project` refuses a media with
    // no input colorspace, so a monitor that asked hopefully would show a
    // refusal where a picture belongs; `picture_at` answers it instead.
    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let (tl, clip, _) = a_clip(&mut c, &source);
    let at = json!({ "ticks": 2, "rate_num": 24, "rate_den": 1 });

    let shown = c.call("picture_at", json!({ "timeline": tl, "time": at }));
    assert_eq!(shown["clip"], json!(clip), "{shown}");
    let colour = &shown["colour"];
    assert_eq!(
        colour["available"], true,
        "an ingested take has a colorspace from a file rule: {shown}"
    );
    assert_eq!(colour["why_not"], Value::Null, "{shown}");
    assert_eq!(colour["graded"], false, "nothing graded yet: {shown}");

    // The picture the monitor would fetch on that answer is really served.
    let (status, got, body) = frame(
        &h,
        &format!(
            "media={}&index={}&view=project&clip={clip}",
            shown["asset"].as_str().unwrap(),
            shown["index"].as_u64().unwrap()
        ),
    );
    assert!(
        status.contains("200"),
        "{status}: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(got.as_deref(), Some(clip.as_str()));

    c.call(
        "grade_set",
        json!({ "clip": clip, "ops": [{ "op": "saturation", "amount": 0.0 }] }),
    );
    let shown = c.call("picture_at", json!({ "timeline": tl, "time": at }));
    assert_eq!(
        shown["colour"]["graded"], true,
        "and it says when there is a grade to see: {shown}"
    );
}

#[test]
fn a_primary_that_would_destroy_an_operation_it_cannot_express_is_refused() {
    // `grade_set` replaces the first layer's operations. If that layer holds a
    // curve — put there by a look import, or by a build with an interface for
    // it — writing a primary from this protocol would delete it. The answer is
    // a refusal that names the operation, and `grade` says so in advance so an
    // interface can disable the controls instead of discovering it by failing.
    use cine_color::grade::{GradeLayer, GradeOp, GradeStack};
    use cine_pipeline::{OmniProject, ProjectSetup};
    use cine_runtime::rpc::dispatch;

    let dir = tempfile::tempdir().unwrap();
    let source = take(dir.path());
    let root = dir.path().join("with-a-curve");
    let p = OmniProject::create(&root, ProjectSetup::default()).expect("create the project");
    let call = |method: &str, params: Value| -> Value {
        dispatch(&p, method, &params).unwrap_or_else(|e| panic!("{method}: {e}"))
    };
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    let media = call("ingest", json!({ "path": source.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let tl = call(
        "new_timeline",
        json!({ "name": "Grade", "rate_num": 24, "rate_den": 1 }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = call("timeline", json!({ "id": tl }));
    call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(0),
                "clip": { "media": media,
                          "source_range": { "start_time": at(0), "duration": at(12) } } }),
    );
    let clip = call("timeline", json!({ "id": tl }))["tracks"][0]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let clip_id = ucg_core::EntityId::parse(&clip).unwrap();

    // A curve on the first layer, written with the engine's own API: this is
    // the state a richer build leaves behind, not a contrivance.
    p.engine
        .transact("a look with a curve", |tx| {
            let grade = cine_color::ucg::ensure_grade_for_clip(tx, clip_id)
                .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))?;
            let stack = GradeStack {
                layers: vec![GradeLayer {
                    name: "look".into(),
                    ops: vec![GradeOp::Curves(cine_color::curve::Curves::default())],
                    ..Default::default()
                }],
                ..Default::default()
            };
            cine_color::ucg::store_grade(tx, grade, &stack)
                .map_err(|e| ucg_core::UcgError::Aborted(e.to_string()))
        })
        .expect("store a stack with a curve on it");

    // Said in advance...
    let g = dispatch(&p, "grade", &json!({ "clip": clip })).unwrap();
    assert_eq!(g["primary_settable"], false, "{g}");
    let why = g["primary_why_not"].as_str().unwrap_or("");
    assert!(why.contains("curves"), "and names the operation: {why}");
    assert_eq!(g["layers"][0]["ops"][0]["op"], "curves");
    assert_eq!(
        g["layers"][0]["ops"][0]["editable"], false,
        "listed, and marked as something this build cannot set: {g}"
    );

    // ...and enforced.
    let refused = dispatch(
        &p,
        "grade_set",
        &json!({ "clip": clip, "ops": [{ "op": "exposure", "stops": 1.0 }] }),
    )
    .expect_err("writing a primary over a curve is refused");
    assert!(refused.contains("curves"), "{refused}");

    // Nothing was written: the curve is still the only thing on the layer.
    let after = dispatch(&p, "grade", &json!({ "clip": clip })).unwrap();
    assert_eq!(after["layers"][0]["ops"].as_array().unwrap().len(), 1);
    assert_eq!(after["layers"][0]["ops"][0]["op"], "curves");
}
