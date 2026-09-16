//! The application window: the reference viewer and, inside `cine start`, the
//! project's native surface.
//!
//! Moved out of `main.rs` so the same event loop can run from two places: the
//! standalone reference viewer (`cine-shell <media>`), and `cine start`, where
//! it is the picture-critical surface of the one application and plays the
//! project's cut through a [`crate::link::ProjectLink`].

use crate::egui_layer::EguiLayer;
use crate::gpu::SurfaceContext;
use crate::link::{ProjectLink, SequencePlan, Viewing};
use crate::present::Presenter;
use crate::sound::{AppTime, SoundEngine};
use crate::viewer::FrameUploader;
use crate::workspace::{Command, ViewerStatus, Workspace};
use crate::ShellCapabilities;
use cine_gpu::device::GpuContext;
use cine_media::clock::{PlaybackClock, PlaybackRate, TransportState};
use cine_media::decode_ahead::{AheadConfig, DecodeAhead};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::{EditListSource, VideoFrame, VideoReader};
use cine_timeline::session::TrackState;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

type Playback = Scheduler<AppTime, DecodeAhead<AnySource>>;

/// What the window plays: one file (the reference viewer), or the project's cut.
///
/// An enum rather than a boxed trait object so the decode-ahead worker keeps a
/// concrete, `Send` source and nothing is dispatched dynamically per picture.
pub enum AnySource {
    File(Box<VideoReader>),
    Sequence(EditListSource),
}

impl FrameSource for AnySource {
    fn frame_count(&self) -> usize {
        match self {
            AnySource::File(r) => FrameSource::frame_count(r.as_ref()),
            AnySource::Sequence(s) => s.frame_count(),
        }
    }
    fn pts_of(&self, index: usize) -> Option<ucg_core::RationalTime> {
        match self {
            AnySource::File(r) => FrameSource::pts_of(r.as_ref(), index),
            AnySource::Sequence(s) => s.pts_of(index),
        }
    }
    fn index_at(&self, t: ucg_core::RationalTime) -> Option<usize> {
        match self {
            AnySource::File(r) => FrameSource::index_at(r.as_ref(), t),
            AnySource::Sequence(s) => s.index_at(t),
        }
    }
    fn extent(&self) -> Option<(ucg_core::RationalTime, ucg_core::RationalTime)> {
        match self {
            AnySource::File(r) => FrameSource::extent(r.as_ref()),
            AnySource::Sequence(s) => s.extent(),
        }
    }
    fn decode_index(&mut self, index: usize) -> cine_media::Result<Option<VideoFrame>> {
        match self {
            AnySource::File(r) => FrameSource::decode_index(r.as_mut(), index),
            AnySource::Sequence(s) => s.decode_index(index),
        }
    }
    fn keyframe_at_or_before(&self, index: usize) -> Option<usize> {
        match self {
            AnySource::File(r) => FrameSource::keyframe_at_or_before(r.as_ref(), index),
            AnySource::Sequence(_) => None,
        }
    }
    fn supports_reverse(&self) -> bool {
        true
    }
}

/// Sent by the decode-ahead worker when a picture lands, so a starved loop wakes
/// exactly when there is something to show.
#[derive(Debug)]
enum Wake {
    /// A decoded picture landed.
    Picture,
    /// Someone committed to the project, or the application is quitting.
    Project,
}

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

/// How the window was launched, and what it should do.
#[derive(Default)]
pub struct ShellOptions {
    pub media: Option<PathBuf>,
    pub smoke_frames: Option<u32>,
    /// Play for this many seconds, then report and exit.
    pub play_secs: Option<f64>,
    /// Draw the workspace for this many frames, then report and exit.
    ///
    /// The equivalent of `--smoke` for the interface: it drives the real
    /// window, the real surface and the real egui pass, so "the workspace
    /// renders" is something a machine can check rather than something someone
    /// remembers seeing.
    pub ui_frames: Option<u32>,
    /// Keep the workspace up during `--play`, so the cost of drawing the
    /// interface is part of the measurement rather than excluded from it.
    pub with_ui: bool,
    /// Write the exact image the viewer presented, for inspection.
    pub dump: Option<PathBuf>,
    /// An OTIO file to show in the timeline pane.
    pub timeline: Option<PathBuf>,
    /// The project this window is a surface of, inside `cine start`.
    pub link: Option<Box<dyn ProjectLink>>,
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

/// Fit a picture inside a pane without distorting it.
///
/// The blit used to stretch the image to the window, which is wrong for a
/// reference viewer: an anamorphic frame would have looked correct and been a
/// lie. Returns a viewport in physical pixels.
fn letterbox(pane: egui::Rect, pixels_per_point: f32, picture: Option<(u32, u32)>) -> [f32; 4] {
    let x = pane.min.x * pixels_per_point;
    let y = pane.min.y * pixels_per_point;
    let w = pane.width() * pixels_per_point;
    let h = pane.height() * pixels_per_point;
    let Some((pw, ph)) = picture else {
        return [x, y, w, h];
    };
    if pw == 0 || ph == 0 || w <= 0.0 || h <= 0.0 {
        return [x, y, w, h];
    }
    let aspect = pw as f32 / ph as f32;
    let (fit_w, fit_h) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    [x + (w - fit_w) * 0.5, y + (h - fit_h) * 0.5, fit_w, fit_h]
}

struct App {
    args: ShellOptions,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceContext>,
    presenter: Option<Presenter>,
    compute: Option<GpuContext>,
    playback: Option<Playback>,
    uploader: FrameUploader,
    /// How the picture is being interpreted, and whether that was assumed.
    colour: Option<(String, bool)>,
    keyframes: Vec<usize>,
    /// Surface presentations, which is not the same as pictures presented: the
    /// window may redraw the same picture several times.
    presented: u32,
    skipped: u32,
    play_until: Option<Instant>,
    failure: Option<String>,
    /// The last thing the transport did, printed rather than guessed at.
    last_report: Option<String>,
    proxy: EventLoopProxy<Wake>,
    /// The editorial workspace. Absent in the evidence modes, which measure the
    /// presentation path and must not end up measuring the interface too.
    egui: Option<EguiLayer>,
    workspace: Workspace,
    /// Where the picture goes within the surface, in physical pixels.
    picture_rect: Option<[f32; 4]>,
    /// Dimensions of the decoded picture, for letterboxing.
    picture_size: Option<(u32, u32)>,
    /// What the interface asked for during the last draw, acted on afterwards:
    /// the interface is drawn while several of `App`'s fields are borrowed.
    pending_commands: Vec<Command>,
    /// Workspace frames drawn, for `--ui-smoke`.
    ui_drawn: u32,
    /// When egui next needs painting. `MAX` means "not until something happens".
    egui_repaint_in: Duration,
    /// The cut shown in the timeline pane.
    timeline: Option<cine_timeline::Timeline>,
    /// The panels need rebuilding: input arrived, or something they show
    /// changed. Otherwise the cached interface is re-rendered over the new
    /// picture, which costs a fraction of running it again.
    ui_dirty: bool,
    /// When the panels were last rebuilt, so they still refresh while playing.
    ui_built: Option<Instant>,
    stages: Stages,
    preroll: Option<Preroll>,
    preroll_took: Option<(usize, Duration)>,
    /// Evidence mode reached the end of the media; stop at the next chance.
    finished: bool,
    /// The sequence being played, when this window is a project surface.
    sequence: Option<SequencePlan>,
    /// How the pictures must be shown: the project's colour config, compiled
    /// and baked by the runtime. `None` means they are shown as decoded.
    viewing: Option<Viewing>,
    /// Scopes of the picture on screen, measured when the panels are rebuilt
    /// rather than per picture: a scope is for looking at, and a viewer that
    /// dropped frames to draw one would be measuring the wrong thing.
    scopes: Option<Arc<crate::scopes::Scopes>>,
    /// The picture last put on the GPU, and how: what the scopes measure.
    /// Planes are `Arc`-shared, so keeping it costs a pointer, not a frame.
    last_picture: Option<(
        VideoFrame,
        cine_gpu::yuv::YuvConversion,
        Option<Arc<cine_color::lut::Lut3D>>,
    )>,
    /// The sequence's sound on the device, when there is one.
    sound: Option<SoundEngine>,
}

impl App {
    fn new(args: ShellOptions, proxy: EventLoopProxy<Wake>) -> App {
        App {
            args,
            window: None,
            surface: None,
            presenter: None,
            compute: None,
            playback: None,
            uploader: FrameUploader::new(),
            colour: None,
            keyframes: Vec::new(),
            presented: 0,
            skipped: 0,
            play_until: None,
            failure: None,
            last_report: None,
            proxy,
            egui: None,
            workspace: Workspace::new(),
            picture_rect: None,
            picture_size: None,
            pending_commands: Vec::new(),
            ui_drawn: 0,
            egui_repaint_in: Duration::MAX,
            timeline: None,
            ui_dirty: true,
            ui_built: None,
            stages: Stages::default(),
            preroll: None,
            preroll_took: None,
            finished: false,
            sequence: None,
            viewing: None,
            scopes: None,
            last_picture: None,
            sound: None,
        }
    }

    /// Put one decoded picture on the GPU as its planes.
    ///
    /// The conversion happens in the fragment shader at draw time. `--dump`
    /// additionally runs `cine-gpu`'s compute kernel, because that is the path
    /// verified against a CPU reference and the dumped PNG should be what *it*
    /// produced.
    fn upload(&mut self, frame: &VideoFrame) {
        // Which clip this picture came from decides how it must be shown: the
        // sequence's pictures are presented at sequence time, and the plan says
        // what plays when. A clip with no colorspace gets no transform, and the
        // window shows it as decoded rather than through someone else's.
        let lut = self.lut_for(frame.pts);
        let (Some(surface), Some(presenter)) = (self.surface.as_ref(), self.presenter.as_mut())
        else {
            return;
        };
        let conversion = match crate::viewer::conversion_for(frame) {
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
        self.picture_size = Some((conversion.width, conversion.height));
        self.last_picture = Some((frame.clone(), conversion, lut.clone()));
        if let Err(e) = presenter.set_view_lut(&surface.device, &surface.queue, lut.as_ref()) {
            self.failure = Some(format!("the view transform could not be uploaded: {e}"));
            return;
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

    /// Measure the picture on screen, for the scopes pane.
    ///
    /// Done while the panels are being rebuilt — a few times a second — and
    /// not per picture: playback's budget belongs to playback.
    fn measure_scopes(&mut self) {
        let Some((frame, conversion, lut)) = self.last_picture.as_ref() else {
            return;
        };
        self.scopes = crate::scopes::of_frame(
            frame,
            conversion,
            lut.as_ref(),
            crate::scopes::Sampling::default(),
            256,
            256,
        )
        .map(Arc::new);
    }

    /// The view transform for the picture presenting at `pts`, if the project
    /// established one for the clip playing then.
    fn lut_for(
        &self,
        pts: ucg_core::RationalTime,
    ) -> Option<std::sync::Arc<cine_color::lut::Lut3D>> {
        let viewing = self.viewing.as_ref()?;
        let entry = self
            .sequence
            .as_ref()?
            .entries
            .iter()
            .position(|e| e.record.contains(pts))?;
        viewing.luts.get(entry).cloned().flatten()
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
                if let (Some(sound), Some(pts)) = (
                    self.sound.as_mut(),
                    self.playback
                        .as_ref()
                        .and_then(|s| s.source().pts_of(p.index)),
                ) {
                    sound.observe(pts);
                }
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

    /// Everything the interface shows, assembled in one place from the
    /// scheduler and the decoder, so no number on screen has a second source.
    fn status(&self) -> ViewerStatus {
        let Some(p) = self.playback.as_ref() else {
            return ViewerStatus::default();
        };
        let s = p.stats();
        let a = p.source().stats();
        let note = self.sequence.as_ref().and_then(|seq| {
            let index = p
                .on_screen()
                .or_else(|| p.source().index_at(p.position()))?;
            cine_media::edit_list::why_black(&seq.entries, seq.rate, index)
        });
        ViewerStatus {
            note,
            media: match &self.sequence {
                Some(seq) => Some(format!("{} (sequence)", seq.name)),
                None => self.args.media.as_ref().map(|m| m.display().to_string()),
            },
            state: Some(p.state()),
            position: Some(p.position()),
            picture: p.on_screen(),
            pictures: p.source().frame_count(),
            presented: s.presented,
            dropped: s.dropped,
            late: s.late,
            underrun_episodes: s.underrun_episodes,
            decode_mean: a.mean_decode(),
            buffered: p.source().buffered().len(),
            buffer_bytes: a.bytes,
            colour: self.colour.clone(),
            scopes: self.scopes.clone(),
            view: self
                .viewing
                .as_ref()
                .map(|v| (format!("{} / {}", v.display, v.view), v.worst_error)),
            keyframes: self.keyframes.clone(),
        }
    }

    /// Act on what the editor asked for.
    fn apply(&mut self, command: Command) {
        if self.playback.is_none() {
            return;
        }
        match command {
            Command::TogglePlay => {
                let advancing = self
                    .playback
                    .as_ref()
                    .is_some_and(|p| p.state().is_advancing());
                if advancing || self.preroll.is_some() {
                    if let Some(p) = self.playback.as_mut() {
                        p.clock_mut().pause();
                    }
                    self.preroll = None;
                } else {
                    let reverse = self
                        .playback
                        .as_ref()
                        .is_some_and(|p| p.clock().rate().is_reverse());
                    self.start_play(reverse);
                }
            }
            Command::Play => self.start_play(false),
            Command::PlayReverse => self.start_play(true),
            Command::Pause => {
                if let Some(p) = self.playback.as_mut() {
                    p.clock_mut().pause();
                }
                self.preroll = None;
            }
            Command::Stop => {
                if let Some(p) = self.playback.as_mut() {
                    p.clock_mut().stop();
                }
                self.preroll = None;
            }
            Command::Step(by) => {
                self.preroll = None;
                if let Some(p) = self.playback.as_mut() {
                    let _ = p.step(by);
                }
            }
            Command::SeekNear(at) => {
                // The click was approximate; only the source knows where the
                // pictures actually are.
                self.preroll = None;
                if let Some(p) = self.playback.as_mut() {
                    let exact = p
                        .source()
                        .index_at(at)
                        .and_then(|index| p.source().pts_of(index));
                    if let Some(pts) = exact {
                        p.seek(pts);
                    }
                }
            }
            Command::ToggleLock(track) => self.toggle_track(track, |s| s.locked = !s.locked),
            Command::ToggleMute(track) => self.toggle_track(track, |s| s.muted = !s.muted),
            Command::ToggleSolo(track) => self.toggle_track(track, |s| s.solo = !s.solo),
            Command::ToggleTarget(track) => self.toggle_track(track, |s| s.targeted = !s.targeted),
        }
    }

    /// Change one track's editorial state.
    ///
    /// The state lives on the track, in OTIO metadata, so this is the same
    /// state `cine-timeline` reads and the same state that survives a save.
    fn toggle_track(&mut self, index: usize, change: impl FnOnce(&mut TrackState)) {
        // A project surface does not keep its own copy of the truth: the
        // change is committed through the project, and the redraw that follows
        // the broadcast is what shows it.
        if let (Some(link), Some(seq)) = (self.args.link.as_ref(), self.sequence.as_ref()) {
            let Some(track) = seq
                .timeline
                .tracks
                .children
                .iter()
                .filter_map(cine_timeline::Composable::as_track)
                .nth(index)
            else {
                return;
            };
            let mut state = TrackState::of(track);
            change(&mut state);
            if let Some(id) = seq.tracks.get(index) {
                if let Err(e) = link.set_track_state(*id, state) {
                    self.last_report = Some(format!("track state refused: {e}"));
                }
            }
            return;
        }
        let Some(timeline) = self.timeline.as_mut() else {
            return;
        };
        let Some(track) = timeline
            .tracks
            .children
            .iter_mut()
            .filter_map(cine_timeline::Composable::as_track_mut)
            .nth(index)
        else {
            return;
        };
        let mut state = TrackState::of(track);
        change(&mut state);
        state.apply(track);
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
        if let Some(sound) = &self.sound {
            println!("{}", sound.report());
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
        // Pre-roll starts from a stopped clock. It is also the only state in
        // which the clock may change which source it reads (see `sound`).
        if p.state().is_advancing() {
            p.clock_mut().pause();
        }
        if let Some(sound) = &self.sound {
            sound.stop(p.position());
        }
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
        // Sound leads at normal forward speed; at any other speed it cannot be
        // heard correctly, and the system clock paces the picture instead.
        if let Some(sound) = &self.sound {
            if p.clock().rate() == PlaybackRate::FORWARD {
                sound.play_from(p.position());
            }
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
        // The scopes measure the picture on screen, and the panels show what
        // they measured: both happen before anything is borrowed for drawing.
        let rebuilding = self.ui_dirty
            || self
                .ui_built
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(200));
        if rebuilding {
            self.measure_scopes();
        }
        // Gathered before anything is borrowed mutably: `status` reads the
        // whole of `self`, and the drawing below borrows four of its fields.
        let status = self.status();
        let picture_size = self.picture_size;
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
                if self.args.smoke_frames.is_some() || self.args.ui_frames.is_some() {
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
        presenter.draw_into(&surface.device, &surface.queue, &view, self.picture_rect);

        if let (Some(egui), Some(window)) = (self.egui.as_mut(), self.window.as_ref()) {
            let size = [surface.config.width, surface.config.height];
            let mut encoder =
                surface
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("workspace"),
                    });
            // The panels are rebuilt on input, on a transport change, and at
            // least a few times a second so the counters stay live. Otherwise
            // the cached interface is re-rendered over the new picture: the
            // timecode is redrawn as part of that rebuild cadence rather than
            // costing a full interface run per picture.
            let stale = self
                .ui_built
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(200));
            if self.ui_dirty || stale {
                let workspace = &mut self.workspace;
                let timeline = self.timeline.as_ref();
                let mut asked = Vec::new();
                let repaint_in = egui.draw(
                    window,
                    &surface.device,
                    &surface.queue,
                    &mut encoder,
                    &view,
                    size,
                    |ui| asked.extend(workspace.ui(ui, &status, timeline)),
                );
                self.egui_repaint_in = repaint_in;
                self.pending_commands = asked;
                self.ui_dirty = false;
                self.ui_built = Some(Instant::now());
                let ppp = egui.context().pixels_per_point();
                self.picture_rect = workspace
                    .program_rect()
                    .map(|r| letterbox(r, ppp, picture_size));
            } else {
                egui.redraw_last(&surface.device, &surface.queue, &mut encoder, &view);
            }
            surface.queue.submit(Some(encoder.finish()));
            self.ui_drawn += 1;
        }

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

    /// Load (or reload) the project's sequence as the transport's source.
    ///
    /// A reload keeps the playhead where it was and resumes playing if it was
    /// playing. Nothing is rebuilt when the cut did not change: a script edit
    /// elsewhere in the project must not interrupt playback.
    fn load_sequence(&mut self) {
        let Some(link) = self.args.link.as_ref() else {
            return;
        };
        let plan = link.sequence();
        let viewing = link.viewing();
        let same = match (&viewing, &self.viewing) {
            (Some(a), Some(b)) => a.same_as(b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            match &viewing {
                Some(v) => {
                    println!(
                        "view: {} / {} — {} (lattice within {:.4} of the transform)",
                        v.display,
                        v.view,
                        if v.describe.is_empty() {
                            "nothing to show it through".to_string()
                        } else {
                            v.describe.join("; ")
                        },
                        v.worst_error
                    );
                }
                None => println!("view: none — pictures are shown as decoded"),
            }
            self.viewing = viewing;
            self.ui_dirty = true;
        }
        if plan == self.sequence && self.playback.is_some() {
            return;
        }
        let resume = self.playback.as_ref().map(|p| {
            (
                p.position(),
                p.state().is_advancing() || self.preroll.is_some(),
                p.clock().rate().is_reverse(),
            )
        });
        self.playback = None;
        self.preroll = None;
        self.timeline = plan.as_ref().map(|p| p.timeline.clone());
        let Some(seq) = plan.clone() else {
            self.sequence = None;
            println!("project has no sequence to play");
            return;
        };
        let unplayable = seq
            .entries
            .iter()
            .filter(|e| e.unplayable.is_some())
            .count();
        println!(
            "sequence: {} — {} entries ({} unplayable), {} transitions played as cuts, rate {}/{}",
            seq.name,
            seq.entries.len(),
            unplayable,
            seq.transitions,
            seq.rate.num(),
            seq.rate.den()
        );
        let source = EditListSource::new(seq.rate, seq.end, seq.entries.clone());
        let Some((start, end)) = source.extent() else {
            self.sequence = Some(seq);
            println!("the sequence is empty");
            return;
        };
        // Rebuilt with the picture: the edit list changed, so the mix did too.
        self.sound = None;
        let time = match SoundEngine::open(seq.end, seq.sound.clone()) {
            Ok(engine) => {
                println!("sound: {}", engine.device);
                let t = engine.time();
                self.sound = Some(engine);
                t
            }
            Err(why) => {
                println!("sound: none — {why}");
                AppTime::system()
            }
        };
        let clock = PlaybackClock::new(time, start, end);
        let ahead = DecodeAhead::new(AnySource::Sequence(source), AheadConfig::default());
        let proxy = Mutex::new(self.proxy.clone());
        ahead.set_notify(move || {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(Wake::Picture);
            }
        });
        let mut playback = Scheduler::new(clock, ahead);
        if let Some((position, _, _)) = resume {
            playback.seek(position.min(end));
        }
        playback.poll();
        self.playback = Some(playback);
        self.sequence = Some(seq);
        self.colour = None;
        self.ui_dirty = true;
        if let Some((_, true, reverse)) = resume {
            self.start_play(reverse);
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
                self.keyframes = reader.keyframe_indices().to_vec();
                let clock = PlaybackClock::new(AppTime::system(), start, end);
                let ahead =
                    DecodeAhead::new(AnySource::File(Box::new(reader)), AheadConfig::default());
                // The worker wakes the event loop when a picture lands. A proxy
                // is `Send` but not `Sync`, hence the mutex.
                let proxy = Mutex::new(self.proxy.clone());
                ahead.set_notify(move || {
                    if let Ok(p) = proxy.lock() {
                        let _ = p.send_event(Wake::Picture);
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
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        if let Wake::Project = event {
            if self.args.link.as_ref().is_some_and(|l| l.should_quit()) {
                event_loop.exit();
                return;
            }
            self.load_sequence();
            self.ui_dirty = true;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

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
        // Evidence modes deliberately run without the interface: `--play`
        // measures the presentation path, and an interface drawn over it would
        // become part of the number.
        if self.args.with_ui || (self.args.smoke_frames.is_none() && self.args.play_secs.is_none())
        {
            self.egui = Some(EguiLayer::new(
                &window,
                &surface.device,
                surface.config.format,
            ));
        }
        self.surface = Some(surface);

        // Compute runs on the window's device, not a second one: a kernel's
        // result is then already on the device that will present it. The
        // adapter is the display's, chosen with `compatible_surface`.
        if self.args.dump.is_some() {
            self.compute = self.surface.as_ref().map(|s| s.compute_context());
        }

        if let Some(path) = self.args.timeline.clone() {
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| cine_timeline::read_otio(&text).map_err(|e| e.to_string()))
            {
                Ok(timeline) => {
                    let tracks = timeline.tracks.children.len();
                    println!("timeline: {} ({tracks} tracks)", path.display());
                    self.timeline = Some(timeline);
                }
                // A timeline that will not load is reported, not silently
                // replaced with an empty one that looks like an empty cut.
                Err(e) => eprintln!("cannot read {}: {e}", path.display()),
            }
        }

        if let Some(path) = self.args.media.clone() {
            self.open(&path);
        }

        if let Some(link) = self.args.link.as_ref() {
            window.set_title(&format!("CINE-OMNI — {}", link.name()));
            if let Some(address) = link.address() {
                println!("browser surface: {address}");
            }
            let proxy = Mutex::new(self.proxy.clone());
            link.subscribe(Box::new(move || {
                if let Ok(p) = proxy.lock() {
                    let _ = p.send_event(Wake::Project);
                }
            }));
            self.load_sequence();
        }

        for (name, availability) in ShellCapabilities::all() {
            match availability {
                crate::Availability::Working => {}
                crate::Availability::Degraded(why) => println!("degraded — {name}: {why}"),
                crate::Availability::Unavailable(why) => {
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
        if self
            .args
            .ui_frames
            .is_some_and(|limit| self.ui_drawn >= limit)
        {
            println!("UI SMOKE OK: drew {} workspace frames", self.ui_drawn);
            self.report();
            event_loop.exit();
            return;
        }

        for command in std::mem::take(&mut self.pending_commands) {
            self.apply(command);
        }

        // Sound follows the picture's transport: whatever stopped the clock —
        // a pause, a step, a seek, the end of the sequence — stops the sound.
        if let (Some(sound), Some(p)) = (self.sound.as_ref(), self.playback.as_ref()) {
            if sound.is_playing() && !p.state().is_advancing() {
                sound.stop(p.position());
            }
        }

        let prerolling = self.advance_preroll();

        // Pump in *every* mode, before deciding how to wait. After a step, a
        // seek or opening the file, the due picture may have just arrived from
        // the worker; when nothing has changed this is a `Hold` and costs
        // nothing. An earlier version returned before this in the interface
        // modes, which drew the panels perfectly around a black rectangle.
        if !prerolling && self.pump() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }

        // Anything that drives frames as fast as it can. `--play --with-ui` is
        // deliberately *not* one of them: it must keep waking on the
        // scheduler's deadline, or it would measure a spin rather than playback.
        let driving = self.args.smoke_frames.is_some() || self.args.ui_frames.is_some();
        if driving {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }

        if prerolling {
            // The worker's notification wakes us.
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        // Wake for whichever comes first: the next picture, or the next thing
        // the interface needs to animate.
        //
        // **While the transport is advancing, the picture has priority.** egui
        // asks for a repaint continuously while anything is animating — a
        // hovered tab, a resize handle — and honouring that during playback
        // redrew 44 times a second at ~17 ms each, starved the decoder on two
        // cores, and dropped 46% of the pictures. The interface is redrawn with
        // the picture instead, which is 24 times a second here and is also
        // exactly often enough: the timecode changes once per frame. Paused,
        // egui's own timing is honoured in full, so hovering stays smooth.
        let advancing = self
            .playback
            .as_ref()
            .is_some_and(|p| p.state().is_advancing());
        let next = self.playback.as_ref().and_then(|p| p.next_deadline());
        let wake = match (next, self.egui_repaint_in) {
            (Some(d), _) if advancing => Some(d),
            (Some(d), r) => Some(d.min(r)),
            (None, r) if r < Duration::MAX => Some(r),
            // Paused with a still interface: nothing to wake for. Starved: the
            // worker wakes us. A viewer that polls in either case burns a core
            // to show a still frame.
            (None, _) => None,
        };
        if wake.is_some_and(|w| w.is_zero()) {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        event_loop.set_control_flow(match wake {
            Some(d) => ControlFlow::WaitUntil(Instant::now() + d),
            None => ControlFlow::Wait,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // The interface gets first refusal on every event, so a keystroke meant
        // for a text field does not also drive the transport.
        //
        // The redraw request deliberately excludes `RedrawRequested`, which is
        // itself a window event: asking for a redraw while handling one is an
        // unconditional spin. It drew 76 times a second, competed with the
        // decode worker for two cores, and cost playback 38% of its pictures —
        // all of it invisible except in the counters.
        let mut dirty = false;
        let consumed = match (self.egui.as_mut(), self.window.as_ref()) {
            (Some(egui), Some(window)) => {
                let consumed = egui.on_window_event(window, &event);
                if !matches!(event, WindowEvent::RedrawRequested) {
                    window.request_redraw();
                    dirty = true;
                }
                consumed
            }
            _ => false,
        };
        self.ui_dirty |= dirty;
        if consumed && !matches!(event, WindowEvent::CloseRequested | WindowEvent::Resized(_)) {
            return;
        }
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

/// What a finished run reports, for the evidence modes and for callers.
pub struct RunReport {
    pub presented: u32,
    pub ui_drawn: u32,
    pub failure: Option<String>,
}

/// Open the window and run until it closes.
pub fn run(options: ShellOptions) -> std::result::Result<RunReport, String> {
    let smoke = options.smoke_frames.is_some();
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| format!("no display available: {e}"))?;
    // Smoke mode drives surface frames as fast as it can. Everything else is
    // driven by the scheduler through `about_to_wait`.
    event_loop.set_control_flow(if smoke {
        ControlFlow::Poll
    } else {
        ControlFlow::Wait
    });
    let proxy = event_loop.create_proxy();
    let mut app = App::new(options, proxy);
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop failed: {e}"))?;
    Ok(RunReport {
        presented: app.presented,
        ui_drawn: app.ui_drawn,
        failure: app.failure,
    })
}
