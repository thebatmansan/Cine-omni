//! Subject and lens: what the user is working on, and what the project
//! connects to it.
//!
//! The architecture these hold: the subject is a node of the project graph and
//! the relationships around it are that graph's own edges. Nothing about a
//! scene having shots is written in an interface — it is asked. And the
//! subject survives a change of lens, because the lens never decides it.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn subject(c: &mut Client, id: &str) -> Value {
    c.call("subject", json!({ "id": id }))
}

/// Every entry of every group, flattened.
fn entries(v: &Value) -> Vec<Value> {
    v["related"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["entries"].as_array().unwrap().clone())
        .collect()
}

fn group_names(v: &Value) -> Vec<String> {
    v["related"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["group"].as_str().unwrap().to_string())
        .collect()
}

/// The first scene of the harness's screenplay.
fn a_scene(c: &mut Client) -> String {
    c.call("scenes", json!({}))
        .as_array()
        .unwrap()
        .first()
        .expect("the harness imports a screenplay")["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_subject_says_what_it_is_and_what_the_graph_connects_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let scene = a_scene(&mut c);

    let v = subject(&mut c, &scene);
    assert_eq!(v["subject"]["kind"], "script.scene", "{}", v["subject"]);
    assert_eq!(v["subject"]["group"], "Story");
    assert_eq!(v["subject"]["lens"], "script", "the lens that opens it");
    assert!(
        v["subject"]["sub"].as_str().unwrap().contains("INT.")
            || v["subject"]["sub"].as_str().unwrap().contains("EXT."),
        "a scene is known by its heading: {}",
        v["subject"]
    );

    // Its neighbours are the graph's edges, not a list this test was given.
    let found = entries(&v);
    assert!(!found.is_empty(), "a scene in a screenplay has neighbours");
    for e in &found {
        assert!(e["id"].as_str().is_some(), "{e}");
        assert!(!e["kind"].as_str().unwrap().is_empty(), "{e}");
        assert!(
            !e["kind_label"].as_str().unwrap().is_empty(),
            "every kind reads as words: {e}"
        );
    }
    // The script lines of the scene are among them, under Story.
    assert!(
        found
            .iter()
            .any(|e| e["kind"].as_str().unwrap().starts_with("script.") && e["group"] == "Story"),
        "{found:?}"
    );
}

#[test]
fn the_path_locates_the_subject_without_the_user_reading_the_graph() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let scene = a_scene(&mut c);
    let v = subject(&mut c, &scene);

    let path = v["path"].as_array().unwrap();
    assert!(!path.is_empty(), "a scene sits inside a screenplay: {v}");
    // Outermost first, and the subject itself is never in its own path.
    assert!(
        path.iter().all(|p| p["id"] != json!(scene)),
        "the subject is not its own ancestor: {path:?}"
    );
    assert!(
        path.iter()
            .any(|p| p["kind"].as_str().unwrap().starts_with("script.")),
        "{path:?}"
    );
}

#[test]
fn groups_are_the_order_a_film_is_made_and_a_kind_is_never_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, ucg_core::Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = c.call("ingest", json!({ "path": source.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    let v = subject(&mut c, &media);
    assert_eq!(v["subject"]["group"], "Production", "{}", v["subject"]);
    assert_eq!(v["subject"]["lens"], "media");

    // Media contains its own streams, which are Production things too.
    let names = group_names(&v);
    let order = [
        "Project",
        "Story",
        "Visual",
        "Production",
        "Post",
        "Delivery",
        "Other",
    ];
    let positions: Vec<usize> = names
        .iter()
        .map(|n| order.iter().position(|o| o == n).unwrap_or(order.len()))
        .collect();
    let mut sorted = positions.clone();
    sorted.sort();
    assert_eq!(
        positions, sorted,
        "groups come in the order a film is made: {names:?}"
    );
}

#[test]
fn a_subject_with_no_relationship_of_a_kind_says_so_rather_than_inventing_one() {
    // The honesty requirement: an empty group is absent, never filled with a
    // placeholder record. A media nobody has cut with has no Post neighbours.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, ucg_core::Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = c.call("ingest", json!({ "path": source.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    let v = subject(&mut c, &media);
    let names = group_names(&v);
    assert!(
        !names.contains(&"Post".to_string()),
        "nothing has been cut with it, so there is no Post group at all: {names:?}"
    );
    assert!(
        entries(&v).iter().all(|e| e["id"].as_str().is_some()),
        "every entry is a real node: {:?}",
        entries(&v)
    );
}

#[test]
fn a_subject_that_is_not_in_the_project_is_refused_not_invented() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let refused = c
        .call_result(
            "subject",
            json!({ "id": "00000000-0000-5000-8000-000000000000" }),
        )
        .expect_err("a stale subject is refused");
    assert!(refused.contains("no such thing"), "{refused}");
}

#[test]
fn the_subject_is_the_same_answer_whichever_lens_or_client_asks() {
    // The subject is a fact about the project, not about a window. Two
    // clients asking about one scene get one answer — so a lens change, which
    // is a client-side matter, cannot alter what the subject is.
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut one = h.client();
    let mut two = h.client();
    let scene = a_scene(&mut one);

    let from_one = subject(&mut one, &scene);
    let from_two = subject(&mut two, &scene);
    assert_eq!(from_one, from_two, "two clients, one project, one answer");

    // A lens is never sent, so nothing about the answer can depend on it.
    let again = one.call("subject", json!({ "id": scene, "lens": "colour" }));
    assert_eq!(
        again, from_one,
        "the runtime does not know which craft's view is open, and must not"
    );
}

#[test]
fn a_subject_survives_the_project_being_reopened() {
    // Navigation is not project data, but the *subject* is a node id, and a
    // node id has to still mean the same thing after a reload or the
    // interface would be restoring a pointer into nothing.
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path());
    let scene_id;
    let before;
    {
        let h = Harness::start_in(root.clone());
        let mut c = h.client();
        scene_id = a_scene(&mut c);
        before = subject(&mut c, &scene_id);
        c.call("save", json!({}));
    }
    let h = Harness::start_in(root);
    let mut c = h.client();
    let after = subject(&mut c, &scene_id);
    assert_eq!(
        after["subject"]["id"], before["subject"]["id"],
        "the same id names the same scene after a reopen"
    );
    assert_eq!(after["subject"]["kind"], before["subject"]["kind"]);
}

#[test]
fn walking_an_edge_is_how_a_distant_relationship_is_reached() {
    // One hop is deliberate: a scene's takes are three edges away, and
    // reaching them by stepping is what makes this navigation rather than a
    // dump of the project. Each step must land on a real subject.
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let scene = a_scene(&mut c);

    let here = subject(&mut c, &scene);
    let first = entries(&here)
        .into_iter()
        .find(|e| e["kind"].as_str().unwrap().starts_with("script."))
        .expect("a scene has script neighbours");
    let next = subject(&mut c, first["id"].as_str().unwrap());
    assert_eq!(
        next["subject"]["id"], first["id"],
        "the step landed: {next}"
    );
    // And the way back is in its own path or neighbours — the edge is real
    // in both directions, which is what makes stepping reversible.
    let back = entries(&next)
        .iter()
        .chain(next["path"].as_array().unwrap().iter())
        .any(|e| e["id"] == json!(scene));
    assert!(
        back,
        "the scene is reachable from where the step landed: {next}"
    );
}
