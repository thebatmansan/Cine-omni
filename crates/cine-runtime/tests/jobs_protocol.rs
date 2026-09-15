//! Background work through the protocol, on real media.

mod common;

use common::{media_path, Client, Harness};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn wait_for(c: &mut Client, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let jobs = c.call("jobs", json!({}));
        if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
            let state = job["state"].as_str().unwrap();
            if matches!(state, "succeeded" | "failed" | "cancelled") {
                return job.clone();
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ingest(c: &mut Client, path: &std::path::Path) -> String {
    c.call("ingest", json!({ "path": path.display().to_string() }))["id"]
        .as_str()
        .unwrap()
        .to_string()
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

#[test]
fn a_checksum_is_established_then_verified_and_every_client_hears_about_it() {
    let Some(path) = media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    let asset = ingest(&mut a, &path);

    let first = a.call(
        "job_submit",
        json!({ "kind": "media.checksum", "media": asset }),
    );
    assert_eq!(first["resource"], "io");
    let done = wait_for(&mut a, first["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["output"]["result"], "established");
    let expected = match cine_media::ContentHash::full(&path).unwrap() {
        cine_media::ContentHash::Full { hex, .. } => hex,
        _ => unreachable!(),
    };
    assert_eq!(
        done["output"]["sha256"], expected,
        "the digest is the file's SHA-256"
    );
    println!(
        "MEASURED checksum: {} bytes in {:.3} s ({:.0} MiB/s)",
        done["output"]["size"],
        done["output"]["seconds"].as_f64().unwrap(),
        done["output"]["mib_per_s"].as_f64().unwrap()
    );
    assert_eq!(media_view(&mut a, &asset)["checksum"]["sha256"], expected);

    let second = a.call(
        "job_submit",
        json!({ "kind": "media.checksum", "media": asset }),
    );
    assert_ne!(
        second["id"], first["id"],
        "verification is new work each time the project moved on"
    );
    let verified = wait_for(&mut a, second["id"].as_u64().unwrap());
    assert_eq!(verified["output"]["result"], "match");

    // The other client was told about the jobs as they happened.
    let mut heard = 0;
    b.stream_messages(Duration::from_millis(300), |m| {
        if m["method"] == "job" {
            heard += 1;
        }
    });
    assert!(
        heard >= 2,
        "job notifications reached the other client ({heard})"
    );
}

#[test]
fn verification_catches_a_changed_byte_that_the_fingerprint_cannot() {
    let Some(original) = media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("copy.mp4");
    std::fs::copy(&original, &copy).unwrap();
    let h = Harness::start();
    let mut c = h.client();
    let asset = ingest(&mut c, &copy);
    let fingerprint = media_view(&mut c, &asset)["hash"].clone();

    let job = c.call(
        "job_submit",
        json!({ "kind": "media.checksum", "media": asset }),
    );
    assert_eq!(
        wait_for(&mut c, job["id"].as_u64().unwrap())["output"]["result"],
        "established"
    );

    // One byte in the middle: outside the head and tail the fingerprint reads.
    let mut bytes = std::fs::read(&copy).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    std::fs::write(&copy, &bytes).unwrap();
    assert_eq!(
        cine_media::ContentHash::fast(&copy).unwrap().hex(),
        fingerprint.as_str().unwrap(),
        "the fast fingerprint is blind to this change"
    );

    let job = c.call(
        "job_submit",
        json!({ "kind": "media.checksum", "media": asset }),
    );
    let result = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(result["output"]["result"], "mismatch", "{result}");
    let record = &media_view(&mut c, &asset)["checksum"];
    assert_ne!(
        record["sha256"], record["last"]["sha256"],
        "the reference is kept, not overwritten"
    );
    assert_eq!(record["last"]["result"], "mismatch");
}

#[test]
fn a_thumbnail_is_made_in_the_background_and_served_from_the_cache() {
    let Some(path) = media_path() else {
        eprintln!("SKIP: no real media on this machine");
        return;
    };
    let h = Harness::start();
    let mut c = h.client();
    let asset = ingest(&mut c, &path);
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.thumbnail", "media": asset, "index": 7 }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    assert!(done["output"]["width"].as_u64().unwrap_or(320) <= 320);
    let (status, _, body) = h.get(&format!("/thumbnail?media={asset}&index=7"));
    assert!(status.contains("200"), "{status}");
    assert!(body.starts_with(b"\x89PNG"));
    let (missing, _, text) = h.get(&format!("/thumbnail?media={asset}&index=99999"));
    assert!(missing.contains("404"));
    assert!(String::from_utf8_lossy(&text).contains("media.thumbnail"));
}

#[test]
fn work_that_does_not_exist_or_cannot_change_state_is_refused() {
    let h = Harness::start();
    let mut raw = h.client();
    raw.send_masked(
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "job_submit",
        "params": { "kind": "media.teleport", "media": h.runtime.project.root.to_string() } })
        .to_string(),
    );
    let reply = raw.read_reply(1);
    assert!(reply["error"].is_object(), "{reply}");
    raw.send_masked(
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "job_cancel", "params": { "id": 424242 } })
            .to_string(),
    );
    assert!(raw.read_reply(2)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("cannot be cancelled"));
}
