//! The CINE-OMNI reference viewer.
//!
//! ```text
//! cargo run -p cine-shell -- <media file>            # open a window
//! cargo run -p cine-shell -- <media file> --smoke 30 # present N frames, exit
//! cargo run -p cine-shell -- <media file> --play 5   # play N seconds, report
//! ```
//!
//! ## What drives the picture
//!
//! `cine_media::Scheduler` over `cine_media::DecodeAhead`. Decoding happens on
//! a worker thread, ahead of the playhead, into a bounded buffer. The event loop
//! does not tick at a frame rate: it sleeps until the scheduler says the picture
//! on screen has expired, takes whatever the clock says is due *then* from the
//! buffer, and puts it on the surface. When the buffer does not have it, the
//! transport reports an underrun and the loop sleeps until the worker wakes it —
//! through a winit user event, not a timer and not a spin.
//!
//! Starting playback pre-rolls: the clock is not started until a cushion of
//! pictures is decoded, and how long that took is printed. It is a visible,
//! measured step, not a silent delay.
//!
//! `--smoke` verifies the presentation path without anyone watching: it drives
//! the real window, surface and swapchain for a fixed number of frames and
//! exits non-zero if anything failed.
//!
//! `--play` is the evidence mode: it plays for a wall-clock duration and prints
//! the scheduler's counters — presented, dropped, late, underruns — the
//! decode-ahead worker's, and the time spent in each presentation stage. Those
//! numbers, not an adjective, are what this build claims.
//!
//! ## Transport
//!
//! `space` play/pause · `J`/`K`/`L` reverse/pause/forward · `←`/`→` step one
//! frame · `home` stop · `esc` quit.

use cine_gpu::device::GpuContext;
use cine_media::clock::{PlaybackClock, PlaybackRate, SystemTime, TransportState};
use cine_media::decode_ahead::{AheadConfig, DecodeAhead};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::{VideoFrame, VideoReader};
use cine_shell::gpu::SurfaceContext;
use cine_shell::present::Presenter;
use cine_shell::viewer::FrameUploader;
use cine_shell::ShellCapabilities;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

type Playback = Scheduler<SystemTime, DecodeAhead<VideoReader>>;

/// Sent by the decode-ahead worker when a picture lands, so a starved loop wakes
/// exactly when there is something to show.
#[derive(Debug)]
struct Wake;

/// Pictures decoded before the clock starts. One second at 24 fps: the measured
/// decode deficit on the reference asset is about 2 pictures per second of
/// playback, so this covers a ten-second clip. It is a starting point to be
/// re-measured, not a tuned constant.
const PREROLL_PICTURES: usize = 24;
/// Pre-roll never blocks the transport for longer than this; it starts anyway
/// and the underrun counters say what that cost.
const PREROLL_LIMIT: Duration = Duration::from_secs(10);

/// Wall time spent in each stage of getting a decoded picture onto the screen.
#[derive(Default)]
struct Stages {
    pictures: u32,
    /// Getting the decoded planes onto the GPU. The conversion itself now
    /// happens in the fragment shader, so it is part of the draw.
    to_gpu: Duration,
    to_gpu_worst: Duration,
    /// Surface draws: blit, convert and queue present.
    draws: u32,
    draw: Duration,
}

struct Preroll {
    from: usize,
    want: usize,
    began: Instant,
}

struct Args {
    media: Option<PathBuf>,
    smoke_frames: Option<u32>,
    /// Play for this many seconds, then report and exit.
    play_secs: Option<f64>,
    /// Write the exact image the viewer presented, for inspection.
    dump: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut media = None;
    let mut smoke_frames = None;
    let mut play_secs = None;
    let mut dump = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--smoke" => smoke_frames = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(30)),
            "--play" => play_secs = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(5.0)),
            "--dump" => dump = it.next().map(PathBuf::from),
            other if !other.starts_with("--") => media = Some(PathBuf::from(other)),
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    Args {
        media,
        smoke_frames,
        play_secs,
        dump,
    }
}

/// Write the presented RGBA as an 8-bit PNG.
///
/// The viewer works in float; this encodes for inspection only. Values are
/// clamped, because out-of-range results are a real possibility with a wrong
/// matrix and clamping here would otherwise hide them — so anything clipped is
/// reported.
fn write_png(path: &PathBuf, rgba: &[f32], width: u32, height: u32) -> std::io::Result<()> {
    let mut clipped = 0usize;
    let mut bytes = Vec::with_capacity(rgba.len());
    for &v in rgba {
        if !(0.0..=1.0).contains(&v) {
            clipped += 1;
        }
        bytes.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    if clipped > 0 {
        println!(
            "note: {clipped} of {} samples fell outside 0..1 and were clamped for the PNG",
            rgba.len()
        );
    }
    image::save_buffer(path, &bytes, width, height, image::ColorType::Rgba8)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

struct App {
    args: Args,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceContext>,
    presenter: Option<Presenter>,
    compute: Option<GpuContext>,
    playback: Option<Playback>,
    uploader: FrameUploader,
    /// How the picture is being interpreted, and whether that was assumed.
    colour: Option<(String, bool)>,
    /// Surface presentations, which is not the same as pictures presented: the
    /// window may redraw the same picture several times.
    presented: u32,
    skipped: u32,
    play_until: Option<Instant>,
    failure: Option<String>,
    /// The last thing the transport did, printed rather than guessed at.
    last_report: Option<String>,
    proxy: EventLoopProxy<Wake>,
    stages: Stages,
    preroll: Option<Preroll>,
    preroll_took: Option<(usize, Duration)>,
    /// Evidence mode reached the end of the media; stop at the next chance.
    finished: bool,
}

impl App {
    fn new(args: Args, proxy: EventLoopProxy<Wake>) -> App {
        App {
            args,
            window: None,
            surface: None,
            presenter: None,
            compute: None,
            playback: None,
            uploader: FrameUploader::new(),
            colour: None,
            presented: 0,
            skipped: 0,
            play_until: None,
            failure: None,
            last_report: None,
            proxy,
            stages: Stages::default(),
            preroll: None,
            preroll_took: None,
            finished: false,
        }
    }

    /// Put one decoded picture on the GPU as its planes.
    ///
    /// The conversion happens in the fragment shader at draw time. `--dump`
    /// additionally runs `cine-gpu`'s compute kernel, because that is the path
    /// verified against a CPU reference and the dumped PNG should be what *it*
    /// produced.
    fn upload(&mut self, frame: &VideoFrame) {
        let (Some(surface), Some(presenter)) = (self.surface.as_ref(), self.presenter.as_mut())
        else {
            return;
        };
        let conversion = match cine_shell::viewer::conversion_for(frame) {
            Ok(c) => c,
            Err(e) => {
                self.failure = Some(format!("cannot interpret this picture: {e}"));
                return;
            }
        };
        if self.colour.is_none() {
            self.colour = Some((
                format!(
                    "{} · {}",
                    conversion.choice.describe(),
                    match conversion.range {
                        cine_gpu::yuv::Range::Limited => "limited range",
                        cine_gpu::yuv::Range::Full => "full range",
                    }
                ),
                conversion.choice.is_assumed(),
            ));
        }
        let (Some(y), Some(u), Some(v)) = (frame.plane(0), frame.plane(1), frame.plane(2)) else {
            self.failure = Some("decoded picture is missing a plane".into());
            return;
        };
        let moving = Instant::now();
        let sent = presenter.set_planes(&surface.device, &surface.queue, &conversion, [y, u, v]);
        let took = moving.elapsed();
        self.stages.pictures += 1;
        self.stages.to_gpu += took;
        self.stages.to_gpu_worst = self.stages.to_gpu_worst.max(took);
        if let Err(e) = sent {
            self.failure = Some(format!("uploading planes failed: {e}"));
            return;
        }
        if let Some(path) = self.args.dump.clone() {
            let (w, h) = (frame.width, frame.height);
            let Some(compute) = self.compute.as_mut() else {
                return;
            };
            match self.uploader.upload(compute, frame) {
                Ok(rgba) => match write_png(&path, rgba, w, h) {
                    Ok(()) => println!("wrote {} ({w}x{h})", path.display()),
                    Err(e) => self.failure = Some(format!("dump failed: {e}")),
                },
                Err(e) => self.failure = Some(format!("colour conversion failed: {e}")),
            }
        }
    }

    /// One turn of the scheduler. Returns whether the window needs redrawing.
    ///
    /// The scheduler borrow is closed before anything else on `self` is
    /// touched, so a decoded picture is owned by the time it is uploaded.
    fn pump(&mut self) -> bool {
        let Some(outcome) = self.playback.as_mut().map(|p| p.poll()) else {
            return false;
        };
        match outcome {
            Presentation::Present(p) => {
                let first = self.colour.is_none();
                if p.dropped > 0 || p.late_by.is_some() {
                    self.last_report = Some(format!(
                        "frame {} — {} dropped, {}decode {:?}",
                        p.index,
                        p.dropped,
                        match p.late_by {
                            Some(l) => format!("late by {l:?}, "),
                            None => String::new(),
                        },
                        p.decode_took
                    ));
                }
                self.upload(&p.frame);
                if first {
                    if let Some((note, assumed)) = &self.colour {
                        println!("colour: {note}");
                        if *assumed {
                            println!("        ^ assumed, not declared by the file");
                        }
                    }
                }
                true
            }
            Presentation::Hold => false,
            Presentation::EndOfMedia => {
                println!("end of media");
                // In evidence mode the run is over: reporting here and again
                // when the clock runs out printed everything twice.
                if self.args.play_secs.is_some() {
                    self.finished = true;
                } else {
                    self.report();
                }
                false
            }
            Presentation::BeforeStart => false,
            // Counted by the scheduler once per stretch and reported at the end;
            // printing per starved poll would measure the poll rate.
            Presentation::Underrun => false,
            Presentation::Failed(why) => {
                // A decode failure is shown, not substituted with a blank frame.
                eprintln!("playback stopped: {why}");
                self.failure = Some(why);
                false
            }
            Presentation::Unsupported => {
                eprintln!("this build cannot play this media");
                false
            }
        }
    }

    /// Print exactly what the scheduler counted. No adjectives.
    fn report(&self) {
        let Some(p) = self.playback.as_ref() else {
            return;
        };
        let s = p.stats();
        println!(
            "transport {:?} — presented {}, dropped {}, late {}, held {} over {} polls",
            p.state(),
            s.presented,
            s.dropped,
            s.late,
            s.held,
            s.polls
        );
        if s.presented > 0 {
            println!(
                "decode: mean {:?}, worst {:?}; drop rate {:.1}%",
                s.mean_decode().unwrap_or_default(),
                Duration::from_nanos(s.decode_worst_nanos.min(u128::from(u64::MAX)) as u64),
                s.drop_rate() * 100.0
            );
        }
        println!(
            "underruns: {} episodes over {} starved polls",
            s.underrun_episodes, s.starved_polls
        );
        if let Some((n, took)) = self.preroll_took {
            println!("preroll: {n} pictures in {took:?}");
        }
        let a = p.source().stats();
        println!(
            "decode-ahead: {} decoded (mean {:?}, worst {:?}), {} stale, {} evicted, \
             {} hits / {} misses, peak {:.1} MiB",
            a.decoded,
            a.mean_decode().unwrap_or_default(),
            Duration::from_nanos(a.decode_worst_nanos.min(u128::from(u64::MAX)) as u64),
            a.stale_discarded,
            a.evicted,
            a.hits,
            a.misses,
            a.peak_bytes as f64 / (1024.0 * 1024.0)
        );
        let st = &self.stages;
        if st.pictures > 0 {
            println!(
                "stages: planes to GPU {:?} per picture (worst {:?}); \
                 draw, including conversion, {:?} over {} draws",
                st.to_gpu / st.pictures,
                st.to_gpu_worst,
                st.draw.checked_div(st.draws).unwrap_or_default(),
                st.draws
            );
        }
        if let Some(note) = &self.last_report {
            println!("last irregular frame: {note}");
        }
    }

    /// Begin playing in a direction, after pre-rolling a cushion of pictures.
    ///
    /// Does not block: the clock starts from `about_to_wait` once the worker
    /// has decoded enough, and the worker's notification is what wakes it.
    fn start_play(&mut self, reverse: bool) {
        let Some(p) = self.playback.as_mut() else {
            return;
        };
        let rate = if reverse {
            PlaybackRate::new(-1, 1)
        } else {
            Some(PlaybackRate::FORWARD)
        };
        if let Some(r) = rate {
            let _ = p.clock_mut().set_rate(r);
        }
        let count = p.source().frame_count();
        if count == 0 {
            return;
        }
        let mut from = p
            .on_screen()
            .or_else(|| p.source().index_at(p.position()))
            .unwrap_or(0);
        // At the far end in the direction of travel, go round to the other end
        // first, as the clock itself would — and pre-roll *there*.
        if !reverse && from + 1 >= count {
            from = 0;
        } else if reverse && from == 0 {
            from = count - 1;
        }
        if let Some(pts) = p.source().pts_of(from) {
            p.seek(pts);
        }
        p.source_mut().prepare(from, reverse);
        let remaining = if reverse { from + 1 } else { count - from };
        self.preroll = Some(Preroll {
            from,
            want: PREROLL_PICTURES.min(remaining),
            began: Instant::now(),
        });
    }

    /// Start the clock if pre-roll is satisfied or has run out of patience.
    /// Returns whether pre-roll is still in progress.
    fn advance_preroll(&mut self) -> bool {
        let (Some(pre), Some(p)) = (self.preroll.as_ref(), self.playback.as_mut()) else {
            return false;
        };
        let have = p.source().buffered_run(pre.from);
        let waited = pre.began.elapsed();
        if have < pre.want && waited < PREROLL_LIMIT {
            return true;
        }
        self.preroll_took = Some((have, waited));
        if have < pre.want {
            println!(
                "preroll gave up after {waited:?} with {have} of {} pictures",
                pre.want
            );
        }
        if let Err(state) = p.clock_mut().play() {
            eprintln!("cannot play: transport is {}", state.label());
        }
        self.preroll = None;
        if let Some(secs) = self.args.play_secs {
            self.play_until = Some(Instant::now() + Duration::from_secs_f64(secs));
        }
        false
    }

    fn transport(&mut self, key: &Key) -> bool {
        let Some(p) = self.playback.as_mut() else {
            return false;
        };
        let playing = p.state().is_advancing() || self.preroll.is_some();
        let reverse = p.clock().rate().is_reverse();
        match key {
            Key::Named(NamedKey::Space) => {
                if playing {
                    p.clock_mut().pause();
                    self.preroll = None;
                } else {
                    self.start_play(reverse);
                }
            }
            Key::Character(c) if c.eq_ignore_ascii_case("l") => self.start_play(false),
            Key::Character(c) if c.eq_ignore_ascii_case("j") => self.start_play(true),
            Key::Character(c) if c.eq_ignore_ascii_case("k") => {
                p.clock_mut().pause();
                self.preroll = None;
            }
            Key::Named(NamedKey::Home) => {
                p.clock_mut().stop();
                self.preroll = None;
            }
            Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::ArrowLeft) => {
                // By picture, not by clock tick: on this asset a tick of the
                // time base is 1/12288 s and never changed the picture.
                let step = if matches!(key, Key::Named(NamedKey::ArrowRight)) {
                    1
                } else {
                    -1
                };
                self.preroll = None;
                let _ = p.step(step);
            }
            _ => return false,
        }
        true
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(surface), Some(presenter)) = (self.surface.as_mut(), self.presenter.as_ref())
        else {
            return;
        };
        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match surface.surface.get_current_texture() {
            Acquired::Success(f) | Acquired::Suboptimal(f) => f,
            // Recoverable: reconfigure and try again next redraw. These are
            // ordinary window-manager events, not failures.
            Acquired::Lost | Acquired::Outdated => {
                let (w, h) = (surface.config.width, surface.config.height);
                surface.resize(w, h);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                return;
            }
            Acquired::Timeout | Acquired::Occluded => {
                // Skip this frame. In smoke mode an occluded window would
                // otherwise spin forever without ever presenting.
                if self.args.smoke_frames.is_some() {
                    self.skipped += 1;
                    if self.skipped > 600 {
                        self.failure =
                            Some("surface never became presentable (timeout/occluded)".into());
                        event_loop.exit();
                    } else if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                return;
            }
            Acquired::Validation => {
                self.failure = Some("surface validation error".into());
                event_loop.exit();
                return;
            }
        };
        let drawing = Instant::now();
        let view = frame.texture.create_view(&Default::default());
        presenter.draw(&surface.device, &surface.queue, &view);
        // Presentation moved to the queue in wgpu 30.
        surface.queue.present(frame);
        self.stages.draw += drawing.elapsed();
        self.stages.draws += 1;
        self.presented += 1;

        if let Some(limit) = self.args.smoke_frames {
            if self.presented >= limit {
                println!("SMOKE OK: presented {} frames", self.presented);
                event_loop.exit();
            } else if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    /// Open the media and build the transport over it.
    fn open(&mut self, path: &Path) {
        match VideoReader::open_path(path) {
            Ok(reader) => {
                println!(
                    "media: {} ({} pictures, {} sync samples, time base {:?})",
                    path.display(),
                    reader.frame_count(),
                    reader.keyframe_count(),
                    reader.time_base()
                );
                let Some((start, end)) = reader.extent() else {
                    eprintln!("{}: the track has no samples", path.display());
                    return;
                };
                println!("extent: {start:?} .. {end:?}");
                println!(
                    "keyframes at display indices {:?}",
                    reader.keyframe_indices()
                );
                let clock = PlaybackClock::new(SystemTime::default(), start, end);
                let ahead = DecodeAhead::new(reader, AheadConfig::default());
                // The worker wakes the event loop when a picture lands. A proxy
                // is `Send` but not `Sync`, hence the mutex.
                let proxy = Mutex::new(self.proxy.clone());
                ahead.set_notify(move || {
                    if let Ok(p) = proxy.lock() {
                        let _ = p.send_event(Wake);
                    }
                });
                let mut playback = Scheduler::new(clock, ahead);
                // Ask for the first picture now; it is shown when the worker
                // delivers it, which wakes the loop. An empty window with a
                // transport on it says nothing about whether decode works.
                playback.poll();
                self.playback = Some(playback);
            }
            Err(e) => {
                // A file we cannot decode is reported, not faked. The window
                // still opens so the failure is visible in context, and no
                // transport is built — so every transport key is inert rather
                // than appearing to work.
                //
                // `PlaybackClock::unsupported` exists for exactly this case and
                // is not used here, because `Scheduler` is generic over a
                // `FrameSource` and there is no reader to be generic over when
                // the open failed. Giving the shell a source-less refusing
                // transport needs a boxed `FrameSource`, which is a change to
                // make when the viewer has a transport bar to grey out.
                eprintln!("cannot open {}: {e}", path.display());
            }
        }
    }
}

impl ApplicationHandler<Wake> for App {
    /// A decoded picture landed. Nothing to do here: `about_to_wait` runs next
    /// and takes it if it is due.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wake) {}

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("CINE-OMNI — reference viewer")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.failure = Some(format!("could not create a window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let surface = match SurfaceContext::new(window.clone(), size.width, size.height) {
            Ok(s) => s,
            Err(e) => {
                self.failure = Some(format!("could not create a surface: {e}"));
                event_loop.exit();
                return;
            }
        };
        println!("adapter: {}", surface.adapter_name());
        println!(
            "surface: {:?} {}x{}",
            surface.config.format, size.width, size.height
        );

        self.presenter = Some(Presenter::new(&surface.device, surface.config.format));
        self.surface = Some(surface);

        // The compute context is separate today: `GpuContext` owns its own
        // device. Sharing one device between compute and presentation is the
        // stated architecture and the next step; it is not yet done, and the
        // cost is an extra device rather than a wrong result.
        match GpuContext::new() {
            Ok(c) => self.compute = Some(c),
            Err(e) => {
                self.failure = Some(format!("no compute adapter: {e}"));
                event_loop.exit();
                return;
            }
        }

        if let Some(path) = self.args.media.clone() {
            self.open(&path);
        }

        for (name, availability) in ShellCapabilities::all() {
            match availability {
                cine_shell::Availability::Working => {}
                cine_shell::Availability::Degraded(why) => println!("degraded — {name}: {why}"),
                cine_shell::Availability::Unavailable(why) => {
                    println!("unavailable — {name}: {why}")
                }
            }
        }

        // Evidence mode: pre-roll, then play for the requested time. The clock
        // for `play_until` starts when playback does, not when pre-roll does.
        if let Some(secs) = self.args.play_secs {
            if self.playback.is_some() {
                println!("playing for {secs} s after pre-rolling {PREROLL_PICTURES} pictures");
                self.start_play(false);
            }
        }

        window.request_redraw();
        self.window = Some(window);
    }

    /// Where playback actually happens: the loop is woken by the scheduler's
    /// deadline, not by a timer.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.finished || self.play_until.is_some_and(|until| Instant::now() >= until) {
            self.report();
            event_loop.exit();
            return;
        }
        if self.advance_preroll() {
            // Still pre-rolling: the worker's notification wakes us.
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        // Pump even when paused: after a step, a seek or opening the file the
        // due picture may have just arrived from the worker. When nothing has
        // changed this is a `Hold` and costs nothing.
        if self.pump() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }

        // Smoke mode drives surface frames as fast as it can, but only after
        // pumping: with decode-ahead the first picture arrives from the worker,
        // and returning early here meant `--dump` silently wrote nothing.
        if self.args.smoke_frames.is_some() {
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }
        let Some(p) = self.playback.as_ref() else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        event_loop.set_control_flow(match p.next_deadline() {
            Some(d) => ControlFlow::WaitUntil(Instant::now() + d),
            // Paused: nothing to wake for. Starved: the worker wakes us. A viewer
            // that polls in either case burns a core to show a still frame.
            None => ControlFlow::Wait,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(s) = self.surface.as_mut() {
                    s.resize(size.width, size.height);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key,
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if matches!(logical_key, Key::Named(NamedKey::Escape)) {
                    self.report();
                    event_loop.exit();
                    return;
                }
                if !self.transport(&logical_key) {
                    return;
                }
                let state = self.playback.as_ref().map(|p| p.state());
                if let Some(s) = state {
                    println!("transport: {}", s.label());
                }
                // A paused transport still has to redraw after a step or a
                // seek: nothing else is going to wake the loop.
                if self.pump() {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                if state.is_some_and(|s| s != TransportState::Unsupported) {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let args = parse_args();
    let smoke = args.smoke_frames.is_some();

    let event_loop = match EventLoop::<Wake>::with_user_event().build() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("no display available: {e}");
            std::process::exit(2);
        }
    };
    // Smoke mode drives surface frames as fast as it can. Everything else is
    // driven by the scheduler through `about_to_wait`.
    event_loop.set_control_flow(if smoke {
        ControlFlow::Poll
    } else {
        ControlFlow::Wait
    });

    let proxy = event_loop.create_proxy();
    let mut app = App::new(args, proxy);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop failed: {e}");
        std::process::exit(1);
    }
    if let Some(failure) = app.failure {
        eprintln!("FAILED: {failure}");
        std::process::exit(1);
    }
    if smoke && app.presented == 0 {
        eprintln!("FAILED: no frames were presented");
        std::process::exit(1);
    }
}
