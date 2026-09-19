//! What the project says about a media, and whether every surface says it.
//!
//! The defect these exist for: `online` is a tri-state pretending to be a
//! boolean — set true at import, set false by a check, and **absent** for a
//! media that was never probed, which is what a clip imported from an OTIO or
//! an EDL is. Every surface tested `online !== false`, so *never established*
//! read as *online*, and the frame server answered "no media with that id in
//! this project" for a media that was in the project.
//!
//! These go through the real protocol and the real HTTP endpoint the interface
//! uses, not through a helper, because the defect was in what those answered.

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

fn import(c: &mut Client, path: &Path) -> String {
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.ingest", "path": path.display().to_string(),
                "thumbnail": false, "waveform": false }),
    );
    let done = wait_for(c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    done["output"]["id"].as_str().unwrap().to_string()
}

fn state_of(c: &mut Client, media: &str) -> Value {
    c.call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(media))
        .unwrap_or_else(|| panic!("{media} is not in the media list"))["state"]
        .clone()
}

/// A take imported from an OTIO document: a media node with a recorded
/// location and nothing else, exactly as a conformed cut arrives.
fn from_a_cut(c: &mut Client, name: &str, target_url: &str) -> String {
    let doc = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "tracks": { "OTIO_SCHEMA": "Stack.1", "name": "tracks", "children": [{
            "OTIO_SCHEMA": "Track.1", "name": "V1", "kind": "Video", "children": [{
                "OTIO_SCHEMA": "Clip.1", "name": name,
                "source_range": { "OTIO_SCHEMA": "TimeRange.1",
                    "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 0.0 },
                    "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 24.0 } },
                "media_reference": { "OTIO_SCHEMA": "ExternalReference.1",
                    "target_url": target_url, "available_range": null }
            }]
        }]}
    });
    let done = c.call(
        "import_otio",
        json!({ "text": doc.to_string(), "key": format!("{name}.otio") }),
    );
    assert!(done.get("error").is_none(), "{done}");
    // Matched on the location the document recorded, which is exact — a media
    // node's name comes from the file, not from the clip that references it.
    c.call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == json!(target_url))
        .unwrap_or_else(|| panic!("no media at {target_url} after importing the cut"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_media_that_exists_but_was_never_probed_is_not_called_missing() {
    // The exact defect. Before this, `/frame` answered "no media with that id
    // in this project" for a media the project browser was listing.
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = from_a_cut(&mut c, "SLICE_A001", "/cards/day1/SLICE_A001.mov");

    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "never_probed", "{state}");
    assert_eq!(state["readable"], false);
    assert_eq!(state["evidence"], "never", "nobody ever established it");
    assert_eq!(state["at"], Value::Null);
    assert_eq!(state["label"], "Not yet verified");
    assert!(
        state["detail"]
            .as_str()
            .unwrap()
            .contains("never been read"),
        "{state}"
    );
    assert_eq!(state["action"], "check_media", "and the way to resolve it");

    let (status, _, body) = h.get(&format!("/frame?media={media}&index=0"));
    let said = String::from_utf8_lossy(&body).to_string();
    assert!(!status.contains("200"), "{status}");
    assert!(
        !said.contains("no media with that id"),
        "the media IS in this project; saying otherwise is the defect: {said}"
    );
    assert!(said.contains("Not yet verified"), "{said}");
}

#[test]
fn a_media_recorded_somewhere_this_machine_cannot_open_says_which_kind() {
    // `file:///media/…` was read as a relative path called `file:` — so every
    // check answered "not there", the right conclusion for the wrong reason.
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let remote = from_a_cut(
        &mut c,
        "SLICE_REMOTE",
        "https://vault.example.com/SLICE_REMOTE.mov",
    );

    let state = state_of(&mut c, &remote);
    assert_eq!(state["presence"], "not_on_this_machine", "{state}");
    assert_eq!(state["local_path"], Value::Null, "there is no local path");
    assert_eq!(
        state["recorded_at_location"], "https://vault.example.com/SLICE_REMOTE.mov",
        "what the document said is kept"
    );
    assert!(
        state["detail"].as_str().unwrap().contains("https"),
        "{state}"
    );
    assert_eq!(state["action"], "relink");

    // And a `file://` URL naming a real local path is a path, not a nowhere.
    let source = dir.path().join("real.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 1).unwrap();
    let url = format!("file://{}", source.display());
    let local = from_a_cut(&mut c, "SLICE_LOCAL", &url);
    let state = state_of(&mut c, &local);
    assert_eq!(
        state["local_path"],
        json!(source.display().to_string()),
        "a file URL names a path on this machine: {state}"
    );
    assert_eq!(
        state["presence"], "never_probed",
        "it is a real place, and still nobody has read it: {state}"
    );
}

#[test]
fn an_imported_media_is_present_and_says_the_presence_is_from_import() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "present", "{state}");
    assert_eq!(state["readable"], true);
    // M1: the project must not present an import-time record as a fact about
    // now. It says when, and what established it.
    assert_eq!(state["evidence"], "at_import", "{state}");
    assert!(state["at"].as_i64().unwrap() > 1_600_000_000, "{state}");
    assert_eq!(state["detail"], Value::Null, "nothing to explain");
    let (status, _, _) = h.get(&format!("/frame?media={media}&index=0"));
    assert!(status.contains("200"), "{status}");

    // A check promotes the evidence: same belief, better grounds.
    c.call("check_media", json!({}));
    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "present");
    assert_eq!(state["evidence"], "checked", "{state}");
}

#[test]
fn a_media_that_went_away_is_offline_and_checked_not_merely_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // Gone. The project has not noticed yet, and says so honestly: what it
    // holds is an import-time record, not a fact about now.
    std::fs::remove_file(&source).unwrap();
    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "present", "{state}");
    assert_eq!(
        state["evidence"], "at_import",
        "the belief is from import, and the interface can say so rather than \
         calling it current: {state}"
    );

    c.call("check_media", json!({}));
    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "offline", "{state}");
    assert_eq!(state["evidence"], "checked");
    assert_eq!(state["readable"], false);
    assert_eq!(state["action"], "relink");
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=0"));
    assert!(!status.contains("200"), "{status}");
    assert!(
        String::from_utf8_lossy(&body).contains("Offline"),
        "{:?}",
        String::from_utf8_lossy(&body)
    );
}

#[test]
fn a_different_file_at_the_recorded_path_is_a_mismatch_not_an_absence() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // A different take written over it, with the same name.
    std::fs::remove_file(&source).unwrap();
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 2).unwrap();
    c.call("check_media", json!({}));

    let state = state_of(&mut c, &media);
    assert_eq!(state["presence"], "replaced", "{state}");
    assert_eq!(
        state["readable"], false,
        "a stranger's take is never served"
    );
    assert_eq!(state["evidence"], "checked");
    assert_eq!(state["action"], "relink");
    assert!(
        state["detail"]
            .as_str()
            .unwrap()
            .contains("different media"),
        "the note names both contents: {state}"
    );
}

#[test]
fn a_media_id_that_is_not_in_the_project_is_the_only_honest_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let stranger = "00000000-0000-5000-8000-000000000000";
    let (status, _, body) = h.get(&format!("/frame?media={stranger}&index=0"));
    let said = String::from_utf8_lossy(&body).to_string();
    assert!(!status.contains("200"), "{status}");
    assert!(
        said.contains("no media with that id"),
        "this is the one case where that sentence is true: {said}"
    );
}

#[test]
fn every_surface_reads_one_state_for_one_media() {
    // The architecture requirement: viewer, inspector and timeline must not
    // each infer availability. They read the same answer.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    let never = from_a_cut(&mut c, "Z999_C001", "/cards/never/Z999_C001.mov");

    std::fs::remove_file(&source).unwrap();
    c.call("check_media", json!({}));

    for (id, want, readable) in [(&media, "offline", false), (&never, "never_probed", false)] {
        let state = state_of(&mut c, id);
        assert_eq!(state["presence"], want, "{state}");
        assert_eq!(state["readable"], readable);

        // The frame endpoint the viewer uses refuses with the same words the
        // media list shows, so the two cannot contradict each other.
        let (status, _, body) = h.get(&format!("/frame?media={id}&index=0"));
        assert!(!status.contains("200"), "{status}");
        let said = String::from_utf8_lossy(&body).to_string();
        assert!(
            said.starts_with(state["label"].as_str().unwrap()),
            "the viewer's refusal begins with the state the list shows: {said} / {state}"
        );
    }
}
