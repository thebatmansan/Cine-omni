//! The CINE-OMNI reference viewer.
//!
//! ```text
//! cargo run -p cine-shell -- <media file>          # open a window
//! cargo run -p cine-shell -- <media file> --smoke  # present N frames, then exit
//! ```
//!
//! `--smoke` exists so the presentation path can be *verified* rather than
//! eyeballed: it drives the real window, the real surface and the real
//! swapchain for a fixed number of frames and exits non-zero if anything
//! failed. That is what lets CI assert the shell works on a machine nobody is
//! looking at.
//!
//! Frame stepping is by arrow key. There is no playback, because there is no
//! presentation clock — see `ShellCapabilities`.

use cine_gpu::device::GpuContext;
use cine_media::VideoReader;
use cine_shell::gpu::SurfaceContext;
use cine_shell::present::Presenter;
use cine_shell::viewer::FrameUploader;
use cine_shell::{Availability, ShellCapabilities};
use std::path::PathBuf;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

struct Args {
    media: Option<PathBuf>,
    smoke_frames: Option<u32>,
    /// Write the exact image the viewer presented, for inspection.
    dump: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut media = None;
    let mut smoke_frames = None;
    let mut dump = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--smoke" => smoke_frames = Some(it.next().and_then(|v| v.parse().ok()).unwrap_or(30)),
            "--dump" => dump = it.next().map(PathBuf::from),
            other if !other.starts_with("--") => media = Some(PathBuf::from(other)),
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    Args {
        media,
        smoke_frames,
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
    reader: Option<VideoReader>,
    uploader: FrameUploader,
    frame_index: usize,
    presented: u32,
    skipped: u32,
    failure: Option<String>,
}

impl App {
    fn new(args: Args) -> App {
        App {
            args,
            window: None,
            surface: None,
            presenter: None,
            compute: None,
            reader: None,
            uploader: FrameUploader::new(),
            frame_index: 0,
            presented: 0,
            skipped: 0,
            failure: None,
        }
    }

    /// Decode, convert and upload the frame at `self.frame_index`.
    fn show_frame(&mut self) {
        let (Some(reader), Some(compute), Some(surface), Some(presenter)) = (
            self.reader.as_mut(),
            self.compute.as_mut(),
            self.surface.as_ref(),
            self.presenter.as_mut(),
        ) else {
            return;
        };
        let frame = match reader.frame_at_index(self.frame_index) {
            Ok(Some(f)) => f,
            Ok(None) => return,
            Err(e) => {
                self.failure = Some(format!("decode failed: {e}"));
                return;
            }
        };
        let (w, h) = (frame.width, frame.height);
        match self.uploader.upload(compute, &frame) {
            Ok(rgba) => {
                if let Some(path) = self.args.dump.clone() {
                    if let Err(e) = write_png(&path, rgba, w, h) {
                        self.failure = Some(format!("dump failed: {e}"));
                    } else {
                        println!("wrote {} ({w}x{h})", path.display());
                    }
                }
                if let Err(e) = presenter.set_image(&surface.device, &surface.queue, rgba, w, h) {
                    self.failure = Some(format!("upload failed: {e}"));
                }
            }
            Err(e) => self.failure = Some(format!("colour conversion failed: {e}")),
        }
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
        let view = frame.texture.create_view(&Default::default());
        presenter.draw(&surface.device, &surface.queue, &view);
        // Presentation moved to the queue in wgpu 30.
        surface.queue.present(frame);
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
}

impl ApplicationHandler for App {
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
            match VideoReader::open_path(&path) {
                Ok(r) => {
                    println!("media: {} ({} frames)", path.display(), r.frame_count());
                    self.reader = Some(r);
                    self.show_frame();
                    if let Some(note) = self.uploader.colour_note() {
                        println!("colour: {note}");
                        if self.uploader.colour_is_assumed() {
                            println!("        ^ assumed, not declared by the file");
                        }
                    }
                }
                Err(e) => {
                    // A file we cannot decode is reported, not faked. The window
                    // still opens so the failure is visible in context.
                    eprintln!("cannot open {}: {e}", path.display());
                }
            }
        }

        for (name, availability) in ShellCapabilities::all() {
            if let Availability::Unavailable(why) = availability {
                println!("unavailable — {name}: {why}");
            }
        }

        window.request_redraw();
        self.window = Some(window);
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
                let step: i64 = match logical_key {
                    Key::Named(NamedKey::ArrowRight) => 1,
                    Key::Named(NamedKey::ArrowLeft) => -1,
                    Key::Named(NamedKey::Escape) => {
                        event_loop.exit();
                        return;
                    }
                    _ => return,
                };
                let count = self.reader.as_ref().map(|r| r.frame_count()).unwrap_or(0);
                if count == 0 {
                    return;
                }
                let next = (self.frame_index as i64 + step).clamp(0, count as i64 - 1);
                if next as usize != self.frame_index {
                    self.frame_index = next as usize;
                    self.show_frame();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let args = parse_args();
    let smoke = args.smoke_frames.is_some();

    let event_loop = match EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("no display available: {e}");
            std::process::exit(2);
        }
    };
    // Smoke mode drives frames as fast as it can; interactive mode waits for
    // input, because a viewer with no playback has nothing to redraw for.
    event_loop.set_control_flow(if smoke {
        ControlFlow::Poll
    } else {
        ControlFlow::Wait
    });

    let mut app = App::new(args);
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
