//! Two clients, one runtime, one truth — over a real socket.
//!
//! The unit tests cover the pieces. This drives the actual server: a real
//! TCP connection, a real RFC 6455 handshake, real frames. It is the test that
//! would have failed under the previous architecture, where every browser
//! connection spawned its own `cine serve` and two windows diverged silently.

use cine_pipeline::{OmniProject, ProjectSetup};
use cine_runtime::ws::{self, Frame};
use cine_runtime::{Runtime, ServeConfig, Server};
use serde_json::json;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A client speaking JSON-RPC over a real WebSocket.
struct Client {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
    next_id: u64,
}

impl Client {
    fn connect(addr: std::net::SocketAddr) -> Client {
        let mut stream = TcpStream::connect(addr).expect("connect");
        // A fixed key is fine: the handshake's job is framing agreement, not
        // secrecy, and the accept value is checked below.
        write!(
            stream,
            "GET /ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\nUser-Agent: integration-test\r\n\r\n"
        )
        .expect("send handshake");
        stream.flush().unwrap();

        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut header = Vec::new();
        // Read exactly the header, stopping at the blank line so no frame bytes
        // are consumed.
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).expect("read handshake");
            header.push(byte[0]);
            if header.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header = String::from_utf8_lossy(&header).to_string();
        assert!(
            header.starts_with("HTTP/1.1 101"),
            "handshake refused: {header}"
        );
        assert!(
            header.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "wrong accept value; a browser would refuse this connection:\n{header}"
        );

        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            reader,
            stream,
            next_id: 1,
        }
    }

    /// Client frames must be masked, per RFC 6455 §5.1.
    fn send_masked(&mut self, text: &str) {
        let payload = text.as_bytes();
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut out = vec![0x81u8];
        let n = payload.len();
        if n < 126 {
            out.push(0x80 | n as u8);
        } else {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&out).expect("send");
        self.stream.flush().unwrap();
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send_masked(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
        );
        // Notifications may arrive before the reply; keep reading until the
        // matching id turns up.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Frame::Text(t) = ws::read_frame(&mut self.reader).expect("read") {
                let v: serde_json::Value = serde_json::from_str(&t).expect("json");
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    if let Some(err) = v.get("error") {
                        panic!("{method} failed: {err}");
                    }
                    return v["result"].clone();
                }
            }
        }
        panic!("no reply to {method}");
    }

    /// Drain queued messages, returning how many were `changed` notifications.
    fn drain_changes(&mut self, at_least: usize, within: Duration) -> usize {
        let deadline = Instant::now() + within;
        let mut seen = 0;
        self.stream
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        while Instant::now() < deadline && seen < at_least {
            match ws::read_frame(&mut self.reader) {
                Ok(Frame::Text(t)) => {
                    let v: serde_json::Value = serde_json::from_str(&t).unwrap_or(json!({}));
                    if v.get("method").and_then(|m| m.as_str()) == Some("changed") {
                        seen += 1;
                    }
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        seen
    }
}

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
