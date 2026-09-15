//! Writing the screenplay through the protocol: whole-document rewrites keep
//! scene identity where the script says they should, reach production, and
//! refuse to cut production work loose without being told to.

mod common;

use common::Harness;
use serde_json::{json, Value};

fn document(c: &mut common::Client) -> String {
    c.call("script", json!({}))["documents"][0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn scenes(c: &mut common::Client) -> Vec<Value> {
    c.call("script", json!({}))["documents"][0]["scenes"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_new_scene_written_in_fountain_is_broken_down_and_scheduled() {
    let h = Harness::start();
    let mut c = h.client();
    let doc = document(&mut c);
    let text = c.call("script_text", json!({ "document": doc }));
    let text = text.as_str().unwrap();
    assert!(text.contains("HARBOUR OFFICE"), "{text}");

    let written = format!(
        "{text}\n\nINT. LIGHTHOUSE - NIGHT #3#\n\nJONAS climbs.\n\nJONAS\nThe lamp is out.\n"
    );
    let r = c.call("script_write", json!({ "document": doc, "text": written }));
    assert_eq!(r["refused"], false);
    let departments: Vec<&str> = r["impact"]["departments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["department"].as_str().unwrap())
        .collect();
    assert!(
        departments.contains(&"Script") && departments.contains(&"Production"),
        "{departments:?}"
    );
    assert_eq!(scenes(&mut c).len(), 3);
    let production = c.call("production", json!({}));
    assert!(production["breakdowns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["heading"].as_str().unwrap().contains("LIGHTHOUSE")));

    // Writing the same text again changes nothing.
    let revision = c.call("project", json!({}))["revision"].clone();
    c.call("script_write", json!({ "document": doc, "text": written }));
    assert_eq!(c.call("project", json!({}))["revision"], revision);
}

#[test]
fn renaming_a_numbered_scene_keeps_it_and_its_cut_material() {
    let h = Harness::start();
    let mut c = h.client();
    let doc = document(&mut c);
    let before = scenes(&mut c);
    let scene = before[0]["id"].as_str().unwrap().to_string();
    let tl = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let clip = c.call("timeline", json!({ "id": tl }))["tracks"][0]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    c.call("bind_clip", json!({ "clip": clip, "scene": scene }));

    let text = c.call("script_text", json!({ "document": doc }));
    let renamed = text.as_str().unwrap().replace(
        "INT. HARBOUR OFFICE - NIGHT",
        "INT. HARBOUR MASTER'S OFFICE - NIGHT",
    );
    let r = c.call("script_write", json!({ "document": doc, "text": renamed }));
    assert_eq!(r["refused"], false, "{r}");
    let after = scenes(&mut c);
    assert_eq!(after[0]["id"], scene, "scene #1# is still scene #1#");
    assert!(after[0]["heading"].as_str().unwrap().contains("MASTER"));
    assert_eq!(
        after[0]["clips"][0]["id"], clip,
        "the binding survived the rename"
    );
    // And the rename reached the cut as a script change.
    assert!(r["impact"]["conform"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["clip"] == clip));
}

#[test]
fn removing_a_scene_that_holds_cut_material_is_refused_until_confirmed() {
    let h = Harness::start();
    let mut c = h.client();
    let doc = document(&mut c);
    let scene2 = scenes(&mut c)[1]["id"].as_str().unwrap().to_string();
    let tl = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let clip = c.call("timeline", json!({ "id": tl }))["tracks"][0]["items"][2]["id"]
        .as_str()
        .unwrap()
        .to_string();
    c.call("bind_clip", json!({ "clip": clip, "scene": scene2 }));

    let text = c.call("script_text", json!({ "document": doc }));
    let text = text.as_str().unwrap();
    let cut = &text[..text.find("EXT. QUAY").expect("scene 2 in the text")];
    let revision = c.call("project", json!({}))["revision"].clone();

    let refused = c.call("script_write", json!({ "document": doc, "text": cut }));
    assert_eq!(refused["refused"], true);
    assert_eq!(refused["orphans"][0]["scene"], scene2);
    assert_eq!(refused["orphans"][0]["clips"], 1);
    assert_eq!(
        c.call("project", json!({}))["revision"],
        revision,
        "nothing was written"
    );
    assert_eq!(scenes(&mut c).len(), 2);

    let applied = c.call(
        "script_write",
        json!({ "document": doc, "text": cut, "allow_orphans": true }),
    );
    assert_eq!(applied["refused"], false);
    assert_eq!(scenes(&mut c).len(), 1);
    let t = c.call("timeline", json!({ "id": tl }));
    let still = t["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == clip)
        .unwrap()
        .clone();
    assert!(
        still["scenes"].as_array().unwrap().is_empty(),
        "the clip remains, unbound"
    );
    c.call("undo", json!({}));
    assert_eq!(scenes(&mut c).len(), 2, "undo brings the scene back");
}
