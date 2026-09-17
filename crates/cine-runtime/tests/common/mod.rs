//! A client speaking the real protocol over a real socket.
//!
//! Shared by the runtime integration tests. It lives here rather than being
//! copied into each of them: the framing and the handshake are the thing under
//! test, and two copies of them could drift into agreeing with each other while
//! both being wrong.

// A shared test module is compiled separately into every test binary that
// declares it, so a helper only one of them uses looks dead to the others.
#![allow(dead_code)]

use cine_pipeline::{OmniProject, ProjectSetup};
use cine_runtime::ws::{self, Frame};
use cine_runtime::{Runtime, ServeConfig, Server};
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A client speaking JSON-RPC over a real WebSocket.
pub struct Client {
    reader: BufReader<TcpStream>,
    stream: TcpStream,
    next_id: u64,
}

impl Client {
    pub fn connect(addr: std::net::SocketAddr) -> Client {
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
    pub fn send_masked(&mut self, text: &str) {
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

    pub fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
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

    /// Like `call`, but a refusal is a value to assert on rather than a panic.
    pub fn call_result(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_masked(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Frame::Text(t) = ws::read_frame(&mut self.reader).expect("read") {
                let v: serde_json::Value = serde_json::from_str(&t).expect("json");
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    return match v.get("error") {
                        Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                        None => Ok(v["result"].clone()),
                    };
                }
            }
        }
        panic!("no reply to {method}");
    }

    /// Drain queued messages, returning how many were `changed` notifications.
    pub fn drain_changes(&mut self, at_least: usize, within: Duration) -> usize {
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

    /// Hand every message that arrives within `window` to `f`.
    pub fn stream_messages(&mut self, window: Duration, mut f: impl FnMut(&serde_json::Value)) {
        let deadline = Instant::now() + window;
        self.stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        while Instant::now() < deadline {
            if let Ok(Frame::Text(t)) = ws::read_frame(&mut self.reader) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    f(&v);
                }
            }
        }
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
    }

    /// Read until a reply with `id` arrives, returning the whole message so a
    /// test can inspect an error as well as a result.
    pub fn read_reply(&mut self, id: u64) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Frame::Text(t)) = ws::read_frame(&mut self.reader) {
                let v: serde_json::Value = serde_json::from_str(&t).expect("json");
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    return v;
                }
            }
        }
        panic!("no reply with id {id}");
    }
}

// ---------------------------------------------------------------------------
// A running server over a project built the way a user's would be
// ---------------------------------------------------------------------------

pub const SCREENPLAY: &str = "Title: Protocol Test\n\nINT. HARBOUR OFFICE - NIGHT #1#\n\n\
    MAREN crosses the room.\n\nMAREN\nWe ride at dawn.\n\n\
    EXT. QUAY - DAWN #2#\n\nThe boats are gone.\n\n";

pub const TIMELINE: &str = include_str!("../../../../tests/fixtures/sample.otio");

/// The verified media asset, when this machine has it.
pub fn media_path() -> Option<PathBuf> {
    ["/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

pub struct Harness {
    pub addr: std::net::SocketAddr,
    pub runtime: std::sync::Arc<Runtime>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Harness {
    pub fn start() -> Harness {
        Harness::start_in(std::env::temp_dir())
    }

    /// A harness whose project directory — where jobs journal and renders
    /// land — is `root`.
    pub fn start_in(root: std::path::PathBuf) -> Harness {
        let project = OmniProject::new(ProjectSetup::default()).expect("project");
        project
            .import_fountain(SCREENPLAY, "test.fountain")
            .expect("screenplay");
        project
            .import_otio(TIMELINE, "test.otio")
            .expect("timeline");
        let runtime = Runtime::new(project, root);
        let server = Server::bind(
            runtime.clone(),
            ServeConfig {
                port: 0,
                ui_dir: None,
                ..Default::default()
            },
        )
        .expect("bind");
        let addr = server.local_addr().expect("addr");
        let shutdown = server.shutdown_flag();
        server.spawn_change_broadcaster();
        std::thread::spawn(move || {
            let _ = server.run();
        });
        Harness {
            addr,
            runtime,
            shutdown,
        }
    }

    pub fn client(&self) -> Client {
        Client::connect(self.addr)
    }

    /// A raw HTTP GET, returning (status line, headers, body).
    pub fn get(&self, path: &str) -> (String, Vec<String>, Vec<u8>) {
        let mut stream = TcpStream::connect(self.addr).expect("connect");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .expect("request");
        stream.flush().unwrap();
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).expect("status");
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header");
            if line.trim().is_empty() {
                break;
            }
            headers.push(line.trim().to_string());
        }
        let mut body = Vec::new();
        reader.read_to_end(&mut body).expect("body");
        (status.trim().to_string(), headers, body)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
    }
}

pub fn header<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| {
            h.to_ascii_lowercase()
                .starts_with(&name.to_ascii_lowercase())
        })
        .and_then(|h| h.split_once(':'))
        .map(|(_, v)| v.trim())
}
