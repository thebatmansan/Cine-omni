//! Killing the runtime and restarting it, for real.
//!
//! A crash test that calls a shutdown function tests the shutdown function.
//! This spawns the actual `cine` binary, edits a project through its
//! WebSocket API **without saving**, sends `SIGKILL` — which no handler can
//! intercept, so no destructor runs and no snapshot is written — and then
//! starts it again.
//!
//! What has to survive:
//!
//! * every committed edit, from the op log alone (the snapshot on disk predates
//!   them);
//! * the project lock, which the dead process could not release, being
//!   reclaimed rather than blocking the project forever;
//! * a partial record left by a write that was interrupted mid-line, discarded
//!   **and reported** rather than silently swallowed.
//!
//! `#[ignore]`d because it needs the release binary. Run with:
//!
//! ```text
//! cargo build --release -p cine-cli
//! cargo test -p cine-runtime --test crash_recovery -- --ignored --nocapture
//! ```

use cine_runtime::ws::{self, Frame};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn binary() -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    for _ in 0..3 {
        dir.pop();
    }
    // target/debug/deps/<test> -> target/release/cine
    let release = dir.join("release/cine");
    release.exists().then_some(release)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

const SCREENPLAY: &str = "Title: Crash\n\nINT. OFFICE - NIGHT #1#\n\n\
    MAREN crosses the room.\n\nMAREN\nWe ride at dawn.\n\n";

struct Runtime {
    child: Child,
    port: u16,
}

impl Runtime {
    /// Start the binary and wait until it answers on /health.
    fn start(bin: &Path, project: &Path, port: u16) -> Runtime {
        let child = Command::new(bin)
            .arg("start")
            .arg(project)
            .arg("--no-window")
            .args(["--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cine start");
        let rt = Runtime { child, port };
        rt.wait_until_serving();
        rt
    }

    fn wait_until_serving(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.health().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the runtime never began serving on port {}", self.port);
    }

    fn health(&self) -> Option<String> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        write!(s, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n").ok()?;
        s.flush().ok()?;
        let mut body = String::new();
        let _ = s.read_to_string(&mut body);
        body.contains("\"status\":\"ok\"").then_some(body)
    }

    fn revision(&self) -> u64 {
        let body = self.health().expect("health");
        let at = body.find("\"revision\":").expect("revision field") + 11;
        body[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("revision number")
    }

    /// One JSON-RPC call over a real WebSocket.
    fn call(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        write!(
            stream,
            "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .unwrap();
        stream.flush().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("handshake");
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();

        let body =
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                .to_string();
        let payload = body.as_bytes();
        let mask = [1u8, 2, 3, 4];
        let mut out = vec![0x81u8];
        if payload.len() < 126 {
            out.push(0x80 | payload.len() as u8);
        } else {
            out.push(0x80 | 126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        stream.write_all(&out).unwrap();
        stream.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Frame::Text(t)) = ws::read_frame(&mut reader) {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                    assert!(v.get("error").is_none(), "{method} failed: {v}");
                    return v["result"].clone();
                }
            }
        }
        panic!("no reply to {method}");
    }

    /// SIGKILL: uncatchable, so nothing gets to clean up.
    fn kill_hard(&mut self) {
        let pid = self.child.id();
        // `Child::kill` is SIGKILL on Unix, which is what we want here.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && Path::new(&format!("/proc/{pid}")).exists() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn stderr(&mut self) -> String {
        let mut s = String::new();
        if let Some(e) = self.child.stderr.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    }
}

fn make_project(bin: &Path, dir: &Path) {
    let script = dir.with_extension("fountain");
    std::fs::write(&script, SCREENPLAY).unwrap();
    let ok = Command::new(bin)
        .args(["new", dir.to_str().unwrap(), "--name", "Crash"])
        .output()
        .expect("cine new");
    assert!(ok.status.success(), "cine new failed");
    let ok = Command::new(bin)
        .args(["import", dir.to_str().unwrap(), script.to_str().unwrap()])
        .output()
        .expect("cine import");
    assert!(ok.status.success(), "cine import failed");
}

#[test]
#[ignore]
fn a_killed_runtime_loses_no_committed_work() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first (cargo build --release -p cine-cli)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    make_project(&bin, &project);

    let port = free_port();
    let mut rt = Runtime::start(&bin, &project, port);

    // Commit edits and deliberately do NOT save: the only durable record is
    // the op log.
    let scenes = rt.call("scenes", serde_json::json!({}));
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    let line = scenes[0]["lines"][0].as_str().unwrap().to_string();
    rt.call(
        "edit_dialogue",
        serde_json::json!({ "line": line, "text": "We ride at dawn and we do not look back." }),
    );
    rt.call(
        "set",
        serde_json::json!({ "id": scene, "key": "notes", "value": "continuity: lamp on" }),
    );
    let before = rt.revision();
    assert!(before > 0);

    // The snapshot must predate the edits, or this proves nothing.
    let snapshot = std::fs::metadata(project.join("graph.json")).unwrap();
    let oplog = std::fs::metadata(project.join("oplog/current.jsonl"))
        .expect("the op log must exist -- autosave should append every commit");
    assert!(
        oplog.modified().unwrap() >= snapshot.modified().unwrap(),
        "the op log is older than the snapshot; the edits were already saved and \
         this test would prove nothing about recovery"
    );

    rt.kill_hard();

    // The lock could not have been released by a killed process.
    let lock = project.join(".cine/runtime.lock");
    assert!(lock.exists(), "a killed runtime leaves its lock behind");

    // Restart. The stale lock must be reclaimed and the edits replayed.
    let mut rt2 = Runtime::start(&bin, &project, free_port());
    assert_eq!(
        rt2.revision(),
        before,
        "the restarted runtime did not recover every committed edit"
    );
    rt2.stop();
}

#[test]
#[ignore]
fn a_torn_final_record_is_discarded_and_reported() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    make_project(&bin, &project);

    let mut rt = Runtime::start(&bin, &project, free_port());
    let scenes = rt.call("scenes", serde_json::json!({}));
    let scene = scenes[0]["id"].as_str().unwrap().to_string();
    rt.call(
        "set",
        serde_json::json!({ "id": scene, "key": "notes", "value": "before the tear" }),
    );
    let before = rt.revision();
    rt.kill_hard();

    // A write interrupted mid-line, which is what a crash during an append
    // actually leaves on disk.
    let log = project.join("oplog/current.jsonl");
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    f.write_all(br#"{"revision":999,"label":"torn","ops":[{"SetAttr":{"id":"dead"#)
        .unwrap();
    drop(f);

    let mut rt2 = Runtime::start(&bin, &project, free_port());
    assert_eq!(
        rt2.revision(),
        before,
        "the completed edits before the torn record must survive"
    );
    rt2.stop();

    let reported = rt2.stderr();
    assert!(
        reported.contains("mid-write"),
        "recovery discarded a partial record without saying so:\n{reported}"
    );
    assert!(
        reported.contains("nothing committed has been lost"),
        "the report must say what it means for the user's work:\n{reported}"
    );
}

/// A crashed runtime must not make the project permanently unopenable.
#[test]
#[ignore]
fn a_stale_lock_from_a_dead_runtime_is_reclaimed() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    make_project(&bin, &project);

    let mut rt = Runtime::start(&bin, &project, free_port());
    let killed_pid = rt.child.id();
    rt.kill_hard();

    let lock = std::fs::read_to_string(project.join(".cine/runtime.lock")).unwrap();
    assert!(
        lock.contains(&killed_pid.to_string()),
        "the lock should still name the dead process"
    );

    // It opens anyway, and takes the lock for itself.
    let mut rt2 = Runtime::start(&bin, &project, free_port());
    let now = std::fs::read_to_string(project.join(".cine/runtime.lock")).unwrap();
    assert!(
        !now.contains(&format!("\"pid\": {killed_pid}")),
        "the stale lock was not reclaimed"
    );
    rt2.stop();
}
