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

/// Revisions: the pages that went out, and the marks that say what has moved
/// since — which is the only way a crew can tell whether the page in their
/// hand is the one being shot.
#[test]
fn issuing_a_revision_marks_what_changes_after_it() {
    let h = Harness::start();
    let mut c = h.client();
    let script = c.call("script", json!({}));
    let document = script["documents"][0]["id"].as_str().unwrap().to_string();
    assert!(
        script["documents"][0]["current_revision"].is_null(),
        "nothing has gone out yet"
    );
    assert_eq!(script["documents"][0]["next_colour"], json!("White"));

    // The first pages go out.
    let issued = c.call(
        "issue_revision",
        json!({ "document": document, "date": "2026-01-12" }),
    );
    assert_eq!(issued["revision"]["colour"], json!("White"));
    assert!(
        issued["revision"]["elements"].as_u64().unwrap() > 0,
        "it snapshotted the script"
    );
    let after = c.call("script", json!({}));
    assert_eq!(
        after["documents"][0]["current_revision"],
        json!("White — 2026-01-12")
    );
    assert_eq!(
        after["documents"][0]["marked"],
        json!(0),
        "nothing has moved yet"
    );
    assert_eq!(after["documents"][0]["next_colour"], json!("Blue"));

    // A line is rewritten: it carries a mark, and nothing else does.
    let line = after["documents"][0]["scenes"][0]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "dialogue")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let was = after["documents"][0]["scenes"][0]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(line))
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    c.call(
        "edit_dialogue",
        json!({ "line": line, "text": "We go at first light, and not before." }),
    );
    let marked = c.call("script", json!({}));
    let elements = marked["documents"][0]["scenes"][0]["elements"]
        .as_array()
        .unwrap();
    let mine = elements.iter().find(|e| e["id"] == json!(line)).unwrap();
    assert_eq!(
        mine["revised"],
        json!("changed"),
        "the rewritten line is starred"
    );
    assert_eq!(
        elements.iter().filter(|e| !e["revised"].is_null()).count(),
        1,
        "and only that line"
    );
    assert_eq!(marked["documents"][0]["marked"], json!(1));

    // Put it back: the mark goes, because the page is the page that went out.
    c.call("edit_dialogue", json!({ "line": line, "text": was }));
    let restored = c.call("script", json!({}));
    assert_eq!(
        restored["documents"][0]["marked"],
        json!(0),
        "an undone change is not a change"
    );

    // The next revision is the next colour, and starts its own marks.
    c.call(
        "edit_dialogue",
        json!({ "line": line, "text": "Not before first light." }),
    );
    let blue = c.call(
        "issue_revision",
        json!({ "document": document, "date": "2026-01-19" }),
    );
    assert_eq!(blue["revision"]["colour"], json!("Blue"));
    let fresh = c.call("script", json!({}));
    assert_eq!(
        fresh["documents"][0]["marked"],
        json!(0),
        "the blue pages are the new baseline"
    );
    assert_eq!(
        fresh["documents"][0]["revisions"].as_array().unwrap().len(),
        2
    );
    assert_eq!(fresh["documents"][0]["next_colour"], json!("Pink"));
}

/// A caption linked to the line it transcribes, over the protocol and in both
/// directions: the caption knows its line, the line knows its caption, and
/// rewriting the line in the script flags the caption with the old and new
/// words until somebody accepts the rewrite.
#[test]
fn a_rewritten_line_flags_the_caption_that_transcribes_it() {
    let h = Harness::start();
    let mut c = h.client();
    let doc = document(&mut c);
    let scene1 = scenes(&mut c)[0]["id"].as_str().unwrap().to_string();

    // Bind the first picture clip of the cut to scene 1.
    let tl = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let seq = c.call("timeline", json!({ "id": tl }));
    let clip = seq["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["kind"] == "video")
        .flat_map(|t| t["items"].as_array().unwrap().clone())
        .find(|i| i["kind"] == "clip")
        .expect("a clip");
    let (clip_id, start, end) = (
        clip["id"].as_str().unwrap().to_string(),
        clip["at"]["start"].clone(),
        clip["at"]["end"].clone(),
    );
    c.call("bind_clip", json!({ "clip": clip_id, "scene": scene1 }));

    // A caption over it reading exactly MAREN's line, linked by the exact pass.
    c.call(
        "set_caption",
        json!({ "timeline": tl, "language": "en", "start": start, "end": end,
                "text": "We ride at dawn." }),
    );
    let r = c.call(
        "link_captions_exact",
        json!({ "timeline": tl, "language": "en" }),
    );
    assert_eq!(r["linked"], 1);
    let caption = r["track"]["captions"][0].clone();
    let key = caption["key"].as_str().unwrap().to_string();
    assert_eq!(caption["link"]["character"], "MAREN");
    assert_eq!(caption["link"]["transcribed"], "We ride at dawn.");
    assert!(caption["review"].is_null());

    // The other direction: the line lists its caption.
    let line_captions = |c: &mut common::Client| -> Value {
        scenes(c)[0]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["character"] == "MAREN" && e["kind"] == "dialogue")
            .unwrap()["captions"]
            .clone()
    };
    assert_eq!(line_captions(&mut c).as_array().unwrap().len(), 1);

    // Candidates are the dialogue of the scene under the caption.
    let cands = c.call(
        "caption_candidates",
        json!({ "timeline": tl, "language": "en", "key": key }),
    );
    assert_eq!(cands["scenes"], json!([scene1]));
    assert_eq!(cands["candidates"][0]["line"]["text"], "We ride at dawn.");
    assert_eq!(cands["candidates"][0]["score"], 1.0);

    // Rewrite the line through the script editor's own path.
    let text = c.call("script_text", json!({ "document": doc }));
    let rewritten = text
        .as_str()
        .unwrap()
        .replace("We ride at dawn.", "We ride at first light.");
    c.call(
        "script_write",
        json!({ "document": doc, "text": rewritten }),
    );

    let now = c.call("captions", json!({ "timeline": tl, "language": "en" }));
    let flagged = &now["track"]["captions"][0];
    let why = flagged["review"].as_str().expect("flagged");
    assert!(
        why.contains("\"We ride at dawn.\" is now \"We ride at first light.\""),
        "{why}"
    );
    assert_eq!(flagged["link"]["now"], "We ride at first light.");
    assert!(
        line_captions(&mut c)[0]["review"].is_string(),
        "and the script sees the flag"
    );

    // Accept: the new words become the transcription and the flag clears.
    let r = c.call(
        "accept_caption_line",
        json!({ "timeline": tl, "language": "en", "key": key }),
    );
    assert!(r["track"]["captions"][0]["review"].is_null());
    assert_eq!(
        r["track"]["captions"][0]["link"]["transcribed"],
        "We ride at first light."
    );

    // Editing the caption's text keeps the link: it is the same caption.
    let r = c.call(
        "set_caption",
        json!({ "timeline": tl, "language": "en", "index": 0, "start": start, "end": end,
                "text": "We ride at first light." }),
    );
    assert_eq!(r["track"]["captions"][0]["key"], key.as_str());
    assert_eq!(r["track"]["captions"][0]["link"]["character"], "MAREN");

    // Unlink.
    let r = c.call(
        "unlink_caption",
        json!({ "timeline": tl, "language": "en", "key": key }),
    );
    assert!(r["track"]["captions"][0]["link"].is_null());
    assert_eq!(line_captions(&mut c).as_array().unwrap().len(), 0);
}
