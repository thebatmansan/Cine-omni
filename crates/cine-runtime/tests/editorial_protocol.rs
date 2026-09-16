//! Editing through the protocol, and seeing what an edit changes.
//!
//! Every edit here is sent by a client over a real socket, committed as one
//! transaction on the shared engine, broadcast to the other clients, and read
//! back from projections. The impact assertions read only what the engine's
//! deltas record: the tests would fail if the impact claimed a consequence the
//! cascade did not produce.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::time::Duration;

fn timeline_id(c: &mut Client) -> String {
    c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn timeline(c: &mut Client) -> Value {
    let id = timeline_id(c);
    c.call("timeline", json!({ "id": id }))
}

fn frames(n: i64) -> Value {
    json!({ "ticks": n, "rate_num": 24, "rate_den": 1 })
}

fn departments(impact: &Value) -> Vec<String> {
    impact["departments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["department"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_ripple_trim_commits_projects_broadcasts_and_undoes() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    let before = timeline(&mut a);
    let v1 = before["tracks"][0]["id"].as_str().unwrap().to_string();
    let item = &before["tracks"][0]["items"][2];
    assert_eq!(item["kind"], "clip");
    assert_eq!(item["at"]["duration"]["ticks"], 120);
    let following_start = before["tracks"][0]["items"][3]["at"]["start"]["ticks"]
        .as_i64()
        .unwrap();

    let r = a.call(
        "edit",
        json!({ "track": v1, "op": { "op": "ripple_trim", "index": 2, "edge": "tail", "delta": frames(-12) } }),
    );
    assert_eq!(r["outcome"]["label"], "ripple trim");
    assert_eq!(r["outcome"]["applied"]["ticks"], -12);
    assert_eq!(r["outcome"]["clamped"], false);
    assert_eq!(departments(&r["impact"]), vec!["Editorial"]);

    assert!(
        b.drain_changes(1, Duration::from_secs(5)) >= 1,
        "not broadcast"
    );
    let after = timeline(&mut b);
    assert_eq!(
        after["tracks"][0]["items"][2]["at"]["duration"]["ticks"],
        108
    );
    assert_eq!(
        after["tracks"][0]["items"][3]["at"]["start"]["ticks"]
            .as_i64()
            .unwrap(),
        following_start - 12,
        "the following item rippled"
    );

    let history = b.call("history", json!({}));
    let last = history["undo"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["label"], "edit: ripple trim");

    b.call("undo", json!({}));
    a.drain_changes(1, Duration::from_secs(5));
    let undone = timeline(&mut a);
    assert_eq!(
        undone["tracks"], before["tracks"],
        "undo from the other client restores the cut exactly"
    );
}

#[test]
fn a_trim_past_the_media_reports_what_was_applied() {
    let h = Harness::start();
    let mut c = h.client();
    let t = timeline(&mut c);
    let v1 = t["tracks"][0]["id"].as_str().unwrap().to_string();
    let tail = t["tracks"][0]["items"][4]["handles"]["tail"]["ticks"].as_i64();
    let Some(tail) = tail else {
        panic!("the fixture's last clip must declare its media, so its handles are known");
    };
    let r = c.call(
        "edit",
        json!({ "track": v1, "op": { "op": "ripple_trim", "index": 4, "edge": "tail", "delta": frames(tail + 500) } }),
    );
    assert_eq!(r["outcome"]["requested"]["ticks"], tail + 500);
    assert_eq!(r["outcome"]["applied"]["ticks"], tail);
    assert_eq!(r["outcome"]["clamped"], true);
}

#[test]
fn linked_picture_and_sound_move_together_as_one_undo_step() {
    let h = Harness::start();
    let mut c = h.client();
    let t = timeline(&mut c);
    let v1 = t["tracks"][0]["id"].as_str().unwrap().to_string();
    let video = t["tracks"][0]["items"][2]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let audio = t["tracks"][2]["items"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    c.call(
        "link",
        json!({ "clips": [video, audio], "group": "A001_C007" }),
    );
    let linked = timeline(&mut c);
    assert_eq!(linked["tracks"][0]["items"][2]["link"], "A001_C007");
    assert_eq!(linked["tracks"][2]["items"][1]["link"], "A001_C007");

    let r = c.call(
        "edit",
        json!({ "track": v1, "op": { "op": "slip", "index": 2, "delta": frames(4) } }),
    );
    assert_eq!(r["outcome"]["linked_clips"], 1);
    assert_eq!(r["outcome"]["changed_tracks"].as_array().unwrap().len(), 2);
    let slipped = timeline(&mut c);
    assert_eq!(
        slipped["tracks"][0]["items"][2]["source_range"]["start"]["ticks"],
        slipped["tracks"][2]["items"][1]["source_range"]["start"]["ticks"],
    );

    c.call("undo", json!({}));
    assert_eq!(
        timeline(&mut c)["tracks"],
        linked["tracks"],
        "one undo restored both"
    );
}

#[test]
fn an_edit_on_a_locked_track_is_refused_and_changes_nothing() {
    let h = Harness::start();
    let mut c = h.client();
    let t = timeline(&mut c);
    let v1 = t["tracks"][0]["id"].as_str().unwrap().to_string();
    c.call(
        "track_state",
        json!({ "track": v1, "key": "locked", "value": true }),
    );
    let revision = c.call("project", json!({}))["revision"].clone();

    let mut raw = h.client();
    raw.send_masked(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "edit",
                 "params": { "track": v1, "op": { "op": "split_at", "time": frames(10) } } })
        .to_string(),
    );
    let reply = raw.read_reply(7);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("locked"),
        "{reply}"
    );
    assert_eq!(c.call("project", json!({}))["revision"], revision);
}

#[test]
fn a_malformed_edit_is_refused_with_the_reason() {
    let h = Harness::start();
    let mut c = h.client();
    let v1 = timeline(&mut c)["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut raw = h.client();
    for (id, op) in [
        (1, json!({ "op": "teleport" })),
        (
            2,
            json!({ "op": "slip", "index": 0, "delta": { "ticks": 1, "rate_num": 24, "rate_den": 0 } }),
        ),
    ] {
        raw.send_masked(
            &json!({ "jsonrpc": "2.0", "id": id, "method": "edit", "params": { "track": v1, "op": op } })
                .to_string(),
        );
        let reply = raw.read_reply(id);
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid edit op"),
            "{reply}"
        );
    }
}

#[test]
fn insert_uses_the_projects_own_media() {
    let h = Harness::start();
    let mut c = h.client();
    let t = timeline(&mut c);
    let v2 = t["tracks"][1]["id"].as_str().unwrap().to_string();
    let source = &t["tracks"][0]["items"][0];
    let asset = source["asset"]
        .as_str()
        .expect("the clip names its asset")
        .to_string();
    let assets_before = c.call("media", json!({})).as_array().unwrap().len();

    let r = c.call(
        "edit",
        json!({ "track": v2, "op": { "op": "insert", "time": frames(0), "clip": {
            "media": asset,
            "source_range": { "start_time": frames(30), "duration": frames(24) },
        } } }),
    );
    let new_clip = r["outcome"]["item"].as_str().unwrap().to_string();
    let after = timeline(&mut c);
    let placed = &after["tracks"][1]["items"][0];
    assert_eq!(placed["id"], new_clip);
    assert_eq!(placed["asset"], asset);
    assert_eq!(placed["at"]["duration"]["ticks"], 24);
    assert_eq!(
        after["tracks"][1]["items"][1]["at"]["start"]["ticks"], 24,
        "ripple insert pushed V2"
    );
    assert_eq!(
        c.call("media", json!({})).as_array().unwrap().len(),
        assets_before
    );
    assert!(
        r["impact"]["departments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["department"] == "Editorial")
            .flat_map(|d| d["entities"].as_array().unwrap())
            .any(|e| e["id"] == new_clip && e["change"] == "created"),
        "the impact names the clip it created: {}",
        r["impact"]
    );
}

/// The thesis, end to end: bind cut material to a scene, change the words of
/// that scene, and the cut says which material needs conforming, and why.
#[test]
fn a_script_change_flags_only_the_material_bound_to_that_scene() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    let script = a.call("script", json!({}));
    let scene = &script["documents"][0]["scenes"][0];
    let scene_id = scene["id"].as_str().unwrap().to_string();
    let line = scene["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "dialogue")
        .expect("scene 1 has a line")
        .clone();
    assert_eq!(line["text"], "We ride at dawn.");

    let t = timeline(&mut a);
    let bound = t["tracks"][0]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let unbound = t["tracks"][0]["items"][2]["id"]
        .as_str()
        .unwrap()
        .to_string();
    a.call("bind_clip", json!({ "clip": bound, "scene": scene_id }));
    assert_eq!(
        a.call("script", json!({}))["documents"][0]["scenes"][0]["clips"][0]["id"],
        bound
    );

    // Same number of words: a rewording must still reach the cut.
    let r = a.call(
        "edit_dialogue",
        json!({ "line": line["id"], "text": "We sail at noon." }),
    );
    let conform = r["impact"]["conform"].as_array().unwrap();
    assert_eq!(conform.len(), 1, "exactly the bound clip: {}", r["impact"]);
    assert_eq!(conform[0]["clip"], bound);
    assert!(conform[0]["reason"]
        .as_str()
        .unwrap()
        .starts_with("script changed: "));
    let departments = departments(&r["impact"]);
    assert!(departments.contains(&"Script".to_string()));
    assert!(departments.contains(&"Editorial".to_string()));

    // The other window sees the flag on the timeline.
    b.drain_changes(1, Duration::from_secs(5));
    let seen = timeline(&mut b);
    let flagged: Vec<&Value> = seen["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["items"].as_array().unwrap())
        .filter(|i| i["needs_conform"] == true)
        .collect();
    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0]["id"], bound);
    assert!(seen["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == unbound && i["needs_conform"] == false));

    // Any client can ask for the impact of the latest change by revision.
    let latest = b.call("project", json!({}))["revision"].as_u64().unwrap();
    let impact = b.call("impact", json!({ "revision": latest }));
    assert_eq!(impact["label"], "edit dialogue");

    // Conforming clears the flag; undo brings it back.
    a.call("conform_done", json!({ "clip": bound }));
    assert_eq!(
        timeline(&mut a)["tracks"][0]["items"][0]["needs_conform"],
        false
    );
    let undo = a.call("undo", json!({}));
    assert_eq!(undo["impact"]["label"], "mark conformed");
    assert_eq!(
        timeline(&mut a)["tracks"][0]["items"][0]["needs_conform"],
        true
    );
}

/// A cut made of real media, from nothing: a new sequence, a real file
/// ingested, its picture and sound placed as a linked pair, and the record
/// monitor resolving — in the runtime — which decoded picture the cut shows.
#[test]
fn a_real_cut_is_built_from_ingested_media_and_its_record_picture_decodes() {
    let Some(path) = common::media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let h = Harness::start();
    let mut c = h.client();
    let made = c.call(
        "new_timeline",
        json!({ "name": "Assembly", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let seq = c.call("timeline", json!({ "id": tl }));
    assert_eq!(seq["name"], "Assembly");
    let v1 = seq["tracks"][0]["id"].as_str().unwrap().to_string();
    let a1 = seq["tracks"][1]["id"].as_str().unwrap().to_string();

    let asset = c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let media = c.call("media", json!({}));
    let info = media
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == asset)
        .unwrap()
        .clone();
    let (num, den) = (
        info["video"]["rate_num"].as_u64().unwrap(),
        info["video"]["rate_den"].as_u64().unwrap(),
    );
    let at = |ticks: i64| json!({ "ticks": ticks, "rate_num": num, "rate_den": den });

    // Pictures 30..54 of the file, at the head of the sequence, on picture and sound.
    let placed = c.call(
        "place",
        json!({ "tracks": [v1, a1], "time": { "ticks": 0, "rate_num": 24, "rate_den": 1 },
                "clip": { "media": asset, "source_range": { "start_time": at(30), "duration": at(24) } } }),
    );
    let clips = placed["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    let seq = c.call("timeline", json!({ "id": tl }));
    let v = &seq["tracks"][0]["items"][0];
    let a = &seq["tracks"][1]["items"][0];
    assert_eq!(v["asset"], asset);
    assert_eq!(a["asset"], asset);
    assert!(
        v["link"].is_string() && v["link"] == a["link"],
        "picture and sound are linked"
    );

    // Ten frames into the cut is picture 40 of the file.
    let shown = c.call(
        "picture_at",
        json!({ "timeline": tl, "time": { "ticks": 10, "rate_num": 24, "rate_den": 1 } }),
    );
    assert_eq!(shown["black"], false);
    assert_eq!(shown["asset"], asset);
    if (num, den) == (24, 1) {
        assert_eq!(shown["index"], 40);
    }
    let index = shown["index"]
        .as_i64()
        .expect("an ingested asset declares its range");
    let (status, headers, body) = h.get(&format!("/frame?media={asset}&index={index}"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(
        common::header(&headers, "x-cine-index"),
        Some(index.to_string().as_str())
    );
    assert!(body.starts_with(b"\x89PNG"));

    // Past the end of the cut there is nothing to show, and it says so.
    let after = c.call(
        "picture_at",
        json!({ "timeline": tl, "time": { "ticks": 500, "rate_num": 24, "rate_den": 1 } }),
    );
    assert_eq!(after["black"], true);
}

/// Opening a sequence in one surface opens it in every surface: the session is
/// shared, announced, validated, and not project state.
#[test]
fn the_open_sequence_is_shared_across_clients() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    assert_eq!(a.call("focus", json!({}))["timeline"], Value::Null);
    let made = a.call("new_timeline", json!({ "name": "Second cut" }));
    let id = made["id"].as_str().unwrap().to_string();
    let revision = a.call("project", json!({}))["revision"].clone();

    a.call("focus", json!({ "timeline": id }));
    assert_eq!(b.call("focus", json!({}))["timeline"], id);
    assert_eq!(
        a.call("project", json!({}))["revision"],
        revision,
        "opening a sequence is not a commit"
    );

    let mut raw = h.client();
    raw.send_masked(
        &json!({ "jsonrpc": "2.0", "id": 5, "method": "focus",
                 "params": { "timeline": h.runtime.project.root.to_string() } })
        .to_string(),
    );
    let reply = raw.read_reply(5);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a sequence"),
        "{reply}"
    );
    assert_eq!(
        b.call("focus", json!({}))["timeline"],
        id,
        "a refused focus changes nothing"
    );
}

/// The cut leaves the application as an EDL, and says what the format could
/// not carry rather than letting the receiver discover it.
#[test]
fn the_cut_exports_as_an_edl_that_says_what_it_left_behind() {
    let h = Harness::start();
    let mut c = h.client();
    let text = c.call("export", json!({ "what": "edl", "scene": 0 }));
    let text = text.as_str().unwrap_or_default().to_string();
    assert!(text.starts_with("TITLE: "), "{text}");
    assert!(text.contains("FCM: NON-DROP FRAME"), "{text}");

    // Every event has the four timecodes in the columns the format fixes.
    let events: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert!(
        !events.is_empty(),
        "the imported timeline has cuts:\n{text}"
    );
    for event in &events {
        let fields: Vec<&str> = event.split_whitespace().collect();
        assert!(fields.len() >= 8, "{event}");
        assert_eq!(fields[3], "C", "cuts only: {event}");
        for tc in &fields[4..8] {
            assert!(
                tc.len() == 11 && tc.chars().filter(|c| *c == ':').count() == 3,
                "{tc} is not a timecode in {event}"
            );
        }
    }
    // Record times run forwards and never overlap. They are not always
    // continuous: a gap on the track is black, which advances the record clock
    // without an event, and an EDL says so by leaving the hole.
    let frames = |tc: &str| -> i64 {
        let p: Vec<i64> = tc.split(':').map(|v| v.parse().unwrap_or(0)).collect();
        ((p[0] * 60 + p[1]) * 60 + p[2]) * 24 + p[3]
    };
    let field = |line: &str, n: usize| {
        line.split_whitespace()
            .nth(n)
            .unwrap_or_default()
            .to_string()
    };
    for pair in events.windows(2) {
        let (out, next_in) = (frames(&field(pair[0], 7)), frames(&field(pair[1], 6)));
        assert!(next_in >= out, "events overlap in record time: {:?}", pair);
    }
    // Each event's record duration is its source duration: an EDL cut does not
    // retime.
    for event in &events {
        let source = frames(&field(event, 5)) - frames(&field(event, 4));
        let record = frames(&field(event, 7)) - frames(&field(event, 6));
        assert_eq!(source, record, "{event}");
    }
    // And every event names its clip.
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("* FROM CLIP NAME:"))
            .count(),
        events.len()
    );
}
