//! A client speaking the real protocol over a real socket.
//!
//! Shared by the runtime integration tests. It lives here rather than being
//! copied into each of them: the framing and the handshake are the thing under
//! test, and two copies of them could drift into agreeing with each other while
//! both being wrong.

// A shared test module is compiled separately into every test binary that
// declares it, so a helper only one of them uses looks dead to the others.
#![allow(dead_code)]

use cine_runtime::ws::{self, Frame};
use serde_json::json;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
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
