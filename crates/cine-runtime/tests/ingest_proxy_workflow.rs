//! The whole way from a file on a card to a proxy being watched.
//!
//! The other tests hold one step each. This one holds the chain: an import
//! asked for a proxy starts it, the proxy finishes and is registered against
//! the *imported* asset rather than one of its own, the timeline reads it when
//! proxies are asked for, and when the file under the asset changes the chain
//! reports it all the way to what is played — the proxy stale, the original
//! replaced, nothing silently substituted. Then the file comes back, and
//! everything is current again without anything being made a second time.

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

fn media_view(c: &mut Client, id: &str) -> Value {
    c.call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap()
        .clone()
}

struct Seq {
    tl: String,
}

fn sequence_with(c: &mut Client, media: &str, frames: i64) -> Seq {
    let made = c.call(
        "new_timeline",
        json!({ "name": "Cut", "rate_num": 24, "rate_den": 1 }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let (v1, a1) = (
        t["tracks"][0]["id"].as_str().unwrap().to_string(),
        t["tracks"][1]["id"].as_str().unwrap().to_string(),
    );
    let f24 = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [v1, a1], "time": f24(0),
                "clip": { "media": media, "source_range": { "start_time": f24(0), "duration": f24(frames) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    Seq { tl }
}

#[test]
fn an_import_that_asks_for_a_proxy_ends_with_that_proxy_being_watched() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C002.mov");
    cine_media::testsignal::bars_and_pops(&source, 640, 360, Rate::F24, 1).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();

    // One call: import this file, and make what follows from it.
    let began = Instant::now();
    let submitted = c.call(
        "job_submit",
        json!({
            "kind": "media.ingest",
            "path": source.display().to_string(),
            "thumbnail": true,
            "waveform": true,
            "proxy_height": 180,
        }),
    );
    let done = wait_for(&mut c, submitted["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    let out = &done["output"];
    let media = out["id"].as_str().unwrap().to_string();

    // The import itself does none of the derived work: it starts jobs for it.
    let dispatched: Vec<(String, u64)> = out["dispatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["kind"].as_str().unwrap().to_string(),
                d["job"].as_u64().unwrap(),
            )
        })
        .collect();
    let kinds: Vec<&str> = dispatched.iter().map(|(k, _)| k.as_str()).collect();
    assert!(kinds.contains(&"media.thumbnail"), "{out}");
    assert!(kinds.contains(&"media.waveform"), "{out}");
    assert!(kinds.contains(&"media.proxy"), "{out}");
    for (kind, id) in &dispatched {
        let job = wait_for(&mut c, *id);
        assert_eq!(job["state"], "succeeded", "{kind}: {job}");
    }
    println!(
        "MEASURED import of a 1 s 640x360 movie with sound, thumbnail, waveform and a 180p proxy, \
         submit to all derived work finished: {} ms (dev)",
        began.elapsed().as_millis()
    );

    // Everything hangs off the one asset the import made. No derived thing is
    // an asset of its own, and the browser is told so by the runtime.
    let view = media_view(&mut c, &media);
    let hash = out["content_hash"].as_str().unwrap();
    let same_content = c
        .call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["hash"] == json!(hash))
        .count();
    assert_eq!(
        same_content, 1,
        "the import made exactly one asset for this content: {view}"
    );
    assert_eq!(view["proxies"].as_array().unwrap().len(), 1, "{view}");
    assert_eq!(view["proxies"][0]["state"], "current", "{view}");
    assert_eq!(view["proxies"][0]["height"], 180, "{view}");
    assert_eq!(view["waveform"]["state"], "current", "{view}");
    let thumb = PathBuf::from(
        wait_for(
            &mut c,
            dispatched
                .iter()
                .find(|(k, _)| k == "media.thumbnail")
                .unwrap()
                .1,
        )["output"]["path"]
            .as_str()
            .unwrap(),
    );
    assert!(thumb.is_file(), "the thumbnail was written: {thumb:?}");

    // Cut with it, and watch the proxy because that is what was asked for.
    let s = sequence_with(&mut c, &media, 24);
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    assert_eq!(
        c.call("media_preference", json!({ "preference": "proxy" }))["preference"],
        "proxy"
    );
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(8) }));
    let proxy = shown["proxy"]
        .as_str()
        .expect("the proxy plays")
        .to_string();
    let list = c.call("edit_list", json!({ "timeline": s.tl }));
    assert_eq!(list["entries"][0]["proxy"], json!(proxy));
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=8&proxy={proxy}"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 320);

    // A different film arrives at the same path. The asset does not follow
    // it: what was derived was derived from content that has not changed, so
    // it stays current, while the *original* becomes unreachable and the
    // project says so in those words rather than playing the stranger.
    let kept = std::fs::read(&source).unwrap();
    let other = dir.path().join("other.mov");
    cine_media::testsignal::bars_and_pops(&other, 640, 360, Rate::F24, 2).unwrap();
    std::fs::copy(&other, &source).unwrap();
    let checked = c.call("check_media", json!({}));
    assert!(
        !checked["changed"].as_array().unwrap().is_empty(),
        "{checked}"
    );
    let view = media_view(&mut c, &media);
    assert_eq!(
        view["online"], false,
        "a namesake is not this media: {view}"
    );
    assert_eq!(view["resolution"]["state"], "replaced", "{view}");
    let note = view["resolution"]["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("different media now"),
        "the note names both contents: {view}"
    );
    assert_eq!(
        view["proxies"][0]["state"], "current",
        "the proxy is the old content's proxy, and that content did not change: {view}"
    );
    assert_eq!(view["waveform"]["state"], "current", "{view}");

    // The original cannot be shown, because the original is not there.
    c.call("media_preference", json!({ "preference": "original" }));
    let (status, _, _) = h.get(&format!("/frame?media={media}&index=8"));
    assert!(
        !status.contains("200"),
        "the file at that path is not this take: {status}"
    );
    // The proxy is still real work, and is still watchable.
    c.call("media_preference", json!({ "preference": "proxy" }));
    let shown = c.call("picture_at", json!({ "timeline": s.tl, "time": at(8) }));
    assert_eq!(shown["proxy"].as_str(), Some(proxy.as_str()), "{shown}");
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=8&proxy={proxy}"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 320);

    // The right file comes back. Nothing is remade; the asset resolves again,
    // because content is what it was always identified by.
    std::fs::write(&source, &kept).unwrap();
    c.call("check_media", json!({}));
    let view = media_view(&mut c, &media);
    assert_eq!(view["online"], true, "{view}");
    assert_eq!(
        view["resolution"]["state"],
        Value::Null,
        "nothing left to say about where it resolves: {view}"
    );
    assert_eq!(view["proxies"][0]["state"], "current", "{view}");
    assert_eq!(view["waveform"]["state"], "current", "{view}");
    c.call("media_preference", json!({ "preference": "original" }));
    let (status, _, body) = h.get(&format!("/frame?media={media}&index=8"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 640);
}
