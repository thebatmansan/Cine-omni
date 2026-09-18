//! A small HTTP server: static UI files, a health endpoint, and the WebSocket
//! upgrade.
//!
//! Thread per connection. This serves a handful of local windows on the
//! loopback interface, not the public internet, and a thread costs less than an
//! async runtime does in complexity across a synchronous codebase.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Component, Path, PathBuf};

/// A parsed request line and headers. Bodies are not read: nothing this server
/// serves takes one.
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Whether this is a WebSocket upgrade, per RFC 6455 §4.2.1.
    pub fn is_websocket_upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
            && self
                .header("connection")
                .is_some_and(|v| v.to_ascii_lowercase().contains("upgrade"))
            && self.header("sec-websocket-key").is_some()
    }
}

/// Read the request line and headers from a connection.
pub fn read_request<R: Read>(stream: &mut BufReader<R>) -> std::io::Result<Option<HttpRequest>> {
    let mut line = String::new();
    if stream.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let (method, path) = (method.to_string(), path.to_string());

    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if stream.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Ok(Some(HttpRequest {
        method,
        path,
        headers,
    }))
}

pub fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// `respond` with extra headers, for a response a browser should treat as a
/// file rather than a page.
pub fn respond_with(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra: &[&str],
    body: &[u8],
) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n",
        body.len()
    );
    for header in extra {
        head.push_str(header);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

pub fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}

/// Resolve a URL path to a file inside `root`, refusing to escape it.
///
/// Path traversal is the one security question a static file server has to get
/// right. Rather than pattern-matching on `..`, which misses encodings and
/// symlinks, the resolved path is required to remain inside the canonicalised
/// root — so a link pointing outside is refused even though it contains no `..`.
pub fn resolve_static(root: &Path, url_path: &str) -> Option<PathBuf> {
    let trimmed = url_path
        .split('?')
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    let relative = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };

    // Reject anything that is not a plain name before touching the filesystem.
    let candidate = Path::new(relative);
    if candidate
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }

    let root = root.canonicalize().ok()?;
    let full = root.join(candidate).canonicalize().ok()?;
    full.starts_with(&root).then_some(full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn request(raw: &str) -> HttpRequest {
        read_request(&mut BufReader::new(raw.as_bytes()))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_websocket_upgrade_is_recognised() {
        let r = request(
            "GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: abc\r\n\r\n",
        );
        assert_eq!(r.path, "/ws");
        assert!(r.is_websocket_upgrade());
        // Header lookup is case-insensitive, as HTTP requires.
        assert_eq!(r.header("SEC-WEBSOCKET-KEY"), Some("abc"));
    }

    /// Browsers send `Connection: keep-alive, Upgrade`, not a bare `Upgrade`.
    #[test]
    fn an_upgrade_inside_a_compound_connection_header_is_recognised() {
        let r = request(
            "GET /ws HTTP/1.1\r\nUpgrade: WebSocket\r\n\
             Connection: keep-alive, Upgrade\r\nSec-WebSocket-Key: k\r\n\r\n",
        );
        assert!(r.is_websocket_upgrade());
    }

    #[test]
    fn an_ordinary_get_is_not_an_upgrade() {
        let r = request("GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(!r.is_websocket_upgrade());
    }

    #[test]
    fn path_traversal_is_refused() {
        let dir = tempdir();
        std::fs::write(dir.join("index.html"), "ok").unwrap();
        let secret = dir.parent().unwrap().join("outside.txt");
        std::fs::write(&secret, "secret").unwrap();

        assert!(resolve_static(&dir, "/index.html").is_some());
        for attack in [
            "/../outside.txt",
            "/../../etc/passwd",
            "/subdir/../../outside.txt",
            "//etc/passwd",
            "/./../outside.txt",
        ] {
            assert!(
                resolve_static(&dir, attack).is_none(),
                "traversal not refused: {attack}"
            );
        }
        let _ = std::fs::remove_file(secret);
    }

    /// A symlink pointing outside the root contains no `..`, so a textual check
    /// would pass it. Canonicalising catches it.
    #[test]
    fn a_symlink_escaping_the_root_is_refused() {
        let dir = tempdir();
        let outside = dir.parent().unwrap().join("escape_target.txt");
        std::fs::write(&outside, "secret").unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("escape.txt");
            let _ = std::os::unix::fs::symlink(&outside, &link);
            assert!(
                resolve_static(&dir, "/escape.txt").is_none(),
                "a symlink out of the root must be refused"
            );
        }
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn the_root_path_serves_the_index() {
        let dir = tempdir();
        std::fs::write(dir.join("index.html"), "ok").unwrap();
        let resolved = resolve_static(&dir, "/").expect("root resolves");
        assert!(resolved.ends_with("index.html"));
        // A query string must not become part of the filename.
        assert!(resolve_static(&dir, "/?v=2").is_some());
    }

    #[test]
    fn content_types_cover_what_the_ui_ships() {
        assert!(content_type_for(Path::new("a.html")).starts_with("text/html"));
        assert!(content_type_for(Path::new("a.js")).starts_with("text/javascript"));
        assert!(content_type_for(Path::new("a.css")).starts_with("text/css"));
        assert_eq!(
            content_type_for(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("cine-http-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

/// Text for an HTTP header value: percent-encoded outside printable ASCII.
///
/// Header bytes are not UTF-8 to a browser; it reads them as Latin-1, so a
/// `·` sent raw arrives as `Â·`. Encoding makes the value exact at the other
/// end with `decodeURIComponent`, and leaves plain ASCII readable as it is.
pub fn header_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if (0x20..0x7F).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod header_tests {
    use super::header_text;

    #[test]
    fn non_ascii_is_encoded_and_ascii_is_left_alone() {
        assert_eq!(
            header_text("BT.709 · limited range"),
            "BT.709 %C2%B7 limited range"
        );
        assert_eq!(header_text("50%"), "50%25");
        assert_eq!(header_text("plain"), "plain");
        assert!(header_text("ASSUMED — no matrix\r\n").is_ascii());
        assert!(!header_text("a\r\nInjected: yes").contains('\n'));
    }
}
