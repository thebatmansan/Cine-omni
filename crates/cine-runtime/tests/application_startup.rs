//! `cine start`, the actual binary, serving the actual application.
//!
//! Everything above this level tests the runtime as a library. This tests the
//! thing a filmmaker types: one command from a project directory that opens the
//! project, serves the interface, exposes the protocol, and owns the project so
//! that a second one cannot quietly start a rival engine.
//!
//! `#[ignore]`d because it needs the release binary and the built UI. Run with:
//!
//! ```text
//! cargo build --release -p cine-cli
//! cd ui && npm run build
//! cargo test -p cine-runtime --test application_startup -- --ignored --nocapture
//! ```

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

const SCREENPLAY: &str = "Title: Startup\n\nINT. HARBOUR OFFICE - NIGHT #1#\n\n\
    MAREN crosses the room.\n\nMAREN\nWe ride at dawn.\n\n";

struct App {
    child: Child,
    port: u16,
    project: PathBuf,
}

impl App {
    fn start(bin: &Path, project: &Path) -> App {
        let port = free_port();
        let child = Command::new(bin)
            .arg("start")
            .arg(project)
            .arg("--no-window")
            .args(["--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cine start");
        let app = App {
            child,
            port,
            project: project.to_path_buf(),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if app.get("/health").is_some() {
                return app;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the runtime never began serving on port {port}");
    }

    /// A raw GET, returning (status line, headers, body) when it answers.
    fn request(&self, path: &str) -> Option<(String, Vec<String>, Vec<u8>)> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        s.flush().ok()?;
        let mut reader = BufReader::new(s);
        let mut status = String::new();
        reader.read_line(&mut status).ok()?;
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            if line.trim().is_empty() {
                break;
            }
            headers.push(line.trim().to_string());
        }
        let mut body = Vec::new();
        reader.read_to_end(&mut body).ok()?;
        Some((status.trim().to_string(), headers, body))
    }

    fn get(&self, path: &str) -> Option<String> {
        let (status, _, body) = self.request(path)?;
        status
            .contains("200")
            .then(|| String::from_utf8_lossy(&body).to_string())
    }

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
        let mask = [0x11, 0x22, 0x33, 0x44];
        let mut frame = vec![0x81u8];
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        stream.write_all(&frame).unwrap();
        stream.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(cine_runtime::ws::Frame::Text(t)) = cine_runtime::ws::read_frame(&mut reader)
            {
                let v: serde_json::Value = serde_json::from_str(&t).expect("json");
                if v.get("id").and_then(|i| i.as_u64()) == Some(1) {
                    if let Some(err) = v.get("error") {
                        panic!("{method} failed: {err}");
                    }
                    return v["result"].clone();
                }
            }
        }
        panic!("no reply to {method}");
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.project);
    }
}

fn make_project(bin: &Path, dir: &Path) -> PathBuf {
    let script = dir.with_extension("fountain");
    std::fs::write(&script, SCREENPLAY).unwrap();
    assert!(
        Command::new(bin)
            .args(["new", dir.to_str().unwrap(), "--name", "Startup"])
            .output()
            .expect("cine new")
            .status
            .success(),
        "cine new failed"
    );
    assert!(
        Command::new(bin)
            .args(["import", dir.to_str().unwrap(), script.to_str().unwrap()])
            .output()
            .expect("cine import")
            .status
            .success(),
        "cine import failed"
    );
    script
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cine-startup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One command, from a project directory, and the application is available:
/// the project is open, the protocol answers, and the interface is served.
#[test]
#[ignore]
fn one_command_opens_the_project_and_serves_the_application() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first");
        return;
    };
    let dir = scratch("serve");
    let script = make_project(&bin, &dir);
    let app = App::start(&bin, &dir);

    // The project is open and reports itself.
    let health = app.get("/health").expect("health");
    assert!(health.contains("\"status\":\"ok\""), "{health}");
    let project = app.call("project", serde_json::json!({}));
    assert!(project["revision"].as_u64().unwrap() > 0);
    assert!(
        project["by_kind"]["script.scene"].as_u64().unwrap() >= 1,
        "the imported screenplay must be in the project the runtime opened"
    );
    println!(
        "MEASURED startup: project {} at revision {}, {} nodes",
        project["name"], project["revision"], project["nodes"]
    );

    // The interface is served, not merely the API.
    let index = app.get("/").expect("the UI must be served");
    assert!(
        index.contains("<div id=\"root\""),
        "not the application page"
    );
    assert!(
        index.contains("assets/index-") && index.contains(".js"),
        "the page must load the built bundle: {index}"
    );
    let bundle = index
        .split("src=\"/")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("bundle path");
    let (status, _, body) = app.request(&format!("/{bundle}")).expect("bundle");
    assert!(status.contains("200"), "bundle not served: {status}");
    assert!(body.len() > 50_000, "bundle is {} bytes", body.len());

    // The application's own protocol answers, not only the old one.
    let caps = app.call("capabilities", serde_json::json!({}));
    assert!(caps["capabilities"].as_array().unwrap().len() > 8);
    assert!(app.call("timelines", serde_json::json!({})).is_array());
    assert!(app.call("media", serde_json::json!({})).is_array());
    assert!(app.call("structure", serde_json::json!({}))["scenes"].is_array());

    let _ = std::fs::remove_file(script);
}

/// A second `cine start` must attach to the running one or say who owns the
/// project. What it must never do is start a rival engine: two op logs over one
/// project is how a cut gets silently corrupted.
#[test]
#[ignore]
fn a_second_start_defers_to_the_runtime_that_owns_the_project() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first");
        return;
    };
    let dir = scratch("owner");
    make_project(&bin, &dir);
    let app = App::start(&bin, &dir);
    let before = app.call("project", serde_json::json!({}))["revision"]
        .as_u64()
        .unwrap();

    let second = Command::new(&bin)
        .arg("start")
        .arg(&dir)
        .arg("--no-window")
        .output()
        .expect("second cine start");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        said.contains("already running"),
        "a second start must report the owner, said: {said}"
    );
    assert!(
        said.contains(&app.port.to_string()) || said.contains("http://"),
        "it must say where the running one is: {said}"
    );
    println!("MEASURED second start said:\n{}", said.trim());

    // And the first runtime is untouched: same engine, same revision, still
    // serving.
    let after = app.call("project", serde_json::json!({}))["revision"]
        .as_u64()
        .unwrap();
    assert_eq!(
        before, after,
        "the second start disturbed the running project"
    );
    assert!(app.get("/health").is_some(), "the owner stopped serving");
}

/// What the application shows about itself must come from the runtime, and the
/// runtime must not claim what this build cannot do.
#[test]
#[ignore]
fn the_served_application_is_told_the_truth_about_playback() {
    let Some(bin) = binary() else {
        eprintln!("SKIP: build the release binary first");
        return;
    };
    let dir = scratch("caps");
    make_project(&bin, &dir);
    let app = App::start(&bin, &dir);

    let caps = app.call("capabilities", serde_json::json!({}));
    let list = caps["capabilities"].as_array().unwrap();
    let find = |id: &str| {
        list.iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no capability {id}"))
    };
    assert_eq!(find("media.playback")["state"], "unavailable");
    assert_eq!(find("audio.monitoring")["state"], "unavailable");
    assert_eq!(find("media.preview")["state"], "degraded");
    for c in list {
        assert!(
            c["detail"].as_str().unwrap_or("").len() > 15,
            "{} states no reason",
            c["id"]
        );
    }
    println!(
        "MEASURED {} capabilities reported by the running binary",
        list.len()
    );
}
