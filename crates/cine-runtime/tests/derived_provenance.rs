//! What derived media depends on, held to exactly that.
//!
//! Proxies, waveforms and renders are all made *from* something, and the whole
//! value of saying so is that the answer to "is this still good" is neither
//! optimistic nor paranoid. Optimism plays the wrong pictures; paranoia throws
//! away hours of rendering because a file moved. So this file walks the matrix:
//!
//! | what happened | what must follow |
//! |---|---|
//! | same content, another path | everything stays current |
//! | the original is gone | derived data stays valid; the original is offline |
//! | a namesake with other content | the original is not online; derived data is untouched |
//! | the content really changed | the new content is a *different asset*, with nothing derived |
//! | the derived file changed | that one thing is stale |
//! | the recipe changed | that one thing is stale |
//! | an edit elsewhere in the cut | the render stays current |
//! | an edit the render read | the render is stale |

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

fn run(c: &mut Client, params: Value) -> Value {
    let job = c.call("job_submit", params);
    wait_for(c, job["id"].as_u64().unwrap())
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

/// A movie whose content depends on `seconds`, so two lengths are two takes.
fn movie(path: &Path, seconds: u32) -> PathBuf {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    cine_media::testsignal::bars_and_pops(path, 160, 90, Rate::F24, seconds).unwrap();
    path.to_path_buf()
}

/// Import, make a proxy and a waveform, cut it and render it. The state every
/// case below starts from.
struct Ready {
    media: String,
    timeline: String,
    track: String,
}

fn set_up(c: &mut Client, source: &Path) -> Ready {
    let done = run(
        c,
        json!({ "kind": "media.ingest", "path": source.display().to_string(), "proxy_height": 16 }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    for d in done["output"]["dispatched"].as_array().unwrap() {
        let finished = wait_for(c, d["job"].as_u64().unwrap());
        assert_eq!(finished["state"], "succeeded", "{finished}");
    }
    let media = done["output"]["id"].as_str().unwrap().to_string();

    let made = c.call(
        "new_timeline",
        json!({ "name": "Cut", "rate_num": 24, "rate_den": 1 }),
    );
    let timeline = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": timeline }));
    let track = t["tracks"][0]["id"].as_str().unwrap().to_string();
    let f24 = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [track, t["tracks"][1]["id"]], "time": f24(0),
                "clip": { "media": media, "source_range": { "start_time": f24(0), "duration": f24(12) } } }),
    );
    let rendered = run(
        c,
        json!({ "kind": "render.sequence", "timeline": timeline, "output": "png8",
                "start_frame": 0, "end_frame": 2 }),
    );
    assert_eq!(rendered["state"], "succeeded", "{rendered}");
    Ready {
        media,
        timeline,
        track,
    }
}

fn states(c: &mut Client, r: &Ready) -> (String, String, String, bool) {
    let view = media_view(c, &r.media);
    let renders = c.call("renders", json!({ "timeline": r.timeline }));
    (
        view["proxies"][0]["state"]
            .as_str()
            .unwrap_or("none")
            .to_string(),
        view["waveform"]["state"]
            .as_str()
            .unwrap_or("none")
            .to_string(),
        renders["renders"][0]["state"]
            .as_str()
            .unwrap_or("none")
            .to_string(),
        view["online"].as_bool().unwrap_or(false),
    )
}

#[test]
fn media_that_moved_leaves_everything_derived_from_it_current() {
    let dir = tempfile::tempdir().unwrap();
    let cards = dir.path().join("cards");
    let source = movie(&cards.join("day1/A001C001.mov"), 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let ready = set_up(&mut c, &source);
    assert_eq!(
        states(&mut c, &ready),
        ("current".into(), "current".into(), "current".into(), true)
    );

    // Moved, renamed, relinked by content.
    let moved = cards.join("archive/TAKE.mov");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&source, &moved).unwrap();
    let relinked = run(
        &mut c,
        json!({ "kind": "media.relink", "folder": cards.display().to_string() }),
    );
    assert_eq!(relinked["output"]["relinked"], 1, "{relinked}");

    let (proxy, waveform, render, online) = states(&mut c, &ready);
    assert_eq!(
        (proxy.as_str(), waveform.as_str(), render.as_str(), online),
        ("current", "current", "current", true),
        "a move is not a change: nothing derived from the content may go stale"
    );
    assert_eq!(
        media_view(&mut c, &ready.media)["path"],
        json!(moved.display().to_string())
    );
}

#[test]
fn an_original_that_goes_away_leaves_its_derived_data_valid_and_says_it_is_offline() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let ready = set_up(&mut c, &source);

    std::fs::remove_file(&source).unwrap();
    c.call("check_media", json!({}));
    let (proxy, waveform, render, online) = states(&mut c, &ready);
    assert!(!online, "the original is offline");
    assert_eq!(
        (proxy.as_str(), waveform.as_str(), render.as_str()),
        ("current", "current", "current"),
        "what was made from its content is still what was made from its content"
    );

    // And the cut can still be worked on: Auto plays the proxy, saying why.
    c.call("media_preference", json!({ "preference": "auto" }));
    let shown = c.call(
        "picture_at",
        json!({ "timeline": ready.timeline, "time": { "ticks": 3, "rate_num": 24, "rate_den": 1 } }),
    );
    assert!(shown["proxy"].is_string(), "{shown}");
    assert!(
        shown["substitution"].as_str().unwrap().contains("offline"),
        "{shown}"
    );
    assert_eq!(shown["playable"], true);

    // Reading the original itself refuses rather than inventing a picture.
    c.call("media_preference", json!({ "preference": "original" }));
    let (status, _, _) = h.get(&format!("/frame?media={}&index=3", ready.media));
    assert!(
        !status.contains("200"),
        "an offline original cannot be shown: {status}"
    );
}

#[test]
fn a_namesake_with_other_content_is_not_this_media_and_changes_nothing_derived() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 1);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let ready = set_up(&mut c, &source);

    // A different take written over it, with the same name.
    std::fs::remove_file(&source).unwrap();
    movie(&dir.path().join("A001C001.mov"), 2);
    c.call("check_media", json!({}));

    let view = media_view(&mut c, &ready.media);
    assert_eq!(view["online"], false, "a namesake is not this media");
    assert_eq!(view["resolution"]["state"], "replaced");
    let (proxy, waveform, render, _) = states(&mut c, &ready);
    assert_eq!(
        (proxy.as_str(), waveform.as_str(), render.as_str()),
        ("current", "current", "current"),
        "derived data was made from content that has not changed; only the file at that path has"
    );

    // Importing the new file makes a *different* asset — with nothing derived,
    // because nothing has been derived from that content.
    let done = run(
        &mut c,
        json!({ "kind": "media.ingest", "path": dir.path().join("A001C001.mov").display().to_string(),
                "thumbnail": false, "waveform": false }),
    );
    assert_eq!(done["state"], "succeeded", "{done}");
    let other = done["output"]["id"].as_str().unwrap().to_string();
    assert_ne!(other, ready.media, "other content is another asset");
    let view = media_view(&mut c, &other);
    assert_eq!(view["proxies"], json!([]), "and it has no proxy of its own");
    assert_eq!(view["waveform"], Value::Null);
}

#[test]
fn a_derived_file_that_changed_is_stale_and_only_that_one() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 1);
    let root = project(dir.path());
    let h = Harness::start_in(root.clone());
    let mut c = h.client();
    let ready = set_up(&mut c, &source);

    // Damage the proxy only.
    let proxy_path = PathBuf::from(
        media_view(&mut c, &ready.media)["proxies"][0]["path"]
            .as_str()
            .unwrap(),
    );
    let kept = std::fs::read(&proxy_path).unwrap();
    std::fs::write(&proxy_path, [kept.clone(), vec![0u8; 32]].concat()).unwrap();
    let (proxy, waveform, render, _) = states(&mut c, &ready);
    assert_eq!(proxy, "stale", "the proxy's own file changed");
    assert_eq!(
        (waveform.as_str(), render.as_str()),
        ("current", "current"),
        "and nothing else did"
    );
    std::fs::write(&proxy_path, &kept).unwrap();

    // Damage the render only: it cannot be trusted at all.
    let render_dir = PathBuf::from(
        c.call("renders", json!({ "timeline": ready.timeline }))["renders"][0]["path"]
            .as_str()
            .unwrap(),
    );
    std::fs::remove_file(render_dir.join("frame_000001.png")).unwrap();
    let (proxy, waveform, render, _) = states(&mut c, &ready);
    assert_eq!(
        render, "invalid",
        "a render missing a frame is not a render"
    );
    assert_eq!((proxy.as_str(), waveform.as_str()), ("current", "current"));
}

/// A recipe version is what a derived thing *means*: bump it and last week's
/// output is not what this build would make now. There is deliberately no
/// protocol call to rewrite an attribute — that would let any client bypass
/// every invariant — so this reaches the project directly, as a migration or
/// an older build would have left it.
#[test]
fn derived_data_made_by_an_older_recipe_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 1);
    let root = project(dir.path());
    let store = cine_pipeline::OmniProject::new(cine_pipeline::ProjectSetup {
        name: "Recipes".into(),
        ..Default::default()
    })
    .unwrap();
    let _ = store.directory.set(root.clone());

    let imported = cine_runtime::ingest::ingest(
        &store,
        &cine_runtime::ingest::Request {
            path: source.clone(),
            sequence: Some(false),
            rate: Rate::F24,
            derive: cine_runtime::ingest::Derive {
                thumbnail: false,
                waveform: false,
                proxy: None,
            },
        },
        &mut |_, _| true,
    )
    .unwrap()
    .unwrap();
    let made = cine_runtime::proxy::generate(
        &store,
        &root,
        imported.media,
        cine_runtime::proxy::ProxySettings {
            height: 16,
            quality: 30,
        },
        None,
        &mut |_, _| true,
    )
    .unwrap();
    let proxy = ucg_core::EntityId::parse(made["proxy"].as_str().unwrap()).unwrap();
    assert!(
        cine_runtime::derived::status(
            &store.engine.read(),
            proxy,
            cine_runtime::proxy::RECIPE_VERSION,
            true
        )
        .is_current(),
        "it was just made"
    );

    // The same file, recorded as made by an older recipe.
    let older = store
        .engine
        .read()
        .node(proxy)
        .and_then(|n| n.get_str("recipe"))
        .unwrap()
        .replace("proxy/1", "proxy/0");
    store
        .engine
        .transact("an older build's recipe", |tx| {
            tx.set(proxy, "recipe", older.as_str())?;
            Ok(())
        })
        .unwrap();

    let status = cine_runtime::derived::status(
        &store.engine.read(),
        proxy,
        cine_runtime::proxy::RECIPE_VERSION,
        true,
    );
    assert_eq!(status.state, "stale", "{status:?}");
    assert!(status.reasons[0].contains("older recipe"), "{status:?}");
    // And nothing reads it: the choice of what plays asks the same question.
    let choice = cine_runtime::proxy::choose(
        &store.engine.read(),
        imported.media,
        cine_pipeline::session::MediaPreference::Proxy,
    );
    assert_eq!(
        choice.proxy, None,
        "a stale proxy is never chosen: {choice:?}"
    );
    assert!(choice.reason.unwrap().contains("stale"));
}

#[test]
fn a_render_is_stale_only_for_edits_it_actually_read() {
    let dir = tempfile::tempdir().unwrap();
    let source = movie(&dir.path().join("A001C001.mov"), 2);
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let ready = set_up(&mut c, &source);
    let f24 = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });

    // An edit beyond the rendered frames, and a whole other sequence.
    c.call("new_timeline", json!({ "name": "Elsewhere" }));
    c.call(
        "edit",
        json!({ "track": ready.track, "op": { "op": "ripple_trim", "index": 0, "edge": "tail", "delta": f24(-2) } }),
    );
    let (proxy, waveform, render, _) = states(&mut c, &ready);
    assert_eq!(
        (proxy.as_str(), waveform.as_str(), render.as_str()),
        ("current", "current", "current"),
        "an edit the render did not read changes nothing"
    );

    // An edit inside the frames it rendered.
    c.call(
        "edit",
        json!({ "track": ready.track, "op": { "op": "ripple_trim", "index": 0, "edge": "head", "delta": f24(3) } }),
    );
    let (proxy, waveform, render, _) = states(&mut c, &ready);
    assert_eq!(
        render, "stale",
        "the frames it rendered show something else now"
    );
    assert_eq!(
        (proxy.as_str(), waveform.as_str()),
        ("current", "current"),
        "and a cut is not a change to the media it plays"
    );
    c.call("undo", json!({}));
    assert_eq!(
        states(&mut c, &ready).2,
        "current",
        "undo restores what it read"
    );
}
