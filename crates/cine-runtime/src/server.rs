//! The listening runtime: accept connections, serve the UI, carry RPC.

use crate::http::{content_type_for, read_request, resolve_static, respond, respond_with};
use crate::rpc::{self, Request};
use crate::session::Runtime;
use crate::ws::{self, Frame};
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How the runtime is configured at start.
pub struct ServeConfig {
    /// Directory of built UI assets, if the UI has been built.
    pub ui_dir: Option<PathBuf>,
    /// Preferred port; 0 asks the OS for any free one.
    pub port: u16,
    /// Loopback only by default: a film project is not a public web service.
    pub bind_host: String,
}

impl Default for ServeConfig {
    fn default() -> Self {
        ServeConfig {
            ui_dir: None,
            port: 7331,
            bind_host: "127.0.0.1".into(),
        }
    }
}

pub struct Server {
    listener: TcpListener,
    runtime: Arc<Runtime>,
    config: ServeConfig,
    shutdown: Arc<AtomicBool>,
}

impl Server {
    /// Bind, falling back to an OS-assigned port if the preferred one is taken.
    ///
    /// A busy port usually means something unrelated is listening, not that a
    /// runtime is running — the instance lock answers that question, and it has
    /// already been consulted by the time we get here. So moving to another port
    /// is right, and the caller reports the address actually bound.
    pub fn bind(runtime: Arc<Runtime>, config: ServeConfig) -> std::io::Result<Server> {
        let preferred: SocketAddr = format!("{}:{}", config.bind_host, config.port)
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
        let listener = TcpListener::bind(preferred).or_else(|_| {
            let any: SocketAddr = format!("{}:0", config.bind_host).parse().unwrap();
            TcpListener::bind(any)
        })?;
        Ok(Server {
            listener,
            runtime,
            config,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// A flag that makes [`Server::run`] return at the next accept.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Fan engine commits out to every client, for as long as the runtime lives.
    ///
    /// One subscriber, many clients. This is what makes two windows agree: the
    /// engine is the only source of revisions, and every client is told about
    /// every commit regardless of which client caused it.
    pub fn spawn_change_broadcaster(&self) -> std::thread::JoinHandle<()> {
        let runtime = self.runtime.clone();
        let shutdown = self.shutdown.clone();
        // Session changes (which sequence is open) go out as they happen. The
        // listener holds the runtime weakly: the runtime owns the session that
        // owns the listener.
        if let Some(jobs) = runtime.project.jobs.get() {
            let weak = Arc::downgrade(&runtime);
            jobs.subscribe(move |record| {
                if let Some(rt) = weak.upgrade() {
                    rt.broadcast(&rpc::job_notification(record));
                }
            });
        }
        let weak = Arc::downgrade(&runtime);
        runtime.project.session.subscribe(move |event| {
            if let Some(rt) = weak.upgrade() {
                rt.broadcast(&rpc::session_notification(event));
            }
        });
        let rx = runtime.project.engine.subscribe_channel();
        std::thread::Builder::new()
            .name("cine-changes".into())
            .spawn(move || {
                while !shutdown.load(Ordering::Relaxed) {
                    match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                        Ok(cs) => {
                            runtime.broadcast(&rpc::change_notification(&cs));
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn change broadcaster")
    }

    /// Accept connections until shutdown.
    pub fn run(self) -> std::io::Result<()> {
        // A short accept timeout is what lets the shutdown flag be noticed
        // without a second signalling mechanism.
        self.listener.set_nonblocking(false)?;
        let shutdown = self.shutdown.clone();
        let ui_dir = self.config.ui_dir.clone();

        for incoming in self.listener.incoming() {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let stream = match incoming {
                Ok(s) => s,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            };
            // Replies are small and a client pipelines several. With Nagle's
            // algorithm on, the second reply waits for the ACK of the first,
            // which the client delays: a 40 ms stall per burst, measured as
            // a 46 ms median to refetch seven 1–3 ms projections.
            let _ = stream.set_nodelay(true);
            let runtime = self.runtime.clone();
            let ui_dir = ui_dir.clone();
            let shutdown = shutdown.clone();
            // A panic in one connection must not take the runtime down with it.
            let _ = std::thread::Builder::new()
                .name("cine-conn".into())
                .spawn(move || {
                    if let Err(e) = handle_connection(stream, runtime, ui_dir, shutdown) {
                        // Ordinary disconnects are not worth reporting.
                        if !matches!(e, ws::WsError::Closed) {
                            eprintln!("connection ended: {e}");
                        }
                    }
                });
        }
        Ok(())
    }
}

fn handle_connection(
    mut stream: TcpStream,
    runtime: Arc<Runtime>,
    ui_dir: Option<PathBuf>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), ws::WsError> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };

    if request.is_websocket_upgrade() {
        let key = request
            .header("sec-websocket-key")
            .unwrap_or("")
            .to_string();
        let agent = request
            .header("user-agent")
            .unwrap_or("unknown")
            .to_string();
        stream.write_all(ws::handshake_response(&key).as_bytes())?;
        stream.flush()?;
        return serve_websocket(stream, reader, runtime, &agent, shutdown);
    }

    let (route, query) = match request.path.split_once('?') {
        Some((r, q)) => (r, q),
        None => (request.path.as_str(), ""),
    };
    match route {
        "/frame" => serve_frame(&mut stream, &runtime, query),
        "/thumbnail" => serve_thumbnail(&mut stream, &runtime, query),
        "/render-file" => serve_render_file(&mut stream, &runtime, query),
        "/render-frame" => serve_render_frame(&mut stream, &runtime, query),
        "/callsheet" => serve_call_sheet(&mut stream, &runtime, query),
        "/health" => {
            let body = serde_json::json!({
                "status": "ok",
                "clients": runtime.client_count(),
                "revision": runtime.project.engine.revision(),
                "project": runtime.root_dir.display().to_string(),
                "uptime_s": runtime.uptime().as_secs(),
            })
            .to_string();
            respond(&mut stream, "200 OK", "application/json", body.as_bytes());
        }
        path => match ui_dir.as_deref().and_then(|d| resolve_static(d, path)) {
            Some(file) => match std::fs::read(&file) {
                Ok(body) => respond(&mut stream, "200 OK", content_type_for(&file), &body),
                Err(_) => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
            },
            None => {
                // No built UI is a configuration state, not a crash. Say what to
                // do about it rather than returning a bare 404.
                let body = match &ui_dir {
                    Some(d) => format!(
                        "CINE-OMNI runtime is serving, but no UI asset matched {path}.\n\
                         UI directory: {}\n",
                        d.display()
                    ),
                    None => "CINE-OMNI runtime is serving.\n\n\
                             The UI has not been built, so there is nothing to show here.\n\
                             Build it with:  cd ui && npm install && npm run build\n\
                             The WebSocket API is available at /ws and is unaffected.\n"
                        .to_string(),
                };
                respond(
                    &mut stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    body.as_bytes(),
                );
            }
        },
    }
    Ok(())
}

fn serve_websocket(
    stream: TcpStream,
    mut reader: BufReader<TcpStream>,
    runtime: Arc<Runtime>,
    agent: &str,
    shutdown: Arc<AtomicBool>,
) -> Result<(), ws::WsError> {
    let (client_id, outbound) = runtime.connect(agent);

    // One writer thread owns the socket's write half, so RPC replies and
    // broadcast notifications cannot interleave mid-frame.
    let mut writer = stream.try_clone()?;
    let writer_shutdown = shutdown.clone();
    let writer_thread = std::thread::Builder::new()
        .name(format!("cine-ws-out-{}", client_id.0))
        .spawn(move || {
            while !writer_shutdown.load(Ordering::Relaxed) {
                match outbound.recv_timeout(std::time::Duration::from_millis(250)) {
                    Ok(msg) => {
                        if ws::write_frame(&mut writer, &Frame::Text(msg)).is_err() {
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn websocket writer");

    // Tell the client who it is and what it has joined.
    runtime.send_to(
        client_id,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "welcome",
            "params": {
                "client": client_id.0,
                "clients": runtime.client_count(),
                "revision": runtime.project.engine.revision(),
                "project": runtime.root_dir.display().to_string(),
            }
        })
        .to_string(),
    );

    let result = loop {
        match ws::read_frame(&mut reader) {
            Ok(Frame::Text(line)) => {
                let reply = handle_rpc_line(&runtime, &line);
                if let Some(reply) = reply {
                    if !runtime.send_to(client_id, &reply) {
                        break Ok(());
                    }
                }
            }
            Ok(Frame::Ping(payload)) => {
                let mut w = stream.try_clone()?;
                ws::write_frame(&mut w, &Frame::Pong(payload))?;
            }
            Ok(Frame::Close) | Err(ws::WsError::Closed) => break Ok(()),
            Ok(_) => continue,
            Err(e) => break Err(e),
        }
    };

    runtime.disconnect(client_id);
    let _ = writer_thread.join();
    result
}

/// One day's call sheet, as the PDF that gets printed and taped to a wall.
///
/// Built on request: it is a page of text derived from state the runtime
/// already holds, which is not the same kind of work as decoding a picture.
fn serve_call_sheet(stream: &mut TcpStream, runtime: &Runtime, query: &str) {
    let day = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("day="))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    match crate::rpc::call_sheet_pdf(&runtime.project, day) {
        Ok(bytes) => respond_with(
            stream,
            "200 OK",
            "application/pdf",
            &[&format!(
                "Content-Disposition: inline; filename=\"call-sheet-day-{day}.pdf\""
            )],
            &bytes,
        ),
        Err(why) => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            why.as_bytes(),
        ),
    }
}

/// A thumbnail from the project's cache, if one has been made.
///
/// Never made on request: making one is a job (`media.thumbnail`), so a page of
/// thumbnails cannot turn into a page of synchronous decodes. A missing one is a
/// 404 that says how to make it.
fn serve_thumbnail(stream: &mut TcpStream, runtime: &Runtime, query: &str) {
    let mut media = None;
    let mut index = 0usize;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("media", v)) => media = ucg_core::EntityId::parse(v),
            Some(("index", v)) => index = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    let hash = media.and_then(|m| {
        let g = runtime.project.engine.read();
        g.node(m)
            .and_then(|n| n.get_str(cine_media::ucg::HASH_ATTR).map(str::to_string))
    });
    let Some(hash) = hash else {
        respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"no media with that id in this project",
        );
        return;
    };
    let path = crate::jobs::thumbnail_path(
        &crate::jobs::thumbnail_cache(&runtime.root_dir),
        &hash,
        index,
    );
    match std::fs::read(&path) {
        Ok(bytes) => respond(stream, "200 OK", "image/png", &bytes),
        Err(_) => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"no thumbnail yet: submit a media.thumbnail job",
        ),
    }
}

/// Where a render lives, from `?timeline=<id>&render=<directory name>`.
///
/// Only a render directory of this project's own `derived/render/`, named as
/// renders are named: the query cannot name a path, so this cannot become a
/// file-read endpoint.
fn render_dir(runtime: &Runtime, query: &str) -> Result<PathBuf, &'static str> {
    let mut timeline = None;
    let mut render = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("timeline", v)) => timeline = ucg_core::EntityId::parse(v),
            Some(("render", v)) => render = Some(v.to_string()),
            _ => {}
        }
    }
    let timeline = timeline.ok_or("`timeline` must be an entity id")?;
    let render = render.ok_or("missing `render`")?;
    let plain = !render.is_empty()
        && !render.starts_with('.')
        && render
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain {
        return Err("`render` must be a render's directory name");
    }
    let dir = crate::render::render_root(&runtime.root_dir, timeline).join(render);
    if !dir.is_dir() {
        return Err("no such render");
    }
    Ok(dir)
}

/// One file of a render, streamed: `?timeline=&render=&file=` where `file` is
/// `manifest.json`, the movie, or a frame.
fn serve_render_file(stream: &mut TcpStream, runtime: &Runtime, query: &str) {
    let dir = match render_dir(runtime, query) {
        Ok(d) => d,
        Err(why) => {
            return respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                why.as_bytes(),
            )
        }
    };
    let file = query
        .split('&')
        .find_map(|p| p.strip_prefix("file="))
        .unwrap_or("");
    let is_frame = file.len() == "frame_000000.png".len()
        && file.starts_with("frame_")
        && file.ends_with(".png")
        && file[6..12].chars().all(|c| c.is_ascii_digit());
    let content_type = match file {
        "manifest.json" => "application/json",
        crate::render::MOVIE_FILE => "video/quicktime",
        _ if is_frame => "image/png",
        _ => {
            return respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"`file` must be manifest.json, movie.mov or a frame",
            )
        }
    };
    let path = dir.join(file);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"no such file in that render",
        );
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nCache-Control: no-cache\r\n"
    );
    if file == crate::render::MOVIE_FILE {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        head.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{name}.mov\"\r\n"
        ));
    }
    head.push_str("Connection: close\r\n\r\n");
    if stream.write_all(head.as_bytes()).is_ok() {
        // Streamed: a movie is not read into memory to be sent.
        let _ = std::io::copy(&mut f, stream);
        let _ = stream.flush();
    }
}

/// Picture `index` of a render as PNG: `?timeline=&render=&index=`. For frames
/// the index is the record frame; for a movie it is the movie's own picture,
/// decoded from the movie — what was delivered, not what the cut is now.
fn serve_render_frame(stream: &mut TcpStream, runtime: &Runtime, query: &str) {
    let dir = match render_dir(runtime, query) {
        Ok(d) => d,
        Err(why) => {
            return respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                why.as_bytes(),
            )
        }
    };
    let index: i64 = query
        .split('&')
        .find_map(|p| p.strip_prefix("index="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let movie = dir.join(crate::render::MOVIE_FILE);
    if movie.is_file() {
        let png = cine_media::Pictures::open_path(&movie)
            .map_err(|e| e.to_string())
            .and_then(|mut r| {
                r.frame_at_index(index.max(0) as usize)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("the movie has no picture {index}"))
            })
            .and_then(|f| {
                let (rgba, w, h, _, _) = crate::frames::rgba8(&f)?;
                crate::frames::png(&rgba, w, h)
            });
        return match png {
            Ok(bytes) => respond(stream, "200 OK", "image/png", &bytes),
            Err(e) => respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                e.as_bytes(),
            ),
        };
    }
    match std::fs::read(dir.join(crate::render::frame_file(index))) {
        Ok(bytes) => respond(stream, "200 OK", "image/png", &bytes),
        Err(_) => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"no such frame in that render",
        ),
    }
}

/// The project's view for one picture.
///
/// `clip` is what decides whether a grade applies. Without it this is the
/// media itself — the same file may back several clips with different grades,
/// so a media-only request is ungraded on purpose and any grade shown there
/// would be one clip's opinion presented as the file's. With it, that clip's
/// CDL and its layered grade are applied in the working space by the same
/// function a render uses, so a preview and a render cannot disagree.
///
/// A clip that does not play this media is refused rather than ignored:
/// applying one clip's grade to another's picture is the substitution the
/// resolution model exists to prevent.
///
/// An HDR display is named in the description together with the fact that the
/// browser cannot present it, because a PQ picture shown on an SDR surface is
/// not HDR however correct the transform was.
fn project_view_for(
    runtime: &Runtime,
    media: ucg_core::EntityId,
    clip: Option<ucg_core::EntityId>,
) -> Result<cine_color::view::ViewTransform, String> {
    let graph = runtime.project.engine.read();
    let (config, _) = cine_color::view::project_config(&graph, runtime.project.root)
        .ok_or("the project has no colour config to look through")?;
    let (display, view) = match runtime.project.session.view() {
        Some(v) => (v.display, v.view),
        None => cine_color::view::default_view(&config)
            .ok_or("the project's colour config declares no display and view")?,
    };
    let path = graph
        .node(media)
        .and_then(|n| n.get_str(ucg_schema::attrs::MEDIA_PATH))
        .ok_or("no media with that id in this project")?;
    let input = cine_color::view::input_colorspaces(&graph)
        .get(path)
        .cloned()
        .ok_or("this media has no input colorspace established, so it cannot be looked at through a view")?;
    let working = config.role("scene_linear").unwrap_or("ACEScg").to_string();
    // The clip's colour, read the same way `wanted_views` reads it for a
    // render: the CDL first, then the layered grade, both in the working
    // space. A clip bound to other media is not a grade for this picture.
    let (cdl, grade) = match clip {
        None => (None, None),
        Some(clip) => {
            match cine_timeline::graph::media_of_clip(&graph, clip) {
                Some(bound) if bound == media => {}
                Some(_) => {
                    return Err(
                        "that clip plays other media, so its grade is not this picture's"
                            .to_string(),
                    )
                }
                None => return Err("that clip is bound to no media asset".to_string()),
            }
            let cdl = cine_color::ucg::grade_of_clip(&graph, clip);
            let grade = cine_color::ucg::load_grade(
                &graph,
                ucg_core::EntityId::derived(clip, ucg_schema::kinds::COLOR_GRADE),
            )
            .ok()
            .filter(|g| !g.is_empty());
            (cdl, grade)
        }
    };
    let transform = cine_color::view::view_for_graded(
        &config,
        &input,
        &working,
        cdl.as_ref(),
        grade.as_ref(),
        &display,
        &view,
    )?;
    let hdr = display.contains("2100")
        || display.to_lowercase().contains("pq")
        || display.to_lowercase().contains("hlg");
    if hdr {
        // Honest rather than flattering: the transform is right, the surface
        // is not an HDR one, and saying so is the difference between a
        // reference picture and a pretty one.
        return Ok(match transform {
            cine_color::view::ViewTransform::Exact { processor, describe } => {
                cine_color::view::ViewTransform::Exact {
                    processor,
                    describe: format!(
                        "{describe} · shown on an SDR surface: these are {display} code values, not an HDR presentation"
                    ),
                }
            }
            other => other,
        });
    }
    Ok(transform)
}

/// One decoded picture, as a PNG.
///
/// `?media=<entity id>&index=<n>`. The media must already be registered in the
/// project: this serves the project's media, not an arbitrary path off the
/// filesystem, which is also what keeps it from becoming a file-read endpoint.
///
/// `&view=project` looks through the project's display and view. `&clip=<id>`
/// says which clip this picture stands for, and is what makes a grade apply;
/// it is only meaningful together with `view=project`, since a grade lives in
/// the working space that the view transform ends.
fn serve_frame(stream: &mut TcpStream, runtime: &Runtime, query: &str) {
    let mut media = None;
    let mut index = 0usize;
    let mut proxy = None;
    let mut through_view = false;
    let mut clip = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("media", v)) => media = ucg_core::EntityId::parse(v),
            Some(("index", v)) => index = v.parse().unwrap_or(0),
            Some(("proxy", v)) => proxy = ucg_core::EntityId::parse(v),
            Some(("view", v)) => through_view = v == "project",
            Some(("clip", v)) => clip = ucg_core::EntityId::parse(v),
            _ => {}
        }
    }
    let Some(media) = media else {
        respond(
            stream,
            "400 Bad Request",
            "text/plain; charset=utf-8",
            b"usage: /frame?media=<entity id>&index=<picture>",
        );
        return;
    };
    // The path comes from the graph, never from the request. A proxy is
    // served only when it is that media's own proxy and it is current, so a
    // stale stand-in can never be shown as the picture.
    let wanted = {
        let g = runtime.project.engine.read();
        // An original the project has recorded as not being where it says is
        // never decoded from that path. The file there may well open — a
        // namesake is a whole other take — and serving a picture out of it
        // would be the substitution the whole resolution model exists to
        // prevent. The project's record decides; hashing on every frame would
        // be the wrong price for an answer the project already has.
        match proxy {
            // One authoritative state decides, and says which of the eight
            // things is true. "No media with that id" used to be the answer
            // for a media that was in the project and had merely never been
            // probed, which is the one statement here that was false.
            None => {
                let state = crate::media_state::MediaState::of(&g, media);
                match state.why_not_readable() {
                    Some(why) => Err(why),
                    None => cine_media::ucg::read_media(&g, media)
                        .ok_or_else(|| "that media was never probed".to_string()),
                }
            }
            Some(p) if !crate::proxy::proxies_of(&g, media).contains(&p) => {
                Err("that proxy is not a proxy of that media".to_string())
            }
            Some(p) if crate::proxy::status(&g, p, false).state != "current" => {
                Err("that proxy is not current; ask for the original".to_string())
            }
            Some(p) => cine_media::ucg::read_media(&g, p)
                .ok_or_else(|| "that proxy was never probed".to_string()),
        }
    };
    let wanted = match wanted {
        Ok(p) => p,
        Err(why) => {
            respond(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                why.as_bytes(),
            );
            return;
        }
    };
    // The project's view, when asked for: built here from the same function a
    // render uses, so a preview and a render cannot disagree about colour.
    let view = if through_view {
        match project_view_for(runtime, media, clip) {
            Ok(v) => Some(v),
            Err(why) => {
                respond(
                    stream,
                    "422 Unprocessable Content",
                    "text/plain; charset=utf-8",
                    why.as_bytes(),
                );
                return;
            }
        }
    } else {
        None
    };
    // The camera's own name for this picture, worked out by the one
    // implementation of drop-frame labelling rather than by the interface. A
    // media with no timecode track gets no header, and the interface shows the
    // picture number, which is all anybody knows about it.
    let source_timecode = {
        let g = runtime.project.engine.read();
        cine_media::ucg::read_media(&g, media)
            .and_then(|i| i.labelling_timecode())
            .and_then(|t| t.plus_frames(index as i64).ok())
    };
    match runtime
        .frames
        .picture_through(&wanted, index, view.as_ref())
    {
        Ok(picture) => {
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\
                 Cache-Control: no-store\r\nX-Cine-Index: {}\r\nX-Cine-Decode-Ms: {:.1}\r\n\
                 X-Cine-Colour: {}\r\nX-Cine-Colour-Assumed: {}\r\n{}{}\r\n",
                picture.png.len(),
                picture.index,
                picture.decode_ms,
                crate::http::header_text(&picture.colour),
                picture.colour_assumed,
                // Which clip's colour this picture is, when it is a clip's at
                // all. Without it the interface would have to take on trust
                // that the grade it asked for is the grade it got.
                match clip.filter(|_| through_view) {
                    Some(c) => format!("X-Cine-Clip: {c}\r\n"),
                    None => String::new(),
                },
                match source_timecode {
                    Some(tc) => format!("X-Cine-Timecode: {tc}\r\n"),
                    None => String::new(),
                },
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&picture.png);
            let _ = stream.flush();
        }
        // A decoder that cannot decode says so. It does not return a blank
        // image, which would look like a black frame in the media.
        Err(why) => respond(
            stream,
            "422 Unprocessable Content",
            "text/plain; charset=utf-8",
            why.as_bytes(),
        ),
    }
}

fn handle_rpc_line(runtime: &Runtime, line: &str) -> Option<String> {
    let request = match Request::parse(line) {
        Ok(r) => r,
        Err(e) => return Some(rpc::error_response(&serde_json::Value::Null, -32700, &e)),
    };
    let id = request.id.clone()?;
    // Session facts the project itself does not know.
    let session = rpc::SessionFacts {
        clients: runtime.client_count(),
        uptime_s: runtime.uptime().as_secs(),
        waveforms: Some(&runtime.waveforms),
    };
    match rpc::dispatch_with(
        &runtime.project,
        &request.method,
        &request.params,
        Some(session),
    ) {
        Ok(result) => Some(rpc::ok_response(&id, result)),
        Err(e) => Some(rpc::error_response(&id, -32000, &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cine_pipeline::{OmniProject, ProjectSetup};

    fn runtime() -> Arc<Runtime> {
        let p = OmniProject::new(ProjectSetup::default()).unwrap();
        Runtime::new(p, PathBuf::from("/tmp/none"))
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let rt = runtime();
        assert!(handle_rpc_line(&rt, r#"{"jsonrpc":"2.0","method":"stats"}"#).is_none());
    }

    #[test]
    fn a_call_gets_a_reply_carrying_its_id() {
        let rt = runtime();
        let reply = handle_rpc_line(&rt, r#"{"jsonrpc":"2.0","id":11,"method":"stats"}"#).unwrap();
        assert!(reply.contains("\"id\":11") && reply.contains("revision"));
    }

    #[test]
    fn malformed_json_returns_a_parse_error_rather_than_dropping_the_connection() {
        let rt = runtime();
        let reply = handle_rpc_line(&rt, "{not json").unwrap();
        assert!(reply.contains("-32700"));
    }

    #[test]
    fn the_server_reports_the_port_it_actually_bound() {
        let rt = runtime();
        let server = Server::bind(
            rt,
            ServeConfig {
                port: 0,
                ..Default::default()
            },
        )
        .expect("bind");
        let addr = server.local_addr().unwrap();
        assert!(addr.port() > 0, "an OS-assigned port must be reported back");
        assert!(
            addr.ip().is_loopback(),
            "the runtime must not bind a public interface"
        );
    }

    /// A port already in use must not stop the runtime; it moves and reports.
    #[test]
    fn a_busy_preferred_port_falls_back_instead_of_failing() {
        let squatter = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = squatter.local_addr().unwrap().port();

        let server = Server::bind(
            runtime(),
            ServeConfig {
                port: busy,
                ..Default::default()
            },
        )
        .expect("bind should fall back rather than fail");
        assert_ne!(server.local_addr().unwrap().port(), busy);
    }
}
