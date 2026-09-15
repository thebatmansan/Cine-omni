//! Two clients, one runtime, one truth — over a real socket.
//!
//! The unit tests cover the pieces. This drives the actual server: a real
//! TCP connection, a real RFC 6455 handshake, real frames. It is the test that
//! would have failed under the previous architecture, where every browser
//! connection spawned its own `cine serve` and two windows diverged silently.

mod common;

use cine_pipeline::{OmniProject, ProjectSetup};
use cine_runtime::{Runtime, ServeConfig, Server};
use common::Client;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SCREENPLAY: &str = "Title: Runtime Test\n\nINT. OFFICE - NIGHT #1#\n\n\
    MAREN crosses the room.\n\nMAREN\nWe ride at dawn.\n\n";

#[test]
fn two_clients_share_one_project_and_never_diverge() {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");
    project
        .import_fountain(SCREENPLAY, "test.fountain")
        .expect("import");
    let runtime = Runtime::new(project, std::env::temp_dir());

    let server = Server::bind(
        runtime.clone(),
        ServeConfig {
            port: 0,
            ui_dir: None,
            ..Default::default()
        },
    )
    .expect("bind");
    let addr = server.local_addr().unwrap();
    let shutdown = server.shutdown_flag();
    let broadcaster = server.spawn_change_broadcaster();
    let serving = std::thread::spawn(move || server.run());

    let mut a = Client::connect(addr);
    let mut b = Client::connect(addr);

    // At rest both clients agree.
    let rev_a = a.call("stats", json!({}))["revision"].as_u64().unwrap();
    let rev_b = b.call("stats", json!({}))["revision"].as_u64().unwrap();
    assert_eq!(
        rev_a, rev_b,
        "clients disagree before anything has happened"
    );

    // A edits. The engine's cascade produces several deltas.
    let scenes = a.call("scenes", json!({}));
    let line = scenes[0]["lines"][0]
        .as_str()
        .expect("a dialogue line")
        .to_string();
    let cascade = a.call(
        "edit_dialogue",
        json!({ "line": line, "text": "We ride at midnight." }),
    );
    let deltas = cascade["deltas"].as_array().expect("deltas").len();
    assert!(deltas > 0, "the edit committed nothing");

    // B must hear about an edit it did not make. This is the whole point.
    let seen_by_b = b.drain_changes(1, Duration::from_secs(5));
    assert!(
        seen_by_b > 0,
        "the second client never learned of the first client's commit — the windows have diverged"
    );

    let after_a = a.call("stats", json!({}))["revision"].as_u64().unwrap();
    let after_b = b.call("stats", json!({}))["revision"].as_u64().unwrap();
    assert_eq!(after_a, after_b, "revisions diverged after an edit");
    assert!(after_a > rev_a, "the edit did not advance the revision");

    // And symmetrically: B undoes, A must see it.
    b.call("undo", json!({}));
    let seen_by_a = a.drain_changes(1, Duration::from_secs(5));
    assert!(
        seen_by_a > 0,
        "the first client never learned of the second client's undo"
    );
    assert_eq!(
        a.call("stats", json!({}))["revision"],
        b.call("stats", json!({}))["revision"],
        "undo diverged"
    );

    assert_eq!(
        runtime.client_count(),
        2,
        "the runtime should see exactly two clients"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = TcpStream::connect(addr);
    let _ = serving.join();
    let _ = broadcaster.join();
}

#[test]
fn the_health_endpoint_reports_the_shared_state() {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");
    let runtime = Runtime::new(project, std::env::temp_dir());
    let server = Server::bind(
        runtime,
        ServeConfig {
            port: 0,
            ui_dir: None,
            ..Default::default()
        },
    )
    .expect("bind");
    let addr = server.local_addr().unwrap();
    let shutdown = server.shutdown_flag();
    let serving = std::thread::spawn(move || server.run());

    let mut stream = TcpStream::connect(addr).unwrap();
    write!(stream, "GET /health HTTP/1.1\r\nHost: {addr}\r\n\r\n").unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _ = stream.read_to_string(&mut response);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("\"status\":\"ok\""), "{response}");
    assert!(response.contains("\"revision\""), "{response}");

    shutdown.store(true, Ordering::Relaxed);
    let _ = TcpStream::connect(addr);
    let _ = serving.join();
}

/// Without a built UI the runtime must still serve its API and say what is
/// missing, rather than appearing broken.
#[test]
fn a_missing_ui_is_explained_rather_than_returning_a_bare_404() {
    let project = OmniProject::new(ProjectSetup::default()).expect("project");
    let runtime = Runtime::new(project, std::env::temp_dir());
    let server = Server::bind(
        runtime,
        ServeConfig {
            port: 0,
            ui_dir: None,
            ..Default::default()
        },
    )
    .expect("bind");
    let addr = server.local_addr().unwrap();
    let shutdown = server.shutdown_flag();
    let serving = std::thread::spawn(move || server.run());

    let mut stream = TcpStream::connect(addr).unwrap();
    write!(stream, "GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n").unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _ = stream.read_to_string(&mut response);

    assert!(response.contains("404"));
    assert!(
        response.contains("npm run build"),
        "the response must say how to fix it: {response}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = TcpStream::connect(addr);
    let _ = serving.join();
}
