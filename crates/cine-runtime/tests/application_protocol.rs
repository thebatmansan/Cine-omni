//! The protocol the application actually speaks, over a real socket.
//!
//! Every assertion here goes through the running server: a real TCP connection,
//! a real RFC 6455 handshake, real JSON-RPC, and for pictures a real HTTP
//! request. Nothing is tested against an in-process shortcut, because the thing
//! that has historically been wrong is the seam.
//!
//! The project these tests run against is built the way a user's would be: a
//! screenplay imported, a cut imported, real media ingested. Nothing is
//! fabricated into the graph.

mod common;

use common::{header, media_path, Harness};
use serde_json::{json, Value};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Project identity and session
// ---------------------------------------------------------------------------

#[test]
fn the_project_reports_its_own_identity_and_session() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("project", json!({}));

    assert!(
        p["revision"].as_u64().unwrap() > 0,
        "a built project has a revision"
    );
    assert!(p["nodes"].as_u64().unwrap() > 10);
    assert!(p["by_kind"]["script.scene"].as_u64().unwrap() >= 2);
    assert!(
        p["by_kind"]["nle.timeline"].as_u64().unwrap() >= 1,
        "the imported cut must be in the graph"
    );
    assert_eq!(p["clients"].as_u64(), Some(1), "this client is attached");
    assert!(p["path"].is_string());
}

// ---------------------------------------------------------------------------
// Capabilities — the honesty surface
// ---------------------------------------------------------------------------

/// Every capability must carry a state the interface can render and a reason a
/// person can read. A capability with no detail is how a false claim ships.
#[test]
fn every_capability_states_what_it_is_and_why() {
    let h = Harness::start();
    let mut c = h.client();
    let caps = c.call("capabilities", json!({}));
    let list = caps["capabilities"].as_array().expect("a list");
    assert!(list.len() > 8, "got {} capabilities", list.len());

    for cap in list {
        let state = cap["state"].as_str().expect("state");
        assert!(
            matches!(state, "verified" | "degraded" | "unavailable" | "refused"),
            "unknown capability state {state:?}"
        );
        let detail = cap["detail"].as_str().unwrap_or("");
        assert!(
            detail.len() > 15,
            "{} is {state} without a usable reason: {detail:?}",
            cap["id"]
        );
        assert!(cap["name"].is_string() && cap["group"].is_string());
    }
}

/// The two claims this build must never make.
#[test]
fn real_time_playback_and_audio_are_reported_unavailable() {
    let h = Harness::start();
    let mut c = h.client();
    let caps = c.call("capabilities", json!({}));
    let list = caps["capabilities"].as_array().unwrap().clone();
    let find = |id: &str| -> Value {
        list.iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no capability {id}"))
            .clone()
    };
    assert_eq!(find("media.playback")["state"], "unavailable");
    assert!(find("media.playback")["detail"]
        .as_str()
        .unwrap()
        .contains("GPU"));
    assert_eq!(find("audio.monitoring")["state"], "unavailable");
    // And the honest thing the browser *can* do is not oversold either.
    assert_eq!(find("media.preview")["state"], "degraded");
    assert!(find("media.preview")["detail"]
        .as_str()
        .unwrap()
        .contains("not real-time"));
}

// ---------------------------------------------------------------------------
// Editorial
// ---------------------------------------------------------------------------

#[test]
fn a_timeline_projects_its_tracks_clips_and_placement() {
    let h = Harness::start();
    let mut c = h.client();
    let timelines = c.call("timelines", json!({}));
    let first = timelines.as_array().expect("a list")[0].clone();
    let id = first["id"].as_str().expect("id");

    let t = c.call("timeline", json!({ "id": id }));
    let tracks = t["tracks"].as_array().expect("tracks");
    assert!(!tracks.is_empty(), "the fixture has tracks");

    let mut saw_clip = false;
    for track in tracks {
        assert!(track["name"].is_string());
        assert!(matches!(
            track["kind"].as_str(),
            Some("video") | Some("audio")
        ));
        for item in track["items"].as_array().unwrap() {
            // Placement is computed by the runtime, exactly, and carried as
            // ticks plus a rate. Seconds are for layout only.
            let at = &item["at"];
            assert!(at["start"]["ticks"].is_i64(), "exact start missing");
            assert!(at["start"]["rate_num"].as_u64().unwrap() > 0);
            assert!(at["duration"]["ticks"].as_i64().unwrap() > 0);
            assert!(at["end"]["seconds"].as_f64().unwrap() > 0.0);
            if item["kind"] == "clip" {
                saw_clip = true;
            }
        }
    }
    assert!(saw_clip, "the fixture has clips");
}

/// Clips and gaps must be laid end to end: each starts where the previous
/// ended. If the runtime got composition wrong, the interface would draw a
/// wrong cut very convincingly.
///
/// **Transitions are excluded, because a transition is not laid end to end.**
/// It sits *on* a cut and reaches back into the outgoing shot and forward into
/// the incoming one — that overlap is what a dissolve is. This test asserted
/// contiguity over everything first and the fixture's dissolve failed it, which
/// is the model telling the test it was wrong.
#[test]
fn clips_and_gaps_are_laid_end_to_end_and_transitions_overlap_the_cut() {
    let h = Harness::start();
    let mut c = h.client();
    let id = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = c.call("timeline", json!({ "id": id }));

    let mut saw_transition = false;
    for track in t["tracks"].as_array().unwrap() {
        let items = track["items"].as_array().unwrap();
        let timed: Vec<&Value> = items.iter().filter(|i| i["kind"] != "transition").collect();
        for pair in timed.windows(2) {
            let end = pair[0]["at"]["end"]["seconds"].as_f64().unwrap();
            let start = pair[1]["at"]["start"]["seconds"].as_f64().unwrap();
            assert!(
                (end - start).abs() < 1e-9,
                "track {} has a hole: {} ends at {end}, {} starts at {start}",
                track["name"],
                pair[0]["name"],
                pair[1]["name"]
            );
        }
        for (i, item) in items.iter().enumerate() {
            if item["kind"] != "transition" {
                continue;
            }
            saw_transition = true;
            let start = item["at"]["start"]["seconds"].as_f64().unwrap();
            let end = item["at"]["end"]["seconds"].as_f64().unwrap();
            assert!(end > start, "a transition must have a duration");
            if let Some(prev) = i.checked_sub(1).and_then(|p| items.get(p)) {
                let prev_end = prev["at"]["end"]["seconds"].as_f64().unwrap();
                assert!(
                    start < prev_end,
                    "a transition must reach back into the outgoing shot"
                );
            }
        }
    }
    assert!(
        saw_transition,
        "the fixture has a dissolve; the projection lost it"
    );
}

/// Track state set through the protocol must land in the graph, come back in
/// the next projection, and be the same state an edit is refused by.
#[test]
fn track_state_commits_through_the_graph_and_comes_back() {
    let h = Harness::start();
    let mut c = h.client();
    let id = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = c.call("timeline", json!({ "id": id }));
    let track = before["tracks"][0]["id"]
        .as_str()
        .expect("track id")
        .to_string();
    assert_eq!(before["tracks"][0]["locked"], json!(false));

    let cascade = c.call(
        "track_state",
        json!({ "track": track, "key": "locked", "value": true }),
    );
    assert!(
        !cascade["deltas"].as_array().unwrap().is_empty(),
        "setting track state must be a real commit"
    );

    let after = c.call("timeline", json!({ "id": id }));
    assert_eq!(after["tracks"][0]["locked"], json!(true));

    // And the engine agrees: this is the state cine-timeline reads.
    let g = h.runtime.project.engine.read();
    let entity = ucg_core::EntityId::parse(&track).unwrap();
    let state = cine_timeline::graph::track_state(&g, entity).expect("state");
    assert!(
        state.locked,
        "the graph must hold the lock, not the projection"
    );
}

#[test]
fn an_unknown_track_state_is_refused_rather_than_ignored() {
    let h = Harness::start();
    let mut c = h.client();
    let id = c.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = c.call("timeline", json!({ "id": id }));
    let track = t["tracks"][0]["id"].as_str().unwrap().to_string();

    let mut raw = h.client();
    raw.send_masked(
        &json!({"jsonrpc":"2.0","id":99,"method":"track_state",
                "params":{"track":track,"key":"invisible","value":true}})
        .to_string(),
    );
    // The reply must be an error, not a silent success.
    let reply = raw.read_reply(99);
    assert!(reply["error"].is_object(), "got {reply}");
}

// ---------------------------------------------------------------------------
// Live propagation
// ---------------------------------------------------------------------------

/// The defining claim: one project, many views. A change made by one client
/// must reach the other without a reload, and both must then read the same
/// revision from the same engine.
#[test]
fn a_change_by_one_client_reaches_another_and_they_agree() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();

    let id = a.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let track = a.call("timeline", json!({ "id": id }))["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = b.call("project", json!({}))["revision"].as_u64().unwrap();

    a.call(
        "track_state",
        json!({ "track": track, "key": "muted", "value": true }),
    );

    assert!(
        b.drain_changes(1, Duration::from_secs(5)) >= 1,
        "the second client was never told"
    );
    let after = b.call("project", json!({}))["revision"].as_u64().unwrap();
    assert!(
        after > before,
        "revision did not advance for the second client"
    );

    // B sees the change itself, not merely a notification.
    let seen = b.call("timeline", json!({ "id": id }));
    assert_eq!(seen["tracks"][0]["muted"], json!(true));

    // And back the other way, on the same engine.
    b.call(
        "track_state",
        json!({ "track": track, "key": "targeted", "value": true }),
    );
    assert!(a.drain_changes(1, Duration::from_secs(5)) >= 1);
    let back = a.call("timeline", json!({ "id": id }));
    assert_eq!(back["tracks"][0]["targeted"], json!(true));
    assert_eq!(
        back["tracks"][0]["muted"],
        json!(true),
        "the first change must still be there: this is one project, not two"
    );
}

/// Undo must propagate too, or a second window keeps showing a state the
/// project no longer has.
#[test]
fn undo_reaches_every_client() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    let id = a.call("timelines", json!({}))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let track = a.call("timeline", json!({ "id": id }))["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    a.call(
        "track_state",
        json!({ "track": track, "key": "solo", "value": true }),
    );
    b.drain_changes(1, Duration::from_secs(5));
    assert_eq!(
        b.call("timeline", json!({ "id": id }))["tracks"][0]["solo"],
        json!(true)
    );

    a.call("undo", json!({}));
    assert!(
        b.drain_changes(1, Duration::from_secs(5)) >= 1,
        "undo was not broadcast"
    );
    assert_eq!(
        b.call("timeline", json!({ "id": id }))["tracks"][0]["solo"],
        json!(false),
        "the second client kept a state the project had undone"
    );
}

// ---------------------------------------------------------------------------
// Media and pictures
// ---------------------------------------------------------------------------

#[test]
fn ingested_media_appears_with_what_the_file_declares() {
    let Some(path) = media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let h = Harness::start();
    let mut c = h.client();
    // Importing a cut already registers the media its clips reference, so the
    // project is not empty here. What matters is that ingesting a real file
    // adds a real entry with what the file declares.
    let before = c.call("media", json!({})).as_array().unwrap().len();

    let ingested = c.call("ingest", json!({ "path": path.display().to_string() }));
    let id = ingested["id"].as_str().expect("an entity id").to_string();

    let media = c.call("media", json!({}));
    let list = media.as_array().unwrap();
    assert_eq!(
        list.len(),
        before + 1,
        "ingest added exactly one media item"
    );
    let m = list
        .iter()
        .find(|m| m["id"] == id.as_str())
        .expect("the ingested media");
    assert_eq!(m["online"], json!(true));
    assert!(
        m["hash"].as_str().unwrap().len() >= 16,
        "content hash recorded"
    );
    let video = &m["video"];
    assert_eq!(video["codec"], json!("h264"));
    assert_eq!(video["width"], json!(720));
    assert_eq!(video["height"], json!(1280));
    assert_eq!(video["frames"], json!(240));
    assert_eq!(video["decodable"], json!(true));
    assert!(!m["streams"].as_array().unwrap().is_empty());
}

/// A real decoded picture, over real HTTP, from the project's own media.
#[test]
fn a_decoded_picture_is_served_as_a_png() {
    let Some(path) = media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let h = Harness::start();
    let mut c = h.client();
    let id = c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, headers, body) = h.get(&format!("/frame?media={id}&index=0"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(header(&headers, "content-type"), Some("image/png"));
    assert_eq!(&body[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");

    // The dimensions in the PNG header must be the media's own.
    let width = u32::from_be_bytes(body[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(body[20..24].try_into().unwrap());
    assert_eq!((width, height), (720, 1280));
    assert!(
        body.len() > 10_000,
        "a real picture, not a stub: {} bytes",
        body.len()
    );

    // The colour interpretation travels with it, including that it was assumed.
    assert_eq!(header(&headers, "x-cine-colour-assumed"), Some("true"));
    assert!(header(&headers, "x-cine-colour")
        .unwrap()
        .contains("ASSUMED"));
    println!(
        "MEASURED first picture: {} bytes, {} ms",
        body.len(),
        header(&headers, "x-cine-decode-ms").unwrap_or("?")
    );

    // A different index gives a different picture.
    let (_, h2, body2) = h.get(&format!("/frame?media={id}&index=12"));
    assert_eq!(header(&h2, "x-cine-index"), Some("12"));
    assert_ne!(body, body2, "two indices returned the same image");
    println!(
        "MEASURED stepping to picture 12: {} ms",
        header(&h2, "x-cine-decode-ms").unwrap_or("?")
    );
}

#[test]
fn a_frame_request_for_unknown_media_is_refused() {
    let h = Harness::start();
    let missing = ucg_core::EntityId::from_key("no such media");
    let (status, _, body) = h.get(&format!("/frame?media={missing}&index=0"));
    assert!(status.contains("404"), "{status}");
    assert!(String::from_utf8_lossy(&body).contains("no media"));

    let (status, _, _) = h.get("/frame");
    assert!(
        status.contains("400"),
        "a request without media must be refused"
    );
}

// ---------------------------------------------------------------------------
// The production chain
// ---------------------------------------------------------------------------

#[test]
fn the_structure_projection_shows_scenes_from_the_screenplay() {
    let h = Harness::start();
    let mut c = h.client();
    let s = c.call("structure", json!({}));
    let scenes = s["scenes"].as_array().expect("scenes");
    assert_eq!(scenes.len(), 2, "the screenplay has two scenes");
    assert!(scenes[0]["heading"].as_str().unwrap().contains("HARBOUR"));
    assert!(scenes[0]["eighths"].is_i64());
    // No shots have been broken down in this fixture, and the projection says
    // so rather than inventing any.
    assert!(scenes[0]["shots"].as_array().unwrap().is_empty());
}
